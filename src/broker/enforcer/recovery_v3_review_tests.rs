//! Four source-review regressions. These fixtures own every modified anchor.
//! Correct MACs precede each mutation; no old guard or runtime is touched.
use super::*;
use std::os::unix::fs::MetadataExt;

/// Unlike the accepting-loop fixture, this owner accepts only on explicit
/// demand. A later accept check cannot accidentally close an old connection
/// and make a missing per-connection checkpoint look correct.
pub(super) struct AnchorFixture {
    temp: Option<tempfile::TempDir>,
    root: Directory,
    listener: Option<PrivateListener>,
    state: Arc<RecoveryState>,
    progress: Option<tokio::task::JoinHandle<()>>,
    connections: tokio::task::JoinSet<EnforcerResult<()>>,
    replacements: Vec<PrivateListener>,
    replacement_sockets: Vec<std::os::unix::net::UnixListener>,
}
impl Drop for AnchorFixture {
    fn drop(&mut self) {
        self.connections.abort_all();
        if let Some(task) = self.progress.take() {
            task.abort();
        }
        if let Some(temp) = self.temp.take() {
            let _preserved = temp.keep();
        }
    }
}
impl AnchorFixture {
    pub(super) fn new() -> Self {
        let base = if cfg!(target_os = "macos") {
            "/private/tmp"
        } else {
            "/tmp"
        };
        let temp = tempfile::Builder::new()
            .prefix("v3-review-")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(base)
            .unwrap();
        let root = Directory::open_existing(&temp.path().canonicalize().unwrap()).unwrap();
        root.persistent_create("auth", &[0x5a; 32]).unwrap();
        root.persistent_create(".owner.lock", &[]).unwrap();
        root.persistent_child("endpoints", true).unwrap().unwrap();
        root.persistent_child("announcements", true)
            .unwrap()
            .unwrap();
        let lease = EnforcerRoleLeaseV2::acquire_existing(&root).unwrap();
        let listener = PrivateListener::bind_new(&lease.listener_root().unwrap()).unwrap();
        let binding = lease.root_binding(listener.endpoint()).unwrap();
        let mut state = RecoveryState::new(
            listener.endpoint().epoch(),
            8,
            [0x5a; 32],
            binding,
            None,
            Some(lease.clone()),
        );
        state
            .retain_transport_v3(&root, listener.endpoint())
            .unwrap();
        // Do not inherit fault marker/barrier paths from the test process.
        state.faults = TestFaults {
            stage: "spawn_once".into(),
            arm_id: None,
            armed: AtomicBool::new(true),
            barrier_arm: None,
            barrier_root: None,
            barrier_once: AtomicBool::new(true),
            repeat_barrier: false,
            started_marker: None,
            fault_marker: None,
        };
        lease.publish_endpoint(listener.endpoint()).unwrap();
        let state = Arc::new(state);
        let ticking = Arc::clone(&state);
        let progress = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(25));
            loop {
                interval.tick().await;
                ticking.progress.fetch_add(1, Ordering::SeqCst);
            }
        });
        Self {
            temp: Some(temp),
            root,
            listener: Some(listener),
            state,
            progress: Some(progress),
            connections: tokio::task::JoinSet::new(),
            replacements: Vec::new(),
            replacement_sockets: Vec::new(),
        }
    }
    fn listener(&self) -> &PrivateListener {
        self.listener.as_ref().unwrap()
    }
    async fn accepted_pair(&self) -> EnforcerResult<(UnixStream, UnixStream)> {
        let endpoints = self
            .root
            .persistent_child("endpoints", false)
            .map_err(|_| EnforcerError::UnsafeFilesystem)?
            .ok_or(EnforcerError::UnsafeFilesystem)?;
        let (server, client) = tokio::join!(
            self.listener().accept_checked(),
            self.listener().endpoint().connect_checked(&endpoints)
        );
        Ok((server?, client?))
    }
    pub(super) async fn preface(&mut self) -> EnforcerResult<(UnixStream, ChallengeV2)> {
        let (server, mut client) = self.accepted_pair().await?;
        self.connections
            .spawn(serve_connection(server, Arc::clone(&self.state)));
        let preface: ChallengeV2 = read_frame(&mut client, HANDSHAKE_FRAME, IO_TIMEOUT).await?;
        if preface.epoch != self.listener().endpoint().epoch()
            || preface.root_binding
                != recovery_root_binding(&self.root, self.listener().endpoint())?
        {
            return Err(EnforcerError::Unauthorized);
        }
        Ok((client, preface))
    }
    pub(super) async fn connection(&mut self) -> EnforcerResult<StopConnection> {
        let (mut stream, challenge) = self.preface().await?;
        let session = client_handshake(
            &mut stream,
            &challenge,
            &self.state.capability,
            Instant::now() + HANDSHAKE_WINDOW,
            RequestedRole::StopsOnly,
            8,
        )
        .await?;
        // This is the actual normal client-side final anchor check, before any
        // fault. It must not be omitted to manufacture a server-only failure.
        if recovery_root_binding(&self.root, self.listener().endpoint())? != challenge.root_binding
        {
            return Err(EnforcerError::UnsafeFilesystem);
        }
        Ok(StopConnection {
            stream,
            epoch: challenge.epoch,
            v3: Some(session),
        })
    }
    fn request(&self, with_cancel: bool) -> EnforcerResult<RestoreRequiredStopV1> {
        let mut registration = unavailable_registration(self.root.path(), "review-known-stop");
        registration.arm.clock.observed_utc = None;
        registration.receipt.input_hash = registration
            .arm
            .input_hash()
            .map_err(|_| EnforcerError::InvalidInput)?;
        registration.validate()?;
        let head =
            RevisionRefV1::new(1, &"b".repeat(64)).map_err(|_| EnforcerError::InvalidInput)?;
        let cancel = with_cancel.then(|| CancelReferenceV1 {
            command_id: Uuid::new_v4(),
            input_hash: "c".repeat(64),
            confirmed_head: head.clone(),
        });
        let value = RestoreRequiredStopV1 {
            version: 1,
            request_id: Uuid::new_v4(),
            registration_hash: registration.digest()?,
            source: StopSourceV1 {
                scope_hash: "d".repeat(64),
                job_id: Uuid::new_v4(),
                intent_hash: "e".repeat(64),
                confirmed_head: head,
                arm_operation_index: 0,
            },
            registration,
            cancel_generation: 0,
            durable_cancel: cancel.clone(),
            cancel_prefix: cancel.into_iter().collect(),
        };
        value.validate()?;
        Ok(value)
    }
    async fn retain_known_stop(&mut self) -> EnforcerResult<serde_json::Value> {
        let value = self.request(false)?;
        let arm = value.registration.arm.operation_id.clone();
        let mut connection = self.connection().await?;
        let reply = request(
            &mut connection,
            StopRequest::RestoreRequiredStop {
                value: Box::new(value),
            },
        )
        .await;
        if !matches!(reply, Err(EnforcerError::NotReady)) {
            return Err(EnforcerError::Conflict);
        }
        let actor = defer_failed_stop_actor(&self.state, &arm)?;
        serde_json::to_value(actor.snapshot()?).map_err(|_| EnforcerError::InvalidInput)
    }
    fn known_snapshot(&self) -> EnforcerResult<serde_json::Value> {
        let slots = self
            .state
            .slots
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?;
        if slots.len() != 1 {
            return Err(EnforcerError::Conflict);
        }
        serde_json::to_value(
            slots
                .values()
                .next()
                .ok_or(EnforcerError::Conflict)?
                .snapshot()?,
        )
        .map_err(|_| EnforcerError::InvalidInput)
    }
    fn replace_owned_anchor(&mut self, socket: bool) -> EnforcerResult<()> {
        let lease = self
            .state
            .lease
            .as_ref()
            .ok_or(EnforcerError::UnsafeFilesystem)?;
        lease.revalidate()?;
        if socket {
            let path = self
                .root
                .path()
                .join("endpoints")
                .join(self.listener().endpoint().directory_name())
                .join("s");
            let before =
                std::fs::symlink_metadata(&path).map_err(|_| EnforcerError::UnsafeFilesystem)?;
            std::fs::remove_file(&path).map_err(|_| EnforcerError::UnsafeFilesystem)?;
            let replacement = std::os::unix::net::UnixListener::bind(&path)
                .map_err(|_| EnforcerError::Unavailable)?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .map_err(|_| EnforcerError::UnsafeFilesystem)?;
            let after =
                std::fs::symlink_metadata(&path).map_err(|_| EnforcerError::UnsafeFilesystem)?;
            if before.ino() == after.ino()
                || before.uid() != after.uid()
                || after.mode() & 0o777 != 0o600
            {
                return Err(EnforcerError::Conflict);
            }
            self.replacement_sockets.push(replacement);
            if recovery_root_binding(&self.root, self.listener().endpoint()).is_ok() {
                return Err(EnforcerError::Conflict);
            }
        } else {
            let replacement = PrivateListener::bind_new(&lease.listener_root()?)?;
            lease.publish_endpoint(replacement.endpoint())?;
            let current = load_endpoint(&self.root)?.ok_or(EnforcerError::Unavailable)?;
            if current.epoch() != replacement.endpoint().epoch()
                || current.epoch() == self.state.epoch
            {
                return Err(EnforcerError::Conflict);
            }
            self.replacements.push(replacement);
        }
        // The mutation is confined to the endpoint/socket; root/lock stay valid.
        lease.revalidate()?;
        Ok(())
    }
    pub(super) async fn finish_connection(mut self) -> EnforcerResult<()> {
        let result =
            match tokio::time::timeout(HANDSHAKE_WINDOW + IO_TIMEOUT, self.connections.join_next())
                .await
            {
                Ok(Some(Ok(result))) => result,
                _ => Err(EnforcerError::Unavailable),
            };
        if !self.finish().await {
            return Err(EnforcerError::Unavailable);
        }
        result
    }
    async fn finish(mut self) -> bool {
        self.connections.shutdown().await;
        if let Some(task) = self.progress.take() {
            task.abort();
            let _ = task.await;
        }
        drop(self.listener.take());
        self.replacements.clear();
        self.replacement_sockets.clear();
        drop(std::mem::replace(&mut self.state, fixture_state()));
        drop(self.temp.take());
        true
    }
}

