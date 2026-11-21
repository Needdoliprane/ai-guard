//! Library and CLI workspace-claim inspection against an owned loopback TLS
//! peer. Claim bodies are declared fixtures: they never qualify a provisioner,
//! ReadWriteOncePod exclusivity, capacity enforcement or any cluster effect.
mod support;
use agentsandbox::{
    control_fs::PrivateFile,
    runtime::kubernetes::{
        KubernetesAuthorityV1, KubernetesCredentials, KubernetesError, KubernetesRuntime,
    },
    tls::{ServerIdentity, serve},
};
use axum::{Router, body::Body, extract::State, http::Request, response::Response};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Output, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
};
use uuid::Uuid;

const CLUSTER_UID: &str = "11111111-1111-4111-8111-111111111111";
const NAMESPACE_UID: &str = "22222222-2222-4222-8222-222222222222";
const CLAIM_UID: &str = "55555555-5555-4555-8555-555555555555";
const TOKEN: &str = "public-session-claim-fixture-only-token-0000000000000";
const CLAIM: &str = "ai-guard-ws-0123456789abcdef0123456789abcdef";
const KUBE_SYSTEM: &str = "GET /api/v1/namespaces/kube-system";
const TENANT: &str = "GET /api/v1/namespaces/tenant-a";

struct Behavior {
    namespace_uid: String,
    namespace_uid_after_claim: Option<String>,
    pvc: Value,
    requests: Vec<String>,
}
struct Peer {
    root: tempfile::TempDir,
    ca: PathBuf,
    token: PathBuf,
    authority: KubernetesAuthorityV1,
    authority_file: PathBuf,
    state: Arc<Mutex<Behavior>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Peer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Peer {
    async fn new() -> Self {
        let root = support::private_tempdir();
        let root_path = root.path().canonicalize().unwrap();
        let pki = root_path.join("pki");
        let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_ai-guard-demo-pki"))
            .env_clear()
            .arg("--out")
            .arg(&pki)
            .arg("--dns")
            .arg("fixture.invalid")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(120), child.wait())
                .await
                .unwrap()
                .unwrap()
                .success(),
            "TLS fixture preparation failed"
        );
        let ca = root_path.join("ca.pem");
        private_write(&ca, &fs::read(pki.join("root-ca.pem")).unwrap());
        let token = root_path.join("bearer");
        private_write(&token, TOKEN.as_bytes());
        let identity =
            ServerIdentity::from_files(&pki.join("server-chain.pem"), &pki.join("server.key"))
                .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("https://{}", listener.local_addr().unwrap());
        let authority = KubernetesAuthorityV1::new(
            Uuid::from_u128(1),
            &endpoint,
            CLUSTER_UID,
            "tenant-a",
            NAMESPACE_UID,
            PrivateFile::open(&ca, 65536).unwrap().digest(),
        )
        .unwrap();
        let authority_file = root_path.join("authority.json");
        private_write(&authority_file, &serde_json::to_vec(&authority).unwrap());
        let state = Arc::new(Mutex::new(Behavior {
            namespace_uid: NAMESPACE_UID.into(),
            namespace_uid_after_claim: None,
            pvc: bound_pvc(),
            requests: Vec::new(),
        }));
        let app = Router::new().fallback(handler).with_state(state.clone());
        let task = tokio::spawn(async move {
            serve(listener, app, identity, std::future::pending::<()>())
                .await
                .unwrap();
        });
        Self {
            root,
            ca,
            token,
            authority,
            authority_file,
            state,
            task,
        }
    }
    fn runtime(&self) -> KubernetesRuntime {
        let credentials = KubernetesCredentials::bearer(
            PrivateFile::open(&self.ca, 65536).unwrap(),
            PrivateFile::open(&self.token, 8192).unwrap(),
        );
        KubernetesRuntime::connect(self.authority.clone(), credentials).unwrap()
    }
    async fn inspect_cli(&self, name: &str) -> Output {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
        command
            .env_clear()
            .current_dir(self.root.path())
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .args(["kubernetes", "inspect-workspace", "--claim-name", name])
            .arg("--authority-file")
            .arg(&self.authority_file)
            .arg("--ca-file")
            .arg(&self.ca)
            .arg("--token-file")
            .arg(&self.token);
        tokio::time::timeout(Duration::from_secs(50), command.output())
            .await
            .expect("workspace inspection CLI exceeded its test budget")
            .unwrap()
    }
    fn requests(&self) -> Vec<String> {
        self.state.lock().unwrap().requests.clone()
    }
}

