//! The CLI never selects its engine program from a project-controlled PATH.
#![cfg(unix)]
use std::{fs, path::Path, time::Duration};

#[tokio::test]
async fn unbound_runtime_refuses_tty_preset_before_engine_create_or_start() {
    use agentsandbox::{
        control::SecurityPresetV1,
        runtime::{ContainerPurpose, ContainerSpec, NetworkMode, Runtime, RuntimeKind},
    };
    let temporary = tempfile::tempdir().unwrap();
    let program = marker_program(temporary.path());
    let runtime = Runtime::with_program(RuntimeKind::Docker, &program).unwrap();
    let spec = ContainerSpec::new(
        uuid::Uuid::new_v4(),
        ContainerPurpose::Run,
        "worker:fixture",
        vec!["/usr/bin/id".into()],
        NetworkMode::None,
    )
    .unwrap()
    .with_security_preset(SecurityPresetV1::FixedSandboxInteractiveV3);
    let error = runtime
        .execute_retained(&spec, None, Duration::from_secs(5), 0, |_| async { Ok(()) })
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("interactive execution requires ManagedRuntime")
    );
    let error = runtime
        .execute(&spec, None, Duration::from_secs(5), 0, |_| async { Ok(()) })
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("interactive execution requires ManagedRuntime")
    );
    assert!(
        !program.with_extension("fixture").exists(),
        "TTY reached an unbound engine"
    );
    assert!(
        !program.with_extension("calls").exists(),
        "TTY emitted a create/start/cleanup command"
    );
}

#[tokio::test]
async fn second_image_build_revalidates_the_same_engine_id() {
    let temporary = tempfile::tempdir().unwrap();
    let program = marker_program(temporary.path());
    fs::write(program.with_extension("case"), "engine-after-build").unwrap();
    let source = temporary.path().join("project");
    copy_public_build_source(&source);
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
        .args(["build", "--runtime-program"])
        .arg(&program)
        .arg("--source")
        .arg(&source)
        .output()
        .await
        .unwrap();
    assert!(!output.status.success());
    let calls = fs::read_to_string(program.with_extension("calls")).unwrap();
    assert_eq!(
        calls
            .lines()
            .filter(|line| line.starts_with("effect "))
            .count(),
        1,
        "second build escaped authority revalidation: {calls}"
    );
}

#[tokio::test]
async fn build_refuses_missing_worker_dockerfile_before_any_engine_call() {
    let temporary = tempfile::tempdir().unwrap();
    let program = marker_program(temporary.path());
    let source = temporary.path().join("project");
    copy_public_build_source(&source);
    fs::remove_file(source.join("images/worker.Dockerfile")).unwrap();

    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
        .args(["build", "--runtime-program"])
        .arg(&program)
        .arg("--source")
        .arg(&source)
        .output()
        .await
        .unwrap();

    assert!(!output.status.success(), "incomplete source was accepted");
    assert!(
        !program.with_extension("fixture").exists(),
        "build spawned the engine before rejecting incomplete source"
    );
    assert!(
        !program.with_extension("calls").exists(),
        "build invoked the engine before rejecting incomplete source"
    );
    let diagnostic = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        diagnostic.contains("images/worker.Dockerfile"),
        "missing-worker refusal omitted its relative-path diagnostic"
    );
}

