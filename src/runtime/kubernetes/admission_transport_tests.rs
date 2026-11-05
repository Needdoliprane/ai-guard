//! Local held-CREATE transport tests. SQLite and the private journal are real;
//! the TLS peer returns declared fixture facts and never launches a Pod.
use super::{journal::*, permission::*, *};
use crate::{
    audit::kubernetes_admission_state_tests::Fixture as StoreFixture,
    broker::clock::{ClockError, ClockResult, HostClock},
    control::{HostClockMethodV1, HostClockSampleV1, QuotaPolicyV1, kubernetes_admission::*},
    control_fs::{Directory, PrivateFile, TrustedExecutable},
    runtime::kubernetes::{KubernetesAuthorityV1, KubernetesCredentials, KubernetesRuntime},
    tls::{ServerIdentity, serve},
};
use axum::{Router, body::Body, extract::State, http::Request, response::Response};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    fs,
    net::TcpListener as StdTcpListener,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex, atomic::AtomicBool},
    time::Duration,
};
use tokio::{net::TcpListener, task::JoinHandle};
use uuid::Uuid;

const TOKEN: &str = "public-held-create-loopback-token-000000000000000000";
pub(super) const EXPIRY: &str = "2100-01-01T00:00:00.000Z";
pub(super) const CLUSTER_UID: &str = "11111111-1111-4111-8111-111111111111";
pub(super) const NAMESPACE_UID: &str = "22222222-2222-4222-8222-222222222222";
pub(super) const POD_UID: &str = "33333333-3333-4333-8333-333333333333";
pub(super) const CHANGED_NAMESPACE_UID: &str = "44444444-4444-4444-8444-444444444444";
const PRIVATE_CANARY: &str = "private-api-body-must-not-escape";

pub(crate) struct TlsMaterial {
    _root: tempfile::TempDir,
    pub(crate) certificate: PathBuf,
    pub(crate) key: PathBuf,
    pub(crate) ca: PathBuf,
}
impl TlsMaterial {
    pub(crate) async fn create() -> Self {
        let root = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .prefix("held-create-tls-")
            .tempdir()
            .expect("create private loopback TLS fixture directory");
        let certificate = root.path().join("server-chain.pem");
        let leaf_certificate = root.path().join("server-leaf.pem");
        let key = root.path().join("server.key");
        let ca = root.path().join("root-ca.pem");
        let ca_key = root.path().join("root-ca.key");
        let csr = root.path().join("server.csr");
        let extensions = root.path().join("server.ext");
        private_write(
            &extensions,
            b"subjectAltName=IP:127.0.0.1\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n",
        );
        run_openssl(
            root.path(),
            [
                "req",
                "-x509",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-sha256",
                "-days",
                "30",
                "-subj",
                "/CN=held-create-test-root",
                "-addext",
                "basicConstraints=critical,CA:TRUE,pathlen:0",
                "-addext",
                "keyUsage=critical,digitalSignature,keyCertSign,cRLSign",
                "-keyout",
            ]
            .into_iter()
            .chain([ca_key.to_str().unwrap()])
            .chain(["-out", ca.to_str().unwrap()]),
        )
        .await;
        run_openssl(
            root.path(),
            [
                "req",
                "-new",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-sha256",
                "-subj",
                "/CN=127.0.0.1",
                "-keyout",
            ]
            .into_iter()
            .chain([key.to_str().unwrap()])
            .chain(["-out", csr.to_str().unwrap()]),
        )
        .await;
        run_openssl(
            root.path(),
            ["x509", "-req", "-sha256", "-days", "30", "-in"]
                .into_iter()
                .chain([csr.to_str().unwrap()])
                .chain([
                    "-CA",
                    ca.to_str().unwrap(),
                    "-CAkey",
                    ca_key.to_str().unwrap(),
                ])
                .chain([
                    "-CAcreateserial",
                    "-extfile",
                    extensions.to_str().unwrap(),
                    "-out",
                ])
                .chain([leaf_certificate.to_str().unwrap()]),
        )
        .await;
        let mut chain = fs::read(&leaf_certificate).expect("read signed loopback leaf");
        chain.extend_from_slice(b"\n");
        chain.extend_from_slice(&fs::read(&ca).expect("read private loopback root certificate"));
        private_write(&certificate, &chain);
        for path in [&certificate, &leaf_certificate, &key, &ca, &ca_key, &csr] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))
                .expect("make fixture PEM private");
        }
        let trusted_ca = root.path().join("trust.pem");
        private_write(
            &trusted_ca,
            &fs::read(&ca).expect("read fixture root certificate"),
        );
        Self {
            _root: root,
            certificate,
            key,
            ca: trusted_ca,
        }
    }

    fn credentials(&self, token: &Path) -> KubernetesCredentials {
        KubernetesCredentials::bearer(private(&self.ca), private(token))
    }

    fn trust_digest(&self) -> String {
        private(&self.ca).digest().to_owned()
    }
}

