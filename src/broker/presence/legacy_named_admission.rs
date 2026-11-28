//! Private broker transport for the retained Legacy confirmation/admission.
//! This client has no Runner surface and never starts a runtime.
use super::*;
use crate::{
    audit::legacy_session_prepare::LegacyNamedCancelReservationV1,
    broker::{
        checkpoint::LegacyNamedAdmissionCheckpoint,
        enforcer::{
            EnforcerClient, EnforcerError, ExecutionEnforcerV1, LegacyNamedProspectiveFailure,
            NamedImportArmBirthV1, ProspectiveArmExchange, ProspectiveRegistrationV1,
        },
        journal::{CancelStopProjectionV1, JournalError, JournalScopeV1, RevisionRefV1, canonical},
        lifecycle::LocalCommandRequestV1,
        presence::http::{Client, Limits},
        writer::{NamedImportArmWriteFailure, RecordedLegacyNamedImportArm as ImportArmWrite},
    },
    control::{AdmissionRefusalV1, MutationResultV1, RuntimeKindV1, WorkerV1, session_control::*},
};
use reqwest::{Method, StatusCode};
use sha2::{Digest, Sha256};

const RESPONSE_BYTES: u64 = 128 * 1024;

pub(crate) enum Outcome {
    Accepted(LegacyNamedAdmissionReceiptV1),
    Refused(LegacyNamedAdmissionReceiptV1),
}
impl Outcome {
    fn from_receipt(receipt: LegacyNamedAdmissionReceiptV1) -> Self {
        if receipt.admission.accepted {
            Self::Accepted(receipt)
        } else {
            Self::Refused(receipt)
        }
    }

    fn receipt(&self) -> &LegacyNamedAdmissionReceiptV1 {
        match self {
            Self::Accepted(receipt) | Self::Refused(receipt) => receipt,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AdmissionOperation {
    Reserve,
    Reconcile,
}

/// Live handoff from the Writer's first confirmed Arm CAS, bound to the
/// exact target, Writer, presence and local STOP arbiter. Recovery reads
/// cannot reconstruct this non-cloneable capability.
pub(crate) struct RecordedLegacyNamedImportArm<'enforcer> {
    presence: Arc<PresenceAuthority>,
    enforcer: &'enforcer EnforcerClient,
    writer: WriterClient,
    operation: Uuid,
    write: ImportArmWrite,
    birth: NamedImportArmBirthV1,
    target: ProspectiveRegistrationV1,
    cancel: CancelGeneration,
    expected: u64,
}

pub(super) async fn read_plan(
    presence: &PresenceAuthority,
    expected_broker_profile_id: &str,
    binding_ref: Uuid,
) -> Result<LegacyConfirmedPlanV1> {
    exchange_confirmation(
        presence,
        Method::GET,
        binding_ref,
        "",
        RESPONSE_BYTES,
        None,
        |status, bytes| {
            let value = decode_confirmation(presence, status, bytes, &[StatusCode::OK])?;
            let plan = serde_json::from_value(value).map_err(|_| PresenceError::Protocol)?;
            validate_plan_scope(
                &presence.scope.control_namespace_uuid.to_string(),
                expected_broker_profile_id,
                binding_ref,
                &plan,
            )?;
            Ok(plan)
        },
    )
    .await
}

pub(super) async fn read_launch_spec(
    presence: &PresenceAuthority,
    expected_broker_profile_id: &str,
    binding_ref: Uuid,
) -> Result<LegacyConfirmedLaunchSpecV1> {
    if presence.scope.profile_id != expected_broker_profile_id {
        return Err(PresenceError::Forbidden);
    }
    exchange_confirmation(
        presence,
        Method::GET,
        binding_ref,
        "/launch-spec",
        LEGACY_CONFIRMED_LAUNCH_SPEC_RESPONSE_BYTES as u64,
        None,
        |status, bytes| decode_launch_spec(presence, binding_ref, status, bytes),
    )
    .await
}

pub(super) async fn initial_decision(
    presence: &PresenceAuthority,
    writer: &WriterClient,
    material: &LegacyConfirmedLaunchSpecV1,
    generation: (&CancelGeneration, u64),
) -> Result<LegacyNamedInitialDecisionV1> {
    validate_scope(presence, writer.scope())?;
    let binding_ref = material.plan.binding_ref;
    require_launch_scope(presence, binding_ref, material)?;
    writer
        .prepare_legacy_named_initial_decision(&material.plan)
        .await
        .map_err(named_write_error)?;
    let decision = exchange_confirmation(
        presence,
        Method::POST,
        binding_ref,
        "/initial-decision",
        LEGACY_CONFIRMED_LAUNCH_SPEC_RESPONSE_BYTES as u64,
        Some(generation),
        |status, bytes| decode_initial_decision(presence, material, status, bytes),
    )
    .await?;
    writer
        .acknowledge_legacy_named_initial_decision(&material.plan, &decision)
        .await
        .map_err(named_write_error)?;
    Ok(decision)
}

async fn exchange_confirmation<T>(
    presence: &PresenceAuthority,
    method: Method,
    binding_ref: Uuid,
    suffix: &str,
    success_bytes: u64,
    generation: Option<(&CancelGeneration, u64)>,
    decode: impl FnOnce(StatusCode, &[u8]) -> Result<T>,
) -> Result<T> {
    if binding_ref.is_nil() {
        return Err(PresenceError::Protocol);
    }
    let path = format!("/api/v1/control/session-confirmations/{binding_ref}{suffix}");
    let client = Arc::new(Client::new(presence)?);
    let before = checked_identity(&client, presence).await?;
    presence.revalidate()?;
    let transport = Arc::clone(&client);
    let send = async move {
        transport
            .send_session_bounded(
                method,
                &path,
                None,
                Limits {
                    request: 0,
                    success: success_bytes,
                    error: RESPONSE_BYTES,
                },
            )
            .await
    };
    let (status, bytes) = match generation {
        Some((cancel, expected)) => dispatch_before_stop(cancel, expected, send).await?,
        None => send.await?,
    };
    let confirmed = decode(status, &bytes)?;
    let after = checked_identity(&client, presence).await?;
    if before != after {
        return Err(PresenceError::Conflict);
    }
    presence.revalidate()?;
    Ok(confirmed)
}

pub(super) async fn admit(
    presence: &PresenceAuthority,
    writer: &WriterClient,
    material: &LegacyConfirmedLaunchSpecV1,
    (archive, config): (LegacyNamedWorkspaceArchiveV1, Directory),
    (cancel, expected_generation): (&CancelGeneration, u64),
) -> Result<Outcome> {
    validate_scope(presence, writer.scope())?;
    let plan = &material.plan;
    require_launch_scope(presence, plan.binding_ref, material)?;
    let client = Arc::new(Client::new(presence)?);
    checked_identity(&client, presence).await?;
    let job = writer
        .load(plan.operation_id)
        .await
        .map_err(|_| PresenceError::AdmissionOutcomeUnknown)?
        .ok_or(PresenceError::Conflict)?;
    let bound_launch = job
        .intent()
        .legacy_named_launch()
        .ok_or(PresenceError::Conflict)?;
    if material.source_identity_hash.is_none()
        || canonical(bound_launch).map_err(named_write_error)?
            != canonical(material).map_err(named_write_error)?
        || job.intent().initial_options().run_options().config_dir != config.path()
    {
        return Err(PresenceError::Conflict);
    }
    let retained_material = material.clone();
    let (archive_digest, retained_archive) = tokio::task::spawn_blocking(move || {
        let digest = archive.digest_for_launch(&retained_material)?.to_owned();
        let archive = archive.reopen(
            &config,
            retained_material.plan.operation_id,
            &retained_material,
        )?;
        Ok::<_, anyhow::Error>((digest, archive))
    })
    .await
    .map_err(|_| PresenceError::Unavailable)?
    .map_err(|_| PresenceError::UnsafeFilesystem)?;
    presence.revalidate()?;
    cancel
        .with_current_generation(expected_generation, || ())
        .map_err(|_| PresenceError::Stopped)?;
    let pending = writer
        .record_legacy_named_admission(plan, archive_digest)
        .await
        .map_err(named_write_error)?;
    let request = pending.into_request();
    validate_plan(presence, writer.scope().profile_id(), &request, plan)?;
    let outcome = send_request(
        &client,
        presence,
        &request,
        Dispatch::Reserve {
            cancel,
            expected_generation,
        },
    )
    .await?
    .ok_or(PresenceError::AdmissionOutcomeUnknown)?;
    drop(retained_archive);
    writer
        .acknowledge_legacy_named_admission(plan, &request, outcome.receipt().clone())
        .await
        .map_err(|_| PresenceError::AdmissionOutcomeUnknown)?;
    Ok(outcome)
}

pub(super) async fn reconcile(
    presence: &PresenceAuthority,
    writer: &WriterClient,
    operation: Uuid,
) -> Result<Option<Outcome>> {
    validate_scope(presence, writer.scope())?;
    let retained = writer
        .load_legacy_named_admission(operation)
        .await
        .map_err(|_| PresenceError::AdmissionOutcomeUnknown)?;
    let plan = retained.plan();
    let request = retained.request();
    validate_plan(presence, writer.scope().profile_id(), request, plan)?;
    let client = Arc::new(Client::new(presence)?);
    checked_identity(&client, presence).await?;
    // Use the gateway to reauthorize the read even if the broker already
    // holds an acknowledgement. The Store alone resolves the current grant.
    let outcome = send_request(&client, presence, request, Dispatch::Reconcile).await?;
    if let Some(outcome) = &outcome {
        writer
            .acknowledge_legacy_named_admission(plan, request, outcome.receipt().clone())
            .await
            .map_err(|_| PresenceError::AdmissionOutcomeUnknown)?;
    }
    Ok(outcome)
}

pub(super) async fn record_import_arm<'enforcer>(
    presence: Arc<PresenceAuthority>,
    enforcer: &'enforcer ExecutionEnforcerV1,
    writer: &WriterClient,
    plan: &LegacyConfirmedPlanV1,
    archive_digest: &str,
    target: ProspectiveRegistrationV1,
    (cancel, expected): (&CancelGeneration, u64),
) -> Result<RecordedLegacyNamedImportArm<'enforcer>> {
    let ExecutionEnforcerV1::Legacy(enforcer) = enforcer else {
        return Err(PresenceError::InvalidConfig);
    };
    require_arm_generation(cancel, expected)?;
    presence.revalidate()?;
    let scope = writer.scope();
    validate_scope(&presence, scope)?;
    target.validate().map_err(prospective_error)?;
    if target.tenant_id() != scope.tenant_id()
        || target.control_origin_hash()
            != hex::encode(Sha256::digest(scope.gateway_origin().as_bytes()))
    {
        return Err(PresenceError::Forbidden);
    }
    let operation = plan.operation_id;
    let (retained, _) = open_named_import(writer, operation, &target).await?;
    let owned = retained
        .import_creation()
        .and_then(|creation| creation.owned())
        .ok_or(PresenceError::Conflict)?;
    if retained.plan() != plan
        || retained.archive_digest() != Some(archive_digest)
        || retained.import_arm().is_some()
        || canonical(owned).map_err(import_arm_journal_error)?
            != canonical(target.owned()).map_err(import_arm_journal_error)?
    {
        // Historical pending and acknowledged Arms are reconciliation only.
        return Err(PresenceError::Conflict);
    }
    retained
        .with_import_arm(target.arm().clone())
        .map_err(import_arm_journal_error)?;
    enforcer
        .legacy_require_new_prearm_identity()
        .map_err(prospective_error)?;
    require_arm_generation(cancel, expected)?;
    let recorded = match writer
        .record_legacy_named_import_arm_diagnosed(plan, archive_digest, target.arm().clone())
        .await
    {
        Ok(recorded) => recorded,
        Err(NamedImportArmWriteFailure::BeforePublication(error)) => {
            return Err(import_arm_journal_error(error));
        }
        Err(
            NamedImportArmWriteFailure::PublicationUnconfirmed(_)
            | NamedImportArmWriteFailure::ReloadAfterPublication(_),
        ) => {
            // A durable pending Arm may exist without a confirmed ticket.
            enforcer
                .legacy_fence_unknown_prearm_write()
                .map_err(prospective_error)?;
            return Err(PresenceError::AdmissionOutcomeUnknown);
        }
    };
    require_arm_generation(cancel, expected)?;
    let birth = NamedImportArmBirthV1::from_plan(scope, plan, recorded.birth())
        .map_err(prospective_error)?;
    Ok(RecordedLegacyNamedImportArm {
        presence,
        enforcer,
        writer: writer.clone(),
        operation,
        write: recorded,
        birth,
        target,
        cancel: cancel.clone(),
        expected,
    })
}

