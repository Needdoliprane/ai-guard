#![cfg(unix)]

use agentsandbox::{
    broker::{
        local_ipc::{EndpointRecordV1, PrivateListener, read_frame, write_frame},
        service::{
            RequestedExecutionV1, SubmitV1,
            protocol::{
                BrokerCommandV1, BrokerErrorCodeV1, BrokerReplyV1, JobResultOutcomeV1, JobResultV1,
                TerminalJobResultV1,
            },
        },
    },
    control_fs::Directory,
    model::{Decision, Outcome},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs::{self, DirBuilder, OpenOptions, Permissions},
    io::Write,
    os::unix::{
        fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        process::ExitStatusExt,
    },
    path::{Path, PathBuf},
    process::{Output, Stdio},
    time::Duration,
};
use tempfile::TempDir;
use tokio::{
    io::AsyncReadExt,
    net::UnixStream,
    process::{Child, Command},
    time::timeout,
};
use uuid::Uuid;

const PROFILE: &str = "run-cli-fixture";
const TOKEN: &str = "run-cli-synthetic-credential";
const BROKER_SELECTION_ENV: &str = "AGENTSANDBOX_BROKER_SELECTION";
const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const FRAME_CAP: u32 = 512 * 1024;
const ACCEPT_DEADLINE: Duration = Duration::from_secs(15);
const IPC_DEADLINE: Duration = Duration::from_secs(5);
const CLI_DEADLINE: Duration = Duration::from_secs(30);

struct Fixture {
    _temporary: TempDir,
    root: PathBuf,
    project: PathBuf,
    token: PathBuf,
    path: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        #[cfg(target_os = "macos")]
        let base = "/private/tmp";
        #[cfg(not(target_os = "macos"))]
        let base = "/tmp";

        let temporary = tempfile::Builder::new()
            .prefix("br")
            .permissions(Permissions::from_mode(0o700))
            .tempdir_in(base)
            .unwrap();
        let directory = temporary.path().canonicalize().unwrap();
        let root = directory.join("root");
        let project = directory.join("project");
        let path = directory.join("bin");
        for child in [&root, &project, &path] {
            private_dir(child);
        }
        let token = directory.join("token");
        write_new(&token, TOKEN.as_bytes());
        for name in ["docker", "podman"] {
            let executable = path.join(name);
            write_new(
                &executable,
                b"#!/bin/sh\nprintf 'invoked\\n' > \"${0%/*}/engine-invoked\"\nexit 89\n",
            );
            fs::set_permissions(executable, Permissions::from_mode(0o700)).unwrap();
        }
        Self {
            _temporary: temporary,
            root,
            project,
            token,
            path,
        }
    }

    fn cli(&self, operation: &str) -> Command {
        let mut child = Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
        child
            .env_clear()
            .env("PATH", &self.path)
            .current_dir(&self.project)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .arg(operation);
        child
    }

    fn listen(&self) -> PrivateListener {
        let role = self.root.join("broker");
        let endpoints = role.join("endpoints");
        let announcements = role.join("announcements");
        for child in [&role, &endpoints, &announcements] {
            private_dir(child);
        }
        self.publish_listener(1)
    }

    fn publish_listener(&self, sequence: u32) -> PrivateListener {
        let roles = self.root.join("broker");
        let endpoints = Directory::open_existing(&roles.join("endpoints")).unwrap();
        let listener = PrivateListener::bind_new(&endpoints).unwrap();
        let announcement = Announcement {
            version: 1,
            sequence,
            endpoint: listener.endpoint(),
        };
        write_new(
            &roles.join(format!("announcements/e-{sequence:04}.json")),
            &serde_json::to_vec(&announcement).unwrap(),
        );
        listener
    }

    fn broker_selection(&self) -> Command {
        let mut command = self.cli("run");
        command
            .arg("--broker-root")
            .arg(&self.root)
            .args(["--broker-profile", PROFILE, "--broker-token-file"])
            .arg(&self.token)
            .arg("--project")
            .arg(&self.project);
        command
    }

    fn registry(&self) -> PathBuf {
        let directory = self.root.parent().unwrap();
        let registry = directory.join("broker-registry.json");
        let profile = serde_json::json!({
            "id": PROFILE,
            "gateway": "http://127.0.0.1:9",
            "tenant_id": "run-cli-tenant",
            "control_namespace_uuid": Uuid::new_v4(),
            "config_dir": directory.join("config"),
            "runner_token_file": self.token,
            "runtime": "docker",
            "runtime_program": self.path.join("docker"),
            "runtime_endpoint": "unix:///not-opened/broker-run.sock",
            "image": "worker:synthetic",
            "policy_profile": "strict",
            "network": "none",
            "allow_rootful_runtime": false,
            "max_timeout_seconds": 30,
            "allow_capture_output": false,
            "managed_clients": false,
            "source_roots": [self.project],
            "export_roots": [directory.join("export")],
            "prearmed_start_profile": { "version": 1, "mode": "docker_minimal" }
        });
        let mut neighbor = profile.clone();
        neighbor["id"] = serde_json::json!("run-cli-neighbor");
        neighbor["tenant_id"] = serde_json::json!("run-cli-neighbor-tenant");
        neighbor["control_namespace_uuid"] = serde_json::json!(Uuid::new_v4());
        neighbor["runner_token_file"] = serde_json::json!(directory.join("neighbor-token-absent"));
        let data = serde_json::json!({
            "version": 2,
            "installation_id": Uuid::new_v4(),
            "root": self.root,
            "max_enforcer_slots": 8,
            "profiles": [neighbor, profile]
        });
        write_new(&registry, &serde_json::to_vec(&data).unwrap());
        registry
    }

    fn configured_broker_run(&self, registry: &Path, operation: Uuid) -> Command {
        let mut command = self.cli("run");
        command
            .arg("--broker-config")
            .arg(registry)
            .args(["--broker-profile", PROFILE, "--operation-id"])
            .arg(operation.to_string())
            .arg("--project")
            .arg(&self.project);
        command
    }

    fn configured_broker_command(&self, name: &str, registry: &Path) -> Command {
        let mut command = self.cli("broker");
        command
            .args([name, "--broker-config"])
            .arg(registry)
            .args(["--profile", PROFILE]);
        command
    }

    fn prepare_broker_client(&self, registry: &Path, destination: &Path) -> Command {
        let mut command = self.cli("broker");
        command
            .args(["prepare-client", "--broker-config"])
            .arg(registry)
            .args(["--profile", PROFILE, "--destination"])
            .arg(destination);
        command
    }

    fn broker_run(&self, operation: Uuid) -> Command {
        let mut command = self.broker_selection();
        command.arg("--operation-id").arg(operation.to_string());
        command
    }

    fn assert_legacy_untouched(&self) {
        assert!(!self.path.join("engine-invoked").exists());
        assert!(!self.project.join(".agentsandbox").exists());
    }
}

