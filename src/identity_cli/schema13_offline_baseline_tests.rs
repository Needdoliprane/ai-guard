//! Real offline provisioning across a schema12 migration. No provider or HTTP.
use super::*;
use std::os::unix::fs::PermissionsExt;

fn schema11(path: &Path) -> Store {
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
}

fn args(source: &Path, output: &Path, name: &str) -> ProvisionArgs {
    ProvisionArgs {
        offline: OfflineArgs {
            data: source.to_owned(),
            tenant_id: "tenant-a".into(),
            confirm_control_stopped: true,
            max_events: 10_000,
            audit_capacity_file: None,
        },
        name: name.into(),
        organization: "fixture-organization".into(),
        output_dir: output.to_owned(),
        capabilities: vec![
            super::super::Capability::ReadBrokerRegistry,
            super::super::Capability::ReadWorkerRegistry,
        ],
    }
}

#[test]
fn schema12_offline_provisioning_preserves_both_inherited_registry_permissions() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let source = directory.path().join("tenant.sqlite");
    drop(schema11(&source));
    let canary_output = directory.path().join("schema11-canary");
    provision(args(&source, &canary_output, "schema11-canary")).unwrap();
    let canary: Completion =
        serde_json::from_slice(&std::fs::read(canary_output.join("completion.json")).unwrap())
            .unwrap();
    assert_eq!(canary.receipts.len(), 3);
    let store = Store::open_existing_for_tenant(&source, "tenant-a", Redactor::new(vec![]), 10_000)
        .unwrap();
    assert_eq!(store.supervision_state().unwrap().schema_version, 11);
    assert!(!store.supervision_state().unwrap().active);
    store
        .migrate_broker_checkpoints_v12(&source.with_extension("before12"))
        .unwrap();
    assert!(store.integrity().unwrap().valid);
    let before = store.head().unwrap();
    drop(store);
    let output = directory.path().join("schema12-human");
    eprintln!(
        "schema12_provision_ready: schema11 provisioning with both registry permissions passed, real migration12 valid and inactive"
    );
    provision(args(&source, &output, "schema12-human")).unwrap_or_else(|error| {
        panic!("schema12 must provision inherited broker and worker registry permissions: {error}")
    });
    let completion: Completion =
        serde_json::from_slice(&std::fs::read(output.join("completion.json")).unwrap()).unwrap();
    assert_eq!(completion.receipts.len(), 3);
    let recovery: Recovery =
        serde_json::from_slice(&std::fs::read(output.join("recovery.json")).unwrap()).unwrap();
    let IdentityChangeV1::Membership { membership, .. } = &recovery.provision.mutations[2].change
    else {
        panic!("the real provisioner must retain its exact membership operation");
    };
    assert_eq!(membership.tenant_id, "tenant-a");
    assert_eq!(
        membership.capabilities,
        BTreeSet::from([
            HumanCapabilityV1::ReadBrokerRegistry,
            HumanCapabilityV1::ReadWorkerRegistry,
        ])
    );
    let store = Store::open_existing_for_tenant(&source, "tenant-a", Redactor::new(vec![]), 10_000)
        .unwrap();
    assert_eq!(store.head().unwrap(), before + 3);
    assert_eq!(store.supervision_state().unwrap().schema_version, 12);
    assert!(!store.supervision_state().unwrap().active);
    assert!(store.integrity().unwrap().valid);
    assert_eq!(
        std::fs::metadata(output.join("credential"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[test]
fn schema12_offline_broker_only_provisioning_reaches_its_own_registry_guard() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let source = directory.path().join("tenant.sqlite");
    drop(schema11(&source));
    let canary_output = directory.path().join("schema11-broker-canary");
    let mut canary_args = args(&source, &canary_output, "schema11-broker-canary");
    canary_args.capabilities = vec![super::super::Capability::ReadBrokerRegistry];
    provision(canary_args).unwrap();
    let canary: Completion =
        serde_json::from_slice(&std::fs::read(canary_output.join("completion.json")).unwrap())
            .unwrap();
    assert_eq!(canary.receipts.len(), 3);
    let store = Store::open_existing_for_tenant(&source, "tenant-a", Redactor::new(vec![]), 10_000)
        .unwrap();
    assert_eq!(store.supervision_state().unwrap().schema_version, 11);
    assert!(!store.supervision_state().unwrap().active);
    store
        .migrate_broker_checkpoints_v12(&source.with_extension("before12"))
        .unwrap();
    assert!(store.integrity().unwrap().valid);
    let before = store.head().unwrap();
    drop(store);
    let output = directory.path().join("schema12-broker-human");
    let mut broker_args = args(&source, &output, "schema12-broker-human");
    broker_args.capabilities = vec![super::super::Capability::ReadBrokerRegistry];
    eprintln!(
        "schema12_broker_provision_ready: schema11 broker-only provisioning passed, real migration12 valid and inactive, no worker permission requested"
    );
    provision(broker_args).unwrap_or_else(|error| {
        panic!("schema12 must provision its inherited broker-only registry permission: {error}")
    });
    let completion: Completion =
        serde_json::from_slice(&std::fs::read(output.join("completion.json")).unwrap()).unwrap();
    assert_eq!(completion.receipts.len(), 3);
    let recovery: Recovery =
        serde_json::from_slice(&std::fs::read(output.join("recovery.json")).unwrap()).unwrap();
    let IdentityChangeV1::Membership { membership, .. } = &recovery.provision.mutations[2].change
    else {
        panic!("the real provisioner must retain its exact membership operation");
    };
    assert_eq!(membership.tenant_id, "tenant-a");
    assert_eq!(
        membership.capabilities,
        BTreeSet::from([HumanCapabilityV1::ReadBrokerRegistry])
    );
    let store = Store::open_existing_for_tenant(&source, "tenant-a", Redactor::new(vec![]), 10_000)
        .unwrap();
    assert_eq!(store.head().unwrap(), before + 3);
    assert_eq!(store.supervision_state().unwrap().schema_version, 12);
    assert!(!store.supervision_state().unwrap().active);
    assert!(store.integrity().unwrap().valid);
}