async fn negotiate_only(
    stream: &mut UnixStream,
    challenge: &ChallengeV2,
    key: &[u8; 32],
) -> EnforcerResult<(Negotiate3, Challenge3)> {
    let mut negotiate = Negotiate3 {
        kind: "negotiate_v3".into(),
        protocol: 3,
        requested_role: RequestedRole::StopsOnly,
        client_nonce: fresh_nonce(&[challenge.nonce]),
        proof: empty_proof(),
    };
    negotiate.proof = negotiate_proof(key, challenge, &negotiate)?;
    write_frame(stream, &negotiate, HANDSHAKE_FRAME, IO_TIMEOUT).await?;
    let Handshake3::Challenge(offered) = read_frame(stream, HANDSHAKE_FRAME, IO_TIMEOUT).await?
    else {
        return Err(EnforcerError::Unauthorized);
    };
    if offered.epoch.0 != challenge.epoch
        || offered.client_nonce != negotiate.client_nonce
        || offered.root_binding.0 != challenge.root_binding
        || offered.session_id != session_id(challenge, &negotiate, &offered)?
    {
        return Err(EnforcerError::Unauthorized);
    }
    verify(
        key,
        CHALLENGE3,
        &(
            negotiate_core(challenge, &negotiate),
            offered.core(),
            &offered.session_id,
        ),
        &offered.proof,
    )?;
    Ok((negotiate, offered))
}

