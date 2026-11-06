//! Real CLI subprocesses and owned loopback TLS endpoints. API responses are
//! fixtures; these tests do not qualify a cluster, confinement or admission.
mod support;

use agentsandbox::{
    control_fs::{PrivateFile, TrustedExecutable},
    runtime::{capture_stream, kubernetes::KubernetesAuthorityV1},
};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
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
const TOKEN: &str = "public-kubernetes-cli-fixture-token-000000000000";
const BODY_CANARY: &str = "private-fixture-body-must-not-enter-cli-output";

#[tokio::test]
async fn kubernetes_doctor_is_an_available_read_only_cli_command() {
    let directory = support::private_tempdir();
    let mut command = command();
    command.args(["kubernetes", "doctor", "--help"]);
    let output = run(command, directory.path()).await;
    assert!(
        output.status.success(),
        "the read-only Kubernetes doctor CLI route must be available; exit={:?}",
        output.status.code()
    );
    let help = String::from_utf8(output.stdout).unwrap();
    for option in [
        "--authority-file",
        "--ca-file",
        "--token-file",
        "--client-cert-file",
        "--client-key-file",
        "--check-permissions",
    ] {
        assert!(help.contains(option), "explicit credential option absent");
    }
    assert!(output.stderr.is_empty());
    let mut preflight = crate::command();
    preflight.args(["kubernetes", "preflight", "--help"]);
    let output = run(preflight, directory.path()).await;
    assert!(output.status.success());
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains("--check-permissions"),
        "the diagnostic opt-in must not leak into the preflight connection arguments"
    );
}

fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
    command.env_clear();
    command
}

