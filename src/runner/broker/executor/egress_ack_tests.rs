//! The real E3 callback runs below the closed service admission port.
//! Runtime observations and native release authority in this fixture are synthetic.

use super::*;
use crate::runner::broker::executor::egress::{
    EGRESS_OPENING_PROGRESS, EgressOpeningProgress, EgressWitnessWriterFailure,
};
use crate::{
    broker::{
        egress_owner::resolve::{HostLookup, LookupFuture, Resolver},
        journal::ReservationMode,
        writer::EgressReleaseAckWitness,
    },
    control::egress_decision::{
        EgressChannelOpeningDispositionV1, EgressChannelOpeningReceiptV1, EgressObservationV1,
        EgressReceiptV1,
    },
    egress::{channel_protocol as egress_wire, ids::IdV1, proxy_env},
    runner::{RunOptions, broker::prepare_broker_submission_with_egress},
    runtime::{
        BeforeSpawnGuard, ContainerSpec, ManagedPrearmedOutcome, NetworkMode, PrearmedStartControl,
        bootstrap_stdin::{
            BootstrapStdinSubmitError, NativeBootstrapRelease, NativeConnectGuard,
            NativeConnectRefusal,
        },
    },
};
use std::{
    future::Future,
    process::Stdio,
    sync::{OnceLock, atomic::AtomicBool},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    sync::{Mutex, oneshot, watch},
    task::{JoinHandle, JoinSet},
    time::{Instant, sleep_until, timeout, timeout_at},
};

#[path = "../../../../tests/support/held_egress_witness.rs"]
mod witness;

const ENDPOINT: &str = "unix:///held-egress-fixture.sock";
const PORT: u16 = 3128;
const WAIT: Duration = Duration::from_secs(8);
const WRITER_WAIT: Duration = Duration::from_secs(10);
// AuditClient's request budget. The observation slack belongs to this oracle.
const HTTP_WAIT: Duration = Duration::from_secs(15);
const OBSERVATION_SLACK: Duration = Duration::from_secs(2);
const EXTERNAL_LIMIT: Duration = Duration::from_secs(180);
const CALLBACK_DRAIN: Duration = Duration::from_secs(1);
const STDIN_CUSTODY_WINDOW: Duration = Duration::from_millis(500);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn held_egress_release_waits_for_committed_sqlite_opening_ack() -> Result<()> {
    // Leave time for two controller waits and the peer's single reap window.
    let started = Instant::now();
    let hard_deadline = started + EXTERNAL_LIMIT - WAIT * 3 - OBSERVATION_SLACK;
    let seed_deadline = hard_deadline - CALLBACK_DRAIN - WRITER_WAIT - WAIT;
    println!(
        "e3_seed deadline_ms={}",
        seed_deadline.duration_since(started).as_millis()
    );
    let base = timeout_at(seed_deadline, Fixture::new_with_writer_timeout(WRITER_WAIT))
        .await
        .context("harness seed deadline elapsed opening the base fixture")?;
    let root = base.config.path().parent().context("fixture root")?;
    let mut program = HeldRuntime::new(root, seed_deadline).await?;
    let mut starting_child = None;
    let mut peer = None;
    let result = boxed_execution_phase(|| async {
        let context = seed_before(
            seed_deadline,
            "authenticate",
            AuthenticatedBatchContext::for_executor_test(
                base.client.clone(),
                base.scope.clone(),
                base.config.clone(),
                program.runtime.clone(),
            ),
        )
        .await?;
        seed_before(seed_deadline, "native peer readiness", async {
            peer = Some(NativePeer::for_egress_ack_oracle(&base, WAIT, &mut starting_child).await?);
            Ok(())
        })
        .await?;
        let peer = peer.as_ref().context("missing ready native peer")?;
        boxed_execution_phase(|| {
            run_held_egress_oracle(
                &base,
                &program,
                &context,
                peer,
                started,
                seed_deadline,
                hard_deadline,
            )
        })
        .await
    })
    .await;
    let collection_ms = started.elapsed().as_millis();
    let controller = program.close_and_join().await;
    let peer_stop = match (peer.as_mut(), starting_child.as_mut()) {
        (Some(peer), None) => peer.stop_egress_ack_oracle(),
        (None, Some(NativePeerChild::Egress(child))) => child.stop(),
        (None, None) => Ok(()),
        _ => anyhow::bail!("unexpected E3 peer custody"),
    };
    if result.is_err() || controller.is_err() || peer_stop.is_err() {
        let cleanup = format!(
            "E3 cleanup collection_ms={collection_ms} controller={} peer={}",
            bounded_fixture_outcome(Some(&controller)),
            bounded_fixture_outcome(Some(&peer_stop))
        );
        return match result {
            Ok(()) => Err(anyhow::anyhow!(cleanup)),
            Err(error) => Err(error.context(cleanup)),
        };
    }
    result
}

async fn seed_before<T>(
    deadline: Instant,
    phase: &'static str,
    step: impl Future<Output = Result<T>>,
) -> Result<T> {
    ensure!(
        Instant::now() < deadline,
        "harness seed deadline before {phase}"
    );
    let output = tokio::select! {
        biased;
        () = sleep_until(deadline) => anyhow::bail!("harness seed deadline elapsed in {phase}"),
        result = step => result?,
    };
    ensure!(
        Instant::now() < deadline,
        "harness seed deadline elapsed in {phase}"
    );
    Ok(output)
}

