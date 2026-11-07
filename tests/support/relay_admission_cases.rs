//! Synthetic runtime facts traverse the real supervision/SQLite/router gates.
//! No runtime, provider or external service is invoked.
use super::*;

async fn post(app: &Router, token: &str, path: &str, value: &Value) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(value).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn relay_admission_reserved_replays_and_revocation_blocks_claim_without_worker_terminal() {
    let directory = support::private_tempdir();
    let store = Arc::new(fresh(directory.path(), "tenant-a", 10000));
    let worker = running_now(&store);
    let token = "relay-admission-workload-fixture-token-0123456789";
    let (credential, revision) = bind(&store, &worker, token);
    let input = request(&store, &worker, &credential, revision, token);
    let app = gateway_router(
        GatewayState::for_tenants(
            vec![TenantGateway {
                tenant_id: "tenant-a".into(),
                store: store.clone(),
                runner_token: "r".repeat(48),
                viewer_token: "v".repeat(48),
            }],
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )
        .unwrap(),
    );
    let uri = "/control/api/v1/relay/channels/admissions";
    let (status, reserved) = post(&app, &"r".repeat(48), uri, &input).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "the typed admission route is absent: {reserved}"
    );
    assert_eq!(reserved["channel"]["state"], "reserved");
    let head = store.head().unwrap();
    let (status, replay) = post(&app, &"r".repeat(48), uri, &input).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay["replayed"], true);
    assert_eq!(head, store.head().unwrap());
    store
        .revoke_workload_binding(&WorkloadBindingRevocationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            credential_id: credential,
            expected_revision: revision,
        })
        .unwrap();
    let claim = json!({"version":1,"operation_id":Uuid::new_v4().to_string(),
        "channel_id":input["channel_id"],"epoch":1,"expected_revision":1,
        "admission_input_hash":reserved["channel"]["admission_input_hash"]});
    let claim_uri = format!(
        "/control/api/v1/relay/channels/{}/claims",
        input["channel_id"].as_str().unwrap()
    );
    assert_eq!(
        post(&app, &"r".repeat(48), &claim_uri, &claim).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        store.worker(&worker.worker_id).unwrap().unwrap().state,
        WorkerStateV1::Running
    );
    assert!(store.worker(&worker.worker_id).unwrap().unwrap().quota_held);
    // Cleanup of an unclaimed reservation remains available after revocation.
    let cancel = json!({"version":1,"operation_id":Uuid::new_v4().to_string(),
        "channel_id":input["channel_id"],"epoch":1,"expected_revision":1,
        "admission_input_hash":reserved["channel"]["admission_input_hash"],
        "transition":{"kind":"cancel_reserved"}});
    let transition_uri = format!(
        "/control/api/v1/relay/channels/{}/transitions",
        input["channel_id"].as_str().unwrap()
    );
    let (status, cancelled) = post(&app, &"r".repeat(48), &transition_uri, &cancel).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cancelled["channel"]["state"], "cancelled_before_claim");
}

