//! Additive federation composition; existing worker-only authority is unchanged.
use super::*;

const ROUTE: &str = "/api/v1/human/federation/workers";
const CAPS: [HumanCapabilityV1; 2] = [
    HumanCapabilityV1::ReadWorkerRegistry,
    HumanCapabilityV1::ReadBrokerRegistry,
];

fn grant_broker(store: &Store, authority: &str) -> String {
    let broker_id = id();
    store
        .mutate_broker_grant(&BrokerGrantMutationV1 {
            version: 1,
            operation_id: id(),
            broker_id: broker_id.clone(),
            expected_revision: None,
            credential_id: id(),
            incarnation_id: id(),
            authorities: vec![BrokerAuthorityV1 {
                authority_id: authority.into(),
                runtime_kind: BrokerRuntimeKindV1::Docker,
            }],
            lease_seconds: 30,
            not_after: "2100-01-01T00:00:00.000Z".into(),
        })
        .unwrap();
    broker_id
}

#[tokio::test]
async fn federation_composes_two_brokers_and_workers_without_inventing_inventory() {
    let f = Fixture::new().await;
    f.upgrade10();
    let token = provision(&f.a, &CAPS);
    let context = f.context(&token, "administration").await;
    let a1_authority = id();
    let a2_authority = id();
    let a1_broker = grant_broker(&f.a, &a1_authority);
    let a2_broker = grant_broker(&f.a, &a2_authority);
    // A registered broker without any worker must remain visible, with unknown
    // runtime inventory. Absence from this worker page never establishes zero.
    let empty_broker = grant_broker(&f.a, &id());
    let a1 = reserve(&f.a, &a1_authority);
    let a2 = reserve(&f.a, &a2_authority);
    let b1_broker = grant_broker(&f.b, &a1_authority);
    let b1 = reserve(&f.b, &a1_authority);
    f.a.revoke_broker(&agentsandbox::broker_registry::BrokerRevocationV1 {
        version: 1,
        operation_id: id(),
        broker_id: a2_broker.clone(),
        expected_revision: 1,
    })
    .unwrap();
    let before = f.a.head().unwrap();
    let (status, page) = f.get(ROUTE, Some(&token), Some(&context)).await;
    assert_eq!(status, StatusCode::OK, "the additive route must exist");
    assert_eq!(f.a.head().unwrap(), before + 2);
    let mut keys = page
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "brokers",
            "consistency",
            "coverage",
            "runtime_inventory",
            "source",
            "version",
            "workers"
        ]
    );
    assert_eq!(page["version"], 1);
    assert_eq!(page["source"], "tenant_federation_registry");
    assert_eq!(page["consistency"], "independent_snapshots");
    assert_eq!(page["coverage"], "partial");
    assert_eq!(page["runtime_inventory"], "unknown");
    for (leg, consistency) in [("workers", "snapshot_per_page"), ("brokers", "live_keyset")] {
        assert_eq!(page[leg]["context_id"], context);
        assert_eq!(page[leg]["runtime_inventory"], "unknown");
        assert_eq!(page[leg]["consistency"], consistency);
        assert!(page[leg]["evaluated_at"].is_string());
        assert!(page[leg]["read_receipt"]["audit_event_id"].is_string());
        assert_eq!(page[leg]["has_more"], false);
    }
    assert_eq!(
        page["workers"]["authorization_version"],
        page["brokers"]["authorization_version"]
    );
    assert_ne!(
        page["workers"]["read_receipt"]["audit_event_id"],
        page["brokers"]["read_receipt"]["audit_event_id"]
    );
    let workers = page["workers"]["items"].as_array().unwrap();
    assert_eq!(workers.len(), 2);
    for (worker, broker, state) in [(&a1, &a1_broker, "unseen"), (&a2, &a2_broker, "revoked")] {
        let row = workers
            .iter()
            .find(|row| row["worker_id"] == worker.worker_id)
            .unwrap();
        assert_eq!(row["launch_broker"]["broker"]["broker_id"], *broker);
        assert_eq!(row["launch_broker"]["broker"]["state"], state);
        assert_eq!(row["state"], "reserved");
        assert_eq!(row["quota_held"], true);
        assert_eq!(row["last_observation"], Value::Null);
    }
    let brokers = page["brokers"]["items"].as_array().unwrap();
    assert_eq!(brokers.len(), 3);
    assert!(brokers.iter().any(|row| row["broker_id"] == empty_broker));
    let encoded = page.to_string();
    for forbidden in [&b1.worker_id, &b1_broker, &token] {
        assert!(!encoded.contains(forbidden));
    }
    assert!(page.get("total").is_none());
    assert!(f.a.integrity().unwrap().valid && f.b.integrity().unwrap().valid);
}

