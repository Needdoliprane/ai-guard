//! Relay retention fairness through the real router, typed supervision and SQLite.
//! Worker runtime facts are synthetic: these software tests qualify no real transport.
mod support;

#[path = "support/relay_admission_fixture.rs"]
#[allow(dead_code)]
mod relay_admission_fixture;
use relay_admission_fixture::*;

use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::{AuditCapacityFailureV1, AuditCapacityV1, EventQuery, Store, WorkloadAuditPolicyV1},
    control::*,
    model::{CommandSpec, DecisionRequest, Event, StoredEvent},
    policy::{self, Policy},
    redaction::Redactor,
    relay::*,
    workload::{WorkloadBindingIssueV1, WorkloadSubjectV1},
};
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Arc};
use tower::ServiceExt;
use uuid::Uuid;

const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const HASH_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

fn runner_a() -> String {
    "a".repeat(48)
}

fn router(a: Arc<Store>, b: Arc<Store>) -> Router {
    gateway_router(
        GatewayState::for_tenants(
            vec![
                TenantGateway {
                    tenant_id: "tenant-a".into(),
                    store: a,
                    runner_token: runner_a(),
                    viewer_token: "v".repeat(48),
                },
                TenantGateway {
                    tenant_id: "tenant-b".into(),
                    store: b,
                    runner_token: "b".repeat(48),
                    viewer_token: "w".repeat(48),
                },
            ],
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )
        .unwrap(),
    )
}

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    token: &str,
    body: Value,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn code(answer: &Value) -> &str {
    answer["error"]["code"]
        .as_str()
        .unwrap_or("<no error code>")
}

fn directed(source: &WorkerV1, destination: &WorkerV1, channel: &str) -> RelayGrantV1 {
    RelayGrantV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        grant_id: Uuid::new_v4().to_string(),
        source: endpoint(source),
        destination: endpoint(destination),
        channel: channel.into(),
        expires_at: (chrono::Utc::now() + chrono::Duration::seconds(600)).to_rfc3339(),
    }
}

async fn authorize(app: &Router, grant: &RelayGrantV1) {
    let (status, answer) = call(
        app,
        "POST",
        "/api/v1/relay/grants",
        &runner_a(),
        json!(grant),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "fixture grant refused: {answer}"
    );
}

fn message(grant: &RelayGrantV1, ttl_seconds: u32, body: &str) -> RelaySendV1 {
    RelaySendV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        grant_id: grant.grant_id.clone(),
        ttl_seconds,
        body: body.into(),
    }
}

/// Deliver one send operation and return the relay's own verdict. The tenant's
/// shared I/O admission gate (429 `capacity_exhausted`) is neither a RED nor a
/// GREEN of this lot: the same operation is replayed in a later window.
async fn relay_send(app: &Router, token: &str, input: &RelaySendV1) -> (StatusCode, Value) {
    for _ in 0..80 {
        let (status, answer) =
            call(app, "POST", "/api/v1/relay/messages", token, json!(input)).await;
        if status == StatusCode::TOO_MANY_REQUESTS && code(&answer) == "capacity_exhausted" {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            continue;
        }
        return (status, answer);
    }
    panic!("the shared I/O admission gate never admitted the fixture send");
}

struct Fill {
    accepted: usize,
    refusal: Option<Value>,
}

/// Round-robin over the grants (the durable rate is per grant) until the first
/// relay refusal or `attempts` accepted sends. Any other answer is a fixture
/// failure, never a reproduction.
async fn fill(
    app: &Router,
    token: &str,
    grants: &[RelayGrantV1],
    body: &str,
    ttl_seconds: u32,
    attempts: usize,
) -> Fill {
    for index in 0..attempts {
        let input = message(&grants[index % grants.len()], ttl_seconds, body);
        let (status, answer) = relay_send(app, token, &input).await;
        if status == StatusCode::TOO_MANY_REQUESTS {
            assert!(
                code(&answer).starts_with("relay_") && code(&answer) != "relay_rate_limited",
                "fill stopped on a refusal that is not a retention verdict: {answer}"
            );
            return Fill {
                accepted: index,
                refusal: Some(answer),
            };
        }
        assert_eq!(status, StatusCode::CREATED, "fixture send failed: {answer}");
    }
    Fill {
        accepted: attempts,
        refusal: None,
    }
}

/// Physically retained bodies, read through a separate connection.
fn retained(path: &Path) -> (i64, i64) {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT COUNT(*),COALESCE(SUM(body_bytes),0) FROM relay_messages WHERE body IS NOT NULL",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap()
}

/// A history is conformant only if a fresh process accepts it: the in-process
/// `integrity()` does not replay relay rules over this Store's own writes.
fn assert_reopens_valid(path: &Path, tenant: &str, max_events: u64) {
    let reopened =
        Store::open_existing_for_tenant(path, tenant, Redactor::new(vec![]), max_events).unwrap();
    assert!(
        reopened.healthy(),
        "the reopened store rejected its own history"
    );
    let integrity = reopened.integrity().unwrap();
    assert!(
        integrity.valid,
        "reopened history is invalid: {integrity:?}"
    );
}

struct Shipped {
    directory: tempfile::TempDir,
    a: Arc<Store>,
    app: Router,
    token1: String,
    token2: String,
    outbound: Vec<RelayGrantV1>,
    reverse: RelayGrantV1,
}

const SHIPPED_EVENTS: u64 = 100_000;
const SMALL_EVENTS: u64 = 20_000;
/// Distinctive content: it must never appear in a refusal, a diagnostic or an audit fact.
const MARKER: &str = "relay-body-marker-7f3c";

fn capacity(messages: u64, bytes: u64) -> RelayCapacityV1 {
    RelayCapacityV1 { messages, bytes }
}
fn budgets(
    tenant: RelayCapacityV1,
    sender: RelayCapacityV1,
    grant: RelayCapacityV1,
) -> RelayRetentionPolicyV1 {
    RelayRetentionPolicyV1 {
        tenant,
        sender,
        grant,
        ..RelayRetentionPolicyV1::default()
    }
}