fn private_dir(path: &Path) {
    DirBuilder::new().mode(0o700).create(path).unwrap();
}

fn write_new(path: &Path, bytes: &[u8]) {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap()
        .write_all(bytes)
        .unwrap();
}

#[derive(Serialize)]
struct Announcement<'a> {
    version: u32,
    sequence: u32,
    endpoint: &'a EndpointRecordV1,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: u32,
    profile_id: String,
    credential: String,
    request: BrokerCommandV1,
}

#[derive(Serialize)]
struct Reply {
    version: u32,
    reply: BrokerReplyV1,
}

async fn request(listener: &PrivateListener) -> (UnixStream, BrokerCommandV1) {
    let mut connection = timeout(ACCEPT_DEADLINE, listener.accept_checked())
        .await
        .expect("CLI did not connect to the selected broker")
        .expect("fixture IPC connection failed");
    let request: Request = read_frame(&mut connection, FRAME_CAP, IPC_DEADLINE)
        .await
        .expect("CLI did not send a valid IPC request");
    assert_eq!(request.version, 1);
    assert_eq!(request.profile_id, PROFILE);
    assert!(
        request.credential == TOKEN,
        "CLI sent a different credential"
    );
    (connection, request.request)
}

async fn reply(mut connection: UnixStream, reply: BrokerReplyV1) {
    write_frame(
        &mut connection,
        &Reply { version: 1, reply },
        FRAME_CAP,
        IPC_DEADLINE,
    )
    .await
    .expect("fixture broker could not send its reply");
}

async fn collect(child: Child) -> Output {
    let output = timeout(CLI_DEADLINE, child.wait_with_output())
        .await
        .expect("CLI command exceeded its test deadline")
        .expect("read the real agentsandbox binary output");
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains(TOKEN)
            && !String::from_utf8_lossy(&output.stderr).contains(TOKEN),
        "CLI surfaced the fixture credential"
    );
    output
}

async fn run(mut command: Command) -> Output {
    collect(command.spawn().expect("start the real agentsandbox binary")).await
}

async fn run_without_contact(command: Command, listener: &PrivateListener) -> Output {
    tokio::select! {
        biased;
        _ = listener.accept_checked() => panic!("CLI opened an unexpected broker connection"),
        output = run(command) => output,
    }
}

async fn assert_no_connection(listener: &PrivateListener) {
    assert!(
        timeout(Duration::from_millis(50), listener.accept_checked())
            .await
            .is_err(),
        "CLI opened an unexpected broker connection"
    );
}

fn require_error(output: &Output, diagnostic: &str) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "CLI accepted the failing run");
    assert!(
        stderr.contains(diagnostic),
        "expected {diagnostic:?} in {stderr}"
    );
}

fn require_operation(output: &Output, operation: Uuid) {
    let reported = String::from_utf8_lossy(&output.stderr);
    assert!(
        reported.contains(&operation.to_string()),
        "operation ID lost: {reported}"
    );
}

fn expect_submit(command: BrokerCommandV1, project: &Path) -> SubmitV1 {
    let BrokerCommandV1::Submit { payload } = command else {
        panic!("expected Submit as the first broker command");
    };
    let input: SubmitV1 = serde_json::from_str(&payload).expect("valid broker submission");
    assert_eq!(input.version, 1);
    assert!(!input.operation_id.is_nil());
    assert_eq!(input.project, project);
    let RequestedExecutionV1::Command { argv } = &input.execution else {
        panic!("manual CLI argv did not produce a manual broker request");
    };
    assert_eq!(argv.as_slice(), ["/usr/bin/id"]);
    assert!(!input.capture_output);
    input
}