#[tokio::test]
async fn federation_requires_both_capabilities_before_either_inventory_read() {
    let f = Fixture::new().await;
    f.upgrade10();
    reserve(&f.a, &id());
    for capability in CAPS {
        let token = provision(&f.a, &[capability]);
        let context = f.context(&token, "administration").await;
        let before = f.a.head().unwrap();
        let (status, error) = f.get(ROUTE, Some(&token), Some(&context)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(error["error"]["code"], "permission_denied");
        assert!(error.get("workers").is_none() && error.get("brokers").is_none());
        assert_eq!(
            f.a.head().unwrap(),
            before,
            "neither leg may record an inventory read"
        );
    }
    let token = provision(&f.a, &CAPS);
    let personal = f.context(&token, "personal").await;
    let before = f.a.head().unwrap();
    assert_eq!(
        f.get(ROUTE, Some(&token), Some(&personal)).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(f.a.head().unwrap(), before);
}

#[tokio::test]
async fn federation_validates_inputs_and_authentication_without_inventory_receipts() {
    let f = Fixture::new().await;
    f.upgrade10();
    assert_eq!(
        f.get(&format!("{ROUTE}?worker_limit=0"), None, None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    for token in [
        "human-workers-test-runner-a".repeat(3),
        "human-workers-test-viewer-a".repeat(3),
    ] {
        assert_eq!(
            f.get(ROUTE, Some(&token), None).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    let token = provision(&f.a, &CAPS);
    let context = f.context(&token, "administration").await;
    for query in [
        "worker_limit=0",
        "broker_limit=101",
        "tenant_id=tenant-b",
        "broker_id=untrusted",
        "worker_limit=1&worker_limit=2",
    ] {
        let before = f.a.head().unwrap();
        assert_eq!(
            f.get(&format!("{ROUTE}?{query}"), Some(&token), Some(&context))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(f.a.head().unwrap(), before);
    }
}

#[tokio::test]
async fn federation_cursors_are_independent_and_bound_to_route_leg_limit_and_context() {
    let f = Fixture::new().await;
    f.upgrade10();
    let token = provision(&f.a, &CAPS);
    let context = f.context(&token, "administration").await;
    for _ in 0..2 {
        let authority = id();
        grant_broker(&f.a, &authority);
        reserve(&f.a, &authority);
    }
    let (status, page) = f
        .get(
            &format!("{ROUTE}?worker_limit=1&broker_limit=1"),
            Some(&token),
            Some(&context),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let workers = page["workers"]["next_cursor"].as_str().unwrap();
    let brokers = page["brokers"]["next_cursor"].as_str().unwrap();
    for path in [
        format!("{ROUTE}?worker_limit=1&broker_limit=1&worker_cursor={brokers}"),
        format!("{ROUTE}?worker_limit=1&broker_limit=1&broker_cursor={workers}"),
        format!("{ROUTE}?worker_limit=2&broker_limit=1&worker_cursor={workers}"),
        format!("/api/v1/human/workers?limit=1&cursor={workers}"),
        format!("/api/v1/human/brokers?limit=1&cursor={brokers}"),
    ] {
        let before = f.a.head().unwrap();
        assert_eq!(
            f.get(&path, Some(&token), Some(&context)).await.0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            f.a.head().unwrap(),
            before,
            "both cursor positions must validate before either leg is read"
        );
    }
    let (_, old) = f
        .get(
            "/api/v1/human/workers?limit=1",
            Some(&token),
            Some(&context),
        )
        .await;
    let old = old["next_cursor"].as_str().unwrap();
    let before = f.a.head().unwrap();
    assert_eq!(
        f.get(
            &format!("{ROUTE}?worker_limit=1&broker_limit=1&worker_cursor={old}"),
            Some(&token),
            Some(&context)
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(f.a.head().unwrap(), before);
    let (status, next) = f.get(&format!("{ROUTE}?worker_limit=1&broker_limit=1&worker_cursor={workers}&broker_cursor={brokers}"), Some(&token), Some(&context)).await;
    assert_eq!(status, StatusCode::OK);
    for leg in ["workers", "brokers"] {
        assert_eq!(next[leg]["has_more"], false);
        assert_ne!(page[leg]["items"][0], next[leg]["items"][0]);
    }
    let other = provision(&f.b, &CAPS);
    let foreign = f.context(&other, "administration").await;
    let before = f.b.head().unwrap();
    assert_eq!(
        f.get(
            &format!("{ROUTE}?worker_limit=1&worker_cursor={workers}"),
            Some(&other),
            Some(&foreign)
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(f.b.head().unwrap(), before);
}

#[tokio::test]
async fn federation_never_publishes_one_leg_when_the_second_read_budget_is_exhausted() {
    let f = Fixture::with_read_budget(Some(2), None).await;
    f.upgrade10();
    let token = provision(&f.a, &CAPS);
    let context = f.context(&token, "administration").await;
    let before = f.a.head().unwrap();
    let (status, error) = f.get(ROUTE, Some(&token), Some(&context)).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(error["error"]["code"], "audit_read_budget_exhausted");
    assert_eq!(
        f.a.head().unwrap(),
        before + 1,
        "the first durable read is not undone by the second refusal"
    );
    assert!(error.get("workers").is_none() && error.get("brokers").is_none());
    assert!(f.a.integrity().unwrap().valid);
}
