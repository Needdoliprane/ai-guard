//! Protocol regressions using the real framed transport and dispatcher.
//! No runtime, provider, or ledger producer is exercised by these fixtures.
use super::*;
use crate::broker::clock::checked_deadline;
use crate::control::{
    ArmSubjectV1, NetworkV1, RestartPolicyV1, RuntimeInspectionV1, SecurityPresetV1,
};
use serde_json::json;
use std::os::unix::fs::PermissionsExt;

fn defer_failed_stop_actor(state: &RecoveryState, arm_id: &str) -> EnforcerResult<Arc<StopActor>> {
    let actor = state
        .slots
        .lock()
        .map_err(|_| EnforcerError::Unhealthy)?
        .get(arm_id)
        .cloned()
        .ok_or(EnforcerError::Conflict)?;
    let slot = actor.snapshot()?;
    if actor.worker_owned.load(Ordering::SeqCst)
        || actor.fail_spawn_once.load(Ordering::SeqCst)
        || slot.phase != StopPhaseV1::Quarantined
        || slot.stop_failures != 1
    {
        return Err(EnforcerError::Conflict);
    }
    // Replays in these IPC fixtures must not start an unowned native thread.
    *actor
        .next_start
        .lock()
        .map_err(|_| EnforcerError::Unhealthy)? = Instant::now() + Duration::from_secs(3600);
    Ok(actor)
}

fn unavailable_registration(root: &Path, suffix: &str) -> RegistrationV1 {
    // Explicitly unavailable fixture metadata; no host program may execute.
    // execute any host program. These are explicit test data, not engine facts.
    let missing = root.join("unavailable-runtime");
    let authority: RuntimeAuthority = serde_json::from_value(json!({
        "authority_id": Uuid::new_v4(),
        "kind": "docker",
        "endpoint": format!("unix://{}", root.join("unavailable.sock").display()),
        "engine_id": "synthetic-unavailable-engine",
        "podman": null,
        "invocation": {
            "policy_version": 2,
            "kind": "docker",
            "approved_entry": missing,
            "canonical_program": missing,
            "file_identity": {
                "device": 1, "inode": 2, "owner": 0, "group": 0,
                "mode": 493, "length": 1, "sha256": "b".repeat(64)
            },
            "environment": {}, "cwd": root
        }
    }))
    .unwrap();
    let inspection = RuntimeInspectionV1 {
        authority_id: authority.authority_id.to_string(),
        runtime_kind: RuntimeKindV1::Docker,
        container_id: hex::encode(Sha256::digest(suffix.as_bytes())),
        stable_name: format!("aig-cancel-{suffix}"),
        volume_name: format!("aig-cancel-{suffix}-work"),
        image_id: format!("sha256:{}", "b".repeat(64)),
        spec_hash: "d".repeat(64),
        cpu_millis: 1000,
        memory_bytes: 1024 * 1024 * 1024,
        pids_limit: 256,
        network: NetworkV1::None,
        security_preset: SecurityPresetV1::FixedSandboxV2,
        restart_policy: RestartPolicyV1::No,
    };
    let owned: OwnedContainerSnapshot = serde_json::from_value(json!({
        "handle": {
            "authority_fingerprint": authority.fingerprint(),
            "session": Uuid::new_v4(), "operation": Uuid::new_v4(),
            "id": inspection.container_id, "name": inspection.stable_name,
            "configuration_fingerprint_version": 2
        },
        "observation": {
            "id": inspection.container_id, "name": inspection.stable_name,
            "image_id": inspection.image_id, "restart_count": 0,
            "process": {
                "status": "running", "running": true,
                "started_at": "2026-10-31T00:00:00Z", "finished_at": null,
                "exit_code": null, "oom_killed": false
            },
            "allocation": {
                "cpu": {"quota": 100000, "period": 100000},
                "memory_bytes": 1024_u64 * 1024 * 1024, "pids_limit": 256
            },
            "configuration_fingerprint": "c".repeat(64),
            "configuration_fingerprint_version": 2,
            "observed_at": "2026-10-31T00:00:00Z", "diagnostics": []
        }
    }))
    .unwrap();
    let clock = NativeHostClock::new().unwrap().sample().unwrap();
    let arm = ArmV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: Uuid::new_v4().to_string(),
        attempt_id: Uuid::new_v4().to_string(),
        expected_revision: 1,
        intent_hash: "e".repeat(64),
        subject: ArmSubjectV1::Worker,
        inspection,
        authority_binding_hash: authority.fingerprint(),
        configuration_fingerprint_version: 2,
        configuration_fingerprint: "c".repeat(64),
        deadline_ns_decimal: checked_deadline(clock.continuous_ns().unwrap(), 3600)
            .unwrap()
            .to_string(),
        clock,
        timeout_seconds: 3600,
    };
    let receipt = ArmReceiptV1 {
        version: 1,
        arm_id: arm.operation_id.clone(),
        worker_id: arm.worker_id.clone(),
        attempt_id: arm.attempt_id.clone(),
        input_hash: arm.input_hash().unwrap(),
        control_namespace_uuid: Uuid::new_v4().to_string(),
        audit_seq: 1,
        audit_hash: "f".repeat(64),
    };
    RegistrationV1::new(
        "a".repeat(64),
        "tenant-retained-cancel".into(),
        arm,
        receipt,
        authority,
        owned,
    )
    .unwrap()
}

