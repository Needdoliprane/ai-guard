use agentsandbox::{
    control::{
        ArmReceiptV1, ArmSubjectV1, ArmV1, ArmedStartClaimV1, DecisionPhaseV1, DecisionReceiptV1,
        HelperPurposeV1, HostClockMethodV1, HostClockSampleV1, LaunchSpecV1, NetworkV1,
        RestartPolicyV1, RuntimeInspectionV1, RuntimeKindV1, SecurityPresetV1, WorkerAttemptV1,
        WorkerStateV1, WorkerV1, helper_execution_policy_v1,
    },
    model::CommandSpec,
};
use uuid::Uuid;

const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn clock(method: HostClockMethodV1, boot_id: &str, ns: &str) -> HostClockSampleV1 {
    HostClockSampleV1 {
        version: 1,
        method,
        boot_id: boot_id.into(),
        continuous_ns_decimal: ns.into(),
        observed_utc: Some("2026-10-30T12:00:00Z".into()),
    }
}

fn arm() -> ArmV1 {
    let start_ns = 100_000_000_000_u128;
    let timeout_seconds = 60_u32;
    ArmV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: Uuid::new_v4().to_string(),
        attempt_id: Uuid::new_v4().to_string(),
        expected_revision: 7,
        intent_hash: HASH_A.into(),
        subject: ArmSubjectV1::Worker,
        inspection: RuntimeInspectionV1 {
            authority_id: Uuid::new_v4().to_string(),
            runtime_kind: RuntimeKindV1::Docker,
            container_id: "c".repeat(64),
            stable_name: "aig-worker".into(),
            volume_name: "aig-worker-work".into(),
            image_id: format!("sha256:{HASH_A}"),
            spec_hash: HASH_B.into(),
            cpu_millis: 2000,
            memory_bytes: 1_073_741_824,
            pids_limit: 256,
            network: NetworkV1::None,
            security_preset: SecurityPresetV1::FixedSandboxV2,
            restart_policy: RestartPolicyV1::No,
        },
        authority_binding_hash: HASH_A.into(),
        configuration_fingerprint_version: 2,
        configuration_fingerprint: HASH_B.into(),
        clock: clock(
            HostClockMethodV1::DarwinMachContinuousV1,
            "11111111-1111-4111-8111-111111111111",
            &start_ns.to_string(),
        ),
        timeout_seconds,
        deadline_ns_decimal: (start_ns + u128::from(timeout_seconds) * 1_000_000_000).to_string(),
    }
}

fn parent_worker() -> WorkerV1 {
    let launch = LaunchSpecV1 {
        version: 1,
        session_id: Uuid::new_v4().to_string(),
        command: CommandSpec {
            argv: vec!["/usr/bin/codex".into(), "--version".into()],
            cwd: "/workspace".into(),
        },
        profile: "observe".into(),
        runtime_kind: RuntimeKindV1::Podman,
        authority_id: Uuid::new_v4().to_string(),
        image_ref: "worker:fixture".into(),
        image_id: format!("sha256:{HASH_A}"),
        network: NetworkV1::Bridge,
        cpu_millis: 2000,
        memory_bytes: 1_073_741_824,
        pids_limit: 256,
        timeout_seconds: 600,
        security_preset: SecurityPresetV1::FixedSandboxInteractiveV3,
        restart_policy: RestartPolicyV1::No,
        workspace_manifest_digest: HASH_B.into(),
        workspace_import_policy_version: 1,
        provider: "generic".into(),
        capture_output: false,
        credential_binding_id: None,
        credential_names: Vec::new(),
        provider_resolution: None,
        egress: None,
    };
    let spec_hash = launch.spec_hash().unwrap();
    WorkerV1 {
        worker_id: Uuid::new_v4().to_string(),
        admission_operation_id: Uuid::new_v4().to_string(),
        request_id: Uuid::new_v4().to_string(),
        receipt: DecisionReceiptV1 {
            version: 1,
            request_id: Uuid::new_v4().to_string(),
            request_hash: HASH_A.into(),
            phase: DecisionPhaseV1::Initial,
            audit_seq: 1,
            audit_hash: HASH_B.into(),
        },
        launch,
        spec_hash,
        policy_hash: HASH_B.into(),
        state: WorkerStateV1::Preparing,
        quota_held: true,
        revision: 9,
        attempt: WorkerAttemptV1 {
            attempt_id: Uuid::new_v4().to_string(),
            stable_name: "aig-parent-worker".into(),
            volume_name: "aig-parent-work".into(),
            container_id: None,
            created_inspection: None,
            create_operation_id: None,
            start_operation_id: None,
            start_emitter_id: None,
        },
        observed_started_at: None,
        allocation_inconsistent: false,
        last_observation: None,
        unknown_reason: None,
        cleanup_removed: None,
        cleanup_error: None,
    }
}

