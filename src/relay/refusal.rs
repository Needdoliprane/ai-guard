//! What a refused relay send tells the worker that sent it.
//!
//! The gateway answers a send beyond a retained share, or beyond the rate of
//! its grant, with a typed HTTP 429. This module reads that bounded error body
//! into a closed value for the worker client. It never widens the answer:
//!
//! - a body that is not exactly one published relay refusal yields nothing,
//!   and the caller keeps the plain `Capacity` error;
//! - free text is never carried over: only closed enumerations, the sender's
//!   own figures and a canonical timestamp, none of which can hold a
//!   credential or a message body;
//! - a tenant or rate refusal that came with figures is not accepted, because
//!   figures at those scopes would describe other senders.
use super::client::RelayClientError;
use super::{
    RelayCapacityDimensionV1, RelayCapacityScopeV1, RelayCapacityV1, RelayRefusalScopeV1,
    RelayRetryActionV1,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

/// The refused sender's own figures in its `sender` or `grant` scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayOwnShareV1 {
    pub dimension: RelayCapacityDimensionV1,
    pub limit: RelayCapacityV1,
    pub retained: RelayCapacityV1,
    pub requested_bytes: u64,
    /// Earliest expiry among the sender's own bodies retained in that scope,
    /// in canonical millisecond UTC. An expiry is a release time only when
    /// sends release expired bodies.
    pub next_release_at: Option<String>,
}

/// One published relay refusal of a send.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelaySendRefusalV1 {
    pub scope: RelayRefusalScopeV1,
    pub retry_action: RelayRetryActionV1,
    /// Present for a `sender` or `grant` capacity refusal only. A tenant or
    /// rate refusal never carries a figure.
    pub own: Option<RelayOwnShareV1>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope<T> {
    error: T,
}
/// `relay_capacity_exhausted` and `relay_rate_limited`: scope and action only.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScopeAndAction {
    code: String,
    message: String,
    scope: RelayRefusalScopeV1,
    retry_action: RelayRetryActionV1,
}
/// `relay_sender_capacity_exhausted` and `relay_grant_capacity_exhausted`.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnScope {
    code: String,
    message: String,
    scope: RelayCapacityScopeV1,
    dimension: RelayCapacityDimensionV1,
    limit: RelayCapacityV1,
    retained: RelayCapacityV1,
    requested_bytes: u64,
    next_release_at: Option<String>,
    retry_action: RelayRetryActionV1,
}

/// Closed decoding: no duplicate key, no unknown or missing field, and an
/// absent deadline is an explicit `null`.
fn closed<T: Serialize + DeserializeOwned>(value: &serde_json::Value) -> Option<T> {
    let decoded: Envelope<T> = serde_json::from_value(value.clone()).ok()?;
    (serde_json::to_value(&decoded).ok()? == *value).then_some(decoded.error)
}
fn canonical_deadline(value: &str) -> bool {
    chrono::DateTime::parse_from_rfc3339(value).is_ok_and(|time| {
        time.with_timezone(&chrono::Utc)
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
            == value
    })
}

impl RelaySendRefusalV1 {
    /// The same closed combinations apply to HTTP and opted-in send frames.
    /// Public fields are not authority: validate again at each wire boundary.
    pub(crate) fn valid(&self) -> bool {
        use RelayRefusalScopeV1::{Grant, Sender, Tenant};
        use RelayRetryActionV1::{ContactOperator, RetryAfterRelease, RetryAfterWindow};
        match (self.scope, self.retry_action, &self.own) {
            (Sender | Grant, RetryAfterRelease | ContactOperator, Some(own)) => own
                .next_release_at
                .as_deref()
                .is_none_or(canonical_deadline),
            (Tenant, RetryAfterRelease | ContactOperator, None)
            | (Grant, RetryAfterWindow, None) => true,
            _ => false,
        }
    }

