//! Real tenant Store and bounded loopback HTTP tests. No runtime is contacted.
use super::*;
use crate::{
    api::gateway_router,
    broker_registry::{
        BrokerAuthorityV1, BrokerGrantMutationV1, BrokerIdentityViewV1, BrokerRevocationV1,
        BrokerRuntimeKindV1, format_time,
    },
    control::QuotaPolicyV1,
    workload::{WorkloadBindingIssueV1, WorkloadSubjectV1},
};
use axum::{body::Body, http::Request};
use reqwest::StatusCode;
use sha2::{Digest, Sha256};
use std::{fs, os::unix::fs::PermissionsExt, time::Duration};
use tower::ServiceExt;
use uuid::Uuid;

struct RecoveryServer {
    url: String,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}
impl RecoveryServer {
    async fn serve(router: axum::Router) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
        });
        Self {
            url,
            stop: Some(stop),
            task,
        }
    }
    async fn stop(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        tokio::time::timeout(Duration::from_secs(15), &mut self.task)
            .await
            .unwrap()
            .unwrap();
    }
}
impl Drop for RecoveryServer {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        self.task.abort();
    }
}

// The underlying real gateway commits first. Only its response is modified by
// this local test transport; no Store state, identity or proof is fabricated.
async fn lose_first_recovery_ack(
    request: Request<Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let recovery = request.method() == axum::http::Method::POST
        && request.uri().path().ends_with("/recoveries");
    let response = next.run(request).await;
    if recovery && response.status() == StatusCode::CREATED {
        use axum::response::IntoResponse;
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(serde_json::json!({"error":{"code":"test_ack_unavailable"}})),
        )
            .into_response();
    }
    response
}

async fn change_recovery_ack(
    request: Request<Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let recovery = request.method() == axum::http::Method::POST
        && request.uri().path().ends_with("/recoveries");
    let response = next.run(request).await;
    if recovery && response.status() == StatusCode::CREATED {
        let body = axum::body::to_bytes(response.into_body(), 128 * 1024)
            .await
            .unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        value["snapshot"]["quota_held"] = serde_json::json!(true);
        use axum::response::IntoResponse;
        return (StatusCode::CREATED, axum::Json(value)).into_response();
    }
    response
}

const EXPIRY: &str = "2100-01-01T00:00:00.000Z";

fn migrate(f: &tests::Fixture) {
    let dir = f.directory.path();
    f.store()
        .migrate_human_workers_v10(&dir.join("before10"))
        .unwrap();
    f.store()
        .migrate_kubernetes_session_reservations_v11(&dir.join("before11"))
        .unwrap();
    f.store()
        .migrate_broker_checkpoints_v12(&dir.join("before12"))
        .unwrap();
    f.store()
        .migrate_kubernetes_recovery_v13(&dir.join("before13"))
        .unwrap();
}

fn request(intent: &KubernetesCreateIntentV1) -> KubernetesCreateRecoveryV3 {
    KubernetesCreateRecoveryV3 {
        version: 3,
        operation_id: Uuid::new_v4().to_string(),
        create_id: intent.create_id.clone(),
        intent_hash: intent.intent_hash().unwrap(),
        expected_revision: 1,
        expected_create_epoch: 1,
    }
}

fn path(request: &KubernetesCreateRecoveryV3, lookup: bool) -> String {
    let base = format!(
        "/api/v1/brokers/kubernetes/held-creates/{}/recoveries",
        request.create_id
    );
    if lookup {
        format!("{base}/{}", request.operation_id)
    } else {
        base
    }
}

fn public_token(f: &tests::Fixture) -> String {
    fs::read_to_string(f.directory.path().join("broker-token")).unwrap()
}

