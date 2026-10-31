#![cfg(unix)]

use agentsandbox::{
    broker::{
        journal::{
            ControlSecrets, FsExclusions, Journal, JournalError, JournalLimitsV1, JournalScopeV1,
            JournalStatusV1, RejectionCodeV1, RejectionPhaseV1, ReserveResult,
            recovery::StopRecoveryReaderV1,
        },
        writer::{ProfileWriterCapabilities, WriterClient},
    },
    control_fs::Directory,
};
use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    time::Duration,
};
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
async fn recovery_report_admits_one_actor_and_retains_the_original_lease() {
    let (root, journal) = private_journal();
    let directory = Directory::open_existing(&root.path().canonicalize().unwrap()).unwrap();
    let scope = journal.scope().clone();
    let namespace = fs::read_dir(root.path().join("broker-journal-v1"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let lock = namespace.join(".writer.lock");
    let before = fs::metadata(&lock).unwrap();
    drop(journal);
    let report = StopRecoveryReaderV1::open_existing(
        &directory,
        scope.clone(),
        &FsExclusions::new(),
        JournalLimitsV1::default(),
        ControlSecrets::new(vec![]).unwrap(),
    )
    .unwrap()
    .unwrap()
    .scan();
    assert!(!report.incomplete());
    let capabilities =
        ProfileWriterCapabilities::from_recovery_report(&report, false, 4, Duration::from_secs(2))
            .unwrap();
    assert!(!capabilities.is_stop_only());
    assert_eq!(
        ProfileWriterCapabilities::from_recovery_report(&report, true, 4, Duration::from_secs(2))
            .unwrap_err(),
        JournalError::Busy
    );
    drop(report);
    let stops = capabilities.stops();
    drop(capabilities);
    let reopen = || {
        Journal::open(
            &directory,
            scope.clone(),
            &FsExclusions::new(),
            JournalLimitsV1::default(),
            ControlSecrets::new(vec![]).unwrap(),
        )
    };
    assert!(matches!(reopen(), Err(JournalError::Busy)));
    let after = fs::metadata(&lock).unwrap();
    assert_eq!((before.dev(), before.ino()), (after.dev(), after.ino()));
    drop(stops);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        match reopen() {
            Ok(_) => break,
            Err(JournalError::Busy) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(error) => panic!(
                "final actor must release the same lease after bounded queue drain: {error:?}"
            ),
        }
    }
}

#[tokio::test]
async fn recovery_handoff_retains_the_hash_of_an_already_observed_rejection() {
    let (root, journal) = private_journal();
    let scope = journal.scope().clone();
    let operation = Uuid::new_v4();
    let ReserveResult::New(ticket) = journal
        .reserve(operation, b"synthetic-rejection-floor")
        .unwrap()
    else {
        panic!("fresh operation");
    };
    journal
        .reject(
            ticket,
            RejectionPhaseV1::SubmissionPreparation,
            RejectionCodeV1::PreparationFailed,
        )
        .unwrap();
    drop(journal);
    let directory = Directory::open_existing(&root.path().canonicalize().unwrap()).unwrap();
    let report = StopRecoveryReaderV1::open_existing(
        &directory,
        scope,
        &FsExclusions::new(),
        JournalLimitsV1::default(),
        ControlSecrets::new(vec![]).unwrap(),
    )
    .unwrap()
    .unwrap()
    .scan();
    assert!(!report.incomplete());
    assert!(
        report.jobs().is_empty(),
        "the report has observed a rejection, not an accepted job"
    );
    let capabilities =
        ProfileWriterCapabilities::from_recovery_report(&report, false, 4, Duration::from_secs(2))
            .unwrap();
    let namespace = fs::read_dir(root.path().join("broker-journal-v1"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let rejection = namespace
        .join("jobs")
        .join(operation.to_string())
        .join("rejected.json");
    let original = fs::read(&rejection).unwrap();
    let mut changed: serde_json::Value = serde_json::from_slice(&original).unwrap();
    changed["code"] = serde_json::json!("cancelled_before_acceptance");
    // Editing an existing field retains the original canonical key order.
    let bytes = serde_json::to_vec(&changed).unwrap();
    assert_ne!(bytes, original);
    fs::write(&rejection, bytes).unwrap();
    assert!(
        matches!(
            capabilities.full().unwrap().status(operation).await,
            Err(JournalError::Conflict)
        ),
        "continuous lease must not forget a rejection hash during actor handoff"
    );
    assert!(matches!(
        capabilities.stops().recovery_status(operation).await,
        Err(JournalError::Conflict)
    ));
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
