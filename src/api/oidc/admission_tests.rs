//! The real membership helper with a verified provider proof and real Stores.
//! This isolates profile admission from the earlier collision scan. It does
//! not replace the end-to-end intention/admission/rotation/cleanup canaries.
use super::*;
use crate as test_product;
use crate::api::read_execution_tests::{identity_change, local_identity_state};
use crate::audit::{
    authentication::{ApprovedAuthenticationClientV1, AuthenticationAuditPolicyV1},
    read_test_observer::{Call, observe},
};
use crate::identity::*;

// The shared issuer's other fault controls are exercised by integration targets.
#[allow(dead_code)]
#[path = "../../../tests/support/oidc_fixture.rs"]
mod controlled;

async fn verified_memberships(
    state: &GatewayState,
    access: &Arc<VerifiedAccess>,
) -> ApiResult<BTreeSet<HumanCapabilityV1>> {
    let access = Arc::clone(access);
    state
        .blocking_identity(move |state| {
            let oidc = state.oidc.as_ref().unwrap();
            membership(&state, oidc, &access)
        })
        .await
}

#[tokio::test]
async fn verified_oidc_memberships_do_not_open_saturated_profile_stores() {
    let (directory, state, _local_token) = local_identity_state();
    let issuer = controlled::ControlledIssuer::new().await;
    let principal = PrincipalRefV1::HumanOidc {
        issuer: issuer.issuer.clone(),
        subject: "profile-reader".into(),
    };
    for tenant in state.tenants.iter() {
        tenant
            .store
            .mutate_identity(&identity_change(IdentityChangeV1::Account {
                expected_revision: 0,
                account: HumanAccountV1 {
                    version: 1,
                    principal: principal.clone(),
                    display_name: "OIDC profile reader".into(),
                    revision: 1,
                    active: true,
                },
            }))
            .unwrap();
        tenant
            .store
            .mutate_identity(&identity_change(IdentityChangeV1::Membership {
                expected_revision: 0,
                membership: MembershipV1 {
                    version: 1,
                    membership_id: Uuid::new_v4().to_string(),
                    principal: principal.clone(),
                    tenant_id: tenant.tenant_id.clone(),
                    organization_scope: "organization-ab".into(),
                    revision: 1,
                    active: true,
                    capabilities: BTreeSet::from([HumanCapabilityV1::ReadMetadata]),
                },
            }))
            .unwrap();
    }
    let audit = Arc::new(
        AuthenticationAudit::init_new(
            &directory.path().join("authentication.sqlite"),
            Uuid::new_v4(),
            AuthenticationAuditPolicyV1 {
                max_records: 1000,
                max_serialized_bytes: 4 * 1024 * 1024,
                approved_clients: ["web-public", "cli-public"]
                    .map(|client_id| ApprovedAuthenticationClientV1 {
                        issuer: issuer.issuer.clone(),
                        client_id: client_id.into(),
                    })
                    .to_vec(),
            },
            Redactor::new(vec![]),
        )
        .unwrap(),
    );
    let oidc = OidcGateway::new(
        OidcGatewayConfigV1 {
            version: 1,
            provider: issuer.config(),
            provider_label: "Controlled issuer".into(),
            web_origin: "http://127.0.0.1:18100".into(),
            mapping: OidcClaimsMappingV1 {
                revision: "profile-admission-1".into(),
                default_capabilities: BTreeSet::from([HumanCapabilityV1::ReadMetadata]),
                rules: vec![],
            },
        },
        audit,
        None,
    )
    .unwrap();
    let state = state.with_oidc(oidc).unwrap();
    let oidc = state.oidc.as_ref().unwrap();
    let raw = issuer.token(
        "cli-public",
        "profile-reader",
        None,
        &[],
        "profile-admission",
    );
    let access = Arc::new(oidc.provider.verify_access(&raw).await.unwrap());
    assert_eq!(access.subject(), "profile-reader");
    // Satisfy the helper's durable-intention precondition after real signature
    // verification. The integration canaries verify install's ordering itself.
    oidc.audit
        .append_typed(record(
            oidc,
            Action::SessionAdmission,
            Uuid::new_v4(),
            "cli-public",
            None,
            None,
            Uuid::new_v4(),
        ))
        .unwrap();
    let observations_a = observe(&state.tenants[0].store);
    let observations_b = observe(&state.tenants[1].store);
    let expected = BTreeSet::from([HumanCapabilityV1::ReadMetadata]);
    assert_eq!(
        verified_memberships(&state, &access).await.unwrap(),
        expected
    );
    let baseline_a = observations_a
        .try_iter()
        .filter(|sample| sample.call == Call::BeginChecked)
        .count();
    let baseline_b = observations_b
        .try_iter()
        .filter(|sample| sample.call == Call::BeginChecked)
        .count();
    assert!(baseline_a > 0 && baseline_b > 0);

    let _slots_b: Vec<_> = (0..4)
        .map(|_| {
            state.tenants[1]
                .io
                .admit(store_io::Lane::Read, false)
                .unwrap()
        })
        .collect();
    let actual = verified_memberships(&state, &access).await;
    let transactions_b = observations_b
        .try_iter()
        .filter(|sample| sample.call == Call::BeginChecked)
        .count();
    assert_eq!(
        transactions_b, 0,
        "OIDC membership bypassed the saturated profile B admission"
    );
    assert_eq!(
        actual.unwrap(),
        expected,
        "the verified healthy A membership remains usable"
    );
    assert!(
        observations_a
            .try_iter()
            .filter(|sample| sample.call == Call::BeginChecked)
            .count()
            > 0
    );

    let _slots_a: Vec<_> = (0..4)
        .map(|_| {
            state.tenants[0]
                .io
                .admit(store_io::Lane::Read, false)
                .unwrap()
        })
        .collect();
    let refused = verified_memberships(&state, &access).await.unwrap_err().0;
    assert!(matches!(
        refused,
        StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE
    ));
    assert_eq!(
        observations_a
            .try_iter()
            .filter(|sample| sample.call == Call::BeginChecked)
            .count(),
        0
    );
    assert_eq!(
        observations_b
            .try_iter()
            .filter(|sample| sample.call == Call::BeginChecked)
            .count(),
        0
    );
}