/// The standard local recipe: A1 and A2 belong to tenant A, B1 to tenant B.
struct World {
    directory: tempfile::TempDir,
    a: Arc<Store>,
    b: Arc<Store>,
    app: Router,
    a1: WorkerV1,
    a2: WorkerV1,
    b1: WorkerV1,
    token_a1: String,
    token_a2: String,
    token_b1: String,
}
impl World {
    fn path_a(&self) -> std::path::PathBuf {
        self.directory.path().join("tenant-a.sqlite")
    }
}
async fn world(policy: RelayRetentionPolicyV1) -> World {
    world_with(policy, SMALL_EVENTS, None).await
}
async fn world_with(
    policy: RelayRetentionPolicyV1,
    max_events: u64,
    workload_audit: Option<WorkloadAuditPolicyV1>,
) -> World {
    let directory = support::private_tempdir();
    let mut store = fresh(directory.path(), "tenant-a", max_events)
        .with_relay_retention_policy(policy)
        .unwrap();
    if let Some(workload_audit) = workload_audit {
        store = store.with_workload_audit_policy(workload_audit).unwrap();
    }
    let a = Arc::new(store);
    let b = Arc::new(fresh(directory.path(), "tenant-b", max_events));
    let a1 = running_now(&a);
    let a2 = running_now(&a);
    let b1 = running_now(&b);
    let token_a1 = "1".repeat(48);
    let token_a2 = "2".repeat(48);
    let token_b1 = "3".repeat(48);
    bind(&a, &a1, &token_a1);
    bind(&a, &a2, &token_a2);
    bind(&b, &b1, &token_b1);
    let app = router(a.clone(), b.clone());
    World {
        directory,
        a,
        b,
        app,
        a1,
        a2,
        b1,
        token_a1,
        token_a2,
        token_b1,
    }
}
async fn inbox(app: &Router, token: &str) -> Vec<Value> {
    for _ in 0..80 {
        let (status, answer) = call(
            app,
            "GET",
            "/api/v1/relay/messages?limit=32",
            token,
            Value::Null,
        )
        .await;
        if status == StatusCode::TOO_MANY_REQUESTS && code(&answer) == "capacity_exhausted" {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            continue;
        }
        assert_eq!(status, StatusCode::OK, "inbox read failed: {answer}");
        return answer["items"].as_array().unwrap().clone();
    }
    panic!("the shared I/O admission gate never admitted the fixture inbox read");
}
fn with_batch(
    policy: RelayRetentionPolicyV1,
    automatic_prune_batch: u32,
) -> RelayRetentionPolicyV1 {
    RelayRetentionPolicyV1 {
        automatic_prune_batch,
        ..policy
    }
}
fn count(path: &Path, query: &str) -> i64 {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row(query, [], |row| row.get(0))
        .unwrap()
}
/// Messages whose body was removed; their metadata and audit facts remain.
fn released(path: &Path) -> i64 {
    count(
        path,
        "SELECT COUNT(*) FROM relay_messages WHERE body IS NULL",
    )
}
fn body_present(path: &Path, message_id: &str) -> bool {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT body IS NOT NULL FROM relay_messages WHERE message_id=?1",
            [message_id],
            |row| row.get(0),
        )
        .unwrap()
}
fn expiry_of(path: &Path, message_id: &str) -> String {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT expires_at FROM relay_messages WHERE message_id=?1",
            [message_id],
            |row| row.get(0),
        )
        .unwrap()
}
fn events_after(store: &Store, after: i64) -> Vec<StoredEvent> {
    store
        .events(&EventQuery {
            after: Some(after),
            order: Some("asc".into()),
            limit: Some(1000),
            ..Default::default()
        })
        .unwrap()
}
fn relay_facts(store: &Store, after: i64) -> Vec<StoredEvent> {
    events_after(store, after)
        .into_iter()
        .filter(|stored| stored.event.kind == "control.relay.mutation")
        .collect()
}
/// Provenance of an automatic release: the triggering send operation and the
/// authenticated worker that sent it. Neither has to own the released body,
/// and the fact itself stays unattributed.
fn automatic_release_of(trigger: &RelaySendV1, sender: &WorkerV1) -> Option<String> {
    Some(format!(
        "{AUTOMATIC_PRUNE_CORRELATION_PREFIX}{}:{}",
        trigger.operation_id, sender.worker_id
    ))
}
/// Scope and action of a relay 429, read through the published wire enums: a
/// value those enums do not know is a contract defect, not a fixture failure.
fn published(answer: &Value) -> (RelayRefusalScopeV1, RelayRetryActionV1) {
    let error = &answer["error"];
    let scope = serde_json::from_value(error["scope"].clone())
        .unwrap_or_else(|_| panic!("relay refusal scope is not a published value: {answer}"));
    let action = serde_json::from_value(error["retry_action"].clone())
        .unwrap_or_else(|_| panic!("relay refusal action is not a published value: {answer}"));
    (scope, action)
}
/// A refusal that describes no sender: neither occupancy nor deadline.
fn assert_only_scope_and_action(answer: &Value, disclosure: &str) {
    let mut fields: Vec<_> = answer["error"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    fields.sort_unstable();
    assert_eq!(
        fields,
        ["code", "message", "retry_action", "scope"],
        "{disclosure}: {answer}"
    );
}
/// The relay validator requires every relay fact to be dated at or after the
/// reception of all earlier events. Timestamps are canonical millisecond UTC
/// strings, so they order lexicographically. `after` is an existing sequence.
fn assert_relay_facts_follow_the_time_floor(store: &Store, after: i64) {
    let events = events_after(store, after - 1);
    for pair in events.windows(2) {
        if pair[1].event.kind == "control.relay.mutation" {
            assert!(
                pair[1].event.timestamp >= pair[0].received_at,
                "relay fact {} is dated {} before the reception {} of event {}: the history cannot be reopened",
                pair[1].seq,
                pair[1].event.timestamp,
                pair[0].received_at,
                pair[0].seq
            );
        }
    }
}
async fn runner_prune(app: &Router, operation_id: &str, message_id: &str) -> (StatusCode, Value) {
    call(
        app,
        "POST",
        &format!("/api/v1/relay/messages/{message_id}/prune"),
        &runner_a(),
        json!(RelayPruneV1 {
            version: 1,
            operation_id: operation_id.into(),
            message_id: message_id.into(),
        }),
    )
    .await
}
fn earliest_retained_expiry(path: &Path) -> String {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT MIN(expires_at) FROM relay_messages WHERE body IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

/// A1 and A2 under the shipped limits. The durable rate is 60 sends per grant
/// and minute, so five directed grants A1→A2 carry the 256-message fill.
async fn shipped() -> Shipped {
    let directory = support::private_tempdir();
    let a = Arc::new(fresh(directory.path(), "tenant-a", SHIPPED_EVENTS));
    let b = Arc::new(fresh(directory.path(), "tenant-b", SHIPPED_EVENTS));
    let a1 = running_now(&a);
    let a2 = running_now(&a);
    let token1 = "1".repeat(48);
    let token2 = "2".repeat(48);
    bind(&a, &a1, &token1);
    bind(&a, &a2, &token2);
    let app = router(a.clone(), b);
    let outbound: Vec<_> = (0..5)
        .map(|index| directed(&a1, &a2, &format!("fill-{index}")))
        .collect();
    for grant in &outbound {
        authorize(&app, grant).await;
    }
    let reverse = directed(&a2, &a1, "tasks");
    authorize(&app, &reverse).await;
    Shipped {
        directory,
        a,
        app,
        token1,
        token2,
        outbound,
        reverse,
    }
}

#[tokio::test]
async fn saturated_sender_does_not_starve_its_neighbor_at_shipped_limits() {
    let fixture = shipped().await;
    let path = fixture.directory.path().join("tenant-a.sqlite");
    let body = "x".repeat(MAX_MESSAGE_BYTES);
    let filled = fill(
        &fixture.app,
        &fixture.token1,
        &fixture.outbound,
        &body,
        60,
        256,
    )
    .await;
    let occupied = retained(&path);
    // Discriminating oracle: the fill ended on a retention verdict, or A1 alone
    // reached the tenant ceiling. Nothing else may explain the neighbor's answer.
    assert!(
        filled.refusal.is_some() || occupied == (256, 16_777_216),
        "fill ended early without a retention verdict: {} accepted, {occupied:?} retained",
        filled.accepted
    );
    let (status, answer) = relay_send(
        &fixture.app,
        &fixture.token2,
        &message(&fixture.reverse, 60, "n"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "a saturated sender starved its neighbor: A1 retains {occupied:?} (messages, bytes) after {} accepted sends and A2 received {}",
        filled.accepted,
        code(&answer)
    );
    let refusal = filled
        .refusal
        .expect("A1 exceeded its own share without a typed refusal");
    assert_eq!(code(&refusal), "relay_sender_capacity_exhausted");
    assert_eq!(refusal["error"]["scope"], "sender");
    assert_eq!(refusal["error"]["dimension"], "bytes");
    assert_eq!(filled.accepted, 32, "shipped sender share is 2 MiB");
    assert_eq!(occupied, (32, 2_097_152));
    assert!(fixture.a.integrity().unwrap().valid);
}

#[tokio::test]
async fn small_budgets_bound_each_sender_and_grant_without_touching_the_neighbor_or_tenant_b() {
    let w = world(budgets(
        capacity(8, 524_288),
        capacity(3, 196_608),
        capacity(2, 131_072),
    ))
    .await;
    let tasks = directed(&w.a1, &w.a2, "tasks");
    let results = directed(&w.a1, &w.a2, "results");
    let reverse = directed(&w.a2, &w.a1, "tasks");
    for grant in [&tasks, &results, &reverse] {
        authorize(&w.app, grant).await;
    }
    let b_head = w.b.head().unwrap();
    let body = format!("{MARKER} task");
    for _ in 0..2 {
        let (status, answer) = relay_send(&w.app, &w.token_a1, &message(&tasks, 60, &body)).await;
        assert_eq!(status, StatusCode::CREATED, "{answer}");
    }

    // The directed grant is full; the sender still has room on another grant.
    let (status, answer) = relay_send(&w.app, &w.token_a1, &message(&tasks, 60, &body)).await;
    assert_eq!(
        (status, code(&answer)),
        (
            StatusCode::TOO_MANY_REQUESTS,
            "relay_grant_capacity_exhausted"
        ),
        "the grant share did not bound its own retained bodies: {answer}"
    );
    let error = &answer["error"];
    assert_eq!(error["scope"], "grant");
    assert_eq!(error["dimension"], "messages");
    assert_eq!(error["limit"], json!({"messages":2,"bytes":131_072}));
    assert_eq!(
        error["retained"],
        json!({"messages":2,"bytes":2 * body.len()})
    );
    assert_eq!(error["requested_bytes"], body.len());
    assert_eq!(
        published(&answer),
        (
            RelayRefusalScopeV1::Grant,
            RelayRetryActionV1::RetryAfterRelease
        )
    );
    assert!(!answer.to_string().contains(MARKER));
    let (status, answer) = relay_send(&w.app, &w.token_a1, &message(&results, 60, &body)).await;
    assert_eq!(status, StatusCode::CREATED, "{answer}");

    // The sender is now full on every grant, and learns its own next deadline.
    let (status, answer) = relay_send(&w.app, &w.token_a1, &message(&results, 60, &body)).await;
    assert_eq!(
        (status, code(&answer)),
        (
            StatusCode::TOO_MANY_REQUESTS,
            "relay_sender_capacity_exhausted"
        ),
        "{answer}"
    );
    let error = &answer["error"];
    assert_eq!(error["scope"], "sender");
    assert_eq!(error["dimension"], "messages");
    assert_eq!(error["limit"], json!({"messages":3,"bytes":196_608}));
    assert_eq!(error["retained"]["messages"], 3);
    assert_eq!(
        error["next_release_at"],
        earliest_retained_expiry(&w.path_a())
    );
    assert_eq!(
        published(&answer),
        (
            RelayRefusalScopeV1::Sender,
            RelayRetryActionV1::RetryAfterRelease
        )
    );
    assert!(!answer.to_string().contains(MARKER));

    // The saturated sender does not deprive its neighbor.
    let (status, answer) = relay_send(&w.app, &w.token_a2, &message(&reverse, 60, "pong")).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "a saturated sender starved its neighbor: {answer}"
    );

    // Authorized communication is delivered to its directed destination only.
    let delivered = inbox(&w.app, &w.token_a2).await;
    assert_eq!(delivered.len(), 3);
    assert!(delivered.iter().all(|item| item["body"] == body));
    assert_eq!(inbox(&w.app, &w.token_a1).await.len(), 1);

    // No exchange between tenants: neither a grant towards B1 nor B1 using A's grant.
    let cross = directed(&w.a1, &w.b1, "tasks");
    let (status, answer) = call(
        &w.app,
        "POST",
        "/api/v1/relay/grants",
        &runner_a(),
        json!(cross),
    )
    .await;
    assert_eq!(
        (status, code(&answer)),
        (StatusCode::FORBIDDEN, "relay_forbidden")
    );
    let (status, answer) =
        relay_send(&w.app, &w.token_b1, &message(&reverse, 60, "intrusion")).await;
    assert_eq!(
        (status, code(&answer)),
        (StatusCode::FORBIDDEN, "relay_forbidden")
    );
    assert_eq!(
        w.b.head().unwrap(),
        b_head,
        "tenant A's relay traffic or B1's refused attempt wrote into tenant B"
    );
    assert_eq!(
        retained(&w.directory.path().join("tenant-b.sqlite")),
        (0, 0)
    );

    // Revocation closes the grant for new sends and for queued deliveries.
    let revoke = RelayRevokeV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        grant_id: tasks.grant_id.clone(),
    };
    let (status, answer) = call(
        &w.app,
        "POST",
        &format!("/api/v1/relay/grants/{}/revoke", tasks.grant_id),
        &runner_a(),
        json!(revoke),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let (status, answer) = relay_send(&w.app, &w.token_a1, &message(&tasks, 60, &body)).await;
    assert_eq!(
        (status, code(&answer)),
        (StatusCode::FORBIDDEN, "relay_forbidden")
    );
    let delivered = inbox(&w.app, &w.token_a2).await;
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0]["metadata"]["grant_id"], results.grant_id);
    assert!(w.a.integrity().unwrap().valid);

    let path = w.path_a();
    let World { a, b, app, .. } = w;
    drop(app);
    drop(a);
    drop(b);
    assert_reopens_valid(&path, "tenant-a", SMALL_EVENTS);
}

