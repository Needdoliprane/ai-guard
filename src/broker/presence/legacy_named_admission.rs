//! Private broker transport for the retained Legacy confirmation/admission.
//! This client has no Runner surface and never starts a runtime.
use super::*;
use crate::{
    broker::presence::http::{Client, Limits},
    control::{AdmissionRefusalV1, WorkerStateV1, session_control::*},
};
use reqwest::{Method, StatusCode};

const RESPONSE_BYTES: u64 = 128 * 1024;

pub(crate) enum Outcome {
    Accepted(LegacyNamedAdmissionReceiptV1),
    Refused(LegacyNamedAdmissionReceiptV1),
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
    presence.revalidate()?;

    let value = serde_json::to_value(request).map_err(|_| PresenceError::Protocol)?;
    presence.screen(&value)?;
    let body = serde_json::to_vec(request).map_err(|_| PresenceError::Protocol)?;
    if body.len() > LEGACY_NAMED_ADMISSION_REQUEST_BYTES {
        return Err(PresenceError::Protocol);
    }
    // The Store re-resolves the plan in its write transaction. The client
    // checks the same facts before sending and accepts only a linked receipt.
    request
        .require_plan(&plan)
        .map_err(|_| PresenceError::Protocol)?;
    let path = format!("{path}/admission");
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
        .map_err(|error| match error {
            PresenceError::Transport
            | PresenceError::Protocol
            | PresenceError::Unavailable
            | PresenceError::Capacity => PresenceError::AdmissionOutcomeUnknown,
            other => other,
        })?;
    if !matches!(
        status,
        StatusCode::OK
            | StatusCode::CREATED
            | StatusCode::FORBIDDEN
            | StatusCode::TOO_MANY_REQUESTS
    ) {
        let value = crate::json::decode_unique(&bytes, 64)
            .map_err(|_| PresenceError::AdmissionOutcomeUnknown)?;
        presence
            .screen(&value)
            .map_err(|_| PresenceError::AdmissionOutcomeUnknown)?;
        let error = super::http::remote_error(status, &value);
        return Err(match error {
            PresenceError::Unavailable | PresenceError::Transport | PresenceError::Protocol => {
                PresenceError::AdmissionOutcomeUnknown
            }
            other => other,
        });
    }
    let value = crate::json::decode_unique(&bytes, 64)
        .map_err(|_| PresenceError::AdmissionOutcomeUnknown)?;
    presence
        .screen(&value)
        .map_err(|_| PresenceError::AdmissionOutcomeUnknown)?;
    let receipt: LegacyNamedAdmissionReceiptV1 =
        serde_json::from_value(value.clone()).map_err(|_| {
            if value.get("error").is_some() {
                super::http::remote_error(status, &value)
            } else {
                PresenceError::AdmissionOutcomeUnknown
            }
        })?;
    validate_receipt(request, &plan, &receipt, status)
        .map_err(|_| PresenceError::AdmissionOutcomeUnknown)?;
    if receipt.admission.accepted {
        Ok(Outcome::Accepted(receipt))
    } else {
        Ok(Outcome::Refused(receipt))
    }
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
    let valid_hash = |value: &str| {
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    };
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
        || !valid_hash(&plan.launch_spec_hash)
    {
        return Err(PresenceError::Forbidden);
    }
    Ok(())
}