fn fixture_state() -> Arc<RecoveryState> {
    Arc::new(RecoveryState::new(
        Uuid::new_v4(),
        8,
        [0x5a; 32],
        "a".repeat(64),
        None,
        None,
    ))
}

async fn open_pair() -> (StopConnection, tokio::task::JoinHandle<EnforcerResult<()>>) {
    let mut fixture = review_regressions::AnchorFixture::new();
    let connection = fixture.connection().await.unwrap();
    let task = tokio::spawn(fixture.finish_connection());
    (connection, task)
}

#[test]
fn first_authentication_rejects_duplicate_mixed_and_noncanonical_records() {
    let proof = "a".repeat(64);
    let nonce = Uuid::from_u128(0xaaaaaaaa_aaaa_4aaa_8aaa_aaaaaaaaaaaa);
    let legacy = format!(r#"{{"requested_role":"stops_only","proof":"{proof}"}}"#);
    assert!(matches!(
        serde_json::from_str::<FirstAuthentication>(&legacy).unwrap(),
        FirstAuthentication::V2(_)
    ));
    let negotiate = format!(
        r#"{{"kind":"negotiate_v3","protocol":3,"requested_role":"stops_only","client_nonce":"{nonce}","proof":"{proof}"}}"#
    );
    assert!(matches!(
        serde_json::from_str::<FirstAuthentication>(&negotiate).unwrap(),
        FirstAuthentication::V3(_)
    ));
    for invalid in [
        legacy.replace("\"proof\":", "\"proof\":\"b\",\"proof\":"),
        legacy.replace("{", "{\"protocol\":3,"),
        negotiate.replace("\"protocol\":3", "\"protocol\":3,\"protocol\":3"),
        negotiate.replace(&nonce.to_string(), &nonce.to_string().to_uppercase()),
        format!("{negotiate} {{}}"),
    ] {
        assert!(serde_json::from_str::<FirstAuthentication>(&invalid).is_err());
    }
}

#[tokio::test]
async fn stop3_status_is_authenticated_without_inventing_ledger_coverage() {
    let (mut connection, task) = open_pair().await;
    let received = request(&mut connection, StopRequest::StatusStops {}).await;
    drop(connection);
    let joined = task.await.unwrap();
    assert!(joined.is_ok());
    let ReplyValue3::StatusStops { value } = received.unwrap() else {
        panic!("wrong response")
    };
    assert_eq!(value.known_stop_slot_count(), 0);
    assert_eq!(value.retirement_count(), None);
    assert_eq!(value.ledger_state_reason(), "recovery_transport_only");
    assert!(!value.admission_available());
    let wire = serde_json::to_value(&value).unwrap();
    assert_eq!(
        wire["retirement_count"],
        json!({"state":"unknown","reason":"ledger_unverified"})
    );
    assert_eq!(
        wire["ledger_state"],
        json!({"state":"unverified","reason":"recovery_transport_only"})
    );
}

#[tokio::test]
async fn stop3_watch_binds_sequence_session_request_and_epoch() {
    let (mut connection, task) = open_pair().await;
    let mut watch = start_watch(&mut connection).await.unwrap();
    let first = watch.read(&mut connection.stream).await;
    let second = watch.read(&mut connection.stream).await;
    drop(connection);
    let _ = task.await.unwrap();
    assert!(first.is_ok());
    assert!(second.is_ok());
    assert_eq!(watch.next_sequence, 2);
}

#[tokio::test]
async fn full3_is_refused_on_its_own_connection_without_a_stop_grant() {
    let mut fixture = review_regressions::AnchorFixture::new();
    let (mut stream, challenge) = fixture.preface().await.unwrap();
    let result = client_handshake(
        &mut stream,
        &challenge,
        &[0x5a; 32],
        Instant::now() + HANDSHAKE_WINDOW,
        RequestedRole::Full,
        8,
    )
    .await;
    drop(stream);
    assert!(fixture.finish_connection().await.is_ok());
    assert!(matches!(result, Err(EnforcerError::Unavailable)));
}

#[tokio::test]
async fn expired_global_handshake_budget_never_writes_an_authentication() {
    let (mut stream, mut peer) = UnixStream::pair().unwrap();
    let challenge = challenge_for(&fixture_state());
    let result = client_handshake(
        &mut stream,
        &challenge,
        &[0x5a; 32],
        Instant::now() - HANDSHAKE_WINDOW,
        RequestedRole::StopsOnly,
        8,
    )
    .await;
    drop(stream);
    let frame = read_frame::<FirstAuthentication>(&mut peer, HANDSHAKE_FRAME, IO_TIMEOUT).await;
    assert!(matches!(result, Err(EnforcerError::Unavailable)));
    assert!(frame.is_err());
}

struct PublicFixture {
    temp: Option<tempfile::TempDir>,
    root: Directory,
    state: Arc<RecoveryState>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl Drop for PublicFixture {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        if let Some(temp) = self.temp.take() {
            let _preserved = temp.keep();
        }
    }
}
impl PublicFixture {
    fn new() -> Self {
        let configured = std::env::var_os("AI_GUARD_TEST_D5_IPC_ROOT").map(PathBuf::from);
        let fallback = if cfg!(target_os = "macos") {
            "/private/tmp"
        } else {
            "/tmp"
        };
        let requested = configured.as_deref().unwrap_or_else(|| Path::new(fallback));
        assert!(
            requested.is_absolute(),
            "AI_GUARD_TEST_D5_IPC_ROOT must name an absolute directory"
        );
        let base = std::fs::canonicalize(requested)
            .expect("AI_GUARD_TEST_D5_IPC_ROOT or the native fallback must exist");
        assert!(base.is_dir(), "the D5 IPC fixture root must be a directory");
        if configured.is_some() {
            assert_eq!(
                requested,
                base.as_path(),
                "AI_GUARD_TEST_D5_IPC_ROOT must already be canonical"
            );
        }
        let temp = tempfile::Builder::new()
            .prefix("aig-v3-")
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
        // The one new reservation fails before OS-thread creation. This fixture
        // proves durable-STOP protocol handling, never a Ready actor or engine.
        state.faults.stage = "spawn_once".into();
        state.faults.arm_id = None;
        lease.publish_endpoint(listener.endpoint()).unwrap();
        let state = Arc::new(state);
        let serving = Arc::clone(&state);
        let (stop, mut stopped) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let mut children = tokio::task::JoinSet::new();
            let mut tick = tokio::time::interval(Duration::from_millis(25));
            loop {
                tokio::select! {
                    _ = &mut stopped => { children.shutdown().await; break; },
                    _ = tick.tick() => { serving.progress.fetch_add(1, Ordering::SeqCst); },
                    Some(_) = children.join_next(), if !children.is_empty() => {},
                    accepted = listener.accept_checked() => match accepted {
                        Ok(stream) => { let state = Arc::clone(&serving); children.spawn(serve_connection(stream, state)); },
                        Err(_) => break,
                    },
                }
            }
        });
        Self {
            temp: Some(temp),
            root,
            state,
            stop: Some(stop),
            task: Some(task),
        }
    }
    async fn finish(mut self) -> bool {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let Some(mut task) = self.task.take() else {
            return false;
        };
        match tokio::time::timeout(Duration::from_secs(3), &mut task).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => return false,
            Err(_) => {
                task.abort();
                let _ = task.await;
                return false;
            }
        }
        // No actor thread was created: the owned spawn_once fault fires before
        // spawn. Release all retained fixture leases before its directory.
        drop(std::mem::replace(&mut self.state, fixture_state()));
        drop(self.temp.take());
        true
    }
}

