use super::*;
use crate::broker_cli::policy::{Phase, storage as mutation};
use agentsandbox::control::egress_policy::EgressPolicyInspectionV1;
use clap::Parser;
use std::{
    fs,
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::Path,
};

const NS: &str = "22222222-2222-4222-8222-222222222222";
const SESSION: &str = "33333333-3333-4333-8333-333333333333";
const ORIGIN: &str = "http://127.0.0.1:1";

fn fixture() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("policy-inspection-cli-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}

fn write(path: &Path, bytes: &[u8]) {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

fn inspection(
    session: bool,
    head: &str,
    ceiling: &str,
    resource: &str,
) -> (EgressPolicyInspectionRequestV1, EgressPolicyInspectionV1) {
    let selector = if session {
        EgressPolicySelectorV1::Session(uuid::Uuid::parse_str(SESSION).unwrap())
    } else {
        EgressPolicySelectorV1::Tenant
    };
    let selector_json = serde_json::to_string(&selector).unwrap();
    let request = EgressPolicyInspectionRequestV1::new(selector).unwrap();
    let raw = format!(
        r#"{{"version":1,"schema_version":15,"scope":{{"tenant_id":"tenant-a","control_namespace_uuid":"{NS}"}},"selector":{selector_json},"head":{head},"tenant_ceiling":{ceiling},"session_resource":"{resource}","observed_at":"2026-11-14T08:00:00.000Z","coverage":"policy_registry_metadata","historical":true}}"#
    );
    let observation = EgressPolicyInspectionV1::from_canonical_bytes(raw.as_bytes()).unwrap();
    observation.validate_for(&request).unwrap();
    (request, observation)
}

fn report(
    temp: &tempfile::TempDir,
    session: bool,
    head: &str,
    ceiling: &str,
    resource: &str,
) -> storage::Report {
    let (request, observation) = inspection(session, head, ceiling, resource);
    storage::Report::prepare(
        ORIGIN,
        &request,
        &observation,
        &temp.path().join("report"),
        || Ok(()),
    )
    .unwrap()
}

fn tenant_report(temp: &tempfile::TempDir) -> storage::Report {
    report(
        temp,
        false,
        r#"{"presence":"absent"}"#,
        r#"{"status":"not_applicable"}"#,
        "not_applicable",
    )
}

fn replace_draft() -> &'static [u8] {
    br#"{"version":1,"action":"replace","policy":{"not_before_ms":1000,"expires_at_ms":2000,"destinations":[],"max_destinations":1,"max_resolution_ips":1}}"#
}

fn derive(temp: &tempfile::TempDir, draft: &[u8]) -> anyhow::Result<mutation::Loaded> {
    write(&temp.path().join("draft.json"), draft);
    mutation::prepare_from_inspection(
        &temp.path().join("report"),
        &temp.path().join("draft.json"),
        &temp.path().join("operation"),
    )
}

#[derive(Parser)]
struct ParserFixture {
    #[command(subcommand)]
    _command: crate::broker_cli::policy::PolicyCommand,
}

#[test]
fn inspection_parser_requires_one_selector_and_one_explicit_prepare_source() {
    for selector in [vec!["--tenant"], vec!["--session", SESSION]] {
        let mut argv = vec![
            "policy", "inspect", "--gateway", ORIGIN, "--token-file", "/private/token",
            "--output-dir", "/private/report",
        ];
        argv.extend(selector);
        assert!(ParserFixture::try_parse_from(argv).is_ok());
    }
    for extra in [vec![], vec!["--tenant", "--session", SESSION]] {
        let mut argv = vec![
            "policy", "inspect", "--gateway", ORIGIN, "--token-file", "/private/token",
            "--output-dir", "/private/report",
        ];
        argv.extend(extra);
        assert!(ParserFixture::try_parse_from(argv).is_err());
    }
    for sources in [
        vec!["--inspection-dir", "/private/report"],
        vec!["--gateway", ORIGIN],
    ] {
        let mut argv = vec!["policy", "prepare", "--input", "/private/input", "--output-dir", "/private/new"];
        argv.extend(sources);
        assert!(ParserFixture::try_parse_from(argv).is_ok());
    }
    assert!(ParserFixture::try_parse_from([
        "policy", "prepare", "--gateway", ORIGIN, "--inspection-dir", "/private/report",
        "--input", "/private/input", "--output-dir", "/private/new",
    ]).is_err());
    assert!(ParserFixture::try_parse_from([
        "policy", "prepare", "--input", "/private/input", "--output-dir", "/private/new",
    ]).is_err());
}

#[test]
fn report_roundtrip_is_private_historical_and_not_a_mutation() {
    let temp = fixture();
    let loaded = tenant_report(&temp);
    assert_eq!(loaded.gateway(), ORIGIN);
    let exact = fs::read(temp.path().join("report/current.json")).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&output(&loaded).unwrap()).unwrap();
    assert_eq!(value["historical"], true);
    assert_eq!(value["coverage"], "policy_registry_metadata");
    for field in ["gateway", "tenant_id", "control_namespace_uuid", "policy", "token", "runtime_applied"] {
        assert!(value.get(field).is_none());
    }
    let mut names: Vec<_> = fs::read_dir(temp.path().join("report")).unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap()).collect();
    names.sort();
    assert_eq!(names, ["current.json", "metadata.json", "owner.lock", "request.json"]);
    assert_eq!(fs::metadata(temp.path().join("report")).unwrap().permissions().mode() & 0o777, 0o700);
    for name in names {
        assert_eq!(fs::metadata(temp.path().join("report").join(name)).unwrap().permissions().mode() & 0o777, 0o600);
    }
    drop(loaded);
    assert!(mutation::Loaded::open(&temp.path().join("report")).is_err());
    let reopened = storage::Report::open(&temp.path().join("report")).unwrap();
    assert_eq!(reopened.observation.canonical_bytes().unwrap(), exact);
}

