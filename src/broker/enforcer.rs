//! Private stop-only helper of the broker binary. No start or deletion authority.
//!
//! Slots are deliberately retained: an ordinary transport return does not prove
//! that an earlier runtime start can no longer take effect (D5).

mod client;
mod diagnostics;
mod execution;
pub(crate) mod ledger;
mod recovery;
pub(crate) mod retirement;
mod server;

#[cfg(test)]
mod auth_bootstrap_tests;

use crate::{
    control::{ArmReceiptV1, ArmV1, HostClockSampleV1, RuntimeKindV1},
    runtime::{ContainerObservation, OwnedContainerSnapshot, RuntimeAuthority, RuntimeKind},
};
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};
use sha2::{Digest, Sha256};
use std::{
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use uuid::Uuid;

pub(crate) use client::EnforcerFullClientV3;
pub use client::{EnforcedSlot, EnforcerClient};
pub(crate) use diagnostics::EnforcerStageFailure;
pub(crate) use execution::ExecutionEnforcerV1;
pub use recovery::{
    EnforcerStopClient, EnforcerStopClientV3, EnforcerStopStatusV2, EnforcerStopStatusV3,
    StopPhaseV1, StopSlotV1, run_internal_recovery_enforcer,
    run_internal_recovery_enforcer_with_budget,
};
pub use retirement::{
    run_internal_full3_existing, run_internal_full3_existing_with_budget, run_internal_full3_fresh,
    run_internal_full3_fresh_with_budget,
};
pub use server::run_internal_enforcer;

const MAX_SLOTS: u32 = 1024;
const MAX_FRAME: u32 = 512 * 1024;
const MAX_SLOT_QUERY: usize = 32;
const IO_TIMEOUT: Duration = Duration::from_secs(3);
const REGISTER_TIMEOUT: Duration = Duration::from_secs(45);
const HEALTH_WINDOW: Duration = Duration::from_millis(750);
const POLL: Duration = Duration::from_millis(100);

pub type EnforcerResult<T> = Result<T, EnforcerError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnforcerError {
    InvalidInput,
    UnsafeFilesystem,
    Unauthorized,
    Unavailable,
    Conflict,
    Capacity,
    NotReady,
    AlreadyPrepared,
    Unhealthy,
    NotIdle,
}
impl<'de> Deserialize<'de> for EnforcerError {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "snake_case")]
        enum Wire {
            InvalidInput,
            UnsafeFilesystem,
            Unauthorized,
            Unavailable,
            Conflict,
            Capacity,
            NotReady,
            AlreadyPrepared,
            Unhealthy,
            NotIdle,
        }
        Ok(
            match Wire::deserialize(decoder)
                .map_err(|_| D::Error::custom("invalid private enforcer code"))?
            {
                Wire::InvalidInput => Self::InvalidInput,
                Wire::UnsafeFilesystem => Self::UnsafeFilesystem,
                Wire::Unauthorized => Self::Unauthorized,
                Wire::Unavailable => Self::Unavailable,
                Wire::Conflict => Self::Conflict,
                Wire::Capacity => Self::Capacity,
                Wire::NotReady => Self::NotReady,
                Wire::AlreadyPrepared => Self::AlreadyPrepared,
                Wire::Unhealthy => Self::Unhealthy,
                Wire::NotIdle => Self::NotIdle,
            },
        )
    }
}
impl fmt::Display for EnforcerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidInput => "invalid private enforcer request",
            Self::UnsafeFilesystem => "private enforcer filesystem validation failed",
            Self::Unauthorized => "private enforcer authentication failed",
            Self::Unavailable => "private enforcer is unavailable",
            Self::Conflict => "private enforcer registration conflicts with retained state",
            Self::Capacity => "private enforcer slot capacity is exhausted",
            Self::NotReady => "private enforcer stop actor is not ready; no start permitted",
            Self::AlreadyPrepared => "private enforcer start permit has already been consumed",
            Self::Unhealthy => "private enforcer progress or clock is no longer trusted",
            Self::NotIdle => "private enforcer retains armed targets; shutdown refused",
        })
    }
}
impl std::error::Error for EnforcerError {}
impl From<super::local_ipc::IpcError> for EnforcerError {
    fn from(error: super::local_ipc::IpcError) -> Self {
        use super::local_ipc::IpcError;
        match error {
            IpcError::Unauthorized => Self::Unauthorized,
            IpcError::UnsafeFilesystem | IpcError::PathTooLong => Self::UnsafeFilesystem,
            IpcError::Conflict => Self::Conflict,
            IpcError::LimitExceeded => Self::Capacity,
            IpcError::InvalidInput | IpcError::TooLarge => Self::InvalidInput,
            IpcError::Timeout | IpcError::Unavailable => Self::Unavailable,
        }
    }
}

