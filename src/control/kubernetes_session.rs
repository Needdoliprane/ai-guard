//! Descriptive V2 reservation contracts. No DTO grants CREATE, release or start.
use crate::runtime::kubernetes::{
    KubernetesAuthorityV1,
    session_profile::{
        KubernetesSessionBindingV2, KubernetesSessionIdsV2, KubernetesSessionProfileV2,
        KubernetesSessionResourcesV2,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;

pub(crate) mod lifecycle;
mod policy;
pub(crate) mod validation;
pub use policy::{
    KubernetesSessionControlRecoveryPolicyV2, KubernetesSessionRecoveryPolicyV2,
    KubernetesSessionReservationApprovalV2, KubernetesSessionReservationPolicyV2,
};
use validation::{canonical, digest, hash, require, uuid};

/// Descriptive input selected after Runner authentication. It grants no
/// authority and cannot select a tenant, policy or workload charge.
pub(crate) enum KubernetesSessionControlRecoveryTargetV2 {
    ObserveReserved {
        reservation_id: String,
        reservation_input_hash: String,
    },
    CloseBeforeEffect {
        close: KubernetesSessionReservationCloseV2,
    },
    LookupCloseReceipt {
        reservation_id: String,
        reservation_input_hash: String,
        close_input_hash: String,
        operation_id: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KubernetesSessionReservationFailureV2 {
    InvalidRequest,
    Conflict,
    PermissionDenied,
    SupervisionInactive,
    MigrationRequired,
    NotFound,
    QuotaExceeded,
    Unavailable,
}
impl fmt::Display for KubernetesSessionReservationFailureV2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidRequest => "invalid Kubernetes session reservation",
            Self::Conflict => "Kubernetes session reservation conflicts with retained content or state",
            Self::PermissionDenied => "Kubernetes session reservation authority is unavailable or outside its authenticated scope",
            Self::SupervisionInactive => "new Kubernetes session reservation requires active supervision",
            Self::MigrationRequired => "explicit Kubernetes session reservation schema11 migration is required",
            Self::NotFound => "Kubernetes session reservation or operation was not found",
            Self::QuotaExceeded => "Kubernetes session reservation exceeds the shared tenant quota",
            Self::Unavailable => "Kubernetes session reservation is unavailable",
        })
    }
}
impl std::error::Error for KubernetesSessionReservationFailureV2 {}

