//! Test-first intent archive: real private SQLite/source/binding fixture only.
//! Executed solely by Root's admitted offline QA, never a runtime/network test.
use super::*;
use fixtures::Fixture;

// Removing durable projections, or reconstructing authority from DTO fields,
// must break these assertions on a real reopened tenant Store.
#[test]
fn tenant_policy_survives_reopen_and_isolated_other_tenant_remains_empty() {
    let fixture = Fixture::new(EgressStoreLimitsV1::default());
    let request = fixture.replace(
        EgressPolicySelectorV1::Tenant,
        0,
        0,
        &["api.example.test:443"],
    );
    let receipt = fixture.install(&request).unwrap();
    assert_eq!(receipt.state(), EgressPolicyStateV1::Active);
    let original = fixture.rows(0);
    let reopened = fixture.reopen(0);
    assert_eq!(fixture.rows_for(&reopened), original);
    assert_eq!(fixture.rows(1).heads, 0);
    assert_eq!(fixture.rows(1).operations, 0);
    // The legacy integrity entrance deliberately refuses modern sources. Use
    // the genuine retained read epoch/source for this fixture's full check.
    let legacy_refusal = reopened.integrity().unwrap_err();
    assert_eq!(
        legacy_refusal.downcast_ref::<review_authority::ReviewAuthorityFailure>(),
        Some(&review_authority::ReviewAuthorityFailure::Required)
    );
    assert!(fixture.integrity_with_source(&reopened, 0).unwrap().valid);
    let foreign_source_refusal = fixture.integrity_with_source(&reopened, 1).unwrap_err();
    assert_eq!(
        foreign_source_refusal.downcast_ref::<review_authority::ReviewAuthorityFailure>(),
        Some(&review_authority::ReviewAuthorityFailure::Required)
    );
    assert_eq!(fixture.rows_for(&reopened), original);
}

// A replay that trusts rows before current source/actor validation leaks a
// retained receipt to Store B or a different authenticated machine identity.
#[test]
fn exact_retry_is_byte_identical_and_foreign_source_is_refused_before_replay() {
    let fixture = Fixture::new(EgressStoreLimitsV1::default());
    let request = fixture.replace(
        EgressPolicySelectorV1::Tenant,
        0,
        0,
        &["api.example.test:443"],
    );
    let first = fixture.install(&request).unwrap();
    let before = fixture.rows(0);
    assert_eq!(
        fixture
            .install_on(&fixture.reopen(0), &request)
            .unwrap()
            .canonical_bytes()
            .unwrap(),
        first.canonical_bytes().unwrap()
    );
    assert_eq!(fixture.rows(0), before);
    assert!(fixture.install_on(&fixture.stores[1], &request).is_err());
    assert!(fixture.install_with_other_credential(&request).is_err());
    assert_eq!(fixture.rows(0), before);
    assert_eq!(fixture.rows(1).operations, 0);
    let changed = fixture.replace_with_id(
        request.operation_id(),
        EgressPolicySelectorV1::Tenant,
        0,
        0,
        &["other.example.test:443"],
    );
    assert!(fixture.install(&changed).is_err());
    assert_eq!(fixture.rows(0), before);
}

