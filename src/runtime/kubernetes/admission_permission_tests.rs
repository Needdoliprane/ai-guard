//! Real Store and journal; synthetic clock, no network or runtime effect.
use super::*;
use crate::{
    audit::kubernetes_admission_state_tests::Fixture,
    broker::clock::{ClockResult, HostClock},
    control::{HostClockMethodV1, QuotaPolicyV1},
    control_fs::Directory,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use uuid::Uuid;

struct CountedClock {
    calls: AtomicUsize,
}
impl HostClock for CountedClock {
    fn sample(&self) -> ClockResult<HostClockSampleV1> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(HostClockSampleV1 {
            version: 1,
            method: HostClockMethodV1::LinuxBoottimeV1,
            boot_id: "11111111-1111-4111-8111-111111111111".into(),
            continuous_ns_decimal: "100000000000".into(),
            observed_utc: None,
        })
    }
}

fn fixed_clock() -> Arc<dyn HostClock> {
    Arc::new(CountedClock {
        calls: AtomicUsize::new(0),
    })
}
fn retained_request(
    f: &Fixture,
    intent: &KubernetesCreateIntentV1,
    child: &str,
) -> (
    KubernetesCreateAuthorization,
    KubernetesCreateClaimV1,
    AdmissionJournal,
    Directory,
) {
    let authorization = f
        .store
        .authorize_held_create_for_test(
            &f.identity,
            intent,
            &QuotaPolicyV1::default(),
            "2100-01-01T00:00:00.000Z",
        )
        .unwrap();
    let (reservation, reserved, _) = f
        .store
        .reserve_kubernetes_create(&f.identity, &authorization, intent)
        .unwrap();
    let request = KubernetesCreateClaimV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        create_id: intent.create_id.clone(),
        intent_hash: intent.intent_hash().unwrap(),
        expected_revision: 1,
        expected_create_epoch: 1,
        emitter_id: Uuid::new_v4().to_string(),
    };
    let root = Directory::open_existing(f._directory.path())
        .unwrap()
        .persistent_child(child, true)
        .unwrap()
        .unwrap();
    let journal = AdmissionJournal::prepare(
        root.clone(),
        super::super::journal::AdmissionJournalProtection::new(vec![], vec![]),
        f.store.tenant_id(),
        intent,
        super::super::prepare(intent).unwrap(),
        &reservation,
        &reserved,
        &request,
    )
    .unwrap();
    (authorization, request, journal, root)
}

#[test]
fn scope_and_prepared_disk_are_checked_before_any_central_claim() {
    let mut f = Fixture::new();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut authority = serde_json::to_value(&f.intent.authority).unwrap();
    authority["endpoint"] =
        serde_json::json!(format!("https://{}", listener.local_addr().unwrap()));
    f.intent.authority = serde_json::from_value(authority).unwrap();
    let (authorization_a, request_a, mut journal_a, _) = retained_request(&f, &f.intent, "scope-a");
    let ready = PendingHeldCreateClaim::begin_for_test(
        HeldCreateActivation::for_test(&f.intent, &listener).unwrap(),
        fixed_clock(),
        &f.intent,
    )
    .unwrap()
    .claim(
        &f.store,
        &f.identity,
        &authorization_a,
        &request_a,
        &mut journal_a,
    )
    .unwrap();
    assert!(
        matches!(ready, HeldCreateClaimOutcome::Ready { .. }),
        "a valid real pair must work before negative scope cases"
    );
    let mut intent_b = f.intent.clone();
    intent_b.create_id = Uuid::new_v4().to_string();
    intent_b.session_id = Uuid::new_v4().to_string();
    let (authorization_b, request_b, mut journal_b, root_b) =
        retained_request(&f, &intent_b, "scope-b");
    let head = f.store.head().unwrap();
    let pending_a = PendingHeldCreateClaim::begin_for_test(
        HeldCreateActivation::for_test(&f.intent, &listener).unwrap(),
        fixed_clock(),
        &f.intent,
    )
    .unwrap();
    assert!(
        pending_a
            .claim(
                &f.store,
                &f.identity,
                &authorization_b,
                &request_b,
                &mut journal_b
            )
            .is_err()
    );
    assert_eq!(
        f.store.head().unwrap(),
        head,
        "a mismatched pair cannot consume the other valid claim"
    );
    std::fs::write(root_b.path().join("prepared.json"), b"{}").unwrap();
    let pending_b = PendingHeldCreateClaim::begin_for_test(
        HeldCreateActivation::for_test(&intent_b, &listener).unwrap(),
        fixed_clock(),
        &intent_b,
    )
    .unwrap();
    assert!(
        pending_b
            .claim(
                &f.store,
                &f.identity,
                &authorization_b,
                &request_b,
                &mut journal_b
            )
            .is_err()
    );
    assert_eq!(
        f.store.head().unwrap(),
        head,
        "tampering cannot lose the durable replay identity before claiming"
    );
    assert_eq!(
        f.store
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        2
    );
}

