//! Reuse the existing real supervision envelopes; no engine is started.
use super::*;
use agentsandbox::api::gateway_control_router;
use axum::routing::any;
use tokio::sync::{oneshot, watch};

const GUARD: Duration = Duration::from_secs(30);

struct Server {
    url: String,
    stop: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl Server {
    async fn new(router: Router) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
        });
        Self {
            url,
            stop: Some(stop),
            task: Some(task),
        }
    }
    async fn shutdown(mut self) {
        let _ = self.stop.take().unwrap().send(());
        let mut task = self.task.take().unwrap();
        match tokio::time::timeout(GUARD, &mut task).await {
            Ok(result) => result.unwrap(),
            Err(_) => {
                task.abort();
                let _ = task.await;
                panic!("HARNESS: owned loopback server did not drain");
            }
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
struct Release(watch::Sender<bool>);
impl Drop for Release {
    fn drop(&mut self) {
        let _ = self.0.send(true);
    }
}

fn current_fixture() -> Fixture {
    let fixture = Fixture::new();
    for (store, id) in [(&fixture.a, "a"), (&fixture.b, "b")] {
        let backup = |version| {
            fixture
                ._directory
                .path()
                .join(format!("h-{id}-before{version}"))
        };
        store.migrate_identity_v4(&backup(4)).unwrap();
        store.migrate_human_access_v5(&backup(5)).unwrap();
        store.migrate_workload_v6(&backup(6)).unwrap();
        store.migrate_human_brokers_v7(&backup(7)).unwrap();
        store.migrate_kubernetes_create_v8(&backup(8)).unwrap();
        store.migrate_kubernetes_policy_v9(&backup(9)).unwrap();
        store.migrate_human_workers_v10(&backup(10)).unwrap();
        assert_eq!(store.supervision_state().unwrap().schema_version, 10);
    }
    fixture
}

#[tokio::test]
#[ignore = "requires the atomic broker-side named admission rail"]
async fn legacy_named_admission_requires_a_reference_and_preserves_manual_replay() {
    use agentsandbox::{
        broker_registry::{BrokerAuthorityV1, BrokerGrantMutationV1, BrokerRuntimeKindV1},
        identity::{ResourceKindV1, ResourceRefV1},
        workload::{WorkloadBindingIssueV1, WorkloadSubjectV1},
    };
    use sha2::{Digest, Sha256};

    // Proposed V1 private target for the named rail. Its discriminator is
    // the route, so the broker role can be checked before parsing the body.
    const NAMED: &str = "/control/api/v1/brokers/session-admissions";
    const PUBLIC: &str = "/api/v1/brokers/session-admissions";
    const MANUAL: &str = "/control/api/v1/workers/supervised-reservations";
    let fixture = current_fixture();
    let directory = fixture._directory.path();
    fixture
        .a
        .migrate_human_rights_v2(&directory.join("named-before-human-rights.sqlite"))
        .unwrap();
    let broker_proof = "fixture-g04-named-admission-broker".repeat(2);
    let broker_id = Uuid::new_v4().to_string();
    let incarnation_id = Uuid::new_v4().to_string();
    let credential_id = Uuid::new_v4().to_string();
    let authority_id = Uuid::new_v4().to_string();
    fixture
        .a
        .issue_workload_binding(&WorkloadBindingIssueV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            credential_id: credential_id.clone(),
            expected_revision: 0,
            subject: WorkloadSubjectV1::Broker {
                broker_id: broker_id.clone(),
                incarnation_id: incarnation_id.clone(),
            },
            sha256_digest: hex::encode(Sha256::digest(broker_proof.as_bytes())),
            expires_at: "2100-01-01T00:00:00.000Z".into(),
        })
        .unwrap();
    fixture
        .a
        .mutate_broker_grant(&BrokerGrantMutationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            broker_id: broker_id.clone(),
            expected_revision: None,
            credential_id,
            incarnation_id,
            authorities: vec![BrokerAuthorityV1 {
                authority_id: authority_id.clone(),
                runtime_kind: BrokerRuntimeKindV1::Docker,
            }],
            lease_seconds: 120,
            not_after: "2100-01-01T00:00:00.000Z".into(),
        })
        .unwrap();
    let identity = send(
        &fixture.router,
        Method::GET,
        "/control/api/v1/brokers/identity",
        Some(&broker_proof),
        None,
    )
    .await;
    assert_eq!(identity.status(), StatusCode::OK, "broker proof fixture");
    let identity: Value = response_json(identity).await;
    assert_eq!(identity["broker_id"], broker_id);
    assert_eq!(identity["tenant_id"], "tenant-a");
    assert_eq!(identity["schema_version"], 10);

    let mut named = supervised(admission(&fixture.a));
    named.intent_hash = HASH_A.into();
    named.admission.launch.authority_id = authority_id;
    named.validate().unwrap();
    let resource = ResourceRefV1 {
        kind: ResourceKindV1::Session,
        resource_id: named.admission.launch.session_id.clone(),
    };
    let operation = named.admission.operation_id.clone();
    // The fixture deliberately retains no Prepare record. This first oracle
    // exercises missing-reference refusal on the explicitly named rail only.
    let missing_reference = json!({ "version": 1, "request": named });
    let manual = supervised(admission(&fixture.a));
    let before_refusals = fixture.a.head().unwrap();
    let private = json_request(
        &fixture.router,
        Method::POST,
        NAMED,
        Some(&broker_proof),
        &missing_reference,
    )
    .await
    .status();
    let public = json_request(
        &fixture.router,
        Method::POST,
        PUBLIC,
        Some(&broker_proof),
        &missing_reference,
    )
    .await
    .status();
    let (runner, polls) =
        counted_post(&fixture.router, NAMED, Some(bearer(&fixture.runner_a))).await;
    assert_eq!(fixture.a.head().unwrap(), before_refusals);
    assert!(fixture.a.workers().unwrap().is_empty());

    let acceptance = json_request(
        &fixture.router,
        Method::POST,
        MANUAL,
        Some(&fixture.runner_a),
        &manual,
    )
    .await;
    assert_eq!(acceptance.status(), StatusCode::CREATED, "manual canary");
    let acceptance: AdmissionResultV1 = response_json(acceptance).await;
    assert!(acceptance.accepted && !acceptance.replayed);
    let worker_id = acceptance.worker.as_ref().unwrap().worker_id.clone();
    let expected_head = fixture.a.head().unwrap();
    let Fixture {
        _directory: directory,
        router,
        a,
        b,
        v2,
        runner_a,
        viewer_a,
        ..
    } = fixture;
    drop((router, a, b, v2));
    let redactor = Redactor::new(vec![]);
    let reopened = Arc::new(
        Store::open_for_tenant(
            &directory.path().join("a.sqlite"),
            "tenant-a",
            redactor.clone(),
            10_000,
        )
        .unwrap(),
    );
    assert!(reopened.integrity().unwrap().valid);
    let router = gateway_router(
        GatewayState::for_tenants(
            vec![TenantGateway {
                tenant_id: "tenant-a".into(),
                store: reopened.clone(),
                runner_token: runner_a.clone(),
                viewer_token: viewer_a,
            }],
            Policy::default(),
            None,
            redactor,
        )
        .unwrap(),
    );
    let replay = json_request(&router, Method::POST, MANUAL, Some(&runner_a), &manual).await;
    assert_eq!(replay.status(), StatusCode::OK);
    let replay: AdmissionResultV1 = response_json(replay).await;
    assert!(replay.accepted && replay.replayed);
    assert_eq!(replay.receipt, acceptance.receipt);
    assert_eq!(replay.worker.unwrap().worker_id, worker_id);
    let reopened_private = json_request(
        &router,
        Method::POST,
        NAMED,
        Some(&broker_proof),
        &missing_reference,
    )
    .await
    .status();
    assert!(reopened.worker_operation(&operation).unwrap().is_none());
    assert!(
        reopened
            .identity_resource_grant(&resource)
            .unwrap()
            .is_none()
    );
    let workers = reopened.workers().unwrap();
    assert_eq!(workers.len(), 1);
    assert_eq!(workers[0].worker_id, worker_id);
    assert_eq!(workers[0].state, WorkerStateV1::Reserved);
    assert_eq!(
        reopened
            .worker_quota(&agentsandbox::control::QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        1
    );
    assert_eq!(reopened.head().unwrap(), expected_head);

    assert_eq!(
        private,
        StatusCode::BAD_REQUEST,
        "named V1 requires its reference"
    );
    assert_eq!(reopened_private, private);
    assert_eq!(public, StatusCode::NOT_FOUND, "named V1 stays private");
    assert!(matches!(
        runner,
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ));
    assert_eq!(polls, 0, "runner is refused before the body");
}

fn rejection_code(payload: &Value) -> &'static str {
    match payload
        .get("code")
        .or_else(|| payload.get("error").and_then(|error| error.get("code")))
        .and_then(Value::as_str)
    {
        Some("worker_conflict") => "worker_conflict",
        Some("supervision_inactive") => "supervision_inactive",
        Some("invalid_worker_request") => "invalid_worker_request",
        Some("migration_required") => "migration_required",
        Some("workload_forbidden") => "workload_forbidden",
        Some("authentication_required") => "authentication_required",
        Some("identity_unavailable") => "identity_unavailable",
        Some("control_unavailable") => "control_unavailable",
        _ => "unclassified",
    }
}

