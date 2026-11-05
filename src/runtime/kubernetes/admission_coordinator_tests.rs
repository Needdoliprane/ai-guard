//! Coordinator integration against the real tenant Store, retained journal,
//! and an owned loopback TLS API peer. The peer declares fixture responses;
//! these tests do not contact a cluster or start a Pod.
use super::{
    coordinator::{CoordinatorError, HeldCreateCoordinator},
    journal::{AdmissionJournal, AdmissionJournalProtection, CandidateRefusal, RecoveryPhase},
    permission::{HeldCreateActivation, PendingHeldCreateClaim},
    prepare,
    transport_tests::{
        CLUSTER_UID, EXPIRY, NAMESPACE_UID, Peer, ResponseMode, TestClock, authority, claim, sample,
    },
};
use crate::{
    audit::{EventQuery, Store, kubernetes_admission_state_tests::Fixture as StoreFixture},
    broker::clock::HostClock,
    control::{HostClockMethodV1, QuotaPolicyV1, kubernetes_admission::*},
    control_fs::Directory,
    runtime::kubernetes::KubernetesRuntime,
};
use sha2::{Digest, Sha256};
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, sync::atomic::AtomicBool};
use uuid::Uuid;

struct PreparedCoordinatorCall {
    directory: tempfile::TempDir,
    store: Store,
    identity: crate::broker_registry::BrokerIdentityV1,
    authorization: crate::audit::KubernetesCreateAuthorization,
    intent: KubernetesCreateIntentV1,
    claim: KubernetesCreateClaimV1,
    journal: AdmissionJournal,
    journal_path: PathBuf,
    expected_pod: Vec<u8>,
    runtime: KubernetesRuntime,
    pending: PendingHeldCreateClaim,
    peer: Peer,
}

async fn prepared_call(mode: ResponseMode) -> PreparedCoordinatorCall {
    let boot_id = Uuid::from_u128(0x00c0_0a11).to_string();
    let clock: std::sync::Arc<dyn HostClock> =
        TestClock::repeated(sample(&boot_id, HostClockMethodV1::LinuxBoottimeV1, 1_000));
    prepared_call_with_clock(mode, clock).await
}

async fn prepared_call_with_clock(
    mode: ResponseMode,
    clock: std::sync::Arc<dyn HostClock>,
) -> PreparedCoordinatorCall {
    let peer = Peer::new(mode).await;
    let StoreFixture {
        _directory: directory,
        store,
        identity,
        mut intent,
    } = StoreFixture::new();
    let authority_id = intent.authority.authority_id();
    intent.authority = authority(authority_id, &peer.endpoint, peer.trust_digest());

    let runtime = KubernetesRuntime::connect(intent.authority.clone(), peer.credentials())
        .expect("connect only to the owned loopback TLS peer");
    let diagnostic = runtime
        .doctor()
        .await
        .expect("validate the local CA, endpoint, cluster, and namespace before claim");
    assert_eq!(diagnostic.cluster_namespace_uid, CLUSTER_UID);
    assert_eq!(diagnostic.namespace_uid, NAMESPACE_UID);
    assert_eq!(
        peer.read_request_count(),
        3,
        "doctor finishes before the claim window"
    );

    let quota = QuotaPolicyV1::default();
    let authorization = store
        .authorize_held_create_for_test(&identity, &intent, &quota, EXPIRY)
        .expect("authorize with the fixture's real workload binding and broker grant");
    let (reservation, reserved, replayed) = store
        .reserve_kubernetes_create(&identity, &authorization, &intent)
        .expect("reserve through the real tenant Store");
    assert!(!replayed);
    let claim = claim(&intent);
    let journal_path = directory
        .path()
        .join(format!("private-coordinator-{}", intent.create_id));
    fs::create_dir(&journal_path).expect("create the private local journal directory");
    fs::set_permissions(&journal_path, fs::Permissions::from_mode(0o700))
        .expect("keep the local journal directory private");
    let prepared = prepare(&intent).expect("prepare the exact retained Pod request");
    let expected_pod = prepared.pod_bytes().to_vec();
    let journal = AdmissionJournal::prepare(
        Directory::open_existing(&journal_path).expect("open private journal root"),
        AdmissionJournalProtection::new(Vec::new(), Vec::new()),
        store.tenant_id(),
        &intent,
        prepared,
        &reservation,
        &reserved,
        &claim,
    )
    .expect("persist request bytes before the one-shot claim");
    let activation = HeldCreateActivation::for_test(&intent, &peer.activation_listener)
        .expect("bind the test capability to the exact listener");
    let pending = PendingHeldCreateClaim::begin_for_test(activation, clock, &intent)
        .expect("sample the synthetic HostClock before the Store claim");

    PreparedCoordinatorCall {
        directory,
        store,
        identity,
        authorization,
        intent,
        claim,
        journal,
        journal_path,
        expected_pod,
        runtime,
        pending,
        peer,
    }
}

