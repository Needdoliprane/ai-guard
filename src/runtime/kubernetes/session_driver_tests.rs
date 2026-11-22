//! The driver at its seam: the real tenant Store as the central ledger, the
//! real HTTPS client, and one owned raw TLS loopback peer standing in for the
//! API server. The peer's PVC bodies are declared fixtures: they never
//! qualify an API server, a CSI provisioner or `Immediate` binding.
use super::*;
use crate::{
    audit::kubernetes_admission_state_tests::Fixture,
    control::kubernetes_session::effects::{
        KubernetesSessionEffectPolicyV2, KubernetesSessionStoragePolicyV2,
    },
    control_fs::PrivateFile,
    runtime::kubernetes::{
        KubernetesCredentials,
        session::tests::fixture_at,
        session_claim::tests::{CLAIM_UID, CREATE, TOKEN, created, namespace_reply, storage},
        session_instance::tests::bound_pvc,
        session_pod::session_workspace_claim_name,
        session_test_peer::{PeerReply, PeerRequest, SessionTestPeer},
    },
};
use serde_json::{Value, json};
use std::{
    os::unix::fs::PermissionsExt,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

/// The tenant Store behind the same seam as the broker's HTTP client.
struct StoreLedger {
    fixture: Fixture,
    policy: KubernetesSessionEffectPolicyV2,
}
fn refused(error: anyhow::Error) -> SessionDriverErrorV2 {
    SessionDriverErrorV2::Ledger(format!("{error:#}"))
}
impl SessionLedgerV2 for StoreLedger {
    async fn claim(
        &self,
        request: &KubernetesSessionEffectRequestV2,
    ) -> Driven<KubernetesSessionEffectClaimAckV2> {
        self.fixture
            .store
            .claim_kubernetes_session_effect(&self.fixture.identity, request, Some(&self.policy))
            .map_err(refused)
    }
    async fn result(
        &self,
        result: &KubernetesSessionEffectResultV2,
    ) -> Driven<KubernetesSessionEffectStateV2> {
        self.fixture
            .store
            .record_kubernetes_session_effect_result(&self.fixture.identity, result)
            .map_err(refused)
    }
    async fn terminal(
        &self,
        terminal: &KubernetesSessionTerminalV2,
    ) -> Driven<KubernetesSessionEffectStateV2> {
        self.fixture
            .store
            .record_kubernetes_session_terminal(&self.fixture.identity, terminal)
            .map_err(refused)
    }
    async fn settle(
        &self,
        settlement: &KubernetesSessionSettlementV2,
    ) -> Driven<KubernetesSessionEffectStateV2> {
        self.fixture
            .store
            .settle_kubernetes_session(&self.fixture.identity, settlement)
            .map_err(refused)
    }
    async fn state(&self, reservation_id: &str) -> Driven<KubernetesSessionEffectStateV2> {
        self.fixture
            .store
            .kubernetes_session_effect_state(&self.fixture.identity, reservation_id)
            .map_err(refused)
    }
}

struct Case {
    peer: SessionTestPeer,
    runtime: KubernetesRuntime,
    intent: KubernetesSessionReservationIntentV2,
    ledger: StoreLedger,
}
impl Case {
    /// Real reservation, effect capability enabled with its backup, and an
    /// effect policy activating the peer's authority for `allowed_class`.
    async fn new(allowed_class: &str) -> Self {
        let peer = SessionTestPeer::start().await;
        let ca = PrivateFile::open(peer.ca(), 64 * 1024).unwrap();
        let trust = ca.digest().to_owned();
        let token = peer.ca().parent().unwrap().join("session-driver-token");
        std::fs::write(&token, TOKEN).unwrap();
        std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600)).unwrap();
        let (fixture, intent, _confirmed) = fixture_at(peer.endpoint(), &trust);
        fixture
            .store
            .enable_kubernetes_session_effects_v1(
                &fixture._directory.path().join("before-effects.sqlite"),
            )
            .unwrap();
        let runtime = KubernetesRuntime::connect(
            intent.authority().clone(),
            KubernetesCredentials::bearer(ca, PrivateFile::open(&token, 8192).unwrap()),
        )
        .unwrap();
        let policy = KubernetesSessionEffectPolicyV2 {
            version: 1,
            claim_window_seconds: 30,
            storage: KubernetesSessionStoragePolicyV2 {
                version: 1,
                allowed_storage_classes: vec![allowed_class.into()],
                max_capacity_mib: 4096,
            },
            authorities: vec![intent.authority().clone()],
        };
        Self {
            peer,
            runtime,
            intent,
            ledger: StoreLedger { fixture, policy },
        }
    }
    fn claim_path(&self) -> String {
        format!(
            "/api/v1/namespaces/tenant-a/persistentvolumeclaims/{}",
            session_workspace_claim_name(self.intent.ids().unwrap().attempt_id)
        )
    }
    /// Active anchors, `effect` for every other request.
    fn respond(&self, mut effect: impl FnMut(&PeerRequest) -> PeerReply + Send + 'static) {
        let authority = self.intent.authority().clone();
        self.peer.respond(move |request: &PeerRequest| {
            match (request.method(), request.target()) {
                ("GET", "/api/v1/namespaces/kube-system") => {
                    namespace_reply("kube-system", &authority.cluster_namespace_uid)
                }
                ("GET", "/api/v1/namespaces/tenant-a") => {
                    namespace_reply("tenant-a", &authority.namespace_uid)
                }
                _ => effect(request),
            }
        });
    }
    fn effects(&self) -> Vec<PeerRequest> {
        self.peer
            .requests()
            .into_iter()
            .filter(|request| request.method() != "GET")
            .collect()
    }
    fn state(&self) -> KubernetesSessionEffectStateV2 {
        self.ledger
            .fixture
            .store
            .kubernetes_session_effect_state(
                &self.ledger.fixture.identity,
                self.intent.reservation_id(),
            )
            .unwrap()
    }
    async fn driver(&self) -> SessionDriverV2<'_, StoreLedger> {
        SessionDriverV2::open(
            &self.runtime,
            &self.ledger,
            self.intent.clone(),
            "tenant-a",
            SessionDriverWaitsV2 {
                bind: Duration::from_secs(30),
                poll: Duration::from_millis(20),
            },
        )
        .await
        .unwrap()
    }
}

