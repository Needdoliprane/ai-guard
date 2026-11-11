//! `agentsandbox verify-audit` and the signed audit checkpoints, through the
//! real binaries: a gateway child on loopback, the operator command, and
//! `ai-guard-audit-checkpoint` for the key, the records and the anchor.
//!
//! Without the checkpoint options the command prints the gateway answer and
//! nothing else. With them it also prints the signed verdict, and a refusal or
//! an unreadable anchor makes it fail.

#![cfg(unix)]

use agentsandbox::{audit::Store, model::Event, redaction::Redactor, setup::SetupManifestV2};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    ffi::{OsStr, OsString},
    io::{BufRead, BufReader},
    net::SocketAddr,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

const TENANT: &str = "local";

fn private_dir(path: &Path) -> PathBuf {
    std::fs::DirBuilder::new().mode(0o700).create(path).unwrap();
    path.to_owned()
}

fn private_file(path: &Path, bytes: &[u8]) {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
}

/// One local installation: the database of the gateway, the operator
/// configuration, the signing area and the directory of the anchor holder.
struct Site {
    _root: tempfile::TempDir,
    database: PathBuf,
    config_dir: PathBuf,
    key_dir: PathBuf,
    log_dir: PathBuf,
    holder: PathBuf,
}

impl Site {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let control = private_dir(&root.path().join("control"));
        let config_dir = private_dir(&root.path().join("config"));
        let secrets = private_dir(&config_dir.join("secrets"));
        private_file(&secrets.join("runner"), "r".repeat(48).as_bytes());
        private_file(&secrets.join("viewer"), "v".repeat(48).as_bytes());
        let signing = private_dir(&root.path().join("signing"));
        Self {
            database: control.join("audit.sqlite"),
            config_dir,
            key_dir: signing.join("key-1"),
            log_dir: signing.join("checkpoints"),
            holder: private_dir(&root.path().join("holder")),
            _root: root,
        }
    }
}

/// Arguments of mixed kinds (words and paths) as one argument vector.
fn arguments(parts: &[&dyn AsRef<OsStr>]) -> Vec<OsString> {
    parts.iter().map(|part| part.as_ref().to_owned()).collect()
}

fn agentsandbox() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null());
    command
}

