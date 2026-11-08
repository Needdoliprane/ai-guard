use super::*;
use crate::{
    broker_registry::{format_time, parse_time},
    runtime::kubernetes::admission,
};
use chrono::Utc;
use reqwest::StatusCode;

fn require(condition: bool) -> ClientResult<()> {
    if condition {
        Ok(())
    } else {
        Err(HeldCreateClientError::Protocol)
    }
}
pub(super) fn id(value: &str) -> ClientResult<()> {
    crate::broker_registry::registry_uuid(value).map_err(|_| HeldCreateClientError::InvalidRequest)
}
fn revision(value: u64) -> bool {
    value > 0 && value <= i64::MAX as u64
}

pub(super) fn intent(
    scope: &HeldCreateClientScope,
    intent: &KubernetesCreateIntentV1,
) -> ClientResult<()> {
    intent
        .validate()
        .map_err(|_| HeldCreateClientError::InvalidRequest)?;
    if scope.control_namespace_uuid != intent.control_namespace_uuid
        || scope.broker_id != intent.broker_id
        || scope.incarnation_id != intent.incarnation_id
    {
        return Err(HeldCreateClientError::InvalidContext);
    }
    Ok(())
}
pub(super) fn protocol(
    scope: &HeldCreateClientScope,
    identity: &BrokerIdentityViewV1,
    protocol: &KubernetesHeldCreateProtocolV1,
) -> ClientResult<()> {
    require(
        identity.version == 1
            && [9, 10, 11].contains(&identity.schema_version)
            && protocol.version == 1
            && [9, 10, 11].contains(&protocol.schema_version)
            && identity.schema_version == protocol.schema_version
            && revision(identity.binding_revision)
            && revision(identity.grant_revision),
    )?;
    if identity.tenant_id != scope.tenant_id
        || identity.control_namespace_uuid != scope.control_namespace_uuid
        || identity.broker_id != scope.broker_id
        || identity.credential_id != scope.credential_id
        || identity.incarnation_id != scope.incarnation_id
    {
        return Err(HeldCreateClientError::InvalidContext);
    }
    require(
        protocol.tenant_id == identity.tenant_id
            && protocol.control_namespace_uuid == identity.control_namespace_uuid
            && protocol.broker_id == identity.broker_id
            && protocol.incarnation_id == identity.incarnation_id
            && protocol.binding_revision == identity.binding_revision
            && protocol.grant_revision == identity.grant_revision
            && protocol.operations
                == vec![
                    KubernetesHeldCreateOperationV1::Reserve,
                    KubernetesHeldCreateOperationV1::Claim,
                    KubernetesHeldCreateOperationV1::ObserveClaim,
                    KubernetesHeldCreateOperationV1::Result,
                    KubernetesHeldCreateOperationV1::Cancel,
                ],
    )
}
fn window(window: &KubernetesCreateEmissionWindowV1) -> ClientResult<()> {
    let start = parse_time(&window.issued_at).map_err(|_| HeldCreateClientError::Protocol)?;
    let end = parse_time(&window.not_after).map_err(|_| HeldCreateClientError::Protocol)?;
    require(
        format_time(start) == window.issued_at
            && format_time(end) == window.not_after
            && end > start
            && end - start <= chrono::Duration::seconds(8),
    )
}
fn snapshot(
    scope: &HeldCreateClientScope,
    snapshot: &KubernetesCreateSnapshotV1,
) -> ClientResult<()> {
    intent(scope, &snapshot.intent).map_err(|_| HeldCreateClientError::Protocol)?;
    let prepared =
        admission::prepare(&snapshot.intent).map_err(|_| HeldCreateClientError::Protocol)?;
    require(
        snapshot.version == 1
            && revision(snapshot.revision)
            && revision(snapshot.create_epoch)
            && snapshot.intent_hash
                == snapshot
                    .intent
                    .intent_hash()
                    .map_err(|_| HeldCreateClientError::Protocol)?
            && snapshot.target == *prepared.target(),
    )?;
    if let Some(claim) = &snapshot.claim {
        claim
            .validate()
            .map_err(|_| HeldCreateClientError::Protocol)?;
        require(
            claim.create_id == snapshot.intent.create_id
                && claim.intent_hash == snapshot.intent_hash,
        )?;
        window(
            snapshot
                .emission_window
                .as_ref()
                .ok_or(HeldCreateClientError::Protocol)?,
        )?;
    } else {
        require(snapshot.emission_window.is_none())?;
    }
    if let Some(result) = &snapshot.last_result {
        result
            .validate()
            .map_err(|_| HeldCreateClientError::Protocol)?;
        require(
            result.create_id == snapshot.intent.create_id
                && result.intent_hash == snapshot.intent_hash
                && snapshot
                    .claim
                    .as_ref()
                    .is_some_and(|claim| claim.operation_id == result.claim_operation_id),
        )?;
        if let KubernetesCreateOutcomeV1::CreatedHeld {
            observation,
            admitted_spec,
            admitted_configuration_sha256,
            ..
        } = &result.outcome
        {
            admission::validate_created_held(
                &snapshot.intent,
                observation,
                admitted_spec,
                admitted_configuration_sha256,
            )
            .map_err(|_| HeldCreateClientError::Protocol)?;
        }
    }
    if let Some(cancel) = &snapshot.last_cancel {
        cancel
            .validate()
            .map_err(|_| HeldCreateClientError::Protocol)?;
        require(
            cancel.create_id == snapshot.intent.create_id
                && cancel.intent_hash == snapshot.intent_hash,
        )?;
    }
    Ok(())
}
fn receipt(
    scope: &HeldCreateClientScope,
    receipt: &KubernetesCreateReceiptV1,
    operation: &str,
    hash: &str,
) -> ClientResult<()> {
    receipt
        .validate()
        .map_err(|_| HeldCreateClientError::Protocol)?;
    require(
        receipt.operation_id == operation
            && receipt.input_hash == hash
            && receipt.control_namespace_uuid == scope.control_namespace_uuid,
    )
}