#[tokio::test]
async fn v3_revalidates_owned_endpoint_and_socket_at_each_connection_checkpoint() {
    let mut observations = Vec::new();
    for socket in [true, false] {
        for stage in 0..4 {
            let mut fixture = AnchorFixture::new();
            let observed = async {
                let known = fixture.retain_known_stop().await?;
                let key = fixture.state.capability;
                let rejected = match stage {
                    0 => {
                        let (mut stream, challenge) = fixture.preface().await?;
                        fixture.replace_owned_anchor(socket)?;
                        let response = negotiate_only(&mut stream, &challenge, &key).await;
                        drop(stream);
                        response.is_err()
                    }
                    1 => {
                        let (mut stream, challenge) = fixture.preface().await?;
                        let (negotiate, offered) =
                            negotiate_only(&mut stream, &challenge, &key).await?;
                        fixture.replace_owned_anchor(socket)?;
                        let mut auth = Auth3 {
                            kind: "auth_v3".into(),
                            protocol: 3,
                            session_id: offered.session_id.clone(),
                            requested_role: RequestedRole::StopsOnly,
                            epoch: offered.epoch,
                            root_binding: offered.root_binding.clone(),
                            client_nonce: offered.client_nonce,
                            server_nonce: offered.server_nonce,
                            proof: empty_proof(),
                        };
                        let computed_proof = proof(
                            &key,
                            AUTH3,
                            &(
                                negotiate_core(&challenge, &negotiate),
                                offered.core(),
                                &offered.session_id,
                                auth.core(),
                            ),
                        )?;
                        auth.proof = computed_proof;
                        write_frame(&mut stream, &auth, HANDSHAKE_FRAME, IO_TIMEOUT).await?;
                        let result =
                            read_frame::<Handshake3>(&mut stream, HANDSHAKE_FRAME, IO_TIMEOUT)
                                .await;
                        drop(stream);
                        !matches!(result, Ok(Handshake3::Grant(_)))
                    }
                    2 => {
                        let mut connection = fixture.connection().await?;
                        let query = Request3::new(
                            connection.v3.as_ref().ok_or(EnforcerError::Unauthorized)?,
                            StopRequest::StatusStops {},
                        )?;
                        query
                            .validate(connection.v3.as_ref().ok_or(EnforcerError::Unauthorized)?)?;
                        fixture.replace_owned_anchor(socket)?;
                        write_frame(&mut connection.stream, &query, MAX_FRAME, IO_TIMEOUT).await?;
                        let reply =
                            read_frame::<Reply3>(&mut connection.stream, MAX_FRAME, IO_TIMEOUT)
                                .await;
                        drop(connection);
                        reply.is_err()
                    }
                    _ => {
                        let mut connection = fixture.connection().await?;
                        let mut watch = start_watch(&mut connection).await?;
                        let before = watch.read(&mut connection.stream).await?;
                        if before.slot_count() != 1 {
                            return Err(EnforcerError::Conflict);
                        }
                        fixture.replace_owned_anchor(socket)?;
                        let boundary = fixture.state.progress.load(Ordering::SeqCst);
                        let mut post_mutation = 0;
                        let mut frames = 0;
                        let closed = loop {
                            frames += 1;
                            if frames > 8 {
                                return Err(EnforcerError::Unhealthy);
                            }
                            match watch.read(&mut connection.stream).await {
                                Err(_) => break true,
                                Ok(value) if value.progress() > boundary => {
                                    post_mutation += 1;
                                    // At most one snapshot could race the
                                    // replacement after its pre-write check.
                                    if post_mutation == 2 {
                                        break false;
                                    }
                                }
                                Ok(_) => {}
                            }
                        };
                        drop(connection);
                        closed
                    }
                };
                let retained = fixture.known_snapshot()? == known;
                Ok::<_, EnforcerError>((rejected, retained))
            }
            .await;
            let cleaned = fixture.finish().await;
            observations.push((socket, stage, cleaned, observed));
        }
    }
    for (socket, stage, cleaned, observed) in &observations {
        assert!(
            *cleaned,
            "fixture cleanup failed: socket={socket} stage={stage}"
        );
        assert!(
            observed.is_ok(),
            "positive/setup failed: socket={socket} stage={stage}"
        );
        assert!(
            observed.as_ref().unwrap().1,
            "known STOP target changed: socket={socket} stage={stage}"
        );
    }
    assert!(
        observations
            .iter()
            .all(|(_, _, _, observed)| observed.as_ref().is_ok_and(|(rejected, _)| *rejected)),
        "each substituted owned anchor must close its existing V3 connection before an agreement/dispatch/watch publication"
    );
}

