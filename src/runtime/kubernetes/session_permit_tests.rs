//! The production permit at its two seams: the synchronous guard check and
//! one real transport effect (workspace claim CREATE) against the owned TLS
//! loopback peer. The peer's answers are declared fixtures.
use super::*;
use crate::{
    control::kubernetes_session::effects::{
        KubernetesSessionEffectReceiptV2, KubernetesSessionEffectRequestV2,
    },
    control_fs::PrivateFile,
    runtime::kubernetes::{
        KubernetesCredentials, KubernetesRuntime,
        session::tests::fixture_at,
        session_claim::{
            KubernetesSessionClaimRequestV2, KubernetesSessionStorageRequestV2,
            SessionClaimCreateOutcomeV2,
        },
        session_pod::tests::{rendered_case, rendered_case_with},
        session_test_peer::{PeerReply, PeerRequest, SessionTestPeer},
    },
};
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;

const OPERATION: &str = "66666666-6666-4666-8666-666666666666";

fn storage() -> KubernetesSessionStorageRequestV2 {
    KubernetesSessionStorageRequestV2 {
        version: 1,
        storage_class_name: "ai-guard-rwop".into(),
        capacity_mib: 1024,
    }
}
/// A central acknowledgement of the claim CREATE `digest` of `reservation`.
fn ack(
    reservation_id: &str,
    reservation_input_hash: &str,
    digest: &str,
    disposition: KubernetesSessionEffectDispositionV2,
    replayed: bool,
    not_after: &str,
) -> KubernetesSessionEffectClaimAckV2 {
    let request = KubernetesSessionEffectRequestV2 {
        version: 1,
        operation_id: OPERATION.into(),
        reservation_id: reservation_id.into(),
        reservation_input_hash: reservation_input_hash.into(),
        step: 1,
        subject: Subject::WorkspaceClaim,
        action: Action::Create,
        request_sha256: digest.into(),
        object_uid: None,
        object_resource_version: None,
        storage: Some(storage()),
        deadline: None,
    };
    KubernetesSessionEffectClaimAckV2 {
        version: 1,
        receipt: KubernetesSessionEffectReceiptV2 {
            version: 1,
            operation_id: OPERATION.into(),
            input_hash: request.input_hash().unwrap(),
            control_namespace_uuid: "77777777-7777-4777-8777-777777777777".into(),
            audit_seq: 3,
            audit_hash: "d".repeat(64),
        },
        request,
        disposition,
        issued_at: "2026-01-01T00:00:00.000Z".into(),
        not_after: not_after.into(),
        replayed,
    }
}
const FUTURE: &str = "2099-01-01T00:00:00.000Z";

#[test]
fn only_a_fresh_emit_once_acknowledgement_can_be_retained() {
    let rendered = rendered_case();
    let authority = rendered.intent.authority();
    let digest = "b".repeat(64);
    let input = "a".repeat(64);
    let id = rendered.intent.reservation_id();
    use KubernetesSessionEffectDispositionV2 as D;
    assert!(
        RetainedEffectClaimV2::retained(
            ack(id, &input, &digest, D::EmitOnce, false, FUTURE),
            authority
        )
        .is_ok()
    );
    for (disposition, replayed) in [(D::ObserveOnly, false), (D::ObserveOnly, true)] {
        assert_eq!(
            RetainedEffectClaimV2::retained(
                ack(id, &input, &digest, disposition, replayed, FUTURE),
                authority
            )
            .unwrap_err(),
            REFUSED
        );
    }
}

#[test]
fn the_permit_refuses_every_other_effect_digest_authority_window_or_cancellation() {
    let rendered = rendered_case();
    let authority = rendered.intent.authority().clone();
    let digest = "b".repeat(64);
    let input = "a".repeat(64);
    let id = rendered.intent.reservation_id().to_owned();
    let permit = |not_after: &str| {
        CentralEffectPermitV2::from_retained(
            RetainedEffectClaimV2::retained(
                ack(
                    &id,
                    &input,
                    &digest,
                    KubernetesSessionEffectDispositionV2::EmitOnce,
                    false,
                    not_after,
                ),
                &authority,
            )
            .unwrap(),
        )
        .unwrap()
    };
    let idle = AtomicBool::new(false);
    assert_eq!(
        permit(FUTURE).check_before_send(
            SessionEffectKindV2::CreateClaim,
            &digest,
            &authority,
            &idle
        ),
        Ok(())
    );
    let mut other = authority.clone();
    other.namespace_uid = "88888888-8888-4888-8888-888888888888".into();
    assert_ne!(other.fingerprint(), authority.fingerprint());
    let other_digest = "c".repeat(64);
    for (kind, request, binding, cancelled, window) in [
        (
            SessionEffectKindV2::CreateHeld,
            digest.as_str(),
            &authority,
            false,
            FUTURE,
        ),
        (
            SessionEffectKindV2::CreateClaim,
            other_digest.as_str(),
            &authority,
            false,
            FUTURE,
        ),
        (
            SessionEffectKindV2::CreateClaim,
            digest.as_str(),
            &other,
            false,
            FUTURE,
        ),
        (
            SessionEffectKindV2::CreateClaim,
            digest.as_str(),
            &authority,
            true,
            FUTURE,
        ),
        (
            SessionEffectKindV2::CreateClaim,
            digest.as_str(),
            &authority,
            false,
            "2026-01-01T00:00:01.000Z",
        ),
    ] {
        assert_eq!(
            permit(window).check_before_send(kind, request, binding, &AtomicBool::new(cancelled)),
            Err(REFUSED),
            "{kind:?} {window}"
        );
    }
}

