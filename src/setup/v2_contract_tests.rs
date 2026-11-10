//! Builder, closed input and retained bootstrap evidence on real local files.
use super::*;
use std::sync::Arc;

fn public_gateway(root: &Path, preparation: &Preparation) -> crate::api::GatewayState {
    let tenants = preparation
        .manifest
        .tenants
        .iter()
        .map(|tenant| crate::api::TenantGateway {
            tenant_id: tenant.tenant_id.clone(),
            store: Arc::new(
                crate::audit::Store::open_existing_for_tenant(
                    &root
                        .join("tenants")
                        .join(&tenant.tenant_id)
                        .join("audit.sqlite"),
                    &tenant.tenant_id,
                    Redactor::new(vec![]),
                    MAX_EVENTS,
                )
                .unwrap(),
            ),
            runner_token: fs::read_to_string(
                root.join("tenants").join(&tenant.tenant_id).join("runner"),
            )
            .unwrap()
            .trim()
            .into(),
            viewer_token: fs::read_to_string(
                root.join("tenants").join(&tenant.tenant_id).join("viewer"),
            )
            .unwrap()
            .trim()
            .into(),
        })
        .collect();
    crate::api::GatewayState::for_tenants(
        tenants,
        crate::policy::Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .unwrap()
    .with_oidc(
        crate::api::OidcGateway::new(
            preparation.manifest.gateway.clone(),
            Arc::new(audit(root, preparation)),
            preparation.manifest.ca_bundle_pem.clone(),
        )
        .unwrap(),
    )
    .unwrap()
}

fn interrupted(
    root: &Path,
    cut: installation::AuthenticationFault,
) -> (std::path::PathBuf, Preparation) {
    installation::AUTHENTICATION_FAULT.with(|hook| hook.set(Some(cut)));
    assert!(storage::prepare(manifest(), &root.join("installation")).is_err());
    let stage = fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.is_dir())
        .unwrap();
    let preparation =
        serde_json::from_slice(&fs::read(stage.join("preparation.json")).unwrap()).unwrap();
    (stage, preparation)
}

fn synthetic_intention(ledger: &AuthenticationAudit, input: &SetupManifestV1, tenant: &str) {
    use crate::audit::authentication::*;
    // Declared verifier input for a ledger-format test, not proof of a real
    // JWT, membership lookup, provider response or human instruction.
    let principal = input.tenants[0].identities[0].principal.clone();
    ledger
        .begin_verified_initial_scoped(
            AuthenticationRecordV1 {
                operation_id: Uuid::new_v4(),
                action: AuthenticationActionV1::SessionAdmission,
                phase: AuthenticationPhaseV1::Intention,
                issuer: input.gateway.provider.issuer.clone(),
                client_id: input.gateway.provider.cli_client_id.clone(),
                principal: None,
                transaction_id: None,
                session_id: Some(Uuid::new_v4()),
                causal: None,
            },
            principal.clone(),
            AuthenticationTokenFingerprintV1::access_token("synthetic-public-format-proof"),
            AuthenticationTenantCandidatesV1::from_verified_memberships(
                principal,
                vec![tenant.into()],
            )
            .unwrap(),
        )
        .unwrap();
}

#[test]
fn v1_mono_tenant_public_builder_retains_genesis_one() {
    let root = parent();
    let output = root.path().join("legacy");
    let input = legacy();
    assert_eq!(input.tenants.len(), 1);
    storage::prepare(input, &output).unwrap();
    let preparation =
        serde_json::from_slice(&fs::read(output.join("preparation.json")).unwrap()).unwrap();
    drop(public_gateway(&output, &preparation));
    assert_eq!(audit(&output, &preparation).format_version().unwrap(), 3);
    assert_eq!(audit(&output, &preparation).verify().unwrap().records, 1);
}

