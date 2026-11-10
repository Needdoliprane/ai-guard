//! Durable V2 preparation from a real central before-effect confirmation.
//! No record, replay or bootstrap binding grants CREATE, release or execution.
use crate::{
    audit::kubernetes_session::ConfirmedSessionReservationV2,
    control::kubernetes_session::{
        KubernetesSessionEffectFenceV2, KubernetesSessionReservationIntentV2,
        KubernetesSessionReservationStateV2,
    },
    control_fs::{Directory, PrivateFile},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;
#[path = "session_disk.rs"]
mod disk;
pub(crate) use disk::SessionJournalProtection;

const MAX_PREPARATION: u64 = 96 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionError {
    InvalidInput,
    Conflict,
    Corrupt,
    Capacity,
    Busy,
    Unavailable,
    UnsafeMaterial,
}
impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidInput => "invalid Kubernetes session preparation",
            Self::Conflict => {
                "Kubernetes session preparation differs from retained identity or state"
            }
            Self::Corrupt => "Kubernetes session journal requires reconciliation",
            Self::Capacity => "Kubernetes session preparation exceeds its bound",
            Self::Busy => "Kubernetes session journal is already owned",
            Self::Unavailable => "Kubernetes session journal durability is unavailable",
            Self::UnsafeMaterial => "Kubernetes session journal refused protected material",
        })
    }
}
impl std::error::Error for SessionError {}
pub(crate) type SessionResult<T> = Result<T, SessionError>;
fn require(valid: bool) -> SessionResult<()> {
    if valid {
        Ok(())
    } else {
        Err(SessionError::Conflict)
    }
}
fn canonical(value: &impl Serialize) -> SessionResult<Vec<u8>> {
    crate::control::kubernetes_session::validation::canonical(value)
        .map_err(|_| SessionError::InvalidInput)
}
fn commitment(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(b"ai-guard:kubernetes-session-runtime-preparation:v2\0");
    hash.update(bytes);
    hex::encode(hash.finalize())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Preparation {
    version: u32,
    tenant_id: String,
    intent: Value,
    reservation_input_hash: String,
    session_hash: String,
    attempt_hash: String,
    profile_hash: String,
    audit_seq: i64,
    audit_hash: String,
}
impl Preparation {
    fn from_confirmed(
        expected: &KubernetesSessionReservationIntentV2,
        confirmed: &ConfirmedSessionReservationV2,
    ) -> SessionResult<Self> {
        let snapshot = confirmed.snapshot();
        snapshot
            .validate(confirmed.tenant_id())
            .map_err(|_| SessionError::InvalidInput)?;
        require(
            snapshot
                .intent
                .same(expected)
                .map_err(|_| SessionError::InvalidInput)?,
        )?;
        require(
            snapshot.state.state == KubernetesSessionReservationStateV2::Reserved
                && snapshot.state.revision == 1
                && snapshot.state.quota_held
                && snapshot.state.effect_fence
                    == (KubernetesSessionEffectFenceV2::NeverIssued { revision: 0 })
                && snapshot.state.last_close.is_none(),
        )?;
        let value = Self {
            version: 2,
            tenant_id: confirmed.tenant_id().into(),
            intent: serde_json::to_value(expected).map_err(|_| SessionError::InvalidInput)?,
            reservation_input_hash: snapshot.input_hash.clone(),
            session_hash: snapshot.session_hash.clone(),
            attempt_hash: snapshot.attempt_hash.clone(),
            profile_hash: snapshot.profile_hash.clone(),
            audit_seq: confirmed.audit_seq(),
            audit_hash: confirmed.audit_hash().into(),
        };
        value.validate()?;
        Ok(value)
    }
    fn validate(&self) -> SessionResult<KubernetesSessionReservationIntentV2> {
        let intent = KubernetesSessionReservationIntentV2::retained_from_value(self.intent.clone())
            .map_err(|_| SessionError::Corrupt)?;
        crate::tenancy::validate_tenant_id(&self.tenant_id).map_err(|_| SessionError::Corrupt)?;
        require(
            self.version == 2
                && self.audit_seq > 0
                && super::valid_digest(&self.audit_hash)
                && self.reservation_input_hash
                    == intent
                        .input_hash(&self.tenant_id)
                        .map_err(|_| SessionError::Corrupt)?
                && self.session_hash
                    == intent
                        .session_hash(&self.tenant_id)
                        .map_err(|_| SessionError::Corrupt)?
                && self.attempt_hash
                    == intent
                        .attempt_hash(&self.tenant_id)
                        .map_err(|_| SessionError::Corrupt)?
                && self.profile_hash == intent.profile_hash().map_err(|_| SessionError::Corrupt)?,
        )?;
        Ok(intent)
    }
}

/// Exact local preparation metadata. This is not a live peer identity or permit.
/// It is intentionally not deserializable and carries no credential or argv.
#[derive(Clone, PartialEq, Eq, Serialize)]
pub(crate) struct SessionBootstrapBindingV2 {
    pub(super) version: u32,
    pub(super) installation_id: Uuid,
    pub(super) control_namespace_uuid: Uuid,
    pub(super) broker_id: Uuid,
    pub(super) incarnation_id: Uuid,
    pub(super) session_id: Uuid,
    pub(super) attempt_id: Uuid,
    pub(super) reservation_id: String,
    pub(super) reservation_input_hash: String,
    pub(super) authority_fingerprint: String,
    pub(super) profile_hash: String,
    pub(super) preparation_sha256: String,
}
impl std::fmt::Debug for SessionBootstrapBindingV2 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SessionBootstrapBindingV2(<local metadata; no authority>)")
    }
}