#[tokio::test]
async fn selected_broker_is_never_started_implicitly() {
    let fixture = Fixture::new();
    let operation = Uuid::new_v4();
    let mut command = fixture.broker_run(operation);
    command.args(["--", "/usr/bin/id"]);
    let output = run(command).await;
    require_error(&output, "start broker serve explicitly");
    require_operation(&output, operation);
    assert!(output.stdout.is_empty());
    assert!(!fixture.root.join("broker").exists());
    assert!(!fixture.root.join("enforcer").exists());
    fixture.assert_legacy_untouched();
}

#[tokio::test]
async fn incomplete_broker_selection_fails_before_legacy_execution() {
    let fixture = Fixture::new();
    let listener = fixture.listen();
    let registry = fixture.registry();
    for (flag, value) in [
        ("--broker-root", fixture.root.to_str().unwrap()),
        ("--broker-config", registry.to_str().unwrap()),
        ("--broker-profile", PROFILE),
        ("--broker-token-file", fixture.token.to_str().unwrap()),
    ] {
        let mut command = fixture.cli("run");
        command.args([flag, value, "--", "/usr/bin/id"]);
        let output = run_without_contact(command, &listener).await;
        require_error(&output, "broker");
        assert!(output.stdout.is_empty());
        fixture.assert_legacy_untouched();
    }
}

#[tokio::test]
async fn stale_latest_broker_neither_uses_an_older_endpoint_nor_falls_back() {
    let fixture = Fixture::new();
    let old_listener = fixture.listen();
    drop(fixture.publish_listener(2));

    let operation = Uuid::new_v4();
    let mut command = fixture.broker_run(operation);
    command.args(["--", "/usr/bin/id"]);
    let output = run_without_contact(command, &old_listener).await;
    require_error(&output, "broker is unavailable; request was not sent");
    require_operation(&output, operation);
    assert!(output.stdout.is_empty());
    fixture.assert_legacy_untouched();
}

#[tokio::test]
async fn unauthorized_retry_keeps_the_operation_id_and_never_falls_back() {
    let fixture = Fixture::new();
    let listener = fixture.listen();
    let deny = || async {
        let (connection, command) = request(&listener).await;
        let submit = expect_submit(command, &fixture.project);
        assert_eq!(submit.timeout_seconds, None);
        reply(
            connection,
            BrokerReplyV1::Error {
                code: BrokerErrorCodeV1::Unauthorized,
            },
        )
        .await;
        submit
    };

    let mut first = fixture.broker_selection();
    first.args(["--", "/usr/bin/id"]);
    let (first_output, first_submit) = tokio::join!(run(first), deny());
    let mut retry = fixture.broker_run(first_submit.operation_id);
    retry.args(["--", "/usr/bin/id"]);
    let (retry_output, retry_submit) = tokio::join!(run(retry), deny());
    assert_eq!(
        serde_json::to_vec(&retry_submit).unwrap(),
        serde_json::to_vec(&first_submit).unwrap(),
        "an explicit replay must preserve the generated operation and its payload"
    );
    for output in [first_output, retry_output] {
        require_error(&output, "not authenticated");
        require_operation(&output, first_submit.operation_id);
        assert!(output.stdout.is_empty());
    }
    assert_no_connection(&listener).await;
    fixture.assert_legacy_untouched();
}

#[tokio::test]
async fn explicit_legacy_flags_are_rejected_before_contacting_the_selected_broker() {
    let fixture = Fixture::new();
    let listener = fixture.listen();
    for flags in [
        &["--runtime", "docker"][..],
        &["--runtime-program", "/not-a-runtime"],
        &["--runtime-endpoint", "unix:///not-a-runtime.sock"],
        &["--image", "agentsandbox-worker:local"],
        &["--profile", "strict"],
        &["--network", "none"],
        &["--gateway", "http://127.0.0.1:8787"],
        &["--config-dir", ".agentsandbox"],
        &["--provider", "generic"],
        &["--providers-file", "unsupported.toml"],
        &["--approve-hash", "not-a-broker-approval"],
        &["--interactive"],
        &["--secret-env-file", "unsupported.env"],
        &["--allow-rootful-runtime"],
    ] {
        let mut command = fixture.broker_run(Uuid::new_v4());
        command.args(flags).args(["--", "/usr/bin/id"]);
        let output = run_without_contact(command, &listener).await;
        require_error(&output, flags[0]);
        require_error(&output, "broker");
        fixture.assert_legacy_untouched();
    }
}

async fn accept_submission(
    listener: &PrivateListener,
    operation: Uuid,
    project: &Path,
) -> SubmitV1 {
    let (connection, command) = request(listener).await;
    let submit = expect_submit(command, project);
    assert_eq!(submit.operation_id, operation);
    reply(
        connection,
        BrokerReplyV1::Accepted {
            operation_id: operation,
            intent_hash: HASH.into(),
            submit_hash: HASH.into(),
            revision: 1,
            checkpoint_hash: HASH.into(),
            replayed: false,
            provider_resolution: None,
        },
    )
    .await;
    submit
}

fn job_result(operation_id: Uuid, outcome: JobResultOutcomeV1) -> JobResultV1 {
    JobResultV1 {
        version: 1,
        operation_id,
        cancel_requested: matches!(outcome, JobResultOutcomeV1::Cancelled),
        outcome,
    }
}

