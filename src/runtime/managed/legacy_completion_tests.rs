//! Deterministic fault at the real post-transport native observation boundary.
//! Software fixture only; no Store/finalizer, engine or confinement qualification.
use super::*;
use std::{cell::RefCell, fs, os::unix::fs::PermissionsExt, path::PathBuf};

struct Hook {
    target: String,
    action: Box<dyn FnOnce()>,
}
thread_local! {
    static AFTER_TRANSPORT: RefCell<Option<Hook>> = const { RefCell::new(None) };
}
struct HookGuard {
    target: String,
}
impl Drop for HookGuard {
    fn drop(&mut self) {
        AFTER_TRANSPORT.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.as_ref().is_some_and(|hook| hook.target == self.target) {
                slot.take();
            }
        });
    }
}
fn arm(target: &str, action: impl FnOnce() + 'static) -> HookGuard {
    AFTER_TRANSPORT.with(|slot| {
        let mut slot = slot.borrow_mut();
        assert!(
            slot.is_none(),
            "test fault hook cannot replace another owner"
        );
        *slot = Some(Hook {
            target: target.into(),
            action: Box::new(action),
        });
    });
    HookGuard {
        target: target.into(),
    }
}
pub(super) fn after_transport(target: &str) {
    let hook = AFTER_TRANSPORT.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.as_ref().is_some_and(|hook| hook.target == target) {
            slot.take()
        } else {
            None
        }
    });
    // No RefCell borrow is held through caller code. The hook returns unit and
    // cannot supply an observation, result, permission or native proof.
    if let Some(hook) = hook {
        (hook.action)();
    }
}

