//! Real Axum/SQLite contracts for the v1 broker-supervision API.
//! Fixtures exercise the control protocol, not a Docker/Podman runtime.
#[path = "supervision_api/control_transport.rs"]
mod control_transport;
mod support;

use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::Store,
    control::{
        AdmissionResultV1, AdmissionV1, ArmResultV1, ArmSubjectV1, ArmV1, ArmedStartClaimV1,
        ClaimDispositionV1, DecisionPhaseV1, DecisionReceiptV1, ExportRequestV1,
        HelperClaimResultV1, HelperCleanupV1, HelperExecutionPolicyV1, HelperObservationV1,
        HelperPermissionV1, HelperPurposeV1, HelperStartClaimV1, HostClockMethodV1,
        HostClockSampleV1, ImportVerificationReportV1, MutationReceiptV1, MutationResultV1,
        NetworkV1, RuntimeInspectionV1, RuntimeObservationV1, RuntimeStatusV1, SecurityPresetV1,
        SupervisedAdmissionV1, SupervisionProtocolV1, WorkerMutationV1, WorkerStateV1,
        WorkerTransitionV1, WorkerV1, helper_execution_policy_v1,
    },
    jev::JevClient,
    model::{CommandSpec, DecisionRequest},
    policy::{self, Policy},
    redaction::Redactor,
};
use axum::{
    Router,
    body::{Body, Bytes, to_bytes},
    http::{HeaderValue, Method, Request, Response, StatusCode, header},
};
use futures_util::stream;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    convert::Infallible,
    net::TcpListener,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    task::Poll,
    thread,
    time::{Duration, Instant},
};
use tower::ServiceExt;
use uuid::Uuid;

const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const HASH_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const WORKER_PATH_ID: &str = "00000000-0000-4000-8000-000000000001";

const NEW_WRITE_PATHS: [&str; 7] = [
    "/api/v1/workers/supervised-reservations",
    "/api/v1/workers/00000000-0000-4000-8000-000000000001/arms",
    "/api/v1/workers/00000000-0000-4000-8000-000000000001/armed-start-claims",
    "/api/v1/workers/00000000-0000-4000-8000-000000000001/helper-start-claims",
    "/api/v1/workers/00000000-0000-4000-8000-000000000001/helper-observations",
    "/api/v1/workers/00000000-0000-4000-8000-000000000001/helper-cleanup",
    "/api/v1/workers/00000000-0000-4000-8000-000000000001/export-requests",
];

struct Fixture {
    _directory: tempfile::TempDir,
    router: Router,
    a: Arc<Store>,
    b: Arc<Store>,
    v2: Arc<Store>,
    runner_a: String,
    viewer_a: String,
    runner_b: String,
    viewer_b: String,
    runner_v2: String,
    viewer_v2: String,
}

impl Fixture {
    fn new() -> Self {
        Self::with_max_events(10_000)
    }

    fn with_max_events(max_events: u64) -> Self {
        let directory = support::private_tempdir();
        let redactor = Redactor::new(vec![]);
        let a = Arc::new(
            Store::open_for_tenant(
                &directory.path().join("a.sqlite"),
                "tenant-a",
                redactor.clone(),
                max_events,
            )
            .unwrap(),
        );
        a.migrate_control_v3(&directory.path().join("a-v3-backup.sqlite"))
            .unwrap();
        let b = Arc::new(
            Store::open_for_tenant(
                &directory.path().join("b.sqlite"),
                "tenant-b",
                redactor.clone(),
                max_events,
            )
            .unwrap(),
        );
        b.migrate_control_v3(&directory.path().join("b-v3-backup.sqlite"))
            .unwrap();
        // This tenant intentionally remains schema2 to prove the protocol
        // endpoint reports maintenance without granting a v3 capability.
        let v2 = Arc::new(
            Store::open_for_tenant(
                &directory.path().join("v2.sqlite"),
                "tenant-v2",
                redactor.clone(),
                max_events,
            )
            .unwrap(),
        );
        let runner_a = "fixture-control-runner-a".repeat(3);
        let viewer_a = "fixture-control-viewer-a".repeat(3);
        let runner_b = "fixture-control-runner-b".repeat(3);
        let viewer_b = "fixture-control-viewer-b".repeat(3);
        let viewer_v2 = "fixture-control-viewer-v2".repeat(3);
        let runner_v2 = "fixture-control-runner-v2".repeat(3);
        let state = GatewayState::for_tenants(
            vec![
                TenantGateway {
                    tenant_id: "tenant-a".into(),
                    store: Arc::clone(&a),
                    runner_token: runner_a.clone(),
                    viewer_token: viewer_a.clone(),
                },
                TenantGateway {
                    tenant_id: "tenant-b".into(),
                    store: Arc::clone(&b),
                    runner_token: runner_b.clone(),
                    viewer_token: viewer_b.clone(),
                },
                TenantGateway {
                    tenant_id: "tenant-v2".into(),
                    store: Arc::clone(&v2),
                    runner_token: runner_v2.clone(),
                    viewer_token: viewer_v2.clone(),
                },
            ],
            Policy::default(),
            None,
            redactor,
        )
        .unwrap();
        Self {
            _directory: directory,
            router: gateway_router(state),
            a,
            b,
            v2,
            runner_a,
            viewer_a,
            runner_b,
            viewer_b,
            runner_v2,
            viewer_v2,
        }
    }

    fn heads(&self) -> [i64; 3] {
        [
            self.a.head().unwrap(),
            self.b.head().unwrap(),
            self.v2.head().unwrap(),
        ]
    }

