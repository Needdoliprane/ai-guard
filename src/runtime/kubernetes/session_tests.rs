//! Real SQLite reservation/audit and private journal; no Pod or runtime effect.
use super::*;
use crate::{
    audit::kubernetes_admission_state_tests::Fixture,
    control::{QuotaPolicyV1, kubernetes_session::*},
    provider::{
        AuthMode, ChoiceSource, ClientKind, ClientTransport, ProviderChoiceProvenanceV1,
        ProviderResolutionV1,
    },
    runtime::kubernetes::{
        KubernetesAuthorityV1,
        session_instance::{
            parse_workspace_claim,
            tests::{CLAIM_UID, OBSERVED_AT, bound_claim, bound_pvc},
        },
        session_profile::*,
    },
};
use std::{fs, os::unix::fs::PermissionsExt};
use uuid::Uuid;

pub(crate) fn fixture() -> (
    Fixture,
    KubernetesSessionReservationIntentV2,
    ConfirmedSessionReservationV2,
) {
    fixture_with(None)
}
/// Same real reservation path for an owned loopback endpoint. The authority
/// keeps the fixture's id and anchors, so the real broker grant still covers
/// it; only the endpoint and trust digest differ.
pub(crate) fn fixture_at(
    endpoint: &str,
    trust_sha256: &str,
) -> (
    Fixture,
    KubernetesSessionReservationIntentV2,
    ConfirmedSessionReservationV2,
) {
    fixture_with(Some((endpoint, trust_sha256)))
}
/// Preparation binding of a real confirmed journal (dropped afterwards).
pub(crate) fn prepared_binding(
    f: &Fixture,
    intent: &KubernetesSessionReservationIntentV2,
    confirmed: &ConfirmedSessionReservationV2,
) -> SessionBootstrapBindingV2 {
    SessionJournal::prepare_or_resume(root(f, "binding"), protection(), intent, confirmed)
        .unwrap()
        .bootstrap_binding()
        .unwrap()
}
fn fixture_with(
    loopback: Option<(&str, &str)>,
) -> (
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
    let authority = match loopback {
        None => f.intent.authority.clone(),
        Some((endpoint, trust_sha256)) => KubernetesAuthorityV1::new(
            f.intent.authority.authority_id(),
            endpoint,
            &f.intent.authority.cluster_namespace_uid,
            f.intent.authority.namespace(),
            &f.intent.authority.namespace_uid,
            trust_sha256,
        )
        .unwrap(),
    };
    let binding = KubernetesSessionBindingV2::new(
        &authority,
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
        &authority,
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
        &authority,
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
/// A fresh private journal directory under the fixture's private root.
pub(crate) fn root(f: &Fixture, name: &str) -> Directory {
    Directory::open_existing(f._directory.path())
        .unwrap()
        .persistent_child(name, true)
        .unwrap()
        .unwrap()
}
pub(crate) fn protection() -> SessionJournalProtection {
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

const RETAINED: [&str; 3] = ["prepared-v2.json", "workspace-v2.json", "pod-v2.json"];
/// Writes a private file the journal did not create (tamper or foreign name).
pub(crate) fn write_private(root: &Directory, name: &str, bytes: &[u8]) {
    let path = root.path().join(name);
    fs::write(&path, bytes).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
}
fn retained_files(root: &Directory) -> [Vec<u8>; 3] {
    RETAINED.map(|name| fs::read(root.path().join(name)).unwrap())
}

#[test]
fn retained_request_survives_reopen_with_a_new_real_confirmation_and_no_central_event() {
    let (f, intent, confirmed) = fixture();
    let dir = root(&f, "retained");
    let central_head = f.store.head().unwrap();
    let held = f
        .store
        .worker_quota(&QuotaPolicyV1::default())
        .unwrap()
        .held_workers;
    let mut journal =
        SessionJournal::prepare_or_resume(dir.clone(), protection(), &intent, &confirmed).unwrap();
    assert_eq!(journal.phase().unwrap(), SessionPhaseV2::Prepared);
    let claim = bound_claim(&intent, CLAIM_UID);
    journal.retain_workspace_claim(&claim).unwrap();
    assert_eq!(journal.phase().unwrap(), SessionPhaseV2::WorkspaceRetained);
    let binding = journal.bootstrap_binding().unwrap();
    let request = KubernetesSessionPodRequestV2::render(&intent, &binding, &claim).unwrap();
    journal.retain_pod_request(&request).unwrap();
    // Identical retention is an idempotent success that writes nothing new.
    journal.retain_workspace_claim(&claim).unwrap();
    journal.retain_pod_request(&request).unwrap();
    assert_eq!(journal.phase().unwrap(), SessionPhaseV2::RequestRetained);
    let before = retained_files(&dir);
    assert_eq!(before[2], request.bytes());
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
        SessionJournal::prepare_or_resume(dir.clone(), protection(), &intent, &confirmed).unwrap();
    assert_eq!(reopened.phase().unwrap(), SessionPhaseV2::RequestRetained);
    assert!(reopened.intent().unwrap().same(&intent).unwrap());
    assert_eq!(reopened.bootstrap_binding().unwrap(), binding);
    assert_eq!(retained_files(&dir), before);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 4);
    // Retention and reopen are local: no central event, no quota change.
    assert_eq!(central_head, f.store.head().unwrap());
    assert_eq!(
        held,
        f.store
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers
    );
    assert!(intent.authority().require_workload_admission().is_err());
}

#[test]
fn divergent_claim_or_request_is_a_conflict_and_preserves_retained_bytes() {
    let (f, intent, confirmed) = fixture();
    let dir = root(&f, "divergent");
    let mut journal =
        SessionJournal::prepare_or_resume(dir.clone(), protection(), &intent, &confirmed).unwrap();
    let claim = bound_claim(&intent, CLAIM_UID);
    journal.retain_workspace_claim(&claim).unwrap();
    let workspace = fs::read(dir.path().join("workspace-v2.json")).unwrap();
    // Another claim UID behind the same attempt name is never adopted.
    let other_uid = bound_claim(&intent, "66666666-6666-4666-8666-666666666666");
    assert!(matches!(
        journal.retain_workspace_claim(&other_uid),
        Err(SessionError::Conflict)
    ));
    let binding = journal.bootstrap_binding().unwrap();
    let for_other_uid =
        KubernetesSessionPodRequestV2::render(&intent, &binding, &other_uid).unwrap();
    assert!(matches!(
        journal.retain_pod_request(&for_other_uid),
        Err(SessionError::Conflict)
    ));
    assert!(!dir.path().join("pod-v2.json").exists());
    let request = KubernetesSessionPodRequestV2::render(&intent, &binding, &claim).unwrap();
    journal.retain_pod_request(&request).unwrap();
    let pod = fs::read(dir.path().join("pod-v2.json")).unwrap();
    // The real request of another reservation (own Store and journal).
    let (other_fixture, other_intent, other_confirmed) = fixture();
    let other = KubernetesSessionPodRequestV2::render(
        &other_intent,
        &prepared_binding(&other_fixture, &other_intent, &other_confirmed),
        &bound_claim(&other_intent, CLAIM_UID),
    )
    .unwrap();
    assert!(matches!(
        journal.retain_pod_request(&other),
        Err(SessionError::Conflict)
    ));
    assert_eq!(
        workspace,
        fs::read(dir.path().join("workspace-v2.json")).unwrap()
    );
    assert_eq!(pod, fs::read(dir.path().join("pod-v2.json")).unwrap());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 4);
    assert_eq!(journal.phase().unwrap(), SessionPhaseV2::RequestRetained);
}

#[test]
fn out_of_order_or_reserved_checkpoint_is_corrupt_and_preserved() {
    let (f, intent, confirmed) = fixture();
    // An exact request file whose workspace predecessor is missing.
    let skipped = root(&f, "skipped");
    let journal =
        SessionJournal::prepare_or_resume(skipped.clone(), protection(), &intent, &confirmed)
            .unwrap();
    let request = KubernetesSessionPodRequestV2::render(
        &intent,
        &journal.bootstrap_binding().unwrap(),
        &bound_claim(&intent, CLAIM_UID),
    )
    .unwrap();
    drop(journal);
    write_private(&skipped, "pod-v2.json", request.bytes());
    assert!(matches!(
        SessionJournal::prepare_or_resume(skipped.clone(), protection(), &intent, &confirmed),
        Err(SessionError::Corrupt)
    ));
    assert!(matches!(
        SessionJournal::inspect_retained(skipped.clone(), protection()),
        Err(SessionError::Corrupt)
    ));
    assert_eq!(
        fs::read(skipped.path().join("pod-v2.json")).unwrap(),
        request.bytes()
    );
    assert!(!skipped.path().join("workspace-v2.json").exists());
    // A future effect checkpoint name is refused until its stage exists.
    let reserved = root(&f, "reserved");
    let mut journal =
        SessionJournal::prepare_or_resume(reserved.clone(), protection(), &intent, &confirmed)
            .unwrap();
    journal
        .retain_workspace_claim(&bound_claim(&intent, CLAIM_UID))
        .unwrap();
    drop(journal);
    let workspace = fs::read(reserved.path().join("workspace-v2.json")).unwrap();
    write_private(&reserved, "created-v2.json", b"{\"reserved\":true}");
    assert!(matches!(
        SessionJournal::prepare_or_resume(reserved.clone(), protection(), &intent, &confirmed),
        Err(SessionError::Corrupt)
    ));
    assert!(matches!(
        SessionJournal::inspect_retained(reserved.clone(), protection()),
        Err(SessionError::Corrupt)
    ));
    assert_eq!(
        fs::read(reserved.path().join("created-v2.json")).unwrap(),
        b"{\"reserved\":true}"
    );
    assert_eq!(
        fs::read(reserved.path().join("workspace-v2.json")).unwrap(),
        workspace
    );
}

#[test]
fn protected_material_in_a_retained_record_is_refused_before_write() {
    let (f, intent, confirmed) = fixture();
    let dir = root(&f, "protected");
    let name = session_workspace_claim_name(intent.ids().unwrap().attempt_id);
    let secret = SessionJournalProtection::new(Vec::new(), vec![name]).unwrap();
    let mut journal =
        SessionJournal::prepare_or_resume(dir.clone(), secret, &intent, &confirmed).unwrap();
    assert!(matches!(
        journal.retain_workspace_claim(&bound_claim(&intent, CLAIM_UID)),
        Err(SessionError::UnsafeMaterial)
    ));
    assert!(!dir.path().join("workspace-v2.json").exists());
    assert_eq!(journal.phase().unwrap(), SessionPhaseV2::Prepared);
}

#[test]
fn excluded_known_identity_is_never_read_and_is_preserved() {
    let (f, intent, confirmed) = fixture();
    let dir = root(&f, "excluded");
    let mut journal =
        SessionJournal::prepare_or_resume(dir.clone(), protection(), &intent, &confirmed).unwrap();
    journal
        .retain_workspace_claim(&bound_claim(&intent, CLAIM_UID))
        .unwrap();
    drop(journal);
    let names = [".session-v2.lock", "prepared-v2.json", "workspace-v2.json"];
    let snapshot = || names.map(|name| fs::read(dir.path().join(name)).unwrap());
    let before = snapshot();
    let excluding = |parent: &Directory, name: &str| {
        let mut excluded = protection();
        excluded.exclude_known_file(parent, name).unwrap();
        excluded
    };
    // Control: excluding a protected file outside the journal still reopens
    // it, so each refusal below comes from the identity match.
    let outside = root(&f, "protected-outside");
    write_private(&outside, "credential", b"synthetic protected material");
    assert_eq!(
        SessionJournal::prepare_or_resume(
            dir.clone(),
            excluding(&outside, "credential"),
            &intent,
            &confirmed
        )
        .unwrap()
        .phase()
        .unwrap(),
        SessionPhaseV2::WorkspaceRetained
    );
    assert_eq!(
        SessionJournal::inspect_retained(dir.clone(), excluding(&outside, "credential"))
            .unwrap()
            .phase(),
        SessionPhaseV2::WorkspaceRetained
    );
    // A journal file whose identity is excluded is refused by both openers
    // before its bytes are read, and nothing is changed.
    for name in names {
        assert!(
            matches!(
                SessionJournal::prepare_or_resume(
                    dir.clone(),
                    excluding(&dir, name),
                    &intent,
                    &confirmed
                ),
                Err(SessionError::Corrupt)
            ),
            "{name}"
        );
        assert!(
            matches!(
                SessionJournal::inspect_retained(dir.clone(), excluding(&dir, name)),
                Err(SessionError::Corrupt)
            ),
            "{name}"
        );
        assert_eq!(snapshot(), before);
    }
    // A protected identity that moved after its exclusion was recorded is
    // refused before any journal file is opened.
    let stale = excluding(&outside, "credential");
    fs::rename(
        outside.path().join("credential"),
        outside.path().join("credential-moved"),
    )
    .unwrap();
    assert!(matches!(
        SessionJournal::prepare_or_resume(dir.clone(), stale, &intent, &confirmed),
        Err(SessionError::UnsafeMaterial)
    ));
    assert_eq!(snapshot(), before);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 3);
}

