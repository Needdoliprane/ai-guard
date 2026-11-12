//! Synthetic installations, built by product code or by the documented file
//! formats. Every credential value here is synthetic. Edits to a fixture keep
//! file modes unless a test states the contrary.

use super::{
    child::{self, Output},
    tree_manifest,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    ffi::{OsStr, OsString},
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

pub const DEMO_ISSUER: &str = "http://keycloak.ai-guard.test:18080/realms/ai-guard-demo";
pub const HTTPS_ISSUER: &str = "https://login.example.invalid/realms/company";
/// One public certificate, generated once in the QA container with its key
/// written to `/dev/null`. It signs nothing and protects nothing.
pub const PUBLIC_TEST_ROOT: &str = include_str!("fixtures/public-test-root.pem");
/// The documented version 2 manifest with one tenant.
pub const LOCAL_DEMO_MANIFEST: &str = include_str!("../../docs/setup/sso/examples/local-demo.json");

/// One private parent directory for a whole scenario.
pub struct Scenario {
    _guard: tempfile::TempDir,
    pub root: PathBuf,
    /// The children's working directory, inside the root.
    pub work: PathBuf,
}

/// One run of the real `ai-guard-diagnose`.
pub struct Run {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Run {
    /// The JSON document of a `--json` run.
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.stdout).unwrap_or_else(|error| {
            panic!(
                "stdout is not one JSON document ({error}); exit {}; stderr: {}",
                self.code, self.stderr
            )
        })
    }
}