fn successor(
    f: &tests::Fixture,
    url: &str,
    expected_revision: Option<u64>,
    stable_broker: &str,
    authority_id: String,
    not_after: &str,
) -> (HeldCreateControlClient, String) {
    let token = format!(
        "public-recovery-broker-{}-{}",
        Uuid::new_v4(),
        "s".repeat(32)
    );
    let credential_id = Uuid::new_v4().to_string();
    let incarnation_id = Uuid::new_v4().to_string();
    f.store()
        .issue_workload_binding(&WorkloadBindingIssueV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            credential_id: credential_id.clone(),
            expected_revision: 0,
            subject: WorkloadSubjectV1::Broker {
                broker_id: stable_broker.into(),
                incarnation_id: incarnation_id.clone(),
            },
            sha256_digest: hex::encode(Sha256::digest(token.as_bytes())),
            expires_at: EXPIRY.into(),
        })
        .unwrap();
    f.store()
        .mutate_broker_grant(&BrokerGrantMutationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            broker_id: stable_broker.into(),
            expected_revision,
            credential_id: credential_id.clone(),
            incarnation_id: incarnation_id.clone(),
            authorities: vec![BrokerAuthorityV1 {
                authority_id,
                runtime_kind: BrokerRuntimeKindV1::Kubernetes,
            }],
            lease_seconds: 120,
            not_after: not_after.into(),
        })
        .unwrap();
    let token_path = f
        .directory
        .path()
        .join(format!("successor-{credential_id}"));
    fs::write(&token_path, &token).unwrap();
    fs::set_permissions(&token_path, fs::Permissions::from_mode(0o600)).unwrap();
    let scope = HeldCreateClientScope {
        gateway: url.into(),
        tenant_id: f.client(url).expected_scope().tenant_id.clone(),
        control_namespace_uuid: f.intent.control_namespace_uuid.clone(),
        broker_id: stable_broker.into(),
        credential_id,
        incarnation_id,
    };
    (
        HeldCreateControlClient::new(
            scope,
            PrivateFile::open(&token_path, 4096).unwrap(),
            None,
            vec![token.clone()],
        )
        .unwrap(),
        token,
    )
}

async fn raw(
    router: &axum::Router,
    method: &str,
    target: &str,
    token: Option<&str>,
    body: Vec<u8>,
) -> (StatusCode, Vec<u8>) {
    let mut request = Request::builder()
        .method(method)
        .uri(target)
        .header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = tokio::time::timeout(
        Duration::from_secs(15),
        router
            .clone()
            .oneshot(request.body(Body::from(body)).unwrap()),
    )
    .await
    .unwrap()
    .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 128 * 1024)
        .await
        .unwrap();
    (status, body.to_vec())
}

async fn reserved(f: &tests::Fixture) {
    let server = f.serve(true, QuotaPolicyV1::default()).await;
    let reply = f.client(&server.url).reserve(&f.intent).await.unwrap();
    assert_eq!(reply.snapshot.state, KubernetesCreateStateV1::Reserved);
    server.stop().await;
}