struct Gateway {
    child: Child,
    url: String,
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start_gateway(site: &Site) -> Gateway {
    let mut command = agentsandbox();
    command
        .args(["gateway", "--data"])
        .arg(&site.database)
        .arg("--runner-token-file")
        .arg(site.config_dir.join("secrets/runner"))
        .arg("--viewer-token-file")
        .arg(site.config_dir.join("secrets/viewer"));
    start_gateway_command(command)
}

fn start_gateway_command(mut command: Command) -> Gateway {
    let policy = Path::new(env!("CARGO_MANIFEST_DIR")).join("policies/default.json");
    let child = command
        .args(["--listen", "127.0.0.1:0"])
        .arg("--policy")
        .arg(policy)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn the gateway");
    // Own the child before any startup assertion: a timeout or malformed
    // announcement must still kill and reap this test's gateway.
    let mut gateway = Gateway {
        child,
        url: String::new(),
    };
    // The first line of standard error announces the bound address; the rest
    // is drained so that the child never blocks on a full pipe.
    let stderr = BufReader::new(gateway.child.stderr.take().unwrap());
    let (announce, announced) = mpsc::channel();
    std::thread::spawn(move || {
        for line in stderr.lines().map_while(Result::ok) {
            let _ = announce.send(line);
        }
    });
    let deadline = Instant::now() + Duration::from_secs(60);
    let address = loop {
        let line = announced
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("the gateway announces its address");
        if let Some(address) = line
            .strip_prefix("Gateway ")
            .and_then(|rest| rest.split_whitespace().next())
        {
            break address
                .parse::<SocketAddr>()
                .expect("a gateway socket address");
        }
    };
    assert!(
        address.ip().is_loopback(),
        "the gateway must bind to loopback"
    );
    let url = format!("http://{address}");
    let healthy = (0..100).any(|_| {
        let probe = agentsandbox()
            .args(["healthcheck", "--url", &format!("{url}/healthz")])
            .output()
            .unwrap();
        probe.status.success() || {
            std::thread::sleep(Duration::from_millis(100));
            false
        }
    });
    assert!(healthy, "the gateway did not become healthy");
    gateway.url = url;
    gateway
}

fn verify_audit<I, S>(site: &Site, gateway: &Gateway, extra: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    verify_audit_command(&site.config_dir, gateway)
        .args(extra)
        .output()
        .unwrap()
}

fn verify_audit_command(config_dir: &Path, gateway: &Gateway) -> Command {
    let mut command = agentsandbox();
    command
        .arg("verify-audit")
        .arg("--config-dir")
        .arg(config_dir)
        .arg("--gateway")
        .arg(&gateway.url);
    command
}

/// The JSON documents printed on standard output, in order.
fn documents(output: &Output) -> Vec<Value> {
    serde_json::Deserializer::from_slice(&output.stdout)
        .into_iter::<Value>()
        .map(|document| document.expect("standard output holds JSON documents only"))
        .collect()
}

#[test]
fn verify_audit_uses_setup_v2_viewer_token_and_preserves_legacy_default() {
    let root = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let root_path = root.path().canonicalize().unwrap();
    let mut manifest: SetupManifestV2 =
        serde_json::from_str(include_str!("../docs/setup/sso/examples/local-demo.json")).unwrap();
    let first_id = manifest.tenants[0].tenant_id.clone();
    let second_id = "verify-audit-tenant";
    let mut second = manifest.tenants[0].clone();
    second.tenant_id = second_id.into();
    second.organization_id = second_id.into();
    second.identities.clear();
    manifest.tenants.push(second);
    let first_capacity = &mut manifest.capacity_plan.tenants[0];
    first_capacity.capacity.max_records = 10_000;
    first_capacity.capacity.max_serialized_bytes = 8 * 1024 * 1024;
    let mut second_capacity = first_capacity.clone();
    second_capacity.tenant_id = second_id.into();
    manifest.capacity_plan.tenants.push(second_capacity);

    let input = root_path.join("manifest.json");
    private_file(&input, &serde_json::to_vec(&manifest).unwrap());
    let installation = root_path.join("installation");
    let prepared = agentsandbox()
        .args(["setup", "prepare", "--input"])
        .arg(&input)
        .arg("--output-dir")
        .arg(&installation)
        .output()
        .unwrap();
    assert!(
        prepared.status.success(),
        "SetupV2 failed: {}",
        prepared.status
    );
    assert_eq!(documents(&prepared)[0]["status"], "prepared");
    assert!(!installation.join("secrets/viewer").exists());

    let registry = installation.join("tenants.json");
    let config: Value = serde_json::from_slice(&std::fs::read(&registry).unwrap()).unwrap();
    let fixture = |tenant_id: &str| {
        let entry = config["tenants"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["tenant_id"] == tenant_id)
            .expect("SetupV2 registered the fixture tenant");
        let data = Path::new(entry["data"].as_str().unwrap());
        let viewer = PathBuf::from(entry["viewer_token_file"].as_str().unwrap());
        assert!(data.starts_with(&installation) && viewer.starts_with(&installation));
        let store =
            Store::open_existing_for_tenant(data, tenant_id, Redactor::new(vec![]), 100_000)
                .unwrap();
        let integrity = store.integrity().unwrap();
        assert!(integrity.valid);
        (viewer, integrity.head_hash)
    };
    let (first_viewer, first_head) = fixture(&first_id);
    let (second_viewer, second_head) = fixture(second_id);
    assert_ne!(
        first_head, second_head,
        "the tenant heads must discriminate"
    );

    let legacy_config = private_dir(&root_path.join("legacy-config"));
    let legacy_secrets = private_dir(&legacy_config.join("secrets"));
    private_file(
        &legacy_secrets.join("viewer"),
        agentsandbox::runner::read_token(&first_viewer)
            .unwrap()
            .as_bytes(),
    );
    let mut gateway_command = agentsandbox();
    gateway_command
        .args(["gateway", "--tenants-file"])
        .arg(&registry);
    let gateway = start_gateway_command(gateway_command);

    let legacy = verify_audit_command(&legacy_config, &gateway)
        .output()
        .unwrap();
    assert!(
        legacy.status.success(),
        "legacy verify-audit failed: {}",
        legacy.status
    );
    let answers = documents(&legacy);
    assert_eq!(answers.len(), 1);
    assert_eq!(answers[0]["valid"], true);
    assert_eq!(answers[0]["head_hash"], first_head);

    let explicit = verify_audit_command(&installation, &gateway)
        .arg("--viewer-token-file")
        .arg(second_viewer)
        .output()
        .unwrap();
    let option_rejected = String::from_utf8_lossy(&explicit.stderr)
        .contains("unexpected argument '--viewer-token-file'");
    assert!(
        explicit.status.success(),
        "verify-audit did not accept the SetupV2 viewer file: {}, option_rejected={option_rejected}",
        explicit.status
    );
    let answers = documents(&explicit);
    assert_eq!(answers.len(), 1);
    assert_eq!(answers[0]["valid"], true);
    assert_eq!(answers[0]["head_hash"], second_head);
}

fn checkpoint<I, S>(arguments: I) -> Value
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new(env!("CARGO_BIN_EXE_ai-guard-audit-checkpoint"))
        .args(arguments)
        .env_clear()
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(output.status.success(), "{document}");
    document
}

