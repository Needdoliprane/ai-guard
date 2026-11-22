//! Real Export helper/retirement path: Journal, SQLite, filesystem target
//! validation and native Full3 IPC. Prior runtime observations and the selected
//! Rust info/inspect program are synthetic. No Docker, provider, confinement or
//! registry authentication qualification is inferred from this test.
use super::*;
use crate::{
    audit::EventQuery,
    broker::{
        enforcer::{EnforcerFullClientV3, run_internal_full3_fresh},
        journal::{canonical, retirement_live_fixture::INVOCATION_DATA_PROGRAM},
        service::{AbandonBeforeClaimV1, preclaim::PreclaimStateV1},
    },
    runtime::{ContainerHandle, RuntimeAuthority},
};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
};

const ENGINE: &str = "export-retirement-synthetic-engine";
const STARTED: &str = "2026-10-30T12:00:00Z";
const FINISHED: &str = "2026-10-30T12:00:02Z";
const CHILD: &str = "runner::broker::executor::journal::tests::export_retirement_tests::export_helper_retirement_child";
const SIGNAL: &str = "broker.abandon-before-claim.requested";
const INTENTION: &str = "broker.retirement.pending";
const FINAL: &str = "broker.retirement.final";
// Fixture preparation performs many real journal/SQLite acknowledgements. The
// earlier 120s cap expired while setup was still making observable progress on
// the constrained development host. This outer fixture budget changes no
// worker, helper Arm, runtime operation or retry deadline.
const FIXTURE_BUDGET: Duration = Duration::from_secs(300);

fn trace_phase(phase: &'static str) {
    static ORIGIN: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let elapsed = ORIGIN.get_or_init(std::time::Instant::now).elapsed();
    eprintln!(
        "export-retirement phase={phase} elapsed_ms={}",
        elapsed.as_millis()
    );
}

fn fixture_rustc() -> Result<PathBuf> {
    let configured = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    // Resolve the selected toolchain in the parent, where a rustup shim can
    // still use the runner's environment. Pass the real compiler to the child.
    let output = Command::new(configured)
        .args(["--print", "sysroot"])
        .output()?;
    ensure!(
        output.status.success(),
        "cannot resolve the fixture toolchain"
    );
    let sysroot = std::str::from_utf8(&output.stdout)?;
    let compiler = Path::new(sysroot.trim_end_matches(['\r', '\n'])).join("bin/rustc");
    ensure!(
        compiler.is_absolute() && compiler.is_file(),
        "fixture toolchain did not provide an absolute compiler path"
    );
    Ok(compiler)
}

/// A failing oracle cannot leave an original native actor in the test runner.
/// This parent owns the exact child PID and reaps it on every path.
#[test]
fn export_helper_abandonment_propagates_without_extraction_or_export_success() {
    let rustc = fixture_rustc().expect("resolve the export fixture compiler in the parent");
    let mut root = tempfile::Builder::new()
        .prefix("er-parent-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let log_path = root.path().join("child.log");
    let log = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&log_path)
        .unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", CHILD, "--ignored", "--nocapture"])
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("AI_GUARD_TEST_EXPORT_RETIREMENT_CHILD", "owned")
        .env("RUSTC", rustc)
        .env("TMPDIR", root.path())
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone().unwrap()))
        .stderr(Stdio::from(log));
    let mut child = NativeChild(command.spawn().unwrap());
    // Fixture budget only, not a production retirement bound.
    let expires = std::time::Instant::now() + FIXTURE_BUDGET;
    let status = loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            break status;
        }
        if std::time::Instant::now() >= expires {
            child.0.kill().unwrap();
            child.0.wait().unwrap();
            root.disable_cleanup(true);
            let mut diagnostic = String::new();
            if let Ok(log) = fs::File::open(&log_path) {
                let _ = log.take(16 * 1024).read_to_string(&mut diagnostic);
            }
            panic!("owned export-retirement child exceeded its 300s fixture budget:\n{diagnostic}");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut diagnostic = String::new();
    fs::File::open(log_path)
        .unwrap()
        .take(128 * 1024)
        .read_to_string(&mut diagnostic)
        .unwrap();
    let oracle_reached =
        diagnostic.contains("export-retirement oracle=break_without_export_success");
    if !status.success() || !oracle_reached {
        root.disable_cleanup(true);
    }
    assert!(
        status.success(),
        "export-retirement child failed:\n{diagnostic}"
    );
    assert!(
        oracle_reached,
        "export-retirement child omitted its oracle marker:\n{diagnostic}"
    );
}

#[test]
#[ignore = "owned child invoked only by the export-retirement parent"]
fn export_helper_retirement_child() {
    assert_eq!(
        std::env::var("AI_GUARD_TEST_EXPORT_RETIREMENT_CHILD").as_deref(),
        Ok("owned")
    );
    let driver = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let handle = driver.handle().clone();
    // Separate polling frames avoid hiding a deep fixture under a larger stack.
    driver
        .block_on(
            async move { tokio::task::spawn_blocking(move || run_export_oracle(&handle)).await },
        )
        .unwrap()
        .unwrap();
}

