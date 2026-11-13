//! Before-effect lifecycle over real tenant SQLite Stores, real private
//! journal directories, real broker identities (including a rotated successor)
//! and, for the report, the real HTTPS client against one owned raw TLS
//! loopback peer. Both Stores of a test belong to `tenant-a`: nothing here is a
//! cross-tenant test. Pod and PVC bodies are declared fixtures; they never
//! qualify Kubernetes, a provisioner or ReadWriteOncePod.
use super::*;
use crate::{
    audit::{
        kubernetes_admission_state_tests::Fixture,
        kubernetes_session::ConfirmedSessionReservationV2,
    },
    broker_registry::{
        BrokerAuthorityV1, BrokerGrantMutationV1, BrokerRevocationV1, BrokerRuntimeKindV1,
    },
    control::{
        QuotaPolicyV1,
        kubernetes_session::{
            KubernetesSessionEffectFenceV2, KubernetesSessionReservationIntentV2,
        },
    },
    control_fs::PrivateFile,
    runtime::kubernetes::{
        KubernetesAuthorityV1, KubernetesCredentials,
        session::tests::{fixture, fixture_at, protection, root, write_private},
        session_instance::tests::{CLAIM_UID, bound_claim, bound_pvc},
        session_pod::tests::{admitted, collect_keys},
        session_test_peer::{PeerReply, PeerRequest, SessionTestPeer},
    },
    workload::{AuthenticatedWorkload, WorkloadBindingIssueV1, WorkloadSubjectV1},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

const EXPIRY: &str = "2100-01-01T00:00:00.000Z";
const STRANGER_TOKEN: &str = "public-session-lifecycle-stranger-broker-token-0000000";
const SUCCESSOR_TOKEN: &str = "public-session-lifecycle-successor-token-000000000000";
const LOOPBACK_TOKEN: &str = "public-session-lifecycle-loopback-token-0000000000000";
const OTHER_CLAIM_UID: &str = "66666666-6666-4666-8666-666666666666";
const ANCHORS: [&str; 2] = [
    "GET /api/v1/namespaces/kube-system",
    "GET /api/v1/namespaces/tenant-a",
];

fn held(f: &Fixture) -> u32 {
    f.store
        .worker_quota(&QuotaPolicyV1::default())
        .unwrap()
        .held_workers
}
fn incarnation(intent: &KubernetesSessionReservationIntentV2) -> Uuid {
    intent.ids().unwrap().incarnation_id
}
/// The close a journal retains for its own reservation (test-built, with the
/// tenant's real input hash); the Store decides whether it commits.
fn abandonment(
    intent: &KubernetesSessionReservationIntentV2,
) -> KubernetesSessionReservationCloseV2 {
    KubernetesSessionReservationCloseV2 {
        version: 2,
        operation_id: Uuid::new_v4().to_string(),
        reservation_id: intent.reservation_id().into(),
        reservation_input_hash: intent.input_hash("tenant-a").unwrap(),
        expected_revision: 1,
        reason: KubernetesSessionReservationCloseReasonV2::PreparationAbandoned,
    }
}
fn open_journal(
    f: &Fixture,
    intent: &KubernetesSessionReservationIntentV2,
    confirmed: &ConfirmedSessionReservationV2,
    name: &str,
) -> (Directory, SessionJournal) {
    let dir = root(f, name);
    let journal =
        SessionJournal::prepare_or_resume(dir.clone(), protection(), intent, confirmed).unwrap();
    (dir, journal)
}
fn view_of(dir: &Directory) -> RetainedSessionViewV2 {
    SessionJournal::inspect_retained(dir.clone(), protection()).unwrap()
}
/// Every entry with its exact bytes, sorted by name.
fn entries(dir: &Directory) -> Vec<(String, Vec<u8>)> {
    let mut entries: Vec<(String, Vec<u8>)> = fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().into_string().unwrap(),
                fs::read(entry.path()).unwrap(),
            )
        })
        .collect();
    entries.sort();
    entries
}
fn names_in(dir: &Directory) -> Vec<String> {
    entries(dir).into_iter().map(|(name, _)| name).collect()
}
fn store_failure(error: &anyhow::Error) -> Option<KubernetesSessionReservationFailureV2> {
    error
        .downcast_ref::<KubernetesSessionReservationFailureV2>()
        .copied()
}
fn authenticated(store: &Store, token: &str) -> BrokerIdentityV1 {
    let Some(AuthenticatedWorkload::Broker(identity)) =
        store.authenticate_workload_token(token).unwrap()
    else {
        panic!("a real broker binding and grant must authenticate");
    };
    identity
}
fn recovery_policy(broker_id: &str) -> KubernetesSessionRecoveryPolicyV2 {
    KubernetesSessionRecoveryPolicyV2 {
        version: 2,
        decision_ttl_seconds: 60,
        allowed_broker_ids: vec![broker_id.into()],
    }
}
/// Revokes the fixture broker: every later Store call with its identity fails.
fn revoke(f: &Fixture) {
    f.store
        .revoke_broker(&BrokerRevocationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            broker_id: f.identity.broker_id().into(),
            expected_revision: f.identity.grant_revision(),
        })
        .unwrap();
}
/// The original broker is revoked and a successor incarnation of the same
/// broker id is authenticated with public Store APIs, as in
/// `audit/kubernetes_session/tests.rs` (successor_recovers_revoked_original).
fn rotate_successor(f: &Fixture) -> (BrokerIdentityV1, Uuid) {
    revoke(f);
    let credential = Uuid::new_v4().to_string();
    let successor = Uuid::new_v4();
    f.store
        .issue_workload_binding(&WorkloadBindingIssueV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            credential_id: credential.clone(),
            expected_revision: 0,
            subject: WorkloadSubjectV1::Broker {
                broker_id: f.identity.broker_id().into(),
                incarnation_id: successor.to_string(),
            },
            sha256_digest: hex::encode(Sha256::digest(SUCCESSOR_TOKEN.as_bytes())),
            expires_at: EXPIRY.into(),
        })
        .unwrap();
    f.store
        .mutate_broker_grant(&BrokerGrantMutationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            broker_id: f.identity.broker_id().into(),
            expected_revision: Some(f.identity.grant_revision() + 1),
            credential_id: credential,
            incarnation_id: successor.to_string(),
            authorities: vec![BrokerAuthorityV1 {
                authority_id: Uuid::new_v4().to_string(),
                runtime_kind: BrokerRuntimeKindV1::Kubernetes,
            }],
            lease_seconds: 120,
            not_after: EXPIRY.into(),
        })
        .unwrap();
    (authenticated(&f.store, SUCCESSOR_TOKEN), successor)
}
/// Another real, currently authenticated broker of the same tenant Store.
fn stranger_broker(f: &Fixture) -> BrokerIdentityV1 {
    let broker = Uuid::new_v4().to_string();
    let credential = Uuid::new_v4().to_string();
    let incarnation = Uuid::new_v4().to_string();
    f.store
        .issue_workload_binding(&WorkloadBindingIssueV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            credential_id: credential.clone(),
            expected_revision: 0,
            subject: WorkloadSubjectV1::Broker {
                broker_id: broker.clone(),
                incarnation_id: incarnation.clone(),
            },
            sha256_digest: hex::encode(Sha256::digest(STRANGER_TOKEN.as_bytes())),
            expires_at: EXPIRY.into(),
        })
        .unwrap();
    f.store
        .mutate_broker_grant(&BrokerGrantMutationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            broker_id: broker,
            expected_revision: None,
            credential_id: credential,
            incarnation_id: incarnation,
            authorities: vec![BrokerAuthorityV1 {
                authority_id: Uuid::new_v4().to_string(),
                runtime_kind: BrokerRuntimeKindV1::Kubernetes,
            }],
            lease_seconds: 120,
            not_after: EXPIRY.into(),
        })
        .unwrap();
    authenticated(&f.store, STRANGER_TOKEN)
}

