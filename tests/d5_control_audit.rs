//! D5 delivery authority and durable reserve regressions, without a runtime.
//! Synthetic facts exercise the real gateway router and private SQLite store;
//! they do not establish a retirement, runtime outcome or broker provenance.
mod support;

use agentsandbox::{
    api::{GatewayState, gateway_router},
    audit::{AuditCapacityFailureV1, AuditCapacityPolicyV1, Store},
    identity::{
        HumanAccountV1, HumanCapabilityV1, IdentityChangeV1, IdentityMutationV1,
        LegacyControlRoleV1, LocalCredentialV1, LocalHumanProvisionV1, MembershipV1,
        PrincipalRefV1,
    },
    model::{Event, StoredEvent},
    policy::Policy,
    redaction::Redactor,
};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tower::ServiceExt;
use uuid::Uuid;

const RUNNER: &str = "d5-synthetic-trusted-control-runner";
const VIEWER: &str = "d5-synthetic-read-only-viewer";
const HUMAN: &str = "aigh1_1111111111111111111111111111111111111111111111111111111111111111";
const OTHER_HUMAN: &str = "aigh1_2222222222222222222222222222222222222222222222222222222222222222";
const CONTROL_PATH: &str = "/api/v1/control/events";
const D5_KINDS: [&str; 3] = [
    "broker.abandon-before-claim.requested",
    "broker.retirement.pending",
    "broker.retirement.final",
];

fn fresh(path: &Path, max_events: u64, policy: AuditCapacityPolicyV1) -> Store {
    let store = Store::open_for_tenant(path, "tenant-d5", Redactor::new(vec![]), max_events)
        .unwrap()
        .with_capacity_policy(policy)
        .unwrap();
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
}

fn router(store: &Arc<Store>) -> Router {
    gateway_router(GatewayState::new(
        store.clone(),
        RUNNER.into(),
        VIEWER.into(),
        Policy::default(),
        None,
        Redactor::new(vec![]),
    ))
}

fn fact(kind: &str) -> Event {
    let mut event = Event::new(kind);
    event.source = "synthetic-d5-control-fixture".into();
    event.provenance = "declared".into();
    event.coverage = "partial".into();
    event.data = json!({"fixture": "delivery-and-capacity-only"});
    event
}

async fn request(
    app: &Router,
    method: &str,
    path: &str,
    token: &str,
    body: Value,
) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 300_000).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

fn provision_reader(store: &Store, token: &str) {
    let principal = PrincipalRefV1::HumanLocal {
        installation_id: "1679fdab-7623-4fad-b121-ed550c936f33".into(),
        account_id: Uuid::new_v4().to_string(),
    };
    let change = |change| IdentityMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        actor: PrincipalRefV1::LegacyControl {
            credential_id: "d5-trusted-provisioning-fixture".into(),
            role: LegacyControlRoleV1::Runner,
        },
        change,
    };
    store
        .provision_local_human(&LocalHumanProvisionV1 {
            version: 1,
            mutations: [
                change(IdentityChangeV1::Account {
                    expected_revision: 0,
                    account: HumanAccountV1 {
                        version: 1,
                        principal: principal.clone(),
                        display_name: "Synthetic D5 reader".into(),
                        revision: 1,
                        active: true,
                    },
                }),
                change(IdentityChangeV1::Credential {
                    expected_revision: 0,
                    binding: LocalCredentialV1 {
                        version: 1,
                        credential_id: Uuid::new_v4().to_string(),
                        principal: principal.clone(),
                        sha256_digest: hex::encode(Sha256::digest(token.as_bytes())),
                        revision: 1,
                        active: true,
                        expires_at: None,
                    },
                }),
                change(IdentityChangeV1::Membership {
                    expected_revision: 0,
                    membership: MembershipV1 {
                        version: 1,
                        membership_id: Uuid::new_v4().to_string(),
                        principal,
                        tenant_id: "tenant-d5".into(),
                        organization_scope: "synthetic-d5-organization".into(),
                        revision: 1,
                        active: true,
                        capabilities: [HumanCapabilityV1::ReadMetadata].into_iter().collect(),
                    },
                }),
            ],
        })
        .unwrap();
}

