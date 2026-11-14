//! Durable broker metadata and receipt freshness, separate from runtime facts.
//!
//! A heartbeat authenticates its sender at the control boundary. Its assertion
//! of being alive remains declared; receiving it is observed by the gateway.
//! Neither fact proves that a worker ran, stopped, or released its reservation.
use anyhow::Context;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

mod checkpoint;
mod federation;
/// Proposed owned broker-supervision profile: pure shared contract only.
pub mod owned_supervision;
pub use checkpoint::{
    BrokerCheckpointCoverageV1, BrokerCheckpointFreshnessV1, BrokerCheckpointObservationV1,
    BrokerCheckpointResultV1, BrokerCheckpointSnapshotV1, BrokerCheckpointV1,
    BrokerCheckpointViewV1, BrokerJobCheckpointV1, MAX_BROKER_CHECKPOINT_BYTES,
    MAX_BROKER_CHECKPOINT_JOBS,
};
pub use federation::{
    AuthorityBrokerAssociationV1, AuthorityBrokerStatusV1, FederationBrokerV1,
    FederationWorkerPageV1, FederationWorkerV1, MAX_FEDERATION_PAGE_BYTES,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrokerRegistryFailureV1 {
    InvalidRequest,
    Conflict,
    NotFound,
    PermissionDenied,
    MigrationRequired,
    ClockRegression,
}
impl std::fmt::Display for BrokerRegistryFailureV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidRequest => "invalid broker registry request",
            Self::Conflict => "broker registry operation conflicts with durable content or state",
            Self::NotFound => "broker or operation not found",
            Self::PermissionDenied => "broker grant is unavailable, expired, revoked or outside the authenticated scope",
            Self::MigrationRequired => "explicit workload schema migration is required",
            Self::ClockRegression => "control clock precedes the last broker registry mutation; reconcile the clock before retrying",
        })
    }
}
impl std::error::Error for BrokerRegistryFailureV1 {}

pub(crate) fn require(ok: bool) -> anyhow::Result<()> {
    if ok {
        Ok(())
    } else {
        Err(BrokerRegistryFailureV1::InvalidRequest.into())
    }
}
pub(crate) fn conflict(ok: bool) -> anyhow::Result<()> {
    if ok {
        Ok(())
    } else {
        Err(BrokerRegistryFailureV1::Conflict.into())
    }
}
pub(crate) fn registry_uuid(value: &str) -> anyhow::Result<()> {
    require(uuid::Uuid::parse_str(value).is_ok_and(|id| !id.is_nil() && id.to_string() == value))
}
pub(crate) fn parse_time(value: &str) -> anyhow::Result<DateTime<Utc>> {
    require(value.len() <= 64)?;
    let time = DateTime::parse_from_rfc3339(value)
        .map_err(|_| BrokerRegistryFailureV1::InvalidRequest)?
        .with_timezone(&Utc);
    require(time.timestamp() > 0)?;
    Ok(time)
}
pub(crate) fn format_time(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
pub(crate) fn revision(value: u64) -> anyhow::Result<()> {
    require(value <= i64::MAX as u64)
}

/// This enum describes an authority scope, not an implemented runtime adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrokerRuntimeKindV1 {
    Docker,
    Podman,
    Kubernetes,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokerAuthorityV1 {
    pub authority_id: String,
    pub runtime_kind: BrokerRuntimeKindV1,
}
impl BrokerAuthorityV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        require(
            !self.authority_id.is_empty()
                && self.authority_id.len() <= 256
                && !self.authority_id.chars().any(char::is_control),
        )
    }
}

/// Trusted operator input. No field of this DTO authenticates a broker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokerGrantMutationV1 {
    pub version: u32,
    pub operation_id: String,
    pub broker_id: String,
    pub expected_revision: Option<u64>,
    pub credential_id: String,
    pub incarnation_id: String,
    pub authorities: Vec<BrokerAuthorityV1>,
    pub lease_seconds: u32,
    pub not_after: String,
}
impl BrokerGrantMutationV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        require(
            self.version == 1
                && (1..=300).contains(&self.lease_seconds)
                && !self.authorities.is_empty()
                && self.authorities.len() <= 16,
        )?;
        for id in [
            &self.operation_id,
            &self.broker_id,
            &self.credential_id,
            &self.incarnation_id,
        ] {
            registry_uuid(id)?;
        }
        if let Some(value) = self.expected_revision {
            require(value > 0)?;
            revision(value)?;
        }
        parse_time(&self.not_after)?;
        let mut ids = BTreeSet::new();
        for authority in &self.authorities {
            authority.validate()?;
            require(ids.insert(authority.authority_id.as_str()))?;
        }
        require(serde_json::to_vec(self)?.len() <= 16384)
    }
}

/// Separate maintenance input: it can never be selected by a heartbeat kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokerRevocationV1 {
    pub version: u32,
    pub operation_id: String,
    pub broker_id: String,
    pub expected_revision: u64,
}
impl BrokerRevocationV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        require(self.version == 1 && self.expected_revision > 0)?;
        registry_uuid(&self.operation_id)?;
        registry_uuid(&self.broker_id)?;
        revision(self.expected_revision)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokerHeartbeatV1 {
    pub version: u32,
    pub operation_id: String,
    pub expected_heartbeat_revision: u64,
    pub incarnation_id: String,
    pub sequence: u64,
}
impl BrokerHeartbeatV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        require(self.version == 1 && self.sequence > 0)?;
        registry_uuid(&self.operation_id)?;
        registry_uuid(&self.incarnation_id)?;
        revision(self.expected_heartbeat_revision)?;
        revision(self.sequence)
    }
}

