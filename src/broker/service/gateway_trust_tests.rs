//! Real local TLS/router/SQLite for the broker's operator trust contract.
//! The default TLS refusal and absent-field RED are distinct observations.
//! No business runtime, broker process, provider or external PKI is invoked.
use super::*;
use crate::{
    api::{GatewayState, gateway_router},
    audit::{EventQuery, Store},
    model::Event,
    policy::Policy,
    redaction::Redactor,
    runtime::kubernetes::admission::transport_tests::TlsMaterial,
    tls::{ClientTrust, ServerIdentity, serve},
};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::oneshot,
    task::JoinHandle,
};

const TENANT: &str = "tenant-broker-gateway-trust";
const TOKEN: &str = "runner-broker-gateway-trust-fixture-000000000000000000";
const EVENT_KIND: &str = "test.broker.gateway_trust";

fn private_write(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

struct OwnedGateway {
    origin: String,
    address: std::net::SocketAddr,
    calls: Arc<AtomicUsize>,
    store: Arc<Store>,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<std::io::Result<()>>>,
}
impl OwnedGateway {
    async fn start(parent: &Path, material: &TlsMaterial) -> Result<Self> {
        let store = Arc::new(Store::create_supervised_for_tenant(
            &parent.join("audit.sqlite"),
            TENANT,
            Redactor::new(vec![]),
            5000,
            &parent.join("before-v3.sqlite"),
        )?);
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&calls);
        let router = gateway_router(GatewayState::new(
            Arc::clone(&store),
            TOKEN.into(),
            "viewer-broker-gateway-trust-fixture-000000000000000000".into(),
            Policy::default(),
            None,
            Redactor::new(vec![]),
        ))
        .layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let seen = Arc::clone(&seen);
                async move {
                    seen.fetch_add(1, Ordering::SeqCst);
                    next.run(request).await
                }
            },
        ));
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let origin = format!("https://{address}");
        let identity = ServerIdentity::from_files(&material.certificate, &material.key)?;
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(serve(listener, router, identity, async move {
            let _ = stopped.await;
        }));
        Ok(Self {
            origin,
            address,
            calls,
            store,
            stop: Some(stop),
            task: Some(task),
        })
    }

    async fn finish(mut self) -> Result<()> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(mut task) = self.task.take() {
            match tokio::time::timeout(Duration::from_secs(10), &mut task).await {
                Ok(result) => result??,
                Err(_) => {
                    task.abort();
                    let _ = task.await;
                    anyhow::bail!("broker trust fixture TLS server did not join");
                }
            }
        }
        Ok(())
    }
}
impl Drop for OwnedGateway {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

fn registry_value(
    parent: &Path,
    origin: &str,
    namespace: &str,
    ca: Option<&Path>,
) -> Result<Value> {
    // This is the actual shipped operator example, not a parallel test schema.
    let mut input: Value = serde_json::from_str(include_str!(
        "../../../docs/setup/broker/examples/gateway-trust.json"
    ))?;
    input["root"] = json!(parent.join("broker-state"));
    let profile = &mut input["profiles"][0];
    profile["gateway"] = json!(origin);
    profile["tenant_id"] = json!(TENANT);
    profile["control_namespace_uuid"] = json!(namespace);
    profile["config_dir"] = json!(parent.join("config"));
    profile["runner_token_file"] = json!(parent.join("runner"));
    profile["runtime_program"] = json!("/usr/bin/false");
    profile["runtime_endpoint"] = json!("unix:///broker-trust-fixture-unused.sock");
    profile["source_roots"] = json!([parent.join("project")]);
    profile["export_roots"] = json!([parent.join("exports")]);
    if let Some(ca) = ca {
        profile["gateway_ca_bundle_file"] = json!(ca);
    } else {
        profile
            .as_object_mut()
            .unwrap()
            .remove("gateway_ca_bundle_file");
    }
    Ok(input)
}

fn load_registry(parent: &Path, input: &Value) -> Result<profile::Registry> {
    let path = parent.join(format!("registry-{}.json", Uuid::new_v4()));
    private_write(&path, &serde_json::to_vec(input)?)?;
    profile::Registry::load(&path)
}

fn registry(
    parent: &Path,
    origin: &str,
    namespace: &str,
    ca: Option<&Path>,
) -> Result<profile::Registry> {
    load_registry(parent, &registry_value(parent, origin, namespace, ca)?)
}

struct Observed {
    default_error: Option<protocol::BrokerErrorCodeV1>,
    default_http: usize,
    v1_error: Option<protocol::BrokerErrorCodeV1>,
    v1_http: usize,
    explicit_field_accepted: bool,
    explicit_identity_ok: bool,
    explicit_http: usize,
    persisted_exact_event: bool,
    wrong_name_error: Option<protocol::BrokerErrorCodeV1>,
    wrong_name_http: usize,
    wrong_name_tcp_connected: bool,
    invalid_configs_rejected: usize,
    invalid_config_http: usize,
    changed_source_reload_refused: bool,
}

async fn observe(parent: &Path, tls: &TlsMaterial, gateway: &OwnedGateway) -> Result<Observed> {
    let namespace = gateway
        .store
        .supervision_state()?
        .control_namespace_uuid
        .ok_or_else(|| anyhow!("TLS fixture lacks its actual control namespace"))?;
    // Positive precondition: real TLS validation and the real protocol route
    // work with this CA. The subsequent default failure is not a dead server.
    let trusted = ClientTrust::from_ca_file(&tls.ca)?
        .apply_to_client(
            reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(15)),
        )?
        .build()?;
    let response = trusted
        .get(format!(
            "{}/control/api/v1/control/protocol",
            gateway.origin
        ))
        .bearer_auth(TOKEN)
        .send()
        .await?;
    ensure!(
        response.status() == reqwest::StatusCode::OK,
        "TLS fixture protocol precondition refused"
    );
    let protocol: crate::control::ControlProtocolV1 = response.json().await?;
    ensure!(
        protocol.tenant_id == TENANT && protocol.role == "runner" && protocol.active,
        "TLS fixture does not expose the expected real runner identity"
    );
    let default_registry = registry(parent, &gateway.origin, &namespace, None)?;
    let before = gateway.calls.load(Ordering::SeqCst);
    let default =
        VerifiedProfile::authenticate(&default_registry, "strict-docker", TOKEN, false).await;
    let default_http = gateway.calls.load(Ordering::SeqCst) - before;
    let default_error = default.err();

