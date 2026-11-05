//! Regressions for the exact, metadata-only V4 completion merge.
//!
//! Local HTTP receipts in these tests are response-integrity inputs only. The
//! loopback peer and its SQLite Store are test-owned; they do not establish a
//! production control-plane identity or durability guarantee.

use super::*;
use crate::{
    audit::Store,
    broker::journal::{BindingInputs, JournalError, ReserveResult},
    model::{Event, StoredEvent},
    redaction::Redactor,
    runner::{AuditClient, CheckpointPayloadV1, ColdAuditBudget, ExactStoredEventReceipt},
};
use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use std::{
    fs,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    sync::Arc,
};
use tokio::{net::TcpListener, task::JoinHandle};
use uuid::Uuid;

#[derive(Clone)]
struct PeerState {
    store: Option<Arc<Store>>,
    fixed_seq: Option<i64>,
}

struct ReceiptPeer {
    _directory: tempfile::TempDir,
    origin: String,
    task: JoinHandle<()>,
}

impl Drop for ReceiptPeer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl ReceiptPeer {
    async fn sqlite() -> Self {
        Self::start(true, None).await
    }

    async fn repeated_sequence(seq: i64) -> Self {
        Self::start(false, Some(seq)).await
    }

    async fn start(with_store: bool, fixed_seq: Option<i64>) -> Self {
        let directory = tempfile::Builder::new()
            .prefix("cold-audit-completion-")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .expect("private receipt peer fixture");
        let store = with_store.then(|| {
            Arc::new(
                Store::open_for_tenant(
                    &directory.path().join("tenant.sqlite"),
                    "cold-audit-completion-test",
                    Redactor::new(Vec::new()),
                    256,
                )
                .expect("test-owned local SQLite Store opens"),
            )
        });
        let state = PeerState {
            store: store.clone(),
            fixed_seq,
        };
        let app = Router::new()
            .route("/api/v1/events", post(receive_event))
            .with_state(state);
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test-owned loopback HTTP peer");
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("test-owned HTTP peer remains available");
        });
        Self {
            _directory: directory,
            origin,
            task,
        }
    }

    fn client(&self) -> AuditClient {
        AuditClient::new(&self.origin, "cold-audit-completion-test-token")
            .expect("loopback AuditClient configuration is valid")
    }

    async fn submit(&self, events: &[Event]) -> Vec<ExactStoredEventReceipt> {
        let client = self.client();
        let budget = ColdAuditBudget::start();
        let mut receipts = Vec::with_capacity(events.len());
        for event in events {
            receipts.push(
                client
                    .post_cold_audit_event(event, &budget)
                    .await
                    .expect("local HTTP peer returns an exact event receipt"),
            );
        }
        receipts
    }
}

