#![cfg(unix)]

use agentsandbox::{
    broker::{
        checkpoint::{BrokerCheckpointV2, SupervisionCheckpointV1, SupervisionOperationV1},
        intent::ExecutionIntentV1,
        journal::{
            BindingInputs, ControlSecrets, FsExclusions, Journal, JournalError, JournalScopeV1,
            ReserveResult,
        },
        lifecycle::{
            BrokerCheckpointV3, LocalCommandRequestV1, LocalCommandV1, LocalFailureCodeV1,
            LocalFailurePhaseV1,
        },
        outbox::{BrokerCheckpointV4, OutboxEventV1},
        writer::WriterClient,
    },
    control::ExportRequestV1,
    control_fs::Directory,
    model::{Decision, DecisionRequest, Event, Outcome},
    runner::CheckpointPayloadV1,
};
use serde_json::{Value, json};
use std::{fs, os::unix::fs::PermissionsExt, time::Duration};
use tempfile::TempDir;
use uuid::Uuid;

const SYNTHETIC_SECRET: &str = "outbox-synthetic-secret-canary-7d91";

struct Fixture {
    writer: WriterClient,
    journal: Journal,
    _root: TempDir,
}

impl Fixture {
    fn new(control_secrets: &[&str]) -> Self {
        Self::with_timeout(control_secrets, Duration::from_secs(10))
    }

    fn with_timeout(control_secrets: &[&str], timeout: Duration) -> Self {
        let root = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let journal_root = root.path().join("journal");
        fs::create_dir(&journal_root).unwrap();
        fs::set_permissions(&journal_root, fs::Permissions::from_mode(0o700)).unwrap();
        let directory = Directory::open_existing(&journal_root.canonicalize().unwrap()).unwrap();
        let scope = JournalScopeV1::new(
            Uuid::from_u128(0x11111111111141118111111111111111),
            "tenant-broker-outbox-test",
            "http://127.0.0.1:8787",
            Uuid::from_u128(0x22222222222242228222222222222222),
            "broker-outbox-test",
        )
        .unwrap();
        let control = ControlSecrets::new(
            control_secrets
                .iter()
                .map(|secret| (*secret).to_owned())
                .collect(),
        )
        .unwrap();
        let journal = Journal::open(
            &directory,
            scope,
            &FsExclusions::new(),
            Default::default(),
            control,
        )
        .unwrap();
        let writer = WriterClient::start(journal.clone(), 16, timeout).unwrap();
        Self {
            writer,
            journal,
            _root: root,
        }
    }

    fn accept(&self, operation_id: Uuid, session_id: Uuid) -> (ExecutionIntentV1, DecisionRequest) {
        let plan = legacy_plan_value(operation_id, session_id);
        let request: DecisionRequest = serde_json::from_value(plan["request"].clone()).unwrap();
        let intent = ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&plan).unwrap())
            .expect("the frozen legacy plan describes a valid no-credential execution intent");
        let ticket = match self
            .journal
            .reserve(operation_id, br#"{"synthetic_outbox_test":true}"#)
            .unwrap()
        {
            ReserveResult::New(ticket) => ticket,
            other => panic!("expected a fresh operation reservation, got {other:?}"),
        };
        let initial = CheckpointPayloadV1::initial(&intent).unwrap();
        self.journal
            .accept(ticket, &intent, BindingInputs::None, &initial)
            .unwrap();
        (intent, request)
    }
}

fn legacy_plan_value(operation_id: Uuid, session_id: Uuid) -> Value {
    let mut plan: Value = serde_json::from_str(include_str!("fixtures/broker_intent-plan-v3.json"))
        .expect("typed frozen private plan fixture");
    plan["version"] = json!(1);
    plan["operation_id"] = json!(operation_id.to_string());
    plan["options"]["profile"] = json!("strict");
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
    plan["request"]["session_id"] = json!(session_id.to_string());
    plan["request"]["sandbox_id"] = json!(session_id.to_string());
    plan["session"] = json!(session_id.to_string());
    plan["request"]
        .as_object_mut()
        .unwrap()
        .remove("provider_resolution");
    plan["provider_input"] = Value::Null;
    plan["provider_credential"] = Value::Null;
    plan
}

