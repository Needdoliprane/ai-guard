use super::{checkpoint_heads_tests::accepted, tests::private_journal, *};

#[tokio::test]
async fn head_claim_excludes_same_job_publication_until_drop() {
    let (_root, journal) = private_journal();
    let job = accepted(&journal, Uuid::new_v4());
    let other = accepted(&journal, Uuid::new_v4());
    let writer = WriterClient::start(journal.clone(), 8, Duration::from_secs(10)).unwrap();
    let operation = job.operation_id();
    let issuer = EgressReleaseFenceIssuer::new();
    let permit = journal
        .issue_release_head_for_test(operation, job.head(), &issuer)
        .unwrap();
    let guard = permit.claim().unwrap();

    assert!(matches!(
        journal.issue_release_head_for_test(operation, job.head(), &issuer),
        Err(JournalError::Busy)
    ));
    let command = Uuid::new_v4();
    let request = LocalCommandRequestV1::CancelRequested;
    assert!(matches!(
        writer
            .append_command(operation, command, request.clone())
            .await,
        Err(JournalError::Busy)
    ));
    let still_current = writer.load(operation).await.unwrap().unwrap();
    assert_eq!(still_current.head(), job.head());
    assert!(
        journal
            .command_receipt(operation, command)
            .unwrap()
            .is_none()
    );
    writer
        .append_command(other.operation_id(), Uuid::new_v4(), request.clone())
        .await
        .unwrap();

    drop(guard);
    let receipt = writer
        .append_command(operation, command, request)
        .await
        .unwrap();
    assert_eq!(receipt.head().revision(), job.head().revision() + 1);
}

#[tokio::test]
async fn head_fence_command_requires_the_exact_intent_on_the_strict_head() {
    use serde_json::json;

    let (_root, journal) = private_journal();
    let job = accepted(&journal, Uuid::new_v4());
    let writer = WriterClient::start(journal, 8, Duration::from_secs(10)).unwrap();
    let operation = job.operation_id();
    let id = Uuid::new_v4();
    let hash = "a".repeat(64);
    let intent: EgressReleaseIntentV1 = serde_json::from_value(json!({
        "version": 1,
        "target": {
            "tenant_id": "tenant-writer-ownership-test", "operation_id": operation,
            "session_id": id, "worker_id": id, "attempt_id": id, "authority_id": id,
            "authority_fingerprint": hash, "generation": 1, "container_id": hash,
            "started_at": "2026-11-25T10:15:30Z", "restart_count": 0,
            "configuration_fingerprint": hash, "configuration_fingerprint_version": 2,
        },
        "opening": {
            "version": 1, "observation_id": id, "disposition": "fresh",
            "binding": {
                "session_id": id, "worker_id": id, "attempt_id": id,
                "channel_id": id, "epoch": 1, "context_digest": hash,
            },
            "receipt": {"audit_seq": 8, "audit_hash": hash},
        },
        "worker_revision": 6, "running_audit_seq": 7,
    }))
    .unwrap();

    writer.status(operation).await.unwrap().unwrap();
    assert!(matches!(
        writer
            .fence_egress_release_head(operation, job.head(), &intent)
            .await,
        Err(JournalError::Conflict)
    ));
    assert_eq!(
        writer.last_confirmed_heads(1).unwrap().entries()[0].1,
        *job.head()
    );
}

#[tokio::test]
async fn outbox_ack_revokes_an_unclaimed_head_ticket() {
    let (_root, journal) = private_journal();
    let job = accepted(&journal, Uuid::new_v4());
    let operation = job.operation_id();
    let request = job.intent().request_data();
    let mut event = Event::new("fixture.audit");
    event.session_id = request.session_id.clone();
    event.project_id = request.project_id.clone();
    event.sandbox_id = request.sandbox_id.clone();
    event.correlation_id = request.correlation_id.clone();
    event.trace_id = request.trace_id.clone();
    let id = Uuid::parse_str(&event.id).unwrap();

    let writer = WriterClient::start(journal.clone(), 8, Duration::from_secs(10)).unwrap();
    let birth = writer.enqueue_events(operation, vec![event]).await.unwrap();
    let issuer = EgressReleaseFenceIssuer::new();
    let permit = journal
        .issue_release_head_for_test(operation, birth[0].head(), &issuer)
        .unwrap();

    let ack = writer
        .acknowledge_events(operation, vec![(id, 1)])
        .await
        .unwrap();
    assert_eq!(ack[0].head().revision(), birth[0].head().revision() + 1);
    assert!(matches!(permit.claim(), Err(JournalError::Conflict)));
}

