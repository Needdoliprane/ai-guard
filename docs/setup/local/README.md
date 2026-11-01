# Local workstation

[Français](README.fr.md) · [Setup index](../README.md) · [Trusted broker](../broker/README.md) · [AI providers](../providers/README.md)

Use this guide to run the control services in Docker or Podman, execute a first command in a worker, inspect its audit and export its changes. The trusted Rust CLI controls the engine on your workstation. The gateway and dashboard run in separate containers; workers receive a filtered copy of the project, never the original project mount.

## Prepare the workstation

You need Git, the Rust toolchain selected by `rust-toolchain.toml`, and either Docker with Compose or rootless Podman with a Compose provider. Building images requires access to their pinned dependencies; prepare images before disconnecting an offline installation. A local image build is supported; no public ai-guard image is required.

Run the following from the repository root. Replace the project path with a real, existing directory. Keep configuration outside that project and outside this checkout.

```sh
cargo build --release --locked
AIGUARD_BIN="$PWD/target/release/agentsandbox"
AIGUARD_CONFIG="$HOME/.ai-guard"
AIGUARD_PROJECT="/absolute/path/to/your/project"
AIGUARD_GATEWAY="http://127.0.0.1:8787"
AIGUARD_DASHBOARD="http://127.0.0.1:4000"
"$AIGUARD_BIN" init --config-dir "$AIGUARD_CONFIG"
```

`init` creates distinct runner and viewer credentials and preserves existing credentials. The configuration directory must be private (`0700`), with private source credentials (`0600`). It also prepares the read-only projections used by containers; do not loosen the source files to make a container read them. Keep configuration, databases, session plans and credentials out of Git, screenshots and exported workspaces.

Choose one engine for this installation:

```sh
"$AIGUARD_BIN" doctor --runtime docker
"$AIGUARD_BIN" build --runtime docker --source .
```

For Podman, use:

```sh
"$AIGUARD_BIN" doctor --runtime podman
"$AIGUARD_BIN" build --runtime podman --source .
```

The normal requirement is a rootless engine. If you deliberately use Docker Desktop or another daemon that does not report rootless operation, add `--allow-rootful-runtime` to `doctor`, `build` and each `run`. This explicit exception is recorded as a runtime security downgrade; a VM does not establish equivalence to rootless isolation. Do not grant the application containers an engine socket or privileged mode.

The default worker image supports ordinary commands. For Codex or Claude Code, build the client versions described in the [provider guide](../providers/README.md) before starting an AI session.

## Create the stack configuration

The [local example](../../../.env.exemple.local) controls Compose interpolation. Create a **new** private copy; the subshell below refuses to overwrite an existing file.

```sh
(
  umask 077
  set -C
  cat .env.exemple.local > "$AIGUARD_CONFIG/local.env"
)
```

Open that new file in your editor. Keep `AGENTSANDBOX_CONFIG_DIR` identical to the directory passed to `init`. `COMPOSE_PROJECT_NAME` identifies this stack and its named audit volume. `AI_GUARD_CONTROL_IMAGE` selects the control image; the build above creates `agentsandbox-control:local`. A different image name requires an image you have built or obtained through your own trusted distribution.

`AI_GUARD_GATEWAY_PORT` and `AI_GUARD_DASHBOARD_PORT` change the host ports, still bound to `127.0.0.1`. Container ports remain `8787` and `4000`. If you change host ports, update `AIGUARD_GATEWAY` and `AIGUARD_DASHBOARD` in your shell too. The Rust CLI does not read this dotenv file. Never `source` it; pass it explicitly to Compose. Shell environment values can override Compose interpolation, so check for an inherited setting when a value appears wrong.

Start Docker services:

```sh
docker compose --env-file "$AIGUARD_CONFIG/local.env" -f deploy/docker/compose.yaml up -d
docker compose --env-file "$AIGUARD_CONFIG/local.env" -f deploy/docker/compose.yaml ps
"$AIGUARD_BIN" healthcheck --url "$AIGUARD_GATEWAY/healthz" --url "$AIGUARD_DASHBOARD/healthz"
```

Or start the Podman variant:

```sh
podman compose --env-file "$AIGUARD_CONFIG/local.env" -f deploy/podman/compose.yaml up -d
podman compose --env-file "$AIGUARD_CONFIG/local.env" -f deploy/podman/compose.yaml ps
"$AIGUARD_BIN" healthcheck --url "$AIGUARD_GATEWAY/healthz" --url "$AIGUARD_DASHBOARD/healthz"
```

Use the same engine account, context/endpoint, env file and Compose file throughout. Keep to this direct Compose workflow when customizing ports or project names; the CLI `up/down` workflow has its own fixed project and default readiness URLs.

## Open the control view

Open `AIGUARD_DASHBOARD` in your browser. The gateway URL is for CLI/API access; `/healthz` reports service availability, not permission to run a worker or proof that an AI provider is authenticated.

Use your named ai-guard account for a personal view and explicitly switch to an administrative scope when authorized. Local identity works without an IdP. [Keycloak/OIDC](../keycloak/README.md) is a separate identity choice; it does not select your AI provider. Personal views include your own sessions and explicitly shared resources. Tenant membership alone must not reveal another user's prompts, output or files.

Keep technical credentials separate from that identity: `viewer` grants its configured read scope and `runner` belongs to trusted control. For a viewer-key connection, obtain the viewer value from the private source file in a private editor and enter it only in the trusted connection form. Never paste the runner credential into a browser, worker or chat. Older events identified only by a shared key remain shared-key history; a later login does not establish their human author.

The dashboard presents session provenance, decisions, runtime observations and changes. A stale connection, partial coverage, denied request and empty result are different states. Use the displayed last observation and scope when deciding whether to refresh, reconnect or request access.