fn validate_receipt(
    request: &LegacyNamedSupervisedAdmissionV1,
    plan: &LegacyConfirmedPlanV1,
    response: &LegacyNamedAdmissionReceiptV1,
    status: StatusCode,
) -> Result<()> {
    let admission = &response.admission;
    let mutation = &admission.receipt;
    let valid_hash = |value: &str| {
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    };
    if response.version != 1
        || response.audit_seq <= 0
        || !valid_hash(&response.audit_hash)
        || mutation.audit_seq <= 0
        || !valid_hash(&mutation.audit_hash)
        || !valid_hash(&mutation.input_hash)
        || mutation.audit_seq >= response.audit_seq
        || mutation.operation_id != request.admission.admission.operation_id
        || mutation.input_hash
            != request
                .admission
                .input_hash()
                .map_err(|_| PresenceError::Protocol)?
    {
        return Err(PresenceError::Protocol);
    }
    let expected_status = response_status(admission);
    if status != expected_status {
        return Err(match status {
            StatusCode::FORBIDDEN => PresenceError::Forbidden,
            StatusCode::TOO_MANY_REQUESTS => PresenceError::Capacity,
            _ => PresenceError::Protocol,
        });
    }
    match (
        &admission.worker,
        &response.grant,
        admission.refusal,
        admission.accepted,
    ) {
        (Some(worker), Some(grant), None, true) => {
            let worker_uuid =
                uuid::Uuid::parse_str(&worker.worker_id).map_err(|_| PresenceError::Protocol)?;
            let expected_grant_seq = mutation
                .audit_seq
                .checked_add(1)
                .ok_or(PresenceError::Protocol)?;
            let worker_receipt =
                serde_json::to_value(&worker.receipt).map_err(|_| PresenceError::Protocol)?;
            let request_receipt = serde_json::to_value(&request.admission.admission.receipt)
                .map_err(|_| PresenceError::Protocol)?;
            let worker_launch =
                serde_json::to_value(&worker.launch).map_err(|_| PresenceError::Protocol)?;
            let request_launch = serde_json::to_value(&request.admission.admission.launch)
                .map_err(|_| PresenceError::Protocol)?;
            if crate::broker_registry::registry_uuid(&worker.worker_id).is_err()
                || crate::broker_registry::registry_uuid(&worker.attempt.attempt_id).is_err()
                || worker.admission_operation_id != request.admission.admission.operation_id
                || worker.request_id != request.admission.admission.decision_request.request_id
                || worker_receipt != request_receipt
                || worker_launch != request_launch
                || worker.launch.session_id != plan.session_id.to_string()
                || worker.spec_hash
                    != worker
                        .launch
                        .spec_hash()
                        .map_err(|_| PresenceError::Protocol)?
                || worker.spec_hash != plan.launch_spec_hash
                || worker.launch.spec_hash().ok()
                    != request.admission.admission.launch.spec_hash().ok()
                || worker.state != WorkerStateV1::Reserved
                || !worker.quota_held
                || worker.revision != 1
                || worker.attempt.stable_name
                    != format!(
                        "aig-{}-{}",
                        plan.control_namespace_uuid.replace('-', ""),
                        worker_uuid.simple()
                    )
                || worker.attempt.volume_name != format!("{}-work", worker.attempt.stable_name)
                || worker.attempt.container_id.is_some()
                || worker.attempt.create_operation_id.is_some()
                || worker.attempt.created_inspection.is_some()
                || worker.attempt.start_emitter_id.is_some()
                || worker.attempt.start_operation_id.is_some()
                || mutation.worker_id.as_deref() != Some(worker.worker_id.as_str())
                || mutation.revision != Some(worker.revision)
                || grant.version != 1
                || grant.operation_id.is_empty()
                || crate::broker_registry::registry_uuid(&grant.operation_id).is_err()
                || grant.authorization_revision == 0
                || grant.authorization_revision > i64::MAX as u64
                || grant.audit_seq != expected_grant_seq
                || grant.audit_seq.checked_add(1) != Some(response.audit_seq)
                || !valid_hash(&grant.input_hash)
                || !valid_hash(&grant.audit_hash)
            {
                return Err(PresenceError::Protocol);
            }
        }
        (None, None, Some(_), false) => {
            if mutation.worker_id.is_some()
                || mutation.revision.is_some()
                || mutation.audit_seq.checked_add(1) != Some(response.audit_seq)
            {
                return Err(PresenceError::Protocol);
            }
        }
        _ => return Err(PresenceError::Protocol),
    }
    Ok(())
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
        let admission = |replayed, refusal| crate::control::AdmissionResultV1 {
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
