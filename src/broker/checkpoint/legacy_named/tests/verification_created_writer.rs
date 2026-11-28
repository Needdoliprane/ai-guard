use super::*;
use crate::{
    broker::enforcer::check_verification_prospective_facts_for_test,
    broker::writer::LegacyNamedImportVerificationArmWrite,
    runner::broker::{broker_helper_spec, create_import_volume},
    runtime::{ContainerPurpose, ContainerSpec, ManagedRuntime, NetworkMode, Runtime},
};
use anyhow::{Context, ensure};
use std::{fs, path::Path};
use tokio::{process::Command, time::timeout};

pub(super) const JOURNAL: &str = "verification-created-writer";

pub(super) enum Scenario {
    AckAfterStop,
    ArmWriter { lose_reply: bool },
}

#[derive(Clone, Copy, Debug)]
struct ArmWriterPhase(&'static str);

impl std::fmt::Display for ArmWriterPhase {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0)
    }
}

#[tokio::test]
async fn named_verification_created_ack_survives_durable_stop_and_lost_reply() -> anyhow::Result<()>
{
    Box::pin(named_admission_fixture(Some(Scenario::AckAfterStop))).await
}

#[tokio::test]
async fn named_verification_arm_writer_retains_one_birth_and_only_replays_after_stop()
-> anyhow::Result<()> {
    for lose_reply in [false, true] {
        Box::pin(named_admission_fixture(Some(Scenario::ArmWriter {
            lose_reply,
        })))
        .await
        .inspect_err(|failure| {
            let phase = failure
                .downcast_ref::<ArmWriterPhase>()
                .copied()
                .unwrap_or(ArmWriterPhase("fixture_prefix"));
            eprintln!("\ng04_oracle_failure_phase={phase}");
        })?;
    }
    Ok(())
}

async fn fixture_runtime(root: &Path, authority: Uuid) -> anyhow::Result<ManagedRuntime> {
    let source = root.join("engine.rs");
    let program = root.join("engine");
    fs::write(
        &source,
        include_str!("../../../../../tests/runtime_fixtures/runner_terminal.rs"),
    )?;
    let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let compiled = timeout(
        Duration::from_secs(30),
        Command::new(compiler)
            .args([
                "--edition=2024",
                "--crate-name",
                "verification_created_fixture",
            ])
            .arg(&source)
            .arg("-o")
            .arg(&program)
            .kill_on_drop(true)
            .output(),
    )
    .await??;
    ensure!(
        compiled.status.success(),
        "synthetic engine fixture did not compile"
    );
    let metadata = program.with_extension("fixture");
    fs::create_dir(&metadata)?;
    fs::write(metadata.join("root"), root.as_os_str().as_encoded_bytes())?;
    fs::write(metadata.join("case"), "named-verification-created-ack")?;
    let runtime = Runtime::with_program(RuntimeKind::Docker, &program)?;
    ManagedRuntime::connect_docker_with_runtime(authority, "unix:///fixture/docker.sock", runtime)
        .await
}

