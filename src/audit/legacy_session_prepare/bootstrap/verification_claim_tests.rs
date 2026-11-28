use super::*;
use crate::{
    broker_registry::BrokerIdentityV1,
    control::{
        ArmReceiptV1, ClaimDispositionV1, HostClockSampleV1,
        session_control::{
            LegacyNamedImportClaimRequestV1, LegacyNamedImportVerificationArmRequestV2,
            LegacyNamedImportVerificationClaimRequestV2,
        },
    },
};

#[tokio::test]
async fn legacy_named_import_verification_claim_emits_once_and_replays_exactly() {
    named_import_arm_fixture(ImportArmCase::VerificationClaim).await;
}

pub(super) fn exercise(
    fixture: &Fixture,
    store: &Store,
    (broker, other_broker): (&BrokerIdentityV1, &BrokerIdentityV1),
    mut human: LocalCredentialV1,
    prepared: &WorkerV1,
    (verification, arm_receipt): (&LegacyNamedImportVerificationArmRequestV2, &ArmReceiptV1),
    import_claim: &LegacyNamedImportClaimRequestV1,
) {
    let ArmSubjectV1::Helper { helper_id, .. } = &verification.arm.subject else {
        panic!("fixture must retain a verification helper");
    };
    let arm_id = Uuid::parse_str(&arm_receipt.arm_id).unwrap();
    let request = LegacyNamedImportVerificationClaimRequestV2 {
        version: 2,
        binding_ref: verification.binding_ref,
        namespace: verification.namespace.clone(),
        claim: HelperStartClaimV1 {
            version: 1,
            operation_id: LegacyNamedImportVerificationClaimRequestV2::operation_id(
                verification.binding_ref,
                arm_id,
            ),
            helper_id: helper_id.clone(),
            emitter_id: Uuid::new_v4().to_string(),
            arm_receipt: arm_receipt.clone(),
            clock_check: HostClockSampleV1 {
                continuous_ns_decimal: (verification.arm.clock.continuous_ns().unwrap() + 1)
                    .to_string(),
                ..verification.arm.clock.clone()
            },
        },
    };
    assert_ne!(
        request.claim.operation_id,
        LegacyNamedImportClaimRequestV1::operation_id(request.binding_ref, arm_id)
    );
    let mut extra = serde_json::to_value(&request).unwrap();
    extra["unsupported"] = true.into();
    assert!(serde_json::from_value::<LegacyNamedImportVerificationClaimRequestV2>(extra).is_err());

    let initial_head = store.head().unwrap();
    let quota = QuotaPolicyV1::default();
    let held_quota = serde_json::to_value(store.worker_quota(&quota).unwrap()).unwrap();
    assert!(
        store
            .read_legacy_named_import_verification_claim(broker, &request)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store
            .claim_legacy_named_import_verification(other_broker, &request)
            .unwrap_err()
            .downcast_ref::<ControlFailure>(),
        Some(&ControlFailure::NotFound)
    );
    assert_eq!(
        store
            .read_legacy_named_import_verification_claim(other_broker, &request)
            .unwrap_err()
            .downcast_ref::<ControlFailure>(),
        Some(&ControlFailure::NotFound)
    );

    let mut wrong_clock = request.clone();
    wrong_clock.claim.clock_check.boot_id = Uuid::new_v4().to_string();
    let mut expired_clock = request.clone();
    expired_clock.claim.clock_check.continuous_ns_decimal =
        verification.arm.deadline_ns_decimal.clone();
    let mut import_arm = request.clone();
    import_arm.claim.arm_receipt = import_claim.claim.arm_receipt.clone();
    import_arm.claim.operation_id = LegacyNamedImportVerificationClaimRequestV2::operation_id(
        request.binding_ref,
        Uuid::parse_str(&import_arm.claim.arm_receipt.arm_id).unwrap(),
    );
    for invalid in [&wrong_clock, &expired_clock, &import_arm] {
        assert_eq!(
            store
                .claim_legacy_named_import_verification(broker, invalid)
                .unwrap_err()
                .downcast_ref::<ControlFailure>(),
            Some(&ControlFailure::InvalidRequest)
        );
    }
    let historical_rail = LegacyNamedImportClaimRequestV1 {
        version: 1,
        binding_ref: request.binding_ref,
        namespace: request.namespace.clone(),
        claim: HelperStartClaimV1 {
            operation_id: LegacyNamedImportClaimRequestV1::operation_id(
                request.binding_ref,
                arm_id,
            ),
            ..request.claim.clone()
        },
    };
    assert_eq!(
        store
            .claim_legacy_named_import(broker, &historical_rail)
            .unwrap_err()
            .downcast_ref::<ControlFailure>(),
        Some(&ControlFailure::NotFound)
    );
    assert_eq!(store.head().unwrap(), initial_head);

    *store.named_control_commit_observation.lock().unwrap() =
        Some(crate::broker_registry::parse_time(DEADLINE).unwrap());
    assert_eq!(
        store
            .claim_legacy_named_import_verification(broker, &request)
            .unwrap_err()
            .downcast_ref::<ControlFailure>(),
        Some(&ControlFailure::NotFound)
    );
    assert_eq!(store.head().unwrap(), initial_head);

    let go = std::sync::Barrier::new(2);
    let (first, second) = std::thread::scope(|scope| {
        let first = scope.spawn(|| {
            go.wait();
            store
                .claim_legacy_named_import_verification(broker, &request)
                .unwrap()
        });
        go.wait();
        let second = store
            .claim_legacy_named_import_verification(broker, &request)
            .unwrap();
        (first.join().unwrap(), second)
    });
    assert_ne!(first.disposition, second.disposition);
    assert!([first.disposition, second.disposition].contains(&ClaimDispositionV1::EmitOnce));
    assert_eq!(first.receipt, second.receipt);
    request.validate_receipt(&first.receipt).unwrap();
    assert_eq!(first.receipt.audit_seq, initial_head + 1);
    assert_eq!(store.head().unwrap(), first.receipt.audit_seq);

    let mut different_emitter = request.clone();
    different_emitter.claim.emitter_id = Uuid::new_v4().to_string();
    assert_eq!(
        store
            .claim_legacy_named_import_verification(broker, &different_emitter)
            .unwrap_err()
            .downcast_ref::<ControlFailure>(),
        Some(&ControlFailure::Conflict)
    );
    assert_eq!(store.head().unwrap(), first.receipt.audit_seq);

    let revision = human.revision;
    human.active = false;
    human.revision += 1;
    let revoked = store
        .mutate_identity(&IdentityMutationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            actor: fixture.operator.clone(),
            change: IdentityChangeV1::Credential {
                expected_revision: revision,
                binding: human,
            },
        })
        .unwrap();
    let reopened = fixture.reopen();
    let read = reopened
        .read_legacy_named_import_verification_claim(broker, &request)
        .unwrap()
        .unwrap();
    let replay = reopened
        .claim_legacy_named_import_verification(broker, &request)
        .unwrap();
    for consumed in [read, replay] {
        assert_eq!(consumed.disposition, ClaimDispositionV1::AlreadyConsumed);
        assert_eq!(consumed.receipt, first.receipt);
    }
    assert_eq!(
        reopened
            .read_legacy_named_import_claim(broker, import_claim)
            .unwrap()
            .unwrap()
            .disposition,
        ClaimDispositionV1::AlreadyConsumed
    );
    assert_eq!(reopened.head().unwrap(), revoked.audit_seq);
    assert!(reopened.integrity().unwrap().valid);
    assert_eq!(
        serde_json::to_value(reopened.worker(&prepared.worker_id).unwrap().unwrap()).unwrap(),
        serde_json::to_value(prepared).unwrap()
    );
    assert_eq!(
        serde_json::to_value(reopened.worker_quota(&quota).unwrap()).unwrap(),
        held_quota
    );
}
