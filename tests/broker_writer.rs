#![cfg(unix)]

use agentsandbox::{
    broker::{
        journal::{
            ControlSecrets, FsExclusions, Journal, JournalError, JournalLimitsV1, JournalScopeV1,
            JournalStatusV1, RejectionCodeV1, RejectionPhaseV1, ReserveResult,
        },
        writer::WriterClient,
    },
    control_fs::Directory,
};
use std::{fs, os::unix::fs::PermissionsExt, time::Duration};
use uuid::Uuid;

fn private_journal() -> (tempfile::TempDir, Journal) {
    let root = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let directory = Directory::open_existing(&root.path().canonicalize().unwrap()).unwrap();
    let scope = JournalScopeV1::new(
        Uuid::from_u128(0x11111111111141118111111111111111),
        "tenant-writer-test",
        "http://127.0.0.1:8787",
        Uuid::from_u128(0x22222222222242228222222222222222),
        "writer_test",
    )
    .unwrap();
    let journal = Journal::open(
        &directory,
        scope,
        &FsExclusions::new(),
        JournalLimitsV1::default(),
        ControlSecrets::new(Vec::new()).unwrap(),
    )
    .unwrap();
    (root, journal)
}

#[tokio::test]
async fn bounded_writer_keeps_scope_private_and_persists_rejection_replay() {
    let (_root, journal) = private_journal();
    assert_eq!(
        WriterClient::start(journal.clone(), 0, Duration::from_secs(1)).unwrap_err(),
        JournalError::InvalidInput
    );
    assert_eq!(
        WriterClient::start(journal.clone(), 257, Duration::from_secs(1)).unwrap_err(),
        JournalError::InvalidInput
    );
    assert_eq!(
        WriterClient::start(journal.clone(), 1, Duration::ZERO).unwrap_err(),
        JournalError::InvalidInput
    );

    let writer = WriterClient::start(journal, 8, Duration::from_secs(2)).unwrap();
    assert_eq!(writer.scope().tenant_id(), "tenant-writer-test");
    assert_eq!(writer.scope().gateway_origin(), "http://127.0.0.1:8787");
    assert_eq!(writer.scope().profile_id(), "writer_test");
    assert_eq!(
        writer.scope().installation_id(),
        Uuid::from_u128(0x11111111111141118111111111111111)
    );
    assert_eq!(
        writer.scope().control_namespace_uuid(),
        Uuid::from_u128(0x22222222222242228222222222222222)
    );

    let operation = Uuid::from_u128(0x33333333333343338333333333333333);
    let submit = br#"{"private":"writer-submit-canary-9e41"}"#.to_vec();
    let ticket = match writer.reserve(operation, submit.clone()).await.unwrap() {
        ReserveResult::New(ticket) => ticket,
        other => panic!("expected a new reservation, got {other:?}"),
    };
    let receipt = writer
        .reject(
            ticket,
            RejectionPhaseV1::SubmissionPreparation,
            RejectionCodeV1::PreparationFailed,
        )
        .await
        .unwrap();
    assert_eq!(receipt.operation_id(), operation);

    assert!(writer.load(operation).await.is_err());
    assert!(matches!(
        writer.status(operation).await.unwrap(),
        Some(JournalStatusV1::Rejected(replayed)) if replayed.operation_id() == operation
    ));
    assert!(matches!(
        writer.reserve(operation, submit.clone()).await.unwrap(),
        ReserveResult::Rejected(replayed) if replayed.operation_id() == operation
    ));
    let page = writer.list(None, 10).await.unwrap();
    assert_eq!(page.entries().len(), 1);
    assert_eq!(page.entries()[0].operation_id(), operation);

    let writer_debug = format!("{writer:?}");
    let scope_debug = format!("{:?}", writer.scope());
    for diagnostic in [writer_debug.as_str(), scope_debug.as_str()] {
        assert!(!diagnostic.contains("writer-submit-canary-9e41"));
        assert!(!diagnostic.contains("tenant-writer-test"));
        assert!(!diagnostic.contains("http://127.0.0.1:8787"));
        assert!(!diagnostic.contains("writer_test"));
    }
}