pub(super) fn reservation(
    scope: &HeldCreateClientScope,
    intent: &KubernetesCreateIntentV1,
    response: &KubernetesCreateMutationResponseV1,
    status: StatusCode,
) -> ClientResult<()> {
    snapshot(scope, &response.snapshot)?;
    receipt(
        scope,
        &response.receipt,
        &intent.create_id,
        &intent
            .intent_hash()
            .map_err(|_| HeldCreateClientError::Protocol)?,
    )?;
    let prepared = admission::prepare(intent).map_err(|_| HeldCreateClientError::Protocol)?;
    let expected = KubernetesCreateSnapshotV1 {
        version: 1,
        intent: intent.clone(),
        intent_hash: intent
            .intent_hash()
            .map_err(|_| HeldCreateClientError::Protocol)?,
        target: prepared.target().clone(),
        state: KubernetesCreateStateV1::Reserved,
        create_epoch: 1,
        revision: 1,
        quota_held: true,
        claim: None,
        emission_window: None,
        last_result: None,
        last_cancel: None,
    };
    require(
        response.version == 1
            && response.snapshot == expected
            && response.receipt.emission_window.is_none()
            && if response.replayed {
                status == StatusCode::OK
            } else {
                status == StatusCode::CREATED
            },
    )
}

pub(super) fn claim(
    scope: &HeldCreateClientScope,
    response: &KubernetesCreateClaimResponseV1,
    create_id: &str,
    operation_id: &str,
    expected: Option<(
        &KubernetesCreateIntentV1,
        &KubernetesCreateClaimV1,
        &KubernetesCreateTargetV1,
    )>,
) -> ClientResult<()> {
    snapshot(scope, &response.snapshot)?;
    let claim = response
        .snapshot
        .claim
        .as_ref()
        .ok_or(HeldCreateClientError::Protocol)?;
    receipt(
        scope,
        &response.receipt,
        operation_id,
        &claim
            .input_hash()
            .map_err(|_| HeldCreateClientError::Protocol)?,
    )?;
    require(
        response.version == 1
            && claim.create_id == create_id
            && claim.operation_id == operation_id
            && response.snapshot.intent.create_id == create_id
            && response.snapshot.state == KubernetesCreateStateV1::EmissionPossible
            && response.snapshot.revision == 2
            && response.snapshot.create_epoch == 1
            && response.snapshot.quota_held
            && claim.expected_revision == 1
            && claim.expected_create_epoch == 1
            && response.snapshot.last_result.is_none()
            && response.snapshot.last_cancel.is_none()
            && response.receipt.emission_window.is_some()
            && response.receipt.emission_window == response.snapshot.emission_window,
    )?;
    if let Some((intent, requested, target)) = expected {
        require(
            response.snapshot.intent == *intent
                && claim == requested
                && response.snapshot.target == *target,
        )?;
    }
    Ok(())
}