async fn run_held_egress_oracle(
    base: &Fixture,
    program: &HeldRuntime,
    context: &AuthenticatedBatchContext,
    peer: &NativePeer,
    started: Instant,
    seed_deadline: Instant,
    hard_deadline: Instant,
) -> Result<()> {
    let drain_start = hard_deadline - CALLBACK_DRAIN;
    let operation = Uuid::new_v4();
    let clock = Arc::new(NativeHostClock::new()?);
    let executor = BatchExecutor::new(base.writer().clone(), peer.client.clone(), clock.clone())
        .with_egress_resolver(Some(Arc::new(Resolver::new(
            Arc::new(RefuseLookup),
            vec![],
            vec!["127.0.0.1".parse()?],
        )?)));

    let accepted = seed_before(seed_deadline, "reserve and accept", async {
        let prepared = prepare(base, operation, &program.runtime)?;
        let ReserveResult::New(ticket) = base
            .writer()
            .reserve_with_mode(
                operation,
                b"held egress oracle".to_vec(),
                ReservationMode::AllowNew(prepared.intent().egress().cloned()),
            )
            .await?
        else {
            anyhow::bail!("fixture reservation must be fresh");
        };
        base.writer()
            .accept_prepared(ticket, prepared)
            .await
            .map_err(|error| anyhow::anyhow!("fixture acceptance failed: {:?}", error.error()))
    })
    .await?;
    let profile = accepted
        .launch_profile()
        .context("fixture launch profile")?
        .clone();
    let runtime = seed_before(
        seed_deadline,
        "connect software runtime",
        ManagedRuntime::connect_docker_with_runtime(
            Uuid::new_v4(),
            ENDPOINT,
            program.runtime.clone(),
        ),
    )
    .await?
    .bind_docker_launch_profile(profile.clone())?;
    let execution = Execution {
        executor: &executor,
        context,
        operation,
        client: base.client.clone(),
        activity_directory: tokio::sync::OnceCell::new(),
        generation: context.expected_generation(),
        phase: AtomicU8::new(0),
        launch_profile: Some(profile),
    };
    let (worker, owned, original_arm) = seed_before(
        seed_deadline,
        "seed StartIssued",
        boxed_execution_phase(|| {
            seed_start_issued(&execution, &accepted, &runtime, base, &program.facts)
        }),
    )
    .await?;
    let spec = ContainerSpec::for_held_worker(&worker)?;
    let attempt = IdV1::parse(&worker.attempt.attempt_id)?;
    require_original_arm_window(
        clock.as_ref(),
        &original_arm,
        hard_deadline.saturating_duration_since(Instant::now()),
    )?;
    let (stop_owner, stop) = watch::channel(false);
    let (first_tx, mut first_rx) = oneshot::channel();
    let (capture_tx, mut capture_rx) = oneshot::channel();
    let (ready_tx, mut ready_rx) = oneshot::channel();
    seed_before(seed_deadline, "install stream witnesses", async {
        let mut streams = program.streams.lock().await;
        ensure!(streams.is_none(), "fixture streams already installed");
        *streams = Some(HeldStreams {
            attempt: attempt.clone(),
            deadline: hard_deadline,
            start: Some((first_tx, capture_tx)),
            ready: Some(ready_tx),
        });
        Ok(())
    })
    .await?;
    println!("e3_seed ready_ms={}", started.elapsed().as_millis());

    let opening_progress = Arc::new(EgressOpeningProgress::default());
    let callback_started = opening_progress.start();
    let (resume, opening_forwarded) = base.pause_next_egress_opening(opening_progress.clone());
    let writer_ack = Arc::new(OnceLock::new());
    let native_gate = Arc::new(OnceLock::new());
    let release = FixtureRelease {
        clock,
        original_arm,
        owned: owned.clone(),
        attempt: attempt.clone(),
        authority: runtime.binding().clone(),
        generation: context.cancel_generation().clone(),
        expected_generation: context.expected_generation(),
        opening_forwarded: opening_forwarded.clone(),
        writer_ack: writer_ack.clone(),
        native_gate: native_gate.clone(),
    };
    let profile = runtime.binding().docker_launch_profile.clone();
    let execute = boxed_execution_phase(|| {
        execution.execute_held_egress(
            &runtime,
            &spec,
            Some(&owned),
            |created| async move {
                ensure!(created.handle.id() == CID);
                Ok(PrearmedStartDirective::EmitOnce(
                    PrearmedStartControl::with_launch_profile(
                        Box::new(ExactStart),
                        stop,
                        profile,
                        Some(Box::new(release)),
                    ),
                ))
            },
            None,
        )
    });
    let execute = EGRESS_OPENING_PROGRESS.scope(opening_progress.clone(), execute);
    let expected_frame = format!("AI-GUARD-EGRESS-RELEASE-V1 {}\n", attempt.as_str());
    // The fixture owns the reply gates and observers through cancellation.
    let mut resume = Some(resume);
    let mut writer_gate = None;
    let mut ready_seen = false;
    let mut first_seen = false;
    let mut captured = None;
    let mut phase = "ready";
    let first_failure = OnceLock::new();
    let stop_source = OnceLock::new();
    let mut callback_after_stop = None;
    let mut callback_joined = None;
    let executed = OnceLock::new();
    let mut witnessed = None;
    let collect_execution = async {
        let result: Result<()> = async {
            ensure!(
                Instant::now() < seed_deadline,
                "harness seed deadline before first callback poll"
            );
            let (outcome, joined) = execute.await?;
            callback_joined = Some(joined);
            let ManagedPrearmedOutcome::Executed(execution) = outcome else {
                anyhow::bail!("held egress callback did not execute");
            };
            let transport_code = execution.result().transport_exit_code;
            let terminal_code = execution.terminal().process.exit_code;
            ensure!(
                transport_code == Some(0) && terminal_code == Some(0),
                "expected exit code 0 for transport and terminal inspection; transport={transport_code:?} terminal={terminal_code:?}"
            );
            ensure!(
                joined,
                "both software transports must close under the real owner"
            );
            Ok(())
        }
        .await;
        callback_after_stop = Some(stop_source.get().is_some());
        if result.is_err() {
            let source = if opening_progress
                .writer_failure_before(drain_start)
                .is_some()
            {
                "writer"
            } else {
                "callback"
            };
            let _ = first_failure.set(source);
            opening_progress.mark(&opening_progress.callback_failed);
        }
        let _ = executed.set(result);
    };
    let collect_oracle = async {
        let result: Result<()> = async {
            let ready = opening_progress
                .wait_for(
                    &opening_progress.ready,
                    callback_started + WRITER_WAIT + WAIT,
                )
                .await?;
            let hello = timeout_at(ready + WAIT, &mut ready_rx)
                .await
                .context("harness READY capture deadline elapsed")??;
            ready_seen = true;
            // A fresh Running exchange first loads history, then reads and
            // advances its pending checkpoint before doing one HTTP request.
            phase = "running_decoded";
            let decoded = opening_progress
                .wait_for(
                    &opening_progress.running_decoded,
                    ready + WRITER_WAIT * 3 + HTTP_WAIT + OBSERVATION_SLACK,
                )
                .await?;
            // Each writer commit uses a strict read and an advance. Preserve
            // time for the Writer to return its own typed failure on either.
            let commit_wait = WRITER_WAIT * 2 + OBSERVATION_SLACK;
            phase = "running_ack";
            let running_ack = opening_progress
                .wait_for(&opening_progress.running_ack, decoded + commit_wait)
                .await?;
            phase = "opening_key_ack";
            let key_ack = opening_progress
                .wait_for(&opening_progress.opening_key_ack, running_ack + commit_wait)
                .await?;
            phase = "opening_post";
            let post = opening_progress
                .wait_for(&opening_progress.opening_http_enter, key_ack + WAIT)
                .await?;
            let invoked = *opening_progress
                .opening_attempt
                .get()
                .context("opening invocation unobserved")?;
            ensure!(
                ready <= decoded
                    && decoded <= running_ack
                    && running_ack <= key_ack
                    && key_ack <= invoked
                    && invoked <= post,
                "opening milestones out of order"
            );
            phase = "opening_receipt";
            let receipt_deadline = post + HTTP_WAIT + OBSERVATION_SLACK;
            let held_at = opening_progress
                .wait_for(&opening_progress.opening_receipt_held, receipt_deadline)
                .await?;
            ensure!(
                post <= held_at,
                "opening receipt retention preceded the POST"
            );
            phase = "opening_evidence";
            let opening = opening_events(base, &worker.launch.session_id)?;
            let [opened] = opening.as_slice() else {
                anyhow::bail!("expected one committed opening while its response is retained");
            };
            let EgressObservationV1::ChannelOpened { binding } =
                serde_json::from_value(opened.event.data.clone())?
            else {
                anyhow::bail!("wrong committed observation");
            };
            ensure!(
                binding.attempt_id == attempt.as_str() && binding.worker_id == worker.worker_id,
                "committed opening binds another worker or attempt"
            );
            ensure!(
                hello.binding_id == attempt
                    && hello.claim_id == binding.channel_id
                    && hello.context_digest == binding.context_digest
                    && hello.transport_epoch == binding.epoch,
                "helper READY differs from committed opening"
            );
            ensure!(
                base.store
                    .worker(&worker.worker_id)?
                    .is_some_and(|worker| worker.state == WorkerStateV1::Running),
                "committed opening lacks its Running worker"
            );
            let (job, draft) =
                witness_load(&execution, &opening_progress, "opening_evidence").await?;
            ensure!(
                draft.egress_release.is_none(),
                "release intent preceded the opening ACK"
            );
            let retained = draft.egress_opening.context("retained opening request")?;
            ensure!(
                retained.request().binding == binding
                    && retained.request().observation_id.as_str() == opened.event.id,
                "retained request differs from committed opening"
            );
            require_quiet_stdin(&mut first_rx, &mut first_seen, "SQLite opening ACK")?;
            let expected = EgressChannelOpeningReceiptV1 {
                version: 1,
                observation_id: IdV1::parse(&opened.event.id)?,
                binding,
                disposition: EgressChannelOpeningDispositionV1::Fresh,
                receipt: EgressReceiptV1 {
                    audit_seq: opened.seq,
                    audit_hash: opened.hash.clone(),
                },
            };
            phase = "writer_barrier_install";
            let (barrier, ack) = base
                .writer()
                .egress_release_reply_barrier_for_test(
                    operation,
                    job.head().clone(),
                    expected.clone(),
                )
                .await
                .map_err(|failure| {
                    opening_progress.record_writer_failure(
                        "writer_barrier_install",
                        "install",
                        failure,
                    )
                })?;
            let writer_gate = writer_gate.insert(barrier);
            ensure!(writer_ack.set(ack).is_ok(), "duplicate release ACK witness");
            phase = "opening_forward";
            ensure!(
                Instant::now() < drain_start,
                "harness deadline before forwarding opening ACK"
            );
            resume
                .take()
                .context("missing opening response sender")?
                .send(())
                .map_err(|_| anyhow::anyhow!("opening response was abandoned"))?;
            phase = "writer_barrier_enter";
            opening_progress
                .observe_release(Instant::now() + commit_wait, async {
                    writer_gate
                        .try_entered_for_test()
                        .await
                        .map_err(anyhow::Error::msg)
                })
                .await
                .context("release intent did not reach the Writer reply barrier")?;
            require_quiet_stdin(&mut first_rx, &mut first_seen, "Writer release intent ACK")?;
            phase = "writer_barrier_release";
            ensure!(
                Instant::now() < drain_start,
                "harness deadline before releasing Writer ACK"
            );
            let writer_released = Instant::now();
            writer_gate
                .release_egress_for_test()
                .map_err(anyhow::Error::msg)?;
            phase = "release_intent_ack";
            let first_byte_deadline = writer_released + WRITER_WAIT + WAIT;
            opening_progress
                .wait_for(&opening_progress.release_intent_ack, first_byte_deadline)
                .await?;
            phase = "first_release_byte";
            opening_progress
                .observe_release(first_byte_deadline, async { Ok((&mut first_rx).await?) })
                .await?;
            first_seen = true;
            phase = "submitted_stdin_custody";
            opening_progress
                .wait_for(&opening_progress.release_submitted, first_byte_deadline)
                .await?;
            // The real attach fixture has received data. While its owner is
            // paused after submission, EOF must wait for the explicit close.
            if let Ok(result) = timeout(STDIN_CUSTODY_WINDOW, &mut capture_rx).await {
                captured = Some(result.context("stdin witness closed before explicit close")?);
                anyhow::bail!("the submitted writer was closed before the egress handoff");
            }
            opening_progress.close_release_stdin.notify_one();
            phase = "release_eof";
            captured = Some(
                opening_progress
                    .observe_release(Instant::now() + WAIT, async {
                        Ok((&mut capture_rx).await?)
                    })
                    .await?,
            );
            ensure!(
                captured.as_deref() == Some(expected_frame.as_bytes()),
                "held bootstrap did not receive the exact release frame through EOF"
            );
            ensure!(
                matches!(native_gate.get(), Some(Ok(()))),
                "the native write point did not attest both ACKs"
            );
            phase = "verify_release_intent";
            ensure!(
                opening_events(base, &worker.launch.session_id)?.len() == 1,
                "expected exactly one committed opening after release"
            );
            let (_, draft) =
                witness_load(&execution, &opening_progress, "verify_release_intent").await?;
            let intent = draft.egress_release.context("release intent")?;
            ensure!(
                intent.opening_for_test() == &expected,
                "release intent differs from the acknowledged opening"
            );
            phase = "oracle_complete";
            Ok(())
        }
        .await;
        if let Err(error) = &result {
            let source = if error.is::<EgressWitnessWriterFailure>() {
                "writer"
            } else {
                "oracle"
            };
            let _ = first_failure.set(source);
            let _ = stop_source.set("oracle");
            // Publish STOP before dropping either still-retained reply owner.
            opening_progress.mark(&opening_progress.stop);
            stop_owner.send_replace(true);
            drop(resume.take());
            drop(writer_gate.take());
        }
        witnessed = Some(result);
    };
    {
        let mut callback = std::pin::pin!(collect_execution);
        let timed_out = {
            let mut collection = std::pin::pin!(async {
                tokio::join!(callback.as_mut(), collect_oracle);
            });
            let timed_out = tokio::select! {
                biased;
                () = sleep_until(drain_start) => true,
                () = collection.as_mut() => Instant::now() >= drain_start,
            };
            if timed_out {
                let _ = first_failure.set("deadline");
                if stop_source.set("deadline").is_ok() {
                    opening_progress.mark(&opening_progress.stop);
                    stop_owner.send_replace(true);
                }
            }
            timed_out
        };
        drop(resume);
        drop(writer_gate);
        // STOP and oracle cancellation precede this callback-only drain. A late
        // reply can enrich the failure, within the original hard deadline.
        if timed_out && executed.get().is_none() && Instant::now() < hard_deadline {
            tokio::select! {
                biased;
                () = sleep_until(hard_deadline) => {},
                () = callback.as_mut() => {},
            }
        }
    }

    if let Some(first_failure) = first_failure.get() {
        let opening_seen = opening_progress.opening_receipt_held.get().is_some();
        let ready_seen = ready_seen || ready_rx.try_recv().is_ok();
        let first_seen = first_seen || first_rx.try_recv().is_ok();
        let captured = captured.or_else(|| capture_rx.try_recv().ok());
        let release = match captured.as_deref() {
            None => "unobserved",
            Some([]) => "empty_eof",
            Some(bytes) if bytes == expected_frame.as_bytes() => "exact_frame",
            Some(_) => "different_frame",
        };
        let writer = writer_ack
            .get()
            .map(|ack| ack.require_explicit_release_and_ack());
        let stop = stop_source.get().copied().unwrap_or("none");
        let callback_joins = match callback_joined {
            Some(true) => "callback joins confirmed",
            Some(false) => "callback joins unconfirmed (callback reported false)",
            None if executed.get().is_none() => {
                "callback joins unconfirmed (collection cancelled at hard deadline)"
            }
            None => "callback joins unconfirmed (callback returned no join witness)",
        };
        anyhow::bail!(
            "E3 fixture oracle_phase={phase} first_observed_failure={first_failure} stop={stop} callback_after_stop={callback_after_stop:?} opening_commit_seen={opening_seen} opening_forwarded={} helper_ready_seen={ready_seen} writer_ack={writer:?} native_gate={:?} first_byte_seen={first_seen} release={release}\ncallback={}\n{callback_joins}\noracle={}\ne3_opening_progress {}",
            opening_forwarded.load(Ordering::Acquire),
            native_gate.get(),
            bounded_fixture_outcome(executed.get()),
            bounded_fixture_outcome(witnessed.as_ref()),
            opening_progress.report(),
        );
    }
    println!("e3_opening_progress {}", opening_progress.report());
    Ok(())
}

