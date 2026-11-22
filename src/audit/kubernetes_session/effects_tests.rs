//! The effect ledger at its Store seam: real fresh SQLite, authenticated
//! broker bindings and real reservations with synthetic metadata. The
//! request digests come from the runtime builders that send the same
//! requests (their exact bodies are pinned by the runtime tests). No cluster.
use crate::audit::kubernetes_session::tests::{intent as session_intent, policy, prepare};
use super::*;
use crate::{
    audit::kubernetes_admission_state_tests::Fixture,
    control::QuotaPolicyV1,
    runtime::kubernetes::{
        session_claim::KubernetesSessionStorageRequestV2,
        session_pod::{session_pod_name, session_workspace_claim_name},
        session_requests::{delete_claim_request, release_finalizer_request, release_gate_request},
    },
};
use uuid::Uuid;

const CLAIM_UID: &str = "55555555-5555-4555-8555-555555555555";
const POD_UID: &str = "33333333-3333-4333-8333-333333333333";
const STAMP: &str = "2026-11-21T19:00:00Z";

fn storage() -> KubernetesSessionStorageRequestV2 {
    KubernetesSessionStorageRequestV2 {
        version: 1,
        storage_class_name: "ai-guard-rwop".into(),
        capacity_mib: 1024,
    }
}
fn storage_policy() -> KubernetesSessionStoragePolicyV2 {
    KubernetesSessionStoragePolicyV2 {
        version: 1,
        allowed_storage_classes: vec!["ai-guard-rwop".into()],
        max_capacity_mib: 4096,
    }
}
fn reserve(f: &Fixture, quota: &QuotaPolicyV1) -> KubernetesSessionReservationIntentV2 {
    let intent = session_intent(f, Uuid::new_v4());
    let policy = policy(&intent);
    let authorization = f
        .store
        .authorize_kubernetes_session_reservation(&f.identity, &intent, &policy, quota)
        .unwrap();
    f.store
        .reserve_kubernetes_session(&f.identity, &authorization, &intent, &policy, quota)
        .unwrap();
    intent
}
fn enable(f: &Fixture) -> crate::control::MigrationReportV1 {
    let backup = f._directory.path().join("before-effects.sqlite");
    f.store
        .enable_kubernetes_session_effects_v1(&backup)
        .unwrap()
}
fn failure(error: anyhow::Error) -> Failure {
    *error
        .downcast_ref::<Failure>()
        .unwrap_or_else(|| panic!("typed failure expected: {error:#}"))
}