async fn run_openssl<'a>(root: &Path, args: impl IntoIterator<Item = &'a str>) {
    let mut command = tokio::process::Command::new(
        TrustedExecutable::open(Path::new("/usr/bin/openssl"))
            .expect("sandbox OpenSSL is required for the private TLS fixture")
            .path(),
    );
    command
        .env_clear()
        .current_dir(root)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .expect("start the sandbox-local TLS fixture generator");
    let status = match tokio::time::timeout(Duration::from_secs(30), child.wait()).await {
        Ok(Ok(status)) => status,
        _ => {
            let _ = child.kill().await;
            panic!("sandbox-local TLS fixture generation did not finish");
        }
    };
    assert!(
        status.success(),
        "sandbox-local TLS fixture generation failed"
    );
}

#[derive(Clone, Copy)]
pub(super) enum ResponseMode {
    Created,
    ChangedNamespaceAfterCreate,
    Status(u16),
    DuplicateJson,
    Oversized,
    ChangedProfile,
    TruncatedBody,
}
pub(super) struct RequestSeen {
    pub(super) uri: String,
    pub(super) method: String,
    pub(super) content_type_is_json: bool,
    pub(super) bearer_matches: bool,
    pub(super) body: Vec<u8>,
}
struct PeerState {
    mode: ResponseMode,
    mode_post_start: usize,
    posts: Vec<RequestSeen>,
    read_requests: usize,
    unexpected_requests: usize,
}
pub(super) struct Peer {
    pub(super) endpoint: String,
    state: Arc<Mutex<PeerState>>,
    task: JoinHandle<()>,
    credential_root: tempfile::TempDir,
    token: PathBuf,
    pub(super) activation_listener: StdTcpListener,
    material: TlsMaterial,
}
impl Drop for Peer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Peer {
    pub(super) async fn new(mode: ResponseMode) -> Self {
        let material = TlsMaterial::create().await;
        let identity = ServerIdentity::from_files(&material.certificate, &material.key)
            .expect("load private loopback server identity");
        let credential_root = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .prefix("held-create-credential-")
            .tempdir()
            .expect("create a private bearer fixture directory");
        let token = credential_root.path().join("bearer");
        private_write(&token, TOKEN.as_bytes());
        let listener = StdTcpListener::bind("127.0.0.1:0").expect("bind loopback-only API fixture");
        listener
            .set_nonblocking(true)
            .expect("make the owned listener nonblocking");
        let endpoint = format!("https://{}", listener.local_addr().unwrap());
        let activation_listener = listener
            .try_clone()
            .expect("retain the exact listener proof");
        let tls_listener = TcpListener::from_std(listener).expect("adopt the bound listener");
        let state = Arc::new(Mutex::new(PeerState {
            mode,
            mode_post_start: 0,
            posts: Vec::new(),
            read_requests: 0,
            unexpected_requests: 0,
        }));
        let app = Router::new()
            .fallback(api_handler)
            .with_state(state.clone());
        let task = tokio::spawn(async move {
            serve(tls_listener, app, identity, std::future::pending::<()>())
                .await
                .expect("owned TLS fixture listener remains available");
        });
        Self {
            endpoint,
            state,
            task,
            credential_root,
            token,
            activation_listener,
            material,
        }
    }

