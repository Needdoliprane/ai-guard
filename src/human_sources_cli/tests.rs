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

#[test]
fn identity_marker_cli_requires_one_complete_source_route() {
    use clap::Parser;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        sources: HumanSourcesArgs,
    }

    let activation = ["--human-sources-activation", "/private/activation.json"];
    let explicit = [
        "--human-sources-manifest",
        "/private/closure.json",
        "--human-credentials-envelope",
        "/private/envelope.json",
        "--tenants-file",
        "/private/tenants.json",
        "--human-control-config-revision",
        "7",
    ];
    for command in ["install", "resume"] {
        let parse = |options: &[&str]| {
            Cli::try_parse_from(
                [
                    "agentsandbox",
                    "human-rights",
                    command,
                    "--tenant-id",
                    "tenant-a",
                    "--maintenance-dir",
                    "/private/maintenance",
                    "--confirm-control-stopped",
                ]
                .into_iter()
                .chain(options.iter().copied()),
            )
        };
        assert!(parse(&activation).is_ok());
        assert!(parse(&explicit).is_ok());
        assert!(parse(&[]).is_err());
        assert!(parse(&explicit[..6]).is_err());
        assert!(parse(&[activation.as_slice(), explicit.as_slice()].concat()).is_err());
        assert!(
            parse(
                &[
                    explicit.as_slice(),
                    &["--oidc-config", "/private/oidc.json"]
                ]
                .concat()
            )
            .is_err()
        );
    }
}

