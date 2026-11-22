//! A retained technical inspection proof, distinct from policy write authority.
use super::*;
use crate::control::egress_policy::{EgressPolicyInspectionRequestV1, EgressPolicyInspectionV1};

pub(in crate::api) struct OwnedPolicyInspection {
    _operation: OwnedSemaphorePermit,
    identity: OwnedProtocolIdentity,
}

impl OwnedPolicyIdentity {
    pub(in crate::api) fn accept_inspection(
        self,
        operation: OwnedSemaphorePermit,
    ) -> OwnedPolicyInspection {
        OwnedPolicyInspection {
            identity: self.0,
            _operation: operation,
        }
    }
}

impl OwnedPolicyInspection {
    pub(in crate::api) async fn inspect(
        self: Arc<Self>,
        request: EgressPolicyInspectionRequestV1,
    ) -> ApiResult<EgressPolicyInspectionV1> {
        request
            .validate()
            .map_err(super::super::super::egress_policy::failure)?;
        let owned = self.identity.owned.clone();
        let digest = self.identity.digest;
        let shared = self.identity.state.io.clone();
        let tenant_io = self.identity.tenant.io.clone();
        let observation = store_io::run_retaining(
            shared,
            Some(tenant_io),
            store_io::Lane::Control,
            false,
            self.identity._usage.clone(),
            move || {
                // Capturing this Arc keeps the real operation, credential,
                // registry, source and usage until the actual job completes.
                let identity = &self.identity;
                let owned = &identity.owned;
                if !owned.technical_candidate(&identity.digest)? {
                    return Err(human_sources::unavailable());
                }
                identity
                    .credential
                    .revalidate()
                    .map_err(policy_store_failure)?;
                let source = owned
                    .sources
                    .get(identity.index)
                    .ok_or_else(human_sources::unavailable)?;
                let (observation, expected) = {
                    // Only this accepted blocking job owns the fresh ReadEpoch.
                    // Nothing source-bound is held across an async boundary.
                    let epoch = owned
                        .owner
                        .try_read_epoch()
                        .map_err(policy_authority_failure)?;
                    let access =
                        HumanSourceAccess::read(&epoch, source).map_err(policy_store_failure)?;
                    let expected = owned
                        .discovery_generation(identity.index)
                        .ok_or_else(human_sources::unavailable)?;
                    let result = identity.tenant.store.modern_egress_policy_inspection(
                        &access,
                        &identity.credential,
                        expected,
                        &request,
                    );
                    if result.as_ref().err().is_some_and(|error| {
                        error.downcast_ref::<crate::audit::egress::EgressStoreFailure>()
                            == Some(&crate::audit::egress::EgressStoreFailure::DiscoveryStale)
                    }) {
                        owned.require_rebuild();
                    }
                    let observation = result.map_err(policy_store_failure)?;
                    epoch
                        .validate_source(source)
                        .map_err(policy_authority_failure)?;
                    epoch.check().map_err(policy_authority_failure)?;
                    (observation, expected)
                }; // True commit, SQLite timeout restoration, then ReadEpoch end.
                #[cfg(test)]
                test_hook::pause(&identity.state)?;

                // This second short checked read verifies publication authority.
                // It deliberately never refreshes or replaces the observed head.
                identity
                    .credential
                    .revalidate()
                    .map_err(policy_store_failure)?;
                let epoch = owned
                    .owner
                    .try_read_epoch()
                    .map_err(policy_authority_failure)?;
                let access =
                    HumanSourceAccess::read(&epoch, source).map_err(policy_store_failure)?;
                let current_generation = owned
                    .discovery_generation(identity.index)
                    .ok_or_else(human_sources::unavailable)?;
                let current = identity
                    .tenant
                    .store
                    .modern_control_protocol(&access, &identity.credential, current_generation)
                    .map_err(policy_store_failure)?;
                let Some(current) = current else {
                    owned.require_rebuild();
                    return Err(human_sources::unavailable());
                };
                if current_generation != expected
                    || current.schema_version != 15
                    || current.control_namespace_uuid.as_deref()
                        != Some(observation.scope().control_namespace_uuid())
                    || identity.tenant.tenant_id != observation.scope().tenant_id()
                    || !owned.technical_candidate(&identity.digest)?
                {
                    return Err(human_sources::unavailable());
                }
                epoch
                    .validate_source(source)
                    .map_err(policy_authority_failure)?;
                epoch.check().map_err(policy_authority_failure)?;
                Ok(observation)
            },
        )
        .await?;
        if !owned.healthy() || !owned.technical_candidate(&digest)? {
            return Err(human_sources::unavailable());
        }
        Ok(observation)
    }
}
