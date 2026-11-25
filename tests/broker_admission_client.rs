use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::Store,
    control::{
        AdmissionRefusalV1, AdmissionResultV1, AdmissionV1, DecisionPhaseV1, DecisionReceiptV1,
        LaunchSpecV1, NetworkV1, QuotaPolicyV1, RestartPolicyV1, RuntimeKindV1, SecurityPresetV1,
        SupervisedAdmissionV1, SupervisionProtocolV1,
    },
    model::{CommandSpec, DecisionRequest, Outcome},
    policy::{self, Policy},
    redaction::Redactor,
    runner::AuditClient,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::{fs, os::unix::fs::PermissionsExt, sync::Arc};
use tempfile::TempDir;
use uuid::Uuid;

const TOKEN: &str = "runner-token-aaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const WRONG_TOKEN: &str = "wrong-token-bbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const HASH_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

struct Server {
    url: String,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Fixture {
    _root: TempDir,
    store: Arc<Store>,
    server: Server,
    initial_worker_corrupted: Arc<AtomicBool>,
}

#[derive(Clone, Copy)]
enum InitialWorkerMutation {
    StartOperationPresent,
    NilAttemptId,
}

#[derive(Clone)]
struct InitialWorkerCorruption {
    mutation: InitialWorkerMutation,
    applied: Arc<AtomicBool>,
}

impl Fixture {
    async fn new(quota: QuotaPolicyV1) -> Self {
        Self::with_policy_and_response_mutation(quota, Policy::default(), None).await
    }

    async fn with_policy_and_response_mutation(
        quota: QuotaPolicyV1,
        policy: Policy,
        mutate_initial_worker: Option<InitialWorkerMutation>,
    ) -> Self {
        let root = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let store = Arc::new(
            Store::open_for_tenant(
                &root.path().join("tenant.sqlite"),
                "tenant-admission-client-test",
                Redactor::new(vec![]),
                10_000,
            )
            .unwrap(),
        );
        store
            .migrate_control_v3(&root.path().join("tenant.before-supervision.sqlite"))
            .unwrap();
        let state = GatewayState::for_tenants(
            vec![TenantGateway {
                tenant_id: "tenant-admission-client-test".into(),
                store: Arc::clone(&store),
                runner_token: TOKEN.into(),
                viewer_token: "viewer-token-cccccccccccccccccccccccccccc".into(),
            }],
            policy,
            None,
            Redactor::new(vec![]),
        )
        .unwrap()
        .with_worker_quotas(vec![("tenant-admission-client-test".into(), quota)])
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let initial_worker_corrupted = Arc::new(AtomicBool::new(false));
        let mut router = gateway_router(state);
        if let Some(mutation) = mutate_initial_worker {
            router = router.layer(axum::middleware::from_fn_with_state(
                InitialWorkerCorruption {
                    mutation,
                    applied: Arc::clone(&initial_worker_corrupted),
                },
                corrupt_initial_worker_response,
            ));
        }
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            _root: root,
            store,
            server: Server { url, task },
            initial_worker_corrupted,
        }
    }

    fn client(&self, token: &str) -> AuditClient {
        AuditClient::new(&self.server.url, token).unwrap()
    }

    fn admission(&self, profile: &str, argv: &[&str]) -> SupervisedAdmissionV1 {
        let session_id = Uuid::new_v4().to_string();
        let request = DecisionRequest {
            request_id: Uuid::new_v4().to_string(),
            profile: profile.into(),
            command: CommandSpec {
                argv: argv.iter().map(|arg| (*arg).to_owned()).collect(),
                cwd: "/workspace".into(),
            },
            project_id: None,
            sandbox_id: Some(session_id.clone()),
            agent_id: None,
            session_id: Some(session_id.clone()),
            correlation_id: None,
            trace_id: None,
            human_approved: false,
            approval_hash: None,
            execution_mode: None,
            provider_resolution: None,
        };
        let policy = Policy::default();
        let decision = policy::evaluate(&policy, &request);
        let stored = self
            .store
            .decision(&request, Some((decision, serde_json::Value::Null)))
            .unwrap()
            .unwrap();
        let admission = AdmissionV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            decision_request: request.clone(),
            receipt: DecisionReceiptV1 {
                version: 1,
                request_id: request.request_id.clone(),
                request_hash: policy::request_hash(&request),
                phase: DecisionPhaseV1::Initial,
                audit_seq: stored.seq,
                audit_hash: stored.hash,
            },
            launch: LaunchSpecV1 {
                version: 1,
                session_id,
                command: request.command.clone(),
                profile: request.profile.clone(),
                runtime_kind: RuntimeKindV1::Docker,
                authority_id: Uuid::new_v4().to_string(),
                image_ref: "worker:admission-client-test".into(),
                image_id: format!("sha256:{HASH_A}"),
                network: NetworkV1::None,
                cpu_millis: 2000,
                memory_bytes: 1_073_741_824,
                pids_limit: 256,
                timeout_seconds: 600,
                security_preset: SecurityPresetV1::FixedSandboxV2,
                restart_policy: RestartPolicyV1::No,
                workspace_manifest_digest: HASH_B.into(),
                workspace_import_policy_version: 1,
                provider: "generic".into(),
                capture_output: false,
                credential_binding_id: None,
                credential_names: vec![],
                provider_resolution: None,
                egress: None,
            },
        };
        let supervised = SupervisedAdmissionV1 {
            version: 1,
            protocol: SupervisionProtocolV1::BrokerSupervisionV1,
            intent_hash: HASH_C.into(),
            admission,
        };
        supervised.validate().unwrap();
        supervised
    }
}

