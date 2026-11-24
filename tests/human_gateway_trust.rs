//! Real human CLI access to the real human router/SQLite over local TLS.
//! Ephemeral certificates are QA fixtures; no external PKI or provider is used.
mod support;

use agentsandbox::{
    api::{GatewayState, gateway_router},
    audit::Store,
    policy::Policy,
    redaction::Redactor,
    tls::{ServerIdentity, serve},
};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    process::{Output, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::process::Command;

async fn output(command: &mut Command) -> Output {
    command
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    tokio::time::timeout(Duration::from_secs(20), command.output())
        .await
        .expect("human CLI exceeded its bounded test deadline")
        .expect("spawn human CLI")
}

async fn certificates(parent: &Path) -> PathBuf {
    let path = parent.join("gateway-pki");
    let mut child = Command::new(env!("CARGO_BIN_EXE_ai-guard-demo-pki"))
        .env_clear()
        .arg("--out")
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn ephemeral TLS fixture generator");
    assert!(
        tokio::time::timeout(Duration::from_secs(120), child.wait())
            .await
            .expect("TLS fixture generation exceeded its deadline")
            .expect("wait TLS fixture generator")
            .success(),
        "ephemeral TLS fixture generation failed"
    );
    path
}

async fn provision(parent: &Path) -> (Arc<Store>, PathBuf) {
    let data = parent.join("human.sqlite");
    let audit = Store::create_supervised_for_tenant(
        &data,
        "tenant-human-tls",
        Redactor::new(vec![]),
        5000,
        &parent.join("before-v3.sqlite"),
    )
    .unwrap();
    audit
        .migrate_identity_v4(&parent.join("before-v4.sqlite"))
        .unwrap();
    audit
        .migrate_human_access_v5(&parent.join("before-v5.sqlite"))
        .unwrap();
    drop(audit);
    let human = parent.join("alice");
    let mut command = Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
    command
        .args(["human", "provision", "--data"])
        .arg(&data)
        .args([
            "--tenant-id",
            "tenant-human-tls",
            "--name",
            "Alice TLS fixture",
            "--organization",
            "organization-human-tls",
            "--output-dir",
        ])
        .arg(&human)
        .arg("--confirm-control-stopped");
    assert!(
        output(&mut command).await.status.success(),
        "prepare a real named local human through the product CLI"
    );
    (
        Arc::new(
            Store::open_existing_for_tenant(&data, "tenant-human-tls", Redactor::new(vec![]), 5000)
                .unwrap(),
        ),
        human.join("credential"),
    )
}

fn grant_fixture_session(parent: &Path, audit: &Store, session: &str) {
    use agentsandbox::identity::*;

    let recovery: Value =
        serde_json::from_slice(&std::fs::read(parent.join("alice/recovery.json")).unwrap())
            .unwrap();
    let provision: LocalHumanProvisionV1 =
        serde_json::from_value(recovery["provision"].clone()).unwrap();
    let IdentityChangeV1::Account { account, .. } = &provision.mutations[0].change else {
        panic!("typed account fixture")
    };
    audit
        .mutate_identity(&IdentityMutationV1 {
            version: 1,
            operation_id: uuid::Uuid::new_v4().to_string(),
            actor: provision.mutations[0].actor.clone(),
            change: IdentityChangeV1::ResourceGrant {
                expected_revision: 0,
                grant: ResourceGrantV1 {
                    version: 1,
                    tenant_id: "tenant-human-tls".into(),
                    organization_scope: "organization-human-tls".into(),
                    resource: ResourceRefV1 {
                        kind: ResourceKindV1::Session,
                        resource_id: session.into(),
                    },
                    owner: account.principal.clone(),
                    revision: 1,
                    active: true,
                    shares: vec![],
                },
            },
        })
        .unwrap();
}

fn identity_command(origin: &str, credential: &Path, ca: Option<&Path>) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
    command
        .args([
            "human",
            "identity",
            "--gateway",
            origin,
            "--credential-file",
        ])
        .arg(credential);
    if let Some(ca) = ca {
        command.arg("--gateway-ca-file").arg(ca);
    }
    command
}

fn no_credential(output: &Output, credential: &Path) {
    let secret = std::fs::read_to_string(credential).unwrap();
    for bytes in [&output.stdout, &output.stderr] {
        assert!(
            !String::from_utf8_lossy(bytes).contains(secret.trim()),
            "human credential appeared in CLI output"
        );
    }
}