fn arm_receipt(arm: &ArmV1) -> ArmReceiptV1 {
    ArmReceiptV1 {
        version: 1,
        arm_id: arm.operation_id.clone(),
        worker_id: arm.worker_id.clone(),
        attempt_id: arm.attempt_id.clone(),
        input_hash: arm.input_hash().unwrap(),
        control_namespace_uuid: Uuid::new_v4().to_string(),
        audit_seq: 12,
        audit_hash: HASH_A.into(),
    }
}

#[test]
fn clock_counter_and_boot_identity_are_canonical() {
    let boot = "11111111-1111-4111-8111-111111111111";
    assert_eq!(
        clock(HostClockMethodV1::LinuxBoottimeV1, boot, "0")
            .continuous_ns()
            .unwrap(),
        0
    );
    assert_eq!(
        clock(
            HostClockMethodV1::LinuxBoottimeV1,
            boot,
            "340282366920938463463374607431768211455"
        )
        .continuous_ns()
        .unwrap(),
        u128::MAX
    );
    for invalid_counter in [
        "",
        "00",
        "+1",
        " 1",
        "340282366920938463463374607431768211456",
    ] {
        assert!(
            clock(HostClockMethodV1::LinuxBoottimeV1, boot, invalid_counter)
                .validate()
                .is_err(),
            "noncanonical/out-of-range counter accepted: {invalid_counter:?}"
        );
    }
    assert!(
        clock(
            HostClockMethodV1::LinuxBoottimeV1,
            "00000000-0000-0000-0000-000000000000",
            "1"
        )
        .validate()
        .is_err(),
        "nil boot UUID must not identify a host boot"
    );
}

#[test]
fn arm_deadline_checks_overflow_method_boot_and_expiration() {
    let arm = arm();
    arm.validate().unwrap();
    arm.validate_clock_check(&clock(arm.clock.method, &arm.clock.boot_id, "100000000000"))
        .unwrap();
    assert!(
        arm.validate_clock_check(&clock(
            HostClockMethodV1::LinuxBoottimeV1,
            &arm.clock.boot_id,
            "100000000001",
        ))
        .is_err()
    );
    assert!(
        arm.validate_clock_check(&clock(
            arm.clock.method,
            "22222222-2222-4222-8222-222222222222",
            "100000000001",
        ))
        .is_err()
    );
    assert!(
        arm.validate_clock_check(&clock(arm.clock.method, &arm.clock.boot_id, "160000000000",))
            .is_err(),
        "deadline is exclusive"
    );

    let mut overflow = arm;
    overflow.clock.continuous_ns_decimal = "340282366920938463463374607431768211455".into();
    overflow.deadline_ns_decimal = u128::MAX.to_string();
    assert!(
        overflow.validate().is_err(),
        "deadline addition must be checked"
    );
}

#[test]
fn armed_claim_must_match_arm_revision_and_receipt_input_hash() {
    let arm = arm();
    let receipt = arm_receipt(&arm);
    let mut claim = ArmedStartClaimV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: arm.worker_id.clone(),
        attempt_id: arm.attempt_id.clone(),
        expected_revision: arm.expected_revision,
        emitter_id: Uuid::new_v4().to_string(),
        arm_receipt: receipt.clone(),
        clock_check: clock(arm.clock.method, &arm.clock.boot_id, "100000000001"),
    };
    claim.validate_for_arm(&arm).unwrap();

    claim.expected_revision += 1;
    assert!(
        claim.validate_for_arm(&arm).is_err(),
        "a claim for a different worker revision must not consume this arm"
    );

    let mut forged_receipt = receipt;
    forged_receipt.input_hash = HASH_B.into();
    let forged = ArmedStartClaimV1 {
        expected_revision: arm.expected_revision,
        arm_receipt: forged_receipt,
        ..claim
    };
    assert!(
        forged.validate_for_arm(&arm).is_err(),
        "forged arm input hash accepted"
    );
}

