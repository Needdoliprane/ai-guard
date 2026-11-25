//! Explicit admission authority retained from service selection to execution.
//! Loaded Full3 assignments require the exact authenticated role binding;
//! there is no conversion to a legacy client or implicit role upgrade.

use super::{
    CancelGeneration, EnforcedSlot, EnforcerClient, EnforcerError, EnforcerFullClientV3,
    EnforcerResult, EnforcerStatusV1, LegacyReservationRefusalV1, ProspectiveRegistrationV1,
    RegistrationV1, client::ProspectiveArmExchange,
};
use crate::{
    broker::{
        retirement::{GenesisCoreV1, LedgerIdentityV1},
        retirement_checkpoint::Full3RegistrationBindingV1,
    },
    control_fs::Directory,
};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub(crate) enum ExecutionEnforcerV1 {
    Legacy(EnforcerClient),
    Full3(EnforcerFullClientV3),
}

impl From<EnforcerClient> for ExecutionEnforcerV1 {
    fn from(client: EnforcerClient) -> Self {
        Self::Legacy(client)
    }
}

impl ExecutionEnforcerV1 {
    pub(crate) fn require_new_prearm_identity(&self) -> EnforcerResult<()> {
        match self {
            Self::Legacy(client) => client.legacy_require_new_prearm_identity(),
            Self::Full3(client) => client.require_new_prearm_identity(),
        }
    }
    pub(crate) fn fence_unknown_prearm_write(&self) -> EnforcerResult<()> {
        match self {
            Self::Legacy(client) => client.legacy_fence_unknown_prearm_write(),
            Self::Full3(client) => client.fence_unknown_prearm_write(),
        }
    }
    pub(crate) async fn reserve_prospective(
        &self,
        target: ProspectiveRegistrationV1,
        generation: CancelGeneration,
        replay_only: bool,
    ) -> EnforcerResult<()> {
        self.reserve_prospective_diagnosed(target, generation, replay_only)
            .await
            .map_err(|refusal| refusal.code())
    }
    /// Same single attempt, retaining the closed Legacy refusal motif when the
    /// authenticated peer negotiated support on this attempt's connection.
    /// Full3 keeps its own closed codes; no motif is invented for it.
    pub(crate) async fn reserve_prospective_diagnosed(
        &self,
        target: ProspectiveRegistrationV1,
        generation: CancelGeneration,
        replay_only: bool,
    ) -> Result<(), LegacyReservationRefusalV1> {
        match self {
            Self::Legacy(client) => {
                client
                    .legacy_reserve_prospective_diagnosed(target, generation, replay_only)
                    .await
            }
            Self::Full3(client) => client
                .reserve_prospective(target, generation, replay_only)
                .await
                .map_err(LegacyReservationRefusalV1::from),
        }
    }
    pub(crate) async fn begin_prospective_arm_exchange(
        &self,
        target: &ProspectiveRegistrationV1,
        generation: &CancelGeneration,
    ) -> EnforcerResult<ProspectiveArmExchange> {
        match self {
            Self::Legacy(client) => {
                client
                    .legacy_begin_prospective_arm_exchange(target, generation)
                    .await
            }
            Self::Full3(client) => {
                client
                    .begin_prospective_arm_exchange(target, generation)
                    .await
            }
        }
    }
    pub(crate) async fn bind_prospective(
        &self,
        registration: RegistrationV1,
        generation: CancelGeneration,
        exchange: ProspectiveArmExchange,
    ) -> EnforcerResult<EnforcedSlot> {
        match self {
            Self::Legacy(client) => {
                client
                    .legacy_bind_prospective(registration, generation, exchange)
                    .await
            }
            Self::Full3(client) => {
                client
                    .bind_prospective(registration, generation, exchange)
                    .await
            }
        }
    }
    pub(crate) fn full3(&self) -> Option<&EnforcerFullClientV3> {
        match self {
            Self::Full3(client) => Some(client),
            Self::Legacy(_) => None,
        }
    }

    pub(crate) async fn require_admission_ready(&self) -> EnforcerResult<()> {
        match self {
            Self::Legacy(client) => client.require_admission_ready().await,
            Self::Full3(client) => client.require_admission_ready().await,
        }
    }

    pub(crate) async fn status(&self) -> EnforcerResult<EnforcerStatusV1> {
        match self {
            Self::Legacy(client) => client.status().await,
            Self::Full3(client) => client.status().await,
        }
    }

