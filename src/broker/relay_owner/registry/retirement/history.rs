//! Post-terminal historical cleanup. No exec claim is reconstructed or replayed.
use super::*;
use crate::broker::relay_owner::journal::{
    ChannelChangeV2, ChannelHistoryV2, ChannelOperationPurposeV2, ChannelTerminalV2,
};

impl RelayRegistry {
    pub(super) fn publish_history_view(
        &self,
        entry: &Arc<Entry>,
        history: &ChannelHistoryV2,
    ) -> Result<()> {
        require_source_history(&entry.source, history)?;
        *entry
            .view
            .lock()
            .map_err(|_| RelayOwnerError::Unavailable)? = Some(history.fact.clone());
        *entry
            .error
            .lock()
            .map_err(|_| RelayOwnerError::Unavailable)? = history.fact.error;
        Ok(())
    }

    pub(super) fn publish_channel_history(
        &self,
        entry: &Arc<Entry>,
        retained: &mut Retained,
        history: &ChannelHistoryV2,
    ) -> Result<()> {
        self.publish_history_view(entry, history)?;
        retained.fact = history.fact.clone();
        Ok(())
    }

    async fn change_channel_history(
        &self,
        entry: &Arc<Entry>,
        retained: &mut Retained,
        change: ChannelChangeV2,
    ) -> Result<ChannelHistoryV2> {
        let literals = retained
            .tenant
            .literals
            .lock()
            .map_err(|_| RelayOwnerError::Unavailable)?
            .clone();
        let history = match retained
            .io
            .save_channel_change(retained.source.job_id, change, literals)
            .await
        {
            Ok(history) => history,
            Err(_) => {
                if let Ok(mut error) = entry.error.lock() {
                    *error = retained
                        .fact
                        .error
                        .or(*error)
                        .or(Some(RelayOwnerError::Unknown));
                }
                return Err(RelayOwnerError::Unknown);
            }
        };
        self.publish_channel_history(entry, retained, &history)?;
        Ok(history)
    }

