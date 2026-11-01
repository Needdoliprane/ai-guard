use agentsandbox::{
    audit::Store,
    broker::{
        checkpoint::{
            BrokerCheckpointV2, DecisionAnswerV1, SupervisionCheckpointV1, SupervisionOperationV1,
        },
        intent::ExecutionIntentV1,
        journal::{
            AcceptedJobV1, CheckpointReceiptV1, Journal, JournalEntryKindV1, JournalEntryV1,
            JournalError, JournalPageV1, JournalScopeV1, JournalStatusV1, RejectionCodeV1,
            RejectionPhaseV1, RejectionReceiptV1, ReservationTicket, ReserveResult, RevisionRefV1,
        },
        lifecycle::{
            BrokerCheckpointV3, LocalCommandReceiptV1, LocalCommandRequestV1, LocalCommandV1,
            LocalFailureCodeV1, LocalFailurePhaseV1,
        },
    },
    control::{
        AdmissionResultV1, AdmissionV1, DecisionPhaseV1, DecisionReceiptV1, NetworkV1,
        QuotaPolicyV1, RestartPolicyV1, RuntimeKindV1, RuntimeStatusV1, SecurityPresetV1,
        SupervisedAdmissionV1, SupervisionProtocolV1, WorkerMutationV1, WorkerStateV1,
        WorkerTransitionV1,
    },
    control_fs::Directory,
    model::{Decision, DecisionRequest, Outcome},
    policy::{self, Policy},
    redaction::Redactor,
    runner::CheckpointPayloadV1,
};
use serde_json::{Value, json};
use std::{fs, os::unix::fs::PermissionsExt};
use tempfile::TempDir;
use uuid::Uuid;

fn legacy_plan_value(profile: &str) -> Value {
    let mut plan: Value = serde_json::from_str(include_str!("fixtures/broker_intent-plan-v3.json"))
        .expect("typed private plan fixture");
    plan["version"] = json!(1);
    plan["options"]["profile"] = json!(profile);
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
    plan["request"]["profile"] = json!(profile);
    plan["request"]["agent_id"] = json!("generic-22222222-2222-4222-8222-222222222222");
    let argv = if profile == "review" {
        json!(["cargo", "test"])
    } else {
        json!(["/usr/bin/id"])
    };
    plan["options"]["argv"] = argv.clone();
    plan["request"]["command"]["argv"] = argv;
    plan["request"]["session_id"] = json!("33333333-3333-4333-8333-333333333333");
    plan["session"] = json!("33333333-3333-4333-8333-333333333333");
    plan["request"]
        .as_object_mut()
        .unwrap()
        .remove("provider_resolution");
    plan["provider_input"] = Value::Null;
    plan["provider_credential"] = Value::Null;
    plan
}

/// Golden observed before the canonical-byte factorization. This public
/// fixture performs no Journal, Store or runtime IO.
#[test]
fn canonical_v3_public_fixture_baseline_and_error_order() {
    use sha2::{Digest, Sha256};

    let plan = legacy_plan_value("strict");
    let intent = ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&plan).unwrap()).unwrap();
    let runner = CheckpointPayloadV1::initial(&intent).unwrap();
    let broker = v2(
        runner,
        Uuid::from_u128(0x22222222222242228222222222222222),
        vec![],
    );
    let command = LocalCommandV1::new(
        intent.operation_id(),
        &intent.intent_hash().unwrap(),
        Uuid::from_u128(0x66666666666646668666666666666666),
        LocalCommandRequestV1::FailureObserved {
            phase: LocalFailurePhaseV1::Worker,
            code: LocalFailureCodeV1::Unavailable,
        },
    )
    .unwrap();
    let checkpoint = BrokerCheckpointV3::new(broker.clone(), vec![command.clone()]).unwrap();
    let bytes = checkpoint.canonical_bytes().unwrap();
    assert_eq!(bytes.len(), 922);
    assert_eq!(
        hex::encode(Sha256::digest(&bytes)),
        "ef72f1658b25c8b2ac538eab2d3ba4b8ec8cbb0cde84970395e3b6dc5f3af206"
    );
    assert_eq!(checkpoint.canonical_bytes().unwrap(), bytes);
    assert_eq!(
        BrokerCheckpointV3::from_bytes(&bytes)
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
            value["commands"][0]["request"]["unexpected"] = Value::Null;
            value
        },
    ] {
        assert_eq!(
            BrokerCheckpointV3::from_bytes(&serde_json::to_vec(&invalid).unwrap()).unwrap_err(),
            JournalError::InvalidInput
        );
    }
    let mut trailing = bytes;
    trailing.extend_from_slice(b" {}");
    assert_eq!(
        BrokerCheckpointV3::from_bytes(&trailing).unwrap_err(),
        JournalError::InvalidInput
    );
    assert_eq!(
        BrokerCheckpointV3::new(broker.clone(), vec![command.clone(); 2]).unwrap_err(),
        JournalError::Conflict
    );
    // Cardinality must precede duplicate command checks.
    assert_eq!(
        BrokerCheckpointV3::new(broker, vec![command; 65]).unwrap_err(),
        JournalError::LimitExceeded
    );
    let oversized_input = vec![b' '; 32 * 1024 * 1024 + 1];
    assert_eq!(
        BrokerCheckpointV3::from_bytes(&oversized_input).unwrap_err(),
        JournalError::TooLarge
    );
}

struct LifecycleHarness {
    _root: TempDir,
    store: Store,
    journal: Journal,
    intent: ExecutionIntentV1,
    request: DecisionRequest,
    initial: CheckpointPayloadV1,
    namespace: Uuid,
}

