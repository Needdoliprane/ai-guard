//! Shared contract of the proposed owned broker-supervision profile.
//!
//! **Proposal submitted to Root, not an admitted interface.** The profile name
//! [`OWNED_SUPERVISION_PROFILE_V1`], its closed operation set, routes, bounds,
//! commitments, Pending/ACK rules and every type below are frozen between the
//! control plane (federation lot A) and the broker publisher (lot B) until Root
//! admits or revises them. Nothing here authenticates, authorizes, opens a
//! Store, reads a file or performs I/O. Deserializing a value grants no
//! permission and reconstructs no capability: the private broker proof, the
//! retained source, its epochs and the checked transaction remain with the
//! control plane. A heartbeat stays a declaration whose reception is observed;
//! a checkpoint stays a `DeclaredPartial` list of declared heads. Neither is a
//! runtime inventory, a terminal observation, a quota release or a usage fact.
//!
//! Closed profile/format matrix, never a numeric range:
//!
//! | profile | Store format | operations | access |
//! |---|---|---|---|
//! | legacy presence | 6–13 heartbeat, 12–13 checkpoint | legacy routes | unscoped legacy Store |
//! | `broker_supervision_owned_v1` | exactly 14 or 15, equal to the retained source descriptor and its verified marker | the five [`OwnedBrokerOperationV1`] | scoped `HumanSourceAccess` only |
//!
//! No format is served by both profiles and no format grants a permission. A
//! future format is refused by [`OwnedSourceFormatV1::from_verified`] until a
//! reviewed variant is added. A new field, operation or format requires a new
//! profile version and a new enrollment handoff, never an implicit upgrade of
//! a v1 journal, an adoption of a legacy journal or of a remote head.
//!
//! Commitments use `crate::control::content_hash`: SHA-256 over a domain
//! string, one zero byte and the `serde_json` bytes of a typed struct, fields
//! in declaration order (no map, no float). Domains are distinct per purpose.
//! The two registry mirrors below must keep the exact serialization of the
//! private `audit::broker_registry::Change`; the control plane recomputes the
//! private hash for every owned operation and refuses any divergence.
//!
//! Journal pins and engaged identity:
//! - Pinned for a journal's lifetime ([`OwnedSupervisionPinsV1`]): profile,
//!   gateway origin, installation, closure ID, source, Store format, tenant,
//!   namespace, broker, incarnation, credential, grant revision and authority.
//!   Any change requires a new enrollment handoff and a new private journal;
//!   the old journal and its Pending bytes are kept untouched.
//! - Engaged per operation ([`OwnedBrokerSubjectV1`]): the pins plus the
//!   binding revision. A Pending is replayed only under an identical subject.
//!   New operations accept a renewed (higher) binding revision.
//! - Observed only: the closure revision. It may advance between operations
//!   and during a Pending; a lower value than already seen is a regression
//!   (for example an older control backup) and blocks explicitly.
use super::*;
use crate::control::content_hash;
use serde_json::Value;

pub const OWNED_SUPERVISION_PROFILE_V1: &str = "broker_supervision_owned_v1";
pub const OWNED_SUPERVISION_VERSION_V1: u32 = 1;

const SUBJECT_DOMAIN: &str = "ai-guard:broker-supervision-owned:v1:subject";
const OPERATION_DOMAIN: &str = "ai-guard:broker-supervision-owned:v1:operation";
const ENROLLMENT_DOMAIN: &str = "ai-guard:broker-supervision-owned:v1:enrollment";
/// Equal to the private `Change::input_hash` domains of `audit::broker_registry`.
const REGISTRY_INPUT_DOMAIN: &str = "ai-guard:broker-registry-input:v1";
const REGISTRY_CHECKPOINT_DOMAIN: &str = "ai-guard:broker-checkpoint-input:v1";

pub const MAX_OWNED_IDENTITY_BYTES: usize = 4 * 1024;
pub const MAX_OWNED_SELF_VIEW_BYTES: usize = 16 * 1024;
pub const MAX_OWNED_HEARTBEAT_REQUEST_BYTES: usize = 4 * 1024;
pub const MAX_OWNED_HEARTBEAT_RESULT_BYTES: usize = 16 * 1024;
pub const MAX_OWNED_HEARTBEAT_PENDING_BYTES: usize = 16 * 1024;
pub const MAX_OWNED_CHECKPOINT_REQUEST_BYTES: usize = MAX_BROKER_CHECKPOINT_BYTES + 4 * 1024;
pub const MAX_OWNED_CHECKPOINT_RESULT_BYTES: usize = MAX_BROKER_CHECKPOINT_BYTES + 8 * 1024;
pub const MAX_OWNED_CHECKPOINT_SELF_VIEW_BYTES: usize = MAX_BROKER_CHECKPOINT_BYTES + 8 * 1024;
pub const MAX_OWNED_CHECKPOINT_PENDING_BYTES: usize = MAX_BROKER_CHECKPOINT_BYTES + 8 * 1024;
pub const MAX_OWNED_ENROLLMENT_BYTES: usize = 8 * 1024;
pub const MAX_OWNED_ERROR_BYTES: usize = 4 * 1024;

/// Result of the pure owned-supervision validators; the error is a closed code.
pub type OwnedChecked<T> = std::result::Result<T, OwnedSupervisionErrorCodeV1>;

/// Closed stable error codes of the owned profile. The wire code, HTTP status
/// and retry class are fixed together; a body naming a known code with
/// another status is unrecognized, never reinterpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OwnedSupervisionErrorCodeV1 {
    AuthenticationRequired,
    Forbidden,
    InvalidRequest,
    ProtocolMismatch,
    IdentityMismatch,
    OperationConflict,
    SequenceConflict,
    ClockUncertain,
    MigrationRequired,
    SourceRebuildRequired,
    SupervisionUnavailable,
    AuditUnavailable,
    ResponseWithheld,
    /// Deterministic server-side refusal to publish a positive DTO (for
    /// example a redaction collision or a server DTO invariant). The outcome
    /// of a mutating operation may be durable; retrying cannot change it.
    ResponseUnpublishable,
    /// The presented digest is claimed by several credential classes or
    /// sources; the operator must resolve the collision, then restart.
    CredentialAmbiguous,
}

impl OwnedSupervisionErrorCodeV1 {
    pub const ALL: [Self; 15] = [
        Self::AuthenticationRequired,
        Self::Forbidden,
        Self::InvalidRequest,
        Self::ProtocolMismatch,
        Self::IdentityMismatch,
        Self::OperationConflict,
        Self::SequenceConflict,
        Self::ClockUncertain,
        Self::MigrationRequired,
        Self::SourceRebuildRequired,
        Self::SupervisionUnavailable,
        Self::AuditUnavailable,
        Self::ResponseWithheld,
        Self::ResponseUnpublishable,
        Self::CredentialAmbiguous,
    ];

