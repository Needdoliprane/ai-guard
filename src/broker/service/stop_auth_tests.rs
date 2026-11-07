//! Fresh HTTP authentication against the real gateway before STOP metadata.
//! The delivery probe qualifies routing only; no runtime or IPC stop is claimed.
use super::*;
use crate::api::{GatewayState, TenantGateway, gateway_router};
use crate::runner::AuditClient;
use std::sync::atomic::AtomicUsize;

const A_RUNNER: &str = "runner-a-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const A_VIEWER: &str = "viewer-a-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const B_RUNNER: &str = "runner-b-cccccccccccccccccccccccccccccccc";
const B_VIEWER: &str = "viewer-b-dddddddddddddddddddddddddddddddd";
const TENANT: &str = "tenant-cancel-retry-test";

#[path = "recovery_inventory_tests.rs"]
mod recovery_inventory_tests;

struct Gateway {
    _root: tempfile::TempDir,
    url: String,
    namespace: Uuid,
    requests: Arc<AtomicUsize>,
    identities: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Gateway {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Gateway {
    async fn active() -> Self {
        Self::with_schema(3, false).await
    }

    async fn with_schema(schema: u32, inactive: bool) -> Self {
        assert!(matches!(schema, 3 | 6 | 7));
        let root = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let store = Arc::new(
            Store::create_supervised_for_tenant(
                &root.path().join("a.sqlite"),
                TENANT,
                Redactor::new(vec![]),
                1000,
                &root.path().join("a-before-v3.sqlite"),
            )
            .unwrap(),
        );
        if schema >= 6 {
            store
                .migrate_identity_v4(&root.path().join("a-before-v4.sqlite"))
                .unwrap();
            store
                .migrate_human_access_v5(&root.path().join("a-before-v5.sqlite"))
                .unwrap();
            store
                .migrate_workload_v6(&root.path().join("a-before-v6.sqlite"))
                .unwrap();
        }
        if schema == 7 {
            store
                .migrate_human_brokers_v7(&root.path().join("a-before-v7.sqlite"))
                .unwrap();
        }
        let store = if inactive {
            drop(store);
            Arc::new(
                Store::open_existing_for_tenant(
                    &root.path().join("a.sqlite"),
                    TENANT,
                    Redactor::new(vec![]),
                    1000,
                )
                .unwrap(),
            )
        } else {
            store
        };
        let state = store.supervision_state().unwrap();
        assert_eq!(state.schema_version, schema);
        assert_eq!(state.active, !inactive);
        let namespace = Uuid::parse_str(state.control_namespace_uuid.as_deref().unwrap()).unwrap();
        let state = GatewayState::for_tenants(
            vec![TenantGateway {
                tenant_id: TENANT.into(),
                store,
                runner_token: A_RUNNER.into(),
                viewer_token: A_VIEWER.into(),
            }],
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )
        .unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let identities = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&requests);
        let observed_identity = Arc::clone(&identities);
        let app = gateway_router(state).layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let observed = Arc::clone(&observed);
                let observed_identity = Arc::clone(&observed_identity);
                async move {
                    observed.fetch_add(1, Ordering::SeqCst);
                    if request.uri().path() == "/control/api/v1/control/protocol" {
                        observed_identity.fetch_add(1, Ordering::SeqCst);
                    }
                    next.run(request).await
                }
            },
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            _root: root,
            url,
            namespace,
            requests,
            identities,
            task,
        }
    }

    async fn inactive(schema3: bool) -> Self {
        let root = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let path = root.path().join("a.sqlite");
        if schema3 {
            // The supported close/reopen path enters maintenance. No PRAGMA or
            // direct SQLite mutation manufactures a protocol state.
            drop(
                Store::create_supervised_for_tenant(
                    &path,
                    TENANT,
                    Redactor::new(vec![]),
                    1000,
                    &root.path().join("a-before-v3.sqlite"),
                )
                .unwrap(),
            );
        }
        let a =
            Arc::new(Store::open_for_tenant(&path, TENANT, Redactor::new(vec![]), 1000).unwrap());
        let state = a.supervision_state().unwrap();
        assert_eq!(state.schema_version, if schema3 { 3 } else { 2 });
        assert!(!state.active);
        let namespace = Uuid::parse_str(state.control_namespace_uuid.as_deref().unwrap()).unwrap();
        let b = Arc::new(
            Store::open_for_tenant(
                &root.path().join("b.sqlite"),
                "tenant-stop-auth-b",
                Redactor::new(vec![]),
                1000,
            )
            .unwrap(),
        );
        let tenant = |id: &str, store, runner: &str, viewer: &str| TenantGateway {
            tenant_id: id.into(),
            store,
            runner_token: runner.into(),
            viewer_token: viewer.into(),
        };
        let state = GatewayState::for_tenants(
            vec![
                tenant(TENANT, a, A_RUNNER, A_VIEWER),
                tenant("tenant-stop-auth-b", b, B_RUNNER, B_VIEWER),
            ],
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )
        .unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let identities = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&requests);
        let observed_identity = Arc::clone(&identities);
        let app = gateway_router(state).layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let observed = Arc::clone(&observed);
                let observed_identity = Arc::clone(&observed_identity);
                async move {
                    observed.fetch_add(1, Ordering::SeqCst);
                    if request.uri().path() == "/control/api/v1/control/protocol" {
                        observed_identity.fetch_add(1, Ordering::SeqCst);
                    }
                    next.run(request).await
                }
            },
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            _root: root,
            url,
            namespace,
            requests,
            identities,
            task,
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn workload_schema6_keeps_fresh_broker_auth_and_inactive_admission_guards() {
    for inactive in [false, true] {
        let gateway = Gateway::with_schema(6, inactive).await;
        let fixture = fresh_namespace_fixture(&gateway, gateway.namespace);
        let registry = &fixture.service.registry;
        assert!(
            VerifiedProfile::authenticate(registry, "cancel_retry_test", A_RUNNER, !inactive)
                .await
                .is_ok(),
            "schema6 must preserve authenticated metadata and compatible admission"
        );
        assert!(matches!(
            VerifiedProfile::authenticate(registry, "cancel_retry_test", A_VIEWER, false).await,
            Err(BrokerErrorCodeV1::Forbidden)
        ));
        if inactive {
            assert!(matches!(
                VerifiedProfile::authenticate(registry, "cancel_retry_test", A_RUNNER, true).await,
                Err(BrokerErrorCodeV1::IncompatibleControl)
            ));
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn human_broker_schema7_keeps_stop_identity_and_inactive_admission_guards() {
    for inactive in [false, true] {
        let gateway = Gateway::with_schema(7, inactive).await;
        let fixture = fresh_namespace_fixture(&gateway, gateway.namespace);
        let registry = &fixture.service.registry;
        let identity = AuditClient::new(&gateway.url, A_RUNNER)
            .unwrap()
            .control_supervision_identity()
            .await
            .unwrap();
        assert_eq!(identity.schema_version, 7);
        assert_eq!(identity.tenant_id, TENANT);
        assert_eq!(identity.role, "runner");
        assert_eq!(identity.active, !inactive);
        assert_eq!(
            identity.control_namespace_uuid.as_deref(),
            Some(gateway.namespace.to_string().as_str())
        );
        assert!(
            VerifiedProfile::authenticate(registry, "cancel_retry_test", A_RUNNER, false)
                .await
                .is_ok(),
            "schema7 retains authenticated STOP metadata in both lease states"
        );
        if inactive {
            assert!(matches!(
                VerifiedProfile::authenticate(registry, "cancel_retry_test", A_RUNNER, true).await,
                Err(BrokerErrorCodeV1::IncompatibleControl)
            ));
        } else {
            assert!(
                VerifiedProfile::authenticate(registry, "cancel_retry_test", A_RUNNER, true)
                    .await
                    .is_ok()
            );
        }
        refused_stop_identity(
            &gateway,
            gateway.namespace,
            A_VIEWER,
            BrokerErrorCodeV1::Forbidden,
        )
        .await;
        refused_stop_identity(
            &gateway,
            Uuid::new_v4(),
            A_RUNNER,
            BrokerErrorCodeV1::Forbidden,
        )
        .await;

        // A compatible schema cannot make a different tenant profile match.
        let mut config: Value = serde_json::from_slice(registry.source.bytes()).unwrap();
        config["profiles"][0]["tenant_id"] = json!("tenant-not-this-gateway");
        let path = fixture
            ._temp
            .path()
            .canonicalize()
            .unwrap()
            .join("wrong-tenant-registry.json");
        fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let wrong_tenant = Registry::load(&path).unwrap();
        assert!(matches!(
            VerifiedProfile::authenticate(&wrong_tenant, "cancel_retry_test", A_RUNNER, false)
                .await,
            Err(BrokerErrorCodeV1::Forbidden)
        ));
    }
}

fn dispatch<'a>(
    service: &'a Arc<Service>,
    gateway: &'a Gateway,
    credential: &'a str,
    request: BrokerCommandV1,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<Response, BrokerErrorCodeV1>> + Send + 'a>,
> {
    // Allocate the whole test helper off the matrix's future, including the
    // dispatch construction/temporaries before its inner Box::pin.
    Box::pin(async move {
        let before = gateway.identities.load(Ordering::SeqCst);
        let pending = service.dispatch(EnvelopeV1 {
            version: 1,
            profile_id: "cancel_retry_test".into(),
            credential: credential.into(),
            request,
        });
        eprintln!(
            "unboxed_dispatch_future_bytes={}",
            std::mem::size_of_val(&pending)
        );
        let result = Box::pin(pending).await;
        assert_eq!(
            gateway.identities.load(Ordering::SeqCst),
            before + 1,
            "every request authenticates afresh against the actual gateway"
        );
        result
    })
}

fn fresh_namespace_fixture(gateway: &Gateway, namespace: Uuid) -> Fixture {
    let mut fixture = Fixture::with_manual_arm_and_identity(false, Some((&gateway.url, namespace)));
    let base = fixture._temp.path().canonicalize().unwrap();
    let fresh = base.join("fresh-broker");
    fs::create_dir(&fresh).unwrap();
    fs::set_permissions(&fresh, fs::Permissions::from_mode(0o700)).unwrap();
    let mut config: Value =
        serde_json::from_slice(fixture.service.registry.source.bytes()).unwrap();
    config["root"] = json!(fresh);
    let path = base.join("fresh-registry.json");
    fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    fixture.service.registry = Arc::new(Registry::load(&path).unwrap());
    fixture.service.root = Directory::open_existing(&fresh).unwrap();
    fixture.service.profiles =
        BTreeMap::from([("cancel_retry_test".into(), Arc::new(ProfileJobs::new()))]);
    fixture
}

fn namespace_inventory(root: &Path) -> Vec<PathBuf> {
    fn visit(root: &Path, current: &Path, entries: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(current).unwrap() {
            let entry = entry.unwrap();
            entries.push(entry.path().strip_prefix(root).unwrap().to_owned());
            if entry.file_type().unwrap().is_dir() {
                visit(root, &entry.path(), entries);
            }
        }
    }
    let mut entries = Vec::new();
    visit(root, root, &mut entries);
    entries.sort();
    entries
}

#[tokio::test(flavor = "current_thread")]
async fn recovery_fresh_profile_reads_and_submit_do_not_initialize_a_journal_or_full_writer() {
    use crate::broker::service::{RequestedExecutionV1, encode_submit};
    let gateway = Gateway::active().await;
    let mut fixture = fresh_namespace_fixture(&gateway, gateway.namespace);
    let project = fixture._temp.path().canonicalize().unwrap().join("sources");
    let root = fixture.service.root.path().to_owned();
    assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
    let (_role_temp, role) = recovery_role_fixture();
    let child = recovery_child(&role, None);
    let client = warmed_recovery_client(&role).await;
    fixture.service.enforcer = None;
    fixture.service.stop_enforcer = Some(client.clone());
    fixture.service.fenced.store(true, Ordering::SeqCst);
    let service = Arc::new(fixture.service);
    let jobs = service.profile_jobs("cancel_retry_test").unwrap();
    assert!(jobs.writer.get().is_none());
    let payload = encode_submit(&SubmitV1 {
        version: 1,
        operation_id: Uuid::new_v4(),
        project,
        execution: RequestedExecutionV1::Command {
            argv: vec!["/usr/bin/id".into()],
        },
        timeout_seconds: None,
        capture_output: false,
    })
    .unwrap();
    let mut refused_requests = Vec::new();
    for (name, request) in [
        (
            "Status",
            BrokerCommandV1::Status {
                operation_id: Uuid::new_v4(),
            },
        ),
        (
            "List",
            BrokerCommandV1::List {
                after: None,
                limit: 20,
            },
        ),
        ("Submit", BrokerCommandV1::Submit { payload }),
    ] {
        let result = dispatch(&service, &gateway, A_RUNNER, request).await;
        refused_requests.push((name, result));
    }
    let entries = fs::read_dir(&root).unwrap().count();
    let full = jobs
        .writer
        .get()
        .is_some_and(|capabilities| capabilities.full().is_some());
    let still_fenced = service.fenced.load(Ordering::SeqCst);

    // The original fixture also owns an existing, valid journal outside the
    // fresh namespace. First keep its old owners/lease alive while the new
    // empty profile cell tries to open it, then close those old owners and
    // require a successful lazy retry through the same STOP-only service.
    let base = fixture._temp.path().canonicalize().unwrap();
    let existing_root = base.join("root");
    let existing_inventory = namespace_inventory(&existing_root);
    let scope = fixture.writer.scope().clone();
    let operation = fixture.operation;
    drop(jobs);
    let mut service = Arc::try_unwrap(service)
        .unwrap_or_else(|_| panic!("completed fixture dispatch must release its service Arc"));
    service.registry = Arc::new(Registry::load(&base.join("registry.json")).unwrap());
    service.root = Directory::open_existing(&existing_root).unwrap();
    service.profiles = BTreeMap::from([("cancel_retry_test".into(), Arc::new(ProfileJobs::new()))]);
    let service = Arc::new(service);
    let first_open = dispatch(
        &service,
        &gateway,
        A_RUNNER,
        BrokerCommandV1::RecoveryStatus {
            operation_id: operation,
        },
    )
    .await;
    let first_cell_empty = service
        .profile_jobs(scope.profile_id())
        .unwrap()
        .writer
        .get()
        .is_none();
    // Independent persistent_read opens get separate file descriptions. The
    // original Journal/Writer above still hold LOCK_EX, so the first open's
    // Busy maps through filesystem() to NeedsReconciliation. Only now close
    // those exact fixture owners; never unlock or replace their lock inode.
    drop((fixture.jobs, fixture.writer, fixture.journal));
    let expires = std::time::Instant::now() + Duration::from_secs(3);
    let existing_status = loop {
        let result = dispatch(
            &service,
            &gateway,
            A_RUNNER,
            BrokerCommandV1::RecoveryStatus {
                operation_id: operation,
            },
        )
        .await;
        if result.is_ok() || std::time::Instant::now() >= expires {
            break result;
        }
        // The old writer closes its queue gracefully and retains its lease
        // until it drains. Do not force-release or replace its retained lock.
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    let existing_jobs = service.profile_jobs(scope.profile_id()).unwrap();
    let existing_stops_only = existing_jobs
        .writer
        .get()
        .is_some_and(|capabilities| capabilities.is_stop_only());
    let existing_inventory_after = namespace_inventory(&existing_root);
    drop((existing_jobs, service, client));
    drop(child);
    assert!(still_fenced);
    for (name, result) in refused_requests {
        assert!(
            matches!(result, Err(BrokerErrorCodeV1::NeedsReconciliation)),
            "authenticated {name} must refuse explicitly before a Full factory"
        );
    }
    assert_eq!(
        entries, 0,
        "read/Submit in existing-only recovery must preserve the fresh profile inventory"
    );
    assert!(
        !full,
        "no lazy path may promote recovery into a Full writer"
    );
    assert!(
        matches!(first_open, Err(BrokerErrorCodeV1::NeedsReconciliation)),
        "the first reopen must refuse while the original fixture owners hold the lease"
    );
    assert!(
        first_cell_empty,
        "a refused open must leave the writer cell empty"
    );
    assert!(
        matches!(existing_status, Ok(Response { reply: BrokerReplyV1::RecoveryStatus { status }, .. }) if status.operation_id == operation),
        "lazy reopen must keep the existing job's recovery metadata usable"
    );
    assert!(
        existing_stops_only,
        "lazy reopen must retain only the STOP facade"
    );
    assert_eq!(existing_inventory_after, existing_inventory);
}

#[tokio::test(flavor = "current_thread")]
async fn inactive_real_gateway_allows_fresh_stop_and_recovery_metadata_only() {
    let gateway = Gateway::inactive(true).await;
    let fixture =
        Fixture::with_manual_arm_and_identity(false, Some((&gateway.url, gateway.namespace)));
    let operation = fixture.operation;
    let service = Arc::new(fixture.service);
    let probe = DeliveryScope::begin();
    let recovery = || BrokerCommandV1::RecoveryStatus {
        operation_id: operation,
    };
    let response = dispatch(&service, &gateway, A_RUNNER, recovery())
        .await
        .unwrap();
    assert!(matches!(
        response.reply,
        BrokerReplyV1::RecoveryStatus { .. }
    ));
    let command = Uuid::new_v4();
    let cancel = || BrokerCommandV1::Cancel {
        operation_id: operation,
        command_id: command,
    };
    assert!(matches!(
        dispatch(&service, &gateway, A_RUNNER, cancel()).await,
        Err(BrokerErrorCodeV1::NeedsReconciliation)
    ));
    let response = dispatch(&service, &gateway, A_RUNNER, cancel())
        .await
        .unwrap();
    assert!(
        matches!(response.reply, BrokerReplyV1::CommandAcknowledged { command_id, .. } if command_id == command)
    );
    assert_eq!(probe.attempts().len(), 2);
    let response = dispatch(&service, &gateway, A_RUNNER, recovery())
        .await
        .unwrap();
    assert!(
        matches!(response.reply, BrokerReplyV1::RecoveryStatus { status } if status.cancel_commands.len() == 1)
    );
    assert_eq!(
        probe.attempts().len(),
        2,
        "diagnostics do not issue stop delivery"
    );
}

async fn refused_stop_identity(
    gateway: &Gateway,
    namespace: Uuid,
    credential: &str,
    wanted: BrokerErrorCodeV1,
) {
    let fixture = fresh_namespace_fixture(gateway, namespace);
    let root = fixture.service.root.path().to_owned();
    let service = Arc::new(fixture.service);
    // This oracle proves no namespace publication, not absence of invisible reads.
    assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
    let operation_id = Uuid::new_v4();
    for request in [
        BrokerCommandV1::Cancel {
            operation_id,
            command_id: Uuid::new_v4(),
        },
        BrokerCommandV1::RecoveryStatus { operation_id },
    ] {
        assert!(
            matches!(dispatch(&service, gateway, credential, request).await, Err(code) if code == wanted)
        );
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn fresh_gateway_guards_refuse_wrong_identity_or_start_before_namespace_creation() {
    let gateway = Gateway::inactive(true).await;
    let fixture = fresh_namespace_fixture(&gateway, gateway.namespace);
    let root = fixture.service.root.path().to_owned();
    let service = Arc::new(fixture.service);
    assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
    let before = gateway.requests.load(Ordering::SeqCst);
    assert!(matches!(
        Box::pin(service.dispatch(EnvelopeV1 {
            version: 1,
            profile_id: "cancel_retry_test".into(),
            credential: String::new(),
            request: BrokerCommandV1::Cancel {
                operation_id: Uuid::new_v4(),
                command_id: Uuid::new_v4(),
            },
        }))
        .await,
        Err(BrokerErrorCodeV1::Unauthorized)
    ));
    assert_eq!(gateway.requests.load(Ordering::SeqCst), before);
    assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
    for request in [
        BrokerCommandV1::Submit {
            payload: "{}".into(),
        },
        BrokerCommandV1::Resume {
            operation_id: Uuid::new_v4(),
            command_id: Uuid::new_v4(),
        },
        BrokerCommandV1::Approve {
            operation_id: Uuid::new_v4(),
            command_id: Uuid::new_v4(),
            request_hash: "a".repeat(64),
        },
    ] {
        assert!(matches!(
            dispatch(&service, &gateway, A_RUNNER, request).await,
            Err(BrokerErrorCodeV1::IncompatibleControl)
        ));
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
    }
    for (credential, wanted) in [
        (A_VIEWER, BrokerErrorCodeV1::Forbidden),
        (B_RUNNER, BrokerErrorCodeV1::Forbidden),
        (
            "unknown-valid-shape-token-eeeeeeeeeeeeeeee",
            BrokerErrorCodeV1::Unauthorized,
        ),
    ] {
        refused_stop_identity(&gateway, gateway.namespace, credential, wanted).await;
    }
    refused_stop_identity(
        &gateway,
        Uuid::new_v4(),
        A_RUNNER,
        BrokerErrorCodeV1::Forbidden,
    )
    .await;
    let schema2 = Gateway::inactive(false).await;
    refused_stop_identity(
        &schema2,
        schema2.namespace,
        A_RUNNER,
        BrokerErrorCodeV1::IncompatibleControl,
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn empty_credential_is_refused_on_the_actual_connection_without_namespace_creation() {
    fn future_bytes<A, F>(_: impl FnOnce(A) -> F) -> usize {
        std::mem::size_of::<F>()
    }
    let gateway = Gateway::inactive(true).await;
    let fixture = fresh_namespace_fixture(&gateway, gateway.namespace);
    let root = fixture.service.root.path().to_owned();
    let service = Arc::new(fixture.service);
    // These factories are never invoked: measuring a type cannot overflow by
    // first constructing that future. This is diagnostic, not a size limit.
    eprintln!(
        "dispatch_future_bytes={} connection_future_bytes={}",
        future_bytes(|envelope: EnvelopeV1| service.dispatch(envelope)),
        future_bytes(|stream: UnixStream| Arc::clone(&service).connection(stream))
    );
    let (mut client, server) = UnixStream::pair().unwrap();
    // Exercise the production connection/frame/dispatch path in a real task,
    // without boxing dispatch in the test or increasing the thread's stack.
    let mut task = tokio::spawn(Arc::clone(&service).connection(server));
    write_frame(
        &mut client,
        &EnvelopeV1 {
            version: 1,
            profile_id: "cancel_retry_test".into(),
            credential: String::new(),
            request: BrokerCommandV1::RecoveryStatus {
                operation_id: Uuid::new_v4(),
            },
        },
        FRAME_BYTES,
        Duration::from_secs(2),
    )
    .await
    .unwrap();
    let reply: ReplyEnvelopeV1 = read_frame(&mut client, FRAME_BYTES, Duration::from_secs(2))
        .await
        .unwrap();
    let joined = tokio::time::timeout(Duration::from_secs(2), &mut task).await;
    if joined.is_err() {
        task.abort();
    }
    joined.unwrap().unwrap();
    assert_eq!(reply.version, 1);
    assert!(matches!(
        reply.reply,
        BrokerReplyV1::Error {
            code: BrokerErrorCodeV1::Unauthorized
        }
    ));
    assert_eq!(gateway.requests.load(Ordering::SeqCst), 0);
    assert_eq!(fs::read_dir(root).unwrap().count(), 0);
}
