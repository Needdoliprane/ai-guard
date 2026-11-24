//! Real full-union migration; no SQL creates a positive modern authority.
use super::*;
use crate::human_authority::{HumanAuthorityFailure, migration::tests::Fixture};
use crate::{
    policy::Policy,
    relay::RelayEndpointV1,
    workload::{WorkloadBindingIssueV1, WorkloadSubjectV1},
};
use uuid::Uuid;

const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const HASH_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

#[path = "../../../tests/support/relay_admission_fixture.rs"]
#[allow(dead_code)]
mod seed;

#[test]
fn owned_format14_post_claim_backup_preserves_durable_facts() {
    assert_owned_claim_backup(14, ClaimBackupCase::InspectConsumed);
}

#[test]
fn owned_format15_post_claim_backup_preserves_durable_facts() {
    assert_owned_claim_backup(15, ClaimBackupCase::InspectConsumed);
}

#[test]
fn owned_format14_readopted_post_claim_replays_without_emission() {
    assert_owned_claim_backup(14, ClaimBackupCase::ReplayConsumed);
}

#[test]
fn owned_format15_readopted_post_claim_replays_without_emission() {
    assert_owned_claim_backup(15, ClaimBackupCase::ReplayConsumed);
}

#[test]
fn owned_format14_readopted_pre_claim_refuses_start() {
    assert_owned_claim_backup(14, ClaimBackupCase::RefuseUnconsumed);
}

#[test]
fn owned_format15_readopted_pre_claim_refuses_start() {
    assert_owned_claim_backup(15, ClaimBackupCase::RefuseUnconsumed);
}

#[derive(Clone, Copy)]
enum ClaimBackupCase {
    InspectConsumed,
    ReplayConsumed,
    RefuseUnconsumed,
}

