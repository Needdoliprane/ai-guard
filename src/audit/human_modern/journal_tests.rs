//! Targeted real checked-Store behavior. All execution remains NOT_RUN.
use super::*;
use crate::human_authority::migration::{MigratedHumanSources, tests::Fixture};

fn change(change: IdentityChangeV1) -> IdentityMutationV1 {
    IdentityMutationV1 {
        version: 1,
        operation_id: uuid::Uuid::new_v4().to_string(),
        actor: PrincipalRefV1::LegacyControl {
            credential_id: "synthetic-journal-store-control".into(),
            role: LegacyControlRoleV1::Runner,
        },
        change,
    }
}

fn fixture() -> (Fixture, String, String) {
    let fixture = Fixture::new();
    let store = fixture.legacy_store("B");
    let mut ids = Vec::new();
    for resource in ["store-first-b", "store-second-b"] {
        let mut event = Event::new("fixture.owned-journal");
        event.session_id = Some(resource.into());
        ids.push(store.append(event).unwrap().event.id);
        store
            .mutate_identity(&change(IdentityChangeV1::ResourceGrant {
                expected_revision: 0,
                grant: ResourceGrantV1 {
                    version: 1,
                    tenant_id: "B".into(),
                    organization_scope: "synthetic-org".into(),
                    resource: ResourceRefV1 {
                        kind: ResourceKindV1::Session,
                        resource_id: resource.into(),
                    },
                    owner: fixture.principal().clone(),
                    revision: 1,
                    active: true,
                    shares: Vec::new(),
                },
            }))
            .unwrap();
    }
    (fixture, ids.remove(0), ids.remove(0))
}

fn read<T>(
    bundle: &MigratedHumanSources,
    work: impl FnOnce(&Store, &HumanSourceAccess<'_>) -> T,
) -> T {
    let descriptor = bundle
        .owner()
        .closure()
        .sources
        .iter()
        .find(|source| source.tenant_id.as_deref() == Some("B"))
        .unwrap();
    let source = bundle.owner().source_token(descriptor.source_id).unwrap();
    let epoch = bundle.owner().try_read_epoch().unwrap();
    let access = HumanSourceAccess::read(&epoch, &source).unwrap();
    work(bundle.store(descriptor.source_id).unwrap(), &access)
}

fn scope(
    store: &Store,
    access: &HumanSourceAccess<'_>,
    principal: &PrincipalRefV1,
) -> AuthorizedScopeV1 {
    let profile = store
        .modern_human_profile(access, principal, &Redactor::new(vec![]))
        .unwrap();
    let member = &profile.memberships[0];
    store
        .modern_authorize_human_context(
            access,
            principal,
            &member.membership_id,
            member.revision,
            ContextModeV1::Personal,
        )
        .unwrap()
}

fn metadata() -> HumanEventFilterV1 {
    HumanEventFilterV1 {
        view: HumanReadViewV1::Metadata,
        resource: None,
        event_id: None,
        export: false,
        stream: false,
    }
}

#[test]
fn metadata_events_rejects_non_list_filters_before_read_audit() {
    let (fixture, _, _) = fixture();
    let bundle = fixture.migrated();
    read(&bundle, |store, access| {
        let scope = scope(store, access, fixture.principal());
        let before = audit_head(&store.db.lock().unwrap()).unwrap();
        let usage = store.modern_test_human_usage();
        for filter in [
            HumanEventFilterV1 {
                view: HumanReadViewV1::Content,
                ..metadata()
            },
            HumanEventFilterV1 {
                event_id: Some(uuid::Uuid::new_v4().to_string()),
                ..metadata()
            },
            HumanEventFilterV1 {
                export: true,
                ..metadata()
            },
            HumanEventFilterV1 {
                stream: true,
                ..metadata()
            },
        ] {
            assert!(
                store
                    .modern_human_events(
                        access,
                        &scope,
                        &filter,
                        0,
                        None,
                        25,
                        &Redactor::new(vec![])
                    )
                    .is_err()
            );
            assert_eq!(audit_head(&store.db.lock().unwrap()).unwrap(), before);
            assert_eq!(store.modern_test_human_usage(), usage);
        }
        let poll = store
            .modern_human_events(
                access,
                &scope,
                &metadata(),
                0,
                None,
                25,
                &Redactor::new(vec![]),
            )
            .unwrap();
        assert_eq!(poll.page.unwrap().items.len(), 2);
        assert_eq!(
            audit_head(&store.db.lock().unwrap()).unwrap().0,
            before.0 + 1
        );
    });
}

#[test]
fn metadata_events_retains_requested_snapshot_upper_bound() {
    let (fixture, first, second) = fixture();
    let bundle = fixture.migrated();
    // Fresh owner epoch per phase; the two-second bound is never reused.
    let (scope, upper) = read(&bundle, |store, access| {
        let scope = scope(store, access, fixture.principal());
        let upper: i64 = store
            .db
            .lock()
            .unwrap()
            .query_row("SELECT seq FROM events WHERE id=?1", [&first], |row| {
                row.get(0)
            })
            .unwrap();
        (scope, upper)
    });
    read(&bundle, |store, access| {
        let poll = store
            .modern_human_events(
                access,
                &scope,
                &metadata(),
                0,
                Some(upper),
                25,
                &Redactor::new(vec![]),
            )
            .unwrap();
        let page = poll.page.unwrap();
        assert_eq!(page.upper_event, upper);
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].metadata.id, first);
    });
    read(&bundle, |store, access| {
        let poll = store
            .modern_human_events(
                access,
                &scope,
                &metadata(),
                0,
                None,
                25,
                &Redactor::new(vec![]),
            )
            .unwrap();
        let page = poll.page.unwrap();
        assert_eq!(page.items.len(), 2);
        assert_eq!(page.items[1].metadata.id, second);
    });
}