pub fn private_dir(path: &Path) {
    fs::create_dir(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

pub fn write_mode(path: &Path, bytes: impl AsRef<[u8]>, mode: u32) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

pub fn private_write(path: &Path, bytes: impl AsRef<[u8]>) {
    write_mode(path, bytes, 0o600);
}

/// Replace the content of an existing file and keep its mode.
pub fn overwrite(path: &Path, bytes: impl AsRef<[u8]>) {
    let mode = fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777;
    write_mode(path, bytes, mode);
}

/// A synthetic control token: well formed for the gateway, and recognisable.
pub fn synthetic_token(label: &str) -> String {
    format!("fixture-{label}-{}", "0".repeat(40))
}

fn os_args<I, S>(args: I) -> Vec<OsString>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    args.into_iter()
        .map(|arg| arg.as_ref().to_owned())
        .collect()
}

impl Scenario {
    pub fn new() -> Self {
        let guard = super::private_tempdir();
        let root = guard.path().canonicalize().unwrap();
        let work = root.join("work");
        private_dir(&work);
        Self {
            _guard: guard,
            root,
            work,
        }
    }

    pub fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    /// The real diagnostics binary with a cleared environment.
    pub fn diagnose_command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ai-guard-diagnose"));
        command.env_clear();
        command
    }

    /// The real product binary with a cleared environment.
    pub fn product_command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
        command.env_clear();
        command
    }

    /// Run a prepared diagnostics command without the manifest comparison.
    pub fn run_command(&self, command: Command) -> Run {
        let output = child::run(
            command,
            &self.work,
            child::DIAGNOSE_BOUND,
            child::DIAGNOSE_STDOUT_CAP,
            child::SMALL_CAP,
        );
        Run {
            code: output
                .code
                .expect("ai-guard-diagnose ended with an exit code"),
            stdout: output.stdout_text(),
            stderr: output.stderr_text(),
        }
    }

    /// Run the tool without the manifest comparison: for the access-time
    /// oracle, whose own steps take the manifest in a different order.
    pub fn diagnose_unobserved<I, S>(&self, args: I) -> Run
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = self.diagnose_command();
        command.args(os_args(args));
        self.run_command(command)
    }

    /// Run the tool and require that the whole scenario root is unchanged:
    /// every scenario is also a no-mutation test.
    pub fn diagnose<I, S>(&self, args: I) -> Run
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args = os_args(args);
        let before = tree_manifest::take(&self.root);
        let run = self.diagnose_unobserved(&args);
        let after = tree_manifest::take(&self.root);
        tree_manifest::assert_unchanged(&before, &after, &format!("arguments {args:?}"));
        run
    }

    /// The same arguments in text and in JSON, both under the no-mutation
    /// check. Returns the text run, the JSON run and its document.
    pub fn diagnose_both<I, S>(&self, args: I) -> (Run, Run, Value)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args = os_args(args);
        let text = self.diagnose(&args);
        let mut json_args = args;
        json_args.push("--json".into());
        let json = self.diagnose(&json_args);
        assert_eq!(
            text.code, json.code,
            "text and JSON runs must give the same exit code"
        );
        let value = json.json();
        (text, json, value)
    }

    /// Run a real product command to its end.
    pub fn product<I, S>(&self, args: I) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = self.product_command();
        command.args(os_args(args));
        child::run(
            command,
            &self.work,
            child::BUILDER_BOUND,
            child::SMALL_CAP,
            child::SMALL_CAP,
        )
    }

    /// F1: a fresh historical config directory, created by the product.
    pub fn historical(&self, name: &str) -> PathBuf {
        let path = self.path(name);
        agentsandbox::operations::initialize_config(&path).expect("product init");
        path
    }

    /// F1j: a historical directory with a synthetic decision-engine key and
    /// its projection, published by the product.
    pub fn historical_with_decision_engine_key(&self, name: &str) -> PathBuf {
        let path = self.historical(name);
        private_write(
            &path.join("secrets/jev"),
            format!("fixture-jev-distinct-{}", "j".repeat(40)),
        );
        agentsandbox::operations::initialize_config(&path).expect("product init with a key");
        path
    }

    /// F2, F3, F6: a prepared installation, created by the real
    /// `setup prepare` from a manifest written beside it.
    pub fn prepare(&self, name: &str, manifest: &Value) -> PathBuf {
        let output = self.try_prepare(name, manifest);
        assert_eq!(
            output.code,
            Some(0),
            "fixture defect: setup prepare refused the manifest: {}",
            output.stderr_text()
        );
        self.path(name)
    }

    pub fn try_prepare(&self, name: &str, manifest: &Value) -> Output {
        let input = self.path(&format!("{name}.manifest.json"));
        private_write(&input, serde_json::to_vec_pretty(manifest).unwrap());
        self.product([
            OsStr::new("setup"),
            OsStr::new("prepare"),
            OsStr::new("--input"),
            input.as_os_str(),
            OsStr::new("--output-dir"),
            self.path(name).as_os_str(),
        ])
    }

    pub fn resume(&self, directory: &Path) -> Output {
        self.product([
            OsStr::new("setup"),
            OsStr::new("resume"),
            OsStr::new("--recovery-dir"),
            directory.as_os_str(),
            OsStr::new("--confirm-control-stopped"),
        ])
    }

    /// The gateway oracle: the real gateway on a registry, with or without an
    /// OIDC file. It opens and may create stores.
    pub fn gateway(&self, tenants_file: &Path, oidc_config: Option<&Path>) -> child::GatewayStart {
        child::gateway_start(self.gateway_command(tenants_file, oidc_config), &self.work)
    }

    /// The same gateway, left running once it listens: it holds the stores
    /// of the installation open until the returned value is dropped.
    pub fn gateway_running(
        &self,
        tenants_file: &Path,
        oidc_config: Option<&Path>,
    ) -> Result<child::RunningGateway, child::GatewayStart> {
        child::gateway_hold(self.gateway_command(tenants_file, oidc_config), &self.work)
    }

    /// The same oracle for a registry alone, with the event bound the gateway
    /// is started with: its own default when none is given.
    pub fn gateway_with_event_bound(
        &self,
        tenants_file: &Path,
        max_events: Option<&str>,
    ) -> child::GatewayStart {
        let mut command = self.gateway_command(tenants_file, None);
        if let Some(max_events) = max_events {
            command.args(["--max-events", max_events]);
        }
        child::gateway_start(command, &self.work)
    }

    /// The same oracle for a registry alone, with further gateway arguments.
    pub fn gateway_with_arguments(
        &self,
        tenants_file: &Path,
        arguments: &[&str],
    ) -> child::GatewayStart {
        let mut command = self.gateway_command(tenants_file, None);
        command.args(arguments);
        child::gateway_start(command, &self.work)
    }

    fn gateway_command(&self, tenants_file: &Path, oidc_config: Option<&Path>) -> Command {
        let mut command = self.product_command();
        command
            .args(["gateway", "--listen", "127.0.0.1:0", "--policy"])
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("policies/default.json"))
            .arg("--tenants-file")
            .arg(tenants_file);
        if let Some(oidc_config) = oidc_config {
            command.arg("--oidc-config").arg(oidc_config);
        }
        command
    }
}