fn pending_for(peer: &Peer, intent: &KubernetesCreateIntentV1) -> PendingHeldCreateClaim {
    let activation = HeldCreateActivation::for_test(intent, &peer.activation_listener)
        .expect("bind the replacement pending claim to the same owned listener");
    let boot_id = Uuid::from_u128(0x00c0_0a11).to_string();
    let clock: std::sync::Arc<dyn HostClock> =
        TestClock::repeated(sample(&boot_id, HostClockMethodV1::LinuxBoottimeV1, 1_000));
    PendingHeldCreateClaim::begin_for_test(activation, clock, intent)
        .expect("prepare the same retained request without changing the Store")
}

fn other_create_intent(intent: &KubernetesCreateIntentV1) -> KubernetesCreateIntentV1 {
    let mut other = intent.clone();
    other.create_id = Uuid::new_v4().to_string();
    other.session_id = Uuid::new_v4().to_string();
    other
}

fn result_events(store: &Store) -> Vec<crate::model::StoredEvent> {
    store
        .events(&EventQuery {
            kind: Some("control.kubernetes.create.result".into()),
            ..EventQuery::default()
        })
        .expect("read the result event from the real tenant audit store")
}

fn persisted_candidate(path: &std::path::Path) -> (Vec<u8>, KubernetesCreateResultV1) {
    let bytes = fs::read(path.join("candidate.json")).expect("read retained candidate file");
    let candidate = serde_json::from_slice(&bytes).expect("decode the retained candidate DTO");
    (bytes, candidate)
}

fn persisted_ack(path: &std::path::Path) -> KubernetesCreateReceiptV1 {
    let bytes = fs::read(path.join("ack.json")).expect("read local result ACK file");
    let record: serde_json::Value =
        serde_json::from_slice(&bytes).expect("decode the local ACK record");
    serde_json::from_value(
        record
            .get("receipt")
            .expect("ACK record carries its exact Store receipt")
            .clone(),
    )
    .expect("decode the exact Store receipt from the ACK record")
}

fn assert_unknown_503(outcome: &KubernetesCreateOutcomeV1) {
    assert!(matches!(
        outcome,
        KubernetesCreateOutcomeV1::OwnershipUnknown {
            reason: KubernetesCreateUnknownReasonV1::ApiRejected,
            http_status: Some(503),
        }
    ));
}

#[tokio::test]
async fn coordinator_commits_unknown_result_without_publishing_a_handle() {
    let mut f = prepared_call(ResponseMode::Status(503)).await;
    let before_head = f.store.head().unwrap();
    let coordinator = HeldCreateCoordinator::new(&f.store, &f.identity, &f.authorization);
    let completion = coordinator
        .execute(
            &f.runtime,
            f.pending,
            &f.claim,
            &mut f.journal,
            &AtomicBool::new(false),
        )
        .await
        .expect("a completed 503 response is retained and centrally acknowledged");

    assert_unknown_503(&completion.outcome);
    assert!(
        completion.handle.is_none(),
        "unknown ownership cannot yield a Pod handle"
    );
    let candidate = f
        .journal
        .candidate()
        .expect("the exact post candidate is durable before control ACK")
        .clone();
    let (_, disk_candidate) = persisted_candidate(&f.journal_path);
    assert_eq!(disk_candidate, candidate);
    assert_eq!(persisted_ack(&f.journal_path), completion.receipt);
    assert_eq!(candidate.outcome, completion.outcome);
    assert_eq!(completion.receipt.operation_id, candidate.operation_id);
    assert_eq!(
        completion.receipt.input_hash,
        candidate.input_hash().unwrap()
    );
    assert_eq!(completion.receipt.audit_seq, before_head + 2);

    let request = f.peer.latest_post();
    assert_eq!(
        f.peer.post_count(),
        1,
        "the coordinator must not retry CREATE"
    );
    assert_eq!(request.method, "POST");
    assert_eq!(
        request.uri,
        "/api/v1/namespaces/tenant-a/pods?fieldValidation=Strict"
    );
    assert!(request.content_type_is_json);
    assert!(request.bearer_matches);
    assert_eq!(request.body, f.expected_pod);
    assert_eq!(
        f.store
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        1,
        "unknown external ownership keeps the real reservation held"
    );
    let events = result_events(&f.store);
    assert_eq!(events.len(), 1, "the exact result is committed once");
    assert_eq!(events[0].event.id, candidate.operation_id);
    assert_eq!(
        events[0].event.data["snapshot"]["state"], "ownership_unknown",
        "the central projection retains its explicit unknown-ownership state"
    );
    assert!(f.store.integrity().unwrap().valid);
}