    pub(super) async fn retire_channel_history(
        &self,
        entry: &Arc<Entry>,
        retained: &mut Retained,
        worker: &WorkerV1,
        terminal_request: &crate::control::WorkerMutationV1,
        terminal_result: &crate::control::MutationResultV1,
    ) -> Result<()> {
        if !retained.fact.local_joined {
            return Err(RelayOwnerError::Unknown);
        }
        let terminal = terminal_from_job(worker, terminal_request, terminal_result)?;
        tokio::time::timeout(Duration::from_secs(5), retained.io.join_pending())
            .await
            .map_err(|_| RelayOwnerError::Unknown)?
            .map_err(|_| RelayOwnerError::Unknown)?;
        let mut history = retained
            .io
            .channel_history(retained.source.job_id)
            .await
            .map_err(|_| RelayOwnerError::Unknown)?
            .ok_or(RelayOwnerError::Unknown)?;
        require_source_history(&retained.source, &history)?;
        if history.fact.phase == AttachmentPhase::Closed {
            if !history.fact.local_joined {
                return Err(RelayOwnerError::Unknown);
            }
            if !history.legacy_closed {
                require_same_terminal(history.terminal.as_ref(), &terminal, true)?;
            }
            // Full-prefix validation has already established this durable
            // closure. A legacy Closed remains explicitly historical.
            self.publish_channel_history(entry, retained, &history)?;
            return Ok(());
        }
        if history.terminal.is_some() {
            require_same_terminal(history.terminal.as_ref(), &terminal, false)?;
            self.publish_channel_history(entry, retained, &history)?;
        } else {
            let mut fact = history.fact.clone();
            fact.error = fact.error.or(*entry
                .error
                .lock()
                .map_err(|_| RelayOwnerError::Unavailable)?);
            prepare_retirement_intent(&mut fact);
            fact.observed_at = chrono::Utc::now().to_rfc3339();
            history = self
                .change_channel_history(
                    entry,
                    retained,
                    ChannelChangeV2::BeginRetirement {
                        fact,
                        terminal: terminal.clone(),
                    },
                )
                .await?;
        }

        if history.fact.admission.is_none() {
            // The index, not the last fact, must establish the local branch.
            if history.fact.claim.is_some()
                || history.fact.transition.is_some()
                || history.fact.core.is_some()
                || !history.operations.is_empty()
                || history.admission_proof.is_some()
            {
                return Err(RelayOwnerError::Unknown);
            }
            return self
                .finalize_channel_history(entry, retained, &history)
                .await;
        }
        let admission = history
            .fact
            .admission
            .clone()
            .ok_or(RelayOwnerError::Unknown)?;

        // One successful cleanup is terminal. The extra rounds allow the
        // exact historical resolutions after that commit; no unchanged poll
        // is retried and no response loss creates another operation UUID.
        for _ in 0..4 {
            let pending: Vec<_> = history
                .operations
                .iter()
                .filter(|operation| operation.resolution.is_none())
                .map(|operation| operation.request.clone())
                .collect();
            if pending.is_empty() {
                history = self
                    .lookup_channel_history(entry, retained, &admission, None, &terminal)
                    .await?;
            } else {
                for request in pending {
                    if history.operations.iter().any(|operation| {
                        operation.request.operation_id() == request.operation_id()
                            && operation.resolution.is_some()
                    }) {
                        continue;
                    }
                    history = self
                        .lookup_channel_history(
                            entry,
                            retained,
                            &admission,
                            Some(request),
                            &terminal,
                        )
                        .await?;
                }
            }
            let all_resolved = history
                .operations
                .iter()
                .all(|operation| operation.resolution.is_some());
            let terminal_core = history
                .fact
                .core
                .as_ref()
                .is_some_and(|core| is_core_terminal(core.state));
            let never_admitted = history.admission_proof.as_ref().is_some_and(|proof| {
                matches!(
                    proof.outcome,
                    RelayChannelHistoryOutcomeV1::AdmissionNeverRecordedExactTerminal { .. }
                )
            });
            if all_resolved && (terminal_core || never_admitted) {
                return self
                    .finalize_channel_history(entry, retained, &history)
                    .await;
            }
            let core = history.fact.core.clone().ok_or(RelayOwnerError::Unknown)?;
            let kind = match core.state {
                RelayChannelStateV1::Reserved => RelayChannelTransitionKindV1::CancelReserved,
                RelayChannelStateV1::AcceptedBeforeExec
                | RelayChannelStateV1::Ready
                | RelayChannelStateV1::Closing
                | RelayChannelStateV1::Unknown => {
                    RelayChannelTransitionKindV1::CloseExactWorkerTerminal {
                        worker_revision: worker.revision,
                        terminal_observation_id: terminal_request.operation_id.clone(),
                    }
                }
                // For example, a structurally valid legacy future CAS can
                // remain unresolved at a terminal state. Do not invent proof.
                _ => return Err(RelayOwnerError::Unknown),
            };
            let cleanups: Vec<_> = history
                .operations
                .iter()
                .filter(|operation| {
                    operation.purpose == ChannelOperationPurposeV2::Retirement
                        && operation.resolution.is_none()
                })
                .collect();
            if cleanups.len() > 1 {
                return Err(RelayOwnerError::Unknown);
            }
            let request = if let Some(operation) = cleanups.first() {
                let RelayChannelPendingV1::Transition(request) = &operation.request else {
                    return Err(RelayOwnerError::Unknown);
                };
                if request.expected_revision != core.revision || request.transition != kind {
                    return Err(RelayOwnerError::Unknown);
                }
                request.clone()
            } else {
                let request = RelayChannelTransitionV1 {
                    version: 1,
                    operation_id: Uuid::new_v4().to_string(),
                    channel_id: admission.channel_id.clone(),
                    epoch: admission.epoch,
                    expected_revision: core.revision,
                    admission_input_hash: admission
                        .input_hash()
                        .map_err(|_| RelayOwnerError::Scope)?,
                    transition: kind,
                };
                history = self
                    .change_channel_history(
                        entry,
                        retained,
                        ChannelChangeV2::RetirementIntent {
                            request: request.clone(),
                        },
                    )
                    .await?;
                if !history.operations.iter().any(|operation| {
                    operation.purpose == ChannelOperationPurposeV2::Retirement
                        && operation.request == RelayChannelPendingV1::Transition(request.clone())
                        && operation.resolution.is_none()
                }) {
                    return Err(RelayOwnerError::Unknown);
                }
                request
            };
            // RetirementIntent is durable and has reserved its result/final
            // envelope capacity before this effect. Only this cleanup may be
            // replayed; old Claim/Ready/Unknown intentions are lookup-only.
            let response = retained
                .source
                .client
                .transition_relay_channel(retained.source.scope.tenant_id(), &admission, &request)
                .await
                .map_err(|_| RelayOwnerError::Unknown)?;
            history = self
                .change_channel_history(
                    entry,
                    retained,
                    ChannelChangeV2::Mutation {
                        operation_id: request.operation_id,
                        response,
                    },
                )
                .await?;
        }
        Err(RelayOwnerError::Unknown)
    }

