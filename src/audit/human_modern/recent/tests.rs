//! Actual modern recent entrance after the verified Router baseline RED.
//! These GREEN candidates remain NOT_RUN until Root admits the composed QA.
use super::*;
use crate::human_authority::migration::{MigratedHumanSources, tests::Fixture};

fn scoped<T>(
    fixture: &Fixture,
    bundle: &MigratedHumanSources,
    work: impl FnOnce(&Store, &HumanSourceAccess<'_>, &AuthorizedScopeV1) -> T,
) -> T {
    let source = bundle.owner().closure().sources.iter()
        .find(|source| source.tenant_id.as_deref() == Some("B"))
        .unwrap();
    let token = bundle.owner().source_token(source.source_id).unwrap();
    let epoch = bundle.owner().try_read_epoch().unwrap();
    let access = HumanSourceAccess::read(&epoch, &token).unwrap();
    let store = bundle.store(source.source_id).unwrap();
    let profile = store.modern_human_profile(&access, fixture.principal(), &Redactor::new(vec![])).unwrap();
    let membership = profile.memberships.iter().find(|membership| membership.tenant_id == "B").unwrap();
    let scope = store.modern_authorize_human_context(
        &access, fixture.principal(), &membership.membership_id,
        membership.revision, ContextModeV1::Personal,
    ).unwrap();
    work(store, &access, &scope)
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
        store.db.lock().unwrap().busy_timeout(std::time::Duration::from_millis(713)).unwrap();
        let writer = rusqlite::Connection::open(store.backing.path()).unwrap();
        writer.execute_batch("BEGIN IMMEDIATE").unwrap();
        let error = store.modern_human_events_recent(access, scope, &filter(), None, 1, &Redactor::new(vec![])).err().unwrap();
        assert!(error.chain().any(|cause| matches!(
            cause.downcast_ref::<rusqlite::Error>(),
            Some(rusqlite::Error::SqliteFailure(code, _))
                if matches!(code.code, rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
        )), "{error:#}");
        let timeout: i64 = store.db.lock().unwrap().pragma_query_value(None, "busy_timeout", |row| row.get(0)).unwrap();
        assert_eq!(error.downcast_ref::<review_access::ReviewAccessFailure>(), Some(&review_access::ReviewAccessFailure::Busy));
        assert_eq!(timeout, 713);
        assert!(store.healthy());
        assert!(!writer.is_autocommit());
        writer.execute_batch("ROLLBACK").unwrap();
        assert!(store.modern_human_events_recent(access, scope, &filter(), None, 1, &Redactor::new(vec![])).is_ok());
        let restored: i64 = store.db.lock().unwrap().pragma_query_value(None, "busy_timeout", |row| row.get(0)).unwrap();
        assert_eq!(restored, 713);
    });
}

#[test]
fn modern_recent_own_mutex_refuses_without_waiting_or_changing_713() {
    let fixture = Fixture::new();
    let bundle = fixture.migrated();
    scoped(&fixture, &bundle, |store, access, scope| {
        let held = store.db.lock().unwrap();
        held.busy_timeout(std::time::Duration::from_millis(713)).unwrap();
        let error = store.modern_human_events_recent(access, scope, &filter(), None, 1, &Redactor::new(vec![])).err().unwrap();
        assert_eq!(error.downcast_ref::<review_access::ReviewAccessFailure>(), Some(&review_access::ReviewAccessFailure::Busy));
        let unchanged: i64 = held.pragma_query_value(None, "busy_timeout", |row| row.get(0)).unwrap();
        assert_eq!(unchanged, 713);
        drop(held);
        assert!(store.modern_human_events_recent(access, scope, &filter(), None, 1, &Redactor::new(vec![])).is_ok());
    });
}

struct AbortAudit<'a> { store: &'a Store, active: bool }
impl<'a> AbortAudit<'a> {
    fn install(store: &'a Store) -> Self {
        store.db.lock().unwrap().execute_batch(
            "CREATE TEMP TRIGGER fixture_recent_audit_abort BEFORE INSERT ON main.events WHEN NEW.kind='control.identity.access' AND json_extract(NEW.payload,'$.data.action')='events' BEGIN SELECT RAISE(ABORT,'synthetic recent audit insertion failure'); END;"
        ).unwrap();
        Self { store, active: true }
    }
    fn remove(mut self) {
        self.store.db.lock().unwrap().execute_batch("DROP TRIGGER temp.fixture_recent_audit_abort;").unwrap();
        self.active = false;
    }
}
impl Drop for AbortAudit<'_> {
    fn drop(&mut self) {
        if self.active {
            if let Ok(db) = self.store.db.try_lock() {
                let _ = db.execute_batch("DROP TRIGGER IF EXISTS temp.fixture_recent_audit_abort;");
            }
        }
    }
}

#[test]
fn modern_recent_real_audit_insert_abort_rolls_back_restores_713_then_recovers() {
    let fixture = Fixture::new();
    let bundle = fixture.migrated();
    scoped(&fixture, &bundle, |store, access, scope| {
        // A positive control establishes that this real checked route can
        // commit an events receipt before only its INSERT is made to fail.
        store.modern_human_events_recent(access, scope, &filter(), None, 1, &Redactor::new(vec![])).unwrap();
        store.db.lock().unwrap().busy_timeout(std::time::Duration::from_millis(713)).unwrap();
        let trigger = AbortAudit::install(store);
        let before = snapshot(store);
        let usage = || {
            let capacity = store.supervision_lease.capacity.lock().unwrap();
            json!({"tenant":capacity.tenant,"principals":capacity.principals})
        };
        let before_usage = usage();
        let error = store.modern_human_events_recent(access, scope, &filter(), None, 1, &Redactor::new(vec![])).err().unwrap();
        assert!(format!("{error:#}").contains("synthetic recent audit insertion failure"), "{error:#}");
        assert_eq!(snapshot(store), before);
        assert_eq!(usage(), before_usage);
        let restored: i64 = store.db.lock().unwrap().pragma_query_value(None, "busy_timeout", |row| row.get(0)).unwrap();
        assert_eq!(restored, 713);
        // Durable floor validation can independently advance; event/head/hash
        // and cached human usage above are the rollback oracle here.
        trigger.remove();
        store.modern_human_events_recent(access, scope, &filter(), None, 1, &Redactor::new(vec![])).unwrap();
        assert_eq!(snapshot(store).0, before.0 + 1);
        let restored: i64 = store.db.lock().unwrap().pragma_query_value(None, "busy_timeout", |row| row.get(0)).unwrap();
        assert_eq!(restored, 713);
    });
}
