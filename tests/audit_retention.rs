//! Audit retention at the SQLite Store seam: a full tenant store recovers its
//! capacity, the live store and its archives verify end to end, the legacy
//! views stay those of the complete history, and every refusal leaves the
//! live chain unchanged. The evaluation time is explicit so that history can
//! be older than the 24-hour policy floor without waiting.
mod support;

use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::{
        EventQuery, Store,
        retention::{
            AuditRetentionPolicyV1, PlannedRetentionV1, ReclaimClassV1, RetentionApplyRequest,
            failure::refusal,
        },
    },
    identity::*,
    model::Event,
    policy::Policy,
    redaction::Redactor,
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::Arc,
};
use tower::ServiceExt;
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
fn archive_destination_cannot_overlap_the_active_sqlite_namespace() {
    let directory = support::private_tempdir();
    let (data, store) = store13(directory.path());
    store
        .install_audit_retention(&directory.path().join("pre-retention.sqlite"))
        .unwrap();
    for index in 0..18 {
        activity(&store, index).unwrap();
    }
    let planned = store
        .audit_retention_plan(&policy(), later(), None, &BTreeSet::new())
        .unwrap();
    let head = store.head().unwrap();
    let reserved_leaf = "tenant-a.sqlite-JOURNAL";
    let reserved = directory.path().join(reserved_leaf);
    assert!(!reserved.exists());
    assert_eq!(
        code_of(&apply(&store, &planned, &reserved).unwrap_err()),
        Some("archive_destination_reserved")
    );
    assert!(!reserved.exists());
    assert_eq!(
        code_of(
            &store
                .export_audit_archive(&reserved, None, None)
                .unwrap_err()
        ),
        Some("archive_destination_reserved")
    );
    assert!(!reserved.exists());
    assert_eq!(store.head().unwrap(), head);
    let after = store
        .audit_retention_plan(&policy(), later(), None, &BTreeSet::new())
        .unwrap();
    assert_eq!(after.usage_before, planned.usage_before);
    assert_eq!(store.audit_retention_ledger().unwrap().segments, 0);
    drop(store);

    let reopened = reopen(&data).unwrap();
    assert_eq!(reopened.head().unwrap(), head);
    assert!(reopened.integrity().unwrap().valid);
    let outside = directory.path().join("archives");
    private_dir(&outside);
    let archive = outside.join(reserved_leaf);
    reopened.export_audit_archive(&archive, None, None).unwrap();
    assert!(archive.join("events.jsonl").is_file());
}

#[test]
fn apply_reissues_the_durable_receipt_when_its_acknowledgement_was_lost() {
    let directory = support::private_tempdir();
    let (data, store) = store13(directory.path());
    store
        .install_audit_retention(&directory.path().join("pre-retention.sqlite"))
        .unwrap();
    for index in 0..18 {
        activity(&store, index).unwrap();
    }
    let planned = store
        .audit_retention_plan(&policy(), later(), None, &BTreeSet::new())
        .unwrap();
    let archive = directory.path().join("segment");
    let committed = apply(&store, &planned, &archive).unwrap();
    // Retain the durable state and archive, as after a lost operator response.
    // A new process may be replaying the operation after further appends.
    store.append(Event::new("fixture.followup")).unwrap();
    let head = store.head().unwrap();
    drop(store);

    let reopened = reopen(&data).unwrap();
    let replayed = apply(&reopened, &planned, &archive).unwrap();
    assert_eq!(
        serde_json::to_value(replayed).unwrap(),
        serde_json::to_value(committed).unwrap()
    );
    assert_eq!(reopened.head().unwrap(), head);
    assert_eq!(reopened.audit_retention_ledger().unwrap().segments, 1);
}

