use super::*;

#[test]
fn activation_is_closed_and_does_not_accept_unpublished_mode() {
    let value = serde_json::json!({"version":1,"public_digest_version":1,"operation_id":Uuid::from_u128(1),"preparation_directory":{"path":"/private/preparation","device":1,"inode":2},"preparation_seal":"preparation-complete.json","manifest":"/private/preparation/journal/migration-00000000-0000-0000-0000-000000000001-closure.json","tenant_registry_path":"/private/registry.json","technical_credentials_envelope":"technical-credentials.json","oidc_configuration_path":null,"control_config_revision":1,"oidc_mapping_revision":null,"max_events":100000,"human_source_mode":"owned_sources"});
    let parsed: HumanSourcesActivationV1 =
        decode_closed(&serde_json::to_vec(&value).unwrap(), 32768, 515).unwrap();
    assert_eq!(parsed.max_events, 100000);
    let mut unknown = value.clone();
    unknown["implicit_legacy_fallback"] = serde_json::json!(true);
    assert!(
        decode_closed::<HumanSourcesActivationV1>(
            &serde_json::to_vec(&unknown).unwrap(),
            32768,
            515
        )
        .is_err()
    );
    let mut missing = value;
    missing
        .as_object_mut()
        .unwrap()
        .remove("oidc_configuration_path");
    assert!(
        decode_closed::<HumanSourcesActivationV1>(
            &serde_json::to_vec(&missing).unwrap(),
            32768,
            515
        )
        .is_err()
    );
}

#[test]
fn check_preserves_directory_without_recovery_and_never_opens_sqlite() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = dir.path().canonicalize().unwrap();
    assert_eq!(check(&path), CheckStatus::Conflict);
    assert_eq!(std::fs::read_dir(&path).unwrap().count(), 0);
}

#[test]
fn cli_refuses_missing_stop_attestation_before_preparation() {
    let args = HumanSourcesArgs {
        command: HumanSourcesCommand::ResumePrepare(StoppedPreparationArgs {
            preparation_dir: PathBuf::from("/private/never-created"),
            confirm_control_stopped: false,
        }),
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    assert!(runtime.block_on(run(args)).is_err());
}

fn real_legacy_fixture() -> (tempfile::TempDir, PrepareArgs, PathBuf) {
    use std::{
        fs,
        io::Write,
        os::unix::fs::{OpenOptionsExt, PermissionsExt},
    };
    let temporary = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let source_parent = root.join("source");
    fs::create_dir(&source_parent).unwrap();
    fs::set_permissions(&source_parent, fs::Permissions::from_mode(0o700)).unwrap();
    let history = root.join("history");
    fs::create_dir(&history).unwrap();
    fs::set_permissions(&history, fs::Permissions::from_mode(0o700)).unwrap();
    let data = source_parent.join("events.sqlite");
    let store = agentsandbox::audit::Store::open_for_tenant(
        &data,
        "tenant-a",
        Redactor::new(vec![]),
        100_000,
    )
    .unwrap();
    use agentsandbox::audit::Store;
    for (i, migrate) in [
        Store::migrate_control_v3,
        Store::migrate_identity_v4,
        Store::migrate_human_access_v5,
        Store::migrate_workload_v6,
        Store::migrate_human_brokers_v7,
        Store::migrate_kubernetes_create_v8,
        Store::migrate_kubernetes_policy_v9,
        Store::migrate_human_workers_v10,
        Store::migrate_kubernetes_session_reservations_v11,
        Store::migrate_broker_checkpoints_v12,
        Store::migrate_kubernetes_recovery_v13,
    ]
    .into_iter()
    .enumerate()
    {
        migrate(&store, &history.join(format!("before-{}.sqlite", i + 3))).unwrap();
    }
    assert_eq!(store.supervision_state().unwrap().schema_version, 13);
    drop(store);
    let write = |path: &Path, bytes: &[u8]| {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .unwrap();
        f.write_all(bytes).unwrap();
        f.sync_all().unwrap();
    };
    let runner = root.join("runner.token");
    let viewer = root.join("viewer.token");
    write(&runner, b"synthetic-runner-credential-long-enough");
    write(&viewer, b"synthetic-viewer-credential-long-enough");
    let registry = root.join("registry.json");
    write(&registry,&serde_json::to_vec(&serde_json::json!({"version":1,"tenants":[{"tenant_id":"tenant-a","data":data,"runner_token_file":runner,"viewer_token_file":viewer}]})).unwrap());
    let args = PrepareArgs {
        tenant_registry: registry,
        installation_id: Uuid::new_v4(),
        control_config_revision: 7,
        output_dir: root.join("prepared"),
        confirm_control_stopped: true,
        oidc_config: None,
        oidc_mapping_revision: None,
        max_events: 100_000,
    };
    (temporary, args, data)
}

#[test]
fn interrupted_preparation_keeps_ids_and_changed_input_refuses_before_first_envelope() {
    let (_temporary, args, _data) = real_legacy_fixture();
    let directory = args.output_dir.clone();
    preparation::FAILURE_POINT.with(|p| p.set(Some("recovery.json")));
    assert!(preparation::prepare(args).is_err());
    let recovery = std::fs::read(directory.join("recovery.json")).unwrap();
    assert!(!directory.join("technical-credentials.json").exists());
    let runner = directory.parent().unwrap().join("runner.token");
    let token = std::fs::read(&runner).unwrap();
    std::fs::write(&runner, b"synthetic-CHANGED-credential-long-enough").unwrap();
    assert!(preparation::resume_prepare(&directory).is_err());
    assert!(!directory.join("technical-credentials.json").exists());
    std::fs::write(&runner, token).unwrap();
    for name in [
        "technical-credentials.json",
        "maintenance-plan.json",
        "preparation-complete.json",
    ] {
        preparation::FAILURE_POINT.with(|p| p.set(Some(name)));
        assert!(preparation::resume_prepare(&directory).is_err());
        assert_eq!(
            std::fs::read(directory.join("recovery.json")).unwrap(),
            recovery
        );
    }
    let envelope = std::fs::read(directory.join("technical-credentials.json")).unwrap();
    preparation::resume_prepare(&directory).unwrap();
    assert_eq!(
        std::fs::read(directory.join("technical-credentials.json")).unwrap(),
        envelope
    );
    assert_eq!(check(&directory), CheckStatus::PreparedFilesValid);
}

#[tokio::test]
async fn activation_publication_interruptions_replay_coordinator_then_final_ack_is_files_only() {
    let (_temporary, args, data) = real_legacy_fixture();
    let directory = args.output_dir.clone();
    preparation::prepare(args).unwrap();
    for name in ["manifest", "activation.json", "activation-complete.json"] {
        preparation::FAILURE_POINT.with(|p| p.set(Some(name)));
        assert!(activation::apply(&directory, false).await.is_err());
    }
    let revision = || {
        let db = rusqlite::Connection::open(&data).unwrap();
        db.query_row(
            "SELECT incarnation_revision FROM review_authority_meta WHERE singleton=1",
            [],
            |r| r.get::<_, u64>(0),
        )
        .unwrap()
    };
    let before = revision();
    assert_eq!(
        activation::apply(&directory, true).await.unwrap().0,
        "already_published_files"
    );
    assert_eq!(
        revision(),
        before,
        "lost final ACK must not create another incarnation"
    );
    assert_eq!(check(&directory), CheckStatus::ActivationFilesValid);
}
