//! Retention budgets of the directed relay.
//!
//! A policy bounds what a tenant, one sender and one directed grant may keep
//! retained in non-NULL database body columns. These are not filesystem byte
//! or inode quotas. A policy never widens an authorization and never selects a
//! tenant or a source: both still come from the authenticated identity.
use super::{MAX_MESSAGE_BYTES, MAX_RETAINED_BODY_BYTES, MAX_RETAINED_MESSAGES};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// One sender may hold this fraction of the tenant ceiling by default.
const DEFAULT_SENDER_DIVISOR: u64 = 8;
/// Expired bodies released by one admitted send, by default and at most.
const DEFAULT_AUTOMATIC_PRUNE_BATCH: u32 = 8;
pub const MAX_AUTOMATIC_PRUNE_BATCH: u32 = 32;
/// `correlation_id` prefix of an automatic release fact. It is followed by
/// `<operation_id>:<worker_id>`: the send operation that triggered the release
/// and the authenticated worker that sent it. This is provenance, not an
/// actor: the fact stays unattributed, and the triggering worker need not own
/// the released body. A runner prune never carries the prefix. The triggering
/// send may itself be refused and therefore absent from the audit.
pub const AUTOMATIC_PRUNE_CORRELATION_PREFIX: &str = "relay-auto-prune:";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayCapacityV1 {
    pub messages: u64,
    pub bytes: u64,
}
impl RelayCapacityV1 {
    fn fits_within(self, other: Self) -> bool {
        self.messages <= other.messages && self.bytes <= other.bytes
    }
    /// A share that cannot hold one maximum-size message could never admit it.
    fn holds_one_message(self) -> bool {
        self.messages >= 1 && self.bytes >= MAX_MESSAGE_BYTES as u64
    }
}

/// Closed operator policy. Every field is required: an omitted bound is refused
/// instead of being silently replaced by a default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayRetentionPolicyV1 {
    pub version: u32,
    /// Bodies physically retained by the whole tenant.
    pub tenant: RelayCapacityV1,
    /// Bodies one source worker may keep retained across all its grants.
    pub sender: RelayCapacityV1,
    /// Bodies one directed grant (source → destination + channel) may keep retained.
    pub grant: RelayCapacityV1,
    /// Expired bodies an admitted send may release, each with its own audit
    /// fact. Zero keeps expired bodies until the runner prunes them deliberately.
    pub automatic_prune_batch: u32,
    /// When true, the recipient's acknowledgement fact also removes the body.
    /// One-way door: a database holding such a fact is refused by binaries that
    /// predate this option.
    pub release_on_ack: bool,
}
impl Default for RelayRetentionPolicyV1 {
    fn default() -> Self {
        let sender = RelayCapacityV1 {
            messages: MAX_RETAINED_MESSAGES / DEFAULT_SENDER_DIVISOR,
            bytes: MAX_RETAINED_BODY_BYTES / DEFAULT_SENDER_DIVISOR,
        };
        Self {
            version: 1,
            tenant: RelayCapacityV1 {
                messages: MAX_RETAINED_MESSAGES,
                bytes: MAX_RETAINED_BODY_BYTES,
            },
            sender,
            // Inert until the operator tightens it below the sender share.
            grant: sender,
            automatic_prune_batch: DEFAULT_AUTOMATIC_PRUNE_BATCH,
            release_on_ack: false,
        }
    }
}
impl RelayRetentionPolicyV1 {
    /// A share is a ceiling, not a reservation. Requiring two full sender
    /// shares inside the tenant ceiling guarantees that one saturated sender
    /// alone can never exhaust the tenant.
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.version == 1,
            "unsupported relay retention policy version"
        );
        let physical = RelayCapacityV1 {
            messages: MAX_RETAINED_MESSAGES,
            bytes: MAX_RETAINED_BODY_BYTES,
        };
        anyhow::ensure!(
            self.tenant.fits_within(physical),
            "relay tenant retention exceeds the physical bound of {MAX_RETAINED_MESSAGES} messages and {MAX_RETAINED_BODY_BYTES} bytes"
        );
        anyhow::ensure!(
            self.sender.holds_one_message() && self.grant.holds_one_message(),
            "relay sender and grant shares must each hold one {MAX_MESSAGE_BYTES}-byte message"
        );
        anyhow::ensure!(
            self.grant.fits_within(self.sender),
            "relay grant share must not exceed the sender share"
        );
        let two_senders = RelayCapacityV1 {
            messages: self.sender.messages.saturating_mul(2),
            bytes: self.sender.bytes.saturating_mul(2),
        };
        anyhow::ensure!(
            two_senders.fits_within(self.tenant),
            "relay tenant retention must hold at least two full sender shares"
        );
        anyhow::ensure!(
            self.automatic_prune_batch <= MAX_AUTOMATIC_PRUNE_BATCH,
            "relay automatic prune batch exceeds {MAX_AUTOMATIC_PRUNE_BATCH}"
        );
        Ok(())
    }
    pub fn fingerprint(&self) -> anyhow::Result<String> {
        let mut digest = Sha256::new();
        digest.update(b"agentsandbox:relay-retention:v1\0");
        digest.update(serde_json::to_vec(self)?);
        Ok(hex::encode(digest.finalize()))
    }
}

