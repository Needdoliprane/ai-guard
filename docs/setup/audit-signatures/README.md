# Signed audit checkpoints

[English](README.md) · [Français](README.fr.md) · [Setup index](../README.md)

This guide describes the offline administration command `ai-guard-audit-checkpoint`: it signs the head of a tenant audit chain and lets someone else verify it later with a key they approved themselves. The exact documents it reads and writes are specified in the [format reference](FORMATS.md). The implementation and its evidence are tracked in [the engineering handoff](../../engineering/evidence/2026-11-10/audit-signatures/README.md); this guide is a contract, not a qualification.

## Which deployments this guide covers

| Deployment | What this guide delivers | Executed evidence |
|---|---|---|
| Control account working on host paths: audit database, signing key and checkpoint records in private directories of that account | Every command below | Isolated Linux ARM64 QA only; see the handoff |
| Auditor workstation holding only a trust anchor and a proof bundle | `verify`, `anchor advance`, `anchor approve-rotation`, `diagnose` | Isolated Linux ARM64 QA only |
| Compose installation whose audit database lives in the `audit_data` volume | **Not served yet.** A host CLI cannot open that volume. The control image definition now packages this binary and [section 12](#12-compose-installation-one-shot-container) gives the one-shot container procedure, but neither the image build nor that procedure was ever executed. See the open gap in the handoff. | `NOT_RUN` |
| Native macOS | Same commands; the Darwin ACL and filesystem policy of the private directories was not executed | `NOT_RUN` |
| Kubernetes | Not served | `NOT_RUN` |

Nothing here is installed as a service, contacts a network or needs an external signing component.

## What a signed checkpoint proves

The audit chain of a tenant is a SHA-256 chain without a key, verified from the database itself. `agentsandbox verify-audit` without checkpoint options and `GET /api/v1/integrity` therefore detect accidental or partial damage, but not a coherent rewrite by someone who can write the SQLite file: such a rewrite recomputes the public hashes and is reported intact.

A signed checkpoint is an Ed25519 signature over a statement that binds:

- the tenant, the real control namespace of the database (`namespace_uuid`, created with the control genesis event) and that control genesis event itself, by sequence and chain hash;
- the statement contract version and the audit schema version;
- a position: the number of events and the hash of the last one;
- the fingerprint of the signing key;
- the previous record of the checkpoint log and the previous signed position.

Verification with an independently held trust anchor then establishes that **the approved key signed this chain head at this log position** and, when a database is compared, that it still holds every signed position that the anchor trusts.

It does **not** establish:

- that an event submitted by a runner is true, or that a runtime effect happened;
- anything about events appended after the last checkpoint, until the next one;
- anything about the history before the first checkpoint: the first checkpoint of an existing audit is a **baseline**, with no retroactive protection;
- a trusted time: `created_at` is declared by the signer;
- anything at all when the same administrator controls the database, the signing key and the trust anchor.

This is unrelated to broker business checkpoints and to the OIDC authentication ledger.

## Roles and trust conditions

| Role | Holds | Must not hold |
|---|---|---|
| Control operator | Audit database, private signing key, checkpoint records | The trust anchor, if independence is wanted |
| Anchor holder (auditor, second administrator, offline medium) | Trust anchor: the approved public keys and the last accepted reference | The private key, write access to the database |

- **Key protection.** The private key is one PKCS#8 file with mode `0600` in a directory with mode `0700`, both owned by the control account. It is protected by filesystem permissions only: no passphrase, hardware module or guaranteed memory erasure. The private-directory policy requires the same account for the database and for the key, so the key cannot be hidden from the control account itself: against that account or `root`, the signature adds nothing and only the trust anchor matters.
- **Anchor independence.** The anchor is public material. Its value comes from being stored where the control account and the storage administrator cannot rewrite it: another machine, another administrator, or a file owned by `root` and only readable by the control account. The command accepts an anchor owned by `root` or by the current account.
- **Verifier binary.** Verify with a binary you trust, on a machine you trust.
- **Supported storage.** Private directories and input files must be on a supported local filesystem (ext4, XFS, Btrfs, tmpfs, overlayfs, F2FS on Linux; APFS/HFS with ownership enabled on macOS). FAT/exFAT, network and FUSE mounts are refused: copy the anchor to a local disk before use.

## Prerequisites

- The binary, built with the project toolchain: `cargo build --release --locked` produces `target/release/ai-guard-audit-checkpoint`.
- An audit database at schema version 2 or later, in a private directory (`0700`) of the control account, with an ASCII file name and mode `0600`. A schema 1 database is refused with `audit_namespace_unavailable`; run the explicit maintenance `agentsandbox migrate-control --target-version 2` first.
- A checkpoint directory and a key directory that are **not** the directory of the database. The key directory is never the checkpoint directory or a directory under it: the checkpoint directory is evidence that you copy, export and back up, and the private key must not travel with it.

The examples use three locations; `key init` and `create --new-log` create the missing directories privately (`0700`):

```sh
AUDIT_DB=/path/to/control/audit.sqlite   # the --data file of the gateway
SIGNING=/path/to/audit-signing           # private area of the control account, outside the database directory
TRANSFER=/path/to/handover               # public files prepared for the anchor holder
```

The database is opened read-only on one consistent snapshot and is never rewritten. A gateway may keep running: the command does not hold the database initialization lock during its scan and does not block writers. Reading may create empty `-wal`/`-shm` files and the reserved `.ai-guard-sqlite-open.lock` file next to the database.

Every command prints exactly one JSON document on standard output; only `--help` prints usage text. Exit status `0` is success, `2` is an integrity, trust or scope refusal, `1` is an operational error (including an invalid command line). Any non-zero status means that nothing was verified.

## 1. Create the signing key

```sh
ai-guard-audit-checkpoint key init --key-dir "$SIGNING/key-1"
```

The directory must not exist; it is created with mode `0700`, and an existing path is never reused, even after an interrupted initialization (choose a new path). The output carries the public fingerprint `key_id` (SHA-256 of the raw public key) and never the private key.

Export the public key document for the anchor holder, and transmit the fingerprint through a **separate channel** (read it aloud, print it, send it by another medium):

```sh
ai-guard-audit-checkpoint key show --key-dir "$SIGNING/key-1" --out "$TRANSFER/key-1.public-key.json"
```

## 2. Create the first checkpoint

```sh
ai-guard-audit-checkpoint create --new-log \
  --data "$AUDIT_DB" --tenant-id tenant-a \
  --key-dir "$SIGNING/key-1" --checkpoint-dir "$SIGNING/checkpoints"
```

`--new-log` starts a checkpoint log and is given **once**. It is required, and only accepted, while the directory holds no complete record: the first record is a baseline, and starting one is a decision. Every later run, in particular the line you give to a scheduler, omits it:

```sh
ai-guard-audit-checkpoint create \
  --data "$AUDIT_DB" --tenant-id tenant-a \
  --key-dir "$SIGNING/key-1" --checkpoint-dir "$SIGNING/checkpoints"
```

Without `--new-log`, a directory that does not exist is refused with `checkpoint_log_unavailable` and one that holds no complete record with `checkpoint_log_empty`; nothing is created, not even the writer lock file `.writer.lock`, and the same holds for `rotate-key`. A mistyped `--checkpoint-dir` therefore fails instead of starting a second log while the real one stops growing. A start that is refused for another reason (wrong tenant, schema too old) leaves no directory behind, and a start that is refused because of what an existing directory holds writes nothing in it, not even `.writer.lock`: a complete record (`invalid_argument`, the log is already started), a record file that holds another record than the one its name announces (`record_name_mismatch`), two complete records at the head (`log_forked`) or the maximum number of record files (`checkpoint_log_full`).

The output names `tenant_id`, `namespace_uuid`, `log_index`, `event_count`, `head_hash`, the record file, and `baseline: true` for log index 1. The namespace can also be read beforehand with `diagnose --data "$AUDIT_DB" --tenant-id tenant-a` (section 11). Records are immutable files named `cp-<log index>-<statement sha256>.json`; they are never rewritten or removed by this tool.

How often to run `create` is an operator decision made with the scheduler of your choice; this tool installs none. The interval bounds the unsigned tail of the audit.

## 3. Initialise the trust anchor

The anchor holder, on their own storage, approves the key **by the fingerprint received through the separate channel**, for one tenant and one control namespace:

```sh
ai-guard-audit-checkpoint anchor init \
  --public-key key-1.public-key.json \
  --expect-key-id <64 hex characters received separately> \
  --tenant-id tenant-a --namespace-uuid <namespace_uuid> \
  --out tenant-a.anchor-1.json
```

A public key file whose fingerprint differs from `--expect-key-id` is refused with `key_id_mismatch` and nothing is written. A key that travels inside the verified material is never trusted: records carry no key, and a record that tries to carry one is refused as malformed.

## 4. Verify

```sh
ai-guard-audit-checkpoint verify \
  --anchor tenant-a.anchor-1.json \
  --checkpoint-dir "$SIGNING/checkpoints" --data "$AUDIT_DB"
```

The report (`ai-guard.audit.checkpoint-verification.v1`) carries a `verdict` (`verified` or `refused`), the trusted log position (`verified_through_log_index`), the comparison with the database, warnings, the limits that apply and the list of errors. Without `--data`, `database.compared` is `false` and the verdict covers the checkpoint records only: it says what the key signed, and nothing about the audit history itself.

With `--data`, the database must hold **every trusted signed position**, oldest first: the positions the anchor retains and references (section 5) and the position of every checkpoint verified after the reference. `database.positions_compared` counts them. The first position that the database does not hold refuses the verification with `database_truncated` or `database_diverged`; `detail.signed_at_log_index` and `detail.position_source` (`anchor_retained`, `anchor_reference` or `checkpoint_record`) name it.

The database must also have the **control genesis** that the trusted records signed. That event is written once, with the control namespace, and no migration moves it: a database that names another one is refused with `scope_mismatch` (`detail.field: control_genesis`), even when it holds every signed position and no record is new. The signed genesis is read from the most recent trusted checkpoint: one verified after the reference, or else the most recent checkpoint at or below the reference when the directory or bundle holds it. When no record at hand names it (a bundle that carries a rotation record alone, for example), the comparison is weaker: the genesis of the database must not stand after a trusted signed position. Between records the rule is the same: a checkpoint that names another control genesis than the trusted checkpoint before it is refused with that code, with or without a database.

`database.events_after_checkpoint` is the unsigned tail: events appended since the last signed position.

A refusal that is already established is never replaced by a later failure: if the records are refused and the database then cannot be read, the report still carries the refusal, with the reading failure next to it and `database.compared: false`.

To verify a **copy** of the database elsewhere, do not copy the main file of a running database: stop the gateway and every other consumer, check that no `-wal` file remains or that it is empty, then copy the main file into a private directory (`0700`) with mode `0600` and an ASCII name. Verification in place needs no copy and does not block the gateway.

## 5. Record the reference: rollback detection

A freshly initialised anchor only says which key to trust. An older database restored together with an older checkpoint log is correctly signed and verifies: the report states `rollback_detection: unavailable_no_reference`. Rollback is detected only against a **reference** kept by the anchor holder:

```sh
ai-guard-audit-checkpoint anchor advance \
  --anchor tenant-a.anchor-1.json \
  --checkpoint-dir "$SIGNING/checkpoints" --data "$AUDIT_DB" \
  --out tenant-a.anchor-2.json
```

The command verifies first. On success it writes a **new revision** whose `reference` is the verified head (log index, statement hash and signed audit position) and whose `previous_anchor_sha256` is the SHA-256 of the previous revision file. No file is ever overwritten: keep every revision. When no record is new, the status is `unchanged` and no file is written, with one exception: if the anchor retains earlier positions (see below) and `--data` is given, the comparison confirms them and the command writes a revision with the same reference and an empty `retained_checkpoints`, with the status `confirmed`. A refused verification exits with status `2` and writes nothing. The reference never moves backwards.

With a reference:

- a checkpoint log that ends before it is refused with `log_rolled_back`;
- another record at the reference position is refused with `log_diverged_from_reference`;
- a record at or below the reference is accepted only as part of the chain that the reference commits to. Such records are not verified by signature again: each one present is matched, by statement hash, to the record that the one above it names. A second complete record at one index is refused with `log_forked`, another record with `log_diverged_from_reference`, and a record below a log index that holds none with `log_discontinuous` (section 6);
- a compared database that is shorter than, or different at, the referenced position is refused with `database_truncated` or `database_diverged`, **whatever was signed after the reference**: a later record signed by the pinned key does not make the tool forget the position that the anchor pinned.

`verify --require-reference` refuses an anchor that has no reference yet (`no_independent_reference`); use it wherever rollback detection is expected.

### Advance with the database whenever you can

Records alone prove that the pinned key signed a new head. They do not prove that the new head extends the history pinned so far: only a database comparison does.

- `anchor advance --data` compares the database with every trusted position, then pins the last one alone.
- `anchor advance` without `--data` (an auditor who holds only a bundle) still moves the reference, so that a shorter log is detected. The positions pinned so far are **not dropped**: they are kept in the new revision under `reference.retained_checkpoints`, the outcome carries the warning `reference_advanced_without_database`, and every later `verify --data` compares each of them. The next `anchor advance --data` confirms the continuity and empties the list, whether or not a record was signed in between: when none was, the status is `confirmed` instead of `advanced`. A database that does not hold one of them refuses that advance like any other verification.
- A revision retains at most 64 such positions. Beyond that, an advance without `--data` fails with `database_comparison_required`.

Rollback detection holds **at and below the reference only**, and for the audit history only when a database is compared. Between the reference and the head, whoever can write the log and the database can return both to an earlier signed state without detection, even without the key, and whoever holds the key can rewrite what follows the reference. How often the anchor holder advances bounds that window.

## 6. Interrupted writes and concurrent writers

- A record is created exclusively, synchronised with its directory, then read back. A file with a record name that does not parse as a record is an **incomplete record**: it is listed in `incomplete_records`, ignored for the chain and never removed. `verify`, `anchor advance` and `anchor approve-rotation`, which read every record, add the warning `incomplete_records_present`; the outcome of `anchor approve-rotation` does not list the files, so run `verify` or `diagnose` for their names. `create` resumes at the same log index.
- An incomplete record above the last complete one is what an interrupted write leaves, and also what truncating a signed record leaves. `verify` cannot tell them apart: it trusts the log through `verified_through_log_index` only and adds the warning `incomplete_record_above_head`, as `anchor advance` and `anchor approve-rotation` do. Like a deleted record, this is detected only by an anchor whose reference is at or after that record.
- A file with a record name that holds a complete record **other than the one its name announces** (for example a record edited in place) cannot come from an interrupted write. `verify`, `anchor advance`, `anchor approve-rotation`, `export`, `create` when extending an existing log, and `rotate-key` read every record and refuse the directory with `record_name_mismatch`: no anchor revision, bundle or record is then written. The file is never removed: preserve it, investigate, and move it out of the directory by an explicit decision.
- If a record was made durable but the command was interrupted before reporting, running `create` again returns `status: unchanged` with the same record.
- An interrupted `key init` or `--out` leaves a path that is never reused: choose a new one.
- A second writer on the same directory is refused with `checkpoint_log_busy`.
- A command that reads the directory while another one is writing a record takes no lock. It reports the file being written as an incomplete record, or fails with `checkpoint_log_unavailable` when that file changes while it is read (exit status `1`, no verdict): run it again.
- Two complete records at the same log index are refused with `log_forked`: by `create` when extending an existing log and by `rotate-key` anywhere in the directory, by `verify` and `anchor advance` anywhere in the directory or bundle, by `anchor approve-rotation` up to the rotation it approves, and by `export` in the range it exports.
- A complete record counts only when a chain attaches it to a trusted record: the anchor reference for `verify`, `anchor advance` and `anchor approve-rotation`, the head of the log for `create` and `rotate-key`. Below that record, each index must hold the single record that the record above it names by hash. Another record there is refused (`log_diverged_from_reference` below a reference, `log_discontinuous` below the head), and so is a record that stands below an index holding no complete record (`log_discontinuous`, with `detail.missing_log_index` and `detail.unattached_log_index`). A record deleted or damaged in the middle of the log is therefore a refusal, not a warning: restore it from a backup of the directory, or move the older records out by an explicit decision. Material that simply starts later (a continuation directory, a partial export, a copy from some index up) is not concerned: nothing is required below its first record. `diagnose` shows the whole structure (`log_forked`, `log_gap`, `log_link_broken`) without deciding anything.
- For that check, `create` when extending an existing log and `rotate-key` read every record of the directory at each run, under the writer lock: one file of at most 2,048 bytes per record, at most 8,208 of them.
- `create` never signs a database that is shorter than, or different at, the position signed by the head of the log **as found in the directory** (`database_truncated`, `database_diverged`). This guards against accidents; it is not a protection against whoever can write both the database and the checkpoint directory. That actor can remove the last records, rewrite the events they covered and let the next scheduled `create` sign the result, without ever reading the key. `create` consults no anchor: only `verify` with a reference at or after the removed records detects it, which is one more reason to advance the anchor often.
- `create` never signs either a database whose control genesis is not the one named by the last checkpoint of the log, or stands after the last signed position (`scope_mismatch`, `detail.field: control_genesis`): the log would otherwise carry the new genesis from then on.

## 7. A full checkpoint directory

A checkpoint directory holds at most 8,192 record files. Beyond that, `create` fails with `checkpoint_log_full` and names the last record in `detail.last_record`. Continue in a new directory, linked to that record:

```sh
ai-guard-audit-checkpoint create \
  --data "$AUDIT_DB" --tenant-id tenant-a --key-dir "$SIGNING/key-1" \
  --checkpoint-dir "$SIGNING/checkpoints-2" \
  --previous-record "$SIGNING/checkpoints/cp-<log index>-<sha256>.json"
```

`--previous-record` starts the new directory (no `--new-log` is needed) and is accepted only while it holds no complete record. Pass the file under the name it has in the old directory: a file that holds another record than the one its name announces, or a copy of the record under another name, is refused with `record_name_mismatch` (`detail.expected_name` gives the name of the record that the file holds) before anything is created, not even the new directory. The new directory starts at the next log index, so it verifies only with an anchor whose reference is at or after the last record of the old directory: run `anchor advance` on the old directory before switching. Keep the old directory.

When the predecessor already signs the database head, continuation returns `unchanged` only after checking an existing destination without a writer lock. A complete record there is refused (`invalid_argument`, exit status `1`); a fork at the scanned head or a renamed record is refused (`log_forked` or `record_name_mismatch`, exit status `2`). This first-record check can stop at the first complete index: it does not authenticate an already-started destination. Use `create` without `--previous-record` after the first record. An absent destination remains absent; an empty destination stays empty; incomplete files are retained and listed. No record or lock file is created by this no-op or these refusals. The inspection takes no lock, so another process can change the directory after the check.

Rotation records may use 16 files beyond that bound, so that a full directory never prevents retiring a key. A proof bundle carries at most 8,208 records, which is everything one directory can hold.

## 8. Backup and restore

Restoring an older database is, for this mechanism, a rollback: with a reference it is detected and refused, which is the purpose. There is deliberately no signed "accept this regression" record, because it would let the key holder alone hide a rollback. A legitimate restore is an explicit break decided by the anchor holder:

1. Keep the old checkpoint directory, every old anchor revision and the restored database for the record.
2. Create a **new signing key** (`key init`) for the new log. The old log is still a valid chain under the old key: an anchor that pins the old key and has no reference yet would verify the abandoned log, with the database that goes with it, exactly as well as the new one.
3. Start a new checkpoint directory with `create --new-log` and the new key.
4. Create a new anchor with `anchor init` that pins the new key **only**.
5. Record the reference at once with `anchor advance --data`: until then the new anchor detects no rollback at all.

Back up the checkpoint directory and every anchor revision like any other evidence. Losing the checkpoint records does not damage the audit, but nothing signed remains to verify it against.

## 9. Rotate the signing key

A rotation is three explicit steps by two people.

1. The operator creates the successor key and exports its public document. The fingerprint travels through the separate channel, as in section 1.

   ```sh
   ai-guard-audit-checkpoint key init --key-dir "$SIGNING/key-2"
   ai-guard-audit-checkpoint key show --key-dir "$SIGNING/key-2" --out "$TRANSFER/key-2.public-key.json"
   ```

2. The operator records the hand-over in the log. The retiring key endorses its successor, and the successor proves possession of its key.

   ```sh
   ai-guard-audit-checkpoint rotate-key \
     --checkpoint-dir "$SIGNING/checkpoints" \
     --key-dir "$SIGNING/key-1" --new-key-dir "$SIGNING/key-2"
   ```

   From then on `create` must use `--key-dir "$SIGNING/key-2"`; the retired key is refused with `key_id_mismatch`. Running the same `rotate-key` again after an interruption returns `status: already_rotated` and writes nothing.

3. The anchor holder approves the successor explicitly, with the fingerprint received separately.

   ```sh
   ai-guard-audit-checkpoint anchor approve-rotation \
     --anchor tenant-a.anchor-2.json --checkpoint-dir "$SIGNING/checkpoints" \
     --public-key key-2.public-key.json \
     --expect-key-id <64 hex characters received separately> \
     --out tenant-a.anchor-3.json
   ```

Until that approval, `verify` and `anchor advance` stop before the rotation record with `untrusted_key` (`detail.record_kind: key_rotation`). An endorsement is not an approval: a compromised retiring key could introduce any successor.

Add `--data "$AUDIT_DB"` to `anchor approve-rotation` whenever the database is at hand. The approval moves the reference to the rotation record, past the checkpoints that the retiring key signed since the previous reference. As with `anchor advance` (section 5), the database is then compared with every trusted position up to the rotation; without it, the positions pinned so far are retained in the new revision and the outcome carries `reference_advanced_without_database`.

The approval requires that the directory holds no record file that contains another record than the one its name announces (`record_name_mismatch`, section 6), that every record at and below the reference is part of the chain the reference commits to (section 6), that every record between the reference and the rotation verifies, that the rotation retires the current key of the anchor, that three fingerprints agree (the expected one, the public key file and the rotation record), that the retiring key pinned in the anchor endorsed the rotation, and that the successor signed it. The new revision then **pins the rotation record itself as its reference** and closes the retired key one position before it. After approval the retired key cannot sign anything at or after the rotation (`key_not_valid_at_index`), and no other record can stand in place of the rotation (`log_diverged_from_reference`, `log_rolled_back`). An incomplete record does not refuse the approval: the outcome carries the warning `incomplete_records_present` and, only when an incomplete record stands above the last complete record, `incomplete_record_above_head` (section 6). The approval examines the records up to the rotation only: when records stand after it, the outcome carries the warning `records_after_rotation_not_examined`, and `verify` then `anchor advance` with the new revision examine them.

### Lost or compromised key

When the retiring key cannot or must not sign, the operator records an unendorsed rotation and the anchor holder approves it as an explicit recovery:

```sh
ai-guard-audit-checkpoint rotate-key \
  --checkpoint-dir "$SIGNING/checkpoints" --new-key-dir "$SIGNING/key-2" \
  --unendorsed-recovery --retiring-key-id <fingerprint of the retiring key>

ai-guard-audit-checkpoint anchor approve-rotation \
  --anchor tenant-a.anchor-2.json --checkpoint-dir "$SIGNING/checkpoints" \
  --public-key key-2.public-key.json --expect-key-id <64 hex characters> \
  --unendorsed-recovery --out tenant-a.anchor-3.json
```

Without `--unendorsed-recovery` the approval is refused with `rotation_not_endorsed`. With it, the key is pinned with `approval: operator_recovery` and every later report carries the warning `unendorsed_key_approval`. `--unendorsed-recovery` is refused for a rotation that is endorsed.

After a compromise, the records between the previous anchor reference and the rotation were signed by the compromised key: **they are not trustworthy**, and only what the reference already covered is. Advancing the anchor often bounds that exposure.

## 10. Export a proof for an auditor

```sh
ai-guard-audit-checkpoint export \
  --checkpoint-dir "$SIGNING/checkpoints" --out "$TRANSFER/tenant-a.proof.json"
```

The bundle (`ai-guard.audit.checkpoint-proof.v1`) carries contiguous records and **no key, public or private**. `--from-log-index` and `--through-log-index` select a range; the default is everything available. A bundle carries at most 8,208 records and 20 MiB: beyond that, `export` fails with `invalid_argument` and asks for a range, rather than writing a bundle that `verify` would refuse. Its header fields are derived from the entries and are not authenticated: the verifier reads the entries.

The auditor needs only the bundle and their own anchor, with no database, key or checkpoint directory:

```sh
ai-guard-audit-checkpoint verify \
  --anchor tenant-a.anchor-2.json --bundle tenant-a.proof.json \
  --require-reference --report-out tenant-a.report.json
```

- `--bundle` replaces `--checkpoint-dir` in `verify`, `anchor advance` and `anchor approve-rotation`.
- A bundle alone says what the key signed. Advancing the anchor from a bundle without `--data` keeps the positions pinned so far under `reference.retained_checkpoints` (section 5); they are compared the next time a database is given.
- `--report-out` writes a copy of the report, verified or refused, to a new file.
- Without `--data` the verdict covers the records only (`database.compared: false`); with a consistent copy of the database (section 4) it also covers its content.
- A bundle may start after the anchor reference if its first record links to it. A bundle that ends before the reference is refused with `log_rolled_back`. A bundle that carries records at or below the reference is refused unless they are the chain that the reference commits to (`log_diverged_from_reference`): no record of a verified bundle is left unexamined.
- Any invalid entry refuses the whole bundle (`bundle_malformed`, `record_malformed`, `record_not_canonical`).

The bundle proves what the checkpoint records prove. It does not make the existing event exports of the gateway verifiable by themselves.

## 11. Diagnose

```sh
ai-guard-audit-checkpoint diagnose \
  --data "$AUDIT_DB" --tenant-id tenant-a \
  --checkpoint-dir "$SIGNING/checkpoints" --key-dir "$SIGNING/key-1" \
  --anchor tenant-a.anchor-2.json
```

Name any subset of the four pieces. `diagnose` verifies no signature and makes **no trust decision**: it explains state. Each section (`database`, `checkpoint_log`, `signing_key`, `trust_anchor`) says whether it is `readable` and carries its own `error` otherwise; an unreadable piece is never shown as empty. `findings` lists what deserves attention:

| Finding | Meaning |
|---|---|
| `unsigned_tail` | Events appended after the last signed position (`detail.events_after_last_checkpoint`) |
| `incomplete_records_present`, `unexpected_entries_present`, `rejected_entries_present` | Debris of interrupted writes, stray entries, or entries that break the private storage policy and make `create`/`verify` refuse the directory |
| `record_name_mismatch` | Record files that hold another complete record than the one their name announces; `verify`, `anchor advance`, `anchor approve-rotation`, `export`, `create` and `rotate-key` refuse the directory |
| `log_forked`, `log_gap`, `log_link_broken` | Structural defects of the log. `log_gap` gives the number of missing positions and the first missing ranges (`detail.missing_ranges`). `create`, `rotate-key`, `verify` and `anchor advance` refuse a directory that shows one (section 6) |
| `scope_differs` | Database, log and anchor do not share tenant and namespace |
| `database_shorter_than_last_checkpoint`, `database_differs_at_last_checkpoint` | The database does not extend the last record of the log |
| `key_is_not_next_signer` | The key directory is not the key the log expects next |
| `anchor_has_no_reference`, `records_after_anchor_reference`, `anchor_reference_ahead_of_log` | Where the anchor reference stands relative to the log |
| `anchor_retains_uncompared_positions` | The reference moved past earlier positions without a database comparison; `anchor advance --data` confirms the continuity and writes a revision that retains nothing, even when no record is new |
| `next_signer_not_current_anchor_key` | A rotation may be waiting for `anchor approve-rotation` |

Use it to read the `namespace_uuid` of a database before the first checkpoint, and to check the chain of anchor revisions by hand: `trust_anchor.sha256` of one revision is the `previous_anchor_sha256` of the next.

## 12. Compose installation: one-shot container

**`NOT_RUN`: this procedure was written from the image definition and was never executed. Neither the image build nor any command below is evidence.** It follows the offline maintenance pattern of the [named local access guide](../identity/README.md): a one-shot container of the control image, running as the control UID against the existing audit volume, without network, socket or credential.

`images/control.Dockerfile` builds and installs `ai-guard-audit-checkpoint` next to `agentsandbox`. Inspect the existing `/data` volume name on the gateway container and the ID of the control image you built; do not guess either. The gateway may keep running for `create`: the database is opened read-only.

```sh
AIGUARD_CONTROL_IMAGE_ID="sha256:inspected-current-control-image-id"
AIGUARD_AUDIT_VOLUME="existing-inspected-audit-volume"
(
set -eu
docker volume inspect --format '{{.Name}}' "$AIGUARD_AUDIT_VOLUME"
docker run --name ai-guard-checkpoint-key-1 --pull never \
  --user 1000:1000 --network none --read-only --cap-drop ALL \
  --security-opt no-new-privileges:true --cpus 1 --memory 384m --memory-swap 384m --pids-limit 128 \
  --tmpfs /tmp:rw,noexec,nosuid,nodev,size=16m,mode=1777 \
  --mount "type=volume,source=$AIGUARD_AUDIT_VOLUME,target=/data,volume-nocopy" \
  --entrypoint ai-guard-audit-checkpoint "$AIGUARD_CONTROL_IMAGE_ID" \
  key init --key-dir /data/audit-signing/key-1
docker run --name ai-guard-checkpoint-first --pull never \
  --user 1000:1000 --network none --read-only --cap-drop ALL \
  --security-opt no-new-privileges:true --cpus 1 --memory 384m --memory-swap 384m --pids-limit 128 \
  --tmpfs /tmp:rw,noexec,nosuid,nodev,size=16m,mode=1777 \
  --mount "type=volume,source=$AIGUARD_AUDIT_VOLUME,target=/data,volume-nocopy" \
  --entrypoint ai-guard-audit-checkpoint "$AIGUARD_CONTROL_IMAGE_ID" \
  create --new-log --data /data/audit.sqlite --tenant-id tenant-a \
  --key-dir /data/audit-signing/key-1 --checkpoint-dir /data/audit-signing/checkpoints
)
```

The other commands use the same `docker run` options, a new container name, and these arguments after the image:

```text
key show --key-dir /data/audit-signing/key-1 --out /data/audit-signing/key-1.public-key.json
create --data /data/audit.sqlite --tenant-id tenant-a --key-dir /data/audit-signing/key-1 --checkpoint-dir /data/audit-signing/checkpoints
export --checkpoint-dir /data/audit-signing/checkpoints --out /data/audit-signing/tenant-a.proof-1.json
diagnose --data /data/audit.sqlite --tenant-id tenant-a --checkpoint-dir /data/audit-signing/checkpoints --key-dir /data/audit-signing/key-1
```

Each container name and each output path must be new; `--out` files go to the existing private directory `/data/audit-signing`, which `key init` created. Check each exit status and JSON document before continuing. Copy the public key document and the bundle out of a retained container with `docker cp` (without `--archive`) and hand them to the anchor holder; never copy the key directory.

What this layout does and does not give:

- The key lives in the same volume as the database, under the same UID. Against the administrator of that volume or the control account, the signature adds nothing: only the anchor held elsewhere matters.
- The anchor, `verify` and `anchor advance` stay **outside** the volume, on the machine of the anchor holder, with a binary built for that machine. Under Docker Desktop an anchor or a bundle cannot be handed to the container through a bind mount: the private file policy refuses FUSE and network filesystems.
- Verifying the database content needs a consistent copy of it on that machine (section 4), or a run of `verify --data` inside a one-shot container with an anchor copied into the volume, which is then no longer independent of the volume.

## Errors

Every error has a `code`, a `message`, a `detail` object and an `action`.

| Code | Exit | Meaning | Expected action |
|---|---|---|---|
| `signature_invalid` | 2 | A signature is missing, extra or does not verify over the exact statement bytes | Do not trust the record; compare the log with an independent copy |
| `untrusted_key` | 2 | The record is signed by a key the anchor does not pin | Only keys approved in the anchor are accepted |
| `key_id_mismatch` | 2 | A fingerprint differs from the expected one | Compare the expected key, the key file and the record |
| `key_not_valid_at_index` | 2 | A pinned key signed outside the log positions it may sign, typically a retired key after its rotation | Treat the record as untrusted and investigate the use of the retired key |
| `rotation_not_endorsed` | 2 | The retiring key did not endorse the rotation, or its endorsement does not verify | Approve only as an explicit recovery, after an out-of-band decision |
| `scope_mismatch` | 2 | Tenant, control namespace or control genesis differs between anchor, records and database (`detail.field`, `detail.between`) | Use the database, log and anchor of the same tenant and namespace; a recreated database is a new namespace. A control genesis never changes: treat the database or the record that names another one as altered |
| `log_rolled_back` | 2 | The log ends before the anchor reference | Restore the missing records, or have the anchor holder decide on a documented break |
| `log_diverged_from_reference` | 2 | Another record stands at the reference position, or at a position below it, than the one the reference chains to (`detail.log_index`) | Treat the log as replaced |
| `log_discontinuous` | 2 | A record is missing at an expected log index, does not link to its predecessor, or stands below an index that holds none (`detail.missing_log_index`, `detail.unattached_log_index`) | Restore the complete checkpoint log |
| `log_forked` | 2 | Two complete records claim the same log index | Preserve both and investigate |
| `checkpoint_regression` | 2 | A record does not recall the last signed position | Treat the log as inconsistent |
| `database_truncated` | 2 | The database holds fewer events than a trusted signed position (`detail.position_source`, `detail.signed_at_log_index`) | Treat as rollback or truncation |
| `database_diverged` | 2 | The database does not hold the signed hash at a trusted signed position | Treat the history as rewritten; preserve database and records |
| `audit_chain_invalid` | 2 | The existing integrity reader already refuses the database, or the file is not a SQLite database | Preserve it and investigate |
| `audit_schema_unversioned` | 2 | The database carries no schema version: it is empty or truncated, or a legacy database that the gateway has not upgraded yet (`detail.schema_objects`) | If a checkpoint was ever signed, treat as truncation; a legacy database is opened once by the gateway, then migrated with `migrate-control --target-version 2` |
| `audit_namespace_unavailable` | 2 | Schema 1 database without control namespace | Run `migrate-control --target-version 2` |
| `record_malformed`, `record_not_canonical`, `bundle_malformed` | 2 | A record or a bundle is not the strict, canonical form of its content | Obtain the records again |
| `record_name_mismatch` | 2 | A record file, in a directory or given to `--previous-record`, holds a complete record other than the one its name announces (`detail.records`) | Treat the directory as altered: preserve it and compare it with an independent copy or export |
| `no_independent_reference` | 2 | `--require-reference` and the anchor has no reference | Run `anchor advance` after a verification you accept |
| `checkpoint_log_busy`, `checkpoint_log_full`, `checkpoint_log_empty`, `checkpoint_log_unavailable`, `checkpoint_record_exists` | 1 | The checkpoint directory cannot be written or read now, does not exist, or holds no complete record | Retry, continue in a new directory, fix the path or the directory policy, or start the log with `--new-log`; unexpected entries are never removed |
| `key_directory_exists`, `key_unavailable`, `key_invalid` | 1 | The key directory cannot be created or used | Choose a new directory, or restore the intended key |
| `no_pending_rotation`, `anchor_key_limit` | 1 | Nothing to approve after the reference, or the anchor already pins 512 keys | Record a rotation first, or have the anchor holder decide on a new log and anchor |
| `database_comparison_required` | 1 | The anchor already retains 64 positions advanced without a database | Advance with `--data` |
| `anchor_unavailable`, `anchor_invalid`, `input_unavailable` | 1 | An input file cannot be read under the trusted local file policy, or is not what this tool wrote | Copy it to a local disk with the required ownership; never edit an anchor |
| `audit_unavailable`, `audit_schema_unsupported` | 1 | The database cannot be opened under the private storage policy or is busy, or its schema version is newer than this build knows | Fix the path and permissions, retry, or use a matching build |
| `output_exists`, `output_unavailable`, `io_failure`, `invalid_argument` | 1 | Output path or arguments | Choose a new output path; nothing is overwritten |

A `checkpoint_log_unavailable` error is also raised when the directory holds an entry that is not a private file (for example a file with mode `0644`): `create` and `verify` fail closed rather than skip it. Entries with an ordinary name and private mode that are not records are listed in `unexpected_entries` with the warning `unexpected_entries_present`.

## Limits

- No resistance to an administrator who controls the data, the key and the anchor at the same time.
- Events after the last checkpoint are covered only by the next one.
- Without a reference in the anchor, a rollback to an earlier signed state is not detected; the report says so. With a reference, detection holds at and below it only.
- With the key and the database, an actor can rewrite what follows the anchor reference. What is at and below the reference is protected while the anchor is intact **and a database is compared with it**: a verification of the records alone, from a bundle or a directory, says nothing about the audit history.
- Above the anchor reference, `create` offers no protection against an actor who writes both the database and the checkpoint directory: the next run signs what it finds.
- A truncated record file above the head of the log is indistinguishable from an interrupted write and is worth a deleted record: only the anchor reference detects it.
- The trust anchor carries no control genesis: the signed one is known only from a checkpoint record. When the material holds no checkpoint at or below the reference and no database is compared, a checkpoint that names another genesis hash at an admissible sequence is not detected.
- The key is protected by filesystem permissions only.
- `created_at` is not a trusted timestamp; there is no transparency log or external anchoring.
- `GET /api/v1/integrity` and `Store::integrity` are unchanged: they still report a coherently rewritten database as intact, and neither renders the verdict of the signed checkpoints. `agentsandbox verify-audit --checkpoint-dir … --anchor … [--data …]` additionally prints the offline signed report after the gateway answer and fails on refusal or error; without these options its output is unchanged. The operator supplies matching scopes: the two documents are not a combined authority verdict. Without a database only signed material is checked, and without an anchor reference rollback detection remains unavailable. The composed command is not yet exercised in its integration QA. The gateway holds no trust anchor and must not hold one: an anchor kept by the control it verifies is not independent. Detection is available through this command, run by whoever holds the anchor, and an independently held anchor.
- This tool signs each tenant Store database only; it does not sign the separate authentication audit V4. Setup manifest2/configuration2 keeps those stores separate and adds no signing configuration. The `verify-audit` wrapper still reads the legacy `--config-dir/secrets/viewer` layout; it does not resolve a SetupV2 installation or select a tenant from `tenants.json`. Use the standalone offline tool with the explicitly chosen tenant database. No SetupV2 command journey is qualified by the wrapper test.
- The signed head binds the event chain and the control projections that the existing reader reconstructs from it; it does not make the existing event exports verifiable by themselves.
