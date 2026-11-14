# Publish a remote broker with the owned supervision profile

[Français](README.fr.md) · [Broker setup](../broker/README.md) · [Legacy presence](../broker/PRESENCE.md) · [Legacy checkpoints](../broker/CHECKPOINTS.md)

> **Status: proposal, not an admitted or qualified feature.** This guide
> describes the broker side of the proposed `broker_supervision_owned_v1`
> profile. It needs the control-plane routes and enrollment maintenance of the
> companion control lot, the Root service seam and Root's decisions on the
> enrollment writer and on protocol evolution. Compilation, tests, runtime,
> multi-host and infrastructure checks are `NOT_RUN`. Commands below are
> **paths to be validated by Root**; none was executed for this guide. Values
> in angle brackets are placeholders, never real identifiers or secrets.

The owned profile lets a trusted broker on another host declare its presence
to the control plane of a tenant whose Store already uses the owned human-source
format (exactly 14 or 15). The broker publishes two things, each with its own
private journal:

- a **heartbeat**: a declaration by the broker; the control plane records only
  its own reception time (`declared` claim, `observed` reception);
- optional **checkpoints**: a `declared_partial` list of at most 100 job heads,
  32 KiB, with `has_more`, already acknowledged by the profile's admitted Full
  writer.

Neither is a runtime inventory, a worker health check, a terminal observation,
a fencing decision, a quota release or a usage fact. An absent source declares
nothing; nothing is ever reported as zero. The publisher holds no runner
credential, sends no command to workers and receives none.

## 1. Profiles and exact compatibility

A presence file selects exactly one profile by its top-level `profile` key.
There is no detection by content, no fallback and no migration between them.

| | Legacy Presence V1 / V2 | Owned `broker_supervision_owned_v1` |
|---|---|---|
| Selection | No `profile` key; `version` 1 (heartbeat) or 2 (checkpoints). A legacy file containing `profile` is refused. | `"profile": "broker_supervision_owned_v1"` and `"version": 1`, both required. Any other `profile` value is refused. |
| Identity source | UUIDs and authority typed in the configuration | Public enrollment handoff written by the control maintenance |
| Credential | `broker_token_file` | `bearer_file` (same file rules) |
| Control routes | `/api/v1/brokers/…` | `/api/v1/owned/brokers/self`, `/heartbeats`, `/checkpoints`, `/checkpoints/self` (the `identity` route exists but the publisher does not use it) |
| Tenant Store format | Legacy formats of [PRESENCE.md](../broker/PRESENCE.md) and [CHECKPOINTS.md](../broker/CHECKPOINTS.md) | Exactly 14 or 15, equal to the enrolled format |
| Journals | `state_dir`, `checkpoint_state_dir` | `heartbeat_journal`, `checkpoint_journal`, with an owned layout |

No Store format is served by both profiles. When a tenant is adopted into the
owned human-source format, its legacy presence publishers are refused by the
control plane; switch that profile to a new owned enrollment and new owned
journals. A legacy journal is never adopted, converted or reused, and a refused
owned configuration never falls back to the legacy profile.

## 2. Prerequisites

1. The control plane of the tenant runs a build in which Root admitted the five
   owned broker routes behind the authenticated control transport. Without
   them the gateway refuses the owned paths and the publisher stops with
   `control_response: gateway_refused` or `unrecognized`.
2. The tenant Store is format 14 or 15 through the
   [human-source adoption](../human-sources-maintenance/README.md).
3. The control operator ran the **offline** owned broker enrollment described
   in the control guide of the companion lot (proposed location
   `docs/setup/federation-control/README.md`): a broker grant with a fresh
   incarnation and exactly one authority, then a broker binding. It produces
   two private files for this broker: the public enrollment handoff and the
   bearer. Transfer them over a channel you already trust for control secrets;
   never paste the bearer in a command line, a ticket or a log.
4. The broker host runs the trusted foreground service of the
   [broker guide](../broker/README.md) with a Registry2 profile whose
   `gateway`, `tenant_id`, `control_namespace_uuid` and `runtime` match the
   enrollment.

## 3. Private files and permissions