pub(super) async fn arm_import(
    presence: &Arc<PresenceAuthority>,
    pending: RecordedLegacyNamedImportArm<'_>,
) -> Result<(ArmResultV1, ProspectiveArmExchange)> {
    let RecordedLegacyNamedImportArm {
        presence: issuer,
        enforcer,
        writer,
        operation,
        write,
        birth,
        target,
        cancel,
        expected,
    } = pending;
    if !Arc::ptr_eq(presence, &issuer) {
        return Err(PresenceError::Forbidden);
    }
    require_arm_generation(&cancel, expected)?;
    let client = Client::new(presence)?;
    checked_identity(&client, presence).await?;
    require_pending_import_arm(&writer, operation, &write, &target).await?;
    enforcer
        .legacy_require_new_prearm_identity()
        .map_err(prospective_error)?;
    require_arm_generation(&cancel, expected)?;
    let reserved = enforcer
        .legacy_reserve_named_prospective(target.clone(), &birth, &cancel, expected)
        .await
        .map_err(named_prospective_error)?;
    let exchange = enforcer
        .legacy_begin_named_prospective_arm_exchange(reserved)
        .await
        .map_err(named_prospective_error)?;
    require_arm_generation(&cancel, expected)?;
    if !exchange.permits_original() {
        return Err(PresenceError::Conflict);
    }
    require_pending_import_arm(&writer, operation, &write, &target).await?;
    let result = send_import_arm(
        &client,
        presence,
        write.request(),
        Some((&cancel, expected)),
    )
    .await?
    .ok_or(PresenceError::AdmissionOutcomeUnknown)?;
    writer
        .acknowledge_legacy_named_import_arm(operation, write.request(), result.clone())
        .await
        .map_err(|_| PresenceError::AdmissionOutcomeUnknown)?;
    // Keep the exact reply even if STOP won during transport. STOP notification
    // belongs to the durable projection, and a stale exchange grants no start.
    require_arm_generation(&cancel, expected)?;
    let exchange = if result.replayed {
        exchange.into_restrictive()
    } else {
        exchange
    };
    Ok((result, exchange))
}

