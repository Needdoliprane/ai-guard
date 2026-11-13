//! Retain unknown resource owners; close Core only with exact Worker terminal evidence.
use super::*;

impl RelayRegistry {
    async fn join_entry(entry: &Arc<Entry>) -> Result<()> {
        let mut handle = entry.task.lock().await;
        if let Some(task) = handle.as_mut() {
            task.await.map_err(|_| RelayOwnerError::Unknown)?;
            handle.take();
        }
        Ok(())
    }

    /// Terminal evidence is read from the same verified journal after the real
    /// Worker Observe commit. Neither EOF nor a caller boolean reaches Core.
    pub(crate) async fn worker_terminal(&self, job: &AcceptedJobV1) -> Result<()> {
        if !self.configured.contains_key(job.scope().profile_id()) {
            return Ok(());
        }
        let operations = job
            .broker_checkpoint_v2()
            .ok_or(RelayOwnerError::Scope)?
            .supervision()
            .operations();
        let worker = operations
            .iter()
            .rev()
            .find_map(|operation| match operation {
                SupervisionOperationV1::WorkerMutation {
                    response: Some(result),
                    ..
                } => Some(&result.worker),
                _ => None,
            })
            .ok_or(RelayOwnerError::Scope)?;
        if worker.state != WorkerStateV1::Terminal || worker.quota_held {
            return Err(RelayOwnerError::Scope);
        }
        let entry = self
            .entries
            .lock()
            .map_err(|_| RelayOwnerError::Unavailable)?
            .get(&(job.scope().tenant_id().into(), job.operation_id()))
            .cloned();
        let Some(entry) = entry else {
            return Ok(());
        };
        if crate::broker::journal::canonical(&entry.source.scope).ok()
            != crate::broker::journal::canonical(job.scope()).ok()
        {
            return Err(RelayOwnerError::Scope);
        }
        entry.stop.send_replace(true);
        tokio::time::timeout(Duration::from_secs(5), Self::join_entry(&entry))
            .await
            .map_err(|_| RelayOwnerError::Unknown)??;
        if entry.source.worker.worker_id != worker.worker_id
            || entry.source.worker.attempt.attempt_id != worker.attempt.attempt_id
        {
            return Err(RelayOwnerError::Scope);
        }
        let mut saved = entry.retained.lock().await;
        if entry.no_channel.load(Ordering::SeqCst) {
            return Ok(());
        }
        if saved.is_none() {
            *saved = self.reload_retained_history(&entry).await?;
            if saved.is_none() {
                return Ok(());
            }
        }
        let Some(retained) = saved.as_mut() else {
            return Err(RelayOwnerError::Unknown);
        };
        if retained.fact.endpoint != endpoint(worker) {
            return Err(RelayOwnerError::Scope);
        }
        if let Some(transport) = retained.transport.as_mut() {
            if !transport.close_and_join().await.local_joined {
                return Err(RelayOwnerError::Unknown);
            }
            if !retained.terminal_observed {
                transport
                    .reconcile_stopped(
                        &retained
                            .source
                            .execution
                            .as_ref()
                            .ok_or(RelayOwnerError::Unknown)?
                            .runtime,
                    )
                    .await
                    .map_err(|_| RelayOwnerError::Unknown)?;
                retained.terminal_observed = true;
            }
        } else if !retained.fact.local_joined {
            // Recovery has no locally owned Child to join. Do not invent proof
            // of the previous process's CLI termination from absent handles.
            return Err(RelayOwnerError::Unknown);
        }
        retained.fact.local_joined = true;
        let observation = worker
            .last_observation
            .as_ref()
            .ok_or(RelayOwnerError::Scope)?;
        // The Store records worker_allocations.terminal_observation_id from
        // WorkerMutationV1.operation_id, not from RuntimeObservationV1. Keep
        // that exact acknowledged Observe request and its correlated receipt.
        let mut terminal = operations.iter().filter_map(|operation| match operation {
            SupervisionOperationV1::WorkerMutation { request, response: Some(result) }
                if matches!(&request.transition, crate::control::WorkerTransitionV1::Observe { observation: given } if given == observation)
                    && request.worker_id == worker.worker_id
                    && request.attempt_id == worker.attempt.attempt_id
                    && result.worker.worker_id == worker.worker_id
                    && result.worker.attempt.attempt_id == worker.attempt.attempt_id
                    && result.worker.state == WorkerStateV1::Terminal
                    && !result.worker.quota_held
                    && result.worker.last_observation.as_ref() == Some(observation) => Some((request, result)),
            _ => None,
        });
        let (terminal_request, terminal_result) = terminal.next().ok_or(RelayOwnerError::Scope)?;
        if terminal.next().is_some() {
            return Err(RelayOwnerError::Scope);
        }
        self.retire_confirmed_terminal(&entry, retained, worker, terminal_request, terminal_result)
            .await?;
        saved.take();
        Ok(())
    }