#[tokio::test]
async fn lost_result_ack_resumes_the_committed_candidate_without_post_or_store_mutation() {
    let mut f = prepared_call(ResponseMode::Status(503)).await;
    let db_path = f.directory.path().join("tenant.sqlite");
    let other_intent = other_create_intent(&f.intent);
    let other_authorization = f
        .store
        .authorize_held_create_for_test(
            &f.identity,
            &other_intent,
            &QuotaPolicyV1::default(),
            EXPIRY,
        )
        .expect("obtain a real capability bound to another create and session");
    let before_execute_head = f.store.head().unwrap();
    let coordinator = HeldCreateCoordinator::new(&f.store, &f.identity, &f.authorization);
    coordinator.lose_result_ack_once_for_test();
    let execution = coordinator
        .execute(
            &f.runtime,
            f.pending,
            &f.claim,
            &mut f.journal,
            &AtomicBool::new(false),
        )
        .await;
    assert!(matches!(
        execution,
        Err(CoordinatorError::ControlUnavailable)
    ));
    assert_eq!(
        f.journal.candidate_refusal(),
        Some(CandidateRefusal::Unavailable),
        "a committed result with a lost confirmation records an indeterminate refusal"
    );

    let candidate = f
        .journal
        .candidate()
        .expect("the injected lost ACK occurs only after durable result commit")
        .clone();
    let (candidate_bytes_before_reopen, disk_candidate) = persisted_candidate(&f.journal_path);
    assert_eq!(disk_candidate, candidate);
    let refusal_bytes_before_reopen = fs::read(f.journal_path.join("refusal.json")).unwrap();
    let refusal: serde_json::Value = serde_json::from_slice(&refusal_bytes_before_reopen).unwrap();
    assert_eq!(refusal["refusal"], "unavailable");
    assert!(
        !f.journal_path.join("ack.json").exists(),
        "the injected lost confirmation must leave the local ACK absent"
    );
    assert_unknown_503(&candidate.outcome);
    let events_before_reopen = result_events(&f.store);
    assert_eq!(events_before_reopen.len(), 1);
    assert_eq!(events_before_reopen[0].event.id, candidate.operation_id);
    assert_eq!(
        events_before_reopen[0].event.data["snapshot"]["state"],
        "ownership_unknown"
    );
    assert_eq!(f.store.head().unwrap(), before_execute_head + 2);
    assert_eq!(
        f.store
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        1
    );
    assert_eq!(f.peer.post_count(), 1);

    let journal_path = f.journal_path.clone();
    drop(f.journal);
    drop(coordinator);
    drop(f.store);
    let reopened = Store::open_existing_for_tenant(
        &db_path,
        "tenant-a",
        crate::redaction::Redactor::new(Vec::new()),
        10_000,
    )
    .expect("reopen the actual Store after losing only the local confirmation");
    assert!(!reopened.supervision_state().unwrap().active);
    assert!(reopened.integrity().unwrap().valid);
    let recovered = AdmissionJournal::recover(
        Directory::open_existing(&journal_path).expect("reopen private result journal"),
        AdmissionJournalProtection::new(Vec::new(), Vec::new()),
    )
    .expect("validate all retained files")
    .expect("the journal contains a retained candidate");
    assert_eq!(recovered.phase(), RecoveryPhase::CandidateRetained);
    let mut journal = recovered.into_journal();
    assert_eq!(journal.candidate(), Some(&candidate));
    assert_eq!(
        journal.candidate_refusal(),
        Some(CandidateRefusal::Unavailable)
    );
    assert_eq!(
        fs::read(journal_path.join("refusal.json")).unwrap(),
        refusal_bytes_before_reopen
    );
    let (candidate_bytes_after_reopen, disk_candidate) = persisted_candidate(&journal_path);
    assert_eq!(candidate_bytes_after_reopen, candidate_bytes_before_reopen);
    assert_eq!(disk_candidate, candidate);
    let before_resume_head = reopened.head().unwrap();
    let before_resume_events = result_events(&reopened);
    let before_resume_quota = reopened
        .worker_quota(&QuotaPolicyV1::default())
        .unwrap()
        .held_workers;
    let wrong_coordinator =
        HeldCreateCoordinator::new(&reopened, &f.identity, &other_authorization);
    assert!(matches!(
        wrong_coordinator.resume_result(&mut journal),
        Err(CoordinatorError::ControlRejected)
    ));
    assert_eq!(journal.candidate(), Some(&candidate));
    assert_eq!(
        journal.candidate_refusal(),
        Some(CandidateRefusal::Unavailable)
    );
    assert_eq!(
        fs::read(journal_path.join("refusal.json")).unwrap(),
        refusal_bytes_before_reopen
    );
    assert_eq!(
        fs::read(journal_path.join("candidate.json")).unwrap(),
        candidate_bytes_before_reopen
    );
    assert!(!journal_path.join("ack.json").exists());
    assert_eq!(reopened.head().unwrap(), before_resume_head);
    assert_eq!(result_events(&reopened).len(), before_resume_events.len());
    assert_eq!(
        reopened
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        before_resume_quota
    );
    drop(wrong_coordinator);
    let coordinator = HeldCreateCoordinator::new(&reopened, &f.identity, &f.authorization);
    let completion = coordinator
        .resume_result(&mut journal)
        .expect("resume from the retained result without a fresh claim or network call");

    assert_unknown_503(&completion.outcome);
    assert!(completion.handle.is_none());
    assert_eq!(completion.receipt.operation_id, candidate.operation_id);
    assert_eq!(
        completion.receipt.input_hash,
        candidate.input_hash().unwrap()
    );
    assert_eq!(completion.receipt.audit_seq, before_resume_events[0].seq);
    assert_eq!(
        persisted_candidate(&journal_path).0,
        candidate_bytes_before_reopen
    );
    assert_eq!(persisted_ack(&journal_path), completion.receipt);
    let ack_bytes_after_resume = fs::read(journal_path.join("ack.json")).unwrap();
    assert_eq!(journal.candidate(), Some(&candidate));
    assert_eq!(
        journal.candidate_refusal(),
        Some(CandidateRefusal::Unavailable),
        "acknowledgement does not erase the first durable refusal"
    );
    assert_eq!(
        fs::read(journal_path.join("refusal.json")).unwrap(),
        refusal_bytes_before_reopen
    );
    assert_eq!(
        f.peer.post_count(),
        1,
        "result recovery cannot repeat CREATE"
    );
    assert_eq!(reopened.head().unwrap(), before_resume_head);
    assert_eq!(result_events(&reopened).len(), 1);
    assert_eq!(
        reopened
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        before_resume_quota
    );
    assert_eq!(before_resume_quota, 1);
    assert!(!reopened.supervision_state().unwrap().active);
    assert!(reopened.integrity().unwrap().valid);

    let same_intent_pending = pending_for(&f.peer, &f.intent);
    assert_eq!(same_intent_pending.intent(), &f.intent);
    let head_before_duplicate_execute = reopened.head().unwrap();
    let duplicate = coordinator
        .execute(
            &f.runtime,
            same_intent_pending,
            &f.claim,
            &mut journal,
            &AtomicBool::new(false),
        )
        .await
        .expect("a retained result takes priority over any new claim attempt");
    assert_eq!(duplicate.receipt, completion.receipt);
    assert_eq!(duplicate.outcome, completion.outcome);
    assert!(duplicate.handle.is_none());
    assert_eq!(reopened.head().unwrap(), head_before_duplicate_execute);
    assert_eq!(result_events(&reopened).len(), 1);
    assert_eq!(f.peer.post_count(), 1);
    assert_eq!(
        fs::read(journal_path.join("candidate.json")).unwrap(),
        candidate_bytes_before_reopen
    );
    assert_eq!(
        fs::read(journal_path.join("ack.json")).unwrap(),
        ack_bytes_after_resume
    );
    assert_eq!(
        fs::read(journal_path.join("refusal.json")).unwrap(),
        refusal_bytes_before_reopen
    );

    let mut changed_intent = f.intent.clone();
    changed_intent.session_id = Uuid::new_v4().to_string();
    let divergent_pending = pending_for(&f.peer, &changed_intent);
    let head_before_divergent_resume = reopened.head().unwrap();
    let posts_before_divergent_resume = f.peer.post_count();
    let candidate_bytes_before_divergent_resume =
        fs::read(journal_path.join("candidate.json")).unwrap();
    let ack_bytes_before_divergent_resume = fs::read(journal_path.join("ack.json")).unwrap();
    assert!(matches!(
        coordinator
            .execute(
                &f.runtime,
                divergent_pending,
                &f.claim,
                &mut journal,
                &AtomicBool::new(false),
            )
            .await,
        Err(CoordinatorError::InvalidContext)
    ));
    assert_eq!(reopened.head().unwrap(), head_before_divergent_resume);
    assert_eq!(f.peer.post_count(), posts_before_divergent_resume);
    assert_eq!(
        fs::read(journal_path.join("candidate.json")).unwrap(),
        candidate_bytes_before_divergent_resume
    );
    assert_eq!(
        fs::read(journal_path.join("ack.json")).unwrap(),
        ack_bytes_before_divergent_resume
    );
}

