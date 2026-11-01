# Trusted local broker

[Français](README.fr.md) · [Setup index](../README.md) · [Local installation](../local/README.md) · [AI providers](../providers/README.md)

The permanent Rust broker owns batch jobs in the trusted control environment. A client submits a request through private local IPC; leaving that client does not cancel an accepted job. The gateway, dashboard and workers stay separate. Neither a worker nor the dashboard receives the engine socket, the broker registry or a runner credential.

This manual describes the complete product contract and the commands implemented by the current CLI. It is not a qualification report. Read [current implementation and gaps](../../engineering/CURRENT_STATE.md), [validation](../../engineering/VALIDATION.md) and [production gates](../../engineering/PRODUCTION_READINESS.md) before choosing an installation. In particular, the active-control prerequisite below prevents treating the ordinary Compose startup as a complete permanent-broker installation.

## Prepare the trusted environment

Use a Unix control account with an already built `agentsandbox` binary, a deliberately selected Docker or Podman installation, prepared worker images and access to the matching gateway. Follow the [local guide](../local/README.md) for building images and private credentials. Docker and Podman are distinct runtimes; a result on one does not qualify the other.

The broker runs on the trusted workstation or platform control host. The gateway and dashboard remain containerized. Do not install the broker inside an application worker, mount the source project into a worker, or grant application containers access to the engine. The broker executes the approved engine program with separate arguments; business commands run in the copied worker workspace.

Before starting the service, prepare these installation values:

| Value | Requirement |
|---|---|
| Binary and runtime program | Absolute paths to the reviewed binary and approved engine executable. Runtime executable identity, content and directory permissions are checked; an ambient `PATH` is not an approval. |
| Registry | A new private regular JSON file, `0600`, in a private `0700` directory owned by the control account. No links or unsupported ACLs. Maximum 256 KiB. |
| Broker `root` and `config_dir` | Dedicated bounded absolute paths outside the project. Preserve existing state. Their private directories and retained identities are checked. |
| Gateway origin | The real authorized origin: HTTPS, or HTTP loopback for a deliberate local installation, without credentials, path, query or fragment. |
| Tenant and namespace | The actual `tenant_id` and `control_namespace_uuid` returned by the authenticated control, as described below. Registry strings are expectations, not identity grants. |
| Runner files | Existing private `0600` credential sources with private parents. Reference their paths; never print their contents or put them in command arguments. |
| Runtime endpoint | An explicit absolute local Unix socket URL for Docker, such as `unix:///absolute/engine/docker.sock`. Podman also accepts its explicit `local` endpoint. Use the account/context intended for the jobs. |
| Source and export roots | Existing allowed directories with safe ownership, permissions and ancestry. Source and export trees must be disjoint. Export roots must also be physically separate from broker/configuration storage. Avoid broad roots containing control data. |

Rootless operation is the normal requirement. `allow_rootful_runtime: true` is a deliberate exception for an installation that requires it, including Docker Desktop configurations that do not report rootless operation. It is not an isolation equivalence. Keep `network: "none"` unless a separately reviewed job profile requires outbound access.

## Require an active compatible control

Every request authenticates its **presented** credential afresh against `GET /api/v1/control/protocol`. The broker requires `role: "runner"`, the configured tenant and namespace, `schema_version: 3` and the `broker_supervision_v1` protocol. Submit, approve, resume and export also require `active: true`. Status, list, recovery-status and cancel can authenticate against an inactive compatible control; their other dependencies and safety checks still apply. Being the same Unix UID is transport access, not tenant authentication.

Obtain the real tenant and namespace through an authenticated control integration operated in the trusted environment. Do not guess the namespace, copy a test UUID, choose another tenant in the registry, or paste a runner into the dashboard. The configured `runner_token_file` serves the broker's autonomous reconciliation; it does not replace authentication of a client request. The client `--token-file` identifies the credential presented for that request.

