//! Real CLI/configuration boundaries. No worker, provider or container runtime.
mod support;
use agentsandbox::{
    audit::{AuditCapacityV1, Store, WorkloadAuditPolicyV1},
    policy::Policy,
    redaction::Redactor,
};
use serde_json::json;
use std::{os::unix::fs::PermissionsExt, path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    process::{Child, Command},
};

fn write(path: &Path, bytes: impl AsRef<[u8]>) {
    std::fs::write(path, bytes).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}
fn workload() -> WorkloadAuditPolicyV1 {
    WorkloadAuditPolicyV1 {
        version: 1,
        tenant: AuditCapacityV1 {
            events: 16,
            bytes: 67108864,
        },
        principal: AuditCapacityV1 {
            events: 4,
            bytes: 16777216,
        },
    }
}
fn command(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
    command
        .env_clear()
        .env("TMPDIR", root)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}
async fn finish(mut child: Child) -> std::process::Output {
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let drain = async move {
        let mut out = Vec::new();
        let mut err = Vec::new();
        tokio::join!(
            async {
                stdout.take(65536).read_to_end(&mut out).await.unwrap();
            },
            async {
                stderr.take(65536).read_to_end(&mut err).await.unwrap();
            }
        );
        (out, err)
    };
    match tokio::time::timeout(Duration::from_secs(30), async {
        tokio::join!(child.wait(), drain)
    })
    .await
    {
        Ok((status, (stdout, stderr))) => std::process::Output {
            status: status.unwrap(),
            stdout,
            stderr,
        },
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            panic!("owned CLI fixture exceeded its wall guard");
        }
    }
}
fn v5(path: &Path, tenant: &str) -> Store {
    let store = Store::open_for_tenant(path, tenant, Redactor::new(vec![]), 64).unwrap();
    store
        .migrate_control_v3(&path.with_extension("v2.bak"))
        .unwrap();
    store
        .migrate_identity_v4(&path.with_extension("v3.bak"))
        .unwrap();
    store
        .migrate_human_access_v5(&path.with_extension("v4.bak"))
        .unwrap();
    store
}

#[tokio::test]
async fn migration_cli_target6_requires_attestation_and_preserves_a_v5_backup() {
    let directory = support::private_tempdir();
    let root = directory.path();
    let path = root.join("tenant.sqlite");
    let store = v5(&path, "tenant-a");
    let before = store.integrity().unwrap().head_hash;
    drop(store);
    let backup = root.join("v5.bak");
    let mut denied = command(root);
    denied
        .args(["migrate-control", "--data"])
        .arg(&path)
        .args(["--tenant-id", "tenant-a", "--backup"])
        .arg(&backup)
        .args(["--target-version", "6", "--max-events", "64"]);
    assert!(!finish(denied.spawn().unwrap()).await.status.success());
    assert!(!backup.exists());
    let mut accepted = command(root);
    accepted
        .args(["migrate-control", "--data"])
        .arg(&path)
        .args(["--tenant-id", "tenant-a", "--backup"])
        .arg(&backup)
        .args([
            "--target-version",
            "6",
            "--max-events",
            "64",
            "--confirm-control-stopped",
        ]);
    let output = finish(accepted.spawn().unwrap()).await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["from_version"], 5);
    assert_eq!(report["to_version"], 6);
    let backed =
        Store::open_existing_for_tenant(&backup, "tenant-a", Redactor::new(vec![]), 64).unwrap();
    assert_eq!(backed.integrity().unwrap().head_hash, before);
    assert_eq!(backed.supervision_state().unwrap().schema_version, 5);
    let migrated =
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 64).unwrap();
    assert_eq!(migrated.supervision_state().unwrap().schema_version, 6);
    assert!(!migrated.supervision_state().unwrap().active);
}