struct Chain<'a> {
    f: &'a Fixture,
    intent: KubernetesSessionReservationIntentV2,
    step: u32,
}
impl<'a> Chain<'a> {
    fn new(f: &'a Fixture, intent: KubernetesSessionReservationIntentV2) -> Self {
        Self { f, intent, step: 0 }
    }
    fn request(
        &self,
        subject: Subject,
        action: Action,
        digest: String,
    ) -> KubernetesSessionEffectRequestV2 {
        KubernetesSessionEffectRequestV2 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            reservation_id: self.intent.reservation_id().into(),
            reservation_input_hash: self.intent.input_hash("tenant-a").unwrap(),
            step: self.step + 1,
            subject,
            action,
            request_sha256: digest,
            object_uid: None,
            object_resource_version: None,
            storage: None,
            deadline: None,
        }
    }
    fn claim_request(&self) -> KubernetesSessionEffectRequestV2 {
        let digest = KubernetesSessionClaimRequestV2::render(&self.intent, &storage())
            .unwrap()
            .target()
            .request_sha256()
            .to_owned();
        KubernetesSessionEffectRequestV2 {
            storage: Some(storage()),
            ..self.request(Subject::WorkspaceClaim, Action::Create, digest)
        }
    }
    fn pod_create(&self) -> KubernetesSessionEffectRequestV2 {
        let (_, digest) =
            effectful_request_bytes(&self.intent, SessionPodPurposeV2::Session).unwrap();
        self.request(Subject::SessionPod, Action::Create, digest)
    }
    fn release(&self, uid: &str) -> KubernetesSessionEffectRequestV2 {
        let name = session_pod_name(self.intent.ids().unwrap().attempt_id);
        let digest = release_gate_request(self.intent.authority(), &name, uid, "44")
            .unwrap()
            .request_sha256;
        KubernetesSessionEffectRequestV2 {
            object_uid: Some(uid.into()),
            object_resource_version: Some("44".into()),
            deadline: Some(KubernetesSessionDeadlineArmV2 {
                version: 1,
                arm_id: Uuid::new_v4().to_string(),
                deadline_at: "2026-11-21T19:10:00Z".into(),
                enforcer_receipt_sha256: "c".repeat(64),
            }),
            ..self.request(Subject::SessionPod, Action::ReleaseGate, digest)
        }
    }
    fn attach(&self, uid: &str) -> KubernetesSessionEffectRequestV2 {
        KubernetesSessionEffectRequestV2 {
            object_uid: Some(uid.into()),
            ..self.request(Subject::SessionPod, Action::Attach, "d".repeat(64))
        }
    }
    fn finalizer(&self, uid: &str) -> KubernetesSessionEffectRequestV2 {
        let name = session_pod_name(self.intent.ids().unwrap().attempt_id);
        let digest = release_finalizer_request(self.intent.authority(), &name, uid)
            .unwrap()
            .request_sha256;
        KubernetesSessionEffectRequestV2 {
            object_uid: Some(uid.into()),
            ..self.request(Subject::SessionPod, Action::ReleaseFinalizer, digest)
        }
    }
    fn claim_delete(&self, uid: &str) -> KubernetesSessionEffectRequestV2 {
        let name = session_workspace_claim_name(self.intent.ids().unwrap().attempt_id);
        let digest = delete_claim_request(self.intent.authority(), &name, uid)
            .unwrap()
            .request_sha256;
        KubernetesSessionEffectRequestV2 {
            object_uid: Some(uid.into()),
            ..self.request(Subject::WorkspaceClaim, Action::Delete, digest)
        }
    }
    fn claim(
        &mut self,
        request: &KubernetesSessionEffectRequestV2,
    ) -> anyhow::Result<KubernetesSessionEffectClaimAckV2> {
        let ack = self.f.store.claim_kubernetes_session_effect(
            &self.f.identity,
            request,
            Some(&storage_policy()),
            30,
        )?;
        self.step = ack.request.step.max(self.step);
        Ok(ack)
    }
    fn result(
        &self,
        ack: &KubernetesSessionEffectClaimAckV2,
        outcome: KubernetesSessionEffectOutcomeV2,
    ) -> anyhow::Result<KubernetesSessionEffectStateV2> {
        self.f.store.record_kubernetes_session_effect_result(
            &self.f.identity,
            &KubernetesSessionEffectResultV2 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                reservation_id: self.intent.reservation_id().into(),
                step: ack.request.step,
                claim_operation_id: ack.request.operation_id.clone(),
                request_sha256: ack.request.request_sha256.clone(),
                outcome,
                observed_at: STAMP.into(),
            },
        )
    }
    fn applied(&mut self, request: KubernetesSessionEffectRequestV2, uid: &str) {
        let ack = self.claim(&request).unwrap();
        self.result(
            &ack,
            KubernetesSessionEffectOutcomeV2::Applied {
                object_uid: uid.into(),
                resource_version: "45".into(),
            },
        )
        .unwrap();
    }
    fn terminal(&self, uid: &str) -> anyhow::Result<KubernetesSessionEffectStateV2> {
        self.f.store.record_kubernetes_session_terminal(
            &self.f.identity,
            &KubernetesSessionTerminalV2 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                reservation_id: self.intent.reservation_id().into(),
                subject: Subject::SessionPod,
                pod_uid: uid.into(),
                end: KubernetesSessionPodEndV2::Terminated {
                    container_id: "containerd://0123".into(),
                    exit_code: Some(0),
                    oom_killed: Some(false),
                    started_at: Some(STAMP.into()),
                    finished_at: Some("2026-11-21T19:05:00Z".into()),
                },
                observed_at: "2026-11-21T19:05:01Z".into(),
                declared_exit_code: Some(0),
            },
        )
    }
    fn settle(&self) -> anyhow::Result<KubernetesSessionEffectStateV2> {
        self.f.store.settle_kubernetes_session(
            &self.f.identity,
            &KubernetesSessionSettlementV2 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                reservation_id: self.intent.reservation_id().into(),
                reservation_input_hash: self.intent.input_hash("tenant-a").unwrap(),
                expected_last_step: self.step,
            },
        )
    }
}

