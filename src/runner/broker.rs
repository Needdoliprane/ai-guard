//! Preparation and recovery data for the trusted broker. These values grant no
//! permission to decide, admit or execute a worker.

use super::{
    CheckpointPayloadV1, CredentialFile, FrozenProviderCredentialV1, PlanPreparation, RunOptions,
    RunPlan, choose_plan_prepared, prepare_manual_credentials, prepare_provider_credentials,
    validate_provider_plan,
};
use crate::{
    broker::{
        intent::ExecutionIntentV1,
        journal::{AcceptedJobV1, BindingInputs, BindingKindV1, JournalError, JournalResult},
    },
    control_fs::{Directory, PrivateFile},
    model::DecisionRequest,
};
use anyhow::{Result, anyhow, ensure};
use sha2::{Digest, Sha256};
use std::{fmt, path::Path};
use uuid::Uuid;

mod executor;
pub(crate) use executor::{BatchExecutor, BatchOutcome, deliver_cancellation};
#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
pub(crate) use executor::{ColdRecoverySeed, terminal_pending_seed_for_cold_audit_test};

/// Live, single-use provenance from the executor's freshly acknowledged claim.
/// There is deliberately no public constructor, Clone or serde implementation.
/// Stored claim dispositions can never reconstruct this process-local value.
pub struct FreshStartPermission {
    arm_id: String,
    worker_id: String,
    attempt_id: String,
    control_namespace: String,
    subject_key: String,
    emitter_id: String,
    claim_operation_id: String,
    generation: u64,
}
impl fmt::Debug for FreshStartPermission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FreshStartPermission(<private consumed claim>)")
    }
}
impl FreshStartPermission {
    pub(crate) fn arm_id(&self) -> &str {
        &self.arm_id
    }
    pub(crate) fn worker_id(&self) -> &str {
        &self.worker_id
    }
    pub(crate) fn attempt_id(&self) -> &str {
        &self.attempt_id
    }
    pub(crate) fn control_namespace(&self) -> &str {
        &self.control_namespace
    }
    pub(crate) fn subject_key(&self) -> &str {
        &self.subject_key
    }
    pub(crate) fn emitter_id(&self) -> &str {
        &self.emitter_id
    }
    pub(crate) fn claim_operation_id(&self) -> &str {
        &self.claim_operation_id
    }
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
}

enum PreparedBinding {
    None,
    Manual(PrivateFile),
    Provider(PrivateFile),
}

/// An initial intention and the real retained sources required by Journal::accept.
/// Callers authenticate, select the profile and reserve the operation first.
pub struct PreparedSubmission {
    intent: ExecutionIntentV1,
    binding: PreparedBinding,
    config: Directory,
    launch: Option<crate::runtime::launch_profile::LaunchProfileSelection>,
}

impl fmt::Debug for PreparedSubmission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PreparedSubmission(<private>)")
    }
}

impl PreparedSubmission {
    pub(crate) fn select_minimal_docker_start_profile(
        &mut self,
        runtime: &crate::runtime::Runtime,
    ) -> Result<()> {
        ensure!(
            self.launch.is_none(),
            "broker start profile already selected"
        );
        let options = self.intent.initial_options().run_options();
        let endpoint = options
            .runtime_endpoint
            .as_deref()
            .ok_or_else(|| anyhow!("minimal Docker start profile requires an explicit endpoint"))?;
        self.launch = Some(
            crate::runtime::launch_profile::LaunchProfileSelection::from_runtime(
                runtime, endpoint,
            )?,
        );
        Ok(())
    }
    pub(crate) fn launch_selection(
        &self,
    ) -> Option<&crate::runtime::launch_profile::LaunchProfileSelection> {
        self.launch.as_ref()
    }
    pub fn intent(&self) -> &ExecutionIntentV1 {
        &self.intent
    }

    pub fn binding_inputs(&self) -> BindingInputs<'_> {
        match &self.binding {
            PreparedBinding::None => BindingInputs::None,
            PreparedBinding::Manual(source) => BindingInputs::Manual { source },
            PreparedBinding::Provider(source) => BindingInputs::Provider { source },
        }
    }

    pub fn initial_checkpoint(&self) -> JournalResult<CheckpointPayloadV1> {
        CheckpointPayloadV1::initial(&self.intent)
    }

    pub fn config(&self) -> &Directory {
        &self.config
    }
}