async fn poll_result(listener: &PrivateListener, result: JobResultV1) {
    let (connection, command) = request(listener).await;
    expect_result_query(command, result.operation_id);
    reply(connection, BrokerReplyV1::Result { result }).await;
}

fn expect_result_query(command: BrokerCommandV1, operation: Uuid) {
    let BrokerCommandV1::Result {
        version,
        operation_id,
    } = command
    else {
        panic!("synchronous run must query the authenticated result");
    };
    assert_eq!(version, 1);
    assert_eq!(operation_id, operation);
}

fn assert_returned_result(output: &Output, expected: &JobResultV1, success: bool) {
    assert_eq!(
        output.status.success(),
        success,
        "unexpected run exit; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    require_operation(output, expected.operation_id);
    let actual: Value = serde_json::from_slice(&output.stdout).expect("one JSON run result");
    assert_eq!(actual, serde_json::to_value(expected).unwrap());
}

#[tokio::test]
async fn installed_broker_registry_selects_the_exact_profile_for_supervised_run() {
    let fixture = Fixture::new();
    let listener = fixture.listen();
    let registry = fixture.registry();
    let original = fs::read(&registry).unwrap();
    let operation = Uuid::new_v4();
    let mut command = fixture.configured_broker_run(&registry, operation);
    command.args([
        "--timeout",
        "19",
        "--broker-wait-seconds",
        "8",
        "--",
        "/usr/bin/id",
    ]);
    let serve_result = async {
        let submit = accept_submission(&listener, operation, &fixture.project).await;
        assert_eq!(submit.timeout_seconds, Some(19));
        poll_result(
            &listener,
            job_result(operation, JobResultOutcomeV1::Cancelled),
        )
        .await;
    };
    let (output, ()) = tokio::join!(run(command), serve_result);
    assert_returned_result(
        &output,
        &job_result(operation, JobResultOutcomeV1::Cancelled),
        false,
    );
    assert_eq!(fs::read(&registry).unwrap(), original);
    assert_no_connection(&listener).await;
    fixture.assert_legacy_untouched();
}

#[tokio::test]
async fn installed_broker_registry_refusals_do_not_contact_broker_or_run_directly() {
    let fixture = Fixture::new();
    let listener = fixture.listen();
    let registry = fixture.registry();
    let operation = Uuid::new_v4();
    for (flags, diagnostic) in [
        (
            &["--broker-root", fixture.root.to_str().unwrap()][..],
            "--broker-root",
        ),
        (
            &["--broker-token-file", fixture.token.to_str().unwrap()][..],
            "--broker-token-file",
        ),
        (&["--interactive"][..], "--interactive"),
        (&["--runtime", "docker"][..], "--runtime"),
    ] {
        let mut command = fixture.configured_broker_run(&registry, operation);
        command.args(flags).args(["--", "/usr/bin/id"]);
        let output = run_without_contact(command, &listener).await;
        require_error(&output, diagnostic);
        require_error(&output, "--broker-config");
        assert!(output.stdout.is_empty());
    }
    let mut unknown = fixture.cli("run");
    unknown.arg("--broker-config").arg(&registry).args([
        "--broker-profile",
        "not-configured",
        "--",
        "/usr/bin/id",
    ]);
    let output = run_without_contact(unknown, &listener).await;
    require_error(&output, "broker request is not authorized");
    assert!(output.stdout.is_empty());

    fs::set_permissions(&registry, Permissions::from_mode(0o644)).unwrap();
    for path in [&registry, Path::new("broker-registry.json")] {
        let mut command = fixture.configured_broker_run(path, operation);
        command.args(["--", "/usr/bin/id"]);
        let output = run_without_contact(command, &listener).await;
        require_error(
            &output,
            "private broker configuration is unavailable or unsafe",
        );
        assert!(output.stdout.is_empty());
    }
    fixture.assert_legacy_untouched();
}

#[tokio::test]
async fn installed_broker_registry_replacement_stops_client_polling() {
    let fixture = Fixture::new();
    let listener = fixture.listen();
    let registry = fixture.registry();
    let operation = Uuid::new_v4();
    let mut command = fixture.configured_broker_run(&registry, operation);
    command.args(["--", "/usr/bin/id"]);
    let child = command.spawn().expect("start the real agentsandbox binary");

    accept_submission(&listener, operation, &fixture.project).await;
    let (connection, command) = request(&listener).await;
    expect_result_query(command, operation);
    let original = fs::read(&registry).unwrap();
    fs::rename(&registry, registry.with_extension("previous")).unwrap();
    write_new(&registry, &original);
    reply(
        connection,
        BrokerReplyV1::Result {
            result: job_result(operation, JobResultOutcomeV1::Pending),
        },
    )
    .await;

    let output = tokio::select! {
        biased;
        _ = listener.accept_checked() => panic!("CLI reused a replaced broker registry"),
        output = collect(child) => output,
    };
    require_error(&output, "private broker configuration changed");
    require_operation(&output, operation);
    assert!(output.stdout.is_empty());
    assert_no_connection(&listener).await;
    fixture.assert_legacy_untouched();
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn broker_service_plan_refuses_an_absent_full3_role_without_publishing() {
    let fixture = Fixture::new();
    let registry = fixture.registry();
    let original = fs::read(&registry).unwrap();
    let platform = if cfg!(target_os = "macos") {
        "launchd"
    } else {
        "systemd"
    };
    let child = fixture
        .cli("broker")
        .args(["service", "plan", "--platform", platform, "--config"])
        .arg(&registry)
        .args(["--enforcer-full3-existing", "missing-full3-role"])
        .spawn()
        .expect("start the real agentsandbox binary");
    let output = collect(child).await;

    require_error(&output, "private Full3 role directory is unavailable");
    assert!(output.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&output.stderr).contains(TOKEN));
    assert_eq!(fs::read(&registry).unwrap(), original);
    assert!(fs::read_dir(&fixture.root).unwrap().next().is_none());
    fixture.assert_legacy_untouched();
}

#[tokio::test]
async fn synchronous_review_keeps_the_exact_hash_and_creates_no_approval() {
    let fixture = Fixture::new();
    let listener = fixture.listen();
    let operation = Uuid::new_v4();
    let mut command = fixture.broker_run(operation);
    command.args([
        "--broker-wait-seconds",
        "8",
        "--timeout",
        "300",
        "--",
        "/usr/bin/id",
    ]);

    let decision = Decision::new(Outcome::Review, "qa_review", "confirm fixture action");
    let outcome = || JobResultOutcomeV1::Review {
        request_hash: HASH.into(),
        decision: decision.clone(),
    };
    let serve_review = async {
        let submit = accept_submission(&listener, operation, &fixture.project).await;
        assert_eq!(submit.timeout_seconds, Some(300));
        poll_result(
            &listener,
            job_result(operation, JobResultOutcomeV1::Pending),
        )
        .await;
        poll_result(&listener, job_result(operation, outcome())).await;
    };
    let (output, ()) = tokio::join!(run(command), serve_review);
    assert_returned_result(&output, &job_result(operation, outcome()), false);
    assert_no_connection(&listener).await;
    fixture.assert_legacy_untouched();
}

#[tokio::test]
async fn a_reply_cannot_change_the_operation_identity_or_result_version() {
    let fixture = Fixture::new();
    let listener = fixture.listen();
    for (result_version, result_id) in [(2, None), (1, Some(Uuid::new_v4()))] {
        let operation = Uuid::new_v4();
        let mut command = fixture.broker_run(operation);
        command.args(["--broker-wait-seconds", "8", "--", "/usr/bin/id"]);
        let mismatched_result = async {
            accept_submission(&listener, operation, &fixture.project).await;
            let (connection, command) = request(&listener).await;
            expect_result_query(command, operation);
            let mut result = job_result(
                result_id.unwrap_or(operation),
                JobResultOutcomeV1::Review {
                    request_hash: HASH.into(),
                    decision: Decision::new(Outcome::Review, "qa_review", "mismatched reply"),
                },
            );
            result.version = result_version;
            reply(connection, BrokerReplyV1::Result { result }).await;
        };
        let (output, ()) = tokio::join!(run(command), mismatched_result);
        require_error(&output, "broker");
        require_operation(&output, operation);
        assert!(output.stdout.is_empty(), "CLI rendered a mismatched result");
        assert_no_connection(&listener).await;
        fixture.assert_legacy_untouched();
    }
}

#[tokio::test]
async fn broker_result_rejects_an_uncorrelated_reply() {
    let fixture = Fixture::new();
    let listener = fixture.listen();
    let operation = Uuid::new_v4();
    let mut command = fixture.cli("broker");
    command
        .arg("result")
        .arg("--root")
        .arg(&fixture.root)
        .args(["--profile", PROFILE, "--token-file"])
        .arg(&fixture.token)
        .arg("--operation-id")
        .arg(operation.to_string());
    let serve = async {
        let (connection, command) = request(&listener).await;
        expect_result_query(command, operation);
        let result = job_result(Uuid::new_v4(), JobResultOutcomeV1::Pending);
        reply(connection, BrokerReplyV1::Result { result }).await;
    };
    let (output, ()) = tokio::join!(run(command), serve);
    require_error(&output, "uncorrelated broker result reply");
    assert!(output.stdout.is_empty());
    assert_no_connection(&listener).await;
    fixture.assert_legacy_untouched();
}

#[tokio::test]
async fn broker_registry_connection_preserves_explicit_cancel_and_authority_refusal() {
    let fixture = Fixture::new();
    let listener = fixture.listen();
    let registry = fixture.registry();
    let operation = Uuid::new_v4();
    let cancel_id = Uuid::new_v4();
    let mut cancel = fixture.configured_broker_command("cancel", &registry);
    cancel
        .arg("--operation-id")
        .arg(operation.to_string())
        .arg("--command-id")
        .arg(cancel_id.to_string());
    let denied = BrokerReplyV1::Error {
        code: BrokerErrorCodeV1::Forbidden,
    };
    let expected = serde_json::to_value(&denied).unwrap();
    let serve = async {
        let (connection, command) = request(&listener).await;
        let BrokerCommandV1::Cancel {
            operation_id,
            command_id,
        } = command
        else {
            panic!("broker cancel sent a different operation");
        };
        assert_eq!(operation_id, operation);
        assert_eq!(command_id, cancel_id);
        reply(connection, denied).await;
    };
    let (output, ()) = tokio::join!(run(cancel), serve);
    require_error(&output, "broker request was not accepted");
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        expected
    );
    assert_no_connection(&listener).await;
    fixture.assert_legacy_untouched();
}

