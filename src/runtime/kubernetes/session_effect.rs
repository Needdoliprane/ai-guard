//! V2 session effect mechanics behind a sealed guard (DESIGN layer D).
//!
//! Every effect function is restricted to `runtime::kubernetes` and consumes
//! a guard of the sealed `SessionEffectGuardV2`. Its only implementor is the
//! `cfg(test)` loopback guard below; the central effect permits wait for the
//! Root contract (R1-R3, R7), so no product binary can reach an effect. No
//! outcome here is a start, a terminal, a settlement or a quota release.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "V2 effect mechanics without a production caller or guard (DESIGN section 0, layer D)"
    )
)]
use super::{
    KubernetesAuthorityV1, KubernetesError, KubernetesRuntime,
    session_claim::KubernetesSessionClaimRequestV2,
    session_instance::{
        KubernetesSessionRuntimeError, SessionContainerStateV2, SessionInstanceV2,
        SessionObservationStateV2, SessionObservedV2, parse_session_observation,
        validate_admitted_session_pod,
    },
    session_pod::{KubernetesSessionPodRequestV2, SESSION_TERMINAL_FINALIZER},
    session_requests::{delete_pod_request, release_finalizer_request, release_gate_request},
    transport::{CreatedClaimResponseV2, CreatedSessionResponseV2},
    valid_dns_label,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicBool, Ordering};

const EFFECT_DOMAIN: &[u8] = b"ai-guard:kubernetes-session-effect-request:v2\0";
const REFUSED: KubernetesSessionRuntimeError =
    KubernetesSessionRuntimeError::EffectRefusedBeforeSend;
const INVALID: KubernetesSessionRuntimeError =
    KubernetesSessionRuntimeError::Kubernetes(KubernetesError::InvalidResponse);
type Outcome<T> = Result<T, KubernetesSessionRuntimeError>;

pub(in crate::runtime::kubernetes) mod sealed {
    /// Nameable only inside `runtime::kubernetes`: no other module can
    /// implement the effect guard.
    pub trait Sealed {}
}

/// Last synchronous check of one prebuilt effect request.
///
/// The transport calls it after the request is built and initiates that
/// request with no await in between. A refusal means no request was sent.
/// The transport refuses a raised cancellation itself just before this call.
/// Every production implementor must also refuse a raised cancellation
/// (defence in depth; CONTRACT S5 obligation 5). The `cfg(test)`
/// `LoopbackEffectGuardV2::ignoring_cancellation` mode skips that check only
/// to isolate the transport check in one test.
pub(in crate::runtime::kubernetes) trait SessionEffectGuardV2:
    sealed::Sealed + Send
{
    fn check_before_send(
        self,
        effect: SessionEffectKindV2,
        request_sha256: &str,
        authority: &KubernetesAuthorityV1,
        cancelled: &AtomicBool,
    ) -> Result<(), KubernetesSessionRuntimeError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionEffectKindV2 {
    CreateHeld,
    ReleaseGate,
    Attach,
    DeleteExact,
    /// Per-attempt workspace claim CREATE (R5).
    CreateClaim,
    /// Workspace claim DELETE with a UID precondition.
    DeleteClaimExact,
    /// Removal of the terminal finalizer after the terminal was read (D5).
    ReleaseFinalizer,
}
impl SessionEffectKindV2 {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::CreateHeld => "create_held",
            Self::ReleaseGate => "release_gate",
            Self::Attach => "attach",
            Self::DeleteExact => "delete_exact",
            Self::CreateClaim => "create_claim",
            Self::DeleteClaimExact => "delete_claim_exact",
            Self::ReleaseFinalizer => "release_finalizer",
        }
    }
}

