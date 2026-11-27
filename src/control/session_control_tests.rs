use super::*;

#[test]
fn launch_target_selection_requires_a_canonical_id_and_revision() {
    let id = "abcdef01-2345-4678-9abc-def012345678";
    let selected = SessionLaunchTargetSelectionV1::parse(id, "7").unwrap();
    assert_eq!(selected.launch_target_id().to_string(), id);
    assert_eq!(selected.expected_target_revision(), 7);
    assert!(SessionLaunchTargetSelectionV1::parse(id, "9223372036854775807").is_ok());

    for invalid_id in [
        "",
        "/workspace/project",
        "00000000-0000-0000-0000-000000000000",
        "ABCDEF01-2345-4678-9ABC-DEF012345678",
        "abcdef01234546789abcdef012345678",
    ] {
        assert!(SessionLaunchTargetSelectionV1::parse(invalid_id, "7").is_err());
    }
    for invalid_revision in ["", "0", "07", "+7", " 7", "9223372036854775808"] {
        assert!(SessionLaunchTargetSelectionV1::parse(id, invalid_revision).is_err());
    }
}

#[test]
fn named_import_arm_operation_id_is_bound_to_confirmation_and_attempt() {
    let binding = "00000000-0000-4000-8000-000000000001".parse().unwrap();
    let attempt = "00000000-0000-4000-8000-000000000002".parse().unwrap();
    let other = "00000000-0000-4000-8000-000000000003".parse().unwrap();
    let operation = LegacyNamedImportArmRequestV1::operation_id(binding, attempt);
    assert_eq!(operation, "47b5b51a-4345-8085-a993-44b9011cbf48");
    assert_ne!(
        operation,
        LegacyNamedImportArmRequestV1::operation_id(other, attempt)
    );
    assert_ne!(
        operation,
        LegacyNamedImportArmRequestV1::operation_id(binding, other)
    );
}

#[test]
fn named_import_claim_matches_the_retained_namespace_and_arm_identity() {
    let binding_ref = "00000000-0000-4000-8000-000000000001".parse().unwrap();
    let namespace = "00000000-0000-4000-8000-000000000002";
    let arm_id = "00000000-0000-4000-8000-000000000003";
    let worker_id = "00000000-0000-4000-8000-000000000004";
    let attempt_id = "00000000-0000-4000-8000-000000000005";
    let other = "00000000-0000-4000-8000-000000000006";
    let request = LegacyNamedImportClaimRequestV1 {
        version: 1,
        binding_ref,
        namespace: namespace.into(),
        claim: crate::control::HelperStartClaimV1 {
            version: 1,
            operation_id: LegacyNamedImportClaimRequestV1::operation_id(
                binding_ref,
                arm_id.parse().unwrap(),
            ),
            helper_id: other.into(),
            emitter_id: worker_id.into(),
            arm_receipt: crate::control::ArmReceiptV1 {
                version: 1,
                arm_id: arm_id.into(),
                worker_id: worker_id.into(),
                attempt_id: attempt_id.into(),
                input_hash: "a".repeat(64),
                control_namespace_uuid: namespace.into(),
                audit_seq: 7,
                audit_hash: "b".repeat(64),
            },
            clock_check: crate::control::HostClockSampleV1 {
                version: 1,
                method: crate::control::HostClockMethodV1::LinuxBoottimeV1,
                boot_id: other.into(),
                continuous_ns_decimal: "100000000001".into(),
                observed_utc: None,
            },
        },
    };
    assert!(request.validate().is_ok());

    let mut changed = request.clone();
    changed.namespace = other.into();
    assert!(changed.validate().is_err());
    let mut changed = request.clone();
    changed.binding_ref = other.parse().unwrap();
    assert!(changed.validate().is_err());
    let mut changed = request.clone();
    changed.claim.arm_receipt.arm_id = other.into();
    assert!(changed.validate().is_err());
    let mut changed = request.clone();
    changed.claim.operation_id = arm_id.into();
    assert!(changed.validate().is_err());
}
