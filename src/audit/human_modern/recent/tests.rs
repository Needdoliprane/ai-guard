//! Actual modern recent entrance after the verified Router baseline RED.
//! These GREEN candidates remain NOT_RUN until Root admits the composed QA.
use super::*;
use crate::human_authority::migration::{MigratedHumanSources, tests::Fixture};

fn read<T>(
    bundle: &MigratedHumanSources,
    work: impl FnOnce(&Store, &HumanSourceAccess<'_>) -> T,
) -> T {
    let source = bundle
        .owner()
        .closure()
        .sources
        .iter()
        .find(|source| source.tenant_id.as_deref() == Some("B"))
        .unwrap();
    let token = bundle.owner().source_token(source.source_id).unwrap();
    let epoch = bundle.owner().try_read_epoch().unwrap();
    let access = HumanSourceAccess::read(&epoch, &token).unwrap();
    work(bundle.store(source.source_id).unwrap(), &access)
}

fn scoped<T>(
    fixture: &Fixture,
    bundle: &MigratedHumanSources,
    work: impl FnOnce(&Store, &HumanSourceAccess<'_>, &AuthorizedScopeV1) -> T,
) -> T {
    read(bundle, |store, access| {
        let profile = store
            .modern_human_profile(access, fixture.principal(), &Redactor::new(vec![]))
            .unwrap();
        let membership = profile
            .memberships
            .iter()
            .find(|membership| membership.tenant_id == "B")
            .unwrap();
        let scope = store
            .modern_authorize_human_context(
                access,
                fixture.principal(),
                &membership.membership_id,
                membership.revision,
                ContextModeV1::Personal,
            )
            .unwrap();
        work(store, access, &scope)
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

fn snapshot(store: &Store) -> (i64, String, i64, i64) {
    store.db.lock().unwrap().query_row(
        "SELECT COALESCE(MAX(seq),0),COALESCE((SELECT hash FROM events ORDER BY seq DESC LIMIT 1),''),COUNT(*),COALESCE(SUM(length(CAST(payload AS BLOB))),0) FROM events",
        [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    ).unwrap()
}

#[test]
fn modern_recent_own_sqlite_busy_rolls_back_and_restores_713_then_recovers() {
    let fixture = Fixture::new();
    let bundle = fixture.migrated();
    scoped(&fixture, &bundle, |store, access, scope| {
        store
            .db
            .lock()
            .unwrap()
            .busy_timeout(std::time::Duration::from_millis(713))
            .unwrap();
        let writer = rusqlite::Connection::open(store.backing.path()).unwrap();
        writer.execute_batch("BEGIN IMMEDIATE").unwrap();
        let error = store
            .modern_human_events_recent(access, scope, &filter(), None, 1, &Redactor::new(vec![]))
            .err()
            .unwrap();
        assert!(error.chain().any(|cause| matches!(
            cause.downcast_ref::<rusqlite::Error>(),
            Some(rusqlite::Error::SqliteFailure(code, _))
                if matches!(code.code, rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
        )), "{error:#}");
        let timeout: i64 = store
            .db
            .lock()
            .unwrap()
            .pragma_query_value(None, "busy_timeout", |row| row.get(0))
            .unwrap();
        assert_eq!(
            error.downcast_ref::<review_access::ReviewAccessFailure>(),
            Some(&review_access::ReviewAccessFailure::Busy)
        );
        assert_eq!(timeout, 713);
        assert!(store.healthy());
        assert!(!writer.is_autocommit());
        writer.execute_batch("ROLLBACK").unwrap();
        assert!(
            store
                .modern_human_events_recent(
                    access,
                    scope,
                    &filter(),
                    None,
                    1,
                    &Redactor::new(vec![])
                )
                .is_ok()
        );
        let restored: i64 = store
            .db
            .lock()
            .unwrap()
            .pragma_query_value(None, "busy_timeout", |row| row.get(0))
            .unwrap();
        assert_eq!(restored, 713);
    });
}

#[test]
fn modern_recent_own_mutex_refuses_without_waiting_or_changing_713() {
    let fixture = Fixture::new();
    let bundle = fixture.migrated();
    scoped(&fixture, &bundle, |store, access, scope| {
        let held = store.db.lock().unwrap();
        held.busy_timeout(std::time::Duration::from_millis(713))
            .unwrap();
        let error = store
            .modern_human_events_recent(access, scope, &filter(), None, 1, &Redactor::new(vec![]))
            .err()
            .unwrap();
        assert_eq!(
            error.downcast_ref::<review_access::ReviewAccessFailure>(),
            Some(&review_access::ReviewAccessFailure::Busy)
        );
        let unchanged: i64 = held
            .pragma_query_value(None, "busy_timeout", |row| row.get(0))
            .unwrap();
        assert_eq!(unchanged, 713);
        drop(held);
        assert!(
            store
                .modern_human_events_recent(
                    access,
                    scope,
                    &filter(),
                    None,
                    1,
                    &Redactor::new(vec![])
                )
                .is_ok()
        );
    });
}

#[test]
fn modern_recent_real_audit_full_rolls_back_restores_713_and_latches_unavailable() {
    let fixture = Fixture::new();
    let bundle = fixture.migrated();
    // Resolve and audit the profile once, before bounding SQLite allocation.
    let scope = scoped(&fixture, &bundle, |_, _, scope| (*scope).clone());
    let source_id = bundle
        .owner()
        .closure()
        .sources
        .iter()
        .find(|source| source.tenant_id.as_deref() == Some("B"))
        .unwrap()
        .source_id;
    let store = bundle.store(source_id).unwrap();
    // This positive control uses a fresh epoch and the real checked route.
    read(&bundle, |store, access| {
        let page = store.modern_human_events_recent(
            access, &scope, &filter(), None, 1, &Redactor::new(vec![]),
        ).unwrap();
        let receipt_exists: bool = store.db.lock().unwrap().query_row(
            "SELECT EXISTS(SELECT 1 FROM events WHERE id=?1 AND kind='control.identity.access' AND json_extract(payload,'$.data.action')='events')",
            [&page.receipt.audit_event_id], |row| row.get(0),
        ).unwrap();
        assert!(receipt_exists);
    });
    store.db.lock().unwrap()
        .busy_timeout(std::time::Duration::from_millis(713))
        .unwrap();
    assert!(store.permanently_healthy());
    assert!(store.healthy());
    let quota = || {
        store
            .db
            .lock()
            .unwrap()
            .query_row("SELECT bytes FROM quota WHERE id=1", [], |row| row.get::<_, i64>(0))
            .unwrap()
    };
    let page_limit = store.modern_test_limit_audit_pages().unwrap();
    let mut failed_state = None;
    for _ in 0..128 {
        let before = snapshot(store);
        let before_quota = quota();
        let usage = store.modern_test_human_usage();
        let result = read(&bundle, |store, access| {
            store.modern_human_events_recent(
                access, &scope, &filter(), None, 1, &Redactor::new(vec![]),
            )
        });
        assert_eq!(
            store.db.lock().unwrap()
                .pragma_query_value(None, "busy_timeout", |row| row.get::<_, i64>(0))
                .unwrap(),
            713
        );
        match result {
            Ok(page) => {
                assert!(page.items.iter().all(|item| item.content.is_none()));
                assert_eq!(snapshot(store).0, before.0 + 1);
                let receipt_exists: bool = store.db.lock().unwrap().query_row(
                    "SELECT EXISTS(SELECT 1 FROM events WHERE seq=?1 AND id=?2 AND received_at=?3 AND kind='control.identity.access' AND json_extract(payload,'$.data.action')='events' AND json_extract(payload,'$.data.request_id')=?4 AND json_extract(payload,'$.data.result_count')=?5)",
                    params![before.0 + 1, &page.receipt.audit_event_id, &page.receipt.received_at, &page.receipt.request_id, i64::try_from(page.items.len()).unwrap()],
                    |row| row.get(0),
                ).unwrap();
                assert!(receipt_exists, "successful read must have its committed receipt");
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
                assert!(error.chain().any(|cause| matches!(
                    cause.downcast_ref::<rusqlite::Error>(),
                    Some(rusqlite::Error::SqliteFailure(code, _))
                        if code.code == rusqlite::ErrorCode::DiskFull
                            && code.extended_code == rusqlite::ffi::SQLITE_FULL
                )), "expected actual SQLite FULL, got {error:#}");
                // The durable floor may independently advance, but no event,
                // quota byte or cached human capacity may survive this failure.
                assert_eq!(snapshot(store), before);
                assert_eq!(quota(), before_quota);
                assert_eq!(store.modern_test_human_usage(), usage);
                assert!(!store.permanently_healthy());
                assert!(!store.healthy());
                failed_state = Some((before, before_quota, usage));
                break;
            }
        }
    }
    let (before, before_quota, usage) = failed_state
        .expect("bounded SQLite page cap did not reach a real allocation refusal");
    page_limit.restore().unwrap();
    assert!(!store.permanently_healthy());
    let latched = read(&bundle, |store, access| {
        store.modern_human_events_recent(
            access, &scope, &filter(), None, 1, &Redactor::new(vec![]),
        )
    }).err().unwrap();
    assert!(
        format!("{latched:#}").contains("audit integrity failure is latched; recovery required"),
        "expected existing permanent audit latch, got {latched:#}"
    );
    assert_eq!(snapshot(store), before);
    assert_eq!(quota(), before_quota);
    assert_eq!(store.modern_test_human_usage(), usage);
    assert_eq!(
        store.db.lock().unwrap()
            .pragma_query_value(None, "busy_timeout", |row| row.get::<_, i64>(0))
            .unwrap(),
        713
    );
    assert!(!store.permanently_healthy());
    assert!(!store.healthy(), "restoring page capacity does not repair the latch");
}
