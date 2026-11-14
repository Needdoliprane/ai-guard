//! `human-sources broker-identity`: offline owned broker identity maintenance
//! for the proposed `broker_supervision_owned_v1` profile (decision D1).
//! Proposal submitted to Root: not admitted, activated or qualified.
//!
//! Every subcommand opens the real owned union exactly as the gateway does,
//! either from the published activation or from the explicit owned-source
//! options, which takes the physical owner locks: a running gateway makes the
//! command refuse. Opening the union records a control incarnation in every
//! source, like any gateway start, so even `status` is not a read-only
//! diagnostic. No secret is read from or written to an argument, standard
//! output or a log: the generated bearer exists only in the new private plan
//! directory. Standard output is one closed JSON report; a refusal is one
//! closed, secret-free line on standard error.
use super::load_human_sources_activation;
use crate::gateway_config::{load_owned_sources, load_owned_tenants};
use agentsandbox::{
    broker_registry::BrokerRuntimeKindV1,
    human_authority::{
        HumanAuthorityFailure,
        broker_identity::{
            BrokerEnrollmentRequestV1, BrokerIdentityBackupV1, BrokerIdentityMaintenance,
            BrokerIdentityOutputV1, BrokerIdentityRefusalV1, BrokerIdentityStatusRequestV1,
            BrokerRevocationRequestV1, BrokerRevocationScopeV1, BrokerRotationRequestV1,
            BrokerSelectionV1,
        },
        startup::HumanOwnedStartupBundle,
    },
    redaction::Redactor,
};
use anyhow::{Result, bail};
use clap::{Args, Subcommand, ValueEnum};
use sha2::{Digest, Sha256};
use std::{fmt, io::Write, path::PathBuf};
use uuid::Uuid;

const DEFAULT_MAX_EVENTS: u64 = 100_000;

#[derive(Args)]
pub(crate) struct BrokerIdentityArgs {
    #[command(subcommand)]
    command: BrokerIdentityCommand,
}

#[derive(Subcommand)]
enum BrokerIdentityCommand {
    /// Grant a fresh incarnation and bind a new digest-only bearer; writes a private handoff.
    Enroll(EnrollArgs),
    /// Replace the bearer of the same engaged broker subject; writes a new private handoff.
    Rotate(RotateArgs),
    /// Revoke the grant and/or the bindings of its incarnation; history and digests are kept.
    Revoke(RevokeArgs),
    /// Finish an existing plan by exact replay of its fixed operations.
    Resume(ResumeArgs),
    /// Read the durable grant, current bindings and digest-free history of one broker.
    Status(StatusArgs),
}

/// The owned union, opened like the gateway: published activation (normal
/// route) or the explicit owned-source options.
#[derive(Args)]
struct UnionArgs {
    /// Published private owned-source activation (normal route).
    #[arg(
        long,
        conflicts_with_all = [
            "human_sources_manifest",
            "human_credentials_envelope",
            "tenants_file",
            "human_control_config_revision",
            "oidc_config",
            "human_oidc_mapping_revision",
            "max_events",
        ]
    )]
    human_sources_activation: Option<PathBuf>,
    /// Existing private closure manifest (explicit owned-source route).
    #[arg(
        long,
        requires_all = [
            "human_credentials_envelope",
            "tenants_file",
            "human_control_config_revision",
        ]
    )]
    human_sources_manifest: Option<PathBuf>,
    /// Existing private technical credential envelope of that manifest.
    #[arg(long, requires = "human_sources_manifest")]
    human_credentials_envelope: Option<PathBuf>,
    /// Owned tenant registry of that manifest.
    #[arg(long, requires = "human_sources_manifest")]
    tenants_file: Option<PathBuf>,
    /// Explicit positive revision of the retained public control configuration.
    #[arg(long, requires = "human_sources_manifest")]
    human_control_config_revision: Option<u64>,
    /// Private OIDC configuration of that manifest, when OIDC is selected.
    #[arg(long, requires_all = ["human_sources_manifest", "human_oidc_mapping_revision"])]
    oidc_config: Option<PathBuf>,
    /// Explicit positive OIDC mapping closure revision.
    #[arg(long, requires = "oidc_config")]
    human_oidc_mapping_revision: Option<u64>,
    /// Audit event bound of the explicit route (default 100000).
    #[arg(long, requires = "human_sources_manifest")]
    max_events: Option<u64>,
    /// Operator attestation: gateway, dashboard, ingress and brokers are stopped.
    /// The owner locks are still taken; this is not fencing of an older process.
    #[arg(long, required = true)]
    confirm_control_stopped: bool,
}