#[test]
fn the_ledger_stays_closed_until_its_capability_marker_exists() {
    let f = Fixture::new();
    prepare(&f);
    let intent = reserve(&f, &QuotaPolicyV1::default());
    let mut chain = Chain::new(&f, intent);
    let request = chain.claim_request();
    assert_eq!(
        failure(chain.claim(&request).unwrap_err()),
        Failure::MigrationRequired
    );
    let head = f.store.head().unwrap();
    let report = enable(&f);
    assert_eq!(report.from_version, report.to_version, "no format change");
    assert_eq!(f.store.head().unwrap(), head + 1, "one genesis marker");
    assert_eq!(report.genesis_seq, head + 1);
    // A second enable is refused and appends nothing.
    let backup = f._directory.path().join("effects-again.sqlite");
    assert!(
        f.store
            .enable_kubernetes_session_effects_v1(&backup)
            .is_err()
    );
    assert_eq!(f.store.head().unwrap(), head + 1);
}

#[test]
fn a_claim_is_granted_once_and_every_replay_observes_its_receipt() {
    let f = Fixture::new();
    prepare(&f);
    enable(&f);
    let mut chain = Chain::new(&f, reserve(&f, &QuotaPolicyV1::default()));
    let request = chain.claim_request();
    let head = f.store.head().unwrap();
    let first = chain.claim(&request).unwrap();
    assert_eq!(
        first.disposition,
        KubernetesSessionEffectDispositionV2::EmitOnce
    );
    assert!(!first.replayed);
    assert_eq!(first.receipt.audit_seq, head + 1);
    let head = f.store.head().unwrap();
    let replay = chain.claim(&request).unwrap();
    assert_eq!(
        replay.disposition,
        KubernetesSessionEffectDispositionV2::ObserveOnly
    );
    assert!(replay.replayed);
    assert_eq!(replay.receipt, first.receipt);
    assert_eq!(f.store.head().unwrap(), head, "a replay appends nothing");
    // The same step with another request body is a conflict, not a replay.
    let mut divergent = request.clone();
    divergent.storage = Some(KubernetesSessionStorageRequestV2 {
        capacity_mib: 2048,
        ..storage()
    });
    divergent.operation_id = Uuid::new_v4().to_string();
    assert!(chain.claim(&divergent).is_err());
}

#[test]
fn the_store_rederives_digests_and_applies_the_tenant_storage_policy() {
    let f = Fixture::new();
    prepare(&f);
    enable(&f);
    let chain = Chain::new(&f, reserve(&f, &QuotaPolicyV1::default()));
    let mut forged = chain.claim_request();
    forged.request_sha256 = "e".repeat(64);
    assert_eq!(
        failure(
            f.store
                .claim_kubernetes_session_effect(&f.identity, &forged, Some(&storage_policy()), 30)
                .unwrap_err()
        ),
        Failure::Conflict
    );
    let request = chain.claim_request();
    assert_eq!(
        failure(
            f.store
                .claim_kubernetes_session_effect(&f.identity, &request, None, 30)
                .unwrap_err()
        ),
        Failure::PermissionDenied
    );
    let narrow = KubernetesSessionStoragePolicyV2 {
        max_capacity_mib: 512,
        ..storage_policy()
    };
    assert_eq!(
        failure(
            f.store
                .claim_kubernetes_session_effect(&f.identity, &request, Some(&narrow), 30)
                .unwrap_err()
        ),
        Failure::PermissionDenied
    );
}

