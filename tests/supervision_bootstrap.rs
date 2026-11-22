//! Fresh tenant-scoped schema3 bootstrap with a verified schema2 backup.

mod support;

use agentsandbox::{
    audit::Store,
    control::{
        AdmissionV1, ControlFailure, DecisionPhaseV1, DecisionReceiptV1, LaunchSpecV1, NetworkV1,
        QuotaPolicyV1, RestartPolicyV1, RuntimeKindV1, SecurityPresetV1, SupervisedAdmissionV1,
        SupervisionProtocolV1,
    },
    model::{CommandSpec, DecisionRequest, Event},
    policy::{self, Policy},
    redaction::Redactor,
};
use serde_json::json;
use std::path::Path;
use uuid::Uuid;

const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn decision_admission(store: &Store) -> AdmissionV1 {
    let session_id = Uuid::new_v4().to_string();
    let request = DecisionRequest {
        request_id: Uuid::new_v4().to_string(),
        profile: "strict".into(),
        command: CommandSpec {
            argv: vec!["/usr/bin/id".into()],
            cwd: "/workspace".into(),
        },
        project_id: None,
        sandbox_id: None,
        agent_id: None,
        session_id: Some(session_id.clone()),
        correlation_id: None,
        trace_id: None,
        human_approved: false,
        approval_hash: None,
        execution_mode: None,
        provider_resolution: None,
    };
    let policy = Policy::default();
    let stored = store
        .decision(
            &request,
            Some((policy::evaluate(&policy, &request), json!(null))),
        )
        .unwrap()
        .unwrap();
    AdmissionV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        receipt: DecisionReceiptV1 {
            version: 1,
            request_id: request.request_id.clone(),
            request_hash: policy::request_hash(&request),
            phase: DecisionPhaseV1::Initial,
            audit_seq: stored.seq,
            audit_hash: stored.hash,
        },
        launch: LaunchSpecV1 {
            version: 1,
            session_id,
            command: request.command.clone(),
            profile: request.profile.clone(),
            runtime_kind: RuntimeKindV1::Docker,
            authority_id: Uuid::new_v4().to_string(),
            image_ref: "worker:fixture".into(),
            image_id: format!("sha256:{HASH_A}"),
            network: NetworkV1::None,
            cpu_millis: 2000,
            memory_bytes: 1_073_741_824,
            pids_limit: 256,
            timeout_seconds: 600,
            security_preset: SecurityPresetV1::FixedSandboxV2,
            restart_policy: RestartPolicyV1::No,
            workspace_manifest_digest: HASH_B.into(),
            workspace_import_policy_version: 1,
            provider: "generic".into(),
            capture_output: false,
            credential_binding_id: None,
            credential_names: vec![],
            provider_resolution: None,
            egress: None,
        },
        decision_request: request,
    }
}

fn supervised_request(admission: AdmissionV1) -> SupervisedAdmissionV1 {
    SupervisedAdmissionV1 {
        version: 1,
        protocol: SupervisionProtocolV1::BrokerSupervisionV1,
        intent_hash: HASH_A.into(),
        admission,
    }
}