    fn assert_envelope_excludes_control_credentials<T: Serialize>(&self, envelope: &T) {
        let serialized = serde_json::to_string(envelope).unwrap();
        assert!(
            [
                &self.runner_a,
                &self.viewer_a,
                &self.runner_b,
                &self.viewer_b,
                &self.runner_v2,
                &self.viewer_v2,
            ]
            .iter()
            .all(|credential| !serialized.contains(credential.as_str())),
            "fixture request envelope must exclude every configured control credential"
        );
    }
}

fn bearer(token: &str) -> HeaderValue {
    HeaderValue::from_str(&format!("Bearer {token}")).unwrap()
}

fn counted_invalid_json_body(polls: Arc<AtomicUsize>) -> Body {
    let mut sent = false;
    Body::from_stream(stream::poll_fn(move |_context| {
        polls.fetch_add(1, Ordering::SeqCst);
        if sent {
            Poll::Ready(None)
        } else {
            sent = true;
            Poll::Ready(Some(Ok::<Bytes, Infallible>(Bytes::from_static(b"{"))))
        }
    }))
}

async fn counted_post(
    router: &Router,
    path: &str,
    authorization: Option<HeaderValue>,
) -> (StatusCode, usize) {
    let polls = Arc::new(AtomicUsize::new(0));
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(value) = authorization {
        builder = builder.header(header::AUTHORIZATION, value);
    }
    let request = builder
        .body(counted_invalid_json_body(Arc::clone(&polls)))
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    (response.status(), polls.load(Ordering::SeqCst))
}

async fn send(
    router: &Router,
    method: Method,
    path: &str,
    token: Option<&str>,
    body: Option<Vec<u8>>,
) -> Response<Body> {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, bearer(token));
    }
    if body.is_some() {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
    }
    router
        .clone()
        .oneshot(builder.body(Body::from(body.unwrap_or_default())).unwrap())
        .await
        .unwrap()
}

async fn json_request<T: Serialize>(
    router: &Router,
    method: Method,
    path: &str,
    token: Option<&str>,
    body: &T,
) -> Response<Body> {
    send(
        router,
        method,
        path,
        token,
        Some(serde_json::to_vec(body).unwrap()),
    )
    .await
}

async fn response_json<T: DeserializeOwned>(response: Response<Body>) -> T {
    let bytes = to_bytes(response.into_body(), 65_536).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn admission(store: &Store) -> AdmissionV1 {
    let session_id = Uuid::new_v4().to_string();
    let request = DecisionRequest {
        request_id: Uuid::new_v4().to_string(),
        profile: "observe".into(),
        command: CommandSpec {
            argv: vec!["/usr/bin/id".into(), "-u".into()],
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
    let decision = store
        .decision(
            &request,
            Some((policy::evaluate(&policy, &request), json!(null))),
        )
        .unwrap()
        .unwrap();
    AdmissionV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        decision_request: request.clone(),
        receipt: DecisionReceiptV1 {
            version: 1,
            request_id: request.request_id.clone(),
            request_hash: policy::request_hash(&request),
            phase: DecisionPhaseV1::Initial,
            audit_seq: decision.seq,
            audit_hash: decision.hash,
        },
        launch: agentsandbox::control::LaunchSpecV1 {
            version: 1,
            session_id,
            command: request.command.clone(),
            profile: request.profile.clone(),
            runtime_kind: agentsandbox::control::RuntimeKindV1::Docker,
            authority_id: Uuid::new_v4().to_string(),
            image_ref: "worker:fixture".into(),
            image_id: format!("sha256:{HASH_A}"),
            network: NetworkV1::None,
            cpu_millis: 2000,
            memory_bytes: 1_073_741_824,
            pids_limit: 256,
            timeout_seconds: 600,
            security_preset: SecurityPresetV1::FixedSandboxV2,
            restart_policy: agentsandbox::control::RestartPolicyV1::No,
            workspace_manifest_digest: HASH_B.into(),
            workspace_import_policy_version: 1,
            provider: "generic".into(),
            capture_output: false,
            credential_binding_id: None,
            credential_names: vec![],
            provider_resolution: None,
        },
    }
}

fn supervised(admission: AdmissionV1) -> SupervisedAdmissionV1 {
    SupervisedAdmissionV1 {
        version: 1,
        protocol: SupervisionProtocolV1::BrokerSupervisionV1,
        intent_hash: HASH_C.into(),
        admission,
    }
}

fn host_clock(ns: u128) -> HostClockSampleV1 {
    HostClockSampleV1 {
        version: 1,
        method: HostClockMethodV1::DarwinMachContinuousV1,
        boot_id: "11111111-1111-4111-8111-111111111111".into(),
        continuous_ns_decimal: ns.to_string(),
        observed_utc: Some("2026-10-30T12:00:00Z".into()),
    }
}

fn inspection_from_helper(
    policy: &HelperExecutionPolicyV1,
    container_id: &str,
) -> RuntimeInspectionV1 {
    RuntimeInspectionV1 {
        authority_id: policy.authority_id.clone(),
        runtime_kind: policy.runtime_kind,
        container_id: container_id.into(),
        stable_name: policy.stable_name.clone(),
        volume_name: policy.volume_name.clone(),
        image_id: policy.image_id.clone(),
        spec_hash: policy.spec_hash().unwrap(),
        cpu_millis: policy.cpu_millis,
        memory_bytes: policy.memory_bytes,
        pids_limit: policy.pids_limit,
        network: policy.network,
        security_preset: policy.security_preset,
        restart_policy: policy.restart_policy,
    }
}

fn helper_arm(
    parent: &WorkerV1,
    purpose: HelperPurposeV1,
    helper_id: Uuid,
    permission: HelperPermissionV1,
    container_id: &str,
) -> ArmV1 {
    let policy = helper_execution_policy_v1(parent, purpose, helper_id).unwrap();
    let clock = host_clock(100_000_000_000);
    ArmV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: parent.worker_id.clone(),
        attempt_id: parent.attempt.attempt_id.clone(),
        expected_revision: parent.revision,
        intent_hash: HASH_C.into(),
        subject: ArmSubjectV1::Helper {
            helper_id: helper_id.to_string(),
            purpose,
            policy_version: 1,
            permission: Box::new(permission),
        },
        inspection: inspection_from_helper(&policy, container_id),
        authority_binding_hash: HASH_A.into(),
        configuration_fingerprint_version: 2,
        configuration_fingerprint: HASH_B.into(),
        deadline_ns_decimal: (clock.continuous_ns().unwrap() + 120_000_000_000).to_string(),
        timeout_seconds: 120,
        clock,
    }
}

fn helper_observation(
    arm: &ArmResultV1,
    helper_id: Uuid,
    verification: Option<ImportVerificationReportV1>,
) -> HelperObservationV1 {
    HelperObservationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        helper_id: helper_id.to_string(),
        arm_receipt: arm.receipt.clone(),
        observation: RuntimeObservationV1 {
            inspection: arm.arm.inspection.clone(),
            observed_at: "2026-10-30T12:00:02Z".into(),
            status: RuntimeStatusV1::Exited,
            running: Some(false),
            started_at: Some("2026-10-30T12:00:00Z".into()),
            finished_at: Some("2026-10-30T12:00:01Z".into()),
            exit_code: Some(0),
            oom_killed: Some(false),
            restart_count: Some(0),
        },
        verification,
    }
}

