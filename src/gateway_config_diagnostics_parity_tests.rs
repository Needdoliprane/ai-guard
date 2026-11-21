//! Parity of the operator diagnostics with the loaders of this module.
//!
//! `ai-guard-diagnose` cannot call `load`, `validate_policies` or
//! `prepare_oidc_file`: they are private to the gateway binary, and `load`
//! reads every token. `agentsandbox::diagnostics::gateway_files` restates
//! their closed envelopes and their structural rules. These tests bind that
//! restatement to the real loaders:
//!
//! 1. the field names of the four private envelopes, read from the source of
//!    this module's parent, equal the lists the inspection declares;
//! 2. on registries whose tokens are valid, `load` then `validate_policies`
//!    succeeds exactly when the inspection reports no refusal, with the same
//!    tenants and the same resolved references;
//! 3. `prepare_oidc_file` succeeds exactly when the inspection reports no
//!    refusal, with the same version, plan tenants, audit path and CA bytes;
//! 4. token content is outside the restated half: the loader refuses an
//!    invalid token that the inspection never opens;
//! 5. an authentication audit policy that the inspection reports apart is
//!    refused by `load_oidc`, which accepts the same file without that edit.
//!
//! Limit: a rule added to a loader is caught only when a case of this corpus
//! exercises it. The field tripwire covers the field names of the envelopes;
//! types and serde attributes are covered only by the corpus.
//! Every credential value here is synthetic.

use super::{load, load_oidc, prepare_oidc_file, validate_policies};
use agentsandbox::diagnostics::{
    catalogue::FindingCode,
    gateway_files::{
        AUTHENTICATION_AUDIT_FIELDS, CaBundle, OIDC_FILE_FIELDS, OidcInspection, REGISTRY_FIELDS,
        Refusal, TENANT_FIELDS, inspect_oidc_file, inspect_registry,
    },
    report::{Details, PolicyKind, Reason},
};
use agentsandbox::redaction::Redactor;
use serde_json::{Value, json};
use std::{
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
};

const SOURCE: &str = include_str!("gateway_config.rs");
const HTTPS_FILE: &str = include_str!("../docs/setup/sso/examples/gateway.https.json");
const DEMO_MANIFEST: &str = include_str!("../docs/setup/sso/examples/local-demo.json");
/// One public certificate; its key was never written.
const PUBLIC_TEST_ROOT: &str =
    include_str!("../tests/diagnose_support/fixtures/public-test-root.pem");
const DEFAULT_MAX_EVENTS: u64 = 100_000;
/// What the private-file reader says of a file above the bound it is given
/// (`src/control_fs.rs`).
const SIZE_REFUSAL: &str = "private file exceeds the configured size limit";

fn private_directory() -> tempfile::TempDir {
    tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .expect("create a private parity test directory")
}

fn write_mode(path: &Path, bytes: impl AsRef<[u8]>, mode: u32) {
    std::fs::write(path, bytes).expect("write a parity fixture");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .expect("set the mode of a parity fixture");
}

fn write_json(path: &Path, value: &Value) {
    write_mode(path, serde_json::to_vec_pretty(value).unwrap(), 0o600);
}

// ---- 1. field tripwire ----------------------------------------------------

