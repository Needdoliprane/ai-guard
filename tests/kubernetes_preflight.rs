//! Closed profile planning and admission inspection. These software checks do
//! not qualify a cluster, CNI, workload execution, or a durable worker deadline.
use agentsandbox::runtime::kubernetes::{
    KubernetesAuthorityV1, KubernetesError, KubernetesPreflightPlanV1,
    KubernetesPreflightResourcesV1,
};
use serde_json::json;
use uuid::Uuid;

fn authority() -> KubernetesAuthorityV1 {
    KubernetesAuthorityV1::new(
        Uuid::from_u128(1),
        "https://127.0.0.1:6443",
        "11111111-1111-4111-8111-111111111111",
        "tenant-a",
        "22222222-2222-4222-8222-222222222222",
        &"a".repeat(64),
    )
    .unwrap()
}

fn plan() -> KubernetesPreflightPlanV1 {
    KubernetesPreflightPlanV1::new(
        &authority(),
        Uuid::from_u128(7),
        Uuid::from_u128(8),
        &format!("local/worker@sha256:{}", "b".repeat(64)),
        KubernetesPreflightResourcesV1 {
            cpu_millis: 2000,
            memory_mib: 1024,
            workspace_mib: 128,
            temporary_mib: 64,
            deadline_seconds: 60,
        },
    )
    .unwrap()
}

#[test]
fn preflight_plan_refuses_free_pod_fields_and_unbounded_or_mutable_inputs() {
    let valid = serde_json::to_value(plan()).unwrap();
    for (field, value) in [
        ("version", json!(2)),
        ("session_id", json!(Uuid::nil())),
        ("operation_id", json!(Uuid::nil())),
        ("authority_fingerprint", json!("c".repeat(64))),
        ("image", json!("local/worker:latest")),
        ("image", json!("local/worker@sha256:bad")),
        ("image", json!("https://private:secret@registry/worker")),
        ("command", json!(["/bin/sh", "-c", "arbitrary"])),
        ("tenant_id", json!("tenant-b")),
        ("env", json!({"SECRET":"must-not-be-sent"})),
    ] {
        let mut changed = valid.clone();
        changed[field] = value;
        let decoded = serde_json::from_value::<KubernetesPreflightPlanV1>(changed);
        assert!(
            decoded.is_err() || decoded.unwrap().target(&authority()).is_err(),
            "untrusted preflight field accepted: {field}"
        );
    }
    for (field, value) in [
        ("cpu_millis", 0),
        ("cpu_millis", 200_001),
        ("memory_mib", 0),
        ("memory_mib", 102_401),
        ("workspace_mib", 0),
        ("temporary_mib", 0),
        ("deadline_seconds", 0),
        ("deadline_seconds", 86_401),
    ] {
        let mut changed = valid.clone();
        changed["resources"][field] = json!(value);
        let decoded: KubernetesPreflightPlanV1 = serde_json::from_value(changed).unwrap();
        assert_eq!(
            decoded.target(&authority()),
            Err(KubernetesError::InvalidPreflightPlan)
        );
    }
}

#[test]
fn preflight_commitment_covers_authority_operation_image_and_resources() {
    let valid = plan();
    let target = valid.target(&authority()).unwrap();
    assert_eq!(target.session_id, Uuid::from_u128(7));
    assert_eq!(target.operation_id, Uuid::from_u128(8));
    assert_eq!(
        target.pod_name,
        "ai-guard-preflight-00000000000000000000000000000008"
    );
    let valid = serde_json::to_value(valid).unwrap();
    for (pointer, value) in [
        ("/session_id", json!(Uuid::from_u128(9))),
        ("/operation_id", json!(Uuid::from_u128(9))),
        (
            "/image",
            json!(format!("local/worker@sha256:{}", "c".repeat(64))),
        ),
        ("/resources/cpu_millis", json!(1500)),
        ("/resources/memory_mib", json!(512)),
        ("/resources/workspace_mib", json!(64)),
        ("/resources/temporary_mib", json!(32)),
        ("/resources/deadline_seconds", json!(30)),
    ] {
        let mut changed = valid.clone();
        *changed.pointer_mut(pointer).unwrap() = value;
        let changed: KubernetesPreflightPlanV1 = serde_json::from_value(changed).unwrap();
        assert_ne!(
            target.request_sha256,
            changed.target(&authority()).unwrap().request_sha256
        );
    }
    let mut changed_authority = serde_json::to_value(authority()).unwrap();
    changed_authority["namespace"] = json!("tenant-b");
    let changed_authority = serde_json::from_value(changed_authority).unwrap();
    assert!(plan().target(&changed_authority).is_err());
    assert_eq!(
        authority().require_workload_admission(),
        Err(KubernetesError::WorkloadAdmissionUnavailable)
    );
}