fn worker_inspection(worker: &WorkerV1) -> RuntimeInspectionV1 {
    RuntimeInspectionV1 {
        authority_id: worker.launch.authority_id.clone(),
        runtime_kind: worker.launch.runtime_kind,
        container_id: "f".repeat(64),
        stable_name: worker.attempt.stable_name.clone(),
        volume_name: worker.attempt.volume_name.clone(),
        image_id: worker.launch.image_id.clone(),
        spec_hash: worker.spec_hash.clone(),
        cpu_millis: worker.launch.cpu_millis,
        memory_bytes: worker.launch.memory_bytes,
        pids_limit: worker.launch.pids_limit,
        network: worker.launch.network,
        security_preset: worker.launch.security_preset,
        restart_policy: worker.launch.restart_policy,
    }
}

fn worker_observation(
    inspection: &RuntimeInspectionV1,
    status: RuntimeStatusV1,
    running: bool,
    finished_at: Option<&str>,
    exit_code: Option<i64>,
) -> RuntimeObservationV1 {
    RuntimeObservationV1 {
        inspection: inspection.clone(),
        observed_at: "2026-10-30T12:00:02Z".into(),
        status,
        running: Some(running),
        started_at: Some("2026-10-30T12:00:00Z".into()),
        finished_at: finished_at.map(str::to_owned),
        exit_code,
        oom_killed: Some(false),
        restart_count: Some(0),
    }
}

async fn transition(
    router: &Router,
    token: &str,
    worker: &WorkerV1,
    transition: WorkerTransitionV1,
) -> WorkerV1 {
    let mutation = WorkerMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        expected_revision: worker.revision,
        transition,
    };
    let path = format!("/api/v1/workers/{}/transitions", worker.worker_id);
    let response = json_request(router, Method::POST, &path, Some(token), &mutation).await;
    assert_eq!(response.status(), StatusCode::OK);
    let result: MutationResultV1 = response_json(response).await;
    result.worker
}

