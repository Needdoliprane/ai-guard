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

Every request authenticates its **presented** credential afresh against `GET /api/v1/control/protocol`. The broker requires `role: "runner"`, the configured tenant and namespace, `schema_version: 3`, `4` or `5` and the `broker_supervision_v1` protocol. Submit, approve, resume, export and abandon-before-claim also require `active: true`. Status, list, recovery-status and cancel can authenticate against an inactive compatible control; their other dependencies and safety checks still apply. Being the same Unix UID is transport access, not tenant authentication.

Read the real tenant, namespace, role and supervision state before writing the registry. Replace the binary, origin and private credential path with your installation values:

```sh
"/absolute/path/to/agentsandbox" broker control-info \
  --gateway "http://127.0.0.1:8787" \
  --token-file "/absolute/private/ai-guard/credentials/runner"
```

This read needs neither a broker registry nor a running broker. It prints the validated control metadata as JSON, including `schema_version`, `active` and available `protocols`. A successful exit confirms an authenticated metadata read; check the active-control requirements above separately. A viewer credential can read its own tenant metadata with role `viewer`, while the broker requires `runner`. Schema2 already has a namespace and is reported inactive. Authentication, unsafe credential files and incompatible/unavailable responses fail with a nonzero exit; the credential is never printed. If a metadata value contains that credential, the command refuses output instead of rewriting the identity.

Do not guess the namespace, copy a test UUID, choose another tenant in the registry, or paste a runner into the dashboard. The configured `runner_token_file` serves the broker's autonomous reconciliation; it does not replace authentication of a client request. The client `--token-file` identifies the credential presented for that request.