A 401/403 refusal on an active read closes the viewer session and removes its data, including open details. Reconnect with an authorized credential; repeated refreshes cannot restore a refused credential. A service error is shown explicitly. In the worker, quota and allocation views, the last successful data remains marked stale within the same session and scope until a successful refresh.

## Execute a first command

Start with a small offline command. It uses the strict policy and disables worker networking:

```sh
"$AIGUARD_BIN" run --project "$AIGUARD_PROJECT" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY" --runtime docker --profile strict --network none --timeout 30 -- /usr/bin/id
```

For Podman, replace `--runtime docker` with `--runtime podman` in every `run`. Record the returned session UUID. Inspect its decision and observed completion in the dashboard, then read the registry and verify the local audit chain:

```sh
AIGUARD_SESSION="replace-with-the-returned-session-uuid"
"$AIGUARD_BIN" worker-status --session "$AIGUARD_SESSION" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY"
"$AIGUARD_BIN" verify-audit --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY"
```

`verify-audit` checks the gateway's local hash chain. Keep that result distinct from independent signed checkpoints and external anchoring. A successful admission is also distinct from a runtime start or a completed command.

To make a deliberate change only in the copied workspace:

```sh
"$AIGUARD_BIN" run --project "$AIGUARD_PROJECT" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY" --runtime docker --profile observe --network none --timeout 30 -- /usr/bin/touch /workspace/ai-guard-example.txt
```

This is a separate session with a fresh project copy. `observe` does not mean that every transitive effect is mediated or understood. Select `review` when a request requires human approval: inspect the exact request and use its full approval hash with `--approve-hash` when resubmitting that request. Changed inputs need a new decision; never approve an unrelated hash to get past a refusal.

## Inspect and export changes

Select the session that created the file, then use its UUID. The export parent must exist and the destination must be **new**, outside the original project. Do not use an existing working tree as the export destination.

```sh
AIGUARD_SESSION="replace-with-the-file-session-uuid"
AIGUARD_EXPORT="$HOME/ai-guard-export-new"
"$AIGUARD_BIN" export-workspace --session "$AIGUARD_SESSION" --output "$AIGUARD_EXPORT" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY"
```

Review the exported files and diff before applying selected changes to your project. ai-guard does not rewrite the original automatically. A failed export is not an empty diff; if a destination was published before a later error, inspect and preserve it before choosing a different new destination.

The copy excludes known sensitive names and dependency/cache directories, but it is not a universal secret scanner. Review the source project's contents before import. Keep keys outside it. Bounds include 20,000 entries, 128 MiB total and 32 MiB per file; links and unsafe paths are rejected. A bound violation requires reducing the input or changing an approved limit, not silently treating a partial copy as complete.

## Diagnose a refusal or interruption

| Symptom | Check and next action |
|---|---|
| Engine missing, unsafe executable or wrong endpoint | Run `doctor` under the same account and repeat explicit `--runtime-program`/`--runtime-endpoint` choices on the commands that accept them. Do not fall back to running the task on the host. |
| Non-rootless daemon refused | Configure rootless operation or deliberately apply the per-command exception described above. |
| Stack unavailable | Check Compose `ps` and bounded `logs --tail 100 gateway dashboard ingress` with the same configuration. Check host-port collisions and private credential projections. Never publish raw logs without reviewing them for secrets. |
| Unauthenticated or forbidden | Reconnect with the appropriate identity and authorized tenant/scope. Do not substitute a runner key for a missing user permission. |
| Policy deny or review | Read the decision reason and requested scope. Adjust the request or obtain approval for that exact request. |
| Project copy or export refused | Check size, links, destination freshness and directory permissions. Preserve the original and any already published export. |
| Runtime unavailable or status unknown | Preserve the session, plans and volume. Query status and reconcile the recorded runtime authority; do not create another start to hide uncertainty. |
| Provider login or client unavailable | Follow the [provider diagnosis](../providers/README.md#diagnose-provider-problems); health checks do not validate provider access. |

The normal recovery commands use the session's recorded engine authority:

```sh
"$AIGUARD_BIN" worker-status --session "$AIGUARD_SESSION" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY"
"$AIGUARD_BIN" worker-resume --session "$AIGUARD_SESSION" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY" --wait-seconds 30
```

Resume reconciles an interrupted session; it does not grant a second use of a consumed start claim. Interactive admission and terminal attachment have their own continuity requirements. When a retained authority needs an explicit executable, use the approved `--runtime-program` path; `--adopt-runtime-program` is a deliberate legacy-authority binding, not a generic retry switch.

## Stop, preserve data and restart

Request a stop for each active worker before stopping its control stack:

```sh
"$AIGUARD_BIN" worker-stop --session "$AIGUARD_SESSION" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY"
"$AIGUARD_BIN" worker-status --session "$AIGUARD_SESSION" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY"
docker compose --env-file "$AIGUARD_CONFIG/local.env" -f deploy/docker/compose.yaml stop
```

A durable stop request is not proof of terminal runtime state. Confirm the observation or retain the unknown state for reconciliation. For Podman, use its matching Compose command and file. Restart the preserved stack with the same `up -d` command, then run health and session checks again.

`stop` preserves containers and named volumes. `down` without `-v` removes stack containers/networks while retaining named audit data; neither operation substitutes for stopping workers. Do not use `down -v`, `prune`, a new project name or database deletion to repair an error. Preserve `<config-dir>` (credentials, plans, bindings and session state), the stack's audit volume and retained worker volumes together. Back up SQLite coherently while stopped, including relevant WAL state; do not copy a live database as an ordinary file. Review [enterprise operations](../enterprise/README.md#operate-and-recover) before migration or restoration.
