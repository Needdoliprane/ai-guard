//! Operator-owned source adoption. File publication and SQLite migration are
//! separate stages; no status emitted here means human/API readiness.
use agentsandbox::{
    api::{
        EnvelopeExpectationsV1, TechnicalCredentialInput, TechnicalCredentialRoleV1,
        prepare_envelope_configuration, validate_technical_credentials_configuration,
    },
    control_fs::PrivateFile,
    human_authority::migration::operator::{
        self, ExpectedOwnedSourceV1, InputBudget, LegacySourceInput, OperatorMigrationRequest,
        OperatorResolvedView, OperatorSourceOptions, PreparationDirectoryIdentityV1,
        PrivatePreparationDirectory, SourceProfileV1, decode_closed, observe_private_file_binding,
        validate_absolute_path,
    },
    human_authority::{
        HumanAuthorityClosureV1, HumanAuthorityConfigurationV1, HumanConfigurationFileKindV1,
        HumanConfigurationFileV1, HumanOidcConfigurationV1, HumanSourceKindV1,
        SourceStorageBindingV1,
    },
    redaction::Redactor,
};
use anyhow::{Context, Result, bail, ensure};
use clap::{Args, Subcommand};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};
use uuid::Uuid;
mod activation;
mod broker_identity;
mod human_rights;
mod inputs;
mod preparation;
mod schema;
pub(crate) use activation::{LoadedHumanSourcesActivation, load_human_sources_activation};
use inputs::*;
use schema::*;

#[derive(Args)]
pub(crate) struct HumanSourcesArgs {
    #[command(subcommand)]
    pub command: HumanSourcesCommand,
}
#[derive(Subcommand)]
pub(crate) enum HumanSourcesCommand {
    /// Adopt existing Store13/Auth4 files; stopped controls are an attestation.
    Prepare(PrepareArgs),
    /// Finish deterministic publications from the same durable recovery.
    ResumePrepare(StoppedPreparationArgs),
    /// Apply or resume the existing union coordinator, then publish activation.
    Apply(StoppedPreparationArgs),
    /// Resume only an existing migration or a complete published activation.
    Resume(StoppedPreparationArgs),
    /// Validate private metadata files only; no SQLite, owner or writes.
    Check(PreparationArgs),
    /// Offline owned broker identity (proposal): enroll, rotate, revoke, resume, status.
    BrokerIdentity(broker_identity::BrokerIdentityArgs),
    /// Offline in-place identity markers (human rights) on one adopted Store14/15.
    HumanRights(human_rights::HumanRightsArgs),
}
#[derive(Args)]
pub(crate) struct PrepareArgs {
    #[arg(long)]
    pub tenant_registry: PathBuf,
    #[arg(long)]
    pub installation_id: Uuid,
    #[arg(long)]
    pub control_config_revision: u64,
    #[arg(long)]
    pub output_dir: PathBuf,
    #[arg(long, required = true)]
    pub confirm_control_stopped: bool,
    #[arg(long, requires = "oidc_mapping_revision")]
    pub oidc_config: Option<PathBuf>,
    #[arg(long, requires = "oidc_config")]
    pub oidc_mapping_revision: Option<u64>,
    #[arg(long, default_value_t = 100_000)]
    pub max_events: u64,
}
#[derive(Args)]
pub(crate) struct PreparationArgs {
    #[arg(long)]
    pub preparation_dir: PathBuf,
}
#[derive(Args)]
pub(crate) struct StoppedPreparationArgs {
    #[arg(long)]
    pub preparation_dir: PathBuf,
    #[arg(long, required = true)]
    pub confirm_control_stopped: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CheckStatus {
    PreparedFilesValid,
    ActivationFilesValid,
    Incomplete,
    Conflict,
}
impl CheckStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::PreparedFilesValid => "prepared_files_valid",
            Self::ActivationFilesValid => "activation_files_valid",
            Self::Incomplete => "incomplete",
            Self::Conflict => "conflict",
        }
    }
}

