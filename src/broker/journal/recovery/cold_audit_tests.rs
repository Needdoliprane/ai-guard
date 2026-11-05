use super::*;
use crate::{
    broker::{
        checkpoint::{BrokerCheckpointV2, SupervisionCheckpointV1},
        intent::ExecutionIntentV1,
        journal::{
            BindingInputs, ControlSecrets, FsExclusions, Journal, JournalError, JournalLimitsV1,
            JournalScopeV1, RejectionCodeV1, RejectionPhaseV1, ReserveResult, StopJournal,
        },
        lifecycle::BrokerCheckpointV3,
        outbox::{BrokerCheckpointV4, OutboxEventV1},
    },
    control_fs::Directory,
    model::Event,
    runner::CheckpointPayloadV1,
};
use serde_json::{Value, json};
use std::{fs, os::unix::fs::PermissionsExt};
use uuid::Uuid;

const MAX_OUTBOX_BYTES: usize = 4 * 1024 * 1024;

struct Fixture {
    _root: tempfile::TempDir,
    journal: Journal,
    operation_id: Uuid,
    head: RevisionRefV1,
}

impl Fixture {
    fn new() -> Self {
        Self::with_pending_fillers(0)
    }

    fn with_pending_fillers(filler_count: usize) -> Self {
        let (mut fixture, intent, initial) = Self::accepted_v1();
        let scope = fixture.journal.scope().clone();
        let supervision =
            SupervisionCheckpointV1::new(scope.control_namespace_uuid(), Vec::new(), Vec::new())
                .unwrap();
        let v2 = BrokerCheckpointV2::new(initial, supervision).unwrap();
        let v3 = BrokerCheckpointV3::new(v2, Vec::new()).unwrap();
        let lifecycle = fixture
            .journal
            .advance_lifecycle(fixture.operation_id, fixture.head.clone(), &v3)
            .unwrap();

        let mut events = vec![
            outbox_event(
                fixture.operation_id,
                &intent,
                "observer.coverage",
                json!({"fixture": true}),
            ),
            outbox_event(
                fixture.operation_id,
                &intent,
                "broker.batch.result",
                json!({"fixture": true}),
            ),
        ];
        for index in 0..filler_count {
            let event = outbox_event(
                fixture.operation_id,
                &intent,
                "fixture.audit",
                json!({"index": index}),
            )
            .acknowledged(i64::try_from(index + 1).unwrap())
            .unwrap();
            events.push(event);
        }
        let v4 = BrokerCheckpointV4::new(v3, events).unwrap();
        let receipt = fixture
            .journal
            .advance_outbox(fixture.operation_id, lifecycle.head().clone(), &v4)
            .unwrap();
        fixture.head = receipt.head().clone();
        fixture
    }

