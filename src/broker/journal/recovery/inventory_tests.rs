//! Real private files and the retained STOP lease; no engine or provider.
use super::*;
use crate::{broker::intent::ExecutionIntentV1, runner::CheckpointPayloadV1};
use serde_json::{Value, json};
use std::{fs, os::unix::fs::PermissionsExt, time::Duration};

fn fixture() -> (tempfile::TempDir, Journal) {
    let temp = tempfile::tempdir().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let root = Directory::open_existing(&temp.path().canonicalize().unwrap()).unwrap();
    let scope = JournalScopeV1::new(
        Uuid::new_v4(),
        "tenant-inventory",
        "http://127.0.0.1:8787",
        Uuid::new_v4(),
        "inventory-test",
    )
    .unwrap();
    let journal = Journal::open(
        &root,
        scope,
        &FsExclusions::new(),
        JournalLimitsV1::default(),
        ControlSecrets::new(vec![]).unwrap(),
    )
    .unwrap();
    (temp, journal)
}

fn accept(journal: &Journal, operation: Uuid, source: Option<&PrivateFile>) -> RevisionRefV1 {
    let mut plan: Value = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/broker_intent-plan-v3.json"
    ))
    .unwrap();
    plan["operation_id"] = json!(operation);
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
    if let Some(source) = source {
        plan["options"]["profile"] = json!("observe");
        plan["options"]["network"] = json!("bridge");
        plan["options"]["secret_env_file"] = json!(source.path());
        plan["request"]["profile"] = json!("observe");
    }
    let intent = ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&plan).unwrap()).unwrap();
    let initial = CheckpointPayloadV1::initial(&intent).unwrap();
    let ReserveResult::New(ticket) = journal
        .reserve(operation, br#"{"fixture":"inventory"}"#)
        .unwrap()
    else {
        panic!("fresh fixture reservation");
    };
    let binding = source.map_or(BindingInputs::None, |source| BindingInputs::Manual {
        source,
    });
    journal
        .accept(ticket, &intent, binding, &initial)
        .unwrap()
        .head()
        .clone()
}

#[test]
fn inventory_discovers_confirmed_ids_without_binding_blob_and_keeps_valid_sibling() {
    let (temp, journal) = fixture();
    let source_path = temp
        .path()
        .canonicalize()
        .unwrap()
        .join("synthetic-fixture.env");
    fs::write(
        &source_path,
        b"FIXTURE_KEY=synthetic-inventory-canary-123\n",
    )
    .unwrap();
    fs::set_permissions(&source_path, fs::Permissions::from_mode(0o600)).unwrap();
    let source = PrivateFile::open(&source_path, 65536).unwrap();
    let a = Uuid::from_u128(1);
    let b = Uuid::from_u128(2);
    let head = accept(&journal, a, Some(&source));
    accept(&journal, b, None);
    let job = journal.load(a).unwrap().unwrap();
    let blob = job.binding_source().unwrap().path().to_owned();
    drop(job);
    fs::rename(blob, temp.path().join("preserved-fixture-blob")).unwrap();
    fs::write(
        journal
            .0
            .jobs
            .path()
            .join(b.to_string())
            .join("current.json"),
        b"invalid",
    )
    .unwrap();
    assert!(
        journal.load(a).is_err(),
        "positive strict business-load control"
    );
    let stops = StopJournal::from_full(&journal);
    assert_eq!(
        stops.recovery_status(a).unwrap().unwrap().head().hash(),
        head.hash()
    );
    let page = stops.recovery_inventory(None, 20).unwrap();
    assert_eq!(page.entries().len(), 1);
    assert_eq!(page.entries()[0].operation_id(), a);
    assert_eq!(page.entries()[0].head().hash(), head.hash());
    assert!(page.entries()[0].incomplete());
    assert!(page.incomplete());
    assert!(
        page.issues()
            .iter()
            .any(|issue| issue.operation_id() == Some(b))
    );
    assert_eq!(
        page.next_after(),
        None,
        "partial scans cannot promise continuation"
    );
}

