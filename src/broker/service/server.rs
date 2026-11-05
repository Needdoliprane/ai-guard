//! Trusted broker process. A Unix peer UID routes a private transport; only a
//! fresh gateway check of the presented credential authenticates a request.
use super::{
    AuthenticatedBatchContext, VerifiedProfile,
    jobs::{self, Job, ProfileJobs, RunningJob},
    profile::{ControlProfileV1, Registry, SubmitV1},
    protocol::*,
    targets,
};
use crate::{
    broker::{
        clock::NativeHostClock,
        discovery::RoleLease,
        enforcer::{CancelGeneration, EnforcerClient, EnforcerStopClient, RegistrationV1},
        journal::{
            AcceptedJobV1, CancelStopProjectionV1, ControlSecrets, FsExclusions, Journal,
            JournalLimitsV1, JournalScopeV1, JournalStatusV1, RejectionCodeV1, RejectionPhaseV1,
            RejectionReceiptV1, ReserveResult, canonical,
            recovery::{StopRecoveryReaderV1, StopRecoveryReportV1},
        },
        lifecycle::{LocalCommandReceiptV1, LocalCommandRequestV1},
        local_ipc::{PrivateListener, read_frame, write_frame},
        presence::{PreparedPresence, PresenceError, PresenceScope},
        writer::{
            CancelInputV1, CompletionFailure, ProfileWriterCapabilities, StopWriterClient,
            WriterClient,
        },
    },
    control_fs::{Directory, PrivateFile},
    model::Event,
    redaction::Redactor,
    runner::broker::{
        BatchExecutor, BatchOutcome, BrokerRunState, deliver_cancellation,
        prepare_broker_submission,
    },
    workspace::AuthorizedSource,
};
use anyhow::{Result, anyhow, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    net::UnixStream,
    sync::{OwnedSemaphorePermit, Semaphore, watch},
    task::JoinSet,
};
use uuid::Uuid;

const FRAME_BYTES: u32 = 512 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(15);
const WRITE_TIMEOUT: Duration = Duration::from_secs(3);
const WRITER_TIMEOUT: Duration = Duration::from_secs(10);

#[path = "retained_audit.rs"]
mod retained_audit;

#[cfg(test)]
#[path = "presence_material_tests.rs"]
mod presence_material_tests;

#[cfg(test)]
#[path = "cancel_retry_tests.rs"]
mod cancel_retry_tests;
#[cfg(test)]
type ServiceEnforcer = cancel_retry_tests::TestEnforcer;
#[cfg(not(test))]
type ServiceEnforcer = EnforcerClient;

struct Material {
    literals: Vec<String>,
    files: Vec<PrivateFile>,
    parents: Vec<Directory>,
    exclusions: FsExclusions,
    complete: bool,
}
impl Material {
    fn load(registry: &Registry) -> Result<Self> {
        let mut material = Self {
            literals: Vec::new(),
            files: Vec::new(),
            parents: Vec::new(),
            exclusions: FsExclusions::new(),
            complete: true,
        };
        material.exclusions.add_known_file(
            &registry.source_directory,
            registry
                .source
                .path()
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| anyhow!("invalid private broker registry name"))?,
        )?;
        let paths = registry
            .data
            .profiles
            .iter()
            .map(|p| &p.runner_token_file)
            .chain(registry.data.additional_control_secret_files.iter())
            .collect::<BTreeSet<_>>();
        for path in paths {
            // A missing reconciler key fences new business but does not prevent
            // restoring previously authorized stop targets from other profiles.
            match Self::retain(path, &mut material) {
                Ok(()) => {}
                Err(_) => material.complete = false,
            }
        }
        material.literals.sort();
        material.literals.dedup();
        ControlSecrets::new(material.literals.clone())?;
        Ok(material)
    }
    fn retain(path: &Path, material: &mut Self) -> Result<()> {
        let file = PrivateFile::open(path, 4096)?;
        let parent = Directory::open_existing(
            file.path()
                .parent()
                .ok_or_else(|| anyhow!("invalid control credential parent"))?,
        )?;
        let value = std::str::from_utf8(file.bytes())
            .map_err(|_| anyhow!("invalid private control credential"))?
            .trim();
        ensure!(
            !value.is_empty()
                && value.len() <= 4096
                && value.is_ascii()
                && !value.chars().any(char::is_whitespace)
                && !value.chars().any(char::is_control),
            "invalid private control credential"
        );
        material.exclusions.add_known_file(
            &parent,
            file.path()
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| anyhow!("invalid control credential name"))?,
        )?;
        material.literals.push(value.to_owned());
        material.files.push(file);
        material.parents.push(parent);
        Ok(())
    }
    fn revalidate(&self) -> Result<()> {
        ensure!(self.complete, "broker control credentials are incomplete");
        for file in &self.files {
            file.revalidate()?;
        }
        for parent in &self.parents {
            parent.revalidate()?;
        }
        Ok(())
    }
    fn protect_presence(&mut self, prepared: &PreparedPresence) -> Result<()> {
        for file in prepared.protected_files() {
            file.revalidate()
                .map_err(|_| anyhow!("broker presence protection is unavailable"))?;
            let parent = Directory::open_existing(
                file.path()
                    .parent()
                    .ok_or_else(|| anyhow!("broker presence protection is invalid"))?,
            )
            .map_err(|_| anyhow!("broker presence protection is unavailable"))?;
            let name = file
                .path()
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| anyhow!("broker presence protection is invalid"))?;
            self.exclusions.add_known_file(&parent, name).map_err(|_| {
                anyhow!("broker presence exceeds the configured protection capacity")
            })?;
            if !self.files.iter().any(|known| known.path() == file.path()) {
                self.files.push(file);
            }
            if !self
                .parents
                .iter()
                .any(|known| known.path() == parent.path())
            {
                self.parents.push(parent);
            }
        }
        for root in prepared.protected_roots() {
            root.revalidate()
                .map_err(|_| anyhow!("broker presence protection is unavailable"))?;
            if !self.parents.iter().any(|known| known.path() == root.path()) {
                self.parents.push(root);
            }
        }
        self.literals.extend(prepared.secret_literals());
        self.literals.sort();
        self.literals.dedup();
        ControlSecrets::new(self.literals.clone())
            .map_err(|_| anyhow!("broker presence exceeds the configured secret capacity"))?;
        Ok(())
    }
    fn reconciler(&self, profile: &ControlProfileV1) -> Result<String> {
        // Caller already revalidated the retained files on the bounded I/O
        // pool. Reading these immutable bytes performs no filesystem access.
        let file = self
            .files
            .iter()
            .find(|file| file.path() == profile.runner_token_file)
            .ok_or_else(|| anyhow!("private reconciler credential is unavailable"))?;
        Ok(std::str::from_utf8(file.bytes())
            .map_err(|_| anyhow!("invalid private reconciler credential"))?
            .trim()
            .to_owned())
    }
    fn screen(&self, value: &Value, presented: &str) -> Result<(), BrokerErrorCodeV1> {
        let literals = self
            .literals
            .iter()
            .cloned()
            .chain(std::iter::once(presented.to_owned()))
            .collect::<Vec<_>>();
        let redactor = Redactor::new(literals.clone());
        fn safe(value: &Value, literals: &[String], redactor: &Redactor) -> bool {
            let text = |value: &str| {
                !literals.iter().any(|secret| value.contains(secret))
                    && redactor.text(value) == value
            };
            match value {
                Value::String(value) => text(value),
                Value::Array(values) => values.iter().all(|value| safe(value, literals, redactor)),
                Value::Object(values) => values
                    .iter()
                    .all(|(key, value)| text(key) && safe(value, literals, redactor)),
                _ => true,
            }
        }
        if safe(value, &literals, &redactor) {
            Ok(())
        } else {
            Err(BrokerErrorCodeV1::InvalidInput)
        }
    }
}

struct Service {
    registry: Arc<Registry>,
    root: Directory,
    material: Arc<Material>,
    profiles: BTreeMap<String, Arc<ProfileJobs>>,
    enforcer: Option<ServiceEnforcer>,
    stop_enforcer: Option<EnforcerStopClient>,
    blocking: Arc<Semaphore>,
    preparation: Arc<Semaphore>,
    execution: Arc<Semaphore>,
    stopping: Arc<AtomicBool>,
    fenced: AtomicBool,
}

struct DeferredRun {
    executor: BatchExecutor,
    operation: Uuid,
    context: AuthenticatedBatchContext,
    job: Arc<Job>,
    _running: RunningJob,
    _capacity: OwnedSemaphorePermit,
    stopping: Arc<AtomicBool>,
}
impl DeferredRun {
    fn start(self) {
        tokio::spawn(async move {
            if self.stopping.load(Ordering::SeqCst) {
                self.job.result(Some(BrokerErrorCodeV1::Unavailable));
                return;
            }
            let outcome = self.executor.run(self.operation, &self.context).await;
            match outcome {
                Ok(BatchOutcome::AwaitingReview) => self.job.result(None),
                Ok(_) => {
                    self.job.result(None);
                    let _ = self.job.set_source(None);
                }
                Err(_) => self
                    .job
                    .result(Some(BrokerErrorCodeV1::NeedsReconciliation)),
            }
            // These capabilities remain owned across the complete async run.
            drop(self);
        });
    }
}
struct Response {
    reply: BrokerReplyV1,
    run: Option<DeferredRun>,
}
struct AuthorizedRequest {
    profile: ControlProfileV1,
    verified: VerifiedProfile,
    writer: WriterClient,
    jobs: Arc<ProfileJobs>,
}
impl Response {
    fn reply(reply: BrokerReplyV1) -> Self {
        Self { reply, run: None }
    }
}