#[tokio::test]
async fn invalid_later_workload_policy_creates_no_database_or_backup_directory() {
    let directory = support::private_tempdir();
    let root = directory.path();
    let credentials = root.join("credentials");
    std::fs::create_dir(&credentials).unwrap();
    std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o700)).unwrap();
    for (name, token) in [
        ("runner-a", "a"),
        ("viewer-a", "b"),
        ("runner-b", "c"),
        ("viewer-b", "d"),
    ] {
        write(&credentials.join(name), token.repeat(48));
    }
    let good = workload();
    let mut bad = good.clone();
    bad.tenant.events = 41;
    let a = root.join("a.sqlite");
    let b = root.join("b.sqlite");
    let backups = root.join("backups");
    let registry = root.join("tenants.json");
    let decisions = root.join("decisions.json");
    write(&registry,serde_json::to_vec(&json!({"version":1,"tenants":[
        {"tenant_id":"tenant-a","data":a,"runner_token_file":credentials.join("runner-a"),"viewer_token_file":credentials.join("viewer-a"),"workload_audit_policy":good},
        {"tenant_id":"tenant-b","data":b,"runner_token_file":credentials.join("runner-b"),"viewer_token_file":credentials.join("viewer-b"),"workload_audit_policy":bad}
    ]})).unwrap());
    write(&decisions, serde_json::to_vec(&Policy::default()).unwrap());
    let mut gateway = command(root);
    gateway
        .args([
            "gateway",
            "--listen",
            "127.0.0.1:0",
            "--max-events",
            "64",
            "--tenants-file",
        ])
        .arg(&registry)
        .arg("--initialize-supervised-stores")
        .arg(&backups)
        .arg("--policy")
        .arg(&decisions);
    let output = finish(gateway.spawn().unwrap()).await;
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!output.status.success());
    assert!(
        stderr.contains("workload audit policy must preserve"),
        "{stderr}"
    );
    assert!(!a.exists() && !b.exists() && !backups.exists());
    for token in ["a", "b", "c", "d"] {
        assert!(!stderr.contains(&token.repeat(48)));
    }
}

#[tokio::test]
async fn invalid_local_worker_configuration_is_refused_before_preparing_backups() {
    let directory = support::private_tempdir();
    let root = directory.path();
    let credentials = root.join("credentials");
    std::fs::create_dir(&credentials).unwrap();
    std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o700)).unwrap();
    let runner = credentials.join("runner");
    let viewer = credentials.join("viewer");
    let decisions = root.join("decisions.json");
    let data = root.join("local.sqlite");
    let backups = root.join("backups");
    write(&runner, "r".repeat(48));
    write(&viewer, "v".repeat(48));
    write(&decisions, serde_json::to_vec(&Policy::default()).unwrap());
    let mut gateway = command(root);
    gateway
        .args([
            "gateway",
            "--listen",
            "127.0.0.1:0",
            "--worker-limit",
            "101",
            "--data",
        ])
        .arg(&data)
        .arg("--runner-token-file")
        .arg(&runner)
        .arg("--viewer-token-file")
        .arg(&viewer)
        .arg("--policy")
        .arg(&decisions)
        .arg("--initialize-supervised-stores")
        .arg(&backups);
    let output = finish(gateway.spawn().unwrap()).await;
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!output.status.success());
    assert!(stderr.contains("Quota workers invalide"), "{stderr}");
    assert!(!data.exists() && !backups.exists());
}

#[tokio::test]
async fn v5_gateway_keeps_first_readiness_line_and_reports_resolved_private_workload_budget() {
    let directory = support::private_tempdir();
    let root = directory.path();
    let path = root.join("tenant.sqlite");
    let store = v5(&path, "local");
    let before = store.head().unwrap();
    drop(store);
    let runner = root.join("runner");
    let viewer = root.join("viewer");
    let decisions = root.join("decisions.json");
    let budget = root.join("workload.json");
    write(&runner, "r".repeat(48));
    write(&viewer, "v".repeat(48));
    write(&decisions, serde_json::to_vec(&Policy::default()).unwrap());
    write(&budget, serde_json::to_vec(&workload()).unwrap());
    let mut gateway = command(root);
    gateway
        .args([
            "gateway",
            "--listen",
            "127.0.0.1:0",
            "--max-events",
            "64",
            "--data",
        ])
        .arg(&path)
        .arg("--runner-token-file")
        .arg(&runner)
        .arg("--viewer-token-file")
        .arg(&viewer)
        .arg("--workload-audit-file")
        .arg(&budget)
        .arg("--policy")
        .arg(&decisions);
    let mut child = gateway.spawn().unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let lines = tokio::time::timeout(Duration::from_secs(30), async {
        let mut lines = Vec::new();
        for _ in 0..3 {
            let mut line = String::new();
            assert!(stderr.read_line(&mut line).await.unwrap() > 0);
            lines.push(line);
        }
        lines
    })
    .await;
    let _ = child.kill().await;
    let _ = child.wait().await;
    let lines = lines.expect("gateway never reached readiness diagnostics");
    assert!(lines[0].starts_with("Gateway "), "{}", lines[0]);
    assert!(lines[1].starts_with("Audit capacity "));
    assert!(lines[2].contains(
        "Workload audit capacity tenant=local source=private_policy_file admission_budget=enabled"
    ));
    assert!(lines[2].contains(&workload().fingerprint().unwrap()));
    for line in lines {
        assert!(!line.contains(&"r".repeat(48)) && !line.contains(&"v".repeat(48)));
    }
    let store = Store::open_existing_for_tenant(&path, "local", Redactor::new(vec![]), 64).unwrap();
    assert_eq!(store.head().unwrap(), before);
    assert_eq!(store.supervision_state().unwrap().schema_version, 5);
}
