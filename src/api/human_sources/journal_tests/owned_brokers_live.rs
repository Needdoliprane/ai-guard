//! Real owned Router/Store reads. Source-only until Root/Luna admit QA.
use super::races::{connection, snapshot, state};
use super::*;
use crate::{
    broker_registry::{
        BrokerAuthorityV1, BrokerCheckpointSnapshotV1, BrokerCheckpointV1,
        BrokerGrantMutationV1, BrokerJobCheckpointV1, BrokerRuntimeKindV1,
    },
    workload::{AuthenticatedWorkload, WorkloadBindingIssueV1, WorkloadSubjectV1},
};
use std::collections::BTreeSet;

type Snapshot = (i64, String, i64, i64);

fn membership(fixture: &Fixture, capability: HumanCapabilityV1) -> MembershipV1 {
    let store = fixture.legacy_store("B");
    let mut members = store.identity_memberships(fixture.principal()).unwrap();
    assert_eq!(members.len(), 1);
    let mut member = members.remove(0);
    let previous = member.revision;
    member.revision += 1;
    member.capabilities.insert(capability);
    store.mutate_identity(&change(IdentityChangeV1::Membership {
        expected_revision: previous,
        membership: member.clone(),
    })).unwrap();
    member
}

struct SeededBroker {
    broker_id: String,
    incarnation_id: String,
    private_values: Vec<String>,
    checkpoint_received_at: Option<String>,
    checkpoint_expires_at: Option<String>,
}

fn seed_broker(fixture: &Fixture, tenant: &str, number: u128, checkpoint: bool) -> SeededBroker {
    let store = fixture.legacy_store(tenant);
    let broker_id = uuid::Uuid::from_u128(number).to_string();
    let credential_id = uuid::Uuid::new_v4().to_string();
    let incarnation_id = uuid::Uuid::new_v4().to_string();
    let token = format!("synthetic-owned-live-broker-{}", uuid::Uuid::new_v4());
    let digest = hex::encode(Sha256::digest(token.as_bytes()));
    let binding_operation = uuid::Uuid::new_v4().to_string();
    store.issue_workload_binding(&WorkloadBindingIssueV1 {
        version: 1,
        operation_id: binding_operation.clone(),
        credential_id: credential_id.clone(),
        expected_revision: 0,
        subject: WorkloadSubjectV1::Broker {
            broker_id: broker_id.clone(),
            incarnation_id: incarnation_id.clone(),
        },
        sha256_digest: digest.clone(),
        expires_at: "2100-01-01T00:00:00.000Z".into(),
    }).unwrap();
    let authority = BrokerAuthorityV1 {
        authority_id: format!("synthetic-owned-live-host-{number}"),
        runtime_kind: BrokerRuntimeKindV1::Docker,
    };
    let grant_operation = uuid::Uuid::new_v4().to_string();
    let grant = store.mutate_broker_grant(&BrokerGrantMutationV1 {
        version: 1,
        operation_id: grant_operation.clone(),
        broker_id: broker_id.clone(),
        expected_revision: None,
        credential_id: credential_id.clone(),
        incarnation_id: incarnation_id.clone(),
        authorities: vec![authority.clone()],
        lease_seconds: 300,
        not_after: "2100-01-01T00:00:00.000Z".into(),
    }).unwrap();
    let AuthenticatedWorkload::Broker(identity) = store.authenticate_workload_token(&token)
        .unwrap().unwrap() else {
        panic!("the real workload binding must authenticate as Broker");
    };
    let mut seeded = SeededBroker {
        broker_id,
        incarnation_id: incarnation_id.clone(),
        private_values: vec![credential_id, token, digest, binding_operation, grant_operation,
            grant.receipt.input_hash, grant.receipt.audit_hash],
        checkpoint_received_at: None,
        checkpoint_expires_at: None,
    };
    if checkpoint {
        // A committed checkpoint with no heartbeat deliberately separates
        // publication freshness from liveness. No elapsed-time assertion.
        let job = uuid::Uuid::new_v4().to_string();
        let operation = uuid::Uuid::new_v4().to_string();
        let hash = "bc".repeat(32);
        let result = store.broker_checkpoint(&identity, &BrokerCheckpointV1 {
            version: 1,
            operation_id: operation.clone(),
            incarnation_id,
            previous_sequence: 0,
            sequence: 1,
            snapshot: BrokerCheckpointSnapshotV1 {
                version: 1,
                authority,
                jobs: vec![BrokerJobCheckpointV1 {
                    job_id: job.clone(), revision: 7, checkpoint_hash: hash.clone(),
                }],
                has_more: true,
            },
        }).unwrap();
        let observed = store.broker_checkpoint_for_identity(&identity).unwrap().checkpoint.unwrap();
        seeded.checkpoint_received_at = Some(observed.received_at);
        seeded.checkpoint_expires_at = Some(observed.lease_expires_at);
        seeded.private_values.extend([job, operation, hash, result.receipt.input_hash,
            result.receipt.audit_hash]);
    }
    seeded
}