All paths are absolute and normalized (no `.`, `..` or repeated separator).
Files are private regular files of the service account, mode `0600`, in private
`0700` parents; links, substitutions and unsafe ancestors are refused. Editing
any of them while the service runs is not a reload: the next revalidation fences
new business admission, as for legacy presence.

| File or directory | Bound and rules |
|---|---|
| Owned presence configuration (`presence_file` of the Registry2 profile) | ≤ 16 KiB, closed keys (section 5). |
| `enrollment_file` | Public handoff `OwnedBrokerEnrollmentV1`, ≤ 8 KiB. It contains no bearer or digest. Do not edit it: its commitment is pinned in the journal anchors. |
| `bearer_file` | One line of 32–4096 visible ASCII bytes, optionally one trailing LF. Sent only as the transport credential; never journaled, logged or displayed. |
| `ca_bundle_file` (optional) | HTTPS only; certificate-only PEM, ≤ 64 KiB, 1–8 certificates, replacing the built-in roots for this client. No CA private key; TLS is never disabled. |
| `heartbeat_journal.state_dir` | Existing private `0700` directory, **empty at first start** and dedicated to this enrollment. |
| `checkpoint_journal.state_dir` (optional) | Same rules; distinct from the heartbeat journal. |

The configuration, enrollment, bearer and CA paths are pairwise distinct. The
service also refuses, before creating any lock or anchor:

- a protected file inside a journal, lexically or physically, including the
  files of other profiles, the registry and the service control credentials;
- two journals that are equal (also by physical identity) or nested, within a
  profile or across profiles of the same broker;
- a journal equal to, inside or containing the broker `root`, a `source_roots`
  entry or an `export_roots` entry of any profile;
- two owned profiles enrolled for the same broker, incarnation or credential.

The bearer, every service secret literal and redaction pattern are screened out
of the configuration, the enrollment, the paths, the pins and every journal
record before persistence. The cumulative service protection limits of the
[presence guide](../broker/PRESENCE.md) (64 protected files, 64 secret literals)
still apply; the owned profile adds the enrollment file to that count.

Create the directories once, before the first start (path to be validated by
Root):

```sh
mkdir -m 0700 /absolute/private/ai-guard/owned-heartbeat-journal
mkdir -m 0700 /absolute/private/ai-guard/owned-checkpoint-journal
chmod 0600 /absolute/private/ai-guard/owned-presence/config.json \
  /absolute/private/ai-guard/owned-presence/enrollment.json \
  /absolute/private/ai-guard/credentials/owned-broker-bearer
```

## 4. Enrollment handoff and pins

The handoff carries the profile, the canonical gateway origin, the closure
revision at issue, the engaged subject (installation, closure, source, Store
format, tenant, namespace, broker, incarnation, credential, binding revision,
grant revision), the single authority, the lease, the grant and credential
expiry, the issue time and the audit receipts of the grant and binding steps.
It authenticates nothing: the gateway still authenticates the bearer and
compares the engaged subject on every operation.

At startup, read-only, the publisher validates the handoff, derives the journal
pins and compares them with its Registry2 profile:

| Pin | Must equal |
|---|---|
| Gateway origin | The profile `gateway` (HTTPS, or HTTP to a numeric loopback IP; no path, query or user) |
| Tenant, namespace | The profile `tenant_id`, `control_namespace_uuid` |
| Authority runtime kind | The profile `runtime` (`docker` or `podman`) |
| Lease | At least twice `interval_seconds` |

The installation ID of the handoff is the control closure's installation; it is
never compared with the broker registry's `installation_id`. Any mismatch
refuses the startup (`broker presence configuration is invalid or unsafe`)
before a journal is opened.

| Field class | Rule |
|---|---|
| Journal pins: profile, origin, installation, closure ID, source, format, tenant, namespace, broker, incarnation, credential, grant revision, authority | A change needs a new enrollment, which comes with a new grant and a new incarnation, and new empty journals. The old journals and their Pending stay untouched. |
| Binding revision | Engaged per operation: a retained Pending is replayed only under the same revision; new operations accept a renewed, higher one. |
| Closure revision | Observed: it may advance, never regress below the enrolled or last acknowledged value. |

## 5. Configuration reference

Copy [owned-presence.json](examples/owned-presence.json) into the private
configuration directory, replace every path, and point the Registry2 profile's
`presence_file` at it:

