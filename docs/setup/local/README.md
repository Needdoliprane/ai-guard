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

`build --source` selects the ai-guard source distribution, not the project an agent will edit. Before contacting the engine, the CLI checks the required Dockerfiles, `.dockerignore`, manifests/locks, Rust/frontend entry files and policy, including their parent directories. A missing, linked or incorrectly typed required entry is reported by its relative path. Restore that entry in the selected source distribution and retry; this refusal does not require deleting images, caches or data. An explicitly selected source root is still canonicalized normally.

This is a structural prerequisite check. It does not validate every import, lockfile content or `.dockerignore` rule, freeze concurrent source changes, or make the two image builds atomic. The first image can still build successfully before a later build failure. Keep the source stable while building and retain the actual engine results; completing the check is not a reproducible-build or production qualification.

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

`AI_GUARD_GATEWAY_PORT` and `AI_GUARD_DASHBOARD_PORT` change the host ports, still bound to `127.0.0.1`. Published container ports remain `8787` and `4000`; control port `8788` stays internal as described below. If you change host ports, update `AIGUARD_GATEWAY` and `AIGUARD_DASHBOARD` in your shell too. The Rust CLI does not read this dotenv file. Never `source` it; pass it explicitly to Compose. Shell environment values can override Compose interpolation, so check for an inherited setting when a value appears wrong.

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

### HTTP ingress limits

The public Rust ingress has independent budgets for **16 request bodies being read** and **64 upstream exchanges**. Each body is limited to **256 KiB**, **5 seconds in total** and **2 seconds without a new chunk**, starting when body reading begins. The complete body is read and its upload permit released before an exchange is acquired. A body known to be empty from its stream state bypasses the upload budget; the HTTP method or declared `Content-Length` does not decide this. Public routes retain their fixed destinations and upstream authentication/authorization. These bounds are fixed in the binary; the dotenv file and CLI expose no setting for them.

A full upload budget can return **429** for a request with a body, while bodyless control reads can still use the separate exchange budget. Complete requests and SSE streams can still fill that exchange budget. This separation does not provide general flood protection or qualify runtime isolation. The runner still requests a targeted worker stop on a gateway tracking error, subject to engine availability; no 60-second retry window is enabled. Preserve the recorded session and reconcile it after an interruption rather than launching another instance to hide an unknown result.

### Explicit control transport: server component

The updated Compose files configure `gateway --control-listen 0.0.0.0:8788` and `ingress --control-upstream http://gateway:8788`. The gateway flag is optional; the ingress flag is required and has no public-upstream fallback. Port 8788 stays on the existing internal network, with no new host port or credential owned by the relay. The base Docker/Podman files, their SSO profiles and both Docker local-model TLS overrides retain this connection. Preserve the existing data, credentials and host loopback bindings when adapting a deployment.

An explicitly selected `/control/` request uses the dedicated destination and bypasses the public 16/64 pools. The gateway authenticates the existing identity before reading the body or reserving one of the tenant's **16 runner** or **4 broker-presence** operation slots. The [concrete server contract](../../engineering/INGRESS_CONTROL_TRANSPORT.md) lists the closed routes, body/response bounds and errors. Keep `AIGUARD_GATEWAY`, `--gateway` and broker registry values as origins without a path; do not append `/control` or point workstation clients at the container-only port 8788.

This is the server connection. SDK, runner and broker consumers still need their explicit transport selection composed and verified; the commands below do not establish that migration. Declared capture remains public, and a mixed broker outbox cannot choose authority from event kind/source. `control_operation_capacity_exhausted` is distinct from public-pool 429 and audit quotas. A closed `control_transport_failure` response reports an **unknown** result, not rollback. Preserve the operation identity and reconcile its existing receipt. Neither this transport nor an arbitrary 502/503 authorizes automatic retries of a mutation, start, claim or mandatory audit.

## Open the control view

Open `AIGUARD_DASHBOARD` in your browser. The gateway URL is for CLI/API access; `/healthz` reports service availability, not permission to run a worker or proof that an AI provider is authenticated.