// CAS must cover both revision and epoch; a different operation cannot create
// another terminal event from a revoked or stale head.
#[test]
fn revoke_is_requested_once_and_exact_retry_never_claims_runtime_application() {
    let fixture = Fixture::new(EgressStoreLimitsV1::default());
    let install = fixture.replace(
        EgressPolicySelectorV1::Tenant,
        0,
        0,
        &["api.example.test:443"],
    );
    fixture.install(&install).unwrap();
    let request =
        EgressRevokePolicyV1::new(uuid::Uuid::new_v4(), EgressPolicySelectorV1::Tenant, 1, 1)
            .unwrap();
    let revoked = fixture.revoke(&request).unwrap();
    assert_eq!(revoked.state(), EgressPolicyStateV1::RevokeRequested);
    assert_eq!(revoked.before_epoch(), 1);
    assert_eq!(revoked.after_epoch(), 2);
    let before = fixture.rows(0);
    assert_eq!(
        fixture.revoke(&request).unwrap().canonical_bytes().unwrap(),
        revoked.canonical_bytes().unwrap()
    );
    assert_eq!(fixture.rows(0), before);
    assert_eq!(before.reservations, 0);
    assert!(
        fixture
            .install(&fixture.replace(EgressPolicySelectorV1::Tenant, 1, 1, &[]))
            .is_err()
    );
    assert!(
        fixture
            .revoke(
                &EgressRevokePolicyV1::new(
                    uuid::Uuid::new_v4(),
                    EgressPolicySelectorV1::Tenant,
                    1,
                    1
                )
                .unwrap()
            )
            .is_err()
    );
    assert!(
        !String::from_utf8(revoked.canonical_bytes().unwrap())
            .unwrap()
            .contains("revoke_applied")
    );
    assert_eq!(fixture.rows_for(&fixture.reopen(0)), before);
}

// If terminal operation capacity is omitted, a logically full registry cannot
// revoke even though its audit reserve is intact (R2 builder finding).
#[test]
fn retained_entry_ceiling_holds_exact_terminal_slot_and_revoke_consumes_it() {
    let fixture = Fixture::new(EgressStoreLimitsV1::new(1, 3, 65_536).unwrap());
    fixture
        .install(&fixture.replace(EgressPolicySelectorV1::Tenant, 0, 0, &[]))
        .unwrap();
    let before = fixture.rows(0);
    assert!(
        fixture
            .install(&fixture.replace(EgressPolicySelectorV1::Tenant, 1, 1, &[]))
            .is_err()
    );
    assert_eq!(fixture.rows(0), before);
    fixture
        .revoke(
            &EgressRevokePolicyV1::new(uuid::Uuid::new_v4(), EgressPolicySelectorV1::Tenant, 1, 1)
                .unwrap(),
        )
        .unwrap();
    let after = fixture.rows(0);
    assert_eq!(after.operations + after.revisions + after.reservations, 3);
    assert_eq!(after.reservations, 0);
    assert_eq!(fixture.rows_for(&fixture.reopen(0)), after);
}

// Any non-consuming class using full physical capacity can steal the held
// terminal capacity; identity maintenance must obey the same held reservation.
#[test]
fn ordinary_control_and_identity_writes_preserve_terminal_audit_reservation() {
    for class in [
        fixtures::FillClass::Ordinary,
        fixtures::FillClass::Control,
        fixtures::FillClass::Identity,
    ] {
        let fixture = Fixture::new(EgressStoreLimitsV1::default());
        fixture
            .install(&fixture.replace(EgressPolicySelectorV1::Tenant, 0, 0, &[]))
            .unwrap();
        let second = EgressPolicySelectorV1::session(&uuid::Uuid::new_v4().to_string()).unwrap();
        fixture.create_session(&second);
        fixture
            .install(&fixture.replace(second.clone(), 0, 0, &[]))
            .unwrap();
        let observation = fixture.saturate_audit(class).unwrap();
        assert!(observation.accepted > 0);
        assert_eq!(
            observation.at_events - observation.before_events,
            observation.accepted
        );
        assert_eq!(observation.at_events, observation.event_limit);
        assert!(observation.at_bytes > observation.before_bytes);
        assert_eq!(
            fixture.audit_occupancy(),
            (observation.at_events, observation.at_bytes)
        );
        let before = fixture.rows(0);
        assert_eq!(before.reservations, 2);
        let other_reserve = fixture.reservation(&second).unwrap();
        assert_eq!(other_reserve, Some((1, EVENT_BYTES as u64)));
        fixture
            .revoke(
                &EgressRevokePolicyV1::new(
                    uuid::Uuid::new_v4(),
                    EgressPolicySelectorV1::Tenant,
                    1,
                    1,
                )
                .unwrap(),
            )
            .unwrap();
        assert_eq!(fixture.rows(0).reservations, 1);
        assert_eq!(fixture.reservation(&second).unwrap(), other_reserve);
        let (events, bytes) = fixture.audit_occupancy();
        assert_eq!(events, observation.at_events + 1);
        assert!(bytes > observation.at_bytes);
    }
}