#[cfg(unix)]
#[test]
fn retention_replays_after_process_is_killed_after_commit() {
    use std::{
        fs,
        process::{Command, Stdio},
        thread,
        time::{Duration, Instant},
    };

    const CHILD_DIR: &str = "AI_GUARD_RETENTION_CRASH_TEST_DIR";
    const EXPECTED_PLAN: &str = "AI_GUARD_RETENTION_CRASH_TEST_PLAN";
    if let Some(root) = std::env::var_os(CHILD_DIR) {
        let root = PathBuf::from(root);
        let store = reopen(&root.join("tenant-a.sqlite")).unwrap();
        let planned = store
            .audit_retention_plan(&policy(), later(), None, &BTreeSet::new())
            .unwrap();
        assert_eq!(planned.plan_sha256, std::env::var(EXPECTED_PLAN).unwrap());
        let receipt = apply(&store, &planned, &root.join("segment")).unwrap();
        // Private test witness only. No receipt is returned to the caller.
        fs::write(
            root.join("witness.tmp"),
            serde_json::to_vec(&receipt).unwrap(),
        )
        .unwrap();
        fs::rename(root.join("witness.tmp"), root.join("committed")).unwrap();
        thread::sleep(Duration::from_secs(30));
        panic!("parent did not interrupt the committed child");
    }

    let directory = support::private_tempdir();
    let root = directory.path();
    let (data, store) = store13(root);
    store
        .install_audit_retention(&root.join("pre-retention.sqlite"))
        .unwrap();
    for index in 0..18 {
        activity(&store, index).unwrap();
    }
    let planned = store
        .audit_retention_plan(&policy(), later(), None, &BTreeSet::new())
        .unwrap();
    drop(store);

    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "retention_replays_after_process_is_killed_after_commit",
        ])
        .env_clear()
        .env(CHILD_DIR, root)
        .env(EXPECTED_PLAN, &planned.plan_sha256)
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let witness = root.join("committed");
    while !witness.is_file() && Instant::now() < deadline {
        if child.try_wait().unwrap().is_some() {
            panic!("child exited before establishing commit");
        }
        thread::sleep(Duration::from_millis(10));
    }
    let delivered = witness.is_file();
    let killed = child.kill();
    let status = child.wait().unwrap();
    killed.unwrap();
    assert!(
        delivered,
        "child did not establish commit before the deadline"
    );
    use std::os::unix::process::ExitStatusExt;
    assert_eq!(status.signal(), Some(libc::SIGKILL));
    let receipt: Value = serde_json::from_slice(&fs::read(witness).unwrap()).unwrap();

    let reopened = reopen(&data).unwrap();
    let head = reopened.head().unwrap();
    let replay = apply(&reopened, &planned, &root.join("segment")).unwrap();
    assert_eq!(serde_json::to_value(replay).unwrap(), receipt);
    assert_eq!(reopened.head().unwrap(), head);
    assert_eq!(reopened.audit_retention_ledger().unwrap().segments, 1);
    let verified = reopened
        .verify_audit_archives(&[root.join("segment")])
        .unwrap();
    assert_eq!(verified.verdict, "verified");
    assert_eq!(verified.segments_verified, 1);
}