fn null_members(value: &serde_json::Value, prefix: &str, result: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(fields) => {
            for (key, value) in fields {
                let escaped = key.replace('~', "~0").replace('/', "~1");
                let path = format!("{prefix}/{escaped}");
                if value.is_null() {
                    result.push(path);
                } else {
                    null_members(value, &path, result);
                }
            }
        }
        serde_json::Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                null_members(value, &format!("{prefix}/{index}"), result);
            }
        }
        _ => {}
    }
}
fn remove_null_member(value: &mut serde_json::Value, pointer: &str) -> bool {
    let Some((parent, field)) = pointer.rsplit_once('/') else {
        return false;
    };
    let field = field.replace("~1", "/").replace("~0", "~");
    value
        .pointer_mut(parent)
        .and_then(serde_json::Value::as_object_mut)
        .and_then(|object| object.remove(&field))
        .is_some_and(|value| value.is_null())
}
fn inject_absent_null_member(value: &mut serde_json::Value, pointer: &str) -> bool {
    let Some((parent, field)) = pointer.rsplit_once('/') else {
        return false;
    };
    let field = field.replace("~1", "/").replace("~0", "~");
    let Some(object) = value
        .pointer_mut(parent)
        .and_then(serde_json::Value::as_object_mut)
    else {
        return false;
    };
    if object.contains_key(&field) {
        return false;
    }
    object.insert(field, serde_json::Value::Null);
    true
}
async fn raw_reply(value: &serde_json::Value) -> EnforcerResult<Reply3> {
    let (mut sender, mut receiver) = UnixStream::pair().map_err(|_| EnforcerError::Unavailable)?;
    let (sent, received) = tokio::join!(
        write_frame(&mut sender, value, MAX_FRAME, IO_TIMEOUT),
        read_frame(&mut receiver, MAX_FRAME, IO_TIMEOUT)
    );
    sent?;
    received.map_err(EnforcerError::from)
}