async fn open_named_import(
    writer: &WriterClient,
    operation: Uuid,
    target: &ProspectiveRegistrationV1,
) -> Result<(LegacyNamedAdmissionCheckpoint, RevisionRefV1)> {
    let job = writer
        .load(operation)
        .await
        .map_err(import_arm_journal_error)?
        .ok_or(PresenceError::Conflict)?;
    let checkpoint = job.broker_checkpoint_v3().ok_or(PresenceError::Conflict)?;
    if checkpoint
        .commands()
        .iter()
        .any(|command| matches!(command.request(), LocalCommandRequestV1::CancelRequested))
    {
        return Err(PresenceError::Stopped);
    }
    let retained = checkpoint
        .broker()
        .supervision()
        .legacy_named_admission()
        .filter(|named| job.intent().legacy_named_plan() == Some(named.plan()))
        .cloned()
        .ok_or(PresenceError::Conflict)?;
    retained
        .require_import_creation_binding(target.authority())
        .map_err(import_arm_journal_error)?;
    Ok((retained, job.head().clone()))
}

async fn require_pending_import_arm(
    writer: &WriterClient,
    operation: Uuid,
    write: &ImportArmWrite,
    target: &ProspectiveRegistrationV1,
) -> Result<()> {
    let (retained, head) = open_named_import(writer, operation, target).await?;
    let arm = retained.import_arm().ok_or(PresenceError::Conflict)?;
    let exact = retained
        .import_arm_request()
        .ok_or(PresenceError::Conflict)?;
    // The Writer verified this exact birth under the live job's append-only
    // journal lease. A later head must still carry the same pending Arm.
    let birth = write.birth();
    if head.revision() < birth.revision()
        || (head.revision() == birth.revision() && &head != birth)
        || arm.receipt().is_some()
        || canonical(&exact).map_err(import_arm_journal_error)?
            != canonical(write.request()).map_err(import_arm_journal_error)?
    {
        return Err(PresenceError::Conflict);
    }
    Ok(())
}

fn require_arm_generation(cancel: &CancelGeneration, expected: u64) -> Result<()> {
    cancel
        .with_current_generation(expected, || ())
        .map_err(|_| PresenceError::Stopped)
}

fn prospective_error(error: EnforcerError) -> PresenceError {
    match error {
        EnforcerError::InvalidInput => PresenceError::Protocol,
        EnforcerError::UnsafeFilesystem => PresenceError::UnsafeFilesystem,
        EnforcerError::Unauthorized => PresenceError::Forbidden,
        EnforcerError::Conflict | EnforcerError::AlreadyPrepared => PresenceError::Conflict,
        EnforcerError::Capacity => PresenceError::Capacity,
        _ => PresenceError::UnsafeState,
    }
}

fn named_prospective_error(failure: LegacyNamedProspectiveFailure) -> PresenceError {
    match failure {
        LegacyNamedProspectiveFailure::OutcomeUnknown(_) => PresenceError::AdmissionOutcomeUnknown,
        LegacyNamedProspectiveFailure::StaleAcknowledged(EnforcerError::NotReady) => {
            PresenceError::Stopped
        }
        LegacyNamedProspectiveFailure::BeforeSend(code)
        | LegacyNamedProspectiveFailure::StaleAcknowledged(code) => prospective_error(code),
        LegacyNamedProspectiveFailure::Refused(refusal) => prospective_error(refusal.code()),
    }
}

fn import_arm_journal_error(error: JournalError) -> PresenceError {
    match error {
        JournalError::Conflict => PresenceError::Conflict,
        JournalError::Busy => PresenceError::Unavailable,
        JournalError::LimitExceeded => PresenceError::Capacity,
        _ => PresenceError::UnsafeState,
    }
}

pub(super) async fn reconcile_import_arm(
    presence: &PresenceAuthority,
    writer: &WriterClient,
    operation: Uuid,
) -> Result<Option<ArmResultV1>> {
    validate_scope(presence, writer.scope())?;
    let Some((request, _)) = writer
        .load_legacy_named_import_arm(operation)
        .await
        .map_err(import_arm_journal_error)?
    else {
        return Ok(None);
    };
    let client = Client::new(presence)?;
    checked_identity(&client, presence).await?;
    let result = send_import_arm(&client, presence, &request, None).await?;
    if let Some(result) = &result {
        writer
            .acknowledge_legacy_named_import_arm(operation, &request, result.clone())
            .await
            .map_err(|_| PresenceError::AdmissionOutcomeUnknown)?;
    }
    Ok(result)
}

async fn send_import_arm(
    client: &Client,
    presence: &PresenceAuthority,
    request: &LegacyNamedImportArmRequestV1,
    generation: Option<(&CancelGeneration, u64)>,
) -> Result<Option<ArmResultV1>> {
    request.validate().map_err(|_| PresenceError::Protocol)?;
    if request.namespace != presence.scope.control_namespace_uuid.to_string() {
        return Err(PresenceError::Forbidden);
    }
    presence.revalidate()?;
    let value = serde_json::to_value(request).map_err(|_| PresenceError::Protocol)?;
    presence.screen(&value)?;
    let body = serde_json::to_vec(request).map_err(|_| PresenceError::Protocol)?;
    let suffix = if generation.is_some() {
        ""
    } else {
        "/reconcile"
    };
    let path = format!(
        "/api/v1/control/session-confirmations/{}/admission/import-arm{suffix}",
        request.binding_ref
    );
    let send = async {
        client
            .send_session_bounded(
                Method::POST,
                &path,
                Some(body),
                Limits {
                    request: LEGACY_NAMED_ADMISSION_REQUEST_BYTES as u64,
                    success: LEGACY_NAMED_IMPORT_ARM_RESPONSE_BYTES as u64,
                    error: RESPONSE_BYTES,
                },
            )
            .await
            .map_err(|_| PresenceError::AdmissionOutcomeUnknown)
    };
    let (status, bytes) = match generation {
        Some((cancel, expected)) => dispatch_before_stop(cancel, expected, send).await?,
        None => send.await?,
    };
    let value = crate::json::decode_unique(&bytes, 64)
        .map_err(|_| PresenceError::AdmissionOutcomeUnknown)?;
    if generation.is_none()
        && status == StatusCode::NOT_FOUND
        && value["error"]["code"] == LEGACY_NAMED_IMPORT_ARM_RECEIPT_NOT_FOUND
    {
        return Ok(None);
    }
    if !matches!(status, StatusCode::OK | StatusCode::CREATED) {
        return Err(PresenceError::AdmissionOutcomeUnknown);
    }
    presence
        .screen(&value)
        .map_err(|_| PresenceError::AdmissionOutcomeUnknown)?;
    let result: ArmResultV1 =
        serde_json::from_value(value).map_err(|_| PresenceError::AdmissionOutcomeUnknown)?;
    if (generation.is_none() && !result.replayed) || result.replayed != (status == StatusCode::OK) {
        return Err(PresenceError::AdmissionOutcomeUnknown);
    }
    presence
        .revalidate()
        .map_err(|_| PresenceError::AdmissionOutcomeUnknown)?;
    Ok(Some(result))
}

