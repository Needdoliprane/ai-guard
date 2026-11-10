//! D8 corrected-behaviour oracles, compatible with fd5000e's current API.
//! Real Registry plus local identity/SQLite/Router producers. No socket,
//! OIDC, engine, external service or project executable is used by this source.
use super::*;
use crate::api::{
    human_state::MAX_CONTEXTS,
    read_execution_tests::{human_get, identity_change, local_identity_state},
};
use std::{collections::BTreeSet, os::unix::fs::PermissionsExt};
use uuid::Uuid;

fn synthetic_binding(tenant: &str, principal: u32, member: usize) -> ContextBinding {
    let mut digest = [0u8; 32];
    digest[..4].copy_from_slice(&principal.to_be_bytes());
    digest[31] = 1;
    ContextBinding {
        digest,
        principal_key: format!("{principal:064x}"),
        credential_id: Uuid::new_v4().to_string(),
        credential_revision: 1,
        authentication_method: "local_credential".into(),
        mapping_revision: None,
        tenant_id: tenant.into(),
        organization_scope: format!("fixture-organization-{member}"),
        membership_id: Uuid::new_v4().to_string(),
        mode: ContextModeV1::Personal,
        membership_revision: 1,
        authorization_revision: 1,
    }
}

#[test]
fn d8_registry_pressure_from_a_preserves_b_configured_share() {
    // Configuration is the real, validated two-tenant Gateway constructor.
    // Bindings are deliberately supplied at the same sink as the audit repro;
    // authentication/producer reachability is covered by the second parent.
    let (_directory, state, _token) = local_identity_state();
    let registry = &state.human.contexts;
    let mut admitted = 0usize;
    let mut refusals = 0usize;
    for principal in 0..(MAX_CONTEXTS / MAX_OWNER_CONTEXTS) as u32 {
        for member in 0..MAX_OWNER_CONTEXTS {
            match registry.obtain(
                synthetic_binding("tenant-a", principal, member),
                CONTEXT_TTL,
                "fixture-expiry".into(),
            ) {
                Ok(_) => admitted += 1,
                Err(StateFailure::Capacity) => refusals += 1,
                Err(error) => panic!("setup: unexpected registry failure {error:?}"),
            }
        }
    }
    assert!(admitted > 0 && admitted <= MAX_CONTEXTS);
    let victim = registry.obtain(
        synthetic_binding("tenant-b", u32::MAX, 0),
        CONTEXT_TTL,
        "fixture-expiry".into(),
    );
    assert!(
        victim.is_ok(),
        "D8: A exhausted B's configured share: admitted_a={admitted}, refusals_a={refusals}, victim={:?}",
        victim.as_ref().err()
    );
}

fn principal() -> PrincipalRefV1 {
    PrincipalRefV1::HumanLocal {
        installation_id: "909d9572-f1a2-4a53-ad8b-a741030a2b18".into(),
        account_id: Uuid::new_v4().to_string(),
    }
}

fn account(principal: &PrincipalRefV1) -> IdentityMutationV1 {
    identity_change(IdentityChangeV1::Account {
        expected_revision: 0,
        account: HumanAccountV1 {
            version: 1,
            principal: principal.clone(),
            display_name: "D8 synthetic local human".into(),
            revision: 1,
            active: true,
        },
    })
}

fn membership(
    tenant: &str,
    principal: &PrincipalRefV1,
    organization: usize,
    administration: bool,
) -> IdentityMutationV1 {
    let mut capabilities = BTreeSet::from([HumanCapabilityV1::ReadMetadata]);
    if administration {
        capabilities.insert(HumanCapabilityV1::ReadTenantMetadata);
    }
    identity_change(IdentityChangeV1::Membership {
        expected_revision: 0,
        membership: MembershipV1 {
            version: 1,
            membership_id: Uuid::new_v4().to_string(),
            principal: principal.clone(),
            tenant_id: tenant.into(),
            organization_scope: format!("fixture-organization-{organization}"),
            revision: 1,
            active: true,
            capabilities,
        },
    })
}

fn provision(
    store: &Store,
    principal: &PrincipalRefV1,
    member_count: usize,
    administration: bool,
) -> String {
    assert!(member_count > 0);
    let token = generate_local_credential().unwrap();
    store
        .provision_local_human(&LocalHumanProvisionV1 {
            version: 1,
            mutations: [
                account(principal),
                identity_change(IdentityChangeV1::Credential {
                    expected_revision: 0,
                    binding: LocalCredentialV1 {
                        version: 1,
                        credential_id: Uuid::new_v4().to_string(),
                        principal: principal.clone(),
                        sha256_digest: hex::encode(Sha256::digest(token.as_bytes())),
                        revision: 1,
                        active: true,
                        expires_at: None,
                    },
                }),
                membership(store.tenant_id(), principal, 0, administration),
            ],
        })
        .unwrap();
    for organization in 1..member_count {
        store
            .mutate_identity(&membership(
                store.tenant_id(),
                principal,
                organization,
                administration,
            ))
            .unwrap();
    }
    token
}

