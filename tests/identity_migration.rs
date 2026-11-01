//! Schema4 product storage/CLI regressions. No IdP, model, or container runtime.
mod support;

use agentsandbox::{
    audit::{EventQuery, Store},
    control::*,
    identity::*,
    model::Event,
    policy::{self, Policy},
    redaction::Redactor,
};
use serde_json::json;
use std::{collections::BTreeSet, path::Path};
use uuid::Uuid;

fn principal() -> PrincipalRefV1 {
    PrincipalRefV1::HumanLocal {
        installation_id: "909d9572-f1a2-4a53-ad8b-a741030a2b18".into(),
        account_id: "871be6f3-b1a5-4eb9-9c72-989cedb05c43".into(),
    }
}
fn actor() -> PrincipalRefV1 {
    PrincipalRefV1::LegacyControl {
        credential_id: "operator-fixture".into(),
        role: LegacyControlRoleV1::Runner,
    }
}
fn mutation(change: IdentityChangeV1) -> IdentityMutationV1 {
    IdentityMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        actor: actor(),
        change,
    }
}
fn account() -> HumanAccountV1 {
    HumanAccountV1 {
        version: 1,
        principal: principal(),
        display_name: "Local user".into(),
        revision: 1,
        active: true,
    }
}
fn membership() -> MembershipV1 {
    MembershipV1 {
        version: 1,
        membership_id: "2cfeaa19-43aa-4666-9584-c0eb81c95366".into(),
        principal: principal(),
        tenant_id: "tenant-a".into(),
        organization_scope: "organization-a".into(),
        revision: 1,
        active: true,
        capabilities: BTreeSet::from([
            HumanCapabilityV1::ReadMetadata,
            HumanCapabilityV1::ReadContent,
            HumanCapabilityV1::Export,
        ]),
    }
}
fn fresh(path: &Path, max: u64) -> Store {
    Store::create_supervised_for_tenant(
        path,
        "tenant-a",
        Redactor::new(vec![]),
        max,
        &path.with_extension("before-v3.sqlite"),
    )
    .unwrap()
}
fn v4(path: &Path) -> Store {
    let store = fresh(path, 1000);
    store
        .migrate_identity_v4(&path.with_extension("before-v4.sqlite"))
        .unwrap();
    store
}
fn version(path: &Path) -> i64 {
    rusqlite::Connection::open(path)
        .unwrap()
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap()
}

