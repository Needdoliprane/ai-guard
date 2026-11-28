//! Private broker transport for the retained Legacy confirmation/admission.
//! This client has no Runner surface and never starts a runtime.
use super::*;
use crate::{
    audit::legacy_session_prepare::LegacyNamedCancelReservationV1,
    broker::{
        journal::{CancelStopProjectionV1, JournalError, JournalScopeV1, canonical},
        presence::http::{Client, Limits},
    },
    control::{
        AdmissionRefusalV1, ArmSubjectV1, ClaimDispositionV1, HelperClaimResultV1,
        HelperPermissionV1, MutationReceiptV1, MutationResultV1, RuntimeKindV1, WorkerV1,
        session_control::*,
    },
};
use reqwest::{Method, StatusCode};

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

pub(super) async fn read_plan(
    presence: &PresenceAuthority,
    expected_broker_profile_id: &str,
    binding_ref: Uuid,
) -> Result<LegacyConfirmedPlanV1> {
    exchange_confirmation(
        presence,
        (Method::GET, None),
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
        (Method::GET, None),
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
        (Method::POST, None),
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
    (method, body): (Method, Option<Vec<u8>>),
    binding_ref: Uuid,
    suffix: &str,
    response_bytes: u64,
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
                body,
                Limits {
                    request: LEGACY_NAMED_ADMISSION_REQUEST_BYTES as u64,
                    success: response_bytes,
                    error: response_bytes,
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

pub(super) async fn arm_import(
    presence: &PresenceAuthority,
    writer: &WriterClient,
    plan: &LegacyConfirmedPlanV1,
    archive_digest: &str,
    arm: ArmV1,
    generation: (&CancelGeneration, u64),
) -> Result<ArmResultV1> {
    validate_scope(presence, writer.scope())?;
    let client = Client::new(presence)?;
    checked_identity(&client, presence).await?;
    let (cancel, expected) = generation;
    cancel
        .with_current_generation(expected, || ())
        .map_err(|_| PresenceError::Stopped)?;
    let request = writer
        .record_legacy_named_import_arm(plan, archive_digest, arm)
        .await
        .map_err(named_write_error)?
        .into_request();
    let result = send_import_arm(&client, presence, &request, Some(generation))
        .await?
        .ok_or(PresenceError::AdmissionOutcomeUnknown)?;
    writer
        .acknowledge_legacy_named_import_arm(plan.operation_id, &request, result.clone())
        .await
        .map_err(|_| PresenceError::AdmissionOutcomeUnknown)?;
    Ok(result)
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
        .map_err(named_write_error)?
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

impl LegacyNamedControl {
    /// Read the historical receipt of the exact named verification Arm.
    /// An absent receipt only describes the remote snapshot of this read.
    #[allow(
        dead_code,
        reason = "the named verification Arm recovery caller is composed separately"
    )]
    pub(crate) async fn reconcile_legacy_named_import_verification_arm(
        &self,
        request: &LegacyNamedImportVerificationArmRequestV2,
    ) -> Result<Option<ArmResultV1>> {
        let presence = &self.0;
        request.validate().map_err(|_| PresenceError::Protocol)?;
        let inspection = &request.arm.inspection;
        let runtime = match inspection.runtime_kind {
            RuntimeKindV1::Docker => BrokerRuntimeKindV1::Docker,
            RuntimeKindV1::Podman => BrokerRuntimeKindV1::Podman,
        };
        if request.namespace != presence.scope.control_namespace_uuid.to_string()
            || inspection.authority_id != presence.config.authority_id
            || runtime != presence.scope.runtime_kind
        {
            return Err(PresenceError::Forbidden);
        }
        presence.revalidate()?;
        let value = serde_json::to_value(request).map_err(|_| PresenceError::Protocol)?;
        presence.screen(&value)?;
        let body = serde_json::to_vec(request).map_err(|_| PresenceError::Protocol)?;
        if body.len() > LEGACY_NAMED_ADMISSION_REQUEST_BYTES {
            return Err(PresenceError::Protocol);
        }
        exchange_confirmation(
            presence,
            (Method::POST, Some(body)),
            request.binding_ref,
            "/admission/import-verification-arm/reconcile",
            LEGACY_NAMED_IMPORT_ARM_RESPONSE_BYTES as u64,
            None,
            |status, bytes| {
                decode_verification_arm_reconciliation(presence, request, status, bytes)
            },
        )
        .await
    }

    /// Reconcile the exact consumed verification claim under this broker.
    /// An absent receipt only describes the remote snapshot of this read.
    #[allow(
        dead_code,
        reason = "the named verification claim recovery caller is composed separately"
    )]
    pub(crate) async fn reconcile_legacy_named_import_verification_claim(
        &self,
        request: &LegacyNamedImportVerificationClaimRequestV2,
    ) -> Result<Option<MutationReceiptV1>> {
        let presence = &self.0;
        request.validate().map_err(|_| PresenceError::Protocol)?;
        if request.namespace != presence.scope.control_namespace_uuid.to_string() {
            return Err(PresenceError::Forbidden);
        }
        presence.revalidate()?;
        let value = serde_json::to_value(request).map_err(|_| PresenceError::Protocol)?;
        presence.screen(&value)?;
        let body = serde_json::to_vec(request).map_err(|_| PresenceError::Protocol)?;
        if body.len() > LEGACY_NAMED_ADMISSION_REQUEST_BYTES {
            return Err(PresenceError::Protocol);
        }
        exchange_confirmation(
            presence,
            (Method::POST, Some(body)),
            request.binding_ref,
            "/admission/import-verification-claim/reconcile",
            LEGACY_NAMED_IMPORT_CLAIM_RESPONSE_BYTES as u64,
            None,
            |status, bytes| {
                decode_verification_claim_reconciliation(presence, request, status, bytes)
            },
        )
        .await
    }
}