#[tokio::test]
async fn embedded_v1_canonical_shape_is_required_in_signed_restore_and_slot_replies() {
    let mut fixture = AnchorFixture::new();
    let observed = async {
        let payload = fixture.request(false)?;
        let mut connection = fixture.connection().await?;
        let original = Request3::new(
            connection.v3.as_ref().ok_or(EnforcerError::Unauthorized)?,
            StopRequest::RestoreRequiredStop {
                value: Box::new(payload.clone()),
            },
        )?;
        original.validate(connection.v3.as_ref().ok_or(EnforcerError::Unauthorized)?)?;
        let wire = serde_json::to_value(&original).map_err(|_| EnforcerError::InvalidInput)?;
        // Root's R4b contract retains the historical V1 serializer's shape.
        // The positive is the actual typed producer, with no injected nulls.
        let canonical_positive: Request3 = serde_json::from_slice(
            &serde_json::to_vec(&original).map_err(|_| EnforcerError::InvalidInput)?,
        )
        .map_err(|_| EnforcerError::InvalidInput)?;
        canonical_positive.validate(connection.v3.as_ref().ok_or(EnforcerError::Unauthorized)?)?;
        write_frame(&mut connection.stream, &original, MAX_FRAME, IO_TIMEOUT).await?;
        let restore_reply: Reply3 =
            read_frame(&mut connection.stream, MAX_FRAME, IO_TIMEOUT).await?;
        restore_reply.validate(
            connection.v3.as_ref().ok_or(EnforcerError::Unauthorized)?,
            &original,
        )?;
        if !matches!(
            restore_reply.result,
            ReplyValue3::Error {
                code: EnforcerError::NotReady
            }
        ) {
            return Err(EnforcerError::Conflict);
        }
        let arm_id = &payload.registration.arm.operation_id;
        defer_failed_stop_actor(&fixture.state, arm_id)?;
        let known = fixture.known_snapshot()?;

        // Query the exact retained STOP for the signed slot reply. The failed
        // restore cannot acknowledge an installed native owner.
        let mut connection = fixture.connection().await?;
        let session = connection.v3.as_ref().ok_or(EnforcerError::Unauthorized)?;
        let query = Request3::new(
            session,
            StopRequest::QueryStop {
                arm_id: arm_id.clone(),
                registration_hash: payload.registration_hash.clone(),
            },
        )?;
        write_frame(&mut connection.stream, &query, MAX_FRAME, IO_TIMEOUT).await?;
        let reply: Reply3 = read_frame(&mut connection.stream, MAX_FRAME, IO_TIMEOUT).await?;
        reply.validate(session, &query)?;
        let ReplyValue3::StopStatus { value } = &reply.result else {
            return Err(EnforcerError::Conflict);
        };
        if serde_json::to_value(value).map_err(|_| EnforcerError::InvalidInput)? != known {
            return Err(EnforcerError::Conflict);
        }
        let mut request_paths = Vec::new();
        null_members(&wire, "", &mut request_paths);
        let mut request_rejections = Vec::new();
        for path in &request_paths {
            let mut connection = fixture.connection().await?;
            let request = Request3::new(
                connection.v3.as_ref().ok_or(EnforcerError::Unauthorized)?,
                StopRequest::RestoreRequiredStop {
                    value: Box::new(payload.clone()),
                },
            )?;
            let mut changed =
                serde_json::to_value(&request).map_err(|_| EnforcerError::InvalidInput)?;
            if !remove_null_member(&mut changed, path) {
                return Err(EnforcerError::Conflict);
            }
            // Deliberately preserve the producer's proof bytes.
            write_frame(&mut connection.stream, &changed, MAX_FRAME, IO_TIMEOUT).await?;
            let received =
                read_frame::<Reply3>(&mut connection.stream, MAX_FRAME, IO_TIMEOUT).await;
            let accepted = received.is_ok_and(|reply| {
                reply
                    .validate(connection.v3.as_ref().unwrap(), &request)
                    .is_ok()
            });
            request_rejections.push((path.clone(), !accepted));
        }
        let mut injection_rejections = Vec::new();
        for path in [
            "/command/value/registration/authority/docker_launch_profile",
            "/command/value/registration/authority/invocation/executable_directory",
        ] {
            if wire.pointer(path).is_some() {
                return Err(EnforcerError::Conflict);
            }
            let mut connection = fixture.connection().await?;
            let request = Request3::new(
                connection.v3.as_ref().ok_or(EnforcerError::Unauthorized)?,
                StopRequest::RestoreRequiredStop {
                    value: Box::new(payload.clone()),
                },
            )?;
            let mut changed =
                serde_json::to_value(&request).map_err(|_| EnforcerError::InvalidInput)?;
            if !inject_absent_null_member(&mut changed, path) {
                return Err(EnforcerError::Conflict);
            }
            // Deliberately preserve the producer's proof bytes after injection.
            write_frame(&mut connection.stream, &changed, MAX_FRAME, IO_TIMEOUT).await?;
            let received =
                read_frame::<Reply3>(&mut connection.stream, MAX_FRAME, IO_TIMEOUT).await;
            let accepted = received.is_ok_and(|reply| {
                reply
                    .validate(connection.v3.as_ref().unwrap(), &request)
                    .is_ok()
            });
            injection_rejections.push((path.to_owned(), !accepted));
        }
        let reply_wire = serde_json::to_value(&reply).map_err(|_| EnforcerError::InvalidInput)?;
        raw_reply(&reply_wire).await?.validate(session, &query)?;
        let mut reply_paths = Vec::new();
        null_members(&reply_wire, "", &mut reply_paths);
        let mut reply_rejections = Vec::new();
        for path in &reply_paths {
            let mut changed = reply_wire.clone();
            if !remove_null_member(&mut changed, path) {
                return Err(EnforcerError::Conflict);
            }
            let accepted = raw_reply(&changed)
                .await
                .is_ok_and(|reply| reply.validate(session, &query).is_ok());
            reply_rejections.push((path.clone(), !accepted));
        }
        Ok::<_, EnforcerError>((
            request_rejections,
            reply_rejections,
            injection_rejections,
            fixture.known_snapshot()? == known,
        ))
    }
    .await;
    let cleaned = fixture.finish().await;
    assert!(cleaned);
    let (requests, replies, injections, retained) =
        observed.expect("historical producer's signed positive must succeed before mutations");
    assert!(retained, "null-member faults cannot remove the known STOP");
    assert!(
        requests
            .iter()
            .any(|(path, _)| path == "/command/value/durable_cancel")
    );
    assert!(
        replies
            .iter()
            .any(|(path, _)| path.ends_with("/cancel_command_id"))
    );
    assert!(
        replies
            .iter()
            .any(|(path, _)| path.ends_with("/durable_cancel"))
    );
    assert_eq!(injections.len(), 2);
    assert!(
        requests
            .iter()
            .chain(&replies)
            .chain(&injections)
            .all(|(_, rejected)| *rejected),
        "removing a serialized null or injecting an omitted member without a new proof must reject wire3: requests={requests:?}, replies={replies:?}, injections={injections:?}"
    );
}

