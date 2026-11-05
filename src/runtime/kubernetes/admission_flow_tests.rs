//! Real Store/Journal/proofs, synthetic safe Pod facts, no network or CREATE.
use super::{journal::*, *};
use crate::audit::kubernetes_admission_state_tests::Fixture;
use crate::control::{QuotaPolicyV1, kubernetes_admission::*};
use crate::control_fs::Directory;
use serde_json::{Value, json};
use std::fs;
use uuid::Uuid;

const EXPIRY: &str = "2100-01-01T00:00:00.000Z";
const POD_UID: &str = "11111111-1111-4111-8111-111111111119";

fn journal_root(f: &Fixture) -> Directory {
    Directory::open_existing(f._directory.path())
        .unwrap()
        .persistent_child("private-held-create", true)
        .unwrap()
        .unwrap()
}
fn protection() -> AdmissionJournalProtection {
    AdmissionJournalProtection::new(Vec::new(), Vec::new())
}
fn claim(intent: &KubernetesCreateIntentV1) -> KubernetesCreateClaimV1 {
    KubernetesCreateClaimV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        create_id: intent.create_id.clone(),
        intent_hash: intent.intent_hash().unwrap(),
        expected_revision: 1,
        expected_create_epoch: 1,
        emitter_id: Uuid::new_v4().to_string(),
    }
}
fn result(
    intent: &KubernetesCreateIntentV1,
    claim: &KubernetesCreateClaimV1,
    outcome: KubernetesCreateOutcomeV1,
) -> KubernetesCreateResultV1 {
    KubernetesCreateResultV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        create_id: intent.create_id.clone(),
        intent_hash: intent.intent_hash().unwrap(),
        claim_operation_id: claim.operation_id.clone(),
        expected_revision: 2,
        expected_create_epoch: 1,
        outcome,
    }
}
fn synthetic_held_outcome(intent: &KubernetesCreateIntentV1) -> KubernetesCreateOutcomeV1 {
    let prepared = prepare(intent).unwrap();
    let mut pod: Value = serde_json::from_slice(prepared.pod_bytes()).unwrap();
    pod["metadata"]["uid"] = json!(POD_UID);
    pod["metadata"]["resourceVersion"] = json!("17");
    parse_created_held(
        intent,
        &serde_json::to_vec(&pod).unwrap(),
        "2026-11-05T08:00:00.000Z",
    )
    .unwrap()
}