struct PressureFixture {
    _directory: tempfile::TempDir,
    state: GatewayState,
    a_tokens: Vec<String>,
    b_only: String,
    mixed: String,
}

fn pressure_fixture() -> PressureFixture {
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let mut tenants = Vec::new();
    for (index, tenant_id) in ["tenant-a", "tenant-b"].into_iter().enumerate() {
        let path = directory.path().join(format!("{tenant_id}.sqlite"));
        let store =
            Store::open_for_tenant(&path, tenant_id, Redactor::new(vec![]), 10_000).unwrap();
        store
            .migrate_control_v3(&path.with_extension("before-v3"))
            .unwrap();
        store
            .migrate_identity_v4(&path.with_extension("before-v4"))
            .unwrap();
        store
            .migrate_human_access_v5(&path.with_extension("before-v5"))
            .unwrap();
        tenants.push(TenantGateway {
            tenant_id: tenant_id.into(),
            store: Arc::new(store),
            runner_token: format!("fixture-d8-runner-{index}").repeat(3),
            viewer_token: format!("fixture-d8-viewer-{index}").repeat(3),
        });
    }
    let a_tokens = (0..MAX_CONTEXTS / MAX_OWNER_CONTEXTS)
        .map(|_| {
            provision(
                &tenants[0].store,
                &principal(),
                MAX_OWNER_CONTEXTS / 2,
                true,
            )
        })
        .collect();
    let b_only = provision(&tenants[1].store, &principal(), 1, false);
    let mixed_principal = principal();
    let mixed = provision(&tenants[1].store, &mixed_principal, 1, false);
    tenants[0]
        .store
        .mutate_identity(&account(&mixed_principal))
        .unwrap();
    tenants[0]
        .store
        .mutate_identity(&membership("tenant-a", &mixed_principal, 0, false))
        .unwrap();
    let state =
        GatewayState::for_tenants(tenants, Policy::default(), None, Redactor::new(vec![])).unwrap();
    PressureFixture {
        _directory: directory,
        state,
        a_tokens,
        b_only,
        mixed,
    }
}

#[tokio::test]
async fn d8_real_identity_pressure_keeps_b_and_mixed_human_context_available() {
    let fixture = pressure_fixture();
    let router = gateway_router(fixture.state.clone());
    // Establish valid proof/account/member setup without allocating a victim
    // context before pressure (an existing context would hide the defect).
    for token in [&fixture.b_only, &fixture.mixed] {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        authenticate(&fixture.state, &headers).unwrap();
    }
    let mut admitted_a = 0usize;
    let mut a_refused = false;
    for token in &fixture.a_tokens {
        let (status, identity) = human_get(&router, token, "/api/v1/human/identity", None).await;
        if status == StatusCode::TOO_MANY_REQUESTS {
            assert_eq!(
                identity["error"]["code"], "capacity_exhausted",
                "setup: unrelated429"
            );
            a_refused = true;
            break;
        }
        assert_eq!(
            status,
            StatusCode::OK,
            "setup: A identity failed before capacity"
        );
        let contexts = identity["contexts"].as_array().unwrap();
        assert_eq!(
            contexts.len(),
            MAX_OWNER_CONTEXTS,
            "setup: not all real membership/mode contexts were produced"
        );
        assert!(
            contexts
                .iter()
                .all(|context| context["tenant_id"] == "tenant-a")
        );
        admitted_a += contexts.len();
    }
    assert!(
        admitted_a > 0 && admitted_a <= MAX_CONTEXTS,
        "setup: no real A pressure"
    );
    let (status, victim) =
        human_get(&router, &fixture.b_only, "/api/v1/human/identity", None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "D8: real A identity contexts denied B's first context; admitted_a={admitted_a}, a_refused={a_refused}, error_code={}",
        victim["error"]["code"]
    );
    let contexts = victim["contexts"].as_array().unwrap();
    assert_eq!(contexts.len(), 1);
    assert_eq!(contexts[0]["tenant_id"], "tenant-b");
    assert_eq!(victim["contexts_complete"], true);

    // A sorts before B in configured iteration. Capacity in A must not make
    // an authenticated member of A+B lose their available B context.
    let (status, mixed) = human_get(&router, &fixture.mixed, "/api/v1/human/identity", None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "D8: full A masked the mixed human's B context"
    );
    let contexts = mixed["contexts"].as_array().unwrap();
    assert_eq!(contexts.len(), 1);
    assert_eq!(contexts[0]["tenant_id"], "tenant-b");
    assert_eq!(
        mixed["contexts_complete"], false,
        "A capacity omission must be explicit"
    );
}