async fn reserve(
    fixture: &Fixture,
    router: &Router,
    intent_hash: &str,
    phase: &'static str,
) -> AdmissionResultV1 {
    let mut request = supervised(admission(&fixture.a));
    request.intent_hash = intent_hash.into();
    request.validate().unwrap();
    let response = json_request(
        router,
        Method::POST,
        "/control/api/v1/workers/supervised-reservations",
        Some(&fixture.runner_a),
        &request,
    )
    .await;
    let status = response.status();
    let payload: Value = response_json(response).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "HARNESS {phase}: code={}",
        rejection_code(&payload)
    );
    serde_json::from_value(payload).unwrap()
}

fn mutate(store: &Store, worker: &WorkerV1, transition: WorkerTransitionV1) -> WorkerV1 {
    store
        .mutate_worker(&WorkerMutationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            worker_id: worker.worker_id.clone(),
            attempt_id: worker.attempt.attempt_id.clone(),
            expected_revision: worker.revision,
            transition,
        })
        .expect("HARNESS: public Store worker transition refused")
        .worker
}

fn complete_helper(
    store: &Store,
    arm: &ArmV1,
    helper_id: Uuid,
    verification: Option<ImportVerificationReportV1>,
) -> (ArmResultV1, MutationReceiptV1) {
    let armed = store
        .arm_execution(arm)
        .expect("HARNESS: public Store helper arm refused");
    let claimed = store
        .claim_helper_start(&HelperStartClaimV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            helper_id: helper_id.to_string(),
            emitter_id: Uuid::new_v4().to_string(),
            arm_receipt: armed.receipt.clone(),
            clock_check: host_clock(100_000_000_001),
        })
        .expect("HARNESS: public Store helper claim refused");
    assert_eq!(claimed.disposition, ClaimDispositionV1::EmitOnce);
    let terminal = store
        .observe_helper(&helper_observation(&armed, helper_id, verification))
        .expect("HARNESS: public Store helper observation refused");
    store
        .record_helper_cleanup(&HelperCleanupV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            helper_id: helper_id.to_string(),
            arm_receipt: armed.receipt.clone(),
            removed: true,
            error: None,
        })
        .expect("HARNESS: public Store helper cleanup refused");
    (armed, terminal)
}

