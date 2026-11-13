//! C1 product regression: real operator Registry, authenticated context, TLS,
//! SQLite, workload provisioning and retained binding/IO/client construction.
//! Runtime observations use the existing declared Store fixture; no exec
//! capability, helper handoff, engine or provider is fabricated or qualified.
use super::*;
use crate::{
    api::{GatewayState, gateway_router},
    audit::Store,
    broker::discovery::RoleLease,
    control::*,
    model::{CommandSpec, DecisionRequest},
    policy::{self, Policy},
    redaction::Redactor,
    relay::{
        RelayAckV1, RelayEndpointV1, RelayGrantV1, RelaySendV1,
        channel::control::TenantRelayIo,
        client::{RelayClientError, RelayClientProfile, WorkerRelayClient},
    },
    runtime::kubernetes::admission::transport_tests::TlsMaterial,
    tls::{ServerIdentity, serve},
    workload::{WorkloadBindingIssueV1, WorkloadSubjectV1},
};
use anyhow::Context;
use clap::Parser;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{Semaphore, oneshot},
    task::JoinHandle,
};

// Reuse the real typed Store transitions, including import, verification, Arm
// and claim. Their runtime facts stay explicitly synthetic.
#[allow(dead_code)]
#[path = "../../../tests/support/relay_admission_fixture.rs"]
mod software_workers;

const TENANT: &str = "tenant-relay-profile-tls";
const TOKEN: &str = "public-relay-profile-tls-runner-fixture-000000000000";
const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const HASH_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