/// Digest of one exact effect request.
///
/// SHA-256 of the domain, kind, authority fingerprint, method, path with
/// query and body, separated as documented. The CREATE guard receives the
/// rendered `request_sha256` instead (the value the central claim binds); a
/// future permit compares this digest with its claim.
pub(crate) fn effect_request_sha256(
    kind: SessionEffectKindV2,
    authority_fingerprint: &str,
    method: &str,
    path_and_query: &str,
    body: &[u8],
) -> String {
    let mut hash = Sha256::new();
    hash.update(EFFECT_DOMAIN);
    hash.update(kind.as_str().as_bytes());
    hash.update(b"\0");
    hash.update(authority_fingerprint.as_bytes());
    hash.update(b"\0");
    hash.update(method.as_bytes());
    hash.update(b" ");
    hash.update(path_and_query.as_bytes());
    hash.update(b"\0");
    hash.update(body);
    hex::encode(hash.finalize())
}

/// One fixed V2 effect route.
///
/// The transport builds the method, path, query and headers; a caller names
/// only a rendered request or the Pod name of a rendered target.
pub(in crate::runtime::kubernetes) enum SessionHttpEffectV2<'a> {
    /// `POST .../pods?fieldValidation=Strict` with the exact rendered bytes.
    CreateHeld {
        request: &'a KubernetesSessionPodRequestV2,
    },
    /// `PATCH .../pods/{name}?fieldValidation=Strict`, JSON Patch body.
    ReleaseGate { name: &'a str, body: Vec<u8> },
    /// `DELETE .../pods/{name}`, DeleteOptions body with preconditions.
    DeleteExact { name: &'a str, body: Vec<u8> },
    /// `POST .../persistentvolumeclaims?fieldValidation=Strict`, exact bytes.
    CreateClaim {
        request: &'a KubernetesSessionClaimRequestV2,
    },
    /// `DELETE .../persistentvolumeclaims/{name}`, UID precondition.
    DeleteClaimExact { name: &'a str, body: Vec<u8> },
    /// `PATCH .../pods/{name}?fieldValidation=Strict`, JSON Patch body.
    ReleaseFinalizer { name: &'a str, body: Vec<u8> },
}
impl SessionHttpEffectV2<'_> {
    pub(in crate::runtime::kubernetes) fn kind(&self) -> SessionEffectKindV2 {
        match self {
            Self::CreateHeld { .. } => SessionEffectKindV2::CreateHeld,
            Self::ReleaseGate { .. } => SessionEffectKindV2::ReleaseGate,
            Self::DeleteExact { .. } => SessionEffectKindV2::DeleteExact,
            Self::CreateClaim { .. } => SessionEffectKindV2::CreateClaim,
            Self::DeleteClaimExact { .. } => SessionEffectKindV2::DeleteClaimExact,
            Self::ReleaseFinalizer { .. } => SessionEffectKindV2::ReleaseFinalizer,
        }
    }
}
/// Answer of an initiated effect request: the effect may have happened.
#[derive(Debug)]
pub(in crate::runtime::kubernetes) enum SessionHttpResponseV2 {
    /// Real bounded 201 of this exact CREATE POST; still needs admission checks.
    CreatedResponse(Box<CreatedSessionResponseV2>),
    /// Real bounded 201 of this exact claim POST; still needs its projection.
    CreatedClaim(Box<CreatedClaimResponseV2>),
    /// Status and body read within the response bound.
    Complete { status: u16, bytes: Vec<u8> },
    /// Transport error, timeout, unreadable or oversized body.
    Uncertain {
        status: Option<u16>,
        error: KubernetesError,
    },
}

/// Held CREATE result. Anything but a validated 201 keeps ownership unknown:
/// a Pod may exist, and no second POST follows.
#[derive(Debug)]
pub(crate) enum SessionCreateOutcomeV2 {
    CreatedHeld(Box<SessionInstanceV2>),
    OwnershipUnknown {
        http_status: Option<u16>,
        cause: KubernetesSessionRuntimeError,
    },
}
/// Release PATCH result. An acknowledged release is not Running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SessionReleaseOutcomeV2 {
    /// Validated 200: same UID, released spec, no gate.
    Released { resource_version: String },
    /// 409 or 422: this request was not applied (a `test` operation or the
    /// resourceVersion check failed). The state of the instance is unknown:
    /// another actor may already have released, changed or replaced it.
    /// Re-observe before any decision; this is never a Held proof.
    RefusedNotApplied { status: u16 },
    /// The Pod may be schedulable.
    Uncertain {
        http_status: Option<u16>,
        cause: KubernetesSessionRuntimeError,
    },
}
/// DELETE result. None of these is terminal: termination needs an exact
/// observation of the same instance and container.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionDeleteOutcomeV2 {
    /// 200 or 202: the API accepted the request. The body is not inspected;
    /// only a later exact observation or absence can settle the instance.
    DeletionRequested,
    NotFound,
    PreconditionFailed,
    Uncertain {
        http_status: Option<u16>,
        cause: KubernetesSessionRuntimeError,
    },
}

