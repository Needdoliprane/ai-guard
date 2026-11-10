//! Real SQLite reservation/audit and private journal; no Pod or runtime effect.
use super::*;
use crate::{
    audit::kubernetes_admission_state_tests::Fixture,
    control::{QuotaPolicyV1, kubernetes_session::*},
    provider::{
        AuthMode, ChoiceSource, ClientKind, ClientTransport, ProviderChoiceProvenanceV1,
        ProviderResolutionV1,
    },
    runtime::kubernetes::session_profile::*,
};
use std::fs;
use uuid::Uuid;

pub(crate) fn fixture() -> (
    Fixture,
    KubernetesSessionReservationIntentV2,
    ConfirmedSessionReservationV2,
) {
    let f = Fixture::new();
    let path = f._directory.path().join("tenant.sqlite");
    f.store
        .migrate_kubernetes_policy_v9(&path.with_extension("before9"))
        .unwrap();
    f.store
        .migrate_human_workers_v10(&path.with_extension("before10"))
        .unwrap();
    f.store
        .migrate_kubernetes_session_reservations_v11(&path.with_extension("before11"))
        .unwrap();
    let binding = KubernetesSessionBindingV2::new(
        &f.intent.authority,
        KubernetesSessionIdsV2 {
            installation_id: Uuid::new_v4(),
            control_namespace_uuid: f.intent.control_namespace_uuid.parse().unwrap(),
            broker_id: f.intent.broker_id.parse().unwrap(),
            incarnation_id: f.intent.incarnation_id.parse().unwrap(),
            session_id: Uuid::new_v4(),
            attempt_id: Uuid::new_v4(),
        },
    )
    .unwrap();
    let provider = ProviderResolutionV1 {
        version: 1,
        client: ClientKind::Codex,
        auth_mode: AuthMode::Api,
        transport: ClientTransport::Batch,
        profile_id: "default".into(),
        profile_revision: 1,
        model: None,
        credential_binding_id: Uuid::new_v4(),
        credential_names: vec![crate::provider::BOOTSTRAP_ENV.into()],
        invocation_preset_version: 1,
        provenance: ProviderChoiceProvenanceV1 {
            client: ChoiceSource::Cli,
            auth_mode: ChoiceSource::Cli,
            profile: ChoiceSource::Cli,
            model: ChoiceSource::Cli,
        },
    };
    let args = [
        "exec",
        "--client",
        "codex",
        "--auth-mode",
        "api",
        "--transport",
        "batch",
        "--preset-version",
        "1",
        "--prompt",
        "public session fixture",
    ]
    .map(str::to_owned);
    let profile = KubernetesSessionProfileV2::build(
        &f.intent.authority,
        &binding,
        &f.intent.image,
        KubernetesSessionResourcesV2 {
            cpu_millis: 500,
            memory_mib: 128,
            temporary_mib: 8,
            home_mib: 8,
            deadline_seconds: 60,
        },
        &provider,
        KubernetesSessionInvocationV2::new(crate::provider::BOOTSTRAP_PROGRAM, &args).unwrap(),
        &crate::workspace::Manifest::new(),
    )
    .unwrap();
    let intent = KubernetesSessionReservationIntentV2::new(
        &Uuid::new_v4().to_string(),
        &Uuid::new_v4().to_string(),
        &f.intent.authority,
        &profile,
    )
    .unwrap();
    let policy = KubernetesSessionReservationPolicyV2 {
        version: 2,
        decision_ttl_seconds: 60,
        approvals: vec![KubernetesSessionReservationApprovalV2 {
            authority_id: intent.authority().authority_id().to_string(),
            authority_fingerprint: intent.authority().fingerprint(),
            image: f.intent.image.clone(),
            client: ClientKind::Codex,
            auth_mode: AuthMode::Api,
            transport: ClientTransport::Batch,
            profile_id: "default".into(),
            profile_revision: 1,
        }],
    };
    let quota = QuotaPolicyV1::default();
    let auth = f
        .store
        .authorize_kubernetes_session_reservation(&f.identity, &intent, &policy, &quota)
        .unwrap();
    f.store
        .reserve_kubernetes_session(&f.identity, &auth, &intent, &policy, &quota)
        .unwrap();
    let observe = f
        .store
        .authorize_kubernetes_session_reconciliation(&f.identity, intent.reservation_id(), None)
        .unwrap();
    let confirmed = f
        .store
        .confirm_kubernetes_session_before_effect(&f.identity, &observe, intent.reservation_id())
        .unwrap();
    (f, intent, confirmed)
}
fn root(f: &Fixture, name: &str) -> Directory {
    Directory::open_existing(f._directory.path())
        .unwrap()
        .persistent_child(name, true)
        .unwrap()
        .unwrap()
}
fn protection() -> SessionJournalProtection {
    SessionJournalProtection::new(Vec::new(), Vec::new()).unwrap()
}