    pub(super) fn set_mode(&self, mode: ResponseMode) {
        let mut state = self.state.lock().unwrap();
        state.mode = mode;
        state.mode_post_start = state.posts.len();
    }

    pub(super) fn trust_digest(&self) -> String {
        self.material.trust_digest()
    }

    pub(super) fn credentials(&self) -> KubernetesCredentials {
        self.material.credentials(&self.token)
    }

    pub(super) fn post_count(&self) -> usize {
        self.state.lock().unwrap().posts.len()
    }

    pub(super) fn read_request_count(&self) -> usize {
        self.state.lock().unwrap().read_requests
    }

    pub(super) fn latest_post(&self) -> RequestSeen {
        let state = self.state.lock().unwrap();
        assert_eq!(
            state.unexpected_requests, 0,
            "fixture received an unexpected API request"
        );
        let request = state.posts.last().expect("expected one held CREATE POST");
        RequestSeen {
            uri: request.uri.clone(),
            method: request.method.clone(),
            content_type_is_json: request.content_type_is_json,
            bearer_matches: request.bearer_matches,
            body: request.body.clone(),
        }
    }

    fn replace_credential(&self) {
        let replacement = self.credential_root.path().join("bearer.replacement");
        private_write(
            &replacement,
            b"public-held-create-replaced-credential-000000000000",
        );
        fs::rename(replacement, &self.token).expect("replace only this test's private credential");
    }

    fn reset_credential(&self) {
        let replacement = self.credential_root.path().join("bearer.reset");
        private_write(&replacement, TOKEN.as_bytes());
        fs::rename(replacement, &self.token).expect("restore only this test's private credential");
    }
}

