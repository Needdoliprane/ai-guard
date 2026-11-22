//! Central V2 effect ledger contract (G11: R1, Q7, quota settlement).
//!
//! Descriptive DTOs exchanged between the authenticated broker and the
//! tenant Store. No value here is an effect permit by itself: a claim
//! acknowledgement with the `EmitOnce` disposition becomes one only after the
//! broker retained it in its private journal (S5). The tenant always comes
//! from the authenticated broker identity, never from these bodies.
//!
//! Steps form one strictly increasing sequence per reservation. The Store
//! re-renders every request whose bytes it can derive (claim and Pod CREATE,
//! gate and finalizer PATCH, DELETE) and compares the digest before granting
//! `EmitOnce`; only the attach digest, whose identity bytes come from two
//! broker observations, stays a declared broker commitment bound to the
//! exact Pod UID. A replay of a consumed step returns `ObserveOnly` with its
//! historical receipt, never a second `EmitOnce`.
use super::validation::{digest, hash, require, uuid};
use crate::runtime::kubernetes::session_claim::KubernetesSessionStorageRequestV2;
use serde::{Deserialize, Serialize};

/// Upper bound of effect steps of one attempt: claim, two Pods with their
/// create, release, attach, finalizer and delete, then the claim delete,
/// with room for the deletions of an abandoned path.
pub const MAX_EFFECT_STEPS: u32 = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KubernetesSessionEffectSubjectV2 {
    WorkspaceClaim,
    SessionPod,
    ExportPod,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KubernetesSessionEffectActionV2 {
    Create,
    ReleaseGate,
    Attach,
    ReleaseFinalizer,
    Delete,
}
impl KubernetesSessionEffectActionV2 {
    /// The only valid subject/action pairs: a claim is created and deleted;
    /// a Pod goes through every action.
    pub fn applies_to(self, subject: KubernetesSessionEffectSubjectV2) -> bool {
        match subject {
            KubernetesSessionEffectSubjectV2::WorkspaceClaim => {
                matches!(self, Self::Create | Self::Delete)
            }
            KubernetesSessionEffectSubjectV2::SessionPod
            | KubernetesSessionEffectSubjectV2::ExportPod => true,
        }
    }
}

/// Durable deadline armed with the broker enforcer before a gate release
/// (R3). The Store keeps it with the claim; it never computes a deadline from
/// its own clock during a replay.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KubernetesSessionDeadlineArmV2 {
    pub version: u32,
    pub arm_id: String,
    /// RFC 3339 UTC, whole seconds; an absolute bound for the enforcer.
    pub deadline_at: String,
    /// Digest of the enforcer's durable acknowledgement of this arm.
    pub enforcer_receipt_sha256: String,
}
impl KubernetesSessionDeadlineArmV2 {
    pub fn validate(&self) -> anyhow::Result<()> {
        require(self.version == 1 && exact_utc_seconds(&self.deadline_at))?;
        uuid(&self.arm_id)?;
        hash(&self.enforcer_receipt_sha256)
    }
}