async fn witness_load(
    execution: &Execution<'_>,
    progress: &EgressOpeningProgress,
    phase: &'static str,
) -> Result<(AcceptedJobV1, Draft)> {
    let writer = &execution.executor.writer;
    let scope = execution.context.scope();
    ensure!(same(writer.scope(), scope)?, "writer scope changed");
    let job = writer
        .load_diagnosed(execution.operation)
        .await
        .map_err(|failure| progress.record_writer_failure(phase, "load", failure))?
        .context("job disappeared")?;
    ensure!(
        job.operation_id() == execution.operation && same(job.scope(), scope)?,
        "job scope changed"
    );
    let draft = Draft::from_job(&job)?;
    Ok((job, draft))
}

fn require_original_arm_window(
    clock: &dyn HostClock,
    arm: &ArmV1,
    remaining: Duration,
) -> Result<()> {
    let now = clock.sample()?;
    let armed_at = arm.clock.continuous_ns()?;
    let now_ns = now.continuous_ns()?;
    let deadline = arm.deadline_ns_decimal.parse::<u128>()?;
    ensure!(
        matches!(arm.subject, ArmSubjectV1::Worker)
            && now.method == arm.clock.method
            && now.boot_id == arm.clock.boot_id
            && now_ns >= armed_at
            && deadline == checked_deadline(armed_at, arm.timeout_seconds)?
            && deadline.saturating_sub(now_ns) > remaining.as_nanos(),
        "original worker Arm does not cover the remaining callback observation window"
    );
    Ok(())
}