/// Prepare all presence protections together before any actor or new helper.
/// Only normal mode with explicit opt-in calls this; STOP-only never opens it.
async fn prepare_presence(
    registry: Arc<Registry>,
) -> Result<(Material, Vec<(String, PreparedPresence)>)> {
    tokio::task::spawn_blocking(move || {
        let mut material = Material::load(&registry)?;
        let mut prepared = Vec::new();
        let mut references = BTreeSet::new();
        for profile in &registry.data.profiles {
            let Some(path) = registry.presence_path(&profile.id) else {
                continue;
            };
            ensure!(
                references.insert(path.to_owned()),
                "broker presence requires a distinct configuration per profile"
            );
            let scope = PresenceScope {
                installation_id: registry.data.installation_id,
                profile_id: profile.id.clone(),
                gateway: profile.gateway.clone(),
                expected_tenant_id: profile.tenant_id.clone(),
                control_namespace_uuid: profile.control_namespace_uuid,
                runtime_kind: match profile.runtime {
                    crate::runtime::RuntimeKind::Docker => {
                        crate::broker_registry::BrokerRuntimeKindV1::Docker
                    }
                    crate::runtime::RuntimeKind::Podman => {
                        crate::broker_registry::BrokerRuntimeKindV1::Podman
                    }
                },
            };
            let presence = PreparedPresence::load(path, scope, material.literals.clone())?;
            prepared.push((profile.id.clone(), presence));
        }
        // Preflight cumulative protection/secret caps before initializing any
        // state. A later profile's key must also protect earlier metadata.
        for (_, presence) in &prepared {
            material.protect_presence(presence)?;
        }
        let mut ready = Vec::new();
        for (profile, mut presence) in prepared {
            match presence.finalize(&material.literals) {
                Ok(()) => {
                    presence.revalidate()?;
                    ready.push((profile, presence));
                }
                Err(error) if error.fences_admission() => return Err(error.into()),
                Err(error) => {
                    // Stream evidence/capacity is independent of business
                    // authority. Keep its static protections even when this
                    // publisher cannot resume. Never repair or reset the data.
                    log_presence_stopped(&profile, error, &material.literals);
                }
            }
        }
        for file in &material.files {
            file.revalidate()
                .map_err(|_| anyhow!("broker presence protection changed"))?;
        }
        for directory in &material.parents {
            directory
                .revalidate()
                .map_err(|_| anyhow!("broker presence protection changed"))?;
        }
        Ok((material, ready))
    })
    .await
    .map_err(|_| anyhow!("broker presence preparation is unavailable"))?
}

fn log_presence_stopped(profile: &str, error: PresenceError, secrets: &[String]) {
    let label = Redactor::new(secrets.to_vec()).text(profile);
    eprintln!(
        "{}",
        json!({"component":"broker_presence","event":"publication_stopped",
            "profile":label,"reason":error.to_string(),
            "retry_action":"contact_operator","runtime_state":"unknown"})
    );
}

/// Run the real leased broker. No service is installed and no business process
/// is ever executed on the host. SIGINT/SIGTERM closes admissions; the distinct
/// helper retains every armed stop and its original deadline.
pub async fn serve(config_path: &Path) -> Result<()> {
    serve_mode(config_path, false).await
}

/// Explicit existing-installation recovery. No Full capability is requested,
/// no new control namespace is bootstrapped and admissions stay fenced.
pub async fn serve_recovery(config_path: &Path) -> Result<()> {
    serve_mode(config_path, true).await
}

async fn serve_mode(config_path: &Path, recovery_only: bool) -> Result<()> {
    let path = config_path.to_owned();
    let registry = Arc::new(tokio::task::spawn_blocking(move || Registry::load(&path)).await??);
    let root = if recovery_only {
        Directory::open_existing(&registry.data.root)
    } else {
        Directory::open_or_create(&registry.data.root)
    }
    .map_err(|_| anyhow!("private broker root is unavailable or unsafe"))?;
    let broker_root = root
        .persistent_child("broker", !recovery_only)?
        .ok_or_else(|| anyhow!("broker role root is unavailable"))?;
    let lease = if recovery_only {
        RoleLease::acquire_existing(&broker_root)?
    } else {
        RoleLease::acquire(&broker_root)?
    };
    let (early_material, prepared_presence) = if !recovery_only
        && registry
            .data
            .profiles
            .iter()
            .any(|profile| registry.presence_path(&profile.id).is_some())
    {
        let (material, presence) = prepare_presence(Arc::clone(&registry)).await?;
        (Some(material), presence)
    } else {
        // Preserve the existing legacy/recovery initialization order. In
        // recovery this branch does not inspect any presence file or state.
        (None, Vec::new())
    };
    let enforcer_root = root
        .persistent_child("enforcer", !recovery_only)?
        .ok_or_else(|| anyhow!("broker stop root is unavailable"))?;
    let executable =
        std::env::current_exe().map_err(|_| anyhow!("broker executable is unavailable"))?;
    let (enforcer, stop_enforcer) = if recovery_only {
        (
            None,
            Some(
                EnforcerStopClient::connect_or_spawn_for_recovery(
                    &enforcer_root,
                    &executable,
                    registry.data.max_enforcer_slots,
                )
                .await?,
            ),
        )
    } else {
        (
            Some(
                EnforcerClient::connect_or_spawn(
                    &enforcer_root,
                    &executable,
                    registry.data.max_enforcer_slots,
                )
                .await?,
            ),
            None,
        )
    };
    let material = if let Some(material) = early_material {
        Arc::new(material)
    } else {
        let material_registry = Arc::clone(&registry);
        Arc::new(tokio::task::spawn_blocking(move || Material::load(&material_registry)).await??)
    };
    let profiles = registry
        .data
        .profiles
        .iter()
        .map(|profile| (profile.id.clone(), Arc::new(ProfileJobs::new())))
        .collect();
    let service = Arc::new(Service {
        execution: Arc::new(Semaphore::new(registry.data.max_enforcer_slots as usize)),
        registry,
        root,
        material,
        profiles,
        #[cfg(test)]
        enforcer: enforcer.map(ServiceEnforcer::Native),
        #[cfg(not(test))]
        enforcer,
        stop_enforcer,
        blocking: Arc::new(Semaphore::new(8)),
        preparation: Arc::new(Semaphore::new(4)),
        stopping: Arc::new(AtomicBool::new(false)),
        fenced: AtomicBool::new(recovery_only),
    });
    service.restore_stops().await;
    let listener_root = if recovery_only {
        lease.listener_root_existing()?
    } else {
        lease.listener_root()?
    };
    let listener = PrivateListener::bind_new(&listener_root)?;
    if recovery_only {
        lease.publish_endpoint_existing(listener.endpoint())?;
    } else {
        lease.publish_endpoint(listener.endpoint())?;
    }
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let connections = Arc::new(Semaphore::new(64));
    let mut requests = JoinSet::new();
    let (presence_shutdown, _) = watch::channel(false);
    let mut presence_tasks = JoinSet::new();
    let presence_attempts = Arc::new(Semaphore::new(4));
    let presence_io = Arc::new(Semaphore::new(2));
    for (profile, presence) in prepared_presence {
        let shutdown = presence_shutdown.subscribe();
        let attempts = Arc::clone(&presence_attempts);
        let io = Arc::clone(&presence_io);
        let owner = Arc::clone(&service);
        presence_tasks.spawn(async move {
            if let Err(error) = presence.run(shutdown, attempts, io).await {
                if error.fences_admission() {
                    owner.fenced.store(true, Ordering::SeqCst);
                }
                log_presence_stopped(&profile, error, &owner.material.literals);
            }
        });
    }
    // An autonomous reconciler uses only the configured private runner key,
    // never as a substitute for an unauthenticated user's request.
    let reconciler_service = Arc::clone(&service);
    let reconciler = tokio::spawn(async move {
        reconciler_service.reconcile_loop().await;
    });
    loop {
        tokio::select! {
            _ = terminate.recv() => break,
            _ = interrupt.recv() => break,
            Some(_) = requests.join_next(), if !requests.is_empty() => {},
            Some(result) = presence_tasks.join_next(), if !presence_tasks.is_empty() => {
                if result.is_err() {
                    eprintln!("{}", json!({"component":"broker_presence",
                        "event":"task_failed","retry_action":"contact_operator",
                        "runtime_state":"unknown"}));
                }
            },
            accepted = listener.accept_checked() => {
                match accepted {
                    Ok(mut stream) => match Arc::clone(&connections).try_acquire_owned() {
                        Ok(capacity) => {
                            let service = Arc::clone(&service);
                            requests.spawn(async move { let _capacity = capacity; service.connection(stream).await; });
                        }
                        Err(_) => {
                            // No unbounded response tasks for a saturated peer.
                            let _ = write_frame(&mut stream, &ReplyEnvelopeV1 { version: 1,
                                reply: BrokerReplyV1::Error { code: BrokerErrorCodeV1::Busy } }, FRAME_BYTES, WRITE_TIMEOUT).await;
                        }
                    },
                    Err(_) => { service.fenced.store(true, Ordering::SeqCst); break; },
                }
            }
        }
    }
    service.stopping.store(true, Ordering::SeqCst);
    let _ = presence_shutdown.send(true);
    for profile in service.profiles.values() {
        profile.invalidate_all();
    }
    reconciler.abort();
    // HTTP exists only in these owned futures. A cancelled blocking filesystem
    // closure may finish later while retaining its lock; it cannot send a POST.
    let presence_drain = async { while presence_tasks.join_next().await.is_some() {} };
    if tokio::time::timeout(Duration::from_secs(15), presence_drain)
        .await
        .is_err()
    {
        presence_tasks.abort_all();
        while presence_tasks.join_next().await.is_some() {}
    }
    // In-flight framing/writer ACKs receive a short bounded drain. A lost reply
    // never rolls back an already durable accepted job or enforcer registration.
    let drain = async { while requests.join_next().await.is_some() {} };
    let _ = tokio::time::timeout(Duration::from_secs(5), drain).await;
    requests.abort_all();
    drop(listener);
    drop(lease);
    Ok(())
}