#[tokio::test]
async fn coordinator_publishes_created_handle_only_with_the_durable_store_ack() {
    let mut f = prepared_call(ResponseMode::Created).await;
    let before_head = f.store.head().unwrap();
    let coordinator = HeldCreateCoordinator::new(&f.store, &f.identity, &f.authorization);
    let completion = coordinator
        .execute(
            &f.runtime,
            f.pending,
            &f.claim,
            &mut f.journal,
            &AtomicBool::new(false),
        )
        .await
        .expect("an authenticated 201 and exact Store ACK complete the retained operation");

    let KubernetesCreateOutcomeV1::CreatedHeld { observation, .. } = &completion.outcome else {
        panic!("the fixture's exact 201 response should produce CreatedHeld");
    };
    assert_eq!(
        observation.metadata.uid,
        "33333333-3333-4333-8333-333333333333"
    );
    assert_eq!(
        completion.handle.as_ref().map(|handle| handle.uid()),
        Some(observation.metadata.uid.as_str())
    );
    let candidate = f.journal.candidate().expect("candidate retained on disk");
    let (_, disk_candidate) = persisted_candidate(&f.journal_path);
    assert_eq!(&disk_candidate, candidate);
    assert_eq!(candidate.outcome, completion.outcome);
    assert_eq!(persisted_ack(&f.journal_path), completion.receipt);
    assert_eq!(completion.receipt.operation_id, candidate.operation_id);
    assert_eq!(completion.receipt.audit_seq, before_head + 2);
    assert_eq!(f.peer.post_count(), 1);
    assert!(f.peer.latest_post().bearer_matches);
    assert_eq!(
        f.store
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        1
    );
    assert_eq!(result_events(&f.store).len(), 1);
}