    pub(crate) async fn register(
        &self,
        registration: RegistrationV1,
        generation: CancelGeneration,
    ) -> EnforcerResult<EnforcedSlot> {
        match self {
            Self::Legacy(client) => client.register(registration, generation).await,
            Self::Full3(client) => client.register(registration, generation).await,
        }
    }

    /// Historical Restore remains a legacy operation. Full3 refuses it because
    /// the loaded Arm has no explicit assignment proof at this call boundary.
    pub(crate) async fn restore(
        &self,
        registration: RegistrationV1,
        generation: CancelGeneration,
    ) -> EnforcerResult<EnforcedSlot> {
        match self {
            Self::Legacy(client) => client.restore(registration, generation).await,
            Self::Full3(client) => client.restore(registration, generation).await,
        }
    }

    pub(crate) async fn restore_with_binding(
        &self,
        registration: RegistrationV1,
        generation: CancelGeneration,
        binding: &Full3RegistrationBindingV1,
    ) -> EnforcerResult<EnforcedSlot> {
        match self {
            Self::Full3(client) => {
                client
                    .restore_with_binding(registration, generation, binding)
                    .await
            }
            Self::Legacy(_) => Err(EnforcerError::Unauthorized),
        }
    }

    pub(crate) async fn cancel_retained(
        &self,
        registration: RegistrationV1,
        generation: CancelGeneration,
    ) -> EnforcerResult<()> {
        match self {
            Self::Legacy(client) => client.cancel_retained(registration, generation).await,
            Self::Full3(_) => Err(EnforcerError::Unauthorized),
        }
    }

    pub(crate) async fn cancel_retained_bound(
        &self,
        registration: RegistrationV1,
        generation: CancelGeneration,
        binding: &Full3RegistrationBindingV1,
    ) -> EnforcerResult<()> {
        match self {
            Self::Full3(client) => {
                client
                    .cancel_retained_bound(registration, generation, binding)
                    .await
            }
            Self::Legacy(_) => Err(EnforcerError::Unauthorized),
        }
    }
}

/// Data-only equality check. The caller derives expected values from its live
/// Full client and validated registration; this does not create a capability.
pub(super) fn validate_assignment(
    ledger: &LedgerIdentityV1,
    epoch: Uuid,
    arm_id: &str,
    registration_hash: &str,
    binding: &Full3RegistrationBindingV1,
) -> EnforcerResult<()> {
    if binding.original_epoch() != epoch
        || binding.arm_id().to_string() != arm_id
        || binding.registration_hash().as_str() != registration_hash
        || !binding
            .ledger()
            .matches(ledger)
            .map_err(|_| EnforcerError::InvalidInput)?
    {
        return Err(EnforcerError::Conflict);
    }
    Ok(())
}

