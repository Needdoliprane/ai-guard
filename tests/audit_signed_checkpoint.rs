//! Acceptance of signed audit checkpoints at the library boundary: real SQLite
//! audit databases, real Ed25519 keys and real files in private directories.

use agentsandbox::{
    audit::{
        Store,
        signed_checkpoint::{
            AdvanceOutcome, AdvanceStatus, AnchorAdvanceRequest, AnchorInitRequest, AnchorOutcome,
            CheckpointStatement, CreateOutcome, CreateRequest, CreateStatus, ExportRequest,
            Failure, MAX_ANCHOR_KEYS, MAX_BUNDLE_ENTRIES, MAX_LOG_RECORDS,
            MAX_RETAINED_CHECKPOINTS, RecordSource, RetiringKey, RollbackDetection, RotateOutcome,
            RotateRequest, RotateStatus, RotationApprovalRequest, RotationStatement,
            SIGNATURE_DOMAIN, SignedRecord, Statement, Verdict, VerificationReport, VerifyRequest,
            Warning, advance_anchor, approve_rotation, create_checkpoint, export_bundle,
            init_anchor, init_key, rotate_key, show_key, verify,
        },
    },
    model::Event,
    redaction::Redactor,
};
use sha2::{Digest, Sha256};
use std::{
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

const TENANT: &str = "tenant-a";

fn private_directory() -> tempfile::TempDir {
    tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}

fn private_child(parent: &Path, name: &str) -> PathBuf {
    use std::os::unix::fs::DirBuilderExt;
    let path = parent.join(name);
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&path)
        .unwrap();
    path
}

fn write_private(path: &Path, bytes: &[u8]) {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
}

fn open_store(path: &Path, tenant: &str) -> Store {
    Store::open_for_tenant(path, tenant, Redactor::new(vec![]), 10_000).unwrap()
}

fn note(text: &str) -> Event {
    let mut event = Event::new("fixture.note");
    event.data = serde_json::json!({ "note": text });
    event
}

/// One control installation: an audit database, a signing key, a checkpoint
/// log, and a separate directory standing for the anchor holder.
struct Installation {
    directory: tempfile::TempDir,
    database: PathBuf,
    key_dir: PathBuf,
    log_dir: PathBuf,
    holder: PathBuf,
    key_id: String,
}

impl Installation {
    /// A database with a short existing history, closed, and a fresh key.
    fn new(tenant: &str) -> Self {
        let root = private_directory();
        let control = private_child(root.path(), "control");
        let database = control.join("audit.sqlite");
        let store = open_store(&database, tenant);
        store.append(note("alpha")).unwrap();
        drop(store);
        let key_dir = root.path().join("keys").join("key-1");
        let key = init_key(&key_dir).unwrap();
        let holder = private_child(root.path(), "holder");
        Self {
            log_dir: root.path().join("checkpoints"),
            database,
            key_dir,
            holder,
            key_id: key.key_id,
            directory: root,
        }
    }

    fn root(&self) -> &Path {
        self.directory.path()
    }

    fn create(&self) -> Result<CreateOutcome, Failure> {
        self.create_for(TENANT, &self.database, &self.log_dir)
    }

    fn create_for(
        &self,
        tenant: &str,
        database: &Path,
        log_dir: &Path,
    ) -> Result<CreateOutcome, Failure> {
        create_checkpoint(&CreateRequest {
            data: database,
            tenant_id: tenant,
            key_dir: &self.key_dir,
            checkpoint_dir: log_dir,
            new_log: starts_a_log(log_dir),
            previous_record: None,
        })
    }

    /// The anchor holder approves the key by its fingerprint.
    fn anchor(&self, name: &str, tenant: &str, namespace: &str) -> PathBuf {
        let public_key = self.holder.join(format!("{name}.public-key.json"));
        show_key(&self.key_dir, Some(&public_key)).unwrap();
        let anchor = self.holder.join(format!("{name}.anchor.json"));
        init_anchor(&AnchorInitRequest {
            public_key: &public_key,
            expect_key_id: &self.key_id,
            tenant_id: tenant,
            namespace_uuid: namespace,
            out: &anchor,
        })
        .unwrap();
        anchor
    }

    fn verify(&self, anchor: &Path, log_dir: &Path, database: Option<&Path>) -> VerificationReport {
        verify(&VerifyRequest {
            anchor,
            source: RecordSource::Directory(log_dir),
            data: database,
            require_reference: false,
            report_out: None,
        })
        .unwrap()
    }
}

/// Whether a checkpoint directory still has to be started explicitly.
fn starts_a_log(log_dir: &Path) -> bool {
    !log_dir.exists() || record_files(log_dir).is_empty()
}

fn assert_refused(report: &VerificationReport, code: &str) {
    assert_eq!(
        report.error_codes(),
        vec![code],
        "expected exactly the refusal {code}: {:?}",
        report.errors
    );
    assert_eq!(report.verdict, Verdict::Refused);
    let error = &report.errors[0];
    assert!(!error.message.is_empty() && !error.action.is_empty());
}

fn record_files(log_dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(log_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("cp-"))
        })
        .collect();
    files.sort();
    files
}