fn run_export_oracle(handle: &tokio::runtime::Handle) -> Result<()> {
    // The existing fixture budget covers setup and gate observation together.
    // Register, runtime RPC and Arm retain their own production deadlines.
    let fixture_deadline = tokio::time::Instant::now() + FIXTURE_BUDGET;
    trace_phase("setup_enter");
    let mut fixture = handle.block_on(Fixture::new_with_writer_timeout(Duration::from_secs(60)));
    // A failed child preserves evidence until all its native threads exit with
    // that exact process. Cleanup is reenabled only after successful IPC join.
    fixture.root.disable_cleanup(true);
    let root = Directory::open_existing(&fixture.root.path().canonicalize()?)?;
    let program = engine_program(root.path())?;
    let runtime = Runtime::with_program(RuntimeKind::Docker, &program)?;
    let authority: RuntimeAuthority = serde_json::from_value(json!({
        "authority_id":Uuid::new_v4(), "kind":"docker", "endpoint":"unix:///export-fixture.sock",
        "engine_id":ENGINE, "invocation":runtime.invocation(), "podman":null
    }))?;
    let installation = fixture.scope.installation_id();
    let serving_root = root.clone();
    let mut server = handle.spawn(async move {
        run_internal_full3_fresh(&serving_root, "role", installation, 8).await
    });
    let full = handle.block_on(async {
        let expires = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(Some(role)) = root.persistent_child("role", false)
                && load_endpoint(&role).ok().flatten().is_some()
            {
                break Ok::<_, anyhow::Error>(
                    EnforcerFullClientV3::connect_existing(&role, 8).await?,
                );
            }
            ensure!(
                !server.is_finished(),
                "owned Full3 server exited during setup"
            );
            ensure!(
                tokio::time::Instant::now() < expires,
                "owned Full3 IPC not published"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })?;
    let executor = BatchExecutor::new(
        fixture.writer().clone(),
        ExecutionEnforcerV1::Full3(full.clone()),
        Arc::new(NativeHostClock::new()?),
    );
    let context = handle.block_on(AuthenticatedBatchContext::for_executor_test(
        fixture.client.clone(),
        fixture.scope.clone(),
        fixture.config.clone(),
        runtime.clone(),
    ))?;
    let operation = Uuid::new_v4();
    let accepted = fixture.accept(operation, false);
    let execution = handle.block_on(Execution::open(&executor, operation, &context))?;
    let worker = seed_terminal_parent(
        handle,
        &execution,
        &fixture,
        &accepted,
        &authority,
        TerminalParentAdmission::Executor,
    )?;
    ensure!(
        worker.state == WorkerStateV1::Terminal && !worker.quota_held,
        "positive parent terminal precondition missing"
    );
    ensure!(
        !handle.block_on(execution.load())?.1.cancelled(),
        "fixture must have no durable Cancel"
    );
    let (target, policy, cid, mut export_root, destination) = prepare_export(
        handle, &execution, &fixture, &root, &worker, &authority, runtime, &program,
    )?;
    fs::remove_file(root.path().join("inspected"))?;
    fs::write(root.path().join("gate-enabled"), b"owned-fixture")?;
    fs::write(root.path().join("calls"), b"")?;
    let worker_before = canonical(&fixture.store.worker(&worker.worker_id)?)?;
    let quota_before = canonical(&fixture.store.worker_quota(&QuotaPolicyV1::default())?)?;
    let usage_before = canonical(&fixture.store.worker_usage()?)?;
    let history_before = handle.block_on(execution.load())?.1.operations;
    let signal_id = Uuid::new_v4();
    let request_id = target.request_id();
    trace_phase("actual_export_entry");
    let export = Box::pin(async {
        let result = execution.export(target).await;
        trace_phase(match &result {
            Ok(ControlFlow::Break(_)) => "actual_export_abandoned",
            Ok(ControlFlow::Continue(_)) => "actual_export_completed",
            Err(_) => "actual_export_failed",
        });
        result
    });
    let signal = Box::pin(async {
        let result = abandon_during_registration(
            &fixture,
            &context,
            &full,
            operation,
            signal_id,
            request_id,
            fixture_deadline,
        )
        .await;
        trace_phase(if result.is_ok() {
            "registration_signal_audited"
        } else {
            "registration_signal_failed"
        });
        result
    });
    let (outcome, signal) = handle.block_on(async { tokio::join!(export, signal) });
    // Both futures have finished. Report the actual Export failure first:
    // a watcher timeout must not replace the cause that prevented its gate.
    let ControlFlow::Break(abandoned) =
        outcome.context("actual Export failed; registration watcher outcome is secondary")?
    else {
        return Err(anyhow!(
            "Export returned ExportCompletion after helper abandonment"
        ));
    };
    let (arm, signal_bytes, signal_seq) = signal?;
    assert_export_abandoned(
        handle,
        &fixture,
        &context,
        &full,
        operation,
        signal_id,
        &arm,
        &abandoned,
        &signal_bytes,
        signal_seq,
        &history_before,
        &worker,
        &worker_before,
        &quota_before,
        &usage_before,
        &policy,
        &cid,
        export_root.path(),
        &destination,
    )?;
    handle.block_on(full.shutdown_if_idle())?;
    handle
        .block_on(async { tokio::time::timeout(Duration::from_secs(10), &mut server).await })???;
    fixture.root.disable_cleanup(false);
    export_root.disable_cleanup(false);
    eprintln!("export-retirement oracle=break_without_export_success");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn prepare_export(
    handle: &tokio::runtime::Handle,
    execution: &Execution<'_>,
    fixture: &Fixture,
    root: &Directory,
    worker: &WorkerV1,
    authority: &RuntimeAuthority,
    runtime: Runtime,
    program: &Path,
) -> Result<(
    ValidatedExportTarget,
    HelperExecutionPolicyV1,
    String,
    tempfile::TempDir,
    PathBuf,
)> {
    let mut export_root = tempfile::Builder::new()
        .prefix("er-dest-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(
            root.path()
                .parent()
                .context("private fixture root has no parent")?,
        )?;
    export_root.disable_cleanup(true);
    let destination = export_root.path().join("fresh");
    let source_root = root.path().join("source");
    fs::create_dir(&source_root)?;
    fs::set_permissions(&source_root, fs::Permissions::from_mode(0o700))?;
    let request_id = Uuid::new_v4();
    let profile = json!({
        "id":fixture.scope.profile_id(), "gateway":fixture.scope.gateway_origin(),
        "tenant_id":fixture.scope.tenant_id(), "control_namespace_uuid":fixture.scope.control_namespace_uuid(),
        "config_dir":fixture.config.path(), "runner_token_file":root.path().join("unused.token"),
        "runtime":"docker", "runtime_program":program, "runtime_endpoint":authority.endpoint,
        "image":"agentsandbox-worker:local", "policy_profile":"strict", "network":"none",
        "allow_rootful_runtime":false, "max_timeout_seconds":600, "allow_capture_output":false,
        "managed_clients":false, "source_roots":[source_root], "export_roots":[export_root.path()]
    });
    // Real target validation/persistence, without claiming registry auth.
    let input_root = root.path().join("export-control");
    fs::create_dir(&input_root)?;
    fs::set_permissions(&input_root, fs::Permissions::from_mode(0o700))?;
    let input_root = Directory::open_existing(&input_root)?;
    let target = ValidatedExportTarget::for_executor_test(
        &input_root,
        profile,
        &fixture.scope,
        execution.operation,
        request_id,
        &destination,
    )?;
    trace_phase("export_target_validated");
    let (export, _) = handle.block_on(execution.rpc(Operation::Export {
        request: ExportRequestV1 {
            version: 1,
            operation_id: request_id.to_string(),
            worker_id: worker.worker_id.clone(),
            attempt_id: worker.attempt.attempt_id.clone(),
            destination_ref: target.destination_ref().into(),
            destination_intent_hash: target.destination_intent_hash().into(),
        },
        response: None,
    }))?;
    trace_phase("export_permission_returned");
    let Operation::Export {
        response: Some(receipt),
        ..
    } = export
    else {
        return Err(anyhow!("real export permission receipt missing"));
    };
    let permission = HelperPermissionV1::Export {
        receipt,
        destination_ref: target.destination_ref().into(),
        destination_intent_hash: target.destination_intent_hash().into(),
    };
    let policy = helper_execution_policy_v1(worker, HelperPurposeV1::Export, request_id)?;
    // Match the real helper lifecycle: a new preparation is first durable
    // without runtime ownership. The checkpoint rejects introducing a new
    // helper already carrying an owned snapshot (checkpoint.rs transition).
    handle.block_on(execution.patch(|draft| {
        draft.helpers.push(HelperPreparationV1::new(
            policy.clone(),
            permission.clone(),
            None,
        )?);
        Ok(())
    }))?;
    trace_phase("export_preparation_persisted");
    let cid = format!("{:064x}", request_id.as_u128());
    fs::write(root.path().join("image-id"), &worker.launch.image_id)?;
    fs::write(root.path().join("inspect-id"), &cid)?;
    fs::write(
        root.path().join("inspect.json"),
        serde_json::to_vec(&helper_inspect(&policy, &cid))?,
    )?;
    let container_handle: ContainerHandle = serde_json::from_value(json!({
        "authority_fingerprint":authority.fingerprint(), "session":worker.launch.session_id,
        "operation":request_id, "id":cid, "name":policy.stable_name, "configuration_fingerprint_version":2
    }))?;
    let managed = handle.block_on(ManagedRuntime::reopen_with_runtime(
        authority.clone(),
        runtime,
    ))?;
    trace_phase("export_runtime_reopened");
    let owned = OwnedContainerSnapshot {
        observation: handle.block_on(managed.observe(&container_handle))?,
        handle: container_handle,
    };
    trace_phase("export_helper_inspected");
    handle.block_on(execution.patch(|draft| {
        let prepared = draft
            .helpers
            .iter_mut()
            .find(|entry| entry.policy().helper_id == policy.helper_id)
            .context("durable export helper preparation missing")?;
        *prepared =
            HelperPreparationV1::new(policy.clone(), permission.clone(), Some(owned.clone()))?;
        Ok(())
    }))?;
    Ok((target, policy, cid, export_root, destination))
}

#[allow(clippy::too_many_arguments)]
fn assert_export_abandoned(
    handle: &tokio::runtime::Handle,
    fixture: &Fixture,
    context: &AuthenticatedBatchContext,
    full: &EnforcerFullClientV3,
    operation: Uuid,
    signal_id: Uuid,
    arm: &ArmV1,
    abandoned: &AbandonedBatch,
    signal_bytes: &[u8],
    signal_seq: i64,
    history_before: &[Operation],
    worker: &WorkerV1,
    worker_before: &[u8],
    quota_before: &[u8],
    usage_before: &[u8],
    policy: &HelperExecutionPolicyV1,
    cid: &str,
    export_root: &Path,
    destination: &Path,
) -> Result<()> {
    let arm_id = Uuid::parse_str(&arm.operation_id)?;
    ensure!(
        abandoned.retirement.operation_id() == operation
            && abandoned.retirement.arm_id() == arm_id
            && abandoned.owned.handle.id() == cid,
        "Export changed the helper's actual abandonment identity"
    );
    let loaded = fixture
        .journal()
        .load(operation)?
        .context("export job missing")?;
    let checkpoint = loaded
        .broker_checkpoint_v5()
        .context("retirement V5 missing")?;
    ensure!(
        checkpoint.retirements().len() == 1,
        "retirement request count changed"
    );
    let entry = &checkpoint.retirements()[0];
    let pending = entry.request();
    let finalized = entry
        .finalized()
        .context("Export Break lacks confirmed retirement F")?;
    let intention_ack = pending
        .audit_ack()
        .context("retirement intention ACK missing")?;
    ensure!(
        *intention_ack.event_id() == pending.intention_event_id()
            && intention_ack.input_hash() == pending.intent_input_hash(),
        "retirement intention ACK changed its fixed event identity or hash"
    );
    ensure!(
        *pending.identity().retirement_id() == signal_id
            && canonical(pending.identity().arm())? == canonical(arm)?
            && *pending.identity().original_epoch() == full.epoch()
            && pending.identity().arm().deadline_ns_decimal == arm.deadline_ns_decimal,
        "Export regenerated retirement identity, Arm, epoch or deadline"
    );
    let final_birth = loaded
        .outbox_receipt(pending.final_event_id(), None)
        .context("retirement final event birth missing")?;
    ensure!(
        abandoned.retirement.final_revision() == final_birth.head(),
        "Export Break lost the exact durable retirement F head"
    );
    let native_q = handle
        .block_on(full.query_retirement(pending.identity()))?
        .context("authentic native Q missing")?;
    ensure!(
        native_q.receipt().digest()? == finalized.receipt().digest()?,
        "Export retirement F differs from the actual native Q"
    );
    ensure!(
        !full.has_local_slot(&arm.operation_id)?
            && handle.block_on(full.status())?.slot_count() == 0,
        "Export retained original capacity after completed retirement"
    );
    let view = context.preclaim().arm_view(arm_id);
    ensure!(
        view.state == PreclaimStateV1::Retiring
            && view.retirement_continuation_retained == Some(false),
        "Export discarded or retained the wrong retirement continuation"
    );
    let events = fixture.store.events(&EventQuery {
        limit: Some(1000),
        ..EventQuery::default()
    })?;
    for kind in [
        "broker.batch.export",
        "workspace.export.policy",
        "broker.batch.result",
    ] {
        ensure!(
            !events.iter().any(|row| row.event.kind == kind)
                && !checkpoint
                    .outbox()
                    .events()
                    .iter()
                    .any(|event| event.event().kind == kind),
            "abandoned Export emitted a success/result fact"
        );
    }
    let signal_rows: Vec<_> = events
        .iter()
        .filter(|row| row.event.kind == SIGNAL)
        .collect();
    ensure!(
        signal_rows.len() == 1
            && signal_rows[0].seq == signal_seq
            && canonical(&signal_rows[0].event)? == signal_bytes,
        "Export changed or duplicated the original audited abandon signal"
    );
    for (kind, event_id) in [
        (INTENTION, pending.intention_event_id()),
        (FINAL, pending.final_event_id()),
    ] {
        let rows: Vec<_> = events.iter().filter(|row| row.event.kind == kind).collect();
        ensure!(
            rows.len() == 1 && rows[0].event.id == event_id.to_string(),
            "Export retirement audit identity changed"
        );
        if kind == INTENTION {
            ensure!(
                *intention_ack.audit_seq() == rows[0].seq,
                "retirement intention ACK does not bind the actual SQLite sequence"
            );
        }
        ensure!(
            checkpoint
                .outbox()
                .events()
                .iter()
                .any(|event| event.event_id() == event_id
                    && event.audit_seq() == Some(rows[0].seq)
                    && canonical(event.event()).ok() == canonical(&rows[0].event).ok()),
            "Export retirement outbox differs from real committed audit"
        );
    }
    let operations = loaded
        .broker_checkpoint_v2()
        .context("supervision history missing")?
        .supervision()
        .operations();
    ensure!(
        operations.len() >= history_before.len()
            && canonical(&&operations[..history_before.len()])? == canonical(&history_before)?,
        "Export rewrote prior admission/claims/terminal history"
    );
    ensure!(
        !operations[history_before.len()..].iter().any(|op| matches!(
            op,
            Operation::WorkerClaim { .. }
                | Operation::HelperClaim { .. }
                | Operation::HelperObservation { .. }
                | Operation::HelperCleanup { .. }
        )),
        "abandoned helper claimed, observed or cleaned up execution"
    );
    ensure!(
        worker_before == canonical(&fixture.store.worker(&worker.worker_id)?)?
            && quota_before == canonical(&fixture.store.worker_quota(&QuotaPolicyV1::default())?)?
            && usage_before == canonical(&fixture.store.worker_usage()?)?,
        "Export altered central terminal, quota or prior usage"
    );
    ensure!(
        !destination.try_exists()?,
        "abandoned Export extracted a destination"
    );
    ensure!(
        fs::read_dir(export_root)?.next().is_none(),
        "abandoned Export left an extraction staging tree"
    );
    let calls = fs::read_to_string(fixture.root.path().join("calls"))?;
    let image_inspection = format!("image inspect {}", worker.launch.image_id);
    let container_inspection = format!("inspect {cid}");
    ensure!(
        calls.lines().any(|line| line == container_inspection)
            && calls.lines().any(|line| line == image_inspection)
            && calls.lines().all(|line| {
                line == "info" || line == image_inspection || line == container_inspection
            }),
        "Export failed to inspect or emitted a runtime effect after abandonment"
    );
    ensure!(
        fs::read(fixture.root.path().join("inspect.json"))?
            == serde_json::to_vec(&helper_inspect(policy, cid))?,
        "abandoned Export rewrote the synthetic Created state"
    );
    Ok(())
}

async fn abandon_during_registration(
    fixture: &Fixture,
    context: &AuthenticatedBatchContext,
    full: &EnforcerFullClientV3,
    operation: Uuid,
    command: Uuid,
    helper: Uuid,
    fixture_deadline: tokio::time::Instant,
) -> Result<(ArmV1, Vec<u8>, i64)> {
    while !fixture
        .root
        .path()
        .join("registration-blocked")
        .try_exists()?
    {
        ensure!(
            tokio::time::Instant::now() < fixture_deadline,
            "actual helper registration did not reach its owned runtime gate within the fixture budget"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let job = fixture
        .writer()
        .load(operation)
        .await?
        .context("registered job missing")?;
    let arm = job.broker_checkpoint_v2().context("registered helper supervision missing")?
        .supervision().operations().iter().find_map(|op| match op {
            Operation::Arm { request, response:Some(_), .. }
                if matches!(&request.subject, ArmSubjectV1::Helper { helper_id, purpose:HelperPurposeV1::Export, .. }
                    if helper_id == &helper.to_string()) => Some(request.clone()),
            _ => None,
        }).context("real acknowledged Export helper Arm missing at registration gate")?;
    let arm_id = Uuid::parse_str(&arm.operation_id)?;
    ensure!(
        context.preclaim().arm_view(arm_id).state == PreclaimStateV1::RegistrationInFlight,
        "abandon did not run during actual registration"
    );
    let signal = context.preclaim().request(AbandonBeforeClaimV1 {
        version: 1,
        operation_id: operation,
        arm_id,
        command_id: command,
        expected_epoch: full.epoch(),
        generation: context.expected_generation(),
    })?;
    let mut event = signal.event().clone();
    let request = &job.checkpoint().0.current_request;
    event.session_id = request.session_id.clone();
    event.project_id = request.project_id.clone();
    event.sandbox_id = request.sandbox_id.clone();
    event.agent_id = request.agent_id.clone();
    event.correlation_id = request.correlation_id.clone();
    event.trace_id = request.trace_id.clone();
    let bytes = canonical(&event)?;
    let birth = fixture
        .writer()
        .enqueue_event(operation, event.clone())
        .await?;
    let seq = fixture
        .client
        .post_control_event(serde_json::to_value(&event)?)
        .await?;
    let ack = fixture
        .writer()
        .acknowledge_event(operation, command, seq)
        .await?;
    ensure!(
        ack.audit_seq() == Some(seq) && ack.input_hash() == birth.input_hash(),
        "abandon signal lost its actual audit acknowledgement"
    );
    signal.confirm_audit()?;
    fs::write(fixture.root.path().join("release-registration"), b"audited")?;
    Ok((arm, bytes, seq))
}

enum WorkerCleanupAckCut {
    StoreResponse,
    WriterReply,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_cleanup_replays_the_exact_store_receipt_after_lost_http_response() -> Result<()> {
    let handle = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        worker_cleanup_ack_cut(&handle, WorkerCleanupAckCut::StoreResponse)
    })
    .await?
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_cleanup_retains_the_published_ack_after_lost_writer_reply() -> Result<()> {
    let handle = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        worker_cleanup_ack_cut(&handle, WorkerCleanupAckCut::WriterReply)
    })
    .await?
}

fn worker_cleanup_ack_cut(handle: &tokio::runtime::Handle, cut: WorkerCleanupAckCut) -> Result<()> {
    use super::super::without_response;

    let fixture = handle.block_on(Fixture::new());
    // Pin an executable as data for the authority's identity. No test path
    // launches it, asks for a live guardian or performs a runtime removal.
    let runtime = Runtime::with_program(RuntimeKind::Docker, Path::new(INVOCATION_DATA_PROGRAM))?;
    let authority: RuntimeAuthority = serde_json::from_value(json!({
        "authority_id": Uuid::new_v4(), "kind": "docker", "endpoint": "unix:///cleanup-oracle.sock",
        "engine_id": ENGINE, "invocation": runtime.invocation(), "podman": null
    }))?;
    let context = handle.block_on(AuthenticatedBatchContext::for_executor_test(
        fixture.client.clone(),
        fixture.scope.clone(),
        fixture.config.clone(),
        runtime,
    ))?;
    let writer = fixture.writer();
    let executor = BatchExecutor::new(
        writer.clone(),
        EnforcerClient::unavailable_for_test(&fixture.config)?,
        Arc::new(NativeHostClock::new()?),
    );
    let operation = Uuid::new_v4();
    let accepted = fixture.accept(operation, false);
    let execution = handle.block_on(Execution::open(&executor, operation, &context))?;
    let worker = seed_terminal_parent(
        handle,
        &execution,
        &fixture,
        &accepted,
        &authority,
        TerminalParentAdmission::StoreFixture,
    )?;
    ensure!(
        worker.state == WorkerStateV1::Terminal
            && !worker.quota_held
            && worker.cleanup_removed.is_none(),
        "the worker must have a durable terminal and no cleanup result yet"
    );
    let allocation = fixture.store.worker_usage()?;
    ensure!(
        allocation.len() == 1,
        "the seed must have one original terminal allocation"
    );
    let store_head = fixture.store.head()?;
    let (before, initial) = handle.block_on(execution.load())?;
    let generation = context.cancel_generation().current();
    let request = WorkerMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        expected_revision: worker.revision,
        transition: WorkerTransitionV1::CleanupResult {
            removed: true,
            error: None,
        },
    };
    let pending = Operation::WorkerMutation {
        request: request.clone(),
        response: None,
    };
    let published = match cut {
        WorkerCleanupAckCut::StoreResponse => {
            fixture
                .drop_next_worker_transition_response
                .store(true, Ordering::SeqCst);
            let error = handle
                .block_on(boxed_execution_phase(|| execution.rpc(pending.clone())))
                .err()
                .context("the committed HTTP body was unexpectedly received")?;
            ensure!(
                !fixture
                    .drop_next_worker_transition_response
                    .load(Ordering::SeqCst)
                    && error.downcast_ref::<reqwest::Error>().is_some(),
                "the post-commit response cut did not reach the HTTP client"
            );
            false
        }
        WorkerCleanupAckCut::WriterReply => {
            // Save Pending before arming the generic barrier so that it
            // targets the next advance, which appends this exact response.
            handle.block_on(execution.patch(|draft| {
                draft.operations.push(pending.clone());
                Ok(())
            }))?;
            let mut barrier = handle.block_on(writer.advance_reply_barrier_for_test(true))?;
            let error = handle.block_on(async {
                let mut attempt = boxed_execution_phase(|| execution.rpc(pending.clone()));
                tokio::select! {
                    _ = &mut attempt => return Err(anyhow!("Cleanup did not reach its Writer ACK barrier")),
                    () = barrier.entered() => {}
                }
                drop(barrier);
                attempt.await.err().context("the lost Writer reply was treated as acknowledged")
            })?;
            ensure!(
                error.downcast_ref::<JournalError>() == Some(&JournalError::Io),
                "the exact Writer ACK cut did not report an uncertain reply"
            );
            true
        }
    };
    let store_receipt = fixture
        .store
        .worker_operation(&request.operation_id)?
        .context("the exact CleanupResult did not commit in the Store")?;
    ensure!(
        store_receipt.audit_seq == store_head + 1
            && store_receipt.worker_id.as_deref() == Some(worker.worker_id.as_str())
            && store_receipt.revision == worker.revision.checked_add(1)
            && store_receipt.input_hash == content_hash("ai-guard:worker-mutation:v1", &request)?,
        "Store cleanup receipt differs from the original request"
    );
    let (cut_head, cut_draft) = handle.block_on(execution.load())?;
    let saved = cut_draft
        .operations
        .last()
        .context("cleanup missing at the cut")?;
    ensure!(
        same(&without_response(saved), &pending)?
            && cut_head.head().revision()
                == before.head().revision() + if published { 2 } else { 1 },
        "the cut did not preserve the original durable cleanup request"
    );
    match saved {
        Operation::WorkerMutation {
            response: Some(ack),
            ..
        } if published => {
            ensure!(
                ack.receipt == store_receipt,
                "the durable Writer ACK must contain the Store receipt"
            );
        }
        Operation::WorkerMutation { response: None, .. } if !published => {}
        _ => bail!("the checkpoint does not distinguish the HTTP cut from a published Writer ACK"),
    }
    if published {
        ensure!(
            matches!(
                handle.block_on(writer.result_status(operation)),
                Err(JournalError::Corrupt)
            ),
            "the Writer result guard must stay closed after its lost acknowledgement"
        );
    }
    drop(execution);

    let resumed = handle.block_on(Execution::open(&executor, operation, &context))?;
    handle.block_on(boxed_execution_phase(|| resumed.reconcile_pending()))?;
    let (closed, after) = handle.block_on(resumed.load())?;
    let Some(Operation::WorkerMutation {
        request: replayed,
        response: Some(ack),
    }) = after.operations.last()
    else {
        bail!("reconciliation did not acknowledge the original cleanup");
    };
    ensure!(
        same(replayed, &request)?
            && ack.receipt == store_receipt
            && ack.worker.attempt == worker.attempt
            && ack.worker.state == WorkerStateV1::Terminal
            && ack.worker.cleanup_removed == Some(true)
            && !ack.worker.quota_held
            && closed.head().revision() == before.head().revision() + 2
            && (!published || closed.head() == cut_head.head())
            && context.cancel_generation().current() == generation
            && after
                .run
                .plan
                .managed
                .as_ref()
                .and_then(|managed| managed.binding.as_ref())
                == Some(&authority)
            && after.operations.len() == initial.operations.len() + 1
            && same(
                &after.operations[..initial.operations.len()].to_vec(),
                &initial.operations
            )?
            && same(&after.commands, &initial.commands)?
            && fixture.store.worker_operation(&request.operation_id)?
                == Some(store_receipt.clone())
            && fixture.store.head()? == store_receipt.audit_seq
            && fixture.store.worker_usage()? == allocation,
        "reconciliation changed authority, allocation or the exact cleanup result"
    );
    Ok(())
}

enum TerminalParentAdmission {
    Executor,
    StoreFixture,
}

fn seed_terminal_parent(
    handle: &tokio::runtime::Handle,
    execution: &Execution<'_>,
    fixture: &Fixture,
    accepted: &AcceptedJobV1,
    authority: &RuntimeAuthority,
    mode: TerminalParentAdmission,
) -> Result<WorkerV1> {
    let request = accepted.checkpoint().0.current_request.clone();
    let (answer, _) = handle.block_on(execution.rpc(Operation::Decision {
        request: request.clone(),
        response: None,
    }))?;
    trace_phase("parent_decision_returned");
    let Operation::Decision {
        response: Some(answer),
        ..
    } = answer
    else {
        return Err(anyhow!("decision missing"));
    };
    ensure!(
        answer.decision().outcome == Outcome::Allow,
        "fixture decision denied"
    );
    let admission = AdmissionV1 {
        version: 1,
        operation_id: accepted.operation_id().to_string(),
        decision_request: request.clone(),
        receipt: answer.receipt().clone(),
        launch: LaunchSpecV1 {
            version: 1,
            session_id: request.session_id.clone().context("session missing")?,
            command: request.command.clone(),
            profile: request.profile.clone(),
            runtime_kind: RuntimeKindV1::Docker,
            authority_id: authority.authority_id.to_string(),
            image_ref: "agentsandbox-worker:local".into(),
            image_id: format!("sha256:{}", "a".repeat(64)),
            network: NetworkV1::None,
            cpu_millis: 2000,
            memory_bytes: 1_073_741_824,
            pids_limit: 256,
            timeout_seconds: 600,
            security_preset: SecurityPresetV1::FixedSandboxV2,
            restart_policy: RestartPolicyV1::No,
            workspace_manifest_digest: content_hash(
                "ai-guard:workspace-manifest:v1",
                &BTreeMap::<String, workspace::Fingerprint>::new(),
            )?,
            workspace_import_policy_version: 1,
            provider: "generic".into(),
            capture_output: false,
            credential_binding_id: None,
            credential_names: vec![],
            provider_resolution: None,
        },
    };
    let prepared: managed::ManagedCheckpoint = serde_json::from_value(json!({
        "tenant_id":TENANT, "gateway_origin":fixture.scope.gateway_origin(), "config_root":fixture.config.path(),
        "binding":authority, "authority_file":"runtime-authority.json", "admission":admission,
        "worker":null, "owned":null, "pending":null, "uncertain":[], "lock_identity":null,
        "before":{}, "import":null, "decision":answer.decision(), "output_event":null,
        "output_acknowledged":false, "result":null, "archive_name":"worker.tar", "archive_digest":"e".repeat(64),
        "credential_digest":null, "workspace_control_exclusion_version":1, "invocation_event":null
    }))?;
    let pending = Operation::Admission {
        request: SupervisedAdmissionV1 {
            version: 1,
            protocol: SupervisionProtocolV1::BrokerSupervisionV1,
            intent_hash: accepted.intent_hash().into(),
            admission,
        },
        response: None,
    };
    handle.block_on(execution.patch(|draft| {
        draft.run.plan.managed = Some(prepared.clone());
        draft.operations.push(pending.clone());
        Ok(())
    }))?;
    trace_phase("parent_admission_prepared");
    let admitted = match mode {
        TerminalParentAdmission::Executor => handle.block_on(execution.rpc(pending))?.0,
        TerminalParentAdmission::StoreFixture => {
            // This test seeds the real Store and journal with a typed
            // admission. It does not exercise executor admission or claim a
            // guardian was ready, and cannot emit a runtime start.
            let Operation::Admission { request, .. } = pending else {
                bail!("pending fixture admission missing");
            };
            let receipt = fixture.store.admit_supervised(
                &request,
                &Policy::default(),
                answer.decision().clone(),
                Value::Null,
                &QuotaPolicyV1::default(),
            )?;
            let worker = receipt
                .worker
                .clone()
                .context("fixture admission did not reserve a worker")?;
            let complete = Operation::Admission {
                request,
                response: Some(receipt),
            };
            handle.block_on(execution.patch(|draft| {
                let entry = draft
                    .operations
                    .last_mut()
                    .context("fixture pending admission missing")?;
                *entry = complete.clone();
                draft
                    .run
                    .plan
                    .managed
                    .as_mut()
                    .context("managed preparation missing")?
                    .worker = Some(worker.clone());
                Ok(())
            }))?;
            complete
        }
    };
    trace_phase("parent_admission_returned");
    let Operation::Admission {
        response: Some(admitted),
        ..
    } = admitted
    else {
        return Err(anyhow!("admission missing"));
    };
    let worker = handle.block_on(execution.mutate(WorkerTransitionV1::Prepare))?;
    trace_phase("parent_prepare_returned");
    let (import, import_arm, import_terminal) = seed_prior_helper(
        handle,
        execution,
        &worker,
        authority,
        accepted.intent_hash(),
        HelperPurposeV1::Import,
        HelperPermissionV1::Admission {
            receipt: admitted.receipt,
        },
        None,
    )?;
    trace_phase("parent_import_seeded");
    seed_prior_helper(
        handle,
        execution,
        &worker,
        authority,
        accepted.intent_hash(),
        HelperPurposeV1::ImportVerification,
        HelperPermissionV1::ImportVerification {
            import_helper_id: import.to_string(),
            import_arm,
            import_terminal,
            manifest_digest: worker.launch.workspace_manifest_digest.clone(),
        },
        Some(ImportVerificationReportV1 {
            import_helper_id: import.to_string(),
            expected_manifest_digest: worker.launch.workspace_manifest_digest.clone(),
            observed_manifest_digest: worker.launch.workspace_manifest_digest.clone(),
        }),
    )?;
    trace_phase("parent_verification_seeded");
    let worker = handle.block_on(execution.mutate(WorkerTransitionV1::CreateIssued))?;
    let inspection = worker_inspection(&worker);
    let worker = handle.block_on(execution.mutate(WorkerTransitionV1::Created {
        inspection: inspection.clone(),
    }))?;
    trace_phase("parent_created_returned");
    let owned = synthetic_owned(&worker, authority, accepted.operation_id(), &inspection);
    handle.block_on(execution.patch(|draft| {
        draft
            .run
            .plan
            .managed
            .as_mut()
            .context("managed preparation missing")?
            .owned = Some(owned.clone());
        Ok(())
    }))?;
    let arm = prior_arm(
        &worker,
        authority,
        accepted.intent_hash(),
        inspection.clone(),
        ArmSubjectV1::Worker,
    )?;
    let (armed, _) = handle.block_on(execution.rpc(Operation::Arm {
        parent: worker.clone(),
        owned,
        request: arm.clone(),
        response: None,
    }))?;
    trace_phase("parent_arm_returned");
    let Operation::Arm {
        response: Some(armed),
        ..
    } = armed
    else {
        return Err(anyhow!("worker arm missing"));
    };
    handle.block_on(execution.rpc(Operation::WorkerClaim {
        request: ArmedStartClaimV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            worker_id: worker.worker_id.clone(),
            attempt_id: worker.attempt.attempt_id.clone(),
            expected_revision: worker.revision,
            emitter_id: Uuid::new_v4().to_string(),
            arm_receipt: armed.receipt,
            clock_check: arm.clock,
        },
        response: None,
    }))?;
    trace_phase("parent_claim_returned");
    let terminal = handle.block_on(execution.mutate(WorkerTransitionV1::Observe {
        observation: terminal_observation(inspection),
    }))?;
    trace_phase("parent_terminal_returned");
    Ok(terminal)
}

