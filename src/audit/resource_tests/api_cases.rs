//! Real authenticated routes and SQLite; only the filesystem fault is injected.
//! Tower exercises HTTP/auth/body contracts, not TCP availability under pressure.
use super::*;
use crate::{
    api::{GatewayState, TenantGateway, gateway_router},
    policy::Policy,
};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use rusqlite::{OpenFlags, OptionalExtension};
use tower::ServiceExt;

// Public synthetic credentials, never read from the host or a real installation.
const A_RUNNER: &str = "resource-api-a-runner-public-fixture";
const A_VIEWER: &str = "resource-api-a-viewer-public-fixture";
const B_RUNNER: &str = "resource-api-b-runner-public-fixture";
const B_VIEWER: &str = "resource-api-b-viewer-public-fixture";

struct ApiFixture {
    directory: tempfile::TempDir,
    app: Router,
    a: Arc<Store>,
    b: Arc<Store>,
    a_observer: Connection,
    b_observer: Connection,
    b_event: Event,
}

fn observer(path: &std::path::Path) -> Connection {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap()
}

async fn request(
    app: &Router,
    method: &str,
    path: &str,
    credential: &str,
    body: &str,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {credential}"))
        .header("content-type", "application/json")
        .header("x-tenant-id", "tenant-a")
        .body(Body::from(body.to_owned()))
        .unwrap();
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        app.clone().oneshot(request),
    )
    .await
    .unwrap()
    .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 32 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn fixture() -> ApiFixture {
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    // Preserve the exact Store objects in for_tenants: attaching this complete
    // masking dictionary needs no sibling with a different test-only hook.
    let redactor = Redactor::new(
        [A_RUNNER, A_VIEWER, B_RUNNER, B_VIEWER]
            .into_iter()
            .map(str::to_owned)
            .collect(),
    )
    .union(&Redactor::new(vec![]));
    let a_path = directory.path().join("a.sqlite");
    let b_path = directory.path().join("b.sqlite");
    let a = Arc::new(Store::open_for_tenant(&a_path, "tenant-a", redactor.clone(), 100).unwrap());
    let b = Arc::new(Store::open_for_tenant(&b_path, "tenant-b", redactor.clone(), 100).unwrap());
    assert!(Arc::ptr_eq(
        &a,
        &a.with_additional_redaction(&redactor).unwrap()
    ));
    let state = GatewayState::for_tenants(
        vec![
            TenantGateway {
                tenant_id: "tenant-a".into(),
                store: Arc::clone(&a),
                runner_token: A_RUNNER.into(),
                viewer_token: A_VIEWER.into(),
            },
            TenantGateway {
                tenant_id: "tenant-b".into(),
                store: Arc::clone(&b),
                runner_token: B_RUNNER.into(),
                viewer_token: B_VIEWER.into(),
            },
        ],
        Policy::default(),
        None,
        redactor,
    )
    .unwrap();
    let app = gateway_router(state);
    for (credential, tenant) in [(A_RUNNER, "tenant-a"), (B_RUNNER, "tenant-b")] {
        let (status, identity) = request(
            &app,
            "GET",
            "/api/identity?tenant_id=tenant-b",
            credential,
            "",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(identity, json!({"tenant_id": tenant, "role": "runner"}));
    }
    let b_event = Event::new("fixture.api_resource_b");
    let (status, receipt) = request(
        &app,
        "POST",
        "/api/events?tenant_id=tenant-a",
        B_RUNNER,
        &serde_json::to_string(&b_event).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(receipt["id"], b_event.id);
    ApiFixture {
        directory,
        app,
        a,
        b,
        a_observer: observer(&a_path),
        b_observer: observer(&b_path),
        b_event,
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    head: i64,
    count: i64,
    bytes: i64,
    copies: i64,
}

fn snapshot(db: &Connection, event_id: &str) -> Snapshot {
    db.query_row(
        "SELECT (SELECT max(seq) FROM events),(SELECT count(*) FROM events),bytes,(SELECT count(*) FROM events WHERE id=?1) FROM quota WHERE id=1",
        [event_id],
        |row| Ok(Snapshot { head: row.get(0)?, count: row.get(1)?, bytes: row.get(2)?, copies: row.get(3)? }),
    ).unwrap()
}

struct PostcommitFault {
    enabled: Arc<AtomicBool>,
    failures: Arc<AtomicUsize>,
    receipt: Arc<Mutex<Option<StoredEvent>>>,
}

fn fail_after_commit(fixture: &ApiFixture, event_id: &str, code: i32) -> PostcommitFault {
    // Open before the request; this observer cannot see an uncommitted append
    // on the Store's connection and does not call a Store validation/recovery.
    let db = observer(&fixture.directory.path().join("a.sqlite"));
    let event_id = event_id.to_owned();
    assert_eq!(snapshot(&db, &event_id).copies, 0);
    let fault = PostcommitFault {
        enabled: Arc::new(AtomicBool::new(true)),
        failures: Arc::new(AtomicUsize::new(0)),
        receipt: Arc::new(Mutex::new(None)),
    };
    let enabled = Arc::clone(&fault.enabled);
    let failures = Arc::clone(&fault.failures);
    let receipt = Arc::clone(&fault.receipt);
    fixture.a.backing.test_hook.set(move || {
        if enabled.load(Ordering::SeqCst) {
            let committed = db
                .query_row(
                    "SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE id=?1",
                    [&event_id],
                    row_event,
                )
                .optional()?;
            if let Some(committed) = committed {
                *receipt.lock().unwrap() = Some(committed);
                failures.fetch_add(1, Ordering::SeqCst);
                return Err(io::Error::from_raw_os_error(code))
                    .context("injected API postcommit metadata failure");
            }
        }
        Ok(())
    });
    fault
}

fn assert_unavailable(status: StatusCode, body: &Value) {
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|error| !error.is_empty())
    );
    assert!(body.get("id").is_none());
    assert!(body.get("seq").is_none());
    assert!(body.get("hash").is_none());
}

fn assert_committed_once(db: &Connection, before: &Snapshot, receipt: &StoredEvent) -> Snapshot {
    let after = snapshot(db, &receipt.event.id);
    let payload_bytes: i64 = db
        .query_row(
            "SELECT length(CAST(payload AS BLOB)) FROM events WHERE id=?1",
            [&receipt.event.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(before.copies, 0);
    assert_eq!(after.copies, 1);
    assert_eq!(receipt.seq, before.head + 1);
    assert_eq!(after.head, receipt.seq);
    assert_eq!(after.count, before.count + 1);
    assert_eq!(after.bytes, before.bytes + payload_bytes);
    after
}

async fn assert_b_scope(fixture: &ApiFixture, excluded: &str) {
    let (status, identity) = request(
        &fixture.app,
        "GET",
        "/api/identity?tenant_id=tenant-a",
        B_VIEWER,
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(identity, json!({"tenant_id":"tenant-b", "role":"viewer"}));
    let (status, history) = request(
        &fixture.app,
        "GET",
        "/api/events?tenant_id=tenant-a&order=asc",
        B_VIEWER,
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let events = history["items"].as_array().unwrap();
    assert!(events.iter().any(|event| event["id"] == fixture.b_event.id));
    assert!(events.iter().all(|event| event["id"] != excluded));
    assert!(fixture.b.healthy());
}

#[tokio::test]
async fn authenticated_postcommit_descriptor_shortage_recovers_exact_replay_without_tenant_leak() {
    let fixture = fixture().await;
    let event = Event::new("fixture.api_resource_retry");
    let wire = serde_json::to_string(&event).unwrap();
    let before_a = snapshot(&fixture.a_observer, &event.id);
    let before_b = snapshot(&fixture.b_observer, &event.id);
    let fault = fail_after_commit(&fixture, &event.id, libc::EMFILE);
    let (status, body) = request(&fixture.app, "POST", "/api/events", A_RUNNER, &wire).await;
    assert_unavailable(status, &body);
    assert_eq!(
        body["error"],
        "Audit indisponible, incohérent ou plein : opération refusée."
    );
    assert_eq!(fault.failures.load(Ordering::SeqCst), 1);
    let committed = fault.receipt.lock().unwrap().clone().unwrap();
    assert_eq!(
        serde_json::to_value(&committed.event).unwrap(),
        json!(event)
    );
    let after_commit = assert_committed_once(&fixture.a_observer, &before_a, &committed);
    assert!(!fixture.a.healthy());
    // B stays independently authorized while A's filesystem remains unavailable.
    // Auth may inspect A for collisions; no assumption depends on that count.
    assert_b_scope(&fixture, &event.id).await;
    assert_eq!(snapshot(&fixture.b_observer, &event.id), before_b);
    fault.enabled.store(false, Ordering::SeqCst);
    // The next ordinary authenticated A request must validate and recover the
    // same live Store, then replay the identical bytes with no second debit.
    let (status, replay) = request(&fixture.app, "POST", "/api/events", A_RUNNER, &wire).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay, serde_json::to_value(&committed).unwrap());
    assert!(fixture.a.healthy());
    assert_eq!(snapshot(&fixture.a_observer, &event.id), after_commit);
    let (status, history) = request(
        &fixture.app,
        "GET",
        "/api/events?tenant_id=tenant-b&order=asc",
        A_VIEWER,
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let events = history["items"].as_array().unwrap();
    assert!(events.iter().any(|event| event == &replay));
    assert!(events.iter().all(|event| event["id"] != fixture.b_event.id));
    assert_b_scope(&fixture, &event.id).await;
    assert_eq!(snapshot(&fixture.a_observer, &event.id), after_commit);
    assert_eq!(snapshot(&fixture.b_observer, &event.id), before_b);
}

#[tokio::test]
async fn authenticated_postcommit_io_failure_keeps_tenant_fenced_after_fault_clears() {
    let fixture = fixture().await;
    let event = Event::new("fixture.api_permanent_io");
    let wire = serde_json::to_string(&event).unwrap();
    let before_a = snapshot(&fixture.a_observer, &event.id);
    let before_b = snapshot(&fixture.b_observer, &event.id);
    let fault = fail_after_commit(&fixture, &event.id, libc::EIO);
    let (status, body) = request(&fixture.app, "POST", "/api/events", A_RUNNER, &wire).await;
    assert_unavailable(status, &body);
    assert_eq!(fault.failures.load(Ordering::SeqCst), 1);
    let committed = fault.receipt.lock().unwrap().clone().unwrap();
    assert_eq!(committed.event.id, event.id);
    let after_commit = assert_committed_once(&fixture.a_observer, &before_a, &committed);
    assert!(!fixture.a.healthy());
    assert_b_scope(&fixture, &event.id).await;
    fault.enabled.store(false, Ordering::SeqCst);
    for (method, path, credential, body) in [
        (
            "POST",
            "/api/events?tenant_id=tenant-b",
            A_RUNNER,
            wire.as_str(),
        ),
        ("GET", "/api/events?tenant_id=tenant-b", A_VIEWER, ""),
        ("GET", "/api/identity?tenant_id=tenant-b", A_RUNNER, ""),
    ] {
        let (status, body) = request(&fixture.app, method, path, credential, body).await;
        assert_unavailable(status, &body);
    }
    assert!(!fixture.a.healthy());
    assert_eq!(snapshot(&fixture.a_observer, &event.id), after_commit);
    assert_eq!(snapshot(&fixture.b_observer, &event.id), before_b);
    // The fence neither promotes A's credential into B nor prevents B's own
    // runner from committing a fresh event in its independently valid Store.
    let b_next = Event::new("fixture.api_b_after_permanent_io");
    let before_b_next = snapshot(&fixture.b_observer, &b_next.id);
    let (status, receipt) = request(
        &fixture.app,
        "POST",
        "/api/events?tenant_id=tenant-a",
        B_RUNNER,
        &serde_json::to_string(&b_next).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let receipt: StoredEvent = serde_json::from_value(receipt).unwrap();
    assert_eq!(receipt.event.id, b_next.id);
    assert_committed_once(&fixture.b_observer, &before_b_next, &receipt);
    assert_b_scope(&fixture, &event.id).await;
    assert!(!fixture.a.healthy());
    assert_eq!(snapshot(&fixture.a_observer, &event.id), after_commit);
}