/// Every entry of a directory, hidden ones included, sorted by name.
fn entry_names(directory: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

fn file_name_of(path: &str) -> String {
    Path::new(path)
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned()
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// The public, unkeyed digest of the audit chain (`agentsandbox:audit:v1`).
fn chain_digest(seq: i64, received_at: &str, previous: &str, payload: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"agentsandbox:audit:v1\0");
    hasher.update(seq.to_be_bytes());
    for part in [received_at, previous, payload] {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    hex::encode(hasher.finalize())
}

/// Rewrite the last `fixture.note` from `alpha` to `bravo`, as an actor with
/// physical write access to the stopped database would. With
/// `recompute_hash`, the public chain digest is updated coherently.
fn rewrite_last_note(database: &Path, recompute_hash: bool) {
    let db = rusqlite::Connection::open(database).unwrap();
    let (seq, received_at, previous, payload): (i64, String, String, String) = db
        .query_row(
            "SELECT seq,received_at,prev_hash,payload FROM events ORDER BY seq DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert!(payload.contains("\"note\":\"alpha\""));
    let rewritten = payload.replace("\"note\":\"alpha\"", "\"note\":\"bravo\"");
    assert_eq!(rewritten.len(), payload.len());
    if recompute_hash {
        let hash = chain_digest(seq, &received_at, &previous, &rewritten);
        db.execute(
            "UPDATE events SET payload=?1,hash=?2 WHERE seq=?3",
            rusqlite::params![rewritten, hash, seq],
        )
        .unwrap();
    } else {
        db.execute(
            "UPDATE events SET payload=?1 WHERE seq=?2",
            rusqlite::params![rewritten, seq],
        )
        .unwrap();
    }
}

fn last_note(database: &Path) -> String {
    let db = rusqlite::Connection::open(database).unwrap();
    let payload: String = db
        .query_row(
            "SELECT payload FROM events ORDER BY seq DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let event: serde_json::Value = serde_json::from_str(&payload).unwrap();
    event["data"]["note"].as_str().unwrap().to_owned()
}

/// Replace the single record of `log_dir` by `content`, named after the
/// SHA-256 of its statement member exactly as a consistent forger would.
fn replace_record_consistently(log_dir: &Path, content: &str) {
    let files = record_files(log_dir);
    assert_eq!(files.len(), 1);
    let start = content.find("\"statement\":").unwrap() + "\"statement\":".len();
    let end = content.rfind(",\"signatures\":").unwrap();
    let name = format!(
        "cp-{:020}-{}.json",
        1,
        sha256_hex(&content.as_bytes()[start..end])
    );
    std::fs::remove_file(&files[0]).unwrap();
    write_private(&log_dir.join(name), content.as_bytes());
}

#[test]
fn valid_checkpoint_verifies_offline_with_explicitly_approved_key() {
    let installation = Installation::new(TENANT);
    let created = installation.create().unwrap();
    assert_eq!(created.status, CreateStatus::Created);
    assert_eq!(created.log_index, 1);
    assert_eq!(created.signing_key_id, installation.key_id);
    assert_eq!(created.tenant_id, TENANT);
    let anchor = installation.anchor("first", TENANT, &created.namespace_uuid);

    let report = installation.verify(&anchor, &installation.log_dir, Some(&installation.database));
    assert_eq!(report.verdict, Verdict::Verified, "{:?}", report.errors);
    assert_eq!(
        report.signatures_verified, 1,
        "the single record must be verified with the pinned key"
    );
    assert_eq!(report.entries_checked, 1);
    assert_eq!(report.verified_through_log_index, Some(1));
    let head = report.head.as_ref().unwrap();
    assert_eq!(head.statement_sha256, created.statement_sha256);
    assert_eq!(head.event_count, created.event_count);
    assert_eq!(head.head_hash, created.head_hash);
    assert_eq!(head.signing_key_id, installation.key_id);
    assert!(report.database.compared);
    assert_eq!(report.database.extends_checkpoint, Some(true));
    assert_eq!(report.database.events_after_checkpoint, Some(0));
    assert_eq!(report.database.head_hash, Some(created.head_hash.clone()));
    assert_eq!(report.scope.tenant_id, TENANT);
    assert_eq!(report.scope.namespace_uuid, created.namespace_uuid);
    assert_eq!(report.anchor.revision, 1);
    // No reference has been accepted yet: the report must say that a rollback
    // to an earlier signed state would go unnoticed.
    assert_eq!(
        report.rollback_detection,
        RollbackDetection::UnavailableNoReference
    );
    assert!(report.reference.is_none());
    assert!(!report.limits.is_empty());

    // Without a database the verdict covers the records only, and says so.
    let records_only = installation.verify(&anchor, &installation.log_dir, None);
    assert_eq!(records_only.verdict, Verdict::Verified);
    assert!(!records_only.database.compared);
    assert_eq!(records_only.database.event_count, None);
    assert_eq!(records_only.database.extends_checkpoint, None);
    assert!(
        records_only
            .limits
            .iter()
            .any(|limit| limit.contains("not compared"))
    );

    // Events appended after the checkpoint are reported as an unsigned tail.
    let store = open_store(&installation.database, TENANT);
    store.append(note("after")).unwrap();
    drop(store);
    let tail = installation.verify(&anchor, &installation.log_dir, Some(&installation.database));
    assert_eq!(tail.verdict, Verdict::Verified, "{:?}", tail.errors);
    assert_eq!(tail.database.events_after_checkpoint, Some(1));
}

#[test]
fn modified_statement_or_signature_is_refused() {
    let installation = Installation::new(TENANT);
    let created = installation.create().unwrap();
    let anchor = installation.anchor("first", TENANT, &created.namespace_uuid);
    let original = std::fs::read_to_string(&record_files(&installation.log_dir)[0]).unwrap();
    let pristine = SignedRecord::parse(original.as_bytes()).unwrap();
    let signature = pristine.signatures()[0].signature.clone();

    // A different signature of the right shape under the same file name.
    let replacement = if signature.starts_with('A') { "B" } else { "A" };
    let forged_signature = original.replacen(
        &format!("\"signature\":\"{}", &signature[..1]),
        &format!("\"signature\":\"{replacement}"),
        1,
    );
    assert_ne!(forged_signature, original);
    let forged_log = private_child(installation.root(), "forged-signature");
    write_private(
        &forged_log.join(pristine.file_name()),
        forged_signature.as_bytes(),
    );
    let report = installation.verify(&anchor, &forged_log, Some(&installation.database));
    assert_refused(&report, "signature_invalid");
    assert_eq!(report.signatures_verified, 0);
    assert_eq!(report.verified_through_log_index, None);

    // A modified statement, renamed after its new hash so that it stays a
    // canonical, self-consistent record: only the signature can refuse it.
    let Statement::Checkpoint(statement) = pristine.statement() else {
        panic!("the first record is a checkpoint");
    };
    let moved_head = original.replacen(
        &format!("\"event_count\":{},", statement.event_count),
        &format!("\"event_count\":{},", statement.event_count + 1),
        1,
    );
    assert_ne!(moved_head, original);
    let forged_log = private_child(installation.root(), "forged-statement");
    write_private(&forged_log.join(pristine.file_name()), original.as_bytes());
    replace_record_consistently(&forged_log, &moved_head);
    let forged = SignedRecord::parse(moved_head.as_bytes()).unwrap();
    assert_ne!(forged.statement_sha256(), pristine.statement_sha256());
    let report = installation.verify(&anchor, &forged_log, None);
    assert_refused(&report, "signature_invalid");

    // The same content in a non-canonical byte form is not a record at all.
    let spaced = original.replacen("\"statement\":{", "\"statement\":{ ", 1);
    assert_eq!(
        SignedRecord::parse(spaced.as_bytes()).unwrap_err().code(),
        "record_not_canonical"
    );
}

#[test]
fn key_supplied_by_the_package_is_not_a_trust_root() {
    let installation = Installation::new(TENANT);
    let created = installation.create().unwrap();
    let anchor = installation.anchor("first", TENANT, &created.namespace_uuid);

    // Another key holder signs the very same database head into its own,
    // perfectly self-consistent log.
    let other_key_dir = installation.root().join("keys").join("other");
    let other_key = init_key(&other_key_dir).unwrap();
    assert_ne!(other_key.key_id, installation.key_id);
    let other_log = installation.root().join("other-checkpoints");
    let other = create_checkpoint(&CreateRequest {
        data: &installation.database,
        tenant_id: TENANT,
        key_dir: &other_key_dir,
        checkpoint_dir: &other_log,
        new_log: true,
        previous_record: None,
    })
    .unwrap();
    assert_eq!(other.head_hash, created.head_hash);

    let report = installation.verify(&anchor, &other_log, Some(&installation.database));
    assert_refused(&report, "untrusted_key");
    assert_eq!(
        report.errors[0]
            .detail
            .get("key_id")
            .and_then(|id| id.as_str()),
        Some(other_key.key_id.as_str())
    );
    assert_eq!(report.signatures_verified, 0);

    // A record cannot carry its own key: the envelope has no place for one.
    let original = std::fs::read_to_string(&record_files(&other_log)[0]).unwrap();
    let with_key = original.replacen(
        "{\"format\":",
        &format!("{{\"public_key\":\"{}\",\"format\":", other_key.public_key),
        1,
    );
    assert_eq!(
        SignedRecord::parse(with_key.as_bytes()).unwrap_err().code(),
        "record_malformed"
    );

    // The anchor is created only for the fingerprint the holder expects.
    let public_key = installation.holder.join("other.public-key.json");
    show_key(&other_key_dir, Some(&public_key)).unwrap();
    let refused = init_anchor(&AnchorInitRequest {
        public_key: &public_key,
        expect_key_id: &installation.key_id,
        tenant_id: TENANT,
        namespace_uuid: &created.namespace_uuid,
        out: &installation.holder.join("substituted.anchor.json"),
    })
    .unwrap_err();
    assert_eq!(refused.code(), "key_id_mismatch");
    assert_eq!(refused.exit_code(), 2);
    assert!(!installation.holder.join("substituted.anchor.json").exists());
}

#[test]
fn wrong_tenant_or_namespace_is_refused() {
    let installation = Installation::new(TENANT);
    let created = installation.create().unwrap();

    // Anchor of another tenant, then of another namespace.
    let other_tenant = installation.anchor("other-tenant", "tenant-b", &created.namespace_uuid);
    let report = installation.verify(&other_tenant, &installation.log_dir, None);
    assert_refused(&report, "scope_mismatch");
    assert_eq!(report.errors[0].detail["field"], "tenant_id");
    let other_namespace = installation.anchor(
        "other-namespace",
        TENANT,
        "0b0e7c57-6c5f-4a3e-9d58-3f1f0d6b2a11",
    );
    let report = installation.verify(&other_namespace, &installation.log_dir, None);
    assert_refused(&report, "scope_mismatch");
    assert_eq!(report.errors[0].detail["field"], "namespace_uuid");

    // A database recreated for the same tenant is a new control namespace.
    let recreated_dir = private_child(installation.root(), "recreated");
    let recreated = recreated_dir.join("audit.sqlite");
    let store = open_store(&recreated, TENANT);
    store.append(note("alpha")).unwrap();
    drop(store);
    let anchor = installation.anchor("first", TENANT, &created.namespace_uuid);
    let report = installation.verify(&anchor, &installation.log_dir, Some(&recreated));
    assert_refused(&report, "scope_mismatch");
    assert_eq!(report.errors[0].detail["field"], "namespace_uuid");
    assert_eq!(
        report.errors[0].detail["between"],
        serde_json::json!(["trust_anchor", "database"])
    );

    // Signing that other database into the existing log is refused, and the
    // log is left untouched.
    let before = record_files(&installation.log_dir);
    let refused = installation
        .create_for(TENANT, &recreated, &installation.log_dir)
        .unwrap_err();
    assert_eq!(refused.code(), "scope_mismatch");
    assert_eq!(refused.exit_code(), 2);
    assert_eq!(refused.error.detail["field"], "namespace_uuid");
    assert_eq!(record_files(&installation.log_dir), before);

    // The tenant comes from the binding of the database, not from the request.
    let unrelated_log = installation.root().join("unrelated-checkpoints");
    let refused = installation
        .create_for("tenant-b", &installation.database, &unrelated_log)
        .unwrap_err();
    assert_eq!(refused.code(), "scope_mismatch");
    assert_eq!(refused.error.detail["field"], "tenant_id");
    assert!(
        !unrelated_log.exists(),
        "a refused start must not leave a checkpoint directory behind"
    );

    // Records and keys never share the directory of the audit database.
    let control = installation.database.parent().unwrap();
    let refused = installation
        .create_for(TENANT, &installation.database, control)
        .unwrap_err();
    assert_eq!(refused.code(), "invalid_argument");
    assert_eq!(refused.exit_code(), 1);
    assert!(record_files(control).is_empty());
}

#[test]
fn nv01_coherent_offline_rewrite_after_signature_is_refused() {
    let installation = Installation::new(TENANT);
    let created = installation.create().unwrap();
    let anchor = installation.anchor("first", TENANT, &created.namespace_uuid);
    assert_eq!(last_note(&installation.database), "alpha");

    // A distinct copy of the untouched database, kept for the negative control.
    let wal = installation.database.with_extension("sqlite-wal");
    assert!(std::fs::metadata(&wal).map_or(true, |facts| facts.len() == 0));
    let copy_dir = private_child(installation.root(), "negative-control");
    let copy = copy_dir.join("audit.sqlite");
    std::fs::copy(&installation.database, &copy).unwrap();

    // The coherent rewrite: same length, public digest recomputed, every
    // projection, quota, genesis and schema object left consistent.
    rewrite_last_note(&installation.database, true);
    assert_eq!(last_note(&installation.database), "bravo");

    // Characterization of the known residual of NV-01, not a desired
    // behaviour: the unkeyed reader used by the gateway and by verify-audit
    // still declares the rewritten history intact after a reopen.
    let reopened = Store::open_existing_for_tenant(
        &installation.database,
        TENANT,
        Redactor::new(vec![]),
        10_000,
    )
    .unwrap();
    let integrity = reopened.integrity().unwrap();
    assert!(integrity.valid, "{:?}", integrity.error);
    assert_eq!(integrity.event_count, created.event_count);
    assert_ne!(integrity.head_hash, created.head_hash);
    drop(reopened);

    // The signed checkpoint, verified with the independently held anchor,
    // refuses the continuity.
    let report = installation.verify(&anchor, &installation.log_dir, Some(&installation.database));
    assert_refused(&report, "database_diverged");
    assert_eq!(report.signatures_verified, 1);
    assert_eq!(report.database.extends_checkpoint, Some(false));
    assert_eq!(
        report.errors[0].detail["signed_head_hash"],
        serde_json::json!(created.head_hash)
    );
    assert_eq!(
        report.errors[0].detail["database_hash_at_position"],
        serde_json::json!(integrity.head_hash)
    );

    // Negative control: without recomputing the public digest, the existing
    // integrity reader already refuses the rewrite, and so does the tool.
    rewrite_last_note(&copy, false);
    let reopened =
        Store::open_existing_for_tenant(&copy, TENANT, Redactor::new(vec![]), 10_000).unwrap();
    assert!(!reopened.integrity().unwrap().valid);
    drop(reopened);
    let report = installation.verify(&anchor, &installation.log_dir, Some(&copy));
    assert_refused(&report, "audit_chain_invalid");
    assert!(!report.database.compared);
}

#[test]
fn checkpoint_creation_is_read_only_and_marks_existing_history_as_baseline() {
    let installation = Installation::new(TENANT);
    let store = open_store(&installation.database, TENANT);
    for text in ["one", "two", "three"] {
        store.append(note(text)).unwrap();
    }
    let before = store.integrity().unwrap();
    drop(store);
    let main_file = std::fs::read(&installation.database).unwrap();

    let first = installation.create().unwrap();
    assert!(
        first.baseline,
        "the first checkpoint of an existing history is a baseline"
    );
    assert_eq!(first.log_index, 1);
    assert_eq!(first.event_count, before.event_count);
    assert_eq!(first.head_hash, before.head_hash);
    assert_eq!(
        std::fs::read(&installation.database).unwrap(),
        main_file,
        "creating a checkpoint must not write the audit database"
    );
    let record =
        SignedRecord::parse(&std::fs::read(&record_files(&installation.log_dir)[0]).unwrap())
            .unwrap();
    let Statement::Checkpoint(statement) = record.statement() else {
        panic!("the first record is a checkpoint");
    };
    assert_eq!(statement.previous_event_count, 0);
    assert_eq!(statement.previous_head_hash, "0".repeat(64));
    assert_eq!(statement.previous_statement_sha256, "0".repeat(64));
    assert_eq!(statement.event_count, before.event_count);
    assert_eq!(statement.tenant_id, TENANT);
    assert_eq!(statement.namespace_uuid, first.namespace_uuid);
    assert_eq!(statement.control_genesis_seq, 1);
    assert_eq!(
        std::fs::metadata(&first.record)
            .unwrap()
            .permissions()
            .mode()
            & 0o7777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(&installation.log_dir)
            .unwrap()
            .permissions()
            .mode()
            & 0o7777,
        0o700
    );

    // The history is neither extended nor rewritten by the checkpoint.
    let store = open_store(&installation.database, TENANT);
    let after = store.integrity().unwrap();
    assert!(after.valid);
    assert_eq!(after.event_count, before.event_count);
    assert_eq!(after.head_hash, before.head_hash);

    // A later checkpoint links to the first one and is no longer a baseline.
    store.append(note("four")).unwrap();
    drop(store);
    let second = installation.create().unwrap();
    assert!(!second.baseline);
    assert_eq!(second.log_index, 2);
    assert_eq!(second.event_count, before.event_count + 1);
    let records = record_files(&installation.log_dir);
    assert_eq!(records.len(), 2);
    let record = SignedRecord::parse(&std::fs::read(&records[1]).unwrap()).unwrap();
    let Statement::Checkpoint(statement) = record.statement() else {
        panic!("the second record is a checkpoint");
    };
    assert_eq!(statement.previous_statement_sha256, first.statement_sha256);
    assert_eq!(statement.previous_event_count, first.event_count);
    assert_eq!(statement.previous_head_hash, first.head_hash);
}

#[test]
fn create_and_verify_work_while_a_live_store_holds_the_database() {
    let installation = Installation::new(TENANT);
    // The gateway keeps its Store open: WAL and shared-memory files exist.
    let live = open_store(&installation.database, TENANT);
    live.append(note("live-1")).unwrap();
    assert!(installation.database.with_extension("sqlite-wal").exists());

    let first = installation.create().unwrap();
    assert_eq!(first.event_count, 3);
    let anchor = installation.anchor("first", TENANT, &first.namespace_uuid);
    live.append(note("live-2")).unwrap();
    live.append(note("live-3")).unwrap();
    let report = installation.verify(&anchor, &installation.log_dir, Some(&installation.database));
    assert_eq!(report.verdict, Verdict::Verified, "{:?}", report.errors);
    assert_eq!(report.database.events_after_checkpoint, Some(2));

    let second = installation.create().unwrap();
    assert_eq!(second.log_index, 2);
    assert_eq!(second.event_count, 5);
    let report = installation.verify(&anchor, &installation.log_dir, Some(&installation.database));
    assert_eq!(report.verdict, Verdict::Verified, "{:?}", report.errors);
    assert_eq!(report.entries_checked, 2);
    assert_eq!(report.signatures_verified, 2);
    assert_eq!(report.verified_through_log_index, Some(2));
    assert_eq!(report.database.events_after_checkpoint, Some(0));

    // The live Store is neither blocked nor invalidated by the tool.
    live.append(note("live-4")).unwrap();
    let integrity = live.integrity().unwrap();
    assert!(integrity.valid, "{:?}", integrity.error);
    assert_eq!(integrity.event_count, 6);
    assert!(live.healthy());
}

// ---------------------------------------------------------------------------
// Independent reference, continuity, rollback and resumption
// ---------------------------------------------------------------------------

/// Copy the closed audit database into a new private directory.
fn copy_database(installation: &Installation, name: &str) -> PathBuf {
    let wal = installation.database.with_extension("sqlite-wal");
    assert!(
        std::fs::metadata(&wal).map_or(true, |facts| facts.len() == 0),
        "the database must be closed and checkpointed before it is copied"
    );
    let directory = private_child(installation.root(), name);
    let copy = directory.join("audit.sqlite");
    std::fs::copy(&installation.database, &copy).unwrap();
    copy
}

/// Copy the record files of a checkpoint directory into a new private one.
fn copy_log(installation: &Installation, source: &Path, name: &str) -> PathBuf {
    let directory = private_child(installation.root(), name);
    for record in record_files(source) {
        std::fs::copy(&record, directory.join(record.file_name().unwrap())).unwrap();
    }
    directory
}

fn append_notes(installation: &Installation, notes: &[&str]) {
    let store = open_store(&installation.database, TENANT);
    for text in notes {
        store.append(note(text)).unwrap();
    }
}

/// Remove the last event coherently, as a truncation that keeps the remaining
/// chain, projections and byte quota valid.
fn truncate_last_event(database: &Path) {
    let db = rusqlite::Connection::open(database).unwrap();
    db.execute_batch(
        "DELETE FROM events WHERE seq=(SELECT MAX(seq) FROM events);
         UPDATE quota SET bytes=(SELECT COALESCE(SUM(length(CAST(payload AS BLOB))),0) FROM events) WHERE id=1;",
    )
    .unwrap();
}

fn advance(
    anchor: &Path,
    log_dir: &Path,
    database: Option<&Path>,
    out: &Path,
) -> Result<AdvanceOutcome, Failure> {
    advance_anchor(&AnchorAdvanceRequest {
        anchor,
        source: RecordSource::Directory(log_dir),
        data: database,
        out,
    })
}

/// Two checkpoints, with a backup of the whole state taken after the first,
/// and an anchor whose reference is the second.
struct History {
    installation: Installation,
    first: CreateOutcome,
    second: CreateOutcome,
    /// Database and log as they were right after the first checkpoint.
    old_database: PathBuf,
    old_log: PathBuf,
    /// Anchor without reference, then with the second record as reference.
    initial_anchor: PathBuf,
    anchor: PathBuf,
}

impl History {
    fn new() -> Self {
        let installation = Installation::new(TENANT);
        let first = installation.create().unwrap();
        let old_database = copy_database(&installation, "backup-database");
        let old_log = copy_log(&installation, &installation.log_dir, "backup-checkpoints");
        append_notes(&installation, &["beta", "gamma"]);
        let second = installation.create().unwrap();
        assert_eq!(second.log_index, 2);
        let initial_anchor = installation.anchor("history", TENANT, &first.namespace_uuid);
        let anchor = installation.holder.join("history.anchor-2.json");
        let advanced = advance(
            &initial_anchor,
            &installation.log_dir,
            Some(&installation.database),
            &anchor,
        )
        .unwrap();
        assert_eq!(
            advanced.status,
            AdvanceStatus::Advanced,
            "{:?}",
            advanced.verification.errors
        );
        Self {
            installation,
            first,
            second,
            old_database,
            old_log,
            initial_anchor,
            anchor,
        }
    }
}

#[test]
fn rollback_of_log_and_database_is_detected_against_independent_reference() {
    let history = History::new();
    let installation = &history.installation;

    // The storage administrator restores the older database and the older log:
    // both are perfectly self-consistent and correctly signed.
    let report = installation.verify(
        &history.anchor,
        &history.old_log,
        Some(&history.old_database),
    );
    assert_eq!(
        report.error_codes(),
        vec!["log_rolled_back", "database_truncated"],
        "{:?}",
        report.errors
    );
    assert_eq!(report.verdict, Verdict::Refused);
    assert_eq!(
        report.rollback_detection,
        RollbackDetection::AnchorReference
    );
    assert_eq!(report.errors[0].detail["reference_log_index"], 2);
    assert_eq!(report.errors[0].detail["log_head_index"], 1);
    assert_eq!(
        report.errors[1].detail["signed_event_count"],
        serde_json::json!(history.second.event_count)
    );
    assert_eq!(
        report.errors[1].detail["database_event_count"],
        serde_json::json!(history.first.event_count)
    );
    // What stays trusted is the accepted reference, not the rolled-back head.
    assert_eq!(report.verified_through_log_index, Some(2));
    assert_eq!(report.entries_checked, 0);

    // Each half of the rollback is detected on its own.
    let report = installation.verify(&history.anchor, &history.old_log, None);
    assert_refused(&report, "log_rolled_back");
    let report = installation.verify(
        &history.anchor,
        &installation.log_dir,
        Some(&history.old_database),
    );
    assert_refused(&report, "database_truncated");

    // An emptied checkpoint directory is a rollback too.
    let empty = private_child(installation.root(), "emptied-checkpoints");
    let report = installation.verify(&history.anchor, &empty, None);
    assert_refused(&report, "log_rolled_back");
    assert_eq!(
        report.errors[0].detail["log_head_index"],
        serde_json::Value::Null
    );

    // The current state still verifies against the same reference.
    let report = installation.verify(
        &history.anchor,
        &installation.log_dir,
        Some(&installation.database),
    );
    assert_eq!(report.verdict, Verdict::Verified, "{:?}", report.errors);
    assert_eq!(
        report
            .reference
            .as_ref()
            .map(|reference| reference.log_index),
        Some(2)
    );
}

#[test]
fn truncation_and_divergence_are_distinguished() {
    let history = History::new();
    let installation = &history.installation;

    // Another complete, correctly signed record at the reference position:
    // the log was replaced, not merely shortened.
    let replaced_log = copy_log(installation, &history.old_log, "replaced-checkpoints");
    let other_second = plant_record(
        &replaced_log,
        &forge_checkpoint(
            installation,
            &installation.key_dir,
            &read_record(&history.first.record),
        ),
    );
    assert_eq!(other_second.log_index(), 2);
    assert_eq!(
        other_second.statement().signed_position().head_hash,
        history.second.head_hash
    );
    assert_ne!(
        other_second.statement_sha256(),
        history.second.statement_sha256
    );
    let report = installation.verify(&history.anchor, &replaced_log, None);
    assert_refused(&report, "log_diverged_from_reference");
    assert_eq!(
        report.errors[0].detail["reference_statement_sha256"],
        serde_json::json!(history.second.statement_sha256)
    );

    // A third record, then a log that misses the second one, and a log whose
    // third record links to another second record.
    append_notes(installation, &["delta"]);
    let third = installation.create().unwrap();
    assert_eq!(third.log_index, 3);
    let records = record_files(&installation.log_dir);
    assert_eq!(records.len(), 3);
    let gap_log = private_child(installation.root(), "gap-checkpoints");
    for record in [&records[0], &records[2]] {
        std::fs::copy(record, gap_log.join(record.file_name().unwrap())).unwrap();
    }
    let report = installation.verify(&history.initial_anchor, &gap_log, None);
    assert_refused(&report, "log_discontinuous");
    assert_eq!(report.errors[0].detail["missing_log_index"], 2);
    assert_eq!(report.verified_through_log_index, Some(1));
    std::fs::copy(
        &records[2],
        replaced_log.join(records[2].file_name().unwrap()),
    )
    .unwrap();
    let report = installation.verify(&history.initial_anchor, &replaced_log, None);
    assert_refused(&report, "log_discontinuous");
    assert_eq!(report.errors[0].detail["log_index"], 3);
    assert_eq!(report.verified_through_log_index, Some(2));

    // The database is shortened coherently below the signed position...
    let truncated = copy_database(installation, "truncated-database");
    truncate_last_event(&truncated);
    let reopened =
        Store::open_existing_for_tenant(&truncated, TENANT, Redactor::new(vec![]), 10_000).unwrap();
    assert!(reopened.integrity().unwrap().valid);
    drop(reopened);
    let report = installation.verify(&history.anchor, &installation.log_dir, Some(&truncated));
    assert_refused(&report, "database_truncated");
    assert_eq!(report.database.extends_checkpoint, Some(false));

    // ...or rewritten coherently at the signed position.
    let diverged = copy_database(installation, "diverged-database");
    let db = rusqlite::Connection::open(&diverged).unwrap();
    let (seq, received_at, previous, payload): (i64, String, String, String) = db
        .query_row(
            "SELECT seq,received_at,prev_hash,payload FROM events ORDER BY seq DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    let rewritten = payload.replace("\"note\":\"delta\"", "\"note\":\"DELTA\"");
    assert_ne!(rewritten, payload);
    db.execute(
        "UPDATE events SET payload=?1,hash=?2 WHERE seq=?3",
        rusqlite::params![
            rewritten,
            chain_digest(seq, &received_at, &previous, &rewritten),
            seq
        ],
    )
    .unwrap();
    drop(db);
    let report = installation.verify(&history.anchor, &installation.log_dir, Some(&diverged));
    assert_refused(&report, "database_diverged");
    assert_eq!(report.signatures_verified, 1);
    assert_eq!(report.verified_through_log_index, Some(3));
}

#[test]
fn without_reference_rollback_detection_is_reported_unavailable() {
    let history = History::new();
    let installation = &history.installation;

    // With an anchor that never accepted a reference, the rolled-back state
    // verifies: this is the limit the report must state, not hide.
    let report = installation.verify(
        &history.initial_anchor,
        &history.old_log,
        Some(&history.old_database),
    );
    assert_eq!(report.verdict, Verdict::Verified, "{:?}", report.errors);
    assert_eq!(
        report.rollback_detection,
        RollbackDetection::UnavailableNoReference
    );
    assert!(report.reference.is_none());
    assert!(
        report
            .limits
            .iter()
            .any(|limit| limit.contains("cannot be detected")),
        "{:?}",
        report.limits
    );

    // A caller that needs rollback detection asks for it and is refused.
    let strict = verify(&VerifyRequest {
        anchor: &history.initial_anchor,
        source: RecordSource::Directory(&history.old_log),
        data: Some(&history.old_database),
        require_reference: true,
        report_out: None,
    })
    .unwrap();
    assert_refused(&strict, "no_independent_reference");

    // With the reference, the same request passes on the current state and
    // the report names the detection it relies on.
    let strict = verify(&VerifyRequest {
        anchor: &history.anchor,
        source: RecordSource::Directory(&installation.log_dir),
        data: Some(&installation.database),
        require_reference: true,
        report_out: None,
    })
    .unwrap();
    assert_eq!(strict.verdict, Verdict::Verified, "{:?}", strict.errors);
    assert_eq!(
        strict.rollback_detection,
        RollbackDetection::AnchorReference
    );
    assert!(
        strict
            .limits
            .iter()
            .any(|limit| limit.contains("at and below the anchor reference only"))
    );
}

#[test]
fn anchor_advance_is_monotonic_and_never_overwrites() {
    let installation = Installation::new(TENANT);
    let first = installation.create().unwrap();
    let initial = installation.anchor("advance", TENANT, &first.namespace_uuid);
    let initial_bytes = std::fs::read(&initial).unwrap();

    let second_revision = installation.holder.join("advance.anchor-2.json");
    let advanced = advance(
        &initial,
        &installation.log_dir,
        Some(&installation.database),
        &second_revision,
    )
    .unwrap();
    assert_eq!(advanced.status, AdvanceStatus::Advanced);
    let written = advanced.anchor.unwrap();
    assert_eq!(written.revision, 2);
    assert_eq!(
        written.previous_anchor_sha256,
        Some(sha256_hex(&initial_bytes))
    );
    let reference = written.reference.unwrap();
    assert_eq!(reference.log_index, 1);
    assert_eq!(reference.statement_sha256, first.statement_sha256);
    assert_eq!(reference.checkpoint.log_index, 1);
    assert_eq!(reference.checkpoint.event_count, first.event_count);
    assert_eq!(reference.checkpoint.head_hash, first.head_hash);
    assert_eq!(
        written.sha256,
        sha256_hex(&std::fs::read(&second_revision).unwrap())
    );
    assert_eq!(
        std::fs::metadata(&second_revision)
            .unwrap()
            .permissions()
            .mode()
            & 0o7777,
        0o600
    );
    assert_eq!(std::fs::read(&initial).unwrap(), initial_bytes);

    // Nothing new: no revision is written.
    let idle = installation.holder.join("advance.anchor-idle.json");
    let unchanged = advance(&second_revision, &installation.log_dir, None, &idle).unwrap();
    assert_eq!(unchanged.status, AdvanceStatus::Unchanged);
    assert!(unchanged.anchor.is_none());
    assert!(!idle.exists());

    // A new checkpoint moves the reference forward in a third revision.
    append_notes(&installation, &["beta"]);
    let second = installation.create().unwrap();
    let third_revision = installation.holder.join("advance.anchor-3.json");
    let advanced = advance(
        &second_revision,
        &installation.log_dir,
        None,
        &third_revision,
    )
    .unwrap();
    assert_eq!(advanced.status, AdvanceStatus::Advanced);
    let written = advanced.anchor.unwrap();
    assert_eq!(written.revision, 3);
    assert_eq!(
        written.reference.unwrap().statement_sha256,
        second.statement_sha256
    );

    // An existing destination is refused before anything else, untouched.
    let second_bytes = std::fs::read(&second_revision).unwrap();
    let refused = advance(
        &third_revision,
        &installation.log_dir,
        None,
        &second_revision,
    )
    .unwrap_err();
    assert_eq!(refused.code(), "output_exists");
    assert_eq!(refused.exit_code(), 1);
    assert_eq!(std::fs::read(&second_revision).unwrap(), second_bytes);

    // The reference never moves backwards: a log that ends before it is a
    // refused verification and no revision is written.
    let short_log = private_child(installation.root(), "short-checkpoints");
    let records = record_files(&installation.log_dir);
    std::fs::copy(&records[0], short_log.join(records[0].file_name().unwrap())).unwrap();
    let backwards = installation.holder.join("advance.anchor-backwards.json");
    let refused = advance(&third_revision, &short_log, None, &backwards).unwrap();
    assert_eq!(refused.status, AdvanceStatus::Refused);
    assert_eq!(refused.verification.error_codes(), vec!["log_rolled_back"]);
    assert!(refused.anchor.is_none());
    assert!(!backwards.exists());
}

fn warned(warnings: &[Warning], code: &str) -> bool {
    warnings.iter().any(|warning| warning.code == code)
}

/// The holder of the signing key rewrites the last event coherently, lets the
/// audit grow, then signs the new head with the key file. The forged record
/// links to `previous` and recalls its signed position honestly: only a
/// comparison of the database with that earlier position can refuse it.
fn rewrite_under_and_sign_past(
    installation: &Installation,
    previous: &SignedRecord,
) -> SignedRecord {
    rewrite_last_note(&installation.database, true);
    append_notes(installation, &["later"]);
    let reopened = open_store(&installation.database, TENANT);
    assert!(reopened.integrity().unwrap().valid);
    drop(reopened);
    let forged = plant_record(
        &installation.log_dir,
        &forge_checkpoint(installation, &installation.key_dir, previous),
    );
    assert_eq!(forged.log_index(), previous.log_index() + 1);
    forged
}

#[test]
fn history_under_the_anchor_reference_stays_compared_after_later_signed_records() {
    let installation = Installation::new(TENANT);
    let first = installation.create().unwrap();
    let initial = installation.anchor("under-reference", TENANT, &first.namespace_uuid);
    let anchor = installation.holder.join("under-reference.anchor-2.json");
    let advanced = advance(
        &initial,
        &installation.log_dir,
        Some(&installation.database),
        &anchor,
    )
    .unwrap();
    assert_eq!(advanced.status, AdvanceStatus::Advanced);
    let first_record = read_record(&first.record);

    // An event under the reference is rewritten, and a later record signed by
    // the pinned key recalls the referenced position as if nothing happened.
    rewrite_under_and_sign_past(&installation, &first_record);

    // The records alone verify: they only prove what the key signed, and the
    // report says that the audit history was not looked at.
    let records_only = installation.verify(&anchor, &installation.log_dir, None);
    assert_eq!(
        records_only.verdict,
        Verdict::Verified,
        "{:?}",
        records_only.errors
    );
    assert!(
        records_only
            .limits
            .iter()
            .any(|limit| limit.contains("says nothing about the audit history itself")),
        "{:?}",
        records_only.limits
    );

    // With the database, the position pinned by the anchor is still compared,
    // whatever was signed after it.
    let report = installation.verify(&anchor, &installation.log_dir, Some(&installation.database));
    assert_refused(&report, "database_diverged");
    let detail = &report.errors[0].detail;
    assert_eq!(detail["position_source"], "anchor_reference");
    assert_eq!(detail["signed_at_log_index"], 1);
    assert_eq!(detail["event_count"], serde_json::json!(first.event_count));
    assert_eq!(
        detail["signed_head_hash"],
        serde_json::json!(first.head_hash)
    );
    assert_eq!(report.database.extends_checkpoint, Some(false));
    assert_eq!(report.database.positions_compared, Some(1));
    // The forged record is correctly signed and linked: nothing but the
    // database comparison refuses it.
    assert_eq!(report.signatures_verified, 1);
    assert_eq!(report.verified_through_log_index, Some(2));

    // The forged head cannot become the next reference.
    let out = installation.holder.join("under-reference.anchor-3.json");
    let refused = advance(
        &anchor,
        &installation.log_dir,
        Some(&installation.database),
        &out,
    )
    .unwrap();
    assert_eq!(refused.status, AdvanceStatus::Refused);
    assert_eq!(
        refused.verification.error_codes(),
        vec!["database_diverged"]
    );
    assert!(!out.exists());
}

#[test]
fn advancing_without_a_database_retains_the_positions_pinned_so_far() {
    let installation = Installation::new(TENANT);
    let first = installation.create().unwrap();
    let initial = installation.anchor("retained", TENANT, &first.namespace_uuid);

    // An advance made with the database confirms the continuity: nothing is
    // retained and nothing is warned about.
    let second_revision = installation.holder.join("retained.anchor-2.json");
    let advanced = advance(
        &initial,
        &installation.log_dir,
        Some(&installation.database),
        &second_revision,
    )
    .unwrap();
    let written = advanced.anchor.unwrap();
    assert!(
        written
            .reference
            .as_ref()
            .unwrap()
            .retained_checkpoints
            .is_empty()
    );
    assert!(!warned(
        &written.warnings,
        "reference_advanced_without_database"
    ));
    let first_record = read_record(&first.record);
    let forged = rewrite_under_and_sign_past(&installation, &first_record);

    // An auditor who holds only the records advances the reference: the
    // records verify, so the reference moves.
    let third_revision = installation.holder.join("retained.anchor-3.json");
    let advanced = advance(
        &second_revision,
        &installation.log_dir,
        None,
        &third_revision,
    )
    .unwrap();
    assert_eq!(
        advanced.status,
        AdvanceStatus::Advanced,
        "{:?}",
        advanced.verification.errors
    );
    let written = advanced.anchor.unwrap();
    let reference = written.reference.unwrap();
    assert_eq!(reference.log_index, 2);
    assert_eq!(reference.statement_sha256, forged.statement_sha256());
    // The position pinned before is not dropped: no database confirmed that
    // the new head extends it.
    assert_eq!(
        reference.retained_checkpoints.len(),
        1,
        "the position pinned by the previous revision must be retained"
    );
    let retained = &reference.retained_checkpoints[0];
    assert_eq!(retained.log_index, 1);
    assert_eq!(retained.event_count, first.event_count);
    assert_eq!(retained.head_hash, first.head_hash);
    assert!(
        warned(&written.warnings, "reference_advanced_without_database"),
        "{:?}",
        written.warnings
    );

    // The new revision therefore still refuses the rewritten database...
    let report = installation.verify(
        &third_revision,
        &installation.log_dir,
        Some(&installation.database),
    );
    assert_refused(&report, "database_diverged");
    assert_eq!(
        report.errors[0].detail["position_source"],
        "anchor_retained"
    );
    assert_eq!(report.errors[0].detail["signed_at_log_index"], 1);
    // ...and an advance made with that database confirms nothing: it is
    // refused, and the retained position stays in the only revision there is.
    let laundered = installation.holder.join("retained.anchor-laundered.json");
    let refused = advance(
        &third_revision,
        &installation.log_dir,
        Some(&installation.database),
        &laundered,
    )
    .unwrap();
    assert_eq!(refused.status, AdvanceStatus::Refused);
    assert_eq!(
        refused.verification.error_codes(),
        vec!["database_diverged"]
    );
    assert!(refused.anchor.is_none());
    assert!(!laundered.exists());
    // ...says what it retains when no database is given...
    let records_only = installation.verify(&third_revision, &installation.log_dir, None);
    assert_eq!(records_only.verdict, Verdict::Verified);
    assert!(
        records_only
            .limits
            .iter()
            .any(|limit| limit.contains("retains 1 earlier position")),
        "{:?}",
        records_only.limits
    );
    // ...and so does the revision that was advanced with the database.
    let report = installation.verify(
        &second_revision,
        &installation.log_dir,
        Some(&installation.database),
    );
    assert_refused(&report, "database_diverged");
    assert_eq!(
        report.errors[0].detail["position_source"],
        "anchor_reference"
    );

    // On an honest installation, a later advance with the database confirms
    // the continuity and stops retaining.
    let honest = Installation::new(TENANT);
    let first = honest.create().unwrap();
    let initial = honest.anchor("honest", TENANT, &first.namespace_uuid);
    let mut revision = honest.holder.join("honest.anchor-2.json");
    let advanced = advance(&initial, &honest.log_dir, None, &revision).unwrap();
    let written = advanced.anchor.unwrap();
    // A first reference has nothing before it to retain, but is still
    // recorded on the word of the signer.
    assert!(written.reference.unwrap().retained_checkpoints.is_empty());
    assert!(warned(
        &written.warnings,
        "reference_advanced_without_database"
    ));
    for (number, with_database) in [(3, false), (4, false), (5, true)] {
        append_notes(&honest, &[&format!("note-{number}")]);
        honest.create().unwrap();
        let next = honest.holder.join(format!("honest.anchor-{number}.json"));
        let advanced = advance(
            &revision,
            &honest.log_dir,
            with_database.then_some(honest.database.as_path()),
            &next,
        )
        .unwrap();
        assert_eq!(advanced.status, AdvanceStatus::Advanced);
        let written = advanced.anchor.unwrap();
        let retained = written.reference.unwrap().retained_checkpoints;
        if with_database {
            assert_eq!(advanced.verification.database.positions_compared, Some(4));
            assert!(retained.is_empty(), "{retained:?}");
            assert!(!warned(
                &written.warnings,
                "reference_advanced_without_database"
            ));
        } else {
            assert_eq!(retained.len(), number - 2, "{retained:?}");
            assert_eq!(retained[0].event_count, first.event_count);
        }
        revision = next;
    }
    let report = honest.verify(&revision, &honest.log_dir, Some(&honest.database));
    assert_eq!(report.verdict, Verdict::Verified, "{:?}", report.errors);
    assert_eq!(report.database.positions_compared, Some(1));
}

/// The `status` member that `anchor advance` prints.
fn advance_status(outcome: &AdvanceOutcome) -> String {
    serde_json::to_value(outcome.status)
        .unwrap()
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn advance_with_a_database_confirms_retained_positions_without_a_new_record() {
    let installation = Installation::new(TENANT);
    let first = installation.create().unwrap();
    let initial = installation.anchor("confirm", TENANT, &first.namespace_uuid);

    // An auditor who holds only the records advances twice: the second
    // revision retains the position pinned by the first.
    let second_revision = installation.holder.join("confirm.anchor-2.json");
    assert_eq!(
        advance(&initial, &installation.log_dir, None, &second_revision)
            .unwrap()
            .status,
        AdvanceStatus::Advanced
    );
    append_notes(&installation, &["beta"]);
    let second = installation.create().unwrap();
    let third_revision = installation.holder.join("confirm.anchor-3.json");
    let advanced = advance(
        &second_revision,
        &installation.log_dir,
        None,
        &third_revision,
    )
    .unwrap();
    let retained = advanced
        .anchor
        .unwrap()
        .reference
        .unwrap()
        .retained_checkpoints;
    assert_eq!(retained.len(), 1, "{retained:?}");
    assert_eq!(retained[0].head_hash, first.head_hash);
    let third_bytes = std::fs::read(&third_revision).unwrap();
    let report = installation.verify(&third_revision, &installation.log_dir, None);
    assert!(
        report
            .limits
            .iter()
            .any(|limit| limit.contains("retains 1 earlier position(s)")),
        "{:?}",
        report.limits
    );

    // Still without a database, nothing new means nothing to write.
    let idle = installation.holder.join("confirm.anchor-idle.json");
    let unchanged = advance(&third_revision, &installation.log_dir, None, &idle).unwrap();
    assert_eq!(advance_status(&unchanged), "unchanged");
    assert!(unchanged.anchor.is_none());
    assert!(!idle.exists());

    // A database that does not hold every pinned position confirms nothing.
    let truncated = copy_database(&installation, "confirm-truncated");
    truncate_last_event(&truncated);
    let refused_out = installation.holder.join("confirm.anchor-refused.json");
    let refused = advance(
        &third_revision,
        &installation.log_dir,
        Some(&truncated),
        &refused_out,
    )
    .unwrap();
    assert_eq!(refused.status, AdvanceStatus::Refused);
    assert_eq!(
        refused.verification.error_codes(),
        vec!["database_truncated"]
    );
    assert!(!refused_out.exists());

    // The database is now at hand and no record was signed since. The advance
    // compares it with the retained position and with the reference, and
    // records that confirmation: a new revision with the same reference that
    // retains nothing.
    let fourth_revision = installation.holder.join("confirm.anchor-4.json");
    let confirmed = advance(
        &third_revision,
        &installation.log_dir,
        Some(&installation.database),
        &fourth_revision,
    )
    .unwrap();
    assert_eq!(
        confirmed.verification.verdict,
        Verdict::Verified,
        "{:?}",
        confirmed.verification.errors
    );
    assert_eq!(confirmed.verification.database.positions_compared, Some(2));
    assert_eq!(
        advance_status(&confirmed),
        "confirmed",
        "an advance made with the database must confirm the retained positions even when no record is new"
    );
    let written = confirmed
        .anchor
        .expect("the confirmation is recorded in a new revision");
    assert_eq!(written.status, "confirmed");
    assert_eq!(written.revision, 4);
    assert_eq!(
        written.previous_anchor_sha256,
        Some(sha256_hex(&third_bytes))
    );
    let reference = written.reference.unwrap();
    assert_eq!(reference.log_index, 2);
    assert_eq!(reference.statement_sha256, second.statement_sha256);
    assert_eq!(reference.checkpoint.log_index, 2);
    assert_eq!(reference.checkpoint.event_count, second.event_count);
    assert_eq!(reference.checkpoint.head_hash, second.head_hash);
    assert!(
        reference.retained_checkpoints.is_empty(),
        "{:?}",
        reference.retained_checkpoints
    );
    assert!(!warned(
        &written.warnings,
        "reference_advanced_without_database"
    ));
    assert_eq!(
        written.sha256,
        sha256_hex(&std::fs::read(&fourth_revision).unwrap())
    );
    assert_eq!(
        std::fs::metadata(&fourth_revision)
            .unwrap()
            .permissions()
            .mode()
            & 0o7777,
        0o600
    );
    // The revision that retained the position is kept, byte for byte.
    assert_eq!(std::fs::read(&third_revision).unwrap(), third_bytes);

    // The confirmed revision has nothing left to confirm, and says so.
    let idle = installation.holder.join("confirm.anchor-idle-2.json");
    let unchanged = advance(
        &fourth_revision,
        &installation.log_dir,
        Some(&installation.database),
        &idle,
    )
    .unwrap();
    assert_eq!(advance_status(&unchanged), "unchanged");
    assert!(unchanged.anchor.is_none());
    assert!(!idle.exists());
    let report = installation.verify(
        &fourth_revision,
        &installation.log_dir,
        Some(&installation.database),
    );
    assert_eq!(report.verdict, Verdict::Verified, "{:?}", report.errors);
    assert_eq!(report.database.positions_compared, Some(1));
    assert!(
        !report
            .limits
            .iter()
            .any(|limit| limit.contains("earlier position(s)")),
        "{:?}",
        report.limits
    );
}

#[test]
fn reference_stops_advancing_without_a_database_at_the_retention_bound() {
    let installation = Installation::new(TENANT);
    let first = installation.create().unwrap();
    let initial = installation.anchor("bound", TENANT, &first.namespace_uuid);
    let mut revision = installation.holder.join("bound.anchor-0.json");
    assert_eq!(
        advance(&initial, &installation.log_dir, None, &revision)
            .unwrap()
            .status,
        AdvanceStatus::Advanced
    );
    for number in 1..=MAX_RETAINED_CHECKPOINTS {
        append_notes(&installation, &[&format!("note-{number}")]);
        installation.create().unwrap();
        let next = installation
            .holder
            .join(format!("bound.anchor-{number}.json"));
        let advanced = advance(&revision, &installation.log_dir, None, &next).unwrap();
        assert_eq!(advanced.status, AdvanceStatus::Advanced);
        assert_eq!(
            advanced
                .anchor
                .unwrap()
                .reference
                .unwrap()
                .retained_checkpoints
                .len(),
            number
        );
        revision = next;
    }

    // One more position cannot be retained: the holder must bring a database.
    append_notes(&installation, &["one-too-many"]);
    installation.create().unwrap();
    let refused_out = installation.holder.join("bound.anchor-refused.json");
    let result = advance(&revision, &installation.log_dir, None, &refused_out);
    let refused = result.expect_err("the retention bound must stop an advance without a database");
    assert_eq!(refused.code(), "database_comparison_required");
    assert_eq!(refused.exit_code(), 1);
    assert!(refused.error.action.contains("--data"));
    assert!(!refused_out.exists());

    let consolidated = installation.holder.join("bound.anchor-consolidated.json");
    let advanced = advance(
        &revision,
        &installation.log_dir,
        Some(&installation.database),
        &consolidated,
    )
    .unwrap();
    assert_eq!(advanced.status, AdvanceStatus::Advanced);
    assert_eq!(
        advanced.verification.database.positions_compared,
        Some(u64::try_from(MAX_RETAINED_CHECKPOINTS).unwrap() + 2)
    );
    assert!(
        advanced
            .anchor
            .unwrap()
            .reference
            .unwrap()
            .retained_checkpoints
            .is_empty()
    );
}

#[test]
fn create_refuses_to_sign_a_regressed_or_diverged_audit() {
    let installation = Installation::new(TENANT);
    installation.create().unwrap();
    append_notes(&installation, &["alpha"]);
    let second = installation.create().unwrap();
    let before = record_files(&installation.log_dir);
    assert_eq!(before.len(), 2);

    // The last signed event is rewritten coherently, then more events arrive.
    let diverged = copy_database(&installation, "diverged-database");
    rewrite_last_note(&diverged, true);
    let store = open_store(&diverged, TENANT);
    store.append(note("later")).unwrap();
    assert!(store.integrity().unwrap().valid);
    drop(store);
    let result = installation.create_for(TENANT, &diverged, &installation.log_dir);
    let refused = result.expect_err("a diverged audit must not be signed");
    assert_eq!(refused.code(), "database_diverged");
    assert_eq!(refused.exit_code(), 2);
    assert_eq!(
        refused.error.detail["signed_head_hash"],
        serde_json::json!(second.head_hash)
    );
    assert_eq!(record_files(&installation.log_dir), before);

    // The database is shorter than the last signed position.
    let truncated = copy_database(&installation, "truncated-database");
    truncate_last_event(&truncated);
    let result = installation.create_for(TENANT, &truncated, &installation.log_dir);
    let refused = result.expect_err("a truncated audit must not be signed");
    assert_eq!(refused.code(), "database_truncated");
    assert_eq!(refused.exit_code(), 2);
    assert_eq!(record_files(&installation.log_dir), before);

    // The untouched database is still accepted.
    append_notes(&installation, &["beta"]);
    assert_eq!(installation.create().unwrap().log_index, 3);
}

#[test]
fn interrupted_record_write_is_reported_and_creation_resumes() {
    let installation = Installation::new(TENANT);
    let first = installation.create().unwrap();
    let anchor = installation.anchor("resume", TENANT, &first.namespace_uuid);
    let first_bytes = std::fs::read(&first.record).unwrap();

    // An interrupted writer left a truncated record and an empty file at the
    // next log index.
    let truncated = installation
        .log_dir
        .join(format!("cp-{:020}-{}.json", 2, "a".repeat(64)));
    write_private(&truncated, &first_bytes[..first_bytes.len() / 2]);
    let empty = installation
        .log_dir
        .join(format!("cp-{:020}-{}.json", 2, "b".repeat(64)));
    write_private(&empty, b"");

    // Re-running without new events reports the debris and writes nothing.
    let unchanged = installation.create();
    let unchanged = unchanged.expect("creation must survive incomplete record files");
    assert_eq!(unchanged.status, CreateStatus::Unchanged);
    assert_eq!(unchanged.statement_sha256, first.statement_sha256);
    assert_eq!(unchanged.record, first.record);
    let names: Vec<&str> = unchanged
        .incomplete_records
        .iter()
        .map(|record| record.name.as_str())
        .collect();
    assert_eq!(
        names,
        vec![
            truncated.file_name().unwrap().to_str().unwrap(),
            empty.file_name().unwrap().to_str().unwrap()
        ]
    );
    assert_eq!(record_files(&installation.log_dir).len(), 3);

    // With a new event, creation resumes at the same log index.
    append_notes(&installation, &["beta"]);
    let second = installation.create().unwrap();
    assert_eq!(second.status, CreateStatus::Created);
    assert_eq!(second.log_index, 2);
    assert_eq!(second.incomplete_records.len(), 2);
    // The debris is kept as evidence, byte for byte.
    assert_eq!(
        std::fs::read(&truncated).unwrap(),
        &first_bytes[..first_bytes.len() / 2]
    );
    assert_eq!(std::fs::read(&empty).unwrap(), b"");

    // The record was durable but its report was lost: re-running is a no-op.
    let again = installation.create().unwrap();
    assert_eq!(again.status, CreateStatus::Unchanged);
    assert_eq!(again.statement_sha256, second.statement_sha256);
    assert_eq!(again.record, second.record);

    let report = installation.verify(&anchor, &installation.log_dir, Some(&installation.database));
    assert_eq!(report.verdict, Verdict::Verified, "{:?}", report.errors);
    assert_eq!(report.verified_through_log_index, Some(2));
    assert_eq!(report.incomplete_records.len(), 2);
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.code == "incomplete_records_present")
    );
}

#[test]
fn record_file_modified_in_place_is_refused() {
    let installation = Installation::new(TENANT);
    let first = installation.create().unwrap();
    let anchor = installation.anchor("in-place", TENANT, &first.namespace_uuid);
    append_notes(&installation, &["beta"]);
    let second = installation.create().unwrap();
    let referenced = installation.holder.join("in-place.anchor-2.json");
    assert_eq!(
        advance(
            &anchor,
            &installation.log_dir,
            Some(&installation.database),
            &referenced
        )
        .unwrap()
        .status,
        AdvanceStatus::Advanced
    );
    let pristine = std::fs::read_to_string(&second.record).unwrap();
    let name = Path::new(&second.record)
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();

    // One digit of the signed position is changed in the file itself. The
    // content is still a complete, canonical record, but no longer the one
    // its name announces: no interrupted write leaves that.
    let altered = pristine.replacen(
        &format!("\"event_count\":{},", second.event_count),
        &format!("\"event_count\":{},", second.event_count + 1),
        1,
    );
    assert_ne!(altered, pristine);
    assert!(SignedRecord::parse(altered.as_bytes()).is_ok());
    std::fs::write(&second.record, &altered).unwrap();

    let report = installation.verify(&anchor, &installation.log_dir, Some(&installation.database));
    assert_refused(&report, "record_name_mismatch");
    assert_eq!(
        report.errors[0].detail["records"],
        serde_json::json!([name])
    );
    assert_eq!(report.verified_through_log_index, Some(1));
    assert_eq!(report.incomplete_records.len(), 1);
    assert!(report.incomplete_records[0].holds_another_record);

    // Nothing is signed over, or exported from, a directory in that state.
    append_notes(&installation, &["gamma"]);
    let before = record_files(&installation.log_dir);
    let result = installation.create();
    let refused = result.expect_err("an altered record must not be signed over");
    assert_eq!(refused.code(), "record_name_mismatch");
    assert_eq!(refused.exit_code(), 2);
    assert_eq!(record_files(&installation.log_dir), before);
    let bundle = installation.holder.join("in-place.proof.json");
    let refused = export_bundle(&ExportRequest {
        checkpoint_dir: &installation.log_dir,
        out: &bundle,
        from_log_index: None,
        through_log_index: None,
    })
    .expect_err("an altered record must not be exported around");
    assert_eq!(refused.code(), "record_name_mismatch");
    assert!(!bundle.exists());
    assert_eq!(std::fs::read_to_string(&second.record).unwrap(), altered);

    // Truncating the same file leaves what an interrupted write leaves, and
    // is worth no more than deleting it: the log is trusted through the
    // previous record only, and the report says so. This is the stated limit
    // above the anchor reference, not a detection.
    std::fs::write(&second.record, &pristine.as_bytes()[..pristine.len() / 2]).unwrap();
    let report = installation.verify(&anchor, &installation.log_dir, Some(&installation.database));
    assert_eq!(report.verdict, Verdict::Verified, "{:?}", report.errors);
    assert_eq!(report.verified_through_log_index, Some(1));
    assert!(!report.incomplete_records[0].holds_another_record);
    assert!(warned(&report.warnings, "incomplete_records_present"));
    assert!(
        warned(&report.warnings, "incomplete_record_above_head"),
        "{:?}",
        report.warnings
    );
    assert_eq!(report.database.events_after_checkpoint, Some(2));
    // An anchor that accepted the record refuses its disappearance.
    let report = installation.verify(
        &referenced,
        &installation.log_dir,
        Some(&installation.database),
    );
    assert_refused(&report, "log_rolled_back");
}

#[test]
fn established_refusal_survives_an_unreadable_database() {
    let history = History::new();
    let installation = &history.installation;
    // A copy of the database that the private storage policy refuses to open.
    let exposed = copy_database(installation, "exposed-database");
    std::fs::set_permissions(&exposed, std::fs::Permissions::from_mode(0o644)).unwrap();

    // The log was rolled back under the reference: that refusal must reach
    // the caller even though the database could not be read.
    let result = verify(&VerifyRequest {
        anchor: &history.anchor,
        source: RecordSource::Directory(&history.old_log),
        data: Some(&exposed),
        require_reference: false,
        report_out: None,
    });
    let report = result.expect(
        "a refusal that is already established must be reported, not replaced by the reading failure",
    );
    assert_eq!(
        report.error_codes(),
        vec!["log_rolled_back", "audit_unavailable"],
        "{:?}",
        report.errors
    );
    assert_eq!(report.verdict, Verdict::Refused);
    assert!(!report.database.compared);
    assert_eq!(report.database.event_count, None);
    assert_eq!(report.verified_through_log_index, Some(2));

    // Without an established refusal there is no verdict to report: the
    // reading failure stays an operational error.
    let failure = verify(&VerifyRequest {
        anchor: &history.anchor,
        source: RecordSource::Directory(&installation.log_dir),
        data: Some(&exposed),
        require_reference: false,
        report_out: None,
    })
    .unwrap_err();
    assert_eq!(failure.code(), "audit_unavailable");
    assert_eq!(failure.exit_code(), 1);
}

#[test]
fn concurrent_writer_is_refused_and_forks_are_detected() {
    let installation = Installation::new(TENANT);
    let first = installation.create().unwrap();
    let anchor = installation.anchor("fork", TENANT, &first.namespace_uuid);
    append_notes(&installation, &["beta"]);

    // Another writer holds the log.
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(installation.log_dir.join(".writer.lock"))
        .unwrap();
    lock.lock().unwrap();
    let result = installation.create();
    let busy = result.expect_err("a second writer must be refused while the log is held");
    assert_eq!(busy.code(), "checkpoint_log_busy");
    assert_eq!(busy.exit_code(), 1);
    assert_eq!(record_files(&installation.log_dir).len(), 1);
    drop(lock);

    // Two writers that did not share the lock each signed a second record.
    let other_log = copy_log(&installation, &installation.log_dir, "other-writer");
    let second = installation.create().unwrap();
    let other_second = plant_record(
        &other_log,
        &forge_checkpoint(
            &installation,
            &installation.key_dir,
            &read_record(&first.record),
        ),
    );
    assert_eq!((second.log_index, other_second.log_index()), (2, 2));
    assert_ne!(second.statement_sha256, other_second.statement_sha256());
    let stray = other_log.join(other_second.file_name());
    std::fs::copy(
        &stray,
        installation.log_dir.join(stray.file_name().unwrap()),
    )
    .unwrap();

    append_notes(&installation, &["gamma"]);
    let result = installation.create();
    let forked = result.expect_err("a forked log must not be extended");
    assert_eq!(forked.code(), "log_forked");
    assert_eq!(forked.exit_code(), 2);
    assert_eq!(forked.error.detail["log_index"], 2);
    assert_eq!(record_files(&installation.log_dir).len(), 3);
    let report = installation.verify(&anchor, &installation.log_dir, None);
    assert_refused(&report, "log_forked");
    assert_eq!(report.verified_through_log_index, Some(1));
}

#[test]
fn full_log_refuses_and_continuation_links_to_the_previous_record() {
    let history = History::new();
    let installation = &history.installation;

    // Fill the directory up to its bound with record-named files.
    let limit = usize::try_from(MAX_LOG_RECORDS).unwrap();
    for number in 0..limit - 2 {
        write_private(
            &installation
                .log_dir
                .join(format!("cp-{:020}-{:064x}.json", 1, number)),
            b"",
        );
    }
    assert_eq!(record_files(&installation.log_dir).len(), limit);
    append_notes(installation, &["delta"]);
    let started = std::time::Instant::now();
    let result = installation.create();
    println!(
        "create on a full directory of {limit} record files: {:?}",
        started.elapsed()
    );
    let full = result.expect_err("a full checkpoint directory must refuse a new record");
    assert_eq!(full.code(), "checkpoint_log_full");
    assert_eq!(full.exit_code(), 1);
    assert_eq!(
        full.error.detail["last_record"],
        serde_json::json!(history.second.record)
    );
    assert!(full.error.action.contains("--previous-record"));
    assert_eq!(record_files(&installation.log_dir).len(), limit);

    // The log continues in a new directory, linked to the last record.
    let continued = installation.root().join("checkpoints-2");
    let previous = PathBuf::from(&history.second.record);
    let third = create_checkpoint(&CreateRequest {
        data: &installation.database,
        tenant_id: TENANT,
        key_dir: &installation.key_dir,
        checkpoint_dir: &continued,
        new_log: false,
        previous_record: Some(&previous),
    })
    .unwrap();
    assert_eq!(third.status, CreateStatus::Created);
    assert_eq!(third.log_index, 3);
    assert!(!third.baseline);
    let record = SignedRecord::parse(&std::fs::read(&third.record).unwrap()).unwrap();
    let Statement::Checkpoint(statement) = record.statement() else {
        panic!("the continuation record is a checkpoint");
    };
    assert_eq!(
        statement.previous_statement_sha256,
        history.second.statement_sha256
    );
    assert_eq!(statement.previous_event_count, history.second.event_count);
    assert_eq!(statement.previous_head_hash, history.second.head_hash);

    // The link is only for the first record of the new directory.
    append_notes(installation, &["epsilon"]);
    let refused = create_checkpoint(&CreateRequest {
        data: &installation.database,
        tenant_id: TENANT,
        key_dir: &installation.key_dir,
        checkpoint_dir: &continued,
        new_log: false,
        previous_record: Some(&previous),
    })
    .unwrap_err();
    assert_eq!(refused.code(), "invalid_argument");
    let fourth = installation
        .create_for(TENANT, &installation.database, &continued)
        .unwrap();
    assert_eq!(fourth.log_index, 4);

    // The reference accepted for the old directory carries the trust over.
    let report = installation.verify(&history.anchor, &continued, Some(&installation.database));
    assert_eq!(report.verdict, Verdict::Verified, "{:?}", report.errors);
    assert_eq!(report.entries_checked, 2);
    assert_eq!(report.verified_through_log_index, Some(4));
    // Without that reference the continuation alone proves nothing.
    let report = installation.verify(&history.initial_anchor, &continued, None);
    assert_refused(&report, "log_discontinuous");
    assert_eq!(report.errors[0].detail["missing_log_index"], 1);

    // The full directory is still readable; its debris is reported.
    let started = std::time::Instant::now();
    let report = installation.verify(&history.anchor, &installation.log_dir, None);
    println!(
        "verify on a full directory of {limit} record files: {:?}",
        started.elapsed()
    );
    assert_eq!(report.verdict, Verdict::Verified, "{:?}", report.errors);
    assert_eq!(report.incomplete_records.len(), limit - 2);
}

#[test]
fn starting_a_log_is_explicit_and_a_refused_start_leaves_nothing() {
    let installation = Installation::new(TENANT);
    let request = |checkpoint_dir: &Path, tenant: &str, new_log: bool| {
        create_checkpoint(&CreateRequest {
            data: &installation.database,
            tenant_id: tenant,
            key_dir: &installation.key_dir,
            checkpoint_dir,
            new_log,
            previous_record: None,
        })
    };

    // A scheduler line with a mistyped directory must not start a second log
    // silently while the real one stops growing.
    let result = request(&installation.log_dir, TENANT, false);
    let refused = result.expect_err("a missing checkpoint directory must not start a log");
    assert_eq!(refused.code(), "checkpoint_log_unavailable");
    assert_eq!(refused.exit_code(), 1);
    assert!(refused.error.action.contains("--new-log"));
    assert!(!installation.log_dir.exists());
    // An existing, empty directory is not a log either.
    let empty = private_child(installation.root(), "empty-checkpoints");
    let refused = request(&empty, TENANT, false).unwrap_err();
    assert_eq!(refused.code(), "checkpoint_log_empty");
    assert_eq!(refused.exit_code(), 1);
    assert!(refused.error.action.contains("--new-log"));
    // Nothing at all is created in it, not even the writer lock.
    assert_eq!(
        entry_names(&empty),
        Vec::<String>::new(),
        "a refused create must leave an empty directory exactly as it found it"
    );
    // Debris of an interrupted write is not a log either, and stays alone.
    let debris_only = private_child(installation.root(), "debris-checkpoints");
    let debris = format!("cp-{:020}-{}.json", 1, "c".repeat(64));
    write_private(&debris_only.join(&debris), b"{\"format\":");
    let refused = request(&debris_only, TENANT, false).unwrap_err();
    assert_eq!(refused.code(), "checkpoint_log_empty");
    assert_eq!(entry_names(&debris_only), vec![debris.clone()]);
    assert_eq!(
        std::fs::read(debris_only.join(&debris)).unwrap(),
        b"{\"format\":"
    );

    // A start that is refused creates nothing.
    let refused_dir = installation.root().join("refused-checkpoints");
    let refused = request(&refused_dir, "tenant-b", true).unwrap_err();
    assert_eq!(refused.code(), "scope_mismatch");
    assert!(!refused_dir.exists());

    // The explicit start works once; afterwards the flag is refused, so that
    // it cannot stay on a scheduler line.
    let first = request(&installation.log_dir, TENANT, true).unwrap();
    assert_eq!(first.status, CreateStatus::Created);
    assert!(first.baseline);
    // A started log holds its first record and the writer lock, nothing else.
    assert_eq!(
        entry_names(&installation.log_dir),
        vec![".writer.lock".to_owned(), file_name_of(&first.record)]
    );
    append_notes(&installation, &["beta"]);
    let refused = request(&installation.log_dir, TENANT, true).unwrap_err();
    assert_eq!(refused.code(), "invalid_argument");
    assert_eq!(record_files(&installation.log_dir).len(), 1);
    let second = request(&installation.log_dir, TENANT, false).unwrap();
    assert_eq!(second.log_index, 2);
    assert!(!second.baseline);

    // The signing key never lives in the directory that is copied, exported
    // and backed up as evidence.
    let mixed = installation.root().join("mixed");
    init_key(&mixed).unwrap();
    let result = create_checkpoint(&CreateRequest {
        data: &installation.database,
        tenant_id: TENANT,
        key_dir: &mixed,
        checkpoint_dir: &mixed,
        new_log: true,
        previous_record: None,
    });
    let refused = result.expect_err("records must not be written next to the signing key");
    assert_eq!(refused.code(), "invalid_argument");
    assert_eq!(refused.error.detail["argument"], "key_dir");
    let entries: Vec<_> = std::fs::read_dir(&mixed)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(entries, vec![std::ffi::OsString::from("signing-key.pk8")]);
    let nested = installation.log_dir.join("key-2");
    init_key(&nested).unwrap();
    let result = rotate_key(&RotateRequest {
        checkpoint_dir: &installation.log_dir,
        new_key_dir: &nested,
        retiring: RetiringKey::Endorsing {
            key_dir: &installation.key_dir,
        },
    });
    let refused = result.expect_err("a successor key must not be kept under the records");
    assert_eq!(refused.code(), "invalid_argument");
    assert_eq!(refused.error.detail["argument"], "new_key_dir");
    assert_eq!(record_files(&installation.log_dir).len(), 2);
}

#[test]
fn a_refused_start_writes_nothing_in_a_directory_that_already_exists() {
    let installation = Installation::new(TENANT);
    let first = installation.create().unwrap();
    let first_name = file_name_of(&first.record);
    let first_bytes = std::fs::read(&first.record).unwrap();
    append_notes(&installation, &["beta"]);
    let start = |checkpoint_dir: &Path, previous_record: Option<&Path>| {
        create_checkpoint(&CreateRequest {
            data: &installation.database,
            tenant_id: TENANT,
            key_dir: &installation.key_dir,
            checkpoint_dir,
            new_log: previous_record.is_none(),
            previous_record,
        })
    };

    // Every case is a private directory that exists before the command runs,
    // holds no writer lock, and must be found exactly as it was after the
    // refusal. The cases whose directory changed are collected, so that one
    // failure names all of them with what the refused command left in them.
    let mut changed: Vec<(&str, Vec<String>)> = Vec::new();
    let mut unchanged_after = |case: &'static str, directory: &Path, before: &[String]| {
        let after = entry_names(directory);
        if after != before {
            let known: std::collections::BTreeSet<&String> = before.iter().collect();
            changed.push((
                case,
                after
                    .into_iter()
                    .filter(|name| !known.contains(name))
                    .collect(),
            ));
        }
    };

    // A checkpoint directory restored from a copy of its records. The writer
    // lock is not a record and was not part of the copy.
    for (case, name, previous_record) in [
        ("restored records, --new-log", "restored-new-log", None),
        (
            "restored records, --previous-record",
            "restored-previous-record",
            Some(Path::new(&first.record)),
        ),
    ] {
        let restored = copy_log(&installation, &installation.log_dir, name);
        let before = entry_names(&restored);
        assert_eq!(before, vec![first_name.clone()]);
        let refused = start(&restored, previous_record)
            .expect_err("a log must not be started in a directory that holds a complete record");
        assert_eq!(refused.code(), "invalid_argument");
        assert_eq!(refused.exit_code(), 1);
        assert!(
            refused.error.action.contains("nothing was written"),
            "{}",
            refused.error.action
        );
        unchanged_after(case, &restored, &before);
        assert_eq!(
            std::fs::read(restored.join(&first_name)).unwrap(),
            first_bytes
        );
    }

    // A directory whose only record file holds a complete record other than
    // the one its name announces.
    let renamed = private_child(installation.root(), "renamed-checkpoints");
    let stray_name = format!("cp-{:020}-{}.json", 1, "e".repeat(64));
    write_private(&renamed.join(&stray_name), &first_bytes);
    let before = entry_names(&renamed);
    let refused = start(&renamed, None)
        .expect_err("a log must not be started beside a record file that was altered");
    assert_eq!(refused.code(), "record_name_mismatch");
    assert_eq!(refused.exit_code(), 2);
    assert_eq!(
        refused.error.detail["records"],
        serde_json::json!([stray_name])
    );
    unchanged_after("renamed record alone", &renamed, &before);
    assert_eq!(
        std::fs::read(renamed.join(&stray_name)).unwrap(),
        first_bytes
    );

    // A directory that has no room left for a record file.
    let full = private_child(installation.root(), "full-checkpoints");
    let limit = usize::try_from(MAX_LOG_RECORDS).unwrap();
    for number in 0..limit {
        write_private(
            &full.join(format!("cp-{:020}-{:064x}.json", 1, number)),
            b"",
        );
    }
    let before = entry_names(&full);
    assert_eq!(before.len(), limit);
    let refused =
        start(&full, None).expect_err("a log must not be started in a directory that is full");
    assert_eq!(refused.code(), "checkpoint_log_full");
    assert_eq!(refused.exit_code(), 1);
    unchanged_after("directory without room", &full, &before);

    // A directory in which two complete records claim the same position.
    let forked = copy_log(&installation, &installation.log_dir, "forked-checkpoints");
    let first_record = read_record(&first.record);
    let template = first_statement(&installation);
    for signed in [
        (first.event_count, first.head_hash.clone()),
        (first.event_count + 1, "f".repeat(64)),
    ] {
        plant_record(
            &forked,
            &forge_checkpoint_with(
                &template,
                &installation.key_dir,
                &first_record,
                (first.event_count, first.head_hash.clone()),
                signed,
            ),
        );
    }
    let before = entry_names(&forked);
    assert_eq!(before.len(), 3);
    let refused = start(&forked, None)
        .expect_err("a log must not be started in a directory that holds a fork");
    assert_eq!(refused.code(), "log_forked");
    assert_eq!(refused.exit_code(), 2);
    assert_eq!(refused.error.detail["log_index"], 2);
    unchanged_after("two records at one position", &forked, &before);

    assert_eq!(
        changed,
        Vec::<(&str, Vec<String>)>::new(),
        "a refused start must leave an existing directory exactly as it found it"
    );

    // An existing directory that holds no complete record can be started:
    // the lock file comes with the first record, and the debris stays.
    let debris_only = private_child(installation.root(), "debris-checkpoints");
    let debris = format!("cp-{:020}-{}.json", 1, "c".repeat(64));
    write_private(&debris_only.join(&debris), b"{\"format\":");
    let started = start(&debris_only, None).unwrap();
    assert_eq!(started.status, CreateStatus::Created);
    assert_eq!(started.log_index, 1);
    assert_eq!(started.incomplete_records.len(), 1);
    let mut expected = vec![
        ".writer.lock".to_owned(),
        debris.clone(),
        file_name_of(&started.record),
    ];
    expected.sort();
    assert_eq!(entry_names(&debris_only), expected);
    assert_eq!(
        std::fs::read(debris_only.join(&debris)).unwrap(),
        b"{\"format\":"
    );

    // Without --new-log, restored records are a log to extend: the command
    // takes the writer lock before it compares its key and the database with
    // the head. A refusal decided under the lock leaves the lock file in a
    // directory that had none, and nothing else.
    let restored = copy_log(&installation, &installation.log_dir, "restored-scheduled");
    let other_key = installation.root().join("keys").join("other-key");
    init_key(&other_key).unwrap();
    let refused = create_checkpoint(&CreateRequest {
        data: &installation.database,
        tenant_id: TENANT,
        key_dir: &other_key,
        checkpoint_dir: &restored,
        new_log: false,
        previous_record: None,
    })
    .expect_err("a key that did not sign the head of the log must not extend it");
    assert_eq!(refused.code(), "key_id_mismatch");
    assert_eq!(refused.exit_code(), 2);
    assert_eq!(
        entry_names(&restored),
        vec![".writer.lock".to_owned(), first_name.clone()]
    );
    assert_eq!(
        std::fs::read(restored.join(&first_name)).unwrap(),
        first_bytes
    );
}