fn require_quiet_stdin(
    first_rx: &mut oneshot::Receiver<()>,
    first_seen: &mut bool,
    retained: &'static str,
) -> Result<()> {
    match first_rx.try_recv() {
        Err(oneshot::error::TryRecvError::Empty) => Ok(()),
        Err(oneshot::error::TryRecvError::Closed) => {
            anyhow::bail!("stdin witness closed while {retained} was retained")
        }
        Ok(()) => {
            *first_seen = true;
            anyhow::bail!("stdin was touched while {retained} was retained")
        }
    }
}

fn bounded_fixture_outcome(result: Option<&Result<()>>) -> String {
    match result {
        Some(Ok(())) => "ok".into(),
        Some(Err(error)) => format!("{error:#}")
            .chars()
            .take(512)
            .flat_map(char::escape_debug)
            .collect(),
        None => "cause inconnue (résultat indisponible à l’échéance)".into(),
    }
}

fn opening_events(base: &Fixture, session: &str) -> Result<Vec<crate::model::StoredEvent>> {
    Ok(base
        .store
        .egress_session_events(session, 10)?
        .into_iter()
        .filter(|stored| stored.event.kind == "egress.channel.opened")
        .collect())
}

fn prepare(
    base: &Fixture,
    operation: Uuid,
    runtime: &Runtime,
) -> Result<crate::runner::broker::PreparedSubmission> {
    let root = base.config.path().parent().context("fixture root")?;
    let source = root.join("e3-source");
    fs::create_dir(&source)?;
    let key = base.config.path().join("synthetic.env");
    let providers = base.config.path().join("providers.json");
    base.config.persistent_create(
        "synthetic.env",
        b"OPENAI_API_KEY=synthetic-egress-fixture-value\n",
    )?;
    base.config.persistent_create("providers.json", &serde_json::to_vec(&json!({
        "version":1,"profiles":[{"id":"codex-api","revision":1,"client":"codex",
          "auth_mode":"api","model":"fixture-model","credential":{"kind":"api_env_file","path":key}}]
    }))?)?;
    let plan: Value = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/broker_intent-plan-v3.json"
    ))?;
    let mut options: RunOptions = serde_json::from_value(plan["provider_input"].clone())?;
    options.project = source;
    options.config_dir = base.config.path().to_owned();
    options.gateway = base.scope.gateway_origin().to_owned();
    options.network = NetworkMode::None;
    options.runtime_endpoint = Some(ENDPOINT.into());
    options.providers_file = Some(providers);
    let mut prepared = prepare_broker_submission_with_egress(
        options,
        operation,
        &[],
        Some(EgressLaunchV1::broker_proxy(PORT)?),
    )?;
    prepared.select_minimal_docker_start_profile(runtime)?;
    Ok(prepared)
}