/// The name `setup prepare` gives to the staging directory of a final path.
/// The derivation is restated here only to build the unpublished state; the
/// real `setup resume` is its oracle.
pub fn staging_name(final_path: &Path) -> String {
    format!(
        ".setup-{}",
        &hex::encode(Sha256::digest(final_path.as_os_str().as_encoded_bytes()))[..24]
    )
}

fn tenant(issuer: &str, id: &str) -> Value {
    json!({
        "tenant_id": id,
        "organization_id": format!("organization-{id}"),
        "identities": [{
            "principal": {"kind": "human_oidc", "issuer": issuer, "subject": format!("stable-{id}")},
            "display_name": format!("Operator of {id}"),
            "capabilities": ["read_metadata"]
        }]
    })
}

fn demo_gateway(issuer: &str) -> Value {
    json!({
        "version": 1,
        "provider_label": "Explicit demo",
        "web_origin": "http://127.0.0.1:18100",
        "provider": {
            "version": 1,
            "issuer": issuer,
            "web_client_id": "web-public",
            "cli_client_id": "cli-public",
            "api_audience": "api-resource",
            "authorization_endpoint": format!("{issuer}/protocol/openid-connect/auth"),
            "token_endpoint": format!("{issuer}/protocol/openid-connect/token"),
            "jwks_uri": format!("{issuer}/protocol/openid-connect/certs"),
            "web_redirect_uri": "http://127.0.0.1:18100/auth/oidc/callback",
            "cli_callback_path": "/oidc/callback",
            "transport": {"kind": "isolated_demo_http", "exact_issuer": issuer}
        },
        "mapping": {"revision": "demo-1", "default_capabilities": ["read_metadata"], "rules": []}
    })
}

fn https_gateway() -> Value {
    json!({
        "version": 1,
        "provider_label": "Company identity",
        "web_origin": "https://guard.example.invalid",
        "provider": {
            "version": 1,
            "issuer": HTTPS_ISSUER,
            "web_client_id": "ai-guard-web-public",
            "cli_client_id": "ai-guard-cli-public",
            "api_audience": "ai-guard-api",
            "authorization_endpoint": format!("{HTTPS_ISSUER}/protocol/openid-connect/auth"),
            "token_endpoint": format!("{HTTPS_ISSUER}/protocol/openid-connect/token"),
            "jwks_uri": format!("{HTTPS_ISSUER}/protocol/openid-connect/certs"),
            "web_redirect_uri": "https://guard.example.invalid/auth/oidc/callback",
            "cli_callback_path": "/oidc/callback",
            "transport": {"kind": "https_only"}
        },
        "mapping": {"revision": "company-policy-1", "default_capabilities": ["read_metadata"], "rules": []}
    })
}

fn policy(issuer: &str, gateway: &Value) -> Value {
    json!({
        "max_records": 1000,
        "max_serialized_bytes": 1_048_576,
        "approved_clients": [
            {"issuer": issuer, "client_id": gateway["provider"]["web_client_id"]},
            {"issuer": issuer, "client_id": gateway["provider"]["cli_client_id"]}
        ]
    })
}