#[test]
fn metadata_events_own_busy_restores_timeout_and_preserves_health() {
    let (fixture, _, _) = fixture();
    let bundle = fixture.migrated();
    // Each phase acquires a fresh owner epoch so the production two-second
    // bound is never reused across the whole scenario; the Busy mechanism
    // (held store lock / immediate connection) and every assertion are kept.
    let scope = read(&bundle, |store, access| {
        let scope = scope(store, access, fixture.principal());
        let held = store.db.lock().unwrap();
        held.busy_timeout(std::time::Duration::from_millis(713))
            .unwrap();
        let error = store
            .modern_human_events(
                access,
                &scope,
                &metadata(),
                0,
                None,
                25,
                &Redactor::new(vec![]),
            )
            .err()
            .unwrap();
        assert_eq!(
            error.downcast_ref::<review_access::ReviewAccessFailure>(),
            Some(&review_access::ReviewAccessFailure::Busy)
        );
        assert_eq!(
            held.pragma_query_value(None, "busy_timeout", |row| row.get::<_, i64>(0))
                .unwrap(),
            713
        );
        scope
    });
    read(&bundle, |store, access| {
        let connection = rusqlite::Connection::open(store.backing.path()).unwrap();
        connection.execute_batch("BEGIN IMMEDIATE").unwrap();
        let before = audit_head(&store.db.lock().unwrap()).unwrap();
        let error = store
            .modern_human_events(
                access,
                &scope,
                &metadata(),
                0,
                None,
                25,
                &Redactor::new(vec![]),
            )
            .err()
            .unwrap();
        assert_eq!(
            error.downcast_ref::<review_access::ReviewAccessFailure>(),
            Some(&review_access::ReviewAccessFailure::Busy)
        );
        assert_eq!(
            store
                .db
                .lock()
                .unwrap()
                .pragma_query_value(None, "busy_timeout", |row| row.get::<_, i64>(0))
                .unwrap(),
            713
        );
        assert_eq!(audit_head(&store.db.lock().unwrap()).unwrap(), before);
        assert!(store.healthy());
        assert!(!connection.is_autocommit());
        connection.execute_batch("ROLLBACK").unwrap();
    });
    read(&bundle, |store, access| {
        assert_eq!(
            store
                .modern_human_events(
                    access,
                    &scope,
                    &metadata(),
                    0,
                    None,
                    25,
                    &Redactor::new(vec![])
                )
                .unwrap()
                .page
                .unwrap()
                .items
                .len(),
            2
        );
    });
}

