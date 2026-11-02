//! Real private files and retained owners; fixtures below are durable data,
//! never evidence of actor eligibility, authentication or quiescence.
use super::*;
use serde_json::json;
use std::{cell::Cell, io::Write, os::unix::fs::PermissionsExt};

thread_local! { static FAULT: Cell<Option<FaultStage>> = const { Cell::new(None) }; }
pub(super) fn checkpoint(stage: FaultStage) -> EnforcerResult<()> {
    FAULT.with(|fault| {
        if fault.get() == Some(stage) {
            fault.set(None);
            Err(EnforcerError::Unavailable)
        } else {
            Ok(())
        }
    })
}
fn fail_at(stage: FaultStage) {
    FAULT.with(|fault| fault.set(Some(stage)));
}

fn setup() -> (tempfile::TempDir, Directory, RetirementLedger, Uuid) {
    let files = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap())
        .unwrap();
    let parent = Directory::open_existing(files.path()).unwrap();
    let install = Uuid::new_v4();
    let ledger =
        RetirementLedger::initialize_new(parent.create_enforcer_root("role").unwrap(), install)
            .unwrap();
    (files, parent, ledger, install)
}

fn accepted(installation: Uuid) -> ClosingAcceptanceV1 {
    let (namespace, job, epoch, actor, arm_id, worker, attempt) = (
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    );
    let arm = json!({"version":1,"operation_id":arm_id,"worker_id":worker,
        "attempt_id":attempt,"expected_revision":1,"intent_hash":"e".repeat(64),
        "subject":{"kind":"worker"},"inspection":{
            "authority_id":epoch,"runtime_kind":"docker","container_id":"a".repeat(64),
            "stable_name":"ledger-fixture","volume_name":"ledger-fixture-work",
            "image_id":format!("sha256:{}", "b".repeat(64)),"spec_hash":"d".repeat(64),
            "cpu_millis":1000,"memory_bytes":1073741824_u64,"pids_limit":256,
            "network":"none","security_preset":"fixed_sandbox_v2","restart_policy":"no"},
        "authority_binding_hash":"f".repeat(64),"configuration_fingerprint_version":2,
        "configuration_fingerprint":"c".repeat(64),"clock":{
            "version":1,"method":"darwin_mach_continuous_v1","boot_id":epoch,
            "continuous_ns_decimal":"1","observed_utc":null},
        "timeout_seconds":60,"deadline_ns_decimal":"60000000001"});
    let arm_typed: crate::control::ArmV1 = serde_json::from_value(arm.clone()).unwrap();
    let identity: RetirementIdentityV1 = serde_json::from_value(json!({"version":1,"scope":{
        "version":1,"installation_id":installation,"tenant_id":"tenant-ledger",
        "gateway_origin":"http://127.0.0.1:8787","control_namespace_uuid":namespace,"profile_id":"ledger-test"},
        "job_operation_id":job,"intent_hash":"e".repeat(64),"arm":arm,
        "arm_receipt":{"version":1,"arm_id":arm_id,"worker_id":worker,"attempt_id":attempt,
            "input_hash":arm_typed.input_hash().unwrap(),"control_namespace_uuid":namespace,
            "audit_seq":1,"audit_hash":"b".repeat(64)},
        "registration_hash":"d".repeat(64),"original_epoch":epoch,"generation":0,
        "retirement_id":Uuid::new_v4(),"reason":"unused_prepare_revoked"})).unwrap();
    let evidence =
        RetirementEvidenceV1::new(identity.digest().unwrap(), actor, Uuid::new_v4()).unwrap();
    let ack = RetirementAuditAckV1::new(
        Uuid::new_v4(),
        RetirementDigestV1::parse(&"a".repeat(64)).unwrap(),
        1,
    )
    .unwrap();
    ClosingAcceptanceV1::new(identity, evidence, ack, Uuid::new_v4()).unwrap()
}
fn complete(ledger: &RetirementLedger, revision: u64, arms: u32) {
    match ledger.coverage().unwrap() {
        LedgerCoverageV1::Complete { head, arms: count } => {
            assert_eq!(*head.revision(), revision);
            assert_eq!(count, arms);
        }
        other => panic!("expected complete coverage, got {other:?}"),
    }
}
fn reopen(parent: &Directory, ledger: RetirementLedger) -> RetirementLedger {
    drop(ledger);
    let role = parent.persistent_child("role", false).unwrap().unwrap();
    RetirementLedger::open_existing(EnforcerRoleLeaseV2::acquire_existing(&role).unwrap()).unwrap()
}