#[allow(clippy::too_many_arguments)]
fn seed_prior_helper(
    handle: &tokio::runtime::Handle,
    execution: &Execution<'_>,
    worker: &WorkerV1,
    authority: &RuntimeAuthority,
    intent: &str,
    purpose: HelperPurposeV1,
    permission: HelperPermissionV1,
    verification: Option<ImportVerificationReportV1>,
) -> Result<(Uuid, ArmReceiptV1, MutationReceiptV1)> {
    let helper = Uuid::new_v4();
    let policy = helper_execution_policy_v1(worker, purpose, helper)?;
    handle.block_on(execution.patch(|draft| {
        draft.helpers.push(HelperPreparationV1::new(
            policy.clone(),
            permission.clone(),
            None,
        )?);
        Ok(())
    }))?;
    let inspection = RuntimeInspectionV1 {
        authority_id: policy.authority_id.clone(),
        runtime_kind: policy.runtime_kind,
        container_id: format!("{:064x}", helper.as_u128()),
        stable_name: policy.stable_name.clone(),
        volume_name: policy.volume_name.clone(),
        image_id: policy.image_id.clone(),
        spec_hash: policy.spec_hash()?,
        cpu_millis: policy.cpu_millis,
        memory_bytes: policy.memory_bytes,
        pids_limit: policy.pids_limit,
        network: policy.network,
        security_preset: policy.security_preset,
        restart_policy: policy.restart_policy,
    };
    let owned = synthetic_owned(worker, authority, helper, &inspection);
    handle.block_on(execution.patch(|draft| {
        *draft
            .helpers
            .last_mut()
            .context("prior helper preparation missing")? =
            HelperPreparationV1::new(policy.clone(), permission.clone(), Some(owned.clone()))?;
        Ok(())
    }))?;
    let arm = prior_arm(
        worker,
        authority,
        intent,
        inspection.clone(),
        ArmSubjectV1::Helper {
            helper_id: helper.to_string(),
            purpose,
            policy_version: 1,
            permission: Box::new(permission),
        },
    )?;
    let (armed, _) = handle.block_on(execution.rpc(Operation::Arm {
        parent: worker.clone(),
        owned,
        request: arm.clone(),
        response: None,
    }))?;
    let Operation::Arm {
        response: Some(armed),
        ..
    } = armed
    else {
        return Err(anyhow!("prior helper arm missing"));
    };
    handle.block_on(execution.rpc(Operation::HelperClaim {
        request: HelperStartClaimV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            helper_id: helper.to_string(),
            emitter_id: Uuid::new_v4().to_string(),
            arm_receipt: armed.receipt.clone(),
            clock_check: arm.clock,
        },
        response: None,
    }))?;
    let (observed, _) = handle.block_on(execution.rpc(Operation::HelperObservation {
        request: HelperObservationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            helper_id: helper.to_string(),
            arm_receipt: armed.receipt.clone(),
            observation: terminal_observation(inspection),
            verification,
        },
        response: None,
    }))?;
    let Operation::HelperObservation {
        response: Some(receipt),
        ..
    } = observed
    else {
        return Err(anyhow!("prior helper terminal missing"));
    };
    Ok((helper, armed.receipt, receipt))
}