The `init` and base Compose commands above prepare technical runner/viewer access, not a named account. For a personal view without an IdP, follow [named local access](../identity/README.md): migrate the existing tenant store explicitly, provision an account and grant its real resources before signing in. Select an offered administrative context only when authorized. Personal resources require an explicit owner or share grant; tenant membership and a matching session name give no access or authorship. [Keycloak/OIDC](../sso/README.md) is a separate identity choice and does not select your AI provider.

Keep technical credentials separate from that identity: `viewer` grants its configured read scope and `runner` belongs to trusted control. For a viewer-key connection, obtain the viewer value from the private source file in a private editor and enter it only in the trusted connection form. Never paste the runner credential into a browser, worker or chat. Older events identified only by a shared key remain shared-key history; a later login does not establish their human author.

The dashboard presents session provenance, decisions, runtime observations and changes. A stale connection, partial coverage, denied request and empty result are different states. Use the displayed last observation and scope when deciding whether to refresh, reconnect or request access.

Open an agent or sandbox to inspect its explicitly linked workers, then open a worker to return to its related agents and sandboxes. Links use exact session or runtime identifiers from the authenticated tenant's data; their provenance is displayed, and a matching session does not prove execution or delegation. The worker registry is paginated: navigate its pages before concluding that no matching record was received. A failed read retains a stale result with a retry action; it is not an empty inventory. The links combine audit projections and registry records, rather than querying an engine or discovering remote brokers.

