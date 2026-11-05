use super::*;
use crate::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::{EventQuery, Store},
    broker::{
        checkpoint::{BrokerCheckpointV2, SupervisionCheckpointV1},
        intent::ExecutionIntentV1,
        journal::{
            BindingInputs, ControlSecrets, FsExclusions, Journal, JournalLimitsV1, JournalScopeV1,
            ReserveResult, RevisionRefV1, recovery::StopRecoveryReaderV1,
        },
        lifecycle::BrokerCheckpointV3,
        outbox::{BrokerCheckpointV4, OutboxEventV1},
    },
    control_fs::Directory,
    model::Event,
    policy::Policy,
    redaction::Redactor,
    runner::{AuditClient, CheckpointPayloadV1, ColdAuditBudget, ExactStoredEventReceipt},
};
use axum::{
    body::Body,
    extract::State,
    http::{Method, Request},
    middleware::{self, Next},
    response::Response,
};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{net::TcpListener, task::JoinHandle};
use uuid::Uuid;

struct Fixture {
    root: tempfile::TempDir,
    scope: JournalScopeV1,
    operation_id: Uuid,
    head: RevisionRefV1,
    coverage_id: Uuid,
    result_id: Uuid,
}

impl Fixture {
    fn new() -> Self {
        Self::new_with_origin("http://127.0.0.1:8787")
    }

    fn new_with_origin(gateway_origin: &str) -> Self {
        let root = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let directory = Directory::open_existing(&root.path().canonicalize().unwrap()).unwrap();
        let scope = JournalScopeV1::new(
            Uuid::new_v4(),
            "tenant-cold-audit-writer",
            gateway_origin,
            Uuid::new_v4(),
            "cold-audit-writer-test",
        )
        .unwrap();
        let journal = Journal::open(
            &directory,
            scope.clone(),
            &FsExclusions::new(),
            JournalLimitsV1::default(),
            ControlSecrets::new(Vec::new()).unwrap(),
        )
        .unwrap();
        let operation_id = Uuid::new_v4();
        let intent = intent_for(operation_id, gateway_origin);
        let initial = CheckpointPayloadV1::initial(&intent).unwrap();
        let ReserveResult::New(ticket) = journal
            .reserve(operation_id, br#"{"fixture":"cold-audit-writer"}"#)
            .unwrap()
        else {
            panic!("fresh metadata fixture must reserve a new operation");
        };
        let accepted = journal
            .accept(ticket, &intent, BindingInputs::None, &initial)
            .unwrap();
        let accepted_job = journal.load(operation_id).unwrap().unwrap();
        let supervision =
            SupervisionCheckpointV1::new(scope.control_namespace_uuid(), Vec::new(), Vec::new())
                .unwrap();
        let supervised =
            BrokerCheckpointV2::new(accepted_job.checkpoint().clone(), supervision).unwrap();
        drop(accepted_job);
        let supervised_receipt = journal
            .advance_supervised(operation_id, accepted.head().clone(), &supervised)
            .unwrap();
        let lifecycle = BrokerCheckpointV3::new(supervised, Vec::new()).unwrap();
        let lifecycle_receipt = journal
            .advance_lifecycle(operation_id, supervised_receipt.head().clone(), &lifecycle)
            .unwrap();

        let coverage = outbox_event(operation_id, &intent, "observer.coverage");
        let result = outbox_event(operation_id, &intent, "broker.batch.result");
        let coverage_id = coverage.event_id();
        let result_id = result.event_id();
        let outbox = BrokerCheckpointV4::new(lifecycle, vec![coverage, result]).unwrap();
        let receipt = journal
            .advance_outbox(operation_id, lifecycle_receipt.head().clone(), &outbox)
            .unwrap();
        Self {
            root,
            scope,
            operation_id,
            head: receipt.head().clone(),
            coverage_id,
            result_id,
        }
    }

    fn report(&self) -> crate::broker::journal::recovery::StopRecoveryReportV1 {
        let directory =
            Directory::open_existing(&self.root.path().canonicalize().unwrap()).unwrap();
        StopRecoveryReaderV1::open_existing(
            &directory,
            self.scope.clone(),
            &FsExclusions::new(),
            JournalLimitsV1::default(),
            ControlSecrets::new(Vec::new()).unwrap(),
        )
        .unwrap()
        .expect("the fixture retains its existing profile lease")
        .scan()
    }

    fn reopen_after_actor_exit(&self) -> Journal {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let directory =
                Directory::open_existing(&self.root.path().canonicalize().unwrap()).unwrap();
            match Journal::open(
                &directory,
                self.scope.clone(),
                &FsExclusions::new(),
                JournalLimitsV1::default(),
                ControlSecrets::new(Vec::new()).unwrap(),
            ) {
                Ok(journal) => return journal,
                Err(JournalError::Busy) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("writer actor did not release its lease: {error}"),
            }
        }
    }
}