/// Field names of a closed envelope of `gateway_config.rs`, read from its
/// source. A field starts at the first indentation level; attributes and the
/// continuation lines of a type do not.
fn declared_fields(name: &str) -> Vec<String> {
    let header = format!("struct {name} {{\n");
    let start = SOURCE
        .find(&header)
        .unwrap_or_else(|| panic!("gateway_config.rs declares no struct {name}"));
    let declaration = &SOURCE[..start];
    let attributes = declaration.strip_suffix("pub(crate) ").unwrap_or(declaration);
    assert!(
        attributes.ends_with("#[derive(Deserialize)]\n#[serde(deny_unknown_fields)]\n"),
        "{name} is no longer a closed envelope that is only deserialised"
    );
    SOURCE[start + header.len()..]
        .lines()
        .take_while(|line| *line != "}")
        .filter_map(|line| {
            let declaration = line.strip_prefix("    ")?;
            let (field, _) = declaration
                .strip_prefix("pub(crate) ")
                .unwrap_or(declaration)
                .split_once(':')?;
            (!field.is_empty()
                && field
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'))
            .then(|| field.to_owned())
        })
        .collect()
}

#[test]
fn diagnostics_parity_envelope_field_names_equal_the_loaders() {
    for (name, restated) in [
        ("Registry", REGISTRY_FIELDS),
        ("TenantFiles", TENANT_FIELDS),
        ("OidcFile", OIDC_FILE_FIELDS),
        ("AuthenticationAuditFile", AUTHENTICATION_AUDIT_FIELDS),
    ] {
        assert_eq!(
            declared_fields(name),
            restated,
            "the envelope {name} changed: update agentsandbox::diagnostics::gateway_files and this corpus"
        );
    }
}

// ---- 2. registry ----------------------------------------------------------

/// A private directory with two distinct, well-formed tokens per tenant, and
/// the registry value that names them with relative references.
fn registry_fixture(tenants: &[&str]) -> (tempfile::TempDir, PathBuf, Value) {
    let directory = private_directory();
    let mut entries = Vec::new();
    for (index, id) in tenants.iter().enumerate() {
        let digit = char::from(b'a' + u8::try_from(index).expect("few tenants"));
        for (role, fill) in [("runner", 'r'), ("viewer", 'v')] {
            write_mode(
                &directory.path().join(format!("{id}-{role}")),
                format!("{fill}{}", digit.to_string().repeat(47)),
                0o600,
            );
        }
        entries.push(json!({
            "tenant_id": id,
            "data": format!("{id}.sqlite"),
            "runner_token_file": format!("{id}-runner"),
            "viewer_token_file": format!("{id}-viewer")
        }));
    }
    let path = directory.path().join("tenants.json");
    let registry = json!({"version": 1, "tenants": entries});
    (directory, path, registry)
}

/// The parity statement for one registry file and one event bound. Returns
/// whether the loaders accept it.
fn registry_parity(path: &Path, max_events: u64, case: &str) -> bool {
    let loaded = load(path).and_then(|tenants| {
        validate_policies(&tenants, max_events)?;
        Ok(tenants)
    });
    let inspection = inspect_registry(path, max_events);
    assert_eq!(
        loaded.is_ok(),
        inspection.refusals.is_empty(),
        "{case}: the loaders {} this registry, the inspection reports {:?}",
        if loaded.is_ok() { "accept" } else { "refuse" },
        inspection.refusals
    );
    if let Ok(tenants) = &loaded {
        assert!(inspection.read && inspection.decoded, "{case}");
        let inspected: Vec<_> = inspection
            .tenants
            .iter()
            .map(|tenant| {
                (
                    tenant.tenant_id.clone(),
                    tenant.data.clone(),
                    tenant.runner_token_file.clone(),
                    tenant.viewer_token_file.clone(),
                )
            })
            .collect();
        let configured: Vec<_> = tenants
            .iter()
            .map(|tenant| {
                (
                    tenant.tenant_id.clone(),
                    Some(tenant.data.clone()),
                    Some(tenant.runner_token_file.clone()),
                    Some(tenant.viewer_token_file.clone()),
                )
            })
            .collect();
        assert_eq!(
            inspected, configured,
            "{case}: tenants and resolved references"
        );
    }
    loaded.is_ok()
}

fn refusal_codes(refusals: &[Refusal]) -> Vec<FindingCode> {
    refusals.iter().map(|refusal| refusal.code).collect()
}

#[test]
fn diagnostics_parity_registry_file_is_read_as_the_loader_reads_it() {
    let (directory, path, registry) = registry_fixture(&["tenant-a"]);
    let unreadable = |path: &Path, case: &str| {
        assert!(!registry_parity(path, DEFAULT_MAX_EVENTS, case), "{case}");
        assert_eq!(
            refusal_codes(&inspect_registry(path, DEFAULT_MAX_EVENTS).refusals),
            [FindingCode::RegistryFileUnreadable],
            "{case}"
        );
    };

    unreadable(&path, "an absent registry");

    write_json(&path, &registry);
    assert!(registry_parity(
        &path,
        DEFAULT_MAX_EVENTS,
        "the baseline registry"
    ));
    // The loader checks no owner, mode or link count on the registry itself.
    write_mode(&path, serde_json::to_vec(&registry).unwrap(), 0o644);
    assert!(registry_parity(
        &path,
        DEFAULT_MAX_EVENTS,
        "a registry in mode 0644"
    ));

    let link = directory.path().join("registry-link.json");
    symlink(&path, &link).unwrap();
    unreadable(&link, "a symbolic link to a registry");
    unreadable(directory.path(), "a directory given as the registry");

    // Exactly the bound is accepted; one byte more is refused.
    let compact = serde_json::to_vec(&registry).unwrap();
    for (size, accepted) in [(1_048_576, true), (1_048_577, false)] {
        let mut padded = compact.clone();
        padded.resize(size, b' ');
        std::fs::write(&path, &padded).unwrap();
        assert_eq!(
            registry_parity(&path, DEFAULT_MAX_EVENTS, "a registry at its size bound"),
            accepted,
            "{size} bytes"
        );
    }

    let mut unknown_top = registry.clone();
    unknown_top["unknown"] = json!(true);
    let mut unknown_tenant = registry.clone();
    unknown_tenant["tenants"][0]["runner_token"] = json!("inline-value-not-permitted");
    let mut wrong_type = registry.clone();
    wrong_type["version"] = json!("1");
    for (case, bytes) in [
        ("invalid JSON", b"{ not json".to_vec()),
        (
            "an unknown top-level field",
            serde_json::to_vec(&unknown_top).unwrap(),
        ),
        (
            "an unknown tenant field",
            serde_json::to_vec(&unknown_tenant).unwrap(),
        ),
        ("a wrong type", serde_json::to_vec(&wrong_type).unwrap()),
    ] {
        std::fs::write(&path, bytes).unwrap();
        assert!(!registry_parity(&path, DEFAULT_MAX_EVENTS, case), "{case}");
        let refusals = inspect_registry(&path, DEFAULT_MAX_EVENTS).refusals;
        assert!(
            matches!(
                refusals.as_slice(),
                [Refusal {
                    code: FindingCode::RegistryJsonInvalid,
                    tenant_id: None,
                    details: Some(Details::JsonError { .. }),
                    variant: None
                }]
            ),
            "{case}: {refusals:?}"
        );
    }
}

#[test]
fn diagnostics_parity_registry_structural_rules() {
    let (directory, path, registry) = registry_fixture(&["tenant-a", "tenant-b"]);
    let check = |value: &Value, case: &str| {
        write_json(&path, value);
        let accepted = registry_parity(&path, DEFAULT_MAX_EVENTS, case);
        (
            accepted,
            refusal_codes(&inspect_registry(&path, DEFAULT_MAX_EVENTS).refusals),
        )
    };
    assert_eq!(check(&registry, "two tenants"), (true, vec![]));

    let mut version = registry.clone();
    version["version"] = json!(2);
    assert_eq!(
        check(&version, "version 2"),
        (false, vec![FindingCode::RegistryVersionUnknown])
    );
    assert_eq!(
        check(&json!({"version": 1, "tenants": []}), "an empty list"),
        (false, vec![FindingCode::RegistryEmpty])
    );

    let mut duplicate = registry.clone();
    duplicate["tenants"][1]["tenant_id"] = json!("tenant-a");
    assert_eq!(
        check(&duplicate, "a duplicate identifier"),
        (false, vec![FindingCode::RegistryTenantIdDuplicate])
    );
    let mut invalid = registry.clone();
    invalid["tenants"][1]["tenant_id"] = json!("tenant b");
    assert_eq!(
        check(&invalid, "an invalid identifier"),
        (false, vec![FindingCode::RegistryTenantIdInvalid])
    );

    for field in ["data", "runner_token_file", "viewer_token_file"] {
        let mut empty = registry.clone();
        empty["tenants"][1][field] = json!("");
        assert_eq!(
            check(&empty, field),
            (false, vec![FindingCode::RegistryReferenceEmpty]),
            "an empty {field}"
        );
    }

    // Absolute references are used as written, relative ones are resolved
    // against the canonical parent of the registry.
    let mut absolute = registry.clone();
    for field in ["data", "runner_token_file", "viewer_token_file"] {
        let relative = absolute["tenants"][0][field].as_str().unwrap().to_owned();
        absolute["tenants"][0][field] = json!(directory.path().join(relative));
    }
    assert_eq!(check(&absolute, "absolute references"), (true, vec![]));

    // A registry reached through a symbolic link in its parent: the loader
    // follows it and resolves against the real directory.
    write_json(&path, &registry);
    let outside = private_directory();
    let alias = outside.path().join("alias");
    symlink(directory.path(), &alias).unwrap();
    let through_link = alias.join("tenants.json");
    assert!(registry_parity(
        &through_link,
        DEFAULT_MAX_EVENTS,
        "a parent reached through a symbolic link"
    ));
    assert_eq!(
        inspect_registry(&through_link, DEFAULT_MAX_EVENTS).tenants[0].data,
        Some(
            directory
                .path()
                .canonicalize()
                .unwrap()
                .join("tenant-a.sqlite")
        )
    );
}

fn audit_capacity_policy() -> Value {
    let capacity = |events: u64, bytes: u64| json!({"events": events, "bytes": bytes});
    json!({
        "version": 1,
        "human_tenant": capacity(50, 128 * 1024 * 1024),
        "human_principal": capacity(12, 32 * 1024 * 1024),
        "control_reserve": capacity(25, 64 * 1024 * 1024),
        "identity_reserve": capacity(12, 32 * 1024 * 1024)
    })
}

fn workload_audit_policy(tenant_events: u64) -> Value {
    json!({
        "version": 1,
        "tenant": {"events": tenant_events, "bytes": 67_108_864},
        "principal": {"events": 4, "bytes": 16_777_216}
    })
}

#[test]
fn diagnostics_parity_registry_policies() {
    let (_directory, path, registry) = registry_fixture(&["tenant-a"]);
    let fingerprint = "a".repeat(64);
    let image = format!("registry.example.invalid/worker@sha256:{}", "b".repeat(64));
    let held_create = |approvals: Value| json!({"version": 1, "decision_ttl_seconds": 30, "approvals": approvals});
    let held_approval = json!({
        "authority_fingerprint": fingerprint,
        "image": format!("worker-a@sha256:{}", "d".repeat(64)),
        "profile": "kubernetes_held_pod_v1"
    });
    let reservation = |version: u32| {
        json!({
            "version": version,
            "decision_ttl_seconds": 30,
            "approvals": [{
                "authority_id": "11111111-1111-4111-8111-111111111111",
                "authority_fingerprint": fingerprint,
                "image": image,
                "client": "codex",
                "auth_mode": "api",
                "transport": "batch",
                "profile_id": "codex_api",
                "profile_revision": 1
            }]
        })
    };
    let recovery = |ttl: u32, broker: &str| json!({"version": 2, "decision_ttl_seconds": ttl, "allowed_broker_ids": [broker]});
    let broker = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";

    // One accepted and one refused value per policy, at the default bound.
    let cases: Vec<(&str, Value, Option<PolicyKind>)> = vec![
        (
            "worker_quota",
            json!({"version": 1, "max_active_workers": 100, "max_cpu_millis": 6000, "max_memory_bytes": 3_221_225_472_u64, "max_timeout_seconds": 3600}),
            None,
        ),
        (
            "worker_quota",
            json!({"version": 1, "max_active_workers": 101, "max_cpu_millis": 6000, "max_memory_bytes": 3_221_225_472_u64, "max_timeout_seconds": 3600}),
            Some(PolicyKind::WorkerQuota),
        ),
        (
            "held_create_policy",
            held_create(json!([held_approval])),
            None,
        ),
        (
            "held_create_policy",
            held_create(json!([])),
            Some(PolicyKind::KubernetesHeldCreate),
        ),
        ("session_reservation_policy", reservation(2), None),
        (
            "session_reservation_policy",
            reservation(1),
            Some(PolicyKind::KubernetesSessionReservation),
        ),
        ("session_recovery_policy", recovery(30, broker), None),
        (
            "session_recovery_policy",
            recovery(30, "not-a-broker-identifier"),
            Some(PolicyKind::KubernetesSessionRecovery),
        ),
        (
            "session_control_recovery_policy",
            recovery(30, broker),
            None,
        ),
        (
            "session_control_recovery_policy",
            recovery(0, broker),
            Some(PolicyKind::KubernetesSessionControlRecovery),
        ),
        ("audit_capacity_policy", audit_capacity_policy(), None),
        (
            "audit_capacity_policy",
            {
                let mut policy = audit_capacity_policy();
                policy["control_reserve"]["events"] = json!(0);
                policy
            },
            Some(PolicyKind::AuditCapacity),
        ),
    ];
    for (field, value, refused) in cases {
        let mut edited = registry.clone();
        edited["tenants"][0][field] = value;
        write_json(&path, &edited);
        let case = format!("{field}, expected refusal {refused:?}");
        assert_eq!(
            registry_parity(&path, DEFAULT_MAX_EVENTS, &case),
            refused.is_none(),
            "{case}"
        );
        let expected: Vec<Refusal> = refused
            .into_iter()
            .map(|policy| Refusal {
                code: FindingCode::RegistryPolicyInvalid,
                tenant_id: Some("tenant-a".to_owned()),
                details: Some(Details::Policy { policy }),
                variant: None,
            })
            .collect();
        assert_eq!(
            inspect_registry(&path, DEFAULT_MAX_EVENTS).refusals,
            expected,
            "{case}"
        );
    }

    // A policy whose verdict depends on the gateway event bound: it needs
    // 87 events. The bound itself is refused outside its range.
    let mut bounded = registry.clone();
    bounded["tenants"][0]["audit_capacity_policy"] = audit_capacity_policy();
    write_json(&path, &bounded);
    for (max_events, accepted) in [
        (0, false),
        (1, false),
        (64, false),
        (86, false),
        (87, true),
        (100_000, true),
        (1_000_000, true),
        (1_000_001, false),
    ] {
        assert_eq!(
            registry_parity(&path, max_events, "an audit capacity policy"),
            accepted,
            "event bound {max_events}"
        );
        assert!(inspect_registry(&path, max_events).uses_max_events);
    }

    // The workload policy is judged against the resolved defaults, or against
    // the explicit capacity policy of the same tenant.
    for (capacity, tenant_events, max_events, accepted) in [
        (None, 16, 64, true),
        (None, 41, 64, false),
        (Some(audit_capacity_policy()), 16, 100_000, true),
        (Some(audit_capacity_policy()), 99_990, 100_000, false),
    ] {
        let mut edited = registry.clone();
        edited["tenants"][0]["workload_audit_policy"] = workload_audit_policy(tenant_events);
        if let Some(capacity) = &capacity {
            edited["tenants"][0]["audit_capacity_policy"] = capacity.clone();
        }
        write_json(&path, &edited);
        let case = format!(
            "a workload policy of {tenant_events} events at bound {max_events}, explicit capacity {}",
            capacity.is_some()
        );
        assert_eq!(
            registry_parity(&path, max_events, &case),
            accepted,
            "{case}"
        );
        if !accepted {
            assert!(
                inspect_registry(&path, max_events)
                    .refusals
                    .iter()
                    .any(|refusal| refusal.details
                        == Some(Details::Policy {
                            policy: PolicyKind::WorkloadAudit
                        })),
                "{case}"
            );
        }
    }
}

#[test]
fn diagnostics_parity_token_content_is_outside_the_restated_half() {
    let (directory, path, registry) = registry_fixture(&["tenant-a"]);
    write_json(&path, &registry);
    assert!(registry_parity(&path, DEFAULT_MAX_EVENTS, "valid tokens"));

    // The same token for both identities, then a malformed token: the loader
    // refuses each; the inspection opens no token and reports nothing.
    let runner = std::fs::read(directory.path().join("tenant-a-runner")).unwrap();
    for (case, viewer) in [
        ("the runner token given to the viewer", runner),
        ("a token that is too short", b"short".to_vec()),
    ] {
        write_mode(&directory.path().join("tenant-a-viewer"), viewer, 0o600);
        assert!(load(&path).is_err(), "{case}: the loader reads the token");
        let inspection = inspect_registry(&path, DEFAULT_MAX_EVENTS);
        assert!(
            inspection.refusals.is_empty() && inspection.decoded,
            "{case}: token content is not restated: {:?}",
            inspection.refusals
        );
    }
}

// ---- 3. OIDC file ---------------------------------------------------------

/// The parity statement for one OIDC file. Returns the inspection, and
/// whether the loader accepts the file.
fn oidc_parity(path: &Path, case: &str) -> (OidcInspection, bool) {
    let prepared = prepare_oidc_file(path);
    let inspection = inspect_oidc_file(path);
    assert_eq!(
        prepared.is_ok(),
        inspection.refusals.is_empty(),
        "{case}: the loader {} this file, the inspection reports {:?}",
        if prepared.is_ok() {
            "accepts"
        } else {
            "refuses"
        },
        inspection.refusals
    );
    if let Ok(prepared) = &prepared {
        let audit = &prepared.file.authentication_audit;
        assert_eq!(inspection.version, Some(prepared.file.version), "{case}");
        assert_eq!(
            inspection.plan_tenants,
            audit.capacity_plan.as_ref().map(|plan| plan
                .tenants
                .iter()
                .map(|tenant| tenant.tenant_id.clone())
                .collect::<Vec<_>>()),
            "{case}"
        );
        assert_eq!(
            inspection.audit_path.as_deref(),
            Some(prepared.audit_path.as_path()),
            "{case}"
        );
        // The loader keeps the bytes of the bundle, not its resolved path:
        // the path the inspection resolved must hold exactly those bytes.
        assert_eq!(
            inspection
                .ca_bundle_file
                .as_ref()
                .map(|path| std::fs::read_to_string(path).unwrap()),
            prepared.ca_bundle_pem,
            "{case}"
        );
        assert_eq!(
            inspection.ca_bundle_declared,
            prepared.file.ca_bundle_file.is_some(),
            "{case}"
        );
    }
    let accepted = prepared.is_ok();
    (inspection, accepted)
}

/// What the file loader says of a file it refuses.
fn loader_refusal(path: &Path) -> String {
    prepare_oidc_file(path)
        .err()
        .map(|error| format!("{error:#}"))
        .expect("the loader refuses this file")
}

/// The documented HTTPS file (version 2, with a plan) in a private directory.
fn https_fixture() -> (tempfile::TempDir, PathBuf, Value) {
    let directory = private_directory();
    let path = directory.path().join("oidc.json");
    (directory, path, serde_json::from_str(HTTPS_FILE).unwrap())
}

/// One edit of an OIDC file that the gateway block validator refuses, with
/// the closed reason the report gives for it.
type GatewayBlockEdit<'a> = (&'a str, Reason, &'a dyn Fn(&mut Value));

/// A version 1 file for the demonstration HTTP transport, from the gateway
/// block and the policy of the documented demonstration manifest.
fn demo_file() -> Value {
    let manifest: Value = serde_json::from_str(DEMO_MANIFEST).unwrap();
    json!({
        "version": 1,
        "gateway": manifest["gateway"],
        "authentication_audit": {
            "path": "authentication.sqlite",
            "installation_id": "22222222-2222-4222-8222-222222222222",
            "policy": manifest["authentication_policy"]
        },
        "ca_bundle_file": null
    })
}

#[tokio::test]
async fn diagnostics_parity_oidc_file_envelope_and_validators() {
    let (_directory, path, file) = https_fixture();
    let check = |value: &Value, case: &str| {
        write_json(&path, value);
        let (inspection, accepted) = oidc_parity(&path, case);
        (accepted, refusal_codes(&inspection.refusals))
    };
    assert_eq!(check(&file, "the documented HTTPS file"), (true, vec![]));
    assert_eq!(
        check(&demo_file(), "a version 1 demonstration file"),
        (true, vec![])
    );

    // The private-file rule of the loader.
    write_mode(&path, serde_json::to_vec(&file).unwrap(), 0o644);
    let (inspection, accepted) = oidc_parity(&path, "mode 0644");
    assert!(!accepted && !inspection.read);
    assert_eq!(
        refusal_codes(&inspection.refusals),
        [FindingCode::OidcFileUnavailableOrNotPrivate]
    );
    std::fs::remove_file(&path).unwrap();
    let (inspection, accepted) = oidc_parity(&path, "an absent file");
    assert!(!accepted);
    assert_eq!(
        refusal_codes(&inspection.refusals),
        [FindingCode::OidcFileAbsent]
    );

    // Exactly the bound of the file is accepted; one byte more is refused by
    // the reader, on both sides.
    let compact = serde_json::to_vec(&file).unwrap();
    for (size, expected) in [(1_048_576, true), (1_048_577, false)] {
        let mut padded = compact.clone();
        padded.resize(size, b' ');
        write_mode(&path, padded, 0o600);
        let (inspection, accepted) = oidc_parity(&path, "an OIDC file at its size bound");
        assert_eq!(accepted, expected, "{size} bytes");
        if !expected {
            assert!(!inspection.read, "{size} bytes");
            assert_eq!(
                refusal_codes(&inspection.refusals),
                [FindingCode::OidcFileUnavailableOrNotPrivate],
                "{size} bytes"
            );
            assert!(
                loader_refusal(&path).contains(SIZE_REFUSAL),
                "{size} bytes: the loader refuses the file for its size"
            );
        }
    }

    let mut unknown_top = file.clone();
    unknown_top["unknown"] = json!(true);
    let mut unknown_audit = file.clone();
    unknown_audit["authentication_audit"]["unknown"] = json!(true);
    for (case, bytes) in [
        ("invalid JSON", b"{ not json".to_vec()),
        (
            "an unknown top-level field",
            serde_json::to_vec(&unknown_top).unwrap(),
        ),
        (
            "an unknown field in the audit block",
            serde_json::to_vec(&unknown_audit).unwrap(),
        ),
    ] {
        write_mode(&path, bytes, 0o600);
        let (inspection, accepted) = oidc_parity(&path, case);
        assert!(!accepted && inspection.read, "{case}");
        assert!(
            matches!(
                inspection.refusals.as_slice(),
                [Refusal {
                    code: FindingCode::OidcJsonInvalid,
                    details: Some(Details::JsonError { .. }),
                    ..
                }]
            ),
            "{case}: {:?}",
            inspection.refusals
        );
    }

    // Version and plan go together: (1, none) or (2, some).
    let mut version_one_with_plan = file.clone();
    version_one_with_plan["version"] = json!(1);
    let mut version_two_without_plan = file.clone();
    version_two_without_plan["authentication_audit"]
        .as_object_mut()
        .unwrap()
        .remove("capacity_plan");
    let mut version_three = file.clone();
    version_three["version"] = json!(3);
    for (case, value) in [
        ("version 1 with a plan", &version_one_with_plan),
        ("version 2 without a plan", &version_two_without_plan),
        ("version 3", &version_three),
    ] {
        assert_eq!(
            check(value, case),
            (false, vec![FindingCode::OidcVersionPlanPairInvalid]),
            "{case}"
        );
    }

    let mut duplicate_plan_tenant = file.clone();
    duplicate_plan_tenant["authentication_audit"]["capacity_plan"]["tenants"][1]["tenant_id"] =
        json!("tenant-a");
    assert_eq!(
        check(&duplicate_plan_tenant, "a plan refused by its validator"),
        (false, vec![FindingCode::OidcCapacityPlanInvalid])
    );

    // Each refusal of the gateway block, with the closed reason the report
    // gives for it. A changed literal in the validator fails here.
    let edits: [GatewayBlockEdit; 6] = [
        ("the block version", Reason::VersionOrTokenBound, &|file| {
            file["gateway"]["version"] = json!(2);
        }),
        (
            "an unparseable web origin",
            Reason::UrlUnparseable,
            &|file| {
                file["gateway"]["web_origin"] = json!("not a URL");
            },
        ),
        (
            "a web origin with a path",
            Reason::WebOriginOrCallback,
            &|file| {
                file["gateway"]["web_origin"] = json!("https://guard.example.invalid/");
            },
        ),
        (
            "an empty provider label",
            Reason::ClaimMappingInvalid,
            &|file| {
                file["gateway"]["provider_label"] = json!("");
            },
        ),
        (
            "two identical claim rules",
            Reason::ClaimMappingAmbiguous,
            &|file| {
                let rule = file["gateway"]["mapping"]["rules"][0].clone();
                file["gateway"]["mapping"]["rules"]
                    .as_array_mut()
                    .unwrap()
                    .push(rule);
            },
        ),
        (
            "one client for web and CLI",
            Reason::ProviderBlockInvalid,
            &|file| {
                let web = file["gateway"]["provider"]["web_client_id"].clone();
                file["gateway"]["provider"]["cli_client_id"] = web;
            },
        ),
    ];
    for (case, reason, edit) in edits {
        let mut edited = file.clone();
        edit(&mut edited);
        write_json(&path, &edited);
        let (inspection, accepted) = oidc_parity(&path, case);
        assert!(!accepted, "{case}");
        assert_eq!(
            inspection.refusals,
            [Refusal {
                code: FindingCode::OidcGatewayBlockInvalid,
                tenant_id: None,
                details: Some(Details::Reason { reason }),
                variant: None
            }],
            "{case}"
        );
        assert!(
            inspection.gateway.is_some(),
            "{case}: the block still decodes"
        );
    }
}

#[tokio::test]
async fn diagnostics_parity_oidc_references_and_ca_bundle() {
    let (directory, path, file) = https_fixture();
    let ca = directory.path().join("ca.pem");
    let with = |edit: &dyn Fn(&mut Value)| {
        let mut edited = file.clone();
        edit(&mut edited);
        edited
    };
    let check = |value: &Value, case: &str| {
        write_json(&path, value);
        let (inspection, accepted) = oidc_parity(&path, case);
        (
            accepted,
            refusal_codes(&inspection.refusals),
            inspection.ca_bundle,
        )
    };
    let with_ca = with(&|file| file["ca_bundle_file"] = json!("ca.pem"));

    // A relative and an absolute reference to an accepted bundle.
    write_mode(&ca, PUBLIC_TEST_ROOT, 0o600);
    assert_eq!(
        check(&with_ca, "a relative CA reference"),
        (
            true,
            vec![],
            CaBundle::Accepted {
                certificates: 1,
                with_client: true
            }
        )
    );
    let absolute = with(&|file| {
        file["ca_bundle_file"] = json!(ca);
        file["authentication_audit"]["path"] = json!(directory.path().join("audit.sqlite"));
    });
    assert!(check(&absolute, "absolute references").0);

    // References that are empty or leave their directory.
    for (case, value) in [
        (
            "an empty CA reference",
            with(&|file| file["ca_bundle_file"] = json!("")),
        ),
        (
            "a CA reference with parent traversal",
            with(&|file| file["ca_bundle_file"] = json!("../ca.pem")),
        ),
        (
            "an empty audit path",
            with(&|file| file["authentication_audit"]["path"] = json!("")),
        ),
        (
            "an audit path with parent traversal",
            with(&|file| file["authentication_audit"]["path"] = json!("../audit.sqlite")),
        ),
    ] {
        let (accepted, codes, _) = check(&value, case);
        assert_eq!(
            (accepted, codes),
            (false, vec![FindingCode::OidcReferenceInvalid]),
            "{case}"
        );
    }

    // The bundle itself: private file, UTF-8, the bundle grammar.
    let refused: [(&str, Vec<u8>, u32, FindingCode, CaBundle); 5] = [
        (
            "a bundle in mode 0644",
            PUBLIC_TEST_ROOT.as_bytes().to_vec(),
            0o644,
            FindingCode::TrustCaUnavailableOrNotPrivate,
            CaBundle::Unavailable,
        ),
        (
            "a bundle that is not UTF-8",
            vec![0xff, 0xfe, 0xfd],
            0o600,
            FindingCode::TrustCaBundleInvalid,
            CaBundle::Refused,
        ),
        (
            "an empty bundle",
            Vec::new(),
            0o600,
            FindingCode::TrustCaBundleInvalid,
            CaBundle::Refused,
        ),
        (
            "a key section in the bundle",
            b"-----BEGIN PRIVATE KEY-----\nAA==\n-----END PRIVATE KEY-----\n".to_vec(),
            0o600,
            FindingCode::TrustCaBundleInvalid,
            CaBundle::Refused,
        ),
        (
            "nine certificates",
            PUBLIC_TEST_ROOT.repeat(9).into_bytes(),
            0o600,
            FindingCode::TrustCaBundleInvalid,
            CaBundle::Refused,
        ),
    ];
    for (case, bytes, mode, code, outcome) in refused {
        write_mode(&ca, bytes, mode);
        assert_eq!(
            check(&with_ca, case),
            (false, vec![code], outcome),
            "{case}"
        );
    }
    // The bound of the bundle file. A bundle of exactly that size is opened,
    // and refused by the bundle grammar; one byte more is refused by the
    // reader, on both sides.
    for (size, code, outcome, by_size) in [
        (
            262_144,
            FindingCode::TrustCaBundleInvalid,
            CaBundle::Refused,
            false,
        ),
        (
            262_145,
            FindingCode::TrustCaUnavailableOrNotPrivate,
            CaBundle::Unavailable,
            true,
        ),
    ] {
        let mut padded = PUBLIC_TEST_ROOT.as_bytes().to_vec();
        padded.resize(size, b'\n');
        write_mode(&ca, padded, 0o600);
        assert_eq!(
            check(&with_ca, "a bundle at the bound of its file"),
            (false, vec![code], outcome),
            "{size} bytes"
        );
        let refusal = loader_refusal(&path);
        assert_eq!(
            refusal.contains(SIZE_REFUSAL),
            by_size,
            "{size} bytes: the loader says: {refusal}"
        );
    }

    std::fs::remove_file(&ca).unwrap();
    assert_eq!(
        check(&with_ca, "an absent bundle"),
        (
            false,
            vec![FindingCode::TrustCaUnavailableOrNotPrivate],
            CaBundle::Unavailable
        )
    );

    // A valid bundle with the demonstration HTTP transport: each is accepted
    // alone, the client refuses them together.
    write_mode(&ca, PUBLIC_TEST_ROOT, 0o600);
    let mut demo_with_ca = demo_file();
    demo_with_ca["ca_bundle_file"] = json!("ca.pem");
    assert_eq!(
        check(
            &demo_with_ca,
            "a CA bundle with the demonstration transport"
        ),
        (
            false,
            vec![FindingCode::TrustCaIncompatibleWithTransport],
            CaBundle::Refused
        )
    );

    // A caller that withholds the bundle: the file is not opened. The caller
    // is given the authentication audit that the same file names.
    write_json(&path, &with_ca);
    let asked_with_the_audit = std::cell::Cell::new(false);
    let withheld =
        agentsandbox::diagnostics::gateway_files::inspect_oidc_file_guarded(&path, &|_, audit| {
            asked_with_the_audit.set(audit.is_some());
            false
        });
    assert!(asked_with_the_audit.get());
    assert_eq!(
        (refusal_codes(&withheld.refusals), withheld.ca_bundle),
        (
            vec![FindingCode::TrustCaReferenceIsACredentialReference],
            CaBundle::Withheld
        )
    );
}

#[tokio::test]
async fn diagnostics_parity_authentication_policy_is_checked_after_the_file_loader() {
    // `prepare_oidc_file` does not validate the authentication audit policy:
    // the gateway does when it opens or creates the audit. The inspection
    // reports that refusal apart, so that statement 3 above stays exact. The
    // gateway side of that verdict is `load_oidc` itself, asked here to create
    // the audit that each file names in its own private directory.
    let gateway_loads = |path: &Path| load_oidc(path, true, Redactor::new(Vec::new())).is_ok();
    let audit_of = |directory: &tempfile::TempDir| directory.path().join("authentication.sqlite");

    // The control: the documented file, unedited.
    let (directory, path, file) = https_fixture();
    write_json(&path, &file);
    let (inspection, accepted) = oidc_parity(&path, "the documented HTTPS file");
    assert!(accepted && inspection.extra_refusals.is_empty());
    assert!(
        gateway_loads(&path),
        "positive control: the gateway creates the audit of the documented file"
    );
    assert!(audit_of(&directory).is_file());

    let (directory, path, mut file) = https_fixture();
    file["authentication_audit"]["policy"]["max_records"] = json!(0);
    write_json(&path, &file);
    let (inspection, accepted) = oidc_parity(&path, "a policy without any record budget");
    assert!(accepted, "the file loader accepts this file");
    assert_eq!(
        refusal_codes(&inspection.extra_refusals),
        [FindingCode::OidcAuthenticationPolicyInvalid]
    );
    assert!(!gateway_loads(&path), "the gateway refuses this policy");
    assert!(
        !audit_of(&directory).exists(),
        "the policy is checked before the audit is created"
    );

    // A plan that the policy cannot fund is reported in the same place. The
    // gateway refuses it as well, but later: it creates the audit first, and
    // the refused initialisation leaves that new file behind.
    let (directory, path, mut file) = https_fixture();
    file["authentication_audit"]["policy"]["max_records"] = json!(1000);
    write_json(&path, &file);
    let (inspection, accepted) = oidc_parity(&path, "a plan larger than its policy");
    assert!(accepted, "the file loader accepts this file");
    assert_eq!(
        refusal_codes(&inspection.extra_refusals),
        [FindingCode::OidcAuthenticationPolicyInvalid]
    );
    assert!(
        !gateway_loads(&path),
        "the gateway refuses a plan that its policy cannot fund"
    );
    assert!(
        audit_of(&directory).exists(),
        "as observed: the funding of the plan is checked once the audit file is created"
    );
}

// ---- composition regression: relay retention is part of the real registry ---

#[test]
fn diagnostics_parity_relay_retention_policy() {
    let (_directory, path, registry) = registry_fixture(&["tenant-a"]);
    let valid =
        serde_json::to_value(agentsandbox::relay::RelayRetentionPolicyV1::default()).unwrap();
    let mut oversized_sender = valid.clone();
    oversized_sender["sender"]["messages"] = json!(513);
    let mut unknown_field = valid.clone();
    unknown_field["burst"] = json!(1);
    for (case, policy, accepted, expected_code) in [
        ("null relay policy", Value::Null, true, None),
        ("valid relay policy", valid, true, None),
        (
            "sender share 513",
            oversized_sender,
            false,
            Some(FindingCode::RegistryPolicyInvalid),
        ),
        (
            "unknown relay field burst",
            unknown_field,
            false,
            Some(FindingCode::RegistryJsonInvalid),
        ),
    ] {
        let mut edited = registry.clone();
        edited["tenants"][0]["relay_retention_policy"] = policy;
        write_json(&path, &edited);
        assert_eq!(
            registry_parity(&path, DEFAULT_MAX_EVENTS, case),
            accepted,
            "{case}"
        );
        let inspection = inspect_registry(&path, DEFAULT_MAX_EVENTS);
        assert!(
            !inspection.uses_max_events,
            "relay retention is independent of the event bound"
        );
        assert_eq!(
            refusal_codes(&inspection.refusals),
            expected_code.into_iter().collect::<Vec<_>>(),
            "{case}"
        );
        if expected_code == Some(FindingCode::RegistryPolicyInvalid) {
            assert_eq!(
                inspection.refusals[0].tenant_id.as_deref(),
                Some("tenant-a")
            );
            assert_eq!(
                serde_json::to_value(inspection.refusals[0].details.as_ref().unwrap()).unwrap(),
                json!({"kind": "policy", "policy": "relay_retention"})
            );
        }
    }
}