impl Service {
    fn recovery_only(&self) -> bool {
        self.stop_enforcer.is_some()
    }
    fn full_enforcer(&self) -> Result<&EnforcerClient, BrokerErrorCodeV1> {
        if self.recovery_only() {
            return Err(BrokerErrorCodeV1::NeedsReconciliation);
        }
        let enforcer = self
            .enforcer
            .as_ref()
            .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
        Ok(enforcer)
    }
    fn profile_jobs(&self, profile: &str) -> Result<Arc<ProfileJobs>, BrokerErrorCodeV1> {
        self.profiles
            .get(profile)
            .cloned()
            .ok_or(BrokerErrorCodeV1::Unauthorized)
    }
    async fn filesystem<T: Send + 'static>(
        &self,
        action: impl FnOnce() -> Result<T> + Send + 'static,
    ) -> Result<T, BrokerErrorCodeV1> {
        let capacity = Arc::clone(&self.blocking)
            .acquire_owned()
            .await
            .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
        tokio::task::spawn_blocking(move || {
            let _capacity = capacity;
            action()
        })
        .await
        .map_err(|_| BrokerErrorCodeV1::Unavailable)?
        .map_err(|error| {
            error
                .downcast_ref::<BrokerErrorCodeV1>()
                .copied()
                .unwrap_or(BrokerErrorCodeV1::NeedsReconciliation)
        })
    }
    async fn validate_control(&self) -> Result<(), BrokerErrorCodeV1> {
        let registry = Arc::clone(&self.registry);
        let material = Arc::clone(&self.material);
        let root = self.root.clone();
        self.filesystem(move || {
            registry.source.revalidate()?;
            registry.source_directory.revalidate()?;
            root.revalidate()?;
            material.revalidate()
        })
        .await
    }
    async fn validate_registry(&self) -> Result<(), BrokerErrorCodeV1> {
        let registry = Arc::clone(&self.registry);
        let root = self.root.clone();
        self.filesystem(move || {
            registry.source.revalidate()?;
            registry.source_directory.revalidate()?;
            root.revalidate()
        })
        .await
    }
    async fn writer(
        &self,
        profile: &str,
        scope: &JournalScopeV1,
    ) -> Result<WriterClient, BrokerErrorCodeV1> {
        // Status/List's full business view is unavailable in this explicitly
        // restricted service. RecoveryStatus/Cancel use the STOP facade below.
        // Refuse before the lazy initializer can acquire or create a Full actor.
        if self.recovery_only() {
            return Err(BrokerErrorCodeV1::NeedsReconciliation);
        }
        self.profile_writers(profile, scope, true)
            .await?
            .full()
            .ok_or(BrokerErrorCodeV1::NeedsReconciliation)
    }

    async fn profile_writers(
        &self,
        profile: &str,
        scope: &JournalScopeV1,
        allow_create: bool,
    ) -> Result<ProfileWriterCapabilities, BrokerErrorCodeV1> {
        let recovery_only = self.recovery_only();
        let allow_create = allow_create && !recovery_only;
        let jobs = self.profile_jobs(profile)?;
        let root = self.root.clone();
        let scope = scope.clone();
        let open_scope = scope.clone();
        let material = Arc::clone(&self.material);
        let writer = jobs
            .writer
            .get_or_try_init(|| async {
                self.filesystem(move || {
                    // Missing required history is not evidence of an empty
                    // journal. Keep the creating Full path's defensive error.
                    let missing_history = if allow_create {
                        BrokerErrorCodeV1::InvalidInput
                    } else {
                        BrokerErrorCodeV1::NeedsReconciliation
                    };
                    let journals = root
                        .persistent_child("journals", allow_create)?
                        .ok_or(missing_history)?;
                    let profile = journals
                        .persistent_child(open_scope.profile_id(), allow_create)?
                        .ok_or(missing_history)?;
                    let control = ControlSecrets::new(material.literals.clone())?;
                    if recovery_only {
                        let reader = StopRecoveryReaderV1::open_existing(
                            &profile,
                            open_scope,
                            &material.exclusions,
                            JournalLimitsV1::default(),
                            control,
                        )?
                        .ok_or(missing_history)?;
                        let report = reader.scan();
                        return Ok(ProfileWriterCapabilities::from_recovery_report(
                            &report,
                            true,
                            64,
                            WRITER_TIMEOUT,
                        )?);
                    }
                    if let Some(existing) = ProfileWriterCapabilities::open_existing(
                        &profile,
                        open_scope.clone(),
                        &material.exclusions,
                        JournalLimitsV1::default(),
                        control.clone(),
                        64,
                        WRITER_TIMEOUT,
                    )? {
                        return Ok(existing);
                    }
                    ensure!(allow_create, missing_history);
                    let journal = Journal::open(
                        &profile,
                        open_scope,
                        &material.exclusions,
                        JournalLimitsV1::default(),
                        control,
                    )?;
                    Ok(ProfileWriterCapabilities::from_full(WriterClient::start(
                        journal,
                        64,
                        WRITER_TIMEOUT,
                    )?))
                })
                .await
            })
            .await?;
        if canonical(writer.stops().scope()).map_err(jobs::journal_error)?
            != canonical(&scope).map_err(jobs::journal_error)?
        {
            return Err(BrokerErrorCodeV1::Conflict);
        }
        if recovery_only && !writer.is_stop_only() {
            self.fenced.store(true, Ordering::SeqCst);
            return Err(BrokerErrorCodeV1::NeedsReconciliation);
        }
        if writer.is_stop_only() {
            self.fenced.store(true, Ordering::SeqCst);
        }
        Ok(writer.clone())
    }

    async fn recovery_status_for_profile(
        &self,
        profile: &ControlProfileV1,
        scope: &JournalScopeV1,
        operation: Uuid,
    ) -> Result<Response, BrokerErrorCodeV1> {
        if operation.is_nil() {
            return Err(BrokerErrorCodeV1::InvalidInput);
        }
        let capabilities = self.profile_writers(&profile.id, scope, false).await?;
        let snapshot = capabilities
            .stops()
            .recovery_status(operation)
            .await
            .map_err(jobs::journal_error)?
            .ok_or(BrokerErrorCodeV1::InvalidInput)?;
        if snapshot.operation_id() != operation {
            return Err(BrokerErrorCodeV1::Conflict);
        }
        if canonical(snapshot.scope()).map_err(jobs::journal_error)?
            != canonical(scope).map_err(jobs::journal_error)?
        {
            return Err(BrokerErrorCodeV1::Conflict);
        }
        if let Some(job) = self.profile_jobs(&profile.id)?.known(operation) {
            let commands = snapshot
                .cancel_receipts()
                .iter()
                .map(LocalCommandReceiptV1::command_id)
                .collect::<Vec<_>>();
            job.remember_cancel_ids(&commands)?;
        }
        let known_arms = snapshot
            .stops()
            .iter()
            .map(|stop| {
                let arm = stop.arm();
                RecoveryTargetV1 {
                    arm_id: arm.operation_id.clone(),
                    worker_id: arm.worker_id.clone(),
                    attempt_id: arm.attempt_id.clone(),
                    subject: match &arm.subject {
                        crate::control::ArmSubjectV1::Worker => RecoveryTargetKindV1::Worker,
                        crate::control::ArmSubjectV1::Helper { helper_id, .. } => {
                            RecoveryTargetKindV1::Helper {
                                helper_id: helper_id.clone(),
                            }
                        }
                    },
                    original_clock: arm.clock.clone(),
                    original_timeout_seconds: arm.timeout_seconds,
                    original_deadline_ns_decimal: arm.deadline_ns_decimal.clone(),
                    cancel_command_id: stop.cancel_after_arm(),
                }
            })
            .collect();
        Ok(Response::reply(BrokerReplyV1::RecoveryStatus {
            status: RecoveryStatusV1 {
                version: 1,
                operation_id: operation,
                tenant_id: snapshot.scope().tenant_id().to_owned(),
                journal_mode: if capabilities.is_stop_only() {
                    JournalModeV1::StopOnly
                } else {
                    JournalModeV1::Full
                },
                source: RecoveryDataSourceV1::ConfirmedJournal,
                runtime_evidence: RecoveryRuntimeEvidenceV1::NotQueried,
                head_revision: snapshot.head().revision(),
                head_hash: snapshot.head().hash().to_owned(),
                incomplete: snapshot.incomplete(),
                reconciliation_required: capabilities.is_stop_only() || snapshot.incomplete(),
                cancel_commands: snapshot
                    .cancel_receipts()
                    .iter()
                    .map(|receipt| RecoveryCommandV1 {
                        command_id: receipt.command_id(),
                        revision: receipt.head().revision(),
                        checkpoint_hash: receipt.head().hash().to_owned(),
                    })
                    .collect(),
                known_arms,
                pending_arms: snapshot.pending_arms(),
            },
        }))
    }

    async fn recovery_inventory_for_profile(
        &self,
        profile: &ControlProfileV1,
        scope: &JournalScopeV1,
        after: Option<Uuid>,
        limit: u32,
    ) -> Result<Response, BrokerErrorCodeV1> {
        if !(1..=100).contains(&limit) || after.is_some_and(|id| id.is_nil()) {
            return Err(BrokerErrorCodeV1::InvalidInput);
        }
        // A normal service must not change admission/fence state because of a
        // cold inventory read. Only explicit recovery mode may bootstrap Stops;
        // an already-retained Full/Stops queue is reused without conversion.
        if !self.recovery_only() && self.profile_jobs(&profile.id)?.writer.get().is_none() {
            return Err(BrokerErrorCodeV1::NeedsReconciliation);
        }
        let capabilities = self.profile_writers(&profile.id, scope, false).await?;
        let page = capabilities
            .stops()
            .recovery_inventory(after, limit)
            .await
            .map_err(jobs::journal_error)?;
        if canonical(page.scope()).map_err(jobs::journal_error)?
            != canonical(scope).map_err(jobs::journal_error)?
        {
            return Err(BrokerErrorCodeV1::Conflict);
        }
        let mode = if capabilities.is_stop_only() {
            JournalModeV1::StopOnly
        } else {
            JournalModeV1::Full
        };
        Ok(Response::reply(BrokerReplyV1::RecoveryInventory {
            inventory: super::recovery_inventory::project(&page, mode),
        }))
    }

    async fn context(
        &self,
        mut verified: VerifiedProfile,
        profile: ControlProfileV1,
        job: &Job,
        generation: u64,
        source: Option<Arc<AuthorizedSource>>,
    ) -> Result<AuthenticatedBatchContext, BrokerErrorCodeV1> {
        let registry = Arc::clone(&self.registry);
        let root = self.root.clone();
        let material = Arc::clone(&self.material);
        let cancel = job.cancel.clone();
        self.filesystem(move || {
            registry.source.revalidate()?;
            registry.source_directory.revalidate()?;
            material.revalidate()?;
            ensure!(
                cancel.current() == generation,
                "broker command generation changed"
            );
            verified.client = verified.client.with_secrets(material.literals.clone());
            let mut context = verified.open_context(&registry, &profile, &root, cancel, source)?;
            ensure!(
                context.expected_generation() == generation
                    && context.cancel_generation().current() == generation,
                "broker command generation changed"
            );
            for parent in &material.parents {
                context.exclusions.push(parent.try_clone_file()?);
            }
            Ok(context)
        })
        .await
    }
    fn executor(&self, writer: WriterClient) -> Result<BatchExecutor, BrokerErrorCodeV1> {
        let clock = NativeHostClock::new().map_err(|_| BrokerErrorCodeV1::Unavailable)?;
        Ok(BatchExecutor::new(
            writer,
            self.full_enforcer()?.clone(),
            Arc::new(clock),
        ))
    }
    async fn connection(self: Arc<Self>, mut stream: UnixStream) {
        let response = match read_frame::<EnvelopeV1>(&mut stream, FRAME_BYTES, READ_TIMEOUT).await
        {
            Ok(envelope) => match self.dispatch(envelope).await {
                Ok(response) => response,
                Err(code) => Response::reply(BrokerReplyV1::Error { code }),
            },
            Err(_) => Response::reply(BrokerReplyV1::Error {
                code: BrokerErrorCodeV1::InvalidInput,
            }),
        };
        let _ = write_frame(
            &mut stream,
            &ReplyEnvelopeV1 {
                version: 1,
                reply: response.reply,
            },
            FRAME_BYTES,
            WRITE_TIMEOUT,
        )
        .await;
        // Client disappearance does not cancel a durable accepted job.
        if let Some(run) = response.run {
            run.start();
        }
    }
    async fn dispatch(
        self: &Arc<Self>,
        envelope: EnvelopeV1,
    ) -> Result<Response, BrokerErrorCodeV1> {
        if envelope.version != 1 || envelope.profile_id.is_empty() || envelope.profile_id.len() > 80
        {
            return Err(BrokerErrorCodeV1::InvalidInput);
        }
        let read_only = matches!(
            envelope.request,
            BrokerCommandV1::Status { .. }
                | BrokerCommandV1::List { .. }
                | BrokerCommandV1::RecoveryStatus { .. }
                | BrokerCommandV1::RecoveryInventory { .. }
        );
        let stop_only = matches!(envelope.request, BrokerCommandV1::Cancel { .. });
        let verified = VerifiedProfile::authenticate(
            &self.registry,
            &envelope.profile_id,
            &envelope.credential,
            !read_only && !stop_only,
        )
        .await?;
        if self.recovery_only() && !read_only && !stop_only {
            // Authentication still precedes this refusal. No writer, submit
            // reservation, approval, source, export target or fresh job is opened.
            return Err(BrokerErrorCodeV1::NeedsReconciliation);
        }
        if read_only || matches!(envelope.request, BrokerCommandV1::Cancel { .. }) {
            self.validate_registry().await?;
        } else {
            self.validate_control().await?;
        }
        let profile = self
            .registry
            .profile(&envelope.profile_id)
            .map_err(|_| BrokerErrorCodeV1::Forbidden)?
            .clone();
        if let BrokerCommandV1::RecoveryStatus { operation_id } = &envelope.request {
            return self
                .recovery_status_for_profile(&profile, &verified.scope, *operation_id)
                .await;
        }
        if let BrokerCommandV1::RecoveryInventory { after, limit } = &envelope.request {
            return self
                .recovery_inventory_for_profile(&profile, &verified.scope, *after, *limit)
                .await;
        }
        if let BrokerCommandV1::Cancel {
            operation_id,
            command_id,
        } = &envelope.request
        {
            return self
                .cancel_for_profile(
                    &profile,
                    &verified.scope,
                    self.profile_jobs(&profile.id)?,
                    *operation_id,
                    *command_id,
                )
                .await;
        }
        let writer = self.writer(&profile.id, &verified.scope).await?;
        let jobs = self.profile_jobs(&profile.id)?;
        if !read_only && self.stopping.load(Ordering::SeqCst) {
            return Err(BrokerErrorCodeV1::Unavailable);
        }
        let request = AuthorizedRequest {
            profile,
            verified,
            writer,
            jobs,
        };
        match envelope.request {
            BrokerCommandV1::Submit { payload } => {
                self.submit(request, payload, &envelope.credential).await
            }
            BrokerCommandV1::Status { operation_id } => {
                let durable = request
                    .writer
                    .status(operation_id)
                    .await
                    .map_err(jobs::journal_error)?;
                let local = request.jobs.known(operation_id);
                Ok(Response::reply(BrokerReplyV1::Status {
                    status: jobs::status(operation_id, durable, local.as_deref()),
                }))
            }
            BrokerCommandV1::List { after, limit } => {
                if !(1..=100).contains(&limit) || after.is_some_and(|id| id.is_nil()) {
                    return Err(BrokerErrorCodeV1::InvalidInput);
                }
                let page = request
                    .writer
                    .list(after, limit)
                    .await
                    .map_err(jobs::journal_error)?;
                let mut entries = Vec::with_capacity(page.entries().len());
                for entry in page.entries() {
                    let local = request.jobs.known(entry.operation_id());
                    let mut status = jobs::entry_status(entry, local.as_deref());
                    if entry.kind() == crate::broker::journal::JournalEntryKindV1::Accepted {
                        let durable = request
                            .writer
                            .load(entry.operation_id())
                            .await
                            .map_err(jobs::journal_error)?
                            .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
                        status.last_decision = jobs::last_decision(&durable);
                    }
                    entries.push(status);
                }
                Ok(Response::reply(BrokerReplyV1::Page {
                    entries,
                    next_after: page.next_after(),
                }))
            }
            BrokerCommandV1::Approve {
                operation_id,
                command_id,
                request_hash,
            } => {
                self.approve(request, operation_id, command_id, request_hash)
                    .await
            }
            BrokerCommandV1::Cancel { .. }
            | BrokerCommandV1::RecoveryStatus { .. }
            | BrokerCommandV1::RecoveryInventory { .. } => Err(BrokerErrorCodeV1::InvalidInput),
            BrokerCommandV1::Resume {
                operation_id,
                command_id,
            } => self.resume(request, operation_id, command_id).await,
            BrokerCommandV1::Export {
                operation_id,
                command_id,
                destination,
            } => {
                self.material.screen(
                    &serde_json::to_value(&destination)
                        .map_err(|_| BrokerErrorCodeV1::InvalidInput)?,
                    &envelope.credential,
                )?;
                self.export(request, operation_id, command_id, destination)
                    .await
            }
        }
    }
    async fn require_starts(&self) -> Result<(), BrokerErrorCodeV1> {
        if self.recovery_only() || self.fenced.load(Ordering::SeqCst) {
            Err(BrokerErrorCodeV1::NeedsReconciliation)
        } else if self.stopping.load(Ordering::SeqCst) {
            Err(BrokerErrorCodeV1::Unavailable)
        } else {
            self.full_enforcer()?
                .require_admission_ready()
                .await
                .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)
        }
    }
    async fn reject_export_command_reuse(
        &self,
        job: &AcceptedJobV1,
        command: Uuid,
    ) -> Result<(), BrokerErrorCodeV1> {
        if job.broker_checkpoint_v2().is_some_and(|cp| {
            cp.supervision().operations().iter().any(|operation| {
                matches!(operation,
                    crate::broker::checkpoint::SupervisionOperationV1::Export { request, .. }
                    if request.operation_id == command.to_string()
                )
            })
        }) {
            return Err(BrokerErrorCodeV1::Conflict);
        }
        let root = self.root.clone();
        let profile = job.scope().profile_id().to_owned();
        self.filesystem(move || {
            targets::reject_reserved_id(&root, &profile, command).map_err(anyhow::Error::new)
        })
        .await
    }
    async fn submit(
        self: &Arc<Self>,
        request: AuthorizedRequest,
        payload: String,
        credential: &str,
    ) -> Result<Response, BrokerErrorCodeV1> {
        let AuthorizedRequest {
            profile,
            verified,
            writer,
            jobs,
        } = request;
        let submit =
            SubmitV1::parse(payload.as_bytes()).map_err(|_| BrokerErrorCodeV1::InvalidInput)?;
        let parsed: Value =
            serde_json::from_str(&payload).map_err(|_| BrokerErrorCodeV1::InvalidInput)?;
        self.material.screen(&parsed, credential)?;
        let operation = submit.operation_id;
        let job = jobs.job(operation)?;
        let preparation_generation = job.cancel.current();
        // Reserve the exact runnable payload before opening a provider or a
        // source directory. The authentication envelope is never persisted.
        let ticket = match writer
            .reserve(operation, payload.into_bytes())
            .await
            .map_err(jobs::journal_error)?
        {
            ReserveResult::Accepted(job) => return Ok(Response::reply(accepted(&job, true))),
            ReserveResult::Rejected(receipt) => {
                return Ok(Response::reply(rejected(&receipt, true)));
            }
            ReserveResult::InProgress => return Err(BrokerErrorCodeV1::Busy),
            ReserveResult::Interrupted => return Err(BrokerErrorCodeV1::Interrupted),
            ReserveResult::New(ticket) => ticket,
        };
        let running = match job.enter() {
            Ok(running) => running,
            Err(error) => {
                job.result(Some(error));
                let code = if job.cancel.current() != preparation_generation {
                    RejectionCodeV1::CancelledBeforeAcceptance
                } else {
                    RejectionCodeV1::PreparationFailed
                };
                let receipt = complete_rejection(&writer, operation, ticket, &job, code).await?;
                return Ok(Response::reply(rejected(&receipt, false)));
            }
        };
        let preparation = match Arc::clone(&self.preparation).acquire_owned().await {
            Ok(preparation) => preparation,
            Err(_) => {
                job.result(Some(BrokerErrorCodeV1::Unavailable));
                let code = if job.cancel.current() != preparation_generation {
                    RejectionCodeV1::CancelledBeforeAcceptance
                } else {
                    RejectionCodeV1::PreparationFailed
                };
                let receipt = complete_rejection(&writer, operation, ticket, &job, code).await?;
                return Ok(Response::reply(rejected(&receipt, false)));
            }
        };
        // Every failure after reservation completes the same durable ticket.
        let prepared = async {
            self.require_starts().await?;
            if job.cancel.current() != preparation_generation {
                return Err(BrokerErrorCodeV1::Interrupted);
            }
            self.validate_control().await?;
            let source_profile = profile.clone();
            let (source, options) = self
                .filesystem(move || {
                    let source = Arc::new(AuthorizedSource::open(
                        &submit.project,
                        &source_profile.source_roots,
                    )?);
                    let options = submit.options(&source_profile, &source)?;
                    Ok((source, options))
                })
                .await?;
            let mut literals = self.material.literals.clone();
            literals.push(credential.to_owned());
            let mut prepared = self
                .filesystem(move || prepare_broker_submission(options, operation, &literals))
                .await?;
            if job.cancel.current() != preparation_generation {
                return Err(BrokerErrorCodeV1::Interrupted);
            }
            let start_profile = self.registry.start_profile(&profile.id);
            let context = self
                .context(
                    verified,
                    profile,
                    &job,
                    preparation_generation,
                    Some(Arc::clone(&source)),
                )
                .await?;
            if start_profile == super::profile::PrearmedStartProfile::DockerMinimal {
                prepared
                    .select_minimal_docker_start_profile(context.runtime())
                    .map_err(|_| BrokerErrorCodeV1::InvalidInput)?;
            }
            if context.expected_generation() != preparation_generation
                || job.cancel.current() != preparation_generation
            {
                return Err(BrokerErrorCodeV1::Interrupted);
            }
            job.set_source(Some(source))?;
            let executor = self.executor(writer.clone())?;
            Ok::<_, BrokerErrorCodeV1>((prepared, context, executor))
        }
        .await;
        drop(preparation);
        let (prepared, context, executor) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                job.result(Some(error));
                let code = if job.cancel.current() != preparation_generation {
                    RejectionCodeV1::CancelledBeforeAcceptance
                } else {
                    RejectionCodeV1::PreparationFailed
                };
                let receipt = complete_rejection(&writer, operation, ticket, &job, code).await?;
                return Ok(Response::reply(rejected(&receipt, false)));
            }
        };
        let accepted_job =
            match complete_acceptance(&writer, ticket, prepared, &job, preparation_generation)
                .await?
            {
                Acceptance::Accepted(job) => job,
                Acceptance::Rejected(receipt) => {
                    job.set_source(None)?;
                    return Ok(Response::reply(rejected(&receipt, false)));
                }
            };
        let capacity = Arc::clone(&self.execution)
            .acquire_owned()
            .await
            .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
        Ok(Response {
            reply: accepted(&accepted_job, false),
            run: Some(DeferredRun {
                executor,
                operation,
                context,
                job,
                _running: running,
                _capacity: capacity,
                stopping: Arc::clone(&self.stopping),
            }),
        })
    }
    async fn load_job(
        &self,
        writer: &WriterClient,
        operation: Uuid,
        profile: &ControlProfileV1,
    ) -> Result<AcceptedJobV1, BrokerErrorCodeV1> {
        let job = writer
            .load(operation)
            .await
            .map_err(jobs::journal_error)?
            .ok_or(BrokerErrorCodeV1::InvalidInput)?;
        jobs::require_profile(&job, profile)?;
        Ok(job)
    }
    async fn approve(
        &self,
        request: AuthorizedRequest,
        operation: Uuid,
        command: Uuid,
        hash: String,
    ) -> Result<Response, BrokerErrorCodeV1> {
        let AuthorizedRequest {
            profile,
            verified,
            writer,
            jobs,
        } = request;
        check_ids(operation, command)?;
        let durable = self.load_job(&writer, operation, &profile).await?;
        reject_event_command_reuse(&durable, command)?;
        self.reject_export_command_reuse(&durable, command).await?;
        if let Some(existing) = durable
            .broker_checkpoint_v3()
            .and_then(|cp| cp.commands().iter().find(|c| c.command_id() == command))
        {
            let LocalCommandRequestV1::ApprovalSubmitted {
                request_hash,
                review_receipt,
            } = existing.request()
            else {
                return Err(BrokerErrorCodeV1::Conflict);
            };
            if request_hash != &hash {
                return Err(BrokerErrorCodeV1::Conflict);
            }
            let acknowledgement = writer
                .append_command(
                    operation,
                    command,
                    LocalCommandRequestV1::ApprovalSubmitted {
                        request_hash: request_hash.clone(),
                        review_receipt: review_receipt.clone(),
                    },
                )
                .await
                .map_err(jobs::journal_error)?;
            return Ok(Response::reply(command_ack(operation, &acknowledgement)));
        }
        self.require_starts().await?;
        let identity = jobs.identity_gate().await?;
        let job = jobs.job(operation)?;
        let running = job.enter()?;
        let durable = self.load_job(&writer, operation, &profile).await?;
        reject_event_command_reuse(&durable, command)?;
        self.reject_export_command_reuse(&durable, command).await?;
        let (actual, receipt) = jobs::review(&durable).ok_or(BrokerErrorCodeV1::Conflict)?;
        if actual != hash {
            return Err(BrokerErrorCodeV1::Conflict);
        }
        let acknowledgement = writer
            .append_command(
                operation,
                command,
                LocalCommandRequestV1::ApprovalSubmitted {
                    request_hash: hash,
                    review_receipt: receipt,
                },
            )
            .await
            .map_err(jobs::journal_error)?;
        drop(identity);
        let source = if durable.checkpoint().broker_context().managed {
            None
        } else {
            job.source()?
        };
        let context = self
            .context(verified, profile, &job, running.generation(), source)
            .await?;
        let capacity = Arc::clone(&self.execution)
            .acquire_owned()
            .await
            .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
        let executor = self.executor(writer).inspect_err(|code| {
            job.result(Some(*code));
        })?;
        Ok(Response {
            reply: command_ack(operation, &acknowledgement),
            run: Some(DeferredRun {
                executor,
                operation,
                context,
                job,
                _running: running,
                _capacity: capacity,
                stopping: Arc::clone(&self.stopping),
            }),
        })
    }
    async fn cancel_for_profile(
        &self,
        profile: &ControlProfileV1,
        scope: &JournalScopeV1,
        jobs: Arc<ProfileJobs>,
        operation: Uuid,
        command: Uuid,
    ) -> Result<Response, BrokerErrorCodeV1> {
        check_ids(operation, command)?;
        let writer = self
            .profile_writers(&profile.id, scope, false)
            .await?
            .stops();
        if self.stopping.load(Ordering::SeqCst) {
            return Err(BrokerErrorCodeV1::Unavailable);
        }
        self.cancel(writer, jobs, operation, command).await
    }
    async fn cancel(
        &self,
        writer: StopWriterClient,
        jobs: Arc<ProfileJobs>,
        operation: Uuid,
        command: Uuid,
    ) -> Result<Response, BrokerErrorCodeV1> {
        check_ids(operation, command)?;
        let job = jobs.job(operation)?;
        // Invalidate before the first awaited journal operation. A blocked
        // writer cannot leave an already issued spawn guard locally valid.
        // A confirmed replay has no new invalidation or execution gate.
        let mut cancellation = if job.has_acknowledged_cancel(command)? {
            None
        } else {
            job.begin_cancel(command)?
        };
        let projection = if let Some(cancellation) = cancellation.as_mut() {
            let identity = jobs.identity_gate().await?;
            let root = self.root.clone();
            let profile = writer.scope().profile_id().to_owned();
            self.filesystem(move || {
                targets::reject_reserved_id(&root, &profile, command).map_err(anyhow::Error::new)
            })
            .await?;
            // Never wait behind the live execution/export gate. The receipt
            // comes from its first confirmed revision. Only this typed command
            // can mutate a STOP writer; it validates lifecycle/event/export
            // collisions from metadata without opening provider bindings.
            let input = CancelInputV1::new(operation, command).map_err(jobs::journal_error)?;
            let projection = match writer.append_cancel(input).await {
                Ok(projection) => projection,
                Err(failure) => {
                    if matches!(&failure, CompletionFailure::Unknown { .. }) {
                        cancellation.unknown();
                    }
                    let code = jobs::journal_error(failure.error());
                    job.result(Some(code));
                    return Err(code);
                }
            };
            cancellation.acknowledged();
            drop(identity);
            projection
        } else {
            writer
                .cancel_stop_projection(operation, command)
                .await
                .map_err(jobs::journal_error)?
                .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?
        };
        let acknowledgement = self
            .deliver_cancel(writer.scope(), projection, &job.cancel)
            .await
            .inspect_err(|code| job.result(Some(*code)))?;
        Ok(Response::reply(command_ack(operation, &acknowledgement)))
    }
    async fn deliver_cancel(
        &self,
        scope: &JournalScopeV1,
        projection: CancelStopProjectionV1,
        cancel: &CancelGeneration,
    ) -> Result<LocalCommandReceiptV1, BrokerErrorCodeV1> {
        #[cfg(test)]
        let operation = projection.operation_id();
        let receipt = projection.receipt().clone();
        let incomplete = projection.incomplete();
        async {
            // Handler control-flow only, after the real confirmed projection.
            // This seam supplies no runtime observation or terminal fact.
            #[cfg(test)]
            if let Some(result) = cancel_retry_tests::delivery(operation, cancel.current()) {
                return result;
            }
            if let Some(enforcer) = &self.stop_enforcer {
                if canonical(scope).map_err(jobs::journal_error)?
                    != canonical(projection.scope()).map_err(jobs::journal_error)?
                {
                    return Err(BrokerErrorCodeV1::Conflict);
                }
                enforcer
                    .deliver_projection(Arc::new(projection), cancel.current())
                    .await
                    .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)
            } else {
                deliver_cancellation(self.full_enforcer()?, scope, projection, cancel)
                    .await
                    .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)
            }
        }
        .await?;
        // Known targets were attempted even when coverage was incomplete.
        if incomplete {
            return Err(BrokerErrorCodeV1::NeedsReconciliation);
        }
        Ok(receipt)
    }
    async fn resume(
        &self,
        request: AuthorizedRequest,
        operation: Uuid,
        command: Uuid,
    ) -> Result<Response, BrokerErrorCodeV1> {
        let AuthorizedRequest {
            profile,
            verified,
            writer,
            jobs,
        } = request;
        self.require_starts().await?;
        check_ids(operation, command)?;
        self.load_job(&writer, operation, &profile).await?;
        let job = jobs.job(operation)?;
        let running = job.enter()?;
        let identity = jobs.identity_gate().await?;
        let durable = self.load_job(&writer, operation, &profile).await?;
        self.reject_export_command_reuse(&durable, command).await?;
        if durable
            .broker_checkpoint_v3()
            .is_some_and(|cp| cp.commands().iter().any(|c| c.command_id() == command))
        {
            return Err(BrokerErrorCodeV1::Conflict);
        }
        let event = resume_event(&durable, command)?;
        let pending = writer
            .enqueue_event(operation, event.clone())
            .await
            .map_err(jobs::journal_error)?;
        drop(identity);
        let sequence = verified
            .client
            .post_event(serde_json::to_value(&event).map_err(|_| BrokerErrorCodeV1::InvalidInput)?)
            .await
            .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
        let receipt = writer
            .acknowledge_event(operation, command, sequence)
            .await
            .map_err(jobs::journal_error)?;
        let source = if durable.checkpoint().broker_context().managed {
            None
        } else {
            // Only this explicit fresh Resume may reopen the same frozen project.
            let state = BrokerRunState::from_job(&durable).map_err(jobs::journal_error)?;
            let project = state.options().project.clone();
            let source_profile = profile.clone();
            Some(
                self.filesystem(move || {
                    crate::runner::broker::require_fresh_source(&durable)?;
                    Ok(Arc::new(AuthorizedSource::open(
                        &project,
                        &source_profile.source_roots,
                    )?))
                })
                .await?,
            )
        };
        job.set_source(source.clone())?;
        let context = self
            .context(verified, profile, &job, running.generation(), source)
            .await?;
        let capacity = Arc::clone(&self.execution)
            .acquire_owned()
            .await
            .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
        let executor = self.executor(writer).inspect_err(|code| {
            job.result(Some(*code));
        })?;
        Ok(Response {
            reply: BrokerReplyV1::CommandAcknowledged {
                operation_id: operation,
                command_id: command,
                input_hash: pending.input_hash().into(),
                revision: receipt.head().revision(),
                checkpoint_hash: receipt.head().hash().into(),
            },
            run: Some(DeferredRun {
                executor,
                operation,
                context,
                job,
                _running: running,
                _capacity: capacity,
                stopping: Arc::clone(&self.stopping),
            }),
        })
    }
    async fn export(
        &self,
        request: AuthorizedRequest,
        operation: Uuid,
        command: Uuid,
        destination: std::path::PathBuf,
    ) -> Result<Response, BrokerErrorCodeV1> {
        let AuthorizedRequest {
            profile,
            verified,
            writer,
            jobs,
        } = request;
        self.require_starts().await?;
        check_ids(operation, command)?;
        self.load_job(&writer, operation, &profile).await?;
        let job = jobs.job(operation)?;
        let _running = job.enter()?;
        let identity = jobs.identity_gate().await?;
        let durable = self.load_job(&writer, operation, &profile).await?;
        reject_event_command_reuse(&durable, command)?;
        if durable
            .broker_checkpoint_v3()
            .is_some_and(|cp| cp.commands().iter().any(|c| c.command_id() == command))
        {
            return Err(BrokerErrorCodeV1::Conflict);
        }
        let root = self.root.clone();
        let target_profile = profile.clone();
        let scope = verified.scope.clone();
        let target = self
            .filesystem(move || {
                targets::validate_and_persist(
                    &root,
                    &target_profile,
                    &scope,
                    operation,
                    command,
                    &destination,
                )
                .map_err(anyhow::Error::new)
            })
            .await?;
        drop(identity);
        let context = self
            .context(verified, profile, &job, _running.generation(), None)
            .await?;
        let _capacity = Arc::clone(&self.execution)
            .acquire_owned()
            .await
            .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
        match self
            .executor(writer)
            .inspect_err(|code| {
                job.result(Some(*code));
            })?
            .export(operation, target, &context)
            .await
        {
            Ok(done) => {
                job.result(None);
                Ok(Response::reply(BrokerReplyV1::ExportAcknowledged {
                    operation_id: operation,
                    request_id: done.request_id,
                    manifest_digest: done.manifest_digest,
                    files: done.files,
                }))
            }
            Err(_) => {
                job.result(Some(BrokerErrorCodeV1::NeedsReconciliation));
                Err(BrokerErrorCodeV1::NeedsReconciliation)
            }
        }
    }

    /// No authentication backend is needed to retain a previously authorized
    /// stop. This phase never emits a decision, admission, claim, copy or start.
    async fn restore_stops(&self) {
        let mut restored = 0_usize;
        if !self.material.complete {
            self.fenced.store(true, Ordering::SeqCst);
        }
        let root = self.root.clone();
        let expected = self.profiles.keys().cloned().collect::<BTreeSet<_>>();
        let inventory = self
            .filesystem(move || {
                let Some(journals) = root.persistent_child("journals", false)? else {
                    return Ok(false);
                };
                let inventory = journals.persistent_inventory(32)?;
                Ok(inventory.incomplete
                    || inventory
                        .entries
                        .iter()
                        .any(|entry| !entry.directory || !expected.contains(&entry.name)))
            })
            .await;
        if inventory != Ok(false) {
            self.fenced.store(true, Ordering::SeqCst);
            eprintln!("broker recovery: unknown or unsafe journal profile; new starts fenced");
        }
        for profile in &self.registry.data.profiles {
            match self.restore_profile(profile).await {
                Ok(count) => restored = restored.saturating_add(count),
                Err(_) => {
                    self.fenced.store(true, Ordering::SeqCst);
                    eprintln!(
                        "broker recovery: a configured journal could not be completely restored; new starts fenced"
                    );
                }
            }
        }
        if let Some(enforcer) = &self.stop_enforcer {
            match enforcer.status().await {
                Ok(status) if status.transport_healthy() && status.slot_count() == restored => {}
                _ => eprintln!(
                    "broker recovery: STOP transport or target coverage incomplete; admission remains fenced"
                ),
            }
            // This explicitly restricted epoch cannot promote into Full.
            self.fenced.store(true, Ordering::SeqCst);
            return;
        }
        let status = match self.full_enforcer() {
            Ok(enforcer) => enforcer.status().await,
            Err(_) => {
                self.fenced.store(true, Ordering::SeqCst);
                return;
            }
        };
        match status {
            Ok(status) if status.healthy() && status.slot_count() == restored => {}
            _ => {
                self.fenced.store(true, Ordering::SeqCst);
                eprintln!(
                    "broker recovery: stop helper has unknown targets or incomplete health; new starts fenced"
                );
            }
        }
    }

    async fn restore_profile(
        &self,
        profile: &ControlProfileV1,
    ) -> Result<usize, BrokerErrorCodeV1> {
        let scope = JournalScopeV1::new(
            self.registry.data.installation_id,
            &profile.tenant_id,
            &profile.gateway,
            profile.control_namespace_uuid,
            &profile.id,
        )
        .map_err(jobs::journal_error)?;
        let root = self.root.clone();
        let material = Arc::clone(&self.material);
        let (report, incomplete) = self
            .filesystem(move || recover_existing_profile(&root, scope, &material))
            .await?;
        if incomplete {
            self.fenced.store(true, Ordering::SeqCst);
        }
        let Some(report) = report else {
            return Ok(0);
        };
        let scope = report.scope().clone();
        let jobs = self.profile_jobs(&profile.id)?;
        let mut restored = 0_usize;
        let mut partial = incomplete || report.incomplete();
        if partial {
            self.fenced.store(true, Ordering::SeqCst);
        }
        let mut tasks = JoinSet::new();
        // Bound independent restores. A corrupt or unavailable job must not
        // serialize every later job behind its engine or credential source.
        for (index, recovered) in report.jobs().iter().enumerate() {
            let job = match jobs.job(recovered.operation_id()) {
                Ok(job) => job,
                Err(_) => {
                    partial = true;
                    continue;
                }
            };
            if job
                .remember_cancel_ids(recovered.cancel_command_ids())
                .is_err()
            {
                partial = true;
                continue;
            }
            let proof = Arc::clone(&report);
            if self
                .filesystem(move || Ok(proof.revalidate()?))
                .await
                .is_err()
            {
                self.fenced.store(true, Ordering::SeqCst);
                partial = true;
                break;
            }
            if tasks.len() >= 16 {
                match tasks.join_next().await {
                    Some(Ok((count, failed))) => {
                        restored += count;
                        partial |= failed;
                    }
                    _ => partial = true,
                }
            }
            let full = self
                .enforcer
                .as_ref()
                .map(|_| self.full_enforcer().cloned())
                .transpose()?;
            tasks.spawn(restore_recovered_job(
                full,
                self.stop_enforcer.clone(),
                Arc::clone(&report),
                index,
                job,
            ));
        }
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok((count, failed)) => {
                    restored += count;
                    partial |= failed;
                }
                Err(_) => partial = true,
            }
        }
        let proof = Arc::clone(&report);
        partial |= self
            .filesystem(move || Ok(proof.revalidate()?))
            .await
            .is_err();
        // Transfer the existing lease into one actor, without an unlock/relock
        // window. Partial recovery can only retain the STOP capability.
        let proof = Arc::clone(&report);
        let force_stops = partial || self.stop_enforcer.is_some();
        match self
            .filesystem(move || {
                Ok(ProfileWriterCapabilities::from_recovery_report(
                    &proof,
                    force_stops,
                    64,
                    WRITER_TIMEOUT,
                )?)
            })
            .await
        {
            Ok(capabilities) => {
                partial |= capabilities.is_stop_only();
                if canonical(capabilities.stops().scope()).map_err(jobs::journal_error)?
                    != canonical(&scope).map_err(jobs::journal_error)?
                    || jobs.writer.set(capabilities).is_err()
                {
                    partial = true;
                }
            }
            Err(_) => partial = true,
        }
        drop(report);
        if partial {
            self.fenced.store(true, Ordering::SeqCst);
        }
        Ok(restored)
    }

    async fn reconcile_loop(self: Arc<Self>) {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if self.stopping.load(Ordering::SeqCst) {
                return;
            }
            // A retained terminal audit is independent of business admission.
            // Only the existing Stops capability can enter this narrow path.
            self.reconcile_retained_audit().await;
            // A recovery fence allows inspection/cancellation but never a new
            // business start, including an autonomous reconciliation.
            if self.require_starts().await.is_err() || self.validate_control().await.is_err() {
                continue;
            }
            for profile in &self.registry.data.profiles {
                let Ok(jobs) = self.profile_jobs(&profile.id) else {
                    continue;
                };
                let Some(writer) = jobs.writer.get().and_then(ProfileWriterCapabilities::full)
                else {
                    continue;
                };
                let credential = match self.material.reconciler(profile) {
                    Ok(value) => value,
                    Err(_) => continue,
                };
                let mut after = None;
                loop {
                    let page = match writer.list(after, 100).await {
                        Ok(page) => page,
                        Err(_) => {
                            self.fenced.store(true, Ordering::SeqCst);
                            break;
                        }
                    };
                    for entry in page.entries() {
                        let operation = entry.operation_id();
                        let durable = match writer.load(operation).await {
                            Ok(Some(job)) => job,
                            _ => continue,
                        };
                        if !needs_autonomous_reconciliation(&durable) {
                            continue;
                        }
                        let Ok(job) = jobs.job(operation) else {
                            continue;
                        };
                        let Ok(running) = job.enter() else {
                            continue;
                        };
                        let Ok(capacity) = Arc::clone(&self.execution).try_acquire_owned() else {
                            continue;
                        };
                        if !job.autonomous_attempt() {
                            continue;
                        }
                        if jobs::require_profile(&durable, profile).is_err() {
                            job.result(Some(BrokerErrorCodeV1::Conflict));
                            continue;
                        }
                        let verified = match VerifiedProfile::authenticate(
                            &self.registry,
                            &profile.id,
                            &credential,
                            true,
                        )
                        .await
                        {
                            Ok(verified) => verified,
                            Err(code) => {
                                job.result(Some(code));
                                continue;
                            }
                        };
                        let context = match self
                            .context(verified, profile.clone(), &job, running.generation(), None)
                            .await
                        {
                            Ok(context) => context,
                            Err(code) => {
                                job.result(Some(code));
                                continue;
                            }
                        };
                        let executor = match self.executor(writer.clone()) {
                            Ok(value) => value,
                            Err(code) => {
                                job.result(Some(code));
                                continue;
                            }
                        };
                        DeferredRun {
                            executor,
                            operation,
                            context,
                            job,
                            _running: running,
                            _capacity: capacity,
                            stopping: Arc::clone(&self.stopping),
                        }
                        .start();
                    }
                    after = page.next_after();
                    if after.is_none() {
                        break;
                    }
                }
            }
        }
    }
}