/// Finalizer removal result. None of these is a settlement; only the later
/// observation of absence by UID or the retained terminal settles the Pod.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SessionFinalizerOutcomeV2 {
    /// Validated 200 of the same UID without the ai-guard finalizer.
    Released,
    /// 409 or 422: not applied (UID or finalizer `test` failed).
    RefusedNotApplied { status: u16 },
    /// 404: no object at the name; never proof that this UID was released.
    NotFound,
    Uncertain {
        http_status: Option<u16>,
        cause: KubernetesSessionRuntimeError,
    },
}

/// Test-only guard bound to an owned loopback listener.
///
/// Same endpoint check as the V1 held-create test activation. It is the only
/// implementor of the sealed guard; no production constructor exists.
#[cfg(test)]
pub(in crate::runtime::kubernetes) struct LoopbackEffectGuardV2 {
    expected: SessionEffectKindV2,
    request_sha256: String,
    owned_listener: std::net::TcpListener,
    /// False only for the test that isolates the transport's own
    /// cancellation check.
    refuses_cancellation: bool,
}
#[cfg(test)]
impl LoopbackEffectGuardV2 {
    pub(in crate::runtime::kubernetes) fn new(
        expected: SessionEffectKindV2,
        request_sha256: &str,
        owned_listener: &std::net::TcpListener,
    ) -> std::io::Result<Self> {
        Ok(Self {
            expected,
            request_sha256: request_sha256.into(),
            owned_listener: owned_listener.try_clone()?,
            refuses_cancellation: true,
        })
    }
    /// The same guard without its own cancellation check, so that a test can
    /// show that the transport refuses a raised cancellation by itself.
    pub(in crate::runtime::kubernetes) fn ignoring_cancellation(
        expected: SessionEffectKindV2,
        request_sha256: &str,
        owned_listener: &std::net::TcpListener,
    ) -> std::io::Result<Self> {
        Ok(Self {
            refuses_cancellation: false,
            ..Self::new(expected, request_sha256, owned_listener)?
        })
    }
}
#[cfg(test)]
impl sealed::Sealed for LoopbackEffectGuardV2 {}
#[cfg(test)]
impl SessionEffectGuardV2 for LoopbackEffectGuardV2 {
    fn check_before_send(
        self,
        effect: SessionEffectKindV2,
        request_sha256: &str,
        authority: &KubernetesAuthorityV1,
        cancelled: &AtomicBool,
    ) -> Result<(), KubernetesSessionRuntimeError> {
        if (self.refuses_cancellation && cancelled.load(Ordering::Acquire))
            || effect != self.expected
            || request_sha256 != self.request_sha256
            || authority.validate().is_err()
        {
            return Err(REFUSED);
        }
        let endpoint = reqwest::Url::parse(&authority.endpoint).map_err(|_| REFUSED)?;
        let ip = endpoint
            .host_str()
            .and_then(|host| {
                host.trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .ok()
            })
            .filter(std::net::IpAddr::is_loopback)
            .ok_or(REFUSED)?;
        let port = endpoint.port_or_known_default().ok_or(REFUSED)?;
        if self.owned_listener.local_addr().map_err(|_| REFUSED)?
            != std::net::SocketAddr::new(ip, port)
        {
            return Err(REFUSED);
        }
        Ok(())
    }
}

