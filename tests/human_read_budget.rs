//! A real SQLite/HTTP regression for audit-capacity isolation, without a runtime.
//! The small physical capacity bounds this reproduction; it does not prescribe
//! the eventual human budget, per-principal share, or control reserve.
mod support;

use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::{EventQuery, Store},
    identity::*,
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
use std::{collections::BTreeSet, path::Path, sync::Arc};
use tower::ServiceExt;
use uuid::Uuid;

const CAPACITY: u64 = 64;
const READ_ATTEMPTS: usize = 96;

fn change(change: IdentityChangeV1) -> IdentityMutationV1 {
    IdentityMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        actor: PrincipalRefV1::LegacyControl {
            credential_id: "budget-fixture-control".into(),
            role: LegacyControlRoleV1::Runner,
        },
        change,
    }
}

// Same real migration/provisioning sequence as human_access; no SQL mutation
// of events, quota, identity projections or migration metadata.
fn fresh(path: &Path, tenant: &str) -> Arc<Store> {
    let store = Store::open_for_tenant(path, tenant, Redactor::new(vec![]), CAPACITY).unwrap();
    store
        .migrate_control_v3(&path.with_extension("before-v3"))
        .unwrap();
    store
        .migrate_identity_v4(&path.with_extension("before-v4"))
        .unwrap();
    store
        .migrate_human_access_v5(&path.with_extension("before-v5"))
        .unwrap();
    Arc::new(store)
}

fn provision(store: &Store, token: &str) -> (PrincipalRefV1, LocalCredentialV1) {
    let principal = PrincipalRefV1::HumanLocal {
        installation_id: "909d9572-f1a2-4a53-ad8b-a741030a2b18".into(),
        account_id: Uuid::new_v4().to_string(),
    };
    let binding = LocalCredentialV1 {
        version: 1,
        credential_id: Uuid::new_v4().to_string(),
        principal: principal.clone(),
        sha256_digest: hex::encode(Sha256::digest(token.as_bytes())),
        revision: 1,
        active: true,
        expires_at: None,
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
                        display_name: "Budget regression reader".into(),
                        revision: 1,
                        active: true,
                    },
                }),
                change(IdentityChangeV1::Credential {
                    expected_revision: 0,
                    binding: binding.clone(),
                }),
                change(IdentityChangeV1::Membership {
                    expected_revision: 0,
                    membership: MembershipV1 {
                        version: 1,
                        membership_id: Uuid::new_v4().to_string(),
                        principal: principal.clone(),
                        tenant_id: store.tenant_id().into(),
                        organization_scope: "budget-fixture-organization".into(),
                        revision: 1,
                        active: true,
                        capabilities: BTreeSet::from([HumanCapabilityV1::ReadMetadata]),
                    },
                }),
            ],
        })
        .unwrap();
    (principal, binding)
}

async fn request(
    router: &Router,
    token: &str,
    method: &str,
    path: &str,
    body: Value,
) -> (StatusCode, Value) {
    let body = if method == "GET" {
        Body::empty()
    } else {
        Body::from(body.to_string())
    };
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 512 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

fn decision() -> Value {
    json!({
        "request_id": Uuid::new_v4().to_string(),
        "profile": "strict",
        "command": {"argv": ["/usr/bin/id"], "cwd": "/workspace"},
        "human_approved": false
    })
}

fn events(store: &Store, after: i64) -> Vec<StoredEvent> {
    store
        .events(&EventQuery {
            after: Some(after),
            order: Some("asc".into()),
            limit: Some(100),
            ..Default::default()
        })
        .unwrap()
}

#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    head: i64,
    count: usize,
    head_hash: String,
    payload_bytes: usize,
}

fn snapshot(store: &Store) -> Snapshot {
    let rows = events(store, 0);
    let head = store.head().unwrap();
    assert_eq!(
        rows.len() as i64,
        head,
        "fixture must fit in one diagnostic page"
    );
    Snapshot {
        head,
        count: rows.len(),
        head_hash: rows.last().unwrap().hash.clone(),
        payload_bytes: rows
            .iter()
            .map(|row| serde_json::to_vec(&row.event).unwrap().len())
            .sum(),
    }
}