#[tokio::test]
async fn new_write_routes_authenticate_before_polling_bodies_and_legacy_aliases_remain() {
    let fixture = Fixture::new();
    let before = fixture.heads();
    let mut failures = Vec::new();
    for path in NEW_WRITE_PATHS {
        let cases = [
            ("missing", None, StatusCode::UNAUTHORIZED),
            (
                "invalid",
                Some(bearer("not-a-configured-credential")),
                StatusCode::UNAUTHORIZED,
            ),
            (
                "viewer-a",
                Some(bearer(&fixture.viewer_a)),
                StatusCode::FORBIDDEN,
            ),
            (
                "viewer-b",
                Some(bearer(&fixture.viewer_b)),
                StatusCode::FORBIDDEN,
            ),
        ];
        for (label, authorization, expected) in cases {
            let (status, polls) = counted_post(&fixture.router, path, authorization).await;
            if status != expected {
                failures.push(format!("{path} {label}: expected {expected}, got {status}"));
            }
            if polls != 0 {
                failures.push(format!("{path} {label}: body polled {polls} time(s)"));
            }
        }
        let polls = Arc::new(AtomicUsize::new(0));
        let request = Request::builder()
            .method(Method::POST)
            .uri(path)
            .header(header::AUTHORIZATION, bearer(&fixture.runner_a))
            .header(header::CONTENT_TYPE, "application/json")
            .body(counted_invalid_json_body(Arc::clone(&polls)))
            .unwrap();
        let response = fixture.router.clone().oneshot(request).await.unwrap();
        if response.status() != StatusCode::BAD_REQUEST {
            failures.push(format!(
                "{path} runner malformed JSON: expected 400, got {}",
                response.status()
            ));
        }
        if polls.load(Ordering::SeqCst) == 0 {
            failures.push(format!("{path} runner malformed JSON body was not read"));
        }
    }
    let after = fixture.heads();
    if after != before {
        failures.push(format!(
            "denied/malformed writes changed audit heads: {before:?} -> {after:?}"
        ));
    }

    let protocol_alias = send(
        &fixture.router,
        Method::GET,
        "/api/control/protocol",
        Some(&fixture.viewer_a),
        None,
    )
    .await;
    if protocol_alias.status() != StatusCode::NOT_FOUND {
        failures.push(format!(
            "new protocol route must remain v1-only, alias returned {}",
            protocol_alias.status()
        ));
    }
    let legacy_v1 = send(
        &fixture.router,
        Method::GET,
        "/api/v1/workers",
        Some(&fixture.viewer_a),
        None,
    )
    .await;
    let legacy_alias = send(
        &fixture.router,
        Method::GET,
        "/api/workers",
        Some(&fixture.viewer_a),
        None,
    )
    .await;
    if legacy_v1.status() != StatusCode::OK || legacy_alias.status() != StatusCode::OK {
        failures.push("existing worker read aliases stopped returning 200".into());
    } else {
        let versioned: Value = response_json(legacy_v1).await;
        let alias: Value = response_json(legacy_alias).await;
        if versioned != alias {
            failures.push("existing /api and /api/v1 worker reads diverged".into());
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn control_protocol_reports_authenticated_tenant_role_namespace_and_inactive_schema2() {
    let fixture = Fixture::new();
    let mut failures = Vec::new();
    let cases = [
        (&fixture.viewer_a, "tenant-a", "viewer", 3, true),
        (&fixture.runner_a, "tenant-a", "runner", 3, true),
        (&fixture.viewer_b, "tenant-b", "viewer", 3, true),
        (&fixture.viewer_v2, "tenant-v2", "viewer", 2, false),
    ];
    let mut namespaces = Vec::new();
    for (token, tenant, role, schema_version, active) in cases {
        let response = send(
            &fixture.router,
            Method::GET,
            "/api/v1/control/protocol",
            Some(token),
            None,
        )
        .await;
        if response.status() != StatusCode::OK {
            failures.push(format!(
                "protocol for {tenant}/{role}: expected 200, got {}",
                response.status()
            ));
            continue;
        }
        let protocol: agentsandbox::control::ControlProtocolV1 = response_json(response).await;
        if protocol.tenant_id != tenant
            || protocol.role != role
            || protocol.schema_version != schema_version
            || protocol.active != active
        {
            failures.push(format!(
                "protocol scope mismatch for {tenant}/{role}: {}",
                serde_json::to_string(&protocol).unwrap()
            ));
        }
        if schema_version == 3 {
            if protocol.protocols != [SupervisionProtocolV1::BrokerSupervisionV1] {
                failures.push(format!("schema3 protocol missing for {tenant}"));
            }
            namespaces.push(protocol.control_namespace_uuid.unwrap_or_default());
        } else if !protocol.protocols.is_empty() || protocol.active {
            failures.push("schema2 tenant advertised an active supervision protocol".into());
        }
    }
    if namespaces.len() != 3 {
        failures.push(format!(
            "expected three schema3 tenant protocol namespaces, got {}",
            namespaces.len()
        ));
    } else {
        for namespace in &namespaces {
            match Uuid::parse_str(namespace) {
                Ok(id) if !id.is_nil() => {}
                _ => failures
                    .push("schema3 protocol returned an invalid control namespace UUID".into()),
            }
        }
        if namespaces[0] != namespaces[1] {
            failures
                .push("same tenant advertised different namespaces to viewer and runner".into());
        }
        if namespaces[0] == namespaces[2] {
            failures.push("independent tenants shared a control namespace".into());
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn ordinary_exhaustion_keeps_control_capacity_and_control_exhaustion_keeps_revocation() {
    use agentsandbox::{audit::AuditCapacityFailureV1, identity::*};

    let fixture = Fixture::with_max_events(64);
    let path = fixture._directory.path();
    fixture
        .a
        .migrate_identity_v4(&path.join("a-before-v4"))
        .unwrap();
    fixture
        .a
        .migrate_human_access_v5(&path.join("a-before-v5"))
        .unwrap();
    let principal = PrincipalRefV1::HumanLocal {
        installation_id: Uuid::new_v4().to_string(),
        account_id: Uuid::new_v4().to_string(),
    };
    let account_change = |expected_revision, active| IdentityMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        actor: PrincipalRefV1::LegacyControl {
            credential_id: "fixture-capacity-operator".into(),
            role: LegacyControlRoleV1::Runner,
        },
        change: IdentityChangeV1::Account {
            expected_revision,
            account: HumanAccountV1 {
                version: 1,
                principal: principal.clone(),
                display_name: "Capacity test account".into(),
                revision: expected_revision + 1,
                active,
            },
        },
    };
    fixture.a.mutate_identity(&account_change(0, true)).unwrap();
    let admission = supervised(admission(&fixture.a));
    let response = json_request(
        &fixture.router,
        Method::POST,
        "/api/v1/workers/supervised-reservations",
        Some(&fixture.runner_a),
        &admission,
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let reserved: AdmissionResultV1 = response_json(response).await;
    let worker = reserved.worker.unwrap();

    let mut reserved_capacity = false;
    for _ in 0..64 {
        match fixture
            .a
            .append(agentsandbox::model::Event::new("capacity.fill"))
        {
            Ok(_) => {}
            Err(error) => {
                assert_eq!(
                    error.downcast_ref::<AuditCapacityFailureV1>(),
                    Some(&AuditCapacityFailureV1::OrdinaryWriteBudgetExhausted)
                );
                reserved_capacity = true;
                break;
            }
        }
    }
    assert!(
        reserved_capacity,
        "the logical reserve must be reached before the physical limit"
    );
    let before = fixture.a.integrity().unwrap();
    assert!(before.valid && fixture.a.healthy());
    assert_eq!(before.event_count, 40);
    let mutation = WorkerMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        expected_revision: worker.revision,
        transition: WorkerTransitionV1::Prepare,
    };
    let response = json_request(
        &fixture.router,
        Method::POST,
        &format!("/api/v1/workers/{}/transitions", worker.worker_id),
        Some(&fixture.runner_a),
        &mutation,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let accepted: MutationResultV1 = response_json(response).await;
    assert_eq!(accepted.worker.revision, worker.revision + 1);
    assert_eq!(accepted.receipt.audit_seq, 41);
    assert!(
        fixture
            .a
            .worker_operation(&mutation.operation_id)
            .unwrap()
            .is_some()
    );

    // Ordinary ingestion has stopped at40. Fill only the remaining trusted
    // control share through the real runner route; identity keeps its8 slots.
    for _ in 41..56 {
        let response = json_request(
            &fixture.router,
            Method::POST,
            "/api/v1/control/events",
            Some(&fixture.runner_a),
            &agentsandbox::model::Event::new("command.completed"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }
    let before = fixture.a.integrity().unwrap();
    assert_eq!(before.event_count, 56);
    let response = json_request(
        &fixture.router,
        Method::POST,
        &format!("/api/v1/workers/{}/transitions", worker.worker_id),
        Some(&fixture.runner_a),
        &mutation,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let replay: MutationResultV1 = response_json(response).await;
    assert_eq!(replay.receipt, accepted.receipt);
    assert_eq!(fixture.a.integrity().unwrap().head_hash, before.head_hash);

    // This transition is valid from Preparing with quota held. CreateIssued
    // would first require import/verification receipts and yield a conflict,
    // hiding the capacity oracle before the storage admission check.
    assert_eq!(accepted.worker.state, WorkerStateV1::Preparing);
    assert!(accepted.worker.quota_held);
    let refused_mutation = WorkerMutationV1 {
        operation_id: Uuid::new_v4().to_string(),
        expected_revision: accepted.worker.revision,
        transition: WorkerTransitionV1::Unknown {
            reason: "bounded audit capacity fixture".into(),
        },
        ..mutation
    };
    let response = json_request(
        &fixture.router,
        Method::POST,
        &format!("/api/v1/workers/{}/transitions", worker.worker_id),
        Some(&fixture.runner_a),
        &refused_mutation,
    )
    .await;
    let status = response.status();
    let body: Value = response_json(response).await;
    let after = fixture.a.integrity().unwrap();
    assert_eq!(
        (after.event_count, &after.head_hash),
        (before.event_count, &before.head_hash)
    );
    assert_eq!(
        status,
        StatusCode::TOO_MANY_REQUESTS,
        "unexpected control refusal: {body}"
    );
    assert_eq!(body["error"]["code"], "audit_write_budget_exhausted");
    assert_eq!(body["error"]["retry_action"], "contact_operator");
    assert!(
        fixture
            .a
            .worker_operation(&refused_mutation.operation_id)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        fixture
            .a
            .worker(&worker.worker_id)
            .unwrap()
            .unwrap()
            .revision,
        accepted.worker.revision
    );
    let revocation = fixture
        .a
        .mutate_identity(&account_change(1, false))
        .unwrap();
    assert_eq!(revocation.audit_seq as u64, before.event_count + 1);
    assert!(!fixture.a.human_account(&principal).unwrap().unwrap().active);
}

#[tokio::test]
async fn supervised_api_arms_import_and_worker_claims_once_then_maintenance_blocks_new_effects() {
    let fixture = Fixture::new();
    let admission_a = supervised(admission(&fixture.a));
    fixture.assert_envelope_excludes_control_credentials(
        &json!({"action":"admission","request":&admission_a}),
    );
    let response = json_request(
        &fixture.router,
        Method::POST,
        "/api/v1/workers/supervised-reservations",
        Some(&fixture.runner_a),
        &admission_a,
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let reserved: AdmissionResultV1 = response_json(response).await;
    assert!(reserved.accepted && !reserved.replayed);
    let mut worker = reserved.worker.clone().unwrap();
    let admission_receipt = reserved.receipt.clone();

    worker = transition(
        &fixture.router,
        &fixture.runner_a,
        &worker,
        WorkerTransitionV1::Prepare,
    )
    .await;
    let import_id = Uuid::new_v4();
    let import_arm = helper_arm(
        &worker,
        HelperPurposeV1::Import,
        import_id,
        HelperPermissionV1::Admission {
            receipt: admission_receipt,
        },
        "1".repeat(64).as_str(),
    );
    let import_policy =
        helper_execution_policy_v1(&worker, HelperPurposeV1::Import, import_id).unwrap();
    assert_eq!(import_arm.inspection.stable_name, import_policy.stable_name);
    assert_eq!(
        import_arm.inspection.spec_hash,
        import_policy.spec_hash().unwrap()
    );

    let head_before_bad_path = fixture.a.head().unwrap();
    let wrong_path = format!("/api/v1/workers/{WORKER_PATH_ID}/arms");
    let wrong_path_response = json_request(
        &fixture.router,
        Method::POST,
        &wrong_path,
        Some(&fixture.runner_a),
        &import_arm,
    )
    .await;
    assert_eq!(wrong_path_response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(fixture.a.head().unwrap(), head_before_bad_path);

    let import_path = format!("/api/v1/workers/{}/arms", worker.worker_id);
    let import_arm_response = json_request(
        &fixture.router,
        Method::POST,
        &import_path,
        Some(&fixture.runner_a),
        &import_arm,
    )
    .await;
    assert_eq!(import_arm_response.status(), StatusCode::OK);
    let import_arm_result: ArmResultV1 = response_json(import_arm_response).await;
    let import_lookup = format!(
        "/api/v1/workers/{}/arms/{}",
        worker.worker_id, import_arm.operation_id
    );
    let imported_read = send(
        &fixture.router,
        Method::GET,
        &import_lookup,
        Some(&fixture.viewer_a),
        None,
    )
    .await;
    assert_eq!(imported_read.status(), StatusCode::OK);
    let imported_read: ArmResultV1 = response_json(imported_read).await;
    assert_eq!(
        imported_read.receipt.audit_hash,
        import_arm_result.receipt.audit_hash
    );

    let mut import_claim = HelperStartClaimV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        helper_id: import_id.to_string(),
        emitter_id: Uuid::new_v4().to_string(),
        arm_receipt: import_arm_result.receipt.clone(),
        clock_check: host_clock(100_000_000_001),
    };
    let before_forgery = fixture.heads();
    import_claim.arm_receipt.control_namespace_uuid = fixture
        .b
        .supervision_state()
        .unwrap()
        .control_namespace_uuid
        .unwrap();
    let forged = json_request(
        &fixture.router,
        Method::POST,
        &format!("/api/v1/workers/{}/helper-start-claims", worker.worker_id),
        Some(&fixture.runner_a),
        &import_claim,
    )
    .await;
    assert_eq!(forged.status(), StatusCode::BAD_REQUEST);
    assert_eq!(fixture.heads(), before_forgery);
    import_claim.arm_receipt = import_arm_result.receipt.clone();

    // Authenticated tenant B cannot read tenant A's durable arm or claim it.
    let cross_tenant_read = send(
        &fixture.router,
        Method::GET,
        &import_lookup,
        Some(&fixture.viewer_b),
        None,
    )
    .await;
    assert_eq!(cross_tenant_read.status(), StatusCode::NOT_FOUND);
    let before_cross_tenant = fixture.heads();
    let cross_tenant_claim = json_request(
        &fixture.router,
        Method::POST,
        &format!("/api/v1/workers/{}/helper-start-claims", worker.worker_id),
        Some(&fixture.runner_b),
        &import_claim,
    )
    .await;
    assert_eq!(cross_tenant_claim.status(), StatusCode::NOT_FOUND);
    assert_eq!(fixture.heads(), before_cross_tenant);

    let helper_claim_path = format!("/api/v1/workers/{}/helper-start-claims", worker.worker_id);
    let helper_claim_response = json_request(
        &fixture.router,
        Method::POST,
        &helper_claim_path,
        Some(&fixture.runner_a),
        &import_claim,
    )
    .await;
    assert_eq!(helper_claim_response.status(), StatusCode::OK);
    let helper_claim_result: HelperClaimResultV1 = response_json(helper_claim_response).await;
    assert_eq!(
        helper_claim_result.disposition,
        ClaimDispositionV1::EmitOnce
    );
    let helper_replay = json_request(
        &fixture.router,
        Method::POST,
        &helper_claim_path,
        Some(&fixture.runner_a),
        &import_claim,
    )
    .await;
    let helper_replay: HelperClaimResultV1 = response_json(helper_replay).await;
    assert_eq!(
        helper_replay.disposition,
        ClaimDispositionV1::AlreadyConsumed
    );

    let import_observation = helper_observation(&import_arm_result, import_id, None);
    let observation_response = json_request(
        &fixture.router,
        Method::POST,
        &format!("/api/v1/workers/{}/helper-observations", worker.worker_id),
        Some(&fixture.runner_a),
        &import_observation,
    )
    .await;
    assert_eq!(observation_response.status(), StatusCode::OK);
    let import_terminal: MutationReceiptV1 = response_json(observation_response).await;
    let cleanup = HelperCleanupV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        helper_id: import_id.to_string(),
        arm_receipt: import_arm_result.receipt.clone(),
        removed: true,
        error: None,
    };
    let cleanup_response = json_request(
        &fixture.router,
        Method::POST,
        &format!("/api/v1/workers/{}/helper-cleanup", worker.worker_id),
        Some(&fixture.runner_a),
        &cleanup,
    )
    .await;
    assert_eq!(cleanup_response.status(), StatusCode::OK);
    let _: MutationReceiptV1 = response_json(cleanup_response).await;

    let verify_id = Uuid::new_v4();
    let verify_arm = helper_arm(
        &worker,
        HelperPurposeV1::ImportVerification,
        verify_id,
        HelperPermissionV1::ImportVerification {
            import_helper_id: import_id.to_string(),
            import_arm: import_arm_result.receipt.clone(),
            import_terminal,
            manifest_digest: worker.launch.workspace_manifest_digest.clone(),
        },
        "2".repeat(64).as_str(),
    );
    let verify_response = json_request(
        &fixture.router,
        Method::POST,
        &import_path,
        Some(&fixture.runner_a),
        &verify_arm,
    )
    .await;
    assert_eq!(verify_response.status(), StatusCode::OK);
    let verify_result: ArmResultV1 = response_json(verify_response).await;
    let verify_claim = HelperStartClaimV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        helper_id: verify_id.to_string(),
        emitter_id: Uuid::new_v4().to_string(),
        arm_receipt: verify_result.receipt.clone(),
        clock_check: host_clock(100_000_000_001),
    };
    let verify_claim_response = json_request(
        &fixture.router,
        Method::POST,
        &helper_claim_path,
        Some(&fixture.runner_a),
        &verify_claim,
    )
    .await;
    let verify_claim_result: HelperClaimResultV1 = response_json(verify_claim_response).await;
    assert_eq!(
        verify_claim_result.disposition,
        ClaimDispositionV1::EmitOnce
    );
    let verification = Some(ImportVerificationReportV1 {
        import_helper_id: import_id.to_string(),
        expected_manifest_digest: worker.launch.workspace_manifest_digest.clone(),
        observed_manifest_digest: worker.launch.workspace_manifest_digest.clone(),
    });
    let verify_observation = helper_observation(&verify_result, verify_id, verification);
    let verify_observation_response = json_request(
        &fixture.router,
        Method::POST,
        &format!("/api/v1/workers/{}/helper-observations", worker.worker_id),
        Some(&fixture.runner_a),
        &verify_observation,
    )
    .await;
    assert_eq!(verify_observation_response.status(), StatusCode::OK);
    let _: MutationReceiptV1 = response_json(verify_observation_response).await;

    worker = transition(
        &fixture.router,
        &fixture.runner_a,
        &worker,
        WorkerTransitionV1::CreateIssued,
    )
    .await;
    let created_inspection = worker_inspection(&worker);
    worker = transition(
        &fixture.router,
        &fixture.runner_a,
        &worker,
        WorkerTransitionV1::Created {
            inspection: created_inspection.clone(),
        },
    )
    .await;

    let worker_arm = ArmV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        expected_revision: worker.revision,
        intent_hash: HASH_C.into(),
        subject: ArmSubjectV1::Worker,
        inspection: created_inspection.clone(),
        authority_binding_hash: HASH_A.into(),
        configuration_fingerprint_version: 2,
        configuration_fingerprint: HASH_B.into(),
        clock: host_clock(100_000_000_000),
        timeout_seconds: worker.launch.timeout_seconds,
        deadline_ns_decimal: (100_000_000_000_u128
            + u128::from(worker.launch.timeout_seconds) * 1_000_000_000)
            .to_string(),
    };
    let worker_arm_path = format!("/api/v1/workers/{}/arms", worker.worker_id);
    let worker_arm_response = json_request(
        &fixture.router,
        Method::POST,
        &worker_arm_path,
        Some(&fixture.runner_a),
        &worker_arm,
    )
    .await;
    assert_eq!(worker_arm_response.status(), StatusCode::OK);
    let worker_arm_result: ArmResultV1 = response_json(worker_arm_response).await;
    let worker_claim_path = format!("/api/v1/workers/{}/armed-start-claims", worker.worker_id);
    let claim = ArmedStartClaimV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        expected_revision: worker.revision,
        emitter_id: Uuid::new_v4().to_string(),
        arm_receipt: worker_arm_result.receipt.clone(),
        clock_check: host_clock(100_000_000_001),
    };
    let claim_response = json_request(
        &fixture.router,
        Method::POST,
        &worker_claim_path,
        Some(&fixture.runner_a),
        &claim,
    )
    .await;
    assert_eq!(claim_response.status(), StatusCode::OK);
    let claimed: MutationResultV1 = response_json(claim_response).await;
    assert_eq!(claimed.disposition, Some(ClaimDispositionV1::EmitOnce));
    let replay_response = json_request(
        &fixture.router,
        Method::POST,
        &worker_claim_path,
        Some(&fixture.runner_a),
        &claim,
    )
    .await;
    let replayed: MutationResultV1 = response_json(replay_response).await;
    assert_eq!(
        replayed.disposition,
        Some(ClaimDispositionV1::AlreadyConsumed)
    );
    worker = replayed.worker;

    worker = transition(
        &fixture.router,
        &fixture.runner_a,
        &worker,
        WorkerTransitionV1::Observe {
            observation: worker_observation(
                &created_inspection,
                RuntimeStatusV1::Running,
                true,
                None,
                None,
            ),
        },
    )
    .await;
    worker = transition(
        &fixture.router,
        &fixture.runner_a,
        &worker,
        WorkerTransitionV1::Stop,
    )
    .await;
    worker = transition(
        &fixture.router,
        &fixture.runner_a,
        &worker,
        WorkerTransitionV1::Observe {
            observation: worker_observation(
                &created_inspection,
                RuntimeStatusV1::Exited,
                false,
                Some("2026-10-30T12:00:01Z"),
                Some(0),
            ),
        },
    )
    .await;
    assert_eq!(worker.state, WorkerStateV1::Terminal);
    let export = ExportRequestV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        destination_ref: Uuid::new_v4().to_string(),
        destination_intent_hash: HASH_B.into(),
    };
    let export_path = format!("/api/v1/workers/{}/export-requests", worker.worker_id);
    let export_response = json_request(
        &fixture.router,
        Method::POST,
        &export_path,
        Some(&fixture.runner_a),
        &export,
    )
    .await;
    assert_eq!(export_response.status(), StatusCode::OK);
    let export_receipt: MutationReceiptV1 = response_json(export_response).await;

    // An independent open is maintenance-only. Reads and exact consumed
    // replay remain available, while fresh operations are refused atomically.
    let reopened = Arc::new(
        Store::open_for_tenant(
            &fixture._directory.path().join("a.sqlite"),
            "tenant-a",
            Redactor::new(vec![]),
            10_000,
        )
        .unwrap(),
    );
    assert!(!reopened.supervision_state().unwrap().active);
    let reopened_state = GatewayState::for_tenants(
        vec![TenantGateway {
            tenant_id: "tenant-a".into(),
            store: Arc::clone(&reopened),
            runner_token: fixture.runner_a.clone(),
            viewer_token: fixture.viewer_a.clone(),
        }],
        Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .unwrap();
    let reopened_router = gateway_router(reopened_state);
    let reopened_protocol = send(
        &reopened_router,
        Method::GET,
        "/api/v1/control/protocol",
        Some(&fixture.viewer_a),
        None,
    )
    .await;
    let reopened_protocol: agentsandbox::control::ControlProtocolV1 =
        response_json(reopened_protocol).await;
    assert!(!reopened_protocol.active);
    let historical_arm = send(
        &reopened_router,
        Method::GET,
        &format!(
            "/api/v1/workers/{}/arms/{}",
            worker.worker_id, worker_arm.operation_id
        ),
        Some(&fixture.viewer_a),
        None,
    )
    .await;
    assert_eq!(historical_arm.status(), StatusCode::OK);
    let historical_arm: ArmResultV1 = response_json(historical_arm).await;
    assert_eq!(
        historical_arm.receipt.audit_hash,
        worker_arm_result.receipt.audit_hash
    );
    let historical_replay = json_request(
        &reopened_router,
        Method::POST,
        &worker_claim_path,
        Some(&fixture.runner_a),
        &claim,
    )
    .await;
    assert_eq!(historical_replay.status(), StatusCode::OK);
    let historical_replay: MutationResultV1 = response_json(historical_replay).await;
    assert_eq!(
        historical_replay.disposition,
        Some(ClaimDispositionV1::AlreadyConsumed)
    );
    let mut fresh_claim = claim.clone();
    fresh_claim.operation_id = Uuid::new_v4().to_string();
    let reopened_head = reopened.head().unwrap();
    let refused = json_request(
        &reopened_router,
        Method::POST,
        &worker_claim_path,
        Some(&fixture.runner_a),
        &fresh_claim,
    )
    .await;
    assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
    let refused: Value = response_json(refused).await;
    assert_eq!(refused["code"], "supervision_inactive");
    assert_eq!(reopened.head().unwrap(), reopened_head);

    let mut fresh_reservation = admission(&reopened);
    fresh_reservation.operation_id = Uuid::new_v4().to_string();
    let reopened_reservation_head = reopened.head().unwrap();
    let refused_reservation = json_request(
        &reopened_router,
        Method::POST,
        "/api/v1/workers/supervised-reservations",
        Some(&fixture.runner_a),
        &supervised(fresh_reservation),
    )
    .await;
    assert_eq!(
        refused_reservation.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(reopened.head().unwrap(), reopened_reservation_head);
    assert_eq!(fixture.a.head().unwrap(), reopened_reservation_head);
    let _ = export_receipt;
}

#[tokio::test]
async fn jev_transport_is_skipped_for_inactive_fresh_admission_and_exact_replay() {
    let fixture = Fixture::new();
    let counter = LocalConnectionCounter::new();
    let endpoint = format!("https://{}/v1/systemone", counter.address());
    let jev = JevClient::new(
        &endpoint,
        "qa-jev-probe-key-never-real".into(),
        "jev-latest".into(),
        0.8,
        false,
    )
    .unwrap();

    // Prove the listener observes a real JevClient transport attempt, without
    // providing TLS or a model response. The QA credential stays in memory.
    let probe = admission(&fixture.a).decision_request;
    assert!(jev.decide(&probe, &Redactor::new(vec![])).await.is_err());
    counter.wait_for(1);
    assert_eq!(counter.count(), 1);
    counter.reset();

    let admitted_request = supervised(admission(&fixture.a));
    let admitted = json_request(
        &fixture.router,
        Method::POST,
        "/api/v1/workers/supervised-reservations",
        Some(&fixture.runner_a),
        &admitted_request,
    )
    .await;
    assert_eq!(admitted.status(), StatusCode::CREATED);
    let admitted: AdmissionResultV1 = response_json(admitted).await;
    assert!(admitted.accepted && !admitted.replayed);

    // The independently opened Store is maintenance-only. Build a gateway
    // with Jev enabled around it to exercise both replay and fresh denial.
    let reopened = Arc::new(
        Store::open_for_tenant(
            &fixture._directory.path().join("a.sqlite"),
            "tenant-a",
            Redactor::new(vec![]),
            10_000,
        )
        .unwrap(),
    );
    let jev_router = gateway_router(
        GatewayState::for_tenants(
            vec![TenantGateway {
                tenant_id: "tenant-a".into(),
                store: Arc::clone(&reopened),
                runner_token: fixture.runner_a.clone(),
                viewer_token: fixture.viewer_a.clone(),
            }],
            Policy::default(),
            Some(jev),
            Redactor::new(vec![]),
        )
        .unwrap(),
    );

    let exact_replay = json_request(
        &jev_router,
        Method::POST,
        "/api/v1/workers/supervised-reservations",
        Some(&fixture.runner_a),
        &admitted_request,
    )
    .await;
    assert_eq!(exact_replay.status(), StatusCode::OK);
    let exact_replay: AdmissionResultV1 = response_json(exact_replay).await;
    assert!(exact_replay.accepted && exact_replay.replayed);
    assert_eq!(
        exact_replay.worker.unwrap().worker_id,
        admitted.worker.unwrap().worker_id
    );

    let mut fresh = admission(&reopened);
    fresh.operation_id = Uuid::new_v4().to_string();
    let inactive_head = reopened.head().unwrap();
    let refused = json_request(
        &jev_router,
        Method::POST,
        "/api/v1/workers/supervised-reservations",
        Some(&fixture.runner_a),
        &supervised(fresh),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
    let refused: Value = response_json(refused).await;
    assert_eq!(refused["code"], "supervision_inactive");
    assert_eq!(reopened.head().unwrap(), inactive_head);
    thread::sleep(Duration::from_millis(50));
    assert_eq!(counter.count(), 0, "refusal or replay contacted Jev");
}

struct LocalConnectionCounter {
    address: std::net::SocketAddr,
    count: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl LocalConnectionCounter {
    fn new() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_count = Arc::clone(&count);
        let thread_stop = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((_stream, _peer)) => {
                        thread_count.fetch_add(1, Ordering::SeqCst);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            address,
            count,
            stop,
            thread: Some(thread),
        }
    }

    fn address(&self) -> std::net::SocketAddr {
        self.address
    }

    fn count(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }

    fn reset(&self) {
        self.count.store(0, Ordering::SeqCst);
    }

    fn wait_for(&self, expected: usize) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.count() < expected {
            assert!(
                Instant::now() < deadline,
                "local listener saw no Jev attempt"
            );
            thread::sleep(Duration::from_millis(2));
        }
    }
}

impl Drop for LocalConnectionCounter {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