    pub const fn code(self) -> &'static str {
        match self {
            Self::AuthenticationRequired => "owned_broker_authentication_required",
            Self::Forbidden => "owned_broker_forbidden",
            Self::InvalidRequest => "owned_invalid_request",
            Self::ProtocolMismatch => "owned_protocol_mismatch",
            Self::IdentityMismatch => "owned_identity_mismatch",
            Self::OperationConflict => "owned_operation_conflict",
            Self::SequenceConflict => "owned_sequence_conflict",
            Self::ClockUncertain => "owned_clock_uncertain",
            Self::MigrationRequired => "owned_migration_required",
            Self::SourceRebuildRequired => "owned_source_rebuild_required",
            Self::SupervisionUnavailable => "owned_supervision_unavailable",
            Self::AuditUnavailable => "owned_audit_unavailable",
            Self::ResponseWithheld => "owned_response_withheld",
            Self::ResponseUnpublishable => "owned_response_unpublishable",
            Self::CredentialAmbiguous => "owned_credential_ambiguous",
        }
    }

    pub const fn status(self) -> u16 {
        match self {
            Self::AuthenticationRequired => 401,
            Self::Forbidden => 403,
            Self::InvalidRequest | Self::ProtocolMismatch => 400,
            Self::IdentityMismatch
            | Self::OperationConflict
            | Self::SequenceConflict
            | Self::ClockUncertain => 409,
            Self::MigrationRequired
            | Self::SourceRebuildRequired
            | Self::SupervisionUnavailable
            | Self::AuditUnavailable
            | Self::ResponseWithheld
            | Self::ResponseUnpublishable
            | Self::CredentialAmbiguous => 503,
        }
    }

    pub const fn retry(self) -> OwnedRetryClassV1 {
        match self {
            Self::SupervisionUnavailable => OwnedRetryClassV1::RetrySameOperation,
            Self::ResponseWithheld => OwnedRetryClassV1::ReconcileSameOperation,
            Self::SourceRebuildRequired
            | Self::AuditUnavailable
            | Self::ResponseUnpublishable
            | Self::CredentialAmbiguous => OwnedRetryClassV1::WaitForOperator,
            Self::AuthenticationRequired
            | Self::Forbidden
            | Self::InvalidRequest
            | Self::ProtocolMismatch
            | Self::IdentityMismatch
            | Self::OperationConflict
            | Self::SequenceConflict
            | Self::ClockUncertain
            | Self::MigrationRequired => OwnedRetryClassV1::Stop,
        }
    }

    /// Fixed operator prose. It never contains a request value or a secret.
    pub const fn message(self) -> &'static str {
        match self {
            Self::AuthenticationRequired => {
                "A current owned broker supervision credential is required."
            }
            Self::Forbidden => {
                "The broker grant or binding is revoked, expired or outside its enrolled scope."
            }
            Self::InvalidRequest => "The owned broker supervision request is invalid.",
            Self::ProtocolMismatch => {
                "The request does not match the owned supervision profile or its commitments."
            }
            Self::IdentityMismatch => {
                "The engaged broker identity differs from the current authenticated identity."
            }
            Self::OperationConflict => {
                "This operation identity was already recorded with different content."
            }
            Self::SequenceConflict => {
                "The operation sequence or revision differs from the durable broker state."
            }
            Self::ClockUncertain => {
                "The control clock precedes the durable broker state; reconcile the clock."
            }
            Self::MigrationRequired => {
                "The tenant source format is not admitted by the owned supervision profile."
            }
            Self::SourceRebuildRequired => {
                "The authenticated source index must be rebuilt by restarting the control plane."
            }
            Self::SupervisionUnavailable => {
                "Owned broker supervision is temporarily unavailable; retry the same operation."
            }
            Self::AuditUnavailable => {
                "The mandatory audit is unavailable; the operation was not accepted."
            }
            Self::ResponseWithheld => {
                "The outcome cannot be published; reconcile the same operation by self-read."
            }
            Self::ResponseUnpublishable => {
                "The control plane cannot publish a valid response; the operator must inspect it."
            }
            Self::CredentialAmbiguous => {
                "The credential is claimed by several classes or sources; resolve, then restart."
            }
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|candidate| candidate.code() == code)
    }

    /// Closed error envelope emitted by the control plane.
    pub fn body(self) -> Value {
        serde_json::json!({"error": {
            "code": self.code(),
            "message": self.message(),
            "retry_action": self.retry().retry_action(),
            "profile": OWNED_SUPERVISION_PROFILE_V1,
        }})
    }
}

impl std::fmt::Display for OwnedSupervisionErrorCodeV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}
impl std::error::Error for OwnedSupervisionErrorCodeV1 {}

/// What a publisher may do after a refusal; it never selects another profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnedRetryClassV1 {
    /// Recognized temporary refusal without mutation: retry the same retained
    /// operation after bounded backoff.
    RetrySameOperation,
    /// Outcome unknown: retry the same operation, then reconcile it by an exact
    /// self-read. A new operation is never allocated for it.
    ReconcileSameOperation,
    /// Control-plane operator action is required: keep the Pending, retry only
    /// at the slow cadence and report the named state.
    WaitForOperator,
    /// Never retried automatically; stop with the Pending retained.
    Stop,
}
impl OwnedRetryClassV1 {
    pub const fn retry_action(self) -> &'static str {
        match self {
            Self::RetrySameOperation => "retry_same_operation",
            Self::ReconcileSameOperation => "reconcile_same_operation",
            Self::WaitForOperator => "wait_for_operator",
            Self::Stop => "contact_operator",
        }
    }
}

/// Publisher-side interpretation of a non-200 response. Only codes paired
/// with their exact status are recognized; any other named or unnamed body is
/// `Unrecognized` and stops the publisher, including an unknown named 503.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnedRemoteFailureV1 {
    Owned(OwnedSupervisionErrorCodeV1),
    /// Valid relay `control_transport_failure` envelope: outcome unknown.
    TransportOutcomeUnknown,
    /// The transport gate refused before dispatch for occupied capacity.
    GatewayCapacityBusy,
    /// The transport gate refused an incomplete or slow body before dispatch.
    GatewayNotDispatched,
    /// The transport gate refused the target, method or size before dispatch.
    GatewayRefused,
    /// The owned human-source coverage is closing or requires a rebuild.
    CoverageUnavailable,
    /// The cumulative audit budget is exhausted: operator action only.
    AuditBudgetExhausted,
    Unrecognized,
}
impl OwnedRemoteFailureV1 {
    pub const fn retry(self) -> OwnedRetryClassV1 {
        match self {
            Self::Owned(code) => code.retry(),
            Self::TransportOutcomeUnknown => OwnedRetryClassV1::ReconcileSameOperation,
            Self::GatewayCapacityBusy | Self::GatewayNotDispatched => {
                OwnedRetryClassV1::RetrySameOperation
            }
            Self::CoverageUnavailable => OwnedRetryClassV1::WaitForOperator,
            Self::GatewayRefused | Self::AuditBudgetExhausted | Self::Unrecognized => {
                OwnedRetryClassV1::Stop
            }
        }
    }
}

/// Classify a bounded, already parsed error body. The caller must not persist,
/// log or display the remote prose; only the returned closed value is kept.
pub fn classify_remote_failure(status: u16, body: &Value) -> OwnedRemoteFailureV1 {
    use crate::control_transport::{
        ControlTransportFailureReasonV1 as Reason, ControlTransportFailureV1,
    };
    let Some(error) = body.get("error") else {
        return OwnedRemoteFailureV1::Unrecognized;
    };
    let code = error.get("code").and_then(Value::as_str);
    if let Some(owned) = code.and_then(OwnedSupervisionErrorCodeV1::from_code) {
        return if owned.status() == status {
            OwnedRemoteFailureV1::Owned(owned)
        } else {
            OwnedRemoteFailureV1::Unrecognized
        };
    }
    let not_dispatched = error.get("outcome").and_then(Value::as_str) == Some("not_dispatched");
    match (status, code) {
        (502 | 504, Some("control_transport_failure")) => {
            match serde_json::from_value::<ControlTransportFailureV1>(error.clone()) {
                Ok(failure)
                    if failure.validate()
                        && matches!(
                            (status, failure.reason),
                            (
                                502,
                                Reason::UpstreamConnectionUnavailable
                                    | Reason::UpstreamConnectionInterrupted
                            ) | (504, Reason::UpstreamTimeout)
                        ) =>
                {
                    OwnedRemoteFailureV1::TransportOutcomeUnknown
                }
                _ => OwnedRemoteFailureV1::Unrecognized,
            }
        }
        (429, Some("control_operation_capacity_exhausted")) if not_dispatched => {
            OwnedRemoteFailureV1::GatewayCapacityBusy
        }
        (408, Some("control_body_timeout")) | (400, Some("control_body_incomplete"))
            if not_dispatched =>
        {
            OwnedRemoteFailureV1::GatewayNotDispatched
        }
        (413, Some("control_body_too_large"))
        | (400, Some("invalid_control_target"))
        | (404, Some("unknown_control_route"))
        | (405, Some("control_method_not_allowed"))
            if not_dispatched =>
        {
            OwnedRemoteFailureV1::GatewayRefused
        }
        (503, Some("identity_unavailable")) => OwnedRemoteFailureV1::CoverageUnavailable,
        (
            429,
            Some(
                "audit_read_budget_exhausted"
                | "audit_write_budget_exhausted"
                | "audit_workload_budget_exhausted",
            ),
        ) => OwnedRemoteFailureV1::AuditBudgetExhausted,
        _ => OwnedRemoteFailureV1::Unrecognized,
    }
}

/// Closed operation set of the owned profile. The operation is chosen by the
/// server route dispatch, never by a request field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnedBrokerOperationV1 {
    Identity,
    SelfView,
    Heartbeat,
    Checkpoint,
    CheckpointSelf,
}
impl OwnedBrokerOperationV1 {
    pub const ALL: [Self; 5] = [
        Self::Identity,
        Self::SelfView,
        Self::Heartbeat,
        Self::Checkpoint,
        Self::CheckpointSelf,
    ];

