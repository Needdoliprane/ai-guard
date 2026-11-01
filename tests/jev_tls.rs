//! Real TLS transport tests for the product clients. Certificates are ephemeral
//! fixtures, not a qualification of the PKI helper or any inference model.
mod support;

use agentsandbox::{
    api::{GatewayState, gateway_router},
    audit::Store,
    jev::{JevBackend, JevClient, JevTrust, SelectionSource, validate_transport_configuration},
    model::DecisionRequest,
    policy::Policy,
    redaction::Redactor,
    tls::{ServerIdentity, serve},
};
use axum::{
    Json, Router,
    body::Body,
    http::Request,
    routing::{get, post},
};
use http_body_util::BodyExt;
use serde_json::json;
use std::{
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::oneshot,
};
use tower::ServiceExt;

async fn certificates(parent: &std::path::Path, name: &str) -> std::path::PathBuf {
    let path = parent.join(name);
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_ai-guard-demo-pki"))
        .env_clear()
        .arg("--out")
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let status = tokio::time::timeout(Duration::from_secs(120), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(status.success(), "ephemeral TLS fixture generation failed");
    path
}

async fn tls_connection(
    address: &str,
    ca: &[u8],
) -> tokio_rustls::client::TlsStream<tokio::net::TcpStream> {
    use tokio_rustls::rustls::{
        self,
        pki_types::{CertificateDer, ServerName, pem::PemObject},
    };
    let mut roots = rustls::RootCertStore::empty();
    for certificate in CertificateDer::pem_slice_iter(ca) {
        roots.add(certificate.unwrap()).unwrap();
    }
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    let stream = tokio::net::TcpStream::connect(address).await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(3),
        tokio_rustls::TlsConnector::from(Arc::new(config))
            .connect(ServerName::try_from("localhost").unwrap(), stream),
    )
    .await
    .unwrap()
    .unwrap()
}