/// The same public supervised sequence as the existing positive API contract,
/// on an origin activated before any worker exists. All runtime facts remain
/// synthetic; consuming a real Store claim never asserts that a process ran.
fn running_supervised_worker(store: &Store) -> WorkerV1 {
    let supervision = store.supervision_state().unwrap();
    assert_eq!(supervision.schema_version, 10);
    assert!(
        supervision.active,
        "HARNESS: a migrated legacy worker cannot create live authority"
    );
    let request = supervised(admission(store));
    let policy = Policy::default();
    let reserved = store
        .admit_supervised(
            &request,
            &policy,
            policy::evaluate(&policy, &request.admission.decision_request),
            Value::Null,
            &agentsandbox::control::QuotaPolicyV1::default(),
        )
        .expect("HARNESS: public Store supervised admission refused");
    assert!(reserved.accepted);
    let mut worker = mutate(
        store,
        &reserved.worker.unwrap(),
        WorkerTransitionV1::Prepare,
    );
    let import_id = Uuid::new_v4();
    let arm = helper_arm(
        &worker,
        HelperPurposeV1::Import,
        import_id,
        HelperPermissionV1::Admission {
            receipt: reserved.receipt,
        },
        &"1".repeat(64),
    );
    let (imported, import_terminal) = complete_helper(store, &arm, import_id, None);
    let verify_id = Uuid::new_v4();
    let arm = helper_arm(
        &worker,
        HelperPurposeV1::ImportVerification,
        verify_id,
        HelperPermissionV1::ImportVerification {
            import_helper_id: import_id.to_string(),
            import_arm: imported.receipt,
            import_terminal,
            manifest_digest: worker.launch.workspace_manifest_digest.clone(),
        },
        &"2".repeat(64),
    );
    complete_helper(
        store,
        &arm,
        verify_id,
        Some(ImportVerificationReportV1 {
            import_helper_id: import_id.to_string(),
            expected_manifest_digest: worker.launch.workspace_manifest_digest.clone(),
            observed_manifest_digest: worker.launch.workspace_manifest_digest.clone(),
        }),
    );
    worker = mutate(store, &worker, WorkerTransitionV1::CreateIssued);
    let inspection = worker_inspection(&worker);
    worker = mutate(
        store,
        &worker,
        WorkerTransitionV1::Created {
            inspection: inspection.clone(),
        },
    );
    let armed = store
        .arm_execution(&ArmV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            worker_id: worker.worker_id.clone(),
            attempt_id: worker.attempt.attempt_id.clone(),
            expected_revision: worker.revision,
            intent_hash: HASH_C.into(),
            subject: ArmSubjectV1::Worker,
            inspection: inspection.clone(),
            authority_binding_hash: HASH_A.into(),
            configuration_fingerprint_version: 2,
            configuration_fingerprint: HASH_B.into(),
            clock: host_clock(100_000_000_000),
            timeout_seconds: worker.launch.timeout_seconds,
            deadline_ns_decimal: (100_000_000_000_u128
                + u128::from(worker.launch.timeout_seconds) * 1_000_000_000)
                .to_string(),
        })
        .expect("HARNESS: public Store worker arm refused");
    let claimed = store
        .claim_armed_start(&ArmedStartClaimV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            worker_id: worker.worker_id.clone(),
            attempt_id: worker.attempt.attempt_id.clone(),
            expected_revision: worker.revision,
            emitter_id: Uuid::new_v4().to_string(),
            arm_receipt: armed.receipt,
            clock_check: host_clock(100_000_000_001),
        })
        .expect("HARNESS: public Store armed worker claim refused");
    assert_eq!(claimed.disposition, Some(ClaimDispositionV1::EmitOnce));
    worker = mutate(
        store,
        &claimed.worker,
        WorkerTransitionV1::Observe {
            observation: worker_observation(
                &inspection,
                RuntimeStatusV1::Running,
                true,
                None,
                None,
            ),
        },
    );
    assert_eq!(worker.state, WorkerStateV1::Running);
    worker
}