fn isolated_client(root: &Directory, protocol: StopProtocol) -> EnforcerStopClient {
    // Exercise the actual open() used by both constructors without spawning a
    // monitor whose reconnects would add unrelated accepted connections.
    EnforcerStopClient(Arc::new(StopClient {
        root: root.clone(),
        executable: std::env::current_exe().unwrap(),
        max_slots: 8,
        max_recovery_stop_owners: 0,
        capability: [0x5a; 32],
        protocol,
        affinity: StopAffinity::Legacy,
        cache: Mutex::new(StopCache::default()),
        changed: Arc::new(tokio::sync::Notify::new()),
        watch_failed: AtomicBool::new(false),
        attempts: Arc::new(tokio::sync::Semaphore::new(MAX_PENDING_ATTEMPTS)),
        observed_watch_epoch: Mutex::new(None),
        open_barrier: Mutex::new(None),
        observed_epochs: Mutex::new(Vec::new()),
    }))
}
async fn preface_case(
    fixture: &AnchorFixture,
    protocol: StopProtocol,
    mutation: usize,
) -> EnforcerResult<(bool, bool, bool)> {
    let client = isolated_client(&fixture.root, protocol);
    let state = Arc::clone(&fixture.state);
    let peer = async {
        let mut stream = fixture.listener().accept_checked().await?;
        let challenge = challenge_for(&state);
        let mut wire = serde_json::to_value(&challenge).map_err(|_| EnforcerError::InvalidInput)?;
        if mutation != 0 {
            let (field, value, compact) = match mutation {
                1 => ("nonce", challenge.nonce, false),
                2 => ("nonce", challenge.nonce, true),
                3 => ("epoch", challenge.epoch, false),
                _ => ("epoch", challenge.epoch, true),
            };
            let canonical = value.to_string();
            let mut changed = if compact {
                value.simple().to_string()
            } else {
                canonical.to_uppercase()
            };
            if changed == canonical {
                changed = value.simple().to_string();
            }
            if changed == canonical {
                return Err(EnforcerError::Conflict);
            }
            wire[field] = serde_json::Value::String(changed);
        }
        let deadline = Instant::now() + HANDSHAKE_WINDOW;
        write_frame(&mut stream, &wire, HANDSHAKE_FRAME, IO_TIMEOUT).await?;
        match read_frame::<FirstAuthentication>(&mut stream, HANDSHAKE_FRAME, IO_TIMEOUT).await {
            Err(_) => Ok((false, false)),
            Ok(FirstAuthentication::V3(negotiate)) => {
                let bytes = serde_json::to_vec(&negotiate_core(&challenge, &negotiate))
                    .map_err(|_| EnforcerError::InvalidInput)?;
                let valid = verify_bytes(&state.capability, N3, &bytes, &negotiate.proof).is_ok();
                if valid {
                    // Finish the genuine authenticated flow on the same nonce
                    // values. The only changed bytes were the preface spelling.
                    let _ = serve(stream, Arc::clone(&state), challenge, negotiate, deadline).await;
                }
                Ok((true, valid))
            }
            Ok(FirstAuthentication::V2(authentication)) => {
                verify_auth(
                    &state.capability,
                    &challenge,
                    authentication.requested_role,
                    false,
                    &authentication.proof,
                )?;
                let reply = AuthenticationReplyV2::Granted {
                    granted_role: RequestedRole::StopsOnly,
                    epoch: challenge.epoch,
                    nonce: challenge.nonce,
                    root_binding: challenge.root_binding.clone(),
                    ledger_state: challenge.ledger_state.clone(),
                    proof: auth_proof(
                        &state.capability,
                        &challenge,
                        RequestedRole::StopsOnly,
                        true,
                    )?,
                };
                write_frame(&mut stream, &reply, HANDSHAKE_FRAME, IO_TIMEOUT).await?;
                Ok((true, true))
            }
        }
    };
    let opening = async {
        let result = client.open().await;
        let opened = result.is_ok();
        drop(result);
        opened
    };
    let (opened, peer) = tokio::join!(opening, peer);
    let (authentication_seen, mac_valid) = peer?;
    Ok((opened, authentication_seen, mac_valid))
}

