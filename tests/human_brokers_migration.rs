mod support;

use agentsandbox::{
    audit::{EventQuery, Store},
    identity::*,
    model::Event,
    redaction::Redactor,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::process::{Command, Stdio};

fn v6(path: &std::path::Path, capacity: u64) -> Store {
    let store = Store::open_for_tenant(path, "tenant-a", Redactor::new(vec![]), capacity).unwrap();
    store
        .migrate_control_v3(&path.with_extension("before3"))
        .unwrap();
    store
        .migrate_identity_v4(&path.with_extension("before4"))
        .unwrap();
    store
        .migrate_human_access_v5(&path.with_extension("before5"))
        .unwrap();
    store
        .migrate_workload_v6(&path.with_extension("before6"))
        .unwrap();
    store
}
fn mutation(change: IdentityChangeV1) -> IdentityMutationV1 {
    IdentityMutationV1 {
        version: 1,
        operation_id: uuid::Uuid::new_v4().to_string(),
        actor: PrincipalRefV1::LegacyControl {
            credential_id: "fixture-operator".into(),
            role: LegacyControlRoleV1::Runner,
        },
        change,
    }
}
fn principal() -> PrincipalRefV1 {
    PrincipalRefV1::HumanLocal {
        installation_id: "909d9572-f1a2-4a53-ad8b-a741030a2b18".into(),
        account_id: "290a1a3b-8538-49ef-a830-25df6fa19030".into(),
    }
}

#[test]
fn explicit_schema7_migration_preserves_schema6_backup_without_activation() {
    let directory = support::private_tempdir();
    let path = directory.path().join("tenant.sqlite");
    let store = Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    store
        .migrate_control_v3(&path.with_extension("v2.bak"))
        .unwrap();
    store
        .migrate_identity_v4(&path.with_extension("v3.bak"))
        .unwrap();
    store
        .migrate_human_access_v5(&path.with_extension("v4.bak"))
        .unwrap();
    store
        .migrate_workload_v6(&path.with_extension("v5.bak"))
        .unwrap();
    let before = store.integrity().unwrap();
    drop(store);
    let backup = directory.path().join("v6.bak");
    let result = Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
        .args(["migrate-control", "--data"])
        .arg(&path)
        .args(["--tenant-id", "tenant-a", "--backup"])
        .arg(&backup)
        .args([
            "--target-version",
            "7",
            "--confirm-control-stopped",
            "--max-events",
            "1000",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "explicit migration7 rejected: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let after =
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    assert_eq!(after.supervision_state().unwrap().schema_version, 7);
    assert!(!after.supervision_state().unwrap().active);
    assert!(after.integrity().unwrap().valid);
    assert_eq!(
        after.integrity().unwrap().event_count,
        before.event_count + 1
    );
    let saved =
        Store::open_existing_for_tenant(&backup, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    assert_eq!(saved.supervision_state().unwrap().schema_version, 6);
    assert_eq!(saved.integrity().unwrap().head_hash, before.head_hash);
}

#[test]
fn schema7_late_audit_refusal_rolls_back_marker_and_version_and_keeps_exact_backup() {
    let files = support::private_tempdir();
    let path = files.path().join("tenant.sqlite");
    let store = v6(&path, 64);
    let namespace = store.supervision_state().unwrap().control_namespace_uuid;
    let mut account = HumanAccountV1 {
        version: 1,
        principal: principal(),
        display_name: "Named migration reader".into(),
        revision: 1,
        active: true,
    };
    let mut exhausted = false;
    for expected_revision in 0..64 {
        account.revision = expected_revision + 1;
        if store
            .mutate_identity(&mutation(IdentityChangeV1::Account {
                expected_revision,
                account: account.clone(),
            }))
            .is_err()
        {
            exhausted = true;
            break;
        }
    }
    assert!(
        exhausted,
        "bounded identity-reserve fixture did not exhaust"
    );
    let before = store.integrity().unwrap();
    let revision = store.identity_authorization_revision().unwrap();
    let backup = files.path().join("before7");
    assert!(store.migrate_human_brokers_v7(&backup).is_err());
    assert!(backup.is_file());
    assert_eq!(store.supervision_state().unwrap().schema_version, 6);
    assert_eq!(
        store.supervision_state().unwrap().control_namespace_uuid,
        namespace
    );
    assert_eq!(store.integrity().unwrap().head_hash, before.head_hash);
    assert_eq!(store.identity_authorization_revision().unwrap(), revision);
    assert!(
        store
            .events(&EventQuery {
                kind: Some("control.identity.brokers_access.genesis".into()),
                ..Default::default()
            })
            .unwrap()
            .is_empty()
    );
    let saved =
        Store::open_existing_for_tenant(&backup, "tenant-a", Redactor::new(vec![]), 64).unwrap();
    assert_eq!(saved.supervision_state().unwrap().schema_version, 6);
    assert_eq!(saved.integrity().unwrap().head_hash, before.head_hash);
    assert!(saved.integrity().unwrap().valid);
}

fn digest(seq: i64, received: &str, previous: &str, payload: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"agentsandbox:audit:v1\0");
    digest.update(seq.to_be_bytes());
    for value in [received, previous, payload] {
        digest.update(value.as_bytes());
        digest.update([0]);
    }
    hex::encode(digest.finalize())
}

#[test]
fn schema7_refuses_missing_malformed_duplicate_and_historical_markers_and_unknown_schema() {
    for fault in [
        "missing",
        "malformed",
        "duplicate",
        "historical",
        "index",
        "unknown",
    ] {
        let files = support::private_tempdir();
        let path = files.path().join("tenant.sqlite");
        let store = v6(&path, 1000);
        if fault != "missing" {
            store
                .migrate_human_brokers_v7(&path.with_extension("before7"))
                .unwrap();
        }
        drop(store);
        let db = rusqlite::Connection::open(&path).unwrap();
        match fault {
            "missing" => db.pragma_update(None, "user_version", 7).unwrap(),
            "historical" => db.pragma_update(None, "user_version", 6).unwrap(),
            "index" => db
                .execute_batch("DROP INDEX workload_binding_history_prefix")
                .unwrap(),
            "unknown" => db
                .execute_batch("CREATE TABLE unrecognized(value TEXT)")
                .unwrap(),
            "malformed" | "duplicate" => {
                let (seq, received, previous, payload, floor): (i64, String, String, String, String) = db.query_row("SELECT e.seq,e.received_at,e.prev_hash,e.payload,f.max_received_at FROM events e JOIN audit_time_floors f ON f.event_seq=e.seq ORDER BY e.seq DESC LIMIT 1", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?,r.get(4)?))).unwrap();
                let mut event: Event = serde_json::from_str(&payload).unwrap();
                if fault == "malformed" {
                    event.data["protocol"] = Value::String("human_broker_registry_read_v0".into());
                    let changed = serde_json::to_string(&event).unwrap();
                    assert_eq!(changed.len(), payload.len());
                    db.execute(
                        "UPDATE events SET payload=?1,hash=?2 WHERE seq=?3",
                        rusqlite::params![
                            changed,
                            digest(seq, &received, &previous, &changed),
                            seq
                        ],
                    )
                    .unwrap();
                } else {
                    let prior_hash = digest(seq, &received, &previous, &payload);
                    event.id = uuid::Uuid::new_v4().to_string();
                    event.data["previous_seq"] = serde_json::json!(seq);
                    event.data["previous_hash"] = serde_json::json!(prior_hash);
                    let changed = serde_json::to_string(&event).unwrap();
                    let hash = digest(seq + 1, &received, &prior_hash, &changed);
                    db.execute("INSERT INTO events(seq,id,received_at,prev_hash,hash,payload,kind) VALUES(?1,?2,?3,?4,?5,?6,?7)", rusqlite::params![seq+1,event.id,received,prior_hash,hash,changed,event.kind]).unwrap();
                    db.execute(
                        "INSERT INTO audit_time_floors(event_seq,max_received_at) VALUES(?1,?2)",
                        rusqlite::params![seq + 1, floor],
                    )
                    .unwrap();
                    db.execute(
                        "UPDATE quota SET bytes=bytes+?1 WHERE id=1",
                        [i64::try_from(changed.len()).unwrap()],
                    )
                    .unwrap();
                }
            }
            _ => unreachable!(),
        }
        drop(db);
        assert!(
            Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000)
                .is_err(),
            "accepted {fault} schema7 capability"
        );
    }
}

