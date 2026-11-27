use super::PendingPredecessor;
use std::sync::Arc;

#[test]
fn failed_reap_retains_exact_predecessor_for_replay_and_blocks_rotation() {
    let pending = PendingPredecessor::default();
    let predecessor = Arc::new(String::from("old-channel"));
    pending.install(Arc::clone(&predecessor)).unwrap();

    let failed = pending.reap(|actual| {
        assert!(std::ptr::eq(actual, predecessor.as_ref()));
        anyhow::bail!("deterministic reap failure")
    });
    assert!(failed.is_err());
    assert!(pending.is_pending());
    assert!(
        pending
            .install(Arc::new(String::from("second-old-channel")))
            .is_err()
    );

    let replay = pending.reap(|actual| {
        assert!(std::ptr::eq(actual, predecessor.as_ref()));
        Ok(())
    });
    assert!(replay.is_ok());
    assert!(!pending.is_pending());

    pending
        .install(Arc::new(String::from("second-old-channel")))
        .unwrap();
}