#[test]
fn fresh_supervised_bootstrap_verifies_backup_and_reopens_in_maintenance() {
    let directory = support::private_tempdir();
    let target = directory.path().join("tenant-a.sqlite");
    // Keep the explicit backup beside its target, as the bootstrap contract
    // holds the source guard only through schema2 creation, then releases it
    // before migration while the backup receives its own guard.
    let backup_path = directory.path().join("tenant-a.before-supervision.sqlite");

    let store = Store::create_supervised_for_tenant(
        &target,
        "tenant-a",
        Redactor::new(vec![]),
        10_000,
        &backup_path,
    )
    .expect("fresh explicit bootstrap creates and activates schema3");
    let active = store.supervision_state().unwrap();
    assert_eq!(active.schema_version, 3);
    assert!(active.active);
    assert_eq!(
        active.protocols,
        [SupervisionProtocolV1::BrokerSupervisionV1]
    );
    let namespace = active.control_namespace_uuid.clone().unwrap();
    assert!(store.integrity().unwrap().valid);
    assert!(backup_path.exists());

    let backup = Store::open_for_tenant(&backup_path, "tenant-a", Redactor::new(vec![]), 10_000)
        .expect("verified pre-migration backup is an ordinary readable store");
    let backup_state = backup.supervision_state().unwrap();
    assert_eq!(backup_state.schema_version, 2);
    assert!(!backup_state.active);
    assert_eq!(
        backup_state.control_namespace_uuid.as_deref(),
        Some(namespace.as_str())
    );
    assert!(backup.integrity().unwrap().valid);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&backup_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    drop(backup);
    drop(store);

    let reopened = Store::open_for_tenant(&target, "tenant-a", Redactor::new(vec![]), 10_000)
        .expect("schema3 remains readable after reopen");
    let reopened_state = reopened.supervision_state().unwrap();
    assert_eq!(reopened_state.schema_version, 3);
    assert!(
        !reopened_state.active,
        "reopen must not restore fresh-origin authority"
    );
    assert_eq!(
        reopened_state.control_namespace_uuid.as_deref(),
        Some(namespace.as_str())
    );

    let request = supervised_request(decision_admission(&reopened));
    let policy = Policy::default();
    let before = reopened.head().unwrap();
    let error = reopened
        .admit_supervised(
            &request,
            &policy,
            policy::evaluate(&policy, &request.admission.decision_request),
            json!(null),
            &QuotaPolicyV1::default(),
        )
        .expect_err("reopened schema3 store remains in maintenance");
    assert_eq!(
        error.downcast_ref::<ControlFailure>(),
        Some(&ControlFailure::SupervisionInactive)
    );
    assert_eq!(
        reopened.head().unwrap(),
        before,
        "maintenance refusal must not append"
    );
    assert!(reopened.workers().unwrap().is_empty());
}

#[test]
fn legacy_default_open_stays_schema2_and_inactive() {
    let directory = support::private_tempdir();
    let store = Store::open(
        &directory.path().join("legacy-default.sqlite"),
        Redactor::new(vec![]),
        100,
    )
    .unwrap();
    let state = store.supervision_state().unwrap();
    assert_eq!(state.schema_version, 2);
    assert!(!state.active);
    assert!(state.protocols.is_empty());
}

const SQLITE_LOCK_PROBE_ENV: &str = "AI_GUARD_BOOTSTRAP_LOCK_PROBE";

#[test]
#[ignore = "child-only F_GETLK and journal_mode=DELETE lock probe"]
fn sqlite_lock_probe_child_for_bootstrap_refusal() {
    let Some(path) = std::env::var_os(SQLITE_LOCK_PROBE_ENV) else {
        return;
    };
    use std::os::fd::AsRawFd;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    // SAFETY: initialized flock asks F_GETLK to inspect a conflicting lock.
    let mut lock: libc::flock = unsafe { std::mem::zeroed() };
    lock.l_type = libc::F_WRLCK as _;
    lock.l_whence = libc::SEEK_SET as _;
    lock.l_start = 0;
    lock.l_len = 0;
    assert_eq!(
        unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETLK, &mut lock) },
        0
    );
    assert_ne!(lock.l_type, libc::F_UNLCK as libc::c_short);
    drop(file);
    let db = rusqlite::Connection::open(&path).unwrap();
    db.busy_timeout(std::time::Duration::from_millis(100))
        .unwrap();
    assert!(db.execute_batch("PRAGMA journal_mode=DELETE").is_err());
}

