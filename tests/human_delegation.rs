//! Delegated identity changes by an organization administrator never raise a
//! right: no self-change, no right the actor does not hold, no owner change.
//! Real private SQLite stores.
mod support;
use agentsandbox::{audit::Store, identity::*, redaction::Redactor};
use sha2::{Digest, Sha256};
use std::path::Path;
use uuid::Uuid;

fn migrated(path: &Path, through_workers: bool) -> Store {
    let store = Store::open_for_tenant(path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    let steps: Vec<fn(&Store, &Path) -> anyhow::Result<agentsandbox::control::MigrationReportV1>> = vec![
        Store::migrate_control_v3,
        Store::migrate_identity_v4,
        Store::migrate_human_access_v5,
        Store::migrate_workload_v6,
        Store::migrate_human_brokers_v7,
        Store::migrate_kubernetes_create_v8,
        Store::migrate_kubernetes_policy_v9,
        Store::migrate_human_workers_v10,
        Store::migrate_kubernetes_session_reservations_v11,
        Store::migrate_broker_checkpoints_v12,
        Store::migrate_kubernetes_recovery_v13,
    ];
    let count = if through_workers { steps.len() } else { 7 };
    for (index, step) in steps.into_iter().take(count).enumerate() {
        step(&store, &path.with_extension(format!("before{}", index + 3))).unwrap();
    }
    store
}
fn control(change: IdentityChangeV1) -> IdentityMutationV1 {
    by(
        PrincipalRefV1::LegacyControl {
            credential_id: "human-rights-migration-control".into(),
            role: LegacyControlRoleV1::Runner,
        },
        change,
    )
}
fn by(actor: PrincipalRefV1, change: IdentityChangeV1) -> IdentityMutationV1 {
    IdentityMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        actor,
        change,
    }
}
fn preparation(capabilities: &[HumanCapabilityV1]) -> (LocalHumanProvisionV1, MembershipV1) {
    let principal = PrincipalRefV1::HumanLocal {
        installation_id: Uuid::new_v4().to_string(),
        account_id: Uuid::new_v4().to_string(),
    };
    let token = generate_local_credential().unwrap();
    let membership = MembershipV1 {
        version: 1,
        membership_id: Uuid::new_v4().to_string(),
        principal: principal.clone(),
        tenant_id: "tenant-a".into(),
        organization_scope: "organization-a".into(),
        revision: 1,
        active: true,
        capabilities: capabilities.iter().copied().collect(),
    };
    let provision = LocalHumanProvisionV1 {
        version: 1,
        mutations: [
            control(IdentityChangeV1::Account {
                expected_revision: 0,
                account: HumanAccountV1 {
                    version: 1,
                    principal: principal.clone(),
                    display_name: "Human rights reader".into(),
                    revision: 1,
                    active: true,
                },
            }),
            control(IdentityChangeV1::Credential {
                expected_revision: 0,
                binding: LocalCredentialV1 {
                    version: 1,
                    credential_id: Uuid::new_v4().to_string(),
                    principal,
                    sha256_digest: hex::encode(Sha256::digest(token.as_bytes())),
                    revision: 1,
                    active: true,
                    expires_at: None,
                },
            }),
            control(IdentityChangeV1::Membership {
                expected_revision: 0,
                membership: membership.clone(),
            }),
        ],
    };
    (provision, membership)
}
fn refusal(result: anyhow::Result<impl std::fmt::Debug>) -> Option<IdentityFailureV1> {
    result
        .unwrap_err()
        .downcast_ref::<IdentityFailureV1>()
        .copied()
}
/// Delegated membership changes (S6): an organization administrator may keep or
/// remove a capability, may add only one it holds itself, and never changes its
/// own membership.
#[test]
fn delegated_membership_changes_never_raise_a_right() {
    let files = support::private_tempdir();
    let path = files.path().join("tenant.sqlite");
    let store = migrated(&path, true);
    store
        .migrate_human_rights_v2(&path.with_extension("exact13"))
        .unwrap();
    let (admin, admin_membership) = preparation(&[
        HumanCapabilityV1::ManageMemberships,
        HumanCapabilityV1::ReadMemberships,
        HumanCapabilityV1::ReadMetadata,
    ]);
    store.provision_local_human(&admin).unwrap();
    let (member, member_membership) =
        preparation(&[HumanCapabilityV1::ReadMetadata, HumanCapabilityV1::Export]);
    store.provision_local_human(&member).unwrap();
    let by_admin =
        |membership: &MembershipV1, capabilities: &[HumanCapabilityV1], revision: u64| {
            by(
                admin_membership.principal.clone(),
                IdentityChangeV1::Membership {
                    expected_revision: revision,
                    membership: MembershipV1 {
                        revision: revision + 1,
                        capabilities: capabilities.iter().copied().collect(),
                        ..membership.clone()
                    },
                },
            )
        };
    let head = store.head().unwrap();
    assert_eq!(
        refusal(store.mutate_identity(&by_admin(
            &member_membership,
            &[
                HumanCapabilityV1::ReadMetadata,
                HumanCapabilityV1::ReadContent
            ],
            1
        ))),
        Some(IdentityFailureV1::PermissionDenied),
        "read_content is not held by the administrator"
    );
    assert_eq!(
        refusal(store.mutate_identity(&by_admin(
            &admin_membership,
            &[
                HumanCapabilityV1::ManageMemberships,
                HumanCapabilityV1::ReadMemberships
            ],
            1
        ))),
        Some(IdentityFailureV1::PermissionDenied),
        "no change of one's own membership, even a removal"
    );
    assert_eq!(store.head().unwrap(), head);
    // Keeping a capability the administrator lacks while removing another is allowed.
    store
        .mutate_identity(&by_admin(
            &member_membership,
            &[HumanCapabilityV1::Export],
            1,
        ))
        .unwrap();
    // Re-adding a held capability is allowed.
    store
        .mutate_identity(&by_admin(
            &member_membership,
            &[HumanCapabilityV1::Export, HumanCapabilityV1::ReadMetadata],
            2,
        ))
        .unwrap();
    // Re-adding a removed capability the administrator lacks is refused.
    let removed = MembershipV1 {
        revision: 3,
        capabilities: [HumanCapabilityV1::ReadMetadata].into_iter().collect(),
        ..member_membership.clone()
    };
    store
        .mutate_identity(&control(IdentityChangeV1::Membership {
            expected_revision: 3,
            membership: MembershipV1 {
                revision: 4,
                ..removed.clone()
            },
        }))
        .unwrap();
    assert_eq!(
        refusal(store.mutate_identity(&by_admin(
            &MembershipV1 {
                revision: 4,
                ..removed
            },
            &[HumanCapabilityV1::ReadMetadata, HumanCapabilityV1::Export],
            4
        ))),
        Some(IdentityFailureV1::PermissionDenied)
    );
    drop(store);
    // Every accepted delegated change replays at reopen.
    Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
}