#[tokio::test]
async fn broker_registry_connection_refuses_ambiguous_or_incomplete_flags() {
    let fixture = Fixture::new();
    let listener = fixture.listen();
    let registry = fixture.registry();
    let registry = registry.to_str().unwrap();
    let root = fixture.root.to_str().unwrap();
    let token = fixture.token.to_str().unwrap();
    let operation = Uuid::new_v4().to_string();
    for (flags, diagnostic) in [
        (&["--broker-config", registry][..], "--profile"),
        (&["--root", root, "--profile", PROFILE][..], "--token-file"),
        (&["--token-file", token, "--profile", PROFILE][..], "--root"),
        (
            &[
                "--broker-config",
                registry,
                "--root",
                root,
                "--profile",
                PROFILE,
                "--token-file",
                token,
            ][..],
            "cannot be used with",
        ),
        (
            &[
                "--broker-config",
                registry,
                "--profile",
                PROFILE,
                "--token-file",
                token,
            ][..],
            "cannot be used with",
        ),
    ] {
        let mut query = fixture.cli("broker");
        query
            .args(["result", "--operation-id", operation.as_str()])
            .args(flags);
        let output = run_without_contact(query, &listener).await;
        require_error(&output, diagnostic);
        assert!(output.stdout.is_empty());
    }
    fixture.assert_legacy_untouched();
}

