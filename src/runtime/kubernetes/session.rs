//! Durable V2 preparation from a real central before-effect confirmation,
//! then the before-effect inventory r2: the workspace claim named for the
//! attempt, the exact Pod request bytes and the before-effect abandonment
//! (the local fence retained before the central close). No record, replay,
//! view or bootstrap binding grants CREATE, release, close or execution; a
//! retained abandonment is not a central close and releases no quota.
#![cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "closed effect-stage journal helpers coexist with R2 production pre-effect callers"
    )
)]
use super::{
    session_instance::KubernetesSessionWorkspaceClaimV2,
    session_pod::{KubernetesSessionPodRequestV2, session_workspace_claim_name},
};
use crate::{
    audit::kubernetes_session::ConfirmedSessionReservationV2,
    control::kubernetes_session::{
        KubernetesSessionEffectFenceV2, KubernetesSessionReservationCloseReasonV2,
        KubernetesSessionReservationCloseV2, KubernetesSessionReservationIntentV2,
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
const MAX_WORKSPACE_RECORD: u64 = 8 * 1024;
const MAX_POD_REQUEST: u64 = 32 * 1024;
const MAX_ABANDONMENT: u64 = 4 * 1024;
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
        confirmed
            .revalidate_material()
            .map_err(|_| SessionError::UnsafeMaterial)?;
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

fn binding_for(prepared: &Preparation, bytes: &[u8]) -> SessionResult<SessionBootstrapBindingV2> {
    let intent = prepared.validate()?;
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
        reservation_input_hash: prepared.reservation_input_hash.clone(),
        authority_fingerprint: intent.authority().fingerprint(),
        profile_hash: prepared.profile_hash.clone(),
        preparation_sha256: commitment(bytes),
    })
}

/// Local before-effect phase of one journal. Not a central state or permit:
/// `Abandoned` means that the close is retained locally, not that the Store
/// committed it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SessionPhaseV2 {
    Prepared,
    WorkspaceRetained,
    RequestRetained,
    Abandoned,
}
fn phase_of(claim: bool, request: bool, abandoned: bool) -> SessionPhaseV2 {
    if abandoned {
        SessionPhaseV2::Abandoned
    } else if request {
        SessionPhaseV2::RequestRetained
    } else if claim {
        SessionPhaseV2::WorkspaceRetained
    } else {
        SessionPhaseV2::Prepared
    }
}

