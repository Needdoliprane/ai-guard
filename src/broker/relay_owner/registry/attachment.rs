//! Attach only from the original verified Arm and one fresh Core exec claim.
use super::*;

impl RelayRegistry {
    pub(super) async fn prepare_entry(
        self: &Arc<Self>,
        entry: &Arc<Entry>,
        source: Arc<Source>,
        terminal_recovery: bool,
    ) -> Result<Option<Retained>> {
        let tenant = self.tenant(&source.scope)?;
        let io = if terminal_recovery {
            self.tenant_io(&tenant).await?
        } else {
            self.while_active(entry, &source, self.tenant_io(&tenant))
                .await??
        };
        let previous = if terminal_recovery {
            io.channel_history(source.job_id).await
        } else {
            self.while_active(entry, &source, io.channel_history(source.job_id))
                .await?
        }
        .map_err(|_| RelayOwnerError::Unknown)?;
        let mut fact = match previous {
            Some(history) => {
                let retiring = history.terminal.is_some();
                let fact = history.fact;
                if fact.endpoint != endpoint(&source.worker)
                    || fact.source_profile_id != source.scope.profile_id()
                    || fact.arm_id != source.deadline.arm.operation_id
                    || crate::broker::journal::canonical(&fact.original_clock).ok()
                        != crate::broker::journal::canonical(&source.deadline.arm.clock).ok()
                    || fact.original_deadline_ns_decimal != source.deadline.arm.deadline_ns_decimal
                {
                    return Err(RelayOwnerError::Scope);
                }
                // Only a complete, checked prefix with no submitted claim
                // proves no historical exec. An unread/recovered claim must
                // never inherit the current process's pre-handoff join path.
                entry
                    .handoff_started
                    .store(fact.claim.is_some(), Ordering::SeqCst);
                // A previous submitted claim may have executed. Recovered facts
                // never reconstruct the process-local FreshRelayExecClaim.
                if terminal_recovery
                    || retiring
                    || fact.claim.is_some()
                    || fact.phase == AttachmentPhase::Closed
                {
                    let mut fact = fact;
                    if terminal_recovery && fact.claim.is_none() {
                        // The immutable claim intent must precede even the
                        // ClaimExec HTTP call and every possible helper spawn.
                        // Its verified absence proves no local exec was issued;
                        // this does not infer a join for a submitted claim.
                        fact.local_joined = true;
                    }
                    if !terminal_recovery && !retiring && fact.phase != AttachmentPhase::Closed {
                        fact.phase = AttachmentPhase::Unknown;
                        fact.error = Some(RelayOwnerError::Unknown);
                        self.save(entry, &tenant, &io, &mut fact).await?;
                    } else {
                        *entry
                            .view
                            .lock()
                            .map_err(|_| RelayOwnerError::Unavailable)? = Some(fact.clone());
                    }
                    return Ok(Some(Retained {
                        source,
                        tenant,
                        io,
                        fact,
                        _authority: None,
                        transport: None,
                        owner: None,
                        terminal_observed: false,
                    }));
                }
                fact
            }
            None if terminal_recovery => {
                // The complete checked prefix has no attachment for this job.
                // Durable intent must precede Reserve/Claim, so there is no
                // local channel to finalize; do not invent a Closed V2 fact.
                return Ok(None);
            }
            None => {
                entry.handoff_started.store(false, Ordering::SeqCst);
                ChannelFact {
                    version: 1,
                    job_id: source.job_id,
                    source_profile_id: source.scope.profile_id().into(),
                    endpoint: endpoint(&source.worker),
                    source_head_revision: source.head_revision,
                    source_head_hash: source.head_hash.clone(),
                    arm_id: source.deadline.arm.operation_id.clone(),
                    original_clock: source.deadline.arm.clock.clone(),
                    original_deadline_ns_decimal: source.deadline.arm.deadline_ns_decimal.clone(),
                    phase: AttachmentPhase::AwaitingIssuedWorkload,
                    observed_at: chrono::Utc::now().to_rfc3339(),
                    error: None,
                    admission: None,
                    claim: None,
                    core: None,
                    transition: None,
                    local_joined: false,
                }
            }
        };
        self.while_active(entry, &source, self.save(entry, &tenant, &io, &mut fact))
            .await??;
        let mut stop = entry.stop.subscribe();
        let binding = loop {
            self.check_source(entry, &source)?;
            if *stop.borrow() || self.stopping.load(Ordering::SeqCst) {
                return Err(RelayOwnerError::Cancelled);
            }
            let paths = self
                .configured
                .get(source.scope.profile_id())
                .ok_or(RelayOwnerError::Unavailable)?;
            if let Some(binding) = self
                .while_active(
                    entry,
                    &source,
                    scan_issued_bindings(&io, paths, &source.scope, &fact.endpoint),
                )
                .await??
            {
                break binding;
            }
            wait_issued_binding_retry(&source.deadline, &mut stop).await?;
        };
        if chrono::DateTime::parse_from_rfc3339(binding.expires_at())
            .map_err(|_| RelayOwnerError::InvalidMaterial)?
            <= chrono::Utc::now()
        {
            // Workload expiry is a separate UTC authority check. It never
            // supplies or renews the original continuous-clock Worker budget.
            return Err(RelayOwnerError::Expired);
        }
        let proof = binding.credential_file();
        let literal = std::str::from_utf8(proof.bytes())
            .map_err(|_| RelayOwnerError::InvalidMaterial)?
            .trim_end_matches('\n')
            .to_owned();
        {
            let mut literals = tenant
                .literals
                .lock()
                .map_err(|_| RelayOwnerError::Unavailable)?;
            if !literals.contains(&literal) {
                literals.push(literal);
            }
        }
        self.check_source(entry, &source)?;
        self.while_active(entry, &source, io.check_binding(binding.clone()))
            .await?
            .map_err(|_| RelayOwnerError::InvalidMaterial)?;
        let execution = source.execution.as_ref().ok_or(RelayOwnerError::Scope)?;
        let observed = self
            .while_active(entry, &source, execution.runtime.observe(&source.handle))
            .await?
            .map_err(|_| RelayOwnerError::Unavailable)?;
        let owned = OwnedContainerSnapshot {
            handle: source.handle.clone(),
            observation: observed,
        };
        let inspection = crate::runner::relay_worker_inspection(&source.worker, &owned.observation)
            .map_err(|_| RelayOwnerError::Scope)?;
        let admission = fact
            .admission
            .get_or_insert_with(|| RelayChannelAdmissionV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                channel_id: Uuid::new_v4().to_string(),
                epoch: 1,
                control_namespace_uuid: source.scope.control_namespace_uuid().to_string(),
                endpoint: fact.endpoint.clone(),
                credential_id: binding.credential_id().into(),
                binding_revision: binding.revision(),
                credential_sha256_digest: binding.digest().into(),
                session_id: source.worker.launch.session_id.clone(),
                expected_worker_revision: source.worker.revision,
                inspection,
                started_at: owned
                    .observation
                    .process
                    .started_at
                    .as_ref()
                    .map(|at| at.to_rfc3339())
                    .unwrap_or_default(),
                runtime_source: RelayRuntimeSourceV1::BrokerReportedRuntime,
                authority_binding_hash: execution.runtime.binding().fingerprint(),
                configuration_fingerprint_version: owned
                    .observation
                    .configuration_fingerprint_version
                    .unwrap_or(0),
                configuration_fingerprint: owned
                    .observation
                    .configuration_fingerprint
                    .clone()
                    .unwrap_or_default(),
                expires_at: binding.expires_at().into(),
            })
            .clone();
        admission.validate().map_err(|_| RelayOwnerError::Scope)?;
        self.while_active(entry, &source, self.save(entry, &tenant, &io, &mut fact))
            .await??;
        self.check_source(entry, &source)?;
        let reserved = self
            .while_active(
                entry,
                &source,
                source
                    .client
                    .reserve_relay_channel(source.scope.tenant_id(), &admission),
            )
            .await?
            .map_err(|_| RelayOwnerError::Unknown)?;
        fact.core = Some(reserved.channel);
        fact.claim = Some(RelayChannelClaimV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            channel_id: admission.channel_id.clone(),
            epoch: admission.epoch,
            expected_revision: 1,
            admission_input_hash: admission.input_hash().map_err(|_| RelayOwnerError::Scope)?,
        });
        self.while_active(entry, &source, self.save(entry, &tenant, &io, &mut fact))
            .await??;
        self.check_source(entry, &source)?;
        self.while_active(entry, &source, io.check_binding(binding.clone()))
            .await?
            .map_err(|_| RelayOwnerError::InvalidMaterial)?;
        let claim = fact.claim.as_ref().ok_or(RelayOwnerError::Unknown)?;
        let fresh = match self
            .while_active(
                entry,
                &source,
                source
                    .client
                    .claim_relay_channel_exec(source.scope.tenant_id(), &admission, claim),
            )
            .await?
            .map_err(|_| RelayOwnerError::Unknown)?
        {
            RelayExecClaimResult::Fresh(fresh) => fresh,
            RelayExecClaimResult::Replayed(_) => {
                fact.phase = AttachmentPhase::Unknown;
                fact.error = Some(RelayOwnerError::Unknown);
                self.save(entry, &tenant, &io, &mut fact).await?;
                return Ok(Some(Retained {
                    source,
                    tenant,
                    io,
                    fact,
                    _authority: None,
                    transport: None,
                    owner: None,
                    terminal_observed: false,
                }));
            }
        };
        let current = fresh.current_authority();
        let snapshot = self
            .while_active(
                entry,
                &source,
                source.client.current_relay_channel(&current),
            )
            .await?
            .map_err(|_| RelayOwnerError::Unknown)?;
        fact.core = Some(snapshot);
        fact.phase = AttachmentPhase::Admitted;
        self.while_active(entry, &source, self.save(entry, &tenant, &io, &mut fact))
            .await??;
        // The consumed capability is handed to the real runtime factory below;
        // no frame, historical DTO or start guard can enter this path.
        entry.handoff_started.store(true, Ordering::SeqCst);
        self.run_handoff(
            entry,
            Retained {
                source,
                tenant,
                io,
                fact,
                _authority: None,
                transport: None,
                owner: None,
                terminal_observed: false,
            },
            binding,
            owned,
            fresh,
            current,
        )
        .await
        .map(Some)
    }
    pub(super) async fn save(
        &self,
        entry: &Arc<Entry>,
        tenant: &Arc<TenantState>,
        io: &Arc<TenantRelayIo>,
        fact: &mut ChannelFact,
    ) -> Result<()> {
        fact.observed_at = chrono::Utc::now().to_rfc3339();
        let literals = tenant
            .literals
            .lock()
            .map_err(|_| RelayOwnerError::Unavailable)?
            .clone();
        *fact = match io.save_channel(fact.clone(), literals).await {
            Ok(saved) => saved,
            Err(_) => {
                // A failed durable write cannot publish the candidate. Keep a
                // visible failure even if the last durable fact was waiting.
                if let Ok(mut error) = entry.error.lock() {
                    *error = fact.error.or(*error).or(Some(RelayOwnerError::Unknown));
                }
                return Err(RelayOwnerError::Unknown);
            }
        };
        *entry
            .view
            .lock()
            .map_err(|_| RelayOwnerError::Unavailable)? = Some(fact.clone());
        *entry
            .error
            .lock()
            .map_err(|_| RelayOwnerError::Unavailable)? = fact.error;
        Ok(())
    }

    pub(super) async fn transition(
        &self,
        entry: &Arc<Entry>,
        tenant: &Arc<TenantState>,
        io: &Arc<TenantRelayIo>,
        source: &Source,
        fact: &mut ChannelFact,
        kind: RelayChannelTransitionKindV1,
    ) -> Result<()> {
        let admission = fact.admission.clone().ok_or(RelayOwnerError::Unknown)?;
        // Reuse a transition whose acknowledgement was lost before requesting
        // a different effect. Its durable identity is never overwritten early.
        if let Some(previous) = fact.transition.clone() {
            if fact
                .core
                .as_ref()
                .is_some_and(|core| core.revision == previous.expected_revision)
            {
                let response = source
                    .client
                    .transition_relay_channel(source.scope.tenant_id(), &admission, &previous)
                    .await
                    .map_err(|_| RelayOwnerError::Unknown)?;
                let mut acknowledged = acknowledged_transition(fact, &previous, response.channel)?;
                self.save(entry, tenant, io, &mut acknowledged).await?;
                *fact = acknowledged;
                if previous.transition == kind || fact.phase == AttachmentPhase::Closed {
                    return Ok(());
                }
            }
        }
        let revision = fact
            .core
            .as_ref()
            .map(|core| core.revision)
            .ok_or(RelayOwnerError::Unknown)?;
        let request = match fact.transition.as_ref() {
            Some(previous)
                if previous.transition == kind && previous.expected_revision == revision =>
            {
                previous.clone()
            }
            _ => RelayChannelTransitionV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                channel_id: admission.channel_id.clone(),
                epoch: admission.epoch,
                expected_revision: revision,
                admission_input_hash: admission.input_hash().map_err(|_| RelayOwnerError::Scope)?,
                transition: kind.clone(),
            },
        };
        let mut intended = fact.clone();
        intended.transition = Some(request.clone());
        if terminal_transition_state(&request.transition).is_some() {
            intended.phase = AttachmentPhase::Unknown;
            intended.error = intended.error.or(Some(RelayOwnerError::Unknown));
        }
        self.save(entry, tenant, io, &mut intended).await?;
        *fact = intended;
        let response = source
            .client
            .transition_relay_channel(source.scope.tenant_id(), &admission, &request)
            .await
            .map_err(|_| RelayOwnerError::Unknown)?;
        let mut acknowledged = acknowledged_transition(fact, &request, response.channel)?;
        self.save(entry, tenant, io, &mut acknowledged).await?;
        *fact = acknowledged;
        Ok(())
    }

    async fn run_handoff(
        self: &Arc<Self>,
        entry: &Arc<Entry>,
        mut retained: Retained,
        binding: Arc<RetainedWorkerBinding>,
        owned: OwnedContainerSnapshot,
        fresh: crate::runner::relay_admission::FreshRelayExecClaim,
        current: CurrentRelayAuthority,
    ) -> Result<Retained> {
        let authority = Arc::new(DispatchAuthority {
            source: retained.source.clone(),
            tenant: retained.tenant.clone(),
            io: retained.io.clone(),
            binding,
            current,
            owned: owned.clone(),
            _role: self.role.clone(),
            registry: Arc::downgrade(self),
            entry: entry.clone(),
        });
        retained._authority = Some(authority.clone());
        let setup = async {
            let execution = retained
                .source
                .execution
                .as_ref()
                .ok_or(RelayOwnerError::Scope)?;
            let owner = crate::relay::channel::control::ControlOwner::from_broker(
                authority.clone(),
                retained.io.clone(),
            )
            .await
            .map_err(|_| RelayOwnerError::Unavailable)?;
            retained.owner = Some(owner);
            let verified = VerifiedRelayExecAdmission {
                fresh,
                owned,
                spec: execution.spec.clone(),
                authority,
            };
            let permit: crate::runtime::RelayExecPermit = execution
                .runtime
                .prepare_relay_exec(verified)
                .map_err(|_: crate::runtime::RelayExecError| RelayOwnerError::Scope)?;
            let transport = execution
                .runtime
                .open_relay_channel(permit)
                .await
                .map_err(|_| RelayOwnerError::Unknown)?;
            // Publish the complete owner immediately after spawn, before any
            // further await can fail. Drop never substitutes for actual join.
            retained.transport = Some(transport);
            Ok::<(), RelayOwnerError>(())
        }
        .await;
        let mut outcome = setup;
        if outcome.is_ok() {
            let (stop, stopped) = tokio::sync::oneshot::channel();
            let mut requested = entry.stop.subscribe();
            let mut bridge = tokio::spawn(async move {
                let stopping = *requested.borrow();
                if !stopping {
                    let _ = requested.changed().await;
                }
                let _ = stop.send(crate::relay::channel::CloseReasonV1::Shutdown);
            });
            outcome = match (retained.owner.as_mut(), retained.transport.as_mut()) {
                (Some(owner), Some(transport)) => match transport.io_pair() {
                    Ok((input, output)) => owner
                        .serve(input, output, stopped)
                        .await
                        .map_err(|error| match error {
                            crate::relay::channel::control::ControlError::AuthorityClockUnavailable => RelayOwnerError::Clock,
                            crate::relay::channel::control::ControlError::AuthorityCancelled => RelayOwnerError::Cancelled,
                            _ => RelayOwnerError::Unknown,
                        }),
                    Err(_) => Err(RelayOwnerError::Unknown),
                },
                _ => Err(RelayOwnerError::Unknown),
            };
            bridge.abort();
            let _ = (&mut bridge).await;
        }
        let joined_transport = match retained.transport.as_mut() {
            Some(transport) => transport.close_and_join().await.local_joined,
            None => true,
        };
        let joined_io = tokio::time::timeout(Duration::from_secs(5), retained.io.join_pending())
            .await
            .is_ok_and(|result| result.is_ok());
        if let Ok(view) = entry.view.lock() {
            if let Some(fact) = view.as_ref() {
                retained.fact = fact.clone();
            }
        }
        retained.fact.local_joined = joined_transport && joined_io;
        retained.fact.phase = AttachmentPhase::Unknown;
        retained.fact.error = Some(outcome.err().unwrap_or(RelayOwnerError::Unknown));
        let _ = self
            .save(entry, &retained.tenant, &retained.io, &mut retained.fact)
            .await;
        // Closing the local flow is not a worker terminal and never settles a
        // quota. Core Unknown is a distinct, durably requested transition.
        let _ = self
            .transition(
                entry,
                &retained.tenant,
                &retained.io,
                &retained.source,
                &mut retained.fact,
                RelayChannelTransitionKindV1::Unknown,
            )
            .await;
        Ok(retained)
    }
}