#[tokio::test]
async fn coordinator_acknowledges_pre_send_cancellation_without_posting() {
    let mut f = prepared_call(ResponseMode::Status(503)).await;
    let before_head = f.store.head().unwrap();
    let coordinator = HeldCreateCoordinator::new(&f.store, &f.identity, &f.authorization);
    let completion = coordinator
        .execute(
            &f.runtime,
            f.pending,
            &f.claim,
            &mut f.journal,
            &AtomicBool::new(true),
        )
        .await
        .expect("a pre-send refusal after Ready is recorded as an unknown result");

    assert!(matches!(
        completion.outcome,
        KubernetesCreateOutcomeV1::OwnershipUnknown {
            reason: KubernetesCreateUnknownReasonV1::CancelledAfterClaim,
            http_status: None,
        }
    ));
    assert!(completion.handle.is_none());
    assert_eq!(
        f.peer.post_count(),
        0,
        "cancelled before initiation means no POST"
    );
    let candidate = f
        .journal
        .candidate()
        .expect("refusal candidate is retained");
    let (_, disk_candidate) = persisted_candidate(&f.journal_path);
    assert_eq!(&disk_candidate, candidate);
    assert_eq!(candidate.outcome, completion.outcome);
    assert_eq!(persisted_ack(&f.journal_path), completion.receipt);
    assert_eq!(completion.receipt.audit_seq, before_head + 2);
    let events = result_events(&f.store);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event.id, candidate.operation_id);
    assert_eq!(
        events[0].event.data["snapshot"]["state"],
        "ownership_unknown"
    );
    assert_eq!(
        f.store
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        1
    );
}

