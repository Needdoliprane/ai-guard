//! The real `ai-guard-diagnose` binary on synthetic installations.
//!
//! Installations are built by product code (`init`, `setup prepare`) or by the
//! documented file formats. Where a finding predicts what a product command
//! does, the test runs that command: a refusal oracle always has a positive
//! control built by the same code and differing by one stated edit. Every run
//! through `Scenario::diagnose` is also a no-mutation check of the scenario.
//!
//! No engine, provider, model or identity provider is involved, and every
//! credential value is synthetic. These tests qualify no runtime.
// The repository's controlled issuer, served on the loopback of the test
// machine. Other OIDC targets use its token and fault controls; this target
// only lets the tool ask it for its discovery document and its keys. The
// file is shared: it is kept out of the formatting of this target.
#[allow(dead_code)]
#[rustfmt::skip]
#[path = "support/oidc_fixture.rs"]
mod controlled;
mod diagnose_support;
use agentsandbox as test_product;

use agentsandbox::{
    diagnostics::{
        DEFAULT_MAX_EVENTS, DiagnoseError,
        catalogue::{CATALOGUE, Program},
        report::{Basis, ReportV1, Status},
    },
    provider::{AuthMode, AuthPreference, ClientKind, ProviderResolutionV1, ProviderSelection},
    redaction::Redactor,
};
use diagnose_support::{
    child::GatewayStart,
    fixtures::{self, Run, Scenario},
    oracle_not_run,
    recording_issuer::RecordingIssuer,
    tree_manifest,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
};

const NO_ARGS: [&str; 0] = [];
const CANARY: &str = "CANARY-7f3a91-must-never-be-printed";

// The guides and their examples are compiled in: a file that moves fails the
// build of this target, and a text that drifts from the binary fails a test.
const GUIDES: [(&str, &str); 2] = [
    (
        "README.md",
        include_str!("../docs/setup/diagnostics/README.md"),
    ),
    (
        "README.fr.md",
        include_str!("../docs/setup/diagnostics/README.fr.md"),
    ),
];
/// The public profile file of the guides: two clients, two modes each.
const DOCUMENTED_PROFILES: &str =
    include_str!("../docs/setup/diagnostics/examples/providers.two-modes.json");
/// The reports of the guides, as the files hold them.
const DOCUMENTED_REPORTS: [(&str, &str); 9] = [
    (
        "report.first-run.txt",
        include_str!("../docs/setup/diagnostics/examples/report.first-run.txt"),
    ),
    (
        "report.local-fresh.txt",
        include_str!("../docs/setup/diagnostics/examples/report.local-fresh.txt"),
    ),
    (
        "report.local-fresh.json",
        include_str!("../docs/setup/diagnostics/examples/report.local-fresh.json"),
    ),
    (
        "report.prepared-v2.txt",
        include_str!("../docs/setup/diagnostics/examples/report.prepared-v2.txt"),
    ),
    (
        "report.prepared-v2.json",
        include_str!("../docs/setup/diagnostics/examples/report.prepared-v2.json"),
    ),
    (
        "report.providers-two-modes.txt",
        include_str!("../docs/setup/diagnostics/examples/report.providers-two-modes.txt"),
    ),
    (
        "report.providers-two-modes.json",
        include_str!("../docs/setup/diagnostics/examples/report.providers-two-modes.json"),
    ),
    (
        "report.providers-explicit-subscription.txt",
        include_str!(
            "../docs/setup/diagnostics/examples/report.providers-explicit-subscription.txt"
        ),
    ),
    (
        "report.providers-explicit-subscription.json",
        include_str!(
            "../docs/setup/diagnostics/examples/report.providers-explicit-subscription.json"
        ),
    ),
];
/// What stands for the package version in a documented report.
const VERSION_PLACEHOLDER: &str = "PACKAGE_VERSION";

/// A documented report is the real output, package version aside.
fn assert_documented(name: &str, actual: &str) {
    let (_, documented) = DOCUMENTED_REPORTS
        .iter()
        .find(|(file, _)| *file == name)
        .unwrap_or_else(|| panic!("no documented report is named {name}"));
    let documented = documented.replace(VERSION_PLACEHOLDER, env!("CARGO_PKG_VERSION"));
    if documented == actual {
        return;
    }
    let line = documented
        .lines()
        .zip(actual.lines())
        .position(|(documented, actual)| documented != actual)
        .unwrap_or_else(|| documented.lines().count().min(actual.lines().count()));
    panic!(
        "the documented example {name} is not the real output; first difference at line {}:\n documented: {:?}\n actual:     {:?}",
        line + 1,
        documented.lines().nth(line),
        actual.lines().nth(line)
    );
}

fn arg(path: &Path) -> &str {
    path.to_str().expect("test paths are UTF-8")
}

/// Findings, then the limits of the build.
fn findings(report: &Value) -> Vec<&Value> {
    ["findings", "limits"]
        .into_iter()
        .flat_map(|section| {
            report[section]
                .as_array()
                .unwrap_or_else(|| panic!("the report has no {section} array"))
        })
        .collect()
}

fn codes(report: &Value) -> Vec<&str> {
    findings(report)
        .into_iter()
        .map(|finding| finding["code"].as_str().unwrap())
        .collect()
}

fn find<'a>(report: &'a Value, code: &str) -> Option<&'a Value> {
    findings(report)
        .into_iter()
        .find(|finding| finding["code"] == code)
}

fn finding<'a>(report: &'a Value, code: &str) -> &'a Value {
    find(report, code).unwrap_or_else(|| {
        panic!(
            "the report has no finding {code}; it has {:?}",
            codes(report)
        )
    })
}

fn text_of<'a>(value: &'a Value, field: &str) -> &'a str {
    value[field]
        .as_str()
        .unwrap_or_else(|| panic!("{field} is not a string in {value}"))
}

fn actions(report: &Value) -> Vec<&str> {
    findings(report)
        .into_iter()
        .map(|finding| text_of(finding, "action"))
        .collect()
}

fn entries<'a>(report: &'a Value, section: &str) -> &'a Vec<Value> {
    report[section]
        .as_array()
        .unwrap_or_else(|| panic!("{section} is not a list: this part was not inspected"))
}

fn input<'a>(report: &'a Value, name: &str) -> &'a Value {
    entries(report, "inputs")
        .iter()
        .find(|input| input["name"] == name)
        .unwrap_or_else(|| panic!("the input {name} is not listed: {}", report["inputs"]))
}

/// The findings of the given axes, for byte comparisons between two runs.
fn findings_of_axes(report: &Value, axes: &[&str]) -> Vec<Value> {
    findings(report)
        .into_iter()
        .filter(|finding| axes.contains(&finding["axis"].as_str().unwrap()))
        .cloned()
        .collect()
}

fn assert_no_canary(run: &Run, canaries: &[&str], context: &str) {
    for canary in canaries {
        assert!(
            !run.stdout.contains(canary) && !run.stderr.contains(canary),
            "a canary reached the output ({context})"
        );
    }
}

/// A run under both oracles: the scenario is unchanged, and the entries whose
/// access time moved are known. `None` when this mount does not record access
/// times: the never-read oracle then fails the test, unless the run names it
/// as left out (`diagnose_support::oracle_not_run`).
struct ObservedRun {
    run: Run,
    accessed: Option<BTreeSet<PathBuf>>,
}

impl ObservedRun {
    /// Relative paths of the scenario root whose access time moved.
    fn accessed(&self) -> Option<&BTreeSet<PathBuf>> {
        self.accessed.as_ref()
    }
}

/// Whether a read and a listing move an armed access time on this mount.
fn mount_records_access_times(scenario: &Scenario) -> bool {
    let probe = scenario.path("access-time-probe");
    fixtures::private_dir(&probe);
    fixtures::private_write(&probe.join("file"), "probe");
    let paths = vec![PathBuf::from(""), PathBuf::from("file")];
    tree_manifest::arm_access_times(&probe, &paths);
    let armed = tree_manifest::access_times(&probe, &paths);
    fs::read(probe.join("file")).unwrap();
    fs::read_dir(&probe).unwrap().for_each(drop);
    let observed = tree_manifest::access_times(&probe, &paths);
    fs::remove_file(probe.join("file")).unwrap();
    fs::remove_dir(&probe).unwrap();
    paths.iter().all(|path| armed[path] != observed[path])
}

fn observed_run(scenario: &Scenario, args: &[&str]) -> ObservedRun {
    let recorded = mount_records_access_times(scenario);
    let paths = tree_manifest::paths(&scenario.root);
    let hashes = tree_manifest::hashes(&scenario.root, &paths);
    tree_manifest::arm_access_times(&scenario.root, &paths);
    let before = tree_manifest::metadata_only(&scenario.root, &paths, &hashes);
    let armed = tree_manifest::access_times(&scenario.root, &paths);
    let run = scenario.diagnose_unobserved(args);
    let observed = tree_manifest::access_times(&scenario.root, &paths);
    let after = tree_manifest::take(&scenario.root);
    tree_manifest::assert_unchanged(&before, &after, "access-time run");
    ObservedRun {
        run,
        accessed: recorded.then(|| {
            paths
                .into_iter()
                .filter(|path| armed[path] != observed[path])
                .collect()
        }),
    }
}

/// The never-read oracle. `calibration` is a file the tool must read: it
/// proves that a read by the tool is visible. `never` are path prefixes,
/// relative to the scenario root, that must not be read or listed.
fn assert_never_read(observed: &ObservedRun, calibration: &str, never: &[&str], context: &str) {
    let Some(accessed) = observed.accessed() else {
        oracle_not_run(
            "never-read",
            "this mount does not record access times; the byte-identical reports are the only evidence",
        );
        return;
    };
    assert!(
        accessed.contains(Path::new(calibration)),
        "calibration: the tool must read {calibration} ({context}); accessed: {accessed:?}"
    );
    let offending: Vec<_> = accessed
        .iter()
        .filter(|path| never.iter().any(|prefix| path.starts_with(prefix)))
        .collect();
    assert!(
        offending.is_empty(),
        "the tool read or listed entries it must never open ({context}): {offending:?}"
    );
}

/// The refusal oracle of the gateway: it exited before its listening line,
/// and its own text names the expected cause. A gateway that refuses for
/// another cause says nothing about the finding it is paired with.
fn assert_gateway_refuses(gateway: &GatewayStart, cause: &str, context: &str) {
    let Some(said) = gateway.refusal() else {
        panic!("the gateway listens ({context}); it was expected to refuse with {cause:?}");
    };
    // The record of what the gateway printed. It masks its own diagnostics,
    // and every credential of these fixtures is synthetic.
    eprintln!(
        "OBSERVED gateway refusal ({context}): {}",
        said.trim_end().replace('\n', " | ")
    );
    assert!(
        said.contains(cause),
        "the gateway refuses ({context}), but not with {cause:?}; it says: {said}"
    );
}

// ---- T01 ------------------------------------------------------------------

#[test]
fn first_run_states() {
    let scenario = Scenario::new();

    // No argument at all, in an empty working directory.
    let (text, json, report) = scenario.diagnose_both(NO_ARGS);
    assert_eq!(json.code, 3, "{}", json.stderr);
    assert!(text.stderr.is_empty() && json.stderr.is_empty());
    assert_eq!(report["schema"], "ai-guard-diagnose-report");
    assert_eq!(report["version"], 1);
    assert_eq!(
        report["mode"],
        json!({
            "network": "none", "mutation": "none", "credential_contents_read": "none",
            "files_read_for_content": [], "active_checks_executed": []
        })
    );
    assert_eq!(
        report["inputs"],
        json!([{"name": "config_dir", "origin": "default", "display": "<config>"}])
    );
    assert_eq!(
        report["layouts"],
        json!([{"input": "config_dir", "kind": "absent", "preparation": null, "oidc_file_version": null}])
    );
    let absent = finding(&report, "layout.target_absent");
    assert_eq!(absent["status"], "error");
    assert_eq!(absent["axis"], "layout");
    assert_eq!(absent["basis"], "observed");
    assert_eq!(
        absent["scope"],
        json!({"kind": "input", "input": "config_dir"})
    );
    assert!(text_of(absent, "message").contains("default directory .agentsandbox"));
    let action = text_of(absent, "action");
    assert!(
        action.contains("agentsandbox init --config-dir \"<config>\"")
            && action.contains("agentsandbox setup prepare --input MANIFEST --output-dir NEW_DIR"),
        "the action names both ways to create an installation: {action}"
    );
    assert_eq!(report["summary"]["status"], "error");
    assert_eq!(report["summary"]["exit_code"], 3);
    assert_eq!(report["summary"]["counts"]["error"], 1);
    assert_eq!(report["summary"]["counts"]["unknown"], 0);
    assert!(
        text.stdout
            .contains("inspected  config directory \"<config>\" (default; no target was given)\n")
    );
    assert!(text.stdout.contains("  [error] layout.target_absent\n"));
    assert!(text.stdout.contains("\nNOT AVAILABLE IN THIS BUILD\n"));
    assert!(text.stdout.ends_with("exit 3\n"));
    assert!(
        text.stdout
            .lines()
            .all(|line| !line.contains('\u{1b}') && line.chars().count() <= 100),
        "plain text within 100 columns"
    );
    assert_documented("report.first-run.txt", &text.stdout);

    // An explicit path that does not exist: same code, the message says so.
    let missing = scenario.path("missing");
    for (flag, name, placeholder) in [
        ("--config-dir", "config_dir", "<config>"),
        ("--installation-dir", "installation_dir", "<installation>"),
    ] {
        let (text, json, report) = scenario.diagnose_both([flag, arg(&missing)]);
        assert_eq!(json.code, 3);
        assert_eq!(
            report["inputs"],
            json!([{"name": name, "origin": "cli", "display": placeholder}])
        );
        let absent = finding(&report, "layout.target_absent");
        assert_eq!(absent["scope"], json!({"kind": "input", "input": name}));
        assert!(text_of(absent, "message").contains("given on the command line"));
        assert!(
            !text.stdout.contains(arg(&scenario.root))
                && !json.stdout.contains(arg(&scenario.root)),
            "absolute paths are shown only with --show-paths"
        );
    }
    let (text, _, report) = scenario.diagnose_both(["--config-dir", arg(&missing), "--show-paths"]);
    assert_eq!(report["inputs"][0]["display"], arg(&missing));
    assert!(text.stdout.contains(&format!("\"{}\"", arg(&missing))));
    assert!(
        text_of(finding(&report, "layout.target_absent"), "action")
            .contains(&format!("--config-dir \"{}\"", arg(&missing)))
    );

    // An empty private directory is not an installation.
    let empty = scenario.path("empty");
    fixtures::private_dir(&empty);
    let (text, json, report) = scenario.diagnose_both(["--config-dir", arg(&empty)]);
    assert_eq!(json.code, 3);
    assert_eq!(report["layouts"][0]["kind"], "unrecognized");
    let unrecognized = finding(&report, "layout.unrecognized");
    assert_eq!(unrecognized["status"], "error");
    assert!(find(&report, "layout.target_absent").is_none());
    assert!(text.stdout.contains("  [error] layout.unrecognized\n"));
    assert!(text.stdout.ends_with("exit 3\n"));
    assert!(fs::read_dir(&empty).unwrap().next().is_none());
}

// ---- T02 ------------------------------------------------------------------

#[test]
fn fresh_historical_directory_is_explained() {
    let scenario = Scenario::new();
    let config = scenario.historical("config");
    let (text, json, report) = scenario.diagnose_both(["--config-dir", arg(&config)]);

    assert_eq!(
        report["layouts"][0]["kind"], "historical_config_dir",
        "a directory created by init is recognised; layouts: {}",
        report["layouts"]
    );
    finding(&report, "layout.historical_config_dir");
    assert_eq!(
        report["topology"],
        json!({"tenant_model": "single_local", "tenant_count": 1})
    );
    finding(&report, "topology.single_local_tenant");
    let absent = finding(&report, "provider.file_absent");
    assert_eq!(
        absent["scope"],
        json!({"kind": "input", "input": "config_dir"})
    );
    assert!(text_of(absent, "action").contains("agentsandbox providers init --config-dir"));
    assert_eq!(report["providers"]["file"]["state"], "absent");
    finding(&report, "identity.sso_not_inspected");
    assert_eq!(report["identity"]["sso"], Value::Null);
    assert_eq!(
        report["identity"]["technical_key_references"],
        json!({"present": 2, "absent": 0, "unknown": 0})
    );
    finding(&report, "data.offline_inspection_not_available");
    finding(&report, "data.location_not_persisted");
    finding(&report, "engine.state_not_checked");
    finding(&report, "reference.content_not_checked");
    assert_eq!(json.code, 0, "a fresh installation has no error");
    assert_eq!(report["summary"]["status"], "ok");
    assert!(text.stdout.ends_with("exit 0\n"));

    assert_documented("report.local-fresh.json", &json.stdout);
    assert_documented("report.local-fresh.txt", &text.stdout);
}

// ---- T03 ------------------------------------------------------------------

#[test]
fn prepared_v1_and_v2_are_explained() {
    let scenario = Scenario::new();
    let local_demo: Value = serde_json::from_str(fixtures::LOCAL_DEMO_MANIFEST).unwrap();
    let cases = [
        ("v1-single", fixtures::manifest_v1(&["tenant-a"]), 1, 1),
        ("v2-single", local_demo, 2, 1),
        (
            "v2-multi",
            fixtures::manifest_v2(&["tenant-a", "tenant-b"]),
            2,
            2,
        ),
    ];
    // Every installation is prepared and inspected before any verdict is read.
    let reports: Vec<_> = cases
        .iter()
        .map(|(name, manifest, _, _)| {
            let installation = scenario.prepare(name, manifest);
            scenario.diagnose_both(["--installation-dir", arg(&installation)])
        })
        .collect();
    for ((name, manifest, version, tenants), (text, json, report)) in cases.into_iter().zip(reports)
    {
        assert_eq!(
            report["layouts"],
            json!([{"input": "installation_dir", "kind": "prepared_installation",
                    "preparation": "complete", "oidc_file_version": version}]),
            "{name}"
        );
        finding(&report, "layout.prepared_installation");
        finding(&report, "layout.legacy_commands_do_not_read_this_layout");
        assert_eq!(
            report["topology"],
            json!({"tenant_model": "registry", "tenant_count": tenants}),
            "{name}"
        );
        assert_eq!(report["tenants"]["registry_total"], tenants, "{name}");
        // Each plan verdict is paired with what the files cannot show.
        if version == 1 {
            finding(&report, "tenants.single_tenant_without_plan");
            assert_eq!(report["tenants"]["plan"], Value::Null);
        } else {
            finding(&report, "tenants.plan_matches_registry");
            assert_eq!(report["tenants"]["correspondence"], "match");
            assert_eq!(report["tenants"]["plan_total"], tenants);
        }
        finding(&report, "tenants.durable_plan_comparison_not_available");
        finding(&report, "registry.offline_rules_passed");
        finding(&report, "registry.loader_acceptance_not_checked");
        finding(&report, "identity.sso_file_valid_offline");
        finding(&report, "oidc.loader_acceptance_not_checked");
        finding(&report, "identity.sso_gateway_enablement_not_persisted");

        // The public SSO parameters are the manifest's.
        let provider = &manifest["gateway"]["provider"];
        let sso = &report["identity"]["sso"];
        assert_eq!(sso["file_version"], version, "{name}");
        for field in [
            "issuer",
            "web_client_id",
            "cli_client_id",
            "api_audience",
            "authorization_endpoint",
            "token_endpoint",
            "jwks_uri",
            "web_redirect_uri",
            "cli_callback_path",
        ] {
            assert_eq!(sso[field], provider[field], "{name}: {field}");
        }
        assert_eq!(sso["transport"], provider["transport"]["kind"], "{name}");
        assert_eq!(sso["provider_label"], manifest["gateway"]["provider_label"]);
        assert_eq!(sso["web_origin"], manifest["gateway"]["web_origin"]);
        assert_eq!(
            sso["mapping"]["revision"],
            manifest["gateway"]["mapping"]["revision"]
        );
        assert_eq!(sso["ca_bundle_declared"], false);
        assert_eq!(sso["idp_contacted"], false);

        // The records that setup keeps are listed by their metadata, never
        // opened. Only a version 2 preparation writes a creation record.
        let records: Vec<&Value> = entries(&report, "credential_references")
            .iter()
            .filter(|entry| entry["role"] == "setup_material")
            .collect();
        let mut expected = vec![
            "<installation>/secret-material.json",
            "<installation>/preparation.json",
            "<installation>/completion.json",
        ];
        if version == 2 {
            expected.push("<installation>/authentication-creation.json");
        }
        expected.push("<installation>/authentication-origin.json");
        assert_eq!(
            records
                .iter()
                .map(|entry| text_of(entry, "display"))
                .collect::<Vec<_>>(),
            expected,
            "{name}"
        );
        for entry in records {
            assert_eq!(entry["presence"], "present", "{name}: {entry}");
            assert_eq!(entry["entry_kind"], "regular_file", "{name}: {entry}");
            assert_eq!(entry["mode"], "0600", "{name}: {entry}");
            assert_eq!(entry["consumer"], "setup_recovery", "{name}: {entry}");
            assert_eq!(entry["content"], "not_read", "{name}: {entry}");
        }

        // SSO is inspected without any AI provider axis.
        finding(&report, "provider.not_inspected");
        assert_eq!(report["providers"], Value::Null);
        assert_eq!(report["summary"]["counts"]["error"], 0, "{name}");
        assert_eq!(json.code, 0, "{name}");
        assert!(
            text.stdout
                .contains("  [ok] identity.sso_file_valid_offline\n")
        );
    }
}

// ---- T04 ------------------------------------------------------------------

#[test]
fn prepared_v1_with_two_tenants_needs_a_plan() {
    let scenario = Scenario::new();
    let single = scenario.prepare("single", &fixtures::manifest_v1(&["tenant-a"]));
    let multi = scenario.prepare("multi", &fixtures::manifest_v1(&["tenant-a", "tenant-b"]));
    let (_, single_json, single_report) =
        scenario.diagnose_both(["--installation-dir", arg(&single)]);
    let (text, json, report) = scenario.diagnose_both(["--installation-dir", arg(&multi)]);

    // The real gateway, same command line: one tenant listens, two do not.
    let control = scenario.gateway(
        &single.join("tenants.json"),
        Some(&single.join("oidc.json")),
    );
    assert!(
        control.listening(),
        "positive control: a version 1 single-tenant preparation starts: {}",
        control.describe()
    );
    let refused = scenario.gateway(&multi.join("tenants.json"), Some(&multi.join("oidc.json")));
    assert_gateway_refuses(
        &refused,
        "unsupported authentication audit format or binding",
        "a multi-tenant version 1 preparation",
    );

    assert!(find(&single_report, "tenants.plan_required_for_multi_tenant").is_none());
    assert_eq!(single_json.code, 0);
    let required = finding(&report, "tenants.plan_required_for_multi_tenant");
    assert_eq!(required["status"], "error");
    assert_eq!(required["basis"], "inferred");
    let action = text_of(required, "action");
    assert!(
        action.contains("3 to 4 migration of the SSO guide"),
        "the action names the documented migration: {action}"
    );
    for action in actions(&report) {
        assert!(
            !action.contains("migrate-"),
            "no action is a migration command line: {action}"
        );
    }
    assert_eq!(json.code, 3);
    assert!(
        text.stdout
            .contains("  [error] tenants.plan_required_for_multi_tenant\n")
    );
}

// ---- T05 ------------------------------------------------------------------

#[test]
fn tenant_set_and_plan_mismatch() {
    let scenario = Scenario::new();
    let manifest = fixtures::manifest_v2(&["tenant-a", "tenant-b"]);
    let base = scenario.prepare("base", &manifest);

    // Each variant is its own preparation with exactly one stated edit.
    let minus = scenario.prepare("registry-minus-b", &manifest);
    fixtures::edit_json(&minus.join("tenants.json"), |registry| {
        registry["tenants"]
            .as_array_mut()
            .unwrap()
            .retain(|tenant| tenant["tenant_id"] != "tenant-b");
    });

    let plus = scenario.prepare("registry-plus-c", &manifest);
    let tenant_c = plus.join("tenants/tenant-c");
    fixtures::private_dir(&tenant_c);
    for role in ["runner", "viewer"] {
        fixtures::private_write(
            &tenant_c.join(role),
            fixtures::synthetic_token(&format!("tenant-c-{role}")),
        );
    }
    fixtures::edit_json(&plus.join("tenants.json"), |registry| {
        registry["tenants"].as_array_mut().unwrap().push(json!({
            "tenant_id": "tenant-c",
            "data": tenant_c.join("audit.sqlite"),
            "runner_token_file": tenant_c.join("runner"),
            "viewer_token_file": tenant_c.join("viewer")
        }));
    });

    let renamed = scenario.prepare("tenant-b-renamed", &manifest);
    fixtures::edit_json(&renamed.join("tenants.json"), |registry| {
        for tenant in registry["tenants"].as_array_mut().unwrap() {
            if tenant["tenant_id"] == "tenant-b" {
                tenant["tenant_id"] = "tenant-b2".into();
            }
        }
    });

    let no_plan = scenario.prepare("plan-removed", &manifest);
    fixtures::edit_json(&no_plan.join("oidc.json"), |oidc| {
        oidc["authentication_audit"]
            .as_object_mut()
            .unwrap()
            .remove("capacity_plan")
            .expect("a version 2 preparation records its plan");
    });

    let run =
        |installation: &Path| scenario.diagnose_both(["--installation-dir", arg(installation)]);
    let (_, base_json, base_report) = run(&base);
    let reports = [
        ("registry minus tenant-b", &minus, run(&minus)),
        ("registry plus tenant-c", &plus, run(&plus)),
        ("tenant-b renamed", &renamed, run(&renamed)),
        ("plan removed", &no_plan, run(&no_plan)),
    ];

    // The real gateway: the unmodified preparation listens, each edit does not.
    let gateway = |installation: &Path| {
        scenario.gateway(
            &installation.join("tenants.json"),
            Some(&installation.join("oidc.json")),
        )
    };
    let control = gateway(&base);
    assert!(
        control.listening(),
        "positive control: the unmodified preparation starts: {}",
        control.describe()
    );
    // The cause the gateway states for each edit. A removed and an added
    // tenant reach its comparison of the tenant set with the stored plan. The
    // renamed tenant does not: its store, created for `tenant-b`, is refused
    // first for its tenant binding, which no build inspects offline. That
    // edit shows that the gateway refuses, not why the mismatch matters.
    let causes = [
        "unsupported authentication audit format or binding",
        "unsupported authentication audit format or binding",
        "database tenant binding mismatch",
        "OIDC configuration requires version1 without a capacity plan or version2 with an explicit plan",
    ];
    for ((edit, installation, _), cause) in reports.iter().zip(causes) {
        assert_gateway_refuses(&gateway(installation), cause, edit);
    }

    assert_eq!(base_json.code, 0);
    finding(&base_report, "tenants.plan_matches_registry");
    let id = |id: &str| json!({"id": id, "masked": false});
    let expected = [
        (vec![], vec![id("tenant-b")]),
        (vec![id("tenant-c")], vec![]),
        (vec![id("tenant-b2")], vec![id("tenant-b")]),
    ];
    for ((edit, _, (text, json, report)), (only_in_registry, only_in_plan)) in
        reports.iter().zip(&expected)
    {
        let mismatch = finding(report, "tenants.plan_set_mismatch");
        assert_eq!(mismatch["status"], "error", "{edit}");
        assert_eq!(
            mismatch["details"],
            json!({"kind": "tenant_set_difference", "only_in_registry": only_in_registry,
                   "only_in_plan": only_in_plan, "omitted": 0}),
            "{edit}"
        );
        assert_eq!(report["tenants"]["correspondence"], "mismatch", "{edit}");
        assert!(find(report, "tenants.plan_matches_registry").is_none());
        assert_eq!(json.code, 3, "{edit}");
        assert!(
            text.stdout
                .contains("  [error] tenants.plan_set_mismatch\n")
        );
    }
    let (_, _, (_, json, report)) = &reports[3];
    finding(report, "oidc.version_plan_pair_invalid");
    assert!(find(report, "tenants.plan_matches_registry").is_none());
    assert_eq!(json.code, 3);
}

// ---- T06 ------------------------------------------------------------------

#[test]
fn setup_states() {
    let scenario = Scenario::new();
    let manifest = fixtures::manifest_v1(&["tenant-a"]);
    let resume_action =
        "agentsandbox setup resume --recovery-dir \"<installation>\" --confirm-control-stopped";

    // An unpublished preparation: the published directory moved back to the
    // staging name the product derives for its final path.
    let unpublished_final = scenario.prepare("unpublished", &manifest);
    let unpublished = scenario.path(&fixtures::staging_name(&unpublished_final));
    fs::rename(&unpublished_final, &unpublished).unwrap();
    let (_, staging_json, staging_report) =
        scenario.diagnose_both(["--installation-dir", arg(&unpublished)]);
    let resumed = scenario.resume(&unpublished);
    let resume_accepts_this_state = resumed.code == Some(0);
    if resume_accepts_this_state {
        assert!(
            unpublished_final.join("tenants.json").is_file() && !unpublished.exists(),
            "setup resume publishes the staging directory to its final path"
        );
    } else {
        oracle_not_run(
            "setup-resume-on-unpublished-preparation",
            "setup resume refused a directory renamed back to its staging name",
        );
    }

    // The staging directory and its final path both exist.
    let both_final = scenario.prepare("both", &manifest);
    let both = scenario.path(&fixtures::staging_name(&both_final));
    fs::rename(&both_final, &both).unwrap();
    fixtures::copy_tree(&both, &both_final);
    let (_, both_json, both_report) = scenario.diagnose_both(["--installation-dir", arg(&both)]);
    let refused = scenario.resume(&both);
    assert_ne!(
        refused.code,
        Some(0),
        "setup resume never publishes over an existing destination"
    );

    // A published installation without its completion marker.
    let unmarked = scenario.prepare("unmarked", &manifest);
    fs::remove_file(unmarked.join("completion.json")).unwrap();
    let (_, unmarked_json, unmarked_report) =
        scenario.diagnose_both(["--installation-dir", arg(&unmarked)]);
    let completed = scenario.resume(&unmarked);
    eprintln!(
        "OBSERVED setup resume on a published installation without completion.json: exit {:?}, marker present afterwards: {}",
        completed.code,
        unmarked.join("completion.json").exists()
    );
    assert_eq!(
        completed.code,
        Some(0),
        "as observed: setup resume accepts a published installation without its marker"
    );
    assert!(
        unmarked.join("completion.json").is_file(),
        "as observed: setup resume writes the completion marker again"
    );

    assert_eq!(
        staging_report["layouts"][0]["kind"],
        "setup_staging_directory"
    );
    let staging = finding(&staging_report, "layout.setup_staging_directory");
    assert!(
        text_of(staging, "action").contains(resume_action),
        "the action resumes this directory"
    );
    assert_eq!(staging_json.code, 3);
    assert!(
        resume_accepts_this_state,
        "the action printed for a staging directory must be accepted by the product"
    );

    let both_finding = finding(&both_report, "layout.staging_and_final_both_exist");
    assert!(!text_of(both_finding, "action").contains("--recovery-dir"));
    assert!(find(&both_report, "layout.setup_staging_directory").is_none());
    assert_eq!(both_json.code, 3);

    let unmarked_finding = finding(&unmarked_report, "layout.completion_marker_absent");
    assert_eq!(unmarked_finding["status"], "warning");
    assert!(text_of(unmarked_finding, "action").contains(resume_action));
    assert_eq!(
        unmarked_report["layouts"][0]["preparation"],
        "completion_marker_absent"
    );
    assert_eq!(
        unmarked_json.code, 0,
        "a warning alone leaves the exit code at 0: {}",
        unmarked_report["summary"]
    );
}

// ---- T07 ------------------------------------------------------------------

#[test]
fn legacy_command_does_not_read_prepared_layout() {
    let scenario = Scenario::new();
    let prepared = scenario.prepare("prepared", &fixtures::manifest_v2(&["tenant-a"]));
    let historical = scenario.historical("historical");
    let (text, json, report) = scenario.diagnose_both(["--config-dir", arg(&prepared)]);

    // A real legacy command: it cannot find its key in a prepared layout, and
    // gets past the key read in a historical directory. It writes nothing.
    let verify = |directory: &Path| {
        let before = tree_manifest::take(&scenario.root);
        let output = scenario.product([
            "verify-audit",
            "--config-dir",
            arg(directory),
            "--gateway",
            "http://127.0.0.1:1",
        ]);
        tree_manifest::assert_unchanged(
            &before,
            &tree_manifest::take(&scenario.root),
            "verify-audit",
        );
        assert_ne!(output.code, Some(0), "no gateway listens on port 1");
        output.stderr_text()
    };
    assert!(
        verify(&prepared).contains("run agentsandbox init first"),
        "the legacy command does not read a prepared installation"
    );
    assert!(
        !verify(&historical).contains("run agentsandbox init first"),
        "positive control: the legacy command reads its key in a historical directory"
    );

    let passed = finding(&report, "layout.prepared_passed_as_config_dir");
    assert_eq!(passed["status"], "error");
    assert!(text_of(passed, "action").contains("--installation-dir"));
    let init_word = regex::Regex::new(r"\binit\b").unwrap();
    for action in actions(&report) {
        assert!(
            !init_word.is_match(action),
            "no action proposes init on a prepared installation: {action}"
        );
    }
    assert_eq!(json.code, 3);
    assert!(
        text.stdout
            .contains("  [error] layout.prepared_passed_as_config_dir\n")
    );
}

// ---- T08 ------------------------------------------------------------------

#[test]
fn init_over_prepared_installation_is_mixed() {
    let scenario = Scenario::new();
    let mixed = scenario.prepare("mixed", &fixtures::manifest_v2(&["tenant-a"]));
    agentsandbox::operations::initialize_config(&mixed)
        .expect("fixture: the product init accepts an existing private directory");
    assert!(mixed.join("secrets/runner").is_file() && mixed.join("preparation.json").is_file());

    for flag in ["--installation-dir", "--config-dir"] {
        let (text, json, report) = scenario.diagnose_both([flag, arg(&mixed)]);
        assert_eq!(report["layouts"][0]["kind"], "mixed", "{flag}");
        let mixed_finding = finding(&report, "layout.mixed");
        assert_eq!(mixed_finding["status"], "error");
        for action in actions(&report) {
            assert!(
                !action.contains("providers init"),
                "no action proposes providers init on a mixed directory: {action}"
            );
        }
        assert_eq!(json.code, 3, "{flag}");
        assert!(text.stdout.contains("  [error] layout.mixed\n"));
    }
}

// ---- T09 ------------------------------------------------------------------

#[test]
fn relocated_installation() {
    let manifest = fixtures::manifest_v2(&["tenant-a"]);

    // The installation moved and nothing is left at its recorded path.
    let emptied = Scenario::new();
    let original = emptied.prepare("installation", &manifest);
    let moved = emptied.path("moved");
    fs::rename(&original, &moved).unwrap();
    let (text, json, report) = emptied.diagnose_both(["--installation-dir", arg(&moved)]);

    // A second valid installation now sits at the recorded path: the report
    // describes what is at the recorded paths and opens no file of the decoy.
    let replaced = Scenario::new();
    let decoy_path = replaced.prepare("installation", &manifest);
    let decoy_moved = replaced.path("moved");
    fs::rename(&decoy_path, &decoy_moved).unwrap();
    assert_eq!(replaced.prepare("installation", &manifest), decoy_path);
    let observed = observed_run(
        &replaced,
        &["--installation-dir", arg(&decoy_moved), "--json"],
    );

    finding(&report, "layout.references_outside_inspected_root");
    let not_found = finding(&report, "reference.not_found_from_here");
    assert_eq!(not_found["status"], "unknown");
    assert!(text_of(not_found, "action").contains("--show-paths"));
    assert!(
        entries(&report, "credential_references")
            .iter()
            .filter(|entry| entry["role"] == "runner" || entry["role"] == "viewer")
            .all(|entry| entry["location"] == "outside_root" && entry["presence"] == "absent")
    );
    assert_eq!(report["summary"]["counts"]["error"], 0);
    assert_eq!(json.code, 4, "an unknown without an error exits 4");
    assert!(text.stdout.ends_with("exit 4\n"));
    assert!(!text.stdout.contains(arg(&original)));

    let report = observed.run.json();
    let outside = finding(&report, "layout.references_outside_inspected_root");
    assert_eq!(outside["status"], "warning");
    assert!(
        entries(&report, "credential_references")
            .iter()
            .filter(|entry| entry["role"] == "runner" || entry["role"] == "viewer")
            .all(|entry| entry["location"] == "outside_root" && entry["presence"] == "present")
    );
    assert!(find(&report, "reference.not_found_from_here").is_none());
    assert_never_read(
        &observed,
        "moved/tenants.json",
        &["installation/tenants.json", "installation/oidc.json"],
        "decoy at the recorded path",
    );
}

// ---- T10 ------------------------------------------------------------------

#[test]
fn axes_are_independent_under_faults() {
    let scenario = Scenario::new();

    // SSO and tenants, with and without a broken provider file.
    let installation = scenario.prepare("installation", &fixtures::manifest_v2(&["tenant-a"]));
    let broken_providers = scenario.historical("config-with-broken-providers");
    fixtures::private_write(&broken_providers.join("providers.json"), "{ not json");
    let (_, _, sso_alone) = scenario.diagnose_both(["--installation-dir", arg(&installation)]);
    let (_, _, sso_with_fault) = scenario.diagnose_both([
        "--installation-dir",
        arg(&installation),
        "--config-dir",
        arg(&broken_providers),
    ]);

    // Provider profiles, with and without a broken OIDC file.
    let config = scenario.historical("config");
    let credentials = fixtures::provider_credentials(&scenario.path("credentials"));
    fixtures::write_providers(
        &config.join("providers.json"),
        &fixtures::four_profiles(&credentials),
    );
    let broken_oidc = scenario.path("broken-oidc.json");
    fixtures::private_write(&broken_oidc, "{ not json");
    let (_, _, providers_alone) = scenario.diagnose_both(["--config-dir", arg(&config)]);
    let (_, _, providers_with_fault) = scenario.diagnose_both([
        "--config-dir",
        arg(&config),
        "--oidc-config",
        arg(&broken_oidc),
    ]);

    assert!(
        sso_alone["identity"]["sso"].is_object(),
        "the SSO file of the installation is inspected"
    );
    finding(&sso_with_fault, "provider.configuration_invalid");
    for section in ["identity", "tenants"] {
        assert_eq!(
            sso_alone[section], sso_with_fault[section],
            "{section} does not depend on the provider axis"
        );
    }
    assert_eq!(
        findings_of_axes(&sso_alone, &["identity", "tenants"]),
        findings_of_axes(&sso_with_fault, &["identity", "tenants"])
    );

    assert_eq!(
        entries(&providers_alone["providers"], "profiles").len(),
        4,
        "the provider profiles are inspected"
    );
    finding(&providers_with_fault, "oidc.json_invalid");
    assert_eq!(
        providers_alone["providers"], providers_with_fault["providers"],
        "providers do not depend on the identity axis"
    );
    assert_eq!(
        findings_of_axes(&providers_alone, &["providers"]),
        findings_of_axes(&providers_with_fault, &["providers"])
    );
}

// ---- T11 ------------------------------------------------------------------

/// What the product's own preparation says for one profile: the class prefix
/// of its refusal, or `None` when it accepts the credential.
fn product_refusal_class(scenario: &Scenario, providers: &Path, profile: &str) -> Option<String> {
    let config = agentsandbox::provider::load_config(providers)
        .expect("fixture: the product accepts this profile file");
    let selection = agentsandbox::provider::ProviderSelection {
        profile_id: Some(profile.to_owned()),
        ..Default::default()
    };
    agentsandbox::provider::prepare(
        &config,
        &selection,
        &scenario.work,
        Some("inspect"),
        uuid::Uuid::new_v4(),
    )
    .err()
    .map(|error| {
        error
            .to_string()
            .split_once(':')
            .map_or_else(|| "unprefixed".to_owned(), |(class, _)| class.to_owned())
    })
}

#[test]
fn provider_references_from_metadata() {
    let scenario = Scenario::new();
    let mut ordinal = 0;
    // Each case: a config directory, its own credentials, and one stated edit.
    let mut case = |profiles: &dyn Fn(&fixtures::ProviderCredentials) -> Value,
                    edit: &dyn Fn(&fixtures::ProviderCredentials)| {
        ordinal += 1;
        let config = scenario.historical(&format!("config-{ordinal}"));
        let credentials =
            fixtures::provider_credentials(&scenario.path(&format!("credentials-{ordinal}")));
        let providers = config.join("providers.json");
        fixtures::write_providers(&providers, &profiles(&credentials));
        edit(&credentials);
        let (_, json, report) = scenario.diagnose_both(["--config-dir", arg(&config)]);
        (providers, json.code, report)
    };
    let one = |profile: Value| json!({"version": 1, "profiles": [profile]});

    // Positive control of every refusal below: untouched credentials are accepted.
    let (providers, code, report) = case(&fixtures::four_profiles, &|_| {});
    for profile in [
        "codex-subscription",
        "claude-subscription",
        "codex-api",
        "claude-api",
    ] {
        assert_eq!(
            product_refusal_class(&scenario, &providers, profile),
            None,
            "positive control: the product accepts the synthetic credential of {profile}"
        );
    }
    let complete = (code, report);

    let (providers, code, report) = case(
        &|credentials| {
            one(fixtures::subscription_profile(
                "only",
                "codex",
                &credentials.codex_subscription,
            ))
        },
        &|credentials| fs::remove_file(&credentials.codex_subscription).unwrap(),
    );
    assert_eq!(
        product_refusal_class(&scenario, &providers, "only").as_deref(),
        Some("login_required")
    );
    let subscription_absent = (code, report);

    let (providers, code, report) = case(
        &|credentials| {
            one(fixtures::api_profile(
                "only",
                "codex",
                &credentials.codex_api,
            ))
        },
        &|credentials| fs::remove_file(&credentials.codex_api).unwrap(),
    );
    assert_eq!(
        product_refusal_class(&scenario, &providers, "only").as_deref(),
        Some("credential_missing")
    );
    let api_absent = (code, report);

    // The automatic selection resolves the Codex subscription profile here:
    // the Claude subscription profile is one that it does not resolve.
    let (providers, code, report) = case(&fixtures::four_profiles, &|credentials| {
        fs::remove_file(&credentials.claude_subscription).unwrap();
    });
    assert_eq!(
        product_refusal_class(&scenario, &providers, "claude-subscription").as_deref(),
        Some("login_required")
    );
    assert_eq!(
        product_refusal_class(&scenario, &providers, "claude-api"),
        None
    );
    let one_of_four_absent = (code, report);

    let (providers, code, report) = case(
        &|credentials| {
            one(fixtures::api_profile(
                "only",
                "codex",
                &credentials.codex_api,
            ))
        },
        &|credentials| {
            fs::set_permissions(&credentials.codex_api, fs::Permissions::from_mode(0o644)).unwrap();
        },
    );
    assert_eq!(
        product_refusal_class(&scenario, &providers, "only").as_deref(),
        Some("credential_unsafe")
    );
    let readable_by_others = (code, report);

    let (providers, code, report) = case(
        &|credentials| {
            one(fixtures::api_profile(
                "only",
                "codex",
                &credentials.directory.join("link.env"),
            ))
        },
        &|credentials| {
            symlink(
                &credentials.codex_api,
                credentials.directory.join("link.env"),
            )
            .unwrap();
        },
    );
    assert_eq!(
        product_refusal_class(&scenario, &providers, "only").as_deref(),
        Some("credential_unsafe")
    );
    let symbolic_link = (code, report);

    let (code, report) = &complete;
    finding(report, "provider.configuration_valid");
    assert_eq!(entries(&report["providers"], "profiles").len(), 4);
    finding(report, "provider.credential_content_not_checked");
    finding(report, "provider.manual_launch_path_not_checked");
    finding(report, "provider.launch_requires_observe_and_bridge");
    assert_eq!(
        finding(report, "provider.selection_resolved")["subjects"]["ids"],
        json!([{"id": "codex-subscription", "masked": false}])
    );
    assert_eq!(*code, 0);

    let (code, report) = &subscription_absent;
    assert_eq!(
        finding(report, "provider.login_required")["status"],
        "error"
    );
    assert!(find(report, "provider.credential_missing").is_none());
    assert_eq!(*code, 3);

    let (code, report) = &api_absent;
    assert_eq!(
        finding(report, "provider.credential_missing")["status"],
        "error"
    );
    assert!(find(report, "provider.login_required").is_none());
    assert_eq!(*code, 3);

    let (code, report) = &one_of_four_absent;
    let unselected = finding(report, "provider.unselected_profile_reference_absent");
    assert_eq!(unselected["status"], "warning");
    assert_eq!(
        unselected["details"],
        json!({"kind": "would_refuse_with", "class": "login_required"})
    );
    assert_eq!(
        unselected["subjects"]["ids"],
        json!([{"id": "claude-subscription", "masked": false}])
    );
    assert!(find(report, "provider.login_required").is_none());
    assert_eq!(
        *code, 0,
        "an absent reference of a profile that the selection does not resolve fails no launch with this selection"
    );

    for (code, report) in [&readable_by_others, &symbolic_link] {
        assert_eq!(
            finding(report, "provider.credential_unsafe")["status"],
            "error"
        );
        assert_eq!(*code, 3);
    }
}

// ---- T12 ------------------------------------------------------------------

#[test]
fn provider_configuration_refusals() {
    let scenario = Scenario::new();
    let credentials = fixtures::provider_credentials(&scenario.path("credentials"));
    let api = |id: &str| fixtures::api_profile(id, "codex", &credentials.codex_api);
    let with = |mut profile: Value, field: &str, value: Value| {
        profile[field] = value;
        profile
    };
    let mut wrong_source = api("a");
    wrong_source["auth_mode"] = "subscription".into();
    let mut wrong_client =
        fixtures::subscription_profile("a", "codex", &credentials.codex_subscription);
    wrong_client["client"] = "claude_code".into();
    let mut wrong_variable = api("a");
    wrong_variable["credential"]["key"] = "ANTHROPIC_API_KEY".into();
    let mut traversal = api("a");
    traversal["credential"]["path"] = "../credentials/codex-api.env".into();
    let mut absolute_traversal = api("a");
    absolute_traversal["credential"]["path"] = "/nonexistent/../codex-api.env".into();

    // One variant per refusal site of the product loader. The site that
    // requires an absolute path is not reachable through the loader, which
    // resolves every relative reference first.
    let variants: Vec<(&str, &str, &str, Value)> = vec![
        (
            "duplicate-profile",
            "provider.configuration_ambiguous",
            "duplicate_or_invalid_profile",
            json!({"version": 1, "profiles": [api("a"), api("a")]}),
        ),
        (
            "revision-zero",
            "provider.configuration_ambiguous",
            "duplicate_or_invalid_profile",
            json!({"version": 1, "profiles": [with(api("a"), "revision", 0.into())]}),
        ),
        (
            "unknown-priority",
            "provider.configuration_ambiguous",
            "unknown_or_duplicate_priority",
            json!({"version": 1, "auto_priority": ["b"], "profiles": [api("a")]}),
        ),
        (
            "default-missing",
            "provider.configuration_ambiguous",
            "default_profile_missing",
            json!({"version": 1, "default_profile": "b", "profiles": [api("a")]}),
        ),
        (
            "unknown-field",
            "provider.configuration_invalid",
            "json_invalid",
            json!({"version": 1, "profiles": [with(api("a"), "url", "https://inference.example.invalid".into())]}),
        ),
        (
            "version",
            "provider.configuration_invalid",
            "version_or_size",
            json!({"version": 2, "profiles": [api("a")]}),
        ),
        (
            "profile-identifier",
            "provider.configuration_invalid",
            "profile_identifier",
            json!({"version": 1, "profiles": [api("not an identifier")]}),
        ),
        (
            "model-identifier",
            "provider.configuration_invalid",
            "model_identifier",
            json!({"version": 1, "profiles": [with(api("a"), "model", "-leading-hyphen".into())]}),
        ),
        (
            "source-for-mode",
            "provider.configuration_invalid",
            "source_for_mode",
            json!({"version": 1, "profiles": [wrong_source]}),
        ),
        (
            "source-for-client",
            "provider.configuration_invalid",
            "source_for_mode",
            json!({"version": 1, "profiles": [wrong_client]}),
        ),
        (
            "variable-for-client",
            "provider.configuration_invalid",
            "variable_for_client",
            json!({"version": 1, "profiles": [wrong_variable]}),
        ),
        (
            "path-traversal",
            "provider.configuration_invalid",
            "path_traversal",
            json!({"version": 1, "profiles": [traversal]}),
        ),
        (
            "absolute-path-traversal",
            "provider.configuration_invalid",
            "path_traversal",
            json!({"version": 1, "profiles": [absolute_traversal]}),
        ),
    ];

    // Positive control: the same shape without any edit is accepted.
    let accepted = scenario.path("accepted.json");
    fixtures::write_providers(&accepted, &json!({"version": 1, "profiles": [api("a")]}));
    agentsandbox::provider::load_config(&accepted)
        .expect("positive control: the product accepts the unedited profile file");

    let mut results = Vec::new();
    for (name, code, reason, config) in variants {
        let providers = scenario.path(&format!("{name}.json"));
        fixtures::write_providers(&providers, &config);
        let refusal = agentsandbox::provider::load_config(&providers)
            .expect_err("the product loader refuses this variant")
            .to_string();
        let (text, json, report) = scenario.diagnose_both(["--providers-file", arg(&providers)]);
        results.push((name, code, reason, refusal, text, json, report));
    }
    for (name, code, reason, refusal, text, json, report) in &results {
        let refused = finding(report, code);
        assert_eq!(refused["status"], "error", "{name}");
        assert_eq!(
            refused["details"],
            json!({"kind": "reason", "reason": reason}),
            "{name}"
        );
        assert_eq!(report["providers"]["file"]["state"], "invalid", "{name}");
        assert_eq!(json.code, 3, "{name}");
        // The loader's own sentence is never printed, with or without its class.
        let sentence = refusal
            .split_once(": ")
            .map_or(refusal.as_str(), |(_, sentence)| sentence);
        for output in [&text.stdout, &json.stdout, &text.stderr, &json.stderr] {
            assert!(
                !output.contains(sentence),
                "{name}: the text of the product error is printed"
            );
        }
    }
}

// ---- T13 ------------------------------------------------------------------

#[test]
fn default_mode_never_reads_credentials() {
    // Provider profiles and the historical keys.
    let scenario = Scenario::new();
    let config = scenario.historical("config");
    let credentials = fixtures::provider_credentials(&scenario.path("credentials"));
    fixtures::write_providers(
        &config.join("providers.json"),
        &fixtures::four_profiles(&credentials),
    );
    let args = ["--config-dir", arg(&config), "--json"];
    let providers_first = observed_run(&scenario, &args);
    let canary_files = [
        config.join("secrets/runner"),
        config.join("secrets/viewer"),
        credentials.codex_api.clone(),
        credentials.claude_api.clone(),
        credentials.codex_subscription.clone(),
        credentials.claude_subscription.clone(),
    ];
    for path in &canary_files {
        fixtures::overwrite(path, format!("not a credential {CANARY}\n"));
    }
    let providers_second = scenario.diagnose(args);
    let providers_text = scenario.diagnose(["--config-dir", arg(&config)]);

    // A prepared installation: tokens, setup records and stores.
    let prepared = Scenario::new();
    let installation = prepared.prepare("installation", &fixtures::manifest_v2(&["tenant-a"]));
    let args = ["--installation-dir", arg(&installation), "--json"];
    let prepared_first = observed_run(&prepared, &args);
    let canary_files = [
        installation.join("tenants/tenant-a/runner"),
        installation.join("tenants/tenant-a/viewer"),
        installation.join("tenants/tenant-a/audit.sqlite"),
        installation.join("authentication.sqlite"),
        installation.join("secret-material.json"),
        installation.join("preparation.json"),
        installation.join("completion.json"),
        installation.join("authentication-creation.json"),
        installation.join("authentication-origin.json"),
    ];
    for path in &canary_files {
        fixtures::overwrite(path, format!("not a record {CANARY}\n"));
    }
    let prepared_second = prepared.diagnose(args);
    let prepared_text = prepared.diagnose(["--installation-dir", arg(&installation)]);

    for run in [
        &providers_first.run,
        &providers_second,
        &providers_text,
        &prepared_first.run,
        &prepared_second,
        &prepared_text,
    ] {
        assert_no_canary(run, &[CANARY], "content of a credential or a record");
    }

    let report = providers_first.run.json();
    assert_eq!(
        entries(&report["providers"], "profiles").len(),
        4,
        "the provider profiles are inspected"
    );
    assert!(
        entries(&report, "credential_references")
            .iter()
            .all(|entry| entry["content"] == "not_read")
    );
    assert_eq!(
        providers_first.run.stdout, providers_second.stdout,
        "the report does not depend on the content of any credential"
    );
    assert_never_read(
        &providers_first,
        "config/providers.json",
        &["config/secrets", "config/mounted-secrets", "credentials"],
        "provider profiles",
    );
    // The closed list of the report is exactly what was opened: one file,
    // and no directory was listed.
    assert_eq!(
        report["mode"]["files_read_for_content"],
        json!(["provider_profiles"])
    );
    if let Some(accessed) = providers_first.accessed() {
        assert_eq!(
            accessed.iter().collect::<Vec<_>>(),
            [Path::new("config/providers.json")],
            "nothing but the profile file is opened or listed"
        );
    }

    let report = prepared_first.run.json();
    assert_eq!(
        entries(&report, "credential_references")
            .iter()
            .filter(|entry| entry["role"] == "runner" || entry["role"] == "viewer")
            .count(),
        2,
        "the registry references are inspected"
    );
    assert_eq!(entries(&report, "data").len(), 2, "both stores are listed");
    assert_eq!(
        prepared_first.run.stdout, prepared_second.stdout,
        "the report does not depend on the content of any token, record or store"
    );
    assert_never_read(
        &prepared_first,
        "installation/tenants.json",
        &[
            "installation/tenants/tenant-a",
            "installation/authentication.sqlite",
            "installation/secret-material.json",
            "installation/preparation.json",
            "installation/completion.json",
            "installation/authentication-creation.json",
            "installation/authentication-origin.json",
            "installation.manifest.json",
        ],
        "prepared installation",
    );
    assert_eq!(
        entries(&report, "credential_references")
            .iter()
            .filter(|entry| entry["role"] == "setup_material")
            .count(),
        5,
        "the three setup records and the two records of the authentication audit are listed"
    );
    // The closed list of the report is exactly what was opened: the two
    // gateway files. No directory is listed, neither the installation nor
    // its parent.
    assert_eq!(
        report["mode"]["files_read_for_content"],
        json!(["tenant_registry", "oidc_file"])
    );
    if let Some(accessed) = prepared_first.accessed() {
        let listed: Vec<_> = accessed
            .iter()
            .filter(|path| prepared.root.join(path).is_dir())
            .collect();
        assert!(listed.is_empty(), "the tool listed a directory: {listed:?}");
        assert_eq!(
            accessed.iter().collect::<Vec<_>>(),
            [
                Path::new("installation/oidc.json"),
                Path::new("installation/tenants.json")
            ],
            "nothing but the registry and the OIDC file is opened"
        );
    }
}

// ---- T14 ------------------------------------------------------------------

#[test]
fn registry_references_follow_the_gateway_reader() {
    let scenario = Scenario::new();
    let variant = |name: &str, edit: &dyn Fn(&Path)| {
        let directory = scenario.path(name);
        fixtures::private_dir(&directory);
        let registry = fixtures::hand_written_registry(&directory, &["tenant-a"]);
        edit(&directory);
        let (_, json, report) = scenario.diagnose_both(["--tenants-file", arg(&registry)]);
        // After the report: the gateway creates the store of the tenant.
        let gateway = scenario.gateway(&registry, None);
        (json.code, report, gateway)
    };
    let set_mode = |path: PathBuf, mode: u32| {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    };

    let private = variant("private", &|_| {});
    let group_readable = variant("mode-0644", &|directory| {
        set_mode(directory.join("tenant-a/runner"), 0o644);
    });
    let projected = variant("mode-0444", &|directory| {
        set_mode(directory.join("tenant-a/runner"), 0o444);
        set_mode(directory.join("tenant-a/viewer"), 0o444);
    });
    let hard_linked = variant("hard-link", &|directory| {
        fs::hard_link(
            directory.join("tenant-a/runner"),
            directory.join("second-name-of-runner"),
        )
        .unwrap();
    });
    let linked = variant("symbolic-link", &|directory| {
        let runner = directory.join("tenant-a/runner");
        fs::rename(&runner, directory.join("real-runner")).unwrap();
        symlink(directory.join("real-runner"), &runner).unwrap();
    });
    let removed = variant("removed", &|directory| {
        fs::remove_file(directory.join("tenant-a/runner")).unwrap();
    });

    // The gateway accepts what its token reader accepts, whatever the mode.
    for (name, (_, _, gateway)) in [
        ("private", &private),
        ("mode 0644", &group_readable),
        ("mode 0444", &projected),
        ("hard link", &hard_linked),
    ] {
        assert!(
            gateway.listening(),
            "the gateway starts with tokens of {name}: {}",
            gateway.describe()
        );
    }
    for (name, (_, _, gateway)) in [("symbolic link", &linked), ("removed", &removed)] {
        assert_gateway_refuses(
            gateway,
            "Impossible de lire le fichier de secret configuré",
            name,
        );
    }

    let runner = |report: &Value| {
        entries(report, "credential_references")
            .iter()
            .find(|entry| entry["role"] == "runner")
            .cloned()
            .expect("the runner reference is listed")
    };
    let (code, report, _) = &private;
    assert_eq!(report["layouts"][0]["kind"], "hand_written_registry");
    finding(report, "layout.hand_written_registry");
    assert_eq!(runner(report)["consumer"], "gateway_token_reader");
    assert_eq!(runner(report)["mode"], "0600");
    assert_eq!(runner(report)["location"], "inside_root");
    assert!(find(report, "reference.token_file_accessible_to_other_accounts").is_none());
    finding(report, "data.store_absent");
    finding(report, "tenants.correspondence_not_checked");
    assert_eq!(*code, 0);

    for (mode, (code, report, _)) in [("0644", &group_readable), ("0444", &projected)] {
        let accessible = finding(report, "reference.token_file_accessible_to_other_accounts");
        assert_eq!(accessible["status"], "warning");
        assert_eq!(runner(report)["mode"], mode);
        assert_eq!(report["summary"]["counts"]["error"], 0, "mode {mode}");
        assert_eq!(*code, 0, "mode {mode}");
    }

    let (code, report, _) = &hard_linked;
    assert_eq!(runner(report)["single_link"], false);
    assert_eq!(
        report["summary"]["counts"]["error"], 0,
        "the gateway token reader does not check the link count"
    );
    assert_eq!(*code, 0);

    let (code, report, _) = &linked;
    assert_eq!(
        finding(report, "reference.not_a_regular_file")["status"],
        "error"
    );
    assert_eq!(runner(report)["entry_kind"], "symbolic_link");
    assert_eq!(*code, 3);

    let (code, report, _) = &removed;
    assert_eq!(finding(report, "reference.absent")["status"], "error");
    assert_eq!(runner(report)["presence"], "absent");
    assert_eq!(*code, 3);

    // The documented enterprise registry points at container paths: from the
    // host they are not found, which is unknown, not an error.
    let example = scenario.path("tenants.example.json");
    fixtures::private_write(&example, include_str!("../config/tenants.example.json"));
    let (text, json, report) = scenario.diagnose_both(["--tenants-file", arg(&example)]);
    let not_found = finding(&report, "reference.not_found_from_here");
    assert_eq!(not_found["status"], "unknown");
    assert_eq!(not_found["subjects"]["total"], 2);
    assert!(find(&report, "reference.absent").is_none());
    assert!(
        entries(&report, "credential_references")
            .iter()
            .all(|entry| entry["location"] == "outside_root" && entry["presence"] == "absent")
    );
    assert_eq!(report["summary"]["counts"]["error"], 0);
    assert_eq!(json.code, 4);
    assert!(
        !text.stdout.contains("/run/secrets"),
        "an operator-chosen location is not shown without --show-paths"
    );
}

// ---- T15 ------------------------------------------------------------------

#[test]
fn historical_roles_and_private_rule() {
    let scenario = Scenario::new();
    let with_key = scenario.historical_with_decision_engine_key("with-key");
    let (_, with_key_json, with_key_report) =
        scenario.diagnose_both(["--config-dir", arg(&with_key)]);

    let readable = scenario.historical("readable");
    agentsandbox::runner::read_token(&readable.join("secrets/runner"))
        .expect("positive control: the private-file reader accepts the key created by init");
    fs::set_permissions(
        readable.join("secrets/runner"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert!(
        agentsandbox::runner::read_token(&readable.join("secrets/runner")).is_err(),
        "the private-file reader refuses a key readable by other accounts"
    );
    let (text, json, report) = scenario.diagnose_both(["--config-dir", arg(&readable)]);

    let roles: Vec<(String, String)> = entries(&with_key_report, "credential_references")
        .iter()
        .map(|entry| {
            (
                text_of(entry, "role").to_owned(),
                text_of(entry, "display").to_owned(),
            )
        })
        .collect();
    for expected in [
        ("runner", "<config>/secrets/runner"),
        ("viewer", "<config>/secrets/viewer"),
        ("decision_engine", "<config>/secrets/jev"),
        ("projection", "<config>/mounted-secrets/runner"),
        ("projection", "<config>/mounted-secrets/viewer"),
        ("projection", "<config>/mounted-secrets/jev"),
    ] {
        assert!(
            roles
                .iter()
                .any(|(role, display)| (role.as_str(), display.as_str()) == expected),
            "the reference {expected:?} is listed; listed: {roles:?}"
        );
    }
    assert!(
        entries(&with_key_report, "credential_references")
            .iter()
            .filter(|entry| entry["role"] != "projection")
            .all(|entry| entry["consumer"] == "private_file_reader"
                && entry["private_rule"] == "satisfied")
    );
    assert_eq!(with_key_json.code, 0);

    let violated = finding(&report, "reference.private_rule_violated");
    assert_eq!(violated["status"], "error");
    assert_eq!(violated["basis"], "inferred");
    assert_eq!(json.code, 3);
    assert!(
        text.stdout
            .contains("  [error] reference.private_rule_violated\n")
    );
    assert_eq!(
        fs::metadata(readable.join("secrets/runner"))
            .unwrap()
            .permissions()
            .mode()
            & 0o7777,
        0o644,
        "the tool never repairs a file"
    );
}

// ---- T16 ------------------------------------------------------------------

#[test]
fn ca_bundles() {
    let scenario = Scenario::new();
    let certificate = fixtures::PUBLIC_TEST_ROOT;
    let installation = scenario.prepare(
        "https",
        &fixtures::manifest_https(&["tenant-a"], Some(certificate)),
    );
    let bundle = installation.join("ca-bundle.pem");
    assert_eq!(
        fs::read_to_string(&bundle).unwrap(),
        certificate,
        "fixture: setup prepare wrote the bundle it accepted"
    );
    let args = ["--installation-dir", arg(&installation)];
    let accepted = scenario.diagnose_both(args);

    // The product's own grammar refuses each edit: setup prepare is the oracle.
    let not_ascii = certificate.replacen("MIIB", "MIIB\u{e9}", 1);
    let edits: Vec<(&str, String)> = vec![
        ("empty", String::new()),
        (
            "a key section",
            "-----BEGIN PRIVATE KEY-----\nAA==\n-----END PRIVATE KEY-----\n".to_owned(),
        ),
        ("nine certificates", certificate.repeat(9)),
        (
            "65537 bytes",
            format!("{certificate}{}", "\n".repeat(65_537 - certificate.len())),
        ),
        ("a non-ASCII byte", not_ascii),
    ];
    let mut refused = Vec::new();
    for (index, (name, pem)) in edits.iter().enumerate() {
        let product = scenario.try_prepare(
            &format!("refused-{index}"),
            &fixtures::manifest_https(&["tenant-a"], Some(pem.as_str())),
        );
        assert_ne!(
            product.code,
            Some(0),
            "the product refuses a bundle with {name}"
        );
        fixtures::overwrite(&bundle, pem);
        refused.push((*name, scenario.diagnose_both(args)));
    }
    fs::remove_file(&bundle).unwrap();
    let removed = scenario.diagnose_both(args);

    // A CA reference with the demonstration HTTP transport.
    let demo = scenario.prepare("demo", &fixtures::manifest_v2(&["tenant-a"]));
    fixtures::private_write(&demo.join("ca-bundle.pem"), certificate);
    fixtures::edit_json(&demo.join("oidc.json"), |oidc| {
        oidc["ca_bundle_file"] = "ca-bundle.pem".into();
    });
    let incompatible = scenario.diagnose_both(["--installation-dir", arg(&demo)]);
    let gateway = scenario.gateway(&demo.join("tenants.json"), Some(&demo.join("oidc.json")));
    assert_gateway_refuses(
        &gateway,
        "OIDC configuration is invalid",
        "a CA bundle with the demonstration transport",
    );

    // A CA reference that points at a runner token: never opened.
    let pointed = Scenario::new();
    let installation = pointed.prepare(
        "https",
        &fixtures::manifest_https(&["tenant-a"], Some(certificate)),
    );
    fixtures::edit_json(&installation.join("oidc.json"), |oidc| {
        oidc["ca_bundle_file"] = "tenants/tenant-a/runner".into();
    });
    let token_as_ca = observed_run(
        &pointed,
        &["--installation-dir", arg(&installation), "--json"],
    );

    let (text, json, report) = &accepted;
    let ok = finding(report, "trust.oidc_ca_bundle_accepted_by_grammar");
    assert_eq!(ok["status"], "ok");
    finding(report, "trust.certificate_validity_period_not_available");
    assert_eq!(report["identity"]["sso"]["ca_bundle_declared"], true);
    assert_eq!(report["identity"]["sso"]["transport"], "https_only");
    assert_eq!(
        report["trust"],
        json!([{"role": "oidc_ca_bundle", "display": "<installation>/ca-bundle.pem",
                "read": true, "certificate_count": 1}])
    );
    assert!(entries(&report["mode"], "files_read_for_content").contains(&json!("oidc_ca_bundle")));
    assert_eq!(json.code, 0);
    assert!(
        text.stdout
            .contains("  [ok] trust.oidc_ca_bundle_accepted_by_grammar\n")
    );
    // The first bytes of the certificate body and the body of the key
    // section: neither is in a report, accepted or refused, text or JSON.
    assert!(
        certificate.contains("MIIB"),
        "fixture: the certificate body"
    );
    let no_bundle_content = |name: &str, runs: [&Run; 2]| {
        for run in runs {
            assert!(
                !run.stdout.contains("AA==") && !run.stdout.contains("MIIB"),
                "{name}: the content of a bundle is never printed"
            );
        }
    };
    no_bundle_content("the accepted bundle", [text, json]);

    for (name, (text, json, report)) in &refused {
        assert_eq!(
            finding(report, "trust.ca_bundle_invalid")["status"],
            "error",
            "{name}"
        );
        assert!(find(report, "trust.oidc_ca_bundle_accepted_by_grammar").is_none());
        assert_eq!(json.code, 3, "{name}");
        no_bundle_content(name, [text, json]);
    }
    let (_, json, report) = &removed;
    finding(report, "trust.ca_unavailable_or_not_private");
    assert_eq!(json.code, 3);

    let (_, json, report) = &incompatible;
    finding(report, "trust.ca_incompatible_with_transport");
    assert_eq!(json.code, 3);

    let report = token_as_ca.run.json();
    finding(&report, "trust.ca_reference_is_a_credential_reference");
    assert!(find(&report, "trust.ca_bundle_invalid").is_none());
    assert_eq!(token_as_ca.run.code, 3);
    assert_never_read(
        &token_as_ca,
        "https/oidc.json",
        &["https/tenants/tenant-a/runner"],
        "a CA reference that points at a token",
    );
}

// ---- T17 ------------------------------------------------------------------

#[test]
fn urls_and_values_are_redacted() {
    let scenario = Scenario::new();
    let canaries: Vec<String> = (0..30)
        .map(|index| format!("{CANARY}-{index:02}"))
        .collect();
    let canary = |index: usize| canaries[index].as_str();
    let mut runs: Vec<(String, Run)> = Vec::new();
    let mut both = |context: &str, args: &[&str]| {
        let (text, json, report) = scenario.diagnose_both(args);
        runs.push((format!("{context} (text)"), text));
        runs.push((format!("{context} (JSON)"), json));
        report
    };

    // URLs given on the command line: user information, query and fragment.
    let with_password = format!(
        "https://user-{}:{}@engine.example.invalid/v1?key={}#{}",
        canary(0),
        canary(1),
        canary(2),
        canary(3)
    );
    let bare_user = format!("https://{}@engine.example.invalid/v1", canary(4));
    let unparseable = format!("https://[{}", canary(5));
    let remote = both("--jev-url with a password", &["--jev-url", &with_password]);
    let bare = both(
        "--jev-url with bare user information",
        &["--jev-url", &bare_user],
    );
    let local = both("--jev-local-url", &["--jev-local-url", &with_password]);
    let unparsed = both("an unparseable --jev-url", &["--jev-url", &unparseable]);

    // The same parts in every URL of an OIDC file.
    let installation = scenario.prepare("installation", &fixtures::manifest_v2(&["tenant-a"]));
    let oidc = installation.join("oidc.json");
    let pristine = fs::read(&oidc).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut oidc_reports = Vec::new();
    for (index, pointer) in [
        "/gateway/provider/issuer",
        "/gateway/provider/authorization_endpoint",
        "/gateway/provider/token_endpoint",
        "/gateway/provider/jwks_uri",
        "/gateway/provider/web_redirect_uri",
        "/gateway/provider/end_session_endpoint",
        "/gateway/web_origin",
    ]
    .into_iter()
    .enumerate()
    {
        fixtures::edit_json(&oidc, |file| {
            // The preparation declares no end-session endpoint: one is added
            // first, so that this field carries the same parts as the others.
            if pointer.ends_with("/end_session_endpoint") {
                file["gateway"]["provider"]["end_session_endpoint"] =
                    format!("{}/protocol/openid-connect/logout", fixtures::DEMO_ISSUER).into();
            }
            let url = file.pointer_mut(pointer).unwrap();
            let original = url.as_str().unwrap().replacen(
                "://",
                &format!("://user:{}@", canary(6 + index)),
                1,
            );
            *url = format!(
                "{original}?state={}#{}",
                canary(13 + index),
                canary(20 + index)
            )
            .into();
        });
        // The product's own validator says whether this edit is a refusal.
        let block: agentsandbox::api::OidcGatewayConfigV1 =
            serde_json::from_value(fixtures::read_json(&oidc)["gateway"].clone())
                .expect("fixture: the edited gateway block still decodes");
        let product_refuses = runtime.block_on(async { block.validate().is_err() });
        oidc_reports.push((
            pointer,
            product_refuses,
            both(pointer, &["--installation-dir", arg(&installation)]),
        ));
        fixtures::overwrite(&oidc, &pristine);
    }

    // An unknown key and a wrong-typed value in a decoded file.
    fixtures::edit_json(&oidc, |file| {
        file[canary(27)] = "unknown key".into();
    });
    let unknown_key = both(
        "an unknown key",
        &["--installation-dir", arg(&installation)],
    );
    fixtures::overwrite(&oidc, &pristine);
    fixtures::edit_json(&oidc, |file| {
        file["gateway"]["provider"]["issuer"] = json!({"wrong type": canary(28)});
    });
    let wrong_type = both(
        "a wrong-typed value",
        &["--installation-dir", arg(&installation)],
    );
    fixtures::overwrite(&oidc, &pristine);

    // A path component that looks like a credential, with and without paths.
    let secret_named = scenario.path(&format!("api_key={}", canary(29)));
    fixtures::private_dir(&secret_named);
    let hidden = both(
        "a credential-like path component",
        &["--config-dir", arg(&secret_named)],
    );
    let shown = both(
        "a credential-like path component with --show-paths",
        &["--config-dir", arg(&secret_named), "--show-paths"],
    );

    let all: Vec<&str> = canaries.iter().map(String::as_str).collect();
    for (context, run) in &runs {
        assert_no_canary(run, &all, context);
        assert_ne!(run.code, 1, "{context}: a report is written");
    }

    // The report still says which input is wrong, and shows the public part.
    assert_eq!(
        *input(&remote, "jev_url"),
        json!({"name": "jev_url", "origin": "cli", "display": "https://engine.example.invalid/v1"})
    );
    assert_eq!(
        input(&bare, "jev_url")["display"],
        "https://engine.example.invalid/v1"
    );
    assert_eq!(input(&local, "jev_local_url")["origin"], "cli");
    assert_eq!(input(&unparsed, "jev_url")["display"], "[unparseable URL]");
    // A path the filter would rewrite is masked whole in both modes: no
    // relative form is derived from it.
    for report in [&hidden, &shown] {
        assert_eq!(input(report, "config_dir")["display"], "[masked path]");
        assert_eq!(finding(report, "output.value_masked")["status"], "warning");
    }

    for report in [&remote, &bare, &local, &unparsed] {
        assert_eq!(
            finding(report, "trust.decision_transport_invalid")["status"],
            "error",
            "user information, a query and an unparseable URL are refused by the product"
        );
    }
    for (pointer, product_refuses, report) in &oidc_reports {
        if *product_refuses {
            let invalid = finding(report, "oidc.gateway_block_invalid");
            assert_eq!(invalid["details"]["kind"], "reason", "{pointer}");
            assert_eq!(
                invalid["scope"],
                json!({"kind": "file", "file": "oidc_file"})
            );
        } else {
            eprintln!(
                "OBSERVED the product accepts user information, a query and a fragment in {pointer}"
            );
            finding(report, "identity.sso_file_valid_offline");
            assert!(report["identity"]["sso"].is_object(), "{pointer}");
        }
    }
    assert_eq!(
        finding(&unknown_key, "oidc.json_invalid")["details"]["kind"],
        "json_error"
    );
    assert_eq!(
        finding(&wrong_type, "oidc.json_invalid")["details"]["kind"],
        "json_error"
    );
}

// ---- T18 ------------------------------------------------------------------

#[test]
fn identifiers_that_look_like_credentials() {
    let scenario = Scenario::new();
    let directory = scenario.path("registry");
    fixtures::private_dir(&directory);
    let plain = ["-api-key", "team-secret", "ops-token", "--token"];
    let masked = ["sk-abcdefghijkl", "sk-mnopqrstuvwx"];
    let tenants: Vec<&str> = plain.iter().chain(&masked).copied().collect();
    for id in &tenants {
        agentsandbox::tenancy::validate_tenant_id(id)
            .expect("fixture: the product accepts this tenant identifier");
    }
    let registry = fixtures::hand_written_registry(&directory, &tenants);

    let credentials = fixtures::provider_credentials(&scenario.path("credentials"));
    let providers = scenario.path("providers.json");
    fixtures::write_providers(
        &providers,
        &json!({"version": 1, "auto_priority": ["ops-token", "sk-abcdefghijkl"], "profiles": [
            fixtures::api_profile("ops-token", "codex", &credentials.codex_api),
            fixtures::api_profile("sk-abcdefghijkl", "claude_code", &credentials.claude_api)
        ]}),
    );
    agentsandbox::provider::load_config(&providers)
        .expect("fixture: the product accepts these profile identifiers");

    let (text, json, report) = scenario.diagnose_both([
        "--tenants-file",
        arg(&registry),
        "--providers-file",
        arg(&providers),
    ]);

    // A path longer than the display bound, with a credential-like component.
    let mut deep = scenario.path("deep");
    fixtures::private_dir(&deep);
    for component in ["a".repeat(240), "token=abc".to_owned(), "b".repeat(240)]
        .iter()
        .cycle()
        .take(9)
    {
        deep = deep.join(component);
        fixtures::private_dir(&deep);
    }
    assert!(arg(&deep).len() > 1024);
    let deep_registry = fixtures::hand_written_registry(&deep, &["tenant-a"]);
    let (deep_text, deep_json, deep_report) =
        scenario.diagnose_both(["--tenants-file", arg(&deep_registry), "--show-paths"]);

    for run in [&text, &json, &deep_text, &deep_json] {
        assert!(
            matches!(run.code, 0 | 3 | 4),
            "a legal identifier or path never withholds the report: exit {}, {}",
            run.code,
            run.stderr
        );
    }
    let id = |id: &str, masked: bool| json!({"id": id, "masked": masked});
    assert_eq!(
        report["tenants"]["registry"],
        json!([
            id("--token", false),
            id("-api-key", false),
            id("ops-token", false),
            id("[masked tenant identifier 1]", true),
            id("[masked tenant identifier 2]", true),
            id("team-secret", false)
        ]),
        "sorted by identifier; distinct placeholders; legal identifiers printed"
    );
    let profiles: Vec<&Value> = entries(&report["providers"], "profiles")
        .iter()
        .map(|profile| &profile["profile"])
        .collect();
    assert_eq!(
        profiles,
        [
            &id("ops-token", false),
            &id("[masked profile identifier 1]", true)
        ]
    );
    assert_eq!(finding(&report, "output.value_masked")["status"], "warning");
    assert!(text.stdout.contains("\"ops-token\"") && text.stdout.contains("\"--token\""));
    assert!(!text.stdout.contains("sk-abcdefghijkl") && !json.stdout.contains("sk-mnopqrstuvwx"));

    assert_eq!(deep_report["inputs"][0]["display"], "[masked path]");
    finding(&deep_report, "output.value_masked");
    assert_eq!(deep_report["tenants"]["registry_total"], 1);
    assert!(!deep_text.stdout.contains("token=abc") && !deep_json.stdout.contains("token=abc"));
}

// ---- T19 ------------------------------------------------------------------

#[test]
fn control_characters_cannot_forge_lines() {
    let scenario = Scenario::new();
    let directory = scenario.path("registry");
    fixtures::private_dir(&directory);
    let registry = fixtures::hand_written_registry(&directory, &["tenant-a"]);
    let forged_runner = "runner\n  [ok] forged.finding\n          a forged line";
    let forged_viewer = "viewer\u{1b}[2K\u{202e}desrever";
    fs::rename(
        directory.join("tenant-a/runner"),
        directory.join("tenant-a").join(forged_runner),
    )
    .unwrap();
    fs::rename(
        directory.join("tenant-a/viewer"),
        directory.join("tenant-a").join(forged_viewer),
    )
    .unwrap();
    fixtures::edit_json(&registry, |file| {
        file["tenants"][0]["runner_token_file"] = format!("tenant-a/{forged_runner}").into();
        file["tenants"][0]["viewer_token_file"] = format!("tenant-a/{forged_viewer}").into();
    });
    let gateway_reads_these_names = {
        let copy = scenario.path("copy");
        fixtures::copy_tree(&directory, &copy);
        scenario.gateway(&copy.join("tenants.json"), None)
    };
    assert!(
        gateway_reads_these_names.listening(),
        "fixture: such file names are legal references for the gateway: {}",
        gateway_reads_these_names.describe()
    );

    let hidden = scenario.diagnose(["--tenants-file", arg(&registry)]);
    let (text, json, report) =
        scenario.diagnose_both(["--tenants-file", arg(&registry), "--show-paths"]);

    for run in [&hidden, &text] {
        assert_ne!(run.code, 1, "a report is written: {}", run.stderr);
        assert!(
            run.stdout
                .chars()
                .all(|character| character == '\n' || !character.is_control()),
            "no raw control character reaches the text"
        );
        assert!(!run.stdout.contains('\u{202e}'));
        assert!(
            run.stdout
                .lines()
                .all(|line| !line.trim_start().starts_with("[ok] forged")
                    && line.trim() != "a forged line"),
            "a file name cannot forge a line"
        );
    }
    assert!(!json.stdout.contains('\u{1b}') && !json.stdout.contains('\u{202e}'));
    let displays: Vec<&str> = entries(&report, "credential_references")
        .iter()
        .map(|entry| text_of(entry, "display"))
        .collect();
    assert!(
        displays.iter().any(|display| display
            .ends_with("runner\\u{a}  [ok] forged.finding\\u{a}          a forged line")),
        "the newline is shown escaped: {displays:?}"
    );
    assert!(
        displays
            .iter()
            .any(|display| display.ends_with("viewer\\u{1b}[2K\\u{202e}desrever")),
        "the escape and the override are shown escaped: {displays:?}"
    );
    assert!(
        text.stdout
            .contains("runner\\u{a}  [ok] forged.finding\\u{a}")
    );
}

// ---- T20 ------------------------------------------------------------------

#[test]
fn hostile_environment_is_ignored() {
    let scenario = Scenario::new();
    let config = scenario.historical("config");
    // Profiles for both clients and both modes, so that the selection below
    // runs the provider code that a variable of a client could reach.
    let credentials = fixtures::provider_credentials(&scenario.path("credentials"));
    fixtures::write_providers(
        &config.join("providers.json"),
        &fixtures::four_profiles(&credentials),
    );
    let tools = scenario.path("path-entries");
    fixtures::private_dir(&tools);
    let markers: Vec<PathBuf> = ["docker", "podman"]
        .into_iter()
        .map(|name| fixtures::marker_program(&tools, name))
        .collect();
    let variables = [
        ("OPENAI_API_KEY", format!("{CANARY}-openai")),
        ("CODEX_API_KEY", format!("{CANARY}-codex")),
        ("ANTHROPIC_API_KEY", format!("{CANARY}-anthropic")),
        ("CLAUDE_CODE_OAUTH_TOKEN", format!("{CANARY}-claude")),
        ("CODEX_HOME", format!("/nonexistent/{CANARY}-codex-home")),
        ("HOME", format!("/nonexistent/{CANARY}-home")),
        (
            "DOCKER_HOST",
            format!("unix:///nonexistent/{CANARY}-docker"),
        ),
        (
            "DOCKER_CONFIG",
            format!("/nonexistent/{CANARY}-docker-config"),
        ),
        ("JEV_BACKEND", "typesafe".to_owned()),
        (
            "JEV_URL",
            format!("https://{CANARY}-jev.example.invalid/v1"),
        ),
        ("JEV_API_KEY_FILE", format!("/nonexistent/{CANARY}-jev-key")),
        ("JEV_MODEL", format!("{CANARY}-model")),
        ("JEV_CA_FILE", format!("/nonexistent/{CANARY}-jev-ca")),
        ("AI_GUARD_PROVIDER_BOOTSTRAP", format!("{CANARY}-bootstrap")),
        ("PATH", arg(&tools).to_owned()),
    ];
    // The first two passes give no engine program: the approval then looks at
    // the fixed locations of the product, while PATH names a directory that
    // holds programs called `docker` and `podman`. The four others resolve a
    // profile of the file and submit one of the marker programs to the
    // approval. The provider axis and both paths of the approval run under
    // the hostile variables.
    for (auth_mode, json) in [
        (None, false),
        (None, true),
        (Some("api"), false),
        (Some("api"), true),
        (Some("subscription"), false),
        (Some("subscription"), true),
    ] {
        let mut args = vec!["--config-dir", arg(&config)];
        if let Some(auth_mode) = auth_mode {
            args.extend(["--client", "codex", "--auth-mode", auth_mode]);
        }
        args.extend(["--runtime", "docker"]);
        if auth_mode.is_some() {
            args.extend(["--runtime-program", arg(&markers[0])]);
        }
        if json {
            args.push("--json");
        }
        let pass = match auth_mode {
            Some(auth_mode) => format!("--auth-mode {auth_mode}"),
            None => "the fixed locations of the engine".to_owned(),
        };
        let clean = scenario.diagnose(&args);
        if let (true, Some(auth_mode)) = (json, auth_mode) {
            // Calibration: the two axes are not idle in these runs.
            let report = clean.json();
            assert_eq!(
                report["providers"]["selection"]["resolved"]["profile"]["id"],
                format!("codex-{auth_mode}"),
                "the provider axis resolves a profile"
            );
            assert!(
                find(&report, "engine.program_approved").is_some()
                    || find(&report, "engine.program_refused").is_some(),
                "the engine approval judges the given program: {:?}",
                codes(&report)
            );
        } else if json {
            // Calibration: no program is given, and the approval still ran.
            // What it finds at the fixed locations depends on the host.
            let report = clean.json();
            assert!(
                entries(&report, "inputs")
                    .iter()
                    .all(|input| input["name"] != "runtime_program"),
                "no engine program is given in this pass"
            );
            let lookup = [
                "engine.program_absent_from_fixed_entries",
                "engine.program_approved",
                "engine.program_refused",
            ]
            .into_iter()
            .find(|code| find(&report, code).is_some())
            .unwrap_or_else(|| {
                panic!(
                    "the approval looks at the fixed locations: {:?}",
                    codes(&report)
                )
            });
            if lookup == "engine.program_approved" {
                assert_eq!(
                    finding(&report, lookup)["details"],
                    json!({"kind": "engine_program", "explicit_program": false})
                );
            }
            eprintln!("OBSERVED engine approval without a given program: {lookup}");
        }
        let before = tree_manifest::take(&scenario.root);
        let mut command = scenario.diagnose_command();
        command.args(&args).envs(variables.iter().cloned());
        let hostile = scenario.run_command(command);
        tree_manifest::assert_unchanged(
            &before,
            &tree_manifest::take(&scenario.root),
            "hostile environment",
        );
        assert_no_canary(&hostile, &[CANARY], "a variable of the environment");
        assert_eq!(
            (hostile.code, &hostile.stdout, &hostile.stderr),
            (clean.code, &clean.stdout, &clean.stderr),
            "the environment changes nothing in the report ({pass})"
        );
    }
    for marker in &markers {
        assert!(
            !fixtures::marker_was_executed(marker),
            "no program is taken from PATH"
        );
    }
}

// ---- T21 ------------------------------------------------------------------

#[test]
fn engine_program_is_approved_not_executed() {
    use agentsandbox::runtime::{Runtime, RuntimeKind};
    let scenario = Scenario::new();
    let directory = scenario.path("engine");
    fixtures::private_dir(&directory);
    let program = fixtures::marker_program(&directory, "docker");
    // A valid target, so that the exit code is decided by the engine axis.
    let config = scenario.historical("config");

    let (text, json, report) = scenario.diagnose_both([
        "--config-dir",
        arg(&config),
        "--runtime",
        "docker",
        "--runtime-program",
        arg(&program),
    ]);
    let (_, fixed_json, fixed_report) =
        scenario.diagnose_both(["--config-dir", arg(&config), "--runtime", "docker"]);
    assert!(
        !fixtures::marker_was_executed(&program),
        "the engine program is never executed"
    );

    // The product's own approval decides what the report must say here.
    let product_approves = Runtime::with_program(RuntimeKind::Docker, &program).is_ok();
    let product_finds_fixed_entry = Runtime::new(RuntimeKind::Docker).is_ok();
    eprintln!(
        "OBSERVED engine approval in this environment: explicit program {product_approves}, fixed entries {product_finds_fixed_entry}"
    );
    assert!(!fixtures::marker_was_executed(&program));

    finding(&report, "engine.state_not_checked");
    assert!(find(&report, "engine.program_not_inspected").is_none());
    if product_approves {
        finding(&report, "engine.program_approved");
        let engine = report["engine"]
            .as_object()
            .expect("an engine section for an approved program");
        assert_eq!(
            engine.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            BTreeSet::from(["entry_display", "kind", "policy_version", "sha256"]),
            "exactly four fields: the invocation binding is never serialised"
        );
        assert_eq!(engine["kind"], "docker");
        assert_eq!(engine["sha256"].as_str().unwrap().len(), 64);
        assert!(
            entries(&report["mode"], "files_read_for_content").contains(&json!("engine_program"))
        );
        assert_eq!(json.code, 0);
    } else {
        finding(&report, "engine.program_refused");
        assert_eq!(report["engine"], Value::Null);
        assert_eq!(json.code, 3);
    }
    // The account home is the one the approval binds, read from the binding
    // the product builds: it is not the HOME of this test process, which the
    // tool never reads.
    let bound_home = Runtime::with_program(RuntimeKind::Docker, &program)
        .ok()
        .and_then(|runtime| runtime.invocation().environment.get("HOME").cloned());
    match bound_home {
        Some(home) if home.len() >= 2 => {
            eprintln!(
                "OBSERVED account home bound by the approval: {} bytes",
                home.len()
            );
            for output in [&text.stdout, &json.stdout, &text.stderr, &json.stderr] {
                assert!(
                    !output.contains(&home),
                    "the home of the control account is never printed"
                );
            }
        }
        _ => oracle_not_run(
            "account-home",
            "the product bound no account home of two bytes or more in this environment",
        ),
    }
    assert!(!text.stdout.contains(arg(&scenario.root)));

    finding(&fixed_report, "engine.state_not_checked");
    if product_finds_fixed_entry {
        assert!(find(&fixed_report, "engine.program_absent_from_fixed_entries").is_none());
    } else {
        finding(&fixed_report, "engine.program_absent_from_fixed_entries");
        assert_eq!(fixed_json.code, 3);
    }
}

// ---- T22 ------------------------------------------------------------------

#[test]
fn no_network_by_default() {
    let scenario = Scenario::new();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let issuer = format!("http://{address}/realms/loopback");
    let installation = scenario.prepare(
        "installation",
        &fixtures::manifest_v1_with_issuer(&["tenant-a"], &issuer),
    );
    let engine = format!("http://{address}/v1/systemone");
    let (_, json, report) = scenario.diagnose_both([
        "--installation-dir",
        arg(&installation),
        "--jev-local-url",
        &engine,
        "--jev-allow-local-http",
    ]);
    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock),
        "nothing connects to the identity provider or to the decision engine"
    );
    assert_ne!(json.code, 1);
    assert_eq!(report["mode"]["network"], "none");
    assert_eq!(report["mode"]["active_checks_executed"], json!([]));
    assert_eq!(
        report["identity"]["sso"]["issuer"], issuer,
        "the OIDC file that names the listener is inspected"
    );
    assert_eq!(report["identity"]["sso"]["idp_contacted"], false);
    assert_eq!(
        finding(&report, "identity.idp_reachability_not_checked")["status"],
        "not_checked"
    );
    assert_eq!(
        report["inference"]["decision_engine_transport"]["url"], engine,
        "the decision-engine transport that names the listener is inspected"
    );
    finding(&report, "inference.decision_transport_valid");
    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
    );
}

// ---- increment A: the opt-in discovery of the identity provider ---------------

#[test]
fn active_oidc_discovery_loopback() {
    // The repository's controlled issuer, on the loopback of this machine.
    // Its server runs on the worker thread of this runtime while the test
    // thread drives the tool.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let issuer = runtime.block_on(controlled::ControlledIssuer::new());
    // The issuer now holds a token response: nothing the tool does may ask
    // for it, and none of it may reach a report.
    let access_token = issuer.login_response("nonce", "subject", "diagnose");
    let discovery = "/realm/.well-known/openid-configuration".to_owned();
    let keys = "/realm/keys".to_owned();
    let request = |path: &String| (path.clone(), String::new());

    let scenario = Scenario::new();
    // An installation prepared by the product for a provider block: the one
    // the issuer itself declares, after one stated edit.
    let prepare = |name: &str, provider: Value| {
        let mut manifest =
            fixtures::manifest_v1_with_issuer(&["tenant-a"], provider["issuer"].as_str().unwrap());
        manifest["gateway"]["provider"] = provider;
        scenario.prepare(name, &manifest)
    };
    let declared = serde_json::to_value(issuer.config()).unwrap();
    let installation = prepare("installation", declared.clone());
    assert!(
        issuer.requests().is_empty(),
        "the preparation contacts nothing"
    );
    let private_parts = [access_token.as_str(), "private-refresh-diagnose"];

    // Without the option: not checked, and nothing reaches the issuer.
    let (text, json, report) = scenario.diagnose_both(["--installation-dir", arg(&installation)]);
    assert!(
        issuer.requests().is_empty(),
        "without the option the tool opens no connection"
    );
    assert_eq!(report["mode"]["network"], "none");
    assert_eq!(report["mode"]["active_checks_executed"], json!([]));
    assert_eq!(report["identity"]["sso"]["issuer"], issuer.issuer);
    assert_eq!(report["identity"]["sso"]["idp_contacted"], false);
    let not_checked = finding(&report, "identity.idp_reachability_not_checked");
    assert_eq!(not_checked["status"], "not_checked");
    assert_eq!(
        text_of(not_checked, "requires"),
        "--active oidc-discovery, with an OIDC file that passes the offline rules"
    );
    assert!(text.stdout.contains(
        "mode       offline; nothing written; no file named as a credential was read; no network\n"
    ));
    assert!(text.stdout.contains("  identity provider  not contacted\n"));
    assert_eq!(json.code, 0);

    // With the option: the discovery document and the keys, once per run,
    // with an empty body, and nothing else. The report says what was done.
    let active = ["--active", "oidc-discovery"];
    let (text, json, report) = scenario.diagnose_both([
        "--installation-dir",
        arg(&installation),
        active[0],
        active[1],
    ]);
    let mut expected = vec![
        request(&discovery),
        request(&keys),
        request(&discovery),
        request(&keys),
    ];
    assert_eq!(
        issuer.requests(),
        expected,
        "two requests per run, the discovery document then the keys, none to the token endpoint, empty bodies"
    );
    assert_eq!(report["mode"]["network"], "active_checks_only");
    assert_eq!(
        report["mode"]["active_checks_executed"],
        json!(["oidc_discovery"])
    );
    assert_eq!(report["mode"]["mutation"], "none");
    assert_eq!(report["identity"]["sso"]["idp_contacted"], true);
    let answered = finding(&report, "active.oidc_discovery_ok");
    assert_eq!(answered["status"], "ok");
    assert_eq!(answered["basis"], "observed");
    assert_eq!(
        answered["scope"],
        json!({"kind": "file", "file": "oidc_file"})
    );
    assert!(find(&report, "identity.idp_reachability_not_checked").is_none());
    assert_eq!(
        *input(&report, "active"),
        json!({"name": "active", "origin": "cli", "display": "oidc-discovery"})
    );
    assert_round_trip(&json, "a report with an active check");
    assert_eq!(json.code, 0);
    assert!(text.stdout.contains(
        "mode       active checks run; nothing written; no file named as a credential was read\nactive     oidc discovery; network used by these checks only\n"
    ));
    assert!(
        text.stdout
            .contains("given      active check oidc-discovery (from --active)\n")
    );
    assert!(text.stdout.contains("  identity provider  contacted\n"));
    assert!(text.stdout.contains("  [ok] active.oidc_discovery_ok\n"));
    assert!(
        text_of(
            finding(&report, "topology.running_services_not_checked"),
            "message"
        )
        .contains("contacted the identity provider and nothing else"),
        "no finding of an active run says that nothing was contacted"
    );
    assert!(
        !text.stdout.contains("contacts nothing") && !text.stdout.contains("was not contacted"),
        "no text of an active run denies the contact"
    );
    for run in [&text, &json] {
        assert_no_canary(run, &private_parts, "a token the issuer holds");
    }

    // The same check against an issuer that keeps what it is sent. The
    // controlled issuer above keeps paths and bodies only; this one keeps
    // the request line and the header names of each request.
    let recorder = RecordingIssuer::start();
    let recorded = prepare(
        "recorded",
        serde_json::from_str(
            &declared
                .to_string()
                .replace(&issuer.issuer, &recorder.issuer),
        )
        .unwrap(),
    );
    assert!(
        recorder.requests().is_empty(),
        "the preparation contacts nothing"
    );
    let (_, json, report) =
        scenario.diagnose_both(["--installation-dir", arg(&recorded), active[0], active[1]]);
    assert_eq!(report["identity"]["sso"]["issuer"], recorder.issuer);
    finding(&report, "active.oidc_discovery_ok");
    assert_eq!(json.code, 0);
    let sent = recorder.requests();
    eprintln!("OBSERVED requests of the active check: {sent:?}");
    assert_eq!(
        sent.iter()
            .map(|request| request.line.as_str())
            .collect::<Vec<_>>(),
        [
            "GET /realm/.well-known/openid-configuration HTTP/1.1",
            "GET /realm/keys HTTP/1.1",
            "GET /realm/.well-known/openid-configuration HTTP/1.1",
            "GET /realm/keys HTTP/1.1",
        ],
        "two GET requests per run: the discovery document, then the keys"
    );
    for request in &sent {
        let mut names = request.header_names.clone();
        names.sort();
        assert_eq!(
            names,
            ["accept", "host"],
            "{}: no header carries a credential: no authorization, no cookie, nothing but these two",
            request.line
        );
        assert_eq!(request.body_bytes, 0, "{}: no body", request.line);
    }
    assert_eq!(issuer.requests(), expected, "the other issuer saw nothing");
    drop(recorder);

    // The file names another token endpoint than the provider announces: the
    // discovery document is fetched, it does not confirm the file, and the
    // keys are never asked for.
    let mut elsewhere = declared.clone();
    elsewhere["token_endpoint"] = format!("{}/token-elsewhere", issuer.issuer).into();
    let mismatched = prepare("mismatched", elsewhere);
    let (_, json, report) =
        scenario.diagnose_both(["--installation-dir", arg(&mismatched), active[0], active[1]]);
    expected.extend([request(&discovery), request(&discovery)]);
    assert_eq!(issuer.requests(), expected);
    let mismatch = finding(&report, "active.oidc_discovery_metadata_mismatch");
    assert_eq!(mismatch["status"], "error");
    assert_eq!(report["mode"]["network"], "active_checks_only");
    assert_eq!(report["identity"]["sso"]["idp_contacted"], true);
    assert!(find(&report, "active.oidc_discovery_ok").is_none());
    assert_eq!(json.code, 3);

    // Nothing listens at the issuer of the file: no usable answer, which is
    // an unknown from where the tool runs, not a refusal of the provider.
    // The port is held for the whole case by a socket that is bound and does
    // not listen: a connection to it is refused, and no other test of this
    // target can be given the port meanwhile.
    let held = {
        let _context = runtime.enter();
        let socket = tokio::net::TcpSocket::new_v4().unwrap();
        socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
        socket
    };
    let silent_issuer = format!("http://{}/realm", held.local_addr().unwrap());
    let silent = prepare(
        "silent",
        serde_json::from_str(&declared.to_string().replace(&issuer.issuer, &silent_issuer))
            .unwrap(),
    );
    let (_, json, report) =
        scenario.diagnose_both(["--installation-dir", arg(&silent), active[0], active[1]]);
    let unavailable = finding(&report, "active.oidc_discovery_transport_unavailable");
    assert_eq!(unavailable["status"], "unknown");
    assert_eq!(report["mode"]["network"], "active_checks_only");
    assert_eq!(json.code, 4);
    assert_eq!(issuer.requests(), expected, "the other issuer saw nothing");
    drop(held);

    // A file the offline rules refuse is never used to contact anything,
    // and the user information written in it is never printed.
    let refused = prepare("refused", declared.clone());
    fixtures::edit_json(&refused.join("oidc.json"), |file| {
        file["gateway"]["provider"]["issuer"] = issuer
            .issuer
            .replacen("://", &format!("://user:{CANARY}@"), 1)
            .into();
    });
    let (text, json, report) =
        scenario.diagnose_both(["--installation-dir", arg(&refused), active[0], active[1]]);
    assert_eq!(
        issuer.requests(),
        expected,
        "a refused file contacts nothing"
    );
    finding(&report, "oidc.gateway_block_invalid");
    let not_run = finding(&report, "identity.idp_reachability_not_checked");
    assert!(text_of(not_run, "message").contains("nothing was contacted"));
    assert_eq!(report["mode"]["network"], "none");
    assert_eq!(report["mode"]["active_checks_executed"], json!([]));
    assert_eq!(report["identity"]["sso"]["idp_contacted"], false);
    assert_eq!(json.code, 3);
    for run in [&text, &json] {
        assert_no_canary(run, &[CANARY], "user information of a refused issuer");
    }

    // The option without any OIDC file: said, and nothing is contacted.
    let config = scenario.historical("config");
    let (_, _, report) =
        scenario.diagnose_both(["--config-dir", arg(&config), active[0], active[1]]);
    let no_file = finding(&report, "identity.idp_reachability_not_checked");
    assert!(text_of(no_file, "message").contains("no OIDC file was inspected"));
    assert_eq!(no_file["scope"], json!({"kind": "host"}));
    assert_eq!(report["mode"]["network"], "none");
    assert_eq!(issuer.requests(), expected);

    // The closed set of the option: a value outside it is a usage error
    // that never repeats the value, and nothing is contacted.
    let usage = scenario.diagnose(["--installation-dir", arg(&installation), "--active", CANARY]);
    assert_eq!(usage.code, 2);
    assert!(usage.stdout.is_empty());
    assert_eq!(
        usage.stderr,
        "ai-guard-diagnose: an option has a value outside its closed set; run ai-guard-diagnose --help\n"
    );
    assert_eq!(issuer.requests(), expected);
}

// ---- T23 ------------------------------------------------------------------

#[test]
fn report_is_stable_and_bounded() {
    const MAX_DOCUMENT_BYTES: usize = 524_288;
    let scenario = Scenario::new();
    let directory = scenario.path("registry");
    fixtures::private_dir(&directory);
    let ids: Vec<String> = (0..300).map(|index| format!("tenant-{index:03}")).collect();
    let tenants: Vec<&str> = ids.iter().map(String::as_str).collect();
    let registry = fixtures::hand_written_registry(&directory, &tenants);
    // The defect is in the tenant that sorts last: a bound on what is printed
    // must never be a bound on what is checked.
    fs::remove_file(directory.join("tenant-299/runner")).unwrap();

    let args = ["--tenants-file", arg(&registry), "--json"];
    let first = scenario.diagnose(args);
    let second = scenario.diagnose(args);
    let text = scenario.diagnose(["--tenants-file", arg(&registry)]);
    let text_again = scenario.diagnose(["--tenants-file", arg(&registry)]);

    // The same registry below a long path, with absolute paths shown.
    let mut deep = scenario.path("deep");
    fixtures::private_dir(&deep);
    for _ in 0..3 {
        deep = deep.join("d".repeat(240));
        fixtures::private_dir(&deep);
    }
    let deep_registry = fixtures::hand_written_registry(&deep, &tenants);
    fs::remove_file(deep.join("tenant-299/runner")).unwrap();
    let shown = scenario.diagnose([
        "--tenants-file",
        arg(&deep_registry),
        "--show-paths",
        "--json",
    ]);

    for run in [&first, &second, &text, &text_again, &shown] {
        assert_eq!(
            run.code, 3,
            "the absent key of the last tenant is an error; stderr: {}",
            run.stderr
        );
        assert!(run.stdout.len() <= MAX_DOCUMENT_BYTES, "within the bound");
    }
    assert_eq!(first.stdout, second.stdout, "two runs are byte-identical");
    assert_eq!(text.stdout, text_again.stdout);

    // The two forms of a bounded report, one run each: lists cut at their own
    // bounds, and a document emptied of its details. Which run takes which
    // form is pinned: a size change that moved both to one form would leave
    // the other unverified.
    for (context, run, emptied) in [
        ("placeholders", &first, false),
        ("absolute paths", &shown, true),
    ] {
        let report = run.json();
        assert_eq!(
            report["summary"]["truncated"] == true,
            emptied,
            "{context}: {} bytes; adjust the size of the fixture, never this pin",
            run.stdout.len()
        );
        let absent = finding(&report, "reference.absent");
        assert_eq!(absent["subjects"]["total"], 1, "{context}");
        assert_eq!(
            absent["subjects"]["ids"],
            json!([{"id": "tenant-299", "masked": false}]),
            "{context}"
        );
        assert_eq!(report["tenants"]["registry_total"], 300, "{context}");
        assert_eq!(report["topology"]["tenant_count"], 300, "{context}");
        let omitted = &report["summary"]["omitted"];
        let listed = |section: &str| entries(&report, section).len() as u64;
        if report["summary"]["truncated"] == true {
            finding(&report, "output.detail_omitted");
            assert_eq!(listed("credential_references"), 0, "{context}");
            assert_eq!(omitted["reference_entries"], 600, "{context}");
            assert_eq!(omitted["data_entries"], 300, "{context}");
            assert_eq!(omitted["tenant_ids"], 300, "{context}");
        } else {
            let references = listed("credential_references");
            assert!(references <= 512, "{context}");
            assert_eq!(
                references + omitted["reference_entries"].as_u64().unwrap(),
                600,
                "{context}: every reference is listed or counted"
            );
            assert_eq!(
                listed("data") + omitted["data_entries"].as_u64().unwrap(),
                300,
                "{context}"
            );
            assert_eq!(
                entries(&report["tenants"], "registry").len(),
                256,
                "{context}"
            );
            assert_eq!(omitted["tenant_ids"], 44, "{context}");
            // The entry tied to the error is printed first.
            assert_eq!(
                entries(&report, "credential_references")[0]["presence"],
                "absent",
                "{context}"
            );
        }
    }

    // The text report says what it does not list: the entries it prints and
    // the counts of its `omitted` line account for every reference and for
    // every store.
    let printed = |titles: &[&str]| {
        text.stdout
            .lines()
            .filter(|line| titles.iter().any(|title| line.starts_with(title)))
            .count() as u64
    };
    let omitted_line = text
        .stdout
        .lines()
        .find_map(|line| line.strip_prefix("omitted    "))
        .expect("the text report says that entries are not listed");
    let omitted: Vec<u64> = omitted_line
        .split(", ")
        .map(|part| {
            part.split(' ')
                .next()
                .and_then(|count| count.parse().ok())
                .unwrap_or_else(|| panic!("a count starts each part of {omitted_line:?}"))
        })
        .collect();
    assert_eq!(
        omitted_line.split(", ").collect::<Vec<_>>(),
        [
            format!("{} reference entries", omitted[0]),
            format!("{} data entries", omitted[1]),
            format!("{} tenant identifiers", omitted[2])
        ],
        "the line names what each count is"
    );
    assert!(omitted[0] > 0, "the fixture exceeds the bound of a list");
    assert_eq!(
        printed(&["  runner of tenant ", "  viewer of tenant "]) + omitted[0],
        600,
        "every reference is printed or counted as not listed"
    );
    assert_eq!(
        printed(&["  tenant store of tenant "]) + omitted[1],
        300,
        "every store is printed or counted as not listed"
    );
}

// ---- T24 ------------------------------------------------------------------

#[test]
fn refusals_and_usage() {
    let scenario = Scenario::new();
    let usage =
        |sentence: &str| format!("ai-guard-diagnose: {sentence}; run ai-guard-diagnose --help\n");

    // A parent traversal in an argument path: a refusal, no report at all.
    for args in [
        vec!["--config-dir", "../outside"],
        vec!["--installation-dir", "a/../../b", "--json"],
        vec!["--tenants-file", "../tenants.json"],
        vec![
            "--runtime",
            "docker",
            "--runtime-program",
            "/usr/../bin/docker",
        ],
    ] {
        let run = scenario.diagnose(&args);
        assert_eq!(run.code, 1, "{args:?}");
        assert!(run.stdout.is_empty(), "a refusal writes no partial report");
        assert_eq!(run.stderr, format!("{}\n", DiagnoseError::InvalidInput));
    }

    // Usage errors: one closed sentence, never clap's echo of the argument.
    let cases: [(&[&str], &str); 9] = [
        (
            &["--no-such-option"],
            "unknown option or unexpected argument",
        ),
        (&[CANARY], "unknown option or unexpected argument"),
        (
            &["--runtime", CANARY],
            "an option has a value outside its closed set",
        ),
        (
            &["--max-events", "0"],
            "an option has a value outside its closed set",
        ),
        (
            &["--max-events", CANARY],
            "an option has a value outside its closed set",
        ),
        (
            &["--jev-url", CANARY, "--jev-local-url", CANARY],
            "invalid command line",
        ),
        (&["--runtime-program", CANARY], "invalid command line"),
        (
            &["--client", CANARY],
            "an option has a value outside its closed set",
        ),
        (
            &["--auth-mode", CANARY],
            "an option has a value outside its closed set",
        ),
    ];
    for (args, sentence) in cases {
        let run = scenario.diagnose(args);
        assert_eq!(run.code, 2, "{args:?}");
        assert!(run.stdout.is_empty());
        assert_eq!(run.stderr, usage(sentence), "{args:?}");
        assert_no_canary(&run, &[CANARY], "usage error");
    }

    // Help and version are available before inspection, on stdout.
    let help = scenario.diagnose(["--help"]);
    assert_eq!(help.code, 0);
    assert!(help.stderr.is_empty());
    for flag in [
        "--config-dir",
        "--installation-dir",
        "--tenants-file",
        "--oidc-config",
        "--providers-file",
        "--client",
        "--auth-mode",
        "--client-profile",
        "--model",
        "--max-events",
        "--jev-url",
        "--jev-local-url",
        "--jev-allow-local-http",
        "--jev-ca-file",
        "--gateway-ca-file",
        "--runtime",
        "--runtime-program",
        "--json",
        "--show-paths",
    ] {
        assert!(help.stdout.contains(flag), "--help lists {flag}");
    }
    assert!(
        !help.stdout.contains("[env:"),
        "no option reads the environment"
    );
    let version = scenario.diagnose(["--version"]);
    assert_eq!(version.code, 0);
    assert!(version.stderr.is_empty());
    // The build_identity_cli target checks the separate provenance lines.
    assert_eq!(
        version.stdout.lines().next(),
        Some(concat!("ai-guard-diagnose ", env!("CARGO_PKG_VERSION")))
    );
}

// ---- T25 ------------------------------------------------------------------

/// The words that follow each `agentsandbox` in a text, and its options.
fn mentions(text: &str) -> (Vec<Vec<String>>, BTreeSet<String>) {
    let clean = |word: &str| {
        word.trim_matches(|character: char| {
            !(character.is_ascii_alphanumeric() || character == '-')
        })
        .to_owned()
    };
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut commands = Vec::new();
    let mut flags = BTreeSet::new();
    for (index, raw) in words.iter().enumerate() {
        let word = clean(raw);
        if word.starts_with("--") {
            flags.insert(word);
        } else if raw.starts_with("agentsandbox") && word == "agentsandbox" {
            // The directory `.agentsandbox` is not the command.
            commands.push(
                words[index + 1..]
                    .iter()
                    .map(|word| clean(word))
                    .take_while(|word| {
                        !word.is_empty()
                            && word
                                .chars()
                                .all(|character| character.is_ascii_lowercase() || character == '-')
                            && !word.starts_with('-')
                    })
                    .take(3)
                    .collect(),
            );
        }
    }
    (commands, flags)
}

/// Verdicts drawn from the read rule and the envelopes that this tool
/// restates from the gateway loader, not from a product function.
const RESTATED_ENVELOPE_VERDICTS: [&str; 11] = [
    "registry.file_unreadable",
    "registry.json_invalid",
    "registry.version_unknown",
    "registry.empty",
    "registry.tenant_id_duplicate",
    "registry.reference_empty",
    "registry.offline_rules_passed",
    "oidc.json_invalid",
    "oidc.version_plan_pair_invalid",
    "oidc.reference_invalid",
    "identity.sso_file_valid_offline",
];

#[test]
fn catalogue_is_safe_and_actionable() {
    let scenario = Scenario::new();
    let filter = Redactor::new(Vec::new());
    let mut seen = BTreeSet::new();
    let mut help: BTreeMap<Vec<&str>, String> = BTreeMap::new();
    let tool_help = scenario.diagnose(["--help"]).stdout;

    for entry in CATALOGUE {
        let code = entry.code.as_str();
        assert!(seen.insert(code), "{code} is declared once");
        assert_eq!(entry.code.entry().code, entry.code, "{code}: table order");
        assert_eq!(
            serde_json::to_value(entry.code).unwrap(),
            json!(code),
            "{code}: serialised form"
        );
        assert!(
            code.split_once('.')
                .is_some_and(|(family, name)| !family.is_empty()
                    && name
                        .chars()
                        .all(|character| character.is_ascii_lowercase() || character == '_')),
            "{code}: dotted lower-case code"
        );

        // A status decides whether `requires` exists.
        assert_eq!(
            entry.requires.is_some(),
            entry.status == Status::NotChecked,
            "{code}: only a not_checked item names what resolves it"
        );
        assert!(
            !entry.limit || entry.status == Status::NotAvailable,
            "{code}: a limit of the build is not_available"
        );
        assert!(
            !entry.message.is_empty() && !entry.action.is_empty(),
            "{code}"
        );
        // What is concluded from the registry and OIDC envelopes restated in
        // this tool is inferred, never observed: the gateway loader itself
        // did not run.
        if RESTATED_ENVELOPE_VERDICTS.contains(&code) {
            assert_eq!(
                entry.basis,
                Basis::Inferred,
                "{code}: a verdict of a restated envelope"
            );
        }

        // Static texts are fixpoints of the output filter, alone and in context.
        for text in entry.texts() {
            for candidate in [text.to_owned(), format!("{text} x"), format!("x {text}")] {
                assert_eq!(
                    filter.text(&candidate),
                    candidate,
                    "{code}: the output filter rewrites a static text"
                );
            }
            assert!(text.is_ascii(), "{code}: ASCII punctuation only");
            assert!(
                !text.contains("migrate-control") && !text.contains("migrate-authentication"),
                "{code}: a migration is never proposed as an inspection"
            );
        }
        assert!(
            !entry.message.contains("{target}")
                && entry.requires.is_none_or(|text| !text.contains("{target}")),
            "{code}: only an action names the operator's input"
        );

        // No action weakens a control.
        for action in entry.actions() {
            let lower = action.to_ascii_lowercase();
            for forbidden in ["--allow", "insecure", "disable", "skip"] {
                assert!(
                    !lower.contains(forbidden),
                    "{code}: an action must not suggest to {forbidden}"
                );
            }
        }

        // Every command and option a text names exists, and is declared.
        let mut named_commands = Vec::new();
        let mut named_flags = BTreeSet::new();
        for text in entry.texts() {
            let (commands, flags) = mentions(text);
            named_commands.extend(commands);
            named_flags.extend(flags);
        }
        let declared_flags: BTreeSet<String> = entry
            .commands
            .iter()
            .flat_map(|command| command.flags.iter().map(|flag| (*flag).to_owned()))
            .collect();
        assert_eq!(
            named_flags, declared_flags,
            "{code}: the options its texts name are exactly the declared ones"
        );
        for named in &named_commands {
            assert!(
                entry.commands.iter().any(|command| {
                    command.program == Program::Product
                        && !command.subcommand.is_empty()
                        && named.len() >= command.subcommand.len()
                        && command
                            .subcommand
                            .iter()
                            .zip(named)
                            .all(|(declared, named)| declared == named)
                }),
                "{code}: the command agentsandbox {named:?} is not declared"
            );
        }
        for command in entry.commands {
            let output = match command.program {
                Program::Diagnose => {
                    assert!(command.subcommand.is_empty(), "{code}");
                    &tool_help
                }
                Program::Product => {
                    assert!(
                        named_commands
                            .iter()
                            .any(|named| named.len() >= command.subcommand.len()
                                && command
                                    .subcommand
                                    .iter()
                                    .zip(named)
                                    .all(|(declared, named)| declared == named)),
                        "{code}: the declared command {:?} is named by no text",
                        command.subcommand
                    );
                    &*help.entry(command.subcommand.to_vec()).or_insert_with(|| {
                        let mut args = command.subcommand.to_vec();
                        args.push("--help");
                        let output = scenario.product(&args);
                        assert_eq!(
                            output.code,
                            Some(0),
                            "agentsandbox {:?} answers --help",
                            command.subcommand
                        );
                        output.stdout_text()
                    })
                }
            };
            for flag in command.flags {
                assert!(
                    output
                        .split(|character: char| character.is_whitespace() || character == ',')
                        .any(|word| word == *flag),
                    "{code}: {:?} does not list {flag}",
                    command.subcommand
                );
            }
        }
    }

    // Every code is documented in both guides.
    for (guide, text) in GUIDES {
        for entry in CATALOGUE {
            assert!(
                text.contains(&format!("`{}`", entry.code.as_str())),
                "{guide} does not document {}",
                entry.code.as_str()
            );
        }
    }
}

// ---- T26 ------------------------------------------------------------------

fn assert_round_trip(run: &Run, context: &str) -> Value {
    let value = run.json();
    let typed: ReportV1 = serde_json::from_value(value.clone())
        .unwrap_or_else(|error| panic!("{context}: the output is not a ReportV1: {error}"));
    assert_eq!(
        serde_json::to_value(&typed).unwrap(),
        value,
        "{context}: decoding and encoding again gives the same document"
    );
    assert_eq!(typed.summary.exit_code, run.code, "{context}");
    value
}

#[test]
fn json_contract_round_trips() {
    let scenario = Scenario::new();
    let first_run = scenario.diagnose(["--json"]);
    assert_round_trip(&first_run, "first run");

    let config = scenario.historical("config");
    let credentials = fixtures::provider_credentials(&scenario.path("credentials"));
    fixtures::write_providers(
        &config.join("providers.json"),
        &fixtures::four_profiles(&credentials),
    );
    fs::remove_file(&credentials.claude_subscription).unwrap();
    let manifest = fixtures::manifest_v2(&["tenant-a", "tenant-b"]);
    let installation = scenario.prepare("installation", &manifest);
    let mismatch = scenario.prepare("mismatch", &manifest);
    fixtures::edit_json(&mismatch.join("tenants.json"), |registry| {
        registry["tenants"].as_array_mut().unwrap().pop();
    });
    // An engine program the product approves, and two bundles it accepts:
    // the engine section and the trust entries are then filled.
    let engine = scenario.path("engine");
    fixtures::private_dir(&engine);
    let program = engine.join("docker");
    fixtures::write_mode(&program, "not a program", 0o755);
    agentsandbox::runtime::Runtime::with_program(
        agentsandbox::runtime::RuntimeKind::Docker,
        &program,
    )
    .expect("fixture: the product approves this program, so the report has an engine section");
    let bundle = scenario.path("bundle.pem");
    fixtures::private_write(&bundle, fixtures::PUBLIC_TEST_ROOT);

    let historical = assert_round_trip(
        &scenario.diagnose(["--config-dir", arg(&config), "--json"]),
        "historical directory with provider profiles",
    );
    let prepared = assert_round_trip(
        &scenario.diagnose(["--installation-dir", arg(&installation), "--json"]),
        "prepared installation",
    );
    let mismatched = assert_round_trip(
        &scenario.diagnose(["--installation-dir", arg(&mismatch), "--json"]),
        "tenant set mismatch",
    );
    let combined = assert_round_trip(
        &scenario.diagnose([
            "--installation-dir",
            arg(&installation),
            "--config-dir",
            arg(&config),
            "--runtime",
            "docker",
            "--runtime-program",
            arg(&program),
            "--jev-url",
            "https://engine.example.invalid/v1",
            "--jev-ca-file",
            arg(&bundle),
            "--gateway-ca-file",
            arg(&bundle),
            "--show-paths",
            "--json",
        ]),
        "every axis at once",
    );

    // The round trip must cover filled sections, not only the frame.
    assert_eq!(entries(&historical["providers"], "profiles").len(), 4);
    finding(&historical, "provider.unselected_profile_reference_absent");
    assert_eq!(
        historical["providers"]["selection"]["resolved"]["chosen_by"],
        "auto"
    );
    assert!(prepared["identity"]["sso"].is_object());
    assert!(!entries(&prepared, "credential_references").is_empty());
    assert!(!entries(&prepared, "data").is_empty());
    assert_eq!(
        finding(&mismatched, "tenants.plan_set_mismatch")["details"]["kind"],
        "tenant_set_difference"
    );
    assert!(combined["inference"]["decision_engine_transport"].is_object());
    assert!(combined["topology"].is_object());
    // The typed decoding of an approved engine and of trust entries.
    assert_eq!(combined["engine"]["kind"], "docker");
    assert_eq!(text_of(&combined["engine"], "sha256").len(), 64);
    assert_eq!(
        finding(&combined, "engine.program_approved")["details"],
        json!({"kind": "engine_program", "explicit_program": true})
    );
    assert_eq!(
        entries(&combined, "trust")
            .iter()
            .map(|entry| (&entry["role"], &entry["read"], &entry["certificate_count"]))
            .collect::<Vec<_>>(),
        [
            (&json!("decision_engine_ca_bundle"), &json!(true), &json!(1)),
            (&json!("gateway_ca_bundle"), &json!(true), &json!(1))
        ]
    );
}

// ---- T27 ------------------------------------------------------------------

#[test]
fn no_mutation_everywhere() {
    let scenario = Scenario::new();

    // Control 1: a file created in the tree is seen.
    let before = tree_manifest::take(&scenario.root);
    fixtures::private_write(&scenario.work.join("created"), "x");
    let created = tree_manifest::differences(&before, &tree_manifest::take(&scenario.root));
    assert!(created.contains(&"created: work/created".to_owned()));
    fs::remove_file(scenario.work.join("created")).unwrap();

    // Control 2: a file created and then removed directly in the root is seen
    // through the timestamps of the root entry itself.
    let before = tree_manifest::take(&scenario.root);
    fixtures::private_write(&scenario.path("transient"), "x");
    fs::remove_file(scenario.path("transient")).unwrap();
    let transient = tree_manifest::differences(&before, &tree_manifest::take(&scenario.root));
    assert_eq!(transient, ["changed: "]);

    // Control 3: the lock file left by a real store open is seen.
    let installation = scenario.prepare("installation", &fixtures::manifest_v2(&["tenant-a"]));
    let scratch = scenario.path("scratch");
    fixtures::copy_tree(&installation.join("tenants"), &scratch);
    let before = tree_manifest::take(&scenario.root);
    drop(
        agentsandbox::audit::Store::open_existing_for_tenant(
            &scratch.join("tenant-a/audit.sqlite"),
            "tenant-a",
            Redactor::new(Vec::new()),
            100_000,
        )
        .expect("control: the product opens a copy of a prepared store"),
    );
    let opened = tree_manifest::differences(&before, &tree_manifest::take(&scenario.root));
    eprintln!("OBSERVED trace of a real store open and close: {opened:?}");
    assert!(
        !opened.is_empty()
            && opened
                .iter()
                .all(|difference| difference.contains("scratch/tenant-a")),
        "a real store open leaves a trace this oracle sees, in the directory of the store: {opened:?}"
    );

    // Every scenario of this suite runs the tool through `Scenario::diagnose`,
    // which compares the manifest. The same oracle here on one tree that holds
    // every kind of installation at once, in text and in JSON.
    let config = scenario.historical_with_decision_engine_key("config");
    let credentials = fixtures::provider_credentials(&scenario.path("credentials"));
    fixtures::write_providers(
        &config.join("providers.json"),
        &fixtures::four_profiles(&credentials),
    );
    let registry_directory = scenario.path("registry");
    fixtures::private_dir(&registry_directory);
    let registry = fixtures::hand_written_registry(&registry_directory, &["tenant-a", "tenant-b"]);
    let https = scenario.prepare(
        "https",
        &fixtures::manifest_https(&["tenant-a"], Some(fixtures::PUBLIC_TEST_ROOT)),
    );
    for args in [
        vec!["--config-dir", arg(&config)],
        vec!["--installation-dir", arg(&installation)],
        vec![
            "--installation-dir",
            arg(&https),
            "--config-dir",
            arg(&config),
        ],
        vec!["--tenants-file", arg(&registry)],
        vec![
            "--tenants-file",
            arg(&registry),
            "--oidc-config",
            arg(&installation.join("oidc.json")),
            "--providers-file",
            arg(&config.join("providers.json")),
            "--gateway-ca-file",
            arg(&https.join("ca-bundle.pem")),
            "--jev-ca-file",
            arg(&https.join("ca-bundle.pem")),
            "--jev-url",
            "https://engine.example.invalid/v1",
            "--runtime",
            "docker",
            "--max-events",
            "64",
            "--show-paths",
        ],
    ] {
        let (text, json, _) = scenario.diagnose_both(&args);
        assert_ne!(text.code, 1, "{args:?}: {}", text.stderr);
        assert_ne!(json.code, 2, "{args:?}: {}", json.stderr);
    }
    for path in [
        scratch.join("tenant-a/.ai-guard-sqlite-open.lock"),
        installation.join("tenants/tenant-a/.ai-guard-sqlite-open.lock"),
        installation.join(".ai-guard-sqlite-open.lock"),
    ] {
        eprintln!("OBSERVED {}: exists {}", path.display(), path.exists());
    }
}

// ---- beyond the table: posture of the target ------------------------------

#[test]
fn unsafe_target_posture_is_reported_not_repaired() {
    let scenario = Scenario::new();
    let config = scenario.historical("config");
    fs::set_permissions(&config, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        agentsandbox::control_fs::Directory::open_existing(&config).is_err(),
        "the product refuses a config directory that is not private"
    );
    let (text, json, report) = scenario.diagnose_both(["--config-dir", arg(&config)]);
    let refused = finding(&report, "layout.posture_refused");
    assert_eq!(refused["status"], "error");
    assert!(
        entries(&report, "credential_references").is_empty(),
        "nothing below a refused directory is described"
    );
    assert_eq!(json.code, 3);
    assert!(text.stdout.contains("  [error] layout.posture_refused\n"));
    assert_eq!(
        fs::metadata(&config).unwrap().permissions().mode() & 0o7777,
        0o755,
        "the tool never repairs a directory"
    );
}

// ---- beyond the table: wrong option, staging names, a file given alone -----

#[test]
fn layout_edge_states() {
    let scenario = Scenario::new();

    // A historical directory given as a prepared installation: an error, as
    // a prepared installation given as a config directory is.
    let historical = scenario.historical("historical");
    let (text, json, report) = scenario.diagnose_both(["--installation-dir", arg(&historical)]);
    assert_eq!(report["layouts"][0]["kind"], "historical_config_dir");
    let wrong_option = finding(&report, "layout.historical_passed_as_installation_dir");
    assert_eq!(wrong_option["status"], "error");
    assert!(text_of(wrong_option, "action").contains("--config-dir"));
    assert!(
        entries(&report, "credential_references").is_empty(),
        "a directory given through the wrong option is not described"
    );
    assert_eq!(json.code, 3);
    assert!(
        text.stdout
            .contains("  [error] layout.historical_passed_as_installation_dir\n")
    );

    // A file is not a directory.
    let file = scenario.path("a-file");
    fixtures::private_write(&file, "not a directory");
    let (_, json, report) = scenario.diagnose_both(["--config-dir", arg(&file)]);
    assert!(
        text_of(finding(&report, "layout.posture_refused"), "message").contains("not a directory")
    );
    assert_eq!(report["layouts"], json!([]));
    assert_eq!(json.code, 3);

    // An installation directory brings its own registry and OIDC file: the
    // command line refuses another one beside it.
    for flag in ["--tenants-file", "--oidc-config"] {
        let run = scenario.diagnose(["--installation-dir", arg(&historical), flag, arg(&file)]);
        assert_eq!(run.code, 2, "{flag}");
        assert!(run.stdout.is_empty(), "{flag}");
        assert_eq!(
            run.stderr, "ai-guard-diagnose: invalid command line; run ai-guard-diagnose --help\n",
            "{flag}"
        );
    }

    // A directory with a staging name and no preparation record.
    let without_intent = scenario.path(".setup-0123456789abcdef01234567");
    fixtures::private_dir(&without_intent);
    let (_, json, report) = scenario.diagnose_both(["--installation-dir", arg(&without_intent)]);
    assert_eq!(
        report["layouts"],
        json!([{"input": "installation_dir", "kind": "setup_staging_directory",
                "preparation": "no_intent", "oidc_file_version": null}])
    );
    let no_intent = finding(&report, "layout.staging_without_intent");
    assert!(!text_of(no_intent, "action").contains("--recovery-dir"));
    assert_eq!(json.code, 3);

    // A destination may itself carry a staging name. Its registry records
    // paths inside it: it is a published installation, not a staging directory.
    let named = Scenario::new();
    let destination = named.prepare(
        ".setup-89abcdef0123456789abcdef",
        &fixtures::manifest_v2(&["tenant-a"]),
    );
    let (_, json, report) = named.diagnose_both(["--installation-dir", arg(&destination)]);
    assert_eq!(report["layouts"][0]["kind"], "prepared_installation");
    assert_eq!(report["layouts"][0]["preparation"], "complete");
    for staging in [
        "layout.setup_staging_directory",
        "layout.staging_and_final_both_exist",
        "layout.references_outside_inspected_root",
    ] {
        assert!(find(&report, staging).is_none(), "{staging}");
    }
    assert_eq!(entries(&report, "data").len(), 2);
    assert_eq!(json.code, 0);

    // An OIDC file given alone: SSO is inspected, its store is described,
    // and the tenant set is not compared with a registry nobody gave.
    let (_, json, report) =
        named.diagnose_both(["--oidc-config", arg(&destination.join("oidc.json"))]);
    assert!(report["identity"]["sso"].is_object());
    assert_eq!(report["layouts"], json!([]));
    assert_eq!(report["tenants"]["registry"], Value::Null);
    assert_eq!(report["tenants"]["plan_total"], 1);
    assert_eq!(report["tenants"]["correspondence"], Value::Null);
    assert_eq!(
        finding(&report, "tenants.correspondence_not_checked")["status"],
        "not_checked"
    );
    assert_eq!(
        entries(&report, "data")[0]["display"],
        "<oidc-config-dir>/authentication.sqlite"
    );
    assert_eq!(json.code, 0);
}

// ---- beyond the table: where the profile file is, and what it may be --------

#[test]
fn provider_file_edge_states() {
    let scenario = Scenario::new();
    let config = scenario.historical("config");
    let credentials = fixtures::provider_credentials(&scenario.path("credentials"));
    let one = json!({"version": 1, "profiles": [
        fixtures::api_profile("only", "codex", &credentials.codex_api)
    ]});
    let provider_reference = |report: &Value| {
        entries(report, "credential_references")
            .iter()
            .find(|entry| entry["role"] == "provider")
            .cloned()
            .expect("the reference of the profile is listed")
    };

    // The profile file of the config directory is the default of a launch:
    // it is listed as assumed, and its credential is shown by kind and rank.
    fixtures::write_providers(&config.join("providers.json"), &one);
    let (text, json, report) = scenario.diagnose_both(["--config-dir", arg(&config)]);
    assert_eq!(
        *input(&report, "providers_file"),
        json!({"name": "providers_file", "origin": "default", "display": "<config>/providers.json"})
    );
    assert_eq!(
        report["providers"]["file"],
        json!({"state": "loaded", "display": "<config>/providers.json"})
    );
    assert_eq!(
        finding(&report, "provider.configuration_valid")["scope"],
        json!({"kind": "file", "file": "provider_profiles"})
    );
    assert!(
        entries(&report["mode"], "files_read_for_content").contains(&json!("provider_profiles"))
    );
    let reference = provider_reference(&report);
    assert_eq!(
        reference["owner"],
        json!({"kind": "provider_profile", "id": "only", "masked": false})
    );
    assert_eq!(
        reference["display"],
        "provider reference #1 (api_env_file, outside the directory of the profile file)"
    );
    assert_eq!(reference["location"], "outside_root");
    assert_eq!(reference["consumer"], "private_file_reader");
    assert_eq!(reference["private_rule"], "satisfied");
    assert_eq!(reference["content"], "not_read");
    assert_eq!(json.code, 0);
    for run in [&text, &json] {
        assert!(
            !run.stdout.contains("codex-api.env") && !run.stdout.contains(arg(&scenario.root)),
            "the location of a credential is shown only with --show-paths"
        );
    }
    assert!(text.stdout.contains(
        "inspected  provider profiles \"<config>/providers.json\" (default; in the config directory)\n"
    ));
    let (_, _, shown) = scenario.diagnose_both(["--config-dir", arg(&config), "--show-paths"]);
    assert_eq!(
        provider_reference(&shown)["display"],
        arg(&credentials.codex_api)
    );

    // A path below a regular file cannot be examined: the product calls it
    // unsafe, not absent.
    let below = scenario.historical("config-below-a-file");
    let below_providers = below.join("providers.json");
    fixtures::write_providers(
        &below_providers,
        &json!({"version": 1, "profiles": [
            fixtures::api_profile("only", "codex", &credentials.codex_api.join("below"))
        ]}),
    );
    let (_, json, report) = scenario.diagnose_both(["--config-dir", arg(&below)]);
    assert_eq!(
        product_refusal_class(&scenario, &below_providers, "only").as_deref(),
        Some("credential_unsafe")
    );
    let unsafe_path = finding(&report, "provider.credential_unsafe");
    let message = text_of(unsafe_path, "message");
    assert!(
        message.contains("is not a directory") && !message.contains("symbolic link"),
        "the message fits a path that no file can have: {message}"
    );
    assert!(text_of(unsafe_path, "action").contains("every component before the file name"));
    assert!(find(&report, "provider.credential_missing").is_none());
    assert_eq!(provider_reference(&report)["presence"], "absent");
    assert_eq!(json.code, 3);

    // The same path for one profile among several: a warning, with the same
    // explanation.
    let several = scenario.historical("config-below-a-file-several");
    fixtures::write_providers(
        &several.join("providers.json"),
        &json!({"version": 1, "auto_priority": ["first"], "profiles": [
            fixtures::api_profile("first", "codex", &credentials.codex_api),
            fixtures::api_profile("second", "claude_code", &credentials.claude_api.join("below"))
        ]}),
    );
    let (_, json, report) = scenario.diagnose_both(["--config-dir", arg(&several)]);
    let unselected = finding(&report, "provider.unselected_profile_reference_unsafe");
    assert_eq!(unselected["status"], "warning");
    assert_eq!(
        unselected["subjects"]["ids"],
        json!([{"id": "second", "masked": false}])
    );
    let message = text_of(unselected, "message");
    assert!(
        message.contains("is not a directory") && !message.contains("symbolic link"),
        "{message}"
    );
    assert_eq!(json.code, 0);

    // A file given alone is the one a launch reads, whatever the config
    // directory holds. Its credential lies below its own directory here.
    let alone = scenario.path("alone.json");
    fixtures::write_providers(
        &alone,
        &json!({"version": 1, "profiles": [
            fixtures::api_profile("other", "claude_code", &credentials.claude_api)
        ]}),
    );
    let (_, json, report) = scenario.diagnose_both([
        "--config-dir",
        arg(&config),
        "--providers-file",
        arg(&alone),
    ]);
    assert_eq!(
        *input(&report, "providers_file"),
        json!({"name": "providers_file", "origin": "cli", "display": "<providers-file>"})
    );
    assert_eq!(
        report["providers"]["profiles"][0]["profile"],
        json!({"id": "other", "masked": false})
    );
    assert_eq!(entries(&report["providers"], "profiles").len(), 1);
    let reference = provider_reference(&report);
    assert_eq!(
        reference["display"],
        "provider reference #1 (api_env_file, inside the directory of the profile file)"
    );
    assert_eq!(reference["location"], "inside_root");
    assert_eq!(json.code, 0);

    // A file given alone that does not exist is an error, and the provider
    // initialisation is never proposed for it.
    let (_, json, report) =
        scenario.diagnose_both(["--providers-file", arg(&scenario.path("missing.json"))]);
    let missing = finding(&report, "provider.file_unavailable_or_not_private");
    assert_eq!(missing["status"], "error");
    assert!(text_of(missing, "message").contains("does not exist"));
    assert_eq!(
        missing["scope"],
        json!({"kind": "input", "input": "providers_file"})
    );
    assert_eq!(report["providers"]["file"]["state"], "absent");
    assert!(find(&report, "provider.file_absent").is_none());
    assert_eq!(
        report["layouts"],
        json!([]),
        "a profile file is a target: no default directory is inspected beside it"
    );
    assert_eq!(json.code, 3);

    // A profile file that other accounts can read is refused unread, as the
    // product refuses it.
    let readable = scenario.path("readable.json");
    fixtures::write_mode(&readable, serde_json::to_vec_pretty(&one).unwrap(), 0o644);
    assert!(
        agentsandbox::provider::load_config(&readable).is_err(),
        "the product refuses a profile file in mode 0644"
    );
    let (_, json, report) = scenario.diagnose_both(["--providers-file", arg(&readable)]);
    let refused = finding(&report, "provider.file_unavailable_or_not_private");
    assert!(text_of(refused, "message").contains("private-file rule"));
    assert_eq!(report["providers"]["file"]["state"], "refused");
    assert!(
        !entries(&report["mode"], "files_read_for_content").contains(&json!("provider_profiles"))
    );
    assert_eq!(json.code, 3);

    // A config directory that does not exist: its profiles are not inspected,
    // and nothing is assumed about them.
    let (_, _, report) =
        scenario.diagnose_both(["--config-dir", arg(&scenario.path("missing-directory"))]);
    let not_inspected = finding(&report, "provider.not_inspected");
    assert_eq!(
        not_inspected["scope"],
        json!({"kind": "input", "input": "config_dir"})
    );
    assert!(text_of(not_inspected, "message").contains("could not be inspected"));
    assert_eq!(report["providers"], Value::Null);
    assert_eq!(entries(&report, "inputs").len(), 1);

    // The option names a key file of an inspected directory: it is not
    // opened. The registry is the file the tool must read in this run.
    let pointed = Scenario::new();
    let config = pointed.historical("config");
    let directory = pointed.path("registry");
    fixtures::private_dir(&directory);
    let registry = fixtures::hand_written_registry(&directory, &["tenant-a"]);
    let observed = observed_run(
        &pointed,
        &[
            "--config-dir",
            arg(&config),
            "--tenants-file",
            arg(&registry),
            "--providers-file",
            arg(&config.join("secrets/runner")),
            "--json",
        ],
    );
    assert_never_read(
        &observed,
        "registry/tenants.json",
        &["config/secrets"],
        "a profile file option that names a key file",
    );
    let report = observed.run.json();
    let reserved = finding(&report, "provider.file_unavailable_or_not_private");
    assert!(text_of(reserved, "message").contains("was not opened"));
    assert_eq!(report["providers"]["file"]["state"], "refused");
    assert!(find(&report, "provider.configuration_invalid").is_none());
    assert_eq!(observed.run.code, 3);
}

// ---- beyond the table: bundles and transport given by option ----------------

#[test]
fn explicit_trust_inputs() {
    use agentsandbox::jev::{JevTrust, validate_transport_configuration};
    let scenario = Scenario::new();
    // A valid target, so that the exit code is decided by the explicit inputs.
    let config = scenario.historical("config");
    let bundles = scenario.path("bundles");
    fixtures::private_dir(&bundles);
    let certificate = fixtures::PUBLIC_TEST_ROOT;
    let bundle = |name: &str, content: &str, mode: u32| {
        let path = bundles.join(name);
        fixtures::write_mode(&path, content, mode);
        path
    };
    let private = bundle("private.pem", certificate, 0o600);
    let projected = bundle("projected.pem", certificate, 0o444);
    let key_section = bundle(
        "key-section.pem",
        "-----BEGIN PRIVATE KEY-----\nAA==\n-----END PRIVATE KEY-----\n",
        0o600,
    );
    let writable = bundle("group-writable.pem", certificate, 0o664);
    let large = bundle(
        "large.pem",
        &format!("{certificate}{}", "\n".repeat(65_537 - certificate.len())),
        0o600,
    );
    let missing = bundles.join("missing.pem");
    let remote = "https://engine.example.invalid/v1";
    let loopback = "http://127.0.0.1:9/v1";
    let plain_http = "http://engine.example.invalid/v1";

    // The product's own readers and validator decide every verdict below.
    let trust = JevTrust::from_ca_file(&private)
        .expect("positive control: the product accepts the public test certificate");
    JevTrust::from_ca_file(&projected)
        .expect("positive control: the product accepts a read-only projection");
    for (name, refused) in [
        ("a key section", &key_section),
        ("a group-writable file", &writable),
        ("65537 bytes", &large),
        ("an absent file", &missing),
    ] {
        assert!(
            JevTrust::from_ca_file(refused).is_err(),
            "the product refuses a bundle with {name}"
        );
    }
    assert!(validate_transport_configuration(remote, false, &trust).is_ok());
    assert!(
        validate_transport_configuration(loopback, true, &JevTrust::default()).is_ok()
            && validate_transport_configuration(loopback, true, &trust).is_err(),
        "the product accepts local HTTP alone and refuses it with a CA bundle"
    );
    assert!(validate_transport_configuration(plain_http, false, &JevTrust::default()).is_err());

    let count = |report: &Value, code: &str| {
        findings(report)
            .into_iter()
            .filter(|finding| finding["code"] == code)
            .count()
    };
    let transport_verdicts = |report: &Value| {
        count(report, "inference.decision_transport_valid")
            + count(report, "trust.decision_transport_invalid")
    };

    // Two accepted bundles and an accepted transport.
    let accepted_args = [
        "--config-dir",
        arg(&config),
        "--jev-ca-file",
        arg(&private),
        "--gateway-ca-file",
        arg(&projected),
        "--jev-url",
        remote,
    ];
    let (text, json, report) = scenario.diagnose_both(accepted_args);
    assert_eq!(
        report["trust"],
        json!([
            {"role": "decision_engine_ca_bundle", "display": "<jev-ca-file>",
             "read": true, "certificate_count": 1},
            {"role": "gateway_ca_bundle", "display": "<gateway-ca-file>",
             "read": true, "certificate_count": 1}
        ])
    );
    assert_eq!(
        finding(&report, "trust.decision_engine_ca_accepted_by_grammar")["scope"],
        json!({"kind": "file", "file": "decision_engine_ca_bundle"})
    );
    assert_eq!(
        finding(&report, "trust.gateway_ca_accepted_by_grammar")["scope"],
        json!({"kind": "file", "file": "gateway_ca_bundle"})
    );
    assert_eq!(
        count(&report, "trust.certificate_validity_period_not_available"),
        2,
        "validity dates are not evaluated for either bundle"
    );
    assert_eq!(
        report["inference"]["decision_engine_transport"],
        json!({"kind": "remote", "url": remote, "allow_local_http": false, "ca_bundle_given": true})
    );
    assert_eq!(
        finding(&report, "inference.decision_transport_valid")["scope"],
        json!({"kind": "input", "input": "jev_url"})
    );
    assert!(find(&report, "inference.decision_transport_not_checked").is_none());
    for role in ["decision_engine_ca_bundle", "gateway_ca_bundle"] {
        assert!(entries(&report["mode"], "files_read_for_content").contains(&json!(role)));
    }
    assert_eq!(json.code, 0);
    for run in [&text, &json] {
        assert!(
            !run.stdout.contains("MIIB") && !run.stdout.contains(arg(&scenario.root)),
            "neither the content nor the location of a bundle is printed"
        );
    }
    assert!(
        text.stdout
            .contains("  decision engine ca bundle  \"<jev-ca-file>\"  read; 1 certificate\n")
    );
    let (_, _, shown) = scenario.diagnose_both(
        accepted_args
            .iter()
            .copied()
            .chain(["--show-paths"])
            .collect::<Vec<_>>(),
    );
    assert_eq!(shown["trust"][0]["display"], arg(&private));

    // Internal inference is its own axis: what is not stored or not
    // inspectable is named in every report, beside the limit of the build.
    assert_eq!(
        finding(&report, "inference.decision_engine_selection_not_persisted")["status"],
        "not_checked"
    );
    for code in [
        "inference.local_model_backend_not_available",
        "inference.model_tls_outputs_not_available",
        "inference.internal_profile_not_available",
    ] {
        assert_eq!(finding(&report, code)["status"], "not_available", "{code}");
    }

    // Local HTTP is refused together with a CA bundle.
    let (_, json, report) = scenario.diagnose_both([
        "--config-dir",
        arg(&config),
        "--jev-ca-file",
        arg(&private),
        "--jev-local-url",
        loopback,
        "--jev-allow-local-http",
    ]);
    let refused = finding(&report, "trust.decision_transport_invalid");
    assert_eq!(
        refused["scope"],
        json!({"kind": "input", "input": "jev_local_url"})
    );
    assert!(text_of(refused, "action").contains("refused together, whatever the URL"));
    assert_eq!(
        report["inference"]["decision_engine_transport"],
        json!({"kind": "local", "url": loopback, "allow_local_http": true, "ca_bundle_given": true})
    );
    assert_eq!(json.code, 3);

    // A bundle the grammar refuses: read, never printed, and no verdict on a
    // transport that cannot be built with it.
    let (text, json, report) = scenario.diagnose_both([
        "--config-dir",
        arg(&config),
        "--jev-ca-file",
        arg(&key_section),
        "--jev-url",
        remote,
    ]);
    assert_eq!(
        finding(&report, "trust.ca_bundle_invalid")["scope"],
        json!({"kind": "file", "file": "decision_engine_ca_bundle"})
    );
    assert_eq!(
        report["trust"],
        json!([{"role": "decision_engine_ca_bundle", "display": "<jev-ca-file>",
                "read": true, "certificate_count": null}])
    );
    assert!(find(&report, "trust.decision_engine_ca_accepted_by_grammar").is_none());
    assert_eq!(transport_verdicts(&report), 0);
    assert_eq!(json.code, 3);
    assert!(!text.stdout.contains("AA==") && !json.stdout.contains("AA=="));
    // What was not judged is said, with what resolves it: it is never left
    // out of the report.
    let not_judged = finding(&report, "inference.decision_transport_not_checked");
    assert_eq!(not_judged["status"], "not_checked");
    assert_eq!(
        not_judged["scope"],
        json!({"kind": "input", "input": "jev_url"})
    );
    assert_eq!(
        not_judged["requires"],
        "a CA bundle that the product accepts, given with --jev-ca-file FILE"
    );
    assert!(
        text.stdout
            .contains("  [not_checked] inference.decision_transport_not_checked\n")
    );

    // The same when the reader of the bundle returns no file at all.
    let (_, _, report) = scenario.diagnose_both([
        "--config-dir",
        arg(&config),
        "--jev-ca-file",
        arg(&missing),
        "--jev-local-url",
        loopback,
    ]);
    finding(&report, "trust.ca_unavailable_or_not_private");
    assert_eq!(transport_verdicts(&report), 1);
    assert!(
        find(&report, "inference.decision_transport_not_checked").is_none(),
        "a URL the validator refuses even alone has its verdict"
    );
    let (_, _, report) = scenario.diagnose_both([
        "--config-dir",
        arg(&config),
        "--jev-ca-file",
        arg(&missing),
        "--jev-local-url",
        loopback,
        "--jev-allow-local-http",
    ]);
    finding(&report, "trust.ca_unavailable_or_not_private");
    assert_eq!(transport_verdicts(&report), 0);
    assert_eq!(
        finding(&report, "inference.decision_transport_not_checked")["scope"],
        json!({"kind": "input", "input": "jev_local_url"})
    );

    // The same bundle with a URL the validator refuses even alone.
    let (_, _, report) = scenario.diagnose_both([
        "--config-dir",
        arg(&config),
        "--jev-ca-file",
        arg(&key_section),
        "--jev-url",
        plain_http,
    ]);
    assert!(
        text_of(
            finding(&report, "trust.decision_transport_invalid"),
            "message"
        )
        .contains("even without the CA bundle")
    );
    assert!(find(&report, "inference.decision_transport_not_checked").is_none());

    // What the reader of a bundle refuses is not read.
    for (name, path, code) in [
        (
            "group-writable",
            &writable,
            "trust.ca_unavailable_or_not_private",
        ),
        ("absent", &missing, "trust.ca_unavailable_or_not_private"),
        ("65537 bytes", &large, "trust.ca_bundle_invalid"),
    ] {
        let (_, json, report) =
            scenario.diagnose_both(["--config-dir", arg(&config), "--gateway-ca-file", arg(path)]);
        assert_eq!(
            finding(&report, code)["scope"],
            json!({"kind": "file", "file": "gateway_ca_bundle"}),
            "{name}"
        );
        assert_eq!(
            report["trust"],
            json!([{"role": "gateway_ca_bundle", "display": "<gateway-ca-file>",
                    "read": false, "certificate_count": null}]),
            "{name}"
        );
        assert!(
            !entries(&report["mode"], "files_read_for_content")
                .contains(&json!("gateway_ca_bundle")),
            "{name}"
        );
        assert_eq!(json.code, 3, "{name}");
    }

    // A bundle option that names a key file of an inspected directory: it is
    // not opened. The accepted bundle is the file the tool must read.
    let observed = observed_run(
        &scenario,
        &[
            "--config-dir",
            arg(&config),
            "--jev-ca-file",
            arg(&private),
            "--gateway-ca-file",
            arg(&config.join("secrets/viewer")),
            "--json",
        ],
    );
    assert_never_read(
        &observed,
        "bundles/private.pem",
        &["config/secrets"],
        "a bundle option that names a key file",
    );
    let report = observed.run.json();
    assert_eq!(
        finding(&report, "trust.ca_reference_is_a_credential_reference")["scope"],
        json!({"kind": "file", "file": "gateway_ca_bundle"})
    );
    assert!(find(&report, "trust.ca_bundle_invalid").is_none());
    assert_eq!(report["trust"][1]["read"], false);
    assert_eq!(observed.run.code, 3);
}

// ---- beyond the table: an engine program the product refuses -----------------

#[test]
fn engine_program_refusals() {
    use agentsandbox::runtime::{Runtime, RuntimeKind};
    let scenario = Scenario::new();
    let config = scenario.historical("config");
    // The approval hashes the program and never executes it: a file that is
    // executable by its mode is enough here.
    let program_in = |name: &str, directory_mode: u32, program_mode: u32| {
        let directory = scenario.path(name);
        fs::create_dir(&directory).unwrap();
        let program = directory.join("docker");
        fixtures::write_mode(&program, "not a program", program_mode);
        fs::set_permissions(&directory, fs::Permissions::from_mode(directory_mode)).unwrap();
        program
    };
    let approved = program_in("engine", 0o700, 0o755);
    let not_executable = program_in("engine-not-executable", 0o700, 0o600);
    let open_directory = program_in("engine-open-directory", 0o777, 0o755);
    let missing = scenario.path("engine/missing");
    let diagnose = |program: &str| {
        scenario.diagnose_both([
            "--config-dir",
            arg(&config),
            "--runtime",
            "docker",
            "--runtime-program",
            program,
        ])
    };

    // Positive control of every refusal below, by the product and by the tool.
    Runtime::with_program(RuntimeKind::Docker, &approved)
        .expect("positive control: the product approves this program");
    let (text, json, report) = diagnose(arg(&approved));
    assert_eq!(
        finding(&report, "engine.program_approved")["details"],
        json!({"kind": "engine_program", "explicit_program": true})
    );
    assert_eq!(report["engine"]["entry_display"], "<runtime-program>");
    assert_eq!(report["engine"]["policy_version"], 2);
    assert_eq!(json.code, 0);
    assert!(text.stdout.contains(&format!(
        "  engine program  docker \"<runtime-program>\"; approval policy version 2\n          sha256 \"{}\"\n",
        text_of(&report["engine"], "sha256")
    )));

    // Each refusal differs from the control by one stated edit. The closed
    // reason is the product's; where this tool's own look at the path says
    // more, the message and the action say what is wrong.
    for (name, program, read, (message, action)) in [
        (
            "a program that is not executable",
            arg(&not_executable),
            true,
            (
                "is not a regular executable file",
                "a regular executable file owned by root or the control account",
            ),
        ),
        (
            "a directory that other accounts can write",
            arg(&open_directory),
            true,
            (
                "The directory that holds the engine program",
                "a directory that only root or the control account can write",
            ),
        ),
        (
            "a program that does not exist",
            arg(&missing),
            false,
            (
                "Nothing exists at the given engine program path",
                "an existing engine program",
            ),
        ),
        (
            "a relative program",
            "docker",
            false,
            ("is not an absolute path", "Give the absolute path"),
        ),
    ] {
        let refusal = Runtime::with_program(RuntimeKind::Docker, Path::new(program))
            .expect_err("the product refuses this program")
            .to_string();
        let (text, json, report) = diagnose(program);
        let refused = finding(&report, "engine.program_refused");
        assert_eq!(refused["status"], "error", "{name}");
        assert_eq!(
            refused["details"],
            json!({"kind": "reason", "reason": "program_or_directory_refused"}),
            "{name}"
        );
        assert_eq!(
            refused["scope"],
            json!({"kind": "input", "input": "runtime"}),
            "{name}"
        );
        assert_eq!(report["engine"], Value::Null, "{name}");
        assert!(
            text_of(refused, "message").contains(message),
            "{name}: the message says what is wrong: {}",
            refused["message"]
        );
        assert!(
            text_of(refused, "action").contains(action),
            "{name}: the action fits the case: {}",
            refused["action"]
        );
        finding(&report, "engine.state_not_checked");
        assert_eq!(
            entries(&report["mode"], "files_read_for_content").contains(&json!("engine_program")),
            read,
            "{name}: a refused program is listed as read unless nothing exists at its path"
        );
        assert_eq!(json.code, 3, "{name}");
        // The product's sentence may carry a system error: it is not printed.
        let sentence = refusal.split(": ").next().unwrap();
        for output in [&text.stdout, &json.stdout, &text.stderr, &json.stderr] {
            assert!(
                !output.contains(sentence),
                "{name}: the text of the product error is printed"
            );
        }
    }

    // No engine named: nothing is approved, and the report says what would.
    let (_, json, report) = scenario.diagnose_both(["--config-dir", arg(&config)]);
    let not_inspected = finding(&report, "engine.program_not_inspected");
    assert_eq!(not_inspected["status"], "not_checked");
    assert_eq!(
        not_inspected["requires"],
        "--runtime docker or --runtime podman"
    );
    finding(&report, "engine.state_not_checked");
    assert_eq!(report["engine"], Value::Null);
    assert!(!entries(&report["mode"], "files_read_for_content").contains(&json!("engine_program")));
    assert_eq!(json.code, 0);
}

// ---- beyond the table: the product texts behind the closed engine reasons ----

/// The product sources that hold the refusals of the engine approval.
const APPROVAL_SOURCES: [(&str, &str); 2] = [
    (
        "src/runtime/invocation.rs",
        include_str!("../src/runtime/invocation.rs"),
    ),
    ("src/control_fs.rs", include_str!("../src/control_fs.rs")),
];

#[test]
fn engine_refusal_reasons_follow_the_product_texts() {
    use agentsandbox::{
        diagnostics::{engine, report::Reason},
        runtime::{Runtime, RuntimeKind},
    };
    // Every text this tool compares still exists, whole, as a string literal
    // of the product, and still maps to its reason. A refusal the product
    // rewords would otherwise be reported as unclassified with no failure.
    // Only the presence of the text is checked: no rule is restated.
    let in_product = |literal: &str| {
        APPROVAL_SOURCES
            .iter()
            .any(|(_, source)| source.contains(&format!("\"{literal}\"")))
    };
    assert!(
        !in_product("runtime program was reworded")
            && engine::reason("runtime program was reworded") == Reason::Unclassified,
        "calibration: a text the product does not hold is seen as absent and is unclassified"
    );
    // Each text also has a situation of its own in the catalogue: a message
    // that names the cause and an action that fits it.
    let texts = CATALOGUE
        .iter()
        .find(|entry| entry.code.as_str() == "engine.program_refused")
        .expect("the catalogue holds the code of a refused engine program");
    let situation_texts = |situation: &str| {
        let variant = texts
            .variants
            .iter()
            .find(|variant| variant.name == situation)
            .unwrap_or_else(|| panic!("the situation {situation:?} has no text in the catalogue"));
        (
            variant
                .message
                .unwrap_or_else(|| panic!("{situation:?} has no message of its own")),
            variant
                .action
                .unwrap_or_else(|| panic!("{situation:?} has no action of its own")),
        )
    };
    let mut reasons = BTreeSet::new();
    let mut unclassified = BTreeSet::new();
    let mut situations = BTreeSet::new();
    for &(literal, expected, situation) in engine::REFUSALS {
        assert!(
            in_product(literal),
            "the product no longer holds the refusal text {literal:?}: its reason {expected:?} and its cause {situation:?} would silently be lost"
        );
        assert_eq!(
            engine::classify(literal),
            (expected, Some(situation)),
            "{literal:?}"
        );
        assert_eq!(engine::reason(literal), expected, "{literal:?}");
        situation_texts(situation);
        situations.insert(situation);
        if expected == Reason::Unclassified {
            unclassified.insert(situation);
        } else {
            reasons.insert(format!("{expected:?}"));
        }
    }
    assert_eq!(
        unclassified,
        BTreeSet::from(["extended_access_entry", "runtime_directory"]),
        "the two refusals that no closed reason names have a text of their own"
    );
    assert!(
        situation_texts("link_outside_approved_roots")
            .1
            .contains("--runtime-program ABS"),
        "an engine linked outside the approved locations is answered by an explicit program"
    );
    assert_eq!(
        engine::classify("runtime program was reworded"),
        (Reason::Unclassified, None),
        "a text the product does not hold has the generic texts"
    );
    for &(prefix, expected, situation) in engine::REFUSAL_PREFIXES {
        situation_texts(situation);
        situations.insert(situation);
        // The product formats a system error after the fixed part.
        assert!(
            in_product(&format!("{prefix}{{}}")),
            "the product no longer holds the refusal text {prefix:?} followed by a system error"
        );
        assert_eq!(
            engine::reason(&format!("{prefix}any system error")),
            expected,
            "{prefix:?}"
        );
    }
    assert!(
        in_product(engine::ABSENT_FROM_FIXED_ENTRIES),
        "the product no longer holds the text of an engine absent from its fixed entries"
    );
    assert_eq!(
        reasons,
        BTreeSet::from([
            "AccountHomeUnavailable".to_owned(),
            "HelperDirectoriesRefused".to_owned(),
            "ProgramOrDirectoryRefused".to_owned()
        ]),
        "each closed engine reason is reached by a product text"
    );
    // Every situation of the catalogue is reached: by a product text, or by
    // this tool's own look at a given path.
    let named: BTreeSet<&str> = texts.variants.iter().map(|variant| variant.name).collect();
    situations.extend(["absent", "relative"]);
    assert_eq!(named, situations);

    // The two refusals that end with a system error, driven for real: a
    // directory and a program that the control account cannot open for
    // reading. Their text is never printed.
    let scenario = Scenario::new();
    let config = scenario.historical("config");
    let unreadable_directory = scenario.path("engine-unreadable-directory");
    fixtures::private_dir(&unreadable_directory);
    let in_unreadable_directory = unreadable_directory.join("docker");
    fixtures::write_mode(&in_unreadable_directory, "not a program", 0o755);
    fs::set_permissions(&unreadable_directory, fs::Permissions::from_mode(0o300)).unwrap();
    let directory = scenario.path("engine-unreadable-program");
    fixtures::private_dir(&directory);
    let unreadable_program = directory.join("docker");
    fixtures::write_mode(&unreadable_program, "not a program", 0o311);

    let mut outcomes = Vec::new();
    for (&(prefix, expected, situation), program) in engine::REFUSAL_PREFIXES
        .iter()
        .zip([&in_unreadable_directory, &unreadable_program])
    {
        let refusal = Runtime::with_program(RuntimeKind::Docker, program)
            .map(|_| ())
            .map_err(|error| error.to_string());
        // No manifest here: an entry that cannot be read cannot be hashed.
        let run = scenario.diagnose_unobserved([
            "--config-dir",
            arg(&config),
            "--runtime",
            "docker",
            "--runtime-program",
            arg(program),
            "--json",
        ]);
        outcomes.push((prefix, expected, situation, refusal, run));
    }
    // Restore what this test made unreadable before any assertion.
    fs::set_permissions(&unreadable_directory, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(&unreadable_program, fs::Permissions::from_mode(0o755)).unwrap();

    for (prefix, expected, situation, refusal, run) in outcomes {
        let refusal = match refusal {
            Err(refusal) => refusal,
            // An account that bypasses permission bits opens both.
            Ok(()) => {
                oracle_not_run(
                    "engine-refusal-with-a-system-error",
                    "this account opens an entry whose mode grants it no read",
                );
                continue;
            }
        };
        assert!(
            refusal.starts_with(prefix),
            "the product gives the planned refusal for an unreadable entry: {refusal:?}"
        );
        let report = run.json();
        assert_eq!(
            finding(&report, "engine.program_refused")["details"],
            serde_json::to_value(agentsandbox::diagnostics::report::Details::Reason {
                reason: expected
            })
            .unwrap(),
            "{prefix:?}"
        );
        assert_eq!(
            text_of(finding(&report, "engine.program_refused"), "message"),
            situation_texts(situation).0,
            "{prefix:?}: the message of its cause"
        );
        assert_eq!(report["engine"], Value::Null);
        assert_eq!(run.code, 3);
        assert!(
            !run.stdout.contains(prefix.trim_end_matches(": ")) && !run.stdout.contains(&refusal),
            "the text of the product error, with its system error, is never printed"
        );
    }
}

// ---- beyond the table: an engine program option that names a reserved file ---

#[test]
fn engine_program_option_never_hashes_a_reserved_file() {
    use agentsandbox::runtime::{Runtime, RuntimeKind};
    let scenario = Scenario::new();
    let config = scenario.historical("config");
    // The profile file is what the tool must read in these runs.
    fixtures::write_providers(
        &config.join("providers.json"),
        &json!({"version": 1, "profiles": []}),
    );
    // A key file with an execute bit, and a file beside it that no reference
    // names. The approval of the product asks nothing else of a program.
    let key = config.join("secrets/runner");
    fs::set_permissions(&key, fs::Permissions::from_mode(0o700)).unwrap();
    let spare = config.join("secrets/spare");
    fixtures::write_mode(&spare, fixtures::synthetic_token("spare"), 0o700);
    // The approval resolves every link before it opens the program.
    let links = scenario.path("links");
    fixtures::private_dir(&links);
    let link = links.join("docker");
    symlink(&spare, &link).unwrap();

    // Control: the product approves a file of this posture, so only the
    // reservation keeps a key from being hashed and its digest printed.
    let control = scenario.path("control");
    fixtures::private_dir(&control);
    let control_program = control.join("docker");
    fixtures::write_mode(
        &control_program,
        fixtures::synthetic_token("control"),
        0o700,
    );
    Runtime::with_program(RuntimeKind::Docker, &control_program)
        .expect("positive control: the product approves an executable file of this posture");

    let runs: Vec<(&str, ObservedRun)> = [
        ("the key file itself", &key),
        ("a link to a file below the key directory", &link),
    ]
    .into_iter()
    .map(|(context, program)| {
        let observed = observed_run(
            &scenario,
            &[
                "--config-dir",
                arg(&config),
                "--runtime",
                "docker",
                "--runtime-program",
                arg(program),
                "--json",
            ],
        );
        eprintln!(
            "OBSERVED entries read with {context} as the engine program: {:?}",
            observed.accessed()
        );
        (context, observed)
    })
    .collect();

    for (context, observed) in &runs {
        assert_never_read(
            observed,
            "config/providers.json",
            &["config/secrets", "config/mounted-secrets"],
            context,
        );
        let report = observed.run.json();
        let reserved = finding(&report, "engine.program_is_a_credential_reference");
        assert_eq!(reserved["status"], "error", "{context}");
        assert_eq!(
            reserved["scope"],
            json!({"kind": "input", "input": "runtime_program"}),
            "{context}"
        );
        assert!(
            text_of(reserved, "message").contains("was not opened"),
            "{context}"
        );
        assert!(
            find(&report, "engine.program_approved").is_none()
                && find(&report, "engine.program_refused").is_none(),
            "{context}: no approval is attempted on a reserved file"
        );
        assert_eq!(
            report["engine"],
            Value::Null,
            "{context}: no digest is printed"
        );
        assert!(
            !entries(&report["mode"], "files_read_for_content").contains(&json!("engine_program")),
            "{context}"
        );
        finding(&report, "engine.state_not_checked");
        assert_eq!(observed.run.code, 3, "{context}");
    }
}

// ---- beyond the table: reserved names, whatever option gives the directory ---

#[test]
fn key_files_are_reserved_whatever_option_gives_their_directory() {
    let manifest = fixtures::manifest_https(&["tenant-a"], Some(fixtures::PUBLIC_TEST_ROOT));
    let credential_reference = "trust.ca_reference_is_a_credential_reference";

    // A mixed directory given as an installation: its historical keys are
    // named by no file of the installation.
    let mixed_scenario = Scenario::new();
    let mixed = mixed_scenario.prepare("mixed", &manifest);
    agentsandbox::operations::initialize_config(&mixed)
        .expect("fixture: the product init accepts an existing private directory");
    fixtures::edit_json(&mixed.join("oidc.json"), |oidc| {
        oidc["ca_bundle_file"] = "secrets/runner".into();
    });
    let mixed_run = observed_run(
        &mixed_scenario,
        &["--installation-dir", arg(&mixed), "--json"],
    );

    // A prepared installation given as a config directory: nothing in it is
    // described, and an OIDC file given alone points into it.
    let wrong_option = Scenario::new();
    let prepared = wrong_option.prepare("prepared", &manifest);
    let pointing = wrong_option.prepare("pointing", &manifest);
    let pointing_oidc = pointing.join("oidc.json");
    let mut reference_runs = Vec::new();
    for reserved in ["tenants/tenant-a/runner", "secret-material.json"] {
        fixtures::edit_json(&pointing_oidc, |oidc| {
            oidc["ca_bundle_file"] = arg(&prepared.join(reserved)).into();
        });
        reference_runs.push((
            reserved,
            observed_run(
                &wrong_option,
                &[
                    "--config-dir",
                    arg(&prepared),
                    "--oidc-config",
                    arg(&pointing_oidc),
                    "--json",
                ],
            ),
        ));
    }

    // The bundle options, with a directory given through the other option.
    let bundle = wrong_option.path("bundle.pem");
    fixtures::private_write(&bundle, fixtures::PUBLIC_TEST_ROOT);
    let historical = wrong_option.historical("historical");
    let option_runs = [
        (
            "a setup record of a prepared installation given as a config directory",
            observed_run(
                &wrong_option,
                &[
                    "--config-dir",
                    arg(&prepared),
                    "--jev-ca-file",
                    arg(&bundle),
                    "--gateway-ca-file",
                    arg(&prepared.join("preparation.json")),
                    "--json",
                ],
            ),
            "prepared",
        ),
        (
            "a key of a historical directory given as an installation",
            observed_run(
                &wrong_option,
                &[
                    "--installation-dir",
                    arg(&historical),
                    "--jev-ca-file",
                    arg(&bundle),
                    "--gateway-ca-file",
                    arg(&historical.join("secrets/viewer")),
                    "--json",
                ],
            ),
            "historical/secrets",
        ),
    ];

    assert_never_read(
        &mixed_run,
        "mixed/oidc.json",
        &["mixed/secrets", "mixed/mounted-secrets", "mixed/tenants"],
        "a mixed directory given as an installation",
    );
    let report = mixed_run.run.json();
    assert_eq!(report["layouts"][0]["kind"], "mixed");
    assert_eq!(finding(&report, credential_reference)["status"], "error");
    assert!(find(&report, "trust.ca_bundle_invalid").is_none());
    assert_eq!(report["trust"][0]["read"], false);

    for (reserved, observed) in &reference_runs {
        assert_never_read(
            observed,
            "pointing/oidc.json",
            &["prepared"],
            &format!("a CA reference to {reserved} of a directory given as --config-dir"),
        );
        let report = observed.run.json();
        finding(&report, "layout.prepared_passed_as_config_dir");
        assert_eq!(
            finding(&report, credential_reference)["status"],
            "error",
            "{reserved}"
        );
        assert!(
            find(&report, "trust.ca_bundle_invalid").is_none(),
            "{reserved}"
        );
        assert_eq!(report["trust"][0]["read"], false, "{reserved}");
    }

    for (context, observed, never) in &option_runs {
        assert_never_read(observed, "bundle.pem", &[never], context);
        let report = observed.run.json();
        assert_eq!(
            finding(&report, credential_reference)["scope"],
            json!({"kind": "file", "file": "gateway_ca_bundle"}),
            "{context}"
        );
        assert!(
            find(&report, "trust.ca_bundle_invalid").is_none(),
            "{context}"
        );
        assert_eq!(observed.run.code, 3, "{context}");
    }
}

// ---- beyond the table: a file option that names a key file -------------------

#[test]
fn file_options_that_name_a_key_file_are_never_opened() {
    // The registry and the OIDC file are read as the gateway reads them. A
    // key file of an inspected directory given in their place is not opened.
    let as_registry = Scenario::new();
    let config = as_registry.historical("config");
    let profiles = as_registry.path("profiles.json");
    fixtures::write_providers(&profiles, &json!({"version": 1, "profiles": []}));
    let registry_run = observed_run(
        &as_registry,
        &[
            "--config-dir",
            arg(&config),
            "--tenants-file",
            arg(&config.join("secrets/runner")),
            "--providers-file",
            arg(&profiles),
            "--json",
        ],
    );
    assert_never_read(
        &registry_run,
        "profiles.json",
        &["config/secrets"],
        "a registry option that names a key file",
    );
    let report = registry_run.run.json();
    let refused = finding(&report, "registry.file_unreadable");
    assert!(text_of(refused, "message").contains("was not opened"));
    assert_eq!(
        refused["scope"],
        json!({"kind": "input", "input": "tenants_file"})
    );
    assert!(find(&report, "registry.json_invalid").is_none());
    assert!(
        !entries(&report["mode"], "files_read_for_content").contains(&json!("tenant_registry"))
    );
    assert_eq!(registry_run.run.code, 3);

    let as_oidc_file = Scenario::new();
    let directory = as_oidc_file.path("registry");
    fixtures::private_dir(&directory);
    let registry = fixtures::hand_written_registry(&directory, &["tenant-a"]);
    let config = as_oidc_file.historical("config");
    let oidc_run = observed_run(
        &as_oidc_file,
        &[
            "--config-dir",
            arg(&config),
            "--tenants-file",
            arg(&registry),
            "--oidc-config",
            arg(&config.join("secrets/viewer")),
            "--json",
        ],
    );
    assert_never_read(
        &oidc_run,
        "registry/tenants.json",
        &["config/secrets"],
        "an OIDC file option that names a key file",
    );
    let report = oidc_run.run.json();
    let refused = finding(&report, "oidc.file_unavailable_or_not_private");
    assert!(text_of(refused, "message").contains("was not opened"));
    assert_eq!(
        refused["scope"],
        json!({"kind": "input", "input": "oidc_config"})
    );
    assert!(find(&report, "oidc.json_invalid").is_none());
    assert_eq!(report["identity"]["sso"], Value::Null);
    assert!(!entries(&report["mode"], "files_read_for_content").contains(&json!("oidc_file")));
    assert_eq!(oidc_run.run.code, 3);

    // The gateway's read of a registry follows the links of the directories
    // that lead to it. A file below a key directory, reached through a linked
    // directory and named by no reference, is recognised by its resolved path.
    let through_a_link = Scenario::new();
    let config = through_a_link.historical("config");
    fixtures::private_write(
        &config.join("secrets/spare"),
        fixtures::synthetic_token("spare"),
    );
    let profiles = through_a_link.path("profiles.json");
    fixtures::write_providers(&profiles, &json!({"version": 1, "profiles": []}));
    symlink(config.join("secrets"), through_a_link.path("linked")).unwrap();
    let linked_run = observed_run(
        &through_a_link,
        &[
            "--config-dir",
            arg(&config),
            "--tenants-file",
            arg(&through_a_link.path("linked/spare")),
            "--providers-file",
            arg(&profiles),
            "--json",
        ],
    );
    assert_never_read(
        &linked_run,
        "profiles.json",
        &["config/secrets"],
        "a registry option that reaches a key directory through a link",
    );
    let report = linked_run.run.json();
    assert!(
        text_of(finding(&report, "registry.file_unreadable"), "message").contains("was not opened")
    );
    assert!(find(&report, "registry.json_invalid").is_none());
    assert!(
        !entries(&report["mode"], "files_read_for_content").contains(&json!("tenant_registry"))
    );
    assert_eq!(linked_run.run.code, 3);
}

// ---- beyond the table: the order of the reads, and the fixed names ------------

#[test]
fn files_are_compared_with_the_known_references_before_they_are_opened() {
    // The profile file of a config directory is read at its fixed name before
    // a registry given by option: a registry option that names one of its
    // credentials, outside every inspected directory, is not opened.
    let by_option = Scenario::new();
    let config = by_option.historical("config");
    let credentials = fixtures::provider_credentials(&by_option.path("credentials"));
    fixtures::write_providers(
        &config.join("providers.json"),
        &fixtures::four_profiles(&credentials),
    );
    let registry_run = observed_run(
        &by_option,
        &[
            "--config-dir",
            arg(&config),
            "--tenants-file",
            arg(&credentials.codex_api),
            "--json",
        ],
    );

    // A registry given by option is read before a profile file given by
    // option: a profile option that names one of its key files is not opened.
    let both_by_option = Scenario::new();
    let directory = both_by_option.path("registry");
    fixtures::private_dir(&directory);
    let registry = fixtures::hand_written_registry(&directory, &["tenant-a"]);
    let profiles_run = observed_run(
        &both_by_option,
        &[
            "--tenants-file",
            arg(&registry),
            "--providers-file",
            arg(&directory.join("tenant-a/runner")),
            "--json",
        ],
    );

    // The OIDC file of an installation directory, at its fixed name, when
    // that name is a second name of a setup record.
    let manifest = fixtures::manifest_v2(&["tenant-a"]);
    let oidc_name = Scenario::new();
    let installation = oidc_name.prepare("installation", &manifest);
    fs::remove_file(installation.join("oidc.json")).unwrap();
    fs::hard_link(
        installation.join("preparation.json"),
        installation.join("oidc.json"),
    )
    .unwrap();
    let oidc_run = observed_run(
        &oidc_name,
        &["--installation-dir", arg(&installation), "--json"],
    );

    // The registry of an installation directory, at its fixed name, when that
    // name is a second name of the setup record that holds the generated
    // tokens.
    let registry_name = Scenario::new();
    let installation = registry_name.prepare("installation", &manifest);
    fs::remove_file(installation.join("tenants.json")).unwrap();
    fs::hard_link(
        installation.join("secret-material.json"),
        installation.join("tenants.json"),
    )
    .unwrap();
    let name_run = observed_run(
        &registry_name,
        &["--installation-dir", arg(&installation), "--json"],
    );
    for (context, observed) in [
        ("a registry option", &registry_run),
        ("a profile option", &profiles_run),
        ("the fixed name of the OIDC file", &oidc_run),
        ("the fixed name of the registry", &name_run),
    ] {
        eprintln!(
            "OBSERVED entries read with {context} that names a reserved file: {:?}",
            observed.accessed()
        );
    }

    assert_never_read(
        &profiles_run,
        "registry/tenants.json",
        &["registry/tenant-a"],
        "a profile option that names a key file of the registry",
    );
    let report = profiles_run.run.json();
    assert!(
        text_of(
            finding(&report, "provider.file_unavailable_or_not_private"),
            "message"
        )
        .contains("was not opened")
    );
    assert_eq!(report["mode"]["credential_contents_read"], "none");

    assert_never_read(
        &oidc_run,
        "installation/tenants.json",
        &[
            "installation/preparation.json",
            "installation/oidc.json",
            "installation/tenants/tenant-a",
        ],
        "the fixed name of the OIDC file is a second name of a setup record",
    );
    let report = oidc_run.run.json();
    assert!(
        text_of(
            finding(&report, "oidc.file_unavailable_or_not_private"),
            "message"
        )
        .contains("was not opened")
    );
    assert!(find(&report, "oidc.json_invalid").is_none());
    assert_eq!(
        report["mode"]["files_read_for_content"],
        json!(["tenant_registry"])
    );

    assert_never_read(
        &registry_run,
        "config/providers.json",
        &["credentials", "config/secrets", "config/mounted-secrets"],
        "a registry option that names a provider credential",
    );
    let report = registry_run.run.json();
    let refused = finding(&report, "registry.file_unreadable");
    assert!(text_of(refused, "message").contains("was not opened"));
    assert_eq!(
        refused["scope"],
        json!({"kind": "input", "input": "tenants_file"})
    );
    assert!(find(&report, "registry.json_invalid").is_none());
    assert_eq!(
        report["mode"]["files_read_for_content"],
        json!(["provider_profiles"])
    );
    assert_eq!(report["mode"]["credential_contents_read"], "none");
    assert_eq!(registry_run.run.code, 3);

    assert_never_read(
        &name_run,
        "installation/oidc.json",
        &[
            "installation/secret-material.json",
            "installation/tenants.json",
            "installation/tenants/tenant-a",
        ],
        "the fixed name of the registry is a second name of a setup record",
    );
    let report = name_run.run.json();
    let refused = finding(&report, "registry.file_unreadable");
    assert!(text_of(refused, "message").contains("was not opened"));
    assert_eq!(
        refused["scope"],
        json!({"kind": "input", "input": "installation_dir"})
    );
    assert!(find(&report, "registry.json_invalid").is_none());
    assert_eq!(
        report["mode"]["files_read_for_content"],
        json!(["oidc_file"])
    );
    assert_eq!(report["mode"]["credential_contents_read"], "none");
    assert_eq!(name_run.run.code, 3);
}

#[test]
fn a_file_named_later_as_a_credential_is_reported() {
    let scenario = Scenario::new();
    let credentials = fixtures::provider_credentials(&scenario.path("credentials"));
    let profiles = scenario.path("profiles.json");
    fixtures::write_providers(&profiles, &fixtures::four_profiles(&credentials));
    let directory = scenario.path("registry");
    fixtures::private_dir(&directory);
    let registry = fixtures::hand_written_registry(&directory, &["tenant-a"]);

    // Control: each option is given the file it describes.
    let (text, json, report) = scenario.diagnose_both([
        "--tenants-file",
        arg(&registry),
        "--providers-file",
        arg(&profiles),
    ]);
    assert_eq!(report["mode"]["credential_contents_read"], "none");
    assert!(find(&report, "reference.file_read_before_it_was_named").is_none());
    assert!(text.stdout.contains(
        "mode       offline; nothing written; no file named as a credential was read; no network\n"
    ));
    assert_ne!(json.code, 1);

    // Both files are given by option, and the registry option names a
    // credential of the profile file. The registry is read first, and nothing
    // read before it names that file: the tool cannot know. Once the profile
    // file is read, the report must say that the content was read.
    let (text, json, report) = scenario.diagnose_both([
        "--tenants-file",
        arg(&credentials.claude_api),
        "--providers-file",
        arg(&profiles),
    ]);
    assert_eq!(
        report["mode"]["credential_contents_read"], "detected_after_read",
        "the report never claims that no credential content was read when one was"
    );
    assert_round_trip(&json, "a report that says a credential content was read");
    let detected = finding(&report, "reference.file_read_before_it_was_named");
    assert_eq!(detected["status"], "error");
    assert_eq!(
        detected["scope"],
        json!({"kind": "file", "file": "tenant_registry"})
    );
    assert!(
        text.stdout.contains(
            "mode       offline; nothing written; a file later named as a credential was read; no network\n"
        ),
        "the text report says the same as the JSON document"
    );
    assert!(entries(&report["mode"], "files_read_for_content").contains(&json!("tenant_registry")));
    assert_eq!(json.code, 3);
    for run in [&text, &json] {
        assert!(
            !run.stdout.contains("synthetic-claude-api-value")
                && !run.stderr.contains("synthetic-claude-api-value"),
            "the content that was read is never printed"
        );
    }
}

// ---- beyond the table: the width of the text report --------------------------

#[test]
fn text_facts_stay_within_the_report_width() {
    let scenario = Scenario::new();
    let config = scenario.historical("config");
    let credentials = fixtures::provider_credentials(&scenario.path("credentials"));
    fixtures::write_providers(
        &config.join("providers.json"),
        &fixtures::four_profiles(&credentials),
    );
    let engine = scenario.path("engine");
    fixtures::private_dir(&engine);
    let program = engine.join("docker");
    fixtures::write_mode(&program, "not a program", 0o755);
    let bundle = scenario.path("bundle.pem");
    fixtures::private_write(&bundle, fixtures::PUBLIC_TEST_ROOT);
    let installation = scenario.prepare(
        "installation",
        &fixtures::manifest_v1_with_issuer(&["tenant-a"], "http://127.0.0.1:9/r"),
    );

    // Every axis has facts here. Every location is a placeholder and every
    // value of the operator is short: no line depends on a path of this
    // machine or on the length of a URL.
    let (text, _, report) = scenario.diagnose_both([
        "--config-dir",
        arg(&config),
        "--installation-dir",
        arg(&installation),
        "--jev-url",
        "https://engine.example.invalid/v1",
        "--jev-ca-file",
        arg(&bundle),
        "--runtime",
        "docker",
        "--runtime-program",
        arg(&program),
    ]);
    assert_eq!(entries(&report["providers"], "profiles").len(), 4);
    assert!(report["engine"].is_object() && report["identity"]["sso"].is_object());
    assert!(report["inference"]["decision_engine_transport"].is_object());
    assert_eq!(entries(&report, "trust").len(), 1);
    assert!(!entries(&report, "credential_references").is_empty());
    assert!(!entries(&report, "data").is_empty());
    let wide: Vec<&str> = text
        .stdout
        .lines()
        .filter(|line| line.chars().count() > 100)
        .collect();
    assert!(wide.is_empty(), "lines wider than 100 columns: {wide:#?}");
    assert!(
        text.stdout
            .contains("  profile \"codex-subscription\"\n          revision 1; client codex; ")
    );
}

// ---- beyond the table: a CA reference into the directories of the keys ------

#[test]
fn ca_reference_below_key_directories_is_never_opened() {
    let manifest = fixtures::manifest_https(&["tenant-a"], Some(fixtures::PUBLIC_TEST_ROOT));
    let credential_reference = "trust.ca_reference_is_a_credential_reference";

    // A moved installation: its registry records the old paths, so the key
    // files that sit in the inspected directory are named by no reference.
    let moved = Scenario::new();
    let original = moved.prepare("installation", &manifest);
    let installation = moved.path("moved");
    fs::rename(&original, &installation).unwrap();
    fixtures::edit_json(&installation.join("oidc.json"), |oidc| {
        oidc["ca_bundle_file"] = "tenants/tenant-a/viewer".into();
    });
    let moved_run = observed_run(
        &moved,
        &["--installation-dir", arg(&installation), "--json"],
    );

    // A staging directory: the entries its files name are not described, and
    // its own key files are still never opened.
    let unpublished = Scenario::new();
    let final_path = unpublished.prepare("installation", &manifest);
    let staging = unpublished.path(&fixtures::staging_name(&final_path));
    fs::rename(&final_path, &staging).unwrap();
    fixtures::edit_json(&staging.join("oidc.json"), |oidc| {
        oidc["ca_bundle_file"] = "tenants/tenant-a/runner".into();
    });
    let staging_run = observed_run(
        &unpublished,
        &["--installation-dir", arg(&staging), "--json"],
    );

    // A historical directory: a private file beside its keys that nothing
    // names, referenced by an OIDC file given alone.
    let historical = Scenario::new();
    let config = historical.historical("config");
    let spare = config.join("secrets/spare-key");
    fixtures::private_write(&spare, fixtures::synthetic_token("spare"));
    let sso = historical.prepare("sso", &manifest);
    fixtures::edit_json(&sso.join("oidc.json"), |oidc| {
        oidc["ca_bundle_file"] = arg(&spare).into();
    });
    let historical_run = observed_run(
        &historical,
        &[
            "--config-dir",
            arg(&config),
            "--oidc-config",
            arg(&sso.join("oidc.json")),
            "--json",
        ],
    );

    for (context, observed, calibration, never) in [
        (
            "moved installation",
            &moved_run,
            "moved/oidc.json",
            "moved/tenants",
        ),
        (
            "staging directory",
            &staging_run,
            &*format!("{}/oidc.json", fixtures::staging_name(&final_path)),
            &*format!("{}/tenants", fixtures::staging_name(&final_path)),
        ),
        (
            "historical directory",
            &historical_run,
            "sso/oidc.json",
            "config/secrets",
        ),
    ] {
        let report = observed.run.json();
        assert_eq!(
            finding(&report, credential_reference)["status"],
            "error",
            "{context}"
        );
        assert!(
            find(&report, "trust.ca_bundle_invalid").is_none(),
            "{context}"
        );
        assert_eq!(report["trust"][0]["read"], false, "{context}");
        assert!(
            !entries(&report["mode"], "files_read_for_content").contains(&json!("oidc_ca_bundle")),
            "{context}"
        );
        assert_eq!(observed.run.code, 3, "{context}");
        assert_never_read(observed, calibration, &[never], context);
    }
}

// ---- increment S: the profile a managed launch selects ------------------------

/// The selection options of one case, as they are given to the tool.
#[derive(Clone, Copy, Default)]
struct Ask {
    client: Option<&'static str>,
    auth_mode: Option<&'static str>,
    profile: Option<&'static str>,
    model: Option<&'static str>,
}

impl Ask {
    fn args(&self) -> Vec<&'static str> {
        let mut args = Vec::new();
        for (flag, value) in [
            ("--client", self.client),
            ("--auth-mode", self.auth_mode),
            ("--client-profile", self.profile),
            ("--model", self.model),
        ] {
            if let Some(value) = value {
                args.extend([flag, value]);
            }
        }
        args
    }

    /// What `agentsandbox run` builds from its options of the same names.
    fn selection(&self) -> ProviderSelection {
        ProviderSelection {
            client: match self.client {
                None | Some("auto") => None,
                Some("codex") => Some(ClientKind::Codex),
                Some("claude-code") => Some(ClientKind::ClaudeCode),
                Some(other) => panic!("fixture defect: {other} is not a client of run"),
            },
            auth_mode: match self.auth_mode {
                None | Some("auto") => AuthPreference::Auto,
                Some("api") => AuthPreference::Api,
                Some("subscription") => AuthPreference::Subscription,
                Some(other) => panic!("fixture defect: {other} is not a mode of run"),
            },
            profile_id: self.profile.map(str::to_owned),
            model: self.model.map(str::to_owned),
            ..ProviderSelection::default()
        }
    }
}

/// What the product's own preparation does with the same selection: its
/// resolution, once it has read and accepted the credential of the chosen
/// profile, or the class of its refusal.
fn product_resolution(
    scenario: &Scenario,
    providers: &Path,
    ask: &Ask,
) -> Result<ProviderResolutionV1, String> {
    let config = agentsandbox::provider::load_config(providers)
        .expect("fixture: the product accepts this profile file");
    agentsandbox::provider::prepare(
        &config,
        &ask.selection(),
        &scenario.work,
        Some("inspect"),
        uuid::Uuid::new_v4(),
    )
    .map(|prepared| prepared.resolution)
    .map_err(|error| {
        error
            .to_string()
            .split_once(':')
            .map_or_else(|| "unprefixed".to_owned(), |(class, _)| class.to_owned())
    })
}

/// The tool on a config directory, with the selection options of `ask`.
fn diagnose_selection(scenario: &Scenario, config: &Path, ask: &Ask) -> (Run, Run, Value) {
    let mut args = vec!["--config-dir", arg(config)];
    args.extend(ask.args());
    scenario.diagnose_both(&args)
}

/// The resolved selection of a report is the resolution of the product.
fn assert_resolved_as_the_product(
    report: &Value,
    resolution: &ProviderResolutionV1,
    context: &str,
) {
    assert_eq!(
        report["providers"]["selection"]["resolved"],
        json!({
            "profile": {"id": resolution.profile_id, "masked": false},
            "revision": resolution.profile_revision,
            "client": resolution.client,
            "auth_mode": resolution.auth_mode,
            "model": resolution.model,
            "chosen_by": resolution.provenance.profile,
        }),
        "{context}: the tool reports the choice of the product"
    );
}

/// What this build cannot know offline about a client stays said as such,
/// whatever the selection: presence, version, qualified versions, account.
fn assert_client_limits_are_stated(report: &Value) {
    for code in [
        "provider.client_presence_not_available",
        "provider.client_version_not_available",
        "provider.qualified_versions_not_available",
        "provider.account_validity_not_available",
    ] {
        let limit = finding(report, code);
        assert_eq!(limit["status"], "not_available", "{code}");
        assert_eq!(limit["scope"], json!({"kind": "build"}), "{code}");
    }
    assert_eq!(
        finding(report, "provider.credential_content_not_checked")["status"],
        "not_checked"
    );
}

#[test]
fn explicit_subscription_is_preserved() {
    let scenario = Scenario::new();
    let config = scenario.historical("config");
    let credentials = fixtures::provider_credentials(&scenario.path("credentials"));
    let providers = config.join("providers.json");
    // Both clients, each with a subscription profile and an API profile.
    fixtures::write_providers(&providers, &fixtures::four_profiles(&credentials));

    let subscription = Some("subscription");
    for (ask, profile, chosen_by, model) in [
        (
            Ask {
                client: Some("codex"),
                auth_mode: subscription,
                ..Ask::default()
            },
            "codex-subscription",
            "auto",
            Value::Null,
        ),
        (
            Ask {
                client: Some("claude-code"),
                auth_mode: subscription,
                model: Some("claude-haiku-5-5"),
                ..Ask::default()
            },
            "claude-subscription",
            "auto",
            json!("claude-haiku-5-5"),
        ),
        (
            Ask {
                auth_mode: subscription,
                profile: Some("claude-subscription"),
                ..Ask::default()
            },
            "claude-subscription",
            "profile",
            Value::Null,
        ),
        // Two subscription candidates: the declared priority decides, and
        // the API profiles are no candidates at all.
        (
            Ask {
                auth_mode: subscription,
                ..Ask::default()
            },
            "codex-subscription",
            "auto",
            Value::Null,
        ),
    ] {
        let context = format!("{:?}", ask.args());
        let (text, json, report) = diagnose_selection(&scenario, &config, &ask);
        let resolution = product_resolution(&scenario, &providers, &ask).expect(
            "positive control: the product resolves this selection and accepts its credential",
        );
        assert_eq!(resolution.auth_mode, AuthMode::Subscription, "{context}");
        assert_resolved_as_the_product(&report, &resolution, &context);

        let selection = &report["providers"]["selection"];
        assert_eq!(selection["requested"]["auth_mode"], "explicit", "{context}");
        assert_eq!(selection["resolved"]["profile"]["id"], profile, "{context}");
        assert_eq!(
            selection["resolved"]["auth_mode"], "subscription",
            "{context}"
        );
        assert_eq!(selection["resolved"]["chosen_by"], chosen_by, "{context}");
        assert_eq!(selection["resolved"]["model"], model, "{context}");
        let resolved = finding(&report, "provider.selection_resolved");
        assert_eq!(resolved["status"], "ok", "{context}");
        assert_eq!(
            resolved["subjects"]["ids"],
            json!([{"id": profile, "masked": false}]),
            "{context}"
        );
        assert!(
            find(&report, "provider.api_billing_notice").is_none(),
            "{context}: an explicit subscription choice selects no billed API profile"
        );
        assert!(
            find(&report, "provider.subscription_never_falls_back_to_api").is_none(),
            "{context}: nothing is refused here"
        );
        assert_client_limits_are_stated(&report);
        assert_eq!(json.code, 0, "{context}");
        assert!(
            text.stdout.contains(&format!(
                "resolved profile \"{profile}\"; revision 1; client "
            )),
            "{context}: the text report shows the resolved profile"
        );
        assert_eq!(
            *input(&report, "auth_mode"),
            json!({"name": "auth_mode", "origin": "cli", "display": "subscription"}),
            "{context}"
        );
    }
}

#[test]
fn refused_subscription_never_becomes_api() {
    let scenario = Scenario::new();
    let credentials = fixtures::provider_credentials(&scenario.path("credentials"));

    // No subscription profile is declared; two valid API profiles are.
    let api_only = scenario.historical("api-only");
    let api_only_file = api_only.join("providers.json");
    fixtures::write_providers(
        &api_only_file,
        &json!({"version": 1, "auto_priority": ["codex-api", "claude-api"], "profiles": [
            fixtures::api_profile("codex-api", "codex", &credentials.codex_api),
            fixtures::api_profile("claude-api", "claude_code", &credentials.claude_api)
        ]}),
    );
    let ask = Ask {
        auth_mode: Some("subscription"),
        ..Ask::default()
    };
    let unrequested = product_resolution(&scenario, &api_only_file, &Ask::default()).expect(
        "positive control: without the request, the product resolves an API profile of this file",
    );
    assert_eq!(unrequested.auth_mode, AuthMode::Api);
    assert_eq!(
        product_resolution(&scenario, &api_only_file, &ask).unwrap_err(),
        "credential_missing",
        "the product refuses a subscription request that no profile answers"
    );
    let (text, json, report) = diagnose_selection(&scenario, &api_only, &ask);
    assert_eq!(
        report["providers"]["selection"],
        json!({
            "requested": {
                "client": "auto", "auth_mode": "explicit", "profile": "auto", "model": "auto"
            },
            "resolved": null
        })
    );
    let refused = finding(&report, "provider.selection_no_profile");
    assert_eq!(refused["status"], "error");
    assert_eq!(
        refused["details"],
        json!({"kind": "reason", "reason": "no_profile_matches"})
    );
    assert_eq!(
        finding(&report, "provider.subscription_never_falls_back_to_api")["status"],
        "info"
    );
    assert!(find(&report, "provider.selection_resolved").is_none());
    assert!(
        find(&report, "provider.api_billing_notice").is_none(),
        "a refused subscription request resolves no API profile"
    );
    assert_client_limits_are_stated(&report);
    assert_eq!(json.code, 3);
    assert!(
        text.stdout
            .contains("resolved none; the product refuses this selection\n")
    );

    // Both modes are declared, and the Codex subscription credential is
    // gone: the subscription profile stays the choice, and the launch asks
    // for a login. The Codex API profile and its valid credential are there.
    let both = scenario.historical("both");
    let both_file = both.join("providers.json");
    fixtures::write_providers(&both_file, &fixtures::four_profiles(&credentials));
    fs::remove_file(&credentials.codex_subscription).unwrap();
    for ask in [
        Ask {
            client: Some("codex"),
            auth_mode: Some("subscription"),
            ..Ask::default()
        },
        Ask {
            profile: Some("codex-subscription"),
            ..Ask::default()
        },
        // No choice at all: the automatic rule prefers the subscription.
        Ask::default(),
    ] {
        let context = format!("{:?}", ask.args());
        assert_eq!(
            product_resolution(&scenario, &both_file, &ask).unwrap_err(),
            "login_required",
            "{context}: the product asks for a login and prepares no API credential"
        );
        let (_, json, report) = diagnose_selection(&scenario, &both, &ask);
        let resolved = &report["providers"]["selection"]["resolved"];
        assert_eq!(
            resolved["profile"],
            json!({"id": "codex-subscription", "masked": false}),
            "{context}"
        );
        assert_eq!(resolved["auth_mode"], "subscription", "{context}");
        let login = finding(&report, "provider.login_required");
        assert_eq!(login["status"], "error", "{context}");
        assert_eq!(
            login["subjects"]["ids"],
            json!([{"id": "codex-subscription", "masked": false}]),
            "{context}"
        );
        finding(&report, "provider.subscription_never_falls_back_to_api");
        assert!(
            find(&report, "provider.api_billing_notice").is_none(),
            "{context}"
        );
        assert!(
            find(&report, "provider.credential_missing").is_none()
                && find(&report, "provider.unselected_profile_reference_absent").is_none(),
            "{context}: a missing subscription file is a login to make, for the resolved profile"
        );
        assert_client_limits_are_stated(&report);
        assert_eq!(json.code, 3, "{context}");
    }

    // Control: billed API use is reached by an explicit request, and only so.
    let ask = Ask {
        client: Some("codex"),
        auth_mode: Some("api"),
        ..Ask::default()
    };
    let resolution = product_resolution(&scenario, &both_file, &ask)
        .expect("positive control: the product resolves the API profile on request");
    assert_eq!(resolution.auth_mode, AuthMode::Api);
    let (_, json, report) = diagnose_selection(&scenario, &both, &ask);
    assert_resolved_as_the_product(&report, &resolution, "explicit API request");
    let notice = finding(&report, "provider.api_billing_notice");
    assert_eq!(notice["status"], "warning");
    assert_eq!(
        notice["subjects"]["ids"],
        json!([{"id": "codex-api", "masked": false}])
    );
    let unselected = finding(&report, "provider.unselected_profile_reference_absent");
    assert_eq!(
        unselected["details"],
        json!({"kind": "would_refuse_with", "class": "login_required"})
    );
    assert_eq!(
        unselected["subjects"]["ids"],
        json!([{"id": "codex-subscription", "masked": false}])
    );
    assert!(find(&report, "provider.login_required").is_none());
    assert!(find(&report, "provider.subscription_never_falls_back_to_api").is_none());
    assert_eq!(json.code, 0);
}

#[test]
fn auto_resolution_and_billing_notice() {
    let scenario = Scenario::new();
    let credentials = fixtures::provider_credentials(&scenario.path("credentials"));
    let auto = Ask::default();
    let requested_auto =
        json!({"client": "auto", "auth_mode": "auto", "profile": "auto", "model": "auto"});

    // Subscription and API profiles together: the subscription wins, and no
    // billing notice is due.
    let mixed = scenario.historical("mixed");
    fixtures::write_providers(
        &mixed.join("providers.json"),
        &fixtures::four_profiles(&credentials),
    );
    let resolution = product_resolution(&scenario, &mixed.join("providers.json"), &auto)
        .expect("positive control: the product resolves the automatic selection");
    assert_eq!(resolution.auth_mode, AuthMode::Subscription);
    let (_, json, report) = diagnose_selection(&scenario, &mixed, &auto);
    assert_resolved_as_the_product(&report, &resolution, "subscription and API profiles");
    assert_eq!(
        report["providers"]["selection"]["requested"],
        requested_auto
    );
    assert!(find(&report, "provider.api_billing_notice").is_none());
    assert!(
        entries(&report, "inputs").iter().all(|input| ![
            "client",
            "auth_mode",
            "client_profile",
            "model"
        ]
        .contains(&input["name"].as_str().unwrap())),
        "an option that was not given is not listed as an input"
    );
    assert_eq!(json.code, 0);

    // The profile file the product itself writes: API profiles only, with
    // their priority. The automatic selection resolves one, and says that
    // its usage is billed.
    let generated = scenario.path("generated");
    let env_file = scenario.path("providers.env");
    fixtures::private_write(
        &env_file,
        "OPENAI_API_KEY=synthetic-codex-api-value-0000\nANTHROPIC_API_KEY=synthetic-claude-api-value-0000\n",
    );
    let written = scenario.product([
        "providers",
        "init",
        "--config-dir",
        arg(&generated),
        "--env-file",
        arg(&env_file),
    ]);
    assert_eq!(
        written.code,
        Some(0),
        "fixture defect: providers init refused: {}",
        written.stderr_text()
    );
    let resolution = product_resolution(&scenario, &generated.join("providers.json"), &auto)
        .expect("positive control: the product resolves its own profile file");
    assert_eq!(resolution.auth_mode, AuthMode::Api);
    let (text, json, report) = diagnose_selection(&scenario, &generated, &auto);
    assert_resolved_as_the_product(&report, &resolution, "the file providers init writes");
    let notice = finding(&report, "provider.api_billing_notice");
    assert_eq!(notice["status"], "warning");
    assert!(text_of(notice, "message").contains("billed by the provider"));
    assert!(
        text.stdout
            .contains("  [warning] provider.api_billing_notice\n")
    );
    assert_eq!(json.code, 0, "a notice is no refusal");

    // A default profile decides before the priority does.
    let defaulted = scenario.historical("defaulted");
    fixtures::write_providers(
        &defaulted.join("providers.json"),
        &json!({
            "version": 1,
            "default_profile": "claude-subscription",
            "auto_priority": ["codex-subscription", "claude-subscription"],
            "profiles": [
                fixtures::subscription_profile("codex-subscription", "codex", &credentials.codex_subscription),
                fixtures::subscription_profile("claude-subscription", "claude_code", &credentials.claude_subscription)
            ]
        }),
    );
    let resolution = product_resolution(&scenario, &defaulted.join("providers.json"), &auto)
        .expect("positive control: the product resolves the default profile");
    let (_, _, report) = diagnose_selection(&scenario, &defaulted, &auto);
    assert_resolved_as_the_product(&report, &resolution, "a default profile");
    assert_eq!(
        report["providers"]["selection"]["resolved"]["chosen_by"],
        "default_profile"
    );
    assert_eq!(
        report["providers"]["selection"]["resolved"]["profile"]["id"],
        "claude-subscription"
    );

    // Two equal candidates and nothing that decides: refused, never guessed.
    let equal = scenario.historical("equal");
    fixtures::write_providers(
        &equal.join("providers.json"),
        &json!({"version": 1, "profiles": [
            fixtures::api_profile("first", "codex", &credentials.codex_api),
            fixtures::api_profile("second", "claude_code", &credentials.claude_api)
        ]}),
    );
    assert_eq!(
        product_resolution(&scenario, &equal.join("providers.json"), &auto).unwrap_err(),
        "ambiguous_configuration"
    );
    let (_, json, report) = diagnose_selection(&scenario, &equal, &auto);
    let ambiguous = finding(&report, "provider.selection_ambiguous");
    assert_eq!(ambiguous["status"], "error");
    assert_eq!(
        ambiguous["details"],
        json!({"kind": "reason", "reason": "several_match_without_priority"})
    );
    assert_eq!(
        report["providers"]["selection"],
        json!({"requested": requested_auto, "resolved": null})
    );
    assert!(find(&report, "provider.selection_resolved").is_none());
    assert!(find(&report, "provider.api_billing_notice").is_none());
    assert_eq!(json.code, 3);
    // The same file with a client named: one candidate is left.
    let named = Ask {
        client: Some("claude-code"),
        ..Ask::default()
    };
    let resolution = product_resolution(&scenario, &equal.join("providers.json"), &named)
        .expect("positive control: the product resolves the only profile of the named client");
    let (_, json, report) = diagnose_selection(&scenario, &equal, &named);
    assert_resolved_as_the_product(&report, &resolution, "a named client");
    assert_eq!(
        *input(&report, "client"),
        json!({"name": "client", "origin": "cli", "display": "claude-code"})
    );
    assert_eq!(json.code, 0);
}

#[test]
fn selection_refusals_are_classified() {
    let scenario = Scenario::new();
    let credentials = fixtures::provider_credentials(&scenario.path("credentials"));
    let codex_only = json!({"version": 1, "profiles": [
        fixtures::api_profile("codex-api", "codex", &credentials.codex_api)
    ]});
    let two_equal = json!({"version": 1, "profiles": [
        fixtures::api_profile("first", "codex", &credentials.codex_api),
        fixtures::api_profile("second", "codex", &credentials.codex_api)
    ]});
    // One case per refusal of the product choice: the profile file, the
    // selection, then the code, the reason and the class the product gives.
    let cases = [
        (
            &codex_only,
            Ask {
                auth_mode: Some("subscription"),
                profile: Some("codex-api"),
                ..Ask::default()
            },
            "provider.selection_ambiguous",
            "explicit_conflict",
            "ambiguous_configuration",
        ),
        (
            &two_equal,
            Ask::default(),
            "provider.selection_ambiguous",
            "several_match_without_priority",
            "ambiguous_configuration",
        ),
        (
            &codex_only,
            Ask {
                profile: Some("not-declared"),
                ..Ask::default()
            },
            "provider.selection_no_profile",
            "requested_profile_not_configured",
            "credential_missing",
        ),
        (
            &codex_only,
            Ask {
                client: Some("claude-code"),
                ..Ask::default()
            },
            "provider.selection_no_profile",
            "no_profile_matches",
            "credential_missing",
        ),
        (
            &codex_only,
            Ask {
                profile: Some("not an identifier"),
                ..Ask::default()
            },
            "provider.selection_invalid",
            "profile_identifier",
            "unprefixed",
        ),
        (
            &codex_only,
            Ask {
                model: Some("not a model"),
                ..Ask::default()
            },
            "provider.selection_invalid",
            "model_identifier",
            "unprefixed",
        ),
    ];
    for (index, (profiles, ask, code, reason, class)) in cases.into_iter().enumerate() {
        let config = scenario.historical(&format!("config-{index}"));
        let providers = config.join("providers.json");
        fixtures::write_providers(&providers, profiles);
        assert_eq!(
            product_resolution(&scenario, &providers, &ask).unwrap_err(),
            class,
            "{reason}: the class of the product's refusal"
        );
        // The text of the refusal, from the choice the tool itself calls.
        let loaded = agentsandbox::provider::load_config(&providers).unwrap();
        let refusal = agentsandbox::provider::choose_profile(&loaded, &ask.selection())
            .expect_err("the product choice refuses this selection")
            .to_string();
        let (text, json, report) = diagnose_selection(&scenario, &config, &ask);
        let refused = finding(&report, code);
        assert_eq!(refused["status"], "error", "{reason}");
        assert_eq!(
            refused["details"],
            json!({"kind": "reason", "reason": reason}),
            "{reason}: a changed product text would degrade the reason"
        );
        assert_eq!(
            refused["scope"],
            json!({"kind": "provider_profiles"}),
            "{reason}"
        );
        assert_eq!(
            report["providers"]["selection"]["resolved"],
            Value::Null,
            "{reason}"
        );
        assert!(
            find(&report, "provider.selection_resolved").is_none(),
            "{reason}"
        );
        finding(&report, "provider.configuration_valid");
        assert_eq!(json.code, 3, "{reason}");
        for run in [&text, &json] {
            assert!(
                !run.stdout.contains(&refusal) && !run.stderr.contains(&refusal),
                "{reason}: the text of the product is never copied"
            );
        }
    }

    // Selection options without a loaded profile file are never accepted
    // silently: the report says that nothing was resolved.
    let unresolved = |report: &Value| {
        let finding = finding(report, "provider.selection_not_resolved");
        assert_eq!(finding["status"], "not_checked");
        assert_eq!(
            text_of(finding, "requires"),
            "a provider profile file that the product loader accepts"
        );
    };
    let bare = scenario.historical("bare");
    let (_, json, report) =
        scenario.diagnose_both(["--config-dir", arg(&bare), "--client", "codex"]);
    unresolved(&report);
    finding(&report, "provider.file_absent");
    assert_eq!(report["providers"]["selection"], Value::Null);
    assert_eq!(json.code, 0);
    let broken = scenario.historical("broken");
    fixtures::private_write(&broken.join("providers.json"), "{ not json");
    let (_, json, report) =
        scenario.diagnose_both(["--config-dir", arg(&broken), "--model", "some-model"]);
    unresolved(&report);
    finding(&report, "provider.configuration_invalid");
    assert_eq!(json.code, 3);
    let installation = scenario.prepare("installation", &fixtures::manifest_v2(&["tenant-a"]));
    let (_, _, report) = scenario.diagnose_both([
        "--installation-dir",
        arg(&installation),
        "--auth-mode",
        "subscription",
    ]);
    unresolved(&report);
    finding(&report, "provider.not_inspected");
    assert_eq!(report["providers"], Value::Null);
    // Without any selection option, nothing is said about a selection.
    let (_, _, report) = scenario.diagnose_both(["--config-dir", arg(&bare)]);
    assert!(find(&report, "provider.selection_not_resolved").is_none());
}

#[test]
fn selection_never_reads_credentials() {
    let scenario = Scenario::new();
    let config = scenario.historical("config");
    let credentials = fixtures::provider_credentials(&scenario.path("credentials"));
    fixtures::write_providers(
        &config.join("providers.json"),
        &fixtures::four_profiles(&credentials),
    );
    let args = [
        "--config-dir",
        arg(&config),
        "--client",
        "claude-code",
        "--auth-mode",
        "api",
        "--model",
        "claude-haiku-5-5",
        "--json",
    ];
    let first = observed_run(&scenario, &args);
    // The selection is resolved, so the choice of the product did run.
    let report = first.run.json();
    assert_eq!(
        report["providers"]["selection"]["resolved"],
        json!({
            "profile": {"id": "claude-api", "masked": false},
            "revision": 1,
            "client": "claude_code",
            "auth_mode": "api",
            "model": "claude-haiku-5-5",
            "chosen_by": "auto"
        })
    );
    for path in [
        &credentials.codex_api,
        &credentials.claude_api,
        &credentials.codex_subscription,
        &credentials.claude_subscription,
    ] {
        fixtures::overwrite(path, format!("not a credential {CANARY}\n"));
    }
    let second = scenario.diagnose(args);
    assert_eq!(
        first.run.stdout, second.stdout,
        "the selection does not depend on the content of any credential"
    );
    assert_no_canary(&second, &[CANARY], "content of a credential");
    assert_never_read(
        &first,
        "config/providers.json",
        &["config/secrets", "config/mounted-secrets", "credentials"],
        "a selection with every option given",
    );
    assert_eq!(
        report["mode"]["files_read_for_content"],
        json!(["provider_profiles"])
    );
    if let Some(accessed) = first.accessed() {
        assert_eq!(
            accessed.iter().collect::<Vec<_>>(),
            [Path::new("config/providers.json")],
            "nothing but the profile file is opened or listed to resolve a selection"
        );
    }
}

#[test]
fn selection_values_cannot_forge_or_leak() {
    let scenario = Scenario::new();
    let config = scenario.historical("config");
    let credentials = fixtures::provider_credentials(&scenario.path("credentials"));
    let providers = config.join("providers.json");
    let credential_like = "sk-abcdefghijkl";
    fixtures::write_providers(
        &providers,
        &json!({"version": 1, "auto_priority": ["ops-token", credential_like], "profiles": [
            fixtures::api_profile("ops-token", "codex", &credentials.codex_api),
            fixtures::api_profile(credential_like, "claude_code", &credentials.claude_api)
        ]}),
    );
    let run = |ask: &[&str]| {
        let mut args = vec!["--config-dir", arg(&config)];
        args.extend(ask);
        scenario.diagnose_both(&args)
    };

    // A declared profile whose identifier looks like a credential, asked for
    // by name: resolved, and shown by its placeholder wherever it appears.
    let ask = Ask {
        profile: Some(credential_like),
        ..Ask::default()
    };
    let resolution = product_resolution(&scenario, &providers, &ask)
        .expect("positive control: the product resolves this identifier");
    assert_eq!(resolution.profile_id, credential_like);
    let (text, json, report) = run(&ask.args());
    assert_eq!(
        report["providers"]["selection"]["resolved"]["profile"],
        json!({"id": "[masked profile identifier 1]", "masked": true})
    );
    finding(&report, "provider.selection_resolved");
    finding(&report, "output.value_masked");
    assert_no_canary(&text, &[credential_like], "a credential-like profile name");
    assert_no_canary(&json, &[credential_like], "a credential-like profile name");

    // A model the product accepts, and that the output filter would rewrite.
    let model = "sk-mnopqrstuvwxyz0123";
    let ask = Ask {
        client: Some("codex"),
        model: Some(model),
        ..Ask::default()
    };
    let resolution = product_resolution(&scenario, &providers, &ask)
        .expect("positive control: the product accepts this model identifier");
    assert_eq!(resolution.model.as_deref(), Some(model));
    let (text, json, report) = run(&ask.args());
    assert_eq!(
        report["providers"]["selection"]["resolved"]["profile"]["id"],
        "ops-token"
    );
    assert!(report["providers"]["selection"]["resolved"]["model"].is_string());
    finding(&report, "output.value_masked");
    assert_no_canary(&text, &[model], "a credential-like model name");
    assert_no_canary(&json, &[model], "a credential-like model name");

    // Control characters in the two free options: the product refuses the
    // value, and the text of the report cannot be forged with it.
    let forged = "ops-token\n  [ok] provider.selection_resolved";
    let (text, json, report) = run(&["--client-profile", forged]);
    assert_eq!(
        finding(&report, "provider.selection_invalid")["details"],
        json!({"kind": "reason", "reason": "profile_identifier"})
    );
    assert_eq!(
        input(&report, "client_profile")["display"],
        "ops-token\\u{a}  [ok] provider.selection_resolved"
    );
    assert!(
        text.stdout
            .lines()
            .all(|line| line != "  [ok] provider.selection_resolved"),
        "a value of the command line starts no line of the report"
    );
    assert_eq!(json.code, 3);
    let (text, _, report) = run(&["--model", "model\u{1b}[2J\u{202e}"]);
    assert_eq!(
        finding(&report, "provider.selection_invalid")["details"],
        json!({"kind": "reason", "reason": "model_identifier"})
    );
    assert_eq!(
        input(&report, "model")["display"],
        "model\\u{1b}[2J\\u{202e}"
    );
    assert!(!text.stdout.contains('\u{1b}') && !text.stdout.contains('\u{202e}'));
}

// ---- beyond the table: the guides and their examples --------------------------

#[test]
fn documented_examples_are_the_real_output() {
    let scenario = Scenario::new();

    // A prepared installation: the public manifest, given to the real
    // `setup prepare` as the guides write it.
    let manifest = scenario.path("manifest.json");
    fixtures::private_write(&manifest, fixtures::LOCAL_DEMO_MANIFEST);
    let installation = scenario.path("installation");
    let prepared = scenario.product([
        "setup",
        "prepare",
        "--input",
        arg(&manifest),
        "--output-dir",
        arg(&installation),
    ]);
    assert_eq!(
        prepared.code,
        Some(0),
        "fixture defect: setup prepare refused the public manifest: {}",
        prepared.stderr_text()
    );
    let (text, json, _) = scenario.diagnose_both(["--installation-dir", arg(&installation)]);
    assert_eq!(json.code, 0);
    assert_documented("report.prepared-v2.txt", &text.stdout);
    assert_documented("report.prepared-v2.json", &json.stdout);

    // Provider profiles: a directory created by the real `init`, the public
    // profile file, and two empty private files where two of its three
    // references point. No credential is opened, so no content is needed.
    let config = scenario.path("try");
    let initialised = scenario.product(["init", "--config-dir", arg(&config)]);
    assert_eq!(
        initialised.code,
        Some(0),
        "fixture defect: init refused a new directory: {}",
        initialised.stderr_text()
    );
    let profiles = config.join("providers.two-modes.json");
    fixtures::private_write(&profiles, DOCUMENTED_PROFILES);
    for reference in ["secrets/providers.env", "secrets/claude-subscription.env"] {
        fixtures::private_write(&config.join(reference), "");
    }
    let (text, json, report) = scenario.diagnose_both([
        "--config-dir",
        arg(&config),
        "--providers-file",
        arg(&profiles),
    ]);
    // The automatic selection resolves the Codex subscription profile, whose
    // file is absent: the launch would ask for a login, and no API profile
    // answers in its place.
    assert_eq!(json.code, 3);
    assert_eq!(
        report["providers"]["selection"]["resolved"]["profile"],
        json!({"id": "codex-subscription", "masked": false})
    );
    assert_eq!(
        finding(&report, "provider.login_required")["subjects"]["ids"],
        json!([{"id": "codex-subscription", "masked": false}])
    );
    finding(&report, "provider.subscription_never_falls_back_to_api");
    assert!(find(&report, "provider.api_billing_notice").is_none());
    assert_documented("report.providers-two-modes.txt", &text.stdout);
    assert_documented("report.providers-two-modes.json", &json.stdout);

    // The same file with an explicit choice: the Claude subscription, whose
    // file exists. The absent file then belongs to a profile that this
    // selection does not resolve.
    let (text, json, report) = scenario.diagnose_both([
        "--config-dir",
        arg(&config),
        "--providers-file",
        arg(&profiles),
        "--client",
        "claude-code",
        "--auth-mode",
        "subscription",
    ]);
    assert_eq!(json.code, 0);
    assert_eq!(
        report["providers"]["selection"]["resolved"]["profile"],
        json!({"id": "claude-subscription", "masked": false})
    );
    assert_eq!(
        finding(&report, "provider.unselected_profile_reference_absent")["details"],
        json!({"kind": "would_refuse_with", "class": "login_required"})
    );
    assert_documented("report.providers-explicit-subscription.txt", &text.stdout);
    assert_documented("report.providers-explicit-subscription.json", &json.stdout);
}

/// The fenced blocks of a guide: the word after the fence, then the content.
fn fenced_blocks(guide: &str) -> Vec<(&str, String)> {
    let mut blocks = Vec::new();
    let mut open: Option<(&str, String)> = None;
    for line in guide.lines() {
        if let Some((info, mut content)) = open.take() {
            if line == "```" {
                blocks.push((info, content));
            } else {
                content.push_str(line);
                content.push('\n');
                open = Some((info, content));
            }
        } else if let Some(info) = line.strip_prefix("```") {
            open = Some((info.trim(), String::new()));
        }
    }
    assert!(open.is_none(), "a fenced block of a guide is not closed");
    blocks
}

/// The level of every heading of a guide, outside its fenced blocks.
fn heading_levels(guide: &str) -> Vec<usize> {
    let mut fenced = false;
    let mut levels = Vec::new();
    for line in guide.lines() {
        if line.starts_with("```") {
            fenced = !fenced;
        } else if !fenced && line.starts_with('#') {
            levels.push(
                line.chars()
                    .take_while(|character| *character == '#')
                    .count(),
            );
        }
    }
    levels
}

fn field_names(value: &Value, names: &mut BTreeSet<String>) {
    match value {
        Value::Object(fields) => {
            for (name, inner) in fields {
                names.insert(name.clone());
                field_names(inner, names);
            }
        }
        Value::Array(items) => {
            for inner in items {
                field_names(inner, names);
            }
        }
        _ => {}
    }
}

#[test]
fn guides_follow_the_binary() {
    let scenario = Scenario::new();
    let guides_directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/setup/diagnostics");
    let help = scenario.diagnose(["--help"]).stdout;
    let options: BTreeSet<&str> = help
        .split(|character: char| character.is_whitespace() || character == ',')
        .filter(|word| word.starts_with("--"))
        .collect();
    assert!(
        options.contains("--config-dir") && options.contains("--show-paths"),
        "calibration: the options are read from --help: {options:?}"
    );
    let mut documented_fields = BTreeSet::new();
    for (name, report) in DOCUMENTED_REPORTS {
        if name.ends_with(".json") {
            field_names(
                &serde_json::from_str(report).expect("a documented JSON report is JSON"),
                &mut documented_fields,
            );
        }
    }
    assert!(
        documented_fields.contains("files_read_for_content"),
        "calibration: the fields are read from the documented reports"
    );

    for (name, guide) in GUIDES {
        // One row per code, with the status, the axis and the basis of the
        // catalogue; what resolves a not_checked item is quoted as printed.
        for entry in CATALOGUE {
            let row = format!(
                "| `{}` | `{}` | `{}` | `{}` |",
                entry.code.as_str(),
                entry.status.as_str(),
                entry.axis.as_str(),
                entry.basis.as_str()
            );
            assert!(guide.contains(&row), "{name} has no catalogue row {row}");
            if let Some(requires) = entry.requires {
                assert!(
                    guide.contains(&format!("| `{requires}` |")),
                    "{name} does not quote what resolves {}",
                    entry.code.as_str()
                );
            }
        }

        // Every option the binary accepts is documented, and a documented
        // command line of the tool uses no other option.
        for option in &options {
            let documented = match *option {
                "--help" | "--version" => {
                    guide.contains(&format!("\"$AIGUARD_DIAGNOSE\" {option}\n"))
                }
                _ => {
                    guide.contains(&format!("| `{option}` "))
                        || guide.contains(&format!("| `{option} "))
                }
            };
            assert!(documented, "{name} does not document the option {option}");
        }
        let mut tool_commands = 0;
        let mut container_commands = 0;
        for (info, content) in fenced_blocks(guide) {
            match info {
                "sh" => {
                    for line in content
                        .lines()
                        .filter(|line| line.starts_with("\"$AIGUARD_DIAGNOSE\""))
                    {
                        tool_commands += 1;
                        for word in line
                            .split_whitespace()
                            .filter(|word| word.starts_with("--"))
                        {
                            assert!(
                                options.contains(word),
                                "{name} documents {word}, which the binary does not accept"
                            );
                        }
                    }
                    // In a container command the tool is the entrypoint: its
                    // own command line is what follows the image.
                    for command in content.replace("\\\n", " ").lines() {
                        let mut words = command
                            .split_whitespace()
                            .skip_while(|word| *word != "ai-guard-diagnose");
                        if words.next().is_none() {
                            continue;
                        }
                        container_commands += 1;
                        for word in words.skip(1).filter(|word| word.starts_with("--")) {
                            assert!(
                                options.contains(word),
                                "{name} documents {word} for the tool in a container, which the binary does not accept"
                            );
                        }
                    }
                }
                // A report shown in a guide is an excerpt of a documented
                // report, which is the real output.
                "text" | "json" => assert!(
                    DOCUMENTED_REPORTS
                        .iter()
                        .any(|(_, report)| report.contains(&content)),
                    "{name} shows a report that is in no documented example:\n{content}"
                ),
                other => panic!("{name} has a fenced block of an unexpected kind: {other:?}"),
            }
        }
        assert!(
            tool_commands >= 10,
            "calibration: the command lines of the tool are found in {name}"
        );
        assert!(
            container_commands >= 2,
            "calibration: the container command lines of the tool are found in {name}"
        );

        // Every field of the documented JSON reports is described.
        for field in &documented_fields {
            assert!(
                guide.contains(&format!("`{field}`")),
                "{name} does not describe the field {field}"
            );
        }

        // The closed texts of a refusal and of a usage error are quoted.
        for refusal in [
            DiagnoseError::InvalidInput,
            DiagnoseError::ProtectedOutput,
            DiagnoseError::OutputTooLarge,
            DiagnoseError::OutputUnavailable,
            DiagnoseError::Internal,
        ] {
            assert!(
                guide.contains(&format!("`{refusal}`")),
                "{name} does not quote the refusal {refusal:?}"
            );
        }
        for sentence in [
            "unknown option or unexpected argument",
            "an option has a value outside its closed set",
            "invalid command line",
        ] {
            assert!(
                guide.contains(&format!("`{sentence}`")),
                "{name} does not quote the usage sentence {sentence:?}"
            );
        }

        // Every example is linked, and every relative link leads to a file.
        for example in DOCUMENTED_REPORTS
            .iter()
            .map(|(file, _)| *file)
            .chain(["providers.two-modes.json"])
        {
            assert!(
                guide.contains(&format!("(examples/{example})")),
                "{name} does not link the example {example}"
            );
        }
        let mut links = 0;
        for (start, _) in guide.match_indices("](") {
            let target = &guide[start + 2..];
            let target = &target[..target.find(')').expect("a link is closed")];
            if target.contains("://") {
                continue;
            }
            links += 1;
            let file = target.split('#').next().unwrap_or(target);
            assert!(
                guides_directory.join(file).is_file(),
                "{name}: the link {target} leads to no file"
            );
        }
        assert!(links >= 10, "calibration: the links of {name} are found");
    }

    // The two languages carry the same commands, the same examples and the
    // same structure.
    let [(_, english), (_, french)] = GUIDES;
    assert_eq!(
        fenced_blocks(english),
        fenced_blocks(french),
        "both guides show the same commands and the same examples"
    );
    assert_eq!(
        heading_levels(english),
        heading_levels(french),
        "both guides have the same headings"
    );
    let table_rows = |guide: &str| guide.lines().filter(|line| line.starts_with('|')).count();
    assert_eq!(
        table_rows(english),
        table_rows(french),
        "both guides have the same table rows"
    );
}

// ---- review: what the files of a staging directory record ---------------------

#[test]
fn files_recorded_by_a_staging_directory_are_never_opened() {
    let credential_reference = "trust.ca_reference_is_a_credential_reference";
    let scenario = Scenario::new();
    // A retained staging directory beside its final path. Its registry and
    // its OIDC file record the final paths: no inspected directory reserves
    // them, and nothing they name is described.
    let final_path = scenario.prepare("both", &fixtures::manifest_v1(&["tenant-a"]));
    let staging_name = fixtures::staging_name(&final_path);
    let staging = scenario.path(&staging_name);
    fs::rename(&final_path, &staging).unwrap();
    fixtures::copy_tree(&staging, &final_path);
    let registry = format!("{staging_name}/tenants.json");
    let runner = final_path.join("tenants/tenant-a/runner");
    let viewer = final_path.join("tenants/tenant-a/viewer");
    let store = final_path.join("tenants/tenant-a/audit.sqlite");
    let audit = final_path.join("authentication.sqlite");
    let recorded = fixtures::read_json(&staging.join("tenants.json"));
    let recorded_oidc = fixtures::read_json(&staging.join("oidc.json"));
    for (field, path) in [
        (&recorded["tenants"][0]["runner_token_file"], &runner),
        (&recorded["tenants"][0]["viewer_token_file"], &viewer),
        (&recorded["tenants"][0]["data"], &store),
        (&recorded_oidc["authentication_audit"]["path"], &audit),
    ] {
        assert_eq!(
            *field,
            arg(path),
            "fixture: the files of the staging directory record the final paths"
        );
        assert!(path.is_file(), "fixture: {path:?} exists at the final path");
    }

    let staging_arg = arg(&staging);
    let keys = observed_run(
        &scenario,
        &[
            "--installation-dir",
            staging_arg,
            "--jev-ca-file",
            arg(&viewer),
            "--gateway-ca-file",
            arg(&runner),
            "--json",
        ],
    );
    let stores = observed_run(
        &scenario,
        &[
            "--installation-dir",
            staging_arg,
            "--jev-ca-file",
            arg(&audit),
            "--gateway-ca-file",
            arg(&store),
            "--json",
        ],
    );
    let other_options = observed_run(
        &scenario,
        &[
            "--installation-dir",
            staging_arg,
            "--providers-file",
            arg(&runner),
            "--runtime",
            "docker",
            "--runtime-program",
            arg(&viewer),
            "--json",
        ],
    );
    // The one read that the order of the reads does not prevent. A profile
    // file is read before the OIDC file of the directory, and that OIDC file
    // is what records the authentication audit: given as the profile file,
    // the audit is read first and reported afterwards.
    let audit_as_profiles_text = scenario.diagnose([
        "--installation-dir",
        staging_arg,
        "--providers-file",
        arg(&audit),
    ]);
    let audit_as_profiles = observed_run(
        &scenario,
        &[
            "--installation-dir",
            staging_arg,
            "--providers-file",
            arg(&audit),
            "--json",
        ],
    );

    // A directory with a staging name whose registry records paths inside it,
    // without any setup record: nothing in it is described either.
    let named = ".setup-0123456789abcdef01234567";
    let undescribed = scenario.path(named);
    fixtures::private_dir(&undescribed);
    fixtures::hand_written_registry(&undescribed, &["tenant-a"]);
    let undescribed_run = observed_run(
        &scenario,
        &[
            "--installation-dir",
            arg(&undescribed),
            "--gateway-ca-file",
            arg(&undescribed.join("tenant-a/runner")),
            "--json",
        ],
    );

    // The CA reference of the OIDC file of the staging directory: a key file
    // that its registry records, then the authentication audit that the OIDC
    // file itself records.
    let mut reference_runs = Vec::new();
    for (context, target) in [
        ("a CA reference to a recorded key file", &runner),
        (
            "a CA reference to the recorded authentication audit",
            &audit,
        ),
    ] {
        fixtures::edit_json(&staging.join("oidc.json"), |oidc| {
            oidc["ca_bundle_file"] = arg(target).into();
        });
        reference_runs.push((
            context,
            observed_run(&scenario, &["--installation-dir", staging_arg, "--json"]),
        ));
    }

    for (context, observed) in [
        ("recorded key files given as CA bundles", &keys),
        ("recorded stores given as CA bundles", &stores),
        (
            "a recorded key file given as a profile file and as an engine program",
            &other_options,
        ),
        (
            "the recorded authentication audit given as a profile file",
            &audit_as_profiles,
        ),
        (
            "a registry that is read and not described",
            &undescribed_run,
        ),
    ]
    .into_iter()
    .chain(
        reference_runs
            .iter()
            .map(|(context, observed)| (*context, observed)),
    ) {
        eprintln!(
            "OBSERVED entries read with {context}: {:?}",
            observed.accessed()
        );
    }

    for (context, observed) in [
        ("recorded key files given as CA bundles", &keys),
        ("recorded stores given as CA bundles", &stores),
    ] {
        assert_never_read(observed, &registry, &["both"], context);
        let report = observed.run.json();
        finding(&report, "layout.staging_and_final_both_exist");
        let refused: Vec<&Value> = findings(&report)
            .into_iter()
            .filter(|finding| finding["code"] == credential_reference)
            .map(|finding| &finding["scope"]["file"])
            .collect();
        assert_eq!(
            refused,
            ["decision_engine_ca_bundle", "gateway_ca_bundle"],
            "{context}"
        );
        for code in [
            "trust.ca_bundle_invalid",
            "trust.ca_unavailable_or_not_private",
        ] {
            assert!(find(&report, code).is_none(), "{context}: {code}");
        }
        assert!(
            entries(&report, "trust")
                .iter()
                .all(|entry| entry["read"] == false),
            "{context}"
        );
        assert_eq!(
            report["mode"]["files_read_for_content"],
            json!(["tenant_registry", "oidc_file"]),
            "{context}"
        );
        assert_eq!(
            report["mode"]["credential_contents_read"], "none",
            "{context}"
        );
        // Registered, and still not described: the report lists the setup
        // records of the staging directory and nothing at the final path.
        assert!(
            entries(&report, "credential_references")
                .iter()
                .all(|entry| entry["role"] == "setup_material"),
            "{context}"
        );
        assert!(entries(&report, "data").is_empty(), "{context}");
        assert_eq!(observed.run.code, 3, "{context}");
    }

    let context = "a recorded key file given as a profile file and as an engine program";
    assert_never_read(&other_options, &registry, &["both"], context);
    let report = other_options.run.json();
    assert!(
        text_of(
            finding(&report, "provider.file_unavailable_or_not_private"),
            "message"
        )
        .contains("was not opened")
    );
    assert_eq!(report["providers"]["file"]["state"], "refused");
    assert!(find(&report, "provider.configuration_invalid").is_none());
    assert_eq!(
        finding(&report, "engine.program_is_a_credential_reference")["status"],
        "error"
    );
    assert!(find(&report, "engine.program_refused").is_none());
    assert_eq!(report["mode"]["credential_contents_read"], "none");

    // The audit is read as a profile file, and the report says so: it never
    // claims that no such file was read.
    let context = "the recorded authentication audit given as a profile file";
    assert!(
        fs::metadata(&audit).unwrap().len() <= 64 * 1024,
        "fixture: the audit is within the bound of the profile loader, which then reads it"
    );
    let report = audit_as_profiles.run.json();
    assert_eq!(
        report["mode"]["files_read_for_content"],
        json!(["tenant_registry", "oidc_file", "provider_profiles"]),
        "{context}"
    );
    assert_eq!(
        report["mode"]["credential_contents_read"], "detected_after_read",
        "{context}: the read is detected once the OIDC file has named the audit"
    );
    let detected = finding(&report, "reference.file_read_before_it_was_named");
    assert_eq!(detected["status"], "error", "{context}");
    assert_eq!(
        detected["scope"],
        json!({"kind": "file", "file": "provider_profiles"}),
        "{context}: the detection names the role of the file that was opened"
    );
    assert_eq!(
        finding(&report, "provider.configuration_invalid")["details"],
        json!({"kind": "reason", "reason": "json_invalid"}),
        "{context}: the loader read the file and found no profile file in it"
    );
    assert_eq!(report["providers"]["file"]["state"], "invalid", "{context}");
    assert!(
        find(&report, "provider.file_unavailable_or_not_private").is_none(),
        "{context}: nothing says that the file was not opened"
    );
    assert!(
        entries(&report, "data").is_empty(),
        "{context}: the audit at the final path is still not described"
    );
    assert_eq!(audit_as_profiles.run.code, 3, "{context}");
    assert_eq!(audit_as_profiles_text.code, 3, "{context}");
    assert!(
        audit_as_profiles_text
            .stdout
            .contains("\nread       tenant registry, oidc file, provider profiles\n"),
        "{context}: the text report lists the read"
    );
    assert!(
        audit_as_profiles_text
            .stdout
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .contains("a file later named as a credential was read"),
        "{context}: the mode line of the text report says that such a file was read"
    );
    // The access times agree with the report: the audit at the final path
    // was read, and no key file or store below it was.
    if let Some(accessed) = audit_as_profiles.accessed() {
        assert!(
            accessed.contains(Path::new("both/authentication.sqlite")),
            "{context}: the report says that the audit was read; accessed: {accessed:?}"
        );
    }
    assert_never_read(&audit_as_profiles, &registry, &["both/tenants"], context);

    let context = "a registry that is read and not described";
    assert_never_read(
        &undescribed_run,
        &format!("{named}/tenants.json"),
        &[&format!("{named}/tenant-a")],
        context,
    );
    let report = undescribed_run.run.json();
    finding(&report, "layout.unrecognized");
    assert_eq!(
        finding(&report, credential_reference)["scope"],
        json!({"kind": "file", "file": "gateway_ca_bundle"})
    );
    assert_eq!(report["trust"][0]["read"], false);
    assert!(entries(&report, "credential_references").is_empty());

    for (context, observed) in &reference_runs {
        assert_never_read(observed, &registry, &["both"], context);
        let report = observed.run.json();
        assert_eq!(
            finding(&report, credential_reference)["scope"],
            json!({"kind": "file", "file": "oidc_ca_bundle"}),
            "{context}"
        );
        for code in [
            "trust.ca_bundle_invalid",
            "trust.ca_unavailable_or_not_private",
            "reference.file_read_before_it_was_named",
        ] {
            assert!(find(&report, code).is_none(), "{context}: {code}");
        }
        assert_eq!(report["trust"][0]["read"], false, "{context}");
        assert_eq!(
            report["mode"]["credential_contents_read"], "none",
            "{context}"
        );
        assert_eq!(observed.run.code, 3, "{context}");
    }
}

// ---- review: a reference that is itself a symbolic link -----------------------

#[test]
fn the_target_of_a_reference_that_is_a_link_is_never_opened() {
    let scenario = Scenario::new();
    let directory = scenario.path("registry");
    fixtures::private_dir(&directory);
    let registry = fixtures::hand_written_registry(&directory, &["tenant-a"]);
    // The runner key is kept elsewhere, and the registry names a symbolic
    // link to it. The readers refuse the link; the file behind it is a key
    // file all the same.
    let kept = scenario.path("kept");
    fixtures::private_dir(&kept);
    let key = kept.join("runner-key");
    fs::rename(directory.join("tenant-a/runner"), &key).unwrap();
    symlink(&key, directory.join("tenant-a/runner")).unwrap();

    let observed = observed_run(
        &scenario,
        &[
            "--tenants-file",
            arg(&registry),
            "--gateway-ca-file",
            arg(&key),
            "--json",
        ],
    );
    eprintln!(
        "OBSERVED entries read with a CA option that names the target of a linked reference: {:?}",
        observed.accessed()
    );
    assert_never_read(
        &observed,
        "registry/tenants.json",
        &["kept"],
        "a CA option that names the target of a reference that is a link",
    );
    let report = observed.run.json();
    assert_eq!(
        finding(&report, "reference.not_a_regular_file")["status"],
        "error",
        "the link itself is refused by its reader"
    );
    assert_eq!(
        finding(&report, "trust.ca_reference_is_a_credential_reference")["scope"],
        json!({"kind": "file", "file": "gateway_ca_bundle"})
    );
    assert!(find(&report, "trust.ca_bundle_invalid").is_none());
    assert_eq!(report["trust"][0]["read"], false);
    assert_eq!(report["mode"]["credential_contents_read"], "none");
    assert_eq!(observed.run.code, 3);
}

// ---- review: a registry read to classify a directory --------------------------

#[test]
fn a_registry_read_to_classify_a_directory_is_listed_as_read() {
    let scenario = Scenario::new();
    let directory = |digit: &str| {
        let name = format!(".setup-{}", digit.repeat(24));
        let path = scenario.path(&name);
        fixtures::private_dir(&path);
        (name, path)
    };

    // No setup record, and bytes that are no registry.
    let (undecodable_name, undecodable) = directory("1");
    fixtures::private_write(&undecodable.join("tenants.json"), "{ not a registry");
    // A registry that records paths inside the directory, and no setup record.
    let (inside_name, inside) = directory("2");
    fixtures::hand_written_registry(&inside, &["tenant-a"]);
    // The same beside the key directory of the historical layout.
    let (historical_name, historical) = directory("3");
    fixtures::hand_written_registry(&historical, &["tenant-a"]);
    fixtures::private_dir(&historical.join("secrets"));
    // A registry that records paths outside the directory, beside the key
    // directory of the historical layout, and no setup record.
    let (outside_name, outside) = directory("4");
    fixtures::private_dir(&outside.join("secrets"));
    let elsewhere = scenario.path("elsewhere");
    fixtures::private_dir(&elsewhere);
    let mut recorded =
        fixtures::read_json(&fixtures::hand_written_registry(&elsewhere, &["tenant-a"]));
    for field in ["data", "runner_token_file", "viewer_token_file"] {
        let reference = elsewhere.join(recorded["tenants"][0][field].as_str().unwrap());
        recorded["tenants"][0][field] = arg(&reference).into();
    }
    fixtures::private_write(
        &outside.join("tenants.json"),
        serde_json::to_vec_pretty(&recorded).unwrap(),
    );

    for (context, name, path, layout) in [
        (
            "a staging name, no setup record, bytes that are no registry",
            &undecodable_name,
            &undecodable,
            "layout.staging_without_intent",
        ),
        (
            "a staging name, paths recorded inside, no setup record",
            &inside_name,
            &inside,
            "layout.unrecognized",
        ),
        (
            "a staging name, paths recorded inside, a historical key directory",
            &historical_name,
            &historical,
            "layout.historical_passed_as_installation_dir",
        ),
        (
            "a staging name, paths recorded outside, a historical key directory",
            &outside_name,
            &outside,
            "layout.mixed",
        ),
    ] {
        let registry = format!("{name}/tenants.json");
        let observed = observed_run(&scenario, &["--installation-dir", arg(path), "--json"]);
        let text = scenario.diagnose(["--installation-dir", arg(path)]);
        eprintln!(
            "OBSERVED entries read with {context}: {:?}",
            observed.accessed()
        );
        let report = observed.run.json();
        assert_eq!(finding(&report, layout)["status"], "error", "{context}");
        assert!(
            entries(&report, "credential_references").is_empty()
                && entries(&report, "data").is_empty(),
            "{context}: nothing of the directory is described"
        );
        assert_eq!(
            report["mode"]["files_read_for_content"],
            json!(["tenant_registry"]),
            "{context}: the list of the files read never understates a read"
        );
        assert!(
            text.stdout.contains("\nread       tenant registry\n"),
            "{context}: the text report says the same"
        );
        // The directory that no arm describes: its message says why its
        // registry is listed as read, and never that it was not opened.
        if layout == "layout.unrecognized" {
            let unrecognized = finding(&report, layout);
            let message = text_of(unrecognized, "message");
            assert!(
                message.contains("the registry was read to tell what the directory is")
                    && !message.contains("neither file"),
                "{context}: the message contradicts the list of the files read: {message}"
            );
            let action = text_of(unrecognized, "action");
            assert!(
                action.contains("--tenants-file FILE") && action.contains("--oidc-config FILE"),
                "{context}: the action still names the two options that read these files: {action}"
            );
        }
        assert_never_read(&observed, &registry, &["elsewhere"], context);
        if let Some(accessed) = observed.accessed() {
            assert_eq!(
                accessed.iter().collect::<Vec<_>>(),
                [Path::new(&registry)],
                "{context}: the registry is the only entry opened or listed"
            );
        }
        assert_eq!(observed.run.code, 3, "{context}");
    }
}

// ---- review: the entry of a file that was read before it was named ------------

#[test]
fn the_entry_of_a_file_read_before_it_was_named_says_so() {
    let detected = "reference.file_read_before_it_was_named";
    let not_checked = [
        "reference.content_not_checked",
        "provider.credential_content_not_checked",
    ];
    // The facts of an entry are wrapped at the width of the report.
    let unwrapped = |run: &Run| run.stdout.split_whitespace().collect::<Vec<_>>().join(" ");

    // A credential whose posture its reader refuses, given by mistake as the
    // registry: it is read, then named. Nothing that the report says of that
    // file claims that it was not read.
    let refused = Scenario::new();
    let refused_credentials = fixtures::provider_credentials(&refused.path("credentials"));
    let refused_profiles = refused.path("profiles.json");
    fixtures::write_providers(
        &refused_profiles,
        &fixtures::four_profiles(&refused_credentials),
    );
    fs::set_permissions(
        &refused_credentials.claude_api,
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let (_, _, report) = refused.diagnose_both([
        "--tenants-file",
        arg(&refused_credentials.claude_api),
        "--providers-file",
        arg(&refused_profiles),
        "--client-profile",
        "claude-api",
    ]);
    finding(&report, detected);
    finding(&report, "provider.credential_unsafe");
    for finding in findings(&report) {
        for field in ["message", "action"] {
            assert!(
                !text_of(finding, field).contains("was not read"),
                "{} says that a file was not read in a report that says one was: {}",
                finding["code"],
                finding[field]
            );
        }
    }

    // A registry option that names a credential of a profile file given by
    // option: the registry is read first, and the profile file names it later.
    let scenario = Scenario::new();
    let credentials = fixtures::provider_credentials(&scenario.path("credentials"));
    let profiles = scenario.path("profiles.json");
    fixtures::write_providers(&profiles, &fixtures::four_profiles(&credentials));
    let (text, json, report) = scenario.diagnose_both([
        "--tenants-file",
        arg(&credentials.claude_api),
        "--providers-file",
        arg(&profiles),
    ]);
    assert_eq!(
        report["mode"]["credential_contents_read"],
        "detected_after_read"
    );
    finding(&report, detected);
    let references = entries(&report, "credential_references");
    assert_eq!(references.len(), 4, "one reference per profile");
    for entry in references {
        let owner = &entry["owner"]["id"];
        let expected = if owner == "claude-api" {
            "read_before_named"
        } else {
            "not_read"
        };
        assert_eq!(
            entry["content"], expected,
            "the entry of {owner} says what was done with its file"
        );
    }
    assert_eq!(
        unwrapped(&text)
            .matches("; content read before named")
            .count(),
        1,
        "the text report says it for that entry, and for no other"
    );
    for code in not_checked {
        let message = text_of(finding(&report, code), "message");
        assert!(
            message.contains(detected) && !message.contains("opens no"),
            "{code} does not claim that no such file was opened: {message}"
        );
    }
    assert_round_trip(&json, "a report with an entry read before it was named");

    // Control: each option is given the file it describes.
    let directory = scenario.path("registry");
    fixtures::private_dir(&directory);
    let registry = fixtures::hand_written_registry(&directory, &["tenant-a"]);
    let (text, _, report) = scenario.diagnose_both([
        "--tenants-file",
        arg(&registry),
        "--providers-file",
        arg(&profiles),
    ]);
    assert_eq!(report["mode"]["credential_contents_read"], "none");
    assert!(
        entries(&report, "credential_references")
            .iter()
            .all(|entry| entry["content"] == "not_read")
    );
    assert!(!unwrapped(&text).contains("read before named"));
    for code in not_checked {
        assert!(
            !text_of(finding(&report, code), "message").contains(detected),
            "{code}: nothing was read before it was named"
        );
    }

    // A store: a registry option that names the authentication audit that an
    // OIDC file, read after the registry, records.
    let stores = Scenario::new();
    let installation = stores.prepare("installation", &fixtures::manifest_v2(&["tenant-a"]));
    let (text, json, report) = stores.diagnose_both([
        "--tenants-file",
        arg(&installation.join("authentication.sqlite")),
        "--oidc-config",
        arg(&installation.join("oidc.json")),
    ]);
    assert_eq!(
        report["mode"]["credential_contents_read"],
        "detected_after_read"
    );
    assert_eq!(
        finding(&report, detected)["scope"],
        json!({"kind": "file", "file": "tenant_registry"})
    );
    let data = entries(&report, "data");
    assert_eq!(data.len(), 1, "the authentication audit is listed");
    assert_eq!(data[0]["role"], "authentication_audit");
    assert_eq!(
        data[0]["content"], "read_before_named",
        "the entry of the store says that its content was read"
    );
    assert!(
        unwrapped(&text).contains("; content read before named"),
        "the text report says the same"
    );
    assert_round_trip(&json, "a report with a store read before it was named");
    assert_eq!(json.code, 3);
}

// ---- review: the root of the file system is no display root -------------------

#[test]
fn the_root_of_the_file_system_is_no_display_root() {
    let scenario = Scenario::new();
    let directory = scenario.path("registry");
    fixtures::private_dir(&directory);
    let registry = fixtures::hand_written_registry(&directory, &["tenant-a"]);
    // One reference outside the directory of the registry, where nothing
    // exists.
    let outside = scenario.path("elsewhere/runner");
    fixtures::edit_json(&registry, |file| {
        file["tenants"][0]["runner_token_file"] = arg(&outside).into();
    });
    // Every absolute path lies below `/`. Shown relative to it, a path would
    // be printed in full without `--show-paths`.
    let hidden = arg(&scenario.root).trim_start_matches('/');

    for (context, args) in [
        (
            "the root given as the config directory",
            ["--config-dir", "/", "--tenants-file", arg(&registry)],
        ),
        (
            "an OIDC file given directly below the root",
            [
                "--tenants-file",
                arg(&registry),
                "--oidc-config",
                "/ai-guard-diagnose-absent-oidc.json",
            ],
        ),
    ] {
        let (text, json, report) = scenario.diagnose_both(args);
        for run in [&text, &json] {
            assert!(
                !run.stdout.contains(hidden) && !run.stderr.contains(hidden),
                "{context}: a path is printed in full although --show-paths was not given"
            );
            assert_ne!(run.code, 1, "{context}: a report is written");
        }
        let references = entries(&report, "credential_references");
        let entry = |display: &str| {
            references
                .iter()
                .find(|entry| entry["display"] == display)
                .unwrap_or_else(|| panic!("{context}: no reference is shown as {display}"))
        };
        assert_eq!(
            entry("<tenants-file-dir>/tenant-a/viewer")["location"],
            "inside_root",
            "{context}"
        );
        let elsewhere = entry("runner key reference #1 (outside the inspected directory)");
        assert_eq!(elsewhere["location"], "outside_root", "{context}");
        assert_eq!(elsewhere["presence"], "absent", "{context}");
        // Outside every inspected directory, an absent file may exist where
        // its consumer runs: unknown, never a certain refusal.
        assert_eq!(
            finding(&report, "reference.not_found_from_here")["status"],
            "unknown",
            "{context}"
        );
        assert!(find(&report, "reference.absent").is_none(), "{context}");
    }

    // Two inspected directories, one of them the root. The other one lies
    // below it like every absolute path, and keeps its own placeholder: shown
    // relative to the root, the directory option itself would print the path.
    let installation = scenario.prepare("installation", &fixtures::manifest_v1(&["tenant-a"]));
    let config = scenario.historical("config");
    for (context, args) in [
        (
            "the root as the config directory, beside an installation directory",
            [
                "--config-dir",
                "/",
                "--installation-dir",
                arg(&installation),
            ],
        ),
        (
            "the root as the installation directory, beside a config directory",
            ["--installation-dir", "/", "--config-dir", arg(&config)],
        ),
    ] {
        let (text, json, report) = scenario.diagnose_both(args);
        for run in [&text, &json] {
            assert!(
                !run.stdout.contains(hidden) && !run.stderr.contains(hidden),
                "{context}: a path is printed in full although --show-paths was not given"
            );
            assert_ne!(run.code, 1, "{context}: a report is written");
        }
        for (name, placeholder) in [
            ("config_dir", "<config>"),
            ("installation_dir", "<installation>"),
        ] {
            assert_eq!(
                input(&report, name)["display"],
                placeholder,
                "{context}: each directory is shown as its own placeholder"
            );
        }
    }
}

// ---- review: characters that a reader does not see ----------------------------

#[test]
fn invisible_characters_are_shown_escaped() {
    let scenario = Scenario::new();
    let directory = scenario.path("registry");
    fixtures::private_dir(&directory);
    // One identifier per class. The product validator refuses each of them,
    // and a refused identifier is still shown: two identifiers that differ
    // only by such a character must not look alike.
    let classes = [
        (
            '\u{061c}',
            "the Arabic letter mark, a bidirectional control",
        ),
        ('\u{200b}', "a zero-width space"),
        ('\u{200d}', "a zero-width joiner"),
        ('\u{00ad}', "a soft hyphen"),
        ('\u{2060}', "a word joiner"),
        ('\u{2063}', "an invisible separator"),
        ('\u{034f}', "a combining grapheme joiner"),
        ('\u{fe0f}', "a variation selector"),
        ('\u{3164}', "a Hangul filler"),
        ('\u{e0041}', "a tag character"),
        ('\u{00a0}', "a no-break space"),
        ('\u{2003}', "an em space"),
        ('\u{3000}', "an ideographic space"),
    ];
    let ids: Vec<String> = classes
        .iter()
        .enumerate()
        .map(|(index, (character, _))| format!("tenant{character}{index:02}"))
        .collect();
    let escaped: Vec<String> = classes
        .iter()
        .enumerate()
        .map(|(index, (character, _))| {
            format!("tenant\\u{{{:x}}}{index:02}", u32::from(*character))
        })
        .collect();
    let tenants: Vec<&str> = ids.iter().map(String::as_str).collect();
    let registry = fixtures::hand_written_registry(&directory, &tenants);

    let (text, json, report) = scenario.diagnose_both(["--tenants-file", arg(&registry)]);
    for run in [&text, &json] {
        assert_ne!(run.code, 1, "a report is written: {}", run.stderr);
        for (character, class) in classes {
            assert!(
                !run.stdout.contains(character),
                "{class} (U+{:04X}) reaches the output as it is",
                u32::from(character)
            );
        }
    }
    let listed: Vec<&str> = entries(&report["tenants"], "registry")
        .iter()
        .map(|id| text_of(id, "id"))
        .collect();
    assert_eq!(listed.len(), classes.len());
    let displays: Vec<&str> = entries(&report, "credential_references")
        .iter()
        .map(|entry| text_of(entry, "display"))
        .collect();
    for (shown, (_, class)) in escaped.iter().zip(classes) {
        assert!(
            listed.contains(&shown.as_str()),
            "{class} is shown escaped in an identifier: {listed:?}"
        );
        assert!(
            text.stdout.contains(&format!("\"{shown}\"")),
            "{class} is shown escaped in the text report"
        );
        assert!(
            displays.contains(&format!("<tenants-file-dir>/{shown}/runner").as_str()),
            "{class} is shown escaped in a path"
        );
    }
}

// ---- review: a tenant policy judged at the assumed event bound ----------------

#[test]
fn a_policy_refused_at_the_assumed_event_bound_is_not_an_observed_error() {
    let assumed_code = "registry.policy_refused_at_assumed_event_bound";
    let scenario = Scenario::new();
    let directory = scenario.path("registry");
    fixtures::private_dir(&directory);
    let registry = fixtures::hand_written_registry(&directory, &["tenant-a"]);
    // An audit-capacity policy sized for a gateway started with a larger
    // event bound: its human share alone needs more than 100000 events.
    let policy = json!({
        "version": 1,
        "human_tenant": {"events": 150_000, "bytes": 134_217_728},
        "human_principal": {"events": 12, "bytes": 33_554_432},
        "control_reserve": {"events": 25, "bytes": 67_108_864},
        "identity_reserve": {"events": 12, "bytes": 33_554_432}
    });
    let typed: agentsandbox::audit::AuditCapacityPolicyV1 =
        serde_json::from_value(policy.clone()).expect("fixture: the policy decodes");
    assert!(
        typed.validate(100_000).is_err() && typed.validate(1_000_000).is_ok(),
        "fixture: the product validator refuses this policy at the gateway default and accepts it at a larger bound"
    );
    fixtures::edit_json(&registry, |file| {
        file["tenants"][0]["audit_capacity_policy"] = policy.clone();
    });
    // The real gateway, on copies: it creates the stores it opens.
    let gateway = |name: &str, max_events: Option<&str>| {
        let copy = scenario.path(name);
        fixtures::copy_tree(&directory, &copy);
        scenario.gateway_with_event_bound(&copy.join("tenants.json"), max_events)
    };
    let at_default = gateway("gateway-default", None);
    let at_larger_bound = gateway("gateway-larger", Some("1000000"));

    // No bound is given: the verdict rests on the default this tool assumes.
    let (text, json, report) = scenario.diagnose_both(["--tenants-file", arg(&registry)]);
    assert!(
        find(&report, "registry.policy_invalid").is_none(),
        "an observed error is reported for a registry that a gateway started with another bound accepts"
    );
    let assumed = finding(&report, assumed_code);
    // The message names the bound that this run assumed, which is the
    // default of the tool.
    let assumed_bound = input(&report, "max_events")["display"]
        .as_str()
        .expect("the assumed bound is listed among the inputs");
    assert_eq!(assumed_bound, DEFAULT_MAX_EVENTS.to_string());
    assert!(
        text_of(assumed, "message").contains(&format!("at {assumed_bound} events")),
        "the message names another bound than the one this run assumed, {assumed_bound}: {}",
        assumed["message"]
    );
    assert_eq!(assumed["status"], "unknown");
    assert_eq!(assumed["basis"], "inferred");
    assert_eq!(assumed["origin"]["assumes"], json!(["max_events"]));
    assert_eq!(
        assumed["details"],
        json!({"kind": "policy", "policy": "audit_capacity"})
    );
    assert_eq!(
        assumed["subjects"]["ids"],
        json!([{"id": "tenant-a", "masked": false}])
    );
    assert!(text_of(assumed, "action").contains("--max-events N"));
    assert!(find(&report, "registry.offline_rules_passed").is_none());
    assert_eq!(
        *input(&report, "max_events"),
        json!({"name": "max_events", "origin": "default", "display": "100000"})
    );
    assert!(
        text.stdout.contains(
            "\nassumed    gateway event bound 100000 (default; the gateway's value is not stored anywhere)\n"
        ),
        "the assumed bound is listed among the inputs of the text report"
    );
    assert!(
        text.stdout
            .contains(&format!("  [unknown] {assumed_code}\n"))
    );
    assert_eq!(
        json.code, 4,
        "an unknown, not an error: {}",
        report["summary"]
    );
    assert_gateway_refuses(
        &at_default,
        "audit capacity policy must preserve reserves",
        "the action says that a gateway started without --max-events refuses this policy",
    );

    // The bound of a gateway that accepts the registry.
    assert!(
        at_larger_bound.listening(),
        "positive control: the gateway started with --max-events 1000000 accepts this registry: {}",
        at_larger_bound.describe()
    );
    let (_, json, report) =
        scenario.diagnose_both(["--tenants-file", arg(&registry), "--max-events", "1000000"]);
    for code in [assumed_code, "registry.policy_invalid"] {
        assert!(find(&report, code).is_none(), "{code}");
    }
    assert_eq!(
        finding(&report, "registry.offline_rules_passed")["origin"]["assumes"],
        json!([])
    );
    assert_eq!(
        *input(&report, "max_events"),
        json!({"name": "max_events", "origin": "cli", "display": "1000000"})
    );
    assert_eq!(json.code, 0, "{}", report["summary"]);

    // The default, given: the refusal is certain for that gateway.
    let (_, json, report) =
        scenario.diagnose_both(["--tenants-file", arg(&registry), "--max-events", "100000"]);
    let invalid = finding(&report, "registry.policy_invalid");
    assert_eq!(invalid["status"], "error");
    assert_eq!(invalid["basis"], "observed");
    assert_eq!(invalid["origin"]["assumes"], json!([]));
    assert_eq!(
        invalid["details"],
        json!({"kind": "policy", "policy": "audit_capacity"})
    );
    assert!(find(&report, assumed_code).is_none());
    assert_eq!(json.code, 3);

    // A policy whose verdict does not depend on the bound stays an error
    // when no bound is given, and nothing is assumed for it.
    fixtures::edit_json(&registry, |file| {
        let tenant = file["tenants"][0].as_object_mut().unwrap();
        tenant.remove("audit_capacity_policy");
        tenant.insert(
            "worker_quota".to_owned(),
            json!({
                "version": 1, "max_active_workers": 101, "max_cpu_millis": 6000,
                "max_memory_bytes": 3_221_225_472_u64, "max_timeout_seconds": 3600
            }),
        );
    });
    let (_, json, report) = scenario.diagnose_both(["--tenants-file", arg(&registry)]);
    let invalid = finding(&report, "registry.policy_invalid");
    assert_eq!(
        invalid["details"],
        json!({"kind": "policy", "policy": "worker_quota"})
    );
    assert_eq!(invalid["origin"]["assumes"], json!([]));
    assert!(find(&report, assumed_code).is_none());
    assert!(
        entries(&report, "inputs")
            .iter()
            .all(|input| input["name"] != "max_events"),
        "no bound is assumed for a policy that does not depend on it"
    );
    assert_eq!(json.code, 3);
}

// ---- review: totals of identifiers that appear twice --------------------------

#[test]
fn duplicate_identifiers_are_counted_once() {
    let scenario = Scenario::new();
    let directory = scenario.path("registry");
    fixtures::private_dir(&directory);
    let registry = fixtures::hand_written_registry(&directory, &["tenant-a"]);
    fixtures::edit_json(&registry, |file| {
        let entry = file["tenants"][0].clone();
        file["tenants"].as_array_mut().unwrap().push(entry);
    });
    let line = |text: &Run, label: &str| {
        text.stdout
            .lines()
            .find(|line| line.starts_with(label))
            .unwrap_or_else(|| panic!("the text report has no line {label:?}"))
            .to_owned()
    };

    let (text, json, report) = scenario.diagnose_both(["--tenants-file", arg(&registry)]);
    assert_eq!(
        finding(&report, "registry.tenant_id_duplicate")["status"],
        "error"
    );
    let listed = entries(&report["tenants"], "registry");
    assert_eq!(*listed, [json!({"id": "tenant-a", "masked": false})]);
    assert_eq!(
        report["tenants"]["registry_total"],
        listed.len(),
        "the total counts the identifiers that the list holds"
    );
    assert_eq!(report["topology"]["tenant_count"], 1);
    assert_eq!(report["summary"]["omitted"]["tenant_ids"], 0);
    assert_eq!(
        line(&text, "  registry  "),
        "  registry  1 tenant  \"tenant-a\"",
        "nothing is said not listed when every identifier is listed"
    );
    assert_eq!(
        line(&text, "  tenant model  "),
        "  tenant model  registry; 1 tenant"
    );
    assert_eq!(json.code, 3);

    // The same for a capacity plan that names a tenant twice.
    let installation = scenario.prepare("installation", &fixtures::manifest_v2(&["tenant-a"]));
    fixtures::edit_json(&installation.join("oidc.json"), |file| {
        let tenants = file["authentication_audit"]["capacity_plan"]["tenants"]
            .as_array_mut()
            .unwrap();
        let entry = tenants[0].clone();
        tenants.push(entry);
    });
    let (text, _, report) = scenario.diagnose_both(["--installation-dir", arg(&installation)]);
    let listed = entries(&report["tenants"], "plan");
    assert_eq!(*listed, [json!({"id": "tenant-a", "masked": false})]);
    assert_eq!(report["tenants"]["plan_total"], listed.len());
    assert_eq!(
        line(&text, "  capacity plan  "),
        "  capacity plan  1 tenant  \"tenant-a\""
    );
}

// ---- review: what every entry point says about the files that are opened ------

#[test]
fn entry_points_state_what_is_opened_as_the_report_does() {
    let scenario = Scenario::new();

    // The help: no file that the inspected files name as a credential is
    // opened, and a file given by option is opened as what its option says.
    let help: Vec<String> = scenario
        .diagnose(["--help"])
        .stdout
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    let help = help.join(" ");
    assert!(
        help.contains("a file given by option is opened as what that option says it is"),
        "--help states the limit that the report contract encodes"
    );
    assert!(
        help.contains("opens no file that the inspected files name as"),
        "--help says which files are never opened"
    );
    assert!(!help.contains("opens no credential"));

    // The lines that list this guide in the manual indexes, where they exist.
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"));
    for (index, stated, absolute) in [
        (
            "README.md",
            "that the inspected files name as a credential",
            ["without opening a credential", "opens no credential"],
        ),
        (
            "docs/setup/README.md",
            "that the inspected files name as a credential",
            ["without opening a credential", "opens no credential"],
        ),
        (
            "README.fr.md",
            "que les fichiers inspectés désignent comme credential",
            ["sans ouvrir aucun credential", "n'ouvre aucun credential"],
        ),
        (
            "docs/setup/README.fr.md",
            "que les fichiers inspectés désignent comme credential",
            ["sans ouvrir aucun credential", "n'ouvre aucun credential"],
        ),
    ] {
        let text = fs::read_to_string(repository.join(index))
            .unwrap_or_else(|error| panic!("the manual index {index} is not readable: {error}"));
        let guide_links = text
            .lines()
            .filter(|line| line.contains("diagnostics/README"));
        assert!(
            guide_links.clone().any(|line| line.contains(stated)),
            "{index} lists this guide without the limit of what is never opened"
        );
        for line in guide_links {
            for phrase in absolute {
                assert!(!line.contains(phrase), "{index}: {phrase:?}");
            }
        }
    }

    // A report without any file read before it was named: the two sentences
    // on content say which files are never opened.
    let credentials = fixtures::provider_credentials(&scenario.path("credentials"));
    let profiles = scenario.path("profiles.json");
    fixtures::write_providers(&profiles, &fixtures::four_profiles(&credentials));
    let directory = scenario.path("registry");
    fixtures::private_dir(&directory);
    let registry = fixtures::hand_written_registry(&directory, &["tenant-a"]);
    let (_, _, report) = scenario.diagnose_both([
        "--tenants-file",
        arg(&registry),
        "--providers-file",
        arg(&profiles),
    ]);
    assert_eq!(report["mode"]["credential_contents_read"], "none");
    assert!(
        text_of(finding(&report, "reference.content_not_checked"), "message")
            .contains("opens no file that the inspected files name as")
    );
    assert!(
        text_of(
            finding(&report, "provider.credential_content_not_checked"),
            "message"
        )
        .contains("opens no file that the profile file names as")
    );

    // A report that says that a file named as a credential, or as a store,
    // was read: no sentence of it says that none was opened.
    let installation = scenario.prepare("installation", &fixtures::manifest_v2(&["tenant-a"]));
    let credential_read = scenario.diagnose_both([
        "--tenants-file",
        arg(&credentials.claude_api),
        "--providers-file",
        arg(&profiles),
    ]);
    let store_read = scenario.diagnose_both([
        "--tenants-file",
        arg(&installation.join("authentication.sqlite")),
        "--oidc-config",
        arg(&installation.join("oidc.json")),
    ]);
    for (context, (text, _, report)) in [
        ("a credential read as a registry", &credential_read),
        ("a store read as a registry", &store_read),
    ] {
        assert_eq!(
            report["mode"]["credential_contents_read"], "detected_after_read",
            "{context}"
        );
        let lower = text.stdout.to_lowercase();
        let lower: Vec<&str> = lower.split_whitespace().collect();
        let lower = lower.join(" ");
        for claim in [
            "opens no key file",
            "opens no provider credential",
            "opens no credential",
            "opens no file that",
            "no credential was opened",
            "no store was opened",
        ] {
            assert!(
                !lower.contains(claim),
                "{context}: the report says that a file was read, and also {claim:?}"
            );
        }
    }
}

// ---- review: what a refused subscription says about the automatic choice ------

#[test]
fn a_refused_subscription_states_the_automatic_rule_of_the_product() {
    let code = "provider.subscription_never_falls_back_to_api";
    let scenario = Scenario::new();
    let credentials = fixtures::provider_credentials(&scenario.path("credentials"));

    // A Codex subscription that is not signed in, and a Claude API profile.
    let config = scenario.historical("config");
    let providers = config.join("providers.json");
    fixtures::write_providers(
        &providers,
        &json!({"version": 1, "profiles": [
            fixtures::subscription_profile("codex-subscription", "codex", &credentials.codex_subscription),
            fixtures::api_profile("claude-api", "claude_code", &credentials.claude_api)
        ]}),
    );
    fs::remove_file(&credentials.codex_subscription).unwrap();

    // No option: the subscription is resolved and the launch asks for a login.
    let auto = Ask::default();
    assert_eq!(
        product_resolution(&scenario, &providers, &auto).unwrap_err(),
        "login_required"
    );
    let (_, json, report) = diagnose_selection(&scenario, &config, &auto);
    let never = finding(&report, code);
    let action = text_of(never, "action");
    assert!(
        !action.contains("only by an explicit request"),
        "the automatic choice of the product can resolve an API profile: {action}"
    );
    assert!(
        action.contains("--auth-mode subscription"),
        "the action names the request that never resolves to an API profile: {action}"
    );
    assert!(
        text_of(never, "message").contains("resolves to a subscription profile"),
        "the message says which situation this is"
    );
    finding(&report, "provider.login_required");
    assert_eq!(json.code, 3);

    // The oracle of that action: with the client alone, and no request for
    // the API, the product resolves the API profile of this file.
    let named = Ask {
        client: Some("claude-code"),
        ..Ask::default()
    };
    let resolution = product_resolution(&scenario, &providers, &named)
        .expect("the product resolves a profile for the client alone");
    assert_eq!(
        resolution.auth_mode,
        AuthMode::Api,
        "under automatic authentication mode the product resolves an API profile when no subscription profile matches the client"
    );
    let (_, json, report) = diagnose_selection(&scenario, &config, &named);
    assert_resolved_as_the_product(&report, &resolution, "the client alone");
    assert_eq!(
        report["providers"]["selection"]["requested"]["auth_mode"],
        "auto"
    );
    assert_eq!(
        finding(&report, "provider.api_billing_notice")["status"],
        "warning"
    );
    assert!(find(&report, code).is_none());
    assert_eq!(json.code, 0);

    // The same client with a subscription request: refused, and no API
    // profile is resolved in its place.
    let requested = Ask {
        client: Some("claude-code"),
        auth_mode: Some("subscription"),
        ..Ask::default()
    };
    assert_eq!(
        product_resolution(&scenario, &providers, &requested).unwrap_err(),
        "credential_missing"
    );
    let (_, json, report) = diagnose_selection(&scenario, &config, &requested);
    finding(&report, "provider.selection_no_profile");
    assert_eq!(report["providers"]["selection"]["resolved"], Value::Null);
    assert!(find(&report, "provider.api_billing_notice").is_none());
    let never = finding(&report, code);
    assert!(
        text_of(never, "message").contains("the product refuses this selection"),
        "the message says which situation this is"
    );
    let action = text_of(never, "action");
    assert!(
        !action.contains("Sign in") && action.contains("--auth-mode subscription"),
        "no sign-in is proposed for a selection that resolves no profile: {action}"
    );
    assert_eq!(json.code, 3);

    // No subscription profile is declared at all.
    let api_only = scenario.historical("api-only");
    let api_only_file = api_only.join("providers.json");
    fixtures::write_providers(
        &api_only_file,
        &json!({"version": 1, "auto_priority": ["codex-api", "claude-api"], "profiles": [
            fixtures::api_profile("codex-api", "codex", &credentials.codex_api),
            fixtures::api_profile("claude-api", "claude_code", &credentials.claude_api)
        ]}),
    );
    let subscription = Ask {
        auth_mode: Some("subscription"),
        ..Ask::default()
    };
    assert_eq!(
        product_resolution(&scenario, &api_only_file, &subscription).unwrap_err(),
        "credential_missing"
    );
    let (_, json, report) = diagnose_selection(&scenario, &api_only, &subscription);
    let never = finding(&report, code);
    assert!(
        text_of(never, "message").contains("declares no subscription profile"),
        "the message says which situation this is"
    );
    let action = text_of(never, "action");
    assert!(
        action.contains("Declare a subscription profile")
            && !action.contains("Correct the subscription profile")
            && !action.contains("Sign in for the resolved"),
        "no profile exists to correct or to sign in for: {action}"
    );
    assert_eq!(json.code, 3);
    // The oracle of that action: without the request, the automatic mode
    // resolves an API profile of this file.
    let unrequested = product_resolution(&scenario, &api_only_file, &Ask::default())
        .expect("the product resolves an API profile of this file without the request");
    assert_eq!(unrequested.auth_mode, AuthMode::Api);
    assert!(
        action.contains("can resolve an API profile of this file"),
        "the action says what the automatic mode does with this file: {action}"
    );
}

// ---- review: long values, and a report that cannot be written -----------------

#[test]
fn long_values_are_masked_before_they_are_clipped() {
    const TRUNCATED: &str = "…[truncated]";
    let scenario = Scenario::new();
    let config = scenario.historical("config");

    // A value above the bound of an operator string: 512 bytes.
    let long_profile = "a".repeat(600);
    let (text, json, report) = scenario.diagnose_both([
        "--config-dir",
        arg(&config),
        "--client-profile",
        long_profile.as_str(),
    ]);
    let display = text_of(input(&report, "client_profile"), "display");
    assert_eq!(
        display,
        format!("{}{TRUNCATED}", "a".repeat(512 - TRUNCATED.len())),
        "a value is cut within its bound, and the cut is marked"
    );
    assert!(text.stdout.contains(&format!(
        "requested profile \"{display}\" (from --client-profile)\n"
    )));
    assert_ne!(json.code, 1, "a report is written: {}", json.stderr);

    // The order of the two steps. A value that the output filter rewrites
    // lies across the bound: masked first, the value is cut before its mask.
    // Cut first, the beginning of the value would stay and no longer look
    // like anything to mask.
    let masked = "sk-mnopqrstuvwxyz0123";
    let filter = Redactor::new(Vec::new());
    assert_eq!(
        filter.text(&format!("model.{masked}.suffix")),
        "model.[REDACTED].suffix",
        "fixture: the output filter rewrites this value"
    );
    let head = format!("{}.", "a".repeat(489));
    let model = format!("{head}{masked}.{}", "b".repeat(99));
    assert_eq!(
        filter.text(&model[..498]),
        &model[..498],
        "fixture: cut at the bound, what is left of the value is not rewritten"
    );
    let (text, json, report) =
        scenario.diagnose_both(["--config-dir", arg(&config), "--model", model.as_str()]);
    assert_eq!(
        text_of(input(&report, "model"), "display"),
        format!("{head}{TRUNCATED}"),
        "the value is masked, then cut before its mask"
    );
    for run in [&text, &json] {
        assert!(
            !run.stdout.contains("sk-m") && !run.stderr.contains("sk-m"),
            "no beginning of a masked value is printed"
        );
        assert_ne!(run.code, 1, "a report is written: {}", run.stderr);
    }
    assert_eq!(finding(&report, "output.value_masked")["status"], "warning");

    // A path above the bound of a path display, 1024 bytes, that holds
    // nothing the filter rewrites: it is cut and marked, not masked.
    let mut deep = scenario.path("deep");
    fixtures::private_dir(&deep);
    for _ in 0..5 {
        deep = deep.join("d".repeat(240));
        fixtures::private_dir(&deep);
    }
    let registry = fixtures::hand_written_registry(&deep, &["tenant-a"]);
    assert!(arg(&registry).len() > 1024);
    let (text, json, report) =
        scenario.diagnose_both(["--tenants-file", arg(&registry), "--show-paths"]);
    let display = text_of(input(&report, "tenants_file"), "display");
    assert_eq!(
        display,
        format!("{}{TRUNCATED}", &arg(&registry)[..1024 - TRUNCATED.len()]),
        "a path is cut within its bound, and the cut is marked"
    );
    assert!(text.stdout.contains(&format!(
        "inspected  tenant registry \"{display}\" (from --tenants-file)\n"
    )));
    assert!(
        find(&report, "output.value_masked").is_none(),
        "a cut is not a mask"
    );
    assert_ne!(json.code, 1, "a report is written: {}", json.stderr);

    // The single write of the report fails: a refusal, and no report.
    let mut command = scenario.diagnose_command();
    command.args(["--config-dir", arg(&config)]);
    let unwritten = diagnose_support::child::run_without_stdout_reader(
        command,
        &scenario.work,
        diagnose_support::child::DIAGNOSE_BOUND,
        diagnose_support::child::SMALL_CAP,
    );
    assert_eq!(unwritten.code, Some(1));
    assert_eq!(
        unwritten.stderr_text(),
        format!("{}\n", DiagnoseError::OutputUnavailable)
    );
}

// ---- review: a credential that its reader refuses for its path ----------------

#[test]
fn a_credential_refused_for_its_path_is_reported() {
    let not_evaluated = "reference.private_rule_not_evaluated";
    let scenario = Scenario::new();
    let one = |profile: Value| json!({"version": 1, "profiles": [profile]});
    let set_mode = |path: &Path, mode: u32| {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    };
    // Each case: a config directory, its own credentials, and one stated edit
    // of the directory that leads to the credential. The product reader is
    // asked after the report.
    let case = |name: &str, edit: &dyn Fn(&fixtures::ProviderCredentials) -> PathBuf| {
        let config = scenario.historical(&format!("config-{name}"));
        let credentials =
            fixtures::provider_credentials(&scenario.path(&format!("credentials-{name}")));
        let credential = edit(&credentials);
        let providers = config.join("providers.json");
        fixtures::write_providers(
            &providers,
            &one(fixtures::api_profile("only", "codex", &credential)),
        );
        let (text, json, report) = scenario.diagnose_both(["--config-dir", arg(&config)]);
        let product = product_refusal_class(&scenario, &providers, "only");
        (text, json.code, report, product)
    };
    let private = case("private", &|credentials| credentials.codex_api.clone());
    let group_writable = case("group-writable", &|credentials| {
        set_mode(&credentials.directory, 0o775);
        credentials.codex_api.clone()
    });
    let linked = case("linked", &|credentials| {
        let link = scenario.path("credentials-through-a-link");
        symlink(&credentials.directory, &link).unwrap();
        link.join("codex-api.env")
    });
    let readable = case("readable", &|credentials| {
        set_mode(&credentials.directory, 0o755);
        credentials.codex_api.clone()
    });
    let reference = |report: &Value| {
        entries(report, "credential_references")
            .iter()
            .find(|entry| entry["role"] == "provider")
            .cloned()
            .expect("the provider reference is listed")
    };

    // Positive control: the reader accepts a credential in a private directory.
    let (_, code, report, product) = &private;
    assert_eq!(*product, None, "positive control of the product reader");
    assert_eq!(reference(report)["private_rule"], "satisfied");
    assert!(find(report, "provider.credential_unsafe").is_none());
    assert!(find(report, not_evaluated).is_none());
    assert_eq!(*code, 0);

    // The reader refuses the path: the report says so, with an error.
    for (name, (text, code, report, product)) in [
        ("a directory that its group can write", &group_writable),
        ("a directory reached through a link", &linked),
    ] {
        assert_eq!(
            product.as_deref(),
            Some("credential_unsafe"),
            "{name}: the product reader refuses this credential"
        );
        let entry = reference(report);
        assert_eq!(entry["mode"], "0600", "{name}: the file itself conforms");
        assert_eq!(entry["entry_kind"], "regular_file", "{name}");
        let unsafe_path = find(report, "provider.credential_unsafe").unwrap_or_else(|| {
            panic!(
                "{name}: a credential that its reader refuses for its path is reported as nothing; the report has {:?}",
                codes(report)
            )
        });
        assert_eq!(unsafe_path["status"], "error", "{name}");
        assert!(
            text_of(unsafe_path, "message").contains("refused by its reader for its path"),
            "{name}: the message names the path: {}",
            unsafe_path["message"]
        );
        // The cause that only macOS has is named too: it is read in the
        // product code, and no run can produce it here.
        assert!(
            text_of(unsafe_path, "message")
                .contains("on macOS, such a directory can also carry an extended access entry"),
            "{name}: {}",
            unsafe_path["message"]
        );
        assert!(
            text_of(unsafe_path, "action").contains("only root or the control account can write"),
            "{name}: {}",
            unsafe_path["action"]
        );
        assert_eq!(entry["private_rule"], "violated", "{name}");
        assert_eq!(entry["content"], "not_read", "{name}");
        assert!(find(report, "provider.selection_resolved").is_some());
        assert_eq!(*code, 3, "{name}");
        assert!(
            text.stdout
                .contains("  [error] provider.credential_unsafe\n"),
            "{name}"
        );
    }

    // The reader accepts a directory that others can read and cannot write.
    // The entry check of the product did not run: that is said, not left out.
    let (text, code, report, product) = &readable;
    assert_eq!(
        *product, None,
        "control: the product reader accepts a credential below a 0755 directory"
    );
    assert!(find(report, "provider.credential_unsafe").is_none());
    assert_eq!(reference(report)["private_rule"], "not_evaluated");
    let explained = finding(report, not_evaluated);
    assert_eq!(explained["status"], "not_checked");
    assert_eq!(explained["basis"], "observed");
    assert_eq!(
        explained["subjects"]["ids"],
        json!([{"id": "only", "masked": false}])
    );
    assert_eq!(
        explained["requires"],
        "the consuming command checks the file system and the access entries of the file when it opens it"
    );
    assert_eq!(*code, 0);
    assert!(
        text.stdout
            .contains(&format!("  [not_checked] {not_evaluated}\n"))
    );

    // The same refusal for profiles that the selection does not resolve.
    let config = scenario.historical("config-four");
    let credentials = fixtures::provider_credentials(&scenario.path("credentials-four"));
    set_mode(&credentials.directory, 0o775);
    let providers = config.join("providers.json");
    fixtures::write_providers(&providers, &fixtures::four_profiles(&credentials));
    let (_, json, report) = scenario.diagnose_both(["--config-dir", arg(&config)]);
    assert_eq!(
        product_refusal_class(&scenario, &providers, "claude-api").as_deref(),
        Some("credential_unsafe")
    );
    let unselected = finding(&report, "provider.unselected_profile_reference_unsafe");
    assert_eq!(unselected["status"], "warning");
    assert_eq!(unselected["subjects"]["total"], 3);
    let message = text_of(unselected, "message");
    assert!(
        message.contains("refused by its reader for its path") && message.contains("on macOS"),
        "{message}"
    );
    assert_eq!(
        finding(&report, "provider.credential_unsafe")["subjects"]["ids"],
        json!([{"id": "codex-subscription", "masked": false}])
    );
    assert_eq!(json.code, 3);

    // The key sources of a historical directory are read by the same reader.
    let keys = scenario.historical("config-keys");
    agentsandbox::runner::read_token(&keys.join("secrets/runner"))
        .expect("positive control: the private-file reader accepts the key created by init");
    set_mode(&keys.join("secrets"), 0o770);
    assert!(
        agentsandbox::runner::read_token(&keys.join("secrets/runner")).is_err(),
        "the private-file reader refuses a key below a directory that its group can write"
    );
    let (_, json, report) = scenario.diagnose_both(["--config-dir", arg(&keys)]);
    let violated = finding(&report, "reference.private_rule_violated");
    let message = text_of(violated, "message");
    assert!(
        message.contains("refused by its reader for its path") && message.contains("on macOS"),
        "{message}"
    );
    assert_eq!(json.code, 3);
    set_mode(&keys.join("secrets"), 0o755);
    agentsandbox::runner::read_token(&keys.join("secrets/runner"))
        .expect("control: the private-file reader accepts a key below a 0755 directory");
    let (_, json, report) = scenario.diagnose_both(["--config-dir", arg(&keys)]);
    assert!(find(&report, "reference.private_rule_violated").is_none());
    assert_eq!(finding(&report, not_evaluated)["status"], "not_checked");
    assert_eq!(json.code, 0);

    // Asking the reader about the directories opens no credential.
    let observed_scenario = Scenario::new();
    let config = observed_scenario.historical("config");
    let credentials = fixtures::provider_credentials(&observed_scenario.path("credentials"));
    set_mode(&credentials.directory, 0o775);
    fixtures::write_providers(
        &config.join("providers.json"),
        &fixtures::four_profiles(&credentials),
    );
    let observed = observed_run(
        &observed_scenario,
        &["--config-dir", arg(&config), "--json"],
    );
    assert_never_read(
        &observed,
        "config/providers.json",
        &["credentials"],
        "credentials below a directory that the reader refuses",
    );
    assert_eq!(
        observed.run.json()["mode"]["credential_contents_read"],
        "none"
    );
}
// ---- review: a registry that is absent, and one that cannot be read -----------

#[test]
fn an_absent_registry_is_told_from_an_unreadable_one() {
    let scenario = Scenario::new();

    // A registry given by option that does not exist.
    let missing = scenario.path("missing-tenants.json");
    let (text, json, report) = scenario.diagnose_both(["--tenants-file", arg(&missing)]);
    let absent = finding(&report, "registry.file_unreadable");
    assert_eq!(absent["status"], "error");
    assert!(
        text_of(absent, "message").contains("does not exist at the given path"),
        "absence is named: {}",
        absent["message"]
    );
    assert!(!text_of(absent, "message").contains("1 MiB"));
    assert!(entries(&report["mode"], "files_read_for_content").is_empty());
    assert_eq!(json.code, 3);
    assert!(text.stdout.contains("  [error] registry.file_unreadable\n"));
    assert_gateway_refuses(
        &scenario.gateway(&missing, None),
        "Registre de tenants inaccessible",
        "an absent registry",
    );

    // A published installation whose registry was removed: the action can
    // be followed with the option that gave the directory.
    let installation = scenario.prepare("installation", &fixtures::manifest_v2(&["tenant-a"]));
    fs::remove_file(installation.join("tenants.json")).unwrap();
    let (_, json, report) = scenario.diagnose_both(["--installation-dir", arg(&installation)]);
    let removed = finding(&report, "registry.file_unreadable");
    assert!(
        text_of(removed, "message").contains("holds no tenants.json"),
        "{}",
        removed["message"]
    );
    let action = text_of(removed, "action");
    assert!(
        action.contains("Restore") && !action.contains("--tenants-file"),
        "the action names no option that is refused beside an installation directory: {action}"
    );
    assert_eq!(json.code, 3);

    // A link, a directory and a file above the bound keep the rule of the
    // reader as their cause.
    let directory = scenario.path("registries");
    fixtures::private_dir(&directory);
    let registry = fixtures::hand_written_registry(&directory, &["tenant-a"]);
    let link = directory.join("link.json");
    symlink(&registry, &link).unwrap();
    let large = directory.join("large.json");
    fixtures::private_write(&large, " ".repeat(1_048_577));
    for (name, path) in [
        ("a link", &link),
        ("a directory", &directory),
        ("1048577 bytes", &large),
    ] {
        let (_, json, report) = scenario.diagnose_both(["--tenants-file", arg(path)]);
        let unreadable = finding(&report, "registry.file_unreadable");
        assert!(
            text_of(unreadable, "message").contains("not a symbolic link, of at most 1 MiB"),
            "{name}: {}",
            unreadable["message"]
        );
        assert_eq!(json.code, 3, "{name}");
    }
}
// ---- review: a plan mismatch and the plan that the audit stores ---------------

#[test]
fn a_plan_mismatch_names_the_stored_plan() {
    let durable = "tenants.durable_plan_comparison_not_available";
    let scenario = Scenario::new();
    let manifest = fixtures::manifest_v2(&["tenant-a"]);
    let add_tenant_b = |installation: &Path| {
        let tenant_b = installation.join("tenants/tenant-b");
        fixtures::private_dir(&tenant_b);
        for role in ["runner", "viewer"] {
            fixtures::private_write(
                &tenant_b.join(role),
                fixtures::synthetic_token(&format!("tenant-b-{role}")),
            );
        }
        fixtures::edit_json(&installation.join("tenants.json"), |registry| {
            registry["tenants"].as_array_mut().unwrap().push(json!({
                "tenant_id": "tenant-b",
                "data": tenant_b.join("audit.sqlite"),
                "runner_token_file": tenant_b.join("runner"),
                "viewer_token_file": tenant_b.join("viewer")
            }));
        });
    };
    // The registry alone gains a tenant: the two files disagree.
    let registry_only = scenario.prepare("registry-only", &manifest);
    add_tenant_b(&registry_only);
    // The registry and the plan of the file both gain it. The files agree;
    // the audit that the preparation created still stores the plan of one tenant.
    let both = scenario.prepare("both", &manifest);
    add_tenant_b(&both);
    fixtures::edit_json(&both.join("oidc.json"), |oidc| {
        oidc["authentication_audit"]["capacity_plan"] =
            fixtures::capacity_plan(&["tenant-a", "tenant-b"]);
    });
    let untouched = scenario.prepare("untouched", &manifest);

    let run =
        |installation: &Path| scenario.diagnose_both(["--installation-dir", arg(installation)]);
    let (_, mismatch_json, mismatch) = run(&registry_only);
    let (_, agree_json, agree) = run(&both);
    let gateway = |installation: &Path| {
        scenario.gateway(
            &installation.join("tenants.json"),
            Some(&installation.join("oidc.json")),
        )
    };
    let control = gateway(&untouched);
    assert!(
        control.listening(),
        "positive control: the unmodified preparation starts: {}",
        control.describe()
    );

    // Editing the plan of the file makes the two files agree, and the gateway
    // still refuses: the stored plan is the third party, which no build reads
    // offline. The report says so beside the match.
    assert_eq!(agree["tenants"]["correspondence"], "match");
    finding(&agree, "tenants.plan_matches_registry");
    assert_eq!(finding(&agree, durable)["status"], "not_available");
    assert_eq!(agree_json.code, 0);
    assert_gateway_refuses(
        &gateway(&both),
        "differs from the durable plan",
        "a plan edited in the file, against the plan that the audit stores",
    );

    // A mismatch therefore never tells the operator to edit the plan, and it
    // lists the comparison that stays out of reach.
    let found = finding(&mismatch, "tenants.plan_set_mismatch");
    let action = text_of(found, "action");
    assert!(
        action.contains("Restore the registry to the tenant set of the capacity plan")
            && action.contains("its stored plan cannot be changed")
            && !action.contains("Make the registry and the plan name exactly the same tenants"),
        "the action follows the product: {action}"
    );
    // The plan of a file is free as long as no audit stores one: before the
    // first initialisation of a hand-written file as well as for the
    // migration. The action does not reserve it to the migration.
    assert!(
        action.contains("while no audit of format 4 exists")
            && action.contains("before the audit of a hand-written file is first initialised")
            && !action.contains("only for the 3 to 4 migration"),
        "the action is stricter than the product about the plan of the file: {action}"
    );
    assert_eq!(
        find(&mismatch, durable).map(|finding| &finding["status"]),
        Some(&json!("not_available")),
        "a mismatch lists the comparison with the stored plan as well: {:?}",
        codes(&mismatch)
    );
    assert_eq!(mismatch_json.code, 3);
}
// ---- review: the cause of each refusal of the engine approval -----------------

#[test]
fn each_engine_refusal_names_its_cause() {
    use agentsandbox::runtime::{Runtime, RuntimeKind};
    let scenario = Scenario::new();
    let config = scenario.historical("config");
    let program_in = |name: &str, directory_mode: u32, program_mode: u32| {
        let directory = scenario.path(name);
        fs::create_dir(&directory).unwrap();
        let program = directory.join("docker");
        fixtures::write_mode(&program, "not a program", program_mode);
        fs::set_permissions(&directory, fs::Permissions::from_mode(directory_mode)).unwrap();
        program
    };
    let not_executable = program_in("engine-not-executable", 0o700, 0o600);
    let open_directory = program_in("engine-open-directory", 0o777, 0o755);
    // A path that exists and resolves to nothing.
    let dangling = scenario.path("engine-dangling");
    fixtures::private_dir(&dangling);
    let dangling_program = dangling.join("docker");
    symlink(dangling.join("nowhere"), &dangling_program).unwrap();

    let mut actions = BTreeSet::new();
    for (name, program, message, action) in [
        (
            "a program that is not executable",
            &not_executable,
            "is not a regular executable file",
            "a regular executable file owned by root or the control account",
        ),
        (
            "a directory that other accounts can write",
            &open_directory,
            "The directory that holds the engine program",
            "a directory that only root or the control account can write",
        ),
        (
            "a link that leads nowhere",
            &dangling_program,
            "cannot be resolved",
            "the engine program itself",
        ),
    ] {
        let refusal = Runtime::with_program(RuntimeKind::Docker, program)
            .expect_err("the product refuses this program")
            .to_string();
        let (text, json, report) = scenario.diagnose_both([
            "--config-dir",
            arg(&config),
            "--runtime",
            "docker",
            "--runtime-program",
            arg(program),
        ]);
        let refused = finding(&report, "engine.program_refused");
        assert_eq!(
            refused["details"],
            json!({"kind": "reason", "reason": "program_or_directory_refused"}),
            "{name}"
        );
        assert!(
            text_of(refused, "message").contains(message),
            "{name}: the message names the cause: {}",
            refused["message"]
        );
        assert!(
            text_of(refused, "action").contains(action),
            "{name}: the action fits the cause: {}",
            refused["action"]
        );
        assert!(
            actions.insert(text_of(refused, "action").to_owned()),
            "{name}: an action of its own"
        );
        assert_eq!(json.code, 3, "{name}");
        let sentence = refusal.split(": ").next().unwrap();
        for output in [&text.stdout, &json.stdout] {
            assert!(
                !output.contains(sentence),
                "{name}: the text of the product error is printed"
            );
        }
    }
}
// ---- review: the fields behind each reason of the gateway block ---------------

/// One edit of the gateway block: its name, the closed reason the report
/// gives for it, the fields that reason is about, and the edit itself.
type GatewayBlockCase<'a> = (&'a str, &'a str, &'a [&'a str], &'a dyn Fn(&mut Value));

#[test]
fn gateway_block_refusals_name_their_fields() {
    let scenario = Scenario::new();
    let installation = scenario.prepare("https", &fixtures::manifest_https(&["tenant-a"], None));
    let oidc = installation.join("oidc.json");
    let original = fixtures::read_json(&oidc);
    let rule =
        json!({"claim_path": ["groups"], "equals": "operators", "capabilities": ["read_metadata"]});
    let cases: [GatewayBlockCase; 7] = [
        (
            "the block version",
            "version_or_token_bound",
            &[
                "gateway.version",
                "gateway.provider.limits.max_token_bytes",
                "gateway.provider.limits.pkce_ttl_seconds",
            ],
            &|file| file["gateway"]["version"] = json!(2),
        ),
        (
            "an unparseable web origin",
            "url_unparseable",
            &["gateway.web_origin", "gateway.provider.web_redirect_uri"],
            &|file| file["gateway"]["web_origin"] = json!("not a URL"),
        ),
        (
            "a web origin with a path",
            "web_origin_or_callback",
            &[
                "gateway.web_origin",
                "gateway.provider.web_redirect_uri",
                "gateway.provider.transport",
            ],
            &|file| file["gateway"]["web_origin"] = json!("https://guard.example.invalid/"),
        ),
        (
            "an empty provider label",
            "claim_mapping_invalid",
            &[
                "gateway.provider_label",
                "gateway.mapping.revision",
                "gateway.mapping.rules",
                "gateway.mapping.default_capabilities",
            ],
            &|file| file["gateway"]["provider_label"] = json!(""),
        ),
        (
            "two identical claim rules",
            "claim_mapping_ambiguous",
            &["gateway.mapping.rules"],
            &|file| file["gateway"]["mapping"]["rules"] = json!([rule, rule]),
        ),
        (
            "one client for web and CLI",
            "provider_block_invalid",
            &["gateway.provider"],
            &|file| {
                let web = file["gateway"]["provider"]["web_client_id"].clone();
                file["gateway"]["provider"]["cli_client_id"] = web;
            },
        ),
        (
            "an http issuer under the https_only transport",
            "provider_block_invalid",
            &["gateway.provider"],
            &|file| {
                file["gateway"]["provider"]["issuer"] =
                    json!("http://login.example.invalid/realms/company");
            },
        ),
    ];
    for (name, reason, fields, edit) in cases {
        let mut edited = original.clone();
        edit(&mut edited);
        fixtures::overwrite(&oidc, serde_json::to_vec_pretty(&edited).unwrap());
        let (_, json, report) = scenario.diagnose_both(["--installation-dir", arg(&installation)]);
        let refused = finding(&report, "oidc.gateway_block_invalid");
        assert_eq!(
            refused["details"],
            json!({"kind": "reason", "reason": reason}),
            "{name}"
        );
        let expected: Vec<Value> = fields
            .iter()
            .map(|field| json!({"file": "oidc.json", "field": field}))
            .collect();
        assert_eq!(
            refused["origin"]["sources"],
            json!(expected),
            "{name}: the fields that the reason is about"
        );
        // One closed value stands for every rule of the provider object: its
        // message says so, and no other reason carries that message.
        let message = text_of(refused, "message");
        assert_eq!(
            message.contains("one closed value for every rule of that object")
                && message.contains("Which rule failed is not known"),
            reason == "provider_block_invalid",
            "{name}: {message}"
        );
        assert!(
            text_of(refused, "action").contains("Correct the fields listed under basis"),
            "{name}: {}",
            refused["action"]
        );
        assert_eq!(json.code, 3, "{name}");
    }
    // Positive control of every edit: the original file passes.
    fixtures::overwrite(&oidc, serde_json::to_vec_pretty(&original).unwrap());
    let (_, json, report) = scenario.diagnose_both(["--installation-dir", arg(&installation)]);
    assert!(find(&report, "oidc.gateway_block_invalid").is_none());
    assert_eq!(json.code, 0);
}
// ---- review: the demonstration transport of an OIDC file ----------------------

#[test]
fn the_demonstration_transport_is_a_warning() {
    let code = "identity.sso_demonstration_transport";
    let scenario = Scenario::new();
    let demo = scenario.prepare("demo", &fixtures::manifest_v2(&["tenant-a"]));
    let (text, json, report) = scenario.diagnose_both(["--installation-dir", arg(&demo)]);
    assert_eq!(report["identity"]["sso"]["transport"], "isolated_demo_http");
    let warning = find(&report, code).unwrap_or_else(|| {
        panic!(
            "an OIDC file on the demonstration transport has no finding that says so; result {}",
            report["summary"]
        )
    });
    assert_eq!(warning["status"], "warning");
    assert_eq!(warning["basis"], "declared");
    assert_eq!(warning["axis"], "identity");
    assert_eq!(
        warning["origin"]["sources"],
        json!([{"file": "oidc.json", "field": "gateway.provider.transport"}])
    );
    assert!(text_of(warning, "message").contains("plain HTTP"));
    assert_eq!(report["summary"]["counts"]["warning"], 1);
    assert_eq!(json.code, 0, "a warning leaves the exit code unchanged");
    assert!(
        text.stdout
            .contains("result     OK - 0 error, 0 unknown, 1 warning;"),
        "the result line carries the warning"
    );
    assert!(text.stdout.contains(&format!("  [warning] {code}\n")));

    // The normal transport has no such finding.
    let https = scenario.prepare("https", &fixtures::manifest_https(&["tenant-a"], None));
    let (_, json, report) = scenario.diagnose_both(["--installation-dir", arg(&https)]);
    assert_eq!(report["identity"]["sso"]["transport"], "https_only");
    assert!(find(&report, code).is_none());
    assert_eq!(report["summary"]["counts"]["warning"], 0);
    assert_eq!(json.code, 0);
}
// ---- review: the local HTTP permission together with a CA bundle --------------

#[test]
fn the_local_http_permission_with_a_bundle_is_named_as_the_cause() {
    use agentsandbox::jev::{JevTrust, validate_transport_configuration};
    let scenario = Scenario::new();
    let config = scenario.historical("config");
    let bundle = scenario.path("public-ca.pem");
    fixtures::private_write(&bundle, fixtures::PUBLIC_TEST_ROOT);
    let remote = "https://engine.example.invalid/v1";
    let trust = JevTrust::from_ca_file(&bundle)
        .expect("positive control: the product accepts the public test certificate");
    assert!(
        validate_transport_configuration(remote, false, &trust).is_ok()
            && validate_transport_configuration(remote, true, &trust).is_err(),
        "the product accepts this HTTPS URL with the bundle, and refuses the same pair with the local HTTP permission"
    );
    let (_, json, report) = scenario.diagnose_both([
        "--config-dir",
        arg(&config),
        "--jev-url",
        remote,
        "--jev-allow-local-http",
        "--jev-ca-file",
        arg(&bundle),
    ]);
    let refused = finding(&report, "trust.decision_transport_invalid");
    assert_eq!(refused["status"], "error");
    let action = text_of(refused, "action");
    assert!(
        action.contains("refused together, whatever the URL"),
        "an HTTPS URL is already in use: the action names the cause: {action}"
    );
    assert_eq!(json.code, 3);
    // Without the permission the same URL and bundle are accepted.
    let (_, json, report) = scenario.diagnose_both([
        "--config-dir",
        arg(&config),
        "--jev-url",
        remote,
        "--jev-ca-file",
        arg(&bundle),
    ]);
    finding(&report, "inference.decision_transport_valid");
    assert_eq!(json.code, 0);
}
// ---- review: three actions of the local mode ----------------------------------

#[test]
fn an_absent_directory_says_to_check_the_path_first() {
    let scenario = Scenario::new();
    let missing = scenario.path("missing");
    for flag in ["--config-dir", "--installation-dir"] {
        let (_, _, report) = scenario.diagnose_both([flag, arg(&missing)]);
        let action = text_of(finding(&report, "layout.target_absent"), "action");
        assert!(
            action.starts_with("Check the path first"),
            "{flag}: a mistyped path is the first thing to rule out: {action}"
        );
    }
    let (_, _, report) = scenario.diagnose_both(NO_ARGS);
    assert!(
        text_of(finding(&report, "layout.target_absent"), "action")
            .starts_with("If an installation exists elsewhere, pass it with --config-dir DIR or --installation-dir DIR.")
    );
}

#[test]
fn a_missing_key_of_a_historical_directory_names_init() {
    let scenario = Scenario::new();
    let config = scenario.historical("config");
    let viewer = fs::read(config.join("secrets/viewer")).unwrap();
    fs::remove_file(config.join("secrets/runner")).unwrap();
    let (_, json, report) = scenario.diagnose_both(["--config-dir", arg(&config)]);
    let absent = finding(&report, "reference.absent");
    let action = text_of(absent, "action");
    assert!(
        action.contains("agentsandbox init --config-dir \"<config>\"")
            && action.contains("preserves the existing ones"),
        "the action names the command that recreates a missing key: {action}"
    );
    assert_eq!(json.code, 3);
    // The product does what the action says, on a copy.
    let copy = scenario.path("config-copy");
    fixtures::copy_tree(&config, &copy);
    let init = scenario.product(["init", "--config-dir", arg(&copy)]);
    assert_eq!(init.code, Some(0), "{}", init.stderr_text());
    assert!(
        copy.join("secrets/runner").is_file(),
        "init recreates the missing key"
    );
    assert_eq!(
        fs::read(copy.join("secrets/viewer")).unwrap(),
        viewer,
        "init preserves the existing key"
    );
    // A registry reference has no such command.
    let directory = scenario.path("registry");
    fixtures::private_dir(&directory);
    let registry = fixtures::hand_written_registry(&directory, &["tenant-a"]);
    fs::remove_file(directory.join("tenant-a/runner")).unwrap();
    let (_, _, report) = scenario.diagnose_both(["--tenants-file", arg(&registry)]);
    assert!(!text_of(finding(&report, "reference.absent"), "action").contains("agentsandbox init"));
}

#[test]
fn a_historical_directory_with_an_oidc_file_states_the_single_tenant_rule() {
    let scenario = Scenario::new();
    let historical = scenario.historical("historical");
    let prepared = scenario.prepare("prepared", &fixtures::manifest_v2(&["tenant-a"]));
    let (_, _, report) = scenario.diagnose_both([
        "--config-dir",
        arg(&historical),
        "--oidc-config",
        arg(&prepared.join("oidc.json")),
    ]);
    finding(&report, "topology.single_local_tenant");
    let not_compared = finding(&report, "tenants.correspondence_not_checked");
    assert_eq!(not_compared["status"], "not_checked");
    let action = text_of(not_compared, "action");
    assert!(
        action.contains("serves the single tenant local") && action.contains("--tenants-file FILE"),
        "the action states the rule of a gateway started without a registry: {action}"
    );
    // An OIDC file alone says nothing of the directory the gateway uses.
    let (_, _, report) =
        scenario.diagnose_both(["--oidc-config", arg(&prepared.join("oidc.json"))]);
    assert!(
        !text_of(
            finding(&report, "tenants.correspondence_not_checked"),
            "action"
        )
        .contains("single tenant local")
    );
}
// ---- review: the cause of a refused directory and of a refused bundle ---------

#[test]
fn a_refused_directory_names_its_cause() {
    let scenario = Scenario::new();

    // A config directory left readable by others: its mode is the cause.
    let config = scenario.historical("config");
    fs::set_permissions(&config, fs::Permissions::from_mode(0o755)).unwrap();
    let (_, json, report) = scenario.diagnose_both(["--config-dir", arg(&config)]);
    let refused = finding(&report, "layout.posture_refused");
    assert!(
        text_of(refused, "message").contains("does not have mode 0700"),
        "{}",
        refused["message"]
    );
    assert_eq!(json.code, 3);
    // A private directory below a directory that its group can write: the
    // mode conforms, and the message says where to look.
    let open_parent = scenario.path("open-parent");
    fixtures::private_dir(&open_parent);
    let below = open_parent.join("config");
    agentsandbox::operations::initialize_config(&below).expect("product init");
    fs::set_permissions(&open_parent, fs::Permissions::from_mode(0o770)).unwrap();
    assert!(
        agentsandbox::control_fs::Directory::open_existing(&below).is_err(),
        "the product refuses a private directory below a directory that its group can write"
    );
    let (_, json, report) = scenario.diagnose_both(["--config-dir", arg(&below)]);
    let refused = finding(&report, "layout.posture_refused");
    let message = text_of(refused, "message");
    assert!(
        message.contains("has mode 0700") && message.contains("a directory above it"),
        "{message}"
    );
    // The cause that only macOS has is named too, from the product code.
    assert!(
        message.contains("On macOS, an extended access entry"),
        "{message}"
    );
    assert_eq!(json.code, 3);
    fs::set_permissions(&open_parent, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn a_refused_bundle_is_told_from_an_absent_one() {
    let scenario = Scenario::new();

    // A bundle given by option: absent, or refused by the rule of its reader.
    let historical = scenario.historical("historical");
    let bundles = scenario.path("bundles");
    fixtures::private_dir(&bundles);
    let writable = bundles.join("group-writable.pem");
    fixtures::write_mode(&writable, fixtures::PUBLIC_TEST_ROOT, 0o664);
    let missing = bundles.join("missing.pem");
    let message_for = |path: &Path| {
        let (_, json, report) = scenario.diagnose_both([
            "--config-dir",
            arg(&historical),
            "--gateway-ca-file",
            arg(path),
        ]);
        assert_eq!(json.code, 3);
        text_of(
            finding(&report, "trust.ca_unavailable_or_not_private"),
            "message",
        )
        .to_owned()
    };
    let absent = message_for(&missing);
    let refused = message_for(&writable);
    assert!(absent.contains("does not exist"), "{absent}");
    assert!(
        refused.contains("refused by the file rule of its consumer")
            && !refused.contains("does not exist"),
        "{refused}"
    );

    // The bundle that an OIDC file names: the same two causes.
    let https = scenario.prepare(
        "https",
        &fixtures::manifest_https(&["tenant-a"], Some(fixtures::PUBLIC_TEST_ROOT)),
    );
    let bundle = https.join("ca-bundle.pem");
    let oidc_message = || {
        let (_, json, report) = scenario.diagnose_both(["--installation-dir", arg(&https)]);
        assert_eq!(json.code, 3);
        text_of(
            finding(&report, "trust.ca_unavailable_or_not_private"),
            "message",
        )
        .to_owned()
    };
    fs::set_permissions(&bundle, fs::Permissions::from_mode(0o644)).unwrap();
    let refused = oidc_message();
    fs::remove_file(&bundle).unwrap();
    let absent = oidc_message();
    assert!(absent.contains("does not exist"), "{absent}");
    assert!(
        refused.contains("refused by the file rule of its consumer")
            && !refused.contains("does not exist"),
        "{refused}"
    );
}
// ---- review: options that a run cannot use ------------------------------------

#[test]
fn the_help_states_the_rules_between_options() {
    // A usage error points to the help: the help states the three rules.
    let scenario = Scenario::new();
    let help = scenario.diagnose(["--help"]).stdout;
    let help = help.split_whitespace().collect::<Vec<_>>().join(" ");
    for phrase in [
        "not together with --jev-local-url",
        "not together with --jev-url",
        "needs --runtime",
    ] {
        assert!(help.contains(phrase), "--help states: {phrase}");
    }
    for (args, name) in [
        (
            vec![
                "--jev-url",
                "https://a.invalid",
                "--jev-local-url",
                "https://b.invalid",
            ],
            "two URL options",
        ),
        (
            vec!["--runtime-program", "/usr/bin/docker"],
            "a program without its engine",
        ),
    ] {
        let run = scenario.diagnose(&args);
        assert_eq!(run.code, 2, "{name}");
        assert_eq!(
            run.stderr, "ai-guard-diagnose: invalid command line; run ai-guard-diagnose --help\n",
            "{name}"
        );
    }
}
#[test]
fn options_given_without_effect_are_said_so() {
    let without_url = "inference.decision_transport_inputs_without_url";
    let unused_bound = "registry.event_bound_not_used";
    let scenario = Scenario::new();
    let config = scenario.historical("config");

    // The local HTTP permission without a URL: listed, and said not judged.
    let (text, json, report) =
        scenario.diagnose_both(["--config-dir", arg(&config), "--jev-allow-local-http"]);
    assert_eq!(
        *input(&report, "jev_allow_local_http"),
        json!({"name": "jev_allow_local_http", "origin": "cli", "display": "given"})
    );
    let not_judged = find(&report, without_url).unwrap_or_else(|| {
        panic!(
            "the permission is accepted without effect and without notice; the report has {:?}",
            codes(&report)
        )
    });
    assert_eq!(not_judged["status"], "not_checked");
    assert_eq!(
        not_judged["requires"],
        "--jev-url URL or --jev-local-url URL"
    );
    assert_eq!(
        not_judged["scope"],
        json!({"kind": "input", "input": "jev_allow_local_http"})
    );
    assert_eq!(
        report["inference"]["decision_engine_transport"],
        Value::Null
    );
    assert_eq!(json.code, 0);
    assert!(
        text.stdout
            .contains(&format!("  [not_checked] {without_url}\n"))
    );

    // A bundle without a URL: judged by its grammar alone, and that is said.
    let bundle = scenario.path("public-ca.pem");
    fixtures::private_write(&bundle, fixtures::PUBLIC_TEST_ROOT);
    let (_, json, report) =
        scenario.diagnose_both(["--config-dir", arg(&config), "--jev-ca-file", arg(&bundle)]);
    finding(&report, "trust.decision_engine_ca_accepted_by_grammar");
    assert_eq!(
        finding(&report, without_url)["scope"],
        json!({"kind": "input", "input": "jev_ca_file"})
    );
    assert_eq!(json.code, 0);

    // With a URL the transport is judged, and what the gateway also requires
    // of its option set is stated.
    let (_, json, report) = scenario.diagnose_both([
        "--config-dir",
        arg(&config),
        "--jev-local-url",
        "http://127.0.0.1:9/v1",
        "--jev-allow-local-http",
    ]);
    assert!(find(&report, without_url).is_none());
    let valid = finding(&report, "inference.decision_transport_valid");
    assert!(
        text_of(valid, "action").contains("refuses an incomplete or mixed set"),
        "{}",
        valid["action"]
    );
    assert!(
        text_of(
            finding(&report, "inference.decision_engine_selection_not_persisted"),
            "message"
        )
        .contains("refuses an incomplete or mixed set")
    );
    assert_eq!(json.code, 0);

    // The product: a gateway started with the permission and no
    // decision-engine URL refuses to start.
    let directory = scenario.path("registry");
    fixtures::private_dir(&directory);
    let registry = fixtures::hand_written_registry(&directory, &["tenant-a"]);
    let control = scenario.gateway_with_arguments(&registry, &[]);
    assert!(
        control.listening(),
        "positive control: the gateway starts without any decision-engine option: {}",
        control.describe()
    );
    for arguments in [
        vec!["--jev-allow-local-http"],
        vec!["--jev-ca-file", arg(&bundle)],
    ] {
        assert_gateway_refuses(
            &scenario.gateway_with_arguments(&registry, &arguments),
            "Incomplete or mixed Jev backend configuration",
            &format!("{arguments:?} without a decision-engine URL"),
        );
    }

    // An event bound that nothing uses.
    let (text, json, report) =
        scenario.diagnose_both(["--config-dir", arg(&config), "--max-events", "5000"]);
    let unused = find(&report, unused_bound).unwrap_or_else(|| {
        panic!(
            "an event bound that validates nothing is accepted without notice; the report has {:?}",
            codes(&report)
        )
    });
    assert_eq!(unused["status"], "not_checked");
    assert_eq!(
        unused["scope"],
        json!({"kind": "input", "input": "max_events"})
    );
    assert_eq!(json.code, 0);
    assert!(
        text.stdout
            .contains(&format!("  [not_checked] {unused_bound}\n"))
    );
    // A registry that declares no policy depending on the bound.
    let (_, _, report) =
        scenario.diagnose_both(["--tenants-file", arg(&registry), "--max-events", "5000"]);
    finding(&report, unused_bound);
    // A policy that depends on it: the bound is used, and nothing is said.
    fixtures::edit_json(&registry, |file| {
        file["tenants"][0]["audit_capacity_policy"] = json!({
            "version": 1,
            "human_tenant": {"events": 150_000, "bytes": 134_217_728},
            "human_principal": {"events": 12, "bytes": 33_554_432},
            "control_reserve": {"events": 25, "bytes": 67_108_864},
            "identity_reserve": {"events": 12, "bytes": 33_554_432}
        });
    });
    let (_, json, report) =
        scenario.diagnose_both(["--tenants-file", arg(&registry), "--max-events", "1000000"]);
    assert!(find(&report, unused_bound).is_none());
    finding(&report, "registry.offline_rules_passed");
    assert_eq!(json.code, 0);
}
// ---- review: a published installation without its authentication audit --------

#[test]
fn a_published_installation_without_its_audit_is_an_error() {
    let missing = "data.authentication_audit_missing_from_prepared_installation";
    let scenario = Scenario::new();
    let manifest = fixtures::manifest_v2(&["tenant-a"]);
    // The audit and any sidecar it left: neither the store nor a part of it.
    let remove_audit = |installation: &Path| {
        fs::remove_file(installation.join("authentication.sqlite")).unwrap();
        for suffix in ["-wal", "-shm", "-journal"] {
            let _ = fs::remove_file(installation.join(format!("authentication.sqlite{suffix}")));
        }
    };
    let control = scenario.prepare("control", &manifest);
    let installation = scenario.prepare("installation", &manifest);
    remove_audit(&installation);

    let (text, json, report) = scenario.diagnose_both(["--installation-dir", arg(&installation)]);
    let audit = entries(&report, "data")
        .iter()
        .find(|entry| entry["role"] == "authentication_audit")
        .cloned()
        .expect("the authentication audit is listed");
    assert_eq!(audit["presence"], "absent");
    let found = find(&report, missing).unwrap_or_else(|| {
        panic!(
            "a published installation without its audit is not an error; result {}; codes {:?}",
            report["summary"],
            codes(&report)
        )
    });
    assert_eq!(found["status"], "error");
    assert_eq!(found["axis"], "data");
    assert_eq!(found["basis"], "observed");
    assert!(find(&report, "data.authentication_audit_absent").is_none());
    assert_eq!(json.code, 3);
    assert!(text.stdout.contains(&format!("  [error] {missing}\n")));

    // The documented start, without --initialize-oidc-audit.
    let gateway = |installation: &Path| {
        scenario.gateway(
            &installation.join("tenants.json"),
            Some(&installation.join("oidc.json")),
        )
    };
    let started = gateway(&control);
    assert!(
        started.listening(),
        "positive control: the unmodified preparation starts: {}",
        started.describe()
    );
    assert_gateway_refuses(
        &gateway(&installation),
        "existing audit database is required",
        "a published installation without its audit",
    );

    // An OIDC file given alone, before any audit exists: absence is expected
    // there, and stays a warning.
    let alone = scenario.prepare("alone", &manifest);
    remove_audit(&alone);
    let (_, json, report) =
        scenario.diagnose_both(["--oidc-config", arg(&alone.join("oidc.json"))]);
    assert_eq!(
        finding(&report, "data.authentication_audit_absent")["status"],
        "warning"
    );
    assert!(find(&report, missing).is_none());
    assert_eq!(json.code, 0, "{}", report["summary"]);
}
// ---- review: the store names that the gateway refuses -------------------------

/// The product source that holds the name rule of the stores.
const STORE_NAME_SOURCE: &str = include_str!("../src/audit/storage.rs");

#[test]
fn store_names_that_the_gateway_refuses_are_refused() {
    let scenario = Scenario::new();
    // Each case: a hand-written registry of its own, with one store name.
    let case = |index: usize, store: &str| {
        let directory = scenario.path(&format!("registry-{index}"));
        fixtures::private_dir(&directory);
        let registry = fixtures::hand_written_registry(&directory, &["tenant-a"]);
        fixtures::edit_json(&registry, |file| {
            file["tenants"][0]["data"] = json!(format!("tenant-a/{store}"));
        });
        let (_, json, report) = scenario.diagnose_both(["--tenants-file", arg(&registry)]);
        // After the report: the gateway creates the store it accepts.
        let gateway = scenario.gateway(&registry, None);
        (json.code, report, gateway)
    };
    let (code, report, gateway) = case(0, "audit.sqlite");
    assert!(
        gateway.listening(),
        "positive control: the gateway accepts an ordinary store name: {}",
        gateway.describe()
    );
    finding(&report, "data.store_absent");
    assert!(find(&report, "data.store_posture_refused").is_none());
    assert_eq!(code, 0);

    for (index, store) in [
        "donn\u{e9}es.sqlite",
        "audit-journal",
        "AUDIT-WAL",
        "audit-shm",
        ".ai-guard-sqlite-store",
    ]
    .into_iter()
    .enumerate()
    {
        let (code, report, gateway) = case(index + 1, store);
        assert_gateway_refuses(
            &gateway,
            "audit database filename",
            &format!("the store name {store:?}"),
        );
        assert!(
            find(&report, "data.store_absent").is_none(),
            "{store:?}: the report announces a store that the gateway refuses to create"
        );
        let refused = finding(&report, "data.store_posture_refused");
        assert_eq!(refused["status"], "error", "{store:?}");
        assert!(
            text_of(refused, "message").contains("for its file name"),
            "{store:?}: {}",
            refused["message"]
        );
        assert_eq!(code, 3, "{store:?}");
    }

    // The rule is restated from the product: its source still holds each part.
    for part in [
        "name.as_bytes().is_ascii()",
        "to_ascii_lowercase()",
        "b\".ai-guard-sqlite-\"",
        "\"-wal\"",
        "\"-shm\"",
        "\"-journal\"",
    ] {
        assert!(
            STORE_NAME_SOURCE.contains(part),
            "the product no longer holds {part:?} in the name rule of the stores: the restated rule must be read again"
        );
    }
}
// ---- review: gateway files in a directory without setup records ---------------

#[test]
fn gateway_files_without_setup_records_name_the_file_options() {
    let scenario = Scenario::new();
    let prepared = scenario.prepare("prepared", &fixtures::manifest_v2(&["tenant-a"]));
    // A directory that holds the two gateway files and no setup record.
    let directory = scenario.path("gateway-files");
    fixtures::private_dir(&directory);
    for name in ["tenants.json", "oidc.json"] {
        fixtures::private_write(
            &directory.join(name),
            fs::read(prepared.join(name)).unwrap(),
        );
    }
    // A profile file is what the tool must read in this run.
    let credentials = fixtures::provider_credentials(&scenario.path("credentials"));
    let providers = scenario.path("providers.json");
    fixtures::write_providers(&providers, &fixtures::four_profiles(&credentials));

    let (text, json, report) = scenario.diagnose_both([
        "--installation-dir",
        arg(&directory),
        "--providers-file",
        arg(&providers),
    ]);
    assert_eq!(report["layouts"][0]["kind"], "unrecognized");
    let unrecognized = finding(&report, "layout.unrecognized");
    assert!(
        text_of(unrecognized, "message").contains("at their fixed names"),
        "the message says what the directory holds: {}",
        unrecognized["message"]
    );
    let action = text_of(unrecognized, "action");
    assert!(
        action.contains("--tenants-file FILE") && action.contains("--oidc-config FILE"),
        "the action names the two options that read these files: {action}"
    );
    assert_eq!(json.code, 3);
    assert!(text.stdout.contains("  [error] layout.unrecognized\n"));
    assert_eq!(
        report["mode"]["files_read_for_content"],
        json!(["provider_profiles"]),
        "neither gateway file is read"
    );
    let observed = observed_run(
        &scenario,
        &[
            "--installation-dir",
            arg(&directory),
            "--providers-file",
            arg(&providers),
            "--json",
        ],
    );
    assert_never_read(
        &observed,
        "providers.json",
        &["gateway-files/tenants.json", "gateway-files/oidc.json"],
        "gateway files in a directory that is not inspected as an installation",
    );

    // The two options the action names do read them.
    let (_, _, report) = scenario.diagnose_both([
        "--tenants-file",
        arg(&directory.join("tenants.json")),
        "--oidc-config",
        arg(&directory.join("oidc.json")),
    ]);
    assert_eq!(report["tenants"]["correspondence"], "match");

    // The directory given together with the two options: the files are read
    // through the options. The message about the directory says nothing of
    // what was opened: the list of the files read does.
    let (_, json, report) = scenario.diagnose_both([
        "--config-dir",
        arg(&directory),
        "--tenants-file",
        arg(&directory.join("tenants.json")),
        "--oidc-config",
        arg(&directory.join("oidc.json")),
    ]);
    assert_eq!(
        report["mode"]["files_read_for_content"],
        json!(["tenant_registry", "oidc_file"]),
        "the two options read the files of the directory"
    );
    let message = text_of(finding(&report, "layout.unrecognized"), "message");
    assert!(
        message.contains("at their fixed names") && !message.contains("opened"),
        "the message about the directory denies a read that the report lists: {message}"
    );
    assert_eq!(json.code, 3);

    // An empty directory keeps the generic text.
    let empty = scenario.path("empty");
    fixtures::private_dir(&empty);
    let (_, _, report) = scenario.diagnose_both(["--installation-dir", arg(&empty)]);
    assert!(!text_of(finding(&report, "layout.unrecognized"), "action").contains("--tenants-file"));
}
// ---- review: a mode check is not an acceptance --------------------------------

#[test]
fn no_text_says_that_a_consumer_accepts_a_file_that_was_not_read() {
    use agentsandbox::diagnostics::report::Axis;
    for entry in CATALOGUE {
        if entry.axis != Axis::CredentialReferences {
            continue;
        }
        for text in entry.texts() {
            assert!(
                !text.contains("accepts this") && !text.contains("accepts the file"),
                "{}: a text states that a consumer accepts a file whose content was not read: {text}",
                entry.code.as_str()
            );
        }
    }

    // The report of a key file that other accounts can read, by the binary.
    let scenario = Scenario::new();
    let directory = scenario.path("registry");
    fixtures::private_dir(&directory);
    let registry = fixtures::hand_written_registry(&directory, &["tenant-a"]);
    fs::set_permissions(
        directory.join("tenant-a/runner"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let (_, json, report) = scenario.diagnose_both(["--tenants-file", arg(&registry)]);
    let accessible = finding(&report, "reference.token_file_accessible_to_other_accounts");
    assert_eq!(accessible["status"], "warning");
    let message = text_of(accessible, "message");
    assert!(
        !message.contains("accepts"),
        "the message states what the reader does, not an acceptance: {message}"
    );
    assert_eq!(json.code, 0);

    // A mode that grants the read to others and denies it to the owner: the
    // same warning, and a file that the gateway cannot open. The message is
    // therefore no promise that a mode is never the cause of a refusal. The
    // file cannot be read by this test either, so the run is made without
    // the manifest comparison, which reads every file.
    let denied = scenario.path("owner-denied");
    fixtures::private_dir(&denied);
    let registry = fixtures::hand_written_registry(&denied, &["tenant-a"]);
    let runner = denied.join("tenant-a/runner");
    fs::set_permissions(&runner, fs::Permissions::from_mode(0o044)).unwrap();
    let run = scenario.diagnose_unobserved(["--tenants-file", arg(&registry), "--json"]);
    let gateway = scenario.gateway(&registry, None);
    let opened_by_this_account = fs::File::open(&runner).is_ok();
    fs::set_permissions(&runner, fs::Permissions::from_mode(0o600)).unwrap();
    if opened_by_this_account {
        oracle_not_run(
            "gateway-refusal-of-an-unreadable-key",
            "this account opens a file whose mode grants it no read",
        );
    } else {
        assert_gateway_refuses(
            &gateway,
            "Impossible de lire le fichier de secret configuré",
            "a key file whose mode denies its owner the read",
        );
    }
    let report = run.json();
    let accessible = finding(&report, "reference.token_file_accessible_to_other_accounts");
    assert_eq!(accessible["status"], "warning");
    let message = text_of(accessible, "message");
    assert!(
        !message.contains("does not refuse a file for its mode"),
        "the message denies a refusal that the gateway gives for the mode of this file: {message}"
    );
    assert!(
        message.contains("checks no mode")
            && message.contains("that the account of the gateway can open for reading"),
        "the message says what the reader does with a mode: {message}"
    );
    // The tool does not open the key: what the gateway does with it is left
    // to the item that says so.
    finding(&report, "registry.loader_acceptance_not_checked");
    assert_eq!(run.code, 0);
}
// ---- review: a registry that the gateway refuses -------------------------------

#[test]
fn a_refused_registry_is_reported_with_its_cause() {
    let scenario = Scenario::new();
    let registry_scope = json!({"kind": "file", "file": "tenant_registry"});
    let subject = |id: &str| json!([{"id": id, "masked": false}]);
    let quota_above_the_limit = json!({
        "version": 1, "max_active_workers": 101, "max_cpu_millis": 6000,
        "max_memory_bytes": 3_221_225_472_u64, "max_timeout_seconds": 3600
    });
    let hand_written = |name: &str| {
        let directory = scenario.path(name);
        fixtures::private_dir(&directory);
        fixtures::hand_written_registry(&directory, &["tenant-a", "tenant-b"])
    };

    // The control: the hand-written registry of two tenants, unedited.
    let control = hand_written("control");
    let (_, json, report) = scenario.diagnose_both(["--tenants-file", arg(&control)]);
    finding(&report, "registry.offline_rules_passed");
    assert_eq!(json.code, 0, "{}", report["summary"]);
    let started = scenario.gateway(&control, None);
    assert!(
        started.listening(),
        "positive control: the gateway starts with the unedited registry: {}",
        started.describe()
    );

    // Each case is that registry plus one stated edit, with the tenant the
    // finding names and the cause the gateway states.
    type Case<'a> = (
        &'a str,
        &'a dyn Fn(&Path),
        &'a str,
        Option<&'a str>,
        &'a str,
    );
    let edit = |registry: &Path, change: &dyn Fn(&mut Value)| fixtures::edit_json(registry, change);
    let cases: [Case; 7] = [
        (
            "invalid JSON",
            &|registry| fixtures::overwrite(registry, "{ not json"),
            "registry.json_invalid",
            None,
            "Registre de tenants invalide",
        ),
        (
            "version 2",
            &|registry| edit(registry, &|file| file["version"] = 2.into()),
            "registry.version_unknown",
            None,
            "Version de registre de tenants inconnue",
        ),
        (
            "an empty tenant list",
            &|registry| edit(registry, &|file| file["tenants"] = json!([])),
            "registry.empty",
            None,
            "Le registre de tenants est vide",
        ),
        (
            "an identifier used twice",
            &|registry| {
                edit(registry, &|file| {
                    file["tenants"][1]["tenant_id"] = "tenant-a".into()
                })
            },
            "registry.tenant_id_duplicate",
            Some("tenant-a"),
            "Identifiant de tenant dupliqué",
        ),
        (
            "an identifier with a space",
            &|registry| {
                edit(registry, &|file| {
                    file["tenants"][1]["tenant_id"] = "tenant b".into()
                })
            },
            "registry.tenant_id_invalid",
            Some("tenant b"),
            "tenant identity must contain 1 to 128 ASCII letters",
        ),
        (
            "an empty store reference",
            &|registry| edit(registry, &|file| file["tenants"][1]["data"] = "".into()),
            "registry.reference_empty",
            Some("tenant-b"),
            "Référence de fichier vide",
        ),
        (
            "a worker quota above the limit of the product",
            &|registry| {
                edit(registry, &|file| {
                    file["tenants"][1]["worker_quota"] = quota_above_the_limit.clone();
                });
            },
            "registry.policy_invalid",
            Some("tenant-b"),
            "Quota workers invalide",
        ),
    ];
    for (index, (name, change, code, tenant, cause)) in cases.into_iter().enumerate() {
        let registry = hand_written(&format!("case-{index}"));
        change(&registry);
        let (text, json, report) = scenario.diagnose_both(["--tenants-file", arg(&registry)]);
        let refused = find(&report, code).unwrap_or_else(|| {
            panic!(
                "{name}: the report has no {code}; result {}; codes {:?}",
                report["summary"],
                codes(&report)
            )
        });
        assert_eq!(refused["status"], "error", "{name}");
        assert_eq!(refused["axis"], "tenants", "{name}");
        assert_eq!(refused["scope"], registry_scope, "{name}");
        assert_eq!(refused["origin"]["assumes"], json!([]), "{name}");
        if let Some(tenant) = tenant {
            assert_eq!(refused["subjects"]["ids"], subject(tenant), "{name}");
        }
        match code {
            "registry.json_invalid" => {
                assert_eq!(refused["details"]["kind"], "json_error", "{name}");
                assert_eq!(refused["details"]["category"], "syntax", "{name}");
                assert_eq!(refused["details"]["line"], 1, "{name}");
            }
            "registry.policy_invalid" => assert_eq!(
                refused["details"],
                json!({"kind": "policy", "policy": "worker_quota"}),
                "{name}"
            ),
            _ => {}
        }
        assert!(
            find(&report, "registry.offline_rules_passed").is_none(),
            "{name}: a refused registry is not said to pass"
        );
        assert_eq!(json.code, 3, "{name}: {}", report["summary"]);
        assert!(
            text.stdout.contains(&format!("  [error] {code}\n")),
            "{name}: the text report lists the finding"
        );
        // After the report: a gateway that accepted the registry would
        // create its stores.
        assert_gateway_refuses(&scenario.gateway(&registry, None), cause, name);
    }

    // The registry of a prepared installation, with one of these edits.
    let manifest = fixtures::manifest_v2(&["tenant-a", "tenant-b"]);
    let prepared_control = scenario.prepare("prepared-control", &manifest);
    let prepared = scenario.prepare("prepared", &manifest);
    fixtures::edit_json(&prepared.join("tenants.json"), |file| {
        file["tenants"][1]["worker_quota"] = quota_above_the_limit.clone();
    });
    let edited_tenant =
        fixtures::read_json(&prepared.join("tenants.json"))["tenants"][1]["tenant_id"]
            .as_str()
            .expect("the registry of a preparation names its tenants")
            .to_owned();
    let (text, json, report) = scenario.diagnose_both(["--installation-dir", arg(&prepared)]);
    let refused = finding(&report, "registry.policy_invalid");
    assert_eq!(refused["status"], "error");
    assert_eq!(refused["scope"], registry_scope);
    assert_eq!(refused["subjects"]["ids"], subject(&edited_tenant));
    assert_eq!(
        refused["details"],
        json!({"kind": "policy", "policy": "worker_quota"})
    );
    assert!(find(&report, "registry.offline_rules_passed").is_none());
    assert_eq!(json.code, 3, "{}", report["summary"]);
    assert!(text.stdout.contains("  [error] registry.policy_invalid\n"));
    let gateway = |installation: &Path| {
        scenario.gateway(
            &installation.join("tenants.json"),
            Some(&installation.join("oidc.json")),
        )
    };
    let started = gateway(&prepared_control);
    assert!(
        started.listening(),
        "positive control: the unmodified preparation starts: {}",
        started.describe()
    );
    assert_gateway_refuses(
        &gateway(&prepared),
        "Quota workers invalide",
        "the registry of a prepared installation with a quota above the limit",
    );

    // The event bound. This policy needs 87 events: the gateway accepts it at
    // its default, and a run that is given no bound says that its verdict
    // rests on that default.
    let bounded = hand_written("bounded");
    fixtures::edit_json(&bounded, |file| {
        file["tenants"][0]["audit_capacity_policy"] = json!({
            "version": 1,
            "human_tenant": {"events": 50, "bytes": 134_217_728},
            "human_principal": {"events": 12, "bytes": 33_554_432},
            "control_reserve": {"events": 25, "bytes": 67_108_864},
            "identity_reserve": {"events": 12, "bytes": 33_554_432}
        });
    });
    // The real gateway, on copies: it creates the stores it opens.
    let gateway_with_bound = |name: &str, max_events: Option<&str>| {
        let copy = scenario.path(name);
        fixtures::copy_tree(bounded.parent().unwrap(), &copy);
        scenario.gateway_with_event_bound(&copy.join("tenants.json"), max_events)
    };
    let at_default = gateway_with_bound("bounded-at-default", None);
    let at_64 = gateway_with_bound("bounded-at-64", Some("64"));

    let (text, json, report) = scenario.diagnose_both(["--tenants-file", arg(&bounded)]);
    assert_eq!(
        finding(&report, "registry.offline_rules_passed")["origin"]["assumes"],
        json!(["max_events"]),
        "the verdict names the input it assumes"
    );
    assert_eq!(
        *input(&report, "max_events"),
        json!({"name": "max_events", "origin": "default", "display": "100000"})
    );
    assert!(text.stdout.contains(
        "\nassumed    gateway event bound 100000 (default; the gateway's value is not stored anywhere)\n"
    ));
    for code in [
        "registry.policy_invalid",
        "registry.policy_refused_at_assumed_event_bound",
    ] {
        assert!(find(&report, code).is_none(), "{code}");
    }
    assert_eq!(json.code, 0, "{}", report["summary"]);
    assert!(
        at_default.listening(),
        "positive control: the gateway accepts this policy at its default bound: {}",
        at_default.describe()
    );

    // The bound of a gateway that refuses it, given: an observed error, and
    // nothing is assumed.
    let (text, json, report) =
        scenario.diagnose_both(["--tenants-file", arg(&bounded), "--max-events", "64"]);
    let invalid = finding(&report, "registry.policy_invalid");
    assert_eq!(invalid["status"], "error");
    assert_eq!(invalid["scope"], registry_scope);
    assert_eq!(
        invalid["details"],
        json!({"kind": "policy", "policy": "audit_capacity"})
    );
    assert_eq!(invalid["origin"]["assumes"], json!([]));
    assert_eq!(invalid["subjects"]["ids"], subject("tenant-a"));
    assert_eq!(
        *input(&report, "max_events"),
        json!({"name": "max_events", "origin": "cli", "display": "64"})
    );
    for code in [
        "registry.offline_rules_passed",
        "registry.policy_refused_at_assumed_event_bound",
    ] {
        assert!(find(&report, code).is_none(), "{code}");
    }
    assert_eq!(json.code, 3, "{}", report["summary"]);
    assert!(text.stdout.contains("  [error] registry.policy_invalid\n"));
    assert_gateway_refuses(
        &at_64,
        "audit capacity policy must preserve reserves",
        "a gateway started with --max-events 64",
    );
}

// ---- review: an OIDC file that the gateway refuses -----------------------------

#[test]
fn a_refused_oidc_file_is_reported_with_its_cause() {
    let scenario = Scenario::new();
    let manifest = fixtures::manifest_v2(&["tenant-a", "tenant-b"]);
    let gateway = |installation: &Path, oidc: &Path| {
        scenario.gateway(&installation.join("tenants.json"), Some(oidc))
    };
    let codes_of_this_test = [
        "oidc.file_absent",
        "oidc.capacity_plan_invalid",
        "oidc.authentication_policy_invalid",
        "oidc.reference_invalid",
    ];

    // The control: the OIDC file of an unmodified preparation, given alone.
    let control = scenario.prepare("control", &manifest);
    let (_, json, report) =
        scenario.diagnose_both(["--oidc-config", arg(&control.join("oidc.json"))]);
    for code in codes_of_this_test {
        assert!(find(&report, code).is_none(), "the control has no {code}");
    }
    assert_eq!(json.code, 0, "{}", report["summary"]);
    let started = gateway(&control, &control.join("oidc.json"));
    assert!(
        started.listening(),
        "positive control: the unmodified preparation starts: {}",
        started.describe()
    );

    let reported = |name: &str, oidc: &Path, code: &str| {
        let (text, json, report) = scenario.diagnose_both(["--oidc-config", arg(oidc)]);
        let refused = find(&report, code).unwrap_or_else(|| {
            panic!(
                "{name}: the report has no {code}; result {}; codes {:?}",
                report["summary"],
                codes(&report)
            )
        });
        assert_eq!(refused["status"], "error", "{name}");
        assert_eq!(refused["axis"], "identity", "{name}");
        if code == "oidc.authentication_policy_invalid" {
            // The gateway applies the policy check before it touches the
            // audit, and refuses a plan that the policy cannot fund only once
            // the audit is open: the message claims no more than that.
            let message = text_of(refused, "message");
            assert!(
                message.contains("only after it has opened or created the audit")
                    && !message.contains("before it opens storage"),
                "{name}: the message says when the gateway applies each check: {message}"
            );
        }
        assert_eq!(json.code, 3, "{name}: {}", report["summary"]);
        assert!(
            text.stdout.contains(&format!("  [error] {code}\n")),
            "{name}: the text report lists the finding"
        );
    };

    // A path where no OIDC file exists, beside a preparation of its own.
    let beside = scenario.prepare("absent", &manifest);
    let absent = scenario.path("no-oidc-file.json");
    reported("an absent OIDC file", &absent, "oidc.file_absent");
    assert_gateway_refuses(
        &gateway(&beside, &absent),
        "private file cannot be opened without following a link",
        "an absent OIDC file",
    );

    // Each case is a preparation of its own, with one stated edit of its
    // OIDC file and the cause the gateway states.
    type Case<'a> = (&'a str, &'a dyn Fn(&mut Value), &'a str, &'a str);
    let cases: [Case; 4] = [
        (
            "a plan that names a tenant twice",
            &|file| {
                file["authentication_audit"]["capacity_plan"]["tenants"][1]["tenant_id"] =
                    "tenant-a".into();
            },
            "oidc.capacity_plan_invalid",
            "invalid authentication audit input",
        ),
        (
            "a policy without any record budget",
            &|file| file["authentication_audit"]["policy"]["max_records"] = 0.into(),
            "oidc.authentication_policy_invalid",
            "invalid authentication audit input",
        ),
        // The audit of this preparation stores the policy it was created
        // with: the gateway opens it, then refuses the edited policy for
        // being another policy. Its text says nothing of the funding of the
        // plan: the last case of this test shows the same refusal for a
        // policy that funds it.
        (
            "a policy that cannot fund the plan",
            &|file| file["authentication_audit"]["policy"]["max_records"] = 500.into(),
            "oidc.authentication_policy_invalid",
            "authentication audit integrity failure",
        ),
        (
            "an empty audit path",
            &|file| file["authentication_audit"]["path"] = "".into(),
            "oidc.reference_invalid",
            "Empty OIDC file reference",
        ),
    ];
    for (index, (name, change, code, cause)) in cases.into_iter().enumerate() {
        let installation = scenario.prepare(&format!("case-{index}"), &manifest);
        let oidc = installation.join("oidc.json");
        fixtures::edit_json(&oidc, change);
        reported(name, &oidc, code);
        assert_gateway_refuses(&gateway(&installation, &oidc), cause, name);
    }

    // What the integrity text above stands for. This policy passes both
    // validators, it funds the plan, and it is not the one the audit was
    // created with. The tool, which does not open the audit, reports no
    // policy finding and says that the acceptance of the gateway is not
    // checked; the gateway refuses with the same text as for the policy that
    // cannot fund the plan.
    let name = "a policy that funds the plan and is not the one of the audit";
    let installation = scenario.prepare("another-policy", &manifest);
    let oidc = installation.join("oidc.json");
    fixtures::edit_json(&oidc, |file| {
        file["authentication_audit"]["policy"]["max_records"] = 2000.into();
    });
    let (_, json, report) = scenario.diagnose_both(["--oidc-config", arg(&oidc)]);
    for code in codes_of_this_test {
        assert!(find(&report, code).is_none(), "{name}: no {code}");
    }
    finding(&report, "identity.sso_file_valid_offline");
    finding(&report, "oidc.loader_acceptance_not_checked");
    assert_eq!(json.code, 0, "{name}: {}", report["summary"]);
    assert_gateway_refuses(
        &gateway(&installation, &oidc),
        "authentication audit integrity failure",
        name,
    );
}

// ---- review: the stores of a running installation, and of a refused one -------

#[test]
fn the_stores_of_a_running_installation_and_of_a_refused_one() {
    let scenario = Scenario::new();
    let manifest = fixtures::manifest_v2(&["tenant-a"]);
    let tenant_a = json!([{"id": "tenant-a", "masked": false}]);
    let store_of = |installation: &Path| {
        let registry = fixtures::read_json(&installation.join("tenants.json"));
        installation.join(text_of(&registry["tenants"][0], "data"))
    };
    let beside = |store: &Path, suffix: &str| {
        let mut name = store.as_os_str().to_owned();
        name.push(suffix);
        PathBuf::from(name)
    };
    let data_entry = |report: &Value, role: &str| {
        entries(report, "data")
            .iter()
            .find(|entry| entry["role"] == role)
            .cloned()
            .unwrap_or_else(|| panic!("no data entry has the role {role}: {}", report["data"]))
    };
    let gateway_files = |installation: &Path| {
        (
            installation.join("tenants.json"),
            installation.join("oidc.json"),
        )
    };

    // A fresh preparation: setup closed its stores, nothing stands beside them.
    let live = scenario.prepare("live", &manifest);
    let (_, json, report) = scenario.diagnose_both(["--installation-dir", arg(&live)]);
    assert!(find(&report, "data.sidecars_present").is_none());
    for role in ["tenant_store", "authentication_audit"] {
        assert_eq!(data_entry(&report, role)["sidecars"], json!([]), "{role}");
    }
    assert_eq!(json.code, 0, "{}", report["summary"]);

    // Its gateway, running: it holds the stores open in the write-ahead mode
    // of SQLite. The tool is run without the manifest comparison here: the
    // tree belongs to the gateway while it runs.
    let (registry, oidc) = gateway_files(&live);
    let mut running = scenario
        .gateway_running(&registry, Some(&oidc))
        .unwrap_or_else(|start| {
            panic!(
                "positive control: the unmodified preparation starts and keeps running: {}",
                start.describe()
            )
        });
    let while_running = scenario.diagnose_unobserved(["--installation-dir", arg(&live), "--json"]);
    assert!(
        running.still_runs(),
        "the gateway runs on after the inspection of its installation"
    );
    drop(running);
    let report = assert_round_trip(&while_running, "the report of a running installation");
    let present = finding(&report, "data.sidecars_present");
    assert_eq!(present["status"], "info");
    assert_eq!(present["axis"], "data");
    assert_eq!(present["basis"], "observed");
    let store = data_entry(&report, "tenant_store");
    let audit = data_entry(&report, "authentication_audit");
    eprintln!(
        "OBSERVED beside the stores of a running gateway: tenant store {}, authentication audit {}",
        store["sidecars"], audit["sidecars"]
    );
    assert_eq!(store["presence"], "present");
    assert_eq!(store["sidecars"], json!(["wal", "shm"]));
    assert_eq!(audit["presence"], "present");
    assert_eq!(
        present["details"],
        json!({"kind": "sidecars", "sidecars": ["wal", "shm"]})
    );
    for code in [
        "data.orphan_sidecars",
        "data.store_posture_refused",
        "data.parent_directory_not_private",
    ] {
        assert!(find(&report, code).is_none(), "{code}");
    }
    assert_eq!(
        while_running.code, 0,
        "the stores of a running installation are no error: {}",
        report["summary"]
    );

    // The gateway was stopped without a clean close: its sidecars stay. The
    // tree is still again, so this run is also the no-mutation check: the
    // tool leaves the sidecars as they are.
    let (text, json, report) = scenario.diagnose_both(["--installation-dir", arg(&live)]);
    assert_eq!(
        data_entry(&report, "tenant_store")["sidecars"],
        json!(["wal", "shm"])
    );
    assert!(text.stdout.contains("  [info] data.sidecars_present\n"));
    assert_eq!(json.code, 0, "{}", report["summary"]);

    // Each edit below is a fresh preparation with one stated change of its
    // tenant store. The installation above is their control: its gateway
    // started.
    type Case<'a> = (&'a str, &'a dyn Fn(&Path), &'a str, &'a str, Value, &'a str);
    let mode = |path: &Path, mode: u32| {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    };
    let cases: [Case; 3] = [
        (
            "a sidecar without its store",
            &|store| {
                fs::remove_file(store).unwrap();
                fixtures::private_write(&beside(store, "-wal"), "");
            },
            "data.orphan_sidecars",
            "absent",
            json!(["wal"]),
            "orphan SQLite sidecars exist",
        ),
        (
            "a store that other accounts can read",
            &|store| mode(store, 0o644),
            "data.store_posture_refused",
            "present",
            json!([]),
            "private database entry requires a regular single-link file",
        ),
        (
            "a store directory that other accounts can enter",
            &|store| mode(store.parent().unwrap(), 0o755),
            "data.parent_directory_not_private",
            "present",
            json!([]),
            "private control directory requires effective-UID ownership and mode 0700",
        ),
    ];
    for (index, (name, change, code, presence, sidecars, cause)) in cases.into_iter().enumerate() {
        let installation = scenario.prepare(&format!("case-{index}"), &manifest);
        change(&store_of(&installation));
        let (text, json, report) =
            scenario.diagnose_both(["--installation-dir", arg(&installation)]);
        let refused = find(&report, code).unwrap_or_else(|| {
            panic!(
                "{name}: the report has no {code}; result {}; codes {:?}",
                report["summary"],
                codes(&report)
            )
        });
        assert_eq!(refused["status"], "error", "{name}");
        assert_eq!(refused["axis"], "data", "{name}");
        assert_eq!(refused["basis"], "observed", "{name}");
        assert_eq!(refused["subjects"]["ids"], tenant_a, "{name}");
        let entry = data_entry(&report, "tenant_store");
        assert_eq!(entry["presence"], presence, "{name}");
        assert_eq!(entry["sidecars"], sidecars, "{name}");
        match code {
            "data.orphan_sidecars" => assert_eq!(
                refused["details"],
                json!({"kind": "sidecars", "sidecars": ["wal"]}),
                "{name}"
            ),
            "data.store_posture_refused" => assert!(
                text_of(refused, "message").contains("private-entry rule"),
                "{name}: the posture of the entry is the cause: {}",
                refused["message"]
            ),
            _ => {}
        }
        assert_eq!(json.code, 3, "{name}: {}", report["summary"]);
        assert!(
            text.stdout.contains(&format!("  [error] {code}\n")),
            "{name}: the text report lists the finding"
        );
        let (registry, oidc) = gateway_files(&installation);
        assert_gateway_refuses(&scenario.gateway(&registry, Some(&oidc)), cause, name);
    }

    // A recorded path that leaves its directory and comes back: the stores
    // refuse it as written. A hand-written registry, with the same registry
    // unedited as its control.
    let hand_written = |name: &str| {
        let directory = scenario.path(name);
        fixtures::private_dir(&directory);
        fixtures::hand_written_registry(&directory, &["tenant-a"])
    };
    let plain = hand_written("plain-path");
    let started = scenario.gateway(&plain, None);
    assert!(
        started.listening(),
        "positive control: the gateway accepts the unedited registry: {}",
        started.describe()
    );
    let traversal = hand_written("path-with-traversal");
    fixtures::edit_json(&traversal, |file| {
        file["tenants"][0]["data"] = "tenant-a/../tenant-a/audit.sqlite".into();
    });
    let (_, json, report) = scenario.diagnose_both(["--tenants-file", arg(&traversal)]);
    let refused = finding(&report, "data.store_posture_refused");
    assert_eq!(refused["status"], "error");
    assert!(
        text_of(refused, "message").contains("parent traversal"),
        "{}",
        refused["message"]
    );
    assert!(find(&report, "data.store_absent").is_none());
    assert_eq!(json.code, 3, "{}", report["summary"]);
    assert_gateway_refuses(
        &scenario.gateway(&traversal, None),
        "control path must not contain parent traversal",
        "a store path with a parent traversal",
    );
}

// ---- review: the model of a profile ---------------------------------------------

#[test]
fn the_model_of_a_profile_is_resolved_as_the_product_resolves_it() {
    let scenario = Scenario::new();
    let config = scenario.historical("config");
    let credentials = fixtures::provider_credentials(&scenario.path("credentials"));
    let providers = config.join("providers.json");
    let declared = "claude-haiku-5-5";
    let write = |model: &str| {
        let mut with_model =
            fixtures::api_profile("claude-api", "claude_code", &credentials.claude_api);
        with_model["model"] = model.into();
        fixtures::write_providers(
            &providers,
            &json!({"version": 1, "profiles": [
                with_model,
                fixtures::api_profile("codex-api", "codex", &credentials.codex_api)
            ]}),
        );
    };
    let listed = |report: &Value, id: &str| {
        entries(&report["providers"], "profiles")
            .iter()
            .find(|profile| profile["profile"]["id"] == id)
            .cloned()
            .unwrap_or_else(|| panic!("the profile {id} is not listed: {}", report["providers"]))
    };
    let by_name = Ask {
        profile: Some("claude-api"),
        ..Ask::default()
    };

    // No model is requested: the product gives the client the model of the
    // profile, and the report shows that one.
    write(declared);
    let resolution = product_resolution(&scenario, &providers, &by_name)
        .expect("positive control: the product resolves a profile that declares a model");
    assert_eq!(
        resolution.model.as_deref(),
        Some(declared),
        "the product falls back to the model of the profile"
    );
    let (text, json, report) = diagnose_selection(&scenario, &config, &by_name);
    assert_resolved_as_the_product(&report, &resolution, "the model of the profile");
    let selection = &report["providers"]["selection"];
    assert_eq!(selection["requested"]["model"], "auto");
    assert_eq!(selection["resolved"]["model"], declared);
    assert_eq!(listed(&report, "claude-api")["model"], declared);
    assert_eq!(listed(&report, "codex-api")["model"], Value::Null);
    assert!(
        text.stdout.contains(declared),
        "the text report shows the model that is resolved"
    );
    assert_eq!(json.code, 0, "{}", report["summary"]);

    // A model is requested: the option wins, as in the product, and the
    // profile is still listed with its own.
    let requested = "requested-model-1";
    let with_option = Ask {
        model: Some(requested),
        ..by_name
    };
    let resolution = product_resolution(&scenario, &providers, &with_option)
        .expect("positive control: the product accepts this requested model");
    assert_eq!(resolution.model.as_deref(), Some(requested));
    let (_, json, report) = diagnose_selection(&scenario, &config, &with_option);
    assert_resolved_as_the_product(&report, &resolution, "a requested model");
    let selection = &report["providers"]["selection"];
    assert_eq!(selection["requested"]["model"], "explicit");
    assert_eq!(selection["resolved"]["model"], requested);
    assert_eq!(listed(&report, "claude-api")["model"], declared);
    assert_eq!(json.code, 0, "{}", report["summary"]);

    // The model of a profile that the product accepts and that the output
    // filter rewrites: it is shown masked wherever it appears.
    let credential_like = "sk-mnopqrstuvwxyz0123";
    let masked = Redactor::new(Vec::new()).text(credential_like);
    assert_ne!(
        masked, credential_like,
        "fixture: the filter rewrites this model"
    );
    write(credential_like);
    let resolution = product_resolution(&scenario, &providers, &by_name)
        .expect("positive control: the product accepts this model in a profile");
    assert_eq!(resolution.model.as_deref(), Some(credential_like));
    let (text, json, report) = diagnose_selection(&scenario, &config, &by_name);
    assert_eq!(
        report["providers"]["selection"]["resolved"]["model"],
        masked
    );
    assert_eq!(listed(&report, "claude-api")["model"], masked);
    assert_eq!(finding(&report, "output.value_masked")["status"], "warning");
    for run in [&text, &json] {
        assert_no_canary(run, &[credential_like], "a credential-like profile model");
    }
    assert_eq!(json.code, 0, "{}", report["summary"]);
}

// ---- review: the selection that `run` builds from its options ------------------

/// The product source that builds the selection of `agentsandbox run`.
const RUN_SOURCE: &str = include_str!("../src/runner.rs");

#[test]
fn the_selection_of_run_is_built_as_it_is_restated() {
    // `run` builds its selection from four options. This tool
    // (`selection_request`, src/diagnostics.rs) and `Ask::selection` above
    // restate that mapping; the choice itself is the product's. Each line of
    // the mapping still stands, once, in the source of `run`: a mapping that
    // changes fails here, before a report predicts another selection than the
    // one `run` makes. Only the presence of the lines is checked.
    let normalised = |source: &str| source.split_whitespace().collect::<Vec<_>>().join(" ");
    let run = normalised(RUN_SOURCE);
    assert_eq!(
        run.matches("None | Some(\"auto\") => Some(").count(),
        0,
        "calibration: a line that `run` does not hold is seen as absent"
    );
    for line in [
        "let client = match options.client.as_deref() {",
        "None | Some(\"auto\") => None,",
        "Some(\"codex\") => Some(crate::provider::ClientKind::Codex),",
        "Some(\"claude-code\") => Some(crate::provider::ClientKind::ClaudeCode),",
        "&crate::provider::ProviderSelection { client,",
        "auth_mode: options.auth_mode.unwrap_or_default(),",
        "profile_id: options.client_profile.clone(),",
        "model: options.model.clone(),",
    ] {
        assert_eq!(
            run.matches(line).count(),
            1,
            "src/runner.rs no longer builds its selection with `{line}`: read the mapping of `run` again, then `selection_request` in src/diagnostics.rs and `Ask::selection` in this file"
        );
    }
}

// ---- composition regression: explicit audit tools remain distinct -----------

#[test]
fn audit_guidance_is_scoped_to_the_diagnostic() {
    let scenario = Scenario::new();
    let prepared = scenario.prepare(
        "prepared-audit-guidance",
        &fixtures::manifest_v2(&["tenant-a"]),
    );
    let historical = scenario.historical("historical-audit-guidance");
    for (option, directory) in [
        ("--installation-dir", &prepared),
        ("--config-dir", &historical),
    ] {
        let (_, _, report) = scenario.diagnose_both([option, arg(directory)]);
        let offline = finding(&report, "data.offline_inspection_not_available");
        assert_eq!(offline["status"], "not_available");
        assert_eq!(offline["axis"], "data");
        assert_eq!(offline["basis"], "observed");
        assert!(text_of(offline, "message").contains("This diagnostic never opens"));
        assert!(!text_of(offline, "message").contains("This build has no offline"));
        let checkpoint = finding(&report, "data.signed_checkpoint_not_checked");
        assert_eq!(checkpoint["status"], "not_checked");
        assert_eq!(checkpoint["axis"], "data");
        assert_eq!(checkpoint["basis"], "observed");
        assert!(text_of(checkpoint, "requires").contains("ai-guard-audit-checkpoint verify"));
        assert!(text_of(checkpoint, "requires").contains("independently held trust anchor"));
        assert!(text_of(checkpoint, "message").contains("not verified"));
        if option == "--installation-dir" {
            let layouts = finding(&report, "layout.legacy_commands_do_not_read_this_layout");
            assert!(text_of(layouts, "message").contains("verify-audit"));
            assert!(text_of(layouts, "action").contains("--viewer-token-file"));
            assert!(
                !text_of(layouts, "message").contains("None of them reads a prepared installation")
            );
        }
    }
}

// Explicit one-shot QA export. It never updates source golden files and never
// relaxes assert_documented or any normal regression. All data is synthetic.
#[test]
#[ignore = "manual bounded export into TMPDIR; Root must validate and promote the nine reports"]
fn regenerate_documented_examples_into_tmpdir() {
    use sha2::{Digest, Sha256};
    use std::{
        io::{Read, Seek, SeekFrom, Write},
        os::unix::fs::{MetadataExt, OpenOptionsExt},
    };

    const MAX_FILE_BYTES: usize = 524_288;
    const MAX_TOTAL_BYTES: usize = 4_718_592;

    fn record_pair(
        generated: &mut Vec<(&'static str, String)>,
        names: (&'static str, Option<&'static str>),
        text: Run,
        json: Run,
        report: &Value,
        expected_exit: i32,
    ) {
        assert_eq!(text.code, expected_exit);
        assert_eq!(json.code, expected_exit);
        assert!(text.stderr.is_empty() && json.stderr.is_empty());
        assert_eq!(report["schema"], "ai-guard-diagnose-report");
        assert_eq!(report["version"], 1);
        assert_eq!(report["mode"]["network"], "none");
        assert_eq!(report["mode"]["mutation"], "none");
        assert_eq!(report["mode"]["credential_contents_read"], "none");
        assert_eq!(report["mode"]["active_checks_executed"], json!([]));
        generated.push((names.0, text.stdout));
        if let Some(name) = names.1 {
            generated.push((name, json.stdout));
        }
    }

    fn write_exclusive(directory: &Path, name: &str, bytes: &[u8]) -> Value {
        assert!(!name.is_empty() && name != "." && name != "..");
        assert!(
            name.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        );
        assert!(bytes.len() <= MAX_FILE_BYTES);
        let path = directory.join(name);
        let mut file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)
            .expect("create one new QA export file");
        file.write_all(bytes).unwrap();
        file.sync_all().unwrap();
        let before = file.metadata().unwrap();
        let path_before = fs::symlink_metadata(&path).unwrap();
        assert!(before.is_file() && path_before.is_file());
        assert_eq!(before.nlink(), 1);
        assert_eq!(path_before.nlink(), 1);
        assert_eq!(before.mode() & 0o777, 0o600);
        // SAFETY: geteuid has no pointer arguments and changes no process state.
        assert_eq!(before.uid(), unsafe { libc::geteuid() });
        assert_eq!(before.len(), bytes.len() as u64);
        assert_eq!(
            (before.dev(), before.ino()),
            (path_before.dev(), path_before.ino())
        );
        file.seek(SeekFrom::Start(0)).unwrap();
        let mut read_back = Vec::new();
        Read::by_ref(&mut file)
            .take(MAX_FILE_BYTES as u64 + 1)
            .read_to_end(&mut read_back)
            .unwrap();
        let after = file.metadata().unwrap();
        let path_after = fs::symlink_metadata(&path).unwrap();
        assert!(after.is_file() && path_after.is_file());
        assert_eq!(after.nlink(), 1);
        assert_eq!(path_after.nlink(), 1);
        assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
        assert_eq!(
            (path_after.dev(), path_after.ino()),
            (before.dev(), before.ino())
        );
        assert_eq!(after.mode() & 0o777, 0o600);
        assert_eq!(after.uid(), before.uid());
        assert_eq!(after.len(), before.len());
        assert_eq!(path_before.len(), before.len());
        assert_eq!(path_after.len(), before.len());
        assert_eq!(read_back, bytes);
        json!({"name": name, "bytes": bytes.len(), "mode": "0600", "sha256": hex::encode(Sha256::digest(bytes))})
    }

    let scenario = Scenario::new();
    let mut generated = Vec::new();
    let (text, json, report) = scenario.diagnose_both(NO_ARGS);
    assert_eq!(report["layouts"][0]["kind"], "absent");
    record_pair(
        &mut generated,
        ("report.first-run.txt", None),
        text,
        json,
        &report,
        3,
    );

    let fresh = scenario.historical("config");
    let (text, json, report) = scenario.diagnose_both(["--config-dir", arg(&fresh)]);
    assert_eq!(report["layouts"][0]["kind"], "historical_config_dir");
    record_pair(
        &mut generated,
        ("report.local-fresh.txt", Some("report.local-fresh.json")),
        text,
        json,
        &report,
        0,
    );

    let manifest = scenario.path("manifest.json");
    fixtures::private_write(&manifest, fixtures::LOCAL_DEMO_MANIFEST);
    let installation = scenario.path("installation");
    let prepared = scenario.product([
        "setup",
        "prepare",
        "--input",
        arg(&manifest),
        "--output-dir",
        arg(&installation),
    ]);
    assert_eq!(
        prepared.code,
        Some(0),
        "synthetic preparation: {}",
        prepared.stderr_text()
    );
    let (text, json, report) = scenario.diagnose_both(["--installation-dir", arg(&installation)]);
    assert_eq!(report["layouts"][0]["kind"], "prepared_installation");
    record_pair(
        &mut generated,
        ("report.prepared-v2.txt", Some("report.prepared-v2.json")),
        text,
        json,
        &report,
        0,
    );

    let config = scenario.path("try");
    let initialized = scenario.product(["init", "--config-dir", arg(&config)]);
    assert_eq!(
        initialized.code,
        Some(0),
        "synthetic init: {}",
        initialized.stderr_text()
    );
    let profiles = config.join("providers.two-modes.json");
    fixtures::private_write(&profiles, DOCUMENTED_PROFILES);
    for reference in ["secrets/providers.env", "secrets/claude-subscription.env"] {
        fixtures::private_write(&config.join(reference), "");
    }
    let (text, json, report) = scenario.diagnose_both([
        "--config-dir",
        arg(&config),
        "--providers-file",
        arg(&profiles),
    ]);
    assert_eq!(
        report["providers"]["selection"]["resolved"]["profile"],
        json!({"id": "codex-subscription", "masked": false})
    );
    finding(&report, "provider.login_required");
    finding(&report, "provider.subscription_never_falls_back_to_api");
    assert!(find(&report, "provider.api_billing_notice").is_none());
    record_pair(
        &mut generated,
        (
            "report.providers-two-modes.txt",
            Some("report.providers-two-modes.json"),
        ),
        text,
        json,
        &report,
        3,
    );

    let (text, json, report) = scenario.diagnose_both([
        "--config-dir",
        arg(&config),
        "--providers-file",
        arg(&profiles),
        "--client",
        "claude-code",
        "--auth-mode",
        "subscription",
    ]);
    assert_eq!(
        report["providers"]["selection"]["resolved"]["profile"],
        json!({"id": "claude-subscription", "masked": false})
    );
    assert_eq!(
        finding(&report, "provider.unselected_profile_reference_absent")["details"],
        json!({"kind": "would_refuse_with", "class": "login_required"})
    );
    record_pair(
        &mut generated,
        (
            "report.providers-explicit-subscription.txt",
            Some("report.providers-explicit-subscription.json"),
        ),
        text,
        json,
        &report,
        0,
    );

    let names: BTreeSet<_> = generated.iter().map(|(name, _)| *name).collect();
    assert_eq!(generated.len(), 9);
    assert_eq!(
        names,
        DOCUMENTED_REPORTS.iter().map(|(name, _)| *name).collect()
    );
    let temporary = std::env::var_os("TMPDIR").expect("QA must supply its private scratch TMPDIR");
    assert!(Path::new(&temporary).is_absolute());
    let output = tempfile::Builder::new()
        .prefix("ai-guard-diagnose-examples-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(temporary)
        .unwrap()
        .keep();
    let directory = fs::symlink_metadata(&output).unwrap();
    assert!(directory.is_dir());
    assert_eq!(directory.mode() & 0o777, 0o700);
    // SAFETY: geteuid has no pointer arguments and changes no process state.
    assert_eq!(directory.uid(), unsafe { libc::geteuid() });
    let raw_dir = output.join("raw");
    let normalized_dir = output.join("normalized");
    fixtures::private_dir(&raw_dir);
    fixtures::private_dir(&normalized_dir);
    let mut total = 0_usize;
    let mut outputs = Vec::new();
    for (name, raw) in generated {
        assert!(raw.len() <= MAX_FILE_BYTES);
        assert!(
            !raw.contains(arg(&scenario.root)),
            "export contains an absolute fixture path"
        );
        let normalized = if name.ends_with(".json") {
            let report: Value = serde_json::from_str(&raw).unwrap();
            assert_eq!(report["tool"]["package_version"], env!("CARGO_PKG_VERSION"));
            let version_field = format!("\"package_version\": \"{}\"", env!("CARGO_PKG_VERSION"));
            assert_eq!(raw.matches(&version_field).count(), 1);
            raw.replacen(
                &version_field,
                "\"package_version\": \"PACKAGE_VERSION\"",
                1,
            )
        } else {
            let header = format!(
                "ai-guard-diagnose {} - report version 1\n",
                env!("CARGO_PKG_VERSION")
            );
            assert!(raw.starts_with(&header));
            raw.replacen(
                &header,
                "ai-guard-diagnose PACKAGE_VERSION - report version 1\n",
                1,
            )
        };
        assert_eq!(
            normalized.replace(VERSION_PLACEHOLDER, env!("CARGO_PKG_VERSION")),
            raw
        );
        total = total
            .checked_add(raw.len())
            .and_then(|n| n.checked_add(normalized.len()))
            .unwrap();
        assert!(total <= MAX_TOTAL_BYTES);
        outputs.push(json!({"name": name, "raw": write_exclusive(&raw_dir, name, raw.as_bytes()), "normalized": write_exclusive(&normalized_dir, name, normalized.as_bytes())}));
    }
    let manifest = serde_json::to_vec_pretty(&json!({
        "schema": "ai-guard-diagnose-example-export", "version": 1,
        "synthetic_fixtures_only": true, "package_version": env!("CARGO_PKG_VERSION"),
        "source_binding": "parent QA retains compiled source manifest", "outputs": outputs
    }))
    .unwrap();
    assert!(total.checked_add(manifest.len()).unwrap() <= MAX_TOTAL_BYTES);
    let manifest_fact = write_exclusive(&output, "MANIFEST.json", &manifest);
    eprintln!("EXAMPLES_EXPORT_DIRECTORY {}", output.display());
    eprintln!("EXAMPLES_EXPORT_MANIFEST {}", manifest_fact);
    eprintln!("EXAMPLES_EXPORT_CONFIRMED nine normalized and nine raw reports; source unchanged");
}