#[test]
fn identity_marker_inputs_validate_explicit_store15_without_opening_sqlite() {
    use crate::gateway_config::{
        human_sources::{public_control_digest_v1, resolved_control},
        load_owned_tenants,
    };
    use activation::{ExplicitOwnedSourcesArgs, load_explicit_identity_marker_inputs};
    use agentsandbox::human_authority::{
        HumanAuthorityMarkerReceiptV1, HumanSourceDescriptorV1, HumanStorageProfileV1,
        startup::observe_human_storage_binding,
    };
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
    let write = |name: &str, bytes: &[u8]| {
        let path = root.join(name);
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap()
            .write_all(bytes)
            .unwrap();
        path
    };

    let sentinel = b"file binding fixture";
    let source = write("events.sqlite", sentinel);
    let runner = "synthetic-runner-credential-long-enough";
    let viewer = "synthetic-viewer-credential-long-enough";
    let runner_file = write("runner.token", runner.as_bytes());
    let viewer_file = write("viewer.token", viewer.as_bytes());
    let registry = write(
        "tenants.json",
        &serde_json::to_vec(&serde_json::json!({
            "version": 1,
            "tenants": [{
                "tenant_id": "tenant-a",
                "data": source,
                "runner_token_file": runner_file,
                "viewer_token_file": viewer_file,
            }],
        }))
        .unwrap(),
    );
    let tenants = load_owned_tenants(&registry).unwrap();
    let max_events = 100_000;
    let revision = 7;
    let public_digest =
        public_control_digest_v1(&resolved_control(revision, max_events, &tenants, None).unwrap())
            .unwrap();
    let installation_id = Uuid::new_v4();
    let closure_id = Uuid::new_v4();
    let credential_inputs = [
        TechnicalCredentialInput {
            tenant_id: "tenant-a",
            role: TechnicalCredentialRoleV1::Runner,
            credential_id: Uuid::new_v4(),
            generation: 1,
            token: runner,
        },
        TechnicalCredentialInput {
            tenant_id: "tenant-a",
            role: TechnicalCredentialRoleV1::Viewer,
            credential_id: Uuid::new_v4(),
            generation: 1,
            token: viewer,
        },
    ];
    let envelope_for = |closure_revision| {
        let expected = EnvelopeExpectationsV1::from_configured_inputs(
            installation_id,
            closure_id,
            closure_revision,
            public_digest.clone(),
            &["tenant-a".to_owned()],
            &credential_inputs,
        )
        .unwrap();
        prepare_envelope_configuration(&expected, &credential_inputs).unwrap()
    };
    let envelope = write("envelope.json", &envelope_for(2));
    let mut closure = HumanAuthorityClosureV1 {
        version: 1,
        installation_id,
        closure_id,
        closure_revision: 2,
        closure_digest: String::new(),
        sources: vec![HumanSourceDescriptorV1 {
            source_id: Uuid::new_v4(),
            kind: HumanSourceKindV1::TenantStore,
            installation_id,
            durable_namespace_id: Uuid::new_v4(),
            tenant_id: Some("tenant-a".to_owned()),
            storage_binding: observe_human_storage_binding(&source).unwrap(),
            required_format: 15,
            authority_marker_receipt: HumanAuthorityMarkerReceiptV1 {
                seq: 1,
                hash: "ab".repeat(32),
            },
        }],
        configuration: HumanAuthorityConfigurationV1 {
            control_config_revision: revision,
            public_config_digest: public_digest.clone(),
            oidc: None,
            files: [
                (
                    HumanConfigurationFileKindV1::ControlConfiguration,
                    &registry,
                ),
                (
                    HumanConfigurationFileKindV1::TechnicalCredentials,
                    &envelope,
                ),
            ]
            .into_iter()
            .map(|(kind, path)| HumanConfigurationFileV1 {
                kind,
                storage_binding: observe_human_storage_binding(path).unwrap(),
            })
            .collect(),
        },
        supported_storage_profile: HumanStorageProfileV1::LocalPosixFlockV1,
    };
    closure.closure_digest = closure.compute_digest().unwrap();
    let manifest_bytes = serde_json::to_vec(&closure).unwrap();
    let args = ExplicitOwnedSourcesArgs {
        human_sources_manifest: Some(write("closure.json", &manifest_bytes)),
        human_credentials_envelope: Some(envelope.clone()),
        tenants_file: Some(registry),
        human_control_config_revision: Some(revision),
        oidc_config: None,
        human_oidc_mapping_revision: None,
        max_events: Some(max_events),
    };
    let loaded = load_explicit_identity_marker_inputs(&args).unwrap();
    assert_eq!(loaded.manifest.bytes(), manifest_bytes.as_slice());
    assert_eq!(loaded.max_events, max_events);
    assert_eq!(loaded.excluded_directories, vec![root.clone()]);
    drop(loaded);

    fs::write(&envelope, envelope_for(1)).unwrap();
    assert!(load_explicit_identity_marker_inputs(&args).is_err());
    fs::write(&envelope, envelope_for(2)).unwrap();
    fs::write(
        &runner_file,
        b"synthetic-renewed-runner-credential-long-enough",
    )
    .unwrap();
    assert!(load_explicit_identity_marker_inputs(&args).is_err());

    assert_eq!(fs::read(source).unwrap().as_slice(), sentinel);
    assert_eq!(fs::read_dir(root).unwrap().count(), 6);
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
            |r| r.get::<_, i64>(0),
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

#[test]
fn check_preserves_main_sidecars_and_all_preparation_documents() {
    use std::os::unix::fs::MetadataExt;
    let (_temporary, args, data) = real_legacy_fixture();
    let directory = args.output_dir.clone();
    preparation::prepare(args).unwrap();
    let snapshot = || {
        let mut paths = std::fs::read_dir(&directory)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.is_file())
            .collect::<Vec<_>>();
        paths.extend([
            data.clone(),
            PathBuf::from(format!("{}-wal", data.display())),
            PathBuf::from(format!("{}-shm", data.display())),
        ]);
        paths.sort();
        paths
            .into_iter()
            .map(|path| {
                let state = std::fs::metadata(&path).ok().map(|m| {
                    (
                        m.dev(),
                        m.ino(),
                        m.mtime(),
                        m.mtime_nsec(),
                        std::fs::read(&path).unwrap(),
                    )
                });
                (path, state)
            })
            .collect::<Vec<_>>()
    };
    let before = snapshot();
    assert_eq!(check(&directory), CheckStatus::PreparedFilesValid);
    assert_eq!(
        snapshot(),
        before,
        "check must preserve source/sidecar bytes and must not change/publish files"
    );
}