#[tokio::test]
async fn real_client2_and_client3_share_stop_dispatch_without_changing_views2() {
    let fixture = PublicFixture::new();
    let executable = std::env::current_exe().unwrap();
    let client2 =
        EnforcerStopClient::connect_or_spawn_for_recovery(&fixture.root, &executable, 8).await;
    let client3 =
        EnforcerStopClientV3::connect_or_spawn_for_recovery(&fixture.root, &executable, 8).await;
    let observed = async {
        let client2 = client2?;
        let client3 = client3?;
        let registration = unavailable_registration(fixture.root.path(), "wire3");
        let hash = registration.digest()?;
        let arm = registration.arm.operation_id.clone();
        let head =
            RevisionRefV1::new(1, &"b".repeat(64)).map_err(|_| EnforcerError::InvalidInput)?;
        let cancel = CancelReferenceV1 {
            command_id: Uuid::new_v4(),
            input_hash: "c".repeat(64),
            confirmed_head: head.clone(),
        };
        let value = RestoreRequiredStopV1 {
            version: 1,
            request_id: Uuid::new_v4(),
            registration,
            registration_hash: hash.clone(),
            source: StopSourceV1 {
                scope_hash: "d".repeat(64),
                job_id: Uuid::new_v4(),
                intent_hash: "e".repeat(64),
                confirmed_head: head,
                arm_operation_index: 0,
            },
            cancel_generation: 2,
            durable_cancel: Some(cancel.clone()),
            cancel_prefix: vec![cancel.clone()],
        };
        let reserved = client3
            .inner
            .request(StopRequest::RestoreRequiredStop {
                value: Box::new(value.clone()),
            })
            .await;
        if !matches!(reserved, Err(EnforcerError::NotReady)) {
            return Err(EnforcerError::Conflict);
        }
        let actor = defer_failed_stop_actor(&fixture.state, &arm)?;
        let replay = client2
            .request(StopRequest::RestoreRequiredStop {
                value: Box::new(value.clone()),
            })
            .await;
        if !matches!(replay, Err(EnforcerError::NotReady)) {
            return Err(EnforcerError::Conflict);
        }
        if actor.snapshot()?.stop_failures != 1 || actor.worker_owned.load(Ordering::SeqCst) {
            return Err(EnforcerError::Conflict);
        }
        let delivered = client3
            .inner
            .request(StopRequest::DeliverDurableStop {
                value: Box::new(value),
            })
            .await?;
        let queried2 = client2.query_stop(&arm, &hash).await?;
        let queried3 = client3.query_stop(&arm, &hash).await?;
        let status2 = client2.status().await?;
        let status3 = client3.status().await?;
        let deadline = Instant::now() + Duration::from_secs(3);
        while client2.observed_watch_epoch() != Some(fixture.state.epoch)
            || client3.inner.observed_watch_epoch() != Some(fixture.state.epoch)
        {
            if Instant::now() >= deadline {
                return Err(EnforcerError::Unavailable);
            }
            tokio::time::sleep(POLL).await;
        }
        let weak2 = Arc::downgrade(&client2.0);
        let weak3 = Arc::downgrade(&client3.inner.0);
        drop((client2, client3));
        let deadline = Instant::now() + Duration::from_secs(3);
        while weak2.strong_count() != 0 || weak3.strong_count() != 0 {
            if Instant::now() >= deadline {
                return Err(EnforcerError::Unavailable);
            }
            tokio::time::sleep(POLL).await;
        }
        Ok::<_, EnforcerError>((
            delivered, queried2, queried3, status2, status3, cancel, hash,
        ))
    }
    .await;
    let clean = fixture.finish().await;
    assert!(
        clean,
        "owned transport fixture must stop before its oracles"
    );
    let (delivered, queried2, queried3, status2, status3, cancel, hash) = observed.unwrap();
    assert!(matches!(delivered.1, StopResponse::StopAccepted { .. }));
    for slot in [queried2, queried3] {
        assert_eq!(slot.cancel_command_id(), Some(cancel.command_id));
        assert_eq!(slot.generation(), 2);
        assert_eq!(slot.registration_hash(), hash);
    }
    let wire2 = serde_json::to_value(status2).unwrap();
    assert_eq!(wire2.as_object().unwrap().len(), 8);
    assert!(wire2.get("retirement_count").is_none());
    assert!(wire2.get("root_binding").is_none());
    assert_eq!(status3.known_stop_slot_count(), 1);
    assert_eq!(status3.retirement_count(), None);
}

