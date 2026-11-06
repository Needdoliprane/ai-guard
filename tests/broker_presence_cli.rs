//! Presence is published by the real foreground broker into the authenticated
//! tenant registry. No worker, engine, provider or model is started here.
#![cfg(unix)]

mod support;

use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::Store,
    broker::enforcer::EnforcerClient,
    broker::service::{
        self,
        protocol::{BrokerCommandV1, BrokerErrorCodeV1, BrokerReplyV1},
    },
    broker_registry::{BrokerAuthorityV1, BrokerGrantMutationV1, BrokerRuntimeKindV1},
    control::QuotaPolicyV1,
    control_fs::Directory,
    policy::Policy,
    redaction::Redactor,
    workload::{WorkloadBindingIssueV1, WorkloadSubjectV1},
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, Response},
    middleware::{self, Next},
    serve,
};
use serde_json::Value;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs, io,
    os::unix::{
        fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        process::ExitStatusExt,
    },
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tempfile::TempDir;
use tokio::{
    net::TcpListener,
    process::{Child, Command},
    sync::Semaphore,
    task::JoinHandle,
};
use uuid::Uuid;

const TENANT: &str = "tenant-broker-presence-cli";
const BROKER_ID: &str = "22222222-2222-4222-8222-222222222222";
const INCARNATION_ID: &str = "33333333-3333-4333-8333-333333333333";
const CREDENTIAL_ID: &str = "44444444-4444-4444-8444-444444444444";
const AUTHORITY_ID: &str = "declared-presence-cli-authority";
const BROKER_TOKEN: &str = "broker-presence-cli-token-aaaaaaaaaaaaaaaa";
const RUNNER_TOKEN: &str = "runner-presence-cli-token-bbbbbbbbbbbbbbbb";

struct Gateway {
    url: String,
    task: JoinHandle<()>,
}

struct TemporaryGatewayRefusals {
    remaining: Mutex<usize>,
    observed: Semaphore,
    posts: Mutex<Vec<Vec<u8>>>,
}
impl TemporaryGatewayRefusals {
    fn new(count: usize) -> Self {
        Self {
            remaining: Mutex::new(count),
            observed: Semaphore::new(0),
            posts: Mutex::new(Vec::new()),
        }
    }
}

async fn refuse_temporary_heartbeat(
    faults: Arc<TemporaryGatewayRefusals>,
    request: Request<Body>,
    next: Next,
) -> Response<Body> {
    if request.uri().path() != "/api/v1/brokers/heartbeat" {
        return next.run(request).await;
    }
    let (parts, body) = request.into_parts();
    let bytes = to_bytes(body, 16_384).await.unwrap();
    faults.posts.lock().unwrap().push(bytes.to_vec());
    let refused = {
        let mut remaining = faults.remaining.lock().unwrap();
        if *remaining == 0 {
            false
        } else {
            *remaining -= 1;
            true
        }
    };
    if refused {
        faults.observed.add_permits(1);
        return Response::builder()
            .status(503)
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap();
    }
    next.run(Request::from_parts(parts, Body::from(bytes)))
        .await
}

impl Drop for Gateway {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn private_write(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).expect("write private test fixture");
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .expect("protect private test fixture");
}

fn start_gateway(root: &Path) -> (Arc<Store>, Gateway, Uuid) {
    start_gateway_with_refusals(root, Arc::new(TemporaryGatewayRefusals::new(0)))
}

