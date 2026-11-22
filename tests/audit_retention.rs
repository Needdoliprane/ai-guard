//! Audit retention at the SQLite Store seam: a full tenant store recovers its
//! capacity, the live store and its archives verify end to end, the legacy
//! views stay those of the complete history, and every refusal leaves the
//! live chain unchanged. The evaluation time is explicit so that history can
//! be older than the 24-hour policy floor without waiting.
mod support;

use agentsandbox::{
    audit::{
        Store,
        retention::{
            AuditRetentionPolicyV1, PlannedRetentionV1, ReclaimClassV1, RetentionApplyRequest,
            failure::refusal,
        },
    },
    identity::*,
    model::Event,
    redaction::Redactor,
};
use chrono::{Duration, Utc};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};
use uuid::Uuid;

const TENANT: &str = "tenant-a";
const MAX_EVENTS: u64 = 200;

fn private_dir(path: &Path) {
    std::fs::create_dir(path).unwrap();
    std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
}

fn store13(dir: &Path) -> (PathBuf, Store) {
    let data = dir.join("tenant-a.sqlite");
    let history = dir.join("history");
    private_dir(&history);
    let store = Store::open_for_tenant(&data, TENANT, Redactor::new(vec![]), MAX_EVENTS).unwrap();
    for (index, migrate) in [
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
    ]
    .into_iter()
    .enumerate()
    {
        migrate(
            &store,
            &history.join(format!("before-{}.sqlite", index + 3)),
        )
        .unwrap();
    }
    (data, store)
}

fn reopen(data: &Path) -> anyhow::Result<Store> {
    Store::open_existing_for_tenant(data, TENANT, Redactor::new(vec![]), MAX_EVENTS)
}

fn policy() -> AuditRetentionPolicyV1 {
    AuditRetentionPolicyV1 {
        version: 1,
        min_age_seconds: 86_400,
        keep_recent_events: 1,
        max_segment_events: 100_000,
        classes: vec![ReclaimClassV1::Ordinary, ReclaimClassV1::HumanRead],
    }
}

fn later() -> chrono::DateTime<Utc> {
    Utc::now() + Duration::days(2)
}

fn code_of(error: &anyhow::Error) -> Option<&'static str> {
    refusal(error).map(|failure| failure.code)
}

fn activity(store: &Store, index: usize) -> anyhow::Result<()> {
    let mut event = Event::new(if index % 3 == 0 {
        "agent.started"
    } else {
        "command.requested"
    });
    event.session_id = Some(format!("session-{}", index % 4));
    event.agent_id = Some(format!("agent-{}", index % 2));
    event.sandbox_id = Some("sandbox-1".into());
    event.data = serde_json::json!({"index": index, "padding": "x".repeat(200)});
    store.append(event).map(|_| ())
}

fn apply(
    store: &Store,
    planned: &PlannedRetentionV1,
    archive: &Path,
) -> anyhow::Result<agentsandbox::audit::retention::RetentionReceiptV1> {
    store.apply_audit_retention(&RetentionApplyRequest {
        policy: &policy(),
        now: later(),
        through_seq: planned.plan.last_seq,
        expect_plan_sha256: &planned.plan_sha256,
        archive_dir: archive,
        kept_positions: &BTreeSet::new(),
    })
}