async fn seed_start_issued(
    execution: &Execution<'_>,
    accepted: &AcceptedJobV1,
    runtime: &ManagedRuntime,
    base: &Fixture,
    facts: &Mutex<Value>,
) -> Result<(WorkerV1, OwnedContainerSnapshot, Box<ArmV1>)> {
    let intent = accepted.intent();
    let request = intent.request_data().clone();
    let (decision, _) = execution
        .rpc(Operation::Decision {
            request: request.clone(),
            response: None,
        })
        .await?;
    let Operation::Decision {
        response: Some(answer),
        ..
    } = decision
    else {
        anyhow::bail!("fixture has no decision ACK");
    };
    let resolution = request
        .provider_resolution
        .clone()
        .context("fixture provider resolution")?;
    let options = intent.initial_options().run_options();
    let manifest = BTreeMap::<String, crate::workspace::Fingerprint>::new();
    let admission = AdmissionV1 {
        version: 1,
        operation_id: execution.operation.to_string(),
        decision_request: request.clone(),
        receipt: answer.receipt().clone(),
        launch: LaunchSpecV1 {
            version: 1,
            session_id: intent.session().to_string(),
            command: request.command,
            profile: options.profile,
            runtime_kind: RuntimeKindV1::Docker,
            authority_id: runtime.binding().authority_id.to_string(),
            image_ref: options.image,
            image_id: format!("sha256:{}", "a".repeat(64)),
            network: NetworkV1::None,
            cpu_millis: 2000,
            memory_bytes: 1_073_741_824,
            pids_limit: 256,
            timeout_seconds: options.timeout.try_into()?,
            security_preset: SecurityPresetV1::FixedSandboxV2,
            restart_policy: RestartPolicyV1::No,
            workspace_manifest_digest: content_hash("ai-guard:workspace-manifest:v1", &manifest)?,
            workspace_import_policy_version: 1,
            provider: options.provider,
            capture_output: options.capture_output,
            credential_binding_id: Some(resolution.credential_binding_id.to_string()),
            credential_names: resolution.credential_names.clone(),
            provider_resolution: Some(resolution),
            egress: intent.egress().cloned(),
        },
    };
    let receipt_digest = accepted.binding_manifest().handoff_sha();
    let prepared: managed::ManagedCheckpoint = serde_json::from_value(json!({
        "tenant_id": TENANT, "gateway_origin": base.scope.gateway_origin(),
        "config_root": base.config.path(), "binding": runtime.binding(),
        "authority_file": "runtime-authority.json", "admission": admission,
        "worker": null, "owned": null, "pending": null, "uncertain": [],
        "lock_identity": null, "before": manifest, "import": null, "decision": answer.decision(),
        "output_event": null, "output_acknowledged": false, "result": null,
        "archive_name": "worker.tar", "archive_digest": "e".repeat(64),
        "credential_digest": receipt_digest, "workspace_control_exclusion_version": 1,
        "invocation_event": null
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
    execution
        .patch(|draft| {
            draft.run.plan.managed = Some(prepared.clone());
            draft.operations.push(pending.clone());
            Ok(())
        })
        .await?;
    execution.rpc(pending).await?;
    let worker = execution.mutate(WorkerTransitionV1::Prepare).await?;
    // The recovery fixture already advances the two local software-only
    // helper histories through their real Store and Writer ports.
    let admission_receipt = execution
        .load()
        .await?
        .1
        .operations
        .iter()
        .find_map(|operation| match operation {
            Operation::Admission {
                response: Some(response),
                ..
            } => Some(response.receipt.clone()),
            _ => None,
        })
        .context("fixture admission receipt")?;
    let (import, import_arm, import_terminal) = boxed_execution_phase(|| {
        seed_helper(
            execution,
            &worker,
            runtime.binding(),
            accepted.intent_hash(),
            HelperPurposeV1::Import,
            HelperPermissionV1::Admission {
                receipt: admission_receipt,
            },
            None,
        )
    })
    .await?;
    boxed_execution_phase(|| {
        seed_helper(
            execution,
            &worker,
            runtime.binding(),
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
        )
    })
    .await?;
    let worker = execution.mutate(WorkerTransitionV1::CreateIssued).await?;
    let inspection = inspection(
        &worker,
        CID,
        worker.attempt.stable_name.clone(),
        worker.spec_hash.clone(),
    );
    let worker = execution
        .mutate(WorkerTransitionV1::Created {
            inspection: inspection.clone(),
        })
        .await?;
    *facts.lock().await = held_facts(&worker);
    let handle: ContainerHandle = serde_json::from_value(json!({
        "authority_fingerprint": runtime.binding().fingerprint(),
        "session": worker.launch.session_id, "operation": worker.admission_operation_id,
        "id": CID, "name": worker.attempt.stable_name, "configuration_fingerprint_version": 2
    }))?;
    let observation = runtime.observe(&handle).await?;
    let owned = OwnedContainerSnapshot {
        handle,
        observation,
    };
    execution
        .patch(|draft| {
            draft
                .run
                .plan
                .managed
                .as_mut()
                .context("fixture managed checkpoint")?
                .owned = Some(owned.clone());
            Ok(())
        })
        .await?;
    let arm = arm(
        &worker,
        runtime.binding(),
        accepted.intent_hash(),
        inspection,
        ArmSubjectV1::Worker,
        owned
            .observation
            .configuration_fingerprint
            .clone()
            .context("fixture confinement fingerprint")?,
    );
    let (armed, _) = execution
        .rpc(Operation::Arm {
            parent: worker.clone(),
            owned: owned.clone(),
            request: arm.clone(),
            response: None,
        })
        .await?;
    let Operation::Arm {
        response: Some(armed),
        ..
    } = armed
    else {
        anyhow::bail!("fixture has no Arm ACK");
    };
    armed.receipt.validate_for_arm(&arm)?;
    execution
        .rpc(Operation::WorkerClaim {
            request: ArmedStartClaimV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                worker_id: worker.worker_id,
                attempt_id: worker.attempt.attempt_id,
                expected_revision: worker.revision,
                emitter_id: Uuid::new_v4().to_string(),
                arm_receipt: armed.receipt,
                clock_check: arm.clock.clone(),
            },
            response: None,
        })
        .await?;
    let worker = execution.load().await?.1.worker()?.clone();
    ensure!(
        worker.state == WorkerStateV1::StartIssued,
        "fixture worker is not StartIssued"
    );
    Ok((worker, owned, Box::new(arm)))
}

fn held_facts(worker: &WorkerV1) -> Value {
    let mut facts = inspect_facts(worker);
    let mut env = vec!["HOME=/home/agent".into(), "LANG=C.UTF-8".into()];
    env.extend(
        proxy_env::proxy_environment(PORT)
            .into_iter()
            .map(|(key, value)| format!("{key}={value}")),
    );
    env.push(format!(
        "AI_GUARD_EGRESS_HELD_V1={}",
        worker.attempt.attempt_id
    ));
    facts["State"]["Status"] = json!("created");
    facts["State"]["Running"] = json!(false);
    facts["State"]["StartedAt"] = json!("0001-01-01T00:00:00Z");
    facts["Config"]["Labels"] = json!({
        "agentsandbox.session": worker.launch.session_id,
        "agentsandbox.operation": worker.admission_operation_id,
        "agentsandbox.purpose": "run",
    });
    facts["Config"]["Entrypoint"] = json!([worker.launch.command.argv[0]]);
    facts["Config"]["Cmd"] = json!(&worker.launch.command.argv[1..]);
    facts["Config"]["Env"] = json!(env);
    facts["Config"]["OpenStdin"] = json!(true);
    facts["Config"]["AttachStdin"] = json!(true);
    facts["Config"]["StdinOnce"] = json!(true);
    facts["HostConfig"]["Tmpfs"] = json!({
        "/tmp":"rw,nosuid,nodev,noexec,size=256m,mode=1777",
        "/home/agent":"rw,nosuid,nodev,size=64m,uid=1000,gid=1000,mode=700",
    });
    facts
}

struct RefuseLookup;

impl HostLookup for RefuseLookup {
    fn lookup(&self, _name: String, _port: u16) -> LookupFuture {
        Box::pin(async { Err(std::io::ErrorKind::PermissionDenied.into()) })
    }
}

struct ExactStart;

impl BeforeSpawnGuard for ExactStart {
    fn check_before_spawn(self: Box<Self>, target: &str) -> Result<()> {
        ensure!(target == CID, "unexpected start target");
        Ok(())
    }
}

struct FixtureRelease {
    clock: Arc<NativeHostClock>,
    original_arm: Box<ArmV1>,
    owned: OwnedContainerSnapshot,
    attempt: IdV1,
    authority: RuntimeAuthority,
    generation: CancelGeneration,
    expected_generation: u64,
    opening_forwarded: Arc<AtomicBool>,
    writer_ack: Arc<OnceLock<EgressReleaseAckWitness>>,
    native_gate: Arc<OnceLock<std::result::Result<(), &'static str>>>,
}

impl NativeBootstrapRelease for FixtureRelease {
    fn submit(
        self: Box<Self>,
        authority: &RuntimeAuthority,
        observed: &OwnedContainerSnapshot,
        attempt: &IdV1,
        check_release: Box<dyn FnOnce() -> Result<()> + '_>,
        write: Box<dyn FnOnce() -> Result<(), BootstrapStdinSubmitError> + '_>,
    ) -> Result<Box<dyn NativeConnectGuard>, BootstrapStdinSubmitError> {
        let refused = |reason: &'static str| {
            let _ = self.native_gate.set(Err(reason));
            BootstrapStdinSubmitError::Refused
        };
        if authority.fingerprint() != self.authority.fingerprint()
            || attempt != &self.attempt
            || observed.handle.id() != self.owned.handle.id()
            || observed
                .observation
                .validate_identity_allocation(&self.owned.observation)
                .is_err()
        {
            return Err(refused("native release identity differs"));
        }
        self.generation
            .with_current_generation(self.expected_generation, || {
                check_release().map_err(|_| refused("live release check refused"))?;
                if !self.opening_forwarded.load(Ordering::Acquire) {
                    return Err(refused("SQLite opening response was not forwarded"));
                }
                // The actor records the explicit receive and reply before
                // serving the subsequent head-fence command.
                self.writer_ack
                    .get()
                    .ok_or_else(|| refused("missing exact Writer advance witness"))?
                    .require_explicit_release_and_ack()
                    .map_err(&refused)?;
                require_original_arm_window(
                    self.clock.as_ref(),
                    &self.original_arm,
                    Duration::ZERO,
                )
                .map_err(|_| refused("original worker Arm window is unavailable at release"))?;
                self.native_gate
                    .set(Ok(()))
                    .map_err(|_| BootstrapStdinSubmitError::Refused)?;
                write()
            })
            .map_err(|_| refused("native release generation refused"))??;
        Ok(self)
    }
}

impl NativeConnectGuard for FixtureRelease {
    fn clock(&self) -> &dyn HostClock {
        self.clock.as_ref()
    }

    fn with_current(
        &self,
        _poll: &mut dyn FnMut(&HostClockSampleV1),
    ) -> Result<(), NativeConnectRefusal> {
        Err(NativeConnectRefusal::Unavailable)
    }
}

struct HeldStreams {
    attempt: IdV1,
    deadline: Instant,
    start: Option<(oneshot::Sender<()>, oneshot::Sender<Vec<u8>>)>,
    ready: Option<oneshot::Sender<egress_wire::CommonEgressHandshakeV1>>,
}

struct HeldRuntime {
    runtime: Runtime,
    facts: Arc<Mutex<Value>>,
    streams: Arc<Mutex<Option<HeldStreams>>>,
    stop: Option<oneshot::Sender<()>>,
    controller: JoinHandle<Result<()>>,
}

impl Drop for HeldRuntime {
    fn drop(&mut self) {
        self.controller.abort();
    }
}

impl HeldRuntime {
    async fn new(root: &Path, seed_deadline: Instant) -> Result<Self> {
        ensure!(
            Instant::now() < seed_deadline,
            "harness seed deadline before fixture compiler"
        );
        let program = root.join("e3-runtime");
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/runtime_fixtures/relay_stdio.rs");
        let mut compiler = tokio::process::Command::new(
            std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()),
        )
        .args([
            "--edition=2024",
            "--crate-name",
            "held_egress_fixture",
            "-o",
        ])
        .arg(&program)
        .arg(source)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
        let compiler_deadline = (Instant::now() + Duration::from_secs(30)).min(seed_deadline);
        let status = match timeout_at(compiler_deadline, compiler.wait()).await {
            Ok(status) => status?,
            Err(_) => {
                let _ = compiler.start_kill();
                let reaped = matches!(timeout(WAIT, compiler.wait()).await, Ok(Ok(_)));
                let limit = if compiler_deadline == seed_deadline {
                    "seed"
                } else {
                    "compiler"
                };
                anyhow::bail!(
                    "harness {limit} deadline while compiling the software fixture, reaped={reaped}"
                );
            }
        };
        ensure!(
            status.success(),
            "software-only runtime fixture did not compile ({status})"
        );
        let socket = root.join("e3-s");
        let listener = UnixListener::bind(&socket)?;
        fs::write(
            program.with_extension("fixture"),
            socket.as_os_str().as_encoded_bytes(),
        )?;
        let runtime = Runtime::with_program(RuntimeKind::Docker, &program)?;
        let facts = Arc::new(Mutex::new(Value::Null));
        let streams = Arc::new(Mutex::new(None));
        let (stop, stopped) = oneshot::channel();
        ensure!(
            Instant::now() < seed_deadline,
            "harness seed deadline before runtime controller"
        );
        let controller = tokio::spawn(serve_runtime(
            listener,
            facts.clone(),
            streams.clone(),
            stopped,
        ));
        Ok(Self {
            runtime,
            facts,
            streams,
            stop: Some(stop),
            controller,
        })
    }

    async fn close_and_join(&mut self) -> Result<()> {
        let stop = self
            .stop
            .take()
            .context("runtime controller already joined")?;
        // A closed receiver can mean the controller already has an error.
        let _ = stop.send(());
        match timeout(WAIT, &mut self.controller).await {
            Ok(joined) => joined.context("software runtime controller task failed")?,
            Err(error) => {
                self.controller.abort();
                let _ = timeout(WAIT, &mut self.controller).await;
                Err(error).context("software runtime controller did not join its handlers")
            }
        }
    }
}