#[test]
fn abandon_closes_the_real_reservation_once_and_replays_after_a_lost_response() {
    let (f, intent, confirmed) = fixture();
    let (dir, mut journal) = open_journal(&f, &intent, &confirmed, "abandon");
    let claim = bound_claim(&intent, CLAIM_UID);
    let request = retain_session_request(&mut journal, &claim).unwrap();
    assert_eq!(journal.phase().unwrap(), SessionPhaseV2::RequestRetained);
    assert_eq!(held(&f), 1);
    let head = f.store.head().unwrap();
    let closed = match abandon_before_effect(
        &f.store,
        &f.identity,
        incarnation(&intent),
        &mut journal,
        None,
    )
    .unwrap()
    {
        SessionAbandonOutcomeV2::Closed(response) => response,
        other => panic!("the first close must commit: {other:?}"),
    };
    let close = journal.retained_abandonment().unwrap().unwrap();
    assert!(!closed.replayed);
    assert_eq!(
        closed.snapshot.state.state,
        KubernetesSessionReservationStateV2::ClosedBeforeEffect
    );
    assert_eq!(closed.snapshot.state.revision, 2);
    assert!(!closed.snapshot.state.quota_held);
    assert_eq!(
        closed.snapshot.state.effect_fence,
        KubernetesSessionEffectFenceV2::NeverIssued { revision: 0 }
    );
    assert_eq!(closed.snapshot.state.last_close.as_ref(), Some(&close));
    assert_eq!(closed.receipt.operation_id, close.operation_id);
    assert_eq!(
        close.reason,
        KubernetesSessionReservationCloseReasonV2::PreparationAbandoned
    );
    assert_eq!(held(&f), 0);
    assert!(f.store.head().unwrap() > head);
    // Nothing is retained after the local fence, not even the same claim.
    assert_eq!(journal.phase().unwrap(), SessionPhaseV2::Abandoned);
    assert_eq!(
        journal.retain_pod_request(&request),
        Err(SessionError::Conflict)
    );
    assert_eq!(
        journal.retain_workspace_claim(&claim),
        Err(SessionError::Conflict)
    );
    // The response was lost: the same live journal replays the receipt.
    let head = f.store.head().unwrap();
    match abandon_before_effect(
        &f.store,
        &f.identity,
        incarnation(&intent),
        &mut journal,
        None,
    )
    .unwrap()
    {
        SessionAbandonOutcomeV2::Replayed(response) => {
            assert!(response.replayed);
            assert_eq!(response.receipt, closed.receipt);
        }
        other => panic!("a retry must replay: {other:?}"),
    }
    drop(journal);
    // After the process stopped, the observe-only view replays it too.
    let view = view_of(&dir);
    assert_eq!(view.phase(), SessionPhaseV2::Abandoned);
    assert_eq!(view.abandonment(), Some(&close));
    assert_eq!(view.request(), Some(&request));
    match resume_abandonment(&f.store, &f.identity, incarnation(&intent), &view, None).unwrap() {
        SessionAbandonOutcomeV2::Replayed(response) => {
            assert_eq!(response.receipt, closed.receipt);
        }
        other => panic!("a resumed close must replay: {other:?}"),
    }
    assert_eq!(f.store.head().unwrap(), head);
    assert_eq!(held(&f), 0);
    // Baseline: another operation id for the same reservation conflicts.
    let fresh = abandonment(&intent);
    let error = f
        .store
        .authorize_kubernetes_session_reconciliation(
            &f.identity,
            intent.reservation_id(),
            Some(&fresh),
        )
        .err()
        .unwrap();
    assert_eq!(
        store_failure(&error),
        Some(KubernetesSessionReservationFailureV2::Conflict)
    );
    assert_eq!(f.store.head().unwrap(), head);
    assert_eq!(
        names_in(&dir),
        [
            ".session-v2.lock",
            "abandoned-v2.json",
            "pod-v2.json",
            "prepared-v2.json",
            "workspace-v2.json",
        ]
    );
}