    pub const fn method(self) -> &'static str {
        match self {
            Self::Heartbeat | Self::Checkpoint => "POST",
            Self::Identity | Self::SelfView | Self::CheckpointSelf => "GET",
        }
    }

    /// Gateway-root path, after the control relay removed exactly `/control`.
    pub const fn path(self) -> &'static str {
        match self {
            Self::Identity => "/api/v1/owned/brokers/identity",
            Self::SelfView => "/api/v1/owned/brokers/self",
            Self::Heartbeat => "/api/v1/owned/brokers/heartbeats",
            Self::Checkpoint => "/api/v1/owned/brokers/checkpoints",
            Self::CheckpointSelf => "/api/v1/owned/brokers/checkpoints/self",
        }
    }

    /// Path inside the `/api/v1` router nest.
    pub const fn router_path(self) -> &'static str {
        match self {
            Self::Identity => "/owned/brokers/identity",
            Self::SelfView => "/owned/brokers/self",
            Self::Heartbeat => "/owned/brokers/heartbeats",
            Self::Checkpoint => "/owned/brokers/checkpoints",
            Self::CheckpointSelf => "/owned/brokers/checkpoints/self",
        }
    }

    pub const fn mutating(self) -> bool {
        matches!(self, Self::Heartbeat | Self::Checkpoint)
    }

    /// Maximum request body; zero means a bodyless GET.
    pub const fn request_limit(self) -> usize {
        match self {
            Self::Heartbeat => MAX_OWNED_HEARTBEAT_REQUEST_BYTES,
            Self::Checkpoint => MAX_OWNED_CHECKPOINT_REQUEST_BYTES,
            Self::Identity | Self::SelfView | Self::CheckpointSelf => 0,
        }
    }

    pub const fn response_limit(self) -> usize {
        match self {
            Self::Identity => MAX_OWNED_IDENTITY_BYTES,
            Self::SelfView => MAX_OWNED_SELF_VIEW_BYTES,
            Self::Heartbeat => MAX_OWNED_HEARTBEAT_RESULT_BYTES,
            Self::Checkpoint => MAX_OWNED_CHECKPOINT_RESULT_BYTES,
            Self::CheckpointSelf => MAX_OWNED_CHECKPOINT_SELF_VIEW_BYTES,
        }
    }

    /// Exact byte comparison; no decoding, query, HEAD alias or prefix match.
    pub fn classify(method: &str, path: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|operation| operation.method() == method && operation.path() == path)
    }
}

/// Store formats admitted by the owned profile. The value must come from the
/// real scoped transaction after source and marker revalidation, never from a
/// client DTO.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnedSourceFormatV1 {
    Format14,
    Format15,
}
impl OwnedSourceFormatV1 {
    pub const fn from_verified(format: u32) -> Option<Self> {
        match format {
            14 => Some(Self::Format14),
            15 => Some(Self::Format15),
            _ => None,
        }
    }
    pub const fn number(self) -> u32 {
        match self {
            Self::Format14 => 14,
            Self::Format15 => 15,
        }
    }
}

fn check(ok: bool, code: OwnedSupervisionErrorCodeV1) -> OwnedChecked<()> {
    if ok { Ok(()) } else { Err(code) }
}
fn is_hex64(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
fn uuid_field(value: &str) -> OwnedChecked<()> {
    registry_uuid(value).map_err(|_| OwnedSupervisionErrorCodeV1::InvalidRequest)
}
fn revision_field(value: u64) -> OwnedChecked<()> {
    revision(value).map_err(|_| OwnedSupervisionErrorCodeV1::InvalidRequest)
}
/// Canonical control time: parses and formats back to the same bytes.
fn canonical_time(value: &str) -> OwnedChecked<DateTime<Utc>> {
    let time = parse_time(value).map_err(|_| OwnedSupervisionErrorCodeV1::InvalidRequest)?;
    check(
        format_time(time) == value,
        OwnedSupervisionErrorCodeV1::InvalidRequest,
    )?;
    Ok(time)
}
fn require_profile(profile: &str, version: u32) -> OwnedChecked<()> {
    check(
        profile == OWNED_SUPERVISION_PROFILE_V1 && version == OWNED_SUPERVISION_VERSION_V1,
        OwnedSupervisionErrorCodeV1::ProtocolMismatch,
    )
}
fn bounded<T: Serialize>(
    value: &T,
    limit: usize,
    code: OwnedSupervisionErrorCodeV1,
) -> OwnedChecked<()> {
    let bytes = serde_json::to_vec(value).map_err(|_| code)?;
    check(bytes.len() <= limit, code)
}
fn hash<T: Serialize>(domain: &str, value: &T) -> OwnedChecked<String> {
    content_hash(domain, value).map_err(|_| OwnedSupervisionErrorCodeV1::InvalidRequest)
}
fn as_protocol<T>(value: OwnedChecked<T>) -> OwnedChecked<T> {
    value.map_err(|_| OwnedSupervisionErrorCodeV1::ProtocolMismatch)
}

/// Canonical gateway origin, with the presence publisher's transport rules:
/// HTTPS, or HTTP only to a loopback IP literal; no user, path, query or
/// fragment. Returns the origin without its trailing slash.
pub fn canonical_gateway_origin(value: &str) -> OwnedChecked<String> {
    let invalid = OwnedSupervisionErrorCodeV1::InvalidRequest;
    check(value.len() <= 2048, invalid)?;
    let url = reqwest::Url::parse(value).map_err(|_| invalid)?;
    let loopback = url.host_str().is_some_and(|host| {
        host.trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
    });
    check(
        url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none()
            && (url.scheme() == "https" || url.scheme() == "http" && loopback),
        invalid,
    )?;
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

/// Engaged broker identity of one owned operation. Every field comes from the
/// control plane's authenticated proof and retained source. A publisher sends
/// it only for exact comparison; it never selects a tenant, Store or scope.
/// No pathname, bearer, digest or secret belongs here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnedBrokerSubjectV1 {
    pub installation_id: String,
    pub closure_id: String,
    pub source_id: String,
    pub source_format: u32,
    pub tenant_id: String,
    pub control_namespace_uuid: String,
    pub broker_id: String,
    pub incarnation_id: String,
    pub credential_id: String,
    pub binding_revision: u64,
    pub grant_revision: u64,
}
impl OwnedBrokerSubjectV1 {
    pub fn validate(&self) -> OwnedChecked<()> {
        for id in [
            &self.installation_id,
            &self.closure_id,
            &self.source_id,
            &self.control_namespace_uuid,
            &self.broker_id,
            &self.incarnation_id,
            &self.credential_id,
        ] {
            uuid_field(id)?;
        }
        crate::tenancy::validate_tenant_id(&self.tenant_id)
            .map_err(|_| OwnedSupervisionErrorCodeV1::InvalidRequest)?;
        check(
            OwnedSourceFormatV1::from_verified(self.source_format).is_some(),
            OwnedSupervisionErrorCodeV1::MigrationRequired,
        )?;
        check(
            self.binding_revision > 0 && self.grant_revision > 0,
            OwnedSupervisionErrorCodeV1::InvalidRequest,
        )?;
        revision_field(self.binding_revision)?;
        revision_field(self.grant_revision)
    }

    pub fn format(&self) -> OwnedChecked<OwnedSourceFormatV1> {
        OwnedSourceFormatV1::from_verified(self.source_format)
            .ok_or(OwnedSupervisionErrorCodeV1::MigrationRequired)
    }

    pub fn commitment(&self) -> OwnedChecked<String> {
        self.validate()?;
        hash(SUBJECT_DOMAIN, self)
    }

    fn registry_evidence(&self) -> RegistryIdentityEvidence<'_> {
        RegistryIdentityEvidence {
            tenant_id: &self.tenant_id,
            broker_id: &self.broker_id,
            credential_id: &self.credential_id,
            binding_revision: self.binding_revision,
            grant_revision: self.grant_revision,
        }
    }
}

/// Instantaneous authenticated identity view. It is evidence of a server
/// observation, not a proof transferable to another operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnedBrokerIdentityViewV1 {
    pub profile: String,
    pub version: u32,
    pub closure_revision: u64,
    pub subject: OwnedBrokerSubjectV1,
}
impl OwnedBrokerIdentityViewV1 {
    pub fn new(closure_revision: u64, subject: OwnedBrokerSubjectV1) -> OwnedChecked<Self> {
        let view = Self {
            profile: OWNED_SUPERVISION_PROFILE_V1.into(),
            version: OWNED_SUPERVISION_VERSION_V1,
            closure_revision,
            subject,
        };
        view.validate()?;
        Ok(view)
    }
    pub fn validate(&self) -> OwnedChecked<()> {
        require_profile(&self.profile, self.version)?;
        check(
            self.closure_revision > 0,
            OwnedSupervisionErrorCodeV1::InvalidRequest,
        )?;
        revision_field(self.closure_revision)?;
        self.subject.validate()?;
        bounded(
            self,
            MAX_OWNED_IDENTITY_BYTES,
            OwnedSupervisionErrorCodeV1::InvalidRequest,
        )
    }
}