pub(crate) async fn run(args: HumanSourcesArgs) -> Result<()> {
    let (step, recovery_action) = match &args.command {
        HumanSourcesCommand::Prepare(_) => (
            "prepare",
            "preserve_and_reconcile; never_reuse_conflicting_output",
        ),
        HumanSourcesCommand::ResumePrepare(_) => (
            "resume-prepare",
            "preserve_and_reconcile; retry_resume-prepare_only_with_unchanged_inputs",
        ),
        HumanSourcesCommand::Apply(_) => ("apply", "preserve_and_reconcile; resume_same_operation"),
        HumanSourcesCommand::Resume(_) => (
            "resume",
            "preserve_and_reconcile; resume_same_operation_only",
        ),
        HumanSourcesCommand::Check(_) => ("check", "inspect_files_only"),
        HumanSourcesCommand::BrokerIdentity(_) => {
            ("broker-identity", "preserve_the_plan_directory")
        }
        HumanSourcesCommand::HumanRights(_) => {
            ("human-rights", "preserve_the_maintenance_directory")
        }
    };
    let result = match args.command {
        HumanSourcesCommand::Prepare(args) => {
            preparation::prepare(args).map(|count| ("prepared_files_valid", count))
        }
        HumanSourcesCommand::ResumePrepare(args) => {
            ensure!(
                args.confirm_control_stopped,
                "human_sources_control_stop_attestation_required"
            );
            preparation::resume_prepare(&args.preparation_dir)
                .map(|count| ("prepared_files_valid", count))
        }
        HumanSourcesCommand::Apply(args) => {
            ensure!(
                args.confirm_control_stopped,
                "human_sources_control_stop_attestation_required"
            );
            activation::apply(&args.preparation_dir, false).await
        }
        HumanSourcesCommand::Resume(args) => {
            ensure!(
                args.confirm_control_stopped,
                "human_sources_control_stop_attestation_required"
            );
            activation::apply(&args.preparation_dir, true).await
        }
        HumanSourcesCommand::Check(args) => {
            println!(
                "{}; files_only; sqlite_not_checked; readiness_not_checked",
                check(&args.preparation_dir).as_str()
            );
            return Ok(());
        }
        // Its own closed report and refusal vocabulary; no shared state here.
        HumanSourcesCommand::BrokerIdentity(args) => return broker_identity::run(args),
        HumanSourcesCommand::HumanRights(args) => return human_rights::run(args),
    };
    match result {
        Ok((state, count)) => {
            println!(
                "{state}; sources={count}; expected_formats=Store14,Auth5_if_selected; readiness_not_checked"
            );
            Ok(())
        }
        Err(error) => {
            let code = if error
                .chain()
                .any(|cause| cause.to_string() == "human_sources_migration_not_started")
            {
                "migration_not_started"
            } else if error.chain().any(|cause| {
                cause.to_string() == "human_sources_recovery_unavailable_or_conflicting"
            }) {
                "recovery_unavailable_or_conflicting"
            } else {
                "conflict_or_incomplete"
            };
            bail!(
                "human_sources_step_refused; step={step}; state={code}; expected_formats=Store13_to14,Auth4_to5_if_selected; action={recovery_action}"
            )
        }
    }
}

pub(crate) fn check(path: &Path) -> CheckStatus {
    match preparation::load_preparation(path, false) {
        Ok(mut loaded) => match activation::validate_activation(&mut loaded) {
            Ok(Some(_)) => CheckStatus::ActivationFilesValid,
            Ok(None) => match loaded
                .directory
                .child("journal", false)
                .and_then(|journal| {
                    journal
                        .map(|j| operator::directory_has_entries(&j))
                        .transpose()
                }) {
                Ok(Some(true)) => CheckStatus::Incomplete,
                Ok(_) => CheckStatus::PreparedFilesValid,
                Err(_) => CheckStatus::Conflict,
            },
            Err(error) => {
                if error
                    .downcast_ref::<activation::ActivationIncomplete>()
                    .is_some()
                {
                    CheckStatus::Incomplete
                } else {
                    CheckStatus::Conflict
                }
            }
        },
        Err(preparation::LoadFailure::Incomplete) => CheckStatus::Incomplete,
        Err(_) => CheckStatus::Conflict,
    }
}

#[cfg(test)]
mod tests;
