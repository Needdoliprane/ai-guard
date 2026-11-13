//! Data-affinity checks use real private role factories. They do not construct
//! retained reader/Cancel proofs or claim an engine STOP/quiescence result.
use super::*;
use crate::broker::{
    enforcer::ledger::RetirementLedger, retirement::RetirementDigestV1,
    retirement_checkpoint::Full3RegistrationBindingV1,
};
use std::os::unix::fs::PermissionsExt;

#[test]
fn classification_retains_exact_native_ledger_even_when_floor_is_partial() {
    use std::io::Write;
    let files = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap())
        .unwrap();
    let parent = Directory::open_existing(files.path()).unwrap();
    let owner =
        RetirementLedger::initialize_new(parent.create_enforcer_root("x").unwrap(), Uuid::new_v4())
            .unwrap();
    let root = owner.lease().ledger_root().clone();
    let expected = super::super::execution::classify_existing_sealed_namespace(&root).unwrap();
    assert_eq!(
        expected.native_binding(),
        &root.enforcer_recovery_binding().unwrap()
    );
    let mut fault = std::fs::OpenOptions::new()
        .append(true)
        .open(root.path().join(".owner.lock"))
        .unwrap();
    fault.write_all(&[0]).unwrap();
    fault.sync_all().unwrap();
    let partial = super::super::execution::classify_existing_sealed_namespace(&root).unwrap();
    assert!(partial.matches(&expected).unwrap());
    // This retained data selects only STOP affinity, never Full or Complete.
    StopAffinity::ExistingFull3(partial)
        .check_root(&root)
        .unwrap();
}

#[test]
fn stop_affinity_rejects_legacy_or_another_role_without_comparing_stop_epoch() {
    let files = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap())
        .unwrap();
    let parent = Directory::open_existing(files.path()).unwrap();
    let install = Uuid::new_v4();
    let owner_x =
        RetirementLedger::initialize_new(parent.create_enforcer_root("x").unwrap(), install)
            .unwrap();
    let owner_y =
        RetirementLedger::initialize_new(parent.create_enforcer_root("y").unwrap(), install)
            .unwrap();
    let x =
        super::super::execution::classify_existing_sealed_namespace(owner_x.lease().ledger_root())
            .unwrap();
    let y =
        super::super::execution::classify_existing_sealed_namespace(owner_y.lease().ledger_root())
            .unwrap();
    let arm = Uuid::new_v4();
    let hash = RetirementDigestV1::parse(&"a".repeat(64)).unwrap();
    let binding =
        Full3RegistrationBindingV1::new(arm, hash.clone(), x.clone(), Uuid::new_v4()).unwrap();
    assert!(
        StopAffinity::Legacy
            .check(&arm.to_string(), hash.as_str(), None)
            .is_ok()
    );
    assert_eq!(
        StopAffinity::Legacy.check(&arm.to_string(), hash.as_str(), Some(&binding)),
        Err(EnforcerError::Unauthorized)
    );
    let matching = StopAffinity::ExistingFull3(x);
    assert!(
        matching
            .check(&arm.to_string(), hash.as_str(), Some(&binding))
            .is_ok()
    );
    assert!(
        matching
            .clone()
            .check(&arm.to_string(), hash.as_str(), Some(&binding))
            .is_ok()
    );
    assert_eq!(
        matching.check(&arm.to_string(), hash.as_str(), None),
        Err(EnforcerError::Unauthorized)
    );
    assert_eq!(
        StopAffinity::ExistingFull3(y).check(&arm.to_string(), hash.as_str(), Some(&binding)),
        Err(EnforcerError::Conflict)
    );
    assert_eq!(
        matching.check(&Uuid::new_v4().to_string(), hash.as_str(), Some(&binding)),
        Err(EnforcerError::Conflict)
    );
    assert_eq!(
        matching.check(&arm.to_string(), &"b".repeat(64), Some(&binding)),
        Err(EnforcerError::Conflict)
    );
    assert_eq!(
        matching.check_root(owner_y.lease().ledger_root()),
        Err(EnforcerError::Conflict)
    );
    // No current STOP-epoch argument exists: an exact assignment remains valid
    // after a STOP-only reconnect, without producing Full or a historical Q.
}