impl LifecycleHarness {
    fn new(profile: &str) -> Self {
        let root = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let journal_root = root.path().join("journal");
        let store_root = root.path().join("store");
        fs::create_dir(&journal_root).unwrap();
        fs::create_dir(&store_root).unwrap();
        fs::set_permissions(&journal_root, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&store_root, fs::Permissions::from_mode(0o700)).unwrap();
        let store = Store::open(
            &store_root.join("audit.sqlite"),
            Redactor::new(vec![]),
            1000,
        )
        .unwrap();
        store
            .migrate_control_v3(&store_root.join("migration-backup.sqlite"))
            .unwrap();
        let namespace = Uuid::parse_str(
            store
                .supervision_state()
                .unwrap()
                .control_namespace_uuid
                .as_deref()
                .expect("migrated Store provides its real namespace"),
        )
        .unwrap();
        let plan = legacy_plan_value(profile);
        let request = serde_json::from_value(plan["request"].clone()).unwrap();
        let intent = ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&plan).unwrap())
            .expect("valid no-credential legacy intent");
        let initial = CheckpointPayloadV1::initial(&intent).unwrap();
        let scope = JournalScopeV1::new(
            Uuid::from_u128(0x11111111111141118111111111111111),
            "tenant-broker-lifecycle-test",
            "http://127.0.0.1:8787",
            namespace,
            "broker-lifecycle-test",
        )
        .unwrap();
        let journal_root = journal_root.canonicalize().unwrap();
        let journal = Journal::open(
            &Directory::open_existing(&journal_root).unwrap(),
            scope,
            &agentsandbox::broker::journal::FsExclusions::new(),
            Default::default(),
            agentsandbox::broker::journal::ControlSecrets::new(Vec::new()).unwrap(),
        )
        .unwrap();
        Self {
            _root: root,
            store,
            journal,
            intent,
            request,
            initial,
            namespace,
        }
    }

    fn accept(&self, submit: &[u8]) -> AcceptedJobV1 {
        let operation_id = self.intent.operation_id();
        let ticket = match self.journal.reserve(operation_id, submit).unwrap() {
            ReserveResult::New(ticket) => ticket,
            other => panic!("expected a fresh operation reservation, got {other:?}"),
        };
        self.journal
            .accept(
                ticket,
                &self.intent,
                agentsandbox::broker::journal::BindingInputs::None,
                &self.initial,
            )
            .unwrap()
    }

    fn stored_decision(&self, request: &DecisionRequest) -> (Decision, DecisionReceiptV1) {
        let policy = Policy::default();
        let decision = policy::evaluate(&policy, request);
        let stored = self
            .store
            .decision(request, Some((decision.clone(), json!(null))))
            .unwrap()
            .unwrap();
        let receipt = DecisionReceiptV1 {
            version: 1,
            request_id: request.request_id.clone(),
            request_hash: policy::request_hash(request),
            phase: if request.human_approved {
                DecisionPhaseV1::Approved
            } else {
                DecisionPhaseV1::Initial
            },
            audit_seq: stored.seq,
            audit_hash: stored.hash,
        };
        (decision, receipt)
    }
}

fn v2(
    runner: CheckpointPayloadV1,
    namespace: Uuid,
    operations: Vec<SupervisionOperationV1>,
) -> BrokerCheckpointV2 {
    BrokerCheckpointV2::new(
        runner,
        SupervisionCheckpointV1::new(namespace, Vec::new(), operations).unwrap(),
    )
    .unwrap()
}

fn local_command(
    harness: &LifecycleHarness,
    command_id: Uuid,
    request: LocalCommandRequestV1,
) -> LocalCommandV1 {
    let intent_hash = harness.intent.intent_hash().unwrap();
    LocalCommandV1::new(
        harness.intent.operation_id(),
        &intent_hash,
        command_id,
        request,
    )
    .unwrap()
}

fn supervised_admission(
    harness: &LifecycleHarness,
    request: &DecisionRequest,
    receipt: DecisionReceiptV1,
) -> SupervisedAdmissionV1 {
    let workspace_manifest_digest = agentsandbox::control::content_hash(
        "ai-guard:workspace-manifest:v1",
        &serde_json::Map::<String, Value>::new(),
    )
    .unwrap();
    SupervisedAdmissionV1 {
        version: 1,
        protocol: SupervisionProtocolV1::BrokerSupervisionV1,
        intent_hash: harness.intent.intent_hash().unwrap(),
        admission: AdmissionV1 {
            version: 1,
            operation_id: harness.intent.operation_id().to_string(),
            decision_request: request.clone(),
            receipt,
            launch: agentsandbox::control::LaunchSpecV1 {
                version: 1,
                session_id: request.session_id.clone().unwrap(),
                command: request.command.clone(),
                profile: request.profile.clone(),
                runtime_kind: RuntimeKindV1::Docker,
                authority_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".into(),
                image_ref: "agentsandbox-worker:local".into(),
                image_id: format!("sha256:{}", "a".repeat(64)),
                network: NetworkV1::None,
                cpu_millis: 2000,
                memory_bytes: 1_073_741_824,
                pids_limit: 256,
                timeout_seconds: 600,
                security_preset: SecurityPresetV1::FixedSandboxV2,
                restart_policy: RestartPolicyV1::No,
                workspace_manifest_digest,
                workspace_import_policy_version: 1,
                provider: "generic".into(),
                capture_output: false,
                credential_binding_id: None,
                credential_names: Vec::new(),
                provider_resolution: None,
            },
        },
    }
}

fn decision_op(
    request: DecisionRequest,
    response: Option<DecisionAnswerV1>,
) -> SupervisionOperationV1 {
    SupervisionOperationV1::Decision { request, response }
}

