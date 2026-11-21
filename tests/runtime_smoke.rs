//! Requires a real, deliberately selected engine and the running control plane.
//! Kept ignored in normal CI: argument/unit tests are not runtime isolation proof.
//! Set AGENTSANDBOX_TEST_RUNTIME_ENDPOINT to the QA Unix socket to use the
//! same explicit target for the run and its probes. Native Podman also accepts
//! `local`. The Docker default uses the control account's current context.
use agentsandbox::{
    runner::{self, AuditClient, RunOptions},
    runtime::{NetworkMode, RuntimeKind},
};
use std::{
    fs,
    path::{Component, Path, PathBuf},
    time::Duration,
};

fn local_gateway(value: &str) -> bool {
    reqwest::Url::parse(value).is_ok_and(|url| {
        ["http", "https"].contains(&url.scheme())
            && url.username().is_empty()
            && url.password().is_none()
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none()
            && url.host_str().is_some_and(|host| {
                host == "localhost"
                    || host
                        .trim_matches(['[', ']'])
                        .parse::<std::net::IpAddr>()
                        .is_ok_and(|ip| ip.is_loopback())
            })
    })
}

#[test]
fn smoke_gateway_is_a_loopback_origin_before_any_credentials_are_loaded() {
    for value in [
        "http://127.0.0.1:8787",
        "http://[::1]:8787",
        "https://localhost:8787",
    ] {
        assert!(local_gateway(value));
    }
    for value in [
        "",
        "https://example.invalid",
        "http://192.0.2.1",
        "http://127.0.0.1@external.invalid",
        "http://user@localhost",
        "http://localhost/api",
        "http://localhost/?token=x",
        "http://localhost/#x",
    ] {
        assert!(!local_gateway(value));
    }
}

fn runtime_probe_target(
    runtime: RuntimeKind,
    endpoint: Option<&str>,
) -> anyhow::Result<Vec<String>> {
    let Some(endpoint) = endpoint.or((runtime == RuntimeKind::Podman).then_some("local")) else {
        return Ok(Vec::new());
    };
    if runtime == RuntimeKind::Podman && endpoint == "local" {
        anyhow::ensure!(
            cfg!(any(target_os = "linux", target_os = "freebsd")),
            "Podman local mode requires a native engine; select an explicit Unix socket"
        );
        return Ok(vec!["--remote=false".into()]);
    }
    let path = endpoint.strip_prefix("unix://").map(Path::new);
    anyhow::ensure!(
        path.is_some_and(|path| path.is_absolute()
            && !path.components().any(|part| part == Component::ParentDir))
            && endpoint.len() <= 4096
            && !endpoint.chars().any(char::is_control)
            && !endpoint.contains(['?', '#', '@']),
        "runtime smoke endpoint must be an absolute local Unix socket"
    );
    Ok(match runtime {
        RuntimeKind::Docker => vec!["--host".into(), endpoint.into()],
        RuntimeKind::Podman => vec!["--remote".into(), "--url".into(), endpoint.into()],
    })
}

#[test]
fn smoke_runtime_probes_use_the_selected_local_target() {
    let endpoint = "unix:///private/qa/engine.sock";
    assert_eq!(
        runtime_probe_target(RuntimeKind::Docker, Some(endpoint)).unwrap(),
        ["--host", endpoint]
    );
    assert_eq!(
        runtime_probe_target(RuntimeKind::Podman, Some(endpoint)).unwrap(),
        ["--remote", "--url", endpoint]
    );
    assert!(
        runtime_probe_target(RuntimeKind::Docker, None)
            .unwrap()
            .is_empty()
    );
    for endpoint in [None, Some("local")] {
        let target = runtime_probe_target(RuntimeKind::Podman, endpoint);
        if cfg!(any(target_os = "linux", target_os = "freebsd")) {
            assert_eq!(target.unwrap(), ["--remote=false"]);
        } else {
            assert!(target.is_err());
        }
    }
    for endpoint in [
        "",
        "local",
        "tcp://127.0.0.1:2375",
        "unix:///qa/../engine.sock",
    ] {
        assert!(runtime_probe_target(RuntimeKind::Docker, Some(endpoint)).is_err());
    }
}