#[test]
fn successor_incarnation_closes_through_recovery_and_replays_through_receipt_lookup() {
    let (f, intent, confirmed) = fixture();
    let (dir, mut journal) = open_journal(&f, &intent, &confirmed, "successor");
    retain_session_request(&mut journal, &bound_claim(&intent, CLAIM_UID)).unwrap();
    // The predecessor retains its close (local fence first) and stops before
    // any Store call.
    let close = abandonment(&intent);
    journal.retain_abandonment(&close).unwrap();
    drop(journal);
    let view = view_of(&dir);
    assert_eq!(view.abandonment(), Some(&close));
    let (successor, successor_incarnation) = rotate_successor(&f);
    assert_ne!(successor_incarnation, incarnation(&intent));
    // The revoked original identity can no longer close.
    let head = f.store.head().unwrap();
    assert!(resume_abandonment(&f.store, &f.identity, incarnation(&intent), &view, None).is_err());
    assert_eq!(held(&f), 1);
    assert_eq!(f.store.head().unwrap(), head);
    let policy = recovery_policy(f.identity.broker_id());
    let closed = match resume_abandonment(
        &f.store,
        &successor,
        successor_incarnation,
        &view,
        Some(&policy),
    )
    .unwrap()
    {
        SessionAbandonOutcomeV2::Closed(response) => response,
        other => panic!("the successor must close through recovery: {other:?}"),
    };
    assert!(!closed.replayed);
    assert_eq!(closed.snapshot.state.last_close.as_ref(), Some(&close));
    assert_eq!(closed.receipt.operation_id, close.operation_id);
    assert_eq!(held(&f), 0);
    // Lost response: the closed reservation replays through receipt lookup.
    let head = f.store.head().unwrap();
    match resume_abandonment(
        &f.store,
        &successor,
        successor_incarnation,
        &view,
        Some(&policy),
    )
    .unwrap()
    {
        SessionAbandonOutcomeV2::Replayed(response) => {
            assert!(response.replayed);
            assert_eq!(response.receipt, closed.receipt);
        }
        other => panic!("the successor retry must replay: {other:?}"),
    }
    assert_eq!(f.store.head().unwrap(), head);
    assert_eq!(held(&f), 0);
    assert!(f.store.integrity().unwrap().valid);
}