    fn accepted_v1() -> (Self, ExecutionIntentV1, CheckpointPayloadV1) {
        let root = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let directory = Directory::open_existing(&root.path().canonicalize().unwrap()).unwrap();
        let scope = JournalScopeV1::new(
            Uuid::new_v4(),
            "tenant-cold-audit-budget",
            "http://127.0.0.1:8787",
            Uuid::new_v4(),
            "cold-audit-budget-test",
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
        let intent = intent_for(operation_id);
        let initial = CheckpointPayloadV1::initial(&intent).unwrap();
        let ReserveResult::New(ticket) = journal
            .reserve(operation_id, br#"{"fixture":"cold-audit-budget"}"#)
            .unwrap()
        else {
            panic!("fresh metadata fixture must reserve a new operation");
        };
        let accepted = journal
            .accept(ticket, &intent, BindingInputs::None, &initial)
            .unwrap();
        (
            Self {
                _root: root,
                journal,
                operation_id,
                head: accepted.head().clone(),
            },
            intent,
            initial,
        )
    }

    fn stop_journal(&self) -> StopJournal {
        StopJournal::from_full(&self.journal)
    }

    fn advance_to_supervised(&mut self) {
        let job = self.journal.load(self.operation_id).unwrap().unwrap();
        let supervised = BrokerCheckpointV2::new(
            job.checkpoint().clone(),
            SupervisionCheckpointV1::new(
                self.journal.scope().control_namespace_uuid(),
                Vec::new(),
                Vec::new(),
            )
            .unwrap(),
        )
        .unwrap();
        let receipt = self
            .journal
            .advance_supervised(self.operation_id, job.head().clone(), &supervised)
            .unwrap();
        self.head = receipt.head().clone();
    }

    fn grow_outbox_to_before_nested_limit(&mut self) {
        let job = self.journal.load(self.operation_id).unwrap().unwrap();
        let v4 = job.broker_checkpoint_v4().unwrap();
        let lifecycle = v4.lifecycle().clone();
        let mut events = v4.events().to_vec();
        let expected_head = job.head().clone();
        drop(job);

        let intent = intent_for(self.operation_id);
        for _index in 0..46 {
            let filler = outbox_event(
                self.operation_id,
                &intent,
                "fixture.audit",
                json!({"padding": "x".repeat(90_000)}),
            );
            events.push(filler);
        }
        let mut final_event = event_for_intent(&intent, "fixture.audit", json!({"padding": ""}));
        let final_pending = OutboxEventV1::new(
            self.operation_id,
            &intent.intent_hash().unwrap(),
            final_event.clone(),
        )
        .unwrap();

        let target_bytes = MAX_OUTBOX_BYTES - 8;
        let mut sizing_events = events.clone();
        sizing_events.push(final_pending);
        let acked_for_sizing: Vec<_> = sizing_events
            .iter()
            .enumerate()
            .map(|(index, event)| {
                if index < 2 {
                    event.clone()
                } else {
                    event
                        .acknowledged(1_000_000_000_000_000_000 + (index - 2) as i64)
                        .unwrap()
                }
            })
            .collect();
        let base_bytes = super::super::canonical(&acked_for_sizing).unwrap().len();
        let padding = target_bytes
            .checked_sub(base_bytes)
            .expect("fixture padding is positive");
        assert!(padding < 120 * 1024, "one event remains within its own cap");
        final_event.data = json!({"padding": "x".repeat(padding)});
        events.push(
            OutboxEventV1::new(
                self.operation_id,
                &intent.intent_hash().unwrap(),
                final_event,
            )
            .unwrap(),
        );
        let acked_events: Vec<_> = events
            .iter()
            .enumerate()
            .map(|(index, event)| {
                if index < 2 {
                    event.clone()
                } else {
                    event
                        .acknowledged(1_000_000_000_000_000_000 + (index - 2) as i64)
                        .unwrap()
                }
            })
            .collect();
        assert_eq!(
            super::super::canonical(&acked_events).unwrap().len(),
            target_bytes
        );

        let append_pending = BrokerCheckpointV4::new(lifecycle.clone(), events).unwrap();
        let pending_receipt = self
            .journal
            .advance_outbox(self.operation_id, expected_head, &append_pending)
            .unwrap();
        let acknowledge_fillers = BrokerCheckpointV4::new(lifecycle, acked_events).unwrap();
        let receipt = self
            .journal
            .advance_outbox(
                self.operation_id,
                pending_receipt.head().clone(),
                &acknowledge_fillers,
            )
            .unwrap();
        self.head = receipt.head().clone();
    }
}

fn intent_for(operation_id: Uuid) -> ExecutionIntentV1 {
    let mut plan: Value = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/broker_intent-plan-v3.json"
    ))
    .unwrap();
    plan["operation_id"] = json!(operation_id);
    plan["version"] = json!(1);
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
    plan["request"]
        .as_object_mut()
        .unwrap()
        .remove("provider_resolution");
    plan["provider_input"] = Value::Null;
    plan["provider_credential"] = Value::Null;
    ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&plan).unwrap()).unwrap()
}

fn outbox_event(
    operation_id: Uuid,
    intent: &ExecutionIntentV1,
    kind: &str,
    data: Value,
) -> OutboxEventV1 {
    OutboxEventV1::new(
        operation_id,
        &intent.intent_hash().unwrap(),
        event_for_intent(intent, kind, data),
    )
    .unwrap()
}

fn event_for_intent(intent: &ExecutionIntentV1, kind: &str, data: Value) -> Event {
    let request = intent.request_data();
    let mut event = Event::new(kind);
    event.data = data;
    event.session_id = request.session_id.clone();
    event.project_id = request.project_id.clone();
    event.sandbox_id = request.sandbox_id.clone();
    event.agent_id = request.agent_id.clone();
    event.correlation_id = request.correlation_id.clone();
    event.trace_id = request.trace_id.clone();
    event
}

