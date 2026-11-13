use std::process::Command;

mod support;

// These tests cover CLI discovery, selection and dispatch to configuration
// validation only. They do not establish a live Full3 registration or claim.
async fn d5_u_cli(args: &[&str], directory: &std::path::Path) -> std::process::Output {
    use tokio::io::AsyncReadExt;

    const MAX_OUTPUT: u64 = 64 * 1024;
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
        .args(args)
        .current_dir(directory)
        .env_clear()
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn the real agentsandbox CLI");
    let mut stdout = child
        .stdout
        .take()
        .expect("piped stdout")
        .take(MAX_OUTPUT + 1);
    let mut stderr = child
        .stderr
        .take()
        .expect("piped stderr")
        .take(MAX_OUTPUT + 1);
    let mut out = Vec::new();
    let mut err = Vec::new();
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::try_join!(stdout.read_to_end(&mut out), stderr.read_to_end(&mut err))?;
        if out.len() > MAX_OUTPUT as usize || err.len() > MAX_OUTPUT as usize {
            child.kill().await?;
        }
        child.wait().await
    })
    .await;
    let status = match result {
        Ok(Ok(status)) => status,
        failure => {
            // Attempt wait even if signalling fails: an I/O error must not
            // skip explicit reaping, and cleanup never makes the test pass.
            let kill = child.start_kill();
            let reap = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await;
            panic!(
                "CLI failed within its bounded budget: {failure:?}; owned-child kill={kill:?}; reap={reap:?}"
            );
        }
    };
    assert!(out.len() <= MAX_OUTPUT as usize && err.len() <= MAX_OUTPUT as usize);
    std::process::Output {
        status,
        stdout: out,
        stderr: err,
    }
}

#[tokio::test]
async fn d5_u_serve_help_announces_explicit_full3_and_stop_role_options() {
    let directory = support::private_tempdir();
    let output = d5_u_cli(&["broker", "serve", "--help"], directory.path()).await;
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    for option in [
        "--enforcer-full3-new",
        "--enforcer-full3-existing",
        "--enforcer-role",
    ] {
        assert!(
            help.split_whitespace().any(|word| word == option),
            "serve help omitted {option}: {help}"
        );
    }
}

#[tokio::test]
async fn d5_u_serve_rejects_conflicting_full3_selections_before_configuration_io() {
    let directory = support::private_tempdir();
    for selection in [
        vec![
            "--enforcer-full3-new",
            "fresh",
            "--enforcer-full3-existing",
            "sealed",
        ],
        vec!["--enforcer-full3-new", "fresh", "--recovery-only"],
        vec!["--enforcer-full3-existing", "sealed", "--recovery-only"],
    ] {
        let mut args = vec!["broker", "serve", "--config", "absent.json"];
        args.extend(selection);
        let output = d5_u_cli(&args, directory.path()).await;
        let error = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            output.status.code(),
            Some(2),
            "CLI must reject incompatible selection: {error}"
        );
        assert!(
            error.contains("cannot be used with"),
            "expected a selection conflict, not an unknown option or configuration failure: {error}"
        );
        assert!(output.stdout.is_empty());
    }
}

#[tokio::test]
async fn d5_u_serve_parses_explicit_roles_then_reaches_private_configuration_refusal() {
    let directory = support::private_tempdir();
    // A known absent file prevents any service, guardian or runtime launch.
    // Establish the existing configuration boundary before testing new inputs.
    let baseline = ["broker", "serve", "--config", "absent.json"];
    let legacy = d5_u_cli(&baseline, directory.path()).await;
    assert!(!legacy.status.success());
    let expected = String::from_utf8_lossy(&legacy.stderr);
    assert!(
        expected.contains("private broker configuration is unavailable or unsafe"),
        "legacy preflight did not reach configuration validation: {expected}"
    );
    for selection in [
        vec!["--enforcer-full3-new", "fresh"],
        vec!["--enforcer-full3-existing", "sealed"],
        vec!["--recovery-only", "--enforcer-role", "sealed"],
    ] {
        let mut args = baseline.to_vec();
        args.extend(selection);
        let output = d5_u_cli(&args, directory.path()).await;
        assert_eq!(
            output.status.code(),
            legacy.status.code(),
            "selection must parse before the existing private configuration refusal: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            output.stderr, legacy.stderr,
            "selection did not reach the existing configuration boundary"
        );
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn broker_help_lists_the_approved_service_commands() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
        .args(["broker", "--help"])
        .output()
        .expect("run the real agentsandbox binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "broker --help must succeed; status={:?}; stderr={stderr}",
        output.status
    );
    for command in [
        "serve",
        "submit",
        "status",
        "recovery-status",
        "list",
        "approve",
        "cancel",
        "resume",
        "export",
    ] {
        assert!(
            stdout.split_whitespace().any(|word| word == command),
            "broker --help omitted `{command}`; stdout={stdout}"
        );
    }
}