#[tokio::test]
async fn byte_budget_is_enforced_and_tenant_refusal_hides_neighbor_usage() {
    let w = world(budgets(
        capacity(12, 393_216),
        capacity(6, 131_072),
        capacity(6, 131_072),
    ))
    .await;
    let tasks = directed(&w.a1, &w.a2, "tasks");
    authorize(&w.app, &tasks).await;
    let full = "x".repeat(MAX_MESSAGE_BYTES);
    for _ in 0..2 {
        let (status, answer) = relay_send(&w.app, &w.token_a1, &message(&tasks, 60, &full)).await;
        assert_eq!(status, StatusCode::CREATED, "{answer}");
    }
    let (status, answer) = relay_send(&w.app, &w.token_a1, &message(&tasks, 60, "y")).await;
    assert_eq!(
        (status, code(&answer)),
        (
            StatusCode::TOO_MANY_REQUESTS,
            "relay_sender_capacity_exhausted"
        ),
        "the sender byte share did not bound its retained bodies: {answer}"
    );
    let error = &answer["error"];
    assert_eq!(error["dimension"], "bytes");
    assert_eq!(error["requested_bytes"], 1);
    assert_eq!(error["retained"], json!({"messages":2,"bytes":131_072}));
    assert_eq!(error["limit"], json!({"messages":6,"bytes":131_072}));

    // A share is a ceiling, not a reservation: with more saturated senders than
    // tenant ÷ sender, a sender under its own share meets the tenant ceiling.
    // That refusal describes no neighbor. It only says what the requester can
    // do: retry later while sends keep releasing expired bodies, or contact
    // the operator when sends release nothing. With a zero batch the tenant
    // answer stays pessimistic even when a neighbor could acknowledge a live
    // body, so the answer does not reveal that neighbor's acknowledgement state.
    for (automatic_prune_batch, release_on_ack, action) in [
        (8, false, RelayRetryActionV1::RetryAfterRelease),
        (0, false, RelayRetryActionV1::ContactOperator),
        (0, true, RelayRetryActionV1::ContactOperator),
    ] {
        let w = world(RelayRetentionPolicyV1 {
            release_on_ack,
            ..with_batch(
                budgets(
                    capacity(2, 131_072),
                    capacity(1, 65_536),
                    capacity(1, 65_536),
                ),
                automatic_prune_batch,
            )
        })
        .await;
        let a3 = running_now(&w.a);
        let token_a3 = "4".repeat(48);
        bind(&w.a, &a3, &token_a3);
        let from_a1 = directed(&w.a1, &w.a2, "tasks");
        let from_a2 = directed(&w.a2, &w.a1, "tasks");
        let from_a3 = directed(&a3, &w.a1, "tasks");
        for grant in [&from_a1, &from_a2, &from_a3] {
            authorize(&w.app, grant).await;
        }
        for (token, grant) in [(&w.token_a1, &from_a1), (&w.token_a2, &from_a2)] {
            let (status, answer) = relay_send(&w.app, token, &message(grant, 60, MARKER)).await;
            assert_eq!(status, StatusCode::CREATED, "{answer}");
        }
        let (status, answer) = relay_send(&w.app, &token_a3, &message(&from_a3, 60, "z")).await;
        assert_eq!(
            (status, code(&answer)),
            (StatusCode::TOO_MANY_REQUESTS, "relay_capacity_exhausted"),
            "{answer}"
        );
        assert_eq!(
            published(&answer),
            (RelayRefusalScopeV1::Tenant, action),
            "automatic_prune_batch {automatic_prune_batch}, release_on_ack {release_on_ack}: {answer}"
        );
        assert_only_scope_and_action(
            &answer,
            "a tenant-scope refusal disclosed occupancy or deadlines of other senders",
        );
        assert!(w.a.integrity().unwrap().valid);
    }
}

#[tokio::test]
async fn tenant_ceiling_refusal_asks_to_retry_and_any_sender_drains_expired_bodies_of_another() {
    // Two bodies of A1 and two of A2 fill the tenant exactly, in both dimensions.
    let w = world(with_batch(
        budgets(
            capacity(4, 131_072),
            capacity(2, 65_536),
            capacity(2, 65_536),
        ),
        1,
    ))
    .await;
    let path = w.path_a();
    let a3 = running_now(&w.a);
    let token_a3 = "4".repeat(48);
    bind(&w.a, &a3, &token_a3);
    let from_a1 = directed(&w.a1, &w.a2, "tasks");
    let from_a2 = directed(&w.a2, &w.a1, "tasks");
    let from_a3 = directed(&a3, &w.a1, "tasks");
    for grant in [&from_a1, &from_a2, &from_a3] {
        authorize(&w.app, grant).await;
    }
    let a1_small = message(&from_a1, 3, "s");
    let a1_large = message(&from_a1, 3, &"x".repeat(65_535));
    let a2_small = message(&from_a2, 60, "t");
    let a2_large = message(&from_a2, 60, &"y".repeat(65_535));
    for (token, input) in [
        (&w.token_a1, &a1_small),
        (&w.token_a1, &a1_large),
        (&w.token_a2, &a2_small),
        (&w.token_a2, &a2_large),
    ] {
        let (status, answer) = relay_send(&w.app, token, input).await;
        assert_eq!(status, StatusCode::CREATED, "{answer}");
        // Distinct expiries: `a1_small` is the oldest body of the tenant.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(retained(&path), (4, 131_072));

    // A3 retains nothing: only the shared ceiling refuses it. Nothing has
    // expired yet, so nothing is released and nothing is written.
    let head = w.a.head().unwrap();
    let blocked = message(&from_a3, 60, &"z".repeat(65_536));
    let (status, answer) = relay_send(&w.app, &token_a3, &blocked).await;
    assert!(
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
            < expiry_of(&path, &a1_small.operation_id),
        "fixture too slow: bodies expired during the fill"
    );
    assert_eq!(
        (status, code(&answer)),
        (StatusCode::TOO_MANY_REQUESTS, "relay_capacity_exhausted"),
        "{answer}"
    );
    assert_eq!(
        published(&answer),
        (
            RelayRefusalScopeV1::Tenant,
            RelayRetryActionV1::RetryAfterRelease
        ),
        "a sender under its own share was told that only the operator frees the tenant ceiling, although later sends release expired bodies: {answer}"
    );
    assert_only_scope_and_action(
        &answer,
        "a tenant-scope refusal disclosed occupancy or deadlines of other senders",
    );
    assert_eq!(
        w.a.head().unwrap(),
        head,
        "a refused send wrote a fact although no body had expired"
    );

    // Once A1's bodies have expired, A3's retry of the same operation releases
    // the oldest one (one per send here). The ceiling still refuses A3 in
    // bytes: the release is kept, audited, and names the triggering worker.
    until_expired().await;
    let (status, answer) = relay_send(&w.app, &token_a3, &blocked).await;
    assert_eq!(
        (status, code(&answer)),
        (StatusCode::TOO_MANY_REQUESTS, "relay_capacity_exhausted"),
        "{answer}"
    );
    assert_eq!(
        published(&answer),
        (
            RelayRefusalScopeV1::Tenant,
            RelayRetryActionV1::RetryAfterRelease
        )
    );
    assert_only_scope_and_action(
        &answer,
        "a tenant-scope refusal disclosed occupancy or deadlines of other senders",
    );
    let facts = relay_facts(&w.a, head);
    assert_eq!(facts.len(), 1, "{facts:?}");
    let record = &facts[0].event.data;
    let metadata = &record["resource"]["metadata"];
    assert_eq!(record["action"], "prune");
    assert!(
        record["actor"].is_null(),
        "a release was attributed to a worker"
    );
    assert_eq!(metadata["message_id"], a1_small.operation_id);
    assert_eq!(metadata["source"]["worker_id"], w.a1.worker_id);
    assert_eq!(
        facts[0].event.correlation_id,
        automatic_release_of(&blocked, &a3),
        "the release does not name the authenticated worker whose send triggered it"
    );
    assert!(!body_present(&path, &a1_small.operation_id));
    assert_eq!(retained(&path), (3, 131_071));

    // The next retry releases A1's second expired body and is admitted.
    let (status, answer) = relay_send(&w.app, &token_a3, &blocked).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "the retry announced by the tenant refusal did not progress: {answer}"
    );
    let facts = relay_facts(&w.a, head);
    assert_eq!(facts.len(), 3, "{facts:?}");
    assert_eq!(facts[1].event.data["action"], "prune");
    assert_eq!(
        facts[1].event.data["resource"]["metadata"]["message_id"],
        a1_large.operation_id
    );
    assert_eq!(
        facts[1].event.correlation_id,
        automatic_release_of(&blocked, &a3)
    );
    assert_eq!(facts[2].event.data["action"], "send");
    assert_eq!(facts[2].event.correlation_id, None);
    // A2's live bodies are untouched; only expired content was released.
    assert!(body_present(&path, &a2_small.operation_id));
    assert!(body_present(&path, &a2_large.operation_id));
    assert_eq!(retained(&path), (3, 131_072));
    assert!(
        !serde_json::to_string(&facts).unwrap().contains(MARKER),
        "an audit fact duplicated a message body"
    );
    assert_relay_facts_follow_the_time_floor(&w.a, head);
    assert!(w.a.integrity().unwrap().valid);
    let World { a, b, app, .. } = w;
    drop(app);
    drop(a);
    drop(b);
    assert_reopens_valid(&path, "tenant-a", SMALL_EVENTS);
}

#[tokio::test]
async fn exact_replays_never_debit_capacity_twice() {
    let w = world(budgets(
        capacity(8, 524_288),
        capacity(3, 196_608),
        capacity(3, 196_608),
    ))
    .await;
    let tasks = directed(&w.a1, &w.a2, "tasks");
    authorize(&w.app, &tasks).await;
    let first = message(&tasks, 60, "first");
    let (status, receipt) = relay_send(&w.app, &w.token_a1, &first).await;
    assert_eq!(status, StatusCode::CREATED, "{receipt}");
    let head = w.a.head().unwrap();
    // Characterization: replays were already resolved before any usage read on
    // the base. This part has no natural RED; it guards the new accounting.
    for _ in 0..10 {
        let (status, replayed) = relay_send(&w.app, &w.token_a1, &first).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(replayed["replayed"], true);
        assert_eq!(replayed["audit_seq"], receipt["audit_seq"]);
        assert_eq!(replayed["audit_hash"], receipt["audit_hash"]);
    }
    assert_eq!(
        w.a.head().unwrap(),
        head,
        "an exact replay wrote an audit fact"
    );
    assert_eq!(retained(&w.path_a()), (1, 5));
    for body in ["second", "third"] {
        let (status, answer) = relay_send(&w.app, &w.token_a1, &message(&tasks, 60, body)).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "ten replays consumed retained capacity: {answer}"
        );
    }
    let (status, answer) = relay_send(&w.app, &w.token_a1, &message(&tasks, 60, "fourth")).await;
    assert_eq!(
        (status, code(&answer)),
        (
            StatusCode::TOO_MANY_REQUESTS,
            "relay_sender_capacity_exhausted"
        ),
        "the sender share admitted a fourth retained body: {answer}"
    );
    // At full capacity the original operation still returns its receipt.
    let (status, replayed) = relay_send(&w.app, &w.token_a1, &first).await;
    assert_eq!(status, StatusCode::OK, "{replayed}");
    assert_eq!(replayed["replayed"], true);
    assert_eq!(replayed["audit_seq"], receipt["audit_seq"]);
    assert_eq!(retained(&w.path_a()).0, 3);

    // Different content under the same operation identity stays a conflict at
    // a full share: never a capacity verdict, never a release, never a write.
    let head = w.a.head().unwrap();
    let divergent = RelaySendV1 {
        body: "another body".into(),
        ..first.clone()
    };
    let (status, answer) = relay_send(&w.app, &w.token_a1, &divergent).await;
    assert_eq!(
        (status, code(&answer)),
        (StatusCode::CONFLICT, "relay_conflict"),
        "a divergent replay at a full share was not reported as a conflict: {answer}"
    );
    assert_eq!(
        w.a.head().unwrap(),
        head,
        "a conflicting replay wrote an audit fact"
    );
    assert_eq!(retained(&w.path_a()).0, 3);
    assert!(w.a.integrity().unwrap().valid);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_sends_admit_exactly_the_budget() {
    let w = world(budgets(
        capacity(8, 524_288),
        capacity(4, 262_144),
        capacity(4, 262_144),
    ))
    .await;
    let tasks = directed(&w.a1, &w.a2, "tasks");
    authorize(&w.app, &tasks).await;
    let mut attempts = Vec::new();
    for index in 0..12 {
        let app = w.app.clone();
        let token = w.token_a1.clone();
        let input = message(&tasks, 60, &format!("concurrent {index}"));
        attempts.push(tokio::spawn(async move {
            relay_send(&app, &token, &input).await
        }));
    }
    let mut admitted = 0;
    let mut refusals = Vec::new();
    for attempt in attempts {
        let (status, answer) = attempt.await.unwrap();
        if status == StatusCode::CREATED {
            admitted += 1;
        } else {
            refusals.push((status, code(&answer).to_owned()));
        }
    }
    assert_eq!(
        (admitted, refusals.len()),
        (4, 8),
        "concurrent sends did not admit exactly the sender share: {refusals:?}"
    );
    assert!(
        refusals.iter().all(|(status, code)| {
            *status == StatusCode::TOO_MANY_REQUESTS && code == "relay_sender_capacity_exhausted"
        }),
        "{refusals:?}"
    );
    assert_eq!(retained(&w.path_a()).0, 4);
    assert!(w.a.integrity().unwrap().valid);
    let path = w.path_a();
    let World { a, b, app, .. } = w;
    drop(app);
    drop(a);
    drop(b);
    assert_reopens_valid(&path, "tenant-a", SMALL_EVENTS);
}
#[tokio::test]
async fn expired_bodies_do_not_hold_capacity_at_shipped_limits() {
    let fixture = shipped().await;
    let path = fixture.directory.path().join("tenant-a.sqlite");
    let body = "x".repeat(MAX_MESSAGE_BYTES);
    let filled = fill(
        &fixture.app,
        &fixture.token1,
        &fixture.outbound,
        &body,
        1,
        256,
    )
    .await;
    // Every body sent above carries a one-second TTL.
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    let before = retained(&path);
    let after_expiry = message(&fixture.outbound[0], 60, "after expiry");
    let (status, answer) = relay_send(&fixture.app, &fixture.token1, &after_expiry).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "expired bodies still hold retained capacity: {before:?} retained (messages, bytes), all past their TTL, after {} accepted sends; A1 received {}",
        filled.accepted,
        code(&answer)
    );
    // Shipped cleanup is bounded to eight expired bodies per admitted send.
    let released = before.0.min(8);
    assert_eq!(
        retained(&path),
        (
            before.0 - released + 1,
            before.1 - released * 65_536 + after_expiry.body.len() as i64
        ),
        "automatic cleanup is not the shipped bounded batch"
    );
    assert!(fixture.a.integrity().unwrap().valid);
    let Shipped { a, app, .. } = fixture;
    drop(app);
    drop(a);
    assert_reopens_valid(&path, "tenant-a", SHIPPED_EVENTS);
}

