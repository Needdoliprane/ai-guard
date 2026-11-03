//! Owned loopback TLS servers exercise the real adapter transport. Their Pod
//! responses are fixtures and never qualify cluster isolation or execution.
mod support;
use agentsandbox::{
    control_fs::PrivateFile,
    runtime::kubernetes::{
        KubernetesAdmissionGapV1, KubernetesAuthorityV1, KubernetesCredentials, KubernetesError,
        KubernetesPodHandle, KubernetesRuntime,
    },
    tls::{ServerIdentity, serve},
};
use axum::{Router, body::Body, extract::State, http::Request, response::Response};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use uuid::Uuid;

const CLUSTER_UID: &str = "11111111-1111-4111-8111-111111111111";
const NAMESPACE_UID: &str = "22222222-2222-4222-8222-222222222222";
const POD_UID: &str = "33333333-3333-4333-8333-333333333333";
const TOKEN: &str = "public-kubernetes-fixture-only-token-000000000000";
const PRIVATE_CANARY: &str = "private-body-must-not-enter-error-or-debug";

#[derive(Clone)]
enum Mode {
    Normal,
    Status(u16),
    Redirect(String),
    Large,
    Slow,
}
struct Behavior {
    mode: Mode,
    cluster_uid: String,
    namespace_uid: String,
    pod: Value,
    requests: usize,
    credentials_valid: bool,
}
struct Fixture {
    root: tempfile::TempDir,
    ca: PathBuf,
    token: PathBuf,
    endpoint: String,
    state: Arc<Mutex<Behavior>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Fixture {
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
            mode: Mode::Normal,
            cluster_uid: CLUSTER_UID.into(),
            namespace_uid: NAMESPACE_UID.into(),
            pod: pod(),
            requests: 0,
            credentials_valid: true,
        }));
        let app = Router::new()
            .fallback(endpoint_handler)
            .with_state(state.clone());
        let task = tokio::spawn(async move {
            serve(listener, app, identity, std::future::pending::<()>())
                .await
                .unwrap();
        });
        Self {
            root,
            ca,
            token,
            endpoint,
            state,
            task,
        }
    }
    fn authority(&self) -> KubernetesAuthorityV1 {
        binding(&self.endpoint, &self.ca)
    }
    fn runtime(&self) -> KubernetesRuntime {
        KubernetesRuntime::connect(self.authority(), credentials(&self.ca, &self.token)).unwrap()
    }
    fn handle(&self) -> KubernetesPodHandle {
        let spec = self.state.lock().unwrap().pod["spec"].clone();
        KubernetesPodHandle::from_control_checkpoint(
            &self.authority(),
            Uuid::from_u128(7),
            Uuid::from_u128(8),
            "worker-a",
            POD_UID,
            &KubernetesPodHandle::configuration_digest(&spec).unwrap(),
        )
        .unwrap()
    }
}
fn credentials(ca: &Path, token: &Path) -> KubernetesCredentials {
    KubernetesCredentials::bearer(
        PrivateFile::open(ca, 65536).unwrap(),
        PrivateFile::open(token, 8192).unwrap(),
    )
}
fn binding(endpoint: &str, ca: &Path) -> KubernetesAuthorityV1 {
    KubernetesAuthorityV1::new(
        Uuid::from_u128(1),
        endpoint,
        CLUSTER_UID,
        "tenant-a",
        NAMESPACE_UID,
        PrivateFile::open(ca, 65536).unwrap().digest(),
    )
    .unwrap()
}
fn pod() -> Value {
    json!({"apiVersion":"v1", "kind":"Pod",
        "metadata":{"name":"worker-a", "namespace":"tenant-a", "uid":POD_UID,"resourceVersion":"42"},
        "spec":{"nodeName":"node-a","containers":[{"name":"worker","image":"fixture:only",
            "resources":{"limits":{"cpu":"2","memory":"1Gi"}}}]},
        "status":{"phase":"Succeeded","containerStatuses":[{"name":"worker", "restartCount":0,
            "containerID":"containerd://abc123", "imageID":"sha256:abc123",
            "state":{"terminated":{"exitCode":7,"startedAt":"2026-11-03T10:00:00Z", "finishedAt":"2026-11-03T10:00:01Z", "reason":"Completed"}}}]}})
}
async fn endpoint_handler(
    State(state): State<Arc<Mutex<Behavior>>>,
    request: Request<Body>,
) -> Response {
    let (mode, body) = {
        let mut state = state.lock().unwrap();
        state.requests += 1;
        state.credentials_valid &= request.method() == "GET"
            && request
                .headers()
                .get("authorization")
                .is_some_and(|value| value.as_bytes() == format!("Bearer {TOKEN}").as_bytes());
        let body = match request.uri().path() {
            "/version" => json!({"gitVersion":"v1.36.1"}),
            "/api/v1/namespaces/kube-system" => {
                json!({"apiVersion":"v1","kind":"Namespace","metadata":{"name":"kube-system","uid":state.cluster_uid}})
            }
            "/api/v1/namespaces/tenant-a" => {
                json!({"apiVersion":"v1","kind":"Namespace","metadata":{"name":"tenant-a","uid":state.namespace_uid,"labels":{"pod-security.kubernetes.io/enforce":"restricted"}}})
            }
            "/api/v1/namespaces/tenant-a/pods/worker-a" => state.pod.clone(),
            _ => return Response::builder().status(404).body(Body::empty()).unwrap(),
        };
        (state.mode.clone(), body)
    };
    match mode {
        Mode::Normal => Response::builder()
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap(),
        Mode::Status(status) => Response::builder()
            .status(status)
            .body(Body::from(PRIVATE_CANARY))
            .unwrap(),
        Mode::Redirect(target) => Response::builder()
            .status(302)
            .header("location", target)
            .body(Body::from(PRIVATE_CANARY))
            .unwrap(),
        Mode::Large => {
            // No content-length: enforce the bound while reading actual chunks.
            let stream = async_stream::stream! {
                for _ in 0..17 { yield Ok::<_, std::io::Error>(vec![b'x'; 65536]); }
            };
            Response::new(Body::from_stream(stream))
        }
        Mode::Slow => {
            tokio::time::sleep(Duration::from_secs(10)).await;
            Response::new(Body::from("{}"))
        }
    }
}