pub(super) fn acknowledged_transition(
    fact: &ChannelFact,
    request: &RelayChannelTransitionV1,
    core: RelayChannelSnapshotV1,
) -> Result<ChannelFact> {
    let mut acknowledged = fact.clone();
    if let Some(terminal) = terminal_transition_state(&request.transition) {
        if !fact.local_joined
            || core.state != terminal
            || request.expected_revision.checked_add(1) != Some(core.revision)
        {
            return Err(RelayOwnerError::Unknown);
        }
        acknowledged.phase = AttachmentPhase::Closed;
        acknowledged.error = None;
    } else {
        acknowledged.phase = transition_phase(&request.transition);
    }
    acknowledged.core = Some(core);
    Ok(acknowledged)
}

pub(super) fn terminal_transition_state(
    kind: &RelayChannelTransitionKindV1,
) -> Option<RelayChannelStateV1> {
    match kind {
        RelayChannelTransitionKindV1::CancelReserved => {
            Some(RelayChannelStateV1::CancelledBeforeClaim)
        }
        RelayChannelTransitionKindV1::KnownRefusedBeforeSpawn => {
            Some(RelayChannelStateV1::KnownRefusedBeforeSpawn)
        }
        RelayChannelTransitionKindV1::CloseExactWorkerTerminal { .. } => {
            Some(RelayChannelStateV1::Closed)
        }
        _ => None,
    }
}