/// Delegated share changes (S6): a share never changes the owner, never
/// grants the actor itself, and adds only a permission the actor holds.
#[test]
fn delegated_share_changes_never_raise_a_right() {
    use agentsandbox::model::Event;
    let files = support::private_tempdir();
    let path = files.path().join("tenant.sqlite");
    let store = migrated(&path, true);
    store
        .migrate_human_rights_v2(&path.with_extension("exact13"))
        .unwrap();
    let (admin, admin_membership) = preparation(&[
        HumanCapabilityV1::ManageGrants,
        HumanCapabilityV1::ReadMetadata,
    ]);
    store.provision_local_human(&admin).unwrap();
    let (owner, owner_membership) = preparation(&[HumanCapabilityV1::ReadMetadata]);
    store.provision_local_human(&owner).unwrap();
    let (reader, reader_membership) = preparation(&[HumanCapabilityV1::ReadMetadata]);
    store.provision_local_human(&reader).unwrap();
    let mut event = Event::new("fixture.authorized");
    event.session_id = Some("session-shared".into());
    store.append(event).unwrap();
    let grant =
        |owner: &PrincipalRefV1, shares: Vec<ResourceShareV1>, revision: u64| ResourceGrantV1 {
            version: 1,
            tenant_id: "tenant-a".into(),
            organization_scope: "organization-a".into(),
            resource: ResourceRefV1 {
                kind: ResourceKindV1::Session,
                resource_id: "session-shared".into(),
            },
            owner: owner.clone(),
            revision,
            active: true,
            shares,
        };
    let share =
        |principal: &PrincipalRefV1, permissions: &[ResourcePermissionV1]| ResourceShareV1 {
            principal: principal.clone(),
            permissions: permissions.iter().copied().collect(),
        };
    store
        .mutate_identity(&control(IdentityChangeV1::ResourceGrant {
            expected_revision: 0,
            grant: grant(&owner_membership.principal, vec![], 1),
        }))
        .unwrap();
    let by_admin = |grant: ResourceGrantV1| {
        by(
            admin_membership.principal.clone(),
            IdentityChangeV1::ResourceGrant {
                expected_revision: grant.revision - 1,
                grant,
            },
        )
    };
    let head = store.head().unwrap();
    for (refused, why) in [
        (
            grant(&admin_membership.principal, vec![], 2),
            "an administrator does not take the ownership",
        ),
        (
            grant(
                &owner_membership.principal,
                vec![share(
                    &reader_membership.principal,
                    &[ResourcePermissionV1::Content],
                )],
                2,
            ),
            "content is not held by the administrator",
        ),
        (
            grant(
                &owner_membership.principal,
                vec![share(
                    &admin_membership.principal,
                    &[ResourcePermissionV1::Metadata],
                )],
                2,
            ),
            "no share to oneself",
        ),
    ] {
        assert_eq!(
            refusal(store.mutate_identity(&by_admin(refused))),
            Some(IdentityFailureV1::PermissionDenied),
            "{why}"
        );
    }
    assert_eq!(store.head().unwrap(), head);
    store
        .mutate_identity(&by_admin(grant(
            &owner_membership.principal,
            vec![share(
                &reader_membership.principal,
                &[ResourcePermissionV1::Metadata],
            )],
            2,
        )))
        .unwrap();
}
