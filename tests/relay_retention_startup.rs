//! Operator configuration of relay retention through the real gateway binary:
//! option or registry field, validation, tenant Store, startup diagnostic and
//! diagnostic route. Owned CLI children on loopback and private files; no
//! worker, no provider. This qualifies no worker transport or runtime.
mod support;

use agentsandbox::{
    audit::Store,
    redaction::Redactor,
    relay::{RelayCapacityV1, RelayRetentionPolicyV1},
};
use serde_json::{Value, json};
use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    process::{Child, ChildStderr, Command},
};

const EVENTS: u64 = 20_000;

fn private_write(path: &Path, bytes: impl AsRef<[u8]>) {
    std::fs::write(path, bytes).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

/// Two distinct private credential files of one tenant.
fn credentials(root: &Path, tenant: &str, runner: char, viewer: char) -> (PathBuf, PathBuf) {
    let runner_file = root.join(format!("{tenant}-runner"));
    let viewer_file = root.join(format!("{tenant}-viewer"));
    private_write(&runner_file, runner.to_string().repeat(48));
    private_write(&viewer_file, viewer.to_string().repeat(48));
    (runner_file, viewer_file)
}

/// The relay requires schema 6. This is the store an operator obtains from the
/// explicit migrations; reopened by the gateway, its supervision is inactive.
fn relay_ready_store(root: &Path, tenant: &str) -> PathBuf {
    let path = root.join(format!("{tenant}.sqlite"));
    let backup = |version: u32| root.join(format!("{tenant}-before-v{version}.sqlite"));
    let store = Store::create_supervised_for_tenant(
        &path,
        tenant,
        Redactor::new(vec![]),
        EVENTS,
        &backup(3),
    )
    .unwrap();
    store.migrate_identity_v4(&backup(4)).unwrap();
    store.migrate_human_access_v5(&backup(5)).unwrap();
    store.migrate_workload_v6(&backup(6)).unwrap();
    path
}

fn tightened() -> RelayRetentionPolicyV1 {
    let policy = RelayRetentionPolicyV1 {
        sender: RelayCapacityV1 {
            messages: 10,
            bytes: 1_048_576,
        },
        grant: RelayCapacityV1 {
            messages: 5,
            bytes: 524_288,
        },
        automatic_prune_batch: 4,
        ..RelayRetentionPolicyV1::default()
    };
    policy.validate().unwrap();
    policy
}

/// Empty child environment: no inherited provider or decision-engine setting.
fn gateway(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
    command
        .env_clear()
        .env("TMPDIR", root)
        .current_dir(root)
        .args(["gateway", "--listen", "127.0.0.1:0", "--max-events"])
        .arg(EVENTS.to_string())
        .arg("--policy")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("policies/default.json"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

struct Running {
    child: Child,
    _stderr: BufReader<ChildStderr>,
    base: String,
    /// Lines printed after readiness, up to the last tenant's diagnostics.
    diagnostics: Vec<String>,
}

/// Start the gateway and read its readiness line and startup diagnostics.
/// The workload audit line is the last one printed for each tenant.
async fn start(mut command: Command, tenants: usize) -> Running {
    let mut child = command.spawn().unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut line = String::new();
    let read = tokio::time::timeout(Duration::from_secs(30), stderr.read_line(&mut line))
        .await
        .expect("gateway startup timed out")
        .unwrap();
    assert!(read > 0, "gateway exited before readiness");
    let address: std::net::SocketAddr = line
        .strip_prefix("Gateway ")
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("gateway did not start: {line}"))
        .parse()
        .unwrap();
    assert!(address.ip().is_loopback());
    let mut diagnostics: Vec<String> = Vec::new();
    while diagnostics
        .iter()
        .filter(|line| line.starts_with("Workload audit capacity "))
        .count()
        < tenants
    {
        let mut line = String::new();
        let read = tokio::time::timeout(Duration::from_secs(15), stderr.read_line(&mut line))
            .await
            .unwrap_or_else(|_| panic!("startup diagnostics incomplete: {diagnostics:#?}"))
            .unwrap();
        assert!(
            read > 0,
            "gateway exited during its startup diagnostics: {diagnostics:#?}"
        );
        diagnostics.push(line.trim_end().to_owned());
    }
    Running {
        child,
        _stderr: stderr,
        base: format!("http://{address}"),
        diagnostics,
    }
}

/// The startup line published by the retention guides, for one tenant.
fn relay_line(tenant: &str, source: &str, policy: &RelayRetentionPolicyV1) -> String {
    format!(
        "Relay retention tenant={tenant} source={source} release_on_ack={} policy={} fingerprint={}",
        if policy.release_on_ack {
            "enabled_history_unreadable_by_earlier_binaries"
        } else {
            "disabled"
        },
        serde_json::to_string(policy).unwrap(),
        policy.fingerprint().unwrap()
    )
}

fn assert_reported(running: &Running, tenant: &str, source: &str, policy: &RelayRetentionPolicyV1) {
    let expected = relay_line(tenant, source, policy);
    assert!(
        running.diagnostics.contains(&expected),
        "startup did not report the relay retention policy applied to {tenant}; expected {expected}; diagnostics: {:#?}",
        running.diagnostics
    );
}

/// The policy the served Store applies, read by the tenant's own credential.
async fn served_policy(running: &Running, token: &str) -> (Value, Value) {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .no_proxy()
        .build()
        .unwrap();
    let response = http
        .get(format!("{}/api/v1/relay/retention", running.base))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    assert_eq!(
        status,
        reqwest::StatusCode::OK,
        "the operator cannot read relay retention: {body}"
    );
    (body["policy"].clone(), body["policy_fingerprint"].clone())
}

async fn exit_of(mut command: Command) -> (std::process::ExitStatus, String) {
    let mut child = command.spawn().unwrap();
    let stderr = child.stderr.take().unwrap();
    let drain = async move {
        let mut bytes = Vec::new();
        stderr
            .take(64 * 1024)
            .read_to_end(&mut bytes)
            .await
            .unwrap();
        bytes
    };
    // A child that unexpectedly starts serving is killed by this wall guard:
    // that is a failed expectation of the test, reported as such.
    match tokio::time::timeout(Duration::from_secs(30), async {
        tokio::join!(child.wait(), drain)
    })
    .await
    {
        Ok((status, stderr)) => (
            status.unwrap(),
            String::from_utf8_lossy(&stderr).into_owned(),
        ),
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            panic!("the gateway kept running although its configuration had to be refused");
        }
    }
}

#[tokio::test]
async fn local_policy_file_reaches_the_store_the_startup_line_and_the_diagnostic_route() {
    let files = support::private_tempdir();
    let root = files.path();
    let data = relay_ready_store(root, "local");
    let (runner, viewer) = credentials(root, "local", 'r', 'v');
    let policy = RelayRetentionPolicyV1 {
        release_on_ack: true,
        ..tightened()
    };
    let policy_file = root.join("relay-retention.json");
    private_write(&policy_file, serde_json::to_vec(&policy).unwrap());
    let local = |root: &Path| {
        let mut command = gateway(root);
        command
            .arg("--data")
            .arg(&data)
            .arg("--runner-token-file")
            .arg(&runner)
            .arg("--viewer-token-file")
            .arg(&viewer);
        command
    };

    let mut command = local(root);
    command.arg("--relay-retention-file").arg(&policy_file);
    let mut running = start(command, 1).await;
    assert_reported(&running, "local", "private_policy_file", &policy);
    for token in ['r', 'v'] {
        assert_eq!(
            served_policy(&running, &token.to_string().repeat(48)).await,
            (json!(policy), json!(policy.fingerprint().unwrap())),
            "the served store does not apply the policy of --relay-retention-file"
        );
    }
    running.child.kill().await.unwrap();
    drop(running);

    // The policy is a startup input, not durable state: without the option
    // the same store is served under the shipped policy, and says so.
    let shipped = RelayRetentionPolicyV1::default();
    let mut running = start(local(root), 1).await;
    assert_reported(&running, "local", "derived_defaults", &shipped);
    assert_eq!(
        served_policy(&running, &"r".repeat(48)).await,
        (json!(shipped), json!(shipped.fingerprint().unwrap()))
    );
    running.child.kill().await.unwrap();
}

#[tokio::test]
async fn registry_policy_is_applied_to_its_own_tenant_only() {
    let files = support::private_tempdir();
    let root = files.path();
    let policy = tightened();
    let mut tenants = Vec::new();
    for (tenant, runner, viewer) in [("tenant-a", 'r', 'v'), ("tenant-b", 's', 'w')] {
        let (runner_file, viewer_file) = credentials(root, tenant, runner, viewer);
        tenants.push(json!({
            "tenant_id":tenant,
            "data":relay_ready_store(root, tenant),
            "runner_token_file":runner_file,
            "viewer_token_file":viewer_file,
        }));
    }
    tenants[0]["relay_retention_policy"] = json!(policy);
    let registry = root.join("tenants.json");
    private_write(
        &registry,
        json!({"version":1,"tenants":tenants}).to_string(),
    );
    let mut command = gateway(root);
    command.arg("--tenants-file").arg(&registry);
    let mut running = start(command, 2).await;

    let shipped = RelayRetentionPolicyV1::default();
    assert_reported(&running, "tenant-a", "tenant_registry", &policy);
    assert_reported(&running, "tenant-b", "derived_defaults", &shipped);
    assert_eq!(
        served_policy(&running, &"r".repeat(48)).await,
        (json!(policy), json!(policy.fingerprint().unwrap())),
        "tenant A's store does not apply its registry policy"
    );
    assert_eq!(
        served_policy(&running, &"s".repeat(48)).await,
        (json!(shipped), json!(shipped.fingerprint().unwrap())),
        "tenant B received a policy configured for tenant A"
    );
    running.child.kill().await.unwrap();
}

#[tokio::test]
async fn refused_retention_configuration_stops_the_gateway_before_any_store_exists() {
    let files = support::private_tempdir();
    let root = files.path();
    let (runner, viewer) = credentials(root, "local", 'r', 'v');
    let data = root.join("never-created.sqlite");
    let local = |policy_file: &Path| {
        let mut command = gateway(root);
        command
            .arg("--data")
            .arg(&data)
            .arg("--runner-token-file")
            .arg(&runner)
            .arg("--viewer-token-file")
            .arg(&viewer)
            .arg("--relay-retention-file")
            .arg(policy_file);
        command
    };

    // One sender able to exhaust the tenant: refused by the prevalidation.
    let mut unfair = tightened();
    unfair.sender.messages = 513;
    assert!(unfair.validate().is_err());
    let unfair_file = root.join("unfair.json");
    private_write(&unfair_file, serde_json::to_vec(&unfair).unwrap());
    let (status, stderr) = exit_of(local(&unfair_file)).await;
    assert!(!status.success(), "an unfair policy started the gateway");
    assert!(
        stderr.contains("two full sender shares"),
        "the refusal does not name the violated bound: {stderr}"
    );
    assert!(!data.exists(), "an invalid policy created the database");

    // A policy file readable by others is not a private operator file.
    let exposed_file = root.join("exposed.json");
    std::fs::write(&exposed_file, serde_json::to_vec(&tightened()).unwrap()).unwrap();
    std::fs::set_permissions(&exposed_file, std::fs::Permissions::from_mode(0o644)).unwrap();
    let (status, _) = exit_of(local(&exposed_file)).await;
    assert!(
        !status.success(),
        "a policy file readable by others was accepted"
    );
    assert!(!data.exists());

    // A closed contract: an unknown field is refused, not ignored.
    let mut unknown = serde_json::to_value(tightened()).unwrap();
    unknown["burst"] = json!(1);
    let unknown_file = root.join("unknown.json");
    private_write(&unknown_file, unknown.to_string());
    let (status, _) = exit_of(local(&unknown_file)).await;
    assert!(!status.success(), "an unknown retention field was ignored");
    assert!(!data.exists());

    // The local option and the registry are exclusive sources.
    let valid_file = root.join("valid.json");
    private_write(&valid_file, serde_json::to_vec(&tightened()).unwrap());
    let registry = root.join("tenants.json");
    let mut command = gateway(root);
    command
        .arg("--relay-retention-file")
        .arg(&valid_file)
        .arg("--tenants-file")
        .arg(&registry);
    let (status, stderr) = exit_of(command).await;
    assert!(!status.success());
    assert!(
        stderr.contains("--relay-retention-file") && stderr.contains("--tenants-file"),
        "the conflict is not reported by the argument parser: {stderr}"
    );

    // In the registry, an invalid policy of a later tenant leaves no earlier
    // tenant database behind.
    let mut tenants = Vec::new();
    for (tenant, runner, viewer) in [("tenant-a", 'a', 'b'), ("tenant-b", 'c', 'd')] {
        let (runner_file, viewer_file) = credentials(root, tenant, runner, viewer);
        tenants.push(json!({
            "tenant_id":tenant,
            "data":root.join(format!("{tenant}.sqlite")),
            "runner_token_file":runner_file,
            "viewer_token_file":viewer_file,
            "relay_retention_policy":tightened(),
        }));
    }
    tenants[1]["relay_retention_policy"] = json!(unfair);
    private_write(
        &registry,
        json!({"version":1,"tenants":tenants}).to_string(),
    );
    let mut command = gateway(root);
    command.arg("--tenants-file").arg(&registry);
    let (status, stderr) = exit_of(command).await;
    assert!(
        !status.success(),
        "an invalid policy of tenant B started the gateway"
    );
    assert!(
        stderr.contains("two full sender shares"),
        "startup did not reach tenant B's policy validation: {stderr}"
    );
    for tenant in ["tenant-a", "tenant-b"] {
        assert!(
            !root.join(format!("{tenant}.sqlite")).exists(),
            "an invalid policy of tenant B created the database of {tenant}"
        );
    }
}