async fn corrupt_initial_worker_response(
    axum::extract::State(corruption): axum::extract::State<InitialWorkerCorruption>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    // The typed client submits through the explicit runner control transport
    // (`/control` prefix), never the historical public path. Targeting the
    // public path left the reply uncorrupted and the rejection untested.
    let target = request.uri().path() == "/control/api/v1/workers/supervised-reservations";
    let response = next.run(request).await;
    if !target || response.status() != axum::http::StatusCode::CREATED {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let bytes = axum::body::to_bytes(body, 512 * 1024).await.unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    match corruption.mutation {
        InitialWorkerMutation::StartOperationPresent => {
            value["worker"]["attempt"]["start_operation_id"] =
                serde_json::json!(Uuid::new_v4().to_string());
        }
        InitialWorkerMutation::NilAttemptId => {
            value["worker"]["attempt"]["attempt_id"] = serde_json::json!(Uuid::nil().to_string());
        }
    }
    corruption.applied.store(true, Ordering::SeqCst);
    parts.headers.remove(axum::http::header::CONTENT_LENGTH);
    axum::http::Response::from_parts(
        parts,
        axum::body::Body::from(serde_json::to_vec(&value).unwrap()),
    )
}

fn quota(max_active_workers: u32) -> QuotaPolicyV1 {
    QuotaPolicyV1 {
        max_active_workers,
        ..QuotaPolicyV1::default()
    }
}

#[tokio::test]
async fn typed_client_preserves_created_replay_and_authentication_error() {
    let fixture = Fixture::new(quota(2)).await;
    let request = fixture.admission("observe", &["/usr/bin/id", "-u"]);
    assert_eq!(
        policy::evaluate(&Policy::default(), &request.admission.decision_request).outcome,
        Outcome::Allow
    );

    let wrong = fixture.client(WRONG_TOKEN);
    let auth_error = wrong.admit_supervised(&request).await.unwrap_err();
    assert!(!auth_error.to_string().contains(WRONG_TOKEN));
    assert!(
        fixture
            .store
            .worker_operation(&request.admission.operation_id)
            .unwrap()
            .is_none()
    );

    let client = fixture.client(TOKEN);
    let first: AdmissionResultV1 = client.admit_supervised(&request).await.unwrap();
    assert!(first.accepted);
    assert!(!first.replayed);
    assert_eq!(first.refusal, None);
    assert!(first.worker.is_some());
    assert!(first.receipt.audit_seq > 0);

    let replay = client.admit_supervised(&request).await.unwrap();
    assert!(replay.accepted && replay.replayed);
    assert_eq!(replay.receipt, first.receipt);
}

#[tokio::test]
async fn typed_client_preserves_policy_refusal_and_its_replay_receipt() {
    let default_policy = Policy::default();
    let fixture = Fixture::with_policy_and_response_mutation(
        quota(2),
        Policy {
            allowed_argv: vec![],
            denied_executables: default_policy.denied_executables.clone(),
        },
        None,
    )
    .await;
    let request = fixture.admission("strict", &["/usr/bin/id"]);
    assert_eq!(
        policy::evaluate(&default_policy, &request.admission.decision_request).outcome,
        Outcome::Allow
    );

    let client = fixture.client(TOKEN);
    let first = client.admit_supervised(&request).await.unwrap();
    assert!(!first.accepted && !first.replayed);
    assert_eq!(first.refusal, Some(AdmissionRefusalV1::Policy));
    assert!(first.worker.is_none());
    assert!(first.receipt.audit_seq > 0);

    let replay = client.admit_supervised(&request).await.unwrap();
    assert!(!replay.accepted && replay.replayed);
    assert_eq!(replay.refusal, Some(AdmissionRefusalV1::Policy));
    assert_eq!(replay.receipt, first.receipt);
}

#[tokio::test]
async fn typed_client_preserves_real_quota_refusal_and_its_replay_receipt() {
    let fixture = Fixture::new(quota(1)).await;
    let client = fixture.client(TOKEN);
    let first_request = fixture.admission("observe", &["/usr/bin/id", "-u"]);
    let first = client.admit_supervised(&first_request).await.unwrap();
    assert!(first.accepted && !first.replayed);

    let quota_request = fixture.admission("observe", &["/usr/bin/id", "-u"]);
    let refused = client.admit_supervised(&quota_request).await.unwrap();
    assert!(!refused.accepted && !refused.replayed);
    assert_eq!(refused.refusal, Some(AdmissionRefusalV1::Quota));
    assert!(refused.worker.is_none());
    assert!(refused.receipt.audit_seq > 0);

    let replay = client.admit_supervised(&quota_request).await.unwrap();
    assert!(!replay.accepted && replay.replayed);
    assert_eq!(replay.refusal, Some(AdmissionRefusalV1::Quota));
    assert_eq!(replay.receipt, refused.receipt);
}

#[tokio::test]
async fn typed_client_rejects_an_inconsistent_initial_worker_from_real_gateway_response() {
    for mutation in [
        InitialWorkerMutation::StartOperationPresent,
        InitialWorkerMutation::NilAttemptId,
    ] {
        let fixture =
            Fixture::with_policy_and_response_mutation(quota(1), Policy::default(), Some(mutation))
                .await;
        let request = fixture.admission("observe", &["/usr/bin/id", "-u"]);
        let client = fixture.client(TOKEN);
        let result = client.admit_supervised(&request).await;
        assert!(
            fixture.initial_worker_corrupted.load(Ordering::SeqCst),
            "the middleware must corrupt the real gateway reply on the runner control transport"
        );
        assert!(
            result.is_err(),
            "the Store-created initial worker cannot contain a start-operation identity or a nil attempt ID"
        );
        assert!(
            fixture
                .store
                .worker_operation(&request.admission.operation_id)
                .unwrap()
                .is_some(),
            "the gateway performed a real Store reservation before the test middleware altered its reply"
        );
    }
}
