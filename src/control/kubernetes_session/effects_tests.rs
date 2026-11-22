//! Closed validation of the effect ledger DTOs. Pure values: no Store, HTTP
//! or cluster. The Store contract (ordering, re-derivation, Q7, settlement)
//! has its own tests at the Store seam.
use super::*;

const RESERVATION: &str = "11111111-1111-4111-8111-111111111111";
const OPERATION: &str = "22222222-2222-4222-8222-222222222222";
const POD_UID: &str = "33333333-3333-4333-8333-333333333333";
const ARM: &str = "44444444-4444-4444-8444-444444444444";

fn h(byte: char) -> String {
    byte.to_string().repeat(64)
}
fn base(
    subject: KubernetesSessionEffectSubjectV2,
    action: KubernetesSessionEffectActionV2,
) -> KubernetesSessionEffectRequestV2 {
    KubernetesSessionEffectRequestV2 {
        version: 1,
        operation_id: OPERATION.into(),
        reservation_id: RESERVATION.into(),
        reservation_input_hash: h('a'),
        step: 2,
        subject,
        action,
        request_sha256: h('b'),
        object_uid: None,
        object_resource_version: None,
        storage: None,
        deadline: None,
    }
}
fn storage() -> KubernetesSessionStorageRequestV2 {
    KubernetesSessionStorageRequestV2 {
        version: 1,
        storage_class_name: "ai-guard-rwop".into(),
        capacity_mib: 1024,
    }
}
fn deadline() -> KubernetesSessionDeadlineArmV2 {
    KubernetesSessionDeadlineArmV2 {
        version: 1,
        arm_id: ARM.into(),
        deadline_at: "2026-11-21T20:00:00.000Z".into(),
        enforcer_receipt_sha256: h('c'),
    }
}
fn valid_requests() -> Vec<KubernetesSessionEffectRequestV2> {
    use KubernetesSessionEffectActionV2 as A;
    use KubernetesSessionEffectSubjectV2 as S;
    vec![
        KubernetesSessionEffectRequestV2 {
            storage: Some(storage()),
            ..base(S::WorkspaceClaim, A::Create)
        },
        KubernetesSessionEffectRequestV2 {
            object_uid: Some(POD_UID.into()),
            ..base(S::WorkspaceClaim, A::Delete)
        },
        base(S::SessionPod, A::Create),
        KubernetesSessionEffectRequestV2 {
            object_uid: Some(POD_UID.into()),
            object_resource_version: Some("44".into()),
            deadline: Some(deadline()),
            ..base(S::SessionPod, A::ReleaseGate)
        },
        KubernetesSessionEffectRequestV2 {
            object_uid: Some(POD_UID.into()),
            ..base(S::SessionPod, A::Attach)
        },
        KubernetesSessionEffectRequestV2 {
            object_uid: Some(POD_UID.into()),
            ..base(S::ExportPod, A::ReleaseFinalizer)
        },
        KubernetesSessionEffectRequestV2 {
            object_uid: Some(POD_UID.into()),
            ..base(S::ExportPod, A::Delete)
        },
    ]
}

#[test]
fn every_valid_subject_action_pair_validates_with_exactly_its_inputs() {
    for request in valid_requests() {
        request.validate().unwrap();
        assert_eq!(request.input_hash().unwrap().len(), 64);
    }
}

#[test]
fn inputs_that_do_not_belong_to_the_action_are_refused() {
    use KubernetesSessionEffectActionV2 as A;
    use KubernetesSessionEffectSubjectV2 as S;
    let refused = [
        // A claim is never released, attached or finalized.
        KubernetesSessionEffectRequestV2 {
            object_uid: Some(POD_UID.into()),
            object_resource_version: Some("1".into()),
            deadline: Some(deadline()),
            ..base(S::WorkspaceClaim, A::ReleaseGate)
        },
        // A claim CREATE needs its storage request.
        base(S::WorkspaceClaim, A::Create),
        // Storage belongs only to the claim CREATE.
        KubernetesSessionEffectRequestV2 {
            storage: Some(storage()),
            ..base(S::SessionPod, A::Create)
        },
        // A CREATE addresses no existing UID.
        KubernetesSessionEffectRequestV2 {
            object_uid: Some(POD_UID.into()),
            ..base(S::SessionPod, A::Create)
        },
        // Every other action addresses one UID.
        base(S::SessionPod, A::Delete),
        // A release needs its observed resourceVersion and an armed deadline.
        KubernetesSessionEffectRequestV2 {
            object_uid: Some(POD_UID.into()),
            object_resource_version: Some("44".into()),
            ..base(S::SessionPod, A::ReleaseGate)
        },
        KubernetesSessionEffectRequestV2 {
            object_uid: Some(POD_UID.into()),
            deadline: Some(deadline()),
            ..base(S::SessionPod, A::ReleaseGate)
        },
        // A deadline belongs only to a release.
        KubernetesSessionEffectRequestV2 {
            object_uid: Some(POD_UID.into()),
            deadline: Some(deadline()),
            ..base(S::SessionPod, A::Attach)
        },
        KubernetesSessionEffectRequestV2 {
            step: 0,
            ..base(S::SessionPod, A::Create)
        },
        KubernetesSessionEffectRequestV2 {
            step: MAX_EFFECT_STEPS + 1,
            ..base(S::SessionPod, A::Create)
        },
        KubernetesSessionEffectRequestV2 {
            operation_id: RESERVATION.into(),
            ..base(S::SessionPod, A::Create)
        },
        KubernetesSessionEffectRequestV2 {
            request_sha256: "B".repeat(64),
            ..base(S::SessionPod, A::Create)
        },
    ];
    for request in refused {
        assert!(request.validate().is_err(), "{request:?}");
    }
    let mut late = deadline();
    late.deadline_at = "2026-11-21T20:00:00Z".into();
    assert!(late.validate().is_err(), "milliseconds are required");
    late.deadline_at = "2026-11-21T21:00:00.000+01:00".into();
    assert!(late.validate().is_err(), "UTC is required");
}