#[test]
fn v2_multi_tenant_public_builder_accepts_exact_generated_plan_and_config() {
    let root = parent();
    let output = root.path().join("multi");
    let mut input = manifest();
    let mut second = input.tenants[0].clone();
    second.tenant_id = "second-tenant".into();
    second.organization_id = "second-organization".into();
    input.tenants.push(second);
    input
        .capacity_plan
        .tenants
        .push(AuthenticationTenantCapacityV1 {
            tenant_id: "second-tenant".into(),
            capacity: AuthenticationCapacityV1 {
                max_records: 1000,
                max_serialized_bytes: 1_000_000,
            },
        });
    let plan = input.capacity_plan.clone();
    storage::prepare(input, &output).unwrap();
    let preparation: Preparation =
        serde_json::from_slice(&fs::read(output.join("preparation.json")).unwrap()).unwrap();
    assert_eq!(preparation.version, 2);
    let cfg: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join("oidc.json")).unwrap()).unwrap();
    assert_eq!(cfg["version"], 2);
    assert_eq!(
        cfg["authentication_audit"]["capacity_plan"],
        serde_json::to_value(&plan).unwrap()
    );
    let ledger = audit(&output, &preparation);
    assert_eq!(ledger.capacity_plan().unwrap(), Some(plan));
    assert_eq!(ledger.verify().unwrap().records, 2);
    assert_eq!(ledger.bootstrap_receipt_v4().unwrap().sequence, 2);
    drop(ledger);
    drop(public_gateway(&output, &preparation));
    storage::resume(&output).unwrap();
    assert_eq!(audit(&output, &preparation).verify().unwrap().records, 2);
}

#[test]
fn v2_wire_rejects_missing_plan_unknown_fields_and_version_mixing() {
    for case in 0..4 {
        let root = parent();
        let input_file = root.path().join("input.json");
        let output = root.path().join("installation");
        let mut value = serde_json::to_value(manifest()).unwrap();
        match case {
            0 => {
                value.as_object_mut().unwrap().remove("capacity_plan");
            }
            1 => {
                value["unexpected"] = true.into();
            }
            2 => {
                value["version"] = 1.into();
            }
            _ => {
                value["capacity_plan"]["unexpected"] = true.into();
            }
        }
        fs::write(&input_file, serde_json::to_vec(&value).unwrap()).unwrap();
        fs::set_permissions(&input_file, fs::Permissions::from_mode(0o600)).unwrap();
        let before = entries(root.path());
        assert!(
            execute(SetupArgs {
                command: SetupCommand::Prepare(PrepareArgs {
                    input: input_file,
                    output_dir: output.clone(),
                    demo_defaults: false
                })
            })
            .is_err()
        );
        assert_eq!(entries(root.path()), before);
        assert!(!output.exists());
    }
}

