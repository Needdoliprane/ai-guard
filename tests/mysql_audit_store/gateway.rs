//! The real gateway process on a prepared MySQL target: startup refusals, the
//! application path of SQL-06 over HTTP, and the control guard of the append.
//! The external fixture prepares disposable schemas, as for the Store tests.
use super::super::SECRET;
use super::*;
use agentsandbox::{
    audit::{AuditCapacityFailureV1, Store},
    broker_registry::{
        BrokerAuthorityV1, BrokerGrantMutationV1, BrokerIdentityViewV1, BrokerMutationResultV1,
        BrokerRuntimeKindV1,
    },
    identity::{
        HumanAccountV1, HumanCapabilityV1, HumanIdentityV1, IdentityChangeV1, IdentityMutationV1,
        LegacyControlRoleV1, LocalCredentialV1, LocalHumanProvisionV1, MembershipV1,
        PrincipalRefV1, generate_local_credential,
    },
    model::Event as ModelEvent,
    mysql_storage::{
        MysqlEndpoint, MysqlProbeError,
        audit::{BatchConflict, ControlGuardRefused, with_control_guard},
        probe,
    },
    policy::Policy,
    redaction::Redactor,
    workload::{WorkloadBindingIssueV1, WorkloadBindingReceiptV1, WorkloadSubjectV1},
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

async fn fresh_gateway_schema(fixture: &FixtureFiles) -> Result<Conn> {
    let mut conn = fixture.maintenance().await?;
    let session = conn
        .query_first::<(String, String), _>("SELECT DATABASE(), CURRENT_USER()")
        .await
        .map_err(|_| anyhow::anyhow!("cannot verify QA maintenance identity"))?
        .context("missing QA maintenance session")?;
    ensure!(
        session.0 == fixture.target.database
            && session.1.rsplit_once('@').map(|(user, _)| user)
                == Some(fixture.target.maintenance_username.as_str()),
        "QA maintenance session uses a different database or account"
    );
    require_fresh(&mut conn).await?;
    let tables = conn
        .query_first::<u64, _>(
            "SELECT COUNT(*) FROM information_schema.TABLES \
             WHERE BINARY TABLE_SCHEMA = BINARY DATABASE()",
        )
        .await
        .map_err(|_| anyhow::anyhow!("cannot verify the QA base schema"))?;
    ensure!(
        tables == Some(10),
        "QA requires only the prepared base schema"
    );
    Ok(conn)
}

async fn fresh_gateway_pair(a: &FixtureFiles, b: &FixtureFiles) -> Result<(Conn, Conn)> {
    ensure!(
        a.target.port == b.target.port
            && a.client_identity.is_some() == b.client_identity.is_some()
            && a.target.namespace != b.target.namespace
            && a.target.username != b.target.username
            && a.target.maintenance_username != b.target.maintenance_username,
        "QA requires two distinct schemas and account pairs on the same local MySQL profile"
    );
    let mut passwords = std::collections::BTreeSet::new();
    for fixture in [a, b] {
        for file in [&fixture.reader_password, &fixture.maintenance_password] {
            ensure!(
                passwords.insert(fixture.credentials(file)?.password),
                "QA requires four distinct SQL account passwords"
            );
        }
    }
    let conn_a = fresh_gateway_schema(a).await?;
    let conn_b = fresh_gateway_schema(b).await?;
    for (source, peer) in [(a, b), (b, a)] {
        for (account, password) in [
            (&source.target.username, &source.reader_password),
            (
                &source.target.maintenance_username,
                &source.maintenance_password,
            ),
        ] {
            probe(
                source.target.endpoint(account),
                source.credentials(password)?,
            )
            .await?;
            let cross = MysqlEndpoint {
                database: &peer.target.database,
                ..source.target.endpoint(account)
            };
            ensure!(
                matches!(
                    probe(cross, source.credentials(password)?).await,
                    Err(MysqlProbeError::ServerRefused(1044))
                ),
                "QA requires an explicit database privilege refusal for the peer schema"
            );
        }
    }
    require_read_only_witness(b).await?;
    Ok((conn_a, conn_b))
}

async fn require_read_only_witness(fixture: &FixtureFiles) -> Result<()> {
    let mut conn = maintenance_connection(
        fixture.target.endpoint(&fixture.target.username),
        fixture.credentials(&fixture.reader_password)?,
    )
    .await?;
    let result = conn
        .query_drop("EXPLAIN UPDATE ai_guard_audit_state SET head_seq=head_seq WHERE singleton=1")
        .await;
    ensure!(
        matches!(result, Err(mysql_async::Error::Server(error)) if matches!(error.code, 1142 | 1143)),
        "the QA witness service account must refuse audit updates"
    );
    Ok(())
}

/// Audit, identity and workload genesis on a fresh target, as the operator's
/// preparation would leave it before the gateway is pointed at it.
async fn prepare_gateway_target(
    conn: &mut Conn,
    fixture: &FixtureFiles,
    tenant: &str,
    credential_digest: &[u8; 32],
) -> Result<AuditHead> {
    let initial = fixture.initialize(tenant).await?;
    let (_, identity_head) =
        prepare_identity(conn, fixture.target.namespace, &initial, credential_digest).await?;
    super::super::workload::prepare_capability(
        conn,
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
async fn assert_startup_refused(
    registry: &Registry,
    secrets: &[&str],
    label: &str,
    expected_reason: Option<&str>,
) -> Result<()> {
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
    if let Some(reason) = expected_reason {
        ensure!(
            text.contains(reason),
            "{label}: expected startup refusal missing"
        );
    }
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

async fn post_batch(
    http: &Client,
    gateway: &Gateway,
    token: &str,
    events: &[&ModelEvent],
) -> Result<(StatusCode, Value)> {
    let response = http
        .post(gateway.url("/api/v1/events/batch"))
        .bearer_auth(token)
        .json(&json!({"events": events}))
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("gateway batch request failed"))?;
    let status = response.status();
    let body = response
        .json()
        .await
        .map_err(|_| anyhow::anyhow!("invalid gateway batch response"))?;
    Ok((status, body))
}

async fn batch_snapshot(conn: &mut Conn) -> Result<AuditSnapshot> {
    audit_snapshot(conn)
        .await
        .map_err(|_| anyhow::anyhow!("batch QA audit snapshot unavailable"))
}

#[tokio::test]
#[ignore = "requires a fresh prepared private MySQL 8.4 QA schema and a built gateway"]
async fn gateway_refuses_wrong_mysql_material_and_writes_no_business_row() -> Result<()> {
    let fixture = FixtureFiles::open_gateway("AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_A")?;
    let mut conn = fresh_gateway_schema(&fixture).await?;
    let prepared = prepare_gateway_target(&mut conn, &fixture, "tenant-a", &DIGEST_A).await?;
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
        assert_startup_refused(&registry, &[wrong_secret], label, None).await?;
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
#[ignore = "requires a fresh prepared private MySQL 8.4 QA schema and a built gateway"]
async fn gateway_requires_identity_and_workload_projections_before_listening() -> Result<()> {
    let fixture = FixtureFiles::open_gateway("AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_A")?;
    let mut conn = fresh_gateway_schema(&fixture).await?;
    let registry = Registry::new()?;
    registry.write(&[registry.entry(&fixture, "tenant-a", "a")?])?;
    let secrets = [RUNNER_A, VIEWER_A];

    let initial = fixture.initialize("tenant-a").await?;
    let audit_only = audit_snapshot(&mut conn).await?;
    assert_startup_refused(
        &registry,
        &secrets,
        "missing identity projection",
        Some(&IdentityFailureV1::MigrationRequired.to_string()),
    )
    .await?;
    ensure!(audit_snapshot(&mut conn).await? == audit_only);

    let (_, identity) =
        prepare_identity(&mut conn, fixture.target.namespace, &initial, &DIGEST_A).await?;
    let without_workload = audit_snapshot(&mut conn).await?;
    assert_startup_refused(
        &registry,
        &secrets,
        "missing workload projection",
        Some(&WorkloadFailureV1::MigrationRequired.to_string()),
    )
    .await?;
    ensure!(audit_snapshot(&mut conn).await? == without_workload);

    super::super::workload::prepare_capability(
        &mut conn,
        "tenant-a",
        fixture.target.namespace,
        (identity.seq, &identity.hash),
    )
    .await?;
    let prepared = audit_snapshot(&mut conn).await?;
    Gateway::start(&registry).await?.stop().await?;
    ensure!(audit_snapshot(&mut conn).await? == prepared);
    Ok(())
}

#[tokio::test]
#[ignore = "requires two fresh prepared private MySQL 8.4 QA schemas and a built gateway"]
async fn gateway_appends_to_the_prepared_target_and_replays_after_restart() -> Result<()> {
    let a = FixtureFiles::open_gateway("AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_A")?;
    let b = FixtureFiles::open_gateway("AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_B")?;
    let (mut conn_a, mut conn_b) = fresh_gateway_pair(&a, &b).await?;
    prepare_gateway_target(&mut conn_a, &a, "tenant-a", &DIGEST_A).await?;
    prepare_gateway_target(&mut conn_b, &b, "tenant-b", &DIGEST_B).await?;
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
async fn gateway_commits_ordinary_batch_and_replays_after_restart() -> Result<()> {
    let a = FixtureFiles::open_gateway("AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_A")?;
    let b = FixtureFiles::open_gateway("AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_B")?;
    let (mut conn_a, mut conn_b) = fresh_gateway_pair(&a, &b)
        .await
        .map_err(|_| anyhow::anyhow!("batch QA pair is not verified"))?;
    prepare_gateway_target(&mut conn_a, &a, "tenant-a", &DIGEST_A)
        .await
        .map_err(|_| anyhow::anyhow!("batch QA writer preparation failed"))?;
    prepare_gateway_target(&mut conn_b, &b, "tenant-b", &DIGEST_B)
        .await
        .map_err(|_| anyhow::anyhow!("batch QA witness preparation failed"))?;
    let prefix_a = batch_snapshot(&mut conn_a).await?;
    let witness_b = batch_snapshot(&mut conn_b).await?;

    let registry = Registry::new()?;
    registry.write(&[
        registry.entry(&a, "tenant-a", "a")?,
        registry.entry(&b, "tenant-b", "b")?,
    ])?;
    let http = Client::builder().no_proxy().timeout(DEADLINE).build()?;
    let first = ModelEvent::new("activity.channel");
    let second = ModelEvent::new("activity.coverage");
    let events = [&first, &second, &first];
    let seq = prefix_a.0.3;
    let expected = json!({"stored": [
        {"id": first.id, "seq": seq + 1},
        {"id": second.id, "seq": seq + 2},
        {"id": first.id, "seq": seq + 1}
    ]});

    let gateway = Gateway::start(&registry)
        .await
        .map_err(|_| anyhow::anyhow!("batch QA initial gateway start was not confirmed"))?;
    let viewer = post_batch(&http, &gateway, VIEWER_A, &events).await?;
    ensure!(
        viewer.0 == StatusCode::FORBIDDEN,
        "batch viewer was not refused"
    );
    ensure!(
        batch_snapshot(&mut conn_a).await? == prefix_a
            && batch_snapshot(&mut conn_b).await? == witness_b,
        "batch viewer changed an audit"
    );

    let committed = post_batch(&http, &gateway, RUNNER_A, &events).await?;
    ensure!(
        committed.0 == StatusCode::OK && committed.1 == expected,
        "batch acknowledgement did not match request order and original duplicate receipt"
    );
    let committed_a = batch_snapshot(&mut conn_a).await?;
    ensure!(
        committed_a.1.starts_with(&prefix_a.1),
        "batch changed the prepared audit prefix"
    );
    let [one, two] = &committed_a.1[prefix_a.1.len()..] else {
        anyhow::bail!("batch did not persist exactly two events")
    };
    ensure!(
        one.0 == seq + 1
            && one.1.as_slice() == first.id.as_bytes()
            && one.3.as_slice() == prefix_a.0.4.as_bytes()
            && two.0 == seq + 2
            && two.1.as_slice() == second.id.as_bytes()
            && two.3 == one.4
            && committed_a.0.3 == two.0
            && committed_a.0.4.as_bytes() == two.4.as_slice()
            && batch_snapshot(&mut conn_b).await? == witness_b,
        "batch did not durably extend only the intended audit in receipt order"
    );

    let next = ModelEvent::new("activity.channel");
    let mut divergent = first.clone();
    divergent.data = json!({"marker": "divergent synthetic event content"});
    let conflict = post_batch(&http, &gateway, RUNNER_A, &[&next, &divergent]).await?;
    ensure!(
        conflict.0 == StatusCode::CONFLICT
            && conflict.1
                == json!({"error": "Un identifiant du lot existe déjà avec un contenu différent."}),
        "divergent batch did not return the exact event-conflict refusal"
    );
    ensure!(
        batch_snapshot(&mut conn_a).await? == committed_a
            && batch_snapshot(&mut conn_b).await? == witness_b,
        "batch conflict changed an audit or retained the preceding event"
    );
    gateway.stop().await?;

    let restarted = Gateway::start(&registry)
        .await
        .map_err(|_| anyhow::anyhow!("batch QA gateway restart was not confirmed"))?;
    let replay = post_batch(&http, &restarted, RUNNER_A, &events).await?;
    ensure!(
        replay == committed,
        "batch replay returned a different acknowledgement"
    );
    ensure!(
        batch_snapshot(&mut conn_a).await? == committed_a
            && batch_snapshot(&mut conn_b).await? == witness_b,
        "batch replay changed an audit"
    );
    restarted.stop().await
}

async fn with_unverifiable_identity_projection(
    conn: &mut Conn,
    observe: impl std::future::Future<Output = Result<()>>,
) -> Result<()> {
    let anchor = conn
        .query_first::<Vec<u8>, _>(
            "SELECT genesis_hash FROM ai_guard_identity_meta WHERE singleton=1",
        )
        .await
        .map_err(|_| anyhow::anyhow!("cannot read QA identity anchor"))?
        .context("prepared QA identity anchor is missing")?;
    let mut divergent = anchor.clone();
    let first = divergent
        .first_mut()
        .context("QA identity anchor is empty")?;
    *first = if *first == b'0' { b'1' } else { b'0' };
    let update = "UPDATE ai_guard_identity_meta SET genesis_hash=? WHERE singleton=1";
    conn.exec_drop(update, (divergent.as_slice(),))
        .await
        .map_err(|_| anyhow::anyhow!("cannot introduce QA identity projection fault"))?;
    let observation = observe.await;
    conn.exec_drop(update, (anchor.as_slice(),))
        .await
        .map_err(|_| anyhow::anyhow!("cannot restore QA identity projection"))?;
    observation
}

#[tokio::test]
#[ignore = "requires two fresh prepared private MySQL 8.4 QA schemas and a built gateway"]
async fn gateway_reports_unverified_selected_mysql_audit_and_recovers() -> Result<()> {
    let a = FixtureFiles::open_gateway("AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_A")?;
    let b = FixtureFiles::open_gateway("AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_B")?;
    let (mut conn_a, mut conn_b) = fresh_gateway_pair(&a, &b).await?;
    prepare_gateway_target(&mut conn_a, &a, "tenant-a", &DIGEST_A).await?;
    prepare_gateway_target(&mut conn_b, &b, "tenant-b", &DIGEST_B).await?;
    let before_a = audit_snapshot(&mut conn_a).await?;
    let before_b = audit_snapshot(&mut conn_b).await?;

    let registry = Registry::new()?;
    registry.write(&[
        registry.entry(&a, "tenant-a", "a")?,
        registry.entry(&b, "tenant-b", "b")?,
    ])?;
    let http = Client::builder().no_proxy().timeout(DEADLINE).build()?;
    let gateway = Gateway::start(&registry).await?;
    let health =
        async || Ok::<_, anyhow::Error>(http.get(gateway.url("/healthz")).send().await?.status());
    let integrity = async |token: &str| {
        Ok::<_, anyhow::Error>(
            http.get(gateway.url("/api/v1/integrity"))
                .bearer_auth(token)
                .send()
                .await?
                .status(),
        )
    };
    ensure!(
        health().await? == StatusCode::OK,
        "prepared gateway is unhealthy"
    );
    with_unverifiable_identity_projection(&mut conn_b, async {
        ensure!(
            integrity(VIEWER_A).await? == StatusCode::OK,
            "A lost verified read access"
        );
        ensure!(
            health().await? == StatusCode::OK,
            "peer discovery changed selected audit health"
        );
        ensure!(
            integrity(VIEWER_B).await? == StatusCode::SERVICE_UNAVAILABLE,
            "B read was served without its required projection"
        );
        ensure!(
            health().await? == StatusCode::SERVICE_UNAVAILABLE,
            "gateway served healthy after the selected audit proof failed"
        );
        Ok(())
    })
    .await?;

    ensure!(
        integrity(VIEWER_A).await? == StatusCode::OK,
        "A lost verified read access after repair"
    );
    ensure!(
        health().await? == StatusCode::SERVICE_UNAVAILABLE,
        "peer verification cleared the selected audit failure"
    );
    ensure!(
        integrity(VIEWER_B).await? == StatusCode::OK,
        "B did not recover with its verified projection"
    );
    ensure!(
        health().await? == StatusCode::OK,
        "gateway stayed degraded after fresh verification of B"
    );
    ensure!(
        audit_snapshot(&mut conn_a).await? == before_a,
        "A audit changed"
    );
    ensure!(
        audit_snapshot(&mut conn_b).await? == before_b,
        "B audit changed"
    );
    gateway.stop().await
}

#[tokio::test]
#[ignore = "requires two fresh prepared private MySQL 8.4 QA schemas and a built gateway"]
async fn gateway_retains_verified_neighbor_collisions_during_projection_failure() -> Result<()> {
    let a = FixtureFiles::open_gateway("AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_A")?;
    let b = FixtureFiles::open_gateway("AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_B")?;
    let (mut conn_a, mut conn_b) = fresh_gateway_pair(&a, &b).await?;
    let runner_a_digest: [u8; 32] = Sha256::digest(RUNNER_A.as_bytes()).into();
    prepare_gateway_target(&mut conn_a, &a, "tenant-a", &DIGEST_A).await?;
    let head_b = prepare_gateway_target(&mut conn_b, &b, "tenant-b", &runner_a_digest).await?;

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
    let during_failure = with_unverifiable_identity_projection(&mut conn_b, async {
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
    })
    .await;
    gateway.stop().await?;
    during_failure?;
    ensure!(b_reader.verify().await? == head_b);
    ensure!(audit_snapshot(&mut conn_a).await? == before_a, "A changed");
    ensure!(audit_snapshot(&mut conn_b).await? == before_b, "B changed");
    Ok(())
}

fn provision_sqlite_reader(store: &Store) -> Result<(PrincipalRefV1, String)> {
    let proof = generate_local_credential()?;
    let principal = PrincipalRefV1::HumanLocal {
        installation_id: Uuid::new_v4().to_string(),
        account_id: Uuid::new_v4().to_string(),
    };
    let mutation = |change| IdentityMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        actor: PrincipalRefV1::LegacyControl {
            credential_id: "mixed-registry-fixture".into(),
            role: LegacyControlRoleV1::Runner,
        },
        change,
    };
    store.provision_local_human(&LocalHumanProvisionV1 {
        version: 1,
        mutations: [
            mutation(IdentityChangeV1::Account {
                expected_revision: 0,
                account: HumanAccountV1 {
                    version: 1,
                    principal: principal.clone(),
                    display_name: "SQLite reader".into(),
                    revision: 1,
                    active: true,
                },
            }),
            mutation(IdentityChangeV1::Credential {
                expected_revision: 0,
                binding: LocalCredentialV1 {
                    version: 1,
                    credential_id: Uuid::new_v4().to_string(),
                    principal: principal.clone(),
                    sha256_digest: hex::encode(Sha256::digest(proof.as_bytes())),
                    revision: 1,
                    active: true,
                    expires_at: None,
                },
            }),
            mutation(IdentityChangeV1::Membership {
                expected_revision: 0,
                membership: MembershipV1 {
                    version: 1,
                    membership_id: Uuid::new_v4().to_string(),
                    principal: principal.clone(),
                    tenant_id: store.tenant_id().into(),
                    organization_scope: "engineering".into(),
                    revision: 1,
                    active: true,
                    capabilities: [HumanCapabilityV1::ReadMetadata].into_iter().collect(),
                },
            }),
        ],
    })?;
    Ok((principal, proof))
}

#[tokio::test]
#[ignore = "requires a fresh prepared private MySQL 8.4 QA schema and a built gateway"]
async fn gateway_issues_sqlite_workload_credential_when_mysql_neighbor_verifies() -> Result<()> {
    let b = FixtureFiles::open_gateway("AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_B")?;
    let mut conn_b = fresh_gateway_schema(&b).await?;
    require_read_only_witness(&b).await?;
    prepare_gateway_target(&mut conn_b, &b, "tenant-b", &DIGEST_B).await?;
    let before_b = audit_snapshot(&mut conn_b).await?;

    let registry = Registry::new()?;
    let data = registry.root.join("tenant-a.sqlite");
    let a = Store::create_supervised_for_tenant(
        &data,
        "tenant-a",
        Redactor::new(Vec::new()),
        128,
        &registry.root.join("tenant-a-before-v3.sqlite"),
    )?;
    a.migrate_identity_v4(&registry.root.join("tenant-a-before-v4.sqlite"))?;
    a.migrate_human_access_v5(&registry.root.join("tenant-a-before-v5.sqlite"))?;
    a.migrate_workload_v6(&registry.root.join("tenant-a-before-v6.sqlite"))?;
    let (human_principal, human_proof) = provision_sqlite_reader(&a)?;
    let before_a = a.integrity()?;
    drop(a);

    registry.write(&[
        json!({
            "tenant_id": "tenant-a",
            "data": data,
            "runner_token_file": registry.root.join("a-runner"),
            "viewer_token_file": registry.root.join("a-viewer"),
        }),
        registry.entry(&b, "tenant-b", "b")?,
    ])?;
    let http = Client::builder().no_proxy().timeout(DEADLINE).build()?;
    let gateway = Gateway::start(&registry).await?;
    let identity = http
        .get(gateway.url("/api/v1/human/identity"))
        .bearer_auth(&human_proof)
        .send()
        .await?;
    ensure!(identity.status() == StatusCode::OK);
    let identity = identity.json::<HumanIdentityV1>().await?;
    ensure!(
        identity.principal == human_principal
            && identity.authentication.method == "local_credential"
            && !identity.contexts_complete
            && identity.contexts.len() == 1
            && identity.contexts[0].tenant_id == "tenant-a"
            && identity.contexts[0].capabilities == [HumanCapabilityV1::ReadMetadata],
        "the local proof must offer only the verified SQLite context with partial coverage"
    );
    let broker_id = Uuid::new_v4().to_string();
    let incarnation_id = Uuid::new_v4().to_string();
    let workload_proof = Uuid::new_v4().simple().to_string();
    let issue = WorkloadBindingIssueV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        credential_id: Uuid::new_v4().to_string(),
        expected_revision: 0,
        subject: WorkloadSubjectV1::Broker {
            broker_id: broker_id.clone(),
            incarnation_id: incarnation_id.clone(),
        },
        sha256_digest: hex::encode(Sha256::digest(workload_proof.as_bytes())),
        expires_at: (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339(),
    };
    let post = |input: &WorkloadBindingIssueV1| {
        http.post(gateway.url("/api/v1/workload-bindings"))
            .bearer_auth(RUNNER_A)
            .json(input)
            .send()
    };
    let issued = post(&issue).await?;
    ensure!(issued.status() == StatusCode::CREATED);
    let receipt: WorkloadBindingReceiptV1 = issued.json().await?;
    ensure!(receipt.credential_id == issue.credential_id && receipt.revision == 1);

    let alias = WorkloadBindingIssueV1 {
        operation_id: Uuid::new_v4().to_string(),
        credential_id: Uuid::new_v4().to_string(),
        sha256_digest: hex::encode(DIGEST_B),
        ..issue.clone()
    };
    ensure!(post(&alias).await?.status() == StatusCode::CONFLICT);
    let grant = http
        .post(gateway.url("/api/v1/brokers/grants"))
        .bearer_auth(RUNNER_A)
        .json(&BrokerGrantMutationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            broker_id: broker_id.clone(),
            expected_revision: None,
            credential_id: issue.credential_id.clone(),
            incarnation_id: incarnation_id.clone(),
            authorities: vec![BrokerAuthorityV1 {
                authority_id: "mixed-registry-fixture".into(),
                runtime_kind: BrokerRuntimeKindV1::Docker,
            }],
            lease_seconds: 120,
            not_after: issue.expires_at.clone(),
        })
        .send()
        .await?;
    ensure!(grant.status() == StatusCode::CREATED);
    let grant = grant.json::<BrokerMutationResultV1>().await?;
    ensure!(grant.receipt.audit_seq == receipt.audit_seq + 1);

    let identity = http
        .get(gateway.url("/api/v1/brokers/identity"))
        .bearer_auth(&workload_proof)
        .send()
        .await?;
    ensure!(identity.status() == StatusCode::OK);
    let identity = identity.json::<BrokerIdentityViewV1>().await?;
    ensure!(
        identity.tenant_id == "tenant-a"
            && identity.broker_id == broker_id
            && identity.incarnation_id == incarnation_id
            && identity.credential_id == issue.credential_id
            && identity.binding_revision == receipt.revision
            && identity.grant_revision == grant.receipt.grant_revision,
        "the checked SQLite workload proof must select its own broker"
    );
    gateway.stop().await?;

    let a = Store::open_existing_for_tenant(&data, "tenant-a", Redactor::new(Vec::new()), 128)?;
    let after_a = a.integrity()?;
    ensure!(after_a.valid && after_a.event_count == before_a.event_count + 3);
    ensure!(after_a.head_hash == grant.receipt.audit_hash);
    ensure!(audit_snapshot(&mut conn_b).await? == before_b, "B changed");
    Ok(())
}

#[tokio::test]
#[ignore = "requires a fresh prepared private MySQL 8.4 QA schema and a writer account"]
async fn ordinary_batch_is_atomic_and_replays_in_request_order() -> Result<()> {
    let fixture = FixtureFiles::open_gateway("AI_GUARD_MYSQL_ACTIVITY_QA_CONFIG")?;
    let mut conn = fresh_gateway_schema(&fixture).await?;
    let genesis = fixture.initialize("tenant-a").await?;
    let open = |max_events| {
        fixture.target.open(
            &fixture.target.username,
            fixture.credentials(&fixture.reader_password)?,
            "tenant-a",
            fixture.target.namespace,
            max_events,
        )
    };
    let store = open(128)?;
    let mut first = ModelEvent::new("activity.channel");
    first.data = json!({"message": SECRET});
    let second = ModelEvent::new("activity.coverage");
    let input = vec![first.clone(), second.clone(), first.clone()];
    let receipts = store.append_batch(input.clone()).await?;
    let [one, two, repeated] = receipts.as_slice() else {
        anyhow::bail!("the batch must acknowledge all three positions")
    };
    ensure!(
        one.seq == genesis.seq + 1
            && one.prev_hash == genesis.hash
            && one.event.id == first.id
            && one.event.data["message"] == "[REDACTED]"
            && two.seq == one.seq + 1
            && two.prev_hash == one.hash
            && two.event.id == second.id
            && serde_json::to_value(repeated)? == serde_json::to_value(one)?,
        "the batch lost masking, ordering, chaining or duplicate replay"
    );
    let committed = store.verify().await?;
    ensure!(committed.seq == two.seq && committed.hash == two.hash);
    let snapshot = audit_snapshot(&mut conn).await?;
    ensure!(
        store.append_batch(Vec::new()).await.is_err()
            && store.append_batch(vec![first.clone(); 65]).await.is_err()
            && audit_snapshot(&mut conn).await? == snapshot,
        "out-of-bound batches must be refused"
    );

    let next = ModelEvent::new("activity.channel");
    let mut divergent = first.clone();
    divergent.data = json!({"message": "different synthetic content"});
    let conflict = store
        .append_batch(vec![next.clone(), divergent])
        .await
        .err()
        .context("a divergent existing id must refuse the batch")?;
    ensure!(
        conflict.is::<BatchConflict>(),
        "expected a typed batch conflict"
    );
    ensure!(
        store.verify().await? == committed && audit_snapshot(&mut conn).await? == snapshot,
        "conflict changed the confirmed head or persisted a partial batch"
    );

    let limited = open(5)?;
    ensure!(limited.verify().await? == committed);
    let too_many = vec![next.clone(), ModelEvent::new("activity.channel")];
    let capacity = limited
        .append_batch(too_many)
        .await
        .err()
        .context("a batch exceeding the ordinary reserve must be refused")?;
    ensure!(
        capacity.downcast_ref::<AuditCapacityFailureV1>()
            == Some(&AuditCapacityFailureV1::OrdinaryWriteBudgetExhausted),
        "expected the cumulative ordinary reserve refusal"
    );
    ensure!(
        limited.verify().await? == committed && audit_snapshot(&mut conn).await? == snapshot,
        "capacity refusal changed the confirmed head or persisted a partial batch"
    );

    let reopened = open(128)?;
    let replayed = reopened.append_batch(input).await?;
    ensure!(
        serde_json::to_vec(&replayed)? == serde_json::to_vec(&receipts)?
            && reopened.verify().await? == committed
            && audit_snapshot(&mut conn).await? == snapshot,
        "reopening changed the original receipts or audit state"
    );
    let following = limited.append_batch(vec![next.clone()]).await?;
    ensure!(
        following.len() == 1
            && following[0].seq == committed.seq + 1
            && following[0].prev_hash == committed.hash
            && following[0].event.id == next.id,
        "a refused batch spent capacity or retained a partial receipt"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires a fresh prepared private MySQL 8.4 QA schema"]
async fn the_control_guard_refuses_inside_the_append_transaction() -> Result<()> {
    let fixture = FixtureFiles::open_gateway("AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_A")?;
    let mut conn = fresh_gateway_schema(&fixture).await?;
    prepare_gateway_target(&mut conn, &fixture, "tenant-a", &DIGEST_A).await?;
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

#[tokio::test]
#[ignore = "requires two fresh prepared private MySQL 8.4 QA schemas"]
async fn owned_origin_probe_refuses_a_held_schema_and_preserves_both_audits() -> Result<()> {
    let a = FixtureFiles::open_gateway("AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_A")?;
    let b = FixtureFiles::open_gateway("AI_GUARD_MYSQL_IDENTITY_QA_CONFIG_B")?;
    let (mut conn_a, mut conn_b) = fresh_gateway_pair(&a, &b).await?;
    let head_a = prepare_gateway_target(&mut conn_a, &a, "tenant-a", &DIGEST_A).await?;
    let head_b = prepare_gateway_target(&mut conn_b, &b, "tenant-b", &DIGEST_B).await?;
    let before_a = audit_snapshot(&mut conn_a).await?;
    let before_b = audit_snapshot(&mut conn_b).await?;
    let server: String = conn_a
        .query_first("SELECT @@GLOBAL.server_uuid")
        .await?
        .context("missing server UUID in the owned-origin fixture")?;
    let server = Uuid::parse_str(&server)?;
    let store_a = a.reader("tenant-a", a.target.namespace)?;
    let store_b = b.reader("tenant-b", b.target.namespace)?;

    // Hold exactly the lock of A's physical database through another verified
    // session. B has its own lock and must remain independently readable.
    let mut scope = b"ai-guard:owned-mysql-source:v1\0".to_vec();
    scope.extend_from_slice(server.as_bytes());
    scope.extend_from_slice(a.target.database.as_bytes());
    let lock = hex::encode(Sha256::digest(scope));
    let acquired: Option<i64> = conn_a.exec_first("SELECT GET_LOCK(?, 0)", (&lock,)).await?;
    ensure!(acquired == Some(1), "fixture could not take the lock for A");
    let contested = store_a
        .verify_owned_origin(server, head_a.seq, &head_a.hash)
        .await;
    let independent = store_b
        .verify_owned_origin(server, head_b.seq, &head_b.hash)
        .await;
    let released: Option<i64> = conn_a
        .exec_first("SELECT RELEASE_LOCK(?)", (&lock,))
        .await?;
    ensure!(
        released == Some(1),
        "fixture could not confirm lock release"
    );
    ensure!(
        contested
            .expect_err("a second owner must refuse")
            .to_string()
            == "MySQL owned audit target is already held"
    );
    ensure!(independent? == head_b);
    ensure!(
        store_a
            .verify_owned_origin(Uuid::new_v4(), head_a.seq, &head_a.hash)
            .await
            .is_err()
    );
    ensure!(
        store_a
            .verify_owned_origin(server, head_a.seq + 1, &head_a.hash)
            .await
            .is_err()
    );
    let wrong_namespace = a.reader("tenant-a", b.target.namespace)?;
    ensure!(
        wrong_namespace
            .verify_owned_origin(server, head_a.seq, &head_a.hash)
            .await
            .is_err()
    );
    ensure!(
        store_a
            .verify_owned_origin(server, head_a.seq, &head_a.hash)
            .await?
            == head_a
    );
    let remaining_owner = conn_a
        .exec_first::<Option<u64>, _, _>("SELECT IS_USED_LOCK(?)", (&lock,))
        .await?;
    ensure!(
        remaining_owner == Some(None),
        "preflight did not release A's lock"
    );
    ensure!(audit_snapshot(&mut conn_a).await? == before_a);
    ensure!(audit_snapshot(&mut conn_b).await? == before_b);
    Ok(())
}