struct Output {
    status: std::process::ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

async fn run(mut command: Command, directory: &std::path::Path) -> Output {
    let mut child = command
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let observations = tokio::time::timeout(Duration::from_secs(35), async {
        tokio::join!(
            child.wait(),
            capture_stream(stdout, 32_768),
            capture_stream(stderr, 8192)
        )
    })
    .await;
    let (status, stdout, stderr) = match observations {
        Ok(observations) => observations,
        Err(_) => {
            let _ = child.start_kill();
            assert!(
                tokio::time::timeout(Duration::from_secs(2), child.wait())
                    .await
                    .is_ok_and(|result| result.is_ok()),
                "reap this test's timed-out CLI child"
            );
            panic!("CLI fixture exceeded its bounded diagnostic budget");
        }
    };
    let stdout = stdout.unwrap();
    let stderr = stderr.unwrap();
    assert!(
        !stdout.truncated && !stderr.truncated,
        "CLI output is bounded"
    );
    Output {
        status: status.unwrap(),
        stdout: stdout.bytes,
        stderr: stderr.bytes,
    }
}

fn private_write(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn refused(output: &Output, expected: &str, secrets: &[&str]) {
    assert_eq!(
        output.status.code(),
        Some(1),
        "operation must refuse with exit1"
    );
    assert!(
        output.stdout.is_empty(),
        "a refusal must not emit partial JSON"
    );
    let stderr = std::str::from_utf8(&output.stderr).unwrap();
    assert!(stderr.contains(expected), "closed failure category differs");
    for secret in secrets.iter().copied().chain([TOKEN, BODY_CANARY]) {
        assert!(
            !output
                .stdout
                .windows(secret.len())
                .chain(output.stderr.windows(secret.len()))
                .any(|window| window == secret.as_bytes()),
            "private fixture value appeared in CLI output"
        );
    }
}

struct Pki {
    _root: tempfile::TempDir,
    directory: PathBuf,
    ca: PathBuf,
    token: PathBuf,
}
impl Pki {
    async fn new() -> Self {
        let root = support::private_tempdir();
        let root_path = root.path().canonicalize().unwrap();
        let directory = root_path.join("pki");
        let mut prepare = Command::new(env!("CARGO_BIN_EXE_ai-guard-demo-pki"));
        prepare
            .env_clear()
            .arg("--out")
            .arg(&directory)
            .args(["--dns", "fixture.invalid"]);
        fixture_command(prepare).await;
        let ca = root_path.join("trust.pem");
        private_write(&ca, &fs::read(directory.join("root-ca.pem")).unwrap());
        let token = root_path.join("bearer");
        private_write(&token, TOKEN.as_bytes());
        Self {
            _root: root,
            directory,
            ca,
            token,
        }
    }
    fn key(&self) -> PathBuf {
        self.directory.join("server.key")
    }
    async fn client_certificate(&self) -> PathBuf {
        let config = self.directory.join("client.cnf");
        private_write(&config, b"[ca]\ndefault_ca=local\n[local]\ndatabase=client-index\nserial=client-serial\nnew_certs_dir=.\ncertificate=intermediate-ca.pem\nprivate_key=intermediate-ca.key\ndefault_md=sha256\npolicy=subject\nx509_extensions=client\n[subject]\ncommonName=supplied\n[client]\nbasicConstraints=critical,CA:false\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=clientAuth\n");
        private_write(&self.directory.join("client-index"), b"");
        private_write(
            &self.directory.join("client-serial"),
            format!("{}\n", Uuid::new_v4().simple()).as_bytes(),
        );
        let executable = TrustedExecutable::open(Path::new("/usr/bin/openssl")).unwrap();
        executable.revalidate().unwrap();
        let mut sign = Command::new(executable.path());
        sign.env_clear().current_dir(&self.directory).args([
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
        fixture_command(sign).await;
        let certificate = self.directory.join("client-chain.pem");
        private_write(
            &certificate,
            &[
                fs::read(self.directory.join("client-leaf.pem")).unwrap(),
                fs::read(self.directory.join("intermediate-ca.pem")).unwrap(),
            ]
            .concat(),
        );
        certificate
    }
}
async fn fixture_command(mut command: Command) {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let status = tokio::time::timeout(Duration::from_secs(120), child.wait()).await;
    if status.is_err() {
        let _ = child.start_kill();
        assert!(
            tokio::time::timeout(Duration::from_secs(2), child.wait())
                .await
                .is_ok_and(|status| status.is_ok()),
            "reap owned fixture preparation"
        );
    }
    assert!(
        status.is_ok_and(|status| status.is_ok_and(|status| status.success())),
        "ephemeral TLS fixture preparation failed"
    );
}

struct Behavior {
    requests: Vec<String>,
    expected_token: Option<String>,
    authentication_valid: bool,
    status: u16,
    version: String,
    namespace_phase: Option<String>,
    access_reviews: Vec<Value>,
    ssar_status: u16,
}
struct Server {
    endpoint: String,
    state: Arc<Mutex<Behavior>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Server {
    async fn new(pki: &Pki, mutual_tls: bool) -> Self {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let builder = rustls::ServerConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .unwrap();
        let builder = if mutual_tls {
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
        let acceptor = TlsAcceptor::from(Arc::new(builder.with_single_cert(chain, key).unwrap()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("https://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(Behavior {
            requests: vec![],
            expected_token: (!mutual_tls).then(|| TOKEN.to_owned()),
            authentication_valid: true,
            status: 200,
            version: "v1.36.1".into(),
            namespace_phase: Some("Active".into()),
            access_reviews: Vec::new(),
            ssar_status: 201,
        }));
        let observed = state.clone();
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (stream, _) = accepted.unwrap();
                        let acceptor = acceptor.clone();
                        let state = observed.clone();
                        connections.spawn(async move {
                            if let Ok(Ok(mut stream)) = tokio::time::timeout(Duration::from_secs(3), acceptor.accept(stream)).await {
                                let mut request = Vec::new();
                                while !request.ends_with(b"\r\n\r\n") {
                                    let mut byte = [0;1];
                                    if stream.read(&mut byte).await.ok() != Some(1) { return; }
                                    request.push(byte[0]);
                                    assert!(request.len() <= 16_384, "request headers bounded");
                                }
                                let request = String::from_utf8(request).unwrap();
                                let first = request.lines().next().unwrap();
                                let content_length: usize = request.lines().find_map(|line| line.split_once(':')
                                    .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                                    .map(|(_, value)| value.trim().parse().unwrap())).unwrap_or(0);
                                assert!(content_length <= 4096, "authorization query body bounded");
                                let mut request_body = vec![0; content_length];
                                if stream.read_exact(&mut request_body).await.is_err() { return; }
                                let authorization = request.lines().find_map(|line| line.split_once(':').filter(|(name, _)| name.eq_ignore_ascii_case("authorization")).map(|(_, value)| value.trim()));
                                let (status, body) = {
                                    let mut state = state.lock().unwrap();
                                    state.requests.push(first.to_owned());
                                    let expected = state.expected_token.as_ref().map(|token| format!("Bearer {token}"));
                                    let valid = authorization == expected.as_deref();
                                    state.authentication_valid &= valid;
                                    let is_ssar = first == "POST /apis/authorization.k8s.io/v1/selfsubjectaccessreviews HTTP/1.1";
                                    let status = if !valid { 401 } else if state.status != 200 { state.status }
                                        else if is_ssar { state.ssar_status } else { 200 };
                                    let body = if ![200,201].contains(&status) { json!({"private":BODY_CANARY}) } else {
                                        match first {
                                            "GET /api/v1/namespaces/kube-system HTTP/1.1" => json!({"apiVersion":"v1","kind":"Namespace","metadata":{"name":"kube-system","uid":CLUSTER_UID},"status":{"phase":"Active"}}),
                                            "GET /api/v1/namespaces/tenant-a HTTP/1.1" => json!({"apiVersion":"v1","kind":"Namespace","metadata":{"name":"tenant-a","uid":NAMESPACE_UID,"labels":{"pod-security.kubernetes.io/enforce":"restricted"}},"status":{"phase":state.namespace_phase}}),
                                            "GET /version HTTP/1.1" => json!({"gitVersion":state.version}),
                                            "POST /apis/authorization.k8s.io/v1/selfsubjectaccessreviews HTTP/1.1" => {
                                                let query: Value = serde_json::from_slice(&request_body).unwrap();
                                                state.access_reviews.push(query.clone());
                                                let allowed = query["spec"]["resourceAttributes"]["verb"] == "create"
                                                    && query["spec"]["resourceAttributes"]["resource"] == "pods"
                                                    && query["spec"]["resourceAttributes"].get("subresource").is_none();
                                                json!({"apiVersion":"authorization.k8s.io/v1","kind":"SelfSubjectAccessReview",
                                                    "spec":query["spec"],"status":{"allowed":allowed,"denied":!allowed,"reason":BODY_CANARY}})
                                            }
                                            _ => panic!("unexpected mutation or read outside the diagnostic scope"),
                                        }
                                    };
                                    (status, serde_json::to_vec(&body).unwrap())
                                };
                                let mut response = format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
                                response.extend_from_slice(&body);
                                let _ = stream.write_all(&response).await;
                                let _ = stream.shutdown().await;
                            }
                        });
                    }
                    completed = connections.join_next(), if !connections.is_empty() => { assert!(completed.unwrap().is_ok(), "controlled TLS connection failed"); }
                }
            }
        });
        Self {
            endpoint,
            state,
            task,
        }
    }
    fn authority(&self, pki: &Pki) -> PathBuf {
        let binding = KubernetesAuthorityV1::new(
            Uuid::from_u128(1),
            &self.endpoint,
            CLUSTER_UID,
            "tenant-a",
            NAMESPACE_UID,
            PrivateFile::open(&pki.ca, 65536).unwrap().digest(),
        )
        .unwrap();
        let path = pki
            .directory
            .join(format!("authority-{}.json", Uuid::new_v4()));
        private_write(&path, &serde_json::to_vec(&binding).unwrap());
        path
    }
}
fn doctor(authority: &Path, ca: &Path) -> Command {
    let mut command = command();
    command
        .args(["kubernetes", "doctor", "--authority-file"])
        .arg(authority)
        .arg("--ca-file")
        .arg(ca);
    command
}
fn bearer_doctor(authority: &Path, pki: &Pki) -> Command {
    let mut command = doctor(authority, &pki.ca);
    command.arg("--token-file").arg(&pki.token);
    command
}

#[tokio::test]
async fn doctor_uses_real_https_static_authentication_and_reports_admission_gaps() {
    let pki = Pki::new().await;
    let server = Server::new(&pki, false).await;
    let authority = server.authority(&pki);
    let mut command = bearer_doctor(&authority, &pki);
    command
        .env("KUBECONFIG", "/nonexistent/ambient-kubeconfig")
        .env("HTTPS_PROXY", "http://127.0.0.1:9")
        .env("ALL_PROXY", "http://127.0.0.1:9")
        .env("NO_PROXY", "");
    let output = run(command, &pki.directory).await;
    assert!(output.status.success(), "authenticated doctor must succeed");
    assert!(output.stderr.is_empty());
    assert!(!String::from_utf8_lossy(&output.stdout).contains(TOKEN));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["version"], 1);
    assert_eq!(report["admission_available"], false);
    assert!(
        report.get("prerequisites").is_none(),
        "legacy report shape must be preserved without opt-in"
    );
    let diagnostics = &report["diagnostics"];
    assert_eq!(diagnostics["server_version"], "v1.36.1");
    assert_eq!(diagnostics["namespace"], "tenant-a");
    assert_eq!(diagnostics["namespace_uid"], NAMESPACE_UID);
    assert_eq!(diagnostics["cluster_namespace_uid"], CLUSTER_UID);
    assert_eq!(
        diagnostics["namespace_pod_security_enforce_label"],
        "restricted"
    );
    assert!(diagnostics["observed_at"].as_str().is_some());
    assert_eq!(
        diagnostics["authority_fingerprint"].as_str().unwrap().len(),
        64
    );
    let expected_gaps = json!([
        "network_enforcement_not_proven",
        "container_confinement_not_qualified",
        "per_pod_pid_limit_not_proven",
        "temporary_filesystem_noexec_not_proven",
        "worker_log_persistence_not_prevented",
        "workspace_transfer_not_implemented",
        "exec_attach_not_implemented",
        "durable_deadline_not_implemented",
        "control_audit_admission_not_connected",
        "service_account_rbac_not_qualified"
    ]);
    assert_eq!(diagnostics["admission_gaps"], expected_gaps);
    let reasons = report["admission_gap_reasons"].as_array().unwrap();
    assert_eq!(reasons.len(), 10);
    assert!(reasons.iter().all(|entry| {
        entry["reason"]
            .as_str()
            .is_some_and(|reason| !reason.is_empty())
    }));
    {
        let state = server.state.lock().unwrap();
        assert!(state.authentication_valid);
        assert_eq!(
            state.requests,
            [
                "GET /api/v1/namespaces/kube-system HTTP/1.1",
                "GET /api/v1/namespaces/tenant-a HTTP/1.1",
                "GET /version HTTP/1.1"
            ]
        );
    }