#[derive(Serialize)]
struct WorkspaceRecord<'a> {
    version: u32,
    preparation_sha256: &'a str,
    claim: &'a KubernetesSessionWorkspaceClaimV2,
}
fn workspace_record(
    claim: &KubernetesSessionWorkspaceClaimV2,
    preparation_sha256: &str,
) -> SessionResult<Vec<u8>> {
    let bytes = canonical(&WorkspaceRecord {
        version: 1,
        preparation_sha256,
        claim,
    })?;
    if bytes.len() as u64 > MAX_WORKSPACE_RECORD {
        return Err(SessionError::Capacity);
    }
    Ok(bytes)
}
fn corrupt_unless(valid: bool) -> SessionResult<()> {
    if valid {
        Ok(())
    } else {
        Err(SessionError::Corrupt)
    }
}
/// A retained record must re-encode to its exact bytes, name the claim of
/// this attempt and authority, and follow this exact preparation.
fn retained_claim(
    bytes: &[u8],
    intent: &KubernetesSessionReservationIntentV2,
    preparation_sha256: &str,
) -> SessionResult<KubernetesSessionWorkspaceClaimV2> {
    let value = crate::json::decode_unique(bytes, 16).map_err(|_| SessionError::Corrupt)?;
    let Value::Object(mut fields) = value else {
        return Err(SessionError::Corrupt);
    };
    let claim = KubernetesSessionWorkspaceClaimV2::retained_from_value(
        fields.remove("claim").ok_or(SessionError::Corrupt)?,
    )
    .map_err(|_| SessionError::Corrupt)?;
    let ids = intent.ids().map_err(|_| SessionError::Corrupt)?;
    corrupt_unless(
        fields.len() == 2
            && fields.get("version").and_then(Value::as_u64) == Some(1)
            && fields.get("preparation_sha256").and_then(Value::as_str) == Some(preparation_sha256)
            && claim.is_for(
                intent.authority(),
                &session_workspace_claim_name(ids.attempt_id),
            )
            && workspace_record(&claim, preparation_sha256)? == bytes,
    )?;
    Ok(claim)
}
#[derive(Serialize)]
struct AbandonmentRecord<'a> {
    version: u32,
    preparation_sha256: &'a str,
    close: &'a KubernetesSessionReservationCloseV2,
}
fn abandonment_record(
    close: &KubernetesSessionReservationCloseV2,
    preparation_sha256: &str,
) -> SessionResult<Vec<u8>> {
    let bytes = canonical(&AbandonmentRecord {
        version: 1,
        preparation_sha256,
        close,
    })?;
    if bytes.len() as u64 > MAX_ABANDONMENT {
        return Err(SessionError::Capacity);
    }
    Ok(bytes)
}
/// The only close a journal retains: the before-effect abandonment of its own
/// reservation at revision 1, under a valid operation id of its own.
fn abandonment_is_for(
    close: &KubernetesSessionReservationCloseV2,
    intent: &KubernetesSessionReservationIntentV2,
    binding: &SessionBootstrapBindingV2,
) -> bool {
    close.validate().is_ok()
        && close.reservation_id == intent.reservation_id()
        && close.reservation_input_hash == binding.reservation_input_hash
        && close.expected_revision == 1
        && close.reason == KubernetesSessionReservationCloseReasonV2::PreparationAbandoned
}
/// A retained abandonment must re-encode to its exact bytes, follow this
/// exact preparation and close only this reservation before any effect.
fn retained_abandonment(
    bytes: &[u8],
    intent: &KubernetesSessionReservationIntentV2,
    binding: &SessionBootstrapBindingV2,
) -> SessionResult<KubernetesSessionReservationCloseV2> {
    let value = crate::json::decode_unique(bytes, 16).map_err(|_| SessionError::Corrupt)?;
    let Value::Object(mut fields) = value else {
        return Err(SessionError::Corrupt);
    };
    let close: KubernetesSessionReservationCloseV2 =
        serde_json::from_value(fields.remove("close").ok_or(SessionError::Corrupt)?)
            .map_err(|_| SessionError::Corrupt)?;
    corrupt_unless(
        fields.len() == 2
            && fields.get("version").and_then(Value::as_u64) == Some(1)
            && fields.get("preparation_sha256").and_then(Value::as_str)
                == Some(binding.preparation_sha256.as_str())
            && abandonment_is_for(&close, intent, binding)
            && abandonment_record(&close, &binding.preparation_sha256)? == bytes,
    )?;
    Ok(close)
}

/// Validated records that follow one preparation.
struct RetainedChain {
    claim: Option<KubernetesSessionWorkspaceClaimV2>,
    request: Option<KubernetesSessionPodRequestV2>,
    abandonment: Option<KubernetesSessionReservationCloseV2>,
}
/// The retained request is valid only if the pure renderer reproduces it.
fn retained_chain(
    workspace: Option<&[u8]>,
    pod: Option<&[u8]>,
    abandoned: Option<&[u8]>,
    intent: &KubernetesSessionReservationIntentV2,
    binding: &SessionBootstrapBindingV2,
) -> SessionResult<RetainedChain> {
    let claim = workspace
        .map(|bytes| retained_claim(bytes, intent, &binding.preparation_sha256))
        .transpose()?;
    let request = match (pod, &claim) {
        (None, _) => None,
        (Some(bytes), Some(claim)) => {
            let request = KubernetesSessionPodRequestV2::render(intent, binding, claim)
                .map_err(|_| SessionError::Corrupt)?;
            corrupt_unless(request.bytes() == bytes)?;
            Some(request)
        }
        (Some(_), None) => return Err(SessionError::Corrupt),
    };
    let abandonment = abandoned
        .map(|bytes| retained_abandonment(bytes, intent, binding))
        .transpose()?;
    Ok(RetainedChain {
        claim,
        request,
        abandonment,
    })
}

