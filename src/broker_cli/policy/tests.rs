use super::*;
use clap::Parser;
use std::{fs, io::Write, os::unix::fs::{OpenOptionsExt, PermissionsExt}, path::Path};

const OP: &str = "11111111-1111-4111-8111-111111111111";
const NS: &str = "22222222-2222-4222-8222-222222222222";

#[derive(Parser)]
struct ParserFixture { #[command(subcommand)] _command: PolicyCommand }

fn fixture() -> tempfile::TempDir {
    tempfile::Builder::new().prefix("a1-cli-").permissions(fs::Permissions::from_mode(0o700)).tempdir().unwrap()
}
fn write(path: &Path, bytes: &[u8]) {
    let mut file = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path).unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}
fn input() -> Vec<u8> {
    format!(r#"{{"version":1,"action":"replace","request":{{"version":1,"expected_scope":{{"tenant_id":"tenant-a","control_namespace_uuid":"{NS}"}},"operation_id":"{OP}","selector":{{"kind":"tenant"}},"expected_revision":0,"expected_epoch":0,"policy":{{"not_before_ms":1000,"expires_at_ms":2000,"destinations":[],"max_destinations":1,"max_resolution_ips":1}},"tenant_ceiling":null}}}}"#).into_bytes()
}
fn prepared(temp: &tempfile::TempDir) -> Loaded {
    write(&temp.path().join("input.json"), &input());
    storage::prepare("http://127.0.0.1:1", &temp.path().join("input.json"), &temp.path().join("operation")).unwrap()
}

#[test]
fn policy_parser_accepts_four_exact_commands_and_refuses_apply_scope_overrides() {
    for argv in [
        vec!["policy", "prepare", "--gateway", "http://127.0.0.1:1", "--input", "/synthetic/input", "--output-dir", "/synthetic/new"],
        vec!["policy", "apply", "--operation-dir", "/synthetic/operation", "--token-file", "/synthetic/token"],
        vec!["policy", "resume", "--operation-dir", "/synthetic/operation", "--token-file", "/synthetic/token"],
        vec!["policy", "status", "--operation-dir", "/synthetic/operation"],
    ] { assert!(ParserFixture::try_parse_from(argv).is_ok()); }
    for option in ["--gateway", "--expected-scope", "--operation-id", "--expected-revision", "--force"] {
        assert!(ParserFixture::try_parse_from(["policy", "apply", "--operation-dir", "/synthetic/operation", "--token-file", "/synthetic/token", option, "override"]).is_err());
    }
    assert!(ParserFixture::try_parse_from(["policy", "status"]).is_err());
}

#[test]
fn canonical_preparation_survives_reopen_with_same_scope_operation_and_zero_attempts() {
    let temp = fixture();
    let loaded = prepared(&temp);
    assert_eq!(loaded.request.operation_id().to_string(), OP);
    assert_eq!(loaded.request.scope().tenant_id(), "tenant-a");
    assert_eq!(loaded.request.scope().control_namespace_uuid(), NS);
    let request = fs::read(temp.path().join("operation/request.json")).unwrap();
    drop(loaded);
    let reopened = Loaded::open(&temp.path().join("operation")).unwrap();
    assert_eq!(reopened.state.attempts, 0);
    assert!(reopened.state.phase == Phase::Prepared);
    assert_eq!(reopened.request.operation_id().to_string(), OP);
    assert_eq!(fs::read(temp.path().join("operation/request.json")).unwrap(), request);
    let value: serde_json::Value = serde_json::from_slice(&output(&reopened).unwrap()).unwrap();
    assert_eq!(value["historical"], true);
    assert_eq!(value["coverage"], "policy_registry_only");
    for forbidden in ["gateway", "tenant_id", "control_namespace_uuid", "token", "runtime_applied"] {
        assert!(value.get(forbidden).is_none());
    }
}

#[test]
fn closed_input_and_scope_validation_refuse_before_publishing_a_directory() {
    for case in 0..4 {
        let temp = fixture();
        let mut value: serde_json::Value = serde_json::from_slice(&input()).unwrap();
        match case {
            0 => { value["extra"] = true.into(); }
            1 => { value["request"].as_object_mut().unwrap().remove("expected_scope"); }
            2 => { value["request"]["expected_scope"] = serde_json::Value::Null; }
            _ => { value["request"]["operation_id"] = "11111111111141118111111111111111".into(); }
        }
        write(&temp.path().join("input.json"), &serde_json::to_vec(&value).unwrap());
        assert!(storage::prepare("http://127.0.0.1:1", &temp.path().join("input.json"), &temp.path().join("operation")).is_err());
        assert!(!temp.path().join("operation").exists());
    }
}

#[test]
fn scope_conflict_code_keeps_prior_unresolved_attempt_and_never_rewrites_preparation() {
    let temp = fixture();
    let mut loaded = prepared(&temp);
    let request = fs::read(temp.path().join("operation/request.json")).unwrap();
    loaded.begin_attempt().unwrap();
    loaded.failed_attempt("egress_policy_scope_conflict").unwrap();
    assert!(loaded.state.phase == Phase::RemoteOutcomeUnresolved);
    assert_eq!(loaded.state.attempts, 1);
    drop(loaded);
    let mut reopened = Loaded::open(&temp.path().join("operation")).unwrap();
    assert_eq!(reopened.state.last_error.as_deref(), Some("egress_policy_scope_conflict"));
    assert!(reopened.receipt().unwrap().is_none());
    reopened.begin_attempt().unwrap();
    assert_eq!(reopened.state.attempts, 2);
    assert_eq!(reopened.request.operation_id().to_string(), OP);
    assert_eq!(reopened.request.scope().control_namespace_uuid(), NS);
    assert_eq!(fs::read(temp.path().join("operation/request.json")).unwrap(), request);
    // Local state only: this test does not fabricate an SDK or HTTP verdict.
}

#[test]
fn pending_transition_requires_explicit_local_resume_and_malformed_pending_is_preserved() {
    let temp = fixture();
    let loaded = prepared(&temp);
    let mut next = loaded.state.clone();
    next.phase = Phase::RemoteOutcomeUnresolved;
    next.attempts = 1;
    drop(loaded);
    write(&temp.path().join("operation/current-next.json"), &serde_json::to_vec(&next).unwrap());
    let mut reopened = Loaded::open(&temp.path().join("operation")).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&output(&reopened).unwrap()).unwrap();
    assert_eq!(value["pending_state"], true);
    assert_eq!(value["attempts"], 0);
    reopened.recover_pending().unwrap();
    assert_eq!(reopened.state.attempts, 1);
    assert!(!reopened.has_pending());
    drop(reopened);
    write(&temp.path().join("operation/current-next.json"), b"partial");
    assert!(Loaded::open(&temp.path().join("operation")).is_err());
    assert_eq!(fs::read(temp.path().join("operation/current-next.json")).unwrap(), b"partial");
}

