//! Report contract of `ai-guard-diagnose`, version 1.
//!
//! Sections carry facts only: a verdict, a `not_checked` or a `not_available`
//! is always a finding or a limit. A section or a field that was not inspected
//! is `null`, never a status word. Optional fields are always serialised, so
//! that decoding a report and encoding it again yields the same JSON value and
//! a missing or an extra field is detectable by a consumer.
//!
//! Three rules keep the output filter (`crate::redaction`) from rewriting a
//! report: no map is keyed by operator data, no array of strings holds
//! operator data, and no field name is a sensitive key of that filter.

use super::catalogue::FindingCode;
use serde::{Deserialize, Serialize};

pub const SCHEMA: &str = "ai-guard-diagnose-report";
pub const VERSION: u32 = 1;
pub const TOOL_NAME: &str = "ai-guard-diagnose";

// Bounds of what is printed, never of what is checked: every tenant, profile
// and reference is evaluated, totals stay exact, and what is not printed is
// counted in `Summary::omitted` or in the `omitted` field of its list.
/// Identifiers printed in one tenant list.
pub const MAX_LISTED_IDS: usize = 256;
/// Identifiers printed per side of a difference and per `Subjects`.
pub const MAX_SUBJECT_IDS: usize = 16;
/// Entries printed in `credential_references` and in `data`.
pub const MAX_LISTED_ENTRIES: usize = 512;
/// Serialised bytes of each of those two arrays.
pub const MAX_ENTRY_LIST_BYTES: usize = 262_144;

/// Outcome of one finding. One code has one status for the whole of version 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// A product validator accepted the item, or the offline rules restated
    /// by this tool found no violation. The message says which.
    Ok,
    Info,
    /// Usable, but deviating or at risk.
    Warning,
    /// The consuming command refuses this for certain, or two files contradict
    /// each other.
    Error,
    /// Observation attempted and inconclusive.
    Unknown,
    /// Not examined in this mode; `requires` names what resolves it.
    NotChecked,
    /// This diagnostic has no offline inspector for it; `requires` is null.
    NotAvailable,
}

impl Status {
    pub const ALL: [Self; 7] = [
        Self::Ok,
        Self::Info,
        Self::Warning,
        Self::Error,
        Self::Unknown,
        Self::NotChecked,
        Self::NotAvailable,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
            Self::Unknown => "unknown",
            Self::NotChecked => "not_checked",
            Self::NotAvailable => "not_available",
        }
    }
}

/// How a statement was obtained.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Basis {
    /// A file-system fact or the result of a product function.
    Observed,
    /// A file says so.
    Declared,
    /// Consequence of a cited rule, or of an assumed input.
    Inferred,
}

impl Basis {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Observed => "observed",
            Self::Declared => "declared",
            Self::Inferred => "inferred",
        }
    }
}

/// Independent configuration axes, in output order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    Layout,
    Topology,
    Identity,
    Tenants,
    CredentialReferences,
    Providers,
    Inference,
    Trust,
    Engine,
    Data,
    Output,
}

