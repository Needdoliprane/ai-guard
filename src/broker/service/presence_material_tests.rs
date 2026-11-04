//! Actual service preparation and workspace filtering, without any runtime.
use super::*;
use crate::workspace::Limits;
use serde_json::json;
use std::{
    fs,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::PathBuf,
};
use uuid::Uuid;

const RUNNER: &str = "material-fixture-runner-key-000000000000000000";
fn private_dir(path: &Path) {
    fs::create_dir(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}
fn write(path: &Path, bytes: &[u8]) {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}
struct Fixture {
    _temporary: tempfile::TempDir,
    project: PathBuf,
    controls: PathBuf,
    registry: PathBuf,
    states: Vec<PathBuf>,
    broker_token: PathBuf,
    token: String,
}
impl Fixture {
    fn new(profiles: usize, controls_in_project: bool) -> Self {
        let temporary = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let project = root.join("project");
        let exports = root.join("exports");
        let broker = root.join("broker");
        private_dir(&project);
        private_dir(&exports);
        private_dir(&broker);
        let controls = if controls_in_project {
            project.join("control-zone")
        } else {
            root.join("control-zone")
        };
        private_dir(&controls);
        write(&project.join("work.txt"), b"owned workspace fixture");
        let runner = controls.join("runner");
        write(&runner, RUNNER.as_bytes());
        let mut entries = Vec::new();
        let mut states = Vec::new();
        let mut first = None;
        for index in 0..profiles {
            let token = format!("material-fixture-broker-key-{index:03}-000000000000000000");
            let key_path = controls.join(format!("broker-{index:03}"));
            write(&key_path, token.as_bytes());
            if first.is_none() {
                first = Some((key_path.clone(), token));
            }
            let state = controls.join(format!("state-{index:03}"));
            private_dir(&state);
            let presence = controls.join(format!("presence-{index:03}.json"));
            write(
                &presence,
                &serde_json::to_vec(&json!({
                    "version":1,"broker_id":Uuid::new_v4(),"credential_id":Uuid::new_v4(),
                    "incarnation_id":Uuid::new_v4(),"authority_id":format!("authority-{index}"),
                    "broker_token_file":key_path,"state_dir":state
                }))
                .unwrap(),
            );
            states.push(state);
            entries.push(json!({
                "id":format!("profile-{index}"),"gateway":"http://127.0.0.1:9",
                "tenant_id":"material-tenant","control_namespace_uuid":"50000000-0000-4000-8000-000000000001",
                "config_dir":controls,"runner_token_file":runner,"runtime":"docker",
                "runtime_program":"/usr/bin/false","runtime_endpoint":"unix:///unavailable-engine.sock",
                "image":"fixture:unused","policy_profile":"strict","network":"none",
                "allow_rootful_runtime":false,"max_timeout_seconds":30,"allow_capture_output":false,
                "managed_clients":false,"providers_file":null,"manual_credential_file":null,
                "source_roots":[project],"export_roots":[exports],
                "prearmed_start_profile":{"version":1,"mode":"legacy"},"presence_file":presence
            }));
        }
        let registry = controls.join("registry.json");
        write(
            &registry,
            &serde_json::to_vec(&json!({
                "version":2,"installation_id":"40000000-0000-4000-8000-000000000001",
                "root":broker,"max_enforcer_slots":8,"profiles":entries,
                "additional_control_secret_files":[]
            }))
            .unwrap(),
        );
        let (broker_token, token) = first.unwrap();
        Self {
            _temporary: temporary,
            project,
            controls,
            registry,
            states,
            broker_token,
            token,
        }
    }
}

#[tokio::test]
async fn real_service_presence_material_blocks_secret_copy_and_changed_credentials() {
    let fixture = Fixture::new(1, true);
    let authorized =
        AuthorizedSource::open(&fixture.project, std::slice::from_ref(&fixture.project)).unwrap();
    let unprotected = authorized.prepare(Limits::default(), &[]).unwrap();
    assert!(
        unprotected.manifest.contains_key("control-zone/broker-000"),
        "the positive control must actually include the otherwise unprotected key"
    );
    drop(unprotected);
    let registry = Arc::new(Registry::load(&fixture.registry).unwrap());
    let (material, prepared) = prepare_presence(registry).await.unwrap();
    assert_eq!(prepared.len(), 1);
    assert!(
        material
            .files
            .iter()
            .any(|file| file.path() == fixture.broker_token)
    );
    assert!(material.literals.contains(&fixture.token));
    let retained = material
        .parents
        .iter()
        .map(Directory::try_clone_file)
        .collect::<Result<Vec<_>>>()
        .unwrap();
    let exclusions = retained.iter().collect::<Vec<_>>();
    let protected = authorized.prepare(Limits::default(), &exclusions).unwrap();
    assert!(protected.manifest.contains_key("work.txt"));
    assert!(
        !protected
            .manifest
            .keys()
            .any(|name| name.starts_with("control-zone/"))
    );
    let bytes = fs::read(protected.archive.path()).unwrap();
    assert!(
        !bytes
            .windows(fixture.token.len())
            .any(|part| part == fixture.token.as_bytes()),
        "workspace archive must not contain the broker proof"
    );
    assert!(
        !bytes
            .windows(RUNNER.len())
            .any(|part| part == RUNNER.as_bytes())
    );
    fs::rename(
        &fixture.broker_token,
        fixture.controls.join("broker-original-preserved"),
    )
    .unwrap();
    write(
        &fixture.broker_token,
        b"material-fixture-replacement-key-000000000000",
    );
    assert!(
        material.revalidate().is_err(),
        "a changed credential must fence further business access"
    );
}

#[tokio::test]
async fn combined_service_protection_capacity_refuses_before_any_presence_anchor() {
    // 32 distinct configurations+keys and the registry/runner exceed the
    // existing64-file exclusion contract. No limit is silently widened.
    let fixture = Fixture::new(32, false);
    let registry = Arc::new(Registry::load(&fixture.registry).unwrap());
    let result = prepare_presence(registry).await;
    assert!(
        result.is_err(),
        "excess protection must fail before helper/admission initialization"
    );
    for state in &fixture.states {
        assert!(
            fs::read_dir(state).unwrap().next().is_none(),
            "no stream may be initialized before the aggregate protection preflight"
        );
    }
}

#[tokio::test]
async fn publisher_evidence_failure_keeps_trusted_service_material_usable() {
    let fixture = Fixture::new(1, false);
    let evidence = fixture.states[0].join("unexpected-evidence-preserved");
    write(
        &evidence,
        b"owned publisher evidence requiring reconciliation",
    );
    let registry = Arc::new(Registry::load(&fixture.registry).unwrap());
    let result = prepare_presence(registry).await;
    assert!(
        result.is_ok(),
        "unavailable publisher evidence must not disable the trusted service when its secret protections are safe"
    );
    let (material, ready) = result.unwrap();
    assert!(ready.is_empty(), "the unsafe stream must not emit presence");
    assert!(material.revalidate().is_ok());
    assert!(
        material
            .files
            .iter()
            .any(|file| file.path() == fixture.broker_token)
    );
    assert_eq!(
        fs::read(&evidence).unwrap(),
        b"owned publisher evidence requiring reconciliation"
    );

    // A local configuration collision with a different profile's key is
    // still fatal. It must not be confused with unavailable stream evidence.
    let unsafe_config = Fixture::new(2, false);
    let path = unsafe_config.controls.join("presence-000.json");
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["authority_id"] = serde_json::Value::String(
        fs::read_to_string(unsafe_config.controls.join("broker-001")).unwrap(),
    );
    fs::rename(
        &path,
        unsafe_config.controls.join("original-config-preserved"),
    )
    .unwrap();
    write(&path, &serde_json::to_vec(&value).unwrap());
    let registry = Arc::new(Registry::load(&unsafe_config.registry).unwrap());
    assert!(
        prepare_presence(registry).await.is_err(),
        "unsafe local material must still refuse service initialization"
    );
    for state in &unsafe_config.states {
        assert!(fs::read_dir(state).unwrap().next().is_none());
    }
}