In the technical viewer's worker table and details, **Autorité déclarée du lancement** identifies the declared launch authority; **Autorité de la dernière inspection enregistrée** identifies the last recorded inspection's authority, alongside its runtime and observation date. An absent or null authority remains unknown, with no fallback from the launch to an inspection. Supplied IDs must be nonempty strings, preserved exactly and limited to 256 UTF-8 bytes without control characters; invalid values produce a contract error. These fields establish neither an associated host or broker, current availability nor a complete inventory. See the [worker view contract](../../ui.md#workers-quotas-et-allocations).

A 401/403 refusal on an active read closes the viewer session and removes its data, including open details. Reconnect with an authorized credential; repeated refreshes cannot restore a refused credential. A service error is shown explicitly. In the worker, quota and allocation views, the last successful data remains marked stale within the same session and scope until a successful refresh.

## Execute a first command

The commands below use the technical runner credential retained in `AIGUARD_CONFIG`, independently of a human dashboard login. They do not create authenticated human initiation or delegation; the [human-to-broker execution contract](../../engineering/HUMAN_EXECUTION.md) remains proposed. Start with a small offline command using the strict policy and disabled worker networking:

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

Exports omit known sensitive/cache names excluded on import and tool metadata including `.husky`, `.envrc`, `.idea`, `.vscode`, `.devcontainer`, `.direnv`, `.pre-commit-config.yaml`, `.gitattributes`, `.gitmodules`, `.mise.toml` and `.tool-versions`. Matching includes case variants and specific Unicode aliases of reserved names; ordinary Unicode filenames remain available. A Git-directory-shaped tree (`HEAD` with `objects`/`refs`, or `HEAD` with `commondir`) is refused before publication, including nested directories and implicit parents. Worker-authored `.git` configuration and hooks are not restored. Regular files are written as `0644`, including files the worker marked executable. Ignored entries still undergo path, type, duplicate and size validation: filtering does not make an invalid archive acceptable.

The export manifest records published files and modes; the workspace scan records the original observed modes. New observations can therefore distinguish a permissions-only change from a content change. Historical manifests without a mode retain unknown mode coverage; unchanged content does not prove unchanged permissions. `workspace.export.completed` reports `omitted_entries`, counting explicit archive entries (files and directories), not implicit parent directories. A historical event without this field does not report zero omissions.

The control emits a generic warning with the omission count, without exposing names. If this warning cannot be written, publication is refused. **Exported content remains untrusted.** Some tool files, including `.cargo/config.toml`, `Makefile` and `package.json`, remain present. The policy does not neutralize every tool's configuration or detect arbitrary secrets. Inspect files as text before opening the project in a tool that can run configuration, installing dependencies or deliberately restoring executable permissions.

The copy excludes known sensitive names and dependency/cache directories, but it is not a universal secret scanner. Review the source project's contents before import. Keep keys outside it. Bounds include 20,000 entries, 128 MiB total and 32 MiB per file; links and unsafe paths are rejected. A bound violation requires reducing the input or changing an approved limit, not silently treating a partial copy as complete.

## Diagnose a refusal or interruption

| Symptom | Check and next action |
|---|---|
| Engine missing, unsafe executable or wrong endpoint | Run `doctor` under the same account and repeat explicit `--runtime-program`/`--runtime-endpoint` choices on the commands that accept them. Do not fall back to running the task on the host. |
| Non-rootless daemon refused | Configure rootless operation or deliberately apply the per-command exception described above. |
| Stack unavailable | Check Compose `ps` and bounded `logs --tail 100 gateway dashboard ingress` with the same configuration. Check host-port collisions and private credential projections. Never publish raw logs without reviewing them for secrets. |
| HTTP 429 through ingress | Distinguish the [upload/exchange budgets](#http-ingress-limits) from upstream quotas using the returned message. Reduce parallel requests and retry a read after capacity is available. Check the recorded session before repeating a launch. |
| HTTP 408 or 413 while sending a body | Check the 5-second total/2-second idle deadlines and 256-KiB body limit. Reduce the payload or fix a stalled sender; changing the dotenv file does not extend these bounds. |
| HTTP 503 with audit descriptor exhaustion (`EMFILE`/`ENFILE`) | Preserve the session and operation identity. Once descriptors are available, retry an authorized read that reaches Store validation; see [audit descriptor recovery](#audit-descriptor-recovery) before replaying a mutation. |
| Unauthenticated or forbidden | Reconnect with the appropriate identity and authorized tenant/scope. Do not substitute a runner key for a missing user permission. |
| Policy deny or review | Read the decision reason and requested scope. Adjust the request or obtain approval for that exact request. |
| Project copy or export refused | Check size, links, destination freshness and directory permissions. Preserve the original and any already published export. |
| Runtime unavailable or status unknown | Preserve the session, plans and volume. Query status and reconcile the recorded runtime authority; do not create another start to hide uncertainty. |
| Provider login or client unavailable | Follow the [provider diagnosis](../providers/README.md#diagnose-provider-problems); health checks do not validate provider access. |

### Audit descriptor recovery

When backing-file validation retains a typed OS error `EMFILE` or `ENFILE`, the Store refuses the operation without a successful acknowledgement or execution permission for it. Gateway routes receiving this error map it to HTTP **503**; descriptor exhaustion may also prevent accepting a connection or transmitting the response. The live supervision origin and sibling connections sharing it remain suspended. Once descriptors are available, the next authorized access reaching Store validation automatically checks a complete, consistent SQLite snapshot: file identity, schema, tenant binding, metadata/quota, event chain, control projections and the live rollback witness. Only successful validation restores that shared origin; a sibling connection can perform the recovery. `/healthz` alone does not trigger it.

SQLite may already have committed before the final file check fails. Preserve the event or operation identity and its exact content. Use that operation's existing idempotent replay path where available: the durable result is reused without another event or quota debit. A new identity or session is a new operation, and a consumed start claim stays consumed. Do not infer that an error response means nothing was committed.

`EIO`, substituted file identity, corruption and a poisoned supervision lock keep the origin permanently invalidated. Apparent file repair, refreshes and `worker-resume` cannot reactivate it; there is no operator override. Preserve data and session references for diagnosis. Descriptor recovery, HTTP status, health checks and a valid audit chain do not prove engine availability or a worker's stop. Continue to use the recorded runtime observations; consult the [dated validation evidence](../../engineering/VALIDATION.md) for the scope actually qualified.

### Reconcile the recorded worker

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