/// Three small bodies with a three-second TTL fill the sender share of `w`.
async fn fill_expiring(w: &World, grant: &RelayGrantV1, count: usize) -> Vec<RelaySendV1> {
    let mut sent = Vec::new();
    for index in 0..count {
        let input = message(grant, 3, &format!("{MARKER} expiring {index}"));
        let (status, answer) = relay_send(&w.app, &w.token_a1, &input).await;
        assert_eq!(status, StatusCode::CREATED, "{answer}");
        sent.push(input);
    }
    assert_eq!(
        released(&w.path_a()),
        0,
        "fixture too slow: bodies expired during the fill"
    );
    sent
}
async fn until_expired() {
    tokio::time::sleep(std::time::Duration::from_millis(3200)).await;
}

#[tokio::test]
async fn expiry_releases_exactly_the_expired_bodies_within_the_cleanup_bound() {
    let w = world(with_batch(
        budgets(
            capacity(8, 524_288),
            capacity(3, 196_608),
            capacity(3, 196_608),
        ),
        2,
    ))
    .await;
    let path = w.path_a();
    let tasks = directed(&w.a1, &w.a2, "tasks");
    authorize(&w.app, &tasks).await;
    let sent = fill_expiring(&w, &tasks, 3).await;
    let mut by_expiry: Vec<_> = sent
        .iter()
        .map(|input| {
            (
                expiry_of(&path, &input.operation_id),
                input.operation_id.clone(),
            )
        })
        .collect();
    by_expiry.sort();

    // Full before expiry: the refusal names the automatic release to wait for.
    let fresh_send = message(&tasks, 60, "fresh");
    let (status, answer) = relay_send(&w.app, &w.token_a1, &fresh_send).await;
    assert_eq!(
        (status, code(&answer)),
        (
            StatusCode::TOO_MANY_REQUESTS,
            "relay_sender_capacity_exhausted"
        ),
        "{answer}"
    );
    assert_eq!(answer["error"]["retry_action"], "retry_after_release");
    assert_eq!(answer["error"]["next_release_at"], by_expiry[0].0);
    assert_eq!(released(&path), 0, "a live body was released");

    until_expired().await;
    let head = w.a.head().unwrap();
    let (status, answer) = relay_send(&w.app, &w.token_a1, &fresh_send).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "expired bodies still hold the sender share: {answer}"
    );
    assert_eq!(
        released(&path),
        2,
        "cleanup is not bounded by automatic_prune_batch"
    );
    assert_eq!(retained(&path).0, 2);

    // Two metadata-only release facts, oldest first, then the send itself.
    let facts = relay_facts(&w.a, head);
    assert_eq!(facts.len(), 3, "{facts:?}");
    for (fact, (expires_at, message_id)) in facts[..2].iter().zip(&by_expiry) {
        let record = &fact.event.data;
        let metadata = &record["resource"]["metadata"];
        assert_eq!(record["action"], "prune");
        assert!(
            record["actor"].is_null(),
            "a release was attributed to a worker"
        );
        assert_eq!(
            fact.event.correlation_id,
            automatic_release_of(&fresh_send, &w.a1)
        );
        assert_eq!(metadata["message_id"], *message_id);
        assert_eq!(metadata["expires_at"], *expires_at);
        assert_eq!(metadata["body_pruned"], true);
        assert_eq!(metadata["acknowledged"], false);
        assert!(
            *expires_at <= fact.event.timestamp,
            "a body was released before its expiry"
        );
        assert!(!body_present(&path, message_id));
    }
    assert_eq!(facts[2].event.data["action"], "send");
    assert_eq!(facts[2].event.correlation_id, None);
    assert!(
        !serde_json::to_string(&facts).unwrap().contains(MARKER),
        "an audit fact duplicated a message body"
    );
    assert_relay_facts_follow_the_time_floor(&w.a, head);

    let delivered = inbox(&w.app, &w.token_a2).await;
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0]["body"], "fresh");
    assert!(w.a.integrity().unwrap().valid);
    let World { a, b, app, .. } = w;
    drop(app);
    drop(a);
    drop(b);
    assert_reopens_valid(&path, "tenant-a", SMALL_EVENTS);
}

