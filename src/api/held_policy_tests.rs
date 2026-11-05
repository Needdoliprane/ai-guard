use super::*;
use crate::control::kubernetes_admission::{
    KUBERNETES_HELD_PROFILE_V1, KubernetesHeldCreateApprovalV1, KubernetesHeldCreatePolicyV1,
};
use std::os::unix::fs::PermissionsExt;

fn policy() -> KubernetesHeldCreatePolicyV1 {
    KubernetesHeldCreatePolicyV1 {
        version: 1,
        decision_ttl_seconds: 30,
        approvals: vec![
            KubernetesHeldCreateApprovalV1 {
                authority_fingerprint: "b".repeat(64),
                image: format!("worker-z@sha256:{}", "e".repeat(64)),
                profile: KUBERNETES_HELD_PROFILE_V1.into(),
            },
            KubernetesHeldCreateApprovalV1 {
                authority_fingerprint: "a".repeat(64),
                image: format!("worker-a@sha256:{}", "d".repeat(64)),
                profile: KUBERNETES_HELD_PROFILE_V1.into(),
            },
        ],
    }
}

fn two_tenant_state() -> (tempfile::TempDir, GatewayState) {
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .expect("create private API held-policy test directory");
    let tenants = ["tenant-a", "tenant-b"]
        .into_iter()
        .enumerate()
        .map(|(index, tenant_id)| {
            let store = Store::open_for_tenant(
                &directory.path().join(format!("{tenant_id}.sqlite")),
                tenant_id,
                Redactor::new(vec![]),
                1_000,
            )
            .expect("open real tenant Store");
            TenantGateway {
                tenant_id: tenant_id.into(),
                store: Arc::new(store),
                runner_token: format!("fixture-runner-{index}-{}", "r".repeat(40)),
                viewer_token: format!("fixture-viewer-{index}-{}", "v".repeat(40)),
            }
        })
        .collect();
    let state = GatewayState::for_tenants(tenants, Policy::default(), None, Redactor::new(vec![]))
        .expect("construct real multi-tenant GatewayState");
    (directory, state)
}

fn tenant_policy<'a>(
    state: &'a GatewayState,
    tenant_id: &str,
) -> Option<&'a KubernetesHeldCreatePolicyV1> {
    state
        .tenants
        .iter()
        .find(|tenant| tenant.tenant_id == tenant_id)
        .and_then(|tenant| tenant.held_create_policy.as_deref())
}

#[test]
fn attaches_canonical_policy_only_to_its_tenant_and_leaves_old_state_unchanged() {
    let (_directory, state) = two_tenant_state();
    assert!(tenant_policy(&state, "tenant-a").is_none());
    assert!(tenant_policy(&state, "tenant-b").is_none());

    let expected = policy().canonicalized().expect("canonical fixture policy");
    let configured = state
        .clone()
        .with_kubernetes_held_create_policies(vec![("tenant-a".into(), policy())])
        .expect("valid policy resolves for the registered tenant");

    assert_eq!(tenant_policy(&configured, "tenant-a"), Some(&expected));
    assert!(tenant_policy(&configured, "tenant-b").is_none());
    assert!(tenant_policy(&state, "tenant-a").is_none());
    assert!(tenant_policy(&state, "tenant-b").is_none());
}

#[test]
fn rejects_unknown_duplicate_and_invalid_tenant_policy_mappings() {
    let (_directory, state) = two_tenant_state();

    assert!(
        state
            .clone()
            .with_kubernetes_held_create_policies(vec![("tenant-unknown".into(), policy())])
            .is_err()
    );
    assert!(
        state
            .clone()
            .with_kubernetes_held_create_policies(vec![
                ("tenant-a".into(), policy()),
                ("tenant-a".into(), policy()),
            ])
            .is_err()
    );

    let mut invalid = policy();
    invalid.version = 2;
    assert!(
        state
            .clone()
            .with_kubernetes_held_create_policies(vec![("tenant-a".into(), invalid)])
            .is_err()
    );

    assert!(tenant_policy(&state, "tenant-a").is_none());
    assert!(tenant_policy(&state, "tenant-b").is_none());
}