async fn api_handler(
    State(state): State<Arc<Mutex<PeerState>>>,
    request: Request<Body>,
) -> Response {
    let uri = request.uri().to_string();
    let method = request.method().as_str().to_owned();
    let content_type_is_json = request
        .headers()
        .get("content-type")
        .is_some_and(|value| value == "application/json");
    let bearer_matches = request
        .headers()
        .get("authorization")
        .is_some_and(|value| value.as_bytes() == format!("Bearer {TOKEN}").as_bytes());
    let is_create =
        method == "POST" && uri == "/api/v1/namespaces/tenant-a/pods?fieldValidation=Strict";
    let changed_namespace_uid = if method == "GET" && uri == "/api/v1/namespaces/tenant-a" {
        let state = state.lock().unwrap();
        matches!(state.mode, ResponseMode::ChangedNamespaceAfterCreate)
            && state.posts.len() > state.mode_post_start
    } else {
        false
    };
    let doctor_response = match (method.as_str(), uri.as_str()) {
        ("GET", "/api/v1/namespaces/kube-system") => Some(json!({
            "apiVersion": "v1",
            "kind": "Namespace",
            "metadata": {"name": "kube-system", "uid": CLUSTER_UID}
        })),
        ("GET", "/api/v1/namespaces/tenant-a") => Some(json!({
            "apiVersion": "v1",
            "kind": "Namespace",
            "metadata": {
                "name": "tenant-a",
                "uid": if changed_namespace_uid { CHANGED_NAMESPACE_UID } else { NAMESPACE_UID },
                "labels": {"pod-security.kubernetes.io/enforce": "restricted"}
            }
        })),
        ("GET", "/version") => Some(json!({"gitVersion": "v1.36.1"})),
        _ => None,
    };
    let body = match axum::body::to_bytes(request.into_body(), 64 * 1024).await {
        Ok(body) => body.to_vec(),
        Err(_) => {
            return Response::builder().status(400).body(Body::empty()).unwrap();
        }
    };
    let mode = {
        let mut state = state.lock().unwrap();
        if is_create && !bearer_matches {
            state.unexpected_requests += 1;
            return Response::builder().status(401).body(Body::empty()).unwrap();
        } else if is_create {
            state.posts.push(RequestSeen {
                uri,
                method,
                content_type_is_json,
                bearer_matches,
                body: body.clone(),
            });
            Some(state.mode)
        } else if doctor_response.is_some() && bearer_matches {
            state.read_requests += 1;
            None
        } else {
            state.unexpected_requests += 1;
            return Response::builder().status(404).body(Body::empty()).unwrap();
        }
    };
    let Some(mode) = mode else {
        return Response::builder()
            .status(200)
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&doctor_response.unwrap()).unwrap(),
            ))
            .unwrap();
    };
    match mode {
        ResponseMode::ChangedNamespaceAfterCreate => {
            let mut pod: Value = serde_json::from_slice(&body).unwrap();
            pod["metadata"]["uid"] = json!(POD_UID);
            pod["metadata"]["resourceVersion"] = json!("42");
            Response::builder()
                .status(201)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&pod).unwrap()))
                .unwrap()
        }
        ResponseMode::Status(status) => Response::builder()
            .status(status)
            .header("content-type", "application/json")
            .body(Body::from(PRIVATE_CANARY))
            .unwrap(),
        ResponseMode::DuplicateJson => Response::builder()
            .status(201)
            .header("content-type", "application/json")
            .body(Body::from(
                br#"{"apiVersion":"v1","apiVersion":"v1"}"#.to_vec(),
            ))
            .unwrap(),
        ResponseMode::Oversized => Response::builder()
            .status(201)
            .header("content-type", "application/json")
            .body(Body::from(vec![b' '; 1024 * 1024 + 1]))
            .unwrap(),
        ResponseMode::ChangedProfile => {
            let mut pod: Value = serde_json::from_slice(&body).unwrap();
            pod["metadata"]["uid"] = json!(POD_UID);
            pod["metadata"]["resourceVersion"] = json!("42");
            pod["spec"]["hostNetwork"] = json!(true);
            Response::builder()
                .status(201)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&pod).unwrap()))
                .unwrap()
        }
        ResponseMode::TruncatedBody => Response::builder()
            .status(201)
            .header("content-type", "application/json")
            .body(Body::from_stream(futures_util::stream::iter([
                Ok::<Vec<u8>, std::io::Error>(b"{\"apiVersion\":\"v1\"".to_vec()),
                Err(std::io::Error::other("fixture truncates the response")),
            ])))
            .unwrap(),
        ResponseMode::Created => {
            let mut pod: Value = serde_json::from_slice(&body).unwrap();
            pod["metadata"]["uid"] = json!(POD_UID);
            pod["metadata"]["resourceVersion"] = json!("42");
            Response::builder()
                .status(201)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&pod).unwrap()))
                .unwrap()
        }
    }
}

pub(super) struct TestClock {
    samples: Mutex<VecDeque<HostClockSampleV1>>,
    last: HostClockSampleV1,
}
impl TestClock {
    pub(super) fn repeated(sample: HostClockSampleV1) -> Arc<Self> {
        Arc::new(Self {
            samples: Mutex::new(VecDeque::new()),
            last: sample,
        })
    }

    pub(super) fn sequence(samples: impl IntoIterator<Item = HostClockSampleV1>) -> Arc<Self> {
        let samples = samples.into_iter().collect::<VecDeque<_>>();
        let last = samples
            .back()
            .cloned()
            .expect("synthetic test clock requires at least one sample");
        Arc::new(Self {
            samples: Mutex::new(samples),
            last,
        })
    }
}
impl HostClock for TestClock {
    fn sample(&self) -> ClockResult<HostClockSampleV1> {
        Ok(self
            .samples
            .lock()
            .map_err(|_| ClockError::InvalidReading)?
            .pop_front()
            .unwrap_or_else(|| self.last.clone()))
    }
}

