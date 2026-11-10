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

Broker RPCs authenticate their **presented** credential afresh against `GET /control/api/v1/control/protocol` on the published origin. The broker requires `role: "runner"`, the configured tenant and namespace, `schema_version` 3 through 10 and the `broker_supervision_v1` protocol. Submit, approve, resume and export also require `active: true`. Status, list, recovery-status and cancel can authenticate against an inactive compatible control; their other dependencies and safety checks still apply. Being the same Unix UID is transport access, not tenant authentication.

Read the real tenant, namespace, role and supervision state before writing the registry. Replace the binary, origin and private credential path with your installation values:

```sh
"/absolute/path/to/agentsandbox" broker control-info \
  --gateway "http://127.0.0.1:8787" \
  --token-file "/absolute/private/ai-guard/credentials/runner"
```

This read needs neither a broker registry nor a running broker. It prints the validated control metadata as JSON, including `schema_version`, `active` and available `protocols`. A successful exit confirms an authenticated metadata read; check the active-control requirements above separately. This diagnostic explicitly uses the existing authenticated public metadata API: a viewer can read its own tenant metadata with role `viewer`. Broker execution uses the separate runner control protocol; the diagnostic grants no access to that channel and is not its fallback after refusal. Schema2 already has a namespace and is reported inactive. Authentication, unsafe credential files and incompatible/unavailable responses fail with a nonzero exit; the credential is never printed. If a metadata value contains that credential, the command refuses output instead of rewriting the identity.

Do not guess the namespace, copy a test UUID, choose another tenant in the registry, or paste a runner into the dashboard. The configured `runner_token_file` serves the broker's autonomous reconciliation; it does not replace authentication of a client request. The client `--token-file` identifies the credential presented for that request.

