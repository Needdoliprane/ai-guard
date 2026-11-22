//! Wire of the administration configuration read (`read_configuration`).
//!
//! Kinds and modes only: storage backend kind, TLS profile kind, decision
//! engine kind, identity mode, runtime kinds, egress mode, provider client and
//! mode availability, versions. Never a host, path, URL, credential, digest or
//! file content. A value the gateway cannot establish is `unknown`.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigurationAxisV1 {
    Build,
    Topology,
    Identity,
    Storage,
    Decision,
    Runtime,
    Egress,
    Providers,
}

/// Where a value comes from. `configured`: explicit gateway configuration;
/// `default`: the built-in default; `derived`: computed by the control from
/// its durable state; `declared`: asserted by a registered broker; `unknown`:
/// not established at the gateway.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigurationProvenanceV1 {
    Configured,
    Default,
    Derived,
    Declared,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationEntryV1 {
    pub axis: ConfigurationAxisV1,
    /// Closed key within its axis.
    pub key: String,
    /// Closed word or version string; never a free-form path or address.
    pub value: String,
    pub provenance: ConfigurationProvenanceV1,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanConfigurationV1 {
    pub version: u32,
    pub context_id: String,
    pub authorization_version: String,
    pub evaluated_at: String,
    pub entries: Vec<ConfigurationEntryV1>,
    pub read_receipt: HumanReadReceiptV1,
}

/// The closed key set of each axis; the route refuses to publish another key.
pub const CONFIGURATION_KEYS: &[(ConfigurationAxisV1, &str)] = &[
    (ConfigurationAxisV1::Build, "product_version"),
    (ConfigurationAxisV1::Topology, "source_profile"),
    (ConfigurationAxisV1::Identity, "local_credentials"),
    (ConfigurationAxisV1::Identity, "oidc"),
    (ConfigurationAxisV1::Identity, "oidc_transport"),
    (ConfigurationAxisV1::Storage, "backend"),
    (ConfigurationAxisV1::Storage, "tls_profile"),
    (ConfigurationAxisV1::Storage, "store_format"),
    (ConfigurationAxisV1::Storage, "human_rights_marker"),
    (ConfigurationAxisV1::Decision, "engine"),
    (ConfigurationAxisV1::Decision, "selection"),
    (ConfigurationAxisV1::Runtime, "declared_runtime_kinds"),
    (ConfigurationAxisV1::Runtime, "kubernetes_held_create"),
    (ConfigurationAxisV1::Egress, "mode"),
    (ConfigurationAxisV1::Providers, "availability"),
];