#[test]
fn the_chain_is_ordered_and_a_claim_uid_is_never_reused() {
    let f = Fixture::new();
    prepare(&f);
    enable(&f);
    let quota = QuotaPolicyV1::default();
    let mut chain = Chain::new(&f, reserve(&f, &quota));
    // A Pod before its workspace claim is refused.
    let early = chain.pod_create();
    assert!(chain.claim(&early).is_err());
    let claim = chain.claim(&chain.claim_request()).unwrap();
    // A pending result blocks the next step.
    assert_eq!(
        failure(chain.claim(&chain.pod_create()).unwrap_err()),
        Failure::Conflict
    );
    let state = chain
        .result(
            &claim,
            KubernetesSessionEffectOutcomeV2::Applied {
                object_uid: CLAIM_UID.into(),
                resource_version: "7".into(),
            },
        )
        .unwrap();
    assert_eq!(state.workspace_claim_uid.as_deref(), Some(CLAIM_UID));
    // Attach before release, and release of another UID, are refused.
    chain.applied(chain.pod_create(), POD_UID);
    assert!(chain.claim(&chain.attach(POD_UID)).is_err());
    assert!(chain.claim(&chain.release(CLAIM_UID)).is_err());
    chain.applied(chain.release(POD_UID), POD_UID);
    chain.applied(chain.attach(POD_UID), POD_UID);
    // stdinOnce: a second attach is never granted.
    assert!(chain.claim(&chain.attach(POD_UID)).is_err());

    // Another reservation cannot bind the same claim UID (Q7).
    let mut other = Chain::new(&f, reserve(&f, &quota));
    let other_claim = other.claim(&other.claim_request()).unwrap();
    assert_eq!(
        failure(
            other
                .result(
                    &other_claim,
                    KubernetesSessionEffectOutcomeV2::Applied {
                        object_uid: CLAIM_UID.into(),
                        resource_version: "8".into(),
                    },
                )
                .unwrap_err()
        ),
        Failure::Conflict
    );
}

#[test]
fn quota_is_released_only_by_a_settlement_after_every_observed_end() {
    let f = Fixture::new();
    prepare(&f);
    enable(&f);
    let quota = QuotaPolicyV1 {
        max_active_workers: 1,
        ..QuotaPolicyV1::default()
    };
    let mut chain = Chain::new(&f, reserve(&f, &quota));
    let held = || f.store.worker_quota(&quota).unwrap().held_workers;
    assert_eq!(held(), 1);
    chain.applied(chain.claim_request(), CLAIM_UID);
    chain.applied(chain.pod_create(), POD_UID);
    chain.applied(chain.release(POD_UID), POD_UID);
    // Settlement before the Pod's observed end is refused; quota stays held.
    assert_eq!(failure(chain.settle().unwrap_err()), Failure::Conflict);
    assert_eq!(held(), 1);
    // A finalizer is only released after the terminal.
    assert!(chain.claim(&chain.finalizer(POD_UID)).is_err());
    let state = chain.terminal(POD_UID).unwrap();
    assert!(state.session_terminal);
    chain.applied(chain.finalizer(POD_UID), POD_UID);
    // The claim is only deleted once nothing can write to it any more: the
    // session ended and no export Pod was created here.
    assert!(chain.claim(&chain.claim_delete(CLAIM_UID)).is_err());
    let state = chain.settle().unwrap();
    assert!(state.settled);
    assert_eq!(held(), 0);
    // After settlement only cleanup is admitted.
    assert!(chain.claim(&chain.pod_create()).is_err());
    // The freed quota admits another reservation.
    reserve(&f, &quota);
    assert_eq!(held(), 1);
}

#[test]
fn an_uncertain_create_keeps_the_quota_held() {
    let f = Fixture::new();
    prepare(&f);
    enable(&f);
    let quota = QuotaPolicyV1::default();
    let mut chain = Chain::new(&f, reserve(&f, &quota));
    chain.applied(chain.claim_request(), CLAIM_UID);
    let create = chain.claim(&chain.pod_create()).unwrap();
    let state = chain
        .result(
            &create,
            KubernetesSessionEffectOutcomeV2::Uncertain { http_status: None },
        )
        .unwrap();
    assert_eq!(state.unresolved_uncertain_steps, 1);
    assert_eq!(failure(chain.settle().unwrap_err()), Failure::Conflict);
    assert_eq!(f.store.worker_quota(&quota).unwrap().held_workers, 1);
}

#[test]
fn a_reopened_store_revalidates_the_capability_family() {
    let f = Fixture::new();
    prepare(&f);
    enable(&f);
    let mut chain = Chain::new(&f, reserve(&f, &QuotaPolicyV1::default()));
    chain.applied(chain.claim_request(), CLAIM_UID);
    let path = f._directory.path().join("tenant.sqlite");
    let reopened = Store::open_existing_for_tenant(
        &path,
        "tenant-a",
        crate::redaction::Redactor::new(vec![]),
        10_000,
    );
    assert!(reopened.is_ok(), "{:?}", reopened.err());
}