#[tokio::test]
async fn stop_only_publication_shares_the_fence_and_preserves_cancel_identity() {
    let (_root, journal) = private_journal();
    let job = accepted(&journal, Uuid::new_v4());
    let operation = job.operation_id();
    let stop_only = StopWriterClient(
        ClientState::start(
            Backend::Stops(StopJournal::from_full(&journal)),
            8,
            Duration::from_secs(10),
        )
        .unwrap(),
    );
    let issuer = EgressReleaseFenceIssuer::new();
    let permit = journal
        .issue_release_head_for_test(operation, job.head(), &issuer)
        .unwrap();
    let guard = permit.claim().unwrap();
    let command = Uuid::new_v4();

    let refused = stop_only
        .append_cancel(CancelInputV1::new(operation, command).unwrap())
        .await;
    assert!(matches!(
        refused,
        Err(CompletionFailure::Unknown {
            error: JournalError::Busy
        })
    ));
    assert!(
        journal
            .command_receipt(operation, command)
            .unwrap()
            .is_none()
    );

    drop(guard);
    let permit = journal
        .issue_release_head_for_test(operation, job.head(), &issuer)
        .unwrap();
    let cancel = stop_only
        .append_cancel(CancelInputV1::new(operation, command).unwrap())
        .await
        .unwrap();
    assert_eq!(cancel.receipt().command_id(), command);
    assert_eq!(
        cancel.receipt().head().revision(),
        job.head().revision() + 1
    );
    assert!(matches!(permit.claim(), Err(JournalError::Conflict)));
}

#[tokio::test]
async fn publication_revokes_before_an_ambiguous_acknowledgement() {
    use futures_util::FutureExt;

    let (_root, journal) = private_journal();
    let job = accepted(&journal, Uuid::new_v4());
    let operation = job.operation_id();
    let writer = WriterClient::start(journal.clone(), 8, Duration::from_secs(10)).unwrap();
    let issuer = EgressReleaseFenceIssuer::new();
    let permit = journal
        .issue_release_head_for_test(operation, job.head(), &issuer)
        .unwrap();
    let mut barrier = writer.advance_reply_barrier_for_test(true).await.unwrap();
    let mutation =
        writer.advance_lifecycle(operation, job.head().clone(), lifecycle(&job).unwrap());
    tokio::pin!(mutation);
    assert!(mutation.as_mut().now_or_never().is_none());
    barrier.entered().await;

    assert!(matches!(permit.claim(), Err(JournalError::Conflict)));
    drop(barrier);
    assert!(matches!(mutation.await, Err(JournalError::Io)));
    assert_eq!(
        writer
            .load(operation)
            .await
            .unwrap()
            .unwrap()
            .head()
            .revision(),
        job.head().revision() + 1
    );
}

#[test]
fn an_undelivered_ticket_and_a_closed_issuer_cannot_block_publication() {
    let (_root, journal) = private_journal();
    let job = accepted(&journal, Uuid::new_v4());
    let operation = job.operation_id();
    let issuer = EgressReleaseFenceIssuer::new();
    let permit = journal
        .issue_release_head_for_test(operation, job.head(), &issuer)
        .unwrap();
    let (reply, receive) = oneshot::channel();
    drop(receive);
    assert!(reply.send(permit).is_err());

    let permit = journal
        .issue_release_head_for_test(operation, job.head(), &issuer)
        .unwrap();
    drop(issuer);
    assert!(matches!(permit.claim(), Err(JournalError::Conflict)));
    assert!(
        journal
            .advance_lifecycle(operation, job.head().clone(), &lifecycle(&job).unwrap())
            .is_ok()
    );
}

#[test]
fn a_publication_error_before_the_head_swap_still_revokes_the_ticket() {
    let (_root, journal) = private_journal();
    let job = accepted(&journal, Uuid::new_v4());
    let operation = job.operation_id();
    let issuer = EgressReleaseFenceIssuer::new();
    let permit = journal
        .issue_release_head_for_test(operation, job.head(), &issuer)
        .unwrap();
    super::super::journal::fail_next_publication_for_test();

    assert!(matches!(
        journal.advance_lifecycle(operation, job.head().clone(), &lifecycle(&job).unwrap()),
        Err(JournalError::Io)
    ));
    assert!(matches!(permit.claim(), Err(JournalError::Conflict)));
}