enum ImportClaimDispatch<'a> {
    #[allow(
        dead_code,
        reason = "the live Service ticket consumer is composed separately"
    )]
    Submit {
        registration: &'a RegistrationV1,
        generation: (&'a CancelGeneration, u64),
    },
    Reconcile,
}

/// Kept inside Presence for the live Service ticket. The caller must retain
/// the exact Writer receipt before handing off a fresh disposition.
pub(super) async fn submit_import_claim(
    presence: &PresenceAuthority,
    request: &LegacyNamedImportClaimRequestV1,
    registration: &RegistrationV1,
    generation: (&CancelGeneration, u64),
) -> Result<HelperClaimResultV1> {
    exchange_import_claim(
        presence,
        request,
        ImportClaimDispatch::Submit {
            registration,
            generation,
        },
    )
    .await?
    .ok_or(PresenceError::AdmissionOutcomeUnknown)
}

pub(super) async fn reconcile_import_claim(
    presence: &PresenceAuthority,
    writer: &WriterClient,
    operation: Uuid,
) -> Result<Option<MutationReceiptV1>> {
    validate_scope(presence, writer.scope())?;
    let retained = writer
        .load_legacy_named_admission(operation)
        .await
        .map_err(import_arm_journal_error)?;
    let request = retained
        .import_claim()
        .ok_or(PresenceError::Conflict)?
        .request();
    validate_plan_scope(
        &presence.scope.control_namespace_uuid.to_string(),
        writer.scope().profile_id(),
        request.binding_ref,
        retained.plan(),
    )?;
    let Some(result) =
        exchange_import_claim(presence, request, ImportClaimDispatch::Reconcile).await?
    else {
        return Ok(None);
    };
    writer
        .acknowledge_legacy_named_import_claim(operation, request, result.receipt.clone())
        .await
        .map_err(|_| PresenceError::AdmissionOutcomeUnknown)?;
    Ok(Some(result.receipt))
}

async fn exchange_import_claim(
    presence: &PresenceAuthority,
    request: &LegacyNamedImportClaimRequestV1,
    dispatch: ImportClaimDispatch<'_>,
) -> Result<Option<HelperClaimResultV1>> {
    request.validate().map_err(|_| PresenceError::Protocol)?;
    let namespace = presence.scope.control_namespace_uuid.to_string();
    if request.namespace != namespace {
        return Err(PresenceError::Forbidden);
    }
    if let ImportClaimDispatch::Submit {
        registration,
        generation: (cancel, expected),
    } = &dispatch
    {
        require_arm_generation(cancel, *expected)?;
        if registration.tenant_id() != presence.scope.expected_tenant_id
            || registration.control_origin_hash()
                != hex::encode(Sha256::digest(presence.scope.gateway.as_bytes()))
        {
            return Err(PresenceError::Forbidden);
        }
        let arm = registration.arm();
        let attempt = canonical_uuid(&arm.attempt_id).map_err(|_| PresenceError::Conflict)?;
        if arm.operation_id
            != LegacyNamedImportArmRequestV1::operation_id(request.binding_ref, attempt)
        {
            return Err(PresenceError::Conflict);
        }
        request
            .validate_for_arm(request.binding_ref, &namespace, arm, registration.receipt())
            .map_err(|_| PresenceError::Conflict)?;
    }
    let value = serde_json::to_value(request).map_err(|_| PresenceError::Protocol)?;
    presence.screen(&value)?;
    let body = serde_json::to_vec(request).map_err(|_| PresenceError::Protocol)?;
    if body.len() > LEGACY_NAMED_ADMISSION_REQUEST_BYTES {
        return Err(PresenceError::Protocol);
    }
    let reconcile = matches!(&dispatch, ImportClaimDispatch::Reconcile);
    let suffix = if reconcile { "/reconcile" } else { "" };
    let path = format!(
        "/api/v1/control/session-confirmations/{}/admission/import-claim{suffix}",
        request.binding_ref
    );
    presence.revalidate()?;
    let client = Client::new(presence)?;
    let before = checked_identity(&client, presence).await?;
    presence.revalidate()?;
    let send = async {
        client
            .send_session_bounded(
                Method::POST,
                &path,
                Some(body),
                Limits {
                    request: LEGACY_NAMED_ADMISSION_REQUEST_BYTES as u64,
                    success: IMPORT_CLAIM_RESPONSE_BYTES,
                    error: IMPORT_CLAIM_RESPONSE_BYTES,
                },
            )
            .await
            .map_err(|_| PresenceError::AdmissionOutcomeUnknown)
    };
    let (status, bytes) = match dispatch {
        ImportClaimDispatch::Submit {
            registration,
            generation: (cancel, expected),
        } => {
            let clock = NativeHostClock::new().map_err(|_| PresenceError::Clock)?;
            let recorded_ns = request
                .claim
                .clock_check
                .continuous_ns()
                .map_err(|_| PresenceError::Clock)?;
            dispatch_before_stop(cancel, expected, async {
                // The generation guard covers this fresh host reading and
                // the transport's first poll. The Writer's claim bytes stay exact.
                let now = clock.sample().map_err(|_| PresenceError::Clock)?;
                if now.continuous_ns().map_err(|_| PresenceError::Clock)? < recorded_ns {
                    return Err(PresenceError::Clock);
                }
                registration
                    .arm()
                    .validate_clock_check(&now)
                    .map_err(|_| PresenceError::Clock)?;
                send.await
            })
            .await?
        }
        ImportClaimDispatch::Reconcile => send.await?,
    };
    let value = crate::json::decode_unique(&bytes, 64)
        .map_err(|_| PresenceError::AdmissionOutcomeUnknown)?;
    presence
        .screen(&value)
        .map_err(|_| PresenceError::AdmissionOutcomeUnknown)?;
    let result = decode_import_claim(request, reconcile, status, value)?;
    let after = checked_identity(&client, presence)
        .await
        .map_err(|_| PresenceError::AdmissionOutcomeUnknown)?;
    if before != after {
        return Err(PresenceError::AdmissionOutcomeUnknown);
    }
    presence
        .revalidate()
        .map_err(|_| PresenceError::AdmissionOutcomeUnknown)?;
    Ok(result)
}

fn decode_import_claim(
    request: &LegacyNamedImportClaimRequestV1,
    reconcile: bool,
    status: StatusCode,
    value: Value,
) -> Result<Option<HelperClaimResultV1>> {
    if reconcile
        && status == StatusCode::NOT_FOUND
        && value["error"]["code"] == IMPORT_CLAIM_RECEIPT_NOT_FOUND
    {
        // This read observed no receipt. A previously dispatched claim may
        // still commit, so the retained request stays pending.
        return Ok(None);
    }
    let result: HelperClaimResultV1 =
        serde_json::from_value(value).map_err(|_| PresenceError::AdmissionOutcomeUnknown)?;
    let disposition_matches = match result.disposition {
        ClaimDispositionV1::EmitOnce => !reconcile && status == StatusCode::CREATED,
        ClaimDispositionV1::AlreadyConsumed => status == StatusCode::OK,
    };
    if !disposition_matches || request.validate_receipt(&result.receipt).is_err() {
        return Err(PresenceError::AdmissionOutcomeUnknown);
    }
    Ok(Some(result))
}