pub(super) fn result(
    scope: &HeldCreateClientScope,
    result: &KubernetesCreateResultV1,
    response: &KubernetesCreateResultResponseV1,
) -> ClientResult<()> {
    snapshot(scope, &response.snapshot)?;
    receipt(
        scope,
        &response.receipt,
        &result.operation_id,
        &result
            .input_hash()
            .map_err(|_| HeldCreateClientError::Protocol)?,
    )?;
    let state = match result.outcome {
        KubernetesCreateOutcomeV1::CreatedHeld { .. } => KubernetesCreateStateV1::CreatedHeld,
        KubernetesCreateOutcomeV1::OwnershipUnknown { .. } => {
            KubernetesCreateStateV1::OwnershipUnknown
        }
    };
    require(
        response.version == 1
            && response.snapshot.intent.create_id == result.create_id
            && response.snapshot.intent_hash == result.intent_hash
            && result.expected_revision == 2
            && result.expected_create_epoch == 1
            && Some(response.snapshot.revision) == result.expected_revision.checked_add(1)
            && response.snapshot.create_epoch == result.expected_create_epoch
            && response.snapshot.state == state
            && response.snapshot.quota_held
            && response.snapshot.last_result.as_ref() == Some(result)
            && response.snapshot.last_cancel.is_none()
            && response.snapshot.claim.as_ref().is_some_and(|claim| {
                claim.operation_id == result.claim_operation_id
                    && claim.expected_revision == 1
                    && claim.expected_create_epoch == 1
            })
            && response.receipt.emission_window.is_none(),
    )
}
pub(super) fn cancel(
    scope: &HeldCreateClientScope,
    cancel: &KubernetesCreateCancelV1,
    response: &KubernetesCreateMutationResponseV1,
) -> ClientResult<()> {
    snapshot(scope, &response.snapshot)?;
    receipt(
        scope,
        &response.receipt,
        &cancel.operation_id,
        &cancel
            .input_hash()
            .map_err(|_| HeldCreateClientError::Protocol)?,
    )?;
    require(
        response.version == 1
            && response.snapshot.intent.create_id == cancel.create_id
            && response.snapshot.intent_hash == cancel.intent_hash
            && Some(response.snapshot.revision) == cancel.expected_revision.checked_add(1)
            && Some(response.snapshot.create_epoch) == cancel.expected_create_epoch.checked_add(1)
            && response.snapshot.last_cancel.as_ref() == Some(cancel)
            && response.receipt.emission_window.is_none(),
    )?;
    if response.snapshot.claim.is_none() {
        require(
            response.snapshot.state == KubernetesCreateStateV1::CancelledBeforeEmission
                && !response.snapshot.quota_held
                && response.snapshot.last_result.is_none(),
        )
    } else {
        let state = if response
            .snapshot
            .last_result
            .as_ref()
            .is_some_and(|r| matches!(r.outcome, KubernetesCreateOutcomeV1::CreatedHeld { .. }))
        {
            KubernetesCreateStateV1::CreatedHeld
        } else {
            KubernetesCreateStateV1::OwnershipUnknown
        };
        require(response.snapshot.state == state && response.snapshot.quota_held)
    }
}

fn elapsed(clock: &Arc<dyn HostClock>, before: &HostClockSampleV1) -> Option<u128> {
    before.validate().ok()?;
    let current = clock.sample().ok()?;
    current.validate().ok()?;
    if current.boot_id != before.boot_id || current.method != before.method {
        return None;
    }
    current
        .continuous_ns()
        .ok()?
        .checked_sub(before.continuous_ns().ok()?)
}
pub(super) fn origin_live(clock: &Arc<dyn HostClock>, before: &HostClockSampleV1) -> bool {
    elapsed(clock, before).is_some_and(|elapsed| elapsed < 8_000_000_000)
}
pub(super) fn fresh(
    clock: &Arc<dyn HostClock>,
    before: &HostClockSampleV1,
    receipt: &KubernetesCreateReceiptV1,
) -> bool {
    let Some(window) = &receipt.emission_window else {
        return false;
    };
    let (Ok(start), Ok(end)) = (parse_time(&window.issued_at), parse_time(&window.not_after))
    else {
        return false;
    };
    let Some(budget) = (end - start)
        .num_nanoseconds()
        .and_then(|n| u128::try_from(n).ok())
    else {
        return false;
    };
    let now = Utc::now();
    budget > 0
        && budget <= 8_000_000_000
        && start <= now
        && now < end
        && elapsed(clock, before).is_some_and(|elapsed| elapsed < budget)
}

#[cfg(test)]
mod schema11_compatibility_tests {
    use super::*;