async fn native_program() -> Result<(tempfile::TempDir, PathBuf)> {
    let temporary = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()?;
    let source = temporary.path().join("d1_completion.rs");
    let program = temporary.path().join("d1_completion");
    let original = include_str!("../../../tests/runtime_fixtures/managed.rs");
    let inspect = "        \"inspect\" => {\n";
    // This is an actual malformed native response at exactly the next inspect,
    // not an injected successful return or fabricated terminal object.
    let inspect_fault = r#"        "inspect" => {
            if root.join("d1_final_observe_armed").exists()
                && fs::OpenOptions::new().write(true).create_new(true)
                    .open(root.join("d1_final_observe_consumed")).is_ok()
            {
                println!("malformed native inspection");
                return;
            }
"#;
    ensure!(
        original.matches(inspect).count() == 1,
        "fixture inspect seam changed"
    );
    let source_text = original.replace(inspect, inspect_fault);
    let start = "        \"start\" => {\n";
    ensure!(
        source_text.matches(start).count() == 1,
        "fixture start seam changed"
    );
    let source_text = source_text.replace(
        start,
        r#"        "start" => {
            println!("d1 genuine transport stdout");
            eprintln!("d1 genuine transport stderr");
"#,
    );
    fs::write(&source, source_text)?;
    let compiled = tokio::time::timeout(
        Duration::from_secs(30),
        Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
            .args(["--edition=2024", "--crate-name", "d1_completion_fixture"])
            .arg(&source)
            .arg("-o")
            .arg(&program)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .context("fixture compiler exceeded its harness budget")??;
    ensure!(
        compiled.status.success(),
        "controlled Rust fixture compilation failed"
    );
    let metadata = program.with_extension("fixture");
    fs::create_dir(&metadata)?;
    fs::write(
        metadata.join("AI_GUARD_MANAGED_ROOT"),
        temporary.path().as_os_str().as_encoded_bytes(),
    )?;
    fs::write(metadata.join("AI_GUARD_MANAGED_CHILD"), b"start_ok")?;
    Ok((temporary, program))
}
fn execution_spec(operation: Uuid) -> Result<ContainerSpec> {
    ContainerSpec::with_operation(
        Uuid::from_u128(0xD100),
        ContainerPurpose::Run,
        "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        vec!["/usr/bin/id".into()],
        NetworkMode::None,
        operation,
    )?
    .with_managed_names("aig-fixture-worker", "aig-fixture-worker-work")
}

#[tokio::test]
async fn late_observe_failure_keeps_real_fallback_completion_and_original_cause() -> Result<()> {
    let (temporary, program) = native_program().await?;
    let root = temporary.path().to_owned();
    let authority = Uuid::new_v4();
    let managed = ManagedRuntime::connect_docker_with_runtime(
        authority,
        "unix:///fixture/docker.sock",
        Runtime::with_program(RuntimeKind::Docker, &program)?,
    )
    .await?;
    let spec = execution_spec(Uuid::new_v4())?;
    let captured = RefCell::new(None);
    let hook_guard = RefCell::new(None);
    let error = match managed
        .execute_retained(
            &spec,
            None,
            Duration::from_secs(120),
            1024,
            |created| {
                let root = root.clone();
                *hook_guard.borrow_mut() = Some(arm(created.handle.id(), move || {
                    fs::write(root.join("d1_final_observe_armed"), b"after real transport")
                        .unwrap();
                }));
                captured.replace(Some(created));
                async { Ok(StartDirective::EmitOnce) }
            },
            |_| async { Ok(()) },
        )
        .await
    {
        Ok(_) => panic!("the exact late native observation must refuse"),
        Err(error) => error,
    };
    assert!(
        root.join("d1_final_observe_consumed").is_file(),
        "the real native inspect fault was not consumed"
    );
    AFTER_TRANSPORT.with(|slot| assert!(slot.borrow().is_none(), "fault hook leaked"));
    let error = error.context("d1 retained caller context");
    let carrier = error
        .downcast_ref::<RetainedLegacyExecutionFailure>()
        .context("late observation lost its native retained carrier")?;
    assert!(carrier.transport.is_none() && carrier.completed.is_some());
    assert!(carrier.fallback_error.is_none() && carrier.terminal_validation_error.is_none());
    let original_cause = &carrier.cause as *const anyhow::Error;
    assert!(
        error
            .chain()
            .any(|cause| cause.to_string() == "runtime returned malformed inspection facts")
    );
    let foreign = ManagedRuntime::connect_docker_with_runtime(
        authority,
        "unix:///fixture/docker.sock",
        Runtime::with_program(RuntimeKind::Docker, &program)?,
    )
    .await?;
    assert_eq!(managed.binding.fingerprint(), foreign.binding.fingerprint());
    let calls_before_extraction = fs::read_to_string(root.join("log"))?;
    let error = match foreign.take_completed_legacy_failure(&spec, error) {
        Ok(_) => panic!("another invocation owner must not consume the completion"),
        Err(error) => error,
    };
    let wrong_spec = execution_spec(Uuid::new_v4())?;
    let error = match managed.take_completed_legacy_failure(&wrong_spec, error) {
        Ok(_) => panic!("another operation must not consume the retained completion"),
        Err(error) => error,
    };
    let (retained, error) = managed.take_completed_legacy_failure(&spec, error)?;
    let carrier = error
        .downcast_ref::<RetainedLegacyExecutionFailure>()
        .unwrap();
    assert!(std::ptr::eq(original_cause, &carrier.cause));
    assert!(
        carrier.completed.is_none(),
        "completion extraction must be affine"
    );
    assert_eq!(
        retained.handle().id(),
        captured.borrow().as_ref().unwrap().handle.id()
    );
    retained.terminal().validate_terminal(
        managed.binding.kind,
        &captured.borrow().as_ref().unwrap().observation,
    )?;
    assert_eq!(
        retained.result().stdout.bytes,
        b"d1 genuine transport stdout\n"
    );
    assert_eq!(
        retained.result().stderr.bytes,
        b"d1 genuine transport stderr\n"
    );
    assert_eq!(retained.result().state.exit_code, 0);
    assert!(!retained.result().timed_out);
    let error = match managed.take_completed_legacy_failure(&spec, error) {
        Ok(_) => panic!("completion cannot be consumed twice"),
        Err(error) => error,
    };
    assert_eq!(error.to_string(), "d1 retained caller context");
    assert!(
        error
            .chain()
            .any(|cause| cause.to_string() == "runtime returned malformed inspection facts")
    );
    let log = fs::read_to_string(root.join("log"))?;
    assert_eq!(
        log, calls_before_extraction,
        "completion recovery must not emit another runtime operation"
    );
    assert_eq!(
        log.lines()
            .filter(|line| line.starts_with("start "))
            .count(),
        1
    );
    assert_eq!(
        log.lines().filter(|line| line.starts_with("kill ")).count(),
        0,
        "the genuinely exited native fallback must not issue a kill"
    );
    assert_eq!(
        log.lines().filter(|line| line.starts_with("rm ")).count(),
        0,
        "technical completion does not authorize cleanup before durable audit"
    );
    drop(error);
    drop(retained);
    drop(hook_guard);
    drop(captured);
    drop(foreign);
    drop(managed);
    // The real invocation/error/transport owners are gone before TempDir.
    drop(temporary);
    Ok(())
}