fn estimate(fixture: &Fixture, selected: bool) -> JobBound {
    estimate_at(fixture, selected, fixture.head.clone())
}

fn estimate_at(fixture: &Fixture, selected: bool, head: RevisionRefV1) -> JobBound {
    estimate_result_at(fixture, selected, head).unwrap()
}

fn estimate_result(fixture: &Fixture, selected: bool) -> JournalResult<JobBound> {
    estimate_result_at(fixture, selected, fixture.head.clone())
}

fn estimate_result_at(
    fixture: &Fixture,
    selected: bool,
    head: RevisionRefV1,
) -> JournalResult<JobBound> {
    let stops = fixture.stop_journal();
    let mut writer = stops.core.writer.lock().unwrap();
    let (_reader, recovered, incomplete) = stops
        .read_locked(fixture.operation_id, &mut writer)
        .unwrap()
        .expect("real accepted job is in the metadata inventory");
    assert!(!incomplete, "the fixture metadata prefix is complete");
    let metadata = recovered
        .metadata
        .as_ref()
        .expect("accepted metadata exists");
    estimate_job(fixture.operation_id, &head, metadata, selected)
}

fn lifecycle_shape_for_current(fixture: &Fixture) -> BrokerCheckpointV3 {
    let job = fixture.journal.load(fixture.operation_id).unwrap().unwrap();
    let broker = job.broker_checkpoint_v2().cloned().unwrap_or_else(|| {
        BrokerCheckpointV2::new(
            job.checkpoint().clone(),
            SupervisionCheckpointV1::new(
                fixture.journal.scope().control_namespace_uuid(),
                Vec::new(),
                Vec::new(),
            )
            .unwrap(),
        )
        .unwrap()
    });
    let commands = job
        .broker_checkpoint_v3()
        .map_or_else(Vec::new, |checkpoint| checkpoint.commands().to_vec());
    BrokerCheckpointV3::new(broker, commands).unwrap()
}

fn encoded_max_lifecycle_bytes(fixture: &Fixture, lifecycle: &BrokerCheckpointV3) -> u64 {
    let job = fixture.journal.load(fixture.operation_id).unwrap().unwrap();
    let intent_hash = job.intent_hash().to_owned();
    let expected = RevisionRefV1::new(u64::MAX - 1, job.head().hash()).unwrap();
    let (bytes, next) = crate::broker::journal::encode_checkpoint(
        &fixture.journal.0,
        fixture.operation_id,
        &intent_hash,
        &expected,
        &crate::broker::journal::LoadedCheckpoint::V3(lifecycle.clone()),
    )
    .unwrap();
    assert_eq!(next.revision(), u64::MAX);
    bytes.len() as u64
}

fn maximum_limits() -> JournalLimitsV1 {
    JournalLimitsV1::new(
        8 * 1024 * 1024 * 1024,
        131_072,
        65_536,
        1024 * 1024 * 1024 * 1024,
        4_194_304,
        16_384,
    )
    .unwrap()
}