    // Descriptive DTOs for the pure validator, never an authenticated proof,
    // live HTTP response, Store or permission to create a workload.
    fn negotiation(
        schema_version: u32,
    ) -> (
        HeldCreateClientScope,
        BrokerIdentityViewV1,
        KubernetesHeldCreateProtocolV1,
    ) {
        let scope = HeldCreateClientScope {
            gateway: "https://control.invalid".into(),
            tenant_id: "tenant-a".into(),
            control_namespace_uuid: uuid::Uuid::from_u128(1).to_string(),
            broker_id: uuid::Uuid::from_u128(2).to_string(),
            credential_id: uuid::Uuid::from_u128(3).to_string(),
            incarnation_id: uuid::Uuid::from_u128(4).to_string(),
        };
        let identity = BrokerIdentityViewV1 {
            version: 1,
            tenant_id: scope.tenant_id.clone(),
            control_namespace_uuid: scope.control_namespace_uuid.clone(),
            schema_version,
            broker_id: scope.broker_id.clone(),
            incarnation_id: scope.incarnation_id.clone(),
            credential_id: scope.credential_id.clone(),
            binding_revision: 1,
            grant_revision: 2,
        };
        let protocol = KubernetesHeldCreateProtocolV1 {
            version: 1,
            schema_version,
            tenant_id: identity.tenant_id.clone(),
            control_namespace_uuid: identity.control_namespace_uuid.clone(),
            broker_id: identity.broker_id.clone(),
            incarnation_id: identity.incarnation_id.clone(),
            binding_revision: identity.binding_revision,
            grant_revision: identity.grant_revision,
            admission_policy_configured: false,
            operations: vec![
                KubernetesHeldCreateOperationV1::Reserve,
                KubernetesHeldCreateOperationV1::Claim,
                KubernetesHeldCreateOperationV1::ObserveClaim,
                KubernetesHeldCreateOperationV1::Result,
                KubernetesHeldCreateOperationV1::Cancel,
            ],
        };
        (scope, identity, protocol)
    }

    #[test]
    fn schema11_accepts_only_matching_supported_held_create_v1_negotiation() {
        for schema_version in [9, 10, 11] {
            let (scope, identity, view) = negotiation(schema_version);
            assert_eq!(protocol(&scope, &identity, &view), Ok(()));
        }
        for schema_version in [0, 8, 12, u32::MAX] {
            let (scope, identity, view) = negotiation(schema_version);
            assert_eq!(
                protocol(&scope, &identity, &view),
                Err(HeldCreateClientError::Protocol)
            );
        }
        let (scope, identity, mut view) = negotiation(11);
        view.schema_version = 10;
        assert_eq!(
            protocol(&scope, &identity, &view),
            Err(HeldCreateClientError::Protocol)
        );
    }

    #[test]
    fn schema11_preserves_held_create_wire_scope_revision_and_operations() {
        let (scope, identity, view) = negotiation(11);
        assert_eq!(protocol(&scope, &identity, &view), Ok(()));
        let mut invalid = identity.clone();
        invalid.version = 2;
        assert_eq!(
            protocol(&scope, &invalid, &view),
            Err(HeldCreateClientError::Protocol)
        );
        let mut invalid = view.clone();
        invalid.version = 2;
        assert_eq!(
            protocol(&scope, &identity, &invalid),
            Err(HeldCreateClientError::Protocol)
        );
        let mut invalid = identity.clone();
        invalid.binding_revision = 0;
        assert_eq!(
            protocol(&scope, &invalid, &view),
            Err(HeldCreateClientError::Protocol)
        );
        let mut invalid = identity.clone();
        invalid.grant_revision = i64::MAX as u64 + 1;
        assert_eq!(
            protocol(&scope, &invalid, &view),
            Err(HeldCreateClientError::Protocol)
        );
        let mut invalid = identity.clone();
        invalid.tenant_id = "tenant-b".into();
        assert_eq!(
            protocol(&scope, &invalid, &view),
            Err(HeldCreateClientError::InvalidContext)
        );
        let mut invalid = view.clone();
        invalid.control_namespace_uuid = uuid::Uuid::from_u128(5).to_string();
        assert_eq!(
            protocol(&scope, &identity, &invalid),
            Err(HeldCreateClientError::Protocol)
        );
        let mut invalid = view.clone();
        invalid.operations.pop();
        assert_eq!(
            protocol(&scope, &identity, &invalid),
            Err(HeldCreateClientError::Protocol)
        );
    }
}
