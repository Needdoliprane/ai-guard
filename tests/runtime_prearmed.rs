//! RED API contract for the prearmed retained runtime port.

#![cfg(unix)]

use agentsandbox::runtime::{
    BeforeSpawnGuard, ManagedPrearmedExecution, ManagedPrearmedOutcome, PrearmedExecutionResult,
    PrearmedStartControl, PrearmedStartDirective,
};
use std::os::unix::fs::PermissionsExt;
use tokio::sync::watch;
use uuid::Uuid;

struct Permit;

impl BeforeSpawnGuard for Permit {
    fn check_before_spawn(self: Box<Self>, _target: &str) -> anyhow::Result<()> {
        Ok(())
    }
}

fn command_count(root: &std::path::Path, command: &str) -> usize {
    std::fs::read_to_string(root.join("log"))
        .unwrap_or_default()
        .lines()
        .filter(|line| line.starts_with(&format!("{command} ")))
        .count()
}

#[test]
fn prearmed_types_are_available_without_legacy_duration_or_timeout_fields() {
    let (_stop_tx, stop_rx) = watch::channel(false);
    let control = PrearmedStartControl::new(Box::new(Permit), stop_rx);
    let _directive = PrearmedStartDirective::EmitOnce(control);
    let _: Option<ManagedPrearmedExecution> = None;
    let _: Option<PrearmedExecutionResult> = None;
}

#[test]
fn prearmed_guard_is_consumed_and_control_debug_is_opaque() {
    let private = "prearmed-guard-canary-not-a-secret";
    let guard = Box::new(Guard {
        private,
        expected_target: Some("immutable-target"),
        checks: None,
        fail: false,
        root: None,
    });
    guard.check_before_spawn("immutable-target").unwrap();

    let (_stop_tx, stop_rx) = watch::channel(false);
    let control = PrearmedStartControl::new(
        Box::new(Guard {
            private,
            expected_target: Some("immutable-target"),
            checks: None,
            fail: false,
            root: None,
        }),
        stop_rx,
    );
    let debug = format!("{control:?}");
    assert!(!debug.contains(private));
    let directive = PrearmedStartDirective::EmitOnce(control);
    assert!(matches!(directive, PrearmedStartDirective::EmitOnce(_)));
    assert!(matches!(
        PrearmedStartDirective::ObserveOnly,
        PrearmedStartDirective::ObserveOnly
    ));
}

#[derive(Debug)]
struct Guard {
    private: &'static str,
    expected_target: Option<&'static str>,
    checks: Option<std::sync::Arc<std::sync::atomic::AtomicUsize>>,
    fail: bool,
    root: Option<std::path::PathBuf>,
}

#[derive(Debug)]
struct GuardRefusal;
impl std::fmt::Display for GuardRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("synthetic pre-start guard refusal")
    }
}
impl std::error::Error for GuardRefusal {}

impl BeforeSpawnGuard for Guard {
    fn check_before_spawn(self: Box<Self>, target: &str) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.expected_target
                .map_or(!target.is_empty(), |expected| target == expected),
            "guard target is not the expected immutable identity"
        );
        // Keep the private marker live without ever including it in diagnostics.
        anyhow::ensure!(!self.private.is_empty());
        if let Some(checks) = self.checks {
            checks.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        if let Some(root) = &self.root {
            let log = std::fs::read_to_string(root.join("log"))?;
            anyhow::ensure!(
                log.lines()
                    .last()
                    .is_some_and(|line| line.starts_with("info ")),
                "authority verification was not the last runtime operation before the guard"
            );
        }
        if self.fail {
            return Err(anyhow::Error::new(GuardRefusal));
        }
        Ok(())
    }
}

