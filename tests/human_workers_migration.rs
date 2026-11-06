//! Explicit9→10 migration and restored historical backup, with real private SQLite.
mod support;
use agentsandbox::{audit::Store, identity::*, redaction::Redactor};
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    process::{Command, Stdio},
};
use uuid::Uuid;

fn schema9(path: &Path, capacity: u64) -> Store {
    let store = Store::open_for_tenant(path, "tenant-a", Redactor::new(vec![]), capacity).unwrap();
    store
        .migrate_control_v3(&path.with_extension("before3"))
        .unwrap();
    store
        .migrate_identity_v4(&path.with_extension("before4"))
        .unwrap();
    store
        .migrate_human_access_v5(&path.with_extension("before5"))
        .unwrap();
    store
        .migrate_workload_v6(&path.with_extension("before6"))
        .unwrap();
    store
        .migrate_human_brokers_v7(&path.with_extension("before7"))
        .unwrap();
    store
        .migrate_kubernetes_create_v8(&path.with_extension("before8"))
        .unwrap();
    store
        .migrate_kubernetes_policy_v9(&path.with_extension("before9"))
        .unwrap();
    store
}
fn change(change: IdentityChangeV1) -> IdentityMutationV1 {
    IdentityMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        actor: PrincipalRefV1::LegacyControl {
            credential_id: "human-workers-migration-control".into(),
            role: LegacyControlRoleV1::Runner,
        },
        change,
    }
}
fn preparation() -> LocalHumanProvisionV1 {
    let principal = PrincipalRefV1::HumanLocal {
        installation_id: Uuid::new_v4().to_string(),
        account_id: Uuid::new_v4().to_string(),
    };
    let token = generate_local_credential().unwrap();
    LocalHumanProvisionV1 {
        version: 1,
        mutations: [
            change(IdentityChangeV1::Account {
                expected_revision: 0,
                account: HumanAccountV1 {
                    version: 1,
                    principal: principal.clone(),
                    display_name: "Migration worker reader".into(),
                    revision: 1,
                    active: true,
                },
            }),
            change(IdentityChangeV1::Credential {
                expected_revision: 0,
                binding: LocalCredentialV1 {
                    version: 1,
                    credential_id: Uuid::new_v4().to_string(),
                    principal: principal.clone(),
                    sha256_digest: hex::encode(Sha256::digest(token.as_bytes())),
                    revision: 1,
                    active: true,
                    expires_at: None,
                },
            }),
            change(IdentityChangeV1::Membership {
                expected_revision: 0,
                membership: MembershipV1 {
                    version: 1,
                    membership_id: Uuid::new_v4().to_string(),
                    principal,
                    tenant_id: "tenant-a".into(),
                    organization_scope: "organization-a".into(),
                    revision: 1,
                    active: true,
                    capabilities: [HumanCapabilityV1::ReadWorkerRegistry]
                        .into_iter()
                        .collect(),
                },
            }),
        ],
    }
}

