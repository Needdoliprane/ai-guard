//! Closed transport routing only. A matching path is never an identity proof:
//! the gateway must run its existing Rust extractor before body/operation slots.
use axum::http::Method;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlTransportFailureReasonV1 {
    UpstreamConnectionUnavailable,
    UpstreamTimeout,
    UpstreamConnectionInterrupted,
}

/// Closed wire evidence of a classified transport failure, never a blanket
/// interpretation of HTTP502/503. `unknown` does not imply rollback or retry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlTransportFailureV1 {
    pub version: u32,
    pub code: String,
    pub phase: String,
    pub outcome: String,
    pub retry_action: String,
    pub reason: ControlTransportFailureReasonV1,
}
impl ControlTransportFailureV1 {
    pub fn new(reason: ControlTransportFailureReasonV1) -> Self {
        Self {
            version: 1,
            code: "control_transport_failure".into(),
            phase: "exchange".into(),
            outcome: "unknown".into(),
            retry_action: "reconcile_same_operation".into(),
            reason,
        }
    }
    pub fn validate(&self) -> bool {
        self.version == 1
            && self.code == "control_transport_failure"
            && self.phase == "exchange"
            && self.outcome == "unknown"
            && self.retry_action == "reconcile_same_operation"
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlRouteIdentity {
    Runner,
    BrokerPresence,
}

/// Classify a gateway-root path after the relay removes exactly `/control`.
/// No decoding/normalization is performed. Dynamic IDs remain the handler's
/// responsibility, after authentication; no UUID assumption is made here.
pub fn control_route(method: &Method, path: &str) -> Option<ControlRouteIdentity> {
    if !path.starts_with('/')
        || !path.is_ascii()
        || path.bytes().any(|byte| {
            byte.is_ascii_control()
                || byte.is_ascii_whitespace()
                || matches!(byte, b'%' | b'\\' | b'?' | b'#')
        })
    {
        return None;
    }
    let (path, v1) = if let Some(path) = path.strip_prefix("/api/v1/") {
        (path, true)
    } else {
        (path.strip_prefix("/api/")?, false)
    };
    let segments: Vec<_> = path.split('/').collect();
    if segments
        .iter()
        .any(|segment| matches!(*segment, "" | "." | ".."))
    {
        return None;
    }
    use ControlRouteIdentity::{BrokerPresence, Runner};
    match (method, segments.as_slice()) {
        (&Method::GET, ["identity"])
        | (&Method::GET, ["workers", _])
        | (&Method::GET, ["workers", "operations", _])
        | (&Method::POST, ["decide"])
        | (&Method::POST, ["workers", "reservations"])
        | (&Method::POST, ["workers", _, "transitions"]) => Some(Runner),
        (&Method::GET, ["control", "protocol"])
        | (&Method::GET, ["workers", _, "arms", _])
        | (&Method::POST, ["control", "events"])
        | (&Method::POST, ["workers", "supervised-reservations"])
        | (&Method::POST, ["workers", _, "arms"])
        | (&Method::POST, ["workers", _, "armed-start-claims"])
        | (&Method::POST, ["workers", _, "helper-start-claims"])
        | (&Method::POST, ["workers", _, "helper-observations"])
        | (&Method::POST, ["workers", _, "helper-cleanup"])
        | (&Method::POST, ["workers", _, "export-requests"])
        | (&Method::POST, ["workload-bindings"])
        | (&Method::POST, ["workload-bindings", _, "revoke"])
        | (&Method::POST, ["brokers", "grants"])
        | (&Method::POST, ["brokers", _, "revoke"])
        | (&Method::POST, ["relay", "grants"])
        | (&Method::POST, ["relay", "grants", _, "revoke"])
        | (&Method::POST, ["relay", "messages", _, "prune"])
            if v1 =>
        {
            Some(Runner)
        }
        (&Method::GET, ["brokers", "identity"])
        | (&Method::GET, ["brokers", "self"])
        | (&Method::POST, ["brokers", "heartbeat"])
            if v1 =>
        {
            Some(BrokerPresence)
        }
        _ => None,
    }
}