#[test]
fn rejection_is_durable_idempotent_and_visible_in_status_and_pages() {
    let harness = LifecycleHarness::new("strict");
    let operation_id = harness.intent.operation_id();
    let submit = br#"{"action":"preparation-failed"}"#;
    let ticket = match harness.journal.reserve(operation_id, submit).unwrap() {
        ReserveResult::New(ticket) => ticket,
        other => panic!("expected a fresh reservation, got {other:?}"),
    };
    assert!(matches!(
        harness.journal.status(operation_id).unwrap(),
        Some(JournalStatusV1::ReservedInProcess)
    ));

    let rejected = harness
        .journal
        .reject(
            ticket,
            RejectionPhaseV1::SubmissionPreparation,
            RejectionCodeV1::PreparationFailed,
        )
        .unwrap();
    assert_eq!(rejected.operation_id(), operation_id);
    assert_eq!(rejected.submit_hash().len(), 64);
    assert_eq!(rejected.phase(), RejectionPhaseV1::SubmissionPreparation);
    assert_eq!(rejected.code(), RejectionCodeV1::PreparationFailed);
    assert_eq!(rejected.record_hash().len(), 64);

    let scope_path = fs::read_dir(harness._root.path().join("journal/broker-journal-v1"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let job = scope_path.join("jobs").join(operation_id.to_string());
    assert!(job.join("rejected.json").is_file());
    for absent in [
        "accepted.json",
        "intent.json",
        "bindings.json",
        "current.json",
    ] {
        assert!(!job.join(absent).exists(), "rejected job has no {absent}");
    }

    let replay = match harness.journal.reserve(operation_id, submit).unwrap() {
        ReserveResult::Rejected(receipt) => receipt,
        other => panic!("exact rejected replay returned {other:?}"),
    };
    assert_eq!(replay.record_hash(), rejected.record_hash());
    assert!(matches!(
        harness.journal.reserve(operation_id, b"different submit"),
        Err(JournalError::Conflict)
    ));
    assert!(matches!(
        harness.journal.status(operation_id).unwrap(),
        Some(JournalStatusV1::Rejected(receipt))
            if receipt.record_hash() == rejected.record_hash()
    ));

    let page = harness.journal.list(None, 10).unwrap();
    assert_eq!(page.entries().len(), 1);
    let entry = &page.entries()[0];
    assert_eq!(entry.operation_id(), operation_id);
    assert_eq!(entry.kind(), JournalEntryKindV1::Rejected);
    assert!(entry.head().is_none());
    assert!(!entry.cancel_requested());
    assert!(!entry.approval_submitted());
    assert_eq!(entry.failure_count(), 0);
    assert_eq!(entry.pending_rpc_count(), None);
    assert!(entry.worker_state().is_none());
    assert!(entry.quota_held().is_none());
    assert!(entry.runtime_status().is_none());
    assert!(entry.last_observed_at().is_none());
    assert!(page.next_after().is_none());
    assert!(matches!(
        harness.journal.list(None, 0),
        Err(JournalError::InvalidInput)
    ));

    let root_path = harness._root.path().to_path_buf();
    let namespace = harness.namespace;
    let LifecycleHarness { journal, .. } = harness;
    drop(journal);
    let journal_root = root_path.join("journal").canonicalize().unwrap();
    let scope = JournalScopeV1::new(
        Uuid::from_u128(0x11111111111141118111111111111111),
        "tenant-broker-lifecycle-test",
        "http://127.0.0.1:8787",
        namespace,
        "broker-lifecycle-test",
    )
    .unwrap();
    let reopened = Journal::open(
        &Directory::open_existing(&journal_root).unwrap(),
        scope,
        &agentsandbox::broker::journal::FsExclusions::new(),
        Default::default(),
        agentsandbox::broker::journal::ControlSecrets::new(Vec::new()).unwrap(),
    )
    .unwrap();
    let rejected_path = job.join("rejected.json");
    let rejected_bytes = fs::read(&rejected_path).unwrap();
    assert!(matches!(
        reopened.status(operation_id).unwrap(),
        Some(JournalStatusV1::Rejected(receipt))
            if receipt.record_hash() == rejected.record_hash()
    ));
    let saved_marker = root_path.join("rejected-marker.saved");
    fs::rename(&rejected_path, &saved_marker).unwrap();
    assert!(matches!(
        reopened.status(operation_id),
        Err(JournalError::Conflict)
    ));
    fs::rename(&saved_marker, &rejected_path).unwrap();

    // A dual terminal marker is corruption; the synthetic accepted marker is
    // removed after observation, while the original rejection stays intact.
    let accepted_marker = job.join("accepted.json");
    fs::write(&accepted_marker, b"{}").unwrap();
    fs::set_permissions(&accepted_marker, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(matches!(
        reopened.status(operation_id),
        Err(JournalError::Corrupt)
    ));
    fs::remove_file(job.join("accepted.json")).unwrap();

    // A substituted rejection remains a conflict against the remembered
    // durable floor. Keep and restore the exact original bytes.
    let mut substituted: Value = serde_json::from_slice(&rejected_bytes).unwrap();
    substituted["code"] = json!("cancelled_before_acceptance");
    fs::write(&rejected_path, serde_json::to_vec(&substituted).unwrap()).unwrap();
    assert!(matches!(
        reopened.status(operation_id),
        Err(JournalError::Conflict)
    ));
    fs::write(&rejected_path, &rejected_bytes).unwrap();
    assert_eq!(
        fs::read(&rejected_path).unwrap(),
        rejected_bytes,
        "the original rejection evidence is restored byte-for-byte"
    );

    // Interrupted partial acceptance is never converted into a durable
    // rejection; the test-owned marker remains available for inspection.
    let partial_operation = Uuid::new_v4();
    let partial_submit = br#"{"action":"partial-accept-test"}"#;
    let partial_ticket = match reopened.reserve(partial_operation, partial_submit).unwrap() {
        ReserveResult::New(ticket) => ticket,
        other => panic!("expected a fresh partial-accept fixture, got {other:?}"),
    };
    let partial_job = scope_path.join("jobs").join(partial_operation.to_string());
    let partial_marker = b"synthetic partial acceptance marker";
    let partial_accepted = partial_job.join("accepted.json");
    fs::write(&partial_accepted, partial_marker).unwrap();
    fs::set_permissions(&partial_accepted, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(matches!(
        reopened.reject(
            partial_ticket,
            RejectionPhaseV1::SubmissionPreparation,
            RejectionCodeV1::PreparationFailed
        ),
        Err(JournalError::Interrupted)
    ));
    assert_eq!(
        fs::read(partial_job.join("accepted.json")).unwrap(),
        partial_marker
    );
    assert!(!partial_job.join("rejected.json").exists());
}

#[test]
fn rejection_enum_deserialization_errors_do_not_echo_unknown_values() {
    let phase_canary = "private-rejection-phase-canary-7f92";
    let phase_error = serde_json::from_str::<RejectionPhaseV1>(&format!("\"{phase_canary}\""))
        .unwrap_err()
        .to_string();
    assert!(!phase_error.contains(phase_canary));

    let code_canary = "private-rejection-code-canary-24bd";
    let code_error = serde_json::from_str::<RejectionCodeV1>(&format!("\"{code_canary}\""))
        .unwrap_err()
        .to_string();
    assert!(!code_error.contains(code_canary));
}

#[test]
fn lifecycle_wire_context_replay_caps_and_legacy_approval_boundaries() {
    let harness = LifecycleHarness::new("strict");
    let operation_id = harness.intent.operation_id();
    let accepted = harness.accept(br#"{"action":"lifecycle-boundaries-a"}"#);
    let mut auto_approval_plan = legacy_plan_value("strict");
    let unapproved_request: DecisionRequest =
        serde_json::from_value(auto_approval_plan["request"].clone()).unwrap();
    auto_approval_plan["request"]["human_approved"] = json!(true);
    auto_approval_plan["request"]["approval_hash"] =
        json!(policy::request_hash(&unapproved_request));
    let approved_request: DecisionRequest =
        serde_json::from_value(auto_approval_plan["request"].clone()).unwrap();
    let approved_runner = CheckpointPayloadV1::from_plan_bytes(
        &harness.intent,
        &serde_json::to_vec(&auto_approval_plan).unwrap(),
    )
    .unwrap();
    let approved_pending_v2 = v2(
        approved_runner,
        harness.namespace,
        vec![decision_op(approved_request, None)],
    );
    let auto_approved_v3 = BrokerCheckpointV3::new(approved_pending_v2, Vec::new()).unwrap();
    assert!(matches!(
        harness
            .journal
            .advance_lifecycle(operation_id, accepted.head().clone(), &auto_approved_v3),
        Err(JournalError::Conflict)
    ));
    assert_eq!(
        harness
            .journal
            .load(operation_id)
            .unwrap()
            .unwrap()
            .head()
            .revision(),
        accepted.head().revision(),
        "a pending Approved decision cannot replace the missing ApprovalSubmitted command"
    );
    let v1_receipt = harness
        .journal
        .advance(operation_id, accepted.head().clone(), &harness.initial)
        .unwrap();
    assert_eq!(v1_receipt.head().revision(), accepted.head().revision() + 1);
    let v2_initial = v2(
        harness.initial.clone(),
        harness.namespace,
        vec![decision_op(harness.request.clone(), None)],
    );
    let rev1 = harness
        .journal
        .advance_supervised(operation_id, v1_receipt.head().clone(), &v2_initial)
        .unwrap();
    let failure = |id| {
        local_command(
            &harness,
            id,
            LocalCommandRequestV1::FailureObserved {
                phase: LocalFailurePhaseV1::Admission,
                code: LocalFailureCodeV1::Unavailable,
            },
        )
    };
    let command1 = failure(Uuid::new_v4());
    let v3_one = BrokerCheckpointV3::new(v2_initial.clone(), vec![command1.clone()]).unwrap();
    let rev2 = harness
        .journal
        .advance_lifecycle(operation_id, rev1.head().clone(), &v3_one)
        .unwrap();

    let bytes = v3_one.canonical_bytes().unwrap();
    assert_eq!(
        BrokerCheckpointV3::from_bytes(&bytes)
            .unwrap()
            .canonical_bytes()
            .unwrap(),
        bytes
    );
    let mut unknown_nullable: Value = serde_json::from_slice(&bytes).unwrap();
    unknown_nullable["unknown_private_canary"] = Value::Null;
    let error = BrokerCheckpointV3::from_bytes(&serde_json::to_vec(&unknown_nullable).unwrap())
        .unwrap_err();
    assert_eq!(error, JournalError::InvalidInput);
    assert!(!error.to_string().contains("unknown_private_canary"));
    let direct_error = serde_json::from_value::<BrokerCheckpointV3>(unknown_nullable.clone())
        .unwrap_err()
        .to_string();
    assert!(!direct_error.contains("unknown_private_canary"));
    let mut bad_version: Value = serde_json::from_slice(&bytes).unwrap();
    bad_version["version"] = json!(4);
    assert!(BrokerCheckpointV3::from_bytes(&serde_json::to_vec(&bad_version).unwrap()).is_err());
    let mut bad_nested: Value = serde_json::from_slice(&bytes).unwrap();
    bad_nested["commands"][0]["request"]["unknown_private_canary"] = Value::Null;
    let error =
        BrokerCheckpointV3::from_bytes(&serde_json::to_vec(&bad_nested).unwrap()).unwrap_err();
    assert_eq!(error, JournalError::InvalidInput);
    assert!(!error.to_string().contains("unknown_private_canary"));
    assert!(matches!(
        BrokerCheckpointV3::from_bytes(&bytes[..bytes.len() - 1]),
        Err(JournalError::InvalidInput)
    ));
    let mut trailing = bytes.clone();
    trailing.extend_from_slice(b" {}");
    assert!(matches!(
        BrokerCheckpointV3::from_bytes(&trailing),
        Err(JournalError::InvalidInput)
    ));

    let mut unknown_command_version: Value = serde_json::from_slice(&bytes).unwrap();
    unknown_command_version["commands"][0]["version"] = json!(2);
    assert!(
        BrokerCheckpointV3::from_bytes(&serde_json::to_vec(&unknown_command_version).unwrap())
            .is_err()
    );
    let cancel_command = local_command(
        &harness,
        Uuid::new_v4(),
        LocalCommandRequestV1::CancelRequested,
    );
    let cancel_wire = BrokerCheckpointV3::new(v2_initial.clone(), vec![cancel_command]).unwrap();
    let mut unknown_cancel: Value =
        serde_json::from_slice(&cancel_wire.canonical_bytes().unwrap()).unwrap();
    unknown_cancel["commands"][0]["request"]["private_null_canary"] = Value::Null;
    let unknown_cancel_bytes = serde_json::to_vec(&unknown_cancel).unwrap();
    assert!(matches!(
        BrokerCheckpointV3::from_bytes(&unknown_cancel_bytes),
        Err(JournalError::InvalidInput)
    ));
    let direct_cancel_error = serde_json::from_slice::<BrokerCheckpointV3>(&unknown_cancel_bytes)
        .unwrap_err()
        .to_string();
    assert!(!direct_cancel_error.contains("private_null_canary"));

    let command2 = failure(Uuid::new_v4());
    let v3_two =
        BrokerCheckpointV3::new(v2_initial.clone(), vec![command1.clone(), command2.clone()])
            .unwrap();
    let rev3 = harness
        .journal
        .advance_lifecycle(operation_id, rev2.head().clone(), &v3_two)
        .unwrap();
    let replay = harness
        .journal
        .advance_lifecycle(operation_id, rev1.head().clone(), &v3_one)
        .expect("an exact historical V3 replay returns its original receipt");
    assert_eq!(replay.operation_id(), operation_id);
    assert_eq!(replay.head().revision(), rev2.head().revision());
    assert_eq!(replay.head().hash(), rev2.head().hash());
    assert_eq!(
        harness
            .journal
            .load(operation_id)
            .unwrap()
            .unwrap()
            .head()
            .revision(),
        rev3.head().revision()
    );
    let wrong_intent_command = LocalCommandV1::new(
        operation_id,
        &"f".repeat(64),
        Uuid::new_v4(),
        LocalCommandRequestV1::FailureObserved {
            phase: LocalFailurePhaseV1::Admission,
            code: LocalFailureCodeV1::Unavailable,
        },
    )
    .unwrap();
    let forged = BrokerCheckpointV3::new(
        v2_initial.clone(),
        vec![command1.clone(), command2.clone(), wrong_intent_command],
    )
    .unwrap();
    assert!(matches!(
        harness
            .journal
            .advance_lifecycle(operation_id, rev3.head().clone(), &forged),
        Err(JournalError::Conflict)
    ));
    assert!(matches!(
        harness
            .journal
            .advance_supervised(operation_id, v1_receipt.head().clone(), &v2_initial),
        Err(JournalError::Conflict)
    ));
    assert!(matches!(
        harness
            .journal
            .advance(operation_id, accepted.head().clone(), &harness.initial),
        Err(JournalError::Conflict)
    ));

    let second_operation = Uuid::new_v4();
    let mut plan = legacy_plan_value("strict");
    plan["operation_id"] = json!(second_operation.to_string());
    let second_intent =
        ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&plan).unwrap()).unwrap();
    let second_request: DecisionRequest = serde_json::from_value(plan["request"].clone()).unwrap();
    let second_initial = CheckpointPayloadV1::initial(&second_intent).unwrap();
    let second_ticket = match harness
        .journal
        .reserve(second_operation, br#"{"action":"lifecycle-boundaries-b"}"#)
        .unwrap()
    {
        ReserveResult::New(ticket) => ticket,
        other => panic!("expected second operation reservation, got {other:?}"),
    };
    let second_accepted = harness
        .journal
        .accept(
            second_ticket,
            &second_intent,
            agentsandbox::broker::journal::BindingInputs::None,
            &second_initial,
        )
        .unwrap();
    let second_v2 = v2(
        second_initial,
        harness.namespace,
        vec![decision_op(second_request, None)],
    );
    let second_rev1 = harness
        .journal
        .advance_supervised(second_operation, second_accepted.head().clone(), &second_v2)
        .unwrap();
    let cross_job = BrokerCheckpointV3::new(second_v2.clone(), vec![command1.clone()]).unwrap();
    assert!(matches!(
        harness
            .journal
            .advance_lifecycle(second_operation, second_rev1.head().clone(), &cross_job),
        Err(JournalError::Conflict)
    ));
    assert_eq!(
        harness
            .journal
            .load(second_operation)
            .unwrap()
            .unwrap()
            .head()
            .revision(),
        second_rev1.head().revision()
    );

    let mut many = vec![command1.clone(), command2.clone()];
    many.extend((0..62).map(|_| failure(Uuid::new_v4())));
    let bounded = BrokerCheckpointV3::new(v2_initial.clone(), many.clone()).unwrap();
    let bounded_receipt = harness
        .journal
        .advance_lifecycle(operation_id, rev3.head().clone(), &bounded)
        .unwrap();
    let mut too_many = many.clone();
    too_many.push(failure(Uuid::new_v4()));
    assert!(matches!(
        BrokerCheckpointV3::new(v2_initial.clone(), too_many),
        Err(JournalError::LimitExceeded)
    ));
    assert_eq!(
        harness
            .journal
            .load(operation_id)
            .unwrap()
            .unwrap()
            .head()
            .revision(),
        bounded_receipt.head().revision()
    );

    // Grandfather V1 approval through either the direct V1→V3 promotion or
    // the V1→V2→V3 path. Then replay the exact committed V1/V2 transitions
    // after V3 and require the downgrade guard to reject them.
    for via_v2 in [false, true] {
        let approved_harness = LifecycleHarness::new("strict");
        let approved_operation = approved_harness.intent.operation_id();
        let approved_job =
            approved_harness.accept(br#"{"action":"lifecycle-approved-grandfather"}"#);
        let mut approved_plan = legacy_plan_value("strict");
        let unapproved: DecisionRequest =
            serde_json::from_value(approved_plan["request"].clone()).unwrap();
        approved_plan["request"]["human_approved"] = json!(true);
        approved_plan["request"]["approval_hash"] = json!(policy::request_hash(&unapproved));
        let approved_v1 = CheckpointPayloadV1::from_plan_bytes(
            &approved_harness.intent,
            &serde_json::to_vec(&approved_plan).unwrap(),
        )
        .unwrap();
        let approved_v1_receipt = approved_harness
            .journal
            .advance(
                approved_operation,
                approved_job.head().clone(),
                &approved_v1,
            )
            .unwrap();
        let approved_v2 = v2(approved_v1.clone(), approved_harness.namespace, Vec::new());
        let approved_v3 = BrokerCheckpointV3::new(approved_v2.clone(), Vec::new()).unwrap();
        let v3_receipt = if via_v2 {
            let approved_v2_receipt = approved_harness
                .journal
                .advance_supervised(
                    approved_operation,
                    approved_v1_receipt.head().clone(),
                    &approved_v2,
                )
                .unwrap();
            approved_harness
                .journal
                .advance_lifecycle(
                    approved_operation,
                    approved_v2_receipt.head().clone(),
                    &approved_v3,
                )
                .expect("an already-approved V2 checkpoint is grandfathered")
        } else {
            approved_harness
                .journal
                .advance_lifecycle(
                    approved_operation,
                    approved_v1_receipt.head().clone(),
                    &approved_v3,
                )
                .expect("an already-approved V1 checkpoint promotes directly to V3")
        };
        assert_eq!(v3_receipt.head().revision(), if via_v2 { 3 } else { 2 });
        assert!(matches!(
            approved_harness.journal.advance(
                approved_operation,
                approved_job.head().clone(),
                &approved_v1
            ),
            Err(JournalError::Conflict)
        ));
        if via_v2 {
            assert!(matches!(
                approved_harness.journal.advance_supervised(
                    approved_operation,
                    approved_v1_receipt.head().clone(),
                    &approved_v2
                ),
                Err(JournalError::Conflict)
            ));
        }
    }
}

#[test]
fn approval_command_requires_prior_store_review_ack_and_uses_two_runner_commits() {
    let harness = LifecycleHarness::new("review");
    let operation_id = harness.intent.operation_id();
    let accepted = harness.accept(br#"{"action":"review-approval"}"#);
    let review_request = harness.request.clone();
    let pending = v2(
        harness.initial.clone(),
        harness.namespace,
        vec![decision_op(review_request.clone(), None)],
    );
    let pending_receipt = harness
        .journal
        .advance_supervised(operation_id, accepted.head().clone(), &pending)
        .unwrap();

    let (review_decision, review_receipt) = harness.stored_decision(&review_request);
    assert_eq!(review_decision.outcome, Outcome::Review);
    let review_answer = DecisionAnswerV1::new(review_decision, review_receipt.clone()).unwrap();
    let review_ack = decision_op(review_request.clone(), Some(review_answer));
    let review_ack_v2 = v2(
        harness.initial.clone(),
        harness.namespace,
        vec![review_ack.clone()],
    );
    let approval = local_command(
        &harness,
        Uuid::parse_str("44444444-4444-4444-8444-444444444444").unwrap(),
        LocalCommandRequestV1::ApprovalSubmitted {
            request_hash: policy::request_hash(&review_request),
            review_receipt: review_receipt.clone(),
        },
    );

    let co_born = BrokerCheckpointV3::new(review_ack_v2.clone(), vec![approval.clone()]).unwrap();
    assert!(matches!(
        harness
            .journal
            .advance_lifecycle(operation_id, pending_receipt.head().clone(), &co_born),
        Err(JournalError::Conflict)
    ));
    assert_eq!(
        harness.journal.load(operation_id).unwrap().unwrap().head(),
        pending_receipt.head(),
        "review ACK and approval command cannot acquire causal order in one revision"
    );

    let review_ack_receipt = harness
        .journal
        .advance_supervised(operation_id, pending_receipt.head().clone(), &review_ack_v2)
        .unwrap();
    let approved_command_v3 =
        BrokerCheckpointV3::new(review_ack_v2, vec![approval.clone()]).unwrap();
    let command_receipt = harness
        .journal
        .advance_lifecycle(
            operation_id,
            review_ack_receipt.head().clone(),
            &approved_command_v3,
        )
        .unwrap();
    let historical_command_receipt = harness
        .journal
        .command_receipt(operation_id, approval.command_id())
        .unwrap()
        .unwrap();
    assert_eq!(
        historical_command_receipt.command_id(),
        approval.command_id()
    );
    assert_eq!(
        historical_command_receipt.input_hash(),
        approval.input_hash()
    );
    assert_eq!(historical_command_receipt.head(), command_receipt.head());

    let mut approved_request = review_request.clone();
    approved_request.human_approved = true;
    approved_request.approval_hash = Some(policy::request_hash(&review_request));
    let mut approved_plan = legacy_plan_value("review");
    approved_plan["request"]["human_approved"] = json!(true);
    approved_plan["request"]["approval_hash"] =
        json!(approved_request.approval_hash.as_deref().unwrap());
    let approved_runner = CheckpointPayloadV1::from_plan_bytes(
        &harness.intent,
        &serde_json::to_vec(&approved_plan).unwrap(),
    )
    .unwrap();
    let approved_pending = decision_op(approved_request.clone(), None);
    let approval_pending_v2 = v2(
        approved_runner.clone(),
        harness.namespace,
        vec![review_ack.clone(), approved_pending],
    );
    let approval_pending_v3 =
        BrokerCheckpointV3::new(approval_pending_v2, vec![approval.clone()]).unwrap();
    let approved_pending_receipt = harness
        .journal
        .advance_lifecycle(
            operation_id,
            command_receipt.head().clone(),
            &approval_pending_v3,
        )
        .unwrap();

    let (approved_decision, approved_decision_receipt) = harness.stored_decision(&approved_request);
    assert_eq!(approved_decision.outcome, Outcome::Allow);
    let approved_answer =
        DecisionAnswerV1::new(approved_decision, approved_decision_receipt.clone()).unwrap();
    let approved_decision_ack = decision_op(approved_request.clone(), Some(approved_answer));
    let approved_ack_operations = vec![review_ack.clone(), approved_decision_ack.clone()];
    let approved_ack_v2 = v2(
        approved_runner.clone(),
        harness.namespace,
        approved_ack_operations.clone(),
    );
    let approved_ack_v3 =
        BrokerCheckpointV3::new(approved_ack_v2.clone(), vec![approval.clone()]).unwrap();
    let approved_ack_receipt = harness
        .journal
        .advance_lifecycle(
            operation_id,
            approved_pending_receipt.head().clone(),
            &approved_ack_v3,
        )
        .unwrap();

    let cancel = local_command(
        &harness,
        Uuid::parse_str("77777777-7777-4777-8777-777777777777").unwrap(),
        LocalCommandRequestV1::CancelRequested,
    );
    let cancelled = BrokerCheckpointV3::new(
        approved_ack_v2.clone(),
        vec![approval.clone(), cancel.clone()],
    )
    .unwrap();
    let cancel_receipt = harness
        .journal
        .advance_lifecycle(
            operation_id,
            approved_ack_receipt.head().clone(),
            &cancelled,
        )
        .unwrap();
    let cancellation_removed =
        BrokerCheckpointV3::new(approved_ack_v2.clone(), vec![approval.clone()]).unwrap();
    assert!(matches!(
        harness.journal.advance_lifecycle(
            operation_id,
            cancel_receipt.head().clone(),
            &cancellation_removed,
        ),
        Err(JournalError::Conflict)
    ));
    assert_eq!(
        harness.journal.load(operation_id).unwrap().unwrap().head(),
        cancel_receipt.head(),
        "a durable cancellation cannot be removed by a later revision"
    );

    let accepted = harness.journal.load(operation_id).unwrap().unwrap();
    let checkpoint = accepted
        .broker_checkpoint_v3()
        .expect("V3 command is present after the durable lifecycle transition");
    assert_eq!(checkpoint.commands().len(), 2);
    assert_eq!(
        harness
            .journal
            .command_receipt(operation_id, approval.command_id())
            .unwrap()
            .unwrap()
            .head(),
        command_receipt.head(),
        "replay keeps the command's first confirmed receipt"
    );
    let page = harness.journal.list(None, 10).unwrap();
    let entry = &page.entries()[0];
    assert!(entry.approval_submitted());
    assert!(entry.cancel_requested());
    assert_eq!(entry.pending_rpc_count(), Some(0));
}

#[test]
fn cancel_preserves_a_pending_store_admission_and_its_held_quota() {
    let harness = LifecycleHarness::new("strict");
    let operation_id = harness.intent.operation_id();
    let accepted = harness.accept(br#"{"action":"cancel-pending-admission"}"#);

    let decision_pending = v2(
        harness.initial.clone(),
        harness.namespace,
        vec![decision_op(harness.request.clone(), None)],
    );
    let rev1 = harness
        .journal
        .advance_supervised(operation_id, accepted.head().clone(), &decision_pending)
        .unwrap();
    let (decision, decision_receipt) = harness.stored_decision(&harness.request);
    assert_eq!(decision.outcome, Outcome::Allow);
    let decision_answer =
        DecisionAnswerV1::new(decision.clone(), decision_receipt.clone()).unwrap();
    let decision_ack = decision_op(harness.request.clone(), Some(decision_answer));
    let decision_ack_v2 = v2(
        harness.initial.clone(),
        harness.namespace,
        vec![decision_ack.clone()],
    );
    let rev2 = harness
        .journal
        .advance_supervised(operation_id, rev1.head().clone(), &decision_ack_v2)
        .unwrap();

    let admission = supervised_admission(&harness, &harness.request, decision_receipt);
    let admission_pending = SupervisionOperationV1::Admission {
        request: admission.clone(),
        response: None,
    };
    let pending_operations = vec![decision_ack.clone(), admission_pending];
    let admission_pending_v2 = v2(
        harness.initial.clone(),
        harness.namespace,
        pending_operations.clone(),
    );
    let rev3 = harness
        .journal
        .advance_supervised(operation_id, rev2.head().clone(), &admission_pending_v2)
        .unwrap();

    // The control Store performs the real reservation after the journal records
    // the RPC as pending. Its response is intentionally not ACKed yet.
    let stored_admission: AdmissionResultV1 = harness
        .store
        .admit_supervised(
            &admission,
            &Policy::default(),
            decision,
            json!(null),
            &QuotaPolicyV1::default(),
        )
        .unwrap();
    assert!(stored_admission.accepted);
    let worker = stored_admission.worker.as_ref().unwrap();
    let worker_id = worker.worker_id.clone();
    assert_eq!(worker.state, WorkerStateV1::Reserved);
    assert!(worker.quota_held);
    assert_eq!(
        harness
            .store
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        1
    );

    let cancel = local_command(
        &harness,
        Uuid::parse_str("55555555-5555-4555-8555-555555555555").unwrap(),
        LocalCommandRequestV1::CancelRequested,
    );
    let cancelled_v3 = BrokerCheckpointV3::new(admission_pending_v2, vec![cancel.clone()]).unwrap();
    let cancel_receipt = harness
        .journal
        .advance_lifecycle(operation_id, rev3.head().clone(), &cancelled_v3)
        .unwrap();

    let failure = local_command(
        &harness,
        Uuid::parse_str("66666666-6666-4666-8666-666666666666").unwrap(),
        LocalCommandRequestV1::FailureObserved {
            phase: LocalFailurePhaseV1::Admission,
            code: LocalFailureCodeV1::Unavailable,
        },
    );
    let failure_v3 = BrokerCheckpointV3::new(
        v2(
            harness.initial.clone(),
            harness.namespace,
            pending_operations.clone(),
        ),
        vec![cancel.clone(), failure.clone()],
    )
    .unwrap();
    let failure_receipt = harness
        .journal
        .advance_lifecycle(operation_id, cancel_receipt.head().clone(), &failure_v3)
        .unwrap();
    let pending_page = harness.journal.list(None, 10).unwrap();
    let pending_entry = &pending_page.entries()[0];
    assert!(pending_entry.cancel_requested());
    assert_eq!(pending_entry.failure_count(), 1);
    assert_eq!(pending_entry.pending_rpc_count(), Some(1));

    let admission_ack_v2 = v2(
        harness.initial.clone(),
        harness.namespace,
        vec![
            decision_ack.clone(),
            SupervisionOperationV1::Admission {
                request: admission.clone(),
                response: Some(stored_admission.clone()),
            },
        ],
    );
    let admission_ack_v3 =
        BrokerCheckpointV3::new(admission_ack_v2, vec![cancel.clone(), failure.clone()]).unwrap();
    let admission_ack_receipt = harness
        .journal
        .advance_lifecycle(
            operation_id,
            failure_receipt.head().clone(),
            &admission_ack_v3,
        )
        .expect("cancel cannot erase the earlier pending admission RPC");
    assert_eq!(
        harness
            .store
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        1,
        "a local failure and cancel do not release the Store's reservation"
    );
    let stored_worker = harness.store.worker(&worker_id).unwrap().unwrap();
    assert_eq!(stored_worker.state, WorkerStateV1::Reserved);
    assert!(stored_worker.quota_held);
    let final_page = harness.journal.list(None, 10).unwrap();
    assert!(final_page.entries()[0].cancel_requested());
    assert_eq!(final_page.entries()[0].failure_count(), 1);
    assert_eq!(final_page.entries()[0].pending_rpc_count(), Some(0));

    let cancel_mutation = WorkerMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        expected_revision: worker.revision,
        transition: WorkerTransitionV1::CancelReserved,
    };
    let cancel_pending_operations = vec![
        decision_ack.clone(),
        SupervisionOperationV1::Admission {
            request: admission.clone(),
            response: Some(stored_admission.clone()),
        },
        SupervisionOperationV1::WorkerMutation {
            request: cancel_mutation.clone(),
            response: None,
        },
    ];
    let cancel_pending = BrokerCheckpointV3::new(
        v2(
            harness.initial.clone(),
            harness.namespace,
            cancel_pending_operations.clone(),
        ),
        vec![cancel.clone(), failure.clone()],
    )
    .unwrap();
    let cancel_pending_receipt = harness
        .journal
        .advance_lifecycle(
            operation_id,
            admission_ack_receipt.head().clone(),
            &cancel_pending,
        )
        .expect("CancelReserved can be recorded pending after the real admission ACK");
    assert_eq!(
        harness
            .store
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        1,
        "the pending cancellation has not released real Store quota"
    );
    assert_eq!(
        harness.store.worker(&worker_id).unwrap().unwrap().state,
        WorkerStateV1::Reserved
    );

    let cancelled_result = harness.store.mutate_worker(&cancel_mutation).unwrap();
    assert_eq!(cancelled_result.worker.state, WorkerStateV1::Cancelled);
    assert!(!cancelled_result.worker.quota_held);
    assert_eq!(
        harness
            .store
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        0,
        "only the actual Store cancellation result releases quota"
    );
    let cancel_ack_operations = vec![
        decision_ack,
        SupervisionOperationV1::Admission {
            request: admission,
            response: Some(stored_admission),
        },
        SupervisionOperationV1::WorkerMutation {
            request: cancel_mutation,
            response: Some(cancelled_result),
        },
    ];
    let cancel_ack = BrokerCheckpointV3::new(
        v2(
            harness.initial.clone(),
            harness.namespace,
            cancel_ack_operations,
        ),
        vec![cancel, failure],
    )
    .unwrap();
    let cancel_ack_receipt = harness
        .journal
        .advance_lifecycle(
            operation_id,
            cancel_pending_receipt.head().clone(),
            &cancel_ack,
        )
        .expect("the real CancelReserved receipt can be ACKed");
    assert_eq!(
        harness.journal.list(None, 10).unwrap().entries()[0].pending_rpc_count(),
        Some(0)
    );
    assert_eq!(
        harness
            .store
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        0
    );
    assert_eq!(cancel_ack_receipt.head().revision(), 8);
}

#[test]
fn lifecycle_journal_and_checkpoint_v3_api_matches_the_contract() {
    let _reject: fn(
        &Journal,
        ReservationTicket,
        RejectionPhaseV1,
        RejectionCodeV1,
    ) -> Result<RejectionReceiptV1, JournalError> = Journal::reject;
    let _status: fn(&Journal, Uuid) -> Result<Option<JournalStatusV1>, JournalError> =
        Journal::status;
    let _list: fn(&Journal, Option<Uuid>, u32) -> Result<JournalPageV1, JournalError> =
        Journal::list;
    let _advance_lifecycle: fn(
        &Journal,
        Uuid,
        RevisionRefV1,
        &BrokerCheckpointV3,
    ) -> Result<CheckpointReceiptV1, JournalError> = Journal::advance_lifecycle;
    let _command_receipt: fn(
        &Journal,
        Uuid,
        Uuid,
    ) -> Result<Option<LocalCommandReceiptV1>, JournalError> = Journal::command_receipt;
    let _accepted_v3: fn(&AcceptedJobV1) -> Option<&BrokerCheckpointV3> =
        AcceptedJobV1::broker_checkpoint_v3;

    let _v3_new: fn(
        BrokerCheckpointV2,
        Vec<LocalCommandV1>,
    ) -> Result<BrokerCheckpointV3, JournalError> = BrokerCheckpointV3::new;
    let _v3_from_bytes: fn(&[u8]) -> Result<BrokerCheckpointV3, JournalError> =
        BrokerCheckpointV3::from_bytes;
    let _v3_bytes: fn(&BrokerCheckpointV3) -> Result<Vec<u8>, JournalError> =
        BrokerCheckpointV3::canonical_bytes;
    let _v3_broker: fn(&BrokerCheckpointV3) -> &BrokerCheckpointV2 = BrokerCheckpointV3::broker;
    let _v3_commands: fn(&BrokerCheckpointV3) -> &[LocalCommandV1] = BrokerCheckpointV3::commands;

    let _command_new: fn(
        Uuid,
        &str,
        Uuid,
        LocalCommandRequestV1,
    ) -> Result<LocalCommandV1, JournalError> = LocalCommandV1::new;
    let _command_id: fn(&LocalCommandV1) -> Uuid = LocalCommandV1::command_id;
    let _command_hash: fn(&LocalCommandV1) -> &str = LocalCommandV1::input_hash;
    let _command_request: fn(&LocalCommandV1) -> &LocalCommandRequestV1 = LocalCommandV1::request;
    let _command_receipt_id: fn(&LocalCommandReceiptV1) -> Uuid = LocalCommandReceiptV1::command_id;
    let _command_receipt_hash: fn(&LocalCommandReceiptV1) -> &str =
        LocalCommandReceiptV1::input_hash;
    let _command_receipt_head: fn(&LocalCommandReceiptV1) -> &RevisionRefV1 =
        LocalCommandReceiptV1::head;

    let _page_entries: fn(&JournalPageV1) -> &[JournalEntryV1] = JournalPageV1::entries;
    let _page_cursor: fn(&JournalPageV1) -> Option<Uuid> = JournalPageV1::next_after;
    let _entry_id: fn(&JournalEntryV1) -> Uuid = JournalEntryV1::operation_id;
    let _entry_kind: fn(&JournalEntryV1) -> JournalEntryKindV1 = JournalEntryV1::kind;
    let _entry_head: fn(&JournalEntryV1) -> Option<&RevisionRefV1> = JournalEntryV1::head;
    let _entry_cancel: fn(&JournalEntryV1) -> bool = JournalEntryV1::cancel_requested;
    let _entry_approval: fn(&JournalEntryV1) -> bool = JournalEntryV1::approval_submitted;
    let _entry_failures: fn(&JournalEntryV1) -> usize = JournalEntryV1::failure_count;
    let _entry_pending: fn(&JournalEntryV1) -> Option<usize> = JournalEntryV1::pending_rpc_count;
    let _entry_worker: fn(&JournalEntryV1) -> Option<&WorkerStateV1> = JournalEntryV1::worker_state;
    let _entry_quota: fn(&JournalEntryV1) -> Option<bool> = JournalEntryV1::quota_held;
    let _entry_runtime: fn(&JournalEntryV1) -> Option<&RuntimeStatusV1> =
        JournalEntryV1::runtime_status;
    let _entry_observed: fn(&JournalEntryV1) -> Option<&str> = JournalEntryV1::last_observed_at;

    let _decision_receipt_type: Option<DecisionReceiptV1> = None;
    let _cancel_command = LocalCommandRequestV1::CancelRequested;
    let _reject_phase = RejectionPhaseV1::SubmissionPreparation;
    let _reject_code = RejectionCodeV1::PreparationFailed;
    let _status_interrupted = JournalStatusV1::Interrupted;
    let _status_reserved = JournalStatusV1::ReservedInProcess;
    let _status_rejected: fn(RejectionReceiptV1) -> JournalStatusV1 = JournalStatusV1::Rejected;
    let _status_accepted: fn(AcceptedJobV1) -> JournalStatusV1 = JournalStatusV1::Accepted;
    let _entry_accepted = JournalEntryKindV1::Accepted;
    let _entry_reserved = JournalEntryKindV1::ReservedInProcess;
    let _entry_interrupted = JournalEntryKindV1::Interrupted;
    let _entry_rejected = JournalEntryKindV1::Rejected;
    let _approval_request: fn(String, DecisionReceiptV1) -> LocalCommandRequestV1 =
        |request_hash, review_receipt| LocalCommandRequestV1::ApprovalSubmitted {
            request_hash,
            review_receipt,
        };
    let _failure_request: fn(LocalFailurePhaseV1, LocalFailureCodeV1) -> LocalCommandRequestV1 =
        |phase, code| LocalCommandRequestV1::FailureObserved { phase, code };
    let _ = (
        _reject,
        _status,
        _list,
        _advance_lifecycle,
        _command_receipt,
        _accepted_v3,
        _v3_new,
        _v3_from_bytes,
        _v3_bytes,
        _v3_broker,
        _v3_commands,
        _command_new,
        _command_id,
        _command_hash,
        _command_request,
        _command_receipt_id,
        _command_receipt_hash,
        _command_receipt_head,
        _page_entries,
        _page_cursor,
        _entry_id,
        _entry_kind,
        _entry_head,
        _entry_cancel,
        _entry_approval,
        _entry_failures,
        _entry_pending,
        _entry_worker,
        _entry_quota,
        _entry_runtime,
        _entry_observed,
        _decision_receipt_type,
        _cancel_command,
        _reject_phase,
        _reject_code,
        _status_interrupted,
        _status_reserved,
        _status_rejected,
        _status_accepted,
        _entry_accepted,
        _entry_reserved,
        _entry_interrupted,
        _entry_rejected,
        _approval_request,
        _failure_request,
    );
}