#[test]
fn a_directory_beyond_the_record_bound_exports_a_bundle_that_verifies() {
    let installation = Installation::new(TENANT);
    let first = installation.create().unwrap();
    let anchor = installation.anchor("full-export", TENANT, &first.namespace_uuid);
    let template = first_statement(&installation);
    let position = (first.event_count, first.head_hash.clone());
    let mut previous = read_record(&first.record);
    // Records that re-sign the same head with the pinned key, as its holder
    // can write them.
    let mut extend = |count: u32| {
        for _ in 0..count {
            previous = plant_record(
                &installation.log_dir,
                &forge_checkpoint_with(
                    &template,
                    &installation.key_dir,
                    &previous,
                    position.clone(),
                    position.clone(),
                ),
            );
        }
        previous.log_index()
    };

    // One record more than create accepts in a directory: what a rotation
    // recorded in a full directory leaves behind.
    let complete = u64::from(MAX_LOG_RECORDS) + 1;
    assert_eq!(extend(MAX_LOG_RECORDS), complete);
    let export = |out: &Path, from_log_index: Option<u64>| {
        export_bundle(&ExportRequest {
            checkpoint_dir: &installation.log_dir,
            out,
            from_log_index,
            through_log_index: None,
        })
    };
    let bundle = installation.holder.join("full.proof.json");
    let exported = export(&bundle, None).unwrap();
    assert_eq!(exported.entries, complete);
    let result = verify(&VerifyRequest {
        anchor: &anchor,
        source: RecordSource::Bundle(&bundle),
        data: None,
        require_reference: false,
        report_out: None,
    });
    let report = result.expect("a bundle written by export must be readable by verify");
    assert_eq!(report.verdict, Verdict::Verified, "{:?}", report.errors);
    assert_eq!(report.entries_checked, complete);
    assert_eq!(report.verified_through_log_index, Some(complete));

    // Beyond the bundle bound, export asks for a range instead of writing a
    // bundle that no verifier reads.
    let bound = u64::from(MAX_BUNDLE_ENTRIES);
    assert_eq!(extend(MAX_BUNDLE_ENTRIES - MAX_LOG_RECORDS), bound + 1);
    let too_large = installation.holder.join("too-large.proof.json");
    let result = export(&too_large, None);
    let refused = result.expect_err("a bundle beyond the bound must not be written");
    assert_eq!(refused.code(), "invalid_argument");
    assert_eq!(refused.exit_code(), 1);
    assert_eq!(refused.error.detail["requested_entries"], bound + 1);
    assert_eq!(refused.error.detail["limit"], bound);
    assert!(refused.error.action.contains("--from-log-index"));
    assert!(!too_large.exists());
    let ranged = installation.holder.join("ranged.proof.json");
    assert_eq!(export(&ranged, Some(2)).unwrap().entries, bound);
}