#[test]
fn predecessor_close_replays_for_a_successor_through_receipt_lookup() {
    let (f, intent, confirmed) = fixture();
    let (dir, mut journal) = open_journal(&f, &intent, &confirmed, "predecessor-closed");
    retain_session_request(&mut journal, &bound_claim(&intent, CLAIM_UID)).unwrap();
    // The predecessor commits its same-incarnation close, then its response
    // is lost and the broker restarts as another incarnation.
    let closed = match abandon_before_effect(
        &f.store,
        &f.identity,
        incarnation(&intent),
        &mut journal,
        None,
    )
    .unwrap()
    {
        SessionAbandonOutcomeV2::Closed(response) => response,
        other => panic!("the predecessor close must commit: {other:?}"),
    };
    assert_eq!(held(&f), 0);
    drop(journal);
    let view = view_of(&dir);
    assert_eq!(view.phase(), SessionPhaseV2::Abandoned);
    let (successor, successor_incarnation) = rotate_successor(&f);
    assert_ne!(successor_incarnation, incarnation(&intent));
    let policy = recovery_policy(f.identity.broker_id());
    let head = f.store.head().unwrap();
    // The successor replays the predecessor's committed close through the
    // receipt lookup: same receipt, no new event, no quota change.
    match resume_abandonment(
        &f.store,
        &successor,
        successor_incarnation,
        &view,
        Some(&policy),
    )
    .unwrap()
    {
        SessionAbandonOutcomeV2::Replayed(response) => {
            assert!(response.replayed);
            assert_eq!(response.receipt, closed.receipt);
            assert_eq!(
                response.snapshot.state.last_close.as_ref(),
                view.abandonment()
            );
        }
        other => panic!("the successor must replay the predecessor close: {other:?}"),
    }
    assert_eq!(f.store.head().unwrap(), head);
    assert_eq!(held(&f), 0);
    assert!(f.store.integrity().unwrap().valid);
}

#[test]
fn successor_without_recovery_policy_is_refused_before_any_store_call() {
    let (f, intent, confirmed) = fixture();
    let (dir, mut journal) = open_journal(&f, &intent, &confirmed, "no-policy");
    // A live journal of another incarnation retains nothing without a policy.
    let before = entries(&dir);
    assert!(matches!(
        abandon_before_effect(&f.store, &f.identity, Uuid::new_v4(), &mut journal, None),
        Ok(SessionAbandonOutcomeV2::SuccessorRequiresRecoveryPolicy)
    ));
    assert_eq!(entries(&dir), before);
    journal.retain_abandonment(&abandonment(&intent)).unwrap();
    drop(journal);
    let (bare_dir, bare) = open_journal(&f, &intent, &confirmed, "no-close");
    drop(bare);
    let view = view_of(&dir);
    let bare = view_of(&bare_dir);
    // Revoked: any Store call with this identity now fails, so an Ok outcome
    // below shows that no Store call was made.
    revoke(&f);
    let head = f.store.head().unwrap();
    let policy = recovery_policy(f.identity.broker_id());
    assert!(matches!(
        resume_abandonment(&f.store, &f.identity, Uuid::new_v4(), &view, None),
        Ok(SessionAbandonOutcomeV2::SuccessorRequiresRecoveryPolicy)
    ));
    for own in [incarnation(&intent), Uuid::new_v4()] {
        assert!(matches!(
            resume_abandonment(&f.store, &f.identity, own, &bare, Some(&policy)),
            Ok(SessionAbandonOutcomeV2::NoRetainedAbandonment)
        ));
    }
    // Control: the same identity does reach the Store on the
    // same-incarnation branch, and is refused there.
    assert!(resume_abandonment(&f.store, &f.identity, incarnation(&intent), &view, None).is_err());
    assert_eq!(f.store.head().unwrap(), head);
    assert_eq!(held(&f), 1);
}

#[test]
fn close_by_another_operation_is_classified_by_observation() {
    let (f, intent, confirmed) = fixture();
    let (dir, mut journal) = open_journal(&f, &intent, &confirmed, "closed-elsewhere");
    journal.retain_abandonment(&abandonment(&intent)).unwrap();
    // Another operation with its own id commits first through the Store.
    let other = KubernetesSessionReservationCloseV2 {
        reason: KubernetesSessionReservationCloseReasonV2::OperatorCancelled,
        ..abandonment(&intent)
    };
    let authorization = f
        .store
        .authorize_kubernetes_session_reconciliation(
            &f.identity,
            intent.reservation_id(),
            Some(&other),
        )
        .unwrap();
    f.store
        .close_kubernetes_session_before_effect(&f.identity, &authorization, &other)
        .unwrap();
    assert_eq!(held(&f), 0);
    let head = f.store.head().unwrap();
    match abandon_before_effect(
        &f.store,
        &f.identity,
        incarnation(&intent),
        &mut journal,
        None,
    )
    .unwrap()
    {
        SessionAbandonOutcomeV2::ClosedCentrallyByAnotherOperation {
            last_close_operation_id,
        } => assert_eq!(last_close_operation_id, other.operation_id),
        outcome => panic!("the other close must be observed: {outcome:?}"),
    }
    drop(journal);
    let view = view_of(&dir);
    match resume_abandonment(&f.store, &f.identity, incarnation(&intent), &view, None).unwrap() {
        SessionAbandonOutcomeV2::ClosedCentrallyByAnotherOperation {
            last_close_operation_id,
        } => assert_eq!(last_close_operation_id, other.operation_id),
        outcome => panic!("the other close must be observed again: {outcome:?}"),
    }
    assert_eq!(f.store.head().unwrap(), head);
    // A successor cannot observe a closed reservation: its retained operation
    // is reported as not found, which stays ambiguous.
    let (successor, successor_incarnation) = rotate_successor(&f);
    let head = f.store.head().unwrap();
    assert!(matches!(
        resume_abandonment(
            &f.store,
            &successor,
            successor_incarnation,
            &view,
            Some(&recovery_policy(f.identity.broker_id())),
        ),
        Ok(SessionAbandonOutcomeV2::SuccessorLookupNotFound)
    ));
    assert_eq!(f.store.head().unwrap(), head);
    assert_eq!(held(&f), 0);
}