#[test]
fn command_mac_rejects_session_epoch_request_operation_and_direction_replays() {
    let session = Session3 {
        key: [0x5a; 32],
        id: Hash3("a".repeat(64)),
        epoch: Uuid::new_v4(),
        root_binding: Hash3("b".repeat(64)),
        max_slots: 8,
    };
    let original = Request3::new(&session, StopRequest::StatusStops {}).unwrap();
    original.validate(&session).unwrap();
    let bytes = serde_json::to_vec(&original).unwrap();
    for changed in 0..5 {
        let mut request: Request3 = serde_json::from_slice(&bytes).unwrap();
        match changed {
            0 => request.session_id = Hash3("c".repeat(64)),
            1 => request.epoch = Uuid3(Uuid::new_v4()),
            2 => request.request_id = Uuid3(Uuid::new_v4()),
            3 => request.command = StopRequest::WatchStops {},
            _ => {
                let computed_proof =
                    proof(&session.key, REPLY3, &(&session.id, request.core())).unwrap();
                request.proof = computed_proof;
            }
        }
        assert!(
            request.validate(&session).is_err(),
            "mutation {changed} must be authenticated"
        );
    }
    let mixed = String::from_utf8(bytes)
        .unwrap()
        .replace("\"protocol\":3", "\"protocol\":3,\"protocol\":3");
    assert!(serde_json::from_str::<Request3>(&mixed).is_err());
}