#[test]
fn helper_policy_uses_fixed_tar_contract_and_has_its_own_hash_domain() {
    let parent = parent_worker();
    parent.launch.validate().unwrap_or_else(|error| {
        panic!(
            "fixture launch should be valid: {error}; {:#?}",
            parent.launch
        )
    });
    assert_eq!(parent.spec_hash, parent.launch.spec_hash().unwrap());
    let helper_id = Uuid::new_v4();
    let purposes = [
        HelperPurposeV1::Import,
        HelperPurposeV1::ImportVerification,
        HelperPurposeV1::FinalScan,
        HelperPurposeV1::Export,
    ];
    let policies: Vec<_> = purposes
        .into_iter()
        .map(|purpose| helper_execution_policy_v1(&parent, purpose, helper_id).unwrap())
        .collect();
    for policy in &policies {
        policy.validate().unwrap();
        assert_eq!(policy.network, NetworkV1::None);
        assert_eq!(policy.security_preset, SecurityPresetV1::FixedSandboxV2);
        assert_eq!(policy.cpu_millis, 2000);
        assert_eq!(policy.memory_bytes, 1_073_741_824);
        assert_eq!(policy.pids_limit, 256);
        assert_eq!(policy.timeout_seconds, 120);
        assert_eq!(policy.credential_binding_id, None);
        assert!(policy.credential_names.is_empty());
        assert_eq!(
            policy.stable_name,
            format!("aig-parent-worker-h-{}", helper_id.simple())
        );
        assert_eq!(policy.volume_name, parent.attempt.volume_name);
        assert_ne!(policy.spec_hash().unwrap(), parent.spec_hash);
    }
    assert_eq!(
        policies[0].command.argv,
        [
            "/bin/tar",
            "--extract",
            "--file=-",
            "--directory=/workspace",
            "--no-same-owner",
            "--no-same-permissions",
        ]
        .map(str::to_owned)
    );
    for policy in &policies[1..] {
        assert_eq!(
            policy.command.argv,
            [
                "/bin/tar",
                "--create",
                "--file=-",
                "--directory=/workspace",
                "."
            ]
            .map(str::to_owned)
        );
    }
    let hashes: Vec<_> = policies
        .iter()
        .map(|policy| policy.spec_hash().unwrap())
        .collect();
    assert_eq!(
        hashes
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        4
    );
    let another_id =
        helper_execution_policy_v1(&parent, HelperPurposeV1::FinalScan, Uuid::new_v4()).unwrap();
    assert_ne!(hashes[2], another_id.spec_hash().unwrap());

    let policy = &policies[2];
    let mut inspection = RuntimeInspectionV1 {
        authority_id: policy.authority_id.clone(),
        runtime_kind: policy.runtime_kind,
        container_id: "c".repeat(64),
        stable_name: policy.stable_name.clone(),
        volume_name: policy.volume_name.clone(),
        image_id: policy.image_id.clone(),
        spec_hash: policy.spec_hash().unwrap(),
        cpu_millis: policy.cpu_millis,
        memory_bytes: policy.memory_bytes,
        pids_limit: policy.pids_limit,
        network: policy.network,
        security_preset: policy.security_preset,
        restart_policy: policy.restart_policy,
    };
    policy.validate_inspection(&inspection).unwrap();
    inspection.spec_hash = parent.spec_hash.clone();
    assert!(policy.validate_inspection(&inspection).is_err());
}

#[test]
fn strict_dtos_reject_unknown_fields_and_protocol_names_are_stable() {
    let mut clock_json = serde_json::to_value(clock(
        HostClockMethodV1::DarwinMachContinuousV1,
        "11111111-1111-4111-8111-111111111111",
        "42",
    ))
    .unwrap();
    clock_json["unexpected"] = serde_json::json!(true);
    assert!(serde_json::from_value::<HostClockSampleV1>(clock_json).is_err());
    assert_eq!(
        serde_json::to_value(agentsandbox::control::SupervisionProtocolV1::BrokerSupervisionV1)
            .unwrap(),
        "broker_supervision_v1"
    );
}
