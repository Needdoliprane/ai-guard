use super::*;
use crate::{
    audit::{EventQuery, Store, authentication::AuthenticationAudit},
    control_fs::{Directory, publication::FAIL_AFTER_RENAME},
    model::Event,
    setup::storage::{Preparation, SecretMaterial},
};
use std::{collections::BTreeSet, fs, os::unix::fs::PermissionsExt, path::Path};

fn parent() -> tempfile::TempDir {
    tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}
fn manifest() -> SetupManifestV1 {
    // Historical V1 retry canaries retain their original closed input shape.
    let mut value: serde_json::Value = serde_json::from_str(include_str!(
        "../../docs/setup/sso/examples/local-demo.json"
    ))
    .unwrap();
    value["version"] = 1.into();
    value.as_object_mut().unwrap().remove("capacity_plan");
    serde_json::from_value(value).unwrap()
}

fn event_receipts(store: &Store) -> Vec<(i64, String, String)> {
    store
        .events(&EventQuery {
            limit: Some(1000),
            order: Some("asc".into()),
            ..EventQuery::default()
        })
        .unwrap()
        .into_iter()
        .map(|stored| (stored.seq, stored.event.id, stored.hash))
        .collect()
}

fn directory_entries(path: &Path) -> BTreeSet<std::ffi::OsString> {
    fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect()
}

#[test]
fn real_publication_failure_retains_final_directory_and_replays_without_new_identity() {
    let parent = parent();
    let output = parent.path().join("installation");
    FAIL_AFTER_RENAME.with(|flag| flag.set(true));
    let result = storage::prepare(manifest(), &output);
    assert!(
        result.is_err(),
        "the real rename must succeed before its injected sync failure"
    );
    assert!(output.join("completion.json").exists());
    let before = fs::read(output.join("preparation.json")).unwrap();
    let completion = fs::read(output.join("completion.json")).unwrap();
    let material = fs::read(output.join("secret-material.json")).unwrap();
    storage::resume(&output).unwrap();
    assert_eq!(fs::read(output.join("preparation.json")).unwrap(), before);
    assert_eq!(
        fs::read(output.join("completion.json")).unwrap(),
        completion
    );
    assert_eq!(
        fs::read(output.join("secret-material.json")).unwrap(),
        material
    );
}