pub(super) fn sample(boot_id: &str, method: HostClockMethodV1, ns: u128) -> HostClockSampleV1 {
    HostClockSampleV1 {
        version: 1,
        method,
        boot_id: boot_id.into(),
        continuous_ns_decimal: ns.to_string(),
        observed_utc: None,
    }
}

struct CallResult {
    outcome: KubernetesResult<KubernetesCreateOutcomeV1>,
    post_count: usize,
    preclaim_doctor_requests: usize,
    runtime_probe_requests: usize,
    unexpected_requests: usize,
    request: Option<RequestSeen>,
    expected_request: Vec<u8>,
    handle_uid: Option<String>,
    held_workers: u32,
    canary_persisted: bool,
}

async fn perform(
    peer: &Peer,
    mode: ResponseMode,
    clock: Arc<dyn HostClock>,
    cancelled: bool,
    replace_credential: bool,
) -> CallResult {
    peer.reset_credential();
    peer.set_mode(mode);
    let posts_before = peer.post_count();
    let reads_before = peer.read_request_count();
    let unexpected_before = peer.state.lock().unwrap().unexpected_requests;
    let store_fixture = StoreFixture::new();
    let mut intent = store_fixture.intent.clone();
    let authority_id = intent.authority.authority_id();
    intent.create_id = Uuid::new_v4().to_string();
    intent.authority = authority(authority_id, &peer.endpoint, peer.trust_digest());
    let runtime = KubernetesRuntime::connect(intent.authority.clone(), peer.credentials())
        .expect("connect the retained runtime to its owned TLS peer");
    let diagnostic = runtime
        .doctor()
        .await
        .expect("the generated leaf must validate through the separate private root CA");
    assert_eq!(diagnostic.server_version, "v1.36.1");
    assert_eq!(diagnostic.namespace_uid, NAMESPACE_UID);
    let preclaim_doctor_requests = peer.read_request_count() - reads_before;
    assert_eq!(
        preclaim_doctor_requests, 3,
        "the positive doctor TLS probe uses exactly three reads"
    );
    let quota = QuotaPolicyV1::default();
    let authorization = store_fixture
        .store
        .authorize_held_create_for_test(&store_fixture.identity, &intent, &quota, EXPIRY)
        .expect("issue test-only authorization from the real broker binding and grant");
    let (reservation, reserved, replayed) = store_fixture
        .store
        .reserve_kubernetes_create(&store_fixture.identity, &authorization, &intent)
        .expect("reserve through the real tenant Store");
    assert!(!replayed);
    let claim = claim(&intent);
    let journal_path = store_fixture
        ._directory
        .path()
        .join(format!("private-held-{}", intent.create_id));
    fs::create_dir(&journal_path).expect("create private journal root");
    fs::set_permissions(&journal_path, fs::Permissions::from_mode(0o700))
        .expect("keep journal root private");
    let journal_root = Directory::open_existing(&journal_path).unwrap();
    let prepared = prepare(&intent).expect("prepare the retained exact Pod request");
    let expected_request = prepared.pod_bytes().to_vec();
    let mut journal = AdmissionJournal::prepare(
        journal_root,
        AdmissionJournalProtection::new(Vec::new(), Vec::new()),
        store_fixture.store.tenant_id(),
        &intent,
        prepared,
        &reservation,
        &reserved,
        &claim,
    )
    .expect("persist prepared bytes before the central claim");
    let activation = HeldCreateActivation::for_test(&intent, &peer.activation_listener)
        .expect("open only the loopback test activation seam");
    let pending = PendingHeldCreateClaim::begin_for_test(activation, clock, &intent)
        .expect("sample the controlled HostClock before claiming");
    let claim_outcome = pending
        .claim(
            &store_fixture.store,
            &store_fixture.identity,
            &authorization,
            &claim,
            &mut journal,
        )
        .expect("commit a real central claim and local emission checkpoint");
    let permission = match claim_outcome {
        HeldCreateClaimOutcome::Ready {
            response,
            permission,
        } => {
            assert_eq!(
                response.disposition,
                KubernetesCreateDispositionV1::EmitOnce
            );
            permission
        }
        HeldCreateClaimOutcome::ObserveOnly { .. } => {
            panic!("first committed claim must return its non-replay fresh capability")
        }
    };
    if replace_credential {
        peer.replace_credential();
    }
    let result = runtime
        .create_held(permission, &journal, &AtomicBool::new(cancelled))
        .await;
    let mut handle_uid = None;
    if let Ok(KubernetesCreateOutcomeV1::CreatedHeld { .. }) = &result {
        let candidate = KubernetesCreateResultV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            create_id: intent.create_id.clone(),
            intent_hash: intent.intent_hash().unwrap(),
            claim_operation_id: claim.operation_id.clone(),
            expected_revision: 2,
            expected_create_epoch: 1,
            outcome: result.as_ref().unwrap().clone(),
        };
        journal
            .retain_candidate(&candidate, None)
            .expect("retain only the exact bounded result before central mutation");
        let confirmed = store_fixture
            .store
            .record_kubernetes_create_result(&store_fixture.identity, &authorization, &candidate)
            .expect("commit the exact response through the tenant Store");
        let handle = journal
            .acknowledge_result(confirmed)
            .expect("persist the central ACK before publishing a handle")
            .expect("a valid locally-acknowledged creation yields inspection metadata");
        handle_uid = Some(handle.uid().to_owned());
    }
    let post_count = peer.post_count() - posts_before;
    let request = if post_count == 0 {
        None
    } else {
        Some(peer.latest_post())
    };
    let held_workers = store_fixture
        .store
        .worker_quota(&quota)
        .expect("read the real tenant quota after the attempted CREATE")
        .held_workers;
    let canary_persisted = fs::read_dir(&journal_path)
        .expect("inspect only this test's private journal")
        .filter_map(std::result::Result::ok)
        .filter_map(|entry| fs::read(entry.path()).ok())
        .any(|bytes| {
            bytes
                .windows(PRIVATE_CANARY.len())
                .any(|part| part == PRIVATE_CANARY.as_bytes())
        });
    let (read_requests, unexpected_requests) = {
        let state = peer.state.lock().unwrap();
        (
            state.read_requests - reads_before,
            state.unexpected_requests - unexpected_before,
        )
    };
    CallResult {
        outcome: result,
        post_count,
        preclaim_doctor_requests,
        runtime_probe_requests: read_requests - preclaim_doctor_requests,
        unexpected_requests,
        request,
        expected_request,
        handle_uid,
        held_workers,
        canary_persisted,
    }
}