    let mut legacy = registry_value(parent, &gateway.origin, &namespace, None)?;
    legacy["version"] = json!(1);
    legacy["profiles"][0]
        .as_object_mut()
        .unwrap()
        .remove("prearmed_start_profile");
    let v1 = load_registry(parent, &legacy)?;
    let before = gateway.calls.load(Ordering::SeqCst);
    let v1_error = VerifiedProfile::authenticate(&v1, "strict-docker", TOKEN, false)
        .await
        .err();
    let v1_http = gateway.calls.load(Ordering::SeqCst) - before;

    // Each invalid case differs from the same accepted operator input. No
    // identity request is needed to reject unusable trust before IPC opens.
    let malformed = parent.join("malformed-ca.pem");
    private_write(&malformed, b"not a certificate")?;
    let oversized = parent.join("oversized-ca.pem");
    let mut padded = fs::read(&tls.ca)?;
    padded.resize(65_537, b' ');
    private_write(&oversized, &padded)?;
    let excessive = parent.join("excessive-ca.pem");
    private_write(&excessive, &fs::read(&tls.ca)?.repeat(9))?;
    let http = gateway.origin.replacen("https://", "http://", 1);
    let invalid_before = gateway.calls.load(Ordering::SeqCst);
    let mut invalid_configs_rejected = 0;
    for (origin, ca) in [
        (http.as_str(), tls.ca.clone()),
        (gateway.origin.as_str(), parent.join("missing-ca.pem")),
        (gateway.origin.as_str(), malformed),
        (gateway.origin.as_str(), oversized),
        (gateway.origin.as_str(), excessive),
    ] {
        invalid_configs_rejected +=
            usize::from(registry(parent, origin, &namespace, Some(&ca)).is_err());
    }
    legacy["profiles"][0]["gateway_ca_bundle_file"] = json!(tls.ca);
    invalid_configs_rejected += usize::from(load_registry(parent, &legacy).is_err());
    let invalid_config_http = gateway.calls.load(Ordering::SeqCst) - invalid_before;