/// One exact effect request to claim before any byte is sent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KubernetesSessionEffectRequestV2 {
    pub version: u32,
    pub operation_id: String,
    pub reservation_id: String,
    pub reservation_input_hash: String,
    pub step: u32,
    pub subject: KubernetesSessionEffectSubjectV2,
    pub action: KubernetesSessionEffectActionV2,
    pub request_sha256: String,
    /// UID of the addressed object; absent only for a `Create`.
    pub object_uid: Option<String>,
    /// Observed resourceVersion bound in the request body: required for a
    /// gate release, optional for a Pod DELETE of a Held observation.
    pub object_resource_version: Option<String>,
    /// Tenant storage request of the workspace claim CREATE only (R5).
    pub storage: Option<KubernetesSessionStorageRequestV2>,
    /// Armed durable deadline of a gate release only (R3).
    pub deadline: Option<KubernetesSessionDeadlineArmV2>,
}
impl KubernetesSessionEffectRequestV2 {
    pub fn validate(&self) -> anyhow::Result<()> {
        use KubernetesSessionEffectActionV2 as Action;
        use KubernetesSessionEffectSubjectV2 as Subject;
        require(
            self.version == 1
                && (1..=MAX_EFFECT_STEPS).contains(&self.step)
                && self.action.applies_to(self.subject)
                && self.operation_id != self.reservation_id,
        )?;
        uuid(&self.operation_id)?;
        uuid(&self.reservation_id)?;
        hash(&self.reservation_input_hash)?;
        hash(&self.request_sha256)?;
        let create = self.action == Action::Create;
        require(create == self.object_uid.is_none())?;
        if let Some(uid) = &self.object_uid {
            uuid(uid)?;
        }
        // A release always tests the observed resourceVersion; a Pod DELETE
        // of a Held observation adds it as a precondition. Nothing else has one.
        let rv_allowed = self.action == Action::ReleaseGate
            || (self.action == Action::Delete && self.subject != Subject::WorkspaceClaim);
        require(
            (self.action != Action::ReleaseGate || self.object_resource_version.is_some())
                && (rv_allowed || self.object_resource_version.is_none())
                && self
                    .object_resource_version
                    .as_deref()
                    .is_none_or(valid_resource_version),
        )?;
        let claim_create = create && self.subject == Subject::WorkspaceClaim;
        require(claim_create == self.storage.is_some())?;
        if let Some(storage) = &self.storage {
            storage
                .validate()
                .map_err(|_| super::KubernetesSessionReservationFailureV2::InvalidRequest)?;
        }
        require((self.action == Action::ReleaseGate) == self.deadline.is_some())?;
        if let Some(deadline) = &self.deadline {
            deadline.validate()?;
        }
        Ok(())
    }
    pub fn input_hash(&self) -> anyhow::Result<String> {
        self.validate()?;
        digest(b"ai-guard:kubernetes-session-effect-request:v1\0", self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KubernetesSessionEffectDispositionV2 {
    /// This acknowledgement may become the single permit of its request.
    EmitOnce,
    /// The step was already claimed: observe, never send.
    ObserveOnly,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KubernetesSessionEffectReceiptV2 {
    pub version: u32,
    pub operation_id: String,
    pub input_hash: String,
    pub control_namespace_uuid: String,
    pub audit_seq: i64,
    pub audit_hash: String,
}

/// Central answer to one claim. `not_after` bounds the use of an `EmitOnce`
/// permit; a lost answer is resolved by replaying the same request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KubernetesSessionEffectClaimAckV2 {
    pub version: u32,
    pub request: KubernetesSessionEffectRequestV2,
    pub disposition: KubernetesSessionEffectDispositionV2,
    pub issued_at: String,
    pub not_after: String,
    pub receipt: KubernetesSessionEffectReceiptV2,
    pub replayed: bool,
}
impl KubernetesSessionEffectClaimAckV2 {
    pub fn validate(&self) -> anyhow::Result<()> {
        self.request.validate()?;
        require(
            self.version == 1
                && exact_utc_seconds(&self.issued_at)
                && exact_utc_seconds(&self.not_after)
                && self.issued_at < self.not_after
                && self.receipt.version == 1
                && self.receipt.operation_id == self.request.operation_id
                && self.receipt.input_hash == self.request.input_hash()?
                && self.receipt.audit_seq > 0
                // A replay never grants a fresh emission.
                && !(self.replayed
                    && self.disposition == KubernetesSessionEffectDispositionV2::EmitOnce),
        )?;
        uuid(&self.receipt.control_namespace_uuid)?;
        hash(&self.receipt.audit_hash)
    }
}

/// What the broker observed for one claimed request. Only `Applied` binds an
/// object UID; `Uncertain` keeps ownership unknown and blocks settlement
/// until an authorized recovery resolves it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum KubernetesSessionEffectOutcomeV2 {
    Applied {
        object_uid: String,
        resource_version: String,
    },
    /// The request was answered without effect (failed `test`, failed
    /// precondition, or 404 of a DELETE/PATCH at the exact name).
    NotApplied {
        http_status: u16,
    },
    Uncertain {
        http_status: Option<u16>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KubernetesSessionEffectResultV2 {
    pub version: u32,
    pub operation_id: String,
    pub reservation_id: String,
    pub step: u32,
    pub claim_operation_id: String,
    pub request_sha256: String,
    pub outcome: KubernetesSessionEffectOutcomeV2,
    pub observed_at: String,
}
impl KubernetesSessionEffectResultV2 {
    pub fn validate(&self) -> anyhow::Result<()> {
        require(
            self.version == 1
                && (1..=MAX_EFFECT_STEPS).contains(&self.step)
                && self.operation_id != self.claim_operation_id
                && exact_utc_seconds(&self.observed_at),
        )?;
        uuid(&self.operation_id)?;
        uuid(&self.reservation_id)?;
        uuid(&self.claim_operation_id)?;
        hash(&self.request_sha256)?;
        match &self.outcome {
            KubernetesSessionEffectOutcomeV2::Applied {
                object_uid,
                resource_version,
            } => {
                uuid(object_uid)?;
                require(valid_resource_version(resource_version))
            }
            KubernetesSessionEffectOutcomeV2::NotApplied { http_status } => {
                require(matches!(http_status, 404 | 409 | 422))
            }
            KubernetesSessionEffectOutcomeV2::Uncertain { http_status } => {
                require(http_status.is_none_or(|status| (100..=599).contains(&status)))
            }
        }
    }
    pub fn input_hash(&self) -> anyhow::Result<String> {
        self.validate()?;
        digest(b"ai-guard:kubernetes-session-effect-result:v1\0", self)
    }
}

/// Runtime-observed end of one Pod, read through the API by its exact UID.
/// The worker's own declared exit stays a separate, declared fact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum KubernetesSessionPodEndV2 {
    /// The `worker` container was observed terminated.
    Terminated {
        container_id: String,
        exit_code: Option<i32>,
        oom_killed: Option<bool>,
        started_at: Option<String>,
        finished_at: Option<String>,
    },
    /// The Pod was deleted while its scheduling gate was still present: it
    /// was never scheduled, so no container ever started.
    NeverStarted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KubernetesSessionTerminalV2 {
    pub version: u32,
    pub operation_id: String,
    pub reservation_id: String,
    pub subject: KubernetesSessionEffectSubjectV2,
    pub pod_uid: String,
    pub end: KubernetesSessionPodEndV2,
    pub observed_at: String,
    /// Exit code declared by the worker bootstrap, if any: `declared`, never
    /// a substitute for the observed terminal.
    pub declared_exit_code: Option<i32>,
}
impl KubernetesSessionTerminalV2 {
    pub fn validate(&self) -> anyhow::Result<()> {
        require(
            self.version == 1
                && self.subject != KubernetesSessionEffectSubjectV2::WorkspaceClaim
                && exact_utc_seconds(&self.observed_at),
        )?;
        uuid(&self.operation_id)?;
        uuid(&self.reservation_id)?;
        uuid(&self.pod_uid)?;
        if let KubernetesSessionPodEndV2::Terminated {
            container_id,
            exit_code,
            started_at,
            finished_at,
            ..
        } = &self.end
        {
            require(
                !container_id.is_empty()
                    && container_id.len() <= 256
                    && container_id.is_ascii()
                    && exit_code.is_none_or(|code| code >= 0)
                    && started_at.as_deref().is_none_or(exact_utc_seconds)
                    && finished_at.as_deref().is_none_or(exact_utc_seconds),
            )?;
        }
        Ok(())
    }
    pub fn input_hash(&self) -> anyhow::Result<String> {
        self.validate()?;
        digest(b"ai-guard:kubernetes-session-terminal:v1\0", self)
    }
}

/// Release of the attempt's quota once every created Pod has an observed
/// end and no claimed CREATE remains uncertain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KubernetesSessionSettlementV2 {
    pub version: u32,
    pub operation_id: String,
    pub reservation_id: String,
    pub reservation_input_hash: String,
    /// The last step known to the broker; a newer step is a conflict.
    pub expected_last_step: u32,
}
impl KubernetesSessionSettlementV2 {
    pub fn validate(&self) -> anyhow::Result<()> {
        require(
            self.version == 1
                && (1..=MAX_EFFECT_STEPS).contains(&self.expected_last_step)
                && self.operation_id != self.reservation_id,
        )?;
        uuid(&self.operation_id)?;
        uuid(&self.reservation_id)?;
        hash(&self.reservation_input_hash)
    }
    pub fn input_hash(&self) -> anyhow::Result<String> {
        self.validate()?;
        digest(b"ai-guard:kubernetes-session-settlement:v1\0", self)
    }
}

/// Tenant storage policy for broker-created workspace claims (R5). Server
/// configuration only; a request outside it is refused before any CREATE.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KubernetesSessionStoragePolicyV2 {
    pub version: u32,
    pub allowed_storage_classes: Vec<String>,
    pub max_capacity_mib: u32,
}
impl KubernetesSessionStoragePolicyV2 {
    pub fn validate(&self) -> anyhow::Result<()> {
        require(
            self.version == 1
                && !self.allowed_storage_classes.is_empty()
                && self.allowed_storage_classes.len() <= 8
                && (1..=crate::runtime::kubernetes::session_claim::MAX_CAPACITY_MIB)
                    .contains(&self.max_capacity_mib),
        )?;
        let mut seen = std::collections::BTreeSet::new();
        for class in &self.allowed_storage_classes {
            KubernetesSessionStorageRequestV2 {
                version: 1,
                storage_class_name: class.clone(),
                capacity_mib: 1,
            }
            .validate()
            .map_err(|_| super::KubernetesSessionReservationFailureV2::InvalidRequest)?;
            require(seen.insert(class))?;
        }
        Ok(())
    }
    pub fn approves(&self, storage: &KubernetesSessionStorageRequestV2) -> anyhow::Result<bool> {
        self.validate()?;
        Ok(storage.validate().is_ok()
            && storage.capacity_mib <= self.max_capacity_mib
            && self
                .allowed_storage_classes
                .contains(&storage.storage_class_name))
    }
}

/// Central position of one reservation's effects. `settled` releases the
/// quota held by the reservation row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KubernetesSessionEffectStateV2 {
    pub version: u32,
    pub reservation_id: String,
    pub last_step: u32,
    pub workspace_claim_uid: Option<String>,
    pub session_pod_uid: Option<String>,
    pub export_pod_uid: Option<String>,
    pub session_terminal: bool,
    pub export_terminal: bool,
    pub unresolved_uncertain_steps: u32,
    pub settled: bool,
}

fn exact_utc_seconds(value: &str) -> bool {
    chrono::DateTime::parse_from_rfc3339(value).is_ok_and(|time| {
        time.offset().local_minus_utc() == 0
            && time.format("%Y-%m-%dT%H:%M:%SZ").to_string() == value
    })
}
fn valid_resource_version(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".-_".contains(&byte))
}

#[cfg(test)]
#[path = "effects_tests.rs"]
mod tests;
