//! The release pass against a real supervised Store: a storage fault in the
//! middle of a batch, and the audit budgets that a release fact draws on.
//! Worker runtime facts come from the shared software fixture and are
//! synthetic; nothing here qualifies a transport or a runtime.
use crate::{
    audit::{AuditCapacityFailureV1, AuditCapacityV1, Store},
    control::*,
    model::{CommandSpec, DecisionRequest},
    policy::{self, Policy},
    redaction::Redactor,
    relay::{
        RelayCapacityRefusalV1, RelayCapacityV1, RelayEndpointV1, RelayFailure, RelayGrantV1,
        RelayRetentionPolicyV1, RelaySendV1, RelayTenantCapacityRefusalV1, WorkerRelayIdentity,
    },
    workload::{AuthenticatedWorkload, WorkloadBindingIssueV1, WorkloadSubjectV1},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::Path;
use uuid::Uuid;

const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const HASH_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

// The typed admission, helper, arm and start transitions of the integration
// fixture, compiled here against the crate itself. Another library test
// module compiles the same file; each needs its own private copy.
#[path = "../../../tests/support/relay_admission_fixture.rs"]
#[allow(dead_code, clippy::duplicate_mod)]
mod fixture;
use fixture::{bind, endpoint, fresh, running_now};

const EVENTS: u64 = 20_000;
const TENANT: &str = "tenant-a";

struct World {
    files: tempfile::TempDir,
    store: Store,
    a1: WorkerV1,
    sender: WorkerRelayIdentity,
    grant: RelayGrantV1,
}

/// A1 holds a directed grant towards A2 and an authenticated relay proof.
fn world() -> World {
    use std::os::unix::fs::PermissionsExt;
    let files = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let share = RelayCapacityV1 {
        messages: 8,
        bytes: 524_288,
    };
    let store = fresh(files.path(), TENANT, EVENTS)
        .with_relay_retention_policy(RelayRetentionPolicyV1 {
            tenant: RelayCapacityV1 {
                messages: 16,
                bytes: 1_048_576,
            },
            sender: share,
            grant: share,
            ..RelayRetentionPolicyV1::default()
        })
        .unwrap();
    let a1 = running_now(&store);
    let a2 = running_now(&store);
    let token = "1".repeat(48);
    bind(&store, &a1, &token);
    // A grant needs both endpoints bound, the destination included.
    bind(&store, &a2, &"2".repeat(48));
    let Some(AuthenticatedWorkload::Worker(sender)) =
        store.authenticate_workload_token(&token).unwrap()
    else {
        panic!("the fixture worker proof was not authenticated");
    };
    let grant = RelayGrantV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        grant_id: Uuid::new_v4().to_string(),
        source: endpoint(&a1),
        destination: endpoint(&a2),
        channel: "tasks".into(),
        expires_at: (chrono::Utc::now() + chrono::Duration::seconds(600)).to_rfc3339(),
    };
    store.relay_grant(&grant).unwrap();
    World {
        files,
        store,
        a1,
        sender,
        grant,
    }
}

impl World {
    fn input(&self, ttl_seconds: u32, body: &str) -> RelaySendV1 {
        RelaySendV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            grant_id: self.grant.grant_id.clone(),
            ttl_seconds,
            body: body.into(),
        }
    }
    /// Three bodies with a two-second TTL, then the wait that expires them.
    fn expired_bodies(&self) {
        for index in 0..3 {
            self.store
                .relay_send(&self.sender, &self.input(2, &format!("expiring {index}")))
                .unwrap();
        }
        assert_eq!(
            self.count("SELECT COUNT(*) FROM relay_messages WHERE body IS NOT NULL"),
            3,
            "fixture too slow: a body expired during the fill"
        );
        std::thread::sleep(std::time::Duration::from_millis(2300));
    }
    fn count(&self, sql: &str) -> i64 {
        self.store
            .db
            .lock()
            .unwrap()
            .query_row(sql, [], |row| row.get(0))
            .unwrap()
    }
    /// Published workload audit usage: the whole tenant, then A1 alone.
    fn workload_usage(&self) -> (AuditCapacityV1, AuditCapacityV1) {
        let capacity = self.store.supervision_lease.capacity.lock().unwrap();
        (
            capacity.workload_tenant,
            capacity
                .workload_principals
                .get(&format!("worker:{}", self.a1.worker_id))
                .copied()
                .unwrap_or_default(),
        )
    }
}

