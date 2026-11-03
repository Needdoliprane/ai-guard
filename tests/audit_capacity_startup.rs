//! Invalid later tenant policies must not leave fresh supervised origins behind.
//! Real owned CLI child and private files; no provider, worker or HTTP client.
mod support;

use agentsandbox::{
    audit::{AuditCapacityPolicyV1, AuditCapacityV1},
    policy::Policy,
};
use serde_json::json;
use std::{os::unix::fs::PermissionsExt, path::Path, process::Stdio, time::Duration};
use tokio::io::AsyncReadExt;

fn private_write(path: &Path, bytes: impl AsRef<[u8]>) {
    std::fs::write(path, bytes).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

#[tokio::test]
async fn invalid_later_tenant_capacity_policy_creates_no_database_or_supervised_backup() {
    let files = support::private_tempdir();
    let root = files.path();
    let credentials = root.join("credentials");
    let data = root.join("data");
    for path in [&credentials, &data] {
        std::fs::create_dir(path).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    for (name, token) in [
        ("runner-a", "a"),
        ("viewer-a", "b"),
        ("runner-b", "c"),
        ("viewer-b", "d"),
    ] {
        private_write(&credentials.join(name), token.repeat(48));
    }
    let valid = AuditCapacityPolicyV1 {
        version: 1,
        human_tenant: AuditCapacityV1 {
            events: 32,
            bytes: 128 * 1024 * 1024,
        },
        human_principal: AuditCapacityV1 {
            events: 8,
            bytes: 32 * 1024 * 1024,
        },
        control_reserve: AuditCapacityV1 {
            events: 16,
            bytes: 64 * 1024 * 1024,
        },
        identity_reserve: AuditCapacityV1 {
            events: 8,
            bytes: 32 * 1024 * 1024,
        },
    };
    valid.validate(64).unwrap();
    let mut invalid = valid.clone();
    invalid.version = 2;
    assert!(invalid.validate(64).is_err());
    let db_a = data.join("a.sqlite");
    let db_b = data.join("b.sqlite");
    let backups = root.join("backups");
    let registry = root.join("tenants.json");
    private_write(&registry, serde_json::to_vec(&json!({
        "version": 1,
        "tenants": [
            {"tenant_id":"tenant-a", "data":db_a, "runner_token_file":credentials.join("runner-a"),
             "viewer_token_file":credentials.join("viewer-a"), "audit_capacity_policy":valid},
            {"tenant_id":"tenant-b", "data":db_b, "runner_token_file":credentials.join("runner-b"),
             "viewer_token_file":credentials.join("viewer-b"), "audit_capacity_policy":invalid}
        ]
    })).unwrap());
    let decision_policy = root.join("decisions.json");
    private_write(
        &decision_policy,
        serde_json::to_vec(&Policy::default()).unwrap(),
    );
    assert!(!db_a.exists() && !db_b.exists() && !backups.exists());

    // Empty child environment prevents inherited JEV/provider configuration.
    // If it unexpectedly starts serving, the wall guard kills/reaps this child
    // only. The guard is a harness failure, never the expected behavioral RED.
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
        .env_clear()
        .env("TMPDIR", root)
        .current_dir(root)
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
        .arg(&decision_policy)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
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
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        tokio::join!(child.wait(), drain)
    })
    .await;
    let (status, stderr) = match result {
        Ok((status, stderr)) => (status.unwrap(), String::from_utf8(stderr).unwrap()),
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            panic!("startup fixture child did not exit within its wall guard");
        }
    };
    assert!(
        !status.success(),
        "invalid tenant B policy must fail startup"
    );
    assert!(
        stderr.contains("unsupported audit capacity policy version"),
        "fixture must reach B's policy validation; CLI stderr: {stderr}"
    );
    let backup_entries = if backups.exists() {
        std::fs::read_dir(&backups).unwrap().count()
    } else {
        0
    };
    println!(
        "F_STARTUP exit={:?} database_a={} database_b={} backup_root={} backup_entries={backup_entries}",
        status.code(),
        db_a.exists(),
        db_b.exists(),
        backups.exists()
    );

    // FIRST expected RED: the current per-tenant loop has already created A
    // and its schema2 backup, then migrated A to3 before discovering invalid B.
    assert!(
        !db_a.exists(),
        "invalid B policy left a fresh tenant A origin behind"
    );
    assert!(!db_b.exists(), "invalid B policy created its database");
    assert!(
        !backups.exists(),
        "invalid policy must be refused before backup-root preparation"
    );
}