fn synthetic_event(request: &DecisionRequest, index: usize) -> Event {
    let mut event = Event::new("agent.progress");
    event.id = Uuid::from_u128(0x70000000000040008000000000000000 + index as u128).to_string();
    event.timestamp = "2026-10-31T12:00:00.000Z".into();
    event.source = "synthetic-outbox-test".into();
    event.provenance = "declared".into();
    event.coverage = "generic".into();
    event.session_id = request.session_id.clone();
    event.project_id = request.project_id.clone();
    event.sandbox_id = request.sandbox_id.clone();
    event.agent_id = request.agent_id.clone();
    event.correlation_id = request.correlation_id.clone();
    event.trace_id = request.trace_id.clone();
    event.data = json!({"synthetic_test": true, "index": index});
    event
}

/// Golden observed before the canonical-byte factorization, with fixed public
/// event identities/timestamps and no runtime or audit services.
#[test]
fn canonical_v4_public_fixture_baseline_and_error_order() {
    use sha2::{Digest, Sha256};

    let operation = Uuid::from_u128(0x66666666666646668666666666666666);
    let session = Uuid::from_u128(0x77777777777747778777777777777777);
    let plan = legacy_plan_value(operation, session);
    let request: DecisionRequest = serde_json::from_value(plan["request"].clone()).unwrap();
    let intent = ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&plan).unwrap()).unwrap();
    let intent_hash = intent.intent_hash().unwrap();
    let runner = CheckpointPayloadV1::initial(&intent).unwrap();
    let broker = BrokerCheckpointV2::new(
        runner,
        SupervisionCheckpointV1::new(
            Uuid::from_u128(0x22222222222242228222222222222222),
            vec![],
            vec![],
        )
        .unwrap(),
    )
    .unwrap();
    let lifecycle = BrokerCheckpointV3::new(broker, vec![]).unwrap();
    let event = OutboxEventV1::new(operation, &intent_hash, synthetic_event(&request, 0)).unwrap();
    let acknowledged = OutboxEventV1::new(operation, &intent_hash, synthetic_event(&request, 1))
        .unwrap()
        .acknowledged(71)
        .unwrap();
    let checkpoint =
        BrokerCheckpointV4::new(lifecycle.clone(), vec![event.clone(), acknowledged]).unwrap();
    let bytes = checkpoint.canonical_bytes().unwrap();
    assert_eq!(bytes.len(), 2172);
    assert_eq!(
        hex::encode(Sha256::digest(&bytes)),
        "f7dab6c73d7fa87da6c4749e3e15465b6e6db5380f801171105c1efbaeb94d47"
    );
    assert_eq!(checkpoint.canonical_bytes().unwrap(), bytes);
    assert_eq!(
        BrokerCheckpointV4::from_bytes(&bytes)
            .unwrap()
            .canonical_bytes()
            .unwrap(),
        bytes
    );

    let wire: Value = serde_json::from_slice(&bytes).unwrap();
    for invalid in [
        {
            let mut value = wire.clone();
            value["version"] = json!(99);
            value
        },
        {
            let mut value = wire.clone();
            value["unexpected"] = Value::Null;
            value
        },
        {
            let mut value = wire.clone();
            value["events"][0]["event"]["unexpected"] = Value::Null;
            value
        },
    ] {
        assert_eq!(
            BrokerCheckpointV4::from_bytes(&serde_json::to_vec(&invalid).unwrap()).unwrap_err(),
            JournalError::InvalidInput
        );
    }
    let mut trailing = bytes;
    trailing.extend_from_slice(b" {}");
    assert_eq!(
        BrokerCheckpointV4::from_bytes(&trailing).unwrap_err(),
        JournalError::InvalidInput
    );
    assert_eq!(
        BrokerCheckpointV4::new(lifecycle.clone(), vec![event.clone(); 2]).unwrap_err(),
        JournalError::Conflict
    );
    // The 512-event cap precedes duplicate event/command identity validation.
    assert_eq!(
        BrokerCheckpointV4::new(lifecycle.clone(), vec![event; 513]).unwrap_err(),
        JournalError::LimitExceeded
    );
    let mut large_event = synthetic_event(&request, 2);
    large_event.data = json!({"padding": "x".repeat(120 * 1024)});
    assert_eq!(
        OutboxEventV1::new(operation, &intent_hash, large_event).unwrap_err(),
        JournalError::TooLarge
    );
    // Each entry fits its 120-KiB cap; 41 entries exceed only the 4-MiB
    // aggregate cap, well below the 32-MiB checkpoint and 512-event limits.
    let large_entries: Vec<_> = (10..51)
        .map(|index| {
            let mut event = synthetic_event(&request, index);
            event.data = json!({"padding": "x".repeat(100 * 1024)});
            OutboxEventV1::new(operation, &intent_hash, event).unwrap()
        })
        .collect();
    assert_eq!(
        BrokerCheckpointV4::new(lifecycle, large_entries).unwrap_err(),
        JournalError::TooLarge
    );
    let oversized_input = vec![b' '; 32 * 1024 * 1024 + 1];
    assert_eq!(
        BrokerCheckpointV4::from_bytes(&oversized_input).unwrap_err(),
        JournalError::TooLarge
    );
}