// A policy store must never materialize sessions or accept a wider restriction
// than the retained tenant policy at its historical installation prefix.
#[test]
fn session_requires_real_active_resource_and_historical_tenant_ceiling() {
    let fixture = Fixture::new(EgressStoreLimitsV1::default());
    fixture
        .install(&fixture.replace(
            EgressPolicySelectorV1::Tenant,
            0,
            0,
            &["api.example.test:443"],
        ))
        .unwrap();
    let selector = EgressPolicySelectorV1::session(&uuid::Uuid::new_v4().to_string()).unwrap();
    assert!(
        fixture
            .install(&fixture.replace(selector.clone(), 0, 0, &[]))
            .is_err()
    );
    fixture.create_session(&selector);
    assert!(
        fixture
            .install(&fixture.replace(selector.clone(), 0, 0, &["other.example.test:443"]))
            .is_err()
    );
    fixture
        .install(&fixture.replace(selector.clone(), 0, 0, &["api.example.test:443"]))
        .unwrap();
    fixture
        .install(&fixture.replace(EgressPolicySelectorV1::Tenant, 1, 1, &[]))
        .unwrap();
    assert!(!fixture.current_intersection_allows(&selector, "api.example.test:443"));
    fixture.end_session(&selector);
    fixture
        .revoke(&EgressRevokePolicyV1::new(uuid::Uuid::new_v4(), selector, 1, 1).unwrap())
        .unwrap();
}

// Missing mandatory audit and actual SQLite FULL must roll back all five
// projections together with the operation receipt; no caller receives success.
#[test]
fn capacity_and_sqlite_full_leave_policy_operation_and_reserve_unchanged() {
    for full in [false, true] {
        let fixture = Fixture::new(EgressStoreLimitsV1::default());
        fixture
            .install(&fixture.replace(EgressPolicySelectorV1::Tenant, 0, 0, &[]))
            .unwrap();
        if full {
            fixture.force_sqlite_full();
        } else {
            fixture.exhaust_policy_audit();
        }
        let before = fixture.rows(0);
        let request = if full {
            fixture.large_replace()
        } else {
            fixture.replace(
                EgressPolicySelectorV1::Tenant,
                1,
                1,
                &["api.example.test:443"],
            )
        };
        let error = fixture.install(&request).unwrap_err();
        if full {
            assert!(error.chain().any(|cause| matches!(cause.downcast_ref::<rusqlite::Error>(), Some(rusqlite::Error::SqliteFailure(code,_)) if code.code == rusqlite::ErrorCode::DiskFull)), "expected actual SQLite FULL, got {error:#}");
        }
        assert_eq!(fixture.rows(0), before);
        assert!(
            Store::check_integrity(&fixture.stores[0].db.lock().unwrap())
                .unwrap()
                .valid
        );
    }
}

// A serialized table is only a projection. Reopen must compare all rows,
// limits and reservation totals with the immutable chain.
#[test]
fn reopen_rejects_projection_and_limits_tampering_and_redaction_collision() {
    for table in [
        "egress_policy_meta",
        "egress_policy_heads",
        "egress_policy_operations",
        "egress_policy_reservations",
        "egress_policy_revisions",
    ] {
        let fixture = Fixture::new(EgressStoreLimitsV1::default());
        fixture
            .install(&fixture.replace(EgressPolicySelectorV1::Tenant, 0, 0, &[]))
            .unwrap();
        fixture.corrupt_projection(table);
        assert!(fixture.try_reopen(0).is_err());
    }
    let fixture = Fixture::new(EgressStoreLimitsV1::default());
    let request = fixture.replace(
        EgressPolicySelectorV1::Tenant,
        0,
        0,
        &["sensitive.example.test:443"],
    );
    let redacted =
        fixture.reopen_with_redactor(Redactor::new(vec!["sensitive.example.test".into()]));
    let before = fixture.rows(0);
    assert!(fixture.install_on(&redacted, &request).is_err());
    assert_eq!(fixture.rows(0), before);
}

