# Publish declared broker checkpoints to a shared tenant control plane

[Français](CHECKPOINTS.fr.md) · [Presence](PRESENCE.md) · [Broker setup](README.md)

This path extends the existing authenticated presence publisher. It reports
bounded references to confirmed local job heads, with no transcript, command,
workspace content or runtime inventory. Several configured brokers can point
to the same tenant gateway while retaining distinct Broker identities, grants,
authorities and private local state. The gateway selects the tenant from each
verified proof; a request cannot choose another tenant or broker.

This is the schema 12 integration path. It requires a build containing the
checkpoint Store, gateway routes, control transport and broker-service source
adapter together. Source preparation is not a tested multi-host deployment.
The software regressions, runtime/transport matrix and qualification state
remain separate in engineering evidence.

## Configure each trusted broker

With every control writer and broker stopped, the explicit next step from an
existing schema 11 store is:

```sh
"/absolute/path/to/agentsandbox" migrate-control \
  --data "/absolute/private/tenant.sqlite" \
  --tenant-id "<authenticated-tenant-id>" \
  --backup "/absolute/private/tenant-before-checkpoints-v12.sqlite" \
  --target-version 12 --max-events 100000 --confirm-control-stopped
```

Use the installation's actual audit capacity and a new backup destination. The
migration preserves existing records, namespace, held reservations and quotas;
it adds the schema 12 marker without activating supervision. Keep its report
and verified schema 11 backup. A retry on schema 12 or an existing backup path
refuses; do not delete the source or backup to obtain another result. Earlier
schemas require their own explicit consecutive migrations and new backups.

1. Stop the control writer according to the normal maintenance procedure,
   preserve its database, and perform the explicit backed-up 11→12 migration.
   An older binary must refuse schema 12 before opening it as an older store.
   Presence V1 records and closed plans 1/2/3 keep their original formats.
