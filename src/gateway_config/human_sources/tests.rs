use super::*;
use serde_json::json;

#[test]
fn canonical_bytes_are_compact_sorted_utf8_with_explicit_null() {
    let value: Value = serde_json::from_str(" { \"é\": \"u\", \"a\": [null, 2] } ").unwrap();
    let bytes = canonical_public_json_v1(value).unwrap();
    assert_eq!(bytes, "{\"a\":[null,2],\"é\":\"u\"}".as_bytes());
    assert_eq!(
        digest_public(CONTROL_DOMAIN, &bytes),
        "5eb5b18f283f64e4b19bf375de626160e8b30879ef9cceec39f512cba055870b"
    );
    assert_eq!(
        digest_public(OIDC_DOMAIN, &bytes),
        "67cb221d7b4b84711558fcc4976228229d4976e8fea2e85244b041ee9493c725"
    );
    assert_ne!(
        digest_public(CONTROL_DOMAIN, &bytes),
        digest_public(OIDC_DOMAIN, &bytes)
    );
}

#[test]
fn canonicalization_preserves_semantic_array_order_and_refuses_noninteger_numbers() {
    assert_ne!(
        canonical_public_json_v1(json!([1, 2])).unwrap(),
        canonical_public_json_v1(json!([2, 1])).unwrap()
    );
    assert!(canonical_public_json_v1(json!({"float":0.5})).is_err());
}

fn tenant(id: &str) -> ConfiguredTenant {
    ConfiguredTenant {
        tenant_id: id.into(),
        storage: crate::gateway_config::ConfiguredStorage::Sqlite("/private/data".into()),
        runner_token_file: "/private/runner".into(),
        viewer_token_file: "/private/viewer".into(),
        runner_token: "synthetic-PRIVATE-runner".into(),
        viewer_token: "synthetic-PRIVATE-viewer".into(),
        worker_quota: Default::default(),
        audit_capacity_policy: None,
        workload_audit_policy: None,
        relay_retention_policy: None,
        held_create_policy: None,
        session_reservation_policy: None,
        session_recovery_policy: None,
        session_control_recovery_policy: None,
        session_effect_policy: None,
        egress_policy: None,
    }
}

#[test]
fn public_digest_sorts_tenants_excludes_credentials_and_paths_and_binds_effective_policy() {
    let mut tenants = vec![tenant("B"), tenant("A")];
    let original = resolved_control(1, 10_000, &tenants, None).unwrap();
    let before = public_control_digest_v1(&original).unwrap();
    let bytes = canonical_control_bytes(&original).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    for private in [
        "PRIVATE",
        "/private",
        "runner_token",
        "viewer_token",
        "\"data\"",
    ] {
        assert!(!text.contains(private), "private field leaked");
    }
    tenants.reverse();
    tenants[0].runner_token = "another-private-secret".into();
    tenants[0].storage =
        crate::gateway_config::ConfiguredStorage::Sqlite("/elsewhere/sensitive".into());
    assert_eq!(
        before,
        public_control_digest_v1(&resolved_control(1, 10_000, &tenants, None).unwrap()).unwrap()
    );
    assert_ne!(
        before,
        public_control_digest_v1(&resolved_control(2, 10_000, &tenants, None).unwrap()).unwrap()
    );
    assert_ne!(
        before,
        public_control_digest_v1(&resolved_control(1, 20_000, &tenants, None).unwrap()).unwrap()
    );
    assert!(resolved_control(0, 10_000, &tenants, None).is_err());
    tenants.push(tenant("A"));
    assert!(resolved_control(1, 10_000, &tenants, None).is_err());
}

#[test]
fn mode_selection_refuses_ambiguous_or_missing_inputs_without_opening_sources() {
    assert!(
        validate_human_mode(
            "legacy",
            Some(Path::new("/manifest")),
            None,
            None,
            false,
            false
        )
        .is_err()
    );
    assert!(validate_human_mode("owned_sources", None, None, None, false, false).is_err());
    assert!(
        validate_human_mode(
            "owned_sources",
            Some(Path::new("/manifest")),
            Some(Path::new("/envelope")),
            Some(Path::new("/registry")),
            true,
            false
        )
        .is_err()
    );
    assert!(validate_human_mode("legacy", None, None, None, false, false).is_ok());
}

#[test]
fn owned_tenants_load_private_role_files_without_opening_the_store() {
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
    let runner = "synthetic-runner-credential-long-enough";
    let viewer = "synthetic-viewer-credential-long-enough";
    let runner_file = write("runner.token", runner.as_bytes());
    let viewer_file = write("viewer.token", viewer.as_bytes());
    let registry = write(
        "tenants.json",
        &serde_json::to_vec(&json!({
            "version": 1,
            "tenants": [{
                "tenant_id": "tenant-a",
                "data": "events.sqlite",
                "runner_token_file": "runner.token",
                "viewer_token_file": "viewer.token"
            }]
        }))
        .unwrap(),
    );
    let tenants = load_owned_tenants(&registry).unwrap();
    let [tenant] = tenants.as_slice() else {
        panic!("expected one configured tenant");
    };
    assert_eq!(tenant.runner_token_file, runner_file);
    assert_eq!(tenant.viewer_token_file, viewer_file);
    assert!(tenant.runner_token == runner && tenant.viewer_token == viewer);

    fs::set_permissions(&viewer_file, fs::Permissions::from_mode(0o640)).unwrap();
    assert!(load_owned_tenants(&registry).is_err());
    assert_eq!(fs::read_dir(root).unwrap().count(), 3);
}
