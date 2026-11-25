//! Broker ↔ gateway egress contracts, shared by every storage backend.
//!
//! The gateway selects the tenant from the broker's authenticated runner
//! credential; no field here names or chooses a tenant. A decision is durable
//! in the tenant journal before the gateway answers, and the broker connects
//! only after an `allow` receipt. Values echoed by the worker never reach these
//! DTOs except the canonical `name:port` authority it requested.

use crate::egress::{
    Authority,
    ids::{DigestV1, IdV1},
};
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{MapAccess, Visitor, value::MapAccessDeserializer},
};
use std::{fmt, marker::PhantomData};

/// Correlation of one broker-owned channel. Built from the broker's retained
/// runtime context, never from worker frames.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EgressBindingV1 {
    pub session_id: String,
    pub worker_id: String,
    pub attempt_id: String,
    pub channel_id: IdV1,
    pub epoch: u64,
    pub context_digest: DigestV1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EgressDecideRequestV1 {
    pub version: u32,
    /// Broker-assigned idempotency key: same id + same content = same receipt.
    pub decision_id: IdV1,
    pub binding: EgressBindingV1,
    pub flow_id: u64,
    /// Canonical `name:port` already validated by the broker.
    pub authority: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EgressOutcomeV1 {
    Allow,
    Deny,
    Review,
}

/// Closed decision reasons; free text never travels to the broker.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EgressDecisionReasonV1 {
    AllowedByPolicy,
    AllowedByGrant,
    NotInPolicy,
    RequiresApproval,
    PolicyInactive,
    WorkerNotRunning,
    EgressNotLaunched,
    BindingMismatch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EgressPolicySourceV1 {
    /// No policy configured: every destination is refused.
    DefaultDeny,
    OperatorFile,
    Store15,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EgressPolicyRefV1 {
    pub source: EgressPolicySourceV1,
    pub revision: u64,
    pub digest: DigestV1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EgressReceiptV1 {
    pub audit_seq: i64,
    pub audit_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EgressDecisionV1 {
    pub version: u32,
    pub decision_id: IdV1,
    pub outcome: EgressOutcomeV1,
    pub reason: EgressDecisionReasonV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<EgressPolicyRefV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_id: Option<IdV1>,
    /// Validity left when the gateway decided. The broker bounds a live flow
    /// by this duration counted from before it sent the request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining_ms: Option<u64>,
    pub receipt: EgressReceiptV1,
}

impl EgressDecisionV1 {
    /// Only a well-formed allow with a bounded validity authorizes a connect.
    pub fn allows(&self, request: &EgressDecideRequestV1) -> bool {
        self.version == 1
            && self.decision_id == request.decision_id
            && self.outcome == EgressOutcomeV1::Allow
            && matches!(
                self.reason,
                EgressDecisionReasonV1::AllowedByPolicy | EgressDecisionReasonV1::AllowedByGrant
            )
            && self.policy.is_some()
            && self.remaining_ms.is_some_and(|ms| ms > 0)
            && self.receipt.audit_seq > 0
    }
}

/// Why a live or pending flow ended, as observed by the broker.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EgressCloseReasonV1 {
    EndOfStream,
    WorkerReset,
    UpstreamError,
    Revoked,
    Expired,
    LeaseLost,
    IdleTimeout,
    OwnerExit,
    Protocol,
}

/// Facts the broker observed itself. Volumes and durations exist only when the
/// broker measured them on its own sockets.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EgressObservationV1 {
    ChannelOpened {
        binding: EgressBindingV1,
    },
    ChannelClosed {
        binding: EgressBindingV1,
        reason: String,
        observations_lost: u64,
    },
    Connection {
        binding: EgressBindingV1,
        decision_id: IdV1,
        flow_id: u64,
        connected: bool,
        /// Closed cause when not connected (resolution_timeout, refused_address, ...).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        failure: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        peer: Option<String>,
        resolver: String,
    },
    Closed {
        binding: EgressBindingV1,
        decision_id: IdV1,
        flow_id: u64,
        reason: EgressCloseReasonV1,
        bytes_up: u64,
        bytes_down: u64,
        duration_ms: u64,
    },
    DirectAttempts {
        binding: EgressBindingV1,
        ipv4_no_route: u64,
        ipv6_no_route: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EgressObserveRequestV1 {
    pub version: u32,
    /// Idempotency key of this observation.
    pub observation_id: IdV1,
    pub observation: EgressObservationV1,
}

/// One broker-owned ChannelOpened observation, with no other event variants.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EgressChannelOpeningRequestV1 {
    pub version: u32,
    pub observation_id: IdV1,
    #[serde(deserialize_with = "deserialize_object")]
    pub binding: EgressBindingV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EgressChannelOpeningDispositionV1 {
    Fresh,
    Replayed,
}

/// Disposition of this exchange, with the original durable audit receipt.
/// Replaying an opening never establishes fresh authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EgressChannelOpeningReceiptV1 {
    pub version: u32,
    pub observation_id: IdV1,
    #[serde(deserialize_with = "deserialize_object")]
    pub binding: EgressBindingV1,
    pub disposition: EgressChannelOpeningDispositionV1,
    #[serde(deserialize_with = "deserialize_object")]
    pub receipt: EgressReceiptV1,
}

// A derived struct also accepts positional JSON arrays. In the opening wire
// contract, each object must carry its own named fields.
pub(crate) fn deserialize_object<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Object<T>(PhantomData<T>);

    impl<'de, T: Deserialize<'de>> Visitor<'de> for Object<T> {
        type Value = T;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("an object with named fields")
        }

        fn visit_map<M: MapAccess<'de>>(self, map: M) -> Result<T, M::Error> {
            T::deserialize(MapAccessDeserializer::new(map))
        }
    }

    deserializer.deserialize_map(Object(PhantomData))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EgressRenewRequestV1 {
    pub version: u32,
    pub binding: EgressBindingV1,
    /// Decisions of the channel's live flows, at most 16.
    pub decisions: Vec<IdV1>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EgressRenewResponseV1 {
    pub version: u32,
    /// Decisions whose grant or policy was revoked: close their flows now.
    pub revoked: Vec<IdV1>,
    /// Decisions whose validity ended: close their flows now.
    pub expired: Vec<IdV1>,
}

/// Upper bound of a policy-backed flow's validity; the lease still applies.
pub const MAX_POLICY_VALIDITY_MS: u64 = 24 * 3600 * 1000;
/// Bounds of the operator policy and of grants.
pub const MAX_POLICY_DESTINATIONS: usize = 128;
pub const MAX_GRANT_SECONDS: u32 = 3600;
pub const DEFAULT_GRANT_SECONDS: u32 = 300;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EgressGrantableV1 {
    /// Unknown destinations are denied; no request can be approved.
    None,
    /// Only `grantable_destinations` can be approved.
    Listed,
    /// Any public DNS name can be approved by an authorized human.
    PublicNames,
}

/// Operator-declared policy of one tenant (legacy mode), bound by digest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EgressTenantPolicyV1 {
    pub version: u32,
    pub revision: u64,
    /// Exact `name:port` destinations allowed for every session.
    pub allow: Vec<String>,
    pub grantable: EgressGrantableV1,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub grantable_destinations: Vec<String>,
    pub max_grant_seconds: u32,
}