```json
"presence_file": "/absolute/private/ai-guard/owned-presence/config.json"
```

| Key | Contract |
|---|---|
| `profile`, `version` | Exactly `broker_supervision_owned_v1` and `1`. Unknown keys are refused. |
| `enrollment_file`, `bearer_file` | Required private files (section 3). |
| `ca_bundle_file` | Optional; omit or `null` for the built-in roots. |
| `interval_seconds` | Default 30, range 5–150. Twice the interval must not exceed the enrolled lease nor the broker lease seen by the control plane (≤ 300 s). |
| `heartbeat_journal` | Required: `state_dir`, `max_operations` (1–32768), `max_state_bytes` (51200–134217728). No default capacity. |
| `checkpoint_journal` | Optional, same keys, `max_state_bytes` 100352–134217728. Present ⇒ checkpoints are published; absent ⇒ heartbeats only. |

One record is bounded by its contract limit plus a 1 KiB envelope: 17 KiB for
heartbeats and 41 KiB for checkpoints. Before each Pending the journal reserves
room for its acknowledgement, and the first limit reached (operations or bytes)
stops that journal explicitly. A heartbeat is published once per interval, and
a checkpoint once per sampling interval while a source exists.

The cost of a journal grows with its history and is **not measured**
(`NOT_RUN`):

- At start, the publisher reads, validates and synchronizes every retained
  record and operation directory once, so the start time of a profile grows
  with its journal.
- During the run, each operation verifies the complete history at most twice
  by re-reading every record, without synchronizing it again: a new operation
  before writing its Pending and before writing its acknowledgement, a
  replayed Pending before its send and before its acknowledgement. Only the
  record being written, its directory and their ancestors are synchronized.

The example uses conservative starting values chosen so that the operations
limit is reached before the bytes limit even with maximal records: 2880
heartbeat operations in 96 MiB (about 24 hours at 30 s) and 1440 checkpoint
operations in 116 MiB (about 12 hours at 30 s). Before raising them, measure
on the target filesystem the start time and the duration of one operation at
the chosen size, and keep one operation well below the interval; no measured
figure is published yet. There is no archive, purge or compaction: plan a new
enrollment and new journals before a journal is exhausted.

## 6. Choose the journals

- Use **new, empty** directories for every enrollment. At first start the
  publisher creates `.owned-owner.lock` and `owned-anchor.json`; operations
  follow as `op-00000000000000000001/{pending.json, ack.json}`.
- The anchor binds the broker scope (broker installation, profile ID, gateway,
  tenant, namespace, runtime), the enrollment pins and commitment, the journal
  kind and the directory's physical identity. The heartbeat and checkpoint
  journals of one profile always belong to the same enrollment. Renaming the Registry2 profile, editing the
  enrollment or copying the journal elsewhere makes it **foreign**.
- A non-empty directory without the owned lock and anchor (for example a legacy
  `state_dir`) or with another anchor is refused as foreign and left untouched.
  An unexpected name or an unreadable record is refused as unsafe state.
- Two interrupted writes are completed at the next start without rewriting or
  removing anything, because no POST can precede the missing record: a
  directory holding only the empty `.owned-owner.lock` (initialization stopped
  before the anchor) receives its anchor, and an empty last `op-*` directory
  (operation stopped before its `pending.json`) is reused by the next
  operation. An empty earlier `op-*` directory or a torn record stays refused.
- The lock is exclusive: a second process on the same journal is refused.
- A checkpoint journal is useful only if the profile runs real Full jobs;
  without an admitted Full writer it declares nothing.

## 7. Start and stop (proposed)

Start the existing foreground service (path to be validated by Root):

```sh
"$AIGUARD_BIN" broker serve --config "$AIGUARD_BROKER_CONFIG"
```

All presence files are loaded read-only first, then the cumulative protections
are checked, then each journal is opened. A configuration or private-material
error refuses the startup. A journal-level refusal (foreign, unsafe state,
capacity, lock held) skips only that publisher, is logged and keeps its bytes;
business admission continues when the control material is safe.

`--recovery-only` opens no presence file or journal. `SIGTERM`/`SIGINT` stop
every publisher: a network wait is interrupted, admitted filesystem work is
joined, and the service joins each publisher before releasing its local role.
A stopped publisher keeps its Pending for the next start.

