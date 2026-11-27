//! Private broker transport for the retained Legacy confirmation/admission.
//! This client has no Runner surface and never starts a runtime.
use super::*;
use crate::{
    broker::presence::http::{Client, Limits},
    control::{AdmissionRefusalV1, RuntimeKindV1, hash, session_control::*},
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

pub(super) async fn read_plan(
    presence: &PreparedPresence,
    expected_broker_profile_id: &str,
    binding_ref: Uuid,
) -> Result<LegacyConfirmedPlanV1> {
    read_confirmation(
        presence,
        binding_ref,
        "",
        RESPONSE_BYTES,
        |status, bytes| {
            let value = decode_confirmation(presence, status, bytes)?;
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
    presence: &PreparedPresence,
    expected_broker_profile_id: &str,
    binding_ref: Uuid,
) -> Result<LegacyConfirmedLaunchSpecV1> {
    if presence.scope.profile_id != expected_broker_profile_id {
        return Err(PresenceError::Forbidden);
    }
    read_confirmation(
        presence,
        binding_ref,
        "/launch-spec",
        LEGACY_CONFIRMED_LAUNCH_SPEC_RESPONSE_BYTES as u64,
        |status, bytes| decode_launch_spec(presence, binding_ref, status, bytes),
    )
    .await
}

async fn read_confirmation<T>(
    presence: &PreparedPresence,
    binding_ref: Uuid,
    suffix: &str,
    success_bytes: u64,
    decode: impl FnOnce(StatusCode, &[u8]) -> Result<T>,
) -> Result<T> {
    if binding_ref.is_nil() {
        return Err(PresenceError::Protocol);
    }
    let path = format!("/api/v1/control/session-confirmations/{binding_ref}{suffix}");
    let client = Client::new(presence)?;
    let before = checked_identity(&client, presence).await?;
    let (status, bytes) = client
        .send_session_bounded(
            Method::GET,
            &path,
            None,
            Limits {
                request: 0,
                success: success_bytes,
                error: RESPONSE_BYTES,
            },
        )
        .await?;
    let confirmed = decode(status, &bytes)?;
    let after = checked_identity(&client, presence).await?;
    if before != after {
        return Err(PresenceError::Conflict);
    }
    presence.revalidate()?;
    Ok(confirmed)
}

pub(super) async fn admit(
    presence: &PreparedPresence,
    expected_broker_profile_id: &str,
    plan: &LegacyConfirmedPlanV1,
    request: &LegacyNamedSupervisedAdmissionV1,
) -> Result<Outcome> {
    validate_plan(presence, expected_broker_profile_id, request, plan)?;
    let client = Client::new(presence)?;
    checked_identity(&client, presence).await?;
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
    Ok(Some(if receipt.admission.accepted {
        Outcome::Accepted(receipt)
    } else {
        Outcome::Refused(receipt)
    }))
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
    presence: &PreparedPresence,
    status: StatusCode,
    bytes: &[u8],
) -> Result<Value> {
    let value = crate::json::decode_unique(bytes, 64).map_err(|_| PresenceError::Protocol)?;
    if status != StatusCode::OK {
        return Err(super::http::remote_error(status, &value));
    }
    presence.screen(&value)?;
    Ok(value)
}

fn decode_launch_spec(
    presence: &PreparedPresence,
    binding_ref: Uuid,
    status: StatusCode,
    bytes: &[u8],
) -> Result<LegacyConfirmedLaunchSpecV1> {
    let value = decode_confirmation(presence, status, bytes)?;
    if Redactor::new(presence.secrets.clone()).value_unclipped(&value) != value {
        return Err(PresenceError::Protocol);
    }
    let material: LegacyConfirmedLaunchSpecV1 =
        serde_json::from_value(value).map_err(|_| PresenceError::Protocol)?;
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
    Ok(material)
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
    fn launch_spec_decoder_requires_exact_private_material() {
        use crate::{
            broker::presence::preparation_tests::Fixture,
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
        let binding_ref = plan.binding_ref;
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
        let mut material = LegacyConfirmedLaunchSpecV1 {
            version: 1,
            plan,
            launch,
        };
        let decode = |body: &[u8]| decode_launch_spec(&presence, binding_ref, StatusCode::OK, body);
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
        material.launch.authority_id = presence.config.authority_id.clone();
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
