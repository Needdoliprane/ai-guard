//! Closed audited mutations and deterministic transitions, shared with replay.
use super::*;
use crate::broker_registry::{BrokerIdentityV1, format_time, parse_time};
use authorization::{AuthorizationEvidence, permission};
use chrono::{DateTime, Duration, Utc};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct IdentityEvidence {
    pub tenant_id: String,
    pub broker_id: String,
    pub credential_id: String,
    pub binding_revision: u64,
    pub grant_revision: u64,
}
impl IdentityEvidence {
    pub fn capture(identity: &BrokerIdentityV1) -> Self {
        Self {
            tenant_id: identity.tenant_id().into(),
            broker_id: identity.broker_id().into(),
            credential_id: identity.credential_id().into(),
            binding_revision: identity.binding_revision(),
            grant_revision: identity.grant_revision(),
        }
    }
    pub fn identity(&self) -> anyhow::Result<BrokerIdentityV1> {
        BrokerIdentityV1::authenticated(
            self.tenant_id.clone(),
            self.broker_id.clone(),
            self.credential_id.clone(),
            self.binding_revision,
            self.grant_revision,
        )
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Change {
    Reserve { intent: KubernetesCreateIntentV1 },
    Claim { claim: KubernetesCreateClaimV1 },
    Result { result: KubernetesCreateResultV1 },
    Cancel { cancel: KubernetesCreateCancelV1 },
}
impl Change {
    pub fn operation_id(&self) -> &str {
        match self {
            Self::Reserve { intent } => &intent.create_id,
            Self::Claim { claim } => &claim.operation_id,
            Self::Result { result } => &result.operation_id,
            Self::Cancel { cancel } => &cancel.operation_id,
        }
    }
    pub fn create_id(&self) -> &str {
        match self {
            Self::Reserve { intent } => &intent.create_id,
            Self::Claim { claim } => &claim.create_id,
            Self::Result { result } => &result.create_id,
            Self::Cancel { cancel } => &cancel.create_id,
        }
    }
    pub fn input_hash(&self) -> anyhow::Result<String> {
        match self {
            Self::Reserve { intent } => intent.intent_hash(),
            Self::Claim { claim } => claim.input_hash(),
            Self::Result { result } => result.input_hash(),
            Self::Cancel { cancel } => cancel.input_hash(),
        }
    }
    pub fn validate(&self) -> anyhow::Result<()> {
        match self {
            Self::Reserve { intent } => intent.validate(),
            Self::Claim { claim } => claim.validate(),
            Self::Result { result } => result.validate(),
            Self::Cancel { cancel } => cancel.validate(),
        }
    }
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Reserve { .. } => "control.kubernetes.create.reserved",
            Self::Claim { .. } => "control.kubernetes.create.emission_possible",
            Self::Result { .. } => "control.kubernetes.create.result",
            Self::Cancel { .. } => "control.kubernetes.create.cancelled",
        }
    }
    pub fn requires_active(&self) -> bool {
        matches!(self, Self::Reserve { .. } | Self::Claim { .. })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Record {
    pub version: u32,
    pub input_hash: String,
    pub received_at: String,
    pub identity: IdentityEvidence,
    // Descriptive, credential-free decision. Keep secret-key masking unchanged.
    #[serde(rename = "control_decision")]
    pub authorization: AuthorizationEvidence,
    pub intent: KubernetesCreateIntentV1,
    pub previous_revision: u64,
    pub change: Change,
    pub snapshot: KubernetesCreateSnapshotV1,
}

pub(super) fn conflict(ok: bool) -> anyhow::Result<()> {
    if ok {
        Ok(())
    } else {
        Err(KubernetesCreateFailureV1::Conflict.into())
    }
}
fn advance(value: u64) -> anyhow::Result<u64> {
    let next = value
        .checked_add(1)
        .ok_or(KubernetesCreateFailureV1::Conflict)?;
    conflict(next <= i64::MAX as u64)?;
    Ok(next)
}
pub(super) fn emission_window(
    at: DateTime<Utc>,
    expires: &str,
    grant: &str,
) -> anyhow::Result<KubernetesCreateEmissionWindowV1> {
    let not_after = at
        .checked_add_signed(Duration::seconds(8))
        .ok_or(KubernetesCreateFailureV1::Unavailable)?
        .min(parse_time(expires)?)
        .min(parse_time(grant)?);
    permission(at < not_after)?;
    Ok(KubernetesCreateEmissionWindowV1 {
        issued_at: format_time(at),
        not_after: format_time(not_after),
    })
}

pub(super) fn transition(
    intent: &KubernetesCreateIntentV1,
    previous: Option<&KubernetesCreateSnapshotV1>,
    change: &Change,
    window: Option<KubernetesCreateEmissionWindowV1>,
) -> anyhow::Result<KubernetesCreateSnapshotV1> {
    change.validate()?;
    intent.validate()?;
    let prepared = crate::runtime::kubernetes::admission::prepare(intent)?;
    if let Change::Reserve { intent: requested } = change {
        conflict(previous.is_none() && requested == intent && window.is_none())?;
        return Ok(KubernetesCreateSnapshotV1 {
            version: 1,
            intent: intent.clone(),
            intent_hash: intent.intent_hash()?,
            target: prepared.target().clone(),
            state: KubernetesCreateStateV1::Reserved,
            create_epoch: 1,
            revision: 1,
            quota_held: true,
            claim: None,
            emission_window: None,
            last_result: None,
            last_cancel: None,
        });
    }
    let old = previous.ok_or(KubernetesCreateFailureV1::NotFound)?;
    conflict(
        old.intent == *intent
            && old.intent_hash == intent.intent_hash()?
            && old.target == *prepared.target()
            && old.intent.create_id == change.create_id(),
    )?;
    let (expected_revision, expected_epoch, intent_hash) = match change {
        Change::Claim { claim } => (
            claim.expected_revision,
            claim.expected_create_epoch,
            &claim.intent_hash,
        ),
        Change::Result { result } => (
            result.expected_revision,
            result.expected_create_epoch,
            &result.intent_hash,
        ),
        Change::Cancel { cancel } => (
            cancel.expected_revision,
            cancel.expected_create_epoch,
            &cancel.intent_hash,
        ),
        Change::Reserve { .. } => return Err(KubernetesCreateFailureV1::Conflict.into()),
    };
    conflict(&old.intent_hash == intent_hash)?;
    if let Change::Result { result } = change {
        // A stale result must first identify this exact retained intention and
        // its actual claim. A cancelled state can still carry that claim;
        // result-state guards deliberately remain after this CAS classification.
        conflict(
            old.claim
                .as_ref()
                .is_some_and(|claim| claim.operation_id == result.claim_operation_id),
        )?;
        if expected_epoch < old.create_epoch {
            return Err(KubernetesCreateFailureV1::StaleEpoch.into());
        }
        conflict(expected_epoch == old.create_epoch)?;
        if expected_revision < old.revision {
            return Err(KubernetesCreateFailureV1::StaleRevision.into());
        }
        conflict(expected_revision == old.revision)?;
    } else {
        conflict(old.revision == expected_revision && old.create_epoch == expected_epoch)?;
    }
    let mut next = old.clone();
    next.revision = advance(old.revision)?;
    match change {
        Change::Claim { claim } => {
            conflict(
                old.state == KubernetesCreateStateV1::Reserved
                    && old.claim.is_none()
                    && old.quota_held,
            )?;
            let window = window.ok_or(KubernetesCreateFailureV1::InvalidRequest)?;
            next.state = KubernetesCreateStateV1::EmissionPossible;
            next.claim = Some(claim.clone());
            next.emission_window = Some(window);
        }
        Change::Result { result } => {
            conflict(
                window.is_none()
                    && old.state == KubernetesCreateStateV1::EmissionPossible
                    && old.last_result.is_none()
                    && old
                        .claim
                        .as_ref()
                        .is_some_and(|claim| claim.operation_id == result.claim_operation_id)
                    && old.quota_held,
            )?;
            if let KubernetesCreateOutcomeV1::CreatedHeld {
                observation,
                admitted_spec,
                admitted_configuration_sha256,
                ..
            } = &result.outcome
            {
                crate::runtime::kubernetes::admission::validate_created_held(
                    intent,
                    observation,
                    admitted_spec,
                    admitted_configuration_sha256,
                )?;
                next.state = KubernetesCreateStateV1::CreatedHeld;
            } else {
                next.state = KubernetesCreateStateV1::OwnershipUnknown;
            }
            next.last_result = Some(result.clone());
        }
        Change::Cancel { cancel } => {
            conflict(
                window.is_none() && old.state != KubernetesCreateStateV1::CancelledBeforeEmission,
            )?;
            next.create_epoch = advance(old.create_epoch)?;
            next.last_cancel = Some(cancel.clone());
            match old.state {
                KubernetesCreateStateV1::Reserved => {
                    next.state = KubernetesCreateStateV1::CancelledBeforeEmission;
                    next.quota_held = false;
                }
                KubernetesCreateStateV1::CreatedHeld => {}
                KubernetesCreateStateV1::EmissionPossible
                | KubernetesCreateStateV1::OwnershipUnknown => {
                    next.state = KubernetesCreateStateV1::OwnershipUnknown;
                }
                KubernetesCreateStateV1::CancelledBeforeEmission => {
                    return Err(KubernetesCreateFailureV1::Conflict.into());
                }
            }
        }
        Change::Reserve { .. } => return Err(KubernetesCreateFailureV1::Conflict.into()),
    }
    Ok(next)
}

pub(super) fn receipt(stored: &StoredEvent, record: &Record) -> KubernetesCreateReceiptV1 {
    KubernetesCreateReceiptV1 {
        version: 1,
        operation_id: record.change.operation_id().into(),
        input_hash: record.input_hash.clone(),
        control_namespace_uuid: record.intent.control_namespace_uuid.clone(),
        audit_seq: stored.seq,
        audit_hash: stored.hash.clone(),
        emission_window: if matches!(record.change, Change::Claim { .. }) {
            record.snapshot.emission_window.clone()
        } else {
            None
        },
    }
}
