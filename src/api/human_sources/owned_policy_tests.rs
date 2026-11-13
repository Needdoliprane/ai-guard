//! A1 tests-first: real owned15 authority and SDK diagnostic before a raw POST.
//! No future policy DTO/SDK is required by this baseline. NOT_RUN until Root QA.
use crate::{
    api::{GatewayState, gateway_router},
    audit::{Store, egress::EgressPolicyReceiptV1},
    egress::{Authority, EgressPolicyV1, EgressTransport, PolicyLimits},
    policy::Policy,
    redaction::Redactor,
    runner::AuditClient,
};
use axum::{
    extract::Request,
    middleware::{self, Next},
};
use serde_json::json;
use std::{
    sync::{Arc, atomic::{AtomicU16, Ordering}},
    time::Duration,
};
use tokio::{sync::oneshot, task::JoinHandle};

const RUNNER: &str = "synthetic-A-runner";
const PROTOCOL: &str = "/control/api/v1/control/protocol";
const REPLACE: &str = "/control/api/v1/egress/policies/replace";

struct Server {
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<std::io::Result<()>>>,
}
impl Server {
    async fn close(&mut self) {
        self.stop.take().unwrap().send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(30), self.task.take().unwrap())
            .await
            .expect("A1 fixture HTTP cleanup timed out")
            .unwrap()
            .unwrap();
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_policy_replace_reaches_real_e1_after_reserved_sdk_diagnostic() {
    // The existing fixture performs real13->14->15 migrations, then a genuine
    // owned startup and binding provision. It retains its private files here.
    let mut fixture = Store::owned_control_fixture_for_test(15);
    let (bundle, credentials, tokens) = fixture.take_startup();
    let state = GatewayState::for_owned_human_sources(
        bundle,
        credentials.clone(),
        tokens,
        Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .unwrap();
    let owned = state.owned_human.as_ref().unwrap();
    let index = state.tenants.iter().position(|tenant| tenant.tenant_id == "A").unwrap();
    let credential = crate::api::authenticate_review_runner_fixture(
        credentials,
        owned.owner.clone(),
        state.tenants.iter().map(|tenant| (tenant.tenant_id.clone(), Arc::clone(&tenant.store))).collect(),
        RUNNER,
    )
    .unwrap();
    let source = &owned.sources[index];
    let (namespace, _) = {
        let epoch = owned.owner.try_read_epoch().unwrap();
        state.tenants[index].store.assert_owned_control_fixture_binding_for_test(
            &epoch, source, &credential, 15,
        )
    };
    let database = source.descriptor().storage_binding.path();
    drop(credential);

    // This is input validation, not an authority or a supplied control clock.
    // No DNS is performed: the core accepts only a descriptive exact name.
    let at = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap();
    let policy = EgressPolicyV1::new(
        1,
        at.saturating_sub(60_000),
        at.checked_add(600_000).unwrap(),
        &[Authority::new("example.com", 443, EgressTransport::ConnectTcpOpaque).unwrap()],
        PolicyLimits::new(1, 1).unwrap(),
    )
    .unwrap();
    let operation_id = uuid::Uuid::new_v4();
    let body = serde_json::to_vec(&json!({
        "version":1,
        "expected_scope":{"tenant_id":"A","control_namespace_uuid":namespace.as_str()},
        "operation_id":operation_id,
        "selector":{"kind":"tenant"},
        "expected_revision":0,
        "expected_epoch":0,
        "policy":{
            "not_before_ms":policy.not_before_ms(),
            "expires_at_ms":policy.expires_at_ms(),
            "destinations":[{"name":"example.com","port":443,"transport":"connect_tcp_opaque"}],
            "max_destinations":1,
            "max_resolution_ips":1
        },
        "tenant_ceiling":null
    })).unwrap();
    assert!(body.len() <= 65_536);
    let retirement = state.human_retirement().unwrap();
    let diagnostic_status = Arc::new(AtomicU16::new(0));
    let record = diagnostic_status.clone();
    let app = gateway_router(state.clone()).layer(middleware::from_fn(
        move |request: Request, next: Next| {
            let record = record.clone();
            async move {
                let diagnostic = request.method() == axum::http::Method::GET
                    && request.uri().path() == PROTOCOL;
                let response = next.run(request).await;
                if diagnostic {
                    record.store(response.status().as_u16(), Ordering::Release);
                }
                response
            }
        },
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = oneshot::channel();
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async { let _ = stopped.await; })
            .await
    });
    let mut server = Server { stop: Some(stop), task: Some(task) };
    let sdk = AuditClient::new(&origin, RUNNER).unwrap();
    let diagnostic = tokio::time::timeout(
        Duration::from_secs(120), sdk.control_supervision_identity(),
    ).await.expect("A1 SDK setup watchdog elapsed; not feature absence");
    let valid_setup = diagnostic.as_ref().is_ok_and(|value| {
        value.schema_version == 15
            && value.tenant_id == "A"
            && value.role == "runner"
            && value.control_namespace_uuid.as_deref() == Some(namespace.as_str())
    }) && diagnostic_status.load(Ordering::Acquire) == 200;
    let mut status = None;
    let mut bytes = Vec::new();
    if valid_setup {
        // The classifier may use this fixed marker only with the named200
        // assertion below. Compile/setup/cleanup failure is never expected RED.
        println!("A1_POLICY_REPLACE_SETUP_REAL_OWNED15_REGISTRY_BINDING_SDK200");
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(15))
            .build().unwrap();
        let mut response = http.post(format!("{origin}{REPLACE}"))
            .bearer_auth(RUNNER)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send().await.expect("A1 raw HTTP delivery failed; not feature absence");
        status = Some(response.status().as_u16());
        while let Some(chunk) = response.chunk().await.unwrap() {
            assert!(chunk.len() <= 4096 - bytes.len(), "A1 receipt/error exceeds4096bytes");
            bytes.extend_from_slice(&chunk);
        }
    }
    drop(sdk);
    server.close().await;
    retirement.retire();
    tokio::time::timeout(Duration::from_secs(30), retirement.drain())
        .await.expect("A1 owned cleanup watchdog elapsed").unwrap();
    assert!(valid_setup, "A1_SETUP_REQUIRES_REAL_FORMAT15_SDK200_AND_SCOPE");
    // Discriminating baseline: actual404/503 feature absence, not a declared
    // vulnerability or a passing product test. The real POST must return200.
    assert_eq!(status, Some(200), "A1_POLICY_REPLACE_EXPECTED_HTTP200_AFTER_REAL_SETUP");

