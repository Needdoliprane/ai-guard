//! Cross-binary acceptance: the actual CLI talks to the actual owned gateway.
//!
//! The ignored case needs a same-build, Cargo-bound normal executable supplied
//! by the isolated QA. It must be selected explicitly; an ignored result is not
//! evidence. Neither environment variable is read by production code.
use crate::{
    api::{GatewayState, gateway_router},
    audit::Store,
    egress::{Authority, EgressPolicyV1, EgressTransport, PolicyLimits},
    policy::Policy,
    redaction::Redactor,
};
use anyhow::{Context, Result, ensure};
use axum::{
    body::{Body, to_bytes},
    extract::Request,
    middleware::{self, Next},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{OpenOptions, Permissions},
    io::{Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command, sync::oneshot, task::JoinHandle};

const RUNNER: &str = "synthetic-A-runner";
const REPLACE: &str = "/control/api/v1/egress/policies/replace";
const REVOKE: &str = "/control/api/v1/egress/policies/revoke";
const OUTPUT_CAP: usize = 8_192;

struct Server {
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<std::io::Result<()>>>,
}

impl Server {
    async fn close(&mut self) -> Result<()> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(mut task) = self.task.take() {
            match tokio::time::timeout(Duration::from_secs(30), &mut task).await {
                Ok(joined) => joined.context("CLI acceptance server task failed")??,
                Err(_) => {
                    task.abort();
                    if tokio::time::timeout(Duration::from_secs(30), &mut task)
                        .await
                        .is_err()
                    {
                        // Keep the unfinished owner reachable by Drop; never
                        // turn a cleanup timeout into successful acceptance.
                        self.task = Some(task);
                    }
                    anyhow::bail!("CLI acceptance server drain watchdog elapsed");
                }
            }
        }
        Ok(())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

pub(super) struct ActualCli {
    executable: PathBuf,
    home: PathBuf,
}

pub(super) struct CliOutput {
    pub(super) status: ExitStatus,
    pub(super) stdout: Vec<u8>,
    pub(super) stderr: Vec<u8>,
}

impl ActualCli {
    pub(super) fn from_qa(home: &Path) -> Result<Self> {
        let executable = PathBuf::from(
            std::env::var_os("AI_GUARD_A1_CLI_TEST_BIN")
                .context("select this acceptance case only with the Cargo-bound normal CLI")?,
        );
        let expected = std::env::var("AI_GUARD_A1_CLI_TEST_SHA256")
            .context("same-build normal CLI digest is required")?;
        ensure!(
            expected.len() == 64
                && expected
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "invalid QA artifact digest"
        );
        ensure!(executable.is_absolute(), "QA executable must be absolute");
        ensure!(
            executable.canonicalize()? == executable,
            "QA executable path must be canonical"
        );
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(&executable)?;
        let before = file.metadata()?;
        ensure!(
            before.is_file() && before.len() > 0 && before.len() <= 1_073_741_824,
            "QA executable must be a bounded regular artifact"
        );
        let mut digest = Sha256::new();
        let mut buffer = [0_u8; 65_536];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
        ensure!(
            hex::encode(digest.finalize()) == expected,
            "normal CLI artifact digest mismatch"
        );
        ensure!(
            file.metadata()?.len() == before.len(),
            "normal CLI changed during hashing"
        );
        Ok(Self {
            executable,
            home: home.to_owned(),
        })
    }

    pub(super) async fn run(&self, args: &[&str]) -> Result<CliOutput> {
        let mut child = Command::new(&self.executable)
            .args(args)
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("HOME", &self.home)
            .env("TMPDIR", &self.home)
            .env("RUST_LOG", "off")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("spawn actual CLI in isolated acceptance QA")?;
        let stdout = child.stdout.take().context("CLI stdout unavailable")?;
        let stderr = child.stderr.take().context("CLI stderr unavailable")?;
        let captured = tokio::time::timeout(Duration::from_secs(90), async {
            let (stdout, stderr, status) =
                tokio::try_join!(bounded_output(stdout), bounded_output(stderr), async {
                    Ok::<_, anyhow::Error>(child.wait().await?)
                },)?;
            Ok::<_, anyhow::Error>(CliOutput {
                status,
                stdout,
                stderr,
            })
        })
        .await;
        match captured {
            Ok(Ok(output)) => Ok(output),
            error => {
                // Reap the actual child before any test outcome is accepted.
                let _ = child.kill().await;
                let _ = child.wait().await;
                match error {
                    Ok(Err(error)) => Err(error),
                    _ => anyhow::bail!("actual CLI acceptance watchdog elapsed"),
                }
            }
        }
    }
}

async fn bounded_output(reader: impl tokio::io::AsyncRead + Unpin) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take((OUTPUT_CAP + 1) as u64)
        .read_to_end(&mut bytes)
        .await?;
    ensure!(
        bytes.len() <= OUTPUT_CAP,
        "CLI acceptance capture exceeded its bound"
    );
    Ok(bytes)
}

pub(super) fn private_input(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

pub(super) fn text(path: &Path) -> Result<&str> {
    path.to_str()
        .context("synthetic acceptance pathname is not UTF-8")
}

fn output_json(
    output: &CliOutput,
    origin: &str,
    operation: uuid::Uuid,
    action: &str,
) -> Result<Value> {
    ensure!(
        output.stdout.ends_with(b"\n")
            && output.stdout.iter().filter(|byte| **byte == b'\n').count() == 1
            && !output.stdout.contains(&b'\r'),
        "CLI must emit exactly one complete JSON line"
    );
    let value: Value = serde_json::from_slice(&output.stdout)?;
    let object = value.as_object().context("CLI output must be an object")?;
    let expected: BTreeSet<&str> = [
        "version",
        "operation_id",
        "action",
        "phase",
        "attempts",
        "last_error",
        "receipt_present",
        "pending_state",
        "receipt",
        "historical",
        "coverage",
    ]
    .into_iter()
    .collect();
    ensure!(
        object.keys().map(String::as_str).collect::<BTreeSet<_>>() == expected,
        "unexpected CLI output field"
    );
    ensure!(
        value["version"] == 1
            && value["operation_id"] == operation.to_string()
            && value["action"] == action
            && value["historical"] == true
            && value["coverage"] == "policy_registry_only",
        "CLI output correlation mismatch"
    );
    for forbidden in [
        RUNNER,
        origin,
        "example.com",
        "control_namespace_uuid",
        "runtime_applied",
        "replace-operation",
        "revoke-operation",
        "runner-token",
        "replace-input.json",
        "revoke-input.json",
    ] {
        ensure!(
            !String::from_utf8_lossy(&output.stdout).contains(forbidden)
                && !String::from_utf8_lossy(&output.stderr).contains(forbidden),
            "CLI output exposed a private or unsupported field"
        );
    }
    Ok(value)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an explicitly selected same-build Cargo-bound normal CLI in isolated QA"]
async fn actual_cli_preserves_operation_after_lost_receipt_then_revokes_without_runtime_claim()
-> Result<()> {
    let files = tempfile::Builder::new()
        .permissions(Permissions::from_mode(0o700))
        .tempdir_in(std::fs::canonicalize(std::env::temp_dir())?)?;
    let cli = ActualCli::from_qa(files.path())?;
    let mut fixture = Store::owned_control_fixture_for_test(15);
    let (bundle, credentials, tokens) = fixture.take_startup();
    let state = GatewayState::for_owned_human_sources(
        bundle,
        credentials.clone(),
        tokens,
        Policy::default(),
        None,
        Redactor::new(vec![]),
    )?;
    let owned = state
        .owned_human
        .as_ref()
        .context("missing genuine owned gateway")?;
    let index = state
        .tenants
        .iter()
        .position(|tenant| tenant.tenant_id == "A")
        .context("missing synthetic tenantA")?;
    let credential = crate::api::authenticate_review_runner_fixture(
        credentials,
        owned.owner.clone(),
        state
            .tenants
            .iter()
            .map(|tenant| (tenant.tenant_id.clone(), Arc::clone(&tenant.store)))
            .collect(),
        RUNNER,
    )?;
    let source = &owned.sources[index];
    let (namespace, _) = {
        let epoch = owned.owner.try_read_epoch()?;
        state.tenants[index]
            .store
            .assert_owned_control_fixture_binding_for_test(&epoch, source, &credential, 15)
    };
    let database = source.descriptor().storage_binding.path();
    drop(credential);
    let retirement = state
        .human_retirement()
        .context("missing owned retirement")?;
    let requests = Arc::new(AtomicUsize::new(0));
    let posts = Arc::new(AtomicUsize::new(0));
    let drop_first_receipt = Arc::new(AtomicBool::new(true));
    let first_receipt = Arc::new(Mutex::new(None::<Vec<u8>>));
    let app = gateway_router(state.clone()).layer(middleware::from_fn({
        let requests = requests.clone();
        let posts = posts.clone();
        let drop_first_receipt = drop_first_receipt.clone();
        let first_receipt = first_receipt.clone();
        move |request: Request, next: Next| {
            let requests = requests.clone();
            let posts = posts.clone();
            let drop_first_receipt = drop_first_receipt.clone();
            let first_receipt = first_receipt.clone();
            async move {
                requests.fetch_add(1, Ordering::AcqRel);
                let replace =
                    request.method() == axum::http::Method::POST && request.uri().path() == REPLACE;
                let policy_post = replace
                    || (request.method() == axum::http::Method::POST
                        && request.uri().path() == REVOKE);
                if policy_post {
                    posts.fetch_add(1, Ordering::AcqRel);
                }
                let response = next.run(request).await;
                if replace
                    && response.status() == axum::http::StatusCode::OK
                    && drop_first_receipt.swap(false, Ordering::AcqRel)
                {
                    // Fault only the delivery AFTER the real gateway produced
                    // its committed receipt. Never replace the backend/verdict.
                    let (parts, body) = response.into_parts();
                    let bytes = to_bytes(body, 4_096).await.expect("actual receipt bound");
                    *first_receipt.lock().expect("acceptance receipt mutex") = Some(bytes.to_vec());
                    let broken = futures_util::stream::once(async {
                        Err::<axum::body::Bytes, std::io::Error>(std::io::Error::new(
                            std::io::ErrorKind::ConnectionReset,
                            "synthetic post-commit delivery loss",
                        ))
                    });
                    return axum::response::Response::from_parts(parts, Body::from_stream(broken));
                }
                response
            }
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let origin = format!("http://{}", listener.local_addr()?);
    let (stop, stopped) = oneshot::channel();
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
    });
    let mut server = Server {
        stop: Some(stop),
        task: Some(task),
    };
    let operation = uuid::Uuid::new_v4();
    let revoke_operation = uuid::Uuid::new_v4();
    let at = u64::try_from(chrono::Utc::now().timestamp_millis())?;
    let policy = EgressPolicyV1::new(
        1,
        at.saturating_sub(60_000),
        at.checked_add(600_000)
            .context("synthetic policy clock overflow")?,
        &[Authority::new(
            "example.com",
            443,
            EgressTransport::ConnectTcpOpaque,
        )?],
        PolicyLimits::new(1, 1)?,
    )?;
    let prepared = files.path().join("replace-operation");
    let revoke_prepared = files.path().join("revoke-operation");
    let input = files.path().join("replace-input.json");
    let revoke_input = files.path().join("revoke-input.json");
    let token = files.path().join("runner-token");
    private_input(&token, RUNNER.as_bytes())?;
    private_input(
        &input,
        &serde_json::to_vec(&json!({"version":1,"action":"replace","request":{
            "version":1,"expected_scope":{"tenant_id":"A","control_namespace_uuid":namespace},
            "operation_id":operation,"selector":{"kind":"tenant"},"expected_revision":0,"expected_epoch":0,
            "policy":{"not_before_ms":policy.not_before_ms(),"expires_at_ms":policy.expires_at_ms(),
                "destinations":[{"name":"example.com","port":443,"transport":"connect_tcp_opaque"}],
                "max_destinations":1,"max_resolution_ips":1},"tenant_ceiling":null
        }}))?,
    )?;
    private_input(
        &revoke_input,
        &serde_json::to_vec(&json!({"version":1,"action":"revoke","request":{
            "version":1,"expected_scope":{"tenant_id":"A","control_namespace_uuid":namespace},
            "operation_id":revoke_operation,"selector":{"kind":"tenant"},"expected_revision":1,"expected_epoch":1
        }}))?,
    )?;
    let outcome: Result<(Vec<u8>, Vec<u8>)> = async {
        let output = cli
            .run(&[
                "broker",
                "policy",
                "prepare",
                "--gateway",
                &origin,
                "--input",
                text(&input)?,
                "--output-dir",
                text(&prepared)?,
            ])
            .await?;
        ensure!(output.status.success(), "actual prepare failed");
        let value = output_json(&output, &origin, operation, "replace")?;
        ensure!(
            value["phase"] == "prepared" && value["attempts"] == 0,
            "invalid prepared evidence"
        );
        ensure!(
            requests.load(Ordering::Acquire) == 0,
            "prepare performed network IO"
        );
        let request_before = std::fs::read(prepared.join("request.json"))?;
        let metadata_before = std::fs::read(prepared.join("metadata.json"))?;

        let output = cli
            .run(&[
                "broker",
                "policy",
                "apply",
                "--operation-dir",
                text(&prepared)?,
                "--token-file",
                text(&token)?,
            ])
            .await?;
        ensure!(
            !output.status.success(),
            "lost receipt must not be reported as success"
        );
        let value = output_json(&output, &origin, operation, "replace")?;
        ensure!(
            value["phase"] == "remote_outcome_unresolved"
                && value["attempts"] == 1
                && value["receipt_present"] == false,
            "delivery loss must preserve unresolved evidence"
        );
        ensure!(
            [
                "egress_policy_transport_unresolved",
                "egress_policy_invalid_response"
            ]
            .contains(&value["last_error"].as_str().unwrap_or("")),
            "injected response loss must reach the SDK transport/response error path"
        );
        ensure!(
            first_receipt
                .lock()
                .expect("acceptance receipt mutex")
                .is_some()
                && posts.load(Ordering::Acquire) == 1,
            "fault did not follow one actual committed response"
        );

        let output = cli
            .run(&[
                "broker",
                "policy",
                "resume",
                "--operation-dir",
                text(&prepared)?,
                "--token-file",
                text(&token)?,
            ])
            .await?;
        ensure!(
            output.status.success(),
            "explicit same-operation recovery failed"
        );
        let value = output_json(&output, &origin, operation, "replace")?;
        ensure!(
            value["phase"] == "receipt_recorded"
                && value["attempts"] == 2
                && value["receipt_present"] == true,
            "same-operation recovery lacks receipt"
        );
        ensure!(
            posts.load(Ordering::Acquire) == 2,
            "resume made more than one POST"
        );
        let replace_receipt = std::fs::read(prepared.join("receipt.json"))?;
        ensure!(
            first_receipt
                .lock()
                .expect("acceptance receipt mutex")
                .as_deref()
                == Some(replace_receipt.as_slice()),
            "replay changed the actual first receipt"
        );
        ensure!(
            std::fs::read(prepared.join("request.json"))? == request_before
                && std::fs::read(prepared.join("metadata.json"))? == metadata_before,
            "recovery changed immutable operation or scope"
        );

        let before_status = requests.load(Ordering::Acquire);
        let output = cli
            .run(&[
                "broker",
                "policy",
                "status",
                "--operation-dir",
                text(&prepared)?,
            ])
            .await?;
        ensure!(output.status.success(), "local status failed");
        let value = output_json(&output, &origin, operation, "replace")?;
        ensure!(
            value["phase"] == "receipt_recorded" && value["attempts"] == 2,
            "status lost evidence"
        );
        ensure!(
            requests.load(Ordering::Acquire) == before_status,
            "status performed network IO"
        );
        let output = cli
            .run(&[
                "broker",
                "policy",
                "resume",
                "--operation-dir",
                text(&prepared)?,
                "--token-file",
                text(&token)?,
            ])
            .await?;
        ensure!(
            output.status.success() && posts.load(Ordering::Acquire) == 2,
            "retained valid receipt must prevent another POST"
        );
        output_json(&output, &origin, operation, "replace")?;

        let output = cli
            .run(&[
                "broker",
                "policy",
                "prepare",
                "--gateway",
                &origin,
                "--input",
                text(&revoke_input)?,
                "--output-dir",
                text(&revoke_prepared)?,
            ])
            .await?;
        ensure!(output.status.success(), "revoke prepare failed");
        output_json(&output, &origin, revoke_operation, "revoke")?;
        let output = cli
            .run(&[
                "broker",
                "policy",
                "apply",
                "--operation-dir",
                text(&revoke_prepared)?,
                "--token-file",
                text(&token)?,
            ])
            .await?;
        ensure!(
            output.status.success() && posts.load(Ordering::Acquire) == 3,
            "revoke POST failed"
        );
        let value = output_json(&output, &origin, revoke_operation, "revoke")?;
        ensure!(
            value["phase"] == "receipt_recorded"
                && value["attempts"] == 1
                && value["receipt"]["state"] == "revoke_requested"
                && value["receipt"]["revision"] == 1
                && value["receipt"]["after_epoch"] == 2,
            "revoke must remain a policy registry request, not a runtime result"
        );
        Ok((
            replace_receipt,
            std::fs::read(revoke_prepared.join("receipt.json"))?,
        ))
    }
    .await;
    let server_close = server.close().await;
    retirement.retire();
    let drained = tokio::time::timeout(Duration::from_secs(30), retirement.drain()).await;
    server_close?;
    drained.context("owned CLI acceptance drain watchdog")??;
    let receipts = outcome?;
    // Only open independent read-only SQL oracles after real gateway work drains.
    let db = rusqlite::Connection::open_with_flags(
        database,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let count: i64 = db.query_row("SELECT COUNT(*) FROM egress_policy_operations", [], |row| {
        row.get(0)
    })?;
    ensure!(
        count == 2,
        "lost receipt replay created an additional operation"
    );
    for (id, bytes, expected_action, expected_epoch) in [
        (operation, receipts.0, "replace", 1),
        (revoke_operation, receipts.1, "revoke", 2),
    ] {
        let (canonical, sequence, hash): (String, i64, String) = db.query_row(
            "SELECT o.receipt_json,e.seq,e.hash FROM egress_policy_operations o JOIN events e ON e.seq=o.event_seq WHERE o.operation_id=?1",
            [id.to_string()], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        ensure!(
            canonical.as_bytes() == bytes,
            "CLI receipt differs from actual durable SQLite receipt"
        );
        let receipt: Value = serde_json::from_slice(&bytes)?;
        ensure!(
            receipt["operation_id"] == id.to_string()
                && receipt["action"] == expected_action
                && receipt["event_seq"] == sequence
                && receipt["event_hash"] == hash
                && receipt["after_epoch"] == expected_epoch,
            "durable receipt correlation mismatch"
        );
        // This independent fixture knows the original policy. It does not add
        // a digest-value expectation to the revoke SDK or CLI request.
        ensure!(
            receipt["policy_digest"] == hex::encode(policy.digest()),
            "policy receipt digest changed"
        );
    }
    println!("A1_ACTUAL_CLI_LOST_RECEIPT_REPLAY_REVOKE_SQLITE_CORRELATED");
    Ok(())
}