fn create(site: &Site, new_log: bool) -> Value {
    let mut command = arguments(&[
        &"create",
        &"--tenant-id",
        &TENANT,
        &"--data",
        &site.database,
        &"--key-dir",
        &site.key_dir,
        &"--checkpoint-dir",
        &site.log_dir,
    ]);
    if new_log {
        command.push("--new-log".into());
    }
    checkpoint(command)
}

fn chain_digest(seq: i64, received_at: &str, previous: &str, payload: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"agentsandbox:audit:v1\0");
    hasher.update(seq.to_be_bytes());
    for part in [received_at, previous, payload] {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    hex::encode(hasher.finalize())
}

/// Rewrite the last event coherently, as an actor with write access to the
/// stopped database would: the payload changes and the public chain digest is
/// recomputed, so the unkeyed reader still finds an intact chain.
fn rewrite_last_note(database: &Path) {
    let db = rusqlite::Connection::open(database).unwrap();
    let (seq, received_at, previous, payload): (i64, String, String, String) = db
        .query_row(
            "SELECT seq,received_at,prev_hash,payload FROM events ORDER BY seq DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert!(payload.contains("\"note\":\"alpha\""));
    let rewritten = payload.replace("\"note\":\"alpha\"", "\"note\":\"bravo\"");
    let hash = chain_digest(seq, &received_at, &previous, &rewritten);
    db.execute(
        "UPDATE events SET payload=?1,hash=?2 WHERE seq=?3",
        rusqlite::params![rewritten, hash, seq],
    )
    .unwrap();
}

