//! Private broker transport for the retained Legacy confirmation/admission.
//! This client has no Runner surface and never starts a runtime.
use super::*;
use crate::{
    broker::presence::http::{Client, Limits},
    control::{AdmissionRefusalV1, hash, session_control::*},
};
use reqwest::{Method, StatusCode};

const RESPONSE_BYTES: u64 = 128 * 1024;

pub(crate) enum Outcome {
    Accepted(LegacyNamedAdmissionReceiptV1),
    Refused(LegacyNamedAdmissionReceiptV1),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AdmissionOperation {
    Reserve,
    Reconcile,
}

pub(super) async fn admit(
    presence: &PreparedPresence,
    expected_broker_profile_id: &str,
    request: &LegacyNamedSupervisedAdmissionV1,
) -> Result<Outcome> {
    request.validate().map_err(|_| PresenceError::Protocol)?;
    let binding = request.binding_ref;
    let path = format!("/api/v1/control/session-confirmations/{binding}");
    let client = Client::new(presence)?;

    let before = checked_identity(&client, presence).await?;
    let plan = confirmed_plan(&client, presence, &path).await?;
    validate_plan(presence, expected_broker_profile_id, request, &plan)?;
    let after = checked_identity(&client, presence).await?;
    if before != after {
        return Err(PresenceError::Conflict);
    }
    send_request(&client, presence, request, AdmissionOperation::Reserve)
        .await?
        .ok_or(PresenceError::AdmissionOutcomeUnknown)
}

pub(super) async fn reconcile(
    presence: &PreparedPresence,
    expected_broker_profile_id: &str,
    plan: &LegacyConfirmedPlanV1,
    request: &LegacyNamedSupervisedAdmissionV1,
) -> Result<Option<Outcome>> {
    validate_plan(presence, expected_broker_profile_id, request, plan)?;
    let client = Client::new(presence)?;
    checked_identity(&client, presence).await?;
    send_request(&client, presence, request, AdmissionOperation::Reconcile).await
}

async fn send_request(
    client: &Client,
    presence: &PreparedPresence,
    request: &LegacyNamedSupervisedAdmissionV1,
    operation: AdmissionOperation,
) -> Result<Option<Outcome>> {
    presence.revalidate()?;
    let value = serde_json::to_value(request).map_err(|_| PresenceError::Protocol)?;
    presence.screen(&value)?;
    let body = serde_json::to_vec(request).map_err(|_| PresenceError::Protocol)?;
    if body.len() > LEGACY_NAMED_ADMISSION_REQUEST_BYTES {
        return Err(PresenceError::Protocol);
    }
    let action = match operation {
        AdmissionOperation::Reserve => "admission",
        AdmissionOperation::Reconcile => "admission/reconcile",
    };
    let binding = request.binding_ref;
    let path = format!("/api/v1/control/session-confirmations/{binding}/{action}");
    let (status, bytes) = client
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
        .map_err(|_| PresenceError::AdmissionOutcomeUnknown)?;
    let value = crate::json::decode_unique(&bytes, 64)
        .map_err(|_| PresenceError::AdmissionOutcomeUnknown)?;
    presence
        .screen(&value)
        .map_err(|_| PresenceError::AdmissionOutcomeUnknown)?;
    decode_response(
        presence.scope.control_namespace_uuid,
        request,
        operation,
        status,
        value,
    )
}

fn decode_response(
    namespace: uuid::Uuid,
    request: &LegacyNamedSupervisedAdmissionV1,
    operation: AdmissionOperation,
    status: StatusCode,
    value: Value,
) -> Result<Option<Outcome>> {
    if operation == AdmissionOperation::Reconcile
        && status == StatusCode::NOT_FOUND
        && value["error"]["code"] == LEGACY_NAMED_ADMISSION_RECEIPT_NOT_FOUND
    {
        // Absence was observed at this read. A separate request may still commit.
        return Ok(None);
    }
    let receipt: LegacyNamedAdmissionReceiptV1 =
        serde_json::from_value(value.clone()).map_err(|_| {
            if value.get("error").is_some() {
                let error = super::http::remote_error(status, &value);
                match error {
                    PresenceError::Authentication
                    | PresenceError::Forbidden
                    | PresenceError::Capacity
                    | PresenceError::Conflict
                    | PresenceError::Clock => error,
                    _ => PresenceError::AdmissionOutcomeUnknown,
                }
            } else {
                PresenceError::AdmissionOutcomeUnknown
            }
        })?;
    if operation == AdmissionOperation::Reconcile && !receipt.admission.replayed {
        return Err(PresenceError::AdmissionOutcomeUnknown);
    }
    validate_receipt(namespace, request, &receipt, status)
        .map_err(|_| PresenceError::AdmissionOutcomeUnknown)?;
    Ok(Some(if receipt.admission.accepted {
        Outcome::Accepted(receipt)
    } else {
        Outcome::Refused(receipt)
    }))
}

async fn confirmed_plan(
    client: &Client,
    presence: &PreparedPresence,
    path: &str,
) -> Result<LegacyConfirmedPlanV1> {
    let (status, bytes) = client
        .send_session_bounded(
            Method::GET,
            path,
            None,
            Limits {
                request: 0,
                success: RESPONSE_BYTES,
                error: RESPONSE_BYTES,
            },
        )
        .await?;
    if status != StatusCode::OK {
        let value = crate::json::decode_unique(&bytes, 64).map_err(|_| PresenceError::Protocol)?;
        presence.screen(&value)?;
        return Err(super::http::remote_error(status, &value));
    }
    let value = crate::json::decode_unique(&bytes, 64).map_err(|_| PresenceError::Protocol)?;
    presence.screen(&value)?;
    serde_json::from_value(value).map_err(|_| PresenceError::Protocol)
}

async fn checked_identity(
    client: &Client,
    presence: &PreparedPresence,
) -> Result<crate::broker_registry::BrokerIdentityViewV1> {
    let identity = client.identity(presence).await?;
    let view = client.view(presence).await?;
    super::actor::validate_handshake(presence, &identity, &view)?;
    Ok(identity)
}

fn validate_plan(
    presence: &PreparedPresence,
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
    if plan.version != 1
        || plan.binding_ref != binding_ref
        || plan.control_namespace_uuid != expected_namespace
        || expected_broker_profile_id.is_empty()
        || expected_broker_profile_id.len() > 128
        || plan.broker_profile_id != expected_broker_profile_id
        || plan.operation_id.is_nil()
        || plan.session_id.is_nil()
        || plan.launch_target_id.is_nil()
        || !(1..=i64::MAX as u64).contains(&plan.target_revision)
        || plan.expires_at.timestamp() <= 0
        || hash(&plan.launch_spec_hash).is_err()
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
