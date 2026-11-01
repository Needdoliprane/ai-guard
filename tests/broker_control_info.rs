//! CLI diagnostics over the real authenticated Axum/SQLite control endpoint.
//! No broker, runtime, provider credential or execution grant is involved.
mod support;

use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::Store,
    control::{ControlProtocolV1, QuotaPolicyV1, SupervisionProtocolV1},
    control_fs::PrivateFile,
    policy::Policy,
    redaction::Redactor,
    runner::{AuditClient, ControlIdentityError},
};
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::Output,
    sync::Arc,
    time::Duration,
};
use tokio::process::Command;

const RUNNER_A: &str = "qa-control-runner-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const VIEWER_A: &str = "qa-control-viewer-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const RUNNER_B: &str = "qa-control-runner-cccccccccccccccccccccccccccccccc";
const VIEWER_B: &str = "qa-control-viewer-dddddddddddddddddddddddddddddddd";
const WRONG: &str = "qa-control-unknown-eeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

struct Server {
    origin: String,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Fixture {
    server: Server,
    a: Arc<Store>,
    b: Arc<Store>,
    root: PathBuf,
    _directory: tempfile::TempDir,
}

impl Fixture {
    async fn new(tenant_a: &str, runner_a: &str) -> Self {
        let directory = support::private_tempdir();
        // PrivateFile deliberately refuses filesystem aliases and symlinks.
        let root = directory.path().canonicalize().unwrap();
        let a = Arc::new(
            Store::open_for_tenant(
                &root.join("a.sqlite"),
                tenant_a,
                Redactor::new(vec![]),
                10_000,
            )
            .unwrap(),
        );
        a.migrate_control_v3(&root.join("a.before-v3.sqlite"))
            .unwrap();
        let b = Arc::new(
            Store::open_for_tenant(
                &root.join("b.sqlite"),
                "tenant-b",
                Redactor::new(vec![]),
                10_000,
            )
            .unwrap(),
        );
        let state = GatewayState::for_tenants(
            vec![
                TenantGateway {
                    tenant_id: tenant_a.into(),
                    store: Arc::clone(&a),
                    runner_token: runner_a.into(),
                    viewer_token: VIEWER_A.into(),
                },
                TenantGateway {
                    tenant_id: "tenant-b".into(),
                    store: Arc::clone(&b),
                    runner_token: RUNNER_B.into(),
                    viewer_token: VIEWER_B.into(),
                },
            ],
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, gateway_router(state)).await.unwrap();
        });
        Self {
            server: Server { origin, task },
            a,
            b,
            root,
            _directory: directory,
        }
    }

    fn token_file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.root.join(name);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        file.write_all(bytes).unwrap();
        path
    }

    fn snapshot(&self) -> Value {
        Value::Array(
            [&self.a, &self.b]
                .into_iter()
                .map(|store| {
                    json!({
                        "head": store.head().unwrap(),
                        "supervision": store.supervision_state().unwrap(),
                        "workers": store.workers().unwrap(),
                        "quota": store.worker_quota(&QuotaPolicyV1::default()).unwrap(),
                        "usage": store.worker_usage().unwrap(),
                    })
                })
                .collect(),
        )
    }

    async fn identity(&self, token: &str) -> Result<ControlProtocolV1, ControlIdentityError> {
        // This positive preflight establishes that the real HTTP fixture and
        // authentication are healthy before any CLI assertion can fail.
        AuditClient::new(&self.server.origin, token)
            .unwrap()
            .supervision_identity()
            .await
    }

    async fn diagnose(&self, token_file: &Path) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
        command
            .args(["broker", "control-info", "--gateway"])
            .arg(&self.server.origin)
            .arg("--token-file")
            .arg(token_file)
            .current_dir(&self.root)
            .env_clear()
            .kill_on_drop(true);
        // Product HTTP is bounded at 15s. The extra 5s covers process startup
        // and shutdown; async output leaves this same runtime serving HTTP.
        tokio::time::timeout(Duration::from_secs(20), command.output())
            .await
            .expect("control-info exceeded its bounded HTTP/process budget")
            .expect("start the real agentsandbox binary")
    }
}

fn assert_no_credential(output: &Output, credentials: &[&str]) {
    for bytes in [&output.stdout, &output.stderr] {
        for credential in credentials {
            assert!(
                !bytes
                    .windows(credential.len())
                    .any(|window| window == credential.as_bytes()),
                "control-info disclosed a fixture credential"
            );
        }
    }
}

fn protocol_output(output: &Output) -> ControlProtocolV1 {
    assert!(
        output.status.success(),
        "control-info must read the live authenticated control endpoint; status={:?}; stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        output.stderr.is_empty(),
        "successful diagnostic wrote an error"
    );
    let protocol: ControlProtocolV1 = serde_json::from_slice(&output.stdout)
        .expect("control-info stdout must contain only the typed control protocol JSON");
    protocol.validate().unwrap();
    protocol
}

#[tokio::test]
async fn runner_diagnostic_reads_real_control_identity_without_broker_or_mutation() {
    let fixture = Fixture::new("tenant-a", RUNNER_A).await;
    let live = fixture.identity(RUNNER_A).await.unwrap();
    assert_eq!(live.role, "runner");
    assert!(live.active);
    let before = fixture.snapshot();
    let file = fixture.token_file("runner", format!("{RUNNER_A}\n").as_bytes());
    PrivateFile::open(&file, 4096)
        .unwrap()
        .revalidate()
        .unwrap();

    let output = fixture.diagnose(&file).await;
    assert_no_credential(&output, &[RUNNER_A, VIEWER_A, RUNNER_B, VIEWER_B]);
    let protocol = protocol_output(&output);
    assert_eq!(protocol.version, 1);
    assert_eq!(protocol.tenant_id, "tenant-a");
    assert_eq!(protocol.role, "runner");
    assert_eq!(protocol.schema_version, 3);
    assert_eq!(
        protocol.control_namespace_uuid,
        fixture
            .a
            .supervision_state()
            .unwrap()
            .control_namespace_uuid,
    );
    assert!(protocol.control_namespace_uuid.is_some());
    assert_eq!(
        protocol.protocols,
        [SupervisionProtocolV1::BrokerSupervisionV1]
    );
    assert!(protocol.active);
    assert!(
        fixture.snapshot() == before,
        "diagnostic mutated durable control state"
    );
}