/// No namespace is created for recovery. A partial sibling inventory fences
/// admissions, while the reader can still prove the configured scope's stops.
fn recover_existing_profile(
    root: &Directory,
    scope: JournalScopeV1,
    material: &Material,
) -> Result<(Option<Arc<StopRecoveryReportV1>>, bool)> {
    let Some(journals) = root.persistent_child("journals", false)? else {
        return Ok((None, false));
    };
    let Some(profile) = journals.persistent_child(scope.profile_id(), false)? else {
        return Ok((None, false));
    };
    let inventory = profile.persistent_inventory(8)?;
    let incomplete = inventory.incomplete
        || inventory
            .entries
            .iter()
            .any(|entry| !entry.directory || entry.name != "broker-journal-v1");
    let report = StopRecoveryReaderV1::open_existing(
        &profile,
        scope,
        &material.exclusions,
        JournalLimitsV1::default(),
        ControlSecrets::new(material.literals.clone())?,
    )?
    .map(|reader| Arc::new(reader.scan()));
    Ok((report, incomplete))
}

async fn restore_recovered_job(
    enforcer: Option<EnforcerClient>,
    stop_enforcer: Option<EnforcerStopClient>,
    report: Arc<StopRecoveryReportV1>,
    index: usize,
    job: Arc<Job>,
) -> (usize, bool) {
    let Some(recovered) = report.jobs().get(index) else {
        job.result(Some(BrokerErrorCodeV1::NeedsReconciliation));
        return (0, true);
    };
    if let Some(enforcer) = stop_enforcer {
        // There are no retirement P/Q records in this transport version. No
        // old Q/R+ is admitted or manufactured. Once retirement is connected,
        // its metadata-only reconciliation must precede Restore of that Arm;
        // an unrelated failure must keep this per-job sweep advancing.
        // The client owns retention and control confirmations for the whole
        // job independently of this waiter; the count is not a runtime effect.
        let (restored, partial) = enforcer
            .restore_report_job(Arc::clone(&report), index, job.cancel.current())
            .await;
        if partial || !recovered.managed_workspace() {
            job.result(Some(BrokerErrorCodeV1::NeedsReconciliation));
        }
        return (restored, partial);
    }
    let Some(enforcer) = enforcer else {
        job.result(Some(BrokerErrorCodeV1::NeedsReconciliation));
        return (0, true);
    };
    let origin = hex::encode(Sha256::digest(report.scope().gateway_origin().as_bytes()));
    let mut slots = Vec::new();
    let mut partial = false;
    for target in recovered.stops() {
        let registration = RegistrationV1::new(
            origin.clone(),
            report.scope().tenant_id().to_owned(),
            target.arm().clone(),
            target.receipt().clone(),
            target.authority().clone(),
            target.owned().clone(),
        );
        let restored = match registration {
            Ok(registration) => enforcer.restore(registration, job.cancel.clone()).await,
            Err(error) => Err(error),
        };
        match restored {
            Ok(slot) => slots.push((slot, target.cancel_after_arm().is_some())),
            Err(_) => partial = true,
        }
    }
    // Restores first seed the shared generation from every acknowledged child
    // slot. Numeric generations are not the causal proof of cancellation.
    if !recovered.cancel_command_ids().is_empty() {
        job.cancel.invalidate();
    }
    for (slot, cancelled) in &slots {
        if *cancelled && slot.cancel(job.cancel.current()).await.is_err() {
            partial = true;
        }
    }
    if partial || !recovered.managed_workspace() {
        job.result(Some(BrokerErrorCodeV1::NeedsReconciliation));
    }
    (slots.len(), partial)
}

