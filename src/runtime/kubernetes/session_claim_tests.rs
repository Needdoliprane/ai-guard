//! Workspace claim CREATE/DELETE over a real reservation intent, through the
//! real HTTPS client and one owned raw TLS loopback peer. The peer's PVC
//! bodies are declared fixtures: they never qualify an API server, a CSI
//! provisioner, `Immediate` binding or `ReadWriteOncePod` enforcement.
use super::*;
use crate::{
    control_fs::PrivateFile,
    runtime::kubernetes::{
        KubernetesCredentials,
        session::tests::fixture_at,
        session_effect::{LoopbackEffectGuardV2, SessionEffectKindV2},
        session_pod::tests::rendered_case_with,
        session_test_peer::{PeerReply, PeerRequest, SessionTestPeer},
    },
};
use std::os::unix::fs::PermissionsExt;

const TOKEN: &str = "public-session-claim-loopback-fixture-token-0000000000000";
const CLAIM_UID: &str = "55555555-5555-4555-8555-555555555555";
const CREATE: &str = "/api/v1/namespaces/tenant-a/persistentvolumeclaims?fieldValidation=Strict";

fn storage() -> KubernetesSessionStorageRequestV2 {
    KubernetesSessionStorageRequestV2 {
        version: 1,
        storage_class_name: "ai-guard-rwop".into(),
        capacity_mib: 1024,
    }
}

