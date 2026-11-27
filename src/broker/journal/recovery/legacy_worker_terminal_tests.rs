use super::super::*;
use crate::{
    broker::journal::retirement_live_fixture::live_fixture,
    runner::broker::{ColdRecoverySeed, terminal_pending_seed_for_cold_audit_test},
};

#[tokio::test]
async fn reopens_the_exact_legacy_worker_claim_at_a_confirmed_stop() {
    let seed = terminal_pending_seed_for_cold_audit_test().await;
    tokio::task::spawn_blocking(move || check_reopened_projection(&seed))
        .await
        .unwrap();
}

fn check_reopened_projection(seed: &ColdRecoverySeed) {
    let directory = Directory::open_existing(seed.journal_root()).unwrap();
    let report = StopRecoveryReaderV1::open_existing(
        &directory,
        seed.scope().clone(),
        &FsExclusions::new(),
        JournalLimitsV1::default(),
        ControlSecrets::new(Vec::new()).unwrap(),
    )
    .unwrap()
    .unwrap()
    .scan();
    let ExistingProfileV1::Stops(stops) = admit_report(&report, true).unwrap() else {
        panic!("expected the retained Stops journal");
    };
    let operation = seed.operation_id();
    let before = stops.recovery_status(operation).unwrap().unwrap();
    let runtime_calls = seed.runtime_calls();
    let proof = stops
        .legacy_worker_terminal_evidence(operation, seed.cancel_id())
        .unwrap();
    assert_eq!(proof.operation_id(), operation);
    assert_eq!(proof.head(), before.head());
    assert_eq!(
        canonical(proof.scope()).unwrap(),
        canonical(seed.scope()).unwrap()
    );
    let stop = before
        .cancel_receipts()
        .iter()
        .find(|receipt| receipt.command_id() == seed.cancel_id())
        .unwrap();
    assert_eq!(proof.cancel_receipt().head(), stop.head());
    assert_eq!(proof.cancel_receipt().input_hash(), stop.input_hash());
    assert_eq!(proof.cancel_receipt().command_id(), seed.cancel_id());
    assert!(proof.head().revision() > stop.head().revision());

    let history = seed.arm_claim_snapshot().as_array().unwrap();
    let expected_arm = history
        .iter()
        .find(|op| op["kind"] == "arm" && op["request"]["subject"]["kind"] == "worker")
        .unwrap();
    let expected_claim = history
        .iter()
        .find(|op| op["kind"] == "worker_claim")
        .unwrap();
    let (arm, arm_ack) = proof.arm();
    let (claim, claim_ack) = proof.claim();
    assert_eq!(serde_json::to_value(arm).unwrap(), expected_arm["request"]);
    assert_eq!(
        serde_json::to_value(arm_ack).unwrap(),
        expected_arm["response"]["receipt"]
    );
    assert_eq!(
        serde_json::to_value(claim).unwrap(),
        expected_claim["request"]
    );
    assert_eq!(
        serde_json::to_value(claim_ack).unwrap(),
        expected_claim["response"]["receipt"]
    );
    let (authority, owned) = proof.target();
    assert_eq!(
        authority.authority_id.to_string(),
        arm.inspection.authority_id
    );
    assert_eq!(owned.handle.id(), seed.container_id());
    assert_eq!(serde_json::to_value(owned).unwrap(), expected_arm["owned"]);
    assert_eq!(proof.worker().worker_id, seed.worker_id());
    assert_eq!(proof.worker().attempt.attempt_id, seed.attempt_id());
    assert!(proof.worker().observed_started_at.is_some());

    let unrecorded_command = Uuid::new_v4();
    assert!(matches!(
        stops.legacy_worker_terminal_evidence(operation, unrecorded_command),
        Err(JournalError::Interrupted)
    ));
    let after = stops.recovery_status(operation).unwrap().unwrap();
    assert_eq!(after.head(), before.head());
    assert_eq!(
        after.cancel_receipts().len(),
        before.cancel_receipts().len()
    );
    assert_eq!(seed.runtime_calls(), runtime_calls);
}

#[test]
fn confirmed_stop_for_an_import_arm_supplies_no_worker_claim_evidence() {
    let fixture = live_fixture();
    let operation = fixture.intent.operation_id();
    let command = Uuid::new_v4();
    let stops = StopJournal::from_full(&fixture.journal);
    let confirmed = stops.append_cancel(operation, command).unwrap();
    assert_eq!(confirmed.receipt().command_id(), command);
    assert!(matches!(
        stops.legacy_worker_terminal_evidence(operation, command),
        Err(JournalError::Interrupted)
    ));
    assert_eq!(
        stops.recovery_status(operation).unwrap().unwrap().head(),
        confirmed.receipt().head()
    );
}