    for status in [401, 403] {
        server.state.lock().unwrap().status = status;
        let before = server.state.lock().unwrap().requests.len();
        refused(
            &run(bearer_doctor(&authority, &pki), &pki.directory).await,
            &format!("HTTP {status}"),
            &[],
        );
        assert_eq!(
            server.state.lock().unwrap().requests.len(),
            before + 1,
            "no retry or later observation after refusal"
        );
    }
    server.state.lock().unwrap().status = 200;
    private_write(&pki.token, b"wrong-private-cli-token-000000000000000000000");
    refused(
        &run(bearer_doctor(&authority, &pki), &pki.directory).await,
        "HTTP 401",
        &["wrong-private-cli-token-000000000000000000000"],
    );
    private_write(&pki.token, TOKEN.as_bytes());

    let other = Pki::new().await;
    let before = server.state.lock().unwrap().requests.len();
    let other_binding = KubernetesAuthorityV1::new(
        Uuid::from_u128(1),
        &server.endpoint,
        CLUSTER_UID,
        "tenant-a",
        NAMESPACE_UID,
        PrivateFile::open(&other.ca, 65536).unwrap().digest(),
    )
    .unwrap();
    let wrong_authority = other.directory.join("wrong-authority.json");
    private_write(
        &wrong_authority,
        &serde_json::to_vec(&other_binding).unwrap(),
    );
    refused(
        &run(bearer_doctor(&wrong_authority, &other), &other.directory).await,
        "HTTPS transport is unavailable",
        &[],
    );
    let mut mismatch = doctor(&authority, &other.ca);
    mismatch.arg("--token-file").arg(&pki.token);
    refused(
        &run(mismatch, &pki.directory).await,
        "credentials or trust are invalid",
        &[],
    );
    assert_eq!(
        server.state.lock().unwrap().requests.len(),
        before,
        "wrong trust must not reach HTTP"
    );