#[test]
fn abandon_is_refused_when_any_effect_checkpoint_name_is_present() {
    let (f, intent, confirmed) = fixture();
    let reserved = b"{\"reserved\":true}";
    // A future effect checkpoint name appears while the journal is live.
    let (dir, mut journal) = open_journal(&f, &intent, &confirmed, "effect-name");
    retain_session_request(&mut journal, &bound_claim(&intent, CLAIM_UID)).unwrap();
    write_private(&dir, "create-possible-v2.json", reserved);
    let head = f.store.head().unwrap();
    assert_eq!(
        abandon_before_effect(
            &f.store,
            &f.identity,
            incarnation(&intent),
            &mut journal,
            None,
        )
        .err(),
        Some(SessionLifecycleError::Journal(SessionError::Corrupt))
    );
    assert!(!dir.path().join("abandoned-v2.json").exists());
    drop(journal);
    assert!(matches!(
        SessionJournal::inspect_retained(dir.clone(), protection()),
        Err(SessionError::Corrupt)
    ));
    assert_eq!(
        fs::read(dir.path().join("create-possible-v2.json")).unwrap(),
        reserved
    );
    // The same refusal once a close is retained: every journal access
    // revalidates the inventory first (here the first one, reading the
    // intent), so no Store call follows. A file appearing between the last
    // revalidation and the Store call cannot be interleaved by this test.
    let (second, mut journal) = open_journal(&f, &intent, &confirmed, "effect-after-close");
    journal.retain_abandonment(&abandonment(&intent)).unwrap();
    write_private(&second, "created-v2.json", reserved);
    assert_eq!(
        abandon_before_effect(
            &f.store,
            &f.identity,
            incarnation(&intent),
            &mut journal,
            None,
        )
        .err(),
        Some(SessionLifecycleError::Journal(SessionError::Corrupt))
    );
    drop(journal);
    assert!(matches!(
        SessionJournal::inspect_retained(second.clone(), protection()),
        Err(SessionError::Corrupt)
    ));
    // A well-formed abandonment that this journal would never retain (another
    // reason) is not adopted and is preserved.
    let (third, journal) = open_journal(&f, &intent, &confirmed, "foreign-close");
    let preparation = journal.bootstrap_binding().unwrap().preparation_sha256;
    drop(journal);
    let foreign = KubernetesSessionReservationCloseV2 {
        reason: KubernetesSessionReservationCloseReasonV2::OperatorCancelled,
        ..abandonment(&intent)
    };
    let bytes = crate::control::kubernetes_session::validation::canonical(
        &json!({"version":1,"preparation_sha256":preparation,"close":foreign}),
    )
    .unwrap();
    write_private(&third, "abandoned-v2.json", &bytes);
    assert!(matches!(
        SessionJournal::prepare_or_resume(third.clone(), protection(), &intent, &confirmed),
        Err(SessionError::Corrupt)
    ));
    assert!(matches!(
        SessionJournal::inspect_retained(third.clone(), protection()),
        Err(SessionError::Corrupt)
    ));
    assert_eq!(
        fs::read(third.path().join("abandoned-v2.json")).unwrap(),
        bytes
    );
    // Control: the same hand-written record with the reason this journal
    // retains is adopted, so the refusal above comes from the reason check.
    let (fourth, journal) = open_journal(&f, &intent, &confirmed, "own-close");
    let own_preparation = journal.bootstrap_binding().unwrap().preparation_sha256;
    drop(journal);
    let own = abandonment(&intent);
    let own_bytes = crate::control::kubernetes_session::validation::canonical(
        &json!({"version":1,"preparation_sha256":own_preparation,"close":own}),
    )
    .unwrap();
    write_private(&fourth, "abandoned-v2.json", &own_bytes);
    let adopted =
        SessionJournal::prepare_or_resume(fourth.clone(), protection(), &intent, &confirmed)
            .unwrap();
    assert_eq!(adopted.phase().unwrap(), SessionPhaseV2::Abandoned);
    assert_eq!(adopted.retained_abandonment().unwrap(), Some(own));
    drop(adopted);
    assert_eq!(view_of(&fourth).phase(), SessionPhaseV2::Abandoned);
    assert_eq!(
        fs::read(fourth.path().join("abandoned-v2.json")).unwrap(),
        own_bytes
    );
    // No Store call happened: no event and the quota is still held.
    assert_eq!(f.store.head().unwrap(), head);
    assert_eq!(held(&f), 1);
}

