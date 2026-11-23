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
            protocol::{
                BrokerCommandV1, BrokerErrorCodeV1, BrokerReplyV1, JobResultOutcomeV1, JobResultV1,
                RecoveryDataSourceV1, RecoveryRuntimeEvidenceV1, RecoveryStatusV1,
                RecoveryTargetKindV1, RecoveryTargetV1, TerminalJobResultV1,
            },
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
use chrono::{DateTime, FixedOffset};
use harness::{CAPACITY, Evidence, Selection};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::process::ExitStatusExt,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::AsyncReadExt,
    process::Command,
    time::{Instant, sleep, sleep_until, timeout, timeout_at},
};
use uuid::Uuid;

const PROFILE: &str = "g09";
const NS: u128 = 1_000_000_000;
const OBSERVATION_GRACE_NS: u128 = 3 * NS;

#[derive(Clone, Copy)]
enum LostProcess {
    Broker { scenario_end: Instant },
    LegacyRecoveryStatus,
    RunCli,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum LegacyRecoveryStage {
    NotReached,
    SpawnFailed,
    Spawned,
    ChildExited,
    ChildWaitFailed,
    AvailabilityTimedOut,
    StatusRefused,
    UnexpectedReply,
    OriginalArmMismatch,
    RecoveryAttested,
}

#[derive(serde::Serialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
enum RunClientRecoveryStage {
    NotReached,
    ReapClock,
    BrokerLiveness,
    WorkerRead,
    StatusRead,
    TerminalWait,
    Executor {
        central_cleanup_sample: CentralCleanupSample,
        executor_error: BrokerErrorCodeV1,
    },
    ResultRead,
    ResultOutcome,
    TerminalCorrelation,
    PostTerminalChecks,
    ControlShutdown,
    Complete,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum CentralCleanupSample {
    NotObserved,
    ReportedUnconfirmed,
    ReportedRemoved,
}

#[derive(serde::Serialize)]
struct RunClientRecoveryReceipt {
    state: &'static str,
    #[serde(flatten)]
    stage: RunClientRecoveryStage,
}

#[test]
fn batch_failure_marker_keeps_only_validated_metadata() {
    let private = "synthetic-private-audit-body-do-not-persist";
    let page = json!({"items":[{
        "id":"11111111-1111-4111-8111-111111111111", "seq":17,
        "kind":"broker.batch.failure", "source":"runner",
        "command":{"argv":[private]},
        "data":{"version":1,"phase":"worker","runtime_effect":"unknown",
            "broker_step":"readiness_before_claim",
            "enforcer":{"stage":"before_claim","private":private},
            "runtime":null,"extra":private}
    }],"next_cursor":17});
    let marker = harness::batch_failure_marker(&page)
        .unwrap()
        .expect("a real coded failure must be observed");
    assert_eq!(marker["audit_seq"], 17);
    assert_eq!(marker["broker_step"], "readiness_before_claim");
    assert_eq!(marker["enforcer_stage"], "before_claim");
    assert_eq!(marker["runtime_effect"], "unknown");
    assert!(!serde_json::to_string(&marker).unwrap().contains(private));
    assert_eq!(marker.as_object().unwrap().len(), 8);
}

#[test]
fn batch_failure_marker_empty_page_is_not_a_failure() {
    assert!(
        harness::batch_failure_marker(&json!({"items":[],"next_cursor":null}))
            .unwrap()
            .is_none()
    );
}

#[test]
fn batch_failure_marker_accepts_observed_failures_without_claiming_effect() {
    let private = "synthetic-private-unclassified-failure-do-not-persist";
    for stage in [
        "activity_spool",
        "failure_observed",
        "ownership_commit",
        "ownership_reload",
        "created_inspection",
        "created_exchange",
    ] {
        let page = json!({"items":[{
            "id":"11111111-1111-4111-8111-111111111111", "seq":17,
            "kind":"broker.batch.failure", "source":"runner",
            "data":{"version":1,"phase":"worker","runtime_effect":"unknown",
                "broker_step":stage,"enforcer":null,"runtime":null,
                "error":private},"private":private
        }],"next_cursor":17});
        let marker = harness::batch_failure_marker(&page)
            .expect("known observed failure stage must parse")
            .expect("an observed failure must remain visible");
        assert_eq!(marker["broker_step"], stage);
        assert_eq!(marker["runtime_effect"], "unknown");
        assert!(marker["enforcer_stage"].is_null());
        assert!(marker["runtime_stage"].is_null());
        assert_eq!(marker.as_object().unwrap().len(), 8);
        assert!(!serde_json::to_string(&marker).unwrap().contains(private));
    }
}

#[test]
fn batch_failure_marker_rejects_invalid_public_metadata() {
    let private = "synthetic-private-invalid-label-do-not-print";
    let valid = json!({"items":[{
        "id":"11111111-1111-4111-8111-111111111111", "seq":1,
        "kind":"broker.batch.failure", "source":"runner",
        "data":{"version":1,"phase":"worker","runtime_effect":"unknown",
            "broker_step":"registration","enforcer":null,"runtime":null}
    }]});
    for pointer in [
        "/items/0/id",
        "/items/0/kind",
        "/items/0/source",
        "/items/0/data/phase",
        "/items/0/data/broker_step",
        "/items/0/data/enforcer",
        "/items/0/data/runtime",
    ] {
        let mut invalid = valid.clone();
        *invalid.pointer_mut(pointer).unwrap() = json!(private);
        let error = harness::batch_failure_marker(&invalid).unwrap_err();
        assert!(!error.to_string().contains(private));
    }
    for (pointer, value) in [
        ("/items/0/seq", json!(0)),
        ("/items/0/data/version", json!(2)),
        ("/items/0/data/runtime_effect", json!("observed")),
        ("/items/0/data/broker_step", Value::Null),
    ] {
        let mut invalid = valid.clone();
        *invalid.pointer_mut(pointer).unwrap() = value;
        assert!(harness::batch_failure_marker(&invalid).is_err());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires explicit Docker program/Unix endpoint/image/rootful opt-in; retains enforcer and QA resources"]
async fn broker_sigkill_preserves_original_deadline_without_second_start() -> Result<()> {
    let selection = Selection::read()?;
    let evidence = Evidence::new()?;
    let scenario_end = Instant::now() + Duration::from_secs(600);
    let outcome = timeout_at(
        scenario_end,
        run(
            &selection,
            &evidence,
            LostProcess::Broker { scenario_end },
            &mut RunClientRecoveryStage::NotReached,
        ),
    )
    .await
    .context("broker runtime qualification exceeded the 600-second global bound")
    .and_then(|result| result);
    evidence.write("result.json", &json!({
        "runtime_test":if outcome.is_ok() { "PASS" } else { "FAIL" },
        "worker_event_proof":if outcome.is_ok() { "observed_live_and_history_agree" } else { "not_qualified" },
        "docker_event_continuity":"unknown",
        "docker_event_scope":"daemon-published create/start effects for the original session; rejected requests and silently omitted daemon events are unobservable",
        "native_start":selection.native_start,
        "timeout_seconds":selection.timeout_seconds,
        "cleanup":"NOT_RUN: nonempty guard and exact resources retained; D5 unqualified",
        "scope":"one native control host / selected Docker engine; no Podman, cloud, SIGSTOP or provider qualification"
    }))?;
    outcome
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires explicit Docker program/Unix endpoint/image/rootful opt-in; retains the Legacy enforcer and QA state"]
async fn broker_sigkill_legacy_recovery_status_attests_original_worker_arm() -> Result<()> {
    let selection = Selection::read()?;
    let evidence = Evidence::new()?;
    evidence.write(
        "legacy-recovery-stage.json",
        &LegacyRecoveryStage::NotReached,
    )?;
    let outcome = timeout(
        Duration::from_secs(600),
        run(
            &selection,
            &evidence,
            LostProcess::LegacyRecoveryStatus,
            &mut RunClientRecoveryStage::NotReached,
        ),
    )
    .await
    .context("Legacy recovery status fixture exceeded the 600-second global bound")
    .and_then(|result| result);
    evidence.write(
        "result.json",
        &json!({
            "runtime_test": if outcome.is_ok() {"PASS"} else {"FAIL"},
            "scope": "legacy_recovery_status_original_worker_arm",
            "result_after_restart": "NOT_RUN",
            "central_quota_after_restart": "NOT_RUN",
            "retained_fixture_state": true
        }),
    )?;
    outcome
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires explicit Docker program/Unix endpoint/image/rootful opt-in; retains the enforcer and QA evidence"]
async fn broker_run_cli_sigkill_preserves_original_deadline_without_second_start() -> Result<()> {
    let selection = Selection::read()?;
    let evidence = Evidence::new()?;
    let mut stage = RunClientRecoveryStage::NotReached;
    let run_outcome = timeout(
        Duration::from_secs(600),
        run(&selection, &evidence, LostProcess::RunCli, &mut stage),
    )
    .await;
    let state = match &run_outcome {
        Ok(Ok(())) => "passed",
        Ok(Err(_)) => "failed",
        Err(_) => "timed_out",
    };
    let outcome = run_outcome
        .context("broker-routed CLI qualification exceeded the 600-second global bound")
        .and_then(|result| result);
    evidence.write("result.json", &json!({
        "runtime_test":if outcome.is_ok() { "PASS" } else { "FAIL" },
        "fault":"owned_run_cli_sigkill",
        "run_client_recovery":RunClientRecoveryReceipt { state, stage },
        "requested_broker_client_source":"published_selection",
        "broker_selection_input":"environment",
        "native_start":selection.native_start,
        "timeout_seconds":selection.timeout_seconds,
        "cleanup":"normal audited worker removal required; retain enforcer, workspace and evidence; D5 unqualified",
        "scope":"one native control host / selected Docker engine and the original live broker; no Podman, cloud, SIGSTOP or provider qualification"
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

async fn run(
    selection: &Selection,
    evidence: &Evidence,
    lost: LostProcess,
    run_client_stage: &mut RunClientRecoveryStage,
) -> Result<()> {
    let timeout_seconds = selection.timeout_seconds;
    let sleep_seconds = timeout_seconds + 15;
    let argv = vec!["/bin/sleep".into(), sleep_seconds.to_string()];
    evidence.write("stage.json", &json!({"stage":"read_only_preflight"}))?;
    let mut binary_hash = Sha256::new();
    std::io::copy(&mut fs::File::open(&selection.binary)?, &mut binary_hash)
        .context("hash selected broker and gateway executable")?;
    evidence.write(
        "binary.json",
        &json!({
            "path":selection.binary,"sha256":format!("{:x}", binary_hash.finalize())
        }),
    )?;
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
    evidence.write("preflight.json", &json!({"engine":engine,"image":image,"program":selection.program,"endpoint":selection.endpoint,"allow_rootful":selection.rootful,
        "native_start":selection.native_start,"timeout_seconds":timeout_seconds,"sleep_seconds":sleep_seconds,"observation_tolerance_ns":OBSERVATION_GRACE_NS.to_string()}))?;

    let config = evidence.root.join("config");
    initialize_config(&config)?;
    let runner = read_token(&config.join("secrets/runner"))?;
    let source = evidence.root.join("source/original.txt");
    harness::write_private(&source, b"broker G09 original source remains unchanged\n")?;
    let original = Sha256::digest(fs::read(&source)?);
    let policy = Policy {
        allowed_argv: vec![argv.clone()],
        ..Policy::default()
    };
    evidence.write("policy.json", &policy)?;
    let (mut gateway, origin) = harness::gateway(
        selection,
        evidence,
        &config,
        &evidence.root.join("policy.json"),
    )?;
    if matches!(lost, LostProcess::RunCli) {
        gateway.require_graceful_shutdown();
    }
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
    let mut registry = json!({
        "version":1,"installation_id":Uuid::new_v4(),"root":evidence.root.join("b"),"max_enforcer_slots":CAPACITY,
        "profiles":[{"id":PROFILE,"gateway":origin,"tenant_id":"local","control_namespace_uuid":namespace,
            "config_dir":config,"runner_token_file":config.join("secrets/runner"),"runtime":"docker",
            "runtime_program":selection.program,"runtime_endpoint":selection.endpoint,"image":selection.image,
            "policy_profile":"strict","network":"none","allow_rootful_runtime":selection.rootful,
            "max_timeout_seconds":timeout_seconds,"allow_capture_output":false,"managed_clients":false,
            "source_roots":[evidence.root.join("source")],"export_roots":[evidence.root.join("exports")]}]
    });
    if selection.native_start {
        registry["version"] = json!(2);
        registry["profiles"][0]["prearmed_start_profile"] =
            json!({"version":1,"mode":"docker_minimal"});
    }
    evidence.write("operator/registry.json", &registry)?;
    let operation = Uuid::new_v4();
    evidence.write(
        "operation.json",
        &json!({"operation_id":operation,"profile":PROFILE}),
    )?;
    let mut broker = harness::broker(selection, evidence, 1)?;
    if matches!(lost, LostProcess::RunCli) {
        broker.require_graceful_shutdown();
    }
    wait_broker(evidence, &runner, operation, &mut broker).await?;
    let daemon_sample_before = Instant::now();
    let docker_events_since = selection.docker_clock().await?;
    evidence.write("stage.json", &json!({"stage":"submit_real_worker"}))?;
    let payload = encode_submit(&SubmitV1 {
        version: 1,
        operation_id: operation,
        project: evidence.root.join("source"),
        execution: RequestedExecutionV1::Command { argv: argv.clone() },
        timeout_seconds: Some(timeout_seconds),
        capture_output: false,
    })?;
    let (mut cli, live_events) = match lost {
        LostProcess::Broker { .. } | LostProcess::LegacyRecoveryStatus => {
            let live = if let LostProcess::Broker { scenario_end } = lost {
                let live = selection
                    .open_worker_event_stream(
                        docker_events_since,
                        daemon_sample_before.into_std(),
                        scenario_end.into_std(),
                    )
                    .await?;
                evidence.write(
                    "docker-worker-stream-open.json",
                    &json!({"http_status":200,"framing":"http1_chunked","since":docker_events_since,"until":live.until}),
                )?;
                Some(live)
            } else {
                None
            };
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
                } if operation_id == operation => {
                    evidence.write("submit-accepted.json", &json!({"stage":"submit_accepted"}))?;
                }
                BrokerReplyV1::Error { code } => {
                    bail!("real submission refused before Running: {code}")
                }
                _ => bail!("real submission did not produce fresh Accepted"),
            }
            (None, live)
        }
        LostProcess::RunCli => {
            evidence.write(
                "stage.json",
                &json!({"stage":"prepare_broker_client_selection"}),
            )?;
            let client_selection = prepare_broker_client(selection, evidence).await?;
            let child =
                harness::run_client(selection, evidence, &client_selection, operation, &argv)?;
            evidence.write("stage.json", &json!({"stage":"submit_real_worker"}))?;
            (Some((child, client_selection)), None)
        }
    };
    let clock = NativeHostClock::new()?;
    // H8 reached 120 seconds without Running or a coded failure; completion of
    // its Arm reply was not established. Give the whole scenario 600 seconds,
    // bound and stop on an explicit failure, not an intermediate timeout.
    // The selected Arm is fixed before Running. The HTTP query does not enqueue
    // any writer/journal read or extend the original deadline.
    let mut next_failure_poll = Instant::now();
    let mut observed_worker: Option<(String, WorkerStateV1)> = None;
    let worker = loop {
        broker.running()?;
        gateway.running()?;
        if let Some((child, _)) = &mut cli {
            child.running()?;
        }
        let running = if let Some(worker) = harness::only_worker(&http, &origin, &runner).await? {
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
                Some(worker)
            } else {
                ensure!(
                    !matches!(
                        worker.state,
                        WorkerStateV1::Terminal | WorkerStateV1::Cancelled
                    ),
                    "worker became terminal before Running was observed"
                );
                None
            }
        } else {
            ensure!(
                observed_worker.is_none(),
                "previously observed worker disappeared"
            );
            None
        };
        // One final diagnostic read is required before leaving this phase.
        // It obeys the same rate bound and can only fail the strict deadline
        // oracle later; it never grants more execution time to the worker.
        if running.is_some() && Instant::now() < next_failure_poll {
            sleep_until(next_failure_poll).await;
        }
        if Instant::now() >= next_failure_poll {
            let page: Value = harness::get(
                &http,
                &origin,
                &runner,
                "events?kind=broker.batch.failure&limit=1&order=asc",
            )
            .await?;
            if let Some(marker) = harness::batch_failure_marker(&page)? {
                evidence.write("reported-batch-failure.json", &marker)?;
                bail!(
                    "FAIL: broker reported a coded batch failure before the planned fault; inspect retained diagnostic event"
                );
            }
            next_failure_poll = Instant::now() + Duration::from_secs(1);
        }
        if let Some(worker) = running {
            break worker;
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
            && arm.arm.timeout_seconds == timeout_seconds,
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
    ensure!(
        arm.arm
            .clock
            .continuous_ns()?
            .checked_add(u128::from(timeout_seconds) * NS)
            == Some(deadline),
        "reported Arm deadline is not the selected duration after its native sample"
    );
    let now = clock.sample()?;
    ensure!(
        now.boot_id == arm.arm.clock.boot_id
            && now.method == arm.arm.clock.method
            && now.continuous_ns()? + 150_000_000 < deadline,
        "Running oracle was too late to signal the selected process before the original deadline"
    );
    if let Some((mut run_cli, client_selection)) = cli {
        evidence.write("stage.json", &json!({"stage":"kill_owned_run_cli_only","kill_clock":now,"original_deadline_ns":deadline.to_string()}))?;
        let cli_pid = run_cli.id();
        let broker_pid = broker.id();
        *run_client_stage = RunClientRecoveryStage::ReapClock;
        let killed_status = run_cli.kill_owned()?;
        let killed = clock.sample()?;
        evidence.write("run-cli-killed.json", &json!({"clock_before":now,"clock_after_reap":killed,"original_deadline_ns":deadline.to_string(),"cli_pid":cli_pid,"broker_pid":broker_pid,"signal":killed_status.signal()}))?;
        ensure!(
            killed_status.signal() == Some(libc::SIGKILL)
                && killed.boot_id == arm.arm.clock.boot_id
                && killed.method == arm.arm.clock.method
                && killed.continuous_ns()? < deadline,
            "owned CLI death/reap before the original Arm deadline was not established"
        );
        *run_client_stage = RunClientRecoveryStage::BrokerLiveness;
        broker.running()?;
        *run_client_stage = RunClientRecoveryStage::TerminalWait;
        evidence.write(
            "stage.json",
            &json!({"stage":"read_original_job_without_resubmit"}),
        )?;
        // The live broker may already have removed an acknowledged terminal CID.
        // A Result and exact Docker events independently establish the stop and removal.
        wait_terminal(
            evidence,
            &runner,
            &http,
            &origin,
            (operation, &worker.worker_id),
            Duration::from_secs(u64::from(timeout_seconds) + 35),
            run_client_stage,
        )
        .await?;
        *run_client_stage = RunClientRecoveryStage::ResultRead;
        let result = read_result(selection, &client_selection, operation).await?;
        *run_client_stage = RunClientRecoveryStage::ResultOutcome;
        ensure!(
            !result.cancel_requested,
            "run CLI loss unexpectedly requested cancellation"
        );
        let JobResultOutcomeV1::Terminal { result: terminal } = &result.outcome else {
            evidence.write("cli-result-nonterminal.json", &result)?;
            bail!("original job has no durable terminal result after the CLI died")
        };
        *run_client_stage = RunClientRecoveryStage::TerminalCorrelation;
        let reconciled = harness::only_worker(&http, &origin, &runner)
            .await?
            .context("original worker missing after CLI loss")?;
        let actual = reconciled
            .last_observation
            .as_ref()
            .context("missing central terminal observation")?;
        let finished = DateTime::parse_from_rfc3339(
            actual
                .finished_at
                .as_deref()
                .context("central terminal FinishedAt missing")?,
        )?;
        ensure!(
            reconciled.worker_id == worker.worker_id
                && reconciled.attempt == worker.attempt
                && reconciled.observed_started_at == worker.observed_started_at
                && reconciled.launch.authority_id == actual.inspection.authority_id
                && reconciled.launch.authority_id == worker.launch.authority_id
                && reconciled.cleanup_removed == Some(true)
                && actual.running == Some(false)
                && actual.restart_count == Some(0)
                && actual.exit_code == Some(137)
                && actual.oom_killed == Some(false)
                && actual.inspection.container_id == cid
                && terminal.session_id.to_string() == worker.launch.session_id
                && terminal.container_id == cid
                && terminal.volume == worker.attempt.volume_name
                && terminal.exit_code == 137
                && terminal.oom_killed == Some(false)
                && terminal.changed_files == 0
                && !terminal.output_captured,
            "central observation and durable Result do not establish the original worker's clean non-OOM deadline stop"
        );
        *run_client_stage = RunClientRecoveryStage::PostTerminalChecks;
        verify_cli_docker_terminal(
            selection,
            evidence,
            &clock,
            docker_events_since,
            &arm,
            terminal,
            (docker_started, finished),
        )
        .await?;
        let actual_run_ns = observed_process_duration_ns(docker_started, finished, sleep_seconds)?;
        evidence.write(
            "deadline-observation.json",
            &json!({
                "original_arm":arm,
                "docker_started_at":docker_started,"docker_finished_at":finished,
                "observed_process_duration_ns":actual_run_ns.to_string(),
                "observation_tolerance_ns":OBSERVATION_GRACE_NS.to_string()
            }),
        )?;
        let after = recovery(evidence, &runner, operation).await?;
        let worker_arms: Vec<_> = after
            .known_arms
            .iter()
            .filter(|arm| matches!(arm.subject, RecoveryTargetKindV1::Worker))
            .collect();
        ensure!(
            worker_arms.len() == 1
                && serde_json::to_value(worker_arms[0])? == serde_json::to_value(target)?,
            "the original worker Arm, deadline or cancel state changed after CLI loss"
        );
        let replay = read_result(selection, &client_selection, operation).await?;
        ensure!(
            serde_json::to_value(&replay)? == serde_json::to_value(&result)?,
            "read-only replay changed the original operation result"
        );
        let usage: Value = harness::get(&http, &origin, &runner, "usage?limit=100").await?;
        let allocation = check_usage(&usage, &reconciled, cid)?;
        let quota: QuotaUsageV1 = harness::get(&http, &origin, &runner, "quota").await?;
        ensure!(
            quota.held_workers == 0
                && quota.held_cpu_millis == 0
                && quota.held_memory_bytes == 0
                && !reconciled.quota_held
                && allocation.source.authority_id == worker.launch.authority_id,
            "terminal quota was held or allocation lost the original authority"
        );
        let audit_after = harness::audit(&http, &origin, &runner).await?;
        ensure!(
            harness::worker_emissions(&audit_after, &worker.worker_id)? == emissions,
            "a second worker Arm or claim appeared after CLI loss"
        );
        verify_result_audit(&audit_after, &worker, terminal)?;
        let integrity: Value = harness::get(&http, &origin, &runner, "integrity").await?;
        ensure!(
            integrity["valid"] == true
                && integrity["event_count"]
                    .as_u64()
                    .is_some_and(|count| count > 0),
            "actual audit integrity is not valid/nonempty"
        );
        let guard_after = guard.status().await?;
        let slot_after = guard.slot_status(&arm.arm.operation_id).await?;
        ensure!(
            guard_after.epoch() == epoch && slot_after.prepared_once(),
            "enforcer epoch or consumed start permit changed after the run CLI died"
        );
        if let Some(observation) = slot_after.observation() {
            ensure!(
                observation.id.as_deref() == Some(cid)
                    && observation.process.finished_at == Some(finished)
                    && observation.process.exit_code == Some(137),
                "retained enforcer observation differs from the original terminal"
            );
        }
        broker.running()?;
        gateway.running()?;
        ensure!(
            Sha256::digest(fs::read(&source)?) == original
                && fs::read_dir(evidence.root.join("source"))?.count() == 1,
            "original source changed"
        );
        evidence.write("audit-after.json", &audit_after)?;
        evidence.write("recovery-after.json", &after)?;
        evidence.write("worker-after.json", &reconciled)?;
        evidence.write("cli-job-result.json", &result)?;
        evidence.write("guard-after-stop.json", &slot_after)?;
        evidence.write("reconciled.json", &json!({"broker_pid":broker_pid,"broker_live_after_result_replay":true,"enforcer_epoch":epoch,"quota":quota,"allocation":allocation,"integrity":integrity,"source_sha256":hex::encode(original),"original_cid":cid,"deadline_ns":deadline.to_string()}))?;
        *run_client_stage = RunClientRecoveryStage::ControlShutdown;
        let broker_exit = broker.terminate_owned()?;
        ensure!(
            broker_exit.success(),
            "original broker did not exit gracefully after evidence collection"
        );
        ensure!(
            gateway.terminate_owned()?.success(),
            "gateway did not exit gracefully after evidence collection"
        );
        *run_client_stage = RunClientRecoveryStage::Complete;
        return Ok(());
    }
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
    let (terminal, terminal_clock, actual_run_ns) = loop {
        gateway.running()?;
        let observed = selection.inspect(cid).await?; // Absence/inspect error is a failure, never terminal.
        let sample = clock.sample()?;
        let observed_ns = sample.continuous_ns()?;
        ensure!(
            observed_ns <= deadline + OBSERVATION_GRACE_NS,
            "FAIL: terminal not observed by original deadline + 3s"
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
            let actual_run_ns =
                observed_process_duration_ns(docker_started, finished, sleep_seconds)?;
            evidence.write("docker-terminal-observed.json", &observed)?;
            break (observed, sample, actual_run_ns);
        }
        sleep(Duration::from_millis(50)).await;
    };
    let (stop_status, guard_terminal_received_clock) = loop {
        let slot = guard.slot_status(&arm.arm.operation_id).await?;
        let received = clock.sample()?;
        ensure!(
            received.boot_id == arm.arm.clock.boot_id
                && received.method == arm.arm.clock.method
                && received.continuous_ns()? <= deadline + OBSERVATION_GRACE_NS,
            "guard terminal snapshot was not received on the original clock by deadline + 3s"
        );
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
            break (slot, received);
        }
        sleep(Duration::from_millis(25)).await;
    };
    ensure!(
        guard.status().await?.epoch() == epoch && stop_status.prepared_once(),
        "original enforcer epoch/consumed permit changed after broker death"
    );
    evidence.write("guard-after-stop.json", &stop_status)?;
    // This projection assumes Docker's wall clock stays continuous between the terminal and this sample.
    let before_clock_probe = clock.sample()?;
    let docker_clock = selection
        .docker(&["info", "--format", "{{json .SystemTime}}"])
        .await?;
    let after_clock_probe = clock.sample()?;
    ensure!(
        before_clock_probe.boot_id == arm.arm.clock.boot_id
            && after_clock_probe.boot_id == arm.arm.clock.boot_id
            && before_clock_probe.method == arm.arm.clock.method
            && after_clock_probe.method == arm.arm.clock.method,
        "terminal clock projection does not use the original boot and method"
    );
    let docker_now = chrono::DateTime::parse_from_rfc3339(
        docker_clock.as_str().context("Docker SystemTime missing")?,
    )?;
    let docker_finished = chrono::DateTime::parse_from_rfc3339(
        terminal["finished_at"]
            .as_str()
            .context("Docker FinishedAt missing")?,
    )?;
    let time_since_finish = docker_now
        .signed_duration_since(docker_finished)
        .num_nanoseconds()
        .context("Docker finish-to-sample duration is not representable")?;
    let earliest_finish = before_clock_probe
        .continuous_ns()?
        .checked_sub(u128::try_from(time_since_finish).context("Docker clock precedes terminal")?)
        .context("Docker terminal wall-clock projection underflowed")?;
    evidence.write("terminal-clock-projection.json", &json!({
        "host_before":before_clock_probe,"host_after":after_clock_probe,"docker_wall_sample":docker_clock,
        "docker_finished_at":terminal["finished_at"],"earliest_finish_ns":earliest_finish.to_string(),
        "original_deadline_ns":deadline.to_string(),"projection_method":"single_post_fault_wall_sample"
    }))?;
    ensure!(
        earliest_finish >= deadline,
        "Docker terminal may precede the original Arm deadline"
    );
    evidence.write("deadline-observation.json", &json!({
        "original_arm":arm,"observed_clock":terminal_clock,
        "guard_terminal_received_clock":guard_terminal_received_clock,
        "observation_lateness_ns":terminal_clock.continuous_ns()?.saturating_sub(deadline).to_string(),
        "observation_tolerance_ns":OBSERVATION_GRACE_NS.to_string(),"observed_process_duration_ns":actual_run_ns.to_string(),
        "physical_deadline_exactly_5s_qualified":false,"docker":terminal
    }))?;

    evidence.write(
        "stage.json",
        &json!({"stage":"restart_same_broker_and_registry"}),
    )?;
    if matches!(lost, LostProcess::LegacyRecoveryStatus) {
        return verify_legacy_recovery(
            selection,
            evidence,
            &runner,
            operation,
            &recovery_before,
            target,
            &mut gateway,
        )
        .await;
    }
    let mut restarted = harness::broker(selection, evidence, 2)?;
    wait_broker(evidence, &runner, operation, &mut restarted).await?;
    evidence.write("stage.json", &json!({"stage":"restart_broker_ready"}))?;
    wait_terminal(
        evidence,
        &runner,
        &http,
        &origin,
        (operation, &worker.worker_id),
        Duration::from_secs(35),
        run_client_stage,
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
    evidence.write(
        "stage.json",
        &json!({"stage":"restart_terminal_quota_released"}),
    )?;
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
        (operation, &worker.worker_id),
        Duration::from_secs(35),
        run_client_stage,
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
    let final_worker = harness::only_worker(&http, &origin, &runner)
        .await?
        .context("original worker missing after terminal replay")?;
    ensure!(
        final_worker.worker_id == worker.worker_id,
        "tenant control replaced the original worker after terminal replay"
    );
    let session =
        Uuid::parse_str(&worker.launch.session_id).context("invalid original session ID")?;
    let live = live_events.context("live Docker worker stream missing")?;
    let after_resume = selection.docker_clock().await?;
    let docker_events_until = live.until;
    ensure!(
        docker_started < after_resume && after_resume < docker_events_until,
        "live Docker event stream cutoff does not cover terminal replay"
    );
    let observed = live.finish(session).await?;
    evidence.write("docker-broker-worker-live-events.json", &observed)?;
    let events = selection
        .session_worker_events(
            session,
            docker_events_since,
            docker_started,
            docker_events_until,
        )
        .await?;
    evidence.write(
        "docker-broker-worker-events.json",
        &json!({"session":session,"since":docker_events_since,"after_resume":after_resume,"until":docker_events_until,"events":events}),
    )?;
    ensure!(
        events
            .iter()
            .filter(|event| event.is_worker_creation_or_start())
            .eq(observed.iter()),
        "live Docker worker create/start events disagree with closed history"
    );
    let create = exactly_one_docker_event(&events, "create")?;
    let start = exactly_one_worker_start(&events, cid)?;
    ensure!(
        create.wall_ns <= start.wall_ns
            && DateTime::from_timestamp_nanos(start.wall_ns) < docker_finished,
        "Docker history does not contain the original worker's create/start before its terminal"
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
    evidence.write(
        "stage.json",
        &json!({"stage":"restart_replay_stable_before_cleanup"}),
    )?;
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

async fn broker_client_output(
    mut command: Command,
    client_selection: Option<&Path>,
) -> Result<Vec<u8>> {
    const OUTPUT_LIMIT: u64 = 64 * 1024;
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if let Some(client_selection) = client_selection {
        command.env(harness::BROKER_SELECTION_ENV, client_selection);
    }
    let mut child = command.spawn().context("spawn broker client CLI")?;
    let stdout = child.stdout.take().context("broker client stdout pipe")?;
    let read = async {
        let mut bytes = Vec::new();
        stdout
            .take(OUTPUT_LIMIT + 1)
            .read_to_end(&mut bytes)
            .await?;
        ensure!(
            bytes.len() as u64 <= OUTPUT_LIMIT,
            "broker client output exceeded bound"
        );
        Ok::<_, anyhow::Error>(bytes)
    };
    timeout(Duration::from_secs(15), async {
        let (status, bytes) =
            tokio::try_join!(async { Ok::<_, anyhow::Error>(child.wait().await?) }, read)?;
        ensure!(
            status.success(),
            "broker client CLI failed (status only: {status})"
        );
        Ok(bytes)
    })
    .await
    .context("broker client CLI exceeded 15s")?
}

async fn prepare_broker_client(selection: &Selection, evidence: &Evidence) -> Result<PathBuf> {
    // The product publishes into a new child of the existing private operator
    // directory. Neither the source nor the export tree contains this input.
    let destination = evidence.root.join("operator/client-selection");
    let mut command = Command::new(&selection.binary);
    command
        .args(["broker", "prepare-client", "--broker-config"])
        .arg(evidence.root.join("operator/registry.json"))
        .args(["--profile", PROFILE, "--destination"])
        .arg(&destination);
    let stdout = broker_client_output(command, None)
        .await
        .context("publish broker client selection")?;
    ensure!(
        stdout.is_empty(),
        "prepare-client unexpectedly wrote to stdout"
    );
    let directory = Directory::open_existing(&destination)?;
    let record = directory.read_private_file("selection.json", 64 * 1024)?;
    let selected: Value = serde_json::from_slice(record.bytes())
        .map_err(|_| anyhow::anyhow!("invalid published broker selection record"))?;
    ensure!(
        selected["version"] == 1 && selected["profile"] == PROFILE,
        "published broker selection does not match the fixture profile"
    );
    record.revalidate()?;
    evidence.write(
        "broker-client-selection.json",
        &json!({
            "mode":"published_selection","input":"environment",
            "profile":PROFILE,"selection_sha256":record.digest()
        }),
    )?;
    Ok(destination)
}

async fn read_result(
    selection: &Selection,
    client_selection: &Path,
    operation_id: Uuid,
) -> Result<JobResultV1> {
    let mut command = Command::new(&selection.binary);
    command
        .args(["broker", "result", "--operation-id"])
        .arg(operation_id.to_string());
    let output = broker_client_output(command, Some(client_selection))
        .await
        .context("read original broker job result")?;
    match serde_json::from_slice::<BrokerReplyV1>(&output)
        .map_err(|_| anyhow::anyhow!("invalid result reply from broker CLI"))?
    {
        BrokerReplyV1::Result { result }
            if result.version == 1 && result.operation_id == operation_id =>
        {
            Ok(result)
        }
        BrokerReplyV1::Error { code } => bail!("Result refused: {code}"),
        _ => bail!("unexpected result for the original operation"),
    }
}

fn observed_process_duration_ns(
    started: DateTime<FixedOffset>,
    finished: DateTime<FixedOffset>,
    sleep_seconds: u32,
) -> Result<i64> {
    let duration = finished
        .signed_duration_since(started)
        .num_nanoseconds()
        .context("terminal process duration is not representable")?;
    ensure!(
        duration > 0 && i128::from(duration) < i128::from(sleep_seconds) * 1_000_000_000,
        "natural sleep completion or invalid process timestamps cannot qualify deadline stop"
    );
    Ok(duration)
}

async fn verify_cli_docker_terminal(
    selection: &Selection,
    evidence: &Evidence,
    clock: &NativeHostClock,
    since: DateTime<FixedOffset>,
    arm: &ArmResultV1,
    terminal: &TerminalJobResultV1,
    (started, finished): (DateTime<FixedOffset>, DateTime<FixedOffset>),
) -> Result<()> {
    // This projection assumes Docker's wall clock stays continuous between the events and this sample.
    let before = clock.sample()?;
    let docker_now = selection.docker_clock().await?;
    let after = clock.sample()?;
    ensure!(
        [&before, &after]
            .iter()
            .all(|sample| sample.boot_id == arm.arm.clock.boot_id
                && sample.method == arm.arm.clock.method),
        "Docker event projection does not use the original host clock boot/method"
    );
    let events = selection
        .session_worker_events(terminal.session_id, since, started, docker_now)
        .await?;
    evidence.write("docker-cli-worker-events.json", &events)?;
    // Event history corroborates the unique start for this CID. Physical duration uses inspect timestamps.
    let start = exactly_one_worker_start(&events, &terminal.container_id)?;
    let kill = exactly_one_docker_event(&events, "kill")?;
    let die = exactly_one_docker_event(&events, "die")?;
    let destroy = exactly_one_docker_event(&events, "destroy")?;
    let finished_wall = finished
        .timestamp_nanos_opt()
        .context("unrepresentable Docker FinishedAt")?;
    ensure!(
        kill.signal == "9"
            && die.exit_code == "137"
            && start.wall_ns < kill.wall_ns
            && kill.wall_ns <= die.wall_ns
            && die.wall_ns <= destroy.wall_ns
            && start.wall_ns < finished_wall
            && finished_wall <= die.wall_ns
            && !events
                .iter()
                .any(|event| matches!(event.action.as_str(), "oom" | "restart")),
        "Docker history does not prove one start followed by SIGKILL, terminal and removal of the exact CID"
    );
    let to_host_bounds = |wall_ns: i64| -> Result<(u128, u128)> {
        let event_time = DateTime::from_timestamp_nanos(wall_ns);
        let elapsed = docker_now
            .signed_duration_since(event_time)
            .num_nanoseconds()
            .context("Docker event clock delta not representable")?;
        let elapsed =
            u128::try_from(elapsed).context("Docker event is later than its clock sample")?;
        Ok((
            before
                .continuous_ns()?
                .checked_sub(elapsed)
                .context("earliest host event time underflow")?,
            after
                .continuous_ns()?
                .checked_sub(elapsed)
                .context("latest host event time underflow")?,
        ))
    };
    let (earliest_kill, _) = to_host_bounds(kill.wall_ns)?;
    let (earliest_finish, latest_finish) = to_host_bounds(finished_wall)?;
    let deadline = arm.arm.deadline_ns_decimal.parse::<u128>()?;
    evidence.write("terminal-clock-projection.json", &json!({
        "host_before":before,"host_after":after,"docker_wall_sample":docker_now,"docker_finished_at":finished,
        "earliest_kill_ns":earliest_kill.to_string(),"earliest_finish_ns":earliest_finish.to_string(),
        "latest_finish_ns":latest_finish.to_string(),"original_deadline_ns":deadline.to_string(),
        "observation_tolerance_ns":OBSERVATION_GRACE_NS.to_string(),"projection_method":"single_post_fault_wall_sample"
    }))?;
    ensure!(
        earliest_kill >= deadline
            && earliest_finish >= deadline
            && latest_finish <= deadline + OBSERVATION_GRACE_NS,
        "Docker terminal is not established between the original deadline and its three-second allowance"
    );
    Ok(())
}

fn exactly_one_worker_start<'a>(
    events: &'a [harness::ContainerEvent],
    cid: &str,
) -> Result<&'a harness::ContainerEvent> {
    ensure!(
        events.iter().all(|event| event.id == cid),
        "Docker history contains another worker CID for the original session"
    );
    exactly_one_docker_event(events, "start")
}

#[test]
fn docker_worker_start_oracle_rejects_incomplete_or_duplicate_history() {
    let original = "a".repeat(64);
    let replacement = "b".repeat(64);
    let event = |action: &str, id: &str, wall_ns| harness::ContainerEvent {
        action: action.to_owned(),
        id: id.to_owned(),
        session: "11111111-1111-4111-8111-111111111111".into(),
        purpose: "run".into(),
        wall_ns,
        exit_code: String::new(),
        signal: String::new(),
    };
    assert_eq!(
        exactly_one_worker_start(
            &[event("start", &original, 1), event("die", &original, 2)],
            &original,
        )
        .unwrap()
        .wall_ns,
        1
    );
    for (history, expected_error) in [
        (vec![event("die", &original, 2)], "omitted the start event"),
        (
            vec![event("start", &original, 1), event("start", &original, 2)],
            "repeated the start event",
        ),
        (
            vec![
                event("start", &original, 1),
                event("start", &replacement, 2),
            ],
            "another worker CID",
        ),
    ] {
        match exactly_one_worker_start(&history, &original) {
            Ok(_) => panic!("incomplete or duplicate Docker history passed"),
            Err(error) => assert!(error.to_string().contains(expected_error)),
        }
    }
}

fn exactly_one_docker_event<'a>(
    events: &'a [harness::ContainerEvent],
    action: &str,
) -> Result<&'a harness::ContainerEvent> {
    let mut matching = events.iter().filter(|event| event.action == action);
    let event = matching
        .next()
        .with_context(|| format!("Docker history omitted the {action} event"))?;
    ensure!(
        matching.next().is_none(),
        "Docker history repeated the {action} event"
    );
    Ok(event)
}

fn verify_result_audit(
    events: &[Value],
    worker: &WorkerV1,
    terminal: &TerminalJobResultV1,
) -> Result<()> {
    let at = |sequence: i64, kind: &str| {
        events
            .iter()
            .find(|event| {
                event["seq"] == sequence
                    && event["kind"] == kind
                    && event["session_id"] == worker.launch.session_id
            })
            .with_context(|| format!("missing matching durable {kind} audit event"))
    };
    let result = at(terminal.result_audit_seq, "broker.batch.result")?;
    let scan = at(terminal.scan_audit_seq, "broker.batch.scan")?;
    let cleanup = at(terminal.cleanup_audit_seq, "control.worker.transition")?;
    let mutation = &cleanup["data"]["input"]["request"];
    ensure!(
        result["data"]["worker_id"] == worker.worker_id
            && result["data"]["observation"]["id"] == terminal.container_id
            && result["data"]["observation"]["process"]["exit_code"] == terminal.exit_code
            && scan["data"]["worker_id"] == worker.worker_id
            && scan["data"]["manifest_digest"] == terminal.manifest_digest
            && scan["data"]["changed_files"].as_u64() == u64::try_from(terminal.changed_files).ok()
            && mutation["worker_id"] == worker.worker_id
            && mutation["attempt_id"] == worker.attempt.attempt_id
            && mutation["transition"]["action"] == "cleanup_result"
            && mutation["transition"]["removed"] == true,
        "Result audit sequences do not bind the worker, observation, scan and cleanup"
    );
    Ok(())
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

async fn verify_legacy_recovery(
    selection: &Selection,
    evidence: &Evidence,
    runner: &str,
    operation: Uuid,
    before: &RecoveryStatusV1,
    original: &RecoveryTargetV1,
    gateway: &mut harness::OwnedProcess,
) -> Result<()> {
    gateway.require_graceful_shutdown();
    let mut restarted = harness::broker(selection, evidence, 2).ok();
    let stage = if let Some(child) = &mut restarted {
        child.require_graceful_shutdown();
        evidence.write("legacy-recovery-stage.json", &LegacyRecoveryStage::Spawned)?;
        match wait_legacy_recovery(evidence, runner, operation, child).await {
            Ok(after) => legacy_recovery_stage(before, &after, operation, original),
            Err(stage) => stage,
        }
    } else {
        LegacyRecoveryStage::SpawnFailed
    };
    let proof = evidence.write("legacy-recovery-stage.json", &stage);
    let broker_closure = restarted.as_mut().map_or("not_spawned", close_legacy_child);
    let gateway_closure = close_legacy_child(gateway);
    let closure = evidence.write(
        "legacy-recovery-closure.json",
        &json!({"broker": broker_closure, "gateway": gateway_closure, "guardian_shutdown": "not_requested"}),
    );
    proof?;
    ensure!(
        matches!(stage, LegacyRecoveryStage::RecoveryAttested),
        "Legacy restart did not attest the original Arm"
    );
    closure
}

async fn wait_legacy_recovery(
    evidence: &Evidence,
    runner: &str,
    operation_id: Uuid,
    child: &mut harness::OwnedProcess,
) -> std::result::Result<RecoveryStatusV1, LegacyRecoveryStage> {
    let limit = Instant::now() + Duration::from_secs(20);
    let mut unavailable = LegacyRecoveryStage::AvailabilityTimedOut;
    loop {
        if child
            .has_exited()
            .map_err(|_| LegacyRecoveryStage::ChildWaitFailed)?
        {
            return Err(LegacyRecoveryStage::ChildExited);
        }
        if Instant::now() >= limit {
            return Err(unavailable);
        }
        if let Ok(Ok(reply)) = timeout_at(
            limit,
            ipc(
                evidence,
                runner,
                BrokerCommandV1::RecoveryStatus { operation_id },
            ),
        )
        .await
        {
            match reply {
                BrokerReplyV1::RecoveryStatus { status } => return Ok(status),
                BrokerReplyV1::Error { .. } => unavailable = LegacyRecoveryStage::StatusRefused,
                _ => return Err(LegacyRecoveryStage::UnexpectedReply),
            }
        }
        sleep_until(limit.min(Instant::now() + Duration::from_millis(50))).await;
    }
}

fn legacy_recovery_stage(
    before: &RecoveryStatusV1,
    after: &RecoveryStatusV1,
    operation: Uuid,
    original: &RecoveryTargetV1,
) -> LegacyRecoveryStage {
    let mut worker_arms = after
        .known_arms
        .iter()
        .filter(|arm| matches!(arm.subject, RecoveryTargetKindV1::Worker));
    let Some(arm) = worker_arms.next() else {
        return LegacyRecoveryStage::OriginalArmMismatch;
    };
    let clock = &arm.original_clock;
    let original_clock = &original.original_clock;
    let same_clock = clock.version == original_clock.version
        && clock.method == original_clock.method
        && clock.boot_id == original_clock.boot_id
        && clock.continuous_ns_decimal == original_clock.continuous_ns_decimal
        && clock.observed_utc == original_clock.observed_utc;
    if after.version == 1
        && before.operation_id == operation
        && after.operation_id == operation
        && after.tenant_id == before.tenant_id
        && after.source == RecoveryDataSourceV1::ConfirmedJournal
        && after.runtime_evidence == RecoveryRuntimeEvidenceV1::NotQueried
        && !after.incomplete
        && after.head_revision >= before.head_revision
        && (after.head_revision > before.head_revision || after.head_hash == before.head_hash)
        && worker_arms.next().is_none()
        && arm.arm_id == original.arm_id
        && arm.worker_id == original.worker_id
        && arm.attempt_id == original.attempt_id
        && arm.original_timeout_seconds == original.original_timeout_seconds
        && arm.original_deadline_ns_decimal == original.original_deadline_ns_decimal
        && same_clock
    {
        LegacyRecoveryStage::RecoveryAttested
    } else {
        LegacyRecoveryStage::OriginalArmMismatch
    }
}

fn close_legacy_child(child: &mut harness::OwnedProcess) -> &'static str {
    match child.has_exited() {
        Ok(true) => "already_exited",
        Ok(false) if child.terminate_owned().is_ok_and(|status| status.success()) => "confirmed",
        _ => "unconfirmed",
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
            Ok(BrokerReplyV1::Status { status }) if status.operation_id == operation_id
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
    (operation_id, worker_id): (Uuid, &str),
    budget: Duration,
    run_client_stage: &mut RunClientRecoveryStage,
) -> Result<()> {
    let limit = Instant::now() + budget;
    loop {
        *run_client_stage = RunClientRecoveryStage::WorkerRead;
        let worker: WorkerV1 =
            harness::get(http, origin, runner, &format!("workers/{worker_id}")).await?;
        *run_client_stage = RunClientRecoveryStage::StatusRead;
        if let BrokerReplyV1::Status { status } =
            ipc(evidence, runner, BrokerCommandV1::Status { operation_id }).await?
            && worker.state == WorkerStateV1::Terminal
            && !worker.quota_held
            && !status.executor_task_running
        {
            if let Some(executor_error) = status.executor_error {
                let central_cleanup_sample = match worker.cleanup_removed {
                    None => CentralCleanupSample::NotObserved,
                    Some(false) => CentralCleanupSample::ReportedUnconfirmed,
                    Some(true) => CentralCleanupSample::ReportedRemoved,
                };
                *run_client_stage = RunClientRecoveryStage::Executor {
                    central_cleanup_sample,
                    executor_error,
                };
            }
            ensure!(
                status.executor_error.is_none(),
                "reconciliation executor failed: {:?}",
                status.executor_error
            );
            return Ok(());
        }
        *run_client_stage = RunClientRecoveryStage::TerminalWait;
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
