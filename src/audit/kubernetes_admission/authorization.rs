//! Exact local control capability. Durable evidence below is descriptive only.
use super::*;
use crate::broker_registry::{
    BrokerIdentityV1, BrokerRuntimeKindV1, BrokerV1, format_time, parse_time,
};
use crate::control::QuotaPolicyV1;
use chrono::{DateTime, Utc};

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct AuthorizationEvidenceV1 {
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
impl AuthorizationEvidenceV1 {
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
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct AuthorizationEvidenceV2 {
    version: u32,
    tenant_id: String,
    control_namespace_uuid: String,
    broker_id: String,
    incarnation_id: String,
    credential_id: String,
    binding_revision: u64,
    grant_revision: u64,
    origin_seq: i64,
    origin_hash: String,
    authority_fingerprint: String,
    intent_hash: String,
    quota: QuotaPolicyV1,
    issued_at: String,
    expires_at: String,
    decision_hash: String,
    basis: AuthorizationBasisV2,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "scope", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum AuthorizationBasisV2 {
    Admission {
        source: AdmissionPolicySourceV2,
        policy_snapshot: KubernetesHeldCreatePolicyV1,
        policy_hash: String,
    },
    Reconcile {
        reservation_seq: i64,
        reservation_hash: String,
        admission_decision_hash: String,
    },
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum AdmissionPolicySourceV2 {
    TenantConfiguration,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) enum AuthorizationEvidence {
    V1(AuthorizationEvidenceV1),
    V2(AuthorizationEvidenceV2),
}
impl Serialize for AuthorizationEvidence {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::V1(value) => value.serialize(serializer),
            Self::V2(value) => value.serialize(serializer),
        }
    }
}
impl<'de> Deserialize<'de> for AuthorizationEvidence {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let value = Value::deserialize(deserializer)?;
        match value.get("version").and_then(Value::as_u64) {
            Some(1) => serde_json::from_value(value)
                .map(Self::V1)
                .map_err(D::Error::custom),
            Some(2) => serde_json::from_value(value)
                .map(Self::V2)
                .map_err(D::Error::custom),
            _ => Err(D::Error::custom(
                "unsupported held Kubernetes authorization version",
            )),
        }
    }
}

macro_rules! evidence_field {
    ($name:ident, $kind:ty) => {
        pub(super) fn $name(&self) -> &$kind {
            match self {
                Self::V1(value) => &value.$name,
                Self::V2(value) => &value.$name,
            }
        }
    };
}
impl AuthorizationEvidence {
    evidence_field!(version, u32);
    evidence_field!(tenant_id, String);
    evidence_field!(control_namespace_uuid, String);
    evidence_field!(broker_id, String);
    evidence_field!(incarnation_id, String);
    evidence_field!(credential_id, String);
    evidence_field!(binding_revision, u64);
    evidence_field!(grant_revision, u64);
    evidence_field!(origin_seq, i64);
    evidence_field!(origin_hash, String);
    evidence_field!(authority_fingerprint, String);
    evidence_field!(intent_hash, String);
    evidence_field!(quota, QuotaPolicyV1);
    evidence_field!(issued_at, String);
    evidence_field!(expires_at, String);
    evidence_field!(decision_hash, String);

    pub(super) fn basis(&self) -> Option<&AuthorizationBasisV2> {
        match self {
            Self::V1(_) => None,
            Self::V2(value) => Some(&value.basis),
        }
    }
    pub(super) fn validate_access(
        &self,
        change: &model::Change,
        access: mutations::HeldChangeAccess,
    ) -> anyhow::Result<()> {
        use model::Change;
        use mutations::HeldChangeAccess;
        permission(match self.basis() {
            None => true,
            Some(AuthorizationBasisV2::Admission { .. }) => {
                access == HeldChangeAccess::Mutate
                    && matches!(change, Change::Reserve { .. } | Change::Claim { .. })
            }
            Some(AuthorizationBasisV2::Reconcile { .. }) => match access {
                HeldChangeAccess::ObserveClaim => matches!(change, Change::Claim { .. }),
                HeldChangeAccess::Mutate => {
                    matches!(change, Change::Result { .. } | Change::Cancel { .. })
                }
            },
        })
    }
    pub(super) fn validate_observe_access(
        &self,
        access: mutations::HeldChangeAccess,
    ) -> anyhow::Result<()> {
        permission(
            access == mutations::HeldChangeAccess::ObserveClaim
                && matches!(self.basis(), Some(AuthorizationBasisV2::Reconcile { .. })),
        )
    }
    fn commitment(&self) -> anyhow::Result<String> {
        match self {
            Self::V1(value) => value.commitment(),
            Self::V2(value) => crate::control::content_hash(
                "ai-guard:kubernetes-create-authorization:v2",
                &(
                    value.version,
                    &value.tenant_id,
                    &value.control_namespace_uuid,
                    &value.broker_id,
                    &value.incarnation_id,
                    &value.credential_id,
                    value.binding_revision,
                    value.grant_revision,
                    value.origin_seq,
                    &value.origin_hash,
                    &value.authority_fingerprint,
                    &value.intent_hash,
                    &value.quota,
                    &value.issued_at,
                    &value.expires_at,
                    &value.basis,
                ),
            ),
        }
    }
    pub(super) fn validate(
        &self,
        identity: &BrokerIdentityV1,
        intent: &KubernetesCreateIntentV1,
        at: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        self.quota().validate()?;
        permission(
            [1, 2].contains(self.version())
                && self.tenant_id() == identity.tenant_id()
                && self.credential_id() == identity.credential_id()
                && *self.binding_revision() == identity.binding_revision()
                && *self.grant_revision() == identity.grant_revision()
                && *self.origin_seq() > 0
                && self.control_namespace_uuid() == &intent.control_namespace_uuid
                && self.broker_id() == &intent.broker_id
                && self.incarnation_id() == &intent.incarnation_id
                && self.authority_fingerprint() == &intent.authority.fingerprint()
                && self.intent_hash() == &intent.intent_hash()?
                && self.decision_hash() == &self.commitment()?
                && format_time(parse_time(self.issued_at())?) == *self.issued_at()
                && format_time(parse_time(self.expires_at())?) == *self.expires_at()
                && parse_time(self.issued_at())? <= at
                && at < parse_time(self.expires_at())?,
        )?;
        crate::control::hash(self.origin_hash())?;
        if let Some(basis) = self.basis() {
            let maximum = match basis {
                AuthorizationBasisV2::Admission {
                    policy_snapshot,
                    policy_hash,
                    ..
                } => {
                    permission(
                        policy_snapshot == &policy_snapshot.canonicalized()?
                            && policy_hash == &policy_snapshot.policy_hash()?
                            && policy_snapshot.approves(intent)?,
                    )?;
                    i64::from(policy_snapshot.decision_ttl_seconds)
                }
                AuthorizationBasisV2::Reconcile {
                    reservation_seq,
                    reservation_hash,
                    admission_decision_hash,
                } => {
                    permission(*reservation_seq > 0 && reservation_seq <= self.origin_seq())?;
                    crate::control::hash(reservation_hash)?;
                    crate::control::hash(admission_decision_hash)?;
                    60
                }
            };
            permission(
                parse_time(self.expires_at())?
                    <= parse_time(self.issued_at())?
                        .checked_add_signed(chrono::Duration::seconds(maximum))
                        .ok_or(KubernetesCreateFailureV1::Unavailable)?,
            )?;
        }
        Ok(())
    }
}

/// Only checked Store issuers construct this type; no public constructor,
/// serde implementation or Clone can recreate a capability from descriptive data.
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

impl Store {
    pub(crate) fn authorize_kubernetes_create_admission(
        &self,
        identity: &BrokerIdentityV1,
        intent: &KubernetesCreateIntentV1,
        policy: &KubernetesHeldCreatePolicyV1,
        quota: &QuotaPolicyV1,
    ) -> anyhow::Result<KubernetesCreateAuthorization> {
        intent.validate()?;
        quota.validate()?;
        let policy = policy.canonicalized()?;
        permission(policy.approves(intent)?)?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| KubernetesCreateFailureV1::Unavailable)?;
        let tx = self.begin_checked(&mut db)?;
        require_v2_schema(&tx)?;
        if !tx.lease.active {
            return Err(KubernetesCreateFailureV1::SupervisionInactive.into());
        }
        let clock = workload::capture_control_time(&tx)?;
        let basis = AuthorizationBasisV2::Admission {
            source: AdmissionPolicySourceV2::TenantConfiguration,
            policy_hash: policy.policy_hash()?,
            policy_snapshot: policy,
        };
        let evidence = self.prepare_v2_evidence(&tx, identity, intent, quota, basis, clock.at())?;
        self.commit_authorized(tx, &clock)?;
        Ok(KubernetesCreateAuthorization { evidence })
    }

    pub(crate) fn authorize_kubernetes_create_reconciliation(
        &self,
        identity: &BrokerIdentityV1,
        create_id: &str,
        quota: &QuotaPolicyV1,
    ) -> anyhow::Result<KubernetesCreateAuthorization> {
        crate::control::uuid(create_id)?;
        quota.validate()?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| KubernetesCreateFailureV1::Unavailable)?;
        let tx = self.begin_checked(&mut db)?;
        require_v2_schema(&tx)?;
        let clock = workload::capture_control_time(&tx)?;
        workload::revalidate_broker_identity(&tx, identity, clock.at())?;
        permission(identity.tenant_id() == self.tenant_id)?;
        let snapshot =
            replay::load_snapshot(&tx, create_id)?.ok_or(KubernetesCreateFailureV1::NotFound)?;
        broker_scope(&tx, identity, &snapshot.intent)?;
        let (stored, record) = replay::reservation(&tx, &snapshot.intent, &self.tenant_id)?;
        let basis = AuthorizationBasisV2::Reconcile {
            reservation_seq: stored.seq,
            reservation_hash: stored.hash,
            admission_decision_hash: record.authorization.decision_hash().clone(),
        };
        let evidence =
            self.prepare_v2_evidence(&tx, identity, &snapshot.intent, quota, basis, clock.at())?;
        self.commit_authorized(tx, &clock)?;
        Ok(KubernetesCreateAuthorization { evidence })
    }

    fn prepare_v2_evidence(
        &self,
        tx: &Connection,
        identity: &BrokerIdentityV1,
        intent: &KubernetesCreateIntentV1,
        quota: &QuotaPolicyV1,
        basis: AuthorizationBasisV2,
        at: DateTime<Utc>,
    ) -> anyhow::Result<AuthorizationEvidence> {
        workload::revalidate_broker_identity(tx, identity, at)?;
        permission(identity.tenant_id() == self.tenant_id)?;
        let broker = broker_scope(tx, identity, intent)?;
        let binding: crate::workload::WorkloadBinding = serde_json::from_str(&tx.query_row(
            "SELECT payload_json FROM workload_bindings WHERE credential_id=?1",
            [identity.credential_id()],
            |row| row.get::<_, String>(0),
        )?)?;
        let ttl = match &basis {
            AuthorizationBasisV2::Admission {
                policy_snapshot, ..
            } => i64::from(policy_snapshot.decision_ttl_seconds),
            AuthorizationBasisV2::Reconcile { .. } => 60,
        };
        let expires = at
            .checked_add_signed(chrono::Duration::seconds(ttl))
            .ok_or(KubernetesCreateFailureV1::Unavailable)?
            .min(parse_time(&broker.not_after)?)
            .min(parse_time(&binding.expires_at)?);
        permission(at < expires)?;
        let (origin_seq, origin_hash) = audit_head(tx)?;
        let mut evidence = AuthorizationEvidence::V2(AuthorizationEvidenceV2 {
            version: 2,
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
            issued_at: format_time(at),
            expires_at: format_time(expires),
            decision_hash: String::new(),
            basis,
        });
        let commitment = evidence.commitment()?;
        if let AuthorizationEvidence::V2(value) = &mut evidence {
            value.decision_hash = commitment;
        }
        evidence.validate(identity, intent, at)?;
        replay::validate_v2_origin(tx, &evidence, None)?;
        Ok(evidence)
    }
}

fn require_v2_schema(db: &Connection) -> anyhow::Result<()> {
    let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if [9, 10, 11].contains(&version) {
        Ok(())
    } else {
        Err(KubernetesCreateFailureV1::MigrationRequired.into())
    }
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
        let mut evidence = AuthorizationEvidenceV1 {
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
        let evidence = AuthorizationEvidence::V1(evidence);
        evidence.validate(identity, intent, clock.at())?;
        self.commit_authorized(tx, &clock)?;
        Ok(KubernetesCreateAuthorization { evidence })
    }
}

#[cfg(test)]
#[path = "authorization_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "authorization_v2_tests.rs"]
mod v2_tests;
