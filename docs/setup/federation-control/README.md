# Owned federation control plane

[Français](README.fr.md) · [Setup index](../README.md) · [Owned remote broker publisher](../federation-broker/README.md) · [Workload identities](../workloads/README.md) · [Adopt human sources](../human-sources-maintenance/README.md)

> **Local integration candidate, not activated or qualified.** Root selected
> offline Rust IdentityAuthority maintenance, the final closed
> `broker_supervision_owned_v2` profile and journal version 2. Claude leaves and
> their parent seams are composed; C3 adds exact rotation recovery before first
> activation. Compilation, formatting, tests, QA, runtime, multi-host and
> providers are `NOT_RUN` on this candidate. Procedures below contain
> placeholders only. Root retains review, QA admission and activation.

## Scope

The owned profile lets a remote broker publish its declared presence to the
control plane of its own tenant, under an identity issued by an offline
maintenance and authenticated in the tenant's retained source. This guide
covers:

- the offline broker identity lifecycle: `enroll`, `rotate`, `revoke`,
  `resume`, `status` (subcommands of `agentsandbox human-sources broker-identity`);
- the trust model of the enrollment handoff;
- the seven closed owned supervision routes on the control transport;
- the owned human broker page on `GET /api/v1/human/owned/brokers` and its
  source-integrated dashboard candidate (HTTP and visual qualification remain
  `NOT_RUN` on this composition);
- diagnosis of the closed refusals, backup/restore and recovery.

It does not cover the publisher, which has its own
[guide](../federation-broker/README.md). Nothing in this profile admits a
worker, a relay, a Kubernetes runtime or an egress path, sends a remote
command, fences a process, releases a quota or meters usage. A heartbeat is
a declaration whose reception is observed; a checkpoint is a
`DeclaredPartial` list of at most 100 declared heads (32 KiB, `has_more`). An
empty list never proves zero activity.

## What exists and what does not

| Part | State |
|---|---|
| Shared final contract v2 and journal v2 | Source integrated; strict old-profile refusal, no implicit v1 conversion |
| Store/API routes, human page and offline IdentityAuthority lifecycle | Source composed; validation and activation pending |
| Exact rotation reconciliation and funded append-only Superseded | Implemented candidate source; tests written, NOT_RUN |
| Human read-profile selector | Owned broker v1 available; worker/federation explicitly unavailable, not permissions |
| Dashboard integration | Separate frontend delivery; no browser qualification inferred here |
| Online identity writer, targeted index invalidation | Not admitted; offline maintenance only |
| Compilation, format, tests, runtime, multi-host qualification | NOT_RUN on this candidate |

## Prerequisites

1. The tenant union is already adopted with
   [human-sources maintenance](../human-sources-maintenance/README.md): each
   tenant Store is format **14 or 15** and its retained descriptor requires
   exactly that format; Auth5 if OIDC is selected. A format 6–13 Store keeps
   the legacy presence lanes and is refused here (`migration_required`).
2. The private activation (`activation.json`) or the explicit owned-source
   options used by the gateway are available.
3. **Every** consumer is stopped: gateway, dashboard, ingress and brokers.
   `--confirm-control-stopped` is the operator's attestation. The command also
   takes the real owner locks: a running gateway makes it refuse with
   `owned_union_owner_contended`. An older process is not fenced.
4. A private parent directory (owner only, mode `0700`) exists for the new
   plan directory, outside every source directory, project and export.

Opening the union records a control incarnation in **every** source, exactly
like a gateway start. Every subcommand, including `status`, therefore writes
that record: none of them is a read-only diagnostic of SQLite.

## Trust model and handoff

The maintenance generates the broker bearer itself (32 bytes from the OS
generator, the same `aigw1_` format as the workload CLI). The bearer never
appears in an argument, the plan, a receipt, the handoff, standard output or
a log. Only its SHA-256 digest reaches the Store. Before any write the digest
is checked against every technical runner/viewer credential and every
current or historical human and workload credential of every source; a
collision refuses the plan.