/// A validated, immutable tenant policy: the ceiling of every decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EgressPolicySnapshot {
    pub source: EgressPolicySourceV1,
    pub revision: u64,
    pub digest: DigestV1,
    allow: Vec<Authority>,
    grantable: EgressGrantableV1,
    grantable_destinations: Vec<Authority>,
    pub max_grant_seconds: u32,
}

fn authorities(raw: &[String]) -> anyhow::Result<Vec<Authority>> {
    anyhow::ensure!(
        raw.len() <= MAX_POLICY_DESTINATIONS,
        "too many egress destinations"
    );
    let mut parsed = raw
        .iter()
        .map(|value| {
            Authority::parse_connect(value)
                .map_err(|_| anyhow::anyhow!("invalid egress destination"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    parsed.sort_unstable();
    anyhow::ensure!(
        parsed.windows(2).all(|pair| pair[0] != pair[1]),
        "duplicate egress destination"
    );
    Ok(parsed)
}

impl EgressTenantPolicyV1 {
    pub fn snapshot(&self) -> anyhow::Result<EgressPolicySnapshot> {
        anyhow::ensure!(
            self.version == 1 && self.revision > 0,
            "unsupported egress policy"
        );
        anyhow::ensure!(
            (1..=MAX_GRANT_SECONDS).contains(&self.max_grant_seconds),
            "egress max_grant_seconds must be 1..=3600"
        );
        anyhow::ensure!(
            self.grantable == EgressGrantableV1::Listed || self.grantable_destinations.is_empty(),
            "grantable_destinations requires grantable=listed"
        );
        let allow = authorities(&self.allow)?;
        let grantable_destinations = authorities(&self.grantable_destinations)?;
        Ok(EgressPolicySnapshot {
            source: EgressPolicySourceV1::OperatorFile,
            revision: self.revision,
            digest: DigestV1::of(&serde_json::to_vec(self)?),
            allow,
            grantable: self.grantable,
            grantable_destinations,
            max_grant_seconds: self.max_grant_seconds,
        })
    }
}

impl EgressPolicySnapshot {
    /// The policy of a tenant without egress configuration.
    pub fn default_deny() -> Self {
        Self {
            source: EgressPolicySourceV1::DefaultDeny,
            revision: 0,
            digest: DigestV1::of(b"ai-guard:egress-policy:default-deny:v1"),
            allow: Vec::new(),
            grantable: EgressGrantableV1::None,
            grantable_destinations: Vec::new(),
            max_grant_seconds: DEFAULT_GRANT_SECONDS,
        }
    }
    pub fn reference(&self) -> EgressPolicyRefV1 {
        EgressPolicyRefV1 {
            source: self.source,
            revision: self.revision,
            digest: self.digest.clone(),
        }
    }
    pub fn allows(&self, authority: &Authority) -> bool {
        self.allow.binary_search(authority).is_ok()
    }
    pub fn grantable(&self, authority: &Authority) -> bool {
        match self.grantable {
            EgressGrantableV1::None => false,
            EgressGrantableV1::Listed => {
                self.grantable_destinations.binary_search(authority).is_ok()
            }
            EgressGrantableV1::PublicNames => true,
        }
    }
    pub fn allowed_destinations(&self) -> impl Iterator<Item = &Authority> {
        self.allow.iter()
    }
}

/// A grant as replayed from the tenant journal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EgressGrantV1 {
    pub version: u32,
    pub grant_id: IdV1,
    pub session_id: String,
    pub authority: String,
    pub not_before_ms: i64,
    pub expires_at_ms: i64,
    /// Authenticated actor that approved it (human subject or runner).
    pub approver: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at_ms: Option<i64>,
}
impl EgressGrantV1 {
    pub fn active_at(&self, now_ms: i64) -> bool {
        self.revoked_at_ms.is_none() && self.not_before_ms <= now_ms && now_ms < self.expires_at_ms
    }
}

/// What the gateway learned about the worker inside the decision transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkerFacts {
    /// Running, same session/attempt, launched with broker egress.
    Eligible,
    NotRunning,
    EgressNotLaunched,
    BindingMismatch,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decided {
    pub outcome: EgressOutcomeV1,
    pub reason: EgressDecisionReasonV1,
    pub grant_id: Option<IdV1>,
    pub remaining_ms: Option<u64>,
}

/// Pure decision shared by every backend. Imperative refusals first, then
/// the operator/registry ceiling, then live grants of this session.
pub fn decide(
    policy: &EgressPolicySnapshot,
    grants: &[EgressGrantV1],
    worker: WorkerFacts,
    session_id: &str,
    authority: &Authority,
    now_ms: i64,
) -> Decided {
    let refused = |reason| Decided {
        outcome: EgressOutcomeV1::Deny,
        reason,
        grant_id: None,
        remaining_ms: None,
    };
    match worker {
        WorkerFacts::Eligible => {}
        WorkerFacts::NotRunning => return refused(EgressDecisionReasonV1::WorkerNotRunning),
        WorkerFacts::EgressNotLaunched => {
            return refused(EgressDecisionReasonV1::EgressNotLaunched);
        }
        WorkerFacts::BindingMismatch => return refused(EgressDecisionReasonV1::BindingMismatch),
    }
    if policy.allows(authority) {
        return Decided {
            outcome: EgressOutcomeV1::Allow,
            reason: EgressDecisionReasonV1::AllowedByPolicy,
            grant_id: None,
            remaining_ms: Some(MAX_POLICY_VALIDITY_MS),
        };
    }
    let canonical = format!("{}:{}", authority.name(), authority.port());
    let granted = grants
        .iter()
        .filter(|grant| {
            grant.session_id == session_id
                && grant.authority == canonical
                && grant.active_at(now_ms)
        })
        .max_by_key(|grant| grant.expires_at_ms);
    if let Some(grant) = granted
        && policy.grantable(authority)
    {
        return Decided {
            outcome: EgressOutcomeV1::Allow,
            reason: EgressDecisionReasonV1::AllowedByGrant,
            grant_id: Some(grant.grant_id.clone()),
            remaining_ms: u64::try_from(grant.expires_at_ms - now_ms).ok(),
        };
    }
    if policy.grantable(authority) {
        return Decided {
            outcome: EgressOutcomeV1::Review,
            reason: EgressDecisionReasonV1::RequiresApproval,
            grant_id: None,
            remaining_ms: None,
        };
    }
    refused(EgressDecisionReasonV1::NotInPolicy)
}

/// Renewal of one live decision against current durable state: `None` keeps
/// the flow, otherwise the flow must close for that reason.
pub fn renewal(
    policy: &EgressPolicySnapshot,
    decided: &Decided,
    grant: Option<&EgressGrantV1>,
    authority: &Authority,
    now_ms: i64,
) -> Option<EgressCloseReasonV1> {
    match decided.reason {
        EgressDecisionReasonV1::AllowedByPolicy if policy.allows(authority) => None,
        EgressDecisionReasonV1::AllowedByGrant => match grant {
            Some(grant) if grant.revoked_at_ms.is_some() => Some(EgressCloseReasonV1::Revoked),
            Some(grant) if grant.active_at(now_ms) && policy.grantable(authority) => None,
            Some(grant) if now_ms >= grant.expires_at_ms => Some(EgressCloseReasonV1::Expired),
            _ => Some(EgressCloseReasonV1::Revoked),
        },
        _ => Some(EgressCloseReasonV1::Revoked),
    }
}

#[cfg(test)]
#[path = "egress_decision_tests.rs"]
mod tests;
