# Workload identities, broker registry and directed relay

[Français](README.fr.md) · [Setup index](../README.md) · [Local broker](../broker/README.md) · [Named access](../identity/README.md) · [SSO](../sso/README.md)

This guide describes the Rust gateway's technical HTTP contracts for workload credentials, broker declarations and directed worker messages. They use one existing SQLite database per tenant. These endpoints do not start workers or connect remote brokers. Workers retain `network: "none"`; the private Unix/runtime transport that would connect them to this relay is not wired or qualified. Do not attach a worker to the control network to make these examples reachable.

Use the [engineering ledger](../../engineering/CURRENT_STATE.md), [validation](../../engineering/VALIDATION.md) and [gates](../../engineering/PRODUCTION_READINESS.md) for qualification. HTTP backend behavior alone does not establish the A1/A2/B1 three-worker recipe, federation, multiple hosts, complete human administration or measured capacity for 100 workers.

## Prepare storage without activating execution

Use the reviewed control binary, the existing tenant database and its private control account. Stop the gateway, all brokers and every other database consumer, including automatic restarts. Preserve their runtime/enforcement anchors; a stopped gateway does not prove a worker stopped. `--confirm-control-stopped` is an operator attestation, not runtime fencing.

| Schema | Reader and feature boundary |
|---|---|
| 2 | Standard local control; not a compatible supervised broker control. |
| 3 | Supervision; compatible broker protocol reader. |
| 4 | Human identity storage; compatible broker protocol reader. |
| 5 | Mandatory human-read audit; compatible broker protocol reader. |
| 6 | Workload bindings, broker registry, relay and durable UTC floor; compatible broker protocol reader. These workload contracts require this explicit migration. |
| 7 | Separate named human broker-registry projection and audit capability marker; explicit6→7, with no automatic grant or activation. |