/// In-memory invalidation precedes any awaited durable cancellation. A new
/// explicit post-cancel export can use the new generation; old guards cannot.
#[derive(Clone, Default)]
pub struct CancelGeneration(Arc<Generation>);
#[derive(Default)]
struct Generation {
    value: AtomicU64,
    gate: Mutex<()>,
}
impl fmt::Debug for CancelGeneration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CancelGeneration(<private>)")
    }
}
impl CancelGeneration {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn current(&self) -> u64 {
        self.0.value.load(Ordering::SeqCst)
    }
    /// Retain an observed recovery generation without granting a new start.
    /// The same gate orders this monotone seed, cancellation and final guards.
    pub fn advance_to(&self, observed: u64) -> u64 {
        let Ok(_guard) = self.0.gate.lock() else {
            self.0.value.store(u64::MAX, Ordering::SeqCst);
            return u64::MAX;
        };
        let next = self.current().max(observed);
        self.0.value.store(next, Ordering::SeqCst);
        next
    }
    pub fn invalidate(&self) -> u64 {
        let Ok(_guard) = self.0.gate.lock() else {
            self.0.value.store(u64::MAX, Ordering::SeqCst);
            return u64::MAX;
        };
        let next = self.current().saturating_add(1);
        self.0.value.store(next, Ordering::SeqCst);
        next
    }
}

#[derive(Clone, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RegistrationV1 {
    control_origin_hash: String,
    tenant_id: String,
    arm: ArmV1,
    receipt: ArmReceiptV1,
    authority: RuntimeAuthority,
    owned: OwnedContainerSnapshot,
}
fn deserialize_owned<'de, D: Deserializer<'de>>(
    decoder: D,
) -> Result<OwnedContainerSnapshot, D::Error> {
    // Register is internally tagged, so its content has already been buffered.
    // Install the guard directly around the legacy snapshot/observation structs.
    let mut ignored = false;
    let value = serde_ignored::deserialize(decoder, |_| ignored = true)
        .map_err(|_| D::Error::custom("invalid private enforcer snapshot"))?;
    if ignored {
        return Err(D::Error::custom("invalid private enforcer snapshot"));
    }
    Ok(value)
}
impl<'de> Deserialize<'de> for RegistrationV1 {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            control_origin_hash: String,
            tenant_id: String,
            arm: ArmV1,
            receipt: ArmReceiptV1,
            authority: RuntimeAuthority,
            #[serde(deserialize_with = "deserialize_owned")]
            owned: OwnedContainerSnapshot,
        }
        let mut ignored = false;
        let wire: Wire = serde_ignored::deserialize(decoder, |_| ignored = true)
            .map_err(|_| D::Error::custom("invalid private enforcer registration"))?;
        if ignored {
            return Err(D::Error::custom("invalid private enforcer registration"));
        }
        Self::new(
            wire.control_origin_hash,
            wire.tenant_id,
            wire.arm,
            wire.receipt,
            wire.authority,
            wire.owned,
        )
        .map_err(|_| D::Error::custom("invalid private enforcer registration"))
    }
}
impl fmt::Debug for RegistrationV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RegistrationV1(<private stop target>)")
    }
}
impl RegistrationV1 {
    pub fn new(
        control_origin_hash: String,
        tenant_id: String,
        arm: ArmV1,
        receipt: ArmReceiptV1,
        authority: RuntimeAuthority,
        owned: OwnedContainerSnapshot,
    ) -> EnforcerResult<Self> {
        let value = Self {
            control_origin_hash,
            tenant_id,
            arm,
            receipt,
            authority,
            owned,
        };
        value.validate()?;
        Ok(value)
    }

    fn validate(&self) -> EnforcerResult<()> {
        let _validated_target = self.prospective()?;
        self.receipt
            .validate_for_arm(&self.arm)
            .map_err(|_| EnforcerError::InvalidInput)
    }