#[test]
fn checkpoints_stay_verifiable_across_schema_migrations() {
    let installation = Installation::new(TENANT);
    let first = installation.create().unwrap();
    assert_eq!(first.event_count, 2);
    let initial = installation.anchor("migrations", TENANT, &first.namespace_uuid);
    let anchor = installation.holder.join("migrations.anchor-2.json");
    assert_eq!(
        advance(
            &initial,
            &installation.log_dir,
            Some(&installation.database),
            &anchor
        )
        .unwrap()
        .status,
        AdvanceStatus::Advanced
    );
    let record = SignedRecord::parse(&std::fs::read(&first.record).unwrap()).unwrap();
    let Statement::Checkpoint(statement) = record.statement() else {
        panic!("the first record is a checkpoint");
    };
    assert_eq!(statement.audit_schema_version, 2);
    let control_genesis_hash = statement.control_genesis_hash.clone();

    // Every explicit maintenance migration reachable through the public API.
    let backups = private_child(installation.root(), "migration-backups");
    let mut expected_log_index = 1;
    for version in 3..=12_u32 {
        let store = open_store(&installation.database, TENANT);
        let backup = backups.join(format!("before-{version}.sqlite"));
        match version {
            3 => store.migrate_control_v3(&backup),
            4 => store.migrate_identity_v4(&backup),
            5 => store.migrate_human_access_v5(&backup),
            6 => store.migrate_workload_v6(&backup),
            7 => store.migrate_human_brokers_v7(&backup),
            8 => store.migrate_kubernetes_create_v8(&backup),
            9 => store.migrate_kubernetes_policy_v9(&backup),
            10 => store.migrate_human_workers_v10(&backup),
            11 => store.migrate_kubernetes_session_reservations_v11(&backup),
            _ => store.migrate_broker_checkpoints_v12(&backup),
        }
        .unwrap();
        store.append(note(&format!("after-{version}"))).unwrap();
        drop(store);

        // The checkpoint signed at schema 2 still matches the migrated chain.
        let report =
            installation.verify(&anchor, &installation.log_dir, Some(&installation.database));
        assert_eq!(
            report.verdict,
            Verdict::Verified,
            "schema {version}: {:?}",
            report.errors
        );
        assert_eq!(report.database.audit_schema_version, Some(version));
        assert!(report.database.events_after_checkpoint.unwrap() >= 2);

        let created = installation.create().unwrap();
        expected_log_index += 1;
        assert_eq!(created.log_index, expected_log_index);
        let record = SignedRecord::parse(&std::fs::read(&created.record).unwrap()).unwrap();
        let Statement::Checkpoint(statement) = record.statement() else {
            panic!("a checkpoint was created");
        };
        assert_eq!(statement.audit_schema_version, version);
        assert_eq!(statement.namespace_uuid, first.namespace_uuid);
        assert_eq!(statement.control_genesis_seq, 1);
        assert_eq!(statement.control_genesis_hash, control_genesis_hash);
    }
    let report = installation.verify(&anchor, &installation.log_dir, Some(&installation.database));
    assert_eq!(report.verdict, Verdict::Verified, "{:?}", report.errors);
    assert_eq!(report.verified_through_log_index, Some(11));
    assert_eq!(report.entries_checked, 10);
    assert_eq!(report.database.audit_schema_version, Some(12));
    assert_eq!(report.database.events_after_checkpoint, Some(0));
}

// ---------------------------------------------------------------------------
// Key rotation
// ---------------------------------------------------------------------------

/// Sign statement bytes exactly as the tool does, with the key stored in
/// `key_dir`. This is what the holder of that key file can do without the
/// tool: tests use it to forge records.
fn sign_with_key_file(key_dir: &Path, statement: &str) -> (String, String) {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use ring::signature::KeyPair;
    let pkcs8 = std::fs::read(key_dir.join("signing-key.pk8")).unwrap();
    let pair = ring::signature::Ed25519KeyPair::from_pkcs8(&pkcs8).unwrap();
    let mut message = SIGNATURE_DOMAIN.to_vec();
    message.extend_from_slice(statement.as_bytes());
    (
        sha256_hex(pair.public_key().as_ref()),
        URL_SAFE_NO_PAD.encode(pair.sign(&message).as_ref()),
    )
}

fn envelope(statement: &str, signatures: &[(String, String)]) -> String {
    let signatures: Vec<String> = signatures
        .iter()
        .map(|(key_id, signature)| {
            format!("{{\"key_id\":\"{key_id}\",\"signature\":\"{signature}\"}}")
        })
        .collect();
    format!(
        "{{\"format\":\"ai-guard.audit.signed-checkpoint.v1\",\"statement\":{statement},\"signatures\":[{}]}}",
        signatures.join(",")
    )
}

/// Write a forged but well-formed record under its content-addressed name.
fn plant_record(log_dir: &Path, content: &str) -> SignedRecord {
    let record = SignedRecord::parse(content.as_bytes()).unwrap();
    write_private(&log_dir.join(record.file_name()), content.as_bytes());
    record
}

fn read_record(path: &str) -> SignedRecord {
    SignedRecord::parse(&std::fs::read(path).unwrap()).unwrap()
}

/// A checkpoint signed with the key file of `key_dir` that links to
/// `previous`, recalls `recalled` as the last signed position and signs
/// `signed`. Scope and genesis are taken from `template`.
fn forge_checkpoint_with(
    template: &CheckpointStatement,
    key_dir: &Path,
    previous: &SignedRecord,
    recalled: (u64, String),
    signed: (u64, String),
) -> String {
    let (key_id, _) = sign_with_key_file(key_dir, "");
    let statement = serde_json::to_string(&CheckpointStatement {
        log_index: previous.log_index() + 1,
        previous_statement_sha256: previous.statement_sha256().into(),
        // A fixed time in the past: a forged record never depends on the
        // clock to differ from a record the tool just created.
        created_at: "2001-01-01T00:00:00.000Z".into(),
        signing_key_id: key_id,
        event_count: signed.0,
        head_hash: signed.1,
        previous_event_count: recalled.0,
        previous_head_hash: recalled.1,
        ..template.clone()
    })
    .unwrap();
    let signature = sign_with_key_file(key_dir, &statement);
    envelope(&statement, &[signature])
}

/// The statement of the first record of the installation log.
fn first_statement(installation: &Installation) -> CheckpointStatement {
    let Statement::Checkpoint(statement) =
        read_record(record_files(&installation.log_dir)[0].to_str().unwrap())
            .statement()
            .clone()
    else {
        panic!("the first record is a checkpoint");
    };
    statement
}

/// A checkpoint over the current audit head, signed with the key file of
/// `key_dir`, that claims to follow `previous` and recalls its position.
fn forge_checkpoint(
    installation: &Installation,
    key_dir: &Path,
    previous: &SignedRecord,
) -> String {
    let store = open_store(&installation.database, TENANT);
    let integrity = store.integrity().unwrap();
    drop(store);
    let position = previous.statement().signed_position();
    forge_checkpoint_with(
        &first_statement(installation),
        key_dir,
        previous,
        (position.event_count, position.head_hash),
        (integrity.event_count, integrity.head_hash),
    )
}

/// A key rotation statement at the log index after `previous`, in canonical
/// form, carrying its signed position forward.
fn rotation_statement(
    namespace: &str,
    previous: &SignedRecord,
    retiring_key_id: &str,
    successor_key_id: &str,
    endorsement: &str,
) -> String {
    let position = previous.statement().signed_position();
    serde_json::to_string(&RotationStatement {
        statement_type: "ai-guard.audit.checkpoint".into(),
        version: 1,
        kind: "key_rotation".into(),
        tenant_id: TENANT.into(),
        namespace_uuid: namespace.into(),
        log_index: previous.log_index() + 1,
        previous_statement_sha256: previous.statement_sha256().into(),
        created_at: "2001-01-01T00:00:00.000Z".into(),
        retiring_key_id: retiring_key_id.into(),
        successor_key_id: successor_key_id.into(),
        endorsement: endorsement.into(),
        previous_event_count: position.event_count,
        previous_head_hash: position.head_hash,
    })
    .unwrap()
}

/// Approve from the records alone, as an auditor without the database does.
fn approve(
    anchor: &Path,
    log_dir: &Path,
    public_key: &Path,
    expect_key_id: &str,
    unendorsed_recovery: bool,
    out: &Path,
) -> Result<AnchorOutcome, Failure> {
    approve_with(
        anchor,
        log_dir,
        None,
        public_key,
        expect_key_id,
        unendorsed_recovery,
        out,
    )
}