#[test]
fn metadata_events_sqlite_audit_full_preserves_head_quota_and_usage() {
    let (fixture, _, _) = fixture();
    let bundle = fixture.migrated();
    // Profile auditing and scope resolution precede the logical page cap.
    let scope = read(&bundle, |store, access| {
        scope(store, access, fixture.principal())
    });
    let source_id = bundle
        .owner()
        .closure()
        .sources
        .iter()
        .find(|source| source.tenant_id.as_deref() == Some("B"))
        .unwrap()
        .source_id;
    let store = bundle.store(source_id).unwrap();
    store
        .db
        .lock()
        .unwrap()
        .busy_timeout(std::time::Duration::from_millis(713))
        .unwrap();
    assert!(store.permanently_healthy());
    assert!(store.healthy());
    let quota = || {
        store
            .db
            .lock()
            .unwrap()
            .query_row("SELECT bytes FROM quota WHERE id=1", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap()
    };
    let page_limit = store.modern_test_limit_audit_pages().unwrap();
    let mut failed_state = None;
    for _ in 0..128 {
        let before = audit_head(&store.db.lock().unwrap()).unwrap();
        let before_quota = quota();
        let usage = store.modern_test_human_usage();
        // A new owner epoch is acquired for every real checked read. The
        // production two-second epoch bound is neither extended nor reused.
        let result = read(&bundle, |store, access| {
            store.modern_human_events(
                access,
                &scope,
                &metadata(),
                0,
                None,
                25,
                &Redactor::new(vec![]),
            )
        });
        assert_eq!(
            store
                .db
                .lock()
                .unwrap()
                .pragma_query_value(None, "busy_timeout", |row| row.get::<_, i64>(0))
                .unwrap(),
            713
        );
        match result {
            Ok(poll) => {
                let page = poll
                    .page
                    .expect("successful metadata read must return a page");
                assert_eq!(page.items.len(), 2);
                assert!(page.items.iter().all(|item| item.content.is_none()));
                assert_eq!(
                    audit_head(&store.db.lock().unwrap()).unwrap().0,
                    before.0 + 1
                );
                let receipt_exists: bool = store.db.lock().unwrap().query_row(
                    "SELECT EXISTS(SELECT 1 FROM events WHERE seq=?1 AND id=?2 AND received_at=?3 AND kind='control.identity.access' AND json_extract(payload,'$.data.action')='events' AND json_extract(payload,'$.data.request_id')=?4 AND json_extract(payload,'$.data.result_count')=2)",
                    params![before.0 + 1, &page.receipt.audit_event_id, &page.receipt.received_at, &page.receipt.request_id],
                    |row| row.get(0),
                ).unwrap();
                assert!(
                    receipt_exists,
                    "successful read must have its committed receipt"
                );
                let after_quota = quota();
                assert!(after_quota > before_quota);
                let after_usage = store.modern_test_human_usage();
                assert_eq!(
                    after_usage["tenant"]["events"].as_u64().unwrap(),
                    usage["tenant"]["events"].as_u64().unwrap() + 1
                );
                assert_eq!(
                    after_usage["tenant"]["bytes"].as_u64().unwrap(),
                    usage["tenant"]["bytes"].as_u64().unwrap()
                        + u64::try_from(after_quota - before_quota).unwrap()
                );
                assert!(store.healthy());
            }
            Err(error) => {
                assert!(
                    error.chain().any(|cause| matches!(
                        cause.downcast_ref::<rusqlite::Error>(),
                        Some(rusqlite::Error::SqliteFailure(code, _))
                            if code.code == rusqlite::ErrorCode::DiskFull
                                && code.extended_code == rusqlite::ffi::SQLITE_FULL
                    )),
                    "expected actual SQLite FULL, got {error:#}"
                );
                assert_eq!(audit_head(&store.db.lock().unwrap()).unwrap(), before);
                assert_eq!(quota(), before_quota);
                assert_eq!(store.modern_test_human_usage(), usage);
                assert!(!store.permanently_healthy());
                assert!(!store.healthy());
                failed_state = Some((before, before_quota, usage));
                break;
            }
        }
    }
    let (before, before_quota, usage) =
        failed_state.expect("bounded SQLite page cap did not reach a real allocation refusal");
    // Restoring fixture capacity is cleanup, not operator latch recovery.
    page_limit.restore().unwrap();
    assert!(!store.permanently_healthy());
    let latched = read(&bundle, |store, access| {
        store.modern_human_events(
            access,
            &scope,
            &metadata(),
            0,
            None,
            25,
            &Redactor::new(vec![]),
        )
    })
    .err()
    .unwrap();
    assert!(
        format!("{latched:#}").contains("audit integrity failure is latched; recovery required"),
        "expected existing permanent audit latch, got {latched:#}"
    );
    assert_eq!(audit_head(&store.db.lock().unwrap()).unwrap(), before);
    assert_eq!(quota(), before_quota);
    assert_eq!(store.modern_test_human_usage(), usage);
    assert_eq!(
        store
            .db
            .lock()
            .unwrap()
            .pragma_query_value(None, "busy_timeout", |row| row.get::<_, i64>(0))
            .unwrap(),
        713
    );
    assert!(!store.permanently_healthy());
    assert!(!store.healthy());
}
