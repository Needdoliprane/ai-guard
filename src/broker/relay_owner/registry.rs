//! Retained service registry. Task results containing transport/IO owners are
//! kept in their entry until explicit joins and exact Worker terminal evidence.
use super::journal::{AttachmentPhase, ChannelFact};
use super::*;
use crate::{
    broker::{
        discovery::RoleLease,
        service::{AuthenticatedBatchContext, protocol::*},
    },
    control_fs::Directory,
    relay::{admission::*, channel::control::TenantRelayIo},
    runner::{
        AuditClient,
        relay_admission::{CurrentRelayAuthority, RelayExecClaimResult},
    },
    runtime::{ContainerSpec, ManagedRuntime, OwnedContainerSnapshot},
    workload_cli::broker_binding::RetainedWorkerBinding,
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::{
    sync::{Mutex as AsyncMutex, Semaphore, watch},
    task::JoinHandle,
};
use uuid::Uuid;
mod attachment;
mod retirement;

struct TenantState {
    scope: JournalScopeV1,
    init: AsyncMutex<Option<JoinHandle<Result<Arc<TenantRelayIo>>>>>,
    io: Mutex<Option<Arc<TenantRelayIo>>>,
    literals: Mutex<Vec<String>>,
    bytes: Arc<Semaphore>,
    slots: Arc<Semaphore>,
    poisoned: AtomicBool,
}
struct Entry {
    source: Arc<Source>,
    view: Mutex<Option<ChannelFact>>,
    error: Mutex<Option<RelayOwnerError>>,
    stop: watch::Sender<bool>,
    task: AsyncMutex<Option<JoinHandle<()>>>,
    retained: AsyncMutex<Option<Retained>>,
    handoff_started: AtomicBool,
    no_channel: AtomicBool,
}
struct Source {
    scope: JournalScopeV1,
    job_id: Uuid,
    head_revision: u64,
    head_hash: String,
    worker: WorkerV1,
    handle: crate::runtime::ContainerHandle,
    deadline: Arc<WorkerRelayDeadline>,
    execution: Option<ExecutionSource>,
    client: AuditClient,
    gateway_trust: crate::tls::ClientTrust,
}
struct ExecutionSource {
    runtime: ManagedRuntime,
    spec: ContainerSpec,
}
struct Retained {
    source: Arc<Source>,
    tenant: Arc<TenantState>,
    io: Arc<TenantRelayIo>,
    fact: ChannelFact,
    _authority: Option<Arc<DispatchAuthority>>,
    transport: Option<crate::runtime::RelayExecTransport>,
    owner: Option<crate::relay::channel::control::ControlOwner>,
    terminal_observed: bool,
}

pub(crate) struct RelayRegistry {
    root: Directory,
    role: Arc<RoleLease>,
    configured: BTreeMap<String, Vec<PathBuf>>,
    literals: Vec<String>,
    tenants: Mutex<BTreeMap<String, Arc<TenantState>>>,
    entries: Mutex<BTreeMap<(String, Uuid), Arc<Entry>>>,
    global_bytes: Arc<Semaphore>,
    global_io: Arc<Semaphore>,
    stopping: AtomicBool,
    retention: Mutex<Option<Arc<Self>>>,
}
impl std::fmt::Debug for RelayRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RelayRegistry([private retained owners])")
    }
}
impl RelayRegistry {
    fn check_source(&self, entry: &Entry, source: &Source) -> Result<()> {
        if self.stopping.load(Ordering::SeqCst) || *entry.stop.borrow() {
            return Err(RelayOwnerError::Cancelled);
        }
        source.deadline.check()
    }
    async fn while_active<T>(
        &self,
        entry: &Entry,
        source: &Source,
        work: impl std::future::Future<Output = T>,
    ) -> Result<T> {
        let mut stop = entry.stop.subscribe();
        self.check_source(entry, source)?;
        tokio::select! {
            biased;
            _ = stop.changed() => Err(RelayOwnerError::Cancelled),
            error = source.deadline.wait_expiration() => Err(error),
            result = work => { self.check_source(entry, source)?; Ok(result) },
        }
    }
    pub(crate) fn new(
        root: Directory,
        role: Arc<RoleLease>,
        configured: BTreeMap<String, Vec<PathBuf>>,
        literals: Vec<String>,
    ) -> Arc<Self> {
        Arc::new(Self {
            root,
            role,
            configured,
            literals,
            tenants: Mutex::new(BTreeMap::new()),
            entries: Mutex::new(BTreeMap::new()),
            global_bytes: Arc::new(Semaphore::new(128 * 1024 * 1024)),
            global_io: Arc::new(Semaphore::new(16)),
            stopping: AtomicBool::new(false),
            retention: Mutex::new(None),
        })
    }
    fn tenant(&self, scope: &JournalScopeV1) -> Result<Arc<TenantState>> {
        let shared = JournalScopeV1::new(
            scope.installation_id(),
            scope.tenant_id(),
            scope.gateway_origin(),
            scope.control_namespace_uuid(),
            "relay_shared",
        )
        .map_err(|_| RelayOwnerError::Scope)?;
        let mut tenants = self
            .tenants
            .lock()
            .map_err(|_| RelayOwnerError::Unavailable)?;
        if let Some(tenant) = tenants.get(scope.tenant_id()) {
            if super::super::journal::canonical(&tenant.scope).ok()
                != super::super::journal::canonical(&shared).ok()
            {
                return Err(RelayOwnerError::Scope);
            }
            return Ok(tenant.clone());
        }
        if tenants.len() >= 32 {
            return Err(RelayOwnerError::Capacity);
        }
        let tenant = Arc::new(TenantState {
            scope: shared,
            init: AsyncMutex::new(None),
            io: Mutex::new(None),
            literals: Mutex::new(self.literals.clone()),
            bytes: Arc::new(Semaphore::new(32 * 1024 * 1024)),
            slots: Arc::new(Semaphore::new(4)),
            poisoned: AtomicBool::new(false),
        });
        tenants.insert(scope.tenant_id().into(), tenant.clone());
        Ok(tenant)
    }
    async fn tenant_io(&self, tenant: &Arc<TenantState>) -> Result<Arc<TenantRelayIo>> {
        if tenant.poisoned.load(Ordering::SeqCst) {
            return Err(RelayOwnerError::Unknown);
        }
        if let Some(io) = tenant
            .io
            .lock()
            .map_err(|_| RelayOwnerError::Unavailable)?
            .clone()
        {
            return Ok(io);
        }
        let mut init = tenant.init.lock().await;
        if tenant.poisoned.load(Ordering::SeqCst) {
            return Err(RelayOwnerError::Unknown);
        }
        if let Some(io) = tenant
            .io
            .lock()
            .map_err(|_| RelayOwnerError::Unavailable)?
            .clone()
        {
            return Ok(io);
        }
        if init.is_none() {
            let slot = tenant
                .slots
                .clone()
                .try_acquire_owned()
                .map_err(|_| RelayOwnerError::Capacity)?;
            let global_slot = self
                .global_io
                .clone()
                .try_acquire_owned()
                .map_err(|_| RelayOwnerError::Capacity)?;
            let bytes = tenant
                .bytes
                .clone()
                .try_acquire_many_owned(3 * 1024 * 1024)
                .map_err(|_| RelayOwnerError::Capacity)?;
            let global_bytes_permit = self
                .global_bytes
                .clone()
                .try_acquire_many_owned(3 * 1024 * 1024)
                .map_err(|_| RelayOwnerError::Capacity)?;
            let scope = tenant.scope.clone();
            let role = self.role.clone();
            let root = self.root.clone();
            let literals = tenant
                .literals
                .lock()
                .map_err(|_| RelayOwnerError::Unavailable)?
                .clone();
            let tenant_bytes = tenant.bytes.clone();
            let tenant_io = tenant.slots.clone();
            let global_bytes = self.global_bytes.clone();
            let global_io = self.global_io.clone();
            *init = Some(tokio::task::spawn_blocking(move || {
                let _retained = (slot, global_slot, bytes, global_bytes_permit, role.clone());
                use sha2::{Digest, Sha256};
                let key = hex::encode(Sha256::digest(
                    super::super::journal::canonical(&scope).map_err(|_| RelayOwnerError::Scope)?,
                ));
                for name in ["relay".to_owned(), key.clone(), format!("relay/{key}")] {
                    crate::relay::channel::control::validate_private_record(
                        &serde_json::to_vec(&name).map_err(|_| RelayOwnerError::InvalidMaterial)?,
                        &literals,
                    )
                    .map_err(|_| RelayOwnerError::InvalidMaterial)?;
                }
                let relay = root
                    .persistent_child("relay", true)
                    .map_err(|_| RelayOwnerError::InvalidMaterial)?
                    .ok_or(RelayOwnerError::InvalidMaterial)?;
                let parent = relay
                    .persistent_child(&key, true)
                    .map_err(|_| RelayOwnerError::InvalidMaterial)?
                    .ok_or(RelayOwnerError::InvalidMaterial)?;
                TenantRelayIo::open(
                    &parent,
                    &scope,
                    role,
                    literals,
                    tenant_bytes,
                    tenant_io,
                    global_bytes,
                    global_io,
                )
                .map_err(|_| RelayOwnerError::InvalidMaterial)
            }));
        }
        let task = init.as_mut().ok_or(RelayOwnerError::Unknown)?;
        let result = task.await;
        init.take();
        let io = match result {
            Ok(Ok(io)) => io,
            _ => {
                tenant.poisoned.store(true, Ordering::SeqCst);
                return Err(RelayOwnerError::Unknown);
            }
        };
        *tenant.io.lock().map_err(|_| RelayOwnerError::Unavailable)? = Some(io.clone());
        Ok(io)
    }
    /// Called from both actual started and durable-claim recovery paths. No new
    /// start or credential is issued here; a duplicated job retains its owner.
    pub(crate) async fn started(
        self: &Arc<Self>,
        context: &AuthenticatedBatchContext,
        job: AcceptedJobV1,
        runtime: ManagedRuntime,
        spec: ContainerSpec,
        clock: Arc<dyn HostClock>,
    ) -> Result<()> {
        // Recovery may observe Stopping/InDoubt before the terminal receipt.
        // Such a state grants no relay exec and must not block the independent
        // terminal observer or STOP. Status remains explicitly unavailable.
        if job
            .broker_checkpoint_v2()
            .and_then(|checkpoint| {
                checkpoint
                    .supervision()
                    .operations()
                    .iter()
                    .rev()
                    .find_map(|operation| match operation {
                        SupervisionOperationV1::WorkerMutation {
                            response: Some(result),
                            ..
                        } => Some(result.worker.state),
                        _ => None,
                    })
            })
            .is_some_and(|state| state != WorkerStateV1::Running)
        {
            return Ok(());
        }
        self.attach(
            context,
            &job,
            Some(ExecutionSource { runtime, spec }),
            clock,
            false,
        )
        .await
    }
    pub(crate) async fn retire(
        self: &Arc<Self>,
        context: &AuthenticatedBatchContext,
        job: AcceptedJobV1,
        clock: Arc<dyn HostClock>,
    ) -> Result<()> {
        self.attach(context, &job, None, clock, true).await?;
        self.worker_terminal(&job).await
    }
    async fn attach(
        self: &Arc<Self>,
        context: &AuthenticatedBatchContext,
        job: &AcceptedJobV1,
        execution: Option<ExecutionSource>,
        clock: Arc<dyn HostClock>,
        terminal_recovery: bool,
    ) -> Result<()> {
        if !self.configured.contains_key(context.scope().profile_id()) {
            return Ok(());
        }
        if !terminal_recovery && self.stopping.load(Ordering::SeqCst) {
            return Err(RelayOwnerError::Cancelled);
        }
        let (deadline, worker) = WorkerRelayDeadline::from_job(
            job,
            context.scope(),
            clock,
            context.cancel_generation().clone(),
            context.expected_generation(),
            terminal_recovery,
        )?;
        let checkpoint = job.broker_checkpoint_v2().ok_or(RelayOwnerError::Scope)?;
        let handle = checkpoint
            .supervision()
            .operations()
            .iter()
            .find_map(|operation| match operation {
                SupervisionOperationV1::Arm {
                    owned,
                    request,
                    response: Some(_),
                    ..
                } if matches!(request.subject, ArmSubjectV1::Worker)
                    && request.worker_id == worker.worker_id
                    && request.attempt_id == worker.attempt.attempt_id =>
                {
                    Some(owned.handle.clone())
                }
                _ => None,
            })
            .ok_or(RelayOwnerError::Scope)?;
        let source = Arc::new(Source {
            scope: context.scope().clone(),
            job_id: job.operation_id(),
            head_revision: job.head().revision(),
            head_hash: job.head().hash().into(),
            worker,
            handle,
            deadline,
            execution,
            client: context.client().clone(),
            gateway_trust: context.gateway_trust().clone(),
        });
        let key = (source.scope.tenant_id().to_owned(), source.job_id);
        let entry = {
            let mut entries = self
                .entries
                .lock()
                .map_err(|_| RelayOwnerError::Unavailable)?;
            if let Some(previous) = entries.get(&key) {
                if super::super::journal::canonical(&previous.source.scope).ok()
                    != super::super::journal::canonical(&source.scope).ok()
                    || endpoint(&previous.source.worker) != endpoint(&source.worker)
                    || previous.source.deadline.arm.operation_id != source.deadline.arm.operation_id
                {
                    return Err(RelayOwnerError::Scope);
                }
                return Ok(());
            }
            if entries.len() >= 4096
                || entries
                    .iter()
                    .filter(|((tenant, _), entry)| {
                        tenant == &key.0
                            && !entry.no_channel.load(Ordering::SeqCst)
                            && entry.view.lock().map_or(true, |view| {
                                view.as_ref()
                                    .is_none_or(|fact| fact.phase != AttachmentPhase::Closed)
                            })
                    })
                    .count()
                    >= 100
            {
                return Err(RelayOwnerError::Capacity);
            }
            let (stop, _) = watch::channel(terminal_recovery);
            let entry = Arc::new(Entry {
                source: source.clone(),
                view: Mutex::new(None),
                error: Mutex::new(None),
                stop,
                task: AsyncMutex::new(None),
                retained: AsyncMutex::new(None),
                // Until the durable prefix is read, an earlier exec remains
                // possible. Only a checked absence of a historical claim may
                // enable this process's pre-handoff join path.
                handoff_started: AtomicBool::new(true),
                no_channel: AtomicBool::new(false),
            });
            // Install the retained handle before publishing the entry. Shutdown
            // and terminal observers must never see a spawned but unowned task.
            let mut handle = entry
                .task
                .try_lock()
                .map_err(|_| RelayOwnerError::Unknown)?;
            // Lazily pin before the first actual task/resource admission. A
            // failed/unused Service setup must not acquire an immortal lease.
            // Unknown results retain this pin until explicit complete joins
            // and exact retirement; dropping Service cannot discard them.
            *self
                .retention
                .lock()
                .map_err(|_| RelayOwnerError::Unavailable)? = Some(self.clone());
            let registry = self.clone();
            let accepted = entry.clone();
            *handle = Some(tokio::spawn(async move {
                registry
                    .run_entry(accepted, source, terminal_recovery)
                    .await;
            }));
            drop(handle);
            entries.insert(key, entry.clone());
            entry
        };
        let _ = entry;
        Ok(())
    }
    async fn run_entry(
        self: Arc<Self>,
        entry: Arc<Entry>,
        source: Arc<Source>,
        terminal_recovery: bool,
    ) {
        let result = self
            .prepare_entry(&entry, source.clone(), terminal_recovery)
            .await;
        match result {
            Ok(Some(retained)) => {
                if let Ok(mut view) = entry.view.lock() {
                    *view = Some(retained.fact.clone());
                }
                if let Ok(mut error) = entry.error.lock() {
                    *error = retained.fact.error;
                }
                *entry.retained.lock().await = Some(retained);
            }
            Ok(None) => entry.no_channel.store(true, Ordering::SeqCst),
            Err(error) => {
                if let Ok(mut status) = entry.error.lock() {
                    *status = Some(error);
                }
            }
        }
    }
}
fn endpoint(worker: &WorkerV1) -> RelayEndpointV1 {
    RelayEndpointV1 {
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        generation: 1,
    }
}
fn transition_phase(kind: &RelayChannelTransitionKindV1) -> AttachmentPhase {
    match kind {
        RelayChannelTransitionKindV1::Ready => AttachmentPhase::Admitted,
        RelayChannelTransitionKindV1::CloseExactWorkerTerminal { .. } => AttachmentPhase::Closed,
        _ => AttachmentPhase::Unknown,
    }
}
fn status_error(error: RelayOwnerError) -> RelayStatusErrorV1 {
    match error {
        RelayOwnerError::Unavailable => RelayStatusErrorV1::Unavailable,
        RelayOwnerError::InvalidMaterial => RelayStatusErrorV1::InvalidMaterial,
        RelayOwnerError::Scope => RelayStatusErrorV1::Scope,
        RelayOwnerError::Expired => RelayStatusErrorV1::Expired,
        RelayOwnerError::Clock => RelayStatusErrorV1::Clock,
        RelayOwnerError::Cancelled => RelayStatusErrorV1::Cancelled,
        RelayOwnerError::Capacity => RelayStatusErrorV1::Capacity,
        RelayOwnerError::Unknown => RelayStatusErrorV1::Unknown,
    }
}

