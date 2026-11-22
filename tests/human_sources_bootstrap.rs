//! Real producer -> CLI -> coordinator -> activation consumer -> HTTP reads.
//! Synthetic private data only; no runtime engine, IdP or model is contacted.
#[path = "support/human_sources_bootstrap.rs"]
mod support;
use serde_json::Value;
use std::{
    fs,
    process::{Command, Stdio},
    time::Duration,
};
use support::Fixture;

fn successful(output: std::process::Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn prepare_check_resume_and_readoption_preserve_real_legacy_and_stable_ids() {
    let f = Fixture::new(false);
    successful(f.prepare());
    let recovery = fs::read(f.preparation.join("recovery.json")).unwrap();
    let envelope = fs::read(f.preparation.join("technical-credentials.json")).unwrap();
    for source in &f.sources {
        let db = rusqlite::Connection::open(source).unwrap();
        assert_eq!(
            db.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
                .unwrap(),
            13
        );
    }
    successful(f.step("check", false));
    successful(f.step("resume-prepare", true));
    assert_eq!(
        recovery,
        fs::read(f.preparation.join("recovery.json")).unwrap()
    );
    assert_eq!(
        envelope,
        fs::read(f.preparation.join("technical-credentials.json")).unwrap()
    );
    assert!(
        !f.step("resume", true).status.success(),
        "resume must not create an absent real journal intention"
    );
    successful(f.step("apply", true));
    let published = fs::read(f.preparation.join("activation-complete.json")).unwrap();
    successful(f.step("resume", true));
    assert_eq!(
        published,
        fs::read(f.preparation.join("activation-complete.json")).unwrap()
    );
    assert!(
        !f.prepare().status.success(),
        "existing output must never be readopted"
    );
    let other = f.root.join("readoption");
    let output = Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
        .env_clear()
        .args(["human-sources", "prepare", "--tenant-registry"])
        .arg(&f.registry)
        .arg("--installation-id")
        .arg(f.installation.to_string())
        .args(["--control-config-revision", "7", "--output-dir"])
        .arg(&other)
        .arg("--confirm-control-stopped")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!other.exists());
}