#[test]
fn held_lock_is_busy_before_any_inventory_judgement() {
    let (f, intent, confirmed) = fixture();
    let dir = root(&f, "held");
    let journal =
        SessionJournal::prepare_or_resume(dir.clone(), protection(), &intent, &confirmed).unwrap();
    write_private(&dir, "foreign", b"preserve fixture evidence");
    // Base 77f7edd judged the inventory before trying the flock, so a live
    // owner with an extra entry was reported Corrupt instead of Busy.
    assert!(matches!(
        SessionJournal::prepare_or_resume(dir.clone(), protection(), &intent, &confirmed),
        Err(SessionError::Busy)
    ));
    assert!(matches!(
        SessionJournal::inspect_retained(dir.clone(), protection()),
        Err(SessionError::Busy)
    ));
    assert_eq!(
        fs::read(dir.path().join("foreign")).unwrap(),
        b"preserve fixture evidence"
    );
    drop(journal);
    assert!(matches!(
        SessionJournal::prepare_or_resume(dir.clone(), protection(), &intent, &confirmed),
        Err(SessionError::Corrupt)
    ));
    assert!(matches!(
        SessionJournal::inspect_retained(dir.clone(), protection()),
        Err(SessionError::Corrupt)
    ));
    assert_eq!(
        fs::read(dir.path().join("foreign")).unwrap(),
        b"preserve fixture evidence"
    );
}