pub(super) async fn exercise(
    root: &Path,
    writer: WriterClient,
    plan: &LegacyConfirmedPlanV1,
    archive_digest: &str,
    mut arm: ArmResultV1,
    (mut claim, mut claim_receipt): (LegacyNamedImportClaimRequestV1, MutationReceiptV1),
    scenario: Scenario,
) -> anyhow::Result<()> {
    let operation = plan.operation_id;
    let admitted = writer.load(operation).await?.context("named job missing")?;
    let retained = writer.load_legacy_named_admission(operation).await?;
    let worker = retained
        .admission_result()
        .and_then(|result| result.worker.as_ref())
        .context("admitted worker missing")?;
    let runtime = fixture_runtime(root, uuid(&worker.launch.authority_id)?).await?;
    let binding = runtime.binding();
    let generation = CancelGeneration::new();
    let expected_generation = generation.current();
    let helper_id = uuid(&claim.claim.helper_id)?;
    let import = writer
        .record_legacy_named_import_creation(plan, archive_digest, helper_id, binding)
        .await?;
    let preparation = import
        .job()
        .broker_checkpoint_v2()
        .and_then(|checkpoint| checkpoint.supervision().legacy_named_admission())
        .and_then(LegacyNamedAdmissionCheckpoint::import_creation)
        .context("Import Create intent missing")?;
    create_import_volume(&runtime, worker, &generation, expected_generation).await?;
    let original = runtime
        .create_owned_import_for_generation(
            &broker_helper_spec(preparation.policy())?,
            &generation,
            expected_generation,
        )
        .await?;
    writer
        .acknowledge_legacy_named_import_creation(plan, archive_digest, binding, original.clone())
        .await?;

    // The fixture supplies the control receipts and the terminal state of the
    // first helper. The Writer and the runtime inspection of both helpers run.
    arm.arm.inspection.container_id = original.handle.id().to_owned();
    arm.arm.authority_binding_hash = binding.fingerprint();
    arm.arm.configuration_fingerprint = original
        .observation
        .configuration_fingerprint
        .clone()
        .context("original Import configuration fingerprint missing")?;
    arm.receipt.input_hash = arm.arm.input_hash()?;
    claim.claim.arm_receipt = arm.receipt.clone();
    claim_receipt.input_hash = claim.input_hash()?;
    let recorded_arm = writer
        .record_legacy_named_import_arm(plan, archive_digest, arm.arm.clone())
        .await?;
    writer
        .acknowledge_legacy_named_import_arm(operation, recorded_arm.request(), arm.clone())
        .await?;
    writer
        .record_legacy_named_import_claim(plan, claim.clone())
        .await?;
    writer
        .acknowledge_legacy_named_import_claim(operation, &claim, claim_receipt.clone())
        .await?;
    fs::write(
        root.join("containers")
            .join(original.handle.id())
            .join("state"),
        "exited",
    )?;

    let terminal = import_observation_fixture(
        plan,
        &claim,
        &claim_receipt,
        &arm,
        LegacyNamedImportObservationPhaseV1::Terminal,
        "2026-10-29T10:00:00Z",
        "2026-10-29T10:00:02Z",
        Some("2026-10-29T10:00:01Z"),
    );
    let terminal_receipt = MutationReceiptV1 {
        operation_id: terminal.observation.operation_id.clone(),
        worker_id: Some(worker.worker_id.clone()),
        revision: None,
        input_hash: terminal.input_hash()?,
        audit_seq: claim_receipt.audit_seq + 1,
        audit_hash: "e".repeat(64),
    };
    writer
        .record_legacy_named_import_observation(plan, terminal.clone())
        .await?;
    writer
        .acknowledge_legacy_named_import_observation(operation, &terminal, terminal_receipt.clone())
        .await?;

    let verification_id = Uuid::new_v4();
    let verification = writer
        .record_legacy_named_import_verification_plan(plan, verification_id)
        .await?;
    let policy = verification.preparation().policy();
    let spec = ContainerSpec::with_operation(
        plan.session_id,
        ContainerPurpose::ImportVerification,
        &policy.image_id,
        policy.command.argv.clone(),
        NetworkMode::None,
        verification_id,
    )?
    .with_security_preset(policy.security_preset)
    .with_managed_names(&policy.stable_name, &policy.volume_name)?
    .with_read_only_workspace_volume()?;
    let authorization = writer
        .record_legacy_named_import_verification_create(
            plan,
            &terminal_receipt,
            binding,
            &generation,
            expected_generation,
        )
        .await?
        .context("single verification Create authorization missing")?;
    if let Scenario::ArmWriter { lose_reply } = scenario {
        return Box::pin(async {
            // The same synthetic inspection can complete before STOP here.
            fs::write(root.join("verification-inspection-release"), b"")?;
            let created = timeout(
                Duration::from_secs(30),
                runtime.create_owned_named_import_verification_for_generation(
                    &spec,
                    operation,
                    policy,
                    authorization,
                    || Ok(()),
                ),
            )
            .await
            .context(ArmWriterPhase("verification_create"))?
            .context(ArmWriterPhase("verification_create"))?;
            writer
                .acknowledge_legacy_named_import_verification_created(&admitted, created)
                .await
                .context(ArmWriterPhase("created_ack"))?;
            Box::pin(exercise_arm_writer(
                root,
                writer,
                &admitted,
                binding,
                &generation,
                expected_generation,
                lose_reply,
            ))
            .await
        })
        .await;
    }
    let create_head = authorization.confirmed_head().clone();
    let cancel_id = Uuid::new_v4();
    let (created, stop_head) = tokio::join!(
        timeout(
            Duration::from_secs(30),
            runtime.create_owned_named_import_verification_for_generation(
                &spec,
                operation,
                policy,
                authorization,
                || Ok(()),
            ),
        ),
        async {
            // The fixture has emitted its Create response and is holding the
            // subsequent inspection. The STOP must commit before the ACK exists.
            timeout(Duration::from_secs(5), async {
                while !root.join("verification-inspection-entered").is_file() {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .context("verification Created inspection was not reached")?;
            assert_eq!(generation.invalidate(), expected_generation + 1);
            let stop = writer
                .stops()
                .append_cancel(CancelInputV1::new(operation, cancel_id)?)
                .await
                .map_err(|failure| failure.error());
            fs::write(root.join("verification-inspection-release"), b"")?;
            Ok::<_, anyhow::Error>(stop?.receipt().head().clone())
        },
    );
    let stop_head = stop_head?;
    assert_eq!(stop_head.revision(), create_head.revision() + 1);
    let created = created??;
    let expected_snapshot = canonical(created.snapshot_for_test())?;
    assert_eq!(
        created.snapshot_for_test().handle.operation_id(),
        verification_id
    );
    let calls = fs::read_to_string(root.join("calls"))?;

    // Fault after publishing the real journal head, before replying to the ACK.
    let mut barrier = writer.advance_reply_barrier_for_test(true).await?;
    {
        let ack = writer.acknowledge_legacy_named_import_verification_created(&admitted, created);
        tokio::pin!(ack);
        tokio::select! {
            _ = &mut ack => anyhow::bail!("verification ACK returned before the publication barrier"),
            entered = timeout(Duration::from_secs(4), barrier.try_entered_for_test()) => {
                entered?.map_err(anyhow::Error::msg)?;
            }
        }
        barrier.release_for_test();
        assert_eq!(ack.await, Err(JournalError::Io));
    }
    drop(barrier);
    let reopened = reopen_writer(root, writer).await?;
    let saved = reopened.load_legacy_named_admission(operation).await?;
    assert_eq!(
        canonical(
            saved
                .import_verification_created()
                .context("Created ACK absent after reopen")?
        )?,
        expected_snapshot
    );
    let stop = reopened
        .stops()
        .cancel_stop_projection(operation, cancel_id)
        .await?
        .context("STOP absent after reopening the Created ACK")?;
    assert_eq!(stop.receipt().head(), &stop_head);
    let head = reopened
        .load(operation)
        .await?
        .context("reopened named job missing")?
        .head()
        .clone();
    assert_eq!(head.revision(), stop_head.revision() + 1);
    assert!(
        reopened
            .record_legacy_named_import_verification_create(
                plan,
                &terminal_receipt,
                binding,
                &generation,
                expected_generation,
            )
            .await?
            .is_none()
    );
    assert_eq!(
        reopened
            .load(operation)
            .await?
            .context("named job missing after replay")?
            .head(),
        &head
    );
    assert_eq!(fs::read_to_string(root.join("calls"))?, calls);
    assert_eq!(
        calls
            .lines()
            .filter(|line| line.starts_with("create ") && line.ends_with(" import_verification"))
            .count(),
        1
    );
    assert!(!calls.lines().any(|line| {
        ["start ", "kill ", "rm "]
            .iter()
            .any(|prefix| line.starts_with(prefix))
    }));
    Ok(())
}

async fn exercise_arm_writer(
    root: &Path,
    writer: WriterClient,
    admitted: &crate::broker::journal::AcceptedJobV1,
    binding: &RuntimeAuthority,
    generation: &CancelGeneration,
    expected_generation: u64,
    lose_reply: bool,
) -> anyhow::Result<()> {
    use LegacyNamedImportVerificationArmWrite::{Recorded, Replayed};

    let operation = admitted.operation_id();
    let before = writer
        .load(operation)
        .await?
        .context("Created job missing")?;
    let before_revision = before.head().revision();
    let checkpoint = before
        .broker_checkpoint_v3()
        .context("Created lifecycle missing")?;
    assert!(checkpoint.commands().is_empty());
    let retained = checkpoint
        .broker()
        .supervision()
        .legacy_named_admission()
        .context("named Created prefix missing")?;
    assert!(retained.import_verification_arm_request().is_none());
    let request = verification_arm_request(retained).context(ArmWriterPhase("arm_request"))?;
    drop(before);
    let calls = fs::read(root.join("calls"))?;
    let retain = || {
        Box::pin(writer.record_legacy_named_import_verification_arm(
            admitted,
            request.clone(),
            binding,
            generation,
            expected_generation,
        ))
    };

    let confirmation = if lose_reply {
        let mut barrier = writer
            .advance_reply_barrier_for_test(true)
            .await
            .context(ArmWriterPhase("lost_reply"))?;
        let mut first = retain();
        tokio::select! {
            _ = &mut first => return Err(anyhow::Error::msg(ArmWriterPhase("lost_reply"))),
            entered = timeout(Duration::from_secs(4), barrier.try_entered_for_test()) => {
                entered.context(ArmWriterPhase("lost_reply"))?
                    .map_err(anyhow::Error::msg).context(ArmWriterPhase("lost_reply"))?;
            }
        }
        barrier.release_for_test();
        assert!(matches!(
            first.await,
            Err(NamedImportArmWriteFailure::PublicationUnconfirmed(
                JournalError::Io
            ))
        ));
        None
    } else {
        let pause = writer
            .pause_for_test()
            .await
            .context(ArmWriterPhase("concurrent_cas"))?;
        let mut first = retain();
        let mut second = retain();
        // Both Loads reach the FIFO before either future can offer its CAS.
        assert!(futures_util::poll!(first.as_mut()).is_pending());
        assert!(futures_util::poll!(second.as_mut()).is_pending());
        drop(pause);
        let (first, second) = tokio::join!(first, second);
        let (recorded, replayed) = match (first, second) {
            (Ok(Recorded(recorded)), Ok(Replayed(replayed)))
            | (Ok(Replayed(replayed)), Ok(Recorded(recorded))) => (recorded, replayed),
            _ => return Err(anyhow::Error::msg(ArmWriterPhase("concurrent_cas"))),
        };
        assert_eq!(canonical(replayed.as_ref())?, canonical(&request)?);
        Some(recorded)
    };

    let arm_head = writer
        .load(operation)
        .await
        .context(ArmWriterPhase("born_head"))?
        .context(ArmWriterPhase("born_head"))?
        .head()
        .clone();
    assert_eq!(arm_head.revision(), before_revision + 1);
    let projected_generation = if let Some(confirmation) = confirmation {
        assert_eq!(confirmation.job().head(), &arm_head);
        assert_eq!(confirmation.job().operation_id(), operation);
        assert_eq!(confirmation.job().intent_hash(), admitted.intent_hash());
        let scope = writer.scope();
        assert_eq!(canonical(confirmation.job().scope())?, canonical(scope)?);
        assert_eq!(confirmation.generation().1, expected_generation);
        let owned = confirmation
            .job()
            .broker_checkpoint_v2()
            .and_then(|checkpoint| checkpoint.supervision().legacy_named_admission())
            .and_then(LegacyNamedAdmissionCheckpoint::import_verification_created)
            .context(ArmWriterPhase("born_head"))?
            .clone();
        let control_origin_hash = hex::encode(Sha256::digest(scope.gateway_origin().as_bytes()));
        let target = ProspectiveRegistrationV1::new(
            control_origin_hash.clone(),
            scope.tenant_id().to_owned(),
            request.arm.clone(),
            binding.clone(),
            owned.clone(),
        )?;
        let other_installation = Uuid::from_u128(scope.installation_id().as_u128() ^ 1);
        let other_scope = JournalScopeV1::new(
            other_installation,
            scope.tenant_id(),
            scope.gateway_origin(),
            scope.control_namespace_uuid(),
            scope.profile_id(),
        )?;
        // Keep the target shape valid and vary only the retained observation.
        let mut other_owned = owned;
        other_owned.observation.observed_at += chrono::TimeDelta::seconds(1);
        let other_target = ProspectiveRegistrationV1::new(
            control_origin_hash,
            scope.tenant_id().to_owned(),
            request.arm.clone(),
            binding.clone(),
            other_owned,
        )?;
        let projected = check_verification_prospective_facts_for_test(
            confirmation,
            target,
            scope,
            &request,
            &other_scope,
            &other_target,
        )
        .context(ArmWriterPhase("born_head"))?;
        assert_eq!(projected.1, expected_generation);
        Some(projected)
    } else {
        None
    };
    assert_replayed(retain().await, &request).context(ArmWriterPhase("replay_before_stop"))?;
    assert_eq!(
        writer
            .load(operation)
            .await?
            .context("replayed Arm job missing")?
            .head(),
        &arm_head,
    );

    let stopped_generation = generation.invalidate();
    assert_eq!(stopped_generation, expected_generation + 1);
    if let Some((projected, expected)) = projected_generation {
        assert_eq!(projected.current(), stopped_generation);
        assert!(projected.with_current_generation(expected, || ()).is_err());
    }
    let stop_id = Uuid::new_v4();
    let stop_head = {
        let stop = writer
            .stops()
            .append_cancel(CancelInputV1::new(operation, stop_id)?)
            .await
            .map_err(|failure| failure.error())
            .context(ArmWriterPhase("stop_append"))?;
        // The projection shares the writer lock. Keep only the durable head
        // across reopening so the new writer acquires its own lease.
        stop.receipt().head().clone()
    };
    assert_eq!(stop_head.revision(), arm_head.revision() + 1);
    let reopened = reopen_writer(root, writer)
        .await
        .context(ArmWriterPhase("cold_reopen"))?;
    let cold_stop = reopened
        .stops()
        .cancel_stop_projection(operation, stop_id)
        .await
        .context(ArmWriterPhase("replay_after_stop"))?
        .context(ArmWriterPhase("replay_after_stop"))?;
    assert_eq!(cold_stop.receipt().head(), &stop_head);
    assert_replayed(
        reopened
            .record_legacy_named_import_verification_arm(
                admitted,
                request.clone(),
                binding,
                generation,
                expected_generation,
            )
            .await,
        &request,
    )
    .context(ArmWriterPhase("replay_after_stop"))?;
    let saved = reopened
        .load(operation)
        .await?
        .context("stopped Arm job missing after reopen")?;
    assert_eq!(saved.head(), &stop_head);
    let retained = saved
        .broker_checkpoint_v2()
        .and_then(|checkpoint| checkpoint.supervision().legacy_named_admission())
        .and_then(LegacyNamedAdmissionCheckpoint::import_verification_arm_request)
        .context("verification Arm body missing after reopen")?;
    assert_eq!(canonical(retained)?, canonical(&request)?);
    assert_eq!(fs::read(root.join("calls"))?, calls);
    Ok(())
}

fn verification_arm_request(
    retained: &LegacyNamedAdmissionCheckpoint,
) -> anyhow::Result<LegacyNamedImportVerificationArmRequestV2> {
    let facts = retained
        .import_verification_arm_facts()?
        .context("verification Arm facts missing")?;
    // Reuse the fixture's clock and deadline bytes as input to this Writer oracle.
    let mut arm = retained
        .import_arm_request()
        .context("first Import Arm missing")?
        .arm;
    let policy = facts.preparation().policy();
    arm.operation_id = facts.operation_id().to_owned();
    arm.subject = ArmSubjectV1::Helper {
        helper_id: policy.helper_id.clone(),
        purpose: HelperPurposeV1::ImportVerification,
        policy_version: policy.version,
        permission: Box::new(facts.preparation().permission().clone()),
    };
    arm.inspection = facts.inspection().clone();
    arm.authority_binding_hash = facts.owned().handle.authority_fingerprint().to_owned();
    arm.configuration_fingerprint_version = facts.owned().handle.fingerprint_version();
    arm.configuration_fingerprint = facts
        .owned()
        .observation
        .configuration_fingerprint
        .clone()
        .context("verification configuration fingerprint missing")?;
    Ok(LegacyNamedImportVerificationArmRequestV2 {
        version: 2,
        binding_ref: facts.plan().binding_ref,
        namespace: facts.plan().control_namespace_uuid.clone(),
        arm,
    })
}

fn assert_replayed(
    result: Result<LegacyNamedImportVerificationArmWrite, NamedImportArmWriteFailure>,
    expected: &LegacyNamedImportVerificationArmRequestV2,
) -> anyhow::Result<()> {
    let LegacyNamedImportVerificationArmWrite::Replayed(saved) =
        result.map_err(|error| anyhow::anyhow!("verification Arm replay failed: {error:?}"))?
    else {
        anyhow::bail!("Arm replay minted a birth");
    };
    assert_eq!(canonical(saved.as_ref())?, canonical(expected)?);
    Ok(())
}

async fn reopen_writer(root: &Path, writer: WriterClient) -> anyhow::Result<WriterClient> {
    let scope = writer.scope().clone();
    drop(writer);
    let directory = Directory::open_existing(&root.join(JOURNAL))?;
    let journal = timeout(Duration::from_secs(5), async {
        loop {
            match Journal::open(
                &directory,
                scope.clone(),
                &FsExclusions::new(),
                JournalLimitsV1::default(),
                ControlSecrets::new(vec![])?,
            ) {
                Ok(journal) => break Ok(journal),
                Err(JournalError::Busy) => tokio::time::sleep(Duration::from_millis(5)).await,
                Err(error) => break Err(error),
            }
        }
    })
    .await??;
    Ok(WriterClient::start(journal, 8, Duration::from_secs(5))?)
}