#[test]
fn fresh_seal_refuses_legacy_factory_and_reopen_preserves_identity() {
    let (_files, parent, ledger, _) = setup();
    complete(&ledger, 0, 0);
    let role = ledger.lease().ledger_root().clone();
    let binding = ledger.lease().ledger_lock().native_binding().unwrap();
    assert!(parent.create_enforcer_root("role").is_err());
    assert!(crate::broker::discovery::RoleLease::acquire_existing(&role).is_err());
    let before = std::fs::read(role.path().join(".owner.lock")).unwrap();
    assert!(matches!(
        RetirementLedger::open_existing(ledger.lease()),
        Err(EnforcerError::Conflict)
    ));
    let ledger = reopen(&parent, ledger);
    complete(&ledger, 0, 0);
    assert_eq!(
        ledger.lease().ledger_lock().native_binding().unwrap(),
        binding
    );
    assert_eq!(
        std::fs::read(role.path().join(".owner.lock")).unwrap(),
        before
    );
}

#[test]
fn legacy_empty_lock_is_unverified_without_upgrade_or_storage_creation() {
    let (_files, parent, ledger, _) = setup();
    let legacy = parent.persistent_child("legacy", true).unwrap().unwrap();
    let role = crate::broker::discovery::RoleLease::acquire(&legacy).unwrap();
    role.listener_root().unwrap();
    legacy.persistent_child("announcements", true).unwrap();
    drop(role);
    drop(ledger);
    let owner = EnforcerRoleLeaseV2::acquire_existing(&legacy).unwrap();
    let ledger = RetirementLedger::open_existing(owner).unwrap();
    assert!(matches!(
        ledger.coverage().unwrap(),
        LedgerCoverageV1::Unverified
    ));
    assert_eq!(
        std::fs::read(legacy.path().join(".owner.lock")).unwrap(),
        b""
    );
    assert!(
        legacy
            .persistent_child("retirement", false)
            .unwrap()
            .is_none()
    );
}

#[test]
fn closing_tombstone_replay_and_recovery_preserve_exact_committed_bytes() {
    let (_files, parent, ledger, install) = setup();
    let accepted = accepted(install);
    let identity = accepted.identity().clone();
    let closing = ledger.begin_closing(accepted.clone()).unwrap();
    complete(&ledger, 1, 1);
    let tombstone = ledger.commit_tombstone(&closing).unwrap();
    assert_eq!(tombstone.arm_id, closing.arm_id);
    assert!(tombstone.ledger.matches(&closing.ledger).unwrap());
    assert_ne!(tombstone.entry_hash, closing.entry_hash);
    complete(&ledger, 2, 1);
    let bytes = ledger
        .query(&identity)
        .unwrap()
        .unwrap()
        .canonical_bytes()
        .unwrap();
    let replay = ledger.begin_closing(accepted).unwrap();
    ledger.commit_tombstone(&replay).unwrap();
    complete(&ledger, 2, 1);
    let ledger = reopen(&parent, ledger);
    complete(&ledger, 2, 1);
    assert_eq!(
        ledger
            .query(&identity)
            .unwrap()
            .unwrap()
            .canonical_bytes()
            .unwrap(),
        bytes
    );
}

#[test]
fn missing_tombstone_closes_full_but_keeps_positive_entry_and_stop_anchor() {
    let (_files, parent, ledger, install) = setup();
    let closing = ledger.begin_closing(accepted(install)).unwrap();
    ledger.commit_tombstone(&closing).unwrap();
    let lease = ledger.lease();
    let path = lease
        .ledger_root()
        .path()
        .join("retirement/tombstones")
        .join(tombstone_name(closing.arm_id));
    // Only this test-created authority is faulted; no runtime or existing role.
    std::fs::remove_file(&path).unwrap();
    assert!(matches!(
        ledger.coverage().unwrap(),
        LedgerCoverageV1::Partial { .. }
    ));
    assert!(ledger.known_entry(closing.arm_id).unwrap().is_some());
    assert!(lease.revalidate().is_ok());
    drop(lease);
    let ledger = reopen(&parent, ledger);
    assert!(matches!(
        ledger.coverage().unwrap(),
        LedgerCoverageV1::Partial {
            phase: LedgerScanPhaseV1::Tombstones,
            ..
        }
    ));
    assert!(ledger.known_entry(closing.arm_id).unwrap().is_some());
    assert!(!path.exists());
}