pub(super) async fn cancel_reserved(
    presence: &Arc<PresenceAuthority>,
    stop: &Arc<CancelStopProjectionV1>,
    io: &Arc<Semaphore>,
) -> Result<()> {
    validate_scope(presence, stop.scope())?;
    let reservation = stop.named_reservation().ok_or(PresenceError::Conflict)?;
    let admission = reservation.admission();
    let client = Arc::new(Client::new(presence)?);
    checked_identity(&client, presence).await?;
    let receipt = match admission.receipt() {
        Some(acknowledgement) => acknowledgement.clone(),
        None => {
            let request = admission.request();
            validate_plan(
                presence,
                stop.scope().profile_id(),
                request,
                admission.plan(),
            )?;
            // The reserve may still be in flight. A missing receipt from this
            // read cannot be promoted to proof that there is no allocation.
            send_request(&client, presence, request, Dispatch::Reconcile)
                .await?
                .ok_or(PresenceError::AdmissionOutcomeUnknown)?
                .receipt()
                .clone()
        }
    };
    let cancellation = reservation
        .cancellation(receipt)
        .map_err(named_write_error)?;
    revalidate_cancel(presence, stop, io).await?;
    let Some((request, reserved)) = cancellation else {
        return Ok(());
    };
    let payload = serde_json::to_value(&request).map_err(|_| PresenceError::Protocol)?;
    presence.screen(&payload)?;
    let body = serde_json::to_vec(&request).map_err(|_| PresenceError::Protocol)?;
    presence.revalidate()?;
    let (status, bytes) = client
        .send_session_bounded(
            Method::POST,
            &format!(
                "/api/v1/control/session-confirmations/{}/admission/cancel-reserved",
                request.binding_ref
            ),
            Some(body),
            Limits {
                request: LEGACY_NAMED_ADMISSION_REQUEST_BYTES as u64,
                success: RESPONSE_BYTES,
                error: RESPONSE_BYTES,
            },
        )
        .await
        .map_err(|_| PresenceError::AdmissionOutcomeUnknown)?;
    if status != StatusCode::OK {
        return Err(PresenceError::AdmissionOutcomeUnknown);
    }
    let value = crate::json::decode_unique(&bytes, 64)
        .map_err(|_| PresenceError::AdmissionOutcomeUnknown)?;
    presence
        .screen(&value)
        .map_err(|_| PresenceError::AdmissionOutcomeUnknown)?;
    let result =
        serde_json::from_value(value).map_err(|_| PresenceError::AdmissionOutcomeUnknown)?;
    validate_cancel_receipt(&request, &reserved, &result)?;
    revalidate_cancel(presence, stop, io).await?;
    Ok(())
}

async fn revalidate_cancel(
    presence: &Arc<PresenceAuthority>,
    stop: &Arc<CancelStopProjectionV1>,
    io: &Arc<Semaphore>,
) -> Result<()> {
    let permit = Arc::clone(io)
        .acquire_owned()
        .await
        .map_err(|_| PresenceError::Unavailable)?;
    let presence = Arc::clone(presence);
    let stop = Arc::clone(stop);
    // The retained STOP reader may fsync. Keep its lease and the service's FS
    // budget until the blocking check completes, even if the request is dropped.
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        stop.revalidate().map_err(named_write_error)?;
        presence.revalidate()
    })
    .await
    .map_err(|_| PresenceError::Unavailable)?
}

fn validate_cancel_receipt(
    request: &LegacyNamedCancelReservationV1,
    reserved: &WorkerV1,
    result: &MutationResultV1,
) -> Result<()> {
    let expected = crate::audit::expected_worker_transition(reserved, &request.mutation)
        .map_err(|_| PresenceError::Protocol)?;
    let receipt = &result.receipt;
    let input_hash = request.input_hash().map_err(|_| PresenceError::Protocol)?;
    if result.disposition.is_some()
        || receipt.operation_id != request.mutation.operation_id
        || receipt.worker_id.as_deref() != Some(expected.worker_id.as_str())
        || receipt.revision != Some(expected.revision)
        || receipt.input_hash != input_hash
        || receipt.audit_seq <= request.admission.audit_seq
        || crate::control::hash(&receipt.audit_hash).is_err()
        || canonical(&expected).map_err(named_write_error)?
            != canonical(&result.worker).map_err(named_write_error)?
    {
        return Err(PresenceError::AdmissionOutcomeUnknown);
    }
    Ok(())
}

fn validate_scope(presence: &PresenceAuthority, scope: &JournalScopeV1) -> Result<()> {
    if scope.installation_id() != presence.scope.installation_id
        || scope.tenant_id() != presence.scope.expected_tenant_id
        || scope.gateway_origin() != presence.scope.gateway
        || scope.profile_id() != presence.scope.profile_id
        || scope.control_namespace_uuid() != presence.scope.control_namespace_uuid
    {
        return Err(PresenceError::Forbidden);
    }
    Ok(())
}

fn named_write_error(error: JournalError) -> PresenceError {
    match error {
        JournalError::Interrupted => PresenceError::Stopped,
        JournalError::Conflict => PresenceError::Conflict,
        _ => PresenceError::AdmissionOutcomeUnknown,
    }
}

async fn dispatch_before_stop<T>(
    cancel: &CancelGeneration,
    expected_generation: u64,
    send: impl Future<Output = Result<T>>,
) -> Result<T> {
    use std::task::Poll;
    tokio::pin!(send);
    // Order the transport's first poll with STOP. The caller prepares all
    // retained data beforehand. No wait or detached task runs under the gate.
    let first = std::future::poll_fn(|context| {
        Poll::Ready(
            cancel.with_current_generation(expected_generation, || send.as_mut().poll(context)),
        )
    })
    .await
    .map_err(|_| PresenceError::Stopped)?;
    match first {
        Poll::Ready(result) => result,
        // Once dispatched the request may have committed. Settle its exact
        // acknowledgement even if STOP wins while the response is pending.
        Poll::Pending => send.await,
    }
}

enum Dispatch<'a> {
    Reserve {
        cancel: &'a CancelGeneration,
        expected_generation: u64,
    },
    Reconcile,
}