/// Operator backup proof of the selected tenant Store.
#[derive(Args)]
struct BackupArgs {
    /// Private copy of the selected tenant Store taken with the control stopped.
    #[arg(long)]
    backup_file: PathBuf,
    /// SHA-256 (64 lowercase hex) recorded for that backup file.
    #[arg(long)]
    backup_sha256: String,
}

#[derive(Clone, Copy, ValueEnum)]
enum RuntimeKind {
    Docker,
    Podman,
}

#[derive(Clone, Copy, ValueEnum)]
enum RevokeScope {
    /// The grant, then every unrevoked binding of its incarnation.
    All,
    Grant,
    Binding,
}

#[derive(Args)]
struct EnrollArgs {
    #[command(flatten)]
    union: UnionArgs,
    #[command(flatten)]
    backup: BackupArgs,
    #[arg(long)]
    tenant_id: String,
    /// Existing broker UUID, or an operator-chosen one for a first registration.
    #[arg(long, conflicts_with = "new_broker", required_unless_present = "new_broker")]
    broker_id: Option<Uuid>,
    /// Register a new random broker UUID.
    #[arg(long)]
    new_broker: bool,
    /// Single authority label of the grant ([A-Za-z0-9._:-], at most 128 bytes).
    #[arg(long)]
    authority_id: String,
    /// Declared runtime scope of that authority; no Kubernetes broker is admitted.
    #[arg(long, value_enum)]
    runtime_kind: RuntimeKind,
    /// Heartbeat lease of the grant, 1 to 300 seconds.
    #[arg(long)]
    lease_seconds: u32,
    /// Grant expiry (RFC 3339), at most 366 days ahead.
    #[arg(long)]
    grant_not_after: String,
    /// Credential expiry (RFC 3339), not after the grant; defaults to it.
    #[arg(long)]
    credential_expires_at: Option<String>,
    /// Canonical gateway origin the broker will use (HTTPS, or HTTP to a loopback IP).
    #[arg(long)]
    gateway_origin: String,
    /// Replace a still active grant of an existing broker (new incarnation).
    #[arg(long)]
    replace_active_grant: bool,
    /// New private plan directory below an existing private parent; also the handoff.
    #[arg(long)]
    plan_dir: PathBuf,
}

#[derive(Args)]
struct RotateArgs {
    #[command(flatten)]
    union: UnionArgs,
    #[command(flatten)]
    backup: BackupArgs,
    #[arg(long)]
    tenant_id: String,
    #[arg(long)]
    broker_id: Uuid,
    /// New credential expiry (RFC 3339), not after the grant; defaults to it.
    #[arg(long)]
    credential_expires_at: Option<String>,
    #[arg(long)]
    gateway_origin: String,
    #[arg(long)]
    plan_dir: PathBuf,
}

#[derive(Args)]
struct RevokeArgs {
    #[command(flatten)]
    union: UnionArgs,
    #[command(flatten)]
    backup: BackupArgs,
    #[arg(long)]
    tenant_id: String,
    #[arg(long)]
    broker_id: Uuid,
    #[arg(long, value_enum, default_value_t = RevokeScope::All)]
    scope: RevokeScope,
    #[arg(long)]
    plan_dir: PathBuf,
}

#[derive(Args)]
struct ResumeArgs {
    #[command(flatten)]
    union: UnionArgs,
    /// Existing plan directory of an interrupted enroll, rotate or revoke.
    #[arg(long)]
    plan_dir: PathBuf,
}