#[tokio::test]
async fn authenticated_observations_bind_namespace_cluster_pod_and_configuration() {
    let fixture = Fixture::new().await;
    let runtime = fixture.runtime();
    let handle = fixture.handle();
    let diagnostic = runtime.doctor().await.unwrap();
    assert_eq!(diagnostic.server_version, "v1.36.1");
    assert_eq!(
        diagnostic.namespace_pod_security_enforce_label.as_deref(),
        Some("restricted")
    );
    assert!(
        diagnostic
            .admission_gaps
            .contains(&KubernetesAdmissionGapV1::NetworkEnforcementNotProven)
    );
    assert!(
        diagnostic
            .admission_gaps
            .iter()
            .all(|gap| !gap.reason().is_empty())
    );
    let observed = runtime.observe_owned(&handle).await.unwrap();
    assert_eq!(observed.process.exit_code, Some(7));
    assert_eq!(observed.process.restart_count, Some(0));
    assert_eq!(observed.configured_limits.cpu.as_deref(), Some("2"));
    assert_eq!(observed.configured_limits.memory.as_deref(), Some("1Gi"));
    assert!(observed.process.finished_at.unwrap() > observed.process.started_at.unwrap());
    assert!(fixture.state.lock().unwrap().credentials_valid);
    let mut altered_handle = serde_json::to_value(&handle).unwrap();
    altered_handle["operation"] = json!(Uuid::nil());
    let altered_handle: KubernetesPodHandle = serde_json::from_value(altered_handle).unwrap();
    let before = fixture.state.lock().unwrap().requests;
    assert_eq!(
        runtime.observe_owned(&altered_handle).await.unwrap_err(),
        KubernetesError::PodChanged
    );
    assert_eq!(
        fixture.state.lock().unwrap().requests,
        before,
        "invalid checkpoint reached the API"
    );
    fixture.state.lock().unwrap().namespace_uid = POD_UID.into();
    assert_eq!(
        runtime.observe_owned(&handle).await.unwrap_err(),
        KubernetesError::AuthorityChanged
    );
    fixture.state.lock().unwrap().namespace_uid = NAMESPACE_UID.into();
    fixture.state.lock().unwrap().cluster_uid = POD_UID.into();
    assert_eq!(
        runtime.doctor().await.unwrap_err(),
        KubernetesError::AuthorityChanged
    );
    fixture.state.lock().unwrap().cluster_uid = CLUSTER_UID.into();
    fixture.state.lock().unwrap().pod["metadata"]["uid"] = json!(CLUSTER_UID);
    assert_eq!(
        runtime.observe_owned(&handle).await.unwrap_err(),
        KubernetesError::PodChanged
    );
    fixture.state.lock().unwrap().pod = pod();
    fixture.state.lock().unwrap().pod["spec"]["hostNetwork"] = json!(true);
    assert_eq!(
        runtime.observe_owned(&handle).await.unwrap_err(),
        KubernetesError::PodChanged
    );
    fixture.state.lock().unwrap().pod = pod();
    // Pod phase does not supply missing process status, exit code or timestamps.
    fixture.state.lock().unwrap().pod["status"] = json!({"phase":"Succeeded"});
    let unknown = runtime.observe_owned(&handle).await.unwrap();
    assert_eq!(unknown.process.exit_code, None);
    assert_eq!(unknown.process.started_at, None);
    assert_eq!(unknown.process.finished_at, None);
    assert_eq!(unknown.process.restart_count, None);
    fixture.state.lock().unwrap().pod = pod();
    fixture.state.lock().unwrap().pod["status"]["containerStatuses"][0]["state"]["terminated"]
        .as_object_mut()
        .unwrap()
        .remove("exitCode");
    assert_eq!(
        runtime
            .observe_owned(&handle)
            .await
            .unwrap()
            .process
            .exit_code,
        None
    );
    fixture.state.lock().unwrap().pod = pod();
    fixture.state.lock().unwrap().pod["status"]["containerStatuses"][0]["restartCount"] = json!(1);
    assert_eq!(
        runtime
            .observe_owned(&handle)
            .await
            .unwrap()
            .process
            .restart_count,
        Some(1)
    );
    fixture.state.lock().unwrap().pod["status"]["containerStatuses"][0]["state"]["terminated"]["finishedAt"] =
        json!("2026-11-03T09:59:00Z");
    assert_eq!(
        runtime.observe_owned(&handle).await.unwrap_err(),
        KubernetesError::InvalidResponse
    );
    fixture.state.lock().unwrap().pod = pod();
    fixture.state.lock().unwrap().pod["status"]["containerStatuses"][0]["state"]["running"] =
        json!({});
    assert_eq!(
        runtime.observe_owned(&handle).await.unwrap_err(),
        KubernetesError::InvalidResponse
    );
    fixture.state.lock().unwrap().pod = pod();
    fixture.state.lock().unwrap().pod["spec"]["ephemeralContainers"] = json!([{"name":"injected"}]);
    let injected = fixture.handle();
    assert_eq!(
        runtime.observe_owned(&injected).await.unwrap_err(),
        KubernetesError::PodChanged
    );
}