async fn send_request(
    client: &Arc<Client>,
    presence: &PresenceAuthority,
    request: &LegacyNamedSupervisedAdmissionV1,
    dispatch: Dispatch<'_>,
) -> Result<Option<Outcome>> {
    presence.revalidate()?;
    let value = serde_json::to_value(request).map_err(|_| PresenceError::Protocol)?;
    presence.screen(&value)?;
    let body = serde_json::to_vec(request).map_err(|_| PresenceError::Protocol)?;
    if body.len() > LEGACY_NAMED_ADMISSION_REQUEST_BYTES {
        return Err(PresenceError::Protocol);
    }
    let (operation, action) = match dispatch {
        Dispatch::Reserve { .. } => (AdmissionOperation::Reserve, "admission"),
        Dispatch::Reconcile => (AdmissionOperation::Reconcile, "admission/reconcile"),
    };
    let binding = request.binding_ref;
    let path = format!("/api/v1/control/session-confirmations/{binding}/{action}");
    let client = Arc::clone(client);
    let send = async move {
        client
            .send_session_bounded(
                Method::POST,
                &path,
                Some(body),
                Limits {
                    request: LEGACY_NAMED_ADMISSION_REQUEST_BYTES as u64,
                    success: RESPONSE_BYTES,
                    error: RESPONSE_BYTES,
                },
            )
            .await
            .map_err(|_| PresenceError::AdmissionOutcomeUnknown)
    };
    let response = match dispatch {
        Dispatch::Reserve {
            cancel,
            expected_generation,
        } => dispatch_before_stop(cancel, expected_generation, send).await?,
        Dispatch::Reconcile => send.await?,
    };
    let (status, bytes) = response;
    let value = crate::json::decode_unique(&bytes, 64)
        .map_err(|_| PresenceError::AdmissionOutcomeUnknown)?;
    let Some(receipt) = decode_receipt(status, operation, value.clone())? else {
        return Ok(None);
    };
    presence
        .screen(&value)
        .map_err(|_| PresenceError::AdmissionOutcomeUnknown)?;
    validate_receipt(
        presence.scope.control_namespace_uuid,
        request,
        &receipt,
        status,
    )
    .map_err(|_| PresenceError::AdmissionOutcomeUnknown)?;
    presence
        .revalidate()
        .map_err(|_| PresenceError::AdmissionOutcomeUnknown)?;
    Ok(Some(Outcome::from_receipt(receipt)))
}

fn decode_receipt(
    status: StatusCode,
    operation: AdmissionOperation,
    value: Value,
) -> Result<Option<LegacyNamedAdmissionReceiptV1>> {
    if operation == AdmissionOperation::Reconcile
        && status == StatusCode::NOT_FOUND
        && value["error"]["code"] == LEGACY_NAMED_ADMISSION_RECEIPT_NOT_FOUND
    {
        // Absence was observed at this read. A separate request may still commit.
        return Ok(None);
    }
    // An error response cannot settle an operation that may already have committed.
    // Its transient prose is never returned, persisted or used as evidence.
    let receipt: LegacyNamedAdmissionReceiptV1 =
        serde_json::from_value(value).map_err(|_| PresenceError::AdmissionOutcomeUnknown)?;
    if operation == AdmissionOperation::Reconcile && !receipt.admission.replayed {
        return Err(PresenceError::AdmissionOutcomeUnknown);
    }
    Ok(Some(receipt))
}

fn decode_confirmation(
    presence: &PresenceAuthority,
    status: StatusCode,
    bytes: &[u8],
    expected_statuses: &[StatusCode],
) -> Result<Value> {
    let value = crate::json::decode_unique(bytes, 64).map_err(|_| PresenceError::Protocol)?;
    if !expected_statuses.contains(&status) {
        return Err(super::http::remote_error(status, &value));
    }
    presence.screen(&value)?;
    Ok(value)
}

fn require_unmasked_document(presence: &PresenceAuthority, value: &Value) -> Result<()> {
    if Redactor::new(presence.secrets.clone()).value_unclipped(value) != *value {
        return Err(PresenceError::Protocol);
    }
    Ok(())
}

fn decode_launch_spec(
    presence: &PresenceAuthority,
    binding_ref: Uuid,
    status: StatusCode,
    bytes: &[u8],
) -> Result<LegacyConfirmedLaunchSpecV1> {
    let value = decode_confirmation(presence, status, bytes, &[StatusCode::OK])?;
    require_unmasked_document(presence, &value)?;
    let material: LegacyConfirmedLaunchSpecV1 =
        serde_json::from_value(value).map_err(|_| PresenceError::Protocol)?;
    require_launch_scope(presence, binding_ref, &material)?;
    Ok(material)
}

fn require_launch_scope(
    presence: &PresenceAuthority,
    binding_ref: Uuid,
    material: &LegacyConfirmedLaunchSpecV1,
) -> Result<()> {
    validate_plan_scope(
        &presence.scope.control_namespace_uuid.to_string(),
        &presence.scope.profile_id,
        binding_ref,
        &material.plan,
    )?;
    material.validate().map_err(|_| PresenceError::Protocol)?;
    let runtime = match material.launch.runtime_kind {
        RuntimeKindV1::Docker => BrokerRuntimeKindV1::Docker,
        RuntimeKindV1::Podman => BrokerRuntimeKindV1::Podman,
    };
    if material.launch.authority_id != presence.config.authority_id
        || runtime != presence.scope.runtime_kind
    {
        return Err(PresenceError::Forbidden);
    }
    Ok(())
}

fn decode_initial_decision(
    presence: &PresenceAuthority,
    material: &LegacyConfirmedLaunchSpecV1,
    status: StatusCode,
    bytes: &[u8],
) -> Result<LegacyNamedInitialDecisionV1> {
    let value = decode_confirmation(
        presence,
        status,
        bytes,
        &[StatusCode::OK, StatusCode::CREATED],
    )?;
    require_unmasked_document(presence, &value)?;
    // Decision is shared by older routes. Here all response fields are closed,
    // including fields the historical Decision deserializer would ignore.
    let mut ignored = false;
    let result: LegacyNamedInitialDecisionV1 =
        serde_ignored::deserialize(value, |_| ignored = true)
            .map_err(|_| PresenceError::Protocol)?;
    result.validate().map_err(|_| PresenceError::Protocol)?;
    let expected = material
        .plan
        .initial_decision_request(&material.launch)
        .map_err(|_| PresenceError::Protocol)?;
    if ignored
        || result.replayed != (status == StatusCode::OK)
        || result.binding_ref != material.plan.binding_ref
        || result.receipt.request_hash != crate::policy::request_hash(&expected)
    {
        return Err(PresenceError::Protocol);
    }
    Ok(result)
}

async fn checked_identity(
    client: &Client,
    presence: &PresenceAuthority,
) -> Result<crate::broker_registry::BrokerIdentityViewV1> {
    let identity = client.identity(presence).await?;
    let view = client.view(presence).await?;
    super::actor::validate_handshake(presence, &identity, &view)?;
    Ok(identity)
}

fn validate_plan(
    presence: &PresenceAuthority,
    expected_broker_profile_id: &str,
    request: &LegacyNamedSupervisedAdmissionV1,
    plan: &LegacyConfirmedPlanV1,
) -> Result<()> {
    validate_plan_scope(
        &presence.scope.control_namespace_uuid.to_string(),
        expected_broker_profile_id,
        request.binding_ref,
        plan,
    )?;
    if request.admission.admission.receipt.phase != crate::control::DecisionPhaseV1::Initial {
        return Err(PresenceError::Forbidden);
    }
    request
        .require_plan(plan)
        .map_err(|_| PresenceError::Protocol)
}

fn validate_plan_scope(
    expected_namespace: &str,
    expected_broker_profile_id: &str,
    binding_ref: uuid::Uuid,
    plan: &LegacyConfirmedPlanV1,
) -> Result<()> {
    if plan.validate().is_err()
        || plan.binding_ref != binding_ref
        || plan.control_namespace_uuid != expected_namespace
        || plan.broker_profile_id != expected_broker_profile_id
    {
        return Err(PresenceError::Forbidden);
    }
    Ok(())
}

