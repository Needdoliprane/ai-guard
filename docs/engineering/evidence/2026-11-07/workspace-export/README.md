# Export policy and observed modes — 7 November 2026

Validation ran on 6 November UTC / 7 November Europe/Paris. This is a product
correction following final run-1 lot J, not a new audit or a production release.

## Source and acceptance

Base: `5e52807d4311d8252c3721e87b032420360c78ee`. The frozen `audit-j-fix03`
contains the implementation and tests from the dedicated export worktree.
The [final source manifest](source.sha256) identifies the exact Cargo and Rust
inputs; the commit carrying this evidence must match them. The initial
[source-03 manifest](source-03.sha256) is retained separately. Frozen input04
differs from03 only in `tests/runner_recovery.rs`; Root verified that comparison
recursively. Application, Cargo inputs and the first48 test functions are
unchanged; results below retain their actual frozen-input attribution.

Acceptance: refuse Git-directory-shaped output before publication; omit reserved
tool metadata and its covered Unicode aliases; keep exported regular files at
0644; observe permissions-only changes without assigning unknown historical
modes; keep old manifest bytes/hashes and the closed V1 IPC acknowledgement.
Omission counts belong to descriptive audit events, not an incompatible V1 field.

## Reproductions before correction

| Frozen input and command | Result | Parent-captured log SHA-256 |
|---|---|---|
| `audit-j-repro01`: `cargo test --offline --locked --test workspace_export` | RC101: three behavioral failures (root/nested Git shape, Unicode alias); two positive controls pass | `6396c6c764206188c6976779d2d90e624b76f392a6484346fbabbf122afb0577` |
| Same input: `cargo test --offline --locked --lib workspace::export_policy_regression_tests::scan_records_mode_only_changes_without_restoring_worker_modes -- --exact` | RC101: chmod-only change omitted from diff, 0 instead of 1 | `96e50421f99301cc932fd8612891248ff0f23cff20894375973f02c189b2fb18` |
| `audit-j-wire-red02`: `cargo test --offline --locked --lib broker::service::protocol::export_report_compatibility_tests::new_export_reply_is_readable_by_the_legacy_v1_client -- --exact` | RC101: independent review's compatibility defect reproduced; historical closed decoder rejects `omitted_entries` | `ade6de876b9aa5682f422b24c1dfac1af0adaa32e00d5d4913399694088e56dc` |

These are archive/data and serializer checks with inert synthetic contents. They
do not execute Git, an editor, exported code or a worker container. The original
operator execution finding belongs to the primary audit's distinct evidence.
The intermediate `audit-j-fix01` was **NOT_RUN** after the review found the IPC
defect. No result from that input is credited to the final implementation.

## Corrected input

All four selections below passed on the same `audit-j-fix03` and Cargo target:

| Command after `cargo test --offline --locked` | Result | Parent-captured log SHA-256 |
|---|---|---|
| `--lib export_report_compatibility_tests` | PASS: 4 | `8586800daf2921edb7634efde89a668a3e9a55e074ac6ccefbbdd147db93b57f` |
| `--test workspace_export` | PASS: 8 | `3eedaa2f7c7a9f58cbe0f2692e10eb16d5e6056ce3c58ce8e54c5ab728d1c41a` |
| `--test audit_workspace_export` | PASS: 6 | `6393f3a9f7f40613f127bbbf54f9ae47b4d66ae743cd7bf5fa1d5bf6978041b9` |
| `--lib workspace:: -- --test-threads=1` | PASS: 30 | `ed02c275cfb61952cfa62ebbb1705ae1b019e10efd4fb36f7ab567834c5da102` |

The 48 functions cover explicit/implicit directory order, linked-worktree
`commondir`, the authorized export path, no publication/staging residue on the
tested refusal paths, existing link/duplicate/type/limit controls, durability
barriers, mode provenance and historical nonempty-manifest projection. Ordinary
Unicode names remain available. The serializer oracle reads the current producer
with the historical closed V1 decoder, rather than testing only old payloads with
a new decoder.

Additional results:

| Input and command | Result | Parent-captured log SHA-256 |
|---|---|---|
|03: `cargo check --offline --locked --all-targets` | RC0,59.22s;119 library warnings,2 SSO-test warnings,3 lib-test warnings | `769fe64bd4679daa80864651482598c9dd9269a7f47ea166086965d34e319941` |
|03: `cargo test --offline --locked --test runner_recovery recovered_create_and_import_keep_the_same_instance_and_first_start -- --exact` | FAIL: original20-second per-command observation expires; functional end not reached | `5f128d85e62a73d59a6d32348b502eb9861635a7fe830a68cc1a11a37c88cb67` |
|04: same selection, with `--nocapture` | PASS: all three recovery schedules finish; exact-one-start, quota, usage, replay and integrity assertions unchanged;86.81s total | `fb216b203e92160dbe667f7d01f4df06b2cff3e98d132aa2ce387dcdc5d88583` |

The test observation is now120seconds per CLI action, with bounded source-owned
case/action/elapsed diagnostics. Product HTTP/runtime deadlines are unchanged.
The04 run observes an import-completion recovery lasting22.879seconds, longer
than the old observation bound. The03 log did not identify its failing phase;
its precise cause is not retrospectively asserted. Expiration remains FAIL.
No other recovery selection is counted as executed after the03 failure.
Compilation of all targets was run on03, not silently relabelled as04; the only
changed04 test module was compiled by its successful targeted command.

## Environment and review

Luna executed only the named commands in disposable Linux ARM64 Docker QA.
The engine hosts the test sandbox; it is not a qualification of ai-guard workers.
Image: `sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`.
Toolchain: Rust1.99.1, Cargo offline/locked, one build job, incremental disabled,
dev/test debug info disabled, distinct target per frozen source. No stack-limit
increase or weakened production timeout was used.

Root verified image/configuration and effective UID/GID1000, empty environment
allowlist, capability set0, no-new-privileges, seccomp, private namespaces,
network none, read-only source/toolchain/registry/root, no host sockets or ports,
2CPU, 8GiB memory without swap, 256PIDs, 3GiB/65536-inode scratch, file-size
512MiB, nofile1024, core0 and a fixed execution window. Only scratch was available
for target output. Raw parent-captured logs remain private with mode0600; no
target binary, database, credential or generated export is distributed as proof.
OOM remained0 for these selections. The retained private authorized-export
compatibility wrapper accounts for one additional unused-method warning; the
warning/Clippy release gate remains open.

The independent reviewer found the V1 compatibility defect above. Root restored
the original ACK and `ExportFactsV1`, then added a distinct generic descriptive
`workspace.export.policy` event. The follow-up review found no blocking defect
in that correction and checked the new authorized-path canary and paired EN/FR
guides. A publication followed by an unacknowledged result still requires
reconciliation; it is never silently adopted as success or replaced.

## Limits

Real APFS name aliasing, Docker/Podman/Kubernetes workers, provider CLIs,
cross-version process migration and physical crash/restore are **NOT_RUN** in
this lot. It neither qualifies confinement nor closes global gates. Historical
mode coverage stays unknown. `.cargo/config.toml`, `Makefile`, `package.json` and
other project code can remain: a filtered export is not universally inert.
Review content before enabling tools or execution. G09 remains FAIL.