#[derive(Serialize)]
struct RegistryIdentityEvidence<'a> {
    tenant_id: &'a str,
    broker_id: &'a str,
    credential_id: &'a str,
    binding_revision: u64,
    grant_revision: u64,
}
/// Mirror of the private `audit::broker_registry::Change` serialization.
#[derive(Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum RegistryChange<'a> {
    Heartbeat {
        identity: RegistryIdentityEvidence<'a>,
        heartbeat: &'a BrokerHeartbeatV1,
    },
    Checkpoint {
        identity: RegistryIdentityEvidence<'a>,
        checkpoint: &'a BrokerCheckpointV1,
    },
}
#[derive(Serialize)]
struct OperationCommitment<'a> {
    profile: &'a str,
    version: u32,
    operation: OwnedBrokerOperationV1,
    subject_commitment: &'a str,
    operation_id: &'a str,
    registry_input_hash: &'a str,
}

/// The registry input hash recorded by the durable audit and the owned
/// operation commitment binding it to the installation, closure, source and
/// format. Both are deterministic; neither authorizes the operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedOperationCommitmentsV1 {
    pub registry_input_hash: String,
    pub operation_commitment: String,
}

fn operation_commitments(
    operation: OwnedBrokerOperationV1,
    subject: &OwnedBrokerSubjectV1,
    operation_id: &str,
    registry_input_hash: String,
) -> OwnedChecked<OwnedOperationCommitmentsV1> {
    let subject_commitment = subject.commitment()?;
    let operation_commitment = hash(
        OPERATION_DOMAIN,
        &OperationCommitment {
            profile: OWNED_SUPERVISION_PROFILE_V1,
            version: OWNED_SUPERVISION_VERSION_V1,
            operation,
            subject_commitment: &subject_commitment,
            operation_id,
            registry_input_hash: &registry_input_hash,
        },
    )?;
    Ok(OwnedOperationCommitmentsV1 {
        registry_input_hash,
        operation_commitment,
    })
}

pub fn heartbeat_commitments(
    subject: &OwnedBrokerSubjectV1,
    heartbeat: &BrokerHeartbeatV1,
) -> OwnedChecked<OwnedOperationCommitmentsV1> {
    subject.validate()?;
    heartbeat
        .validate()
        .map_err(|_| OwnedSupervisionErrorCodeV1::InvalidRequest)?;
    check(
        heartbeat.incarnation_id == subject.incarnation_id,
        OwnedSupervisionErrorCodeV1::InvalidRequest,
    )?;
    let registry = hash(
        REGISTRY_INPUT_DOMAIN,
        &RegistryChange::Heartbeat {
            identity: subject.registry_evidence(),
            heartbeat,
        },
    )?;
    operation_commitments(
        OwnedBrokerOperationV1::Heartbeat,
        subject,
        &heartbeat.operation_id,
        registry,
    )
}

pub fn checkpoint_commitments(
    subject: &OwnedBrokerSubjectV1,
    checkpoint: &BrokerCheckpointV1,
) -> OwnedChecked<OwnedOperationCommitmentsV1> {
    subject.validate()?;
    checkpoint
        .validate()
        .map_err(|_| OwnedSupervisionErrorCodeV1::InvalidRequest)?;
    check(
        checkpoint.incarnation_id == subject.incarnation_id,
        OwnedSupervisionErrorCodeV1::InvalidRequest,
    )?;
    let registry = hash(
        REGISTRY_CHECKPOINT_DOMAIN,
        &RegistryChange::Checkpoint {
            identity: subject.registry_evidence(),
            checkpoint,
        },
    )?;
    operation_commitments(
        OwnedBrokerOperationV1::Checkpoint,
        subject,
        &checkpoint.operation_id,
        registry,
    )
}

/// Closed reason for which an engaged identity cannot be used. Each reason
/// keeps the Pending bytes; none authorizes a rewrite of identity or hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnedIncompatibilityV1 {
    Profile,
    GatewayOrigin,
    Installation,
    Closure,
    ClosureRegression,
    Source,
    Format,
    Tenant,
    Namespace,
    Broker,
    Incarnation,
    Credential,
    BindingRevision,
    BindingRegression,
    GrantRevision,
    Authority,
}
impl OwnedIncompatibilityV1 {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Profile => "profile",
            Self::GatewayOrigin => "gateway_origin",
            Self::Installation => "installation",
            Self::Closure => "closure",
            Self::ClosureRegression => "closure_regression",
            Self::Source => "source",
            Self::Format => "format",
            Self::Tenant => "tenant",
            Self::Namespace => "namespace",
            Self::Broker => "broker",
            Self::Incarnation => "incarnation",
            Self::Credential => "credential",
            Self::BindingRevision => "binding_revision",
            Self::BindingRegression => "binding_regression",
            Self::GrantRevision => "grant_revision",
            Self::Authority => "authority",
        }
    }

    /// Fixed operator guidance; no request value or secret.
    pub const fn operator_action(self) -> &'static str {
        match self {
            Self::ClosureRegression | Self::BindingRegression => {
                "The control plane reports an older state than already acknowledged; \
                 reconcile the control backup before resuming this journal."
            }
            Self::BindingRevision => {
                "The binding was renewed while this operation was pending; the pending \
                 operation cannot be replayed under another engaged identity. Keep the \
                 journal and follow the blocked Pending procedure."
            }
            Self::Profile
            | Self::GatewayOrigin
            | Self::Installation
            | Self::Closure
            | Self::Source
            | Self::Format
            | Self::Tenant
            | Self::Namespace
            | Self::Broker
            | Self::Incarnation
            | Self::Credential
            | Self::GrantRevision
            | Self::Authority => {
                "The enrolled identity no longer matches; install a new enrollment handoff \
                 with a new private journal. The retained journal stays untouched."
            }
        }
    }
}

#[derive(Clone, Copy)]
enum BindingRule {
    Exact,
    AtLeast(u64),
}

fn compare_subject(
    expected: &OwnedBrokerSubjectV1,
    actual: &OwnedBrokerSubjectV1,
    binding: BindingRule,
) -> Result<(), OwnedIncompatibilityV1> {
    use OwnedIncompatibilityV1 as Reason;
    let checks = [
        (expected.installation_id == actual.installation_id, Reason::Installation),
        (expected.closure_id == actual.closure_id, Reason::Closure),
        (expected.source_id == actual.source_id, Reason::Source),
        (expected.source_format == actual.source_format, Reason::Format),
        (expected.tenant_id == actual.tenant_id, Reason::Tenant),
        (
            expected.control_namespace_uuid == actual.control_namespace_uuid,
            Reason::Namespace,
        ),
        (expected.broker_id == actual.broker_id, Reason::Broker),
        (expected.incarnation_id == actual.incarnation_id, Reason::Incarnation),
        (expected.credential_id == actual.credential_id, Reason::Credential),
        (expected.grant_revision == actual.grant_revision, Reason::GrantRevision),
    ];
    if let Some((_, reason)) = checks.into_iter().find(|(same, _)| !same) {
        return Err(reason);
    }
    match binding {
        BindingRule::Exact if actual.binding_revision != expected.binding_revision => {
            Err(Reason::BindingRevision)
        }
        BindingRule::AtLeast(floor) if actual.binding_revision < floor => {
            Err(Reason::BindingRegression)
        }
        BindingRule::Exact | BindingRule::AtLeast(_) => Ok(()),
    }
}

/// A retained Pending may be replayed only under an identical engaged subject.
/// The closure revision may advance but never regress.
pub fn check_replay_identity(
    pending: &OwnedBrokerIdentityViewV1,
    current: &OwnedBrokerIdentityViewV1,
) -> Result<(), OwnedIncompatibilityV1> {
    compare_subject(&pending.subject, &current.subject, BindingRule::Exact)?;
    if current.closure_revision < pending.closure_revision {
        return Err(OwnedIncompatibilityV1::ClosureRegression);
    }
    Ok(())
}