#[tokio::test]
async fn build_refuses_malformed_public_source_inputs_before_any_engine_call() {
    for case in [
        "missing-lock",
        "missing-ignore",
        "lock-is-directory",
        "images-is-file",
        "missing-provider-entrypoint",
        "linked-required-file",
        "linked-required-parent",
    ] {
        let temporary = tempfile::tempdir().unwrap();
        let program = marker_program(temporary.path());
        let source = temporary.path().join("project");
        copy_public_build_source(&source);
        let repository = Path::new(env!("CARGO_MANIFEST_DIR"));
        match case {
            "missing-lock" => fs::remove_file(source.join("Cargo.lock")).unwrap(),
            "missing-ignore" => fs::remove_file(source.join(".dockerignore")).unwrap(),
            "lock-is-directory" => {
                fs::remove_file(source.join("Cargo.lock")).unwrap();
                fs::create_dir(source.join("Cargo.lock")).unwrap();
            }
            "images-is-file" => {
                fs::remove_dir_all(source.join("images")).unwrap();
                fs::write(source.join("images"), "fixture").unwrap();
            }
            "missing-provider-entrypoint" => {
                fs::remove_file(source.join("src/bin/ai-guard-provider.rs")).unwrap()
            }
            "linked-required-file" => {
                fs::remove_file(source.join("Cargo.lock")).unwrap();
                std::os::unix::fs::symlink(
                    repository.join("Cargo.lock"),
                    source.join("Cargo.lock"),
                )
                .unwrap();
            }
            "linked-required-parent" => {
                fs::remove_dir_all(source.join("front/src")).unwrap();
                std::os::unix::fs::symlink(repository.join("front/src"), source.join("front/src"))
                    .unwrap();
            }
            _ => unreachable!(),
        }

        let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
            .args(["build", "--runtime-program"])
            .arg(&program)
            .arg("--source")
            .arg(&source)
            .output()
            .await
            .unwrap();

        assert!(!output.status.success(), "accepted malformed case {case}");
        assert!(
            !program.with_extension("fixture").exists(),
            "case {case} spawned the engine"
        );
        assert!(
            !program.with_extension("calls").exists(),
            "case {case} invoked the engine"
        );
        let expected_path = match case {
            "missing-lock" | "lock-is-directory" | "linked-required-file" => "Cargo.lock",
            "missing-ignore" => ".dockerignore",
            "images-is-file" => "images",
            "missing-provider-entrypoint" => "src/bin/ai-guard-provider.rs",
            "linked-required-parent" => "front/src",
            _ => unreachable!(),
        };
        let diagnostic = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            diagnostic.contains(expected_path),
            "case {case} omitted relative-path diagnostic {expected_path:?}"
        );
    }
}

#[tokio::test]
async fn build_accepts_complete_source_and_forwards_provider_versions() {
    let temporary = tempfile::tempdir().unwrap();
    let program = marker_program(temporary.path());
    let source = temporary.path().join("project");
    copy_public_build_source(&source);

    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
        .args(["build", "--runtime-program"])
        .arg(&program)
        .arg("--source")
        .arg(&source)
        .args(["--codex-version", "1.2.3", "--claude-code-version", "4.5.6"])
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "complete source rejected: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let calls = fs::read_to_string(program.with_extension("calls")).unwrap();
    let builds = calls
        .lines()
        .filter_map(|line| serde_json::from_str::<Vec<String>>(line).ok())
        .filter(|args| args.iter().any(|arg| arg == "build"))
        .collect::<Vec<_>>();
    assert_eq!(
        builds.len(),
        2,
        "expected exactly two image builds: {calls}"
    );
    let worker_build = builds
        .iter()
        .find(|args| args.iter().any(|arg| arg == "agentsandbox-worker:local"))
        .expect("worker image build missing");
    assert!(worker_build.iter().any(|arg| arg == "CODEX_VERSION=1.2.3"));
    assert!(
        worker_build
            .iter()
            .any(|arg| arg == "CLAUDE_CODE_VERSION=4.5.6")
    );
    assert!(
        builds.iter().all(|args| args.last().is_some_and(|path| {
            Path::new(path).is_absolute() && !path.ends_with(".Dockerfile")
        }))
    );
    for args in &builds {
        assert_engine_paths(args);
    }
}

/// The value that follows `flag` in one recorded engine invocation.
fn path_after<'a>(args: &'a [String], flag: &str) -> Option<&'a Path> {
    args.windows(2)
        .find(|pair| pair[0] == flag)
        .map(|pair| Path::new(&pair[1]))
}

/// Wherever the source the engine reads is located, one recorded build or
/// Compose invocation names it by absolute paths only: a build is given a
/// Dockerfile under `images/` of the very context it is given, a Compose
/// command its file under `deploy/docker/`.
fn assert_engine_paths(args: &[String]) {
    if args.iter().any(|arg| arg == "build") {
        let context = args.last().map(Path::new).expect("build context");
        let dockerfile = path_after(args, "--file").expect("build --file");
        assert!(
            context.is_absolute() && dockerfile.is_absolute(),
            "relative build path: {args:?}"
        );
        assert!(
            matches!(
                dockerfile.strip_prefix(context).ok().and_then(Path::to_str),
                Some("images/control.Dockerfile" | "images/worker.Dockerfile")
            ),
            "Dockerfile outside the context handed to the engine: {args:?}"
        );
    } else if args.iter().any(|arg| arg == "compose") {
        let file = path_after(args, "--file").expect("compose --file");
        assert!(
            file.is_absolute() && file.ends_with("deploy/docker/compose.yaml"),
            "relative or foreign Compose file: {args:?}"
        );
        assert!(
            path_after(args, "--env-file").is_none_or(Path::is_absolute),
            "relative Compose environment file: {args:?}"
        );
    }
}