    pub(crate) fn prospective(&self) -> EnforcerResult<ProspectiveRegistrationV1> {
        ProspectiveRegistrationV1::new(
            self.control_origin_hash.clone(),
            self.tenant_id.clone(),
            self.arm.clone(),
            self.authority.clone(),
            self.owned.clone(),
        )
    }

    pub(crate) fn arm(&self) -> &ArmV1 {
        &self.arm
    }
    pub(crate) fn arm_id(&self) -> &str {
        &self.arm.operation_id
    }
    pub(crate) fn receipt(&self) -> &ArmReceiptV1 {
        &self.receipt
    }
    pub(crate) fn control_origin_hash(&self) -> &str {
        &self.control_origin_hash
    }
    pub(crate) fn tenant_id(&self) -> &str {
        &self.tenant_id
    }
    pub(crate) fn digest(&self) -> EnforcerResult<String> {
        self.validate()?;
        // Full private binding, including invocation, not authority.fingerprint
        // alone. Serialization traverses structs and ordered invocation maps.
        let bytes = serde_json::to_vec(self).map_err(|_| EnforcerError::InvalidInput)?;
        if bytes.len() > 256 * 1024 {
            return Err(EnforcerError::InvalidInput);
        }
        let mut digest = Sha256::new();
        digest.update(b"ai-guard:enforcer-registration:v1\0");
        digest.update(bytes);
        Ok(hex::encode(digest.finalize()))
    }
}

/// Exact immutable STOP target reserved before an Arm exchange. This is data;
/// only the authenticated native factory can install an owner from it. It has
/// no receipt, no Original grant and no start permission.
#[derive(Clone, Serialize)]
pub(crate) struct ProspectiveRegistrationV1 {
    control_origin_hash: String,
    tenant_id: String,
    arm: ArmV1,
    authority: RuntimeAuthority,
    owned: OwnedContainerSnapshot,
}
impl<'de> Deserialize<'de> for ProspectiveRegistrationV1 {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            control_origin_hash: String,
            tenant_id: String,
            arm: ArmV1,
            authority: RuntimeAuthority,
            #[serde(deserialize_with = "deserialize_owned")]
            owned: OwnedContainerSnapshot,
        }
        let mut ignored = false;
        let wire: Wire = serde_ignored::deserialize(decoder, |_| ignored = true)
            .map_err(|_| D::Error::custom("invalid private prospective target"))?;
        if ignored {
            return Err(D::Error::custom("invalid private prospective target"));
        }
        Self::new(
            wire.control_origin_hash,
            wire.tenant_id,
            wire.arm,
            wire.authority,
            wire.owned,
        )
        .map_err(|_| D::Error::custom("invalid private prospective target"))
    }
}
impl fmt::Debug for ProspectiveRegistrationV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ProspectiveRegistrationV1(<private stop target>)")
    }
}
impl ProspectiveRegistrationV1 {
    pub(crate) fn new(
        control_origin_hash: String,
        tenant_id: String,
        arm: ArmV1,
        authority: RuntimeAuthority,
        owned: OwnedContainerSnapshot,
    ) -> EnforcerResult<Self> {
        let value = Self {
            control_origin_hash,
            tenant_id,
            arm,
            authority,
            owned,
        };
        value.validate()?;
        Ok(value)
    }
    pub(crate) fn validate(&self) -> EnforcerResult<()> {
        let invalid = || EnforcerError::InvalidInput;
        self.authority
            .validate_docker_launch_profile()
            .map_err(|_| invalid())?;
        if self
            .authority
            .docker_launch_profile
            .as_ref()
            .is_some_and(|profile| profile.intent_hash != self.arm.intent_hash)
        {
            return Err(invalid());
        }
        self.arm.validate().map_err(|_| invalid())?;
        crate::tenancy::validate_tenant_id(&self.tenant_id).map_err(|_| invalid())?;
        let inspection = &self.arm.inspection;
        let handle = &self.owned.handle;
        let observation = &self.owned.observation;
        let runtime_kind = match self.authority.kind {
            RuntimeKind::Docker => RuntimeKindV1::Docker,
            RuntimeKind::Podman => RuntimeKindV1::Podman,
        };
        let cpu = observation.allocation.cpu.as_ref().ok_or_else(invalid)?;
        if !hex_hash(&self.control_origin_hash)
            || self.authority.authority_id.to_string() != inspection.authority_id
            || runtime_kind != inspection.runtime_kind
            || self.authority.invocation.is_none()
            || self.authority.fingerprint() != self.arm.authority_binding_hash
            || handle.authority_fingerprint() != self.arm.authority_binding_hash
            || handle.session_id().is_nil()
            || handle.operation_id().is_nil()
            || !hex_hash(handle.id())
            || handle.id() != inspection.container_id
            || handle.name() != inspection.stable_name
            || observation.id.as_deref() != Some(handle.id())
            || observation.name != inspection.stable_name
            || observation.image_id.as_deref() != Some(inspection.image_id.as_str())
            || observation.configuration_fingerprint.as_deref()
                != Some(self.arm.configuration_fingerprint.as_str())
            || observation.configuration_fingerprint_version.unwrap_or(1)
                != self.arm.configuration_fingerprint_version
            || handle.fingerprint_version() != self.arm.configuration_fingerprint_version
            || cpu.period == 0
            || u128::from(cpu.quota) * 1000
                != u128::from(inspection.cpu_millis) * u128::from(cpu.period)
            || observation.allocation.memory_bytes != Some(inspection.memory_bytes)
            || !observation
                .allocation
                .pids_limit
                .is_some_and(|value| u32::try_from(value).ok() == Some(inspection.pids_limit))
        {
            return Err(invalid());
        }
        if let crate::control::ArmSubjectV1::Helper { helper_id, .. } = &self.arm.subject
            && handle.operation_id().to_string() != *helper_id
        {
            return Err(invalid());
        }
        observation
            .validate_identity_allocation(observation)
            .map_err(|_| invalid())?;
        Ok(())
    }

