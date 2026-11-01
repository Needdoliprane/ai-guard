//! Authenticate Negotiate3 before any Grant on its own connection while real
//! Journal/receipts/IPC keep the independent, unchanged Stops2 path usable.
//! This does not qualify an engine effect, worker deadline or D5 retirement.
use super::*;
use crate::broker::enforcer::{EnforcerError, EnforcerResult};
use std::{
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    process::{Child, Command, Stdio},
};

const TORN_FRAME: &[u8] = &[0, 0, 0, 8, b'{'];

/// Preserve the role on failed setup, cleanup or oracle. Never adopt an older
/// role's files/process; a successful test alone releases this directory.
struct RoleFixture(Option<tempfile::TempDir>);
impl Drop for RoleFixture {
    fn drop(&mut self) {
        if let Some(directory) = self.0.take() {
            let _preserved_directory = directory.keep();
        }
    }
}

// Raw Child has no implicit kill/wait. Never overwrite an earlier retained
// handle; each invocation creates exactly one owned process.
static PRESERVED_CHILDREN: std::sync::Mutex<Vec<Child>> = std::sync::Mutex::new(Vec::new());
struct ChildOwner(Option<Child>);
impl Drop for ChildOwner {
    fn drop(&mut self) {
        if let Some(child) = self.0.take() {
            PRESERVED_CHILDREN
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .push(child);
        }
    }
}
impl ChildOwner {
    fn spawn(root: &Directory) -> Result<Self, &'static str> {
        let executable = std::env::current_exe().map_err(|_| "child_executable_unavailable")?;
        // Wrap the raw process immediately after spawn, before any further
        // observation, await or fallible setup. Drop only preserves it.
        let child = Command::new(executable)
            .args([
                "--exact",
                "broker::service::server::cancel_retry_tests::native_recovery_server_child",
                "--ignored",
            ])
            .env_clear()
            .env(RECOVERY_CHILD_ROOT, root.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "owned_child_spawn_failed")?;
        Ok(Self(Some(child)))
    }
    fn is_alive(&mut self) -> Result<bool, &'static str> {
        self.0
            .as_mut()
            .ok_or("owned_child_handle_missing")?
            .try_wait()
            .map(|status| status.is_none())
            .map_err(|_| "owned_child_liveness_unknown")
    }
    async fn published_endpoint(
        &mut self,
        root: &Directory,
    ) -> Result<crate::broker::local_ipc::EndpointRecordV1, &'static str> {
        let expires = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if !self.is_alive()? {
                return Err("owned_child_exited_before_publication");
            }
            if let Some(endpoint) = crate::broker::discovery::load_endpoint(root)
                .map_err(|_| "owned_endpoint_read_failed")?
            {
                return Ok(endpoint);
            }
            if std::time::Instant::now() >= expires {
                return Err("owned_endpoint_publication_budget_exhausted");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    async fn terminate(&mut self) -> Result<(), &'static str> {
        self.0
            .as_mut()
            .ok_or("owned_child_handle_missing")?
            .kill()
            .map_err(|_| "owned_child_signal_failed")?;
        let expires = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            let status = self
                .0
                .as_mut()
                .ok_or("owned_child_handle_missing")?
                .try_wait()
                .map_err(|_| "owned_child_terminal_unknown")?;
            if status.is_some() {
                // try_wait reaped the terminal child. Raw Child Drop neither
                // waits nor signals; no second implicit cleanup is possible.
                drop(self.0.take());
                return Ok(());
            }
            if std::time::Instant::now() >= expires {
                return Err("owned_child_terminal_budget_exhausted");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

async fn cleanup(
    child: &mut ChildOwner,
    is_extinct: impl Fn() -> bool,
    untracked_full_client: bool,
) -> Result<(), &'static str> {
    if untracked_full_client {
        return Err("unexpected_full_client_owners_not_attested");
    }
    let expires = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        // Extinction must be observed while the child remains alive. Killing
        // it first could allow a surviving monitor to spawn a replacement.
        if !child.is_alive()? {
            return Err("owned_child_not_alive_during_client_extinction");
        }
        if is_extinct() {
            break;
        }
        if std::time::Instant::now() >= expires {
            return Err("client_owner_extinction_budget_exhausted");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    child.terminate().await
}

fn same_private_file(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    before.file_type().is_file()
        && after.file_type().is_file()
        && before.dev() == after.dev()
        && before.ino() == after.ino()
        && before.uid() == after.uid()
        && before.nlink() == 1
        && after.nlink() == 1
        && before.mode() & 0o777 == 0o600
        && after.mode() & 0o777 == 0o600
}

type Negotiation = EnforcerResult<(Uuid, String, Uuid, Uuid, EnforcerResult<Value>)>;
struct Observations {
    epoch: Uuid,
    endpoint: Vec<u8>,
    original_cancels: bool,
    slot_count: usize,
    admission_closed: bool,
    full_refused: bool,
    status_after: EnforcerResult<Value>,
    negotiation: Negotiation,
}

async fn collect_observations(
    child: &mut ChildOwner,
    root: &Directory,
    projection: &Arc<CancelStopProjectionV1>,
    targets: &[(String, String)],
    untracked_full_client: &mut bool,
) -> Result<Observations, &'static str> {
    let endpoint = child.published_endpoint(root).await?;
    let epoch = endpoint.epoch();
    let endpoint = serde_json::to_vec(&endpoint).map_err(|_| "endpoint_snapshot_failed")?;
    let executable = std::env::current_exe().map_err(|_| "client_executable_unavailable")?;
    // The constructor's cfg(test) Weak capture occurs before its first await,
    // so the caller can attest owners even if this operation returns Err.
    let client = EnforcerStopClient::connect_or_spawn_for_recovery(root, &executable, 8)
        .await
        .map_err(|_| "stops2_connection_failed")?;
    let expires = std::time::Instant::now() + Duration::from_secs(3);
    while client.observed_watch_epoch() != Some(epoch) {
        if !child.is_alive()? {
            return Err("owned_child_exited_during_watch_setup");
        }
        if std::time::Instant::now() >= expires {
            return Err("stops2_watch_setup_budget_exhausted");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    client
        .deliver_projection(Arc::clone(projection), 0)
        .await
        .map_err(|_| "proven_stop_delivery_failed")?;
    let receipt_hash = projection.receipt().input_hash();
    let receipt_head = serde_json::to_value(projection.receipt().head())
        .map_err(|_| "confirmed_cancel_head_invalid")?;
    let mut original_cancels = true;
    for (arm, hash) in targets {
        let slot = client
            .query_stop(arm, hash)
            .await
            .map_err(|_| "exact_stop_reservation_query_failed")?;
        let wire = serde_json::to_value(&slot).map_err(|_| "stop_reservation_snapshot_failed")?;
        // The registration digest binds the original complete Arm/receipt,
        // CID/deadline/OwnedSnapshot/authority/invocation/profile.
        original_cancels &= slot.arm_id() == arm.as_str()
            && slot.registration_hash() == hash.as_str()
            && slot.cancel_command_id() == Some(projection.command_id())
            && slot.generation() == 0
            && wire["durable_cancel"]["input_hash"].as_str() == Some(receipt_hash)
            && wire["durable_cancel"]["confirmed_head"] == receipt_head;
    }
    let status = client
        .status()
        .await
        .map_err(|_| "initial_stops2_status_failed")?;
    // Legacy refusal may happen at lock max_bytes=0, not at a Full3 handshake.
    let legacy_full = EnforcerClient::connect_or_spawn(root, &executable, 8).await;
    let full_refused = legacy_full.is_err();
    if !full_refused {
        // A surprising Full success has no ownership witness in this test.
        // Do not kill its live peer or claim cleanup quiescence in that case.
        *untracked_full_client = true;
        drop(legacy_full);
        return Err("unexpected_legacy_full_success");
    }
    drop(legacy_full);
    let negotiation = client.negotiate_v3_frame_for_test().await;
    let status_after = client
        .status()
        .await
        .and_then(|status| serde_json::to_value(status).map_err(|_| EnforcerError::InvalidInput));
    let observed = Observations {
        epoch,
        endpoint,
        original_cancels,
        slot_count: status.slot_count(),
        admission_closed: !status.admission_available(),
        full_refused,
        status_after,
        negotiation,
    };
    // Neither this structure nor either Result carries any StopClient owner.
    drop(client);
    Ok(observed)
}

struct Anchors {
    endpoint: Vec<u8>,
    lock: fs::Metadata,
    auth: fs::Metadata,
    lock_bytes: Vec<u8>,
    auth_bytes: Vec<u8>,
}
fn read_anchors(root: &Directory) -> Result<Anchors, &'static str> {
    let endpoint = crate::broker::discovery::load_endpoint(root)
        .map_err(|_| "post_cleanup_endpoint_read_failed")?
        .ok_or("post_cleanup_endpoint_missing")?;
    Ok(Anchors {
        endpoint: serde_json::to_vec(&endpoint).map_err(|_| "post_cleanup_endpoint_invalid")?,
        lock: fs::symlink_metadata(root.path().join(".owner.lock"))
            .map_err(|_| "post_cleanup_lock_metadata_failed")?,
        auth: fs::symlink_metadata(root.path().join("auth"))
            .map_err(|_| "post_cleanup_auth_metadata_failed")?,
        lock_bytes: fs::read(root.path().join(".owner.lock"))
            .map_err(|_| "post_cleanup_lock_read_failed")?,
        auth_bytes: fs::read(root.path().join("auth"))
            .map_err(|_| "post_cleanup_auth_read_failed")?,
    })
}

#[tokio::test(flavor = "current_thread")]
async fn recovery_v3_negotiation_precedes_grant_without_changing_v2_stops() {
    let fixture = Fixture::with_manual_arm_count(2);
    let command = Uuid::new_v4();
    // Exactly one mutation; the R6c helper makes one confirming direct read
    // only after Unknown Io, preserving owners/nonterminal observers.
    let (fixture, projection) = append_multi_arm_setup_once(fixture, command).await;
    let owned = ObservationFixture(Some(fixture));
    if let Err(reason) = validate_confirmed_setup(owned.fixture(), command, &projection) {
        panic!("D5 setup did not confirm the exact two-Arm Cancel: {reason}");
    }
    assert!(
        projection.stops()[0].arm().operation_id != projection.stops()[1].arm().operation_id
            && projection.stops()[0].arm().inspection.container_id
                != projection.stops()[1].arm().inspection.container_id,
        "D5 setup requires two distinct confirmed targets of the same job"
    );
    let targets = projection
        .stops()
        .iter()
        .map(|target| {
            let (_, hash) = recovery_registration(projection.scope(), target);
            (target.arm().operation_id.clone(), hash)
        })
        .collect::<Vec<_>>();
    let projection = Arc::new(projection);
    let (role_temp, root) = recovery_role_fixture();
    let mut role = RoleFixture(Some(role_temp));
    let lock_path = root.path().join(".owner.lock");
    let auth_path = root.path().join("auth");
    let lock_before = fs::symlink_metadata(&lock_path).expect("inspect the owned fixture lock");
    let auth_before = fs::symlink_metadata(&auth_path).expect("inspect the owned fixture auth");
    let auth_bytes = fs::read(&auth_path).expect("retain the existing synthetic fixture auth");
    assert!(
        lock_before.len() == 0 && same_private_file(&lock_before, &lock_before),
        "D5 setup requires an empty private regular fixture lock before writing"
    );
    {
        // This fixture owns the inode and no child/lease exists yet. No
        // truncate/replacement or valid seal is manufactured.
        let mut lock = fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(&lock_path)
            .expect("open the owned fixture lock without replacing it");
        assert!(
            same_private_file(
                &lock_before,
                &lock.metadata().expect("inspect retained lock FD")
            ),
            "D5 setup must write the same private lock inode"
        );
        lock.write_all(TORN_FRAME)
            .expect("write the torn fixture frame");
        lock.sync_all().expect("synchronize the torn fixture frame");
        root.sync()
            .expect("synchronize the owned fixture role parent");
    }

    let mut child = match ChildOwner::spawn(&root) {
        Ok(child) => child,
        Err(reason) => panic!("D5 harness spawn failed before acquiring a child: {reason}"),
    };
    // No assertion/expect after spawn until the unique cleanup attempt.
    let mut untracked_full_client = false;
    let observations = collect_observations(
        &mut child,
        &root,
        &projection,
        &targets,
        &mut untracked_full_client,
    )
    .await;
    let clients = EnforcerStopClient::take_created_clients_for_test();
    // This Weak observation attests harness clients only, never actor join,
    // effect drainage or D5. It remains available even on setup Err.
    let cleanup = cleanup(&mut child, || clients.is_extinct(), untracked_full_client).await;
    if let Err(reason) = cleanup {
        panic!("D5 harness cleanup failed before negotiation oracle: {reason}");
    }
    let anchors = read_anchors(&root);

    let observed = observations.unwrap_or_else(|reason| {
        panic!("D5 harness setup failed after confirmed child cleanup: {reason}")
    });
    let anchors = anchors.unwrap_or_else(|reason| {
        panic!("D5 harness anchor check failed after confirmed child cleanup: {reason}")
    });
    assert!(
        observed.original_cancels,
        "both original Cancel receipts and registration bindings must survive"
    );
    assert!(
        observed.slot_count == 2,
        "one reservation per original Arm is required"
    );
    assert!(
        observed.admission_closed && observed.full_refused,
        "Full must remain unavailable"
    );
    assert!(
        anchors.endpoint == observed.endpoint,
        "no endpoint replacement is permitted after client extinction and child termination"
    );
    assert!(
        same_private_file(&lock_before, &anchors.lock) && anchors.lock_bytes == TORN_FRAME,
        "the same private torn lock must remain unrepaired"
    );
    assert!(
        same_private_file(&auth_before, &anchors.auth) && anchors.auth_bytes == auth_bytes,
        "existing fixture auth must remain unchanged"
    );
    assert!(
        observed
            .status_after
            .as_ref()
            .is_ok_and(|value| value["slot_count"] == 2 && value["admission_available"] == false),
        "the closed Status2 client must remain usable after the separate negotiation attempt"
    );
    let value = observed
        .status_after
        .expect("Status2 success was checked after child termination");
    assert!(
        value["epoch"] == serde_json::to_value(observed.epoch).unwrap()
            && value["ledger_state"]["state"] == "unverified"
            && value["ledger_state"]["reason"] == "recovery_transport_only",
        "the limited V2 view must keep its epoch, closed admission and Unverified ledger"
    );

    assert!(
        observed.negotiation.is_ok(),
        "Negotiate3 setup must validate the actual Challenge2, send the exact proof and retain the same endpoint"
    );
    let (negotiation_epoch, binding, nonce2, client_nonce, response) = observed
        .negotiation
        .expect("negotiation setup was checked after child termination");
    assert!(
        negotiation_epoch == observed.epoch && !client_nonce.is_nil() && client_nonce != nonce2,
        "the negotiation must name the owned epoch and use its own fresh client nonce"
    );
    // Intended R8 RED: AuthenticationV2 rejects Negotiate3 and closes this
    // separate stream. All real STOP2/setup/cleanup positives precede it.
    assert!(
        response.is_ok(),
        "authenticated Negotiate3 must receive Challenge3 before any Grant"
    );
    let frame = response.expect("Challenge3 receipt was checked after child termination");
    assert!(
        frame["kind"] == "challenge_v3" && frame["protocol"] == 3 && frame["role"] == "enforcer",
        "the first negotiation response must be Challenge3, not a Grant or Status"
    );
    assert!(
        frame["epoch"] == serde_json::to_value(observed.epoch).unwrap()
            && frame["root_binding"].as_str() == Some(binding.as_str())
            && frame["client_nonce"] == serde_json::to_value(client_nonce).unwrap(),
        "Challenge3 must correlate epoch, retained root binding and client nonce"
    );
    let server_nonce = frame
        .get("server_nonce")
        .and_then(Value::as_str)
        .and_then(|text| Uuid::parse_str(text).ok().map(|nonce| (text, nonce)));
    assert!(
        server_nonce.is_some_and(|(text, nonce)| !nonce.is_nil()
            && nonce != nonce2
            && nonce != client_nonce
            && nonce.to_string() == text)
            && frame.get("nonce").is_none(),
        "Challenge3 requires only server_nonce, canonical and fresh relative to both client and V2 nonces"
    );
    // Challenge3 is not Auth3/Grant3, verified ledger coverage or an effect.
    // Partial diagnostics and the complete D5 path remain NOT_RUN.
    drop(projection);
    drop(owned.into_fixture());
    drop(role.0.take());
}