pub(crate) struct SessionJournal {
    disk: disk::SessionDisk,
    prepared: Preparation,
    bytes: Vec<u8>,
    claim: Option<KubernetesSessionWorkspaceClaimV2>,
    request: Option<KubernetesSessionPodRequestV2>,
    abandonment: Option<KubernetesSessionReservationCloseV2>,
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
        // A present next file is adopted only after full predecessor
        // validation; an invalid one is Corrupt and preserved.
        let intent = prepared.validate()?;
        let binding = binding_for(&prepared, &bytes)?;
        let RetainedChain {
            claim,
            request,
            abandonment,
        } = retained_chain(
            disk.workspace_bytes(),
            disk.pod_bytes(),
            disk.abandoned_bytes(),
            &intent,
            &binding,
        )?;
        Ok(Self {
            disk,
            prepared,
            bytes,
            claim,
            request,
            abandonment,
        })
    }
    pub(crate) fn bootstrap_binding(&self) -> SessionResult<SessionBootstrapBindingV2> {
        self.disk.revalidate()?;
        binding_for(&self.prepared, &self.bytes)
    }

    /// Revalidated copy of the confirmed intent retained by this journal.
    pub(crate) fn intent(&self) -> SessionResult<KubernetesSessionReservationIntentV2> {
        self.disk.revalidate()?;
        self.prepared.validate()
    }

    pub(crate) fn phase(&self) -> SessionResult<SessionPhaseV2> {
        self.disk.revalidate()?;
        Ok(phase_of(
            self.claim.is_some(),
            self.request.is_some(),
            self.abandonment.is_some(),
        ))
    }

    /// Revalidated copy of the retained claim. A later observation of the
    /// same PVC (new resourceVersion or time) is not retained again: callers
    /// reuse this one, and the claim UID stays the one first retained.
    pub(crate) fn workspace_claim(
        &self,
    ) -> SessionResult<Option<KubernetesSessionWorkspaceClaimV2>> {
        self.disk.revalidate()?;
        Ok(self.claim.clone())
    }

    /// Retains the claim observed for this attempt before any request. The
    /// same claim again is an idempotent success; any other claim conflicts.
    /// A claim named for another attempt or authority is never adopted.
    pub(crate) fn retain_workspace_claim(
        &mut self,
        claim: &KubernetesSessionWorkspaceClaimV2,
    ) -> SessionResult<()> {
        self.disk.revalidate()?;
        require(self.abandonment.is_none())?;
        let intent = self.prepared.validate()?;
        let ids = intent.ids().map_err(|_| SessionError::Corrupt)?;
        require(claim.is_for(
            intent.authority(),
            &session_workspace_claim_name(ids.attempt_id),
        ))?;
        let bytes = workspace_record(claim, &commitment(&self.bytes))?;
        self.disk.screen(&bytes)?;
        if self.claim.is_some() {
            return require(self.disk.workspace_bytes() == Some(bytes.as_slice()));
        }
        self.disk.retain_workspace(&bytes)?;
        self.claim = Some(claim.clone());
        Ok(())
    }

    /// Retains the exact request only if the pure renderer reproduces it from
    /// the retained intent, preparation and claim. Retention is not CREATE.
    pub(crate) fn retain_pod_request(
        &mut self,
        request: &KubernetesSessionPodRequestV2,
    ) -> SessionResult<()> {
        self.disk.revalidate()?;
        require(self.abandonment.is_none())?;
        let intent = self.prepared.validate()?;
        let binding = binding_for(&self.prepared, &self.bytes)?;
        let claim = self.claim.as_ref().ok_or(SessionError::Conflict)?;
        let expected = KubernetesSessionPodRequestV2::render(&intent, &binding, claim)
            .map_err(|_| SessionError::Corrupt)?;
        require(expected == *request)?;
        self.disk.screen(expected.bytes())?;
        if self.request.is_some() {
            return require(self.disk.pod_bytes() == Some(expected.bytes()));
        }
        self.disk.retain_pod(expected.bytes())?;
        self.request = Some(expected);
        Ok(())
    }

    /// Retains this journal's before-effect abandonment before any central
    /// close: the local fence comes first and keeps the operation id stable
    /// across lost responses and restarts. The same close again is an
    /// idempotent success; any other close conflicts, and no claim or request
    /// is retained afterwards. Retention is not a close: the reservation stays
    /// Reserved, with its quota held, until the Store commits the close.
    pub(crate) fn retain_abandonment(
        &mut self,
        close: &KubernetesSessionReservationCloseV2,
    ) -> SessionResult<()> {
        self.disk.revalidate()?;
        let intent = self.prepared.validate()?;
        let binding = binding_for(&self.prepared, &self.bytes)?;
        require(abandonment_is_for(close, &intent, &binding))?;
        let bytes = abandonment_record(close, &binding.preparation_sha256)?;
        self.disk.screen(&bytes)?;
        if self.abandonment.is_some() {
            return require(self.disk.abandoned_bytes() == Some(bytes.as_slice()));
        }
        self.disk.retain_abandoned(&bytes)?;
        self.abandonment = Some(close.clone());
        Ok(())
    }

    /// Revalidated copy of the retained abandonment, if any.
    pub(crate) fn retained_abandonment(
        &self,
    ) -> SessionResult<Option<KubernetesSessionReservationCloseV2>> {
        self.disk.revalidate()?;
        Ok(self.abandonment.clone())
    }

    /// Observe-only reopen: the writer lock is taken first (Busy for a live
    /// owner), the full chain is validated, the bytes are copied and the lock
    /// is released before returning. Creates no file, needs no confirmation
    /// and grants no CREATE, release or close right, nor any central proof.
    pub(crate) fn inspect_retained(
        root: Directory,
        protection: SessionJournalProtection,
    ) -> SessionResult<RetainedSessionViewV2> {
        let retained = disk::SessionDisk::inspect(root, protection)?;
        let value = crate::json::decode_unique(&retained.prepared, 64)
            .map_err(|_| SessionError::Corrupt)?;
        let prepared: Preparation =
            serde_json::from_value(value).map_err(|_| SessionError::Corrupt)?;
        let intent = prepared.validate().map_err(|_| SessionError::Corrupt)?;
        corrupt_unless(canonical(&prepared)? == retained.prepared)?;
        let binding = binding_for(&prepared, &retained.prepared)?;
        let RetainedChain {
            claim,
            request,
            abandonment,
        } = retained_chain(
            retained.workspace.as_deref(),
            retained.pod.as_deref(),
            retained.abandoned.as_deref(),
            &intent,
            &binding,
        )?;
        Ok(RetainedSessionViewV2 {
            phase: phase_of(claim.is_some(), request.is_some(), abandonment.is_some()),
            intent,
            preparation_sha256: binding.preparation_sha256,
            claim,
            request,
            abandonment,
        })
    }
}