async fn serve_runtime(
    listener: UnixListener,
    facts: Arc<Mutex<Value>>,
    streams: Arc<Mutex<Option<HeldStreams>>>,
    mut stopped: oneshot::Receiver<()>,
) -> Result<()> {
    let mut children = JoinSet::new();
    let mut outcome = loop {
        tokio::select! {
            biased;
            _ = &mut stopped => break Ok(()),
            accepted = listener.accept(), if children.len() < 8 => {
                let (stream, _) = match accepted {
                    Ok(connection) => connection,
                    Err(error) => break Err(error).context("software runtime accept failed"),
                };
                children.spawn(serve_cli(stream, facts.clone(), streams.clone()));
            }
            Some(result) = children.join_next(), if !children.is_empty() => {
                let result = result.context("software runtime handler task failed").and_then(|handler| handler);
                if result.is_err() {
                    break result;
                }
            }
        }
    };
    drop(listener);
    // The READY and capture notifications can precede the handlers' final
    // shutdown. Join all accepted handlers and preserve their real errors.
    while let Some(result) = children.join_next().await {
        outcome = outcome.and(
            result
                .context("software runtime handler task failed")
                .and_then(|handler| handler),
        );
    }
    outcome
}

async fn serve_cli(
    mut stream: UnixStream,
    facts: Arc<Mutex<Value>>,
    streams: Arc<Mutex<Option<HeldStreams>>>,
) -> Result<()> {
    let args = timeout(WAIT, read_cli_args(&mut stream)).await??;
    let mut args: Vec<_> = args.iter().map(String::as_str).collect();
    while let [flag @ ("--host" | "--config"), value, ..] = args.as_slice() {
        ensure!(
            (*flag == "--host" && *value == ENDPOINT)
                || (*flag == "--config" && Path::new(value).is_absolute()),
            "unexpected runtime prefix"
        );
        args.drain(..2);
    }
    match args.as_slice() {
        ["info", "--format", "{{json .}}"] => {
            stream.write_i32(0).await?;
            stream
                .write_all(
                    br#"{"ID":"held-egress-software-engine","SecurityOptions":["name=rootless"]}"#,
                )
                .await?;
        }
        ["inspect", "--format", "{{json .}}", CID] => {
            let bytes = {
                let facts = facts.lock().await;
                ensure!(!facts.is_null(), "inspect before fixture preparation");
                serde_json::to_vec(&*facts)?
            };
            stream.write_i32(0).await?;
            stream.write_all(&bytes).await?;
        }
        ["start", "--attach", "--interactive", CID] => {
            let (first, capture, deadline) = {
                let mut streams = streams.lock().await;
                let streams = streams.as_mut().context("missing egress fixture streams")?;
                let (first, capture) = streams.start.take().context("duplicate start")?;
                (first, capture, streams.deadline)
            };
            let started = chrono::Utc::now();
            {
                let mut facts = facts.lock().await;
                ensure!(
                    facts["State"]["Status"] == "created",
                    "unexpected start state"
                );
                facts["State"]["Status"] = json!("running");
                facts["State"]["Running"] = json!(true);
                facts["State"]["StartedAt"] = json!(started.to_rfc3339());
            }
            stream.write_i32(0).await?;
            let bytes = witness::read_release(&mut stream, first, deadline).await?;
            {
                let mut facts = facts.lock().await;
                facts["State"]["Status"] = json!("exited");
                facts["State"]["Running"] = json!(false);
                facts["State"]["FinishedAt"] = json!(chrono::Utc::now().to_rfc3339());
            }
            capture
                .send(bytes)
                .map_err(|_| anyhow!("release capture abandoned"))?;
        }
        [
            "exec",
            "--interactive",
            "--tty=false",
            "--privileged=false",
            rest @ ..,
        ] => {
            let (env, tail) = rest.split_at(
                rest.iter()
                    .take_while(|arg| arg.starts_with("--env="))
                    .count(),
            );
            ensure!(
                env.iter().all(|arg| arg[6..]
                    .split_once('=')
                    .is_some_and(|(_, value)| value.is_empty())),
                "fixture helper environment must contain only empty overrides"
            );
            ensure!(
                tail == [
                    "--user",
                    "1000:1000",
                    CID,
                    "/usr/local/bin/ai-guard-relay",
                    "serve-egress-v1",
                    "--port",
                    "3128"
                ],
                "unexpected egress helper command"
            );
            ensure!(
                facts.lock().await["State"]["Status"] == "running",
                "helper before start"
            );
            let (attempt, ready, deadline) = {
                let mut streams = streams.lock().await;
                let streams = streams.as_mut().context("missing egress fixture streams")?;
                (
                    streams.attempt.clone(),
                    streams.ready.take().context("duplicate helper")?,
                    streams.deadline,
                )
            };
            stream.write_i32(0).await?;
            let hello = witness::echo_ready(&mut stream, &attempt).await?;
            ready
                .send(hello)
                .map_err(|_| anyhow!("helper READY capture abandoned"))?;
            let mut remaining = [0];
            ensure!(
                timeout_at(deadline, stream.read(&mut remaining)).await?? == 0,
                "unexpected egress flow on the inert helper"
            );
        }
        _ => anyhow::bail!("unsupported software runtime invocation"),
    }
    stream.shutdown().await?;
    Ok(())
}

async fn read_cli_args(stream: &mut UnixStream) -> Result<Vec<String>> {
    let count = stream.read_u32().await? as usize;
    ensure!(count <= 256, "too many software runtime arguments");
    let mut remaining = 96 * 1024;
    let mut args = Vec::with_capacity(count);
    for _ in 0..count {
        let length = stream.read_u32().await? as usize;
        ensure!(
            length <= remaining,
            "software runtime arguments exceed bound"
        );
        remaining -= length;
        let mut arg = vec![0; length];
        stream.read_exact(&mut arg).await?;
        args.push(String::from_utf8(arg)?);
    }
    Ok(args)
}