fn needs_autonomous_reconciliation(job: &AcceptedJobV1) -> bool {
    let context = job.checkpoint().broker_context();
    // An unprepared accepted job never reopens its original source implicitly.
    if !context.managed {
        return false;
    }
    let pending_rpc = job.broker_checkpoint_v2().is_some_and(|cp| {
        cp.supervision()
            .operations()
            .iter()
            .any(|op| !op.has_response())
    });
    let pending_event = job
        .broker_checkpoint_v4()
        .is_some_and(|cp| cp.events().iter().any(|event| event.audit_seq().is_none()));
    let scan_done = job.broker_checkpoint_v4().is_some_and(|cp| {
        cp.events()
            .iter()
            .any(|event| event.event().kind == "broker.batch.scan")
    });
    context.worker.is_none_or(|worker| {
        worker.state != crate::control::WorkerStateV1::Terminal || worker.quota_held
    }) || pending_rpc
        || pending_event
        || !scan_done
}

fn check_ids(operation: Uuid, command: Uuid) -> Result<(), BrokerErrorCodeV1> {
    if operation.is_nil() || command.is_nil() {
        Err(BrokerErrorCodeV1::InvalidInput)
    } else {
        Ok(())
    }
}
fn accepted(job: &AcceptedJobV1, replayed: bool) -> BrokerReplyV1 {
    BrokerReplyV1::Accepted {
        operation_id: job.operation_id(),
        intent_hash: job.intent_hash().into(),
        submit_hash: job.submit_hash().into(),
        provider_resolution: job
            .checkpoint()
            .broker_context()
            .request
            .provider_resolution
            .clone(),
        revision: job.head().revision(),
        checkpoint_hash: job.head().hash().into(),
        replayed,
    }
}
fn rejected(receipt: &RejectionReceiptV1, replayed: bool) -> BrokerReplyV1 {
    BrokerReplyV1::Rejected {
        operation_id: receipt.operation_id(),
        phase: receipt.phase(),
        code: receipt.code(),
        record_hash: receipt.record_hash().into(),
        replayed,
    }
}
fn command_ack(operation: Uuid, receipt: &LocalCommandReceiptV1) -> BrokerReplyV1 {
    BrokerReplyV1::CommandAcknowledged {
        operation_id: operation,
        command_id: receipt.command_id(),
        input_hash: receipt.input_hash().into(),
        revision: receipt.head().revision(),
        checkpoint_hash: receipt.head().hash().into(),
    }
}
fn reject_event_command_reuse(job: &AcceptedJobV1, command: Uuid) -> Result<(), BrokerErrorCodeV1> {
    if job
        .broker_checkpoint_v4()
        .is_some_and(|cp| cp.events().iter().any(|event| event.event_id() == command))
    {
        Err(BrokerErrorCodeV1::Conflict)
    } else {
        Ok(())
    }
}
fn resume_event(job: &AcceptedJobV1, command: Uuid) -> Result<Event, BrokerErrorCodeV1> {
    if let Some(existing) = job
        .broker_checkpoint_v4()
        .and_then(|cp| cp.events().iter().find(|event| event.event_id() == command))
    {
        if existing.event().kind != "broker.resume.requested"
            || existing.event().data
                != json!({"version":1,"operation_id":job.operation_id().to_string()})
        {
            return Err(BrokerErrorCodeV1::Conflict);
        }
        return Ok(existing.event().clone());
    }
    let request = job.checkpoint().broker_context().request;
    let mut event = Event::new("broker.resume.requested");
    event.id = command.to_string();
    event.source = "broker".into();
    event.provenance = "declared".into();
    event.coverage = "generic".into();
    event.session_id = request.session_id.clone();
    event.project_id = request.project_id.clone();
    event.sandbox_id = request.sandbox_id.clone();
    event.agent_id = request.agent_id.clone();
    event.correlation_id = request.correlation_id.clone();
    event.trace_id = request.trace_id.clone();
    event.data = json!({"version":1,"operation_id":job.operation_id().to_string()});
    Ok(event)
}