async fn select(router: &Router, mode: &str) -> (Value, Value) {
    let (status, identity) = get(router, "/api/v1/human/identity", None).await;
    assert_eq!(status, StatusCode::OK, "{identity}");
    assert_eq!(identity["read_profiles"]["broker_registry"], "human_broker_registry_owned_v1");
    let context = identity["contexts"].as_array().unwrap().iter()
        .find(|context| context["tenant_id"] == "B" && context["mode"] == mode)
        .unwrap().clone();
    (identity, context)
}

fn keys(value: &Value, expected: &[&str]) {
    assert_eq!(value.as_object().unwrap().keys().map(String::as_str).collect::<BTreeSet<_>>(),
        expected.iter().copied().collect::<BTreeSet<_>>());
}

fn committed_receipt(
    database: &rusqlite::Connection,
    before: &Snapshot,
    page: &Value,
    identity: &Value,
    member: &MembershipV1,
) {
    let after = snapshot(database);
    assert_eq!(after.0, before.0 + 1);
    assert_eq!(after.2, before.2 + 1);
    assert!(after.3 > before.3);
    assert_ne!(after.1, before.1);
    let receipt = &page["read_receipt"];
    keys(receipt, &["request_id", "audit_event_id", "received_at"]);
    // Root-authorized test-only SELECT on the real read-only fixture
    // connection: the returned UUID must name this committed scoped event.
    // No SQL mutation, schema creation or authority construction occurs here.
    let (sequence, hash, received, payload): (i64, String, String, String) = database.query_row(
        "SELECT seq,hash,received_at,payload FROM events WHERE id=?1",
        [receipt["audit_event_id"].as_str().unwrap()],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    ).unwrap();
    assert_eq!(sequence, after.0);
    assert_eq!(hash, after.1);
    assert_eq!(received, receipt["received_at"].as_str().unwrap());
    let event: Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(event["id"], receipt["audit_event_id"]);
    assert_eq!(event["kind"], "control.identity.brokers.access");
    assert_eq!(event["source"], "gateway");
    assert_eq!(event["provenance"], "observed");
    let record = &event["data"];
    assert_eq!(record["request_id"], receipt["request_id"]);
    assert_eq!(record["principal"], identity["principal"]);
    assert_eq!(record["membership_id"], member.membership_id);
    assert_eq!(record["membership_revision"], member.revision);
    assert_eq!(record["mode"], "administration");
    assert_eq!(record["result_count"], page["items"].as_array().unwrap().len());
    assert!(record["authorization_revision"].as_u64().unwrap() > 0);
    let stored_member: String = database.query_row(
        "SELECT payload_json FROM identity_memberships WHERE membership_id=?1",
        [record["membership_id"].as_str().unwrap()], |row| row.get(0),
    ).unwrap();
    let stored_member: MembershipV1 = serde_json::from_str(&stored_member).unwrap();
    assert_eq!(stored_member, *member);
    assert_eq!(stored_member.tenant_id, "B");
    assert_eq!(serde_json::to_value(&stored_member.principal).unwrap(), identity["principal"]);
}

