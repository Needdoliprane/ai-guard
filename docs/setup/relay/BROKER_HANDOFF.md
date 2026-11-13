# Broker relay handoff — development contract

[Français](BROKER_HANDOFF.fr.md) · [Relay client](README.md) · [Workload provisioning](../workloads/README.md)

This document describes the source being developed in the broker handoff lot.
It has not been compiled or exercised in this checkout. The earlier owner,
runtime and Core SDK proofs do not qualify this composition. No real engine,
model, cloud service or 100-worker campaign has run for this lot.

The opt-in source connects the existing broker's actual Running callback and
durable WorkerClaim recovery path to Core admission, the fixed non-TTY helper
exec and the retained control owner. With no relay configuration, the relay is
unavailable. A profile declaration never establishes Ready.

## Fixed references and late issuance

Only a version 2 broker registry profile accepts this optional fragment:

```json
"relay": {
  "version": 1,
  "workload_operations": ["/absolute/private/workload-operations/worker-operation"]
}
```

Replace that illustrative path. The existing profile, runner authentication,
runtime selection and restrictions still apply. References are unique bounded
absolute paths (1–100 per profile). Their private parents must already exist,
be retained without symlinks and be disjoint from every configured workspace
and export root. The operation child may be absent initially. It is never
created, repaired, issued or rotated by the broker.

The operator fixes the operation reference in the profile before admission.
After the real Worker is Running, `broker relay-status` identifies its exact
worker/attempt/generation from retained authenticated broker facts. Use that
endpoint in the existing closed workload manifest, then use the existing
`workload prepare --input … --control-file … --output-dir …` and
`workload apply --operation-dir … --control-file …` commands. See the workload
guide for the manifest and authorized control identity. Do not derive IDs from
a worker frame or substitute a human/provider credential. Directed send grants
must also have been explicitly authorized through trusted control.

The broker waits only on the configured references. A missing child, a verified
incomplete preparation, or a currently held preparation lock can be retried.
Malformed content, unsafe permissions, changed FD/inode, different scope,
digest, receipt or binding fails explicitly. A complete set with no matching
endpoint, or two matching operations, is refused. A selected operation stays
readonly and FD-retained. No OAuth cache or implicit credential provisioning is
used. Automatic provisioning remains an open product gap.

Waiting and all later effects use the acknowledged original Worker Arm's
continuous clock method, boot ID and absolute deadline. UTC metadata and poll
cadence do not renew it. STOP invalidation and expiry interrupt waiting; the
tenant IO mutex is not held across a poll delay. Material appearing too late
does not grant execution.

## Retention, status and failure

The new source command is separate from historical Status/Page V1:

```text
agentsandbox broker relay-status --root /absolute/private/broker-root \
  --profile configured-profile --token-file /absolute/private/runner-file \
  --operation-id EXISTING-JOB-UUID
```

Those are placeholders and a source interface, not an executed recipe. The
service authenticates the same profile/tenant/namespace as the existing status
command and resolves its accepted journal job before reading relay details.
The response is versioned and closed. It reports AwaitingIssuedWorkload,
Admitted, Unknown or Closed, the retained endpoint/channel metadata, provenance,
last observation time and closed errors. `freshness` explicitly describes
retained facts, never a fresh runtime probe. No credential ID, proof or private
operation path is returned. Admitted is not Ready; inspect the separate Core
state. An error is not an empty inbox or absent activity.

Historical Status V1 keeps its closed fields. Status and RelayStatus may use
the last projection confirmed by the same writer actor, seeded by a successful
strict journal read. The projection is published only with a successful actor
acknowledgement, after the journal's durable post-read; an in-flight update
continues to show its previous confirmed head. This is a retained diagnostic,
not a new filesystem-integrity check or engine probe on each poll. A cold or
unretained job uses the strict path; missing cache data never means an absent job.

The memory projection retains at most 256 accepted jobs and 4 MiB of accounted
projection data per writer, further limited by the journal's job/scope bounds.
It contains no provider binding, history, credential or execution permission.
An oversized projection uses the strict path without truncating its fields.
These are logical storage bounds, not a measured RSS or 100-worker qualification.
Failed strict integrity checks, an unknown publication, service fencing/stop or
loss of its actor/service owner close this diagnostic port. An error is not
empty activity. Preserve the retained records and use the recovery diagnostics;
no cached fact can authorize a claim, start, mutation, export or quota release.
Partial STOP coverage alone can coexist with a valid last-confirmed pending
operation, such as an Arm request without its local acknowledgement. It is not
an integrity error or a runtime-health claim; typed integrity findings still
close the cached diagnostic.

Only a fresh HTTP 201 Core ClaimExec result can be consumed for one helper
exec. A historical replay never remints that capability. Current binding and
the exact pinned runtime incarnation/configuration are checked again before
exec and dispatch. The helper receives only closed frames over the existing
runtime's separate fixed non-TTY exec; no workload/control secret or local user
identity is framed. The worker remains network-isolated.

