//! The real gateway process on a prepared MySQL target: startup refusals, the
//! application path of SQL-06 over HTTP, and the control guard of the append.
//! The external fixture prepares disposable schemas, as for the Store tests.
use super::*;
use agentsandbox::{
    model::Event as ModelEvent,
    mysql_storage::audit::{ControlGuardRefused, with_control_guard},
    policy::Policy,
};
use reqwest::{Client, StatusCode};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, ChildStderr},
    time::timeout,
};

const DEADLINE: Duration = Duration::from_secs(45);
const RUNNER_A: &str = "a-runner-a-runner-a-runner-a-runner-a-runner-a-runner-a-runner-a-runner-";
const VIEWER_A: &str = "a-viewer-a-viewer-a-viewer-a-viewer-a-viewer-a-viewer-a-viewer-a-viewer-";
const RUNNER_B: &str = "b-runner-b-runner-b-runner-b-runner-b-runner-b-runner-b-runner-b-runner-";
const VIEWER_B: &str = "b-viewer-b-viewer-b-viewer-b-viewer-b-viewer-b-viewer-b-viewer-b-viewer-";

fn private_write(path: &Path, bytes: &[u8]) -> Result<()> {
    std::fs::write(path, bytes)?;
    std::fs::set_permissions(path, Permissions::from_mode(0o600))?;
    Ok(())
}

fn optional_material(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).map(PathBuf::from)
}

/// Audit, identity and workload genesis on a fresh target, as the operator's
/// preparation would leave it before the gateway is pointed at it.
async fn prepare_gateway_target(
    fixture: &FixtureFiles,
    tenant: &str,
    credential_digest: &[u8; 32],
) -> Result<AuditHead> {
    let mut conn = fixture.maintenance().await?;
    require_fresh(&mut conn).await?;
    let initial = fixture.initialize(tenant).await?;
    let (_, identity_head) = prepare_identity(
        &mut conn,
        fixture.target.namespace,
        &initial,
        credential_digest,
    )
    .await?;
    super::super::workload::prepare_capability(
        &mut conn,
        tenant,
        fixture.target.namespace,
        (identity_head.seq, &identity_head.hash),
    )
    .await
}

struct Registry {
    root: PathBuf,
    _scratch: tempfile::TempDir,
}

impl Registry {
    fn new() -> Result<Self> {
        let scratch = tempfile::Builder::new()
            .prefix("mysql-gateway-")
            .permissions(Permissions::from_mode(0o700))
            .tempdir()?;
        let root = scratch.path().canonicalize()?;
        for (name, token) in [
            ("a-runner", RUNNER_A),
            ("a-viewer", VIEWER_A),
            ("b-runner", RUNNER_B),
            ("b-viewer", VIEWER_B),
        ] {
            private_write(&root.join(name), token.as_bytes())?;
        }
        private_write(
            &root.join("policy.json"),
            &serde_json::to_vec(&Policy::default())?,
        )?;
        Ok(Self {
            root,
            _scratch: scratch,
        })
    }

    fn entry(&self, fixture: &FixtureFiles, tenant: &str, prefix: &str) -> Result<Value> {
        let mut entry = registry_entry(fixture, tenant)?;
        entry["runner_token_file"] = json!(self.root.join(format!("{prefix}-runner")));
        entry["viewer_token_file"] = json!(self.root.join(format!("{prefix}-viewer")));
        Ok(entry)
    }

