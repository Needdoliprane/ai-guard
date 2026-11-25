use super::*;
use crate::identity::{
    HumanOperationV1, MembershipV1, OperationAvailabilityV1, OperationFactsV1,
    OperationPermissionV1, authorize_context, context_operations,
};
use HumanCapabilityV1 as C;
use IdentityFailureV1 as F;
use SessionLaunchTargetFailureV1 as LaunchFailure;

const TARGET_ID: &str = "22222222-2222-4222-8222-222222222222";
const OTHER_TARGET_ID: &str = "33333333-3333-4333-8333-333333333333";
const TENANT: &str = "tenant-a";
const ORGANIZATION: &str = "org-a";

fn local(account: u128) -> PrincipalRefV1 {
    PrincipalRefV1::HumanLocal {
        installation_id: uuid::Uuid::from_u128(0x1000).to_string(),
        account_id: uuid::Uuid::from_u128(account).to_string(),
    }
}

fn scope(principal: PrincipalRefV1, mode: ContextModeV1, capabilities: &[C]) -> AuthorizedScopeV1 {
    let member = MembershipV1 {
        version: 1,
        membership_id: uuid::Uuid::from_u128(0x2001).to_string(),
        principal: principal.clone(),
        tenant_id: TENANT.into(),
        organization_scope: ORGANIZATION.into(),
        revision: 1,
        active: true,
        capabilities: capabilities.iter().copied().collect(),
    };
    authorize_context(&principal, &member, mode, 1).unwrap()
}

fn policy(launchers: &[PrincipalRefV1]) -> SessionLaunchTargetPolicyV1<'_> {
    SessionLaunchTargetPolicyV1 {
        selection: SessionLaunchTargetSelectionV1::parse(TARGET_ID, "7").unwrap(),
        active: true,
        tenant_id: TENANT,
        organization_scope: ORGANIZATION,
        launchers,
    }
}

#[test]
fn only_an_explicit_personal_launcher_can_list_and_launch() {
    let launchers = [local(1)];
    let target = policy(&launchers);
    let launcher = scope(local(1), ContextModeV1::Personal, &[C::LaunchSession]);
    assert!(target.may_list(&launcher));
    assert_eq!(target.authorize_launch(&launcher, target.selection), Ok(()));

    let reader = scope(
        local(1),
        ContextModeV1::Personal,
        &[
            C::ReadMetadata,
            C::ReadContent,
            C::Export,
            C::ApproveLaunch,
            C::SendSessionMessage,
        ],
    );
    let admin = scope(
        local(1),
        ContextModeV1::Administration,
        &[C::LaunchSession, C::ReadTenantMetadata],
    );
    for context in [&reader, &admin] {
        assert!(!target.may_list(context));
        assert_eq!(
            target.authorize_launch(context, target.selection),
            Err(LaunchFailure::Identity(F::PermissionDenied))
        );
    }
}

#[test]
fn launch_hides_targets_outside_the_acl_tenant_or_organization() {
    let launchers = [local(1)];
    let target = policy(&launchers);
    let named = scope(local(1), ContextModeV1::Personal, &[C::LaunchSession]);
    let outsider = scope(local(2), ContextModeV1::Personal, &[C::LaunchSession]);
    let other_tenant = SessionLaunchTargetPolicyV1 {
        tenant_id: "tenant-b",
        ..target
    };
    let other_organization = SessionLaunchTargetPolicyV1 {
        organization_scope: "org-b",
        ..target
    };
    let inactive = SessionLaunchTargetPolicyV1 {
        active: false,
        ..target
    };
    let stale = SessionLaunchTargetSelectionV1::parse(TARGET_ID, "6").unwrap();

    for (policy, context) in [
        (target, &outsider),
        (other_tenant, &named),
        (other_organization, &named),
        (inactive, &named),
    ] {
        assert!(!policy.may_list(context));
        assert_eq!(
            policy.authorize_launch(context, stale),
            Err(LaunchFailure::Identity(F::NotFound))
        );
    }
}

