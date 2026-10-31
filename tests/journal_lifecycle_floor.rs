use agentsandbox::{
    broker::{
        intent::ExecutionIntentV1,
        journal::{
            BindingInputs, ControlSecrets, FsExclusions, Journal, JournalError, JournalScopeV1,
            ReserveResult,
        },
    },
    control_fs::Directory,
    runner::CheckpointPayloadV1,
};
use serde_json::{Value, json};
use std::{fs, os::unix::fs::PermissionsExt};
use tempfile::tempdir;
use uuid::Uuid;

fn legacy_plan_value() -> Value {
    let mut plan: Value = serde_json::from_str(include_str!("fixtures/broker_intent-plan-v3.json"))
        .expect("typed private plan fixture");
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
    plan
}

#[test]
fn reserve_does_not_report_interrupted_after_live_accepted_marker_is_missing() {
    let temporary = tempdir().expect("private test root");
    fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let directory = Directory::open_existing(&root).unwrap();
    let scope = JournalScopeV1::new(
        uuid::Uuid::from_u128(0x11111111111141118111111111111111),
        "tenant-lifecycle-floor-test",
        "http://127.0.0.1:8787",
        uuid::Uuid::from_u128(0x22222222222242228222222222222222),
        "broker-lifecycle-floor-test",
    )
    .unwrap();
    let journal = Journal::open(
        &directory,
        scope,
        &FsExclusions::new(),
        Default::default(),
        ControlSecrets::new(Vec::new()).unwrap(),
    )
    .unwrap();

    let intent =
        ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&legacy_plan_value()).unwrap())
            .unwrap();
    let operation_id = intent.operation_id();
    let submit = br#"{"action":"accepted-floor"}"#;
    let ticket = match journal.reserve(operation_id, submit).unwrap() {
        ReserveResult::New(ticket) => ticket,
        other => panic!("expected fresh reservation, got {other:?}"),
    };
    let initial = CheckpointPayloadV1::initial(&intent).unwrap();
    journal
        .accept(ticket, &intent, BindingInputs::None, &initial)
        .unwrap();

    let scope_root = fs::read_dir(root.join("broker-journal-v1"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let accepted = scope_root
        .join("jobs")
        .join(operation_id.to_string())
        .join("accepted.json");
    let accepted_bytes = fs::read(&accepted).unwrap();
    let saved = root.join("accepted-marker-saved.outside-namespace");
    fs::rename(&accepted, &saved).unwrap();

    let replay = journal.reserve(operation_id, submit);
    let status = journal.status(operation_id);
    let page = journal.list(None, 10);
    let loaded = journal.load(operation_id);

    fs::rename(&saved, &accepted).unwrap();
    assert_eq!(fs::read(&accepted).unwrap(), accepted_bytes);
    assert!(
        matches!(&replay, Err(JournalError::Conflict)),
        "accepted floor with missing marker must conflict, got {replay:?}"
    );
    assert!(matches!(status, Err(JournalError::Conflict)));
    assert!(matches!(page, Err(JournalError::Conflict)));
    assert!(matches!(loaded, Err(JournalError::Conflict)));
}

#[test]
fn list_validates_previously_observed_reservations_outside_the_requested_page() {
    let temporary = tempdir().expect("private test root");
    let root = temporary.path();
    let journal = open(root);
    let operation_a = Uuid::parse_str("33333333-3333-4333-8333-333333333333").unwrap();
    let operation_b = Uuid::parse_str("44444444-4444-4444-8444-444444444444").unwrap();
    assert!(operation_a < operation_b);

    let intent_a =
        ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&legacy_plan_value()).unwrap())
            .unwrap();
    let initial_a = CheckpointPayloadV1::initial(&intent_a).unwrap();
    let ticket_a = match journal
        .reserve(operation_a, br#"{"action":"page-a"}"#)
        .unwrap()
    {
        ReserveResult::New(ticket) => ticket,
        other => panic!("operation A must reserve, got {other:?}"),
    };
    journal
        .accept(ticket_a, &intent_a, BindingInputs::None, &initial_a)
        .unwrap();

    let mut plan_b = legacy_plan_value();
    plan_b["operation_id"] = json!(operation_b);
    let intent_b =
        ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&plan_b).unwrap()).unwrap();
    assert_eq!(intent_b.operation_id(), operation_b);
    let _ticket_b = match journal
        .reserve(operation_b, br#"{"action":"page-b"}"#)
        .unwrap()
    {
        ReserveResult::New(ticket) => ticket,
        other => panic!("operation B must reserve, got {other:?}"),
    };
    assert!(matches!(
        journal.status(operation_b).unwrap(),
        Some(agentsandbox::broker::journal::JournalStatusV1::ReservedInProcess)
    ));

    let scope_root = fs::read_dir(root.join("broker-journal-v1"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let reservation = scope_root
        .join("jobs")
        .join(operation_b.to_string())
        .join("reservation.json");
    let original = fs::read(&reservation).unwrap();
    fs::write(&reservation, b"{corrupt reservation fixture").unwrap();

    let page = journal.list(None, 1);

    fs::write(&reservation, original).unwrap();
    assert!(
        matches!(&page, Err(JournalError::Corrupt)),
        "listing A must validate the previously observed B outside the page; got {page:?}"
    );
}

fn open(root: &std::path::Path) -> Journal {
    fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
    let root = root.canonicalize().unwrap();
    Journal::open(
        &Directory::open_existing(&root).unwrap(),
        JournalScopeV1::new(
            Uuid::from_u128(0x11111111111141118111111111111111),
            "tenant-lifecycle-floor-test",
            "http://127.0.0.1:8787",
            Uuid::from_u128(0x22222222222242228222222222222222),
            "broker-lifecycle-floor-test",
        )
        .unwrap(),
        &FsExclusions::new(),
        Default::default(),
        ControlSecrets::new(Vec::new()).unwrap(),
    )
    .unwrap()
}