#[test]
fn lone_lock_is_judged_without_taking_the_creators_flock() {
    let (f, intent, confirmed) = fixture();
    let dir = root(&f, "creator-window");
    // A creator after its exclusive create and its own flock, before it has
    // written the preparation.
    let creator = dir.persistent_create(".session-v2.lock", &[]).unwrap();
    creator.persistent_sync().unwrap();
    dir.sync().unwrap();
    assert!(creator.persistent_try_lock().unwrap());
    // An opener judges the absent preparation without the flock: a transient
    // Corrupt, never Busy, so it cannot preempt or block the creator.
    assert!(matches!(
        SessionJournal::prepare_or_resume(dir.clone(), protection(), &intent, &confirmed),
        Err(SessionError::Corrupt)
    ));
    assert!(matches!(
        SessionJournal::inspect_retained(dir.clone(), protection()),
        Err(SessionError::Corrupt)
    ));
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    // A lock left alone stays Corrupt and is preserved, never repaired.
    drop(creator);
    assert!(matches!(
        SessionJournal::prepare_or_resume(dir.clone(), protection(), &intent, &confirmed),
        Err(SessionError::Corrupt)
    ));
    assert!(matches!(
        SessionJournal::inspect_retained(dir.clone(), protection()),
        Err(SessionError::Corrupt)
    ));
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    assert!(dir.path().join(".session-v2.lock").exists());
}