#[test]
fn cli_store15_reuses_exact_closure_and_leaves_the_neighbor_unchanged() {
    use agentsandbox::human_authority::HumanAuthorityClosureV1;
    use rusqlite::{Connection, OpenFlags};
    use std::{
        os::{fd::AsRawFd, unix::fs::MetadataExt},
        path::Path,
    };
    use uuid::Uuid;

    let f = Fixture::new(false);
    successful(f.prepare());
    successful(f.step("apply", true));
    let activation_path = f.preparation.join("activation.json");
    let activation_bytes = fs::read(&activation_path).unwrap();
    let activation: Value = serde_json::from_slice(&activation_bytes).unwrap();
    let original_manifest_path = activation["manifest"].as_str().unwrap();
    let original_manifest = fs::read(original_manifest_path).unwrap();
    let original = HumanAuthorityClosureV1::from_bytes(&original_manifest).unwrap();
    let original_envelope = f.preparation.join("technical-credentials.json");
    let before_credentials = fs::read(&original_envelope).unwrap();
    let snapshot = |source: usize| {
        let db = Connection::open_with_flags(
            &f.sources[source],
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .unwrap();
        let format = db
            .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .unwrap();
        let events = db
            .prepare("SELECT seq,kind,hash FROM events ORDER BY seq")
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        (format, events)
    };
    let before_a = snapshot(0);
    let before_b = snapshot(1);
    let id = Uuid::new_v4();
    let destination = f.root.join("store15-a");
    let upgrade = |output: &Path, resume, operation_id: Uuid| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
        command
            .env_clear()
            .args([
                "human-sources",
                "upgrade-egress",
                "--human-sources-activation",
            ])
            .arg(&activation_path)
            .args(["--tenant", "tenant-a", "--operation-id"])
            .arg(operation_id.to_string())
            .arg("--output-dir")
            .arg(output)
            .arg("--confirm-control-stopped");
        if resume {
            command.arg("--resume");
        }
        command.output().unwrap()
    };
    let explicit_upgrade = |manifest: &Path, envelope: &Path, output: &Path, tenant: &str| {
        Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
            .env_clear()
            .args([
                "human-sources",
                "upgrade-egress",
                "--human-sources-manifest",
            ])
            .arg(manifest)
            .arg("--human-credentials-envelope")
            .arg(envelope)
            .arg("--tenants-file")
            .arg(&f.registry)
            .args([
                "--human-control-config-revision",
                "7",
                "--tenant",
                tenant,
                "--operation-id",
            ])
            .arg(Uuid::new_v4().to_string())
            .arg("--output-dir")
            .arg(output)
            .arg("--confirm-control-stopped")
            .output()
            .unwrap()
    };
    let entries = |directory: &Path| {
        let mut names = fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        names.sort();
        names
    };
    let refused = |output: std::process::Output, code: &str| {
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains(code));
    };
    let preparation_entries = entries(&f.preparation);
    let nested = f.preparation.join("store15-output");
    refused(upgrade(&nested, false, id), "store15_output_protected");
    refused(
        explicit_upgrade(
            Path::new(original_manifest_path),
            &original_envelope,
            &nested,
            "tenant-a",
        ),
        "store15_output_protected",
    );
    let alias = f.root.join(
        f.preparation
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_ascii_uppercase(),
    );
    if alias != f.preparation && alias.try_exists().unwrap() {
        refused(
            upgrade(&alias.join("store15-output"), false, id),
            "store15_output_protected",
        );
    }
    assert_eq!(entries(&f.preparation), preparation_entries);
    assert!(fs::read(&activation_path).unwrap() == activation_bytes);
    let checked = f.step("check", false);
    assert!(checked.status.success());
    assert!(String::from_utf8_lossy(&checked.stdout).starts_with("activation_files_valid"));
    assert_eq!(snapshot(0), before_a);
    assert_eq!(snapshot(1), before_b);

    let occupied = f.root.join("store15-occupied");
    support::private_dir(&occupied);
    refused(upgrade(&occupied, false, id), "store15_output_not_fresh");
    assert!(entries(&occupied).is_empty());

    let root_before_contention = entries(&f.root);
    let lock = fs::File::open(
        f.sources[0]
            .parent()
            .unwrap()
            .join(".ai-guard-human-authority-v1.lock"),
    )
    .unwrap();
    // SAFETY: this descriptor belongs to the test's disposable source owner.
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    refused(upgrade(&destination, false, id), "store15_control_active");
    assert_eq!(entries(&f.root), root_before_contention);
    drop(lock);
    assert_eq!(snapshot(0), before_a);
    assert_eq!(snapshot(1), before_b);

    successful(upgrade(&destination, false, id));
    let after_a = snapshot(0);
    assert_eq!(before_a.0, 14);
    assert_eq!(after_a.0, 15);
    assert_eq!(after_a.1.len(), before_a.1.len() + 1);
    assert_eq!(&after_a.1[..before_a.1.len()], before_a.1.as_slice());
    assert_eq!(after_a.1.last().unwrap().1, "control.egress_policy.genesis");
    assert_eq!(snapshot(1), before_b);
    let manifest = destination.join("closure/manifest.json");
    let bytes = fs::read(&manifest).unwrap();
    let upgraded = HumanAuthorityClosureV1::from_bytes(&bytes).unwrap();
    assert_eq!(upgraded.closure_id, original.closure_id);
    assert_eq!(upgraded.closure_revision, original.closure_revision + 1);
    let mut expected_sources = original.sources.clone();
    expected_sources
        .iter_mut()
        .find(|source| source.tenant_id.as_deref() == Some("tenant-a"))
        .unwrap()
        .required_format = 15;
    assert!(upgraded.sources == expected_sources);
    let mut expected_credentials: Value = serde_json::from_slice(&before_credentials).unwrap();
    expected_credentials["closure_revision"] = upgraded.closure_revision.into();
    let credentials: Value =
        serde_json::from_slice(&fs::read(destination.join("technical-credentials.json")).unwrap())
            .unwrap();
    assert!(
        credentials == expected_credentials,
        "technical identities and roles are preserved"
    );
    let before_backup = fs::metadata(destination.join("backup/source.sqlite")).unwrap();

    successful(upgrade(&destination, true, id));
    // Exercise the same committed/confirmed state without a visible closure.
    // The first publication stays intact in the disposable fixture.
    let retained_closure = f.root.join("retained-store15-closure");
    fs::rename(destination.join("closure"), &retained_closure).unwrap();
    assert!(!manifest.exists());
    successful(upgrade(&destination, true, id));
    assert!(fs::read(retained_closure.join("manifest.json")).unwrap() == bytes);
    assert!(!upgrade(&destination, true, Uuid::new_v4()).status.success());
    let after_backup = fs::metadata(destination.join("backup/source.sqlite")).unwrap();
    assert_eq!(
        (after_backup.dev(), after_backup.ino()),
        (before_backup.dev(), before_backup.ino())
    );
    assert!(fs::read(&manifest).unwrap() == bytes);
    assert!(fs::read(&original_envelope).unwrap() == before_credentials);
    assert!(fs::read(original_manifest_path).unwrap() == original_manifest);
    assert_eq!(snapshot(0), after_a);
    assert_eq!(snapshot(1), before_b);

    let second_output = f.root.join("store15-b");
    successful(explicit_upgrade(
        &manifest,
        &destination.join("technical-credentials.json"),
        &second_output,
        "tenant-b",
    ));
    let second = HumanAuthorityClosureV1::from_bytes(
        &fs::read(second_output.join("closure/manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(second.closure_revision, upgraded.closure_revision + 1);
    let mut expected_sources = upgraded.sources.clone();
    expected_sources
        .iter_mut()
        .find(|source| source.tenant_id.as_deref() == Some("tenant-b"))
        .unwrap()
        .required_format = 15;
    assert!(second.sources == expected_sources);
    assert_eq!(snapshot(0), after_a);
    let after_b = snapshot(1);
    assert_eq!(after_b.0, 15);
    assert_eq!(after_b.1.len(), before_b.1.len() + 1);
    assert_eq!(&after_b.1[..before_b.1.len()], before_b.1.as_slice());
    assert_eq!(after_b.1.last().unwrap().1, "control.egress_policy.genesis");
}

#[test]
fn changed_token_before_envelope_and_truncated_publications_are_preserved() {
    let f = Fixture::new(false);
    successful(f.prepare());
    // An already durable recovery is retained; remove no product artifact.
    // Mutating one synthetic input must conflict before any new publication.
    let token = f.root.join("tenant-a-runner.token");
    let original_token = fs::read(&token).unwrap();
    let before = fs::read(f.preparation.join("technical-credentials.json")).unwrap();
    fs::write(&token, b"synthetic-tenant-a-CHANGED-credential-long-enough").unwrap();
    assert!(!f.step("resume-prepare", true).status.success());
    assert_eq!(
        before,
        fs::read(f.preparation.join("technical-credentials.json")).unwrap()
    );
    let checked = f.step("check", false);
    assert!(String::from_utf8_lossy(&checked.stdout).starts_with("conflict"));
    fs::write(&token, original_token).unwrap();
    let seal = f.preparation.join("preparation-complete.json");
    fs::write(&seal, b"{").unwrap();
    assert!(!f.step("resume-prepare", true).status.success());
    assert!(!f.step("apply", true).status.success());
    assert_eq!(fs::read(&seal).unwrap(), b"{");
}

#[test]
fn activation_contradictory_cli_options_refuse_before_sqlite_or_file_creation() {
    let f = Fixture::new(false);
    for flag in [
        "--human-source-mode",
        "--tenants-file",
        "--max-events",
        "--data",
        "--runner-token-file",
        "--audit-capacity-file",
    ] {
        let value = if flag == "--human-source-mode" {
            "owned_sources"
        } else if flag == "--max-events" {
            "100000"
        } else {
            "/absent/private-input"
        };
        let output = Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
            .env_clear()
            .current_dir(&f.root)
            .args([
                "gateway",
                "--human-sources-activation",
                "/absent/activation.json",
                flag,
                value,
            ])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(!f.root.join(".agentsandbox").exists());
    }
}

#[test]
fn valid_same_inode_legacy_change_after_prepare_is_examined_at_apply_without_a_prepared_head() {
    use agentsandbox::{audit::Store, identity::*, redaction::Redactor};
    use uuid::Uuid;
    let f = Fixture::new(false);
    successful(f.prepare());
    let source = &f.sources[0];
    let store = Store::open_existing_for_tenant(source, "tenant-a", Redactor::new(vec![]), 100_000)
        .unwrap();
    store
        .mutate_identity(&IdentityMutationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            actor: PrincipalRefV1::LegacyControl {
                credential_id: "synthetic-stopped-operator".into(),
                role: LegacyControlRoleV1::Runner,
            },
            change: IdentityChangeV1::Account {
                expected_revision: 0,
                account: HumanAccountV1 {
                    version: 1,
                    principal: PrincipalRefV1::HumanLocal {
                        installation_id: f.installation.to_string(),
                        account_id: Uuid::new_v4().to_string(),
                    },
                    display_name: "Synthetic later named account".into(),
                    revision: 1,
                    active: true,
                },
            },
        })
        .unwrap();
    drop(store);
    let history = |limit: i64| {
        let db = rusqlite::Connection::open_with_flags(
            source,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .unwrap();
        let mut query = db
            .prepare("SELECT seq,hash FROM events WHERE seq<=?1 ORDER BY seq")
            .unwrap();
        query
            .query_map([limit], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    let before = history(i64::MAX);
    let last = before.last().unwrap().0;
    successful(f.step("apply", true));
    assert_eq!(
        history(last),
        before,
        "the real apply-time legacy prefix must be preserved"
    );
}

async fn complete_and_read(oidc: bool) {
    let f = Fixture::new(oidc);
    successful(f.prepare());
    successful(f.step("apply", true));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
        .env_clear()
        .args(["gateway", "--human-sources-activation"])
        .arg(f.preparation.join("activation.json"))
        .arg("--listen")
        .arg(address.to_string())
        .args(["--jev-backend", "rust-rules", "--policy"])
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/policies/default.json"
        ))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let origin = format!("http://{address}");
    let identity = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            assert!(
                child.try_wait().unwrap().is_none(),
                "gateway exited before a real human read"
            );
            if let Ok(response) = client
                .get(format!("{origin}/api/v1/human/identity"))
                .bearer_auth(&f.human_bearer)
                .send()
                .await
            {
                if response.status().is_success() {
                    break response.json::<Value>().await.unwrap();
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(identity["contexts_complete"], false);
    let contexts = identity["contexts"].as_array().unwrap();
    assert!(!contexts.is_empty());
    assert!(contexts.iter().all(|c| c["tenant_id"] == "tenant-a"));
    let response = client
        .get(format!("{origin}/api/v1/human/resources"))
        .bearer_auth(&f.human_bearer)
        .header("x-aiguard-context", contexts[0]["id"].as_str().unwrap())
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    let resources: Value = response.json().await.unwrap();
    let receipt = &resources["read_receipt"];
    let event_id = receipt["audit_event_id"].as_str().unwrap();
    let db = rusqlite::Connection::open_with_flags(
        &f.sources[0],
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap();
    let payload: String = db
        .query_row("SELECT payload FROM events WHERE id=?1", [event_id], |r| {
            r.get(0)
        })
        .unwrap();
    let event: Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(event["data"]["request_id"], receipt["request_id"]);
    assert_eq!(event["data"]["action"], "resources");
    drop(db);
    if oidc {
        assert!(
            !client
                .get(format!("{origin}/api/v1/oidc/config"))
                .send()
                .await
                .unwrap()
                .status()
                .is_success()
        );
    }
    // Signal only this test's owned child; wait for the actual gateway cleanup.
    // SAFETY: child.id() is the still-owned gateway process spawned above;
    // SIGINT requests its normal registered shutdown and no other PID is used.
    unsafe {
        libc::kill(child.id().unwrap() as i32, libc::SIGINT);
    }
    assert!(
        tokio::time::timeout(Duration::from_secs(20), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
}

#[tokio::test]
async fn cli_activation_reaches_real_identity_resources_and_durable_receipt_without_oidc() {
    complete_and_read(false).await;
}
#[tokio::test]
async fn cli_activation_reaches_real_identity_resources_with_selected_auth5_union() {
    complete_and_read(true).await;
}