fn assert_same_head(
    actual: &agentsandbox::broker::journal::RevisionRefV1,
    expected: &agentsandbox::broker::journal::RevisionRefV1,
) {
    assert_eq!(actual.revision(), expected.revision());
    assert_eq!(actual.hash(), expected.hash());
}

fn assert_same_receipts(
    actual: &[agentsandbox::broker::outbox::OutboxReceiptV1],
    expected: &[agentsandbox::broker::outbox::OutboxReceiptV1],
) {
    assert_eq!(actual.len(), expected.len());
    for (actual, expected) in actual.iter().zip(expected) {
        assert_eq!(actual.operation_id(), expected.operation_id());
        assert_eq!(actual.event_id(), expected.event_id());
        assert_eq!(actual.input_hash(), expected.input_hash());
        assert_eq!(actual.audit_seq(), expected.audit_seq());
        assert_same_head(actual.head(), expected.head());
    }
}

fn intent_hash_for(plan: &Value) -> String {
    let bytes = serde_json::to_vec(plan).unwrap();
    ExecutionIntentV1::from_plan_bytes(&bytes)
        .unwrap()
        .intent_hash()
        .unwrap()
}

fn lifecycle_with_export(lifecycle: &BrokerCheckpointV3, request_id: Uuid) -> BrokerCheckpointV3 {
    let mut operations = lifecycle.broker().supervision().operations().to_vec();
    operations.push(SupervisionOperationV1::Export {
        request: ExportRequestV1 {
            version: 1,
            operation_id: request_id.to_string(),
            worker_id: Uuid::from_u128(0xdddddddddddd4ddd8ddddddddddddddd).to_string(),
            attempt_id: Uuid::from_u128(0xeeeeeeeeeeee4eee8eeeeeeeeeeeeeee).to_string(),
            destination_ref: Uuid::from_u128(0xffffffffffff4fff8fffffffffffffff).to_string(),
            destination_intent_hash: "a".repeat(64),
        },
        response: None,
    });
    let supervision = SupervisionCheckpointV1::new(
        lifecycle.broker().supervision().control_namespace_uuid(),
        lifecycle.broker().supervision().helpers().to_vec(),
        operations,
    )
    .unwrap();
    let broker = BrokerCheckpointV2::new(lifecycle.broker().runner().clone(), supervision).unwrap();
    BrokerCheckpointV3::new(broker, lifecycle.commands().to_vec()).unwrap()
}