// Keep the original Arm and stop generation while awaiting late-issued data.
// This seam is shared by prepare_entry and its bounded waiting regression.
async fn wait_issued_binding_retry(
    deadline: &WorkerRelayDeadline,
    stop: &mut watch::Receiver<bool>,
) -> Result<()> {
    tokio::select! {
        error = deadline.wait_expiration() => Err(error),
        _ = stop.changed() => Err(RelayOwnerError::Cancelled),
        _ = tokio::time::sleep(Duration::from_millis(250)) => Ok(()),
    }
}

#[cfg(test)]
mod deadline_cause_tests;

/// Historical readonly material only. This grants no Core or runtime authority;
/// the caller keeps the entire scan under the original Arm and STOP watcher.
async fn scan_issued_bindings(
    io: &TenantRelayIo,
    paths: &[PathBuf],
    scope: &JournalScopeV1,
    endpoint: &RelayEndpointV1,
) -> Result<Option<Arc<RetainedWorkerBinding>>> {
    let mut candidate = None;
    let mut pending = false;
    for path in paths {
        match io.issued_binding(path.clone(), scope.clone()).await {
            Ok(Some(binding)) if binding.endpoint() == endpoint => {
                if candidate.replace(binding).is_some() {
                    return Err(RelayOwnerError::Scope);
                }
            }
            Ok(Some(_)) => {}
            Ok(None) => pending = true,
            Err(crate::relay::channel::control::ControlError::Capacity) => return Ok(None),
            Err(crate::relay::channel::control::ControlError::NeedsReconciliation) => {
                return Err(RelayOwnerError::Unknown);
            }
            Err(_) => return Err(RelayOwnerError::InvalidMaterial),
        }
        // Bounded references should also share scheduler time. A retained
        // candidate is accepted only after every configured path was checked.
        tokio::task::yield_now().await;
    }
    if candidate.is_some() || pending {
        Ok(candidate)
    } else {
        Err(RelayOwnerError::Scope)
    }
}