#[tokio::test]
async fn synchronous_terminal_exit_uses_the_confirmed_container_result() {
    let fixture = Fixture::new();
    let listener = fixture.listen();
    for (exit_code, oom_killed, success) in [(0, false, true), (7, false, false), (0, true, false)]
    {
        let operation = Uuid::new_v4();
        let session_id = Uuid::new_v4();
        let mut command = fixture.broker_run(operation);
        command.args(["--broker-wait-seconds", "8", "--", "/usr/bin/id"]);
        let outcome = || JobResultOutcomeV1::Terminal {
            result: TerminalJobResultV1 {
                session_id,
                volume: format!("agentsandbox-{session_id}"),
                container_id: HASH.into(),
                exit_code,
                oom_killed: Some(oom_killed),
                changed_files: 0,
                manifest_digest: HASH.into(),
                output_captured: false,
                result_audit_seq: 17,
                scan_audit_seq: 23,
                cleanup_audit_seq: 26,
            },
        };
        let serve_result = async {
            accept_submission(&listener, operation, &fixture.project).await;
            poll_result(&listener, job_result(operation, outcome())).await;
        };
        let (output, ()) = tokio::join!(run(command), serve_result);
        assert_returned_result(&output, &job_result(operation, outcome()), success);
        assert_no_connection(&listener).await;
        fixture.assert_legacy_untouched();
    }
}

#[tokio::test]
async fn lost_run_client_after_acceptance_can_read_the_same_operation() {
    let fixture = Fixture::new();
    let listener = fixture.listen();
    let registry = fixture.registry();
    let directory = fixture.root.parent().unwrap();
    let selection = directory.join("selection");
    let offline_token = directory.join("runner-offline");
    fs::rename(&fixture.token, &offline_token).unwrap();
    let prepared = run_without_contact(
        fixture.prepare_broker_client(&registry, &selection),
        &listener,
    )
    .await;
    assert!(
        prepared.status.success(),
        "publish the selected profile without reading its runner file"
    );
    assert!(prepared.stdout.is_empty());
    fs::rename(offline_token, &fixture.token).unwrap();

    let operation = Uuid::new_v4();
    let mut command = fixture.cli("run");
    command
        .env(BROKER_SELECTION_ENV, "invalid-relative-selection")
        .arg("--broker-selection")
        .arg(&selection)
        .arg("--operation-id")
        .arg(operation.to_string())
        .arg("--project")
        .arg(&fixture.project)
        .args(["--", "/usr/bin/id"]);
    let mut child = command.spawn().expect("start the real agentsandbox binary");

    accept_submission(&listener, operation, &fixture.project).await;
    let (mut abandoned, command) = request(&listener).await;
    expect_result_query(command, operation);
    // The first poll establishes that the child actually received Accepted.
    // Leave that reply pending while killing and reaping only this child.
    child.start_kill().expect("kill the owned CLI child");
    let lost = collect(child).await;
    assert_eq!(lost.status.signal(), Some(libc::SIGKILL));
    require_operation(&lost, operation);
    assert!(lost.stdout.is_empty(), "no result had been delivered");
    let mut byte = [0];
    assert_eq!(
        timeout(IPC_DEADLINE, abandoned.read(&mut byte))
            .await
            .expect("lost CLI left its result connection open")
            .expect("read the abandoned result connection"),
        0,
        "the abandoned connection must reach EOF",
    );
    assert_no_connection(&listener).await;

    let mut query = fixture.cli("broker");
    query
        .env(BROKER_SELECTION_ENV, "invalid-relative-selection")
        .args(["result", "--broker-selection"])
        .arg(&selection)
        .arg("--operation-id")
        .arg(operation.to_string());
    let expected = BrokerReplyV1::Result {
        result: job_result(operation, JobResultOutcomeV1::Pending),
    };
    let json = serde_json::to_value(&expected).unwrap();
    let read_original = async {
        let (connection, command) = request(&listener).await;
        expect_result_query(command, operation);
        reply(connection, expected).await;
    };
    let (output, ()) = tokio::join!(run(query), read_original);
    assert!(
        output.status.success(),
        "broker result refused the original job"
    );
    let actual: Value = serde_json::from_slice(&output.stdout).expect("one JSON broker reply");
    assert_eq!(actual, json);
    assert_no_connection(&listener).await;
    fixture.assert_legacy_untouched();
}