async fn http_post<T: DeserializeOwned>(
    client: &reqwest::Client,
    origin: &str,
    token: &str,
    path: &str,
    body: &impl Serialize,
) -> T {
    let response = client
        .post(format!("{origin}/control{path}"))
        .bearer_auth(token)
        .json(body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.json().await.unwrap()
}

#[tokio::test]
async fn real_worker_proof_cannot_consume_runner_or_broker_control_bodies() {
    use agentsandbox::workload::{WorkloadBindingIssueV1, WorkloadSubjectV1};
    use sha2::{Digest, Sha256};
    let fixture = current_fixture();
    let worker = running_supervised_worker(&fixture.a);
    let token = "fixture-running-worker-proof".repeat(3);
    fixture
        .a
        .issue_workload_binding(&WorkloadBindingIssueV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            credential_id: Uuid::new_v4().to_string(),
            expected_revision: 0,
            subject: WorkloadSubjectV1::Worker {
                worker_id: worker.worker_id,
                attempt_id: worker.attempt.attempt_id,
                generation: 1,
            },
            sha256_digest: hex::encode(Sha256::digest(token.as_bytes())),
            expires_at: "2100-01-01T00:00:00Z".into(),
        })
        .unwrap();
    // Positive proof through the existing public worker-only HTTP API. The
    // real VerifiedWorker extractor and Store revalidation must both succeed;
    // no private authentication helper or fabricated Bearer is sufficient.
    let proof_server = Server::new(fixture.router.clone()).await;
    let proof_client = reqwest::Client::builder()
        .no_proxy()
        .timeout(GUARD)
        .build()
        .unwrap();
    let proof = proof_client
        .get(format!(
            "{}/api/v1/relay/messages?limit=32",
            proof_server.url
        ))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    let status = proof.status();
    let payload: Value = proof.json().await.unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "HARNESS worker_inbox_proof: code={}",
        rejection_code(&payload)
    );
    let inbox: agentsandbox::relay::RelayInboxV1 = serde_json::from_value(payload).unwrap();
    assert_eq!(inbox.version, 1);
    assert!(inbox.items.is_empty());
    assert_eq!(inbox.next_after, None);
    drop(proof_client);
    proof_server.shutdown().await;
    let before = fixture.a.head().unwrap();
    for (path, expected) in [
        (
            "/control/api/v1/workers/not-a-uuid/transitions",
            StatusCode::UNAUTHORIZED,
        ),
        ("/control/api/v1/brokers/heartbeat", StatusCode::FORBIDDEN),
    ] {
        let (status, polls) = counted_post(&fixture.router, path, Some(bearer(&token))).await;
        assert_eq!(status, expected);
        assert_eq!(polls, 0);
    }
    assert_eq!(fixture.a.head().unwrap(), before);
    // Runtime facts above are synthetic Store inputs, not an observed process.
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_transition_and_claim_receipts_survive_both_public_budgets() {
    let fixture = current_fixture();
    let state = GatewayState::for_tenants(
        vec![
            TenantGateway {
                tenant_id: "tenant-a".into(),
                store: fixture.a.clone(),
                runner_token: fixture.runner_a.clone(),
                viewer_token: fixture.viewer_a.clone(),
            },
            TenantGateway {
                tenant_id: "tenant-b".into(),
                store: fixture.b.clone(),
                runner_token: fixture.runner_b.clone(),
                viewer_token: fixture.viewer_b.clone(),
            },
        ],
        Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .unwrap();
    let direct = gateway_router(state.clone());
    let transition_worker = reserve(&fixture, &direct, &"d".repeat(64), "reserve_transition")
        .await
        .worker
        .unwrap();
    let reserved_claim = reserve(&fixture, &direct, HASH_C, "reserve_claim").await;
    let claim_worker = transition(
        &direct,
        &fixture.runner_a,
        &reserved_claim.worker.unwrap(),
        WorkerTransitionV1::Prepare,
    )
    .await;
    let helper_id = Uuid::new_v4();
    let arm = helper_arm(
        &claim_worker,
        HelperPurposeV1::Import,
        helper_id,
        HelperPermissionV1::Admission {
            receipt: reserved_claim.receipt,
        },
        &"1".repeat(64),
    );
    let arm_response = json_request(
        &direct,
        Method::POST,
        &format!("/control/api/v1/workers/{}/arms", claim_worker.worker_id),
        Some(&fixture.runner_a),
        &arm,
    )
    .await;
    assert_eq!(arm_response.status(), StatusCode::OK);
    let armed: ArmResultV1 = response_json(arm_response).await;
    let claim = HelperStartClaimV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        helper_id: helper_id.to_string(),
        emitter_id: Uuid::new_v4().to_string(),
        arm_receipt: armed.receipt,
        clock_check: host_clock(100_000_000_001),
    };
    let control = Server::new(gateway_control_router(state.clone())).await;
    let (release, released) = watch::channel(false);
    let release = Release(release);
    // This upstream only drives the real public relay pool. It makes no claim
    // about an anonymous caller passing the real gateway's authorization.
    let public = Server::new(Router::new().fallback(any(move || {
        let mut released = released.clone();
        async move {
            let stream = async_stream::stream! {
                yield Ok::<_, std::io::Error>(Bytes::from_static(b"held"));
                while !*released.borrow_and_update() { if released.changed().await.is_err() { break; } }
            };
            axum::response::Response::new(Body::from_stream(stream))
        }
    }))).await;
    let relay =
        agentsandbox::ingress::ingress_router_with_control(&public.url, &control.url).unwrap();
    let published = Server::new(relay.clone()).await;
    let responses = futures_util::future::join_all((0..64).map(|index| {
        let relay = relay.clone();
        async move {
            let response = relay
                .oneshot(
                    Request::builder()
                        .uri(format!("/hold/{index}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            response
        }
    }))
    .await;
    let mut uploads = tokio::task::JoinSet::new();
    let (reading, mut entered) = tokio::sync::mpsc::unbounded_channel();
    for _ in 0..16 {
        let mut reading = Some(reading.clone());
        let stream = stream::poll_fn(move |_| {
            if let Some(reading) = reading.take() {
                let _ = reading.send(());
            }
            Poll::<Option<Result<Bytes, std::io::Error>>>::Pending
        });
        let relay = relay.clone();
        uploads.spawn(async move {
            relay
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/api/v1/events")
                        .body(Body::from_stream(stream))
                        .unwrap(),
                )
                .await
                .unwrap()
        });
    }
    drop(reading);
    tokio::time::timeout(GUARD, async {
        for _ in 0..16 {
            entered.recv().await.unwrap();
        }
    })
    .await
    .expect("HARNESS: sixteen upload admissions not observed");
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(GUARD)
        .build()
        .unwrap();
    let mutation = WorkerMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: transition_worker.worker_id.clone(),
        attempt_id: transition_worker.attempt.attempt_id.clone(),
        expected_revision: transition_worker.revision,
        transition: WorkerTransitionV1::Prepare,
    };
    assert!(
        uploads.try_join_next().is_none(),
        "HARNESS: upload hold expired before transition"
    );
    let transitioned: MutationResultV1 = http_post(
        &client,
        &published.url,
        &fixture.runner_a,
        &format!(
            "/api/v1/workers/{}/transitions",
            transition_worker.worker_id
        ),
        &mutation,
    )
    .await;
    assert!(
        uploads.try_join_next().is_none(),
        "HARNESS: upload hold expired during transition"
    );
    assert_eq!(transitioned.worker.state, WorkerStateV1::Preparing);
    assert_eq!(
        fixture.a.worker_operation(&mutation.operation_id).unwrap(),
        Some(transitioned.receipt.clone())
    );
    let claim_path = format!(
        "/api/v1/workers/{}/helper-start-claims",
        claim_worker.worker_id
    );
    let claimed: HelperClaimResultV1 = http_post(
        &client,
        &published.url,
        &fixture.runner_a,
        &claim_path,
        &claim,
    )
    .await;
    assert!(
        uploads.try_join_next().is_none(),
        "HARNESS: upload hold expired during claim"
    );
    assert_eq!(claimed.disposition, ClaimDispositionV1::EmitOnce);
    assert_eq!(
        fixture.a.worker_operation(&claim.operation_id).unwrap(),
        Some(claimed.receipt.clone())
    );
    let head = fixture.a.head().unwrap();
    let replayed: HelperClaimResultV1 = http_post(
        &client,
        &published.url,
        &fixture.runner_a,
        &claim_path,
        &claim,
    )
    .await;
    assert!(
        uploads.try_join_next().is_none(),
        "HARNESS: upload hold expired during claim replay"
    );
    assert_eq!(replayed.disposition, ClaimDispositionV1::AlreadyConsumed);
    assert_eq!(replayed.receipt, claimed.receipt);
    assert_eq!(fixture.a.head().unwrap(), head);
    let capacity_probe = relay
        .clone()
        .oneshot(
            Request::builder()
                .uri("/public-capacity-probe")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        capacity_probe.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "HARNESS: public exchange hold expired during business oracle"
    );
    uploads.abort_all();
    while uploads.join_next().await.is_some() {}
    drop(responses);
    drop(release);
    drop(client);
    drop(relay);
    published.shutdown().await;
    control.shutdown().await;
    public.shutdown().await;
    drop(direct);
    drop(state);
    let Fixture {
        _directory: directory,
        router,
        a,
        b,
        v2,
        ..
    } = fixture;
    let path = directory.path().join("a.sqlite");
    drop((router, a, b, v2));
    let reopened =
        Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 10_000).unwrap();
    assert!(reopened.integrity().unwrap().valid);
    assert_eq!(
        reopened.worker_operation(&mutation.operation_id).unwrap(),
        Some(transitioned.receipt)
    );
    assert_eq!(
        reopened.worker_operation(&claim.operation_id).unwrap(),
        Some(claimed.receipt)
    );
}
