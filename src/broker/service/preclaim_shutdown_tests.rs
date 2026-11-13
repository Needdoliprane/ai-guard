//! The real AbandonBeforeClaim connection must not leave an unjoined audit
//! producer after broker shutdown releases its physical RoleLease. The native
//! original is SyntheticReady; no business engine or model is executed.
use super::*;
use crate::broker::journal::retirement_live_fixture::live_fixture_with_gateway_and_program;
use crate::broker::local_ipc::IpcError;
use tokio::sync::Notify;

struct ReleaseBarrier(watch::Sender<bool>);
impl Drop for ReleaseBarrier {
    fn drop(&mut self) {
        let _ = self.0.send(true);
    }
}

struct SignalProbe {
    entered: Notify,
    posts: AtomicUsize,
    commits_without_role_lease: AtomicUsize,
    broker_root: Directory,
    http: Mutex<Vec<HttpTrace>>,
}

// Closed setup diagnostics only: no URI, headers, body or credential is saved.
struct HttpTrace {
    route: &'static str,
    original_control_prefix: bool,
    effective_control_prefix: bool,
    status: Option<u16>,
}
impl SignalProbe {
    fn begin_http(&self, trace: HttpTrace) -> Option<usize> {
        let mut traces = self.http.lock().ok()?;
        if traces.len() >= 16 {
            return None;
        }
        let index = traces.len();
        traces.push(trace);
        Some(index)
    }
    fn end_http(&self, index: Option<usize>, status: u16) {
        if let Some(index) = index
            && let Ok(mut traces) = self.http.lock()
            && let Some(trace) = traces.get_mut(index)
        {
            trace.status = Some(status);
        }
    }
}

