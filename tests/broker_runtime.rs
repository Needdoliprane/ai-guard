//! Opt-in G09 evidence: real Docker, fresh schema3 gateway, native broker/helper.
//! Retains the QA root and nonempty enforcer on BOTH success and failure (D5).
//! No model, existing project, engine cleanup, or synthetic runtime responses.
#![cfg(all(unix, any(target_os = "macos", target_os = "linux")))]

#[path = "support/broker_runtime.rs"]
mod harness;

use agentsandbox::{
    broker::{
        clock::{HostClock, NativeHostClock},
        enforcer::{EnforcerClient, SlotPhaseV1},
        local_ipc::EndpointRecordV1,
        service::{
            RequestedExecutionV1, SubmitV1, encode_submit,
            protocol::{BrokerCommandV1, BrokerReplyV1, RecoveryStatusV1, RecoveryTargetKindV1},
            request,
        },
    },
    control::{
        AllocationV1, ArmResultV1, ControlProtocolV1, HostClockSampleV1, QuotaUsageV1,
        WorkerStateV1, WorkerV1,
    },
    control_fs::Directory,
    operations::initialize_config,
    policy::Policy,
    runner::read_token,
};
use anyhow::{Context, Result, bail, ensure};
use harness::{CAPACITY, Evidence, Selection};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, os::unix::process::ExitStatusExt, time::Duration};
use tokio::time::{Instant, sleep, timeout};
use uuid::Uuid;

const PROFILE: &str = "g09";
const NS: u128 = 1_000_000_000;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires explicit Docker program/Unix endpoint/image/rootful opt-in; retains enforcer and QA resources"]
async fn broker_sigkill_preserves_original_deadline_without_second_start() -> Result<()> {
    let selection = Selection::read()?;
    let evidence = Evidence::new()?;
    let outcome = timeout(Duration::from_secs(180), run(&selection, &evidence))
        .await
        .context("broker runtime qualification exceeded the 180-second global bound")
        .and_then(|result| result);
    evidence.write("result.json", &json!({
        "runtime_test":if outcome.is_ok() { "PASS" } else { "FAIL" },
        "cleanup":"NOT_RUN: nonempty guard and exact resources retained; D5 unqualified",
        "scope":"one native control host / selected Docker engine; no Podman, cloud, SIGSTOP or provider qualification"
    }))?;
    outcome
}

#[tokio::test]
#[ignore = "read-only IPC probe of an explicitly selected retained QA guard; writes one new private diagnostic"]
async fn inspect_retained_guard_after_runtime_failure() -> Result<()> {
    let path = std::path::PathBuf::from(
        std::env::var_os("AGENTSANDBOX_TEST_RETAINED_ROOT")
            .context("select retained QA root explicitly")?,
    );
    let canonical = path.canonicalize()?;
    ensure!(
        path.is_absolute() && path == canonical,
        "retained QA root must already be canonical and absolute"
    );
    let root = Directory::open_existing(&canonical)?;
    let arm = std::env::var("AGENTSANDBOX_TEST_RETAINED_ARM")
        .context("select retained Arm explicitly")?;
    ensure!(
        Uuid::parse_str(&arm)?.to_string() == arm,
        "retained Arm must be a canonical UUID"
    );
    let evidence = Evidence { root: canonical };
    let selected = timeout(Duration::from_secs(15), async {
        let guard = observe_existing_guard(&evidence).await?;
        ensure!(guard.status().await?.healthy(), "retained guard reports unhealthy progress");
        let slot = guard.slot_status(&arm).await?;
        ensure!(slot.arm_id() == arm, "retained guard returned another Arm");
        let wire = serde_json::to_value(&slot)?;
        Ok::<_,anyhow::Error>(json!({"phase":slot.phase(),"prepared_once":slot.prepared_once(),"stop_failures":slot.stop_failures(),"clock":wire["clock"],"observed_status":slot.observation().and_then(|o|o.process.status.as_deref()),"cid":slot.observation().and_then(|o|o.id.as_deref())}))
    }).await.context("retained guard diagnostic exceeded fifteen seconds")??;
    root.revalidate()?;
    let output = evidence
        .root
        .join(format!("guard-diagnostic-{}.json", Uuid::new_v4()));
    harness::write_new_private(&output, &serde_json::to_vec_pretty(&selected)?)?;
    eprintln!("{selected}");
    Ok(())
}

