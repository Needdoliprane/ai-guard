//! Software-only regressions. Clock and inspection values are synthetic data,
//! never a current runtime authority or an exec capability.
use super::*;
use crate::{
    broker::discovery::RoleLease,
    control::{
        HostClockMethodV1, HostClockSampleV1, NetworkV1, RestartPolicyV1, RuntimeInspectionV1,
        RuntimeKindV1, SecurityPresetV1,
    },
    control_fs::Directory,
    relay::channel::control::TenantRelayIo,
};
use std::{os::unix::fs::PermissionsExt, sync::Mutex};
use tokio::sync::Semaphore;
use uuid::Uuid;

fn sample(ns: u128) -> HostClockSampleV1 {
    HostClockSampleV1 {
        version: 1,
        method: HostClockMethodV1::LinuxBoottimeV1,
        boot_id: Uuid::from_u128(1).to_string(),
        continuous_ns_decimal: ns.to_string(),
        observed_utc: Some("2026-11-08T00:00:00Z".into()),
    }
}
struct Clock(Mutex<HostClockSampleV1>);
impl HostClock for Clock {
    fn sample(&self) -> crate::broker::clock::ClockResult<HostClockSampleV1> {
        Ok(self.0.lock().unwrap().clone())
    }
}
pub(super) fn arm() -> ArmV1 {
    ArmV1 {
        version: 1,
        operation_id: Uuid::from_u128(2).to_string(),
        worker_id: Uuid::from_u128(3).to_string(),
        attempt_id: Uuid::from_u128(4).to_string(),
        expected_revision: 1,
        intent_hash: "a".repeat(64),
        subject: ArmSubjectV1::Worker,
        inspection: RuntimeInspectionV1 {
            authority_id: Uuid::from_u128(5).to_string(),
            runtime_kind: RuntimeKindV1::Docker,
            container_id: "b".repeat(64),
            stable_name: "software-clock-fixture".into(),
            volume_name: "software-clock-fixture".into(),
            image_id: format!("sha256:{}", "c".repeat(64)),
            spec_hash: "d".repeat(64),
            cpu_millis: 1000,
            memory_bytes: 1048576,
            pids_limit: 32,
            network: NetworkV1::None,
            security_preset: SecurityPresetV1::FixedSandboxV2,
            restart_policy: RestartPolicyV1::No,
        },
        authority_binding_hash: "e".repeat(64),
        configuration_fingerprint_version: 2,
        configuration_fingerprint: "f".repeat(64),
        clock: sample(10),
        timeout_seconds: 1,
        deadline_ns_decimal: "1000000010".into(),
    }
}
fn deadline(clock: Arc<Clock>, cancel: CancelGeneration) -> WorkerRelayDeadline {
    WorkerRelayDeadline {
        arm: arm(),
        clock,
        generation: cancel.current(),
        cancel,
        last_ns: Mutex::new(10),
    }
}

#[test]
fn original_arm_clock_deadline_and_stop_are_not_renewed() {
    let clock = Arc::new(Clock(Mutex::new(sample(900_000_010))));
    let cancel = CancelGeneration::new();
    let owned = deadline(clock.clone(), cancel.clone());
    assert_eq!(owned.remaining().unwrap(), Duration::from_millis(100));
    // UTC changes do not grant any new continuous-clock budget.
    clock.0.lock().unwrap().observed_utc = Some("2030-01-01T00:00:00Z".into());
    assert_eq!(owned.remaining().unwrap(), Duration::from_millis(100));
    clock.0.lock().unwrap().continuous_ns_decimal = "1000000010".into();
    assert_eq!(owned.check(), Err(RelayOwnerError::Expired));
    cancel.invalidate();
    assert_eq!(owned.check(), Err(RelayOwnerError::Cancelled));
    let original = deadline(Arc::new(Clock(Mutex::new(sample(10)))), cancel.clone());
    assert!(original.check().is_ok());
    cancel.invalidate();
    assert_eq!(original.check(), Err(RelayOwnerError::Cancelled));
}

#[test]
fn boot_method_and_any_observed_clock_regression_close_the_deadline() {
    let clock = Arc::new(Clock(Mutex::new(sample(100))));
    let owned = deadline(clock.clone(), CancelGeneration::new());
    assert!(owned.check().is_ok());
    clock.0.lock().unwrap().continuous_ns_decimal = "99".into();
    assert_eq!(owned.check(), Err(RelayOwnerError::Clock));
    *clock.0.lock().unwrap() = sample(100);
    clock.0.lock().unwrap().boot_id = Uuid::from_u128(6).to_string();
    assert_eq!(owned.check(), Err(RelayOwnerError::Clock));
    *clock.0.lock().unwrap() = sample(100);
    clock.0.lock().unwrap().method = HostClockMethodV1::DarwinMachContinuousV1;
    assert_eq!(owned.check(), Err(RelayOwnerError::Clock));
}