The standard local Compose files start schema2 control. `/healthz` only establishes service availability. Neither a healthy stack nor `init` establishes the supervision protocol or fresh execution authority. The existing [exclusive bootstrap for new supervised control stores](../../operations.md#initialiser-un-contrôle-de-supervision-neuf) uses the gateway option `--initialize-supervised-stores BACKUP_ROOT`, with absent database destinations, private verified backups and the existing credential files. It must be integrated into the operator's containerized control deployment; this broker guide does not supply a schema3 Compose profile.

There is an essential recovery limit: an ordinary reopen of a schema3, schema4 or schema5 store preserves its namespace in inactive maintenance. Reusing the bootstrap option on that database refuses startup. A safe reactivation procedure is not currently delivered. Preserve databases, backups and runtime evidence; do not delete a database, edit activation metadata or create replacement identities to bypass this condition. `broker resume` reconciles a job and cannot reactivate the control. There is no `broker init`, `broker doctor` or OS-service installation command in the current CLI.

### Upgrade storage explicitly

Stop the gateway and every broker/consumer of the tenant database, including automatic restarts. `migrate-control --target-version` accepts the exact next version2,3,4 or5; its default remains2. Use a distinct new backup for every step. Schema5 adds mandatory nominative read audit, as described in [named local access](../identity/README.md). For an existing schema3 source:

```sh
"/absolute/path/to/agentsandbox" migrate-control \
  --data "/absolute/private/ai-guard/audit.sqlite" \
  --tenant-id "<configured-tenant-id>" \
  --backup "/absolute/private/ai-guard/audit-before-identities-v4.sqlite" \
  --target-version 4 \
  --confirm-control-stopped
```

Update gateway and broker readers together before opening schema4. The verified backup and old events, namespace, tenant binding and consumed claims are preserved; an old binary refuses the new schema. Migration/reopening does not activate inactive supervision. Schema4 prepares human identity and permission storage; it does not create an account, attribute old events to a person, turn a technical key into a human credential or perform an OIDC login. Retain backups and reports after failure and investigate before selecting a new destination. See [offline maintenance](../../operations.md#répertoire-sqlite-privé) for the checked backup and restoration limits.

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

The enforcer's private IPC authentication belongs to the broker state and is separate from runner/viewer credentials. Existing authentication is read without rewriting it. If it is missing from a role that already has state, startup refuses to generate a replacement. Preserve the role root, its history and coherent private backups; recover the original authentication through controlled maintenance. Erasing history to obtain an apparently empty root is not a recovery procedure.

### Select a Full3 enforcer explicitly

The command above keeps the historical enforcer selection. To use Full3, select one of the following entries explicitly. The enforcer choice is separate from the registry's `prearmed_start_profile`; neither existing files nor Registry2 activate Full3 automatically. Use the CLI and service from the same release. Before either command, stop the previous foreground broker deliberately and verify that its broker lease is released. Preserve its guardian and retained state.

Choose a role name containing 1–100 ASCII letters, digits, hyphens or underscores. `broker`, `journals` and `enforcer` are reserved. This is one private child name under the configured broker `root`, never a path. Record it with the installation identity and registry.

For a fresh role:

```sh
AIGUARD_ENFORCER_ROLE="full3-primary"
"$AIGUARD_BIN" broker serve --config "$AIGUARD_BROKER_CONFIG" \
  --enforcer-full3-new "$AIGUARD_ENFORCER_ROLE"
```

The role must not exist. The current fresh entry accepts only a new installation or configured profile journals confirmed empty with complete coverage. An existing job, unknown coverage or an unconfigured journal profile causes refusal. This is not a migration of existing jobs or guardians. Keep their roots, journals, credentials, processes, volumes and deadlines; do not empty or rename state to pass the check, and do not adopt the historical `enforcer` role as Full3.

For a later broker connection, preserve the guardian and reuse the same installation, registry, root and role name:

```sh
"$AIGUARD_BIN" broker serve --config "$AIGUARD_BROKER_CONFIG" \
  --enforcer-full3-existing "$AIGUARD_ENFORCER_ROLE"
```

This entry connects only to the existing sealed role. It checks the installation identity and retained ledger before admission. If the peer is absent or its state is incompatible, it refuses; it does not spawn a Full3 replacement, initialise another history or fall back to Legacy. A loaded Arm keeps its confirmed role assignment and cannot be registered again as a fresh original Arm.

The two Full3 options are mutually exclusive and cannot be combined with `--recovery-only` or `--enforcer-role`. Choose the stop-only entry below when admission must stay closed.

### Recover authorised stops with admission closed

For an existing installation that needs only to restore its proved stop targets, start the explicit restricted service:

```sh
"$AIGUARD_BIN" broker serve --config "$AIGUARD_BROKER_CONFIG" --recovery-only
```

Without a role name, this keeps historical recovery. To recover proven STOP targets from a previously sealed Full3 role, retain the same installation and explicitly select that role:

```sh
"$AIGUARD_BIN" broker serve --config "$AIGUARD_BROKER_CONFIG" \
  --recovery-only --enforcer-role "$AIGUARD_ENFORCER_ROLE"
```

`--enforcer-role` is accepted only with `--recovery-only` and follows the same child-name rules as Full3. It opens an existing sealed role without initialising new authentication or a new ledger. It does not grant Full3 admission.

This mode requires the existing private roots, role locks, IPC authentication and discovery namespaces. It does not initialise a new installation or regenerate missing authentication. An occupied lease or an incompatible live enforcer remains a refusal; this command never takes over or stops an existing process. Keep its state and use the supported connection to that process where available.

The service requests the `StopsOnly` capability explicitly: protocol2 for historical recovery, protocol3 for the named sealed role. It restores only targets proved by retained journal readers, keeps their original container identity and deadline, and retains durable cancellations for replay after reconnection. Start admission remains closed for the lifetime of this service: Submit, approve, resume, export and abandon-before-claim are refused. Status, list, recovery-status and cancel retain their normal authentication and data dependencies; this mode does not reactivate an inactive gateway.

For each recovery batch, the broker retains validated targets within its configured capacity before waiting for their individual control confirmations. Losing the client waiting for that request does not discard these targets while the broker remains running. Independent targets continue after a target fails; the result reports partial coverage. A confirmation refers to the guardian instance and target revision acknowledged at that point, not to a worker observed stopped. After a broker crash, recovery still requires the retained durable journal.

An accepted stop reservation or cancellation confirms neither a runtime effect nor a terminal result. Partial journal coverage remains partial. The recovery transport does not prove that an older start is drained, finalise a retirement or permit a new worker to replace an ambiguous one. Missing or unsafe roots, authentication, locks or sockets require controlled recovery of those exact anchors; a new root or credential is not a substitute.

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

The recovery-status **payload version2** keeps the version1 fields and adds `known_arms[].preclaim` and `retirements`. The local IPC envelope remains version1; Status and Page payloads are unchanged. Use matching CLI/service releases and update any consumers that validate the closed payload: an older reader can reject these new fields. An older service rejects the unknown abandon-before-claim command; there is no fallback to Cancel or another command.

For explicit reconciliation of the same accepted job, once the required control and runtime are available:

```sh
AIGUARD_RESUME="<new-resume-command-uuid>"
"$AIGUARD_BIN" broker resume --root "$AIGUARD_BROKER_ROOT" --profile "$AIGUARD_BROKER_PROFILE" --token-file "$AIGUARD_RUNNER_FILE" --operation-id "$AIGUARD_OPERATION" --command-id "$AIGUARD_RESUME"
```

Resume preserves the accepted intent, credentials, runtime authority, original deadlines and consumed claims. It cannot grant another start or renew a duration. A fresh source capability is allowed only by the product's frozen-source checks for the relevant pre-managed stage; do not edit a source, binding or checkpoint to force them to pass. An unavailable dependency, ambiguous runtime state or refused reconciliation remains an explicit error.

## Abandon one original Full3 Arm before claim

An Arm identifies an acknowledged enforcement target for a worker or helper. Use `abandon-before-claim` to inhibit the claim of one exact original Full3 Arm, subject to the race with claim and STOP. It does not cancel the entire job. It requires an active compatible control, the same authenticated mutation rights and the service's Full3 admission mode. A historical or restored Arm, a reserved claim/STOP, a mismatched epoch/generation or an unknown owner cannot use this path. Normal execution proceeds immediately without a human waiting period; the claim may win before the operator can read and send a request. The command adds no grace period and renews no deadline.

First read the original operation:

```sh
"$AIGUARD_BIN" broker recovery-status --root "$AIGUARD_BROKER_ROOT" \
  --profile "$AIGUARD_BROKER_PROFILE" --token-file "$AIGUARD_RUNNER_FILE" \
  --operation-id "$AIGUARD_OPERATION"
```

Select its exact `known_arms[].arm_id`. The nested `preclaim` is version1 **process-local metadata**, with `source: "process_local"` and `freshness: "current_process_state"` or `"unknown"`. It exposes `expected_epoch`, `generation`, `state`, `command_id` and `signal_audit_confirmed`. After a restart, missing owner or incomplete journal read, unknown state and null identities do not reconstruct an original owner or an abandonment signal.

| `preclaim.state` | Operator interpretation |
|---|---|
| `registration_in_flight`, `original_unclaimed` | A request may still race the claim, subject to the service's checks. Copy the current epoch and generation exactly. |
| `claim_reserved`, `stop_reserved` | Abandonment cannot win this Arm. Inspect the original job; Cancel remains available under its normal checks. |
| `abandon_pending_audit` | The local claim path is inhibited, but the signal's audit ACK is not confirmed. Do not infer retirement. |
| `abandon_requested` | The signal is audit-ACKed. Only a confirmed retirement record with `phase: "final"` establishes completion. |
| `retiring` | Retirement is in progress; inspect the confirmed `retirements` records. |
| `unknown` | Do not target this projection. Preserve evidence and use normal recovery/cancellation where admissible. |

Record a new command UUID once. Replace every placeholder below with the exact observed identities and decimal generation; all UUIDs must be non-nil. This command requires an explicit `--command-id` and never generates one for you.

```sh
AIGUARD_ARM="<arm-uuid-from-known-arms>"
AIGUARD_EPOCH="<expected-epoch-from-preclaim>"
AIGUARD_GENERATION="<generation-from-preclaim>"
AIGUARD_ABANDON_COMMAND="<new-abandon-command-uuid>"
"$AIGUARD_BIN" broker abandon-before-claim --root "$AIGUARD_BROKER_ROOT" \
  --profile "$AIGUARD_BROKER_PROFILE" --token-file "$AIGUARD_RUNNER_FILE" \
  --operation-id "$AIGUARD_OPERATION" --arm-id "$AIGUARD_ARM" \
  --expected-epoch "$AIGUARD_EPOCH" --generation "$AIGUARD_GENERATION" \
  --command-id "$AIGUARD_ABANDON_COMMAND"
"$AIGUARD_BIN" broker recovery-status --root "$AIGUARD_BROKER_ROOT" \
  --profile "$AIGUARD_BROKER_PROFILE" --token-file "$AIGUARD_RUNNER_FILE" \
  --operation-id "$AIGUARD_OPERATION"
```

After a lost response, retry only the same command UUID, operation, Arm, epoch and generation. Changed content under that UUID is a conflict; a new generation does not inherit the old request. An `abandon_requested` reply confirms only the signal's durable audit ACK. Its revision/checkpoint refer to that acknowledgment, not to Final. A failed or missing ACK never grants retirement or reopens the inhibited claim. If claim or STOP wins, keep the refusal and use Cancel when required; do not substitute another Arm or issue a second job to bypass it.

In recovery-status, match `retirements[].arm_id` and `retirement_id` to the selected Arm and command UUID. `phase: "pending"` is a recorded retirement intention; `intention_audit_confirmed` concerns that intention, separately from the signal ACK. `phase: "final"` and its `guardian_receipt_hash` identify the confirmed completed retirement. Compare the authorized audit events `broker.abandon-before-claim.requested`, `broker.retirement.pending` and `broker.retirement.final`; an absent target in `known_arms` alone is not Final. An unavailable or partial read remains unknown.

When `preclaim.retirement_continuation_retained` is `true`, this broker process still retains an owner after a blocked publication. Inspect the job error and keep the process and its evidence; the flag grants no permission and does not establish Final. A null value is unknown. Restarting does not reconstruct that live owner from journal JSON.

Pending retirement audit events are retried through the restricted writer even while new starts are fenced or a provider credential blob is unavailable. Delivery still requires the configured runner credential and the matching gateway. A Full3 service already holding the original guardian connection can confirm a completed guardian result and record Final after a lost reply; recovery-only never obtains Full authority to do this.

After abandonment, the owned instance keeps its actual Created state and the central allocation keeps its confirmed state. No held quota is released by inference. This outcome supplies no terminal observation, empty output capture, zero usage or automatic cleanup. Retain the instance and use the explicit cancellation/reconciliation procedures with their normal evidence checks. If an export is abandoned, its CLI prints the retirement response and exits unsuccessfully; it does not report an export acknowledgement.

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
