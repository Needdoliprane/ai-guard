//! Resolve the retained subject and current authority for a fresh named effect.
use super::*;
use crate::{audit::control, control::WorkerV1};

impl Store {
    /// Read the accepted admission and its current authorization in the
    /// writer's transaction. The expected worker supplies the immutable
    /// subject; the writer still checks liveness, transition and receipt.
    /// The returned human and broker deadlines must be checked at commit.
    pub(in crate::audit) fn require_legacy_named_execution(
        &self,
        tx: &CheckedTransaction<'_>,
        broker: &BrokerIdentityV1,
        binding_ref: Uuid,
        namespace: &str,
        expected: &WorkerV1,
        clock: &workload::AuthorizedControlTimeV1,
    ) -> anyhow::Result<(DateTime<Utc>, DateTime<Utc>)> {
        let version = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
        anyhow::ensure!(
            format_admits(version) && capability_present(tx)?,
            ControlFailure::MigrationRequired
        );
        let resolved = self.resolve_legacy_confirmed_plan(tx, broker, binding_ref, clock)?;
        let stored = event_by_id(tx, &fact_id(binding_ref))?.ok_or(ControlFailure::NotFound)?;
        let fact: AdmissionFact = decoded(&stored, ADMITTED)?;
        anyhow::ensure!(
            resolved.plan.control_namespace_uuid == namespace
                && fact.binding_ref == binding_ref
                && fact.confirmation_hash == resolved.confirmed.hash
                && fact.broker_profile_id == resolved.plan.broker_profile_id
                && fact.authorization_revision <= resolved.authorization_revision
                && fact.actor.matches_grant(broker)
                && fact.actor.binding_revision == broker.binding_revision()
                && fact.grant.is_some()
                && expected.launch.spec_hash()? == resolved.plan.launch_spec_hash,
            ControlFailure::NotFound
        );

        let (_, record) = admission_record(tx, &fact)?;
        let (_, admitted) = record.admission()?;
        let admitted = admitted.worker.as_ref().ok_or(ControlFailure::NotFound)?;
        let before_seq = audit_head(tx)?
            .0
            .checked_add(1)
            .context("audit sequence exhausted")?;
        let current = control::worker_at(tx, &expected.worker_id, before_seq)?
            .ok_or(ControlFailure::NotFound)?;
        let session_id = resolved.plan.session_id.to_string();
        let operation_id = resolved.plan.operation_id.to_string();
        for worker in [expected, admitted, &current] {
            anyhow::ensure!(
                worker.worker_id == expected.worker_id
                    && worker.attempt.attempt_id == expected.attempt.attempt_id
                    && worker.launch.session_id == session_id
                    && worker.admission_operation_id == operation_id
                    && worker.spec_hash == resolved.plan.launch_spec_hash,
                ControlFailure::NotFound
            );
        }
        anyhow::ensure!(current.quota_held, ControlFailure::NotFound);

        let retained_grant = session_owner_grant(
            &self.tenant_id,
            &resolved.initiator,
            resolved.plan.session_id,
        );
        let current_grant: Option<ResourceGrantV1> = identity::json_row(
            tx,
            "SELECT payload_json FROM identity_resources WHERE resource_key=?1",
            &retained_grant.resource.key()?,
        )?;
        // The retained revision fences revoke or replacement before first use.
        anyhow::ensure!(
            current_grant.as_ref() == Some(&retained_grant),
            ControlFailure::NotFound
        );
        Ok((resolved.plan.expires_at, resolved.broker_deadline))
    }
}
