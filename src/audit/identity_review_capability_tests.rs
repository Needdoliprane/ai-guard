//! Compatibility tests: an explicit approval capability is never inferred from
//! an existing resource permission or admitted to an older identity history.
use super::*;

fn provision() -> LocalHumanProvisionV1 {
    let principal = PrincipalRefV1::HumanLocal {
        installation_id: "11111111-1111-4111-8111-111111111111".into(),
        account_id: "22222222-2222-4222-8222-222222222222".into(),
    };
    let actor = PrincipalRefV1::LegacyControl {
        credential_id: "synthetic-control".into(),
        role: LegacyControlRoleV1::Runner,
    };
    let change = |id: &str, change| IdentityMutationV1 {
        version: 1,
        operation_id: id.into(),
        actor: actor.clone(),
        change,
    };
    LocalHumanProvisionV1 {
        version: 1,
        mutations: [
            change(
                "33333333-3333-4333-8333-333333333333",
                IdentityChangeV1::Account {
                    expected_revision: 0,
                    account: HumanAccountV1 {
                        version: 1,
                        principal: principal.clone(),
                        display_name: "Synthetic approval fixture".into(),
                        revision: 1,
                        active: true,
                    },
                },
            ),
            change(
                "44444444-4444-4444-8444-444444444444",
                IdentityChangeV1::Credential {
                    expected_revision: 0,
                    binding: LocalCredentialV1 {
                        version: 1,
                        credential_id: "55555555-5555-4555-8555-555555555555".into(),
                        principal: principal.clone(),
                        sha256_digest: "a".repeat(64),
                        revision: 1,
                        active: true,
                        expires_at: None,
                    },
                },
            ),
            change(
                "66666666-6666-4666-8666-666666666666",
                IdentityChangeV1::Membership {
                    expected_revision: 0,
                    membership: MembershipV1 {
                        version: 1,
                        membership_id: "77777777-7777-4777-8777-777777777777".into(),
                        principal,
                        tenant_id: "tenant-a".into(),
                        organization_scope: "org-a".into(),
                        revision: 1,
                        active: true,
                        capabilities: [HumanCapabilityV1::ReadContent].into_iter().collect(),
                    },
                },
            ),
        ],
    }
}

fn with_approval(mut input: LocalHumanProvisionV1) -> LocalHumanProvisionV1 {
    let IdentityChangeV1::Membership { membership, .. } = &mut input.mutations[2].change else {
        panic!("test fixture requires a membership");
    };
    membership
        .capabilities
        .insert(HumanCapabilityV1::ApproveLaunch);
    input
}

#[test]
fn approval_capability_is_personal_and_not_a_legacy_resource_permission() {
    let capability: HumanCapabilityV1 = serde_json::from_str("\"approve_launch\"").unwrap();
    assert_eq!(capability, HumanCapabilityV1::ApproveLaunch);
    assert!(!capability.is_administration());
    assert!(serde_json::from_str::<ResourcePermissionV1>("\"approve_launch\"").is_err());
    for name in ["metadata", "content", "export"] {
        assert!(serde_json::from_value::<ResourcePermissionV1>(serde_json::json!(name)).is_ok());
    }
}

#[test]
fn approval_membership_requires_exact_format_fourteen_even_when_inactive() {
    let ordinary = provision();
    let mut approval = with_approval(ordinary.clone());
    for version in 1..=15 {
        assert!(require_approval_capability_format(&ordinary.mutations[2], version, false).is_ok());
        let result = require_approval_capability_format(&approval.mutations[2], version, false);
        assert_eq!(result.is_ok(), version == 14, "format {version}");
        if let Err(error) = result {
            assert_eq!(
                error.downcast_ref::<IdentityFailureV1>(),
                Some(&IdentityFailureV1::MigrationRequired)
            );
        }
    }
    let IdentityChangeV1::Membership { membership, .. } = &mut approval.mutations[2].change else {
        panic!("test fixture requires a membership");
    };
    membership.active = false;
    assert!(require_approval_capability_format(&approval.mutations[2], 13, false).is_err());
    // On a legacy store the explicit human rights marker admits it.
    assert!(require_approval_capability_format(&approval.mutations[2], 13, true).is_ok());
}

#[test]
fn legacy_provision_and_idempotent_mutation_refuse_approval_before_any_write() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let store = Store::create_supervised_for_tenant(
        &directory.path().join("audit.sqlite"),
        "tenant-a",
        Redactor::new(vec![]),
        1000,
        &directory.path().join("before-v3.sqlite"),
    )
    .unwrap();
    store
        .migrate_identity_v4(&directory.path().join("before-v4.sqlite"))
        .unwrap();
    let ordinary = provision();
    let approval = with_approval(ordinary.clone());
    approval.validate().unwrap();
    let original_head = store.head().unwrap();
    let error = store.provision_local_human(&approval).unwrap_err();
    assert_eq!(
        error.downcast_ref::<IdentityFailureV1>(),
        Some(&IdentityFailureV1::MigrationRequired)
    );
    assert_eq!(store.head().unwrap(), original_head);
    {
        let db = store.db.lock().unwrap();
        let count: i64 = db
            .query_row("SELECT count(*) FROM identity_operations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
    }
    let receipts = store.provision_local_human(&ordinary).unwrap();
    let committed_head = store.head().unwrap();
    assert_eq!(receipts.len(), 3);
    // Same operation ID as an already durable ordinary membership. The modern
    // capability fence must run before replay/conflict can return a receipt.
    let error = store.mutate_identity(&approval.mutations[2]).unwrap_err();
    assert_eq!(
        error.downcast_ref::<IdentityFailureV1>(),
        Some(&IdentityFailureV1::MigrationRequired)
    );
    let error = store.provision_local_human(&approval).unwrap_err();
    assert_eq!(
        error.downcast_ref::<IdentityFailureV1>(),
        Some(&IdentityFailureV1::MigrationRequired)
    );
    assert_eq!(store.head().unwrap(), committed_head);
    assert_eq!(store.provision_local_human(&ordinary).unwrap(), receipts);
    assert!(store.integrity().unwrap().valid);
}