One tenant has one locked outbox and one serialized IO owner, shared by all of
its configured profiles; clients and authority are channel-specific. Immutable
send/ACK request identities and dispatch markers precede HTTP. Confirmed
receipts precede local success. A retry keeps identical identity/content;
uncertain effects, revocation and EOF never erase a request. The new
`relay_channels_v1` namespace retains bounded metadata facts beside `relay_v1`.
It is a private versioned file journal, not a new database or an audit/signature
checkpoint. Corrupt, partial, old-scope or changed files close access; preserve
them, never delete or repair them merely to reopen.

Blocking IO is admitted before `spawn_blocking` by shared tenant/global permits
(4/16 IO segments). Queued/running work retains its permits, FD/lock, RoleLease
and logical byte reservation until actual completion. Cancellation retains its
JoinHandle and refuses replacement until explicit join. Unknown task results
retain transport/IO owners in the registry. Local joins, helper EOF and CLI exit
do not prove Worker terminal or release quota. Core closure requires the exact
durable terminal Worker revision and observation ID.

A retained retirement intent reports Unknown with
`reconciliation_required=true` until a compatible Core acknowledgement is also
durable in the local journal. The last known error remains visible while that
result is unresolved. A rejection, lost reply, unavailable control service or
failed local save does not publish Closed or normal workload waiting. Even
without a Core admission, local closure is published only after its journal
commit. After an interrupted save, retirement joins the existing IO owner and
reads the durable local fact before preparing another intent; it does not
recreate an execution claim or renew the original deadline.

The 3 MiB exchange and 32/128 MiB tenant/global byte denominations are logical
admission units. Decoded inbox data, JSON values and encoded frames can coexist
above an exchange denomination. Total live memory/RSS and FD capacity are
NOT_RUN. Frame and on-disk bounds are independent of those units.

## Open verification and recovery work

Source tests cover the original clock/boot/STOP, durable facts reopening and
partial refusal, idle UDS prefix backpressure and historical Status V1 shape.
They are NOT_RUN for this composition. The two real software parents using
Core/HTTP/SQLite, two endpoints sharing tenant IO, the real Rust helper and a
clearly synthetic runtime witness remain to be written/executed by isolated
QA. The witness will bridge only exec stdio through a private fixture UDS; it
will not qualify a real engine or change production helper arguments.

The retirement source uses a separate historical lookup after the exact Worker
terminal and admissible local join. The authenticated runner and retained
admission fix the tenant, namespace, endpoint and operation identities. A lookup
never calls Reserve or ClaimExec and creates no fresh execution authority. Old
Claim, Ready and other attachment intents remain immutable, including intents
replaced in earlier V1 facts. They are resolved by exact receipts or validated
inapplicability proofs; a channel snapshot alone is not their receipt.

Cleanup is a separate durable intent: CancelReserved for a confirmed Reserved
channel, or CloseExactWorkerTerminal for an admitted nonterminal channel, using
the confirmed channel CAS and exact Worker terminal evidence. A lost cleanup
reply keeps the same UUID/content. Its historical receipt is sought first;
only that still-applicable cleanup may be explicitly replayed. Older attachment
intents are never replayed to discover their result. All pending intents must
be resolved before the complete local prefix can be finalized Closed. An exact
proof that an admission was never recorded can close without inventing a Core
channel. A purely local history with no admission, intention or Core state also
requires the terminal proof and join before finalization.

The same private journal accepts append-only V1/V2 records. Existing V1 bytes
are not rewritten; a historical V1 Closed does not become a new V2 proof or an
exec capability. V2 reserves 48 records per job: at most 16 ordinary/inherited
records and 32 reserved for validated retirement progress. The tenant bound is
12288 records and every full canonical envelope remains limited to 16 KiB.
Unchanged polls or response heads do not spend records. Cleanup reserves room
for its proof and finalization before HTTP; exhausted capacity leaves Unknown,
with records and owners retained. These logical bounds do not establish RSS,
physical storage performance or unlimited retention. An older binary refuses
a prefix containing V2: preserve the original prefix/backup before upgrade;
do not delete records to enable downgrade. No external anti-rollback anchor is
provided by this migration.

Recovered `local_joined=false` cannot prove an old CLI joined from absent
handles. Missing, inconsistent or unsupported future-CAS proofs remain Unknown,
including a terminal channel whose older pending cannot be resolved. A Worker
terminal or broker restart alone does not close those cases. Runtime recovery,
physical failures and the complete verified-job/service wrapper still require
their own evidence; the historical consumer's software tests do not qualify them.

Independent source review, a frozen isolated compilation/test run, the scope
before-detail IPC parent, real engine recipes, HTTPS/private-root profile
qualification and measured 100-worker behavior remain open. This lot does not
qualify the complete operator journey or production release.