#[tokio::test]
async fn expired_claim_window_is_unknown_without_mislabeling_authority_or_posting() {
    let boot_id = Uuid::from_u128(0x00c0_0a11).to_string();
    let clock: std::sync::Arc<dyn HostClock> = TestClock::sequence([
        sample(&boot_id, HostClockMethodV1::LinuxBoottimeV1, 1_000),
        sample(&boot_id, HostClockMethodV1::LinuxBoottimeV1, 9_000_001_000),
    ]);
    let mut f = prepared_call_with_clock(ResponseMode::Status(503), clock).await;
    assert_eq!(f.runtime.binding(), &f.intent.authority);
    let before_head = f.store.head().unwrap();
    let coordinator = HeldCreateCoordinator::new(&f.store, &f.identity, &f.authorization);
    let completion = coordinator
        .execute(
            &f.runtime,
            f.pending,
            &f.claim,
            &mut f.journal,
            &AtomicBool::new(false),
        )
        .await
        .expect("expired freshness is retained as uncertainty, not confused with authority change");

    assert!(matches!(
        completion.outcome,
        KubernetesCreateOutcomeV1::OwnershipUnknown {
            reason: KubernetesCreateUnknownReasonV1::TransportUnknown,
            http_status: None,
        }
    ));
    assert!(completion.handle.is_none());
    assert_eq!(
        f.peer.post_count(),
        0,
        "expired freshness must stop before POST"
    );
    let candidate = f
        .journal
        .candidate()
        .expect("the post-claim refusal is represented by a durable candidate");
    let (_, disk_candidate) = persisted_candidate(&f.journal_path);
    assert_eq!(&disk_candidate, candidate);
    assert_eq!(candidate.outcome, completion.outcome);
    assert_eq!(persisted_ack(&f.journal_path), completion.receipt);
    assert_eq!(completion.receipt.audit_seq, before_head + 2);
    let events = result_events(&f.store);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event.id, candidate.operation_id);
    assert_eq!(
        events[0].event.data["snapshot"]["state"],
        "ownership_unknown"
    );
    assert_eq!(
        f.store
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        1
    );
}