    pub(crate) fn arm(&self) -> &ArmV1 {
        &self.arm
    }
    pub(crate) fn arm_id(&self) -> &str {
        &self.arm.operation_id
    }
    pub(crate) fn authority(&self) -> &RuntimeAuthority {
        &self.authority
    }
    pub(crate) fn owned(&self) -> &OwnedContainerSnapshot {
        &self.owned
    }
    pub(crate) fn control_origin_hash(&self) -> &str {
        &self.control_origin_hash
    }
    pub(crate) fn tenant_id(&self) -> &str {
        &self.tenant_id
    }
    pub(crate) fn matches_registration(&self, registration: &RegistrationV1) -> EnforcerResult<()> {
        registration.validate()?;
        if self.digest()? != registration.prospective()?.digest()? {
            return Err(EnforcerError::Conflict);
        }
        Ok(())
    }
    pub(crate) fn digest(&self) -> EnforcerResult<String> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|_| EnforcerError::InvalidInput)?;
        if bytes.len() > 256 * 1024 {
            return Err(EnforcerError::InvalidInput);
        }
        let mut digest = Sha256::new();
        digest.update(b"ai-guard:enforcer-prospective:v1\0");
        digest.update(bytes);
        Ok(hex::encode(digest.finalize()))
    }
}

/// Acknowledgement from a real native prospective owner, bound to its epoch,
/// admission generation and exact pending Arm. Serialized bytes never create
/// an Original grant; the server checks its retained owner on every use.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProspectiveReservationV1 {
    #[serde(deserialize_with = "crate::broker::retirement::strict_uuid")]
    epoch: Uuid,
    admission_generation: u64,
    cancel_generation: u64,
    #[serde(deserialize_with = "crate::broker::retirement::strict_uuid")]
    reservation_nonce: Uuid,
    arm_id: String,
    prospective_hash: String,
}
impl ProspectiveReservationV1 {
    pub(crate) fn validate_for(
        &self,
        prospective: &ProspectiveRegistrationV1,
        epoch: Uuid,
        cancel_generation: u64,
    ) -> EnforcerResult<()> {
        if self.epoch.is_nil()
            || self.epoch != epoch
            || self.reservation_nonce.is_nil()
            || self.admission_generation == u64::MAX
            || self.cancel_generation == u64::MAX
            || self.cancel_generation != cancel_generation
            || !hex_hash(&self.prospective_hash)
            || self.arm_id != prospective.arm_id()
            || self.prospective_hash != prospective.digest()?
        {
            return Err(EnforcerError::Conflict);
        }
        Ok(())
    }
}