fn worker_inspection(worker: &WorkerV1) -> RuntimeInspectionV1 {
    RuntimeInspectionV1 {
        authority_id: worker.launch.authority_id.clone(),
        runtime_kind: worker.launch.runtime_kind,
        container_id: "d".repeat(64),
        stable_name: worker.attempt.stable_name.clone(),
        volume_name: worker.attempt.volume_name.clone(),
        image_id: worker.launch.image_id.clone(),
        spec_hash: worker.spec_hash.clone(),
        cpu_millis: worker.launch.cpu_millis,
        memory_bytes: worker.launch.memory_bytes,
        pids_limit: worker.launch.pids_limit,
        network: worker.launch.network,
        security_preset: worker.launch.security_preset,
        restart_policy: worker.launch.restart_policy,
    }
}
fn terminal_observation(inspection: RuntimeInspectionV1) -> RuntimeObservationV1 {
    RuntimeObservationV1 {
        inspection,
        observed_at: FINISHED.into(),
        status: RuntimeStatusV1::Exited,
        running: Some(false),
        started_at: Some(STARTED.into()),
        finished_at: Some(FINISHED.into()),
        exit_code: Some(0),
        oom_killed: Some(false),
        restart_count: Some(0),
    }
}
fn prior_arm(
    worker: &WorkerV1,
    authority: &RuntimeAuthority,
    intent: &str,
    inspection: RuntimeInspectionV1,
    subject: ArmSubjectV1,
) -> Result<ArmV1> {
    let clock = NativeHostClock::new()?.sample()?;
    let timeout = if matches!(subject, ArmSubjectV1::Worker) {
        worker.launch.timeout_seconds
    } else {
        120
    };
    Ok(ArmV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        expected_revision: worker.revision,
        intent_hash: intent.into(),
        subject,
        configuration_fingerprint_version: 2,
        inspection,
        authority_binding_hash: authority.fingerprint(),
        configuration_fingerprint: "c".repeat(64),
        deadline_ns_decimal: checked_deadline(clock.continuous_ns()?, timeout)?.to_string(),
        clock,
        timeout_seconds: timeout,
    })
}
fn synthetic_owned(
    worker: &WorkerV1,
    authority: &RuntimeAuthority,
    operation: Uuid,
    inspection: &RuntimeInspectionV1,
) -> OwnedContainerSnapshot {
    serde_json::from_value(json!({
        "handle":{"authority_fingerprint":authority.fingerprint(), "session":worker.launch.session_id, "operation":operation,
            "id":inspection.container_id, "name":inspection.stable_name, "configuration_fingerprint_version":2},
        "observation":{"id":inspection.container_id, "name":inspection.stable_name, "image_id":inspection.image_id, "restart_count":0,
            "process":{"status":"created", "running":false, "started_at":null, "finished_at":null, "exit_code":null, "oom_killed":false},
            "allocation":{"cpu":{"quota":u64::from(inspection.cpu_millis)*100, "period":100000}, "memory_bytes":inspection.memory_bytes, "pids_limit":inspection.pids_limit},
            "configuration_fingerprint":"c".repeat(64), "configuration_fingerprint_version":2, "observed_at":STARTED, "diagnostics":[]}
    })).unwrap()
}