#[test]
fn storage_fault_after_a_written_release_fact_rolls_the_whole_send_back() {
    let w = world();
    w.expired_bodies();
    // The pass releases the sender's expired bodies oldest first. The second
    // release fails after its audit fact was appended, on its projection.
    let victim: String = w
        .store
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT message_id FROM relay_messages ORDER BY expires_at,message_id LIMIT 1 OFFSET 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    w.store
        .db
        .lock()
        .unwrap()
        .execute_batch(&format!(
            "CREATE TEMP TRIGGER fixture_relay_release_fault BEFORE UPDATE ON main.relay_messages WHEN OLD.message_id='{victim}' BEGIN SELECT RAISE(ABORT,'fixture relay release fault'); END"
        ))
        .unwrap();

    let head = w.store.head().unwrap();
    let workload = w.workload_usage();
    let trigger = w.input(60, "after the fault");
    let error = match w.store.relay_send(&w.sender, &trigger) {
        Err(error) => error,
        Ok(receipt) => panic!(
            "a storage fault after a written release fact was treated as a clean stop: the send committed at audit sequence {}",
            receipt.audit_seq
        ),
    };
    assert!(
        format!("{error:#}").contains("fixture relay release fault"),
        "the send failed for another reason: {error:#}"
    );
    assert!(
        error.downcast_ref::<RelayFailure>().is_none()
            && error.downcast_ref::<RelayCapacityRefusalV1>().is_none()
            && error
                .downcast_ref::<RelayTenantCapacityRefusalV1>()
                .is_none()
            && error.downcast_ref::<AuditCapacityFailureV1>().is_none(),
        "a storage fault was reported as a typed refusal: {error:#}"
    );
    // Nothing of the batch remains: neither the first, complete release nor
    // the audit fact of the second one.
    assert_eq!(
        w.store.head().unwrap(),
        head,
        "a release fact survived a failed batch"
    );
    assert_eq!(
        w.count("SELECT COUNT(*) FROM relay_messages WHERE body IS NOT NULL"),
        3,
        "a body was released although its batch failed"
    );
    assert_eq!(
        w.count("SELECT COUNT(*) FROM relay_operations WHERE action='prune'"),
        0
    );
    assert_eq!(
        w.count("SELECT COUNT(*) FROM relay_messages"),
        3,
        "the send was stored"
    );
    assert_eq!(w.workload_usage(), workload);
    assert!(w.store.healthy());
    assert!(w.store.integrity().unwrap().valid);

    // Without the fault, the same operation releases the three bodies and is
    // admitted; a fresh process accepts the resulting history.
    w.store
        .db
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER fixture_relay_release_fault")
        .unwrap();
    let receipt = w.store.relay_send(&w.sender, &trigger).unwrap();
    assert!(!receipt.replayed);
    assert_eq!(w.store.head().unwrap(), head + 4);
    assert_eq!(
        w.count("SELECT COUNT(*) FROM relay_messages WHERE body IS NOT NULL"),
        1
    );
    let path = w.files.path().join(format!("{TENANT}.sqlite"));
    let World { files, store, .. } = w;
    drop(store);
    let reopened =
        Store::open_existing_for_tenant(&path, TENANT, Redactor::new(vec![]), EVENTS).unwrap();
    assert!(reopened.healthy());
    assert!(reopened.integrity().unwrap().valid);
    drop(files);
}

#[test]
fn automatic_release_facts_are_charged_to_no_workload_audit_budget() {
    let w = world();
    w.expired_bodies();
    let head = w.store.head().unwrap();
    let (tenant, worker) = w.workload_usage();
    w.store
        .relay_send(&w.sender, &w.input(60, "after expiry"))
        .unwrap();
    // One send wrote four facts: three releases, then itself.
    assert_eq!(w.store.head().unwrap(), head + 4);
    assert_eq!(
        w.count("SELECT COUNT(*) FROM relay_operations WHERE action='prune'"),
        3
    );
    // Only the send is workload audit. The releases are ordinary writes: they
    // draw on no worker budget and not on the tenant's workload budget, so a
    // worker's sends can induce as many ordinary facts as bodies it expires.
    let (tenant_after, worker_after) = w.workload_usage();
    assert_eq!(
        tenant_after.events,
        tenant.events + 1,
        "a release fact was charged to the tenant workload audit budget"
    );
    assert_eq!(
        worker_after.events,
        worker.events + 1,
        "a release fact was charged to the triggering worker"
    );

    // A message is released at most once: nothing is left for the next send.
    let head = w.store.head().unwrap();
    w.store
        .relay_send(&w.sender, &w.input(60, "nothing left to release"))
        .unwrap();
    assert_eq!(
        w.store.head().unwrap(),
        head + 1,
        "a released body produced a second release fact"
    );
    assert_eq!(
        w.count("SELECT COUNT(*) FROM relay_operations WHERE action='prune'"),
        3
    );
    assert!(w.store.integrity().unwrap().valid);
}