#[tokio::test]
async fn owned_brokers_returns_nonempty_whitelisted_pages_with_real_scoped_audit_receipts() {
    // Breaks on an empty/default DTO, wrong source index, missing audit,
    // credential/checkpoint leakage or broken keyset continuation.
    let fixture = Fixture::new_owned();
    let member = membership(&fixture, HumanCapabilityV1::ReadBrokerRegistry);
    let first = seed_broker(&fixture, "B", 10, true);
    let second = seed_broker(&fixture, "B", 20, false);
    let foreign = seed_broker(&fixture, "A", 5, true);
    let state = state(&fixture);
    let router = gateway_router(state.clone());
    let (identity, context) = select(&router, "administration").await;
    assert!(context["capabilities"].as_array().unwrap().iter().any(|cap| cap == "read_broker_registry"));
    let neighbor = crate::audit::read_test_observer::observe(&state.tenants[0].store);
    let database = connection(&state);
    let before = snapshot(&database);
    let usage = state.tenants[1].store.modern_test_human_usage();
    let (status, page) = get(&router, "/api/v1/human/owned/brokers?limit=1", context["id"].as_str()).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    keys(&page, &["profile", "version", "context_id", "authorization_version", "source",
        "consistency", "evaluated_at", "runtime_inventory", "count_scope", "items",
        "has_more", "next_cursor", "read_receipt"]);
    assert_eq!(page["profile"], "human_broker_registry_owned_v1");
    assert_eq!(page["version"], 1);
    assert_eq!(page["context_id"], context["id"]);
    assert_eq!(page["authorization_version"], context["authorization_version"]);
    assert_eq!(page["source"], "tenant_broker_registry");
    assert_eq!(page["consistency"], "live_keyset");
    assert_eq!(page["runtime_inventory"], "unknown");
    assert_eq!(page["count_scope"], "page");
    assert_eq!(page["has_more"], true);
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    let item = &items[0];
    keys(item, &["broker_id", "incarnation_id", "state", "clock_uncertain", "grant", "heartbeat", "checkpoint"]);
    assert_eq!(item["broker_id"], first.broker_id);
    assert_eq!(item["incarnation_id"], first.incarnation_id);
    keys(&item["grant"], &["revision", "revoked", "granted_at", "not_after", "revoked_at", "lease_seconds", "authorities"]);
    assert_eq!(item["grant"]["revision"], "1");
    assert_eq!(item["grant"]["revoked"], false);
    assert_eq!(item["grant"]["lease_seconds"], 300);
    assert_eq!(item["grant"]["authorities"].as_array().unwrap().len(), 1);
    keys(&item["grant"]["authorities"][0], &["authority_id", "runtime_kind", "authority_id_redacted"]);
    assert_eq!(item["grant"]["authorities"][0]["authority_id"], "synthetic-owned-live-host-10");
    assert_eq!(item["grant"]["authorities"][0]["runtime_kind"], "docker");
    assert_eq!(item["grant"]["authorities"][0]["authority_id_redacted"], false);
    keys(&item["heartbeat"], &["revision", "last", "current_incarnation"]);
    assert_eq!(item["heartbeat"], json!({"revision":"0", "last":null, "current_incarnation":false}));
    let checkpoint = &item["checkpoint"];
    keys(checkpoint, &["status", "received_at", "lease_expires_at", "freshness", "in_current_grant", "coverage", "reference_count", "has_more", "publication_provenance", "reception_provenance"]);
    assert_eq!(checkpoint["status"], "received");
    assert_eq!(checkpoint["received_at"].as_str(), first.checkpoint_received_at.as_deref());
    assert_eq!(checkpoint["lease_expires_at"].as_str(), first.checkpoint_expires_at.as_deref());
    assert_eq!(checkpoint["coverage"], "declared_partial");
    assert_eq!(checkpoint["reference_count"], 1);
    assert_eq!(checkpoint["has_more"], true);
    assert_eq!(checkpoint["in_current_grant"], true);
    assert_eq!(checkpoint["publication_provenance"], "declared");
    assert_eq!(checkpoint["reception_provenance"], "observed");
    let evaluated = chrono::DateTime::parse_from_rfc3339(page["evaluated_at"].as_str().unwrap()).unwrap();
    let received = chrono::DateTime::parse_from_rfc3339(checkpoint["received_at"].as_str().unwrap()).unwrap();
    let expires = chrono::DateTime::parse_from_rfc3339(checkpoint["lease_expires_at"].as_str().unwrap()).unwrap();
    let freshness = if evaluated < received { "clock_uncertain" } else if evaluated >= expires { "stale" } else { "current" };
    assert_eq!(checkpoint["freshness"], freshness);
    let granted = chrono::DateTime::parse_from_rfc3339(item["grant"]["granted_at"].as_str().unwrap()).unwrap();
    let grant_expires = chrono::DateTime::parse_from_rfc3339(item["grant"]["not_after"].as_str().unwrap()).unwrap();
    let liveness = if evaluated < granted { "clock_uncertain" } else if evaluated >= grant_expires { "expired" } else { "unseen" };
    assert_eq!(item["state"], liveness);
    assert_eq!(item["clock_uncertain"], freshness == "clock_uncertain" || liveness == "clock_uncertain");
    committed_receipt(&database, &before, &page, &identity, &member);
    assert_eq!(state.tenants[1].store.modern_test_human_usage()["tenant"]["events"].as_u64().unwrap(),
        usage["tenant"]["events"].as_u64().unwrap() + 1);
    let cursor = page["next_cursor"].as_str().unwrap();
    assert!(!cursor.is_empty());
    let before = snapshot(&database);
    let (status, last) = get(&router, &format!("/api/v1/human/owned/brokers?limit=1&cursor={cursor}"), context["id"].as_str()).await;
    assert_eq!(status, StatusCode::OK, "{last}");
    assert_eq!(last["items"].as_array().unwrap().len(), 1);
    assert_eq!(last["items"][0]["broker_id"], second.broker_id);
    assert_eq!(last["items"][0]["checkpoint"]["status"], "absent");
    for (key, value) in last["items"][0]["checkpoint"].as_object().unwrap() {
        if key != "status" { assert!(value.is_null(), "absent checkpoint field {key}"); }
    }
    assert_eq!(last["has_more"], false);
    assert!(last["next_cursor"].is_null());
    assert_eq!(last["count_scope"], "page");
    committed_receipt(&database, &before, &last, &identity, &member);
    assert_ne!(page["read_receipt"]["request_id"], last["read_receipt"]["request_id"]);
    let output = format!("{page}{last}");
    assert!(!output.contains(&foreign.broker_id));
    for private in first.private_values.iter().chain(&second.private_values).chain(&foreign.private_values) {
        assert!(!output.contains(private), "owned DTO leaked private registry metadata");
    }
    assert!(neighbor.try_recv().is_err(), "the selected source B must not read neighboring Store A");
}