/// A new operation after an acknowledged one: same pins, binding revision and
/// closure revision non-decreasing.
pub fn check_progress_identity(
    acknowledged: &OwnedBrokerIdentityViewV1,
    current: &OwnedBrokerIdentityViewV1,
) -> Result<(), OwnedIncompatibilityV1> {
    compare_subject(
        &acknowledged.subject,
        &current.subject,
        BindingRule::AtLeast(acknowledged.subject.binding_revision),
    )?;
    if current.closure_revision < acknowledged.closure_revision {
        return Err(OwnedIncompatibilityV1::ClosureRegression);
    }
    Ok(())
}

/// Committed receipt of one enrollment step, copied from the durable audit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnedEnrollmentReceiptV1 {
    pub operation_id: String,
    pub audit_seq: i64,
    pub audit_hash: String,
}
impl OwnedEnrollmentReceiptV1 {
    pub fn validate(&self) -> OwnedChecked<()> {
        uuid_field(&self.operation_id)?;
        check(
            self.audit_seq > 0 && is_hex64(&self.audit_hash),
            OwnedSupervisionErrorCodeV1::InvalidRequest,
        )
    }
}

/// Public, non-secret part of the private enrollment handoff, written by the
/// offline identity maintenance from committed receipts only. The bearer is a
/// separate private file and never appears here, nor does its digest. This
/// value authenticates nothing: the gateway still authenticates the bearer in
/// the retained source and compares the engaged subject on every operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnedBrokerEnrollmentV1 {
    pub profile: String,
    pub version: u32,
    pub gateway_origin: String,
    pub closure_revision: u64,
    pub subject: OwnedBrokerSubjectV1,
    pub authority: BrokerAuthorityV1,
    pub lease_seconds: u32,
    pub grant_not_after: String,
    pub credential_expires_at: String,
    pub issued_at: String,
    pub grant_receipt: OwnedEnrollmentReceiptV1,
    pub binding_receipt: OwnedEnrollmentReceiptV1,
}
impl OwnedBrokerEnrollmentV1 {
    pub fn validate(&self) -> OwnedChecked<()> {
        let invalid = OwnedSupervisionErrorCodeV1::InvalidRequest;
        require_profile(&self.profile, self.version)?;
        check(
            canonical_gateway_origin(&self.gateway_origin)? == self.gateway_origin,
            invalid,
        )?;
        check(self.closure_revision > 0, invalid)?;
        revision_field(self.closure_revision)?;
        self.subject.validate()?;
        self.authority.validate().map_err(|_| invalid)?;
        check((1..=300).contains(&self.lease_seconds), invalid)?;
        let issued = canonical_time(&self.issued_at)?;
        check(
            issued < canonical_time(&self.grant_not_after)?
                && issued < canonical_time(&self.credential_expires_at)?,
            invalid,
        )?;
        self.grant_receipt.validate()?;
        self.binding_receipt.validate()?;
        check(
            self.grant_receipt.operation_id != self.binding_receipt.operation_id
                && self.grant_receipt.audit_seq != self.binding_receipt.audit_seq,
            invalid,
        )?;
        bounded(self, MAX_OWNED_ENROLLMENT_BYTES, invalid)
    }

    pub fn commitment(&self) -> OwnedChecked<String> {
        self.validate()?;
        hash(ENROLLMENT_DOMAIN, self)
    }
}

/// Journal pins derived only from a validated enrollment handoff. The
/// publisher stores them in its owned anchor; they never leave the broker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnedSupervisionPinsV1 {
    pub profile: String,
    pub version: u32,
    pub gateway_origin: String,
    pub enrollment_commitment: String,
    pub enrolled: OwnedBrokerSubjectV1,
    pub minimum_closure_revision: u64,
    pub authority: BrokerAuthorityV1,
}
impl OwnedSupervisionPinsV1 {
    pub fn from_enrollment(enrollment: &OwnedBrokerEnrollmentV1) -> OwnedChecked<Self> {
        let pins = Self {
            profile: OWNED_SUPERVISION_PROFILE_V1.into(),
            version: OWNED_SUPERVISION_VERSION_V1,
            gateway_origin: enrollment.gateway_origin.clone(),
            enrollment_commitment: enrollment.commitment()?,
            enrolled: enrollment.subject.clone(),
            minimum_closure_revision: enrollment.closure_revision,
            authority: enrollment.authority.clone(),
        };
        pins.validate()?;
        Ok(pins)
    }

    pub fn validate(&self) -> OwnedChecked<()> {
        let invalid = OwnedSupervisionErrorCodeV1::InvalidRequest;
        require_profile(&self.profile, self.version)?;
        check(
            canonical_gateway_origin(&self.gateway_origin)? == self.gateway_origin,
            invalid,
        )?;
        check(is_hex64(&self.enrollment_commitment), invalid)?;
        self.enrolled.validate()?;
        check(self.minimum_closure_revision > 0, invalid)?;
        revision_field(self.minimum_closure_revision)?;
        self.authority.validate().map_err(|_| invalid)?;
        bounded(self, MAX_OWNED_ENROLLMENT_BYTES, invalid)
    }

    pub fn check_identity(
        &self,
        identity: &OwnedBrokerIdentityViewV1,
    ) -> Result<(), OwnedIncompatibilityV1> {
        compare_subject(
            &self.enrolled,
            &identity.subject,
            BindingRule::AtLeast(self.enrolled.binding_revision),
        )?;
        if identity.closure_revision < self.minimum_closure_revision {
            return Err(OwnedIncompatibilityV1::ClosureRegression);
        }
        Ok(())
    }

    pub fn check_snapshot_authority(
        &self,
        snapshot: &BrokerCheckpointSnapshotV1,
    ) -> Result<(), OwnedIncompatibilityV1> {
        if snapshot.authority == self.authority {
            Ok(())
        } else {
            Err(OwnedIncompatibilityV1::Authority)
        }
    }
}

fn previous_heartbeat_sequence(broker: &BrokerV1, incarnation_id: &str) -> u64 {
    broker
        .last_heartbeat
        .as_ref()
        .filter(|heartbeat| heartbeat.incarnation_id == incarnation_id)
        .map_or(0, |heartbeat| heartbeat.sequence)
}

/// Internal coherence of a broker view with the engaged subject; the same
/// rules as the legacy handshake, without accepting another broker or grant.
fn validate_broker_view(subject: &OwnedBrokerSubjectV1, view: &BrokerViewV1) -> OwnedChecked<()> {
    let protocol = OwnedSupervisionErrorCodeV1::ProtocolMismatch;
    let broker = &view.broker;
    check(
        broker.version == 1
            && broker.broker_id == subject.broker_id
            && broker.credential_id == subject.credential_id
            && broker.incarnation_id == subject.incarnation_id
            && broker.grant_revision == subject.grant_revision
            && !view.runtime_inventory_complete,
        protocol,
    )?;
    revision(broker.heartbeat_revision).map_err(|_| protocol)?;
    for time in [&view.evaluated_at, &broker.granted_at, &broker.not_after] {
        parse_time(time).map_err(|_| protocol)?;
    }
    match &broker.last_heartbeat {
        Some(heartbeat) => {
            registry_uuid(&heartbeat.incarnation_id).map_err(|_| protocol)?;
            check(
                heartbeat.sequence > 0 && broker.heartbeat_revision > 0,
                protocol,
            )?;
            revision(heartbeat.sequence).map_err(|_| protocol)?;
            let received = parse_time(&heartbeat.received_at).map_err(|_| protocol)?;
            let expires = parse_time(&heartbeat.lease_expires_at).map_err(|_| protocol)?;
            check(expires >= received, protocol)?;
        }
        None => check(broker.heartbeat_revision == 0, protocol)?,
    }
    let expected = broker.view_at(&view.evaluated_at).map_err(|_| protocol)?;
    check(*view == expected, protocol)
}

fn validate_receipt(
    receipt: &BrokerMutationReceiptV1,
    subject: &OwnedBrokerSubjectV1,
) -> OwnedChecked<()> {
    let protocol = OwnedSupervisionErrorCodeV1::ProtocolMismatch;
    registry_uuid(&receipt.operation_id).map_err(|_| protocol)?;
    revision(receipt.grant_revision).map_err(|_| protocol)?;
    revision(receipt.heartbeat_revision).map_err(|_| protocol)?;
    check(
        receipt.broker_id == subject.broker_id
            && receipt.audit_seq > 0
            && is_hex64(&receipt.input_hash)
            && is_hex64(&receipt.audit_hash),
        protocol,
    )
}