async fn receive_event(State(state): State<PeerState>, body: Bytes) -> Response {
    let event = match serde_json::from_slice::<Event>(&body) {
        Ok(event) => event,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    let stored = if let Some(store) = &state.store {
        match store.append(event) {
            Ok(stored) => stored,
            Err(_) => return StatusCode::BAD_REQUEST.into_response(),
        }
    } else {
        StoredEvent {
            seq: state.fixed_seq.unwrap_or(33),
            received_at: event.timestamp.clone(),
            prev_hash: "a".repeat(64),
            hash: "b".repeat(64),
            event,
        }
    };
    (StatusCode::OK, Json(stored)).into_response()
}

fn complete(
    stops: &StopJournal,
    prepared: &PreparedColdAudit,
    receipts: &[ExactStoredEventReceipt],
) -> JournalResult<super::super::completion::ConfirmedColdAudit> {
    super::super::completion::complete(stops, prepared, receipts)
}

fn current_head(fixture: &Fixture) -> RevisionRefV1 {
    fixture
        .journal
        .load(fixture.operation_id)
        .unwrap()
        .expect("accepted fixture remains present")
        .head()
        .clone()
}

fn pending_events(prepared: &PreparedColdAudit) -> Vec<Event> {
    prepared
        .pending_events()
        .iter()
        .map(|entry| entry.event().clone())
        .collect()
}

fn add_accepted_sibling(fixture: &Fixture) -> Uuid {
    let operation_id = Uuid::new_v4();
    let intent = super::intent_for(operation_id);
    let initial = CheckpointPayloadV1::initial(&intent).unwrap();
    let ReserveResult::New(ticket) = fixture
        .journal
        .reserve(operation_id, br#"{"fixture":"completion-sibling"}"#)
        .unwrap()
    else {
        panic!("fresh sibling should obtain a new reservation");
    };
    fixture
        .journal
        .accept(ticket, &intent, BindingInputs::None, &initial)
        .unwrap();
    operation_id
}

struct SiblingBackup {
    original: PathBuf,
    saved: PathBuf,
    restored: bool,
}

impl SiblingBackup {
    fn move_out_of_scope(original: PathBuf, saved: PathBuf) -> Self {
        fs::rename(&original, &saved)
            .expect("move the real sibling job outside the journal inventory without deleting it");
        Self {
            original,
            saved,
            restored: false,
        }
    }

    fn restore(&mut self) {
        fs::rename(&self.saved, &self.original)
            .expect("restore the sibling job after the scope-mutation check");
        self.restored = true;
    }
}

impl Drop for SiblingBackup {
    fn drop(&mut self) {
        if !self.restored {
            let _ = fs::rename(&self.saved, &self.original);
        }
    }
}

struct SiblingRevisionRollback {
    current: PathBuf,
    revision: PathBuf,
    saved_current: PathBuf,
    saved_revision: PathBuf,
    replacement_created: bool,
    current_saved: bool,
    revision_saved: bool,
    restored: bool,
}

impl SiblingRevisionRollback {
    fn install(
        current: PathBuf,
        revision: PathBuf,
        original_current: &[u8],
        backup_directory: PathBuf,
    ) -> Self {
        fs::create_dir(&backup_directory).expect("private rollback backup directory is new");
        fs::set_permissions(&backup_directory, fs::Permissions::from_mode(0o700))
            .expect("rollback backup directory is private");
        let saved_current = backup_directory.join("current.json");
        let saved_revision =
            backup_directory.join(revision.file_name().expect("revision has a filename"));
        let mut rollback = Self {
            current,
            revision,
            saved_current,
            saved_revision,
            replacement_created: false,
            current_saved: false,
            revision_saved: false,
            restored: false,
        };

        fs::rename(&rollback.current, &rollback.saved_current)
            .expect("retain the post-cancel current pointer in the private backup");
        rollback.current_saved = true;
        fs::rename(&rollback.revision, &rollback.saved_revision)
            .expect("retain the post-cancel revision in the private backup");
        rollback.revision_saved = true;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&rollback.current)
            .expect("restore the prior current pointer with exclusive private creation");
        rollback.replacement_created = true;
        file.write_all(original_current)
            .expect("write the exact previously captured current pointer");
        file.sync_all()
            .expect("make the test-only rollback pointer durable");
        rollback
    }

    fn restore(&mut self) {
        if self.replacement_created {
            fs::remove_file(&self.current)
                .expect("remove only the test-created rollback pointer before restoration");
            self.replacement_created = false;
        }
        if self.current_saved {
            fs::rename(&self.saved_current, &self.current)
                .expect("restore the original post-cancel current pointer");
            self.current_saved = false;
        }
        if self.revision_saved {
            fs::rename(&self.saved_revision, &self.revision)
                .expect("restore the original post-cancel revision file");
            self.revision_saved = false;
        }
        self.restored = true;
    }
}

impl Drop for SiblingRevisionRollback {
    fn drop(&mut self) {
        if self.restored {
            return;
        }
        if self.replacement_created {
            let _ = fs::remove_file(&self.current);
        }
        if self.current_saved {
            let _ = fs::rename(&self.saved_current, &self.current);
        }
        if self.revision_saved {
            let _ = fs::rename(&self.saved_revision, &self.revision);
        }
    }
}

#[tokio::test]
async fn duplicate_or_scope_changed_receipts_conflict_without_publishing_a_head() {
    let duplicate_fixture = Fixture::new();
    let duplicate_stops = duplicate_fixture.stop_journal();
    let duplicate_prepared =
        super::super::prepare(&duplicate_stops, duplicate_fixture.operation_id).unwrap();
    let duplicate_events = pending_events(&duplicate_prepared);
    assert_eq!(duplicate_events.len(), 2);
    let same_seq_peer = ReceiptPeer::repeated_sequence(33).await;
    let same_seq_receipts = same_seq_peer.submit(&duplicate_events).await;
    let duplicate_before = current_head(&duplicate_fixture);
    let duplicate_result =
        complete(&duplicate_stops, &duplicate_prepared, &same_seq_receipts).map(|_| ());
    let duplicate_unchanged = current_head(&duplicate_fixture) == duplicate_before;

    let mut partial_fixture = Fixture::new();
    let loaded = partial_fixture
        .journal
        .load(partial_fixture.operation_id)
        .unwrap()
        .unwrap();
    let baseline = loaded.broker_checkpoint_v4().unwrap();
    assert_eq!(baseline.events().len(), 2);
    let mut events = baseline.events().to_vec();
    events[0] = events[0].acknowledged(33).unwrap();
    let partial = BrokerCheckpointV4::new(baseline.lifecycle().clone(), events).unwrap();
    let advanced = partial_fixture
        .journal
        .advance_outbox(
            partial_fixture.operation_id,
            loaded.head().clone(),
            &partial,
        )
        .unwrap();
    partial_fixture.head = advanced.head().clone();
    drop(loaded);
    let partial_stops = partial_fixture.stop_journal();
    let partial_prepared =
        super::super::prepare(&partial_stops, partial_fixture.operation_id).unwrap();
    assert_eq!(partial_prepared.pending_events().len(), 1);
    let partial_events = pending_events(&partial_prepared);
    let colliding_peer = ReceiptPeer::repeated_sequence(33).await;
    let collision_receipts = colliding_peer.submit(&partial_events).await;
    let partial_before = current_head(&partial_fixture);
    let collision_result =
        complete(&partial_stops, &partial_prepared, &collision_receipts).map(|_| ());
    let partial_unchanged = current_head(&partial_fixture) == partial_before;

    let scope_fixture = Fixture::new();
    let sibling = add_accepted_sibling(&scope_fixture);
    let scope_stops = scope_fixture.stop_journal();
    let scope_prepared = super::super::prepare(&scope_stops, scope_fixture.operation_id).unwrap();
    let scope_events = pending_events(&scope_prepared);
    let scope_peer = ReceiptPeer::sqlite().await;
    let scope_receipts = scope_peer.submit(&scope_events).await;
    let sibling_path = scope_fixture
        ._root
        .path()
        .join("broker-journal-v1")
        .join(&scope_fixture.journal.0.scope_hash)
        .join("jobs")
        .join(sibling.to_string());
    let saved_sibling = scope_fixture
        ._root
        .path()
        .join(format!("cold-audit-sibling-backup-{}", Uuid::new_v4()));
    let scope_before = current_head(&scope_fixture);
    let mut sibling_backup = SiblingBackup::move_out_of_scope(sibling_path, saved_sibling);
    let scope_result = complete(&scope_stops, &scope_prepared, &scope_receipts).map(|_| ());
    sibling_backup.restore();
    let scope_unchanged = current_head(&scope_fixture) == scope_before;

    let outcomes = [
        (
            "two receipt IDs share one central sequence",
            duplicate_result,
            duplicate_unchanged,
        ),
        (
            "pending result reuses acknowledged coverage sequence",
            collision_result,
            partial_unchanged,
        ),
        (
            "prepared sibling disappeared from scope inventory",
            scope_result,
            scope_unchanged,
        ),
    ];
    assert!(
        outcomes
            .iter()
            .all(|(_, result, unchanged)| *result == Err(JournalError::Conflict) && *unchanged),
        "completion must reject each inconsistent receipt/scope view before publishing: {outcomes:?}"
    );
}

#[tokio::test]
async fn exact_batch_preserves_cancel_and_replay_does_not_publish_another_revision() {
    let fixture = Fixture::new();
    let stops = fixture.stop_journal();
    let prepared = super::super::prepare(&stops, fixture.operation_id).unwrap();
    let events = pending_events(&prepared);
    assert_eq!(events.len(), 2);
    let prepared_head = current_head(&fixture);

    stops
        .append_cancel(fixture.operation_id, Uuid::new_v4())
        .unwrap();
    let head_with_cancel = current_head(&fixture);
    assert_eq!(head_with_cancel.revision(), prepared_head.revision() + 1);

    let peer = ReceiptPeer::sqlite().await;
    let receipts = peer.submit(&events).await;
    assert_eq!(receipts.len(), 2);
    let first = complete(&stops, &prepared, &receipts).unwrap();
    let after_first = current_head(&fixture);
    assert_eq!(after_first.revision(), head_with_cancel.revision() + 1);
    assert_eq!(first.operation_id(), fixture.operation_id);
    assert_eq!(first.event_ids().len(), 2);
    assert_eq!(first.head(), &after_first);

    let job = fixture.journal.load(fixture.operation_id).unwrap().unwrap();
    let stored = job.broker_checkpoint_v4().unwrap();
    let by_id = stored
        .events()
        .iter()
        .map(|entry| (entry.event_id(), entry.audit_seq()))
        .collect::<std::collections::HashMap<_, _>>();
    for receipt in &receipts {
        let event_id = Uuid::parse_str(&receipt.stored_event().event.id).unwrap();
        assert_eq!(by_id.get(&event_id), Some(&Some(receipt.seq())));
    }
    assert_eq!(
        stored.lifecycle().commands().len(),
        1,
        "the Cancel remains present"
    );

    let replay = complete(&stops, &prepared, &receipts).unwrap();
    assert_eq!(replay.head(), &after_first);
    assert_eq!(
        current_head(&fixture),
        after_first,
        "exact replay is revision-idempotent"
    );

    let changed_lot = complete(&stops, &prepared, &receipts[..1]);
    assert_eq!(changed_lot.err(), Some(JournalError::Conflict));
    assert_eq!(
        current_head(&fixture),
        after_first,
        "a different receipt lot cannot publish"
    );
}

#[tokio::test]
async fn partial_receipt_lot_is_exactly_replayable_but_cannot_be_replaced() {
    let fixture = Fixture::new();
    let stops = fixture.stop_journal();
    let prepared = super::super::prepare(&stops, fixture.operation_id).unwrap();
    let events = pending_events(&prepared);
    assert_eq!(events.len(), 2);
    let peer = ReceiptPeer::sqlite().await;

    let coverage = events
        .iter()
        .find(|event| event.kind == "observer.coverage")
        .expect("the selected pair contains one coverage event")
        .clone();
    let coverage_receipt = peer.submit(std::slice::from_ref(&coverage)).await;
    let before = current_head(&fixture);
    let partial = complete(&stops, &prepared, &coverage_receipt).unwrap();
    let after_partial = current_head(&fixture);
    assert_eq!(after_partial.revision(), before.revision() + 1);
    assert_eq!(partial.head(), &after_partial);

    let snapshot = fixture.journal.load(fixture.operation_id).unwrap().unwrap();
    let outbox = snapshot.broker_checkpoint_v4().unwrap();
    let coverage_entry = outbox
        .events()
        .iter()
        .find(|entry| entry.event().kind == "observer.coverage")
        .unwrap();
    let result_entry = outbox
        .events()
        .iter()
        .find(|entry| entry.event().kind == "broker.batch.result")
        .unwrap();
    assert_eq!(coverage_entry.audit_seq(), Some(coverage_receipt[0].seq()));
    assert_eq!(
        result_entry.audit_seq(),
        None,
        "partial completion leaves result pending"
    );

    let replay = complete(&stops, &prepared, &coverage_receipt).unwrap();
    assert_eq!(replay.head(), &after_partial);
    assert_eq!(current_head(&fixture), after_partial);

    let result = events
        .iter()
        .find(|event| event.kind == "broker.batch.result")
        .expect("the selected pair contains one result event")
        .clone();
    let different_lot = peer.submit(std::slice::from_ref(&result)).await;
    let rejected = complete(&stops, &prepared, &different_lot);
    assert_eq!(rejected.err(), Some(JournalError::Conflict));
    assert_eq!(current_head(&fixture), after_partial);
}

/// The fault happens after the real checkpoint/current publication and before
/// the local confirmation response. This checks only the metadata primitive;
/// it does not prove an owning actor's quarantine or recovery protocol.
#[tokio::test]
async fn before_ack_fault_keeps_the_exact_lot_reserved_and_replay_is_revision_idempotent() {
    let fixture = Fixture::new();
    let stops = fixture.stop_journal();
    let prepared = super::super::prepare(&stops, fixture.operation_id).unwrap();
    let events = pending_events(&prepared);
    assert_eq!(events.len(), 2);
    let peer = ReceiptPeer::sqlite().await;
    let receipts = peer.submit(&events).await;
    let before = current_head(&fixture);

    crate::broker::journal::tests::fail_once_at(crate::broker::journal::FaultStage::BeforeAck);
    let interrupted = complete(&stops, &prepared, &receipts);
    assert_eq!(interrupted.err(), Some(JournalError::Io));
    let after_publication = current_head(&fixture);
    assert_eq!(after_publication.revision(), before.revision() + 1);
    assert_eq!(
        super::super::prepare(&stops, fixture.operation_id).err(),
        Some(JournalError::Busy),
        "the same prepared reservation remains live after the fault"
    );

    let replay = complete(&stops, &prepared, &receipts).unwrap();
    assert_eq!(replay.head(), &after_publication);
    assert_eq!(current_head(&fixture), after_publication);
}

#[tokio::test]
async fn completion_rejects_a_sibling_current_pointer_rolled_back_below_its_retained_floor() {
    let fixture = Fixture::new();
    let sibling = add_accepted_sibling(&fixture);
    let sibling_root = fixture
        ._root
        .path()
        .join("broker-journal-v1")
        .join(&fixture.journal.0.scope_hash)
        .join("jobs")
        .join(sibling.to_string());
    let current = sibling_root.join("current.json");
    let initial_current = fs::read(&current).expect("the accepted sibling has a real pointer");
    let initial_sibling = fixture
        .journal
        .load(sibling)
        .unwrap()
        .expect("the sibling accepted V1 operation is real");
    let head_zero = initial_sibling.head().clone();
    assert_eq!(head_zero.revision(), 0);
    drop(initial_sibling);

    let stops = fixture.stop_journal();
    stops.append_cancel(sibling, Uuid::new_v4()).unwrap();
    let after_cancel = fixture.journal.load(sibling).unwrap().unwrap();
    let head_one = after_cancel.head().clone();
    assert_eq!(head_one.revision(), head_zero.revision() + 1);
    drop(after_cancel);
    let revision_one = sibling_root.join(format!("rev-{:020}.json", head_one.revision()));
    let post_cancel_revision = fs::read(&revision_one).expect("Cancel produced its real revision");
    let pointer_after_cancel = fs::read(&current).expect("Cancel published current.json");
    assert_ne!(pointer_after_cancel, initial_current);
    assert!(!post_cancel_revision.is_empty());

    let prepared = super::super::prepare(&stops, fixture.operation_id).unwrap();
    let selected_events = pending_events(&prepared);
    let peer = ReceiptPeer::sqlite().await;
    let receipts = peer.submit(&selected_events).await;
    let selected_head_before = current_head(&fixture);
    let backup_directory = fixture
        ._root
        .path()
        .join(format!("cold-audit-sibling-rollback-{}", Uuid::new_v4()));
    assert_eq!(backup_directory.parent(), Some(fixture._root.path()));

    let mut rollback = SiblingRevisionRollback::install(
        current.clone(),
        revision_one.clone(),
        &initial_current,
        backup_directory,
    );
    let reader = super::super::super::StopRecoveryReaderV1::from_core(&fixture.journal.0).unwrap();
    let mut raw_usage = crate::broker::journal::Usage::default();
    let mut raw_blobs = super::super::super::RecoveryBlobBytes::default();
    let (raw_job, raw_rejected, raw_partial) = reader
        .read_job(sibling, &mut raw_usage, &mut raw_blobs)
        .expect("the valid old sibling prefix remains independently readable");
    let raw_head = raw_job.map(|job| job.head.clone());

    let result = complete(&stops, &prepared, &receipts).map(|_| ());
    rollback.restore();

    let selected_head_after = current_head(&fixture);
    let restored_sibling = fixture.journal.load(sibling).unwrap().unwrap();
    assert_eq!(raw_head.as_ref(), Some(&head_zero));
    assert!(raw_rejected.is_none());
    assert!(
        !raw_partial,
        "the rollback is a valid old prefix, not partial bytes"
    );
    assert_eq!(restored_sibling.head(), &head_one);
    assert_eq!(fs::read(&current).unwrap(), pointer_after_cancel);
    assert_eq!(fs::read(&revision_one).unwrap(), post_cancel_revision);
    let selected_head_unchanged = selected_head_after == selected_head_before;
    assert!(
        result == Err(JournalError::Conflict) && selected_head_unchanged,
        "completion result: {result:?}; selected head unchanged: {selected_head_unchanged}"
    );
}
