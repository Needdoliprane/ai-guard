# Audit retention

[English](README.md) · [Français](README.fr.md) · [Setup index](../README.md)

This guide describes the offline maintenance command `agentsandbox audit-retention`. It archives a closed prefix of a tenant audit chain into a **new** private directory, verifies that archive, then reclaims capacity in the live store **without breaking the chain**: the live store and its archives verify together, end to end. The exact files are specified in the [format reference](FORMATS.md). This guide is a contract; the implementation status and its evidence are tracked in [engineering](../../engineering/CURRENT_STATE.md).

## Which deployments this guide covers

| Deployment | What this guide delivers | Executed evidence |
|---|---|---|
| SQLite tenant store at format 13, on host paths of the control account | Every command below | Software tests on macOS ARM64 only (see engineering) |
| SQLite store at format 14 or 15 (owned human sources) | **Not served yet**: these stores are opened through the owned-source union, which this command does not open | `NOT_RUN` |
| MySQL tenant store (password or password + mTLS) | **Not served yet**: parity is a separate open lot | `NOT_RUN` |
| Compose installation whose store lives in the `audit_data` volume | A host CLI cannot open that volume; a one-shot container procedure is not provided yet | `NOT_RUN` |

Nothing here runs as a service, contacts a network, or deletes an archive.

## Why retention is needed

A tenant store accepts at most `--max-events` events (default 100,000; at most 1,000,000) and `--audit-max-bytes` of event payloads (default 256 MiB; a SQLite store may select up to 2 GiB, at the cost of a longer full validation at each start and after an external change; MySQL stays at 256 MiB). When the budget is spent, every append is refused: launches and control events stop until an operator acts. Retention is that action. It removes old **leaf** events from the live store after archiving them; nothing is ever lost silently.

## What is reclaimed, and what always stays

| Event class | Examples | Reclaimed? |
|---|---|---|
| `ordinary` | collector and runner facts: `command.*`, `runtime.*`, `workspace.*`, `observer.coverage`, `agent.*`, `file.*`, provider output | Yes, when the policy lists `ordinary` |
| `human_read` | the audit of each human read: `control.identity.access`, `control.identity.brokers.access`, `control.identity.workers.access` | Yes, when the policy lists `human_read` |
| Structural | every `control.*` event other than the three above, `security.decision`, `worker.*`, `usage.allocation`, broker retirement facts, relay channel facts | **Never**: projections reference them and the store replays them at every start |

Within a reclaimed range the live store also keeps:

- the **boundary**: the last event of each segment, so the live chain continues from a known hash;
- the **first sighting** of each session, agent, sandbox and project: the event that an identity grant needs to prove that the resource exists;
- every **signed checkpoint position** of the checkpoint directories passed with `--checkpoint-dir`, so that [signed checkpoints](../audit-signatures/README.md) keep comparing the store with each signed position.

The live store also keeps **egress decisions, observations, and grants** so the broker can retrieve the original receipts. New plans record these positions in `kept_positions`. Choose a shorter segment if the combined retained positions exceed the 4,096-position limit.

Worker lifecycle events are structural. In stores measured during development they were most of the payload bytes. Reclaiming the events of closed workers is a separate lot that is still open: until it is delivered, retention postpones capacity exhaustion but does not remove the growth caused by worker lifecycles.

## What the live store records

- **The capability marker** `control.audit_retention.genesis`, appended once by `install`.
- **One `control.audit_retention.applied` event per segment**, in the hash chain itself. It names the archive (`archive_id`, SHA-256 of its manifest and events), the range, the boundary hash, the reclaimed counts and bytes by class, and the previous segment. Signed checkpoints taken afterwards therefore sign the retention record too.
- **The ledger table** `audit_retention_segments`, checked against those events at every start.
- **A snapshot** of the legacy dashboard views (agents, sandboxes, overview counters) at each boundary, so these views keep showing the complete history.

The store format number does not change. A binary that predates retention refuses a store that carries it.

## Policy

There is no implicit policy. Create a private file (`0600`, at most 16 KiB):

