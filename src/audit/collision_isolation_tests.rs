//! Request completion is ordered before releasing the neighbor's actual lock.
//! The timeout only bounds a broken harness; it is not a latency qualification.
use super::*;
use crate::{
    api::{GatewayState, TenantGateway, gateway_router},
    policy::Policy,
};
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};
use tower::ServiceExt;

const HARNESS_LIMIT: Duration = Duration::from_secs(30);
// Installed only on this test's A connection. Refusing each retry makes the
// conflicting write-lock attempt itself observable without timing SQLite.
static NEIGHBOR_BUSY_ATTEMPTS: AtomicUsize = AtomicUsize::new(0);
fn refuse_neighbor_busy(_: i32) -> bool {
    NEIGHBOR_BUSY_ATTEMPTS.fetch_add(1, Ordering::SeqCst);
    false
}
struct RefuseBusy(Arc<Store>);
impl RefuseBusy {
    fn install(store: &Arc<Store>) -> Self {
        store
            .db
            .lock()
            .unwrap()
            .busy_handler(Some(refuse_neighbor_busy))
            .unwrap();
        Self(Arc::clone(store))
    }
}
impl Drop for RefuseBusy {
    fn drop(&mut self) {
        if let Ok(db) = self.0.db.lock() {
            let _ = db.busy_timeout(Duration::from_secs(5));
        }
    }
}

struct HeldLock {
    release: Option<mpsc::Sender<()>>,
    holder: Option<JoinHandle<()>>,
}
impl HeldLock {
    fn start(hold: impl FnOnce(mpsc::SyncSender<()>, mpsc::Receiver<()>) + Send + 'static) -> Self {
        let (ready, reached) = mpsc::sync_channel(1);
        let (release, released) = mpsc::channel();
        let holder = thread::spawn(move || hold(ready, released));
        let held = Self {
            release: Some(release),
            holder: Some(holder),
        };
        reached.recv_timeout(HARNESS_LIMIT).unwrap();
        held
    }
    fn mutex(store: Arc<Store>) -> Self {
        Self::start(move |ready, released| {
            let _guard = store.db.lock().unwrap();
            ready.send(()).unwrap();
            released.recv().unwrap();
        })
    }
    fn lease(store: Arc<Store>) -> Self {
        Self::start(move |ready, released| {
            let _guard = store.supervision_lease.state.lock().unwrap();
            ready.send(()).unwrap();
            released.recv().unwrap();
        })
    }
    fn sqlite(path: std::path::PathBuf) -> Self {
        Self::start(move |ready, released| {
            let mut connection = Connection::open(path).unwrap();
            let transaction = connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .unwrap();
            ready.send(()).unwrap();
            released.recv().unwrap();
            transaction.rollback().unwrap();
        })
    }
}
impl Drop for HeldLock {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
        if let Some(holder) = self.holder.take() {
            holder.join().unwrap();
        }
    }
}

