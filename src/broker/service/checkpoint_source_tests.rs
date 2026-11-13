//! Consumer regressions over the private real Journal/Writer fixture. No
//! authentication, engine, runtime inventory or publication join is inferred.
use super::*;
use crate::{
    broker::{
        journal::cost_measurement::{Measurement as JournalMeasurement, WriterKind},
        lifecycle::BrokerCheckpointV3,
    },
    broker_registry::{BrokerAuthorityV1, BrokerCheckpointSnapshotV1, BrokerRuntimeKindV1},
    control_fs::cost_probe::Measurement as FsMeasurement,
};
use futures_util::FutureExt;

const PROFILE: &str = "cancel_retry_test";

fn authority() -> BrokerAuthorityV1 {
    BrokerAuthorityV1 {
        authority_id: "checkpoint-consumer-fixture".into(),
        runtime_kind: BrokerRuntimeKindV1::Docker,
    }
}

fn snapshot(fixture: &Fixture, profile: &str) -> Option<BrokerCheckpointSnapshotV1> {
    checkpoint_source::confirmed_snapshot(&fixture.service, profile, &authority())
}

async fn seed(fixture: &Fixture) -> AcceptedJobV1 {
    match fixture.writer.status(fixture.operation).await.unwrap() {
        Some(JournalStatusV1::Accepted(job)) => job,
        _ => panic!("the real strict reader must ACK this existing accepted job"),
    }
}

fn assert_head(snapshot: BrokerCheckpointSnapshotV1, operation: Uuid, head: &RevisionRefV1) {
    snapshot.validate().unwrap();
    assert_eq!(snapshot.version, 1);
    assert_eq!(snapshot.authority, authority());
    assert_eq!(snapshot.jobs.len(), 1);
    assert_eq!(snapshot.jobs[0].job_id, operation.to_string());
    assert_eq!(snapshot.jobs[0].revision, head.revision());
    assert_eq!(snapshot.jobs[0].checkpoint_hash, head.hash());
    // This one-entry cache window says nothing about complete journal/runtime
    // coverage. The control plane still labels the report declared_partial.
    assert!(!snapshot.has_more);
}

