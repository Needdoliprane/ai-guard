//! Fresh HTTP authentication against the real gateway before STOP metadata.
//! The delivery probe qualifies routing only; no runtime or IPC stop is claimed.
use super::*;
use crate::api::{GatewayState, TenantGateway, gateway_router};
use std::sync::atomic::AtomicUsize;

const A_RUNNER: &str = "runner-a-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const A_VIEWER: &str = "viewer-a-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const B_RUNNER: &str = "runner-b-cccccccccccccccccccccccccccccccc";
const B_VIEWER: &str = "viewer-b-dddddddddddddddddddddddddddddddd";
const TENANT: &str = "tenant-cancel-retry-test";

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
                    if request.uri().path() == "/api/v1/control/protocol" {
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

async fn dispatch(
    service: &Arc<Service>,
    gateway: &Gateway,
    credential: &str,
    request: BrokerCommandV1,
) -> Result<Response, BrokerErrorCodeV1> {
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
    // Keep the matrix's outer future bounded instead of embedding a copy of
    // the complete dispatcher state at every await site in this debug test.
    let result = Box::pin(pending).await;
    assert_eq!(
        gateway.identities.load(Ordering::SeqCst),
        before + 1,
        "every request authenticates afresh against the actual gateway"
    );
    result
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