/// Non-serializable proof resolved by the trusted authentication boundary.
/// There is deliberately no public constructor or deserializer.
#[derive(Debug, Clone)]
pub struct BrokerIdentityV1 {
    tenant_id: String,
    broker_id: String,
    credential_id: String,
    binding_revision: u64,
    grant_revision: u64,
}
impl BrokerIdentityV1 {
    pub(crate) fn authenticated(
        tenant_id: String,
        broker_id: String,
        credential_id: String,
        binding_revision: u64,
        grant_revision: u64,
    ) -> anyhow::Result<Self> {
        crate::tenancy::validate_tenant_id(&tenant_id)?;
        registry_uuid(&broker_id)?;
        registry_uuid(&credential_id)?;
        require(binding_revision > 0 && grant_revision > 0)?;
        revision(binding_revision)?;
        revision(grant_revision)?;
        Ok(Self {
            tenant_id,
            broker_id,
            credential_id,
            binding_revision,
            grant_revision,
        })
    }
    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }
    pub fn broker_id(&self) -> &str {
        &self.broker_id
    }
    pub fn credential_id(&self) -> &str {
        &self.credential_id
    }
    pub fn binding_revision(&self) -> u64 {
        self.binding_revision
    }
    pub fn grant_revision(&self) -> u64 {
        self.grant_revision
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrokerClaimProvenanceV1 {
    Declared,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrokerReceptionProvenanceV1 {
    Observed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokerHeartbeatReceiptV1 {
    pub incarnation_id: String,
    pub sequence: u64,
    pub received_at: String,
    pub lease_expires_at: String,
    pub heartbeat_provenance: BrokerClaimProvenanceV1,
    pub reception_provenance: BrokerReceptionProvenanceV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokerV1 {
    pub version: u32,
    pub broker_id: String,
    pub grant_revision: u64,
    pub heartbeat_revision: u64,
    pub credential_id: String,
    pub incarnation_id: String,
    pub authorities: Vec<BrokerAuthorityV1>,
    pub lease_seconds: u32,
    pub not_after: String,
    pub revoked: bool,
    pub granted_at: String,
    pub revoked_at: Option<String>,
    pub last_heartbeat: Option<BrokerHeartbeatReceiptV1>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrokerStateV1 {
    Unseen,
    Current,
    Stale,
    Expired,
    Revoked,
    ClockUncertain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokerViewV1 {
    pub broker: BrokerV1,
    pub state: BrokerStateV1,
    pub evaluated_at: String,
    /// Liveness is not a complete runtime inventory or a terminal observation.
    pub runtime_inventory_complete: bool,
}

/// Instantaneous authenticated scope, distinct from the non-serializable proof.
/// Deserializing this view grants no permission for a later operation and says
/// nothing about process health, runtime inventory or execution admission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokerIdentityViewV1 {
    pub version: u32,
    pub tenant_id: String,
    pub control_namespace_uuid: String,
    pub schema_version: u32,
    pub broker_id: String,
    pub incarnation_id: String,
    pub credential_id: String,
    pub binding_revision: u64,
    pub grant_revision: u64,
}

impl BrokerV1 {
    pub(crate) fn latest_received_at(&self) -> anyhow::Result<DateTime<Utc>> {
        let mut latest = parse_time(&self.granted_at)?;
        for time in self
            .revoked_at
            .iter()
            .chain(self.last_heartbeat.iter().map(|v| &v.received_at))
        {
            latest = latest.max(parse_time(time)?);
        }
        Ok(latest)
    }
    pub fn view_at(&self, now: &str) -> anyhow::Result<BrokerViewV1> {
        let time = parse_time(now)?;
        let state = if self.revoked {
            BrokerStateV1::Revoked
        } else if time < self.latest_received_at()? {
            BrokerStateV1::ClockUncertain
        } else if time >= parse_time(&self.not_after)? {
            BrokerStateV1::Expired
        } else if let Some(heartbeat) = self
            .last_heartbeat
            .as_ref()
            .filter(|heartbeat| heartbeat.incarnation_id == self.incarnation_id)
        {
            if time >= parse_time(&heartbeat.lease_expires_at)? {
                BrokerStateV1::Stale
            } else {
                BrokerStateV1::Current
            }
        } else {
            BrokerStateV1::Unseen
        };
        Ok(BrokerViewV1 {
            broker: self.clone(),
            state,
            evaluated_at: now.into(),
            runtime_inventory_complete: false,
        })
    }
    pub(crate) fn require_current_grant(
        &self,
        identity: &BrokerIdentityV1,
        now: &str,
    ) -> anyhow::Result<()> {
        if !self.revoked
            && self.broker_id == identity.broker_id
            && self.credential_id == identity.credential_id
            && self.grant_revision == identity.grant_revision
            && parse_time(now)? < parse_time(&self.not_after)?
        {
            Ok(())
        } else {
            Err(BrokerRegistryFailureV1::PermissionDenied.into())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokerMutationReceiptV1 {
    pub operation_id: String,
    pub broker_id: String,
    pub grant_revision: u64,
    pub heartbeat_revision: u64,
    pub input_hash: String,
    pub audit_seq: i64,
    pub audit_hash: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokerMutationResultV1 {
    pub broker: BrokerV1,
    pub receipt: BrokerMutationReceiptV1,
    pub replayed: bool,
}

pub(crate) fn next_revision(value: u64) -> anyhow::Result<u64> {
    let next = value.checked_add(1).context("broker revision exhausted")?;
    revision(next)?;
    Ok(next)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod checkpoint_tests;