#[test]
fn recovered_prepared_only_cannot_create_a_previously_absent_central_claim() {
    let mut f = Fixture::new();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut authority = serde_json::to_value(&f.intent.authority).unwrap();
    authority["endpoint"] =
        serde_json::json!(format!("https://{}", listener.local_addr().unwrap()));
    f.intent.authority = serde_json::from_value(authority).unwrap();
    let (authorization, request, journal, root) =
        retained_request(&f, &f.intent, "prepared-recovered");
    let head = f.store.head().unwrap();
    drop(journal);
    let recovered = AdmissionJournal::recover(
        root,
        super::super::journal::AdmissionJournalProtection::new(vec![], vec![]),
    )
    .unwrap()
    .unwrap();
    let mut journal = recovered.into_journal();
    let pending = PendingHeldCreateClaim::begin_for_test(
        HeldCreateActivation::for_test(&f.intent, &listener).unwrap(),
        fixed_clock(),
        &f.intent,
    )
    .unwrap();
    assert!(
        pending
            .claim(
                &f.store,
                &f.identity,
                &authorization,
                &request,
                &mut journal
            )
            .is_err()
    );
    assert_eq!(
        f.store.head().unwrap(),
        head,
        "observation of an absent claim must not consume the reserved state"
    );
    assert_eq!(
        f.store
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        1
    );
    assert!(f.store.integrity().unwrap().valid);
}

#[test]
fn recovered_claim_observes_the_exact_history_even_with_inactive_control() {
    let mut f = Fixture::new();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut authority = serde_json::to_value(&f.intent.authority).unwrap();
    authority["endpoint"] =
        serde_json::json!(format!("https://{}", listener.local_addr().unwrap()));
    f.intent.authority = serde_json::from_value(authority).unwrap();
    let (authorization, request, mut journal, root) =
        retained_request(&f, &f.intent, "ack-recovered");
    let pending = PendingHeldCreateClaim::begin_for_test(
        HeldCreateActivation::for_test(&f.intent, &listener).unwrap(),
        fixed_clock(),
        &f.intent,
    )
    .unwrap();
    let HeldCreateClaimOutcome::Ready {
        response,
        permission,
    } = pending
        .claim(
            &f.store,
            &f.identity,
            &authorization,
            &request,
            &mut journal,
        )
        .unwrap()
    else {
        panic!("first actual claim is fresh");
    };
    drop(permission);
    drop(journal);
    let path = f._directory.path().join("tenant.sqlite");
    drop(f.store);
    let reopened = Store::open_existing_for_tenant(
        &path,
        "tenant-a",
        crate::redaction::Redactor::new(vec![]),
        10_000,
    )
    .unwrap();
    assert!(!reopened.supervision_state().unwrap().active);
    let head = reopened.head().unwrap();
    let mut journal = AdmissionJournal::recover(
        root,
        super::super::journal::AdmissionJournalProtection::new(vec![], vec![]),
    )
    .unwrap()
    .unwrap()
    .into_journal();
    let pending = PendingHeldCreateClaim::begin_for_test(
        HeldCreateActivation::for_test(&f.intent, &listener).unwrap(),
        fixed_clock(),
        &f.intent,
    )
    .unwrap();
    let HeldCreateClaimOutcome::ObserveOnly { response: replay } = pending
        .claim(
            &reopened,
            &f.identity,
            &authorization,
            &request,
            &mut journal,
        )
        .unwrap()
    else {
        panic!("reopened control cannot mint emission permission");
    };
    assert_eq!(replay.receipt, response.receipt);
    assert_eq!(replay.snapshot, response.snapshot);
    assert_eq!(reopened.head().unwrap(), head);
    assert!(!reopened.supervision_state().unwrap().active);
    assert!(reopened.integrity().unwrap().valid);
}

