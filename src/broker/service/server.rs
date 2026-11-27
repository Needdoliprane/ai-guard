//! Trusted broker process. A Unix peer UID routes a private transport; only a
//! fresh gateway check of the presented credential authenticates a request.
use super::{
    AuthenticatedBatchContext, ServiceEnforcerSelectionV1, VerifiedProfile,
    jobs::{self, Job, ProfileJobs, RunningJob},
    profile::{ControlProfileV1, Registry, SubmitV1},
    protocol::*,
    result, targets,
};
use crate::{
    broker::{
        clock::NativeHostClock,
        discovery::RoleLease,
        enforcer::{
            CancelGeneration, EnforcerClient, EnforcerError, EnforcerFullClientV3,
            EnforcerStopClient, ExecutionEnforcerV1, FreshEnforcerOwner, RegistrationV1,
            deliver_named_import_stop_from_legacy_root,
        },
        journal::{
            AcceptedJobV1, CancelStopProjectionV1, ControlSecrets, FsExclusions, Journal,
            JournalLimitsV1, JournalScopeV1, JournalStatusV1, RejectionCodeV1, RejectionPhaseV1,
            RejectionReceiptV1, ReservationMode, ReserveResult, canonical,
            recovery::{StopRecoveryReaderV1, StopRecoveryReportV1},
        },
        lifecycle::{LocalCommandReceiptV1, LocalCommandRequestV1},
        local_ipc::{PrivateListener, read_frame, write_frame},
        presence::{
            PresenceError, PresenceScope,
            owned::{SelectedPresence, load_selected},
        },
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
        prepare_broker_submission_with_egress, require_retained_egress_runtime,
    },
    workspace::AuthorizedSource,
};
use anyhow::{Result, anyhow, ensure};
use futures_util::StreamExt;
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
    sync::{OwnedSemaphorePermit, Semaphore, oneshot, watch},
    task::JoinSet,
};
use uuid::Uuid;

const FRAME_BYTES: u32 = 512 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(15);
const WRITE_TIMEOUT: Duration = Duration::from_secs(3);
const WRITER_TIMEOUT: Duration = Duration::from_secs(10);

#[path = "retained_audit.rs"]
mod retained_audit;

#[path = "checkpoint_source.rs"]
mod checkpoint_source;

#[path = "legacy_named_launch.rs"]
mod legacy_named_launch;

/// Proposed owned broker-supervision publisher, selected only by a presence
/// file naming its exact profile. It shares the publication owner above.
#[path = "owned_presence.rs"]
mod owned_presence;

#[path = "retirement_settlement.rs"]
mod retirement_settlement;

#[cfg(test)]
#[path = "presence_material_tests.rs"]
mod presence_material_tests;