impl KubernetesRuntime {
    /// Held CREATE of the exact retained bytes, sent at most once.
    ///
    /// Only a 201 that passes the admitted projection and the authority
    /// recheck retains an instance. Every other answer, including a lost
    /// response, is `OwnershipUnknown`.
    pub(in crate::runtime::kubernetes) async fn create_session_held<G: SessionEffectGuardV2>(
        &self,
        request: &KubernetesSessionPodRequestV2,
        guard: G,
        cancelled: &AtomicBool,
    ) -> Outcome<SessionCreateOutcomeV2> {
        let target = request.target();
        self.require_session_effect_target(
            target.authority_fingerprint(),
            target.pod_name(),
            cancelled,
        )?;
        // The anchors detect divergence; they are not an atomic NamespaceUID
        // precondition on the following CREATE.
        self.verify_active_authority()
            .await
            .map_err(KubernetesSessionRuntimeError::Kubernetes)?;
        let effect = SessionHttpEffectV2::CreateHeld { request };
        let response = match self
            .transport
            .send_session_effect(&self.binding, effect, guard, cancelled)
            .await?
        {
            SessionHttpResponseV2::CreatedResponse(response) => response,
            SessionHttpResponseV2::CreatedClaim(_) => {
                return Ok(ownership_unknown(Some(201), INVALID));
            }
            SessionHttpResponseV2::Complete { status, .. } => {
                return Ok(ownership_unknown(Some(status), rejected(status)));
            }
            SessionHttpResponseV2::Uncertain { status, error } => {
                return Ok(ownership_unknown(
                    status,
                    KubernetesSessionRuntimeError::Kubernetes(error),
                ));
            }
        };
        // Only the exact transport CREATE201 can produce CreatedResponse.
        // Keep that observed status on either later ambiguous outcome.
        let instance = match validate_admitted_session_pod(request, *response) {
            Ok(instance) => instance,
            Err(cause) => return Ok(ownership_unknown(Some(201), cause)),
        };
        // Detection after the possible effect, not prevention.
        if let Err(error) = self.verify_active_authority().await {
            return Ok(ownership_unknown(
                Some(201),
                KubernetesSessionRuntimeError::Kubernetes(error),
            ));
        }
        Ok(SessionCreateOutcomeV2::CreatedHeld(Box::new(instance)))
    }

