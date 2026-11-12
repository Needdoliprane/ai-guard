use super::*;
use serde_json::{Value, json};

pub(crate) fn binding_json() -> Value {
    json!({
        "version":1,"binding_id":"11000000-0000-4000-8000-000000000001",
        "revision":1,"active":true,
        "credential_id":"11000000-0000-4000-8000-000000000002","credential_generation":1,
        "tenant_id":"tenant-A","control_namespace_uuid":"11000000-0000-4000-8000-000000000003",
        "broker_id":"11000000-0000-4000-8000-000000000004","broker_profile_id":"review-project",
        "source_id":"project-source-A","project_id":"project-A","source_policy_digest":"a".repeat(64),
        "routing_policy_id":"11000000-0000-4000-8000-000000000005","routing_policy_revision":1,
        "routing_policy_digest":"b".repeat(64),
        "session_binding_policy_id":"11000000-0000-4000-8000-000000000006",
        "session_binding_policy_revision":1,"session_binding_policy_digest":"c".repeat(64),
        "closure_id":"11000000-0000-4000-8000-000000000007","closure_revision":1,
        "closure_digest":"d".repeat(64),"control_config_public_digest":"e".repeat(64)
    })
}

pub(crate) fn policy_json() -> Value {
    json!({
        "version":1,"policy_id":"11000000-0000-4000-8000-000000000006","revision":1,"active":true,
        "tenant_id":"tenant-A","control_namespace_uuid":"11000000-0000-4000-8000-000000000003",
        "organization_scope":"organization-A","project_id":"project-A",
        "owner":{"kind":"human_local","installation_id":"11000000-0000-4000-8000-000000000008",
                 "account_id":"11000000-0000-4000-8000-000000000009"},
        "owner_permissions":["content","approve_launch"],
        "shares":[{"principal":{"kind":"human_local",
          "installation_id":"11000000-0000-4000-8000-000000000008",
          "account_id":"11000000-0000-4000-8000-000000000010"},"permissions":["content","approve_launch"]}]
    })
}

pub(crate) fn provision() -> ProvisionRunnerReviewBindingV1 {
    let policy: ReviewSessionBindingPolicyV1 = serde_json::from_value(policy_json()).unwrap();
    let mut value = binding_json();
    value["session_binding_policy_digest"] = json!(policy.digest().unwrap());
    ProvisionRunnerReviewBindingV1 {
        version: 1,
        operation_id: "11000000-0000-4000-8000-000000000011".into(),
        operator_delegation_id: "11000000-0000-4000-8000-000000000012".into(),
        expected_revision: 0,
        expected_policy_revision: 0,
        binding: serde_json::from_value(value).unwrap(),
        policy,
    }
}

// Removing strict DTO deserialization, identifier validation or numeric bounds
// must allow one of these invalid inputs and therefore fail this parent.
#[test]
fn strict_binding_rejects_unknown_noncanonical_and_unbounded_input() {
    let valid = binding_json();
    let parsed = RunnerReviewBindingV1::from_json(&serde_json::to_vec(&valid).unwrap()).unwrap();
    assert_eq!(parsed.tenant_id, "tenant-A"); // Tenant is the existing string contract.
    for (field, replacement) in [
        ("tenant_id", json!("tenant/A")),
        ("tenant_id", json!("A".repeat(129))),
        ("credential_generation", json!(0)),
        ("revision", json!(u64::MAX)),
        ("binding_id", json!("00000000-0000-0000-0000-000000000000")),
        (
            "control_namespace_uuid",
            json!("11000000-0000-4000-8000-00000000000A"),
        ),
        ("source_policy_digest", json!("A".repeat(64))),
        ("source_id", json!("../project")),
        ("token", json!("fixture-private-value")),
    ] {
        let mut value = valid.clone();
        value[field] = replacement;
        assert!(
            serde_json::from_value::<RunnerReviewBindingV1>(value).is_err(),
            "{field}"
        );
    }
    let bytes = serde_json::to_vec(&valid).unwrap();
    let mut duplicate = bytes[..bytes.len() - 1].to_vec();
    duplicate.extend_from_slice(b",\"tenant_id\":\"tenant-B\"}");
    assert!(RunnerReviewBindingV1::from_json(&duplicate).is_err());
    assert!(RunnerReviewBindingV1::from_json(&vec![b' '; 32 * 1024 + 1]).is_err());
}