#[tokio::test]
async fn negotiate_proof_cannot_be_replayed_against_a_new_preface() {
    let state = fixture_state();
    let (server1, mut first) = UnixStream::pair().unwrap();
    let task1 = tokio::spawn(serve_connection(server1, Arc::clone(&state)));
    let challenge1: ChallengeV2 = read_frame(&mut first, HANDSHAKE_FRAME, IO_TIMEOUT)
        .await
        .unwrap();
    let mut negotiate = Negotiate3 {
        kind: "negotiate_v3".into(),
        protocol: 3,
        requested_role: RequestedRole::StopsOnly,
        client_nonce: fresh_nonce(&[challenge1.nonce]),
        proof: empty_proof(),
    };
    negotiate.proof = negotiate_proof(&[0x5a; 32], &challenge1, &negotiate).unwrap();
    drop(first);
    let first_join = task1.await.unwrap();
    let (server2, mut second) = UnixStream::pair().unwrap();
    let task2 = tokio::spawn(serve_connection(server2, state));
    let challenge2: ChallengeV2 = read_frame(&mut second, HANDSHAKE_FRAME, IO_TIMEOUT)
        .await
        .unwrap();
    let sent = write_frame(&mut second, &negotiate, HANDSHAKE_FRAME, IO_TIMEOUT).await;
    let response = read_frame::<Handshake3>(&mut second, HANDSHAKE_FRAME, IO_TIMEOUT).await;
    drop(second);
    let second_join = task2.await.unwrap();
    assert!(first_join.is_err());
    assert_ne!(challenge1.nonce, challenge2.nonce);
    assert!(sent.is_ok());
    assert!(response.is_err());
    assert!(matches!(second_join, Err(EnforcerError::Unauthorized)));
}