fn fact() -> journal::ChannelFact {
    journal::ChannelFact {
        version: 1,
        job_id: Uuid::from_u128(7),
        source_profile_id: "fixture".into(),
        endpoint: RelayEndpointV1 {
            worker_id: Uuid::from_u128(3).to_string(),
            attempt_id: Uuid::from_u128(4).to_string(),
            generation: 1,
        },
        source_head_revision: 1,
        source_head_hash: "a".repeat(64),
        arm_id: arm().operation_id,
        original_clock: sample(10),
        original_deadline_ns_decimal: arm().deadline_ns_decimal,
        phase: journal::AttachmentPhase::AwaitingIssuedWorkload,
        observed_at: "2026-11-08T00:00:00Z".into(),
        error: None,
        admission: None,
        claim: None,
        core: None,
        transition: None,
        local_joined: false,
    }
}

#[test]
fn real_shared_io_retains_exact_facts_across_reopen_and_refuses_partial_files() {
    let temporary = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap())
        .unwrap();
    let root = Directory::open_existing(temporary.path()).unwrap();
    let role = Arc::new(RoleLease::acquire(&root).unwrap());
    let parent = root.persistent_child("tenant", true).unwrap().unwrap();
    let scope = JournalScopeV1::new(
        Uuid::from_u128(8),
        "tenant-a",
        "http://127.0.0.1:9",
        Uuid::from_u128(9),
        "relay_shared",
    )
    .unwrap();
    let literals = vec!["software-private-proof-never-persisted".to_string()];
    let tenant_slots = Arc::new(Semaphore::new(4));
    let global_slots = Arc::new(Semaphore::new(16));
    let tenant_bytes = Arc::new(Semaphore::new(32 * 1024 * 1024));
    let global_bytes = Arc::new(Semaphore::new(128 * 1024 * 1024));
    let open = || {
        TenantRelayIo::open(
            &parent,
            &scope,
            role.clone(),
            literals.clone(),
            tenant_bytes.clone(),
            tenant_slots.clone(),
            global_bytes.clone(),
            global_slots.clone(),
        )
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let io = open().unwrap();
    runtime.block_on(async {
        let initial = io.save_channel(fact(), literals.clone()).await.unwrap();
        let mut repeated = initial.clone();
        repeated.observed_at = "2026-11-08T00:01:00Z".into();
        assert_eq!(
            io.save_channel(repeated, literals.clone())
                .await
                .unwrap()
                .observed_at,
            initial.observed_at
        );
        let mut divergent = initial;
        divergent.original_deadline_ns_decimal = "2000000010".into();
        assert!(io.save_channel(divergent, literals.clone()).await.is_err());
        io.join_pending().await.unwrap();
    });
    drop(io);
    let io = open().unwrap();
    runtime.block_on(async {
        let retained = io.channel(fact().job_id).await.unwrap().unwrap();
        assert_eq!(retained.original_deadline_ns_decimal, "1000000010");
        io.join_pending().await.unwrap();
    });
    drop(io);
    let journal = parent
        .persistent_child("relay_channels_v1", false)
        .unwrap()
        .unwrap();
    assert_eq!(journal.persistent_entries(8).unwrap().len(), 2);
    let partial = journal
        .persistent_create("fact-00000000000000000002.json", b"{")
        .unwrap();
    partial.persistent_sync().unwrap();
    journal.sync().unwrap();
    assert!(open().is_err());
    assert!(
        journal
            .persistent_read("fact-00000000000000000002.json", 16, &[])
            .unwrap()
            .is_some()
    );
}

#[test]
fn channel_journal_refuses_secret_collisions_in_names_before_mutation() {
    let scope = JournalScopeV1::new(
        Uuid::from_u128(8),
        "tenant-a",
        "http://127.0.0.1:9",
        Uuid::from_u128(9),
        "relay_shared",
    )
    .unwrap();
    for collision in ["relay_channels_v1", "scope.json"] {
        let temporary = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap())
            .unwrap();
        let root = Directory::open_existing(temporary.path()).unwrap();
        let _role = RoleLease::acquire(&root).unwrap();
        assert!(journal::ChannelJournal::open(&root, &scope, &[collision.into()]).is_err());
        assert!(
            root.persistent_child("relay_channels_v1", false)
                .unwrap()
                .is_none(),
            "name collision created metadata namespace"
        );
    }
    let temporary = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap())
        .unwrap();
    let root = Directory::open_existing(temporary.path()).unwrap();
    let _role = RoleLease::acquire(&root).unwrap();
    let benign = vec!["software-private-proof-not-persisted".into()];
    let mut records = journal::ChannelJournal::open(&root, &scope, &benign).unwrap();
    let directory = root
        .persistent_child("relay_channels_v1", false)
        .unwrap()
        .unwrap();
    let record_name = "fact-00000000000000000001.json";
    assert!(records.append(fact(), &[record_name.into()]).is_err());
    assert_eq!(
        directory.persistent_entries(8).unwrap().len(),
        1,
        "record-name collision mutated journal"
    );
    records.append(fact(), &benign).unwrap();
    drop(records);
    let before = directory
        .persistent_read(record_name, 16384, &[])
        .unwrap()
        .unwrap();
    assert!(journal::ChannelJournal::open(&root, &scope, &[record_name.into()]).is_err());
    assert_eq!(
        directory
            .persistent_read(record_name, 16384, &[])
            .unwrap()
            .unwrap()
            .bytes(),
        before.bytes()
    );
}