    let certificate = pki.client_certificate().await;
    let mtls = Server::new(&pki, true).await;
    let mtls_authority = mtls.authority(&pki);
    let mut command = doctor(&mtls_authority, &pki.ca);
    command
        .arg("--client-cert-file")
        .arg(&certificate)
        .arg("--client-key-file")
        .arg(pki.key());
    let output = run(command, &pki.directory).await;
    assert!(
        output.status.success(),
        "explicit client identity must authenticate against mandatory mTLS"
    );
    assert_eq!(mtls.state.lock().unwrap().requests.len(), 3);
    assert!(
        mtls.state.lock().unwrap().authentication_valid,
        "mTLS must not add a bearer header"
    );
    refused(
        &run(bearer_doctor(&mtls_authority, &pki), &pki.directory).await,
        "HTTPS transport is unavailable",
        &[],
    );
    assert_eq!(
        mtls.state.lock().unwrap().requests.len(),
        3,
        "missing client certificate must fail before HTTP"
    );
    let mut mismatch = doctor(&mtls_authority, &pki.ca);
    mismatch
        .arg("--client-cert-file")
        .arg(&certificate)
        .arg("--client-key-file")
        .arg(other.key());
    refused(
        &run(mismatch, &pki.directory).await,
        "credentials or trust are invalid",
        &[],
    );
    assert_eq!(
        mtls.state.lock().unwrap().requests.len(),
        3,
        "mismatched key must fail before HTTP"
    );
}

