# Signed audit checkpoints: format reference

[English](FORMATS.md) · [Français](FORMATS.fr.md) · [Guide](README.md)

This page is the published contract of version 1 of the documents read and written by `ai-guard-audit-checkpoint`. It is meant for an auditor who wants to read or re-implement the verification, and as the reference a later version must stay compatible with. The [guide](README.md) explains how to operate the command; the source of these rules is `src/audit/signed_checkpoint/`. A contract is not a qualification: see the [engineering handoff](../../engineering/evidence/2026-11-10/audit-signatures/README.md).

## Common rules

- Every document is JSON in UTF-8. Signed records are ASCII only.
- A SHA-256 is written as 64 lowercase hexadecimal characters. "All zeros" means 64 `0` characters.
- A UUID is written in its canonical lowercase hyphenated form.
- A time is `YYYY-MM-DDTHH:MM:SS.mmmZ` (UTC, milliseconds) and nothing else. Every time is declared by its writer and is not a trusted timestamp.
- Binary values are unpadded base64url (RFC 4648 section 5) in canonical form.
- Integers are unsigned and at most 9223372036854775807.
- A tenant identifier is 1 to 128 ASCII letters, digits, hyphens or underscores.
- Unknown, missing or duplicated members are refused everywhere.

## Signing key

| Item | Rule |
|---|---|
| Algorithm | Ed25519 (RFC 8032, pure variant) |
| Private key file | `signing-key.pk8` in the key directory: PKCS#8 v2 (RFC 5958) with the public key, 83 bytes, mode `0600`, directory mode `0700`. It never appears in any other document. |
| Fingerprint `key_id` | SHA-256 of the raw 32-byte public key |

Public key document, written by `key show --out` as one line followed by a line feed, at most 1,024 bytes:

```json
{"format":"ai-guard.audit.signing-public-key.v1","algorithm":"ed25519","key_id":"<sha256>","public_key":"<base64url of 32 bytes>"}
```

A reader refuses a document whose `key_id` is not the SHA-256 of its `public_key`. This document approves nothing by itself: only an anchor pins a key.

## Signed statement

A statement is one JSON object in **compact form** (no whitespace), with exactly the members below **in this order**. The bytes of this object are what is signed and hashed. A reader parses the statement, serializes it again from its typed content and refuses it unless the result equals the received bytes.

### Kind `checkpoint`

| # | Member | Type | Rule |
|---|---|---|---|
| 1 | `type` | string | `ai-guard.audit.checkpoint` |
| 2 | `version` | integer | `1` |
| 3 | `kind` | string | `checkpoint` |
| 4 | `tenant_id` | string | Tenant bound to the audit database |
| 5 | `namespace_uuid` | string | Control namespace of the database, created with its control genesis event |
| 6 | `log_index` | integer | Position in the checkpoint log, from 1 |
| 7 | `previous_statement_sha256` | string | SHA-256 of the statement at `log_index - 1`; all zeros if and only if `log_index` is 1 |
| 8 | `created_at` | string | Declared time |
| 9 | `signing_key_id` | string | Fingerprint of the key that signs this record and is expected to sign the next one |
| 10 | `audit_schema_version` | integer | Schema version of the database when signed, at least 2 |
| 11 | `chain_algorithm` | string | `agentsandbox:audit:v1` |
| 12 | `event_count` | integer | Number of audit events signed, at least 1 |
| 13 | `head_hash` | string | Chain hash of event number `event_count`; never all zeros |
| 14 | `control_genesis_seq` | integer | Sequence of the control genesis event, between 1 and `event_count` |
| 15 | `control_genesis_hash` | string | Chain hash of that event |
| 16 | `previous_event_count` | integer | `event_count` of the last signed position before this record; 0 if there is none |
| 17 | `previous_head_hash` | string | `head_hash` of that position; all zeros if and only if `previous_event_count` is 0 |

`previous_event_count` is at most `event_count`; when both are equal, `previous_head_hash` equals `head_hash`.

The control genesis of a database is one event, written once with its control namespace; no migration moves it. Every checkpoint signed over one database therefore names the same `control_genesis_seq` and `control_genesis_hash`.

### Kind `key_rotation`

| # | Member | Type | Rule |
|---|---|---|---|
| 1–8 | `type`, `version`, `kind`, `tenant_id`, `namespace_uuid`, `log_index`, `previous_statement_sha256`, `created_at` | | As above, with `kind` equal to `key_rotation` and `log_index` greater than 1 |
| 9 | `retiring_key_id` | string | Fingerprint of the key that hands the log over |
| 10 | `successor_key_id` | string | Fingerprint of the key expected to sign the next record; differs from `retiring_key_id` |
| 11 | `endorsement` | string | `retiring_key` or `none` |
| 12 | `previous_event_count` | integer | Last signed position before the rotation, at least 1 |
| 13 | `previous_head_hash` | string | Its `head_hash` |