#[test]
fn estimator_sizes_real_v4_metadata_and_tracks_decimal_revision_widths_without_mutation() {
    let fixture = Fixture::new();
    let before = fixture.journal.load(fixture.operation_id).unwrap().unwrap();
    let original_head = before.head().clone();
    let actual_v4 = before.broker_checkpoint_v4().unwrap();
    assert_eq!(actual_v4.events().len(), 2);
    assert!(
        actual_v4
            .events()
            .iter()
            .all(|event| event.audit_seq().is_none())
    );

    let selected = estimate(&fixture, true);
    let maximum_events: Vec<_> = actual_v4
        .events()
        .iter()
        .map(|event| event.acknowledged(i64::MAX).unwrap())
        .collect();
    let maximum_outbox_bytes = super::super::canonical(&maximum_events).unwrap().len() as u64;
    assert_eq!(selected.outbox_bytes, Some(maximum_outbox_bytes));
    assert_eq!(selected.remaining_cancels, 64);
    assert_eq!(
        selected.completion_bytes,
        selected
            .checkpoint_bytes
            .checked_add(
                selected
                    .remaining_cancels
                    .checked_mul(selected.cancel_delta_bytes)
                    .unwrap(),
            )
            .and_then(|bytes| bytes.checked_add(selected.pointer_bytes))
            .unwrap()
    );

    let intent_hash = intent_for(fixture.operation_id).intent_hash().unwrap();
    let maximum_v4 = BrokerCheckpointV4::new(actual_v4.lifecycle().clone(), maximum_events)
        .expect("the maximum-width V4 copy remains a valid sizing shape");
    let expected = RevisionRefV1::new(u64::MAX - 1, original_head.hash()).unwrap();
    let (encoded_checkpoint, encoded_head) = crate::broker::journal::encode_checkpoint(
        &fixture.journal.0,
        fixture.operation_id,
        &intent_hash,
        &expected,
        &crate::broker::journal::LoadedCheckpoint::V4(maximum_v4),
    )
    .unwrap();
    assert_eq!(encoded_head.revision(), u64::MAX);
    assert_eq!(
        encoded_checkpoint.len() as u64,
        selected.checkpoint_bytes,
        "the counted maximum V4 envelope matches production encoding at 20-digit revisions"
    );

    let widths = [9, 10, 99, 100].map(|revision| {
        serialized_current_pointer_bytes(
            fixture.operation_id,
            &intent_hash,
            RevisionRefV1::new(revision, original_head.hash()).unwrap(),
        )
    });
    assert_eq!(widths[1], widths[0] + 1);
    assert_eq!(widths[3], widths[2] + 1);
    let widest_pointer = serialized_current_pointer_bytes(
        fixture.operation_id,
        &intent_hash,
        RevisionRefV1::new(u64::MAX, original_head.hash()).unwrap(),
    );
    assert!(
        widths.iter().all(|bytes| *bytes <= selected.pointer_bytes)
            && widest_pointer <= selected.pointer_bytes,
        "the persisted pointer grows with revision width and stays within its maximum bound"
    );
    assert_eq!(
        fixture
            .journal
            .load(fixture.operation_id)
            .unwrap()
            .unwrap()
            .head(),
        &original_head,
        "the sizing copies never modify the persisted chain"
    );
}

fn serialized_current_pointer_bytes(
    operation_id: Uuid,
    intent_hash: &str,
    head: RevisionRefV1,
) -> u64 {
    let current = crate::broker::journal::Current {
        version: 1,
        operation_id,
        intent_hash: intent_hash.to_owned(),
        head,
    };
    crate::broker::journal::canonical(&current).unwrap().len() as u64
}

#[test]
fn v1_and_v2_metadata_cancel_envelopes_fit_the_counted_growth_bound() {
    for start_supervised in [false, true] {
        let (mut fixture, _intent, _initial) = Fixture::accepted_v1();
        if start_supervised {
            fixture.advance_to_supervised();
        }

        let bound = estimate_result(&fixture, false).unwrap();
        let base_shape = lifecycle_shape_for_current(&fixture);
        assert_eq!(
            encoded_max_lifecycle_bytes(&fixture, &base_shape),
            bound.checkpoint_bytes,
            "the counted V3-shaped baseline matches production encoding"
        );

        for cancel_count in 1..=3_u64 {
            fixture
                .stop_journal()
                .append_cancel(fixture.operation_id, Uuid::new_v4())
                .unwrap();
            let job = fixture.journal.load(fixture.operation_id).unwrap().unwrap();
            let lifecycle = job
                .broker_checkpoint_v3()
                .expect("a real Cancel upgrades V1/V2 metadata to V3")
                .clone();
            assert_eq!(lifecycle.commands().len(), cancel_count as usize);
            let actual = encoded_max_lifecycle_bytes(&fixture, &lifecycle);
            let allowed = bound
                .checkpoint_bytes
                .checked_add(cancel_count.checked_mul(bound.cancel_delta_bytes).unwrap())
                .unwrap();
            assert!(
                actual <= allowed,
                "real {cancel_count}-Cancel envelope {actual} exceeds counted bound {allowed}"
            );
            fixture.head = job.head().clone();
        }
    }
}