#[test]
fn stale_epoch_candidate_refusal_survives_reopen_without_result_event_or_ack() {
    let StoreFixture {
        _directory: directory,
        store,
        identity,
        intent,
    } = StoreFixture::new();
    let quota = QuotaPolicyV1::default();
    let authorization = store
        .authorize_held_create_for_test(&identity, &intent, &quota, EXPIRY)
        .unwrap();
    let (reservation, reserved, replayed) = store
        .reserve_kubernetes_create(&identity, &authorization, &intent)
        .unwrap();
    assert!(!replayed);
    let claim = claim(&intent);
    let journal_path = directory
        .path()
        .join(format!("private-stale-epoch-{}", intent.create_id));
    fs::create_dir(&journal_path).unwrap();
    fs::set_permissions(&journal_path, fs::Permissions::from_mode(0o700)).unwrap();
    let mut journal = AdmissionJournal::prepare(
        Directory::open_existing(&journal_path).unwrap(),
        AdmissionJournalProtection::new(Vec::new(), Vec::new()),
        store.tenant_id(),
        &intent,
        prepare(&intent).unwrap(),
        &reservation,
        &reserved,
        &claim,
    )
    .unwrap();
    let (claim_response, fresh) = store
        .claim_kubernetes_create(&identity, &authorization, &claim)
        .unwrap();
    assert_eq!(claim_response.snapshot.revision, 2);
    let checkpoint = journal
        .commit_emission_possible(&fresh.expect("real claim commit yields one proof"))
        .unwrap();
    journal.revalidate_emission(&checkpoint).unwrap();
    let candidate = KubernetesCreateResultV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        create_id: intent.create_id.clone(),
        intent_hash: intent.intent_hash().unwrap(),
        claim_operation_id: claim.operation_id.clone(),
        expected_revision: claim_response.snapshot.revision,
        expected_create_epoch: claim_response.snapshot.create_epoch,
        outcome: KubernetesCreateOutcomeV1::OwnershipUnknown {
            reason: KubernetesCreateUnknownReasonV1::TransportUnknown,
            http_status: None,
        },
    };
    journal.retain_candidate(&candidate, None).unwrap();
    let cancellation = KubernetesCreateCancelV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        create_id: intent.create_id.clone(),
        intent_hash: intent.intent_hash().unwrap(),
        expected_revision: claim_response.snapshot.revision,
        expected_create_epoch: claim_response.snapshot.create_epoch,
    };
    let (_, current, replayed) = store
        .cancel_kubernetes_create(&identity, &authorization, &cancellation)
        .unwrap();
    assert!(!replayed);
    assert_eq!(current.revision, 3);
    assert_eq!(current.create_epoch, 2);
    assert_eq!(current.state, KubernetesCreateStateV1::OwnershipUnknown);
    assert!(current.quota_held);

    let candidate_path = journal_path.join("candidate.json");
    let candidate_bytes = fs::read(&candidate_path).unwrap();
    let refusal_path = journal_path.join("refusal.json");
    let before_resume_head = store.head().unwrap();
    let coordinator = HeldCreateCoordinator::new(&store, &identity, &authorization);
    assert!(matches!(
        coordinator.resume_result(&mut journal),
        Err(CoordinatorError::ControlRejected)
    ));
    assert_eq!(journal.candidate(), Some(&candidate));
    assert_eq!(
        journal.candidate_refusal(),
        Some(CandidateRefusal::StaleEpoch)
    );
    assert_eq!(fs::read(&candidate_path).unwrap(), candidate_bytes);
    assert!(!journal_path.join("ack.json").exists());
    let refusal_bytes = fs::read(&refusal_path).expect("typed CAS refusal is durable");
    let refusal: serde_json::Value = serde_json::from_slice(&refusal_bytes).unwrap();
    assert_eq!(refusal["refusal"], "stale_epoch");
    assert_eq!(
        refusal["candidate_sha256"],
        hex::encode(Sha256::digest(&candidate_bytes))
    );
    assert_eq!(store.head().unwrap(), before_resume_head);
    assert!(result_events(&store).is_empty());
    assert_eq!(store.worker_quota(&quota).unwrap().held_workers, 1);

    let database_path = directory.path().join("tenant.sqlite");
    drop(journal);
    drop(coordinator);
    drop(store);
    let reopened = Store::open_existing_for_tenant(
        &database_path,
        "tenant-a",
        crate::redaction::Redactor::new(Vec::new()),
        10_000,
    )
    .unwrap();
    assert!(!reopened.supervision_state().unwrap().active);
    let recovered = AdmissionJournal::recover(
        Directory::open_existing(&journal_path).unwrap(),
        AdmissionJournalProtection::new(Vec::new(), Vec::new()),
    )
    .unwrap()
    .unwrap();
    assert_eq!(recovered.phase(), RecoveryPhase::CandidateRetained);
    let journal = recovered.into_journal();
    assert_eq!(journal.candidate(), Some(&candidate));
    assert_eq!(
        journal.candidate_refusal(),
        Some(CandidateRefusal::StaleEpoch)
    );
    assert_eq!(fs::read(&candidate_path).unwrap(), candidate_bytes);
    assert_eq!(fs::read(&refusal_path).unwrap(), refusal_bytes);
    assert!(!journal_path.join("ack.json").exists());
    assert_eq!(reopened.head().unwrap(), before_resume_head);
    assert!(result_events(&reopened).is_empty());
    assert_eq!(reopened.worker_quota(&quota).unwrap().held_workers, 1);
    assert!(reopened.integrity().unwrap().valid);
}