fn assert_sqlite_lock_survives_factory_refusal(path: &Path, phase: &str) {
    use std::process::{Command, Stdio};
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "sqlite_lock_probe_child_for_bootstrap_refusal",
            "--ignored",
        ])
        .env(SQLITE_LOCK_PROBE_ENV, path)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "SQLite lock probe failed {phase}; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn bootstrap_durably_creates_missing_nested_target_and_backup_parents() {
    let directory = support::private_tempdir();
    let target_parent = directory.path().join("new-target").join("nested");
    let backup_parent = directory.path().join("new-backups").join("tenant");
    let target = target_parent.join("audit.sqlite");
    let backup = backup_parent.join("before-v3.sqlite");
    let store = Store::create_supervised_for_tenant(
        &target,
        "tenant-a",
        Redactor::new(vec![]),
        100,
        &backup,
    )
    .expect("bootstrap creates and synchronizes missing private parent chains");
    assert_eq!(store.supervision_state().unwrap().schema_version, 3);
    assert!(store.supervision_state().unwrap().active);
    assert!(backup.is_file());
    for parent in [
        directory.path().join("new-target"),
        target_parent,
        directory.path().join("new-backups"),
        backup_parent,
    ] {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let metadata = std::fs::metadata(&parent).unwrap();
            assert_eq!(metadata.mode() & 0o777, 0o700);
            assert_eq!(metadata.uid(), unsafe { libc::geteuid() });
        }
    }
    drop(store);
    let backup_store =
        Store::open_for_tenant(&backup, "tenant-a", Redactor::new(vec![]), 100).unwrap();
    assert_eq!(backup_store.supervision_state().unwrap().schema_version, 2);
    assert!(backup_store.integrity().unwrap().valid);
}

#[test]
fn bootstrap_refuses_preexisting_target_and_backup_without_closing_existing_store() {
    let directory = support::private_tempdir();
    let target = directory.path().join("existing.sqlite");
    let backup = directory.path().join("must-not-be-created.sqlite");
    let existing = Store::open_for_tenant(&target, "tenant-a", Redactor::new(vec![]), 100).unwrap();
    let before = existing.head().unwrap();
    let independent = rusqlite::Connection::open(&target).unwrap();
    let _: i64 = independent
        .query_row("SELECT count(*) FROM events", [], |row| row.get(0))
        .unwrap();
    assert_sqlite_lock_survives_factory_refusal(&target, "before refusal");

    let Err(error) = Store::create_supervised_for_tenant(
        &target,
        "tenant-b",
        Redactor::new(vec![]),
        100,
        &backup,
    ) else {
        panic!("bootstrap must not rebind or replace a preexisting tenant database");
    };
    assert_sqlite_lock_survives_factory_refusal(&target, "immediately after factory refusal");
    assert!(!error.to_string().is_empty());
    assert_eq!(existing.supervision_state().unwrap().schema_version, 2);
    assert_eq!(existing.head().unwrap(), before);
    assert!(existing.integrity().unwrap().valid);
    assert!(!backup.exists());
    existing
        .append(Event::new("bootstrap.refusal.preserves.writer"))
        .unwrap();
    assert_eq!(
        existing.head().unwrap(),
        before + 1,
        "the preexisting Store writer remains usable"
    );
    drop(existing);

    let fresh_target = directory.path().join("fresh-target.sqlite");
    let preexisting_backup = directory.path().join("sentinel-backup.sqlite");
    let sentinel = b"preserve-existing-backup-destination";
    std::fs::write(&preexisting_backup, sentinel).unwrap();
    assert!(
        Store::create_supervised_for_tenant(
            &fresh_target,
            "tenant-a",
            Redactor::new(vec![]),
            100,
            &preexisting_backup,
        )
        .is_err()
    );
    assert_eq!(std::fs::read(&preexisting_backup).unwrap(), sentinel);
}

