//! Offline installation CLI over real private SQLite, without an IdP or engine.
mod support;

use agentsandbox::{
    audit::{
        Store,
        authentication::{AuthenticationAudit, AuthenticationAuditPolicyV1},
    },
    identity::PrincipalRefV1,
    redaction::Redactor,
};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    os::unix::fs::{PermissionsExt, symlink},
    path::Path,
    process::{Child, ChildStdin, Command, Output, Stdio},
    thread::sleep,
    time::{Duration, Instant},
};

const ISSUER: &str = "http://keycloak.ai-guard.test:18080/realms/ai-guard-demo";
const MAX_SETUP_STDIN_BYTES: usize = 262_144;
const SETUP_STDIN_TEST_BOUND: Duration = Duration::from_secs(8);

fn manifest() -> Value {
    json!({
        "version":1,
        "gateway":{
            "version":1,"provider_label":"Explicit demo","web_origin":"http://127.0.0.1:18100",
            "provider":{
                "version":1,"issuer":ISSUER,"web_client_id":"web-public","cli_client_id":"cli-public","api_audience":"api-resource",
                "authorization_endpoint":format!("{ISSUER}/protocol/openid-connect/auth"),
                "token_endpoint":format!("{ISSUER}/protocol/openid-connect/token"),
                "jwks_uri":format!("{ISSUER}/protocol/openid-connect/certs"),
                "web_redirect_uri":"http://127.0.0.1:18100/auth/oidc/callback","cli_callback_path":"/oidc/callback",
                "transport":{"kind":"isolated_demo_http","exact_issuer":ISSUER}
            },
            "mapping":{"revision":"demo-1","default_capabilities":["read_metadata"],"rules":[]}
        },
        "authentication_policy":{"max_records":1000,"max_serialized_bytes":1048576,"approved_clients":[
            {"issuer":ISSUER,"client_id":"web-public"},{"issuer":ISSUER,"client_id":"cli-public"}
        ]},
        "tenants":[
            {"tenant_id":"tenant-a","organization_id":"organization-a","identities":[
                {"principal":{"kind":"human_oidc","issuer":ISSUER,"subject":"stable-a"},"display_name":"admin","capabilities":["read_metadata","read_content"]}
            ]},
            {"tenant_id":"tenant-b","organization_id":"organization-b","identities":[
                {"principal":{"kind":"human_oidc","issuer":ISSUER,"subject":"stable-b"},"display_name":"User B","capabilities":["read_metadata"]}
            ]}
        ]
    })
}

fn prepare(input: &Value, path: &Path) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
        .args(["setup", "prepare", "--input", "-", "--output-dir"])
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // A base binary rejects the subcommand before reading stdin. Its exit is
    // the intended RED oracle, so an early broken pipe is not a fixture error.
    let _ = child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(input).unwrap());
    child.wait_with_output().unwrap()
}

fn spawn_prepare_with_stdin(path: &Path) -> (Child, ChildStdin) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
        .args(["setup", "prepare", "--input", "-", "--output-dir"])
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdin = child.stdin.take().unwrap();
    (child, stdin)
}

fn wait_for_setup_child(
    mut child: Child,
    keep_stdin_open: Option<ChildStdin>,
    bound: Duration,
) -> (Output, bool) {
    let deadline = Instant::now() + bound;
    let exited_before_bound = loop {
        if child.try_wait().unwrap().is_some() {
            break true;
        }
        let now = Instant::now();
        if now >= deadline {
            // This is the fresh child owned by this test. Reap only it; no
            // detached writer or reader thread can outlive the test.
            let _ = child.kill();
            child.wait().expect("timed-out setup child must be reaped");
            break false;
        }
        sleep((deadline - now).min(Duration::from_millis(20)));
    };
    drop(keep_stdin_open);
    let output = child.wait_with_output().unwrap();
    (output, exited_before_bound)
}

fn assert_no_setup_files(parent: &Path, output_dir: &Path) {
    assert!(
        !output_dir.exists(),
        "refused setup must not publish its destination"
    );
    assert!(
        fs::read_dir(parent).unwrap().next().is_none(),
        "refused setup must not leave staging files"
    );
}