#[derive(Args)]
struct StatusArgs {
    #[command(flatten)]
    union: UnionArgs,
    #[arg(long)]
    tenant_id: String,
    #[arg(long)]
    broker_id: Uuid,
    /// Credential whose digest-free history is shown; defaults to the grant's.
    #[arg(long)]
    credential_id: Option<Uuid>,
    #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..=100))]
    history_limit: u32,
    /// Show revisions strictly below this one (next history page).
    #[arg(long)]
    before_revision: Option<u64>,
}

/// Closed CLI refusal; never a path, a secret or an underlying error text.
enum Refused {
    Union(&'static str),
    Maintenance(BrokerIdentityRefusalV1),
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Union(reason) => write!(
                f,
                "broker_identity_refused; reason={reason}; state=no_step_confirmed_by_this_run; action=stop_every_control_consumer_check_the_owned_union_options_and_retry"
            ),
            Self::Maintenance(refusal) => fmt::Display::fmt(refusal, f),
        }
    }
}

pub(crate) fn run(args: BrokerIdentityArgs) -> Result<()> {
    let (command, outcome) = match args.command {
        BrokerIdentityCommand::Enroll(args) => ("enroll", enroll(args)),
        BrokerIdentityCommand::Rotate(args) => ("rotate", rotate(args)),
        BrokerIdentityCommand::Revoke(args) => ("revoke", revoke(args)),
        BrokerIdentityCommand::Resume(args) => ("resume", resume(args)),
        BrokerIdentityCommand::Status(args) => ("status", status(args)),
    };
    match outcome {
        Ok(output) => std::io::stdout()
            .lock()
            .write_all(output.bytes())
            .map_err(|_| {
                anyhow::anyhow!(
                    "broker_identity_output_unavailable; command={command}; the_durable_outcome_is_unchanged; run_status"
                )
            }),
        Err(refused) => bail!("{refused}; command={command}"),
    }
}

/// Configured runner and viewer tokens: they seed the gateway's redaction
/// dictionary and their digests are refused as a new bearer digest.
fn technical_tokens(tenants: &[crate::gateway_config::ConfiguredTenant]) -> Vec<String> {
    tenants
        .iter()
        .flat_map(|tenant| [tenant.runner_token.clone(), tenant.viewer_token.clone()])
        .collect()
}

/// A running gateway retains the owner locks; its contention is named.
fn union_refusal(error: &anyhow::Error) -> Refused {
    let contended = Some(&HumanAuthorityFailure::OwnerContended);
    if error
        .chain()
        .any(|cause| cause.downcast_ref::<HumanAuthorityFailure>() == contended)
    {
        Refused::Union("owned_union_owner_contended")
    } else {
        Refused::Union("owned_union_open_refused")
    }
}

fn open_union(args: UnionArgs) -> Result<BrokerIdentityMaintenance, Refused> {
    if !args.confirm_control_stopped {
        return Err(Refused::Union("control_stop_attestation_required"));
    }
    let (bundle, tokens): (HumanOwnedStartupBundle, Vec<String>) =
        match (&args.human_sources_activation, &args.human_sources_manifest) {
            (Some(activation), None) => {
                let loaded = load_human_sources_activation(activation)
                    .map_err(|_| Refused::Union("owned_union_activation_refused"))?;
                let tokens = technical_tokens(&loaded.tenants);
                let bundle = HumanOwnedStartupBundle::open(loaded.request)
                    .map_err(|error| union_refusal(&error))?;
                (bundle, tokens)
            }
            (None, Some(manifest)) => {
                let selection = Refused::Union("owned_union_options_incomplete");
                let (Some(envelope), Some(tenants_file), Some(revision)) = (
                    args.human_credentials_envelope.as_deref(),
                    args.tenants_file.as_deref(),
                    args.human_control_config_revision,
                ) else {
                    return Err(selection);
                };
                let tenants = load_owned_tenants(tenants_file)
                    .map_err(|_| Refused::Union("owned_union_tenants_refused"))?;
                let tokens = technical_tokens(&tenants);
                let prepared = load_owned_sources(
                    manifest,
                    envelope,
                    tenants_file,
                    args.oidc_config.as_deref(),
                    &tenants,
                    args.max_events.unwrap_or(DEFAULT_MAX_EVENTS),
                    revision,
                    args.human_oidc_mapping_revision,
                    Redactor::new(tokens.clone()),
                )
                .map_err(|error| union_refusal(&error))?;
                (prepared.bundle, tokens)
            }
            _ => return Err(Refused::Union("owned_union_selection_required")),
        };
    let digests: Vec<[u8; 32]> = tokens
        .iter()
        .map(|token| Sha256::digest(token.as_bytes()).into())
        .collect();
    BrokerIdentityMaintenance::open(bundle, digests).map_err(Refused::Maintenance)
}