#[test]
fn verify_audit_renders_the_signed_verdict_only_when_asked() {
    let site = Site::new();
    let gateway = start_gateway(&site);

    // Without the checkpoint options: the gateway answer, pretty-printed, and
    // nothing else, exactly as before.
    let plain = verify_audit(&site, &gateway, [] as [&OsStr; 0]);
    assert!(plain.status.success(), "{plain:?}");
    let answers = documents(&plain);
    assert_eq!(answers.len(), 1, "{plain:?}");
    assert_eq!(answers[0]["valid"], true);
    assert_eq!(
        String::from_utf8(plain.stdout.clone()).unwrap(),
        format!("{}\n", serde_json::to_string_pretty(&answers[0]).unwrap())
    );
    assert!(plain.stderr.is_empty(), "{plain:?}");

    // The operator signs a first checkpoint while the gateway runs, and the
    // anchor holder approves the key by its fingerprint.
    let key = checkpoint(arguments(&[&"key", &"init", &"--key-dir", &site.key_dir]));
    let public_key = site.holder.join("key-1.public-key.json");
    checkpoint(arguments(&[
        &"key",
        &"show",
        &"--key-dir",
        &site.key_dir,
        &"--out",
        &public_key,
    ]));
    let first = create(&site, true);
    let anchor = site.holder.join("anchor-1.json");
    checkpoint(arguments(&[
        &"anchor",
        &"init",
        &"--tenant-id",
        &TENANT,
        &"--expect-key-id",
        &key["key_id"].as_str().unwrap(),
        &"--namespace-uuid",
        &first["namespace_uuid"].as_str().unwrap(),
        &"--public-key",
        &public_key,
        &"--out",
        &anchor,
    ]));
    let signed = |anchor: &Path| {
        arguments(&[
            &"--checkpoint-dir",
            &site.log_dir,
            &"--anchor",
            &anchor,
            &"--data",
            &site.database,
        ])
    };

    // With them: the same first document, then the signed verdict.
    let verified = verify_audit(&site, &gateway, signed(&anchor));
    assert!(verified.status.success(), "{verified:?}");
    let both = documents(&verified);
    assert_eq!(both.len(), 2, "{verified:?}");
    assert_eq!(both[0]["valid"], true);
    assert!(
        String::from_utf8(verified.stdout.clone())
            .unwrap()
            .starts_with(&format!(
                "{}\n",
                serde_json::to_string_pretty(&both[0]).unwrap()
            ))
    );
    assert_eq!(
        both[1]["format"],
        "ai-guard.audit.checkpoint-verification.v1"
    );
    assert_eq!(both[1]["verdict"], "verified");
    assert_eq!(both[1]["database"]["compared"], true);
    assert_eq!(both[1]["verified_through_log_index"], 1);

    // One option without the other is a usage error, before any request.
    let partial = verify_audit(
        &site,
        &gateway,
        arguments(&[&"--checkpoint-dir", &site.log_dir]),
    );
    assert!(!partial.status.success());
    assert!(partial.stdout.is_empty(), "{partial:?}");

    // An anchor that is not there: the gateway answer, then a structured
    // error, and a failing status.
    let missing = verify_audit(&site, &gateway, signed(&site.holder.join("no-anchor.json")));
    assert!(!missing.status.success(), "{missing:?}");
    let both = documents(&missing);
    assert_eq!(both.len(), 2, "{missing:?}");
    assert_eq!(both[0]["valid"], true);
    assert_eq!(both[1]["errors"][0]["code"], "anchor_unavailable");

    // The gateway stops; one more event is appended and signed; then that
    // event is rewritten coherently in the stopped database.
    drop(gateway);
    let store =
        Store::open_for_tenant(&site.database, TENANT, Redactor::new(vec![]), 100_000).unwrap();
    let mut event = Event::new("fixture.note");
    event.data = serde_json::json!({ "note": "alpha" });
    store.append(event).unwrap();
    drop(store);
    assert_eq!(create(&site, false)["log_index"], 2);
    rewrite_last_note(&site.database);

    // The restarted gateway finds its unkeyed chain intact, and the plain
    // command still succeeds: this is what the signed verdict is for.
    let gateway = start_gateway(&site);
    let plain = verify_audit(&site, &gateway, [] as [&OsStr; 0]);
    assert!(plain.status.success(), "{plain:?}");
    assert_eq!(documents(&plain)[0]["valid"], true);

    let refused = verify_audit(&site, &gateway, signed(&anchor));
    assert!(!refused.status.success(), "{refused:?}");
    let both = documents(&refused);
    assert_eq!(both.len(), 2, "{refused:?}");
    assert_eq!(both[0]["valid"], true);
    assert_eq!(both[1]["verdict"], "refused");
    assert_eq!(both[1]["errors"][0]["code"], "database_diverged");
    assert_eq!(both[1]["errors"][0]["detail"]["signed_at_log_index"], 2);
}