#[test]
fn mismatched_store_authorization_persists_control_refusal_before_exact_ack() {
    let StoreFixture {
        _directory: directory,
        store,
        identity,
        intent,
    } = StoreFixture::new();
    let quota = QuotaPolicyV1::default();
    let authorization = store
        .authorize_held_create_for_test(&identity, &intent, &quota, EXPIRY)
        .unwrap();
    let other_intent = other_create_intent(&intent);
    let other_authorization = store
        .authorize_held_create_for_test(&identity, &other_intent, &quota, EXPIRY)
        .expect("the mismatched authority is still a real Store-issued capability");
    let (reservation, reserved, replayed) = store
        .reserve_kubernetes_create(&identity, &authorization, &intent)
        .unwrap();
    assert!(!replayed);
    let claim = claim(&intent);
    let journal_path = directory
        .path()
        .join(format!("private-control-refusal-{}", intent.create_id));
    fs::create_dir(&journal_path).unwrap();
    fs::set_permissions(&journal_path, fs::Permissions::from_mode(0o700)).unwrap();
    let prepared = prepare(&intent).unwrap();
    let mut journal = AdmissionJournal::prepare(
        Directory::open_existing(&journal_path).unwrap(),
        AdmissionJournalProtection::new(Vec::new(), Vec::new()),
        store.tenant_id(),
        &intent,
        prepared,
        &reservation,
        &reserved,
        &claim,
    )
    .unwrap();
    let (claim_response, fresh) = store
        .claim_kubernetes_create(&identity, &authorization, &claim)
        .unwrap();
    let checkpoint = journal
        .commit_emission_possible(&fresh.expect("claim is committed in the real Store"))
        .unwrap();
    journal.revalidate_emission(&checkpoint).unwrap();
    let candidate = KubernetesCreateResultV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        create_id: intent.create_id.clone(),
        intent_hash: intent.intent_hash().unwrap(),
        claim_operation_id: claim.operation_id.clone(),
        expected_revision: claim_response.snapshot.revision,
        expected_create_epoch: claim_response.snapshot.create_epoch,
        outcome: KubernetesCreateOutcomeV1::OwnershipUnknown {
            reason: KubernetesCreateUnknownReasonV1::TransportUnknown,
            http_status: None,
        },
    };
    journal.retain_candidate(&candidate, None).unwrap();

    let head_before_refusal = store.head().unwrap();
    let wrong_coordinator = HeldCreateCoordinator::new(&store, &identity, &other_authorization);
    assert!(matches!(
        wrong_coordinator.resume_result(&mut journal),
        Err(CoordinatorError::ControlRejected)
    ));
    assert_eq!(journal.candidate(), Some(&candidate));
    assert_eq!(
        journal.candidate_refusal(),
        Some(CandidateRefusal::ControlRejected)
    );
    let candidate_bytes = fs::read(journal_path.join("candidate.json")).unwrap();
    let refusal_bytes = fs::read(journal_path.join("refusal.json")).unwrap();
    let refusal: serde_json::Value = serde_json::from_slice(&refusal_bytes).unwrap();
    assert_eq!(refusal["refusal"], "control_rejected");
    assert!(!journal_path.join("ack.json").exists());
    assert!(result_events(&store).is_empty());
    assert_eq!(store.head().unwrap(), head_before_refusal);
    assert_eq!(store.worker_quota(&quota).unwrap().held_workers, 1);
    drop(wrong_coordinator);

    let coordinator = HeldCreateCoordinator::new(&store, &identity, &authorization);
    let completion = coordinator
        .resume_result(&mut journal)
        .expect("the exact candidate succeeds after using its matching Store capability");
    assert!(completion.handle.is_none());
    assert_eq!(completion.receipt.operation_id, candidate.operation_id);
    assert_eq!(completion.outcome, candidate.outcome);
    assert_eq!(journal.candidate(), Some(&candidate));
    assert_eq!(
        journal.candidate_refusal(),
        Some(CandidateRefusal::ControlRejected),
        "a later successful ACK cannot erase the first refusal"
    );
    assert_eq!(
        fs::read(journal_path.join("candidate.json")).unwrap(),
        candidate_bytes
    );
    assert_eq!(
        fs::read(journal_path.join("refusal.json")).unwrap(),
        refusal_bytes
    );
    assert_eq!(persisted_ack(&journal_path), completion.receipt);
    assert_eq!(result_events(&store).len(), 1);
    assert_eq!(store.worker_quota(&quota).unwrap().held_workers, 1);
    assert!(store.integrity().unwrap().valid);
}