Broker compatibility also requires the configured namespace, tenant, runner role and `broker_supervision_v1`. Fresh Submit, approve, resume and export require active supervision. **Migrating or reopening a store does not activate it.** Ordinary reopen of schemas 3–7 remains inactive maintenance; safe reactivation is not delivered. Do not edit metadata, delete storage or reuse exclusive fresh-store initialization to bypass that condition. See [active control and recovery](../broker/README.md#require-an-active-compatible-control).

Offline human maintenance (`human provision`, `human resume-provision` and `human apply`) accepts an existing validated schema 4–7 database with inactive supervision. After the explicit 5→6 migration, use the same private control filesystem, tenant binding and `--confirm-control-stopped` attestation. The commands do not create or migrate the database or reactivate supervision. Preserve private credentials and recovery files; resume only the exact retained preparation, without rotating its credential or overwriting its files. Follow [account provisioning and recovery](../identity/README.md#provision-an-account-and-retain-its-recovery) and keep the same audit capacity policy across consumers.

For an existing schema 5 store, replace every installation reference below. The backup destination must be new; its private parent must already exist. Migration verifies a backup and upgrades exactly one step. Its default target is still 2, so spell out 6:

```sh
"/absolute/path/to/agentsandbox" migrate-control \
  --data "/absolute/private/ai-guard/audit.sqlite" \
  --tenant-id "<configured-tenant-id>" \
  --backup "/absolute/private/ai-guard/before-workloads-v6.sqlite" \
  --target-version 6 --max-events 100000 --confirm-control-stopped
```

For older sources, apply each next migration with a separate new backup: 3→4→5→6, following [offline storage maintenance](../../operations.md#répertoire-sqlite-privé). Update all readers together; older binaries refuse newer schemas. Preserve tenant binding, namespace, events, consumed claims, credentials and verified backups. Never replace migration with a SQLite `user_version` edit. For container-owned storage, use the existing control UID/GID and its inspected existing data volume with other consumers stopped; the [identity guide](../identity/README.md#prepare-storage-and-authority) explains that maintenance boundary. Do not guess a volume name or initialize a replacement store.

## Enable named broker registry reads

Schema7 adds the separately authorized human projection of this existing registry. It uses the **same tenant SQLite database**, broker records and human audit capacity; it adds no worker transport or automatic broker registration. Keep gateway, broker/STOP readers, native clients and the built dashboard matched to the version accepting7. Those readers retain the historical5/6 contracts; older binaries that do not recognize7 cannot reopen it. The new capability is not granted by migration, defaults or historical preparation replay.

Storage from `setup prepare` is at5: first run the explicit5→6 command above, check its successful report and retain its verified schema5 backup. Then, with every consumer still stopped, migrate the **same** database6→7. Replace paths/tenant and use the installation's actual `--max-events`; do not rerun5→6 on a7 store:

```sh
"/absolute/path/to/agentsandbox" migrate-control \
  --data "/absolute/private/ai-guard/audit.sqlite" \
  --tenant-id "<configured-tenant-id>" \
  --backup "/absolute/private/ai-guard/before-human-brokers-v7.sqlite" \
  --target-version 7 --max-events 100000 --confirm-control-stopped
```

For container-owned data, follow the [identity guide's offline boundary](../identity/README.md#prepare-storage-and-authority): existing control UID/GID, inspected existing volume, no network/socket, matching image, all other consumers stopped. `/data/installation/tenants/<tenant>/audit.sqlite` applies to a prepared SSO volume rather than legacy `/data/audit.sqlite`; inspect and preserve the installation in use. Keep each backup in a new private path on that storage. Do not substitute a volume or infer its name from this example.

Migration creates and verifies a new backup of6 against its exact audit head, then atomically appends the hash-linked `control.identity.brokers_access.genesis` marker and changes the schema to7. It adds no tables/indexes, clears no budgets, rewrites no grants, issues no credentials and activates no execution. A failed final transaction leaves the source version/head unchanged; a created backup may remain and must be preserved. Check the exit and successful report's `from_version:6`, `to_version:7`, `backup_path`, `genesis_seq`, `genesis_hash` before continuing. No automatic restore/downgrade is supplied. Do not edit `user_version`, markers, receipts or projections to bypass a refusal.

Ordinary cold reopen remains inactive maintenance at7. New admissions, arms and claims remain refused; this procedure provides no supervision reactivation or workaround for unresolved fencing/recovery. Retain runtime/enforcement anchors and consumed claims. Reopening for reads is not authority to launch a replacement worker.

Next explicitly [grant `read_broker_registry`](../identity/README.md#authorize-named-broker-registry-readers) through trusted control/offline maintenance. OIDC also needs its [verified-claim ceiling](../sso/README.md#opt-in-to-broker-registry-reads). `setup prepare` still prepares5 and refuses this capability before publishing an installation; do not add it to the setup manifest. Keep `--max-events` and the intended audit policy consistent across gateway and offline identity consumers. `migrate-control` has no `--audit-capacity-file` flag; that option belongs to gateway/offline identity commands. Migration consumes mandatory identity audit capacity and cannot bypass a full ledger.

The human route is `GET /api/v1/human/brokers`, with an admitted local/OIDC proof and an administration context. It differs from technical `GET /api/v1/brokers`: no raw grants or credential IDs/digests go to the browser. The [API contract](../../api.md#registre-brokers-humain--human-broker-registry) defines the closed query/response. Rows appear only after real authorized control registration; use the issue/grant/heartbeat contracts below for the actual broker concerned. Do not post fixture observations or synthetic audit events to populate a dashboard. This read view supplies no runtime inventory, federation or production qualification.

## Set a bounded workload audit policy

[workload-audit.json](examples/workload-audit.json) is a public, secret-free V1 example for `--max-events 100000` with the default control/identity reserves. Copy it into a **new** private regular file, mode 0600 under a private 0700 control parent outside source/export trees. The policy file is limited to 16 KiB and loaded at startup; editing it is not hot reload. For the local single-tenant gateway, add `--workload-audit-file /absolute/private/ai-guard/workload-audit.json` to its existing launch command. This is a gateway option, not a broker registry field or an environment selector.

The option conflicts with `--tenants-file`. With a tenant registry, place the same JSON object in the relevant tenant entry's optional `workload_audit_policy` field instead. Keep its existing `data`, credential references, quota and optional `audit_capacity_policy`; neither a public example nor a request body selects the tenant. Use consistent `--max-events` and capacity policies across consumers.

| Budget in the supplied example | Events | Prepared audit payload bytes |
|---|---:|---:|
| All workload audit in the tenant | 25000 | 67108864 (64 MiB) |
| One broker or worker principal | 6250 | 16777216 (16 MiB) |
| Existing default control reserve | 25000 | 67108864 (64 MiB) |
| Existing default identity reserve | 12500 | 33554432 (32 MiB) |

Both dimensions apply. A principal budget must be positive and strictly smaller than the tenant budget. The tenant workload budget must fit within physical capacity after the existing control and identity reserves. Physical prepared payload capacity is 256 MiB; `--max-events` bounds events. These are shared occupancy checks, not independent storage partitions: existing ordinary and human-read events consume space too. The example does not prove free capacity or guarantee an operation can be recorded.

Workload usage is cumulative and durable, keyed by broker ID or worker ID within its tenant. Heartbeats, sends, ACKs and inbox read receipts consume their applicable workload budget; an empty inbox read also records a receipt. Rotation, a new credential, reconnect, restart or a policy change does not reset that principal's usage. Defaults derive from the existing reserves; insufficient automatic capacity disables new workload audit admission explicitly. Startup diagnostics report the resolved policy, source, fingerprint and admission-budget state, not a promise of remaining capacity.

On `429 audit_workload_budget_exhausted` with `retry_action:"contact_operator"`, preserve database, policy and receipts, and inspect occupancy. Repeated timer retries or rotation cannot recover this cumulative budget. A deliberate policy increase helps only within physical capacity and protected reserves. No automatic archival, purge or qualified archive procedure is supplied. Do not delete audit events to free capacity. Physical exhaustion or unavailable mandatory storage remains an error.

## Keep four technical roles separate

| Proof | Permitted scope here |
|---|---|
| `runner` | Trusted control only: issue/rotate/revoke bindings, manage broker grants and directed relay grants, prune expired message bodies. |
| `viewer` | Read broker metadata for its authenticated tenant; no workload administration or relay body access. The runner can also use this technical read route. |
| `broker` workload | Read its own broker entry and submit its own authenticated heartbeat under a current binding and grant. No runner, registry-wide or worker-message rights. |
| `worker` workload | Send under an explicit source→destination/channel grant, read its own inbox and ACK its own messages. No control registry or global metrics rights. |

The gateway derives tenant and role from the presented Bearer proof. There is no free tenant header, query selector or `tenant_id` in these workload DTOs. JSON IDs name resources inside an already authenticated scope; they are not identity proofs. A worker Bearer on a runner-only route does not acquire runner authority.

Human local credentials and OIDC sessions remain separate, as do AI-provider credentials. Neither an IdP administrator nor an ai-guard application role creates a runner or workload credential. These APIs do not establish a named human initiator or authenticate a complete agent/delegation chain. Follow [named access](../identity/README.md) or [SSO](../sso/README.md) for human access.

## Prepare and record a credential operation from the CLI

Use the Rust operator CLI from the trusted control environment. Build the
current `agentsandbox` binary, prepare storage schema6 or7 as above and keep
the current runner credential in a private file. The command does not obtain
runner authority from a human account or an IdP role.
The runner file uses the current gateway CLI profile:32–512 bytes without
whitespace, optionally followed by one LF. The operation never replaces or
regenerates that existing runner credential.

Copy the [control template](examples/cli-control.json) and one request
template into an operator-owned private directory. The examples are inputs
to configure, not installed identities: replace every `<...>` and example
origin/path. Use the actual authenticated tenant and control namespace from
the approved installation profile. These are equality guards; the server
still chooses the tenant from the runner credential.

| Request template | Values to prepare |
|---|---|
| [Issue](examples/cli-issue.json) | Approved broker/incarnation, future expiry, revision0. The CLI generates the credential ID and secret once. |
| [Rotate](examples/cli-rotate.json) | Same credential ID and subject, explicit current revision and new future expiry. A new secret is generated. |
| [Revoke](examples/cli-revoke.json) | Credential ID and explicit current revision. No new secret is generated. |

For a worker subject, use its approved `worker_id`, `attempt_id` and
`generation:1` under `kind:worker`, matching the existing authorized runtime
attempt. A supplied ID does not establish that attempt or start a worker.
Do not put a tenant, raw key, digest, actor or operation ID in the request
template; unknown fields are refused.

Keep both input and control files private, with existing trusted parents
and mode0600. Keep the operation directory outside projects and directories
imported into workers. Use HTTPS with normal chain/name verification, or a numeric
loopback HTTP origin for local control. For a private CA, set
`ca_bundle_file` to the absolute private-file path of the approved public
certificate bundle, at most64KiB/eight certificates. These explicit roots
replace built-in roots. Never include a CA private key or disable TLS.

The following paths are operator-selected examples. The parent must already
be private; the operation directory must be new:

```sh
AIGUARD_BIN=/absolute/path/to/agentsandbox
AIGUARD_PRIVATE=/absolute/private/operator
"$AIGUARD_BIN" workload prepare \
  --input "$AIGUARD_PRIVATE/issue.json" \
  --control-file "$AIGUARD_PRIVATE/control.json" \
  --output-dir "$AIGUARD_PRIVATE/broker-key-issue"

"$AIGUARD_BIN" workload status \
  --operation-dir "$AIGUARD_PRIVATE/broker-key-issue"

"$AIGUARD_BIN" workload apply \
  --operation-dir "$AIGUARD_PRIVATE/broker-key-issue" \
  --control-file "$AIGUARD_PRIVATE/control.json"
```

`prepare` performs no HTTP. It retains the exact request and, for issue or
rotation, a random credential in the operation's private `credential` file
before any remote mutation. It never prints that secret. Container users
can use `--input -` with stdin instead of a host bind; input remains bounded
to16KiB/five seconds, and control/destination files retain their private
filesystem requirements.

`apply` validates the current explicitly supplied runner, expected origin,
tenant and namespace, then makes one mutation request. There is no automatic
retry. Success reports a recorded historical operation and its durable
receipt, with `current_state:not_observed`; it does not assert that the key
is still active. A binding alone neither grants a broker nor authorizes a
relay channel, distributes a key or starts/stops a worker. Keep these
operations separate under the grants described below. Never mount the
whole operation directory or a runner/SSO credential in a worker.

### Resume, rotate and revoke without discarding state

After a lost response, preserve the whole private operation directory.
Resume that operation explicitly instead of preparing a replacement:

```sh
"$AIGUARD_BIN" workload resume \
  --operation-dir "$AIGUARD_PRIVATE/broker-key-issue" \
  --control-file "$AIGUARD_PRIVATE/control.json"
```

Resume retains IDs, bytes, secret, expiry and CAS. A now-expired request can
still recover an earlier receipt. The current control file may reference a
replacement authorized runner or approved CA bundle, but origin, tenant and
namespace must match the preparation. A preflight followed by a POST is not
an atomic server scope comparison during concurrent trusted-control
reconfiguration; preserve the installation's routing/credential assignment
while performing the operation.

For rotation or revocation, review the corresponding template, set the
actual expected revision, and run prepare/apply with a **different new**
operation directory. A local receipt is historical and cannot discover the
current revision. A409 conflict must be investigated; do not guess a higher
CAS or edit a retained request. Distribute a rotated key only through the
approved consumer configuration and preserve the old operation for audit.
Revocation records credential refusal, not termination of a running process.

Partial preparation is retained and cannot be adopted or repaired by editing
JSON. An apply/resume attempt with an unknown result fails until a validated
receipt is durable locally. `status` can successfully display that retained
local state while leaving the remote outcome explicitly unresolved.
No secret, existing operation, database or partial receipt
is deleted automatically. Retained staging is bounded; an exhausted or
corrupt operation needs operator diagnosis, without deleting evidence to
obtain a successful status. On quota/clock/storage rejection, preserve state
and use the diagnostics below before deliberately resuming.

## Issue, rotate and revoke through the HTTP API

The CLI above handles durable preparation for operators. An authorized integration caller may also prepare a high-entropy private Bearer credential, compute its SHA-256 digest locally and submit **only the digest** in the issue DTO. The gateway does not generate or return the raw credential. Authentication later presents that credential privately over the approved control transport; do not put it in argv, public JSON, logs or screenshots. Keep its source as a private file reference in the trusted caller. No runner, SSO token, engine socket or complete environment belongs in a worker handoff.

All routes below are under `/api/v1`. JSON objects are closed: unknown fields are refused. The gateway's request-body limit is 256 KiB, including JSON escaping/envelope; a body's own limit does not override it. Use `version:1`, canonical lowercase non-nil UUIDs, valid future RFC3339 expiries and SHA-256 hex digests. Workload Bearers must be 32–512 bytes without whitespace. The following is a **request template**, not an installed identity. Replace every `<...>` reference with fresh IDs, the real prepared digest and an intended expiry; do not send placeholders:

```json
{
  "version": 1,
  "operation_id": "<new-operation-uuid>",
  "credential_id": "<new-credential-uuid>",
  "expected_revision": 0,
  "subject": {
    "kind": "broker",
    "broker_id": "<broker-uuid>",
    "incarnation_id": "<new-incarnation-uuid>"
  },
  "sha256_digest": "<sha256-of-private-credential>",
  "expires_at": "<future-rfc3339-time>"
}
```

| Method and path | Runner request fields / result |
|---|---|
| `POST /workload-bindings` | `version`, `operation_id`, `credential_id`, `expected_revision`, `subject`, `sha256_digest`, `expires_at`. New binding uses revision 0; rotation/renewal uses the exact current revision. Returns 201, or 200 on exact replay. |
| `POST /workload-bindings/{id}/revoke` | `version`, new `operation_id`, `credential_id` matching `{id}`, positive `expected_revision`. Returns the durable receipt. |

The alternative worker subject is `{"kind":"worker","worker_id":"<worker-uuid>","attempt_id":"<attempt-uuid>","generation":1}`. It must refer to the existing authorized worker attempt; issuing a key does not create or launch it. V1 has no generation-transition protocol: rotation cannot change the worker, attempt or generation. A binding cannot change its subject. At most 64 credential IDs are retained per subject; rotation of the same binding preserves its credential ID and advances its revision.

Record each new `operation_id` and exact request before transmission. An exact retry returns the original receipt; different durable content under the same operation ID conflicts. Receipts carry `version`, `credential_id`, `revision`, `audit_event_id`, `audit_seq`, `audit_hash` and `replayed`. A receipt proves durable control recording, not credential delivery or execution.

Rotation requires a new unused digest; the replaced raw key stops authenticating. Historical digests remain ownership evidence and cannot be reintroduced after rotation, revocation or expiry. Only renewal of the still-current active key may reuse its digest. Revocation requires the current revision and advances it. Do not reuse another tenant's, human's or control key; ambiguous ownership fails closed. Preserve private preparation, exact request and receipt after a lost response rather than issuing another identity blindly.

## Grant a broker and interpret its heartbeat

After preparing the broker binding, a runner grants that broker's authority scope. This registry is distinct from the local `broker serve --config` Registry2. An authority object has the form `{"authority_id":"<approved-authority-id>","runtime_kind":"docker"}`. Its `authority_id` is a bounded label (1–256 bytes, no controls), paired with `runtime_kind:docker|podman|kubernetes`. It is not a runtime connection or an installed Kubernetes adapter. Historical authority ownership cannot be transferred to another broker or runtime kind by editing a grant.

| Method and path | Proof and closed request |
|---|---|
| `POST /brokers/grants` | Runner: `version`, `operation_id`, `broker_id`, `expected_revision` (`null` initially, exact positive current grant revision afterward), `credential_id`, `incarnation_id`, `authorities` (1–16 unique authority IDs), `lease_seconds` (1–300), future `not_after`. Serialized grant ≤16 KiB. Returns 201, or 200 on replay. |
| `POST /brokers/{id}/revoke` | Runner: `version`, `operation_id`, `broker_id` matching `{id}`, positive `expected_revision`. |
| `POST /brokers/heartbeat` | Broker: `version`, `operation_id`, `expected_heartbeat_revision`, `incarnation_id`, `sequence`. |
| `GET /brokers/self` | Broker: no body or scope selector; returns its own `BrokerViewV1`. |
| `GET /brokers?limit=50` | Technical viewer or runner: limit 1–100, default 50; optional `cursor` is the exact broker UUID returned as `next_cursor`. Returns `version`, `items`, `next_cursor`. |

A new grant uses a previously unused incarnation UUID and a matching broker binding. Replacing a grant also requires a new incarnation and matching subject/binding; rotating a credential alone does not invent a new incarnation. Compare the durable grant revision, not a cached guess. A heartbeat carries the exact current heartbeat revision, the grant's incarnation and the next sequence (first 1, then previous+1). Heartbeat replay still requires a current authenticated binding and unexpired/unrevoked grant; a previous receipt does not restore authority. Runner mutations and heartbeat receipts contain broker metadata, grant/heartbeat revisions, operation/input hash and audit sequence/hash, with `replayed` explicitly reported.

`last_heartbeat.heartbeat_provenance:"declared"` describes the broker assertion. `reception_provenance:"observed"` and `received_at` describe the gateway's reception, not a runtime effect. `lease_expires_at` is bounded by both reception plus the configured lease and `not_after`. Views expose `state` (`unseen`, `current`, `stale`, `expired`, `revoked`, `clock_uncertain`), `evaluated_at` and `runtime_inventory_complete:false`. A stale broker means its lease is no longer current; it does not mean zero workers, stopped workers, released quota or a complete runtime inventory. Preserve last known facts and investigate the same authority.

## Authorize directed messages

A runner explicitly grants one source endpoint → one different destination endpoint + channel inside its tenant. Same tenant does not imply permission; reverse traffic needs another grant. Endpoints have `worker_id`, `attempt_id`, `generation:1`. Both must have current valid workload bindings and be recorded Running with a container identity and held quota under active control. These checks do not themselves supply the missing runtime transport.

| Method and path | Proof and closed request |
|---|---|
| `POST /relay/grants` | Runner: `version`, `operation_id`, `grant_id`, `source`, `destination`, `channel`, `expires_at`. Channel1–64 ASCII alphanumeric/`-_.`; expiry after now and within 24 hours. Returns 201, or 200 on replay. |
| `POST /relay/grants/{id}/revoke` | Runner: `version`, `operation_id`, `grant_id` matching `{id}`. Revocation also blocks delivery of already queued content. |
| `POST /relay/messages` | Source worker: `version`, `operation_id`, `grant_id`, `ttl_seconds`, `body` (a string). No tenant/source selector. Returns 201, or 200 on replay. |
| `GET /relay/messages?limit=32` | Destination worker: limit 1–32, default 32; optional nonnegative `after` uses the returned `next_after`. Returns `version`, `items`, `next_after`. |
| `POST /relay/messages/{id}/ack` | Destination worker: `version`, `operation_id`, `message_id` matching `{id}`. |
| `POST /relay/messages/{id}/prune` | Runner: `version`, `operation_id`, `message_id` matching `{id}`; only after message expiry. Removes the body, retaining metadata/tombstones and audit. |

For example, the **public send DTO template** is `{"version":1,"operation_id":"<new-operation-uuid>","grant_id":"<authorized-grant-uuid>","ttl_seconds":60,"body":"Public example: report is available."}`. Replace IDs before use. An ACK or prune request uses its own new operation UUID; `message_id` comes from the send receipt's `resource_id` or the inbox metadata. Relay receipts contain `version`, `operation_id`, `resource_id`, `audit_seq`, `audit_hash`, `replayed`.

| Relay bound | Current limit |
|---|---|
| Nonempty UTF-8 body | 65536 bytes (64 KiB), before JSON escaping |
| Message TTL | 1–3600 seconds, additionally capped by grant expiry |
| New sends per grant | 60 in the preceding 60 seconds, durably checked |
| Retained bodies per tenant | 1024 messages and 16 MiB, including acknowledged/expired bodies until explicit prune |
| Inbox page | At most 32 items and 2 MiB encoded JSON budget |

Bodies are tenant business data stored in its database. Audit records message metadata, hashes and sizes, never a duplicate of the body. Known-secret and structured redaction checks reject bodies that would need masking, but this is not permission to send secrets or a guarantee to recognize every secret. Do not put credentials or sensitive tokens in messages.

Inbox reads are audited durably before content leaves control. A page includes each message's `sequence`, `metadata` and `body`; metadata identifies both endpoints/channel, expiry, body hash/size and ACK/prune state. Continue with the returned cursor under the same identity; a page is not a snapshot or total. Delivery rechecks the grant and current endpoint authority. Expired/revoked grants and revoked or terminated endpoints stop delivery of queued messages too. An ACK records the recipient's acknowledgement, **not execution or an effect**, and does not prune the retained body.

Record exact operation IDs and requests for retries. Idempotence is not a permission bypass: send/ACK replays revalidate the current worker proof, grant and endpoints. A rotated binding with the same credential ID preserves that identity, but a different credential ID changes send/ACK identity and cannot assume its replay. Grant replay also requires currently authorized endpoints; runner revoke/prune retain their own exact replay rules. Do not use an old receipt to bypass current refusals. No automatic regrant, restart or cross-tenant routing occurs.

## Diagnose, stop and preserve state

| HTTP result | Interpretation and next action |
|---|---|
|401 | Missing, ambiguous header, invalid, expired or revoked Bearer proof. Workload authentication uses `authentication_required`; a worker proof on a runner-only route is refused by the legacy control-key guard, whose error may be a plain JSON string without that code. Check the private file and route's required role without printing the key. |
|403 `workload_forbidden`, `relay_forbidden` or `relay_expired` | Proof, scope, grant, endpoint or expiry does not authorize this exchange. Correct authority through the trusted control, without broadening network access. |
|400 invalid workload/relay request | Check closed fields, UUIDs, path/body IDs, revisions and limits. |
|409 `workload_conflict`, `broker_conflict`, `relay_conflict` | CAS revision or operation content differs, or an immutable identity/authority would change. Preserve and compare the original request/receipt. |
|429 `audit_workload_budget_exhausted` / `audit_write_budget_exhausted` | Cumulative capacity refusal with `retry_action:"contact_operator"`; preserve state and inspect occupancy/reserves. |
|429 `relay_rate_limited`, `relay_capacity_exhausted`, `issuance_busy`, I/O admission refusal | Respect the applicable rate/concurrency bound; retained bodies require deliberate expired-body prune. Retry only the same intended operation after resolving the cause. |
|503 `control_clock_uncertain` | UTC precedes durable authorization evidence; `retry_action:"contact_operator"`. Preserve state and repair/reconcile the trusted clock. Never treat the result as an empty inbox or no brokers. |
|503 identity/storage/schema unavailable | Check schema 6/7, unambiguous credential ownership, integrity and required storage. An unavailable tenant source is not proof of absence; no identity substitution or viewer fallback. |

The schema6/7 UTC floor is derived from durable audit reception evidence. Authorization/expiry comparisons use millisecond UTC precision; this does not claim detection of cross-request rollback within one millisecond, trusted SSH/SSO time, or a general clock guarantee. Actual reception timestamps remain actual observations; the floor does not fabricate a later `received_at`. A clock refusal does not authorize editing timestamps or durable floor tables.

Before maintenance, revoke intended relay/broker authorities and workload bindings through their separate authorized operations, retain requests/receipts, and stop jobs through the [broker's supported procedure](../broker/README.md#stop-restart-and-maintain-the-installation). Revocation is not a runtime stop observation or a quota release. Preserve existing enforcement even when control becomes unavailable. Restart only the same intended installation after checking ownership, storage and clock; budgets, revocations and historical key ownership survive restart. Keep private backups, bindings, broker journal, tenant data and runtime evidence together. No command here supplies federation, automatic service installation, safe supervision reactivation or destructive cleanup.

The exact DTOs and limits are in [workload.rs](../../../src/workload.rs), [broker_registry.rs](../../../src/broker_registry.rs) and [relay.rs](../../../src/relay.rs); routes are in [api/workload.rs](../../../src/api/workload.rs), [api/brokers.rs](../../../src/api/brokers.rs) and [api/relay.rs](../../../src/api/relay.rs). Policy consumers are [gateway_config.rs](../../../src/gateway_config.rs), [main.rs](../../../src/main.rs) and [audit/capacity.rs](../../../src/audit/capacity.rs).
