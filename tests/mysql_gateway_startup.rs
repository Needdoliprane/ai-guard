//! The gateway refuses ambiguous and unsupported storage choices before startup effects.
mod support;

use agentsandbox::{
    diagnostics::gateway_files::{
        SelectedTenantStorage, TenantStorageFiles, select_tenant_storage,
    },
    policy::Policy,
};
use serde_json::{Value, json};
use std::{
    ffi::OsString, os::unix::fs::PermissionsExt, path::Path, process::Stdio, time::Duration,
};
use tokio::io::AsyncReadExt;

fn private_write(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

fn entries(path: &Path) -> Vec<OsString> {
    let mut entries: Vec<_> = std::fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    entries.sort();
    entries
}

fn mysql_storage(authentication: Value) -> Value {
    let mysql = json!({
        "backend":"mysql",
        "host":"127.0.0.1",
        "port":9,
        "database":"audit_fixture_b",
        "username":"writer_b",
        "namespace":"5657019d-5302-4ced-922f-406aa3d8ec5c",
        "password_file":"absent-sql-password",
        "ca_file":"absent-sql-ca",
        "authentication":authentication
    });
    let decoded = serde_json::from_value::<TenantStorageFiles>(mysql.clone()).unwrap();
    assert!(matches!(
        select_tenant_storage(None, Some(&decoded)),
        Ok(SelectedTenantStorage::MySql(_))
    ));
    mysql
}

#[tokio::test]
async fn ambiguous_mysql_selection_refuses_before_credentials_or_storage_effects() {
    assert_storage_startup_refused(
        mysql_storage(json!({"mode":"password"})),
        Some("b.sqlite"),
        "Error: Déclarer exactement une référence data ou un profil storage",
    )
    .await;
}

#[tokio::test]
async fn valid_mysql_profiles_refuse_before_credentials_or_storage_effects() {
    // MySQL builds resolve the SQL references before reading gateway credentials.
    #[cfg(feature = "mysql-storage")]
    let expected = "Error: MySQL password reference is not a private regular file";
    #[cfg(not(feature = "mysql-storage"))]
    let expected = "Error: Ce gateway refuse le profil MySQL tant que les transactions nécessaires sont indisponibles";

    for authentication in [
        json!({"mode":"password"}),
        json!({
            "mode":"password_and_mtls",
            "client_cert_file":"absent-sql-client-cert",
            "client_key_file":"absent-sql-client-key"
        }),
    ] {
        assert_storage_startup_refused(mysql_storage(authentication), None, expected).await;
    }
}

async fn assert_storage_startup_refused(mysql: Value, data: Option<&str>, expected: &str) {
    let fixture = support::private_tempdir();
    let root = fixture.path();
    let registry = root.join("tenants.json");
    let backups = root.join("backups");
    let policy = root.join("decisions.json");
    let mut tenant_b = json!({
        "tenant_id":"tenant-b",
        "storage":mysql,
        "runner_token_file":"absent-runner-b",
        "viewer_token_file":"absent-viewer-b"
    });
    if let Some(data) = data {
        tenant_b["data"] = json!(data);
    }
    private_write(
        &registry,
        &serde_json::to_vec(&json!({
            "version":1,
            "tenants":[
                {
                    "tenant_id":"tenant-a",
                    "storage":{"backend":"sqlite","path":"a.sqlite"},
                    "runner_token_file":"absent-runner-a",
                    "viewer_token_file":"absent-viewer-a"
                },
                tenant_b
            ]
        }))
        .unwrap(),
    );
    private_write(&policy, &serde_json::to_vec(&Policy::default()).unwrap());
    let before = entries(root);
    assert_eq!(
        before,
        [
            OsString::from("decisions.json"),
            OsString::from("tenants.json")
        ]
    );

    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
        .env_clear()
        .env("TMPDIR", root)
        .current_dir(root)
        .args(["gateway", "--listen", "127.0.0.1:0", "--tenants-file"])
        .arg(&registry)
        .arg("--initialize-supervised-stores")
        .arg(&backups)
        .arg("--policy")
        .arg(&policy)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let stderr = child.stderr.take().unwrap();
    let drain = async move {
        let mut bytes = Vec::new();
        stderr.take(64 * 1024).read_to_end(&mut bytes).await?;
        Ok::<_, std::io::Error>(bytes)
    };
    let finished = tokio::time::timeout(Duration::from_secs(30), async {
        tokio::join!(child.wait(), drain)
    })
    .await;
    let (status, stderr) = match finished {
        Ok((status, stderr)) => (status.unwrap(), String::from_utf8(stderr.unwrap()).unwrap()),
        Err(_) => {
            // A timeout is a failed test, and this owns the only child it kills.
            let _ = child.start_kill();
            assert!(
                matches!(
                    tokio::time::timeout(Duration::from_secs(5), child.wait()).await,
                    Ok(Ok(_))
                ),
                "gateway child cleanup could not be confirmed"
            );
            panic!("gateway child exceeded the startup test deadline");
        }
    };
    assert!(status.code().is_some_and(|code| code != 0));
    // An attempt to open A's absent runner credential would have ended the
    // loader with a file error instead, before proceeding to the next tenant.
    assert!(stderr.lines().any(|line| line == expected));
    assert!(!stderr.contains("Impossible de lire le fichier de secret configuré"));
    // Also catches fallback stores and SQLite sidecars under any name at this
    // fixture root, in addition to the two chosen paths and the backup root.
    assert_eq!(entries(root), before);
}