#[tokio::test]
async fn relay_admission_claim_is_single_use_and_unknown_requires_exact_worker_terminal() {
    let directory = support::private_tempdir();
    let store = Arc::new(fresh(directory.path(), "tenant-a", 10000));
    let worker = running_now(&store);
    let token = "relay-admission-claim-fixture-token-0123456789";
    let (credential, revision) = bind(&store, &worker, token);
    let input = request(&store, &worker, &credential, revision, token);
    let app = gateway_router(
        GatewayState::for_tenants(
            vec![TenantGateway {
                tenant_id: "tenant-a".into(),
                store: store.clone(),
                runner_token: "r".repeat(48),
                viewer_token: "v".repeat(48),
            }],
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )
        .unwrap(),
    );
    let uri = "/control/api/v1/relay/channels/admissions";
    let (status, reserved) = post(&app, &"r".repeat(48), uri, &input).await;
    assert_eq!(status, StatusCode::CREATED);
    let claim = json!({"version":1,"operation_id":Uuid::new_v4().to_string(),
        "channel_id":input["channel_id"],"epoch":1,"expected_revision":1,
        "admission_input_hash":reserved["channel"]["admission_input_hash"]});
    let claim_uri = format!(
        "/control/api/v1/relay/channels/{}/claims",
        input["channel_id"].as_str().unwrap()
    );
    let (status, claimed) = post(&app, &"r".repeat(48), &claim_uri, &claim).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(claimed["channel"]["state"], "accepted_before_exec");
    assert_eq!(claimed["replayed"], false);
    let head = store.head().unwrap();
    let (status, replay) = post(&app, &"r".repeat(48), &claim_uri, &claim).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["receipt"], claimed["receipt"]);
    assert_eq!(head, store.head().unwrap());
    let mut second_claim = claim.clone();
    second_claim["operation_id"] = json!(Uuid::new_v4().to_string());
    assert_eq!(
        post(&app, &"r".repeat(48), &claim_uri, &second_claim)
            .await
            .0,
        StatusCode::CONFLICT
    );
    let transition_uri = format!(
        "/control/api/v1/relay/channels/{}/transitions",
        input["channel_id"].as_str().unwrap()
    );
    let unknown = json!({"version":1,"operation_id":Uuid::new_v4().to_string(),
        "channel_id":input["channel_id"],"epoch":1,"expected_revision":2,
        "admission_input_hash":reserved["channel"]["admission_input_hash"],
        "transition":{"kind":"unknown"}});
    assert_eq!(
        post(&app, &"r".repeat(48), &transition_uri, &unknown)
            .await
            .0,
        StatusCode::OK
    );
    let forged_close = json!({"version":1,"operation_id":Uuid::new_v4().to_string(),
        "channel_id":input["channel_id"],"epoch":1,"expected_revision":3,
        "admission_input_hash":reserved["channel"]["admission_input_hash"],
        "transition":{"kind":"close_exact_worker_terminal","worker_revision":worker.revision,
            "terminal_observation_id":Uuid::new_v4().to_string()}});
    assert_eq!(
        post(&app, &"r".repeat(48), &transition_uri, &forged_close)
            .await
            .0,
        StatusCode::CONFLICT
    );
    let mut replacement = input.clone();
    replacement["operation_id"] = json!(Uuid::new_v4().to_string());
    replacement["channel_id"] = json!(Uuid::new_v4().to_string());
    replacement["epoch"] = json!(2);
    assert_eq!(
        post(&app, &"r".repeat(48), uri, &replacement).await.0,
        StatusCode::CONFLICT
    );
    let mut terminal = worker.last_observation.clone().unwrap();
    terminal.observed_at = chrono::Utc::now().to_rfc3339();
    terminal.status = RuntimeStatusV1::Exited;
    terminal.running = Some(false);
    terminal.finished_at = Some(chrono::Utc::now().to_rfc3339());
    terminal.exit_code = Some(0);
    let terminal_request = mutation(
        &worker,
        WorkerTransitionV1::Observe {
            observation: terminal,
        },
    );
    let terminal_worker = store.mutate_worker(&terminal_request).unwrap().worker;
    assert_eq!(terminal_worker.state, WorkerStateV1::Terminal);
    store
        .revoke_workload_binding(&WorkloadBindingRevocationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            credential_id: credential,
            expected_revision: revision,
        })
        .unwrap();
    let mut close = forged_close;
    close["operation_id"] = json!(Uuid::new_v4().to_string());
    close["transition"]["worker_revision"] = json!(terminal_worker.revision);
    close["transition"]["terminal_observation_id"] = json!(terminal_request.operation_id);
    let (status, closed) = post(&app, &"r".repeat(48), &transition_uri, &close).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(closed["channel"]["state"], "closed");
    assert!(store.integrity().unwrap().valid);
    drop(app);
    drop(store);
    let reopened = Store::open_for_tenant(
        &directory.path().join("tenant-a.sqlite"),
        "tenant-a",
        Redactor::new(vec![]),
        10000,
    )
    .unwrap();
    assert!(reopened.integrity().unwrap().valid);
    assert!(!reopened.supervision_state().unwrap().active);
}

#[tokio::test]
async fn relay_admission_guards_roles_tenant_and_reserved_namespace() {
    let directory = support::private_tempdir();
    let a = Arc::new(fresh(directory.path(), "tenant-a", 10000));
    let b = Arc::new(fresh(directory.path(), "tenant-b", 10000));
    let worker = running_now(&a);
    let token = "relay-admission-boundary-fixture-token-0123456789";
    let (credential, revision) = bind(&a, &worker, token);
    let input = request(&a, &worker, &credential, revision, token);
    let app = gateway_router(
        GatewayState::for_tenants(
            vec![
                TenantGateway {
                    tenant_id: "tenant-a".into(),
                    store: a.clone(),
                    runner_token: "r".repeat(48),
                    viewer_token: "v".repeat(48),
                },
                TenantGateway {
                    tenant_id: "tenant-b".into(),
                    store: b.clone(),
                    runner_token: "b".repeat(48),
                    viewer_token: "w".repeat(48),
                },
            ],
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )
        .unwrap(),
    );
    let uri = "/control/api/v1/relay/channels/admissions";
    for credential in [&"v".repeat(48), token, &"unknown".repeat(8)] {
        let polled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let read = polled.clone();
        let body = Body::from_stream(futures_util::stream::once(async move {
            read.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok::<_, std::io::Error>("not valid JSON")
        }));
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("authorization", format!("Bearer {credential}"))
                    .header("content-type", "application/json")
                    .body(body)
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(matches!(
            response.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ));
        assert!(!polled.load(std::sync::atomic::Ordering::SeqCst));
    }
    let b_head = b.head().unwrap();
    assert_eq!(
        post(&app, &"b".repeat(48), uri, &input).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(b.head().unwrap(), b_head);
    let mut invalid = input.clone();
    invalid["tenant_id"] = json!("tenant-b");
    assert_eq!(
        post(&app, &"r".repeat(48), uri, &invalid).await.0,
        StatusCode::BAD_REQUEST
    );
    let mut event = agentsandbox::model::Event::new("relay_channel.v1");
    event.data = json!({"forged":true});
    let event = serde_json::to_value(event).unwrap();
    for uri in ["/api/v1/events", "/control/api/v1/control/events"] {
        assert_eq!(
            post(&app, &"r".repeat(48), uri, &event).await.0,
            StatusCode::FORBIDDEN
        );
    }
}