fn decode_verification_reconciliation(
    presence: &PresenceAuthority,
    status: StatusCode,
    bytes: &[u8],
    receipt_not_found: &str,
) -> Result<Option<Value>> {
    let value = crate::json::decode_unique(bytes, 64)
        .map_err(|_| PresenceError::AdmissionOutcomeUnknown)?;
    presence
        .screen(&value)
        .map_err(|_| PresenceError::AdmissionOutcomeUnknown)?;
    if status == StatusCode::OK {
        return Ok(Some(value));
    }
    if status == StatusCode::NOT_FOUND
        && value.as_object().is_some_and(|fields| fields.len() == 1)
        && value["error"]
            .as_object()
            .is_some_and(|fields| fields.len() == 2)
        && value["error"]["code"] == receipt_not_found
        && value["error"]["message"].is_string()
    {
        return Ok(None);
    }
    Err(PresenceError::AdmissionOutcomeUnknown)
}

fn decode_verification_arm_reconciliation(
    presence: &PresenceAuthority,
    request: &LegacyNamedImportVerificationArmRequestV2,
    status: StatusCode,
    bytes: &[u8],
) -> Result<Option<ArmResultV1>> {
    let Some(value) = decode_verification_reconciliation(
        presence,
        status,
        bytes,
        LEGACY_NAMED_IMPORT_VERIFICATION_ARM_RECEIPT_NOT_FOUND,
    )?
    else {
        return Ok(None);
    };
    let expected_arm =
        serde_json::to_value(&request.arm).map_err(|_| PresenceError::AdmissionOutcomeUnknown)?;
    if value["arm"] != expected_arm {
        return Err(PresenceError::AdmissionOutcomeUnknown);
    }
    let result: ArmResultV1 =
        serde_json::from_value(value).map_err(|_| PresenceError::AdmissionOutcomeUnknown)?;
    let ArmSubjectV1::Helper { permission, .. } = &request.arm.subject else {
        return Err(PresenceError::AdmissionOutcomeUnknown);
    };
    let HelperPermissionV1::ImportVerification {
        import_terminal, ..
    } = permission.as_ref()
    else {
        return Err(PresenceError::AdmissionOutcomeUnknown);
    };
    if !result.replayed
        || result.receipt.control_namespace_uuid != request.namespace
        || result.receipt.audit_seq <= import_terminal.audit_seq
        || result.receipt.validate_for_arm(&request.arm).is_err()
    {
        return Err(PresenceError::AdmissionOutcomeUnknown);
    }
    Ok(Some(result))
}