#[derive(Clone, Serialize)]
pub struct KubernetesSessionReservationIntentV2 {
    version: u32,
    reservation_id: String,
    worker_id: String,
    generation: u64,
    authority: KubernetesAuthorityV1,
    profile: KubernetesSessionProfileV2,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IntentWire {
    version: u32,
    reservation_id: String,
    worker_id: String,
    generation: u64,
    authority: Value,
    profile: Value,
}
impl KubernetesSessionReservationIntentV2 {
    pub fn new(
        reservation_id: &str,
        worker_id: &str,
        authority: &KubernetesAuthorityV1,
        profile: &KubernetesSessionProfileV2,
    ) -> anyhow::Result<Self> {
        let intent = Self {
            version: 2,
            reservation_id: reservation_id.into(),
            worker_id: worker_id.into(),
            generation: 1,
            authority: authority.clone(),
            profile: profile.clone(),
        };
        intent.validate()?;
        Ok(intent)
    }
    pub fn from_json(
        bytes: &[u8],
        authority: &KubernetesAuthorityV1,
        binding: &KubernetesSessionBindingV2,
    ) -> anyhow::Result<Self> {
        let value = validation::unique(bytes, 80 * 1024)?;
        let wire: IntentWire = serde_json::from_value(value)
            .map_err(|_| KubernetesSessionReservationFailureV2::InvalidRequest)?;
        let retained = Self::parse_wire(wire)?;
        require(retained.authority == *authority)?;
        KubernetesSessionProfileV2::from_json(
            &retained.profile.canonical_bytes()?,
            authority,
            binding,
        )?;
        Ok(retained)
    }
    fn parse_wire(wire: IntentWire) -> anyhow::Result<Self> {
        let authority_id = wire
            .authority
            .get("authority_id")
            .and_then(Value::as_str)
            .ok_or(KubernetesSessionReservationFailureV2::InvalidRequest)?;
        uuid(authority_id)?;
        let authority: KubernetesAuthorityV1 = serde_json::from_value(wire.authority)
            .map_err(|_| KubernetesSessionReservationFailureV2::InvalidRequest)?;
        authority.validate()?;
        let profile =
            KubernetesSessionProfileV2::retained_from_json(&canonical(&wire.profile)?, &authority)?;
        let intent = Self {
            version: wire.version,
            reservation_id: wire.reservation_id,
            worker_id: wire.worker_id,
            generation: wire.generation,
            authority,
            profile,
        };
        intent.validate()?;
        Ok(intent)
    }
    pub(crate) fn retained_from_value(value: Value) -> anyhow::Result<Self> {
        require(canonical(&value)?.len() <= 80 * 1024)?;
        Self::parse_wire(
            serde_json::from_value(value)
                .map_err(|_| KubernetesSessionReservationFailureV2::InvalidRequest)?,
        )
    }
    pub fn validate(&self) -> anyhow::Result<()> {
        require(self.version == 2 && self.generation == 1)?;
        uuid(&self.reservation_id)?;
        uuid(&self.worker_id)?;
        self.authority.validate()?;
        self.profile.retained_ids()?;
        // Reparse through the existing profile's strict authority validation.
        KubernetesSessionProfileV2::retained_from_json(
            &self.profile.canonical_bytes()?,
            &self.authority,
        )?;
        require(canonical(self)?.len() <= 80 * 1024)
    }
    pub fn reservation_id(&self) -> &str {
        &self.reservation_id
    }
    pub fn worker_id(&self) -> &str {
        &self.worker_id
    }
    pub fn authority(&self) -> &KubernetesAuthorityV1 {
        &self.authority
    }
    pub fn profile(&self) -> &KubernetesSessionProfileV2 {
        &self.profile
    }
    pub(crate) fn ids(&self) -> anyhow::Result<KubernetesSessionIdsV2> {
        Ok(self.profile.retained_ids()?)
    }
    pub(crate) fn resources(&self) -> KubernetesSessionResourcesV2 {
        self.profile.retained_resources()
    }
    pub fn profile_hash(&self) -> anyhow::Result<String> {
        Ok(self.profile.profile_hash()?)
    }
    pub fn session_hash(&self, tenant: &str) -> anyhow::Result<String> {
        crate::tenancy::validate_tenant_id(tenant)?;
        let ids = self.ids()?;
        digest(
            b"ai-guard:kubernetes-session:v2\0",
            &(
                2,
                tenant,
                ids.installation_id,
                ids.control_namespace_uuid,
                ids.session_id,
            ),
        )
    }
    pub fn attempt_hash(&self, tenant: &str) -> anyhow::Result<String> {
        let ids = self.ids()?;
        digest(
            b"ai-guard:kubernetes-session-attempt:v2\0",
            &(
                2,
                self.session_hash(tenant)?,
                &self.worker_id,
                ids.attempt_id,
                1,
                ids.broker_id,
                ids.incarnation_id,
                self.authority.fingerprint(),
                self.profile_hash()?,
            ),
        )
    }
    pub fn input_hash(&self, tenant: &str) -> anyhow::Result<String> {
        self.validate()?;
        digest(
            b"ai-guard:kubernetes-session-reservation:v2\0",
            &(
                2,
                &self.reservation_id,
                &self.worker_id,
                1,
                self.session_hash(tenant)?,
                self.attempt_hash(tenant)?,
                self.authority.fingerprint(),
                self.profile_hash()?,
            ),
        )
    }
    pub(crate) fn same(&self, other: &Self) -> anyhow::Result<bool> {
        Ok(canonical(self)? == canonical(other)?)
    }
}
impl fmt::Debug for KubernetesSessionReservationIntentV2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("KubernetesSessionReservationIntentV2(<private metadata>)")
    }
}
impl<'de> Deserialize<'de> for KubernetesSessionReservationIntentV2 {
    fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        Self::parse_wire(IntentWire::deserialize(decoder)?)
            .map_err(|_| serde::de::Error::custom("invalid Kubernetes session reservation intent"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KubernetesSessionReservationCloseReasonV2 {
    OperatorCancelled,
    PreparationAbandoned,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KubernetesSessionReservationCloseV2 {
    pub version: u32,
    pub operation_id: String,
    pub reservation_id: String,
    pub reservation_input_hash: String,
    pub expected_revision: u64,
    pub reason: KubernetesSessionReservationCloseReasonV2,
}
impl KubernetesSessionReservationCloseV2 {
    pub fn validate(&self) -> anyhow::Result<()> {
        require(
            self.version == 2
                && self.expected_revision == 1
                && self.operation_id != self.reservation_id,
        )?;
        uuid(&self.operation_id)?;
        uuid(&self.reservation_id)?;
        hash(&self.reservation_input_hash)
    }
    pub fn input_hash(&self) -> anyhow::Result<String> {
        self.validate()?;
        digest(
            b"ai-guard:kubernetes-session-reservation-close:v2\0",
            &(
                2,
                &self.operation_id,
                &self.reservation_id,
                &self.reservation_input_hash,
                self.expected_revision,
                self.reason,
            ),
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KubernetesSessionReservationStateV2 {
    Reserved,
    ClosedBeforeEffect,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum KubernetesSessionEffectFenceV2 {
    NeverIssued { revision: u64 },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KubernetesSessionReservationStateDataV2 {
    pub state: KubernetesSessionReservationStateV2,
    pub revision: u64,
    pub quota_held: bool,
    pub effect_fence: KubernetesSessionEffectFenceV2,
    pub last_close: Option<KubernetesSessionReservationCloseV2>,
}
impl KubernetesSessionReservationStateDataV2 {
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        require(
            self.effect_fence == (KubernetesSessionEffectFenceV2::NeverIssued { revision: 0 }),
        )?;
        match self.state {
            KubernetesSessionReservationStateV2::Reserved => {
                require(self.revision == 1 && self.quota_held && self.last_close.is_none())
            }
            KubernetesSessionReservationStateV2::ClosedBeforeEffect => {
                require(self.revision == 2 && !self.quota_held)?;
                self.last_close
                    .as_ref()
                    .ok_or(KubernetesSessionReservationFailureV2::InvalidRequest)?
                    .validate()
            }
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KubernetesSessionReservationSnapshotV2 {
    pub version: u32,
    pub intent: KubernetesSessionReservationIntentV2,
    pub input_hash: String,
    pub session_hash: String,
    pub attempt_hash: String,
    pub profile_hash: String,
    pub state: KubernetesSessionReservationStateDataV2,
}
impl fmt::Debug for KubernetesSessionReservationSnapshotV2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("KubernetesSessionReservationSnapshotV2(<private metadata>)")
    }
}
impl KubernetesSessionReservationSnapshotV2 {
    pub(crate) fn validate(&self, tenant: &str) -> anyhow::Result<()> {
        self.intent.validate()?;
        self.state.validate()?;
        require(
            self.version == 2
                && self.input_hash == self.intent.input_hash(tenant)?
                && self.session_hash == self.intent.session_hash(tenant)?
                && self.attempt_hash == self.intent.attempt_hash(tenant)?
                && self.profile_hash == self.intent.profile_hash()?,
        )?;
        if let Some(close) = &self.state.last_close {
            require(
                close.reservation_id == self.intent.reservation_id
                    && close.reservation_input_hash == self.input_hash,
            )?;
        }
        Ok(())
    }
    pub(crate) fn snapshot_hash(&self, tenant: &str) -> anyhow::Result<String> {
        self.validate(tenant)?;
        digest(
            b"ai-guard:kubernetes-session-reservation-snapshot:v2\0",
            &(
                2,
                &self.input_hash,
                &self.session_hash,
                &self.attempt_hash,
                &self.profile_hash,
                self.state.state,
                self.state.revision,
                self.state.quota_held,
                &self.state.effect_fence,
                &self.state.last_close,
            ),
        )
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KubernetesSessionReservationOperationV2 {
    Reserve,
    CloseBeforeEffect,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KubernetesSessionReservationReceiptV2 {
    pub version: u32,
    pub operation: KubernetesSessionReservationOperationV2,
    pub operation_id: String,
    pub input_hash: String,
    pub control_namespace_uuid: String,
    pub audit_seq: i64,
    pub audit_hash: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KubernetesSessionReservationMutationResponseV2 {
    pub version: u32,
    pub receipt: KubernetesSessionReservationReceiptV2,
    pub snapshot: KubernetesSessionReservationSnapshotV2,
    pub replayed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KubernetesSessionReservationCurrentV2 {
    pub version: u32,
    pub snapshot: KubernetesSessionReservationSnapshotV2,
    pub last_event_seq: i64,
    pub last_event_hash: String,
}