    /// The only setup task has already been joined by the production caller.
    /// An unread/historical claim keeps the conservative handoff marker and
    /// cannot acquire local join evidence merely because no Child is retained.
    async fn reload_retained_history(&self, entry: &Arc<Entry>) -> Result<Option<Retained>> {
        let tenant = self.tenant(&entry.source.scope)?;
        let io = tokio::time::timeout(Duration::from_secs(5), self.tenant_io(&tenant))
            .await
            .map_err(|_| RelayOwnerError::Unknown)??;
        tokio::time::timeout(Duration::from_secs(5), io.join_pending())
            .await
            .map_err(|_| RelayOwnerError::Unknown)?
            .map_err(|_| RelayOwnerError::Unknown)?;
        let Some(history) = io
            .channel_history(entry.source.job_id)
            .await
            .map_err(|_| RelayOwnerError::Unknown)?
        else {
            entry.no_channel.store(true, Ordering::SeqCst);
            *entry
                .error
                .lock()
                .map_err(|_| RelayOwnerError::Unavailable)? = None;
            return Ok(None);
        };
        let mut fact = history.fact;
        if !entry.handoff_started.load(Ordering::SeqCst) {
            fact.local_joined = true;
        }
        Ok(Some(Retained {
            source: entry.source.clone(),
            tenant,
            io,
            fact,
            _authority: None,
            transport: None,
            owner: None,
            terminal_observed: false,
        }))
    }

    /// The production caller has already selected the unique terminal Observe
    /// from its verified job and proved the admissible local join above. Keep
    /// the exact receipt checks and cleanup in this shared post-terminal phase.
    async fn retire_confirmed_terminal(
        &self,
        entry: &Arc<Entry>,
        retained: &mut Retained,
        worker: &WorkerV1,
        terminal_request: &crate::control::WorkerMutationV1,
        terminal_result: &crate::control::MutationResultV1,
    ) -> Result<()> {
        if terminal_result.receipt.operation_id != terminal_request.operation_id
            || terminal_result.receipt.worker_id.as_deref() != Some(worker.worker_id.as_str())
            || terminal_result.receipt.revision != Some(terminal_result.worker.revision)
            || terminal_request.expected_revision.checked_add(1)
                != Some(terminal_result.worker.revision)
            || terminal_result.receipt.audit_seq <= 0
            || crate::control::hash(&terminal_result.receipt.audit_hash).is_err()
            || crate::control::content_hash("ai-guard:worker-mutation:v1", terminal_request)
                .ok()
                .as_ref()
                != Some(&terminal_result.receipt.input_hash)
        {
            return Err(RelayOwnerError::Scope);
        }
        self.retire_channel_history(entry, retained, worker, terminal_request, terminal_result)
            .await
    }