/// Scopes whose figures belong to the refused sender itself. The tenant ceiling
/// is reported separately and without figures: they would describe neighbors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayCapacityScopeV1 {
    Sender,
    Grant,
}

/// Every value of the `scope` field of a relay 429. Capacity refusals name
/// `tenant`, `sender` or `grant`; a rate refusal names `grant`. Figures only
/// ever accompany `sender` and `grant` capacity refusals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayRefusalScopeV1 {
    Tenant,
    Sender,
    Grant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayCapacityDimensionV1 {
    Messages,
    Bytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayRetryActionV1 {
    /// Retry the same operation: something other than the operator can still
    /// free the refused scope. With a positive `automatic_prune_batch`, sends
    /// release its bodies once expired (`next_release_at` gives the earliest
    /// expiry in a refused sender or grant scope; a tenant refusal gives no
    /// deadline). When the policy opts in, the recipient's acknowledgement
    /// releases a body that has not expired; with a zero batch that is the
    /// only release, and this action is given for an own sender/grant scope
    /// only while it still holds a body that is neither expired nor acknowledged.
    /// A tenant refusal with a zero batch always asks for the operator.
    RetryAfterRelease,
    /// The per-minute send rate of the grant was reached: retry the same
    /// operation once the 60-second window has moved on.
    RetryAfterWindow,
    /// Ask the operator. For a sender or grant, the policy and retained bodies
    /// offer no automatic release path. At the tenant ceiling with a zero
    /// prune batch, this advice depends only on the policy.
    ContactOperator,
}

/// A sender exceeded its own retained share. Every figure describes the
/// requester's own scope after cleanup; no body and no neighbor figure is carried.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RelayCapacityRefusalV1 {
    pub scope: RelayCapacityScopeV1,
    pub dimension: RelayCapacityDimensionV1,
    pub limit: RelayCapacityV1,
    pub retained: RelayCapacityV1,
    pub requested_bytes: u64,
    /// Earliest expiry among the bodies retained in this scope.
    pub next_release_at: Option<String>,
    pub retry_action: RelayRetryActionV1,
}
impl std::fmt::Display for RelayCapacityRefusalV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self.scope {
            RelayCapacityScopeV1::Sender => "relay sender retained share reached",
            RelayCapacityScopeV1::Grant => "relay grant retained share reached",
        })
    }
}
impl std::error::Error for RelayCapacityRefusalV1 {}

/// The tenant ceiling refused a sender that is still under its own shares.
/// Only the action is carried: occupancy and deadlines would describe other
/// senders. `RetryAfterRelease` says that bodies of the tenant can still be
/// released by sends once expired, so a later retry of the same operation can
/// be admitted. When sends release nothing, `ContactOperator` is always given,
/// even if a recipient could acknowledge a live body: the advice must not
/// reveal that state about another sender.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RelayTenantCapacityRefusalV1 {
    pub retry_action: RelayRetryActionV1,
}
impl std::fmt::Display for RelayTenantCapacityRefusalV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("relay tenant retained capacity reached")
    }
}
impl std::error::Error for RelayTenantCapacityRefusalV1 {}

/// Largest number of rows returned per list of a retention status.
pub const MAX_RETENTION_STATUS_ITEMS: u32 = 100;