A rotation signs no new audit position: it carries the previous one forward.

## Signature and record

The **signed message** is the 35 ASCII bytes `ai-guard:audit-signed-checkpoint:v1`, one zero byte, then the exact statement bytes. `statement_sha256` is the SHA-256 of the statement bytes alone.

A record is one JSON object in compact form, at most 2,048 bytes, without a trailing line feed:

```json
{"format":"ai-guard.audit.signed-checkpoint.v1","statement":{…},"signatures":[{"key_id":"<sha256>","signature":"<base64url of 64 bytes>"}]}
```

- `signatures` holds one or two entries. The required signers, in this order and with no other entry, are: `signing_key_id` for a checkpoint; `retiring_key_id` then `successor_key_id` for a rotation whose `endorsement` is `retiring_key`; `successor_key_id` alone when it is `none`.
- `key_id` only names a key. The public key is always taken from the trust anchor. A record has no member that could carry a key.
- The whole record must be byte for byte the compact serialization of its content.

### Checkpoint directory

For `create --previous-record` with nothing new to sign, the destination's first-record policy is still checked without a lock before `unchanged`: an existing complete head, a fork at that head, a renamed record or an unavailable directory is refused. The scan can stop at the first complete index; it does not authenticate a log that has already started. No directory or lock is created; incomplete files are reported. After the first record, invoke `create` without `--previous-record`. This unlocked no-op is an inspection at that point, not a guarantee against a concurrent writer.

- A record is the file `cp-<log_index as 20 decimal digits>-<statement_sha256>.json`, mode `0600`, in a private directory (mode `0700`). Files are created exclusively and are never renamed, rewritten or removed by the tool. `.writer.lock` is the writer lock: an empty file, never removed, created by the first command that takes the lock. `create --new-log` or `--previous-record` takes it to write the first record, after reading the directory without it: a start refused because the directory holds a complete record, a record file that holds another record than the one its name announces, a fork or the maximum number of record files creates no lock file. `create` without `--new-log` and `rotate-key` take it on a directory that holds a complete record. Before creating the lock file, they refuse a directory that holds no complete record, two complete records at the head, or a record file at or above the head that holds another record than the one its name announces. Under the lock they read every record of the directory, then compare their key, and `create` the database, with the head of the log: a refusal decided there (`log_forked`, `log_discontinuous` or `record_name_mismatch` below the head, `key_id_mismatch`, `database_truncated`, `database_diverged` for example) leaves the lock file in a directory that had none, such as one restored from a copy of its records.
- A file with a record name that does not parse as a record is an **incomplete record**: reported, ignored for the chain, kept. This is what an interrupted write leaves.
- A file with a record name that holds a complete record **other than the one its name announces** cannot come from an interrupted write: the refusal is `record_name_mismatch`. `verify`, `anchor advance`, `anchor approve-rotation`, `export`, `create` when extending an existing log, and `rotate-key` refuse a directory that holds one, and `create --previous-record` refuses a file that is not named after the record it holds, before creating anything.
- The head of the log is the complete record with the highest log index. Two complete records at one index are a fork (`log_forked`).
- A complete record is trusted for what attaches it to a trusted record, not for being in the directory. `create` and `rotate-key` read every record under the writer lock and write only after a head whose directory holds, below it, the chain that the head commits to: at each lower index, the single complete record whose statement hash is the `previous_statement_sha256` of the record above. They refuse a second complete record at one index (`log_forked`), another record than the one the chain names (`log_discontinuous`, with `detail.log_index`) and a complete record below an index that holds none (`log_discontinuous`, with `detail.missing_log_index` and `detail.unattached_log_index`). A directory may start at any index: nothing is required below its first record.
- `create` signs only a database that has the control genesis named by the most recent checkpoint that the directory holds at or below the head of the log, and whose control genesis does not stand after the last signed position; `create --previous-record`, which reads that one record, applies the first rule only when the record is a checkpoint. The refusal is `scope_mismatch` with `detail.field` equal to `control_genesis`.
- A directory holds at most 8,192 record files, plus 16 reserved for rotation records. `create --previous-record` continues in a new directory whose first record links to the last record of the full one.

### Known-answer example

Key: RFC 8032 section 7.1 TEST 1 (public key `d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a`).

| Value | Result |
|---|---|
| `key_id` | `21fe31dfa154a261626bf854046fd2271b7bed4b6abe45aa58877ef47f9721b9` |
| `public_key` | `11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo` |
| `statement_sha256` | `e6ae63c790cdb627184eae9456d121da689c6b5b23eee1e20854817c7b2ccabf` |
| `signature` | `ZQQAfRJ4nYrjgO-L60YjnJIQwE-OHBaA0BV5gkJirThk3D0QmLzD6q7tHExkM_MpEWQJAoC5PDhazeF2T24cAA` |
| File name | `cp-00000000000000000002-e6ae63c790cdb627184eae9456d121da689c6b5b23eee1e20854817c7b2ccabf.json` |