/// Concrete authority retained in control. Its constructor is only the fresh
/// verified broker path above, never deserialization or a caller predicate.
pub(crate) struct DispatchAuthority {
    source: Arc<Source>,
    tenant: Arc<TenantState>,
    io: Arc<TenantRelayIo>,
    binding: Arc<RetainedWorkerBinding>,
    current: CurrentRelayAuthority,
    owned: OwnedContainerSnapshot,
    _role: Arc<RoleLease>,
    registry: std::sync::Weak<RelayRegistry>,
    entry: Arc<Entry>,
}
impl DispatchAuthority {
    pub(crate) fn admission(&self) -> &RelayChannelAdmissionV1 {
        self.current.admission()
    }
    pub(crate) fn scope(&self) -> &JournalScopeV1 {
        &self.source.scope
    }
    pub(crate) fn binding(&self) -> Arc<RetainedWorkerBinding> {
        self.binding.clone()
    }
    pub(crate) fn gateway_trust(&self) -> &crate::tls::ClientTrust {
        &self.source.gateway_trust
    }
    pub(crate) fn literals(&self) -> Result<Vec<String>> {
        self.tenant
            .literals
            .lock()
            .map(|values| values.clone())
            .map_err(|_| RelayOwnerError::Unavailable)
    }
    pub(crate) async fn wait_expiration(&self) -> RelayOwnerError {
        self.source.deadline.wait_expiration().await
    }
    /// The concrete Core SDK revalidates the exact claim/binding. This method
    /// is also called after the runtime's final inspection, before its spawn.
    pub(crate) async fn current_binding(&self) -> Result<RelayChannelSnapshotV1> {
        self.check_deadline()?;
        self.io
            .check_binding(self.binding.clone())
            .await
            .map_err(|_| RelayOwnerError::InvalidMaterial)?;
        let snapshot = self
            .source
            .client
            .current_relay_channel(&self.current)
            .await
            .map_err(|_| RelayOwnerError::Unavailable)?;
        if snapshot.tenant_id != self.source.scope.tenant_id()
            || !self.source.deadline.endpoint_matches(&snapshot.endpoint)
            || snapshot.control_namespace_uuid
                != self.source.scope.control_namespace_uuid().to_string()
            || snapshot.endpoint != *self.binding.endpoint()
            || snapshot.credential_id != self.binding.credential_id()
            || snapshot.binding_revision != self.binding.revision()
        {
            return Err(RelayOwnerError::Scope);
        }
        self.check_deadline()?;
        Ok(snapshot)
    }
    pub(crate) async fn check(&self) -> Result<()> {
        self.current_binding().await?;
        let execution = self
            .source
            .execution
            .as_ref()
            .ok_or(RelayOwnerError::Scope)?;
        execution
            .runtime
            .revalidate_relay_channel(&self.owned, &execution.spec)
            .await
            .map_err(|_| RelayOwnerError::Scope)?;
        self.check_deadline()
    }
    pub(crate) fn check_deadline(&self) -> Result<()> {
        if *self.entry.stop.borrow()
            || self
                .registry
                .upgrade()
                .is_none_or(|registry| registry.stopping.load(Ordering::SeqCst))
        {
            return Err(RelayOwnerError::Cancelled);
        }
        self.source.deadline.check()
    }
    pub(crate) async fn ready(&self) -> Result<()> {
        let registry = self
            .registry
            .upgrade()
            .ok_or(RelayOwnerError::Unavailable)?;
        let mut fact = self
            .entry
            .view
            .lock()
            .map_err(|_| RelayOwnerError::Unavailable)?
            .clone()
            .ok_or(RelayOwnerError::Unknown)?;
        registry
            .transition(
                &self.entry,
                &self.tenant,
                &self.io,
                &self.source,
                &mut fact,
                RelayChannelTransitionKindV1::Ready,
            )
            .await
    }
}