#[tokio::test]
async fn refused_send_commits_bounded_cleanup_and_the_retry_progresses() {
    let w = world(with_batch(
        budgets(
            capacity(12, 393_216),
            capacity(6, 131_072),
            capacity(6, 131_072),
        ),
        1,
    ))
    .await;
    let path = w.path_a();
    let tasks = directed(&w.a1, &w.a2, "tasks");
    authorize(&w.app, &tasks).await;
    let live = message(&tasks, 60, &"l".repeat(65_536));
    let small = message(&tasks, 3, "s");
    let large = message(&tasks, 3, &"x".repeat(65_535));
    for input in [&live, &small, &large] {
        let (status, answer) = relay_send(&w.app, &w.token_a1, input).await;
        assert_eq!(status, StatusCode::CREATED, "{answer}");
        // Distinct expiries: `small` is released first.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(released(&path), 0, "fixture too slow");
    until_expired().await;

    // One release per send cannot make room for 65536 bytes yet.
    let head = w.a.head().unwrap();
    let retry = message(&tasks, 60, &"r".repeat(65_536));
    let (status, answer) = relay_send(&w.app, &w.token_a1, &retry).await;
    assert_eq!(
        (status, code(&answer)),
        (
            StatusCode::TOO_MANY_REQUESTS,
            "relay_sender_capacity_exhausted"
        ),
        "{answer}"
    );
    let error = &answer["error"];
    assert_eq!(error["dimension"], "bytes");
    assert_eq!(error["retry_action"], "retry_after_release");
    assert!(
        !body_present(&path, &small.operation_id),
        "a refused send did not keep its bounded cleanup: the retry would repeat it forever"
    );
    assert_eq!(error["retained"], json!({"messages":2,"bytes":131_071}));
    assert_eq!(
        error["next_release_at"],
        expiry_of(&path, &large.operation_id)
    );
    // The release is audited although its triggering send is not in the audit.
    let facts = relay_facts(&w.a, head);
    assert_eq!(facts.len(), 1, "{facts:?}");
    assert_eq!(facts[0].event.data["action"], "prune");
    assert_eq!(
        facts[0].event.correlation_id,
        automatic_release_of(&retry, &w.a1)
    );
    assert_eq!(
        count(&path, "SELECT COUNT(*) FROM relay_messages"),
        3,
        "the refused send was stored"
    );

    // The same operation now progresses.
    let (status, answer) = relay_send(&w.app, &w.token_a1, &retry).await;
    assert_eq!(status, StatusCode::CREATED, "{answer}");
    assert_eq!(released(&path), 2);
    assert_eq!(retained(&path), (2, 131_072));
    assert_relay_facts_follow_the_time_floor(&w.a, head);
    assert!(w.a.integrity().unwrap().valid);
    let World { a, b, app, .. } = w;
    drop(app);
    drop(a);
    drop(b);
    assert_reopens_valid(&path, "tenant-a", SMALL_EVENTS);
}

#[tokio::test]
async fn largest_cleanup_batch_dates_every_fact_after_the_previous_one_and_reopens_valid() {
    let w = world(with_batch(
        budgets(
            capacity(80, 5_242_880),
            capacity(40, 2_621_440),
            capacity(40, 2_621_440),
        ),
        32,
    ))
    .await;
    let path = w.path_a();
    let tasks = directed(&w.a1, &w.a2, "tasks");
    authorize(&w.app, &tasks).await;
    fill_expiring(&w, &tasks, 33).await;
    until_expired().await;

    let head = w.a.head().unwrap();
    let trigger = message(&tasks, 60, "after the largest batch");
    let (status, answer) = relay_send(&w.app, &w.token_a1, &trigger).await;
    assert_eq!(status, StatusCode::CREATED, "{answer}");
    let facts = relay_facts(&w.a, head);
    assert_eq!(
        facts.len(),
        33,
        "one send must write 32 release facts and its own"
    );
    assert!(
        facts[..32]
            .iter()
            .all(|fact| fact.event.data["action"] == "prune"
                && fact.event.correlation_id == automatic_release_of(&trigger, &w.a1))
    );
    assert_eq!(facts[32].event.data["action"], "send");
    assert_eq!(released(&path), 32);
    assert_eq!(retained(&path).0, 2);
    // One fact, one clock: a single clock shared by these 33 facts would date
    // later facts before the reception of earlier ones.
    assert_relay_facts_follow_the_time_floor(&w.a, head);
    assert!(w.a.integrity().unwrap().valid);
    let World { a, b, app, .. } = w;
    drop(app);
    drop(a);
    drop(b);
    assert_reopens_valid(&path, "tenant-a", SMALL_EVENTS);
}

#[tokio::test]
async fn audit_exhaustion_blocks_cleanup_and_send_without_untraced_mutation() {
    const EVENTS: u64 = 256;
    let w = world_with(
        budgets(
            capacity(8, 524_288),
            capacity(3, 196_608),
            capacity(3, 196_608),
        ),
        EVENTS,
        None,
    )
    .await;
    let path = w.path_a();
    let tasks = directed(&w.a1, &w.a2, "tasks");
    authorize(&w.app, &tasks).await;
    let sent = fill_expiring(&w, &tasks, 3).await;
    until_expired().await;
    let mut exhausted = None;
    for _ in 0..EVENTS {
        if let Err(error) = w.a.append(Event::new("ordinary.capacity.fixture")) {
            exhausted = error.downcast_ref::<AuditCapacityFailureV1>().copied();
            break;
        }
    }
    assert_eq!(
        exhausted,
        Some(AuditCapacityFailureV1::OrdinaryWriteBudgetExhausted),
        "fixture did not exhaust the ordinary audit budget"
    );

    // Expired bodies exist, but their release cannot be audited: nothing moves.
    let head = w.a.head().unwrap();
    let blocked = message(&tasks, 60, "must not persist without audit");
    let (status, answer) = relay_send(&w.app, &w.token_a1, &blocked).await;
    assert_eq!(
        (status, code(&answer)),
        (
            StatusCode::TOO_MANY_REQUESTS,
            "relay_sender_capacity_exhausted"
        ),
        "{answer}"
    );
    // The capacity advice follows the configured policy. Audit gates every write.
    assert_eq!(answer["error"]["retry_action"], "retry_after_release");
    assert_eq!(
        w.a.head().unwrap(),
        head,
        "an unaudited attempt wrote a fact"
    );
    assert_eq!(
        released(&path),
        0,
        "a body was released without its audit fact"
    );
    assert_eq!(retained(&path).0, 3);

    // The runner's deliberate prune uses the control reserve and still works.
    let (status, answer) =
        runner_prune(&w.app, &Uuid::new_v4().to_string(), &sent[0].operation_id).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(released(&path), 1);

    // Room exists now, but the send itself cannot be audited either.
    let head = w.a.head().unwrap();
    let (status, answer) = relay_send(&w.app, &w.token_a1, &blocked).await;
    assert_eq!(
        (status, code(&answer)),
        (
            StatusCode::TOO_MANY_REQUESTS,
            "audit_workload_budget_exhausted"
        ),
        "{answer}"
    );
    assert_eq!(answer["error"]["retry_action"], "contact_operator");
    assert_eq!(w.a.head().unwrap(), head);
    assert_eq!(released(&path), 1);
    assert_eq!(count(&path, "SELECT COUNT(*) FROM relay_messages"), 3);
    assert!(w.a.integrity().unwrap().valid);
    let World { a, b, app, .. } = w;
    drop(app);
    drop(a);
    drop(b);
    assert_reopens_valid(&path, "tenant-a", EVENTS);
}

#[tokio::test]
async fn send_refused_by_its_audit_budget_keeps_only_audited_cleanup() {
    const EVENTS: u64 = 1000;
    // A1 may write three workload audit events in total.
    let w = world_with(
        budgets(
            capacity(8, 524_288),
            capacity(3, 196_608),
            capacity(3, 196_608),
        ),
        EVENTS,
        Some(WorkloadAuditPolicyV1 {
            version: 1,
            tenant: AuditCapacityV1 {
                events: 12,
                bytes: 96 * 1024,
            },
            principal: AuditCapacityV1 {
                events: 3,
                bytes: 24 * 1024,
            },
        }),
    )
    .await;
    let path = w.path_a();
    let tasks = directed(&w.a1, &w.a2, "tasks");
    authorize(&w.app, &tasks).await;
    let sent = fill_expiring(&w, &tasks, 3).await;
    until_expired().await;

    // Releases are written, then the send's own audit fact is refused.
    let head = w.a.head().unwrap();
    let refused = message(&tasks, 60, "over the worker audit budget");
    let (status, answer) = relay_send(&w.app, &w.token_a1, &refused).await;
    assert_eq!(
        (status, code(&answer)),
        (
            StatusCode::TOO_MANY_REQUESTS,
            "audit_workload_budget_exhausted"
        ),
        "{answer}"
    );
    // Every mutation that remains has its own audit fact; the send left none.
    let facts = relay_facts(&w.a, head);
    assert_eq!(w.a.head().unwrap(), head + 3);
    assert_eq!(facts.len(), 3, "{facts:?}");
    let mut audited: Vec<_> = facts
        .iter()
        .map(|fact| {
            assert_eq!(fact.event.data["action"], "prune");
            assert_eq!(
                fact.event.correlation_id,
                automatic_release_of(&refused, &w.a1)
            );
            fact.event.data["resource"]["metadata"]["message_id"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    audited.sort();
    let mut expected: Vec<_> = sent
        .iter()
        .map(|input| input.operation_id.clone())
        .collect();
    expected.sort();
    assert_eq!(audited, expected);
    assert_eq!(released(&path), 3);
    assert_eq!(
        count(&path, "SELECT COUNT(*) FROM relay_messages"),
        3,
        "a send without its audit fact was stored"
    );
    assert_relay_facts_follow_the_time_floor(&w.a, head);
    assert!(w.a.integrity().unwrap().valid);
    let World { a, b, app, .. } = w;
    drop(app);
    drop(a);
    drop(b);
    assert_reopens_valid(&path, "tenant-a", EVENTS);
}

#[tokio::test]
async fn disabled_automatic_prune_keeps_bodies_until_the_runner_prunes() {
    let w = world(with_batch(
        budgets(
            capacity(8, 524_288),
            capacity(3, 196_608),
            capacity(3, 196_608),
        ),
        0,
    ))
    .await;
    let path = w.path_a();
    let tasks = directed(&w.a1, &w.a2, "tasks");
    authorize(&w.app, &tasks).await;
    let sent = fill_expiring(&w, &tasks, 3).await;
    until_expired().await;

    // Characterization of the deliberate path: no automatic release at all.
    let fourth = message(&tasks, 60, "fourth");
    let (status, answer) = relay_send(&w.app, &w.token_a1, &fourth).await;
    assert_eq!(
        (status, code(&answer)),
        (
            StatusCode::TOO_MANY_REQUESTS,
            "relay_sender_capacity_exhausted"
        ),
        "{answer}"
    );
    assert_eq!(answer["error"]["retry_action"], "contact_operator");
    assert_eq!(released(&path), 0);

    let prune = Uuid::new_v4().to_string();
    let (status, answer) = runner_prune(&w.app, &prune, &sent[0].operation_id).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["replayed"], false);
    let (status, answer) = relay_send(&w.app, &w.token_a1, &fourth).await;
    assert_eq!(status, StatusCode::CREATED, "{answer}");

    // Releasing is idempotent: neither a replay nor a second prune of the same
    // message credits capacity twice.
    let (status, answer) = runner_prune(&w.app, &prune, &sent[0].operation_id).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["replayed"], true);
    let (status, answer) =
        runner_prune(&w.app, &Uuid::new_v4().to_string(), &sent[0].operation_id).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(released(&path), 1);
    assert_eq!(retained(&path).0, 3);
    let (status, answer) = relay_send(&w.app, &w.token_a1, &message(&tasks, 60, "fifth")).await;
    assert_eq!(
        (status, code(&answer)),
        (
            StatusCode::TOO_MANY_REQUESTS,
            "relay_sender_capacity_exhausted"
        ),
        "one released body admitted two sends: {answer}"
    );
    assert!(w.a.integrity().unwrap().valid);
    let World { a, b, app, .. } = w;
    drop(app);
    drop(a);
    drop(b);
    assert_reopens_valid(&path, "tenant-a", SMALL_EVENTS);
}

async fn retention_status(app: &Router, token: &str, query: &str) -> (StatusCode, Value) {
    for _ in 0..80 {
        let (status, answer) = call(
            app,
            "GET",
            &format!("/api/v1/relay/retention{query}"),
            token,
            Value::Null,
        )
        .await;
        if status == StatusCode::TOO_MANY_REQUESTS && code(&answer) == "capacity_exhausted" {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            continue;
        }
        return (status, answer);
    }
    panic!("the shared I/O admission gate never admitted the fixture status read");
}
fn without_observation(mut status: Value) -> Value {
    status.as_object_mut().unwrap().remove("observed_at");
    status
}

#[tokio::test]
async fn retention_status_reports_exact_scoped_usage_and_survives_reopen() {
    // Automatic release is disabled so the retained picture stays deterministic.
    let policy = with_batch(
        budgets(
            capacity(8, 524_288),
            capacity(3, 196_608),
            capacity(2, 131_072),
        ),
        0,
    );
    let w = world(policy.clone()).await;
    let path = w.path_a();
    let path_b = w.directory.path().join("tenant-b.sqlite");
    let tasks = directed(&w.a1, &w.a2, "tasks");
    let results = directed(&w.a1, &w.a2, "results");
    let reverse = directed(&w.a2, &w.a1, "tasks");
    for grant in [&tasks, &results, &reverse] {
        authorize(&w.app, grant).await;
    }
    let expiring = message(&tasks, 3, &format!("{MARKER}-a"));
    let acknowledged = message(&tasks, 60, &format!("{MARKER}-acknowledged"));
    let other = message(&results, 60, &format!("{MARKER}-result"));
    let pong = message(&reverse, 60, "pong");
    for (token, input) in [
        (&w.token_a1, &expiring),
        (&w.token_a1, &acknowledged),
        (&w.token_a1, &other),
        (&w.token_a2, &pong),
    ] {
        let (status, answer) = relay_send(&w.app, token, input).await;
        assert_eq!(status, StatusCode::CREATED, "{answer}");
    }
    let (status, answer) = call(
        &w.app,
        "POST",
        &format!("/api/v1/relay/messages/{}/ack", acknowledged.operation_id),
        &w.token_a2,
        json!(RelayAckV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            message_id: acknowledged.operation_id.clone(),
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    until_expired().await;

    let bytes = |input: &RelaySendV1| input.body.len();
    let (a1, a2) = (w.a1.worker_id.clone(), w.a2.worker_id.clone());
    let head = w.a.head().unwrap();
    let (status, status_a) = retention_status(&w.app, &runner_a(), "").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the operator cannot read relay retention: {status_a}"
    );
    assert_eq!(status_a["version"], 1);
    assert_eq!(status_a["policy"], json!(policy));
    assert_eq!(
        status_a["policy_fingerprint"],
        w.a.relay_retention_policy_fingerprint().unwrap()
    );
    let expiring_at = expiry_of(&path, &expiring.operation_id);
    assert_eq!(
        status_a["tenant"],
        json!({
            "limit":{"messages":8,"bytes":524_288},
            "retained":{"messages":4,"bytes":bytes(&expiring)+bytes(&acknowledged)+bytes(&other)+bytes(&pong)},
            "expired":{"messages":1,"bytes":bytes(&expiring)},
            "acknowledged":{"messages":1,"bytes":bytes(&acknowledged)},
            "next_expiry_at":expiring_at,
        })
    );
    // Senders and grants are listed largest first.
    assert_eq!(status_a["senders_total"], 2);
    assert_eq!(
        status_a["senders"][0],
        json!({
            "worker_id":a1,
            "limit":{"messages":3,"bytes":196_608},
            "retained":{"messages":3,"bytes":bytes(&expiring)+bytes(&acknowledged)+bytes(&other)},
            "expired":{"messages":1,"bytes":bytes(&expiring)},
            "next_expiry_at":expiring_at,
        })
    );
    assert_eq!(status_a["senders"][1]["worker_id"], a2);
    assert_eq!(
        status_a["senders"][1]["retained"],
        json!({"messages":1,"bytes":bytes(&pong)})
    );
    assert_eq!(
        status_a["senders"][1]["expired"],
        json!({"messages":0,"bytes":0})
    );
    assert_eq!(status_a["grants_total"], 3);
    assert_eq!(
        status_a["grants"][0],
        json!({
            "grant_id":tasks.grant_id,
            "source_worker_id":a1,
            "destination_worker_id":a2,
            "channel":"tasks",
            "limit":{"messages":2,"bytes":131_072},
            "retained":{"messages":2,"bytes":bytes(&expiring)+bytes(&acknowledged)},
            "expired":{"messages":1,"bytes":bytes(&expiring)},
            "next_expiry_at":expiring_at,
        })
    );
    assert_eq!(status_a["grants"][1]["grant_id"], results.grant_id);
    assert_eq!(status_a["grants"][1]["channel"], "results");
    assert_eq!(status_a["grants"][2]["grant_id"], reverse.grant_id);
    assert_eq!(status_a["grants"][2]["source_worker_id"], a2);
    assert_eq!(status_a["prunable_total"], 1);
    assert_eq!(
        status_a["prunable"],
        json!([{
            "message_id":expiring.operation_id,
            "grant_id":tasks.grant_id,
            "source_worker_id":a1,
            "destination_worker_id":a2,
            "body_bytes":bytes(&expiring),
            "expires_at":expiring_at,
            "acknowledged":false,
        }])
    );
    assert!(
        !status_a.to_string().contains(MARKER),
        "the diagnostic returned message content"
    );

    // The viewer of the tenant reads the same picture; a worker proof does not.
    let (status, viewed) = retention_status(&w.app, &"v".repeat(48), "").await;
    assert_eq!(status, StatusCode::OK, "{viewed}");
    assert_eq!(
        without_observation(viewed),
        without_observation(status_a.clone())
    );
    let (status, _) = retention_status(&w.app, &w.token_a1, "").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        w.a.head().unwrap(),
        head,
        "a diagnostic read wrote an audit fact"
    );

    // Tenant B sees its own empty relay and nothing of tenant A.
    let (status, status_b) = retention_status(&w.app, &"b".repeat(48), "").await;
    assert_eq!(status, StatusCode::OK, "{status_b}");
    assert_eq!(
        status_b["tenant"]["retained"],
        json!({"messages":0,"bytes":0})
    );
    assert!(status_b["tenant"]["next_expiry_at"].is_null());
    for list in ["senders", "grants", "prunable"] {
        assert_eq!(status_b[list], json!([]));
        assert_eq!(status_b[format!("{list}_total")], 0);
    }
    let encoded_b = status_b.to_string();
    for private in [&a1, &a2, &tasks.grant_id, &expiring.operation_id] {
        assert!(
            !encoded_b.contains(private.as_str()),
            "tenant B read an identifier of tenant A"
        );
    }

    // Truncation is visible; the closed query refuses anything else.
    let (status, truncated) = retention_status(&w.app, &runner_a(), "?limit=1").await;
    assert_eq!(status, StatusCode::OK, "{truncated}");
    assert_eq!(truncated["senders"].as_array().unwrap().len(), 1);
    assert_eq!(truncated["senders"][0]["worker_id"], a1);
    assert_eq!(truncated["senders_total"], 2);
    assert_eq!(truncated["grants"].as_array().unwrap().len(), 1);
    assert_eq!(truncated["grants_total"], 3);
    assert_eq!(truncated["prunable_total"], 1);
    for refused in [
        "?limit=0",
        "?limit=101",
        "?limit=1&tenant=tenant-b",
        "?after=1",
    ] {
        let (status, answer) = retention_status(&w.app, &runner_a(), refused).await;
        assert_eq!(
            (status, code(&answer)),
            (StatusCode::BAD_REQUEST, "invalid_relay_request"),
            "{refused}"
        );
    }

    // After a restart supervision is inactive: the diagnostic and the runner's
    // deliberate prune remain usable. A tightened policy reports occupancy
    // above its new share without invalidating anything.
    let World { a, b, app, .. } = w;
    drop(app);
    drop(a);
    drop(b);
    let stricter = with_batch(
        budgets(
            capacity(8, 524_288),
            capacity(2, 131_072),
            capacity(1, 65_536),
        ),
        0,
    );
    let reopen = |path: &Path, tenant: &str| {
        Store::open_existing_for_tenant(path, tenant, Redactor::new(vec![]), SMALL_EVENTS).unwrap()
    };
    let a = Arc::new(
        reopen(&path, "tenant-a")
            .with_relay_retention_policy(stricter.clone())
            .unwrap(),
    );
    assert!(!a.supervision_state().unwrap().active);
    let app = router(a.clone(), Arc::new(reopen(&path_b, "tenant-b")));
    let (status, reopened) = retention_status(&app, &runner_a(), "").await;
    assert_eq!(status, StatusCode::OK, "{reopened}");
    assert_eq!(reopened["policy"], json!(stricter));
    assert_ne!(
        reopened["policy_fingerprint"],
        status_a["policy_fingerprint"]
    );
    for unchanged in ["retained", "expired", "acknowledged", "next_expiry_at"] {
        assert_eq!(reopened["tenant"][unchanged], status_a["tenant"][unchanged]);
    }
    assert_eq!(reopened["prunable"], status_a["prunable"]);
    assert_eq!(
        reopened["senders"][0]["limit"],
        json!({"messages":2,"bytes":131_072})
    );
    assert_eq!(reopened["senders"][0]["retained"]["messages"], 3);
    assert!(a.healthy());
    assert!(a.integrity().unwrap().valid);

    let listed = reopened["prunable"][0]["message_id"].as_str().unwrap();
    let (status, answer) = runner_prune(&app, &Uuid::new_v4().to_string(), listed).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let (status, after) = retention_status(&app, &runner_a(), "").await;
    assert_eq!(status, StatusCode::OK, "{after}");
    assert_eq!(
        after["tenant"]["retained"],
        json!({"messages":3,"bytes":bytes(&acknowledged)+bytes(&other)+bytes(&pong)}),
        "the deliberate prune did not release exactly the listed body"
    );
    assert_eq!(after["tenant"]["expired"], json!({"messages":0,"bytes":0}));
    assert_eq!(after["prunable"], json!([]));
    assert_eq!(
        after["senders"][0]["retained"],
        json!({"messages":2,"bytes":bytes(&acknowledged)+bytes(&other)})
    );
    drop(app);
    drop(a);
    assert_reopens_valid(&path, "tenant-a", SMALL_EVENTS);
}

async fn acknowledge(app: &Router, token: &str, input: &RelayAckV1) -> (StatusCode, Value) {
    call(
        app,
        "POST",
        &format!("/api/v1/relay/messages/{}/ack", input.message_id),
        token,
        json!(input),
    )
    .await
}

#[tokio::test]
async fn acknowledgement_releases_the_body_only_when_the_policy_opts_in() {
    let full_at_two = |release_on_ack| RelayRetentionPolicyV1 {
        release_on_ack,
        ..with_batch(
            budgets(
                capacity(8, 524_288),
                capacity(2, 131_072),
                capacity(2, 131_072),
            ),
            0,
        )
    };

    // Shipped behavior: an acknowledgement records receipt and keeps the body.
    let w = world(full_at_two(false)).await;
    let tasks = directed(&w.a1, &w.a2, "tasks");
    authorize(&w.app, &tasks).await;
    let first = message(&tasks, 60, &format!("{MARKER} first"));
    for input in [&first, &message(&tasks, 60, "second")] {
        let (status, answer) = relay_send(&w.app, &w.token_a1, input).await;
        assert_eq!(status, StatusCode::CREATED, "{answer}");
    }
    let ack = RelayAckV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        message_id: first.operation_id.clone(),
    };
    let (status, answer) = acknowledge(&w.app, &w.token_a2, &ack).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert!(
        body_present(&w.path_a(), &first.operation_id),
        "an acknowledgement destroyed retained content without the operator opting in"
    );
    let (status, answer) = relay_send(&w.app, &w.token_a1, &message(&tasks, 60, "third")).await;
    assert_eq!(
        (status, code(&answer)),
        (
            StatusCode::TOO_MANY_REQUESTS,
            "relay_sender_capacity_exhausted"
        ),
        "{answer}"
    );
    assert_eq!(answer["error"]["retry_action"], "contact_operator");

    // Explicit opt-in: the acknowledgement fact itself retires the body.
    let w = world(full_at_two(true)).await;
    let path = w.path_a();
    let tasks = directed(&w.a1, &w.a2, "tasks");
    authorize(&w.app, &tasks).await;
    let first = message(&tasks, 60, &format!("{MARKER} first"));
    let second = message(&tasks, 60, "second");
    for input in [&first, &second] {
        let (status, answer) = relay_send(&w.app, &w.token_a1, input).await;
        assert_eq!(status, StatusCode::CREATED, "{answer}");
    }
    let third = message(&tasks, 60, "third");
    let (status, answer) = relay_send(&w.app, &w.token_a1, &third).await;
    assert_eq!(
        (status, code(&answer)),
        (
            StatusCode::TOO_MANY_REQUESTS,
            "relay_sender_capacity_exhausted"
        ),
        "{answer}"
    );
    assert_eq!(answer["error"]["retry_action"], "retry_after_release");

    let head = w.a.head().unwrap();
    let ack = RelayAckV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        message_id: first.operation_id.clone(),
    };
    let (status, receipt) = acknowledge(&w.app, &w.token_a2, &ack).await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    assert!(
        !body_present(&path, &first.operation_id),
        "release_on_ack is accepted by the policy but the acknowledged body is still retained"
    );
    // One fact only: the acknowledgement, attributed to the recipient. No prune.
    let facts = relay_facts(&w.a, head);
    assert_eq!(facts.len(), 1, "{facts:?}");
    let record = &facts[0].event.data;
    assert_eq!(record["action"], "ack");
    assert_eq!(
        record["actor"]["endpoint"]["worker_id"], w.a2.worker_id,
        "the release is not attributed to the acknowledging recipient"
    );
    assert_eq!(record["resource"]["metadata"]["acknowledged"], true);
    assert_eq!(record["resource"]["metadata"]["body_pruned"], true);
    assert_eq!(facts[0].event.correlation_id, None);
    assert!(!serde_json::to_string(&facts).unwrap().contains(MARKER));

    // The released share admits the refused operation; replays stay exact.
    let (status, answer) = relay_send(&w.app, &w.token_a1, &third).await;
    assert_eq!(status, StatusCode::CREATED, "{answer}");
    let (status, replayed) = acknowledge(&w.app, &w.token_a2, &ack).await;
    assert_eq!(status, StatusCode::OK, "{replayed}");
    assert_eq!(replayed["replayed"], true);
    assert_eq!(replayed["audit_seq"], receipt["audit_seq"]);
    assert_eq!(retained(&path).0, 2);

    // Nothing else is loosened: an unacknowledged live body cannot be pruned.
    let (status, answer) =
        runner_prune(&w.app, &Uuid::new_v4().to_string(), &second.operation_id).await;
    assert_eq!(
        (status, code(&answer)),
        (StatusCode::FORBIDDEN, "relay_forbidden")
    );
    let delivered = inbox(&w.app, &w.token_a2).await;
    let bodies: Vec<_> = delivered.iter().map(|item| &item["body"]).collect();
    assert_eq!(bodies, ["second", "third"]);
    assert!(w.a.integrity().unwrap().valid);
    let World { a, b, app, .. } = w;
    drop(app);
    drop(a);
    drop(b);
    assert_reopens_valid(&path, "tenant-a", SMALL_EVENTS);
}