#[tokio::test]
async fn broker_selection_environment_runs_and_reads_the_same_operation() {
    let fixture = Fixture::new();
    let listener = fixture.listen();
    let registry = fixture.registry();
    let selection = fixture.root.parent().unwrap().join("selection");
    let mut prepare = fixture.prepare_broker_client(&registry, &selection);
    prepare.env(BROKER_SELECTION_ENV, "invalid-relative-selection");
    let prepared = run_without_contact(prepare, &listener).await;
    assert!(
        prepared.status.success(),
        "prepare-client must use its own inputs"
    );

    let operation = Uuid::new_v4();
    let mut launch = fixture.cli("run");
    launch
        .env(BROKER_SELECTION_ENV, &selection)
        .arg("--operation-id")
        .arg(operation.to_string())
        .args(["--broker-wait-seconds", "8", "--", "/usr/bin/id"]);
    let result = || job_result(operation, JobResultOutcomeV1::Cancelled);
    let serve_launch = async {
        accept_submission(&listener, operation, &fixture.project).await;
        poll_result(&listener, result()).await;
    };
    let (launched, ()) = tokio::join!(run(launch), serve_launch);
    assert_returned_result(&launched, &result(), false);

    let mut query = fixture.cli("broker");
    query
        .env(BROKER_SELECTION_ENV, &selection)
        .args(["result", "--operation-id"])
        .arg(operation.to_string());
    let (output, ()) = tokio::join!(run(query), poll_result(&listener, result()));
    assert!(
        output.status.success(),
        "read the original result through the same selection"
    );
    let actual: Value = serde_json::from_slice(&output.stdout).expect("one JSON broker reply");
    assert_eq!(
        actual,
        serde_json::to_value(BrokerReplyV1::Result { result: result() }).unwrap()
    );
    assert_no_connection(&listener).await;
    fixture.assert_legacy_untouched();
}

#[tokio::test]
async fn broker_selection_environment_refuses_invalid_paths_and_conflicts() {
    let fixture = Fixture::new();
    let listener = fixture.listen();
    let missing = fixture.root.parent().unwrap().join("unpublished");
    let operation = Uuid::new_v4().to_string();
    let launch = || {
        let mut command = fixture.cli("run");
        command.args(["--operation-id", operation.as_str()]);
        command
    };
    let query = || {
        let mut command = fixture.cli("broker");
        command.args(["result", "--operation-id", operation.as_str()]);
        command
    };

    let mut relative = launch();
    relative
        .env(BROKER_SELECTION_ENV, "relative-selection")
        .args(["--", "/usr/bin/id"]);
    let mut absent = query();
    absent.env(BROKER_SELECTION_ENV, &missing);
    let mut empty = query();
    empty.env(BROKER_SELECTION_ENV, "");
    let mut tty = launch();
    tty.env(BROKER_SELECTION_ENV, &missing)
        .args(["--interactive", "--", "/usr/bin/id"]);
    let mut runtime_override = launch();
    runtime_override.env(BROKER_SELECTION_ENV, &missing).args([
        "--runtime",
        "docker",
        "--",
        "/usr/bin/id",
    ]);
    let mut explicit_root = query();
    explicit_root
        .env(BROKER_SELECTION_ENV, &missing)
        .arg("--root")
        .arg(&fixture.root)
        .args(["--profile", PROFILE, "--token-file"])
        .arg(&fixture.token);
    for (command, diagnostic) in [
        (relative, "broker selection paths must be absolute"),
        (
            absent,
            "published private broker selection is unavailable or unsafe",
        ),
        (empty, "--broker-selection"),
        (tty, "cannot be used with"),
        (runtime_override, "cannot be used with"),
        (explicit_root, "cannot be used with"),
    ] {
        let output = run_without_contact(command, &listener).await;
        require_error(&output, diagnostic);
        assert!(output.stdout.is_empty());
    }
    fixture.assert_legacy_untouched();
}