    // On unchanged code this is a closed-config rejection, separately from
    // the real default TLS refusal observed above. No new API is needed to
    // compile this pre-implementation regression.
    let snapshot_ca = parent.join("operator-ca.pem");
    private_write(&snapshot_ca, &fs::read(&tls.ca)?)?;
    let declared = registry(parent, &gateway.origin, &namespace, Some(&snapshot_ca));
    let explicit_field_accepted = declared.is_ok();
    let mut explicit_identity_ok = false;
    let mut persisted_exact_event = false;
    let mut wrong_name_error = None;
    let mut wrong_name_http = 0;
    let mut wrong_name_tcp_connected = false;
    let mut changed_source_reload_refused = false;
    let before = gateway.calls.load(Ordering::SeqCst);
    if let Ok(declared) = declared {
        // The approved roots were captured before IPC admission. Replacing
        // their source must not silently change this retained registry/client.
        fs::write(&snapshot_ca, b"changed after registry load")?;
        changed_source_reload_refused =
            registry(parent, &gateway.origin, &namespace, Some(&snapshot_ca)).is_err();
        if let Ok(verified) =
            VerifiedProfile::authenticate(&declared, "strict-docker", TOKEN, false).await
        {
            explicit_identity_ok = true;
            let event = Event::new(EVENT_KIND);
            let seq = verified
                .client
                .post_event(serde_json::to_value(&event)?)
                .await?;
            let stored = gateway.store.events(&EventQuery {
                kind: Some(EVENT_KIND.into()),
                ..EventQuery::default()
            })?;
            persisted_exact_event = stored
                .iter()
                .any(|stored| stored.seq == seq && stored.event.id == event.id);
        }
        let wrong_origin = gateway.origin.replacen("127.0.0.1", "localhost", 1);
        // A DNS or connect failure must fail this fixture, not satisfy the SAN
        // oracle. Connect under the exact wrong name and confirm the same live
        // listener before asking the product client to verify that name.
        let stream = tokio::time::timeout(
            Duration::from_secs(3),
            TcpStream::connect(("localhost", gateway.address.port())),
        )
        .await
        .map_err(|_| anyhow!("SAN fixture TCP precondition timed out"))??;
        ensure!(
            stream.peer_addr()? == gateway.address,
            "SAN fixture connected to a different listener"
        );
        wrong_name_tcp_connected = true;
        drop(stream);
        let wrong = registry(parent, &wrong_origin, &namespace, Some(&tls.ca))?;
        let wrong_before = gateway.calls.load(Ordering::SeqCst);
        wrong_name_error = VerifiedProfile::authenticate(&wrong, "strict-docker", TOKEN, false)
            .await
            .err();
        wrong_name_http = gateway.calls.load(Ordering::SeqCst) - wrong_before;
    }
    Ok(Observed {
        default_error,
        default_http,
        v1_error,
        v1_http,
        explicit_field_accepted,
        explicit_identity_ok,
        explicit_http: gateway.calls.load(Ordering::SeqCst) - before,
        persisted_exact_event,
        wrong_name_error,
        wrong_name_http,
        wrong_name_tcp_connected,
        invalid_configs_rejected,
        invalid_config_http,
        changed_source_reload_refused,
    })
}

#[tokio::test]
async fn operator_gateway_ca_reaches_broker_authentication_and_durable_audit_without_tls_bypass() {
    let temporary = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let parent = temporary.path().canonicalize().unwrap();
    for directory in ["broker-state", "config", "project", "exports"] {
        fs::create_dir(parent.join(directory)).unwrap();
        fs::set_permissions(parent.join(directory), fs::Permissions::from_mode(0o700)).unwrap();
    }
    private_write(&parent.join("runner"), TOKEN.as_bytes()).unwrap();
    let tls = TlsMaterial::create().await;
    let gateway = OwnedGateway::start(&parent, &tls).await.unwrap();
    let observed = observe(&parent, &tls, &gateway).await;
    // Always join the owned TLS server before any regression assertion,
    // including the expected missing-field RED.
    let joined = gateway.finish().await;
    joined.unwrap();
    let observed = observed.unwrap();
    eprintln!(
        "broker_gateway_trust default_error={:?} default_http={} declared_field={} explicit_identity={} explicit_http={} exact_event={} wrong_name_error={:?} wrong_name_http={}",
        observed.default_error,
        observed.default_http,
        observed.explicit_field_accepted,
        observed.explicit_identity_ok,
        observed.explicit_http,
        observed.persisted_exact_event,
        observed.wrong_name_error,
        observed.wrong_name_http
    );
    assert_eq!(
        observed.default_error,
        Some(protocol::BrokerErrorCodeV1::Unavailable)
    );
    assert_eq!(
        observed.default_http, 0,
        "default trust must fail before authenticated HTTP"
    );
    assert_eq!(
        observed.v1_error,
        Some(protocol::BrokerErrorCodeV1::Unavailable)
    );
    assert_eq!(observed.v1_http, 0);
    assert_eq!(observed.invalid_configs_rejected, 6);
    assert_eq!(observed.invalid_config_http, 0);
    assert!(
        observed.explicit_field_accepted,
        "the explicit operator gateway CA field is absent on the pre-implementation source"
    );
    assert!(
        observed.explicit_identity_ok && observed.persisted_exact_event,
        "the same verified broker audit client must retain explicit trust through durable writes"
    );
    assert_eq!(observed.explicit_http, 2);
    assert!(
        observed.changed_source_reload_refused,
        "a fresh registry must reject the changed malformed CA while the retained snapshot still works"
    );
    assert!(
        observed.wrong_name_tcp_connected,
        "SAN rejection requires a successful controlled TCP connection first"
    );
    assert_eq!(
        observed.wrong_name_error,
        Some(protocol::BrokerErrorCodeV1::Unavailable)
    );
    assert_eq!(
        observed.wrong_name_http, 0,
        "explicit CA must not disable server name validation"
    );
}