#[tokio::test]
async fn https_failures_are_bounded_opaque_and_not_retried_or_redirected() {
    let fixture = Fixture::new().await;
    let runtime = fixture.runtime();
    let wrong_host = fixture.endpoint.replace("127.0.0.1", "localhost");
    let wrong_host_runtime = KubernetesRuntime::connect(
        binding(&wrong_host, &fixture.ca),
        credentials(&fixture.ca, &fixture.token),
    )
    .unwrap();
    assert_eq!(
        wrong_host_runtime.doctor().await.unwrap_err(),
        KubernetesError::TransportUnavailable
    );
    assert_eq!(
        fixture.state.lock().unwrap().requests,
        0,
        "hostname failure sent an HTTP credential"
    );
    let other = Fixture::new().await;
    let wrong_root = KubernetesRuntime::connect(
        binding(&fixture.endpoint, &other.ca),
        credentials(&other.ca, &fixture.token),
    )
    .unwrap();
    assert_eq!(
        wrong_root.doctor().await.unwrap_err(),
        KubernetesError::TransportUnavailable
    );
    assert_eq!(fixture.state.lock().unwrap().requests, 0);
    assert!(matches!(
        KubernetesRuntime::connect(fixture.authority(), credentials(&other.ca, &fixture.token)),
        Err(KubernetesError::InvalidCredentials)
    ));
    for status in [401, 403, 404, 429, 503] {
        fixture.state.lock().unwrap().mode = Mode::Status(status);
        let before = fixture.state.lock().unwrap().requests;
        let error = runtime.doctor().await.unwrap_err();
        assert_eq!(error, KubernetesError::ApiRejected(status));
        assert_eq!(fixture.state.lock().unwrap().requests, before + 1);
        assert!(!format!("{error:?} {error} {runtime:?}").contains(PRIVATE_CANARY));
        assert!(!format!("{error:?} {error} {runtime:?}").contains(TOKEN));
    }
    fixture.state.lock().unwrap().mode = Mode::Redirect(format!("{}/version", other.endpoint));
    assert_eq!(
        runtime.doctor().await.unwrap_err(),
        KubernetesError::ApiRejected(302)
    );
    assert_eq!(
        other.state.lock().unwrap().requests,
        0,
        "redirect destination received credentials"
    );
    fixture.state.lock().unwrap().mode = Mode::Large;
    assert_eq!(
        runtime.doctor().await.unwrap_err(),
        KubernetesError::ResponseTooLarge
    );
    fixture.state.lock().unwrap().mode = Mode::Slow;
    assert_eq!(
        runtime.doctor().await.unwrap_err(),
        KubernetesError::Timeout
    );
    fixture.state.lock().unwrap().mode = Mode::Normal;
    fs::write(
        &fixture.token,
        "replaced-private-credential-0000000000000000",
    )
    .unwrap();
    let before = fixture.state.lock().unwrap().requests;
    assert_eq!(
        runtime.doctor().await.unwrap_err(),
        KubernetesError::InvalidCredentials
    );
    assert_eq!(fixture.state.lock().unwrap().requests, before);
}

