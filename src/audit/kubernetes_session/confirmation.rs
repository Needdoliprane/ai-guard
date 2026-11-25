//! Local evidence of a checked before-effect observation, either an actual
//! Store transaction or a nonce-matched live HTTP ACK from the broker client.
//! This is neither serializable authority nor a CREATE/release/start permit.
use super::*;
use crate::api::AuthenticatedControlRecoveryV2;

pub(crate) struct ConfirmedSessionReservationV2 {
    tenant_id: String,
    snapshot: KubernetesSessionReservationSnapshotV2,
    audit_seq: i64,
    audit_hash: String,
    protected_files: Vec<crate::control_fs::PrivateFile>,
}
impl ConfirmedSessionReservationV2 {
    pub(crate) fn from_live_http_ack(
        ack: crate::broker::held_create_client::sessions::LiveSessionConfirmationAckV2,
    ) -> anyhow::Result<Self> {
        let (tenant, current, files) = ack.into_parts();
        let mut confirmed = confirmed(&tenant, current)?;
        confirmed.protected_files = files;
        Ok(confirmed)
    }
    pub(crate) fn revalidate_material(&self) -> anyhow::Result<()> {
        for file in &self.protected_files {
            file.revalidate()?;
        }
        Ok(())
    }
    pub(crate) fn tenant_id(&self) -> &str {
        &self.tenant_id
    }
    pub(crate) fn snapshot(&self) -> &KubernetesSessionReservationSnapshotV2 {
        &self.snapshot
    }
    pub(crate) fn audit_seq(&self) -> i64 {
        self.audit_seq
    }
    pub(crate) fn audit_hash(&self) -> &str {
        &self.audit_hash
    }
}

// Kept private: a deserialized response alone cannot mint this evidence.
// Callers below first complete the actual checked Store observation.
fn confirmed(
    tenant_id: &str,
    current: KubernetesSessionReservationCurrentV2,
) -> anyhow::Result<ConfirmedSessionReservationV2> {
    crate::tenancy::validate_tenant_id(tenant_id)?;
    current.snapshot.validate(tenant_id)?;
    hash(&current.last_event_hash)?;
    permission(
        current.version == 2
            && current.last_event_seq > 0
            && current.snapshot.state.state == KubernetesSessionReservationStateV2::Reserved
            && current.snapshot.state.revision == 1
            && current.snapshot.state.quota_held
            && current.snapshot.state.last_close.is_none()
            && current.snapshot.state.effect_fence
                == (KubernetesSessionEffectFenceV2::NeverIssued { revision: 0 }),
    )?;
    Ok(ConfirmedSessionReservationV2 {
        tenant_id: tenant_id.into(),
        snapshot: current.snapshot,
        audit_seq: current.last_event_seq,
        audit_hash: current.last_event_hash,
        protected_files: Vec::new(),
    })
}

impl Store {
    /// Observe and commit using the current authenticated broker scope before
    /// retaining evidence for local journal preparation. No new event or quota.
    pub(crate) fn confirm_kubernetes_session_before_effect(
        &self,
        identity: &BrokerIdentityV1,
        authorization: &KubernetesSessionReservationAuthorizationV2,
        reservation_id: &str,
    ) -> anyhow::Result<ConfirmedSessionReservationV2> {
        let current = self.observe_kubernetes_session(identity, authorization, reservation_id)?;
        confirmed(&self.tenant_id, current)
    }

    /// S2 counterpart: preserves the actual auth epoch, policy, retained usage
    /// and before-effect checks instead of accepting an operator-supplied DTO.
    // Closed Kubernetes session before-effect target; production wiring pending.
    #[allow(dead_code)]
    pub(crate) fn confirm_kubernetes_session_control_before_effect(
        &self,
        auth: &AuthenticatedControlRecoveryV2,
        authorization: &ControlRecoverBeforeEffectV2<'_>,
        policy: &KubernetesSessionControlRecoveryPolicyV2,
    ) -> anyhow::Result<ConfirmedSessionReservationV2> {
        let current =
            self.observe_kubernetes_session_control_recovery(auth, authorization, policy)?;
        confirmed(&self.tenant_id, current)
    }
}