#[tokio::test]
async fn prearmed_stop_interrupts_pending_started_callback_without_global_ctrl_c() {
    const CHILD: &str = "AI_GUARD_PREARMED_CHILD";
    if std::env::var_os(CHILD).is_some() {
        use agentsandbox::runtime::{
            ContainerPurpose, ContainerSpec, ManagedRuntime, NetworkMode, Runtime, RuntimeKind,
        };
        use std::{path::PathBuf, time::Duration};
        use tokio::sync::oneshot;

        let root = PathBuf::from(std::env::var_os("AI_GUARD_PREARMED_ROOT").unwrap());
        let runtime = Runtime::with_program(
            RuntimeKind::Docker,
            &PathBuf::from(std::env::var_os("AI_GUARD_RUNTIME_PROGRAM").unwrap()),
        )
        .unwrap();
        let managed = ManagedRuntime::connect_docker_with_runtime(
            Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap(),
            "unix:///fixture/docker.sock",
            runtime,
        )
        .await
        .unwrap();
        assert!(managed.doctor(false).await.is_err());
        assert!(!managed.doctor(true).await.unwrap().rootless);

        let interactive_spec = ContainerSpec::with_operation(
            Uuid::from_u128(0x11111111111141118111111111111111),
            ContainerPurpose::Run,
            "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            vec!["/usr/bin/id".into()],
            NetworkMode::None,
            Uuid::from_u128(0x72000000000040008000000000000000),
        )
        .unwrap()
        .with_security_preset(agentsandbox::control::SecurityPresetV1::FixedSandboxInteractiveV3)
        .with_managed_names(
            "aig-prearmed-interactive-worker",
            "aig-prearmed-interactive-work",
        )
        .unwrap();
        let creates_before = std::fs::read_to_string(root.join("log"))
            .unwrap_or_default()
            .lines()
            .filter(|line| line.starts_with("create "))
            .count();
        let callback_called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let callback_flag = callback_called.clone();
        assert!(
            managed
                .execute_prearmed_retained(
                    &interactive_spec,
                    None,
                    1024,
                    move |_| {
                        callback_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                        async { Ok(PrearmedStartDirective::ObserveOnly) }
                    },
                    |_| async { anyhow::bail!("interactive session cannot start in batch mode") },
                )
                .await
                .is_err()
        );
        assert!(!callback_called.load(std::sync::atomic::Ordering::SeqCst));
        let creates_after = std::fs::read_to_string(root.join("log"))
            .unwrap_or_default()
            .lines()
            .filter(|line| line.starts_with("create "))
            .count();
        assert_eq!(
            creates_after, creates_before,
            "interactive refusal precedes runtime create"
        );

        let observe_spec = ContainerSpec::with_operation(
            Uuid::from_u128(0x11111111111141118111111111111111),
            ContainerPurpose::Run,
            "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            vec!["/usr/bin/id".into()],
            NetworkMode::None,
            Uuid::from_u128(0x71000000000040008000000000000000),
        )
        .unwrap()
        .with_managed_names("aig-prearmed-observe-worker", "aig-prearmed-observe-work")
        .unwrap();
        let observe_starts_before = command_count(&root, "start");
        let observe_only = managed
            .execute_prearmed_retained(
                &observe_spec,
                None,
                1024,
                |_| async { Ok(PrearmedStartDirective::ObserveOnly) },
                |_| async { anyhow::bail!("ObserveOnly cannot invoke the started callback") },
            )
            .await;
        assert!(observe_only.is_err());
        assert_eq!(command_count(&root, "start"), observe_starts_before);
        assert_eq!(
            std::fs::read_to_string(root.join("state")).unwrap(),
            "created"
        );
        let owned = managed
            .lookup_created(&observe_spec)
            .await
            .unwrap()
            .unwrap();
        let successful_case = root.join("engine.fixture/AI_GUARD_MANAGED_CHILD");
        std::fs::write(&successful_case, b"success").unwrap();
        let (recovery_stop_tx, recovery_stop_rx) = watch::channel(false);
        let recovery_guard_root = root.clone();
        let recovered = managed
            .start_owned_prearmed_retained(
                &observe_spec,
                &owned,
                None,
                1024,
                move |_| {
                    let control = PrearmedStartControl::new(
                        Box::new(Guard {
                            private: "fixture-only-guard-marker",
                            expected_target: Some(
                                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                            ),
                            checks: None,
                            fail: false,
                            root: Some(recovery_guard_root.clone()),
                        }),
                        recovery_stop_rx,
                    );
                    async move { Ok(PrearmedStartDirective::EmitOnce(control)) }
                },
                |_| async { Ok(()) },
            )
            .await
            .expect("recovered owned Created instance starts only with a new permit");
        let recovered = match recovered {
            ManagedPrearmedOutcome::Executed(execution) => execution,
            ManagedPrearmedOutcome::Abandoned { .. } => {
                panic!("fresh recovered start must execute rather than abandon before claim")
            }
        };
        drop(recovery_stop_tx);
        assert_eq!(recovered.handle().id(), owned.handle.id());
        assert_eq!(recovered.terminal().process.exit_code, Some(0));

        for (index, case) in ["stop_true", "stop_closed", "guard_error"]
            .into_iter()
            .enumerate()
        {
            let (stop_tx, stop_rx) = watch::channel(case == "stop_true");
            if case == "stop_closed" {
                drop(stop_tx);
            }
            let checks = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let spec = ContainerSpec::with_operation(
                Uuid::from_u128(0x11111111111141118111111111111111),
                ContainerPurpose::Run,
                "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                vec!["/usr/bin/id".into()],
                NetworkMode::None,
                Uuid::from_u128(0x70000000000040008000000000000000 + index as u128),
            )
            .unwrap()
            .with_managed_names(
                &format!("aig-prearmed-{case}-worker"),
                &format!("aig-prearmed-{case}-work"),
            )
            .unwrap();
            let case_name = case.to_owned();
            let starts_before = command_count(&root, "start");
            let guard_checks = checks.clone();
            let guard_root = root.clone();
            let result = managed
                .execute_prearmed_retained(
                    &spec,
                    None,
                    1024,
                    move |_created| {
                        let control = PrearmedStartControl::new(
                            Box::new(Guard {
                                private: "fixture-only-guard-marker",
                                expected_target: Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
                                checks: Some(guard_checks),
                                fail: case_name == "guard_error",
                                root: Some(guard_root.clone()),
                            }),
                            stop_rx,
                        );
                        async move {
                            Ok(if case_name == "observe_only" {
                                PrearmedStartDirective::ObserveOnly
                            } else {
                                PrearmedStartDirective::EmitOnce(control)
                            })
                        }
                    },
                    |_| async { anyhow::bail!("started callback must not run") },
                )
                .await;
            assert!(result.is_err(), "{case} must refuse a prearmed start");
            if case == "guard_error" {
                let error = result.unwrap_err();
                assert!(
                    error.downcast_ref::<GuardRefusal>().is_some(),
                    "runtime stage and retained-instance contexts must preserve the typed guard refusal"
                );
                assert!(!format!("{error:#} {error:?}").contains("fixture-only-guard-marker"));
            }
            let expected_checks = usize::from(case == "guard_error");
            assert_eq!(
                checks.load(std::sync::atomic::Ordering::SeqCst),
                expected_checks
            );
            assert_eq!(
                std::fs::read_to_string(root.join("state")).unwrap(),
                "created"
            );
            let log = std::fs::read_to_string(root.join("log")).unwrap();
            assert_eq!(command_count(&root, "start"), starts_before);
            assert!(!log.lines().any(|line| line.starts_with("kill ")));
            assert!(!log.lines().any(|line| line.starts_with("rm ")));
        }

        let spec = ContainerSpec::with_operation(
            Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap(),
            ContainerPurpose::Run,
            "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            vec!["/usr/bin/id".into()],
            NetworkMode::None,
            Uuid::parse_str("33333333-3333-4333-8333-333333333333").unwrap(),
        )
        .unwrap()
        .with_managed_names("aig-prearmed-fixture-worker", "aig-prearmed-fixture-work")
        .unwrap();

        std::fs::write(&successful_case, b"timeout").unwrap();
        let (service_signal_tx, mut service_signal_rx) = watch::channel(false);
        let mut interrupts =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()).unwrap();
        tokio::spawn(async move {
            if interrupts.recv().await.is_some() {
                let _ = service_signal_tx.send(true);
            }
        });

        let (stop_tx, stop_rx) = watch::channel(false);
        let (started_tx, started_rx) = oneshot::channel();
        let mut started_tx = Some(started_tx);
        let guard_root = root.clone();
        let task = tokio::spawn({
            let managed = managed.clone();
            async move {
                managed
                    .execute_prearmed_retained(
                        &spec,
                        None,
                        1024,
                        move |_created| {
                            let control = PrearmedStartControl::new(
                                Box::new(Guard {
                                    private: "fixture-only-guard-marker",
                                    expected_target: Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
                                    checks: None,
                                    fail: false,
                                    root: Some(guard_root.clone()),
                                }),
                                stop_rx,
                            );
                            async move {
                                Ok(PrearmedStartDirective::EmitOnce(control))
                            }
                        },
                        move |_observation| {
                            if let Some(sender) = started_tx.take() {
                                let _ = sender.send(());
                            }
                            async { std::future::pending::<anyhow::Result<()>>().await }
                        },
                    )
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(5), started_rx)
            .await
            .expect("runtime observed the started fixture")
            .expect("started callback entered");

        // Signal only this child test process, whose explicit service handler
        // is independent of the session's stop watch receiver.
        let signal_result = unsafe { libc::kill(libc::getpid(), libc::SIGINT) };
        assert_eq!(signal_result, 0);
        tokio::time::timeout(Duration::from_secs(2), service_signal_rx.changed())
            .await
            .expect("service-level SIGINT handler observed the signal")
            .expect("service signal channel remains open");
        assert!(*service_signal_rx.borrow());
        assert!(!*stop_tx.borrow(), "SIGINT did not request session stop");
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            !task.is_finished(),
            "daemon session remains independent of SIGINT"
        );

        stop_tx.send(true).unwrap();
        let result = tokio::time::timeout(Duration::from_secs(12), task)
            .await
            .expect("the stopped transport is drained and bounded")
            .expect("runtime task does not panic");
        assert!(result.is_err(), "missing terminal evidence is an error");
        assert_eq!(
            std::fs::read_to_string(root.join("state")).unwrap(),
            "running"
        );
        let log = std::fs::read_to_string(root.join("log")).unwrap();
        assert!(log.lines().any(|line| line.starts_with("start ")));
        assert!(!log.lines().any(|line| line.starts_with("kill ")));
        assert!(!log.lines().any(|line| line.starts_with("rm ")));
        return;
    }

    let root = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let fixture_source = root.path().join("engine.rs");
    let fixture_binary = root.path().join("engine");
    std::fs::write(&fixture_source, include_str!("runtime_fixtures/managed.rs")).unwrap();
    let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let compiled = std::process::Command::new(compiler)
        .args(["--edition=2024", "--crate-name", "prearmed_fixture"])
        .arg(&fixture_source)
        .arg("-o")
        .arg(&fixture_binary)
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let metadata = fixture_binary.with_extension("fixture");
    std::fs::create_dir(&metadata).unwrap();
    std::fs::write(
        metadata.join("AI_GUARD_MANAGED_ROOT"),
        root.path().as_os_str().as_encoded_bytes(),
    )
    .unwrap();
    std::fs::write(metadata.join("AI_GUARD_MANAGED_CHILD"), b"timeout").unwrap();

    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "prearmed_stop_interrupts_pending_started_callback_without_global_ctrl_c",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .env("AI_GUARD_PREARMED_ROOT", root.path())
        .env("AI_GUARD_RUNTIME_PROGRAM", &fixture_binary)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let output = std::thread::spawn(move || child.wait_with_output().unwrap())
        .join()
        .unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn pending_started_callback_is_not_reinvoked_when_attach_reaches_eof() {
    const CHILD: &str = "AI_GUARD_PREARMED_EOF_CHILD";
    if std::env::var_os(CHILD).is_some() {
        use agentsandbox::runtime::{
            ContainerPurpose, ContainerSpec, ManagedRuntime, NetworkMode, Runtime, RuntimeKind,
        };
        use std::{
            path::PathBuf,
            sync::{
                Arc,
                atomic::{AtomicUsize, Ordering},
            },
            time::Duration,
        };
        use tokio::sync::{Notify, oneshot};

        let root = PathBuf::from(std::env::var_os("AI_GUARD_PREARMED_EOF_ROOT").unwrap());
        let runtime = Runtime::with_program(
            RuntimeKind::Docker,
            &PathBuf::from(std::env::var_os("AI_GUARD_RUNTIME_PROGRAM").unwrap()),
        )
        .unwrap();
        let managed = ManagedRuntime::connect_docker_with_runtime(
            Uuid::parse_str("44444444-4444-4444-8444-444444444444").unwrap(),
            "unix:///fixture/docker.sock",
            runtime,
        )
        .await
        .unwrap();
        assert!(!managed.doctor(true).await.unwrap().rootless);
        let spec = ContainerSpec::with_operation(
            Uuid::parse_str("55555555-5555-4555-8555-555555555555").unwrap(),
            ContainerPurpose::Run,
            "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            vec!["/usr/bin/id".into()],
            NetworkMode::None,
            Uuid::parse_str("66666666-6666-4666-8666-666666666666").unwrap(),
        )
        .unwrap()
        .with_managed_names("aig-prearmed-eof-worker", "aig-prearmed-eof-work")
        .unwrap();
        let (_stop_tx, stop_rx) = watch::channel(false);
        let (entered_tx, entered_rx) = oneshot::channel();
        let mut entered_tx = Some(entered_tx);
        let (first_finished_tx, mut first_finished_rx) = oneshot::channel();
        let mut first_finished_tx = Some(first_finished_tx);
        let calls = Arc::new(AtomicUsize::new(0));
        let notify = Arc::new(Notify::new());
        let guard_root = root.clone();
        let mut execution = tokio::spawn({
            let managed = managed.clone();
            let calls = calls.clone();
            let notify = notify.clone();
            async move {
                managed
                    .execute_prearmed_retained(
                        &spec,
                        None,
                        1024,
                        move |_created| {
                            let control = PrearmedStartControl::new(
                                Box::new(Guard {
                                    private: "fixture-only-guard-marker",
                                    expected_target: Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
                                    checks: None,
                                    fail: false,
                                    root: Some(guard_root.clone()),
                                }),
                                stop_rx,
                            );
                            async move { Ok(PrearmedStartDirective::EmitOnce(control)) }
                        },
                        move |_observation| {
                            let call = calls.fetch_add(1, Ordering::SeqCst) + 1;
                            let notify = notify.clone();
                            if call == 1 && let Some(sender) = entered_tx.take() {
                                let _ = sender.send(());
                            }
                            let finished = if call == 1 {
                                first_finished_tx.take()
                            } else {
                                None
                            };
                            async move {
                                notify.notified().await;
                                if let Some(sender) = finished {
                                    let _ = sender.send(());
                                }
                                Ok(())
                            }
                        },
                    )
                    .await
            }
        });
        // This is only the fixture preparation budget. Observation has a
        // separate harness budget after the Rust fixture reaches start.
        tokio::time::timeout(Duration::from_secs(30), async {
            while !root.join("start_seen").exists() {
                tokio::select! {
                    outcome = &mut execution => match outcome {
                        Ok(Err(_)) => panic!("execution failed before fixture start"),
                        Ok(Ok(_)) => panic!("execution completed before fixture start"),
                        Err(_) => panic!("execution task failed before fixture start"),
                    },
                    _ = tokio::time::sleep(Duration::from_millis(20)) => {}
                }
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "fixture preparation timed out: info={} create={} start={} inspect={}",
                command_count(&root, "info"),
                command_count(&root, "create"),
                command_count(&root, "start"),
                command_count(&root, "inspect"),
            );
        });
        // Before the user callback, execution_inspect and the managed callback
        // wrapper each perform info (15s cap) + inspect (10s cap). The harness
        // allows 60s for these two phases; this is not a runtime latency bound.
        tokio::time::timeout(Duration::from_secs(60), async {
            tokio::select! {
                entered = entered_rx => entered.expect("callback entry signal delivered"),
                outcome = &mut execution => match outcome {
                    Ok(Err(_)) => panic!("execution failed before callback entry"),
                    Ok(Ok(_)) => panic!("execution completed before its pending callback"),
                    Err(_) => panic!("execution task failed before callback entry"),
                }
            }
        })
        .await
        .unwrap_or_else(|_| {
            let state = match std::fs::read_to_string(root.join("state")).as_deref() {
                Ok("created") => "created",
                Ok("running") => "running",
                Ok("exited") => "exited",
                _ => "unknown",
            };
            panic!(
                "first observation callback timed out: info={} create={} start={} inspect={} state={state}",
                command_count(&root, "info"),
                command_count(&root, "create"),
                command_count(&root, "start"),
                command_count(&root, "inspect"),
            );
        });
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // The first callback is pending on Notify, so its inspection path
        // cannot run again. A terminal inspect after attach_done therefore
        // belongs to the runtime's branch after child.wait() has consumed EOF.
        std::fs::write(root.join("state"), "exited").unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            while !root.join("attach_done").exists() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("fixture attach reaches its exit marker");
        // Unlike the fixture exit marker, this phase runs external authority
        // and inspection commands (15s and 10s caps). This is a harness budget.
        tokio::time::timeout(Duration::from_secs(30), async {
            while !root.join("terminal_after_attach").exists() {
                tokio::select! {
                    outcome = &mut execution => match outcome {
                        Ok(Err(_)) => panic!("execution failed before terminal EOF inspection"),
                        Ok(Ok(_)) => panic!("execution completed while its first callback was pending"),
                        Err(_) => panic!("execution task failed before terminal EOF inspection"),
                    },
                    _ = tokio::time::sleep(Duration::from_millis(20)) => {}
                }
            }
        })
        .await
        .expect("runtime inspected terminal state after attach EOF");
        assert!(
            !execution.is_finished(),
            "runtime must still await the original callback after attach EOF"
        );
        assert!(
            matches!(
                first_finished_rx.try_recv(),
                Err(oneshot::error::TryRecvError::Empty)
            ),
            "the first callback future must remain pending after attach EOF"
        );
        let calls_at_eof = calls.load(Ordering::SeqCst);
        assert_eq!(calls_at_eof, 1, "on_started was reinvoked after attach EOF");
        notify.notify_one();
        // The marker precedes inspect's process exit, so that command (10s
        // cap) may still be pending before the runtime repolls this future.
        tokio::time::timeout(Duration::from_secs(30), first_finished_rx)
            .await
            .expect("the first callback future finishes after its notification")
            .expect("the first callback future is preserved until completion");
        // With input=None, the remaining path can drain two captures (10s
        // each), then observe again (info 15s + inspect 10s). Allow 60s for
        // those existing phases in the harness; no runtime cap is changed.
        let result = tokio::time::timeout(Duration::from_secs(60), execution)
            .await
            .expect("fixture runtime finishes its captures and final observation")
            .expect("runtime task does not panic")
            .expect("terminal attach completes without a synthetic timeout");
        let result = match result {
            ManagedPrearmedOutcome::Executed(execution) => execution,
            ManagedPrearmedOutcome::Abandoned { .. } => {
                panic!("completed terminal attach must retain an executed transport")
            }
        };
        assert_eq!(calls_at_eof, 1, "on_started was reinvoked after attach EOF");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "one started observation was recorded"
        );
        assert_eq!(result.result().stdout.bytes, b"prearmed fixture stdout\n");
        assert_eq!(result.result().stderr.bytes, b"prearmed fixture stderr\n");
        assert_eq!(result.result().transport_exit_code, Some(0));
        assert_eq!(result.terminal().process.exit_code, Some(0));
        assert_eq!(result.created().handle.id(), result.handle().id());
        return;
    }

    let root = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let fixture_source = root.path().join("engine.rs");
    let fixture_binary = root.path().join("engine");
    let mut source = include_str!("runtime_fixtures/managed.rs").to_owned();
    let start = "        \"start\" => {\n            assert_eq!(args.last().unwrap(), ID);";
    assert_eq!(source.matches(start).count(), 1);
    source = source.replace(
        start,
        "        \"start\" => {\n            println!(\"prearmed fixture stdout\");\n            eprintln!(\"prearmed fixture stderr\");\n            assert_eq!(args.last().unwrap(), ID);\n            fs::write(root.join(\"start_seen\"), \"seen\").unwrap();",
    );
    let end = "            }\n        }\n        \"inspect\" => {";
    assert_eq!(source.matches(end).count(), 1);
    source = source.replace(
        end,
        "            }\n            fs::write(root.join(\"attach_done\"), \"done\").unwrap();\n        }\n        \"inspect\" => {",
    );
    let inspect = "            let state = fs::read_to_string(root.join(\"state\")).unwrap();";
    assert_eq!(source.matches(inspect).count(), 1);
    source = source.replace(
        inspect,
        "            let state = fs::read_to_string(root.join(\"state\")).unwrap();\n            if state == \"exited\" && root.join(\"attach_done\").exists() {\n                fs::write(root.join(\"terminal_after_attach\"), \"observed\").unwrap();\n            }",
    );
    std::fs::write(&fixture_source, source).unwrap();
    let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let compiled = std::process::Command::new(compiler)
        .args(["--edition=2024", "--crate-name", "prearmed_eof_fixture"])
        .arg(&fixture_source)
        .arg("-o")
        .arg(&fixture_binary)
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let metadata = fixture_binary.with_extension("fixture");
    std::fs::create_dir(&metadata).unwrap();
    std::fs::write(
        metadata.join("AI_GUARD_MANAGED_ROOT"),
        root.path().as_os_str().as_encoded_bytes(),
    )
    .unwrap();
    std::fs::write(metadata.join("AI_GUARD_MANAGED_CHILD"), b"timeout").unwrap();

    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "pending_started_callback_is_not_reinvoked_when_attach_reaches_eof",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .env("AI_GUARD_PREARMED_EOF_ROOT", root.path())
        .env("AI_GUARD_RUNTIME_PROGRAM", &fixture_binary)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