struct BarrierGateway {
    release: ReleaseBarrier,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<std::io::Result<()>>>,
}
impl BarrierGateway {
    async fn start(
        listener: TcpListener,
        store: Arc<Store>,
        tenant: &str,
        probe: Arc<SignalProbe>,
    ) -> Result<Self> {
        let state = GatewayState::for_tenants(
            vec![TenantGateway {
                tenant_id: tenant.into(),
                store,
                runner_token: RUNNER.into(),
                viewer_token: VIEWER.into(),
            }],
            crate::policy::Policy::default(),
            None,
            Redactor::new(Vec::new()),
        )?;
        let (release, receive) = watch::channel(false);
        let app = gateway_router(state).layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let probe = Arc::clone(&probe);
                let mut receive = receive.clone();
                async move {
                    let effective = request.uri().path();
                    let original = request
                        .extensions()
                        .get::<axum::extract::OriginalUri>()
                        .map_or(effective, |uri| uri.0.path());
                    // Axum's /control nest may have stripped the effective
                    // URI before this per-route middleware sees it. Both
                    // spellings denote only this fixture's exact control route.
                    let is_events = [original, effective].iter().any(|path| {
                        matches!(
                            *path,
                            "/control/api/v1/control/events" | "/api/v1/control/events"
                        )
                    });
                    let is_protocol = [original, effective].iter().any(|path| {
                        matches!(
                            *path,
                            "/control/api/v1/control/protocol" | "/api/v1/control/protocol"
                        )
                    });
                    let trace = probe.begin_http(HttpTrace {
                        route: if is_protocol {
                            "control_protocol"
                        } else if is_events {
                            "control_events"
                        } else {
                            "other"
                        },
                        original_control_prefix: original.starts_with("/control/"),
                        effective_control_prefix: effective.starts_with("/control/"),
                        status: None,
                    });
                    if request.method() != axum::http::Method::POST || !is_events {
                        let response = next.run(request).await;
                        probe.end_http(trace, response.status().as_u16());
                        return response;
                    }
                    let (parts, body) = request.into_parts();
                    let bytes = match axum::body::to_bytes(body, 128 * 1024).await {
                        Ok(bytes) => bytes,
                        Err(_) => {
                            return axum::response::IntoResponse::into_response(
                                axum::http::StatusCode::PAYLOAD_TOO_LARGE,
                            );
                        }
                    };
                    let signal = serde_json::from_slice::<Event>(&bytes)
                        .ok()
                        .is_some_and(|event| event.kind == SIGNAL);
                    if signal {
                        probe.posts.fetch_add(1, Ordering::SeqCst);
                        probe.entered.notify_one();
                        // Withhold before the actual router/SQLite commit. Drop
                        // of the test's release owner also opens this barrier.
                        while !*receive.borrow_and_update() {
                            if receive.changed().await.is_err() {
                                break;
                            }
                        }
                    }
                    let response = next
                        .run(axum::extract::Request::from_parts(
                            parts,
                            axum::body::Body::from(bytes),
                        ))
                        .await;
                    probe.end_http(trace, response.status().as_u16());
                    if signal && response.status().is_success() {
                        // Observe the real file lock, never a simulated owner
                        // flag. A successful acquire is dropped immediately.
                        if RoleLease::acquire_existing(&probe.broker_root).is_ok() {
                            probe
                                .commits_without_role_lease
                                .fetch_add(1, Ordering::SeqCst);
                        }
                    }
                    response
                }
            },
        ));
        let (stop, receive_stop) = oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = receive_stop.await;
                })
                .await
        });
        Ok(Self {
            release: ReleaseBarrier(release),
            stop: Some(stop),
            task: Some(task),
        })
    }
    fn release(&self) {
        let _ = self.release.0.send(true);
    }
    async fn finish(mut self) -> Result<()> {
        self.release();
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(mut task) = self.task.take() {
            match tokio::time::timeout(Duration::from_secs(5), &mut task).await {
                Ok(result) => result??,
                Err(_) => {
                    task.abort();
                    let _ = task.await;
                    return Err(anyhow!("preclaim shutdown gateway did not join"));
                }
            }
        }
        Ok(())
    }
}
impl Drop for BarrierGateway {
    fn drop(&mut self) {
        self.release();
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

struct Observed {
    shutdown_returned_while_signal_blocked: bool,
    shutdown_result_ok: bool,
    response_waiter_cancelled_while_signal_blocked: bool,
    role_lease_held_while_signal_blocked: bool,
    job_guard_held_while_signal_blocked: bool,
    signal_job_capture_present: bool,
    acknowledged_after_role_release: bool,
    posts: usize,
    commits_without_role_lease: usize,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preclaim_signal_audit_shutdown_retains_role_lease_until_real_child_completion() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let live = ProducerFixture::from(live_fixture_with_gateway_and_program(
        &origin,
        true,
        Path::new(INVOCATION_DATA_PROGRAM),
    ));
    let (native, full, slot) = OriginalUnusedFixture::register(
        live.journal.scope().installation_id(),
        live.registration.clone(),
    )
    .await
    .unwrap();
    let observed = observe_shutdown(&live, &full, &slot, listener).await;
    // All barriers, HTTP work and connection tasks are released/reaped inside
    // observe_shutdown before any regression assertion or native cleanup.
    native.finish().await;
    let observed = observed.unwrap();
    eprintln!(
        "preclaim_shutdown returned_early={} shutdown_result_ok={} response_waiter_cancelled={} role_lease_held={} job_guard_held={} signal_job_capture={} ack_after_role_release={} posts={} commits_without_role_lease={}",
        observed.shutdown_returned_while_signal_blocked,
        observed.shutdown_result_ok,
        observed.response_waiter_cancelled_while_signal_blocked,
        observed.role_lease_held_while_signal_blocked,
        observed.job_guard_held_while_signal_blocked,
        observed.signal_job_capture_present,
        observed.acknowledged_after_role_release,
        observed.posts,
        observed.commits_without_role_lease,
    );
    assert!(
        observed.signal_job_capture_present,
        "signal child was not reached"
    );
    assert!(
        observed.response_waiter_cancelled_while_signal_blocked,
        "shutdown did not cancel the actual IPC response waiter before signal release"
    );
    assert!(
        !observed.shutdown_returned_while_signal_blocked
            && observed.role_lease_held_while_signal_blocked,
        "shutdown released the physical broker RoleLease before the signal audit child completed"
    );
    assert!(
        !observed.acknowledged_after_role_release && observed.commits_without_role_lease == 0,
        "signal POST/ACK progressed after the physical broker RoleLease was released"
    );
    assert_eq!(
        observed.posts, 1,
        "the same signal must not be reposted during shutdown"
    );
    assert!(
        observed.shutdown_result_ok,
        "a completed audit signal with a cancelled response receiver must not poison shutdown"
    );
}

struct ConnectionCompletion {
    reply: Option<oneshot::Sender<bool>>,
    returned: bool,
}
impl Drop for ConnectionCompletion {
    fn drop(&mut self) {
        if let Some(reply) = self.reply.take() {
            let _ = reply.send(self.returned);
        }
    }
}

/// Private profile for this test only. The intention retains its existing
/// 600-second request; native Arm, HTTP and shutdown bounds are unchanged.
fn shutdown_registry(
    root: &Path,
    scope: &JournalScopeV1,
    intent: &ExecutionIntentV1,
    filesystem_roots: &Path,
) -> Result<Arc<Registry>> {
    let config = root.join("config");
    for name in ["sources", "exports"] {
        fs::create_dir(filesystem_roots.join(name))?;
        fs::set_permissions(
            filesystem_roots.join(name),
            fs::Permissions::from_mode(0o700),
        )?;
    }
    private_file(&config.join("runner.token"), RUNNER.as_bytes())?;
    let options = intent.initial_options().run_options();
    let path = config.join("registry.json");
    private_file(
        &path,
        &serde_json::to_vec(&json!({
            "version":1,"installation_id":scope.installation_id(),"root":root,"max_enforcer_slots":8,
            "profiles":[{"id":scope.profile_id(),"gateway":scope.gateway_origin(),"tenant_id":scope.tenant_id(),
                "control_namespace_uuid":scope.control_namespace_uuid(),"config_dir":config,
                "runner_token_file":config.join("runner.token"),"runtime":"docker","runtime_program":INVOCATION_DATA_PROGRAM,
                "runtime_endpoint":options.runtime_endpoint,"image":options.image,"policy_profile":options.profile,
                "network":options.network,"allow_rootful_runtime":false,"max_timeout_seconds":600,
                "allow_capture_output":false,"managed_clients":false,"manual_credential_file":options.secret_env_file,
                "source_roots":[filesystem_roots.join("sources")],"export_roots":[filesystem_roots.join("exports")]}]
        }))?,
    )?;
    Ok(Arc::new(Registry::load(&path)?))
}

async fn observe_shutdown(
    live: &ProducerFixture,
    full: &EnforcerFullClientV3,
    slot: &EnforcedSlot,
    listener: TcpListener,
) -> Result<Observed> {
    let scope = live.journal.scope().clone();
    let writer = WriterClient::start(live.journal.clone(), 16, Duration::from_secs(10))?;
    let operation = live.intent.operation_id();
    let arm = slot.full3_registration_binding()?.arm_id();
    writer
        .bind_full3_registration(
            operation,
            live.head.clone(),
            slot.full3_registration_binding()?,
        )
        .await?;
    let root = live._root.path().canonicalize()?;
    let registry = shutdown_registry(
        &root,
        &scope,
        &live.intent,
        &live._store_root.path().canonicalize()?,
    )?;
    let durable = writer
        .load(operation)
        .await?
        .context("preclaim shutdown fixture has no real accepted job")?;
    jobs::require_profile(&durable, registry.profile(scope.profile_id())?)
        .context("preclaim shutdown fixture intention/profile preflight")?;
    drop(durable);
    let material = Arc::new(Material::load(&registry)?);
    let jobs = Arc::new(ProfileJobs::new());
    jobs.writer
        .set(ProfileWriterCapabilities::from_full(writer.clone()))
        .map_err(|_| anyhow!("preclaim shutdown repeated writer factory"))?;
    let local = jobs.job(operation)?;
    local
        .preclaim
        .announce_registration(operation, arm, full.epoch(), 0)?;
    local
        .preclaim
        .mark_original_unclaimed(arm, full.epoch(), 0)?;
    let service = Arc::new(Service {
        full_owners: FullOwners::new(registry.data.max_enforcer_slots as usize)?,
        cold_owners: retained_audit::ColdOwners::new(registry.data.max_enforcer_slots as usize)?,
        registry,
        root: Directory::open_existing(&root)?,
        material,
        profiles: BTreeMap::from([(scope.profile_id().to_owned(), jobs)]),
        enforcer: Some(ServiceEnforcer::Native(ExecutionEnforcerV1::Full3(
            full.clone(),
        ))),
        stop_enforcer: None,
        blocking: Arc::new(Semaphore::new(2)),
        preparation: Arc::new(Semaphore::new(1)),
        execution: Arc::new(Semaphore::new(1)),
        stopping: Arc::new(AtomicBool::new(false)),
        fenced: Arc::new(AtomicBool::new(false)),
    });
    let broker_root = service.root.persistent_child("broker", true)?.unwrap();
    let lease = RoleLease::acquire(&broker_root)?;
    let probe = Arc::new(SignalProbe {
        entered: Notify::new(),
        posts: AtomicUsize::new(0),
        commits_without_role_lease: AtomicUsize::new(0),
        broker_root: broker_root.clone(),
        http: Mutex::new(Vec::new()),
    });
    let gateway = BarrierGateway::start(
        listener,
        Arc::clone(&live.store),
        scope.tenant_id(),
        Arc::clone(&probe),
    )
    .await?;
    let signal = AbandonBeforeClaimV1 {
        version: 1,
        operation_id: operation,
        arm_id: arm,
        command_id: Uuid::new_v4(),
        expected_epoch: full.epoch(),
        generation: 0,
    };
    let input = envelope(scope.profile_id(), RUNNER, &signal)?;
    let baseline_job_captures = Arc::strong_count(&local);
    let (mut peer, server) = UnixStream::pair()?;
    let mut requests = JoinSet::new();
    let (connection_done, mut connection_result) = oneshot::channel();
    let connection_service = Arc::clone(&service);
    requests.spawn(async move {
        let mut completion = ConnectionCompletion {
            reply: Some(connection_done),
            returned: false,
        };
        connection_service.connection(server).await;
        completion.returned = true;
    });
    let entered: Result<()> = async {
        write_frame(&mut peer, &input, FRAME_BYTES, WRITE_TIMEOUT).await?;
        tokio::select! {
            reached = tokio::time::timeout(Duration::from_secs(10), probe.entered.notified()) => {
                reached.context("signal never reached the real HTTP barrier")?;
                Ok(())
            }
            reply = read_frame::<ReplyEnvelopeV1>(&mut peer, FRAME_BYTES, Duration::from_secs(10)) => {
                match reply {
                    Ok(ReplyEnvelopeV1 { reply: BrokerReplyV1::Error { code }, .. }) => {
                        Err(anyhow!("real IPC dispatch refused the signal before the HTTP barrier: {code:?}"))
                    }
                    Ok(_) => Err(anyhow!("real IPC dispatch replied before the signal HTTP barrier")),
                    Err(error) => Err(anyhow!("real IPC signal response failed before the HTTP barrier: {error}")),
                }
            }
        }
    }
    .await;
    drop(peer);
    // Actual Unix connection/dispatch is in the same JoinSet cancelled by the
    // production shutdown helper. No synthetic running guard is introduced.
    let producer = tokio::spawn(async {});
    let (presence_shutdown, _) = watch::channel(false);
    let mut presence_tasks = JoinSet::new();
    let mut shutdown = Box::pin(async {
        let result = service
            .shutdown(
                producer,
                &presence_shutdown,
                &mut presence_tasks,
                &mut requests,
            )
            .await;
        // This is the physical lease-release order of serve_selected_owned;
        // either Ok or Err is relevant, never an assumed Drained result.
        drop(lease);
        result
    });
    let before_release = tokio::time::timeout(Duration::from_secs(6), shutdown.as_mut()).await;
    let returned_early = before_release.is_ok();
    // A false result is sent only when the actual connection future was
    // destroyed before returning. The held HTTP response prevents a normal
    // connection completion while its signal's response receiver is present.
    let response_waiter_cancelled = matches!(connection_result.try_recv(), Ok(false));
    let role_held = matches!(
        RoleLease::acquire_existing(&broker_root),
        Err(IpcError::Conflict)
    );
    let guard_held = matches!(local.enter(), Err(BrokerErrorCodeV1::Busy));
    let capture_present = Arc::strong_count(&local) > baseline_job_captures;
    let before_ack = writer.load(operation).await;
    // Never assert or propagate an observation error while the HTTP child is
    // blocked. Release and reap first, including the baseline RED path.
    gateway.release();
    let shutdown_result = match before_release {
        Ok(result) => Some(result),
        Err(_) => tokio::time::timeout(Duration::from_secs(20), shutdown.as_mut())
            .await
            .ok(),
    };
    let shutdown_joined = shutdown_result.is_some();
    let shutdown_result_ok = shutdown_result.is_some_and(|result| result.is_ok());
    drop(shutdown);
    requests.abort_all();
    while requests.join_next().await.is_some() {}
    // The production child has no exposed JoinHandle on the failing source.
    // Its actual captured Arc<Job> must disappear after real ACK/confirmation;
    // this waits for that body to return rather than dropping a waiter future.
    // Include the existing 47-second HTTP retry budget plus the writer's
    // ten-second request bound when a cleanup transport itself fails.
    let child_finished = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            if Arc::strong_count(&local) <= baseline_job_captures {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .is_ok();
    let after_ack = writer.load(operation).await;
    let signal_confirmed = local.preclaim.arm_view(arm).audit_confirmed;
    let gateway_joined = gateway.finish().await;
    if let Ok(traces) = probe.http.lock() {
        for (index, trace) in traces.iter().enumerate() {
            eprintln!(
                "preclaim_shutdown setup_http index={index} route={} original_control_prefix={} effective_control_prefix={} status={:?}",
                trace.route,
                trace.original_control_prefix,
                trace.effective_control_prefix,
                trace.status,
            );
        }
    }
    // All cleanup above precedes these fallible checks and final assertions.
    entered?;
    ensure!(
        shutdown_joined && child_finished,
        "preclaim shutdown/child failed to finish after release"
    );
    gateway_joined?;
    let before = before_ack?.context("signal job absent before release")?;
    let after = after_ack?.context("signal job absent after release")?;
    let ack = |job: &AcceptedJobV1| {
        job.broker_checkpoint_v4().and_then(|cp| {
            cp.events()
                .iter()
                .find(|event| event.event_id() == signal.command_id)
                .and_then(|event| event.audit_seq())
        })
    };
    ensure!(
        ack(&before).is_none(),
        "signal was ACKed while its HTTP request was blocked"
    );
    ensure!(
        ack(&after).is_some() && signal_confirmed,
        "released real signal did not reach its durable ACK/confirmation"
    );
    ensure!(
        local.enter().is_ok(),
        "signal left an actual Job guard behind after completion"
    );
    Ok(Observed {
        shutdown_returned_while_signal_blocked: returned_early,
        shutdown_result_ok,
        response_waiter_cancelled_while_signal_blocked: response_waiter_cancelled,
        role_lease_held_while_signal_blocked: role_held,
        job_guard_held_while_signal_blocked: guard_held,
        signal_job_capture_present: capture_present,
        acknowledged_after_role_release: !role_held && ack(&after).is_some(),
        posts: probe.posts.load(Ordering::SeqCst),
        commits_without_role_lease: probe.commits_without_role_lease.load(Ordering::SeqCst),
    })
}