#[tokio::test]
async fn local_command_event_and_export_ids_are_unique_across_checkpoint_stores() {
    // A durable cancel reserves the ID before a racing outbox insertion.
    let command_first = Fixture::new(&[]);
    let operation = Uuid::from_u128(0x12121212121242128212121212121212);
    let session = Uuid::from_u128(0x13131313131343138313131313131313);
    let (_intent, request) = command_first.accept(operation, session);
    let shared = Uuid::from_u128(0x14141414141444148414141414141414);
    command_first
        .writer
        .append_command(operation, shared, LocalCommandRequestV1::CancelRequested)
        .await
        .unwrap();
    let before = command_first
        .journal
        .load(operation)
        .unwrap()
        .unwrap()
        .head()
        .clone();
    let mut event = synthetic_event(&request, 77);
    event.id = shared.to_string();
    assert_eq!(
        command_first
            .writer
            .enqueue_event(operation, event)
            .await
            .unwrap_err(),
        JournalError::Conflict
    );
    assert_same_head(
        command_first
            .journal
            .load(operation)
            .unwrap()
            .unwrap()
            .head(),
        &before,
    );

    // Conversely, an event already present reserves the ID against a cancel.
    let event_first = Fixture::new(&[]);
    let event_first_operation = Uuid::from_u128(0x15151515151545158515151515151515);
    let session = Uuid::from_u128(0x16161616161646168616161616161616);
    let (_intent, request) = event_first.accept(event_first_operation, session);
    let shared = Uuid::from_u128(0x17171717171747178717171717171717);
    let mut event = synthetic_event(&request, 78);
    event.id = shared.to_string();
    event_first
        .writer
        .enqueue_event(event_first_operation, event)
        .await
        .unwrap();
    let before = event_first
        .journal
        .load(event_first_operation)
        .unwrap()
        .unwrap()
        .head()
        .clone();
    assert_eq!(
        event_first
            .writer
            .append_command(
                event_first_operation,
                shared,
                LocalCommandRequestV1::CancelRequested
            )
            .await
            .unwrap_err(),
        JournalError::Conflict
    );
    assert_same_head(
        event_first
            .journal
            .load(event_first_operation)
            .unwrap()
            .unwrap()
            .head(),
        &before,
    );

    // Metadata-only ExportRequest shape is valid without a store grant, but its
    // operation UUID shares the same namespace as events and local commands.
    let export_first = Fixture::new(&[]);
    let operation = Uuid::from_u128(0x18181818181848188818181818181818);
    let session = Uuid::from_u128(0x19191919191949198919191919191919);
    let (_intent, request) = export_first.accept(operation, session);
    let shared = Uuid::from_u128(0x20202020202040208020202020202020);
    export_first
        .writer
        .append_command(operation, shared, LocalCommandRequestV1::CancelRequested)
        .await
        .unwrap();
    let job = export_first.journal.load(operation).unwrap().unwrap();
    let lifecycle = job.broker_checkpoint_v3().unwrap();
    let export_and_cancel = lifecycle_with_export(lifecycle, shared);
    assert_eq!(
        BrokerCheckpointV4::new(export_and_cancel, vec![]).unwrap_err(),
        JournalError::Conflict
    );

    // Reverse ordering: an export operation already in the checkpoint reserves
    // its UUID against a subsequently added local cancel.
    let export_id = Uuid::from_u128(0x22222222222242228222222222222223);
    let export_before_cancel = lifecycle_with_export(lifecycle, export_id);
    let cancel = LocalCommandV1::new(
        operation,
        &intent_hash_for(&legacy_plan_value(operation, session)),
        export_id,
        LocalCommandRequestV1::CancelRequested,
    )
    .unwrap();
    let export_cancel =
        BrokerCheckpointV3::new(export_before_cancel.broker().clone(), vec![cancel]).unwrap();
    assert_eq!(
        BrokerCheckpointV4::new(export_cancel, vec![]).unwrap_err(),
        JournalError::Conflict
    );

    let event_id = Uuid::from_u128(0x17171717171747178717171717171717);
    let mut event = synthetic_event(&request, 79);
    event.id = event_id.to_string();
    let export_first_lifecycle = lifecycle_with_export(lifecycle, event_id);
    let intent = ExecutionIntentV1::from_plan_bytes(
        &serde_json::to_vec(&legacy_plan_value(operation, session)).unwrap(),
    )
    .unwrap();
    let outbox_event =
        OutboxEventV1::new(operation, &intent.intent_hash().unwrap(), event).unwrap();
    assert_eq!(
        BrokerCheckpointV4::new(export_first_lifecycle, vec![outbox_event.clone()]).unwrap_err(),
        JournalError::Conflict
    );

    // The opposite order is covered from a persisted event checkpoint too:
    // appending export metadata with that already-used event ID is a conflict.
    let event_job = event_first
        .journal
        .load(event_first_operation)
        .unwrap()
        .unwrap();
    let event_checkpoint = event_job.broker_checkpoint_v4().unwrap();
    let event_then_export = lifecycle_with_export(event_checkpoint.lifecycle(), event_id);
    assert_eq!(
        BrokerCheckpointV4::new(event_then_export, event_checkpoint.events().to_vec()).unwrap_err(),
        JournalError::Conflict
    );
}

