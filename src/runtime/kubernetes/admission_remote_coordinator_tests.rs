//! Real control HTTP/SQLite and private journal, with an owned TLS fixture peer.
//! The Kubernetes peer declares fixture responses and starts no Pod or worker.
use super::{
    coordinator::CoordinatorError,
    journal::{AdmissionJournal, RecoveryPhase},
    journal_io::JournalIo,
    permission::{HeldCreateActivation, PendingHeldCreateClaim},
    remote_coordinator::RemoteHeldCreateCoordinator,
    transport_tests::{Peer, ResponseMode, TestClock, authority, sample},
};
use crate::{
    broker::{clock::HostClock, held_create_client::tests::Fixture as ControlFixture},
    control::{HostClockMethodV1, QuotaPolicyV1, kubernetes_admission::*},
    control_fs::{Directory, PrivateFile},
    runtime::kubernetes::KubernetesRuntime,
};
use std::{
    fs,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::Semaphore;
use uuid::Uuid;

#[tokio::test]
async fn remote_coordinator_does_not_attribute_local_screening_to_control_rejection() {
    use crate::{audit::EventQuery, broker::held_create_client::HeldCreateControlClient};
    let mut fixture = ControlFixture::new("remote-local-refusal");
    let peer = Peer::new(ResponseMode::Status(503)).await;
    let authority_id = fixture.intent.authority.authority_id();
    fixture.intent.authority = authority(authority_id, &peer.endpoint, peer.trust_digest());
    let runtime =
        KubernetesRuntime::connect(fixture.intent.authority.clone(), peer.credentials()).unwrap();
    runtime.doctor().await.unwrap();
    let server = fixture.serve(true, QuotaPolicyV1::default()).await;
    let scope = fixture.client(&server.url).expected_scope().clone();
    let token = PrivateFile::open(&fixture.directory.path().join("broker-token"), 4096).unwrap();
    // Intentionally differing dictionaries reproduce a local refusal before
    // Result HTTP; neither dictionary nor response contains a real secret.
    let client = Arc::new(
        HeldCreateControlClient::new(scope, token, None, vec!["api_rejected".into()]).unwrap(),
    );
    let reservation = client.reserve(&fixture.intent).await.unwrap();
    let claim = fixture.claim();
    let journal = fixture.prepared_journal(&reservation, &claim);
    let root = fixture.directory.path().join("held-create-journal");
    let capacity = Arc::new(Semaphore::new(1));
    let owned = JournalIo::new(journal, Arc::clone(&capacity).try_acquire_owned().unwrap());
    let clock: Arc<dyn HostClock> = TestClock::repeated(sample(
        &Uuid::new_v4().to_string(),
        HostClockMethodV1::LinuxBoottimeV1,
        1_000,
    ));
    let activation =
        HeldCreateActivation::for_test(&fixture.intent, &peer.activation_listener).unwrap();
    let pending =
        PendingHeldCreateClaim::begin_for_test(activation, clock, &fixture.intent).unwrap();
    let result = RemoteHeldCreateCoordinator::new(client)
        .execute(&runtime, pending, owned, &AtomicBool::new(false))
        .await;
    assert!(matches!(result, Err(CoordinatorError::InvalidContext)));
    assert_eq!(peer.post_count(), 1);
    assert_eq!(
        fixture
            .store()
            .events(&EventQuery {
                kind: Some("control.kubernetes.create.result".into()),
                ..EventQuery::default()
            })
            .unwrap()
            .len(),
        0,
        "no central Result decision was requested"
    );
    let refusal: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("refusal.json")).unwrap()).unwrap();
    assert_eq!(
        refusal["refusal"], "unavailable",
        "a local screen is not a control refusal"
    );
    assert!(root.join("candidate.json").is_file());
    assert!(!root.join("ack.json").exists());
    assert_eq!(capacity.available_permits(), 1);
    server.stop().await;
}