#[test]
fn held_journal_real_ack_and_historical_confirmation_publish_only_exact_retained_uid() {
    let f = Fixture::new();
    let quota = QuotaPolicyV1::default();
    let authorization = f
        .store
        .authorize_held_create_for_test(&f.identity, &f.intent, &quota, EXPIRY)
        .unwrap();
    let (receipt, reserved, replayed) = f
        .store
        .reserve_kubernetes_create(&f.identity, &authorization, &f.intent)
        .unwrap();
    assert!(!replayed);
    let request = claim(&f.intent);
    let root = journal_root(&f);
    let mut journal = AdmissionJournal::prepare(
        root.clone(),
        protection(),
        f.store.tenant_id(),
        &f.intent,
        prepare(&f.intent).unwrap(),
        &receipt,
        &reserved,
        &request,
    )
    .unwrap();
    let (response, fresh) = f
        .store
        .claim_kubernetes_create(&f.identity, &authorization, &request)
        .unwrap();
    assert_eq!(
        response.disposition,
        KubernetesCreateDispositionV1::EmitOnce
    );
    let fresh = fresh.expect("only the actual first Store commit supplies this proof");
    let checkpoint = journal.commit_emission_possible(&fresh).unwrap();
    journal.revalidate_emission(&checkpoint).unwrap();
    let candidate = result(&f.intent, &request, synthetic_held_outcome(&f.intent));
    let retained = journal.retain_candidate(&candidate, None).unwrap();
    assert_eq!(retained.operation_id(), candidate.operation_id);
    assert_eq!(retained.input_hash(), candidate.input_hash().unwrap());
    assert_eq!(retained.refusal(), None);
    assert!(journal.revalidate_emission(&checkpoint).is_err());
    let confirmed = f
        .store
        .record_kubernetes_create_result(&f.identity, &authorization, &candidate)
        .unwrap();
    assert_eq!(
        confirmed.snapshot().state,
        KubernetesCreateStateV1::CreatedHeld
    );
    assert_eq!(confirmed.snapshot().revision, 3);
    assert!(confirmed.snapshot().quota_held);
    let exact_receipt = confirmed.receipt().clone();
    let historical_snapshot = confirmed.snapshot().clone();
    let handle = journal.acknowledge_result(confirmed).unwrap().unwrap();
    assert_eq!(handle.uid(), POD_UID);
    assert_eq!(handle.name(), reserved.target.pod_name);
    assert_eq!(handle.session_id().to_string(), f.intent.session_id);
    assert_eq!(handle.operation_id().to_string(), f.intent.create_id);
    let ack_bytes = fs::read(root.path().join("ack.json")).unwrap();
    assert_eq!(f.store.worker_quota(&quota).unwrap().held_workers, 1);

    // Advance CURRENT epoch after the result. Exact replay still confirms the
    // original historical receipt; it must never reactivate/reset that epoch.
    let cancel = KubernetesCreateCancelV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        create_id: f.intent.create_id.clone(),
        intent_hash: f.intent.intent_hash().unwrap(),
        expected_revision: 3,
        expected_create_epoch: 1,
    };
    let (_, current, _) = f
        .store
        .cancel_kubernetes_create(&f.identity, &authorization, &cancel)
        .unwrap();
    assert_eq!(current.create_epoch, 2);
    assert_eq!(current.revision, 4);
    assert!(current.quota_held);
    let head = f.store.head().unwrap();
    drop(journal);
    let recovered = AdmissionJournal::recover(root.clone(), protection())
        .unwrap()
        .unwrap();
    assert_eq!(recovered.phase(), RecoveryPhase::ResultAcknowledged);
    let mut journal = recovered.into_journal();
    assert_eq!(journal.candidate(), Some(&candidate));
    assert_eq!(journal.tenant_id(), f.store.tenant_id());
    let confirmed = f
        .store
        .record_kubernetes_create_result(&f.identity, &authorization, &candidate)
        .unwrap();
    assert_eq!(confirmed.receipt(), &exact_receipt);
    assert_eq!(confirmed.snapshot(), &historical_snapshot);
    assert_eq!(
        journal.acknowledge_result(confirmed).unwrap().unwrap(),
        handle
    );
    assert_eq!(fs::read(root.path().join("ack.json")).unwrap(), ack_bytes);
    assert_eq!(f.store.head().unwrap(), head);
    assert_eq!(f.store.worker_quota(&quota).unwrap().held_workers, 1);
    assert!(f.store.integrity().unwrap().valid);
}