#[tokio::test]
async fn five_hundred_events_birth_and_ack_in_single_revisions_with_stable_history() {
    let fixture = Fixture::new(&[]);
    let operation_id = Uuid::from_u128(0x33333333333343338333333333333333);
    let session_id = Uuid::from_u128(0x22222222222242228222222222222222);
    let (_intent, request) = fixture.accept(operation_id, session_id);
    let before = fixture.journal.load(operation_id).unwrap().unwrap();
    let events: Vec<_> = (0..500)
        .map(|index| synthetic_event(&request, index))
        .collect();

    let pending = fixture
        .writer
        .enqueue_events(operation_id, events.clone())
        .await
        .unwrap();
    assert_eq!(pending.len(), 500);
    let pending_head = pending[0].head().clone();
    assert_eq!(pending_head.revision(), before.head().revision() + 1);
    assert!(pending.iter().all(|receipt| receipt.audit_seq().is_none()));
    assert!(pending.iter().all(|receipt| {
        receipt.head().revision() == pending_head.revision()
            && receipt.head().hash() == pending_head.hash()
    }));

    let replayed_pending = fixture
        .writer
        .enqueue_events(operation_id, events.clone())
        .await
        .unwrap();
    assert_same_receipts(&replayed_pending, &pending);

    // These positive sequence numbers are synthetic Journal inputs, not audit evidence.
    let acknowledgements: Vec<_> = pending
        .iter()
        .enumerate()
        .map(|(index, receipt)| (receipt.event_id(), 20_001 + index as i64))
        .collect();
    let acknowledged = fixture
        .writer
        .acknowledge_events(operation_id, acknowledgements.clone())
        .await
        .unwrap();
    assert_eq!(acknowledged.len(), 500);
    let ack_head = acknowledged[0].head().clone();
    assert_eq!(ack_head.revision(), pending_head.revision() + 1);
    assert!(acknowledged.iter().enumerate().all(|(index, receipt)| {
        receipt.audit_seq() == Some(20_001 + index as i64)
            && receipt.head().revision() == ack_head.revision()
            && receipt.head().hash() == ack_head.hash()
    }));

    let current = fixture.journal.load(operation_id).unwrap().unwrap();
    let checkpoint = current.broker_checkpoint_v4().unwrap();
    assert_eq!(checkpoint.events().len(), 500);
    assert!(
        checkpoint
            .events()
            .iter()
            .enumerate()
            .all(|(index, event)| event.audit_seq() == Some(20_001 + index as i64))
    );

    let replayed_pending_after_ack = fixture
        .writer
        .enqueue_events(operation_id, events)
        .await
        .unwrap();
    assert_same_receipts(&replayed_pending_after_ack, &pending);
    let replayed_ack = fixture
        .writer
        .acknowledge_events(operation_id, acknowledgements)
        .await
        .unwrap();
    assert_same_receipts(&replayed_ack, &acknowledged);
}

