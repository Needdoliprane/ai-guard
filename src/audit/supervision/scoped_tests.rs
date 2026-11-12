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