#[tokio::test]
async fn doctor_opt_in_reports_scoped_queries_without_granting_admission() {
    let pki = Pki::new().await;
    let server = Server::new(&pki, false).await;
    let authority = server.authority(&pki);
    let mut command = bearer_doctor(&authority, &pki);
    command.arg("--check-permissions");
    let output = run(command, &pki.directory).await;
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["version"], 1);
    assert_eq!(report["admission_available"], false);
    let prerequisites = &report["prerequisites"];
    assert_eq!(prerequisites["version"], 1);
    assert_eq!(prerequisites["namespace"]["state"], "active");
    assert_eq!(prerequisites["namespace"]["uid"], NAMESPACE_UID);
    assert_eq!(prerequisites["permission_review_status"], "complete");
    assert_eq!(prerequisites["permission_coverage"], "queried_actions_only");
    let reviews = prerequisites["access_reviews"].as_array().unwrap();
    assert_eq!(reviews.len(), 12);
    assert!(
        reviews
            .iter()
            .any(|review| review["action"] == "create_pods" && review["decision"] == "allowed")
    );
    assert!(
        reviews
            .iter()
            .any(|review| review["action"] == "create_pod_exec" && review["decision"] == "denied")
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(!text.contains(TOKEN));
    assert!(!text.contains(BODY_CANARY));
    let state = server.state.lock().unwrap();
    assert!(state.authentication_valid);
    assert_eq!(state.requests.len(), 17);
    assert_eq!(state.access_reviews.len(), 12);
    assert!(state.access_reviews.iter().all(|query| {
        query["spec"].as_object().unwrap().len() == 1
            && query["spec"]["resourceAttributes"]["namespace"] == "tenant-a"
            && query["spec"]["resourceAttributes"]["version"] == "v1"
    }));
}

