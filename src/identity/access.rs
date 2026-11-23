//! Wire projections for authenticated human readers, separate from control keys.
use super::*;
use crate::model::{CommandSpec, Decision};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HumanReadViewV1 {
    Metadata,
    Content,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanContextV1 {
    pub id: String,
    pub tenant_id: String,
    pub organization_scope: String,
    pub mode: ContextModeV1,
    pub capabilities: Vec<HumanCapabilityV1>,
    pub membership_revision: String,
    pub authorization_version: String,
    pub expires_at: String,
    /// The control's closed operation catalogue for this context. Presentation
    /// only: every route revalidates. Omitted by gateways that predate it.
    #[serde(default)]
    pub operations: Vec<HumanOperationAccessV1>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanAuthenticationV1 {
    pub method: String,
    pub credential_id: String,
    pub expires_at: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanIdentityV1 {
    pub version: u32,
    pub principal: PrincipalRefV1,
    pub display_name: String,
    pub authentication: HumanAuthenticationV1,
    /// False when at least one tenant store could not supply its authorized
    /// contexts. Missing contexts are unknown, not proof of absent membership.
    pub contexts_complete: bool,
    pub contexts: Vec<HumanContextV1>,
    /// Available read protocols, not permissions. Omitted by legacy gateways.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_profiles: Option<HumanReadProfilesV1>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HumanBrokerRegistryReadProfileV1 {
    #[serde(rename = "human_broker_registry_owned_v1")]
    OwnedV1,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HumanWorkerRegistryReadProfileV1 {
    #[serde(rename = "human_worker_registry_owned_v1")]
    OwnedV1,
    Unavailable,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HumanUnavailableReadProfileV1 {
    Unavailable,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanReadProfilesV1 {
    pub broker_registry: HumanBrokerRegistryReadProfileV1,
    pub worker_registry: HumanWorkerRegistryReadProfileV1,
    pub federation_registry: HumanUnavailableReadProfileV1,
}

#[cfg(test)]
mod read_profile_tests {
    use super::*;
    #[test]
    fn read_profiles_are_closed_and_preserve_historical_values() {
        let value = serde_json::json!({"broker_registry":"human_broker_registry_owned_v1", "worker_registry":"human_worker_registry_owned_v1", "federation_registry":"unavailable"});
        let profiles: HumanReadProfilesV1 = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(profiles).unwrap(), value);
        let mut historical = value.clone();
        historical["worker_registry"] = serde_json::json!("unavailable");
        let profiles: HumanReadProfilesV1 = serde_json::from_value(historical.clone()).unwrap();
        assert_eq!(serde_json::to_value(profiles).unwrap(), historical);
        for field in ["broker_registry", "worker_registry", "federation_registry"] {
            let mut unknown = value.clone();
            unknown[field] = serde_json::json!("legacy");
            assert!(serde_json::from_value::<HumanReadProfilesV1>(unknown).is_err());
        }
        let mut unknown = value;
        unknown["future"] = serde_json::json!(true);
        assert!(serde_json::from_value::<HumanReadProfilesV1>(unknown).is_err());
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanResourcePermissionsV1 {
    pub metadata: bool,
    pub content: bool,
    pub export: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanResourceItemV1 {
    pub resource: ResourceRefV1,
    pub owner: PrincipalRefV1,
    pub revision: String,
    pub permissions: HumanResourcePermissionsV1,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanEventMetadataV1 {
    pub id: String,
    pub timestamp: String,
    pub received_at: String,
    pub kind: String,
    pub source: String,
    pub provenance: String,
    pub coverage: String,
    pub resource: ResourceRefV1,
    pub decision: Option<crate::model::Outcome>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanEventContentV1 {
    pub command: Option<CommandSpec>,
    pub decision: Option<Decision>,
    pub result: Option<serde_json::Value>,
    pub data: serde_json::Value,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanEventItemV1 {
    pub metadata: HumanEventMetadataV1,
    pub content: Option<HumanEventContentV1>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanReadReceiptV1 {
    pub request_id: String,
    pub audit_event_id: String,
    pub received_at: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanPageV1<T> {
    pub version: u32,
    pub context_id: String,
    pub authorization_version: String,
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
    pub coverage: String,
    pub read_receipt: HumanReadReceiptV1,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view: Option<HumanReadViewV1>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection_complete: Option<bool>,
    /// Event pages of a store whose closed prefix is archived offline: the
    /// live journal holds no reclaimed event at or below this sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_through_seq: Option<i64>,
}

/// Typed CLI preparation; contains digests and operation identities, never a raw secret.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalHumanProvisionV1 {
    pub version: u32,
    pub mutations: [IdentityMutationV1; 3],
}
impl LocalHumanProvisionV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(self.version == 1, "unsupported local provisioning version");
        for input in &self.mutations {
            input.validate()?;
        }
        let [account_change, binding_change, membership_change] = &self.mutations;
        let (
            IdentityChangeV1::Account {
                expected_revision: 0,
                account,
            },
            IdentityChangeV1::Credential {
                expected_revision: 0,
                binding,
            },
            IdentityChangeV1::Membership {
                expected_revision: 0,
                membership,
            },
        ) = (
            &account_change.change,
            &binding_change.change,
            &membership_change.change,
        )
        else {
            anyhow::bail!("local provisioning requires three fresh typed mutations");
        };
        anyhow::ensure!(
            matches!(account.principal, PrincipalRefV1::HumanLocal { .. })
                && account.principal == binding.principal
                && account.principal == membership.principal
                && account.revision == 1
                && binding.revision == 1
                && membership.revision == 1
                && account.active
                && binding.active
                && membership.active
                && self
                    .mutations
                    .iter()
                    .all(|m| m.actor == account_change.actor)
                && matches!(
                    account_change.actor,
                    PrincipalRefV1::LegacyControl {
                        role: LegacyControlRoleV1::Runner,
                        ..
                    }
                )
                && account_change.operation_id != binding_change.operation_id
                && account_change.operation_id != membership_change.operation_id
                && binding_change.operation_id != membership_change.operation_id,
            "inconsistent local provisioning identities"
        );
        Ok(())
    }
}

pub(crate) struct VerifiedLocalCredentialV1 {
    pub principal: PrincipalRefV1,
    pub credential_id: String,
    pub revision: u64,
    pub expires_at: Option<String>,
}

pub const HUMAN_MAX_PAGE_ITEMS: usize = 100;
pub const HUMAN_MAX_PAGE_BYTES: usize = 262_144;
pub(crate) const HUMAN_PAGE_ENVELOPE_RESERVE: usize = 4096;
pub(crate) const HUMAN_CURSOR_HEX_BYTES: usize = (1 + 12 + 512 + 16) * 2;

/// Closed infrastructure metadata; never the technical WorkerV1/LaunchSpecV1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HumanWorkerTimeStatusV1 {
    Known,
    Absent,
    Unknown,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanWorkerObservationV1 {
    pub observed_at: Option<String>,
    pub observed_at_status: HumanWorkerTimeStatusV1,
    pub status: crate::control::RuntimeStatusV1,
    pub running: Option<bool>,
    pub started_at: Option<String>,
    pub started_at_status: HumanWorkerTimeStatusV1,
    pub finished_at: Option<String>,
    pub finished_at_status: HumanWorkerTimeStatusV1,
    pub exit_code: Option<String>,
    pub oom_killed: Option<bool>,
    pub restart_count: Option<u32>,
    pub provenance: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanWorkerItemV1 {
    pub worker_id: String,
    pub request_id: String,
    pub attempt_id: String,
    pub revision: String,
    pub state: crate::control::WorkerStateV1,
    pub quota_held: bool,
    /// A launch reference, not evidence of a human owner or delegation.
    pub session_id: String,
    pub session_provenance: String,
    pub runtime_kind: crate::control::RuntimeKindV1,
    pub cpu_millis: u32,
    pub memory_bytes: String,
    pub pids_limit: u32,
    pub timeout_seconds: u32,
    pub network: crate::control::NetworkV1,
    pub observed_started_at: Option<String>,
    pub observed_started_at_status: HumanWorkerTimeStatusV1,
    pub allocation_inconsistent: bool,
    pub cleanup_removed: Option<bool>,
    pub last_observation: Option<HumanWorkerObservationV1>,
    pub launch_broker: crate::broker_registry::AuthorityBrokerAssociationV1,
    pub observation_broker: crate::broker_registry::AuthorityBrokerAssociationV1,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanWorkerRegistryPageV1 {
    pub version: u32,
    pub context_id: String,
    pub authorization_version: String,
    pub source: String,
    pub consistency: String,
    pub evaluated_at: String,
    pub runtime_inventory: String,
    pub registry_status: String,
    pub items: Vec<HumanWorkerItemV1>,
    pub has_more: bool,
    pub next_cursor: Option<String>,
    pub read_receipt: HumanReadReceiptV1,
}

/// Tenant-wide infrastructure metadata, distinct from resource ACL projections.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanBrokerRegistryPageV1 {
    pub version: u32,
    pub context_id: String,
    pub authorization_version: String,
    pub source: String,
    pub consistency: String,
    pub evaluated_at: String,
    pub runtime_inventory: String,
    pub items: Vec<HumanBrokerItemV1>,
    pub has_more: bool,
    pub next_cursor: Option<String>,
    pub read_receipt: HumanReadReceiptV1,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanBrokerAuthorityV1 {
    pub authority_id: Option<String>,
    pub runtime_kind: crate::broker_registry::BrokerRuntimeKindV1,
    pub authority_id_redacted: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanBrokerHeartbeatV1 {
    pub incarnation_id: String,
    pub sequence: String,
    pub received_at: String,
    pub lease_expires_at: String,
    pub heartbeat_provenance: crate::broker_registry::BrokerClaimProvenanceV1,
    pub reception_provenance: crate::broker_registry::BrokerReceptionProvenanceV1,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanBrokerItemV1 {
    pub broker_id: String,
    pub incarnation_id: String,
    pub grant_revision: String,
    pub heartbeat_revision: String,
    pub state: crate::broker_registry::BrokerStateV1,
    pub authorities: Vec<HumanBrokerAuthorityV1>,
    pub granted_at: String,
    pub not_after: String,
    pub revoked_at: Option<String>,
    pub last_heartbeat: Option<HumanBrokerHeartbeatV1>,
}
impl HumanBrokerItemV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        use crate::broker_registry::{parse_time, registry_uuid};
        registry_uuid(&self.broker_id)?;
        registry_uuid(&self.incarnation_id)?;
        broker_decimal(&self.grant_revision, false)?;
        broker_decimal(&self.heartbeat_revision, true)?;
        parse_time(&self.granted_at)?;
        parse_time(&self.not_after)?;
        if let Some(time) = &self.revoked_at {
            parse_time(time)?;
        }
        anyhow::ensure!(
            (1..=16).contains(&self.authorities.len()),
            "invalid broker authority count"
        );
        for authority in &self.authorities {
            match (&authority.authority_id, authority.authority_id_redacted) {
                (None, true) => {}
                (Some(id), false) => crate::broker_registry::BrokerAuthorityV1 {
                    authority_id: id.clone(),
                    runtime_kind: authority.runtime_kind,
                }
                .validate()?,
                _ => anyhow::bail!("inconsistent broker authority redaction"),
            }
        }
        if let Some(heartbeat) = &self.last_heartbeat {
            registry_uuid(&heartbeat.incarnation_id)?;
            broker_decimal(&heartbeat.sequence, false)?;
            parse_time(&heartbeat.received_at)?;
            parse_time(&heartbeat.lease_expires_at)?;
        }
        Ok(())
    }
}
fn broker_decimal(value: &str, zero: bool) -> anyhow::Result<()> {
    let number: u64 = value.parse()?;
    anyhow::ensure!(
        number.to_string() == value && number <= i64::MAX as u64 && (zero || number > 0),
        "invalid broker decimal"
    );
    Ok(())
}