#[tokio::test]
async fn one_human_cannot_exhaust_control_revocation_or_another_humans_read_budget() {
    let files = support::private_tempdir();
    let a = fresh(&files.path().join("a.sqlite"), "tenant-a");
    let b = fresh(&files.path().join("b.sqlite"), "tenant-b");
    let human_a1 = generate_local_credential().unwrap();
    let human_a2 = generate_local_credential().unwrap();
    let (principal_a1, mut credential_a1) = provision(&a, &human_a1);
    let (principal_a2, _) = provision(&a, &human_a2);
    let runner_a = "budget-fixture-runner-a".repeat(3);
    let runner_b = "budget-fixture-runner-b".repeat(3);
    let router = gateway_router(
        GatewayState::for_tenants(
            vec![
                TenantGateway {
                    tenant_id: "tenant-a".into(),
                    store: Arc::clone(&a),
                    runner_token: runner_a.clone(),
                    viewer_token: "budget-fixture-viewer-a".repeat(3),
                },
                TenantGateway {
                    tenant_id: "tenant-b".into(),
                    store: Arc::clone(&b),
                    runner_token: runner_b.clone(),
                    viewer_token: "budget-fixture-viewer-b".repeat(3),
                },
            ],
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )
        .unwrap(),
    );

    // Preconditions have independent positive controls. Errors here are a
    // harness/setup failure, never the expected RED of this regression.
    let before_revoke = snapshot(&a);
    credential_a1.active = false;
    credential_a1.revision = 2;
    let baseline_revoke = a
        .mutate_identity(&change(IdentityChangeV1::Credential {
            expected_revision: 1,
            binding: credential_a1.clone(),
        }))
        .unwrap();
    let revoked = snapshot(&a);
    assert_eq!(baseline_revoke.audit_seq, before_revoke.head + 1);
    assert_eq!(revoked.head_hash, baseline_revoke.audit_hash);
    assert_eq!(
        request(
            &router,
            &human_a1,
            "GET",
            "/api/v1/human/identity",
            Value::Null
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED,
        "baseline credential revocation must be effective"
    );
    assert_eq!(snapshot(&a), revoked);
    credential_a1.active = true;
    credential_a1.revision = 3;
    let reactivation = a
        .mutate_identity(&change(IdentityChangeV1::Credential {
            expected_revision: 2,
            binding: credential_a1.clone(),
        }))
        .unwrap();
    assert_eq!(reactivation.audit_seq, baseline_revoke.audit_seq + 1);
    for runner in [&runner_a, &runner_b] {
        assert_eq!(
            request(&router, runner, "POST", "/api/v1/decide", decision())
                .await
                .0,
            StatusCode::OK,
            "baseline decision must succeed before reader pressure"
        );
    }
    assert_eq!(
        request(
            &router,
            &runner_a,
            "POST",
            "/api/v1/events",
            json!(Event::new("fixture.baseline"))
        )
        .await
        .0,
        StatusCode::OK,
        "baseline ingest must succeed before reader pressure"
    );
    for (token, principal) in [(&human_a1, &principal_a1), (&human_a2, &principal_a2)] {
        let (status, body) =
            request(&router, token, "GET", "/api/v1/human/identity", Value::Null).await;
        assert_eq!(status, StatusCode::OK, "baseline human authentication");
        assert_eq!(body["principal"], serde_json::to_value(principal).unwrap());
        assert_eq!(body["contexts"][0]["tenant_id"], "tenant-a");
    }
    let baseline = snapshot(&a);
    let baseline_b = snapshot(&b);
    assert!(
        baseline.head + 16 < CAPACITY as i64,
        "fixture must leave ample capacity for the oracle"
    );

    // Fewer than 100 sequential calls: this cannot consume D's 200/s
    // per-tenant admission budget, even including both baseline identities.
    let mut accepted = 0;
    let mut refusal = None;
    for _ in 0..READ_ATTEMPTS {
        let (status, body) = request(
            &router,
            &human_a1,
            "GET",
            "/api/v1/human/identity",
            Value::Null,
        )
        .await;
        if status != StatusCode::OK {
            refusal = Some((status, body));
            break;
        }
        accepted += 1;
    }
    let (refusal_status, refusal_body) =
        refusal.expect("bounded reader pressure must reach a controlled refusal");
    let refused = snapshot(&a);
    let reads = events(&a, baseline.head);
    assert_eq!(
        reads.len(),
        accepted,
        "every successful read needs its durable audit"
    );
    assert_eq!(refused.head, baseline.head + accepted as i64);
    for row in &reads {
        assert_eq!(row.event.kind, "control.identity.access");
        assert_eq!(
            row.event.data["principal"],
            serde_json::to_value(&principal_a1).unwrap()
        );
        assert_eq!(row.event.data["action"], "identity");
        Uuid::parse_str(row.event.data["request_id"].as_str().unwrap()).unwrap();
        assert!(!row.received_at.is_empty());
    }
    assert_eq!(snapshot(&b), baseline_b, "A's reads must not append to B");
    let retry = request(
        &router,
        &human_a1,
        "GET",
        "/api/v1/human/identity",
        Value::Null,
    )
    .await;
    assert_eq!(
        snapshot(&a),
        refused,
        "a refused read must not mutate audit"
    );

    // Observe all affected controls before the first protection assertion, so
    // one RED records its causal blast radius rather than stopping at a 503.
    let decide_a = request(&router, &runner_a, "POST", "/api/v1/decide", decision()).await;
    let after_decision = snapshot(&a);
    let ingest_a = request(
        &router,
        &runner_a,
        "POST",
        "/api/v1/events",
        json!(Event::new("fixture.after_reader")),
    )
    .await;
    let after_ingest = snapshot(&a);
    let peer = request(
        &router,
        &human_a2,
        "GET",
        "/api/v1/human/identity",
        Value::Null,
    )
    .await;
    let after_peer = snapshot(&a);
    credential_a1.active = false;
    credential_a1.revision = 4;
    let revocation = a.mutate_identity(&change(IdentityChangeV1::Credential {
        expected_revision: 3,
        binding: credential_a1,
    }));
    let after_revocation = snapshot(&a);
    let revoked_reader = request(
        &router,
        &human_a1,
        "GET",
        "/api/v1/human/identity",
        Value::Null,
    )
    .await;
    let decide_b = request(&router, &runner_b, "POST", "/api/v1/decide", decision()).await;
    let after_b = snapshot(&b);
    println!(
        "F1 baseline={baseline:?} accepted={accepted} refused={refused:?} refusal={} refusal_code={} retry={} decision_a={} ingest_a={} peer_a={} revocation_ok={} revoked_reader={} decision_b={} control_heads={:?} after_revocation={after_revocation:?}",
        refusal_status,
        refusal_body["error"]["code"],
        retry.0,
        decide_a.0,
        ingest_a.0,
        peer.0,
        revocation.is_ok(),
        revoked_reader.0,
        decide_b.0,
        (after_decision.head, after_ingest.head, after_peer.head),
    );

    // FIRST expected failing protection assertion on the uncorrected source:
    // the reader currently reaches physical head=64, instead of leaving room.
    assert!(
        refused.head < CAPACITY as i64,
        "one read-only human consumed physical tenant capacity: {refused:?}; decision={}, ingest={}, peer={}, revocation_ok={}",
        decide_a.0,
        ingest_a.0,
        peer.0,
        revocation.is_ok()
    );
    assert_eq!(refusal_status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(retry.0, StatusCode::TOO_MANY_REQUESTS);
    assert!(
        refusal_body.get("contexts").is_none(),
        "refused reads emit no identity data"
    );
    assert_eq!(decide_a.0, StatusCode::OK);
    assert_eq!(after_decision.head, refused.head + 1);
    assert_eq!(ingest_a.0, StatusCode::OK);
    assert_eq!(after_ingest.head, after_decision.head + 1);
    assert_eq!(peer.0, StatusCode::OK);
    assert_eq!(after_peer.head, after_ingest.head + 1);
    let receipt = revocation.expect("the tenant must retain durable revocation capacity");
    assert_eq!(receipt.audit_seq, after_peer.head + 1);
    assert_eq!(after_revocation.head, receipt.audit_seq);
    assert_eq!(after_revocation.head_hash, receipt.audit_hash);
    assert_eq!(revoked_reader.0, StatusCode::UNAUTHORIZED);
    assert_eq!(
        snapshot(&a),
        after_revocation,
        "a revoked credential cannot append"
    );
    assert_eq!(decide_b.0, StatusCode::OK);
    assert_eq!(after_b.head, baseline_b.head + 1);
}
