//! Offline human maintenance remains available after the explicit workload migration.
//! These tests use private files, real Store migrations and the product CLI subprocess.
mod support;

use agentsandbox::{
    audit::{EventQuery, Store},
    identity::{HumanAccountV1, IdentityChangeV1, IdentityReceiptV1, LocalHumanProvisionV1},
    redaction::Redactor,
};
use serde_json::{Value, json};
use std::{
    fs::OpenOptions,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
    process::{Command, Output, Stdio},
};
use uuid::Uuid;

const TENANT: &str = "tenant-a";
const CAPACITY: u64 = 1000;

fn store(path: &Path) -> Store {
    Store::open_existing_for_tenant(path, TENANT, Redactor::new(vec![]), CAPACITY).unwrap()
}

fn initialized(path: &Path, version: u32) {
    let origin = Store::create_supervised_for_tenant(
        path,
        TENANT,
        Redactor::new(vec![]),
        CAPACITY,
        &path.with_extension("before-v3.sqlite"),
    )
    .unwrap();
    if version >= 4 {
        origin
            .migrate_identity_v4(&path.with_extension("before-v4.sqlite"))
            .unwrap();
    }
    if version >= 5 {
        origin
            .migrate_human_access_v5(&path.with_extension("before-v5.sqlite"))
            .unwrap();
    }
    if version >= 6 {
        origin
            .migrate_workload_v6(&path.with_extension("before-v6.sqlite"))
            .unwrap();
    }
    let namespace = origin.supervision_state().unwrap().control_namespace_uuid;
    drop(origin);
    let reopened = store(path);
    let state = reopened.supervision_state().unwrap();
    assert_eq!(state.schema_version, version);
    assert_eq!(state.control_namespace_uuid, namespace);
    assert!(!state.active);
    assert!(reopened.integrity().unwrap().valid);
    for target in 4..=version {
        let backup = store(&path.with_extension(format!("before-v{target}.sqlite")));
        assert_eq!(
            backup.supervision_state().unwrap().schema_version,
            target - 1
        );
        assert!(!backup.supervision_state().unwrap().active);
        assert!(backup.integrity().unwrap().valid);
    }
}

fn command(verb: &str, data: &Path, tenant: &str, confirm: bool) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .args(["human", verb, "--data"])
        .arg(data)
        .args(["--tenant-id", tenant]);
    if confirm {
        command.arg("--confirm-control-stopped");
    }
    command
}

fn provision(data: &Path, output: &Path, tenant: &str, confirm: bool) -> Command {
    let mut command = command("provision", data, tenant, confirm);
    command
        .args([
            "--name",
            "Alice",
            "--organization",
            "organization-a",
            "--output-dir",
        ])
        .arg(output);
    command
}

fn resume(data: &Path, output: &Path) -> Output {
    command("resume-provision", data, TENANT, true)
        .arg("--output-dir")
        .arg(output)
        .output()
        .unwrap()
}

fn success(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "human maintenance on a validated inactive schema6 database exited {:?}; offline compatibility guard reported: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
            .contains("offline identity maintenance requires an existing validated schema")
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn no_secret(output: &Output, secret: &str) {
    assert!(!String::from_utf8_lossy(&output.stdout).contains(secret));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(secret));
}

fn preparation(output: &Path) -> LocalHumanProvisionV1 {
    let recovery: Value =
        serde_json::from_slice(&std::fs::read(output.join("recovery.json")).unwrap()).unwrap();
    serde_json::from_value(recovery["provision"].clone()).unwrap()
}

fn account(prepared: &LocalHumanProvisionV1) -> HumanAccountV1 {
    let IdentityChangeV1::Account { account, .. } = &prepared.mutations[0].change else {
        panic!("expected a typed local account preparation")
    };
    account.clone()
}

fn private_json(path: &Path, value: &Value) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(&serde_json::to_vec(value).unwrap()).unwrap();
    file.sync_all().unwrap();
}