pub(super) fn authority(
    authority_id: Uuid,
    endpoint: &str,
    trust_sha256: String,
) -> KubernetesAuthorityV1 {
    KubernetesAuthorityV1::new(
        authority_id,
        endpoint,
        CLUSTER_UID,
        "tenant-a",
        NAMESPACE_UID,
        &trust_sha256,
    )
    .expect("valid private loopback test authority")
}

pub(super) fn claim(intent: &KubernetesCreateIntentV1) -> KubernetesCreateClaimV1 {
    KubernetesCreateClaimV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        create_id: intent.create_id.clone(),
        intent_hash: intent.intent_hash().unwrap(),
        expected_revision: 1,
        expected_create_epoch: 1,
        emitter_id: Uuid::new_v4().to_string(),
    }
}

pub(super) fn private_write(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).expect("write a private test fixture file");
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .expect("keep test fixture file private");
}
pub(super) fn private(path: &Path) -> PrivateFile {
    PrivateFile::open(path, 65_536).expect("open a retained private test fixture")
}

#[tokio::test]
async fn fresh_claim_posts_exact_retained_bytes_and_acknowledges_only_the_201_uid() {
    let boot = Uuid::from_u128(700).to_string();
    let peer = Peer::new(ResponseMode::Created).await;
    let call = perform(
        &peer,
        ResponseMode::Created,
        TestClock::repeated(sample(&boot, HostClockMethodV1::LinuxBoottimeV1, 1_000)),
        false,
        false,
    )
    .await;
    let outcome = call.outcome.as_ref().unwrap();
    let KubernetesCreateOutcomeV1::CreatedHeld {
        observation,
        admitted_spec: _,
        admitted_configuration_sha256: _,
        observed_at: _,
    } = outcome
    else {
        panic!("an exact valid HTTP 201 must produce CreatedHeld")
    };
    let request = call.request.as_ref().expect("one captured request");
    assert_eq!(request.method, "POST");
    assert_eq!(
        request.uri,
        "/api/v1/namespaces/tenant-a/pods?fieldValidation=Strict"
    );
    assert!(request.content_type_is_json);
    assert!(request.bearer_matches);
    assert_eq!(request.body, call.expected_request);
    assert_eq!(call.post_count, 1, "no retry after the one CREATE");
    assert_eq!(call.preclaim_doctor_requests, 3);
    assert_eq!(
        call.runtime_probe_requests, 4,
        "two namespace checks bracket a valid 201"
    );
    assert_eq!(call.unexpected_requests, 0);
    assert_eq!(observation.metadata.uid, POD_UID);
    assert_eq!(observation.metadata.namespace, "tenant-a");
    assert_eq!(call.handle_uid.as_deref(), Some(POD_UID));
    assert_eq!(call.held_workers, 1);
    assert!(!call.canary_persisted);
}

