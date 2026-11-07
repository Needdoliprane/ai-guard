# Private control owner — software component

This component extends the [approved transport direction](../../engineering/RELAY_RUNTIME_HANDOFF.md).
It is not an available runtime transport or a command to launch workers.
The existing [helper](HELPER.md) and [HTTP client](README.md) keep their own
qualification scopes. The [component evidence](../../engineering/evidence/2026-11-07/relay-control-owner/README.md)
records executed checks separately from tests merely present in source.

Frozen input `relay-owner66` passed 12 owner tests, 13 existing client cases and
6 existing profile cases in QA AL: 31 distinct functions. The owner tests use
real local sockets, HTTP and private files with declared HTTP receipts; the
existing client/profile canaries exercise the gateway and SQLite. This does
not make the worker transport available. All-target compilation passed on the
same code in QA AO. Strict Clippy for input66 remains NOT_RUN; the evidence
retains the distinct sources and limits.

`src/relay/channel/control.rs` is private. It has no production admission
constructor; its production admission check refuses every exchange. Tests alone
can supply a clearly named synthetic admission. A frame never chooses a tenant,
source endpoint, credential, origin, HTTP route or runtime authority.

The owner borrows a retained IO owner, retains the workload client in control,
and performs only `send`, `inbox` and `ack` using the fixed `ControlIngress`
profile. Hello/Ready, channel ID, epoch, correlation reuse, the 1024-exchange
ceiling and response correlation use the existing closed protocol. One exchange
is admitted at a time. A second prefix or EOF during HTTP ends admission without
reading a second JSON body. A receipt confirms server acceptance, never recipient
execution, worker termination or quota release.

Before reading an exchange prefix, the owner reserves one logical unit denominated
as 3 MiB through shared tenant and global counters. That same reservation stays
held through response backpressure and pending IO. The service must supply the
same counters to all relevant
owners, with the approved 32 MiB tenant/128 MiB global ceilings; creating new
counters per owner would defeat the admission bound. These units do not measure
the sum of live buffers: a 2 MiB decoded inbox, JSON values and an encoded frame
of approximately 2 MiB can coexist. Raw DTO/frame/depth limits remain separate;
total live memory and RSS peaks are NOT_RUN. The 3/32/128 MiB denominations are
not RAM caps or measured capacity. No production budget factory is exposed here.

## Private `relay_v1` records

The format uses existing `Directory`/`PrivateFile` primitives in a dedicated
broker-business namespace outside databases, source, workspaces and exports.
There is no new database, no reinterpretation of execution checkpoints, and no
business body copied into the audit outbox. The parent must already belong to
trusted control. Creation requires a new `relay_v1` child; reopening never creates
missing entries or repairs a partial namespace.

| Record | Meaning |
|---|---|
| `.owner.lock` | Immutable format/version record, held with an exclusive flock. |
| `scope.json` | Canonical `relay_v1`, version 1 and the exact existing `JournalScopeV1`. |
| `op-UUID/request.json` | Format/version, endpoint, credential ID and exact typed send or ACK. |
| `op-UUID/dispatch.json` | Durable SHA-256 reference to the request before any HTTP dispatch. |
| `op-UUID/receipt.json` | First validated receipt with the same request reference, durable before local success. |

Files remain private, retained by FD and immutable; each creation synchronizes
the file and parent directory. The initial namespace also synchronizes its
ancestor chain. Unknown entries, missing records, unsupported versions,
replacement, unsafe links/modes and failed writes close that owner. Nothing is
deleted or overwritten to make recovery pass. These hashes do not provide signed
checkpoints or detect restoration of an older complete namespace across restart.

The operation key is the UUID alone within this tenant scope. Reuse across
actions, credential IDs, endpoints or divergent content conflicts. Credential
revision and channel epoch are deliberately absent from the operation key:
rotation of the same credential ID does not create another server idempotency
space. A different credential ID cannot replay the first operation. The HTTP
server still validates current binding and permissions on every explicit replay;
a stored receipt is never returned as a fresh delivery authorization.

Both client preflight helpers validate the typed DTO, decoded body and final
fixed HTTP target before any operation directory is created. Known secrets are
also checked against the complete encoded record and decoded JSON. The retained
proof stays in control memory; it is absent from records and frames. This is not
exhaustive DLP.

The common durable-write primitive checks every record, including the constant
lock record and generated dispatch digest. Generated operation directory names
are checked before creation as well. A metadata value matching a known secret
is refused even when the ordinary CLI would generate a different proof format.

At most 1024 operation identities and 64 MiB of reserved encoded storage are
admitted per scope, including 32 KiB reserved for dispatch/receipt metadata per
operation. At most one non-confirmed mutation per endpoint is admitted; an exact
replay can use its existing reservation at capacity. Neither reconnection nor
rotation resets those reservations. There is no automatic pruning or replay.

A request without a dispatch marker is prepared. A dispatch without a validated
receipt is unresolved after reopening. A current refusal never clears a prior
unknown outcome. This segment conservatively retains **every** non-confirmed
dispatch, including a first definite HTTP refusal; refusal classification and an
explicit operator reconciliation procedure remain required before production
availability. A new UUID, TTL, start or grant cannot bypass that reservation.

## Blocking IO and ownership

All journal validation, parsing and synchronization for the async owner run in
`spawn_blocking` segments. Separate shared tenant/global permits are acquired
without a waiting queue before spawning. Each queued/running segment owns those
permits, the buffer reservation, journal FDs/flock and a real `Arc<RoleLease>`.
The blocking-pool default is not itself the admission bound.

The handle remains inside the retained `IoOwner` mutex while awaited.
Cancelling `serve` or its observer releases the wait, not that handle or its
resources. The service registry must keep this owner until explicit join.
Close, output shutdown and join share one five-second closing deadline. An
unfinished join returns `NeedsReconciliation`; it does not permit replacement,
replay or release of the retained role. There is no synchronous IO fallback.
An already dispatched HTTP request may still commit after local closure.

## Required integration before availability

The service must still connect its owner registry and existing exclusive role
lease, retain shared budgets, prepare durable channel/epoch records, obtain a
distinct relay-exec permit, and validate the exact current workload binding and
runtime incarnation before each dispatch. The current fail-closed admission
point must be replaced only by that concrete path, including mandatory audit
reservation. No consumed start permit, worker frame or public callback supplies
that authority.

The exact non-TTY runtime child, stderr drainage, helper image/profile validation,
revocation and process shutdown remain separate required work. Docker, Podman,
Kubernetes, three real workers, providers, network confinement and production
gates are not qualified by this component. STOP/enforcer remains independent;
this module creates no terminal observation or usage measurement.