    /// Read the error body of an HTTP 429 answered to a relay send. `None`
    /// means the body is not a published relay refusal, for instance an audit
    /// budget or I/O admission refusal: nothing is inferred from it.
    pub fn from_error_body(body: &[u8]) -> Option<Self> {
        use RelayRetryActionV1::{ContactOperator, RetryAfterRelease, RetryAfterWindow};
        let value = crate::json::decode_unique(body, 8).ok()?;
        let code = value.get("error")?.get("code")?.as_str()?;
        match code {
            "relay_sender_capacity_exhausted" | "relay_grant_capacity_exhausted" => {
                let own: OwnScope = closed(&value)?;
                let scope = match (code, own.scope) {
                    ("relay_sender_capacity_exhausted", RelayCapacityScopeV1::Sender) => {
                        RelayRefusalScopeV1::Sender
                    }
                    ("relay_grant_capacity_exhausted", RelayCapacityScopeV1::Grant) => {
                        RelayRefusalScopeV1::Grant
                    }
                    _ => return None,
                };
                let deadline_valid = own
                    .next_release_at
                    .as_deref()
                    .is_none_or(canonical_deadline);
                (deadline_valid && matches!(own.retry_action, RetryAfterRelease | ContactOperator))
                    .then_some(Self {
                        scope,
                        retry_action: own.retry_action,
                        own: Some(RelayOwnShareV1 {
                            dimension: own.dimension,
                            limit: own.limit,
                            retained: own.retained,
                            requested_bytes: own.requested_bytes,
                            next_release_at: own.next_release_at,
                        }),
                    })
            }
            "relay_capacity_exhausted" | "relay_rate_limited" => {
                let bare: ScopeAndAction = closed(&value)?;
                let published = matches!(
                    (code, bare.scope, bare.retry_action),
                    (
                        "relay_capacity_exhausted",
                        RelayRefusalScopeV1::Tenant,
                        RetryAfterRelease | ContactOperator,
                    ) | (
                        "relay_rate_limited",
                        RelayRefusalScopeV1::Grant,
                        RetryAfterWindow
                    )
                );
                published.then_some(Self {
                    scope: bare.scope,
                    retry_action: bare.retry_action,
                    own: None,
                })
            }
            _ => None,
        }
    }
}

/// Failure of `WorkerRelayClient::send_with_refusal`. `error` is exactly what
/// `send` reports for the same response. `refusal` is what the relay said
/// about it, when the response was a published relay refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelaySendFailureV1 {
    pub error: RelayClientError,
    pub refusal: Option<RelaySendRefusalV1>,
}
impl From<RelayClientError> for RelaySendFailureV1 {
    fn from(error: RelayClientError) -> Self {
        Self {
            error,
            refusal: None,
        }
    }
}
impl RelaySendFailureV1 {
    /// `error` is the client's own reading of the status and `body` the
    /// bounded response body. Only a capacity error can carry a refusal.
    pub(super) fn refused(error: RelayClientError, body: &[u8]) -> Self {
        let refusal = (error == RelayClientError::Capacity)
            .then(|| RelaySendRefusalV1::from_error_body(body))
            .flatten();
        Self { error, refusal }
    }
}
impl std::fmt::Display for RelaySendFailureV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.refusal.as_ref().map(|refusal| refusal.retry_action) {
            Some(RelayRetryActionV1::RetryAfterRelease) => f.write_str(
                "relay retained capacity refused the send; retain it and retry the same operation once retained bodies are released",
            ),
            Some(RelayRetryActionV1::RetryAfterWindow) => f.write_str(
                "relay grant rate refused the send; retain it and retry the same operation once the 60-second window has moved on",
            ),
            Some(RelayRetryActionV1::ContactOperator) => f.write_str(
                "relay retained capacity refused the send; retain it and contact the operator",
            ),
            None => self.error.fmt(f),
        }
    }
}
impl std::error::Error for RelaySendFailureV1 {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn read(body: serde_json::Value) -> Option<RelaySendRefusalV1> {
        RelaySendRefusalV1::from_error_body(body.to_string().as_bytes())
    }
    fn own_scope(code: &str, scope: &str) -> serde_json::Value {
        json!({"error":{
            "code":code,"message":"free text is never surfaced",
            "scope":scope,"dimension":"bytes",
            "limit":{"messages":128,"bytes":2_097_152},
            "retained":{"messages":32,"bytes":2_097_152},
            "requested_bytes":65_536,
            "next_release_at":"2026-11-10T12:00:42.000Z",
            "retry_action":"retry_after_release"
        }})
    }
    fn bare(code: &str, scope: &str, action: &str) -> serde_json::Value {
        json!({"error":{"code":code,"message":"m","scope":scope,"retry_action":action}})
    }