#[test]
fn observe_only_view_never_creates_a_file_and_releases_the_lock() {
    let (f, intent, confirmed) = fixture();
    let empty = root(&f, "empty");
    assert!(matches!(
        SessionJournal::inspect_retained(empty.clone(), protection()),
        Err(SessionError::InvalidInput)
    ));
    assert_eq!(fs::read_dir(empty.path()).unwrap().count(), 0);
    let dir = root(&f, "viewed");
    let mut journal =
        SessionJournal::prepare_or_resume(dir.clone(), protection(), &intent, &confirmed).unwrap();
    let claim = bound_claim(&intent, CLAIM_UID);
    journal.retain_workspace_claim(&claim).unwrap();
    let binding = journal.bootstrap_binding().unwrap();
    let request = KubernetesSessionPodRequestV2::render(&intent, &binding, &claim).unwrap();
    journal.retain_pod_request(&request).unwrap();
    drop(journal);
    let before = retained_files(&dir);
    let view = SessionJournal::inspect_retained(dir.clone(), protection()).unwrap();
    assert_eq!(view.phase(), SessionPhaseV2::RequestRetained);
    assert!(view.intent().same(&intent).unwrap());
    assert_eq!(view.preparation_sha256(), binding.preparation_sha256);
    assert_eq!(view.claim(), Some(&claim));
    assert_eq!(view.request(), Some(&request));
    assert_eq!(view.request().unwrap().bytes(), before[2]);
    assert_eq!(retained_files(&dir), before);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 4);
    // The view released the lock: the writer reopens the same journal.
    let reopened =
        SessionJournal::prepare_or_resume(dir.clone(), protection(), &intent, &confirmed).unwrap();
    assert_eq!(reopened.phase().unwrap(), SessionPhaseV2::RequestRetained);
    assert_eq!(retained_files(&dir), before);
}