fn decode_verification_claim_reconciliation(
    presence: &PresenceAuthority,
    request: &LegacyNamedImportVerificationClaimRequestV2,
    status: StatusCode,
    bytes: &[u8],
) -> Result<Option<MutationReceiptV1>> {
    let Some(value) = decode_verification_reconciliation(
        presence,
        status,
        bytes,
        LEGACY_NAMED_IMPORT_VERIFICATION_CLAIM_RECEIPT_NOT_FOUND,
    )?
    else {
        return Ok(None);
    };
    let result: HelperClaimResultV1 =
        serde_json::from_value(value).map_err(|_| PresenceError::AdmissionOutcomeUnknown)?;
    if result.disposition != ClaimDispositionV1::AlreadyConsumed {
        return Err(PresenceError::AdmissionOutcomeUnknown);
    }
    request
        .validate_receipt(&result.receipt)
        .map_err(|_| PresenceError::AdmissionOutcomeUnknown)?;
    Ok(Some(result.receipt))
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

    #[test]
    fn verification_arm_reconciliation_requires_an_exact_historical_receipt() -> anyhow::Result<()>
    {
        use crate::control::{
            ArmReceiptV1, ArmSubjectV1, ArmV1, HelperPermissionV1, HelperPurposeV1,
            HostClockMethodV1, HostClockSampleV1, RuntimeInspectionV1,
        };

        let (_fixture, presence, material) = launch_material();
        let binding_ref = material.plan.binding_ref;
        let namespace = material.plan.control_namespace_uuid;
        let launch = material.launch;
        let worker_id = Uuid::new_v4().to_string();
        let attempt = Uuid::new_v4();
        let import_arm = ArmReceiptV1 {
            version: 1,
            arm_id: LegacyNamedImportArmRequestV1::operation_id(binding_ref, attempt),
            worker_id: worker_id.clone(),
            attempt_id: attempt.to_string(),
            input_hash: "a".repeat(64),
            control_namespace_uuid: namespace.clone(),
            audit_seq: 10,
            audit_hash: "b".repeat(64),
        };
        let import_terminal = MutationReceiptV1 {
            operation_id: Uuid::new_v4().to_string(),
            worker_id: Some(worker_id.clone()),
            revision: None,
            input_hash: "a".repeat(64),
            audit_seq: 12,
            audit_hash: "d".repeat(64),
        };
        let request = LegacyNamedImportVerificationArmRequestV2 {
            version: 2,
            binding_ref,
            namespace: namespace.clone(),
            arm: ArmV1 {
                version: 1,
                operation_id: LegacyNamedImportVerificationArmRequestV2::operation_id(
                    binding_ref,
                    attempt,
                ),
                worker_id: worker_id.clone(),
                attempt_id: attempt.to_string(),
                expected_revision: 2,
                intent_hash: "a".repeat(64),
                subject: ArmSubjectV1::Helper {
                    helper_id: Uuid::new_v4().to_string(),
                    purpose: HelperPurposeV1::ImportVerification,
                    policy_version: 1,
                    permission: Box::new(HelperPermissionV1::ImportVerification {
                        import_helper_id: Uuid::new_v4().to_string(),
                        import_arm,
                        import_terminal,
                        manifest_digest: launch.workspace_manifest_digest,
                    }),
                },
                inspection: RuntimeInspectionV1 {
                    authority_id: launch.authority_id,
                    runtime_kind: launch.runtime_kind,
                    container_id: "a".repeat(64),
                    stable_name: "fixture-verification".into(),
                    volume_name: "fixture-workspace".into(),
                    image_id: launch.image_id,
                    spec_hash: "e".repeat(64),
                    cpu_millis: launch.cpu_millis,
                    memory_bytes: launch.memory_bytes,
                    pids_limit: launch.pids_limit,
                    network: launch.network,
                    security_preset: launch.security_preset,
                    restart_policy: launch.restart_policy,
                },
                authority_binding_hash: "a".repeat(64),
                configuration_fingerprint_version: 2,
                configuration_fingerprint: "c".repeat(64),
                clock: HostClockSampleV1 {
                    version: 1,
                    method: HostClockMethodV1::LinuxBoottimeV1,
                    boot_id: Uuid::new_v4().to_string(),
                    continuous_ns_decimal: "100000000000".into(),
                    observed_utc: None,
                },
                timeout_seconds: 120,
                deadline_ns_decimal: "220000000000".into(),
            },
        };
        request.validate()?;
        let response = ArmResultV1 {
            replayed: true,
            arm: request.arm.clone(),
            receipt: ArmReceiptV1 {
                version: 1,
                arm_id: request.arm.operation_id.clone(),
                worker_id,
                attempt_id: attempt.to_string(),
                input_hash: request.arm.input_hash()?,
                control_namespace_uuid: namespace,
                audit_seq: 13,
                audit_hash: "f".repeat(64),
            },
        };
        let decode = |status, bytes: &[u8]| {
            decode_verification_arm_reconciliation(&presence.authority, &request, status, bytes)
        };
        let decode_result =
            |status, reply: &ArmResultV1| decode(status, &serde_json::to_vec(reply).unwrap());
        let acknowledged = decode_result(StatusCode::OK, &response)?.unwrap();
        assert!(acknowledged.replayed);
        assert_eq!(acknowledged.receipt.audit_seq, 13);
        assert_eq!(acknowledged.receipt.audit_hash, "f".repeat(64));
        assert!(decode_result(StatusCode::CREATED, &response).is_err());

        let mut fresh = response.clone();
        fresh.replayed = false;
        assert!(decode_result(StatusCode::OK, &fresh).is_err());
        assert!(decode_result(StatusCode::CREATED, &fresh).is_err());
        let mut divergent_arm = response.clone();
        divergent_arm.arm.configuration_fingerprint = "e".repeat(64);
        divergent_arm.receipt.input_hash = divergent_arm.arm.input_hash()?;
        assert!(decode_result(StatusCode::OK, &divergent_arm).is_err());
        let mut divergent_receipt = response.clone();
        divergent_receipt.receipt.input_hash = "a".repeat(64);
        assert!(decode_result(StatusCode::OK, &divergent_receipt).is_err());
        let mut foreign = response.clone();
        foreign.receipt.control_namespace_uuid = Uuid::new_v4().to_string();
        assert!(decode_result(StatusCode::OK, &foreign).is_err());
        let mut earlier = response;
        earlier.receipt.audit_seq = 12;
        assert!(decode_result(StatusCode::OK, &earlier).is_err());

        let absent = br#"{"error":{"code":"legacy_named_import_verification_arm_receipt_not_found","message":"Missing receipt."}}"#;
        assert!(decode(StatusCode::NOT_FOUND, absent)?.is_none());
        assert!(decode(StatusCode::FORBIDDEN, absent).is_err());
        let unsafe_absence = serde_json::json!({
            "error": {
                "code": LEGACY_NAMED_IMPORT_VERIFICATION_ARM_RECEIPT_NOT_FOUND,
                "message": super::super::preparation_tests::OWN_TOKEN,
            },
        });
        assert!(decode(StatusCode::NOT_FOUND, &serde_json::to_vec(&unsafe_absence)?).is_err());
        let foreign = br#"{"error":{"code":"not_found","message":"Missing receipt."}}"#;
        assert!(matches!(
            decode(StatusCode::NOT_FOUND, foreign),
            Err(PresenceError::AdmissionOutcomeUnknown)
        ));
        let duplicate = br#"{"error":{"code":"not_found","code":"legacy_named_import_verification_arm_receipt_not_found","message":"Missing receipt."}}"#;
        assert!(decode(StatusCode::NOT_FOUND, duplicate).is_err());
        Ok(())
    }

    fn import_claim_fixture()
    -> anyhow::Result<(LegacyNamedImportClaimRequestV1, HelperClaimResultV1)> {
        use crate::control::{
            ArmReceiptV1, HelperStartClaimV1, HostClockMethodV1, HostClockSampleV1,
        };

        let binding_ref = Uuid::new_v4();
        let namespace = Uuid::new_v4().to_string();
        let arm_id = Uuid::new_v4();
        let worker_id = Uuid::new_v4().to_string();
        let request = LegacyNamedImportClaimRequestV1 {
            version: 1,
            binding_ref,
            namespace: namespace.clone(),
            claim: HelperStartClaimV1 {
                version: 1,
                operation_id: LegacyNamedImportClaimRequestV1::operation_id(binding_ref, arm_id),
                helper_id: Uuid::new_v4().to_string(),
                emitter_id: Uuid::new_v4().to_string(),
                arm_receipt: ArmReceiptV1 {
                    version: 1,
                    arm_id: arm_id.to_string(),
                    worker_id: worker_id.clone(),
                    attempt_id: Uuid::new_v4().to_string(),
                    input_hash: "a".repeat(64),
                    control_namespace_uuid: namespace,
                    audit_seq: 11,
                    audit_hash: "b".repeat(64),
                },
                clock_check: HostClockSampleV1 {
                    version: 1,
                    method: HostClockMethodV1::LinuxBoottimeV1,
                    boot_id: Uuid::new_v4().to_string(),
                    continuous_ns_decimal: "100000000000".into(),
                    observed_utc: None,
                },
            },
        };
        let response = HelperClaimResultV1 {
            disposition: ClaimDispositionV1::EmitOnce,
            receipt: MutationReceiptV1 {
                operation_id: request.claim.operation_id.clone(),
                worker_id: Some(worker_id),
                revision: None,
                input_hash: request.input_hash()?,
                audit_seq: 12,
                audit_hash: "c".repeat(64),
            },
        };
        Ok((request, response))
    }

    #[test]
    fn verification_claim_reconciliation_accepts_only_the_exact_consumed_receipt()
    -> anyhow::Result<()> {
        let fixture = Fixture::new();
        fixture.write_config();
        let scope = fixture.scope();
        let namespace = scope.control_namespace_uuid.to_string();
        let presence = PreparedPresence::load(&fixture.config, scope, vec![])?;
        let (legacy, mut response) = import_claim_fixture()?;
        let binding_ref = legacy.binding_ref;
        let mut claim = legacy.claim;
        claim.arm_receipt.control_namespace_uuid = namespace.clone();
        claim.arm_receipt.arm_id = LegacyNamedImportVerificationArmRequestV2::operation_id(
            binding_ref,
            canonical_uuid(&claim.arm_receipt.attempt_id)?,
        );
        claim.operation_id = LegacyNamedImportVerificationClaimRequestV2::operation_id(
            binding_ref,
            canonical_uuid(&claim.arm_receipt.arm_id)?,
        );
        let request = LegacyNamedImportVerificationClaimRequestV2 {
            version: 2,
            binding_ref,
            namespace,
            claim,
        };
        response.receipt.operation_id = request.claim.operation_id.clone();
        response.receipt.input_hash = request.input_hash()?;
        response.disposition = ClaimDispositionV1::AlreadyConsumed;
        let decode = |status, bytes: &[u8]| {
            decode_verification_claim_reconciliation(&presence.authority, &request, status, bytes)
        };
        let consumed = serde_json::to_vec(&response)?;
        assert_eq!(
            decode(StatusCode::OK, &consumed)?,
            Some(response.receipt.clone())
        );
        assert!(decode(StatusCode::CREATED, &consumed).is_err());

        let mut divergent = response.clone();
        divergent.disposition = ClaimDispositionV1::EmitOnce;
        for status in [StatusCode::OK, StatusCode::CREATED] {
            assert!(decode(status, &serde_json::to_vec(&divergent)?).is_err());
        }
        divergent = response.clone();
        divergent.receipt.input_hash = request.claim.input_hash()?;
        assert!(decode(StatusCode::OK, &serde_json::to_vec(&divergent)?).is_err());
        divergent = response;
        divergent.receipt.audit_seq = request.claim.arm_receipt.audit_seq;
        assert!(decode(StatusCode::OK, &serde_json::to_vec(&divergent)?).is_err());

        let absent = br#"{"error":{"code":"legacy_named_import_verification_claim_receipt_not_found","message":"Missing receipt."}}"#;
        assert!(decode(StatusCode::NOT_FOUND, absent)?.is_none());
        assert!(decode(StatusCode::FORBIDDEN, absent).is_err());
        let legacy_absence = br#"{"error":{"code":"legacy_named_import_claim_receipt_not_found","message":"Missing receipt."}}"#;
        assert!(matches!(
            decode(StatusCode::NOT_FOUND, legacy_absence),
            Err(PresenceError::AdmissionOutcomeUnknown)
        ));
        let orphan = br#"{"error":{"code":"not_found","message":"Missing receipt."}}"#;
        assert!(decode(StatusCode::NOT_FOUND, orphan).is_err());
        Ok(())
    }

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

        let cancel = CancelGeneration::new();
        let (started, wait_started) = tokio::sync::oneshot::channel();
        let (reply, wait_reply) = tokio::sync::oneshot::channel();
        let send = dispatch_before_stop(&cancel, cancel.current(), async {
            started.send(()).unwrap();
            Ok(wait_reply.await.unwrap())
        });
        let stop_then_reply = async {
            wait_started.await.unwrap();
            cancel.invalidate();
            reply.send(1u8).unwrap();
        };
        let (receipt, ()) = tokio::join!(send, stop_then_reply);
        assert_eq!(receipt, Ok(1));
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