struct HttpGatewayFixture {
    _directory: tempfile::TempDir,
    origin: String,
    token: String,
    store: Arc<Store>,
    initial_head: i64,
    initial_events: Vec<Value>,
    post_count: Arc<AtomicUsize>,
    task: JoinHandle<()>,
}

impl Drop for HttpGatewayFixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl HttpGatewayFixture {
    async fn new(tenant_id: &str) -> Self {
        let directory = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .expect("private actor HTTP Store fixture");
        let store = Arc::new(
            Store::open_for_tenant(
                &directory.path().join("tenant.sqlite"),
                tenant_id,
                Redactor::new(vec![]),
                1000,
            )
            .unwrap(),
        );
        // Store::open_for_tenant calls control::initialize on a fresh schema,
        // which appends control.schema.genesis. Preserve the observed baseline.
        let initial_head = store.head().unwrap();
        let initial_events = store
            .events(&EventQuery {
                limit: Some(1000),
                ..Default::default()
            })
            .unwrap()
            .iter()
            .map(|event| serde_json::to_value(event).unwrap())
            .collect::<Vec<_>>();
        assert!(
            initial_events
                .iter()
                .any(|event| event["kind"] == "control.schema.genesis")
        );
        let token = format!("cold-audit-actor-runner-{}", "r".repeat(40));
        let viewer_token = format!("cold-audit-actor-viewer-{}", "v".repeat(40));
        let state = GatewayState::for_tenants(
            vec![TenantGateway {
                tenant_id: tenant_id.to_owned(),
                store: Arc::clone(&store),
                runner_token: token.clone(),
                viewer_token,
            }],
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )
        .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let post_count = Arc::new(AtomicUsize::new(0));
        let router = gateway_router(state).layer(middleware::from_fn_with_state(
            Arc::clone(&post_count),
            count_event_posts,
        ));
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            _directory: directory,
            origin,
            token,
            store,
            initial_head,
            initial_events,
            post_count,
            task,
        }
    }

    async fn close(&mut self) {
        self.task.abort();
        let _ = (&mut self.task).await;
    }

    async fn submit_pending(&self, reserved: &ReservedColdAudit) -> Vec<ExactStoredEventReceipt> {
        let client = AuditClient::new(&self.origin, &self.token).unwrap();
        let budget = ColdAuditBudget::start();
        let mut receipts = Vec::new();
        for pending in reserved.prepared().pending_events() {
            receipts.push(
                client
                    .post_cold_audit_event(pending.event(), &budget)
                    .await
                    .unwrap(),
            );
        }
        receipts
    }

    fn post_count(&self) -> usize {
        self.post_count.load(Ordering::SeqCst)
    }
}

async fn count_event_posts(
    State(counter): State<Arc<AtomicUsize>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if request.method() == Method::POST && request.uri().path() == "/api/v1/events" {
        counter.fetch_add(1, Ordering::SeqCst);
    }
    next.run(request).await
}

fn stop_capabilities(fixture: &Fixture) -> ProfileWriterCapabilities {
    let report = fixture.report();
    let capabilities =
        ProfileWriterCapabilities::from_recovery_report(&report, true, 1, Duration::from_secs(10))
            .unwrap();
    drop(report);
    capabilities
}

fn receipt_pairs(receipts: &[ExactStoredEventReceipt]) -> Vec<(Uuid, i64)> {
    receipts
        .iter()
        .map(|receipt| {
            (
                Uuid::parse_str(&receipt.stored_event().event.id).unwrap(),
                receipt.seq(),
            )
        })
        .collect()
}