#[tokio::test]
async fn recovery_successor_http_post_retry_and_get_keep_one_exact_close_and_legacy_scope() {
    let f = tests::Fixture::new("recovery-http-successor");
    migrate(&f);
    reserved(&f).await;
    let server = f.serve(true, QuotaPolicyV1::default()).await;
    let creator = f.client(&server.url);
    let (current, _) = successor(
        &f,
        &server.url,
        Some(1),
        &f.intent.broker_id,
        f.intent.authority.authority_id().to_string(),
        EXPIRY,
    );
    assert_ne!(
        current.expected_scope().incarnation_id,
        f.intent.incarnation_id
    );
    let recovery = request(&f.intent);
    let head = f.store().head().unwrap();
    assert_eq!(
        f.store()
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        1
    );
    // An absent read is never a hidden call to the mutating preparation port.
    assert_eq!(
        current
            .observe_recovery(&f.intent, &recovery)
            .await
            .unwrap_err(),
        HeldCreateClientError::NotFound
    );
    assert_eq!(f.store().head().unwrap(), head);
    let closed = current.recover(&f.intent, &recovery).await.unwrap();
    assert!(!closed.replayed);
    assert_eq!(closed.snapshot.intent, f.intent);
    assert_eq!(
        closed.snapshot.state,
        KubernetesCreateStateV1::CancelledBeforeEmission
    );
    assert!(!closed.snapshot.quota_held);
    assert!(closed.receipt.emission_window.is_none());
    let head = f.store().head().unwrap();
    let retried = current.recover(&f.intent, &recovery).await.unwrap();
    let observed = current
        .observe_recovery(&f.intent, &recovery)
        .await
        .unwrap();
    assert!(retried.replayed && observed.replayed);
    assert_eq!(retried.receipt, closed.receipt);
    assert_eq!(observed.receipt, closed.receipt);
    assert_eq!(observed.snapshot, closed.snapshot);
    assert_eq!(f.store().head().unwrap(), head);
    assert_eq!(
        f.store()
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        0
    );
    assert_eq!(
        creator.recover(&f.intent, &recovery).await.unwrap_err(),
        HeldCreateClientError::Authentication
    );
    // The existing legacy validator remains strict about the old incarnation.
    assert_eq!(
        validation::intent(current.expected_scope(), &f.intent),
        Err(HeldCreateClientError::InvalidContext)
    );
    let legacy = KubernetesCreateCancelV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        create_id: f.intent.create_id.clone(),
        intent_hash: f.intent.intent_hash().unwrap(),
        expected_revision: 1,
        expected_create_epoch: 1,
    };
    assert!(matches!(
        current.cancel(&legacy).await,
        Err(HeldCreateClientError::Forbidden)
    ));
    assert_eq!(f.store().head().unwrap(), head);
    let events = f
        .store()
        .events(&crate::audit::EventQuery {
            kind: Some("control.kubernetes.create.recovered_before_emission".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].seq, closed.receipt.audit_seq);
    assert_eq!(events[0].hash, closed.receipt.audit_hash);
    let data = &events[0].event.data;
    assert_eq!(
        data["identity"]["credential_id"],
        current.expected_scope().credential_id
    );
    assert_eq!(
        data["control_decision"]["incarnation_id"],
        current.expected_scope().incarnation_id
    );
    assert_eq!(
        data["control_decision"]["basis"]["creator_incarnation_id"],
        f.intent.incarnation_id
    );
    assert_ne!(
        data["control_decision"]["incarnation_id"],
        data["control_decision"]["basis"]["creator_incarnation_id"]
    );
    assert!(f.store().integrity().unwrap().valid);
    server.stop().await;
}

