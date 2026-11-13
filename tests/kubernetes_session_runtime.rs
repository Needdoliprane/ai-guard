//! Public read-only workspace-claim inspection against an owned loopback TLS
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
    process::Stdio,
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
    pvc: Value,
    requests: Vec<String>,
}
struct Peer {
    _root: tempfile::TempDir,
    ca: PathBuf,
    token: PathBuf,
    endpoint: String,
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
        fs::write(&ca, fs::read(pki.join("root-ca.pem")).unwrap()).unwrap();
        fs::set_permissions(&ca, fs::Permissions::from_mode(0o600)).unwrap();
        let token = root_path.join("bearer");
        fs::write(&token, TOKEN).unwrap();
        fs::set_permissions(&token, fs::Permissions::from_mode(0o600)).unwrap();
        let identity =
            ServerIdentity::from_files(&pki.join("server-chain.pem"), &pki.join("server.key"))
                .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("https://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(Behavior {
            namespace_uid: NAMESPACE_UID.into(),
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
            _root: root,
            ca,
            token,
            endpoint,
            state,
            task,
        }
    }
    fn runtime(&self) -> KubernetesRuntime {
        let ca = PrivateFile::open(&self.ca, 65536).unwrap();
        let authority = KubernetesAuthorityV1::new(
            Uuid::from_u128(1),
            &self.endpoint,
            CLUSTER_UID,
            "tenant-a",
            NAMESPACE_UID,
            ca.digest(),
        )
        .unwrap();
        let credentials =
            KubernetesCredentials::bearer(ca, PrivateFile::open(&self.token, 8192).unwrap());
        KubernetesRuntime::connect(authority, credentials).unwrap()
    }
    fn requests(&self) -> Vec<String> {
        self.state.lock().unwrap().requests.clone()
    }
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
