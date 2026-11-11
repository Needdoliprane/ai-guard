# Relay retention budgets

[Français](RETENTION.fr.md) · [Relay routes and contract](../workloads/README.md#authorize-directed-messages) · [Rust worker relay client](README.md)

The relay keeps every accepted message body in the tenant database until that
body is removed. This guide explains how much a tenant, one sender and one
directed grant may keep retained, and what a sender is told when a bound is
reached. It describes delivered software behavior. It does not qualify a real
worker transport, a container runtime or the 100-worker target.

## What counts as retained

A retained body is a message whose `relay_messages.body` column is not `NULL`.
It counts from the accepted send until that column is set to `NULL`, **including after
acknowledgement and after expiry**. Metadata, tombstones and audit facts remain
after removal and do not count.

These budgets count retained message rows and their content bytes. They are
not disk-space or inode quotas: SQLite pages, indexes, tombstones, audit
history and the WAL also consume storage. Setting a body to `NULL` does not
promise that file size shrinks or that old content is securely erased. Size
and filesystem capacity require separate operator monitoring and limits.

Usage is read from the retained rows inside the SQLite transaction that admits
the send. There is no separate counter to reconcile. An exact replay of an
accepted send returns its receipt and consumes nothing, even when the sender is
full. Concurrent sends are serialized by that transaction: a share is never
exceeded by a race.

## Three scopes

| Scope | Bodies counted | Shipped limit |
|---|---|---|
| Tenant | Every retained body of the tenant | 1024 messages and 16 MiB |
| Sender | Bodies sent by one source worker, over all its grants | 128 messages and 2 MiB |
| Grant | Bodies sent over one directed grant (source → destination + channel) | 128 messages and 2 MiB |

Every scope is bounded both in messages and in bytes. A send is checked against
the sender, then the grant, then the tenant; the first exceeded scope is the
one reported. The sender is identified by the authenticated workload proof. No
request field selects a sender, a grant owner or a tenant.

A share is a **ceiling, not a reservation**. For new sends admitted under the
shipped limits, one sender alone cannot exhaust its tenant: at least eight
senders must contribute. Past `tenant ÷ sender` saturated senders, a sender
still under its own share can meet the tenant ceiling.

The grant share equals the sender share by default, so it changes nothing until
it is tightened. A tighter grant share keeps one slow destination or channel
from consuming the whole share of its sender.

The shipped limits apply to every tenant unless the operator
[configures a policy](#configure).

## Configure

The policy is one closed JSON object. Every field is required; an unknown or
missing field is refused. [relay-retention.json](examples/relay-retention.json)
is a public, secret-free example equal to the shipped policy:

```json
{
  "version": 1,
  "tenant": {"messages": 1024, "bytes": 16777216},
  "sender": {"messages": 128, "bytes": 2097152},
  "grant": {"messages": 128, "bytes": 2097152},
  "automatic_prune_batch": 8,
  "release_on_ack": false
}
```

| Field | Accepted values |
|---|---|
| `version` | `1` |
| `tenant` | At most 1024 messages and 16777216 bytes, and at least twice the sender share in each dimension |
| `sender` | At least 1 message and 65536 bytes, so one maximum-size message fits an empty share |
| `grant` | At least 1 message and 65536 bytes, and no more than the sender share |
| `automatic_prune_batch` | 0 to 32; 0 disables automatic release |
| `release_on_ack` | `false`, or `true` after reading [its condition](#release-of-retained-bodies) |

For a strict guarantee that no sender under its share ever meets the tenant
ceiling, set `sender` to `tenant ÷ (largest number of sending workers)`.

- **Tenant registry.** Put the object in the optional `relay_retention_policy`
  field of the tenant entry of `--tenants-file`. Each tenant has its own policy;
  an absent field means the shipped policy.
- **Local single-tenant gateway.** Copy the object into a new private regular
  file, mode 0600 under a private 0700 control parent, at most 16 KiB, and add
  `--relay-retention-file /absolute/private/ai-guard/relay-retention.json` to the
  existing gateway command. The option conflicts with `--tenants-file`.

Every policy of the registry is validated before the first tenant database is
opened or created; an invalid policy stops the gateway without touching any
store. At startup the gateway prints one line per tenant:

```text
Relay retention tenant=<id> source=derived_defaults|tenant_registry|private_policy_file release_on_ack=<state> policy=<json> fingerprint=<64 hex>
```

The policy is read at startup only; editing the file or the registry is not a
hot reload. A tightened policy never deletes or invalidates content: senders
above their new share are refused new sends until bodies are released, and the
[diagnostic](#diagnose) shows `retained` above `limit`.

A restarted gateway reopens its stores with supervision inactive. Until
supervised activity resumes through the trusted control, worker sends are
refused, so the new policy governs no traffic and automatic release does not
run; the diagnostic and the runner prune stay available.

## Release of retained bodies

A body stops counting when its column is set to `NULL`. Removal keeps the message
metadata and writes a metadata-only `prune` audit fact; the body itself is never
copied into the audit.

**Automatic release on send.** Each authorized send first releases expired
bodies, at most 8 per send (`automatic_prune_batch`, 0 to 32): the sender's own
expired bodies first, then the oldest expired bodies of the tenant, so bodies
left by a departed sender are drained by the traffic of the others. A body is
only released at or after its `expires_at`; there is no grace period.

- Every release is one audit fact with no actor, dated at or after the expiry.
  Its `correlation_id` is `relay-auto-prune:<operation_id>:<worker_id>`: the
  send operation that triggered the release and the authenticated worker that
  sent it. This is provenance, not attribution: that worker need not own the
  released body, which the fact names in its own metadata. A runner prune never
  carries that prefix.
- A release is kept even when the send that triggered it is then refused. The
  triggering operation may therefore be absent from the audit. A retry of the
  same operation continues the release instead of repeating it.
- Automatic release is admitted in the ordinary audit class. It never consumes
  the control or identity audit reserves. When the ordinary audit budget is
  exhausted, automatic release stops and nothing is removed without its fact.
- Release only happens inside an authorized send. An idle tenant keeps its
  expired bodies, and so does a gateway that was restarted: a reopened store
  admits no worker send until supervision is active again.

**Release on acknowledgement (opt-in).** By default an acknowledgement records
receipt and keeps the body until it expires. With `release_on_ack: true` the
recipient's acknowledgement fact also removes the body: one fact, attributed to
the recipient, and no separate prune. Unacknowledged bodies still wait for
their expiry, and an expired body can no longer be acknowledged: with
`automatic_prune_batch: 0` it then stays until a runner prune. This is a
**one-way door**: a database holding such a fact is
refused by gateway binaries that predate this option. Take a backup before
enabling it; going back means restoring that backup or keeping this revision.
The startup line states `release_on_ack=enabled_history_unreadable_by_earlier_binaries`.

**Deliberate prune.** The runner can remove one expired body with
[`POST /relay/messages/{id}/prune`](../workloads/README.md#authorize-directed-messages).
It works with automatic release disabled, with an idle tenant, after a restart
and when the ordinary audit budget is exhausted, because it uses the control
audit reserve. Replaying the same prune returns its receipt. Pruning an already
released body again frees nothing twice: capacity is counted from the rows.

## Refusals

A send beyond a bound is refused with HTTP 429. The message is not stored and
no send fact is written. The only thing a refused send can leave behind is the
bounded release of expired bodies described above, each with its own audit
fact.

| `error.code` | Meaning | Added fields |
|---|---|---|
| `relay_sender_capacity_exhausted` | The authenticated sender reached its own share. | `scope:"sender"`, `dimension`, `limit`, `retained`, `requested_bytes`, `next_release_at`, `retry_action` |
| `relay_grant_capacity_exhausted` | This directed grant reached its share; another grant of the same sender may still have room. | Same fields, `scope:"grant"` |
| `relay_capacity_exhausted` | The tenant ceiling is reached although this sender is under its own shares. | `scope:"tenant"` and `retry_action` only |
| `relay_rate_limited` | More than 60 sends on this grant in the last 60 seconds. | `scope:"grant"`, `retry_action:"retry_after_window"` |

`dimension` is `messages` or `bytes`. `limit` and `retained` are
`{"messages":…,"bytes":…}` for the reported scope, measured after the release
pass of this attempt. `next_release_at` is the earliest expiry among the bodies
still retained in that scope, or `null`.

| `retry_action` | What to do |
|---|---|
| `retry_after_release` | The policy allows capacity to be freed. Try the same operation again after a release. |
| `retry_after_window` | Retry the same operation once the 60-second rate window has moved on. |
| `contact_operator` | Ask the operator to inspect usage, prune or change the policy. At the tenant ceiling with no automatic pruning this advice is deliberately pessimistic. |

Capacity advice follows the configured policy and, for a sender or grant,
the bodies held in that scope.

| `automatic_prune_batch` | `release_on_ack` | What frees the refused scope | Answer |
|---|---|---|---|
| above 0 | `false` | Sends release its bodies once they have expired. | `retry_after_release` |
| above 0 | `true` | The same, and the recipient's acknowledgement frees a body before its expiry. | `retry_after_release` |
| 0 | `true` | Only the acknowledgement of a body that has not expired. Sends release nothing, and an expired body can no longer be acknowledged. | Sender/grant: `retry_after_release` while the own scope holds a body that is neither expired nor acknowledged, `contact_operator` otherwise. Tenant: always `contact_operator`. |
| 0 | `false` | Nothing automatic. | `contact_operator` |

The advice is indicative. It stays the same when the release pass stops for
lack of ordinary audit capacity. The gateway blocks a release or send whenever
its required audit fact cannot be written. The runner can explicitly prune an
expired body if the control audit reserve permits the write.

- **Sender or grant refusal, `automatic_prune_batch` above 0.** Retry at or
  after `next_release_at`. A value in the past means expired bodies remain.
  A retry can run another bounded release pass if audit capacity is available.
- **Tenant refusal, `automatic_prune_batch` above 0.** No `next_release_at` is
  given: retry later, after a delay of your choice. The bodies of the other
  senders expire within their TTL, at most 3600 seconds. Authorized sends can
  then release them subject to the configured batch and required audit.
- **Sender or grant, `automatic_prune_batch: 0` and `release_on_ack: true`.** `next_release_at`
  is an expiry, not a release time: expiry releases nothing. A retry can pass
  once acknowledgements have freed enough capacity in the refused scope. When
  every body of that scope has expired or is already acknowledged, the answer
  becomes `contact_operator` and only a runner prune frees the scope.
- **Tenant refusal, `automatic_prune_batch: 0`.** Always contact the operator,
  regardless of `release_on_ack` and of live, expired or acknowledged bodies.
  An acknowledgement may still free capacity, but the response does not reveal
  whether another sender has a body that could be acknowledged.
- In acknowledgement-only mode, `retry_after_release` can persist until a
  body expires after its grant has been revoked or its recipient has stopped.
  The advice does not verify the recipient's current ability to acknowledge.

A sender only ever receives figures about its own scope. A tenant-scope refusal
carries no occupancy and no deadline, because those would describe other
senders. Its `retry_action` is based only on the automatic prune setting.
No refusal contains a message body.

`scope` is always one of `tenant`, `sender` and `grant`, and `retry_action` one
of the three values above; the Rust contracts `RelayRefusalScopeV1` and
`RelayRetryActionV1` name exactly these values.

Example of a refused send:

```json
{"error":{
  "code":"relay_sender_capacity_exhausted",
  "message":"This sender reached its retained relay share.",
  "scope":"sender","dimension":"bytes",
  "limit":{"messages":128,"bytes":2097152},
  "retained":{"messages":32,"bytes":2097152},
  "requested_bytes":65536,
  "next_release_at":"2026-11-10T12:00:42.000Z",
  "retry_action":"retry_after_release"
}}
```

Keep the exact refused request. Retry that same operation, with the same
`operation_id` and content, once capacity has been released. A new operation ID
for the same business message would be a second message.

## Diagnose

`GET /api/v1/relay/retention?limit=32` returns the retained picture of the
authenticated tenant to its runner or viewer credential. A worker proof is
refused with 401. The tenant comes from the credential; the only query
parameter is `limit`, 1 to 100 (default 32), and anything else is refused with
400 `invalid_relay_request`. The read writes no audit event, returns no message
body and works while supervision is inactive, as after a restart.

Keep the credential out of the command line. With a private file containing
the single line `Authorization: Bearer <viewer credential>`:

```sh
curl --fail-with-body --silent --show-error --max-time 30 --noproxy '*' \
  --header @/absolute/private/ai-guard/viewer-authorization.header \
  'http://127.0.0.1:8787/api/v1/relay/retention?limit=32'
```

```json
{
  "version": 1,
  "observed_at": "2026-11-10T12:00:00.000Z",
  "policy": {"version": 1, "tenant": {"messages": 1024, "bytes": 16777216}, "sender": {"messages": 128, "bytes": 2097152}, "grant": {"messages": 128, "bytes": 2097152}, "automatic_prune_batch": 8, "release_on_ack": false},
  "policy_fingerprint": "<64 hex>",
  "tenant": {
    "limit": {"messages": 1024, "bytes": 16777216},
    "retained": {"messages": 3, "bytes": 196608},
    "expired": {"messages": 1, "bytes": 65536},
    "acknowledged": {"messages": 1, "bytes": 65536},
    "next_expiry_at": "2026-11-10T11:59:42.000Z"
  },
  "senders": [{"worker_id": "<uuid>", "limit": {"messages": 128, "bytes": 2097152}, "retained": {"messages": 3, "bytes": 196608}, "expired": {"messages": 1, "bytes": 65536}, "next_expiry_at": "2026-11-10T11:59:42.000Z"}],
  "senders_total": 1,
  "grants": [{"grant_id": "<uuid>", "source_worker_id": "<uuid>", "destination_worker_id": "<uuid>", "channel": "tasks", "limit": {"messages": 128, "bytes": 2097152}, "retained": {"messages": 3, "bytes": 196608}, "expired": {"messages": 1, "bytes": 65536}, "next_expiry_at": "2026-11-10T11:59:42.000Z"}],
  "grants_total": 1,
  "prunable": [{"message_id": "<uuid>", "grant_id": "<uuid>", "source_worker_id": "<uuid>", "destination_worker_id": "<uuid>", "body_bytes": 65536, "expires_at": "2026-11-10T11:59:42.000Z", "acknowledged": false}],
  "prunable_total": 1
}
```

- `retained` counts non-`NULL` body columns. `expired` and `acknowledged` are
  subsets of it, evaluated at `observed_at`.
- `senders` and `grants` list only scopes holding at least one retained body,
  largest retained bytes first, then by identifier. `prunable` lists expired
  bodies still retained, oldest expiry first: each `message_id` can be given to
  the runner prune.
- Each list holds at most `limit` rows. Compare its length with `senders_total`,
  `grants_total` or `prunable_total`: a larger total means the list is
  truncated, not that the rest is empty.
- `policy` and `policy_fingerprint` are the policy this gateway process applies.
- A 429 `capacity_exhausted` or a 503 is an error to retry or investigate, never
  an empty relay.

This route is served on the gateway API only. It is not published under
`/control` and the dashboard does not relay or display it.

## Compatibility

- The SQLite schema, the relay DTOs, existing grants, messages and audit
  history are unchanged. No migration is required.
- Bodies already retained count in the new shares as soon as this revision
  runs. A sender above its share is refused new sends until bodies are
  removed; nothing is invalidated or deleted to make room.
- New default behavior: one sender is refused beyond 128 retained messages or
  2 MiB, and one grant likewise. Earlier revisions only applied the tenant
  ceiling.
- New default behavior: expired bodies are released automatically by later
  sends. Earlier revisions kept every body until a runner prune.
- Rolling back: every automatic release fact follows the rule earlier
  revisions already enforce for a prune (no actor, expiry at or before the
  fact, its own later timestamp). Opening such a database with an earlier
  binary has not been executed as a test; take a backup before upgrading.
- `release_on_ack: true` is the exception: its acknowledgement facts are
  refused by earlier binaries. It is off unless the operator enables it.
- The tenant registry gains the optional `relay_retention_policy` field. An
  earlier gateway refuses a registry that contains it; remove the field before
  starting an earlier binary.

## Limits

- The Rust worker client and the stdio helper report every HTTP 429 as
  `Capacity`, whose text asks to contact the operator; they do not read the
  typed fields above. Only a caller that reads the HTTP error body sees the
  scope and figures.
- Starting the gateway binary with these options is covered by a software
  test on loopback, with stores prepared by the explicit migrations: startup
  line, applied policy read through the diagnostic route, and refused
  configurations. This is not a deployment qualification.
- The diagnostic is not available to workers, on `/control` or in the
  dashboard.
- The per-minute send rate remains per grant. There is no per-sender rate.
- Other shared bounds still apply to all workers of a tenant and can produce
  their own 429: the tenant I/O admission gate (`capacity_exhausted`) and the
  workload audit budgets (`audit_workload_budget_exhausted`).
- Bodies of a revoked grant or of a terminated worker keep counting until they
  expire and are released.
- Each message eventually costs one more audit event, its release. That event
  is an ordinary audit write, charged to no workload audit budget: neither a
  worker's nor the tenant's. For N messages it sends, a worker therefore
  induces up to N release facts in the ordinary audit share, on top of its own
  workload budget: up to twice that budget in total, and likewise for the
  tenant workload budget. Releases never draw on the control or identity
  reserves. The lifetime audit capacity of the store is not extended by this
  lot.
- One send performs at most 32 releases and reads at most 1024 retained
  metadata rows inside its transaction. This is bounded, but its duration has
  not been measured under load.
- These are software tests over the real router and SQLite with synthetic
  worker runtime facts. Real worker transport, Docker, Podman and Kubernetes
  remain unqualified for this behavior.