struct Case {
    peer: SessionTestPeer,
    intent: KubernetesSessionReservationIntentV2,
    runtime: KubernetesRuntime,
    request: KubernetesSessionClaimRequestV2,
    _rendered: crate::runtime::kubernetes::session_pod::tests::Rendered,
}
impl Case {
    async fn new() -> Self {
        let peer = SessionTestPeer::start().await;
        let ca = PrivateFile::open(peer.ca(), 64 * 1024).unwrap();
        let trust = ca.digest().to_owned();
        let token = peer.ca().parent().unwrap().join("session-claim-token");
        std::fs::write(&token, TOKEN).unwrap();
        std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600)).unwrap();
        let rendered = rendered_case_with(fixture_at(peer.endpoint(), &trust));
        let intent = rendered.intent.clone();
        let runtime = KubernetesRuntime::connect(
            intent.authority().clone(),
            KubernetesCredentials::bearer(ca, PrivateFile::open(&token, 8192).unwrap()),
        )
        .unwrap();
        let request = KubernetesSessionClaimRequestV2::render(&intent, &storage()).unwrap();
        Self {
            peer,
            intent,
            runtime,
            request,
            _rendered: rendered,
        }
    }
    fn guard(&self, kind: SessionEffectKindV2, digest: &str) -> LoopbackEffectGuardV2 {
        LoopbackEffectGuardV2::new(kind, digest, self.peer.listener()).unwrap()
    }
    fn create_guard(&self) -> LoopbackEffectGuardV2 {
        self.guard(
            SessionEffectKindV2::CreateClaim,
            self.request.target().request_sha256(),
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
    async fn create(
        &self,
        guard: LoopbackEffectGuardV2,
        cancelled: bool,
    ) -> Result<SessionClaimCreateOutcomeV2, KubernetesSessionRuntimeError> {
        self.runtime
            .create_session_workspace_claim(&self.request, guard, &AtomicBool::new(cancelled))
            .await
    }
}

fn namespace_reply(name: &str, uid: &str) -> PeerReply {
    PeerReply::json(
        200,
        &json!({"apiVersion":"v1","kind":"Namespace","metadata":{"name":name,"uid":uid},
            "status":{"phase":"Active"}}),
    )
}
/// Echo of the received body with the server facts a 201 adds.
fn created(body: &[u8]) -> Value {
    let mut claim: Value = serde_json::from_slice(body).unwrap();
    claim["metadata"]["uid"] = json!(CLAIM_UID);
    claim["metadata"]["resourceVersion"] = json!("7");
    claim["metadata"]["creationTimestamp"] = json!("2026-11-21T19:00:00Z");
    claim["metadata"]["finalizers"] = json!(["kubernetes.io/pvc-protection"]);
    claim["status"] = json!({"phase":"Pending"});
    claim
}

#[test]
fn render_is_the_exact_rwop_filesystem_claim_for_the_attempt() {
    let rendered = crate::runtime::kubernetes::session_pod::tests::rendered_case();
    let request = KubernetesSessionClaimRequestV2::render(&rendered.intent, &storage()).unwrap();
    let ids = rendered.intent.ids().unwrap();
    let claim_name = format!("ai-guard-ws-{}", ids.attempt_id.simple());
    // Written out by hand: canonical key order, no status, no volumeName,
    // no dataSource, no annotation.
    let expected = format!(
        "{{\"apiVersion\":\"v1\",\"kind\":\"PersistentVolumeClaim\",\"metadata\":{{\
         \"labels\":{{\"ai-guard/attempt\":\"{attempt}\",\"ai-guard/purpose\":\"workspace-v2\",\
         \"ai-guard/session\":\"{session}\",\"app.kubernetes.io/managed-by\":\"ai-guard\"}},\
         \"name\":\"{claim_name}\",\"namespace\":\"tenant-a\"}},\"spec\":{{\
         \"accessModes\":[\"ReadWriteOncePod\"],\"resources\":{{\"requests\":{{\"storage\":\"1024Mi\"}}}},\
         \"storageClassName\":\"ai-guard-rwop\",\"volumeMode\":\"Filesystem\"}}}}",
        attempt = ids.attempt_id,
        session = ids.session_id,
    );
    assert_eq!(
        String::from_utf8(request.bytes().to_vec()).unwrap(),
        expected
    );
    assert_eq!(request.target().claim_name(), claim_name);
    // Deterministic: the central ledger re-renders the same digest.
    let again = KubernetesSessionClaimRequestV2::render(&rendered.intent, &storage()).unwrap();
    assert_eq!(again, request);
    // Another class or capacity is another request.
    let mut bigger = storage();
    bigger.capacity_mib = 2048;
    assert_ne!(
        KubernetesSessionClaimRequestV2::render(&rendered.intent, &bigger)
            .unwrap()
            .target()
            .request_sha256(),
        request.target().request_sha256()
    );
}

#[test]
fn render_refuses_an_invalid_storage_request() {
    let rendered = crate::runtime::kubernetes::session_pod::tests::rendered_case();
    for storage in [
        KubernetesSessionStorageRequestV2 {
            version: 2,
            ..storage()
        },
        KubernetesSessionStorageRequestV2 {
            capacity_mib: 0,
            ..storage()
        },
        KubernetesSessionStorageRequestV2 {
            capacity_mib: MAX_CAPACITY_MIB + 1,
            ..storage()
        },
        KubernetesSessionStorageRequestV2 {
            storage_class_name: "Not_A_Class".into(),
            ..storage()
        },
        KubernetesSessionStorageRequestV2 {
            storage_class_name: String::new(),
            ..storage()
        },
    ] {
        assert_eq!(
            KubernetesSessionClaimRequestV2::render(&rendered.intent, &storage),
            Err(KubernetesError::InvalidPreflightPlan)
        );
    }
}

#[tokio::test]
async fn create_posts_the_exact_claim_once_and_a_validated_201_yields_its_uid() {
    let case = Case::new().await;
    case.respond(|request: &PeerRequest| {
        if request.method() == "POST" && request.target() == CREATE {
            PeerReply::json(201, &created(request.body()))
        } else {
            PeerReply::json(404, &json!({"kind":"Status","code":404}))
        }
    });
    let outcome = case.create(case.create_guard(), false).await.unwrap();
    let SessionClaimCreateOutcomeV2::Created(created) = outcome else {
        panic!("a validated 201 must yield the claim: {outcome:?}");
    };
    assert_eq!(created.uid(), CLAIM_UID);
    assert_eq!(created.resource_version(), "7");
    assert_eq!(created.claim_name(), case.request.target().claim_name());
    // The later bound observation must be this very object.
    let bound = |uid: &str| {
        KubernetesSessionWorkspaceClaimV2::retained_from_value(json!({
            "version":1,"authority_fingerprint":case.intent.authority().fingerprint(),
            "namespace":"tenant-a","name":case.request.target().claim_name(),"uid":uid,
            "resource_version":"9","storage_class_name":"ai-guard-rwop",
            "volume_name":"pvc-55555555-5555-4555-8555-555555555555","capacity":"1Gi",
            "access_mode":"ReadWriteOncePod","observed_at":"2026-11-21T19:00:01.000Z"}))
        .unwrap()
    };
    assert!(created.is_observed_as(&bound(CLAIM_UID)));
    assert!(!created.is_observed_as(&bound("66666666-6666-4666-8666-666666666666")));
    let effects = case.effects();
    assert_eq!(effects.len(), 1);
    assert_eq!(effects[0].line(), format!("POST {CREATE}"));
    assert_eq!(effects[0].body(), case.request.bytes());
    assert_eq!(effects[0].header("content-type"), Some("application/json"));
}

#[tokio::test]
async fn every_unproven_answer_keeps_ownership_unknown_without_a_second_post() {
    let cases: Vec<(&str, Box<dyn Fn(&PeerRequest) -> PeerReply + Send>)> = vec![
        (
            "already exists",
            Box::new(|_: &PeerRequest| PeerReply::json(409, &json!({"kind":"Status","code":409}))),
        ),
        (
            "lost answer",
            Box::new(|_: &PeerRequest| PeerReply::CloseWithoutResponse),
        ),
        (
            "another storage class",
            Box::new(|request: &PeerRequest| {
                let mut claim = created(request.body());
                claim["spec"]["storageClassName"] = json!("standard");
                PeerReply::json(201, &claim)
            }),
        ),
        (
            "a data source",
            Box::new(|request: &PeerRequest| {
                let mut claim = created(request.body());
                claim["spec"]["dataSource"] = json!({"kind":"PersistentVolumeClaim","name":"x"});
                PeerReply::json(201, &claim)
            }),
        ),
        (
            "a foreign finalizer",
            Box::new(|request: &PeerRequest| {
                let mut claim = created(request.body());
                claim["metadata"]["finalizers"] = json!(["example.com/hold"]);
                PeerReply::json(201, &claim)
            }),
        ),
        (
            "an already bound status",
            Box::new(|request: &PeerRequest| {
                let mut claim = created(request.body());
                claim["status"] = json!({"phase":"Bound"});
                PeerReply::json(201, &claim)
            }),
        ),
        (
            "an invalid uid",
            Box::new(|request: &PeerRequest| {
                let mut claim = created(request.body());
                claim["metadata"]["uid"] = json!("not-a-uid");
                PeerReply::json(201, &claim)
            }),
        ),
    ];
    for (label, reply) in cases {
        let case = Case::new().await;
        case.respond(move |request: &PeerRequest| reply(request));
        let outcome = case.create(case.create_guard(), false).await.unwrap();
        let SessionClaimCreateOutcomeV2::OwnershipUnknown { http_status, cause } = outcome else {
            panic!("{label}: {outcome:?}");
        };
        // A lost answer has no status; every received answer keeps its own.
        assert_eq!(http_status.is_none(), label == "lost answer", "{label}");
        assert_ne!(
            cause,
            KubernetesSessionRuntimeError::EffectRefusedBeforeSend,
            "{label}"
        );
        assert_eq!(case.effects().len(), 1, "{label}: exactly one POST");
    }
}

#[tokio::test]
async fn a_wrong_guard_or_a_cancellation_refuses_before_any_effect() {
    let case = Case::new().await;
    case.respond(|request: &PeerRequest| PeerReply::json(201, &created(request.body())));
    let wrong_kind = case.guard(
        SessionEffectKindV2::CreateHeld,
        case.request.target().request_sha256(),
    );
    assert_eq!(
        case.create(wrong_kind, false).await.unwrap_err(),
        KubernetesSessionRuntimeError::EffectRefusedBeforeSend
    );
    let wrong_digest = case.guard(SessionEffectKindV2::CreateClaim, &"0".repeat(64));
    assert_eq!(
        case.create(wrong_digest, false).await.unwrap_err(),
        KubernetesSessionRuntimeError::EffectRefusedBeforeSend
    );
    assert_eq!(
        case.create(case.create_guard(), true).await.unwrap_err(),
        KubernetesSessionRuntimeError::EffectRefusedBeforeSend
    );
    assert!(case.effects().is_empty());
}

#[tokio::test]
async fn delete_sends_the_uid_precondition_and_classifies_answers() {
    let case = Case::new().await;
    let claim = KubernetesSessionWorkspaceClaimV2::retained_from_value(json!({
        "version":1,
        "authority_fingerprint":case.intent.authority().fingerprint(),
        "namespace":"tenant-a",
        "name":case.request.target().claim_name(),
        "uid":CLAIM_UID,
        "resource_version":"9",
        "storage_class_name":"ai-guard-rwop",
        "volume_name":"pvc-55555555-5555-4555-8555-555555555555",
        "capacity":"1Gi",
        "access_mode":"ReadWriteOncePod",
        "observed_at":"2026-11-21T19:00:01.000Z"
    }))
    .unwrap();
    let path = format!(
        "/api/v1/namespaces/tenant-a/persistentvolumeclaims/{}",
        case.request.target().claim_name()
    );
    // Written out by hand: canonical DeleteOptions with only the UID.
    let body = format!(
        "{{\"apiVersion\":\"v1\",\"kind\":\"DeleteOptions\",\"preconditions\":{{\"uid\":\"{CLAIM_UID}\"}},\
         \"propagationPolicy\":\"Background\"}}"
    );
    let digest = crate::runtime::kubernetes::session_effect::effect_request_sha256(
        SessionEffectKindV2::DeleteClaimExact,
        &case.intent.authority().fingerprint(),
        "DELETE",
        &path,
        body.as_bytes(),
    );
    for (status, expected) in [
        (200, SessionDeleteOutcomeV2::DeletionRequested),
        (404, SessionDeleteOutcomeV2::NotFound),
        (409, SessionDeleteOutcomeV2::PreconditionFailed),
    ] {
        case.respond(move |_: &PeerRequest| {
            PeerReply::json(status, &json!({"kind":"Status","code":status}))
        });
        let before = case.effects().len();
        let outcome = case
            .runtime
            .delete_session_workspace_claim_exact(
                &claim,
                case.guard(SessionEffectKindV2::DeleteClaimExact, &digest),
                &AtomicBool::new(false),
            )
            .await
            .unwrap();
        assert_eq!(outcome, expected, "status {status}");
        let effects = case.effects();
        assert_eq!(effects.len(), before + 1);
        assert_eq!(effects[before].line(), format!("DELETE {path}"));
        assert_eq!(effects[before].body(), body.as_bytes());
    }
}
