//! Real private-filesystem tests. Reservation/claim records here are descriptive
//! synthetic facts; they never construct a Store proof or a runtime permission.
use super::*;
use crate::control::kubernetes_admission::{
    KubernetesCreateEmissionWindowV1, KubernetesCreateOutcomeV1, KubernetesCreateStateV1,
    KubernetesCreateUnknownReasonV1,
};
use crate::runtime::kubernetes::{KubernetesAuthorityV1, admission};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;
use tempfile::TempDir;
use uuid::Uuid;

struct Fixture {
    root: TempDir,
    intent: KubernetesCreateIntentV1,
    reserved: KubernetesCreateSnapshotV1,
    reservation: KubernetesCreateReceiptV1,
    claim: KubernetesCreateClaimV1,
}
impl Fixture {
    fn new() -> Self {
        let temporary = if cfg!(target_os = "linux") {
            std::env::temp_dir()
        } else {
            "/private/tmp".into()
        };
        let root = tempfile::Builder::new()
            .prefix("ai-guard-held-journal-")
            .tempdir_in(temporary)
            .unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let authority = KubernetesAuthorityV1::new(
            Uuid::from_u128(1),
            "https://127.0.0.1:6443",
            "11111111-1111-4111-8111-111111111111",
            "tenant-a",
            "22222222-2222-4222-8222-222222222222",
            &"a".repeat(64),
        )
        .unwrap();
        let intent: KubernetesCreateIntentV1 = serde_json::from_value(json!({
            "version":1,"create_id":Uuid::from_u128(8).to_string(),
            "session_id":Uuid::from_u128(7).to_string(),
            "control_namespace_uuid":Uuid::from_u128(3).to_string(),
            "broker_id":Uuid::from_u128(4).to_string(),
            "incarnation_id":Uuid::from_u128(5).to_string(),"authority":authority,
            "profile":"kubernetes_held_pod_v1",
            "image":format!("local/worker@sha256:{}","b".repeat(64)),
            "resources":{"cpu_millis":2000,"memory_mib":1024,
                "workspace_mib":128,"temporary_mib":64,"deadline_seconds":60}
        }))
        .unwrap();
        let prepared = admission::prepare(&intent).unwrap();
        let reserved = KubernetesCreateSnapshotV1 {
            version: 1,
            intent: intent.clone(),
            intent_hash: intent.intent_hash().unwrap(),
            target: prepared.target().clone(),
            state: KubernetesCreateStateV1::Reserved,
            create_epoch: 1,
            revision: 1,
            quota_held: true,
            claim: None,
            emission_window: None,
            last_result: None,
            last_cancel: None,
        };
        let reservation = KubernetesCreateReceiptV1 {
            version: 1,
            operation_id: intent.create_id.clone(),
            input_hash: reserved.intent_hash.clone(),
            control_namespace_uuid: intent.control_namespace_uuid.clone(),
            audit_seq: 10,
            audit_hash: "c".repeat(64),
            emission_window: None,
        };
        let claim = KubernetesCreateClaimV1 {
            version: 1,
            operation_id: Uuid::from_u128(9).to_string(),
            create_id: intent.create_id.clone(),
            intent_hash: reserved.intent_hash.clone(),
            expected_revision: 1,
            expected_create_epoch: 1,
            emitter_id: Uuid::from_u128(10).to_string(),
        };
        Self {
            root,
            intent,
            reserved,
            reservation,
            claim,
        }
    }
    fn directory(&self) -> Directory {
        Directory::open_existing(self.root.path()).unwrap()
    }
    fn protection(&self) -> AdmissionJournalProtection {
        AdmissionJournalProtection::new(Vec::new(), Vec::new())
    }
    fn prepare(&self) -> JournalResult<AdmissionJournal> {
        AdmissionJournal::prepare(
            self.directory(),
            self.protection(),
            "tenant-a",
            &self.intent,
            admission::prepare(&self.intent).unwrap(),
            &self.reservation,
            &self.reserved,
            &self.claim,
        )
    }
    fn recover(&self) -> JournalResult<Option<RecoveredHeldCreate>> {
        AdmissionJournal::recover(self.directory(), self.protection())
    }
    fn candidate(&self) -> KubernetesCreateResultV1 {
        KubernetesCreateResultV1 {
            version: 1,
            operation_id: Uuid::from_u128(11).to_string(),
            create_id: self.intent.create_id.clone(),
            intent_hash: self.reserved.intent_hash.clone(),
            claim_operation_id: self.claim.operation_id.clone(),
            expected_revision: 2,
            expected_create_epoch: 1,
            outcome: KubernetesCreateOutcomeV1::OwnershipUnknown {
                reason: KubernetesCreateUnknownReasonV1::RecoveredAfterClaim,
                http_status: None,
            },
        }
    }
    // Model only a descriptive historical checkpoint already on disk. No proof
    // is fabricated: recover must expose ObserveOnly, never fresh authority.
    fn historical_emission(&self) -> AdmissionJournal {
        drop(self.prepare().expect("prepare must retain exact request"));
        let window = KubernetesCreateEmissionWindowV1 {
            issued_at: "2026-11-05T08:00:00.000Z".into(),
            not_after: "2026-11-05T08:00:08.000Z".into(),
        };
        let mut snapshot = self.reserved.clone();
        snapshot.state = KubernetesCreateStateV1::EmissionPossible;
        snapshot.revision = 2;
        snapshot.claim = Some(self.claim.clone());
        snapshot.emission_window = Some(window.clone());
        let receipt = KubernetesCreateReceiptV1 {
            version: 1,
            operation_id: self.claim.operation_id.clone(),
            input_hash: self.claim.input_hash().unwrap(),
            control_namespace_uuid: self.intent.control_namespace_uuid.clone(),
            audit_seq: 11,
            audit_hash: "d".repeat(64),
            emission_window: Some(window),
        };
        let prepared_bytes = fs::read(self.root.path().join("prepared.json")).unwrap();
        let record = json!({"version":1,
            "prepared_sha256":hex::encode(Sha256::digest(&prepared_bytes)),
            "claim":self.claim,"receipt":receipt,"snapshot":snapshot});
        let directory = self.directory();
        let record = directory
            .persistent_create("emission.json", &serde_json::to_vec(&record).unwrap())
            .unwrap();
        record.persistent_sync().unwrap();
        directory.sync().unwrap();
        drop(record);
        let recovered = self.recover().unwrap().unwrap();
        assert_eq!(recovered.phase(), RecoveryPhase::ObserveOnly);
        recovered.into_journal()
    }
}