fn assert_no_io(filesystem: FsMeasurement, journal: JournalMeasurement) {
    let fs = serde_json::to_value(filesystem.finish().unwrap()).unwrap();
    assert_eq!(fs["outstanding"], 0);
    assert!(
        fs["samples"]
            .as_array()
            .unwrap()
            .iter()
            .all(|sample| sample["calls"] == 0),
        "the synchronous consumer performed filesystem/sync work: {fs}"
    );
    let cost = serde_json::to_value(journal.finish().unwrap()).unwrap();
    for counter in [
        "usage_calls",
        "loads_started",
        "accepted_loads_completed",
        "verified_revisions",
        "verified_revision_bytes",
        "caller_outstanding",
        "actor_outstanding",
        "context_conflicts",
        "missed_hooks",
    ] {
        assert_eq!(cost[counter], 0, "consumer performed Full work: {counter}");
    }
    for counters in [
        "inventories",
        "entries_visited",
        "writer_load",
        "writer_status",
    ] {
        assert!(
            cost[counters]
                .as_array()
                .unwrap()
                .iter()
                .all(|value| value.as_u64() == Some(0)),
            "consumer performed inventory or FIFO work: {counters}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cold_uninitialized_and_stop_only_sources_are_unknown_without_creating_a_full_writer() {
    // A fallback that scans the journal, initializes the OnceCell, or upgrades
    // Stops to Full must fail this regression instead of reporting zero jobs.
    for case in ["cold", "uninitialized", "stop_only"] {
        let mut fixture = Fixture::new();
        let selected = if case == "cold" {
            Arc::clone(&fixture.jobs)
        } else {
            let jobs = Arc::new(ProfileJobs::new());
            if case == "stop_only" {
                // A separate real journal avoids reopening the Full fixture's
                // retained exclusive lease or fabricating a Stops capability.
                let root = fixture._temp.path().join("checkpoint-stop-only");
                fs::create_dir(&root).unwrap();
                fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
                let directory = Directory::open_existing(&root).unwrap();
                let journal = Journal::open(
                    &directory,
                    fixture.journal.scope().clone(),
                    &FsExclusions::new(),
                    JournalLimitsV1::default(),
                    ControlSecrets::new(Vec::new()).unwrap(),
                )
                .unwrap();
                let intent = ExecutionIntentV1::from_plan_bytes(
                    &serde_json::to_vec(&plan(fixture.operation)).unwrap(),
                )
                .unwrap();
                let ReserveResult::New(ticket) = journal
                    .reserve(fixture.operation, br#"{"checkpoint_stop_only":true}"#)
                    .unwrap()
                else {
                    panic!("fresh stop-only journal fixture");
                };
                let accepted = journal
                    .accept(
                        ticket,
                        &intent,
                        BindingInputs::None,
                        &CheckpointPayloadV1::initial(&intent).unwrap(),
                    )
                    .unwrap();
                drop(accepted);
                drop(journal);
                let reader = StopRecoveryReaderV1::open_existing(
                    &directory,
                    fixture.journal.scope().clone(),
                    &FsExclusions::new(),
                    JournalLimitsV1::default(),
                    ControlSecrets::new(Vec::new()).unwrap(),
                )
                .unwrap()
                .expect("the fixture journal already exists");
                let report = reader.scan();
                let stops = ProfileWriterCapabilities::from_recovery_report(
                    &report,
                    true,
                    8,
                    Duration::from_secs(10),
                )
                .unwrap();
                assert!(stops.is_stop_only() && stops.full().is_none());
                jobs.writer.set(stops).unwrap();
            }
            fixture
                .service
                .profiles
                .insert(PROFILE.into(), Arc::clone(&jobs));
            jobs
        };
        let pause = fixture.writer.pause_for_test().await.unwrap();
        assert!(pause.is_paused());
        let _exclusive = crate::control_fs::cost_probe::EXCLUSIVE.lock().await;
        let filesystem =
            FsMeasurement::start(&fixture._temp.path().canonicalize().unwrap()).unwrap();
        let journal_cost = JournalMeasurement::start(
            &fixture.journal,
            fixture.writer.cost_context_for_test(),
            fixture.operation,
            WriterKind::Status,
        )
        .unwrap();
        assert!(
            snapshot(&fixture, PROFILE).is_none(),
            "{case}: unknown is not an empty inventory"
        );
        assert_no_io(filesystem, journal_cost);
        match case {
            "uninitialized" => assert!(selected.writer.get().is_none()),
            "stop_only" => assert!(selected.writer.get().unwrap().full().is_none()),
            "cold" => assert!(selected.writer.get().unwrap().full().is_some()),
            _ => unreachable!(),
        }
        drop(_exclusive);
        drop(pause);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn confirmed_snapshot_reads_the_exact_last_ack_head_without_fifo_or_filesystem_work() {
    let fixture = Fixture::new();
    assert!(snapshot(&fixture, PROFILE).is_none());
    let job = seed(&fixture).await;
    let pause = fixture.writer.pause_for_test().await.unwrap();
    assert!(pause.is_paused());
    let _exclusive = crate::control_fs::cost_probe::EXCLUSIVE.lock().await;
    let filesystem = FsMeasurement::start(&fixture._temp.path().canonicalize().unwrap()).unwrap();
    let journal_cost = JournalMeasurement::start(
        &fixture.journal,
        fixture.writer.cost_context_for_test(),
        fixture.operation,
        WriterKind::Status,
    )
    .unwrap();
    assert_head(
        snapshot(&fixture, PROFILE).unwrap(),
        fixture.operation,
        job.head(),
    );
    assert_no_io(filesystem, journal_cost);
    drop(_exclusive);
    drop(pause);

    let next = BrokerCheckpointV3::new(
        BrokerCheckpointV2::new(
            job.checkpoint().clone(),
            SupervisionCheckpointV1::new(
                fixture.journal.scope().control_namespace_uuid(),
                vec![],
                vec![],
            )
            .unwrap(),
        )
        .unwrap(),
        vec![],
    )
    .unwrap();
    let mut barrier = fixture
        .writer
        .advance_reply_barrier_for_test(false)
        .await
        .unwrap();
    let mutation = fixture
        .writer
        .advance_lifecycle(fixture.operation, job.head().clone(), next);
    tokio::pin!(mutation);
    assert!(mutation.as_mut().now_or_never().is_none());
    barrier.entered().await;
    assert_head(
        snapshot(&fixture, PROFILE).unwrap(),
        fixture.operation,
        job.head(),
    );
    drop(barrier);
    let receipt = mutation.await.unwrap();
    assert_ne!(
        receipt.head(),
        job.head(),
        "the real append must advance the head"
    );
    assert_head(
        snapshot(&fixture, PROFILE).unwrap(),
        fixture.operation,
        receipt.head(),
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn confirmed_snapshot_rejects_every_retained_writer_scope_mismatch() {
    // Removing any scope comparison must expose a warm foreign head here.
    for mismatch in ["tenant", "namespace", "installation", "profile", "gateway"] {
        let mut fixture = Fixture::new();
        let job = seed(&fixture).await;
        assert_head(
            snapshot(&fixture, PROFILE).unwrap(),
            fixture.operation,
            job.head(),
        );
        let mut selected_profile = PROFILE;
        let registry = Arc::get_mut(&mut fixture.service.registry).unwrap();
        match mismatch {
            "tenant" => registry.data.profiles[0].tenant_id = "tenant-other-checkpoint-test".into(),
            "namespace" => registry.data.profiles[0].control_namespace_uuid = Uuid::new_v4(),
            "installation" => registry.data.installation_id = Uuid::new_v4(),
            "gateway" => registry.data.profiles[0].gateway = "http://127.0.0.1:8788".into(),
            "profile" => {
                selected_profile = "other_checkpoint_profile";
                registry.data.profiles[0].id = selected_profile.into();
                // Both lookups succeed. The actual retained writer still has
                // its old profile_id, which the consumer must compare.
                let jobs = fixture.service.profiles.remove(PROFILE).unwrap();
                fixture
                    .service
                    .profiles
                    .insert(selected_profile.into(), jobs);
            }
            _ => unreachable!(),
        }
        assert!(fixture.service.registry.profile(selected_profile).is_ok());
        assert!(fixture.service.profiles.contains_key(selected_profile));
        assert!(
            snapshot(&fixture, selected_profile).is_none(),
            "scope mismatch: {mismatch}"
        );
        let heads = fixture.writer.last_confirmed_heads(100).unwrap();
        assert_eq!(heads.entries().len(), 1, "the actual writer remains warm");
        assert_eq!(heads.entries()[0].0, fixture.operation);
        assert_eq!(&heads.entries()[0].1, job.head());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn confirmed_snapshot_returns_unknown_after_fence_stop_incomplete_material_or_cache_loss() {
    for trigger in [
        "fenced",
        "stopping",
        "material",
        "tainted",
        "weak_owner_lost",
    ] {
        let mut fixture = Fixture::new();
        fixture
            .writer
            .bind_status_service(&fixture.service.fenced, &fixture.service.stopping);
        let job = seed(&fixture).await;
        assert_head(
            snapshot(&fixture, PROFILE).unwrap(),
            fixture.operation,
            job.head(),
        );
        match trigger {
            "fenced" => fixture.service.fenced.store(true, Ordering::SeqCst),
            "stopping" => fixture.service.stopping.store(true, Ordering::SeqCst),
            "material" => {
                Arc::get_mut(&mut fixture.service.material)
                    .unwrap()
                    .complete = false
            }
            "tainted" => fixture.writer.invalidate_status_projection(),
            "weak_owner_lost" => {
                // Drop the real Arc bound weakly by the writer. A new service
                // flag is false, so only the retained cache owner guard rejects.
                let old = std::mem::replace(
                    &mut fixture.service.fenced,
                    Arc::new(AtomicBool::new(false)),
                );
                drop(old);
            }
            _ => unreachable!(),
        }
        assert!(
            snapshot(&fixture, PROFILE).is_none(),
            "{trigger}: unavailable must not be Some(empty)"
        );
        if trigger == "material" {
            assert_eq!(
                fixture
                    .writer
                    .last_confirmed_heads(100)
                    .unwrap()
                    .entries()
                    .len(),
                1
            );
        } else {
            assert!(
                fixture.writer.last_confirmed_heads(100).is_err(),
                "{trigger}: cache must close"
            );
            fixture.service.fenced.store(false, Ordering::SeqCst);
            fixture.service.stopping.store(false, Ordering::SeqCst);
            assert!(
                snapshot(&fixture, PROFILE).is_none(),
                "{trigger}: resetting flags cannot revive a closed owner"
            );
        }
    }
}
