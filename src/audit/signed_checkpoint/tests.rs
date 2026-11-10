use super::{
    evidence,
    format::{
        CheckpointStatement, RecordSignature, SignedRecord, Statement, parse_record_file_name,
        sha256_hex,
    },
    keys::{SigningKey, verify_signature},
};
use crate::{audit::Store, model::Event, redaction::Redactor};
use std::os::unix::fs::PermissionsExt;

// RFC 8032 section 7.1, TEST 1.
const RFC8032_SEED: &str = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";
const RFC8032_PUBLIC: &str = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";
const RFC8032_SIGNATURE: &str = "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b";
// SHA-256 of the raw RFC 8032 TEST 1 public key, computed independently.
const RFC8032_KEY_ID: &str = "21fe31dfa154a261626bf854046fd2271b7bed4b6abe45aa58877ef47f9721b9";

const FROZEN_STATEMENT: &str = r#"{"type":"ai-guard.audit.checkpoint","version":1,"kind":"checkpoint","tenant_id":"tenant-a","namespace_uuid":"6f1c2a9e-3b7d-4c5e-8f90-1a2b3c4d5e6f","log_index":2,"previous_statement_sha256":"1111111111111111111111111111111111111111111111111111111111111111","created_at":"2026-11-10T09:00:00.000Z","signing_key_id":"21fe31dfa154a261626bf854046fd2271b7bed4b6abe45aa58877ef47f9721b9","audit_schema_version":11,"chain_algorithm":"agentsandbox:audit:v1","event_count":42,"head_hash":"2222222222222222222222222222222222222222222222222222222222222222","control_genesis_seq":1,"control_genesis_hash":"3333333333333333333333333333333333333333333333333333333333333333","previous_event_count":17,"previous_head_hash":"4444444444444444444444444444444444444444444444444444444444444444"}"#;
// SHA-256 of FROZEN_STATEMENT, computed independently with shasum.
const FROZEN_STATEMENT_SHA256: &str =
    "e6ae63c790cdb627184eae9456d121da689c6b5b23eee1e20854817c7b2ccabf";
// Ed25519 signature of the domain prefix followed by FROZEN_STATEMENT with the
// RFC 8032 TEST 1 key, computed independently of this crate. Published in the
// format reference of the guides.
const FROZEN_SIGNATURE: &str =
    "ZQQAfRJ4nYrjgO-L60YjnJIQwE-OHBaA0BV5gkJirThk3D0QmLzD6q7tHExkM_MpEWQJAoC5PDhazeF2T24cAA";

/// PKCS#8 v2 document of the RFC 8032 TEST 1 key: fixed prefix, seed, public
/// key marker, public key.
fn rfc8032_pkcs8() -> Vec<u8> {
    let mut document = hex::decode("3051020101300506032b657004220420").unwrap();
    document.extend(hex::decode(RFC8032_SEED).unwrap());
    document.extend(hex::decode("812100").unwrap());
    document.extend(hex::decode(RFC8032_PUBLIC).unwrap());
    document
}

fn rfc8032_key() -> SigningKey {
    SigningKey::from_pkcs8(&rfc8032_pkcs8()).unwrap()
}

fn frozen_statement() -> Statement {
    Statement::Checkpoint(CheckpointStatement {
        statement_type: "ai-guard.audit.checkpoint".into(),
        version: 1,
        kind: "checkpoint".into(),
        tenant_id: "tenant-a".into(),
        namespace_uuid: "6f1c2a9e-3b7d-4c5e-8f90-1a2b3c4d5e6f".into(),
        log_index: 2,
        previous_statement_sha256: "1".repeat(64),
        created_at: "2026-11-10T09:00:00.000Z".into(),
        signing_key_id: RFC8032_KEY_ID.into(),
        audit_schema_version: 11,
        chain_algorithm: "agentsandbox:audit:v1".into(),
        event_count: 42,
        head_hash: "2".repeat(64),
        control_genesis_seq: 1,
        control_genesis_hash: "3".repeat(64),
        previous_event_count: 17,
        previous_head_hash: "4".repeat(64),
    })
}

fn frozen_record() -> SignedRecord {
    let statement = frozen_statement();
    let signature = rfc8032_key().sign(&statement.canonical_bytes().unwrap());
    SignedRecord::seal(statement, vec![signature]).unwrap()
}

fn record_text() -> String {
    String::from_utf8(frozen_record().to_bytes().unwrap()).unwrap()
}

fn refusal(text: &str) -> &'static str {
    SignedRecord::parse(text.as_bytes()).unwrap_err().code()
}

