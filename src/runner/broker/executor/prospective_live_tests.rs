//! Real writer/Gateway/SQLite/native IPC boundary tests. Created snapshots are
//! explicit synthetic DATA and the native fixture supplies SyntheticReady;
//! no ManagedRuntime, engine effect or provider is invoked or qualified.
use super::*;
use crate::{
    api::{GatewayState, TenantGateway, gateway_router},
    broker::{
        clock::NativeHostClock,
        discovery::load_endpoint,
        enforcer::{EnforcerClient, retirement::test_support::ProspectiveFixture},
        journal::retirement_live_fixture::{
            INVOCATION_DATA_PROGRAM, LiveFixture, live_fixture_with_created_helper_for_prospective,
            live_fixture_with_pending_arm_for_prospective,
        },
    },
    control_fs::Directory,
    redaction::Redactor,
    runtime::{Runtime, RuntimeKind},
};
use std::{os::unix::fs::PermissionsExt, path::Path, sync::atomic::AtomicUsize};
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle};

const RUNNER: &str = "runner-prospective-boundary-aaaaaaaaaaaaaaaaaaaaaaaa";
const VIEWER: &str = "viewer-prospective-boundary-bbbbbbbbbbbbbbbbbbbbbbbb";

struct BoundaryFixture {
    roots: Option<(tempfile::TempDir, tempfile::TempDir)>,
    store: Arc<crate::audit::Store>,
    journal: crate::broker::journal::Journal,
    intent: crate::broker::intent::ExecutionIntentV1,
    registration: RegistrationV1,
}
impl From<LiveFixture> for BoundaryFixture {
    fn from(value: LiveFixture) -> Self {
        Self {
            roots: Some((value._root, value._store_root)),
            store: Arc::new(value.store),
            journal: value.journal,
            intent: value.intent,
            registration: value.registration,
        }
    }
}
impl Drop for BoundaryFixture {
    fn drop(&mut self) {
        // Writer Drop is deliberately nonblocking. Preserve these private
        // roots rather than deleting a namespace before its native return is
        // independently observed; the test does not invent a writer join.
        if let Some((journal, store)) = self.roots.take() {
            let _journal = journal.keep();
            let _store = store.keep();
        }
    }
}

#[derive(Default)]
struct Posts {
    arm: AtomicUsize,
    claim: AtomicUsize,
}
struct Gateway {
    stop: oneshot::Sender<()>,
    task: JoinHandle<std::io::Result<()>>,
}
impl Gateway {
    async fn start(
        listener: TcpListener,
        fixture: &BoundaryFixture,
        posts: Arc<Posts>,
    ) -> Result<Self> {
        // The same real database is retained; no response DTO or fake receipt
        // is substituted for the authenticated HTTP endpoint.
        let state = GatewayState::for_tenants(
            vec![TenantGateway {
                tenant_id: fixture.journal.scope().tenant_id().into(),
                store: Arc::clone(&fixture.store),
                runner_token: RUNNER.into(),
                viewer_token: VIEWER.into(),
            }],
            crate::policy::Policy::default(),
            None,
            Redactor::new(vec![]),
        )?;
        let app = gateway_router(state).layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let posts = Arc::clone(&posts);
                async move {
                    if request.method() == axum::http::Method::POST {
                        if request.uri().path().ends_with("/arms") {
                            posts.arm.fetch_add(1, Ordering::SeqCst);
                        }
                        if request.uri().path().ends_with("/armed-start-claims")
                            || request.uri().path().ends_with("/helper-start-claims")
                        {
                            posts.claim.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                    next.run(request).await
                }
            },
        ));
        let (stop, receive) = oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = receive.await;
                })
                .await
        });
        Ok(Self { stop, task })
    }
    async fn finish(self) -> Result<()> {
        let _ = self.stop.send(());
        tokio::time::timeout(Duration::from_secs(5), self.task).await???;
        Ok(())
    }
}