#[tokio::test]
async fn d5_facts_require_authenticated_control_delivery_and_a_closed_kind_set() {
    let files = support::private_tempdir();
    let store = Arc::new(fresh(
        &files.path().join("d5.sqlite"),
        64,
        AuditCapacityPolicyV1::resolved(64),
    ));
    let app = router(&store);
    let before = store.integrity().unwrap();
    for (token, expected) in [
        (VIEWER, StatusCode::FORBIDDEN),
        (
            "unknown-synthetic-worker-credential",
            StatusCode::UNAUTHORIZED,
        ),
    ] {
        let polled = Arc::new(AtomicBool::new(false));
        let observed = polled.clone();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(CONTROL_PATH)
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from_stream(futures_util::stream::once(async move {
                        observed.store(true, Ordering::SeqCst);
                        Ok::<_, std::io::Error>("unread invalid body")
                    })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        assert!(!polled.load(Ordering::SeqCst));
    }
    assert_eq!(store.integrity().unwrap().head_hash, before.head_hash);

    for kind in D5_KINDS {
        let event = fact(kind);
        let (status, receipt) = request(&app, "POST", CONTROL_PATH, RUNNER, json!(event)).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "control delivery of {kind}: {receipt}"
        );
        let stored: StoredEvent = serde_json::from_value(receipt).unwrap();
        assert_eq!(stored.event.id, event.id);
        assert_eq!(stored.event.kind, kind);
        let (status, replay) = request(&app, "POST", CONTROL_PATH, RUNNER, json!(event)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(replay["seq"], stored.seq);
        assert_eq!(replay["hash"], stored.hash);

        let head = store.integrity().unwrap().head_hash;
        let (status, _) = request(&app, "POST", "/api/events", RUNNER, json!(fact(kind))).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "ordinary ingestion of {kind}"
        );
        assert_eq!(store.integrity().unwrap().head_hash, head);
    }
    let head = store.integrity().unwrap().head_hash;
    let (status, _) = request(
        &app,
        "POST",
        CONTROL_PATH,
        RUNNER,
        json!(fact("broker.retirement.final.unapproved")),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let after = store.integrity().unwrap();
    assert!(after.valid && store.healthy());
    assert_eq!(after.event_count, before.event_count + 3);
    assert_eq!(after.head_hash, head);
}

#[tokio::test]
async fn d5_control_reserve_survives_ordinary_writes_human_reads_and_reopen() {
    let files = support::private_tempdir();
    let path = files.path().join("d5.sqlite");
    // Real explicit policy: 32 total, 8 control, 4 identity => 20 ordinary.
    // Two audited reads consume the principal's share; no timing or load oracle.
    let mut policy = AuditCapacityPolicyV1::resolved(32);
    policy.human_tenant.events = 4;
    policy.human_principal.events = 2;
    let store = fresh(&path, 32, policy.clone());
    provision_reader(&store, HUMAN);
    provision_reader(&store, OTHER_HUMAN);
    let store = Arc::new(store);
    let app = router(&store);
    let before_reads = store.integrity().unwrap();
    for _ in 0..2 {
        let (status, response) =
            request(&app, "GET", "/api/v1/human/identity", HUMAN, Value::Null).await;
        assert_eq!(status, StatusCode::OK, "reader fixture: {response}");
    }
    let after_reads = store.integrity().unwrap();
    assert_eq!(after_reads.event_count, before_reads.event_count + 2);
    let (status, refused) =
        request(&app, "GET", "/api/v1/human/identity", HUMAN, Value::Null).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(refused["error"]["code"], "audit_read_budget_exhausted");
    assert_eq!(store.integrity().unwrap().head_hash, after_reads.head_hash);

    // Minimal prefill through real checked transactions, not direct SQL or a
    // request campaign. The crossing Store oracle is independent of the route
    // allowlist, so a baseline403 cannot hide a missing durable control reserve.
    assert!(
        after_reads.event_count < 20,
        "fixture must leave ordinary capacity"
    );
    for _ in after_reads.event_count..20 {
        store.append(fact("fixture.ordinary")).unwrap();
    }
    let boundary = store.integrity().unwrap();
    assert_eq!(boundary.event_count, 20);
    let crossing = store.append(fact("fixture.ordinary"));
    assert!(
        crossing.is_err(),
        "ordinary write consumed the durable control reserve at total21"
    );
    assert_eq!(
        crossing
            .unwrap_err()
            .downcast_ref::<AuditCapacityFailureV1>(),
        Some(&AuditCapacityFailureV1::OrdinaryWriteBudgetExhausted)
    );
    assert_eq!(store.integrity().unwrap().head_hash, boundary.head_hash);
    let (status, refused) = request(
        &app,
        "GET",
        "/api/v1/human/identity",
        OTHER_HUMAN,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(refused["error"]["code"], "audit_read_budget_exhausted");
    assert_eq!(store.integrity().unwrap().head_hash, boundary.head_hash);
    drop(app);
    drop(store);

    let store = Arc::new(
        Store::open_existing_for_tenant(&path, "tenant-d5", Redactor::new(vec![]), 32)
            .unwrap()
            .with_capacity_policy(policy.clone())
            .unwrap(),
    );
    let app = router(&store);
    // Neither a payload label nor reopening the same durable chain can mint
    // control delivery authority or restore the exhausted human read budget.
    let mut spoofed = fact("fixture.ordinary");
    spoofed.data = json!({"capacity_class": "control", "trusted": true});
    let (status, refused) = request(&app, "POST", "/api/events", RUNNER, json!(spoofed)).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(refused["error"]["code"], "audit_write_budget_exhausted");
    let (status, refused) =
        request(&app, "GET", "/api/v1/human/identity", HUMAN, Value::Null).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(refused["error"]["code"], "audit_read_budget_exhausted");
    assert_eq!(store.integrity().unwrap().head_hash, boundary.head_hash);

    let mut receipts = Vec::new();
    for kind in D5_KINDS {
        let event = fact(kind);
        let (status, receipt) = request(&app, "POST", CONTROL_PATH, RUNNER, json!(event)).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "reserved control delivery of {kind}: {receipt}"
        );
        let stored: StoredEvent = serde_json::from_value(receipt).unwrap();
        assert_eq!(stored.event.id, event.id);
        assert_eq!(stored.event.kind, kind);
        receipts.push(stored);
    }
    let after = store.integrity().unwrap();
    assert!(after.valid && store.healthy());
    assert_eq!(after.event_count, 23);
    drop(app);
    drop(store);
    let reopened = Store::open_existing_for_tenant(&path, "tenant-d5", Redactor::new(vec![]), 32)
        .unwrap()
        .with_capacity_policy(policy)
        .unwrap();
    assert_eq!(reopened.integrity().unwrap().head_hash, after.head_hash);
    for receipt in receipts {
        // Read back the exact persisted identity and receipt after reopening,
        // without appending another event or reconstructing an expected hash.
        let events = reopened
            .events(&agentsandbox::audit::EventQuery {
                kind: Some(receipt.event.kind.clone()),
                limit: Some(1),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].seq, receipt.seq);
        assert_eq!(events[0].hash, receipt.hash);
        assert_eq!(events[0].event.id, receipt.event.id);
    }
    assert_eq!(reopened.integrity().unwrap().event_count, 23);
}
