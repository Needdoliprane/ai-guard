//! Real Router/local-proof controls with direct registry pressure. Unlike the
//! separate full producer regression, the pressure bindings here are declared
//! synthetic sink inputs; only the victim takes the authenticated route.
use super::*;
use crate::api::{
    human_state::MAX_CONTEXTS,
    read_execution_tests::{human_get, identity_change, local_identity_state},
};

fn fill_tenant(state: &GatewayState, tenant: &str, first_owner: u32) {
    // The actual fixture has exactly two configured tenants. Fill one fixed
    // share using separate synthetic principals, respecting the owner cap.
    for index in 0..MAX_CONTEXTS / 2 {
        let owner = first_owner + u32::try_from(index / MAX_OWNER_CONTEXTS).unwrap();
        let digest: [u8; 32] = Sha256::digest(owner.to_be_bytes()).into();
        state
            .human
            .contexts
            .obtain(
                ContextBinding {
                    digest,
                    principal_key: hex::encode(digest),
                    credential_id: uuid::Uuid::from_u128(u128::from(owner)).to_string(),
                    credential_revision: 1,
                    authentication_method: "local_credential".into(),
                    mapping_revision: None,
                    tenant_id: tenant.into(),
                    organization_scope: "fixture-organization".into(),
                    membership_id: uuid::Uuid::from_u128(u128::try_from(index + 1).unwrap())
                        .to_string(),
                    mode: ContextModeV1::Personal,
                    membership_revision: 1,
                    authorization_revision: 1,
                },
                CONTEXT_TTL,
                "fixture-expiry".into(),
            )
            .unwrap();
    }
}

#[tokio::test]
async fn full_a_yields_partial_b_context_without_replacing_its_live_binding() {
    let (_directory, state, token) = local_identity_state();
    fill_tenant(&state, "tenant-a", 10_000);
    let router = gateway_router(state);
    let (status, identity) = human_get(&router, &token, "/api/v1/human/identity", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(identity["contexts_complete"], false);
    let contexts = identity["contexts"].as_array().unwrap();
    assert_eq!(contexts.len(), 1);
    assert_eq!(contexts[0]["tenant_id"], "tenant-b");
    let context = contexts[0]["id"].as_str().unwrap();
    let original_expiry = contexts[0]["expires_at"].clone();
    let (status, _) = human_get(&router, &token, "/api/v1/human/resources", Some(context)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "available B context did not authorize its real read"
    );
    let (status, repeated) = human_get(&router, &token, "/api/v1/human/identity", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(repeated["contexts"][0]["id"], context);
    assert_eq!(repeated["contexts"][0]["expires_at"], original_expiry);
    assert_eq!(repeated["contexts_complete"], false);
}

#[tokio::test]
async fn all_shares_full_return_capacity_and_credential_revocation_keeps_its_error() {
    let (_directory, state, token) = local_identity_state();
    fill_tenant(&state, "tenant-a", 10_000);
    fill_tenant(&state, "tenant-b", 20_000);
    let router = gateway_router(state.clone());
    let (status, capacity) = human_get(&router, &token, "/api/v1/human/identity", None).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(capacity["error"]["code"], "capacity_exhausted");
    assert_eq!(capacity["error"]["retry_action"], "retry");
    let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
    let mut credential = state.tenants[1]
        .store
        .local_credential_binding(&digest)
        .unwrap()
        .unwrap();
    credential.active = false;
    credential.revision += 1;
    state.tenants[1]
        .store
        .mutate_identity(&identity_change(IdentityChangeV1::Credential {
            expected_revision: credential.revision - 1,
            binding: credential,
        }))
        .unwrap();
    let (status, revoked) = human_get(&router, &token, "/api/v1/human/identity", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(revoked["error"]["code"], "identity_revoked");
    assert_eq!(revoked["error"]["retry_action"], "authenticate");
}