#[tokio::test]
async fn cancellation_stale_clock_and_replaced_credentials_never_start_the_post() {
    let boot = Uuid::from_u128(701).to_string();
    let peer = Peer::new(ResponseMode::Created).await;
    let valid = sample(&boot, HostClockMethodV1::LinuxBoottimeV1, 1_000);
    let cases = [
        (vec![valid.clone()], true, false),
        (vec![valid.clone()], false, true),
        (
            vec![
                valid.clone(),
                sample(&boot, HostClockMethodV1::LinuxBoottimeV1, 8_000_001_000),
            ],
            false,
            false,
        ),
        (
            vec![
                valid.clone(),
                sample(
                    &Uuid::from_u128(702).to_string(),
                    HostClockMethodV1::LinuxBoottimeV1,
                    2_000,
                ),
            ],
            false,
            false,
        ),
        (
            vec![
                valid.clone(),
                sample(&boot, HostClockMethodV1::LinuxBoottimeV1, 999),
            ],
            false,
            false,
        ),
        (
            vec![
                valid,
                sample(&boot, HostClockMethodV1::DarwinMachContinuousV1, 2_000),
            ],
            false,
            false,
        ),
    ];
    for (index, (samples, cancelled, replace_credential)) in cases.into_iter().enumerate() {
        let call = perform(
            &peer,
            ResponseMode::Created,
            TestClock::sequence(samples),
            cancelled,
            replace_credential,
        )
        .await;
        let error = match call.outcome {
            Err(error) => error,
            Ok(_) => panic!("case must fail closed before initiating HTTPS"),
        };
        assert!(
            !error.to_string().contains(TOKEN),
            "case {index} must not disclose the retained credential"
        );
        assert_eq!(call.post_count, 0, "case {index} must issue zero POSTs");
        assert_eq!(call.preclaim_doctor_requests, 3);
        assert!(call.runtime_probe_requests <= 2);
        assert_eq!(
            call.unexpected_requests, 0,
            "case {index} must issue no other request"
        );
        assert_eq!(
            call.held_workers, 1,
            "a durable claim retains quota when emission is denied"
        );
    }
}