fn start_gateway_with_refusals(
    root: &Path,
    faults: Arc<TemporaryGatewayRefusals>,
) -> (Arc<Store>, Gateway, Uuid) {
    let db = root.join("tenant.sqlite");
    let store =
        Arc::new(Store::open_for_tenant(&db, TENANT, Redactor::new(vec![]), 10_000).unwrap());
    store
        .migrate_control_v3(&db.with_extension("before3"))
        .unwrap();
    store
        .migrate_identity_v4(&db.with_extension("before4"))
        .unwrap();
    store
        .migrate_human_access_v5(&db.with_extension("before5"))
        .unwrap();
    store
        .migrate_workload_v6(&db.with_extension("before6"))
        .unwrap();
    let namespace = Uuid::parse_str(
        &store
            .supervision_state()
            .unwrap()
            .control_namespace_uuid
            .unwrap(),
    )
    .expect("schema6 control namespace is a canonical UUID");

    store
        .issue_workload_binding(&WorkloadBindingIssueV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            credential_id: CREDENTIAL_ID.into(),
            expected_revision: 0,
            subject: WorkloadSubjectV1::Broker {
                broker_id: BROKER_ID.into(),
                incarnation_id: INCARNATION_ID.into(),
            },
            sha256_digest: hex::encode(Sha256::digest(BROKER_TOKEN.as_bytes())),
            expires_at: "2100-01-01T00:00:00.000Z".into(),
        })
        .unwrap();
    store
        .mutate_broker_grant(&BrokerGrantMutationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            broker_id: BROKER_ID.into(),
            expected_revision: None,
            credential_id: CREDENTIAL_ID.into(),
            incarnation_id: INCARNATION_ID.into(),
            authorities: vec![BrokerAuthorityV1 {
                authority_id: AUTHORITY_ID.into(),
                runtime_kind: BrokerRuntimeKindV1::Docker,
            }],
            lease_seconds: 30,
            not_after: "2100-01-01T00:00:00.000Z".into(),
        })
        .unwrap();

    let state = GatewayState::for_tenants(
        vec![TenantGateway {
            tenant_id: TENANT.into(),
            store: Arc::clone(&store),
            runner_token: RUNNER_TOKEN.into(),
            viewer_token: "viewer-presence-cli-token-cccccccccccccccc".into(),
        }],
        Policy::default(),
        None,
        Redactor::new(vec![BROKER_TOKEN.into(), RUNNER_TOKEN.into()]),
    )
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let listener = TcpListener::from_std(listener).unwrap();
    let app = gateway_router(state).layer(middleware::from_fn(move |request, next| {
        refuse_temporary_heartbeat(Arc::clone(&faults), request, next)
    }));
    let task = tokio::spawn(async move {
        serve(listener, app).await.unwrap();
    });
    (
        store,
        Gateway {
            url: format!("http://{address}"),
            task,
        },
        namespace,
    )
}