    /// Gate removal bound by JSON Patch `test` operations.
    ///
    /// `observed` must be a Held observation of this exact instance; the
    /// patch tests its UID, its observed resourceVersion and the exact gate.
    pub(in crate::runtime::kubernetes) async fn release_session_gate<G: SessionEffectGuardV2>(
        &self,
        instance: &SessionInstanceV2,
        observed: &SessionObservedV2,
        guard: G,
        cancelled: &AtomicBool,
    ) -> Outcome<SessionReleaseOutcomeV2> {
        let target = instance.target();
        self.require_session_effect_target(
            target.authority_fingerprint(),
            target.pod_name(),
            cancelled,
        )?;
        if observed.pod_uid() != instance.pod_uid() {
            return Err(REFUSED);
        }
        match observed.state() {
            SessionObservationStateV2::Held => {}
            SessionObservationStateV2::Deleting { .. } => {
                return Err(KubernetesSessionRuntimeError::InstanceDeleting);
            }
            SessionObservationStateV2::ReleasedUnscheduled
            | SessionObservationStateV2::Scheduled { .. } => return Err(REFUSED),
        }
        let body = release_gate_request(
            &self.binding,
            target.pod_name(),
            instance.pod_uid(),
            observed.resource_version(),
        )?
        .body;
        self.verify_active_authority()
            .await
            .map_err(KubernetesSessionRuntimeError::Kubernetes)?;
        let effect = SessionHttpEffectV2::ReleaseGate {
            name: target.pod_name(),
            body,
        };
        let (status, bytes) = match self
            .transport
            .send_session_effect(&self.binding, effect, guard, cancelled)
            .await?
        {
            SessionHttpResponseV2::Complete { status, bytes } => (status, bytes),
            SessionHttpResponseV2::CreatedResponse(_) | SessionHttpResponseV2::CreatedClaim(_) => {
                return Ok(SessionReleaseOutcomeV2::Uncertain {
                    http_status: Some(201),
                    cause: INVALID,
                });
            }
            SessionHttpResponseV2::Uncertain { status, error } => {
                return Ok(SessionReleaseOutcomeV2::Uncertain {
                    http_status: status,
                    cause: KubernetesSessionRuntimeError::Kubernetes(error),
                });
            }
        };
        let outcome = match status {
            200 => match released_resource_version(&self.binding, instance, &bytes) {
                Ok(resource_version) => SessionReleaseOutcomeV2::Released { resource_version },
                Err(cause) => {
                    return Ok(SessionReleaseOutcomeV2::Uncertain {
                        http_status: Some(status),
                        cause,
                    });
                }
            },
            409 | 422 => SessionReleaseOutcomeV2::RefusedNotApplied { status },
            _ => {
                return Ok(SessionReleaseOutcomeV2::Uncertain {
                    http_status: Some(status),
                    cause: rejected(status),
                });
            }
        };
        if let Err(error) = self.verify_active_authority().await {
            return Ok(SessionReleaseOutcomeV2::Uncertain {
                http_status: Some(status),
                cause: KubernetesSessionRuntimeError::Kubernetes(error),
            });
        }
        Ok(outcome)
    }

    /// DELETE of the exact instance (code only; never run on a cluster).
    ///
    /// The UID precondition is always sent; the resourceVersion one only for a
    /// Held observation. No grace period override is ever sent.
    pub(in crate::runtime::kubernetes) async fn delete_session_exact<G: SessionEffectGuardV2>(
        &self,
        instance: &SessionInstanceV2,
        observed: &SessionObservedV2,
        guard: G,
        cancelled: &AtomicBool,
    ) -> Outcome<SessionDeleteOutcomeV2> {
        let target = instance.target();
        self.require_session_effect_target(
            target.authority_fingerprint(),
            target.pod_name(),
            cancelled,
        )?;
        if observed.pod_uid() != instance.pod_uid() {
            return Err(REFUSED);
        }
        // A forced deletion (gracePeriodSeconds 0) can remove the object
        // while its containers still run (EXT-11), so it is never requested.
        let held = matches!(observed.state(), SessionObservationStateV2::Held);
        let body = delete_pod_request(
            &self.binding,
            target.pod_name(),
            instance.pod_uid(),
            held.then(|| observed.resource_version()),
        )?
        .body;
        self.verify_active_authority()
            .await
            .map_err(KubernetesSessionRuntimeError::Kubernetes)?;
        let effect = SessionHttpEffectV2::DeleteExact {
            name: target.pod_name(),
            body,
        };
        let status = match self
            .transport
            .send_session_effect(&self.binding, effect, guard, cancelled)
            .await?
        {
            SessionHttpResponseV2::Complete { status, .. } => status,
            SessionHttpResponseV2::CreatedResponse(_) | SessionHttpResponseV2::CreatedClaim(_) => {
                return Ok(SessionDeleteOutcomeV2::Uncertain {
                    http_status: Some(201),
                    cause: INVALID,
                });
            }
            SessionHttpResponseV2::Uncertain { status, error } => {
                return Ok(SessionDeleteOutcomeV2::Uncertain {
                    http_status: status,
                    cause: KubernetesSessionRuntimeError::Kubernetes(error),
                });
            }
        };
        let outcome = match status {
            200 | 202 => SessionDeleteOutcomeV2::DeletionRequested,
            404 => SessionDeleteOutcomeV2::NotFound,
            409 => SessionDeleteOutcomeV2::PreconditionFailed,
            _ => {
                return Ok(SessionDeleteOutcomeV2::Uncertain {
                    http_status: Some(status),
                    cause: rejected(status),
                });
            }
        };
        if let Err(error) = self.verify_active_authority().await {
            return Ok(SessionDeleteOutcomeV2::Uncertain {
                http_status: Some(status),
                cause: KubernetesSessionRuntimeError::Kubernetes(error),
            });
        }
        Ok(outcome)
    }