#[test]
fn ed25519_matches_the_rfc_8032_known_answer() {
    let key = rfc8032_key();
    assert_eq!(hex::encode(key.public_key()), RFC8032_PUBLIC);
    assert_eq!(key.key_id(), RFC8032_KEY_ID);
    // The RFC vector signs the empty message without any prefix: exercise the
    // primitive directly, then the domain-separated wrapper.
    let pair = ring::signature::Ed25519KeyPair::from_pkcs8(&rfc8032_pkcs8()).unwrap();
    assert_eq!(hex::encode(pair.sign(b"").as_ref()), RFC8032_SIGNATURE);
    let signature = key.sign(b"statement");
    assert_eq!(signature.key_id, RFC8032_KEY_ID);
    assert!(verify_signature(
        key.public_key(),
        b"statement",
        &signature.signature
    ));
    assert!(!verify_signature(
        key.public_key(),
        b"statemenT",
        &signature.signature
    ));
    // A signature made without the domain prefix is not accepted.
    let bare = super::format::encode_base64url(pair.sign(b"statement").as_ref());
    assert!(!verify_signature(key.public_key(), b"statement", &bare));
}

#[test]
fn statement_bytes_are_deterministic_and_match_the_frozen_vector() {
    let statement = frozen_statement();
    let bytes = statement.canonical_bytes().unwrap();
    assert_eq!(String::from_utf8(bytes.clone()).unwrap(), FROZEN_STATEMENT);
    assert_eq!(sha256_hex(&bytes), FROZEN_STATEMENT_SHA256);
    assert_eq!(statement.canonical_bytes().unwrap(), bytes);
    let record = frozen_record();
    assert_eq!(record.statement_bytes(), FROZEN_STATEMENT.as_bytes());
    assert_eq!(record.statement_sha256(), FROZEN_STATEMENT_SHA256);
    assert_eq!(record.signatures()[0].signature, FROZEN_SIGNATURE);
    assert_eq!(FROZEN_STATEMENT.len(), 772);
    assert_eq!(
        record.file_name(),
        format!("cp-00000000000000000002-{FROZEN_STATEMENT_SHA256}.json")
    );
    assert_eq!(
        parse_record_file_name(&record.file_name()),
        Some((2, FROZEN_STATEMENT_SHA256))
    );
    // Ed25519 is deterministic: the same key and statement give the same record.
    assert_eq!(
        frozen_record().to_bytes().unwrap(),
        record.to_bytes().unwrap()
    );
    let parsed = SignedRecord::parse(&record.to_bytes().unwrap()).unwrap();
    assert_eq!(parsed, record);
    assert!(verify_signature(
        rfc8032_key().public_key(),
        parsed.statement_bytes(),
        &parsed.signatures()[0].signature
    ));
}

#[test]
fn strict_parser_refuses_every_non_canonical_or_extended_form() {
    let text = record_text();
    assert!(SignedRecord::parse(text.as_bytes()).is_ok());

    // Optional whitespace, inside the statement and inside the envelope.
    let spaced_statement = text.replacen("\"statement\":{", "\"statement\":{ ", 1);
    assert_eq!(refusal(&spaced_statement), "record_not_canonical");
    let spaced_envelope = text.replacen("\"statement\":", "\"statement\": ", 1);
    assert_eq!(refusal(&spaced_envelope), "record_not_canonical");
    assert_eq!(refusal(&format!("{text}\n")), "record_not_canonical");

    // Reordered statement fields carry the same content in another byte form.
    let reordered = text.replacen(
        "\"type\":\"ai-guard.audit.checkpoint\",\"version\":1,",
        "\"version\":1,\"type\":\"ai-guard.audit.checkpoint\",",
        1,
    );
    assert_ne!(reordered, text);
    assert_eq!(refusal(&reordered), "record_not_canonical");

    // Unknown fields: a key shipped inside the envelope or the statement is
    // refused, never read as a trust root.
    let embedded_key = text.replacen(
        "{\"format\":",
        "{\"public_key\":\"11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo\",\"format\":",
        1,
    );
    assert_eq!(refusal(&embedded_key), "record_malformed");
    let unknown_statement_field =
        text.replacen("\"version\":1,", "\"version\":1,\"note\":\"x\",", 1);
    assert_eq!(refusal(&unknown_statement_field), "record_malformed");

    // Duplicated and missing fields.
    let duplicated = text.replacen("\"version\":1,", "\"version\":1,\"version\":1,", 1);
    assert_eq!(refusal(&duplicated), "record_malformed");
    let missing = text.replacen("\"audit_schema_version\":11,", "", 1);
    assert_ne!(missing, text);
    assert_eq!(refusal(&missing), "record_malformed");

    // Uppercase hexadecimal and non-canonical numbers, times and base64.
    let uppercase = text.replacen(&"2".repeat(64), &"A".repeat(64), 1);
    assert_eq!(refusal(&uppercase), "record_malformed");
    let decimal = text.replacen("\"event_count\":42,", "\"event_count\":42.0,", 1);
    assert_eq!(refusal(&decimal), "record_malformed");
    let offset_time = text.replacen("2026-11-10T09:00:00.000Z", "2026-11-10T09:00:00+00:00", 1);
    assert_eq!(refusal(&offset_time), "record_malformed");
    let signature = frozen_record().signatures()[0].signature.clone();
    let padded = text.replacen(&signature, &format!("{signature}=="), 1);
    assert_eq!(refusal(&padded), "record_malformed");
    let standard_alphabet = text.replacen(
        &signature,
        &signature.replace('-', "+").replace('_', "/"),
        1,
    );
    if standard_alphabet != text {
        assert_eq!(refusal(&standard_alphabet), "record_malformed");
    }
    let short = text.replacen(&signature, &signature[..84], 1);
    assert_eq!(refusal(&short), "record_malformed");

    // Unsupported contract values.
    let other_version = text.replacen("\"version\":1,", "\"version\":2,", 1);
    assert_eq!(refusal(&other_version), "record_malformed");
    let other_chain = text.replacen("agentsandbox:audit:v1", "agentsandbox:audit:v2", 1);
    assert_eq!(refusal(&other_chain), "record_malformed");
    let regressing = text.replacen(
        "\"previous_event_count\":17,",
        "\"previous_event_count\":43,",
        1,
    );
    assert_eq!(refusal(&regressing), "record_malformed");
    let three_signatures = {
        let entry = format!("{{\"key_id\":\"{RFC8032_KEY_ID}\",\"signature\":\"{signature}\"}}");
        text.replacen(&entry, &format!("{entry},{entry},{entry}"), 1)
    };
    assert_ne!(three_signatures, text);
    assert_eq!(refusal(&three_signatures), "record_malformed");
    assert_eq!(refusal(&"x".repeat(4096)), "record_malformed");
}