#[test]
fn capacity_check_enforces_exact_scope_job_entry_byte_and_revision_margins() {
    let fixture = Fixture::new();
    let usage = inventory_usage(&fixture.journal.0).unwrap();
    let job_usage = usage
        .jobs
        .get(&fixture.operation_id)
        .expect("physical inventory contains selected job");
    let bound = estimate(&fixture, true);
    let cancels = bound.remaining_cancels;
    let per_cancel = bound
        .checkpoint_bytes
        .checked_add(bound.pointer_bytes)
        .unwrap();
    let cancel_reserve = cancels
        .checked_mul(per_cancel)
        .and_then(|value| {
            cancels
                .checked_mul(cancels + 1)
                .and_then(|triangle| triangle.checked_mul(bound.cancel_delta_bytes))
                .and_then(|triangle| value.checked_add(triangle / 2))
        })
        .unwrap();
    let scope_bytes_required = usage
        .bytes
        .checked_add(cancel_reserve)
        .and_then(|value| value.checked_add(bound.completion_bytes))
        .unwrap();
    let job_bytes_required = job_usage
        .bytes
        .checked_add(cancel_reserve)
        .and_then(|value| value.checked_add(bound.completion_bytes))
        .unwrap();
    let scope_entries_required = usage
        .entries
        .checked_add(u32::try_from(2 * cancels + 2).unwrap())
        .unwrap();
    let job_entries_required = job_usage
        .entries
        .checked_add(u32::try_from(2 * cancels + 2).unwrap())
        .unwrap();
    let revisions_required = u32::try_from(bound.head.revision() + cancels + 2).unwrap();
    assert!(job_bytes_required < scope_bytes_required);
    assert!(job_entries_required < scope_entries_required);
    assert!(revisions_required <= job_entries_required);

    let exact = JournalLimitsV1::new(
        job_bytes_required,
        job_entries_required,
        revisions_required,
        scope_bytes_required,
        scope_entries_required,
        u32::try_from(usage.jobs.len()).unwrap(),
    )
    .unwrap();
    assert!(check_capacity(&exact, &usage, std::slice::from_ref(&bound)).is_ok());

    let fail_scope_bytes = JournalLimitsV1::new(
        job_bytes_required,
        job_entries_required,
        revisions_required,
        scope_bytes_required - 1,
        scope_entries_required,
        u32::try_from(usage.jobs.len()).unwrap(),
    )
    .unwrap();
    let fail_job_bytes = JournalLimitsV1::new(
        job_bytes_required - 1,
        job_entries_required,
        revisions_required,
        scope_bytes_required,
        scope_entries_required,
        u32::try_from(usage.jobs.len()).unwrap(),
    )
    .unwrap();
    let fail_scope_entries = JournalLimitsV1::new(
        job_bytes_required,
        job_entries_required,
        revisions_required,
        scope_bytes_required,
        scope_entries_required - 1,
        u32::try_from(usage.jobs.len()).unwrap(),
    )
    .unwrap();
    let fail_job_entries = JournalLimitsV1::new(
        job_bytes_required,
        job_entries_required - 1,
        revisions_required,
        scope_bytes_required,
        scope_entries_required,
        u32::try_from(usage.jobs.len()).unwrap(),
    )
    .unwrap();
    let fail_revisions = JournalLimitsV1::new(
        job_bytes_required,
        job_entries_required,
        revisions_required - 1,
        scope_bytes_required,
        scope_entries_required,
        u32::try_from(usage.jobs.len()).unwrap(),
    )
    .unwrap();

    for (name, limits) in [
        ("scope bytes", fail_scope_bytes),
        ("job bytes", fail_job_bytes),
        ("scope entries", fail_scope_entries),
        ("job entries", fail_job_entries),
        ("revision margin", fail_revisions),
    ] {
        assert_eq!(
            check_capacity(&limits, &usage, std::slice::from_ref(&bound)),
            Err(JournalError::LimitExceeded),
            "one-unit-under {name} must refuse preparation"
        );
    }
    assert!(
        estimate_result(&fixture, false).is_ok(),
        "an unselected job remains estimable without reserving completion margin"
    );
}

