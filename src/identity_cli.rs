//! Explicit offline identity maintenance and authenticated human read access.
//!
//! Offline authority comes from the private control filesystem and effective
//! UID. It is distinct from a human login or an HTTP runner credential.
use anyhow::Result;
use clap::{Args, Subcommand, ValueEnum};
use std::path::PathBuf;

mod export;
mod http;
mod offline;
mod oidc;
mod oidc_callback;
mod oidc_session;
mod trust;

#[derive(Args)]
pub struct HumanArgs {
    #[command(subcommand)]
    pub command: HumanCommand,
}

#[derive(Subcommand)]
pub enum HumanCommand {
    /// Sign in to ai-guard in an external browser; keep this human session in memory.
    Sso(oidc::SsoArgs),
    /// Prepare private files before atomically provisioning a local human.
    Provision(ProvisionArgs),
    /// Replay the exact retained preparation, without rotating or overwriting.
    ResumeProvision(ResumeProvisionArgs),
    /// Apply a typed offline mutation; actor is derived from control ownership.
    Apply(ApplyArgs),
    /// Provision a fixed Legacy session target with the control stopped.
    #[command(mut_arg("max_events", |arg| arg.default_value(crate::audit::DEFAULT_GATEWAY_AUDIT_MAX_EVENTS_ARG)))]
    ProvisionLegacySessionTarget(LegacySessionTargetArgs),
    /// Retain a preparation for a fixed target as a locally authenticated human.
    PrepareLegacySession(LegacySessionRequestArgs),
    /// Confirm the exact retained Legacy preparation for later broker admission.
    ConfirmLegacySession(LegacySessionRequestArgs),
    /// Display authenticated identity and the contexts offered by the control.
    Identity(ConnectionArgs),
    /// Read one authorized resource page, or list contexts when none is selected.
    Resources(ResourcesArgs),
    /// Read an explicitly selected resource and metadata/content view.
    Events(EventsArgs),
    /// Export an explicit authorized selection into a new private destination.
    Export(ExportArgs),
    /// Resume a retained incomplete export using its exact context and selection.
    ResumeExport(ResumeExportArgs),
}