#[tokio::test]
async fn duplicate_divergent_and_conflicting_events_do_not_mutate_head() {
    let fixture = Fixture::new(&[]);
    let operation_id = Uuid::from_u128(0x44444444444444448444444444444444);
    let session_id = Uuid::from_u128(0x55555555555545558555555555555555);
    let (_intent, request) = fixture.accept(operation_id, session_id);
    let event = synthetic_event(&request, 0);
    let pending = fixture
        .writer
        .enqueue_event(operation_id, event.clone())
        .await
        .unwrap();
    let born_head = pending.head().clone();

    assert_eq!(
        fixture
            .writer
            .enqueue_events(operation_id, vec![event.clone(), event.clone()])
            .await
            .unwrap_err(),
        JournalError::Conflict
    );
    assert_same_head(
        fixture.journal.load(operation_id).unwrap().unwrap().head(),
        &born_head,
    );

    let mut divergent = event.clone();
    divergent.data = json!({"synthetic_test": true, "changed": true});
    assert_eq!(
        fixture
            .writer
            .enqueue_event(operation_id, divergent)
            .await
            .unwrap_err(),
        JournalError::Conflict
    );
    assert_same_head(
        fixture.journal.load(operation_id).unwrap().unwrap().head(),
        &born_head,
    );

    let event_id = pending.event_id();
    assert_eq!(
        fixture
            .writer
            .acknowledge_events(operation_id, vec![(event_id, 71), (event_id, 71)])
            .await
            .unwrap_err(),
        JournalError::Conflict
    );
    assert_same_head(
        fixture.journal.load(operation_id).unwrap().unwrap().head(),
        &born_head,
    );

    let ack = fixture
        .writer
        .acknowledge_event(operation_id, event_id, 71)
        .await
        .unwrap();
    let ack_head = ack.head().clone();
    assert_eq!(
        fixture
            .writer
            .acknowledge_event(operation_id, event_id, 72)
            .await
            .unwrap_err(),
        JournalError::Conflict
    );
    assert_same_head(
        fixture.journal.load(operation_id).unwrap().unwrap().head(),
        &ack_head,
    );
}