/// Resolve a new batch submission once. Never call this method during replay.
/// Public failures deliberately omit private paths, values and parser causes.
pub fn prepare_broker_submission(
    options: RunOptions,
    operation_id: Uuid,
    control_literals: &[String],
) -> Result<PreparedSubmission> {
    ensure!(
        !operation_id.is_nil(),
        "broker operation UUID must not be nil"
    );
    ensure!(
        options.approve_hash.is_none(),
        "broker submission does not accept approve_hash"
    );
    ensure!(
        !options.interactive,
        "broker submission currently requires batch mode"
    );
    ensure!(
        control_literals.len() <= 64
            && control_literals
                .iter()
                .all(|value| !value.is_empty() && value.len() <= 4096)
            && control_literals.iter().map(String::len).sum::<usize>() <= 256 * 1024,
        "broker control literal context exceeds its bounds"
    );
    prepare_submission(options, operation_id, control_literals)
        .map_err(|_| anyhow!("broker submission preparation failed"))
}

/// Preflight before granting a fresh source capability for an unprepared job.
/// Configuration metadata only; never reads project/provider contents or opens
/// the original credential source. Call on the service's blocking work queue.
pub(crate) fn require_fresh_source(job: &AcceptedJobV1) -> Result<()> {
    let state = BrokerRunState::from_job(job)
        .map_err(|_| anyhow!("broker source preparation is unavailable"))?;
    ensure!(
        state.plan.managed.is_none(),
        "broker job already has an immutable workspace"
    );
    super::managed::broker_require_new_archive(&state.options().config_dir, state.operation_id())
        .map_err(|_| anyhow!("broker immutable workspace requires reconciliation"))
}

fn prepare_submission(
    options: RunOptions,
    operation_id: Uuid,
    control_literals: &[String],
) -> Result<PreparedSubmission> {
    let chosen = choose_plan_prepared(
        options,
        PlanPreparation::Broker {
            operation_id,
            control_literals,
        },
    )?;
    let mut plan = chosen.plan;
    validate_provider_plan(&plan)?;
    let binding = match (plan.credential_source.take(), chosen.provider_record) {
        (None, None) => PreparedBinding::None,
        (Some(source), None) => {
            let material = prepare_manual_credentials(source.bytes())?;
            ensure!(
                !control_literals.iter().any(|literal| {
                    material
                        .bytes
                        .windows(literal.len())
                        .any(|bytes| bytes == literal.as_bytes())
                }),
                "control credential rejected from broker handoff"
            );
            PreparedBinding::Manual(source)
        }
        (None, Some(source)) => PreparedBinding::Provider(source),
        (Some(_), Some(_)) => return Err(anyhow!("conflicting broker bindings")),
    };
    let intent = ExecutionIntentV1::from_projection(
        plan.version,
        plan.operation_id,
        plan.session,
        plan.options,
        plan.request,
        plan.provider_input,
        plan.provider_credential
            .map(|reference| (reference.binding_id, reference.file_name, reference.digest)),
    )?;
    Ok(PreparedSubmission {
        intent,
        binding,
        config: chosen.config,
        launch: None,
    })
}

/// Pure reconstruction of the current runner data, including approval state.
/// It does not resolve providers, inspect sources, or grant runtime authority.
pub struct BrokerRunState {
    // Drafts move through nested journal futures on the broker worker stack.
    plan: Box<RunPlan>,
}

impl fmt::Debug for BrokerRunState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BrokerRunState(<private>)")
    }
}

impl BrokerRunState {
    pub fn from_job(job: &AcceptedJobV1) -> JournalResult<Self> {
        let intent = job.intent();
        intent.validate().map_err(|_| JournalError::InvalidInput)?;
        if job.operation_id() != intent.operation_id() {
            return Err(JournalError::Conflict);
        }
        let checkpoint = job.checkpoint();
        checkpoint.canonical_bytes()?;
        checkpoint.validate_request(intent)?;
        let plan = Box::new(RunPlan {
            version: intent.plan_version(),
            operation_id: intent.operation_id(),
            options: intent.initial_options().run_options(),
            request: checkpoint.0.current_request.clone(),
            session: intent.session(),
            managed: checkpoint.0.managed.clone(),
            provider_input: intent.provider_input().map(|input| input.run_options()),
            provider_credential: intent.provider_credential().map(|reference| {
                FrozenProviderCredentialV1 {
                    binding_id: reference.binding_id(),
                    file_name: reference.file_name().into(),
                    digest: reference.digest().into(),
                }
            }),
            credential_source: None,
        });
        validate_provider_plan(&plan).map_err(|_| JournalError::Conflict)?;
        Ok(Self { plan })
    }