    fn write(&self, entries: &[Value]) -> Result<()> {
        private_write(
            &self.root.join("tenants.json"),
            &serde_json::to_vec(&json!({"version": 1, "tenants": entries}))?,
        )
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
        command
            .env_clear()
            .env("TMPDIR", &self.root)
            .current_dir(&self.root)
            .args(["gateway", "--listen", "127.0.0.1:0", "--tenants-file"])
            .arg(self.root.join("tenants.json"))
            .arg("--policy")
            .arg(self.root.join("policy.json"))
            .args(["--max-events", "128"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        command
    }
}

struct Gateway {
    child: Child,
    _stderr: BufReader<ChildStderr>,
    address: std::net::SocketAddr,
}

impl Gateway {
    async fn start(registry: &Registry) -> Result<Self> {
        let mut child = registry.command().spawn()?;
        let mut stderr = BufReader::new(child.stderr.take().context("stderr missing")?);
        let mut line = String::new();
        timeout(DEADLINE, stderr.read_line(&mut line))
            .await
            .context("gateway startup timed out")??;
        let address = line
            .strip_prefix("Gateway ")
            .and_then(|rest| rest.split_whitespace().next())
            .with_context(|| format!("gateway did not report readiness: {}", line.trim()))?
            .parse()?;
        Ok(Self {
            child,
            _stderr: stderr,
            address,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.address)
    }

    async fn stop(mut self) -> Result<()> {
        let pid = i32::try_from(self.child.id().context("no PID")?)?;
        // SAFETY: this handle owns the child.
        ensure!(
            unsafe { libc::kill(pid, libc::SIGTERM) } == 0,
            "kill failed"
        );
        let status = timeout(DEADLINE, self.child.wait()).await??;
        ensure!(status.success(), "gateway did not exit cleanly: {status}");
        Ok(())
    }
}

/// The gateway must exit non-zero without ever reporting readiness, and its
/// diagnostics must not echo any secret.
async fn assert_startup_refused(registry: &Registry, secrets: &[&str], label: &str) -> Result<()> {
    let mut child = registry.command().spawn()?;
    let mut stderr = child.stderr.take().context("stderr missing")?;
    let mut text = Vec::new();
    let status = timeout(DEADLINE, async {
        use tokio::io::AsyncReadExt;
        let mut bounded = stderr.take(16_384);
        let read = bounded.read_to_end(&mut text);
        let (status, _) = tokio::join!(child.wait(), read);
        status
    })
    .await
    .with_context(|| format!("{label}: the gateway neither exited nor served"))??;
    let text = String::from_utf8_lossy(&text);
    ensure!(!status.success(), "{label}: the gateway started");
    ensure!(!text.contains("Gateway "), "{label}: readiness reported");
    for secret in secrets {
        ensure!(!text.contains(secret), "{label}: a secret reached stderr");
    }
    Ok(())
}

async fn post(
    http: &Client,
    gateway: &Gateway,
    token: &str,
    event: &ModelEvent,
) -> Result<reqwest::Response> {
    Ok(http
        .post(gateway.url("/api/v1/events"))
        .bearer_auth(token)
        .json(event)
        .send()
        .await?)
}

#[tokio::test]
#[ignore = "requires a fresh prepared private MySQL 8.4 QA schema and a built gateway"]
async fn gateway_refuses_wrong_mysql_material_and_writes_no_business_row() -> Result<()> {
    let fixture = FixtureFiles::open("AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_A")?;
    let prepared = prepare_gateway_target(&fixture, "tenant-a", &DIGEST_A).await?;
    let mut conn = fixture.maintenance().await?;
    let before = audit_snapshot(&mut conn).await?;
    let registry = Registry::new()?;
    let good = registry.entry(&fixture, "tenant-a", "a")?;
    let wrong_secret = "wrong-password-fixture";

    let mut cases: Vec<(&str, Value)> = Vec::new();
    let mut wrong_password = good.clone();
    private_write(
        &registry.root.join("wrong.password"),
        wrong_secret.as_bytes(),
    )?;
    wrong_password["storage"]["password_file"] = json!(registry.root.join("wrong.password"));
    cases.push(("wrong password", wrong_password));
    let mut wrong_name = good.clone();
    wrong_name["storage"]["host"] = json!("127.0.0.1");
    cases.push(("wrong server name", wrong_name));
    if let Some(other_ca) = optional_material("AI_GUARD_MYSQL_GATEWAY_OTHER_CA") {
        let mut entry = good.clone();
        entry["storage"]["ca_file"] = json!(other_ca);
        cases.push(("wrong CA", entry));
    }
    if fixture.client_identity.is_some() {
        let mut no_certificate = good.clone();
        no_certificate["storage"]["authentication"] = json!({"mode": "password"});
        cases.push(("missing client certificate", no_certificate));
        for (label, cert, key) in [
            (
                "expired client certificate",
                "AI_GUARD_MYSQL_GATEWAY_EXPIRED_CERT",
                "AI_GUARD_MYSQL_GATEWAY_EXPIRED_KEY",
            ),
            (
                "foreign client certificate",
                "AI_GUARD_MYSQL_GATEWAY_FOREIGN_CERT",
                "AI_GUARD_MYSQL_GATEWAY_FOREIGN_KEY",
            ),
        ] {
            if let (Some(cert), Some(key)) = (optional_material(cert), optional_material(key)) {
                let mut entry = good.clone();
                entry["storage"]["authentication"] = json!({
                    "mode": "password_and_mtls",
                    "client_cert_file": cert,
                    "client_key_file": key,
                });
                cases.push((label, entry));
            }
        }
    }
    for (label, entry) in &cases {
        registry.write(std::slice::from_ref(entry))?;
        assert_startup_refused(&registry, &[wrong_secret], label).await?;
        ensure!(
            audit_snapshot(&mut conn).await? == before,
            "{label}: the target changed"
        );
    }

    // The positive witness brackets the refusals: the same target serves.
    registry.write(&[good])?;
    let gateway = Gateway::start(&registry).await?;
    let http = Client::builder().no_proxy().build()?;
    let integrity: Value = http
        .get(gateway.url("/api/v1/integrity"))
        .bearer_auth(VIEWER_A)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    ensure!(integrity["valid"] == true && integrity["event_count"] == prepared.seq);
    gateway.stop().await?;
    ensure!(
        audit_snapshot(&mut conn).await? == before,
        "serving without a request changed the target"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires two fresh prepared private MySQL 8.4 QA schemas and a built gateway"]
async fn gateway_appends_to_the_prepared_target_and_replays_after_restart() -> Result<()> {
    let a = FixtureFiles::open("AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_A")?;
    let b = FixtureFiles::open("AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_B")?;
    ensure!(
        a.target.namespace != b.target.namespace,
        "targets must differ"
    );
    prepare_gateway_target(&a, "tenant-a", &DIGEST_A).await?;
    prepare_gateway_target(&b, "tenant-b", &DIGEST_B).await?;
    let mut conn_a = a.maintenance().await?;
    let mut conn_b = b.maintenance().await?;
    let prefix_a = audit_snapshot(&mut conn_a).await?;
    let witness_b = audit_snapshot(&mut conn_b).await?;

    let registry = Registry::new()?;
    registry.write(&[
        registry.entry(&a, "tenant-a", "a")?,
        registry.entry(&b, "tenant-b", "b")?,
    ])?;
    let http = Client::builder().no_proxy().build()?;
    let mut event = ModelEvent::new("qa.sql06.append");
    event.data = json!({"marker": "append-only-to-a"});

    let gateway = Gateway::start(&registry).await?;
    ensure!(post(&http, &gateway, VIEWER_A, &event).await?.status() == StatusCode::FORBIDDEN);
    ensure!(
        audit_snapshot(&mut conn_a).await? == prefix_a,
        "viewer wrote"
    );
    let first = post(&http, &gateway, RUNNER_A, &event).await?;
    ensure!(first.status() == StatusCode::OK);
    let receipt: Value = first.json().await?;
    ensure!(
        receipt["seq"].as_i64() == Some(prefix_a.0.3 + 1),
        "the append did not extend A's head"
    );
    ensure!(audit_snapshot(&mut conn_b).await? == witness_b, "B changed");
    gateway.stop().await?;

    let restarted = Gateway::start(&registry).await?;
    let replay = post(&http, &restarted, RUNNER_A, &event).await?;
    ensure!(replay.status() == StatusCode::OK);
    ensure!(
        replay.json::<Value>().await? == receipt,
        "replay returned a different receipt"
    );
    let after = audit_snapshot(&mut conn_a).await?;
    ensure!(
        after.1.len() == prefix_a.1.len() + 1,
        "replay appended a row"
    );
    ensure!(
        after.1.starts_with(&prefix_a.1),
        "A's stored prefix changed"
    );
    ensure!(audit_snapshot(&mut conn_b).await? == witness_b, "B changed");
    restarted.stop().await
}

#[tokio::test]
#[ignore = "requires two fresh prepared private MySQL 8.4 QA schemas and a built gateway"]
async fn gateway_retains_verified_neighbor_collisions_during_projection_failure() -> Result<()> {
    let a = FixtureFiles::open("AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_A")?;
    let b = FixtureFiles::open("AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_B")?;
    ensure!(
        a.target.namespace != b.target.namespace,
        "targets must differ"
    );
    let runner_a_digest: [u8; 32] = Sha256::digest(RUNNER_A.as_bytes()).into();
    prepare_gateway_target(&a, "tenant-a", &DIGEST_A).await?;
    let head_b = prepare_gateway_target(&b, "tenant-b", &runner_a_digest).await?;

    let mut conn_a = a.maintenance().await?;
    let mut conn_b = b.maintenance().await?;
    let before_a = audit_snapshot(&mut conn_a).await?;
    let before_b = audit_snapshot(&mut conn_b).await?;
    let b_reader = b.reader("tenant-b", b.target.namespace)?;
    ensure!(b_reader.verify().await? == head_b);
    let registry = Registry::new()?;
    registry.write(&[
        registry.entry(&a, "tenant-a", "a")?,
        registry.entry(&b, "tenant-b", "b")?,
    ])?;
    let http = Client::builder().no_proxy().timeout(DEADLINE).build()?;
    let gateway = Gateway::start(&registry).await?;

    // After the startup verification, make only B's disposable projection
    // temporarily unverifiable. No authenticated request has primed its cache.
    let anchor = conn_b
        .query_first::<Vec<u8>, _>(
            "SELECT genesis_hash FROM ai_guard_identity_meta WHERE singleton=1",
        )
        .await?
        .context("B's prepared identity anchor is missing")?;
    let mut divergent = anchor.clone();
    let first = divergent
        .first_mut()
        .context("B's identity anchor is empty")?;
    *first = if *first == b'0' { b'1' } else { b'0' };
    let update = "UPDATE ai_guard_identity_meta SET genesis_hash=? WHERE singleton=1";
    conn_b.exec_drop(update, (divergent.as_slice(),)).await?;
    let during_failure = async {
        ensure!(b_reader.verify().await.is_err(), "B remained verifiable");
        let url = gateway.url("/api/v1/integrity");
        let unrelated = http.get(url.as_str()).bearer_auth(VIEWER_A).send().await?;
        ensure!(
            unrelated.status() == StatusCode::OK,
            "A lost healthy access"
        );
        let colliding = http.get(url).bearer_auth(RUNNER_A).send().await?;
        ensure!(
            colliding.status() == StatusCode::SERVICE_UNAVAILABLE,
            "B's verified collision must still block the ambiguous credential"
        );
        ensure!(colliding.json::<Value>().await?["error"]["code"] == "identity_unavailable");
        Ok::<_, anyhow::Error>(())
    }
    .await;
    conn_b.exec_drop(update, (anchor.as_slice(),)).await?;
    gateway.stop().await?;
    during_failure?;
    ensure!(b_reader.verify().await? == head_b);
    ensure!(audit_snapshot(&mut conn_a).await? == before_a, "A changed");
    ensure!(audit_snapshot(&mut conn_b).await? == before_b, "B changed");
    Ok(())
}

#[tokio::test]
#[ignore = "requires a fresh prepared private MySQL 8.4 QA schema"]
async fn the_control_guard_refuses_inside_the_append_transaction() -> Result<()> {
    let fixture = FixtureFiles::open("AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_A")?;
    prepare_gateway_target(&fixture, "tenant-a", &DIGEST_A).await?;
    let mut conn = fixture.maintenance().await?;
    let before = audit_snapshot(&mut conn).await?;
    let store = fixture.reader("tenant-a", fixture.target.namespace)?;

    // The credential is bound to a human in the verified prefix: refuse and
    // roll back with no event and no head change.
    let refused = with_control_guard(DIGEST_A, store.append(ModelEvent::new("qa.guard.refused")))
        .await
        .expect_err("a colliding technical credential must not append");
    ensure!(refused.downcast_ref::<ControlGuardRefused>().is_some());
    ensure!(
        audit_snapshot(&mut conn).await? == before,
        "the refusal wrote"
    );

    let unrelated: [u8; 32] = Sha256::digest(RUNNER_A.as_bytes()).into();
    let stored = with_control_guard(
        unrelated,
        store.append(ModelEvent::new("qa.guard.accepted")),
    )
    .await?;
    ensure!(stored.seq == before.0.3 + 1);
    Ok(())
}