#[test]
fn sealing_refuses_a_statement_that_the_parser_would_refuse() {
    let Statement::Checkpoint(mut statement) = frozen_statement() else {
        unreachable!("the frozen statement is a checkpoint");
    };
    statement.tenant_id = "tenant a".into();
    let signature = RecordSignature {
        key_id: RFC8032_KEY_ID.into(),
        signature: frozen_record().signatures()[0].signature.clone(),
    };
    let failure =
        SignedRecord::seal(Statement::Checkpoint(statement), vec![signature]).unwrap_err();
    assert_eq!(failure.code(), "record_malformed");
}

#[test]
fn signing_key_debug_output_carries_no_private_material() {
    let key = rfc8032_key();
    let shown = format!("{key:?} {:?}", Some(&key));
    assert!(shown.contains(RFC8032_KEY_ID));
    assert!(shown.contains("<redacted>"));
    let seed = hex::decode(RFC8032_SEED).unwrap();
    for needle in [
        RFC8032_SEED.to_owned(),
        super::format::encode_base64url(&seed),
        hex::encode(rfc8032_pkcs8()),
    ] {
        assert!(!shown.contains(&needle), "private material leaked: {shown}");
    }
}

fn private_directory() -> tempfile::TempDir {
    tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}

#[test]
fn evidence_reader_releases_the_initialization_lock_before_its_scan_and_reads_one_snapshot() {
    let directory = private_directory();
    let path = directory.path().join("audit.sqlite");
    let live = Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    live.append(Event::new("fixture.before")).unwrap();
    let before = live.integrity().unwrap();
    assert!(before.valid);
    let mut appended = None;
    let mut concurrent_open = None;
    let evidence = evidence::read_with_observer(&path, "tenant-a", &[1, 2, 3], &mut || {
        appended = Some(
            live.append(Event::new("fixture.during"))
                .map(|event| event.seq)
                .map_err(|error| error.to_string()),
        );
        concurrent_open = Some(
            Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000)
                .and_then(|store| store.head())
                .map_err(|error| error.to_string()),
        );
    })
    .unwrap();
    assert_eq!(
        appended,
        Some(Ok(3)),
        "a live writer must not be blocked while the evidence reader holds its snapshot"
    );
    assert_eq!(
        concurrent_open,
        Some(Ok(3)),
        "a concurrent maintenance open must succeed during the evidence scan"
    );
    // The evidence is the snapshot taken before the concurrent append.
    assert_eq!(evidence.event_count, before.event_count);
    assert_eq!(evidence.head_hash, before.head_hash);
    assert_eq!(evidence.event_count, 2);
    assert_eq!(evidence.hashes.len(), 2);
    assert_eq!(evidence.hashes.get(&2), Some(&before.head_hash));
    assert_eq!(evidence.control_genesis_seq, 1);
    assert_eq!(
        evidence.hashes.get(&1),
        Some(&evidence.control_genesis_hash)
    );
    assert!(evidence.audit_schema_version >= 2);
    assert_eq!(live.head().unwrap(), 3);
    assert!(live.integrity().unwrap().valid);
}