pub(crate) struct SessionJournal {
    disk: disk::SessionDisk,
    prepared: Preparation,
    bytes: Vec<u8>,
}
impl SessionJournal {
    /// The control caller performs this synchronous segment under retained FS
    /// capacity. Reopen always needs a new real S1/S2 confirmation; disk alone
    /// never confirms a reservation, a live broker or a never-issued effect.
    pub(crate) fn prepare_or_resume(
        root: Directory,
        protection: SessionJournalProtection,
        expected: &KubernetesSessionReservationIntentV2,
        confirmed: &ConfirmedSessionReservationV2,
    ) -> SessionResult<Self> {
        let prepared = Preparation::from_confirmed(expected, confirmed)?;
        let bytes = canonical(&prepared)?;
        if bytes.len() as u64 > MAX_PREPARATION {
            return Err(SessionError::Capacity);
        }
        protection.screen(&bytes)?;
        let disk = disk::SessionDisk::acquire(root, protection, &bytes)?;
        // The retained disk record has no execution authority. Compare all
        // immutable bytes, including the real original audit position/hash.
        let retained = disk.prepared.bytes();
        let value = crate::json::decode_unique(retained, 64).map_err(|_| SessionError::Corrupt)?;
        let decoded: Preparation =
            serde_json::from_value(value).map_err(|_| SessionError::Corrupt)?;
        decoded.validate()?;
        require(canonical(&decoded)? == bytes && retained == bytes)?;
        Ok(Self {
            disk,
            prepared,
            bytes,
        })
    }
    pub(crate) fn bootstrap_binding(&self) -> SessionResult<SessionBootstrapBindingV2> {
        self.disk.revalidate()?;
        let intent = self.prepared.validate()?;
        let ids = intent.ids().map_err(|_| SessionError::Corrupt)?;
        Ok(SessionBootstrapBindingV2 {
            version: 2,
            installation_id: ids.installation_id,
            control_namespace_uuid: ids.control_namespace_uuid,
            broker_id: ids.broker_id,
            incarnation_id: ids.incarnation_id,
            session_id: ids.session_id,
            attempt_id: ids.attempt_id,
            reservation_id: intent.reservation_id().into(),
            reservation_input_hash: self.prepared.reservation_input_hash.clone(),
            authority_fingerprint: intent.authority().fingerprint(),
            profile_hash: self.prepared.profile_hash.clone(),
            preparation_sha256: commitment(&self.bytes),
        })
    }
}

#[cfg(test)]
#[path = "session_tests.rs"]
pub(crate) mod tests;