/// One-time explicit existing-namespace classification, not a verified seal,
/// ledger scan or Full permission. Torn seal/floor bytes remain STOP Unverified.
/// Existing STOP transports never call this preselection check on delivery.
pub(super) fn classify_existing_sealed_namespace(
    root: &Directory,
) -> EnforcerResult<LedgerIdentityV1> {
    let unsafe_fs = |_| EnforcerError::UnsafeFilesystem;
    let native = root.enforcer_recovery_binding().map_err(unsafe_fs)?;
    let namespace = root
        .persistent_child("retirement", false)
        .map_err(unsafe_fs)?
        .ok_or(EnforcerError::Unauthorized)?;
    // This exact closed record is small: UUIDs, SHA256 and six native scalars.
    let genesis_file = namespace
        .persistent_read("genesis.json", 4096, &[])
        .map_err(unsafe_fs)?
        .ok_or(EnforcerError::Unauthorized)?;
    let genesis = GenesisCoreV1::from_canonical_bytes(genesis_file.bytes())
        .map_err(|_| EnforcerError::UnsafeFilesystem)?;
    if genesis.native_binding() != &native {
        return Err(EnforcerError::Conflict);
    }
    // Use the CFS checked descriptor path, not a pathname metadata guess or a
    // second lease. The read is bounded; no floors/deltas are parsed or repaired.
    let lock = root
        .persistent_read(".owner.lock", 8 * 1024 * 1024, &[])
        .map_err(unsafe_fs)?
        .ok_or(EnforcerError::Unauthorized)?;
    if lock.bytes().is_empty() {
        return Err(EnforcerError::Unauthorized);
    }
    genesis_file.persistent_revalidate().map_err(unsafe_fs)?;
    lock.persistent_revalidate().map_err(unsafe_fs)?;
    if root.enforcer_recovery_binding().map_err(unsafe_fs)? != native {
        return Err(EnforcerError::Conflict);
    }
    LedgerIdentityV1::from_genesis(&genesis).map_err(|_| EnforcerError::UnsafeFilesystem)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::broker::retirement::{GenesisCoreV1, RetirementDigestV1};
    use std::{io::Write, os::unix::fs::PermissionsExt};

    #[test]
    fn existing_namespace_refuses_legacy_or_preseal_but_does_not_require_complete_floors() {
        use crate::broker::{discovery::RoleLease, enforcer::ledger::RetirementLedger};
        let files = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap())
            .unwrap();
        let parent = Directory::open_existing(files.path()).unwrap();
        let legacy = parent
            .persistent_child("another-legacy-role", true)
            .unwrap()
            .unwrap();
        let _legacy_owner = RoleLease::acquire(&legacy).unwrap();
        assert!(matches!(
            classify_existing_sealed_namespace(&legacy),
            Err(EnforcerError::Unauthorized)
        ));
        let namespace = legacy
            .persistent_child("retirement", true)
            .unwrap()
            .unwrap();
        let genesis = GenesisCoreV1::new(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            legacy.enforcer_recovery_binding().unwrap(),
        )
        .unwrap();
        namespace
            .persistent_create("genesis.json", &genesis.canonical_bytes().unwrap())
            .unwrap();
        assert!(matches!(
            classify_existing_sealed_namespace(&legacy),
            Err(EnforcerError::Unauthorized)
        ));
        let ledger = RetirementLedger::initialize_new(
            parent.create_enforcer_root("new-role").unwrap(),
            Uuid::new_v4(),
        )
        .unwrap();
        let root = ledger.lease().ledger_root().clone();
        assert!(classify_existing_sealed_namespace(&root).is_ok());
        let mut fault = std::fs::OpenOptions::new()
            .append(true)
            .open(root.path().join(".owner.lock"))
            .unwrap();
        fault.write_all(&[0, 0]).unwrap();
        fault.sync_all().unwrap();
        assert!(
            classify_existing_sealed_namespace(&root).is_ok(),
            "a torn floor cannot be treated as Full or close structural STOP selection"
        );
    }

    #[test]
    fn loaded_assignment_requires_exact_role_genesis_arm_registration_and_epoch() {
        let installation = Uuid::new_v4();
        let genesis = GenesisCoreV1::new(
            installation,
            Uuid::new_v4(),
            Uuid::new_v4(),
            [1, 2, 3, 4, 5, 6],
        )
        .unwrap();
        let ledger = LedgerIdentityV1::from_genesis(&genesis).unwrap();
        let arm = Uuid::new_v4();
        let epoch = Uuid::new_v4();
        let hash = RetirementDigestV1::parse(&"a".repeat(64)).unwrap();
        let binding =
            Full3RegistrationBindingV1::new(arm, hash.clone(), ledger.clone(), epoch).unwrap();
        assert!(
            validate_assignment(&ledger, epoch, &arm.to_string(), hash.as_str(), &binding).is_ok()
        );
        assert_eq!(
            validate_assignment(
                &ledger,
                Uuid::new_v4(),
                &arm.to_string(),
                hash.as_str(),
                &binding
            ),
            Err(EnforcerError::Conflict)
        );
        assert_eq!(
            validate_assignment(
                &ledger,
                epoch,
                &Uuid::new_v4().to_string(),
                hash.as_str(),
                &binding
            ),
            Err(EnforcerError::Conflict)
        );
        assert_eq!(
            validate_assignment(&ledger, epoch, &arm.to_string(), &"b".repeat(64), &binding),
            Err(EnforcerError::Conflict)
        );
        let different_role = GenesisCoreV1::new(
            installation,
            Uuid::new_v4(),
            Uuid::new_v4(),
            [1, 2, 3, 4, 5, 6],
        )
        .unwrap();
        let other_ledger = LedgerIdentityV1::from_genesis(&different_role).unwrap();
        assert_eq!(
            validate_assignment(
                &other_ledger,
                epoch,
                &arm.to_string(),
                hash.as_str(),
                &binding
            ),
            Err(EnforcerError::Conflict)
        );
        let changed_native = GenesisCoreV1::new(
            installation,
            *ledger.role_id(),
            *ledger.init_id(),
            [1, 2, 3, 4, 5, 7],
        )
        .unwrap();
        let changed_native = LedgerIdentityV1::from_genesis(&changed_native).unwrap();
        assert_eq!(
            validate_assignment(
                &changed_native,
                epoch,
                &arm.to_string(),
                hash.as_str(),
                &binding
            ),
            Err(EnforcerError::Conflict)
        );
    }
}