#[tokio::test]
async fn watch_rejects_an_authenticated_duplicate_sequence() {
    let session = Session3 {
        key: [0x5a; 32],
        id: Hash3("a".repeat(64)),
        epoch: Uuid::new_v4(),
        root_binding: Hash3("b".repeat(64)),
        max_slots: 8,
    };
    let request = Request3::new(&session, StopRequest::WatchStops {}).unwrap();
    let mut status =
        EnforcerStopStatusV3::from_legacy(fixture_state().status().unwrap(), &"b".repeat(64))
            .unwrap();
    status.epoch = Uuid3(session.epoch);
    let mut frame = Watch3 {
        kind: "watch_v3".into(),
        protocol: 3,
        session_id: session.id.clone(),
        epoch: Uuid3(session.epoch),
        request_id: request.request_id,
        sequence: 0,
        value: status,
        proof: empty_proof(),
    };
    let computed_proof = proof(
        &session.key,
        WATCH3,
        &(&session.id, request.core(), frame.core()),
    )
    .unwrap();
    frame.proof = computed_proof;
    let (mut writer, mut reader) = UnixStream::pair().unwrap();
    let task = tokio::spawn(async move {
        write_frame(&mut writer, &frame, MAX_FRAME, IO_TIMEOUT)
            .await
            .unwrap();
        write_frame(&mut writer, &frame, MAX_FRAME, IO_TIMEOUT)
            .await
            .unwrap();
    });
    let mut watch = Watch3Reader {
        session,
        request,
        next_sequence: 0,
    };
    let first = watch.read(&mut reader).await;
    let repeated = watch.read(&mut reader).await;
    drop(reader);
    task.await.unwrap();
    assert!(first.is_ok());
    assert!(matches!(repeated, Err(EnforcerError::Unauthorized)));
}

#[tokio::test]
async fn client_rejects_unsigned_challenge_and_full_refusal_before_auth3() {
    for full in [false, true] {
        let state = fixture_state();
        let challenge = challenge_for(&state);
        let epoch = challenge.epoch;
        let (mut server, mut client) = UnixStream::pair().unwrap();
        let peer = tokio::spawn(async move {
            write_frame(&mut server, &challenge, HANDSHAKE_FRAME, IO_TIMEOUT)
                .await
                .unwrap();
            let FirstAuthentication::V3(negotiate) =
                read_frame(&mut server, HANDSHAKE_FRAME, IO_TIMEOUT)
                    .await
                    .unwrap()
            else {
                panic!("test requires an explicit Negotiate3");
            };
            let nonce = fresh_nonce(&[challenge.nonce, negotiate.client_nonce.0]);
            if full {
                let refused = Refusal3 {
                    kind: "refused_v3".into(),
                    protocol: 3,
                    stage: RefusalStage3::Negotiate,
                    requested_role: RequestedRole::Full,
                    epoch: Uuid3(epoch),
                    root_binding: Hash3(challenge.root_binding.clone()),
                    client_nonce: negotiate.client_nonce,
                    server_nonce: nonce,
                    session_id: None,
                    code: RefusalCode3::FullUnavailable,
                    ledger_state: ledger3(),
                    proof: empty_proof(),
                };
                write_frame(&mut server, &refused, HANDSHAKE_FRAME, IO_TIMEOUT)
                    .await
                    .unwrap();
            } else {
                let mut offered = Challenge3 {
                    kind: "challenge_v3".into(),
                    protocol: 3,
                    role: "enforcer".into(),
                    epoch: Uuid3(epoch),
                    root_binding: Hash3(challenge.root_binding.clone()),
                    requested_role: RequestedRole::StopsOnly,
                    client_nonce: negotiate.client_nonce,
                    server_nonce: nonce,
                    offered_roles: vec![RequestedRole::StopsOnly],
                    ledger_state: ledger3(),
                    session_id: empty_proof(),
                    proof: empty_proof(),
                };
                offered.session_id = session_id(&challenge, &negotiate, &offered).unwrap();
                write_frame(&mut server, &offered, HANDSHAKE_FRAME, IO_TIMEOUT)
                    .await
                    .unwrap();
            }
            read_frame::<Handshake3>(&mut server, HANDSHAKE_FRAME, IO_TIMEOUT)
                .await
                .is_err()
        });
        let received: ChallengeV2 = read_frame(&mut client, HANDSHAKE_FRAME, IO_TIMEOUT)
            .await
            .unwrap();
        let result = client_handshake(
            &mut client,
            &received,
            &[0x5a; 32],
            Instant::now() + HANDSHAKE_WINDOW,
            if full {
                RequestedRole::Full
            } else {
                RequestedRole::StopsOnly
            },
            8,
        )
        .await;
        drop(client);
        let no_authentication = peer.await.unwrap();
        assert!(matches!(result, Err(EnforcerError::Unauthorized)));
        assert!(
            no_authentication,
            "a forged server message cannot obtain Auth3"
        );
    }
}