#[test]
fn other_reservation_or_other_broker_cannot_reopen_or_abandon() {
    let (f, intent, confirmed) = fixture();
    // A second real reservation in its own Store (also tenant-a).
    let (g, other_intent, other_confirmed) = fixture();
    let (dir, journal) = open_journal(&f, &intent, &confirmed, "owned");
    drop(journal);
    let stranger = stranger_broker(&f);
    let heads = (f.store.head().unwrap(), g.store.head().unwrap());
    // The other reservation's real confirmation cannot reopen this journal.
    assert!(matches!(
        SessionJournal::prepare_or_resume(
            dir.clone(),
            protection(),
            &other_intent,
            &other_confirmed
        ),
        Err(SessionError::Conflict)
    ));
    // Another authenticated broker of this Store is refused by the Store, on
    // both branches.
    let mut journal =
        SessionJournal::prepare_or_resume(dir.clone(), protection(), &intent, &confirmed).unwrap();
    assert_eq!(
        abandon_before_effect(
            &f.store,
            &stranger,
            incarnation(&intent),
            &mut journal,
            None
        )
        .err(),
        Some(SessionLifecycleError::Control(
            KubernetesSessionReservationFailureV2::PermissionDenied
        ))
    );
    assert_eq!(
        abandon_before_effect(
            &f.store,
            &stranger,
            Uuid::new_v4(),
            &mut journal,
            Some(&recovery_policy(stranger.broker_id())),
        )
        .err(),
        Some(SessionLifecycleError::Control(
            KubernetesSessionReservationFailureV2::PermissionDenied
        ))
    );
    drop(journal);
    let view = view_of(&dir);
    // The other Store does not hold this reservation at all.
    assert_eq!(
        resume_abandonment(&g.store, &g.identity, incarnation(&intent), &view, None).err(),
        Some(SessionLifecycleError::Control(
            KubernetesSessionReservationFailureV2::NotFound
        ))
    );
    assert_eq!((f.store.head().unwrap(), g.store.head().unwrap()), heads);
    assert_eq!((held(&f), held(&g)), (1, 1));
    // The close retained during the refused attempts stays the owner's own.
    assert!(matches!(
        resume_abandonment(&f.store, &f.identity, incarnation(&intent), &view, None),
        Ok(SessionAbandonOutcomeV2::Closed(_))
    ));
    assert_eq!((held(&f), held(&g)), (0, 1));
    assert_eq!(g.store.head().unwrap(), heads.1);
}

/// A real reservation for one owned loopback peer and a runtime bound to it.
struct Loopback {
    peer: SessionTestPeer,
    token: PathBuf,
    trust: String,
    fixture: Fixture,
    intent: KubernetesSessionReservationIntentV2,
    confirmed: ConfirmedSessionReservationV2,
    runtime: KubernetesRuntime,
}
impl Loopback {
    async fn new() -> Self {
        let peer = SessionTestPeer::start().await;
        let trust = PrivateFile::open(peer.ca(), 64 * 1024)
            .unwrap()
            .digest()
            .to_owned();
        let token = peer.ca().parent().unwrap().join("session-lifecycle-token");
        fs::write(&token, LOOPBACK_TOKEN).unwrap();
        fs::set_permissions(&token, fs::Permissions::from_mode(0o600)).unwrap();
        let (fixture, intent, confirmed) = fixture_at(peer.endpoint(), &trust);
        assert_eq!(intent.authority().namespace(), "tenant-a");
        let runtime = connect(&peer, &token, intent.authority());
        Self {
            peer,
            token,
            trust,
            fixture,
            intent,
            confirmed,
            runtime,
        }
    }
    /// Active anchors, then the Pod and the PVC at their session names (404
    /// when `None`). Any other request, including every non-GET, is a 404.
    fn answer(&self, pod: Option<Value>, pvc: Option<Value>) {
        let cluster_uid = self.intent.authority().cluster_namespace_uid.clone();
        let namespace_uid = self.intent.authority().namespace_uid.clone();
        self.peer.respond(move |request: &PeerRequest| {
            let target = request.target();
            let object = |value: &Option<Value>| {
                value
                    .as_ref()
                    .map_or_else(not_found, |value| PeerReply::json(200, value))
            };
            if request.method() != "GET" {
                not_found()
            } else if target == "/api/v1/namespaces/kube-system" {
                namespace_reply("kube-system", &cluster_uid)
            } else if target == "/api/v1/namespaces/tenant-a" {
                namespace_reply("tenant-a", &namespace_uid)
            } else if target.starts_with("/api/v1/namespaces/tenant-a/pods/") {
                object(&pod)
            } else if target.starts_with("/api/v1/namespaces/tenant-a/persistentvolumeclaims/") {
                object(&pvc)
            } else {
                not_found()
            }
        });
    }
    fn lines(&self) -> Vec<String> {
        self.peer.requests().iter().map(PeerRequest::line).collect()
    }
    fn only_gets(&self) -> bool {
        self.peer
            .requests()
            .iter()
            .all(|request| request.method() == "GET")
    }
}
fn connect(
    peer: &SessionTestPeer,
    token: &Path,
    authority: &KubernetesAuthorityV1,
) -> KubernetesRuntime {
    KubernetesRuntime::connect(
        authority.clone(),
        KubernetesCredentials::bearer(
            PrivateFile::open(peer.ca(), 64 * 1024).unwrap(),
            PrivateFile::open(token, 8192).unwrap(),
        ),
    )
    .unwrap()
}
fn namespace_reply(name: &str, uid: &str) -> PeerReply {
    PeerReply::json(
        200,
        &json!({"apiVersion":"v1","kind":"Namespace","metadata":{"name":name,"uid":uid},
            "status":{"phase":"Active"}}),
    )
}
fn not_found() -> PeerReply {
    PeerReply::json(404, &json!({"kind":"Status","code":404}))
}