    #[test]
    fn published_refusals_are_read_into_closed_values() {
        let sender = read(own_scope("relay_sender_capacity_exhausted", "sender")).unwrap();
        assert_eq!(sender.scope, RelayRefusalScopeV1::Sender);
        assert_eq!(sender.retry_action, RelayRetryActionV1::RetryAfterRelease);
        assert_eq!(
            sender.own,
            Some(RelayOwnShareV1 {
                dimension: RelayCapacityDimensionV1::Bytes,
                limit: RelayCapacityV1 {
                    messages: 128,
                    bytes: 2_097_152
                },
                retained: RelayCapacityV1 {
                    messages: 32,
                    bytes: 2_097_152
                },
                requested_bytes: 65_536,
                next_release_at: Some("2026-11-10T12:00:42.000Z".into()),
            })
        );
        let mut grant = own_scope("relay_grant_capacity_exhausted", "grant");
        grant["error"]["next_release_at"] = json!(null);
        grant["error"]["retry_action"] = json!("contact_operator");
        let grant = read(grant).unwrap();
        assert_eq!(grant.scope, RelayRefusalScopeV1::Grant);
        assert_eq!(grant.retry_action, RelayRetryActionV1::ContactOperator);
        assert_eq!(grant.own.unwrap().next_release_at, None);

        for action in [
            RelayRetryActionV1::RetryAfterRelease,
            RelayRetryActionV1::ContactOperator,
        ] {
            let wire = serde_json::to_value(action).unwrap();
            assert_eq!(
                read(bare(
                    "relay_capacity_exhausted",
                    "tenant",
                    wire.as_str().unwrap()
                )),
                Some(RelaySendRefusalV1 {
                    scope: RelayRefusalScopeV1::Tenant,
                    retry_action: action,
                    own: None
                })
            );
        }
        assert_eq!(
            read(bare("relay_rate_limited", "grant", "retry_after_window")),
            Some(RelaySendRefusalV1 {
                scope: RelayRefusalScopeV1::Grant,
                retry_action: RelayRetryActionV1::RetryAfterWindow,
                own: None
            })
        );
    }

