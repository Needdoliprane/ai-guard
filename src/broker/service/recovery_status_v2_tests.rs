//! Actual Service/Journal/Writer → private IPC → public client, followed by
//! bounded incompatible-peer responses derived from that real producer.
use super::*;
use std::future::Future;

struct OwnedIpcPeer {
    _root: tempfile::TempDir,
    root: std::path::PathBuf,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<Result<()>>>,
}

impl OwnedIpcPeer {
    async fn start<F, Fut>(mut handle: F) -> Result<Self>
    where
        F: FnMut(UnixStream) -> Fut + Send + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        // A separate, owned transport root keeps Unix paths within their native
        // bound. Service authentication still uses its original real registry.
        let temp = tempfile::Builder::new()
            .prefix("ag-rs2-")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(
                std::env::var_os("AI_GUARD_TEST_D5_IPC_ROOT").unwrap_or_else(|| "/tmp".into()),
            )?;
        let root = temp.path().canonicalize()?;
        let directory = Directory::open_existing(&root)?;
        let broker = directory.persistent_child("broker", true)?.unwrap();
        let lease = RoleLease::acquire(&broker)?;
        let listener = PrivateListener::bind_new(&lease.listener_root()?)?;
        lease.publish_endpoint(listener.endpoint())?;
        let (stop, mut receive) = oneshot::channel();
        let task = tokio::spawn(async move {
            let _lease = lease;
            loop {
                tokio::select! {
                    _ = &mut receive => return Ok(()),
                    accepted = listener.accept_checked() => {
                        handle(accepted?).await?;
                    }
                }
            }
        });
        Ok(Self {
            _root: temp,
            root,
            stop: Some(stop),
            task: Some(task),
        })
    }

    async fn finish(mut self) -> Result<()> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(mut task) = self.task.take() {
            match tokio::time::timeout(Duration::from_secs(5), &mut task).await {
                Ok(result) => result??,
                Err(_) => {
                    task.abort();
                    let _ = task.await;
                    return Err(anyhow!("owned recovery-status IPC peer did not drain"));
                }
            }
        }
        Ok(())
    }
}