fn private_write(path: &Path, bytes: &[u8]) -> Result<()> {
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
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<std::io::Result<()>>>,
}
impl OwnedGateway {
    async fn start(store: Arc<Store>, tls: &TlsMaterial) -> Result<Self> {
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&calls);
        let app = gateway_router(GatewayState::new(
            store,
            TOKEN.into(),
            "public-relay-profile-tls-viewer-fixture-000000000000".into(),
            Policy::default(),
            None,
            Redactor::new(vec![TOKEN.into()]),
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
        let identity = ServerIdentity::from_files(&tls.certificate, &tls.key)?;
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(serve(listener, app, identity, async move {
            let _ = stopped.await;
        }));
        Ok(Self {
            origin: format!("https://{address}"),
            address,
            calls,
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
                    anyhow::bail!("relay TLS fixture server did not join");
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

fn registry(
    parent: &Path,
    origin: &str,
    namespace: &str,
    ca: &Path,
    operations: &[PathBuf],
) -> Result<profile::Registry> {
    let mut input: Value = serde_json::from_str(include_str!(
        "../../../docs/setup/broker/examples/gateway-trust.json"
    ))?;
    input["root"] = json!(parent.join("broker-state"));
    let profile = &mut input["profiles"][0];
    profile["gateway"] = json!(origin);
    profile["gateway_ca_bundle_file"] = json!(ca);
    profile["tenant_id"] = json!(TENANT);
    profile["control_namespace_uuid"] = json!(namespace);
    profile["config_dir"] = json!(parent.join("config"));
    profile["runner_token_file"] = json!(parent.join("runner"));
    profile["runtime_program"] = json!("/usr/bin/false");
    profile["runtime_endpoint"] = json!("unix:///unused-relay-tls-fixture.sock");
    profile["source_roots"] = json!([parent.join("project")]);
    profile["export_roots"] = json!([parent.join("exports")]);
    profile["relay"] = json!({"version":1,"workload_operations":operations});
    let path = parent.join(format!("registry-{}.json", Uuid::new_v4()));
    private_write(&path, &serde_json::to_vec(&input)?)?;
    profile::Registry::load(&path)
}

#[derive(Parser)]
struct Provision {
    #[command(flatten)]
    workload: crate::workload_cli::WorkloadArgs,
}
async fn issue(parent: &Path, control: &Path, worker: &WorkerV1, leaf: &str) -> Result<PathBuf> {
    let manifest = parent.join(format!("issue-{leaf}.json"));
    private_write(
        &manifest,
        &serde_json::to_vec(&json!({
            "action":"issue","version":1,"expected_revision":0,
            "subject":{"kind":"worker","worker_id":worker.worker_id,
                "attempt_id":worker.attempt.attempt_id,"generation":1},
            "expires_at":(chrono::Utc::now()+chrono::Duration::minutes(5)).to_rfc3339()
        }))?,
    )?;
    let operation = parent.join("issued").join(leaf);
    // Real in-process product CLI: prepare a private proof, then persist the
    // actual HTTP receipt. No parser-only receipt or handcrafted credential.
    let prepare = Provision::try_parse_from([
        "fixture",
        "prepare",
        "--input",
        manifest.to_str().context("fixture manifest")?,
        "--control-file",
        control.to_str().context("fixture control")?,
        "--output-dir",
        operation.to_str().context("fixture operation")?,
    ])?;
    crate::workload_cli::run(prepare.workload).await?;
    let apply = Provision::try_parse_from([
        "fixture",
        "apply",
        "--operation-dir",
        operation.to_str().context("fixture operation")?,
        "--control-file",
        control.to_str().context("fixture control")?,
    ])?;
    crate::workload_cli::run(apply.workload).await?;
    Ok(operation)
}

async fn exercise(
    parent: &Path,
    store: &Store,
    workers: &[WorkerV1; 2],
    tls: &TlsMaterial,
    other_tls: &TlsMaterial,
    gateway: &OwnedGateway,
    owners: &mut Vec<Arc<TenantRelayIo>>,
) -> Result<()> {
    let namespace = store
        .supervision_state()?
        .control_namespace_uuid
        .ok_or_else(|| anyhow!("fixture has no actual control namespace"))?;
    let control = parent.join("control.json");
    private_write(
        &control,
        &serde_json::to_vec(&json!({
            "version":1,"gateway":gateway.origin,"expected_tenant_id":TENANT,
            "control_namespace_uuid":namespace,"runner_token_file":parent.join("runner"),
            "ca_bundle_file":tls.ca
        }))?,
    )?;
    let operations = [
        issue(parent, &control, &workers[0], "source").await?,
        issue(parent, &control, &workers[1], "destination").await?,
    ];
    let ca = parent.join("profile-ca.pem");
    private_write(&ca, &fs::read(&tls.ca)?)?;
    let retained = registry(parent, &gateway.origin, &namespace, &ca, &operations)?;

    // Each negative uses a real Registry and must fail before HTTP or Store
    // mutation. No untrusted profile is converted into a fake context.
    let before = store.head()?;
    let calls = gateway.calls.load(Ordering::SeqCst);
    let bad_ca = registry(
        parent,
        &gateway.origin,
        &namespace,
        &other_tls.ca,
        &operations,
    )?;
    ensure!(
        matches!(
            VerifiedProfile::authenticate(&bad_ca, "strict-docker", TOKEN, true).await,
            Err(protocol::BrokerErrorCodeV1::Unavailable)
        ),
        "wrong CA must refuse broker authentication"
    );
    let wrong_name = format!("https://localhost:{}", gateway.address.port());
    let tcp = tokio::time::timeout(
        Duration::from_secs(3),
        TcpStream::connect(("localhost", gateway.address.port())),
    )
    .await
    .map_err(|_| anyhow!("SAN fixture TCP precondition timed out"))??;
    ensure!(
        tcp.peer_addr()? == gateway.address,
        "SAN fixture reached another peer"
    );
    drop(tcp);
    let bad_name = registry(parent, &wrong_name, &namespace, &ca, &operations)?;
    ensure!(
        matches!(
            VerifiedProfile::authenticate(&bad_name, "strict-docker", TOKEN, true).await,
            Err(protocol::BrokerErrorCodeV1::Unavailable)
        ),
        "wrong SAN must refuse broker authentication"
    );
    ensure!(
        gateway.calls.load(Ordering::SeqCst) == calls && store.head()? == before,
        "TLS refusals reached HTTP or changed durable Store state"
    );

    // The path now contains a DIFFERENT valid CA. Authentication and all later
    // worker clients must use the original Registry snapshot, not this path.
    fs::write(&ca, fs::read(&other_tls.ca)?)?;
    let reloaded = registry(parent, &gateway.origin, &namespace, &ca, &operations)?;
    ensure!(
        matches!(
            VerifiedProfile::authenticate(&reloaded, "strict-docker", TOKEN, true).await,
            Err(protocol::BrokerErrorCodeV1::Unavailable)
        ),
        "a fresh wrong-CA snapshot must refuse"
    );
    ensure!(
        gateway.calls.load(Ordering::SeqCst) == calls && store.head()? == before,
        "fresh wrong-CA refusal reached HTTP or changed Store state"
    );
    let verified = VerifiedProfile::authenticate(&retained, "strict-docker", TOKEN, true)
        .await
        .map_err(|_| anyhow!("original Registry CA snapshot was lost"))?;
    let root = Directory::open_existing(&parent.join("broker-state"))?;
    let cancel = CancelGeneration::new();
    let context = verified.open_context(
        &retained,
        retained.profile("strict-docker")?,
        &root,
        cancel.clone(),
        preclaim::PreclaimControlV1::new(cancel),
        None,
    )?;
    let role_root = root
        .persistent_child("role", true)?
        .context("fixture role directory")?;
    let role = Arc::new(RoleLease::acquire(&role_root)?);
    let business = root
        .persistent_child("relay-io", true)?
        .context("fixture IO directory")?;
    let io = TenantRelayIo::open(
        &business,
        context.scope(),
        role,
        vec![TOKEN.into()],
        Arc::new(Semaphore::new(32 * 1024 * 1024)),
        Arc::new(Semaphore::new(4)),
        Arc::new(Semaphore::new(128 * 1024 * 1024)),
        Arc::new(Semaphore::new(16)),
    )
    .map_err(|_| anyhow!("real tenant IO did not open"))?;
    owners.push(io.clone());
    let first = io
        .issued_binding(operations[0].clone(), context.scope().clone())
        .await
        .map_err(|_| anyhow!("source issued binding was refused"))?
        .context("source issued binding missing")?;
    let second = io
        .issued_binding(operations[1].clone(), context.scope().clone())
        .await
        .map_err(|_| anyhow!("destination issued binding was refused"))?
        .context("destination issued binding missing")?;
    // Negative worker HTTP checks use the actual retained workload proof and
    // existing public CA-file API, which shares the constructor's TLS policy.
    // This never supplies a free trust parameter to the authenticated bridge.
    let proof_file = first.credential_file();
    let proof = std::str::from_utf8(proof_file.bytes())?.trim_end_matches('\n');
    let bad_worker_ca = WorkerRelayClient::with_profile_and_ca_file(
        &gateway.origin,
        proof,
        first.endpoint().clone(),
        RelayClientProfile::ControlIngress,
        &other_tls.ca,
    )?;
    let bad_worker_name = WorkerRelayClient::with_profile_and_ca_file(
        &wrong_name,
        proof,
        first.endpoint().clone(),
        RelayClientProfile::ControlIngress,
        &tls.ca,
    )?;
    let before = store.head()?;
    let calls = gateway.calls.load(Ordering::SeqCst);
    ensure!(
        matches!(
            bad_worker_ca.inbox(None, 1).await,
            Err(RelayClientError::Transport)
        ),
        "wrong worker CA must refuse TLS"
    );
    ensure!(
        matches!(
            bad_worker_name.inbox(None, 1).await,
            Err(RelayClientError::Transport)
        ),
        "wrong worker SAN must refuse TLS"
    );
    ensure!(
        gateway.calls.load(Ordering::SeqCst) == calls && store.head()? == before,
        "worker TLS refusals reached HTTP or changed Store state"
    );
    let grant = RelayGrantV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        grant_id: Uuid::new_v4().to_string(),
        source: first.endpoint().clone(),
        destination: second.endpoint().clone(),
        channel: "tasks".into(),
        expires_at: (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339(),
    };
    store.relay_grant(&grant)?;
    let source = io
        .authenticated_binding_client_for_tls_test(&context, first)
        .await
        .map_err(|_| anyhow!("source worker client construction failed"))?;
    let destination = io
        .authenticated_binding_client_for_tls_test(&context, second)
        .await
        .map_err(|_| anyhow!("destination worker client construction failed"))?;
    let send = RelaySendV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        grant_id: grant.grant_id,
        ttl_seconds: 60,
        body: "public C1 TLS product regression".into(),
    };
    source
        .send(&send)
        .await
        .context("retained worker Send must use the same Registry TLS snapshot")?;
    let inbox = destination.inbox(None, 8).await?;
    ensure!(
        inbox.items.len() == 1
            && inbox.items[0].body == send.body
            && inbox.items[0].metadata.message_id == send.operation_id,
        "real TLS Inbox did not preserve the exact message"
    );
    let ack = RelayAckV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        message_id: send.operation_id,
    };
    let receipt = destination.ack(&ack).await?;
    ensure!(
        receipt.resource_id == ack.message_id && !receipt.replayed,
        "real TLS recipient ACK was not recorded"
    );
    let head = store.head()?;
    let replay = destination.ack(&ack).await?;
    ensure!(
        replay.replayed
            && replay.audit_seq == receipt.audit_seq
            && replay.audit_hash == receipt.audit_hash
            && store.head()? == head,
        "exact ACK replay changed the Store"
    );
    Ok(())
}

#[tokio::test]
async fn retained_worker_clients_keep_real_registry_ca_snapshot_through_send_inbox_ack()
-> Result<()> {
    let temporary = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()?;
    let parent = temporary.path().canonicalize()?;
    for name in ["broker-state", "config", "project", "exports", "issued"] {
        fs::create_dir(parent.join(name))?;
        fs::set_permissions(parent.join(name), fs::Permissions::from_mode(0o700))?;
    }
    private_write(&parent.join("runner"), TOKEN.as_bytes())?;
    let tls = TlsMaterial::create().await;
    let other_tls = TlsMaterial::create().await;
    let store = Arc::new(software_workers::fresh(&parent, TENANT, 10_000));
    let workers = [
        software_workers::running_now(&store),
        software_workers::running_now(&store),
    ];
    let gateway = OwnedGateway::start(store.clone(), &tls).await?;
    let mut owners = Vec::new();
    let result = exercise(
        &parent,
        &store,
        &workers,
        &tls,
        &other_tls,
        &gateway,
        &mut owners,
    )
    .await;
    // Join real IO and TLS owners before reporting the original RED/oracle.
    let mut cleanup_failed = false;
    for owner in owners {
        cleanup_failed |= owner.join_pending().await.is_err();
    }
    cleanup_failed |= gateway.finish().await.is_err();
    match (result, cleanup_failed) {
        (Err(error), true) => Err(error.context("owned C1 fixture cleanup also failed")),
        (Err(error), false) => Err(error),
        (Ok(()), true) => Err(anyhow!("owned C1 fixture cleanup failed")),
        (Ok(()), false) => Ok(()),
    }
}
