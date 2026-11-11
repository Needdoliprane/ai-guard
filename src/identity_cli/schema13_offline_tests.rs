//! Actual schema13 identity attribution and private recovery replay.
use super::*;
use std::os::unix::fs::PermissionsExt;

fn schema13(path: &Path) -> Store {
    let store = Store::open_for_tenant(path, "tenant-a", Redactor::new(vec![]), 10_000).unwrap();
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
        .migrate_human_workers_v10(&path.with_extension("before10"))
        .unwrap();
    store
        .migrate_kubernetes_session_reservations_v11(&path.with_extension("before11"))
        .unwrap();
    store
        .migrate_broker_checkpoints_v12(&path.with_extension("before12"))
        .unwrap();
    store
        .migrate_kubernetes_recovery_v13(&path.with_extension("before13"))
        .unwrap();
    store
}

fn offline(path: &Path) -> OfflineArgs {
    OfflineArgs {
        data: path.to_owned(),
        tenant_id: "tenant-a".into(),
        confirm_control_stopped: true,
        max_events: 10_000,
        audit_capacity_file: None,
    }
}

#[test]
fn schema13_local_registry_identity_authenticates_and_replays_without_rotating_or_appending() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let source = directory.path().join("tenant.sqlite");
    drop(schema13(&source));
    let store = Store::open_existing_for_tenant(&source, "tenant-a", Redactor::new(vec![]), 10_000)
        .unwrap();
    assert_eq!(store.supervision_state().unwrap().schema_version, 13);
    assert!(!store.supervision_state().unwrap().active);
    let before = store.head().unwrap();
    drop(store);
    let output = directory.path().join("schema13-human");
    let capabilities = BTreeSet::from([
        HumanCapabilityV1::ReadBrokerRegistry,
        HumanCapabilityV1::ReadWorkerRegistry,
    ]);
    provision(ProvisionArgs {
        offline: offline(&source),
        name: "schema13-human".into(),
        organization: "fixture-organization".into(),
        output_dir: output.clone(),
        capabilities: vec![
            super::super::Capability::ReadBrokerRegistry,
            super::super::Capability::ReadWorkerRegistry,
        ],
    })
    .unwrap();
    let credential_bytes = std::fs::read(output.join("credential")).unwrap();
    let completion: Completion =
        serde_json::from_slice(&std::fs::read(output.join("completion.json")).unwrap()).unwrap();
    assert_eq!(completion.receipts.len(), 3);
    let store = Store::open_existing_for_tenant(&source, "tenant-a", Redactor::new(vec![]), 10_000)
        .unwrap();
    assert_eq!(store.head().unwrap(), before + 3);
    let secret = super::super::http::credential_text(&credential_bytes).unwrap();
    let digest: [u8; 32] = Sha256::digest(secret.as_bytes()).into();
    let identity = store
        .authenticate_local_digest(&digest)
        .unwrap()
        .expect("actual schema13 credential must authenticate");
    let profile = store
        .human_profile(&identity.principal, &Redactor::new(vec![secret.to_owned()]))
        .unwrap();
    assert_eq!(profile.account.unwrap().principal, identity.principal);
    assert_eq!(profile.memberships.len(), 1);
    assert_eq!(profile.memberships[0].tenant_id, "tenant-a");
    assert_eq!(profile.memberships[0].capabilities, capabilities);
    assert!(profile.authorization_revision.is_some());
    let after_audited_read = store.head().unwrap();
    assert!(after_audited_read > before + 3);
    drop(store);
    resume(ResumeProvisionArgs {
        offline: offline(&source),
        output_dir: output.clone(),
    })
    .unwrap();
    let replay: Completion =
        serde_json::from_slice(&std::fs::read(output.join("completion.json")).unwrap()).unwrap();
    assert_eq!(replay.receipts, completion.receipts);
    assert_eq!(
        std::fs::read(output.join("credential")).unwrap(),
        credential_bytes
    );
    let store = Store::open_existing_for_tenant(&source, "tenant-a", Redactor::new(vec![]), 10_000)
        .unwrap();
    assert_eq!(store.head().unwrap(), after_audited_read);
    assert_eq!(store.supervision_state().unwrap().schema_version, 13);
    assert!(!store.supervision_state().unwrap().active);
    assert!(store.integrity().unwrap().valid);
}
