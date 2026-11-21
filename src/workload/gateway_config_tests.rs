use super::*;
use serde_json::json;

fn private_tempdir() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;

    tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .expect("create a private workload config test directory")
}

fn create_private_file(path: &std::path::Path, bytes: &[u8]) {
    use std::io::Write;

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).expect("create a private test file");
    file.write_all(bytes).expect("write a private test file");
}

#[test]
fn workload_registry_policy_is_optional_closed_and_prevalidated_against_human_reserves() {
    let directory = private_tempdir();
    let path = directory.path().join("tenants.json");
    create_private_file(&directory.path().join("runner"), "r".repeat(48).as_bytes());
    create_private_file(&directory.path().join("viewer"), "v".repeat(48).as_bytes());
    let mut value = json!({"version":1,"tenants":[{"tenant_id":"tenant-a","data":"a.sqlite","runner_token_file":"runner","viewer_token_file":"viewer"}]});
    create_private_file(&path, value.to_string().as_bytes());
    assert!(load(&path).unwrap()[0].workload_audit_policy.is_none());
    value["tenants"][0]["workload_audit_policy"] = json!({"version":1,"tenant":{"events":16,"bytes":67108864},"principal":{"events":4,"bytes":16777216}});
    std::fs::write(&path, value.to_string()).unwrap();
    let configured = load(&path).unwrap();
    validate_policies(&configured, 64).unwrap();
    assert!(!configured[0].storage.sqlite_path().unwrap().exists());
    value["tenants"][0]["workload_audit_policy"]["tenant"]["events"] = json!(41);
    std::fs::write(&path, value.to_string()).unwrap();
    assert!(validate_policies(&load(&path).unwrap(), 64).is_err());
    value["tenants"][0]["workload_audit_policy"]["tenant"]["extra"] = json!(true);
    std::fs::write(&path, value.to_string()).unwrap();
    assert!(load(&path).is_err());
}

#[cfg(unix)]
#[test]
fn workload_policy_file_is_private_bounded_and_closed() {
    use std::os::unix::fs::PermissionsExt;
    let directory = private_tempdir();
    let path = directory.path().join("workload.json");
    let value = json!({"version":1,"tenant":{"events":16,"bytes":67108864},"principal":{"events":4,"bytes":16777216}});
    create_private_file(&path, value.to_string().as_bytes());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(agentsandbox::audit::Store::load_workload_audit_policy(&path).is_ok());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(agentsandbox::audit::Store::load_workload_audit_policy(&path).is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let alias = directory.path().join("alias.json");
    std::os::unix::fs::symlink(&path, &alias).unwrap();
    assert!(agentsandbox::audit::Store::load_workload_audit_policy(&alias).is_err());
    std::fs::write(&path, vec![b' '; 16385]).unwrap();
    assert!(agentsandbox::audit::Store::load_workload_audit_policy(&path).is_err());
    let mut unknown = value;
    unknown["token"] = json!("not-a-supported-field");
    std::fs::write(&path, unknown.to_string()).unwrap();
    assert!(agentsandbox::audit::Store::load_workload_audit_policy(&path).is_err());
}