/// Self view: identity, broker state and the latest heartbeat receipt read in
/// one checked control transaction, so a publisher can recognize only its
/// exact committed operation after a lost response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnedBrokerSelfViewV1 {
    pub profile: String,
    pub version: u32,
    pub identity: OwnedBrokerIdentityViewV1,
    pub view: BrokerViewV1,
    pub last_heartbeat_receipt: Option<BrokerMutationReceiptV1>,
}
impl OwnedBrokerSelfViewV1 {
    pub fn new(
        identity: OwnedBrokerIdentityViewV1,
        view: BrokerViewV1,
        last_heartbeat_receipt: Option<BrokerMutationReceiptV1>,
    ) -> OwnedChecked<Self> {
        let value = Self {
            profile: OWNED_SUPERVISION_PROFILE_V1.into(),
            version: OWNED_SUPERVISION_VERSION_V1,
            identity,
            view,
            last_heartbeat_receipt,
        };
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> OwnedChecked<()> {
        let protocol = OwnedSupervisionErrorCodeV1::ProtocolMismatch;
        require_profile(&self.profile, self.version)?;
        as_protocol(self.identity.validate())?;
        validate_broker_view(&self.identity.subject, &self.view)?;
        let broker = &self.view.broker;
        match (&broker.last_heartbeat, &self.last_heartbeat_receipt) {
            (None, None) => {}
            (Some(_), Some(receipt)) => {
                validate_receipt(receipt, &self.identity.subject)?;
                // Only heartbeats advance this revision: the latest heartbeat
                // operation produced the current one.
                check(
                    receipt.heartbeat_revision == broker.heartbeat_revision
                        && receipt.grant_revision <= broker.grant_revision,
                    protocol,
                )?;
            }
            _ => return Err(protocol),
        }
        bounded(self, MAX_OWNED_SELF_VIEW_BYTES, protocol)
    }
}

/// Checkpoint self view, read with its identity in one checked transaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnedBrokerCheckpointSelfViewV1 {
    pub profile: String,
    pub version: u32,
    pub identity: OwnedBrokerIdentityViewV1,
    pub view: BrokerCheckpointViewV1,
}
impl OwnedBrokerCheckpointSelfViewV1 {
    pub fn new(
        identity: OwnedBrokerIdentityViewV1,
        view: BrokerCheckpointViewV1,
    ) -> OwnedChecked<Self> {
        let value = Self {
            profile: OWNED_SUPERVISION_PROFILE_V1.into(),
            version: OWNED_SUPERVISION_VERSION_V1,
            identity,
            view,
        };
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> OwnedChecked<()> {
        let protocol = OwnedSupervisionErrorCodeV1::ProtocolMismatch;
        require_profile(&self.profile, self.version)?;
        as_protocol(self.identity.validate())?;
        check(self.view.version == 1, protocol)?;
        validate_broker_view(&self.identity.subject, &self.view.broker)?;
        if let Some(observed) = &self.view.checkpoint {
            observed.checkpoint.validate().map_err(|_| protocol)?;
            validate_receipt(&observed.receipt, &self.identity.subject)?;
            let received = parse_time(&observed.received_at).map_err(|_| protocol)?;
            let expires = parse_time(&observed.lease_expires_at).map_err(|_| protocol)?;
            check(
                observed.receipt.operation_id == observed.checkpoint.operation_id
                    && expires >= received
                    && observed.coverage == BrokerCheckpointCoverageV1::DeclaredPartial
                    && observed.publication_provenance == BrokerClaimProvenanceV1::Declared
                    && observed.reception_provenance == BrokerReceptionProvenanceV1::Observed,
                protocol,
            )?;
        }
        bounded(self, MAX_OWNED_CHECKPOINT_SELF_VIEW_BYTES, protocol)
    }
}

/// Closed handshake refusal. None of them updates an anchor or a Pending.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnedHandshakeRejectionV1 {
    Protocol,
    Incompatible(OwnedIncompatibilityV1),
    Forbidden,
    Clock,
}

fn handshake_broker(
    pins: &OwnedSupervisionPinsV1,
    identity: &OwnedBrokerIdentityViewV1,
    view: &BrokerViewV1,
    min_lease_seconds: u32,
) -> Result<(), OwnedHandshakeRejectionV1> {
    pins.validate()
        .map_err(|_| OwnedHandshakeRejectionV1::Protocol)?;
    pins.check_identity(identity)
        .map_err(OwnedHandshakeRejectionV1::Incompatible)?;
    let broker = &view.broker;
    if !matches!(broker.authorities.as_slice(), [only] if *only == pins.authority) {
        return Err(OwnedHandshakeRejectionV1::Incompatible(
            OwnedIncompatibilityV1::Authority,
        ));
    }
    if broker.revoked
        || broker.revoked_at.is_some()
        || broker.lease_seconds < min_lease_seconds
        || broker.lease_seconds > 300
    {
        return Err(OwnedHandshakeRejectionV1::Forbidden);
    }
    match view.state {
        BrokerStateV1::Unseen | BrokerStateV1::Current | BrokerStateV1::Stale => Ok(()),
        BrokerStateV1::ClockUncertain => Err(OwnedHandshakeRejectionV1::Clock),
        BrokerStateV1::Expired | BrokerStateV1::Revoked => {
            Err(OwnedHandshakeRejectionV1::Forbidden)
        }
    }
}

/// Publisher handshake for heartbeats, against its pinned enrollment.
pub fn validate_heartbeat_handshake(
    pins: &OwnedSupervisionPinsV1,
    view: &OwnedBrokerSelfViewV1,
    min_lease_seconds: u32,
) -> Result<(), OwnedHandshakeRejectionV1> {
    view.validate()
        .map_err(|_| OwnedHandshakeRejectionV1::Protocol)?;
    handshake_broker(pins, &view.identity, &view.view, min_lease_seconds)
}

/// Publisher handshake for checkpoints, against its pinned enrollment.
pub fn validate_checkpoint_handshake(
    pins: &OwnedSupervisionPinsV1,
    view: &OwnedBrokerCheckpointSelfViewV1,
    min_lease_seconds: u32,
) -> Result<(), OwnedHandshakeRejectionV1> {
    view.validate()
        .map_err(|_| OwnedHandshakeRejectionV1::Protocol)?;
    handshake_broker(pins, &view.identity, &view.view.broker, min_lease_seconds)?;
    if view
        .view
        .checkpoint
        .as_ref()
        .is_some_and(|observed| observed.freshness == BrokerCheckpointFreshnessV1::ClockUncertain)
    {
        return Err(OwnedHandshakeRejectionV1::Clock);
    }
    Ok(())
}

/// Heartbeat request. `subject` and both hashes are compared by the server
/// with its own authenticated values before any replay or mutation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnedBrokerHeartbeatRequestV1 {
    pub profile: String,
    pub version: u32,
    pub subject: OwnedBrokerSubjectV1,
    pub heartbeat: BrokerHeartbeatV1,
    pub registry_input_hash: String,
    pub operation_commitment: String,
}
impl OwnedBrokerHeartbeatRequestV1 {
    pub fn validate(&self) -> OwnedChecked<OwnedOperationCommitmentsV1> {
        require_profile(&self.profile, self.version)?;
        bounded(
            self,
            MAX_OWNED_HEARTBEAT_REQUEST_BYTES,
            OwnedSupervisionErrorCodeV1::InvalidRequest,
        )?;
        let commitments = heartbeat_commitments(&self.subject, &self.heartbeat)?;
        check(
            commitments.registry_input_hash == self.registry_input_hash
                && commitments.operation_commitment == self.operation_commitment,
            OwnedSupervisionErrorCodeV1::ProtocolMismatch,
        )?;
        Ok(commitments)
    }
    /// Server comparison with the current authenticated identity.
    pub fn verify_for(
        &self,
        current: &OwnedBrokerIdentityViewV1,
    ) -> OwnedChecked<OwnedOperationCommitmentsV1> {
        let commitments = self.validate()?;
        as_protocol(current.validate())?;
        check(
            self.subject == current.subject,
            OwnedSupervisionErrorCodeV1::IdentityMismatch,
        )?;
        Ok(commitments)
    }
}