#[test]
fn actual_audit_capacity_failure_keeps_stage_then_exact_retry_commits_once() {
    let parent = parent();
    let output = parent.path().join("installation");
    storage::FAIL_AFTER_INTENT.with(|flag| flag.set(true));
    assert!(storage::prepare(manifest(), &output).is_err());
    assert!(!output.exists());
    let stage = fs::read_dir(parent.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".setup-")
        })
        .unwrap();
    let directory =
        Directory::open_existing(&crate::control_fs::absolute_path(&stage).unwrap()).unwrap();
    let preparation: Preparation = storage::read(&directory, "preparation.json")
        .unwrap()
        .unwrap();
    let secrets: SecretMaterial = storage::read(&directory, "secret-material.json")
        .unwrap()
        .unwrap();
    let tenant = &preparation.manifest.tenants[0];
    let data = stage
        .join("tenants")
        .join(&tenant.tenant_id)
        .join("audit.sqlite");
    let parent_handle = Directory::open_existing(directory.path().parent().unwrap()).unwrap();
    let intent = directory
        .persistent_read("preparation.json", MAX_RECOVERY, &[])
        .unwrap()
        .unwrap();
    assert!(intent.persistent_try_lock().unwrap());
    // Four schema genesis records and the first account fill the real Store
    // capacity. The membership's mandatory audit must then refuse it.
    assert!(
        storage::finish(
            &parent_handle,
            &directory,
            &intent,
            &preparation,
            &secrets,
            5
        )
        .is_err()
    );
    assert!(!output.exists());
    let store =
        Store::open_existing_for_tenant(&data, &tenant.tenant_id, Redactor::new(vec![]), 100_000)
            .unwrap();
    assert_eq!(store.head().unwrap(), 5);
    assert!(
        store
            .identity_memberships(&tenant.identities[0].principal)
            .unwrap()
            .is_empty()
    );
    drop(store);
    let material_before = fs::read(stage.join("secret-material.json")).unwrap();
    drop(intent);
    storage::resume(&stage).unwrap();
    assert_eq!(
        fs::read(output.join("secret-material.json")).unwrap(),
        material_before
    );
    let path = output
        .join("tenants")
        .join(&tenant.tenant_id)
        .join("audit.sqlite");
    let store =
        Store::open_existing_for_tenant(&path, &tenant.tenant_id, Redactor::new(vec![]), 100_000)
            .unwrap();
    assert_eq!(
        store.head().unwrap(),
        4 + 2 * tenant.identities.len() as i64
    );
    assert_eq!(
        store
            .identity_memberships(&tenant.identities[0].principal)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn insufficient_authentication_audit_capacity_is_rejected_before_staging() {
    let parent = parent();
    let manifest_path = parent.path().join("manifest.json");
    let output = parent.path().join("installation");
    let mut input = manifest();
    input.authentication_policy.max_serialized_bytes = 1;
    fs::write(&manifest_path, serde_json::to_vec(&input).unwrap()).unwrap();
    fs::set_permissions(&manifest_path, fs::Permissions::from_mode(0o600)).unwrap();
    let entries_before = directory_entries(parent.path());

    let result = execute(SetupArgs {
        command: SetupCommand::Prepare(PrepareArgs {
            input: manifest_path,
            output_dir: output.clone(),
            demo_defaults: false,
        }),
    });

    assert!(
        result.is_err(),
        "an unusable audit byte budget must be refused"
    );
    assert_eq!(
        directory_entries(parent.path()),
        entries_before,
        "invalid audit capacity must be rejected before staging, secrets, or databases are created"
    );
    assert!(!output.exists());
}

#[test]
fn resume_preserves_a_real_store_larger_than_the_private_file_read_bound() {
    const FILLER_EVENT_COUNT: usize = 270;
    const FILLER_BYTES_PER_FIELD: usize = 31_500;
    const PRIVATE_FILE_READ_BOUND: u64 = 32 * 1024 * 1024;
    const FILLER_EVENT_KIND: &str = "setup.resume.size_boundary";

    let parent = parent();
    let output = parent.path().join("installation");
    storage::prepare(manifest(), &output).unwrap();
    let preparation: Preparation =
        serde_json::from_slice(&fs::read(output.join("preparation.json")).unwrap()).unwrap();
    let secret_material_digest = preparation.secret_material_sha256.clone();
    let tenant_id = preparation.manifest.tenants[0].tenant_id.clone();
    let authentication_path = output.join("authentication.sqlite");
    let authentication_before = AuthenticationAudit::open_existing(
        &authentication_path,
        preparation.installation_id,
        preparation.manifest.authentication_policy.clone(),
        Redactor::new(vec![]),
    )
    .unwrap()
    .verify()
    .unwrap();

    let database = output.join("tenants").join(&tenant_id).join("audit.sqlite");
    let store =
        Store::open_existing_for_tenant(&database, &tenant_id, Redactor::new(vec![]), MAX_EVENTS)
            .unwrap();
    let mut expected_receipts = event_receipts(&store);
    expected_receipts.reserve(FILLER_EVENT_COUNT);
    for _ in 0..FILLER_EVENT_COUNT {
        let mut event = Event::new(FILLER_EVENT_KIND);
        event.data = serde_json::json!({
            "padding_a":"a".repeat(FILLER_BYTES_PER_FIELD),
            "padding_b":"b".repeat(FILLER_BYTES_PER_FIELD),
            "padding_c":"c".repeat(FILLER_BYTES_PER_FIELD),
            "padding_d":"d".repeat(FILLER_BYTES_PER_FIELD)
        });
        let stored = store.append(event).unwrap();
        expected_receipts.push((stored.seq, stored.event.id, stored.hash));
    }
    drop(store);
    assert!(
        fs::metadata(&database).unwrap().len() > PRIVATE_FILE_READ_BOUND,
        "the real SQLite fixture must exceed the production private-file read bound"
    );

    storage::resume(&output).unwrap();
    storage::resume(&output).unwrap();

    let resumed_preparation: Preparation =
        serde_json::from_slice(&fs::read(output.join("preparation.json")).unwrap()).unwrap();
    assert_eq!(
        resumed_preparation.secret_material_sha256, secret_material_digest,
        "resume must preserve the retained setup key digest"
    );
    let resumed_authentication = AuthenticationAudit::open_existing(
        &authentication_path,
        resumed_preparation.installation_id,
        resumed_preparation.manifest.authentication_policy.clone(),
        Redactor::new(vec![]),
    )
    .unwrap()
    .verify()
    .unwrap();
    assert!(
        resumed_authentication.namespace_id == authentication_before.namespace_id
            && resumed_authentication.records == authentication_before.records
            && resumed_authentication.serialized_bytes == authentication_before.serialized_bytes
            && resumed_authentication.head == authentication_before.head,
        "resume must preserve authentication audit records and receipt"
    );

    let resumed_store =
        Store::open_existing_for_tenant(&database, &tenant_id, Redactor::new(vec![]), MAX_EVENTS)
            .unwrap();
    let actual_receipts = event_receipts(&resumed_store);
    assert!(
        actual_receipts == expected_receipts,
        "resume must preserve every business event receipt without duplicates"
    );
    let integrity = resumed_store.integrity().unwrap();
    assert!(
        integrity.valid,
        "resumed business audit must remain verifiable"
    );
    assert_eq!(integrity.event_count as usize, expected_receipts.len());
    assert_eq!(
        integrity.head_hash,
        expected_receipts.last().unwrap().2,
        "resume must preserve the business audit head"
    );
}