/// Acknowledgement is the only automatic release: sends release nothing.
fn released_by_acknowledgement_only(
    tenant: RelayCapacityV1,
    sender: RelayCapacityV1,
    grant: RelayCapacityV1,
) -> RelayRetentionPolicyV1 {
    RelayRetentionPolicyV1 {
        release_on_ack: true,
        ..with_batch(budgets(tenant, sender, grant), 0)
    }
}
fn acknowledgement_of(input: &RelaySendV1) -> RelayAckV1 {
    RelayAckV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        message_id: input.operation_id.clone(),
    }
}

/// One refused scope under `released_by_acknowledgement_only`. A refusal may
/// announce a retry only while a body of that own scope can still be acknowledged.
/// The tenant ceiling always asks for the operator without revealing neighbor state.
struct AcknowledgementOnlyScope<'a> {
    w: &'a World,
    code: &'a str,
    scope: RelayRefusalScopeV1,
    /// The two accepted bodies, three-second TTL, that fill the scope: each
    /// with the token of its recipient.
    held: [(&'a str, &'a RelaySendV1); 2],
    /// Send refused by the scope: its sender's token, the operation and the
    /// token of its recipient.
    blocked: (&'a str, &'a RelaySendV1, &'a str),
    /// Another send that the same scope refuses once `blocked` is retained.
    next: (&'a str, &'a RelaySendV1),
}
impl AcknowledgementOnlyScope<'_> {
    async fn refused(&self, token: &str, input: &RelaySendV1) -> RelayRetryActionV1 {
        let (status, answer) = relay_send(&self.w.app, token, input).await;
        assert_eq!(
            (status, code(&answer)),
            (StatusCode::TOO_MANY_REQUESTS, self.code),
            "{answer}"
        );
        let (scope, action) = published(&answer);
        assert_eq!(scope, self.scope, "{answer}");
        if scope == RelayRefusalScopeV1::Tenant {
            assert_only_scope_and_action(
                &answer,
                "a tenant-scope refusal disclosed occupancy or deadlines of other senders",
            );
        }
        action
    }

    async fn check(&self) {
        let (app, path) = (&self.w.app, self.w.path_a());
        let (sender, blocked, recipient) = self.blocked;
        let live_action = if self.scope == RelayRefusalScopeV1::Tenant {
            RelayRetryActionV1::ContactOperator
        } else {
            RelayRetryActionV1::RetryAfterRelease
        };

        // Live, unacknowledged bodies: their recipients can still release them.
        let action = self.refused(sender, blocked).await;
        for (_, body) in &self.held {
            assert!(
                chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
                    < expiry_of(&path, &body.operation_id),
                "fixture too slow: bodies expired during the fill"
            );
        }
        assert_eq!(
            action, live_action,
            "unexpected advice with live bodies of the {:?} scope",
            self.scope
        );

        // Expired, unacknowledged bodies: nothing but the runner frees the scope.
        until_expired().await;
        let head = self.w.a.head().unwrap();
        assert_eq!(
            self.refused(sender, blocked).await,
            RelayRetryActionV1::ContactOperator,
            "every body of the {:?} scope has expired unacknowledged while sends release nothing: no retry can pass, yet one is announced",
            self.scope
        );
        for (holder, body) in &self.held {
            let (status, answer) = acknowledge(app, holder, &acknowledgement_of(body)).await;
            assert_eq!(
                (status, code(&answer)),
                (StatusCode::FORBIDDEN, "relay_expired"),
                "an expired body was acknowledged: {answer}"
            );
        }
        assert_eq!(
            self.refused(sender, blocked).await,
            RelayRetryActionV1::ContactOperator
        );
        assert_eq!(released(&path), 0);
        assert!(
            relay_facts(&self.w.a, head).is_empty(),
            "a refused send or a refused acknowledgement wrote a relay fact"
        );

        // The operator path named by the refusal does free the scope.
        let (status, answer) = runner_prune(
            app,
            &Uuid::new_v4().to_string(),
            &self.held[0].1.operation_id,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{answer}");
        let (status, answer) = relay_send(app, sender, blocked).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "the runner prune did not free the refused scope: {answer}"
        );

        // The scope is full again: one expired body and the live `blocked`.
        // Its recipient can acknowledge it. Own-scope advice announces a retry;
        // tenant advice stays pessimistic. Either way, the ACK frees the capacity.
        let (next_sender, next) = self.next;
        assert_eq!(
            self.refused(next_sender, next).await,
            live_action,
            "unexpected advice with mixed live and expired bodies of the {:?} scope",
            self.scope
        );
        let (status, answer) = acknowledge(app, recipient, &acknowledgement_of(blocked)).await;
        assert_eq!(status, StatusCode::OK, "{answer}");
        assert!(!body_present(&path, &blocked.operation_id));
        let (status, answer) = relay_send(app, next_sender, next).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "the acknowledgement did not free the scope: {answer}"
        );
        assert!(self.w.a.integrity().unwrap().valid);
    }
}