async fn run(selection: &Selection, evidence: &Evidence) -> Result<()> {
    evidence.write("stage.json", &json!({"stage":"read_only_preflight"}))?;
    let engine = selection.docker(&["info", "--format", r#"{"id":{{json .ID}},"os":{{json .OSType}},"architecture":{{json .Architecture}},"memory":{{json .MemTotal}},"security_options":{{json .SecurityOptions}}}"#]).await?;
    let rootless = engine["security_options"]
        .as_array()
        .context("engine security options unknown")?
        .iter()
        .any(|value| {
            value
                .as_str()
                .is_some_and(|value| value.contains("rootless"))
        });
    ensure!(
        rootless || selection.rootful,
        "rootful engine requires explicit opt-in"
    );
    let image = selection
        .docker(&[
            "image",
            "inspect",
            "--format",
            r#"{"id":{{json .Id}},"os":{{json .Os}},"architecture":{{json .Architecture}}}"#,
            &selection.image,
        ])
        .await?;
    ensure!(
        image["id"] == selection.image && image["os"] == "linux",
        "selected local image identity/OS mismatch"
    );
    evidence.write("preflight.json", &json!({"engine":engine,"image":image,"program":selection.program,"endpoint":selection.endpoint,"allow_rootful":selection.rootful}))?;

    let config = evidence.root.join("config");
    initialize_config(&config)?;
    let runner = read_token(&config.join("secrets/runner"))?;
    let source = evidence.root.join("source/original.txt");
    harness::write_private(&source, b"broker G09 original source remains unchanged\n")?;
    let original = Sha256::digest(fs::read(&source)?);
    let policy = Policy {
        allowed_argv: vec![vec!["/bin/sleep".into(), "20".into()]],
        ..Policy::default()
    };
    evidence.write("policy.json", &policy)?;
    let (mut gateway, origin) =
        harness::gateway(evidence, &config, &evidence.root.join("policy.json"))?;
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()?;
    let identity: ControlProtocolV1 =
        harness::get(&http, &origin, &runner, "control/protocol").await?;
    ensure!(
        identity.schema_version == 3
            && identity.active
            && identity.role == "runner"
            && identity.tenant_id == "local",
        "new gateway must really be active schema3 / local runner"
    );
    let namespace = identity
        .control_namespace_uuid
        .as_deref()
        .context("new control namespace missing")?;
    Uuid::parse_str(namespace)?;
    evidence.write("control-identity.json", &identity)?;
    let initial_quota: QuotaUsageV1 = harness::get(&http, &origin, &runner, "quota").await?;
    ensure!(
        initial_quota.held_workers == 0,
        "fresh control unexpectedly holds worker quota"
    );
    evidence.write("operator/registry.json", &json!({
        "version":1,"installation_id":Uuid::new_v4(),"root":evidence.root.join("b"),"max_enforcer_slots":CAPACITY,
        "profiles":[{"id":PROFILE,"gateway":origin,"tenant_id":"local","control_namespace_uuid":namespace,
            "config_dir":config,"runner_token_file":config.join("secrets/runner"),"runtime":"docker",
            "runtime_program":selection.program,"runtime_endpoint":selection.endpoint,"image":selection.image,
            "policy_profile":"strict","network":"none","allow_rootful_runtime":selection.rootful,
            "max_timeout_seconds":5,"allow_capture_output":false,"managed_clients":false,
            "source_roots":[evidence.root.join("source")],"export_roots":[evidence.root.join("exports")]}]
    }))?;
    let operation = Uuid::new_v4();
    evidence.write(
        "operation.json",
        &json!({"operation_id":operation,"profile":PROFILE}),
    )?;
    let mut broker = harness::broker(evidence, 1)?;
    wait_broker(evidence, &runner, operation, &mut broker).await?;
    evidence.write("stage.json", &json!({"stage":"submit_real_worker"}))?;
    let payload = encode_submit(&SubmitV1 {
        version: 1,
        operation_id: operation,
        project: evidence.root.join("source"),
        execution: RequestedExecutionV1::Command {
            argv: vec!["/bin/sleep".into(), "20".into()],
        },
        timeout_seconds: Some(5),
        capture_output: false,
    })?;
    match ipc(
        evidence,
        &runner,
        BrokerCommandV1::Submit {
            payload: payload.clone(),
        },
    )
    .await?
    {
        BrokerReplyV1::Accepted {
            replayed: false,
            operation_id,
            ..
        } if operation_id == operation => {}
        BrokerReplyV1::Error { code } => bail!("real submission refused before Running: {code}"),
        _ => bail!("real submission did not produce fresh Accepted"),
    }
    let clock = NativeHostClock::new()?;
    let limit = Instant::now() + Duration::from_secs(45);
    let mut observed_worker: Option<(String, WorkerStateV1)> = None;
    let worker = loop {
        broker.running()?;
        gateway.running()?;
        ensure!(
            Instant::now() < limit,
            "FAIL: worker never observed Running with Ready+prepared_once; inspect retained control/journal/stderr"
        );
        if let Some(worker) = harness::only_worker(&http, &origin, &runner).await? {
            if let Some((previous, _)) = &observed_worker {
                ensure!(
                    previous == &worker.worker_id,
                    "worker identity changed while awaiting Running"
                );
            }
            let observed = (worker.worker_id.clone(), worker.state);
            if observed_worker.as_ref() != Some(&observed) {
                evidence.write("worker.json", &worker)?;
                observed_worker = Some(observed);
            }
            if worker.state == WorkerStateV1::Running {
                break worker;
            }
            ensure!(
                !matches!(
                    worker.state,
                    WorkerStateV1::Terminal | WorkerStateV1::Cancelled
                ),
                "worker became terminal before Running was observed"
            );
        } else {
            ensure!(
                observed_worker.is_none(),
                "previously observed worker disappeared"
            );
        }
        sleep(Duration::from_millis(200)).await;
    };
    // Do not queue full journal recovery reads ahead of this worker's durable
    // Arm/claim writes. The tenant-scoped gateway list above is the cheap poll.
    let recovery_before = recovery(evidence, &runner, operation).await?;
    let targets: Vec<_> = recovery_before
        .known_arms
        .iter()
        .filter(|target| matches!(target.subject, RecoveryTargetKindV1::Worker))
        .collect();
    ensure!(
        targets.len() == 1,
        "Running lacks one exact original worker Arm"
    );
    let target = targets[0];
    ensure!(
        target.worker_id == worker.worker_id && target.attempt_id == worker.attempt.attempt_id,
        "recovered Arm does not bind the observed worker/attempt"
    );
    let guard = observe_existing_guard(evidence).await?;
    let status = guard.status().await?;
    let epoch = status.epoch();
    let slot = guard.slot_status(&target.arm_id).await?;
    ensure!(
        status.healthy() && slot.phase() == SlotPhaseV1::Ready && slot.prepared_once(),
        "Running worker lacks actual Ready+prepared_once stop guard"
    );
    let arm: ArmResultV1 = harness::get(
        &http,
        &origin,
        &runner,
        &format!("workers/{}/arms/{}", worker.worker_id, target.arm_id),
    )
    .await?;
    ensure!(
        serde_json::to_value(&arm.arm.clock)? == serde_json::to_value(&target.original_clock)?
            && arm.arm.deadline_ns_decimal == target.original_deadline_ns_decimal
            && arm.arm.timeout_seconds == 5,
        "recovery did not preserve the original native Arm"
    );
    evidence.write("guard-before.json", &json!({"status":status,"slot":slot}))?;
    let cid = worker
        .attempt
        .container_id
        .as_deref()
        .context("CID disappeared")?;
    let running = selection.inspect(cid).await?;
    ensure!(
        running["running"] == true
            && running["id"] == cid
            && running["image"] == selection.image
            && running["network"] == "none"
            && running["privileged"] == false
            && running["restarts"] == 0,
        "real worker sandbox/image/CID differs from its contract"
    );
    check_mounts(&running, &worker.attempt.volume_name)?;
    ensure!(
        worker.quota_held
            && worker.attempt.start_operation_id.is_some()
            && worker.attempt.start_emitter_id.is_some(),
        "Running lacks centrally persisted claim/quota"
    );
    let central_started = chrono::DateTime::parse_from_rfc3339(
        worker
            .observed_started_at
            .as_deref()
            .context("central Running StartedAt missing")?,
    )?;
    let docker_started = chrono::DateTime::parse_from_rfc3339(
        running["started_at"]
            .as_str()
            .context("Docker StartedAt missing")?,
    )?;
    ensure!(
        central_started == docker_started,
        "central and Docker StartedAt disagree"
    );
    let audit_before = harness::audit(&http, &origin, &runner).await?;
    let emissions = harness::worker_emissions(&audit_before, &worker.worker_id)?;
    ensure!(
        emissions.0[0]["data"]["input"]["request"]["operation_id"] == arm.arm.operation_id
            && emissions.1[0]["data"]["input"]["request"]["operation_id"].as_str()
                == worker.attempt.start_operation_id.as_deref(),
        "audit claim/Arm do not bind the real worker"
    );
    evidence.write("audit-before.json", &audit_before)?;
    evidence.write("recovery-before.json", &recovery_before)?;
    evidence.write("docker-running.json", &running)?;
    let deadline = arm.arm.deadline_ns_decimal.parse::<u128>()?;
    let now = clock.sample()?;
    ensure!(
        now.boot_id == arm.arm.clock.boot_id
            && now.method == arm.arm.clock.method
            && now.continuous_ns()? + 150_000_000 < deadline,
        "Running oracle was too late to kill broker before its original deadline"
    );
    evidence.write("stage.json", &json!({"stage":"kill_owned_broker_only","kill_clock":now,"original_deadline_ns":deadline.to_string()}))?;
    let killed_pid = broker.id();
    let killed_status = broker.kill_owned()?;
    let killed = clock.sample()?;
    evidence.write("broker-killed.json", &json!({"clock_before":now,"clock_after_reap":killed,"original_deadline_ns":deadline.to_string(),"owned_pid":killed_pid,"status":killed_status.to_string(),"signal":killed_status.signal()}))?;
    ensure!(
        killed_status.signal() == Some(libc::SIGKILL),
        "owned broker did not exit from the requested SIGKILL; fault not established"
    );
    ensure!(
        killed.continuous_ns()? < deadline,
        "broker death/reap occurred after the original deadline; autonomous-stop fault was not established"
    );
    let terminal = loop {
        gateway.running()?;
        let observed = selection.inspect(cid).await?; // Absence/inspect error is a failure, never terminal.
        let sample = clock.sample()?;
        let observed_ns = sample.continuous_ns()?;
        ensure!(
            observed_ns <= deadline + 3 * NS,
            "FAIL: terminal not observed by original deadline + 3s; sleep20 natural completion cannot pass"
        );
        ensure!(
            observed["id"] == running["id"]
                && observed["started_at"] == running["started_at"]
                && observed["restarts"] == 0,
            "worker was replaced/restarted during broker outage"
        );
        if observed["running"] == false
            && matches!(observed["status"].as_str(), Some("exited" | "dead"))
        {
            ensure!(
                observed_ns >= deadline,
                "worker became terminal before its Arm deadline; autonomous deadline stop was not established"
            );
            // The product sends docker kill (default SIGKILL). An OOM or a
            // successful natural sleep exit is not this enforcement result.
            // https://docs.docker.com/reference/cli/docker/container/kill/
            ensure!(
                observed["oom_killed"] == false && observed["exit_code"] == 137,
                "terminal was not the expected non-OOM SIGKILL outcome"
            );
            let finished = chrono::DateTime::parse_from_rfc3339(
                observed["finished_at"]
                    .as_str()
                    .context("terminal FinishedAt missing")?,
            )?;
            let actual_run_ns = finished
                .signed_duration_since(docker_started)
                .num_nanoseconds()
                .context("terminal process duration is not representable")?;
            ensure!(
                actual_run_ns > 0 && actual_run_ns < 20_000_000_000,
                "sleep20 natural completion or invalid process timestamps cannot qualify deadline stop"
            );
            evidence.write("deadline-observation.json", &json!({"original_arm":arm,"observed_clock":sample,"observation_lateness_ns":observed_ns.saturating_sub(deadline).to_string(),"observation_tolerance_ns":(3*NS).to_string(),"observed_process_duration_ns":actual_run_ns.to_string(),"physical_deadline_exactly_5s_qualified":false,"docker":observed}))?;
            break observed;
        }
        sleep(Duration::from_millis(50)).await;
    };
    let stop_status = loop {
        let slot = guard.slot_status(&arm.arm.operation_id).await?;
        if slot.phase() == SlotPhaseV1::Quarantined
            && let Some(observation) = slot.observation()
        {
            ensure!(
                observation.id.as_deref() == Some(cid)
                    && observation.image_id.as_deref() == Some(selection.image.as_str())
                    && observation.process.running == Some(false)
                    && matches!(
                        observation.process.status.as_deref(),
                        Some("exited" | "dead")
                    )
                    && observation.process.started_at == Some(docker_started)
                    && observation.process.finished_at
                        == Some(chrono::DateTime::parse_from_rfc3339(
                            terminal["finished_at"]
                                .as_str()
                                .context("Docker FinishedAt missing")?
                        )?),
                "quarantined guard snapshot does not match the exact Docker terminal CID/StartedAt/FinishedAt"
            );
            let wire = serde_json::to_value(&slot)?;
            let actor_clock: HostClockSampleV1 = serde_json::from_value(wire["clock"].clone())?;
            ensure!(
                actor_clock.boot_id == arm.arm.clock.boot_id
                    && actor_clock.method == arm.arm.clock.method
                    && actor_clock.continuous_ns()? >= deadline,
                "guard terminal snapshot lacks an original-boot deadline clock"
            );
            break slot;
        }
        ensure!(
            clock.sample()?.continuous_ns()? <= deadline + 3 * NS,
            "guard failed to retain an exact terminal snapshot by deadline + 3s"
        );
        sleep(Duration::from_millis(25)).await;
    };
    ensure!(
        guard.status().await?.epoch() == epoch && stop_status.prepared_once(),
        "original enforcer epoch/consumed permit changed after broker death"
    );
    evidence.write("guard-after-stop.json", &stop_status)?;

    evidence.write(
        "stage.json",
        &json!({"stage":"restart_same_broker_and_registry"}),
    )?;
    let mut restarted = harness::broker(evidence, 2)?;
    wait_broker(evidence, &runner, operation, &mut restarted).await?;
    wait_terminal(
        evidence,
        &runner,
        &http,
        &origin,
        operation,
        &worker.worker_id,
    )
    .await?;
    let after = recovery(evidence, &runner, operation).await?;
    let worker_arms: Vec<_> = after
        .known_arms
        .iter()
        .filter(|target| matches!(target.subject, RecoveryTargetKindV1::Worker))
        .collect();
    ensure!(
        worker_arms.len() == 1
            && worker_arms[0].arm_id == arm.arm.operation_id
            && worker_arms[0].original_deadline_ns_decimal == arm.arm.deadline_ns_decimal
            && serde_json::to_value(&worker_arms[0].original_clock)?
                == serde_json::to_value(&arm.arm.clock)?,
        "restart created/rearmed worker or extended its deadline"
    );
    let recovered: WorkerV1 = harness::get(
        &http,
        &origin,
        &runner,
        &format!("workers/{}", worker.worker_id),
    )
    .await?;
    ensure!(
        recovered.attempt == worker.attempt
            && recovered.observed_started_at == worker.observed_started_at,
        "worker CID/attempt/claim/StartedAt changed on recovery"
    );
    let inspected_after = selection.inspect(cid).await?;
    ensure!(
        inspected_after["started_at"] == running["started_at"]
            && inspected_after["finished_at"] == terminal["finished_at"]
            && inspected_after["running"] == false
            && inspected_after["restarts"] == 0,
        "original Docker instance changed during reconciliation"
    );
    let usage_before: Value = harness::get(&http, &origin, &runner, "usage?limit=100").await?;
    let allocation = check_usage(&usage_before, &recovered, cid)?;
    let quota: QuotaUsageV1 = harness::get(&http, &origin, &runner, "quota").await?;
    ensure!(
        quota.held_workers == 0
            && quota.held_cpu_millis == 0
            && quota.held_memory_bytes == 0
            && !recovered.quota_held,
        "terminal ACK did not release the real quota"
    );
    // Exercise the already-durable Submit identity, then an explicit real Resume.
    ensure!(
        matches!(
            ipc(evidence, &runner, BrokerCommandV1::Submit { payload }).await?,
            BrokerReplyV1::Accepted { replayed: true, .. }
        ),
        "same Submit was not replayed exactly"
    );
    let command_id = Uuid::new_v4();
    match ipc(
        evidence,
        &runner,
        BrokerCommandV1::Resume {
            operation_id: operation,
            command_id,
        },
    )
    .await?
    {
        BrokerReplyV1::CommandAcknowledged {
            command_id: ack, ..
        } if ack == command_id => {}
        BrokerReplyV1::Error { code } => bail!("real terminal Resume refused: {code}"),
        _ => bail!("terminal Resume lacked durable ACK"),
    }
    wait_terminal(
        evidence,
        &runner,
        &http,
        &origin,
        operation,
        &worker.worker_id,
    )
    .await?;
    let usage_after: Value = harness::get(&http, &origin, &runner, "usage?limit=100").await?;
    ensure!(
        usage_after == usage_before,
        "idempotent recovery/replay duplicated or changed allocation"
    );
    let audit_after = harness::audit(&http, &origin, &runner).await?;
    ensure!(
        harness::worker_emissions(&audit_after, &worker.worker_id)? == emissions,
        "second worker Arm/claim appeared after restart/replay"
    );
    ensure!(
        guard.status().await?.epoch() == epoch,
        "enforcer epoch changed during recovery"
    );
    let final_inspect = selection.inspect(cid).await?;
    ensure!(
        final_inspect == inspected_after,
        "original worker changed after terminal replay"
    );
    let integrity: Value = harness::get(&http, &origin, &runner, "integrity").await?;
    ensure!(
        integrity["valid"] == true
            && integrity["event_count"]
                .as_u64()
                .is_some_and(|count| count > 0),
        "actual audit integrity is not valid/nonempty"
    );
    ensure!(
        Sha256::digest(fs::read(&source)?) == original
            && fs::read_dir(evidence.root.join("source"))?.count() == 1,
        "original source changed"
    );
    evidence.write("audit-after.json", &audit_after)?;
    evidence.write("recovery-after.json", &after)?;
    evidence.write("worker-after.json", &recovered)?;
    evidence.write("reconciled.json", &json!({"quota":quota,"allocation":allocation,"usage":usage_after,"integrity":integrity,"source_sha256":hex::encode(original),"original_cid":cid,"volume":worker.attempt.volume_name,"enforcer_epoch":epoch,"worker_arm":arm.arm.operation_id,"deadline_ns":deadline.to_string(),"helper_arms":after.known_arms.iter().filter(|target|matches!(target.subject,RecoveryTargetKindV1::Helper{..})).count(),"cleanup":"NOT_RUN: retain guard, root and exact resources; no disarm inferred"}))?;
    let _ = restarted.kill_owned()?;
    let _ = gateway.kill_owned()?;
    Ok(())
}

async fn ipc(evidence: &Evidence, runner: &str, command: BrokerCommandV1) -> Result<BrokerReplyV1> {
    timeout(
        Duration::from_secs(15),
        request(&evidence.root.join("b"), PROFILE, runner, command),
    )
    .await
    .context("public broker IPC exceeded 15s")?
}
async fn recovery(
    evidence: &Evidence,
    runner: &str,
    operation_id: Uuid,
) -> Result<RecoveryStatusV1> {
    match ipc(
        evidence,
        runner,
        BrokerCommandV1::RecoveryStatus { operation_id },
    )
    .await?
    {
        BrokerReplyV1::RecoveryStatus { status } => Ok(status),
        BrokerReplyV1::Error { code } => bail!("RecoveryStatus refused: {code}"),
        _ => bail!("unexpected recovery reply"),
    }
}
async fn wait_broker(
    evidence: &Evidence,
    runner: &str,
    operation_id: Uuid,
    child: &mut harness::OwnedProcess,
) -> Result<()> {
    let limit = Instant::now() + Duration::from_secs(20);
    loop {
        child.running()?;
        if matches!(
            ipc(evidence, runner, BrokerCommandV1::Status { operation_id }).await,
            Ok(BrokerReplyV1::Status { .. })
        ) {
            return Ok(());
        }
        ensure!(
            Instant::now() < limit,
            "broker did not become available; inspect private stderr"
        );
        sleep(Duration::from_millis(50)).await;
    }
}
async fn observe_existing_guard(evidence: &Evidence) -> Result<EnforcerClient> {
    let root = Directory::open_existing(&evidence.root.join("b/enforcer"))?;
    let auth = root.read_private_file("auth", 32)?;
    ensure!(
        auth.bytes().len() == 32,
        "existing guard capability missing"
    );
    let announcements = Directory::open_existing(&root.path().join("announcements"))?;
    ensure!(
        fs::read_dir(announcements.path())?.count() == 1,
        "fresh test must have exactly one existing enforcer announcement"
    );
    let record = announcements.read_private_file("e-0001.json", 4096)?;
    let wire: Value = serde_json::from_slice(record.bytes())?;
    let endpoint: EndpointRecordV1 = serde_json::from_value(wire["endpoint"].clone())?;
    // Never use the real executable here: this observer cannot spawn a replacement.
    let absent = evidence.root.join("observer-must-not-spawn");
    ensure!(!absent.exists(), "observer sentinel unexpectedly exists");
    let client = EnforcerClient::connect_or_spawn(&root, &absent, CAPACITY).await?;
    ensure!(
        client.status().await?.epoch() == endpoint.epoch(),
        "observed enforcer is not the originally announced child"
    );
    auth.revalidate()?;
    record.revalidate()?;
    Ok(client)
}
async fn wait_terminal(
    evidence: &Evidence,
    runner: &str,
    http: &reqwest::Client,
    origin: &str,
    operation_id: Uuid,
    worker_id: &str,
) -> Result<()> {
    let limit = Instant::now() + Duration::from_secs(35);
    loop {
        let worker: WorkerV1 =
            harness::get(http, origin, runner, &format!("workers/{worker_id}")).await?;
        if let BrokerReplyV1::Status { status } =
            ipc(evidence, runner, BrokerCommandV1::Status { operation_id }).await?
            && worker.state == WorkerStateV1::Terminal
            && !worker.quota_held
            && !status.executor_task_running
        {
            ensure!(
                status.executor_error.is_none(),
                "reconciliation executor failed: {:?}",
                status.executor_error
            );
            return Ok(());
        }
        ensure!(
            Instant::now() < limit,
            "terminal central ACK/reconciliation did not complete"
        );
        sleep(Duration::from_millis(100)).await;
    }
}
fn check_usage(page: &Value, worker: &WorkerV1, cid: &str) -> Result<AllocationV1> {
    ensure!(page["next_cursor"].is_null(), "usage evidence truncated");
    let items = page["items"].as_array().context("usage items missing")?;
    ensure!(
        items.len() == 1,
        "fresh one-worker control must have exactly one allocation"
    );
    let allocation: AllocationV1 = serde_json::from_value(items[0].clone())?;
    ensure!(
        allocation.worker_id == worker.worker_id
            && allocation.attempt_id == worker.attempt.attempt_id
            && allocation.source.container_id == cid
            && !allocation.billable
            && allocation.duration_ns.parse::<u128>()? > 0,
        "allocation lacks actual immutable worker source/duration"
    );
    Ok(allocation)
}

fn check_mounts(inspection: &Value, volume: &str) -> Result<()> {
    ensure!(
        inspection["bind_count"] == 0,
        "worker has configured bind mounts"
    );
    let mounts = inspection["mounts"]
        .as_array()
        .context("Docker mounts unknown")?;
    ensure!(
        (1..=3).contains(&mounts.len()),
        "unexpected worker mount count"
    );
    let mut workspace = 0;
    let mut destinations = std::collections::BTreeSet::new();
    // ContainerSpec::create_args configures exactly this named volume and these
    // two tmpfs destinations; managed::validate_configuration permits Docker to
    // omit --tmpfs entries from Mounts. No Source/host path is collected.
    for mount in mounts {
        let destination = mount["destination"]
            .as_str()
            .context("mount destination unknown")?;
        ensure!(
            mount["rw"] == true && destinations.insert(destination),
            "worker mount is read-only or duplicated"
        );
        match (mount["type"].as_str(), destination) {
            (Some("volume"), "/workspace")
                if mount["name"] == volume && mount["driver"] == "local" =>
            {
                workspace += 1
            }
            (Some("tmpfs"), "/tmp" | "/home/agent")
                if (mount["name"].is_null() || mount["name"] == "")
                    && (mount["driver"].is_null() || mount["driver"] == "") => {}
            _ => bail!("worker has a bind, plugin or unexpected mount"),
        }
    }
    ensure!(
        workspace == 1,
        "worker lacks its exact exclusive workspace volume"
    );
    Ok(())
}