## 8. Diagnose

The service writes one JSON line per event on standard error, without secret
or remote prose:

| Event | Meaning |
|---|---|
| `broker_owned_presence` / `publication_started` | The owned profile was selected and the journal's actor runs. It proves nothing about the control plane, a runtime or a worker. |
| `broker_owned_presence` / `publication_retrying` | The short retries of one operation are exhausted (transport cut, lost outcome, temporary refusal); the same operation is retried every `min(interval, 30)` s. |
| `broker_owned_presence` / `publication_waiting_for_operator` | The control plane needs operator action; the same operation is retried at `max(interval, 60)` s. |
| `broker_owned_presence` / `publication_resumed` | The retained operation was acknowledged after one of these slow states. |
| `broker_owned_presence` / `checkpoint_source_unavailable`, `checkpoint_source_available` | The service is cold, fenced, has incomplete control material, no admitted Full writer or no acknowledged head; nothing is declared meanwhile. |
| `broker_presence` / `publication_stopped` | The publisher stopped; `reason` names the closed cause and `runtime_state` stays `unknown`. |

Owned reasons read `owned broker supervision is blocked (<category>: <detail>); <action>`:

| Category | Details | Operator action |
|---|---|---|
| `control_response` | an `owned_*` code, `transport_outcome_unknown`, `gateway_capacity_busy`, `gateway_not_dispatched`, `gateway_refused`, `source_coverage_unavailable`, `audit_budget_exhausted`, `unrecognized` | See section 9. |
| `handshake` | `forbidden` (revoked, expired or short lease), `clock_uncertain`, `protocol`, or a pin name | Fix the grant, the control clock or the enrollment; a pin name needs a new enrollment. |
| `identity` | a pin name, `binding_revision`, `binding_regression`, `closure_regression` | Section 11. |
| `acknowledgement` | `commitment`, `receipt`, `snapshot`, `conflict`, `protocol` | The control response does not acknowledge the retained Pending; keep the journal and reconcile with the control operator. |
| `contract` | an `owned_*` code | The operation could not be prepared under the shared contract. |
| `remote_head` | `without_local_history`, `diverged` | Section 13. |
| `journal` | `foreign` | Configure an empty private directory; keep the foreign one. |

Generic reasons keep their legacy wording, for example `broker presence
evidence requires operator reconciliation` (unsafe journal state) or
`broker presence retained capacity is exhausted` (journal limit).

## 9. Control responses and transport cuts

Only a known code with its exact HTTP status is recognized. Remote prose is
never kept.

| Response | Publisher behaviour |
|---|---|
| Connection, TLS or body stream failure; relay `control_transport_failure` (502/504); `owned_response_withheld` (503) | Outcome unknown: the same operation is reconciled by self-read, then resent with the same bytes. |
| `owned_supervision_unavailable` (503); gateway `not_dispatched` refusal for capacity (429) or body (408/400) | Same operation after backoff. |
| `owned_source_rebuild_required`, `owned_audit_unavailable`, `owned_response_unpublishable`, `owned_credential_ambiguous` (503); `identity_unavailable` (503) | Waits for the control operator at the slow cadence, Pending retained. |
| 401, 403, 400, 409, `owned_migration_required`, audit budgets (429), gateway route/method/size refusals | Stops with the Pending retained. |
| Any other body, including an unknown named 503, a redirect, a non-JSON or oversized body | Stops (`unrecognized` or protocol). An unknown 503 is never treated as temporary. |

Retried operations wait 100, 250 and 500 ms, then `min(interval, 30)` s between
attempts, until acknowledgement or a stopping refusal; entering that slow
cadence logs `publication_retrying` once. A long transport cut is therefore
survived without restart and without a new operation, and the publisher never
loses or replaces the unacknowledged Pending. A TLS trust error is a transport
failure too: it is retried, so check `ca_bundle_file` when the event persists.
The client uses no proxy, redirect or transparent retry, HTTP/1 only, 3/5/12 s
timeouts.

## 10. Pending and acknowledgement recovery