fn create_registry(root: &Path, gateway: &str, namespace: Uuid) -> (PathBuf, PathBuf) {
    let broker_root = root.join("broker-state");
    let config_dir = root.join("config");
    let source = root.join("source");
    let export = root.join("exports");
    for path in [&broker_root, &config_dir, &source, &export] {
        fs::create_dir(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let credentials = root.join("credentials");
    fs::create_dir(&credentials).unwrap();
    fs::set_permissions(&credentials, fs::Permissions::from_mode(0o700)).unwrap();
    let broker_token = credentials.join("broker-token");
    let runner_token = credentials.join("runner-token");
    private_write(&broker_token, BROKER_TOKEN.as_bytes());
    private_write(&runner_token, RUNNER_TOKEN.as_bytes());
    let presence = config_dir.join("presence.json");
    private_write(
        &presence,
        &serde_json::to_vec(&json!({
            "version": 1,
            "broker_id": BROKER_ID,
            "credential_id": CREDENTIAL_ID,
            "incarnation_id": INCARNATION_ID,
            "authority_id": AUTHORITY_ID,
            "broker_token_file": broker_token,
            "ca_bundle_file": null,
            "state_dir": config_dir.join("presence-state"),
            "interval_seconds": 5,
            "max_operations": 16,
            "max_state_bytes": 1048576
        }))
        .unwrap(),
    );
    fs::create_dir(config_dir.join("presence-state")).unwrap();
    fs::set_permissions(
        config_dir.join("presence-state"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();

    let registry = config_dir.join("registry.json");
    let value = json!({
        "version": 2,
        "installation_id": "11111111-1111-4111-8111-111111111111",
        "root": broker_root,
        "max_enforcer_slots": 8,
        "profiles": [{
            "id": "presence-cli",
            "gateway": gateway,
            "tenant_id": TENANT,
            "control_namespace_uuid": namespace,
            "config_dir": config_dir,
            "runner_token_file": runner_token,
            "runtime": "docker",
            "runtime_program": "/usr/bin/false",
            "runtime_endpoint": "unix:///run/docker.sock",
            "image": "worker:presence-fixture",
            "policy_profile": "strict",
            "network": "none",
            "allow_rootful_runtime": false,
            "max_timeout_seconds": 30,
            "allow_capture_output": false,
            "managed_clients": false,
            "providers_file": null,
            "manual_credential_file": null,
            "source_roots": [source],
            "export_roots": [export],
            "prearmed_start_profile": {"version": 1, "mode": "legacy"},
            "presence_file": presence
        }],
        "additional_control_secret_files": []
    });
    private_write(&registry, &serde_json::to_vec(&value).unwrap());
    (registry, broker_root)
}

fn command(root: &Path, registry: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
    command
        .args(["broker", "serve", "--config"])
        .arg(registry)
        .env_clear()
        .env("TMPDIR", root)
        .env("HOME", root)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

fn recovery_command(root: &Path, registry: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
    command
        .args(["broker", "serve", "--config"])
        .arg(registry)
        .arg("--recovery-only")
        .env_clear()
        .env("TMPDIR", root)
        .env("HOME", root)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

fn recovery_helper_command(helper_root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
    command
        .args(["__broker-enforcer", "--root"])
        .arg(helper_root)
        .args(["--max-slots", "8", "--recovery-only"])
        .env_clear()
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    command
}

fn effective_uid() -> u32 {
    // SAFETY: read-only query of the test process credentials.
    unsafe { libc::geteuid() }
}

fn role_lease_available(root: &Path) -> io::Result<bool> {
    use std::os::fd::AsRawFd;
    let path = root.join(".owner.lock");
    let file = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(error),
    };
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file()
        || metadata.uid() != effective_uid()
        || metadata.mode() & 0o7777 != 0o600
        || metadata.len() != 0
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "private helper lease has unexpected metadata",
        ));
    }
    // SAFETY: a shared nonblocking flock observes the lease; it is released below.
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) };
    if result == 0 {
        let unlocked = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
        if unlocked != 0 {
            return Err(io::Error::last_os_error());
        }
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    if error.kind() == io::ErrorKind::WouldBlock {
        Ok(false)
    } else {
        Err(error)
    }
}

async fn wait_for_role_lease_release(root: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if role_lease_available(root).is_ok_and(|available| available) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn announcement_names(root: &Path) -> io::Result<BTreeSet<String>> {
    let mut names = BTreeSet::new();
    for entry in fs::read_dir(root.join("announcements"))? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with("e-") && name.ends_with(".json") {
            names.insert(name.to_owned());
        }
    }
    Ok(names)
}

fn new_valid_announcement_exists(root: &Path, before: &BTreeSet<String>) -> bool {
    let Ok(entries) = fs::read_dir(root.join("announcements")) else {
        return false;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if before.contains(name) || !name.starts_with("e-") || !name.ends_with(".json") {
            continue;
        }
        let Ok(metadata) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if !metadata.file_type().is_file()
            || metadata.uid() != effective_uid()
            || metadata.mode() & 0o7777 != 0o600
            || metadata.len() > 4096
        {
            continue;
        }
        let Ok(bytes) = fs::read(entry.path()) else {
            continue;
        };
        let Ok(record) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        if record["version"] == 1
            && record["sequence"]
                .as_u64()
                .is_some_and(|sequence| sequence > 0)
            && record["endpoint"]["epoch"].as_str().is_some()
        {
            return true;
        }
    }
    false
}

async fn wait_for_recovery_helper(
    helper_root: &Path,
    before: &BTreeSet<String>,
    child: &mut Child,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let running = child.try_wait().ok().flatten().is_none();
        let announced = new_valid_announcement_exists(helper_root, before);
        // Do not probe with LOCK_SH before the child publishes: its exclusive
        // nonblocking startup lease could otherwise lose a fixture race.
        let owns_lease =
            announced && role_lease_available(helper_root).is_ok_and(|available| !available);
        if running && announced && owns_lease {
            return true;
        }
        if !running || Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

struct DrainedOutput {
    retained: Vec<u8>,
    contains_secret: bool,
}

async fn drain_bounded<R: tokio::io::AsyncRead + Unpin>(mut stream: R) -> DrainedOutput {
    use tokio::io::AsyncReadExt;
    let mut retained = Vec::new();
    let mut buffer = [0u8; 4096];
    let secrets = [BROKER_TOKEN.as_bytes(), RUNNER_TOKEN.as_bytes()];
    let overlap = secrets
        .iter()
        .map(|value| value.len())
        .max()
        .unwrap_or(1)
        .saturating_sub(1);
    let mut carry = Vec::new();
    let mut contains_secret = false;
    loop {
        let count = stream.read(&mut buffer).await.unwrap_or(0);
        if count == 0 {
            break;
        }
        let keep = (65_536usize.saturating_sub(retained.len())).min(count);
        retained.extend_from_slice(&buffer[..keep]);
        carry.extend_from_slice(&buffer[..count]);
        if secrets
            .iter()
            .any(|secret| carry.windows(secret.len()).any(|window| window == *secret))
        {
            contains_secret = true;
        }
        if carry.len() > overlap {
            let drain = carry.len() - overlap;
            carry.drain(..drain);
        }
    }
    DrainedOutput {
        retained,
        contains_secret,
    }
}

async fn wait_for_heartbeat(store: &Store, child: &mut Child, timeout: Duration) -> Option<u64> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(Some(view)) = store.broker(BROKER_ID) {
            if let Some(receipt) = view.broker.last_heartbeat {
                return Some(receipt.sequence);
            }
        }
        if child.try_wait().ok().flatten().is_some() {
            return None;
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_for_recovery_reply(
    broker_root: &Path,
    child: &mut Child,
    timeout: Duration,
) -> Option<BrokerReplyV1> {
    let deadline = Instant::now() + timeout;
    loop {
        let request = tokio::time::timeout(
            Duration::from_secs(2),
            service::request(
                broker_root,
                "presence-cli",
                RUNNER_TOKEN,
                BrokerCommandV1::RecoveryInventory {
                    after: None,
                    limit: 1,
                },
            ),
        )
        .await;
        if let Ok(Ok(reply)) = request {
            return Some(reply);
        }
        if child.try_wait().ok().flatten().is_some() || Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn prepare_empty_full_journal(broker_root: &Path) -> bool {
    matches!(
        tokio::time::timeout(
            Duration::from_secs(10),
            service::request(
                broker_root,
                "presence-cli",
                RUNNER_TOKEN,
                BrokerCommandV1::List {
                    after: None,
                    limit: 1,
                },
            ),
        )
        .await,
        Ok(Ok(BrokerReplyV1::Page {
            entries,
            next_after: None
        })) if entries.is_empty()
    )
}

async fn wait_child(child: &mut Child, timeout: Duration) -> Option<std::process::ExitStatus> {
    tokio::time::timeout(timeout, child.wait())
        .await
        .ok()
        .and_then(Result::ok)
}

#[tokio::test]
async fn real_broker_serve_publishes_durable_presence_and_stops_after_shutdown() {
    exercise_real_broker_presence(0).await;
}

#[tokio::test]
async fn real_broker_serve_recovers_presence_after_four_temporary_gateway_refusals() {
    exercise_real_broker_presence(4).await;
}

async fn exercise_real_broker_presence(temporary_refusals: usize) {
    let fixture: TempDir = support::private_tempdir();
    let root = fixture.path().canonicalize().unwrap();
    let faults = Arc::new(TemporaryGatewayRefusals::new(temporary_refusals));
    let (store, _gateway, namespace) = start_gateway_with_refusals(&root, Arc::clone(&faults));
    let initial = store.broker(BROKER_ID).unwrap().unwrap();
    assert!(initial.broker.last_heartbeat.is_none());
    assert_eq!(initial.broker.heartbeat_revision, 0);
    assert!(store.worker_usage().unwrap().is_empty());
    let quota_before = store.worker_quota(&QuotaPolicyV1::default()).unwrap();
    assert_eq!(quota_before.held_workers, 0);

    let (registry, broker_root) = create_registry(&root, &_gateway.url, namespace);
    let mut child = command(&root, &registry)
        .spawn()
        .expect("spawn real broker CLI");
    let stdout = tokio::spawn(drain_bounded(child.stdout.take().unwrap()));
    let stderr = tokio::spawn(drain_bounded(child.stderr.take().unwrap()));

    let refusals_observed = if temporary_refusals == 0 {
        true
    } else {
        match tokio::time::timeout(
            Duration::from_secs(30),
            faults
                .observed
                .acquire_many(u32::try_from(temporary_refusals).unwrap()),
        )
        .await
        {
            Ok(Ok(permits)) => {
                permits.forget();
                true
            }
            _ => false,
        }
    };
    let publication_watchdog = if temporary_refusals == 0 { 90 } else { 30 };
    let sequence = wait_for_heartbeat(
        &store,
        &mut child,
        Duration::from_secs(publication_watchdog),
    )
    .await;
    let initial_publication = sequence.is_some();
    let service_still_running = child.try_wait().ok().flatten().is_none();
    // Four refused heartbeat requests also prove the service reached its
    // initialized publisher. Retain the existing helper for cleanup even when
    // the baseline publisher stops before its first successful heartbeat.
    let publisher_reached = initial_publication || (temporary_refusals > 0 && refusals_observed);
    let enforcer = if publisher_reached && service_still_running {
        let enforcer_root = Directory::open_existing(&broker_root.join("enforcer")).ok();
        let executable = PathBuf::from(env!("CARGO_BIN_EXE_agentsandbox"))
            .canonicalize()
            .ok();
        match (enforcer_root, executable) {
            (Some(root), Some(executable)) => {
                EnforcerClient::connect_or_spawn(&root, &executable, 8)
                    .await
                    .ok()
            }
            _ => None,
        }
    } else {
        None
    };

    // Always attempt normal shutdown and reap the directly owned process before
    // assertions. If it exceeds the guard, kill and reap it, then preserve its
    // private fixture root for the owner sandbox's final inventory.
    let already_exited = child.try_wait().ok().flatten();
    let signal_result = if already_exited.is_some() {
        -1
    } else if let Some(pid) = child.id() {
        // SAFETY: signal only the still-running, directly spawned broker child.
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGINT) }
    } else {
        -1
    };
    let mut exit = match already_exited {
        Some(status) => Some(status),
        None => wait_child(&mut child, Duration::from_secs(45)).await,
    };
    let mut forced_cleanup = false;
    if exit.is_none() {
        forced_cleanup = true;
        let _ = child.start_kill();
        exit = wait_child(&mut child, Duration::from_secs(5)).await;
    }
    // Only stop the helper after serve has exited, using the already connected
    // public client. Never reconnect and risk implicitly creating a new helper.
    let helper_shutdown = if let Some(enforcer) = enforcer.as_ref() {
        enforcer.shutdown_if_idle().await.is_ok()
    } else {
        !initial_publication
    };
    drop(enforcer);

    let stopped_revision = store
        .broker(BROKER_ID)
        .ok()
        .flatten()
        .map(|view| view.broker.heartbeat_revision);
    let worker_usage_empty = store.worker_usage().is_ok_and(|usage| usage.is_empty());
    let quota_empty = store
        .worker_quota(&QuotaPolicyV1::default())
        .is_ok_and(|usage| usage.held_workers == 0);
    if initial_publication && stopped_revision.is_some() {
        tokio::time::sleep(Duration::from_secs(6)).await;
    }
    let stable_after_interval = match (stopped_revision, store.broker(BROKER_ID).ok().flatten()) {
        (Some(revision), Some(view)) => {
            view.broker.heartbeat_revision == revision
                && sequence.is_some_and(|expected| {
                    view.broker
                        .last_heartbeat
                        .as_ref()
                        .is_some_and(|receipt| receipt.sequence == expected)
                })
        }
        _ => false,
    };
    let stdout = stdout.await.unwrap_or_else(|_| DrainedOutput {
        retained: Vec::new(),
        contains_secret: false,
    });
    let stderr = stderr.await.unwrap_or_else(|_| DrainedOutput {
        retained: Vec::new(),
        contains_secret: false,
    });
    let outputs_secret_free = !stdout.contains_secret && !stderr.contains_secret;
    let exit_success = exit.is_some_and(|status| status.success());
    let heartbeat_revision = store
        .broker(BROKER_ID)
        .ok()
        .flatten()
        .map(|view| view.broker.heartbeat_revision)
        .unwrap_or(0);
    let same_pending_recovered = {
        let posts = faults.posts.lock().unwrap();
        temporary_refusals == 0
            || (posts.len() >= temporary_refusals + 1
                && posts[..=temporary_refusals]
                    .iter()
                    .all(|post| post == &posts[0]))
    };
    let success = refusals_observed
        && same_pending_recovered
        && initial_publication
        && sequence.is_some_and(|value| value > 0)
        && stopped_revision.is_some_and(|revision| revision >= 1)
        && exit_success
        && signal_result == 0
        && helper_shutdown
        && !forced_cleanup
        && stable_after_interval
        && worker_usage_empty
        && quota_empty
        && outputs_secret_free;
    if !success {
        let preserved = fixture.keep();
        panic!(
            "broker presence CLI oracle failed: temporary_refusals={temporary_refusals}, refusals_observed={refusals_observed}, same_pending_recovered={same_pending_recovered}, published={initial_publication}, positive_sequence={}, exit_success={exit_success}, signal_sent={}, helper_shutdown={helper_shutdown}, forced_cleanup={forced_cleanup}, stable_after_interval={stable_after_interval}, heartbeat_revision={heartbeat_revision}, worker_usage_empty={worker_usage_empty}, quota_empty={quota_empty}, output_secret_free={outputs_secret_free}, retained_output_bytes={}; private fixture retained at {preserved:?}",
            sequence.is_some_and(|value| value > 0),
            signal_result == 0,
            stdout.retained.len() + stderr.retained.len()
        );
    }
}

#[tokio::test]
async fn recovery_serve_answers_authenticated_rpc_without_opening_missing_presence() {
    let fixture: TempDir = support::private_tempdir();
    let root = fixture.path().canonicalize().unwrap();
    let (store, _gateway, namespace) = start_gateway(&root);
    let (registry, broker_root) = create_registry(&root, &_gateway.url, namespace);
    let config_dir = registry.parent().unwrap();
    let presence = config_dir.join("presence.json");
    let preserved_presence = config_dir.join("presence.json.retained");

    let mut full = command(&root, &registry)
        .spawn()
        .expect("spawn Full broker CLI");
    let full_stdout = tokio::spawn(drain_bounded(full.stdout.take().unwrap()));
    let full_stderr = tokio::spawn(drain_bounded(full.stderr.take().unwrap()));
    let first_sequence = wait_for_heartbeat(&store, &mut full, Duration::from_secs(90)).await;
    let full_ready = first_sequence.is_some();

    let enforcer_root_path = broker_root.join("enforcer");
    // Connect after Full publishes presence; close that helper through its public API.
    let enforcer = if full_ready && full.try_wait().ok().flatten().is_none() {
        match (
            Directory::open_existing(&enforcer_root_path).ok(),
            PathBuf::from(env!("CARGO_BIN_EXE_agentsandbox"))
                .canonicalize()
                .ok(),
        ) {
            (Some(root), Some(executable)) => {
                EnforcerClient::connect_or_spawn(&root, &executable, 8)
                    .await
                    .ok()
            }
            _ => None,
        }
    } else {
        None
    };
    let full_journal_prepared =
        if full_ready && enforcer.is_some() && full.try_wait().ok().flatten().is_none() {
            prepare_empty_full_journal(&broker_root).await
        } else {
            false
        };

    let full_already_exited = full.try_wait().ok().flatten();
    let full_signal = if full_already_exited.is_some() {
        -1
    } else if let Some(pid) = full.id() {
        // SAFETY: signal only the still-running, directly spawned Full child.
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGINT) }
    } else {
        -1
    };
    let mut full_exit = match full_already_exited {
        Some(status) => Some(status),
        None => wait_child(&mut full, Duration::from_secs(45)).await,
    };
    let mut full_forced_cleanup = false;
    if full_exit.is_none() {
        full_forced_cleanup = true;
        let _ = full.start_kill();
        full_exit = wait_child(&mut full, Duration::from_secs(5)).await;
    }
    let full_shutdown_ok = full_exit.is_some_and(|status| status.success())
        && full_signal == 0
        && !full_forced_cleanup;
    let full_helper_shutdown = if let Some(enforcer) = enforcer.as_ref() {
        enforcer.shutdown_if_idle().await.is_ok()
    } else {
        false
    };
    drop(enforcer);
    let full_helper_stopped =
        wait_for_role_lease_release(&enforcer_root_path, Duration::from_secs(10)).await;
    let revision_before_recovery = store
        .broker(BROKER_ID)
        .ok()
        .flatten()
        .map(|view| view.broker.heartbeat_revision);
    let presence_preserved = fs::rename(&presence, &preserved_presence).is_ok()
        && !presence.exists()
        && preserved_presence.is_file();

    let previous_helper_announcements = announcement_names(&enforcer_root_path).ok();
    let mut recovery_helper = if full_ready
        && full_shutdown_ok
        && full_helper_shutdown
        && full_helper_stopped
        && full_journal_prepared
        && presence_preserved
    {
        recovery_helper_command(&enforcer_root_path).spawn().ok()
    } else {
        None
    };
    let recovery_helper_ready = match (
        previous_helper_announcements.as_ref(),
        recovery_helper.as_mut(),
    ) {
        (Some(previous), Some(child)) => {
            wait_for_recovery_helper(
                &enforcer_root_path,
                previous,
                child,
                Duration::from_secs(10),
            )
            .await
        }
        _ => false,
    };
    let should_start_recovery = full_ready
        && full_shutdown_ok
        && full_helper_shutdown
        && full_helper_stopped
        && full_journal_prepared
        && presence_preserved
        && recovery_helper_ready;
    let mut recovery_reply = None;
    let mut recovery_signal = -1;
    let mut recovery_exit = None;
    let mut recovery_forced_cleanup = false;
    let mut recovery_stdout = None;
    let mut recovery_stderr = None;
    if should_start_recovery {
        if let Ok(mut recovery) = recovery_command(&root, &registry).spawn() {
            let stdout = tokio::spawn(drain_bounded(recovery.stdout.take().unwrap()));
            let stderr = tokio::spawn(drain_bounded(recovery.stderr.take().unwrap()));
            recovery_reply =
                wait_for_recovery_reply(&broker_root, &mut recovery, Duration::from_secs(90)).await;
            let already_exited = recovery.try_wait().ok().flatten();
            recovery_signal = if already_exited.is_some() {
                -1
            } else if let Some(pid) = recovery.id() {
                // SAFETY: signal only the still-running, directly spawned recovery child.
                unsafe { libc::kill(pid as libc::pid_t, libc::SIGINT) }
            } else {
                -1
            };
            recovery_exit = match already_exited {
                Some(status) => Some(status),
                None => wait_child(&mut recovery, Duration::from_secs(45)).await,
            };
            if recovery_exit.is_none() {
                recovery_forced_cleanup = true;
                let _ = recovery.start_kill();
                recovery_exit = wait_child(&mut recovery, Duration::from_secs(5)).await;
            }
            recovery_stdout = Some(stdout.await.unwrap_or(DrainedOutput {
                retained: Vec::new(),
                contains_secret: false,
            }));
            recovery_stderr = Some(stderr.await.unwrap_or(DrainedOutput {
                retained: Vec::new(),
                contains_secret: false,
            }));
        }
    }

    let recovery_helper_kill_reaped = if let Some(helper) = recovery_helper.as_mut() {
        let already_exited = helper.try_wait().ok().flatten();
        let kill_sent = if already_exited.is_some() {
            false
        } else {
            helper.start_kill().is_ok()
        };
        let exit = match already_exited {
            Some(status) => Some(status),
            None => wait_child(helper, Duration::from_secs(5)).await,
        };
        kill_sent && exit.is_some_and(|status| status.signal() == Some(libc::SIGKILL))
    } else {
        false
    };
    drop(recovery_helper);
    let recovery_helper_stopped =
        wait_for_role_lease_release(&enforcer_root_path, Duration::from_secs(5)).await;
    let full_stdout = full_stdout.await.unwrap_or(DrainedOutput {
        retained: Vec::new(),
        contains_secret: false,
    });
    let full_stderr = full_stderr.await.unwrap_or(DrainedOutput {
        retained: Vec::new(),
        contains_secret: false,
    });
    let recovery_outputs_secret_free = recovery_stdout
        .as_ref()
        .is_none_or(|output| !output.contains_secret)
        && recovery_stderr
            .as_ref()
            .is_none_or(|output| !output.contains_secret);
    let recovery_exit_success = recovery_exit.is_some_and(|status| status.success());
    let authenticated_rpc = matches!(
        recovery_reply.as_ref(),
        Some(BrokerReplyV1::RecoveryInventory { .. })
            | Some(BrokerReplyV1::Error {
                code: BrokerErrorCodeV1::NeedsReconciliation
            })
    );
    let rpc_error = match recovery_reply.as_ref() {
        Some(BrokerReplyV1::Error { code }) => Some(*code),
        _ => None,
    };
    let rpc_reply_kind = match recovery_reply.as_ref() {
        Some(BrokerReplyV1::RecoveryInventory { .. }) => "recovery_inventory",
        Some(BrokerReplyV1::Error { .. }) => "service_error",
        Some(_) => "other_reply",
        None => "no_framed_reply",
    };
    let revision_after_recovery = store
        .broker(BROKER_ID)
        .ok()
        .flatten()
        .map(|view| view.broker.heartbeat_revision);
    let recovery_left_presence_missing = !presence.exists() && preserved_presence.is_file();
    tokio::time::sleep(Duration::from_secs(6)).await;
    let revision_after_interval = store
        .broker(BROKER_ID)
        .ok()
        .flatten()
        .map(|view| view.broker.heartbeat_revision);
    let worker_usage_empty = store.worker_usage().is_ok_and(|usage| usage.is_empty());
    let quota_empty = store
        .worker_quota(&QuotaPolicyV1::default())
        .is_ok_and(|usage| usage.held_workers == 0);
    let success = full_ready
        && full_shutdown_ok
        && full_helper_shutdown
        && full_helper_stopped
        && presence_preserved
        && recovery_helper_ready
        && authenticated_rpc
        && recovery_exit_success
        && recovery_signal == 0
        && !recovery_forced_cleanup
        && recovery_helper_kill_reaped
        && recovery_helper_stopped
        && recovery_left_presence_missing
        && revision_before_recovery.is_some_and(|revision| revision > 0)
        && revision_after_recovery == revision_before_recovery
        && revision_after_interval == revision_before_recovery
        && worker_usage_empty
        && quota_empty
        && !full_stdout.contains_secret
        && !full_stderr.contains_secret
        && recovery_outputs_secret_free;
    if !success {
        let preserved = fixture.keep();
        panic!(
            "recovery-only presence isolation failed: full_ready={full_ready}, full_journal_prepared={full_journal_prepared}, full_shutdown_ok={full_shutdown_ok}, full_helper_shutdown={full_helper_shutdown}, full_helper_stopped={full_helper_stopped}, presence_preserved={presence_preserved}, recovery_helper_ready={recovery_helper_ready}, authenticated_rpc={authenticated_rpc}, rpc_reply_kind={rpc_reply_kind}, rpc_error={rpc_error:?}, recovery_exit_success={recovery_exit_success}, recovery_signal_sent={}, recovery_forced_cleanup={recovery_forced_cleanup}, recovery_helper_kill_reaped={recovery_helper_kill_reaped}, recovery_helper_stopped={recovery_helper_stopped}, presence_still_missing={recovery_left_presence_missing}, revision_before={revision_before_recovery:?}, revision_after={revision_after_recovery:?}, revision_after_interval={revision_after_interval:?}, worker_usage_empty={worker_usage_empty}, quota_empty={quota_empty}, output_secret_free={}, retained_output_bytes={}; private fixture retained at {preserved:?}",
            recovery_signal == 0,
            !full_stdout.contains_secret
                && !full_stderr.contains_secret
                && recovery_outputs_secret_free,
            full_stdout.retained.len()
                + full_stderr.retained.len()
                + recovery_stdout
                    .as_ref()
                    .map_or(0, |output| output.retained.len())
                + recovery_stderr
                    .as_ref()
                    .map_or(0, |output| output.retained.len())
        );
    }
}