#[tokio::test]
async fn transport_capacity_is_bounded_and_released_on_cancellation() {
    let fixture = Fixture::new().await;
    fixture.state.lock().unwrap().mode = Mode::Slow;
    let runtime = Arc::new(fixture.runtime());
    let mut clients = Vec::new();
    for _ in 0..4 {
        let runtime = runtime.clone();
        clients.push(tokio::spawn(async move { runtime.doctor().await }));
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        while fixture.state.lock().unwrap().requests < 4 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(runtime.doctor().await.unwrap_err(), KubernetesError::Busy);
    for client in clients {
        client.abort();
        let _ = client.await;
    }
    fixture.state.lock().unwrap().mode = Mode::Normal;
    assert!(runtime.doctor().await.is_ok());
}

#[tokio::test]
async fn ambient_proxy_variables_are_ignored_by_the_real_client() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let fixture = Fixture::new().await;
    let proxy = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_url = format!("http://{}", proxy.local_addr().unwrap());
    let hits = Arc::new(AtomicUsize::new(0));
    let recorded = hits.clone();
    let proxy_task = tokio::spawn(async move {
        while let Ok((mut stream, _)) = proxy.accept().await {
            recorded.fetch_add(1, Ordering::SeqCst);
            use tokio::io::AsyncWriteExt;
            let _ = stream
                .write_all(
                    b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await;
        }
    });
    let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
        .env_clear()
        .args(["--exact", "proxy_environment_child", "--ignored"])
        .env("AI_GUARD_KUBE_FIXTURE_ENDPOINT", &fixture.endpoint)
        .env("AI_GUARD_KUBE_FIXTURE_CA", &fixture.ca)
        .env("AI_GUARD_KUBE_FIXTURE_TOKEN", &fixture.token)
        .env("HTTPS_PROXY", &proxy_url)
        .env("https_proxy", &proxy_url)
        .env("HTTP_PROXY", &proxy_url)
        .env("http_proxy", &proxy_url)
        .env("ALL_PROXY", &proxy_url)
        .env("all_proxy", &proxy_url)
        .env("NO_PROXY", "")
        .env("no_proxy", "")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(15), child.wait()).await;
    proxy_task.abort();
    assert!(
        result.unwrap().unwrap().success(),
        "isolated adapter proxy test child failed"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.state.lock().unwrap().requests, 3);
    assert!(fixture.state.lock().unwrap().credentials_valid);
    assert!(fixture.root.path().exists());
}

#[tokio::test]
#[ignore = "internal child; invoked by ambient_proxy_variables_are_ignored_by_the_real_client"]
async fn proxy_environment_child() {
    let endpoint = std::env::var("AI_GUARD_KUBE_FIXTURE_ENDPOINT").unwrap();
    let ca = PathBuf::from(std::env::var_os("AI_GUARD_KUBE_FIXTURE_CA").unwrap());
    let token = PathBuf::from(std::env::var_os("AI_GUARD_KUBE_FIXTURE_TOKEN").unwrap());
    let runtime =
        KubernetesRuntime::connect(binding(&endpoint, &ca), credentials(&ca, &token)).unwrap();
    assert_eq!(runtime.doctor().await.unwrap().server_version, "v1.36.1");
}