#[tokio::test]
async fn client3_rejects_noncanonical_preface_uuids_before_any_authentication() {
    let fixture = AnchorFixture::new();
    let mut observations = Vec::new();
    for mutation in 0..5 {
        let old_client = preface_case(&fixture, StopProtocol::V2, mutation).await;
        let client3 = preface_case(&fixture, StopProtocol::V3, mutation).await;
        observations.push((mutation, old_client, client3));
    }
    let cleaned = fixture.finish().await;
    assert!(cleaned);
    for (mutation, old, new) in &observations {
        assert!(
            old.as_ref().is_ok_and(|value| *value == (true, true, true)),
            "legacy2 control failed for spelling {mutation}"
        );
        assert!(
            new.is_ok(),
            "authenticated V3 pair setup failed for spelling {mutation}"
        );
    }
    assert_eq!(
        *observations[0].2.as_ref().unwrap(),
        (true, true, true),
        "canonical V3 positive must finish"
    );
    assert!(
        observations.iter().skip(1).all(|(_, _, new)| new
            .as_ref()
            .is_ok_and(|value| *value == (false, false, false))),
        "noncanonical epoch/nonce must close before Negotiate3, hence before Auth3"
    );
}

async fn relay_signed_capacity_fault(
    mut front: UnixStream,
    state: Arc<RecoveryState>,
) -> EnforcerResult<()> {
    use tokio::io::AsyncReadExt;
    let (server, mut backend) = UnixStream::pair().map_err(|_| EnforcerError::Unavailable)?;
    let serving = Arc::clone(&state);
    let mut task = tokio::spawn(serve_connection(server, serving));
    let relayed = async {
        let preface: ChallengeV2 = read_frame(&mut backend, HANDSHAKE_FRAME, IO_TIMEOUT).await?;
        write_frame(&mut front, &preface, HANDSHAKE_FRAME, IO_TIMEOUT).await?;
        let FirstAuthentication::V3(negotiate) =
            read_frame(&mut front, HANDSHAKE_FRAME, IO_TIMEOUT).await?
        else {
            return Err(EnforcerError::Unauthorized);
        };
        write_frame(&mut backend, &negotiate, HANDSHAKE_FRAME, IO_TIMEOUT).await?;
        let Handshake3::Challenge(offered) =
            read_frame(&mut backend, HANDSHAKE_FRAME, IO_TIMEOUT).await?
        else {
            return Err(EnforcerError::Unauthorized);
        };
        write_frame(&mut front, &offered, HANDSHAKE_FRAME, IO_TIMEOUT).await?;
        let Handshake3::Auth(auth) = read_frame(&mut front, HANDSHAKE_FRAME, IO_TIMEOUT).await?
        else {
            return Err(EnforcerError::Unauthorized);
        };
        write_frame(&mut backend, &auth, HANDSHAKE_FRAME, IO_TIMEOUT).await?;
        let Handshake3::Grant(grant) =
            read_frame(&mut backend, HANDSHAKE_FRAME, IO_TIMEOUT).await?
        else {
            return Err(EnforcerError::Unauthorized);
        };
        write_frame(&mut front, &grant, HANDSHAKE_FRAME, IO_TIMEOUT).await?;
        let request: Request3 = read_frame(&mut front, MAX_FRAME, IO_TIMEOUT).await?;
        if !matches!(&request.command, StopRequest::WatchStops {}) {
            return Err(EnforcerError::InvalidInput);
        }
        verify(
            &state.capability,
            COMMAND3,
            &(&offered.session_id, request.core()),
            &request.proof,
        )?;
        write_frame(&mut backend, &request, MAX_FRAME, IO_TIMEOUT).await?;
        let first: Watch3 = read_frame(&mut backend, MAX_FRAME, IO_TIMEOUT).await?;
        verify(
            &state.capability,
            WATCH3,
            &(&offered.session_id, request.core(), first.core()),
            &first.proof,
        )?;
        if first.sequence != 0 || first.value.max_slots != 8 || first.value.known_stop_slots != 1 {
            return Err(EnforcerError::Conflict);
        }
        write_frame(&mut front, &first, MAX_FRAME, IO_TIMEOUT).await?;
        let mut second: Watch3 = read_frame(&mut backend, MAX_FRAME, IO_TIMEOUT).await?;
        verify(
            &state.capability,
            WATCH3,
            &(&offered.session_id, request.core(), second.core()),
            &second.proof,
        )?;
        if second.sequence != 1
            || second.value.max_slots != 8
            || second.value.progress <= first.value.progress
        {
            return Err(EnforcerError::Conflict);
        }
        // This peer owns the synthetic capability. Re-signing is intentional:
        // the regression is expected-cap correlation, not a forged MAC test.
        second.value.max_slots = 16;
        second.value.known_stop_slots = 9;
        let computed_proof = proof(
            &state.capability,
            WATCH3,
            &(&offered.session_id, request.core(), second.core()),
        )?;
        second.proof = computed_proof;
        verify(
            &state.capability,
            WATCH3,
            &(&offered.session_id, request.core(), second.core()),
            &second.proof,
        )?;
        write_frame(&mut front, &second, MAX_FRAME, IO_TIMEOUT).await?;
        let mut extra = [0_u8; 1];
        tokio::time::timeout(IO_TIMEOUT, front.read(&mut extra))
            .await
            .map_err(|_| EnforcerError::Unavailable)?
            .map_err(|_| EnforcerError::Unavailable)?;
        Ok::<_, EnforcerError>(())
    }
    .await;
    drop(front);
    drop(backend);
    let joined = match tokio::time::timeout(IO_TIMEOUT, &mut task).await {
        Ok(Ok(_)) => true,
        Ok(Err(_)) => false,
        Err(_) => {
            task.abort();
            let _ = task.await;
            false
        }
    };
    if !joined {
        return Err(EnforcerError::Unavailable);
    }
    relayed
}