struct LegacyFixture {
    files: tempfile::TempDir,
    client: EnforcerClient,
    task: JoinHandle<crate::broker::enforcer::EnforcerResult<()>>,
}
impl LegacyFixture {
    async fn start() -> Result<Self> {
        let files = tempfile::Builder::new()
            .prefix("d7-v1-")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(std::fs::canonicalize("/tmp")?)?;
        let root = Directory::open_existing(files.path())?;
        let auth = root.persistent_create("auth", &[0x42; 32])?;
        auth.persistent_sync()?;
        root.sync()?;
        let serving = root.clone();
        let task = tokio::spawn(async move {
            crate::broker::enforcer::run_internal_enforcer(&serving, 1).await
        });
        let expires = std::time::Instant::now() + Duration::from_secs(5);
        while load_endpoint(&root)?.is_none() {
            ensure!(
                std::time::Instant::now() < expires,
                "legacy fixture endpoint unavailable"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // A live actual V1 endpoint exists. This deliberately missing path
        // prevents this connection helper from starting any subprocess.
        let client =
            EnforcerClient::connect_or_spawn(&root, Path::new("/not-executed/prospective-test"), 1)
                .await?;
        Ok(Self {
            files,
            client,
            task,
        })
    }
    async fn finish(self) -> Result<()> {
        let result = async {
            self.client.shutdown_if_idle().await?;
            tokio::time::timeout(Duration::from_secs(5), self.task).await???;
            Ok::<_, anyhow::Error>(())
        }
        .await;
        if result.is_err() {
            let _retained = self.files.keep();
        }
        result
    }
}

async fn context(fixture: &BoundaryFixture, origin: &str) -> Result<AuthenticatedBatchContext> {
    let runtime = Runtime::with_program(RuntimeKind::Docker, Path::new(INVOCATION_DATA_PROGRAM))?;
    AuthenticatedBatchContext::for_executor_test(
        AuditClient::new(origin, RUNNER)?,
        fixture.journal.scope().clone(),
        Directory::open_existing(&fixture.intent.initial_options().run_options().config_dir)?,
        runtime,
    )
    .await
}

#[derive(Clone, Copy)]
enum Refusal {
    Legacy,
    Capacity,
    Cancelled,
    WriterUnknown,
}

async fn refused_before_arm(scenario: Refusal) -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let origin = format!("http://{}", listener.local_addr()?);
    let fixture = BoundaryFixture::from(live_fixture_with_created_helper_for_prospective(&origin));
    let posts = Arc::new(Posts::default());
    let gateway = Gateway::start(listener, &fixture, Arc::clone(&posts)).await?;
    let context = context(&fixture, &origin).await?;
    let mut native = None;
    let mut legacy = None;
    let enforcer = if matches!(scenario, Refusal::Legacy) {
        let value = LegacyFixture::start().await?;
        let enforcer = ExecutionEnforcerV1::Legacy(value.client.clone());
        legacy = Some(value);
        enforcer
    } else {
        let mut value =
            ProspectiveFixture::start(fixture.journal.scope().installation_id(), 1).await;
        let full = value.client().await;
        if matches!(scenario, Refusal::Capacity) {
            let target = fixture.registration.prospective()?;
            value.install(&target);
            full.reserve_prospective(target, context.cancel_generation().clone(), false)
                .await?;
        }
        native = Some(value);
        ExecutionEnforcerV1::Full3(full)
    };
    let result = async {
        let writer = WriterClient::start(fixture.journal.clone(), 16, Duration::from_secs(10))?;
        let executor =
            BatchExecutor::new(writer.clone(), enforcer, Arc::new(NativeHostClock::new()?));
        let execution = Execution::open(&executor, fixture.intent.operation_id(), &context).await?;
        let (before, draft) = execution.load().await?;
        ensure!(
            !draft
                .operations
                .iter()
                .any(|op| matches!(op, Operation::Arm { .. })),
            "Created fixture already has an Arm"
        );
        let helper = draft
            .helpers
            .iter()
            .find(|helper| helper.owned().is_some())
            .context("Created helper missing")?;
        if matches!(scenario, Refusal::Cancelled) {
            context.cancel_generation().invalidate();
        }
        let target = fixture.registration.prospective()?;
        let mut barrier = if matches!(scenario, Refusal::WriterUnknown) {
            Some(writer.advance_reply_barrier_for_test(true).await?)
        } else { None };
        let attempt = execution.arm_and_claim_bound(
                target.authority(),
                helper.owned().unwrap().clone(),
                Some((helper.policy(), helper.permission())),
            );
        tokio::pin!(attempt);
        if let Some(held) = barrier.as_mut() {
            tokio::select! {
                _ = &mut attempt => return Err(anyhow!("writer failed before reaching its real publication barrier")),
                entered = tokio::time::timeout(Duration::from_secs(5), held.entered()) => {
                    entered.context("writer publication barrier was not reached")?;
                },
            }
        }
        drop(barrier);
        let outcome = attempt.await;
        let error = outcome
            .err()
            .context("unprotected fresh Arm was accepted")?;
        if matches!(scenario, Refusal::Legacy) {
            ensure!(
                format!("{error:#}").contains("legacy enforcer has no native pre-ACK reservation"),
                "legacy refusal was not explicit"
            );
        }
        ensure!(
            posts.arm.load(Ordering::SeqCst) == 0 && posts.claim.load(Ordering::SeqCst) == 0,
            "refusal occurred after HTTP Arm/claim"
        );
        let strict = execution.load().await;
        if matches!(scenario, Refusal::Capacity | Refusal::WriterUnknown) {
            if matches!(scenario, Refusal::WriterUnknown) {
                ensure!(executor.enforcer.full3().unwrap().require_new_prearm_identity().is_err(),
                    "unknown pending publication left new identities enabled");
                ensure!(executor.enforcer.status().await?.slot_count() == 0,
                    "native owner was allocated before the failed pending publication");
            }
            let known = match strict {
                Ok((head, draft)) => {
                    let pending: Vec<_> = draft.operations.iter().filter_map(|op| match op {
                        Operation::Arm {request,response:None,..} => Some(request), _=>None,
                    }).collect();
                    ensure!(pending.len() == 1, "refusal lost or duplicated the exact pending Arm");
                    Some((head.head().clone(), pending[0].operation_id.clone(), pending[0].deadline_ns_decimal.clone()))
                }
                Err(_) if matches!(scenario, Refusal::WriterUnknown) => None,
                Err(error) => return Err(error),
            };
            let retry = execution.arm_and_claim_bound(target.authority(), helper.owned().unwrap().clone(),
                Some((helper.policy(), helper.permission()))).await;
            ensure!(retry.is_err(), "unowned pending retry acquired a new permission");
            ensure!(posts.arm.load(Ordering::SeqCst) == 0 && posts.claim.load(Ordering::SeqCst) == 0,
                "unowned/unknown pending retry issued an Arm or claim RPC");
            match (known, execution.load().await) {
                (Some((head, arm_id, deadline)), Ok((after, draft))) => {
                    let pending: Vec<_> = draft.operations.iter().filter_map(|op| match op {
                        Operation::Arm {request,..} => Some(request), _=>None,
                    }).collect();
                    ensure!(after.head() == &head && pending.len() == 1
                        && pending[0].operation_id == arm_id && pending[0].deadline_ns_decimal == deadline,
                        "refused retry changed pending identity, deadline or durable head");
                }
                (None, Err(_)) => {}, // Strict publication remains UNKNOWN.
                _ => return Err(anyhow!("unknown pending publication was silently reclassified")),
            }
        } else {
            let (after, draft) = strict?;
            ensure!(before.head() == after.head(), "pre-ACK refusal changed the durable head");
            ensure!(!draft.operations.iter().any(|op| matches!(op, Operation::Arm {..})), "refusal persisted a new Arm");
        }
        Ok(())
    }
    .await;
    if let Some(value) = native {
        value.finish().await;
    }
    if let Some(value) = legacy {
        value.finish().await?;
    }
    gateway.finish().await?;
    result
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fresh_legacy_refuses_explicitly_before_arm_rpc() {
    Box::pin(refused_before_arm(Refusal::Legacy)).await.unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn installed_prospective_capacity_refuses_before_arm_rpc() {
    Box::pin(refused_before_arm(Refusal::Capacity)).await.unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_generation_refuses_before_arm_rpc() {
    Box::pin(refused_before_arm(Refusal::Cancelled)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_pending_writer_ack_never_generates_another_arm_or_rpc() {
    Box::pin(refused_before_arm(Refusal::WriterUnknown)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lost_arm_ack_replays_same_owner_and_deadline_without_claim() {
    let result = Box::pin(lost_ack()).await;
    result.unwrap();
}
async fn lost_ack() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let origin = format!("http://{}", listener.local_addr()?);
    let fixture = BoundaryFixture::from(live_fixture_with_pending_arm_for_prospective(&origin));
    let posts = Arc::new(Posts::default());
    let gateway = Gateway::start(listener, &fixture, Arc::clone(&posts)).await?;
    let context = context(&fixture, &origin).await?;
    let target = fixture.registration.prospective()?;
    let mut native = ProspectiveFixture::start(fixture.journal.scope().installation_id(), 1).await;
    native.install(&target);
    let full = native.client().await;
    full.reserve_prospective(target.clone(), context.cancel_generation().clone(), false)
        .await?;
    let _issued = full
        .begin_prospective_arm_exchange(&target, context.cancel_generation())
        .await?;
    let result = async {
        let writer = WriterClient::start(fixture.journal.clone(), 16, Duration::from_secs(10))?;
        let executor = BatchExecutor::new(
            writer.clone(),
            ExecutionEnforcerV1::Full3(full.clone()),
            Arc::new(NativeHostClock::new()?),
        );
        let execution = Execution::open(&executor, fixture.intent.operation_id(), &context).await?;
        let (_, draft) = execution.load().await?;
        let helper = draft
            .helpers
            .iter()
            .find(|helper| helper.owned().is_some())
            .context("pending helper missing")?;
        ensure!(
            draft
                .operations
                .iter()
                .any(|op| matches!(op, Operation::Arm { response: None, .. })),
            "lost ACK pending missing"
        );
        let before = serde_json::to_vec(&fixture.store.worker(&target.arm().worker_id)?)?;
        let outcome = execution
            .arm_and_claim_bound(
                target.authority(),
                helper.owned().unwrap().clone(),
                Some((helper.policy(), helper.permission())),
            )
            .await?;
        ensure!(
            matches!(outcome, PrearmedStartDirective::ObserveOnly),
            "pending Arm replay created a fresh permission"
        );
        ensure!(
            posts.arm.load(Ordering::SeqCst) == 1 && posts.claim.load(Ordering::SeqCst) == 0,
            "lost ACK replay emitted an extra claim or Arm"
        );
        ensure!(
            before == serde_json::to_vec(&fixture.store.worker(&target.arm().worker_id)?)?,
            "idempotent Arm replay changed worker state"
        );
        let (_, draft) = execution.load().await?;
        let arms: Vec<_> = draft
            .operations
            .iter()
            .filter_map(|op| match op {
                Operation::Arm {
                    request,
                    response: Some(response),
                    ..
                } => Some((request, response)),
                _ => None,
            })
            .collect();
        ensure!(
            arms.len() == 1
                && arms[0].0.operation_id == target.arm_id()
                && arms[0].0.deadline_ns_decimal == target.arm().deadline_ns_decimal
                && arms[0].1.receipt.input_hash == fixture.registration.receipt().input_hash,
            "lost ACK changed identity, input or original deadline"
        );
        full.reserve_prospective(target, context.cancel_generation().clone(), true)
            .await?;
        Ok(())
    }
    .await;
    native.finish().await;
    gateway.finish().await?;
    result
}