#[tokio::test]
async fn reconcile_report_classifies_without_adoption_or_quota_change() {
    let case = Loopback::new().await;
    let (f, intent) = (&case.fixture, &case.intent);
    // A journal without a retained request issues no request at all, and its
    // report says that the names were not inspected.
    let (early, journal) = open_journal(f, intent, &case.confirmed, "report-early");
    drop(journal);
    let report = inspect_session_report(&case.runtime, early.clone(), protection())
        .await
        .unwrap();
    assert_eq!(report.phase(), SessionPhaseV2::Prepared);
    assert!(report.names().is_none());
    assert_eq!(
        report.interpretation(),
        SessionInterpretationV2::NoRequestRetained
    );
    // Uninspected names are absent from the serialized report, not null.
    let wire = serde_json::to_value(&report).unwrap();
    assert!(wire.get("names").is_none(), "{wire}");
    assert_eq!(wire["interpretation"], json!("no_request_retained"));
    assert!(case.peer.requests().is_empty());
    assert_eq!(case.peer.accepted(), 0);
    // A retained request: names come from its rendered target only.
    let (dir, mut journal) = open_journal(f, intent, &case.confirmed, "report");
    let request = retain_session_request(&mut journal, &bound_claim(intent, CLAIM_UID)).unwrap();
    drop(journal);
    let target = request.target();
    let claim_name = target.workspace_claim_name().to_owned();
    let bound = bound_pvc("tenant-a", &claim_name, CLAIM_UID);
    let head = f.store.head().unwrap();
    let before = entries(&dir);
    case.answer(None, Some(bound.clone()));
    let report = inspect_session_report(&case.runtime, dir.clone(), protection())
        .await
        .unwrap();
    assert_eq!(
        report.interpretation(),
        SessionInterpretationV2::NoEffectIssuedLocally
    );
    assert_eq!(report.phase(), SessionPhaseV2::RequestRetained);
    assert_eq!(report.reservation_id(), intent.reservation_id());
    assert_eq!(
        report.attempt_id(),
        intent.ids().unwrap().attempt_id.to_string()
    );
    let names = report.names().unwrap();
    assert_eq!(
        names.pod(),
        &KubernetesSessionObjectPresenceV2::AbsentObserved404
    );
    assert_eq!(names.workspace_claim_uid_matches_retained(), Some(true));
    assert_eq!(
        case.lines(),
        [
            ANCHORS[0].to_owned(),
            ANCHORS[1].to_owned(),
            format!("GET /api/v1/namespaces/tenant-a/pods/{}", target.pod_name()),
            format!("GET /api/v1/namespaces/tenant-a/persistentvolumeclaims/{claim_name}"),
            ANCHORS[0].to_owned(),
            ANCHORS[1].to_owned(),
        ]
    );
    // Descriptive only: no key of the report names an owner. Inspected names
    // are serialized as an object.
    let wire = serde_json::to_value(&report).unwrap();
    assert!(wire.get("names").is_some_and(Value::is_object), "{wire}");
    let mut keys = BTreeSet::new();
    collect_keys(&wire, &mut keys);
    assert!(!keys.iter().any(|key| key.contains("own")), "{keys:?}");
    // Something exists under the derived Pod name: reported, never adopted.
    case.answer(Some(admitted(&request)), Some(bound.clone()));
    let report = inspect_session_report(&case.runtime, dir.clone(), protection())
        .await
        .unwrap();
    assert_eq!(
        report.interpretation(),
        SessionInterpretationV2::ObjectPresentAtSessionName
    );
    // Another claim UID at the attempt name, the retained claim UID being
    // deleted, then no claim at all.
    let mut terminating = bound.clone();
    terminating["metadata"]["deletionTimestamp"] = json!("2026-11-12T08:05:00Z");
    for pvc in [
        Some(bound_pvc("tenant-a", &claim_name, OTHER_CLAIM_UID)),
        Some(terminating),
        None,
    ] {
        case.answer(None, pvc);
        let report = inspect_session_report(&case.runtime, dir.clone(), protection())
            .await
            .unwrap();
        assert_eq!(
            report.interpretation(),
            SessionInterpretationV2::WorkspaceClaimChanged
        );
    }
    // No effect request, no central event, no quota change, no file change;
    // every accepted connection carried a parsed GET.
    assert!(case.only_gets());
    assert_eq!(case.peer.requests().len(), 30);
    assert_eq!(case.peer.accepted(), 30);
    assert_eq!(f.store.head().unwrap(), head);
    assert_eq!(held(f), 1);
    assert_eq!(entries(&dir), before);
    // A retained abandonment is reported; the central state is not observed.
    let mut journal =
        SessionJournal::prepare_or_resume(dir.clone(), protection(), intent, &case.confirmed)
            .unwrap();
    assert!(matches!(
        abandon_before_effect(
            &f.store,
            &f.identity,
            incarnation(intent),
            &mut journal,
            None
        ),
        Ok(SessionAbandonOutcomeV2::Closed(_))
    ));
    drop(journal);
    let head = f.store.head().unwrap();
    case.answer(None, Some(bound));
    let report = inspect_session_report(&case.runtime, dir.clone(), protection())
        .await
        .unwrap();
    assert_eq!(
        report.interpretation(),
        SessionInterpretationV2::AbandonmentRetained
    );
    assert_eq!(report.phase(), SessionPhaseV2::Abandoned);
    assert!(case.only_gets());
    assert_eq!(f.store.head().unwrap(), head);
    assert_eq!(held(f), 0);
}