#[test]
fn schema7_replay_rejects_a_new_capability_inserted_before_its_marker_even_with_valid_hashes() {
    let files = support::private_tempdir();
    let path = files.path().join("tenant.sqlite");
    let store = v6(&path, 1000);
    store
        .mutate_identity(&mutation(IdentityChangeV1::Account {
            expected_revision: 0,
            account: HumanAccountV1 {
                version: 1,
                principal: principal(),
                display_name: "Named historical reader".into(),
                revision: 1,
                active: true,
            },
        }))
        .unwrap();
    let mut member = MembershipV1 {
        version: 1,
        membership_id: uuid::Uuid::new_v4().to_string(),
        principal: principal(),
        tenant_id: "tenant-a".into(),
        organization_scope: "organization-a".into(),
        revision: 1,
        active: true,
        capabilities: std::collections::BTreeSet::from([HumanCapabilityV1::ReadMetadata]),
    };
    let original = mutation(IdentityChangeV1::Membership {
        expected_revision: 0,
        membership: member.clone(),
    });
    let receipt = store.mutate_identity(&original).unwrap();
    store
        .migrate_human_brokers_v7(&path.with_extension("before7"))
        .unwrap();
    drop(store);
    member
        .capabilities
        .insert(HumanCapabilityV1::ReadBrokerRegistry);
    let mut forged = original.clone();
    forged.change = IdentityChangeV1::Membership {
        expected_revision: 0,
        membership: member.clone(),
    };
    let hash = forged.input_hash().unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    let mut previous: String = db
        .query_row(
            "SELECT prev_hash FROM events WHERE seq=?1",
            [receipt.audit_seq],
            |r| r.get(0),
        )
        .unwrap();
    let rows: Vec<(i64, String, String)> = {
        let mut statement = db
            .prepare("SELECT seq,received_at,payload FROM events WHERE seq>=?1 ORDER BY seq")
            .unwrap();
        statement
            .query_map([receipt.audit_seq], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    for (seq, received, payload) in rows {
        let mut event: Event = serde_json::from_str(&payload).unwrap();
        if seq == receipt.audit_seq {
            event.data["mutation"] = serde_json::to_value(&forged).unwrap();
            event.data["input_hash"] = serde_json::json!(hash);
        }
        if event.kind == "control.identity.brokers_access.genesis" {
            event.data["previous_hash"] = serde_json::json!(previous);
        }
        let payload = serde_json::to_string(&event).unwrap();
        let current = digest(seq, &received, &previous, &payload);
        db.execute(
            "UPDATE events SET prev_hash=?1,hash=?2,payload=?3 WHERE seq=?4",
            rusqlite::params![previous, current, payload, seq],
        )
        .unwrap();
        previous = current;
    }
    db.execute(
        "UPDATE identity_memberships SET payload_json=?1 WHERE membership_id=?2",
        rusqlite::params![
            serde_json::to_string(&member).unwrap(),
            member.membership_id
        ],
    )
    .unwrap();
    db.execute(
        "UPDATE identity_operations SET input_hash=?1 WHERE operation_id=?2",
        rusqlite::params![hash, original.operation_id],
    )
    .unwrap();
    db.execute_batch(
        "UPDATE quota SET bytes=(SELECT SUM(length(CAST(payload AS BLOB))) FROM events) WHERE id=1",
    )
    .unwrap();
    let expected_head: String = db
        .query_row(
            "SELECT hash FROM events ORDER BY seq DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    drop(db);
    // Opening retains a diagnostic handle after replay failure. Acceptance is
    // its health and authorization state, not the presence of that handle.
    let rejected =
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    assert!(
        !rejected.healthy(),
        "pre-marker capability must invalidate the Store authority"
    );
    let error = rejected.integrity().unwrap_err();
    assert_eq!(
        error.downcast_ref::<IdentityFailureV1>(),
        Some(&IdentityFailureV1::MigrationRequired)
    );
    assert!(
        rejected
            .authorize_human_context(
                &principal(),
                &member.membership_id,
                1,
                ContextModeV1::Administration
            )
            .is_err()
    );
    assert!(rejected.identity_memberships(&principal()).is_err());
    assert!(rejected.brokers_page(None, 1).is_err());
    assert!(
        rejected
            .mutate_identity(&mutation(IdentityChangeV1::Account {
                expected_revision: 1,
                account: HumanAccountV1 {
                    version: 1,
                    principal: principal(),
                    display_name: "Refused mutation".into(),
                    revision: 2,
                    active: true
                }
            }))
            .is_err()
    );
    let db = rusqlite::Connection::open(&path).unwrap();
    let actual_head: String = db
        .query_row(
            "SELECT hash FROM events ORDER BY seq DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(actual_head, expected_head);
}
