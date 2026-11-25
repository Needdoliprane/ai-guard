//! Real migrated sources and borrowed epochs; no format-14 authority is
//! manufactured by SQL. Execution belongs to the separately admitted QA.
use super::*;
use crate::human_authority::migration::{MigratedHumanSources, tests::Fixture};

fn source_id(bundle: &MigratedHumanSources, tenant: &str) -> uuid::Uuid {
    bundle
        .owner()
        .closure()
        .sources
        .iter()
        .find(|source| source.tenant_id.as_deref() == Some(tenant))
        .unwrap()
        .source_id
}

fn read<T>(
    bundle: &MigratedHumanSources,
    tenant: &str,
    work: impl FnOnce(&Store, &HumanSourceAccess<'_>) -> T,
) -> T {
    let id = source_id(bundle, tenant);
    let token = bundle.owner().source_token(id).unwrap();
    let epoch = bundle.owner().try_read_epoch().unwrap();
    let access = HumanSourceAccess::read(&epoch, &token).unwrap();
    work(bundle.store(id).unwrap(), &access)
}

fn mutation(change: IdentityChangeV1) -> IdentityMutationV1 {
    IdentityMutationV1 {
        version: 1,
        operation_id: uuid::Uuid::new_v4().to_string(),
        actor: PrincipalRefV1::LegacyControl {
            credential_id: "synthetic-scoped-reader-control".into(),
            role: LegacyControlRoleV1::Runner,
        },
        change,
    }
}

fn mutate(bundle: &MigratedHumanSources, tenant: &str, change: IdentityChangeV1) {
    let id = source_id(bundle, tenant);
    let token = bundle.owner().source_token(id).unwrap();
    let epoch = bundle.owner().try_write_epoch().unwrap();
    bundle
        .store(id)
        .unwrap()
        .mutate_identity_scoped(&epoch, &token, &mutation(change))
        .unwrap();
}

fn scope(bundle: &MigratedHumanSources, principal: &PrincipalRefV1) -> AuthorizedScopeV1 {
    read(bundle, "A", |store, access| {
        let members = store
            .identity_memberships_scoped(access, principal)
            .unwrap();
        store
            .authorize_human_context_scoped(
                access,
                principal,
                &members[0].membership_id,
                members[0].revision,
                ContextModeV1::Personal,
            )
            .unwrap()
    })
}

fn filter() -> HumanEventFilterV1 {
    HumanEventFilterV1 {
        view: HumanReadViewV1::Metadata,
        resource: None,
        event_id: None,
        export: false,
        stream: false,
    }
}

fn head(store: &Store) -> i64 {
    audit_head(&store.db.lock().unwrap()).unwrap().0
}

fn assert_identity_failure<T>(result: anyhow::Result<T>, expected: IdentityFailureV1) {
    assert_eq!(
        result.err().unwrap().downcast_ref::<IdentityFailureV1>(),
        Some(&expected)
    );
}

#[test]
fn scoped_identity_and_authentication_reads_require_the_migrated_source() {
    let fixture = Fixture::new();
    let bundle = fixture.migrated();
    let principal = fixture.principal();
    let missing = ResourceRefV1 {
        kind: ResourceKindV1::Project,
        resource_id: "absent-project".into(),
    };
    // Fresh owner epoch per phase; the two-second bound is never reused across
    // the whole scenario. Metadata/authentication reads invent no receipt, so
    // the head captured in the first phase stays valid for the final check.
    let before = read(&bundle, "A", |store, access| {
        let before = head(store);
        assert_eq!(
            store
                .human_account_scoped(access, principal)
                .unwrap()
                .unwrap()
                .principal,
            *principal
        );
        let metadata = store
            .authentication_membership_snapshot_scoped(access, principal)
            .unwrap();
        assert_eq!(metadata.account.unwrap().principal, *principal);
        assert_eq!(metadata.memberships.len(), 1);
        assert_eq!(metadata.memberships[0].tenant_id, "A");
        assert_eq!(
            store
                .identity_authorization_revision_scoped(access)
                .unwrap(),
            metadata.authorization_revision
        );
        before
    });
    read(&bundle, "A", |store, access| {
        assert!(
            store
                .identity_resource_grant_scoped(access, &missing)
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .local_credential_binding_scoped(access, &[0xad; 32])
                .unwrap()
                .is_some()
        );
        assert_eq!(
            store
                .authenticate_local_digest_scoped(access, &[0xad; 32])
                .unwrap()
                .unwrap()
                .principal,
            *principal
        );
        assert!(
            store
                .authenticate_local_digest_scoped(access, &[0x34; 32])
                .unwrap()
                .is_none()
        );
    });
    // The unscoped refusals and head checks need no borrowed read epoch: head
    // is a direct durable read and the unscoped calls must simply refuse.
    let store = bundle.store(source_id(&bundle, "A")).unwrap();
    assert_eq!(
        head(store),
        before,
        "metadata and authentication do not invent human read receipts"
    );
    assert!(store.human_account(principal).is_err());
    assert!(store.authentication_membership_snapshot(principal).is_err());
    assert!(store.identity_memberships(principal).is_err());
    assert!(store.identity_resource_grant(&missing).is_err());
    assert!(store.identity_authorization_revision().is_err());
    assert!(store.local_credential_binding(&[0xad; 32]).is_err());
    assert!(store.authenticate_local_digest(&[0xad; 32]).is_err());
    assert_eq!(head(store), before);
}

#[test]
fn scoped_human_reads_preserve_resource_acl_redaction_and_atomic_receipts() {
    let fixture = Fixture::new();
    let principal = fixture.principal();
    let resource = ResourceRefV1 {
        kind: ResourceKindV1::Project,
        resource_id: "scoped-project".into(),
    };
    let mut event = Event::new("synthetic.scoped.observation");
    event.project_id = Some(resource.resource_id.clone());
    event.data = json!({"visible": "synthetic-content-token"});
    let event_id = fixture.legacy_store("A").append(event).unwrap().event.id;
    let bundle = fixture.migrated();
    mutate(
        &bundle,
        "A",
        IdentityChangeV1::ResourceGrant {
            expected_revision: 0,
            grant: ResourceGrantV1 {
                version: 1,
                tenant_id: "A".into(),
                organization_scope: "synthetic-org".into(),
                resource: resource.clone(),
                owner: principal.clone(),
                revision: 1,
                active: true,
                shares: vec![],
            },
        },
    );
    let current = scope(&bundle, principal);
    let transient = Redactor::new(vec!["synthetic-content-token".into()]);
    // Fresh owner epoch per read; receipts are durable so the head captured in
    // the first phase keeps incrementing across phases (before+1..before+4).
    let before = read(&bundle, "A", |store, access| {
        assert!(
            store
                .authorized_resource_grant_scoped(
                    access,
                    &current,
                    &resource,
                    ResourcePermissionV1::Metadata
                )
                .unwrap()
                .is_some()
        );
        let before = head(store);
        let profile = store
            .human_profile_scoped(access, principal, &transient)
            .unwrap();
        assert_eq!(profile.account.unwrap().principal, *principal);
        assert_eq!(profile.memberships.len(), 1);
        assert_eq!(head(store), before + 1);
        before
    });
    read(&bundle, "A", |store, access| {
        let resources = store
            .human_resources_scoped(access, &current, "", 1, &transient)
            .unwrap();
        assert_eq!(resources.items.len(), 1);
        assert_eq!(resources.items[0].resource, resource);
        assert_eq!(head(store), before + 2);
    });
    let mut content = filter();
    content.view = HumanReadViewV1::Content;
    read(&bundle, "A", |store, access| {
        let events = store
            .human_events_scoped(access, &current, &content, 0, None, 1, &transient)
            .unwrap()
            .page
            .unwrap();
        assert_eq!(events.items.len(), 1);
        assert_eq!(events.items[0].metadata.id, event_id);
        assert!(
            !serde_json::to_string(&events.items[0])
                .unwrap()
                .contains("synthetic-content-token")
        );
        assert_eq!(head(store), before + 3);
    });
    read(&bundle, "A", |store, access| {
        let recent = store
            .human_events_recent_scoped(access, &current, &content, None, 1, &transient)
            .unwrap();
        assert_eq!(recent.items[0].metadata.id, event_id);
        assert_eq!(head(store), before + 4);
    });
    read(&bundle, "A", |store, access| {
        assert!(store.human_profile(principal, &transient).is_err());
        assert!(
            store
                .authorize_human_context(
                    principal,
                    current.membership_id(),
                    current.membership_revision(),
                    current.mode()
                )
                .is_err()
        );
        assert!(
            store
                .authorized_resource_grant(&current, &resource, ResourcePermissionV1::Metadata)
                .is_err()
        );
        assert!(store.human_resources(&current, "", 1, &transient).is_err());
        assert!(
            store
                .human_events(&current, &content, 0, None, 1, &transient)
                .is_err()
        );
        assert!(
            store
                .human_events_recent(&current, &content, None, 1, &transient)
                .is_err()
        );
        let mut forbidden = filter();
        forbidden.export = true;
        assert_identity_failure(
            store.human_events_scoped(access, &current, &forbidden, 0, None, 1, &transient),
            IdentityFailureV1::PermissionDenied,
        );
        let mut absent = filter();
        absent.resource = Some(ResourceRefV1 {
            kind: ResourceKindV1::Project,
            resource_id: "absent".into(),
        });
        assert_identity_failure(
            store.human_events_recent_scoped(access, &current, &absent, None, 1, &transient),
            IdentityFailureV1::NotFound,
        );
        assert_eq!(head(store), before + 4, "refused reads append no receipt");
    });
}

#[test]
fn scoped_reads_refuse_other_source_tenant_and_drain_accepted_epoch_on_close() {
    let fixture = Fixture::new();
    let bundle = fixture.migrated();
    let current = scope(&bundle, fixture.principal());
    let a = bundle.store(source_id(&bundle, "A")).unwrap();
    read(&bundle, "B", |b, access| {
        let before = head(a);
        assert!(a.human_account_scoped(access, fixture.principal()).is_err());
        assert!(
            a.human_profile_scoped(access, fixture.principal(), &Redactor::new(vec![]))
                .is_err()
        );
        assert_identity_failure(
            b.human_resources_scoped(access, &current, "", 1, &Redactor::new(vec![])),
            IdentityFailureV1::ContextRevoked,
        );
        assert_eq!(head(a), before);
    });
    let token = bundle
        .owner()
        .source_token(source_id(&bundle, "A"))
        .unwrap();
    let epoch = bundle.owner().try_read_epoch().unwrap();
    let access = HumanSourceAccess::read(&epoch, &token).unwrap();
    let before = head(a);
    bundle.owner().begin_close();
    assert!(matches!(
        bundle.owner().try_read_epoch(),
        Err(crate::human_authority::HumanAuthorityFailure::Closing)
    ));
    assert!(matches!(
        bundle.owner().try_write_epoch(),
        Err(crate::human_authority::HumanAuthorityFailure::Closing)
    ));
    // Normal close stops admissions; accepted epochs drain within their
    // unchanged lease budget and still retain authority through commit.
    epoch.check().unwrap();
    assert_eq!(
        a.identity_authorization_revision_scoped(&access).unwrap(),
        current.authorization_revision()
    );
    let events = a
        .human_events_scoped(
            &access,
            &current,
            &filter(),
            0,
            None,
            1,
            &Redactor::new(vec![]),
        )
        .unwrap()
        .page
        .unwrap();
    assert!(events.items.is_empty());
    assert!(!events.receipt.audit_event_id.is_empty());
    assert_eq!(head(a), before + 1);
}

#[test]
fn scoped_human_reads_revalidate_revision_permissions_and_credential_revocation() {
    let fixture = Fixture::new();
    let bundle = fixture.migrated();
    let principal = fixture.principal();
    let old_scope = scope(&bundle, principal);
    let (mut membership, mut credential) = read(&bundle, "A", |store, access| {
        (
            store
                .identity_memberships_scoped(access, principal)
                .unwrap()
                .remove(0),
            store
                .local_credential_binding_scoped(access, &[0xad; 32])
                .unwrap()
                .unwrap(),
        )
    });
    membership.revision += 1;
    membership.capabilities.clear();
    mutate(
        &bundle,
        "A",
        IdentityChangeV1::Membership {
            expected_revision: membership.revision - 1,
            membership: membership.clone(),
        },
    );
    read(&bundle, "A", |store, access| {
        let before = head(store);
        assert_identity_failure(
            store.human_resources_scoped(access, &old_scope, "", 1, &Redactor::new(vec![])),
            IdentityFailureV1::ContextRevoked,
        );
        assert_identity_failure(
            store.human_events_recent_scoped(
                access,
                &old_scope,
                &filter(),
                None,
                1,
                &Redactor::new(vec![]),
            ),
            IdentityFailureV1::ContextRevoked,
        );
        let current = store
            .authorize_human_context_scoped(
                access,
                principal,
                &membership.membership_id,
                membership.revision,
                ContextModeV1::Personal,
            )
            .unwrap();
        assert_identity_failure(
            store.human_events_scoped(
                access,
                &current,
                &filter(),
                0,
                None,
                1,
                &Redactor::new(vec![]),
            ),
            IdentityFailureV1::PermissionDenied,
        );
        assert_eq!(head(store), before);
    });
    credential.revision += 1;
    credential.active = false;
    mutate(
        &bundle,
        "A",
        IdentityChangeV1::Credential {
            expected_revision: credential.revision - 1,
            binding: credential,
        },
    );
    read(&bundle, "A", |store, access| {
        assert!(
            !store
                .local_credential_binding_scoped(access, &[0xad; 32])
                .unwrap()
                .unwrap()
                .active
        );
        assert_identity_failure(
            store.authenticate_local_digest_scoped(access, &[0xad; 32]),
            IdentityFailureV1::IdentityRevoked,
        );
    });
}

#[test]
fn scoped_reads_refuse_expired_epoch_without_a_read_receipt() {
    let fixture = Fixture::new();
    let bundle = fixture.migrated();
    let id = source_id(&bundle, "A");
    let store = bundle.store(id).unwrap();
    let token = bundle.owner().source_token(id).unwrap();
    let epoch = bundle.owner().try_write_epoch().unwrap();
    let access = HumanSourceAccess::write(&epoch, &token).unwrap();
    let before = head(store);
    std::thread::sleep(
        epoch
            .test_deadline()
            .saturating_duration_since(std::time::Instant::now()),
    );
    assert!(epoch.check().is_err(), "the actual epoch has expired");
    assert!(
        store
            .authentication_membership_snapshot_scoped(&access, fixture.principal())
            .is_err()
    );
    assert!(
        store
            .human_profile_scoped(&access, fixture.principal(), &Redactor::new(vec![]))
            .is_err()
    );
    assert_eq!(head(store), before);
}
