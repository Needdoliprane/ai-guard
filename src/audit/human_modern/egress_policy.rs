//! Technical A0-style read: one actual owned15 checked transaction, no Event.
use super::*;
use crate::{
    api::AuthenticatedReviewRunnerCredential,
    control::egress_policy::{EgressPolicyInspectionRequestV1, EgressPolicyInspectionV1},
};

impl Store {
    pub(crate) fn modern_egress_policy_inspection(
        &self,
        access: &HumanSourceAccess<'_>,
        credential: &AuthenticatedReviewRunnerCredential,
        expected_generation: HumanDiscoveryGeneration,
        request: &EgressPolicyInspectionRequestV1,
    ) -> anyhow::Result<EgressPolicyInspectionV1> {
        let HumanSourceAccess::Read { epoch, source } = access else {
            return Err(egress::EgressStoreFailure::Permission.into());
        };
        self.with_modern_read(access, |tx| {
            tx.require_business_access()?;
            egress::require_inspection_format(tx)?;
            let binding = self.verify_review_binding_in_transaction(tx, epoch, source, credential)?;
            if discovery_generation(tx)? != expected_generation {
                return Err(egress::EgressStoreFailure::DiscoveryStale.into());
            }
            let clock = workload::capture_control_time(tx)?;
            let observation = self.egress_policy_inspection_checked(tx, request, &clock)?;
            // These checks precede the true commit in with_modern_read. They
            // never grant mutation authority or advance a durable clock floor.
            binding.revalidate()?;
            access.check(StoreOperationClass::Business)?;
            egress::require_inspection_format(tx)?;
            if discovery_generation(tx)? != expected_generation {
                return Err(egress::EgressStoreFailure::DiscoveryStale.into());
            }
            let value = serde_json::to_value(&observation)?;
            anyhow::ensure!(self.redactor.value(&value) == value, "inspection metadata is redacted");
            workload::validate_fresh_control_time(tx, &clock)?;
            tx.require_business_access()?;
            drop(binding);
            Ok(observation)
        })
    }
}
