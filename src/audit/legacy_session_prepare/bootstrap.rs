//! The original live Legacy Store used by the admission fixture.
use super::*;
use crate::{
    broker_registry::{BrokerGrantMutationV1, parse_time},
    workload::{WorkloadBindingIssueV1, WorkloadSubjectV1},
};
use std::path::Path;

mod tests;

/// Owns the same Store handle from initial activation through target publication.
pub(in crate::audit) struct FreshLegacySessionBootstrap {
    store: Store,
    head: (i64, String),
}

impl FreshLegacySessionBootstrap {
    pub(in crate::audit) fn create(
        source: &Path,
        tenant: &str,
        redactor: Redactor,
        limits: AuditCapacityV1,
        policy: Option<AuditCapacityPolicyV1>,
        backups: &Path,
        enrollment: (&WorkloadBindingIssueV1, &BrokerGrantMutationV1),
    ) -> anyhow::Result<Self> {
        let (credential, grant) = enrollment;
        credential.validate()?;
        grant.validate()?;
        let now = workload::canonical_control_time(chrono::Utc::now())?;
        anyhow::ensure!(
            credential.expected_revision == 0
                && grant.expected_revision.is_none()
                && credential.credential_id == grant.credential_id
                && matches!(
                    &credential.subject,
                    WorkloadSubjectV1::Broker { broker_id, incarnation_id }
                        if broker_id == &grant.broker_id && incarnation_id == &grant.incarnation_id
                )
                && workload::canonical_control_time(parse_time(&credential.expires_at)?)? > now
                && workload::canonical_control_time(parse_time(&grant.not_after)?)? > now,
            IdentityFailureV1::Conflict
        );
        anyhow::ensure!(
            (1..=1_000_000).contains(&limits.events),
            "invalid audit capacity"
        );
        let store = Store::create_supervised_for_tenant_with_capacity(
            source,
            tenant,
            redactor,
            limits,
            policy,
            &backups.join("before-3.sqlite"),
        )?;
        for version in 4..=13 {
            store.migrate_control_version(
                &backups.join(format!("before-{version}.sqlite")),
                version - 1,
                version,
            )?;
        }
        store.migrate_human_rights_v2(&backups.join("before-rights.sqlite"))?;
        store.enable_legacy_session_prepare_v1(&backups.join("before-prepare.sqlite"))?;
        store.issue_workload_binding(credential)?;
        let grant = store.mutate_broker_grant(grant)?;
        anyhow::ensure!(!grant.replayed, IdentityFailureV1::Conflict);
        Ok(Self {
            store,
            head: (grant.receipt.audit_seq, grant.receipt.audit_hash),
        })
    }

    pub(in crate::audit) fn provision_target(
        self,
        operator: PrincipalRefV1,
        command: ProvisionLegacySessionTargetV1,
    ) -> anyhow::Result<(Store, LegacyTargetReceipt)> {
        anyhow::ensure!(command.expected_revision == 0, IdentityFailureV1::Conflict);
        self.store
            .validate_legacy_target_provision(&operator, &command)?;
        let receipt = {
            let mut db = self
                .store
                .db
                .lock()
                .map_err(|_| IdentityFailureV1::IdentityUnavailable)?;
            let tx = self.store.begin_checked(&mut db)?;
            self.check(&tx)?;
            self.store
                .publish_legacy_session_target(tx, operator, command)?
        };
        Ok((self.store, receipt))
    }

    fn check(&self, tx: &CheckedTransaction<'_>) -> anyhow::Result<()> {
        // begin_checked verifies the backing and the retained prefix. The
        // enrollment also pins the exact head on this live Store handle.
        anyhow::ensure!(
            std::ptr::eq(tx.origin, &self.store) && tx.lease.active && tx.lease.fresh_origin,
            IdentityFailureV1::PermissionDenied
        );
        anyhow::ensure!(
            tx.lease.floor == self.head && audit_head(tx)? == self.head,
            IdentityFailureV1::Conflict
        );
        let empty: bool = tx.query_row(
            "SELECT NOT EXISTS(SELECT 1 FROM events WHERE kind=?1)",
            [TARGET_KIND],
            |row| row.get(0),
        )?;
        anyhow::ensure!(empty, IdentityFailureV1::Conflict);
        Ok(())
    }
}