impl Axis {
    pub const ALL: [Self; 11] = [
        Self::Layout,
        Self::Topology,
        Self::Identity,
        Self::Tenants,
        Self::CredentialReferences,
        Self::Providers,
        Self::Inference,
        Self::Trust,
        Self::Engine,
        Self::Data,
        Self::Output,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Layout => "layout",
            Self::Topology => "topology",
            Self::Identity => "identity",
            Self::Tenants => "tenants",
            Self::CredentialReferences => "credential_references",
            Self::Providers => "providers",
            Self::Inference => "inference",
            Self::Trust => "trust",
            Self::Engine => "engine",
            Self::Data => "data",
            Self::Output => "output",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportV1 {
    pub schema: String,
    pub version: u32,
    pub tool: Tool,
    pub mode: Mode,
    pub summary: Summary,
    pub inputs: Vec<Input>,
    pub layouts: Vec<Layout>,
    pub topology: Option<Topology>,
    pub identity: Identity,
    pub tenants: Option<Tenants>,
    pub credential_references: Vec<ReferenceEntry>,
    pub providers: Option<Providers>,
    pub inference: Inference,
    pub trust: Vec<TrustEntry>,
    pub engine: Option<Engine>,
    pub data: Vec<DataEntry>,
    pub findings: Vec<Finding>,
    /// Fixed for a build: what this build cannot examine or does not qualify.
    pub limits: Vec<Finding>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub package_version: String,
}

/// What a run did. Every value is a closed word, never a status.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mode {
    pub network: NetworkUse,
    pub mutation: MutationUse,
    pub credential_contents_read: CredentialContentUse,
    /// The closed list of files opened for content in this run.
    pub files_read_for_content: Vec<ContentRole>,
    /// The active checks that ran, whatever their result. A check that was
    /// asked for and could not run is not listed.
    pub active_checks_executed: Vec<ActiveCheck>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkUse {
    /// No connection was opened.
    None,
    /// Connections were opened by the active checks listed in the mode, and
    /// by nothing else.
    ActiveChecksOnly,
}

/// A check that contacts something. None runs unless its option names it.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, clap::ValueEnum,
)]
#[serde(rename_all = "snake_case")]
pub enum ActiveCheck {
    /// Ask the identity provider of an inspected OIDC file for its discovery
    /// document and its keys, through the client of the product.
    OidcDiscovery,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationUse {
    None,
}

/// Whether this run read the content of a key file, a token file, a
/// credential, a setup record or a store. A file is opened only as the input
/// its option or its fixed name describes, after it was compared with every
/// such file known at that moment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialContentUse {
    /// No opened file is one that the inspected files name as such. A file
    /// that no inspected file names cannot be recognised.
    None,
    /// A file was opened, and a file read after it names it as such: its
    /// content was read before that could be known. It is never printed.
    DetectedAfterRead,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentRole {
    TenantRegistry,
    OidcFile,
    OidcCaBundle,
    ProviderProfiles,
    DecisionEngineCaBundle,
    GatewayCaBundle,
    /// Listed when an approval was attempted on an existing program: the
    /// approval hashes the program unless one of its earlier checks refuses.
    EngineProgram,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    pub status: SummaryStatus,
    pub exit_code: i32,
    pub counts: Counts,
    /// True when detail arrays were emptied to respect the document bound.
    /// False does not mean that every entry is listed: `omitted` says that.
    pub truncated: bool,
    pub omitted: Omitted,
}

/// `ok` is exit 0, `error` exit 3, `unknown` exit 4.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SummaryStatus {
    Ok,
    Error,
    Unknown,
}

/// Findings and limits by status.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub ok: u64,
    pub info: u64,
    pub warning: u64,
    pub error: u64,
    pub unknown: u64,
    pub not_checked: u64,
    pub not_available: u64,
}