#[test]
fn invalid_tenant_is_refused_before_database_or_backup_creation() {
    let directory = support::private_tempdir();
    let target = directory.path().join("tenant.sqlite");
    let backup = directory.path().join("backup.sqlite");
    assert!(
        Store::create_supervised_for_tenant(
            &target,
            "../tenant",
            Redactor::new(vec![]),
            100,
            &backup,
        )
        .is_err()
    );
    assert!(!target.exists());
    assert!(!backup.exists());
}

#[test]
fn post_backup_migration_failure_preserves_target_and_verified_partial_backup() {
    let directory = support::private_tempdir();
    let target = directory.path().join("limited.sqlite");
    let backup = directory.path().join("limited.before-supervision.sqlite");

    // Schema2 initialization consumes the single allowed event. Schema3
    // migration then fails after it has created, copied and verified backup.
    let error = match Store::create_supervised_for_tenant(
        &target,
        "tenant-a",
        Redactor::new(vec![]),
        1,
        &backup,
    ) {
        Err(error) => error,
        Ok(_) => panic!("schema3 genesis cannot exceed the configured audit limit"),
    };
    assert!(!error.to_string().is_empty());
    assert!(
        target.exists(),
        "failed migration keeps its initialized target for diagnosis"
    );
    assert!(
        backup.exists(),
        "durable backup must be retained after post-copy failure"
    );

    let target_store = Store::open_for_tenant(&target, "tenant-a", Redactor::new(vec![]), 1)
        .expect("target returns to its intact schema2 state");
    let target_state = target_store.supervision_state().unwrap();
    assert_eq!(target_state.schema_version, 2);
    assert!(!target_state.active);
    assert!(target_store.integrity().unwrap().valid);
    drop(target_store);

    let backup_store = Store::open_for_tenant(&backup, "tenant-a", Redactor::new(vec![]), 1)
        .expect("verified partial backup remains readable");
    let backup_state = backup_store.supervision_state().unwrap();
    assert_eq!(backup_state.schema_version, 2);
    assert!(!backup_state.active);
    assert!(backup_store.integrity().unwrap().valid);
}

struct OwnedBootstrapChild(Option<std::process::Child>);

impl OwnedBootstrapChild {
    fn new(child: std::process::Child) -> Self {
        Self(Some(child))
    }

    fn child(&mut self) -> &mut std::process::Child {
        self.0.as_mut().expect("owned child remains present")
    }

    fn wait_bounded(&mut self, duration: std::time::Duration) -> Option<std::process::ExitStatus> {
        let deadline = std::time::Instant::now() + duration;
        loop {
            match self.0.as_mut()?.try_wait() {
                Ok(Some(status)) => {
                    self.0.take();
                    return Some(status);
                }
                Ok(None) => {}
                Err(_) => return None,
            }
            if std::time::Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    fn stop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
        }
        let _ = self.wait_bounded(std::time::Duration::from_secs(2));
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
        }
    }
}

impl Drop for OwnedBootstrapChild {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run_bounded_gateway_cli(
    mut command: std::process::Command,
) -> (std::process::ExitStatus, String) {
    use std::{io::Read, process::Stdio, sync::mpsc};
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let child = command.spawn().expect("spawn synthetic gateway CLI");
    let mut owned = OwnedBootstrapChild::new(child);
    let mut stderr = owned.child().stderr.take().unwrap();
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        let mut output = String::new();
        let _ = stderr.read_to_string(&mut output);
        let _ = send.send(output);
    });
    let status = owned
        .wait_bounded(std::time::Duration::from_secs(5))
        .unwrap_or_else(|| {
            owned.stop();
            panic!("synthetic CLI child did not exit within five seconds");
        });
    let output = receive
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("stderr reader should finish after bounded child exit");
    (status, output)
}