fn audit_stamp(data: &Path) -> (u64, String) {
    let audit = store(data);
    let integrity = audit.integrity().unwrap();
    assert!(integrity.valid);
    assert!(!audit.supervision_state().unwrap().active);
    (integrity.event_count, integrity.head_hash)
}

#[test]
fn schema_six_provision_resume_and_revocation_preserve_durable_identity() {
    let files = support::private_tempdir();
    let data = files.path().join("audit.sqlite");
    initialized(&data, 6);
    let before = audit_stamp(&data);
    let output = files.path().join("alice");
    let provisioned = provision(&data, &output, TENANT, true).output().unwrap();
    assert_eq!(success(&provisioned)["status"], "provisioned");
    let prepared = preparation(&output);
    let mut human = account(&prepared);
    let credential = std::fs::read(output.join("credential")).unwrap();
    let bearer = std::str::from_utf8(&credential).unwrap().trim();
    no_secret(&provisioned, bearer);
    let recovery = std::fs::read(output.join("recovery.json")).unwrap();
    let completion = std::fs::read(output.join("completion.json")).unwrap();
    assert_eq!(
        std::fs::metadata(&output).unwrap().permissions().mode() & 0o7777,
        0o700
    );
    for name in ["credential", "recovery.json", "completion.json"] {
        assert_eq!(
            std::fs::metadata(output.join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            0o600
        );
    }
    let audit = store(&data);
    assert_eq!(audit.head().unwrap() as u64, before.0 + 3);
    assert_eq!(
        audit.human_account(&human.principal).unwrap(),
        Some(human.clone())
    );
    assert_eq!(
        audit.identity_memberships(&human.principal).unwrap().len(),
        1
    );
    let receipts: Vec<IdentityReceiptV1> =
        serde_json::from_slice::<Value>(&completion).unwrap()["receipts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| serde_json::from_value(value.clone()).unwrap())
            .collect();
    assert_eq!(receipts.len(), 3);
    let events = audit.events(&EventQuery::default()).unwrap();
    assert!(!serde_json::to_string(&events).unwrap().contains(bearer));
    for (receipt, mutation) in receipts.iter().zip(&prepared.mutations) {
        assert_eq!(receipt.operation_id, mutation.operation_id);
        let event = events
            .iter()
            .find(|event| event.seq == receipt.audit_seq)
            .unwrap();
        assert_eq!(event.hash, receipt.audit_hash);
        assert_eq!(event.event.kind, "control.identity.mutation");
    }
    drop(audit);
    let committed = audit_stamp(&data);
    // Simulate a response/receipt publication lost after the SQLite transaction.
    std::fs::rename(
        output.join("completion.json"),
        output.join("retained-completion.json"),
    )
    .unwrap();
    let resumed = resume(&data, &output);
    success(&resumed);
    no_secret(&resumed, bearer);
    assert!(std::fs::read(output.join("completion.json")).unwrap() == completion);
    success(&resume(&data, &output));
    assert_eq!(audit_stamp(&data), committed);
    let duplicate = provision(&data, &output, TENANT, true).output().unwrap();
    assert!(!duplicate.status.success());
    no_secret(&duplicate, bearer);
    std::fs::set_permissions(
        output.join("credential"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let unsafe_file = resume(&data, &output);
    assert!(!unsafe_file.status.success());
    no_secret(&unsafe_file, bearer);
    std::fs::set_permissions(
        output.join("credential"),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    assert!(std::fs::read(output.join("credential")).unwrap() == credential);
    assert!(std::fs::read(output.join("recovery.json")).unwrap() == recovery);
    assert_eq!(audit_stamp(&data), committed);

    human.active = false;
    human.revision = 2;
    let mutation = files.path().join("revoke-alice.json");
    let operation = Uuid::new_v4().to_string();
    private_json(
        &mutation,
        &json!({
            "version": 1, "operation_id": operation,
            "change": IdentityChangeV1::Account { expected_revision: 1, account: human.clone() }
        }),
    );
    let apply = || {
        command("apply", &data, TENANT, true)
            .arg("--mutation-file")
            .arg(&mutation)
            .output()
            .unwrap()
    };
    let applied = apply();
    assert_eq!(success(&applied)["operation_id"], operation);
    no_secret(&applied, bearer);
    let audit = store(&data);
    assert_eq!(audit.human_account(&human.principal).unwrap(), Some(human));
    assert_eq!(audit.identity_authorization_revision().unwrap(), 4);
    assert_eq!(audit.supervision_state().unwrap().schema_version, 6);
    drop(audit);
    let revoked = audit_stamp(&data);
    assert_eq!(revoked.0, committed.0 + 1);
    success(&apply());
    assert_eq!(audit_stamp(&data), revoked);
}

#[test]
fn schema_six_refuses_wrong_tenant_and_missing_confirmation_without_mutation() {
    let files = support::private_tempdir();
    let data = files.path().join("audit.sqlite");
    initialized(&data, 6);
    let before = audit_stamp(&data);
    for (tenant, confirm, name) in [
        ("tenant-b", true, "wrong-tenant"),
        (TENANT, false, "not-confirmed"),
    ] {
        let output = files.path().join(name);
        let refused = provision(&data, &output, tenant, confirm).output().unwrap();
        assert!(!refused.status.success());
        assert!(!output.exists());
        assert_eq!(audit_stamp(&data), before);
    }
    assert_eq!(store(&data).supervision_state().unwrap().schema_version, 6);
}

#[test]
fn unsupported_schema_three_is_not_migrated_for_human_maintenance() {
    let files = support::private_tempdir();
    let data = files.path().join("audit.sqlite");
    initialized(&data, 3);
    let before = audit_stamp(&data);
    let output = files.path().join("alice");
    let refused = provision(&data, &output, TENANT, true).output().unwrap();
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("migrate explicitly"));
    assert!(!output.exists());
    assert_eq!(store(&data).supervision_state().unwrap().schema_version, 3);
    assert_eq!(audit_stamp(&data), before);
    assert!(!data.with_extension("before-v4.sqlite").exists());
}

#[test]
fn schema_six_audit_capacity_failure_retains_exact_preparation_for_retry() {
    let files = support::private_tempdir();
    let data = files.path().join("audit.sqlite");
    initialized(&data, 6);
    let before = audit_stamp(&data);
    let output = files.path().join("alice");
    // Provision requires three audit records in one transaction. Two remaining
    // physical slots must not leave a partially activated account or receipt.
    let refused = provision(&data, &output, TENANT, true)
        .args(["--max-events", &(before.0 + 2).to_string()])
        .output()
        .unwrap();
    assert!(!refused.status.success());
    let prepared = preparation(&output);
    let human = account(&prepared);
    let credential = std::fs::read(output.join("credential")).unwrap();
    let recovery = std::fs::read(output.join("recovery.json")).unwrap();
    no_secret(&refused, std::str::from_utf8(&credential).unwrap().trim());
    assert!(!output.join("completion.json").exists());
    assert!(
        store(&data)
            .human_account(&human.principal)
            .unwrap()
            .is_none()
    );
    assert_eq!(audit_stamp(&data), before);
    let resumed = resume(&data, &output);
    success(&resumed);
    no_secret(&resumed, std::str::from_utf8(&credential).unwrap().trim());
    assert!(std::fs::read(output.join("credential")).unwrap() == credential);
    assert!(std::fs::read(output.join("recovery.json")).unwrap() == recovery);
    assert_eq!(audit_stamp(&data).0, before.0 + 3);
    assert_eq!(
        store(&data).human_account(&human.principal).unwrap(),
        Some(human)
    );
}