impl Drop for OwnedIpcPeer {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

async fn read_v2(
    peer: &OwnedIpcPeer,
    scope: &JournalScopeV1,
    operation: Uuid,
) -> Result<RecoveryStatusV2> {
    let reply = crate::broker::service::request(
        &peer.root,
        scope.profile_id(),
        RUNNER,
        BrokerCommandV1::RecoveryStatusV2 {
            operation_id: operation,
        },
    )
    .await?;
    let BrokerReplyV1::RecoveryStatusV2 { status } = reply else {
        return Err(anyhow!("actual V2 client returned another variant"));
    };
    Ok(status)
}

// The old service's closed request decoder rejects the additive V2 command.
// Credentials are synthetic and never rendered in assertion diagnostics.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyEnvelope {
    version: u32,
    profile_id: String,
    credential: String,
    request: LegacyCommand,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum LegacyCommand {
    RecoveryStatus { operation_id: Uuid },
}

async fn incompatible_peers_do_not_trigger_v1_fallback(
    produced: Value,
    legacy_reply: Value,
    scope: &JournalScopeV1,
    operation: Uuid,
) -> Result<()> {
    let mut version = produced.clone();
    version["reply"]["recovery_status_v2"]["status"]["version"] = json!(1);
    let mut journal_version = produced.clone();
    journal_version["reply"]["recovery_status_v2"]["status"]["journal"]["version"] = json!(2);
    let mut preclaim_version = produced.clone();
    preclaim_version["reply"]["recovery_status_v2"]["status"]["preclaim"][0]["preclaim"]["version"] =
        json!(2);
    let mut correlation = produced;
    correlation["reply"]["recovery_status_v2"]["status"]["journal"]["operation_id"] =
        json!(Uuid::new_v4());
    for (label, response, old_service) in [
        ("wrong_variant", legacy_reply, false),
        ("wrong_version", version, false),
        ("wrong_journal_version", journal_version, false),
        ("wrong_preclaim_version", preclaim_version, false),
        ("wrong_operation", correlation, false),
        (
            "old_service",
            json!({"version":1,"reply":{"error":{"code":"invalid_input"}}}),
            true,
        ),
    ] {
        let requests = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&requests);
        let peer = OwnedIpcPeer::start(move |mut stream| {
            let response = response.clone();
            let observed = Arc::clone(&observed);
            async move {
                let request: Value = read_frame(&mut stream, FRAME_BYTES, READ_TIMEOUT).await?;
                observed.fetch_add(1, Ordering::SeqCst);
                ensure!(
                    request["request"].get("recovery_status_v2").is_some(),
                    "V2 request silently changed into a legacy command"
                );
                if old_service {
                    let decoded = serde_json::from_value::<LegacyEnvelope>(request);
                    // Read every old field on its positive branch so this
                    // copied decoder remains a real checked schema, not Value.
                    if let Ok(legacy) = decoded {
                        let LegacyCommand::RecoveryStatus { operation_id } = legacy.request;
                        let _ = (
                            legacy.version,
                            legacy.profile_id,
                            legacy.credential,
                            operation_id,
                        );
                        return Err(anyhow!("old closed decoder accepted the V2 request"));
                    }
                }
                write_frame(&mut stream, &response, FRAME_BYTES, WRITE_TIMEOUT).await?;
                Ok(())
            }
        })
        .await?;
        let result = crate::broker::service::request(
            &peer.root,
            scope.profile_id(),
            RUNNER,
            BrokerCommandV1::RecoveryStatusV2 {
                operation_id: operation,
            },
        )
        .await;
        let one_request = requests.load(Ordering::SeqCst) == 1;
        peer.finish().await?;
        ensure!(
            one_request,
            "{label}: the client emitted more than one request"
        );
        let error = result
            .err()
            .ok_or_else(|| anyhow!("{label}: incompatible V2 response accepted"))?;
        ensure!(
            error.to_string().contains("no V1 fallback was attempted"),
            "{label}: missing explicit V2 compatibility refusal"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recovery_status_v2_real_producer_client_preserve_scope_and_reject_incompatible_replies_without_fallback()
 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let LiveFixture {
        _root,
        _store_root,
        store,
        journal,
        intent,
        head,
        registration,
    } = live_fixture_with_gateway(&origin, true);
    let scope = journal.scope().clone();
    let store = Arc::new(store);
    let gateway = OwnedGateway::start(
        listener,
        Arc::clone(&store),
        scope.tenant_id(),
        Arc::new(GatewayObservations::default()),
    )
    .await
    .unwrap();
    let (native, full, slot) =
        OriginalUnusedFixture::register(scope.installation_id(), registration)
            .await
            .unwrap();
    let outcome: Result<()> = boxed_recovery_phase(|| async {
        let operation = intent.operation_id();
        let writer = WriterClient::start(journal.clone(), 16, Duration::from_secs(10))?;
        let arm = slot.full3_registration_binding()?.arm_id();
        writer
            .bind_full3_registration(operation, head, slot.full3_registration_binding()?)
            .await?;
        let root = _root.path().canonicalize()?;
        let registry =
            configured_registry(&root, &scope, &intent, &_store_root.path().canonicalize()?)?;
        let jobs = Arc::new(ProfileJobs::new());
        jobs.writer
            .set(ProfileWriterCapabilities::from_full(writer.clone()))
            .map_err(|_| anyhow!("V2 fixture repeated its writer factory"))?;
        let local = jobs.job(operation)?;
        local.preclaim.announce_registration(operation, arm, full.epoch(), 0)?;
        local.preclaim.mark_original_unclaimed(arm, full.epoch(), 0)?;
        let service = retained_service(
            &root,
            registry,
            Arc::clone(&jobs),
            Some(ExecutionEnforcerV1::Full3(full.clone())),
            None,
        )?;
        let serving = Arc::clone(&service);
        let peer = OwnedIpcPeer::start(move |stream| {
            let serving = Arc::clone(&serving);
            async move {
                serving.connection(stream).await;
                Ok(())
            }
        }).await?;
        let known_head = writer.stops().recovery_status(operation).await?.unwrap().head().clone();
        let known = read_v2(&peer, &scope, operation).await?;
        ensure!(
            known.version == 2 && known.journal.version == 1
                && known.journal.operation_id == operation
                && known.journal.tenant_id == scope.tenant_id()
                && known.journal.head_revision == known_head.revision()
                && known.journal.head_hash == known_head.hash()
                && known.journal.runtime_evidence == RecoveryRuntimeEvidenceV1::NotQueried
                && known.journal.known_arms.len() == 1 && known.retirements.is_empty()
                && known.preclaim.len() == 1 && known.preclaim[0].arm_id == arm.to_string()
                && known.preclaim[0].preclaim.state == crate::broker::service::PreclaimStateV1::OriginalUnclaimed
                && matches!(known.preclaim[0].preclaim.source, PreclaimSourceV1::ProcessLocal)
                && matches!(known.preclaim[0].preclaim.freshness, PreclaimFreshnessV1::CurrentProcessState),
            "V2 known-Arm diagnostic changed journal identity/provenance or lost local preclaim data"
        );
        let pending = audited_pending(&writer, &AuditClient::new(&origin, RUNNER)?, &intent, &full, &slot).await?;
        let identity = pending.identity().clone();
        let pending_head = writer.stops().recovery_status(operation).await?.unwrap().head().clone();
        let status = read_v2(&peer, &scope, operation).await?;
        ensure!(
            status.journal.reconciliation_required
                && status.journal.head_revision == pending_head.revision()
                && status.retirements.len() == 1
                && status.retirements[0].arm_id == arm
                && status.retirements[0].retirement_id == *identity.retirement_id()
                && matches!(status.retirements[0].phase, RetirementPhaseV1::Pending)
                && status.retirements[0].intention_audit_confirmed
                && status.retirements[0].guardian_receipt_hash.is_none(),
            "V2 did not expose the genuine audit-ACKed Pending without a guardian receipt"
        );
        let authorized = writer.authorize_unused_retirement(pending).await?;
        let verified = full.retire_unused(authorized).await?;
        let receipt_hash = verified.receipt().digest()?.as_str().to_owned();
        writer.stops().finalize_retirement(verified).await?;
        let replay = full.query_retirement(&identity).await?.unwrap();
        full.retire_local_slot(&replay)?;
        let before = writer.stops().recovery_status(operation).await?.unwrap().head().clone();
        let quota = canonical(&store.worker_quota(&QuotaPolicyV1::default())?)?;
        let usage = canonical(&store.worker_usage()?)?;
        let admission_closed = service.require_starts().await.is_err();
        let final_status = read_v2(&peer, &scope, operation).await?;
        ensure!(
            final_status.journal.reconciliation_required
                && final_status.journal.runtime_evidence == RecoveryRuntimeEvidenceV1::NotQueried
                && final_status.journal.head_revision == before.revision()
                && final_status.retirements.len() == 1
                && matches!(final_status.retirements[0].phase, RetirementPhaseV1::Final)
                && final_status.retirements[0].guardian_receipt_hash.as_deref() == Some(receipt_hash.as_str()),
            "V2 Final diagnostic invented runtime completion or lost its real guardian receipt"
        );
        for (profile, credential) in [(scope.profile_id(), VIEWER), ("unknown-profile", RUNNER)] {
            let refused = crate::broker::service::request(&peer.root, profile, credential,
                BrokerCommandV1::RecoveryStatusV2 { operation_id: operation }).await;
            ensure!(refused.is_err(), "V2 bypassed the existing identity/profile guard");
        }
        ensure!(
            writer.stops().recovery_status(operation).await?.unwrap().head() == &before
                && canonical(&store.worker_quota(&QuotaPolicyV1::default())?)? == quota
                && canonical(&store.worker_usage()?)? == usage
                && service.require_starts().await.is_err() == admission_closed
                && jobs.known(Uuid::nil()).is_none(),
            "V2 reads changed journal/allocation data or upgraded admission"
        );
        let legacy = crate::broker::service::request(&peer.root, scope.profile_id(), RUNNER,
            BrokerCommandV1::RecoveryStatus { operation_id: operation }).await?;
        let produced = serde_json::to_value(ReplyEnvelopeV1 {
            version: 1,
            reply: BrokerReplyV1::RecoveryStatusV2 { status: known },
        })?;
        let legacy_reply = serde_json::to_value(ReplyEnvelopeV1 { version: 1, reply: legacy })?;
        peer.finish().await?;
        incompatible_peers_do_not_trigger_v1_fallback(produced, legacy_reply, &scope, operation).await?;
        Ok(())
    }).await;
    native.finish().await;
    let drained = gateway.finish().await;
    outcome.unwrap();
    drained.unwrap();
}