/// Byte snapshot of one validated observe-only reading. Holds no lock and is
/// no proof of central state, liveness or a never-issued effect.
pub(crate) struct RetainedSessionViewV2 {
    phase: SessionPhaseV2,
    intent: KubernetesSessionReservationIntentV2,
    preparation_sha256: String,
    claim: Option<KubernetesSessionWorkspaceClaimV2>,
    request: Option<KubernetesSessionPodRequestV2>,
    abandonment: Option<KubernetesSessionReservationCloseV2>,
}
impl RetainedSessionViewV2 {
    pub(crate) fn phase(&self) -> SessionPhaseV2 {
        self.phase
    }
    pub(crate) fn intent(&self) -> &KubernetesSessionReservationIntentV2 {
        &self.intent
    }
    pub(crate) fn preparation_sha256(&self) -> &str {
        &self.preparation_sha256
    }
    pub(crate) fn claim(&self) -> Option<&KubernetesSessionWorkspaceClaimV2> {
        self.claim.as_ref()
    }
    pub(crate) fn request(&self) -> Option<&KubernetesSessionPodRequestV2> {
        self.request.as_ref()
    }
    /// The locally retained close; the central state is not observed here.
    pub(crate) fn abandonment(&self) -> Option<&KubernetesSessionReservationCloseV2> {
        self.abandonment.as_ref()
    }
}

#[cfg(test)]
#[path = "session_tests.rs"]
pub(crate) mod tests;