#[test]
fn selected_ack_width_is_refused_when_the_real_outbox_is_at_its_nested_limit() {
    let mut fixture = Fixture::new();
    fixture.grow_outbox_to_before_nested_limit();
    let job = fixture.journal.load(fixture.operation_id).unwrap().unwrap();
    let checkpoint = job.broker_checkpoint_v4().unwrap();
    let current_bytes = super::super::canonical(&checkpoint.events()).unwrap().len();
    assert!(current_bytes < MAX_OUTBOX_BYTES);
    assert!(checkpoint.events().len() <= 512);
    drop(job);

    let current = estimate(&fixture, false);
    assert_eq!(current.outbox_bytes, Some(current_bytes as u64));
    match estimate_result(&fixture, true) {
        Err(JournalError::TooLarge | JournalError::LimitExceeded) => {}
        Err(error) => panic!("unexpected nested-size refusal: {error}"),
        Ok(selected) => {
            assert!(
                selected.outbox_bytes.unwrap() > current_bytes as u64,
                "the two real pending null audit_seq values are sized at i64::MAX"
            );
            assert!(
                matches!(
                    check_capacity(
                        &maximum_limits(),
                        &inventory_usage(&fixture.journal.0).unwrap(),
                        &[selected]
                    ),
                    Err(JournalError::TooLarge | JournalError::LimitExceeded)
                ),
                "nested outbox cap must be checked even when physical margins are ample"
            );
        }
    }
}

#[test]
fn cold_reservation_is_unique_and_drop_releases_the_same_metadata_reserve() {
    let fixture = Fixture::new();
    let stops = fixture.stop_journal();
    let prepared = prepare(&stops, fixture.operation_id).unwrap();
    let original_head = prepared.head().clone();
    let original_ids: Vec<_> = prepared
        .pending_events()
        .iter()
        .map(OutboxEventV1::event_id)
        .collect();
    assert_eq!(prepared.operation_id(), fixture.operation_id);
    assert_eq!(
        prepared.scope().installation_id(),
        fixture.journal.scope().installation_id()
    );
    assert_eq!(
        prepared.scope().tenant_id(),
        fixture.journal.scope().tenant_id()
    );
    assert_eq!(
        prepared.scope().gateway_origin(),
        fixture.journal.scope().gateway_origin()
    );
    assert_eq!(
        prepared.scope().control_namespace_uuid(),
        fixture.journal.scope().control_namespace_uuid()
    );
    assert_eq!(
        prepared.scope().profile_id(),
        fixture.journal.scope().profile_id()
    );
    assert_eq!(original_ids.len(), 2);
    assert_eq!(
        prepare(&stops, fixture.operation_id).err(),
        Some(JournalError::Busy),
        "one logical reservation may be active for the actor"
    );

    drop(prepared);
    let retry = prepare(&stops, fixture.operation_id).unwrap();
    assert_eq!(retry.head(), &original_head);
    assert_eq!(
        retry
            .pending_events()
            .iter()
            .map(OutboxEventV1::event_id)
            .collect::<Vec<_>>(),
        original_ids,
        "dropping the preparation releases only capacity, not or changes metadata"
    );
}