#[test]
fn inventory_pages_only_accepted_jobs_and_validates_closed_bounds() {
    let (_temp, journal) = fixture();
    let a = Uuid::from_u128(1);
    let b = Uuid::from_u128(2);
    accept(&journal, b, None);
    accept(&journal, a, None);
    let rejected = Uuid::from_u128(4);
    let ReserveResult::New(ticket) = journal.reserve(rejected, b"rejected-fixture").unwrap() else {
        panic!("fresh");
    };
    journal
        .reject(
            ticket,
            RejectionPhaseV1::SubmissionPreparation,
            RejectionCodeV1::PreparationFailed,
        )
        .unwrap();
    let stops = StopJournal::from_full(&journal);
    for (after, limit) in [(None, 0), (None, 101), (Some(Uuid::nil()), 1)] {
        assert!(matches!(
            stops.recovery_inventory(after, limit),
            Err(JournalError::InvalidInput)
        ));
    }
    let first = stops.recovery_inventory(None, 1).unwrap();
    assert_eq!(first.entries().len(), 1);
    assert_eq!(first.entries()[0].operation_id(), a);
    assert_eq!(first.next_after(), Some(a));
    assert!(!first.incomplete());
    let second = stops.recovery_inventory(first.next_after(), 1).unwrap();
    assert_eq!(second.entries()[0].operation_id(), b);
    assert_eq!(second.next_after(), None);
    assert_eq!(
        stops
            .recovery_inventory(Some(b), 100)
            .unwrap()
            .entries()
            .len(),
        0
    );
    let reserved = Uuid::from_u128(3);
    let _ticket = journal.reserve(reserved, b"reserved-fixture").unwrap();
    let partial = stops.recovery_inventory(None, 100).unwrap();
    assert_eq!(
        partial.entries().len(),
        2,
        "a reservation is not an accepted job"
    );
    assert!(partial.incomplete());
    assert_eq!(partial.next_after(), None);
}

#[test]
fn inventory_preserves_live_floors_and_does_not_hide_disappeared_job() {
    let (temp, journal) = fixture();
    let a = Uuid::from_u128(1);
    let b = Uuid::from_u128(2);
    accept(&journal, a, None);
    accept(&journal, b, None);
    let stops = StopJournal::from_full(&journal);
    let old = fs::read(
        journal
            .0
            .jobs
            .path()
            .join(a.to_string())
            .join("current.json"),
    )
    .unwrap();
    let initial = journal.load(a).unwrap().unwrap().checkpoint().clone();
    let head = journal.load(a).unwrap().unwrap().head().clone();
    journal.advance(a, head, &initial).unwrap();
    assert_eq!(
        stops.recovery_inventory(None, 20).unwrap().entries()[0]
            .head()
            .revision(),
        1
    );
    fs::write(
        journal
            .0
            .jobs
            .path()
            .join(a.to_string())
            .join("current.json"),
        old,
    )
    .unwrap();
    let page = stops.recovery_inventory(None, 20).unwrap();
    assert_eq!(page.entries().len(), 1);
    assert_eq!(page.entries()[0].operation_id(), b);
    assert!(page.issues().iter().any(|issue| issue.operation_id() == Some(a) && issue.error() == JournalError::Conflict));
    fs::rename(
        journal.0.jobs.path().join(b.to_string()),
        temp.path().join("preserved-job-b"),
    )
    .unwrap();
    assert!(
        stops.recovery_inventory(None, 20).is_err(),
        "no successful empty page after errors"
    );
}