async fn complete_rejection(
    writer: &WriterClient,
    operation: Uuid,
    mut ticket: crate::broker::journal::ReservationTicket,
    job: &Job,
    code: RejectionCodeV1,
) -> Result<RejectionReceiptV1, BrokerErrorCodeV1> {
    loop {
        match writer
            .reject(ticket, RejectionPhaseV1::SubmissionPreparation, code)
            .await
        {
            Ok(receipt) => return Ok(receipt),
            Err(CompletionFailure::NotQueued { error, input }) => {
                job.result(Some(jobs::journal_error(error)));
                (ticket, _, _) = input.into_parts();
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(CompletionFailure::Unknown { .. }) => {
                job.result(Some(BrokerErrorCodeV1::NeedsReconciliation));
                loop {
                    match writer.status(operation).await {
                        Ok(Some(JournalStatusV1::Rejected(receipt))) => return Ok(receipt),
                        Err(
                            crate::broker::journal::JournalError::Busy
                            | crate::broker::journal::JournalError::Io,
                        ) => {
                            tokio::time::sleep(Duration::from_millis(50)).await;
                        }
                        _ => return Err(BrokerErrorCodeV1::NeedsReconciliation),
                    }
                }
            }
        }
    }
}
enum Acceptance {
    Accepted(AcceptedJobV1),
    Rejected(RejectionReceiptV1),
}
async fn complete_acceptance(
    writer: &WriterClient,
    mut ticket: crate::broker::journal::ReservationTicket,
    mut prepared: crate::runner::broker::PreparedSubmission,
    job: &Job,
    generation: u64,
) -> Result<Acceptance, BrokerErrorCodeV1> {
    let operation = prepared.intent().operation_id();
    loop {
        if job.cancel.current() != generation {
            return complete_rejection(
                writer,
                operation,
                ticket,
                job,
                RejectionCodeV1::CancelledBeforeAcceptance,
            )
            .await
            .map(Acceptance::Rejected);
        }
        match writer.accept_prepared(ticket, prepared).await {
            Ok(accepted) => return Ok(Acceptance::Accepted(accepted)),
            Err(CompletionFailure::NotQueued { error, input }) => {
                job.result(Some(jobs::journal_error(error)));
                (ticket, prepared) = input.into_parts();
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(CompletionFailure::Unknown { .. }) => {
                // The same operation may already be committed. Never create a
                // replacement UUID or rerun provider preparation after timeout.
                job.result(Some(BrokerErrorCodeV1::NeedsReconciliation));
                loop {
                    match writer.status(operation).await {
                        Ok(Some(JournalStatusV1::Accepted(accepted))) => {
                            job.result(None);
                            return Ok(Acceptance::Accepted(accepted));
                        }
                        Ok(Some(JournalStatusV1::Rejected(receipt))) => {
                            return Ok(Acceptance::Rejected(receipt));
                        }
                        Err(
                            crate::broker::journal::JournalError::Busy
                            | crate::broker::journal::JournalError::Io,
                        ) => {
                            // Retain this same operation's context and source;
                            // queue saturation is not a completed reconciliation.
                            tokio::time::sleep(Duration::from_millis(50)).await;
                        }
                        _ => return Err(BrokerErrorCodeV1::NeedsReconciliation),
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn material(literals: Vec<String>) -> Material {
        Material {
            literals,
            files: Vec::new(),
            parents: Vec::new(),
            exclusions: FsExclusions::new(),
            complete: true,
        }
    }
    #[test]
    fn submissions_screen_presented_and_configured_control_literals_after_json_decoding() {
        let material = material(vec!["configured-private-value".into()]);
        let escaped: Value =
            serde_json::from_str(r#"{"prompt":"fresh-\u0070rivate-value"}"#).expect("JSON");
        assert_eq!(
            material.screen(&escaped, "fresh-private-value"),
            Err(BrokerErrorCodeV1::InvalidInput)
        );
        assert_eq!(
            material.screen(
                &json!({"argv":["echo","configured-private-value"]}),
                "fresh-private-value"
            ),
            Err(BrokerErrorCodeV1::InvalidInput)
        );
        assert!(
            material
                .screen(
                    &json!({"prompt":"inspect bounded source files"}),
                    "fresh-private-value"
                )
                .is_ok()
        );
    }
    #[test]
    fn screening_preserves_long_safe_prompt_and_rejects_short_literal_credentials() {
        let material = material(Vec::new());
        assert!(
            material
                .screen(
                    &json!({"prompt":"x".repeat(64 * 1024)}),
                    "private-credential"
                )
                .is_ok()
        );
        assert_eq!(
            material.screen(&json!({"prompt":"a"}), "a"),
            Err(BrokerErrorCodeV1::InvalidInput)
        );
    }
}