#[derive(Args)]
pub struct OfflineArgs {
    /// Existing validated schema4 through schema13 tenant database; registry permissions still require their explicit migrations. Never created or migrated implicitly.
    #[arg(long)]
    pub data: PathBuf,
    /// Verify the existing database binding; this grants no remote tenant access.
    #[arg(long)]
    pub tenant_id: String,
    /// Confirm that all controls using this database have been stopped explicitly.
    #[arg(long, required = true)]
    pub confirm_control_stopped: bool,
    /// Audit capacity, including the mandatory identity mutation records.
    #[arg(long, default_value_t = 1_000_000)]
    pub max_events: u64,
    /// Private audit capacity policy; cumulative usage remains in the same chain.
    #[arg(long)]
    pub audit_capacity_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Capability {
    ReadMetadata,
    ReadContent,
    Export,
    ReadTenantMetadata,
    ManageMemberships,
    ManageGrants,
    ReadBrokerRegistry,
    ReadWorkerRegistry,
    ApproveLaunch,
    ReadMemberships,
    ReadConfiguration,
    LaunchSession,
    SendSessionMessage,
    ApproveEgress,
}
impl From<Capability> for crate::identity::HumanCapabilityV1 {
    fn from(value: Capability) -> Self {
        use crate::identity::HumanCapabilityV1 as C;
        match value {
            Capability::ReadMetadata => C::ReadMetadata,
            Capability::ReadContent => C::ReadContent,
            Capability::Export => C::Export,
            Capability::ReadTenantMetadata => C::ReadTenantMetadata,
            Capability::ManageMemberships => C::ManageMemberships,
            Capability::ManageGrants => C::ManageGrants,
            Capability::ReadBrokerRegistry => C::ReadBrokerRegistry,
            Capability::ReadWorkerRegistry => C::ReadWorkerRegistry,
            Capability::ReadMemberships => C::ReadMemberships,
            Capability::ReadConfiguration => C::ReadConfiguration,
            Capability::ApproveLaunch => C::ApproveLaunch,
            Capability::LaunchSession => C::LaunchSession,
            Capability::SendSessionMessage => C::SendSessionMessage,
            Capability::ApproveEgress => C::ApproveEgress,
        }
    }
}

#[derive(Args)]
pub struct ProvisionArgs {
    #[command(flatten)]
    pub offline: OfflineArgs,
    #[arg(long)]
    pub name: String,
    #[arg(long)]
    pub organization: String,
    /// Fresh directory below an existing private parent; retained on any error.
    #[arg(long)]
    pub output_dir: PathBuf,
    #[arg(long, value_enum, value_delimiter = ',', default_values = ["read-metadata", "read-content", "export"])]
    pub capabilities: Vec<Capability>,
}

#[derive(Args)]
pub struct ResumeProvisionArgs {
    #[command(flatten)]
    pub offline: OfflineArgs,
    #[arg(long)]
    pub output_dir: PathBuf,
}

#[derive(Args)]
pub struct ApplyArgs {
    #[command(flatten)]
    pub offline: OfflineArgs,
    /// Private JSON {version,operation_id,change}; an actor field is refused.
    #[arg(long)]
    pub mutation_file: PathBuf,
}

#[derive(Args)]
pub struct LegacySessionTargetArgs {
    #[command(flatten)]
    pub offline: OfflineArgs,
    /// Payload byte cap the gateway uses for this SQLite tenant store.
    #[arg(long, default_value_t = crate::audit::DEFAULT_PHYSICAL_BYTES)]
    pub audit_max_bytes: u64,
    /// Private JSON containing the exact target and its expected previous revision.
    #[arg(long)]
    pub target_file: PathBuf,
    /// Private broker registry V2 for deriving or verifying the exact target's source identity.
    #[arg(long)]
    pub broker_registry: Option<PathBuf>,
}

#[derive(Args)]
pub struct LegacySessionRequestArgs {
    #[command(flatten)]
    pub context: ContextArgs,
    /// Private JSON containing a typed request and its stable operation ID.
    #[arg(long)]
    pub request_file: PathBuf,
}

#[derive(Args)]
pub struct ConnectionArgs {
    /// Gateway origin: verified HTTPS, or HTTP on a loopback address.
    #[arg(long)]
    pub gateway: String,
    /// Approved public CA PEM for this HTTPS gateway only; replaces built-in roots.
    #[arg(long)]
    pub gateway_ca_file: Option<PathBuf>,
    /// Checked private local human credential file; no token argv/env option.
    #[arg(long)]
    pub credential_file: PathBuf,
}

#[derive(Args)]
pub struct ContextArgs {
    #[command(flatten)]
    pub connection: ConnectionArgs,
    /// Exact opaque context ID returned by `human identity`.
    #[arg(long)]
    pub context: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum View {
    Metadata,
    Content,
}
impl View {
    fn wire(self) -> &'static str {
        match self {
            Self::Metadata => "metadata",
            Self::Content => "content",
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ResourceKind {
    Session,
    Sandbox,
    Agent,
    Project,
}
impl ResourceKind {
    fn wire(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Sandbox => "sandbox",
            Self::Agent => "agent",
            Self::Project => "project",
        }
    }
}

#[derive(Args)]
pub struct SelectionArgs {
    #[arg(long, value_enum)]
    pub view: View,
    #[arg(long, value_enum)]
    pub resource_kind: ResourceKind,
    #[arg(long)]
    pub resource_id: String,
}

#[derive(Args)]
pub struct ResourcesArgs {
    #[command(flatten)]
    pub connection: ConnectionArgs,
    #[arg(long)]
    pub context: Option<String>,
    #[arg(long, default_value_t = 25, value_parser = clap::value_parser!(u16).range(1..=100))]
    pub limit: u16,
    #[arg(long)]
    pub cursor: Option<String>,
}

#[derive(Args)]
pub struct EventsArgs {
    #[command(flatten)]
    pub access: ContextArgs,
    #[command(flatten)]
    pub selection: SelectionArgs,
    #[arg(long, default_value_t = 25, value_parser = clap::value_parser!(u16).range(1..=100))]
    pub limit: u16,
    #[arg(long)]
    pub cursor: Option<String>,
}

#[derive(Args)]
pub struct ExportArgs {
    #[command(flatten)]
    pub access: ContextArgs,
    #[command(flatten)]
    pub selection: SelectionArgs,
    /// New final directory; incomplete preparation is retained beside it.
    #[arg(long)]
    pub output_dir: PathBuf,
}

#[derive(Args)]
pub struct ResumeExportArgs {
    #[command(flatten)]
    pub access: ContextArgs,
    /// Same final directory requested by the original export.
    #[arg(long)]
    pub output_dir: PathBuf,
}

pub async fn execute(args: HumanArgs) -> Result<()> {
    match args.command {
        HumanCommand::Sso(args) => oidc::execute(args).await,
        HumanCommand::Provision(args) => offline::provision(args),
        HumanCommand::ResumeProvision(args) => offline::resume(args),
        HumanCommand::Apply(args) => offline::apply(args),
        HumanCommand::ProvisionLegacySessionTarget(args) => {
            offline::provision_legacy_session_target(args)
        }
        HumanCommand::PrepareLegacySession(args) => http::prepare_legacy_session(args).await,
        HumanCommand::ConfirmLegacySession(args) => http::confirm_legacy_session(args).await,
        HumanCommand::Identity(args) => http::identity(args).await,
        HumanCommand::Resources(args) => http::resources(args).await,
        HumanCommand::Events(args) => http::events(args).await,
        HumanCommand::Export(args) => export::start(args).await,
        HumanCommand::ResumeExport(args) => export::resume(args).await,
    }
}
