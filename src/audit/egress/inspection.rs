//! Narrow registry metadata from one already checked Business transaction.
//! No policy body, human authority, audit append or execution permit is exposed.
use super::*;
use crate::control::egress_policy::{
    EgressPolicyExpectedScopeV1, EgressPolicyInspectionRequestV1, EgressPolicyInspectionV1,
    EgressPolicyObservedCeilingV1 as CeilingObservation,
    EgressPolicyObservedHeadV1 as HeadObservation,
    EgressPolicyObservedSessionResourceV1 as ResourceObservation,
};

pub(in crate::audit) fn require_inspection_format(tx: &CheckedTransaction<'_>) -> anyhow::Result<()> {
    tx.require_business_access()?;
    require15(tx)
}

impl Store {
    pub(in crate::audit) fn egress_policy_inspection_checked(
        &self,
        tx: &CheckedTransaction<'_>,
        request: &EgressPolicyInspectionRequestV1,
        clock: &workload::AuthorizedControlTimeV1,
    ) -> anyhow::Result<EgressPolicyInspectionV1> {
        tx.require_business_access()?;
        require15(tx)?;
        request.validate().map_err(|_| EgressStoreFailure::Invalid)?;
        let projection = replay(tx)?;
        let selector = request.selector();
        let head = match projection.heads.get(selector) {
            Some(head) => HeadObservation::Present {
                revision: head.policy.revision(),
                epoch: head.epoch,
                state: head.state,
            },
            None => HeadObservation::Absent,
        };
        let (ceiling, resource) = match selector {
            EgressPolicySelectorV1::Tenant => (
                CeilingObservation::NotApplicable,
                ResourceObservation::NotApplicable,
            ),
            EgressPolicySelectorV1::Session(id) => {
                let ceiling = match projection.heads.get(&EgressPolicySelectorV1::Tenant) {
                    None => CeilingObservation::Absent,
                    Some(head) if head.state == EgressPolicyStateV1::Active => {
                        CeilingObservation::Active {
                            revision: head.policy.revision(),
                            epoch: head.epoch,
                            digest: head.policy.digest().into(),
                        }
                    }
                    Some(head) => CeilingObservation::Inactive {
                        revision: head.policy.revision(),
                        epoch: head.epoch,
                    },
                };
                let resource = crate::identity::ResourceRefV1 {
                    kind: crate::identity::ResourceKindV1::Session,
                    resource_id: id.clone(),
                };
                let presence = match current_session_grant(tx, &resource)? {
                    None => ResourceObservation::Absent,
                    Some(grant) => {
                        grant.validate()?;
                        ensure(grant.tenant_id == self.tenant_id() && grant.resource == resource)?;
                        if grant.active {
                            ResourceObservation::Active
                        } else {
                            ResourceObservation::Inactive
                        }
                    }
                };
                (ceiling, presence)
            }
        };
        let namespace = crate::audit::control::supervision::namespace(tx)?;
        let scope = EgressPolicyExpectedScopeV1::new(self.tenant_id(), &namespace)
            .map_err(|_| EgressStoreFailure::Integrity)?;
        let observation = EgressPolicyInspectionV1::observed(
            scope,
            selector.clone(),
            head,
            ceiling,
            resource,
            crate::broker_registry::format_time(clock.at()),
        )
        .map_err(|_| EgressStoreFailure::Integrity)?;
        // A collision in the configured dictionary refuses the exact report;
        // alteration would break its canonical scope/CAS and historical value.
        let value = serde_json::to_value(&observation)?;
        ensure(self.redactor.value(&value) == value)?;
        tx.require_business_access()?;
        Ok(observation)
    }
}