fn names(root: &Path) -> BTreeSet<String> {
    fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect()
}
fn private_write(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn held_journal_prepared_request_and_claim_survive_reopen_without_emission_authority() {
    let fixture = Fixture::new();
    assert!(fixture.recover().unwrap().is_none());
    let expected = admission::prepare(&fixture.intent).unwrap();
    let journal = fixture
        .prepare()
        .expect("private prepared record must be durable");
    assert_eq!(
        fs::read(fixture.root.path().join("pod.json")).unwrap(),
        expected.pod_bytes()
    );
    let record: Value =
        serde_json::from_slice(&fs::read(fixture.root.path().join("prepared.json")).unwrap())
            .unwrap();
    assert_eq!(
        record["claim_request"],
        serde_json::to_value(&fixture.claim).unwrap()
    );
    assert_eq!(
        names(fixture.root.path()),
        BTreeSet::from([
            ".writer.lock".into(),
            "pod.json".into(),
            "prepared.json".into()
        ])
    );
    assert!(matches!(fixture.recover(), Err(JournalError::Busy)));
    drop(journal);
    let recovered = fixture.recover().unwrap().unwrap();
    assert_eq!(recovered.phase(), RecoveryPhase::Prepared);
    assert!(recovered.into_journal().candidate().is_none());
}

#[test]
fn held_journal_partial_unknown_and_duplicate_records_are_preserved_and_refused() {
    for (name, bytes) in [
        ("prepared.json", b"{".as_slice()),
        ("unexpected", b"safe".as_slice()),
    ] {
        let fixture = Fixture::new();
        drop(fixture.prepare().unwrap());
        private_write(&fixture.root.path().join(name), bytes);
        assert!(matches!(fixture.recover(), Err(JournalError::Corrupt)));
        assert_eq!(fs::read(fixture.root.path().join(name)).unwrap(), bytes);
    }
    let fixture = Fixture::new();
    drop(fixture.prepare().unwrap());
    let path = fixture.root.path().join("prepared.json");
    let bytes = fs::read(&path).unwrap();
    let duplicated = [b"{\"version\":1,".as_slice(), &bytes[1..]].concat();
    private_write(&path, &duplicated);
    assert!(matches!(fixture.recover(), Err(JournalError::Corrupt)));
    assert_eq!(fs::read(&path).unwrap(), duplicated);
}

#[test]
fn held_journal_rejects_links_and_excluded_private_inodes_without_changing_them() {
    let safe = Fixture::new();
    drop(
        safe.prepare()
            .expect("safe private directory must first be accepted"),
    );
    assert_eq!(
        safe.recover().unwrap().unwrap().phase(),
        RecoveryPhase::Prepared
    );
    // A regular nlink=1 file in a valid journal is refused when its identity is
    // explicitly excluded, independently of the generic hardlink/symlink rules.
    let mut protection = safe.protection();
    protection
        .exclude_known_file(&safe.directory(), "prepared.json")
        .unwrap();
    assert!(AdmissionJournal::recover(safe.directory(), protection).is_err());
    let fixture = Fixture::new();
    let outside = fixture.root.path().join("protected");
    fs::create_dir(&outside).unwrap();
    fs::set_permissions(&outside, fs::Permissions::from_mode(0o700)).unwrap();
    let file = outside.join("credential");
    private_write(&file, b"synthetic-private-material");
    let protected = PrivateFile::open(&file, 1024).unwrap();
    // Use a distinct journal root, so the protected directory is not an unknown entry.
    let journal_root = fixture.root.path().join("journal");
    fs::create_dir(&journal_root).unwrap();
    fs::set_permissions(&journal_root, fs::Permissions::from_mode(0o700)).unwrap();
    fs::hard_link(&file, journal_root.join("prepared.json")).unwrap();
    let result = AdmissionJournal::prepare(
        Directory::open_existing(&journal_root).unwrap(),
        AdmissionJournalProtection::new(vec![protected], Vec::new()),
        "tenant-a",
        &fixture.intent,
        admission::prepare(&fixture.intent).unwrap(),
        &fixture.reservation,
        &fixture.reserved,
        &fixture.claim,
    );
    assert!(result.is_err());
    assert_eq!(fs::read(&file).unwrap(), b"synthetic-private-material");
    fs::remove_file(journal_root.join("prepared.json")).unwrap();
    symlink(&file, journal_root.join("prepared.json")).unwrap();
    assert!(
        AdmissionJournal::recover(
            Directory::open_existing(&journal_root).unwrap(),
            fixture.protection()
        )
        .is_err()
    );
    assert!(
        fs::symlink_metadata(journal_root.join("prepared.json"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn held_journal_retains_one_exact_candidate_and_first_refusal_across_reopen() {
    let fixture = Fixture::new();
    let mut journal = fixture.historical_emission();
    let candidate = fixture.candidate();
    journal.retain_candidate(&candidate, None).unwrap();
    journal
        .retain_candidate(&candidate, Some(CandidateRefusal::StaleEpoch))
        .unwrap();
    let candidate_bytes = fs::read(fixture.root.path().join("candidate.json")).unwrap();
    let refusal_bytes = fs::read(fixture.root.path().join("refusal.json")).unwrap();
    journal
        .retain_candidate(&candidate, Some(CandidateRefusal::StaleEpoch))
        .unwrap();
    let mut changed = candidate.clone();
    changed.expected_revision += 1;
    assert!(matches!(
        journal.retain_candidate(&changed, None),
        Err(JournalError::Conflict)
    ));
    assert!(matches!(
        journal.retain_candidate(&candidate, Some(CandidateRefusal::Unavailable)),
        Err(JournalError::Conflict)
    ));
    assert_eq!(
        fs::read(fixture.root.path().join("candidate.json")).unwrap(),
        candidate_bytes
    );
    assert_eq!(
        fs::read(fixture.root.path().join("refusal.json")).unwrap(),
        refusal_bytes
    );
    drop(journal);
    let recovered = fixture.recover().unwrap().unwrap();
    assert_eq!(recovered.phase(), RecoveryPhase::CandidateRetained);
    let mut journal = recovered.into_journal();
    assert_eq!(journal.candidate(), Some(&candidate));
    journal.retain_candidate(&candidate, None).unwrap();
    assert_eq!(names(fixture.root.path()).len(), 6);
}

#[test]
fn held_journal_live_tamper_refuses_candidate_without_rewriting_history() {
    for changed in ["pod.json", "prepared.json", "emission.json", "unexpected"] {
        let fixture = Fixture::new();
        let mut journal = fixture.historical_emission();
        let path = fixture.root.path().join(changed);
        private_write(&path, b"{}");
        assert!(matches!(
            journal.retain_candidate(&fixture.candidate(), None),
            Err(JournalError::Corrupt)
        ));
        assert!(!fixture.root.path().join("candidate.json").exists());
        assert_eq!(fs::read(path).unwrap(), b"{}");
    }
}

#[test]
fn held_journal_screens_material_before_creating_any_record() {
    let fixture = Fixture::new();
    let result = AdmissionJournal::prepare(
        fixture.directory(),
        AdmissionJournalProtection::new(Vec::new(), vec!["local/worker".into()]),
        "tenant-a",
        &fixture.intent,
        admission::prepare(&fixture.intent).unwrap(),
        &fixture.reservation,
        &fixture.reserved,
        &fixture.claim,
    );
    assert!(matches!(result, Err(JournalError::UnsafeMaterial)));
    assert!(names(fixture.root.path()).is_empty());
}

#[test]
fn held_journal_protection_budgets_count_distinct_secrets_and_private_inodes() {
    let protected = Fixture::new();
    let credential = protected.root.path().join("public-test-credential");
    private_write(&credential, b"synthetic-private-material");
    let file = PrivateFile::open(&credential, 1024).unwrap();
    for duplicate_files in [false, true] {
        let fixture = Fixture::new();
        let (files, secrets) = if duplicate_files {
            (vec![file.clone(); 65], Vec::new())
        } else {
            (Vec::new(), vec!["never-present-private-literal".into(); 65])
        };
        let journal = AdmissionJournal::prepare(
            fixture.directory(),
            AdmissionJournalProtection::new(files, secrets),
            "tenant-a",
            &fixture.intent,
            admission::prepare(&fixture.intent).unwrap(),
            &fixture.reservation,
            &fixture.reserved,
            &fixture.claim,
        )
        .expect("65 copies of one protection must consume one distinct slot");
        drop(journal);
        assert_eq!(
            fixture.recover().unwrap().unwrap().phase(),
            RecoveryPhase::Prepared
        );
    }
    // The acceptance of duplicates must not enlarge either distinct cap.
    for distinct_files in [false, true] {
        let fixture = Fixture::new();
        let (files, secrets) = if distinct_files {
            let mut files = Vec::new();
            for index in 0..65 {
                let path = protected.root.path().join(format!("protected-{index}"));
                private_write(&path, b"synthetic-private-material");
                files.push(PrivateFile::open(&path, 1024).unwrap());
            }
            (files, Vec::new())
        } else {
            (
                Vec::new(),
                (0..65)
                    .map(|index| format!("never-present-private-{index}"))
                    .collect(),
            )
        };
        let result = AdmissionJournal::prepare(
            fixture.directory(),
            AdmissionJournalProtection::new(files, secrets),
            "tenant-a",
            &fixture.intent,
            admission::prepare(&fixture.intent).unwrap(),
            &fixture.reservation,
            &fixture.reserved,
            &fixture.claim,
        );
        assert!(matches!(result, Err(JournalError::Capacity)));
        assert!(names(fixture.root.path()).is_empty());
    }
}
