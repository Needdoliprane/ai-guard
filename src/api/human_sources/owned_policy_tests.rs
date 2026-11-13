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
        let task = self.task.as_mut().unwrap();
        match tokio::time::timeout(Duration::from_secs(30), &mut *task).await {
            Ok(result) => { result.unwrap().unwrap(); self.task.take(); }
            Err(_) => {
                task.abort();
                let aborted = tokio::time::timeout(Duration::from_secs(30), &mut *task).await;
                if aborted.is_ok() { self.task.take(); }
                // A still-unfinished handle remains retained by Server. No
                // timeout is mistaken for observed server drainage.
                panic!("A1 fixture HTTP cleanup timed out; abort observed={}", aborted.is_ok());
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


// Additional implementation regressions use the same genuine fixture and real
// Router. They do not participate in the archived feature-absence baseline.
fn policy_state(format: u32) -> (impl Send, GatewayState) {
    let mut fixture = Store::owned_control_fixture_for_test(format);
    let (bundle, credentials, tokens) = fixture.take_startup();
    let state = GatewayState::for_owned_human_sources(bundle, credentials, tokens, Policy::default(), None, Redactor::new(vec![])).unwrap();
    (fixture, state)
}
fn policy_body(state: &GatewayState) -> serde_json::Value {
    let owned = state.owned_human.as_ref().unwrap();
    let index = state.tenants.iter().position(|t|t.tenant_id=="A").unwrap();
    let now = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap();
    json!({"version":1,"expected_scope":{"tenant_id":"A","control_namespace_uuid":owned.sources[index].descriptor().durable_namespace_id},
        "operation_id":uuid::Uuid::new_v4(),"selector":{"kind":"tenant"},"expected_revision":0,"expected_epoch":0,
        "policy":{"not_before_ms":now.saturating_sub(60_000),"expires_at_ms":now+600_000,
            "destinations":[{"name":"example.com","port":443,"transport":"connect_tcp_opaque"}],
            "max_destinations":1,"max_resolution_ips":1},"tenant_ceiling":null})
}
async fn policy_call(state: &GatewayState, method: &str, path: &str, token: &str, value: serde_json::Value) -> (u16, Vec<u8>) {
    use tower::ServiceExt;
    let request = axum::http::Request::builder().method(method).uri(path)
        .header("authorization",format!("Bearer {token}")).header("content-type","application/json")
        .body(axum::body::Body::from(serde_json::to_vec(&value).unwrap())).unwrap();
    let response = tokio::time::timeout(Duration::from_secs(120),gateway_router(state.clone()).oneshot(request)).await.unwrap().unwrap();
    let status = response.status().as_u16();
    let body = axum::body::to_bytes(response.into_body(),4096).await.unwrap();
    (status,body.to_vec())
}
async fn policy_drain(state: &GatewayState) {
    let retirement = state.human_retirement().unwrap(); retirement.retire();
    tokio::time::timeout(Duration::from_secs(30),retirement.drain()).await.unwrap().unwrap();
}
fn policy_snapshot(state: &GatewayState) -> Vec<Vec<rusqlite::types::Value>> {
    let owned = state.owned_human.as_ref().unwrap();
    let index = state.tenants.iter().position(|t|t.tenant_id=="A").unwrap();
    let path = owned.sources[index].descriptor().storage_binding.path();
    let db = rusqlite::Connection::open_with_flags(path,rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let mut all = Vec::new();
    for (table,columns) in [("egress_policy_meta",5),("egress_policy_revisions",6),("egress_policy_heads",6),("egress_policy_operations",5),("egress_policy_reservations",4),("audit_time_floors",2)] {
        all.extend(db.prepare(&format!("SELECT * FROM {table} ORDER BY 1,2")).unwrap().query_map([],|row|{
            (0..columns).map(|i|row.get(i)).collect::<rusqlite::Result<Vec<rusqlite::types::Value>>>()
        }).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap());
    }
    all.push(db.query_row("SELECT count(*),coalesce(max(seq),0) FROM events",[],|r|Ok(vec![r.get(0)?,r.get(1)?])).unwrap());
    all.push(db.query_row("SELECT bytes FROM quota WHERE id=1",[],|r|Ok(vec![r.get(0)?])).unwrap());
    all
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn expected_scope_conflicts_before_fresh_and_replayed_policy_with_no_neighbor_sql() {
    let (_fixture,state) = policy_state(15);
    let b = state.tenants.iter().position(|t|t.tenant_id=="B").unwrap();
    let neighbor = crate::audit::read_test_observer::observe(&state.tenants[b].store);
    let request = policy_body(&state);
    for replay in [false,true] {
        if replay { assert_eq!(policy_call(&state,"POST",REPLACE,RUNNER,request.clone()).await.0,200); }
        let before = policy_snapshot(&state);
        for (field,value) in [("tenant_id",json!("B")),("control_namespace_uuid",json!(uuid::Uuid::new_v4()))] {
            let mut wrong = request.clone(); wrong["expected_scope"][field]=value;
            let (status,body) = policy_call(&state,"POST",REPLACE,RUNNER,wrong).await;
            assert_eq!(status,409);
            assert_eq!(serde_json::from_slice::<serde_json::Value>(&body).unwrap()["error"]["code"],"egress_policy_scope_conflict");
            assert_eq!(policy_snapshot(&state),before);
            assert!(neighbor.try_recv().is_err());
        }
    }
    policy_drain(&state).await;
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn expected_scope_survives_real_gateway_namespace_replacement() {
    let (_old,old) = policy_state(15);
    let request = policy_body(&old);
    let (status,body) = policy_call(&old,"GET",PROTOCOL,RUNNER,json!({})).await;
    assert_eq!(status,200);
    let diagnostic: crate::control::ControlProtocolV1 = serde_json::from_slice(&body).unwrap();
    assert_eq!(diagnostic.control_namespace_uuid.as_deref(),request["expected_scope"]["control_namespace_uuid"].as_str());
    let (_new,new) = policy_state(15);
    assert_ne!(policy_body(&new)["expected_scope"],request["expected_scope"]);
    let before = policy_snapshot(&new);
    // Both routers have genuine but different authorities with the same
    // configured bearer; no mocked diagnostic or Store result participates.
    assert_eq!(policy_call(&new,"POST",REPLACE,RUNNER,request).await.0,409);
    assert_eq!(policy_snapshot(&new),before);
    policy_drain(&old).await; policy_drain(&new).await;
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn policy_scope_body_and_method_failures_keep_existing_boundaries() {
    let (_fixture,state) = policy_state(15);
    let original = policy_body(&state);
    let before = policy_snapshot(&state);
    for (token,status) in [("synthetic-A-viewer",403),("synthetic-unknown-control",401)] {
        assert_eq!(policy_call(&state,"POST",REPLACE,token,original.clone()).await.0,status);
        assert_eq!(policy_snapshot(&state),before);
    }
    for scope in [None,Some(json!(null)),Some(json!({"tenant_id":"A"}))] {
        let mut request = original.clone();
        match scope {Some(scope)=>request["expected_scope"]=scope,None=>{request.as_object_mut().unwrap().remove("expected_scope");}}
        assert_eq!(policy_call(&state,"POST",REPLACE,RUNNER,request).await.0,400);
        assert_eq!(policy_snapshot(&state),before);
    }
    for (method,path) in [("POST","/api/v1/egress/policies/replace"),("HEAD",REPLACE),("GET",REPLACE),("POST","/control/api/v1/egress/policies/unknown")] {
        assert!(matches!(policy_call(&state,method,path,RUNNER,original.clone()).await.0,404|405|503));
        assert_eq!(policy_snapshot(&state),before);
    }
    policy_drain(&state).await;
    let (_fixture,state) = policy_state(14);
    let (status,body) = policy_call(&state,"POST",REPLACE,RUNNER,policy_body(&state)).await;
    assert_eq!(status,409);
    assert_eq!(serde_json::from_slice::<serde_json::Value>(&body).unwrap()["error"]["code"],"egress_policy_migration_required");
    policy_drain(&state).await;
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn policy_role_format_and_declared_size_refuse_before_body_polling() {
    use std::sync::atomic::AtomicUsize;
    use tower::ServiceExt;
    for (format,token,length,expected_status) in [
        (15,"synthetic-A-viewer",None,403),
        (15,"synthetic-unknown-control",None,401),
        (14,RUNNER,None,409),
        (15,RUNNER,Some("65537"),413),
    ] {
        let (_fixture,state) = policy_state(format);
        let b = state.tenants.iter().position(|t|t.tenant_id=="B").unwrap();
        let neighbor = crate::audit::read_test_observer::observe(&state.tenants[b].store);
        let polls = Arc::new(AtomicUsize::new(0));
        let observed = polls.clone();
        // An actual untrusted request stream witnesses ordering: polling it
        // would return an IO error, rather than a fabricated HTTP response.
        let body = axum::body::Body::from_stream(futures_util::stream::poll_fn(move |_| {
            observed.fetch_add(1,Ordering::SeqCst);
            std::task::Poll::Ready(Some(Err::<axum::body::Bytes,_>(std::io::Error::other("synthetic body must remain unpolled"))))
        }));
        let mut request = axum::http::Request::builder().method("POST").uri(REPLACE)
            .header("authorization",format!("Bearer {token}"))
            .header("content-type","application/json");
        if let Some(length) = length { request = request.header("content-length",length); }
        let request = request.body(body).unwrap();
        let response = tokio::time::timeout(Duration::from_secs(120),gateway_router(state.clone()).oneshot(request)).await.unwrap().unwrap();
        assert_eq!(response.status().as_u16(),expected_status);
        assert_eq!(polls.load(Ordering::SeqCst),0,"rejected admission polled the request body");
        assert!(neighbor.try_recv().is_err());
        policy_drain(&state).await;
    }
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn retained_read_gate_refuses_policy_write_until_the_same_guard_is_released() {
    let (_fixture,state) = policy_state(15);
    let owned = state.owned_human.as_ref().unwrap();
    let a = state.tenants.iter().position(|t|t.tenant_id=="A").unwrap();
    let b = state.tenants.iter().position(|t|t.tenant_id=="B").unwrap();
    let neighbor = crate::audit::read_test_observer::observe(&state.tenants[b].store);
    let request = policy_body(&state);
    let before = policy_snapshot(&state);
    let retained = owned.owner.try_read_epoch().unwrap();
    retained.validate_source(&owned.sources[a]).unwrap();
    assert!(matches!(owned.owner.try_write_epoch(),Err(crate::human_authority::HumanAuthorityFailure::GateBusy)));
    let (status,body) = policy_call(&state,"POST",REPLACE,RUNNER,request.clone()).await;
    assert_eq!(status,503);
    assert_eq!(serde_json::from_slice::<serde_json::Value>(&body).unwrap()["error"]["code"],"egress_policy_busy");
    assert_eq!(policy_snapshot(&state),before);
    assert!(neighbor.try_recv().is_err());
    // The retained guard, rather than an elapsed epoch deadline, is the gate
    // witness. No stale-epoch validation after the HTTP wait is asserted.
    assert!(matches!(owned.owner.try_write_epoch(),Err(crate::human_authority::HumanAuthorityFailure::GateBusy)));
    drop(retained);
    assert_eq!(policy_call(&state,"POST",REPLACE,RUNNER,request).await.0,200);
    policy_drain(&state).await;
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn binding_revoked_after_real_policy_commit_cannot_publish_a_receipt() {
    use crate::control::review_binding::{ReviewBindingSnapshotV1,RevokeRunnerReviewBindingV1,ReviewBindingRevocationReasonV1};
    use tower::ServiceExt;
    let (_fixture,state) = policy_state(15);
    let owned = state.owned_human.as_ref().unwrap();
    let a = state.tenants.iter().position(|t|t.tenant_id=="A").unwrap();
    let path = owned.sources[a].descriptor().storage_binding.path();
    let db = rusqlite::Connection::open_with_flags(path,rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let original_id = crate::control::review_binding::tests::provision().binding.binding_id;
    let snapshot: String = db.query_row("SELECT snapshot_json FROM runner_review_bindings WHERE binding_id=?1",[original_id],|r|r.get(0)).unwrap();
    let binding: ReviewBindingSnapshotV1 = serde_json::from_str(&snapshot).unwrap();
    let (reached,release) = crate::api::auth::owned_control::test_hook::install(&state);
    let request = axum::http::Request::builder().method("POST").uri(REPLACE)
        .header("authorization",format!("Bearer {RUNNER}")).header("content-type","application/json")
        .body(axum::body::Body::from(serde_json::to_vec(&policy_body(&state)).unwrap())).unwrap();
    let router = gateway_router(state.clone());
    let observer = tokio::spawn(async move {router.oneshot(request).await});
    tokio::task::spawn_blocking(move || reached.recv_timeout(Duration::from_secs(15))).await.unwrap().unwrap();
    assert_eq!(db.query_row("SELECT count(*) FROM egress_policy_operations",[],|r|r.get::<_,i64>(0)).unwrap(),1);
    {
        // Success here also proves that the true write epoch ended before
        // the post-commit rendezvous; no epoch is held over this test wait.
        let epoch = owned.owner.try_write_epoch().unwrap();
        let result = state.tenants[a].store.revoke_review_binding(&epoch,&owned.sources[a],&RevokeRunnerReviewBindingV1 {
            version:1,operation_id:uuid::Uuid::new_v4().to_string(),operator_delegation_id:uuid::Uuid::new_v4().to_string(),
            binding_id:binding.binding.binding_id,expected_revision:binding.binding.revision,
            expected_binding_digest:binding.binding_digest,reason:ReviewBindingRevocationReasonV1::OperatorRevoked,
        }).unwrap();
        assert!(!result.snapshot.binding.active);
    }
    let ack = release.release_with_ack().expect("postcommit waiter must still accept release");
    tokio::task::spawn_blocking(move || ack.recv_timeout(Duration::from_secs(15))).await.unwrap()
        .expect("postcommit job must acknowledge receiving release before its refusal is accepted");
    let response = tokio::time::timeout(Duration::from_secs(30),observer).await.unwrap().unwrap().unwrap();
    assert_eq!(response.status().as_u16(),503);
    let body = axum::body::to_bytes(response.into_body(),4096).await.unwrap();
    assert_eq!(serde_json::from_slice::<serde_json::Value>(&body).unwrap()["error"]["code"],"egress_policy_unavailable");
    assert_eq!(db.query_row("SELECT count(*) FROM egress_policy_operations",[],|r|r.get::<_,i64>(0)).unwrap(),1);
    drop(db);
    policy_drain(&state).await;
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn aborted_policy_observer_retains_committed_work_and_operation_until_drain() {
    use tower::ServiceExt;
    let (_fixture,state) = policy_state(15);
    let index = state.tenants.iter().position(|t|t.tenant_id=="A").unwrap();
    let semaphore = state.tenants[index].control_capacity.runner_semaphore_for_test();
    let available = semaphore.available_permits();
    let (reached,release) = crate::api::auth::owned_control::test_hook::install(&state);
    let request = axum::http::Request::builder().method("POST").uri(REPLACE)
        .header("authorization",format!("Bearer {RUNNER}")).header("content-type","application/json")
        .body(axum::body::Body::from(serde_json::to_vec(&policy_body(&state)).unwrap())).unwrap();
    let router = gateway_router(state.clone());
    let observer = tokio::spawn(async move {router.oneshot(request).await});
    tokio::task::spawn_blocking(move || reached.recv_timeout(Duration::from_secs(15))).await.unwrap().unwrap();
    assert_eq!(semaphore.available_permits(),available-1);
    let owned = state.owned_human.as_ref().unwrap();
    assert_eq!(owned.usages.load(Ordering::Acquire),1);
    let path = owned.sources[index].descriptor().storage_binding.path();
    let db = rusqlite::Connection::open_with_flags(path,rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert_eq!(db.query_row("SELECT count(*) FROM egress_policy_operations",[],|r|r.get::<_,i64>(0)).unwrap(),1);
    drop(db);
    observer.abort(); let _ = observer.await;
    assert_eq!(semaphore.available_permits(),available-1);
    assert_eq!(owned.usages.load(Ordering::Acquire),1);
    let retirement = state.human_retirement().unwrap();
    retirement.retire();
    assert!(!owned.healthy());
    let ack = release.release_with_ack().expect("abandoned job must still accept release");
    tokio::task::spawn_blocking(move || ack.recv_timeout(Duration::from_secs(15))).await.unwrap()
        .expect("abandoned job must acknowledge receiving release before drainage is accepted");
    tokio::time::timeout(Duration::from_secs(30),retirement.drain()).await.unwrap().unwrap();
    assert_eq!(semaphore.available_permits(),available);
    assert_eq!(owned.usages.load(Ordering::Acquire),super::CLOSED);
}
