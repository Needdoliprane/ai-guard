use super::*;
use crate::human_authority::migration::tests::Fixture;

#[test]
fn exact_union_is_required_before_any_new_incarnation() {
    let fixture = Fixture::new_owned_local();
    let (mut request, _, _) = fixture.owned_request();
    let path = request.configuration.sources[0].storage_binding.path();
    let read_revision = || {
        rusqlite::Connection::open(&path)
            .unwrap()
            .query_row(
                "SELECT incarnation_revision FROM review_authority_meta WHERE singleton=1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
    };
    let before = read_revision();
    request.sources.remove(&Uuid::from_u128(2));
    assert!(HumanOwnedStartupBundle::open(request).is_err());
    assert_eq!(read_revision(), before);
}

#[test]
fn later_source_busy_cannot_append_an_incarnation_in_first_source() {
    let fixture = Fixture::new_owned_local();
    let (request, _, _) = fixture.owned_request();
    let first = request.configuration.sources[0].storage_binding.path();
    let second = request.configuration.sources[1].storage_binding.path();
    let first_db = rusqlite::Connection::open(first).unwrap();
    let before: i64 = first_db
        .query_row(
            "SELECT incarnation_revision FROM review_authority_meta WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let busy = rusqlite::Connection::open(second).unwrap();
    busy.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert!(HumanOwnedStartupBundle::open(request).is_err());
    let after: i64 = first_db
        .query_row(
            "SELECT incarnation_revision FROM review_authority_meta WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(before, after);
    assert!(!busy.is_autocommit());
    busy.execute_batch("ROLLBACK").unwrap();
}

#[test]
fn configured_auth_capacity_mismatch_refuses_before_first_incarnation() {
    let fixture = Fixture::new_owned();
    let (mut request, _, _) = fixture.owned_request();
    let first = request
        .configuration
        .sources
        .iter()
        .find(|source| source.kind == HumanSourceKindV1::TenantStore)
        .unwrap()
        .storage_binding
        .path();
    let db = rusqlite::Connection::open(first).unwrap();
    let before: i64 = db
        .query_row(
            "SELECT incarnation_revision FROM review_authority_meta WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let options = request
        .sources
        .values_mut()
        .find(|options| {
            matches!(
                options,
                HumanOwnedSourceOptions::AuthenticationLedger { .. }
            )
        })
        .unwrap();
    if let HumanOwnedSourceOptions::AuthenticationLedger { capacity_plan, .. } = options {
        capacity_plan.unscoped.max_records += 1;
    }
    assert!(HumanOwnedStartupBundle::open(request).is_err());
    let after: i64 = db
        .query_row(
            "SELECT incarnation_revision FROM review_authority_meta WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(before, after);
}

#[tokio::test]
async fn drain_error_does_not_release_owner_retained_by_a_real_epoch() {
    let fixture = Fixture::new_owned_local();
    let (bundle, _, _) = fixture.owned_startup();
    let owner = bundle.owner.clone();
    let weak = Arc::downgrade(&owner.owner);
    let epoch = owner.try_read_epoch().unwrap();
    // Poison only synthetic owner memory. This is an error-path ownership
    // test, not a forged successful drain or a source authority constructor.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _state = owner.owner.state.lock().unwrap();
        panic!("synthetic owner state poison");
    }));
    assert!(owner.drain().await.is_err());
    drop(bundle);
    drop(owner);
    assert!(
        weak.upgrade().is_some(),
        "real epoch must retain owner/FDs after unestablished drain"
    );
    drop(epoch);
    assert!(weak.upgrade().is_none());
}