#[cfg(test)]
mod scan_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[tokio::test]
    async fn real_io_capacity_after_a_candidate_requires_a_complete_new_scan() {
        let temporary = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap())
            .unwrap();
        let root = Directory::open_existing(temporary.path()).unwrap();
        let scope = JournalScopeV1::new(
            Uuid::from_u128(1),
            "tenant-a",
            "http://127.0.0.1:9",
            Uuid::from_u128(2),
            "fixture",
        )
        .unwrap();
        let endpoint = RelayEndpointV1 {
            worker_id: Uuid::from_u128(3).to_string(),
            attempt_id: Uuid::from_u128(4).to_string(),
            generation: 1,
        };
        let operations = root.persistent_child("operations", true).unwrap().unwrap();
        let first = crate::workload_cli::broker_binding::fixtures::issued_operation(
            &operations,
            "first",
            &scope,
            &endpoint,
        );
        let second = crate::workload_cli::broker_binding::fixtures::issued_operation(
            &operations,
            "second",
            &scope,
            &endpoint,
        );
        let business = root.persistent_child("business", true).unwrap().unwrap();
        let slots = Arc::new(Semaphore::new(4));
        let io = TenantRelayIo::open(
            &business,
            &scope,
            Arc::new(RoleLease::acquire(&root).unwrap()),
            vec!["software-private-record-fixture-0000000000".into()],
            Arc::new(Semaphore::new(32 * 1024 * 1024)),
            slots.clone(),
            Arc::new(Semaphore::new(128 * 1024 * 1024)),
            Arc::new(Semaphore::new(16)),
        )
        .unwrap();
        let paths = [first.clone(), second];
        let scan = scan_issued_bindings(&io, &paths, &scope, &endpoint);
        tokio::pin!(scan);
        let held = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                assert!(futures_util::poll!(&mut scan).is_pending());
                if let Ok(held) = slots.clone().try_acquire_many_owned(4) {
                    // The retained first binding's real flock proves its load
                    // completed. Taking all IO permits denies the next path.
                    if matches!(
                        RetainedWorkerBinding::open(&first, &scope),
                        Err(crate::workload_cli::WorkloadCliError::Busy)
                    ) {
                        break held;
                    }
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(
            scan.await.unwrap().is_none(),
            "an incomplete scan accepted its earlier candidate"
        );
        drop(held);
        assert!(
            matches!(
                scan_issued_bindings(&io, &paths, &scope, &endpoint).await,
                Err(RelayOwnerError::Scope)
            ),
            "a complete retry did not reject the two matching issued operations"
        );
        io.join_pending().await.unwrap();
    }
}