#[tokio::test]
async fn reconcile_report_refuses_another_authority_before_any_request() {
    let case = Loopback::new().await;
    let (dir, mut journal) = open_journal(&case.fixture, &case.intent, &case.confirmed, "foreign");
    let request =
        retain_session_request(&mut journal, &bound_claim(&case.intent, CLAIM_UID)).unwrap();
    drop(journal);
    case.answer(
        None,
        Some(bound_pvc(
            "tenant-a",
            request.target().workspace_claim_name(),
            CLAIM_UID,
        )),
    );
    // Same endpoint, trust, namespace and anchors; another authority id, so
    // another fingerprint.
    let authority = case.intent.authority();
    let elsewhere = KubernetesAuthorityV1::new(
        Uuid::new_v4(),
        case.peer.endpoint(),
        &authority.cluster_namespace_uid,
        authority.namespace(),
        &authority.namespace_uid,
        &case.trust,
    )
    .unwrap();
    assert_ne!(elsewhere.fingerprint(), authority.fingerprint());
    let runtime = connect(&case.peer, &case.token, &elsewhere);
    let before = entries(&dir);
    assert_eq!(
        inspect_session_report(&runtime, dir.clone(), protection())
            .await
            .err(),
        Some(SessionLifecycleError::InvalidContext)
    );
    assert!(case.peer.requests().is_empty());
    assert_eq!(case.peer.accepted(), 0);
    assert_eq!(entries(&dir), before);
    // Control: the journal's own authority reaches the peer, with GETs only.
    let report = inspect_session_report(&case.runtime, dir, protection())
        .await
        .unwrap();
    assert_eq!(
        report.interpretation(),
        SessionInterpretationV2::NoEffectIssuedLocally
    );
    assert_eq!(case.peer.requests().len(), 6);
    assert_eq!(case.peer.accepted(), 6);
    assert!(case.only_gets());
}

#[tokio::test]
async fn live_owner_makes_observe_only_reopen_busy() {
    let case = Loopback::new().await;
    let (f, intent) = (&case.fixture, &case.intent);
    let (dir, mut journal) = open_journal(f, intent, &case.confirmed, "live");
    retain_session_request(&mut journal, &bound_claim(intent, CLAIM_UID)).unwrap();
    case.answer(None, None);
    let busy = |dir: &Directory| {
        matches!(
            SessionJournal::inspect_retained(dir.clone(), protection()),
            Err(SessionError::Busy)
        )
    };
    let before = entries(&dir);
    assert!(busy(&dir));
    assert_eq!(
        inspect_session_report(&case.runtime, dir.clone(), protection())
            .await
            .err(),
        Some(SessionLifecycleError::Journal(SessionError::Busy))
    );
    assert!(matches!(
        SessionJournal::prepare_or_resume(dir.clone(), protection(), intent, &case.confirmed),
        Err(SessionError::Busy)
    ));
    assert_eq!(entries(&dir), before);
    assert!(case.peer.requests().is_empty());
    // The owner keeps working while observers are refused.
    assert!(matches!(
        abandon_before_effect(
            &f.store,
            &f.identity,
            incarnation(intent),
            &mut journal,
            None
        ),
        Ok(SessionAbandonOutcomeV2::Closed(_))
    ));
    let after = entries(&dir);
    assert!(busy(&dir));
    assert_eq!(entries(&dir), after);
    drop(journal);
    // Once the owner is gone the same reading succeeds and changes nothing.
    assert_eq!(view_of(&dir).phase(), SessionPhaseV2::Abandoned);
    assert_eq!(entries(&dir), after);
    assert!(case.peer.requests().is_empty());
    assert_eq!(case.peer.accepted(), 0);
}