/// Non-Clone/non-Deserialize composition of the fresh SDK capability and the
/// already verified broker source. Only run_handoff constructs this value.
pub(crate) struct VerifiedRelayExecAdmission {
    fresh: crate::runner::relay_admission::FreshRelayExecClaim,
    owned: OwnedContainerSnapshot,
    spec: ContainerSpec,
    authority: Arc<DispatchAuthority>,
}
impl VerifiedRelayExecAdmission {
    pub(crate) fn into_parts(
        self,
    ) -> (
        crate::runner::relay_admission::FreshRelayExecClaim,
        OwnedContainerSnapshot,
        ContainerSpec,
        Arc<DispatchAuthority>,
    ) {
        (self.fresh, self.owned, self.spec, self.authority)
    }
}

#[cfg(test)]
mod retention_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[tokio::test]
    async fn known_secret_in_fixed_relay_parent_is_refused_before_directory_creation() {
        use sha2::{Digest, Sha256};
        let scope = JournalScopeV1::new(
            Uuid::from_u128(1),
            "tenant-a",
            "http://127.0.0.1:9",
            Uuid::from_u128(2),
            "fixture",
        )
        .unwrap();
        let shared = JournalScopeV1::new(
            Uuid::from_u128(1),
            "tenant-a",
            "http://127.0.0.1:9",
            Uuid::from_u128(2),
            "relay_shared",
        )
        .unwrap();
        let key = hex::encode(Sha256::digest(
            crate::broker::journal::canonical(&shared).unwrap(),
        ));
        for literal in ["relay".to_string(), format!("relay/{key}")] {
            let temporary = tempfile::Builder::new()
                .permissions(std::fs::Permissions::from_mode(0o700))
                .tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap())
                .unwrap();
            let root = Directory::open_existing(temporary.path()).unwrap();
            let registry = RelayRegistry::new(
                root.clone(),
                Arc::new(RoleLease::acquire(&root).unwrap()),
                BTreeMap::new(),
                vec![literal],
            );
            let tenant = registry.tenant(&scope).unwrap();
            assert!(registry.tenant_io(&tenant).await.is_err());
            assert!(
                root.persistent_child("relay", false).unwrap().is_none(),
                "secret-bearing parent was created before validation"
            );
        }
    }

    #[tokio::test]
    async fn concurrent_tenant_initializers_share_one_locked_outbox_without_poisoning() {
        let temporary = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap())
            .unwrap();
        let root = Directory::open_existing(temporary.path()).unwrap();
        let registry = RelayRegistry::new(
            root.clone(),
            Arc::new(RoleLease::acquire(&root).unwrap()),
            BTreeMap::new(),
            vec!["software-private-proof-not-persisted".into()],
        );
        let scope = JournalScopeV1::new(
            Uuid::from_u128(1),
            "tenant-a",
            "http://127.0.0.1:9",
            Uuid::from_u128(2),
            "fixture",
        )
        .unwrap();
        let tenant = registry.tenant(&scope).unwrap();
        let gate = tenant.init.lock().await;
        let first = registry.tenant_io(&tenant);
        let second = registry.tenant_io(&tenant);
        tokio::pin!(first, second);
        // Both callers have observed an empty cache and are queued at the real
        // initializer mutex. No sleep or assumed scheduler race is required.
        assert!(futures_util::poll!(&mut first).is_pending());
        assert!(futures_util::poll!(&mut second).is_pending());
        drop(gate);
        let (first, second) = tokio::join!(first, second);
        let first = first.unwrap();
        let second = second.unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert!(!tenant.poisoned.load(Ordering::SeqCst));
        assert!(Arc::ptr_eq(
            &first,
            &registry.tenant_io(&tenant).await.unwrap()
        ));
        first.join_pending().await.unwrap();
        registry.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn cancelled_registry_io_remains_pinned_until_an_explicit_real_join() {
        let temporary = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap())
            .unwrap();
        let root = Directory::open_existing(temporary.path()).unwrap();
        let registry = RelayRegistry::new(
            root.clone(),
            Arc::new(RoleLease::acquire(&root).unwrap()),
            BTreeMap::new(),
            vec!["software-private-proof-not-persisted".into()],
        );
        let weak = Arc::downgrade(&registry);
        // This data/IO test mints no broker admission, fresh claim or exec.
        // Pin at the same point as the production task admission before IO.
        *registry.retention.lock().unwrap() = Some(registry.clone());
        let scope = JournalScopeV1::new(
            Uuid::from_u128(1),
            "tenant-a",
            "http://127.0.0.1:9",
            Uuid::from_u128(2),
            "fixture",
        )
        .unwrap();
        let tenant = registry.tenant(&scope).unwrap();
        let io = registry.tenant_io(&tenant).await.unwrap();
        let (started, ready) = tokio::sync::oneshot::channel();
        let (release, finish) = std::sync::mpsc::channel();
        io.pause_next(started, finish);
        let admitted = io.clone();
        let waiter = tokio::spawn(async move { admitted.channel(Uuid::from_u128(3)).await });
        ready.await.unwrap();
        waiter.abort();
        match waiter.await {
            Err(error) => assert!(error.is_cancelled()),
            Ok(_) => panic!("cancelled IO waiter completed normally"),
        }
        assert_eq!(tenant.slots.available_permits(), 3);
        assert_eq!(registry.global_io.available_permits(), 15);
        assert!(matches!(
            io.channel(Uuid::from_u128(4)).await,
            Err(crate::relay::channel::control::ControlError::NeedsReconciliation)
        ));
        drop(io);
        drop(tenant);
        drop(registry);
        let retained = weak
            .upgrade()
            .expect("private resource registry remains pinned");
        assert!(RoleLease::acquire(&root).is_err());
        assert!(
            tokio::time::timeout(Duration::from_millis(50), retained.shutdown())
                .await
                .is_err()
        );
        assert!(weak.upgrade().is_some());
        assert!(RoleLease::acquire(&root).is_err());
        release.send(()).unwrap();
        retained.shutdown().await.unwrap();
        assert_eq!(retained.global_io.available_permits(), 16);
        drop(retained);
        assert!(weak.upgrade().is_none());
        assert!(RoleLease::acquire(&root).is_ok());
    }
}