/// Checkpoint request, compared like a heartbeat request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnedBrokerCheckpointRequestV1 {
    pub profile: String,
    pub version: u32,
    pub subject: OwnedBrokerSubjectV1,
    pub checkpoint: BrokerCheckpointV1,
    pub registry_input_hash: String,
    pub operation_commitment: String,
}
impl OwnedBrokerCheckpointRequestV1 {
    pub fn validate(&self) -> OwnedChecked<OwnedOperationCommitmentsV1> {
        require_profile(&self.profile, self.version)?;
        bounded(
            self,
            MAX_OWNED_CHECKPOINT_REQUEST_BYTES,
            OwnedSupervisionErrorCodeV1::InvalidRequest,
        )?;
        let commitments = checkpoint_commitments(&self.subject, &self.checkpoint)?;
        check(
            commitments.registry_input_hash == self.registry_input_hash
                && commitments.operation_commitment == self.operation_commitment,
            OwnedSupervisionErrorCodeV1::ProtocolMismatch,
        )?;
        Ok(commitments)
    }
    pub fn verify_for(
        &self,
        current: &OwnedBrokerIdentityViewV1,
    ) -> OwnedChecked<OwnedOperationCommitmentsV1> {
        let commitments = self.validate()?;
        as_protocol(current.validate())?;
        check(
            self.subject == current.subject,
            OwnedSupervisionErrorCodeV1::IdentityMismatch,
        )?;
        Ok(commitments)
    }
}

/// Positive heartbeat outcome, emitted only after the audit event and the
/// projection are committed (or for an exact replay revalidated under the
/// current authority). A replay returns the original receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnedBrokerHeartbeatResultV1 {
    pub profile: String,
    pub version: u32,
    pub identity: OwnedBrokerIdentityViewV1,
    pub operation_commitment: String,
    pub result: BrokerMutationResultV1,
}
impl OwnedBrokerHeartbeatResultV1 {
    pub fn new(
        identity: OwnedBrokerIdentityViewV1,
        operation_commitment: String,
        result: BrokerMutationResultV1,
    ) -> OwnedChecked<Self> {
        let value = Self {
            profile: OWNED_SUPERVISION_PROFILE_V1.into(),
            version: OWNED_SUPERVISION_VERSION_V1,
            identity,
            operation_commitment,
            result,
        };
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> OwnedChecked<()> {
        let protocol = OwnedSupervisionErrorCodeV1::ProtocolMismatch;
        require_profile(&self.profile, self.version)?;
        as_protocol(self.identity.validate())?;
        check(is_hex64(&self.operation_commitment), protocol)?;
        validate_receipt(&self.result.receipt, &self.identity.subject)?;
        check(
            self.result.broker.broker_id == self.identity.subject.broker_id,
            protocol,
        )?;
        bounded(self, MAX_OWNED_HEARTBEAT_RESULT_BYTES, protocol)
    }
}

/// Positive checkpoint outcome, with the same durability rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnedBrokerCheckpointResultV1 {
    pub profile: String,
    pub version: u32,
    pub identity: OwnedBrokerIdentityViewV1,
    pub operation_commitment: String,
    pub result: BrokerCheckpointResultV1,
}
impl OwnedBrokerCheckpointResultV1 {
    pub fn new(
        identity: OwnedBrokerIdentityViewV1,
        operation_commitment: String,
        result: BrokerCheckpointResultV1,
    ) -> OwnedChecked<Self> {
        let value = Self {
            profile: OWNED_SUPERVISION_PROFILE_V1.into(),
            version: OWNED_SUPERVISION_VERSION_V1,
            identity,
            operation_commitment,
            result,
        };
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> OwnedChecked<()> {
        let protocol = OwnedSupervisionErrorCodeV1::ProtocolMismatch;
        require_profile(&self.profile, self.version)?;
        as_protocol(self.identity.validate())?;
        check(is_hex64(&self.operation_commitment), protocol)?;
        self.result.checkpoint.validate().map_err(|_| protocol)?;
        validate_receipt(&self.result.receipt, &self.identity.subject)?;
        check(
            self.result.receipt.operation_id == self.result.checkpoint.operation_id,
            protocol,
        )?;
        bounded(self, MAX_OWNED_CHECKPOINT_RESULT_BYTES, protocol)
    }
}

/// Closed reason for refusing a positive response or a self-read as the ACK
/// of a retained Pending. The Pending is kept in every case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnedAckRejectionV1 {
    Protocol,
    Incompatible(OwnedIncompatibilityV1),
    Commitment,
    Receipt,
    Snapshot,
    /// The remote durable state shows another or a regressed operation.
    Conflict,
}

/// How a publisher established its acknowledgement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnedAcknowledgementSourceV1 {
    Response,
    SelfRead,
}

/// Durable publisher Pending, written before any POST. Retries, timeouts,
/// restarts and lost acknowledgements reuse its exact bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnedHeartbeatPendingV1 {
    pub profile: String,
    pub version: u32,
    pub identity: OwnedBrokerIdentityViewV1,
    pub prior: BrokerViewV1,
    pub heartbeat: BrokerHeartbeatV1,
    pub registry_input_hash: String,
    pub operation_commitment: String,
}
impl OwnedHeartbeatPendingV1 {
    /// Next operation from a validated handshake self view. The caller owns
    /// the fresh operation ID and must persist the Pending before any POST.
    pub fn prepare(view: &OwnedBrokerSelfViewV1, operation_id: String) -> OwnedChecked<Self> {
        view.validate()?;
        let subject = &view.identity.subject;
        let previous = previous_heartbeat_sequence(&view.view.broker, &subject.incarnation_id);
        let heartbeat = BrokerHeartbeatV1 {
            version: 1,
            operation_id,
            expected_heartbeat_revision: view.view.broker.heartbeat_revision,
            incarnation_id: subject.incarnation_id.clone(),
            sequence: previous
                .checked_add(1)
                .ok_or(OwnedSupervisionErrorCodeV1::SequenceConflict)?,
        };
        let commitments = heartbeat_commitments(subject, &heartbeat)?;
        let pending = Self {
            profile: OWNED_SUPERVISION_PROFILE_V1.into(),
            version: OWNED_SUPERVISION_VERSION_V1,
            identity: view.identity.clone(),
            prior: view.view.clone(),
            heartbeat,
            registry_input_hash: commitments.registry_input_hash,
            operation_commitment: commitments.operation_commitment,
        };
        pending.validate()?;
        Ok(pending)
    }

    pub fn validate(&self) -> OwnedChecked<()> {
        let invalid = OwnedSupervisionErrorCodeV1::InvalidRequest;
        require_profile(&self.profile, self.version)?;
        self.identity.validate()?;
        let subject = &self.identity.subject;
        validate_broker_view(subject, &self.prior)?;
        self.heartbeat.validate().map_err(|_| invalid)?;
        let prior = &self.prior.broker;
        let previous = previous_heartbeat_sequence(prior, &subject.incarnation_id);
        check(
            self.heartbeat.incarnation_id == subject.incarnation_id
                && self.heartbeat.expected_heartbeat_revision == prior.heartbeat_revision
                && previous.checked_add(1) == Some(self.heartbeat.sequence),
            invalid,
        )?;
        let commitments = heartbeat_commitments(subject, &self.heartbeat)?;
        check(
            commitments.registry_input_hash == self.registry_input_hash
                && commitments.operation_commitment == self.operation_commitment,
            OwnedSupervisionErrorCodeV1::ProtocolMismatch,
        )?;
        bounded(self, MAX_OWNED_HEARTBEAT_PENDING_BYTES, invalid)
    }

    pub fn request(&self) -> OwnedBrokerHeartbeatRequestV1 {
        OwnedBrokerHeartbeatRequestV1 {
            profile: self.profile.clone(),
            version: self.version,
            subject: self.identity.subject.clone(),
            heartbeat: self.heartbeat.clone(),
            registry_input_hash: self.registry_input_hash.clone(),
            operation_commitment: self.operation_commitment.clone(),
        }
    }