The standard local Compose files start schema2 control. `/healthz` only establishes service availability. Neither a healthy stack nor `init` establishes the supervision protocol or fresh execution authority. The existing [exclusive bootstrap for new supervised control stores](../../operations.md#initialiser-un-contrôle-de-supervision-neuf) uses the gateway option `--initialize-supervised-stores BACKUP_ROOT`, with absent database destinations, private verified backups and the existing credential files. It must be integrated into the operator's containerized control deployment; this broker guide does not supply a schema3 Compose profile.

There is an essential recovery limit: an ordinary reopen of a schema3–10 store preserves its namespace in inactive maintenance. Reusing the bootstrap option on that database refuses startup. A safe reactivation procedure is not currently delivered. Preserve databases, backups and runtime evidence; do not delete a database, edit activation metadata or create replacement identities to bypass this condition. `broker resume` reconciles a job and cannot reactivate the control. There is no `broker init`, `broker doctor` or OS-service installation command in the current CLI.

### Keep the gateway origin when selecting control transport

The [control transport](../../engineering/INGRESS_CONTROL_TRANSPORT.md) uses an explicit `/control/` path on the same published gateway origin. Updated Compose profiles use the optional gateway `--control-listen` on internal port 8788 and the required ingress `--control-upstream http://gateway:8788`, without another host port or relay-owned credential. Identity, tenant, authority and consumed claims still come from the existing Rust contracts. Authentication precedes the tenant's separate runner/presence operation budgets; the listener does not activate supervision or migrate storage.

Keep the registry's `gateway` and CLI `--gateway` as origins **without `/control`**; the clients add it. No field is added to closed plans 1/2/3. Runner identity, admission, decisions, typed control reads/writes and trusted control events explicitly use this channel. Broker presence has its own three scoped routes and keeps the same Pending and operation identity, without runner authority. Ordinary ingestion and generic public reads remain public. The mixed broker outbox still needs durable provenance; event kind/source cannot promote its historical entries. Existing foreground commands retain their stated prerequisites and qualification limits.

Update CLI, gateway and ingress together. An older gateway without the control route is refused without public fallback. Preserve the same Pending and operation identity after an unknown transport result. Only eligible post-ACK monitor GETs accept the closed transport DTO with coherent 502/504; protocol failures, malformed responses and audit 503 remain errors. This adds no retry for mutation, start, claim or mandatory audit, and the start callback keeps one attempt. The frozen SDK v2 passes [22 targeted software checks and all-target compilation](../../engineering/evidence/2026-11-07/control-clients/README.md), with warnings retained; the [combined control/recovery checks](../../engineering/evidence/2026-11-07/control-recovery-composition/README.md) also pass 52 software functions and all-target compilation. Consult the [local guide](../local/README.md#explicit-control-transport) for deployment options; runtime, FD/TCP and end-to-end H qualification remain separate.

### Upgrade storage explicitly

Stop the gateway and every broker/consumer of the tenant database, including automatic restarts. `migrate-control --target-version` accepts the exact next version from 2 through 10; its default remains 2. Use a distinct new backup for every step. Schema5 adds mandatory nominative read audit, as described in [named local access](../identity/README.md). Schema6 adds workload bindings, broker registry, directed relay and a durable UTC floor; follow the [workload guide](../workloads/README.md) for explicit5→6 migration, protected audit budgets and HTTP role contracts. This does not connect workers to a runtime transport or activate supervision. For an existing schema3 source:

```sh
"/absolute/path/to/agentsandbox" migrate-control \
  --data "/absolute/private/ai-guard/audit.sqlite" \
  --tenant-id "<configured-tenant-id>" \
  --backup "/absolute/private/ai-guard/audit-before-identities-v4.sqlite" \
  --target-version 4 --max-events 100000 \
  --confirm-control-stopped
```

Update gateway and broker readers together before opening schema4. The verified backup and old events, namespace, tenant binding and consumed claims are preserved; an old binary refuses the new schema. Migration/reopening does not activate inactive supervision. Schema4 prepares human identity and permission storage; it does not create an account, attribute old events to a person, turn a technical key into a human credential or perform an OIDC login. Retain backups and reports after failure and investigate before selecting a new destination. See [offline maintenance](../../operations.md#répertoire-sqlite-privé) for the checked backup and restoration limits.

### Read registered brokers after migration7

Schema7 adds human read access to the tenant's existing broker registry through `GET /api/v1/human/brokers`. Follow the [exact5→6→7 backup procedure](../workloads/README.md#enable-named-broker-registry-reads), including for SSO setup storage at5. Migration uses the same tenant database and adds its audit capability marker without activating supervision. Readers accepting7 retain5/6 compatibility; update gateway, broker/STOP clients and dashboard together before opening7. This issues no new claim and supplies no workaround for inactive cold reopen or unresolved fencing/recovery.

The **Brokers** dashboard view requires explicitly granted administration capability `read_broker_registry`; [named-access instructions](../identity/README.md#authorize-named-broker-registry-readers) cover provisioning, membership updates, withdrawal and recovery. It shows tenant infrastructure independently of organization-filtered `read_tenant_metadata` resource access. SSO requires membership and a configured claim ceiling. Never send runner credentials to the browser.

Records come from real authenticated [binding/grant/heartbeat operations](../workloads/README.md#grant-a-broker-and-interpret-its-heartbeat). To publish the foreground service's presence, explicitly configure its wire2 profile's `presence_file` using the [presence guide](PRESENCE.md) and [typed example](examples/presence.json). Profiles without it do not publish automatically. A current lease records a broker declaration and gateway reception, not runtime inspection or worker health; a stale/revoked entry neither stops a runtime nor releases quota. Preserve any older heartbeat incarnation and reception time during diagnosis.

This human registry differs from private Registry2 below and the local IPC `broker list` job list. It exposes no raw grants or credential IDs/digests, supplies no export, worker federation or engine access to the frontend. Cumulative audit refusal requires preserving database/policy and repairing capacity before an explicit retry, without deleting audit or reinitializing control. The view adds no production qualification or supervision reactivation path.

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

For a gateway using a private CA, Registry2 profiles accept the optional absolute
`gateway_ca_bundle_file` reference shown in the [consumed operator example](examples/gateway-trust.json).
Use a public PEM certificate bundle only: at most 64 KiB and 1–8 certificates;
private keys, unrelated PEM sections, malformed or missing files are refused.
An explicit bundle requires an HTTPS gateway and replaces the built-in roots
for that profile's audit client. Certificate-chain and server-name validation
remain active. There is no proxy, redirect, HTTP downgrade or trust-all fallback.
Without this field, or with `null`, the existing built-in HTTPS trust and HTTP
loopback behavior remain unchanged. Registry1 does not accept this field.

The broker loads the bundle before opening IPC and retains an immutable trust
snapshot in the registry. The same audit client uses it to verify the runner
identity and to send subsequent requests/events. Replacing the source file does
not reload an active registry or client; a fresh registry load validates the
file again and refuses invalid material. Review trust changes before restarting
the service. The reference contains no credential value and grants no tenant,
worker, execution or broker-registry permission.

This setting is independent of presence `ca_bundle_file`: neither configuration
inherits trust from the other. If both need private roots, declare both
explicitly, using the reviewed bundle for each. The local TLS tests do not
qualify enterprise PKI, certificate rotation/revocation or physical multi-host
federation; broker/worker provenance and freshness keep their existing limits.

Registry1 retains its original closed schema and Legacy behavior; it accepts
neither `prearmed_start_profile` nor `gateway_ca_bundle_file`. Registry2 can
select Legacy explicitly with the following **fragment inside one profile**,
not a complete registry:

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
| `presence_file` | Optional absolute private Presence V1 JSON path, wire2 profiles only; see [authority, storage and restart requirements](PRESENCE.md). Does not issue credentials, grant authority or activate supervision. |

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

### Recover authorised stops with admission closed

For an existing installation that needs only to restore its proved stop targets, start the explicit restricted service:

```sh
"$AIGUARD_BIN" broker serve --config "$AIGUARD_BROKER_CONFIG" --recovery-only
```

This mode requires the existing private roots, role locks, IPC authentication and discovery namespaces. It does not initialise a new installation or regenerate missing authentication. An occupied lease or an incompatible live enforcer remains a refusal; this command never takes over or stops an existing process. Keep its state and use the supported connection to that process where available.

The service requests protocol 2's `StopsOnly` capability explicitly. It restores only targets proved by retained journal readers, keeps their original container identity and deadline, and retains durable cancellations for replay after reconnection. Start admission remains closed for the lifetime of this service: Submit, approve, resume and export cannot acquire a Full capability. Status, list, recovery-status and cancel retain their normal authentication and data dependencies; this mode does not reactivate an inactive gateway.

For each recovery batch, the broker retains validated targets within its configured capacity before waiting for their individual control confirmations. Losing the client waiting for that request does not discard these targets while the broker remains running. Independent targets continue after a target fails; the result reports partial coverage. A confirmation refers to the guardian instance and target revision acknowledged at that point, not to a worker observed stopped. After a broker crash, recovery still requires the retained durable journal.

An accepted stop reservation or cancellation confirms neither a runtime effect nor a terminal result. Partial journal coverage remains partial. The recovery transport does not prove that an older start is drained, finalise a retirement or permit a new worker to replace an ambiguous one. Missing or unsafe roots, authentication, locks or sockets require controlled recovery of those exact anchors; a new root or credential is not a substitute.

### Deliver retained terminal audit events

This V4 outbox path is distinct from the direct `run`/`worker-resume` file and
capture journal described in the [local recovery guide](../local/README.md#reconcile-the-recorded-worker).
The new control-event route does not migrate these broker records or establish
that this service has performed the direct CLI's file scan and diff recovery.

The restricted service also retries the existing V4 `observer.coverage` and
`broker.batch.result` events of a proved terminal worker. Configure the profile's
`runner_token_file` with its trusted runner credential. Each attempt authenticates
that same client against the configured gateway and checks tenant, namespace,
retained worker/attempt and terminal observation. The central worker must already
be terminal with its quota released and consistent. An inactive control may serve
these authenticated audit operations; this does not activate execution.

The broker sends the retained events with their original IDs and payloads, then
records only the exact returned audit sequences. If the second send fails or an
orderly stop is observed after the first receipt, it submits the received subset.
The other event stays pending for a later bounded retry. Identity failure or a
terminal inconsistency leaves the events unacknowledged. No runtime observation,
provider login, start, export or cleanup is performed by this audit path.
Delivery updates only the local outbox acknowledgements; it does not finalise
the managed result or retire its supervision lifecycle.

After an ambiguous local completion, the running writer retains the exact input
and its logical reservation in quarantine. On a later reconcile pass, the
restricted service can confirm that same sealed receipt batch and durable target,
including later Cancel revisions, using the existing actor and lease. This
confirmation writes no checkpoint and sends no HTTP. It releases the reservation
only after all exact metadata checks succeed. After confirmation or a confirmation
error, that actor receives no further attempt in this pass; the other profiles
continue. A remaining event is considered by a later normal pass. Losing the
confirmation waiter does not cancel accepted work, and repeating it after success
supplies no new confirmation or release.

An unsealed attempt, an unproved target or a failed metadata check stays in
quarantine. That broader recovery is not implemented by this confirmation path;
new cold-audit preparation remains blocked while ordinary cancellation stays
available. Quarantine is process-local: actor shutdown drops its in-memory input
without proving a completion or providing recovery after process death. Preserve
the journal; do not delete partial files or replace its root to bypass a refusal.

On normal shutdown, the broker closes admission, joins the reconcile producer,
and waits for every retained-audit owner already accepted. It does not abandon
those owners on a global drain timeout. If shutdown begins after the first
receipt, that exact receipt is acknowledged and the second event remains pending;
the job stays incomplete for a later bounded retry. An owner error or join error
makes shutdown degraded only after the other accepted owners have also been
drained. This is an internal test of the same shutdown helper used by `serve`;
it does not qualify SIGTERM handling, process death, physical failures or a
10-second delay. The 45-second HTTP budget and writer budgets are unchanged, and
these checks establish no duration SLO.

The same original 45-second HTTP budget covers identity, worker read and sends.
The detached task survives loss of the reconcile-loop observer while the process
and async runtime remain alive; this is not a power-loss or process-death
guarantee. A writer failure is not an ACK. Preserve the journal and event IDs for
recovery; do not reset its root to force a retry. The [validation record](../../engineering/VALIDATION.md)
states which software, failure and runtime scenarios have actually been run.

### Interpret a retirement Resume audit request

The exact kind `broker.retirement.resume.requested` is closed request data for
the existing runner-only POST `/api/v1/control/events`, through
`/control/api/v1/control/events` for control transport. Ordinary ingestion or
Store append refuses it, including with spare budget. Its version1 body has11
bounded fields: canonical UUIDs, generation, digests and the original deadline.
Unknown fields, invalid attribution or a masking collision in a bound field
refuse. The authenticated credential still selects the tenant.

The internal HTTP sender requires the whole original Event unchanged and
well-formed StoredEvent metadata; a positive sequence alone is not an ACK.
It keeps the smaller of the original remaining deadline and the existing
45-second budget. Preserve original IDs/payloads and receipts after an ambiguous
response; timeout does not prove rollback of a durable audit commit.

This Core/HTTP slice records only a request. It does not wire a retirement retry
into `broker resume`, reconstruct an original owner/timer from JSON, confirm a
local writer ACK, release quota or grant a start. A fact/receipt is no permission,
runtime observation, fencing or cold recovery. Use the existing status and
reconciliation procedures; no new CLI, service or credential is introduced.

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

If the operation UUID was lost, discover accepted jobs within the authenticated profile:

```sh
"$AIGUARD_BIN" broker recovery-inventory --root "$AIGUARD_BROKER_ROOT" --profile "$AIGUARD_BROKER_PROFILE" --token-file "$AIGUARD_RUNNER_FILE" --limit 20
```

Cold discovery requires an existing broker explicitly started with `broker serve --config "$AIGUARD_BROKER_CONFIG" --recovery-only`. A normal service with an unopened profile returns `needs_reconciliation` without opening that journal or changing its admission state. An already retained Full/STOP queue can be read in place. Do not start a second owner or replace its root to bypass a lease. Recovery mode keeps admission closed; this command never starts or resumes a worker.

If the required journal directory, profile or journal namespace is absent, valid recovery reads and cancellation requests return `needs_reconciliation`. This is not an empty inventory and does not establish that no worker exists. Check the selected installation and preserve its original journal, ownership and coherent backups; do not initialize a replacement journal to clear the error. Nil UUIDs or invalid pagination bounds still return `invalid_input` after authentication; malformed frames can be rejected before authentication. A valid but unknown operation in an existing journal keeps its existing refusal.

The response contains confirmed accepted-job UUIDs and journal heads, not runtime activity, business payloads or a total worker count. Use a returned `operation_id` with `recovery-status` above. Pagination accepts `--limit` from 1 to 100 and an optional `--after` UUID; continue only with the returned `next_after`. A partial scan supplies issues and no continuation guarantee; missing data is not evidence of an idle runtime. Loss of a shared journal anchor fails the read. A scan can still read the journal's configured bounds; a small page does not guarantee low latency. At most one inventory is queued or running per retained writer, including after a caller timeout. This does not preempt an ongoing scan or reserve queue space for cancellation.

For explicit reconciliation of the same accepted job, once the required control and runtime are available:

```sh
AIGUARD_RESUME="<new-resume-command-uuid>"
"$AIGUARD_BIN" broker resume --root "$AIGUARD_BROKER_ROOT" --profile "$AIGUARD_BROKER_PROFILE" --token-file "$AIGUARD_RUNNER_FILE" --operation-id "$AIGUARD_OPERATION" --command-id "$AIGUARD_RESUME"
```

Resume preserves the accepted intent, credentials, runtime authority, original deadlines and consumed claims. It cannot grant another start or renew a duration. A fresh source capability is allowed only by the product's frozen-source checks for the relevant pre-managed stage; do not edit a source, binding or checkpoint to force them to pass. An unavailable dependency, ambiguous runtime state or refused reconciliation remains an explicit error.

After a durable cancellation, reconciliation of a previously Running or Stopping worker uses only its retained runtime authority and immutable container ID. A complete terminal observation must match the retained ownership, configuration, resource limits and centrally observed start time. Only the gateway's durable transaction recording Terminal releases the allocation quota. The allocation record remains nonbillable, and repeating reconciliation does not add another allocation. An unavailable, still-running or divergent observation leaves the quota held and reports a reconciliation error.

This observation performs no new start, helper, cleanup or enforcer retirement, and does not renew a deadline. Use `status` after the command acknowledgement: accepting Resume is not a terminal result. Resume still requires the normal active compatible control and Full service prerequisites above; `--recovery-only` does not enable it or reactivate an inactive control.

The code covers Running and Stopping; targeted tests currently exercise Running only, using synthetic runtime observations and a 60-second fixture writer budget. Reliability under the product's normal 10-second writer reply budget remains unqualified. An error or lost acknowledgement may follow a durable terminal transition: inspect `status` before concluding failure or retrying. These results do not provide cold recovery of an inactive control. See the [validation scope](../../engineering/TEST_STRATEGY.md#réconciliation-après-cancel--sélection-logicielle-warm).

## Export into a new destination

After reviewing decisions, observed results and changes, request an audited export from the accepted job. The parent directory must already exist under an allowed export root; the destination must be **new**, outside the source project and private control storage. The original is never rewritten automatically.

```sh
AIGUARD_EXPORT_COMMAND="<new-export-command-uuid>"
AIGUARD_EXPORT="/absolute/data/exports/new-reviewed-result"
"$AIGUARD_BIN" broker export --root "$AIGUARD_BROKER_ROOT" --profile "$AIGUARD_BROKER_PROFILE" --token-file "$AIGUARD_RUNNER_FILE" --operation-id "$AIGUARD_OPERATION" --command-id "$AIGUARD_EXPORT_COMMAND" --destination "$AIGUARD_EXPORT"
```

Keep the command UUID and exact destination together. An acknowledged export returns its request identity, manifest digest and file count. Review the exported content and diff before applying changes to the project. After an error or lost ACK, a destination may already have been published: preserve it, inspect status and retry only the same request. An unknown existing tree cannot be adopted as a completed export. Choose another new destination only as a separate deliberate export with a new command identity, after resolving the previous outcome.

An explicit export after cancellation requires its own subsequent audited grant and all export prerequisites; cancel does not implicitly authorize it. A grant or ACK is not permission to restart the worker. Links, traversal, changed parent identity, unsafe permissions and unavailable evidence cause a refusal rather than partial success hidden as an empty diff.

The [workspace export policy](../local/README.md#inspect-and-export-changes) also applies here: blocked metadata is omitted, Git-directory-shaped output is refused before publication, and published regular files use `0644`. The V1 IPC acknowledgement keeps its existing request/digest/file-count fields so an older V1 client can still read it. Omission counts are recorded separately in the descriptive audit event `workspace.export.policy`, correlated by `request_id` and `manifest_digest`; `omitted_entries` counts explicit archive headers, including directory entries. If an old export has no such event, its omission count is unknown. Preserve the original export request when a post-publication audit error leaves its outcome unresolved. Tool files such as `.cargo/config.toml` can remain: exported content still requires review before opening or executing it.

## Diagnose errors and preserve recovery evidence

The CLI prints JSON replies and exits with an error for `error` and `rejected`. Transport errors distinguish a request not sent from an unknown outcome or unavailable reply. Preserve operation/command identities before retrying; a nonzero client exit is not proof that no durable mutation occurred.

The runner's control HTTP client disables implicit retries, redirects and proxy
discovery. Its connection timeout is 3 seconds and each request is limited to
15 seconds. A lost response can follow a committed mutation: inspect durable
status and replay only the same admissible operation identity. These limits do
not provide an automatic restart, extend a worker deadline or reactivate control.

The direct runner (`run` / `worker-resume`) has a separate **read-only monitor**
after both Running and `command.started` are acknowledged. Typed connection
interruptions, bounded transport timeouts and HTTP 429 may be retried for at
most **60 seconds from the start of the first failing request, including its
timeout**. The original execution deadline wins; retries and `Retry-After`
cannot renew it. Visibility remains degraded/unknown. Authentication, identity,
revision, malformed-response and mandatory-audit errors, including unclassified
HTTP 503, still request an immediate targeted stop. Recovery requires the
complete current response for the same worker/attempt and revision floors,
followed by a durable recovery record. The start audit callback is excluded.
Before the first GET, durable epoch creation has its own **15-second arming
bound**, sampled before the writer starts and capped by the original execution
deadline. Expiry requests stop, without opening another 60-second interval.

This monitor's private epoch stays open until execution ends and its writes
are confirmed; a pending closure does not hold up execution finalization. An
interrupted write keeps its exact identity and is not assumed rolled back.
The separate journal is bounded to 64 intervals and 128 KiB of raw file bytes
before decoding, including whitespace; a write/capacity failure requests stop.
Preserve it. A restart with an open, absent or unverifiable
epoch cannot grant a fresh interval, even after a healthy last poll. For an
already-active direct session, `worker-resume` first verifies the private
tenant/origin, worker/attempt, lock, owned CID and unchanged runtime binding,
then requests stop of that exact instance without waiting for gateway health.
It performs no adoption, start or quota release. Repeat the same resume when
the gateway is available to reconcile terminal accounting; an unconfirmed stop
remains unknown. An explicitly requested replacement of a historical invocation
still uses the existing authenticated adoption path for a valid candidate:
gateway availability, the same engine and durable adoption audit remain required
before stopping the retained CID. The local stop port cannot adopt or turn an
identity/unsafe-file/engine conflict into permission. These rules do not relax broker RPC authentication or qualify
the independent guardian or real runtime stop guarantees. See the
[operator contract](../../operations.md#suivi-de-lecture-après-démarrage-acquitté).

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

Normal shutdown also waits for every accepted Full batch task before releasing
the broker lease. A completed task can still report `needs_reconciliation` or
leave audit events pending; its completion does not prove that a runtime has
stopped. An operation already in flight may prolong shutdown until its task
returns. There is no global abort deadline or qualified maximum shutdown time.
Keep the control process alive and preserve its lease during that wait. The
internal shutdown helper does not qualify real SIGTERM handling, process death
or ambiguous-start fencing.

Preserve the private registry, installation identity, broker journal, export-input records, configuration and credential bindings, gateway databases and coherent backups, retained runtime authorities and worker volumes together. Backup/restore must account for SQLite WAL state, confidentiality and the authority of outstanding claims. Restoring old files does not restore fresh execution permission. No automatic retention or safe rollback is established by this guide; follow [operations](../../operations.md) and the [journal/recovery contract](../../engineering/BROKER_JOURNAL.md) for the applicable boundaries.

The complete product also requires safe control reactivation, OS-managed lifecycle, ambiguous-start fencing, terminal continuity and real platform/failure qualification. Their current status and required evidence remain in [engineering](../../engineering/PRODUCTION_READINESS.md), including [the supervision contract](../../engineering/BROKER_SUPERVISION.md). Do not substitute the foreground commands or a configuration example for those results.