fn hex_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|value| value.is_ascii_digit() || (b'a'..=b'f').contains(&value))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnforcerStatusV1 {
    epoch: Uuid,
    slot_count: usize,
    max_slots: u32,
    progress: u64,
    healthy: bool,
}
impl EnforcerStatusV1 {
    pub fn epoch(&self) -> Uuid {
        self.epoch
    }
    pub fn slot_count(&self) -> usize {
        self.slot_count
    }
    pub fn healthy(&self) -> bool {
        self.healthy
    }
    pub fn progress(&self) -> u64 {
        self.progress
    }
    pub fn max_slots(&self) -> u32 {
        self.max_slots
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlotPhaseV1 {
    Initializing,
    Ready,
    Stopping,
    Quarantined,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnforcerSlotStatusV1 {
    arm_id: String,
    registration_hash: String,
    phase: SlotPhaseV1,
    prepared_once: bool,
    generation: u64,
    clock: Option<HostClockSampleV1>,
    #[serde(deserialize_with = "deserialize_observation")]
    observation: Option<ContainerObservation>,
    stop_failures: u64,
}
fn deserialize_observation<'de, D: Deserializer<'de>>(
    decoder: D,
) -> Result<Option<ContainerObservation>, D::Error> {
    // Responses buffer their tagged content too. Close the historical runtime
    // observation exactly here, including process/allocation nested fields.
    let mut ignored = false;
    let value = serde_ignored::deserialize(decoder, |_| ignored = true)
        .map_err(|_| D::Error::custom("invalid private enforcer observation"))?;
    if ignored {
        return Err(D::Error::custom("invalid private enforcer observation"));
    }
    Ok(value)
}
impl fmt::Debug for EnforcerSlotStatusV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EnforcerSlotStatusV1(<private stop state>)")
    }
}
impl EnforcerSlotStatusV1 {
    pub fn arm_id(&self) -> &str {
        &self.arm_id
    }
    pub fn phase(&self) -> SlotPhaseV1 {
        self.phase
    }
    pub fn prepared_once(&self) -> bool {
        self.prepared_once
    }
    pub fn observation(&self) -> Option<&ContainerObservation> {
        self.observation.as_ref()
    }
    pub fn stop_failures(&self) -> u64 {
        self.stop_failures
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Challenge {
    epoch: Uuid,
    nonce: Uuid,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Authentication {
    proof: String,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Status {},
    Watch {},
    Slots {
        arm_ids: Vec<String>,
    },
    Register {
        registration: Box<RegistrationV1>,
        generation: u64,
        observe_only: bool,
    },
    Prepare {
        arm_id: String,
        registration_hash: String,
        generation: u64,
    },
    Cancel {
        arm_id: String,
        registration_hash: String,
        generation: u64,
    },
    ShutdownIfIdle {},
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Response {
    Status {
        value: EnforcerStatusV1,
    },
    Slots {
        values: Vec<EnforcerSlotStatusV1>,
    },
    Registered {
        value: Box<EnforcerSlotStatusV1>,
        replayed: bool,
    },
    Prepared {
        value: Box<EnforcerSlotStatusV1>,
    },
    Acknowledged {},
    Error {
        code: EnforcerError,
    },
}

fn proof(capability: &[u8; 32], challenge: &Challenge) -> String {
    let mut digest = Sha256::new();
    digest.update(b"ai-guard:enforcer-auth:v1\0");
    digest.update(capability);
    digest.update(challenge.epoch.as_bytes());
    digest.update(challenge.nonce.as_bytes());
    hex::encode(digest.finalize())
}

fn capability(root: &crate::control_fs::Directory, create: bool) -> EnforcerResult<[u8; 32]> {
    use std::io::Read;
    let map_error = |_| EnforcerError::UnsafeFilesystem;
    let retained = match root.persistent_read("auth", 32, &[]).map_err(map_error)? {
        Some(file) => file,
        None if create => {
            let mut bytes = [0; 32];
            std::fs::File::open("/dev/urandom")
                .and_then(|mut file| file.read_exact(&mut bytes))
                .map_err(|_| EnforcerError::Unavailable)?;
            match root.persistent_create("auth", &bytes) {
                Ok(file) => {
                    file.persistent_sync().map_err(map_error)?;
                    root.sync().map_err(map_error)?;
                    file
                }
                Err(_) => root
                    .persistent_read("auth", 32, &[])
                    .map_err(map_error)?
                    .ok_or(EnforcerError::UnsafeFilesystem)?,
            }
        }
        None => return Err(EnforcerError::Unauthorized),
    };
    retained.persistent_revalidate().map_err(map_error)?;
    retained
        .bytes()
        .try_into()
        .map_err(|_| EnforcerError::UnsafeFilesystem)
}