#[test]
fn torn_floor_keeps_valid_prefix_without_repair_or_fresh_initialization() {
    let (_files, parent, ledger, install) = setup();
    let closing = ledger.begin_closing(accepted(install)).unwrap();
    let role = ledger.lease().ledger_root().clone();
    let lock = role.path().join(".owner.lock");
    let mut fault = std::fs::OpenOptions::new()
        .append(true)
        .open(&lock)
        .unwrap();
    fault.write_all(&[0, 0, 0, 20, b'{']).unwrap();
    fault.sync_all().unwrap();
    drop(fault);
    let bytes = std::fs::read(&lock).unwrap();
    let ledger = reopen(&parent, ledger);
    assert!(matches!(
        ledger.coverage().unwrap(),
        LedgerCoverageV1::Partial { .. }
    ));
    assert!(ledger.known_entry(closing.arm_id).unwrap().is_some());
    assert!(ledger.begin_closing(accepted(install)).is_err());
    assert!(ledger.lease().revalidate().is_ok());
    assert_eq!(std::fs::read(lock).unwrap(), bytes);
    assert!(parent.create_enforcer_root("role").is_err());
}

#[test]
fn committed_floor_with_lost_pointer_ack_recovers_only_exact_cas() {
    for stage in [FaultStage::FloorSynced, FaultStage::PointerSynced] {
        let (_files, parent, ledger, install) = setup();
        let data = accepted(install);
        let identity = data.identity().clone();
        fail_at(stage);
        assert!(ledger.begin_closing(data).is_err());
        assert!(matches!(
            ledger.coverage().unwrap(),
            LedgerCoverageV1::Partial { .. }
        ));
        assert!(ledger.lease().revalidate().is_ok());
        let ledger = reopen(&parent, ledger);
        complete(&ledger, 1, 1);
        assert!(ledger.query(&identity).unwrap().is_some());
    }
}

#[test]
fn delta_without_committed_floor_is_partial_and_is_not_adopted() {
    let (_files, parent, ledger, install) = setup();
    fail_at(FaultStage::DeltaSynced);
    assert!(ledger.begin_closing(accepted(install)).is_err());
    let path = ledger
        .lease()
        .ledger_root()
        .path()
        .join("retirement/deltas/d-0001.json");
    let bytes = std::fs::read(&path).unwrap();
    let ledger = reopen(&parent, ledger);
    assert!(matches!(
        ledger.coverage().unwrap(),
        LedgerCoverageV1::Partial { .. }
    ));
    assert_eq!(std::fs::read(path).unwrap(), bytes);
    assert!(ledger.lease().revalidate().is_ok());
}

#[test]
fn durable_tombstone_before_t_delta_is_exactly_replayable_from_closing() {
    let (_files, parent, ledger, install) = setup();
    let accepted = accepted(install);
    let closing = ledger.begin_closing(accepted.clone()).unwrap();
    fail_at(FaultStage::TombstoneSynced);
    assert!(ledger.commit_tombstone(&closing).is_err());
    assert!(matches!(
        ledger.coverage().unwrap(),
        LedgerCoverageV1::Partial {
            phase: LedgerScanPhaseV1::Tombstones,
            reason: LedgerFailureV1::Tainted,
            ..
        }
    ));
    assert!(ledger.begin_closing(accepted.clone()).is_err());
    let ledger = reopen(&parent, ledger);
    complete(&ledger, 1, 1);
    let closing = ledger.begin_closing(accepted).unwrap();
    ledger.commit_tombstone(&closing).unwrap();
    complete(&ledger, 2, 1);
}

#[test]
fn cumulative_budget_reserves_future_deltas_before_closing() {
    let (_files, _parent, ledger, install) = setup();
    let data = accepted(install);
    let arm = Uuid::parse_str(&data.identity().arm().operation_id).unwrap();
    {
        let mut state = ledger.state.lock().unwrap();
        state.logical_bytes = MAX_LOGICAL_BYTES - 1;
    }
    assert_eq!(
        ledger.begin_closing(data).err(),
        Some(EnforcerError::Capacity)
    );
    let state = ledger.state.lock().unwrap();
    assert_eq!(state.reserved_bytes, 0);
    assert_eq!(state.reserved_revisions, 0);
    assert!(!state.entries.contains_key(&arm));
    assert_eq!(*state.head.as_ref().unwrap().revision(), 0);
    assert!(
        ledger
            .lease()
            .ledger_root()
            .path()
            .join("retirement/deltas/d-0001.json")
            .try_exists()
            .is_ok_and(|exists| !exists)
    );
}

#[test]
fn retirement_id_cannot_be_reassigned_to_a_different_arm_in_same_job() {
    let (_files, _parent, ledger, install) = setup();
    let first = accepted(install);
    ledger.begin_closing(first.clone()).unwrap();
    let conflicting = rebind_retirement(accepted(install), &first, true);
    assert_eq!(
        serde_json::to_value(first.identity().scope()).unwrap(),
        serde_json::to_value(conflicting.identity().scope()).unwrap()
    );
    assert_ne!(
        first.identity().arm().operation_id,
        conflicting.identity().arm().operation_id
    );
    assert_eq!(
        ledger.begin_closing(conflicting).err(),
        Some(EnforcerError::Conflict)
    );
    complete(&ledger, 1, 1);
}

