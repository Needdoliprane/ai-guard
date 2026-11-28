use super::*;
use crate::{
    broker::{
        enforcer::server::test_support::registration,
        intent::ExecutionIntentV1,
        journal::{
            BindingInputs, ControlSecrets, FsExclusions, Journal, JournalLimitsV1, ReserveResult,
            StopJournal,
        },
    },
    runner::CheckpointPayloadV1,
};
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;

#[tokio::test]
async fn legacy_terminal_read_confirms_only_the_existing_stop_latch() -> anyhow::Result<()> {
    let tmp = std::fs::canonicalize("/tmp")?;
    let files = tempfile::Builder::new()
        .prefix("aig-stop-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(tmp)?;
    let root = Directory::open_existing(files.path())?;
    let scope = JournalScopeV1::new(
        Uuid::new_v4(),
        "tenant-legacy-fixture",
        "http://127.0.0.1:8787",
        Uuid::new_v4(),
        "legacy-stop-read",
    )?;
    let mut registration = registration(root.path());
    let operation_id = registration.owned.handle.operation_id();
    let journal = Journal::open(
        &root,
        scope.clone(),
        &FsExclusions::new(),
        JournalLimitsV1::default(),
        ControlSecrets::new(Vec::new())?,
    )?;
    let intent = fixture_intent(operation_id)?;
    let ReserveResult::New(ticket) =
        journal.reserve(operation_id, br#"{"fixture":"stop-read"}"#)?
    else {
        anyhow::bail!("expected fresh fixture job");
    };
    journal.accept(
        ticket,
        &intent,
        BindingInputs::None,
        &CheckpointPayloadV1::initial(&intent)?,
    )?;
    let stops = StopJournal::from_full(&journal);
    let stop = Arc::new(stops.append_cancel(operation_id, Uuid::new_v4())?);
    let cancel = CancelReferenceV1::from_receipt(stop.receipt());
    registration.control_origin_hash =
        hex::encode(Sha256::digest(scope.gateway_origin().as_bytes()));
    registration.arm.intent_hash = intent.intent_hash()?;
    registration.receipt.input_hash = registration.arm.input_hash()?;
    registration.receipt.control_namespace_uuid = scope.control_namespace_uuid().to_string();
    let arm_id = registration.arm.operation_id.clone();
    let registration_hash = registration.digest()?;
    let scope_hash = hex::encode(Sha256::digest(canonical(&scope)?));
    let request = RestoreRequiredStopV1 {
        version: 1,
        request_id: Uuid::new_v4(),
        registration,
        registration_hash: registration_hash.clone(),
        source: StopSourceV1 {
            scope_hash: scope_hash.clone(),
            job_id: operation_id,
            intent_hash: intent.intent_hash()?,
            confirmed_head: stop.receipt().head().clone(),
            arm_operation_index: 3,
        },
        cancel_generation: 4,
        durable_cancel: Some(cancel.clone()),
        cancel_prefix: vec![cancel.clone()],
    };
    request.validate()?;

    let key = [0xa5; 32];
    root.persistent_create("auth", &key)?;
    root.persistent_create(".owner.lock", &[])?;
    let endpoints = root
        .persistent_child("endpoints", true)?
        .expect("fixture endpoints directory");
    let _announcements = root
        .persistent_child("announcements", true)?
        .expect("fixture announcements directory");
    let lease = EnforcerRoleLeaseV2::acquire_existing(&root)?;
    let listener = PrivateListener::bind_new(&endpoints)?;
    lease.publish_endpoint(listener.endpoint())?;
    let epoch = listener.endpoint().epoch();
    let state = Arc::new(RecoveryState::new(
        epoch,
        1,
        key,
        lease.root_binding(listener.endpoint())?,
        None,
        Some(lease.clone()),
    ));
    // Seed only the existing slot/ACK data around the real journal receipt.
    // Accept a single authenticated query at a time, with no maintenance
    // loop, native worker, runtime program or fallback to another daemon.
    let actor = Arc::new(StopActor {
        identity: request.identity()?,
        registration: request.registration.clone(),
        value: Mutex::new(StopSlotV1 {
            arm_id: arm_id.clone(),
            registration_hash: registration_hash.clone(),
            phase: StopPhaseV1::Quarantined,
            generation: 4,
            cancel_command_id: Some(cancel.command_id),
            durable_cancel: Some(cancel.clone()),
            cancel_prefix: vec![cancel.clone()],
            stop_failures: 0,
        }),
        stop: AtomicBool::new(true),
        worker_owned: AtomicBool::new(false),
        native_ready: AtomicBool::new(false),
        native_worker: Mutex::new(None),
        failures: NativeStopFailures::new(),
        next_start: Mutex::new(Instant::now()),
        _lease: lease.clone(),
        fail_spawn_once: AtomicBool::new(false),
        started_marker: None,
        fault_marker: None,
    });
    state
        .slots
        .lock()
        .unwrap()
        .insert(arm_id.clone(), Arc::clone(&actor));
    let client = EnforcerStopClient(Arc::new(StopClient {
        root: root.clone(),
        executable: root.path().join("no-enforcer-executable"),
        max_slots: 1,
        max_recovery_stop_owners: 0,
        capability: key,
        protocol: StopProtocol::V2,
        affinity: StopAffinity::Legacy,
        cache: Mutex::new(StopCache {
            epoch: Some(epoch),
            entries: BTreeMap::from([(
                arm_id.clone(),
                PendingRestore {
                    retained: RetainedRestore {
                        request,
                        owner: StopOwner::Cancel(Arc::clone(&stop)),
                        binding: None,
                    },
                    revision: 1,
                    acknowledged: Some((epoch, 1)),
                    in_flight: None,
                    retry_at: Instant::now(),
                    failures: 0,
                },
            )]),
            ..StopCache::default()
        }),
        changed: Arc::new(tokio::sync::Notify::new()),
        watch_failed: AtomicBool::new(false),
        attempts: Arc::new(tokio::sync::Semaphore::new(MAX_PENDING_ATTEMPTS)),
        observed_watch_epoch: Mutex::new(None),
        open_barrier: Mutex::new(None),
        observed_epochs: Mutex::new(Vec::new()),
    }));
    // Exercise the bound data, checked endpoint and real STOP receipt. The
    // running Worker fixture covers the separate journal evidence projection.
    let mut target = LegacyTerminalStopLatchTarget {
        arm_id,
        registration_hash,
        scope_hash,
        operation_id,
        cancel,
    };
    let other_scope_hash = hex::encode([0xff; 32]);
    let scope_hash = std::mem::replace(&mut target.scope_hash, other_scope_hash);
    assert_eq!(
        client.confirm_existing_legacy_stop_latch(&target).await,
        Err(EnforcerError::Conflict)
    );
    target.scope_hash = scope_hash;

    let query = || async {
        let (result, served) =
            tokio::join!(client.confirm_existing_legacy_stop_latch(&target), async {
                let stream = tokio::time::timeout(IO_TIMEOUT, listener.accept_checked()).await??;
                serve_connection(stream, Arc::clone(&state)).await?;
                Ok::<_, anyhow::Error>(())
            });
        served?;
        Ok::<_, anyhow::Error>(result)
    };
    assert_eq!(query().await?, Ok(()));
    let mut other_cancel = target.cancel.clone();
    other_cancel.input_hash = "b".repeat(64);
    {
        let mut value = actor.value.lock().unwrap();
        value.durable_cancel = Some(other_cancel.clone());
        value.cancel_prefix = vec![other_cancel];
    }
    assert_eq!(query().await?, Err(EnforcerError::Conflict));
    {
        let mut value = actor.value.lock().unwrap();
        value.durable_cancel = Some(target.cancel.clone());
        value.cancel_prefix = vec![target.cancel.clone()];
    }
    state.slots.lock().unwrap().clear();
    assert_eq!(query().await?, Err(EnforcerError::Conflict));

    // A successor published after authenticated open invalidates even an
    // exact response on the original connection.
    state
        .slots
        .lock()
        .unwrap()
        .insert(target.arm_id.clone(), Arc::clone(&actor));
    let successor = PrivateListener::bind_new(&endpoints)?;
    let barrier = client.pause_next_checked_open();
    let switch_endpoint = async {
        tokio::time::timeout(IO_TIMEOUT, async {
            while !barrier.is_blocked() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await?;
        assert_eq!(barrier.entered_epoch(), Some(epoch));
        let published = lease.publish_endpoint(successor.endpoint());
        barrier.release();
        published?;
        Ok::<_, anyhow::Error>(())
    };
    let (read, changed) = tokio::join!(query(), switch_endpoint);
    changed?;
    assert_eq!(read?, Err(EnforcerError::Conflict));
    // No reconnect to the successor and no restore.
    assert_eq!(
        client.confirm_existing_legacy_stop_latch(&target).await,
        Err(EnforcerError::Conflict)
    );
    let cache = client.0.cache.lock().unwrap();
    assert_eq!(cache.epoch, Some(epoch));
    assert_eq!(cache.entries[&target.arm_id].acknowledged, Some((epoch, 1)));
    assert_eq!(state.slots.lock().unwrap().len(), 1);
    assert!(!actor.worker_owned.load(Ordering::SeqCst));
    drop(cache);
    assert_eq!(
        stops.recovery_status(operation_id)?.unwrap().head(),
        stop.receipt().head()
    );
    Ok(())
}

fn fixture_intent(operation: Uuid) -> anyhow::Result<ExecutionIntentV1> {
    let mut plan: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/broker_intent-plan-v3.json"
    ))?;
    plan["operation_id"] = json!(operation);
    plan["version"] = json!(1);
    plan["options"]["profile"] = json!("strict");
    plan["options"]["network"] = json!("none");
    plan["options"]["provider"] = json!("generic");
    plan["options"]["argv"] = json!(["/usr/bin/id"]);
    for field in [
        "client",
        "auth_mode",
        "client_profile",
        "providers_file",
        "prompt",
        "model",
    ] {
        plan["options"].as_object_mut().unwrap().remove(field);
    }
    plan["request"]["profile"] = json!("strict");
    plan["request"]["agent_id"] = json!("generic-22222222-2222-4222-8222-222222222222");
    plan["request"]["command"]["argv"] = json!(["/usr/bin/id"]);
    plan["request"]
        .as_object_mut()
        .unwrap()
        .remove("provider_resolution");
    plan["provider_input"] = Value::Null;
    plan["provider_credential"] = Value::Null;
    Ok(ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(
        &plan,
    )?)?)
}
