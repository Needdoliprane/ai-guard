//! Relay boundary regressions; no model or container runtime is invoked.
mod support;

use agentsandbox::{
    audit::Store,
    model::Event,
    redaction::Redactor,
    relay::{RelayAckV1, RelayEndpointV1, RelayGrantV1, RelayPruneV1, RelaySendV1},
};
use serde_json::json;
use uuid::Uuid;

fn endpoint() -> RelayEndpointV1 {
    RelayEndpointV1 {
        worker_id: Uuid::new_v4().to_string(),
        attempt_id: Uuid::new_v4().to_string(),
        generation: 1,
    }
}

#[test]
fn worker_payload_cannot_select_tenant_source_or_runtime_authority() {
    let send = json!({
        "version":1,"operation_id":Uuid::new_v4().to_string(),
        "grant_id":Uuid::new_v4().to_string(),"ttl_seconds":60,"body":"business message"
    });
    assert!(serde_json::from_value::<RelaySendV1>(send.clone()).is_ok());
    for field in [
        "tenant_id",
        "worker_id",
        "source",
        "attempt_id",
        "generation",
        "credential_id",
        "role",
        "authority_id",
    ] {
        let mut forged = send.clone();
        forged[field] = json!("untrusted selector");
        assert!(
            serde_json::from_value::<RelaySendV1>(forged).is_err(),
            "accepted {field}"
        );
    }
    let ack = json!({"version":1,"operation_id":Uuid::new_v4().to_string(),"message_id":Uuid::new_v4().to_string()});
    assert!(serde_json::from_value::<RelayAckV1>(ack.clone()).is_ok());
    assert!(serde_json::from_value::<RelayPruneV1>(ack.clone()).is_ok());
    let mut forged = ack;
    forged["tenant_id"] = json!("tenant-b");
    assert!(serde_json::from_value::<RelayAckV1>(forged.clone()).is_err());
    assert!(serde_json::from_value::<RelayPruneV1>(forged).is_err());
}

#[test]
fn generic_append_cannot_forge_relay_mutations_or_content_read_receipts() {
    let directory = support::private_tempdir();
    let store = Store::open_for_tenant(
        &directory.path().join("tenant.sqlite"),
        "tenant-a",
        Redactor::new(vec![]),
        1000,
    )
    .unwrap();
    let before = store.head().unwrap();
    for kind in [
        "control.relay.mutation",
        "control.relay.read",
        "control.relay.grant",
        "control.relay.refused",
    ] {
        let mut event = Event::new(kind);
        event.data =
            json!({"version":1,"operation_id":Uuid::new_v4().to_string(),"action":"grant"});
        assert!(
            store.append(event).is_err(),
            "generic append accepted {kind}"
        );
        assert_eq!(store.head().unwrap(), before);
    }
    assert!(store.integrity().unwrap().valid);
}

#[test]
fn relay_requires_explicit_schema_migration_and_never_repairs_it_on_a_request() {
    let directory = support::private_tempdir();
    let path = directory.path().join("tenant.sqlite");
    let store = Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    let before = store.head().unwrap();
    let input = RelayGrantV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        grant_id: Uuid::new_v4().to_string(),
        source: endpoint(),
        destination: endpoint(),
        channel: "tasks".into(),
        expires_at: (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339(),
    };
    assert!(store.relay_grant(&input).is_err());
    assert_eq!(store.head().unwrap(), before);
    assert!(store.integrity().unwrap().valid);
    let db = rusqlite::Connection::open(&path).unwrap();
    let count: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE name LIKE 'relay_%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0, "request silently installed relay tables");
    assert_eq!(
        db.pragma_query_value::<i64, _>(None, "user_version", |r| r.get(0))
            .unwrap(),
        2
    );
}