async fn send_held(w: &World, token: &str, grant: &RelayGrantV1, label: &str) -> RelaySendV1 {
    let input = message(grant, 3, label);
    let (status, answer) = relay_send(&w.app, token, &input).await;
    assert_eq!(status, StatusCode::CREATED, "{answer}");
    input
}
fn assert_world_reopens_valid(w: World) {
    let path = w.path_a();
    let World { a, b, app, .. } = w;
    drop(app);
    drop(a);
    drop(b);
    assert_reopens_valid(&path, "tenant-a", SMALL_EVENTS);
}

#[tokio::test]
async fn sender_share_of_expired_bodies_asks_for_the_operator_when_only_acknowledgement_releases() {
    let w = world(released_by_acknowledgement_only(
        capacity(8, 524_288),
        capacity(2, 131_072),
        capacity(2, 131_072),
    ))
    .await;
    let tasks = directed(&w.a1, &w.a2, "tasks");
    let results = directed(&w.a1, &w.a2, "results");
    for grant in [&tasks, &results] {
        authorize(&w.app, grant).await;
    }
    let first = send_held(&w, &w.token_a1, &tasks, "first").await;
    let second = send_held(&w, &w.token_a1, &tasks, "second").await;
    // The other grant is empty: only the sender share refuses these sends.
    let blocked = message(&results, 60, "blocked");
    let next = message(&results, 60, "next");
    AcknowledgementOnlyScope {
        w: &w,
        code: "relay_sender_capacity_exhausted",
        scope: RelayRefusalScopeV1::Sender,
        held: [(&w.token_a2, &first), (&w.token_a2, &second)],
        blocked: (&w.token_a1, &blocked, &w.token_a2),
        next: (&w.token_a1, &next),
    }
    .check()
    .await;
    assert_world_reopens_valid(w);
}

#[tokio::test]
async fn grant_share_of_expired_bodies_asks_for_the_operator_when_only_acknowledgement_releases() {
    let w = world(released_by_acknowledgement_only(
        capacity(8, 524_288),
        capacity(3, 196_608),
        capacity(2, 131_072),
    ))
    .await;
    let tasks = directed(&w.a1, &w.a2, "tasks");
    authorize(&w.app, &tasks).await;
    let first = send_held(&w, &w.token_a1, &tasks, "first").await;
    let second = send_held(&w, &w.token_a1, &tasks, "second").await;
    // The sender keeps room for a third body: only the grant share refuses.
    let blocked = message(&tasks, 60, "blocked");
    let next = message(&tasks, 60, "next");
    AcknowledgementOnlyScope {
        w: &w,
        code: "relay_grant_capacity_exhausted",
        scope: RelayRefusalScopeV1::Grant,
        held: [(&w.token_a2, &first), (&w.token_a2, &second)],
        blocked: (&w.token_a1, &blocked, &w.token_a2),
        next: (&w.token_a1, &next),
    }
    .check()
    .await;
    assert_world_reopens_valid(w);
}

#[tokio::test]
async fn tenant_ceiling_advice_stays_pessimistic_with_live_expired_and_mixed_neighbor_bodies() {
    let w = world(released_by_acknowledgement_only(
        capacity(2, 131_072),
        capacity(1, 65_536),
        capacity(1, 65_536),
    ))
    .await;
    let a3 = running_now(&w.a);
    let token_a3 = "4".repeat(48);
    bind(&w.a, &a3, &token_a3);
    let from_a1 = directed(&w.a1, &w.a2, "tasks");
    let from_a2 = directed(&w.a2, &w.a1, "tasks");
    let from_a3 = directed(&a3, &w.a1, "tasks");
    for grant in [&from_a1, &from_a2, &from_a3] {
        authorize(&w.app, grant).await;
    }
    let of_a1 = send_held(&w, &w.token_a1, &from_a1, MARKER).await;
    let of_a2 = send_held(&w, &w.token_a2, &from_a2, MARKER).await;
    // A3 and, once its body is pruned, A1 retain nothing: only the shared
    // ceiling refuses them, and its refusal describes no other sender.
    let blocked = message(&from_a3, 60, "blocked");
    let next = message(&from_a1, 60, "next");
    AcknowledgementOnlyScope {
        w: &w,
        code: "relay_capacity_exhausted",
        scope: RelayRefusalScopeV1::Tenant,
        held: [(&w.token_a2, &of_a1), (&w.token_a1, &of_a2)],
        blocked: (&token_a3, &blocked, &w.token_a1),
        next: (&w.token_a1, &next),
    }
    .check()
    .await;
    assert_world_reopens_valid(w);
}

