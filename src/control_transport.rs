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
    BrokerSession,
    WorkerRelay,
}

/// Exact reserved owned policy methods; classification alone grants no authority.
pub(crate) fn owned_policy_route(method: &Method, path: &str) -> bool {
    *method == Method::POST
        && matches!(
            path,
            "/api/v1/egress/policies/replace" | "/api/v1/egress/policies/revoke"
        )
}

/// Exact technical read; it is never a policy writer classifier.
pub(crate) fn owned_policy_inspection_route(method: &Method, path: &str) -> bool {
    *method == Method::POST && path == "/api/v1/egress/policies/inspect"
}

pub(crate) fn owned_protocol_route(method: &Method, path: &str) -> bool {
    *method == Method::GET && path == "/api/v1/control/protocol"
}

/// The only query accepted by the control transport. Values are parsed without
/// URL decoding; forwarding reconstructs this object rather than its input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlQuery {
    None,
    Inbox {
        after: Option<i64>,
        limit: Option<u32>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidControlQuery;

impl ControlQuery {
    pub fn encoded(self) -> Option<String> {
        match self {
            Self::None => None,
            Self::Inbox { after, limit } => match (after, limit) {
                (None, None) => None,
                (Some(after), None) => Some(format!("after={after}")),
                (None, Some(limit)) => Some(format!("limit={limit}")),
                (Some(after), Some(limit)) => Some(format!("after={after}&limit={limit}")),
            },
        }
    }
}

/// Canonical unsigned decimal syntax, bounded by the destination numeric type.
fn decimal(value: &str) -> Option<u64> {
    if value.is_empty()
        || value.len() > 1 && value.starts_with('0')
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    value.parse().ok()
}

pub fn control_query(
    method: &Method,
    path: &str,
    query: Option<&str>,
) -> Result<ControlQuery, InvalidControlQuery> {
    if *method != Method::GET || path != "/api/v1/relay/messages" {
        return query.map_or(Ok(ControlQuery::None), |_| Err(InvalidControlQuery));
    }
    let mut after = None;
    let mut limit = None;
    if let Some(query) = query {
        for field in query.split('&') {
            let (key, value) = field.split_once('=').ok_or(InvalidControlQuery)?;
            let value = decimal(value).ok_or(InvalidControlQuery)?;
            match key {
                "after" if after.is_none() => {
                    after = Some(i64::try_from(value).map_err(|_| InvalidControlQuery)?);
                }
                "limit" if limit.is_none() && (1..=32).contains(&value) => {
                    limit = Some(value as u32);
                }
                _ => return Err(InvalidControlQuery),
            }
        }
    }
    Ok(ControlQuery::Inbox { after, limit })
}

fn canonical_uuid(value: &str) -> bool {
    crate::control::uuid(value).is_ok()
}

/// Classify a gateway-root path after the relay removes exactly `/control`.
/// No decoding/normalization is performed. Historical dynamic IDs remain the
/// handler's responsibility; worker ACK and session recovery paths require a
/// canonical UUID. Session recovery additionally refuses nil in its handler.
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
    use ControlRouteIdentity::{BrokerPresence, BrokerSession, Runner, WorkerRelay};
    match (method, segments.as_slice()) {
        (&Method::GET, ["identity"])
        | (&Method::GET, ["workers", _])
        | (&Method::GET, ["workers", "operations", _])
        | (&Method::POST, ["decide"])
        | (&Method::POST, ["workers", "reservations"])
        | (&Method::POST, ["workers", _, "transitions"]) => Some(Runner),
        (&Method::GET, ["control", "protocol"])
        | (&Method::GET, ["control", "capabilities"])
        | (&Method::GET, ["workers", _, "arms", _])
        | (&Method::POST, ["control", "events"])
        | (&Method::POST, ["egress", "policies", "replace"])
        | (&Method::POST, ["egress", "policies", "revoke"])
        | (&Method::POST, ["egress", "policies", "inspect"])
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
        | (&Method::POST, ["relay", "channels", "admissions"])
        | (&Method::POST, ["relay", "channels", _, "claims"])
        | (&Method::POST, ["relay", "channels", _, "transitions"])
            if v1 =>
        {
            Some(Runner)
        }
        (&Method::POST, ["kubernetes", "session-reservations", id, "control-recovery"])
            if v1 && canonical_uuid(id) =>
        {
            Some(Runner)
        }
        (&Method::POST, ["relay", "channels", id, "reconcile"])
            if v1
                && uuid::Uuid::parse_str(id)
                    .is_ok_and(|uuid| !uuid.is_nil() && uuid.to_string() == *id) =>
        {
            Some(Runner)
        }
        (&Method::GET, ["brokers", "identity"])
        | (&Method::GET, ["brokers", "self"])
        | (&Method::POST, ["brokers", "heartbeat"])
        | (&Method::POST, ["brokers", "checkpoints"])
        | (&Method::GET, ["brokers", "checkpoints", "self"])
            if v1 =>
        {
            Some(BrokerPresence)
        }
        (&Method::GET, ["control", "session-confirmations", id])
        | (&Method::GET, ["control", "session-confirmations", id, "launch-spec"])
            if v1 && canonical_uuid(id) =>
        {
            Some(BrokerSession)
        }
        (&Method::POST, ["control", "session-confirmations", id, "initial-decision"])
        | (&Method::POST, ["control", "session-confirmations", id, "admission"])
        | (
            &Method::POST,
            [
                "control",
                "session-confirmations",
                id,
                "admission",
                "reconcile" | "cancel-reserved" | "import-arm",
            ],
        )
        | (
            &Method::POST,
            [
                "control",
                "session-confirmations",
                id,
                "admission",
                "import-arm",
                "reconcile",
            ],
        ) if v1 && canonical_uuid(id) => Some(BrokerSession),
        (
            &Method::POST,
            [
                "control",
                "session-confirmations",
                id,
                "admission",
                "import-claim" | "import-observation",
            ],
        )
        | (
            &Method::POST,
            [
                "control",
                "session-confirmations",
                id,
                "admission",
                "import-claim" | "import-observation" | "import-verification-arm",
                "reconcile",
            ],
        ) if v1
            && crate::control::session_control::canonical_uuid(id)
                .is_ok_and(|binding| !binding.is_nil()) =>
        {
            Some(BrokerSession)
        }
        // Proposed owned broker-supervision targets (`OwnedBrokerOperationV2`).
        // Classified for relay forwarding and presence capacity only: the
        // gateway gate authenticates the owned proof, and a legacy gateway
        // refuses these targets before any extractor or body read.
        (&Method::GET, ["owned", "brokers", "identity"])
        | (&Method::GET, ["owned", "brokers", "self"])
        | (&Method::POST, ["owned", "brokers", "heartbeats"])
        | (&Method::POST, ["owned", "brokers", "heartbeats", "reconcile"])
        | (&Method::POST, ["owned", "brokers", "checkpoints"])
        | (&Method::POST, ["owned", "brokers", "checkpoints", "reconcile"])
        | (&Method::GET, ["owned", "brokers", "checkpoints", "self"])
            if v1 =>
        {
            Some(BrokerPresence)
        }
        (&Method::POST | &Method::GET, ["relay", "messages"]) if v1 => Some(WorkerRelay),
        (&Method::POST, ["relay", "messages", id, "ack"]) if v1 && canonical_uuid(id) => {
            Some(WorkerRelay)
        }
        _ => None,
    }
}