#[tokio::test]
async fn doctor_opt_in_shows_namespace_unknown_and_fails_opaque_on_ssar_refusal() {
    let pki = Pki::new().await;
    let server = Server::new(&pki, false).await;
    let authority = server.authority(&pki);
    for (phase, expected) in [
        (None, "unknown"),
        (Some("Terminating".into()), "terminating"),
    ] {
        server.state.lock().unwrap().namespace_phase = phase;
        let mut command = bearer_doctor(&authority, &pki);
        command.arg("--check-permissions");
        let output = run(command, &pki.directory).await;
        assert!(output.status.success());
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["prerequisites"]["namespace"]["state"], expected);
        assert_eq!(
            report["prerequisites"]["permission_review_status"],
            "skipped_namespace_not_active"
        );
        assert_eq!(report["prerequisites"]["access_reviews"], json!([]));
        assert!(server.state.lock().unwrap().access_reviews.is_empty());
    }
    {
        let mut state = server.state.lock().unwrap();
        state.namespace_phase = Some("Active".into());
        state.ssar_status = 403;
    }
    let mut command = bearer_doctor(&authority, &pki);
    command.arg("--check-permissions");
    refused(&run(command, &pki.directory).await, "HTTP 403", &[]);
    assert!(server.state.lock().unwrap().access_reviews.is_empty());
}

#[tokio::test]
async fn doctor_refuses_credential_echoes_without_rewriting_typed_observations() {
    let pki = Pki::new().await;
    let server = Server::new(&pki, false).await;
    let authority = server.authority(&pki);
    server.state.lock().unwrap().version = TOKEN.into();
    refused(
        &run(bearer_doctor(&authority, &pki), &pki.directory).await,
        "output contains protected credential material",
        &[],
    );
    server.state.lock().unwrap().version = "v1.36.1".into();
    private_write(&pki.token, NAMESPACE_UID.as_bytes());
    server.state.lock().unwrap().expected_token = Some(NAMESPACE_UID.into());
    refused(
        &run(bearer_doctor(&authority, &pki), &pki.directory).await,
        "output contains protected credential material",
        &[NAMESPACE_UID],
    );

    let certificate = pki.client_certificate().await;
    server.state.lock().unwrap().expected_token = None;
    // Rewrap the valid PKCS#8 bytes at their stable DER prefix. This avoids
    // depending on a random RSA body line lacking slash/padding characters,
    // while exercising a credential line valid in server-version metadata.
    let original_key = fs::read_to_string(pki.key()).unwrap();
    let body: String = original_key
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect();
    let safe_alphabet_line = &body[..32];
    assert!(
        safe_alphabet_line
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric())
    );
    let key = format!(
        "-----BEGIN PRIVATE KEY-----\n{safe_alphabet_line}\n{}\n-----END PRIVATE KEY-----\n",
        &body[32..]
    );
    private_write(&pki.key(), key.as_bytes());
    server.state.lock().unwrap().version = safe_alphabet_line.into();
    let mut command = doctor(&authority, &pki.ca);
    command
        .arg("--client-cert-file")
        .arg(&certificate)
        .arg("--client-key-file")
        .arg(pki.key());
    refused(
        &run(command, &pki.directory).await,
        "output contains protected credential material",
        &[safe_alphabet_line, &key],
    );
}