    pub(crate) fn status(&self, scope: &JournalScopeV1, job: Uuid) -> Result<RelayStatusV1> {
        let configured = self.configured.contains_key(scope.profile_id());
        let entry = self
            .entries
            .lock()
            .map_err(|_| RelayOwnerError::Unavailable)?
            .get(&(scope.tenant_id().into(), job))
            .cloned();
        if let Some(entry) = entry.as_ref() {
            if crate::broker::journal::canonical(&entry.source.scope).ok()
                != crate::broker::journal::canonical(scope).ok()
            {
                return Err(RelayOwnerError::Scope);
            }
        }
        let fact = entry
            .as_ref()
            .map(|entry| {
                entry
                    .view
                    .lock()
                    .map(|fact| fact.clone())
                    .map_err(|_| RelayOwnerError::Unavailable)
            })
            .transpose()?
            .flatten();
        let error = entry
            .as_ref()
            .map(|entry| {
                entry
                    .error
                    .lock()
                    .map(|error| *error)
                    .map_err(|_| RelayOwnerError::Unavailable)
            })
            .transpose()?
            .flatten()
            .or_else(|| fact.as_ref().and_then(|fact| fact.error))
            .or_else(|| {
                fact.as_ref()
                    .filter(|fact| pending_retirement(fact))
                    .map(|_| RelayOwnerError::Unknown)
            });
        let no_channel = entry
            .as_ref()
            .is_some_and(|entry| entry.no_channel.load(Ordering::SeqCst));
        Ok(RelayStatusV1 {
            version: 1,
            operation_id: job,
            tenant_id: scope.tenant_id().into(),
            configured,
            source: RelayStatusSourceV1::BrokerRetainedRegistry,
            freshness: if fact.is_some() {
                RelayStatusFreshnessV1::RetainedAtLastObservation
            } else {
                RelayStatusFreshnessV1::NotObserved
            },
            phase: fact
                .as_ref()
                .map(|fact| attachment_status_phase(fact, error))
                .or(if no_channel {
                    Some(RelayAttachmentPhaseV1::Closed)
                } else if error.is_some() {
                    Some(RelayAttachmentPhaseV1::Unknown)
                } else {
                    None
                }),
            channel_id: fact.as_ref().and_then(|fact| {
                fact.admission
                    .as_ref()
                    .map(|admission| admission.channel_id.clone())
            }),
            endpoint: entry.as_ref().map(|entry| endpoint(&entry.source.worker)),
            epoch: fact
                .as_ref()
                .and_then(|fact| fact.admission.as_ref().map(|admission| admission.epoch)),
            core_state: fact
                .as_ref()
                .and_then(|fact| fact.core.as_ref().map(|core| core.state)),
            last_observed_at: fact.as_ref().map(|fact| fact.observed_at.clone()),
            last_error: error.map(status_error).or(
                if configured && (fact.is_some() || no_channel) {
                    None
                } else {
                    Some(RelayStatusErrorV1::Unavailable)
                },
            ),
            local_joined: fact.as_ref().map(|fact| fact.local_joined),
            reconciliation_required: reconciliation_required(fact.as_ref(), error),
        })
    }
    pub(crate) fn close_admission(&self) -> Result<()> {
        self.stopping.store(true, Ordering::SeqCst);
        let entries: Vec<_> = self
            .entries
            .lock()
            .map_err(|_| RelayOwnerError::Unavailable)?
            .values()
            .cloned()
            .collect();
        for entry in &entries {
            entry.stop.send_replace(true);
        }
        Ok(())
    }
    pub(crate) async fn shutdown(&self) -> Result<()> {
        self.close_admission()?;
        let entries: Vec<_> = self
            .entries
            .lock()
            .map_err(|_| RelayOwnerError::Unavailable)?
            .values()
            .cloned()
            .collect();
        let tenants: Vec<_> = self
            .tenants
            .lock()
            .map_err(|_| RelayOwnerError::Unavailable)?
            .values()
            .cloned()
            .collect();
        let drain = async {
            for entry in &entries {
                Self::join_entry(entry).await?;
            }
            for tenant in tenants {
                // Join only an already admitted initializer. Shutdown never
                // retries a failed open or creates another filesystem owner.
                let mut init = tenant.init.lock().await;
                if let Some(task) = init.as_mut() {
                    let result = task.await;
                    init.take();
                    match result {
                        Ok(Ok(io)) => {
                            *tenant.io.lock().map_err(|_| RelayOwnerError::Unavailable)? = Some(io)
                        }
                        _ => {
                            tenant.poisoned.store(true, Ordering::SeqCst);
                            return Err(RelayOwnerError::Unknown);
                        }
                    }
                }
                drop(init);
                if tenant.poisoned.load(Ordering::SeqCst) {
                    return Err(RelayOwnerError::Unknown);
                }
                let io = tenant
                    .io
                    .lock()
                    .map_err(|_| RelayOwnerError::Unavailable)?
                    .clone();
                if let Some(io) = io {
                    io.join_pending()
                        .await
                        .map_err(|_| RelayOwnerError::Unknown)?;
                }
            }
            for entry in entries {
                let mut retained = entry.retained.lock().await;
                let closed_view = entry
                    .view
                    .lock()
                    .map_err(|_| RelayOwnerError::Unavailable)?
                    .as_ref()
                    .is_some_and(|fact| fact.phase == AttachmentPhase::Closed);
                if closed_view
                    || retained
                        .as_ref()
                        .is_some_and(|owner| owner.fact.phase == AttachmentPhase::Closed)
                {
                    // Use only an already initialized tenant owner. Shutdown
                    // never creates one to validate a cached Closed view.
                    let tenant = self
                        .tenants
                        .lock()
                        .map_err(|_| RelayOwnerError::Unavailable)?
                        .get(entry.source.scope.tenant_id())
                        .cloned()
                        .ok_or(RelayOwnerError::Unknown)?;
                    let io = tenant
                        .io
                        .lock()
                        .map_err(|_| RelayOwnerError::Unavailable)?
                        .clone()
                        .ok_or(RelayOwnerError::Unknown)?;
                    let history = io
                        .channel_history(entry.source.job_id)
                        .await
                        .map_err(|_| RelayOwnerError::Unknown)?
                        .ok_or(RelayOwnerError::Unknown)?;
                    if history.fact.phase != AttachmentPhase::Closed || !history.fact.local_joined {
                        return Err(RelayOwnerError::Unknown);
                    }
                    self.publish_history_view(&entry, &history)?;
                    if let Some(owner) = retained.as_mut() {
                        owner.fact = history.fact;
                    }
                }
                if let Some(owner) = retained.as_mut() {
                    if let Some(transport) = owner.transport.as_mut() {
                        if !transport.close_and_join().await.local_joined {
                            return Err(RelayOwnerError::Unknown);
                        }
                    }
                    if owner.fact.phase != AttachmentPhase::Closed {
                        return Err(RelayOwnerError::Unknown);
                    }
                }
                if entry
                    .error
                    .lock()
                    .map_err(|_| RelayOwnerError::Unavailable)?
                    .is_some()
                    || entry
                        .view
                        .lock()
                        .map_err(|_| RelayOwnerError::Unavailable)?
                        .as_ref()
                        .is_some_and(|fact| fact.phase != AttachmentPhase::Closed)
                {
                    return Err(RelayOwnerError::Unknown);
                }
            }
            Ok::<(), RelayOwnerError>(())
        };
        tokio::time::timeout(Duration::from_secs(5), drain)
            .await
            .map_err(|_| RelayOwnerError::Unknown)??;
        self.retention
            .lock()
            .map_err(|_| RelayOwnerError::Unavailable)?
            .take();
        Ok(())
    }
}