#[tokio::test]
async fn shipped_shares_still_admit_twenty_full_messages_on_one_grant() {
    // Existing callers send 20 × 64 KiB (1.31 MiB) from one worker over one
    // grant. No retry here: any refusal, of whatever layer, is reported whole.
    let fixture = shipped().await;
    let body = "\n".repeat(MAX_MESSAGE_BYTES);
    for index in 0..20 {
        let (status, answer) = call(
            &fixture.app,
            "POST",
            "/api/v1/relay/messages",
            &fixture.token1,
            json!(message(&fixture.outbound[0], 60, &body)),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "send {index} of 20 was refused under the shipped shares: {answer}"
        );
    }
    assert_eq!(
        retained(&fixture.directory.path().join("tenant-a.sqlite")),
        (20, 1_310_720)
    );
    assert!(fixture.a.integrity().unwrap().valid);
}

#[tokio::test]
async fn rate_limited_send_names_its_grant_and_the_window_to_wait_for() {
    let w = world(RelayRetentionPolicyV1::default()).await;
    let tasks = directed(&w.a1, &w.a2, "tasks");
    let results = directed(&w.a1, &w.a2, "results");
    for grant in [&tasks, &results] {
        authorize(&w.app, grant).await;
    }
    let started = std::time::Instant::now();
    for index in 0..MAX_MESSAGES_PER_MINUTE {
        let (status, answer) = relay_send(&w.app, &w.token_a1, &message(&tasks, 60, "r")).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "fixture send {index} failed: {answer}"
        );
    }
    let head = w.a.head().unwrap();
    let over = message(&tasks, 60, "over the rate");
    let (status, answer) = relay_send(&w.app, &w.token_a1, &over).await;
    assert!(
        started.elapsed() < std::time::Duration::from_secs(50),
        "fixture too slow: the 60-second rate window moved during the fill"
    );
    assert_eq!(
        (status, code(&answer)),
        (StatusCode::TOO_MANY_REQUESTS, "relay_rate_limited"),
        "{answer}"
    );
    assert_eq!(
        published(&answer),
        (
            RelayRefusalScopeV1::Grant,
            RelayRetryActionV1::RetryAfterWindow
        )
    );
    assert_only_scope_and_action(&answer, "a rate refusal carried unexpected fields");
    assert_eq!(
        w.a.head().unwrap(),
        head,
        "a rate-limited send wrote an audit fact"
    );
    assert_eq!(retained(&w.path_a()), (60, 60));

    // The rate is per grant: another grant of the same sender is not limited.
    let (status, answer) =
        relay_send(&w.app, &w.token_a1, &message(&results, 60, "other grant")).await;
    assert_eq!(status, StatusCode::CREATED, "{answer}");
    assert!(w.a.integrity().unwrap().valid);
}

/// The gateway router behind a real HTTP listener on loopback, for the Rust
/// worker client. Worker runtime facts stay synthetic.
struct Listener {
    origin: String,
    task: tokio::task::JoinHandle<()>,
}
impl Listener {
    async fn start(app: Router) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { origin, task }
    }
    fn worker(&self, token: &str, worker: &WorkerV1) -> client::WorkerRelayClient {
        client::WorkerRelayClient::new(&self.origin, token, endpoint(worker)).unwrap()
    }
}
impl Drop for Listener {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// A fixture send that must be accepted. As in `relay_send`, an untyped 429
/// is the shared I/O admission gate: the same operation is replayed.
async fn client_accepts(client: &client::WorkerRelayClient, input: &RelaySendV1) {
    for _ in 0..80 {
        match client.send_with_refusal(input).await {
            Ok(_) => return,
            Err(failure)
                if failure.error == client::RelayClientError::Capacity
                    && failure.refusal.is_none() =>
            {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
            Err(failure) => panic!("fixture send failed: {failure:?}"),
        }
    }
    panic!("the shared I/O admission gate never admitted the fixture send");
}

#[tokio::test]
async fn worker_client_tells_a_retry_from_an_operator_call_and_learns_nothing_about_neighbors() {
    use client::RelayClientError::Capacity;
    for (automatic_prune_batch, action) in [
        (8, RelayRetryActionV1::RetryAfterRelease),
        (0, RelayRetryActionV1::ContactOperator),
    ] {
        let w = world(with_batch(
            budgets(
                capacity(2, 131_072),
                capacity(1, 65_536),
                capacity(1, 65_536),
            ),
            automatic_prune_batch,
        ))
        .await;
        let a3 = running_now(&w.a);
        let token_a3 = "4".repeat(48);
        bind(&w.a, &a3, &token_a3);
        let from_a1 = directed(&w.a1, &w.a2, "tasks");
        let from_a2 = directed(&w.a2, &w.a1, "tasks");
        let from_a3 = directed(&a3, &w.a1, "tasks");
        for grant in [&from_a1, &from_a2, &from_a3] {
            authorize(&w.app, grant).await;
        }
        let server = Listener::start(w.app.clone()).await;
        let client_a1 = server.worker(&w.token_a1, &w.a1);
        let client_a2 = server.worker(&w.token_a2, &w.a2);
        let client_a3 = server.worker(&token_a3, &a3);

        // A1 fills its own share: the refusal carries A1's own figures.
        let first = message(&from_a1, 60, &format!("{MARKER} first"));
        client_accepts(&client_a1, &first).await;
        let second = message(&from_a1, 60, "second");
        let failure = client_a1.send_with_refusal(&second).await.unwrap_err();
        assert_eq!(failure.error, Capacity);
        let refusal = failure
            .refusal
            .clone()
            .expect("the worker client reduced a typed relay refusal to a bare capacity error");
        assert_eq!(
            (refusal.scope, refusal.retry_action),
            (RelayRefusalScopeV1::Sender, action),
            "automatic_prune_batch {automatic_prune_batch}"
        );
        assert_eq!(
            refusal.own,
            Some(RelayOwnShareV1 {
                dimension: RelayCapacityDimensionV1::Messages,
                limit: capacity(1, 65_536),
                retained: capacity(1, first.body.len() as u64),
                requested_bytes: second.body.len() as u64,
                next_release_at: Some(expiry_of(&w.path_a(), &first.operation_id)),
            })
        );
        assert_eq!(
            failure.to_string().contains("operator"),
            action == RelayRetryActionV1::ContactOperator,
            "{failure}"
        );
        // The historical method reports the same refusal as before.
        assert_eq!(client_a1.send(&second).await.unwrap_err(), Capacity);

        // A2 fills the tenant. A3 retains nothing and meets the shared
        // ceiling: it learns what to do and nothing about A1 or A2.
        client_accepts(&client_a2, &message(&from_a2, 60, MARKER)).await;
        let blocked = message(&from_a3, 60, "blocked");
        let failure = client_a3.send_with_refusal(&blocked).await.unwrap_err();
        assert_eq!(failure.error, Capacity);
        assert_eq!(
            failure.refusal,
            Some(RelaySendRefusalV1 {
                scope: RelayRefusalScopeV1::Tenant,
                retry_action: action,
                own: None,
            }),
            "automatic_prune_batch {automatic_prune_batch}"
        );
        let surfaced = format!("{failure:?} {failure}");
        for private in [
            w.a1.worker_id.as_str(),
            w.a2.worker_id.as_str(),
            first.operation_id.as_str(),
            MARKER,
            w.token_a1.as_str(),
            w.token_a2.as_str(),
            token_a3.as_str(),
        ] {
            assert!(
                !surfaced.contains(private),
                "a refused send surfaced data of another sender, a body or a credential: {surfaced}"
            );
        }
        assert_eq!(
            failure.to_string().contains("operator"),
            action == RelayRetryActionV1::ContactOperator,
            "{failure}"
        );
        assert_eq!(retained(&w.path_a()).0, 2, "a refused send was stored");
        assert!(w.a.integrity().unwrap().valid);
    }

    // A tighter directed-grant share refuses before the sender share. Read the
    // real HTTP refusal through the same worker client, not a synthetic body.
    let w = world(budgets(
        capacity(8, 524_288),
        capacity(3, 196_608),
        capacity(2, 131_072),
    ))
    .await;
    let tasks = directed(&w.a1, &w.a2, "tasks");
    authorize(&w.app, &tasks).await;
    let server = Listener::start(w.app.clone()).await;
    let client_a1 = server.worker(&w.token_a1, &w.a1);
    let first = message(&tasks, 60, "first");
    let second = message(&tasks, 60, "second");
    for input in [&first, &second] {
        client_accepts(&client_a1, input).await;
    }
    let head = w.a.head().unwrap();
    let blocked = message(&tasks, 60, "blocked");
    let failure = client_a1.send_with_refusal(&blocked).await.unwrap_err();
    assert_eq!(failure.error, Capacity);
    assert_eq!(
        failure.refusal,
        Some(RelaySendRefusalV1 {
            scope: RelayRefusalScopeV1::Grant,
            retry_action: RelayRetryActionV1::RetryAfterRelease,
            own: Some(RelayOwnShareV1 {
                dimension: RelayCapacityDimensionV1::Messages,
                limit: capacity(2, 131_072),
                retained: capacity(2, (first.body.len() + second.body.len()) as u64),
                requested_bytes: blocked.body.len() as u64,
                next_release_at: Some(expiry_of(&w.path_a(), &first.operation_id)),
            }),
        })
    );
    assert_eq!(
        w.a.head().unwrap(),
        head,
        "a grant refusal wrote an audit fact"
    );
    assert_eq!(
        retained(&w.path_a()).0,
        2,
        "a grant refusal retained its body"
    );
    assert!(w.a.integrity().unwrap().valid);

    // The durable rate of one grant: wait for the window, nothing to release.
    let w = world(RelayRetentionPolicyV1::default()).await;
    let tasks = directed(&w.a1, &w.a2, "tasks");
    authorize(&w.app, &tasks).await;
    let server = Listener::start(w.app.clone()).await;
    let client_a1 = server.worker(&w.token_a1, &w.a1);
    let started = std::time::Instant::now();
    for _ in 0..MAX_MESSAGES_PER_MINUTE {
        client_accepts(&client_a1, &message(&tasks, 60, "r")).await;
    }
    // A fresh window of the shared I/O admission gate for the refused send.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let failure = client_a1
        .send_with_refusal(&message(&tasks, 60, "over the rate"))
        .await
        .unwrap_err();
    assert!(
        started.elapsed() < std::time::Duration::from_secs(50),
        "fixture too slow: the 60-second rate window moved during the fill"
    );
    assert_eq!(failure.error, Capacity);
    assert_eq!(
        failure.refusal,
        Some(RelaySendRefusalV1 {
            scope: RelayRefusalScopeV1::Grant,
            retry_action: RelayRetryActionV1::RetryAfterWindow,
            own: None,
        })
    );
    assert!(
        failure.to_string().contains("60-second window"),
        "{failure}"
    );
    assert_eq!(retained(&w.path_a()), (60, 60));
    assert!(w.a.integrity().unwrap().valid);
}