fn approve_with(
    anchor: &Path,
    log_dir: &Path,
    database: Option<&Path>,
    public_key: &Path,
    expect_key_id: &str,
    unendorsed_recovery: bool,
    out: &Path,
) -> Result<AnchorOutcome, Failure> {
    approve_rotation(&RotationApprovalRequest {
        anchor,
        source: RecordSource::Directory(log_dir),
        data: database,
        public_key,
        expect_key_id,
        unendorsed_recovery,
        out,
    })
}

/// An installation whose anchor accepted the first checkpoint, a second
/// checkpoint by the first key, and a successor key that signed nothing yet.
struct Succession {
    installation: Installation,
    first: CreateOutcome,
    second: CreateOutcome,
    /// Anchor with the first record as reference.
    anchor: PathBuf,
    new_key_dir: PathBuf,
    new_key_id: String,
    new_public_key: PathBuf,
}

impl Succession {
    fn new() -> Self {
        let installation = Installation::new(TENANT);
        let first = installation.create().unwrap();
        let initial = installation.anchor("succession", TENANT, &first.namespace_uuid);
        let anchor = installation.holder.join("succession.anchor-2.json");
        assert_eq!(
            advance(&initial, &installation.log_dir, None, &anchor)
                .unwrap()
                .status,
            AdvanceStatus::Advanced
        );
        append_notes(&installation, &["beta"]);
        let second = installation.create().unwrap();
        let new_key_dir = installation.root().join("keys").join("key-2");
        let new_key = init_key(&new_key_dir).unwrap();
        let new_public_key = installation.holder.join("key-2.public-key.json");
        show_key(&new_key_dir, Some(&new_public_key)).unwrap();
        Self {
            installation,
            first,
            second,
            anchor,
            new_key_dir,
            new_key_id: new_key.key_id,
            new_public_key,
        }
    }

    fn rotate(&self, retiring: RetiringKey<'_>) -> Result<RotateOutcome, Failure> {
        rotate_key(&RotateRequest {
            checkpoint_dir: &self.installation.log_dir,
            new_key_dir: &self.new_key_dir,
            retiring,
        })
    }

    fn endorsing(&self) -> RetiringKey<'_> {
        RetiringKey::Endorsing {
            key_dir: &self.installation.key_dir,
        }
    }

    fn create_with_new_key(&self) -> Result<CreateOutcome, Failure> {
        create_checkpoint(&CreateRequest {
            data: &self.installation.database,
            tenant_id: TENANT,
            key_dir: &self.new_key_dir,
            checkpoint_dir: &self.installation.log_dir,
            new_log: false,
            previous_record: None,
        })
    }
}

#[test]
fn endorsed_rotation_is_verified_after_explicit_approval() {
    let succession = Succession::new();
    let installation = &succession.installation;

    // Until the rotation is recorded, the new key cannot extend the log.
    let refused = succession.create_with_new_key().unwrap_err();
    assert_eq!(refused.code(), "key_id_mismatch");

    let rotated = succession.rotate(succession.endorsing()).unwrap();
    assert_eq!(rotated.status, RotateStatus::Rotated);
    assert_eq!(rotated.log_index, 3);
    assert_eq!(rotated.endorsement, "retiring_key");
    assert_eq!(rotated.retiring_key_id, installation.key_id);
    assert_eq!(rotated.successor_key_id, succession.new_key_id);
    let record = read_record(&rotated.record);
    let Statement::KeyRotation(statement) = record.statement() else {
        panic!("the third record is a key rotation");
    };
    assert_eq!(
        statement.previous_statement_sha256,
        succession.second.statement_sha256
    );
    assert_eq!(
        statement.previous_event_count,
        succession.second.event_count
    );
    assert_eq!(statement.previous_head_hash, succession.second.head_hash);
    let signers: Vec<&str> = record
        .signatures()
        .iter()
        .map(|signature| signature.key_id.as_str())
        .collect();
    assert_eq!(
        signers,
        vec![installation.key_id.as_str(), succession.new_key_id.as_str()]
    );

    // After the rotation the retired key cannot extend the log, the new one can.
    append_notes(installation, &["gamma"]);
    let refused = installation.create().unwrap_err();
    assert_eq!(refused.code(), "key_id_mismatch");
    let fourth = succession.create_with_new_key().unwrap();
    assert_eq!(fourth.log_index, 4);
    assert_eq!(fourth.signing_key_id, succession.new_key_id);

    // Endorsed is not approved: the verifier stops before the rotation.
    let report = installation.verify(&succession.anchor, &installation.log_dir, None);
    assert_refused(&report, "untrusted_key");
    assert_eq!(report.errors[0].detail["record_kind"], "key_rotation");
    assert_eq!(
        report.errors[0].detail["key_id"],
        serde_json::json!(succession.new_key_id)
    );
    assert_eq!(report.verified_through_log_index, Some(2));
    let stalled = installation.holder.join("succession.anchor-stalled.json");
    let refused = advance(&succession.anchor, &installation.log_dir, None, &stalled).unwrap();
    assert_eq!(refused.status, AdvanceStatus::Refused);
    assert!(!stalled.exists());

    // Explicit approval by the anchor holder, with the fingerprint received
    // through a separate channel.
    let approved_anchor = installation.holder.join("succession.anchor-3.json");
    let approved = approve_with(
        &succession.anchor,
        &installation.log_dir,
        Some(&installation.database),
        &succession.new_public_key,
        &succession.new_key_id,
        false,
        &approved_anchor,
    )
    .unwrap();
    assert_eq!(approved.status, "rotation_approved");
    assert_eq!(approved.revision, 3);
    // The fourth record stands after the rotation: the approval examined the
    // records up to the rotation only, and says so.
    assert_eq!(
        approved
            .warnings
            .iter()
            .map(|warning| warning.code)
            .collect::<Vec<_>>(),
        vec!["records_after_rotation_not_examined"],
        "an approval must say that it did not examine the records that stand after the rotation: {:?}",
        approved.warnings
    );
    assert_eq!(approved.keys.len(), 2);
    assert_eq!(approved.keys[0].key_id, installation.key_id);
    assert_eq!(approved.keys[0].valid_through_log_index, Some(2));
    assert_eq!(approved.keys[1].key_id, succession.new_key_id);
    assert_eq!(approved.keys[1].valid_from_log_index, 3);
    assert_eq!(approved.keys[1].valid_through_log_index, None);
    assert_eq!(approved.keys[1].approval, "endorsed_rotation");
    // The rotation record itself is pinned as the reference.
    let reference = approved.reference.unwrap();
    assert_eq!(reference.log_index, 3);
    assert_eq!(reference.statement_sha256, rotated.statement_sha256);
    assert_eq!(reference.checkpoint.log_index, 2);
    assert_eq!(
        reference.checkpoint.event_count,
        succession.second.event_count
    );
    assert_eq!(reference.checkpoint.head_hash, succession.second.head_hash);
    // The database held every position up to the rotation: nothing to retain.
    assert!(reference.retained_checkpoints.is_empty());

    let report = installation.verify(
        &approved_anchor,
        &installation.log_dir,
        Some(&installation.database),
    );
    assert_eq!(report.verdict, Verdict::Verified, "{:?}", report.errors);
    assert_eq!(report.verified_through_log_index, Some(4));
    assert_eq!(report.entries_checked, 1);
    assert_eq!(report.signatures_verified, 1);
    assert_eq!(
        report.head.as_ref().unwrap().signing_key_id,
        succession.new_key_id
    );
    assert_eq!(report.database.events_after_checkpoint, Some(0));
}

#[test]
fn retired_key_cannot_sign_after_rotation() {
    let succession = Succession::new();
    let installation = &succession.installation;
    let rotated = succession.rotate(succession.endorsing()).unwrap();
    let approved_anchor = installation.holder.join("succession.anchor-3.json");
    approve(
        &succession.anchor,
        &installation.log_dir,
        &succession.new_public_key,
        &succession.new_key_id,
        false,
        &approved_anchor,
    )
    .unwrap();
    let records = record_files(&installation.log_dir);
    assert_eq!(records.len(), 3);
    let first = read_record(records[0].to_str().unwrap());
    let second = read_record(records[1].to_str().unwrap());
    let rotation = read_record(&rotated.record);
    append_notes(installation, &["gamma"]);

    // The holder of the retired key signs a checkpoint after the rotation.
    let after_rotation = copy_log(installation, &installation.log_dir, "retired-after");
    let forged = plant_record(
        &after_rotation,
        &forge_checkpoint(installation, &installation.key_dir, &rotation),
    );
    assert_eq!(forged.log_index(), 4);
    let report = installation.verify(&approved_anchor, &after_rotation, None);
    assert_refused(&report, "key_not_valid_at_index");
    assert_eq!(report.errors[0].detail["log_index"], 4);
    assert_eq!(
        report.errors[0].detail["key_id"],
        serde_json::json!(installation.key_id)
    );
    assert_eq!(report.verified_through_log_index, Some(3));

    // The same holder replaces the rotation record by a checkpoint of its own
    // at the rotation index.
    let in_place = private_child(installation.root(), "retired-in-place");
    for record in &records[..2] {
        std::fs::copy(record, in_place.join(record.file_name().unwrap())).unwrap();
    }
    let forged = plant_record(
        &in_place,
        &forge_checkpoint(installation, &installation.key_dir, &second),
    );
    assert_eq!(forged.log_index(), 3);
    let report = installation.verify(&approved_anchor, &in_place, None);
    assert_refused(&report, "log_diverged_from_reference");
    assert_eq!(report.errors[0].detail["log_index"], 3);

    // Or rewrites everything between the previous reference and the
    // rotation.
    let alternative = private_child(installation.root(), "retired-alternative");
    std::fs::copy(
        &records[0],
        alternative.join(records[0].file_name().unwrap()),
    )
    .unwrap();
    let second_forged = plant_record(
        &alternative,
        &forge_checkpoint(installation, &installation.key_dir, &first),
    );
    assert_eq!(second_forged.log_index(), 2);
    let report = installation.verify(&approved_anchor, &alternative, None);
    assert_refused(&report, "log_rolled_back");
    let third_forged = plant_record(
        &alternative,
        &forge_checkpoint(installation, &installation.key_dir, &second_forged),
    );
    assert_eq!(third_forged.log_index(), 3);
    let report = installation.verify(&approved_anchor, &alternative, Some(&installation.database));
    assert_eq!(
        report.error_codes()[0],
        "log_diverged_from_reference",
        "{:?}",
        report.errors
    );
    assert_eq!(report.verdict, Verdict::Refused);

    // Before the approval, the previous anchor still trusts the retired key:
    // what it signed between the old reference and the rotation is only as
    // trustworthy as that key. This is the stated limit, not a defect.
    let report = installation.verify(&succession.anchor, &alternative, None);
    assert_eq!(report.verdict, Verdict::Verified, "{:?}", report.errors);
    assert_eq!(report.verified_through_log_index, Some(3));
    assert_eq!(first.statement_sha256(), succession.first.statement_sha256);
}

#[test]
fn forged_or_unendorsed_rotation_requires_explicit_recovery_approval() {
    let succession = Succession::new();
    let installation = &succession.installation;
    let out = installation.holder.join("succession.anchor-3.json");

    // No rotation recorded yet: nothing to approve.
    let nothing = approve(
        &succession.anchor,
        &installation.log_dir,
        &succession.new_public_key,
        &succession.new_key_id,
        false,
        &out,
    )
    .unwrap_err();
    assert_eq!(nothing.code(), "no_pending_rotation");
    assert_eq!(nothing.exit_code(), 1);

    // A rotation that claims an endorsement it does not have: the retiring
    // signature is not one of the pinned key.
    let records = record_files(&installation.log_dir);
    let second = read_record(records[1].to_str().unwrap());
    let position = second.statement().signed_position();
    let rotation_statement = serde_json::to_string(&RotationStatement {
        statement_type: "ai-guard.audit.checkpoint".into(),
        version: 1,
        kind: "key_rotation".into(),
        tenant_id: TENANT.into(),
        namespace_uuid: succession.first.namespace_uuid.clone(),
        log_index: 3,
        previous_statement_sha256: second.statement_sha256().into(),
        created_at: "2026-11-10T12:00:00.000Z".into(),
        retiring_key_id: installation.key_id.clone(),
        successor_key_id: succession.new_key_id.clone(),
        endorsement: "retiring_key".into(),
        previous_event_count: position.event_count,
        previous_head_hash: position.head_hash,
    })
    .unwrap();
    let successor_signature = sign_with_key_file(&succession.new_key_dir, &rotation_statement);
    // The forger signs the "endorsement" with the only key it has.
    let fake_endorsement = (installation.key_id.clone(), successor_signature.1.clone());
    let forged_log = copy_log(installation, &installation.log_dir, "forged-endorsement");
    plant_record(
        &forged_log,
        &envelope(
            &rotation_statement,
            &[fake_endorsement, successor_signature],
        ),
    );
    let refused = approve(
        &succession.anchor,
        &forged_log,
        &succession.new_public_key,
        &succession.new_key_id,
        false,
        &out,
    )
    .expect_err("a rotation with a forged endorsement must not be approved");
    assert_eq!(refused.code(), "rotation_not_endorsed");
    assert_eq!(refused.exit_code(), 2);
    assert!(!out.exists());

    // The retiring key is lost: the operator records an unendorsed rotation.
    let wrong = succession
        .rotate(RetiringKey::UnendorsedRecovery {
            retiring_key_id: &succession.new_key_id,
        })
        .unwrap_err();
    assert_eq!(wrong.code(), "invalid_argument");
    let wrong = succession
        .rotate(RetiringKey::UnendorsedRecovery {
            retiring_key_id: &"c".repeat(64),
        })
        .unwrap_err();
    assert_eq!(wrong.code(), "key_id_mismatch");
    let rotated = succession
        .rotate(RetiringKey::UnendorsedRecovery {
            retiring_key_id: &installation.key_id,
        })
        .unwrap();
    assert_eq!(rotated.status, RotateStatus::Rotated);
    assert_eq!(rotated.endorsement, "none");
    assert_eq!(read_record(&rotated.record).signatures().len(), 1);

    // Without an explicit recovery decision it is refused.
    let refused = approve(
        &succession.anchor,
        &installation.log_dir,
        &succession.new_public_key,
        &succession.new_key_id,
        false,
        &out,
    )
    .expect_err("an unendorsed rotation needs an explicit recovery decision");
    assert_eq!(refused.code(), "rotation_not_endorsed");
    assert!(!out.exists());

    // The three fingerprints must agree.
    let refused = approve(
        &succession.anchor,
        &installation.log_dir,
        &succession.new_public_key,
        &installation.key_id,
        true,
        &out,
    )
    .expect_err("the expected fingerprint must match the public key file");
    assert_eq!(refused.code(), "key_id_mismatch");
    let third_key_dir = installation.root().join("keys").join("key-3");
    let third_key = init_key(&third_key_dir).unwrap();
    let third_public_key = installation.holder.join("key-3.public-key.json");
    show_key(&third_key_dir, Some(&third_public_key)).unwrap();
    let refused = approve(
        &succession.anchor,
        &installation.log_dir,
        &third_public_key,
        &third_key.key_id,
        true,
        &out,
    )
    .expect_err("the expected fingerprint must match the rotation record");
    assert_eq!(refused.code(), "key_id_mismatch");
    assert!(!out.exists());

    // The explicit recovery is accepted and stays visible.
    let approved = approve_with(
        &succession.anchor,
        &installation.log_dir,
        Some(&installation.database),
        &succession.new_public_key,
        &succession.new_key_id,
        true,
        &out,
    )
    .unwrap();
    assert_eq!(approved.keys[1].approval, "operator_recovery");
    assert_eq!(approved.warnings.len(), 1);
    assert_eq!(approved.warnings[0].code, "unendorsed_key_approval");
    append_notes(installation, &["gamma"]);
    succession.create_with_new_key().unwrap();
    let report = installation.verify(&out, &installation.log_dir, Some(&installation.database));
    assert_eq!(report.verdict, Verdict::Verified, "{:?}", report.errors);
    assert_eq!(report.verified_through_log_index, Some(4));
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.code == "unendorsed_key_approval")
    );

    // An endorsed rotation is never approved through the recovery switch.
    let other = Succession::new();
    other.rotate(other.endorsing()).unwrap();
    let refused = approve(
        &other.anchor,
        &other.installation.log_dir,
        &other.new_public_key,
        &other.new_key_id,
        true,
        &other.installation.holder.join("other.anchor-3.json"),
    )
    .unwrap_err();
    assert_eq!(refused.code(), "invalid_argument");
}

#[test]
fn record_that_misstates_the_last_signed_position_is_refused() {
    let installation = Installation::new(TENANT);
    let first = installation.create().unwrap();
    let anchor = installation.anchor("regression", TENANT, &first.namespace_uuid);
    append_notes(&installation, &["beta"]);
    let store = open_store(&installation.database, TENANT);
    let head = store.integrity().unwrap();
    drop(store);
    let template = first_statement(&installation);

    // Signed by the pinned key and linked to the first record, but recalling
    // an earlier position than the one that record signed. Accepting it would
    // let the signer move the chain of positions wherever it wants.
    let forged = plant_record(
        &installation.log_dir,
        &forge_checkpoint_with(
            &template,
            &installation.key_dir,
            &read_record(&first.record),
            (
                template.control_genesis_seq,
                template.control_genesis_hash.clone(),
            ),
            (head.event_count, head.head_hash),
        ),
    );
    assert_eq!(forged.log_index(), 2);
    assert!(template.control_genesis_seq < first.event_count);
    let report = installation.verify(&anchor, &installation.log_dir, None);
    assert_refused(&report, "checkpoint_regression");
    let detail = &report.errors[0].detail;
    assert_eq!(detail["log_index"], 2);
    assert_eq!(
        detail["expected_previous_event_count"],
        serde_json::json!(first.event_count)
    );
    assert_eq!(
        detail["actual_previous_event_count"],
        serde_json::json!(template.control_genesis_seq)
    );
    assert_eq!(report.verified_through_log_index, Some(1));
    assert_eq!(report.signatures_verified, 1);
}

#[test]
fn record_with_other_signers_than_required_is_refused() {
    let installation = Installation::new(TENANT);
    let first = installation.create().unwrap();
    let anchor = installation.anchor("signers", TENANT, &first.namespace_uuid);
    append_notes(&installation, &["beta"]);
    let single = SignedRecord::parse(
        forge_checkpoint(
            &installation,
            &installation.key_dir,
            &read_record(&first.record),
        )
        .as_bytes(),
    )
    .unwrap();
    let statement = String::from_utf8(single.statement_bytes().to_vec()).unwrap();
    let signature = (
        single.signatures()[0].key_id.clone(),
        single.signatures()[0].signature.clone(),
    );

    // A checkpoint is signed by its signing key, once: a second entry, even a
    // valid one of the same key, is not what the statement requires.
    plant_record(
        &installation.log_dir,
        &envelope(&statement, &[signature.clone(), signature]),
    );
    let report = installation.verify(&anchor, &installation.log_dir, None);
    assert_refused(&report, "signature_invalid");
    let detail = &report.errors[0].detail;
    assert_eq!(
        detail["required_signers"],
        serde_json::json!([installation.key_id])
    );
    assert_eq!(
        detail["actual_signers"],
        serde_json::json!([installation.key_id, installation.key_id])
    );
    assert_eq!(report.verified_through_log_index, Some(1));
}