fn resume(path: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
        .args(["setup", "resume", "--recovery-dir"])
        .arg(path)
        .arg("--confirm-control-stopped")
        .output()
        .unwrap()
}

fn success(output: &Output) {
    assert!(
        output.status.success(),
        "setup CLI must complete a fresh offline installation: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
fn principal(subject: &str) -> PrincipalRefV1 {
    PrincipalRefV1::HumanOidc {
        issuer: ISSUER.into(),
        subject: subject.into(),
    }
}
fn store(root: &Path, tenant: &str) -> Store {
    Store::open_existing_for_tenant(
        &root.join("tenants").join(tenant).join("audit.sqlite"),
        tenant,
        Redactor::new(vec![]),
        100_000,
    )
    .unwrap()
}

#[test]
fn prepare_creates_schema5_separate_audit_and_only_explicit_oidc_memberships() {
    let parent = support::private_tempdir();
    let root = parent.path().join("installation");
    let output = prepare(&manifest(), &root);
    success(&output);
    let oidc: Value = serde_json::from_slice(&fs::read(root.join("oidc.json")).unwrap()).unwrap();
    let policy: AuthenticationAuditPolicyV1 =
        serde_json::from_value(manifest()["authentication_policy"].clone()).unwrap();
    let installation =
        serde_json::from_value(oidc["authentication_audit"]["installation_id"].clone()).unwrap();
    let audit = AuthenticationAudit::open_existing(
        &root.join("authentication.sqlite"),
        installation,
        policy,
        Redactor::new(vec![]),
    )
    .unwrap();
    assert_eq!(
        audit.verify().unwrap().records,
        1,
        "preparation must not fabricate authentication"
    );
    for (tenant, own, other) in [
        ("tenant-a", "stable-a", "stable-b"),
        ("tenant-b", "stable-b", "stable-a"),
    ] {
        let tenant_store = store(&root, tenant);
        assert_eq!(tenant_store.supervision_state().unwrap().schema_version, 5);
        assert!(tenant_store.integrity().unwrap().valid);
        audit.ensure_distinct_from_business(&tenant_store).unwrap();
        assert_eq!(
            tenant_store
                .identity_memberships(&principal(own))
                .unwrap()
                .len(),
            1
        );
        assert!(
            tenant_store
                .identity_memberships(&principal(other))
                .unwrap()
                .is_empty()
        );
        assert!(
            tenant_store
                .human_account(&principal("admin"))
                .unwrap()
                .is_none(),
            "display name/master name must not enroll another subject"
        );
        for name in ["runner", "viewer"] {
            let path = root.join("tenants").join(tenant).join(name);
            let secret = fs::read_to_string(&path).unwrap();
            assert!(secret.trim().len() >= 48);
            assert!(!String::from_utf8_lossy(&output.stdout).contains(secret.trim()));
            assert!(!String::from_utf8_lossy(&output.stderr).contains(secret.trim()));
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o7777,
                0o600
            );
        }
    }
    assert_eq!(
        fs::metadata(&root).unwrap().permissions().mode() & 0o7777,
        0o700
    );
    assert!(root.join("preparation.json").is_file());
    assert!(root.join("completion.json").is_file());
}

#[test]
fn prepare_times_out_with_partial_stdin_open_before_any_filesystem_mutation() {
    let parent = support::private_tempdir();
    let root = parent.path().join("installation");
    let (child, mut stdin) = spawn_prepare_with_stdin(&root);
    stdin.write_all(b"{\"version\":").unwrap();
    stdin.flush().unwrap();

    let (output, exited_before_bound) =
        wait_for_setup_child(child, Some(stdin), SETUP_STDIN_TEST_BOUND);

    assert_no_setup_files(parent.path(), &root);
    assert!(
        exited_before_bound,
        "setup CLI did not time out while the parent kept partial stdin open"
    );
    assert!(
        !output.status.success(),
        "incomplete setup input must be refused"
    );
    let diagnostic = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    assert!(
        diagnostic.contains("input")
            && (diagnostic.contains("timeout")
                || diagnostic.contains("timed out")
                || diagnostic.contains("deadline exceeded")),
        "incomplete stdin must report an input timeout"
    );
}

#[test]
fn prepare_accepts_valid_json_written_in_two_chunks_and_closed_at_eof() {
    let parent = support::private_tempdir();
    let root = parent.path().join("installation");
    let (child, mut stdin) = spawn_prepare_with_stdin(&root);
    let bytes = serde_json::to_vec(&manifest()).unwrap();
    let split = bytes.len() / 2;
    stdin.write_all(&bytes[..split]).unwrap();
    stdin.flush().unwrap();
    sleep(Duration::from_millis(25));
    stdin.write_all(&bytes[split..]).unwrap();
    drop(stdin);

    // The five-second product deadline covers reading stdin. Successful
    // durable installation work happens after EOF, so this is only a generous
    // owned-child test guard, not a product completion-time promise.
    let (output, exited_before_bound) = wait_for_setup_child(child, None, Duration::from_secs(60));

    assert!(
        exited_before_bound,
        "complete setup input must finish within the test guard"
    );
    assert!(
        output.status.success(),
        "valid setup input must be accepted"
    );
    assert!(root.join("preparation.json").is_file());
    assert!(root.join("completion.json").is_file());
}

#[test]
fn prepare_rejects_oversized_stdin_without_waiting_for_eof_or_publishing_files() {
    let parent = support::private_tempdir();
    let root = parent.path().join("installation");
    let mut bytes = serde_json::to_vec(&manifest()).unwrap();
    assert!(bytes.len() < MAX_SETUP_STDIN_BYTES);
    bytes.resize(MAX_SETUP_STDIN_BYTES + 1, b' ');

    let (child, mut stdin) = spawn_prepare_with_stdin(&root);
    for chunk in bytes.chunks(8 * 1024) {
        if let Err(error) = stdin.write_all(chunk) {
            assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
            break;
        }
    }

    let (output, exited_before_bound) =
        wait_for_setup_child(child, Some(stdin), SETUP_STDIN_TEST_BOUND);

    assert_no_setup_files(parent.path(), &root);
    assert!(
        exited_before_bound,
        "oversized setup input was not rejected while stdin remained open"
    );
    assert!(
        !output.status.success(),
        "oversized setup input must be refused"
    );
    let diagnostic = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    assert!(
        diagnostic.contains("262144") && diagnostic.contains("exceed"),
        "oversized setup input must report the byte limit"
    );
}

#[test]
fn resume_replays_exact_durable_receipts_and_credentials_after_missing_completion() {
    let parent = support::private_tempdir();
    let root = parent.path().join("installation");
    success(&prepare(&manifest(), &root));
    let preparation = fs::read(root.join("preparation.json")).unwrap();
    let completion = fs::read(root.join("completion.json")).unwrap();
    let credential = fs::read(root.join("tenants/tenant-a/runner")).unwrap();
    let head = store(&root, "tenant-a").head().unwrap();
    fs::rename(
        root.join("completion.json"),
        parent.path().join("retained-completion.json"),
    )
    .unwrap();
    success(&resume(&root));
    success(&resume(&root));
    assert_eq!(
        fs::read(root.join("preparation.json")).unwrap(),
        preparation
    );
    assert_eq!(fs::read(root.join("completion.json")).unwrap(), completion);
    assert_eq!(
        fs::read(root.join("tenants/tenant-a/runner")).unwrap(),
        credential
    );
    assert_eq!(store(&root, "tenant-a").head().unwrap(), head);
    assert!(
        !prepare(&manifest(), &root).status.success(),
        "prepare never adopts existing installations"
    );
}

#[test]
fn broker_registry_membership_is_refused_before_any_setup_publication() {
    let parent = support::private_tempdir();
    let root = parent.path().join("installation");
    let mut input = manifest();
    input["tenants"][0]["identities"][0]["capabilities"] = json!(["read_broker_registry"]);
    let refused = prepare(&input, &root);
    assert!(!refused.status.success());
    let diagnostic = String::from_utf8_lossy(&refused.stderr);
    assert!(diagnostic.contains("prepare without read_broker_registry"));
    assert!(diagnostic.contains("migrate6→7"));
    assert_no_setup_files(parent.path(), &root);
}

#[test]
fn malformed_duplicates_cross_issuer_and_unsafe_destination_fail_before_publication() {
    let parent = support::private_tempdir();
    let mut cases = Vec::new();
    let mut unknown = manifest();
    unknown["unknown"] = json!(true);
    cases.push(unknown);
    let mut duplicate = manifest();
    duplicate["tenants"][1] = duplicate["tenants"][0].clone();
    cases.push(duplicate);
    let mut duplicate_identity = manifest();
    let identity = duplicate_identity["tenants"][0]["identities"][0].clone();
    duplicate_identity["tenants"][0]["identities"]
        .as_array_mut()
        .unwrap()
        .push(identity);
    cases.push(duplicate_identity);
    let mut wrong = manifest();
    wrong["tenants"][0]["identities"][0]["principal"]["issuer"] =
        json!("https://other.example.test");
    cases.push(wrong);
    let mut secret = manifest();
    secret["ca_bundle_pem"] =
        json!("-----BEGIN PRIVATE KEY-----\nsecret\n-----END PRIVATE KEY-----");
    cases.push(secret);
    let mut wrong_policy = manifest();
    wrong_policy["authentication_policy"]["approved_clients"][0]["client_id"] = json!("unapproved");
    cases.push(wrong_policy);
    for (index, input) in cases.iter().enumerate() {
        let path = parent.path().join(format!("invalid-{index}"));
        assert!(!prepare(input, &path).status.success());
        assert!(!path.exists());
    }
    let target = parent.path().join("target");
    fs::create_dir(&target).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o700)).unwrap();
    let link = parent.path().join("link");
    symlink(&target, &link).unwrap();
    assert!(
        !prepare(&manifest(), &link.join("installation"))
            .status
            .success()
    );
    assert!(fs::read_dir(target).unwrap().next().is_none());
}