#[tokio::test]
async fn doctor_rejects_invalid_private_configuration_before_transport() {
    let root = support::private_tempdir();
    let root = root.path().canonicalize().unwrap();
    let authority = root.join("authority.json");
    let ca = root.join("ca.pem");
    let token = root.join("token");
    let binding = json!({"version":1,"authority_id":"00000000-0000-0000-0000-000000000001","endpoint":"https://127.0.0.1:9/","cluster_namespace_uid":CLUSTER_UID,"namespace":"tenant-a","namespace_uid":NAMESPACE_UID,"trust_sha256":"0".repeat(64)});
    let bytes = serde_json::to_vec(&binding).unwrap();
    private_write(&authority, &bytes);
    private_write(&ca, b"invalid-fixture-ca-never-used-for-network");
    private_write(&token, TOKEN.as_bytes());
    let invoke = |path: &Path| {
        let mut command = doctor(path, &ca);
        command.arg("--token-file").arg(&token);
        command
    };
    for invalid in [
        b"{private-invalid-json-canary".to_vec(),
        [bytes.clone(), vec![b' '; 16384]].concat(),
    ] {
        private_write(&authority, &invalid);
        refused(
            &run(invoke(&authority), &root).await,
            "authority configuration is invalid",
            &["private-invalid-json-canary"],
        );
    }
    for (field, value) in [
        ("version", json!(2)),
        ("unknown_private_field", json!(BODY_CANARY)),
        ("endpoint", json!("http://127.0.0.1:9/")),
        ("namespace", json!("kube-system")),
        ("namespace_uid", json!(CLUSTER_UID)),
    ] {
        let mut invalid = binding.clone();
        invalid[field] = value;
        private_write(&authority, &serde_json::to_vec(&invalid).unwrap());
        refused(
            &run(invoke(&authority), &root).await,
            "authority configuration is invalid",
            &[],
        );
    }
    private_write(&authority, &bytes);
    fs::set_permissions(&authority, fs::Permissions::from_mode(0o644)).unwrap();
    refused(
        &run(invoke(&authority), &root).await,
        "authority configuration is invalid",
        &[],
    );
    fs::set_permissions(&authority, fs::Permissions::from_mode(0o600)).unwrap();
    let alias = root.join("authority-link");
    symlink(&authority, &alias).unwrap();
    refused(
        &run(invoke(&alias), &root).await,
        "authority configuration is invalid",
        &[],
    );
    let mutable = root.join("mutable");
    fs::create_dir(&mutable).unwrap();
    fs::set_permissions(&mutable, fs::Permissions::from_mode(0o777)).unwrap();
    let mutable_authority = mutable.join("authority");
    private_write(&mutable_authority, &bytes);
    refused(
        &run(invoke(&mutable_authority), &root).await,
        "authority configuration is invalid",
        &[],
    );
    fs::set_permissions(&token, fs::Permissions::from_mode(0o644)).unwrap();
    refused(
        &run(invoke(&authority), &root).await,
        "credentials or trust are invalid",
        &[],
    );
    fs::set_permissions(&token, fs::Permissions::from_mode(0o600)).unwrap();
    let token_alias = root.join("token-link");
    symlink(&token, &token_alias).unwrap();
    let mut command = doctor(&authority, &ca);
    command.arg("--token-file").arg(&token_alias);
    refused(
        &run(command, &root).await,
        "credentials or trust are invalid",
        &[],
    );
    let hardlink = root.join("token-hardlink");
    fs::hard_link(&token, &hardlink).unwrap();
    refused(
        &run(invoke(&authority), &root).await,
        "credentials or trust are invalid",
        &[],
    );
    fs::remove_file(&hardlink).unwrap();
    private_write(&token, &vec![b'a'; 8193]);
    refused(
        &run(invoke(&authority), &root).await,
        "credentials or trust are invalid",
        &[],
    );
    private_write(&token, TOKEN.as_bytes());
    private_write(&ca, &vec![b'a'; 65537]);
    refused(
        &run(invoke(&authority), &root).await,
        "credentials or trust are invalid",
        &[],
    );
    private_write(&ca, b"invalid-fixture-ca-never-used-for-network");
    let cert = root.join("client.pem");
    let key = root.join("client.key");
    private_write(&cert, b"not-used");
    private_write(&key, &vec![b'a'; 16385]);
    let mut command = doctor(&authority, &ca);
    command
        .arg("--client-cert-file")
        .arg(&cert)
        .arg("--client-key-file")
        .arg(&key);
    refused(
        &run(command, &root).await,
        "credentials or trust are invalid",
        &[],
    );
    for options in [
        vec![],
        vec!["--client-key-file"],
        vec!["--client-cert-file"],
        vec!["--token-file", "--client-cert-file", "--client-key-file"],
    ] {
        let mut command = doctor(&authority, &ca);
        for option in options {
            command.arg(option).arg(&token);
        }
        let output = run(command, &root).await;
        assert_eq!(
            output.status.code(),
            Some(2),
            "incomplete/ambiguous authentication must fail in CLI parsing"
        );
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains(TOKEN));
    }
}
