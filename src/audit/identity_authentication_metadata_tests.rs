//! Real private identity projection: bounded preflight is not an authority.
use super::*;
use uuid::Uuid;

#[test]
fn authentication_metadata_refuses_membership_overflow_and_ignores_inactive_account_rows() {
    use std::os::unix::fs::PermissionsExt;

    let files = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let path = files.path().join("identity.sqlite");
    let store = Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 10_000).unwrap();
    store
        .migrate_control_v3(&files.path().join("before-v3"))
        .unwrap();
    store
        .migrate_identity_v4(&files.path().join("before-v4"))
        .unwrap();
    let principal = PrincipalRefV1::HumanOidc {
        issuer: "https://identity.example.test".into(),
        subject: "metadata-reader".into(),
    };
    let account = HumanAccountV1 {
        version: 1,
        principal: principal.clone(),
        display_name: "Synthetic metadata reader".into(),
        revision: 1,
        active: true,
    };
    let mutate = |change| {
        store
            .mutate_identity(&IdentityMutationV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                actor: PrincipalRefV1::LegacyControl {
                    credential_id: "metadata-test-operator".into(),
                    role: LegacyControlRoleV1::Runner,
                },
                change,
            })
            .unwrap();
    };
    mutate(IdentityChangeV1::Account {
        expected_revision: 0,
        account: account.clone(),
    });
    for index in 0..257 {
        mutate(IdentityChangeV1::Membership {
            expected_revision: 0,
            membership: MembershipV1 {
                version: 1,
                membership_id: Uuid::new_v4().to_string(),
                principal: principal.clone(),
                tenant_id: "tenant-a".into(),
                organization_scope: format!("metadata-organization-{index}"),
                revision: 1,
                active: true,
                capabilities: std::collections::BTreeSet::from([HumanCapabilityV1::ReadMetadata]),
            },
        });
        if index == 0 {
            let snapshot = store
                .authentication_membership_snapshot(&principal)
                .unwrap();
            assert_eq!(snapshot.account.as_ref(), Some(&account));
            assert_eq!(snapshot.memberships.len(), 1);
            assert_eq!(
                snapshot.authorization_revision,
                store.identity_authorization_revision().unwrap()
            );
        }
    }
    let revision = store.identity_authorization_revision().unwrap();
    let error = store
        .authentication_membership_snapshot(&principal)
        .err()
        .unwrap();
    assert_eq!(error.to_string(), "human membership capacity exceeded");
    assert_eq!(store.identity_authorization_revision().unwrap(), revision);

    mutate(IdentityChangeV1::Account {
        expected_revision: 1,
        account: HumanAccountV1 {
            active: false,
            revision: 2,
            ..account
        },
    });
    let snapshot = store
        .authentication_membership_snapshot(&principal)
        .unwrap();
    assert!(!snapshot.account.unwrap().active);
    assert!(snapshot.memberships.is_empty());
    assert_eq!(snapshot.authorization_revision, revision + 1);
}