#[tokio::test]
async fn remote_coordinator_attributes_a_received_400_to_control_rejection() {
    use crate::audit::EventQuery;

    let mut fixture = ControlFixture::new("remote-control-rejection");
    let peer = Peer::new(ResponseMode::Status(503)).await;
    let authority_id = fixture.intent.authority.authority_id();
    fixture.intent.authority = authority(authority_id, &peer.endpoint, peer.trust_digest());
    let runtime = KubernetesRuntime::connect(fixture.intent.authority.clone(), peer.credentials())
        .expect("connect only to the owned TLS peer");
    runtime.doctor().await.unwrap();

    // The gateway knows this synthetic marker, while the client only knows
    // its bearer. The marker appears in the real Result payload's api_rejected
    // outcome, so the HTTP 400 is produced by the authenticated gateway after
    // the coordinator has reached remote Result submission.
    let (server, result_requests) = fixture.serve_with_redaction_secret("api_rejected").await;
    let client = Arc::new(fixture.client(&server.url));
    let reservation = client.reserve(&fixture.intent).await.unwrap();
    let claim = fixture.claim();
    let journal = fixture.prepared_journal(&reservation, &claim);
    let root = fixture.directory.path().join("held-create-journal");
    let capacity = Arc::new(Semaphore::new(1));
    let owned = JournalIo::new(journal, Arc::clone(&capacity).try_acquire_owned().unwrap());
    let clock: Arc<dyn HostClock> = TestClock::repeated(sample(
        &Uuid::new_v4().to_string(),
        HostClockMethodV1::LinuxBoottimeV1,
        1_000,
    ));
    let activation =
        HeldCreateActivation::for_test(&fixture.intent, &peer.activation_listener).unwrap();
    let pending =
        PendingHeldCreateClaim::begin_for_test(activation, clock, &fixture.intent).unwrap();

    let result = RemoteHeldCreateCoordinator::new(client)
        .execute(&runtime, pending, owned, &AtomicBool::new(false))
        .await;
    assert_eq!(result_requests.load(Ordering::SeqCst), 1);
    assert_eq!(peer.post_count(), 1);

    let candidate: KubernetesCreateResultV1 =
        serde_json::from_slice(&fs::read(root.join("candidate.json")).unwrap()).unwrap();
    assert!(matches!(
        candidate.outcome,
        KubernetesCreateOutcomeV1::OwnershipUnknown {
            reason: KubernetesCreateUnknownReasonV1::ApiRejected,
            http_status: Some(503),
        }
    ));
    let refusal: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("refusal.json")).unwrap()).unwrap();
    assert_eq!(
        refusal["refusal"], "control_rejected",
        "an authenticated gateway 400 is distinct from local input screening"
    );
    assert!(!root.join("ack.json").exists());
    assert_eq!(
        fixture
            .store()
            .events(&EventQuery {
                kind: Some("control.kubernetes.create.result".into()),
                ..EventQuery::default()
            })
            .unwrap()
            .len(),
        0,
        "redaction refuses Result before a central result event is committed"
    );
    assert_eq!(capacity.available_permits(), 1);
    assert_eq!(
        result.as_ref().err(),
        Some(&CoordinatorError::ControlRejected)
    );
    server.stop().await;
}