#[test]
fn each_claimed_subject_action_maps_to_exactly_one_transport_effect() {
    use SessionEffectKindV2 as K;
    for (subject, action, kind) in [
        (Subject::WorkspaceClaim, Action::Create, K::CreateClaim),
        (Subject::WorkspaceClaim, Action::Delete, K::DeleteClaimExact),
        (Subject::SessionPod, Action::Create, K::CreateHeld),
        (Subject::SessionPod, Action::ReleaseGate, K::ReleaseGate),
        (Subject::SessionPod, Action::Attach, K::Attach),
        (
            Subject::SessionPod,
            Action::ReleaseFinalizer,
            K::ReleaseFinalizer,
        ),
        (Subject::SessionPod, Action::Delete, K::DeleteExact),
        (Subject::ExportPod, Action::Create, K::CreateHeld),
        (Subject::ExportPod, Action::Delete, K::DeleteExact),
    ] {
        assert_eq!(effect_kind(subject, action), kind);
    }
}

#[tokio::test]
async fn a_production_permit_sends_one_claim_create_and_is_consumed() {
    let peer = SessionTestPeer::start().await;
    let ca = PrivateFile::open(peer.ca(), 64 * 1024).unwrap();
    let trust = ca.digest().to_owned();
    let token = peer.ca().parent().unwrap().join("session-permit-token");
    std::fs::write(
        &token,
        "public-session-permit-loopback-fixture-token-000000000000",
    )
    .unwrap();
    std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600)).unwrap();
    let rendered = rendered_case_with(fixture_at(peer.endpoint(), &trust));
    let intent = rendered.intent.clone();
    let authority = intent.authority().clone();
    let runtime = KubernetesRuntime::connect(
        authority.clone(),
        KubernetesCredentials::bearer(ca, PrivateFile::open(&token, 8192).unwrap()),
    )
    .unwrap();
    let request = KubernetesSessionClaimRequestV2::render(&intent, &storage()).unwrap();
    let (cluster, namespace) = (
        authority.cluster_namespace_uid.clone(),
        authority.namespace_uid.clone(),
    );
    peer.respond(
        move |seen: &PeerRequest| match (seen.method(), seen.target()) {
            ("GET", "/api/v1/namespaces/kube-system") => namespace_reply("kube-system", &cluster),
            ("GET", "/api/v1/namespaces/tenant-a") => namespace_reply("tenant-a", &namespace),
            ("POST", _) => {
                let mut claim: Value = serde_json::from_slice(seen.body()).unwrap();
                claim["metadata"]["uid"] = json!("99999999-9999-4999-8999-999999999999");
                claim["metadata"]["resourceVersion"] = json!("3");
                PeerReply::json(201, &claim)
            }
            _ => PeerReply::json(404, &json!({"kind":"Status","code":404})),
        },
    );
    let input = "a".repeat(64);
    let permit = CentralEffectPermitV2::from_retained(
        RetainedEffectClaimV2::retained(
            ack(
                intent.reservation_id(),
                &input,
                request.target().request_sha256(),
                KubernetesSessionEffectDispositionV2::EmitOnce,
                false,
                FUTURE,
            ),
            &authority,
        )
        .unwrap(),
    )
    .unwrap();
    let outcome = runtime
        .create_session_workspace_claim(&request, permit, &AtomicBool::new(false))
        .await
        .unwrap();
    assert!(
        matches!(outcome, SessionClaimCreateOutcomeV2::Created(_)),
        "{outcome:?}"
    );
    assert_eq!(
        peer.requests()
            .iter()
            .filter(|seen| seen.method() == "POST")
            .count(),
        1
    );
    // A permit for another request digest sends nothing.
    let foreign = CentralEffectPermitV2::from_retained(
        RetainedEffectClaimV2::retained(
            ack(
                intent.reservation_id(),
                &input,
                &"e".repeat(64),
                KubernetesSessionEffectDispositionV2::EmitOnce,
                false,
                FUTURE,
            ),
            &authority,
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        runtime
            .create_session_workspace_claim(&request, foreign, &AtomicBool::new(false))
            .await
            .unwrap_err(),
        REFUSED
    );
    assert_eq!(
        peer.requests()
            .iter()
            .filter(|seen| seen.method() == "POST")
            .count(),
        1
    );
}

fn namespace_reply(name: &str, uid: &str) -> PeerReply {
    PeerReply::json(
        200,
        &json!({"apiVersion":"v1","kind":"Namespace","metadata":{"name":name,"uid":uid},
            "status":{"phase":"Active"}}),
    )
}
