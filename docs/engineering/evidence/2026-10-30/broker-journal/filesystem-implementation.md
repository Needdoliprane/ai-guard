# Broker journal filesystem primitives — implementation handoff

Worktree `/Users/louis/Documents/ai-guard/.worktrees/broker-supervision`, branch `codex/broker-supervision`, base `3372150fdb99d8bc71700a33dd1d49c0e75c45b5`.

Owner: journal_filesystem_core. Files owned and changed: `src/control_fs.rs` (only `pub(crate) mod persistent` declaration inherited from the initial draft) and new `src/control_fs/persistent.rs` (replacement of that draft). No other repository file edited; no commit or push. No application code in any language other than Rust.

## Implemented behavior

- `Directory::durable_bootstrap()` opens and retains every ancestor directory, matches the complete recorded device/inode chain, validates DAC/ACL/filesystem policy, and synchronizes from child to root. This complements legacy `open_or_create` without changing its behavior.
- `persistent_child(name, false)` never creates or syncs. Existing child type, effective UID, 0700, pathname ACL and identity are checked before directory open; held FD metadata and ancestry are then checked. `true` creates only if absent, leaves errors untouched, and syncs child and retained parent.
- `persistent_read` performs metadata-only `private_entry` admission before any regular-file open: regular type, effective UID, 0600, single link, ACL, stable identity; known protected device/inode identities are rejected before opening. Opens NOFOLLOW/NONBLOCK/CLOEXEC and checks the opened identity, bounded immutable bytes and metadata.
- `persistent_create` bounds input before I/O and uses new O_RDWR|O_CREAT|O_EXCL|NOFOLLOW|NONBLOCK|CLOEXEC mode0600 files. No existing inode is opened, truncated, repaired or removed. Write errors leave their partial file. The caller explicitly syncs file and parent at its acknowledgement barriers.
- `PrivateFile::persistent_revalidate` validates held FD metadata against prepared metadata; bounded cached bytes and digest; actual contents read by `FileExt::read_at` using the SAME retained FD; and metadata-only pathname identity. It never calls legacy `PrivateFile::revalidate`, `read_private`, or any regular-file reopen. Shared cursors are not mutated.
- `persistent_sync` checks before and after `sync_all`; parent synchronization remains explicit at the caller.
- `persistent_try_lock` uses nonblocking exclusive flock on the retained CLOEXEC FD. `PrivateFile` clones retain the same Arc/File. No explicit LOCK_UN exists, so dropping a clone cannot release the shared file description early. Journal itself must retain the handle through its shared Inner lifetime.
- `persistent_publish` is restricted to `current-next.json` -> `current.json`. It checks same retained directory chain/path, distinct file identities and expected old current name; syncs temporary; revalidates old FD/cache/name immediately before rename; renames through the retained parent; checks new identity; syncs parent; checks new identity again. Any failure propagates; only this pointer can be replaced.
- `persistent_entries` enumerates all entries (including partials, hidden names and temporary directories), limits the returned count without upfront allocation, and reads file metadata without opening ANY regular file. Directory-only duplicate FDs are atomically CLOEXEC; readdir EOF and errno are distinguished; successful closedir errors propagate. Each directory counts as one entry and zero logical file bytes, agreed with the journal owner.
- Names are closed single ASCII components of at most128bytes with `[A-Za-z0-9._-]` and no `.`/`..`. All new unsafe ABI calls have safety comments.

No SQLite helper, legacy reader, legacy revalidate or snapshot implementation was refactored or reused for business persistence. No cleanup/unlink or repair operation was added.

## API frozen with journal core

All methods below are crate-visible and return the existing `anyhow::Result`; no dependency on JournalError or journal domain was added.

```rust
struct Entry { name: String, directory: bool, bytes: u64 }

impl Directory {
    fn durable_bootstrap(&self) -> Result<()>;
    fn persistent_child(&self, name: &str, create: bool) -> Result<Option<Self>>;
    fn persistent_read(&self, name: &str, max: u64,
        exclusions: &[PrivateEntryIdentity]) -> Result<Option<PrivateFile>>;
    fn persistent_create(&self, name: &str, bytes: &[u8]) -> Result<PrivateFile>;
    fn persistent_publish(&self, temporary: &PrivateFile,
        current: &PrivateFile, target: &str) -> Result<()>;
    fn persistent_entries(&self, limit: u32) -> Result<Vec<Entry>>;
}
impl PrivateFile {
    fn persistent_revalidate(&self) -> Result<()>;
    fn persistent_sync(&self) -> Result<()>;
    fn persistent_try_lock(&self) -> Result<bool>;
}
```

Caller duties: acquire/retain the one journal lock through an Arc-shared journal, serialize writers and scans, apply logical budgets BEFORE creation, sync each immutable file and its parent before acknowledgement, re-sync the confirmed graph before replay acknowledgements, and map low-level errors opaquely. Both old handles passed to successful pointer publication are stale by name afterward; reload `current.json` through the protected bounded reader. Publication includes physical rename+parent sync in one helper, so existing caller cfg(test) stages after this method cannot claim to inject BETWEEN those two syscalls.

## Checks and evidence status

PASS: `rustfmt --edition 2024 src/control_fs/persistent.rs` parsed/formatted the new Rust source. PASS: `git diff --check -- src/control_fs.rs src/control_fs/persistent.rs` exited0 (new untracked module is not covered by git diff; rustfmt did parse it). PASS: static search confirmed that persistent code never invokes legacy regular-file reader/revalidation or SQLite helpers.

NOT_RUN by explicit task ownership: cargo check, cargo test, clippy, build, runtime/provider calls. Luna owns targeted tests and GREEN proof after combined freeze. No functional PASS or production qualification is asserted by this handoff.

## Limits retained

- Exclusive private business namespace without SQLite/VFS is an embedding precondition. Optional known-inode exclusions are metadata-only and cannot universally detect an unknown single-link database inode.
- DAC/ACL/filesystem policies reuse existing Linux local POSIX-ACL and macOS local APFS/HFS checks. Other-UID boundaries do not promise resistance against hostile same UID, root, administrators or VM/filesystem restores.
- File content checks and fsync may block. This module provides no autonomous scheduler/stop deadline, fencing, fresh authorization or execution grant.
- Logical byte/entry caps are caller admission limits, not physical filesystem/inode quotas. SIGKILL, if run by Luna, does not qualify power-loss durability.
- Supported-platform behavior and actual locks/ACL/fsync still require the separately owned tests; no Linux, cross-UID, cloud or provider qualification was executed here.

## SHA-256 at handoff

```
ee6d53fb9b3dd181886dba6b24ac1a97830949441eb968abda95b637cbaf4670  src/control_fs.rs
95d616e977479efbce7364055feeb114d947587f5cecd2705f1203112b273c0d  src/control_fs/persistent.rs
```