// Migration must append only its genesis, preserve source facts and never grant a human context.
#[test]
fn migration_preserves_history_binding_namespace_and_live_lease_then_reopens_inactive() {
    let files = support::private_tempdir();
    let path = files.path().join("audit.sqlite");
    let backup = files.path().join("before-v4.sqlite");
    let store = fresh(&path, 1000);
    let legacy = store.append(Event::new("legacy.observed")).unwrap();
    let before = store
        .events(&EventQuery {
            order: Some("asc".into()),
            ..Default::default()
        })
        .unwrap();
    let state = store.supervision_state().unwrap();
    assert!(state.active);
    let report = store.migrate_identity_v4(&backup).unwrap();
    assert_eq!((report.from_version, report.to_version), (3, 4));
    assert_eq!(report.genesis_seq, legacy.seq + 1);
    assert_eq!(version(&path), 4);
    let after = store
        .events(&EventQuery {
            order: Some("asc".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        serde_json::to_value(&after[..before.len()]).unwrap(),
        serde_json::to_value(&before).unwrap()
    );
    assert_eq!(store.tenant_id(), "tenant-a");
    let migrated = store.supervision_state().unwrap();
    assert_eq!(
        migrated.control_namespace_uuid,
        state.control_namespace_uuid
    );
    assert_eq!(
        migrated.protocols,
        [SupervisionProtocolV1::BrokerSupervisionV1]
    );
    assert!(
        migrated.active,
        "migration preserves the existing live witness"
    );
    assert_eq!(store.identity_authorization_revision().unwrap(), 0);
    assert_eq!(store.human_account(&principal()).unwrap(), None);
    assert!(
        store
            .authorize_human_context(
                &principal(),
                &membership().membership_id,
                1,
                ContextModeV1::Personal
            )
            .is_err()
    );
    assert!(store.integrity().unwrap().valid);
    drop(store);
    let reopened = Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    assert!(
        !reopened.supervision_state().unwrap().active,
        "reopening4 cannot recreate runtime authority"
    );
    assert!(reopened.integrity().unwrap().valid);
    let backed = Store::open_for_tenant(&backup, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    assert_eq!(backed.supervision_state().unwrap().schema_version, 3);
    assert_eq!(backed.head().unwrap(), legacy.seq);
    assert_eq!(
        backed.supervision_state().unwrap().control_namespace_uuid,
        state.control_namespace_uuid
    );
    assert!(backed.identity_authorization_revision().is_err());
    assert!(backed.integrity().unwrap().valid);
}

// Identity changes must be durable/idempotent and stale grants cannot survive revocation.
#[test]
fn identity_cas_idempotence_replay_and_scope_revocation_are_durable() {
    let files = support::private_tempdir();
    let path = files.path().join("audit.sqlite");
    let store = v4(&path);
    let create = mutation(IdentityChangeV1::Account {
        expected_revision: 0,
        account: account(),
    });
    let receipt = store.mutate_identity(&create).unwrap();
    let head = store.head().unwrap();
    assert_eq!(store.mutate_identity(&create).unwrap(), receipt);
    assert_eq!(store.head().unwrap(), head);
    let mut divergent = create.clone();
    if let IdentityChangeV1::Account { account, .. } = &mut divergent.change {
        account.display_name = "Different payload".into();
    }
    assert_eq!(
        store
            .mutate_identity(&divergent)
            .unwrap_err()
            .downcast_ref::<IdentityFailureV1>(),
        Some(&IdentityFailureV1::Conflict)
    );
    assert_eq!(store.head().unwrap(), head);
    let member = membership();
    store
        .mutate_identity(&mutation(IdentityChangeV1::Membership {
            expected_revision: 0,
            membership: member.clone(),
        }))
        .unwrap();
    let resource = ResourceRefV1 {
        kind: ResourceKindV1::Session,
        resource_id: "session-known-by-control".into(),
    };
    let mut event = Event::new("legacy.session.observed");
    event.session_id = Some(resource.resource_id.clone());
    store.append(event).unwrap();
    assert!(
        store.identity_resource_grant(&resource).unwrap().is_none(),
        "historical presence does not create a grant"
    );
    let grant = ResourceGrantV1 {
        version: 1,
        tenant_id: "tenant-a".into(),
        organization_scope: "organization-a".into(),
        resource: resource.clone(),
        owner: principal(),
        revision: 1,
        active: true,
        shares: vec![],
    };
    store
        .mutate_identity(&mutation(IdentityChangeV1::ResourceGrant {
            expected_revision: 0,
            grant: grant.clone(),
        }))
        .unwrap();
    let scope = store
        .authorize_human_context(
            &principal(),
            &member.membership_id,
            1,
            ContextModeV1::Personal,
        )
        .unwrap();
    assert!(
        store
            .authorized_resource_grant(&scope, &resource, ResourcePermissionV1::Content)
            .unwrap()
            .is_some()
    );
    let mut revoked = member.clone();
    revoked.active = false;
    revoked.revision = 2;
    store
        .mutate_identity(&mutation(IdentityChangeV1::Membership {
            expected_revision: 1,
            membership: revoked.clone(),
        }))
        .unwrap();
    assert!(
        store
            .authorized_resource_grant(&scope, &resource, ResourcePermissionV1::Content)
            .is_err()
    );
    assert!(
        store
            .authorize_human_context(
                &principal(),
                &member.membership_id,
                1,
                ContextModeV1::Personal
            )
            .is_err()
    );
    assert!(store.integrity().unwrap().valid);
    drop(store);
    let reopened = Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    assert_eq!(
        reopened.human_account(&principal()).unwrap(),
        Some(account())
    );
    assert_eq!(
        reopened.identity_memberships(&principal()).unwrap(),
        vec![revoked]
    );
    assert_eq!(
        reopened.identity_resource_grant(&resource).unwrap(),
        Some(grant)
    );
    assert!(
        reopened
            .authorized_resource_grant(&scope, &resource, ResourcePermissionV1::Content)
            .is_err()
    );
    assert_eq!(reopened.mutate_identity(&create).unwrap(), receipt);
    assert!(reopened.integrity().unwrap().valid);
}

// ACL mutations must stay in the confirmed tenant/org and reject invented resource identities.
#[test]
fn identity_refuses_unknown_resources_foreign_tenants_and_organization_privilege() {
    let files = support::private_tempdir();
    let store = v4(&files.path().join("audit.sqlite"));
    store
        .mutate_identity(&mutation(IdentityChangeV1::Account {
            expected_revision: 0,
            account: account(),
        }))
        .unwrap();
    let mut foreign = membership();
    foreign.tenant_id = "tenant-b".into();
    let head = store.head().unwrap();
    assert!(
        store
            .mutate_identity(&mutation(IdentityChangeV1::Membership {
                expected_revision: 0,
                membership: foreign
            }))
            .is_err()
    );
    assert_eq!(store.head().unwrap(), head);
    let mut member = membership();
    member
        .capabilities
        .insert(HumanCapabilityV1::ManageMemberships);
    member.capabilities.insert(HumanCapabilityV1::ManageGrants);
    store
        .mutate_identity(&mutation(IdentityChangeV1::Membership {
            expected_revision: 0,
            membership: member.clone(),
        }))
        .unwrap();
    let mut other = member;
    other.membership_id = Uuid::new_v4().to_string();
    other.organization_scope = "organization-b".into();
    let mut human_change = mutation(IdentityChangeV1::Membership {
        expected_revision: 0,
        membership: other,
    });
    human_change.actor = principal();
    let head = store.head().unwrap();
    assert_eq!(
        store
            .mutate_identity(&human_change)
            .unwrap_err()
            .downcast_ref::<IdentityFailureV1>(),
        Some(&IdentityFailureV1::PermissionDenied)
    );
    let grant = ResourceGrantV1 {
        version: 1,
        tenant_id: "tenant-a".into(),
        organization_scope: "organization-a".into(),
        resource: ResourceRefV1 {
            kind: ResourceKindV1::Session,
            resource_id: "invented-session".into(),
        },
        owner: principal(),
        revision: 1,
        active: true,
        shares: vec![],
    };
    assert!(
        store
            .mutate_identity(&mutation(IdentityChangeV1::ResourceGrant {
                expected_revision: 0,
                grant
            }))
            .is_err()
    );
    assert_eq!(store.head().unwrap(), head);
    let mut viewer = mutation(IdentityChangeV1::Account {
        expected_revision: 0,
        account: account(),
    });
    viewer.actor = PrincipalRefV1::LegacyControl {
        credential_id: "viewer-fixture".into(),
        role: LegacyControlRoleV1::Viewer,
    };
    assert!(store.mutate_identity(&viewer).is_err());
    assert!(
        store
            .append(Event::new("control.identity.mutation"))
            .is_err()
    );
    assert!(store.integrity().unwrap().valid);
}

// A tenant-local historical identifier is not proof that an organization owns
// the resource. ManageGrants cannot establish this first binding for a human.
#[test]
fn human_cannot_claim_known_legacy_resource_without_operator_binding() {
    let files = support::private_tempdir();
    let path = files.path().join("audit.sqlite");
    let store = v4(&path);
    store
        .mutate_identity(&mutation(IdentityChangeV1::Account {
            expected_revision: 0,
            account: account(),
        }))
        .unwrap();
    let mut admin = membership();
    admin.capabilities = BTreeSet::from([
        HumanCapabilityV1::ManageGrants,
        HumanCapabilityV1::ReadContent,
    ]);
    store
        .mutate_identity(&mutation(IdentityChangeV1::Membership {
            expected_revision: 0,
            membership: admin.clone(),
        }))
        .unwrap();
    let resource = ResourceRefV1 {
        kind: ResourceKindV1::Session,
        resource_id: "known-legacy-session-without-organization-binding".into(),
    };
    let mut legacy = Event::new("legacy.session.observed");
    legacy.session_id = Some(resource.resource_id.clone());
    store.append(legacy).unwrap();
    let scope = store
        .authorize_human_context(
            &principal(),
            &admin.membership_id,
            1,
            ContextModeV1::Personal,
        )
        .unwrap();
    assert!(store.identity_resource_grant(&resource).unwrap().is_none());
    assert!(
        store
            .authorized_resource_grant(&scope, &resource, ResourcePermissionV1::Content)
            .unwrap()
            .is_none()
    );
    let grant = ResourceGrantV1 {
        version: 1,
        tenant_id: "tenant-a".into(),
        organization_scope: "organization-a".into(),
        resource: resource.clone(),
        owner: principal(),
        revision: 1,
        active: true,
        shares: vec![],
    };
    let mut request = mutation(IdentityChangeV1::ResourceGrant {
        expected_revision: 0,
        grant,
    });
    request.actor = principal();
    let head = store.head().unwrap();
    let authorization_revision = store.identity_authorization_revision().unwrap();
    let failure = store
        .mutate_identity(&request)
        .expect_err("a human cannot bind an unassigned legacy resource to their organization");
    assert_eq!(
        failure.downcast_ref::<IdentityFailureV1>(),
        Some(&IdentityFailureV1::PermissionDenied)
    );
    let unknown_resource = ResourceRefV1 {
        kind: ResourceKindV1::Session,
        resource_id: "unknown-session-without-organization-binding".into(),
    };
    let mut unknown_request = request.clone();
    unknown_request.operation_id = Uuid::new_v4().to_string();
    if let IdentityChangeV1::ResourceGrant { grant, .. } = &mut unknown_request.change {
        grant.resource = unknown_resource.clone();
    }
    let unknown_failure = store
        .mutate_identity(&unknown_request)
        .expect_err("an absent binding must not reveal whether a legacy event exists");
    assert_eq!(
        unknown_failure.downcast_ref::<IdentityFailureV1>(),
        Some(&IdentityFailureV1::PermissionDenied)
    );
    assert!(
        store
            .identity_resource_grant(&unknown_resource)
            .unwrap()
            .is_none()
    );
    assert_eq!(store.head().unwrap(), head);
    assert_eq!(
        store.identity_authorization_revision().unwrap(),
        authorization_revision
    );
    assert!(store.identity_resource_grant(&resource).unwrap().is_none());
    assert!(
        store
            .authorized_resource_grant(&scope, &resource, ResourcePermissionV1::Content)
            .unwrap()
            .is_none()
    );
    assert!(store.integrity().unwrap().valid);
    drop(store);
    let reopened = Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    assert_eq!(reopened.head().unwrap(), head);
    assert_eq!(
        reopened.identity_authorization_revision().unwrap(),
        authorization_revision
    );
    assert!(
        reopened
            .identity_resource_grant(&resource)
            .unwrap()
            .is_none()
    );
    assert!(reopened.integrity().unwrap().valid);
}

// The first operator binding remains present after deactivation. It cannot be
// rebound to another organization, while its own admin can reactivate by CAS.
#[test]
fn operator_resource_binding_retains_organization_across_tombstone() {
    let files = support::private_tempdir();
    let path = files.path().join("audit.sqlite");
    let store = v4(&path);
    let admin_a = principal();
    let admin_b = PrincipalRefV1::HumanLocal {
        installation_id: "909d9572-f1a2-4a53-ad8b-a741030a2b18".into(),
        account_id: "f7e62c9f-54bb-4e99-99b9-99902da33cd6".into(),
    };
    let mut account_b = account();
    account_b.principal = admin_b.clone();
    account_b.display_name = "Organization B administrator".into();
    for user in [account(), account_b] {
        store
            .mutate_identity(&mutation(IdentityChangeV1::Account {
                expected_revision: 0,
                account: user,
            }))
            .unwrap();
    }
    let mut member_a = membership();
    member_a.capabilities = BTreeSet::from([
        HumanCapabilityV1::ManageGrants,
        HumanCapabilityV1::ReadContent,
    ]);
    let mut member_b = member_a.clone();
    member_b.principal = admin_b.clone();
    member_b.membership_id = "95712539-2c42-4b0c-8c2d-5163692700b9".into();
    member_b.organization_scope = "organization-b".into();
    for member in [member_a.clone(), member_b] {
        store
            .mutate_identity(&mutation(IdentityChangeV1::Membership {
                expected_revision: 0,
                membership: member,
            }))
            .unwrap();
    }
    let resource = ResourceRefV1 {
        kind: ResourceKindV1::Session,
        resource_id: "legacy-session-bound-by-control-operator".into(),
    };
    let mut legacy = Event::new("legacy.session.observed");
    legacy.session_id = Some(resource.resource_id.clone());
    store.append(legacy).unwrap();
    let mut grant = ResourceGrantV1 {
        version: 1,
        tenant_id: "tenant-a".into(),
        organization_scope: "organization-a".into(),
        resource: resource.clone(),
        owner: admin_a.clone(),
        revision: 1,
        active: true,
        shares: vec![],
    };
    let bind = mutation(IdentityChangeV1::ResourceGrant {
        expected_revision: 0,
        grant: grant.clone(),
    });
    let bind_receipt = store.mutate_identity(&bind).unwrap();
    let scope = store
        .authorize_human_context(
            &admin_a,
            &member_a.membership_id,
            1,
            ContextModeV1::Personal,
        )
        .unwrap();
    assert!(
        store
            .authorized_resource_grant(&scope, &resource, ResourcePermissionV1::Content)
            .unwrap()
            .is_some()
    );
    grant.active = false;
    grant.revision = 2;
    let mut deactivate = mutation(IdentityChangeV1::ResourceGrant {
        expected_revision: 1,
        grant: grant.clone(),
    });
    deactivate.actor = admin_a.clone();
    let deactivate_receipt = store.mutate_identity(&deactivate).unwrap();
    assert_eq!(
        store.identity_resource_grant(&resource).unwrap(),
        Some(grant.clone())
    );
    assert!(
        store
            .authorized_resource_grant(&scope, &resource, ResourcePermissionV1::Content)
            .is_err(),
        "deactivation must invalidate the earlier context"
    );
    let fresh_scope = store
        .authorize_human_context(
            &admin_a,
            &member_a.membership_id,
            1,
            ContextModeV1::Personal,
        )
        .unwrap();
    assert!(
        store
            .authorized_resource_grant(&fresh_scope, &resource, ResourcePermissionV1::Content)
            .unwrap()
            .is_none()
    );
    let mut foreign = grant.clone();
    foreign.organization_scope = "organization-b".into();
    foreign.owner = admin_b.clone();
    foreign.active = true;
    foreign.revision = 3;
    let mut reassign = mutation(IdentityChangeV1::ResourceGrant {
        expected_revision: 2,
        grant: foreign,
    });
    reassign.actor = admin_b;
    let head = store.head().unwrap();
    let authorization_revision = store.identity_authorization_revision().unwrap();
    assert!(store.mutate_identity(&reassign).is_err());
    assert_eq!(store.head().unwrap(), head);
    assert_eq!(
        store.identity_authorization_revision().unwrap(),
        authorization_revision
    );
    assert_eq!(
        store.identity_resource_grant(&resource).unwrap(),
        Some(grant.clone())
    );
    grant.active = true;
    grant.revision = 3;
    let mut reactivate = mutation(IdentityChangeV1::ResourceGrant {
        expected_revision: 2,
        grant: grant.clone(),
    });
    reactivate.actor = admin_a.clone();
    let reactivate_receipt = store.mutate_identity(&reactivate).unwrap();
    let head = store.head().unwrap();
    assert_eq!(store.mutate_identity(&bind).unwrap(), bind_receipt);
    assert_eq!(
        store.mutate_identity(&deactivate).unwrap(),
        deactivate_receipt
    );
    assert_eq!(
        store.mutate_identity(&reactivate).unwrap(),
        reactivate_receipt
    );
    assert_eq!(store.head().unwrap(), head);
    assert_eq!(
        store.identity_resource_grant(&resource).unwrap(),
        Some(grant.clone())
    );
    let fresh_scope = store
        .authorize_human_context(
            &admin_a,
            &member_a.membership_id,
            1,
            ContextModeV1::Personal,
        )
        .unwrap();
    assert!(
        store
            .authorized_resource_grant(&fresh_scope, &resource, ResourcePermissionV1::Content)
            .unwrap()
            .is_some()
    );
    assert!(store.integrity().unwrap().valid);
    drop(store);
    let reopened = Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    assert_eq!(reopened.mutate_identity(&bind).unwrap(), bind_receipt);
    assert_eq!(
        reopened.mutate_identity(&reactivate).unwrap(),
        reactivate_receipt
    );
    assert_eq!(reopened.head().unwrap(), head);
    assert_eq!(
        reopened.identity_resource_grant(&resource).unwrap(),
        Some(grant)
    );
    assert!(reopened.integrity().unwrap().valid);
}

// Credential bindings must survive normal redaction without admitting aliases
// of the same digest or losing their explicit durable revocation.
#[test]
fn local_credential_digest_binding_is_unique_and_revocation_replays() {
    let files = support::private_tempdir();
    let path = files.path().join("audit.sqlite");
    let store = v4(&path);
    store
        .mutate_identity(&mutation(IdentityChangeV1::Account {
            expected_revision: 0,
            account: account(),
        }))
        .unwrap();
    let mut binding = LocalCredentialV1 {
        version: 1,
        credential_id: Uuid::new_v4().to_string(),
        principal: principal(),
        sha256_digest: "d".repeat(64),
        revision: 1,
        active: true,
        expires_at: Some("2026-11-02T12:00:00Z".into()),
    };
    let create = mutation(IdentityChangeV1::Credential {
        expected_revision: 0,
        binding: binding.clone(),
    });
    let receipt = store.mutate_identity(&create).unwrap();
    assert_eq!(store.mutate_identity(&create).unwrap(), receipt);
    let mut duplicate = binding.clone();
    duplicate.credential_id = Uuid::new_v4().to_string();
    let head = store.head().unwrap();
    assert_eq!(
        store
            .mutate_identity(&mutation(IdentityChangeV1::Credential {
                expected_revision: 0,
                binding: duplicate
            }))
            .unwrap_err()
            .downcast_ref::<IdentityFailureV1>(),
        Some(&IdentityFailureV1::Conflict)
    );
    assert_eq!(store.head().unwrap(), head);
    binding.active = false;
    binding.revision = 2;
    store
        .mutate_identity(&mutation(IdentityChangeV1::Credential {
            expected_revision: 1,
            binding: binding.clone(),
        }))
        .unwrap();
    assert!(store.integrity().unwrap().valid);
    drop(store);
    let reopened = Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    assert!(reopened.integrity().unwrap().valid);
    let db = rusqlite::Connection::open(&path).unwrap();
    let payload: String = db
        .query_row(
            "SELECT payload_json FROM identity_credentials WHERE credential_id=?1",
            [binding.credential_id.clone()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<LocalCredentialV1>(&payload).unwrap(),
        binding
    );
}

// DDL and audit share one transaction; a full audit leaves the exact3 schema intact.
#[test]
fn migration_audit_failure_rolls_back_all_identity_ddl_and_preserves_backup() {
    let files = support::private_tempdir();
    let path = files.path().join("audit.sqlite");
    let backup = files.path().join("before-v4.sqlite");
    let store = fresh(&path, 2);
    let before = store.head().unwrap();
    assert!(store.migrate_identity_v4(&backup).is_err());
    assert_eq!(version(&path), 3);
    assert_eq!(store.head().unwrap(), before);
    assert!(store.supervision_state().unwrap().active);
    let db = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        db.query_row::<i64, _, _>(
            "SELECT count(*) FROM sqlite_schema WHERE name GLOB 'identity_*'",
            [],
            |r| r.get(0)
        )
        .unwrap(),
        0
    );
    drop(db);
    let backup_bytes = std::fs::read(&backup).unwrap();
    assert!(store.migrate_identity_v4(&backup).is_err());
    assert_eq!(std::fs::read(&backup).unwrap(), backup_bytes);
    assert!(store.integrity().unwrap().valid);
    assert!(
        Store::open_for_tenant(&backup, "tenant-a", Redactor::new(vec![]), 1000)
            .unwrap()
            .integrity()
            .unwrap()
            .valid
    );
}

// Version numbers cannot excuse a partial schema, unknown objects, or forged projections.
#[test]
fn exact_schema_and_identity_projection_tampering_are_refused_without_repair() {
    for tampering in [
        "PRAGMA user_version=5",
        "DROP INDEX identity_memberships_principal",
        "CREATE TABLE identity_extra(value TEXT)",
    ] {
        let files = support::private_tempdir();
        let path = files.path().join("audit.sqlite");
        drop(v4(&path));
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch(tampering).unwrap();
        drop(db);
        let before = std::fs::read(&path).unwrap();
        assert!(Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
    let files = support::private_tempdir();
    let path = files.path().join("audit.sqlite");
    let store = v4(&path);
    store
        .mutate_identity(&mutation(IdentityChangeV1::Account {
            expected_revision: 0,
            account: account(),
        }))
        .unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    let mut altered = account();
    altered.display_name = "Tampered without audit".into();
    db.execute(
        "UPDATE identity_accounts SET payload_json=?1",
        [serde_json::to_string(&altered).unwrap()],
    )
    .unwrap();
    drop(db);
    assert!(store.human_account(&principal()).is_err());
    assert!(!store.healthy());
    drop(store);
    let reopened = Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    assert!(!reopened.healthy());
    assert!(reopened.human_account(&principal()).is_err());
}

fn worker_change(worker: &WorkerV1, transition: WorkerTransitionV1) -> WorkerMutationV1 {
    WorkerMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        expected_revision: worker.revision,
        transition,
    }
}

// New identity storage cannot rewrite legacy worker/claim facts or reissue an EmitOnce.
#[test]
fn migration_preserves_consumed_legacy_start_claim_and_inactive_supervision() {
    let files = support::private_tempdir();
    let path = files.path().join("audit.sqlite");
    let store = Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    let mut admission: AdmissionV1 = serde_json::from_str(include_str!(
        "fixtures/control/admission-fixed-sandbox-v1.json"
    ))
    .unwrap();
    let policy = Policy::default();
    let decision = store
        .decision(
            &admission.decision_request,
            Some((
                policy::evaluate(&policy, &admission.decision_request),
                json!(null),
            )),
        )
        .unwrap()
        .unwrap();
    admission.receipt.audit_seq = decision.seq;
    admission.receipt.audit_hash = decision.hash;
    let mut worker = store
        .admit_worker(
            &admission,
            &policy,
            policy::evaluate(&policy, &admission.decision_request),
            json!(null),
            &QuotaPolicyV1::default(),
        )
        .unwrap()
        .worker
        .unwrap();
    for transition in [
        WorkerTransitionV1::Prepare,
        WorkerTransitionV1::CreateIssued,
    ] {
        worker = store
            .mutate_worker(&worker_change(&worker, transition))
            .unwrap()
            .worker;
    }
    let inspection = RuntimeInspectionV1 {
        authority_id: worker.launch.authority_id.clone(),
        runtime_kind: worker.launch.runtime_kind,
        container_id: "c".repeat(64),
        stable_name: worker.attempt.stable_name.clone(),
        volume_name: worker.attempt.volume_name.clone(),
        image_id: worker.launch.image_id.clone(),
        spec_hash: worker.spec_hash.clone(),
        cpu_millis: worker.launch.cpu_millis,
        memory_bytes: worker.launch.memory_bytes,
        pids_limit: worker.launch.pids_limit,
        network: worker.launch.network,
        security_preset: worker.launch.security_preset,
        restart_policy: worker.launch.restart_policy,
    };
    worker = store
        .mutate_worker(&worker_change(
            &worker,
            WorkerTransitionV1::Created { inspection },
        ))
        .unwrap()
        .worker;
    let claim = worker_change(
        &worker,
        WorkerTransitionV1::StartClaim {
            emitter_id: Uuid::new_v4().to_string(),
        },
    );
    let claimed = store.mutate_worker(&claim).unwrap();
    assert_eq!(claimed.disposition, Some(ClaimDispositionV1::EmitOnce));
    store
        .migrate_control_v3(&files.path().join("before-v3.sqlite"))
        .unwrap();
    assert!(!store.supervision_state().unwrap().active);
    let before = serde_json::to_value(store.worker(&worker.worker_id).unwrap()).unwrap();
    store
        .migrate_identity_v4(&files.path().join("before-v4.sqlite"))
        .unwrap();
    assert!(!store.supervision_state().unwrap().active);
    assert_eq!(
        serde_json::to_value(store.worker(&worker.worker_id).unwrap()).unwrap(),
        before
    );
    assert_eq!(
        store.mutate_worker(&claim).unwrap().disposition,
        Some(ClaimDispositionV1::AlreadyConsumed)
    );
    assert!(
        store
            .mutate_worker(&worker_change(
                &claimed.worker,
                WorkerTransitionV1::StartClaim {
                    emitter_id: Uuid::new_v4().to_string()
                }
            ))
            .is_err()
    );
    assert!(store.integrity().unwrap().valid);
    drop(store);
    let reopened = Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    assert_eq!(
        reopened.mutate_worker(&claim).unwrap().disposition,
        Some(ClaimDispositionV1::AlreadyConsumed)
    );
    assert_eq!(
        serde_json::to_value(reopened.worker(&worker.worker_id).unwrap()).unwrap(),
        before
    );
    assert!(reopened.integrity().unwrap().valid);
}

// The operator CLI must select4 explicitly and fail atomically on a second migration.
#[test]
fn cli_target_four_is_explicit_and_never_overwrites_a_backup() {
    let files = support::private_tempdir();
    let path = files.path().join("audit.sqlite");
    let backup = files.path().join("before-v4.sqlite");
    drop(fresh(&path, 1000));
    let run = || {
        std::process::Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
            .arg("migrate-control")
            .arg("--data")
            .arg(&path)
            .arg("--tenant-id")
            .arg("tenant-a")
            .arg("--backup")
            .arg(&backup)
            .arg("--target-version")
            .arg("4")
            .arg("--confirm-control-stopped")
            .output()
            .unwrap()
    };
    let first = run();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(version(&path), 4);
    let bytes = std::fs::read(&backup).unwrap();
    assert!(!run().status.success());
    assert_eq!(std::fs::read(&backup).unwrap(), bytes);
    assert!(
        Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000)
            .unwrap()
            .integrity()
            .unwrap()
            .valid
    );
}

// Schema4 advertises the same checked broker capability, without changing a
// viewer into a human account or a runner into an unscoped human identity.
#[tokio::test]
async fn schema_four_protocol_preserves_legacy_roles_and_rejects_unknown_capabilities() {
    use agentsandbox::api::{GatewayState, gateway_router};
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use std::sync::Arc;
    use tower::ServiceExt;
    let files = support::private_tempdir();
    let store = Arc::new(v4(&files.path().join("audit.sqlite")));
    let state = GatewayState::new(
        Arc::clone(&store),
        "r".repeat(48),
        "v".repeat(48),
        Policy::default(),
        None,
        Redactor::new(vec![]),
    );
    let router = gateway_router(state);
    for (key, role) in [("r", "runner"), ("v", "viewer")] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/control/protocol")
                    .header("authorization", format!("Bearer {}", key.repeat(48)))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        let mut protocol: ControlProtocolV1 = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(protocol.role, role);
        assert_eq!(protocol.schema_version, 4);
        assert_eq!(
            protocol.protocols,
            [SupervisionProtocolV1::BrokerSupervisionV1]
        );
        protocol.validate().unwrap();
        protocol.schema_version = 5;
        assert!(protocol.validate().is_err());
        protocol.schema_version = 4;
        protocol.protocols.clear();
        protocol.active = false;
        assert!(
            protocol.validate().is_err(),
            "inactive4 still requires the explicit broker protocol"
        );
    }
    assert!(store.human_account(&principal()).unwrap().is_none());
    let request = Request::builder()
        .uri("/api/v1/control/protocol")
        .header("authorization", "Bearer unregistered-human-shaped-value")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        router.oneshot(request).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
}
