//! Reserved-kind regressions on a private SQLite fixture. No listener, runtime,
//! provider, or real credential is used.
mod support;

use agentsandbox::{
    api::{GatewayState, gateway_router},
    audit::Store,
    model::{Event, StoredEvent},
    policy::Policy,
    redaction::Redactor,
};
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use rusqlite::{Connection, OpenFlags, types::Value};
use std::{path::Path, path::PathBuf, sync::Arc};
use tower::ServiceExt;

fn fixture() -> (tempfile::TempDir, PathBuf, Arc<Store>) {
    let directory = support::private_tempdir();
    let path = directory.path().join("tenant.sqlite");
    let store = Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    store
        .migrate_control_v3(&path.with_extension("v3.backup"))
        .unwrap();
    store
        .migrate_identity_v4(&path.with_extension("v4.backup"))
        .unwrap();
    store
        .migrate_human_access_v5(&path.with_extension("v5.backup"))
        .unwrap();
    store
        .migrate_workload_v6(&path.with_extension("v6.backup"))
        .unwrap();
    let db = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert_eq!(
        db.pragma_query_value::<i64, _>(None, "user_version", |row| row.get(0))
            .unwrap(),
        6
    );
    assert!(store.healthy());
    assert!(store.integrity().unwrap().valid);
    (directory, path, Arc::new(store))
}

/// Preserve the stored values themselves, including the original payload bytes,
/// rather than deserialize and serialize historical events again.
fn audit_rows(path: &Path) -> Vec<Vec<Value>> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let mut query = db.prepare("SELECT seq,id,received_at,prev_hash,hash,payload,kind,agent_id,sandbox_id,project_id,decision FROM events ORDER BY seq").unwrap();
    query
        .query_map([], |row| {
            (0..11)
                .map(|column| row.get::<_, Value>(column))
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

fn assert_reopens_unchanged(path: &Path, before: &[Vec<Value>]) {
    let store =
        Store::open_existing_for_tenant(path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    assert!(
        store.healthy(),
        "ordinary ingestion left the reopened audit unhealthy"
    );
    assert!(store.integrity().unwrap().valid);
    assert_eq!(
        audit_rows(path),
        before,
        "opening the audit rewrote stored event values"
    );
}

fn app(store: Arc<Store>) -> Router {
    gateway_router(GatewayState::new(
        store,
        "r".repeat(48),
        "v".repeat(48),
        Policy::default(),
        None,
        Redactor::new(vec![]),
    ))
}

fn ingest(path: &str, event: &Event) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", "r".repeat(48)))
        .body(Body::from(serde_json::to_vec(event).unwrap()))
        .unwrap()
}

#[test]
fn ordinary_store_append_refuses_control_case_variants_before_audit_write() {
    let (_directory, path, store) = fixture();
    let before = audit_rows(&path);
    let head = store.head().unwrap();
    for kind in [
        "control.relay.x",
        "Control.relay.x",
        "CONTROL.relay.x",
        "control.RELAY.x",
        "CONTROL.schema.migrated",
    ] {
        let event = Event::new(kind);
        event.validate().unwrap();
        let result = store.append(event);
        assert!(
            result.is_err(),
            "ordinary Store append accepted reserved kind {kind}"
        );
        assert_eq!(
            store.head().unwrap(),
            head,
            "refused {kind} advanced the audit head"
        );
        assert_eq!(
            audit_rows(&path),
            before,
            "refused {kind} changed the audit rows"
        );
    }
    drop(store);
    assert_reopens_unchanged(&path, &before);
}

#[test]
fn ordinary_store_mixed_case_kinds_keep_their_bytes_after_reopen() {
    let (_directory, path, store) = fixture();
    let initial_head = store.head().unwrap();
    for kind in [
        "Fixture.MixedCase",
        "ControlPlane.relay.x",
        "Security.decision_reported",
    ] {
        let stored = store.append(Event::new(kind)).unwrap();
        assert_eq!(stored.event.kind, kind, "ordinary kind was normalized");
    }
    assert_eq!(store.head().unwrap(), initial_head + 3);
    let before = audit_rows(&path);
    drop(store);
    assert_reopens_unchanged(&path, &before);
}

#[tokio::test]
async fn both_ingestion_aliases_refuse_reserved_case_variants_before_audit_write() {
    let (_directory, path, store) = fixture();
    let app = app(Arc::clone(&store));
    let before = audit_rows(&path);
    let head = store.head().unwrap();
    for path_alias in ["/api/events", "/api/v1/events"] {
        for kind in [
            "control.relay.x",
            "Control.relay.x",
            "control.RELAY.x",
            "SECURITY.decision",
            "security.DECISION.extra",
            "WORKER.start.claimed",
            "USAGE.Allocation.terminal",
        ] {
            let event = Event::new(kind);
            event.validate().unwrap();
            let response = app
                .clone()
                .oneshot(ingest(path_alias, &event))
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::FORBIDDEN,
                "{path_alias} accepted reserved kind {kind}"
            );
            assert_eq!(
                store.head().unwrap(),
                head,
                "refused {kind} advanced the audit head"
            );
            assert_eq!(
                audit_rows(&path),
                before,
                "refused {kind} changed the audit rows"
            );
        }
    }
    drop(app);
    drop(store);
    assert_reopens_unchanged(&path, &before);
}

#[tokio::test]
async fn both_ingestion_aliases_preserve_ordinary_mixed_case_kinds() {
    let (_directory, path, store) = fixture();
    let app = app(Arc::clone(&store));
    let initial_head = store.head().unwrap();
    for path_alias in ["/api/events", "/api/v1/events"] {
        for kind in [
            "Fixture.MixedCase",
            "ControlPlane.relay.x",
            "Security.decision_reported",
        ] {
            let response = app
                .clone()
                .oneshot(ingest(path_alias, &Event::new(kind)))
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "{path_alias} refused ordinary kind {kind}"
            );
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            let stored: StoredEvent = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(stored.event.kind, kind, "ordinary kind was normalized");
        }
    }
    assert_eq!(store.head().unwrap(), initial_head + 6);
    let before = audit_rows(&path);
    drop(app);
    drop(store);
    assert_reopens_unchanged(&path, &before);
}