#[tokio::test]
async fn v4_survives_command_and_lifecycle_updates_and_direct_v3_downgrade_is_refused() {
    let fixture = Fixture::new(&[]);
    let operation_id = Uuid::from_u128(0x66666666666646668666666666666666);
    let session_id = Uuid::from_u128(0x77777777777747778777777777777777);
    let (_intent, request) = fixture.accept(operation_id, session_id);
    let accepted = fixture.journal.load(operation_id).unwrap().unwrap();
    let initial_lifecycle = BrokerCheckpointV3::new(
        BrokerCheckpointV2::new(
            accepted.checkpoint().clone(),
            SupervisionCheckpointV1::new(accepted.scope().control_namespace_uuid(), vec![], vec![])
                .unwrap(),
        )
        .unwrap(),
        vec![],
    )
    .unwrap();
    let v3_receipt = fixture
        .writer
        .advance_lifecycle(
            operation_id,
            accepted.head().clone(),
            initial_lifecycle.clone(),
        )
        .await
        .unwrap();
    let v3_replay = fixture
        .writer
        .advance_lifecycle(operation_id, accepted.head().clone(), initial_lifecycle)
        .await
        .unwrap();
    assert_same_head(v3_replay.head(), v3_receipt.head());
    let event = fixture
        .writer
        .enqueue_event(operation_id, synthetic_event(&request, 0))
        .await
        .unwrap();
    fixture
        .writer
        .acknowledge_event(operation_id, event.event_id(), 71)
        .await
        .unwrap();

    fixture
        .writer
        .append_command(
            operation_id,
            Uuid::from_u128(0x88888888888848888888888888888888),
            LocalCommandRequestV1::FailureObserved {
                phase: LocalFailurePhaseV1::Export,
                code: LocalFailureCodeV1::IoFailed,
            },
        )
        .await
        .unwrap();
    let after_command = fixture.journal.load(operation_id).unwrap().unwrap();
    let checkpoint = after_command.broker_checkpoint_v4().unwrap();
    assert_eq!(checkpoint.events().len(), 1);
    assert_eq!(checkpoint.events()[0].audit_seq(), Some(71));
    assert_eq!(checkpoint.lifecycle().commands().len(), 1);
    let acknowledged_events = serde_json::to_vec(checkpoint.events()).unwrap();

    let expected = after_command.head().clone();
    let lifecycle = checkpoint.lifecycle().clone();
    let lifecycle_receipt = fixture
        .writer
        .advance_lifecycle(operation_id, expected.clone(), lifecycle.clone())
        .await
        .unwrap();
    assert_eq!(lifecycle_receipt.head().revision(), expected.revision() + 1);
    let after_lifecycle = fixture.journal.load(operation_id).unwrap().unwrap();
    let current_v4 = after_lifecycle.broker_checkpoint_v4().unwrap();
    assert_eq!(current_v4.events().len(), 1);
    assert_eq!(current_v4.lifecycle().commands().len(), 1);
    assert_eq!(
        serde_json::to_vec(current_v4.events()).unwrap(),
        acknowledged_events
    );

    // Unlike direct checkpoint history replay, the writer's V4 adapter has
    // always required the current expected head, even for identical contents.
    assert_eq!(
        fixture
            .writer
            .advance_lifecycle(operation_id, expected, lifecycle)
            .await
            .unwrap_err(),
        JournalError::Conflict
    );
    assert_same_head(
        fixture.journal.load(operation_id).unwrap().unwrap().head(),
        after_lifecycle.head(),
    );

    let before_downgrade = after_lifecycle.head().clone();
    assert_eq!(
        fixture
            .journal
            .advance_lifecycle(
                operation_id,
                before_downgrade.clone(),
                current_v4.lifecycle()
            )
            .unwrap_err(),
        JournalError::Conflict
    );
    let after_refusal = fixture.journal.load(operation_id).unwrap().unwrap();
    assert_same_head(after_refusal.head(), &before_downgrade);
    assert_eq!(
        after_refusal.broker_checkpoint_v4().unwrap().events().len(),
        1
    );

    let before_events = serde_json::to_vec(current_v4.events()).unwrap();
    let stops = fixture.writer.stops();
    let collision = stops
        .append_cancel(
            agentsandbox::broker::writer::CancelInputV1::new(
                operation_id,
                current_v4.events()[0].event_id(),
            )
            .unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(collision.error(), JournalError::Conflict);
    let collision = stops
        .append_cancel(
            agentsandbox::broker::writer::CancelInputV1::new(
                operation_id,
                Uuid::from_u128(0x88888888888848888888888888888888),
            )
            .unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(collision.error(), JournalError::Conflict);
    assert_same_head(
        fixture.journal.load(operation_id).unwrap().unwrap().head(),
        &before_downgrade,
    );
    let cancel_id = Uuid::from_u128(0x99999999999949998999999999999999);
    let cancel = stops
        .append_cancel(
            agentsandbox::broker::writer::CancelInputV1::new(operation_id, cancel_id).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        cancel.receipt().head().revision(),
        before_downgrade.revision() + 1
    );
    let after_stop = fixture.journal.load(operation_id).unwrap().unwrap();
    let v4 = after_stop.broker_checkpoint_v4().unwrap();
    assert_eq!(v4.lifecycle().commands().len(), 2);
    assert_eq!(serde_json::to_vec(v4.events()).unwrap(), before_events);
    let replay = stops
        .append_cancel(
            agentsandbox::broker::writer::CancelInputV1::new(operation_id, cancel_id).unwrap(),
        )
        .await
        .unwrap();
    assert_same_head(replay.receipt().head(), cancel.receipt().head());
    assert_same_head(
        fixture.journal.load(operation_id).unwrap().unwrap().head(),
        after_stop.head(),
    );
}

#[tokio::test]
async fn wrong_context_secrets_strict_nested_decision_and_event_cap_are_rejected_without_writes() {
    // This checks functional event bounds in debug, not qualified 10-second
    // latency. Only this test uses the existing Writer API's 60-second ceiling.
    let fixture = Fixture::with_timeout(&[SYNTHETIC_SECRET], Duration::from_secs(60));
    let first_operation = Uuid::from_u128(0x99999999999949998999999999999999);
    let first_session = Uuid::from_u128(0xaaaaaaaaaaaa4aaa8aaaaaaaaaaaaaaa);
    let second_operation = Uuid::from_u128(0xbbbbbbbbbbbb4bbb8bbbbbbbbbbbbbbb);
    let second_session = Uuid::from_u128(0xcccccccccccc4ccc8ccccccccccccccc);
    let (intent, first_request) = fixture.accept(first_operation, first_session);
    let (_second_intent, second_request) = fixture.accept(second_operation, second_session);
    let boundary: Vec<_> = (0..512)
        .map(|index| synthetic_event(&first_request, index))
        .collect();
    let at_cap = fixture
        .writer
        .enqueue_events(first_operation, boundary)
        .await
        .unwrap();
    assert_eq!(at_cap.len(), 512);
    let first_head = fixture
        .journal
        .load(first_operation)
        .unwrap()
        .unwrap()
        .head()
        .clone();
    let second_head = fixture
        .journal
        .load(second_operation)
        .unwrap()
        .unwrap()
        .head()
        .clone();

    let over_cap: Vec<_> = (1_000..1_513)
        .map(|index| synthetic_event(&first_request, index))
        .collect();
    assert_eq!(
        fixture
            .writer
            .enqueue_events(first_operation, over_cap)
            .await
            .unwrap_err(),
        JournalError::LimitExceeded
    );
    assert_same_head(
        fixture
            .journal
            .load(first_operation)
            .unwrap()
            .unwrap()
            .head(),
        &first_head,
    );

    assert_eq!(
        fixture
            .writer
            .enqueue_event(second_operation, synthetic_event(&first_request, 900))
            .await
            .unwrap_err(),
        JournalError::Conflict
    );
    assert_same_head(
        fixture
            .journal
            .load(second_operation)
            .unwrap()
            .unwrap()
            .head(),
        &second_head,
    );

    let mut secret_in_key = synthetic_event(&second_request, 901);
    let mut data = serde_json::Map::new();
    data.insert(SYNTHETIC_SECRET.into(), json!("synthetic"));
    secret_in_key.data = Value::Object(data);
    assert_eq!(
        fixture
            .writer
            .enqueue_event(second_operation, secret_in_key)
            .await
            .unwrap_err(),
        JournalError::ControlSecret
    );
    assert_same_head(
        fixture
            .journal
            .load(second_operation)
            .unwrap()
            .unwrap()
            .head(),
        &second_head,
    );

    let mut secret_in_value = synthetic_event(&second_request, 902);
    secret_in_value.data = json!({"synthetic_value": SYNTHETIC_SECRET});
    assert_eq!(
        fixture
            .writer
            .enqueue_event(second_operation, secret_in_value)
            .await
            .unwrap_err(),
        JournalError::ControlSecret
    );
    assert_same_head(
        fixture
            .journal
            .load(second_operation)
            .unwrap()
            .unwrap()
            .head(),
        &second_head,
    );

    let mut decision_event = synthetic_event(&first_request, 903);
    decision_event.decision = Some(Decision::new(Outcome::Review, "synthetic", "synthetic"));
    let entry = OutboxEventV1::new(
        first_operation,
        &intent.intent_hash().unwrap(),
        decision_event,
    )
    .unwrap();
    let mut wire = serde_json::to_value(&entry).unwrap();
    wire["event"]["decision"]["unexpected_decision_field"] = Value::Null;
    let error = serde_json::from_value::<OutboxEventV1>(wire).unwrap_err();
    let diagnostic = error.to_string();
    assert!(!diagnostic.contains("unexpected_decision_field"));
    assert!(!diagnostic.contains(SYNTHETIC_SECRET));
    assert!(!format!("{entry:?}").contains("synthetic"));
}
