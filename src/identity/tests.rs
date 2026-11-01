use super::*;
use std::collections::BTreeSet;

fn local(account: &str) -> PrincipalRefV1 {
    PrincipalRefV1::HumanLocal {
        installation_id: "909d9572-f1a2-4a53-ad8b-a741030a2b18".into(),
        account_id: account.into(),
    }
}

fn first() -> PrincipalRefV1 {
    local("871be6f3-b1a5-4eb9-9c72-989cedb05c43")
}

fn second() -> PrincipalRefV1 {
    local("f7e62c9f-54bb-4e99-99b9-99902da33cd6")
}

fn membership(principal: PrincipalRefV1) -> MembershipV1 {
    MembershipV1 {
        version: 1,
        membership_id: "2cfeaa19-43aa-4666-9584-c0eb81c95366".into(),
        principal,
        tenant_id: "tenant-a".into(),
        organization_scope: "organization-a".into(),
        revision: 3,
        active: true,
        capabilities: BTreeSet::from([
            HumanCapabilityV1::ReadMetadata,
            HumanCapabilityV1::ReadContent,
            HumanCapabilityV1::Export,
        ]),
    }
}

fn grant() -> ResourceGrantV1 {
    ResourceGrantV1 {
        version: 1,
        tenant_id: "tenant-a".into(),
        resource: ResourceRefV1 {
            kind: ResourceKindV1::Session,
            resource_id: "session-a".into(),
        },
        organization_scope: "organization-a".into(),
        owner: first(),
        revision: 1,
        active: true,
        shares: vec![ResourceShareV1 {
            principal: second(),
            permissions: BTreeSet::from([ResourcePermissionV1::Metadata]),
        }],
    }
}

// A shared resource must never inherit its owner's content/export permissions.
#[test]
fn resource_sharing_does_not_promote_metadata_to_content_or_export() {
    let principal = second();
    let scope = authorize_context(
        &principal,
        &membership(principal.clone()),
        ContextModeV1::Personal,
        3,
    )
    .unwrap();
    let grant = grant();
    assert!(scope.permits_resource(&grant, ResourcePermissionV1::Metadata));
    assert!(!scope.permits_resource(&grant, ResourcePermissionV1::Content));
    assert!(!scope.permits_resource(&grant, ResourcePermissionV1::Export));
}

// Ownership cannot bypass a membership capability or cross a tenant boundary.
#[test]
fn owner_is_bounded_by_membership_and_tenant() {
    let principal = first();
    let mut member = membership(principal.clone());
    member.capabilities = BTreeSet::from([HumanCapabilityV1::ReadMetadata]);
    let scope = authorize_context(&principal, &member, ContextModeV1::Personal, 3).unwrap();
    let mut resource = grant();
    assert!(scope.permits_resource(&resource, ResourcePermissionV1::Metadata));
    assert!(!scope.permits_resource(&resource, ResourcePermissionV1::Content));
    resource.tenant_id = "tenant-b".into();
    assert!(!scope.permits_resource(&resource, ResourcePermissionV1::Metadata));
}

// Administration is explicitly selected and does not imply access to contents.
#[test]
fn administration_requires_explicit_scope_and_does_not_read_private_content() {
    let principal = second();
    let mut member = membership(principal.clone());
    assert!(authorize_context(&principal, &member, ContextModeV1::Administration, 3).is_err());
    member
        .capabilities
        .insert(HumanCapabilityV1::ManageMemberships);
    member
        .capabilities
        .insert(HumanCapabilityV1::ReadTenantMetadata);
    let scope = authorize_context(&principal, &member, ContextModeV1::Administration, 3).unwrap();
    let mut resource = grant();
    resource.shares.clear();
    assert!(scope.permits_resource(&resource, ResourcePermissionV1::Metadata));
    assert!(!scope.permits_resource(&resource, ResourcePermissionV1::Content));
    assert!(!scope.permits_resource(&resource, ResourcePermissionV1::Export));
    let personal = authorize_context(&principal, &member, ContextModeV1::Personal, 3).unwrap();
    assert!(!personal.permits_resource(&resource, ResourcePermissionV1::Metadata));
    assert!(!personal.has_capability(HumanCapabilityV1::ManageMemberships));
}

// Previously issued contexts cannot survive principal or membership changes.
#[test]
fn context_refuses_revoked_stale_or_different_principal() {
    let principal = first();
    let mut member = membership(principal.clone());
    assert!(authorize_context(&second(), &member, ContextModeV1::Personal, 3).is_err());
    assert!(authorize_context(&principal, &member, ContextModeV1::Personal, 2).is_err());
    member.active = false;
    assert!(authorize_context(&principal, &member, ContextModeV1::Personal, 3).is_err());
}

// Neither a shared key nor an empty OIDC subject is a named human principal.
#[test]
fn human_membership_rejects_legacy_control_and_invalid_oidc_identity() {
    let legacy = PrincipalRefV1::LegacyControl {
        credential_id: "legacy-key-a".into(),
        role: LegacyControlRoleV1::Runner,
    };
    assert!(membership(legacy).validate().is_err());
    let oidc = PrincipalRefV1::HumanOidc {
        issuer: "https://identity.example.test/realms/a".into(),
        subject: String::new(),
    };
    assert!(membership(oidc).validate().is_err());
    let a = PrincipalRefV1::HumanOidc {
        issuer: "https://identity.example.test/realms/a".into(),
        subject: "same-subject".into(),
    };
    let b = PrincipalRefV1::HumanOidc {
        issuer: "https://identity.example.test/realms/b".into(),
        subject: "same-subject".into(),
    };
    assert_ne!(a, b);
}

// Duplicate sharing entries must be rejected instead of combined ambiguously.
#[test]
fn resource_grant_rejects_ambiguous_shares_and_disabled_grants() {
    let mut resource = grant();
    resource.shares.push(resource.shares[0].clone());
    assert!(resource.validate().is_err());
    let principal = first();
    let scope = authorize_context(
        &principal,
        &membership(principal.clone()),
        ContextModeV1::Personal,
        3,
    )
    .unwrap();
    resource = grant();
    resource.active = false;
    assert!(!scope.permits_resource(&resource, ResourcePermissionV1::Metadata));
}