    /// Removal of the ai-guard terminal finalizer, bound by JSON Patch
    /// `test` operations on the UID and the exact finalizer list.
    ///
    /// Refused before any request unless `observed` is this exact instance
    /// with a terminated container, or a deletion of a Pod that was never
    /// released. The terminal must already be retained by the caller.
    pub(in crate::runtime::kubernetes) async fn release_session_finalizer<
        G: SessionEffectGuardV2,
    >(
        &self,
        instance: &SessionInstanceV2,
        observed: &SessionObservedV2,
        guard: G,
        cancelled: &AtomicBool,
    ) -> Outcome<SessionFinalizerOutcomeV2> {
        let target = instance.target();
        self.require_session_effect_target(
            target.authority_fingerprint(),
            target.pod_name(),
            cancelled,
        )?;
        if observed.pod_uid() != instance.pod_uid() || !terminal_or_never_released(observed) {
            return Err(REFUSED);
        }
        let body =
            release_finalizer_request(&self.binding, target.pod_name(), instance.pod_uid())?.body;
        self.verify_active_authority()
            .await
            .map_err(KubernetesSessionRuntimeError::Kubernetes)?;
        let effect = SessionHttpEffectV2::ReleaseFinalizer {
            name: target.pod_name(),
            body,
        };
        let uncertain =
            |http_status, cause| SessionFinalizerOutcomeV2::Uncertain { http_status, cause };
        let (status, bytes) = match self
            .transport
            .send_session_effect(&self.binding, effect, guard, cancelled)
            .await?
        {
            SessionHttpResponseV2::Complete { status, bytes } => (status, bytes),
            SessionHttpResponseV2::CreatedResponse(_) | SessionHttpResponseV2::CreatedClaim(_) => {
                return Ok(uncertain(Some(201), INVALID));
            }
            SessionHttpResponseV2::Uncertain { status, error } => {
                return Ok(uncertain(
                    status,
                    KubernetesSessionRuntimeError::Kubernetes(error),
                ));
            }
        };
        let outcome = match status {
            200 => match finalizer_released(instance, &bytes) {
                Ok(()) => SessionFinalizerOutcomeV2::Released,
                Err(cause) => return Ok(uncertain(Some(status), cause)),
            },
            404 => SessionFinalizerOutcomeV2::NotFound,
            409 | 422 => SessionFinalizerOutcomeV2::RefusedNotApplied { status },
            _ => return Ok(uncertain(Some(status), rejected(status))),
        };
        if let Err(error) = self.verify_active_authority().await {
            return Ok(uncertain(
                Some(status),
                KubernetesSessionRuntimeError::Kubernetes(error),
            ));
        }
        Ok(outcome)
    }

    /// Most-recent GET of the exact instance by name, after the anchors.
    ///
    /// A 404 stays `ApiRejected(404)`: unknown, never terminal.
    pub(in crate::runtime::kubernetes) async fn observe_session_instance(
        &self,
        instance: &SessionInstanceV2,
    ) -> Outcome<SessionObservedV2> {
        let target = instance.target();
        let name = target.pod_name();
        if target.authority_fingerprint() != self.binding.fingerprint() || !valid_dns_label(name) {
            return Err(KubernetesSessionRuntimeError::Kubernetes(
                KubernetesError::InvalidAuthority,
            ));
        }
        self.verify_active_authority()
            .await
            .map_err(KubernetesSessionRuntimeError::Kubernetes)?;
        let namespace = self.binding.namespace();
        let pod: Value = self
            .transport
            .get_unique(&format!("/api/v1/namespaces/{namespace}/pods/{name}"))
            .await
            .map_err(KubernetesSessionRuntimeError::Kubernetes)?;
        parse_session_observation(&self.binding, instance, pod)
    }

