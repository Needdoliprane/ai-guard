//! Corroborate a durable Legacy STOP with the already restored owner.
use super::*;
use crate::broker::journal::recovery::legacy_worker_terminal::LegacyWorkerTerminalEvidenceV1;

struct LegacyTerminalStopTarget {
    arm_id: String,
    registration_hash: String,
    scope_hash: String,
    operation_id: Uuid,
    cancel: CancelReferenceV1,
}

impl EnforcerStopClient {
    /// Read the existing guardian under the retained journal lease. Success
    /// corroborates one STOP in the authenticated snapshot. The caller still
    /// needs to recheck its current journal head and configured invocation.
    #[allow(
        dead_code,
        reason = "the Legacy terminal observer is composed separately"
    )]
    pub(in crate::broker) async fn confirm_legacy_terminal_stop_owner(
        &self,
        proof: &LegacyWorkerTerminalEvidenceV1,
    ) -> EnforcerResult<()> {
        let (arm, receipt) = proof.arm();
        let (authority, owned) = proof.target();
        let registration = recovery_registration(proof.scope(), arm, receipt, authority, owned)?;
        self.confirm_existing_legacy_stop(&LegacyTerminalStopTarget {
            arm_id: arm.operation_id.clone(),
            registration_hash: registration.digest()?,
            scope_hash: recovery_scope_hash(proof.scope())?,
            operation_id: proof.operation_id(),
            cancel: CancelReferenceV1::from_receipt(proof.cancel_receipt()),
        })
        .await
    }

    async fn confirm_existing_legacy_stop(
        &self,
        target: &LegacyTerminalStopTarget,
    ) -> EnforcerResult<()> {
        if self.0.protocol != StopProtocol::V2 || !matches!(self.0.affinity, StopAffinity::Legacy) {
            return Err(EnforcerError::Unauthorized);
        }
        let (epoch, version, revision, retained) = {
            let cache = self.0.cache.lock().map_err(|_| EnforcerError::Unhealthy)?;
            let (epoch, entry) = acknowledged_stop(&cache, &target.arm_id)?;
            let retained = &entry.retained;
            let request = &retained.request;
            if retained.binding.is_some()
                || request.registration_hash != target.registration_hash
                || request.source.scope_hash != target.scope_hash
                || request.source.job_id != target.operation_id
                || !request.cancel_prefix.contains(&target.cancel)
            {
                return Err(EnforcerError::Conflict);
            }
            (
                epoch,
                cache.observation_version,
                entry.revision,
                retained.clone(),
            )
        };

        tokio::time::timeout(IO_TIMEOUT * 2, async {
            let request = retained.request;
            let owner = retained.owner;
            let root = self.0.root.clone();
            let key = self.0.capability;
            let (root, owner) = tokio::task::spawn_blocking(move || {
                require_legacy_root(&root)?;
                owner.revalidate()?;
                Ok::<_, EnforcerError>((root, owner))
            })
            .await
            .map_err(|_| EnforcerError::Unavailable)??;
            // The normal request path can start a guardian. Even
            // open_observed may publish a successor epoch to the monitor.
            // This read uses the authenticated open without either action.
            let (mut connection, binding) =
                self.open_with_binding(Some(epoch))
                    .await
                    .map_err(|failure| match failure {
                        OpenFailure::Other(error) => error,
                        OpenFailure::Absent | OpenFailure::Superseded => EnforcerError::Unavailable,
                    })?;
            let response = request_stops2(
                &mut connection.stream,
                StopRequest::QueryStop {
                    arm_id: target.arm_id.clone(),
                    registration_hash: target.registration_hash.clone(),
                },
            )
            .await?;
            let StopResponse::StopStatus { value } = response else {
                return Err(EnforcerError::Conflict);
            };
            // A quarantined actor may have processed STOP or encountered an
            // error. Only the exact durable prefix can corroborate its latch.
            if value.arm_id != target.arm_id
                || value.registration_hash != target.registration_hash
                || !matches!(
                    value.phase,
                    StopPhaseV1::StopRequested | StopPhaseV1::Quarantined
                )
                || value.generation < request.cancel_generation
                || value.cancel_prefix != request.cancel_prefix
                || value.durable_cancel != request.durable_cancel
                || value.cancel_command_id
                    != request
                        .durable_cancel
                        .as_ref()
                        .map(|cancel| cancel.command_id)
            {
                return Err(EnforcerError::Conflict);
            }
            tokio::task::spawn_blocking(move || {
                require_legacy_root(&root)?;
                let current = load_endpoint(&root)?.ok_or(EnforcerError::Unavailable)?;
                if current.epoch() != epoch || recovery_root_binding(&root, &current)? != binding {
                    return Err(EnforcerError::Conflict);
                }
                if capability(&root, false)? != key {
                    return Err(EnforcerError::Unauthorized);
                }
                owner.revalidate()
            })
            .await
            .map_err(|_| EnforcerError::Unavailable)??;
            let cache = self.0.cache.lock().map_err(|_| EnforcerError::Unhealthy)?;
            let (latest, entry) = acknowledged_stop(&cache, &target.arm_id)?;
            if latest != epoch || cache.observation_version != version || entry.revision != revision
            {
                return Err(EnforcerError::Conflict);
            }
            Ok(())
        })
        .await
        .map_err(|_| EnforcerError::Unavailable)?
    }
}

fn acknowledged_stop<'a>(
    cache: &'a StopCache,
    arm_id: &str,
) -> EnforcerResult<(Uuid, &'a PendingRestore)> {
    let epoch = cache.epoch.ok_or(EnforcerError::NotReady)?;
    let entry = cache.entries.get(arm_id).ok_or(EnforcerError::NotReady)?;
    if entry.acknowledged != Some((epoch, entry.revision)) {
        return Err(EnforcerError::NotReady);
    }
    Ok((epoch, entry))
}

fn require_legacy_root(root: &Directory) -> EnforcerResult<()> {
    // Stops2 is also understood by a Full3 listener. The Legacy port never
    // treats a sealed role as its own, including an incomplete namespace.
    if root
        .persistent_child("retirement", false)
        .map_err(|_| EnforcerError::UnsafeFilesystem)?
        .is_some()
    {
        return Err(EnforcerError::Unauthorized);
    }
    // Check the Legacy lock shape without trying to acquire the owner's
    // lease. A nonempty or absent lock cannot qualify even if the directory
    // that normally contains the retirement ledger is absent.
    root.persistent_read(".owner.lock", 0, &[])
        .map_err(|_| EnforcerError::UnsafeFilesystem)?
        .ok_or(EnforcerError::UnsafeFilesystem)?
        .persistent_revalidate()
        .map_err(|_| EnforcerError::UnsafeFilesystem)?;
    Ok(())
}

#[cfg(test)]
#[path = "recovery_legacy_terminal_tests.rs"]
mod tests;
