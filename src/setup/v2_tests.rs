//! Real private preparation/reopen regressions; no IdP, provider or runtime.
use super::*;
#[path = "v2_contract_tests.rs"]
mod contract_tests;
use crate::audit::authentication::{
    AuthenticationAudit, AuthenticationCapacityPlanV1, AuthenticationCapacityV1,
    AuthenticationTenantCapacityV1,
};
use crate::setup::storage::{Preparation, SecretMaterial};
use std::{fs, os::unix::fs::PermissionsExt, path::Path};

fn parent() -> tempfile::TempDir {
    tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}
fn legacy() -> SetupManifestV1 {
    // Keep the historical public shape independent of the upgraded DEMO file.
    let mut value: serde_json::Value = serde_json::from_str(include_str!(
        "../../docs/setup/sso/examples/local-demo.json"
    ))
    .unwrap();
    value["version"] = 1.into();
    value.as_object_mut().unwrap().remove("capacity_plan");
    serde_json::from_value(value).unwrap()
}
fn manifest() -> SetupManifestV2 {
    let v1 = legacy();
    let plan = AuthenticationCapacityPlanV1 {
        version: 1,
        unscoped: AuthenticationCapacityV1 {
            max_records: 20,
            max_serialized_bytes: 100_000,
        },
        tenants: v1
            .tenants
            .iter()
            .map(|tenant| AuthenticationTenantCapacityV1 {
                tenant_id: tenant.tenant_id.clone(),
                capacity: AuthenticationCapacityV1 {
                    max_records: 1000,
                    max_serialized_bytes: 1_000_000,
                },
            })
            .collect(),
    };
    SetupManifestV2 {
        version: 2,
        gateway: v1.gateway,
        authentication_policy: v1.authentication_policy,
        capacity_plan: plan,
        ca_bundle_pem: v1.ca_bundle_pem,
        tenants: v1.tenants,
    }
}
fn audit(root: &Path, preparation: &Preparation) -> AuthenticationAudit {
    AuthenticationAudit::open_existing(
        &root.join("authentication.sqlite"),
        preparation.installation_id,
        preparation.manifest.authentication_policy.clone(),
        Redactor::new(vec![]),
    )
    .unwrap()
}
fn entries(root: &Path) -> BTreeSet<std::ffi::OsString> {
    fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect()
}

#[test]
fn invalid_v2_funding_or_scope_fails_before_any_artifact() {
    for kind in 0..4 {
        let root = parent();
        let output = root.path().join("installation");
        let mut input = manifest();
        match kind {
            0 => input.authentication_policy.max_records = 2,
            1 => input.authentication_policy.max_serialized_bytes = 1000,
            2 => input.capacity_plan.tenants[0].tenant_id = "unconfigured-tenant".into(),
            _ => input
                .capacity_plan
                .tenants
                .push(input.capacity_plan.tenants[0].clone()),
        }
        if kind == 1 {
            let mut common = legacy();
            common.authentication_policy = input.authentication_policy.clone();
            assert!(
                common.validate().is_ok(),
                "the policy/common genesis preflight must pass before testing V4 partition funding"
            );
        }
        let before = entries(root.path());
        assert!(storage::prepare(input, &output).is_err());
        assert_eq!(entries(root.path()), before);
        assert!(!output.exists());
    }
}

#[test]
fn v1_origin_and_replay_keep_their_exact_meaning() {
    let root = parent();
    let output = root.path().join("legacy");
    storage::prepare(legacy(), &output).unwrap();
    let preparation: Preparation =
        serde_json::from_slice(&fs::read(output.join("preparation.json")).unwrap()).unwrap();
    assert_eq!(preparation.version, 1);
    let ledger = audit(&output, &preparation);
    assert_eq!(ledger.format_version().unwrap(), 3);
    let verified = ledger.verify().unwrap();
    assert_eq!(verified.records, 1);
    let origin = fs::read(output.join("authentication-origin.json")).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&origin).unwrap();
    assert_eq!(value["version"], 1);
    assert_eq!(value["genesis"]["sequence"], 1);
    drop(ledger);
    storage::resume(&output).unwrap();
    assert_eq!(
        fs::read(output.join("authentication-origin.json")).unwrap(),
        origin
    );
    assert_eq!(audit(&output, &preparation).verify().unwrap(), verified);
}

#[test]
fn v2_bootstrap_retry_fault_cuts_preserve_binding_receipts_and_credentials() {
    for cut in [
        installation::AuthenticationFault::AfterCreationBinding,
        installation::AuthenticationFault::AfterBootstrap,
        installation::AuthenticationFault::AfterOrigin,
    ] {
        let root = parent();
        let output = root.path().join("new");
        installation::AUTHENTICATION_FAULT.with(|hook| hook.set(Some(cut)));
        assert!(storage::prepare(manifest(), &output).is_err());
        let stage = fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.is_dir())
            .unwrap();
        let preparation: Preparation =
            serde_json::from_slice(&fs::read(stage.join("preparation.json")).unwrap()).unwrap();
        let secrets = fs::read(stage.join("secret-material.json")).unwrap();
        let creation = fs::read(stage.join("authentication-creation.json")).unwrap();
        let raw: SecretMaterial = serde_json::from_slice(&secrets).unwrap();
        let identity = storage::NativeIdentity::entry(
            &crate::control_fs::Directory::open_existing(&stage).unwrap(),
            "authentication.sqlite",
        )
        .unwrap();
        storage::resume(&stage).unwrap();
        assert_eq!(
            fs::read(output.join("secret-material.json")).unwrap(),
            secrets
        );
        assert_eq!(
            fs::read(output.join("authentication-creation.json")).unwrap(),
            creation
        );
        let db = audit(&output, &preparation);
        assert_eq!(db.format_version().unwrap(), 4);
        let verified = db.verify().unwrap();
        assert_eq!(verified.records, 2);
        assert_eq!(
            verified.namespace_id,
            preparation.authentication_namespace_id.unwrap()
        );
        assert!(
            storage::NativeIdentity::entry(
                &crate::control_fs::Directory::open_existing(&output).unwrap(),
                "authentication.sqlite"
            )
            .unwrap()
                == identity
        );
        let origin: serde_json::Value =
            serde_json::from_slice(&fs::read(output.join("authentication-origin.json")).unwrap())
                .unwrap();
        assert_eq!(origin["version"], 2);
        assert_eq!(origin["bootstrap"]["sequence"], 2);
        for tenant in raw.tenants {
            assert_eq!(
                fs::read_to_string(
                    output
                        .join("tenants")
                        .join(&tenant.tenant_id)
                        .join("runner")
                )
                .unwrap()
                .trim(),
                tenant.runner
            );
            assert_eq!(
                fs::read_to_string(
                    output
                        .join("tenants")
                        .join(&tenant.tenant_id)
                        .join("viewer")
                )
                .unwrap()
                .trim(),
                tenant.viewer
            );
        }
        drop(db);
        storage::resume(&output).unwrap();
        assert_eq!(audit(&output, &preparation).verify().unwrap(), verified);
    }
}