/// The declared time that `create` writes for an instant.
fn declared_time(instant: chrono::DateTime<chrono::Utc>) -> String {
    instant.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn wait_until(instant: chrono::DateTime<chrono::Utc>) {
    while let Ok(remaining) = (instant - chrono::Utc::now()).to_std() {
        if remaining.is_zero() {
            break;
        }
        std::thread::sleep(remaining.min(std::time::Duration::from_millis(20)));
    }
}

/// What an attempt measured when `create` declared its time outside the
/// window of planted names.
struct MissedWindow {
    /// Time spent planting the names of the window.
    planting: std::time::Duration,
    /// Time `create` took in the planted directory before declaring a time.
    lead: std::time::Duration,
    /// Where the declared time fell.
    declared: String,
}

/// Plant every name that the second record of a new installation can take
/// during `window` milliseconds of declared times, starting `opens_in` from
/// now, and ask for that record in the middle of the window.
///
/// `Ok` means that the refusal was observed, with every assertion on it. An
/// `Err` is a run that created its record, proven by its declared time to
/// have fallen outside the window.
fn refusal_over_a_planted_name(
    window: u32,
    opens_in: std::time::Duration,
) -> Result<(), MissedWindow> {
    let installation = Installation::new(TENANT);
    let first = installation.create().unwrap();
    // The database as the first record signed it: there, `create` reads the
    // same directory with the same key and finds nothing to sign.
    let signed_database = copy_database(&installation, "as-signed");
    append_notes(&installation, &["beta"]);
    let store = open_store(&installation.database, TENANT);
    let integrity = store.integrity().unwrap();
    drop(store);
    let template = first_statement(&installation);

    let planting = std::time::Instant::now();
    let start = chrono::Utc::now() + chrono::Duration::from_std(opens_in).unwrap();
    let width = chrono::Duration::milliseconds(i64::from(window));
    let end = start + width;
    let mut planted = Vec::new();
    for offset in 0..i64::from(window) {
        let statement = serde_json::to_string(&CheckpointStatement {
            log_index: 2,
            previous_statement_sha256: first.statement_sha256.clone(),
            created_at: declared_time(start + chrono::Duration::milliseconds(offset)),
            event_count: integrity.event_count,
            head_hash: integrity.head_hash.clone(),
            previous_event_count: first.event_count,
            previous_head_hash: first.head_hash.clone(),
            ..template.clone()
        })
        .unwrap();
        let name = format!("cp-{:020}-{}.json", 2, sha256_hex(statement.as_bytes()));
        write_private(&installation.log_dir.join(&name), b"");
        planted.push(name);
    }
    let planting = planting.elapsed();
    let before = entry_names(&installation.log_dir);

    // A rehearsal in the planted directory. It stops at `unchanged`, right
    // before the point where a run with something to sign declares its time:
    // its duration is the lead of that declaration over the start of the run.
    let rehearsal = std::time::Instant::now();
    let rehearsed = installation
        .create_for(TENANT, &signed_database, &installation.log_dir)
        .unwrap();
    let lead = rehearsal.elapsed();
    assert_eq!(rehearsed.status, CreateStatus::Unchanged);
    assert_eq!(rehearsed.incomplete_records.len(), planted.len());
    assert_eq!(entry_names(&installation.log_dir), before);

    // Start the run so that it declares its time in the middle of the window,
    // or at once when that moment has passed.
    wait_until(start + width / 2 - chrono::Duration::from_std(lead).unwrap());
    let refused = match installation.create() {
        Err(refused) => refused,
        Ok(created) => {
            // The record was created: that is only acceptable when its
            // declared time really fell outside the window.
            let record = read_record(&created.record);
            let Statement::Checkpoint(statement) = record.statement() else {
                panic!("create signs a checkpoint");
            };
            let declared = statement.created_at.clone();
            assert!(
                declared < declared_time(start) || declared >= declared_time(end),
                "a record declared at {declared} inside the planted window was created over an existing name"
            );
            return Err(MissedWindow {
                planting,
                lead,
                declared: format!(
                    "{declared}, outside [{}, {})",
                    declared_time(start),
                    declared_time(end)
                ),
            });
        }
    };
    assert_eq!(refused.code(), "checkpoint_record_exists");
    assert_eq!(refused.exit_code(), 1);
    assert!(refused.error.action.contains("Retry"));
    let name = refused.error.detail["record"].as_str().unwrap().to_owned();
    assert!(planted.contains(&name), "{name}");
    // The existing file is preserved and nothing else was written.
    assert_eq!(entry_names(&installation.log_dir), before);
    assert_eq!(
        std::fs::read(installation.log_dir.join(&name)).unwrap(),
        b""
    );

    // A new attempt declares a new time, hence a new name: once the window
    // has passed, the record is created at the same log index.
    wait_until(end);
    let second = installation.create().unwrap();
    assert_eq!(second.status, CreateStatus::Created);
    assert_eq!(second.log_index, 2);
    assert!(!planted.contains(&file_name_of(&second.record)));
    assert_eq!(second.incomplete_records.len(), planted.len());
    assert_eq!(
        std::fs::read(installation.log_dir.join(&name)).unwrap(),
        b""
    );
    Ok(())
}

/// A record is named after the SHA-256 of its statement, and the statement
/// carries the time declared by the signer in milliseconds. Without a clock to
/// set, the test occupies every name that the next record can take during a
/// window of declared times, then asks for that record inside the window.
///
/// No speed of the host is assumed. The window lies ahead of the run instead
/// of starting before its names are planted, and the run is aimed at its
/// middle from a rehearsal of the same command in the same directory. A run
/// that misses proves it by the time it declared; the next attempt then plants
/// a wider window, as far ahead as the planting and the rehearsal just took.
/// The test passes only after it observed the refusal.
#[test]
fn an_existing_file_with_the_name_of_the_new_record_is_reported_and_preserved() {
    const FIRST_WINDOW_MILLISECONDS: u32 = 2_048;
    // The planted names and the head record leave room for one more record
    // under the directory bound: the run reaches the write.
    const WIDEST_WINDOW_MILLISECONDS: u32 = MAX_LOG_RECORDS - 2;
    const _: () = assert!(FIRST_WINDOW_MILLISECONDS <= WIDEST_WINDOW_MILLISECONDS);
    const ATTEMPTS: usize = 5;

    let mut window = FIRST_WINDOW_MILLISECONDS;
    // Nothing was measured yet: the first window opens half its width ahead.
    let mut opens_in = std::time::Duration::from_millis(u64::from(window) / 2);
    let mut misses = Vec::new();
    for _ in 0..ATTEMPTS {
        let missed = match refusal_over_a_planted_name(window, opens_in) {
            Ok(()) => return,
            Err(missed) => missed,
        };
        // Planting and reading cost in proportion to the number of names. The
        // next window opens late enough for its names to be planted and for
        // the rehearsal to end before the run has to start, with half as much
        // again to spare.
        let wider = window.saturating_mul(2).min(WIDEST_WINDOW_MILLISECONDS);
        let growth = f64::from(wider) / f64::from(window);
        let before_the_run = (missed.planting + 2 * missed.lead).mul_f64(1.5 * growth);
        let miss = format!(
            "a window of {window} ms opening in {opens_in:?} was planted in {:?}, the rehearsal took {:?} and the record was declared at {}",
            missed.planting, missed.lead, missed.declared
        );
        println!("missed, trying a wider window: {miss}");
        misses.push(miss);
        opens_in =
            before_the_run.saturating_sub(std::time::Duration::from_millis(u64::from(wider) / 2));
        window = wider;
    }
    panic!(
        "create declared its time outside the planted window {ATTEMPTS} times: this host is too slow or too irregular for the test, which says nothing about the product. {misses:#?}"
    );
}

#[test]
fn create_refuses_to_extend_a_head_that_its_key_did_not_sign() {
    let installation = Installation::new(TENANT);
    installation.create().unwrap();
    append_notes(&installation, &["beta"]);
    let second = installation.create().unwrap();

    // A copy of the log whose head carries the right statement, under the
    // right name, with a signature that is not one of the key.
    let log = copy_log(&installation, &installation.log_dir, "unsigned-head");
    let pristine = std::fs::read_to_string(&second.record).unwrap();
    let record = read_record(&second.record);
    let signature = &record.signatures()[0].signature;
    let replacement = if signature.starts_with('A') { "B" } else { "A" };
    let altered = pristine.replacen(
        &format!("\"signature\":\"{}", &signature[..1]),
        &format!("\"signature\":\"{replacement}"),
        1,
    );
    assert_ne!(altered, pristine);
    std::fs::write(log.join(record.file_name()), &altered).unwrap();
    append_notes(&installation, &["gamma"]);
    let before = record_files(&log);

    let result = installation.create_for(TENANT, &installation.database, &log);
    let refused = result.expect_err("a head that the key did not sign must not be extended");
    assert_eq!(refused.code(), "signature_invalid");
    assert_eq!(refused.exit_code(), 2);
    assert_eq!(refused.error.detail["log_index"], 2);
    assert_eq!(record_files(&log), before);
}

/// A copy of the succession log with one forged rotation record at the log
/// index after `previous`, and the approval of it against `anchor`.
fn approve_forged_rotation(
    succession: &Succession,
    name: &str,
    anchor: &Path,
    record: &str,
    public_key: &Path,
    expect_key_id: &str,
    unendorsed_recovery: bool,
) -> (Result<AnchorOutcome, Failure>, PathBuf) {
    let installation = &succession.installation;
    let log = copy_log(installation, &installation.log_dir, name);
    plant_record(&log, record);
    let out = installation.holder.join(format!("{name}.anchor.json"));
    let result = approve(
        anchor,
        &log,
        public_key,
        expect_key_id,
        unendorsed_recovery,
        &out,
    );
    (result, out)
}

#[test]
fn rotation_without_proof_of_possession_is_not_approved() {
    let succession = Succession::new();
    let installation = &succession.installation;
    // Endorsed by the pinned retiring key, but the entry named after the
    // successor is not a signature of the successor.
    let statement = rotation_statement(
        &succession.first.namespace_uuid,
        &read_record(&succession.second.record),
        &installation.key_id,
        &succession.new_key_id,
        "retiring_key",
    );
    let endorsement = sign_with_key_file(&installation.key_dir, &statement);
    let not_possessed = (succession.new_key_id.clone(), endorsement.1.clone());
    let (result, out) = approve_forged_rotation(
        &succession,
        "no-possession",
        &succession.anchor,
        &envelope(&statement, &[endorsement, not_possessed]),
        &succession.new_public_key,
        &succession.new_key_id,
        false,
    );
    let refused = result.expect_err("a successor that did not sign must not be approved");
    assert_eq!(refused.code(), "signature_invalid");
    assert_eq!(refused.exit_code(), 2);
    assert_eq!(
        refused.error.detail["key_id"],
        serde_json::json!(succession.new_key_id)
    );
    assert!(!out.exists());
}

#[test]
fn rotation_that_retires_another_key_than_the_current_one_is_not_approved() {
    let succession = Succession::new();
    let installation = &succession.installation;
    let third_key = init_key(&installation.root().join("keys").join("key-3")).unwrap();
    let statement = rotation_statement(
        &succession.first.namespace_uuid,
        &read_record(&succession.second.record),
        &third_key.key_id,
        &succession.new_key_id,
        "none",
    );
    let (result, out) = approve_forged_rotation(
        &succession,
        "other-retiring",
        &succession.anchor,
        &envelope(
            &statement,
            &[sign_with_key_file(&succession.new_key_dir, &statement)],
        ),
        &succession.new_public_key,
        &succession.new_key_id,
        true,
    );
    let refused = result.expect_err("a rotation must retire the current key of the anchor");
    assert_eq!(refused.code(), "key_id_mismatch");
    assert_eq!(refused.exit_code(), 2);
    assert_eq!(
        refused.error.detail["between"],
        serde_json::json!(["trust_anchor_current_key", "rotation_record"])
    );
    assert!(!out.exists());
}

#[test]
fn rotation_with_a_signature_too_many_is_not_approved() {
    let succession = Succession::new();
    let installation = &succession.installation;
    // An unendorsed statement requires the successor alone.
    let statement = rotation_statement(
        &succession.first.namespace_uuid,
        &read_record(&succession.second.record),
        &installation.key_id,
        &succession.new_key_id,
        "none",
    );
    let (result, out) = approve_forged_rotation(
        &succession,
        "extra-signer",
        &succession.anchor,
        &envelope(
            &statement,
            &[
                sign_with_key_file(&installation.key_dir, &statement),
                sign_with_key_file(&succession.new_key_dir, &statement),
            ],
        ),
        &succession.new_public_key,
        &succession.new_key_id,
        true,
    );
    let refused = result.expect_err("a rotation must carry exactly the required signatures");
    assert_eq!(refused.code(), "signature_invalid");
    assert_eq!(
        refused.error.detail["required_signers"],
        serde_json::json!([succession.new_key_id])
    );
    assert!(!out.exists());
}

#[test]
fn retired_key_is_not_pinned_again_by_a_later_rotation() {
    let succession = Succession::new();
    let installation = &succession.installation;
    let rotated = succession.rotate(succession.endorsing()).unwrap();
    let approved = installation.holder.join("approved.anchor-3.json");
    approve_with(
        &succession.anchor,
        &installation.log_dir,
        Some(&installation.database),
        &succession.new_public_key,
        &succession.new_key_id,
        false,
        &approved,
    )
    .unwrap();

    // The new current key hands the log back to the key it replaced, with
    // both signatures valid.
    let statement = rotation_statement(
        &succession.first.namespace_uuid,
        &read_record(&rotated.record),
        &succession.new_key_id,
        &installation.key_id,
        "retiring_key",
    );
    let (result, out) = approve_forged_rotation(
        &succession,
        "back-to-retired",
        &approved,
        &envelope(
            &statement,
            &[
                sign_with_key_file(&succession.new_key_dir, &statement),
                sign_with_key_file(&installation.key_dir, &statement),
            ],
        ),
        &installation.holder.join("succession.public-key.json"),
        &installation.key_id,
        false,
    );
    let refused = result.expect_err("a retired key must not be pinned a second time");
    assert_eq!(refused.code(), "key_id_mismatch");
    assert_eq!(
        refused.error.detail["key_id"],
        serde_json::json!(installation.key_id)
    );
    assert!(!out.exists());
}

#[test]
fn anchor_at_its_key_bound_refuses_another_rotation() {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use ring::signature::KeyPair;
    let succession = Succession::new();
    let installation = &succession.installation;
    succession.rotate(succession.endorsing()).unwrap();

    // An anchor that pins as many keys as it may: retired keys that never
    // signed anything, then the current key.
    let mut anchor: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&succession.anchor).unwrap()).unwrap();
    let current = anchor["keys"][0].clone();
    let mut keys = Vec::with_capacity(MAX_ANCHOR_KEYS);
    for number in 1..MAX_ANCHOR_KEYS {
        let mut seed = [0_u8; 32];
        seed[..8].copy_from_slice(&u64::try_from(number).unwrap().to_be_bytes());
        let pair = ring::signature::Ed25519KeyPair::from_seed_unchecked(&seed).unwrap();
        let public_key = pair.public_key().as_ref();
        keys.push(serde_json::json!({
            "key_id": sha256_hex(public_key),
            "algorithm": "ed25519",
            "public_key": URL_SAFE_NO_PAD.encode(public_key),
            "valid_from_log_index": 1,
            "valid_through_log_index": 0,
            "approval": "initial",
        }));
    }
    keys.push(current);
    anchor["keys"] = serde_json::Value::Array(keys);
    let full = installation.holder.join("full.anchor.json");
    write_private(&full, serde_json::to_string(&anchor).unwrap().as_bytes());

    // The full anchor is valid: it verifies the log up to the pending rotation.
    let report = installation.verify(&full, &installation.log_dir, None);
    assert_refused(&report, "untrusted_key");
    assert_eq!(report.verified_through_log_index, Some(2));

    let out = installation.holder.join("full.anchor-next.json");
    let refused = approve(
        &full,
        &installation.log_dir,
        &succession.new_public_key,
        &succession.new_key_id,
        false,
        &out,
    )
    .unwrap_err();
    assert_eq!(refused.code(), "anchor_key_limit");
    assert_eq!(refused.exit_code(), 1);
    assert_eq!(refused.error.detail["limit"], MAX_ANCHOR_KEYS);
    assert!(!out.exists());
}

#[test]
fn edited_trust_anchor_is_refused_and_never_repaired() {
    let history = History::new();
    let installation = &history.installation;
    let text = std::fs::read_to_string(&history.anchor).unwrap();
    let pristine: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(pristine["revision"], 2);
    let other_key = init_key(&installation.root().join("keys").join("other")).unwrap();
    let edited = |change: &dyn Fn(&mut serde_json::Value)| {
        let mut anchor = pristine.clone();
        change(&mut anchor);
        serde_json::to_string(&anchor).unwrap()
    };
    let check = |name: &str, content: &str| {
        let path = installation
            .holder
            .join(format!("edited-{name}.anchor.json"));
        write_private(&path, content.as_bytes());
        let verified = verify(&VerifyRequest {
            anchor: &path,
            source: RecordSource::Directory(&installation.log_dir),
            data: None,
            require_reference: false,
            report_out: None,
        });
        let out = installation
            .holder
            .join(format!("edited-{name}.anchor-next.json"));
        let advanced = advance(&path, &installation.log_dir, None, &out);
        assert!(
            !out.exists(),
            "{name}: an edited anchor must not be advanced"
        );
        (verified, advanced.map(|outcome| outcome.status))
    };

    // Control: the same content written again by another serializer, in
    // another member order, is the same anchor. Only the edits below refuse.
    let (verified, advanced) = check("control", &edited(&|_| {}));
    assert_eq!(verified.unwrap().verdict, Verdict::Verified);
    assert_eq!(advanced.unwrap(), AdvanceStatus::Unchanged);

    let other_public_key = other_key.public_key.clone();
    let other_pinned = serde_json::json!({
        "key_id": other_key.key_id,
        "algorithm": "ed25519",
        "public_key": other_key.public_key,
        "valid_from_log_index": 1,
        "valid_through_log_index": null,
        "approval": "initial",
    });
    let uppercase_namespace = pristine["namespace_uuid"].as_str().unwrap().to_uppercase();
    let cases: Vec<(&str, String)> = vec![
        (
            "substituted-key",
            edited(&|anchor| anchor["keys"][0]["public_key"] = other_public_key.clone().into()),
        ),
        (
            "second-current-key",
            edited(&|anchor| {
                anchor["keys"]
                    .as_array_mut()
                    .unwrap()
                    .push(other_pinned.clone())
            }),
        ),
        (
            "unknown-member",
            edited(&|anchor| anchor["note"] = "reviewed".into()),
        ),
        (
            "unlinked-revision",
            edited(&|anchor| anchor["previous_anchor_sha256"] = serde_json::Value::Null),
        ),
        (
            "position-above-reference",
            edited(&|anchor| anchor["reference"]["checkpoint"]["log_index"] = 3.into()),
        ),
        (
            "retained-out-of-order",
            edited(&|anchor| {
                anchor["reference"]["retained_checkpoints"] = serde_json::json!([{
                    "log_index": 2,
                    "event_count": 1,
                    "head_hash": "a".repeat(64),
                }])
            }),
        ),
        (
            "uppercase-namespace",
            edited(&|anchor| anchor["namespace_uuid"] = uppercase_namespace.clone().into()),
        ),
        (
            "missing-member",
            edited(&|anchor| {
                anchor.as_object_mut().unwrap().remove("updated_at");
            }),
        ),
        (
            "missing-key-window",
            edited(&|anchor| {
                anchor["keys"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("valid_through_log_index");
            }),
        ),
        (
            "duplicated-member",
            text.replacen("\"revision\":2,", "\"revision\":2,\"revision\":2,", 1),
        ),
        ("truncated", text[..text.len() / 2].to_owned()),
    ];
    for (name, content) in &cases {
        assert_ne!(content, &text, "{name}: the edit changed nothing");
        let (verified, advanced) = check(name, content);
        for failure in [verified.unwrap_err(), advanced.unwrap_err()] {
            assert_eq!(failure.code(), "anchor_invalid", "{name}: {failure}");
            assert_eq!(failure.exit_code(), 1, "{name}");
        }
    }
    // The revision held by the anchor holder is untouched.
    assert_eq!(std::fs::read_to_string(&history.anchor).unwrap(), text);
}

#[test]
fn signing_key_must_be_private_and_intact() {
    let installation = Installation::new(TENANT);
    installation.create().unwrap();
    let key_file = installation.key_dir.join("signing-key.pk8");
    let pristine = std::fs::read(&key_file).unwrap();
    append_notes(&installation, &["beta"]);
    let before = record_files(&installation.log_dir);
    let mode = |path: &Path, mode: u32| {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    };

    // A key file or a key directory that others can read is refused.
    for (path, exposed, private) in [
        (key_file.as_path(), 0o644, 0o600),
        (installation.key_dir.as_path(), 0o755, 0o700),
    ] {
        mode(path, exposed);
        for failure in [
            installation.create().unwrap_err(),
            show_key(&installation.key_dir, None).unwrap_err(),
        ] {
            assert_eq!(failure.code(), "key_unavailable", "{failure}");
            assert_eq!(failure.exit_code(), 1);
        }
        mode(path, private);
    }

    // A file that is no longer the key is refused without being quoted.
    let mut altered = pristine.clone();
    altered[20] ^= 0xff;
    for content in [altered.as_slice(), &pristine[..40]] {
        std::fs::write(&key_file, content).unwrap();
        let failure = installation.create().unwrap_err();
        assert_eq!(failure.code(), "key_invalid", "{failure}");
        assert_eq!(failure.exit_code(), 1);
        assert!(
            failure.error.detail.is_empty(),
            "{:?}",
            failure.error.detail
        );
        let rendered = serde_json::to_string(&failure.error).unwrap();
        assert!(!rendered.contains(&hex::encode(&content[16..40])));
    }
    assert_eq!(record_files(&installation.log_dir), before);

    // The intended key, restored, signs again.
    std::fs::write(&key_file, &pristine).unwrap();
    assert_eq!(installation.create().unwrap().log_index, 2);
}

#[test]
fn rotation_is_not_approved_from_a_directory_that_holds_a_renamed_record() {
    let succession = Succession::new();
    let installation = &succession.installation;
    let rotated = succession.rotate(succession.endorsing()).unwrap();
    assert_eq!(rotated.log_index, 3);

    // Beside an intact chain, a file with a record name holds a complete
    // record other than the one it announces: the second record, under
    // another hash. No interrupted write leaves that.
    let second_bytes = std::fs::read(&succession.second.record).unwrap();
    let stray_name = format!("cp-{:020}-{}.json", 2, "e".repeat(64));
    let stray = installation.log_dir.join(&stray_name);
    write_private(&stray, &second_bytes);

    // `verify` refuses the directory, beside the rotation that waits...
    let report = installation.verify(&succession.anchor, &installation.log_dir, None);
    assert_eq!(
        report.error_codes(),
        vec!["untrusted_key", "record_name_mismatch"],
        "{:?}",
        report.errors
    );

    // ...and the approval, which reads the same directory, refuses it too,
    // with or without a database. Nothing is written.
    for (name, database) in [
        ("renamed.anchor-records.json", None),
        (
            "renamed.anchor-database.json",
            Some(installation.database.as_path()),
        ),
    ] {
        let out = installation.holder.join(name);
        let result = approve_with(
            &succession.anchor,
            &installation.log_dir,
            database,
            &succession.new_public_key,
            &succession.new_key_id,
            false,
            &out,
        );
        let refused = result.expect_err(
            "a rotation must not be approved from a directory that holds a renamed record",
        );
        assert_eq!(refused.code(), "record_name_mismatch");
        assert_eq!(refused.exit_code(), 2);
        assert_eq!(
            refused.error.detail["records"],
            serde_json::json!([stray_name])
        );
        assert!(!out.exists());
    }
    // The file is never removed by the tool.
    assert_eq!(std::fs::read(&stray).unwrap(), second_bytes);

    // Once the anchor holder had it moved out by an explicit decision, the
    // same approval goes through.
    let aside = private_child(installation.root(), "set-aside");
    std::fs::rename(&stray, aside.join(&stray_name)).unwrap();
    let out = installation.holder.join("renamed.anchor-3.json");
    let approved = approve(
        &succession.anchor,
        &installation.log_dir,
        &succession.new_public_key,
        &succession.new_key_id,
        false,
        &out,
    )
    .unwrap();
    assert_eq!(approved.status, "rotation_approved");
    assert_eq!(approved.reference.unwrap().log_index, 3);
}

#[test]
fn rotation_approval_retains_or_confirms_the_pinned_positions() {
    let succession = Succession::new();
    let installation = &succession.installation;
    succession.rotate(succession.endorsing()).unwrap();

    // Approved from the records alone: the reference moves from the first
    // checkpoint to the rotation, past a second checkpoint that only the
    // retiring key vouches for. The first position stays compared.
    let without = installation.holder.join("approval-without-database.json");
    let approved = approve(
        &succession.anchor,
        &installation.log_dir,
        &succession.new_public_key,
        &succession.new_key_id,
        false,
        &without,
    )
    .unwrap();
    let reference = approved.reference.unwrap();
    assert_eq!(reference.log_index, 3);
    assert_eq!(reference.checkpoint.log_index, 2);
    assert_eq!(
        reference.retained_checkpoints.len(),
        1,
        "the position pinned before the rotation must be retained"
    );
    assert_eq!(reference.retained_checkpoints[0].log_index, 1);
    assert_eq!(
        reference.retained_checkpoints[0].head_hash,
        succession.first.head_hash
    );
    assert!(
        warned(&approved.warnings, "reference_advanced_without_database"),
        "{:?}",
        approved.warnings
    );
    let report = installation.verify(
        &without,
        &installation.log_dir,
        Some(&installation.database),
    );
    assert_eq!(report.verdict, Verdict::Verified, "{:?}", report.errors);
    assert_eq!(report.database.positions_compared, Some(2));

    // A database that does not hold a position signed before the rotation
    // refuses the approval, and nothing is written.
    let truncated = copy_database(installation, "approval-truncated");
    truncate_last_event(&truncated);
    let refused_out = installation.holder.join("approval-refused.json");
    let refused = approve_with(
        &succession.anchor,
        &installation.log_dir,
        Some(&truncated),
        &succession.new_public_key,
        &succession.new_key_id,
        false,
        &refused_out,
    )
    .expect_err("a database shorter than a signed position must refuse the approval");
    assert_eq!(refused.code(), "database_truncated");
    assert_eq!(refused.exit_code(), 2);
    assert_eq!(refused.error.detail["signed_at_log_index"], 2);
    assert!(!refused_out.exists());
}

/// The warnings about record files that are not complete records, in order.
fn incomplete_record_warnings(warnings: &[Warning]) -> Vec<&Warning> {
    warnings
        .iter()
        .filter(|warning| {
            matches!(
                warning.code,
                "incomplete_records_present" | "incomplete_record_above_head"
            )
        })
        .collect()
}

#[test]
fn rotation_approval_reports_incomplete_records_as_verify_does() {
    let succession = Succession::new();
    let installation = &succession.installation;
    let rotated = succession.rotate(succession.endorsing()).unwrap();
    assert_eq!(rotated.log_index, 3);
    let approve_to = |name: &str| {
        approve_with(
            &succession.anchor,
            &installation.log_dir,
            Some(&installation.database),
            &succession.new_public_key,
            &succession.new_key_id,
            false,
            &installation.holder.join(name),
        )
        .unwrap()
    };
    let codes = |warnings: &[Warning]| -> Vec<&'static str> {
        warnings.iter().map(|warning| warning.code).collect()
    };

    // Below the last complete record, a record file that is not a complete
    // record: what an interrupted write leaves when a later attempt wrote the
    // record under another name. The approval goes through, and says so.
    let second_bytes = std::fs::read(&succession.second.record).unwrap();
    let debris = &second_bytes[..second_bytes.len() / 3];
    let below = installation
        .log_dir
        .join(format!("cp-{:020}-{}.json", 2, "d".repeat(64)));
    write_private(&below, debris);
    let with_debris_below = approve_to("incomplete.anchor-below.json");
    assert_eq!(with_debris_below.status, "rotation_approved");

    // The same above the rotation, which is the last complete record. An
    // interrupted write leaves this, and so does truncating a record signed
    // after the rotation: the reference pinned by this approval stands below
    // it and would not detect that.
    let above = installation
        .log_dir
        .join(format!("cp-{:020}-{}.json", 4, "d".repeat(64)));
    write_private(&above, debris);
    let approved = approve_to("incomplete.anchor-above.json");
    assert_eq!(approved.status, "rotation_approved");
    assert_eq!(approved.reference.as_ref().unwrap().log_index, 3);
    assert_eq!(
        (
            codes(&with_debris_below.warnings),
            codes(&approved.warnings)
        ),
        (
            vec!["incomplete_records_present"],
            vec!["incomplete_records_present", "incomplete_record_above_head"]
        ),
        "an approval must report the incomplete records of the directory it read, and that one stands above the last complete record"
    );

    // These are the warnings of `verify` for the same directory, word for
    // word: both commands read every record and say the same about it.
    let report = installation.verify(&succession.anchor, &installation.log_dir, None);
    assert_eq!(report.incomplete_records.len(), 2);
    assert_eq!(incomplete_record_warnings(&report.warnings).len(), 2);
    assert_eq!(
        incomplete_record_warnings(&approved.warnings),
        incomplete_record_warnings(&report.warnings)
    );

    // Neither file was touched.
    assert_eq!(std::fs::read(&below).unwrap(), debris);
    assert_eq!(std::fs::read(&above).unwrap(), debris);
}