fn start_bootstrap_test_gateway(
    registry: Option<&Path>,
    data: Option<&Path>,
    runner: Option<&Path>,
    viewer: Option<&Path>,
    backup_root: Option<&Path>,
    policy: &Path,
) -> (OwnedBootstrapChild, String) {
    use std::{
        process::{Command, Stdio},
        sync::mpsc,
    };

    let mut command = Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
    command.args(["gateway", "--listen", "127.0.0.1:0"]);
    if let Some(registry) = registry {
        command.arg("--tenants-file").arg(registry);
    }
    if let Some(data) = data {
        command.arg("--data").arg(data);
    }
    if let Some(runner) = runner {
        command.arg("--runner-token-file").arg(runner);
    }
    if let Some(viewer) = viewer {
        command.arg("--viewer-token-file").arg(viewer);
    }
    if let Some(backup_root) = backup_root {
        command
            .arg("--initialize-supervised-stores")
            .arg(backup_root);
    }
    let child = command
        .arg("--policy")
        .arg(policy)
        .stdin(std::process::Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn synthetic gateway child");
    let mut owned = OwnedBootstrapChild::new(child);
    let (send, receive) = mpsc::channel();
    let mut stderr = owned.child().stderr.take().unwrap();
    std::thread::spawn(move || {
        use std::io::BufRead;
        let mut line = String::new();
        let _ = std::io::BufReader::new(&mut stderr).read_line(&mut line);
        let _ = send.send(line);
    });
    let readiness = receive.recv_timeout(std::time::Duration::from_secs(5));
    match readiness {
        Ok(line) if line.starts_with("Gateway ") => {
            let address = line
                .strip_prefix("Gateway ")
                .and_then(|rest| rest.split_whitespace().next())
                .expect("readiness has a bound local address");
            (owned, format!("http://{address}"))
        }
        Ok(line) => {
            owned.stop();
            panic!("gateway exited before readiness: {line}");
        }
        Err(error) => {
            owned.stop();
            panic!("gateway readiness timed out: {error}");
        }
    }
}

#[tokio::test]
async fn cli_rejects_backup_root_overlapping_configured_credential_directory_before_database_creation()
 {
    use std::{os::unix::fs::PermissionsExt, process::Command};

    let directory = support::private_tempdir();
    let root = directory.path();
    let secrets_a = root.join("secrets-a");
    let secrets_b = root.join("secrets-b");
    let db_dir = root.join("data");
    for private_dir in [&secrets_a, &secrets_b, &db_dir] {
        std::fs::create_dir(private_dir).unwrap();
        std::fs::set_permissions(private_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    std::fs::create_dir(secrets_a.join("nested-backup")).unwrap();
    std::fs::set_permissions(
        secrets_a.join("nested-backup"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    for (path, value) in [
        (
            secrets_a.join("runner"),
            "synthetic-runner-token-a-0123456789abcdef",
        ),
        (
            secrets_a.join("viewer"),
            "synthetic-viewer-token-a-0123456789abcdef",
        ),
        (
            secrets_b.join("runner"),
            "synthetic-runner-token-b-0123456789abcdef",
        ),
        (
            secrets_b.join("viewer"),
            "synthetic-viewer-token-b-0123456789abcdef",
        ),
    ] {
        std::fs::write(path, value).unwrap();
    }
    let registry = root.join("tenants.json");
    std::fs::write(
        &registry,
        serde_json::json!({
            "version": 1,
            "tenants": [
                {"tenant_id":"tenant-a","data":"data/a.sqlite","runner_token_file":"secrets-a/runner","viewer_token_file":"secrets-a/viewer"},
                {"tenant_id":"tenant-b","data":"data/b.sqlite","runner_token_file":"secrets-b/runner","viewer_token_file":"secrets-b/viewer"}
            ]
        }).to_string(),
    )
    .unwrap();
    let policy = Path::new(env!("CARGO_MANIFEST_DIR")).join("policies/default.json");

    let overlap_cases = vec![
        ("same-directory-a", secrets_a.clone()),
        ("same-directory-b", secrets_b.clone()),
        ("ancestor", root.to_path_buf()),
        ("descendant", secrets_a.join("nested-backup")),
    ];
    // Exercise a real Darwin /var alias only when both spellings resolve to
    // the same existing inode; Linux does not emulate this filesystem rule.
    #[cfg(target_os = "macos")]
    let overlap_cases = {
        use std::{os::unix::fs::MetadataExt, path::PathBuf};
        let mut overlap_cases = overlap_cases;
        let text = secrets_a.to_string_lossy();
        let alias = text
            .strip_prefix("/var/")
            .map(|rest| PathBuf::from(format!("/private/var/{rest}")))
            .or_else(|| {
                text.strip_prefix("/private/var/")
                    .map(|rest| PathBuf::from(format!("/var/{rest}")))
            });
        if let Some(alias) = alias.filter(|candidate| candidate.is_dir()) {
            let actual = std::fs::metadata(&secrets_a).unwrap();
            let through_alias = std::fs::metadata(&alias).unwrap();
            if actual.dev() == through_alias.dev() && actual.ino() == through_alias.ino() {
                overlap_cases.push(("darwin-var-alias", alias));
            }
        }
        overlap_cases
    };
    for (label, backup_root) in overlap_cases {
        let mut command = Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
        command
            .args(["gateway", "--listen", "127.0.0.1:0", "--tenants-file"])
            .arg(&registry)
            .arg("--initialize-supervised-stores")
            .arg(&backup_root)
            .arg("--policy")
            .arg(&policy);
        let (status, stderr) = run_bounded_gateway_cli(command);
        let stderr = stderr.to_lowercase();
        assert!(
            !status.success(),
            "{label}: overlapping backup root must be rejected"
        );
        assert!(
            stderr.contains("backup") && stderr.contains("credential"),
            "{label}: expected credential-overlap diagnostic; synthetic CLI stderr was: {stderr}"
        );
        assert!(!db_dir.join("a.sqlite").exists());
        assert!(!db_dir.join("b.sqlite").exists());
    }

    // Dedicated sibling is valid. Readiness is emitted only after both tenant
    // stores and backups were initialized, then stop only this owned process.
    let backup_root = root.join("backups");
    std::fs::create_dir(&backup_root).unwrap();
    std::fs::set_permissions(&backup_root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let (mut gateway, gateway_url) = start_bootstrap_test_gateway(
        Some(&registry),
        None,
        None,
        None,
        Some(&backup_root),
        &policy,
    );
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .no_proxy()
        .build()
        .unwrap();
    let mut namespaces = Vec::new();
    for (tenant, token) in [
        ("a", "synthetic-runner-token-a-0123456789abcdef"),
        ("b", "synthetic-runner-token-b-0123456789abcdef"),
    ] {
        let response = http
            .get(format!("{gateway_url}/api/v1/control/protocol"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let value: serde_json::Value = response.json().await.unwrap();
        assert_eq!(value["tenant_id"], format!("tenant-{tenant}"));
        assert_eq!(value["role"], "runner");
        assert_eq!(value["schema_version"], 3);
        assert_eq!(value["active"], true);
        let namespace = value["control_namespace_uuid"].as_str().unwrap();
        assert_ne!(Uuid::parse_str(namespace).unwrap(), Uuid::nil());
        namespaces.push(namespace.to_owned());
    }
    assert_ne!(namespaces[0], namespaces[1]);
    gateway.stop();

    use sha2::Digest;
    for tenant in ["a", "b"] {
        let db = db_dir.join(format!("{tenant}.sqlite"));
        let store = Store::open_for_tenant(
            &db,
            &format!("tenant-{tenant}"),
            Redactor::new(vec![]),
            100_000,
        )
        .unwrap();
        let state = store.supervision_state().unwrap();
        assert_eq!(state.schema_version, 3);
        assert!(
            !state.active,
            "reopening gateway-created schema3 enters maintenance"
        );
        let digest = sha2::Sha256::digest(format!("tenant-{tenant}").as_bytes());
        let backup = backup_root.join(format!("tenant-{digest:x}-v2.sqlite"));
        let backup_store = Store::open_for_tenant(
            &backup,
            &format!("tenant-{tenant}"),
            Redactor::new(vec![]),
            100_000,
        )
        .unwrap();
        assert_eq!(backup_store.supervision_state().unwrap().schema_version, 2);
        assert!(backup_store.integrity().unwrap().valid);
    }

    // Restart without the opt-in flag is allowed for reads but does not reactivate.
    let (mut restarted, restart_url) =
        start_bootstrap_test_gateway(Some(&registry), None, None, None, None, &policy);
    for (index, (tenant, token)) in [
        ("a", "synthetic-runner-token-a-0123456789abcdef"),
        ("b", "synthetic-runner-token-b-0123456789abcdef"),
    ]
    .into_iter()
    .enumerate()
    {
        let response = http
            .get(format!("{restart_url}/api/v1/control/protocol"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let value: serde_json::Value = response.json().await.unwrap();
        assert_eq!(value["tenant_id"], format!("tenant-{tenant}"));
        assert_eq!(value["role"], "runner");
        assert_eq!(value["schema_version"], 3);
        assert_eq!(value["active"], false);
        assert_eq!(value["control_namespace_uuid"], namespaces[index]);
    }
    restarted.stop();
    for tenant in ["a", "b"] {
        let db = db_dir.join(format!("{tenant}.sqlite"));
        let store = Store::open_for_tenant(
            &db,
            &format!("tenant-{tenant}"),
            Redactor::new(vec![]),
            100_000,
        )
        .unwrap();
        assert_eq!(store.supervision_state().unwrap().schema_version, 3);
        assert!(!store.supervision_state().unwrap().active);
    }

    // Repeating opt-in on an existing DB must refuse before disturbing its
    // live SQLite locks or creating any backup artifact.
    let existing_path = db_dir.join("a.sqlite");
    let existing =
        Store::open_for_tenant(&existing_path, "tenant-a", Redactor::new(vec![]), 100_000).unwrap();
    let before_head = existing.head().unwrap();
    let independent = rusqlite::Connection::open(&existing_path).unwrap();
    let _: i64 = independent
        .query_row("SELECT count(*) FROM events", [], |row| row.get(0))
        .unwrap();
    assert_sqlite_lock_survives_factory_refusal(&existing_path, "before CLI retry");
    let retry_backup_root = root.join("retry-backups");
    std::fs::create_dir(&retry_backup_root).unwrap();
    std::fs::set_permissions(&retry_backup_root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut retry_command = Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
    retry_command
        .args(["gateway", "--listen", "127.0.0.1:0", "--tenants-file"])
        .arg(&registry)
        .arg("--initialize-supervised-stores")
        .arg(&retry_backup_root)
        .arg("--policy")
        .arg(&policy);
    let (retry_status, _) = run_bounded_gateway_cli(retry_command);
    assert!(!retry_status.success());
    assert_sqlite_lock_survives_factory_refusal(&existing_path, "immediately after CLI retry");
    assert_eq!(existing.head().unwrap(), before_head);
    assert!(
        std::fs::read_dir(&retry_backup_root)
            .unwrap()
            .next()
            .is_none()
    );

    // The historical single-tenant path without an opt-in flag remains schema2.
    let local_db = root.join("legacy.sqlite");
    let (mut local, _) = start_bootstrap_test_gateway(
        None,
        Some(&local_db),
        Some(&secrets_a.join("runner")),
        Some(&secrets_a.join("viewer")),
        None,
        &policy,
    );
    local.stop();
    let legacy =
        Store::open_for_tenant(&local_db, "local", Redactor::new(vec![]), 100_000).unwrap();
    assert_eq!(legacy.supervision_state().unwrap().schema_version, 2);
    assert!(!legacy.supervision_state().unwrap().active);
}