#[tokio::test]
#[ignore = "requires built images, an explicitly selected real Docker/Podman engine, and a running gateway; not executed in the authoring environment"]
async fn real_worker_copy_uid_and_export_preserve_the_original_project() {
    let runtime = match std::env::var("AGENTSANDBOX_TEST_RUNTIME").as_deref() {
        Ok("docker") => RuntimeKind::Docker,
        Ok("podman") => RuntimeKind::Podman,
        _ => panic!("set AGENTSANDBOX_TEST_RUNTIME=docker or podman explicitly"),
    };
    let config = PathBuf::from(
        std::env::var("AGENTSANDBOX_TEST_CONFIG")
            .expect("set AGENTSANDBOX_TEST_CONFIG to initialized configuration"),
    );
    let allow_rootful = std::env::var("AGENTSANDBOX_TEST_ALLOW_ROOTFUL").as_deref() == Ok("1");
    let gateway = std::env::var("AGENTSANDBOX_TEST_GATEWAY")
        .unwrap_or_else(|_| "http://127.0.0.1:8787".into());
    assert!(
        local_gateway(&gateway),
        "the local smoke gateway must be a loopback origin without URL credentials, path, query or fragment"
    );
    let image = std::env::var("AGENTSANDBOX_TEST_IMAGE")
        .unwrap_or_else(|_| "agentsandbox-worker:local".into());
    let runtime_endpoint = match std::env::var("AGENTSANDBOX_TEST_RUNTIME_ENDPOINT") {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => panic!("runtime smoke endpoint must be UTF-8"),
    };
    let engine_target = runtime_probe_target(runtime, runtime_endpoint.as_deref()).unwrap();
    let source = tempfile::tempdir().unwrap().keep();
    eprintln!("Retained runtime QA source: {}", source.display());
    fs::write(source.join("original.txt"), "original unchanged").unwrap();
    fs::write(source.join(".env"), "PRIVATE_TEST_VALUE=never-export").unwrap();
    let options = RunOptions {
        project: source.clone(),
        config_dir: config.clone(),
        gateway: gateway.clone(),
        runtime,
        runtime_program: std::env::var_os("AGENTSANDBOX_TEST_RUNTIME_PROGRAM").map(PathBuf::from),
        runtime_endpoint,
        image,
        profile: "strict".into(),
        network: NetworkMode::None,
        allow_rootful_runtime: allow_rootful,
        timeout: 30,
        provider: "generic".into(),
        approve_hash: None,
        capture_output: true,
        interactive: false,
        secret_env_file: None,
        client: None,
        auth_mode: None,
        client_profile: None,
        providers_file: None,
        prompt: None,
        model: None,
        argv: vec!["/usr/bin/id".into()],
    };
    let id = runner::run(options.clone()).await.unwrap();
    assert_eq!(id.exit_code, 0);
    let token = runner::read_token(&config.join("secrets/viewer")).unwrap();
    let client = AuditClient::new(&gateway, &token).unwrap();
    let events = client
        .get_json(&format!(
            "api/events?sandbox_id={}&limit=100",
            id.session_id
        ))
        .await
        .unwrap();
    let completed = events["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["kind"] == "command.completed")
        .expect("durably audited completion");
    assert_eq!(completed["session_id"], id.session_id.to_string());
    assert_eq!(completed["sandbox_id"], id.session_id.to_string());
    assert!(
        completed["result"]["stdout"]
            .as_str()
            .unwrap()
            .contains("uid=1000")
    );
    let status = runner::worker_status(id.session_id, &config, &gateway)
        .await
        .unwrap();
    assert_eq!(status["worker"]["state"], "terminal");
    assert_eq!(status["worker"]["quota_held"], false);
    let worker_id = status["worker"]["worker_id"].as_str().unwrap();
    let usage = client.get_json("api/v1/usage").await.unwrap();
    let allocation = usage["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["worker_id"] == worker_id)
        .expect("real terminal allocation")
        .clone();
    assert_eq!(allocation["billable"], false);
    assert_eq!(allocation["source"]["provenance"], "observed");
    assert_eq!(
        allocation["source"]["container_id"],
        status["worker"]["attempt"]["container_id"]
    );
    assert_eq!(
        allocation["source"]["spec_hash"],
        status["worker"]["spec_hash"]
    );
    assert!(
        allocation["duration_ns"]
            .as_str()
            .unwrap()
            .parse::<u128>()
            .unwrap()
            > 0
    );
    for _ in 0..2 {
        let resumed = runner::resume_worker(id.session_id, &config, &gateway, 5)
            .await
            .unwrap();
        assert_eq!(resumed["worker"]["worker_id"], worker_id);
        assert_eq!(resumed["worker"]["state"], "terminal");
    }
    let usage_after = client.get_json("api/v1/usage").await.unwrap();
    let rows: Vec<_> = usage_after["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["worker_id"] == worker_id)
        .collect();
    assert_eq!(rows.len(), 1, "two resumes must not double the allocation");
    assert_eq!(
        rows[0], &allocation,
        "reconciliation must preserve exact amounts and provenance"
    );
    let engine = match &options.runtime_program {
        Some(program) => agentsandbox::runtime::Runtime::with_program(runtime, program),
        None => agentsandbox::runtime::Runtime::new(runtime),
    }
    .unwrap();
    let probe = |args: &[&str]| {
        let mut command = engine_target.clone();
        command.extend(args.iter().map(|argument| (*argument).to_owned()));
        let engine = &engine;
        async move { engine.cli(&command, Duration::from_secs(15)).await }
    };
    let volume_list = ["volume", "ls", "--format", "{{.Name}}"];
    let before_denial = probe(&volume_list).await.unwrap();
    assert!(!before_denial.truncated);
    let before_names: std::collections::BTreeSet<_> = std::str::from_utf8(&before_denial.bytes)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    let marker = format!("denied-{}", uuid::Uuid::new_v4());
    let mut denied = options.clone();
    // If a regression executes the denied command on the host, this path is
    // writable by the test and makes that fallback observable and harmless.
    denied.argv = vec![
        "/usr/bin/touch".into(),
        source.join(&marker).to_str().unwrap().into(),
    ];
    assert!(runner::run(denied).await.is_err());
    let denial = client
        .get_json(&format!(
            "api/events?kind=security.decision&q={marker}&limit=10"
        ))
        .await
        .unwrap();
    let denied_events = denial["items"].as_array().unwrap();
    assert_eq!(denied_events.len(), 1);
    assert_eq!(denied_events[0]["decision"]["outcome"], "deny");
    let denied_session =
        uuid::Uuid::parse_str(denied_events[0]["sandbox_id"].as_str().unwrap()).unwrap();
    let containers = probe(&[
        "ps",
        "--all",
        "--filter",
        &format!("label=agentsandbox.session={denied_session}"),
        "--format",
        "{{.Names}}",
    ])
    .await
    .unwrap();
    assert!(!containers.truncated);
    assert!(
        containers.bytes.is_empty(),
        "a policy denial must create no container"
    );
    // Also require a positive control in the same successful engine response,
    // so an unsupported filter or engine outage cannot look like absence.
    let volumes = probe(&volume_list).await.unwrap();
    assert!(!volumes.truncated);
    let volume_names = std::str::from_utf8(&volumes.bytes).unwrap();
    assert!(volume_names.lines().any(|name| name == id.volume));
    let after_names: std::collections::BTreeSet<_> =
        volume_names.lines().map(str::to_owned).collect();
    assert_eq!(
        before_names, after_names,
        "denial must leave the actual engine volume inventory unchanged; run this QA smoke without concurrent volume operations"
    );
    assert!(!source.join(&marker).exists());
    let mut options = options;
    options.profile = "observe".into();
    options.argv = vec!["/usr/bin/touch".into(), "/workspace/generated.txt".into()];
    let mutation = runner::run(options.clone()).await.unwrap();
    assert_eq!(mutation.exit_code, 0);
    assert!(!source.join("generated.txt").exists());
    let parent = tempfile::tempdir().unwrap().keep();
    let destination = parent.join("export");
    eprintln!("Retained runtime QA export: {}", parent.display());
    runner::export_workspace(mutation.session_id, destination.clone(), &config, &gateway)
        .await
        .unwrap();
    assert!(destination.join("generated.txt").is_file());
    assert!(!destination.join(".env").exists());
    assert_eq!(
        fs::read_to_string(source.join("original.txt")).unwrap(),
        "original unchanged"
    );
    // A synthetic provider value is supplied by the explicit credential path,
    // never by argv or a control credential. No provider request is performed.
    let credentials = tempfile::tempdir().unwrap().keep();
    let credential_path = credentials.join("capture.env");
    let secret = format!("capture-value-{}", "Q".repeat(4097 - 14));
    assert_eq!(secret.len(), 4097);
    fs::write(&credential_path, format!("PRIVATE_TOKEN={secret}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&credential_path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let mut capture = options;
    capture.network = NetworkMode::Bridge; // Existing explicit credential profile.
    capture.secret_env_file = Some(credential_path);
    capture.argv = vec!["/bin/sh".into(), "-c".into(),
        "printf 'capture-safe\\n'; i=0; while [ \"$i\" -lt 64 ]; do printf '%s' \"$PRIVATE_TOKEN\"; printf '%s' \"$PRIVATE_TOKEN\" >&2; i=$((i+1)); done".into()];
    let captured = runner::run(capture).await.unwrap();
    assert_eq!(captured.exit_code, 0);
    let history = client
        .get_json(&format!(
            "api/events?sandbox_id={}&limit=100",
            captured.session_id
        ))
        .await
        .unwrap();
    let terminal = history["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| {
            item["kind"] == "command.completed"
                && item["session_id"] == captured.session_id.to_string()
        })
        .unwrap();
    let encoded = serde_json::to_string(terminal).unwrap();
    assert!(
        !encoded.contains(&secret[..32]),
        "neither value nor boundary fragment may persist"
    );
    assert_eq!(terminal["result"]["stdout_truncated"], true);
    assert_eq!(terminal["result"]["stderr_truncated"], true);
    assert!(
        terminal["result"]["stdout"]
            .as_str()
            .unwrap()
            .contains("capture-safe")
    );
    assert!(
        terminal["result"]["stdout"]
            .as_str()
            .unwrap()
            .contains("[REDACTED]")
    );
    assert!(
        terminal["result"]["stderr"]
            .as_str()
            .unwrap()
            .contains("[REDACTED]")
    );
    eprintln!(
        "Validated real bounded secret capture session {}; synthetic credentials retained privately at {}. No provider request was performed.",
        captured.session_id,
        credentials.display()
    );
    eprintln!(
        "Validated real sessions {} and {}, policy denial {} without container/volume. Named volumes retained; source proof {}; export proof {}.",
        id.session_id,
        mutation.session_id,
        denied_session,
        source.display(),
        parent.display()
    );
}