#[tokio::test]
async fn watch3_rejects_signed_wrong_capacity_before_healthy_publication_and_keeps_stops() {
    let mut fixture = AnchorFixture::new();
    let known = fixture.retain_known_stop().await;
    let core = isolated_client(&fixture.root, StopProtocol::V3);
    let opening = async {
        let mut connection = core.ensure_open().await?;
        let mut reader = start_watch(&mut connection).await?;
        let first = reader.read(&mut connection.stream).await?;
        let mut publication = WatchProgress::default();
        publication.accept(connection.epoch, &first, Instant::now())?;
        if first.max_slots != 8 || first.slot_count() != 1 {
            return Err(EnforcerError::Conflict);
        }
        let second = reader.read(&mut connection.stream).await;
        let rejected = second.is_err();
        let current_epoch = core
            .0
            .cache
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .epoch;
        let healthy_publication = current_epoch == Some(connection.epoch)
            && second.as_ref().is_ok_and(|value| {
                publication
                    .accept(connection.epoch, value, Instant::now())
                    .is_ok()
            });
        drop(connection);
        Ok::<_, EnforcerError>((rejected, healthy_publication))
    };
    let peer = async {
        let accepted = fixture.listener().accept_checked().await?;
        relay_signed_capacity_fault(accepted, Arc::clone(&fixture.state)).await
    };
    let (observed, peer) = tokio::join!(opening, peer);
    let retained = known
        .as_ref()
        .is_ok_and(|before| fixture.known_snapshot().is_ok_and(|after| &after == before));
    let cleaned = fixture.finish().await;
    assert!(cleaned);
    assert!(
        known.is_ok() && peer.is_ok(),
        "causal signed peer and known STOP setup must succeed"
    );
    assert!(
        retained,
        "wrong-capacity Watch must not remove the known STOP"
    );
    let (rejected, healthy_publication) =
        observed.expect("authenticated cap8 Watch positive must succeed first");
    assert!(
        rejected && !healthy_publication,
        "a signed cap16/known9 frame cannot be published healthy by a cap8 client"
    );
}

#[path = "recovery_v3_hardening_tests.rs"]
mod hardening;
