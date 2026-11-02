//! Real journal regression coverage. No live retirement capability is fabricated.
use super::*;
use crate::broker::{
    checkpoint::SupervisionCheckpointV1, retirement_checkpoint::BrokerCheckpointV5,
};

pub(super) fn accepted() -> (tempfile::TempDir, Journal, AcceptedJobV1) {
    let root = tempfile::tempdir().unwrap();
    let journal = open(root.path());
    let intent = intent();
    let ticket = match journal
        .reserve(intent.operation_id(), br#"{"retirement_tests":true}"#)
        .unwrap()
    {
        ReserveResult::New(ticket) => ticket,
        _ => panic!("fresh journal"),
    };
    let initial = CheckpointPayloadV1::initial(&intent).unwrap();
    let job = journal
        .accept(ticket, &intent, BindingInputs::None, &initial)
        .unwrap();
    (root, journal, job)
}

fn v5(job: &AcceptedJobV1) -> BrokerCheckpointV5 {
    let v2 = BrokerCheckpointV2::new(
        job.checkpoint().clone(),
        SupervisionCheckpointV1::new(job.scope().control_namespace_uuid(), Vec::new(), Vec::new())
            .unwrap(),
    )
    .unwrap();
    let v3 = BrokerCheckpointV3::new(v2, Vec::new()).unwrap();
    BrokerCheckpointV5::new(
        BrokerCheckpointV4::new(v3, Vec::new()).unwrap(),
        Vec::new(),
        Vec::new(),
    )
    .unwrap()
}

#[test]
fn v5_journal_refuses_direct_downgrade_and_preserves_extensions_for_outbox_and_cancel() {
    let (_root, journal, job) = accepted();
    let checkpoint = v5(&job);
    journal
        .advance_retirement(job.operation_id(), job.head().clone(), &checkpoint)
        .unwrap();
    let loaded = journal.load(job.operation_id()).unwrap().unwrap();
    assert!(loaded.broker_checkpoint_v5().is_some());
    assert!(matches!(
        journal.advance_lifecycle(
            job.operation_id(),
            loaded.head().clone(),
            checkpoint.outbox().lifecycle()
        ),
        Err(JournalError::Conflict)
    ));
    journal
        .advance_outbox(
            job.operation_id(),
            loaded.head().clone(),
            checkpoint.outbox(),
        )
        .unwrap();
    let loaded = journal.load(job.operation_id()).unwrap().unwrap();
    assert!(loaded.broker_checkpoint_v5().is_some());
    let stops = StopJournal::from_full(&journal);
    let command = Uuid::new_v4();
    stops.append_cancel(job.operation_id(), command).unwrap();
    let loaded = journal.load(job.operation_id()).unwrap().unwrap();
    assert!(loaded.broker_checkpoint_v5().is_some());
    assert_eq!(
        loaded.broker_checkpoint_v3().unwrap().commands()[0].command_id(),
        command
    );
}

#[test]
fn v5_checkpoint_codec_is_closed_and_canonical() {
    let (_root, _journal, job) = accepted();
    let checkpoint = v5(&job);
    let bytes = canonical(&checkpoint).unwrap();
    let wire: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(wire.get("full3_registrations").is_some());
    assert!(wire.get("bindings").is_none());
    let decoded: BrokerCheckpointV5 = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(canonical(&decoded).unwrap(), bytes);
    let mut value: Value = serde_json::from_slice(&bytes).unwrap();
    value["unknown"] = Value::Bool(true);
    assert!(serde_json::from_value::<BrokerCheckpointV5>(value).is_err());
}

fn open(path: &std::path::Path) -> Journal {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    let directory = Directory::open_existing(&path.canonicalize().unwrap()).unwrap();
    Journal::open(
        &directory,
        JournalScopeV1::new(
            Uuid::new_v4(),
            "tenant-retirement",
            "http://127.0.0.1:8787",
            Uuid::new_v4(),
            "retirement-test",
        )
        .unwrap(),
        &FsExclusions::new(),
        JournalLimitsV1::default(),
        ControlSecrets::new(Vec::new()).unwrap(),
    )
    .unwrap()
}
fn intent() -> ExecutionIntentV1 {
    let mut value: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/broker_intent-plan-v3.json"
    ))
    .unwrap();
    value["operation_id"] = serde_json::json!(Uuid::new_v4());
    value["version"] = serde_json::json!(1);
    value["options"]["profile"] = serde_json::json!("strict");
    value["options"]["network"] = serde_json::json!("none");
    value["options"]["provider"] = serde_json::json!("generic");
    value["options"]["argv"] = serde_json::json!(["/usr/bin/id"]);
    for field in [
        "client",
        "auth_mode",
        "client_profile",
        "providers_file",
        "prompt",
        "model",
    ] {
        value["options"].as_object_mut().unwrap().remove(field);
    }
    value["request"]["profile"] = serde_json::json!("strict");
    value["request"]["agent_id"] =
        serde_json::json!("generic-22222222-2222-4222-8222-222222222222");
    value["request"]["command"]["argv"] = serde_json::json!(["/usr/bin/id"]);
    value["request"]
        .as_object_mut()
        .unwrap()
        .remove("provider_resolution");
    value["provider_input"] = Value::Null;
    value["provider_credential"] = Value::Null;
    ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&value).unwrap()).unwrap()
}

#[test]
fn v5_independent_cancel_survives_malformed_other_job_metadata() {
    use std::io::Write;
    let (_root, journal, job) = accepted();
    let checkpoint = v5(&job);
    journal
        .advance_retirement(job.operation_id(), job.head().clone(), &checkpoint)
        .unwrap();
    let other = intent();
    let ticket = match journal
        .reserve(other.operation_id(), br#"{"other_job":true}"#)
        .unwrap()
    {
        ReserveResult::New(ticket) => ticket,
        _ => panic!("fresh other operation"),
    };
    journal
        .accept(
            ticket,
            &other,
            BindingInputs::None,
            &CheckpointPayloadV1::initial(&other).unwrap(),
        )
        .unwrap();
    let other_revision = journal
        .0
        .jobs
        .path()
        .join(other.operation_id().to_string())
        .join("rev-00000000000000000000.json");
    // This temporary corruption belongs only to the test. No source record or
    // production evidence is repaired, replaced or deleted by the product.
    std::fs::OpenOptions::new()
        .append(true)
        .open(other_revision)
        .unwrap()
        .write_all(b"malformed-suffix")
        .unwrap();
    let command = Uuid::new_v4();
    let projection = StopJournal::from_full(&journal)
        .append_cancel(job.operation_id(), command)
        .unwrap();
    assert_eq!(projection.command_id(), command);
    let selected = journal.load(job.operation_id()).unwrap().unwrap();
    assert!(selected.broker_checkpoint_v5().is_some());
    assert_eq!(selected.broker_checkpoint_v3().unwrap().commands().len(), 1);
}

#[test]
fn new_submission_refuses_unknown_existing_future_budget_before_creating_job() {
    use std::io::Write;
    let (_root, journal, job) = accepted();
    let file = journal
        .0
        .jobs
        .path()
        .join(job.operation_id().to_string())
        .join("rev-00000000000000000000.json");
    std::fs::OpenOptions::new()
        .append(true)
        .open(file)
        .unwrap()
        .write_all(b"corrupt-future-metadata")
        .unwrap();
    let next = Uuid::new_v4();
    assert!(journal.reserve(next, b"new submission").is_err());
    assert!(!journal.0.jobs.path().join(next.to_string()).exists());
}