fn helper_inspect(policy: &HelperExecutionPolicyV1, cid: &str) -> Value {
    json!({
        "Id":cid, "Name":format!("/{}", policy.stable_name), "Image":policy.image_id, "RestartCount":0,
        "State":{"Status":"created", "Running":false, "StartedAt":"0001-01-01T00:00:00Z", "FinishedAt":"0001-01-01T00:00:00Z", "ExitCode":0, "OOMKilled":false},
        "Config":{"Labels":{"agentsandbox.session":policy.session_id, "agentsandbox.operation":policy.helper_id, "agentsandbox.purpose":"export"},
            "User":"1000:1000", "WorkingDir":"/workspace", "Entrypoint":&policy.command.argv[..1], "Cmd":&policy.command.argv[1..],
            "Tty":false, "OpenStdin":false, "StdinOnce":false},
        "HostConfig":{"CpuQuota":200000, "CpuPeriod":100000, "Memory":1073741824_u64, "PidsLimit":256,
            "Privileged":false, "ReadonlyRootfs":true, "NetworkMode":"none", "CapDrop":["ALL"], "CapAdd":[],
            "SecurityOpt":["no-new-privileges"], "RestartPolicy":{"Name":"no"}, "AutoRemove":false,
            "Tmpfs":{"/tmp":"rw,nosuid,nodev,noexec,size=268435456,mode=1777", "/home/agent":"rw,nosuid,nodev,size=67108864,mode=700,uid=1000,gid=1000"},
            "LogConfig":{"Type":"none", "Config":{}}},
        "Mounts":[{"Type":"volume", "Name":policy.volume_name, "Destination":"/workspace", "RW":true}]
    })
}