fn prepare_retirement_intent(fact: &mut ChannelFact) {
    fact.local_joined = true;
    fact.phase = AttachmentPhase::Unknown;
    fact.error = fact.error.or(Some(RelayOwnerError::Unknown));
}

fn attachment_status_phase(
    fact: &ChannelFact,
    error: Option<RelayOwnerError>,
) -> RelayAttachmentPhaseV1 {
    if pending_retirement(fact) {
        return RelayAttachmentPhaseV1::Unknown;
    }
    match fact.phase {
        AttachmentPhase::AwaitingIssuedWorkload => {
            if error.is_some() {
                RelayAttachmentPhaseV1::Unknown
            } else {
                RelayAttachmentPhaseV1::AwaitingIssuedWorkload
            }
        }
        AttachmentPhase::Admitted => {
            if error.is_some() {
                RelayAttachmentPhaseV1::Unknown
            } else {
                RelayAttachmentPhaseV1::Admitted
            }
        }
        AttachmentPhase::Unknown => RelayAttachmentPhaseV1::Unknown,
        AttachmentPhase::Closed => RelayAttachmentPhaseV1::Closed,
    }
}

fn reconciliation_required(fact: Option<&ChannelFact>, error: Option<RelayOwnerError>) -> bool {
    error.is_some()
        || fact
            .is_some_and(|fact| fact.phase == AttachmentPhase::Unknown || pending_retirement(fact))
}

fn pending_retirement(fact: &ChannelFact) -> bool {
    fact.phase != AttachmentPhase::Closed
        && fact.transition.as_ref().is_some_and(|request| {
            super::attachment::terminal_transition_state(&request.transition).is_some()
        })
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod io_tests;

#[cfg(test)]
mod history_tests;

mod history;