#[tokio::test]
async fn explicit_operator_docker_endpoint_bypasses_context_and_rejects_network_targets() {
    let temporary = tempfile::tempdir().unwrap();
    let program = marker_program(temporary.path());
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
        .args(["doctor", "--runtime-program"])
        .arg(&program)
        .args(["--runtime-endpoint", "unix:///explicit-fixture.sock"])
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let calls = fs::read_to_string(program.with_extension("calls")).unwrap();
    assert!(!calls.contains("\"context\""));
    assert!(calls.contains("effect unix:///explicit-fixture.sock version"));
    for endpoint in [
        "tcp://127.0.0.1:2375",
        "ssh://engine",
        "unix://relative.sock",
    ] {
        let root = tempfile::tempdir().unwrap();
        let program = marker_program(root.path());
        let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
            .args(["doctor", "--runtime-program"])
            .arg(&program)
            .args(["--runtime-endpoint", endpoint])
            .output()
            .await
            .unwrap();
        assert!(!output.status.success(), "accepted {endpoint}");
        assert!(
            !program.with_extension("fixture").exists(),
            "invalid endpoint spawned the engine"
        );
    }
}

#[tokio::test]
async fn operators_keep_the_original_docker_endpoint_when_context_changes_after_preflight() {
    let temporary = tempfile::tempdir().unwrap();
    for action in ["doctor", "build", "up", "down"] {
        let root = temporary.path().join(action);
        fs::create_dir(&root).unwrap();
        let program = marker_program(&root);
        if action == "up" {
            fs::write(program.with_extension("case"), "compose-failure").unwrap();
        }
        let source = root.join("project");
        copy_public_build_source(&source);
        fs::create_dir_all(source.join("deploy/docker")).unwrap();
        fs::write(source.join("deploy/docker/compose.yaml"), "services: {}\n").unwrap();
        let config = root.join("config");
        agentsandbox::operations::initialize_config(&config).unwrap();
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
        command.arg(action).arg("--runtime-program").arg(&program);
        if action != "doctor" {
            command.arg("--source").arg(&source);
        }
        if matches!(action, "up" | "down") {
            command.arg("--config-dir").arg(&config);
        }
        let output = tokio::time::timeout(Duration::from_secs(20), command.output())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            output.status.success(),
            action != "up",
            "{action}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let calls = fs::read_to_string(program.with_extension("calls")).unwrap();
        let effects = calls
            .lines()
            .filter(|line| line.starts_with("effect "))
            .collect::<Vec<_>>();
        assert!(
            !effects.is_empty(),
            "{action}: no effect was exercised: {calls}"
        );
        assert!(
            effects
                .iter()
                .all(|line| line.starts_with("effect unix:///first-fixture.sock ")),
            "{action}: context changed destination: {calls}"
        );
        assert_eq!(
            fs::read_to_string(program.with_extension("context")).unwrap(),
            "unix:///second-fixture.sock"
        );
    }
}

#[tokio::test]
async fn operators_refuse_an_engine_id_change_before_build_or_compose_effects() {
    let temporary = tempfile::tempdir().unwrap();
    for action in ["build", "up", "down"] {
        let root = temporary.path().join(action);
        fs::create_dir(&root).unwrap();
        let program = marker_program(&root);
        fs::write(
            program.with_extension("case"),
            if action == "down" {
                "engine-changed"
            } else {
                "engine-after-doctor"
            },
        )
        .unwrap();
        let source = root.join("project");
        copy_public_build_source(&source);
        fs::create_dir_all(source.join("deploy/docker")).unwrap();
        fs::write(source.join("deploy/docker/compose.yaml"), "services: {}\n").unwrap();
        let config = root.join("config");
        agentsandbox::operations::initialize_config(&config).unwrap();
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
        command
            .arg(action)
            .arg("--runtime-program")
            .arg(&program)
            .arg("--source")
            .arg(&source);
        if action != "build" {
            command.arg("--config-dir").arg(&config);
        }
        let output = tokio::time::timeout(Duration::from_secs(20), command.output())
            .await
            .unwrap()
            .unwrap();
        assert!(
            !output.status.success(),
            "{action}: changed engine accepted"
        );
        let calls = fs::read_to_string(program.with_extension("calls")).unwrap();
        assert!(
            !calls.lines().any(|line| line.starts_with("effect ")),
            "{action}: effect before identity validation: {calls}"
        );
    }
}