    #[test]
    fn anything_else_yields_no_refusal_and_never_a_neighbor_figure() {
        let sender = || own_scope("relay_sender_capacity_exhausted", "sender");
        let edited = |edit: &dyn Fn(&mut serde_json::Value)| {
            let mut body = sender();
            edit(&mut body);
            body
        };
        let refused: Vec<(&str, serde_json::Value)> = vec![
            // Other 429 of the same route: the worker learns nothing typed.
            (
                "I/O admission gate",
                json!({"error":{"code":"capacity_exhausted","message":"m","retry_action":"retry"}}),
            ),
            (
                "workload audit budget",
                json!({"error":{"code":"audit_workload_budget_exhausted","message":"m","retry_action":"contact_operator"}}),
            ),
            ("untyped error", json!({"error":"Audit indisponible"})),
            // A tenant or rate refusal with figures would describe neighbors.
            ("tenant refusal with figures", {
                let mut body = sender();
                body["error"]["code"] = json!("relay_capacity_exhausted");
                body["error"]["scope"] = json!("tenant");
                body
            }),
            ("tenant refusal with one extra field", {
                let mut body = bare("relay_capacity_exhausted", "tenant", "retry_after_release");
                body["error"]["next_release_at"] = json!("2026-11-10T12:00:42.000Z");
                body
            }),
            (
                "tenant code at another scope",
                bare("relay_capacity_exhausted", "sender", "retry_after_release"),
            ),
            (
                "rate refusal with a capacity action",
                bare("relay_rate_limited", "grant", "retry_after_release"),
            ),
            (
                "capacity refusal with the rate action",
                bare("relay_capacity_exhausted", "tenant", "retry_after_window"),
            ),
            (
                "sender code at the grant scope",
                own_scope("relay_sender_capacity_exhausted", "grant"),
            ),
            (
                "unknown action",
                edited(&|body| body["error"]["retry_action"] = json!("retry")),
            ),
            (
                "own refusal with the rate action",
                edited(&|body| body["error"]["retry_action"] = json!("retry_after_window")),
            ),
            (
                "unknown field",
                edited(&|body| body["error"]["worker_id"] = json!("neighbor")),
            ),
            (
                "missing deadline instead of an explicit null",
                edited(&|body| {
                    body["error"]
                        .as_object_mut()
                        .unwrap()
                        .remove("next_release_at");
                }),
            ),
            (
                "deadline that is not a canonical timestamp",
                edited(&|body| body["error"]["next_release_at"] = json!("2026-11-10T12:00:42Z")),
            ),
            (
                "deadline carrying free text",
                edited(&|body| body["error"]["next_release_at"] = json!("1".repeat(48))),
            ),
            (
                "field beside the error object",
                edited(&|body| body["neighbor"] = json!(1)),
            ),
            (
                "negative figure",
                edited(&|body| body["error"]["retained"]["bytes"] = json!(-1)),
            ),
        ];
        for (reason, body) in refused {
            assert_eq!(read(body), None, "accepted: {reason}");
        }
        let raw: [&[u8]; 4] = [
            b"",
            b"not json",
            b"[]",
            // A duplicated key is ambiguous: refused before any typed reading.
            br#"{"error":{"code":"relay_rate_limited","code":"relay_rate_limited","message":"m","scope":"grant","retry_action":"retry_after_window"}}"#,
        ];
        for body in raw {
            assert_eq!(RelaySendRefusalV1::from_error_body(body), None);
        }
    }

    #[test]
    fn a_failure_keeps_the_client_error_and_reads_a_refusal_only_from_a_capacity_answer() {
        let body = bare("relay_rate_limited", "grant", "retry_after_window").to_string();
        let failure = RelaySendFailureV1::refused(RelayClientError::Capacity, body.as_bytes());
        assert_eq!(failure.error, RelayClientError::Capacity);
        assert_eq!(
            failure.refusal.as_ref().map(|refusal| refusal.retry_action),
            Some(RelayRetryActionV1::RetryAfterWindow)
        );
        assert!(failure.to_string().contains("60-second window"));
        // The same body under another status is not a relay refusal.
        for error in [
            RelayClientError::Forbidden,
            RelayClientError::Unavailable,
            RelayClientError::Protocol,
        ] {
            let failure = RelaySendFailureV1::refused(error, body.as_bytes());
            assert_eq!(failure, RelaySendFailureV1::from(error));
            assert_eq!(failure.to_string(), error.to_string());
        }
        // An unread 429 keeps the historical text of the client error.
        let unread = RelaySendFailureV1::refused(RelayClientError::Capacity, b"{}");
        assert_eq!(unread.refusal, None);
        assert_eq!(unread.to_string(), RelayClientError::Capacity.to_string());
        let contact = RelaySendFailureV1::refused(
            RelayClientError::Capacity,
            bare("relay_capacity_exhausted", "tenant", "contact_operator")
                .to_string()
                .as_bytes(),
        );
        assert!(contact.to_string().contains("contact the operator"));
        let retry = RelaySendFailureV1::refused(
            RelayClientError::Capacity,
            bare("relay_capacity_exhausted", "tenant", "retry_after_release")
                .to_string()
                .as_bytes(),
        );
        assert!(!retry.to_string().contains("operator"));
    }
}