#[tokio::test]
async fn local_human_requires_explicit_gateway_ca_before_authenticated_http() {
    let temp = support::private_tempdir();
    let pki = certificates(temp.path()).await;
    let (audit, credential) = provision(temp.path()).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let router = gateway_router(GatewayState::new(
        audit,
        "runner-human-tls-fixture".into(),
        "viewer-human-tls-fixture".into(),
        Policy::default(),
        None,
        Redactor::new(vec![]),
    ))
    .layer(axum::middleware::from_fn(
        move |request: axum::extract::Request, next: axum::middleware::Next| {
            let seen = seen.clone();
            async move {
                seen.fetch_add(1, Ordering::SeqCst);
                next.run(request).await
            }
        },
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("https://{}", listener.local_addr().unwrap());
    let identity =
        ServerIdentity::from_files(&pki.join("server-chain.pem"), &pki.join("server.key")).unwrap();
    let server = tokio::spawn(serve(listener, router, identity, std::future::pending()));

    // This existing default path must fail at TLS, rather than authenticate or
    // merely reject an unknown new option. No HTTP request reaches the router.
    let rejected = output(&mut identity_command(&origin, &credential, None)).await;
    let default_calls = calls.load(Ordering::SeqCst);

    // Before implementation this intentionally fails because --gateway-ca-file
    // is absent from the CLI. It is feature absence, not a TLS vulnerability.
    let accepted = output(&mut identity_command(
        &origin,
        &credential,
        Some(&pki.join("root-ca.pem")),
    ))
    .await;
    let explicit_calls = calls.load(Ordering::SeqCst);
    // The expected pre-implementation failure must not leave the TLS server
    // or its Store running while fixture directories are dropped.
    server.abort();
    let _ = server.await;
    no_credential(&rejected, &credential);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("connection, TLS or timeout"));
    assert_eq!(default_calls, 0);
    no_credential(&accepted, &credential);
    assert!(
        accepted.status.success(),
        "human identity must accept the explicit approved gateway CA; unknown option is the expected pre-implementation RED"
    );
    let identity: Value = serde_json::from_slice(&accepted.stdout).unwrap();
    assert_eq!(identity["authentication"]["method"], "local_credential");
    assert_eq!(identity["contexts"].as_array().unwrap().len(), 1);
    assert_eq!(explicit_calls, 1);
}

async fn counted_gateway(
    audit: Arc<Store>,
    identity: ServerIdentity,
) -> (
    String,
    Arc<AtomicUsize>,
    tokio::task::JoinHandle<std::io::Result<()>>,
) {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let router = gateway_router(GatewayState::new(
        audit,
        "runner-human-tls-negative".into(),
        "viewer-human-tls-negative".into(),
        Policy::default(),
        None,
        Redactor::new(vec![]),
    ))
    .layer(axum::middleware::from_fn(
        move |request: axum::extract::Request, next: axum::middleware::Next| {
            let seen = seen.clone();
            async move {
                seen.fetch_add(1, Ordering::SeqCst);
                next.run(request).await
            }
        },
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("https://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(serve(listener, router, identity, std::future::pending()));
    (origin, calls, server)
}

#[tokio::test]
async fn wrong_gateway_ca_and_excess_certificates_never_send_authenticated_http() {
    let temp = support::private_tempdir();
    let other = support::private_tempdir();
    let pki = certificates(temp.path()).await;
    let wrong = certificates(other.path()).await;
    let (audit, credential) = provision(temp.path()).await;
    let identity =
        ServerIdentity::from_files(&pki.join("server-chain.pem"), &pki.join("server.key")).unwrap();
    let (origin, calls, server) = counted_gateway(audit, identity).await;
    let rejected = output(&mut identity_command(
        &origin,
        &credential,
        Some(&wrong.join("root-ca.pem")),
    ))
    .await;
    let excessive = temp.path().join("nine-certificates.pem");
    public_ca_file(
        &excessive,
        &std::fs::read(pki.join("root-ca.pem")).unwrap().repeat(9),
    );
    let too_many = output(&mut identity_command(
        &origin,
        &credential,
        Some(&excessive),
    ))
    .await;
    let count = calls.load(Ordering::SeqCst);
    server.abort();
    let _ = server.await;
    for result in [&rejected, &too_many] {
        no_credential(result, &credential);
        assert!(!result.status.success());
    }
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("connection, TLS or timeout"));
    assert!(
        String::from_utf8_lossy(&too_many.stderr)
            .contains("gateway CA configuration is invalid or unavailable")
    );
    assert_eq!(count, 0);
}

fn public_ca_file(path: &Path, bytes: &[u8]) {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

// Same owned fixture signing as jev_tls: these leaves exercise product TLS
// verification, without a certificate service or any installation trust change.
async fn signed_leaf(
    pki: &Path,
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
        agentsandbox::control_fs::TrustedExecutable::open(Path::new("/usr/bin/openssl")).unwrap();
    executable.revalidate().unwrap();
    let mut child = Command::new(executable.path())
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
async fn gateway_san_and_validity_are_checked_before_authenticated_http() {
    let temp = support::private_tempdir();
    let pki = certificates(temp.path()).await;
    let (audit, credential) = provision(temp.path()).await;
    let key = std::fs::read(pki.join("server.key")).unwrap();
    let now = chrono::Utc::now();
    for (label, start, end, san) in [
        (
            "wrong-san",
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
        let (origin, calls, server) = counted_gateway(audit.clone(), identity).await;
        let rejected = output(&mut identity_command(
            &origin,
            &credential,
            Some(&pki.join("root-ca.pem")),
        ))
        .await;
        let count = calls.load(Ordering::SeqCst);
        server.abort();
        let _ = server.await;
        no_credential(&rejected, &credential);
        assert!(!rejected.status.success());
        assert!(String::from_utf8_lossy(&rejected.stderr).contains("connection, TLS or timeout"));
        assert_eq!(
            count, 0,
            "invalid name or validity sent an application request"
        );
    }
}

#[tokio::test]
async fn export_resume_binds_exact_gateway_ca_bytes_and_accepts_the_same_snapshot() {
    use agentsandbox::model::Event;
    use axum::response::IntoResponse;
    use serde_json::json;
    let temp = support::private_tempdir();
    let pki = certificates(temp.path()).await;
    let (audit, credential) = provision(temp.path()).await;
    let mut event = Event::new("session.observed");
    event.session_id = Some("export-tls-session".into());
    audit.append(event).unwrap();
    grant_fixture_session(temp.path(), &audit, "export-tls-session");
    let exports = Arc::new(AtomicUsize::new(0));
    let observed = exports.clone();
    let router = gateway_router(GatewayState::new(audit, "runner-export-tls-fixture".into(), "viewer-export-tls-fixture".into(), Policy::default(), None, Redactor::new(vec![])))
        .layer(axum::middleware::from_fn(move |request: axum::extract::Request, next: axum::middleware::Next| {
            let observed = observed.clone();
            async move {
                if request.uri().path() == "/api/v1/human/export" && observed.fetch_add(1, Ordering::SeqCst) == 0 {
                    return (axum::http::StatusCode::SERVICE_UNAVAILABLE, [("cache-control", "no-store")], axum::Json(json!({"error":{"code":"audit_unavailable","message":"owned transport interruption","retry_action":"retry"}}))).into_response();
                }
                next.run(request).await
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("https://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(serve(
        listener,
        router,
        ServerIdentity::from_files(&pki.join("server-chain.pem"), &pki.join("server.key")).unwrap(),
        std::future::pending(),
    ));
    let ca = pki.join("root-ca.pem");
    let identity = output(&mut identity_command(&origin, &credential, Some(&ca))).await;
    assert!(identity.status.success());
    let identity: Value = serde_json::from_slice(&identity.stdout).unwrap();
    let context = identity["contexts"][0]["id"].as_str().unwrap();
    let destination = temp.path().join("resumable-tls");
    let export_command = |mode: &str, ca: &Path| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
        cmd.args(["human", mode, "--gateway", &origin, "--credential-file"])
            .arg(&credential)
            .arg("--gateway-ca-file")
            .arg(ca)
            .args(["--context", context, "--output-dir"])
            .arg(&destination);
        if mode == "export" {
            cmd.args([
                "--view",
                "metadata",
                "--resource-kind",
                "session",
                "--resource-id",
                "export-tls-session",
            ]);
        }
        cmd
    };
    let interrupted = output(&mut export_command("export", &ca)).await;
    no_credential(&interrupted, &credential);
    assert!(!interrupted.status.success());
    assert!(String::from_utf8_lossy(&interrupted.stderr).contains("HTTP 503"));
    let selection = temp.path().join("resumable-tls.preparing/selection.json");
    let retained = std::fs::read(&selection).unwrap();
    let altered = temp.path().join("altered-ca.pem");
    let bytes = std::fs::read(&ca).unwrap();
    // The same root duplicated is still valid trust; rejection must be caused
    // by the retained exact-byte binding, rather than an unrelated TLS error.
    public_ca_file(&altered, &bytes.repeat(2));
    let refused = output(&mut export_command("resume-export", &altered)).await;
    let refused_exports = exports.load(Ordering::SeqCst);
    let unchanged = std::fs::read(&selection).unwrap() == retained;
    let same = temp.path().join("same-ca-different-path.pem");
    public_ca_file(&same, &bytes);
    let resumed = output(&mut export_command("resume-export", &same)).await;
    server.abort();
    let _ = server.await;
    no_credential(&refused, &credential);
    no_credential(&resumed, &credential);
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr)
            .contains("retained export differs from the exact gateway")
    );
    assert_eq!(
        refused_exports, 1,
        "changed trust must fail before another export request"
    );
    assert!(
        unchanged,
        "refused trust must preserve the retained preparation"
    );
    assert!(
        resumed.status.success(),
        "same CA bytes at another checked path must resume"
    );
    assert!(destination.join("completion.json").exists());
}

async fn read_one_session_frame(
    response: &mut reqwest::Response,
    pending: &mut Vec<u8>,
) -> anyhow::Result<(String, String)> {
    use anyhow::{Context, ensure};
    const MAX_FRAME: usize = 64 * 1024;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(end) = pending.windows(2).position(|bytes| bytes == b"\n\n") {
                let frame: Vec<_> = pending.drain(..end + 2).collect();
                let frame = std::str::from_utf8(&frame)?;
                if frame.starts_with(':') {
                    continue;
                }
                ensure!(
                    frame.lines().any(|line| line == "event: human_audit"),
                    "expected human audit SSE frame"
                );
                let field = |prefix| frame.lines().find_map(|line| line.strip_prefix(prefix));
                let cursor = field("id: ")
                    .filter(|value| !value.is_empty())
                    .context("missing SSE cursor")?;
                let page: Value =
                    serde_json::from_str(field("data: ").context("missing SSE data")?)?;
                let items = page["items"].as_array().context("missing SSE items")?;
                ensure!(items.len() == 1, "expected exactly one audit item");
                let id = items[0]["metadata"]["id"]
                    .as_str()
                    .context("missing audit event ID")?;
                return Ok((cursor.to_owned(), id.to_owned()));
            }
            let chunk = response
                .chunk()
                .await?
                .context("stream ended before audit frame")?;
            ensure!(
                chunk.len() <= MAX_FRAME.saturating_sub(pending.len()),
                "SSE frame exceeded fixture bound"
            );
            pending.extend_from_slice(&chunk);
        }
    })
    .await
    .context("audit frame deadline")?
}

#[tokio::test]
async fn authenticated_sse_resumes_after_disconnect_over_loopback_tls() -> anyhow::Result<()> {
    exercise_authenticated_sse_over_loopback_tls(false).await
}

#[tokio::test]
async fn authenticated_sse_resumes_after_disconnect_through_loopback_tls_ingress()
-> anyhow::Result<()> {
    exercise_authenticated_sse_over_loopback_tls(true).await
}

async fn exercise_authenticated_sse_over_loopback_tls(through_ingress: bool) -> anyhow::Result<()> {
    use agentsandbox::{ingress::ingress_router, model::Event, runner::read_token};
    use anyhow::{Context, ensure};
    use axum::{
        body::Body,
        extract::Request,
        http::StatusCode,
        middleware::{self, Next},
        response::Response,
    };
    use futures_util::{FutureExt as _, StreamExt as _};
    use tokio::{
        sync::{Semaphore, oneshot},
        time::timeout,
    };

    let temp = support::private_tempdir();
    let pki = certificates(temp.path()).await;
    let (audit, credential) = provision(temp.path()).await;
    let session = "human-sse-tls-session";
    let publish = || -> anyhow::Result<String> {
        let mut event = Event::new("observer.coverage");
        event.session_id = Some(session.into());
        Ok(audit.append(event)?.event.id)
    };
    let initial = publish()?;
    grant_fixture_session(temp.path(), &audit, session);
    let token = read_token(&credential)?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .http1_only()
        .pool_max_idle_per_host(0)
        .redirect(reqwest::redirect::Policy::none())
        .tls_built_in_root_certs(false)
        .add_root_certificate(reqwest::Certificate::from_pem(&std::fs::read(
            pki.join("root-ca.pem"),
        )?)?)
        .connect_timeout(Duration::from_secs(5))
        .build()?;

    // Hold a test permit for the lifetime of the real server-side SSE body.
    let bodies = Arc::new(Semaphore::new(1));
    let gateway = gateway_router(GatewayState::new(
        audit.clone(),
        "runner-human-sse-tls-fixture".into(),
        "viewer-human-sse-tls-fixture".into(),
        Policy::default(),
        None,
        Redactor::new(vec![]),
    ))
    .layer(middleware::from_fn({
        let bodies = bodies.clone();
        move |request: Request, next: Next| {
            let bodies = bodies.clone();
            async move {
                let stream = request.uri().path() == "/api/v1/human/stream";
                let response = next.run(request).await;
                if !stream || response.status() != StatusCode::OK {
                    return response;
                }
                let Ok(permit) = bodies.try_acquire_owned() else {
                    return Response::builder()
                        .status(StatusCode::CONFLICT)
                        .body(Body::empty())
                        .unwrap();
                };
                let (parts, body) = response.into_parts();
                Response::from_parts(
                    parts,
                    Body::from_stream(body.into_data_stream().map(move |chunk| {
                        let _held = &permit;
                        chunk
                    })),
                )
            }
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let origin = format!("https://{}", listener.local_addr()?);
    let identity =
        ServerIdentity::from_files(&pki.join("server-chain.pem"), &pki.join("server.key"))?;
    let (router, upstream) = if through_ingress {
        let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let origin = format!("http://{}", upstream.local_addr()?);
        (ingress_router(&origin)?, Some((upstream, gateway)))
    } else {
        (gateway, None)
    };
    let (shutdown, stopped) = oneshot::channel();
    let stopped = async {
        let _ = stopped.await;
    }
    .shared();
    let serve_upstream = {
        let stopped = stopped.clone();
        async move {
            if let Some((listener, router)) = upstream {
                axum::serve(listener, router)
                    .with_graceful_shutdown(stopped)
                    .await?;
            }
            Ok::<_, std::io::Error>(())
        }
    };
    let mut server = tokio::spawn(async move {
        let (downstream, upstream) =
            tokio::join!(serve(listener, router, identity, stopped), serve_upstream,);
        downstream.context("TLS listener")?;
        upstream.context("gateway listener")
    });
    let outcome: anyhow::Result<()> = async {
        let identity: Value = timeout(Duration::from_secs(5), async {
            client
                .get(format!("{origin}/api/v1/human/identity"))
                .bearer_auth(&token)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await
        })
        .await
        .context("human identity deadline")??;
        let contexts = identity["contexts"]
            .as_array()
            .context("missing identity contexts")?;
        ensure!(
            contexts.len() == 1 && contexts[0]["tenant_id"] == "tenant-human-tls",
            "unexpected human reader context"
        );
        let context = contexts[0]["id"].as_str().context("missing context ID")?;
        let stream = || {
            client
                .get(format!("{origin}/api/v1/human/stream"))
                .bearer_auth(&token)
                .header("X-AiGuard-Context", context)
                .query(&[
                    ("view", "metadata"),
                    ("limit", "1"),
                    ("resource_kind", "session"),
                    ("resource_id", session),
                ])
        };
        let open = |request: reqwest::RequestBuilder| async {
            let response = timeout(Duration::from_secs(5), request.send())
                .await
                .context("SSE response deadline")??
                .error_for_status()?;
            ensure!(
                response
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .is_some_and(|value| value == "text/event-stream"),
                "expected an SSE response"
            );
            Ok::<_, anyhow::Error>(response)
        };
        let mut first = open(stream()).await?;
        let mut pending = Vec::new();
        let (cursor, received) = read_one_session_frame(&mut first, &mut pending).await?;
        ensure!(received == initial, "wrong first audit event");
        ensure!(
            bodies.available_permits() == 0,
            "first SSE body was not held"
        );
        drop(first);
        drop(
            timeout(Duration::from_secs(5), bodies.acquire())
                .await
                .context("first SSE body remained held")??,
        );

        let expected = publish()?;
        let sentinel = publish()?;
        let mut resumed = open(stream().query(&[("cursor", &cursor)])).await?;
        let mut pending = Vec::new();
        let (next, received) = read_one_session_frame(&mut resumed, &mut pending).await?;
        ensure!(
            received == expected && next != cursor,
            "resume did not deliver exactly the next event"
        );
        let (after_sentinel, received) = read_one_session_frame(&mut resumed, &mut pending).await?;
        ensure!(
            received == sentinel && after_sentinel != next && after_sentinel != cursor,
            "resume did not deliver the sentinel immediately after the next event"
        );
        drop(resumed);
        drop(
            timeout(Duration::from_secs(5), bodies.acquire())
                .await
                .context("resumed SSE body remained held")??,
        );
        Ok(())
    }
    .await;
    let _ = shutdown.send(());
    let closure = match timeout(Duration::from_secs(7), &mut server).await {
        Ok(result) => result
            .context("SSE fixture server join")?
            .context("SSE fixture server shutdown"),
        Err(_) => {
            server.abort();
            let _ = server.await;
            anyhow::bail!("SSE fixture listeners did not close within their deadline");
        }
    };
    outcome?;
    closure
}