#[tokio::test]
async fn recovery_http_is_closed_scoped_and_get_never_mutates() {
    let f = tests::Fixture::new("recovery-http-closed");
    migrate(&f);
    reserved(&f).await;
    let router = gateway_router(f.gateway(true, QuotaPolicyV1::default()));
    let token = public_token(&f);
    let recovery = request(&f.intent);
    let exact = serde_json::to_vec(&recovery).unwrap();
    let head = f.store().head().unwrap();
    for (method, target, body, expected) in [
        ("GET", path(&recovery, true), vec![], StatusCode::NOT_FOUND),
        (
            "GET",
            format!("{}?tenant=other", path(&recovery, true)),
            vec![],
            StatusCode::BAD_REQUEST,
        ),
        (
            "GET",
            path(&recovery, true),
            b"{}".to_vec(),
            StatusCode::BAD_REQUEST,
        ),
        (
            "POST",
            format!("{}?x=1", path(&recovery, false)),
            exact.clone(),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let (status, _) = raw(&router, method, &target, Some(&token), body).await;
        assert_eq!(status, expected);
        assert_eq!(f.store().head().unwrap(), head);
    }
    for extra in [
        "\"actor\":\"operator\"",
        "\"tenant_id\":\"other\"",
        "\"version\":3",
    ] {
        let mut payload = String::from_utf8(exact.clone()).unwrap();
        payload.pop();
        payload.push(',');
        payload.push_str(extra);
        payload.push('}');
        assert_eq!(
            raw(
                &router,
                "POST",
                &path(&recovery, false),
                Some(&token),
                payload.into_bytes()
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    for bad in [
        Uuid::nil().to_string(),
        "AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA".into(),
        "not-a-uuid".into(),
    ] {
        let target = format!(
            "/api/v1/brokers/kubernetes/held-creates/{bad}/recoveries/{}",
            recovery.operation_id
        );
        assert_eq!(
            raw(&router, "GET", &target, Some(&token), vec![]).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    let mut other = recovery.clone();
    other.create_id = Uuid::new_v4().to_string();
    assert_eq!(
        raw(
            &router,
            "POST",
            &path(&recovery, false),
            Some(&token),
            serde_json::to_vec(&other).unwrap()
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    for token in [
        None,
        Some("unknown-public-bearer-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"),
    ] {
        let (status, _) = raw(
            &router,
            "POST",
            &path(&recovery, false),
            token,
            exact.clone(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    let viewer = "public-held-viewer-recovery-http-closed".to_string();
    let runner = format!(
        "public-control-runner-recovery-http-closed-{}",
        "r".repeat(40)
    );
    for (token, role) in [(viewer, "viewer"), (runner, "runner")] {
        let (status, identity) =
            raw(&router, "GET", "/api/v1/identity", Some(&token), vec![]).await;
        assert_eq!(status, StatusCode::OK);
        let identity: serde_json::Value = serde_json::from_slice(&identity).unwrap();
        assert_eq!(identity["tenant_id"], "recovery-http-closed");
        assert_eq!(identity["role"], role);
        for (method, target, body) in [
            ("POST", path(&recovery, false), exact.clone()),
            ("GET", path(&recovery, true), vec![]),
        ] {
            assert_eq!(
                raw(&router, method, &target, Some(&token), body).await.0,
                StatusCode::FORBIDDEN
            );
        }
    }
    assert_eq!(f.store().head().unwrap(), head);
    let (status, body) = raw(
        &router,
        "POST",
        &path(&recovery, false),
        Some(&token),
        exact,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let closed: KubernetesCreateRecoveryResultV3 = serde_json::from_slice(&body).unwrap();
    let head = f.store().head().unwrap();
    let mut changed = recovery.clone();
    changed.intent_hash = "c".repeat(64);
    assert_eq!(
        raw(
            &router,
            "POST",
            &path(&recovery, false),
            Some(&token),
            serde_json::to_vec(&changed).unwrap()
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let mut fresh_id = recovery.clone();
    fresh_id.operation_id = Uuid::new_v4().to_string();
    assert_eq!(
        raw(
            &router,
            "POST",
            &path(&recovery, false),
            Some(&token),
            serde_json::to_vec(&fresh_id).unwrap()
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let (_, body) = raw(&router, "GET", &path(&recovery, true), Some(&token), vec![]).await;
    let observed: KubernetesCreateRecoveryResultV3 = serde_json::from_slice(&body).unwrap();
    assert_eq!(observed.receipt, closed.receipt);
    assert_eq!(f.store().head().unwrap(), head);
}

#[tokio::test]
async fn recovery_current_auth_is_rechecked_before_existing_receipt_lookup() {
    let f = tests::Fixture::new("recovery-auth-recheck");
    migrate(&f);
    reserved(&f).await;
    let server = f.serve(true, QuotaPolicyV1::default()).await;
    let old = f.client(&server.url);
    let recovery = request(&f.intent);
    let closed = old.recover(&f.intent, &recovery).await.unwrap();
    let (current, _) = successor(
        &f,
        &server.url,
        Some(1),
        &f.intent.broker_id,
        f.intent.authority.authority_id().to_string(),
        EXPIRY,
    );
    let head = f.store().head().unwrap();
    assert_eq!(
        old.observe_recovery(&f.intent, &recovery)
            .await
            .unwrap_err(),
        HeldCreateClientError::Authentication
    );
    assert_eq!(
        old.recover(&f.intent, &recovery).await.unwrap_err(),
        HeldCreateClientError::Authentication
    );
    assert_eq!(
        current
            .observe_recovery(&f.intent, &recovery)
            .await
            .unwrap()
            .receipt,
        closed.receipt
    );
    f.store()
        .revoke_broker(&BrokerRevocationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            broker_id: f.intent.broker_id.clone(),
            expected_revision: 2,
        })
        .unwrap();
    let revoked_head = f.store().head().unwrap();
    assert!(revoked_head > head);
    assert_eq!(
        current
            .observe_recovery(&f.intent, &recovery)
            .await
            .unwrap_err(),
        HeldCreateClientError::Forbidden
    );
    assert_eq!(
        current.recover(&f.intent, &recovery).await.unwrap_err(),
        HeldCreateClientError::Forbidden
    );
    assert_eq!(f.store().head().unwrap(), revoked_head);
    server.stop().await;
}

#[tokio::test]
async fn recovery_rejects_other_stable_broker_authority_tenant_and_expired_actor() {
    let f = tests::Fixture::new("recovery-scope-recheck");
    migrate(&f);
    reserved(&f).await;
    let server = f.serve(true, QuotaPolicyV1::default()).await;
    let recovery = request(&f.intent);
    let closed = f
        .client(&server.url)
        .recover(&f.intent, &recovery)
        .await
        .unwrap();
    assert_eq!(closed.recovery, recovery);
    let other_broker_id = Uuid::new_v4().to_string();
    let other_authority_id = Uuid::new_v4().to_string();
    assert_ne!(other_broker_id, f.intent.broker_id);
    assert_ne!(
        other_authority_id,
        f.intent.authority.authority_id().to_string()
    );
    // Authority ownership cannot transfer to a different stable broker. Give
    // this valid broker its own authority before checking the recovery refusal.
    let (other_broker, token) = successor(
        &f,
        &server.url,
        None,
        &other_broker_id,
        other_authority_id,
        EXPIRY,
    );
    let router = gateway_router(f.gateway(true, QuotaPolicyV1::default()));
    let head = f.store().head().unwrap();
    let (status, body) = raw(
        &router,
        "GET",
        "/api/v1/brokers/identity",
        Some(&token),
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let identity: BrokerIdentityViewV1 = serde_json::from_slice(&body).unwrap();
    let scope = other_broker.expected_scope();
    assert_eq!(
        identity,
        BrokerIdentityViewV1 {
            version: 1,
            tenant_id: scope.tenant_id.clone(),
            control_namespace_uuid: scope.control_namespace_uuid.clone(),
            schema_version: 13,
            broker_id: other_broker_id,
            incarnation_id: scope.incarnation_id.clone(),
            credential_id: scope.credential_id.clone(),
            binding_revision: 1,
            grant_revision: 1,
        }
    );
    assert_eq!(f.store().head().unwrap(), head);
    assert_eq!(
        other_broker
            .observe_recovery(&f.intent, &recovery)
            .await
            .unwrap_err(),
        HeldCreateClientError::InvalidContext
    );
    assert_eq!(
        raw(&router, "GET", &path(&recovery, true), Some(&token), vec![])
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(f.store().head().unwrap(), head);
    let (wrong_authority, _) = successor(
        &f,
        &server.url,
        Some(1),
        &f.intent.broker_id,
        Uuid::new_v4().to_string(),
        EXPIRY,
    );
    let head = f.store().head().unwrap();
    assert_eq!(
        wrong_authority
            .observe_recovery(&f.intent, &recovery)
            .await
            .unwrap_err(),
        HeldCreateClientError::Forbidden
    );
    assert_eq!(f.store().head().unwrap(), head);
    let expires = chrono::Utc::now() + chrono::Duration::seconds(15);
    let (expires_soon, _) = successor(
        &f,
        &server.url,
        Some(2),
        &f.intent.broker_id,
        f.intent.authority.authority_id().to_string(),
        &format_time(expires),
    );
    let remaining = (expires - chrono::Utc::now()).to_std().unwrap_or_default();
    tokio::time::sleep(remaining + Duration::from_millis(250)).await;
    let head = f.store().head().unwrap();
    assert_eq!(
        expires_soon
            .observe_recovery(&f.intent, &recovery)
            .await
            .unwrap_err(),
        HeldCreateClientError::Forbidden
    );
    assert_eq!(
        expires_soon
            .recover(&f.intent, &recovery)
            .await
            .unwrap_err(),
        HeldCreateClientError::Forbidden
    );
    assert_eq!(f.store().head().unwrap(), head);
    server.stop().await;
    let b = tests::Fixture::new("recovery-unrelated-tenant");
    migrate(&b);
    let router = gateway_router(b.gateway(true, QuotaPolicyV1::default()));
    let head = b.store().head().unwrap();
    assert_eq!(
        raw(
            &router,
            "GET",
            &path(&recovery, true),
            Some(&public_token(&b)),
            vec![]
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        raw(
            &router,
            "POST",
            &path(&recovery, false),
            Some(&public_token(&b)),
            serde_json::to_vec(&recovery).unwrap()
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(b.store().head().unwrap(), head);
}

#[tokio::test]
async fn recovery_client_validates_entire_descriptive_result_without_relaxing_legacy_intent() {
    let f = tests::Fixture::new("recovery-client-result");
    migrate(&f);
    reserved(&f).await;
    let server = f.serve(true, QuotaPolicyV1::default()).await;
    let client = f.client(&server.url);
    let recovery = request(&f.intent);
    let result = client.recover(&f.intent, &recovery).await.unwrap();
    assert_eq!(
        validation::recovery(
            client.expected_scope(),
            &f.intent,
            &recovery,
            &result,
            StatusCode::CREATED,
            false
        ),
        Ok(())
    );
    let mut value = serde_json::to_value(&result).unwrap();
    for pointer in [
        "/version",
        "/recovery/version",
        "/recovery/expected_revision",
        "/receipt/version",
        "/snapshot/version",
        "/snapshot/revision",
        "/snapshot/create_epoch",
    ] {
        let old = value.pointer(pointer).unwrap().clone();
        *value.pointer_mut(pointer).unwrap() = serde_json::json!(99);
        let bad = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(
            validation::recovery(
                client.expected_scope(),
                &f.intent,
                &recovery,
                &bad,
                StatusCode::CREATED,
                false
            ),
            Err(HeldCreateClientError::Protocol),
            "{pointer}"
        );
        *value.pointer_mut(pointer).unwrap() = old;
    }
    for pointer in [
        "/receipt/input_hash",
        "/receipt/audit_hash",
        "/snapshot/intent_hash",
        "/snapshot/target/request_sha256",
    ] {
        let old = value.pointer(pointer).unwrap().clone();
        *value.pointer_mut(pointer).unwrap() = serde_json::json!("x".repeat(64));
        let bad = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(
            validation::recovery(
                client.expected_scope(),
                &f.intent,
                &recovery,
                &bad,
                StatusCode::CREATED,
                false
            ),
            Err(HeldCreateClientError::Protocol),
            "{pointer}"
        );
        *value.pointer_mut(pointer).unwrap() = old;
    }
    for (pointer, replacement) in [
        (
            "/receipt/operation_id",
            serde_json::json!(Uuid::new_v4().to_string()),
        ),
        (
            "/receipt/control_namespace_uuid",
            serde_json::json!(Uuid::new_v4().to_string()),
        ),
        ("/receipt/audit_seq", serde_json::json!(0)),
        ("/snapshot/quota_held", serde_json::json!(true)),
        ("/snapshot/state", serde_json::json!("reserved")),
        ("/snapshot/last_cancel", serde_json::Value::Null),
        (
            "/receipt/emission_window",
            serde_json::json!({"issued_at":"2099-01-01T00:00:00.000Z","not_after":"2099-01-01T00:00:01.000Z"}),
        ),
    ] {
        let old = value.pointer(pointer).unwrap().clone();
        *value.pointer_mut(pointer).unwrap() = replacement;
        let bad = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(
            validation::recovery(
                client.expected_scope(),
                &f.intent,
                &recovery,
                &bad,
                StatusCode::CREATED,
                false
            ),
            Err(HeldCreateClientError::Protocol),
            "{pointer}"
        );
        *value.pointer_mut(pointer).unwrap() = old;
    }
    assert_eq!(
        validation::recovery(
            client.expected_scope(),
            &f.intent,
            &recovery,
            &result,
            StatusCode::OK,
            false
        ),
        Err(HeldCreateClientError::Protocol)
    );
    assert_eq!(
        validation::recovery(
            client.expected_scope(),
            &f.intent,
            &recovery,
            &result,
            StatusCode::OK,
            true
        ),
        Err(HeldCreateClientError::Protocol)
    );
    server.stop().await;
}

#[tokio::test]
async fn recovery_client_reuses_exact_operation_after_a_real_commit_loses_its_ack() {
    let f = tests::Fixture::new("recovery-lost-ack");
    migrate(&f);
    reserved(&f).await;
    let router = gateway_router(f.gateway(true, QuotaPolicyV1::default()))
        .layer(axum::middleware::from_fn(lose_first_recovery_ack));
    let server = RecoveryServer::serve(router).await;
    let client = f.client(&server.url);
    let recovery = request(&f.intent);
    let head = f.store().head().unwrap();
    assert_eq!(
        client.recover(&f.intent, &recovery).await.unwrap_err(),
        HeldCreateClientError::Unavailable
    );
    assert_eq!(f.store().head().unwrap(), head + 1);
    assert_eq!(
        f.store()
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        0
    );
    let closed_head = f.store().head().unwrap();
    let recovered = client.recover(&f.intent, &recovery).await.unwrap();
    assert!(recovered.replayed);
    assert_eq!(recovered.recovery, recovery);
    assert_eq!(recovered.receipt.operation_id, recovery.operation_id);
    assert_eq!(recovered.receipt.audit_seq, closed_head);
    assert_eq!(
        client
            .observe_recovery(&f.intent, &recovery)
            .await
            .unwrap()
            .receipt,
        recovered.receipt
    );
    assert_eq!(f.store().head().unwrap(), closed_head);
    server.stop().await;
}

#[tokio::test]
async fn recovery_client_refuses_an_altered_real_http_result_and_keeps_lookup_read_only() {
    let f = tests::Fixture::new("recovery-altered-ack");
    migrate(&f);
    reserved(&f).await;
    let router = gateway_router(f.gateway(true, QuotaPolicyV1::default()))
        .layer(axum::middleware::from_fn(change_recovery_ack));
    let server = RecoveryServer::serve(router).await;
    let client = f.client(&server.url);
    let recovery = request(&f.intent);
    let head = f.store().head().unwrap();
    assert_eq!(
        client.recover(&f.intent, &recovery).await.unwrap_err(),
        HeldCreateClientError::Protocol
    );
    assert_eq!(f.store().head().unwrap(), head + 1);
    let closed_head = f.store().head().unwrap();
    let exact = client.observe_recovery(&f.intent, &recovery).await.unwrap();
    assert!(!exact.snapshot.quota_held);
    assert!(exact.replayed);
    assert_eq!(f.store().head().unwrap(), closed_head);
    server.stop().await;
}