#[test]
fn inventory_shared_anchor_loss_never_returns_cached_rows() {
    let (temp, journal) = fixture();
    accept(&journal, Uuid::from_u128(1), None);
    let stops = StopJournal::from_full(&journal);
    assert_eq!(
        stops.recovery_inventory(None, 20).unwrap().entries().len(),
        1
    );
    fs::rename(journal.0.jobs.path(), temp.path().join("preserved-jobs")).unwrap();
    fs::create_dir(journal.0.jobs.path()).unwrap();
    fs::set_permissions(journal.0.jobs.path(), fs::Permissions::from_mode(0o700)).unwrap();
    assert!(stops.recovery_inventory(None, 20).is_err());
}

#[test]
fn inventory_issue_limit_is_explicit_without_an_invented_cursor() {
    let (_temp, journal) = fixture();
    let operation = Uuid::from_u128(1);
    accept(&journal, operation, None);
    for index in 0..101 {
        fs::write(
            journal
                .0
                .jobs
                .path()
                .join(format!("invalid-fixture-{index:03}")),
            b"",
        )
        .unwrap();
        fs::set_permissions(
            journal
                .0
                .jobs
                .path()
                .join(format!("invalid-fixture-{index:03}")),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }
    let page = StopJournal::from_full(&journal)
        .recovery_inventory(None, 100)
        .unwrap();
    assert_eq!(page.entries().len(), 1);
    assert!(page.incomplete() && page.issues_truncated());
    assert_eq!(page.issues().len(), 100);
    assert_eq!(page.next_after(), None);
}

#[test]
fn inventory_scan_limit_retains_proven_rows_without_an_invented_cursor() {
    let (temp, journal) = fixture();
    accept(&journal, Uuid::from_u128(1), None);
    accept(&journal, Uuid::from_u128(2), None);
    let scope = journal.scope().clone();
    drop(journal);
    let root = Directory::open_existing(&temp.path().canonicalize().unwrap()).unwrap();
    let limits =
        JournalLimitsV1::new(256 * 1024 * 1024, 512, 256, 1024 * 1024 * 1024, 512, 1).unwrap();
    let report = StopRecoveryReaderV1::open_existing(
        &root,
        scope,
        &FsExclusions::new(),
        limits,
        ControlSecrets::new(vec![]).unwrap(),
    )
    .unwrap()
    .unwrap()
    .scan();
    let ExistingProfileV1::Stops(stops) = admit_report(&report, true).unwrap() else {
        panic!("STOP-only");
    };
    let page = stops.recovery_inventory(None, 1).unwrap();
    assert!(page.incomplete());
    assert_eq!(page.entries().len(), 1);
    assert!(
        page.issues()
            .iter()
            .any(|issue| issue.error() == JournalError::LimitExceeded)
    );
    assert_eq!(page.next_after(), None);
}

#[tokio::test(flavor = "current_thread")]
async fn inventory_uses_the_existing_stop_only_queue_without_full_capability() {
    use crate::broker::writer::ProfileWriterCapabilities;
    let (temp, journal) = fixture();
    let operation = Uuid::from_u128(1);
    accept(&journal, operation, None);
    let scope = journal.scope().clone();
    drop(journal);
    let root = Directory::open_existing(&temp.path().canonicalize().unwrap()).unwrap();
    let report = StopRecoveryReaderV1::open_existing(
        &root,
        scope,
        &FsExclusions::new(),
        JournalLimitsV1::default(),
        ControlSecrets::new(vec![]).unwrap(),
    )
    .unwrap()
    .unwrap()
    .scan();
    let capabilities =
        ProfileWriterCapabilities::from_recovery_report(&report, true, 4, Duration::from_secs(2))
            .unwrap();
    assert!(capabilities.full().is_none());
    let page = capabilities
        .stops()
        .recovery_inventory(None, 1)
        .await
        .unwrap();
    assert_eq!(page.entries()[0].operation_id(), operation);
    assert!(capabilities.full().is_none());
    assert!(
        capabilities
            .stops()
            .recovery_status(operation)
            .await
            .unwrap()
            .is_some()
    );
}
