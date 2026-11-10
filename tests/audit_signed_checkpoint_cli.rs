//! The operator journey through the real `ai-guard-audit-checkpoint` binary:
//! every step is a subprocess with an empty environment, one JSON document on
//! standard output and an exit status.

use agentsandbox::{audit::Store, model::Event, redaction::Redactor};
use serde_json::Value;
use std::{
    ffi::OsStr,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const TENANT: &str = "tenant-a";

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Run {
    /// The single JSON document printed on standard output.
    fn json(&self) -> Value {
        serde_json::from_str(&self.stdout).unwrap_or_else(|error| {
            panic!("stdout is not one JSON document ({error}): {}", self.stdout)
        })
    }

    fn error_codes(&self) -> Vec<String> {
        self.json()["errors"]
            .as_array()
            .map(|errors| {
                errors
                    .iter()
                    .map(|error| error["code"].as_str().unwrap_or_default().to_owned())
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn run<I, S>(arguments: I) -> Run
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new(env!("CARGO_BIN_EXE_ai-guard-audit-checkpoint"))
        .args(arguments)
        .env_clear()
        .stdin(Stdio::null())
        .output()
        .unwrap();
    Run {
        code: output
            .status
            .code()
            .expect("the command exits with a status"),
        stdout: String::from_utf8(output.stdout).unwrap(),
        stderr: String::from_utf8(output.stderr).unwrap(),
    }
}

/// Run the command in a process that can create files but cannot write a
/// single byte to them: its file size limit is zero, as on a full disk or an
/// exhausted quota. SIGXFSZ is ignored so that the write fails with an error
/// instead of killing the process.
fn run_without_file_space<I, S>(arguments: I) -> Run
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    use std::os::unix::process::CommandExt;
    let mut command = Command::new(env!("CARGO_BIN_EXE_ai-guard-audit-checkpoint"));
    command.args(arguments).env_clear().stdin(Stdio::null());
    // SAFETY: the closure runs in the forked child before exec and only calls
    // async-signal-safe functions. It changes the limit and the signal
    // disposition of that child alone, never of the test process.
    unsafe {
        command.pre_exec(|| {
            let none = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            if libc::signal(libc::SIGXFSZ, libc::SIG_IGN) == libc::SIG_ERR
                || libc::setrlimit(libc::RLIMIT_FSIZE, &none) != 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let output = command.output().unwrap();
    Run {
        code: output
            .status
            .code()
            .expect("the command exits with a status"),
        stdout: String::from_utf8(output.stdout).unwrap(),
        stderr: String::from_utf8(output.stderr).unwrap(),
    }
}

/// A flag followed by a path, as two arguments.
fn at(flag: &str, path: &Path) -> [std::ffi::OsString; 2] {
    [flag.into(), path.as_os_str().to_owned()]
}

fn arguments(parts: &[&[std::ffi::OsString]]) -> Vec<std::ffi::OsString> {
    parts.iter().flat_map(|part| part.iter().cloned()).collect()
}

fn words(words: &[&str]) -> Vec<std::ffi::OsString> {
    words.iter().map(Into::into).collect()
}

fn private_child(parent: &Path, name: &str) -> PathBuf {
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

fn note(text: &str) -> Event {
    let mut event = Event::new("fixture.note");
    event.data = serde_json::json!({ "note": text });
    event
}

fn append_notes(database: &Path, notes: &[&str]) {
    let store = Store::open_for_tenant(database, TENANT, Redactor::new(vec![]), 10_000).unwrap();
    for text in notes {
        store.append(note(text)).unwrap();
    }
}

/// The paths of one control installation and of its anchor holder.
struct Site {
    root: tempfile::TempDir,
    database: PathBuf,
    key_dir: PathBuf,
    log_dir: PathBuf,
    holder: PathBuf,
}

impl Site {
    fn new() -> Self {
        let root = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let control = private_child(root.path(), "control");
        let database = control.join("audit.sqlite");
        append_notes(&database, &["alpha"]);
        Self {
            key_dir: root.path().join("keys").join("key-1"),
            log_dir: root.path().join("checkpoints"),
            holder: private_child(root.path(), "holder"),
            database,
            root,
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }

    /// Sign the current head. The first checkpoint starts the log explicitly.
    fn create(&self) -> Run {
        let mut command = vec!["create", "--tenant-id", TENANT];
        if !self.has_records() {
            command.push("--new-log");
        }
        run(arguments(&[
            &words(&command),
            &at("--data", &self.database),
            &at("--key-dir", &self.key_dir),
            &at("--checkpoint-dir", &self.log_dir),
        ]))
    }

    fn has_records(&self) -> bool {
        std::fs::read_dir(&self.log_dir).is_ok_and(|entries| {
            entries
                .map(|entry| entry.unwrap().file_name())
                .any(|name| name.to_string_lossy().starts_with("cp-"))
        })
    }

    /// Key, first checkpoint and an anchor approved by fingerprint.
    fn bootstrap(&self) -> (String, PathBuf) {
        let key = run(arguments(&[
            &words(&["key", "init"]),
            &at("--key-dir", &self.key_dir),
        ]));
        assert_eq!(key.code, 0, "{}", key.stdout);
        let key_id = key.json()["key_id"].as_str().unwrap().to_owned();
        let public_key = self.holder.join("key-1.public-key.json");
        let shown = run(arguments(&[
            &words(&["key", "show"]),
            &at("--key-dir", &self.key_dir),
            &at("--out", &public_key),
        ]));
        assert_eq!(shown.code, 0, "{}", shown.stdout);
        let created = self.create();
        assert_eq!(created.code, 0, "{}", created.stdout);
        let namespace = created.json()["namespace_uuid"]
            .as_str()
            .unwrap()
            .to_owned();
        let anchor = self.holder.join("anchor-1.json");
        let initialised = run(arguments(&[
            &words(&[
                "anchor",
                "init",
                "--tenant-id",
                TENANT,
                "--expect-key-id",
                &key_id,
                "--namespace-uuid",
                &namespace,
            ]),
            &at("--public-key", &public_key),
            &at("--out", &anchor),
        ]));
        assert_eq!(initialised.code, 0, "{}", initialised.stdout);
        (key_id, anchor)
    }
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

/// S4: even when the predecessor already signs the database head, an
/// explicit continuation must inspect an existing destination before saying
/// `unchanged`. This exercises the real binary and checks every byte left in
/// that destination, including the absence of a newly created writer lock.
#[test]
fn unchanged_continuation_checks_the_destination_without_writing() {
    use agentsandbox::audit::signed_checkpoint::{SignedRecord, Statement};

    let site = Site::new();
    site.bootstrap();
    let previous = site.create().json()["record"].as_str().unwrap().to_owned();
    let previous = Path::new(&previous);
    let original = std::fs::read(previous).unwrap();
    let record = SignedRecord::parse(&original).unwrap();
    let target = site.path("continuation");
    let invoke = || {
        run(arguments(&[
            &words(&["create", "--tenant-id", TENANT]),
            &at("--data", &site.database),
            &at("--key-dir", &site.key_dir),
            &at("--checkpoint-dir", &target),
            &at("--previous-record", previous),
        ]))
    };

    let absent = invoke();
    assert_eq!(absent.code, 0, "{}", absent.stdout);
    assert_eq!(absent.json()["status"], "unchanged");
    assert!(!target.exists(), "a no-op must not create its destination");

    private_child(site.root.path(), "continuation");
    let empty = invoke();
    assert_eq!(empty.code, 0, "{}", empty.stdout);
    assert_eq!(empty.json()["status"], "unchanged");
    assert!(entry_names(&target).is_empty());

    // An interrupted record stays incomplete and is included in the outcome.
    let fragment = target.join(format!("cp-{:020}-{}.json", 2, "f".repeat(64)));
    write_private(&fragment, b"{");
    let incomplete = invoke();
    assert_eq!(incomplete.code, 0, "{}", incomplete.stdout);
    assert_eq!(incomplete.json()["status"], "unchanged");
    assert_eq!(
        incomplete.json()["incomplete_records"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        entry_names(&target),
        vec![fragment.file_name().unwrap().to_str().unwrap()]
    );
    assert_eq!(std::fs::read(&fragment).unwrap(), b"{");
    std::fs::remove_file(&fragment).unwrap();

    let snapshot = || {
        entry_names(&target)
            .into_iter()
            .map(|name| {
                let bytes = std::fs::read(target.join(&name)).unwrap();
                (name, bytes)
            })
            .collect::<Vec<_>>()
    };
    let expect_refusal = |code, error| {
        let before = snapshot();
        let outcome = invoke();
        assert_eq!(outcome.code, code, "{}", outcome.stdout);
        assert_eq!(outcome.error_codes(), vec![error], "{}", outcome.stdout);
        assert!(outcome.stderr.is_empty(), "{}", outcome.stderr);
        assert_eq!(
            snapshot(),
            before,
            "a refused continuation must write nothing"
        );
    };

    let named = target.join(record.file_name());
    write_private(&named, &original);
    expect_refusal(1, "invalid_argument");

    // A structurally complete second record at the same index is a fork;
    // listing must refuse it before any signature could be trusted.
    let Statement::Checkpoint(checkpoint) = record.statement() else {
        panic!("the predecessor is a checkpoint");
    };
    let other_time = if checkpoint.created_at == "2026-11-10T09:00:00.000Z" {
        "2026-11-10T09:00:00.001Z"
    } else {
        "2026-11-10T09:00:00.000Z"
    };
    let altered = String::from_utf8(original.clone()).unwrap().replace(
        &format!("\"created_at\":\"{}\"", checkpoint.created_at),
        &format!("\"created_at\":\"{other_time}\""),
    );
    let fork = SignedRecord::parse(altered.as_bytes()).unwrap();
    let fork_path = target.join(fork.file_name());
    write_private(&fork_path, &fork.to_bytes().unwrap());
    expect_refusal(2, "log_forked");
    std::fs::remove_file(&fork_path).unwrap();
    std::fs::remove_file(&named).unwrap();

    let renamed = target.join(format!(
        "cp-{:020}-{}.json",
        record.log_index(),
        "e".repeat(64)
    ));
    assert_ne!(renamed.file_name(), previous.file_name());
    write_private(&renamed, &original);
    expect_refusal(2, "record_name_mismatch");
}

#[test]
fn operator_journey_create_export_verify_offline() {
    let site = Site::new();

    // The operator creates the key and reads its fingerprint.
    let key = run(arguments(&[
        &words(&["key", "init"]),
        &at("--key-dir", &site.key_dir),
    ]));
    assert_eq!(key.code, 0, "{}", key.stdout);
    assert!(key.stderr.is_empty());
    let document = key.json();
    assert_eq!(document["format"], "ai-guard.audit.checkpoint-command.v1");
    assert_eq!(document["command"], "key init");
    let key_id = document["key_id"].as_str().unwrap().to_owned();
    assert_eq!(key_id.len(), 64);
    let public_key = site.holder.join("key-1.public-key.json");
    let shown = run(arguments(&[
        &words(&["key", "show"]),
        &at("--key-dir", &site.key_dir),
        &at("--out", &public_key),
    ]));
    assert_eq!(shown.code, 0, "{}", shown.stdout);
    assert_eq!(shown.json()["key_id"], key_id.as_str());

    // The namespace to approve is read from the database, before signing.
    let diagnosed = run(arguments(&[
        &words(&["diagnose", "--tenant-id", TENANT]),
        &at("--data", &site.database),
    ]));
    assert_eq!(diagnosed.code, 0, "{}", diagnosed.stdout);
    let namespace = diagnosed.json()["database"]["namespace_uuid"]
        .as_str()
        .unwrap()
        .to_owned();

    // The anchor holder approves the key by its fingerprint.
    let anchor = site.holder.join("anchor-1.json");
    let initialised = run(arguments(&[
        &words(&[
            "anchor",
            "init",
            "--tenant-id",
            TENANT,
            "--expect-key-id",
            &key_id,
            "--namespace-uuid",
            &namespace,
        ]),
        &at("--public-key", &public_key),
        &at("--out", &anchor),
    ]));
    assert_eq!(initialised.code, 0, "{}", initialised.stdout);
    assert_eq!(initialised.json()["status"], "initialized");

    // First checkpoint of the existing history, then verification in place.
    let created = site.create();
    assert_eq!(created.code, 0, "{}", created.stdout);
    let created = created.json();
    assert_eq!(created["status"], "created");
    assert_eq!(created["baseline"], true);
    assert_eq!(created["namespace_uuid"], namespace.as_str());
    let verified = run(arguments(&[
        &words(&["verify"]),
        &at("--anchor", &anchor),
        &at("--checkpoint-dir", &site.log_dir),
        &at("--data", &site.database),
    ]));
    assert_eq!(verified.code, 0, "{}", verified.stdout);
    assert_eq!(verified.json()["verdict"], "verified");
    // In this fixture the anchor sits on the same filesystem as the database
    // it vouches for: the report says so instead of implying independence.
    assert!(
        verified.json()["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning["code"] == "same_filesystem_device"),
        "{}",
        verified.stdout
    );
    assert_eq!(
        verified.json()["format"],
        "ai-guard.audit.checkpoint-verification.v1"
    );

    // More events, a second checkpoint, and a public proof for the auditor.
    append_notes(&site.database, &["beta", "gamma"]);
    let second = site.create();
    assert_eq!(second.code, 0, "{}", second.stdout);
    assert_eq!(second.json()["log_index"], 2);
    let bundle = site.path("proof.json");
    let exported = run(arguments(&[
        &words(&["export"]),
        &at("--checkpoint-dir", &site.log_dir),
        &at("--out", &bundle),
    ]));
    assert_eq!(
        exported.code, 0,
        "a proof bundle must be exportable: {}",
        exported.stdout
    );
    let exported = exported.json();
    assert_eq!(exported["entries"], 2);
    assert_eq!(exported["first_log_index"], 1);
    assert_eq!(exported["last_log_index"], 2);
    assert_eq!(
        exported["sha256"],
        sha256(&std::fs::read(&bundle).unwrap()).as_str()
    );

    // The auditor has only the anchor and the bundle: no database, no key, no
    // checkpoint directory.
    let auditor = private_child(site.root.path(), "auditor");
    std::fs::copy(&bundle, auditor.join("proof.json")).unwrap();
    std::fs::copy(&anchor, auditor.join("anchor.json")).unwrap();
    let report_file = auditor.join("report.json");
    let offline = run(arguments(&[
        &words(&["verify"]),
        &at("--anchor", &auditor.join("anchor.json")),
        &at("--bundle", &auditor.join("proof.json")),
        &at("--report-out", &report_file),
    ]));
    assert_eq!(offline.code, 0, "{}", offline.stdout);
    let report = offline.json();
    assert_eq!(report["verdict"], "verified");
    assert_eq!(report["entries_checked"], 2);
    assert_eq!(report["signatures_verified"], 2);
    assert_eq!(report["verified_through_log_index"], 2);
    assert_eq!(report["database"]["compared"], false);
    assert_eq!(report["database"]["event_count"], Value::Null);
    assert_eq!(report["rollback_detection"], "unavailable_no_reference");
    assert_eq!(report["warnings"], serde_json::json!([]));
    let saved: Value = serde_json::from_slice(&std::fs::read(&report_file).unwrap()).unwrap();
    assert_eq!(saved, report);

    // A range that the directory does not hold is refused and writes nothing.
    let nowhere = auditor.join("out-of-range-proof.json");
    let out_of_range = run(arguments(&[
        &words(&["export", "--from-log-index", "3"]),
        &at("--checkpoint-dir", &site.log_dir),
        &at("--out", &nowhere),
    ]));
    assert_eq!(out_of_range.code, 1, "{}", out_of_range.stdout);
    assert_eq!(out_of_range.error_codes(), vec!["invalid_argument"]);
    assert!(!nowhere.exists());

    // The auditor records the verified head as the reference; from then on a
    // shorter proof is a rollback.
    let advanced_anchor = auditor.join("anchor-2.json");
    let advanced = run(arguments(&[
        &words(&["anchor", "advance"]),
        &at("--anchor", &auditor.join("anchor.json")),
        &at("--bundle", &auditor.join("proof.json")),
        &at("--out", &advanced_anchor),
    ]));
    assert_eq!(advanced.code, 0, "{}", advanced.stdout);
    assert_eq!(advanced.json()["status"], "advanced");
    assert_eq!(advanced.json()["anchor"]["reference"]["log_index"], 2);
    let short_bundle = auditor.join("short-proof.json");
    let exported = run(arguments(&[
        &words(&["export", "--through-log-index", "1"]),
        &at("--checkpoint-dir", &site.log_dir),
        &at("--out", &short_bundle),
    ]));
    assert_eq!(exported.code, 0, "{}", exported.stdout);
    let rolled_back = run(arguments(&[
        &words(&["verify", "--require-reference"]),
        &at("--anchor", &advanced_anchor),
        &at("--bundle", &short_bundle),
    ]));
    assert_eq!(rolled_back.code, 2, "{}", rolled_back.stdout);
    assert_eq!(rolled_back.error_codes(), vec!["log_rolled_back"]);

    // With a copy of the database, the same bundle also covers its content.
    let with_database = run(arguments(&[
        &words(&["verify", "--require-reference"]),
        &at("--anchor", &advanced_anchor),
        &at("--bundle", &bundle),
        &at("--data", &site.database),
    ]));
    assert_eq!(with_database.code, 0, "{}", with_database.stdout);
    assert_eq!(with_database.json()["database"]["compared"], true);
    assert_eq!(
        with_database.json()["database"]["events_after_checkpoint"],
        0
    );
    assert_eq!(
        with_database.json()["rollback_detection"],
        "anchor_reference"
    );
}

/// The private seed of the key stored in `key_dir`, proven to be the private
/// half of `key_id`: the public key derived from it has that fingerprint.
fn private_seed(key_dir: &Path, key_id: &str) -> Vec<u8> {
    use ring::signature::KeyPair;
    let pkcs8 = std::fs::read(key_dir.join("signing-key.pk8")).unwrap();
    assert_eq!(pkcs8.len(), 83, "PKCS#8 v2 Ed25519 document");
    let seed = &pkcs8[16..48];
    let derived = ring::signature::Ed25519KeyPair::from_seed_unchecked(seed).unwrap();
    assert_eq!(
        sha256(derived.public_key().as_ref()),
        key_id,
        "bytes 16 to 48 of the key file must be the private seed of the reported key"
    );
    seed.to_vec()
}

/// Every textual and binary form in which the private seed could leak.
fn seed_forms(key_dir: &Path) -> Vec<Vec<u8>> {
    let pkcs8 = std::fs::read(key_dir.join("signing-key.pk8")).unwrap();
    seed_forms_of(&pkcs8)
}

fn seed_forms_of(pkcs8: &[u8]) -> Vec<Vec<u8>> {
    use base64::{
        Engine,
        engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD},
    };
    assert_eq!(pkcs8.len(), 83, "PKCS#8 v2 Ed25519 document");
    let seed = &pkcs8[16..48];
    vec![
        seed.to_vec(),
        hex::encode(seed).into_bytes(),
        hex::encode_upper(seed).into_bytes(),
        STANDARD.encode(seed).into_bytes(),
        STANDARD_NO_PAD.encode(seed).into_bytes(),
        URL_SAFE.encode(seed).into_bytes(),
        URL_SAFE_NO_PAD.encode(seed).into_bytes(),
        STANDARD.encode(pkcs8).into_bytes(),
        URL_SAFE_NO_PAD.encode(pkcs8).into_bytes(),
        hex::encode(pkcs8).into_bytes(),
    ]
}

#[test]
fn leak_detector_finds_every_form_of_a_seed() {
    // RFC 8032 section 7.1 TEST 1, as the PKCS#8 v2 document the tool stores.
    let seed = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";
    let public = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";
    let pkcs8 = hex::decode(format!(
        "3051020101300506032b657004220420{seed}812100{public}"
    ))
    .unwrap();
    let forms = seed_forms_of(&pkcs8);
    assert_eq!(forms.len(), 10);
    assert_eq!(forms[0], hex::decode(seed).unwrap());
    assert_eq!(forms[1], seed.as_bytes());
    for (position, form) in forms.iter().enumerate() {
        assert!(
            forms[..position].iter().all(|other| other != form),
            "form {position} repeats an earlier one"
        );
        // Each form is found when it is embedded in an output...
        let mut output = b"{\"detail\":{\"reason\":\"".to_vec();
        output.extend_from_slice(form);
        output.extend_from_slice(b"\"}}\n");
        assert!(contains(&output, form), "form {position} is not detected");
        // ...and not in an output that carries public material only.
        let public_only = format!("{{\"key_id\":\"{public}\",\"public_key\":\"{public}\"}}");
        assert!(
            !contains(public_only.as_bytes(), form),
            "form {position} matches public material"
        );
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn files_under(directory: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files_under(&path, found);
        } else {
            found.push(path);
        }
    }
}

#[test]
fn no_private_key_material_in_outputs_exports_or_logs() {
    let site = Site::new();
    let (key_id, anchor) = site.bootstrap();
    let mut transcripts: Vec<Run> = Vec::new();

    // Successful commands.
    append_notes(&site.database, &["beta"]);
    transcripts.push(site.create());
    let new_key_dir = site.path("keys").join("key-2");
    transcripts.push(run(arguments(&[
        &words(&["key", "init"]),
        &at("--key-dir", &new_key_dir),
    ])));
    let new_key_id = transcripts.last().unwrap().json()["key_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let new_public_key = site.holder.join("key-2.public-key.json");
    transcripts.push(run(arguments(&[
        &words(&["key", "show"]),
        &at("--key-dir", &new_key_dir),
        &at("--out", &new_public_key),
    ])));
    let advanced = site.holder.join("anchor-2.json");
    transcripts.push(run(arguments(&[
        &words(&["anchor", "advance"]),
        &at("--anchor", &anchor),
        &at("--checkpoint-dir", &site.log_dir),
        &at("--data", &site.database),
        &at("--out", &advanced),
    ])));
    transcripts.push(run(arguments(&[
        &words(&["rotate-key"]),
        &at("--checkpoint-dir", &site.log_dir),
        &at("--key-dir", &site.key_dir),
        &at("--new-key-dir", &new_key_dir),
    ])));
    let approved = site.holder.join("anchor-3.json");
    transcripts.push(run(arguments(&[
        &words(&["anchor", "approve-rotation", "--expect-key-id", &new_key_id]),
        &at("--anchor", &advanced),
        &at("--checkpoint-dir", &site.log_dir),
        &at("--public-key", &new_public_key),
        &at("--out", &approved),
    ])));
    let bundle = site.holder.join("proof.json");
    transcripts.push(run(arguments(&[
        &words(&["export"]),
        &at("--checkpoint-dir", &site.log_dir),
        &at("--out", &bundle),
    ])));
    transcripts.push(run(arguments(&[
        &words(&["verify"]),
        &at("--anchor", &approved),
        &at("--bundle", &bundle),
        &at("--data", &site.database),
        &at("--report-out", &site.holder.join("report.json")),
    ])));
    transcripts.push(run(arguments(&[
        &words(&["diagnose", "--tenant-id", TENANT]),
        &at("--data", &site.database),
        &at("--checkpoint-dir", &site.log_dir),
        &at("--key-dir", &site.key_dir),
        &at("--anchor", &approved),
    ])));
    for transcript in &transcripts {
        assert_eq!(transcript.code, 0, "{}", transcript.stdout);
    }

    // Refused and failed commands, including ones that load the private key.
    let refused = [
        // The retired key can no longer extend the log.
        site.create(),
        run(arguments(&[
            &words(&["create", "--tenant-id", "tenant-b"]),
            &at("--data", &site.database),
            &at("--key-dir", &new_key_dir),
            &at("--checkpoint-dir", &site.log_dir),
        ])),
        run(arguments(&[
            &words(&["key", "init"]),
            &at("--key-dir", &site.key_dir),
        ])),
        run(arguments(&[
            &words(&["key", "show"]),
            &at("--key-dir", &site.key_dir),
            &at("--out", &bundle),
        ])),
        run(arguments(&[
            &words(&["verify"]),
            &at("--anchor", &anchor),
            &at("--checkpoint-dir", &site.log_dir),
        ])),
        run(arguments(&[
            &words(&[
                "rotate-key",
                "--unendorsed-recovery",
                "--retiring-key-id",
                &key_id,
            ]),
            &at("--checkpoint-dir", &site.log_dir),
            &at("--new-key-dir", &new_key_dir),
        ])),
        run(words(&["create", "--no-such-option"])),
    ];
    for transcript in refused {
        assert_ne!(transcript.code, 0, "{}", transcript.stdout);
        transcripts.push(transcript);
    }

    // The needle is the private seed, not an arbitrary slice of the file: the
    // public key derived from it is the key that each command reported.
    for (key_dir, reported) in [(&site.key_dir, &key_id), (&new_key_dir, &new_key_id)] {
        let seed = private_seed(key_dir, reported);
        assert_eq!(seed_forms(key_dir)[0], seed);
    }

    // Nothing printed and no file outside the key directories carries a key.
    let mut files = Vec::new();
    files_under(site.root.path(), &mut files);
    let keys_root = site.path("keys");
    let outside: Vec<&PathBuf> = files
        .iter()
        .filter(|path| !path.starts_with(&keys_root))
        .collect();
    assert!(outside.iter().any(|path| path.ends_with("proof.json")));
    assert!(outside.iter().any(|path| path.ends_with("report.json")));
    assert!(outside.iter().any(|path| path.ends_with("anchor-3.json")));
    for key_dir in [&site.key_dir, &new_key_dir] {
        for form in seed_forms(key_dir) {
            for transcript in &transcripts {
                assert!(
                    !contains(transcript.stdout.as_bytes(), &form)
                        && !contains(transcript.stderr.as_bytes(), &form),
                    "private key material in command output"
                );
            }
            for path in &outside {
                assert!(
                    !contains(&std::fs::read(path).unwrap(), &form),
                    "private key material in {}",
                    path.display()
                );
            }
        }
    }
    // The key directories hold exactly one private file each.
    let inside: Vec<&PathBuf> = files
        .iter()
        .filter(|path| path.starts_with(&keys_root))
        .collect();
    assert_eq!(inside.len(), 2);
    for path in inside {
        assert!(path.ends_with("signing-key.pk8"));
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o7777,
            0o600
        );
    }
}

#[test]
fn refusals_are_structured_with_exit_code_2_and_actions() {
    let site = Site::new();
    let (key_id, anchor) = site.bootstrap();
    let bundle = site.holder.join("proof.json");
    let exported = run(arguments(&[
        &words(&["export"]),
        &at("--checkpoint-dir", &site.log_dir),
        &at("--out", &bundle),
    ]));
    assert_eq!(exported.code, 0, "{}", exported.stdout);

    let structured = |outcome: &Run, exit_code: i32, code: &str| {
        assert_eq!(outcome.code, exit_code, "{}", outcome.stdout);
        assert!(outcome.stderr.is_empty(), "{}", outcome.stderr);
        let document = outcome.json();
        let errors = document["errors"].as_array().unwrap();
        assert_eq!(errors.len(), 1, "{}", outcome.stdout);
        let error = &errors[0];
        assert_eq!(error["code"], code, "{}", outcome.stdout);
        assert!(!error["message"].as_str().unwrap().is_empty());
        assert!(!error["action"].as_str().unwrap().is_empty());
        assert!(error["detail"].is_object());
        document
    };

    // Wrong scope: an anchor of another tenant, a request for another tenant.
    let other_anchor = site.holder.join("other-tenant.anchor.json");
    let namespace = run(arguments(&[
        &words(&["diagnose", "--tenant-id", TENANT]),
        &at("--data", &site.database),
    ]))
    .json()["database"]["namespace_uuid"]
        .as_str()
        .unwrap()
        .to_owned();
    let initialised = run(arguments(&[
        &words(&[
            "anchor",
            "init",
            "--tenant-id",
            "tenant-b",
            "--expect-key-id",
            &key_id,
            "--namespace-uuid",
            &namespace,
        ]),
        &at("--public-key", &site.holder.join("key-1.public-key.json")),
        &at("--out", &other_anchor),
    ]));
    assert_eq!(initialised.code, 0, "{}", initialised.stdout);
    let refused = run(arguments(&[
        &words(&["verify"]),
        &at("--anchor", &other_anchor),
        &at("--bundle", &bundle),
    ]));
    let report = structured(&refused, 2, "scope_mismatch");
    assert_eq!(
        report["format"],
        "ai-guard.audit.checkpoint-verification.v1"
    );
    assert_eq!(report["verdict"], "refused");
    assert_eq!(report["errors"][0]["detail"]["field"], "tenant_id");
    let refused = run(arguments(&[
        &words(&["create", "--tenant-id", "tenant-b"]),
        &at("--data", &site.database),
        &at("--key-dir", &site.key_dir),
        &at("--checkpoint-dir", &site.log_dir),
    ]));
    let document = structured(&refused, 2, "scope_mismatch");
    assert_eq!(document["format"], "ai-guard.audit.checkpoint-error.v1");

    // Modified content: a non-canonical bundle entry, then a changed signature.
    let original = std::fs::read_to_string(&bundle).unwrap();
    let spaced = site.holder.join("spaced-proof.json");
    write_private(
        &spaced,
        original
            .replacen("\"statement\":{", "\"statement\":{ ", 1)
            .as_bytes(),
    );
    let refused = run(arguments(&[
        &words(&["verify"]),
        &at("--anchor", &anchor),
        &at("--bundle", &spaced),
    ]));
    structured(&refused, 2, "record_not_canonical");
    let signature_start = original.find("\"signature\":\"").unwrap() + "\"signature\":\"".len();
    let mut forged = original.clone();
    let replacement = if &original[signature_start..signature_start + 1] == "A" {
        "B"
    } else {
        "A"
    };
    forged.replace_range(signature_start..signature_start + 1, replacement);
    let forged_bundle = site.holder.join("forged-proof.json");
    write_private(&forged_bundle, forged.as_bytes());
    let refused = run(arguments(&[
        &words(&["verify"]),
        &at("--anchor", &anchor),
        &at("--bundle", &forged_bundle),
    ]));
    let report = structured(&refused, 2, "signature_invalid");
    assert_eq!(report["signatures_verified"], 0);
    let with_key = site.holder.join("keyed-proof.json");
    write_private(
        &with_key,
        original
            .replacen("{\"format\":", "{\"public_key\":\"AAAA\",\"format\":", 1)
            .as_bytes(),
    );
    let refused = run(arguments(&[
        &words(&["verify"]),
        &at("--anchor", &anchor),
        &at("--bundle", &with_key),
    ]));
    structured(&refused, 2, "bundle_malformed");

    // A caller that requires rollback detection is refused without reference.
    let refused = run(arguments(&[
        &words(&["verify", "--require-reference"]),
        &at("--anchor", &anchor),
        &at("--bundle", &bundle),
    ]));
    structured(&refused, 2, "no_independent_reference");

    // Operational errors use exit status 1 and the same structure.
    let failed = run(arguments(&[
        &words(&["verify"]),
        &at("--anchor", &site.holder.join("missing.anchor.json")),
        &at("--bundle", &bundle),
    ]));
    structured(&failed, 1, "anchor_unavailable");
    let failed = run(arguments(&[
        &words(&["verify"]),
        &at("--anchor", &anchor),
        &at("--bundle", &bundle),
        &at("--data", &site.path("control").join("missing.sqlite")),
    ]));
    structured(&failed, 1, "audit_unavailable");
    let failed = run(arguments(&[
        &words(&["verify"]),
        &at("--anchor", &anchor),
        &at("--bundle", &site.holder.join("missing.proof.json")),
    ]));
    structured(&failed, 1, "input_unavailable");
    let failed = run(arguments(&[
        &words(&["verify"]),
        &at("--anchor", &anchor),
        &at("--bundle", &bundle),
        &at("--checkpoint-dir", &site.log_dir),
    ]));
    structured(&failed, 1, "invalid_argument");
    let failed = run(words(&["create", "--no-such-option"]));
    let document = structured(&failed, 1, "invalid_argument");
    assert!(
        document["errors"][0]["detail"]["usage"]
            .as_str()
            .unwrap()
            .contains("--no-such-option")
    );
    let failed = run(Vec::<&str>::new());
    structured(&failed, 1, "invalid_argument");

    // Help is the only output that is not a JSON document.
    let help = run(["--help"]);
    assert_eq!(help.code, 0);
    assert!(help.stdout.contains("Exit status"));
    assert!(help.stderr.is_empty());
    for command in [
        "key",
        "anchor",
        "create",
        "rotate-key",
        "verify",
        "diagnose",
        "export",
    ] {
        assert!(
            help.stdout.contains(command),
            "{command} is missing from --help"
        );
    }
}

#[test]
fn diagnose_explains_state_without_trust_decision() {
    let site = Site::new();
    let (_, anchor) = site.bootstrap();
    let advanced = site.holder.join("anchor-2.json");
    let moved = run(arguments(&[
        &words(&["anchor", "advance"]),
        &at("--anchor", &anchor),
        &at("--checkpoint-dir", &site.log_dir),
        &at("--out", &advanced),
    ]));
    assert_eq!(moved.code, 0, "{}", moved.stdout);
    append_notes(&site.database, &["beta"]);
    assert_eq!(site.create().code, 0);
    append_notes(&site.database, &["gamma", "delta"]);
    // Debris of an interrupted write, a stray note, and a key that is not the
    // one the log expects.
    write_private(
        &site
            .log_dir
            .join(format!("cp-{:020}-{}.json", 3, "e".repeat(64))),
        b"{\"format\":",
    );
    write_private(&site.log_dir.join("notes.txt"), b"operator note");
    let other_key_dir = site.path("keys").join("key-2");
    assert_eq!(
        run(arguments(&[
            &words(&["key", "init"]),
            &at("--key-dir", &other_key_dir)
        ]))
        .code,
        0
    );

    let diagnosed = run(arguments(&[
        &words(&["diagnose", "--tenant-id", TENANT]),
        &at("--data", &site.database),
        &at("--checkpoint-dir", &site.log_dir),
        &at("--key-dir", &other_key_dir),
        &at("--anchor", &advanced),
    ]));
    assert_eq!(diagnosed.code, 0, "{}", diagnosed.stdout);
    let report = diagnosed.json();
    assert_eq!(report["format"], "ai-guard.audit.checkpoint-diagnostic.v1");
    assert!(
        report["notice"]
            .as_str()
            .unwrap()
            .contains("no trust decision")
    );
    assert!(report.get("verdict").is_none());
    assert_eq!(report["database"]["readable"], true);
    assert_eq!(report["database"]["event_count"], 5);
    assert_eq!(report["checkpoint_log"]["readable"], true);
    assert_eq!(report["checkpoint_log"]["complete_records"], 2);
    assert_eq!(report["checkpoint_log"]["head"]["log_index"], 2);
    assert_eq!(report["checkpoint_log"]["head"]["event_count"], 3);
    assert_eq!(
        report["checkpoint_log"]["incomplete_records"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        report["checkpoint_log"]["unexpected_entries"],
        serde_json::json!(["notes.txt"])
    );
    assert_eq!(report["signing_key"]["readable"], true);
    assert_eq!(report["trust_anchor"]["readable"], true);
    assert_eq!(report["trust_anchor"]["revision"], 2);
    assert_eq!(
        report["trust_anchor"]["previous_anchor_sha256"],
        sha256(&std::fs::read(&anchor).unwrap()).as_str()
    );
    assert_eq!(
        report["trust_anchor"]["sha256"],
        sha256(&std::fs::read(&advanced).unwrap()).as_str()
    );
    let findings: Vec<&str> = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|finding| finding["code"].as_str().unwrap())
        .collect();
    for expected in [
        "incomplete_records_present",
        "unexpected_entries_present",
        "unsigned_tail",
        "key_is_not_next_signer",
        "records_after_anchor_reference",
    ] {
        assert!(
            findings.contains(&expected),
            "{expected} missing from {findings:?}"
        );
    }
    let tail = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|finding| finding["code"] == "unsigned_tail")
        .unwrap();
    assert_eq!(tail["detail"]["events_after_last_checkpoint"], 2);

    // An unreadable piece is reported in its own section, not as empty state.
    let partial = run(arguments(&[
        &words(&["diagnose"]),
        &at("--checkpoint-dir", &site.path("no-such-directory")),
        &at("--anchor", &site.holder.join("missing.anchor.json")),
    ]));
    assert_eq!(partial.code, 0, "{}", partial.stdout);
    let report = partial.json();
    assert_eq!(report["checkpoint_log"]["readable"], false);
    assert_eq!(
        report["checkpoint_log"]["error"]["code"],
        "checkpoint_log_unavailable"
    );
    assert_eq!(report["checkpoint_log"]["complete_records"], Value::Null);
    assert_eq!(report["trust_anchor"]["readable"], false);
    assert_eq!(
        report["trust_anchor"]["error"]["code"],
        "anchor_unavailable"
    );
    assert_eq!(report["database"], Value::Null);

    let nothing = run(["diagnose"]);
    assert_eq!(nothing.code, 1, "{}", nothing.stdout);
    assert_eq!(nothing.error_codes(), vec!["invalid_argument"]);

    // An entry that is not a private file makes create and verify fail closed
    // rather than skip it, while the diagnosis still describes the rest. The
    // entry is never removed.
    let stray = site.log_dir.join("stray.json");
    std::fs::write(&stray, b"{}").unwrap();
    std::fs::set_permissions(&stray, std::fs::Permissions::from_mode(0o644)).unwrap();
    let refused = run(arguments(&[
        &words(&["verify"]),
        &at("--anchor", &advanced),
        &at("--checkpoint-dir", &site.log_dir),
    ]));
    assert_eq!(refused.code, 1, "{}", refused.stdout);
    assert_eq!(refused.error_codes(), vec!["checkpoint_log_unavailable"]);
    let refused = site.create();
    assert_eq!(refused.code, 1, "{}", refused.stdout);
    assert_eq!(refused.error_codes(), vec!["checkpoint_log_unavailable"]);
    let diagnosed = run(arguments(&[
        &words(&["diagnose"]),
        &at("--checkpoint-dir", &site.log_dir),
    ]));
    assert_eq!(diagnosed.code, 0, "{}", diagnosed.stdout);
    let report = diagnosed.json();
    assert_eq!(report["checkpoint_log"]["readable"], true);
    assert_eq!(report["checkpoint_log"]["rejected_entries"], 1);
    assert_eq!(report["checkpoint_log"]["enumeration_complete"], false);
    assert_eq!(report["checkpoint_log"]["complete_records"], 2);
    assert!(
        report["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| finding["code"] == "rejected_entries_present"),
        "{}",
        diagnosed.stdout
    );
    assert_eq!(std::fs::read(&stray).unwrap(), b"{}");
}

/// An audit database at a given raw schema version, built without the Store.
fn raw_database(path: &Path, user_version: u32) {
    write_private(path, b"");
    let db = rusqlite::Connection::open(path).unwrap();
    db.execute_batch(&format!("CREATE TABLE events(seq INTEGER PRIMARY KEY,id TEXT UNIQUE NOT NULL,received_at TEXT NOT NULL,prev_hash TEXT NOT NULL,hash TEXT NOT NULL,payload TEXT NOT NULL,kind TEXT NOT NULL,agent_id TEXT,sandbox_id TEXT,project_id TEXT,decision TEXT);
CREATE TABLE quota(id INTEGER PRIMARY KEY CHECK(id=1),bytes INTEGER NOT NULL);
CREATE TABLE tenant_binding(singleton INTEGER PRIMARY KEY CHECK(singleton=1),tenant_id TEXT NOT NULL UNIQUE,schema_version INTEGER NOT NULL CHECK(schema_version=1));
CREATE INDEX events_agent ON events(agent_id,seq); CREATE INDEX events_sandbox ON events(sandbox_id,seq);CREATE INDEX events_project ON events(project_id,seq);CREATE INDEX events_kind ON events(kind,seq);
INSERT INTO quota VALUES(1,0);INSERT INTO tenant_binding VALUES(1,'{TENANT}',1);PRAGMA user_version={user_version};")).unwrap();
}

#[test]
fn legacy_schema_v1_audit_is_refused_without_modification() {
    let site = Site::new();
    let key = run(arguments(&[
        &words(&["key", "init"]),
        &at("--key-dir", &site.key_dir),
    ]));
    assert_eq!(key.code, 0, "{}", key.stdout);

    // Schema 1 has no control namespace to bind a checkpoint to.
    let legacy_dir = private_child(site.root.path(), "legacy");
    let legacy = legacy_dir.join("audit.sqlite");
    raw_database(&legacy, 1);
    let before = std::fs::read(&legacy).unwrap();
    let refused = run(arguments(&[
        &words(&["create", "--new-log", "--tenant-id", TENANT]),
        &at("--data", &legacy),
        &at("--key-dir", &site.key_dir),
        &at("--checkpoint-dir", &site.path("legacy-checkpoints")),
    ]));
    assert_eq!(refused.code, 2, "{}", refused.stdout);
    assert_eq!(refused.error_codes(), vec!["audit_namespace_unavailable"]);
    assert!(
        !site.path("legacy-checkpoints").exists(),
        "a refused start must not leave a checkpoint directory behind"
    );
    assert!(
        refused.json()["errors"][0]["action"]
            .as_str()
            .unwrap()
            .contains("migrate-control --target-version 2")
    );
    assert_eq!(std::fs::read(&legacy).unwrap(), before);
    let diagnosed = run(arguments(&[
        &words(&["diagnose", "--tenant-id", TENANT]),
        &at("--data", &legacy),
    ]));
    assert_eq!(diagnosed.code, 0, "{}", diagnosed.stdout);
    assert_eq!(diagnosed.json()["database"]["readable"], false);
    assert_eq!(
        diagnosed.json()["database"]["error"]["code"],
        "audit_namespace_unavailable"
    );
    assert_eq!(std::fs::read(&legacy).unwrap(), before);

    // A schema newer than this build is an operational limit, not a refusal
    // of the chain.
    let future_dir = private_child(site.root.path(), "future");
    let future = future_dir.join("audit.sqlite");
    raw_database(&future, 999);
    let before = std::fs::read(&future).unwrap();
    let failed = run(arguments(&[
        &words(&["create", "--new-log", "--tenant-id", TENANT]),
        &at("--data", &future),
        &at("--key-dir", &site.key_dir),
        &at("--checkpoint-dir", &site.path("future-checkpoints")),
    ]));
    assert_eq!(failed.code, 1, "{}", failed.stdout);
    assert_eq!(failed.error_codes(), vec!["audit_schema_unsupported"]);
    assert_eq!(failed.json()["errors"][0]["detail"]["user_version"], 999);
    assert_eq!(std::fs::read(&future).unwrap(), before);
    assert!(!site.path("future-checkpoints").exists());
}

#[test]
fn create_starts_a_log_only_on_request() {
    let site = Site::new();
    let key = run(arguments(&[
        &words(&["key", "init"]),
        &at("--key-dir", &site.key_dir),
    ]));
    assert_eq!(key.code, 0, "{}", key.stdout);
    let create = |flags: &[&str]| {
        let mut command = vec!["create", "--tenant-id", TENANT];
        command.extend_from_slice(flags);
        run(arguments(&[
            &words(&command),
            &at("--data", &site.database),
            &at("--key-dir", &site.key_dir),
            &at("--checkpoint-dir", &site.log_dir),
        ]))
    };

    // The line a scheduler runs: it never starts a log by itself.
    let refused = create(&[]);
    assert_eq!(
        refused.code, 1,
        "a missing checkpoint directory must not start a log: {}",
        refused.stdout
    );
    assert_eq!(refused.error_codes(), vec!["checkpoint_log_unavailable"]);
    assert!(
        refused.json()["errors"][0]["action"]
            .as_str()
            .unwrap()
            .contains("--new-log")
    );
    assert!(!site.log_dir.exists());

    // The operator starts it once, knowingly; the flag is refused afterwards.
    let started = create(&["--new-log"]);
    assert_eq!(started.code, 0, "{}", started.stdout);
    assert_eq!(started.json()["baseline"], true);
    let again = create(&["--new-log"]);
    assert_eq!(again.code, 1, "{}", again.stdout);
    assert_eq!(again.error_codes(), vec!["invalid_argument"]);
    let unchanged = create(&[]);
    assert_eq!(unchanged.code, 0, "{}", unchanged.stdout);
    assert_eq!(unchanged.json()["status"], "unchanged");
}

#[test]
fn anchor_advance_with_a_database_confirms_retained_positions_with_exit_status_0() {
    let site = Site::new();
    let (_, first_revision) = site.bootstrap();
    let advance = |anchor: &Path, out: &Path, with_database: bool| {
        let mut command = arguments(&[
            &words(&["anchor", "advance"]),
            &at("--anchor", anchor),
            &at("--checkpoint-dir", &site.log_dir),
            &at("--out", out),
        ]);
        if with_database {
            command.extend(at("--data", &site.database));
        }
        run(command)
    };

    // The auditor advances twice from the records alone: the third revision
    // retains the position that the second one pinned.
    let second_revision = site.holder.join("anchor-2.json");
    let advanced = advance(&first_revision, &second_revision, false);
    assert_eq!(advanced.code, 0, "{}", advanced.stdout);
    assert_eq!(advanced.json()["status"], "advanced");
    append_notes(&site.database, &["beta"]);
    let created = site.create();
    assert_eq!(created.code, 0, "{}", created.stdout);
    assert_eq!(created.json()["log_index"], 2);
    let third_revision = site.holder.join("anchor-3.json");
    let advanced = advance(&second_revision, &third_revision, false);
    assert_eq!(advanced.code, 0, "{}", advanced.stdout);
    let document = advanced.json();
    assert_eq!(document["status"], "advanced");
    let retained = document["anchor"]["reference"]["retained_checkpoints"].clone();
    assert_eq!(retained.as_array().map(Vec::len), Some(1), "{retained}");
    assert_eq!(retained[0]["log_index"], 1);
    let third_bytes = std::fs::read(&third_revision).unwrap();

    // No record was signed since. With the database, the command compares it
    // with the retained position and with the reference, writes a revision
    // that keeps the reference and retains nothing, and reports `confirmed`
    // as a success.
    let fourth_revision = site.holder.join("anchor-4.json");
    let confirmed = advance(&third_revision, &fourth_revision, true);
    assert_eq!(confirmed.code, 0, "{}", confirmed.stdout);
    assert!(confirmed.stderr.is_empty(), "{}", confirmed.stderr);
    let document = confirmed.json();
    assert_eq!(document["format"], "ai-guard.audit.checkpoint-command.v1");
    assert_eq!(document["command"], "anchor advance");
    assert_eq!(document["status"], "confirmed");
    let fourth_bytes = std::fs::read(&fourth_revision).unwrap();
    let written = &document["anchor"];
    assert_eq!(written["status"], "confirmed");
    assert_eq!(
        std::fs::read(written["anchor_file"].as_str().unwrap()).unwrap(),
        fourth_bytes
    );
    assert_eq!(written["revision"], 4);
    assert_eq!(written["sha256"], sha256(&fourth_bytes).as_str());
    assert_eq!(
        written["previous_anchor_sha256"],
        sha256(&third_bytes).as_str()
    );
    assert_eq!(
        written["reference"]["log_index"],
        created.json()["log_index"]
    );
    assert_eq!(
        written["reference"]["statement_sha256"],
        created.json()["statement_sha256"]
    );
    assert_eq!(
        written["reference"]["retained_checkpoints"],
        serde_json::json!([])
    );
    let warned_without_database = written["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning["code"] == "reference_advanced_without_database");
    assert!(!warned_without_database, "{}", confirmed.stdout);
    let verification = &document["verification"];
    assert_eq!(verification["verdict"], "verified");
    assert_eq!(verification["database"]["compared"], true);
    assert_eq!(verification["database"]["positions_compared"], 2);
    assert_eq!(verification["database"]["extends_checkpoint"], true);
    assert_eq!(verification["errors"], serde_json::json!([]));
    // The file is the revision that the document describes, and the revision
    // it confirms is kept byte for byte.
    let stored: Value = serde_json::from_slice(&fourth_bytes).unwrap();
    assert_eq!(stored["revision"], 4);
    assert_eq!(stored["reference"], written["reference"]);
    assert_eq!(std::fs::read(&third_revision).unwrap(), third_bytes);

    // Nothing is left to confirm: the same command now reports `unchanged`
    // and writes no file.
    let idle = site.holder.join("anchor-idle.json");
    let unchanged = advance(&fourth_revision, &idle, true);
    assert_eq!(unchanged.code, 0, "{}", unchanged.stdout);
    assert_eq!(unchanged.json()["status"], "unchanged");
    assert_eq!(unchanged.json()["anchor"], Value::Null);
    assert!(!idle.exists());
}

/// Run with a deadline. `None` means that the command did not finish and was
/// killed.
fn run_within(arguments: Vec<std::ffi::OsString>, deadline: std::time::Duration) -> Option<Run> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ai-guard-audit-checkpoint"))
        .args(arguments)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let started = std::time::Instant::now();
    while child.try_wait().unwrap().is_none() {
        if started.elapsed() > deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let output = child.wait_with_output().unwrap();
    Some(Run {
        code: output
            .status
            .code()
            .expect("the command exits with a status"),
        stdout: String::from_utf8(output.stdout).unwrap(),
        stderr: String::from_utf8(output.stderr).unwrap(),
    })
}

#[test]
fn diagnose_stays_bounded_when_a_record_sits_at_a_far_log_index() {
    let site = Site::new();
    let (_, anchor) = site.bootstrap();

    // A well-formed record at the highest log index. Listing a record checks
    // no signature: whoever can write the directory can plant this one.
    let far = u64::try_from(i64::MAX).unwrap();
    let statement = format!(
        "{{\"type\":\"ai-guard.audit.checkpoint\",\"version\":1,\"kind\":\"checkpoint\",\"tenant_id\":\"{TENANT}\",\"namespace_uuid\":\"6f1c2a9e-3b7d-4c5e-8f90-1a2b3c4d5e6f\",\"log_index\":{far},\"previous_statement_sha256\":\"{one}\",\"created_at\":\"2026-11-10T09:00:00.000Z\",\"signing_key_id\":\"{two}\",\"audit_schema_version\":11,\"chain_algorithm\":\"agentsandbox:audit:v1\",\"event_count\":1,\"head_hash\":\"{three}\",\"control_genesis_seq\":1,\"control_genesis_hash\":\"{three}\",\"previous_event_count\":0,\"previous_head_hash\":\"{zero}\"}}",
        one = "1".repeat(64),
        two = "2".repeat(64),
        three = "3".repeat(64),
        zero = "0".repeat(64),
    );
    let record = format!(
        "{{\"format\":\"ai-guard.audit.signed-checkpoint.v1\",\"statement\":{statement},\"signatures\":[{{\"key_id\":\"{}\",\"signature\":\"{}\"}}]}}",
        "2".repeat(64),
        "A".repeat(86),
    );
    write_private(
        &site.log_dir.join(format!(
            "cp-{far:020}-{}.json",
            sha256(statement.as_bytes())
        )),
        record.as_bytes(),
    );

    let outcome = run_within(
        arguments(&[
            &words(&["diagnose"]),
            &at("--checkpoint-dir", &site.log_dir),
        ]),
        std::time::Duration::from_secs(10),
    );
    let diagnosed = outcome.expect("diagnose must not walk the distance between two log indexes");
    assert_eq!(diagnosed.code, 0, "{}", diagnosed.stdout);
    let report = diagnosed.json();
    // The planted file is a complete record for the listing.
    assert_eq!(report["checkpoint_log"]["complete_records"], 2);
    assert_eq!(report["checkpoint_log"]["first_log_index"], 1);
    assert_eq!(report["checkpoint_log"]["head"]["log_index"], far);
    let gap = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|finding| finding["code"] == "log_gap")
        .expect("the gap must be reported");
    assert_eq!(gap["detail"]["count"], far - 2);
    assert_eq!(
        gap["detail"]["missing_ranges"],
        serde_json::json!([{ "first_log_index": 2, "last_log_index": far - 1 }])
    );

    // The commands that decide stop at the first missing index.
    let verified = run_within(
        arguments(&[
            &words(&["verify"]),
            &at("--anchor", &anchor),
            &at("--checkpoint-dir", &site.log_dir),
        ]),
        std::time::Duration::from_secs(10),
    )
    .expect("verify must stop at the first missing log index");
    assert_eq!(verified.code, 2, "{}", verified.stdout);
    assert_eq!(verified.error_codes(), vec!["log_discontinuous"]);
    let exported = run_within(
        arguments(&[
            &words(&["export"]),
            &at("--checkpoint-dir", &site.log_dir),
            &at("--out", &site.holder.join("far.proof.json")),
        ]),
        std::time::Duration::from_secs(10),
    )
    .expect("export must stop at the first missing log index");
    assert_eq!(exported.code, 1, "{}", exported.stdout);
    assert_eq!(exported.error_codes(), vec!["invalid_argument"]);
}

#[test]
fn emptied_legacy_or_foreign_database_is_refused_as_content() {
    let site = Site::new();
    let (_, anchor) = site.bootstrap();
    let referenced = site.holder.join("anchor-2.json");
    let advanced = run(arguments(&[
        &words(&["anchor", "advance"]),
        &at("--anchor", &anchor),
        &at("--checkpoint-dir", &site.log_dir),
        &at("--data", &site.database),
        &at("--out", &referenced),
    ]));
    assert_eq!(advanced.code, 0, "{}", advanced.stdout);
    let verify_with = |database: &Path| {
        run(arguments(&[
            &words(&["verify", "--require-reference"]),
            &at("--anchor", &referenced),
            &at("--checkpoint-dir", &site.log_dir),
            &at("--data", database),
        ]))
    };
    let create_with = |database: &Path| {
        run(arguments(&[
            &words(&["create", "--tenant-id", TENANT]),
            &at("--data", database),
            &at("--key-dir", &site.key_dir),
            &at("--checkpoint-dir", &site.log_dir),
        ]))
    };
    let records_before = std::fs::read_dir(&site.log_dir).unwrap().count();

    // Three files that are not an audit database of this build: emptied to
    // zero bytes, a legacy database that no gateway has versioned yet, and a
    // file that is not SQLite at all.
    let emptied = private_child(site.root.path(), "emptied").join("audit.sqlite");
    write_private(&emptied, b"");
    let legacy = private_child(site.root.path(), "legacy-0").join("audit.sqlite");
    raw_database(&legacy, 0);
    let legacy_before = std::fs::read(&legacy).unwrap();
    let foreign = private_child(site.root.path(), "foreign").join("audit.sqlite");
    write_private(
        &foreign,
        &b"this file is not a SQLite database\n".repeat(64),
    );

    // Each is refused as content, with exit status 2: none is "a schema newer
    // than this build" or "a database that might be busy".
    let observed: Vec<(i32, Vec<String>)> = [&emptied, &legacy, &foreign]
        .iter()
        .map(|database| {
            let outcome = create_with(database);
            (outcome.code, outcome.error_codes())
        })
        .collect();
    assert_eq!(
        observed,
        vec![
            (2, vec!["audit_schema_unversioned".to_owned()]),
            (2, vec!["audit_schema_unversioned".to_owned()]),
            (2, vec!["audit_chain_invalid".to_owned()]),
        ]
    );

    // Against a signed position, an emptied database is the most complete
    // truncation: the verification reports it, with the facts it read.
    let refused = verify_with(&emptied);
    assert_eq!(refused.code, 2, "{}", refused.stdout);
    let report = refused.json();
    assert_eq!(
        report["format"],
        "ai-guard.audit.checkpoint-verification.v1"
    );
    assert_eq!(report["verdict"], "refused");
    assert_eq!(refused.error_codes(), vec!["audit_schema_unversioned"]);
    assert_eq!(report["errors"][0]["detail"]["user_version"], 0);
    assert_eq!(report["errors"][0]["detail"]["schema_objects"], 0);
    assert!(
        report["errors"][0]["action"]
            .as_str()
            .unwrap()
            .contains("truncation")
    );
    assert_eq!(report["database"]["compared"], false);
    let diagnosed = run(arguments(&[
        &words(&["diagnose", "--tenant-id", TENANT]),
        &at("--data", &emptied),
    ]));
    assert_eq!(diagnosed.code, 0, "{}", diagnosed.stdout);
    assert_eq!(diagnosed.json()["database"]["readable"], false);
    assert_eq!(
        diagnosed.json()["database"]["error"]["code"],
        "audit_schema_unversioned"
    );
    assert!(std::fs::read(&emptied).unwrap().is_empty());

    // The legacy database names the objects it holds and the maintenance to
    // run, and is left untouched.
    let refused = create_with(&legacy);
    let error = &refused.json()["errors"][0];
    assert_eq!(error["detail"]["user_version"], 0);
    assert!(error["detail"]["schema_objects"].as_u64().unwrap() > 0);
    assert!(
        error["action"]
            .as_str()
            .unwrap()
            .contains("migrate-control --target-version 2")
    );
    assert_eq!(std::fs::read(&legacy).unwrap(), legacy_before);

    // The foreign file is a refusal in a verification report too.
    let refused = verify_with(&foreign);
    assert_eq!(refused.code, 2, "{}", refused.stdout);
    assert_eq!(refused.error_codes(), vec!["audit_chain_invalid"]);
    assert_eq!(refused.json()["database"]["compared"], false);

    // Nothing was signed, and the intact database still verifies.
    assert_eq!(
        std::fs::read_dir(&site.log_dir).unwrap().count(),
        records_before
    );
    let verified = verify_with(&site.database);
    assert_eq!(verified.code, 0, "{}", verified.stdout);
}

#[test]
fn output_location_is_checked_before_any_work() {
    let site = Site::new();
    let (_, anchor) = site.bootstrap();
    // A directory that others in the group can write to: the trusted reader
    // would refuse anything read back from it.
    let shared = site.path("shared");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&shared)
        .unwrap();
    std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o775)).unwrap();
    let refused = |outcome: Run| {
        assert_eq!(outcome.code, 1, "{}", outcome.stdout);
        assert_eq!(
            outcome.error_codes(),
            vec!["output_unavailable"],
            "{}",
            outcome.stdout
        );
    };
    refused(run(arguments(&[
        &words(&["verify"]),
        &at("--anchor", &anchor),
        &at("--checkpoint-dir", &site.log_dir),
        &at("--report-out", &shared.join("report.json")),
    ])));
    refused(run(arguments(&[
        &words(&["export"]),
        &at("--checkpoint-dir", &site.log_dir),
        &at("--out", &shared.join("proof.json")),
    ])));
    refused(run(arguments(&[
        &words(&["anchor", "advance"]),
        &at("--anchor", &anchor),
        &at("--checkpoint-dir", &site.log_dir),
        &at("--out", &shared.join("anchor-2.json")),
    ])));
    refused(run(arguments(&[
        &words(&["key", "show"]),
        &at("--key-dir", &site.key_dir),
        &at("--out", &shared.join("key.public-key.json")),
    ])));
    assert_eq!(std::fs::read_dir(&shared).unwrap().count(), 0);
}

#[test]
fn existing_outputs_and_key_directories_are_never_overwritten() {
    let site = Site::new();
    let (key_id, anchor) = site.bootstrap();
    let key_file = site.key_dir.join("signing-key.pk8");
    let key_bytes = std::fs::read(&key_file).unwrap();
    let anchor_bytes = std::fs::read(&anchor).unwrap();
    let public_key = site.holder.join("key-1.public-key.json");
    let public_key_bytes = std::fs::read(&public_key).unwrap();
    let exists = |outcome: Run, code: &str| {
        assert_eq!(outcome.code, 1, "{}", outcome.stdout);
        assert_eq!(outcome.error_codes(), vec![code], "{}", outcome.stdout);
    };

    // A key directory is never reused, even an empty one left by an
    // interrupted initialization.
    exists(
        run(arguments(&[
            &words(&["key", "init"]),
            &at("--key-dir", &site.key_dir),
        ])),
        "key_directory_exists",
    );
    let interrupted = private_child(&site.path("keys"), "interrupted");
    exists(
        run(arguments(&[
            &words(&["key", "init"]),
            &at("--key-dir", &interrupted),
        ])),
        "key_directory_exists",
    );
    assert_eq!(std::fs::read_dir(&interrupted).unwrap().count(), 0);
    assert_eq!(std::fs::read(&key_file).unwrap(), key_bytes);

    // Every --out and --report-out refuses an existing path.
    exists(
        run(arguments(&[
            &words(&["key", "show"]),
            &at("--key-dir", &site.key_dir),
            &at("--out", &anchor),
        ])),
        "output_exists",
    );
    let namespace: Value = serde_json::from_slice(&anchor_bytes).unwrap();
    exists(
        run(arguments(&[
            &words(&[
                "anchor",
                "init",
                "--tenant-id",
                TENANT,
                "--expect-key-id",
                &key_id,
                "--namespace-uuid",
                namespace["namespace_uuid"].as_str().unwrap(),
            ]),
            &at("--public-key", &public_key),
            &at("--out", &anchor),
        ])),
        "output_exists",
    );
    exists(
        run(arguments(&[
            &words(&["anchor", "advance"]),
            &at("--anchor", &anchor),
            &at("--checkpoint-dir", &site.log_dir),
            &at("--out", &public_key),
        ])),
        "output_exists",
    );
    exists(
        run(arguments(&[
            &words(&["export"]),
            &at("--checkpoint-dir", &site.log_dir),
            &at("--out", &anchor),
        ])),
        "output_exists",
    );
    exists(
        run(arguments(&[
            &words(&["verify"]),
            &at("--anchor", &anchor),
            &at("--checkpoint-dir", &site.log_dir),
            &at("--report-out", &public_key),
        ])),
        "output_exists",
    );
    assert_eq!(std::fs::read(&anchor).unwrap(), anchor_bytes);
    assert_eq!(std::fs::read(&public_key).unwrap(), public_key_bytes);

    // Outputs are private files; record files are never rewritten.
    assert_eq!(
        std::fs::metadata(&anchor).unwrap().permissions().mode() & 0o7777,
        0o600
    );
    let record = std::fs::read_dir(&site.log_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .unwrap();
    let record_bytes = std::fs::read(&record).unwrap();
    let unchanged = site.create();
    assert_eq!(unchanged.code, 0, "{}", unchanged.stdout);
    assert_eq!(unchanged.json()["status"], "unchanged");
    assert_eq!(std::fs::read(&record).unwrap(), record_bytes);
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

#[test]
fn a_failed_write_is_an_io_failure_that_keeps_the_partial_file_and_resumes() {
    let site = Site::new();
    let (_, anchor) = site.bootstrap();

    // A public output that cannot be written: the command says so, exits 1,
    // and leaves the empty file it created.
    let public_key = site.holder.join("no-space.public-key.json");
    let show = |out: &Path| {
        arguments(&[
            &words(&["key", "show"]),
            &at("--key-dir", &site.key_dir),
            &at("--out", out),
        ])
    };
    let failed = run_without_file_space(show(&public_key));
    assert_eq!(failed.code, 1, "{}", failed.stdout);
    assert_eq!(
        failed.error_codes(),
        vec!["io_failure"],
        "{}",
        failed.stdout
    );
    assert_eq!(failed.stderr, "");
    let error = &failed.json()["errors"][0];
    assert_eq!(error["detail"]["path"], public_key.to_str().unwrap());
    assert!(
        error["action"]
            .as_str()
            .unwrap()
            .contains("new output paths")
    );
    assert_eq!(std::fs::read(&public_key).unwrap(), b"");
    // The partial file is never reused or overwritten: a new path is needed.
    let reused = run(show(&public_key));
    assert_eq!(reused.code, 1, "{}", reused.stdout);
    assert_eq!(reused.error_codes(), vec!["output_exists"]);
    assert_eq!(std::fs::read(&public_key).unwrap(), b"");
    let retried = run(show(&site.holder.join("retried.public-key.json")));
    assert_eq!(retried.code, 0, "{}", retried.stdout);

    // A record that cannot be written: same error, and the empty record file
    // stays in the log as an incomplete record.
    let new_key_dir = site.path("keys").join("key-2");
    let key = run(arguments(&[
        &words(&["key", "init"]),
        &at("--key-dir", &new_key_dir),
    ]));
    assert_eq!(key.code, 0, "{}", key.stdout);
    let rotate = || {
        arguments(&[
            &words(&["rotate-key"]),
            &at("--checkpoint-dir", &site.log_dir),
            &at("--key-dir", &site.key_dir),
            &at("--new-key-dir", &new_key_dir),
        ])
    };
    let before = entry_names(&site.log_dir);
    let failed = run_without_file_space(rotate());
    assert_eq!(failed.code, 1, "{}", failed.stdout);
    assert_eq!(
        failed.error_codes(),
        vec!["io_failure"],
        "{}",
        failed.stdout
    );
    assert_eq!(failed.stderr, "");
    let partial = failed.json()["errors"][0]["detail"]["record"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(partial.starts_with(&format!("cp-{:020}-", 2)), "{partial}");
    let mut expected = before.clone();
    expected.push(partial.clone());
    expected.sort();
    assert_eq!(entry_names(&site.log_dir), expected);
    assert_eq!(std::fs::read(site.log_dir.join(&partial)).unwrap(), b"");

    // The log is still trusted through its last complete record, and the
    // report names the debris.
    let verified = run(arguments(&[
        &words(&["verify"]),
        &at("--anchor", &anchor),
        &at("--checkpoint-dir", &site.log_dir),
    ]));
    assert_eq!(verified.code, 0, "{}", verified.stdout);
    let report = verified.json();
    assert_eq!(report["verified_through_log_index"], 1);
    assert_eq!(report["incomplete_records"][0]["name"], partial.as_str());
    let warnings: Vec<&str> = report["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|warning| warning["code"].as_str().unwrap())
        .collect();
    assert!(
        warnings.contains(&"incomplete_record_above_head"),
        "{warnings:?}"
    );

    // Once space is back, the same command resumes at the same log index and
    // keeps the partial file as evidence.
    let rotated = run(rotate());
    assert_eq!(rotated.code, 0, "{}", rotated.stdout);
    let outcome = rotated.json();
    assert_eq!(outcome["status"], "rotated");
    assert_eq!(outcome["log_index"], 2);
    assert_eq!(outcome["incomplete_records"][0]["name"], partial.as_str());
    assert_eq!(std::fs::read(site.log_dir.join(&partial)).unwrap(), b"");
    assert_eq!(entry_names(&site.log_dir).len(), expected.len() + 1);
}