Statement (772 bytes, one line):

```json
{"type":"ai-guard.audit.checkpoint","version":1,"kind":"checkpoint","tenant_id":"tenant-a","namespace_uuid":"6f1c2a9e-3b7d-4c5e-8f90-1a2b3c4d5e6f","log_index":2,"previous_statement_sha256":"1111111111111111111111111111111111111111111111111111111111111111","created_at":"2026-11-10T09:00:00.000Z","signing_key_id":"21fe31dfa154a261626bf854046fd2271b7bed4b6abe45aa58877ef47f9721b9","audit_schema_version":11,"chain_algorithm":"agentsandbox:audit:v1","event_count":42,"head_hash":"2222222222222222222222222222222222222222222222222222222222222222","control_genesis_seq":1,"control_genesis_hash":"3333333333333333333333333333333333333333333333333333333333333333","previous_event_count":17,"previous_head_hash":"4444444444444444444444444444444444444444444444444444444444444444"}
```

The unit test `statement_bytes_are_deterministic_and_match_the_frozen_vector` asserts these values.

## Trust anchor

Written as one line followed by a line feed, at most 262,144 bytes. Members, in the order written:

```json
{"format":"ai-guard.audit.trust-anchor.v1","revision":3,"previous_anchor_sha256":"<sha256>","tenant_id":"tenant-a","namespace_uuid":"<uuid>","keys":[{"key_id":"<sha256>","algorithm":"ed25519","public_key":"<base64url>","valid_from_log_index":1,"valid_through_log_index":null,"approval":"initial"}],"reference":{"log_index":5,"statement_sha256":"<sha256>","checkpoint":{"log_index":5,"event_count":42,"head_hash":"<sha256>"},"retained_checkpoints":[]},"updated_at":"<time>"}
```

| Member | Rule |
|---|---|
| `revision` | From 1. Each change is a new file with the next revision. |
| `previous_anchor_sha256` | SHA-256 of the exact bytes of the previous revision file; `null` if and only if `revision` is 1 |
| `tenant_id`, `namespace_uuid` | The only scope this anchor verifies |
| `keys` | 1 to 512 pinned keys, each with `key_id` equal to the SHA-256 of `public_key`. Exactly one key, the last, has `valid_through_log_index` equal to `null`: the current key. |
| `keys[].valid_from_log_index`, `valid_through_log_index` | Log positions at which the key may sign a checkpoint. A retired key stops one position before its rotation record. |
| `keys[].approval` | `initial`, `endorsed_rotation` or `operator_recovery` |
| `reference` | `null` until a record is accepted. Otherwise the last accepted record. |
| `reference.log_index`, `statement_sha256` | The accepted record. A log must hold exactly this record at this index. |
| `reference.checkpoint` | The last signed audit position at or below the reference record, with the log index of the checkpoint that signed it |
| `reference.retained_checkpoints` | At most 64 earlier positions, oldest first, that the reference moved past **without a database comparison**. A verification with a database compares each of them. Emptied by an `anchor advance` or `anchor approve-rotation` whose database held every one of them; `anchor advance --data` does so even when no record is new, in a revision that keeps the same reference. |

The anchor is public material. Its value comes from where it is kept.

## Proof bundle

Written by `export` as one line followed by a line feed, at most 20 MiB and 8,208 entries:

```json
{"format":"ai-guard.audit.checkpoint-proof.v1","tenant_id":"tenant-a","namespace_uuid":"<uuid>","first_log_index":1,"last_log_index":5,"entries":[<record>,…],"exported_at":"<time>","notice":"…"}
```

`entries` holds complete records at contiguous log indexes from `first_log_index` to `last_log_index`, each byte for byte as in its file. The header members repeat what the entries say; they are not authenticated, and a reader refuses a bundle whose header disagrees with its entries (`bundle_malformed`). A bundle carries no key.

## Verification

Given an anchor, a set of records and optionally a database, a verifier:

1. Reads the anchor. Its tenant and namespace are the only scope: every record verified in steps 3 to 5 must carry them (`scope_mismatch`).
2. With a reference: refuses a log whose head is before the reference (`log_rolled_back`), then authenticates the records at and below the reference by the chain that the accepted record commits to, without verifying their signatures again. Going down from the reference index, each record present must be the single complete record at its index (`log_forked`) and have the expected statement hash: `reference.statement_sha256` at the reference index, then the `previous_statement_sha256` of the record above (`log_diverged_from_reference`, with `detail.log_index`). The descent stops at the first index that holds no complete record. A set may start there, or after the reference: a continuation directory and a partial export hold nothing below their first record. A complete record that stands below that index is attached to the reference by no chain and is refused (`log_discontinuous`, with `detail.missing_log_index` and `detail.unattached_log_index`). The walk of step 3 then starts at the index after the reference. Without a reference: starts at index 1.
3. For each next index up to the head of the log, requires exactly one complete record (`log_discontinuous`, `log_forked`), linked to the previous one by `previous_statement_sha256` (`log_discontinuous`) and recalling the last trusted signed position in `previous_event_count` and `previous_head_hash` (`checkpoint_regression`).
4. Stops at a `key_rotation` record: its successor is not trusted until the anchor holder approves it (`untrusted_key` with `record_kind: key_rotation`). An approval pins the rotation record as the reference of the next revision.
5. For a `checkpoint` record, requires exactly the required signers (`signature_invalid`), each pinned in the anchor (`untrusted_key`), valid at this log index (`key_not_valid_at_index`), and each signature valid over the signed message with the pinned public key (`signature_invalid`). The checkpoint must then name the control genesis of the trusted checkpoint before it: the last one verified in this step, or else the most recent checkpoint at or below the reference when the set holds it on the chain of step 2 (`scope_mismatch` with `detail.field` equal to `control_genesis`). When the set holds no earlier checkpoint, the earlier genesis is unknown, since an anchor carries none: the only requirement is then that `control_genesis_seq` does not exceed the `event_count` of the last trusted signed position.
6. With a database: reads it on one read-only snapshot with the existing integrity validators, requires the tenant and namespace of the anchor (`scope_mismatch`), then the control genesis that the trusted records signed (`scope_mismatch` with `detail.field` equal to `control_genesis`): the database must name the genesis of the most recent trusted checkpoint as defined in step 5, which is known even when no record is verified in this run as long as the set holds a checkpoint at or below the reference; and, whether or not a record at hand names the signed genesis, the genesis of the database must not stand after the oldest trusted signed position. It then requires the database to hold **every trusted signed position**, oldest first: the retained positions of the anchor, its reference position, and the position of every checkpoint verified in step 5 (`database_truncated`, `database_diverged`). A position is held when the database has at least `event_count` events and the chain hash of event number `event_count` is `head_hash`.

The verdict is `verified` when no step refused. Without a database, the verdict covers the records only. An anchor carries no control genesis: when the set holds no checkpoint at or below the reference and no database is compared, a checkpoint that names another hash at a sequence admitted by step 5 is not told from the signed one.

## Results

| Document | `format` | Printed by |
|---|---|---|
| Command result | `ai-guard.audit.checkpoint-command.v1`, with `command` and the members of the result | Every successful command other than `verify` |
| Verification report | `ai-guard.audit.checkpoint-verification.v1` | `verify`, and `anchor advance` when the verification refuses |
| Diagnostic | `ai-guard.audit.checkpoint-diagnostic.v1` | `diagnose` |
| Error | `ai-guard.audit.checkpoint-error.v1`, with `errors` | Any command that could not complete |

An error entry is `{"code":"…","message":"…","detail":{…},"action":"…"}`. The codes and their exit status are listed in the [guide](README.md#errors).

The result of `anchor advance` carries `status`: `advanced` (a new revision whose reference is the verified head), `confirmed` (no record is new; a new revision with the same reference, written because the compared database held every retained position, which it no longer retains) or `unchanged` (no file written). `anchor` holds the written revision, or `null` when the status is `unchanged`. The result of `anchor approve-rotation` is the written revision, with the status `rotation_approved`. An approval checks the record chain, checkpoint signatures and control genesis up to the first rotation record after the reference, verifies the retiring-key endorsement (or explicit recovery approval) and successor proof of possession, and, when `--data` is supplied, compares every trusted signed position with the database. It examines no record after that rotation: its `warnings` carry `records_after_rotation_not_examined` when complete records stand after the approved rotation. Its `warnings`, like those of a verification report, also carry `incomplete_records_present` when the directory holds record files that are not complete records, and `incomplete_record_above_head` only when one of them stands above the last complete record; the result does not list those files: the reports of `verify` and `diagnose` do, in `incomplete_records`.

Members of a verification report: `format`, `verdict` (`verified` or `refused`), `summary`, `scope`, `anchor` (`revision`, `sha256`), `reference`, `rollback_detection` (`anchor_reference` or `unavailable_no_reference`), `entries_checked`, `signatures_verified`, `verified_through_log_index`, `head`, `database` (`compared`, `event_count`, `head_hash`, `audit_schema_version`, `positions_compared`, `extends_checkpoint`, `events_after_checkpoint`), `incomplete_records` (`name`, `reason`, `holds_another_record`), `unexpected_entries`, `warnings`, `limits`, `errors`. A member whose value is unknown is `null`, never zero.