```json
{"version":1,"min_age_seconds":2592000,"keep_recent_events":10000,"max_segment_events":250000,"classes":["ordinary","human_read"]}
```

| Field | Meaning | Bounds |
|---|---|---|
| `min_age_seconds` | Only events whose highest preceding receive time is at least this old | 86,400 (24 hours) or more |
| `keep_recent_events` | Events closest to the head that are never part of a segment | 1 to 1,000,000 |
| `max_segment_events` | Largest segment, in archived events | 1 to 1,000,000 |
| `classes` | `ordinary`, `human_read`, or both, in that order | at least one |

The 24-hour floor exists because a reclaimed event identifier is no longer in the live store: a producer that retried it later would not be recognised as a duplicate. After retention, the store refuses any ordinary event declared at or before the archived horizon (`event_predates_archive`), and `verify` reports a reused identifier (`duplicate_event_id`).

## Procedure

Run every command as the control account, with the same `--max-events` and `--audit-max-bytes` as the gateway. `install` and `apply` require `--confirm-control-stopped`: stop the gateway, dashboard and brokers of the tenant first. This is an operator attestation, not runtime fencing; the commands also hold the SQLite write lock for their whole transaction.

1. **Optional, recommended with signed checkpoints.** Sign the current head (`ai-guard-audit-checkpoint create`) and let the anchor holder run `anchor advance --data`.
2. **Install once**, with a new backup path:

   ```sh
   agentsandbox audit-retention install --data "$STORE" --tenant-id "$TENANT" \
     --backup "$BACKUPS/before-retention.sqlite" --confirm-control-stopped
   ```

3. **Plan.** Nothing is written:

   ```sh
   agentsandbox audit-retention plan --data "$STORE" --tenant-id "$TENANT" --policy "$POLICY" \
     --checkpoint-dir "$CHECKPOINTS"
   ```

   `plan` and `apply` require an explicit choice: pass every checkpoint directory of the tenant (`--checkpoint-dir`, repeated for continuation directories), or attest `--without-signed-checkpoints`. Without one of them the command refuses. A reclaimed event at a signed position would make `ai-guard-audit-checkpoint verify --data` refuse that position with `database_truncated`.

   Review `plan.first_seq`, `plan.last_seq`, `plan.reclaimed_events`, `plan.reclaimed_by_class`, `usage_before` and `usage_after_reclaim`. Keep `plan_sha256`.
4. **Apply** the reviewed plan into a new directory under an existing private parent:

   ```sh
   agentsandbox audit-retention apply --data "$STORE" --tenant-id "$TENANT" --policy "$POLICY" \
     --through-seq "$LAST_SEQ" --expect-plan "$PLAN_SHA256" --checkpoint-dir "$CHECKPOINTS" \
     --archive-dir "$ARCHIVES/segment-1" --confirm-control-stopped
   ```

   `apply` recomputes the plan inside its transaction and refuses with `plan_mismatch` if anything changed. It writes and re-reads the archive before it removes anything.
   If the response is lost or `retention_commit_unconfirmed` is reported, keep the archive and retry the same command. Once the recorded plan and the archive both verify, the command returns the original receipt without reclaiming again. If it cannot confirm the receipt, run `verify` with the archive and review the result before planning another apply.
5. **Preserve the archive.** Copy the archive directory to two durable locations and record its `manifest_sha256` out of band. Deleting an archive makes its prefix unverifiable.
6. **Verify**, then restart the control plane:

   ```sh
   agentsandbox audit-retention verify --data "$STORE" --tenant-id "$TENANT" \
     --archive-dir "$ARCHIVES/segment-1"
   ```

7. **Optional.** Sign the new head, which now includes the retention record.

Run `plan` and `apply` again for each further segment; each new archive chains to the previous one.

## Verify end to end

`verify` reads only. It checks:

- the live chain, including the sparse prefix: each kept event hashes to itself, each segment keeps exactly its accounted events and its boundary;
- that every segment has its archive, whose manifest digest the live ledger records;
- that every archive is a valid chain from the end of the previous one, starting from the zero hash;
- that every kept event is byte-identical to its archived copy and every missing event is one that the segment was allowed to reclaim (`unaccounted_loss` otherwise);
- the reclaim accounting and the persisted view snapshots, recomputed from the archives;
- that no live event reuses the identifier of a reclaimed one.

An archive passed to `verify` that the ledger does not reference is reported as a warning `orphan_archive`: it is what an interrupted `apply` leaves, and nothing it holds was reclaimed.

While the gateway runs, `agentsandbox verify-audit --archive-dir A1 --archive-dir A2 …` first asks the gateway for its integrity check, which covers the live chain, its sparse prefix and the per-segment commitment to the rows kept live (`retained_sha256`). It then verifies every archive against the retention ledger that the gateway reports: chain from the zero hash, manifest digests, reclaim accounting and view snapshots. On a store with segments, `verify-audit` fails when an archive is missing.

## Verifiable export

`export` writes the same archive format for a contiguous live range, without reclaiming anything, into a new directory. `verify --archive-dir` then checks it against the live store. Use it to hand an auditor a verifiable copy of recent history.

```sh
agentsandbox audit-retention export --data "$STORE" --tenant-id "$TENANT" --archive-dir "$EXPORTS/2026-11"
```

## Results and errors

Each command prints one JSON document. Exit status `0` is success, `2` a refusal with a closed code (nothing changed in the live store), `1` an operational error.

| Code | Meaning | What to do |
|---|---|---|
| `retention_capability_missing` | `install` was not run | Run `install` |
| `retention_capability_present` | `install` was already run | Nothing |
| `retention_requires_format` | The store is not at format 13 to 15 | Migrate first |
| `policy_invalid` | The policy is malformed, keeps less than 24 hours or lists classes out of order | Fix the policy |
| `nothing_to_reclaim` | No event of the admitted range can be reclaimed yet | Wait, or review the policy |
| `range_invalid` | `--through-seq` is outside the admitted range | Plan again |
| `plan_mismatch` | The store changed since the plan was reviewed | Plan again |
| `archive_destination_not_new` | The archive path exists | Choose a new path |
| `archive_destination_unavailable` | The parent is missing or not a private directory | Create a private parent (`0700`) |
| `archive_destination_reserved` | The archive shares the physical SQLite parent and its name is reserved by SQLite or uses non-ASCII characters | Choose an ASCII name outside the SQLite namespace or another private parent |
| `archive_write_failed`, `archive_unverified` | The archive could not be written or verified | Keep the directory for diagnosis and run `verify` before applying again |
| `retention_commit_unconfirmed` | The archive is complete and the database did not confirm the commit | Keep the archive and retry the same command to check the receipt |
| `reclaim_referenced_event` | A projection references an event selected for reclaim | Nothing was reclaimed; report it as a defect |
| `checkpoint_log_unavailable` | A `--checkpoint-dir` could not be read or belongs to another tenant | Nothing was changed; fix the path |
| `snapshot_too_large` | The legacy view snapshot exceeds 8 MiB | Nothing was reclaimed |
| `event_predates_archive` | An ordinary append declared at or before the archived horizon | The producer must send a new event |
| `archive_missing`, `archive_manifest_mismatch`, `archive_chain_invalid`, `archive_digest_mismatch`, `retained_event_diverges`, `unaccounted_loss`, `duplicate_event_id`, `snapshot_mismatch`, `export_diverges` | `verify` refusals | Restore the archive from a copy; investigate before any further retention |

## Limits

- The live chain is unkeyed. Against someone who can rewrite both the live store and its archives, only [signed checkpoints](../audit-signatures/README.md) with an independent anchor detect a coherent rewrite.
- Reclaimed human read audits are verified as chained bytes; they are not re-validated against the identity projection at their position.
- Event capacity counts stored events; the chain length keeps growing. Human read and workload audit budgets restart at each segment boundary.
- Structural events are never reclaimed by this version; see above.
- Formats 14 and 15, MySQL and the Compose volume are not served yet.