/// A capacity plan with one equal share per tenant, inside the policy above.
pub fn capacity_plan(tenants: &[&str]) -> Value {
    json!({
        "version": 1,
        "unscoped": {"max_records": 20, "max_serialized_bytes": 65_536},
        "tenants": tenants.iter().map(|id| json!({
            "tenant_id": id,
            "capacity": {"max_records": 400, "max_serialized_bytes": 393_216}
        })).collect::<Vec<_>>()
    })
}

/// Manifest version 1 (no capacity plan), demonstration HTTP transport.
pub fn manifest_v1(tenants: &[&str]) -> Value {
    manifest_v1_with_issuer(tenants, DEMO_ISSUER)
}

/// The same manifest for another exact demonstration issuer.
pub fn manifest_v1_with_issuer(tenants: &[&str], issuer: &str) -> Value {
    let gateway = demo_gateway(issuer);
    json!({
        "version": 1,
        "authentication_policy": policy(issuer, &gateway),
        "gateway": gateway,
        "tenants": tenants.iter().map(|id| tenant(issuer, id)).collect::<Vec<_>>()
    })
}

/// Manifest version 2 with a plan for exactly the given tenants.
pub fn manifest_v2(tenants: &[&str]) -> Value {
    let mut manifest = manifest_v1(tenants);
    manifest["version"] = 2.into();
    manifest["capacity_plan"] = capacity_plan(tenants);
    manifest
}

/// Manifest version 2 with the HTTPS-only transport and an optional CA.
pub fn manifest_https(tenants: &[&str], ca_bundle_pem: Option<&str>) -> Value {
    let gateway = https_gateway();
    json!({
        "version": 2,
        "authentication_policy": policy(HTTPS_ISSUER, &gateway),
        "gateway": gateway,
        "capacity_plan": capacity_plan(tenants),
        "ca_bundle_pem": ca_bundle_pem,
        "tenants": tenants.iter().map(|id| tenant(HTTPS_ISSUER, id)).collect::<Vec<_>>()
    })
}

pub fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

/// Rewrite a JSON file in place and keep its mode.
pub fn edit_json(path: &Path, edit: impl FnOnce(&mut Value)) {
    let mut value = read_json(path);
    edit(&mut value);
    overwrite(path, serde_json::to_vec_pretty(&value).unwrap());
}

/// F4: a hand-written registry beside its files. `tenants` are created with
/// relative references, a private directory each and two synthetic tokens;
/// no store exists yet.
pub fn hand_written_registry(directory: &Path, tenants: &[&str]) -> PathBuf {
    let mut entries = Vec::new();
    for id in tenants {
        let tenant_dir = directory.join(id);
        private_dir(&tenant_dir);
        for role in ["runner", "viewer"] {
            private_write(
                &tenant_dir.join(role),
                synthetic_token(&format!("{id}-{role}")),
            );
        }
        entries.push(json!({
            "tenant_id": id,
            "data": format!("{id}/audit.sqlite"),
            "runner_token_file": format!("{id}/runner"),
            "viewer_token_file": format!("{id}/viewer")
        }));
    }
    let registry = directory.join("tenants.json");
    private_write(
        &registry,
        serde_json::to_vec_pretty(&json!({"version": 1, "tenants": entries})).unwrap(),
    );
    registry
}

/// The four synthetic provider credentials of F1p, outside the config
/// directory. Each is well formed for the product's own reader.
pub struct ProviderCredentials {
    pub directory: PathBuf,
    pub codex_api: PathBuf,
    pub claude_api: PathBuf,
    pub codex_subscription: PathBuf,
    pub claude_subscription: PathBuf,
}