#[tokio::test]
async fn explicit_ca_is_strict_and_typesafe_requires_a_verified_handshake() {
    let temp = support::private_tempdir();
    let pki = certificates(temp.path(), "valid").await;
    let other = certificates(temp.path(), "other").await;
    let ca = std::fs::read(pki.join("root-ca.pem")).unwrap();
    let trust = JevTrust::from_ca_pem(&ca).unwrap();
    let wrong = JevTrust::from_ca_pem(&std::fs::read(other.join("root-ca.pem")).unwrap()).unwrap();
    assert!(
        validate_transport_configuration("https://localhost/v1/systemone", false, &trust).is_ok()
    );
    assert!(
        validate_transport_configuration("http://127.0.0.1/v1/systemone", true, &trust).is_err()
    );
    for bytes in [
        Vec::new(),
        b"not PEM".to_vec(),
        vec![b' '; 65_537],
        std::fs::read(pki.join("server.key")).unwrap(),
        [ca.clone(), b"garbage".to_vec()].concat(),
        ca.repeat(9),
    ] {
        assert!(JevTrust::from_ca_pem(&bytes).is_err());
    }
    let identity = ServerIdentity::from_pem(
        &std::fs::read(pki.join("server-chain.pem")).unwrap(),
        &std::fs::read(pki.join("server.key")).unwrap(),
    )
    .unwrap();
    let saturation_identity = identity.clone();
    assert!(
        ServerIdentity::from_pem(
            &std::fs::read(pki.join("server-chain.pem")).unwrap(),
            &std::fs::read(other.join("server.key")).unwrap()
        )
        .is_err()
    );
    let chain = std::fs::read(pki.join("server-chain.pem")).unwrap();
    let key = std::fs::read(pki.join("server.key")).unwrap();
    assert!(ServerIdentity::from_pem(&chain, &key.repeat(2)).is_err());
    assert!(
        ServerIdentity::from_pem(
            &chain,
            &[key.clone(), b"unparsed material".to_vec()].concat()
        )
        .is_err()
    );
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let local_seen = calls.clone();
    let blocked = Arc::new(tokio::sync::Notify::new());
    let blocked_seen = blocked.clone();
    let app = Router::new()
        .route("/v1/systemone", post(move || {
            seen.fetch_add(1, Ordering::SeqCst);
            async { Json(json!({
                "model":"tls-fixture", "answers":{"execution_policy":{
                    "type":"choice","choice":"deny","confidence":0.99
                }},"usage":null
            })) }
        }))
        .route("/v1/chat/completions", post(move || {
            local_seen.fetch_add(1,Ordering::SeqCst);
            async { Json(json!({
                "model":"tls-local-fixture","choices":[{
                    "index":0,"finish_reason":"stop","message":{
                        "role":"assistant","content":"{\"outcome\":\"allow\",\"reason\":\"fixture advice\"}"
                    }
                }],"usage":null
            })) }
        }))
        .route("/health",get(|| async {Json(json!({"status":"ok"}))}))
        .route("/blocked",post(move || {
            let blocked=blocked_seen.clone();
            async move {
                blocked.notify_one();
                std::future::pending::<axum::http::StatusCode>().await
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("https://{}/v1/systemone", listener.local_addr().unwrap());
    let (stop, stopped) = oneshot::channel();
    let task = tokio::spawn(serve(listener, app, identity, async {
        let _ = stopped.await;
    }));
    let request: DecisionRequest = serde_json::from_value(json!({"request_id":uuid::Uuid::new_v4(),"profile":"strict","command":{"argv":["/usr/bin/id"],"cwd":"/workspace"}})).unwrap();
    let redactor = Redactor::new(vec![]);
    for client in [
        JevClient::new(
            &url,
            "fixture-tls-only-credential".into(),
            "requested".into(),
            0.85,
            false,
        )
        .unwrap(),
        JevClient::new_with_trust(
            &url,
            "fixture-tls-only-credential".into(),
            "requested".into(),
            0.85,
            false,
            wrong,
        )
        .unwrap(),
    ] {
        assert!(client.decide(&request, &redactor).await.is_err());
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "unverified TLS sent an application request"
        );
    }
    let client = JevClient::new_with_trust(
        &url,
        "fixture-tls-only-credential".into(),
        "requested".into(),
        0.85,
        false,
        trust.clone(),
    )
    .unwrap();
    assert!(client.decide(&request, &redactor).await.is_ok());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let local_url = url.replace("/v1/systemone", "/v1/chat/completions");
    let store =
        Arc::new(Store::open(&temp.path().join("audit.sqlite"), redactor.clone(), 100).unwrap());
    let mut hashes = Vec::new();
    for selected_trust in [
        trust.clone(),
        JevTrust::from_ca_pem(&ca).unwrap(),
        JevTrust::from_ca_pem(&std::fs::read(other.join("root-ca.pem")).unwrap()).unwrap(),
    ] {
        let backend = JevBackend::local_openai_with_trust(
            &local_url,
            None,
            "requested-local".into(),
            false,
            SelectionSource::Programmatic,
            selected_trust,
        )
        .unwrap();
        let app = gateway_router(GatewayState::new_with_backend(
            store.clone(),
            "r".repeat(48),
            "v".repeat(48),
            Policy::default(),
            Some(backend),
            redactor.clone(),
        ));
        let mut local_request = request.clone();
        local_request.request_id = uuid::Uuid::new_v4().to_string();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/decide")
                    .header("authorization", format!("Bearer {}", "r".repeat(48)))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&local_request).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let result: serde_json::Value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        hashes.push(result["jev"]["config_sha256"].clone());
        assert_eq!(result["decision"]["outcome"], "deny");
        if hashes.len() < 3 {
            assert_eq!(result["jev"]["provider_outcome"], "allow");
            assert_eq!(result["jev"]["outcome"], "review");
            assert_eq!(
                result["jev"]["configuration"]["transport"]["trust"]["mode"],
                "explicit_ca_bundle"
            );
        } else {
            assert_eq!(result["jev"]["status"], "unavailable_or_invalid");
        }
        assert_eq!(
            store
                .decision(&local_request, None)
                .unwrap()
                .unwrap()
                .event
                .data["jev"],
            result["jev"]
        );
    }
    assert_eq!(hashes[0], hashes[1]);
    assert_ne!(hashes[0], hashes[2]);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    let health_url = url.replace("/v1/systemone", "/health");
    let mut probe = tokio::process::Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
        .env_clear()
        .env("HTTPS_PROXY", "http://127.0.0.1:9")
        .args(["healthcheck", "--url", &health_url, "--ca-file"])
        .arg(pki.join("root-ca.pem"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(4), probe.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    let address = url
        .strip_prefix("https://")
        .unwrap()
        .split('/')
        .next()
        .unwrap();
    let mut slow = tokio::net::TcpStream::connect(address).await.unwrap();
    slow.write_all(b"\x16").await.unwrap();
    let mut byte = [0; 1];
    let result = tokio::time::timeout(Duration::from_secs(3), slow.read(&mut byte))
        .await
        .unwrap();
    assert!(
        matches!(result, Ok(0) | Err(_)),
        "incomplete TLS handshake stayed open"
    );
    drop(slow);
    let mut active = tls_connection(address, &ca).await;
    active
        .write_all(b"POST /blocked HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\n\r\n")
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), blocked.notified())
        .await
        .unwrap();
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(6), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(1), active.read(&mut byte))
        .await
        .unwrap();
    assert!(
        matches!(result, Ok(0) | Err(_)),
        "TLS connection remained usable after server join"
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let (stop, stopped) = oneshot::channel();
    let task = tokio::spawn(serve(listener, Router::new(), saturation_identity, async {
        let _ = stopped.await;
    }));
    let mut held = Vec::new();
    for _ in 0..32 {
        held.push(tls_connection(&address, &ca).await);
    }
    let mut excess = tokio::net::TcpStream::connect(&address).await.unwrap();
    let _ = excess.write_all(b"\x16").await;
    let result = tokio::time::timeout(Duration::from_secs(3), excess.read(&mut byte))
        .await
        .unwrap();
    assert!(
        matches!(result, Ok(0) | Err(_)),
        "connection cap accepted a 33rd TLS connection"
    );
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(6), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    drop(held);
}

#[tokio::test]
async fn gateway_ca_errors_precede_credential_opens_and_do_not_create_audit() {
    let temp = support::private_tempdir();
    let ca = temp.path().join("invalid.pem");
    std::fs::write(&ca, b"not a certificate").unwrap();
    let audit = temp.path().join("never-created.sqlite");
    let runner = temp.path().join("runner-fifo");
    let encoded = std::ffi::CString::new(runner.as_os_str().as_encoded_bytes()).unwrap();
    // Opening this fixture as a normal credential would fail or block. The
    // separate bounded process also checks no audit file was initialized.
    assert_eq!(unsafe { libc::mkfifo(encoded.as_ptr(), 0o600) }, 0);
    for backend in ["typesafe", "local-openai", "rust-rules"] {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
        command
            .env_clear()
            .env_remove("JEV_URL")
            .env_remove("JEV_MODEL")
            .env_remove("JEV_API_KEY_FILE")
            .env_remove("JEV_BACKEND")
            .env_remove("JEV_CA_FILE")
            .args([
                "gateway",
                "--listen",
                "127.0.0.1:0",
                "--jev-backend",
                backend,
                "--jev-ca-file",
            ])
            .arg(&ca)
            .arg("--runner-token-file")
            .arg(&runner)
            .arg("--viewer-token-file")
            .arg(temp.path().join("missing-viewer"))
            .arg("--data")
            .arg(&audit);
        if backend == "typesafe" {
            command
                .args([
                    "--jev-url",
                    "https://localhost/v1/systemone",
                    "--jev-model",
                    "fixture",
                    "--jev-key-file",
                ])
                .arg(temp.path().join("missing-judge"));
        }
        if backend == "local-openai" {
            command.args([
                "--jev-local-url",
                "https://localhost/v1/chat/completions",
                "--jev-local-model",
                "fixture",
            ]);
        }
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stderr = child.stderr.take().unwrap();
        let read = tokio::spawn(async move {
            let mut bytes = Vec::new();
            stderr.take(4096).read_to_end(&mut bytes).await.unwrap();
            bytes
        });
        let status = tokio::time::timeout(Duration::from_secs(3), child.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(!status.success());
        let diagnostic = String::from_utf8(read.await.unwrap()).unwrap();
        assert!(
            diagnostic.contains(if backend == "rust-rules" {
                "Incomplete or mixed Jev backend configuration"
            } else {
                "Jev configuration is invalid"
            }),
            "expected preflight error, not unknown CLI option or credential failure"
        );
        assert!(!audit.exists());
    }
}

async fn signed_leaf(
    pki: &std::path::Path,
    label: &str,
    start: chrono::DateTime<chrono::Utc>,
    end: chrono::DateTime<chrono::Utc>,
    san: &str,
) -> Vec<u8> {
    let config = format!(
        "[ca]\ndefault_ca=local\n[local]\ndatabase=index-{label}\nserial=serial-{label}\nnew_certs_dir=.\ncertificate=intermediate-ca.pem\nprivate_key=intermediate-ca.key\ndefault_md=sha256\npolicy=subject\nx509_extensions=leaf\n[subject]\ncommonName=supplied\n[leaf]\nbasicConstraints=critical,CA:false\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName={san}\n"
    );
    let config_name = format!("ca-{label}.cnf");
    let output_name = format!("leaf-{label}.pem");
    std::fs::write(pki.join(&config_name), config).unwrap();
    std::fs::write(pki.join(format!("index-{label}")), b"").unwrap();
    std::fs::write(
        pki.join(format!("serial-{label}")),
        format!("{}\n", uuid::Uuid::new_v4().simple()),
    )
    .unwrap();
    let executable =
        agentsandbox::control_fs::TrustedExecutable::open(std::path::Path::new("/usr/bin/openssl"))
            .unwrap();
    executable.revalidate().unwrap();
    let mut child = tokio::process::Command::new(executable.path())
        .current_dir(pki)
        .env_clear()
        .args([
            "ca",
            "-batch",
            "-notext",
            "-config",
            &config_name,
            "-in",
            "server.csr",
            "-out",
            &output_name,
            "-startdate",
            &start.format("%Y%m%d%H%M%SZ").to_string(),
            "-enddate",
            &end.format("%Y%m%d%H%M%SZ").to_string(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(20), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success(),
        "TLS negative fixture signing failed"
    );
    [
        std::fs::read(pki.join(output_name)).unwrap(),
        std::fs::read(pki.join("intermediate-ca.pem")).unwrap(),
    ]
    .concat()
}

#[tokio::test]
async fn tls_rejects_wrong_san_expired_and_future_leaf_before_application() {
    let temp = support::private_tempdir();
    let pki = certificates(temp.path(), "negative-leaves").await;
    let trust = JevTrust::from_ca_pem(&std::fs::read(pki.join("root-ca.pem")).unwrap()).unwrap();
    let key = std::fs::read(pki.join("server.key")).unwrap();
    let now = chrono::Utc::now();
    for (label, start, end, san) in [
        (
            "san",
            now - chrono::Duration::days(1),
            now + chrono::Duration::days(1),
            "DNS:not-contacted.ai-guard.test",
        ),
        (
            "expired",
            now - chrono::Duration::days(3),
            now - chrono::Duration::days(2),
            "IP:127.0.0.1",
        ),
        (
            "future",
            now + chrono::Duration::days(2),
            now + chrono::Duration::days(3),
            "IP:127.0.0.1",
        ),
    ] {
        let chain = signed_leaf(&pki, label, start, end, san).await;
        let identity = ServerIdentity::from_pem(&chain, &key).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let app = Router::new().route(
            "/v1/systemone",
            post(move || {
                seen.fetch_add(1, Ordering::SeqCst);
                async { Json(json!({"unexpected":true})) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("https://{}/v1/systemone", listener.local_addr().unwrap());
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(serve(listener, app, identity, async {
            let _ = stopped.await;
        }));
        let client = JevClient::new_with_trust(
            &url,
            "fixture-negative-tls-credential".into(),
            "requested".into(),
            0.85,
            false,
            trust.clone(),
        )
        .unwrap();
        let request:DecisionRequest=serde_json::from_value(json!({"request_id":uuid::Uuid::new_v4(),"profile":"strict","command":{"argv":["/usr/bin/id"],"cwd":"/workspace"}})).unwrap();
        assert!(
            client
                .decide(&request, &Redactor::new(vec![]))
                .await
                .is_err(),
            "unverified leaf {label} was accepted"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        stop.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(6), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}

/// The malformed inputs contain public certificate bytes and AA== only. They
/// never contain a real private key, including one from the ephemeral fixture.
#[tokio::test]
async fn nested_pem_markers_are_rejected_before_certificate_classification() {
    let temporary = support::private_tempdir();
    let pki = certificates(temporary.path(), "pem-review").await;
    let public_ca = std::fs::read(pki.join("root-ca.pem")).unwrap();
    let positive = JevTrust::from_ca_pem(&public_ca).is_ok();
    let ca_text = std::str::from_utf8(&public_ca).unwrap();
    let malformed = [
        (
            "private_section_inside_certificate",
            format!(
                "-----BEGIN CERTIFICATE-----\n-----BEGIN PRIVATE KEY-----\nAA==\n-----END PRIVATE KEY-----\n{ca_text}"
            ),
        ),
        (
            "certificate_inside_certificate",
            format!("-----BEGIN CERTIFICATE-----\n{ca_text}"),
        ),
        (
            "unknown_section_inside_certificate",
            format!(
                "-----BEGIN CERTIFICATE-----\n-----BEGIN OPAQUE TEST BLOCK-----\nAA==\n-----END OPAQUE TEST BLOCK-----\n{ca_text}"
            ),
        ),
        (
            "mismatched_end_inside_certificate",
            format!("-----BEGIN CERTIFICATE-----\n-----END PRIVATE KEY-----\n{ca_text}"),
        ),
        (
            "unmatched_outer_end",
            format!("-----BEGIN CERTIFICATE-----\n{ca_text}-----END CERTIFICATE-----\n"),
        ),
    ];
    let observations: Vec<_> = malformed
        .into_iter()
        .map(|(label, input)| (label, JevTrust::from_ca_pem(input.as_bytes()).is_err()))
        .collect();
    let suffix_control =
        JevTrust::from_ca_pem(&[public_ca.clone(), b"unparsed suffix".to_vec()].concat()).is_err();
    let byte_bound_control = JevTrust::from_ca_pem(&vec![b' '; 65_537]).is_err();
    let section_bound_control = JevTrust::from_ca_pem(&public_ca.repeat(9)).is_err();
    // The generator has already joined. All observations precede destruction
    // of this test's fresh PKI directory and the final behavior oracle.
    drop(temporary);
    assert!(positive, "the public CA positive must be valid");
    assert!(
        suffix_control && byte_bound_control && section_bound_control,
        "existing complete-input/size/count controls must remain discriminant"
    );
    eprintln!("PEM nested rejection observations: {observations:?}");
    assert!(
        observations.iter().all(|(_, rejected)| *rejected),
        "nested or mismatched PEM markers must not be skipped by the typed certificate decoder"
    );
}

struct ShutdownDropSentinel(Option<oneshot::Sender<()>>);
impl Drop for ShutdownDropSentinel {
    fn drop(&mut self) {
        if let Some(dropped) = self.0.take() {
            let _ = dropped.send(());
        }
    }
}
struct CancelledServerScope {
    owner: Option<tokio::task::JoinHandle<std::io::Result<()>>>,
    cleanup: Option<oneshot::Sender<()>>,
}
impl Drop for CancelledServerScope {
    fn drop(&mut self) {
        if let Some(owner) = &self.owner {
            owner.abort();
        }
        // Unblock a retained signal only as cleanup; the regression records
        // its pre-cleanup observation independently before this point.
        if let Some(cleanup) = self.cleanup.take() {
            let _ = cleanup.send(());
        }
    }
}

#[tokio::test]
async fn aborting_tls_serve_releases_pending_shutdown_and_closes_owned_connection() {
    let temporary = support::private_tempdir();
    let pki = certificates(temporary.path(), "abort-review").await;
    let public_ca = std::fs::read(pki.join("root-ca.pem")).unwrap();
    let identity = ServerIdentity::from_pem(
        &std::fs::read(pki.join("server-chain.pem")).unwrap(),
        &std::fs::read(pki.join("server.key")).unwrap(),
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let (cleanup, pending_shutdown) = oneshot::channel();
    let (dropped, mut drop_observation) = oneshot::channel();
    let sentinel = ShutdownDropSentinel(Some(dropped));
    let entered = Arc::new(tokio::sync::Notify::new());
    let signal_entered = entered.clone();
    let shutdown = async move {
        let _retained = sentinel;
        signal_entered.notify_one();
        let _ = pending_shutdown.await;
    };
    let router = Router::new().route("/health", get(|| async { Json(json!({"status":"ok"})) }));
    let owner = tokio::spawn(serve(listener, router, identity, shutdown));
    let mut scope = CancelledServerScope {
        owner: Some(owner),
        cleanup: Some(cleanup),
    };
    tokio::time::timeout(Duration::from_secs(3), entered.notified())
        .await
        .unwrap();
    let mut connection = tls_connection(&address, &public_ca).await;
    connection
        .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: keep-alive\r\n\r\n")
        .await
        .unwrap();
    let positive = tokio::time::timeout(Duration::from_secs(3), async {
        let mut received = Vec::new();
        let mut chunk = [0; 512];
        while received.len() < 4096 {
            let count = match connection.read(&mut chunk).await {
                Ok(count) if count > 0 => count,
                _ => return false,
            };
            received.extend_from_slice(&chunk[..count]);
            if received
                .windows(b"{\"status\":\"ok\"}".len())
                .any(|bytes| bytes == b"{\"status\":\"ok\"}")
            {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false);
    let owner = scope.owner.as_mut().unwrap();
    owner.abort();
    let join = tokio::time::timeout(Duration::from_secs(3), owner).await;
    let cancelled = matches!(join, Ok(Err(ref error)) if error.is_cancelled());
    scope.owner.take();
    let mut byte = [0; 1];
    let (socket, sentinel) = tokio::join!(
        tokio::time::timeout(Duration::from_secs(2), connection.read(&mut byte)),
        tokio::time::timeout(Duration::from_secs(1), &mut drop_observation),
    );
    let socket_closed = matches!(socket, Ok(Ok(0) | Err(_)));
    let sentinel_before_cleanup = matches!(sentinel, Ok(Ok(())));
    // Always unblock the harness signal after the oracle was collected, even
    // when T1 retains Axum's forwarder. This cannot turn that oracle into PASS.
    if let Some(cleanup) = scope.cleanup.take() {
        let _ = cleanup.send(());
    }
    let cleanup_complete = sentinel_before_cleanup
        || tokio::time::timeout(Duration::from_secs(3), &mut drop_observation)
            .await
            .is_ok_and(|result| result.is_ok());
    drop(connection);
    drop(scope);
    drop(temporary);
    eprintln!(
        "TLS owner cancellation observations: positive={positive} cancelled={cancelled} socket_closed={socket_closed} sentinel_before_cleanup={sentinel_before_cleanup} cleanup_complete={cleanup_complete}"
    );
    assert!(
        positive && cancelled,
        "establish a real serving owner before cancellation"
    );
    assert!(
        cleanup_complete,
        "the harness must release its pending shutdown resources"
    );
    assert!(
        socket_closed,
        "owner cancellation must close its established TLS connection"
    );
    assert!(
        sentinel_before_cleanup,
        "owner cancellation must release shutdown resources without the harness sending shutdown"
    );
}