#[test]
fn tampered_workspace_or_request_record_is_corrupt_and_preserved() {
    let (f, intent, confirmed) = fixture();
    let corrupt = |dir: &Directory| {
        matches!(
            SessionJournal::prepare_or_resume(dir.clone(), protection(), &intent, &confirmed),
            Err(SessionError::Corrupt)
        ) && matches!(
            SessionJournal::inspect_retained(dir.clone(), protection()),
            Err(SessionError::Corrupt)
        )
    };
    let claim = bound_claim(&intent, CLAIM_UID);
    // A well-formed canonical request that the renderer does not reproduce.
    let dir = root(&f, "tampered-request");
    let mut journal =
        SessionJournal::prepare_or_resume(dir.clone(), protection(), &intent, &confirmed).unwrap();
    journal.retain_workspace_claim(&claim).unwrap();
    let binding = journal.bootstrap_binding().unwrap();
    let request = KubernetesSessionPodRequestV2::render(&intent, &binding, &claim).unwrap();
    drop(journal);
    // Control: the exact rendered bytes written by hand are adopted.
    write_private(&dir, "pod-v2.json", request.bytes());
    assert_eq!(
        SessionJournal::prepare_or_resume(dir.clone(), protection(), &intent, &confirmed)
            .unwrap()
            .phase()
            .unwrap(),
        SessionPhaseV2::RequestRetained
    );
    let mut pod: Value = serde_json::from_slice(request.bytes()).unwrap();
    pod["spec"]["activeDeadlineSeconds"] = serde_json::json!(61);
    let tampered = crate::control::kubernetes_session::validation::canonical(&pod).unwrap();
    assert_ne!(tampered, request.bytes());
    write_private(&dir, "pod-v2.json", &tampered);
    assert!(corrupt(&dir));
    assert_eq!(fs::read(dir.path().join("pod-v2.json")).unwrap(), tampered);
    // A workspace record for another attempt or another preparation, where
    // no claim was retained yet.
    let dir = root(&f, "tampered-workspace");
    let journal =
        SessionJournal::prepare_or_resume(dir.clone(), protection(), &intent, &confirmed).unwrap();
    let preparation = journal.bootstrap_binding().unwrap().preparation_sha256;
    drop(journal);
    // Control: the exact record a journal writes, written by hand, is adopted.
    let own = workspace_record(&claim, &preparation).unwrap();
    write_private(&dir, "workspace-v2.json", &own);
    assert_eq!(
        SessionJournal::prepare_or_resume(dir.clone(), protection(), &intent, &confirmed)
            .unwrap()
            .phase()
            .unwrap(),
        SessionPhaseV2::WorkspaceRetained
    );
    let other_name = session_workspace_claim_name(Uuid::new_v4());
    let other_attempt = parse_workspace_claim(
        intent.authority(),
        &other_name,
        bound_pvc(intent.authority().namespace(), &other_name, CLAIM_UID),
        OBSERVED_AT,
    )
    .unwrap();
    for record in [
        workspace_record(&other_attempt, &preparation).unwrap(),
        workspace_record(&claim, &"0".repeat(64)).unwrap(),
    ] {
        write_private(&dir, "workspace-v2.json", &record);
        assert!(corrupt(&dir));
        assert_eq!(
            fs::read(dir.path().join("workspace-v2.json")).unwrap(),
            record
        );
    }
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 3);
}