#[test]
fn policy_cannot_name_technical_owner_duplicate_share_or_wider_permissions() {
    let valid = policy_json();
    assert!(serde_json::from_value::<ReviewSessionBindingPolicyV1>(valid.clone()).is_ok());
    for invalid in [
        {
            let mut v = valid.clone();
            v["owner"] = json!({"kind":"legacy_control","credential_id":"runner","role":"runner"});
            v
        },
        {
            let mut v = valid.clone();
            v["shares"][0]["principal"] = v["owner"].clone();
            v
        },
        {
            let mut v = valid.clone();
            let share = v["shares"][0].clone();
            v["shares"].as_array_mut().unwrap().push(share);
            v
        },
        {
            let mut v = valid.clone();
            v["shares"][0]["permissions"] = json!(["export"]);
            v
        },
        {
            let mut v = valid.clone();
            v["shares"][0]["permissions"] = json!(["approve_launch"]);
            v
        },
        {
            let mut v = valid.clone();
            v["shares"][0]["permissions"] = json!(["content", "content"]);
            v
        },
    ] {
        assert!(
            ReviewSessionBindingPolicyV1::from_json(&serde_json::to_vec(&invalid).unwrap())
                .is_err()
        );
    }
}

// Defaulting an omitted ACL or accepting unordered/duplicate permissions would
// silently turn a legacy owner into an approver. Both serde entry paths refuse.
#[test]
fn owner_permissions_are_mandatory_canonical_and_always_include_content() {
    let valid = policy_json();
    let mut missing = valid.clone();
    missing.as_object_mut().unwrap().remove("owner_permissions");
    assert!(serde_json::from_value::<ReviewSessionBindingPolicyV1>(missing.clone()).is_err());
    assert!(
        ReviewSessionBindingPolicyV1::from_json(&serde_json::to_vec(&missing).unwrap()).is_err()
    );
    let mut nested = serde_json::to_value(provision()).unwrap();
    nested["policy"]
        .as_object_mut()
        .unwrap()
        .remove("owner_permissions");
    assert!(serde_json::from_value::<ProvisionRunnerReviewBindingV1>(nested).is_err());

    for permissions in [
        json!([]),
        json!(["approve_launch"]),
        json!(["content", "content"]),
        json!(["approve_launch", "content"]),
        json!(["content", "approve_launch", "content"]),
        json!(["content", "export"]),
        json!(null),
        json!("content"),
    ] {
        let mut value = valid.clone();
        value["owner_permissions"] = permissions;
        assert!(serde_json::from_value::<ReviewSessionBindingPolicyV1>(value.clone()).is_err());
        assert!(
            ReviewSessionBindingPolicyV1::from_json(&serde_json::to_vec(&value).unwrap()).is_err()
        );
    }
    let bytes = serde_json::to_vec(&valid).unwrap();
    let mut duplicate = bytes[..bytes.len() - 1].to_vec();
    duplicate.extend_from_slice(b",\"owner_permissions\":[\"content\"]}");
    assert!(ReviewSessionBindingPolicyV1::from_json(&duplicate).is_err());
}

#[test]
fn explicit_owner_approval_changes_policy_and_bound_command_digests() {
    let approving = provision();
    let mut content_only = approving.clone();
    content_only.policy.owner_permissions = vec![ReviewSessionPermissionV1::Content];
    content_only.binding.session_binding_policy_digest = content_only.policy.digest().unwrap();
    content_only.validate().unwrap();
    assert_eq!(
        content_only.policy.owner_permissions,
        vec![ReviewSessionPermissionV1::Content]
    );
    assert_ne!(
        content_only.policy.digest().unwrap(),
        approving.policy.digest().unwrap()
    );
    assert_ne!(
        content_only.binding.digest().unwrap(),
        approving.binding.digest().unwrap()
    );
    assert_ne!(
        content_only.input_hash().unwrap(),
        approving.input_hash().unwrap()
    );
    let encoded = serde_json::to_vec(&content_only.policy).unwrap();
    assert_eq!(
        ReviewSessionBindingPolicyV1::from_json(&encoded).unwrap(),
        content_only.policy
    );
}

#[test]
fn provision_pairs_exact_policy_and_binds_every_routing_reference() {
    let command = provision();
    command.validate().unwrap();
    let initial = command.input_hash().unwrap();
    let mut changed = command.clone();
    changed.binding.project_id = "another-project".into();
    assert!(changed.validate().is_err());
    changed = command.clone();
    changed.binding.session_binding_policy_digest = "0".repeat(64);
    assert!(changed.validate().is_err());
    for field in [
        "credential_generation",
        "routing_policy_revision",
        "closure_revision",
    ] {
        let mut value = serde_json::to_value(&command).unwrap();
        value["binding"][field] = json!(2);
        let changed: ProvisionRunnerReviewBindingV1 = serde_json::from_value(value).unwrap();
        assert_ne!(initial, changed.input_hash().unwrap(), "{field}");
    }
    let public = serde_json::to_string(&command).unwrap();
    for forbidden in [
        "bearer",
        "token",
        "sha256_digest",
        "client_secret",
        "private_key",
    ] {
        assert!(!public.contains(forbidden));
    }
    let mut widened = serde_json::to_value(&command).unwrap();
    widened["policy"]["shares"][0]["permissions"] = json!(["content", "approve_launch", "export"]);
    assert!(serde_json::from_value::<ProvisionRunnerReviewBindingV1>(widened).is_err());
}