2. Prepare the existing [Broker credential and grant](PRESENCE.md#prepare-authority-and-private-storage).
   Retain the same tenant namespace, broker, credential, incarnation and exact
   single authority approved for this Registry2 profile. No credential is
   created by enabling this publisher. Never give its proof to a worker.
3. Prepare two distinct private directories, outside project/export trees:
   `state_dir` for heartbeat evidence and `checkpoint_state_dir` for checkpoint
   evidence. Both must already exist, mode 0700, with private ancestry. Neither
   may contain the other. Preserve both directories when resuming.
4. Copy [presence-checkpoints.json](examples/presence-checkpoints.json) to a
   private regular mode 0600 file, replacing the placeholders with approved
   IDs and paths. Its `version:2` explicitly enables the checkpoint source
   requirement. The three `checkpoint_*` fields are mandatory and rejected in
   Presence V1, including fields whose value is null.
5. Point the Registry2 profile's existing `presence_file` at that file and
   start the existing `broker serve --config /absolute/private/registry.json`
   command. Its service adapter supplies confirmed local heads directly;
   there is no operator-authored snapshot file, shell command or worker feed.
   A V2 configuration without that adapter is refused rather than silently
   downgraded to heartbeat-only publication.

The source reads only a Full writer already admitted by that profile for its
real journal. Enabling V2 neither opens a writer nor accepts or starts a job.
Without that retained capability, checkpoint state stays unknown while the
heartbeat can continue. The journal's installation, profile, tenant, namespace
and canonical gateway must match the configured profile exactly.

Checkpoint budgets are explicit and independent of the heartbeat budgets:
`checkpoint_max_operations` is 1–32768; `checkpoint_max_state_bytes` is positive
and at most 134217728. The example reserves 64 MiB for each retained stream,
not a measured disk/RAM guarantee. Receipt space is reserved before POST.
Capacity exhaustion requires operator action; there is no implicit purge,
archive, credential rotation or new incarnation that resets audit capacity.

HTTPS, explicit public-CA trust, hostname validation, no proxy/redirect/fallback,
private-file revalidation and shared network/filesystem admission permits
follow the [presence transport contract](PRESENCE.md). No CA private key, runner
proof, SSO token or engine socket is sent to a worker.

## Publication and freshness

The gateway refuses metadata containing a credential known in the current
request context, even when it has the syntax of an identifier or hash. It checks
the request before persistence and withholds a response whose identifiers or
commitments would need masking. It returns a generic error without substituting
another hash or rewriting retained evidence. A withheld POST response does not
prove rollback: preserve and reconcile the same operation. A GET may become
unavailable after credential rotation if retained data now matches a known
secret; preserve the evidence and investigate rather than purging it.

The new scoped routes are POST `/api/v1/brokers/checkpoints` and GET
`/api/v1/brokers/checkpoints/self`, reached through the existing `/control`
transport. Each body is at most 32768 bytes and has a closed versioned schema.
It contains an operation UUID, incarnation, exact previous/current sequence,
authority and at most 100 sorted unique `(job_id, revision, checkpoint_hash)`
references. The snapshot has `has_more`; this never makes its coverage complete.
Both checkpoint routes require the authenticated Broker workload proof and its
current matching grant; neither a runner/viewer proof nor a human login can
replace it. Registry reads remain separate: technical runner/viewer readers use
GET `/api/v1/brokers`; named users use GET `/api/human/brokers` with an authorized
tenant context and explicit `read_broker_registry` permission (schema 7 or later).
Those reads do not authorize checkpoint writes or cross-broker access.

Before a POST, the publisher commits an immutable Pending with the same
operation ID, identity, sequence and payload. A lost response replays that
operation after revalidating the current Broker binding/grant. A differing
payload for that identity is a conflict. The current newer source state waits
until the retained operation is acknowledged; intermediate watch values may
be coalesced, while the broker's journal remains the full local history.
This stream is not an audit of every local checkpoint transition.

A source value of `None` means unavailable/not yet captured. It produces no
empty checkpoint and does not renew checkpoint freshness. A producer update
can report an empty declared list, but cannot prove zero runtime activity.
At each configured interval (5–150 seconds), the service issues one
`list(None, 100)` through that existing writer's FIFO. A successful real ACK is
required even for a known empty page. Missing/foreign Full capability, a failed
or tainted read, or a stopping/fenced/unavailable control keeps the source
unknown. The adapter sends at most 100 durable heads in the writer's unique
order; omitted entries without a head or a following page set `has_more:true`.
It samples again even when heads are unchanged. Reads queue behind real writer
work; one source read is awaited per profile, with no new latency or fairness
guarantee. Intermediate updates can coalesce. These are acknowledged snapshots,
not an atomic observation of all later journal mutations or runtime activity.
Both metadata and its references remain `declared`, with observed gateway
reception and `coverage:declared_partial`, including when `has_more:false`.

The control derives checkpoint expiry from its reception time, the granted
lease and grant expiry. An old report becomes stale even while heartbeat
presence remains current. A revoked/replaced grant retains historical report
metadata. Clock uncertainty remains explicit. Neither report nor heartbeat
creates a worker observation, completes a runtime inventory, changes quota,
refreshes an execution deadline or releases a reservation.

## Refusals, interruption and resumption

Transport/unavailability retries keep the exact Pending, use the existing
100/250/500 ms initial backoffs followed by at most 30 s recovery cadence, and
release attempt permits while waiting. Shutdown interrupts the wait; no
business start is introduced. Authentication, grant/binding, sequence, clock,
protocol or private-state refusal stops publication without an automatic
new identity, grant or runner fallback.

The living service retains the source reader and both publishers until their
real join, including filesystem work already admitted and terminal errors.
Cancelling a shutdown observer retains the same tasks for a later join. Normal
shutdown waits before releasing its local role; blocked filesystem work can
keep that wait open. Waiting establishes no physical collection bound and no
fencing or recovery after process loss. Dropping a wrapper only requests stop;
it does not prove a join. Preserve the state after an interruption and reconcile
it through the existing procedures, rather than treating process exit as proof
that runtime work or reservations have ended.

Resume only with the retained configuration and state. An unresolved Pending
cannot cross a changed binding/grant. A fully acknowledged stream can resume
with an explicit rotation of the same credential identity and matching grant;
its historical checkpoint remains attributable to the original binding.
An empty replacement directory cannot adopt a report already committed for
the same incarnation. Preserve evidence and reconcile the discrepancy.

Use the tenant's authorized common control/dashboard view. Missing or stale
broker metadata and `declared_partial` coverage must remain visible beside
job/worker facts; do not present these references as independently observed
execution or a complete fleet inventory. Real host disconnection/restart,
TLS/PKI lifecycle and a load of 100 workers require their own qualification.
The bound of 100 head references per snapshot is not a qualified worker count,
transport matrix or sustained-capacity guarantee.