#[test]
fn abandonment_retention_is_idempotent_and_a_divergent_close_conflicts() {
    let (f, intent, confirmed) = fixture();
    let dir = root(&f, "abandonment");
    let mut journal =
        SessionJournal::prepare_or_resume(dir.clone(), protection(), &intent, &confirmed).unwrap();
    let binding = journal.bootstrap_binding().unwrap();
    let close = KubernetesSessionReservationCloseV2 {
        version: 2,
        operation_id: Uuid::new_v4().to_string(),
        reservation_id: intent.reservation_id().into(),
        reservation_input_hash: binding.reservation_input_hash.clone(),
        expected_revision: 1,
        reason: KubernetesSessionReservationCloseReasonV2::PreparationAbandoned,
    };
    journal.retain_abandonment(&close).unwrap();
    let path = dir.path().join("abandoned-v2.json");
    let bytes = fs::read(&path).unwrap();
    // The same close again is an idempotent success that writes nothing.
    journal.retain_abandonment(&close).unwrap();
    assert_eq!(fs::read(&path).unwrap(), bytes);
    // Another operation id, or a close this journal never retains, conflicts
    // and leaves the retained bytes intact.
    for divergent in [
        KubernetesSessionReservationCloseV2 {
            operation_id: Uuid::new_v4().to_string(),
            ..close.clone()
        },
        KubernetesSessionReservationCloseV2 {
            reason: KubernetesSessionReservationCloseReasonV2::OperatorCancelled,
            ..close.clone()
        },
        KubernetesSessionReservationCloseV2 {
            expected_revision: 2,
            ..close.clone()
        },
    ] {
        assert_eq!(
            journal.retain_abandonment(&divergent),
            Err(SessionError::Conflict)
        );
    }
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(journal.retained_abandonment().unwrap(), Some(close.clone()));
    assert_eq!(journal.phase().unwrap(), SessionPhaseV2::Abandoned);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 3);
    drop(journal);
    // Reopen adopts the exact retained close, and retaining it again is still
    // an idempotent success.
    let mut reopened =
        SessionJournal::prepare_or_resume(dir.clone(), protection(), &intent, &confirmed).unwrap();
    assert_eq!(
        reopened.retained_abandonment().unwrap(),
        Some(close.clone())
    );
    reopened.retain_abandonment(&close).unwrap();
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 3);
}