#[test]
fn explicit_cli_migration_preserves_exact9_backup_and_reopen_without_activation() {
    let files = support::private_tempdir();
    let path = files.path().join("tenant.sqlite");
    let store = schema9(&path, 1000);
    let before = store.integrity().unwrap();
    let namespace = store.supervision_state().unwrap().control_namespace_uuid;
    let prepared = preparation();
    assert_eq!(
        store
            .provision_local_human(&prepared)
            .unwrap_err()
            .downcast_ref::<IdentityFailureV1>(),
        Some(&IdentityFailureV1::MigrationRequired)
    );
    assert_eq!(store.integrity().unwrap().head_hash, before.head_hash);
    drop(store);
    let backup = files.path().join("exact9.backup");
    let output = Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
        .args(["migrate-control", "--data"])
        .arg(&path)
        .args(["--tenant-id", "tenant-a", "--backup"])
        .arg(&backup)
        .args([
            "--target-version",
            "10",
            "--confirm-control-stopped",
            "--max-events",
            "1000",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let upgraded =
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    let state = upgraded.supervision_state().unwrap();
    assert_eq!(state.schema_version, 10);
    assert_eq!(state.control_namespace_uuid, namespace);
    assert!(!state.active, "migration10 must not activate supervision");
    assert!(upgraded.integrity().unwrap().valid);
    assert_eq!(
        upgraded.integrity().unwrap().event_count,
        before.event_count + 1
    );
    upgraded.provision_local_human(&prepared).unwrap();
    let head = upgraded.head().unwrap();
    upgraded.provision_local_human(&prepared).unwrap();
    assert_eq!(
        upgraded.head().unwrap(),
        head,
        "exact identity replay is idempotent"
    );
    drop(upgraded);
    let reopened =
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    assert!(reopened.integrity().unwrap().valid);
    assert_eq!(reopened.head().unwrap(), head);
    drop(reopened);
    let saved =
        Store::open_existing_for_tenant(&backup, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    assert_eq!(saved.supervision_state().unwrap().schema_version, 9);
    assert_eq!(saved.integrity().unwrap().head_hash, before.head_hash);
    assert_eq!(saved.integrity().unwrap().event_count, before.event_count);
    drop(saved);
    // Restore into a new private destination; never overwrite the upgraded source.
    let restored_path = files.path().join("restored9.sqlite");
    std::fs::copy(&backup, &restored_path).unwrap();
    let restored =
        Store::open_existing_for_tenant(&restored_path, "tenant-a", Redactor::new(vec![]), 1000)
            .unwrap();
    assert_eq!(restored.supervision_state().unwrap().schema_version, 9);
    assert!(!restored.supervision_state().unwrap().active);
    assert_eq!(
        restored
            .provision_local_human(&prepared)
            .unwrap_err()
            .downcast_ref::<IdentityFailureV1>(),
        Some(&IdentityFailureV1::MigrationRequired)
    );
    assert_eq!(restored.integrity().unwrap().head_hash, before.head_hash);
}

#[test]
fn migration10_audit_capacity_refusal_keeps_schema9_and_exact_backup() {
    let files = support::private_tempdir();
    let path = files.path().join("tenant.sqlite");
    let store = schema9(&path, 64);
    let principal = PrincipalRefV1::HumanLocal {
        installation_id: Uuid::new_v4().to_string(),
        account_id: Uuid::new_v4().to_string(),
    };
    let mut exhausted = false;
    for prior in 0..64 {
        if store
            .mutate_identity(&change(IdentityChangeV1::Account {
                expected_revision: prior,
                account: HumanAccountV1 {
                    version: 1,
                    principal: principal.clone(),
                    display_name: "Capacity migration probe".into(),
                    revision: prior + 1,
                    active: true,
                },
            }))
            .is_err()
        {
            exhausted = true;
            break;
        }
    }
    assert!(exhausted);
    let before = store.integrity().unwrap();
    let backup = files.path().join("full9.backup");
    assert!(store.migrate_human_workers_v10(&backup).is_err());
    assert_eq!(store.supervision_state().unwrap().schema_version, 9);
    assert_eq!(store.integrity().unwrap().head_hash, before.head_hash);
    let saved =
        Store::open_existing_for_tenant(&backup, "tenant-a", Redactor::new(vec![]), 64).unwrap();
    assert_eq!(saved.supervision_state().unwrap().schema_version, 9);
    assert_eq!(saved.integrity().unwrap().head_hash, before.head_hash);
}

#[test]
fn schema_version_cannot_hide_a_workers_capability_marker() {
    let files = support::private_tempdir();
    let path = files.path().join("tenant.sqlite");
    let store = schema9(&path, 1000);
    store
        .migrate_human_workers_v10(&path.with_extension("exact9"))
        .unwrap();
    drop(store);
    let db = rusqlite::Connection::open(&path).unwrap();
    db.pragma_update(None, "user_version", 9).unwrap();
    drop(db);
    assert!(
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).is_err(),
        "a schema downcast must not make a new capability valid on9"
    );
}