#[test]
fn a_full_store_recovers_capacity_and_verifies_end_to_end() {
    let dir = support::private_tempdir();
    let (data, store) = store13(dir.path());
    store
        .install_audit_retention(&dir.path().join("before-retention.sqlite"))
        .unwrap();
    let mut filled = 0;
    let refused = loop {
        match activity(&store, filled) {
            Ok(()) => filled += 1,
            Err(error) => break error,
        }
        assert!(filled < 400, "the ordinary budget never closed");
    };
    assert!(
        format!("{refused:#}").contains("audit capacity"),
        "{refused:#}"
    );
    let entities_before = (
        store.entities(false).unwrap(),
        store.entities(true).unwrap(),
    );
    let counts_before = store.overview().unwrap()["counts"].clone();

    let planned = store
        .audit_retention_plan(&policy(), later(), None, &BTreeSet::new())
        .unwrap();
    assert_eq!(planned.plan.segment_index, 1);
    assert_eq!(planned.plan.first_seq, 1);
    assert!(planned.plan.reclaimed_events > 0);
    // Four sessions, two agents and one sandbox are first sighted once each;
    // the boundary is also kept.
    assert!(planned.plan.retained_pins >= 4);
    let archive_1 = dir.path().join("archive-1");
    let receipt = apply(&store, &planned, &archive_1).unwrap();
    assert_eq!(receipt.reclaimed_events, planned.plan.reclaimed_events);
    assert_eq!(
        receipt.usage_after.stored_events,
        planned.usage_before.stored_events - planned.plan.reclaimed_events + 1
    );
    assert!(receipt.usage_after.stored_bytes < planned.usage_before.stored_bytes);

    // The legacy views are still those of the complete history.
    assert_eq!(
        (
            store.entities(false).unwrap(),
            store.entities(true).unwrap()
        ),
        entities_before
    );
    assert_eq!(store.overview().unwrap()["counts"], counts_before);

    // Capacity is available again, the store reopens and verifies.
    activity(&store, 1000).unwrap();
    drop(store);
    let store = reopen(&data).unwrap();
    let integrity = store.integrity().unwrap();
    assert!(integrity.valid, "{:?}", integrity.error);
    assert_eq!(
        integrity.event_count,
        u64::try_from(receipt.retention_event_seq).unwrap() + 1
    );
    let ledger = store.audit_retention_ledger().unwrap();
    assert_eq!(ledger.segments, 1);
    assert_eq!(ledger.archived_through_seq, planned.plan.last_seq);
    assert_eq!(ledger.reclaimed_events, planned.plan.reclaimed_events);

    let verified = store.verify_audit_archives(&[archive_1.clone()]).unwrap();
    assert_eq!(verified.verdict, "verified", "{:?}", verified.failure);
    assert_eq!(verified.segments_verified, 1);
    let missing = store.verify_audit_archives(&[]).unwrap();
    assert_eq!(missing.verdict, "refused");
    assert_eq!(missing.failure.unwrap().code, "archive_missing");

    // A second segment chains to the first; archives may come in any order.
    for index in 0..20 {
        activity(&store, 2000 + index).unwrap();
    }
    let planned_2 = store
        .audit_retention_plan(&policy(), later(), None, &BTreeSet::new())
        .unwrap();
    assert_eq!(planned_2.plan.segment_index, 2);
    assert_eq!(planned_2.plan.first_seq, planned.plan.last_seq + 1);
    let archive_2 = dir.path().join("archive-2");
    apply(&store, &planned_2, &archive_2).unwrap();
    let verified = store
        .verify_audit_archives(&[archive_2.clone(), archive_1.clone()])
        .unwrap();
    assert_eq!(verified.verdict, "verified", "{:?}", verified.failure);
    assert_eq!(verified.segments_verified, 2);

    // One flipped payload byte in the first archive refuses the whole prefix.
    let events = archive_1.join("events.jsonl");
    let original = std::fs::read(&events).unwrap();
    let mut tampered = original.clone();
    let at = tampered
        .windows(7)
        .position(|window| window == b"padding")
        .unwrap();
    tampered[at] = b'P';
    std::fs::write(&events, &tampered).unwrap();
    let refused = store
        .verify_audit_archives(&[archive_1.clone(), archive_2.clone()])
        .unwrap();
    assert_eq!(refused.verdict, "refused");
    assert_eq!(refused.failure.unwrap().code, "archive_chain_invalid");
    std::fs::write(&events, &original).unwrap();
    assert_eq!(
        store
            .verify_audit_archives(&[archive_1.clone(), archive_2.clone()])
            .unwrap()
            .verdict,
        "verified"
    );
    drop(store);

    // A deletion that no segment accounts for is detected at reopening.
    let pinned = {
        let db = rusqlite::Connection::open(&data).unwrap();
        let (seq, bytes): (i64, i64) = db
            .query_row(
                "SELECT seq,length(CAST(payload AS BLOB)) FROM events WHERE seq<=?1 AND kind='command.requested' ORDER BY seq LIMIT 1",
                [planned.plan.last_seq - 1],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        db.execute_batch(&format!(
            "DELETE FROM audit_time_floors WHERE event_seq={seq}; DELETE FROM events WHERE seq={seq}; UPDATE quota SET bytes=bytes-{bytes} WHERE id=1;"
        ))
        .unwrap();
        seq
    };
    let reopened = reopen(&data);
    let detected = match &reopened {
        Err(_) => true,
        Ok(store) => !store.integrity().unwrap().valid,
    };
    assert!(
        detected,
        "the deletion of retained event {pinned} went unnoticed"
    );
}

#[test]
fn refusals_leave_the_live_chain_and_destinations_unchanged() {
    let dir = support::private_tempdir();
    let (data, store) = store13(dir.path());
    for index in 0..10 {
        activity(&store, index).unwrap();
    }
    let missing = store
        .audit_retention_plan(&policy(), later(), None, &BTreeSet::new())
        .unwrap_err();
    assert_eq!(code_of(&missing), Some("retention_capability_missing"));
    store
        .install_audit_retention(&dir.path().join("before-retention.sqlite"))
        .unwrap();
    let twice = store
        .install_audit_retention(&dir.path().join("second-backup.sqlite"))
        .unwrap_err();
    assert_eq!(code_of(&twice), Some("retention_capability_present"));
    assert!(!dir.path().join("second-backup.sqlite").exists());

    let before = store.integrity().unwrap();
    let young = store
        .audit_retention_plan(&policy(), Utc::now(), None, &BTreeSet::new())
        .unwrap_err();
    assert_eq!(code_of(&young), Some("nothing_to_reclaim"));
    let planned = store
        .audit_retention_plan(&policy(), later(), None, &BTreeSet::new())
        .unwrap();
    let beyond = store
        .audit_retention_plan(
            &policy(),
            later(),
            Some(planned.plan.head_seq),
            &BTreeSet::new(),
        )
        .unwrap_err();
    assert_eq!(code_of(&beyond), Some("range_invalid"));

    let existing = dir.path().join("existing");
    private_dir(&existing);
    let reused = apply(&store, &planned, &existing).unwrap_err();
    assert_eq!(code_of(&reused), Some("archive_destination_not_new"));
    assert!(std::fs::read_dir(&existing).unwrap().next().is_none());

    let fresh = dir.path().join("archive-1");
    let mismatch = store
        .apply_audit_retention(&RetentionApplyRequest {
            policy: &policy(),
            now: later(),
            through_seq: planned.plan.last_seq,
            expect_plan_sha256: &"0".repeat(64),
            archive_dir: &fresh,
            kept_positions: &BTreeSet::new(),
        })
        .unwrap_err();
    assert_eq!(code_of(&mismatch), Some("plan_mismatch"));
    assert!(!fresh.exists());

    // A writer between plan and apply changes the plan.
    activity(&store, 100).unwrap();
    let moved = apply(&store, &planned, &fresh).unwrap_err();
    assert_eq!(code_of(&moved), Some("plan_mismatch"));
    assert!(!fresh.exists());
    let after = store.integrity().unwrap();
    assert!(after.valid);
    assert_eq!(after.event_count, before.event_count + 1);
    drop(store);
    assert!(reopen(&data).unwrap().integrity().unwrap().valid);
}

#[test]
fn an_ordinary_event_dated_before_the_archived_horizon_is_refused() {
    let dir = support::private_tempdir();
    let (_data, store) = store13(dir.path());
    store
        .install_audit_retention(&dir.path().join("before-retention.sqlite"))
        .unwrap();
    for index in 0..10 {
        activity(&store, index).unwrap();
    }
    let planned = store
        .audit_retention_plan(&policy(), later(), None, &BTreeSet::new())
        .unwrap();
    apply(&store, &planned, &dir.path().join("archive-1")).unwrap();
    let mut replay = Event::new("command.requested");
    replay.timestamp =
        (Utc::now() - Duration::hours(1)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let refused = store.append(replay).unwrap_err();
    assert_eq!(code_of(&refused), Some("event_predates_archive"));
    store.append(Event::new("command.requested")).unwrap();
}

fn operator() -> PrincipalRefV1 {
    PrincipalRefV1::LegacyControl {
        credential_id: "fixture-control-operator".into(),
        role: LegacyControlRoleV1::Runner,
    }
}

fn mutation(change: IdentityChangeV1) -> IdentityMutationV1 {
    IdentityMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        actor: operator(),
        change,
    }
}

#[test]
fn a_grant_after_retention_still_finds_the_first_sighting_of_its_session() {
    let dir = support::private_tempdir();
    let (data, store) = store13(dir.path());
    store
        .install_audit_retention(&dir.path().join("before-retention.sqlite"))
        .unwrap();
    let principal = PrincipalRefV1::HumanLocal {
        installation_id: "909d9572-f1a2-4a53-ad8b-a741030a2b18".into(),
        account_id: "871be6f3-b1a5-4eb9-9c72-989cedb05c43".into(),
    };
    let token = generate_local_credential().unwrap();
    store
        .provision_local_human(&LocalHumanProvisionV1 {
            version: 1,
            mutations: [
                mutation(IdentityChangeV1::Account {
                    expected_revision: 0,
                    account: HumanAccountV1 {
                        version: 1,
                        principal: principal.clone(),
                        display_name: "Named local reader".into(),
                        revision: 1,
                        active: true,
                    },
                }),
                mutation(IdentityChangeV1::Credential {
                    expected_revision: 0,
                    binding: LocalCredentialV1 {
                        version: 1,
                        credential_id: Uuid::new_v4().to_string(),
                        principal: principal.clone(),
                        sha256_digest: hex::encode(Sha256::digest(token.as_bytes())),
                        revision: 1,
                        active: true,
                        expires_at: None,
                    },
                }),
                mutation(IdentityChangeV1::Membership {
                    expected_revision: 0,
                    membership: MembershipV1 {
                        version: 1,
                        membership_id: Uuid::new_v4().to_string(),
                        principal: principal.clone(),
                        tenant_id: TENANT.into(),
                        organization_scope: "organization-a".into(),
                        revision: 1,
                        active: true,
                        capabilities: BTreeSet::from([HumanCapabilityV1::ReadMetadata]),
                    },
                }),
            ],
        })
        .unwrap();
    let grant = |session: &str| {
        mutation(IdentityChangeV1::ResourceGrant {
            expected_revision: 0,
            grant: ResourceGrantV1 {
                version: 1,
                tenant_id: TENANT.into(),
                organization_scope: "organization-a".into(),
                resource: ResourceRefV1 {
                    kind: ResourceKindV1::Session,
                    resource_id: session.into(),
                },
                owner: principal.clone(),
                revision: 1,
                active: true,
                shares: vec![],
            },
        })
    };
    // session-0 is granted before retention, session-1 only afterwards.
    for index in 0..12 {
        activity(&store, index).unwrap();
    }
    store.mutate_identity(&grant("session-0")).unwrap();
    for index in 12..20 {
        activity(&store, index).unwrap();
    }
    let planned = store
        .audit_retention_plan(&policy(), later(), None, &BTreeSet::new())
        .unwrap();
    apply(&store, &planned, &dir.path().join("archive-1")).unwrap();
    let mut sightings = 0;
    for event in store
        .events(&agentsandbox::audit::EventQuery {
            kind: Some("command.requested".into()),
            ..Default::default()
        })
        .unwrap()
    {
        if event.event.session_id.as_deref() == Some("session-1")
            && event.seq <= planned.plan.last_seq
        {
            sightings += 1;
        }
    }
    // The first sighting stays; at most the segment boundary is also kept.
    assert!((1..=2).contains(&sightings), "{sightings}");
    store.mutate_identity(&grant("session-1")).unwrap();
    drop(store);
    let reopened = reopen(&data).unwrap();
    assert!(reopened.integrity().unwrap().valid);
    let verified = reopened
        .verify_audit_archives(&[dir.path().join("archive-1")])
        .unwrap();
    assert_eq!(verified.verdict, "verified", "{:?}", verified.failure);
}

#[test]
fn a_kept_signed_position_stays_live_and_the_prefix_still_verifies() {
    let dir = support::private_tempdir();
    let (data, store) = store13(dir.path());
    store
        .install_audit_retention(&dir.path().join("before-retention.sqlite"))
        .unwrap();
    for index in 0..30 {
        activity(&store, index).unwrap();
    }
    let unpinned = store
        .audit_retention_plan(&policy(), later(), None, &BTreeSet::new())
        .unwrap();
    // A reclaimable ordinary event that a signed checkpoint names.
    let signed = store
        .events(&agentsandbox::audit::EventQuery {
            kind: Some("command.requested".into()),
            ..Default::default()
        })
        .unwrap()
        .into_iter()
        .map(|event| event.seq)
        .filter(|seq| *seq < unpinned.plan.last_seq)
        .max()
        .unwrap();
    let kept = BTreeSet::from([signed]);
    let planned = store
        .audit_retention_plan(&policy(), later(), None, &kept)
        .unwrap();
    assert_eq!(planned.plan.kept_positions, vec![signed]);
    assert_eq!(
        planned.plan.reclaimed_events,
        unpinned.plan.reclaimed_events - 1
    );
    assert_ne!(planned.plan_sha256, unpinned.plan_sha256);
    let archive = dir.path().join("archive-1");
    store
        .apply_audit_retention(&RetentionApplyRequest {
            policy: &policy(),
            now: later(),
            through_seq: planned.plan.last_seq,
            expect_plan_sha256: &planned.plan_sha256,
            archive_dir: &archive,
            kept_positions: &kept,
        })
        .unwrap();
    let live = store
        .events(&agentsandbox::audit::EventQuery {
            kind: Some("command.requested".into()),
            ..Default::default()
        })
        .unwrap();
    assert!(live.iter().any(|event| event.seq == signed));
    drop(store);
    let store = reopen(&data).unwrap();
    assert!(store.integrity().unwrap().valid);
    let verified = store.verify_audit_archives(&[archive]).unwrap();
    assert_eq!(verified.verdict, "verified", "{:?}", verified.failure);
}

#[test]
fn the_payload_cap_is_bounded_and_selected_before_any_capacity_policy() {
    use agentsandbox::audit::{
        AuditCapacityPolicyV1, DEFAULT_PHYSICAL_BYTES, MAX_SQLITE_PHYSICAL_BYTES,
    };
    let dir = support::private_tempdir();
    let (data, store) = store13(dir.path());
    drop(store);
    for refused in [DEFAULT_PHYSICAL_BYTES - 1, MAX_SQLITE_PHYSICAL_BYTES + 1] {
        assert!(reopen(&data).unwrap().with_physical_bytes(refused).is_err());
    }
    let wide = reopen(&data)
        .unwrap()
        .with_physical_bytes(MAX_SQLITE_PHYSICAL_BYTES)
        .unwrap();
    assert_eq!(wide.physical_bytes(), MAX_SQLITE_PHYSICAL_BYTES);
    assert_eq!(
        wide.capacity_policy(),
        &AuditCapacityPolicyV1::resolved_for(MAX_EVENTS, MAX_SQLITE_PHYSICAL_BYTES)
    );
    wide.append(Event::new("command.requested")).unwrap();
    let mut policy = AuditCapacityPolicyV1::resolved(MAX_EVENTS);
    policy.control_reserve.events += 1;
    let explicit = reopen(&data).unwrap().with_capacity_policy(policy).unwrap();
    assert!(
        explicit
            .with_physical_bytes(MAX_SQLITE_PHYSICAL_BYTES)
            .is_err()
    );
}