fn private_write(path: &std::path::Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn bound_pvc() -> Value {
    json!({
        "apiVersion":"v1","kind":"PersistentVolumeClaim",
        "metadata":{"name":CLAIM,"namespace":"tenant-a","uid":CLAIM_UID,"resourceVersion":"77",
            "finalizers":["kubernetes.io/pvc-protection"]},
        "spec":{"accessModes":["ReadWriteOncePod"],"resources":{"requests":{"storage":"2Gi"}},
            "storageClassName":"csi-rwop","volumeMode":"Filesystem","volumeName":"pvc-fixture-a"},
        "status":{"phase":"Bound","accessModes":["ReadWriteOncePod"],"capacity":{"storage":"2Gi"}}
    })
}
async fn handler(State(state): State<Arc<Mutex<Behavior>>>, request: Request<Body>) -> Response {
    let authorized = request
        .headers()
        .get("authorization")
        .is_some_and(|value| value.as_bytes() == format!("Bearer {TOKEN}").as_bytes());
    let get = request.method() == "GET" && request.uri().query().is_none();
    let path = request.uri().path().to_owned();
    let mut state = state.lock().unwrap();
    state
        .requests
        .push(format!("{} {}", request.method(), request.uri()));
    if !authorized || !get {
        return Response::builder().status(405).body(Body::empty()).unwrap();
    }
    let body = match path.as_str() {
        "/api/v1/namespaces/kube-system" => json!({"apiVersion":"v1","kind":"Namespace",
            "metadata":{"name":"kube-system","uid":CLUSTER_UID},"status":{"phase":"Active"}}),
        "/api/v1/namespaces/tenant-a" => json!({"apiVersion":"v1","kind":"Namespace",
            "metadata":{"name":"tenant-a","uid":state.namespace_uid},"status":{"phase":"Active"}}),
        _ if path == format!("/api/v1/namespaces/tenant-a/persistentvolumeclaims/{CLAIM}") => {
            if let Some(namespace_uid) = state.namespace_uid_after_claim.take() {
                state.namespace_uid = namespace_uid;
            }
            state.pvc.clone()
        }
        _ => return Response::builder().status(404).body(Body::empty()).unwrap(),
    };
    Response::builder()
        .status(200)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

#[tokio::test]
async fn workspace_claim_inspection_uses_exact_gets_and_never_mutates() {
    let peer = Peer::new().await;
    let runtime = peer.runtime();
    let claim = runtime
        .inspect_session_workspace_claim(CLAIM)
        .await
        .unwrap();
    assert_eq!(claim.name(), CLAIM);
    assert_eq!(claim.namespace(), "tenant-a");
    assert_eq!(claim.uid(), CLAIM_UID);
    assert_eq!(claim.resource_version(), "77");
    assert_eq!(claim.storage_class_name(), "csi-rwop");
    assert_eq!(claim.volume_name(), "pvc-fixture-a");
    assert_eq!(claim.capacity(), "2Gi");
    assert_eq!(claim.access_mode(), "ReadWriteOncePod");
    assert_eq!(
        claim.authority_fingerprint(),
        runtime.binding().fingerprint()
    );
    assert!(chrono::DateTime::parse_from_rfc3339(claim.observed_at()).is_ok());
    let pvc = format!("GET /api/v1/namespaces/tenant-a/persistentvolumeclaims/{CLAIM}");
    assert_eq!(
        peer.requests(),
        [KUBE_SYSTEM, TENANT, pvc.as_str(), KUBE_SYSTEM, TENANT]
    );
    // A claim that is not exclusive to one Pod is a closed refusal.
    peer.state.lock().unwrap().pvc["spec"]["accessModes"] = json!(["ReadWriteOnce"]);
    assert_eq!(
        runtime
            .inspect_session_workspace_claim(CLAIM)
            .await
            .unwrap_err(),
        KubernetesError::InvalidResponse
    );
    // The refused projection stops before the closing anchors; only GETs.
    let requests = peer.requests();
    assert_eq!(requests[5..], [KUBE_SYSTEM, TENANT, pvc.as_str()]);
    assert!(requests.iter().all(|line| line.starts_with("GET ")));
}

#[tokio::test]
async fn workspace_claim_name_with_traversal_or_query_is_refused_before_any_request() {
    let peer = Peer::new().await;
    let runtime = peer.runtime();
    let long = "a".repeat(64);
    for name in [
        "../secrets/x",
        "a?b=c",
        "%2e%2e",
        "x/y",
        "A",
        long.as_str(),
        "",
        "-a",
        "a-",
        "a.b",
        "a b",
    ] {
        assert_eq!(
            runtime
                .inspect_session_workspace_claim(name)
                .await
                .unwrap_err(),
            KubernetesError::InvalidAuthority,
            "{name:?}"
        );
    }
    assert!(peer.requests().is_empty());
}

#[tokio::test]
async fn workspace_claim_inspection_refuses_a_changed_namespace_before_the_pvc_get() {
    let peer = Peer::new().await;
    peer.state.lock().unwrap().namespace_uid = "44444444-4444-4444-8444-444444444444".into();
    let runtime = peer.runtime();
    assert_eq!(
        runtime
            .inspect_session_workspace_claim(CLAIM)
            .await
            .unwrap_err(),
        KubernetesError::AuthorityChanged
    );
    let requests = peer.requests();
    assert_eq!(requests, [KUBE_SYSTEM, TENANT]);
    assert!(
        !requests
            .iter()
            .any(|line| line.contains("persistentvolumeclaims"))
    );
}

#[tokio::test]
async fn inspect_workspace_cli_reports_the_exact_claim_with_no_admission() {
    let peer = Peer::new().await;
    let output = peer.inspect_cli(CLAIM).await;
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let observed_at = report["claim"]["observed_at"].as_str().unwrap();
    assert!(chrono::DateTime::parse_from_rfc3339(observed_at).is_ok());
    assert_eq!(
        report,
        json!({
            "version": 1,
            "admission_available": false,
            "authority_id": peer.authority.authority_id(),
            "claim": {
                "version": 1,
                "authority_fingerprint": peer.authority.fingerprint(),
                "namespace": "tenant-a",
                "name": CLAIM,
                "uid": CLAIM_UID,
                "resource_version": "77",
                "storage_class_name": "csi-rwop",
                "volume_name": "pvc-fixture-a",
                "capacity": "2Gi",
                "access_mode": "ReadWriteOncePod",
                "observed_at": observed_at,
            },
        })
    );
    let pvc = format!("GET /api/v1/namespaces/tenant-a/persistentvolumeclaims/{CLAIM}");
    assert_eq!(
        peer.requests(),
        [KUBE_SYSTEM, TENANT, pvc.as_str(), KUBE_SYSTEM, TENANT]
    );
}

#[tokio::test]
async fn inspect_workspace_cli_distinguishes_storage_prerequisites() {
    let peer = Peer::new().await;
    let mut pending = bound_pvc();
    pending["status"] = json!({"phase": "Pending"});
    pending["spec"]
        .as_object_mut()
        .unwrap()
        .remove("volumeName");
    let mut unsupported_mode = bound_pvc();
    unsupported_mode["spec"]["accessModes"] = json!(["ReadWriteOnce"]);
    unsupported_mode["status"]["accessModes"] = json!(["ReadWriteOnce"]);
    let mut missing_class = bound_pvc();
    missing_class["spec"]
        .as_object_mut()
        .unwrap()
        .remove("storageClassName");
    let mut malformed_status_modes = pending.clone();
    malformed_status_modes["status"]["accessModes"] = json!("ReadWriteOncePod");
    let mut malformed_volume_name = pending.clone();
    malformed_volume_name["spec"]["volumeName"] = json!(1);
    let mut malformed_capacity = pending.clone();
    malformed_capacity["status"]["capacity"] = json!({"storage": "unknown"});
    let mut empty_mode = pending.clone();
    empty_mode["spec"]["accessModes"] = json!([""]);
    let mut unknown_mode = pending.clone();
    unknown_mode["status"]["accessModes"] = json!(["FutureMode"]);

    let invalid =
        "Kubernetes workspace inspection returned an invalid namespace or claim observation";
    let pvc = format!("GET /api/v1/namespaces/tenant-a/persistentvolumeclaims/{CLAIM}");
    let expected = [KUBE_SYSTEM, TENANT, pvc.as_str(), KUBE_SYSTEM, TENANT];
    for (claim, diagnostic, reads) in [
        (pending, "Kubernetes workspace claim is Pending", 5),
        (
            unsupported_mode,
            "Kubernetes workspace claim requires ReadWriteOncePod as its only access mode",
            5,
        ),
        (
            missing_class,
            "Kubernetes workspace claim has no named StorageClass",
            5,
        ),
        (malformed_status_modes, invalid, 3),
        (malformed_volume_name, invalid, 3),
        (malformed_capacity, invalid, 3),
        (empty_mode, invalid, 3),
        (unknown_mode, invalid, 3),
    ] {
        let before = peer.requests().len();
        peer.state.lock().unwrap().pvc = claim;
        let output = peer.inspect_cli(CLAIM).await;
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let error = std::str::from_utf8(&output.stderr).unwrap();
        assert!(error.contains(diagnostic), "expected {diagnostic}");
        assert!(!error.contains(TOKEN) && !error.contains("csi-rwop"));
        assert_eq!(peer.requests()[before..], expected[..reads]);
    }
}

#[tokio::test]
async fn inspect_workspace_cli_revalidates_authority_before_reporting_storage_prerequisites() {
    let peer = Peer::new().await;
    {
        let mut state = peer.state.lock().unwrap();
        state.pvc["status"] = json!({"phase": "Pending"});
        state.namespace_uid_after_claim = Some("44444444-4444-4444-8444-444444444444".into());
    }
    let output = peer.inspect_cli(CLAIM).await;
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error = std::str::from_utf8(&output.stderr).unwrap();
    assert!(error.contains("Kubernetes cluster or namespace identity differs"));
    assert!(!error.contains("Pending"));
    let pvc = format!("GET /api/v1/namespaces/tenant-a/persistentvolumeclaims/{CLAIM}");
    assert_eq!(
        peer.requests(),
        [KUBE_SYSTEM, TENANT, pvc.as_str(), KUBE_SYSTEM, TENANT]
    );
}

#[tokio::test]
async fn inspect_workspace_cli_refuses_an_invalid_name_before_any_request() {
    let peer = Peer::new().await;
    let output = peer.inspect_cli("InvalidFixtureName").await;
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error = std::str::from_utf8(&output.stderr).unwrap();
    assert!(error.contains("Kubernetes workspace claim name must be a lowercase DNS label"));
    assert!(!error.contains("InvalidFixtureName"));
    assert!(peer.requests().is_empty());
}

#[tokio::test]
async fn inspect_workspace_cli_refuses_a_protected_name_before_any_request() {
    let peer = Peer::new().await;
    let output = peer.inspect_cli(TOKEN).await;
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error = std::str::from_utf8(&output.stderr).unwrap();
    assert!(error.contains("workspace claim name contains protected credential material"));
    assert!(!error.contains(TOKEN));
    assert!(peer.requests().is_empty());
}

#[tokio::test]
async fn inspect_workspace_cli_refuses_a_claim_observation_that_would_echo_credentials() {
    let peer = Peer::new().await;
    peer.state.lock().unwrap().pvc["spec"]["storageClassName"] = json!(TOKEN);
    let output = peer.inspect_cli(CLAIM).await;
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error = std::str::from_utf8(&output.stderr).unwrap();
    assert!(error.contains("output contains protected credential material"));
    assert!(!error.contains(TOKEN));
    let pvc = format!("GET /api/v1/namespaces/tenant-a/persistentvolumeclaims/{CLAIM}");
    assert_eq!(
        peer.requests(),
        [KUBE_SYSTEM, TENANT, pvc.as_str(), KUBE_SYSTEM, TENANT]
    );
}