#[cfg(test)]
mod reconciliation_tests {
    use super::*;

    #[test]
    fn named_import_claim_routes_require_broker_session_and_exact_scope_path() {
        assert_named_import_routes_require_broker_session("import-claim");
    }

    #[test]
    fn named_import_observation_routes_require_broker_session_and_exact_scope_path() {
        assert_named_import_routes_require_broker_session("import-observation");
    }

    #[test]
    fn named_import_verification_arm_allows_only_exact_broker_reconciliation() {
        let binding = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
        let base = format!(
            "/api/v1/control/session-confirmations/{binding}/admission/import-verification-arm"
        );
        let reconcile = format!("{base}/reconcile");
        assert_eq!(
            control_route(&Method::POST, &reconcile),
            Some(ControlRouteIdentity::BrokerSession)
        );
        assert_eq!(control_route(&Method::GET, &reconcile), None);
        assert_eq!(
            control_query(&Method::POST, &reconcile, Some("tenant=tenant-a")),
            Err(InvalidControlQuery)
        );
        for invalid in [
            base,
            reconcile.replacen("/api/v1/", "/api/", 1),
            reconcile.replace(binding, "00000000-0000-0000-0000-000000000000"),
            reconcile.replace(binding, &binding.to_uppercase()),
            format!("{reconcile}/extra"),
        ] {
            assert_eq!(control_route(&Method::POST, &invalid), None);
        }
    }

    fn assert_named_import_routes_require_broker_session(operation: &str) {
        let binding = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
        let base = format!("/api/v1/control/session-confirmations/{binding}/admission/{operation}");
        for path in [base.clone(), format!("{base}/reconcile")] {
            assert_eq!(
                control_route(&Method::POST, &path),
                Some(ControlRouteIdentity::BrokerSession)
            );
            assert_eq!(control_route(&Method::GET, &path), None);
            assert_eq!(
                control_query(&Method::POST, &path, None),
                Ok(ControlQuery::None)
            );
            assert_eq!(
                control_query(&Method::POST, &path, Some("tenant=tenant-a")),
                Err(InvalidControlQuery)
            );
            for bad in [
                path.replacen("/api/v1/", "/api/v2/", 1),
                path.replace(binding, "00000000-0000-0000-0000-000000000000"),
                path.replace(binding, &binding.to_uppercase()),
                format!("{path}/extra"),
            ] {
                assert_eq!(control_route(&Method::POST, &bad), None, "accepted {bad}");
            }
        }
    }

    #[test]
    fn reconciliation_route_accepts_only_exact_runner_v1_without_query() {
        let path = "/api/v1/relay/channels/aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa/reconcile";
        assert_eq!(
            control_route(&Method::POST, path),
            Some(ControlRouteIdentity::Runner)
        );
        for method in [Method::GET, Method::PUT, Method::DELETE] {
            assert_eq!(control_route(&method, path), None);
        }
        for path in [
            "/api/relay/channels/aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa/reconcile",
            "/api/v1/relay/channels/00000000-0000-0000-0000-000000000000/reconcile",
            "/api/v1/relay/channels/AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA/reconcile",
            "/api/v1/relay/channels/free/reconcile",
            "/api/v1/relay/channels/aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa/reconcile/extra",
        ] {
            assert_eq!(control_route(&Method::POST, path), None);
        }
        assert_eq!(
            control_query(&Method::POST, path, None),
            Ok(ControlQuery::None)
        );
        for query in ["", "tenant=tenant-a", "after=0"] {
            assert_eq!(
                control_query(&Method::POST, path, Some(query)),
                Err(InvalidControlQuery)
            );
        }
    }
}