fn engine_program(root: &Path) -> Result<PathBuf> {
    // Export reopens its authority, diagnoses rootless info, resolves the exact
    // admitted image, then inspects the recovered Created helper before Arm /
    // Register. Only that container inspection marks `inspected`: the native
    // Full3 actor's next reopen/info is the owned registration gate. These are
    // the fixture's three exact read commands; every other invocation fails.
    let directory = root.join("engine");
    fs::create_dir(&directory)?;
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
    let source = directory.join("fixture.rs");
    fs::write(
        &source,
        r#"
use std::{fs, io::Write, time::{Duration, Instant}};
fn forbidden(calls: &mut fs::File) -> ! {
    writeln!(calls, "forbidden").unwrap();
    std::process::exit(125);
}
fn main() {
    let root = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().to_owned();
    let all: Vec<String> = std::env::args().skip(1).collect();
    let mut calls = fs::OpenOptions::new().append(true).create(true).open(root.join("calls")).unwrap();
    let args = match all.as_slice() {
        [flag, endpoint, args @ ..]
            if flag == "--host" && endpoint == "unix:///export-fixture.sock" => args,
        _ => forbidden(&mut calls),
    };
    let args = args.iter().map(String::as_str).collect::<Vec<_>>();
    match args.as_slice() {
        ["info", "--format", "{{json .}}"] => {
            writeln!(calls, "info").unwrap();
            if root.join("gate-enabled").exists() && root.join("inspected").exists() {
                fs::write(root.join("registration-blocked"), b"native-info").unwrap();
                let expires = Instant::now() + Duration::from_secs(20);
                while !root.join("release-registration").exists() {
                    if Instant::now() >= expires { std::process::exit(124); }
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
            // Synthetic info metadata, not evidence of a real rootless engine.
            println!("{{\"ID\":\"export-retirement-synthetic-engine\",\"SecurityOptions\":[\"name=rootless\"]}}");
        }
        ["image", "inspect", "--format", "{{.Id}}", image] => {
            let expected = fs::read_to_string(root.join("image-id")).unwrap();
            if *image != expected.as_str() { forbidden(&mut calls); }
            writeln!(calls, "image inspect {image}").unwrap();
            println!("{expected}");
        }
        ["inspect", "--format", "{{json .}}", id] => {
            let expected = fs::read_to_string(root.join("inspect-id")).unwrap();
            if *id != expected.as_str() { forbidden(&mut calls); }
            writeln!(calls, "inspect {id}").unwrap();
            fs::write(root.join("inspected"), b"owned-helper").unwrap();
            print!("{}", fs::read_to_string(root.join("inspect.json")).unwrap());
        }
        _ => forbidden(&mut calls),
    }
}
"#,
    )?;
    let program = directory.join("fixture");
    let rustc = std::env::var_os("RUSTC").context("missing export fixture compiler handoff")?;
    ensure!(
        Path::new(&rustc).is_absolute(),
        "fixture compiler path is relative"
    );
    let result = Command::new(rustc)
        .args([
            "--edition=2024",
            "--crate-name",
            "export_retirement_engine_fixture",
        ])
        .arg(source)
        .arg("-o")
        .arg(&program)
        .output()?;
    ensure!(
        result.status.success(),
        "owned synthetic Rust engine fixture failed to compile"
    );
    Ok(program)
}