// Exact14 is a complete historical profile. It must not acquire hidden egress
// tables/events, and a migration cannot proceed without its real backup proof.
#[test]
fn maintenance_migration_is_explicit_backed_up_and_format14_remains_closed() {
    fixtures::assert_format14_compatibility_and_migration_refusals();
}

// Imported/corrupt history only: public ingestion already refuses this family.
// Removing the15 lower(kind) selection must make this genuine-source reopen
// accept a noncanonical extra line and therefore break the refusal assertion.
#[test]
fn format15_replay_refuses_noncanonical_reserved_family_without_projection_mutation() {
    for kind in [
        "Control.Egress_Policy.Unknown",
        "CONTROL.EGRESS_POLICY.REPLACED",
        "control.egress_policy.unknown",
    ] {
        let fixture = Fixture::new(EgressStoreLimitsV1::default());
        fixture.append_history_event(kind).unwrap();
        let before = fixture.rows(0);
        let error = fixture
            .try_reopen(0)
            .err()
            .expect("reserved corruption must refuse real source reopen");
        assert_eq!(
            error.downcast_ref::<EgressStoreFailure>(),
            Some(&EgressStoreFailure::Integrity)
        );
        assert!(
            fixture
                .install(&fixture.replace(EgressPolicySelectorV1::Tenant, 0, 0, &[]))
                .is_err()
        );
        assert_eq!(fixture.rows(0), before);
        assert_eq!(before.operations, 0);
    }
    fixtures::assert_format14_reserved_history_refused("Control.Egress_Policy.Unknown");
}

#[test]
fn inherited_noncanonical_ordinary_history_stays_ordinary_in_format15() {
    let fixture = Fixture::new(EgressStoreLimitsV1::default());
    fixture
        .append_history_event("Control.relay.historical_ordinary")
        .unwrap();
    let before = fixture.rows(0);
    assert_eq!(fixture.rows_for(&fixture.reopen(0)), before);
    fixture
        .install(&fixture.replace(EgressPolicySelectorV1::Tenant, 0, 0, &[]))
        .unwrap();
}

#[test]
fn saturation_helper_propagates_real_source_refusal_and_rejects_clock_error_classification() {
    for class in [
        fixtures::FillClass::Ordinary,
        fixtures::FillClass::Control,
        fixtures::FillClass::Identity,
    ] {
        let fixture = Fixture::new(EgressStoreLimitsV1::default());
        fixture
            .install(&fixture.replace(EgressPolicySelectorV1::Tenant, 0, 0, &[]))
            .unwrap();
        let before = fixture.rows(0);
        let error = fixture
            .saturate_audit_with_foreign_source(class)
            .unwrap_err();
        assert_eq!(
            error.downcast_ref::<review_authority::ReviewAuthorityFailure>(),
            Some(&review_authority::ReviewAuthorityFailure::Required)
        );
        assert_eq!(fixture.rows(0), before);
        let clock_error = crate::workload::WorkloadFailureV1::ClockRegression.into();
        assert!(fixtures::require_saturation_cause(class, clock_error).is_err());
    }
}

#[test]
fn maintenance_active_owner_refusal_is_typed_at_acquisition_and_effect_free() {
    fixtures::assert_stopped_consumer_maintenance_refusal_is_typed_and_effect_free();
}

#[test]
fn format15_genesis_predecessor_must_include_the_original_format14_anchor() {
    let fixture = Fixture::new(EgressStoreLimitsV1::default());
    fixture.assert_genesis_anchor_order_and_impossible_history();
}