#[test]
fn report_hashes_selector_and_closed_metadata_must_correlate() {
    for field in ["request_sha256", "observation_sha256", "kind", "gateway", "unknown", "selector"] {
        let temp = fixture();
        drop(tenant_report(&temp));
        let path = temp.path().join("report/metadata.json");
        let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        if field == "selector" {
            let request = EgressPolicyInspectionRequestV1::new(EgressPolicySelectorV1::Session(uuid::Uuid::parse_str(SESSION).unwrap())).unwrap().canonical_bytes().unwrap();
            fs::write(temp.path().join("report/request.json"), &request).unwrap();
            use sha2::{Digest, Sha256};
            value["request_sha256"] = hex::encode(Sha256::digest(&request)).into();
        } else {
            value[field] = match field {
                "kind" => "policy_operation".into(),
                "gateway" => "http://127.0.0.1:1/".into(),
                "unknown" => true.into(),
                _ => "a".repeat(64).into(),
            };
        }
        // Preserve the actual metadata field order so hash/selector failures
        // are not accidentally witnessed only by the canonical-byte guard.
        let mut changed = format!(
            r#"{{"version":{},"kind":{},"gateway":{},"request_sha256":{},"observation_sha256":{}}}"#,
            value["version"], value["kind"], value["gateway"], value["request_sha256"], value["observation_sha256"],
        ).into_bytes();
        if field == "unknown" {
            changed.pop();
            changed.extend_from_slice(br#", "unknown":true}"#);
        }
        fs::write(&path, &changed).unwrap();
        assert!(storage::Report::open(&temp.path().join("report")).is_err(), "{field}");
        assert_eq!(fs::read(&path).unwrap(), changed);
    }
}

#[test]
fn report_never_adopts_pending_or_receipt_and_keeps_real_lock() {
    for extra in ["current-next.json", "receipt.json"] {
        let temp = fixture();
        let loaded = tenant_report(&temp);
        assert!(storage::Report::open(&temp.path().join("report")).is_err());
        drop(loaded);
        write(&temp.path().join("report").join(extra), b"{}");
        assert!(storage::Report::open(&temp.path().join("report")).is_err());
        assert!(derive(&temp, replace_draft()).is_err());
        assert!(!temp.path().join("operation").exists());
        assert_eq!(fs::read(temp.path().join("report").join(extra)).unwrap(), b"{}");
    }
}

#[test]
fn retained_report_refuses_changed_record_and_cannot_repair_itself() {
    let temp = fixture();
    let loaded = tenant_report(&temp);
    let path = temp.path().join("report/current.json");
    let original = fs::read(&path).unwrap();
    fs::write(&path, b"changed").unwrap();
    assert!(loaded.revalidate().is_err());
    fs::write(&path, &original).unwrap();
    assert!(loaded.revalidate().is_err());
    assert_eq!(fs::read(path).unwrap(), original);
}

#[test]
fn publication_revalidates_private_source_and_retains_late_failure_evidence() {
    for fail_at in [1, 2] {
        let temp = fixture();
        let source_path = temp.path().join("synthetic-token");
        write(&source_path, b"private-synthetic-credential");
        let source = PrivateFile::open(&absolute_path(&source_path).unwrap(), 4096).unwrap();
        let calls = std::cell::Cell::new(0);
        let (request, observation) = inspection(false, r#"{"presence":"absent"}"#, r#"{"status":"not_applicable"}"#, "not_applicable");
        let result = storage::Report::prepare(ORIGIN, &request, &observation, &temp.path().join("report"), || {
            let next = calls.get() + 1;
            calls.set(next);
            if next == fail_at {
                fs::write(&source_path, b"changed-synthetic-credential")?;
            }
            source.revalidate().map_err(|_| anyhow!("egress_policy_token_changed"))
        });
        assert!(result.is_err());
        assert_eq!(temp.path().join("report").exists(), fail_at == 2);
        if fail_at == 2 {
            assert!(storage::Report::open(&temp.path().join("report")).is_ok());
        }
    }
}

#[test]
fn derived_destination_cannot_add_records_inside_the_report() {
    fn snapshot(path: &Path) -> Vec<(String, Vec<u8>)> {
        let mut names: Vec<_> = fs::read_dir(path).unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap()).collect();
        names.sort();
        assert_eq!(names, ["current.json", "metadata.json", "owner.lock", "request.json"]);
        names.into_iter().map(|name| {
            let bytes = fs::read(path.join(&name)).unwrap();
            (name, bytes)
        }).collect()
    }
    for (parent, physical_alias) in [("report", false), ("REPORT", true)] {
        let temp = fixture();
        drop(tenant_report(&temp));
        let report_path = temp.path().join("report");
        if physical_alias {
            let original = fs::metadata(&report_path).unwrap();
            match fs::metadata(temp.path().join(parent)) {
                Ok(alias) => {
                    assert_eq!((alias.dev(), alias.ino()), (original.dev(), original.ino()),
                        "case alias must refer to the actual retained report");
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    eprintln!("NOT_APPLICABLE_CASE_ALIAS: filesystem resolves report/REPORT separately; physical alias branch was not exercised (including case-sensitive Linux QA)");
                    continue;
                }
                Err(error) => panic!("case alias inspection failed: {error}"),
            }
        }
        let before = snapshot(&report_path);
        write(&temp.path().join("draft.json"), replace_draft());
        let output = temp.path().join(parent).join("operation");
        if physical_alias {
            assert!(!output.starts_with(&report_path),
                "physical alias branch must not rely on lexical containment");
        }
        let error = mutation::prepare_from_inspection(
            &report_path,
            &temp.path().join("draft.json"),
            &output,
        ).err().unwrap();
        assert_eq!(error.to_string(), "egress_policy_output_overlaps_inspection");
        assert!(!output.exists());
        assert_eq!(snapshot(&report_path), before,
            "refusal must precede every stage/child or record mutation");
        assert!(storage::Report::open(&report_path).is_ok());
        if physical_alias {
            eprintln!("CASE_ALIAS_EXERCISED: actual same-device/inode alias refused before any stage or child; report bytes and inventory preserved");
        }
    }
}

#[test]
fn draft_is_closed_and_never_accepts_scope_cas_or_identity_overrides() {
    for case in 0..11 {
        let temp = fixture();
        drop(tenant_report(&temp));
        let mut value: serde_json::Value = serde_json::from_slice(replace_draft()).unwrap();
        match case {
            0 => value["version"] = 2.into(),
            1 => value["action"] = "unknown".into(),
            2 => { value.as_object_mut().unwrap().remove("policy"); },
            3 => value = serde_json::json!({"version":1,"action":"revoke","policy":null}),
            _ => {
                let field = ["operation_id", "expected_scope", "selector", "expected_revision", "expected_epoch", "tenant_ceiling", "request"][case - 4];
                value[field] = serde_json::json!({});
            }
        }
        assert!(derive(&temp, &serde_json::to_vec(&value).unwrap()).is_err(), "case {case}");
        assert!(!temp.path().join("operation").exists());
    }
}

#[test]
fn derived_tenant_replace_uses_explicit_absence_or_exact_revoke_requested_head() {
    for (head, revision, epoch) in [
        (r#"{"presence":"absent"}"#, 0, 0),
        (r#"{"presence":"present","revision":7,"epoch":9,"state":"revoke_requested"}"#, 7, 9),
    ] {
        let temp = fixture();
        drop(report(&temp, false, head, r#"{"status":"not_applicable"}"#, "not_applicable"));
        let loaded = derive(&temp, replace_draft()).unwrap();
        assert!(loaded.state.phase == Phase::Prepared);
        assert_eq!(loaded.state.attempts, 0);
        assert_eq!(loaded.metadata.gateway, format!("{ORIGIN}/"));
        let id = loaded.request.operation_id();
        assert!(!id.is_nil());
        assert_eq!(loaded.request.scope().tenant_id(), "tenant-a");
        assert_eq!(loaded.request.scope().control_namespace_uuid(), NS);
        let mutation::Request::Replace(request) = &loaded.request else { panic!("expected replace") };
        assert_eq!(request.expected_revision(), revision);
        assert_eq!(request.expected_epoch(), epoch);
        assert!(matches!(request.selector(), EgressPolicySelectorV1::Tenant));
        assert!(request.tenant_ceiling().is_none());
        let bytes = fs::read(temp.path().join("operation/request.json")).unwrap();
        drop(loaded);
        let reopened = mutation::Loaded::open(&temp.path().join("operation")).unwrap();
        assert_eq!(reopened.request.operation_id(), id);
        drop(reopened);
        assert!(mutation::prepare_from_inspection(&temp.path().join("report"), &temp.path().join("draft.json"), &temp.path().join("operation")).is_err());
        assert_eq!(fs::read(temp.path().join("operation/request.json")).unwrap(), bytes);
    }
}

#[test]
fn derived_session_replace_requires_and_copies_the_actual_active_ceiling() {
    let active = format!(r#"{{"status":"active","revision":4,"epoch":6,"digest":"{}"}}"#, "b".repeat(64));
    for (ceiling, resource, allowed) in [
        (active.as_str(), "active", true),
        (active.as_str(), "inactive", false),
        (active.as_str(), "absent", false),
        (r#"{"status":"absent"}"#, "active", false),
        (r#"{"status":"inactive","revision":4,"epoch":6}"#, "active", false),
    ] {
        let temp = fixture();
        drop(report(&temp, true, r#"{"presence":"absent"}"#, ceiling, resource));
        let result = derive(&temp, replace_draft());
        assert_eq!(result.is_ok(), allowed);
        if let Ok(loaded) = result {
            let mutation::Request::Replace(request) = &loaded.request else { panic!("expected replace") };
            let ceiling = request.tenant_ceiling().unwrap();
            assert_eq!((ceiling.revision(), ceiling.epoch()), (4, 6));
            assert_eq!(ceiling.digest(), "b".repeat(64));
            assert!(matches!(request.selector(), EgressPolicySelectorV1::Session(id) if id.to_string() == SESSION));
        } else {
            assert!(!temp.path().join("operation").exists());
        }
    }
}

#[test]
fn derived_revoke_preserves_cleanup_without_an_active_resource_or_ceiling() {
    for (head, allowed) in [
        (r#"{"presence":"present","revision":3,"epoch":5,"state":"active"}"#, true),
        (r#"{"presence":"present","revision":3,"epoch":5,"state":"revoke_requested"}"#, false),
        (r#"{"presence":"absent"}"#, false),
    ] {
        for (ceiling, resource) in [
            (r#"{"status":"absent"}"#, "absent"),
            (r#"{"status":"inactive","revision":4,"epoch":6}"#, "inactive"),
        ] {
            let temp = fixture();
            drop(report(&temp, true, head, ceiling, resource));
            let result = derive(&temp, br#"{"version":1,"action":"revoke"}"#);
            assert_eq!(result.is_ok(), allowed);
            if let Ok(loaded) = result {
                let mutation::Request::Revoke(request) = &loaded.request else { panic!("expected revoke") };
                assert_eq!((request.expected_revision(), request.expected_epoch()), (3, 5));
            } else {
                assert!(!temp.path().join("operation").exists());
            }
        }
    }
}

#[test]
fn private_draft_bounds_and_output_failure_preserve_evidence() {
    for oversized in [false, true] {
        let temp = fixture();
        drop(tenant_report(&temp));
        let draft = if oversized { vec![b' '; 66_561] } else { replace_draft().to_vec() };
        write(&temp.path().join("draft.json"), &draft);
        if !oversized {
            fs::set_permissions(temp.path().join("draft.json"), fs::Permissions::from_mode(0o644)).unwrap();
        }
        assert!(mutation::prepare_from_inspection(&temp.path().join("report"), &temp.path().join("draft.json"), &temp.path().join("operation")).is_err());
        assert!(!temp.path().join("operation").exists());
    }
    struct BrokenOutput;
    impl Write for BrokenOutput {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> { Err(std::io::ErrorKind::BrokenPipe.into()) }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }
    let temp = fixture();
    let loaded = tenant_report(&temp);
    assert!(emit_to(&loaded, None, &mut BrokenOutput).is_err());
    let mut sink = Vec::new();
    assert!(emit_to(&loaded, Some("policy_registry_metadata"), &mut sink).is_err());
    assert!(sink.is_empty());
    drop(loaded);
    assert!(storage::Report::open(&temp.path().join("report")).is_ok());
}