fn pending_pvc(name: &str) -> Value {
    let mut pvc = bound_pvc("tenant-a", name, CLAIM_UID);
    pvc["status"] = json!({"phase":"Pending"});
    pvc["spec"]
        .as_object_mut()
        .unwrap()
        .remove("volumeName")
        .unwrap();
    pvc
}

#[tokio::test]
async fn a_workspace_is_claimed_created_once_and_bound_by_its_exact_uid() {
    let case = Case::new("ai-guard-rwop").await;
    let path = case.claim_path();
    let name = path.rsplit('/').next().unwrap().to_owned();
    let reads = Arc::new(AtomicUsize::new(0));
    let seen = reads.clone();
    case.respond(move |request: &PeerRequest| {
        match (request.method(), request.target()) {
            ("POST", CREATE) => PeerReply::json(201, &created(request.body())),
            ("GET", target) if target == path => {
                // Pending once, then bound: the driver waits, never re-sends.
                if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                    PeerReply::json(200, &pending_pvc(&name))
                } else {
                    PeerReply::json(200, &bound_pvc("tenant-a", &name, CLAIM_UID))
                }
            }
            _ => PeerReply::json(404, &json!({"kind":"Status","code":404})),
        }
    });
    assert_eq!(case.state().last_step, 0);
    let mut driver = case.driver().await;
    let claim = driver.provision_workspace(&storage()).await.unwrap();
    assert_eq!(claim.uid(), CLAIM_UID);
    assert!(reads.load(Ordering::SeqCst) >= 2);
    let effects = case.effects();
    assert_eq!(effects.len(), 1, "exactly one POST");
    assert_eq!(effects[0].line(), format!("POST {CREATE}"));
    let state = case.state();
    assert_eq!(state.last_step, 1);
    assert_eq!(state.workspace_claim_uid.as_deref(), Some(CLAIM_UID));
    assert_eq!(state.unresolved_uncertain_steps, 0);
    assert!(!state.settled);
}

#[tokio::test]
async fn an_unproven_create_is_recorded_uncertain_and_never_resent() {
    let case = Case::new("ai-guard-rwop").await;
    case.respond(|_: &PeerRequest| PeerReply::CloseWithoutResponse);
    let mut driver = case.driver().await;
    let error = driver.provision_workspace(&storage()).await.unwrap_err();
    assert_eq!(error, SessionDriverErrorV2::Uncertain { step: 1 });
    assert_eq!(case.effects().len(), 1, "exactly one POST");
    let state = case.state();
    assert_eq!(state.last_step, 1);
    assert_eq!(state.workspace_claim_uid, None);
    assert_eq!(state.unresolved_uncertain_steps, 1);
    // The quota stays held: settlement is refused while ownership is unknown.
    assert!(matches!(
        driver.settle().await,
        Err(SessionDriverErrorV2::Ledger(_))
    ));
    assert!(!case.state().settled);
}

#[tokio::test]
async fn a_ledger_refusal_sends_nothing() {
    // The effect policy approves another storage class only.
    let case = Case::new("standard").await;
    case.respond(|request: &PeerRequest| PeerReply::json(201, &created(request.body())));
    let mut driver = case.driver().await;
    let error = driver.provision_workspace(&storage()).await.unwrap_err();
    assert!(
        matches!(error, SessionDriverErrorV2::Ledger(_)),
        "{error:?}"
    );
    assert!(case.effects().is_empty(), "no request without a claim");
    assert_eq!(case.state().last_step, 0);
}