/// Operator view of retained relay content. It carries identifiers, sizes and
/// deadlines only: never a message body. `retained` counts bodies physically
/// present; `expired` and `acknowledged` are subsets of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayRetentionStatusV1 {
    pub version: u32,
    /// Instant at which expiry was evaluated.
    pub observed_at: String,
    pub policy: RelayRetentionPolicyV1,
    pub policy_fingerprint: String,
    pub tenant: RelayTenantRetentionV1,
    /// Senders holding at least one retained body, largest first.
    pub senders: Vec<RelaySenderRetentionV1>,
    pub senders_total: u64,
    /// Grants holding at least one retained body, largest first.
    pub grants: Vec<RelayGrantRetentionV1>,
    pub grants_total: u64,
    /// Expired bodies still retained, oldest expiry first.
    pub prunable: Vec<RelayPrunableBodyV1>,
    pub prunable_total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayTenantRetentionV1 {
    pub limit: RelayCapacityV1,
    pub retained: RelayCapacityV1,
    pub expired: RelayCapacityV1,
    pub acknowledged: RelayCapacityV1,
    pub next_expiry_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelaySenderRetentionV1 {
    pub worker_id: String,
    pub limit: RelayCapacityV1,
    pub retained: RelayCapacityV1,
    pub expired: RelayCapacityV1,
    pub next_expiry_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayGrantRetentionV1 {
    pub grant_id: String,
    pub source_worker_id: String,
    pub destination_worker_id: String,
    pub channel: String,
    pub limit: RelayCapacityV1,
    pub retained: RelayCapacityV1,
    pub expired: RelayCapacityV1,
    pub next_expiry_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayPrunableBodyV1 {
    pub message_id: String,
    pub grant_id: String,
    pub source_worker_id: String,
    pub destination_worker_id: String,
    pub body_bytes: u64,
    pub expires_at: String,
    pub acknowledged: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capacity(messages: u64, bytes: u64) -> RelayCapacityV1 {
        RelayCapacityV1 { messages, bytes }
    }

    #[test]
    fn policy_defaults_validate_and_reject_unfair_or_unbounded_shapes() {
        let shipped = RelayRetentionPolicyV1::default();
        shipped.validate().unwrap();
        assert_eq!(shipped.tenant, capacity(1024, 16 * 1024 * 1024));
        assert_eq!(shipped.sender, capacity(128, 2 * 1024 * 1024));
        assert_eq!(shipped.grant, shipped.sender);
        assert_eq!(shipped.automatic_prune_batch, 8);
        assert!(
            !shipped.release_on_ack,
            "acknowledgement must keep the body by default"
        );
        RelayRetentionPolicyV1 {
            release_on_ack: true,
            ..shipped.clone()
        }
        .validate()
        .unwrap();
        RelayRetentionPolicyV1 {
            automatic_prune_batch: 0,
            ..shipped.clone()
        }
        .validate()
        .unwrap();
        RelayRetentionPolicyV1 {
            automatic_prune_batch: 32,
            ..shipped.clone()
        }
        .validate()
        .unwrap();
        assert_eq!(shipped.fingerprint().unwrap().len(), 64);

        let small = RelayRetentionPolicyV1 {
            tenant: capacity(2, 131_072),
            sender: capacity(1, 65_536),
            grant: capacity(1, 65_536),
            ..shipped.clone()
        };
        small.validate().unwrap();
        assert_ne!(
            small.fingerprint().unwrap(),
            shipped.fingerprint().unwrap(),
            "two different policies share a fingerprint"
        );

        let refused = [
            (
                "unknown version",
                RelayRetentionPolicyV1 {
                    version: 2,
                    ..shipped.clone()
                },
            ),
            (
                "tenant messages above the physical bound",
                RelayRetentionPolicyV1 {
                    tenant: capacity(1025, 16 * 1024 * 1024),
                    ..shipped.clone()
                },
            ),
            (
                "tenant bytes above the physical bound",
                RelayRetentionPolicyV1 {
                    tenant: capacity(1024, 16 * 1024 * 1024 + 1),
                    ..shipped.clone()
                },
            ),
            (
                "one sender may exhaust the tenant message ceiling",
                RelayRetentionPolicyV1 {
                    sender: capacity(513, 2 * 1024 * 1024),
                    ..shipped.clone()
                },
            ),
            (
                "one sender may exhaust the tenant byte ceiling",
                RelayRetentionPolicyV1 {
                    sender: capacity(128, 8 * 1024 * 1024 + 1),
                    ..shipped.clone()
                },
            ),
            (
                "sender share cannot hold one message",
                RelayRetentionPolicyV1 {
                    sender: capacity(1, 65_535),
                    grant: capacity(1, 65_535),
                    ..shipped.clone()
                },
            ),
            (
                "sender share admits no message",
                RelayRetentionPolicyV1 {
                    sender: capacity(0, 65_536),
                    grant: capacity(0, 65_536),
                    ..shipped.clone()
                },
            ),
            (
                "grant share cannot hold one message",
                RelayRetentionPolicyV1 {
                    grant: capacity(1, 65_535),
                    ..shipped.clone()
                },
            ),
            (
                "grant share above the sender share",
                RelayRetentionPolicyV1 {
                    grant: capacity(129, 2 * 1024 * 1024),
                    ..shipped.clone()
                },
            ),
            (
                "unbounded cleanup inside one send",
                RelayRetentionPolicyV1 {
                    automatic_prune_batch: 33,
                    ..shipped.clone()
                },
            ),
        ];
        for (reason, policy) in refused {
            assert!(policy.validate().is_err(), "accepted policy: {reason}");
        }
    }

    #[test]
    fn refusal_wire_enums_name_every_emitted_scope_and_action() {
        use serde_json::{from_value, json, to_value};
        for (scope, wire) in [
            (RelayRefusalScopeV1::Tenant, "tenant"),
            (RelayRefusalScopeV1::Sender, "sender"),
            (RelayRefusalScopeV1::Grant, "grant"),
        ] {
            assert_eq!(to_value(scope).unwrap(), json!(wire));
            assert_eq!(
                from_value::<RelayRefusalScopeV1>(json!(wire)).unwrap(),
                scope
            );
        }
        // Scopes that carry the requester's own figures are a subset of them.
        for (own, wire) in [
            (RelayCapacityScopeV1::Sender, RelayRefusalScopeV1::Sender),
            (RelayCapacityScopeV1::Grant, RelayRefusalScopeV1::Grant),
        ] {
            assert_eq!(
                from_value::<RelayRefusalScopeV1>(to_value(own).unwrap()).unwrap(),
                wire
            );
        }
        for (action, wire) in [
            (RelayRetryActionV1::RetryAfterRelease, "retry_after_release"),
            (RelayRetryActionV1::RetryAfterWindow, "retry_after_window"),
            (RelayRetryActionV1::ContactOperator, "contact_operator"),
        ] {
            assert_eq!(to_value(action).unwrap(), json!(wire));
            assert_eq!(
                from_value::<RelayRetryActionV1>(json!(wire)).unwrap(),
                action
            );
        }
        // A tenant refusal has no field that could describe another sender.
        assert_eq!(
            to_value(RelayTenantCapacityRefusalV1 {
                retry_action: RelayRetryActionV1::RetryAfterRelease
            })
            .unwrap(),
            json!({"retry_action":"retry_after_release"})
        );
    }

    #[test]
    fn published_example_is_the_shipped_policy() {
        let example: RelayRetentionPolicyV1 = serde_json::from_str(include_str!(
            "../../docs/setup/relay/examples/relay-retention.json"
        ))
        .unwrap();
        example.validate().unwrap();
        assert_eq!(example, RelayRetentionPolicyV1::default());
    }

    #[test]
    fn policy_is_a_closed_contract_with_every_field_required() {
        let shipped = RelayRetentionPolicyV1::default();
        let encoded = serde_json::to_value(&shipped).unwrap();
        assert_eq!(
            serde_json::from_value::<RelayRetentionPolicyV1>(encoded.clone()).unwrap(),
            shipped
        );
        let mut unknown = encoded.clone();
        unknown["burst"] = serde_json::json!(1);
        assert!(serde_json::from_value::<RelayRetentionPolicyV1>(unknown).is_err());
        for field in [
            "version",
            "tenant",
            "sender",
            "grant",
            "automatic_prune_batch",
            "release_on_ack",
        ] {
            let mut missing = encoded.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(
                serde_json::from_value::<RelayRetentionPolicyV1>(missing).is_err(),
                "policy without {field} was accepted"
            );
        }
    }
}