#[test]
fn held_journal_lost_claim_ack_retains_history_without_reissuing_or_rewriting_cas() {
    let f = Fixture::new();
    let quota = QuotaPolicyV1::default();
    let authorization = f
        .store
        .authorize_held_create_for_test(&f.identity, &f.intent, &quota, EXPIRY)
        .unwrap();
    let (receipt, reserved, _) = f
        .store
        .reserve_kubernetes_create(&f.identity, &authorization, &f.intent)
        .unwrap();
    let request = claim(&f.intent);
    let root = journal_root(&f);
    let journal = AdmissionJournal::prepare(
        root.clone(),
        protection(),
        f.store.tenant_id(),
        &f.intent,
        prepare(&f.intent).unwrap(),
        &receipt,
        &reserved,
        &request,
    )
    .unwrap();
    let (first, fresh) = f
        .store
        .claim_kubernetes_create(&f.identity, &authorization, &request)
        .unwrap();
    let fresh = fresh.unwrap();
    // Commit happened, but its ACK/checkpoint did not reach the journal. Keep
    // the old in-memory proof only to test that reopen cannot use even this.
    assert!(!root.path().join("emission.json").exists());
    drop(journal);
    let recovered = AdmissionJournal::recover(root.clone(), protection())
        .unwrap()
        .unwrap();
    assert_eq!(recovered.phase(), RecoveryPhase::Prepared);
    let mut journal = recovered.into_journal();
    assert_eq!(journal.claim_request(), &request);
    assert!(journal.commit_emission_possible(&fresh).is_err());
    let head = f.store.head().unwrap();
    let (history, replay_fresh) = f
        .store
        .claim_kubernetes_create(&f.identity, &authorization, &request)
        .unwrap();
    assert!(replay_fresh.is_none());
    assert_eq!(
        history.disposition,
        KubernetesCreateDispositionV1::ObserveOnly
    );
    assert_eq!(history.receipt, first.receipt);
    assert_eq!(history.snapshot, first.snapshot);
    journal
        .retain_observed_claim(f.store.tenant_id(), &history)
        .expect("exact historical claim must be durably retained without emission authority");
    let emission_bytes = fs::read(root.path().join("emission.json")).unwrap();
    assert!(journal.commit_emission_possible(&fresh).is_err());
    assert_eq!(f.store.head().unwrap(), head);
    assert!(journal.retain_observed_claim("tenant-b", &history).is_err());
    assert!(
        journal
            .retain_observed_claim(f.store.tenant_id(), &first)
            .is_err()
    );
    let mut changed = history.clone();
    changed.snapshot.revision += 1;
    assert!(
        journal
            .retain_observed_claim(f.store.tenant_id(), &changed)
            .is_err()
    );
    assert_eq!(
        fs::read(root.path().join("emission.json")).unwrap(),
        emission_bytes
    );
    drop(journal);
    let recovered = AdmissionJournal::recover(root.clone(), protection())
        .unwrap()
        .unwrap();
    assert_eq!(recovered.phase(), RecoveryPhase::ObserveOnly);
    let mut journal = recovered.into_journal();
    journal
        .retain_observed_claim(f.store.tenant_id(), &history)
        .unwrap();
    assert!(journal.commit_emission_possible(&fresh).is_err());
    let candidate = result(
        &f.intent,
        &request,
        KubernetesCreateOutcomeV1::OwnershipUnknown {
            reason: KubernetesCreateUnknownReasonV1::RecoveredAfterClaim,
            http_status: None,
        },
    );
    journal.retain_candidate(&candidate, None).unwrap();
    let confirmed = f
        .store
        .record_kubernetes_create_result(&f.identity, &authorization, &candidate)
        .unwrap();
    assert_eq!(
        confirmed.snapshot().state,
        KubernetesCreateStateV1::OwnershipUnknown
    );
    assert!(confirmed.snapshot().quota_held);
    assert!(journal.acknowledge_result(confirmed).unwrap().is_none());
    assert_eq!(f.store.worker_quota(&quota).unwrap().held_workers, 1);
    assert_eq!(
        fs::read(root.path().join("emission.json")).unwrap(),
        emission_bytes
    );
    assert!(f.store.integrity().unwrap().valid);
}

#[test]
fn held_journal_historical_retention_invalidates_an_existing_live_emission_checkpoint() {
    let f = Fixture::new();
    let authorization = f
        .store
        .authorize_held_create_for_test(&f.identity, &f.intent, &QuotaPolicyV1::default(), EXPIRY)
        .unwrap();
    let (receipt, reserved, _) = f
        .store
        .reserve_kubernetes_create(&f.identity, &authorization, &f.intent)
        .unwrap();
    let request = claim(&f.intent);
    let root = journal_root(&f);
    let mut journal = AdmissionJournal::prepare(
        root.clone(),
        protection(),
        f.store.tenant_id(),
        &f.intent,
        prepare(&f.intent).unwrap(),
        &receipt,
        &reserved,
        &request,
    )
    .unwrap();
    let (_, fresh) = f
        .store
        .claim_kubernetes_create(&f.identity, &authorization, &request)
        .unwrap();
    let fresh = fresh.unwrap();
    let checkpoint = journal.commit_emission_possible(&fresh).unwrap();
    journal.revalidate_emission(&checkpoint).unwrap();
    let (history, replay_fresh) = f
        .store
        .claim_kubernetes_create(&f.identity, &authorization, &request)
        .unwrap();
    assert!(replay_fresh.is_none());
    let bytes = fs::read(root.path().join("emission.json")).unwrap();
    journal
        .retain_observed_claim(f.store.tenant_id(), &history)
        .expect("historical retention must permanently demote the live journal");
    assert!(journal.revalidate_emission(&checkpoint).is_err());
    assert!(journal.commit_emission_possible(&fresh).is_err());
    assert_eq!(fs::read(root.path().join("emission.json")).unwrap(), bytes);
}