#[test]
fn interrupted_rotation_resumes_idempotently() {
    let succession = Succession::new();
    let installation = &succession.installation;

    // A rotation needs an existing log with at least one record.
    let nowhere = rotate_key(&RotateRequest {
        checkpoint_dir: &installation.root().join("no-checkpoints"),
        new_key_dir: &succession.new_key_dir,
        retiring: succession.endorsing(),
    })
    .unwrap_err();
    assert_eq!(nowhere.code(), "checkpoint_log_unavailable");
    assert!(!installation.root().join("no-checkpoints").exists());
    let empty = private_child(installation.root(), "empty-checkpoints");
    let nothing = rotate_key(&RotateRequest {
        checkpoint_dir: &empty,
        new_key_dir: &succession.new_key_dir,
        retiring: succession.endorsing(),
    })
    .unwrap_err();
    assert_eq!(nothing.code(), "checkpoint_log_empty");
    assert_eq!(nothing.exit_code(), 1);
    assert_eq!(
        entry_names(&empty),
        Vec::<String>::new(),
        "a refused rotation must leave an empty directory exactly as it found it"
    );

    // The successor key exists, but the command was interrupted while writing
    // the rotation record: only a truncated file is left.
    let second_bytes = std::fs::read(&succession.second.record).unwrap();
    let debris = installation
        .log_dir
        .join(format!("cp-{:020}-{}.json", 3, "d".repeat(64)));
    write_private(&debris, &second_bytes[..second_bytes.len() / 3]);

    let rotated = succession.rotate(succession.endorsing()).unwrap();
    assert_eq!(rotated.status, RotateStatus::Rotated);
    assert_eq!(rotated.log_index, 3);
    assert_eq!(rotated.incomplete_records.len(), 1);
    let files = record_files(&installation.log_dir).len();

    // The record is durable but its report was lost: re-running is a no-op.
    let again = succession.rotate(succession.endorsing());
    let again = again.expect("re-running a durable rotation must not fail");
    assert_eq!(again.status, RotateStatus::AlreadyRotated);
    assert_eq!(again.statement_sha256, rotated.statement_sha256);
    assert_eq!(again.record, rotated.record);
    assert_eq!(record_files(&installation.log_dir).len(), files);

    append_notes(installation, &["gamma"]);
    assert_eq!(succession.create_with_new_key().unwrap().log_index, 4);
}

// ---------------------------------------------------------------------------
// Records at and below a trusted record
// ---------------------------------------------------------------------------

/// A proof bundle written by hand from complete records, as whoever holds
/// records can write one without the tool.
fn write_bundle(path: &Path, namespace: &str, records: &[&SignedRecord]) {
    let entries: Vec<String> = records
        .iter()
        .map(|record| String::from_utf8(record.to_bytes().unwrap()).unwrap())
        .collect();
    let bundle = format!(
        "{{\"format\":\"ai-guard.audit.checkpoint-proof.v1\",\"tenant_id\":\"{TENANT}\",\"namespace_uuid\":\"{namespace}\",\"first_log_index\":{},\"last_log_index\":{},\"entries\":[{}],\"exported_at\":\"2001-01-01T00:00:00.000Z\",\"notice\":\"written by hand\"}}\n",
        records[0].log_index(),
        records[records.len() - 1].log_index(),
        entries.join(",")
    );
    write_private(path, bundle.as_bytes());
}

fn verify_bundle(anchor: &Path, bundle: &Path, database: Option<&Path>) -> VerificationReport {
    verify(&VerifyRequest {
        anchor,
        source: RecordSource::Bundle(bundle),
        data: database,
        require_reference: false,
        report_out: None,
    })
    .unwrap()
}

/// What a verification answered, in one comparable line.
fn answer(report: &VerificationReport) -> String {
    format!("{:?} {:?}", report.verdict, report.error_codes())
}

/// Three checkpoints and an anchor whose reference is the third: the first
/// two records stand below the reference.
struct Referenced {
    history: History,
    third: CreateOutcome,
    /// Anchor with the third record as reference.
    anchor: PathBuf,
}

impl Referenced {
    fn new() -> Self {
        let history = History::new();
        append_notes(&history.installation, &["delta"]);
        let third = history.installation.create().unwrap();
        assert_eq!(third.log_index, 3);
        let anchor = history.installation.holder.join("referenced.anchor-3.json");
        let advanced = advance(
            &history.anchor,
            &history.installation.log_dir,
            Some(&history.installation.database),
            &anchor,
        )
        .unwrap();
        assert_eq!(
            advanced.status,
            AdvanceStatus::Advanced,
            "{:?}",
            advanced.verification.errors
        );
        Self {
            history,
            third,
            anchor,
        }
    }

    /// Another record at log index 2, linked to the first record and signed
    /// with the pinned key file: what a second writer, or the key holder, can
    /// leave beside the second record.
    fn second_by_the_pinned_key(&self) -> String {
        let installation = &self.history.installation;
        let first = &self.history.first;
        forge_checkpoint_with(
            &first_statement(installation),
            &installation.key_dir,
            &read_record(&first.record),
            (first.event_count, first.head_hash.clone()),
            (first.event_count + 1, "f".repeat(64)),
        )
    }

    /// A canonical record at log index 2 for `tenant`, linked to the first
    /// record, recalling its position, and signed by a key that no anchor
    /// pins: what anyone who read the first record can write.
    fn second_by_an_unpinned_key(&self, tenant: &str) -> String {
        let installation = &self.history.installation;
        let first = &self.history.first;
        let key_dir = installation
            .root()
            .join("keys")
            .join(format!("unpinned-{tenant}"));
        if !key_dir.exists() {
            init_key(&key_dir).unwrap();
        }
        forge_checkpoint_with(
            &CheckpointStatement {
                tenant_id: tenant.into(),
                ..first_statement(installation)
            },
            &key_dir,
            &read_record(&first.record),
            (first.event_count, first.head_hash.clone()),
            (first.event_count + 1, "b".repeat(64)),
        )
    }

    /// A copy of the checkpoint directory in which what stands below the
    /// third record is not the chain that record commits to.
    fn altered_below(&self, alteration: Below, name: &str) -> PathBuf {
        let installation = &self.history.installation;
        let second = file_name_of(&self.history.second.record);
        let directory = copy_log(installation, &installation.log_dir, name);
        match alteration {
            Below::Fork => {
                plant_record(&directory, &self.second_by_the_pinned_key());
            }
            Below::AnotherRecord => {
                std::fs::remove_file(directory.join(&second)).unwrap();
                plant_record(&directory, &self.second_by_an_unpinned_key("tenant-b"));
            }
            Below::Missing => std::fs::remove_file(directory.join(&second)).unwrap(),
            Below::Renamed => write_private(
                &directory.join(format!("cp-{:020}-{}.json", 2, "e".repeat(64))),
                &std::fs::read(&self.history.second.record).unwrap(),
            ),
        }
        directory
    }
}

/// What stands at log index 2, below a trusted third record.
#[derive(Clone, Copy)]
enum Below {
    /// A second complete record beside the genuine one.
    Fork,
    /// A canonical record of another tenant, signed by an unpinned key, in
    /// place of the genuine one.
    AnotherRecord,
    /// Nothing, above the first record.
    Missing,
    /// The genuine record, and its bytes again under another record name.
    Renamed,
}

#[test]
fn records_below_the_anchor_reference_are_refused_unless_they_chain_to_it() {
    let fixture = Referenced::new();
    let installation = &fixture.history.installation;
    let forked = fixture.altered_below(Below::Fork, "below-forked");
    let replaced = fixture.altered_below(Below::AnotherRecord, "below-replaced");
    let holed = fixture.altered_below(Below::Missing, "below-holed");
    assert_eq!(
        (
            record_files(&forked).len(),
            record_files(&replaced).len(),
            record_files(&holed).len()
        ),
        (4, 3, 2)
    );

    // Each directory holds the accepted third record. Whatever the command and
    // with or without a database, none of them may be answered as verified or
    // unchanged: every answer is collected, so that one failure names them all.
    let mut observed = Vec::new();
    let mut expected = Vec::new();
    for (case, anchor, directory, code) in [
        ("a fork", &fixture.anchor, &forked, "log_forked"),
        (
            "another record",
            &fixture.anchor,
            &replaced,
            "log_diverged_from_reference",
        ),
        (
            "a missing record above an older one",
            &fixture.anchor,
            &holed,
            "log_discontinuous",
        ),
        (
            "the reference record itself missing above an older one",
            &fixture.history.anchor,
            &holed,
            "log_discontinuous",
        ),
    ] {
        for (with, database) in [
            ("records only", None),
            ("with the database", Some(installation.database.as_path())),
        ] {
            let report = installation.verify(anchor, directory, database);
            observed.push((format!("verify, {case}, {with}"), answer(&report)));
            expected.push((
                format!("verify, {case}, {with}"),
                format!("Refused [\"{code}\"]"),
            ));
            let out = installation
                .holder
                .join(format!("below.anchor-{}.json", observed.len()));
            let outcome = advance(anchor, directory, database, &out).unwrap();
            observed.push((
                format!("anchor advance, {case}, {with}"),
                format!(
                    "{:?} {:?} written={}",
                    outcome.status,
                    outcome.verification.error_codes(),
                    out.exists()
                ),
            ));
            expected.push((
                format!("anchor advance, {case}, {with}"),
                format!("Refused [\"{code}\"] written=false"),
            ));
        }
    }
    assert_eq!(
        observed, expected,
        "no command may answer verified or unchanged over a directory that holds, at or below the anchor reference, a complete record that it did not attach to the reference"
    );

    // Each refusal names the position, and the reference itself stays the
    // trusted position of the report.
    let report = installation.verify(&fixture.anchor, &forked, None);
    assert_eq!(report.errors[0].detail["log_index"], 2);
    assert_eq!(report.verified_through_log_index, Some(3));
    assert_eq!(report.entries_checked, 0);
    let report = installation.verify(&fixture.anchor, &replaced, None);
    let detail = &report.errors[0].detail;
    assert_eq!(detail["log_index"], 2);
    assert_eq!(detail["reference_log_index"], 3);
    assert_eq!(
        detail["reference_statement_sha256"],
        serde_json::json!(fixture.third.statement_sha256)
    );
    assert_eq!(
        detail["expected_statement_sha256"],
        serde_json::json!(fixture.history.second.statement_sha256)
    );
    assert_ne!(
        detail["actual_statement_sha256"],
        detail["expected_statement_sha256"]
    );
    let report = installation.verify(&fixture.anchor, &holed, None);
    let detail = &report.errors[0].detail;
    assert_eq!(detail["missing_log_index"], 2);
    assert_eq!(detail["unattached_log_index"], 1);
    assert_eq!(detail["reference_log_index"], 3);

    // What was already refused stays refused with the same codes: a reference
    // at the altered position, and no reference at all.
    for (anchor, directory, code) in [
        (&fixture.history.anchor, &forked, "log_forked"),
        (&fixture.history.initial_anchor, &forked, "log_forked"),
        (
            &fixture.history.anchor,
            &replaced,
            "log_diverged_from_reference",
        ),
        (&fixture.history.initial_anchor, &replaced, "scope_mismatch"),
        (&fixture.history.initial_anchor, &holed, "log_discontinuous"),
    ] {
        assert_refused(&installation.verify(anchor, directory, None), code);
    }

    // Partial material stays legitimate: the records from the reference up, or
    // from any index below it, as long as nothing stands below a gap.
    let records = record_files(&installation.log_dir);
    assert_eq!(records.len(), 3);
    for (name, kept) in [
        ("partial-from-reference", &records[2..]),
        ("partial-from-second", &records[1..]),
    ] {
        let partial = private_child(installation.root(), name);
        for record in kept {
            std::fs::copy(record, partial.join(record.file_name().unwrap())).unwrap();
        }
        let report = installation.verify(&fixture.anchor, &partial, Some(&installation.database));
        assert_eq!(
            report.verdict,
            Verdict::Verified,
            "{name}: {:?}",
            report.errors
        );
        assert_eq!(report.verified_through_log_index, Some(3));
    }

    // The untouched directory verifies, and the report says how the records at
    // and below the reference were accepted: no signature is verified again.
    let report = installation.verify(
        &fixture.anchor,
        &installation.log_dir,
        Some(&installation.database),
    );
    assert_eq!(report.verdict, Verdict::Verified, "{:?}", report.errors);
    assert_eq!(report.entries_checked, 0);
    assert_eq!(report.signatures_verified, 0);
    assert!(
        report
            .limits
            .iter()
            .any(|limit| limit.contains("at and below the anchor reference")
                && limit.contains("statement hash")),
        "{:?}",
        report.limits
    );
}

#[test]
fn a_bundle_is_refused_when_its_records_below_the_reference_are_not_the_accepted_chain() {
    let fixture = Referenced::new();
    let installation = &fixture.history.installation;
    let namespace = &fixture.history.first.namespace_uuid;
    let first = read_record(&fixture.history.first.record);
    let second = read_record(&fixture.history.second.record);
    let third = read_record(&fixture.third.record);

    // A bundle carries one record per contiguous index and one scope. The
    // forged second record is of the same tenant, linked to the first record
    // and signed by a key that nobody pinned: no signature of it is ever
    // verified, since it stands below the reference.
    let forged = SignedRecord::parse(fixture.second_by_an_unpinned_key(TENANT).as_bytes()).unwrap();
    assert_eq!(forged.log_index(), 2);
    let bundle = installation.holder.join("below.forged.proof.json");
    write_bundle(&bundle, namespace, &[&first, &forged, &third]);

    let mut observed = Vec::new();
    for (with, database) in [
        ("records only", None),
        ("with the database", Some(installation.database.as_path())),
    ] {
        observed.push((
            format!("verify, {with}"),
            answer(&verify_bundle(&fixture.anchor, &bundle, database)),
        ));
        let out = installation
            .holder
            .join(format!("below.bundle-anchor-{}.json", observed.len()));
        let outcome = advance_anchor(&AnchorAdvanceRequest {
            anchor: &fixture.anchor,
            source: RecordSource::Bundle(&bundle),
            data: database,
            out: &out,
        })
        .unwrap();
        observed.push((
            format!("anchor advance, {with}"),
            format!(
                "{:?} {:?} written={}",
                outcome.status,
                outcome.verification.error_codes(),
                out.exists()
            ),
        ));
    }
    let refused = "Refused [\"log_diverged_from_reference\"]".to_owned();
    assert_eq!(
        observed,
        vec![
            ("verify, records only".to_owned(), refused.clone()),
            (
                "anchor advance, records only".to_owned(),
                format!("{refused} written=false")
            ),
            ("verify, with the database".to_owned(), refused.clone()),
            (
                "anchor advance, with the database".to_owned(),
                format!("{refused} written=false")
            ),
        ],
        "no command may answer verified or unchanged over a bundle that carries, below the anchor reference, a record that the reference does not chain to"
    );
    let report = verify_bundle(&fixture.anchor, &bundle, None);
    assert_eq!(report.errors[0].detail["log_index"], 2);
    assert_eq!(
        report.errors[0].detail["actual_statement_sha256"],
        serde_json::json!(forged.statement_sha256())
    );
    assert_eq!(report.verified_through_log_index, Some(3));

    // The same hand-written form with the genuine records verifies, whole or
    // from any index up to the reference: the refusal is about the record.
    for (name, records) in [
        ("whole", vec![&first, &second, &third]),
        ("from-second", vec![&second, &third]),
        ("from-reference", vec![&third]),
    ] {
        let genuine = installation
            .holder
            .join(format!("below.genuine-{name}.proof.json"));
        write_bundle(&genuine, namespace, &records);
        let report = verify_bundle(&fixture.anchor, &genuine, Some(&installation.database));
        assert_eq!(
            report.verdict,
            Verdict::Verified,
            "{name}: {:?}",
            report.errors
        );
        assert_eq!(report.verified_through_log_index, Some(3));
        assert_eq!(report.entries_checked, 0);
    }

    // Without a reference every record is verified by signature: the same
    // forged bundle is refused for its unpinned key.
    assert_refused(
        &verify_bundle(&fixture.history.initial_anchor, &bundle, None),
        "untrusted_key",
    );

    // A bundle cannot carry two records at one index: it is not a bundle.
    let forked = installation.holder.join("below.forked.proof.json");
    write_bundle(&forked, namespace, &[&first, &second, &forged, &third]);
    let failure = verify(&VerifyRequest {
        anchor: &fixture.anchor,
        source: RecordSource::Bundle(&forked),
        data: None,
        require_reference: false,
        report_out: None,
    })
    .unwrap_err();
    assert_eq!(failure.code(), "bundle_malformed");
    assert_eq!(failure.exit_code(), 2);
}

#[test]
fn rotation_is_not_approved_over_records_below_the_reference_that_do_not_chain_to_it() {
    let fixture = Referenced::new();
    let installation = &fixture.history.installation;
    let new_key_dir = installation.root().join("keys").join("key-2");
    let new_key = init_key(&new_key_dir).unwrap();
    let new_public_key = installation.holder.join("key-2.public-key.json");
    show_key(&new_key_dir, Some(&new_public_key)).unwrap();
    let rotated = rotate_key(&RotateRequest {
        checkpoint_dir: &installation.log_dir,
        new_key_dir: &new_key_dir,
        retiring: RetiringKey::Endorsing {
            key_dir: &installation.key_dir,
        },
    })
    .unwrap();
    assert_eq!(rotated.log_index, 4);

    // The rotation waits right after the reference, in directories and in a
    // bundle whose records below the reference are not the accepted chain.
    let forked = fixture.altered_below(Below::Fork, "approval-forked");
    let replaced = fixture.altered_below(Below::AnotherRecord, "approval-replaced");
    let holed = fixture.altered_below(Below::Missing, "approval-holed");
    let forged = SignedRecord::parse(fixture.second_by_an_unpinned_key(TENANT).as_bytes()).unwrap();
    let bundle = installation.holder.join("approval.forged.proof.json");
    write_bundle(
        &bundle,
        &fixture.history.first.namespace_uuid,
        &[
            &read_record(&fixture.history.first.record),
            &forged,
            &read_record(&fixture.third.record),
            &read_record(&rotated.record),
        ],
    );

    let mut observed = Vec::new();
    let mut expected = Vec::new();
    for (case, source, code) in [
        ("a fork", RecordSource::Directory(&forked), "log_forked"),
        (
            "another record",
            RecordSource::Directory(&replaced),
            "log_diverged_from_reference",
        ),
        (
            "a missing record above an older one",
            RecordSource::Directory(&holed),
            "log_discontinuous",
        ),
        (
            "a bundle with another record",
            RecordSource::Bundle(&bundle),
            "log_diverged_from_reference",
        ),
    ] {
        for (with, database) in [
            ("records only", None),
            ("with the database", Some(installation.database.as_path())),
        ] {
            let out = installation
                .holder
                .join(format!("approval.anchor-{}.json", observed.len()));
            let result = approve_rotation(&RotationApprovalRequest {
                anchor: &fixture.anchor,
                source,
                data: database,
                public_key: &new_public_key,
                expect_key_id: &new_key.key_id,
                unendorsed_recovery: false,
                out: &out,
            });
            observed.push((
                format!("{case}, {with}"),
                match result {
                    Ok(approved) => format!("{} written={}", approved.status, out.exists()),
                    Err(refused) => format!(
                        "{} exit={} written={}",
                        refused.code(),
                        refused.exit_code(),
                        out.exists()
                    ),
                },
            ));
            expected.push((
                format!("{case}, {with}"),
                format!("{code} exit=2 written=false"),
            ));
        }
    }
    assert_eq!(
        observed, expected,
        "a rotation must not be approved over material that holds, below the anchor reference, a record that the reference does not chain to"
    );

    // The untouched directory is approved.
    let out = installation.holder.join("approval.anchor-approved.json");
    let approved = approve_with(
        &fixture.anchor,
        &installation.log_dir,
        Some(&installation.database),
        &new_public_key,
        &new_key.key_id,
        false,
        &out,
    )
    .unwrap();
    assert_eq!(approved.status, "rotation_approved");
    assert_eq!(approved.reference.unwrap().log_index, 4);
    // The rotation is the last record: nothing was left unexamined.
    assert!(approved.warnings.is_empty(), "{:?}", approved.warnings);
}

