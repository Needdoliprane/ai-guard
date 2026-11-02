//! Owner transfer regressions use the real retained FD/core, without a forged Q.
use super::*;

#[test]
fn pre_factory_recovery_core_transfers_once_with_same_mutex_and_lease() {
    let (root, journal, job) = super::super::retirement_tests::accepted();
    let scope = journal.scope().clone();
    drop(job);
    drop(journal);
    let directory = Directory::open_existing(&root.path().canonicalize().unwrap()).unwrap();
    let report = StopRecoveryReaderV1::open_existing(
        &directory,
        scope,
        &FsExclusions::new(),
        JournalLimitsV1::default(),
        ControlSecrets::new(Vec::new()).unwrap(),
    )
    .unwrap()
    .unwrap()
    .scan();
    let before = report.pre_factory_core().unwrap();
    let pointer = Arc::as_ptr(&before);
    let backend = admit_report(&report, true).unwrap();
    let ExistingProfileV1::Stops(stops) = backend else {
        panic!("explicit Stops");
    };
    assert_eq!(pointer, Arc::as_ptr(&stops.core));
    assert!(Arc::ptr_eq(&before, &stops.core));
    assert!(std::ptr::eq(&before.writer, &stops.core.writer));
    assert!(matches!(report.pre_factory_core(), Err(JournalError::Busy)));
    assert!(matches!(
        admit_report(&report, true),
        Err(JournalError::Busy)
    ));
}