/// Entries evaluated but not printed. Totals elsewhere stay exact.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Omitted {
    pub reference_entries: u64,
    pub data_entries: u64,
    pub tenant_ids: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Input {
    pub name: InputName,
    pub origin: InputOrigin,
    pub display: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputName {
    ConfigDir,
    InstallationDir,
    TenantsFile,
    OidcConfig,
    ProvidersFile,
    Client,
    AuthMode,
    ClientProfile,
    Model,
    MaxEvents,
    JevUrl,
    JevLocalUrl,
    JevAllowLocalHttp,
    JevCaFile,
    GatewayCaFile,
    Runtime,
    RuntimeProgram,
    Active,
}

impl InputName {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ConfigDir => "config_dir",
            Self::InstallationDir => "installation_dir",
            Self::TenantsFile => "tenants_file",
            Self::OidcConfig => "oidc_config",
            Self::ProvidersFile => "providers_file",
            Self::Client => "client",
            Self::AuthMode => "auth_mode",
            Self::ClientProfile => "client_profile",
            Self::Model => "model",
            Self::MaxEvents => "max_events",
            Self::JevUrl => "jev_url",
            Self::JevLocalUrl => "jev_local_url",
            Self::JevAllowLocalHttp => "jev_allow_local_http",
            Self::JevCaFile => "jev_ca_file",
            Self::GatewayCaFile => "gateway_ca_file",
            Self::Runtime => "runtime",
            Self::RuntimeProgram => "runtime_program",
            Self::Active => "active",
        }
    }

    /// The command-line option that carries this input.
    pub fn flag(self) -> &'static str {
        match self {
            Self::ConfigDir => "--config-dir",
            Self::InstallationDir => "--installation-dir",
            Self::TenantsFile => "--tenants-file",
            Self::OidcConfig => "--oidc-config",
            Self::ProvidersFile => "--providers-file",
            Self::Client => "--client",
            Self::AuthMode => "--auth-mode",
            Self::ClientProfile => "--client-profile",
            Self::Model => "--model",
            Self::MaxEvents => "--max-events",
            Self::JevUrl => "--jev-url",
            Self::JevLocalUrl => "--jev-local-url",
            Self::JevAllowLocalHttp => "--jev-allow-local-http",
            Self::JevCaFile => "--jev-ca-file",
            Self::GatewayCaFile => "--gateway-ca-file",
            Self::Runtime => "--runtime",
            Self::RuntimeProgram => "--runtime-program",
            Self::Active => "--active",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputOrigin {
    /// Given on the command line.
    Cli,
    /// Assumed by this tool because nothing was given.
    Default,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layout {
    pub input: InputName,
    pub kind: LayoutKind,
    pub preparation: Option<PreparationState>,
    pub oidc_file_version: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutKind {
    Absent,
    /// The directory given to `init`, `run` and `providers`.
    HistoricalConfigDir,
    /// A directory published by `setup prepare`.
    PreparedInstallation,
    /// A retained, unpublished preparation.
    SetupStagingDirectory,
    /// Both families in one directory.
    Mixed,
    /// A registry given alone with `--tenants-file`.
    HandWrittenRegistry,
    Unrecognized,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreparationState {
    Complete,
    CompletionMarkerAbsent,
    Incomplete,
    NoIntent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Topology {
    pub tenant_model: TenantModel,
    pub tenant_count: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TenantModel {
    SingleLocal,
    Registry,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    /// Runner and viewer references by presence; never a value.
    pub technical_key_references: Option<ReferenceCounts>,
    pub sso: Option<Sso>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReferenceCounts {
    pub present: u64,
    pub absent: u64,
    pub unknown: u64,
}

/// Public parameters of an inspected OIDC file. URLs are rebuilt from their
/// parsed value: user information, query and fragment are never printed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sso {
    pub file_version: Option<u32>,
    pub issuer: Option<String>,
    pub transport: Option<SsoTransport>,
    pub provider_label: Option<String>,
    pub web_origin: Option<String>,
    pub web_client_id: Option<String>,
    pub cli_client_id: Option<String>,
    pub api_audience: Option<String>,
    pub authorization_endpoint: Option<String>,
    pub token_endpoint: Option<String>,
    pub jwks_uri: Option<String>,
    pub end_session_endpoint: Option<String>,
    pub web_redirect_uri: Option<String>,
    pub cli_callback_path: Option<String>,
    pub mapping: Option<SsoMapping>,
    pub ca_bundle_declared: bool,
    /// True when the active discovery ran against this provider, whatever
    /// its result.
    pub idp_contacted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SsoTransport {
    HttpsOnly,
    IsolatedDemoHttp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SsoMapping {
    pub revision: String,
    pub rule_count: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tenants {
    pub registry: Option<Vec<IdDisplay>>,
    pub plan: Option<Vec<IdDisplay>>,
    pub registry_total: Option<u64>,
    pub plan_total: Option<u64>,
    pub correspondence: Option<Correspondence>,
}

/// An identifier as an object, so the array rule of the output filter never
/// applies. `masked` is true when the filter would rewrite the identifier and
/// a numbered placeholder is shown in its place.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct IdDisplay {
    pub id: String,
    pub masked: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Correspondence {
    Match,
    Mismatch,
}

/// One referenced credential file, described by metadata only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReferenceEntry {
    pub owner: Owner,
    pub role: ReferenceRole,
    pub consumer: Consumer,
    pub location: Location,
    pub display: String,
    pub presence: Presence,
    pub entry_kind: Option<EntryKind>,
    /// Permission bits as four octal digits.
    pub mode: Option<String>,
    pub owner_is_inspector: Option<bool>,
    pub single_link: Option<bool>,
    pub private_rule: PrivateRule,
    pub content: ContentState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Owner {
    pub kind: OwnerKind,
    pub id: Option<String>,
    pub masked: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerKind {
    Installation,
    Tenant,
    ProviderProfile,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceRole {
    Runner,
    Viewer,
    DecisionEngine,
    Projection,
    Provider,
    SetupMaterial,
}

/// The reader whose rule decides whether a reference is usable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Consumer {
    GatewayTokenReader,
    PrivateFileReader,
    ComposeProjection,
    SetupRecovery,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Location {
    InsideRoot,
    OutsideRoot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Presence {
    Present,
    Absent,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    RegularFile,
    Directory,
    SymbolicLink,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivateRule {
    Satisfied,
    Violated,
    NotEvaluated,
}

/// What this run did with the content of a referenced file or of a store.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentState {
    /// The file was not opened.
    NotRead,
    /// The file was opened as the input that an option or a fixed name
    /// describes, before a file read after it named it as a key file, a
    /// credential, a setup record or a store: the file that
    /// `reference.file_read_before_it_was_named` reports. Its content was
    /// read and is never printed.
    ReadBeforeNamed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Providers {
    pub file: ProviderFile,
    pub default_profile: Option<IdDisplay>,
    /// Declared order, as the selection rule reads it.
    pub auto_priority: Vec<IdDisplay>,
    pub profiles: Vec<ProviderProfile>,
    /// What a managed launch selects among these profiles; `null` when no
    /// profile file was loaded.
    pub selection: Option<Selection>,
}

/// The profile a managed launch selects for the selection options given to
/// this tool, as the product's own choice returns it. No credential is opened
/// to make that choice.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    pub requested: RequestedSelection,
    /// `null` when the product refuses the selection.
    pub resolved: Option<ResolvedSelection>,
}

/// How each part of the selection was requested. The values of the options
/// are not repeated here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestedSelection {
    pub client: Requested,
    pub auth_mode: Requested,
    pub profile: Requested,
    pub model: Requested,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Requested {
    /// Given on the command line of this tool.
    Explicit,
    /// Left to the automatic choice of the product.
    Auto,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedSelection {
    pub profile: IdDisplay,
    pub revision: u32,
    pub client: crate::provider::ClientKind,
    pub auth_mode: crate::provider::AuthMode,
    /// The model a launch passes to the client: the requested one, else the
    /// one of the profile; `null` when neither names a model.
    pub model: Option<String>,
    /// What decided the choice, as the product returns it.
    pub chosen_by: crate::provider::ChoiceSource,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderFile {
    pub state: ProviderFileState,
    pub display: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderFileState {
    Absent,
    Refused,
    Invalid,
    Loaded,
}

/// One declared profile. Its reference is the `credential_references` entry
/// whose owner is this profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderProfile {
    pub profile: IdDisplay,
    pub revision: u32,
    pub client: crate::provider::ClientKind,
    pub auth_mode: crate::provider::AuthMode,
    pub model: Option<String>,
    pub source_kind: CredentialSourceKind,
    pub variable_name: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialSourceKind {
    ApiEnvFile,
    CodexAuthFile,
    ClaudeOauthEnvFile,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inference {
    /// The explicit decision-engine transport inputs, when given.
    pub decision_engine_transport: Option<DecisionTransport>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionTransport {
    pub kind: DecisionTransportKind,
    pub url: String,
    pub allow_local_http: bool,
    pub ca_bundle_given: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionTransportKind {
    Remote,
    Local,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustEntry {
    pub role: ContentRole,
    pub display: String,
    /// False when the file was refused before any read.
    pub read: bool,
    pub certificate_count: Option<u64>,
}

/// An approved engine program. The invocation binding is never serialised:
/// environment, working directory, account home, canonical path, device,
/// inode and length stay out of the report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Engine {
    pub kind: crate::runtime::RuntimeKind,
    pub entry_display: String,
    pub sha256: String,
    pub policy_version: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataEntry {
    pub owner: Owner,
    pub role: DataRole,
    pub location: Location,
    pub display: String,
    pub presence: Presence,
    pub sidecars: Vec<Sidecar>,
    pub content: ContentState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataRole {
    TenantStore,
    AuthenticationAudit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sidecar {
    Wal,
    Shm,
    Journal,
}

/// One verdict or one named gap, with its origin, its scope and an action.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub code: FindingCode,
    pub axis: Axis,
    pub status: Status,
    pub basis: Basis,
    pub scope: Scope,
    pub origin: Origin,
    pub message: String,
    pub action: String,
    /// For `not_checked`: an accepted flag of this tool, an existing command,
    /// or the place where the value lives.
    pub requires: Option<String>,
    pub subjects: Option<Subjects>,
    pub details: Option<Details>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Scope {
    /// One named entry of `inputs`.
    Input {
        input: InputName,
    },
    Tenants,
    ProviderProfiles,
    /// One file read for content.
    File {
        file: ContentRole,
    },
    Host,
    Build,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Origin {
    pub sources: Vec<Source>,
    /// The command that applies the rule. Never a Rust symbol.
    pub consumer: Option<String>,
    /// Assumed inputs the verdict depends on.
    pub assumes: Vec<InputName>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub file: String,
    pub field: Option<String>,
}

/// The tenants or profiles a finding concerns: exact total, bounded list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subjects {
    pub kind: SubjectKind,
    pub total: u64,
    pub ids: Vec<IdDisplay>,
    pub omitted: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubjectKind {
    Tenants,
    ProviderProfiles,
}

/// Closed detail shapes. No variant carries an error text of a loader, a
/// parser or the file system.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Details {
    TenantSetDifference {
        only_in_registry: Vec<IdDisplay>,
        only_in_plan: Vec<IdDisplay>,
        omitted: u64,
    },
    JsonError {
        category: JsonErrorCategory,
        line: u64,
        column: u64,
    },
    Reason {
        reason: Reason,
    },
    Policy {
        policy: PolicyKind,
    },
    WouldRefuseWith {
        class: ProviderRefusalClass,
    },
    Sidecars {
        sidecars: Vec<Sidecar>,
    },
    EngineProgram {
        /// True for an explicit program, false for the fixed entries.
        explicit_program: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JsonErrorCategory {
    Syntax,
    Data,
    Eof,
    Io,
}

/// Closed reasons of a refusal by a product validator. The validator's text
/// is never printed; an unrecognised refusal is `unclassified`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    // OIDC gateway block.
    VersionOrTokenBound,
    WebOriginOrCallback,
    ClaimMappingInvalid,
    ClaimMappingAmbiguous,
    UrlUnparseable,
    ProviderBlockInvalid,
    // Provider profiles.
    DuplicateOrInvalidProfile,
    UnknownOrDuplicatePriority,
    DefaultProfileMissing,
    JsonInvalid,
    VersionOrSize,
    ProfileIdentifier,
    ModelIdentifier,
    SourceForMode,
    VariableForClient,
    PathNotAbsolute,
    PathTraversal,
    // Provider selection.
    ExplicitConflict,
    SeveralMatchWithoutPriority,
    RequestedProfileNotConfigured,
    NoProfileMatches,
    // Engine program.
    AccountHomeUnavailable,
    HelperDirectoriesRefused,
    ProgramOrDirectoryRefused,
    Unclassified,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyKind {
    WorkerQuota,
    KubernetesHeldCreate,
    KubernetesSessionReservation,
    KubernetesSessionRecovery,
    KubernetesSessionControlRecovery,
    AuditCapacity,
    WorkloadAudit,
    RelayRetention,
}

/// The class a launch would refuse with, from metadata only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderRefusalClass {
    LoginRequired,
    CredentialMissing,
}