#[tokio::test]
async fn viewer_diagnostic_preserves_authenticated_scope_and_inactive_schema2() {
    let fixture = Fixture::new("tenant-a", RUNNER_A).await;
    let before = fixture.snapshot();
    for (name, token, tenant, schema, active, store) in [
        ("viewer-a", VIEWER_A, "tenant-a", 3, true, &fixture.a),
        ("viewer-b", VIEWER_B, "tenant-b", 2, false, &fixture.b),
    ] {
        let live = fixture.identity(token).await.unwrap();
        assert_eq!(live.role, "viewer");
        assert_eq!(live.active, active);
        let file = fixture.token_file(name, token.as_bytes());
        let output = fixture.diagnose(&file).await;
        assert_no_credential(&output, &[RUNNER_A, VIEWER_A, RUNNER_B, VIEWER_B]);
        let protocol = protocol_output(&output);
        assert_eq!(protocol.version, 1);
        assert_eq!(protocol.tenant_id, tenant);
        assert_eq!(protocol.role, "viewer");
        assert_eq!(protocol.schema_version, schema);
        assert_eq!(protocol.active, active);
        assert_eq!(
            protocol.control_namespace_uuid,
            store.supervision_state().unwrap().control_namespace_uuid,
        );
        if schema == 2 {
            assert!(protocol.control_namespace_uuid.is_some());
            assert!(protocol.protocols.is_empty());
        } else {
            assert_eq!(
                protocol.protocols,
                [SupervisionProtocolV1::BrokerSupervisionV1]
            );
        }
    }
    assert!(
        fixture.snapshot() == before,
        "diagnostic mutated durable control state"
    );
}

#[tokio::test]
async fn unauthenticated_diagnostic_fails_without_identity_secret_or_mutation() {
    let fixture = Fixture::new("tenant-a", RUNNER_A).await;
    assert!(matches!(
        fixture.identity(WRONG).await,
        Err(ControlIdentityError::Unauthorized)
    ));
    let before = fixture.snapshot();
    let file = fixture.token_file("unknown", WRONG.as_bytes());
    let output = fixture.diagnose(&file).await;
    assert_no_credential(&output, &[WRONG, RUNNER_A, VIEWER_A, RUNNER_B, VIEWER_B]);
    assert!(!output.status.success());
    assert!(
        output.stdout.is_empty(),
        "authentication failure disclosed an identity"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("not authenticated"),
        "authentication failure must retain its closed error class"
    );
    assert!(
        fixture.snapshot() == before,
        "failed diagnostic mutated control state"
    );
}

#[tokio::test]
async fn unsafe_or_oversized_credential_files_are_refused_without_secret_or_mutation() {
    let fixture = Fixture::new("tenant-a", RUNNER_A).await;
    fixture.identity(RUNNER_A).await.unwrap();
    let before = fixture.snapshot();
    let public = fixture.token_file("public", RUNNER_A.as_bytes());
    fs::set_permissions(&public, fs::Permissions::from_mode(0o644)).unwrap();
    let target = fixture.token_file("private-target", RUNNER_A.as_bytes());
    let link = fixture.root.join("linked");
    symlink(&target, &link).unwrap();
    let oversized = fixture.token_file("oversized", "z".repeat(4097).as_bytes());
    for file in [public, link, oversized] {
        assert!(PrivateFile::open(&file, 4096).is_err());
        let output = fixture.diagnose(&file).await;
        assert_no_credential(&output, &[RUNNER_A, VIEWER_A, RUNNER_B, VIEWER_B]);
        assert!(!output.status.success());
        assert!(
            output.stdout.is_empty(),
            "unsafe input produced an identity"
        );
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(
            error.contains("credential") && error.contains("private"),
            "unsafe input must fail at the private credential boundary"
        );
    }
    assert!(
        fixture.snapshot() == before,
        "failed diagnostic mutated control state"
    );
}

#[tokio::test]
async fn credential_collision_in_real_control_identity_is_refused_without_masking() {
    const COLLIDING: &str = "qa-colliding-credential-ffffffffffffffffffffffff";
    let tenant = format!("tenant-{COLLIDING}");
    let fixture = Fixture::new(&tenant, COLLIDING).await;
    let live = fixture.identity(COLLIDING).await.unwrap();
    assert!(live.tenant_id.contains(COLLIDING));
    let before = fixture.snapshot();
    let file = fixture.token_file("collision", COLLIDING.as_bytes());
    let output = fixture.diagnose(&file).await;
    assert_no_credential(&output, &[COLLIDING, VIEWER_A, RUNNER_B, VIEWER_B]);
    assert!(!output.status.success());
    assert!(
        output.stdout.is_empty(),
        "sensitive identity must be refused, never rewritten"
    );
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains("credential") || error.contains("sensitive"),
        "sensitive identity requires an opaque rendering refusal"
    );
    assert!(
        fixture.snapshot() == before,
        "failed diagnostic mutated control state"
    );
}