async fn refused_read(capability: HumanCapabilityV1, mode: &str) {
    let fixture = Fixture::new_owned();
    membership(&fixture, capability);
    let broker = seed_broker(&fixture, "B", 10, true);
    let state = state(&fixture);
    let router = gateway_router(state.clone());
    let (_, context) = select(&router, mode).await;
    let database = connection(&state);
    let before = snapshot(&database);
    let usage = state.tenants[1].store.modern_test_human_usage();
    let (status, body) = get(&router, "/api/v1/human/owned/brokers", context["id"].as_str()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "permission_denied");
    assert!(body.get("items").is_none());
    assert!(body.get("read_receipt").is_none());
    assert!(!body.to_string().contains(&broker.broker_id));
    assert_eq!(snapshot(&database), before, "a refused read must not append a successful read receipt");
    assert_eq!(state.tenants[1].store.modern_test_human_usage(), usage);
}

#[tokio::test]
async fn owned_brokers_refuses_administration_without_registry_permission() {
    // Read-profile availability and another administrative capability do not
    // grant ReadBrokerRegistry.
    refused_read(HumanCapabilityV1::ReadTenantMetadata, "administration").await;
}

#[tokio::test]
async fn owned_brokers_refuses_personal_context_even_with_registry_membership() {
    // The same real membership's personal context cannot use its
    // Administration-only registry capability.
    refused_read(HumanCapabilityV1::ReadBrokerRegistry, "personal").await;
}