fn copy_public_build_source(destination: &Path) {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"));
    fs::create_dir_all(destination).unwrap();
    for directory in [
        "images",
        "src",
        "src/bin",
        "front",
        "front/src",
        "front/public",
        "policies",
    ] {
        fs::create_dir_all(destination.join(directory)).unwrap();
    }
    for file in [
        "Cargo.toml",
        "Cargo.lock",
        "rust-toolchain.toml",
        ".dockerignore",
        "LICENSE",
        "images/control.Dockerfile",
        "images/worker.Dockerfile",
        "src/main.rs",
        "src/lib.rs",
        "src/bin/ai-guard-provider.rs",
        "front/package.json",
        "front/package-lock.json",
        "front/tsconfig.json",
        "front/vite.config.ts",
        "front/index.html",
        "front/src/main.ts",
        "front/src/style.css",
        "front/public/favicon.svg",
        "policies/default.json",
    ] {
        fs::copy(repository.join(file), destination.join(file)).unwrap();
    }
}

fn marker_program(root: &Path) -> std::path::PathBuf {
    let source = root.join("engine.rs");
    fs::write(&source, r#"
fn main() {
 for name in ["DOCKER_HOST", "DOCKER_CONTEXT", "LD_PRELOAD", "DYLD_INSERT_LIBRARIES", "AI_GUARD_TEST_PRIVATE"] {
  assert!(std::env::var_os(name).is_none(), "ambient environment was inherited");
 }
 let executable = std::env::current_exe().unwrap();
 assert_eq!(std::env::current_dir().unwrap(), executable.parent().unwrap());
 let marker = executable.with_extension("fixture");
 std::fs::write(marker, "executed").unwrap();
 use std::io::Write;
 let args = std::env::args().skip(1).collect::<Vec<_>>();
 let mut calls = std::fs::OpenOptions::new().create(true).append(true).open(executable.with_extension("calls")).unwrap();
 writeln!(calls, "{args:?}").unwrap();
 let context = executable.with_extension("context");
 let target = args.windows(2).find(|pair| pair[0] == "--host").map(|pair| pair[1].clone())
  .unwrap_or_else(||std::fs::read_to_string(&context).unwrap_or_else(|_|"unix:///first-fixture.sock".into()));
 let case = std::fs::read_to_string(executable.with_extension("case")).unwrap_or_default();
 if args.iter().any(|a| a == "context") {
  println!("{target:?}");
 } else if args.iter().any(|a| a == "info") {
  let count_path = executable.with_extension("info-count");
  let count = std::fs::read_to_string(&count_path).unwrap_or_default().parse::<u64>().unwrap_or(0);
  std::fs::write(count_path, (count + 1).to_string()).unwrap();
  std::fs::write(&context,"unix:///second-fixture.sock").unwrap();
  let id = if (case == "engine-changed" && count > 0) || (case == "engine-after-doctor" && count >= 3) || executable.with_extension("engine-switched").exists() {"changed-engine"} else {"fixture"};
  println!("{{\"ID\":\"{id}\",\"SecurityOptions\":[\"name=rootless\"],\"ServerVersion\":\"fixture\"}}");
 } else {
  let verb = args.iter().find(|a| matches!(a.as_str(), "version"|"build"|"compose")).unwrap();
  writeln!(calls,"effect {target} {verb}").unwrap();
  if case == "engine-after-build" && verb == "build" { std::fs::write(executable.with_extension("engine-switched"),"changed").unwrap(); }
  if case == "compose-failure" && verb == "compose" { std::process::exit(125); }
  println!("fixture");
 }
}

"#).unwrap();
    let program = root.join("docker");
    let output =
        std::process::Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
            .args(["--edition=2024", "--crate-name", "program_fixture"])
            .arg(source)
            .arg("-o")
            .arg(&program)
            .output()
            .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    program
}

#[tokio::test]
async fn doctor_ignores_project_program_for_empty_relative_and_absolute_path_entries() {
    let temporary = tempfile::tempdir().unwrap();
    let project = temporary.path().join("project");
    fs::create_dir(&project).unwrap();
    let program = marker_program(&project);
    let marker = program.with_extension("fixture");
    for path in ["".to_owned(), ".".to_owned(), project.display().to_string()] {
        let output = tokio::time::timeout(
            Duration::from_secs(20),
            tokio::process::Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
                .arg("doctor")
                .current_dir(&project)
                .env("PATH", &path)
                .kill_on_drop(true)
                .output(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            !marker.exists(),
            "doctor executed a project program for PATH={path:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[tokio::test]
async fn doctor_accepts_explicit_absolute_control_program_with_untrusted_path() {
    let temporary = tempfile::tempdir().unwrap();
    let control = temporary.path().join("control");
    fs::create_dir(&control).unwrap();
    let program = marker_program(&control);
    let project = temporary.path().join("project");
    fs::create_dir(&project).unwrap();
    let output = tokio::time::timeout(
        Duration::from_secs(20),
        tokio::process::Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
            .args(["doctor", "--runtime-program"])
            .arg(&program)
            .current_dir(&project)
            .env("PATH", ".")
            .kill_on_drop(true)
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(program.with_extension("fixture").exists());
}

#[tokio::test]
async fn build_and_compose_use_the_same_program_and_absolute_source_paths() {
    let temporary = tempfile::tempdir().unwrap();
    let control = temporary.path().join("control");
    fs::create_dir(&control).unwrap();
    let program = marker_program(&control);
    let source = temporary.path().join("project");
    copy_public_build_source(&source);
    fs::create_dir_all(source.join("deploy/docker")).unwrap();
    fs::write(source.join("deploy/docker/compose.yaml"), "services: {}\n").unwrap();
    let config = temporary.path().join("config");
    agentsandbox::operations::initialize_config(&config).unwrap();
    for action in ["build", "down"] {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
        command
            .arg(action)
            .arg("--runtime-program")
            .arg(&program)
            .arg("--source")
            .arg(".")
            .current_dir(&source)
            .env("PATH", ".")
            .env("DOCKER_HOST", "unix:///ambient-should-not-select.sock")
            .env("DOCKER_CONTEXT", "ambient-should-not-select")
            .env("AI_GUARD_TEST_PRIVATE", "private-never-inherited")
            .kill_on_drop(true);
        if action == "down" {
            command.arg("--config-dir").arg(&config);
        }
        let output = tokio::time::timeout(Duration::from_secs(20), command.output())
            .await
            .unwrap()
            .unwrap();
        assert!(
            output.status.success(),
            "{action}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let calls = fs::read_to_string(program.with_extension("calls")).unwrap();
    assert!(calls.contains("/images/control.Dockerfile\""));
    assert!(calls.contains("/images/worker.Dockerfile\""));
    assert!(calls.contains("/deploy/docker/compose.yaml\""));
    assert!(
        !calls.contains("\"./") && !calls.contains("\"images/") && !calls.contains("\"deploy/")
    );
    assert_eq!(
        calls
            .lines()
            .filter(|line| serde_json::from_str::<Vec<String>>(line)
                .is_ok_and(|args| args.iter().any(|arg| arg == "build")))
            .count(),
        2
    );
    let invocations = calls
        .lines()
        .filter_map(|line| serde_json::from_str::<Vec<String>>(line).ok())
        .collect::<Vec<_>>();
    for args in &invocations {
        assert_engine_paths(args);
    }
    assert_eq!(
        invocations
            .iter()
            .filter(|args| args.iter().any(|arg| arg == "compose"))
            .count(),
        1,
        "expected exactly one Compose command: {calls}"
    );
}

#[tokio::test]
async fn run_refuses_an_explicit_program_or_installation_link_inside_the_project_before_spawn() {
    let temporary = tempfile::tempdir().unwrap();
    let config = temporary.path().join("config");
    agentsandbox::operations::initialize_config(&config).unwrap();
    let project = temporary.path().join("project");
    fs::create_dir(&project).unwrap();
    let program = marker_program(&project);
    let control = temporary.path().join("control");
    fs::create_dir(&control).unwrap();
    let outside = marker_program(&control);
    let link = project.join("engine-link");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let indirect = control.join("engine-link-through-project");
    std::os::unix::fs::symlink(&link, &indirect).unwrap();
    for entry in [&program, &link, &indirect] {
        let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
            .arg("run")
            .arg("--runtime-program")
            .arg(entry)
            .arg("--config-dir")
            .arg(&config)
            .arg("--project")
            .arg(&project)
            .args(["--profile", "observe", "--", "/usr/bin/id"])
            .output()
            .await
            .unwrap();
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("outside the imported project"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!program.with_extension("fixture").exists());
        assert!(!outside.with_extension("fixture").exists());
    }
}