    /// Local refusals before any request: another authority, a name that is
    /// not a DNS label, or a cancellation.
    fn require_session_effect_target(
        &self,
        authority_fingerprint: &str,
        pod_name: &str,
        cancelled: &AtomicBool,
    ) -> Outcome<()> {
        if authority_fingerprint != self.binding.fingerprint() || !valid_dns_label(pod_name) {
            return Err(KubernetesSessionRuntimeError::Kubernetes(
                KubernetesError::InvalidAuthority,
            ));
        }
        if cancelled.load(Ordering::Acquire) {
            return Err(REFUSED);
        }
        Ok(())
    }
}

/// A validated 200 of the release PATCH: the exact instance in its released
/// form. A still gated object or a deletion request is not a release.
fn released_resource_version(
    authority: &KubernetesAuthorityV1,
    instance: &SessionInstanceV2,
    bytes: &[u8],
) -> Outcome<String> {
    let pod = crate::json::decode_unique(bytes, 64).map_err(|_| INVALID)?;
    let observed = parse_session_observation(authority, instance, pod)?;
    match observed.state() {
        SessionObservationStateV2::ReleasedUnscheduled
        | SessionObservationStateV2::Scheduled { .. } => Ok(observed.resource_version().into()),
        SessionObservationStateV2::Held | SessionObservationStateV2::Deleting { .. } => {
            Err(INVALID)
        }
    }
}
/// The finalizer may be removed only once the container of this exact
/// instance is observed terminated, or the Pod is being deleted while still
/// gated (never released, so never scheduled or started).
fn terminal_or_never_released(observed: &SessionObservedV2) -> bool {
    match observed.state() {
        SessionObservationStateV2::Scheduled { container, .. } => {
            matches!(container, SessionContainerStateV2::Terminated { .. })
        }
        SessionObservationStateV2::Deleting { container } => {
            matches!(container, SessionContainerStateV2::Terminated { .. })
                || (observed.gated() && matches!(container, SessionContainerStateV2::Absent))
        }
        SessionObservationStateV2::Held | SessionObservationStateV2::ReleasedUnscheduled => false,
    }
}
/// A validated 200 of the finalizer PATCH: same UID, finalizer gone.
fn finalizer_released(instance: &SessionInstanceV2, bytes: &[u8]) -> Outcome<()> {
    let pod = crate::json::decode_unique(bytes, 64).map_err(|_| INVALID)?;
    let finalizers = pod.pointer("/metadata/finalizers");
    if pod.pointer("/metadata/uid").and_then(Value::as_str) != Some(instance.pod_uid())
        || finalizers.is_some_and(|value| {
            value.as_array().is_none_or(|values| {
                values
                    .iter()
                    .any(|value| value.as_str() == Some(SESSION_TERMINAL_FINALIZER))
            })
        })
    {
        return Err(INVALID);
    }
    Ok(())
}
#[cfg(test)]
fn canonical(value: &Value) -> Outcome<Vec<u8>> {
    crate::control::kubernetes_session::validation::canonical(value).map_err(|_| REFUSED)
}
fn ownership_unknown(
    http_status: Option<u16>,
    cause: KubernetesSessionRuntimeError,
) -> SessionCreateOutcomeV2 {
    SessionCreateOutcomeV2::OwnershipUnknown { http_status, cause }
}
fn rejected(status: u16) -> KubernetesSessionRuntimeError {
    KubernetesSessionRuntimeError::Kubernetes(KubernetesError::ApiRejected(status))
}

#[cfg(test)]
#[path = "session_effect_tests.rs"]
mod tests;