#[tokio::test]
async fn extended_server_keeps_the_4096_byte_first_frame_limit() {
    use tokio::io::AsyncWriteExt;
    let (server, mut client) = UnixStream::pair().unwrap();
    let task = tokio::spawn(serve_connection(server, fixture_state()));
    let preface = read_frame::<ChallengeV2>(&mut client, HANDSHAKE_FRAME, IO_TIMEOUT).await;
    let sent = client.write_all(&(HANDSHAKE_FRAME + 1).to_be_bytes()).await;
    let response = read_frame::<Handshake3>(&mut client, HANDSHAKE_FRAME, IO_TIMEOUT).await;
    drop(client);
    let joined = task.await.unwrap();
    assert!(preface.is_ok());
    assert!(sent.is_ok());
    assert!(response.is_err());
    assert!(matches!(joined, Err(EnforcerError::InvalidInput)));
}

#[tokio::test]
async fn server_does_not_issue_challenge3_after_its_global_deadline() {
    let state = fixture_state();
    let challenge = challenge_for(&state);
    let mut negotiate = Negotiate3 {
        kind: "negotiate_v3".into(),
        protocol: 3,
        requested_role: RequestedRole::StopsOnly,
        client_nonce: fresh_nonce(&[challenge.nonce]),
        proof: empty_proof(),
    };
    negotiate.proof = negotiate_proof(&state.capability, &challenge, &negotiate).unwrap();
    let (server, mut client) = UnixStream::pair().unwrap();
    let task = tokio::spawn(serve(
        server,
        state,
        challenge,
        negotiate,
        Instant::now() - HANDSHAKE_WINDOW,
    ));
    let response = read_frame::<Handshake3>(&mut client, HANDSHAKE_FRAME, IO_TIMEOUT).await;
    drop(client);
    let joined = task.await.unwrap();
    assert!(response.is_err());
    assert!(matches!(joined, Err(EnforcerError::Unavailable)));
}

#[tokio::test]
async fn a_granted_stop3_connection_cannot_reauthenticate_as_protocol2() {
    let (mut connection, task) = open_pair().await;
    let sent = write_frame(
        &mut connection.stream,
        &AuthenticationV2 {
            requested_role: RequestedRole::Full,
            proof: "a".repeat(64),
        },
        HANDSHAKE_FRAME,
        IO_TIMEOUT,
    )
    .await;
    let response = read_frame::<Reply3>(&mut connection.stream, MAX_FRAME, IO_TIMEOUT).await;
    drop(connection);
    let joined = task.await.unwrap();
    assert!(sent.is_ok());
    assert!(response.is_err());
    assert!(matches!(joined, Err(EnforcerError::InvalidInput)));
}

#[tokio::test]
async fn stop3_command_frames_remain_bounded_to_512_kib_before_body_allocation() {
    use tokio::io::AsyncWriteExt;
    let (mut connection, task) = open_pair().await;
    let sent = connection
        .stream
        .write_all(&(MAX_FRAME + 1).to_be_bytes())
        .await;
    let response = read_frame::<Reply3>(&mut connection.stream, MAX_FRAME, IO_TIMEOUT).await;
    drop(connection);
    let joined = task.await.unwrap();
    assert!(sent.is_ok());
    assert!(response.is_err());
    assert!(matches!(joined, Err(EnforcerError::InvalidInput)));
}

#[path = "recovery_v3_idle_stop_tests.rs"]
mod idle_stop_admission;
#[path = "recovery_v3_review_tests.rs"]
mod review_regressions;