fn rebind_retirement(
    second: ClosingAcceptanceV1,
    first: &ClosingAcceptanceV1,
    same_scope: bool,
) -> ClosingAcceptanceV1 {
    let scope = if same_scope {
        first.identity().scope()
    } else {
        second.identity().scope()
    };
    let scope_json = serde_json::to_value(scope).unwrap();
    rebind_retirement_scope(second, first, scope_json)
}

fn rebind_retirement_scope(
    second: ClosingAcceptanceV1,
    first: &ClosingAcceptanceV1,
    scope: serde_json::Value,
) -> ClosingAcceptanceV1 {
    let mut identity_json = serde_json::to_value(second.identity()).unwrap();
    identity_json["job_operation_id"] = json!(first.identity().job_operation_id());
    identity_json["retirement_id"] = json!(first.identity().retirement_id());
    identity_json["arm_receipt"]["control_namespace_uuid"] =
        scope["control_namespace_uuid"].clone();
    identity_json["scope"] = scope;
    let identity: RetirementIdentityV1 = serde_json::from_value(identity_json).unwrap();
    let evidence = RetirementEvidenceV1::new(
        identity.digest().unwrap(),
        *second.evidence().original_actor_nonce(),
        *second.evidence().prepare_revocation_nonce(),
    )
    .unwrap();
    ClosingAcceptanceV1::new(
        identity,
        evidence,
        second.audit_ack().clone(),
        *second.closing_nonce(),
    )
    .unwrap()
}

#[test]
fn different_scoped_jobs_with_same_operation_and_retirement_ids_coexist_after_reopen() {
    let (_files, parent, ledger, install) = setup();
    let first = accepted(install);
    let second = rebind_retirement(accepted(install), &first, false);
    // Both scopes are valid and share installation, tenant, profile and origin;
    // their independently allocated control namespaces distinguish their jobs.
    assert_eq!(
        first.identity().scope().tenant_id(),
        second.identity().scope().tenant_id()
    );
    assert_ne!(
        first.identity().scope().control_namespace_uuid(),
        second.identity().scope().control_namespace_uuid()
    );
    assert_eq!(
        first.identity().job_operation_id(),
        second.identity().job_operation_id()
    );
    assert_eq!(
        first.identity().retirement_id(),
        second.identity().retirement_id()
    );
    let first_identity = first.identity().clone();
    let second_identity = second.identity().clone();
    ledger.begin_closing(first).unwrap();
    ledger.begin_closing(second).unwrap();
    complete(&ledger, 2, 2);
    let ledger = reopen(&parent, ledger);
    complete(&ledger, 2, 2);
    assert!(ledger.query(&first_identity).unwrap().is_some());
    assert!(ledger.query(&second_identity).unwrap().is_some());
}

#[test]
fn each_job_scope_axis_is_preserved_in_retirement_id_uniqueness() {
    let (_files, parent, ledger, install) = setup();
    let first = accepted(install);
    let variants = [
        ("tenant_id", json!("tenant-other")),
        ("profile_id", json!("profile-other")),
        ("gateway_origin", json!("http://127.0.0.1:8788")),
        ("control_namespace_uuid", json!(Uuid::new_v4())),
    ];
    let mut identities = vec![first.identity().clone()];
    ledger.begin_closing(first.clone()).unwrap();
    for (field, value) in variants {
        let mut scope = serde_json::to_value(first.identity().scope()).unwrap();
        scope[field] = value;
        let next = rebind_retirement_scope(accepted(install), &first, scope);
        identities.push(next.identity().clone());
        ledger.begin_closing(next).unwrap();
    }
    complete(&ledger, 5, 5);
    let ledger = reopen(&parent, ledger);
    complete(&ledger, 5, 5);
    for identity in identities {
        assert!(ledger.query(&identity).unwrap().is_some());
    }
}

#[test]
fn frame_limits_count_be32_and_preserve_invalid_tail() {
    assert_eq!(
        frame(&vec![1; MAX_FRAME_BYTES]).unwrap().len(),
        MAX_FRAME_BYTES + 4
    );
    assert!(frame(&vec![1; MAX_FRAME_BYTES + 1]).is_err());
    let mut bytes = frame(b"complete").unwrap();
    bytes.extend([0, 0, 0, 5, b'{']);
    let (prefix, complete) = frames(&bytes).unwrap();
    assert!(!complete);
    assert_eq!(prefix, vec![b"complete".as_slice()]);
    assert_eq!(bytes.len(), 17);
}