fn ack(
    disposition: KubernetesSessionEffectDispositionV2,
    replayed: bool,
) -> KubernetesSessionEffectClaimAckV2 {
    let request = valid_requests().remove(2);
    KubernetesSessionEffectClaimAckV2 {
        version: 1,
        receipt: KubernetesSessionEffectReceiptV2 {
            version: 1,
            operation_id: request.operation_id.clone(),
            input_hash: request.input_hash().unwrap(),
            control_namespace_uuid: "55555555-5555-4555-8555-555555555555".into(),
            audit_seq: 9,
            audit_hash: h('d'),
        },
        request,
        disposition,
        issued_at: "2026-11-21T19:00:00.000Z".into(),
        not_after: "2026-11-21T19:00:30.000Z".into(),
        replayed,
    }
}

#[test]
fn a_replayed_acknowledgement_never_grants_a_fresh_emission() {
    use KubernetesSessionEffectDispositionV2 as D;
    ack(D::EmitOnce, false).validate().unwrap();
    ack(D::ObserveOnly, true).validate().unwrap();
    ack(D::ObserveOnly, false).validate().unwrap();
    assert!(ack(D::EmitOnce, true).validate().is_err());
}

#[test]
fn an_acknowledgement_is_bound_to_its_exact_request_and_window() {
    use KubernetesSessionEffectDispositionV2 as D;
    let mut other = ack(D::EmitOnce, false);
    other.request.request_sha256 = h('e');
    assert!(other.validate().is_err(), "receipt input hash mismatch");
    let mut inverted = ack(D::EmitOnce, false);
    inverted.not_after = inverted.issued_at.clone();
    assert!(inverted.validate().is_err(), "empty window");
    let mut operation = ack(D::EmitOnce, false);
    operation.receipt.operation_id = ARM.into();
    assert!(operation.validate().is_err(), "foreign receipt");
}

#[test]
fn results_terminals_and_settlements_are_closed_values() {
    let result = |outcome| KubernetesSessionEffectResultV2 {
        version: 1,
        operation_id: ARM.into(),
        reservation_id: RESERVATION.into(),
        step: 3,
        claim_operation_id: OPERATION.into(),
        request_sha256: h('b'),
        outcome,
        observed_at: "2026-11-21T19:00:01.000Z".into(),
    };
    result(KubernetesSessionEffectOutcomeV2::Applied {
        object_uid: POD_UID.into(),
        resource_version: "42".into(),
    })
    .validate()
    .unwrap();
    result(KubernetesSessionEffectOutcomeV2::Uncertain { http_status: None })
        .validate()
        .unwrap();
    // Only an answered no-effect status is NotApplied.
    assert!(
        result(KubernetesSessionEffectOutcomeV2::NotApplied { http_status: 500 })
            .validate()
            .is_err()
    );
    let terminal = KubernetesSessionTerminalV2 {
        version: 1,
        operation_id: ARM.into(),
        reservation_id: RESERVATION.into(),
        subject: KubernetesSessionEffectSubjectV2::SessionPod,
        pod_uid: POD_UID.into(),
        end: KubernetesSessionPodEndV2::Terminated {
            container_id: "containerd://0123".into(),
            exit_code: Some(0),
            oom_killed: Some(false),
            started_at: Some("2026-11-21T19:00:02.000Z".into()),
            finished_at: Some("2026-11-21T19:05:00.000Z".into()),
        },
        observed_at: "2026-11-21T19:05:01.000Z".into(),
        declared_exit_code: Some(0),
    };
    terminal.validate().unwrap();
    let claim_terminal = KubernetesSessionTerminalV2 {
        subject: KubernetesSessionEffectSubjectV2::WorkspaceClaim,
        ..terminal.clone()
    };
    assert!(claim_terminal.validate().is_err());
    let settlement = KubernetesSessionSettlementV2 {
        version: 1,
        operation_id: ARM.into(),
        reservation_id: RESERVATION.into(),
        reservation_input_hash: h('a'),
        expected_last_step: 12,
    };
    settlement.validate().unwrap();
    assert!(
        KubernetesSessionSettlementV2 {
            expected_last_step: 0,
            ..settlement
        }
        .validate()
        .is_err()
    );
}