    /// A 200 or a syntactically valid body is not enough: the result must be
    /// the exact successor of the retained prior view under this operation.
    pub fn validate_result(
        &self,
        result: &OwnedBrokerHeartbeatResultV1,
    ) -> Result<(), OwnedAckRejectionV1> {
        use OwnedAckRejectionV1 as Rejection;
        result.validate().map_err(|_| Rejection::Protocol)?;
        check_replay_identity(&self.identity, &result.identity)
            .map_err(Rejection::Incompatible)?;
        if result.operation_commitment != self.operation_commitment {
            return Err(Rejection::Commitment);
        }
        let subject = &self.identity.subject;
        let prior = &self.prior.broker;
        let heartbeat = result
            .result
            .broker
            .last_heartbeat
            .as_ref()
            .ok_or(Rejection::Snapshot)?;
        let received = parse_time(&heartbeat.received_at).map_err(|_| Rejection::Snapshot)?;
        let evaluated = parse_time(&self.prior.evaluated_at).map_err(|_| Rejection::Snapshot)?;
        let not_after = parse_time(&prior.not_after).map_err(|_| Rejection::Snapshot)?;
        let expiry = received
            .checked_add_signed(chrono::Duration::seconds(i64::from(prior.lease_seconds)))
            .ok_or(Rejection::Snapshot)?
            .min(not_after);
        let mut expected = prior.clone();
        expected.heartbeat_revision = expected
            .heartbeat_revision
            .checked_add(1)
            .ok_or(Rejection::Snapshot)?;
        expected.last_heartbeat = Some(BrokerHeartbeatReceiptV1 {
            incarnation_id: self.heartbeat.incarnation_id.clone(),
            sequence: self.heartbeat.sequence,
            received_at: format_time(received),
            lease_expires_at: format_time(expiry),
            heartbeat_provenance: BrokerClaimProvenanceV1::Declared,
            reception_provenance: BrokerReceptionProvenanceV1::Observed,
        });
        if result.result.broker != expected || received < evaluated || received >= not_after {
            return Err(Rejection::Snapshot);
        }
        let receipt = &result.result.receipt;
        if receipt.operation_id != self.heartbeat.operation_id
            || receipt.broker_id != subject.broker_id
            || receipt.grant_revision != subject.grant_revision
            || receipt.heartbeat_revision != expected.heartbeat_revision
            || receipt.input_hash != self.registry_input_hash
            || receipt.audit_seq <= 0
            || !is_hex64(&receipt.audit_hash)
        {
            return Err(Rejection::Receipt);
        }
        Ok(())
    }

    /// Exact self-read reconciliation after a lost response: `Ok(None)` means
    /// this operation is not committed yet and stays the next one to send.
    pub fn recognize(
        &self,
        view: &OwnedBrokerSelfViewV1,
    ) -> Result<Option<OwnedBrokerHeartbeatResultV1>, OwnedAckRejectionV1> {
        use OwnedAckRejectionV1 as Rejection;
        view.validate().map_err(|_| Rejection::Protocol)?;
        check_replay_identity(&self.identity, &view.identity).map_err(Rejection::Incompatible)?;
        let current =
            previous_heartbeat_sequence(&view.view.broker, &self.identity.subject.incarnation_id);
        if current < self.heartbeat.sequence {
            return if current.checked_add(1) == Some(self.heartbeat.sequence) {
                Ok(None)
            } else {
                Err(Rejection::Conflict)
            };
        }
        let receipt = view
            .last_heartbeat_receipt
            .as_ref()
            .ok_or(Rejection::Protocol)?;
        if current != self.heartbeat.sequence
            || receipt.operation_id != self.heartbeat.operation_id
            || receipt.input_hash != self.registry_input_hash
        {
            return Err(Rejection::Conflict);
        }
        let result = OwnedBrokerHeartbeatResultV1 {
            profile: OWNED_SUPERVISION_PROFILE_V1.into(),
            version: OWNED_SUPERVISION_VERSION_V1,
            identity: view.identity.clone(),
            operation_commitment: self.operation_commitment.clone(),
            result: BrokerMutationResultV1 {
                broker: view.view.broker.clone(),
                receipt: receipt.clone(),
                replayed: true,
            },
        };
        self.validate_result(&result)?;
        Ok(Some(result))
    }
}

/// Durable checkpoint Pending, written before any POST. A newer source value
/// is coalesced only after this Pending's acknowledgement is durable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnedCheckpointPendingV1 {
    pub profile: String,
    pub version: u32,
    pub identity: OwnedBrokerIdentityViewV1,
    pub checkpoint: BrokerCheckpointV1,
    pub registry_input_hash: String,
    pub operation_commitment: String,
}
impl OwnedCheckpointPendingV1 {
    /// `previous_sequence` is the locally acknowledged head of this journal
    /// (zero for none); it is never taken from a remote view without history.
    pub fn prepare(
        identity: &OwnedBrokerIdentityViewV1,
        previous_sequence: u64,
        snapshot: BrokerCheckpointSnapshotV1,
        operation_id: String,
    ) -> OwnedChecked<Self> {
        identity.validate()?;
        let checkpoint = BrokerCheckpointV1 {
            version: 1,
            operation_id,
            incarnation_id: identity.subject.incarnation_id.clone(),
            previous_sequence,
            sequence: previous_sequence
                .checked_add(1)
                .ok_or(OwnedSupervisionErrorCodeV1::SequenceConflict)?,
            snapshot,
        };
        let commitments = checkpoint_commitments(&identity.subject, &checkpoint)?;
        let pending = Self {
            profile: OWNED_SUPERVISION_PROFILE_V1.into(),
            version: OWNED_SUPERVISION_VERSION_V1,
            identity: identity.clone(),
            checkpoint,
            registry_input_hash: commitments.registry_input_hash,
            operation_commitment: commitments.operation_commitment,
        };
        pending.validate()?;
        Ok(pending)
    }

    pub fn validate(&self) -> OwnedChecked<()> {
        let invalid = OwnedSupervisionErrorCodeV1::InvalidRequest;
        require_profile(&self.profile, self.version)?;
        self.identity.validate()?;
        let commitments = checkpoint_commitments(&self.identity.subject, &self.checkpoint)?;
        check(
            commitments.registry_input_hash == self.registry_input_hash
                && commitments.operation_commitment == self.operation_commitment,
            OwnedSupervisionErrorCodeV1::ProtocolMismatch,
        )?;
        bounded(self, MAX_OWNED_CHECKPOINT_PENDING_BYTES, invalid)
    }

    pub fn request(&self) -> OwnedBrokerCheckpointRequestV1 {
        OwnedBrokerCheckpointRequestV1 {
            profile: self.profile.clone(),
            version: self.version,
            subject: self.identity.subject.clone(),
            checkpoint: self.checkpoint.clone(),
            registry_input_hash: self.registry_input_hash.clone(),
            operation_commitment: self.operation_commitment.clone(),
        }
    }

    pub fn validate_result(
        &self,
        result: &OwnedBrokerCheckpointResultV1,
    ) -> Result<(), OwnedAckRejectionV1> {
        use OwnedAckRejectionV1 as Rejection;
        result.validate().map_err(|_| Rejection::Protocol)?;
        check_replay_identity(&self.identity, &result.identity)
            .map_err(Rejection::Incompatible)?;
        if result.operation_commitment != self.operation_commitment {
            return Err(Rejection::Commitment);
        }
        if result.result.checkpoint != self.checkpoint {
            return Err(Rejection::Snapshot);
        }
        let subject = &self.identity.subject;
        let receipt = &result.result.receipt;
        if receipt.operation_id != self.checkpoint.operation_id
            || receipt.broker_id != subject.broker_id
            || receipt.grant_revision != subject.grant_revision
            || receipt.input_hash != self.registry_input_hash
            || receipt.audit_seq <= 0
            || !is_hex64(&receipt.audit_hash)
        {
            return Err(Rejection::Receipt);
        }
        Ok(())
    }

    /// Exact self-read reconciliation; never adopts a remote head without the
    /// local acknowledged history that `previous_sequence` represents.
    pub fn recognize(
        &self,
        view: &OwnedBrokerCheckpointSelfViewV1,
    ) -> Result<Option<OwnedBrokerCheckpointResultV1>, OwnedAckRejectionV1> {
        use OwnedAckRejectionV1 as Rejection;
        view.validate().map_err(|_| Rejection::Protocol)?;
        check_replay_identity(&self.identity, &view.identity).map_err(Rejection::Incompatible)?;
        let incarnation = &self.identity.subject.incarnation_id;
        let Some(observed) = view
            .view
            .checkpoint
            .as_ref()
            .filter(|observed| observed.checkpoint.incarnation_id == *incarnation)
        else {
            return if self.checkpoint.previous_sequence == 0 {
                Ok(None)
            } else {
                Err(Rejection::Conflict)
            };
        };
        if observed.checkpoint.sequence == self.checkpoint.previous_sequence
            && observed.checkpoint.operation_id != self.checkpoint.operation_id
        {
            return Ok(None);
        }
        if observed.checkpoint != self.checkpoint
            || observed.receipt.input_hash != self.registry_input_hash
            || !observed.in_current_grant
        {
            return Err(Rejection::Conflict);
        }
        let result = OwnedBrokerCheckpointResultV1 {
            profile: OWNED_SUPERVISION_PROFILE_V1.into(),
            version: OWNED_SUPERVISION_VERSION_V1,
            identity: view.identity.clone(),
            operation_commitment: self.operation_commitment.clone(),
            result: BrokerCheckpointResultV1 {
                checkpoint: observed.checkpoint.clone(),
                receipt: observed.receipt.clone(),
                replayed: true,
            },
        };
        self.validate_result(&result)?;
        Ok(Some(result))
    }
}