fn backup(args: BackupArgs) -> BrokerIdentityBackupV1 {
    BrokerIdentityBackupV1 {
        path: args.backup_file,
        sha256: args.backup_sha256,
    }
}

fn enroll(args: EnrollArgs) -> Result<BrokerIdentityOutputV1, Refused> {
    let broker = match (args.broker_id, args.new_broker) {
        (Some(id), false) => BrokerSelectionV1::Existing(id),
        (None, true) => BrokerSelectionV1::New,
        _ => return Err(Refused::Union("broker_selection_required")),
    };
    let runtime_kind = match args.runtime_kind {
        RuntimeKind::Docker => BrokerRuntimeKindV1::Docker,
        RuntimeKind::Podman => BrokerRuntimeKindV1::Podman,
    };
    let maintenance = open_union(args.union)?;
    maintenance
        .enroll(BrokerEnrollmentRequestV1 {
            tenant_id: args.tenant_id,
            broker,
            authority_id: args.authority_id,
            runtime_kind,
            lease_seconds: args.lease_seconds,
            grant_not_after: args.grant_not_after,
            credential_expires_at: args.credential_expires_at,
            gateway_origin: args.gateway_origin,
            replace_active_grant: args.replace_active_grant,
            backup: backup(args.backup),
            plan_dir: args.plan_dir,
        })
        .map_err(Refused::Maintenance)
}

fn rotate(args: RotateArgs) -> Result<BrokerIdentityOutputV1, Refused> {
    let maintenance = open_union(args.union)?;
    maintenance
        .rotate(BrokerRotationRequestV1 {
            tenant_id: args.tenant_id,
            broker_id: args.broker_id,
            credential_expires_at: args.credential_expires_at,
            gateway_origin: args.gateway_origin,
            backup: backup(args.backup),
            plan_dir: args.plan_dir,
        })
        .map_err(Refused::Maintenance)
}

fn revoke(args: RevokeArgs) -> Result<BrokerIdentityOutputV1, Refused> {
    let scope = match args.scope {
        RevokeScope::All => BrokerRevocationScopeV1::All,
        RevokeScope::Grant => BrokerRevocationScopeV1::Grant,
        RevokeScope::Binding => BrokerRevocationScopeV1::Binding,
    };
    let maintenance = open_union(args.union)?;
    maintenance
        .revoke(BrokerRevocationRequestV1 {
            tenant_id: args.tenant_id,
            broker_id: args.broker_id,
            scope,
            backup: backup(args.backup),
            plan_dir: args.plan_dir,
        })
        .map_err(Refused::Maintenance)
}

fn resume(args: ResumeArgs) -> Result<BrokerIdentityOutputV1, Refused> {
    let maintenance = open_union(args.union)?;
    maintenance
        .resume(&args.plan_dir)
        .map_err(Refused::Maintenance)
}

fn status(args: StatusArgs) -> Result<BrokerIdentityOutputV1, Refused> {
    let maintenance = open_union(args.union)?;
    maintenance
        .status(BrokerIdentityStatusRequestV1 {
            tenant_id: args.tenant_id,
            broker_id: args.broker_id,
            credential_id: args.credential_id,
            history_limit: args.history_limit,
            before_revision: args.before_revision,
        })
        .map_err(Refused::Maintenance)
}