pub fn provider_credentials(directory: &Path) -> ProviderCredentials {
    private_dir(directory);
    let credentials = ProviderCredentials {
        directory: directory.to_owned(),
        codex_api: directory.join("codex-api.env"),
        claude_api: directory.join("claude-api.env"),
        codex_subscription: directory.join("codex-auth.json"),
        claude_subscription: directory.join("claude-subscription.env"),
    };
    private_write(
        &credentials.codex_api,
        "OPENAI_API_KEY=synthetic-codex-api-value-0000\n",
    );
    private_write(
        &credentials.claude_api,
        "ANTHROPIC_API_KEY=synthetic-claude-api-value-0000\n",
    );
    private_write(
        &credentials.codex_subscription,
        json!({
            "auth_mode": "chatgpt",
            "tokens": {
                "access_token": "synthetic-codex-access-value-0000",
                "refresh_token": "synthetic-codex-refresh-value-0000",
                "id_token": "synthetic-codex-id-value-0000",
                "account_id": "synthetic-account"
            },
            "last_refresh": "2026-01-01T00:00:00Z"
        })
        .to_string(),
    );
    private_write(
        &credentials.claude_subscription,
        "CLAUDE_CODE_OAUTH_TOKEN=synthetic-claude-subscription-value-0000\n",
    );
    credentials
}

pub fn api_profile(id: &str, client: &str, path: &Path) -> Value {
    json!({
        "id": id, "revision": 1, "client": client, "auth_mode": "api",
        "credential": {"kind": "api_env_file", "path": path}
    })
}

pub fn subscription_profile(id: &str, client: &str, path: &Path) -> Value {
    let kind = if client == "codex" {
        "codex_auth_file"
    } else {
        "claude_oauth_env_file"
    };
    json!({
        "id": id, "revision": 1, "client": client, "auth_mode": "subscription",
        "credential": {"kind": kind, "path": path}
    })
}

/// The four profiles of F1p: Codex CLI and Claude Code, each with a
/// subscription profile and an API profile.
pub fn four_profiles(credentials: &ProviderCredentials) -> Value {
    json!({
        "version": 1,
        "auto_priority": ["codex-subscription", "claude-subscription"],
        "profiles": [
            subscription_profile("codex-subscription", "codex", &credentials.codex_subscription),
            subscription_profile("claude-subscription", "claude_code", &credentials.claude_subscription),
            api_profile("codex-api", "codex", &credentials.codex_api),
            api_profile("claude-api", "claude_code", &credentials.claude_api)
        ]
    })
}

/// Write a provider profile file in its private form.
pub fn write_providers(path: &Path, config: &Value) {
    private_write(path, serde_json::to_vec_pretty(config).unwrap());
}

/// Copy a directory tree with its modes, for an oracle that mutates.
pub fn copy_tree(from: &Path, to: &Path) {
    let metadata = fs::symlink_metadata(from).unwrap();
    if metadata.is_dir() {
        fs::create_dir(to).unwrap();
        for entry in fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            copy_tree(&entry.path(), &to.join(entry.file_name()));
        }
        fs::set_permissions(to, metadata.permissions()).unwrap();
    } else if metadata.is_file() {
        fs::copy(from, to).unwrap();
    }
}

/// A program that only records that it was executed, by writing
/// `<program>.executed` beside itself. It is compiled here because a test
/// must prove that an engine program or a `PATH` entry is never run.
pub fn marker_program(directory: &Path, name: &str) -> PathBuf {
    let source = directory.join(format!("{name}-marker.rs"));
    fs::write(
        &source,
        r#"fn main() {
    let executable = std::env::current_exe().unwrap();
    std::fs::write(executable.with_extension("executed"), "executed").unwrap();
}
"#,
    )
    .unwrap();
    let program = directory.join(name);
    let output = Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
        .args(["--edition=2024", "--crate-name", "marker_fixture"])
        .arg(&source)
        .arg("-o")
        .arg(&program)
        .output()
        .expect("fixture defect: the Rust compiler is unavailable to build the marker program");
    assert!(
        output.status.success(),
        "fixture defect: the marker program did not compile: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::remove_file(&source).unwrap();
    program
}

pub fn marker_was_executed(program: &Path) -> bool {
    program.with_extension("executed").exists()
}