The standard local Compose files start schema2 control. `/healthz` only establishes service availability. Neither a healthy stack nor `init` establishes the schema3 protocol or fresh execution authority. The existing [exclusive bootstrap for new supervised control stores](../../operations.md#initialiser-un-contrôle-de-supervision-neuf) uses the gateway option `--initialize-supervised-stores BACKUP_ROOT`, with absent database destinations, private verified backups and the existing credential files. It must be integrated into the operator's containerized control deployment; this broker guide does not supply a schema3 Compose profile.

There is an essential recovery limit: an ordinary reopen of a schema3 store preserves its namespace in inactive maintenance. Reusing the bootstrap option on that database refuses startup. A safe reactivation procedure is not currently delivered. Preserve databases, backups and runtime evidence; do not delete a database, edit activation metadata or create replacement identities to bypass this condition. `broker resume` reconciles a job and cannot reactivate the control. There is no `broker init`, `broker doctor` or OS-service installation command in the current CLI.

## Write the private operator registry

`broker serve --config` is the only reader of this registry. The CLI does not source dotenv or merge shell settings into it. The registry sets runtime, image, policy, filesystem roots and provider references; a submitting client cannot override those controls. `--profile` on broker client commands selects a registry `id`, whereas the decision policy is the registry's `policy_profile`.

The following is a **complete Registry wire version2 template** for a new Docker installation. Replace every `<...>` value and every `/absolute/...` reference with reviewed installation values before loading it. The two UUID placeholders must be non-nil UUIDs; the installation UUID is newly assigned to this installation, while the namespace UUID and tenant come from the real authenticated control. Paths and the image are illustrative references, not installed resources. No credential value belongs in this JSON.

```json
{
  "version": 2,
  "installation_id": "<new-installation-uuid>",
  "root": "/absolute/private/ai-guard/broker-state",
  "max_enforcer_slots": 8,
  "profiles": [
    {
      "id": "strict-docker",
      "gateway": "<actual-gateway-origin>",
      "tenant_id": "<tenant-id-returned-by-control>",
      "control_namespace_uuid": "<namespace-uuid-returned-by-control>",
      "config_dir": "/absolute/private/ai-guard/config",
      "runner_token_file": "/absolute/private/ai-guard/credentials/runner",
      "runtime": "docker",
      "runtime_program": "/absolute/approved/docker",
      "runtime_endpoint": "unix:///absolute/engine/docker.sock",
      "image": "<prepared-worker-image-reference>",
      "policy_profile": "strict",
      "network": "none",
      "allow_rootful_runtime": false,
      "max_timeout_seconds": 30,
      "allow_capture_output": false,
      "managed_clients": false,
      "providers_file": null,
      "manual_credential_file": null,
      "source_roots": ["/absolute/data/projects"],
      "export_roots": ["/absolute/data/exports"],
      "prearmed_start_profile": {
        "version": 1,
        "mode": "docker_minimal"
      }
    }
  ],
  "additional_control_secret_files": []
}
```

All registry/profile/start-profile objects reject unknown fields. Registry2 requires `prearmed_start_profile` in **every** profile. Its closed form has exactly `version: 1` and `mode: "docker_minimal"` or `mode: "legacy"`. Missing selection, unknown version/mode/field and `docker_minimal` with Podman are errors, never requests for a fallback.

Registry1 retains its original closed schema and Legacy behavior; do not add the new field to it. Registry2 can select Legacy explicitly with the following **fragment inside one profile**, not a complete registry:

```json
"prearmed_start_profile": { "version": 1, "mode": "legacy" }
```

For new Docker configuration, select `docker_minimal` explicitly as in the complete template. It gives prearmed Docker start its own private minimal client configuration rather than silently reusing host Docker configuration. The broker creates and binds that material before acceptance; operators do not create or edit its internal files. This selection does not remove central authorization, mandatory audit, engine checks or confinement, and does not promise fencing of an already ambiguous start.

| Registry field | Meaning and bounds |
|---|---|
| `installation_id`, `root` | Stable installation identity and durable private state root. Replacing them does not recover an existing installation. |
| `max_enforcer_slots` | 1–1024 retained stop targets, shared by the service; helpers also use capacity. This is not a qualified worker-count setting. |
| `profiles`, `id` | 1–32 profiles; unique IDs, 1–80 ASCII letters/digits/`_`/`-`. |
| `policy_profile`, `network` | `strict`, `review` or `observe`; `none` or `bridge`. Bridge is outbound connectivity, not a destination allowlist. |
| `max_timeout_seconds`, `allow_capture_output` | Duration 1–86400 seconds; capture must be enabled here before a client can request it. Without a requested timeout, Submit uses the smaller of 300 seconds and this limit. |
| `managed_clients`, `providers_file` | Managed selection requires `true` and an absolute explicit provider-registry path. See the [provider guide](../providers/README.md). |
| `manual_credential_file` | Optional absolute private credential input for manual commands; `null` supplies none. It is not a runner file. Managed selection uses its selected provider credential instead. |
| `source_roots`, `export_roots` | 1–8 bounded absolute paths in each list, subject to authorization and physical filesystem checks. |
| `additional_control_secret_files` | Up to 16 additional absolute private control-credential references for exclusion and secret checks. Not a worker credential list or a general secret scanner. |

All control paths are absolute, at most 4096 bytes and 64 path components. Keep registry, control credentials, provider inputs and broker data outside source/export trees. An unsafe or missing control credential closes new business work; it is not repaired by borrowing a different profile's key. Configuration is loaded at startup and retained files are revalidated. Editing a live registry is not hot reload and can cause requests to fail.

The start-profile choice is used only for a **new acceptance**. An already Accepted operation, including one awaiting Review, keeps its accepted launch binding on exact Submit replay, approval and resume. Changing the registry selection cannot upgrade or downgrade it. Other frozen profile settings, the approved executable and its identity must still match; a mismatch is a refusal, not a new fingerprint. Registry wire2, launch-profile version1 and persistent bindings/Accepted version2 are different versioned formats. Do not infer that old binaries can read new jobs or roll back while jobs remain active.

## Start the foreground service

Set these shell variables to the installation prepared above. They are command arguments, not settings automatically consumed from the environment. The credential variable contains a **path only**.

```sh
AIGUARD_BIN="/absolute/path/to/agentsandbox"
AIGUARD_BROKER_CONFIG="/absolute/private/ai-guard/operator/registry.json"
AIGUARD_BROKER_ROOT="/absolute/private/ai-guard/broker-state"
AIGUARD_BROKER_PROFILE="strict-docker"
AIGUARD_RUNNER_FILE="/absolute/private/ai-guard/credentials/runner"
AIGUARD_PROJECT="/absolute/data/projects/your-project"
"$AIGUARD_BIN" broker serve --config "$AIGUARD_BROKER_CONFIG"
```

`serve` runs in the foreground. Keep it in a deliberately managed control context and use another trusted terminal for clients. It acquires a broker lease, discovers or explicitly starts the internal stop enforcer from the same binary, restores proved stop targets and publishes a private Unix endpoint. Client discovery never starts a missing broker. IPC records and sockets are service-owned implementation data; do not delete, replace or adopt them to repair a busy or unavailable service.

This command installs no systemd unit or launchd job and makes no logout, reboot, live-freeze recovery or autonomous-deadline qualification claim. A stale endpoint or occupied lease does not authorize takeover. The [supervision contract](../../engineering/BROKER_SUPERVISION.md) separates these obligations from the implemented foreground command.

## Submit and inspect a job

In the client terminal, set the same path variables. Choose a fresh non-nil operation UUID **once**, record it and keep the same inputs for a retry. The value below must be replaced before execution.

```sh
AIGUARD_OPERATION="<new-operation-uuid>"
"$AIGUARD_BIN" broker submit --root "$AIGUARD_BROKER_ROOT" --profile "$AIGUARD_BROKER_PROFILE" --token-file "$AIGUARD_RUNNER_FILE" --operation-id "$AIGUARD_OPERATION" --project "$AIGUARD_PROJECT" --timeout-seconds 30 -- /usr/bin/id
"$AIGUARD_BIN" broker status --root "$AIGUARD_BROKER_ROOT" --profile "$AIGUARD_BROKER_PROFILE" --token-file "$AIGUARD_RUNNER_FILE" --operation-id "$AIGUARD_OPERATION"
"$AIGUARD_BIN" broker list --root "$AIGUARD_BROKER_ROOT" --profile "$AIGUARD_BROKER_PROFILE" --token-file "$AIGUARD_RUNNER_FILE" --limit 20
```

Submit accepts either manual argv after `--`, or managed selection with `--client codex|claude-code|auto`. Managed selection requires `--prompt`; its optional flags are `--auth-mode auto|api|subscription`, `--client-profile` and `--model`. Do not combine managed flags with manual argv. The default registry above disables managed clients. To use one, configure an authorized profile with `managed_clients: true`, an absolute `providers_file`, the appropriate prepared image and explicitly approved networking; then choose that profile with `--profile`. For example, after that separate preparation:

```sh
AIGUARD_BROKER_PROFILE="<configured-managed-broker-profile-id>"
AIGUARD_OPERATION="<another-new-operation-uuid>"
"$AIGUARD_BIN" broker submit --root "$AIGUARD_BROKER_ROOT" --profile "$AIGUARD_BROKER_PROFILE" --token-file "$AIGUARD_RUNNER_FILE" --operation-id "$AIGUARD_OPERATION" --project "$AIGUARD_PROJECT" --timeout-seconds 30 --client claude-code --auth-mode api --client-profile claude-api --model claude-haiku-5-5 --prompt "Report the top-level file names only. Do not modify files, run tests, or start subagents."
```

This is an alternative new job, not a second Submit for the manual example's UUID. Assign a new operation UUID first. The selected provider resolution is returned with Accepted and shown by the CLI; an accepted binding is not re-resolved on replay. Provider authentication remains separate from ai-guard authentication. Never place a credential in a prompt or argv. `--capture-output` is optional and requires the profile's permission; captured output has its own bounds and coverage, not an assumed complete transcript. This broker CLI is batch-only and has no `--interactive` option.

If `--operation-id` is omitted, the CLI generates one and prints `Operation: UUID` before the request. Retain it even after an error. Submit payload bytes bind project, execution, requested timeout and capture choice. Exact replay uses the same operation identity and inputs; changed content under that UUID is a conflict. An Accepted reply confirms durable local acceptance, not Allow, Running, completion or a requested file change. Rejected is a recorded preparation outcome; inspect its `phase`, `code`, `record_hash` and `replayed` fields.

Status and list report cached journal/control facts. `accepted`, `reserved_in_process`, `interrupted`, `rejected` and `absent` are local job states, not runtime terminal states. Inspect `last_decision`, `review_request_hash`, `pending_rpc_count`, `worker_state`, `quota_held`, `runtime_status`, `last_observed_at` and `executor_error` together. Null or unavailable facts remain unknown. `executor_task_running: false` does not prove that a container stopped.

List limits are 1–100, default20. Continue with `--after <next_after-uuid>` only when the reply returns that cursor. A page is not a tenant-wide total or a snapshot across pages. Corruption, incomplete state and access errors must not be treated as empty activity.

## Approve, cancel and resume with stable identities

Every mutation targets the original operation. Assign a separate fresh non-nil `--command-id` to each deliberate approve, cancel, resume or export. Record it before sending; retry that same command with the same UUID and content after a lost response. Do not reuse an approval/cancel/resume/export UUID for a different command or destination. If omitted, the CLI generates and prints `Command: UUID`; keep that value for recovery.

For Review, inspect the exact outstanding request and decision, then use the full `review_request_hash` from status. The approval records an approval submission for that request and still requires a central approved decision; it is not a global Allow. A technical runner credential alone does not establish a named human author.

```sh
AIGUARD_APPROVAL="<new-approval-command-uuid>"
AIGUARD_REVIEW_HASH="<exact-outstanding-review-request-hash>"
"$AIGUARD_BIN" broker approve --root "$AIGUARD_BROKER_ROOT" --profile "$AIGUARD_BROKER_PROFILE" --token-file "$AIGUARD_RUNNER_FILE" --operation-id "$AIGUARD_OPERATION" --command-id "$AIGUARD_APPROVAL" --request-hash "$AIGUARD_REVIEW_HASH"
```

To request cancellation and inspect what is known:

```sh
AIGUARD_CANCEL="<new-cancel-command-uuid>"
"$AIGUARD_BIN" broker cancel --root "$AIGUARD_BROKER_ROOT" --profile "$AIGUARD_BROKER_PROFILE" --token-file "$AIGUARD_RUNNER_FILE" --operation-id "$AIGUARD_OPERATION" --command-id "$AIGUARD_CANCEL"
"$AIGUARD_BIN" broker recovery-status --root "$AIGUARD_BROKER_ROOT" --profile "$AIGUARD_BROKER_PROFILE" --token-file "$AIGUARD_RUNNER_FILE" --operation-id "$AIGUARD_OPERATION"
"$AIGUARD_BIN" broker status --root "$AIGUARD_BROKER_ROOT" --profile "$AIGUARD_BROKER_PROFILE" --token-file "$AIGUARD_RUNNER_FILE" --operation-id "$AIGUARD_OPERATION"
```

A cancellation ACK is neither a terminal runtime observation nor proof of quota release or absence of a start still in flight. If the ACK is lost or the response is `needs_reconciliation`, retain the same cancel identity. A durable cancellation can exist despite a failed response. It cannot be undone to restart the worker.

`recovery-status` reads confirmed journal metadata even when full business loading is unavailable. Its `source` is `confirmed_journal` and `runtime_evidence` is `not_queried`: it does **not inspect the engine**. `journal_mode: "stop_only"`, `incomplete`, `reconciliation_required`, pending Arms and original deadlines describe retained evidence, not new observations or execution permission. Missing fields or targets do not establish their absence in the runtime.

For explicit reconciliation of the same accepted job, once the required control and runtime are available:

```sh
AIGUARD_RESUME="<new-resume-command-uuid>"
"$AIGUARD_BIN" broker resume --root "$AIGUARD_BROKER_ROOT" --profile "$AIGUARD_BROKER_PROFILE" --token-file "$AIGUARD_RUNNER_FILE" --operation-id "$AIGUARD_OPERATION" --command-id "$AIGUARD_RESUME"
```

Resume preserves the accepted intent, credentials, runtime authority, original deadlines and consumed claims. It cannot grant another start or renew a duration. A fresh source capability is allowed only by the product's frozen-source checks for the relevant pre-managed stage; do not edit a source, binding or checkpoint to force them to pass. An unavailable dependency, ambiguous runtime state or refused reconciliation remains an explicit error.

## Export into a new destination

After reviewing decisions, observed results and changes, request an audited export from the accepted job. The parent directory must already exist under an allowed export root; the destination must be **new**, outside the source project and private control storage. The original is never rewritten automatically.

```sh
AIGUARD_EXPORT_COMMAND="<new-export-command-uuid>"
AIGUARD_EXPORT="/absolute/data/exports/new-reviewed-result"
"$AIGUARD_BIN" broker export --root "$AIGUARD_BROKER_ROOT" --profile "$AIGUARD_BROKER_PROFILE" --token-file "$AIGUARD_RUNNER_FILE" --operation-id "$AIGUARD_OPERATION" --command-id "$AIGUARD_EXPORT_COMMAND" --destination "$AIGUARD_EXPORT"
```

Keep the command UUID and exact destination together. An acknowledged export returns its request identity, manifest digest and file count. Review the exported content and diff before applying changes to the project. After an error or lost ACK, a destination may already have been published: preserve it, inspect status and retry only the same request. An unknown existing tree cannot be adopted as a completed export. Choose another new destination only as a separate deliberate export with a new command identity, after resolving the previous outcome.

An explicit export after cancellation requires its own subsequent audited grant and all export prerequisites; cancel does not implicitly authorize it. A grant or ACK is not permission to restart the worker. Links, traversal, changed parent identity, unsafe permissions and unavailable evidence cause a refusal rather than partial success hidden as an empty diff.

## Diagnose errors and preserve recovery evidence

The CLI prints JSON replies and exits with an error for `error` and `rejected`. Transport errors distinguish a request not sent from an unknown outcome or unavailable reply. Preserve operation/command identities before retrying; a nonzero client exit is not proof that no durable mutation occurred.

| Code or symptom | Meaning and next action |
|---|---|
| `unauthorized` | The presented credential was not authenticated. Check the private file reference and selected gateway/profile without exposing its contents. |
| `forbidden` | Role, tenant, namespace or allowed resource does not match. Correct the authorized installation scope; a registry edit does not grant membership. |
| `incompatible_control` | Schema/protocol is incompatible, or fresh execution requires an active control. Follow the control prerequisite and retain existing stores. |
| `unavailable` | A required dependency or service is unavailable. Restore the same dependency and inspect the original operation. |
| `busy` | A bounded service, writer or job resource is occupied. Inspect and retry the same request; do not create duplicate jobs to bypass capacity. |
| `invalid_input` | Validate options, IDs, profile configuration, paths and requested bounds. A new meaning needs a new identity, not changed content under an existing one. |
| `conflict` | An identity, immutable input, authority or revision differs. Compare the recorded request; preserve both the evidence and the refusal. |
| `interrupted` | Preparation did not complete. Inspect the original operation; an interrupted reservation is not permission to create a second ticket or start. |
| `limit_exceeded` | Configured capacity or durable journal bounds are exhausted. Investigate retained work and approved capacity; do not erase evidence to free space. |
| `needs_reconciliation` | The operation cannot safely establish the next step. Use status and recovery-status, then the same explicit resume/cancel where admissible. Unknown remains unknown. |
| Broker absent or unsafe discovery | Start `broker serve` explicitly only after checking the installation and existing owner. Clients do not autostart it. Preserve leases, endpoint records and state. |

The source of these closed responses and fields is [the IPC protocol](../../../src/broker/service/protocol.rs); implemented flags are in [the CLI](../../../src/broker_cli.rs), and registry fields are in [the profile parser](../../../src/broker/service/profile.rs). Internal journal files can contain credentials and private content. Neither a public error report nor an exported proof bundle should include raw registry, bindings, tokens, database contents or unreviewed output.

## Stop, restart and maintain the installation

Request cancellation for affected jobs, then check their durable and observed state before stopping control services. When an observation is unavailable, retain its uncertainty and enforcement state. Do not equate a client exit, Created, runtime absence, timeout, `kill` or `rm` with fencing of an in-flight start. Do not terminate/disarm a retained enforcer, rearm a consumed start, delete its discovery files, remove volumes or erase evidence to obtain a clean status.

The foreground broker handles SIGINT/SIGTERM by closing admissions and invalidating its start generations; its distinct enforcer retains already armed targets and original deadlines. This is not a general cleanup or disarm procedure. Restart the foreground broker with the same binary/configuration/root and compatible control only after investigating the previous owner. The active-control restriction above still applies after gateway restart.

Preserve the private registry, installation identity, broker journal, export-input records, configuration and credential bindings, gateway databases and coherent backups, retained runtime authorities and worker volumes together. Backup/restore must account for SQLite WAL state, confidentiality and the authority of outstanding claims. Restoring old files does not restore fresh execution permission. No automatic retention or safe rollback is established by this guide; follow [operations](../../operations.md) and the [journal/recovery contract](../../engineering/BROKER_JOURNAL.md) for the applicable boundaries.

The complete product also requires safe control reactivation, OS-managed lifecycle, ambiguous-start fencing, terminal continuity and real platform/failure qualification. Their current status and required evidence remain in [engineering](../../engineering/PRODUCTION_READINESS.md), including [the supervision contract](../../engineering/BROKER_SUPERVISION.md). Do not substitute the foreground commands or a configuration example for those results.