fn validate_receipt(
    namespace: uuid::Uuid,
    request: &LegacyNamedSupervisedAdmissionV1,
    receipt: &LegacyNamedAdmissionReceiptV1,
    status: StatusCode,
) -> Result<()> {
    if status != response_status(&receipt.admission) {
        return Err(PresenceError::Protocol);
    }
    crate::broker::checkpoint::validate_legacy_named_admission_receipt(namespace, request, receipt)
        .map_err(|_| PresenceError::Protocol)
}

fn response_status(admission: &crate::control::AdmissionResultV1) -> StatusCode {
    match admission.refusal {
        Some(AdmissionRefusalV1::Policy) => StatusCode::FORBIDDEN,
        Some(AdmissionRefusalV1::Quota) => StatusCode::TOO_MANY_REQUESTS,
        None if admission.replayed => StatusCode::OK,
        None => StatusCode::CREATED,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stopped_generation_never_dispatches_the_named_admission_post() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let cancel = CancelGeneration::new();
        let expected = cancel.current();
        cancel.invalidate();
        let submitted = Arc::new(AtomicBool::new(false));
        let witness = Arc::clone(&submitted);
        let blocked = dispatch_before_stop(&cancel, expected, async move {
            witness.store(true, Ordering::SeqCst);
            Ok(())
        })
        .await;
        assert!(matches!(blocked, Err(PresenceError::Stopped)));
        assert!(!submitted.load(Ordering::SeqCst));

        for outcome in [Ok(1u8), Err(PresenceError::AdmissionOutcomeUnknown)] {
            let cancel = CancelGeneration::new();
            let (started, wait_started) = tokio::sync::oneshot::channel();
            let (reply, wait_reply) = tokio::sync::oneshot::channel();
            let send = dispatch_before_stop(&cancel, cancel.current(), async {
                started.send(()).unwrap();
                wait_reply.await.unwrap()
            });
            let stop_then_reply = async {
                wait_started.await.unwrap();
                cancel.invalidate();
                reply.send(outcome).unwrap();
            };
            let (settled, ()) = tokio::join!(send, stop_then_reply);
            assert_eq!(settled, outcome);
        }
    }

    #[test]
    fn named_import_distinguishes_writer_interruption_from_stop() {
        let interrupted = import_arm_journal_error(JournalError::Interrupted);
        assert_eq!(interrupted, PresenceError::UnsafeState);
        assert!(!interrupted.fences_admission());
        let saturated = import_arm_journal_error(JournalError::Busy);
        assert_eq!(saturated, PresenceError::Unavailable);
        assert!(!saturated.fences_admission());
        assert_eq!(
            named_prospective_error(LegacyNamedProspectiveFailure::StaleAcknowledged(
                EnforcerError::NotReady,
            )),
            PresenceError::Stopped,
        );
        assert_eq!(
            named_prospective_error(LegacyNamedProspectiveFailure::OutcomeUnknown(
                EnforcerError::NotReady,
            )),
            PresenceError::AdmissionOutcomeUnknown,
        );
    }
    use crate::broker::presence::preparation_tests::Fixture;

    fn plan() -> LegacyConfirmedPlanV1 {
        LegacyConfirmedPlanV1 {
            version: 1,
            binding_ref: uuid::Uuid::new_v4(),
            control_namespace_uuid: uuid::Uuid::new_v4().to_string(),
            operation_id: uuid::Uuid::new_v4(),
            session_id: uuid::Uuid::new_v4(),
            launch_target_id: uuid::Uuid::new_v4(),
            target_revision: 1,
            broker_profile_id: "broker-profile-a".into(),
            launch_spec_hash: "a".repeat(64),
            expires_at: chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap(),
        }
    }

    #[test]
    fn plan_scope_rejects_another_profile_namespace_or_binding() {
        let plan = plan();
        assert!(
            validate_plan_scope(
                &plan.control_namespace_uuid,
                &plan.broker_profile_id,
                plan.binding_ref,
                &plan,
            )
            .is_ok()
        );
        assert_eq!(
            validate_plan_scope(
                &plan.control_namespace_uuid,
                "broker-profile-b",
                plan.binding_ref,
                &plan,
            )
            .unwrap_err(),
            PresenceError::Forbidden
        );
        assert_eq!(
            validate_plan_scope(
                &uuid::Uuid::new_v4().to_string(),
                &plan.broker_profile_id,
                plan.binding_ref,
                &plan,
            )
            .unwrap_err(),
            PresenceError::Forbidden
        );
        assert_eq!(
            validate_plan_scope(
                &plan.control_namespace_uuid,
                &plan.broker_profile_id,
                uuid::Uuid::new_v4(),
                &plan,
            )
            .unwrap_err(),
            PresenceError::Forbidden
        );
    }

    fn launch_material() -> (Fixture, PreparedPresence, LegacyConfirmedLaunchSpecV1) {
        use crate::{
            control::{LaunchSpecV1, NetworkV1, RestartPolicyV1, RuntimeKindV1, SecurityPresetV1},
            model::CommandSpec,
        };

        let mut fixture = Fixture::new();
        let authority = Uuid::new_v4().to_string();
        fixture.value["authority_id"] = serde_json::json!(authority);
        fixture.write_config();
        let scope = fixture.scope();
        let presence = PreparedPresence::load(&fixture.config, scope.clone(), vec![]).unwrap();
        let mut plan = plan();
        plan.broker_profile_id = scope.profile_id;
        plan.control_namespace_uuid = scope.control_namespace_uuid.to_string();
        let launch = LaunchSpecV1 {
            version: 1,
            session_id: plan.session_id.to_string(),
            command: CommandSpec {
                argv: vec!["/usr/bin/id".into(), "-u".into()],
                cwd: "/workspace/demo".into(),
            },
            profile: "observe".into(),
            runtime_kind: RuntimeKindV1::Docker,
            authority_id: authority,
            image_ref: "worker:fixture".into(),
            image_id: format!("sha256:{}", "b".repeat(64)),
            network: NetworkV1::None,
            cpu_millis: 2000,
            memory_bytes: 1_073_741_824,
            pids_limit: 256,
            timeout_seconds: 60,
            security_preset: SecurityPresetV1::FixedSandboxV2,
            restart_policy: RestartPolicyV1::No,
            workspace_manifest_digest: "c".repeat(64),
            workspace_import_policy_version: 1,
            provider: "generic".into(),
            capture_output: false,
            credential_binding_id: None,
            credential_names: vec![],
            provider_resolution: None,
            egress: None,
        };
        plan.launch_spec_hash = launch.spec_hash().unwrap();
        let material = LegacyConfirmedLaunchSpecV1 {
            version: 1,
            plan,
            launch,
            source_identity_hash: None,
        };
        (fixture, presence, material)
    }

    #[test]
    fn launch_spec_decoder_requires_exact_private_material() {
        let (_fixture, presence, mut material) = launch_material();
        let binding_ref = material.plan.binding_ref;
        let decode = |body: &[u8]| {
            decode_launch_spec(&presence.authority, binding_ref, StatusCode::OK, body)
        };
        let body = serde_json::to_string(&material).unwrap();
        let decoded = decode(body.as_bytes()).unwrap();
        assert_eq!(decoded.plan, material.plan);
        assert_eq!(decoded.launch.command.cwd, "/workspace/demo");

        let duplicate = format!(r#"{{"version":1,{}"#, body.strip_prefix('{').unwrap());
        assert!(matches!(
            decode(duplicate.as_bytes()),
            Err(PresenceError::Protocol)
        ));
        let mut unrecognized: Value = serde_json::from_str(&body).unwrap();
        unrecognized["launch"]["host_source"] = serde_json::json!("/fixture/host-project");
        assert!(matches!(
            decode(&serde_json::to_vec(&unrecognized).unwrap()),
            Err(PresenceError::Protocol)
        ));
        material.launch.timeout_seconds += 1;
        assert!(matches!(
            decode(&serde_json::to_vec(&material).unwrap()),
            Err(PresenceError::Protocol)
        ));

        material.launch.authority_id = Uuid::new_v4().to_string();
        material.plan.launch_spec_hash = material.launch.spec_hash().unwrap();
        assert!(matches!(
            decode(&serde_json::to_vec(&material).unwrap()),
            Err(PresenceError::Forbidden)
        ));
        material.launch.authority_id = presence.authority.config.authority_id.clone();
        material
            .launch
            .command
            .argv
            .extend(["--token".to_owned(), "synthetic-fixture-value".to_owned()]);
        material.plan.launch_spec_hash = material.launch.spec_hash().unwrap();
        assert!(matches!(
            decode(&serde_json::to_vec(&material).unwrap()),
            Err(PresenceError::Protocol)
        ));
    }

    #[test]
    fn initial_decision_decoder_binds_status_request_and_receipt() {
        use crate::{
            control::{DecisionPhaseV1, DecisionReceiptV1},
            model::{Decision, DecisionRequest, Outcome as PolicyOutcome},
        };

        let (_fixture, presence, material) = launch_material();
        let binding_ref = material.plan.binding_ref;
        let request = DecisionRequest {
            request_id: LegacyNamedInitialDecisionV1::request_id(binding_ref),
            profile: material.launch.profile.clone(),
            command: material.launch.command.clone(),
            project_id: None,
            sandbox_id: None,
            agent_id: None,
            session_id: Some(material.launch.session_id.clone()),
            correlation_id: Some(binding_ref.to_string()),
            trace_id: None,
            human_approved: false,
            approval_hash: None,
            execution_mode: None,
            provider_resolution: None,
        };
        let mut response = LegacyNamedInitialDecisionV1 {
            version: 1,
            binding_ref,
            receipt: DecisionReceiptV1 {
                version: 1,
                request_id: request.request_id.clone(),
                request_hash: crate::policy::request_hash(&request),
                phase: DecisionPhaseV1::Initial,
                audit_seq: 17,
                audit_hash: "a".repeat(64),
            },
            request,
            decision: Decision::new(PolicyOutcome::Deny, "fixture", "The policy refused."),
            replayed: false,
        };
        let decode = |status, bytes: &[u8]| {
            decode_initial_decision(&presence.authority, &material, status, bytes)
        };
        let fresh = serde_json::to_vec(&response).unwrap();
        let recorded = decode(StatusCode::CREATED, &fresh).unwrap();
        assert_eq!(recorded.decision.outcome, PolicyOutcome::Deny);
        assert!(!recorded.replayed);
        assert!(matches!(
            decode(StatusCode::OK, &fresh),
            Err(PresenceError::Protocol)
        ));

        response.replayed = true;
        let replay = serde_json::to_vec(&response).unwrap();
        let retained = decode(StatusCode::OK, &replay).unwrap();
        assert!(retained.replayed);
        assert_eq!(retained.decision.outcome, PolicyOutcome::Deny);
        assert_eq!(
            serde_json::to_value(recorded.receipt).unwrap(),
            serde_json::to_value(retained.receipt).unwrap()
        );
        response.decision = Decision::new(PolicyOutcome::Review, "fixture", "Review is required.");
        response.receipt.audit_seq += 1;
        let review = decode(StatusCode::OK, &serde_json::to_vec(&response).unwrap()).unwrap();
        assert_eq!(review.decision.outcome, PolicyOutcome::Review);

        let mut unknown: Value = serde_json::from_slice(&replay).unwrap();
        unknown["decision"]["runtime_allowed"] = Value::Bool(false);
        assert!(matches!(
            decode(StatusCode::OK, &serde_json::to_vec(&unknown).unwrap()),
            Err(PresenceError::Protocol)
        ));
        response.request.command.argv.push("--version".into());
        response.receipt.request_hash = crate::policy::request_hash(&response.request);
        assert!(matches!(
            decode(StatusCode::OK, &serde_json::to_vec(&response).unwrap()),
            Err(PresenceError::Protocol)
        ));

        response.request.command = material.launch.command.clone();
        response.request.request_id = Uuid::new_v4().to_string();
        response.receipt.request_id = response.request.request_id.clone();
        response.receipt.request_hash = crate::policy::request_hash(&response.request);
        assert!(matches!(
            decode(StatusCode::OK, &serde_json::to_vec(&response).unwrap()),
            Err(PresenceError::Protocol)
        ));
    }

    #[test]
    fn admission_error_responses_cannot_settle_a_pending_request() {
        let absent =
            || serde_json::json!({"error": {"code": LEGACY_NAMED_ADMISSION_RECEIPT_NOT_FOUND}});
        assert!(matches!(
            decode_receipt(
                StatusCode::NOT_FOUND,
                AdmissionOperation::Reconcile,
                absent()
            ),
            Ok(None)
        ));
        assert!(matches!(
            decode_receipt(StatusCode::NOT_FOUND, AdmissionOperation::Reserve, absent()),
            Err(PresenceError::AdmissionOutcomeUnknown)
        ));
        for status in [
            StatusCode::NOT_FOUND,
            StatusCode::FORBIDDEN,
            StatusCode::TOO_MANY_REQUESTS,
        ] {
            assert!(matches!(
                decode_receipt(
                    status,
                    AdmissionOperation::Reconcile,
                    serde_json::json!({"error": "fixture"})
                ),
                Err(PresenceError::AdmissionOutcomeUnknown)
            ));
        }
    }

    #[test]
    fn response_status_distinguishes_replay_and_durable_refusals() {
        let admission =
            |replayed, refusal: Option<AdmissionRefusalV1>| crate::control::AdmissionResultV1 {
                replayed,
                accepted: refusal.is_none(),
                refusal,
                worker: None,
                receipt: crate::control::MutationReceiptV1 {
                    operation_id: uuid::Uuid::new_v4().to_string(),
                    worker_id: None,
                    revision: None,
                    input_hash: "b".repeat(64),
                    audit_seq: 1,
                    audit_hash: "c".repeat(64),
                },
            };
        assert_eq!(
            response_status(&admission(false, None)),
            StatusCode::CREATED
        );
        assert_eq!(response_status(&admission(true, None)), StatusCode::OK);
        assert_eq!(
            response_status(&admission(false, Some(AdmissionRefusalV1::Policy))),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            response_status(&admission(false, Some(AdmissionRefusalV1::Quota))),
            StatusCode::TOO_MANY_REQUESTS
        );
    }
}