fn real_legacy_oidc_fixture() -> (tempfile::TempDir, PrepareArgs, PathBuf) {
    use agentsandbox::audit::authentication::{
        AuthenticationAudit, AuthenticationAuditPolicyV1, AuthenticationCapacityPlanV1,
    };
    use std::{
        fs,
        io::Write,
        os::unix::fs::{OpenOptionsExt, PermissionsExt},
    };
    let (temporary, mut args, data) = real_legacy_fixture();
    let root = args.output_dir.parent().unwrap();
    let auth_parent = root.join("authentication");
    fs::create_dir(&auth_parent).unwrap();
    fs::set_permissions(&auth_parent, fs::Permissions::from_mode(0o700)).unwrap();
    let auth_path = auth_parent.join("audit.sqlite");
    let ca = root.join("public-ca.pem");
    let write = |path: &Path, bytes: &[u8]| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .unwrap();
        file.write_all(bytes).unwrap();
        file.sync_all().unwrap();
    };
    write(
        &ca,
        include_bytes!("../../tests/diagnose_support/fixtures/public-test-root.pem"),
    );
    let mut config: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../../docs/setup/sso/examples/gateway.https.json"
    ))
    .unwrap();
    config["authentication_audit"]["path"] = serde_json::json!(auth_path);
    config["authentication_audit"]["installation_id"] = serde_json::json!(args.installation_id);
    config["authentication_audit"]["capacity_plan"]["tenants"]
        .as_array_mut()
        .unwrap()
        .retain(|tenant| tenant["tenant_id"] == "tenant-a");
    config["ca_bundle_file"] = serde_json::json!(ca);
    let policy: AuthenticationAuditPolicyV1 =
        serde_json::from_value(config["authentication_audit"]["policy"].clone()).unwrap();
    let plan: AuthenticationCapacityPlanV1 =
        serde_json::from_value(config["authentication_audit"]["capacity_plan"].clone()).unwrap();
    let audit = AuthenticationAudit::init_new_v4(
        &auth_path,
        args.installation_id,
        policy,
        plan,
        Redactor::new(vec![]),
    )
    .unwrap();
    assert_eq!(audit.format_version().unwrap(), 4);
    drop(audit);
    let path = root.join("oidc.json");
    write(&path, &serde_json::to_vec(&config).unwrap());
    args.oidc_config = Some(path);
    args.oidc_mapping_revision = Some(9);
    (temporary, args, data)
}

#[tokio::test]
async fn final_ack_input_resync_failure_preserves_published_ids_and_commits_then_same_retry_succeeds()
 {
    use std::{
        fs,
        io::Write,
        os::unix::fs::{OpenOptionsExt, PermissionsExt},
    };
    for oidc in [false, true] {
        let (_temporary, args, data) = if oidc {
            real_legacy_oidc_fixture()
        } else {
            real_legacy_fixture()
        };
        let directory = args.output_dir.clone();
        let root = directory.parent().unwrap().to_owned();
        preparation::prepare(args).unwrap();
        activation::apply(&directory, false).await.unwrap();
        let snapshot = || {
            let mut files = walkdir::WalkDir::new(&directory)
                .into_iter()
                .map(|entry| entry.unwrap())
                .filter(|entry| entry.file_type().is_file())
                .map(|entry| entry.path().to_owned())
                .collect::<Vec<_>>();
            files.push(data.clone());
            if oidc {
                files.push(root.join("authentication/audit.sqlite"));
            }
            files.sort();
            files
                .into_iter()
                .map(|path| {
                    let bytes = fs::read(&path).unwrap();
                    (path, bytes)
                })
                .collect::<Vec<_>>()
        };
        let before = snapshot();
        let token = root.join("runner.token");
        let preserved = root.join("preserved-runner.token");
        let replacement_saved = root.join("divergent-runner.token");
        let token_for_hook = token.clone();
        let preserved_for_hook = preserved.clone();
        let _ = activation::BEFORE_FINAL_ACK_RESYNC.with(|hook| {
            hook.replace(Some(Box::new(move || {
                // Real private-file replacement after the full activation/input
                // validation. The retained original FD must refuse the new inode.
                let bytes = fs::read(&token_for_hook).unwrap();
                fs::rename(&token_for_hook, &preserved_for_hook).unwrap();
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&token_for_hook)
                    .unwrap();
                file.write_all(&bytes).unwrap();
                file.sync_all().unwrap();
            })))
        });
        assert!(
            activation::apply(&directory, true).await.is_err(),
            "an input resync refusal must not acknowledge already_published_files"
        );
        assert_eq!(
            snapshot(),
            before,
            "all published IDs, journal/backups and committed sources remain unchanged"
        );
        assert_eq!(check(&directory), CheckStatus::Conflict);
        // Explicit synthetic-fixture reconciliation preserves both original
        // and divergent artifacts; no product file is removed or overwritten.
        fs::rename(&token, &replacement_saved).unwrap();
        fs::rename(&preserved, &token).unwrap();
        assert_eq!(
            activation::apply(&directory, true).await.unwrap().0,
            "already_published_files"
        );
        assert_eq!(
            snapshot(),
            before,
            "a final ACK retry must not rerun migration/startup or rotate IDs"
        );
        assert_eq!(
            fs::metadata(&token).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
