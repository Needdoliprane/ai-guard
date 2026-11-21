# ai-guard

[English](README.md) · [Français](README.fr.md)

ai-guard is an open source, local-first control plane for running Codex, Claude Code and other commands in isolated project copies. Request policy authorization, follow activity in one dashboard, inspect changes and export results to a new destination.

The trusted Rust CLI and execution broker control Docker/Podman workers. A separate Rust gateway serves the API and audit; a containerized Rust dashboard serves the static TypeScript interface. Each tenant has its own authenticated scope and SQLite database. Workers receive neither the container-engine socket nor the original project mount.

Local use needs no ai-guard cloud account. You build and distribute images from the supplied Dockerfiles; no public prebuilt ai-guard image is required. The [manual index](docs/setup/README.md) covers the complete product contract; [implementation status and evidence](docs/engineering/CURRENT_STATE.md) identify what is delivered, qualified or still missing.

## Start locally

You need Git, the Rust toolchain selected by [rust-toolchain.toml](rust-toolchain.toml), a running Docker engine with Compose, and a project directory to copy. Rootless is required by default. For a deliberate Docker Desktop or other rootful installation, add `--allow-rootful-runtime` to `doctor`, `build`, `up` and every `run`; this records a security downgrade. For Podman, follow the [local guide](docs/setup/local/README.md) with a rootless engine and a compatible Compose provider.

Run from the source distribution's root. Replace `AIGUARD_PROJECT` with an existing absolute path. Keep the private configuration outside that project and this checkout. Prepare build dependencies and images before disconnecting an offline installation.

```sh
cargo build --release --locked
AIGUARD_BIN="$PWD/target/release/agentsandbox"
AIGUARD_DIAGNOSE="$PWD/target/release/ai-guard-diagnose"
AIGUARD_CONFIG="$HOME/.ai-guard"
AIGUARD_PROJECT="/absolute/path/to/your/existing/project"
AIGUARD_GATEWAY="http://127.0.0.1:8787"
"$AIGUARD_BIN" init --config-dir "$AIGUARD_CONFIG"
"$AIGUARD_BIN" doctor --runtime docker
"$AIGUARD_BIN" build --runtime docker --source .
```

`init` creates technical runner/viewer credentials and preserves existing ones. Their private source files stay under `secrets/`; container projections are prepared separately. Keep the configuration directory, credentials and data private. A directory created by SSO `setup prepare` has a different layout: use its [SSO guide](docs/setup/sso/README.md) rather than initializing it with this flow.