#[tokio::test]
async fn remote_result_ack_loss_replays_the_committed_result_without_a_new_claim_or_runtime_post() {
    use crate::audit::EventQuery;

    let mut fixture = ControlFixture::new("remote-result-ack-loss");
    let peer = Peer::new(ResponseMode::Status(503)).await;
    let authority_id = fixture.intent.authority.authority_id();
    fixture.intent.authority = authority(authority_id, &peer.endpoint, peer.trust_digest());
    let runtime = KubernetesRuntime::connect(fixture.intent.authority.clone(), peer.credentials())
        .expect("connect only to the owned TLS peer");
    runtime.doctor().await.unwrap();

    let (server, result_requests, hidden_committed_ack) =
        fixture.serve_with_lost_result_ack_once().await;
    let client = Arc::new(fixture.client(&server.url));
    let reservation = client.reserve(&fixture.intent).await.unwrap();
    let claim = fixture.claim();
    let journal = fixture.prepared_journal(&reservation, &claim);
    let root = fixture.directory.path().join("held-create-journal");
    let capacity = Arc::new(Semaphore::new(1));
    let owned = JournalIo::new(journal, Arc::clone(&capacity).try_acquire_owned().unwrap());
    let clock: Arc<dyn HostClock> = TestClock::repeated(sample(
        &Uuid::new_v4().to_string(),
        HostClockMethodV1::LinuxBoottimeV1,
        1_000,
    ));
    let activation =
        HeldCreateActivation::for_test(&fixture.intent, &peer.activation_listener).unwrap();
    let pending =
        PendingHeldCreateClaim::begin_for_test(activation, clock, &fixture.intent).unwrap();
    let coordinator = RemoteHeldCreateCoordinator::new(client);

    let first = coordinator
        .execute(&runtime, pending, owned, &AtomicBool::new(false))
        .await;
    assert_eq!(
        first.as_ref().err(),
        Some(&CoordinatorError::ControlUnavailable)
    );
    assert!(
        hidden_committed_ack.load(Ordering::SeqCst),
        "the real gateway committed the Result before the test middleware hid its 200 ACK"
    );
    assert_eq!(result_requests.load(Ordering::SeqCst), 1);
    assert_eq!(peer.post_count(), 1);
    assert_eq!(capacity.available_permits(), 1);

    let candidate_bytes = fs::read(root.join("candidate.json")).unwrap();
    let candidate: KubernetesCreateResultV1 = serde_json::from_slice(&candidate_bytes).unwrap();
    let refusal_bytes = fs::read(root.join("refusal.json")).unwrap();
    let refusal: serde_json::Value = serde_json::from_slice(&refusal_bytes).unwrap();
    assert_eq!(refusal["refusal"], "unavailable");
    assert!(!root.join("ack.json").exists());
    let head_after_commit = fixture.store().head().unwrap();
    let result_events_after_commit = fixture
        .store()
        .events(&EventQuery {
            kind: Some("control.kubernetes.create.result".into()),
            ..EventQuery::default()
        })
        .unwrap();
    assert_eq!(result_events_after_commit.len(), 1);

    let recovered = AdmissionJournal::recover(
        Directory::open_existing(&root).unwrap(),
        fixture.journal_protection(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(recovered.phase(), RecoveryPhase::CandidateRetained);
    let owned = JournalIo::new(
        recovered.into_journal(),
        Arc::clone(&capacity).try_acquire_owned().unwrap(),
    );
    let replay = coordinator.resume_result(owned).await.unwrap();
    assert_eq!(replay.receipt.operation_id, candidate.operation_id);
    assert_eq!(replay.outcome, candidate.outcome);
    assert!(replay.handle.is_none());
    assert_eq!(result_requests.load(Ordering::SeqCst), 2);
    assert_eq!(
        peer.post_count(),
        1,
        "recovery cannot issue a second Kube POST"
    );
    assert_eq!(fixture.store().head().unwrap(), head_after_commit);
    assert_eq!(
        fixture
            .store()
            .events(&EventQuery {
                kind: Some("control.kubernetes.create.result".into()),
                ..EventQuery::default()
            })
            .unwrap()
            .len(),
        1,
        "exact Result replay does not create a second central event or Claim"
    );
    assert_eq!(
        fs::read(root.join("candidate.json")).unwrap(),
        candidate_bytes
    );
    assert_eq!(
        fs::read(root.join("refusal.json")).unwrap(),
        refusal_bytes,
        "the original lost-ACK refusal remains the first durable refusal"
    );
    let ack: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("ack.json")).unwrap()).unwrap();
    assert_eq!(ack["receipt"]["operation_id"], candidate.operation_id);
    assert_eq!(capacity.available_permits(), 1);
    server.stop().await;
}

