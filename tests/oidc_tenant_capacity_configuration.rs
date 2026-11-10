//! Public gateway construction must enforce the durable capacity contract.
//! Only private SQLite fixtures and offline OIDC configuration are used.
mod support;

use agentsandbox::{
    api::{GatewayState, OidcClaimsMappingV1, OidcGateway, OidcGatewayConfigV1, TenantGateway},
    audit::{Store, authentication::*},
    identity::HumanCapabilityV1,
    policy::Policy,
    redaction::Redactor,
};
use std::{collections::BTreeSet, sync::Arc};
use uuid::Uuid;

fn fixture(
    tenants: &[&str],
    plan: Option<AuthenticationCapacityPlanV1>,
) -> (tempfile::TempDir, GatewayState, OidcGateway) {
    let files = support::private_tempdir();
    let issuer = "https://identity.example.test/realms/product";
    let policy = AuthenticationAuditPolicyV1 {
        max_records: 1000,
        max_serialized_bytes: 1_048_576,
        approved_clients: ["web-public", "cli-public"]
            .map(|client_id| ApprovedAuthenticationClientV1 {
                issuer: issuer.into(),
                client_id: client_id.into(),
            })
            .to_vec(),
    };
    let audit_path = files.path().join("authentication.sqlite");
    let audit = match plan {
        Some(plan) => AuthenticationAudit::init_new_v4(
            &audit_path,
            Uuid::new_v4(),
            policy,
            plan,
            Redactor::new(vec![]),
        ),
        None => AuthenticationAudit::init_new(
            &audit_path,
            Uuid::new_v4(),
            policy,
            Redactor::new(vec![]),
        ),
    }
    .unwrap();
    let configured = tenants
        .iter()
        .map(|tenant| TenantGateway {
            tenant_id: (*tenant).into(),
            store: Arc::new(
                Store::open_for_tenant(
                    &files.path().join(format!("{tenant}.sqlite")),
                    tenant,
                    Redactor::new(vec![]),
                    1000,
                )
                .unwrap(),
            ),
            runner_token: format!("runner-{tenant}-{}", "r".repeat(48)),
            viewer_token: format!("viewer-{tenant}-{}", "v".repeat(48)),
        })
        .collect();
    let state =
        GatewayState::for_tenants(configured, Policy::default(), None, Redactor::new(vec![]))
            .unwrap();
    let provider = serde_json::from_value(serde_json::json!({
        "version": 1,
        "issuer": issuer,
        "web_client_id": "web-public",
        "cli_client_id": "cli-public",
        "api_audience": "ai-guard",
        "authorization_endpoint": format!("{issuer}/authorize"),
        "token_endpoint": format!("{issuer}/token"),
        "jwks_uri": format!("{issuer}/jwks"),
        "web_redirect_uri": "https://dashboard.example.test/auth/oidc/callback",
        "cli_callback_path": "/callback",
        "transport": {"kind": "https_only"}
    }))
    .unwrap();
    let oidc = OidcGateway::new(
        OidcGatewayConfigV1 {
            version: 1,
            provider,
            provider_label: "Synthetic offline capacity fixture".into(),
            web_origin: "https://dashboard.example.test".into(),
            mapping: OidcClaimsMappingV1 {
                revision: "capacity-fixture".into(),
                default_capabilities: BTreeSet::from([HumanCapabilityV1::ReadMetadata]),
                rules: vec![],
            },
        },
        Arc::new(audit),
        None,
    )
    .unwrap();
    (files, state, oidc)
}

#[tokio::test]
async fn public_multi_tenant_builder_refuses_an_unpartitioned_v3_authentication_audit() {
    let (_files, state, oidc) = fixture(&["tenant-a", "tenant-b"], None);
    let error = state
        .with_oidc(oidc)
        .err()
        .expect("multi-tenant admission requires its exact durable capacity plan");
    assert!(
        matches!(
            error.downcast_ref::<AuthenticationAuditFailureV1>(),
            Some(AuthenticationAuditFailureV1::Format)
        ),
        "capacity rejection must not be masked by an unrelated failure: {error:#}"
    );
}

#[tokio::test]
async fn explicit_single_tenant_v3_compatibility_remains_available() {
    let (_files, state, oidc) = fixture(&["tenant-a"], None);
    assert!(state.with_oidc(oidc).is_ok());
}

fn plan(tenants: &[&str]) -> AuthenticationCapacityPlanV1 {
    AuthenticationCapacityPlanV1 {
        version: 1,
        unscoped: AuthenticationCapacityV1 {
            max_records: 10,
            max_serialized_bytes: 100_000,
        },
        tenants: tenants
            .iter()
            .map(|tenant| AuthenticationTenantCapacityV1 {
                tenant_id: (*tenant).into(),
                capacity: AuthenticationCapacityV1 {
                    max_records: 100,
                    max_serialized_bytes: 100_000,
                },
            })
            .collect(),
    }
}

#[tokio::test]
async fn public_multi_tenant_builder_accepts_only_the_exact_durable_v4_plan() {
    let (_files, state, oidc) = fixture(
        &["tenant-a", "tenant-b"],
        Some(plan(&["tenant-a", "tenant-b"])),
    );
    assert!(state.with_oidc(oidc).is_ok());
    for planned in [vec!["tenant-a"], vec!["tenant-a", "tenant-b", "tenant-c"]] {
        let (_files, state, oidc) = fixture(&["tenant-a", "tenant-b"], Some(plan(&planned)));
        assert!(state.with_oidc(oidc).is_err());
    }
    let (_files, state, oidc) = fixture(&["tenant-a"], Some(plan(&["tenant-a", "tenant-b"])));
    assert!(state.with_oidc(oidc).is_err());
}