#[test]
fn untrusted_case_variants_are_reserved_even_before_egress_migration() {
    use std::os::unix::fs::PermissionsExt;
    // tempfile3.27 defaults to0777 & process umask for directories. Request
    // this new fixture's private mode at creation; never repair existing data.
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    assert_eq!(
        directory.path().metadata().unwrap().permissions().mode() & 0o7777,
        0o700
    );
    let store = Store::open_for_tenant(
        &directory.path().join("audit.sqlite"),
        "A",
        Redactor::new(vec![]),
        1000,
    )
    .unwrap();
    let ordinary = "fixture.ordinary_before_egress";
    assert_eq!(
        store.append(Event::new(ordinary)).unwrap().event.kind,
        ordinary
    );
    assert!(store.integrity().unwrap().valid);
    let snapshot = || {
        let db = store.db.lock().unwrap();
        (
            db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            audit_head(&db).unwrap(),
            db.query_row("SELECT count(*) FROM events", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            db.query_row("SELECT bytes FROM quota WHERE id=1", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap(),
            db.query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name GLOB 'egress_policy_*'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap(),
        )
    };
    let before = snapshot();
    assert_eq!(before.0, 2);
    assert_eq!(before.4, 0);
    for kind in [
        "control.egress_policy.unknown",
        "Control.Egress_Policy.Unknown",
        "CONTROL.EGRESS_POLICY.GENESIS",
    ] {
        let error = store.append(Event::new(kind)).unwrap_err();
        assert_eq!(error.to_string(), "control events require typed mutation");
        assert_eq!(snapshot(), before);
    }
    assert_eq!(
        store.append(Event::new(ordinary)).unwrap().event.kind,
        ordinary
    );
    assert!(store.integrity().unwrap().valid);
}

#[test]
fn revoked_live_binding_cannot_replay_retained_machine_receipt() {
    let fixture = Fixture::new(EgressStoreLimitsV1::default());
    let request = fixture.replace(EgressPolicySelectorV1::Tenant, 0, 0, &[]);
    fixture.install(&request).unwrap();
    fixture.revoke_control_binding();
    let before = fixture.rows(0);
    assert!(fixture.install(&request).is_err());
    assert_eq!(fixture.rows(0), before);
}

#[test]
fn durable_clock_regression_refuses_new_mutation_and_exact_replay() {
    let fixture = Fixture::new(EgressStoreLimitsV1::default());
    let request = fixture.replace(EgressPolicySelectorV1::Tenant, 0, 0, &[]);
    fixture.install(&request).unwrap();
    fixture.advance_clock_floor();
    let before = fixture.rows(0);
    for request in [
        request,
        fixture.replace(EgressPolicySelectorV1::Tenant, 1, 1, &[]),
    ] {
        assert!(fixture.install(&request).is_err());
        assert_eq!(fixture.rows(0), before);
    }
}

#[test]
fn terminal_transition_does_not_require_old_policy_interval_to_be_active() {
    let fixture = Fixture::new(EgressStoreLimitsV1::default());
    fixture
        .install(&fixture.replace(EgressPolicySelectorV1::Tenant, 0, 0, &[]))
        .unwrap();
    fixture.assert_expired_head_terminal_transition();
}

#[test]
fn invalid_limits_and_sqlite_integer_overflow_refuse_before_storage() {
    for limits in [
        (0, 3, 65536),
        (4097, 3, 65536),
        (1, 0, 65536),
        (1, 65537, 65536),
        (1, 3, 0),
        (1, 3, 67108865),
    ] {
        assert!(EgressStoreLimitsV1::new(limits.0, limits.1, limits.2).is_err());
    }
    assert!(
        EgressRevokePolicyV1::new(uuid::Uuid::nil(), EgressPolicySelectorV1::Tenant, 1, 1).is_err()
    );
    assert!(
        EgressRevokePolicyV1::new(
            uuid::Uuid::new_v4(),
            EgressPolicySelectorV1::Tenant,
            u64::MAX,
            1
        )
        .is_err()
    );
    assert!(EgressPolicySelectorV1::session("../other-session").is_err());
}