#[tokio::test]
async fn published_broker_selection_refuses_overwrite_mixed_options_and_registry_drift() {
    let fixture = Fixture::new();
    let listener = fixture.listen();
    let registry = fixture.registry();
    let selection = fixture.root.parent().unwrap().join("selection");
    let publish = || fixture.prepare_broker_client(&registry, &selection);
    assert!(
        run_without_contact(publish(), &listener)
            .await
            .status
            .success()
    );

    assert_eq!(fs::read_dir(&selection).unwrap().count(), 1);
    assert_eq!(
        fs::metadata(&selection).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let record = selection.join("selection.json");
    let facts = fs::metadata(&record).unwrap();
    assert_eq!(facts.permissions().mode() & 0o777, 0o600);
    let original = fs::read_to_string(&record).unwrap();
    assert!(!original.contains(TOKEN));
    assert!(!original.contains(fixture.token.to_str().unwrap()));
    let duplicate = run_without_contact(publish(), &listener).await;
    assert!(
        !duplicate.status.success(),
        "prepare-client overwrote an existing selection"
    );
    assert_eq!(fs::read_to_string(&record).unwrap(), original);
    assert_eq!(fs::metadata(&record).unwrap().ino(), facts.ino());

    let operation = Uuid::new_v4().to_string();
    let result = || {
        let mut query = fixture.cli("broker");
        query
            .args(["result", "--broker-selection"])
            .arg(&selection)
            .args(["--operation-id", operation.as_str()]);
        query
    };
    let mut mixed_result = result();
    mixed_result.args(["--profile", PROFILE]);
    let mut mixed_run = fixture.cli("run");
    mixed_run.arg("--broker-selection").arg(&selection).args([
        "--broker-profile",
        PROFILE,
        "--",
        "/usr/bin/id",
    ]);
    for command in [mixed_result, mixed_run] {
        let output = run_without_contact(command, &listener).await;
        require_error(&output, "cannot be used with");
        assert!(output.stdout.is_empty());
    }

    let original_registry = fs::read(&registry).unwrap();
    fs::rename(&registry, registry.with_extension("previous")).unwrap();
    write_new(&registry, &original_registry);
    let refused = run_without_contact(result(), &listener).await;
    require_error(
        &refused,
        "private broker registry no longer matches the published selection",
    );
    assert!(refused.stdout.is_empty());
    assert_eq!(fs::read_to_string(&record).unwrap(), original);
    fixture.assert_legacy_untouched();
}

#[tokio::test]
async fn control_c_requests_cancel_for_the_submitted_operation_and_waits_for_result() {
    let fixture = Fixture::new();
    let listener = fixture.listen();
    let operation = Uuid::new_v4();
    let mut command = fixture.broker_run(operation);
    command.args(["--broker-wait-seconds", "8", "--", "/usr/bin/id"]);
    let child = command.spawn().expect("start the real agentsandbox binary");
    let pid = libc::pid_t::try_from(child.id().expect("the run process is owned by this test"))
        .expect("the child process ID fits pid_t");
    let serve_cancel = async {
        accept_submission(&listener, operation, &fixture.project).await;
        poll_result(
            &listener,
            job_result(operation, JobResultOutcomeV1::Pending),
        )
        .await;
        // SAFETY: the child is alive and awaiting this test's IPC replies. Signal its
        // individual PID while the test owns and waits on the Child, never a group.
        assert_eq!(unsafe { libc::kill(pid, libc::SIGINT) }, 0);
        let (connection, command) = request(&listener).await;
        let BrokerCommandV1::Cancel {
            operation_id,
            command_id,
        } = command
        else {
            panic!("Control-C did not request durable broker cancellation");
        };
        assert_eq!(operation_id, operation);
        assert!(!command_id.is_nil());
        reply(
            connection,
            BrokerReplyV1::CommandAcknowledged {
                operation_id: operation,
                command_id,
                input_hash: HASH.into(),
                revision: 2,
                checkpoint_hash: HASH.into(),
            },
        )
        .await;
        let (connection, command) = request(&listener).await;
        expect_result_query(command, operation);
        reply(
            connection,
            BrokerReplyV1::Error {
                code: BrokerErrorCodeV1::Unavailable,
            },
        )
        .await;
        poll_result(
            &listener,
            job_result(operation, JobResultOutcomeV1::Cancelled),
        )
        .await;
    };
    let output = collect(child);
    tokio::pin!(output);
    let output = tokio::select! {
        biased;
        () = serve_cancel => output.await,
        early = &mut output => early,
    };
    assert_returned_result(
        &output,
        &job_result(operation, JobResultOutcomeV1::Cancelled),
        false,
    );
    assert_no_connection(&listener).await;
    fixture.assert_legacy_untouched();
}

#[tokio::test]
async fn broker_run_keeps_result_deadline_and_authority_refusal() {
    let fixture = Fixture::new();
    let listener = fixture.listen();
    for (code, diagnostic) in [
        (BrokerErrorCodeV1::Unavailable, "broker result wait expired"),
        (BrokerErrorCodeV1::Forbidden, "broker request is forbidden"),
    ] {
        let operation = Uuid::new_v4();
        let mut command = fixture.broker_run(operation);
        command.args(["--broker-wait-seconds", "1", "--", "/usr/bin/id"]);
        let serve = async {
            accept_submission(&listener, operation, &fixture.project).await;
            let (connection, command) = request(&listener).await;
            expect_result_query(command, operation);
            reply(connection, BrokerReplyV1::Error { code }).await;
        };
        let (output, ()) = tokio::join!(run(command), serve);
        require_error(&output, diagnostic);
        require_operation(&output, operation);
        assert!(output.stdout.is_empty(), "no durable result was read");
        assert_no_connection(&listener).await;
    }
    fixture.assert_legacy_untouched();
}