fn assert_owned_claim_backup(format: u32, case: ClaimBackupCase) {
    use crate::{
        audit::OwnedRetainedSourceFacts,
        control_fs::{MAX_PRIVATE_FILE_BYTES, PrivateFile},
    };
    use rusqlite::backup::{Backup, StepResult};
    use std::{
        os::unix::fs::PermissionsExt,
        time::{Duration, Instant},
    };

    let mut seeded = None;
    let mut fixture = Store::owned_control_fixture_with_legacy_setup_for_test(format, |legacy| {
        let admission = seed::decision_admission(legacy);
        let (claim, worker, claim_receipt) = match case {
            ClaimBackupCase::RefuseUnconsumed => {
                let (claim, worker, _) = seed::armed_from(legacy, admission);
                assert_eq!(worker.state, WorkerStateV1::Created);
                assert_eq!(worker.revision, claim.expected_revision);
                assert!(worker.attempt.start_operation_id.is_none());
                assert!(
                    legacy
                        .worker_operation(&claim.operation_id)
                        .unwrap()
                        .is_none()
                );
                (claim, worker, None)
            }
            ClaimBackupCase::InspectConsumed | ClaimBackupCase::ReplayConsumed => {
                let (claim, worker, _) = seed::claimed_from(legacy, admission);
                assert_eq!(worker.state, WorkerStateV1::StartIssued);
                assert_eq!(
                    worker.attempt.start_operation_id.as_deref(),
                    Some(claim.operation_id.as_str())
                );
                let receipt = legacy
                    .worker_operation(&claim.operation_id)
                    .unwrap()
                    .unwrap();
                (claim, worker, Some(receipt))
            }
        };
        assert!(worker.quota_held);
        let receipts = [
            worker.admission_operation_id.as_str(),
            claim.arm_receipt.arm_id.as_str(),
        ]
        .map(|id| legacy.worker_operation(id).unwrap().unwrap());
        seeded = Some((claim, worker, receipts, claim_receipt));
    });
    let (claim, worker, receipts, claim_receipt) = seeded.unwrap();
    let (bundle, _, _) = fixture.take_startup();
    let source_id = bundle
        .owner
        .closure()
        .sources
        .iter()
        .find(|source| source.tenant_id.as_deref() == Some("A"))
        .unwrap()
        .source_id;
    let store = &bundle.stores[&source_id];
    let token = bundle.owner.source_token(source_id).unwrap();
    let facts = OwnedRetainedSourceFacts::from_token(&token).unwrap();
    let source_state = {
        let epoch = bundle.owner.try_read_epoch().unwrap();
        let access = HumanSourceAccess::read(&epoch, &token).unwrap();
        store.supervision_state_scoped(&access).unwrap()
    };
    assert_eq!(source_state.schema_version, format);
    assert!(!source_state.active);

    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(std::env::temp_dir().canonicalize().unwrap())
        .unwrap();
    let path = directory.path().join("claim-backup.sqlite");
    let (backing, initialization) = storage::StoreStorage::create_new(&path).unwrap();
    let (source_head, source_integrity) = {
        let source = store.db.lock().unwrap();
        store.backing.verify(&source).unwrap();
        let head = audit_head(&source).unwrap();
        let integrity = Store::check_integrity(&source).unwrap();
        assert!(integrity.valid);
        let mut destination = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .unwrap();
        backing.verify(&destination).unwrap();
        {
            // SQLite's backup includes committed WAL pages from the retained connection.
            let backup = Backup::new(&source, &mut destination).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while backup.step(256).unwrap() != StepResult::Done {
                assert!(Instant::now() < deadline, "SQLite backup timed out");
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        let mode: String = destination
            .query_row("PRAGMA journal_mode=DELETE", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "delete");
        backing.verify(&destination).unwrap();
        destination.close().unwrap();
        backing.sync_closed(&initialization).unwrap();
        assert_eq!(audit_head(&source).unwrap(), head);
        (head, integrity)
    };

    let backup_file = PrivateFile::open(&path, MAX_PRIVATE_FILE_BYTES).unwrap();
    let inspection = store
        .owned_inspect_backup(&facts, &path, backup_file.digest())
        .unwrap();
    assert_eq!(inspection.head_seq(), source_head.0);
    assert_eq!(inspection.head_hash(), source_head.1);

    // Inspect the copied facts before any fixture adoption changes its head.
    let copy = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap();
    let integrity = Store::check_integrity(&copy).unwrap();
    assert!(integrity.valid);
    assert_eq!(integrity.event_count, source_integrity.event_count);
    assert_eq!(integrity.head_hash, source_integrity.head_hash);
    for receipt in &receipts {
        assert_eq!(
            operation_receipt(&copy, &receipt.operation_id)
                .unwrap()
                .as_ref(),
            Some(receipt)
        );
    }
    let copied_arm = arm_entry(&copy, &claim.arm_receipt.arm_id)
        .unwrap()
        .unwrap();
    assert_eq!(json!(copied_arm.receipt), json!(claim.arm_receipt));
    assert_eq!(
        operation_receipt(&copy, &claim.operation_id).unwrap(),
        claim_receipt
    );
    if claim_receipt.is_some() {
        let (_, record) = load_record(&copy, &claim.operation_id).unwrap().unwrap();
        let Input::WorkerClaim(saved_claim) = record.input else {
            panic!("copied operation must be a worker claim");
        };
        assert_eq!(json!(saved_claim), json!(claim));
    }
    let restored = load_worker(&copy, &worker.worker_id).unwrap().unwrap();
    assert_eq!(restored.state, worker.state);
    assert_eq!(restored.attempt, worker.attempt);
    assert_eq!(restored.revision, worker.revision);
    assert!(restored.quota_held);
    let quota = quota_usage(&copy, &QuotaPolicyV1::default()).unwrap();
    assert_eq!(quota.held_workers, 1);
    assert_eq!(quota.held_cpu_millis, u64::from(worker.launch.cpu_millis));
    assert_eq!(quota.held_memory_bytes, worker.launch.memory_bytes);

    if matches!(case, ClaimBackupCase::InspectConsumed) {
        return;
    }

    let previous_incarnation = token.incarnation();
    let original_identity = token.descriptor().storage_binding.source_identity();
    drop(token);
    copy.close().unwrap();
    drop(backup_file);
    drop(initialization);
    let bundle = fixture.reopen_claim_copy(bundle, source_id, &path);
    let store = &bundle.stores[&source_id];
    let token = bundle.owner.source_token(source_id).unwrap();
    assert_ne!(token.incarnation(), previous_incarnation);
    assert_ne!(
        token.descriptor().storage_binding.source_identity(),
        original_identity
    );
    assert_eq!(token.descriptor().storage_binding.path(), path);
    let epoch = bundle.owner.try_read_epoch().unwrap();
    let access = HumanSourceAccess::read(&epoch, &token).unwrap();
    let state = store.supervision_state_scoped(&access).unwrap();
    assert_eq!(state.schema_version, format);
    assert_eq!(
        state.control_namespace_uuid,
        source_state.control_namespace_uuid
    );
    assert!(!state.active);
    let before = {
        let db = store.db.lock().unwrap();
        let head = audit_head(&db).unwrap();
        let (kind, previous_hash): (String, String) = db
            .query_row(
                "SELECT kind,prev_hash FROM events WHERE seq=?1",
                [head.0],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(head.0, source_head.0 + 1);
        assert_eq!(kind, crate::audit::review_authority::INCARNATION_KIND);
        assert_eq!(previous_hash, source_head.1);
        head
    };

    let result = store.claim_armed_start_scoped(&access, &claim);
    match claim_receipt.as_ref() {
        Some(receipt) => {
            let replay = result.unwrap();
            assert_eq!(
                replay.disposition,
                Some(ClaimDispositionV1::AlreadyConsumed)
            );
            assert_eq!(&replay.receipt, receipt);
            assert_eq!(json!(replay.worker), json!(worker));
        }
        None => {
            let error = result.unwrap_err();
            assert_eq!(
                error.downcast_ref::<ControlFailure>(),
                Some(&ControlFailure::SupervisionInactive)
            );
        }
    }
    let db = store.db.lock().unwrap();
    assert_eq!(audit_head(&db).unwrap(), before);
    assert_eq!(
        operation_receipt(&db, &claim.operation_id).unwrap(),
        claim_receipt
    );
    let retained = load_worker(&db, &worker.worker_id).unwrap().unwrap();
    assert_eq!(json!(retained), json!(worker));
    let held = quota_usage(&db, &QuotaPolicyV1::default()).unwrap();
    assert_eq!(held.held_workers, quota.held_workers);
    assert_eq!(held.held_cpu_millis, quota.held_cpu_millis);
    assert_eq!(held.held_memory_bytes, quota.held_memory_bytes);
}

#[test]
fn migrated_supervision_finishes_previously_claimed_helper_without_recreating_start_authority() {
    let fixture = Fixture::new();
    let legacy = fixture.legacy_store("A");
    let request = seed::supervised_request(seed::decision_admission(legacy));
    let admitted = seed::admit_supervised(legacy, &request);
    let worker = legacy
        .mutate_worker(&seed::mutation(
            &admitted.worker.unwrap(),
            WorkerTransitionV1::Prepare,
        ))
        .unwrap()
        .worker;
    let helper_id = Uuid::new_v4();
    let arm = seed::helper_arm(
        &worker,
        &request.intent_hash,
        helper_id,
        HelperPurposeV1::Import,
        HelperPermissionV1::Admission {
            receipt: admitted.receipt,
        },
    );
    let armed = legacy.arm_execution(&arm).unwrap();
    let claim = seed::helper_claim(&arm, &armed.receipt, helper_id);
    assert_eq!(
        legacy.claim_helper_start(&claim).unwrap().disposition,
        ClaimDispositionV1::EmitOnce
    );
    let pending = seed::supervised_request(seed::decision_admission(legacy));
    let bundle = fixture.migrated();
    let source_id = bundle
        .owner()
        .closure()
        .sources
        .iter()
        .find(|source| source.tenant_id.as_deref() == Some("A"))
        .unwrap()
        .source_id;
    let store = bundle.store(source_id).unwrap();
    let token = bundle.owner().source_token(source_id).unwrap();
    let epoch = bundle.owner().try_read_epoch().unwrap();
    let access = HumanSourceAccess::read(&epoch, &token).unwrap();
    let before = audit_head(&store.db.lock().unwrap()).unwrap();
    let policy = Policy::default();
    let error = store
        .admit_supervised_scoped(
            &access,
            &pending,
            &policy,
            policy::evaluate(&policy, &pending.admission.decision_request),
            json!(null),
            &QuotaPolicyV1::default(),
        )
        .unwrap_err();
    assert_eq!(
        error.downcast_ref::<ControlFailure>(),
        Some(&ControlFailure::SupervisionInactive)
    );
    assert_eq!(audit_head(&store.db.lock().unwrap()).unwrap(), before);
    assert!(store.arm_execution_scoped(&access, &arm).unwrap().replayed);
    assert_eq!(
        store
            .execution_arm_scoped(&access, &arm.operation_id)
            .unwrap()
            .unwrap()
            .receipt
            .audit_hash,
        armed.receipt.audit_hash
    );
    assert_eq!(
        store
            .claim_helper_start_scoped(&access, &claim)
            .unwrap()
            .disposition,
        ClaimDispositionV1::AlreadyConsumed
    );
    let terminal = seed::helper_observation(&arm, &armed.receipt, helper_id, Some(false), 0, None);
    let receipt = store.observe_helper_scoped(&access, &terminal).unwrap();
    assert_eq!(
        store
            .observe_helper_scoped(&access, &terminal)
            .unwrap()
            .audit_hash,
        receipt.audit_hash
    );
    let cleanup = HelperCleanupV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        helper_id: helper_id.to_string(),
        arm_receipt: armed.receipt,
        removed: true,
        error: None,
    };
    let cleaned = store
        .record_helper_cleanup_scoped(&access, &cleanup)
        .unwrap();
    assert_eq!(
        store
            .record_helper_cleanup_scoped(&access, &cleanup)
            .unwrap()
            .audit_hash,
        cleaned.audit_hash
    );
    let after = audit_head(&store.db.lock().unwrap()).unwrap();
    assert_eq!(after.0, before.0 + 2);
    let mut divergent = terminal.clone();
    divergent.observation.exit_code = Some(1);
    assert!(store.observe_helper_scoped(&access, &divergent).is_err());
    assert!(store.observe_helper(&terminal).is_err());
    assert_eq!(audit_head(&store.db.lock().unwrap()).unwrap(), after);
    assert!(!store.supervision_state_scoped(&access).unwrap().active);
    assert!(store.permanently_healthy());
}

#[test]
fn migrated_supervision_reads_require_matching_live_source_and_keep_legacy_closed() {
    let fixture = Fixture::new();
    let bundle = fixture.migrated();
    let source_id = |tenant: &str| {
        bundle
            .owner()
            .closure()
            .sources
            .iter()
            .find(|source| source.tenant_id.as_deref() == Some(tenant))
            .unwrap()
            .source_id
    };
    let a = source_id("A");
    let b = source_id("B");
    let store = bundle.store(a).unwrap();
    let token = bundle.owner().source_token(a).unwrap();
    let other = bundle.owner().source_token(b).unwrap();
    let epoch = bundle.owner().try_read_epoch().unwrap();
    let access = HumanSourceAccess::read(&epoch, &token).unwrap();
    let foreign = HumanSourceAccess::read(&epoch, &other).unwrap();
    let head = audit_head(&store.db.lock().unwrap()).unwrap();
    let state = store.supervision_state_scoped(&access).unwrap();
    assert_eq!(state.schema_version, 14);
    assert_eq!(
        state.protocols,
        vec![SupervisionProtocolV1::BrokerSupervisionV1]
    );
    // Opening a migrated source does not recreate execution authority.
    assert!(!state.active);
    assert!(
        store
            .execution_arm_scoped(&access, &Uuid::new_v4().to_string())
            .unwrap()
            .is_none()
    );
    for error in [
        store.supervision_state().unwrap_err(),
        store.supervision_state_scoped(&foreign).unwrap_err(),
        store
            .execution_arm(&Uuid::new_v4().to_string())
            .unwrap_err(),
    ] {
        assert_eq!(
            error.downcast_ref::<review_authority::ReviewAuthorityFailure>(),
            Some(&review_authority::ReviewAuthorityFailure::Required)
        );
    }
    assert_eq!(audit_head(&store.db.lock().unwrap()).unwrap(), head);
    assert!(store.permanently_healthy());
}

#[test]
fn expired_supervision_access_refuses_before_any_durable_change() {
    let fixture = Fixture::new();
    let bundle = fixture.migrated();
    let source_id = bundle
        .owner()
        .closure()
        .sources
        .iter()
        .find(|source| source.tenant_id.as_deref() == Some("A"))
        .unwrap()
        .source_id;
    let store = bundle.store(source_id).unwrap();
    let token = bundle.owner().source_token(source_id).unwrap();
    let epoch = bundle.owner().try_write_epoch().unwrap();
    let access = HumanSourceAccess::write(&epoch, &token).unwrap();
    let head = audit_head(&store.db.lock().unwrap()).unwrap();
    std::thread::sleep(
        epoch
            .test_deadline()
            .saturating_duration_since(std::time::Instant::now()),
    );
    let error = store.supervision_state_scoped(&access).unwrap_err();
    assert_eq!(
        error.downcast_ref::<HumanAuthorityFailure>(),
        Some(&HumanAuthorityFailure::EpochExpired)
    );
    assert_eq!(audit_head(&store.db.lock().unwrap()).unwrap(), head);
    assert!(store.permanently_healthy());
    drop(access);
    drop(epoch);
    bundle.owner().begin_close();
    assert!(matches!(
        bundle.owner().try_read_epoch(),
        Err(HumanAuthorityFailure::Closing)
    ));
}