#[test]
fn sqlite_witness_mismatch_refuses_before_any_bootstrap_entry() {
    use crate::{audit::authentication::AuthenticationStorageIdentityV1, control_fs::Directory};
    use std::ffi::OsStr;
    let root = parent();
    let directory = Directory::open_existing(root.path()).unwrap();
    for name in ["a.sqlite", "b.sqlite"] {
        let file = directory.create_private_entry(OsStr::new(name)).unwrap();
        file.sync_all().unwrap();
    }
    directory.sync().unwrap();
    let expected_a = AuthenticationStorageIdentityV1::from_checked_entry(
        directory
            .private_entry(OsStr::new("a.sqlite"))
            .unwrap()
            .unwrap(),
    );
    let input = manifest();
    let install = Uuid::new_v4();
    let namespace = Uuid::new_v4();
    let path = root.path().join("b.sqlite");
    assert!(
        AuthenticationAudit::initialize_prepared_v4(
            &path,
            install,
            namespace,
            input.authentication_policy.clone(),
            input.capacity_plan.clone(),
            Redactor::new(vec![]),
            |actual| {
                ensure!(actual == expected_a, "expected A is not B");
                Ok(())
            }
        )
        .is_err()
    );
    let db =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM sqlite_schema", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        db.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        db.pragma_query_value(None, "application_id", |row| row.get::<_, i64>(0))
            .unwrap(),
        0
    );
    drop(db);
    let expected_b = AuthenticationStorageIdentityV1::from_checked_entry(
        directory
            .private_entry(OsStr::new("b.sqlite"))
            .unwrap()
            .unwrap(),
    );
    let audit = AuthenticationAudit::initialize_prepared_v4(
        &path,
        install,
        namespace,
        input.authentication_policy,
        input.capacity_plan,
        Redactor::new(vec![]),
        |actual| {
            ensure!(actual == expected_b, "original B witness");
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(audit.verify().unwrap().records, 2);
}

#[test]
fn orphan_bootstrap_substitution_activity_or_missing_binding_never_gets_adopted() {
    for case in 0..4 {
        let root = parent();
        let (stage, preparation) = interrupted(
            root.path(),
            installation::AuthenticationFault::AfterBootstrap,
        );
        let path = stage.join("authentication.sqlite");
        let before = audit(&stage, &preparation).verify().unwrap();
        let retained_identity = storage::NativeIdentity::entry(
            &crate::control_fs::Directory::open_existing(&stage).unwrap(),
            "authentication.sqlite",
        )
        .unwrap();
        let original = fs::read(&path).unwrap();
        fs::write(stage.join("original-bootstrap-evidence"), &original).unwrap();
        fs::set_permissions(
            stage.join("original-bootstrap-evidence"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        match case {
            0 => {
                fs::rename(&path, stage.join("original-inode.sqlite")).unwrap();
                fs::write(&path, &original).unwrap();
                fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            }
            1 => {
                let other = stage.join("different-namespace.sqlite");
                let foreign = AuthenticationAudit::init_new_v4(
                    &other,
                    preparation.installation_id,
                    preparation.manifest.authentication_policy.clone(),
                    preparation.manifest.capacity_plan.clone().unwrap(),
                    Redactor::new(vec![]),
                )
                .unwrap();
                assert_ne!(foreign.verify().unwrap().namespace_id, before.namespace_id);
                drop(foreign);
                // Same-inode overwrite of a synthetic fixture only; original
                // bytes are retained above for this negative control.
                fs::copy(other, &path).unwrap();
                assert!(
                    storage::NativeIdentity::entry(
                        &crate::control_fs::Directory::open_existing(&stage).unwrap(),
                        "authentication.sqlite"
                    )
                    .unwrap()
                        == retained_identity,
                    "this case changes the valid namespace while retaining the same synthetic inode"
                );
            }
            2 => {
                let ledger = audit(&stage, &preparation);
                synthetic_intention(
                    &ledger,
                    &preparation.manifest,
                    &preparation.manifest.tenants[0].tenant_id,
                );
                assert_eq!(ledger.verify().unwrap().records, 3);
            }
            _ => {
                fs::rename(
                    stage.join("authentication-creation.json"),
                    stage.join("retained-creation-evidence.json"),
                )
                .unwrap();
            }
        }
        let expected = audit(&stage, &preparation).verify().unwrap();
        assert!(storage::resume(&stage).is_err());
        assert_eq!(audit(&stage, &preparation).verify().unwrap(), expected);
        assert!(!stage.join("authentication-origin.json").exists());
        assert!(!root.path().join("installation").exists());
        assert_eq!(
            fs::read(stage.join("original-bootstrap-evidence")).unwrap(),
            original
        );
    }
}

#[test]
fn bootstrap_receipt_is_exact_after_activity_and_rejects_a_migrated_activity_at_two() {
    use crate::audit::authentication::{
        AuthenticationActionV1, AuthenticationAuditFailureV1, AuthenticationPhaseV1,
        AuthenticationRecordV1,
    };
    let root = parent();
    let input = manifest();
    let install = Uuid::new_v4();
    let fresh_path = root.path().join("fresh.sqlite");
    let ledger = AuthenticationAudit::init_new_v4(
        &fresh_path,
        install,
        input.authentication_policy.clone(),
        input.capacity_plan.clone(),
        Redactor::new(vec![]),
    )
    .unwrap();
    let bootstrap = ledger.bootstrap_receipt_v4().unwrap();
    synthetic_intention(
        &ledger,
        &SetupManifest::from(input.clone()),
        &input.tenants[0].tenant_id,
    );
    assert_eq!(ledger.verify().unwrap().records, 3);
    assert_eq!(ledger.bootstrap_receipt_v4().unwrap(), bootstrap);
    drop(ledger);
    let reopened = AuthenticationAudit::open_existing(
        &fresh_path,
        install,
        input.authentication_policy.clone(),
        Redactor::new(vec![]),
    )
    .unwrap();
    assert_eq!(reopened.bootstrap_receipt_v4().unwrap(), bootstrap);
    let old = AuthenticationAudit::init_new(
        &root.path().join("legacy.sqlite"),
        install,
        input.authentication_policy.clone(),
        Redactor::new(vec![]),
    )
    .unwrap();
    old.append_typed(AuthenticationRecordV1 {
        operation_id: Uuid::new_v4(),
        action: AuthenticationActionV1::SessionAdmission,
        phase: AuthenticationPhaseV1::Intention,
        issuer: input.gateway.provider.issuer.clone(),
        client_id: input.gateway.provider.cli_client_id.clone(),
        principal: None,
        transaction_id: None,
        session_id: Some(Uuid::new_v4()),
        causal: None,
    })
    .unwrap();
    old.migrate_tenant_capacity_v4(&root.path().join("before-v4.sqlite"), input.capacity_plan)
        .unwrap();
    let before = old.verify().unwrap();
    assert_eq!(
        old.bootstrap_receipt_v4()
            .unwrap_err()
            .downcast_ref::<AuthenticationAuditFailureV1>(),
        Some(&AuthenticationAuditFailureV1::Format)
    );
    assert_eq!(old.verify().unwrap(), before);
}

#[test]
fn origin_two_matches_the_real_bootstrap_after_activity_and_refuses_a_false_ack() {
    let root = parent();
    let output = root.path().join("installation");
    storage::prepare(manifest(), &output).unwrap();
    let preparation: Preparation =
        serde_json::from_slice(&fs::read(output.join("preparation.json")).unwrap()).unwrap();
    let origin = fs::read(output.join("authentication-origin.json")).unwrap();
    let secrets = fs::read(output.join("secret-material.json")).unwrap();
    let ledger = audit(&output, &preparation);
    synthetic_intention(
        &ledger,
        &preparation.manifest,
        &preparation.manifest.tenants[0].tenant_id,
    );
    let before = ledger.verify().unwrap();
    assert_eq!(before.records, 3);
    drop(ledger);
    storage::resume(&output).unwrap();
    assert_eq!(audit(&output, &preparation).verify().unwrap(), before);
    assert_eq!(
        fs::read(output.join("authentication-origin.json")).unwrap(),
        origin
    );
    let mut false_ack: serde_json::Value = serde_json::from_slice(&origin).unwrap();
    false_ack["bootstrap"]["hash"] = "f".repeat(64).into();
    fs::write(output.join("original-origin-evidence.json"), &origin).unwrap();
    fs::set_permissions(
        output.join("original-origin-evidence.json"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    // A missing completion is a supported retry. Retain it separately so its
    // old bytes cannot accidentally be the guard rejecting this false ACK.
    fs::rename(
        output.join("completion.json"),
        output.join("original-completion-evidence.json"),
    )
    .unwrap();
    fs::write(
        output.join("authentication-origin.json"),
        serde_json::to_vec(&false_ack).unwrap(),
    )
    .unwrap();
    assert!(storage::resume(&output).is_err());
    assert!(!output.join("completion.json").exists());
    assert_eq!(audit(&output, &preparation).verify().unwrap(), before);
    assert_eq!(
        fs::read(output.join("secret-material.json")).unwrap(),
        secrets
    );
}

#[test]
fn v2_publication_failure_keeps_origin_two_and_replays_after_real_rename() {
    let root = parent();
    let output = root.path().join("installation");
    crate::control_fs::publication::FAIL_AFTER_RENAME.with(|flag| flag.set(true));
    assert!(storage::prepare(manifest(), &output).is_err());
    assert!(output.join("completion.json").exists());
    let preparation_bytes = fs::read(output.join("preparation.json")).unwrap();
    let preparation = serde_json::from_slice(&preparation_bytes).unwrap();
    let completion = fs::read(output.join("completion.json")).unwrap();
    let origin = fs::read(output.join("authentication-origin.json")).unwrap();
    let material = fs::read(output.join("secret-material.json")).unwrap();
    let before = audit(&output, &preparation).verify().unwrap();
    assert_eq!(before.records, 2);
    storage::resume(&output).unwrap();
    assert_eq!(
        fs::read(output.join("preparation.json")).unwrap(),
        preparation_bytes
    );
    assert_eq!(
        fs::read(output.join("completion.json")).unwrap(),
        completion
    );
    assert_eq!(
        fs::read(output.join("authentication-origin.json")).unwrap(),
        origin
    );
    assert_eq!(
        fs::read(output.join("secret-material.json")).unwrap(),
        material
    );
    assert_eq!(audit(&output, &preparation).verify().unwrap(), before);
}