fn assert_durable_receipts(journal: &Journal, fixture: &Fixture, expected: &[(Uuid, i64)]) {
    let job = journal
        .load(fixture.operation_id)
        .unwrap()
        .expect("accepted metadata remains durable after actor shutdown");
    let checkpoint = job
        .broker_checkpoint_v4()
        .expect("the job retains its V4 outbox");
    assert_eq!(
        checkpoint
            .events()
            .iter()
            .filter(|event| event.audit_seq().is_some())
            .count(),
        expected.len()
    );
    for (event_id, seq) in expected {
        assert_eq!(
            checkpoint
                .events()
                .iter()
                .find(|event| event.event_id() == *event_id)
                .and_then(|event| event.audit_seq()),
            Some(*seq),
            "the durable V4 ACK retains the exact central Store sequence"
        );
    }
}

fn assert_http_store_matches(gateway: &HttpGatewayFixture, receipts: &[ExactStoredEventReceipt]) {
    assert_eq!(
        receipts.len(),
        2,
        "the complete retained pair has two receipts"
    );
    assert_eq!(gateway.post_count(), receipts.len());
    let head = gateway.store.head().unwrap();
    assert_eq!(head - gateway.initial_head, receipts.len() as i64);
    let stored = gateway
        .store
        .events(&EventQuery {
            limit: Some(1000),
            ..Default::default()
        })
        .unwrap();
    let stored_values = stored
        .iter()
        .map(|event| serde_json::to_value(event).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        stored_values.len(),
        gateway.initial_events.len() + receipts.len(),
        "the Store preserves its initial rows and appends exactly the receipt batch"
    );
    assert!(
        gateway
            .initial_events
            .iter()
            .all(|event| stored_values.contains(event))
    );
    for receipt in receipts {
        let event_id = &receipt.stored_event().event.id;
        let actual = stored
            .iter()
            .find(|entry| entry.event.id == *event_id)
            .expect("real gateway Store contains every exact HTTP receipt");
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            serde_json::to_value(receipt.stored_event()).unwrap()
        );
    }
}

fn intent_for(operation_id: Uuid, gateway_origin: &str) -> ExecutionIntentV1 {
    let mut plan: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/broker_intent-plan-v3.json"
    ))
    .unwrap();
    plan["operation_id"] = json!(operation_id);
    plan["version"] = json!(1);
    plan["options"]["profile"] = json!("strict");
    plan["options"]["gateway"] = json!(gateway_origin);
    plan["options"]["network"] = json!("none");
    plan["options"]["provider"] = json!("generic");
    plan["options"]["argv"] = json!(["/usr/bin/id"]);
    for field in [
        "client",
        "auth_mode",
        "client_profile",
        "providers_file",
        "prompt",
        "model",
    ] {
        plan["options"].as_object_mut().unwrap().remove(field);
    }
    plan["request"]["profile"] = json!("strict");
    plan["request"]["agent_id"] = json!("generic-22222222-2222-4222-8222-222222222222");
    plan["request"]["command"]["argv"] = json!(["/usr/bin/id"]);
    plan["request"]
        .as_object_mut()
        .unwrap()
        .remove("provider_resolution");
    plan["provider_input"] = Value::Null;
    plan["provider_credential"] = Value::Null;
    ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&plan).unwrap()).unwrap()
}