#[tokio::test]
async fn remote_coordinator_persists_unknown_result_and_replays_without_claim_or_runtime() {
    let mut fixture = ControlFixture::new("remote-coordinator-test");
    let peer = Peer::new(ResponseMode::Status(503)).await;
    let authority_id = fixture.intent.authority.authority_id();
    fixture.intent.authority = authority(authority_id, &peer.endpoint, peer.trust_digest());
    let runtime = KubernetesRuntime::connect(fixture.intent.authority.clone(), peer.credentials())
        .expect("connect only to the owned TLS peer");
    runtime
        .doctor()
        .await
        .expect("inspect the peer before the claim window");

    let server = fixture.serve(true, QuotaPolicyV1::default()).await;
    let client = Arc::new(fixture.client(&server.url));
    let reservation = client.reserve(&fixture.intent).await.unwrap();
    let claim = fixture.claim();
    let journal = fixture.prepared_journal(&reservation, &claim);
    let root = fixture.directory.path().join("held-create-journal");
    let capacity = Arc::new(Semaphore::new(1));
    let owned = JournalIo::new(journal, Arc::clone(&capacity).try_acquire_owned().unwrap());
    let boot = Uuid::new_v4().to_string();
    let clock: Arc<dyn HostClock> =
        TestClock::repeated(sample(&boot, HostClockMethodV1::LinuxBoottimeV1, 1_000));
    let activation = HeldCreateActivation::for_test(&fixture.intent, &peer.activation_listener)
        .expect("fixture activation is tied to the exact owned listener");
    let pending =
        PendingHeldCreateClaim::begin_for_test(activation, clock, &fixture.intent).unwrap();
    let coordinator = RemoteHeldCreateCoordinator::new(Arc::clone(&client));

    let completion = coordinator
        .execute(&runtime, pending, owned, &AtomicBool::new(false))
        .await
        .expect("retain the runtime result through real control HTTP and local ACK");
    assert!(matches!(
        completion.outcome,
        KubernetesCreateOutcomeV1::OwnershipUnknown {
            reason: KubernetesCreateUnknownReasonV1::ApiRejected,
            http_status: Some(503),
        }
    ));
    assert!(completion.handle.is_none());
    assert_eq!(peer.post_count(), 1);
    assert_eq!(capacity.available_permits(), 1);
    let candidate_bytes = fs::read(root.join("candidate.json")).unwrap();
    let ack_bytes = fs::read(root.join("ack.json")).unwrap();
    let candidate: KubernetesCreateResultV1 = serde_json::from_slice(&candidate_bytes).unwrap();
    assert_eq!(candidate.outcome, completion.outcome);
    assert_eq!(candidate.claim_operation_id, claim.operation_id);
    assert_eq!(
        completion.receipt.input_hash,
        candidate.input_hash().unwrap()
    );
    let head = fixture.store().head().unwrap();

    let recovered = AdmissionJournal::recover(
        Directory::open_existing(&root).unwrap(),
        fixture.journal_protection(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(recovered.phase(), RecoveryPhase::ResultAcknowledged);
    let owned = JournalIo::new(
        recovered.into_journal(),
        Arc::clone(&capacity).try_acquire_owned().unwrap(),
    );
    let replay = coordinator.resume_result(owned).await.unwrap();
    assert_eq!(replay.receipt, completion.receipt);
    assert_eq!(replay.outcome, completion.outcome);
    assert!(replay.handle.is_none());
    assert_eq!(
        peer.post_count(),
        1,
        "recovery cannot repeat runtime CREATE"
    );
    assert_eq!(
        fixture.store().head().unwrap(),
        head,
        "no new central Claim/Result event on replay"
    );
    assert_eq!(
        fs::read(root.join("candidate.json")).unwrap(),
        candidate_bytes
    );
    assert_eq!(fs::read(root.join("ack.json")).unwrap(), ack_bytes);
    assert_eq!(capacity.available_permits(), 1);
    server.stop().await;
}
