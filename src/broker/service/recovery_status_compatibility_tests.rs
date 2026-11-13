//! Current Service producer → the closed recovery decoder shipped at bd57034.
//! The native fixture supplies only SyntheticReady; no engine is qualified.
use super::*;

// Copied from bd57034:src/broker/service/protocol.rs, restricted to the exact
// RecoveryStatus reply branch. Every closed record keeps deny_unknown_fields;
// the stable clock type remains the same production type as in that source.
mod bd57034 {
    use serde::{Deserialize, Serialize};
    use uuid::Uuid;

    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub(super) struct ReplyEnvelopeV1 {
        pub(super) version: u32,
        pub(super) reply: BrokerReplyV1,
    }

    #[derive(Serialize, Deserialize)]
    #[serde(rename_all = "snake_case", deny_unknown_fields)]
    pub(super) enum BrokerReplyV1 {
        RecoveryStatus { status: RecoveryStatusV1 },
    }

    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub(super) struct RecoveryStatusV1 {
        pub(super) version: u32,
        pub(super) operation_id: Uuid,
        pub(super) tenant_id: String,
        pub(super) journal_mode: JournalModeV1,
        pub(super) source: RecoveryDataSourceV1,
        pub(super) runtime_evidence: RecoveryRuntimeEvidenceV1,
        pub(super) head_revision: u64,
        pub(super) head_hash: String,
        pub(super) incomplete: bool,
        pub(super) reconciliation_required: bool,
        pub(super) cancel_commands: Vec<RecoveryCommandV1>,
        pub(super) known_arms: Vec<RecoveryTargetV1>,
        pub(super) pending_arms: usize,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub(super) enum JournalModeV1 {
        Full,
        StopOnly,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub(super) enum RecoveryDataSourceV1 {
        ConfirmedJournal,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub(super) enum RecoveryRuntimeEvidenceV1 {
        NotQueried,
    }

    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub(super) struct RecoveryCommandV1 {
        pub(super) command_id: Uuid,
        pub(super) revision: u64,
        pub(super) checkpoint_hash: String,
    }

    #[derive(Serialize, Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
    pub(super) enum RecoveryTargetKindV1 {
        Worker,
        Helper { helper_id: String },
    }

    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub(super) struct RecoveryTargetV1 {
        pub(super) arm_id: String,
        pub(super) worker_id: String,
        pub(super) attempt_id: String,
        pub(super) subject: RecoveryTargetKindV1,
        pub(super) original_clock: crate::control::HostClockSampleV1,
        pub(super) original_timeout_seconds: u32,
        pub(super) original_deadline_ns_decimal: String,
        pub(super) cancel_command_id: Option<Uuid>,
    }
}

async fn capture_v1_status(
    service: &Arc<Service>,
    writer: &WriterClient,
    scope: &JournalScopeV1,
    operation: Uuid,
) -> Result<Vec<u8>> {
    let before = writer.stops().recovery_status(operation).await?.unwrap();
    let expected_head = before.head().clone();
    let response = service
        .dispatch(serde_json::from_slice(&serde_json::to_vec(&EnvelopeV1 {
            version: 1,
            profile_id: scope.profile_id().into(),
            credential: RUNNER.into(),
            request: BrokerCommandV1::RecoveryStatus {
                operation_id: operation,
            },
        })?)?)
        .await?;
    ensure!(
        response.run.is_none(),
        "a recovery read scheduled execution"
    );
    let BrokerReplyV1::RecoveryStatus { status } = &response.reply else {
        return Err(anyhow!("the actual V1 read did not produce a status"));
    };
    ensure!(
        status.operation_id == operation
            && status.tenant_id == scope.tenant_id()
            && status.head_revision == expected_head.revision()
            && status.head_hash == expected_head.hash()
            && status.runtime_evidence == RecoveryRuntimeEvidenceV1::NotQueried,
        "the producer changed the requested identity/head or invented runtime evidence"
    );
    let after = writer.stops().recovery_status(operation).await?.unwrap();
    ensure!(
        after.head() == &expected_head,
        "a status read advanced its head"
    );
    Ok(serde_json::to_vec(&ReplyEnvelopeV1 {
        version: 1,
        reply: response.reply,
    })?)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recovery_status_v1_real_producer_is_readable_by_bd57034_for_known_pending_and_final() {
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
    let observed = Arc::new(GatewayObservations::default());
    let gateway = OwnedGateway::start(
        listener,
        Arc::new(store),
        scope.tenant_id(),
        Arc::clone(&observed),
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
            .map_err(|_| anyhow!("compatibility fixture repeated its writer factory"))?;
        let local = jobs.job(operation)?;
        local
            .preclaim
            .announce_registration(operation, arm, full.epoch(), 0)?;
        local
            .preclaim
            .mark_original_unclaimed(arm, full.epoch(), 0)?;
        let service = retained_service(
            &root,
            registry,
            jobs,
            Some(ExecutionEnforcerV1::Full3(full.clone())),
            None,
        )?;
        let known = writer.stops().recovery_status(operation).await?.unwrap();
        ensure!(
            known.stops().len() == 1 && known.retirements().is_empty(),
            "known-Arm canary must contain one genuine Arm and no retirement"
        );
        let mut produced = vec![(
            "known_arm",
            capture_v1_status(&service, &writer, &scope, operation).await?,
        )];
        let pending = audited_pending(
            &writer,
            &AuditClient::new(&origin, RUNNER)?,
            &intent,
            &full,
            &slot,
        )
        .await?;
        let identity = pending.identity().clone();
        let snapshot = writer.stops().recovery_status(operation).await?.unwrap();
        ensure!(
            snapshot.retirements().len() == 1 && snapshot.retirements()[0].finalized().is_none(),
            "Pending canary requires the real writer's unfinalized retirement"
        );
        produced.push((
            "pending",
            capture_v1_status(&service, &writer, &scope, operation).await?,
        ));
        let authorized = writer.authorize_unused_retirement(pending).await?;
        let verified = full.retire_unused(authorized).await?;
        writer.stops().finalize_retirement(verified).await?;
        let replay = full
            .query_retirement(&identity)
            .await?
            .ok_or_else(|| anyhow!("real Q missing after Final"))?;
        full.retire_local_slot(&replay)?;
        let snapshot = writer.stops().recovery_status(operation).await?.unwrap();
        ensure!(
            snapshot.retirements().len() == 1 && snapshot.retirements()[0].finalized().is_some(),
            "Final canary requires the real writer's finalized retirement"
        );
        produced.push((
            "final",
            capture_v1_status(&service, &writer, &scope, operation).await?,
        ));
        ensure!(
            observed.runner_identity_requests.load(Ordering::SeqCst) == 3,
            "every status request must authenticate on the actual control transport"
        );
        // All bytes come from the production reply envelope. The copied
        // historical decoder is neither weakened nor fed a hand-built status.
        let mut failures = Vec::new();
        for (phase, bytes) in produced {
            match serde_json::from_slice::<bd57034::ReplyEnvelopeV1>(&bytes) {
                Ok(legacy) => {
                    let bd57034::BrokerReplyV1::RecoveryStatus { status } = &legacy.reply;
                    ensure!(
                        legacy.version == 1 && status.version == 1,
                        "the original RecoveryStatus request received a different protocol version"
                    );
                    ensure!(
                        status.runtime_evidence == bd57034::RecoveryRuntimeEvidenceV1::NotQueried
                            && (phase == "known_arm" || status.reconciliation_required),
                        "a V1 view hid an undescribed D5 retirement as reconciled runtime evidence"
                    );
                    ensure!(
                        serde_json::to_value(&legacy)? == serde_json::from_slice::<Value>(&bytes)?,
                        "the historical decoder discarded part of a V1 status"
                    );
                }
                Err(error) => failures.push(format!("{phase}: {error}")),
            }
        }
        ensure!(
            failures.is_empty(),
            "actual RecoveryStatus V1 producer broke the bd57034 closed decoder: {}",
            failures.join("; ")
        );
        Ok(())
    })
    .await;
    native.finish().await;
    let drained = gateway.finish().await;
    outcome.unwrap();
    drained.unwrap();
}