    async fn lookup_channel_history(
        &self,
        entry: &Arc<Entry>,
        retained: &mut Retained,
        admission: &RelayChannelAdmissionV1,
        pending: Option<RelayChannelPendingV1>,
        terminal: &ChannelTerminalV2,
    ) -> Result<ChannelHistoryV2> {
        let pending_operation_id = pending
            .as_ref()
            .map(|request| request.operation_id().to_owned());
        let request = RelayChannelReconcileV1 {
            version: 1,
            admission: admission.clone(),
            pending,
        };
        let response = retained
            .source
            .client
            .reconcile_relay_channel(&request)
            .await
            .map_err(|_| RelayOwnerError::Unknown)?;
        if let RelayChannelHistoryOutcomeV1::AdmissionNeverRecordedExactTerminal {
            worker_revision,
            terminal_observation_receipt,
            ..
        } = &response.outcome
            && (*worker_revision != terminal.worker_revision
                || terminal_observation_receipt != &terminal.receipt)
        {
            return Err(RelayOwnerError::Scope);
        }
        self.change_channel_history(
            entry,
            retained,
            ChannelChangeV2::History {
                pending_operation_id,
                response,
            },
        )
        .await
    }

    async fn finalize_channel_history(
        &self,
        entry: &Arc<Entry>,
        retained: &mut Retained,
        history: &ChannelHistoryV2,
    ) -> Result<()> {
        let mut fact = history.fact.clone();
        fact.phase = AttachmentPhase::Closed;
        fact.local_joined = true;
        fact.error = None;
        fact.observed_at = chrono::Utc::now().to_rfc3339();
        self.change_channel_history(entry, retained, ChannelChangeV2::Finalize { fact })
            .await?;
        Ok(())
    }
}

fn terminal_from_job(
    worker: &WorkerV1,
    request: &crate::control::WorkerMutationV1,
    result: &crate::control::MutationResultV1,
) -> Result<ChannelTerminalV2> {
    if worker.state != WorkerStateV1::Terminal
        || worker.quota_held
        || result.worker.state != WorkerStateV1::Terminal
        || result.worker.quota_held
        || request.worker_id != worker.worker_id
        || request.attempt_id != worker.attempt.attempt_id
        || result.worker.worker_id != worker.worker_id
        || result.worker.attempt.attempt_id != worker.attempt.attempt_id
        || worker.revision < result.worker.revision
        || worker.last_observation != result.worker.last_observation
        || !matches!(&request.transition, crate::control::WorkerTransitionV1::Observe { observation }
            if Some(observation) == worker.last_observation.as_ref())
    {
        return Err(RelayOwnerError::Scope);
    }
    Ok(ChannelTerminalV2 {
        request: request.clone(),
        receipt: result.receipt.clone(),
        worker_revision: worker.revision,
    })
}

fn require_same_terminal(
    stored: Option<&ChannelTerminalV2>,
    expected: &ChannelTerminalV2,
    already_closed: bool,
) -> Result<()> {
    let stored = stored.ok_or(RelayOwnerError::Unknown)?;
    if crate::broker::journal::canonical(&stored.request).map_err(|_| RelayOwnerError::Scope)?
        != crate::broker::journal::canonical(&expected.request)
            .map_err(|_| RelayOwnerError::Scope)?
        || stored.receipt != expected.receipt
        || if already_closed {
            stored.worker_revision > expected.worker_revision
        } else {
            stored.worker_revision != expected.worker_revision
        }
    {
        return Err(RelayOwnerError::Scope);
    }
    Ok(())
}

fn require_source_history(source: &Source, history: &ChannelHistoryV2) -> Result<()> {
    let fact = &history.fact;
    if fact.job_id != source.job_id
        || fact.endpoint != endpoint(&source.worker)
        || fact.source_profile_id != source.scope.profile_id()
        || fact.arm_id != source.deadline.arm.operation_id
        || crate::broker::journal::canonical(&fact.original_clock)
            .map_err(|_| RelayOwnerError::Scope)?
            != crate::broker::journal::canonical(&source.deadline.arm.clock)
                .map_err(|_| RelayOwnerError::Scope)?
        || fact.original_deadline_ns_decimal != source.deadline.arm.deadline_ns_decimal
    {
        return Err(RelayOwnerError::Scope);
    }
    Ok(())
}

fn is_core_terminal(state: RelayChannelStateV1) -> bool {
    matches!(
        state,
        RelayChannelStateV1::Closed
            | RelayChannelStateV1::CancelledBeforeClaim
            | RelayChannelStateV1::KnownRefusedBeforeSpawn
    )
}