#[tokio::test]
async fn non_201_duplicate_oversized_and_changed_profiles_are_unknown_without_retry() {
    let boot = Uuid::from_u128(703).to_string();
    let peer = Peer::new(ResponseMode::Created).await;
    // Establish a successful held CREATE through the same wire peer before
    // checking that each uncertain response remains closed and non-retriable.
    let modes = [
        ResponseMode::Created,
        ResponseMode::ChangedNamespaceAfterCreate,
        ResponseMode::Status(200),
        ResponseMode::Status(202),
        ResponseMode::DuplicateJson,
        ResponseMode::Oversized,
        ResponseMode::ChangedProfile,
        ResponseMode::TruncatedBody,
    ];
    for mode in modes {
        let call = perform(
            &peer,
            mode,
            TestClock::repeated(sample(&boot, HostClockMethodV1::LinuxBoottimeV1, 1_000)),
            false,
            false,
        )
        .await;
        assert_eq!(call.preclaim_doctor_requests, 3);
        if matches!(mode, ResponseMode::Created) {
            let outcome = call.outcome.expect("the baseline 201 must be accepted");
            assert!(matches!(
                outcome,
                KubernetesCreateOutcomeV1::CreatedHeld { .. }
            ));
            assert_eq!(call.post_count, 1);
            assert_eq!(call.runtime_probe_requests, 4);
            assert_eq!(call.unexpected_requests, 0);
            assert_eq!(call.handle_uid.as_deref(), Some(POD_UID));
            assert_eq!(call.held_workers, 1);
            assert!(!call.canary_persisted);
            let request = call.request.expect("one captured valid CREATE");
            assert_eq!(request.method, "POST");
            assert_eq!(
                request.uri,
                "/api/v1/namespaces/tenant-a/pods?fieldValidation=Strict"
            );
            assert!(request.content_type_is_json && request.bearer_matches);
            assert_eq!(request.body, call.expected_request);
            continue;
        }
        if matches!(mode, ResponseMode::ChangedNamespaceAfterCreate) {
            let outcome = call
                .outcome
                .as_ref()
                .expect("a post-201 authority change is a closed outcome");
            assert!(matches!(
                outcome,
                KubernetesCreateOutcomeV1::OwnershipUnknown {
                    reason: KubernetesCreateUnknownReasonV1::AuthorityChanged,
                    http_status: None
                }
            ));
            assert_eq!(call.post_count, 1);
            assert_eq!(call.runtime_probe_requests, 4);
            assert_eq!(call.handle_uid, None);
            assert_eq!(call.held_workers, 1);
            assert!(!call.canary_persisted);
            let request = call.request.expect("one captured CREATE");
            assert_eq!(request.method, "POST");
            assert!(request.content_type_is_json && request.bearer_matches);
            assert_eq!(request.body, call.expected_request);
            continue;
        }
        let outcome = call
            .outcome
            .expect("after request initiation, uncertain effects are closed outcomes");
        assert!(
            matches!(&outcome, KubernetesCreateOutcomeV1::OwnershipUnknown { .. }),
            "non-201, malformed, oversized or changed-profile responses cannot yield a handle"
        );
        assert_eq!(call.post_count, 1, "uncertain CREATE is never retried");
        assert!(call.runtime_probe_requests <= 4);
        assert_eq!(call.unexpected_requests, 0);
        assert_eq!(call.handle_uid, None);
        assert_eq!(call.held_workers, 1);
        assert!(
            !call.canary_persisted,
            "raw API error bytes must not enter the journal"
        );
        let request = call.request.expect("one captured request");
        assert_eq!(request.method, "POST");
        assert_eq!(
            request.uri,
            "/api/v1/namespaces/tenant-a/pods?fieldValidation=Strict"
        );
        assert!(request.content_type_is_json && request.bearer_matches);
        assert_eq!(request.body, call.expected_request);
    }
}