#[test]
fn existing_destination_and_changed_preparation_refuse_without_repair_or_new_intent() {
    let temp = fixture();
    let loaded = prepared(&temp);
    let original = fs::read(temp.path().join("operation/request.json")).unwrap();
    drop(loaded);
    assert!(storage::prepare("http://127.0.0.1:1", &temp.path().join("input.json"), &temp.path().join("operation")).is_err());
    assert_eq!(fs::read(temp.path().join("operation/request.json")).unwrap(), original);
    let mut loaded = Loaded::open(&temp.path().join("operation")).unwrap();
    fs::write(temp.path().join("operation/metadata.json"), b"changed").unwrap();
    assert!(loaded.revalidate().is_err());
    assert!(loaded.begin_attempt().is_err());
    assert_eq!(fs::read(temp.path().join("operation/current.json")).unwrap(), serde_json::to_vec(&loaded.state).unwrap());
}

#[test]
fn input_bound_private_mode_and_invalid_receipt_refuse_without_destroying_evidence() {
    let temp = fixture();
    write(&temp.path().join("large.json"), &vec![b' '; 66_561]);
    assert!(storage::prepare("http://127.0.0.1:1", &temp.path().join("large.json"), &temp.path().join("large-op")).is_err());
    assert!(!temp.path().join("large-op").exists());
    let loaded = prepared(&temp);
    drop(loaded);
    write(&temp.path().join("operation/receipt.json"), b"not a canonical receipt");
    assert!(Loaded::open(&temp.path().join("operation")).is_err());
    assert_eq!(fs::read(temp.path().join("operation/receipt.json")).unwrap(), b"not a canonical receipt");
    fs::set_permissions(temp.path().join("input.json"), fs::Permissions::from_mode(0o644)).unwrap();
    assert!(storage::prepare("http://127.0.0.1:1", &temp.path().join("input.json"), &temp.path().join("unsafe-op")).is_err());
    assert!(!temp.path().join("unsafe-op").exists());
}

#[test]
fn origin_preparation_refuses_credentials_nonloopback_plaintext_and_hidden_query() {
    for gateway in ["http://example.invalid", "https://user:secret@example.invalid", "https://example.invalid/?token=secret", "https://example.invalid/#secret"] {
        let temp = fixture();
        write(&temp.path().join("input.json"), &input());
        let error = storage::prepare(gateway, &temp.path().join("input.json"), &temp.path().join("operation")).err().unwrap().to_string();
        assert_eq!(error, "egress_policy_invalid_origin");
        assert!(!error.contains("secret"));
        assert!(!temp.path().join("operation").exists());
    }
}

#[test]
fn output_failure_and_literal_token_overlap_preserve_the_real_preparation() {
    struct BrokenOutput;
    impl std::io::Write for BrokenOutput {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> { Err(std::io::ErrorKind::BrokenPipe.into()) }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }
    let temp = fixture();
    let loaded = prepared(&temp);
    let request = fs::read(temp.path().join("operation/request.json")).unwrap();
    assert_eq!(emit_to(&loaded, None, &mut BrokenOutput).unwrap_err().to_string(), "egress_policy_output_unresolved");
    let mut sink = Vec::new();
    assert!(emit_to(&loaded, Some(OP), &mut sink).is_err());
    assert!(sink.is_empty());
    assert_eq!(fs::read(temp.path().join("operation/request.json")).unwrap(), request);
    assert_eq!(loaded.state.attempts, 0);
    drop(loaded);
    assert!(Loaded::open(&temp.path().join("operation")).is_ok());
}

#[test]
fn persisted_scope_operation_and_request_hash_must_correlate_on_every_reopen() {
    for field in ["expected_scope", "operation_id", "request_sha256"] {
        let temp = fixture();
        let loaded = prepared(&temp);
        drop(loaded);
        let path = temp.path().join("operation/metadata.json");
        let mut metadata: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        match field {
            "expected_scope" => metadata["expected_scope"]["tenant_id"] = "tenant-b".into(),
            "operation_id" => metadata[field] = "33333333-3333-4333-8333-333333333333".into(),
            _ => metadata[field] = "a".repeat(64).into(),
        }
        let changed = serde_json::to_vec(&metadata).unwrap();
        fs::write(&path, &changed).unwrap();
        assert!(Loaded::open(&temp.path().join("operation")).is_err());
        assert_eq!(fs::read(&path).unwrap(), changed);
    }
}
