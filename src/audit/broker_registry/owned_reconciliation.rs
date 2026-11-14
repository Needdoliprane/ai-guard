//! Read-only exact operation recovery. Authentication and the current source
//! transaction precede every lookup, including replay and durable absence.
use super::*;
use super::owned_supervision::{
    OwnedBrokerWindow, OwnedProof, OwnedRetainedSourceFacts, classify, owned_check,
    latest_receipt, HEARTBEAT_KIND,
};
use crate::broker_registry::owned_supervision::*;
use crate::broker_registry::owned_supervision::OwnedSupervisionErrorCodeV2 as Code;
use crate::audit::workload::owned_broker_identity::checked_rotation_fence;

/// Global ID check. A present ID of another broker/type/hash is a conflict,
/// never an absence; no neighboring Store or unbounded history scan exists.
fn exact_operation(store: &Store, tx: &CheckedTransaction<'_>, change: &Change) -> OwnedChecked<Option<BrokerMutationResultV1>> {
    let row = tx.query_row(
        "SELECT input_hash,event_seq,broker_id,result_json FROM broker_registry_operations WHERE operation_id=?1",
        [change.operation_id()], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?)))
        .optional().map_err(|error| classify(store, &error.into()))?;
    let Some((input_hash, seq, broker, json)) = row else { return Ok(None); };
    let expected = change.input_hash().map_err(|_| Code::ProtocolMismatch)?;
    owned_check(broker == change.broker_id() && input_hash == expected, Code::OperationConflict)?;
    let stored = tx.query_row("SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE seq=?1", [seq], row_event)
        .map_err(|error| classify(store, &error.into()))?;
    let record: Record = serde_json::from_value(stored.event.data.clone()).map_err(|_| Code::AuditUnavailable)?;
    owned_check(stored.event.kind == change.kind() && record.change.kind() == change.kind()
        && record.change.operation_id() == change.operation_id()
        && record.change.broker_id() == change.broker_id()
        && record.version == record.change.record_version()
        && record.input_hash == input_hash
        && record.change.input_hash().map_err(|_| Code::AuditUnavailable)? == input_hash, Code::OperationConflict)?;
    record.change.validate().map_err(|error| classify(store, &error))?;
    let broker: BrokerV1 = serde_json::from_str(&json).map_err(|_| Code::AuditUnavailable)?;
    owned_check(broker.broker_id == change.broker_id(), Code::OperationConflict)?;
    Ok(Some(result(&stored, broker, input_hash, change.operation_id().into(), true)))
}

impl Store {
    pub(crate) fn owned_broker_reconcile(
        &self,
        access: &HumanSourceAccess<'_>,
        window: &OwnedBrokerWindow<'_>,
        facts: &OwnedRetainedSourceFacts,
        broker: &BrokerIdentityV1,
        request: &OwnedReconciliationRequestV2,
    ) -> OwnedChecked<OwnedReconciliationResultV2> {
        request.validate()?;
        let proof = OwnedProof::Windowed(window, broker);
        self.owned_business(access, &proof, false, |tx, clock| {
            // Source, same bearer/proof, current grant and control time have
            // authority; the old subject from the body selects no tenant.
            let current = self.owned_current(tx, clock, facts, broker)?;
            let retained = self.review_authority.as_ref().ok_or(Code::SourceRebuildRequired)?.source_token();
            let fence = checked_rotation_fence(tx, retained, request.identity(), &current.identity)
                .map_err(|error| {
                    if let Some(code) = error.downcast_ref::<Code>() { return *code; }
                    if error.downcast_ref::<crate::workload::WorkloadFailureV1>()
                        == Some(&crate::workload::WorkloadFailureV1::Conflict) {
                        Code::IdentityMismatch
                    } else { classify(self, &error) }
                })?;
            let old = &request.identity().subject;
            // Historical evidence only, used to recompute the original hash;
            // never a BrokerIdentity::authenticated or a mutation permission.
            let evidence = IdentityEvidence {
                tenant_id: old.tenant_id.clone(), broker_id: old.broker_id.clone(),
                credential_id: old.credential_id.clone(), binding_revision: old.binding_revision,
                grant_revision: old.grant_revision,
            };
            let change = match request {
                OwnedReconciliationRequestV2::Heartbeat { pending, .. } => Change::Heartbeat { identity: evidence, heartbeat: pending.heartbeat.clone() },
                OwnedReconciliationRequestV2::Checkpoint { pending, .. } => Change::Checkpoint { identity: evidence, checkpoint: pending.checkpoint.clone() },
            };
            self.owned_change_input(&change)?;
            owned_check(change.input_hash().map_err(|_| Code::ProtocolMismatch)? == request.registry_input_hash(), Code::ProtocolMismatch)?;
            let historical = exact_operation(self, tx, &change)?;
            let view = current.broker.view_at(&current.evaluated_at).map_err(|error| classify(self, &error))?;
            let head = match request {
                OwnedReconciliationRequestV2::Heartbeat { .. } => OwnedReconciliationHeadV2::Heartbeat {
                    view,
                    receipt: latest_receipt(tx, broker.broker_id(), HEARTBEAT_KIND).map_err(|error| classify(self, &error))?,
                },
                OwnedReconciliationRequestV2::Checkpoint { .. } => {
                    let observed = checkpoint::latest(tx, broker.broker_id()).map_err(|error| classify(self, &error))?
                        .map(|retained| checkpoint::observation(retained, &current.broker, &current.evaluated_at))
                        .transpose().map_err(|error| classify(self, &error))?;
                    let current = match observed {
                        Some(value) if value.in_current_grant => Some(Box::new(BrokerCheckpointResultV1 { checkpoint: value.checkpoint, receipt: value.receipt, replayed: true })),
                        Some(value) if value.checkpoint.incarnation_id == old.incarnation_id => return Err(Code::OperationConflict),
                        Some(_) | None => None,
                    };
                    OwnedReconciliationHeadV2::Checkpoint { view, current }
                }
            };
            let state = match (request, historical) {
                (_, None) => OwnedReconciliationStateV2::NonCommitted,
                (OwnedReconciliationRequestV2::Heartbeat { pending, .. }, Some(result)) => OwnedReconciliationStateV2::CommittedExact {
                    historical: OwnedHistoricalResultV2::Heartbeat { result: Box::new(OwnedBrokerHeartbeatResultV2::new(
                        pending.identity.clone(), pending.operation_commitment.clone(), result)?) },
                },
                (OwnedReconciliationRequestV2::Checkpoint { pending, .. }, Some(result)) => OwnedReconciliationStateV2::CommittedExact {
                    historical: OwnedHistoricalResultV2::Checkpoint { result: Box::new(OwnedBrokerCheckpointResultV2::new(
                        pending.identity.clone(), pending.operation_commitment.clone(), BrokerCheckpointResultV1 {
                            checkpoint: pending.checkpoint.clone(), receipt: result.receipt, replayed: true,
                        })?) },
                },
            };
            let value = OwnedReconciliationResultV2 {
                profile: OWNED_SUPERVISION_PROFILE_V2.into(), version: OWNED_SUPERVISION_VERSION_V2,
                request_commitment: request.commitment()?, observation: current.identity, fence, head, state,
            };
            value.validate_for(request)?;
            self.owned_public(&value)?;
            Ok(value)
        })
    }
}

#[cfg(test)]
mod tests;