fn tenant(path: &Path, id: &str, runner: char, viewer: char, redactor: &Redactor) -> TenantGateway {
    TenantGateway {
        tenant_id: id.into(),
        store: Arc::new(Store::open_for_tenant(path, id, redactor.clone(), 1000).unwrap()),
        runner_token: runner.to_string().repeat(48),
        viewer_token: viewer.to_string().repeat(48),
    }
}
fn fixture() -> (tempfile::TempDir, Router, Arc<Store>, std::path::PathBuf) {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    let mut builder = tempfile::Builder::new();
    #[cfg(unix)]
    builder.permissions(std::fs::Permissions::from_mode(0o700));
    let directory = builder.tempdir().unwrap();
    // GatewayState attaches the union of every control token to every Store.
    // Cover it at open time so the gateway retains these exact Arc/connection
    // instances, rather than creating siblings with independent DB mutexes.
    let redactor = Redactor::new(
        ['a', 'v', 'b', 'w']
            .map(|token| token.to_string().repeat(48))
            .to_vec(),
    );
    let a_path = directory.path().join("a.sqlite");
    let a = tenant(&a_path, "tenant-a", 'a', 'v', &redactor);
    let a_store = Arc::clone(&a.store);
    let b = tenant(
        &directory.path().join("b.sqlite"),
        "tenant-b",
        'b',
        'w',
        &redactor,
    );
    for store in [&a.store, &b.store] {
        assert!(Arc::ptr_eq(
            store,
            &store.with_additional_redaction(&redactor).unwrap()
        ));
    }
    let state = GatewayState::for_tenants(vec![a, b], Policy::default(), None, redactor).unwrap();
    (directory, gateway_router(state), a_store, a_path)
}
async fn request(
    app: &Router,
    method: &str,
    path: &str,
    token: char,
    body: Value,
) -> (StatusCode, Value) {
    let body = if body.is_null() {
        Body::empty()
    } else {
        Body::from(body.to_string())
    };
    request_body(app, method, path, token, body).await
}
async fn request_body(
    app: &Router,
    method: &str,
    path: &str,
    token: char,
    body: Body,
) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header(
                    "authorization",
                    format!("Bearer {}", token.to_string().repeat(48)),
                )
                .header("content-type", "application/json")
                // A free tenant hint must never choose A for B's credential.
                .header("x-tenant-id", "tenant-a")
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}
async fn independent_requests(app: Router) -> [(StatusCode, Value); 3] {
    let polled = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&polled);
    let workload_body = Body::from_stream(futures_util::stream::once(async move {
        observed.store(true, Ordering::SeqCst);
        Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"must not be read"))
    }));
    let (identity, decide, workload) = tokio::join!(
        request(&app, "GET", "/api/v1/identity", 'w', Value::Null),
        request(
            &app,
            "POST",
            "/api/v1/decide",
            'b',
            json!({
                "request_id": uuid::Uuid::new_v4(), "profile":"strict",
                "command":{"argv":["id"],"cwd":"/workspace"}
            })
        ),
        request_body(&app, "POST", "/api/v1/relay/messages", 'x', workload_body),
    );
    assert!(
        !polled.load(Ordering::SeqCst),
        "unknown workload body was polled before authentication"
    );
    [identity, decide, workload]
}
fn assert_independent_responses(responses: [(StatusCode, Value); 3]) {
    let [identity, decide, workload] = responses;
    assert_eq!(
        identity,
        (
            StatusCode::OK,
            json!({"tenant_id":"tenant-b","role":"viewer"})
        )
    );
    assert_eq!(decide.0, StatusCode::OK);
    // An unknown proof has no independently verified source. Even the healthy
    // baseline bundle may observe a busy B connection while its two positive
    // requests run concurrently. Unknown metadata never proves an absent owner.
    assert!(matches!(
        workload.0,
        StatusCode::UNAUTHORIZED | StatusCode::SERVICE_UNAVAILABLE
    ));
    assert!(workload.1.get("tenant_id").is_none());
}
async fn completes_before_release(app: Router, held: HeldLock) {
    let mut requests = tokio::spawn(independent_requests(app));
    let completed = tokio::time::timeout(HARNESS_LIMIT, &mut requests).await;
    // Always release and join the holder and request tasks before the oracle,
    // including the expected RED path. No blocked thread escapes this test.
    drop(held);
    let responses = match completed {
        Ok(result) => result.unwrap(),
        Err(_) => {
            let _ = tokio::time::timeout(HARNESS_LIMIT, requests)
                .await
                .unwrap()
                .unwrap();
            panic!("tenant B requests did not complete while tenant A's lock remained held");
        }
    };
    assert_independent_responses(responses);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn neighbor_mutex_never_delays_fresh_control_or_workload_authentication() {
    let (_directory, app, a_store, _a_path) = fixture();
    assert_independent_responses(independent_requests(app.clone()).await);
    completes_before_release(app, HeldLock::mutex(a_store)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn neighbor_lease_never_delays_fresh_control_or_workload_authentication() {
    let (_directory, app, a_store, _a_path) = fixture();
    assert_independent_responses(independent_requests(app.clone()).await);
    completes_before_release(app, HeldLock::lease(a_store)).await;
}

#[test]
fn neighbor_metadata_reads_the_validated_snapshot_under_an_external_writer() {
    let (_directory, _app, a_store, a_path) = fixture();
    let before: i64 = a_store
        .db
        .lock()
        .unwrap()
        .pragma_query_value(None, "busy_timeout", |row| row.get(0))
        .unwrap();
    let held = HeldLock::sqlite(a_path);
    let local = a_store.try_local_credential_binding(&[7; 32]);
    let workload = a_store.try_workload_digest_binding(&[7; 32]);
    drop(held);
    assert!(matches!(local, MetadataLookup::Absent));
    assert!(matches!(workload, MetadataLookup::Absent));
    let after: i64 = a_store
        .db
        .lock()
        .unwrap()
        .pragma_query_value(None, "busy_timeout", |row| row.get(0))
        .unwrap();
    assert_eq!(before, after);
}

#[test]
fn changed_metadata_is_unavailable_until_the_source_validates_it() {
    let (_directory, _app, a_store, a_path) = fixture();
    let external = Connection::open(a_path).unwrap();
    external
        .execute("UPDATE events SET hash=?1", ["0".repeat(64)])
        .unwrap();
    assert!(matches!(
        a_store.try_local_credential_binding(&[7; 32]),
        MetadataLookup::Unavailable(_)
    ));
    assert!(a_store.local_credential_binding(&[7; 32]).is_err());
    assert!(!a_store.healthy());
}

#[tokio::test]
async fn sibling_binding_mutations_preserve_historical_control_collision_refusal() {
    use crate::workload::{WorkloadBindingIssueV1, WorkloadBindingRevocationV1, WorkloadSubjectV1};
    let (_directory, app, a_store, a_path) = fixture();
    a_store
        .migrate_control_v3(&a_path.with_extension("before-v3"))
        .unwrap();
    a_store
        .migrate_identity_v4(&a_path.with_extension("before-v4"))
        .unwrap();
    a_store
        .migrate_human_access_v5(&a_path.with_extension("before-v5"))
        .unwrap();
    a_store
        .migrate_workload_v6(&a_path.with_extension("before-v6"))
        .unwrap();
    let sibling = a_store.sibling().unwrap();
    let old_digest = Sha256::digest("b".repeat(48).as_bytes());
    let mut binding = WorkloadBindingIssueV1 {
        version: 1,
        operation_id: uuid::Uuid::new_v4().to_string(),
        credential_id: uuid::Uuid::new_v4().to_string(),
        expected_revision: 0,
        subject: WorkloadSubjectV1::Broker {
            broker_id: uuid::Uuid::new_v4().to_string(),
            incarnation_id: uuid::Uuid::new_v4().to_string(),
        },
        sha256_digest: hex::encode(old_digest),
        expires_at: (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339(),
    };
    sibling.issue_workload_binding(&binding).unwrap();
    assert_eq!(
        request(&app, "GET", "/api/v1/identity", 'b', Value::Null)
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    binding.operation_id = uuid::Uuid::new_v4().to_string();
    binding.expected_revision = 1;
    binding.sha256_digest = hex::encode(Sha256::digest("c".repeat(48).as_bytes()));
    sibling.issue_workload_binding(&binding).unwrap();
    assert_eq!(
        request(&app, "GET", "/api/v1/identity", 'b', Value::Null)
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    sibling
        .revoke_workload_binding(&WorkloadBindingRevocationV1 {
            version: 1,
            operation_id: uuid::Uuid::new_v4().to_string(),
            credential_id: binding.credential_id,
            expected_revision: 2,
        })
        .unwrap();
    collision_refused_before_release(app.clone(), HeldLock::mutex(Arc::clone(&a_store))).await;
    collision_refused_before_release(app, HeldLock::lease(a_store)).await;
}

async fn collision_refused_before_release(app: Router, held: HeldLock) {
    let mut lookup =
        tokio::spawn(
            async move { request(&app, "GET", "/api/v1/identity", 'b', Value::Null).await },
        );
    let completed = tokio::time::timeout(HARNESS_LIMIT, &mut lookup).await;
    drop(held);
    let response = match completed {
        Ok(response) => response.unwrap(),
        Err(_) => {
            let _ = tokio::time::timeout(HARNESS_LIMIT, lookup)
                .await
                .unwrap()
                .unwrap();
            panic!("a historical collision lookup waited on its neighbor's mutex");
        }
    };
    assert_eq!(response.0, StatusCode::SERVICE_UNAVAILABLE);
}

/// A real SQLite commit is followed by an injected EMFILE only at the existing
/// postcommit filesystem seam. No process-wide FD exhaustion is performed;
/// this does not qualify real EIO/ENOSPC, OS pressure or runtime availability.
#[tokio::test]
async fn postcommit_descriptor_fault_recovery_and_replay_preserve_known_collision() {
    use crate::workload::{WorkloadBindingIssueV1, WorkloadSubjectV1};
    let (_directory, app, a_store, a_path) = fixture();
    a_store
        .migrate_control_v3(&a_path.with_extension("before-v3"))
        .unwrap();
    a_store
        .migrate_identity_v4(&a_path.with_extension("before-v4"))
        .unwrap();
    a_store
        .migrate_human_access_v5(&a_path.with_extension("before-v5"))
        .unwrap();
    a_store
        .migrate_workload_v6(&a_path.with_extension("before-v6"))
        .unwrap();
    let sibling = a_store.sibling().unwrap();
    let binding = WorkloadBindingIssueV1 {
        version: 1,
        operation_id: uuid::Uuid::new_v4().to_string(),
        credential_id: uuid::Uuid::new_v4().to_string(),
        expected_revision: 0,
        subject: WorkloadSubjectV1::Broker {
            broker_id: uuid::Uuid::new_v4().to_string(),
            incarnation_id: uuid::Uuid::new_v4().to_string(),
        },
        sha256_digest: hex::encode(Sha256::digest("b".repeat(48).as_bytes())),
        expires_at: (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339(),
    };
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&calls);
    sibling.backing.test_hook.set(move || {
        if observed.fetch_add(1, Ordering::SeqCst) + 1 == 4 {
            return Err(std::io::Error::from_raw_os_error(libc::EMFILE))
                .context("injected postcommit identity metadata descriptor shortage");
        }
        Ok(())
    });
    let error = sibling.issue_workload_binding(&binding).unwrap_err();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        4,
        "fault did not reach the exact postcommit seam"
    );
    assert!(storage::descriptor_shortage(&error));
    assert!(!sibling.healthy() && !a_store.healthy());
    // This raw connection observes the durable mutation without recovering
    // either Store or accidentally populating its positive-binding cache.
    let external = Connection::open(&a_path).unwrap();
    let committed_seq: i64 = external
        .query_row(
            "SELECT event_seq FROM workload_operations WHERE operation_id=?1",
            [&binding.operation_id],
            |row| row.get(0),
        )
        .unwrap();
    let committed_count: i64 = external
        .query_row("SELECT count(*) FROM events", [], |row| row.get(0))
        .unwrap();
    let receipt = sibling.issue_workload_binding(&binding).unwrap();
    assert!(receipt.replayed);
    assert_eq!(receipt.audit_seq, committed_seq);
    assert!(
        sibling.healthy() && a_store.healthy(),
        "transient EMFILE must recover without quarantine"
    );
    let replayed_count: i64 = external
        .query_row("SELECT count(*) FROM events", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        replayed_count, committed_count,
        "exact replay appended another event"
    );
    collision_refused_before_release(app, HeldLock::mutex(a_store)).await;
}

#[tokio::test]
async fn independent_origin_binding_validated_by_integrity_remains_a_known_collision() {
    use crate::workload::{WorkloadBindingIssueV1, WorkloadSubjectV1};
    let (_directory, app, a_store, a_path) = fixture();
    a_store
        .migrate_control_v3(&a_path.with_extension("before-v3"))
        .unwrap();
    a_store
        .migrate_identity_v4(&a_path.with_extension("before-v4"))
        .unwrap();
    a_store
        .migrate_human_access_v5(&a_path.with_extension("before-v5"))
        .unwrap();
    a_store
        .migrate_workload_v6(&a_path.with_extension("before-v6"))
        .unwrap();
    // This is an independent open of the exact same SQLite backing, with its
    // own Origin and positive index. It is neither sibling() nor a redaction
    // attachment; no in-memory publication may leak from A1 into gateway A0.
    let independent =
        Store::open_for_tenant(&a_path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    assert!(a_store.backing.identity_key() == independent.backing.identity_key());
    assert!(!Arc::ptr_eq(
        &a_store.supervision_lease,
        &independent.supervision_lease,
    ));
    let digest: [u8; 32] = Sha256::digest("b".repeat(48).as_bytes()).into();
    let binding = WorkloadBindingIssueV1 {
        version: 1,
        operation_id: uuid::Uuid::new_v4().to_string(),
        credential_id: uuid::Uuid::new_v4().to_string(),
        expected_revision: 0,
        subject: WorkloadSubjectV1::Broker {
            broker_id: uuid::Uuid::new_v4().to_string(),
            incarnation_id: uuid::Uuid::new_v4().to_string(),
        },
        sha256_digest: hex::encode(digest),
        expires_at: (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339(),
    };
    let receipt = independent.issue_workload_binding(&binding).unwrap();
    let integrity = a_store.integrity().unwrap();
    assert!(integrity.valid);
    assert!(integrity.event_count >= u64::try_from(receipt.audit_seq).unwrap());
    // A later checked lookup must not hide the publication gap by silently
    // treating integrity()'s already advanced validation markers as new work.
    let owner = a_store.workload_digest_binding(&digest).unwrap().unwrap();
    assert_eq!(owner.credential_id, binding.credential_id);
    collision_refused_before_release(app, HeldLock::mutex(a_store)).await;
}

#[tokio::test]
async fn dropping_a_checked_read_preserves_its_validated_external_collision() {
    use crate::workload::{WorkloadBindingIssueV1, WorkloadSubjectV1};
    let (_directory, app, a_store, a_path) = fixture();
    a_store
        .migrate_control_v3(&a_path.with_extension("before-v3"))
        .unwrap();
    a_store
        .migrate_identity_v4(&a_path.with_extension("before-v4"))
        .unwrap();
    a_store
        .migrate_human_access_v5(&a_path.with_extension("before-v5"))
        .unwrap();
    a_store
        .migrate_workload_v6(&a_path.with_extension("before-v6"))
        .unwrap();
    let independent =
        Store::open_for_tenant(&a_path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    assert!(!Arc::ptr_eq(
        &a_store.supervision_lease,
        &independent.supervision_lease,
    ));
    let digest: [u8; 32] = Sha256::digest("b".repeat(48).as_bytes()).into();
    independent
        .issue_workload_binding(&WorkloadBindingIssueV1 {
            version: 1,
            operation_id: uuid::Uuid::new_v4().to_string(),
            credential_id: uuid::Uuid::new_v4().to_string(),
            expected_revision: 0,
            subject: WorkloadSubjectV1::Broker {
                broker_id: uuid::Uuid::new_v4().to_string(),
                incarnation_id: uuid::Uuid::new_v4().to_string(),
            },
            sha256_digest: hex::encode(digest),
            expires_at: (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339(),
        })
        .unwrap();
    {
        let mut db = a_store.db.lock().unwrap();
        let checked = a_store.begin_checked(&mut db).unwrap();
        let present: bool = checked
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM workload_digests WHERE sha256_digest=?1)",
                [hex::encode(digest)],
                |row| row.get(0),
            )
            .unwrap();
        assert!(present);
        // No mutation is attempted. A caller may return a business error after
        // this successful validation; dropping the checked transaction rolls
        // it back, but cannot make already committed external ownership unknown.
        drop(checked);
    }
    assert!(a_store.healthy());
    collision_refused_before_release(app, HeldLock::mutex(a_store)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn neighbor_sqlite_writer_never_delays_but_own_store_stays_fail_closed() {
    let (_directory, app, a_store, a_path) = fixture();
    assert_independent_responses(independent_requests(app.clone()).await);
    let busy_policy = RefuseBusy::install(&a_store);
    NEIGHBOR_BUSY_ATTEMPTS.store(0, Ordering::SeqCst);
    completes_before_release(app.clone(), HeldLock::sqlite(a_path.clone())).await;
    let neighbor_attempts = NEIGHBOR_BUSY_ATTEMPTS.load(Ordering::SeqCst);
    assert!(
        a_store.healthy(),
        "a transient external writer must not quarantine A"
    );

    let held = HeldLock::sqlite(a_path);
    let response = request(&app, "GET", "/api/v1/identity", 'v', Value::Null).await;
    drop(held);
    assert_eq!(response.0, StatusCode::SERVICE_UNAVAILABLE);
    assert!(response.1.get("tenant_id").is_none());
    assert!(
        a_store.healthy(),
        "a transient external writer must not quarantine A"
    );
    drop(busy_policy);
    assert_eq!(
        neighbor_attempts, 0,
        "B's neighboring metadata lookup requested A's SQLite write lock"
    );
}