    let receipt: EgressPolicyReceiptV1 = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(receipt.canonical_bytes().unwrap(), bytes);
    let value = serde_json::to_value(&receipt).unwrap();
    assert_eq!(value["operation_id"], operation_id.to_string());
    assert_eq!(value["event_id"], operation_id.to_string());
    assert_eq!(value["action"], "replace");
    assert_eq!(value["selector"], json!({"kind":"tenant"}));
    assert_eq!(value["revision"], 1);
    assert_eq!(value["before_epoch"], 0);
    assert_eq!(value["after_epoch"], 1);
    assert_eq!(value["state"], "active");
    assert_eq!(value["policy_digest"], hex::encode(policy.digest()));
    // Read-only oracle on this synthetic tenantA database after real drain.
    // A receipt-shaped HTTP body alone must not satisfy the future GREEN test.
    let db = rusqlite::Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let (canonical, sequence, hash, kind): (String, i64, String, String) = db.query_row(
        "SELECT o.receipt_json,e.seq,e.hash,e.kind FROM egress_policy_operations o JOIN events e ON e.seq=o.event_seq WHERE o.operation_id=?1",
        [operation_id.to_string()], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
    ).unwrap();
    assert_eq!(canonical.as_bytes(), bytes);
    assert_eq!(value["event_seq"], sequence);
    assert_eq!(value["event_hash"], hash);
    assert_eq!(kind, "control.egress_policy.replaced");
    let held: (i64, i64) = db.query_row(
        "SELECT events,bytes FROM egress_policy_reservations WHERE scope_kind='tenant' AND scope_id=''",
        [], |row| Ok((row.get(0)?,row.get(1)?)),
    ).unwrap();
    assert_eq!(held, (1, 98_304));
}