Each operation is written and synchronized as `pending.json` **before** the
POST; every retry, timeout and restart sends the exact durable bytes, the same
operation ID and sequence. An acknowledgement is accepted only when the
response is the exact expected successor (or an exact self-read proves this
operation committed), and it is synchronized as `ack.json` before any new
operation or checkpoint coalescing.

| Situation | What happens |
|---|---|
| The control committed, the response was lost | Next attempt reads the self view, recognizes the committed operation and records the ACK (`self_read`). |
| The response arrived but writing the ACK failed | The publisher stops; the Pending stays. At the next start the self-read recognizes it and the ACK is recorded. |
| Stop or power loss before a record existed (lock without anchor, empty last operation directory) | Completed at the next start (section 6); nothing was sent and nothing is rewritten. |
| Power loss while a record was written | An incomplete record can remain; the journal then refuses to open (unsafe state) and is preserved. No automatic repair exists; the qualified procedure is open. |
| A journal action fails unexpectedly (internal panic) | The publisher stops with `broker presence evidence requires operator reconciliation`; the Pending stays and the next start verifies the whole journal again. |
| Restart with a Pending | Same subject required (same binding revision); otherwise the journal is blocked. |

Never edit, delete or recreate a Pending, an ACK, the lock or the anchor.

## 11. Rotation, revocation and closure evolution

- **Binding renewal** (same credential, higher binding revision; produced by
  the control-side `broker-identity rotate`, which replaces the bearer key):
  accepted for new operations with the same enrollment. Replace the bearer
  file only while the service is stopped, and keep the original enrollment
  file and both journals; do not install the `enrollment.json` that `rotate`
  writes (it is evidence only, and its new commitment would make these
  journals foreign). Order (a path to be validated by Root): stop this
  publisher while the gateway still serves; check read-only that, in each
  owned journal, the last `op-*` directory holding a `pending.json` also holds
  an `ack.json` (if not, restart the publisher with the gateway serving until
  it is acknowledged, then stop it); only then stop the gateway and rotate;
  replace the bearer; start the gateway, then the publisher. A Pending engaged
  under the previous revision is blocked
  (`identity: binding_revision`) and needs a new enrollment and new journals
  today; a safe supersession is proposed to Root, not implemented.
- **New credential, regrant, new incarnation**: produced only by the
  control-side `broker-identity enroll`, always with a fresh incarnation, so
  new empty journals see no remote head of their incarnation; keep the old
  journals as evidence. A new credential on the same incarnation is not a
  supported path (new journals would refuse the existing remote heads).
- **Revocation or expiry**: the handshake or the gateway refuses; the publisher
  stops with its Pending retained and never replays it under a revoked
  credential.
- **Closure revision advance**: accepted, also during a Pending.
- **Closure revision regression** (for example an older control backup):
  `closure_regression`, stop.
- **New closure, source, Store format (14→15), tenant or namespace**: the
  handshake reports the pin; a new enrollment and new journals are required.

## 12. Backup and restore

Back up the configuration, enrollment and journals only with the service
stopped, as evidence. The anchor binds the journal to its original directory:
a restored copy in another directory is foreign, and an older history restored
in place diverges from the control head. A journal backup is therefore not a
resumable state. After loss of a journal, keep every remaining byte and request
a new enrollment with a new incarnation.

A restored control backup that lags behind the broker's acknowledged state is
detected as a closure regression or a diverged head; reconcile on the control
side first.

## 13. No remote adoption

A journal without history that meets a control head of its incarnation stops
with `remote_head: without_local_history`; a control head that differs from the
last local acknowledgement stops with `remote_head: diverged` (for example a
second publisher using the same enrollment on another host). The publisher
never adopts, rewrites or resets a head. Run exactly one publisher per
enrollment; local startup refuses two owned profiles of the same broker, but
another host can only be detected through this divergence.

## 14. Limits and status

- Admission, compilation, formatting, tests, runtime, multi-host, provider and
  infrastructure checks: `NOT_RUN`. Root decisions on the offline enrollment
  writer and the binding-renewal Pending remain open.
- No hot reload, no archive or purge, no automatic repair after a torn write,
  no qualified retention horizon, no measured start or per-operation journal
  cost (section 5).
- The control-side views, freshness and permissions are described by the
  companion control guide; a heartbeat older than its lease only shows that the
  last declaration expired.