    pub fn operation_id(&self) -> Uuid {
        self.plan.operation_id
    }

    pub fn session(&self) -> Uuid {
        self.plan.session
    }

    pub fn request(&self) -> &DecisionRequest {
        &self.plan.request
    }

    pub fn options(&self) -> &RunOptions {
        &self.plan.options
    }
}

/// A minimal handoff snapshot backed exclusively by the job's durable binding.
/// Values are exposed only to trusted callers for handoff and redaction.
pub struct BrokerCredentials {
    credential: CredentialFile,
    config: Directory,
    handoff_sha: String,
}

impl fmt::Debug for BrokerCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BrokerCredentials(<private>)")
    }
}

impl BrokerCredentials {
    pub fn path(&self) -> &Path {
        self.credential.file.path()
    }

    pub fn names(&self) -> &[String] {
        &self.credential.names
    }

    pub fn secrets(&self) -> &[String] {
        &self.credential.secrets
    }

    pub fn validate(&self) -> Result<()> {
        self.validate_retained()
            .map_err(|_| anyhow!("broker credential binding validation failed"))
    }

    fn validate_retained(&self) -> Result<()> {
        self.config.revalidate()?;
        self.credential._source.persistent_revalidate()?;
        self.credential
            .file
            .private_file()
            .persistent_revalidate()?;
        ensure!(
            self.credential.file.digest() == self.handoff_sha,
            "broker handoff differs from its binding"
        );
        Ok(())
    }
}

/// Create a handoff from the retained durable binding, never the original source
/// path or legacy configuration. The supplied control root must match the intent.
pub fn credentials_from_job(
    job: &AcceptedJobV1,
    config: &Directory,
) -> Result<Option<BrokerCredentials>> {
    credentials_from_binding(job, config)
        .map_err(|_| anyhow!("broker credential binding is unavailable or invalid"))
}

fn credentials_from_binding(
    job: &AcceptedJobV1,
    config: &Directory,
) -> Result<Option<BrokerCredentials>> {
    let options = job.intent().initial_options().run_options();
    ensure!(
        config.path() == options.config_dir,
        "broker configuration differs from its intention"
    );
    config.revalidate()?;
    let manifest = job.binding_manifest();
    if manifest.kind() == BindingKindV1::None {
        ensure!(
            job.binding_source().is_none()
                && options.secret_env_file.is_none()
                && job.intent().provider_credential().is_none(),
            "broker intention requires a credential binding"
        );
        return Ok(None);
    }
    let source = job
        .binding_source()
        .ok_or_else(|| anyhow!("broker credential binding is missing"))?;
    source.persistent_revalidate()?;
    ensure!(
        Some(source.digest()) == manifest.stored_sha()
            && source.bytes().len() as u64 == manifest.stored_bytes(),
        "broker stored binding differs from its manifest"
    );
    let material = match manifest.kind() {
        BindingKindV1::None => return Err(anyhow!("broker binding kind changed")),
        BindingKindV1::Manual => {
            ensure!(
                options.secret_env_file.is_some() && job.intent().provider_credential().is_none(),
                "broker manual binding conflicts with its intention"
            );
            prepare_manual_credentials(source.bytes())?
        }
        BindingKindV1::Provider => {
            let reference = job
                .intent()
                .provider_credential()
                .ok_or_else(|| anyhow!("broker provider reference is missing"))?;
            ensure!(
                options.secret_env_file.is_none()
                    && manifest.binding_id() == Some(reference.binding_id())
                    && manifest.file_name() == Some(reference.file_name())
                    && manifest.source_sha() == Some(source.digest())
                    && reference.digest() == source.digest(),
                "broker provider binding conflicts with its intention"
            );
            prepare_provider_credentials(source.bytes(), reference.binding_id())?
        }
    };
    let handoff_sha = hex::encode(Sha256::digest(&material.bytes));
    ensure!(
        manifest.names() == material.names && manifest.handoff_sha() == Some(handoff_sha.as_str()),
        "broker handoff differs from its manifest"
    );
    let file = config
        .child("credential-snapshots")?
        .snapshot(&material.bytes)?;
    let credentials = BrokerCredentials {
        credential: CredentialFile {
            file,
            _source: source.clone(),
            secrets: material.secrets,
            names: material.names,
        },
        config: config.clone(),
        handoff_sha,
    };
    credentials.validate_retained()?;
    Ok(Some(credentials))
}
