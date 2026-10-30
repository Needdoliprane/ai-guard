use agentsandbox::{
    broker::{intent::ExecutionIntentV1, journal::{BindingInputs, ControlSecrets, FsExclusions, Journal, JournalLimitsV1, JournalScopeV1, ReserveResult}},
    control_fs::Directory,
    runner::CheckpointPayloadV1,
};
use serde_json::{Value, json};
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};
use uuid::Uuid;

fn legacy_plan() -> Value {
    let mut plan: Value = serde_json::from_str(include_str!("/Users/louis/Documents/ai-guard/.worktrees/broker-service/tests/fixtures/broker_intent-plan-v3.json")).unwrap();
    plan["version"] = json!(1);
    plan["options"]["profile"] = json!("strict");
    plan["options"]["network"] = json!("none");
    plan["options"]["provider"] = json!("generic");
    plan["options"]["argv"] = json!(["/usr/bin/id"]);
    for field in ["client", "auth_mode", "client_profile", "providers_file", "prompt", "model"] {
        plan["options"].as_object_mut().unwrap().remove(field);
    }
    plan["request"]["profile"] = json!("strict");
    plan["request"]["agent_id"] = json!("generic-22222222-2222-4222-8222-222222222222");
    plan["request"]["command"]["argv"] = json!(["/usr/bin/id"]);
    plan["request"].as_object_mut().unwrap().remove("provider_resolution");
    plan["provider_input"] = Value::Null;
    plan["provider_credential"] = Value::Null;
    plan
}

fn main() {
    let root = PathBuf::from("/private/tmp/ai-guard-broker-checkpoint-v1-golden-3e2cffa/journal-verified");
    let plan = legacy_plan();
    let intent = ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&plan).unwrap()).unwrap();
    let operation = intent.operation_id();
    let initial = CheckpointPayloadV1::initial(&intent).unwrap();
    let namespace = Uuid::from_u128(0x22222222222242228222222222222222);
    let scope = JournalScopeV1::new(
        Uuid::from_u128(0x11111111111141118111111111111111),
        "tenant-checkpoint-test",
        "http://127.0.0.1:8787",
        namespace,
        "broker-test",
    ).unwrap();
    let directory = Directory::open_existing(&root).unwrap();
    let journal = Journal::open(
        &directory, scope, &FsExclusions::new(), JournalLimitsV1::default(),
        ControlSecrets::new(Vec::new()).unwrap(),
    ).unwrap();
    let ticket = match journal.reserve(operation, br#"{"action":"submit","request":"checkpoint-v2"}"#).unwrap() {
        ReserveResult::New(ticket) => ticket,
        _ => panic!("fresh reservation required"),
    };
    let accepted = journal.accept(ticket, &intent, BindingInputs::None, &initial).unwrap();
    let scope_path = fs::read_dir(root.join("broker-journal-v1")).unwrap().next().unwrap().unwrap().path();
    let rev0 = scope_path.join("jobs").join(operation.to_string()).join("rev-00000000000000000000.json");
    let bytes = fs::read(rev0).unwrap();
    let output = PathBuf::from("/private/tmp/ai-guard-broker-checkpoint-v1-golden-3e2cffa/rev0.json");
    fs::write(&output, bytes).unwrap();
    fs::set_permissions(&output, fs::Permissions::from_mode(0o600)).unwrap();
    let head = accepted.head().hash();
    let head_path = PathBuf::from("/private/tmp/ai-guard-broker-checkpoint-v1-golden-3e2cffa/rev0-head.sha256");
    fs::write(&head_path, format!("{head}\n")).unwrap();
    fs::set_permissions(&head_path, fs::Permissions::from_mode(0o600)).unwrap();
    println!("rev0_head={head}");
}
