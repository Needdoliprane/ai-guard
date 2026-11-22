# Audit retention formats

[English](FORMATS.md) · [Français](FORMATS.fr.md) · [Guide](README.md)

Every document is UTF-8 JSON. Unknown fields are refused. Digests are lowercase hexadecimal SHA-256.

## Archive directory

An archive is a directory created exclusively with mode `0700` under an existing private parent. It holds two files, each `0600`:

| File | Content |
|---|---|
| `events.jsonl` | One archived event per line, `\n`-terminated, in ascending sequence, with no gap |
| `manifest.json` | The manifest below |

The archive covers a contiguous range `first_seq..=last_seq`, structural events included. No file is ever rewritten or removed by ai-guard.

### Archived event (`ai-guard.audit.archive-events.v1`)

| Field | Meaning |
|---|---|
| `seq` | Chain position |
| `id` | Event identifier |
| `received_at` | Receive time recorded by the control |
| `prev_hash` | Hash of the previous position |
| `hash` | `SHA-256("agentsandbox:audit:v1\0" ‖ seq as 8-byte big-endian ‖ received_at ‖ 0 ‖ prev_hash ‖ 0 ‖ payload ‖ 0)` |
| `payload` | The exact stored event JSON, as one string: the hashed bytes |

### Manifest (`ai-guard.audit.archive.v1`)

| Field | Meaning |
|---|---|
| `format` | `ai-guard.audit.archive.v1` |
| `archive_id` | UUID of this archive |
| `purpose` | `retention` (a ledger segment) or `export` (no reclaim) |
| `tenant_id`, `control_namespace_uuid`, `control_genesis_seq`, `control_genesis_hash` | Identity of the tenant chain |
| `backend` | `sqlite` (or `mysql` once served) |
| `segment_index` | 1-based segment of a retention archive; `0` for an export |
| `first_seq`, `start_prev_hash`, `last_seq`, `last_hash` | The range and the hashes that bind it to the chain |
| `floor_at_last` | Highest receive time through `last_seq`, as the live time floor records it |
| `event_count`, `payload_bytes` | Archived events and the sum of their payload sizes |
| `events_file`, `events_sha256`, `events_bytes` | `events.jsonl`, its digest and size |
| `previous_manifest_sha256` | Manifest digest of the previous segment; `null` for the first segment and for exports |
| `reclaim` | Retention only: `classes`, `reclaimed_events`, `reclaimed_bytes`, `reclaim_sha256`, `by_class`, `retained_structural`, `retained_pins`, `kept_positions` (reclaimable events kept live because a signed checkpoint names them) |
| `snapshot_sha256` | Retention only: digest of the persisted legacy view snapshot at `last_seq` |
| `policy_sha256` | Retention only: fingerprint of the policy |
| `created_at` | Declared by the producer; not a trusted timestamp |
| `producer` | `component` and `version` of the binary |

`reclaim_sha256` is `SHA-256("ai-guard:audit-retention-reclaim:v1\0" ‖ each reclaimed seq as 8-byte big-endian, ascending)`. The live ledger records the SHA-256 of the exact `manifest.json` bytes.

## Chain records

### `control.audit_retention.genesis`

`data`: `version` (1), `protocol` (`audit_retention_v1`), `control_namespace_uuid`, `previous_seq`, `previous_hash` (the chain head immediately before the marker).

### `control.audit_retention.applied`

`data` (`RetentionAppliedV1`): `version`, `protocol`, `control_namespace_uuid`, `segment_index`, `first_seq`, `last_seq`, `last_hash`, `floor_at_last`, `archive_id`, `manifest_sha256`, `events_sha256`, `archived_events`, `archived_bytes`, `classes`, `reclaimed_events`, `reclaimed_bytes`, `reclaim_sha256`, `reclaimed_by_class`, `retained_structural`, `retained_pins`, `kept_positions`, `snapshot_sha256`, `policy_sha256`, `previous_manifest_sha256`, `previous_retention_seq` and `previous_retention_hash` (the genesis marker for segment 1, else the previous applied record), `previous_seq` and `previous_hash` (the chain head immediately before this record).

New records include `apply.plan_sha256` and `apply.usage_before` (`stored_events`, `stored_bytes`). The chained record binds the plan and lets `apply` recover the original receipt from a verified archive. The archive format is unchanged. A record without `apply` remains verifiable and cannot reissue an apply receipt.

## Command documents

| `format` | Printed by |
|---|---|
| `ai-guard.audit.retention-command.v1` | `install`, `plan`, `apply`, `export`, with `command` naming the subcommand |
| `ai-guard.audit.retention-verification.v1` | `verify`: `verdict` (`verified` or `refused`), `segments`, `segments_verified`, `archived_through_seq`, `reclaimed_events`, `exports_verified`, `stored_events`, `live_head_seq`, `live_head_hash`, `warnings`, `failure` (`code`, `message`, `detail`) and `limits` |
| `ai-guard.audit.retention-error.v1` | Any refusal or error: `code`, `message`, `detail` |

The plan digest `plan_sha256` is `SHA-256("ai-guard:audit-retention-plan:v1\0" ‖ JSON of plan)`; `apply --expect-plan` requires it.