#[test]
fn resume_refuses_substituted_credential_or_tenant_database_without_repair() {
    for database in [false, true] {
        let parent = support::private_tempdir();
        let root = parent.path().join("installation");
        success(&prepare(&manifest(), &root));
        let path = root.join(if database {
            "tenants/tenant-a/audit.sqlite"
        } else {
            "tenants/tenant-a/runner"
        });
        let saved = parent.path().join("retained-original");
        fs::rename(&path, &saved).unwrap();
        fs::write(&path, b"substitution-must-not-be-adopted").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(!resume(&root).status.success());
        assert_eq!(fs::read(path).unwrap(), b"substitution-must-not-be-adopted");
        assert!(saved.exists());
    }
}

#[test]
fn demo_override_guard_is_explicit_and_generic_prepare_ignores_ambient_demo_settings() {
    let parent = support::private_tempdir();
    let mut input = manifest();
    input["gateway"]["web_origin"] = json!("http://127.0.0.1:4000");
    input["gateway"]["provider"]["web_redirect_uri"] =
        json!("http://127.0.0.1:4000/auth/oidc/callback");
    for (name, value) in [
        ("AI_GUARD_DEMO_IDP_HOST", "unreviewed.example.test"),
        ("AI_GUARD_DEMO_IDP_PORT", "18888"),
        ("AI_GUARD_DEMO_WEB_ORIGIN", "http://127.0.0.1:4999"),
        ("AI_GUARD_GATEWAY_PORT", "8788"),
        ("AI_GUARD_DASHBOARD_PORT", "4001"),
    ] {
        let path = parent.path().join(name.to_ascii_lowercase());
        let mut child = Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
            .args([
                "setup",
                "prepare",
                "--demo-defaults",
                "--input",
                "-",
                "--output-dir",
            ])
            .arg(&path)
            .env(name, value)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(&input).unwrap())
            .unwrap();
        assert!(!child.wait_with_output().unwrap().status.success());
        assert!(!path.exists());
    }
    let path = parent.path().join("generic");
    let mut child = Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
        .args(["setup", "prepare", "--input", "-", "--output-dir"])
        .arg(&path)
        .env("AI_GUARD_DEMO_IDP_HOST", "unreviewed.example.test")
        .env("AI_GUARD_GATEWAY_PORT", "8788")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&input).unwrap())
        .unwrap();
    success(&child.wait_with_output().unwrap());
    assert!(path.join("completion.json").exists());
}
