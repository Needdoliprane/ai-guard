# Journal / SQLite coexistence probe

Scope: only `tests/journal_sqlite.rs` is owned by this task. Four targeted runs are recorded; the earlier FAIL results remain below for auditability.

## Test behavior

`hardlink_alias_is_refused_without_releasing_sqlite_locks` uses real private temporary directories, a real Store and a separate rusqlite connection; main/WAL/SHM exist. It accepts a journal operation with explicit `BindingInputs::None`, moves the old `current.json` to a fixture backup outside the namespace, then hardlinks the actual SQLite main inode at `current.json`.

Journal open/refusal runs in the same process as the live Store. The implementation refuses with `JournalError::UnsafeFilesystem` during its metadata inventory before opening the hardlink. Only two Rust child invocations are used: each runs the existing F_GETLK plus `PRAGMA journal_mode=DELETE` probe (before and after refusal). The parent retains a main-file FD across the entire proof and uses `FileExt::read_at` to compare DB bytes without reopening/closing the inode. It then removes only its injected hardlink, checks Store append and integrity, and confirms nlink returns to one.

The case demonstrates refusal of an injected known hardlink only. It does not assert universal SQLite detection, tenant/cloud behavior, or protection from same-UID/root mutation. No engine, model, API call, runtime, or E2E was used.

## Run history

### Run 1 — FAIL: fixture UUID mismatch

Command: `cargo test --locked --test journal_sqlite` — exit 101.

The initial fixture reserved an operation UUID that differed from the plan fixture, so `accept` returned `Conflict` before the hardlink case. The test was corrected to use the fixture's exact operation UUID; an unused constant was also removed. Exact run output: `/tmp/ai-guard-journal-sqlite-cargo.log`.

### Run 2 — FAIL: child probe recursively ran the test body

Command: `cargo test --locked --test journal_sqlite` — exit 101.

The first probe child entered the complete test function because its environment dispatch returned only from the helper. Diagnostic output: `/tmp/ai-guard-journal-sqlite-diagnostic.log`. Fixed by making top-level test dispatch run only the child probe then return; F_GETLK and PRAGMA assertions were preserved.

### Run 3 — FAIL: test itself closed a DB descriptor

Command: `cargo test --locked --test journal_sqlite` — exit 101.

The child probe passed before Journal, while the after-Journal F_GETLK probe reported `F_UNLCK`. Root cause was the test's own `fs::read(&db_path)`: opening and closing that same inode in the Store PID releases its POSIX locks. Diagnostic output: `/tmp/ai-guard-journal-sqlite-diagnostic3.log`. Fixed by retaining a database `File` descriptor from setup, comparing bytes with positional `read_at`, and running F_GETLK immediately after Journal refusal. No application code or assertions were weakened.

### Run 4 — PASS

Command: `cargo test --locked --test journal_sqlite`

Exit code: 0.

Exact output in `/tmp/ai-guard-journal-sqlite-diagnostic4.log`:

```text
Compiling agentsandbox v0.1.0 (...)
Finished `test` profile ...
Running tests/journal_sqlite.rs (...)
running 1 test
test hardlink_alias_is_refused_without_releasing_sqlite_locks ... ok

test result: ok. 1 passed; 0 failed; 0 ignored
```

## State manifest for passing run

Source SHA-256 before and after Run 4: `55375d225c09c312304e0e9dcd5bf64747a00451fbba83cbd024691dbf275f39`.

Relevant `git status --short` was identical before/after Run 4:

```text
 M Cargo.lock
 M Cargo.toml
 M src/broker/mod.rs
 M src/control_fs.rs
 M src/runner.rs
?? src/broker/journal.rs
?? src/broker/journal/
?? src/control_fs/
?? tests/journal_sqlite.rs
```

`rustfmt --edition 2024 --check tests/journal_sqlite.rs` — PASS. `git diff --check` — PASS. No other test was run; no commit or push.
