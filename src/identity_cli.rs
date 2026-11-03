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
    /// Existing schema4 or schema5 tenant database; never created or migrated implicitly.
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
pub struct ConnectionArgs {
    /// Gateway origin: verified HTTPS, or HTTP on a loopback address.
    #[arg(long)]
    pub gateway: String,
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
        HumanCommand::Identity(args) => http::identity(args).await,
        HumanCommand::Resources(args) => http::resources(args).await,
        HumanCommand::Events(args) => http::events(args).await,
        HumanCommand::Export(args) => export::start(args).await,
        HumanCommand::ResumeExport(args) => export::resume(args).await,
    }
}