fn outbox_event(operation_id: Uuid, intent: &ExecutionIntentV1, kind: &str) -> OutboxEventV1 {
    let request = intent.request_data();
    let mut event = Event::new(kind);
    event.data = json!({"fixture": "metadata-only"});
    event.session_id = request.session_id.clone();
    event.project_id = request.project_id.clone();
    event.sandbox_id = request.sandbox_id.clone();
    event.agent_id = request.agent_id.clone();
    event.correlation_id = request.correlation_id.clone();
    event.trace_id = request.trace_id.clone();
    OutboxEventV1::new(operation_id, &intent.intent_hash().unwrap(), event).unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn full_capability_has_no_cold_audit_client_and_rejects_a_misrouted_prepare() {
    let fixture = Fixture::new();
    let report = fixture.report();
    let capabilities =
        ProfileWriterCapabilities::from_recovery_report(&report, false, 1, Duration::from_secs(10))
            .unwrap();
    drop(report);
    assert!(capabilities.full().is_some());
    assert!(
        capabilities.cold_audit().is_none(),
        "a Full backend does not receive the cold-audit client"
    );

    let full = capabilities.full().unwrap();
    let (reply, receive) = tokio::sync::oneshot::channel();
    assert!(
        full.0
            .sender
            .try_send(Command::PrepareColdAudit {
                operation_id: fixture.operation_id,
                reply,
            })
            .is_ok(),
        "the private misroute reaches the actual Full actor"
    );
    assert!(matches!(
        receive.await.unwrap(),
        Err(JournalError::InvalidInput)
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn stops_preparation_is_exact_exclusive_droppable_and_does_not_block_cancel() {
    let fixture = Fixture::new();
    let report = fixture.report();
    let capabilities =
        ProfileWriterCapabilities::from_recovery_report(&report, true, 1, Duration::from_secs(10))
            .unwrap();
    drop(report);

    assert!(capabilities.full().is_none());
    let cold_audit = capabilities
        .cold_audit()
        .expect("only a genuinely admitted Stops backend exposes this client");
    let stops = capabilities.stops();
    assert_eq!(
        cold_audit.prepare(Uuid::nil()).await.err(),
        Some(JournalError::InvalidInput)
    );

    let first = cold_audit.prepare(fixture.operation_id).await.unwrap();
    let prepared = first.prepared();
    assert_eq!(prepared.operation_id(), fixture.operation_id);
    assert_eq!(prepared.scope().tenant_id(), fixture.scope.tenant_id());
    assert_eq!(
        prepared.scope().control_namespace_uuid(),
        fixture.scope.control_namespace_uuid()
    );
    assert_eq!(prepared.head(), &fixture.head);
    assert_eq!(
        prepared
            .pending_events()
            .iter()
            .map(|event| (event.event_id(), event.event().kind.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (fixture.coverage_id, "observer.coverage"),
            (fixture.result_id, "broker.batch.result"),
        ],
        "the reservation retains the exact pending V4 pair"
    );
    assert_eq!(
        cold_audit.prepare(fixture.operation_id).await.err(),
        Some(JournalError::Busy),
        "one live preparation excludes a second reservation"
    );
    drop(first);

    let second = cold_audit.prepare(fixture.operation_id).await.unwrap();
    assert_eq!(second.prepared().head(), &fixture.head);
    let cancel = CancelInputV1::new(fixture.operation_id, Uuid::new_v4()).unwrap();
    assert!(
        stops.append_cancel(cancel).await.is_ok(),
        "the retained STOP capability continues to accept Cancel while cold audit is reserved"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn stops_actor_commits_exact_http_batch_as_one_ack_revision() {
    let mut gateway = HttpGatewayFixture::new("tenant-cold-audit-writer").await;
    let fixture = Fixture::new_with_origin(&gateway.origin);
    let capabilities = stop_capabilities(&fixture);
    let cold_audit = capabilities.cold_audit().unwrap();
    let stops = capabilities.stops();
    let reserved = cold_audit.prepare(fixture.operation_id).await.unwrap();
    let before = reserved.prepared().head().clone();
    let expected_ids = reserved
        .prepared()
        .pending_events()
        .iter()
        .map(|event| event.event_id())
        .collect::<Vec<_>>();

    let receipts = gateway.submit_pending(&reserved).await;
    assert_http_store_matches(&gateway, &receipts);
    let expected_pairs = receipt_pairs(&receipts);
    let waiter = reserved.submit(receipts).unwrap();
    let confirmed = waiter.confirmed().await.unwrap();
    assert_eq!(confirmed.operation_id(), fixture.operation_id);
    let mut confirmed_ids = confirmed.event_ids().to_vec();
    let mut expected_ids = expected_ids;
    confirmed_ids.sort_unstable();
    expected_ids.sort_unstable();
    assert_eq!(confirmed_ids, expected_ids);
    assert_eq!(confirmed.head().revision(), before.revision() + 1);

    gateway.close().await;
    drop(stops);
    drop(cold_audit);
    drop(capabilities);
    let journal = fixture.reopen_after_actor_exit();
    let job = journal.load(fixture.operation_id).unwrap().unwrap();
    assert_eq!(job.head(), confirmed.head());
    assert_eq!(job.head().revision(), before.revision() + 1);
    assert_durable_receipts(&journal, &fixture, &expected_pairs);
}

#[tokio::test(flavor = "current_thread")]
async fn dropped_completion_waiter_keeps_fifo_ack_before_cancel_without_reposting() {
    let mut gateway = HttpGatewayFixture::new("tenant-cold-audit-writer").await;
    let fixture = Fixture::new_with_origin(&gateway.origin);
    let capabilities = stop_capabilities(&fixture);
    let cold_audit = capabilities.cold_audit().unwrap();
    let stops = capabilities.stops();
    let reserved = cold_audit.prepare(fixture.operation_id).await.unwrap();
    let before = reserved.prepared().head().clone();
    let receipts = gateway.submit_pending(&reserved).await;
    assert_http_store_matches(&gateway, &receipts);
    let expected_pairs = receipt_pairs(&receipts);

    let waiter = reserved.submit(receipts).unwrap();
    drop(waiter);
    assert!(
        stops
            .append_cancel(CancelInputV1::new(fixture.operation_id, Uuid::new_v4()).unwrap())
            .await
            .is_ok(),
        "the FIFO Cancel barrier succeeds after the dropped waiter's accepted ACK command"
    );
    assert_eq!(gateway.post_count(), 2, "the actor never repeats HTTP");
    assert_eq!(
        gateway.store.head().unwrap(),
        gateway.initial_head + 2,
        "the two HTTP writes are the only Store appends after initialization"
    );

    gateway.close().await;
    drop(stops);
    drop(cold_audit);
    drop(capabilities);
    let journal = fixture.reopen_after_actor_exit();
    let job = journal.load(fixture.operation_id).unwrap().unwrap();
    assert_eq!(
        job.head().revision(),
        before.revision() + 2,
        "one ACK revision and the later FIFO Cancel are durable"
    );
    assert_durable_receipts(&journal, &fixture, &expected_pairs);
}

#[tokio::test(flavor = "current_thread")]
async fn before_ack_unknown_quarantines_metadata_but_not_actor_sender_or_fifo_cancel() {
    let mut gateway = HttpGatewayFixture::new("tenant-cold-audit-writer").await;
    let fixture = Fixture::new_with_origin(&gateway.origin);
    let capabilities = stop_capabilities(&fixture);
    let cold_audit = capabilities.cold_audit().unwrap();
    let stops = capabilities.stops();
    let reserved = cold_audit
        .prepare(fixture.operation_id)
        .await
        .unwrap()
        .with_test_before_ack_fault();
    let before = reserved.prepared().head().clone();
    let receipts = gateway.submit_pending(&reserved).await;
    assert_http_store_matches(&gateway, &receipts);
    let expected_pairs = receipt_pairs(&receipts);

    let waiter = reserved.submit(receipts).unwrap();
    let outcome = waiter.confirmed().await;
    assert!(
        matches!(
            outcome,
            Err(CompletionFailure::Unknown {
                error: JournalError::Io
            })
        ),
        "the actor returns an unknown I/O result after the real publication fault"
    );
    assert_eq!(
        cold_audit.prepare(fixture.operation_id).await.err(),
        Some(JournalError::Busy),
        "quarantine keeps the single logical reservation after the ambiguous reply"
    );
    assert!(
        stops
            .append_cancel(CancelInputV1::new(fixture.operation_id, Uuid::new_v4()).unwrap())
            .await
            .is_ok(),
        "quarantined metadata does not block ordinary FIFO Cancel"
    );

    gateway.close().await;
    drop(stops);
    drop(cold_audit);
    drop(capabilities);
    let journal = fixture.reopen_after_actor_exit();
    let job = journal.load(fixture.operation_id).unwrap().unwrap();
    assert_eq!(
        job.head().revision(),
        before.revision() + 2,
        "the pre-ACK fault left one durable ACK revision before the FIFO Cancel"
    );
    assert_durable_receipts(&journal, &fixture, &expected_pairs);
    assert_eq!(gateway.post_count(), 2);
}