A completed `enroll` or `rotate` leaves two private files in the plan
directory:

| File | Content |
|---|---|
| `bearer` | The secret and one newline, mode `0600`, created exclusively before the plan. |
| `enrollment.json` | Public `OwnedBrokerEnrollmentV2`: profile, canonical gateway origin, closure revision at issuance, engaged subject, the single authority, lease, grant expiry, credential expiry, issue time and the audit receipts of the grant and the binding. No bearer, no digest. |

The handoff is built only from committed receipts, then cross-checked by
authenticating the new bearer through the same scoped Store path as the
gateway. It **authenticates nothing by itself**: on every operation the
gateway authenticates the bearer in the retained source and compares the
engaged subject. After `enroll`, transfer both files to the broker host over
a private channel and keep them `0600`; follow the
[publisher guide](../federation-broker/README.md) (delivered with the
publisher lot) to install them with new owned journals. After `rotate`, only
the new `bearer` goes to the broker host; see [Rotate the bearer](#rotate-the-bearer).
The report's `publisher_action` field states the exact action for each
command.

The engaged subject is: installation, closure ID, source, source format,
tenant, control namespace, broker, incarnation, credential, binding revision
and grant revision. The closure revision is observed only: it may advance,
but a lower value than already acknowledged is a regression. Any change of a
pinned field requires a new handoff and a new publisher journal.

## Compatibility and migration

| Profile | Store format | Access |
|---|---|---|
| Legacy presence | 6–13 heartbeat, 12–13 checkpoint | Legacy routes, unchanged |
| `broker_supervision_owned_v2` | Exactly 14 or 15, equal to the retained descriptor | Scoped owned routes only |

No format is served by both profiles. A 14→15 migration changes a pin and
requires a new enrollment. A gateway that does not serve the owned profile
refuses the owned routes: through `/control`, `404 unknown_control_route`
with `outcome:not_dispatched`; through the public router without the gate's
marker, `503 identity_unavailable`. An older binary does not know the
`broker-identity` subcommand; keep the binary that created a plan available
to resume it.

## Take the backup proof

With every consumer stopped, take a consistent copy of the selected tenant
Store with your existing SQLite backup procedure, for example
`sqlite3 <tenant-store> ".backup '<new-backup-file>'"` (to validate in your
environment). Then:

- keep it as a regular single-link file of mode `0600` in a private
  directory that is **not** the directory of any source, and outside the
  configuration files of the union;
- make sure no `-wal`, `-journal` or `-shm` file sits next to it;
- record its SHA-256 (`shasum -a 256 <new-backup-file>` or
  `sha256sum <new-backup-file>`).

Before the first mutation the command verifies the file and its digest
(before and after), opens it read-only and immutable, and checks its format,
schema, tenant binding and quota, control namespace, chain integrity and
`quick_check`. Its audit head must be **exactly** the source state before this
session's own incarnation record. A backup taken before another gateway start
or maintenance session is therefore refused: take a new one. A resume
requires the same file and digest, and its head must still be a prefix of the
live chain.

## Enroll a broker

```sh
agentsandbox human-sources broker-identity enroll \
  --human-sources-activation /absolute/control/human-preparation/activation.json \
  --confirm-control-stopped \
  --backup-file /absolute/private/backups/<tenant>-before-enroll.sqlite \
  --backup-sha256 <64-lowercase-hex> \
  --tenant-id <tenant-id> \
  --new-broker \
  --authority-id <authority-label> \
  --runtime-kind docker \
  --lease-seconds <1-300> \
  --grant-not-after <RFC3339-time> \
  --gateway-origin https://<control-host> \
  --plan-dir /absolute/private/broker-identity/<new-plan-name>
```

- Instead of the activation, the explicit owned-source options of the gateway
  are accepted: `--human-sources-manifest`, `--human-credentials-envelope`,
  `--tenants-file`, `--human-control-config-revision`, and optionally
  `--oidc-config` with `--human-oidc-mapping-revision` and `--max-events`.
- Use `--broker-id <broker-uuid>` instead of `--new-broker` for an existing
  broker or an operator-chosen ID. If that broker still has an active grant,
  add `--replace-active-grant`: a new grant always uses a new incarnation.
- `--authority-id` is a short label (`A-Z a-z 0-9 . _ : -`, at most 128
  bytes); `--runtime-kind` is `docker` or `podman`. It is a declared scope,
  not an installed adapter; no Kubernetes broker is admitted by this profile.
- The grant expires at most 366 days ahead. `--credential-expires-at` is
  optional, never after the grant, and defaults to it. Times are stored as
  millisecond UTC.
- `--gateway-origin` is the canonical origin the broker will use: HTTPS, or
  HTTP only to a loopback IP literal, without user, path, query or fragment.

The plan directory is new and is never adopted if it exists. It receives, in
order, `bearer`, `plan.json` (all operation IDs fixed in advance, the source
pins and the backup evidence), one `step-<n>.json` receipt per committed
step, then `enrollment.json`. Steps: a grant with a fresh incarnation and a
fresh credential ID, then a digest-only broker binding. Each step runs under
a fresh write epoch in an `IdentityAuthority` transaction. On success, the
standard output is one JSON report with `state:"complete"`, the enrollment
commitment, the public subject and `candidate_index:"rebuilt_at_next_gateway_start"`.

Restart the gateway normally afterwards: it rebuilds the candidate index of
the union. A running gateway would have to rebuild anyway, because a
committed binding change advances the owned discovery generation.

## Rotate the bearer

```sh
agentsandbox human-sources broker-identity rotate \
  --human-sources-activation /absolute/control/human-preparation/activation.json \
  --confirm-control-stopped \
  --backup-file /absolute/private/backups/<tenant>-before-rotate.sqlite \
  --backup-sha256 <64-lowercase-hex> \
  --tenant-id <tenant-id> \
  --broker-id <broker-uuid> \
  --gateway-origin https://<control-host> \
  --plan-dir /absolute/private/broker-identity/<new-plan-name>
```

Rotation keeps the credential, broker, incarnation and grant; it binds a new
bearer digest and advances the binding revision. The grant must be current
and the current binding unrevoked and unexpired; expiry is never extended in
place (a later grant is a new enrollment). The previous bearer stops
authenticating.

On the publisher, **replace only the bearer file, while the publisher is
stopped**, and keep its original enrollment file and both owned journals: the
pins of that enrollment accept a higher binding revision for new operations.
The `enrollment.json` written by `rotate` is evidence of the rotation receipts
only and must **not** be installed on the publisher: its new binding receipt
changes the enrollment commitment, so the existing journals would be refused
as foreign, and new empty journals would refuse the control's existing heads
of the same incarnation (a remote head is never adopted). The report states
`publisher_action: replace_only_the_bearer_file_while_stopped_keep_enrollment_and_journals`.

The report lists only `bearer` in `files` and `enrollment.json` in
`evidence_files`.

Order (a path to be validated by Root): (1) stop the owned publisher while the
gateway still serves, so that a retained Pending can still be acknowledged;
(2) check read-only that, in each owned journal, the last `op-*` directory
holding a `pending.json` also holds an `ack.json` (a trailing empty `op-*`
directory is not a Pending); otherwise a Pending is retained: restart the
publisher with the gateway still serving until it is acknowledged, stop it
again, or keep it and accept the blocked case below; (3) stop the gateway; (4) run `rotate`; (5) replace the
bearer file; (6) start the gateway, then the publisher. A retained publisher Pending is never replayed with a new identity/hash.
The v2 recovery below keeps the original enrollment/journals, then either
records the exact historical ACK or durably supersedes an exactly absent
operation after the offline barrier. Conflict/unknown stays blocked.

## Revoke

```sh
agentsandbox human-sources broker-identity revoke \
  --human-sources-activation /absolute/control/human-preparation/activation.json \
  --confirm-control-stopped \
  --backup-file /absolute/private/backups/<tenant>-before-revoke.sqlite \
  --backup-sha256 <64-lowercase-hex> \
  --tenant-id <tenant-id> \
  --broker-id <broker-uuid> \
  --scope all \
  --plan-dir /absolute/private/broker-identity/<new-plan-name>
```

`--scope all` (default) revokes the grant, then every unrevoked binding of
its current incarnation; `grant` or `binding` revokes only one side. The
Store binds each binding revocation to the plan's broker: a plan step that
names a credential of another broker (or of a worker) is refused with
`broker_identity_durable_conflict` and revokes nothing. History and digests
are kept: a revoked digest stays a collision and is never reissued. After
the next gateway start the publisher's next operation is
refused (`owned_broker_forbidden` or `owned_broker_authentication_required`).
Revocation does not prove that workers stopped, release a quota or send a
command; follow the broker stop and runtime reconciliation procedures.

## Resume a partial maintenance

```sh
agentsandbox human-sources broker-identity resume \
  --human-sources-activation /absolute/control/human-preparation/activation.json \
  --confirm-control-stopped \
  --plan-dir /absolute/private/broker-identity/<existing-plan-name>
```

Resume requires equal source pins (installation, closure ID, source, tenant,
format, namespace) and a closure revision that did not regress, the same
backup file and digest, and a backup head that is still a prefix of the live
chain. It then replays every step in order with its fixed operation ID:

- a committed step returns its original receipt (`replayed`) and is compared
  with the recorded one;
- a step whose receipt was lost after commit is recovered by the same replay;
- divergent content for the same operation is a conflict;
- a recorded receipt that is no longer in the durable chain (for example
  after restoring an older control backup) stops **before** any write.

An existing `enrollment.json` is checked against the receipts and never
rewritten. A refusal reports `state=partial_maintenance` with
`completed_steps` and `total_steps` when this run confirmed at least one step.
Preserve the plan directory; never edit, rename or reuse it. If a step can no
longer be applied (for example the grant expired before its binding), revoke
and start a new plan.

## Read the status and history

```sh
agentsandbox human-sources broker-identity status \
  --human-sources-activation /absolute/control/human-preparation/activation.json \
  --confirm-control-stopped \
  --tenant-id <tenant-id> \
  --broker-id <broker-uuid> \
  --history-limit 20
```

The report shows `evaluated_at`, the broker view (grant revision, state
`unseen|current|stale|expired|revoked|clock_uncertain`, lease, expiry,
`runtime_inventory_complete:false`), the bindings of the current incarnation
and one newest-first, digest-free history page of a credential
(`--credential-id`, default the grant's; `--before-revision` for the next
page). `broker_presence:"absent_from_complete_durable_read"` is a complete
durable absence in this tenant Store. A history page has
`count_scope:"page"`; an empty page is not evidence of no history.

## Owned supervision routes (control side)

After Root admits the seams, the control transport classifies exactly these
gateway-root targets (after removal of `/control`):

| Operation | Method and path | Body | Response |
|---|---|---|---|
| identity | `GET /api/v1/owned/brokers/identity` | none | ≤ 4 KiB |
| self view | `GET /api/v1/owned/brokers/self` | none | ≤ 16 KiB |
| heartbeat | `POST /api/v1/owned/brokers/heartbeats` | ≤ 4 KiB | ≤ 16 KiB |
| checkpoint | `POST /api/v1/owned/brokers/checkpoints` | ≤ 36 KiB | ≤ 40 KiB |
| checkpoint self view | `GET /api/v1/owned/brokers/checkpoints/self` | none | ≤ 40 KiB |
| heartbeat reconciliation (read) | `POST /api/v1/owned/brokers/heartbeats/reconcile` | ≤ 96 KiB | ≤ 128 KiB |
| checkpoint reconciliation (read) | `POST /api/v1/owned/brokers/checkpoints/reconcile` | ≤ 96 KiB | ≤ 128 KiB |

Reconciliation authenticates the active current binding before the global
operation-ID lookup, in the same checked source transaction as the exact
binding history r→r+1, head and receipts. A current owner-incarnation receipt
strictly after rotation proves the real exclusive reopen barrier. A different
broker/type/hash at that ID is conflict, not absence. Missing/incomplete,
unhealthy, redacted or stale reads never become NonCommitted.

CommittedExact keeps the historical operation identity r and its receipt
separate from observation r+1, with no business write or POST. NonCommitted
requires exact global absence and the exact local predecessor; only then may
the publisher fund and sync an append-only Superseded record containing one
complete replacement/new UUID, same sequence/CAS and checkpoint snapshot.
The normal mutation guard/replay stays exact. No v1 journal migration,
remote-head adoption, credential revocation exception or new execution
authority exists. See the [publisher recovery contract](../federation-broker/README.md#exact-recovery-across-a-rotation).

Exact bytes only: no decoding, query, HEAD alias or prefix. The gate resolves
the bearer in the complete startup candidate index (exactly one workload
candidate names the only source that may authenticate it; no SQL on another
tenant) and authenticates it in one checked transaction of that source,
then takes the tenant's presence permit and attaches a private marker; a
handler without it answers `503 identity_unavailable` before any body, Store
or audit work.

Each operation then runs in **one** checked transaction of the same source:
the discovery generation recorded at startup is compared with the current
one, the same bearer is authenticated again and must still be the gate's
broker proof, the engaged identity is rebuilt, the request subject and
commitments are compared, the private registry input hash is recomputed
against the shared mirror before any replay or mutation, and the generation
is compared again before the commit. A changed generation (for example a
binding changed since startup) refuses with `owned_source_rebuild_required`
**without** mutation and closes the human contexts until the gateway is
restarted. A positive response is published only after commit and a
redaction screen, never altered; a refusal after a possible commit is
`owned_response_withheld`. The presence operations share the tenant's four
presence permits, and the gateway's Store read lane with the human reads;
their latency under the three-worker recipe is not measured (`NOT_RUN`).

## Owned human broker page

On an owned gateway, `GET /api/v1/human/owned/brokers` (query `limit` 1–100,
default 25, and the sealed `cursor`) serves the owned page. Its shape is not
the legacy registry page's, so it has its own path and names
`profile:"human_broker_registry_owned_v1"`; the legacy
`GET /api/v1/human/brokers` stays closed there (`503 identity_unavailable`),
as before. This composition includes the dashboard's owned parser and states
and its exact GET proxy path. The dashboard selects this page from the
gateway's explicit `read_profiles` announcement, independently of permission.
Integrated HTTP and visual qualification remain `NOT_RUN` on this source;
these changes do not activate the profile or provide owned worker/federation
views. A legacy gateway refuses the owned path before any authentication or
Store work.

The page uses a local human proof in an Administration context with the
`ReadBrokerRegistry` capability, revalidated in the read transaction of the
proof's own source. The mandatory read audit is appended before any page,
including an empty one; an audit failure returns no page. The page is
`count_scope:"page"`, never a tenant total; heartbeat and checkpoint
freshness are independent; `clock_uncertain` is explicit. Job IDs, hashes,
operation and credential IDs and path-like authority labels are excluded.
The cursor is sealed for this owned profile and never opens a legacy page.

`checkpoint.status` is `received`, `absent` or `unknown`, from two bounded
index reads: first the broker's 128 most recent registry operations; if they
hold no publication and older operations exist, the tenant's 4096 most
recent checkpoint publications (read at most once per page). `absent` is
reported only from a complete history: all of the broker's operations, or
all of the tenant's publications. `unknown` means that neither bound holds a
publication of this broker and older history exists, for example a broker
that publishes heartbeats only, in a tenant with more than 4096 checkpoint
publications; it is never shown as absent. The broker's own
`checkpoints/self` view reads its latest publication without that bound.

## Diagnose closed refusals

Maintenance refusals are one line on standard error:
`broker_identity_refused; reason=<code>; state=<state>; completed_steps=<n>; total_steps=<m>; action=<action>; command=<command>`.

| Reason | Meaning | Action |
|---|---|---|
| `owned_union_owner_contended` | A gateway still holds the owner locks. | Stop it, then retry. |
| `owned_union_open_refused`, `owned_union_activation_refused`, `owned_union_tenants_refused` | The union, activation or registry did not open as the gateway would. | Check the options and files; preserve them. |
| `owned_union_selection_required`, `owned_union_options_incomplete`, `control_stop_attestation_required`, `broker_selection_required` | Exactly one union route, its complete options, the stop attestation or exactly one of `--broker-id`/`--new-broker` is missing. | Correct the arguments. |
| `broker_identity_invalid_input` | An argument is invalid (time, lease, origin, authority, ID). | Correct it; no plan step was written. |
| `broker_identity_union_mismatch` | Tenant absent, source pins changed or closure regressed. | Check the union configuration and closure. |
| `broker_identity_migration_required` | The tenant Store is not format 14/15. | Migrate offline first. |
| `broker_identity_backup_refused` | Backup missing, altered, aliased, with sidecar, or not the exact pre-session state. | Take a new backup with the control stopped. |
| `broker_identity_plan_conflict` | Existing directory, divergent file, receipt not durable, foreign entry. | Preserve the directory; reconcile with `status`. |
| `broker_identity_plan_incomplete` | Plan file missing (for example a bearer without plan). | No operation of it was written; start a new plan. |
| `broker_identity_plan_busy` | Another maintenance process holds the plan. | Wait for it to exit. |
| `broker_identity_state_refused` | The durable state does not admit the action (active grant without `--replace-active-grant`, nothing to revoke, revoked binding...). | Inspect `status`. |
| `broker_identity_credential_collision` | The generated digest already exists. | Start a new plan; report it. |
| `broker_identity_durable_conflict` | Revision, sequence or content conflict in the Store. | Inspect `status`; resume only the same plan. |
| `broker_identity_clock_uncertain` | The control clock precedes the durable floor. | Reconcile the clock, then resume. |
| `broker_identity_audit_unavailable` | Audit capacity or integrity refused the write. | Restore it, then resume. |
| `broker_identity_unavailable` | Temporary: busy source, epoch, storage. | Resume with the same plan. |
| `broker_identity_handoff_refused` | The new bearer does not authenticate as planned. | Do not install any handoff; inspect `status`; resume. |
| `broker_identity_output_refused`, `broker_identity_output_unavailable` | The report could not be encoded, screened or written. | The durable outcome is unchanged; run `status` or `resume`. |

`state=no_step_confirmed_by_this_run` means that this run confirmed no plan
step (opening the union still recorded its incarnation);
`state=partial_maintenance` means that at least one step is durable and the
plan must be resumed, never discarded.

Owned route refusals use the closed envelope
`{"error":{"code","message","retry_action","profile"}}`:

| Code | HTTP | Publisher retry class |
|---|---|---|
| `owned_broker_authentication_required` | 401 | stop |
| `owned_broker_forbidden` | 403 | stop |
| `owned_invalid_request`, `owned_protocol_mismatch` | 400 | stop |
| `owned_identity_mismatch`, `owned_operation_conflict`, `owned_sequence_conflict`, `owned_clock_uncertain` | 409 | stop |
| `owned_migration_required` | 503 | stop |
| `owned_source_rebuild_required`, `owned_audit_unavailable` | 503 | wait for operator |
| `owned_response_unpublishable`, `owned_credential_ambiguous` | 503 | wait for operator |
| `owned_supervision_unavailable` | 503 | same operation after bounded backoff |
| `owned_response_withheld` | 503 | reconcile the same operation |

`owned_source_rebuild_required` means that the index or the retained source
changed since startup: restart the gateway. `owned_credential_ambiguous` means
that the presented digest is claimed by several credential classes or sources.
Historical digests are kept forever (a revoked digest stays a collision), so
that bearer can never authenticate again, even after a revocation and a
restart: treat the duplicated secret as compromised, give the broker a new
bearer (`rotate`) or a new enrollment, and investigate how the duplicate arose
(for example a copied or restored Store). `owned_response_unpublishable` is a deterministic refusal to
publish a positive response (a redaction collision or a server invariant).
The Store screen runs before commit and rolls back; the gateway screen runs
after the Store job and may refuse an operation that is already durable. In
both cases retrying cannot change it: the publisher keeps its Pending, and
once the control configuration is corrected the exact replay returns the
original receipt. A legacy presence refusal or an unknown named body is not
an owned code.

## Recovery after durable reception with a lost response

A heartbeat or checkpoint is committed with its audit event and projection in
one transaction. If the response is then lost (connection cut, timeout,
`owned_response_withheld`), the publisher retries the **same** operation: the
exact replay returns the original receipt with `replayed:true`, after the
identity is revalidated under the current authority. The heartbeat self view
carries the receipt of the latest heartbeat operation, read in the same
transaction as the identity, so that a publisher recognizes only its exact
committed operation. Divergent content with the same operation ID is
`owned_operation_conflict`; a revoked, expired or replaced identity refuses
even an exact replay.

On the maintenance side, a lost response after a committed step is recovered
by `resume`: the same operation returns its original receipt.

## Backup and restore

- Keep the pre-maintenance backup and its digest at least until the plan is
  complete and the gateway restarted; `resume` requires them.
- Keep plan directories: they hold the bearer of the last handoff and the
  committed receipts. Protect them like credentials.
- Restoring an older control backup can drop committed identity steps. A
  later `resume` of a plan whose receipts are no longer durable stops before
  any write; reconcile with `status`. The restore itself is not a qualified
  procedure: it can resurrect a revoked grant or binding. Prefer revoking and
  re-enrolling over restoring.
- A restored broker journal or handoff never adopts a remote head; follow the
  publisher guide.

## Root seams not admitted

| Patch | File | Purpose |
|---|---|---|
| P00 | `src/broker_registry.rs` | Declare the shared contract module. |
| A01–A06 | `src/audit/…`, `src/audit.rs` | Declare the owned Store leaves; extract the shared registry and binding bodies; visibility and re-exports. |
| A10–A15 | `src/api/…`, `src/control_transport.rs` | Source-indexed candidates, route classification, gate branch and marker, router, owned human page route. |
| A20–A22 | `src/human_authority/maintenance.rs`, `src/human_authority.rs`, `src/human_sources_cli.rs` | Declare the maintenance, make it reachable by the binary, add the subcommand. |
| A23 | `docs/setup/README*.md` | Index entry for this guide. |

These seams are admitted together, in one integration: partially applied,
they leave unused entries or break existing tests. The tests that Root adapts
and runs, the decisions D1/D2 and the activation order belong to the
integration, not to this guide.

The human identity response announces optional `read_profiles` only for an
actual owned gateway: broker_registry=`human_broker_registry_owned_v1`,
worker_registry=`human_worker_registry_owned_v1` on SQLite 14 and 15,
federation_registry=`unavailable`. Legacy omits this object. The control
revalidates `read_broker_registry` for Brokers and `read_worker_registry`
for Workers in the offered administration context. The complete federation
view remains an open product requirement. [Supervision](../supervision/README.md)

## Qualification

`NOT_RUN`: compilation, formatting, Clippy, unit and integration tests, CLI
runs, backup verification on real files, gateway start with the owned routes,
publisher interoperability, multi-host, Docker/Podman/Kubernetes, providers,
TLS and infrastructure. No production gate is closed by this guide.

Source/Control16 compatibility is a **mandatory next integration** for the
joint federation and egress path. This profile currently refuses 16. An
explicit semantic/versioned compatibility contract and software/runtime
qualification are required; no numeric widening or global production claim
follows from the current 14/15 implementation.
