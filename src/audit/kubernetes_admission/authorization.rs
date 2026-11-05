//! Exact local control capability. Durable evidence below is descriptive only.
use super::*;
use crate::broker_registry::{
    BrokerIdentityV1, BrokerRuntimeKindV1, BrokerV1, format_time, parse_time,
};
use crate::control::QuotaPolicyV1;
use chrono::{DateTime, Utc};

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct AuthorizationEvidence {
    pub version: u32,
    pub tenant_id: String,
    pub control_namespace_uuid: String,
    pub broker_id: String,
    pub incarnation_id: String,
    pub credential_id: String,
    pub binding_revision: u64,
    pub grant_revision: u64,
    pub origin_seq: i64,
    pub origin_hash: String,
    pub authority_fingerprint: String,
    pub intent_hash: String,
    pub quota: QuotaPolicyV1,
    pub issued_at: String,
    pub expires_at: String,
    pub decision_hash: String,
}
impl AuthorizationEvidence {
    fn commitment(&self) -> anyhow::Result<String> {
        crate::control::content_hash(
            "ai-guard:kubernetes-create-authorization:v1",
            &(
                self.version,
                &self.tenant_id,
                &self.control_namespace_uuid,
                &self.broker_id,
                &self.incarnation_id,
                &self.credential_id,
                self.binding_revision,
                self.grant_revision,
                self.origin_seq,
                &self.origin_hash,
                &self.authority_fingerprint,
                &self.intent_hash,
                &self.quota,
                &self.issued_at,
                &self.expires_at,
            ),
        )
    }
    pub(super) fn validate(
        &self,
        identity: &BrokerIdentityV1,
        intent: &KubernetesCreateIntentV1,
        at: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        self.quota.validate()?;
        permission(
            self.version == 1
                && self.tenant_id == identity.tenant_id()
                && self.credential_id == identity.credential_id()
                && self.binding_revision == identity.binding_revision()
                && self.grant_revision == identity.grant_revision()
                && self.origin_seq > 0
                && self.origin_hash.len() == 64
                && self.control_namespace_uuid == intent.control_namespace_uuid
                && self.broker_id == intent.broker_id
                && self.incarnation_id == intent.incarnation_id
                && self.authority_fingerprint == intent.authority.fingerprint()
                && self.intent_hash == intent.intent_hash()?
                && self.decision_hash == self.commitment()?
                && format_time(parse_time(&self.issued_at)?) == self.issued_at
                && format_time(parse_time(&self.expires_at)?) == self.expires_at
                && parse_time(&self.issued_at)? <= at
                && at < parse_time(&self.expires_at)?,
        )
    }
}

/// There is deliberately no production constructor, serde implementation or Clone.
pub(crate) struct KubernetesCreateAuthorization {
    pub(super) evidence: AuthorizationEvidence,
}

pub(super) fn permission(ok: bool) -> anyhow::Result<()> {
    if ok {
        Ok(())
    } else {
        Err(KubernetesCreateFailureV1::PermissionDenied.into())
    }
}
pub(super) fn broker_scope(
    db: &Connection,
    identity: &BrokerIdentityV1,
    intent: &KubernetesCreateIntentV1,
) -> anyhow::Result<BrokerV1> {
    let broker: BrokerV1 = serde_json::from_str(&db.query_row(
        "SELECT payload_json FROM broker_registry_brokers WHERE broker_id=?1",
        [identity.broker_id()],
        |row| row.get::<_, String>(0),
    )?)?;
    let namespace: String = db.query_row(
        "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    permission(
        namespace == intent.control_namespace_uuid
            && intent.broker_id == identity.broker_id()
            && intent.incarnation_id == broker.incarnation_id
            && broker.authorities.iter().any(|authority| {
                authority.authority_id == intent.authority.authority_id().to_string()
                    && authority.runtime_kind == BrokerRuntimeKindV1::Kubernetes
            }),
    )?;
    Ok(broker)
}

#[cfg(test)]
impl Store {
    /// Local fixture seam after real binding/grant authentication. This is not an
    /// endpoint, runner assertion, production policy implementation or cluster proof.
    pub(crate) fn authorize_held_create_for_test(
        &self,
        identity: &BrokerIdentityV1,
        intent: &KubernetesCreateIntentV1,
        quota: &QuotaPolicyV1,
        expires_at: &str,
    ) -> anyhow::Result<KubernetesCreateAuthorization> {
        intent.validate()?;
        quota.validate()?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| KubernetesCreateFailureV1::Unavailable)?;
        let tx = self.begin_checked(&mut db)?;
        require_schema(&tx)?;
        let clock = workload::capture_control_time(&tx)?;
        workload::revalidate_broker_identity(&tx, identity, clock.at())?;
        permission(identity.tenant_id() == self.tenant_id)?;
        let broker = broker_scope(&tx, identity, intent)?;
        let binding_json: String = tx.query_row(
            "SELECT payload_json FROM workload_bindings WHERE credential_id=?1",
            [identity.credential_id()],
            |row| row.get(0),
        )?;
        let binding: crate::workload::WorkloadBinding = serde_json::from_str(&binding_json)?;
        let expires = parse_time(expires_at)?
            .min(parse_time(&broker.not_after)?)
            .min(parse_time(&binding.expires_at)?);
        permission(clock.at() < expires)?;
        let (origin_seq, origin_hash) = audit_head(&tx)?;
        let mut evidence = AuthorizationEvidence {
            version: 1,
            tenant_id: self.tenant_id.clone(),
            control_namespace_uuid: intent.control_namespace_uuid.clone(),
            broker_id: intent.broker_id.clone(),
            incarnation_id: intent.incarnation_id.clone(),
            credential_id: identity.credential_id().into(),
            binding_revision: identity.binding_revision(),
            grant_revision: identity.grant_revision(),
            origin_seq,
            origin_hash,
            authority_fingerprint: intent.authority.fingerprint(),
            intent_hash: intent.intent_hash()?,
            quota: quota.clone(),
            issued_at: format_time(clock.at()),
            expires_at: format_time(expires),
            decision_hash: String::new(),
        };
        evidence.decision_hash = evidence.commitment()?;
        evidence.validate(identity, intent, clock.at())?;
        self.commit_authorized(tx, &clock)?;
        Ok(KubernetesCreateAuthorization { evidence })
    }
}

#[cfg(test)]
#[path = "authorization_tests.rs"]
mod tests;
