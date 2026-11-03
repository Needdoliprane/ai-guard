//! Real loopback TLS connections, mandatory client certificates and peer-close
//! oracles. The HTTP responses are fixtures, not Kubernetes qualification.
mod support;
use agentsandbox::{
    control_fs::{PrivateFile, TrustedExecutable},
    runtime::kubernetes::{
        KubernetesAuthorityV1, KubernetesCredentials, KubernetesError, KubernetesPodHandle,
        KubernetesRuntime,
    },
};
use serde_json::json;
use std::{
    fs, io,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::Notify,
    task::JoinSet,
};
use tokio_rustls::{
    TlsAcceptor,
    rustls::{
        self,
        pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
    },
};
use uuid::Uuid;

const CLUSTER_UID: &str = "11111111-1111-4111-8111-111111111111";
const NAMESPACE_UID: &str = "22222222-2222-4222-8222-222222222222";
const POD_UID: &str = "33333333-3333-4333-8333-333333333333";

struct Pki {
    _root: tempfile::TempDir,
    directory: PathBuf,
    ca: PathBuf,
    token: PathBuf,
}
impl Pki {
    async fn new() -> Self {
        let root = support::private_tempdir();
        let directory = root.path().canonicalize().unwrap().join("pki");
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_ai-guard-demo-pki"));
        command
            .env_clear()
            .arg("--out")
            .arg(&directory)
            .arg("--dns")
            .arg("fixture.invalid");
        run_fixture_command(command).await;
        let ca = root.path().canonicalize().unwrap().join("trust.pem");
        private_write(&ca, &fs::read(directory.join("root-ca.pem")).unwrap());
        let token = root.path().canonicalize().unwrap().join("bearer");
        private_write(
            &token,
            b"public-only-kubernetes-transport-fixture-token-000000000000",
        );
        Self {
            _root: root,
            directory,
            ca,
            token,
        }
    }
    async fn client_certificate(&self) -> PathBuf {
        // Re-sign the existing public test CSR with clientAuth EKU. The same
        // ephemeral key remains private; no host credential is inspected.
        let config = self.directory.join("client.cnf");
        private_write(&config, b"[ca]\ndefault_ca=local\n[local]\ndatabase=client-index\nserial=client-serial\nnew_certs_dir=.\ncertificate=intermediate-ca.pem\nprivate_key=intermediate-ca.key\ndefault_md=sha256\npolicy=subject\nx509_extensions=client\n[subject]\ncommonName=supplied\n[client]\nbasicConstraints=critical,CA:false\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=clientAuth\n");
        private_write(&self.directory.join("client-index"), b"");
        private_write(
            &self.directory.join("client-serial"),
            format!("{}\n", Uuid::new_v4().simple()).as_bytes(),
        );
        let executable = TrustedExecutable::open(Path::new("/usr/bin/openssl")).unwrap();
        executable.revalidate().unwrap();
        let mut command = tokio::process::Command::new(executable.path());
        command.env_clear().current_dir(&self.directory).args([
            "ca",
            "-batch",
            "-notext",
            "-config",
            "client.cnf",
            "-in",
            "server.csr",
            "-out",
            "client-leaf.pem",
            "-days",
            "1",
        ]);
        run_fixture_command(command).await;
        let chain = self.directory.join("client-chain.pem");
        private_write(
            &chain,
            &[
                fs::read(self.directory.join("client-leaf.pem")).unwrap(),
                fs::read(self.directory.join("intermediate-ca.pem")).unwrap(),
            ]
            .concat(),
        );
        chain
    }
    fn key(&self) -> PathBuf {
        self.directory.join("server.key")
    }
    fn bearer(&self) -> KubernetesCredentials {
        KubernetesCredentials::bearer(private(&self.ca), private(&self.token))
    }
    fn mutual_tls(&self, certificate: &Path, key: &Path) -> KubernetesCredentials {
        KubernetesCredentials::mutual_tls(private(&self.ca), private(certificate), private(key))
    }
}
async fn run_fixture_command(mut command: tokio::process::Command) {
    let mut child = command
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
        "ephemeral TLS fixture command failed"
    );
}
fn private_write(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}
fn private(path: &Path) -> PrivateFile {
    PrivateFile::open(path, 65536).unwrap()
}