`build`, `up` and `down` prepare bounded private copies of their selected source inputs before handing them to the engine. Retain the printed source digest and actual command result. The [prepared-source procedure](docs/setup/local/README.md#prepared-source) explains the fixed input set, temporary-directory requirements, refusals and retained copies after an unknown termination. This preparation does not establish reproducible images or runtime qualification.

Before starting services, inspect the configuration:

```sh
"$AIGUARD_DIAGNOSE" --config-dir "$AIGUARD_CONFIG"
```

The [diagnostic](docs/setup/diagnostics/README.md) is read-only and offline by default: it launches nothing, reads credential references through metadata and reports what was not checked or is unavailable. It does not validate credential contents, account access, SQLite integrity or signed checkpoints. Resolve errors and unknowns and read the limits even when the report says `OK`. Add `--json` for the structured report.

```sh
"$AIGUARD_BIN" up --runtime docker --source . --config-dir "$AIGUARD_CONFIG"
"$AIGUARD_BIN" healthcheck --url "$AIGUARD_GATEWAY/healthz" --url http://127.0.0.1:4000/healthz
```

Open `http://127.0.0.1:4000`. For this technical viewer flow, obtain the viewer value from `$AIGUARD_CONFIG/secrets/viewer` in a private editor and enter it only in the trusted connection form. Keep the runner credential in trusted control. Named personal access has its own [local identity](docs/setup/identity/README.md) or [SSO](docs/setup/sso/README.md) procedure.

This CLI stack uses the fixed project name `agentsandbox` and the default loopback ports above. Keep the same engine account/context, configuration directory and source for its lifecycle. For custom ports, project names, Compose env files and logs, use the [local guide](docs/setup/local/README.md#cli-up-stack); do not mix commands for a different stack. A health response establishes service reachability only.

## Run, follow and inspect a session

Start with a small command that needs no provider account and has no worker network access. The trusted CLI uses its technical runner credential to request policy authorization before execution:

```sh
"$AIGUARD_BIN" run --project "$AIGUARD_PROJECT" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY" --runtime docker --profile strict --network none --timeout 30 -- /usr/bin/id
```

Keep the returned session UUID. In the dashboard, inspect its decision, runtime observations and file-change coverage. Admission, an observed start and completion are separate facts; a refused or unknown result needs diagnosis.

```sh
AIGUARD_SESSION="replace-with-the-returned-session-uuid"
"$AIGUARD_BIN" worker-status --session "$AIGUARD_SESSION" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY"
"$AIGUARD_BIN" verify-audit --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY"
```

`verify-audit` asks the gateway to check its local hash chain. [Signed checkpoints and independently anchored rollback checks](docs/setup/audit-signatures/README.md) are separate operations. For a deliberate file change in the copied workspace, follow the [local session examples](docs/setup/local/README.md#execute-a-first-command).

For an interrupted monitor, preserve the session and its worker volume, keep the control services available and reconcile that recorded session:

```sh
"$AIGUARD_BIN" worker-resume --session "$AIGUARD_SESSION" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY" --wait-seconds 30
```

Resume does not grant a second use of a consumed start. Follow the [recovery procedure](docs/setup/local/README.md#reconcile-the-recorded-worker) when the result remains unknown; do not create another task to hide it.

Export the selected session only to a **new** destination outside the original project, with an existing parent:

```sh
AIGUARD_EXPORT="$HOME/ai-guard-export-new"
"$AIGUARD_BIN" export-workspace --session "$AIGUARD_SESSION" --output "$AIGUARD_EXPORT" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY"
```

Review the exported files and diff before applying changes yourself. Exported content remains untrusted; inspect it before opening tools that execute project configuration. ai-guard never rewrites the original project automatically.

Before shutting down the control stack, request a stop for each active session and check its recorded runtime state:

```sh
"$AIGUARD_BIN" worker-stop --session "$AIGUARD_SESSION" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY"
"$AIGUARD_BIN" worker-status --session "$AIGUARD_SESSION" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY"
"$AIGUARD_BIN" down --runtime docker --source . --config-dir "$AIGUARD_CONFIG"
```

A durable stop request alone is not proof that a worker stopped. Preserve unknown states for reconciliation. `down` retains named audit data; keep the configuration, session plans and worker volumes together. Restart with the same `up` command, then repeat health and session checks. Follow [operations and recovery](docs/setup/local/README.md#stop-preserve-data-and-restart) before backups or migrations; do not delete volumes to repair an error.

## Choose identity, AI and deployment

Use the [configuration index](docs/setup/README.md) for file consumers, public examples and combinations. Identity, provider authentication, decision engine and network permissions are independent choices.

| Need | Guide |
|---|---|
| Docker/Podman, custom configuration, first session and recovery | [Local workstation](docs/setup/local/README.md) |
| Diagnose a configuration before launch | [Operator diagnostics](docs/setup/diagnostics/README.md) |
| Codex / Claude Code, subscription or API | [AI providers](docs/setup/providers/README.md) |
| Named local accounts without an IdP | [Local identity](docs/setup/identity/README.md) |
| ai-guard Web/CLI SSO and an opt-in local Keycloak DEMO | [SSO setup](docs/setup/sso/README.md) · [Keycloak provider](docs/setup/keycloak/README.md) |
| Small local decision model and verified TLS | [Local AI](docs/setup/local-ai/README.md) · [TLS](docs/setup/local-ai/TLS.md) |
| Managed tenants, private CAs, internal inference and cloud requirements | [Enterprise configuration](docs/setup/enterprise/README.md) |
| Kubernetes authority and read-only inspection | [Kubernetes](docs/setup/kubernetes/README.md); worker launch remains unavailable |

For an AI task, build the pinned client versions and prepare only the chosen provider credential through the provider guide. Select `--client`, `--auth-mode` and `--client-profile` deliberately. A subscription request never silently becomes a billed API request. Provider sign-in is separate from ai-guard sign-in; an IdP administrator has no implicit ai-guard permission. Internal coding-model integration and cloud topology have their own delivery limits in the enterprise guide.

Examples contain public values only. Create a **new** private configuration file when adapting one, preserve existing credentials and data, and reference secrets by private path. Compose reads env examples through `--env-file`; the Rust CLI does not source dotenv. Never put a credential in argv, Git, screenshots or an exported workspace. Resource allocation records are not CPU-consumption invoices; model advice cannot override mandatory policy, audit or runtime restrictions.

[Operations](docs/operations.md) · [API](docs/api.md) · [Security](docs/security.md) · [Architecture](docs/architecture.md) · [License](LICENSE)

## Contribute

Application code outside the frontend is Rust. See [AGENTS.md](AGENTS.md) for architecture, coding rules and targeted verification. Product behavior changes must update the English/French guides and configuration examples in the same change. Auxiliary Keycloak, Qwen and test-CA experiments stay local and ignored; product security and runtime regressions remain in the repository.

Apache-2.0. Vulnerabilities are reported privately through GitHub Security Advisories; see [SECURITY.md](SECURITY.md) ([FR](SECURITY.fr.md)).

## Additional guides

- [Kubernetes V2 runtime: software mechanics, closed product effects and real-cluster qualification plan](docs/setup/kubernetes-runtime/README.md).
- [Release metadata: offline manifest/verify, declared provenance and qualification limits](docs/setup/release/README.md).
- [Install from a GitHub Release](docs/setup/release/install.md).