#[test]
fn create_and_rotate_key_refuse_a_log_whose_records_do_not_chain_to_its_head() {
    let fixture = Referenced::new();
    let installation = &fixture.history.installation;
    // Something new to sign, and a successor key to hand the log over to.
    append_notes(installation, &["epsilon"]);
    let new_key_dir = installation.root().join("keys").join("key-2");
    init_key(&new_key_dir).unwrap();

    // Each directory has the third record as its head. Below it: a fork,
    // another record, a missing record above the first, or the bytes of the
    // second record again under another record name.
    let mut observed = Vec::new();
    let mut expected = Vec::new();
    let mut left_behind = Vec::new();
    for (case, alteration, code) in [
        ("a fork", Below::Fork, "log_forked"),
        ("another record", Below::AnotherRecord, "log_discontinuous"),
        (
            "a missing record above an older one",
            Below::Missing,
            "log_discontinuous",
        ),
        ("a renamed record", Below::Renamed, "record_name_mismatch"),
    ] {
        for command in ["create", "rotate-key"] {
            let name = format!("head-{}-{command}", observed.len());
            let directory = fixture.altered_below(alteration, &name);
            let before = entry_names(&directory);
            let result = match command {
                "create" => installation
                    .create_for(TENANT, &installation.database, &directory)
                    .map(|created| {
                        format!("{:?} at log index {}", created.status, created.log_index)
                    }),
                _ => rotate_key(&RotateRequest {
                    checkpoint_dir: &directory,
                    new_key_dir: &new_key_dir,
                    retiring: RetiringKey::Endorsing {
                        key_dir: &installation.key_dir,
                    },
                })
                .map(|rotated| format!("{:?} at log index {}", rotated.status, rotated.log_index)),
            };
            observed.push((
                format!("{command}, {case}"),
                match result {
                    Ok(written) => written,
                    Err(refused) => format!("{} exit={}", refused.code(), refused.exit_code()),
                },
            ));
            expected.push((format!("{command}, {case}"), format!("{code} exit=2")));
            let known: std::collections::BTreeSet<&String> = before.iter().collect();
            left_behind.push(
                entry_names(&directory)
                    .into_iter()
                    .filter(|entry| !known.contains(entry))
                    .collect::<Vec<String>>(),
            );
        }
    }
    assert_eq!(
        observed, expected,
        "neither create nor rotate-key may write after a head whose directory holds, below it, a record that the head does not chain to"
    );
    // Nothing was signed. The refusal is decided under the writer lock, after
    // every record was read: the lock file is all it leaves in a directory
    // that had none.
    assert_eq!(
        left_behind,
        vec![vec![".writer.lock".to_owned()]; 8],
        "a refusal must write no record"
    );

    // The refusal names the position.
    let directory = fixture.altered_below(Below::AnotherRecord, "head-detail-replaced");
    let refused = installation
        .create_for(TENANT, &installation.database, &directory)
        .unwrap_err();
    assert_eq!(refused.error.detail["log_index"], 2);
    assert_eq!(
        refused.error.detail["expected_statement_sha256"],
        serde_json::json!(fixture.history.second.statement_sha256)
    );
    let directory = fixture.altered_below(Below::Missing, "head-detail-holed");
    let refused = installation
        .create_for(TENANT, &installation.database, &directory)
        .unwrap_err();
    assert_eq!(refused.error.detail["missing_log_index"], 2);
    assert_eq!(refused.error.detail["unattached_log_index"], 1);

    // A head that signed the current audit head is not answered `unchanged`
    // either while a fork stands below it.
    let idle_database = copy_database(installation, "head-idle-database");
    truncate_last_event(&idle_database);
    let directory = fixture.altered_below(Below::Fork, "head-idle-forked");
    let refused = installation
        .create_for(TENANT, &idle_database, &directory)
        .expect_err("a fork below the head must not be answered as unchanged");
    assert_eq!(refused.code(), "log_forked");
    assert_eq!(refused.error.detail["log_index"], 2);

    // An untouched copy of the same log is extended, and one that starts at
    // its second record too: nothing stands below a gap.
    let intact = copy_log(installation, &installation.log_dir, "head-intact");
    let created = installation
        .create_for(TENANT, &installation.database, &intact)
        .unwrap();
    assert_eq!(
        (created.status, created.log_index),
        (CreateStatus::Created, 4)
    );
    let partial = private_child(installation.root(), "head-partial");
    for record in &record_files(&installation.log_dir)[1..] {
        std::fs::copy(record, partial.join(record.file_name().unwrap())).unwrap();
    }
    let created = installation
        .create_for(TENANT, &installation.database, &partial)
        .unwrap();
    assert_eq!(
        (created.status, created.log_index),
        (CreateStatus::Created, 4)
    );
}

// ---------------------------------------------------------------------------
// Control genesis
// ---------------------------------------------------------------------------

/// Append a second `control.schema.genesis` event of the same namespace after
/// the audit head and repoint `control_meta` to it, as an actor who writes the
/// stopped database and holds no key can. Returns the sequence of that event.
fn append_second_control_genesis(database: &Path, namespace: &str) -> u64 {
    let db = rusqlite::Connection::open(database).unwrap();
    let (seq, previous): (i64, String) = db
        .query_row(
            "SELECT seq,hash FROM events ORDER BY seq DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let mut event = Event::new("control.schema.genesis");
    event.data = serde_json::json!({
        "version": 2,
        "namespace_uuid": namespace,
        "previous_seq": seq,
        "previous_hash": previous,
    });
    let payload = serde_json::to_string(&event).unwrap();
    let received = declared_time(chrono::Utc::now());
    let next = seq + 1;
    let hash = chain_digest(next, &received, &previous, &payload);
    db.execute(
        "INSERT INTO events(seq,id,received_at,prev_hash,hash,payload,kind) VALUES(?1,?2,?3,?4,?5,?6,?7)",
        rusqlite::params![next, event.id, received, previous, hash, payload, event.kind],
    )
    .unwrap();
    db.execute(
        "UPDATE quota SET bytes=bytes+?1 WHERE id=1",
        [i64::try_from(payload.len()).unwrap()],
    )
    .unwrap();
    let repointed = db
        .execute(
            "UPDATE control_meta SET genesis_seq=?1,genesis_hash=?2 WHERE singleton=1",
            rusqlite::params![next, hash],
        )
        .unwrap();
    assert_eq!(repointed, 1);
    u64::try_from(next).unwrap()
}

/// The existing readers of the gateway accept the database: nothing but the
/// signed genesis can tell that `control_meta` was repointed.
fn assert_existing_readers_accept(database: &Path, event_count: u64) {
    let store = open_store(database, TENANT);
    let integrity = store.integrity().unwrap();
    assert!(
        integrity.valid && integrity.event_count == event_count,
        "precondition: the existing validators accept a second control genesis event; if they now refuse it, this regression needs another fixture: {integrity:?}"
    );
}

/// What a command that signs answered, in one comparable line.
fn signing_answer(result: Result<CreateOutcome, Failure>, directory: &Path) -> String {
    match result {
        Ok(created) => format!(
            "{:?} at log index {} directory={}",
            created.status,
            created.log_index,
            directory.exists()
        ),
        Err(refused) => format!(
            "{} exit={} directory={}",
            refused.code(),
            refused.exit_code(),
            directory.exists()
        ),
    }
}

#[test]
fn a_database_with_another_control_genesis_is_refused_and_never_signed() {
    let installation = Installation::new(TENANT);
    let first = installation.create().unwrap();
    let initial = installation.anchor("genesis", TENANT, &first.namespace_uuid);
    let anchor_one = installation.holder.join("genesis.anchor-2.json");
    assert_eq!(
        advance(&initial, &installation.log_dir, None, &anchor_one)
            .unwrap()
            .status,
        AdvanceStatus::Advanced
    );
    append_notes(&installation, &["beta"]);
    let second = installation.create().unwrap();
    assert_eq!(second.log_index, 2);
    // Advanced from the records alone: this revision retains the first
    // position, so that an advance with a database and no new record is the
    // `confirmed` path, in which no checkpoint is verified.
    let anchor = installation.holder.join("genesis.anchor-3.json");
    let advanced = advance(&anchor_one, &installation.log_dir, None, &anchor).unwrap();
    assert_eq!(advanced.status, AdvanceStatus::Advanced);
    assert_eq!(
        advanced
            .anchor
            .unwrap()
            .reference
            .unwrap()
            .retained_checkpoints
            .len(),
        1
    );
    let signed = first_statement(&installation);
    let report = installation.verify(&anchor, &installation.log_dir, Some(&installation.database));
    assert_eq!(report.verdict, Verdict::Verified, "{:?}", report.errors);
    assert_eq!(report.entries_checked, 0);

    let planted = append_second_control_genesis(&installation.database, &first.namespace_uuid);
    assert_eq!(planted, second.event_count + 1);
    assert_existing_readers_accept(&installation.database, planted);

    // The signed positions are all held and no record is new: the control
    // genesis that the reference record signed is the only thing that differs.
    let records = record_files(&installation.log_dir);
    let report = installation.verify(&anchor, &installation.log_dir, Some(&installation.database));
    let out = installation.holder.join("genesis.anchor-confirmed.json");
    let outcome = advance(
        &anchor,
        &installation.log_dir,
        Some(&installation.database),
        &out,
    )
    .unwrap();
    let observed = vec![
        ("verify --data", answer(&report)),
        (
            "anchor advance --data",
            format!(
                "{:?} {:?} written={}",
                outcome.status,
                outcome.verification.error_codes(),
                out.exists()
            ),
        ),
        (
            "create",
            signing_answer(installation.create(), &installation.log_dir),
        ),
    ];
    assert_eq!(
        observed,
        vec![
            ("verify --data", "Refused [\"scope_mismatch\"]".to_owned()),
            (
                "anchor advance --data",
                "Refused [\"scope_mismatch\"] written=false".to_owned()
            ),
            ("create", "scope_mismatch exit=2 directory=true".to_owned()),
        ],
        "a database whose control genesis is not the one that the trusted records signed must be refused by every command that compares it, and never signed"
    );
    assert_eq!(record_files(&installation.log_dir), records);

    let detail = &report.errors[0].detail;
    assert_eq!(detail["field"], "control_genesis");
    assert_eq!(
        detail["between"],
        serde_json::json!(["checkpoint_record", "database"])
    );
    assert_eq!(detail["signed_at_log_index"], 2);
    assert_eq!(
        detail["signed_control_genesis_seq"],
        serde_json::json!(signed.control_genesis_seq)
    );
    assert_eq!(
        detail["signed_control_genesis_hash"],
        serde_json::json!(signed.control_genesis_hash)
    );
    assert_eq!(
        detail["database_control_genesis_seq"],
        serde_json::json!(planted)
    );
    assert!(report.database.compared);
    assert_eq!(report.database.extends_checkpoint, None);
    let refused = installation.create().unwrap_err();
    assert_eq!(refused.error.detail["field"], "control_genesis");
    assert_eq!(
        refused.error.detail["between"],
        serde_json::json!(["checkpoint_log", "database"])
    );
    assert_eq!(refused.error.detail["signed_at_log_index"], 2);

    // The records themselves are what they were: without a database, the
    // verdict covers them only.
    let report = installation.verify(&anchor, &installation.log_dir, None);
    assert_eq!(report.verdict, Verdict::Verified, "{:?}", report.errors);
}

#[test]
fn a_control_genesis_newer_than_a_signed_position_is_refused_without_the_record_that_signed_it() {
    let succession = Succession::new();
    let installation = &succession.installation;
    let rotated = succession.rotate(succession.endorsing()).unwrap();
    assert_eq!(rotated.log_index, 3);
    let anchor = installation.holder.join("genesis.anchor-rotation.json");
    approve_with(
        &succession.anchor,
        &installation.log_dir,
        Some(&installation.database),
        &succession.new_public_key,
        &succession.new_key_id,
        false,
        &anchor,
    )
    .unwrap();

    // The auditor holds the rotation record alone. It is the anchor reference,
    // and a rotation signs no control genesis: no record of this material
    // names the genesis that was signed.
    let bundle = installation.holder.join("genesis.rotation.proof.json");
    let exported = export_bundle(&ExportRequest {
        checkpoint_dir: &installation.log_dir,
        out: &bundle,
        from_log_index: Some(3),
        through_log_index: None,
    })
    .unwrap();
    assert_eq!(exported.entries, 1);
    let report = verify_bundle(&anchor, &bundle, Some(&installation.database));
    assert_eq!(report.verdict, Verdict::Verified, "{:?}", report.errors);

    let planted =
        append_second_control_genesis(&installation.database, &succession.first.namespace_uuid);
    assert_eq!(planted, succession.second.event_count + 1);
    assert_existing_readers_accept(&installation.database, planted);

    // A control genesis is created once, before the first checkpoint: one
    // that stands after a signed position cannot be the one that was signed.
    let records = record_files(&installation.log_dir);
    let report = verify_bundle(&anchor, &bundle, Some(&installation.database));
    let continued = installation.root().join("genesis-continued");
    let observed = vec![
        ("verify --bundle --data", answer(&report)),
        (
            // The head is the rotation; the checkpoint below it signed the
            // genesis.
            "create after the rotation",
            signing_answer(succession.create_with_new_key(), &installation.log_dir),
        ),
        (
            // Only the rotation record is read: no signed genesis is known.
            "create --previous-record <rotation>",
            signing_answer(
                create_checkpoint(&CreateRequest {
                    data: &installation.database,
                    tenant_id: TENANT,
                    key_dir: &succession.new_key_dir,
                    checkpoint_dir: &continued,
                    new_log: false,
                    previous_record: Some(Path::new(&rotated.record)),
                }),
                &continued,
            ),
        ),
    ];
    assert_eq!(
        observed,
        vec![
            (
                "verify --bundle --data",
                "Refused [\"scope_mismatch\"]".to_owned()
            ),
            (
                "create after the rotation",
                "scope_mismatch exit=2 directory=true".to_owned()
            ),
            (
                "create --previous-record <rotation>",
                "scope_mismatch exit=2 directory=false".to_owned()
            ),
        ],
        "a database whose control genesis stands after a trusted signed position must be refused even when no record at hand names the signed genesis"
    );
    assert_eq!(record_files(&installation.log_dir), records);
    let detail = &report.errors[0].detail;
    assert_eq!(detail["field"], "control_genesis");
    assert_eq!(
        detail["between"],
        serde_json::json!(["trusted_signed_position", "database"])
    );
    assert_eq!(
        detail["signed_event_count"],
        serde_json::json!(succession.second.event_count)
    );
    assert_eq!(
        detail["database_control_genesis_seq"],
        serde_json::json!(planted)
    );
}

#[test]
fn checkpoints_that_sign_different_control_geneses_are_refused() {
    let installation = Installation::new(TENANT);
    let first = installation.create().unwrap();
    let initial = installation.anchor("geneses", TENANT, &first.namespace_uuid);
    let referenced = installation.holder.join("geneses.anchor-2.json");
    assert_eq!(
        advance(
            &initial,
            &installation.log_dir,
            Some(&installation.database),
            &referenced
        )
        .unwrap()
        .status,
        AdvanceStatus::Advanced
    );
    append_notes(&installation, &["beta"]);
    let store = open_store(&installation.database, TENANT);
    let head = store.integrity().unwrap();
    drop(store);
    let template = first_statement(&installation);
    let first_record = read_record(&first.record);

    // The holder of the pinned key signs a second checkpoint over the real
    // audit head, linked to the first record and recalling its position, that
    // names another control genesis.
    let forge = |control_genesis_seq: u64, control_genesis_hash: &str| {
        forge_checkpoint_with(
            &CheckpointStatement {
                control_genesis_seq,
                control_genesis_hash: control_genesis_hash.into(),
                ..template.clone()
            },
            &installation.key_dir,
            &first_record,
            (first.event_count, first.head_hash.clone()),
            (head.event_count, head.head_hash.clone()),
        )
    };
    // Another event at the sequence of the signed genesis, and an event that
    // did not exist when the first checkpoint was signed.
    let other_hash = forge(template.control_genesis_seq, &"e".repeat(64));
    let later = forge(head.event_count, &head.head_hash);
    assert!(head.event_count > first.event_count);
    let with_other_hash = copy_log(&installation, &installation.log_dir, "geneses-other-hash");
    let second = plant_record(&with_other_hash, &other_hash);
    let with_later = copy_log(&installation, &installation.log_dir, "geneses-later");
    let second_later = plant_record(&with_later, &later);
    assert_eq!((second.log_index(), second_later.log_index()), (2, 2));
    // The second record alone, as a partial export hands it to an auditor
    // whose reference is the first record.
    let later_alone = installation.holder.join("geneses.later.proof.json");
    write_bundle(&later_alone, &first.namespace_uuid, &[&second_later]);

    // Every signature verifies and no database is compared: only the records
    // can tell.
    let mut observed = vec![
        (
            "no reference, another hash",
            answer(&installation.verify(&initial, &with_other_hash, None)),
        ),
        (
            "reference at the first record, another hash",
            answer(&installation.verify(&referenced, &with_other_hash, None)),
        ),
        (
            "no reference, a later event",
            answer(&installation.verify(&initial, &with_later, None)),
        ),
        (
            "reference at the first record, a later event, second record alone",
            answer(&verify_bundle(&referenced, &later_alone, None)),
        ),
    ];
    let out = installation.holder.join("geneses.anchor-3.json");
    let outcome = advance(&referenced, &with_other_hash, None, &out).unwrap();
    observed.push((
        "anchor advance, another hash",
        format!(
            "{:?} {:?} written={}",
            outcome.status,
            outcome.verification.error_codes(),
            out.exists()
        ),
    ));
    let refused = "Refused [\"scope_mismatch\"]".to_owned();
    assert_eq!(
        observed,
        vec![
            ("no reference, another hash", refused.clone()),
            (
                "reference at the first record, another hash",
                refused.clone()
            ),
            ("no reference, a later event", refused.clone()),
            (
                "reference at the first record, a later event, second record alone",
                refused.clone()
            ),
            (
                "anchor advance, another hash",
                format!("{refused} written=false")
            ),
        ],
        "a checkpoint that names another control genesis than the checkpoints before it must be refused"
    );

    let report = installation.verify(&initial, &with_other_hash, None);
    let detail = &report.errors[0].detail;
    assert_eq!(detail["field"], "control_genesis");
    assert_eq!(
        detail["between"],
        serde_json::json!(["checkpoint_record", "earlier_checkpoint_record"])
    );
    assert_eq!(detail["log_index"], 2);
    assert_eq!(detail["earlier_log_index"], 1);
    assert_eq!(report.verified_through_log_index, Some(1));
    assert_eq!(report.signatures_verified, 1);
    let report = verify_bundle(&referenced, &later_alone, None);
    let detail = &report.errors[0].detail;
    assert_eq!(detail["field"], "control_genesis");
    assert_eq!(
        detail["between"],
        serde_json::json!(["checkpoint_record", "earlier_signed_position"])
    );
    assert_eq!(detail["log_index"], 2);
    assert_eq!(
        detail["earlier_signed_event_count"],
        serde_json::json!(first.event_count)
    );

    // The stated limit: an anchor carries no control genesis. When the
    // material holds no earlier checkpoint, another hash at a sequence that an
    // earlier position admits is told only by a database.
    let other_hash_alone = installation.holder.join("geneses.other-hash.proof.json");
    write_bundle(&other_hash_alone, &first.namespace_uuid, &[&second]);
    let report = verify_bundle(&referenced, &other_hash_alone, None);
    assert_eq!(report.verdict, Verdict::Verified, "{:?}", report.errors);
    let report = verify_bundle(&referenced, &other_hash_alone, Some(&installation.database));
    assert_refused(&report, "scope_mismatch");
    assert_eq!(
        report.errors[0].detail["between"],
        serde_json::json!(["checkpoint_record", "database"])
    );
}

// ---------------------------------------------------------------------------
// Continuation after a previous record
// ---------------------------------------------------------------------------

#[test]
fn previous_record_must_be_named_after_the_record_it_holds() {
    let history = History::new();
    let installation = &history.installation;
    append_notes(installation, &["delta"]);
    let start_after = |previous: &Path, checkpoint_dir: &Path| {
        create_checkpoint(&CreateRequest {
            data: &installation.database,
            tenant_id: TENANT,
            key_dir: &installation.key_dir,
            checkpoint_dir,
            new_log: false,
            previous_record: Some(previous),
        })
    };
    let first_bytes = std::fs::read(&history.first.record).unwrap();
    let second_bytes = std::fs::read(&history.second.record).unwrap();
    let first_name = file_name_of(&history.first.record);
    let second_name = file_name_of(&history.second.record);

    // The last record of the log is the second one. A file named after it
    // that holds the first record, as a full directory whose last file was
    // replaced in place would hand it over; and the second record itself
    // under a name that announces no record.
    let carrier = private_child(installation.root(), "previous-record-carrier");
    let misnamed = carrier.join(&second_name);
    write_private(&misnamed, &first_bytes);
    let unnamed = carrier.join("last-record.json");
    write_private(&unnamed, &second_bytes);

    let mut observed = Vec::new();
    for (case, previous) in [
        ("another record under the name of the last one", &misnamed),
        ("the last record under another name", &unnamed),
    ] {
        let continued = installation
            .root()
            .join(format!("previous-record-continued-{}", observed.len()));
        observed.push((
            case,
            signing_answer(start_after(previous, &continued), &continued),
        ));
    }
    let refused = "record_name_mismatch exit=2 directory=false".to_owned();
    assert_eq!(
        observed,
        vec![
            (
                "another record under the name of the last one",
                refused.clone()
            ),
            ("the last record under another name", refused),
        ],
        "a log must not continue after a file that is not named after the record it holds, and the refusal must create nothing"
    );
    let continued = installation.root().join("previous-record-continued-detail");
    let refused = start_after(&misnamed, &continued).unwrap_err();
    assert_eq!(refused.error.detail["input"], "previous_record");
    assert_eq!(
        refused.error.detail["records"],
        serde_json::json!([second_name])
    );
    assert_eq!(
        refused.error.detail["expected_name"],
        serde_json::json!(first_name)
    );
    assert!(!continued.exists());
    assert_eq!(std::fs::read(&misnamed).unwrap(), first_bytes);
    assert_eq!(std::fs::read(&unnamed).unwrap(), second_bytes);

    // The last record under its own name is accepted wherever the file is.
    let genuine = private_child(installation.root(), "previous-record-genuine").join(&second_name);
    write_private(&genuine, &second_bytes);
    let continued = installation.root().join("previous-record-continued");
    let created = start_after(&genuine, &continued).unwrap();
    assert_eq!(
        (created.status, created.log_index),
        (CreateStatus::Created, 3)
    );
    assert!(!created.baseline);
}