#[test]
fn scope_partition_counts_closed_rejections_and_refuses_an_incomplete_sibling_until_closed() {
    let closed = Fixture::new();
    let rejected_id = Uuid::new_v4();
    let ticket = match closed
        .journal
        .reserve(rejected_id, br#"{"fixture":"closed-rejection"}"#)
        .unwrap()
    {
        ReserveResult::New(ticket) => ticket,
        other => panic!("fresh sibling reservation expected, got {other:?}"),
    };
    closed
        .journal
        .reject(
            ticket,
            RejectionPhaseV1::SubmissionPreparation,
            RejectionCodeV1::PreparationFailed,
        )
        .unwrap();
    let rejected_scope = closed.stop_journal();
    let prepared = prepare(&rejected_scope, closed.operation_id)
        .expect("a strictly validated closed rejection is included in the scope partition");
    assert_eq!(prepared.operation_id(), closed.operation_id);
    assert_eq!(prepared.pending_events().len(), 2);
    drop(prepared);

    let incomplete = Fixture::new();
    let incomplete_id = Uuid::new_v4();
    let ticket = match incomplete
        .journal
        .reserve(incomplete_id, br#"{"fixture":"incomplete-sibling"}"#)
        .unwrap()
    {
        ReserveResult::New(ticket) => ticket,
        other => panic!("fresh sibling reservation expected, got {other:?}"),
    };
    let incomplete_scope = incomplete.stop_journal();
    assert_eq!(
        prepare(&incomplete_scope, incomplete.operation_id).err(),
        Some(JournalError::Interrupted),
        "an in-process reservation cannot be classified as a closed rejection"
    );
    incomplete
        .journal
        .reject(
            ticket,
            RejectionPhaseV1::SubmissionPreparation,
            RejectionCodeV1::PreparationFailed,
        )
        .unwrap();
    let prepared = prepare(&incomplete_scope, incomplete.operation_id)
        .expect("closing the sibling rejection leaves no leaked reservation token");
    assert_eq!(prepared.operation_id(), incomplete.operation_id);
    assert_eq!(prepared.pending_events().len(), 2);
}

#[test]
fn retained_capacity_does_not_drop_selected_or_sibling_cancels() {
    let selected = Fixture::new();
    let sibling_id = Uuid::new_v4();
    let intent = intent_for(sibling_id);
    let initial = CheckpointPayloadV1::initial(&intent).unwrap();
    let ReserveResult::New(ticket) = selected
        .journal
        .reserve(sibling_id, br#"{"fixture":"cold-audit-sibling"}"#)
        .unwrap()
    else {
        panic!("fresh sibling must reserve a new operation");
    };
    let sibling_accepted = selected
        .journal
        .accept(ticket, &intent, BindingInputs::None, &initial)
        .unwrap();
    let sibling_scope = selected.journal.scope().clone();
    let sibling_v2 = BrokerCheckpointV2::new(
        initial,
        SupervisionCheckpointV1::new(
            sibling_scope.control_namespace_uuid(),
            Vec::new(),
            Vec::new(),
        )
        .unwrap(),
    )
    .unwrap();
    let sibling_v3 = BrokerCheckpointV3::new(sibling_v2, Vec::new()).unwrap();
    let sibling_lifecycle = selected
        .journal
        .advance_lifecycle(sibling_id, sibling_accepted.head().clone(), &sibling_v3)
        .unwrap();
    let sibling_events = vec![
        outbox_event(
            sibling_id,
            &intent,
            "observer.coverage",
            json!({"sibling": true}),
        ),
        outbox_event(
            sibling_id,
            &intent,
            "broker.batch.result",
            json!({"sibling": true}),
        ),
    ];
    let sibling_v4 = BrokerCheckpointV4::new(sibling_v3, sibling_events).unwrap();
    selected
        .journal
        .advance_outbox(sibling_id, sibling_lifecycle.head().clone(), &sibling_v4)
        .unwrap();

    let stops = selected.stop_journal();
    let prepared = prepare(&stops, selected.operation_id).unwrap();
    let selected_head = prepared.head().clone();
    let sibling_head = selected
        .journal
        .load(sibling_id)
        .unwrap()
        .unwrap()
        .head()
        .clone();
    let selected_cancel = Uuid::new_v4();
    let sibling_cancel = Uuid::new_v4();

    let selected_stops = stops.clone();
    let sibling_stops = stops.clone();
    let selected_append = std::thread::spawn(move || {
        selected_stops.append_cancel(selected.operation_id, selected_cancel)
    });
    let sibling_append =
        std::thread::spawn(move || sibling_stops.append_cancel(sibling_id, sibling_cancel));
    selected_append
        .join()
        .unwrap()
        .expect("the selected job can consume its pre-counted Cancel margin");
    sibling_append
        .join()
        .unwrap()
        .expect("a sibling Cancel remains independently publishable");

    let selected_after = selected
        .journal
        .load(selected.operation_id)
        .unwrap()
        .unwrap();
    let sibling_after = selected.journal.load(sibling_id).unwrap().unwrap();
    assert_eq!(
        selected_after.head().revision(),
        selected_head.revision() + 1
    );
    assert_eq!(sibling_after.head().revision(), sibling_head.revision() + 1);
    assert_eq!(
        selected_after
            .broker_checkpoint_v3()
            .unwrap()
            .commands()
            .len(),
        1
    );
    assert_eq!(
        sibling_after
            .broker_checkpoint_v3()
            .unwrap()
            .commands()
            .len(),
        1
    );
    assert_eq!(prepared.head(), &selected_head);
    assert_eq!(prepared.pending_events().len(), 2);
    assert_eq!(
        selected_after
            .broker_checkpoint_v4()
            .unwrap()
            .events()
            .len(),
        2
    );
    assert_eq!(
        sibling_after.broker_checkpoint_v4().unwrap().events().len(),
        2
    );
}