#[test]
fn retention_and_kubernetes_effects_preserve_both_capabilities_on_reopen() {
    let dir = support::private_tempdir();
    let (data, store) = store13(dir.path());
    let retention = store
        .install_audit_retention(&dir.path().join("before-retention.sqlite"))
        .unwrap();
    let effects = store
        .enable_kubernetes_session_effects_v1(&dir.path().join("before-effects.sqlite"))
        .unwrap();
    assert_eq!((effects.from_version, effects.to_version), (13, 13));
    assert!(effects.genesis_seq > retention.genesis_seq);
    for index in 0..12 {
        activity(&store, index).unwrap();
    }

    let plan = store
        .audit_retention_plan(&policy(), later(), None, &BTreeSet::new())
        .unwrap();
    assert!(plan.plan.reclaimed_events > 0);
    let archive = dir.path().join("archive");
    apply(&store, &plan, &archive).unwrap();
    drop(store);

    let reopened = reopen(&data).unwrap();
    let verified = reopened.verify_audit_archives(&[archive]).unwrap();
    assert_eq!(verified.verdict, "verified", "{:?}", verified.failure);
    assert_eq!(verified.segments_verified, 1);
    let events = reopened.events(&EventQuery::default()).unwrap();
    for (kind, seq, hash) in [
        (
            "control.audit_retention.genesis",
            retention.genesis_seq,
            retention.genesis_hash,
        ),
        (
            "control.kubernetes.session_effect.genesis",
            effects.genesis_seq,
            effects.genesis_hash,
        ),
    ] {
        assert!(
            events
                .iter()
                .any(|row| { row.event.kind == kind && row.seq == seq && row.hash == hash })
        );
    }
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
fn claude_parent_inference_uses_only_the_live_suffix_after_retention() {
    let dir = support::private_tempdir();
    let (data, store) = store13(dir.path());
    store
        .install_audit_retention(&dir.path().join("before-retention.sqlite"))
        .unwrap();

    let declared = |source: &str, kind: &str, data| {
        let mut event = Event::new(kind);
        event.source = source.into();
        event.provenance = "declared".into();
        event.coverage = "native".into();
        event.session_id = Some("session".into());
        event.sandbox_id = Some("sandbox".into());
        event.agent_id = Some("runner-agent".into());
        event.data = data;
        event
    };
    let claude = |kind: &str, data| declared("claude-adapter", kind, data);
    let agent = |id: &str| claude("agent.declared", json!({"provider_agent_id": id}));
    let call = |id: &str| {
        claude(
            "delegation.declared",
            json!({"delegation_tool":"Agent","provider_tool_use_id":id,"provider_agent_id":"parent","provider_session_id":"provider-session"}),
        )
    };
    let child_activity = |id: &str, call: &str| {
        claude(
            "message.declared",
            json!({"provider_agent_id":id,"provider_parent_tool_use_id":call,"provider_session_id":"provider-session"}),
        )
    };
    store
        .append_batch(vec![
            claude(
                "agent.declared",
                json!({"provider_agent_id":"parent","provider_parent_agent_id":null}),
            ),
            agent("checkpoint-child"),
            declared(
                "codex-adapter",
                "agent.declared",
                json!({"provider_agent_id":"thread"}),
            ),
            declared(
                "codex-adapter",
                "agent.declared",
                json!({"provider_agent_id":"explicit-child","provider_parent_agent_id":"thread"}),
            ),
        ])
        .unwrap();
    let boundary = store.append(call("boundary-call")).unwrap().seq;
    let mut live_call = call("live-call");
    live_call.data["delegation_tool"] = json!("Task");
    store
        .append_batch(vec![
            agent("cross-boundary-child"),
            child_activity("cross-boundary-child", "boundary-call"),
            agent("checkpoint-child"), // An omitted parent remains ambiguous after restore.
            call("checkpoint-child-call"),
            child_activity("checkpoint-child", "checkpoint-child-call"),
            live_call,
            child_activity("live-child", "live-call"),
            agent("live-child"),
        ])
        .unwrap();

    let lookup = |items: &[Value], id: &str| {
        items
            .iter()
            .find(|item| item["provider_agent_id"] == id)
            .unwrap()
            .clone()
    };
    let before = store.entities(false).unwrap();
    let parent_id = lookup(&before, "parent")["id"].clone();
    for id in ["checkpoint-child", "cross-boundary-child", "live-child"] {
        let child = lookup(&before, id);
        assert_eq!(child["parent_id"], parent_id);
        assert_eq!(child["parent_link"], "inferred");
        assert_eq!(child["field_provenance"]["parent_id"], "inferred");
        assert_eq!(child["provenance"], "declared");
    }
    let explicit = lookup(&before, "explicit-child");
    assert_eq!(explicit["parent_id"], lookup(&before, "thread")["id"]);
    assert_eq!(explicit["field_provenance"]["parent_id"], "declared");

    let archive = dir.path().join("archive-claude");
    let planned = store
        .audit_retention_plan(&policy(), later(), Some(boundary), &BTreeSet::new())
        .unwrap();
    assert_eq!(planned.plan.last_seq, boundary);
    apply(&store, &planned, &archive).unwrap();
    let ledger = store.audit_retention_ledger().unwrap();
    assert_eq!(ledger.archived_through_seq, boundary);
    drop(store);

    let store = reopen(&data).unwrap();
    let pinned = store
        .events(&EventQuery {
            after: Some(boundary - 1),
            before: Some(boundary + 1),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(pinned.len(), 1);
    assert_eq!(pinned[0].seq, boundary);
    assert_eq!(
        pinned[0].event.data["provider_tool_use_id"],
        "boundary-call"
    );
    let after = store.entities(false).unwrap();
    assert_eq!(lookup(&after, "explicit-child"), explicit);
    assert_eq!(lookup(&after, "live-child"), lookup(&before, "live-child"));
    for id in ["checkpoint-child", "cross-boundary-child"] {
        let child = lookup(&after, id);
        assert!(child["parent_id"].is_null());
        assert!(child["parent_link"].is_null());
        assert!(child["parent_unresolved"].is_null());
        assert!(child["field_provenance"].get("parent_id").is_none());
        assert_eq!(child["provenance"], "declared");
    }
    assert_eq!(store.audit_retention_ledger().unwrap(), ledger);
    let verified = store.verify_audit_archives(&[archive]).unwrap();
    assert_eq!(verified.verdict, "verified", "{:?}", verified.failure);
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

#[tokio::test]
async fn batch_at_the_archived_horizon_rolls_back_and_keeps_the_typed_conflict() {
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
    let store = Arc::new(store);
    let router = gateway_router(
        GatewayState::for_tenants(
            vec![TenantGateway {
                tenant_id: TENANT.into(),
                store: Arc::clone(&store),
                runner_token: "r".repeat(48),
                viewer_token: "v".repeat(48),
            }],
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )
        .unwrap(),
    );
    let request = |payload: Value| {
        Request::post("/api/v1/events/batch")
            .header(header::AUTHORIZATION, format!("Bearer {}", "r".repeat(48)))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(payload.to_string()))
            .unwrap()
    };

    let floor = planned.plan.floor_at_last;
    let mut at_horizon = Event::new("command.requested");
    at_horizon.timestamp = floor.clone();
    let mut recent = Event::new("command.requested");
    recent.timestamp = (chrono::DateTime::parse_from_rfc3339(&floor).unwrap()
        + Duration::milliseconds(1))
    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let head = store.head().unwrap();
    let hash = store.integrity().unwrap().head_hash;
    let refused = router
        .clone()
        .oneshot(request(json!({"events": [&recent, &at_horizon]})))
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    let bytes = to_bytes(refused.into_body(), 4096).await.unwrap();
    let refusal: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(refusal["error"]["code"], "event_predates_archive");
    assert_eq!(refusal["error"]["retry_action"], "send_new_event");
    let after = store.integrity().unwrap();
    assert!(after.valid);
    assert_eq!(after.head_hash, hash);
    assert_eq!(store.head().unwrap(), head);
    assert!(
        store
            .events(&EventQuery {
                after: Some(head),
                ..EventQuery::default()
            })
            .unwrap()
            .is_empty()
    );

    let retried = router
        .oneshot(request(json!({"events": [&recent]})))
        .await
        .unwrap();
    assert_eq!(retried.status(), StatusCode::OK);
    let bytes = to_bytes(retried.into_body(), 4096).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap(),
        json!({"stored": [{"id": recent.id, "seq": head + 1}]})
    );
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