#[test]
fn pending_claim_samples_before_the_real_commit_and_replay_never_mints_permission() {
    let mut f = Fixture::new();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut authority = serde_json::to_value(&f.intent.authority).unwrap();
    authority["endpoint"] =
        serde_json::json!(format!("https://{}", listener.local_addr().unwrap()));
    f.intent.authority = serde_json::from_value(authority).unwrap();
    let authorization = f
        .store
        .authorize_held_create_for_test(
            &f.identity,
            &f.intent,
            &QuotaPolicyV1::default(),
            "2100-01-01T00:00:00.000Z",
        )
        .unwrap();
    let (reservation, reserved, _) = f
        .store
        .reserve_kubernetes_create(&f.identity, &authorization, &f.intent)
        .unwrap();
    let request = KubernetesCreateClaimV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        create_id: f.intent.create_id.clone(),
        intent_hash: f.intent.intent_hash().unwrap(),
        expected_revision: 1,
        expected_create_epoch: 1,
        emitter_id: Uuid::new_v4().to_string(),
    };
    let root = Directory::open_existing(f._directory.path())
        .unwrap()
        .persistent_child("permission-journal", true)
        .unwrap()
        .unwrap();
    let mut journal = AdmissionJournal::prepare(
        root,
        super::super::journal::AdmissionJournalProtection::new(vec![], vec![]),
        f.store.tenant_id(),
        &f.intent,
        super::super::prepare(&f.intent).unwrap(),
        &reservation,
        &reserved,
        &request,
    )
    .unwrap();
    let counted = Arc::new(CountedClock {
        calls: AtomicUsize::new(0),
    });
    let clock: Arc<dyn HostClock> = counted.clone();
    let head_before = f.store.head().unwrap();
    let pending = PendingHeldCreateClaim::begin_for_test(
        HeldCreateActivation::for_test(&f.intent, &listener).unwrap(),
        clock.clone(),
        &f.intent,
    )
    .unwrap();
    assert_eq!(counted.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        f.store.head().unwrap(),
        head_before,
        "sampling does not fabricate control events"
    );
    let HeldCreateClaimOutcome::Ready {
        response,
        permission,
    } = pending
        .claim(
            &f.store,
            &f.identity,
            &authorization,
            &request,
            &mut journal,
        )
        .unwrap()
    else {
        panic!(
            "the first real committed claim must yield a local permission after journal durability"
        )
    };
    assert_eq!(response.receipt.audit_seq, head_before + 1);
    assert_eq!(
        counted.calls.load(Ordering::SeqCst),
        1,
        "the preclaim sample is not reset after ACK or fsync"
    );
    assert!(Arc::ptr_eq(&permission.clock, &clock));
    assert_eq!(
        permission.before_claim.continuous_ns_decimal,
        "100000000000"
    );
    journal.revalidate_emission(&permission.checkpoint).unwrap();
    assert_eq!(
        permission.pod_bytes(),
        super::super::prepare(&f.intent).unwrap().pod_bytes()
    );
    let pending = PendingHeldCreateClaim::begin_for_test(
        HeldCreateActivation::for_test(&f.intent, &listener).unwrap(),
        clock,
        &f.intent,
    )
    .unwrap();
    let HeldCreateClaimOutcome::ObserveOnly { response: replay } = pending
        .claim(
            &f.store,
            &f.identity,
            &authorization,
            &request,
            &mut journal,
        )
        .unwrap()
    else {
        panic!("exact replay cannot construct a second emission permission")
    };
    assert_eq!(replay.receipt, response.receipt);
    assert!(
        journal.revalidate_emission(&permission.checkpoint).is_err(),
        "historical retention also invalidates the earlier local checkpoint"
    );
    assert_eq!(
        f.store
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        1
    );
    assert!(f.store.integrity().unwrap().valid);
}