#[derive(Clone, Copy)]
enum Mode {
    Complete,
    BeforeHeaders,
    PartialBody,
    ExcessiveContentLength,
    PodNotFound,
}
#[derive(Default)]
struct Counters {
    opened: AtomicUsize,
    active: AtomicUsize,
    closed: AtomicUsize,
    tls_rejected: AtomicUsize,
    http_requests: AtomicUsize,
    blocked: AtomicUsize,
    peer_closed: AtomicUsize,
    completed: AtomicUsize,
    unexpected_io: AtomicUsize,
    changed: Notify,
}
struct ActiveConnection(Arc<Counters>);
impl ActiveConnection {
    fn new(counters: Arc<Counters>) -> Self {
        counters.opened.fetch_add(1, Ordering::SeqCst);
        counters.active.fetch_add(1, Ordering::SeqCst);
        counters.changed.notify_waiters();
        Self(counters)
    }
}
impl Drop for ActiveConnection {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
        self.0.closed.fetch_add(1, Ordering::SeqCst);
        self.0.changed.notify_waiters();
    }
}
struct Server {
    endpoint: String,
    mode: Arc<Mutex<Mode>>,
    counters: Arc<Counters>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Server {
    async fn new(pki: &Pki, require_client_auth: bool) -> Self {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let builder = rustls::ServerConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .unwrap();
        let builder = if require_client_auth {
            let mut roots = rustls::RootCertStore::empty();
            for certificate in CertificateDer::pem_slice_iter(&fs::read(&pki.ca).unwrap()) {
                roots.add(certificate.unwrap()).unwrap();
            }
            let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
                Arc::new(roots),
                provider,
            )
            .build()
            .unwrap();
            builder.with_client_cert_verifier(verifier)
        } else {
            builder.with_no_client_auth()
        };
        let chain = CertificateDer::pem_slice_iter(
            &fs::read(pki.directory.join("server-chain.pem")).unwrap(),
        )
        .map(Result::unwrap)
        .collect();
        let key = PrivateKeyDer::from_pem_slice(&fs::read(pki.key()).unwrap()).unwrap();
        let config = builder.with_single_cert(chain, key).unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(config));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("https://{}", listener.local_addr().unwrap());
        let mode = Arc::new(Mutex::new(Mode::Complete));
        let counters = Arc::new(Counters::default());
        let observed = counters.clone();
        let behavior = mode.clone();
        let task = tokio::spawn(async move {
            // JoinSet owns every accepted socket. Tests keep this owner alive
            // throughout EOF/reset assertions; teardown is never that oracle.
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (stream, _) = accepted.unwrap();
                        let guard = ActiveConnection::new(observed.clone());
                        let acceptor = acceptor.clone();
                        let mode = behavior.clone();
                        let counters = observed.clone();
                        connections.spawn(async move {
                            let _guard = guard;
                            match tokio::time::timeout(Duration::from_secs(3), acceptor.accept(stream)).await {
                                Ok(Ok(stream)) => exchange(stream, mode, counters).await,
                                _ => { counters.tls_rejected.fetch_add(1, Ordering::SeqCst); counters.changed.notify_waiters(); }
                            }
                        });
                    }
                    completed = connections.join_next(), if !connections.is_empty() => {
                        assert!(completed.unwrap().is_ok(), "controlled TLS connection task failed");
                    }
                }
            }
        });
        Self {
            endpoint,
            mode,
            counters,
            task,
        }
    }
    fn binding(&self, pki: &Pki) -> KubernetesAuthorityV1 {
        KubernetesAuthorityV1::new(
            Uuid::from_u128(1),
            &self.endpoint,
            CLUSTER_UID,
            "tenant-a",
            NAMESPACE_UID,
            private(&pki.ca).digest(),
        )
        .unwrap()
    }
    fn runtime(&self, pki: &Pki) -> KubernetesRuntime {
        KubernetesRuntime::connect(self.binding(pki), pki.bearer()).unwrap()
    }
    async fn wait(&self, condition: impl Fn(&Counters) -> bool) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let notified = self.counters.changed.notified();
                if condition(&self.counters) {
                    return;
                }
                notified.await;
            }
        })
        .await
        .expect("controlled TLS peer did not reach the expected socket state");
        assert!(
            !self.task.is_finished(),
            "server owner ended before the socket oracle"
        );
        assert_eq!(self.counters.unexpected_io.load(Ordering::SeqCst), 0);
    }
}
async fn exchange(
    mut stream: tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
    mode: Arc<Mutex<Mode>>,
    counters: Arc<Counters>,
) {
    let mut request = Vec::new();
    loop {
        let mut byte = [0u8; 1];
        match stream.read(&mut byte).await {
            Ok(1) => request.push(byte[0]),
            _ => return,
        }
        assert!(
            request.len() <= 16384,
            "controlled request header bound exceeded"
        );
        if request.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    counters.http_requests.fetch_add(1, Ordering::SeqCst);
    counters.changed.notify_waiters();
    let path = std::str::from_utf8(&request)
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap();
    let mode = *mode.lock().unwrap();
    match mode {
        Mode::BeforeHeaders => {}
        Mode::PartialBody => {
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n1\r\n{\r\n").await.unwrap();
            stream.flush().await.unwrap();
        }
        Mode::ExcessiveContentLength => {
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 1048577\r\n\r\n").await.unwrap();
            stream.flush().await.unwrap();
        }
        Mode::Complete | Mode::PodNotFound => {
            let is_missing_pod =
                matches!(mode, Mode::PodNotFound) && path.ends_with("/pods/worker-a");
            let body = match path {
                "/version" => json!({"gitVersion":"v1.36.1"}),
                "/api/v1/namespaces/kube-system" => {
                    json!({"apiVersion":"v1","kind":"Namespace","metadata":{"name":"kube-system","uid":CLUSTER_UID}})
                }
                "/api/v1/namespaces/tenant-a" => {
                    json!({"apiVersion":"v1","kind":"Namespace","metadata":{"name":"tenant-a","uid":NAMESPACE_UID}})
                }
                _ if is_missing_pod => json!({"private":"body never forwarded"}),
                _ => panic!("unexpected controlled API path"),
            };
            let body = serde_json::to_vec(&body).unwrap();
            let status = if is_missing_pod {
                "404 Not Found"
            } else {
                "200 OK"
            };
            let mut response = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
            response.extend_from_slice(&body);
            stream.write_all(&response).await.unwrap();
            stream.flush().await.unwrap();
            counters.completed.fetch_add(1, Ordering::SeqCst);
            counters.changed.notify_waiters();
            return;
        }
    }
    counters.blocked.fetch_add(1, Ordering::SeqCst);
    counters.changed.notify_waiters();
    // No server timeout, shutdown or response completion closes this socket.
    // Only EOF/reset received from the client satisfies the cancellation oracle.
    let mut byte = [0u8; 1];
    match stream.read(&mut byte).await {
        Ok(0) => {
            counters.peer_closed.fetch_add(1, Ordering::SeqCst);
        }
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::UnexpectedEof
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::ConnectionAborted
                    | io::ErrorKind::BrokenPipe
            ) =>
        {
            counters.peer_closed.fetch_add(1, Ordering::SeqCst);
        }
        _ => {
            counters.unexpected_io.fetch_add(1, Ordering::SeqCst);
        }
    }
    counters.changed.notify_waiters();
}

#[tokio::test]
async fn mutual_tls_requires_a_trusted_compatible_client_identity_before_http() {
    let pki = Pki::new().await;
    let client = pki.client_certificate().await;
    let other = Pki::new().await;
    let other_client = other.client_certificate().await;
    let server = Server::new(&pki, true).await;
    let credentials = pki.mutual_tls(&client, &pki.key());
    let opaque_credentials = format!("{credentials:?}");
    let runtime = KubernetesRuntime::connect(server.binding(&pki), credentials).unwrap();
    assert_eq!(runtime.doctor().await.unwrap().server_version, "v1.36.1");
    server
        .wait(|counts| counts.active.load(Ordering::SeqCst) == 0)
        .await;
    assert_eq!(server.counters.http_requests.load(Ordering::SeqCst), 3);
    for credentials in [
        pki.bearer(),
        pki.mutual_tls(&other_client, &other.key()),
        pki.mutual_tls(&pki.directory.join("server-chain.pem"), &pki.key()),
    ] {
        let rejected = server.counters.tls_rejected.load(Ordering::SeqCst);
        let runtime = KubernetesRuntime::connect(server.binding(&pki), credentials).unwrap();
        let error = runtime.doctor().await.unwrap_err();
        assert_eq!(error, KubernetesError::TransportUnavailable);
        server
            .wait(|counts| {
                counts.tls_rejected.load(Ordering::SeqCst) == rejected + 1
                    && counts.active.load(Ordering::SeqCst) == 0
            })
            .await;
        assert_eq!(
            server.counters.http_requests.load(Ordering::SeqCst),
            3,
            "rejected client reached HTTP"
        );
        assert!(!format!("{error:?} {error} {runtime:?}").contains("BEGIN PRIVATE KEY"));
    }
    let before = server.counters.opened.load(Ordering::SeqCst);
    assert!(matches!(
        KubernetesRuntime::connect(server.binding(&pki), pki.mutual_tls(&client, &other.key())),
        Err(KubernetesError::InvalidCredentials)
    ));
    assert_eq!(
        server.counters.opened.load(Ordering::SeqCst),
        before,
        "mismatched key reached the network"
    );
    let private_key = fs::read_to_string(pki.key()).unwrap();
    let certificate = fs::read_to_string(client).unwrap();
    for output in [opaque_credentials, format!("{runtime:?}")] {
        assert!(!output.contains(&private_key));
        assert!(!output.contains(&certificate));
    }
}

#[tokio::test]
async fn cancellation_closes_real_tls_peers_before_headers_and_during_partial_body() {
    let pki = Pki::new().await;
    let server = Server::new(&pki, false).await;
    let runtime = Arc::new(server.runtime(&pki));
    assert!(runtime.doctor().await.is_ok());
    server
        .wait(|counts| counts.active.load(Ordering::SeqCst) == 0)
        .await;
    assert_eq!(server.counters.completed.load(Ordering::SeqCst), 3);
    assert_eq!(
        server.counters.peer_closed.load(Ordering::SeqCst),
        0,
        "normal server finishes contaminated the cancellation oracle"
    );
    for (index, mode) in [Mode::BeforeHeaders, Mode::PartialBody]
        .into_iter()
        .enumerate()
    {
        *server.mode.lock().unwrap() = mode;
        let client = {
            let runtime = runtime.clone();
            tokio::spawn(async move { runtime.doctor().await })
        };
        server
            .wait(|counts| counts.blocked.load(Ordering::SeqCst) == index + 1)
            .await;
        assert!(
            !client.is_finished(),
            "controlled response was not pending at cancellation"
        );
        client.abort();
        assert!(client.await.unwrap_err().is_cancelled());
        server
            .wait(|counts| {
                counts.peer_closed.load(Ordering::SeqCst) == index + 1
                    && counts.active.load(Ordering::SeqCst) == 0
            })
            .await;
        assert_eq!(
            server.counters.completed.load(Ordering::SeqCst),
            3,
            "suspended response completed normally"
        );
    }
    *server.mode.lock().unwrap() = Mode::Complete;
    assert!(runtime.doctor().await.is_ok());
    server
        .wait(|counts| counts.active.load(Ordering::SeqCst) == 0)
        .await;
    assert_eq!(server.counters.opened.load(Ordering::SeqCst), 8);
    assert_eq!(server.counters.closed.load(Ordering::SeqCst), 8);
    assert_eq!(server.counters.completed.load(Ordering::SeqCst), 6);
    assert_eq!(server.counters.peer_closed.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn pod_absence_and_response_body_bounds_remain_distinct_from_observations() {
    let pki = Pki::new().await;
    let server = Server::new(&pki, false).await;
    let runtime = server.runtime(&pki);
    let spec = json!({"containers":[{"name":"worker","image":"fixture-only"}]});
    let handle = KubernetesPodHandle::from_control_checkpoint(
        &server.binding(&pki),
        Uuid::from_u128(2),
        Uuid::from_u128(3),
        "worker-a",
        POD_UID,
        &KubernetesPodHandle::configuration_digest(&spec).unwrap(),
    )
    .unwrap();
    *server.mode.lock().unwrap() = Mode::PodNotFound;
    assert_eq!(
        runtime.observe_owned(&handle).await.unwrap_err(),
        KubernetesError::ApiRejected(404)
    );
    assert_eq!(
        server.counters.http_requests.load(Ordering::SeqCst),
        3,
        "Pod404 was not reached after both authority anchors"
    );
    server
        .wait(|counts| counts.active.load(Ordering::SeqCst) == 0)
        .await;
    *server.mode.lock().unwrap() = Mode::PartialBody;
    assert_eq!(
        runtime.doctor().await.unwrap_err(),
        KubernetesError::Timeout
    );
    server
        .wait(|counts| {
            counts.peer_closed.load(Ordering::SeqCst) == 1
                && counts.active.load(Ordering::SeqCst) == 0
        })
        .await;
    *server.mode.lock().unwrap() = Mode::ExcessiveContentLength;
    assert_eq!(
        runtime.doctor().await.unwrap_err(),
        KubernetesError::ResponseTooLarge
    );
    server
        .wait(|counts| {
            counts.peer_closed.load(Ordering::SeqCst) == 2
                && counts.active.load(Ordering::SeqCst) == 0
        })
        .await;
}