#[test]
fn launch_requires_the_exact_visible_target_and_revision() {
    let launchers = [local(1)];
    let target = policy(&launchers);
    let caller = scope(local(1), ContextModeV1::Personal, &[C::LaunchSession]);
    let wrong_target = SessionLaunchTargetSelectionV1::parse(OTHER_TARGET_ID, "7").unwrap();
    let stale = SessionLaunchTargetSelectionV1::parse(TARGET_ID, "6").unwrap();

    assert_eq!(
        target.authorize_launch(&caller, wrong_target),
        Err(LaunchFailure::Identity(F::NotFound))
    );
    assert_eq!(
        target.authorize_launch(&caller, stale),
        Err(LaunchFailure::Stale)
    );
    assert!(target.may_list(&caller));
    assert_eq!(target.authorize_launch(&caller, target.selection), Ok(()));
    let duplicates = [local(1), local(1)];
    let untrusted = policy(&duplicates);
    assert!(!untrusted.may_list(&caller));
    assert_eq!(
        untrusted.authorize_launch(&caller, target.selection),
        Err(LaunchFailure::Identity(F::IdentityUnavailable))
    );
}

#[test]
fn named_oidc_launch_respects_the_effective_capability_ceiling() {
    let named = PrincipalRefV1::HumanOidc {
        issuer: "https://issuer.example.invalid".into(),
        subject: "named-launcher".into(),
    };
    let launchers = [named.clone()];
    let target = policy(&launchers);
    let named_scope = scope(
        named,
        ContextModeV1::Personal,
        &[C::ReadMetadata, C::LaunchSession],
    );
    assert!(named_scope.has_capability(C::LaunchSession));
    assert!(!target.may_list(&named_scope));
    assert_eq!(
        target.authorize_launch(&named_scope, target.selection),
        Err(LaunchFailure::Identity(F::PermissionDenied))
    );
    let allowed = named_scope
        .clone()
        .restrict_capabilities([C::LaunchSession].into_iter().collect())
        .unwrap();
    assert!(target.may_list(&allowed));
    assert_eq!(target.authorize_launch(&allowed, target.selection), Ok(()));

    let restricted = named_scope
        .restrict_capabilities([C::ReadMetadata].into_iter().collect())
        .unwrap();
    assert!(!target.may_list(&restricted));
    assert_eq!(
        target.authorize_launch(&restricted, target.selection),
        Err(LaunchFailure::Identity(F::PermissionDenied))
    );
}

#[test]
fn owned_launch_right_matches_the_operation_registry_without_serving_routes() {
    let launchers = [local(1)];
    let target = policy(&launchers);
    let membership = [C::LaunchSession, C::ReadTenantMetadata];
    for (mode, effective, permission) in [
        (
            ContextModeV1::Personal,
            &[C::LaunchSession][..],
            OperationPermissionV1::Granted,
        ),
        (
            ContextModeV1::Administration,
            &membership[..],
            OperationPermissionV1::RequiresPersonal,
        ),
    ] {
        let context = scope(local(1), mode, &membership);
        assert_eq!(
            target.may_list(&context),
            permission == OperationPermissionV1::Granted
        );
        // Both owned formats use the same operation facts. The marker is
        // independent of the permission. Prepare and Launch are both served on
        // the owned profile and gated by the same marker.
        for human_rights_marker in [false, true] {
            let registry = context_operations(
                mode,
                &effective.iter().copied().collect(),
                &membership.into_iter().collect(),
                OperationFactsV1 {
                    owned_profile: true,
                    human_rights_marker,
                },
            );
            for operation in [
                HumanOperationV1::SessionLaunchPrepare,
                HumanOperationV1::SessionLaunch,
            ] {
                let entry = registry
                    .iter()
                    .find(|entry| entry.operation == operation)
                    .unwrap();
                assert_eq!(entry.permission, permission);
                let expected = if human_rights_marker {
                    OperationAvailabilityV1::Available
                } else {
                    OperationAvailabilityV1::MigrationRequired
                };
                assert_eq!(entry.availability, expected);
            }
        }
    }
}