#[test]
fn actual_reservation_reopens_the_same_preparation_without_new_central_event_or_authority() {
    let (f, intent, confirmed) = fixture();
    let root = root(&f, "session");
    let central_head = f.store.head().unwrap();
    let quota = f.store.worker_quota(&QuotaPolicyV1::default()).unwrap();
    let journal =
        SessionJournal::prepare_or_resume(root.clone(), protection(), &intent, &confirmed).unwrap();
    let binding = journal.bootstrap_binding().unwrap();
    let bytes = fs::read(root.path().join("prepared-v2.json")).unwrap();
    assert!(matches!(
        SessionJournal::prepare_or_resume(root.clone(), protection(), &intent, &confirmed),
        Err(SessionError::Busy)
    ));
    assert!(intent.authority().require_workload_admission().is_err());
    drop(journal);
    let observe = f
        .store
        .authorize_kubernetes_session_reconciliation(&f.identity, intent.reservation_id(), None)
        .unwrap();
    let confirmed = f
        .store
        .confirm_kubernetes_session_before_effect(&f.identity, &observe, intent.reservation_id())
        .unwrap();
    let reopened =
        SessionJournal::prepare_or_resume(root.clone(), protection(), &intent, &confirmed).unwrap();
    assert_eq!(binding, reopened.bootstrap_binding().unwrap());
    assert_eq!(
        bytes,
        fs::read(root.path().join("prepared-v2.json")).unwrap()
    );
    assert_eq!(central_head, f.store.head().unwrap());
    assert_eq!(
        quota.held_workers,
        f.store
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers
    );
}

#[test]
fn divergent_intent_and_secret_material_refuse_before_creating_the_writer_lock() {
    let (f, intent, confirmed) = fixture();
    let other = KubernetesSessionReservationIntentV2::new(
        &Uuid::new_v4().to_string(),
        intent.worker_id(),
        intent.authority(),
        intent.profile(),
    )
    .unwrap();
    let root = root(&f, "refused");
    assert!(matches!(
        SessionJournal::prepare_or_resume(root.clone(), protection(), &other, &confirmed),
        Err(SessionError::Conflict)
    ));
    let secret = SessionJournalProtection::new(Vec::new(), vec!["tenant-a".into()]).unwrap();
    assert!(matches!(
        SessionJournal::prepare_or_resume(root.clone(), secret, &intent, &confirmed),
        Err(SessionError::UnsafeMaterial)
    ));
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn partial_and_foreign_inventory_remain_intact_and_cannot_be_adopted_as_a_session() {
    let (f, intent, confirmed) = fixture();
    let root = root(&f, "partial");
    root.persistent_create(".session-v2.lock", &[])
        .unwrap()
        .persistent_sync()
        .unwrap();
    root.sync().unwrap();
    assert!(
        SessionJournal::prepare_or_resume(root.clone(), protection(), &intent, &confirmed).is_err()
    );
    assert!(root.path().join(".session-v2.lock").exists());
    fs::write(root.path().join("foreign"), b"preserve fixture evidence").unwrap();
    assert!(
        SessionJournal::prepare_or_resume(root.clone(), protection(), &intent, &confirmed).is_err()
    );
    assert_eq!(
        fs::read(root.path().join("foreign")).unwrap(),
        b"preserve fixture evidence"
    );
}

#[test]
fn revalidation_refuses_replaced_preparation_and_preserves_the_replacement() {
    let (f, intent, confirmed) = fixture();
    let root = root(&f, "replacement");
    let journal =
        SessionJournal::prepare_or_resume(root.clone(), protection(), &intent, &confirmed).unwrap();
    fs::rename(
        root.path().join("prepared-v2.json"),
        root.path().join("retained-original"),
    )
    .unwrap();
    fs::write(root.path().join("prepared-v2.json"), b"foreign replacement").unwrap();
    assert!(journal.bootstrap_binding().is_err());
    assert_eq!(
        fs::read(root.path().join("prepared-v2.json")).unwrap(),
        b"foreign replacement"
    );
}

#[test]
fn a_descriptive_profile_cannot_replace_the_confirmed_control_namespace() {
    let (f, intent, confirmed) = fixture();
    let mut wire = serde_json::to_value(&intent).unwrap();
    wire["profile"]["binding"]["control_namespace_uuid"] = serde_json::json!(Uuid::new_v4());
    let divergent = KubernetesSessionReservationIntentV2::retained_from_value(wire).unwrap();
    let root = root(&f, "wrong-namespace");
    assert!(matches!(
        SessionJournal::prepare_or_resume(root.clone(), protection(), &divergent, &confirmed),
        Err(SessionError::Conflict)
    ));
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn a_different_real_reservation_cannot_overwrite_an_existing_preparation() {
    let (f, intent, confirmed) = fixture();
    let root = root(&f, "immutable");
    let journal =
        SessionJournal::prepare_or_resume(root.clone(), protection(), &intent, &confirmed).unwrap();
    let original = fs::read(root.path().join("prepared-v2.json")).unwrap();
    drop(journal);
    let (_other_fixture, other_intent, other_confirmed) = fixture();
    assert!(matches!(
        SessionJournal::prepare_or_resume(
            root.clone(),
            protection(),
            &other_intent,
            &other_confirmed
        ),
        Err(SessionError::Conflict)
    ));
    assert_eq!(
        original,
        fs::read(root.path().join("prepared-v2.json")).unwrap()
    );
    let retained =
        SessionJournal::prepare_or_resume(root, protection(), &intent, &confirmed).unwrap();
    assert!(retained.bootstrap_binding().is_ok());
}