#[cfg(test)]
#[path = "cancel_retry_tests.rs"]
mod cancel_retry_tests;
#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
#[path = "full_owner_tests.rs"]
mod full_owner_tests;
#[cfg(test)]
#[path = "full_signal_owner_tests.rs"]
mod full_signal_owner_tests;
#[cfg(test)]
#[path = "owned_ipc_peer.rs"]
mod owned_ipc_peer;
#[cfg(all(test, unix))]
#[path = "preclaim_live_tests.rs"]
mod preclaim_live_tests;
#[cfg(test)]
#[path = "result_auth_tests.rs"]
mod result_auth_tests;
#[cfg(all(test, unix))]
#[path = "retirement_audit_tests.rs"]
mod retirement_audit_tests;
#[cfg(all(test, unix))]
#[path = "retirement_settlement_tests.rs"]
mod retirement_settlement_tests;
#[cfg(test)]
type ServiceEnforcer = cancel_retry_tests::TestEnforcer;
#[cfg(not(test))]
type ServiceEnforcer = ExecutionEnforcerV1;

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
        for path in registry.relay_parents() {
            // A fixed private parent exists before issuance; the selected
            // operation child may appear later under the original Worker Arm.
            match Directory::open_existing(&path) {
                Ok(parent) => material.parents.push(parent),
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
    fn protect_presence(&mut self, prepared: &SelectedPresence) -> Result<()> {
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
    /// Validate only this profile's retained runner material for metadata
    /// delivery. An unrelated missing provider/blob does not close this path.
    fn checked_reconciler(&self, profile: &ControlProfileV1) -> Result<String> {
        let file = self
            .files
            .iter()
            .find(|file| file.path() == profile.runner_token_file)
            .ok_or_else(|| anyhow!("private reconciler credential is unavailable"))?;
        let parent_path = file
            .path()
            .parent()
            .ok_or_else(|| anyhow!("private reconciler parent is unavailable"))?;
        let parent = self
            .parents
            .iter()
            .find(|parent| parent.path() == parent_path)
            .ok_or_else(|| anyhow!("private reconciler parent is unavailable"))?;
        parent.revalidate()?;
        file.revalidate()?;
        self.reconciler(profile)
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
    cold_owners: Arc<retained_audit::ColdOwners>,
    full_owners: Arc<FullOwners>,
    stopping: Arc<AtomicBool>,
    fenced: Arc<AtomicBool>,
}

// Process-local task ownership grants no enforcer or journal capability.
// Signals need their own bounded registry: the Full job they inhibit may
// already occupy the last run slot. Both registries retain their own results.
struct FullOwners {
    runs: Arc<retained_audit::OwnerTasks<()>>,
    signals: Arc<retained_audit::OwnerTasks<()>>,
    failed: AtomicBool,
}
type FullOwnerSlot = retained_audit::OwnerSlot<()>;

impl FullOwners {
    fn new(capacity: usize) -> Result<Arc<Self>, retained_audit::OwnerError> {
        Ok(Arc::new(Self {
            runs: retained_audit::OwnerTasks::new(capacity)?,
            signals: retained_audit::OwnerTasks::new(capacity)?,
            failed: AtomicBool::new(false),
        }))
    }

    fn try_reserve(&self) -> Result<FullOwnerSlot, retained_audit::OwnerError> {
        self.runs.try_reserve()
    }

    fn try_reserve_signal(&self) -> Result<FullOwnerSlot, retained_audit::OwnerError> {
        self.signals.try_reserve()
    }

    fn close(&self) -> Result<(), retained_audit::OwnerError> {
        let runs = self.runs.close();
        let signals = self.signals.close();
        runs.and(signals)
    }

    async fn drain(&self) -> Result<(), retained_audit::DrainError> {
        // Close both admissions before the first await, including when a
        // caller drains directly. A cancelled observer owns neither handle.
        let _ = self.close();
        let runs = self.runs.drain().await;
        if runs.is_err() {
            self.failed.store(true, Ordering::SeqCst);
        }
        // Never short-circuit on a failed run: accepted signal HTTP/ACK work
        // must also finish before the service can release its physical lease.
        let signals = self.signals.drain().await;
        if signals.is_err() {
            self.failed.store(true, Ordering::SeqCst);
        }
        runs.and(signals)
    }

    fn has_failure(&self) -> bool {
        self.failed.load(Ordering::SeqCst)
    }
}

struct DeferredRun {
    owner: FullOwnerSlot,
    executor: BatchExecutor,
    operation: Uuid,
    context: AuthenticatedBatchContext,
    job: Arc<Job>,
    _running: RunningJob,
    _capacity: OwnedSemaphorePermit,
    stopping: Arc<AtomicBool>,
}
impl DeferredRun {
    fn start(self) -> std::result::Result<(), retained_audit::OwnerError> {
        let Self {
            owner,
            executor,
            operation,
            context,
            job,
            _running,
            _capacity,
            stopping,
        } = self;
        let not_started = Arc::clone(&job);
        // Admission already reserved this non-clone slot before its reply.
        // Starting consumes it; it never competes for a new slot after ACK.
        let result = owner.start(async move {
            if stopping.load(Ordering::SeqCst) {
                job.result(Some(BrokerErrorCodeV1::Unavailable));
                // No executor was entered. The caller-visible refusal leaves
                // no unconfirmed task effect for the shutdown owner to join.
                return Ok(());
            }
            let outcome = executor.run(operation, &context).await;
            let result = match outcome {
                Ok(BatchOutcome::AwaitingReview) => {
                    job.result(None);
                    Ok(())
                }
                Ok(BatchOutcome::AbandonedBeforeClaim(abandoned)) => {
                    job.result(None);
                    // These are retained Created/F facts; dropping their data
                    // owners neither cleans the instance nor releases quota.
                    drop(abandoned);
                    Ok(())
                }
                Ok(_) => {
                    job.result(None);
                    let _ = job.set_source(None);
                    Ok(())
                }
                Err(_) => {
                    job.result(Some(BrokerErrorCodeV1::NeedsReconciliation));
                    Err(BrokerErrorCodeV1::NeedsReconciliation)
                }
            };
            // Explicit captures keep both capabilities for the complete run,
            // including every early-return path before this normal completion.
            drop((_running, _capacity));
            result
        });
        if let Err(error) = result {
            not_started.result(Some(match error {
                retained_audit::OwnerError::Busy => BrokerErrorCodeV1::Busy,
                _ => BrokerErrorCodeV1::Unavailable,
            }));
        }
        result
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
/// Each presence file explicitly selects the legacy or the owned profile; a
/// refused owned configuration never falls back to the legacy profile.
async fn prepare_presence(
    registry: Arc<Registry>,
) -> Result<(Material, Vec<(String, SelectedPresence)>)> {
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
            let presence = load_selected(path, scope, material.literals.clone())?;
            prepared.push((profile.id.clone(), presence));
        }
        // Owned journals stay dedicated across every profile and one broker is
        // published once, before any protection, lock or anchor exists.
        owned_presence::check_owned_profiles(&registry, &material, &prepared)?;
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
    serve_selected(config_path, ServiceEnforcerSelectionV1::Legacy).await
}

/// Explicit existing-installation recovery. No Full capability is requested,
/// no new control namespace is bootstrapped and admissions stay fenced.
pub async fn serve_recovery(config_path: &Path) -> Result<()> {
    serve_selected(
        config_path,
        ServiceEnforcerSelectionV1::Recovery { role_name: None },
    )
    .await
}

pub async fn serve_selected(
    config_path: &Path,
    selection: ServiceEnforcerSelectionV1,
) -> Result<()> {
    match serve_selected_owned(config_path, selection).await? {
        ServiceShutdown::Drained => Ok(()),
        ServiceShutdown::Retained(owner) => owner.supervise().await,
    }
}

/// The degraded outcome owns the service, original backend leases and existing
/// endpoint. It must be captured by the CLI rather than flattened into anyhow.
// Public shutdown contract captured by the CLI; keep the value shape.
#[allow(clippy::large_enum_variant)]
pub enum ServiceShutdown {
    Drained,
    Retained(RetainedServiceShutdown),
}
pub struct RetainedServiceShutdown {
    service: Arc<Service>,
    listener: PrivateListener,
    _lease: Arc<RoleLease>,
    fresh_guardian: Option<FreshEnforcerOwner>,
    terminate: tokio::signal::unix::Signal,
    interrupt: tokio::signal::unix::Signal,
}
impl RetainedServiceShutdown {
    pub async fn supervise(self) -> Result<()> {
        let mut owner = self;
        eprintln!(
            "broker shutdown is not drained: broker ownership and the private endpoint remain retained; inspect recovery status and request Cancel if the STOP connection is available; a further termination ends this broker without proving retirement"
        );
        let mut requests = JoinSet::new();
        let capacity = Arc::new(Semaphore::new(64));
        let mut retry = tokio::time::interval(Duration::from_secs(5));
        let mut guardian_drained = false;
        let drained = loop {
            tokio::select! {
                _ = owner.terminate.recv() => break false,
                _ = owner.interrupt.recv() => break false,
                Some(_) = requests.join_next(), if !requests.is_empty() => {},
                _ = retry.tick() => {
                    // Only exact, previously admitted Final continuations and
                    // their audit are eligible. No new Begin/starts are opened.
                    owner.service.reconcile_retirement_audit().await;
                    // An unconfirmed Fresh drain keeps its original process
                    // custody. A later audit ACK cannot certify its exit.
                    if let Some(guardian) = owner.fresh_guardian.as_mut()
                        && matches!(guardian.reap_acknowledged_exit(), Ok(true))
                    {
                        owner.fresh_guardian = None;
                        guardian_drained = true;
                    }
                    if owner.fresh_guardian.is_none() && !owner.service.requires_retained_shutdown() {
                        break guardian_drained;
                    }
                },
                accepted = owner.listener.accept_checked() => {
                    match accepted {
                        Ok(stream) => if let Ok(slot) = Arc::clone(&capacity).try_acquire_owned() {
                            let service = Arc::clone(&owner.service);
                            requests.spawn(async move { let _slot = slot; service.connection(stream).await; });
                        },
                        Err(_) => {
                            owner.service.fenced.store(true, Ordering::Release);
                            // Keep the actual owner/lease until the explicit
                            // termination below; no unsafe endpoint replacement.
                            tokio::select! { _ = owner.terminate.recv() => {}, _ = owner.interrupt.recv() => {} }
                            break false;
                        }
                    }
                }
            }
        };
        requests.abort_all();
        while requests.join_next().await.is_some() {}
        if drained {
            return Ok(());
        }
        // `owner` is still alive here. An explicit degraded termination
        // preserves remaining STOP authority outside the broker.
        Err(anyhow!(
            "broker stopped with a degraded shutdown; no safe drain is claimed"
        ))
    }
}

pub async fn serve_selected_owned(
    config_path: &Path,
    selection: ServiceEnforcerSelectionV1,
) -> Result<ServiceShutdown> {
    selection.validate()?;
    let recovery_only = matches!(&selection, ServiceEnforcerSelectionV1::Recovery { .. });
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
    let lease = Arc::new(if recovery_only {
        RoleLease::acquire_existing(&broker_root)?
    } else {
        RoleLease::acquire(&broker_root)?
    });
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
    let executable =
        std::env::current_exe().map_err(|_| anyhow!("broker executable is unavailable"))?;
    let mut material = early_material.map(Arc::new);
    let (enforcer, stop_enforcer, mut fresh_guardian) = match selection {
        ServiceEnforcerSelectionV1::Legacy => {
            let client = EnforcerClient::connect_or_spawn_legacy_with_budget(
                &root,
                "enforcer",
                &executable,
                registry.data.max_enforcer_slots,
                registry.data.max_recovery_stop_owners,
            )
            .await?;
            (Some(ExecutionEnforcerV1::Legacy(client)), None, None)
        }
        ServiceEnforcerSelectionV1::Recovery { role_name } => {
            let sealed = role_name.is_some();
            let role = root
                .persistent_child(role_name.as_deref().unwrap_or("enforcer"), false)?
                .ok_or_else(|| anyhow!("broker stop root is unavailable"))?;
            let stop = if sealed {
                EnforcerStopClient::connect_or_spawn_existing_sealed_with_budget(
                    &role,
                    &executable,
                    registry.data.max_enforcer_slots,
                    registry.data.max_recovery_stop_owners,
                )
                .await?
            } else {
                EnforcerStopClient::connect_or_spawn_for_recovery_with_budget(
                    &role,
                    &executable,
                    registry.data.max_enforcer_slots,
                    registry.data.max_recovery_stop_owners,
                )
                .await?
            };
            (None, Some(stop), None)
        }
        ServiceEnforcerSelectionV1::Full3Fresh { role_name } => {
            if material.is_none() {
                let material_registry = Arc::clone(&registry);
                material = Some(Arc::new(
                    tokio::task::spawn_blocking(move || Material::load(&material_registry))
                        .await??,
                ));
            }
            let scan_root = root.clone();
            let scan_registry = Arc::clone(&registry);
            let scan_material = Arc::clone(
                material
                    .as_ref()
                    .ok_or_else(|| anyhow!("Full3 material is unavailable"))?,
            );
            // Keep every existing profile lease through exclusive genesis.
            // A source pathname or an empty-looking role is never a fresh proof.
            let reports = tokio::task::spawn_blocking(move || {
                verify_fresh_installation(&scan_root, &scan_registry, &scan_material)
            })
            .await??;
            let (client, owner) = EnforcerFullClientV3::spawn_fresh_with_budget(
                &root,
                &role_name,
                &executable,
                registry.data.installation_id,
                registry.data.max_enforcer_slots,
                registry.data.max_recovery_stop_owners,
            )
            .await?;
            client.verify_installation(registry.data.installation_id)?;
            drop(reports);
            (Some(ExecutionEnforcerV1::Full3(client)), None, Some(owner))
        }
        ServiceEnforcerSelectionV1::Full3Existing { role_name } => {
            let role = root
                .persistent_child(&role_name, false)?
                .ok_or_else(|| anyhow!("sealed enforcer role is unavailable"))?;
            let client =
                EnforcerFullClientV3::connect_existing(&role, registry.data.max_enforcer_slots)
                    .await?;
            client.verify_installation(registry.data.installation_id)?;
            // This same native lease/epoch closes Original admission before
            // the broker reads any recovery source. The Full handle remains
            // for existing Q/F settlements; snapshots use the STOP-only port.
            let stop = EnforcerStopClient::connect_or_spawn_existing_sealed_with_budget(
                &role,
                &executable,
                registry.data.max_enforcer_slots,
                registry.data.max_recovery_stop_owners,
            )
            .await?;
            (Some(ExecutionEnforcerV1::Full3(client)), Some(stop), None)
        }
    };
    let material = if let Some(material) = material {
        material
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
    if !recovery_only {
        let relays = crate::broker::relay_owner::RelayRegistry::new(
            broker_root.clone(),
            lease.clone(),
            registry.relay_operations(),
            material.literals.clone(),
        );
        registry
            .relay_service
            .set(relays)
            .map_err(|_| anyhow!("broker relay owner was already initialized"))?;
    }
    let service = Arc::new(Service {
        cold_owners: retained_audit::ColdOwners::new(registry.data.max_enforcer_slots as usize)?,
        full_owners: FullOwners::new(registry.data.max_enforcer_slots as usize)?,
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
        fenced: Arc::new(AtomicBool::new(recovery_only)),
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
    let mut presence_owners = Vec::new();
    let presence_attempts = Arc::new(Semaphore::new(4));
    let presence_io = Arc::new(Semaphore::new(2));
    for (profile, presence) in prepared_presence {
        let named_control = match &presence {
            SelectedPresence::Legacy(legacy) => match legacy.named_control() {
                Ok(control) => Some(control),
                Err(error) => {
                    if error.fences_admission() {
                        service.fenced.store(true, Ordering::SeqCst);
                    }
                    log_presence_stopped(&profile, error, &service.material.literals);
                    continue;
                }
            },
            SelectedPresence::Owned(_) => None,
        };
        let shutdown = presence_shutdown.subscribe();
        let attempts = Arc::clone(&presence_attempts);
        let io = Arc::clone(&presence_io);
        let owner = Arc::clone(&service);
        let publication = match checkpoint_source::Publication::start(
            &service,
            profile.clone(),
            presence,
            shutdown,
            attempts,
            io,
        ) {
            Ok(publication) => Arc::new(publication),
            Err(error) => {
                if error.fences_admission() {
                    service.fenced.store(true, Ordering::SeqCst);
                }
                log_presence_stopped(&profile, error, &service.material.literals);
                continue;
            }
        };
        if let Some(control) = named_control {
            let attached = service
                .profile_jobs(&profile)
                .is_ok_and(|jobs| jobs.named_control.set(control).is_ok());
            if !attached {
                service.fenced.store(true, Ordering::SeqCst);
                log_presence_stopped(
                    &profile,
                    PresenceError::UnsafeState,
                    &service.material.literals,
                );
                publication.stop();
            }
        }
        // Keep a strong owner outside the cancellable join observer. Shutdown
        // must join its admitted FS work before dropping the service lease.
        presence_owners.push(Arc::clone(&publication));
        presence_tasks.spawn(async move {
            if let Err(error) = publication.join().await {
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
    let operator_shutdown = loop {
        tokio::select! {
            _ = terminate.recv() => break true,
            _ = interrupt.recv() => break true,
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
                    Err(_) => { service.fenced.store(true, Ordering::SeqCst); break false; },
                }
            }
        }
    };
    let result = service
        .shutdown_owned(
            reconciler,
            &presence_shutdown,
            &mut presence_tasks,
            &mut requests,
            &presence_owners,
        )
        .await;
    let retain_service = service.requires_retained_shutdown();
    if operator_shutdown
        && result.is_ok()
        && !retain_service
        && let Some(guardian) = fresh_guardian.as_mut()
    {
        match guardian.shutdown_if_idle().await {
            Ok(true) => fresh_guardian = None,
            Ok(false) => {
                eprintln!("broker guardian acknowledged idle shutdown; waiting for child exit");
            }
            Err(EnforcerError::NotIdle) => {
                eprintln!(
                    "broker guardian is not idle; keep it running and inspect or cancel retained jobs"
                );
            }
            Err(_) => {
                eprintln!("broker guardian shutdown is unconfirmed; reconciliation is required");
            }
        }
    }
    if retain_service || (operator_shutdown && fresh_guardian.is_some()) {
        return Ok(ServiceShutdown::Retained(RetainedServiceShutdown {
            service,
            listener,
            _lease: lease,
            fresh_guardian,
            terminate,
            interrupt,
        }));
    }
    drop(listener);
    drop(lease);
    result?;
    ensure!(
        operator_shutdown,
        "broker listener failed; no clean shutdown is claimed"
    );
    Ok(ServiceShutdown::Drained)
}

impl Service {
    fn requires_retained_shutdown(&self) -> bool {
        // The Final pump cannot clear an unconfirmed Full/signal task outcome.
        // Retain the same shared RoleLease until explicit termination instead.
        self.full_owners.has_failure() || self.has_retained_retirement()
    }

    fn has_retained_retirement(&self) -> bool {
        self.profiles.values().any(|jobs| {
            jobs.has_retained_retirement()
                || jobs
                    .writer
                    .get()
                    .is_some_and(|writers| writers.stops().has_retained_retirement())
        })
    }
    fn reserve_full_owner(&self) -> Result<FullOwnerSlot, BrokerErrorCodeV1> {
        self.full_owners.try_reserve().map_err(|error| match error {
            retained_audit::OwnerError::Busy => BrokerErrorCodeV1::Busy,
            _ => BrokerErrorCodeV1::Unavailable,
        })
    }

    #[cfg(test)]
    async fn shutdown(
        &self,
        reconciler: tokio::task::JoinHandle<()>,
        presence_shutdown: &watch::Sender<bool>,
        presence_tasks: &mut JoinSet<()>,
        requests: &mut JoinSet<()>,
    ) -> Result<()> {
        self.shutdown_owned(reconciler, presence_shutdown, presence_tasks, requests, &[])
            .await
    }

    async fn shutdown_owned(
        &self,
        reconciler: tokio::task::JoinHandle<()>,
        presence_shutdown: &watch::Sender<bool>,
        presence_tasks: &mut JoinSet<()>,
        requests: &mut JoinSet<()>,
        presence_owners: &[Arc<checkpoint_source::Publication>],
    ) -> Result<()> {
        self.stopping.store(true, Ordering::SeqCst);
        let relay_shutdown = self.registry.relay_service.get().cloned();
        let mut degraded = self.cold_owners.close().is_err();
        if relay_shutdown
            .as_ref()
            .is_some_and(|relays| relays.close_admission().is_err())
        {
            degraded = true;
        }
        if self.full_owners.close().is_err() {
            degraded = true;
        }
        let _ = presence_shutdown.send(true);
        for publication in presence_owners {
            publication.stop();
        }
        for profile in self.profiles.values() {
            profile.invalidate_all();
        }
        reconciler.abort();
        if let Err(error) = reconciler.await
            && !error.is_cancelled()
        {
            degraded = true;
        }
        // These futures observe retained owners. A timed-out observer does not
        // remove the actual publisher/FS handle, which is joined below.
        let presence_drain = async { while presence_tasks.join_next().await.is_some() {} };
        if tokio::time::timeout(Duration::from_secs(15), presence_drain)
            .await
            .is_err()
        {
            presence_tasks.abort_all();
            while presence_tasks.join_next().await.is_some() {}
        }
        for publication in presence_owners {
            if publication.join().await.is_err() {
                degraded = true;
            }
        }
        // In-flight framing/writer ACKs receive a short bounded drain. A lost reply
        // never rolls back an already durable accepted job or enforcer registration.
        let drain = async { while requests.join_next().await.is_some() {} };
        let _ = tokio::time::timeout(Duration::from_secs(5), drain).await;
        requests.abort_all();
        // Complete producer cancellation before releasing a prepared DeferredRun
        // held by a response. A closed registry cannot accept any late spawn.
        while let Some(result) = requests.join_next().await {
            if let Err(error) = result
                && !error.is_cancelled()
            {
                degraded = true;
            }
        }
        if self.cold_owners.drain().await.is_err() {
            degraded = true;
        }
        // Join every accepted Full future without an abort or global timeout.
        // Its completion is not a terminal runtime or fencing observation.
        if self.full_owners.drain().await.is_err() {
            degraded = true;
        }
        // An already accepted Full owner can still publish its first exact
        // Final while it drains. Close D5 admission only after these owners,
        // the reconciler and request/factory producers have actually joined.
        // No new Full or Cold owner can be accepted after the initial close.
        for jobs in self.profiles.values() {
            if let Some(writers) = jobs.writer.get() {
                degraded |= writers.stops().close_retirement_owners().is_err();
            }
        }
        for jobs in self.profiles.values() {
            if let Some(writers) = jobs.writer.get() {
                degraded |= writers.stops().drain_retirement_owners().await.is_err();
            }
        }
        if let Some(relays) = relay_shutdown
            && relays.shutdown().await.is_err()
        {
            degraded = true;
        }
        if degraded {
            Err(anyhow!("broker shutdown needs reconciliation"))
        } else {
            Ok(())
        }
    }

    fn recovery_only(&self) -> bool {
        self.stop_enforcer.is_some()
    }
    fn full_enforcer(&self) -> Result<&ExecutionEnforcerV1, BrokerErrorCodeV1> {
        // A Full3 handle already owned before recovery can still settle its
        // existing Q/F continuations while the separate STOP port fences new
        // starts. A pure StopsOnly service has no such handle to return.
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
        if let Some(full) = writer.full() {
            full.bind_status_service(&self.fenced, &self.stopping);
        }
        Ok(writer.clone())
    }

    async fn recovery_status_for_profile(
        &self,
        profile: &ControlProfileV1,
        scope: &JournalScopeV1,
        operation: Uuid,
    ) -> Result<Response, BrokerErrorCodeV1> {
        let snapshot = self
            .recovery_status_snapshot_for_profile(profile, scope, operation)
            .await?;
        Ok(Response::reply(BrokerReplyV1::RecoveryStatus {
            status: snapshot.journal,
        }))
    }

    async fn recovery_status_v2_for_profile(
        &self,
        profile: &ControlProfileV1,
        scope: &JournalScopeV1,
        operation: Uuid,
    ) -> Result<Response, BrokerErrorCodeV1> {
        let status = self
            .recovery_status_snapshot_for_profile(profile, scope, operation)
            .await?;
        Ok(Response::reply(BrokerReplyV1::RecoveryStatusV2 { status }))
    }

    async fn recovery_status_snapshot_for_profile(
        &self,
        profile: &ControlProfileV1,
        scope: &JournalScopeV1,
        operation: Uuid,
    ) -> Result<RecoveryStatusV2, BrokerErrorCodeV1> {
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
        let mut preclaim = Vec::with_capacity(snapshot.stops().len());
        let known_arms = snapshot
            .stops()
            .iter()
            .map(|stop| {
                let arm = stop.arm();
                let local = self
                    .profiles
                    .get(&profile.id)
                    .and_then(|jobs| jobs.known(operation));
                let view = local.as_ref().and_then(|job| {
                    Uuid::parse_str(&arm.operation_id)
                        .ok()
                        .map(|arm| job.preclaim.arm_view(arm))
                });
                let state = match view {
                    Some(view) if !snapshot.incomplete() => RecoveryPreclaimV1 {
                        version: 1,
                        expected_epoch: view.expected_epoch,
                        generation: view.generation,
                        state: view.state,
                        source: PreclaimSourceV1::ProcessLocal,
                        freshness: if view.freshness == "current_process_state" {
                            PreclaimFreshnessV1::CurrentProcessState
                        } else {
                            PreclaimFreshnessV1::Unknown
                        },
                        command_id: view.command_id,
                        signal_audit_confirmed: view.audit_confirmed,
                        retirement_continuation_retained: view.retirement_continuation_retained,
                    },
                    _ => RecoveryPreclaimV1 {
                        version: 1,
                        expected_epoch: None,
                        generation: None,
                        state: super::PreclaimStateV1::Unknown,
                        source: PreclaimSourceV1::ProcessLocal,
                        freshness: PreclaimFreshnessV1::Unknown,
                        command_id: None,
                        signal_audit_confirmed: false,
                        retirement_continuation_retained: None,
                    },
                };
                preclaim.push(RecoveryPreclaimTargetV2 {
                    arm_id: arm.operation_id.clone(),
                    preclaim: state,
                });
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
        let retirements = snapshot
            .retirements()
            .iter()
            .map(|entry| {
                let pending = entry.request();
                Ok(RecoveryRetirementV1 {
                    arm_id: Uuid::parse_str(&pending.identity().arm().operation_id)
                        .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?,
                    retirement_id: *pending.identity().retirement_id(),
                    phase: if entry.finalized().is_some() {
                        RetirementPhaseV1::Final
                    } else {
                        RetirementPhaseV1::Pending
                    },
                    intention_audit_confirmed: pending.audit_ack().is_some(),
                    guardian_receipt_hash: entry
                        .finalized()
                        .map(|entry| {
                            entry
                                .receipt()
                                .digest()
                                .map(|hash| hash.as_str().to_owned())
                        })
                        .transpose()
                        .map_err(jobs::journal_error)?,
                })
            })
            .collect::<Result<Vec<_>, BrokerErrorCodeV1>>()?;
        Ok(RecoveryStatusV2 {
            version: 2,
            journal: RecoveryStatusV1 {
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
                // V1 cannot describe D5 phases. Keep its historical facts and
                // require reconciliation rather than imply runtime completion.
                reconciliation_required: capabilities.is_stop_only()
                    || snapshot.incomplete()
                    || !retirements.is_empty(),
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
            preclaim,
            retirements,
        })
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
        let preclaim = job.preclaim.clone();
        self.filesystem(move || {
            registry.source.revalidate()?;
            registry.source_directory.revalidate()?;
            material.revalidate()?;
            ensure!(
                cancel.current() == generation,
                "broker command generation changed"
            );
            verified.client = verified.client.with_secrets(material.literals.clone());
            let mut context =
                verified.open_context(&registry, &profile, &root, cancel, preclaim, source)?;
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
        // Validated when the registry was loaded; defaults when absent.
        let limits = match self.registry.data.provider_activity {
            Some(budget) => budget
                .limits()
                .map_err(|_| BrokerErrorCodeV1::Unavailable)?,
            None => crate::provider_activity::ActivityLimits::default(),
        };
        Ok(
            BatchExecutor::new(writer, self.full_enforcer()?.clone(), Arc::new(clock))
                .with_relays(self.registry.relay_service.get().cloned())
                .with_egress_resolver(self.registry.egress_resolver())
                .with_activity_root(self.root.clone(), limits),
        )
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
            let _ = run.start();
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
        if matches!(
            &envelope.request,
            BrokerCommandV1::Result { version, operation_id }
                if *version != JOB_RESULT_VERSION || operation_id.is_nil()
        ) {
            return Err(BrokerErrorCodeV1::InvalidInput);
        }
        let read_only = matches!(
            envelope.request,
            BrokerCommandV1::Status { .. }
                | BrokerCommandV1::Result { .. }
                | BrokerCommandV1::RelayStatus { .. }
                | BrokerCommandV1::List { .. }
                | BrokerCommandV1::RecoveryStatus { .. }
                | BrokerCommandV1::RecoveryStatusV2 { .. }
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
        if let BrokerCommandV1::RecoveryStatusV2 { operation_id } = &envelope.request {
            return self
                .recovery_status_v2_for_profile(&profile, &verified.scope, *operation_id)
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
        let writer = if matches!(&envelope.request, BrokerCommandV1::Result { .. }) {
            if self.recovery_only()
                || self.fenced.load(Ordering::SeqCst)
                || self.stopping.load(Ordering::SeqCst)
            {
                return Err(BrokerErrorCodeV1::NeedsReconciliation);
            }
            self.profile_writers(&profile.id, &verified.scope, false)
                .await?
                .full()
                .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?
        } else {
            self.writer(&profile.id, &verified.scope).await?
        };
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
            BrokerCommandV1::AbandonBeforeClaim { request: signal } => {
                self.abandon_before_claim(request, signal).await
            }
            BrokerCommandV1::Status { operation_id } => {
                if self.fenced.load(Ordering::SeqCst) || self.stopping.load(Ordering::SeqCst) {
                    request.writer.invalidate_status_projection();
                }
                let durable = request
                    .writer
                    .last_confirmed_status(operation_id)
                    .await
                    .map_err(jobs::journal_error)?;
                let local = request.jobs.known(operation_id);
                Ok(Response::reply(BrokerReplyV1::Status {
                    status: match durable {
                        Some(projection) => {
                            jobs::last_confirmed_status(&projection, local.as_deref())
                        }
                        None => jobs::status(operation_id, None, local.as_deref()),
                    },
                }))
            }
            BrokerCommandV1::Result { operation_id, .. } => {
                if self.fenced.load(Ordering::SeqCst) || self.stopping.load(Ordering::SeqCst) {
                    return Err(BrokerErrorCodeV1::NeedsReconciliation);
                }
                // Sample before the durable read so a concurrently finishing
                // task cannot make a pre-completion head look abandoned.
                let execution = request
                    .jobs
                    .known(operation_id)
                    .as_deref()
                    .map(Job::execution_status)
                    .unwrap_or_default();
                let durable = request
                    .writer
                    .result_status(operation_id)
                    .await
                    .map_err(jobs::journal_error)?;
                if self.fenced.load(Ordering::SeqCst) || self.stopping.load(Ordering::SeqCst) {
                    return Err(BrokerErrorCodeV1::NeedsReconciliation);
                }
                Ok(Response::reply(BrokerReplyV1::Result {
                    result: result::project(operation_id, durable, execution),
                }))
            }
            BrokerCommandV1::RelayStatus { operation_id } => {
                // Resolve the authenticated profile's journal first. A caller
                // UUID never selects a tenant or another profile's detail.
                if self.fenced.load(Ordering::SeqCst) || self.stopping.load(Ordering::SeqCst) {
                    request.writer.invalidate_status_projection();
                }
                let durable = request
                    .writer
                    .last_confirmed_status(operation_id)
                    .await
                    .map_err(jobs::journal_error)?;
                if !durable.as_ref().is_some_and(|projection| {
                    projection.entry().kind()
                        == crate::broker::journal::JournalEntryKindV1::Accepted
                }) {
                    return Err(BrokerErrorCodeV1::InvalidInput);
                }
                let relays = self
                    .registry
                    .relay_service
                    .get()
                    .ok_or(BrokerErrorCodeV1::Unavailable)?;
                let status = relays
                    .status(&request.verified.scope, operation_id)
                    .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
                Ok(Response::reply(BrokerReplyV1::RelayStatus { status }))
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
            | BrokerCommandV1::RecoveryStatusV2 { .. }
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
        let egress = self.registry.egress(&profile.id).cloned();
        let operation = submit.operation_id;
        let (mode, generation_and_owner) =
            if require_retained_egress_runtime(egress.as_ref()).is_err() {
                (ReservationMode::ReplayOnly, None)
            } else {
                let job = jobs.job(operation)?;
                let generation = job.cancel.current();
                // Status is only a capacity hint. The same writer retains every
                // observed ID and independently checks exact replay and rollback.
                let owner = if writer
                    .status(operation)
                    .await
                    .map_err(jobs::journal_error)?
                    .is_none()
                {
                    Some(self.reserve_full_owner()?)
                } else {
                    None
                };
                (
                    ReservationMode::AllowNew(egress.clone()),
                    Some((job, generation, owner)),
                )
            };
        // Reserve the exact runnable payload before opening a provider or a
        // source directory. The authentication envelope is never persisted.
        let (ticket, job, preparation_generation, owner) = match writer
            .reserve_with_mode(operation, payload.into_bytes(), mode)
            .await
            .map_err(jobs::journal_error)?
        {
            ReserveResult::Accepted(job) => return Ok(Response::reply(accepted(&job, true))),
            ReserveResult::Rejected(receipt) => {
                return Ok(Response::reply(rejected(&receipt, true)));
            }
            ReserveResult::InProgress => return Err(BrokerErrorCodeV1::Busy),
            ReserveResult::Interrupted => return Err(BrokerErrorCodeV1::Interrupted),
            ReserveResult::Absent => return Err(BrokerErrorCodeV1::IncompatibleControl),
            ReserveResult::New(ticket) => {
                let (job, generation, owner) =
                    generation_and_owner.ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
                let Some(owner) = owner else {
                    // This contradicts the live writer's observed-ID floor.
                    // Preserve the interrupted reservation for reconciliation;
                    // no preparation, acceptance or execution is permitted.
                    job.result(Some(BrokerErrorCodeV1::NeedsReconciliation));
                    drop(ticket);
                    return Err(BrokerErrorCodeV1::NeedsReconciliation);
                };
                (ticket, job, generation, owner)
            }
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
            if let Err(code) = self.require_starts().await {
                // Best-effort diagnostic only; the existing durable rejection
                // below remains authoritative. No free-form error is emitted.
                if let Ok(mut diagnostic) = serde_json::to_vec(&json!({
                    "version": 1,
                    "component": "broker",
                    "event": "launch_admission_refused",
                    "stage": "start_gate",
                    "operation_id": operation,
                    "reason_code": code,
                })) && diagnostic.len() < 256
                {
                    diagnostic.push(b'\n');
                    // One bounded, fallible attempt; even a short write must
                    // not change the refusal or add a retry/owner.
                    let _ = std::io::Write::write(&mut std::io::stderr(), &diagnostic);
                }
                return Err(code);
            }
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
            let session_id = Uuid::new_v4();
            let mut prepared = self
                .filesystem(move || {
                    prepare_broker_submission_with_egress(
                        options, operation, session_id, &literals, egress,
                    )
                })
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
                owner,
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
    async fn load_generic_execution_job(
        &self,
        writer: &WriterClient,
        operation: Uuid,
        profile: &ControlProfileV1,
    ) -> Result<AcceptedJobV1, BrokerErrorCodeV1> {
        let job = self.load_job(writer, operation, profile).await?;
        if job.requires_legacy_named_execution() {
            return Err(BrokerErrorCodeV1::IncompatibleControl);
        }
        Ok(job)
    }
    async fn abandon_before_claim(
        &self,
        request: AuthorizedRequest,
        input: super::AbandonBeforeClaimV1,
    ) -> Result<Response, BrokerErrorCodeV1> {
        input.validate()?;
        let full = self
            .full_enforcer()?
            .full3()
            .ok_or(BrokerErrorCodeV1::Forbidden)?;
        if full.epoch() != input.expected_epoch {
            return Err(BrokerErrorCodeV1::Conflict);
        }
        let local = request
            .jobs
            .known(input.operation_id)
            .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
        let durable = self
            .load_job(&request.writer, input.operation_id, &request.profile)
            .await?;
        let arm_is_confirmed = durable.broker_checkpoint_v2().is_some_and(|checkpoint| {
            checkpoint.supervision().operations().iter().any(|operation| matches!(
                operation,
                crate::broker::checkpoint::SupervisionOperationV1::Arm { request: arm, response: Some(_), .. }
                    if arm.operation_id == input.arm_id.to_string()
            ))
        });
        if !arm_is_confirmed {
            return Err(BrokerErrorCodeV1::Conflict);
        }
        // Refuse saturation/closure before mutating the arbiter or outbox.
        let owner = self
            .full_owners
            .try_reserve_signal()
            .map_err(|error| match error {
                retained_audit::OwnerError::Busy => BrokerErrorCodeV1::Busy,
                _ => BrokerErrorCodeV1::Unavailable,
            })?;
        // The same process-local arbitration closes the claim path immediately.
        // Disappearance of this IPC waiter cannot undo the owned audit task.
        let signal = local.preclaim.request(input.clone())?;
        let mut event = signal.event().clone();
        let context = durable.intent().request_data();
        event.session_id = context.session_id.clone();
        event.project_id = context.project_id.clone();
        event.sandbox_id = context.sandbox_id.clone();
        event.agent_id = context.agent_id.clone();
        event.correlation_id = context.correlation_id.clone();
        event.trace_id = context.trace_id.clone();
        let writer = request.writer;
        let client = request.verified.client;
        let (reply, result) = oneshot::channel();
        owner
            .start(async move {
                let mut audit_confirmed = false;
                let outcome = async {
                    let pending = writer
                        .enqueue_event(input.operation_id, event.clone())
                        .await
                        .map_err(jobs::journal_error)?;
                    let audit_seq = client
                        .post_control_event(
                            serde_json::to_value(&event)
                                .map_err(|_| BrokerErrorCodeV1::InvalidInput)?,
                        )
                        .await
                        .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
                    let confirmed = writer
                        .acknowledge_event(input.operation_id, input.command_id, audit_seq)
                        .await
                        .map_err(jobs::journal_error)?;
                    if confirmed.audit_seq() != Some(audit_seq)
                        || confirmed.input_hash() != pending.input_hash()
                    {
                        return Err(BrokerErrorCodeV1::NeedsReconciliation);
                    }
                    let view = signal.confirm_audit()?;
                    audit_confirmed = true;
                    if view.state == super::preclaim::PreclaimStateV1::StopReserved {
                        return Err(BrokerErrorCodeV1::Interrupted);
                    }
                    // Retain the Job until audit completion even if the requester exits.
                    drop(local);
                    Ok(Response::reply(BrokerReplyV1::AbandonRequested {
                        operation_id: input.operation_id,
                        arm_id: input.arm_id,
                        command_id: input.command_id,
                        input_hash: confirmed.input_hash().to_owned(),
                        revision: confirmed.head().revision(),
                        checkpoint_hash: confirmed.head().hash().to_owned(),
                    }))
                }
                .await;
                let owned_result = if audit_confirmed {
                    // STOP may refuse the waiter after confirmed audit. A
                    // journal Interrupted before that point remains a failure.
                    Ok(())
                } else {
                    outcome.as_ref().map(|_| ()).map_err(|code| *code)
                };
                // A disconnected waiter changes only response delivery. The task
                // outcome and JoinHandle remain in the registry until joined.
                let _ = reply.send(outcome);
                owned_result
            })
            .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
        result.await.map_err(|_| BrokerErrorCodeV1::Unavailable)?
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
        let durable = self
            .load_generic_execution_job(&writer, operation, &profile)
            .await?;
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
        let durable = self
            .load_generic_execution_job(&writer, operation, &profile)
            .await?;
        reject_event_command_reuse(&durable, command)?;
        self.reject_export_command_reuse(&durable, command).await?;
        let (actual, receipt) = jobs::review(&durable).ok_or(BrokerErrorCodeV1::Conflict)?;
        if actual != hash {
            return Err(BrokerErrorCodeV1::Conflict);
        }
        let owner = self.reserve_full_owner()?;
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
                owner,
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
        // Shutdown closes starts and Full admission, but a retained service
        // still accepts authenticated Cancel through the durable STOP path.
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
        if canonical(scope).map_err(jobs::journal_error)?
            != canonical(projection.scope()).map_err(jobs::journal_error)?
        {
            return Err(BrokerErrorCodeV1::Conflict);
        }
        let receipt = projection.receipt().clone();
        let projection = Arc::new(projection);
        async {
            // Handler control-flow only, after the real confirmed projection.
            // This seam supplies no runtime observation or terminal fact.
            #[cfg(test)]
            if let Some(result) = cancel_retry_tests::delivery(operation, cancel.current()) {
                return result;
            }
            let ordinary = async {
                if let Some(enforcer) = &self.stop_enforcer {
                    enforcer
                        .deliver_projection(Arc::clone(&projection), cancel.current())
                        .await
                        .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)
                } else {
                    deliver_cancellation(self.full_enforcer()?, scope, &projection, cancel)
                        .await
                        .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)
                }
            };
            if projection.named_import().is_some() {
                let (regular, named) =
                    tokio::join!(ordinary, self.deliver_named_import_cancel(&projection));
                regular?;
                named
            } else {
                ordinary.await
            }
        }
        .await?;
        // Known native targets were attempted even when coverage was incomplete.
        if projection.incomplete() {
            return Err(BrokerErrorCodeV1::NeedsReconciliation);
        }
        if projection.named_reservation().is_some() {
            let jobs = self.profile_jobs(scope.profile_id())?;
            let control = jobs
                .named_control
                .get()
                .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
            control
                .cancel_legacy_named_reservation(&projection, &self.blocking)
                .await
                .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?;
        }
        Ok(receipt)
    }

    async fn deliver_named_import_cancel(
        &self,
        projection: &CancelStopProjectionV1,
    ) -> Result<(), BrokerErrorCodeV1> {
        let result = match self.full_enforcer() {
            Ok(ExecutionEnforcerV1::Legacy(client)) => {
                client.deliver_named_import_stop(projection).await
            }
            _ if self.recovery_only() => {
                let root = self
                    .root
                    .persistent_child("enforcer", false)
                    .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?
                    .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
                deliver_named_import_stop_from_legacy_root(&root, projection).await
            }
            _ => return Err(BrokerErrorCodeV1::NeedsReconciliation),
        };
        result.map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)
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
        self.load_generic_execution_job(&writer, operation, &profile)
            .await?;
        let job = jobs.job(operation)?;
        let running = job.enter()?;
        let identity = jobs.identity_gate().await?;
        let durable = self
            .load_generic_execution_job(&writer, operation, &profile)
            .await?;
        self.reject_export_command_reuse(&durable, command).await?;
        if durable
            .broker_checkpoint_v3()
            .is_some_and(|cp| cp.commands().iter().any(|c| c.command_id() == command))
        {
            return Err(BrokerErrorCodeV1::Conflict);
        }
        let event = resume_event(&durable, command)?;
        let owner = self.reserve_full_owner()?;
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
                owner,
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
        self.load_generic_execution_job(&writer, operation, &profile)
            .await?;
        let job = jobs.job(operation)?;
        let _running = job.enter()?;
        let identity = jobs.identity_gate().await?;
        let durable = self
            .load_generic_execution_job(&writer, operation, &profile)
            .await?;
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
            Ok(std::ops::ControlFlow::Continue(done)) => {
                job.result(None);
                Ok(Response::reply(BrokerReplyV1::ExportAcknowledged {
                    operation_id: operation,
                    request_id: done.request_id,
                    manifest_digest: done.manifest_digest,
                    files: done.files,
                }))
            }
            Ok(std::ops::ControlFlow::Break(abandoned)) => {
                job.result(None);
                Ok(Response::reply(BrokerReplyV1::AbandonedBeforeClaim {
                    operation_id: abandoned.retirement.operation_id(),
                    arm_id: abandoned.retirement.arm_id(),
                    container_id: abandoned.owned.handle.id().to_owned(),
                    final_revision: abandoned.retirement.final_revision().revision(),
                    final_checkpoint_hash: abandoned.retirement.final_revision().hash().to_owned(),
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
        if let Ok(ExecutionEnforcerV1::Legacy(client)) = self.full_enforcer() {
            // Only the actual spawning caller can hold this native-correlated
            // receipt. Empty-looking DATA or a reconnect cannot create it.
            let fresh = client.legacy_fresh_startup().await == Ok(true);
            if fresh && self.material.complete {
                let root = self.root.clone();
                let no_previous_source = self
                    .filesystem(move || {
                        let Some(journals) = root.persistent_child("journals", false)? else {
                            return Ok(true);
                        };
                        let inventory = journals.persistent_inventory(32)?;
                        Ok(!inventory.incomplete && inventory.entries.is_empty())
                    })
                    .await;
                if no_previous_source == Ok(true) {
                    // No recovery reader/scan is opened on this genuine first
                    // installation. Later profile creation uses its normal
                    // durable writer; this never reopens a recovery gate.
                    return;
                }
            }
            // Existing/cold is already fenced by its child constructor. A
            // fresh role finding ANY previous/unknown journal source must
            // commit the same native fence before opening that source.
            self.fenced.store(true, Ordering::SeqCst);
            if client.legacy_begin_recovery().await.is_err() {
                // Existing native deadlines and the live daemon's exact
                // Status/Slots/Cancel transports remain. No unconfirmed
                // fence authorizes opening a new recovery source scan.
                eprintln!(
                    "broker recovery: Legacy native admission fence unavailable; new starts remain refused"
                );
                return;
            }
        }
        if let Some(stop) = &self.stop_enforcer
            && stop.begin_recovery().await.is_err()
        {
            self.fenced.store(true, Ordering::SeqCst);
            eprintln!(
                "broker recovery: native admission fence is unavailable; source transfer refused"
            );
            return;
        }
        if !self.material.complete {
            self.fenced.store(true, Ordering::SeqCst);
            if let Some(stop) = &self.stop_enforcer {
                stop.mark_recovery_incomplete().await;
            }
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
            if let Some(stop) = &self.stop_enforcer {
                stop.mark_recovery_incomplete().await;
            }
            eprintln!("broker recovery: unknown or unsafe journal profile; new starts fenced");
        }
        // Registry validation caps profiles at 32. Poll every retained profile
        // owner independently, so an unrelated Q wait cannot serialize STOPs
        // in a later profile. Each profile still transfers exactly one factory.
        let mut profiles = futures_util::stream::iter(
            self.registry
                .data
                .profiles
                .iter()
                .map(|profile| self.restore_profile(profile)),
        )
        .buffer_unordered(32);
        while let Some(result) = profiles.next().await {
            match result {
                Ok(count) => restored = restored.saturating_add(count),
                Err(_) => {
                    self.fenced.store(true, Ordering::SeqCst);
                    if let Some(stop) = &self.stop_enforcer {
                        stop.mark_recovery_incomplete().await;
                    }
                    eprintln!(
                        "broker recovery: a configured journal could not be completely restored; new starts fenced"
                    );
                }
            }
        }
        if let Some(enforcer) = &self.stop_enforcer {
            match enforcer.status().await {
                Ok(status)
                    if status.transport_healthy()
                        && status.recovery_fenced()
                        && !status.recovery_incomplete()
                        && status.owned_active() == restored => {}
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
        let jobs = self.profile_jobs(&profile.id)?;
        if let Some(owner) = jobs
            .retirement_startup
            .retained()
            .map_err(jobs::journal_error)?
        {
            return self.resume_startup_profile(profile, &jobs, owner).await;
        }
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
        let has_full3 = self.enforcer.as_ref().is_some_and(|_| {
            self.full_enforcer()
                .ok()
                .and_then(ExecutionEnforcerV1::full3)
                .is_some()
        });
        let split_restore = has_full3
            && report.jobs().iter().any(|job| {
                job.retirements().iter().any(|entry| {
                    matches!(
                        entry,
                        crate::broker::retirement_checkpoint::RetirementEntryV1::Pending(_)
                    )
                })
            });
        // Other STOP targets enter their native guardians before any unrelated
        // Q wait. Pending Arms alone wait for their own Q→F reconciliation.
        let (early_restored, early_partial) = if split_restore {
            self.restore_profile_targets(profile, &report, RestorePass::RequiredOnly)
                .await?
        } else {
            (0, false)
        };
        // Complete the exact guardian result before restoring that same Arm.
        // Keep the reader's lease/core; no business blob is read.
        if self
            .enforcer
            .as_ref()
            .and_then(|_| {
                self.full_enforcer()
                    .ok()
                    .and_then(ExecutionEnforcerV1::full3)
            })
            .is_some()
        {
            let owned =
                Arc::try_unwrap(report).map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?;
            let owner = jobs
                .retirement_startup
                .install_with_progress(
                    owned,
                    super::retirement_startup::RestoreProgress {
                        restored: early_restored,
                        partial: incomplete || early_partial,
                        split: split_restore,
                    },
                )
                .map_err(jobs::journal_error)?;
            return self.resume_startup_profile(profile, &jobs, owner).await;
        }
        self.finish_profile_handoff(
            profile,
            &jobs,
            report,
            None,
            super::retirement_startup::RestoreProgress {
                restored: early_restored,
                partial: incomplete || early_partial,
                split: split_restore,
            },
        )
        .await
    }

    async fn resume_startup_profile(
        &self,
        profile: &ControlProfileV1,
        jobs: &Arc<ProfileJobs>,
        owner: Arc<super::retirement_startup::StartupOwner>,
    ) -> Result<usize, BrokerErrorCodeV1> {
        let _continuation = owner.enter().map_err(jobs::journal_error)?;
        let full = self
            .full_enforcer()?
            .full3()
            .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
        // The original filesystem notification may have disappeared. Resume
        // the already owned step before considering any other Q or factory.
        if owner.has_step().map_err(jobs::journal_error)? {
            let dispatched = Arc::clone(&owner);
            self.filesystem(move || {
                dispatched.process()?;
                Ok(())
            })
            .await?;
            owner
                .confirm_step_with_original_slot(full)
                .map_err(jobs::journal_error)?;
        }
        // A completed handoff is recognized rather than calling the factory a
        // second time after loss of its notification.
        if jobs.writer.get().is_some() {
            let report = owner.completed_report().map_err(jobs::journal_error)?;
            if !report.retirement_factory_consumed() {
                return Err(BrokerErrorCodeV1::NeedsReconciliation);
            }
            let (count, _) = owner
                .restored()
                .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
            jobs.retirement_startup
                .release_after_factory(&owner)
                .map_err(jobs::journal_error)?;
            return Ok(count);
        }
        let report = if let Ok(report) = owner.completed_report() {
            report
        } else {
            // Sequential Q avoids producing additional live wrappers after
            // the first failure. The profile already owns its actual report.
            while let Some(identity) = owner.next_query().map_err(jobs::journal_error)? {
                let local = jobs.job(*identity.job_operation_id())?;
                let arm = Uuid::parse_str(&identity.arm().operation_id)
                    .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?;
                let verified = match full.query_retirement(&identity).await {
                    Ok(Some(verified)) => verified,
                    Ok(None) => {
                        owner.no_query_receipt().map_err(jobs::journal_error)?;
                        continue;
                    }
                    Err(_) => {
                        owner.query_failed(&identity).map_err(jobs::journal_error)?;
                        return Err(BrokerErrorCodeV1::NeedsReconciliation);
                    }
                };
                match owner.retain_verified(verified) {
                    Ok(()) => {}
                    Err(super::retirement_startup::InstallError::Retained(error)) => {
                        return Err(jobs::journal_error(error));
                    }
                    Err(super::retirement_startup::InstallError::Returned { error, input }) => {
                        local.preclaim.retain_retirement(
                            arm,
                            *identity.original_epoch(),
                            *identity.generation(),
                            super::preclaim::RetainedRetirementV1::Verified {
                                _owner: Box::new(input),
                            },
                        );
                        return Err(jobs::journal_error(error));
                    }
                }
                let dispatched = Arc::clone(&owner);
                // Notification only. Report/result/Verified are stored in the
                // actual profile owner before this closure can return or fail.
                self.filesystem(move || {
                    dispatched.process()?;
                    Ok(())
                })
                .await?;
                owner
                    .confirm_step_with_original_slot(full)
                    .map_err(jobs::journal_error)?;
            }
            owner.completed_report().map_err(jobs::journal_error)?
        };
        let progress = owner.restore_progress();
        self.finish_profile_handoff(profile, jobs, report, Some(owner), progress)
            .await
    }

    async fn finish_profile_handoff(
        &self,
        profile: &ControlProfileV1,
        jobs: &Arc<ProfileJobs>,
        report: Arc<StopRecoveryReportV1>,
        startup_owner: Option<Arc<super::retirement_startup::StartupOwner>>,
        progress: super::retirement_startup::RestoreProgress,
    ) -> Result<usize, BrokerErrorCodeV1> {
        let scope = report.scope().clone();
        let (restored, mut partial) = if let Some(completed) =
            startup_owner.as_ref().and_then(|owner| owner.restored())
        {
            completed
        } else {
            let (ordinary, named_partial) = tokio::join!(
                self.restore_profile_targets(
                    profile,
                    &report,
                    if progress.split {
                        RestorePass::PendingOnly
                    } else {
                        RestorePass::All
                    },
                ),
                self.restore_named_import_stops(&report),
            );
            let (later_restored, later_partial) = ordinary?;
            let restored = progress.restored.saturating_add(later_restored);
            let partial = progress.partial || report.incomplete() || later_partial || named_partial;
            if let Some(owner) = &startup_owner {
                owner
                    .record_restored(restored, partial)
                    .map_err(jobs::journal_error)?;
            }
            (restored, partial)
        };
        if partial {
            self.fenced.store(true, Ordering::SeqCst);
        }
        let proof = Arc::clone(&report);
        partial |= self
            .filesystem(move || Ok(proof.revalidate()?))
            .await
            .is_err();
        // One factory inherits the retained reader/core after both passes.
        let proof = Arc::clone(&report);
        // Resolving this owner's IO ambiguity never restores business
        // admission. Process loss, a missing/divergent F or a failed Q remain
        // separate retained failures; no caller may mint a replacement owner.
        let force_stops = partial
            || self.stop_enforcer.is_some()
            || self.stopping.load(Ordering::Acquire)
            || startup_owner
                .as_ref()
                .is_some_and(|owner| owner.had_uncertainty());
        let factory_jobs = Arc::clone(jobs);
        let factory_scope = scope.clone();
        match self
            .filesystem(move || {
                let capabilities = ProfileWriterCapabilities::from_recovery_report(
                    &proof,
                    force_stops,
                    64,
                    WRITER_TIMEOUT,
                )?;
                let restricted = capabilities.is_stop_only();
                ensure!(
                    canonical(capabilities.stops().scope())? == canonical(&factory_scope)?,
                    "recovery factory scope changed"
                );
                factory_jobs
                    .writer
                    .set(capabilities)
                    .map_err(|_| anyhow!("recovery writer already installed"))?;
                if let Some(owner) = startup_owner {
                    factory_jobs
                        .retirement_startup
                        .release_after_factory(&owner)?;
                }
                Ok(restricted)
            })
            .await
        {
            Ok(restricted) => partial |= restricted,
            Err(_) => partial = true,
        }
        if partial {
            self.fenced.store(true, Ordering::SeqCst);
        }
        Ok(restored)
    }

    async fn restore_named_import_stops(&self, report: &Arc<StopRecoveryReportV1>) -> bool {
        if !self.recovery_only()
            && !matches!(self.full_enforcer(), Ok(ExecutionEnforcerV1::Legacy(_)))
        {
            return false;
        }
        let mut attempts =
            futures_util::stream::iter(report.named_import_stop_jobs().map(|index| async move {
                let proof = Arc::clone(report);
                let projection = self
                    .filesystem(move || Ok(proof.named_import_stop_projection(index)?))
                    .await;
                match projection {
                    Ok(Some(projection)) => {
                        self.deliver_named_import_cancel(&projection).await.is_err()
                    }
                    Ok(None) | Err(_) => true,
                }
            }))
            .buffer_unordered(16);
        let mut partial = false;
        while let Some(failed) = attempts.next().await {
            partial |= failed;
        }
        partial
    }

    async fn restore_profile_targets(
        &self,
        profile: &ControlProfileV1,
        report: &Arc<StopRecoveryReportV1>,
        pass: RestorePass,
    ) -> Result<(usize, bool), BrokerErrorCodeV1> {
        let jobs = self.profile_jobs(&profile.id)?;
        let mut restored = 0_usize;
        let mut partial = report.incomplete();
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
            let proof = Arc::clone(report);
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
            tasks.spawn(restore_recovered_job_pass(
                full,
                self.stop_enforcer.clone(),
                Arc::clone(report),
                index,
                job,
                pass,
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
        let proof = Arc::clone(report);
        partial |= self
            .filesystem(move || Ok(proof.revalidate()?))
            .await
            .is_err();
        Ok((restored, partial))
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
            // Audit completion is metadata-only and remains available while
            // starts are fenced or the writer has only its STOP facade.
            if self.validate_registry().await.is_ok() {
                self.reconcile_retirement_audit().await;
            }
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
                // The writer is one FIFO actor shared with this profile's
                // executions. Yield this tick to their commits (bounded).
                if !jobs.admit_reconcile(writer.outstanding()) {
                    continue;
                }
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
                        // A Job whose gate is held cannot be entered by this
                        // pass below; do not queue its strict load either.
                        if jobs.known(operation).is_some_and(|job| job.is_running()) {
                            continue;
                        }
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
                        let Ok(owner) = self.reserve_full_owner() else {
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
                        let _ = DeferredRun {
                            owner,
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

    async fn reconcile_retirement_audit(self: &Arc<Self>) {
        let mut profiles = futures_util::stream::FuturesUnordered::new();
        // Each future owns its service/profile. No borrowed iterator is held
        // across awaits in the spawned reconciler; the registry cap is 32.
        for profile in self.registry.data.profiles.iter().cloned() {
            profiles.push(Arc::clone(self).reconcile_retirement_audit_profile(profile));
        }
        while let Some(result) = profiles.next().await {
            if result.is_err() {
                eprintln!(
                    "broker retirement audit reconciliation is unavailable for a configured profile"
                );
            }
        }
    }

    async fn reconcile_retirement_audit_profile(
        self: Arc<Self>,
        profile: ControlProfileV1,
    ) -> Result<(), BrokerErrorCodeV1> {
        let jobs = self.profile_jobs(&profile.id)?;
        if let Some(owner) = jobs
            .retirement_startup
            .retained()
            .map_err(jobs::journal_error)?
            && let Err(error) = self.resume_startup_profile(&profile, &jobs, owner).await
        {
            self.fenced.store(true, Ordering::Release);
            return Err(error);
        }
        let Some(capabilities) = jobs.writer.get() else {
            return Ok(());
        };
        let writer = capabilities.stops();
        // Same FIFO actor as this profile's executions: yield (bounded) while
        // a caller awaits its reply. Pending events stay durable in the
        // journal and a later tick delivers them, even under a live owner.
        if !jobs.admit_retirement_audit(writer.outstanding()) {
            return Ok(());
        }
        let material = Arc::clone(&self.material);
        let credential_profile = profile.clone();
        let credential = self
            .filesystem(move || material.checked_reconciler(&credential_profile))
            .await?;
        let verified =
            VerifiedProfile::authenticate(&self.registry, &profile.id, &credential, false).await?;
        if canonical(writer.scope()).map_err(jobs::journal_error)?
            != canonical(&verified.scope).map_err(jobs::journal_error)?
        {
            return Err(BrokerErrorCodeV1::Conflict);
        }
        let client = verified.client.with_secrets(self.material.literals.clone());
        let full = self.enforcer.as_ref().and_then(|_| {
            self.full_enforcer()
                .ok()
                .and_then(ExecutionEnforcerV1::full3)
                .cloned()
        });
        let mut operations = futures_util::stream::FuturesUnordered::new();
        for operation in jobs.retirement_audit_batch()? {
            let writer = writer.clone();
            let client = client.clone();
            let full = full.clone();
            let profile_jobs = Arc::clone(&jobs);
            operations.push(async move {
                if let Some(full) = full {
                    #[cfg(test)]
                    let mut test_completed = None;
                    // Q work is one entry per job/round and cannot suppress
                    // that job's independently promised audit delivery.
                    let reconciliation: Result<(), BrokerErrorCodeV1> = async {
                        let Some(retirement_query) =
                            profile_jobs.try_retirement_query(operation)?
                        else {
                            // The original Full/Cold producer still owns this
                            // Job. Independent audit delivery below continues.
                            return Ok(());
                        };
                        let operation = retirement_query.operation_id();
                        let local = Arc::clone(retirement_query.job());
                        let Some(mut snapshot) = writer
                            .recovery_status(operation)
                            .await
                            .map_err(jobs::journal_error)?
                        else {
                            return Ok(());
                        };
                        let mut candidates = Vec::new();
                        for entry in snapshot.retirements() {
                            if entry.request().audit_ack().is_none() {
                                continue;
                            }
                            if entry.finalized().is_some()
                                && !full
                                    .has_local_slot(&entry.request().identity().arm().operation_id)
                                    .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?
                            {
                                continue;
                            }
                            candidates.push(*entry.request().identity().retirement_id());
                        }
                        let Some(id) = local.next_retirement_query(&candidates)? else {
                            return Ok(());
                        };
                        let entry = snapshot
                            .retirements()
                            .iter()
                            .find(|entry| *entry.request().identity().retirement_id() == id)
                            .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
                        if local.preclaim.has_retained_retirement() {
                            // A keeper never falls through to the generic Q/F
                            // path. Only its exact Submitted+confirmed-F owner
                            // may finish local cleanup; every other cap stays.
                            return retirement_settlement::settle_confirmed_final(
                                &retirement_query,
                                &writer,
                                &full,
                                &snapshot,
                                id,
                            )
                            .await;
                        }
                        let identity = entry.request().identity();
                        let arm_id = Uuid::parse_str(&identity.arm().operation_id)
                            .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?;
                        #[cfg(test)]
                        {
                            test_completed =
                                profile_jobs.retirement_snapshot_boundary_for_test().await;
                        }
                        let recovered =
                            match writer.recover_final_retirement_state(operation, arm_id, id) {
                                Ok(crate::broker::writer::FinalRetirementRecovery::Absent) => None,
                                Ok(crate::broker::writer::FinalRetirementRecovery::InFlight) => {
                                    // Its original input is already owned by the
                                    // writer. No new Q/wrapper/Final is admissible.
                                    // The independent audit delivery below proceeds.
                                    return Ok(());
                                }
                                Ok(crate::broker::writer::FinalRetirementRecovery::Completed(
                                    receipt,
                                )) => Some(receipt),
                                Err(crate::broker::writer::RetirementFailure::Refused {
                                    continuation,
                                    ..
                                }) => {
                                    // The same strong entry survives this await and
                                    // also survives mapping/dropping a further Err.
                                    Some(
                                        writer.retry_final_retirement(continuation).await.map_err(
                                            |failure| jobs::journal_error(failure.error()),
                                        )?,
                                    )
                                }
                                Err(failure) => return Err(jobs::journal_error(failure.error())),
                            };
                        // Absent is only a registry fact: another real caller
                        // may already have consumed Completed after this old
                        // snapshot. Re-read even then; absence grants no Final
                        // or permission. The enclosing Job guard coordinates
                        // this Q/admission path with the live Full producer.
                        let request = canonical(entry.request()).map_err(jobs::journal_error)?;
                        let current = writer
                            .recovery_status(operation)
                            .await
                            .map_err(jobs::journal_error)?
                            .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
                        let current_entry = current
                            .retirements()
                            .iter()
                            .find(|entry| *entry.request().identity().retirement_id() == id)
                            .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
                        if current.operation_id() != operation
                            || current.head().revision() < snapshot.head().revision()
                            || (current.head().revision() == snapshot.head().revision()
                                && current.head() != snapshot.head())
                            || canonical(current_entry.request()).map_err(jobs::journal_error)?
                                != request
                        {
                            return Err(BrokerErrorCodeV1::NeedsReconciliation);
                        }
                        if let Some(receipt) = recovered {
                            // Completed/retry positively confirmed Final;
                            // unlike Absent, it requires that exact Final now.
                            if receipt.operation_id() != operation
                                || current.head().revision() < receipt.head().revision()
                                || (current.head().revision() == receipt.head().revision()
                                    && current.head() != receipt.head())
                                || current_entry.finalized().is_none()
                            {
                                return Err(BrokerErrorCodeV1::NeedsReconciliation);
                            }
                        }
                        snapshot = current;
                        let entry = snapshot
                            .retirements()
                            .iter()
                            .find(|entry| *entry.request().identity().retirement_id() == id)
                            .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
                        #[cfg(test)]
                        if let Some(completed) =
                            profile_jobs.retirement_before_q_boundary_for_test().await
                        {
                            test_completed = Some(completed);
                        }
                        let Some(verified) = full
                            .query_retirement(entry.request().identity())
                            .await
                            .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?
                        else {
                            return Ok(());
                        };
                        if let Some(final_entry) = entry.finalized() {
                            // Final data alone cannot remove a slot. Obtain a
                            // fresh authenticated Q and compare its exact receipt.
                            if verified.receipt().digest().map_err(jobs::journal_error)?
                                != final_entry
                                    .receipt()
                                    .digest()
                                    .map_err(jobs::journal_error)?
                            {
                                return Err(BrokerErrorCodeV1::NeedsReconciliation);
                            }
                            full.retire_local_slot(&verified)
                                .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?;
                        } else {
                            let identity = verified.identity().clone();
                            let receipt_hash =
                                verified.receipt().digest().map_err(jobs::journal_error)?;
                            #[cfg(test)]
                            if let Some(completed) =
                                profile_jobs.retirement_after_q_boundary_for_test().await
                            {
                                // The actual authenticated Q is already owned;
                                // pause only before its real Final admission.
                                test_completed = Some(completed);
                            }
                            if let Err(failure) = writer.finalize_retirement(verified).await {
                                let code = jobs::journal_error(failure.error());
                                if let crate::broker::writer::RetirementFailure::NotQueued {
                                    input,
                                    ..
                                } = failure
                                {
                                    local.preclaim.retain_retirement(
                                        arm_id,
                                        *identity.original_epoch(),
                                        *identity.generation(),
                                        super::preclaim::RetainedRetirementV1::Verified {
                                            _owner: Box::new(input),
                                        },
                                    );
                                }
                                // Refused/Unknown leave their actual input in
                                // the original writer registry, not this Err.
                                return Err(code);
                            }
                            let replay = full
                                .query_retirement(&identity)
                                .await
                                .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?
                                .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
                            if replay.receipt().digest().map_err(jobs::journal_error)?
                                != receipt_hash
                            {
                                return Err(BrokerErrorCodeV1::NeedsReconciliation);
                            }
                            full.retire_local_slot(&replay)
                                .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?;
                        }
                        // Keep the exact Job mutex across every snapshot, Q,
                        // retained Final/retry and local-slot removal. Early
                        // returns, cancellation and unwind drop it as well.
                        drop(retirement_query);
                        Ok(())
                    }
                    .await;
                    #[cfg(test)]
                    if let Some(completed) = test_completed {
                        let _ = completed.send(());
                    }
                    if reconciliation.is_err() {
                        eprintln!(
                            "broker retirement guardian result remains unknown for a known Arm"
                        );
                    }
                }
                let Some(event) = writer
                    .pending_retirement_events(operation)
                    .await
                    .map_err(jobs::journal_error)?
                    .into_iter()
                    .next()
                else {
                    return Ok::<(), BrokerErrorCodeV1>(());
                };
                let sequence = client
                    .post_control_event(
                        serde_json::to_value(event.event())
                            .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?,
                    )
                    .await
                    .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
                writer
                    .acknowledge_retirement_events(
                        operation,
                        vec![(event.event_id(), event.input_hash().to_owned(), sequence)],
                    )
                    .await
                    .map_err(jobs::journal_error)?;
                Ok(())
            });
        }
        while let Some(result) = operations.next().await {
            if result.is_err() {
                eprintln!("broker retirement audit remains pending for a known job");
            }
        }
        Ok(())
    }
}

/// No namespace is created for recovery. A partial sibling inventory fences
/// admissions, while the reader can still prove the configured scope's stops.
fn verify_fresh_installation(
    root: &Directory,
    registry: &Registry,
    material: &Material,
) -> Result<Vec<Arc<StopRecoveryReportV1>>> {
    registry.source.revalidate()?;
    registry.source_directory.revalidate()?;
    root.revalidate()?;
    material.revalidate()?;
    ensure!(
        material.complete,
        "fresh enforcer requires complete control material"
    );
    let Some(journals) = root.persistent_child("journals", false)? else {
        return Ok(Vec::new());
    };
    let known = registry
        .data
        .profiles
        .iter()
        .map(|profile| profile.id.as_str())
        .collect::<BTreeSet<_>>();
    let inventory = journals.persistent_inventory(32)?;
    ensure!(
        !inventory.incomplete
            && inventory
                .entries
                .iter()
                .all(|entry| { entry.directory && known.contains(entry.name.as_str()) }),
        "fresh enforcer requires complete configured journal inventory"
    );
    let mut reports = Vec::new();
    for profile in &registry.data.profiles {
        let scope = JournalScopeV1::new(
            registry.data.installation_id,
            &profile.tenant_id,
            &profile.gateway,
            profile.control_namespace_uuid,
            &profile.id,
        )?;
        let (report, incomplete) = recover_existing_profile(root, scope, material)?;
        ensure!(
            !incomplete,
            "fresh enforcer journal inventory is incomplete"
        );
        if let Some(report) = report {
            // Conservative initial cutover: only an empty confirmed profile is
            // accepted. Existing jobs keep their original guardian authority.
            // No empty/unknown projection is treated as proof of no old Arm.
            ensure!(
                !report.incomplete() && report.jobs().is_empty(),
                "fresh enforcer requires empty confirmed journal profiles"
            );
            report.revalidate()?;
            reports.push(report);
        }
    }
    Ok(reports)
}

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

#[derive(Clone, Copy, PartialEq, Eq)]
enum RestorePass {
    All,
    RequiredOnly,
    PendingOnly,
}

#[cfg(test)]
async fn restore_recovered_job(
    enforcer: Option<ExecutionEnforcerV1>,
    stop_enforcer: Option<EnforcerStopClient>,
    report: Arc<StopRecoveryReportV1>,
    index: usize,
    job: Arc<Job>,
) -> (usize, bool) {
    restore_recovered_job_pass(
        enforcer,
        stop_enforcer,
        report,
        index,
        job,
        RestorePass::All,
    )
    .await
}

async fn restore_recovered_job_pass(
    enforcer: Option<ExecutionEnforcerV1>,
    stop_enforcer: Option<EnforcerStopClient>,
    report: Arc<StopRecoveryReportV1>,
    index: usize,
    job: Arc<Job>,
    pass: RestorePass,
) -> (usize, bool) {
    let Some(recovered) = report.jobs().get(index) else {
        job.result(Some(BrokerErrorCodeV1::NeedsReconciliation));
        return (0, true);
    };
    if let Some(stop) = stop_enforcer {
        // This port produces STOP owners only. Existing Full3 P/Q/F metadata
        // reconciliation precedes that Arm's Pending pass; it never creates
        // a Q/R+ or Original capability from a restored snapshot.
        // The client owns retention and control confirmations for the whole
        // job independently of this waiter; the count is not a runtime effect.
        let (restored, partial) = stop
            .restore_report_job_pass(
                Arc::clone(&report),
                index,
                job.cancel.current(),
                match pass {
                    RestorePass::All => None,
                    RestorePass::RequiredOnly => Some(false),
                    RestorePass::PendingOnly => Some(true),
                },
            )
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
        let pending = matches!(
            target.retirement(),
            Some(crate::broker::retirement_checkpoint::RetirementEntryV1::Pending(_))
        );
        if (pass == RestorePass::RequiredOnly && pending)
            || (pass == RestorePass::PendingOnly && !pending)
        {
            continue;
        }
        if matches!(
            target.retirement(),
            Some(crate::broker::retirement_checkpoint::RetirementEntryV1::Final(_))
        ) {
            // The verified chain retains F, not a runnable STOP authority.
            continue;
        }
        let registration = RegistrationV1::new(
            origin.clone(),
            report.scope().tenant_id().to_owned(),
            target.arm().clone(),
            target.receipt().clone(),
            target.authority().clone(),
            target.owned().clone(),
        );
        let restored = match registration {
            Ok(registration) => match (enforcer.full3(), target.full3_registration_binding()) {
                (Some(_), Some(binding)) => {
                    enforcer
                        .restore_with_binding(registration, job.cancel.clone(), binding)
                        .await
                }
                (Some(_), None) => Err(crate::broker::enforcer::EnforcerError::Unauthorized),
                (None, Some(_)) => Err(crate::broker::enforcer::EnforcerError::Unauthorized),
                (None, None) => enforcer.restore(registration, job.cancel.clone()).await,
            },
            Err(error) => Err(error),
        };
        match restored {
            Ok(slot) => slots.push((slot, target.cancel_after_arm().is_some())),
            Err(_) => partial = true,
        }
    }
    // Restores first seed the shared generation from every acknowledged child
    // slot. Numeric generations are not the causal proof of cancellation.
    if !recovered.cancel_command_ids().is_empty() && (pass == RestorePass::All || !slots.is_empty())
    {
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
    if job.requires_legacy_named_execution() {
        return false;
    }
    if job
        .broker_checkpoint_v5()
        .is_some_and(|checkpoint| !checkpoint.retirements().is_empty())
    {
        // Abandonment inhibits continuation. A durable Pending/Final is never
        // converted into an automatic new claim, terminal or quota release.
        return false;
    }
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
    let cancelled = job.broker_checkpoint_v3().is_some_and(|checkpoint| {
        checkpoint
            .commands()
            .iter()
            .any(|command| matches!(command.request(), LocalCommandRequestV1::CancelRequested))
    });
    context.worker.is_none_or(|worker| {
        worker.state != crate::control::WorkerStateV1::Terminal
            || worker.quota_held
            || (!cancelled && worker.cleanup_removed != Some(true))
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
