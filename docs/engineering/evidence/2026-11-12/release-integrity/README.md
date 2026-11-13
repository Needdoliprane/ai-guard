# Release integrity (lot B) — handoff record, 13 November 2026

This record describes the lot-B branch `claude/release-integrity-20261112` as delivered for Root/Codex
review. **The code has never been compiled and no test has been executed**: no QA admission exists for
this lot, so every test, Clippy, image build and runtime check is `NOT_RUN`. What follows separates
what the code is written to do, what needs a Root-owned seam, and what is missing as qualification.
Nothing here is production-ready; no gate (G01, G12) and no risk (RC-P3-R11, RC-P7-R07, RC-P7-R04,
RC-P7-R12) changes status. A test file is not a `PASS`; a static review is not a compile result.

User guides: [English](../../../../setup/release/README.md) · [Français](../../../../setup/release/README.fr.md).
Related records: [source lifecycle D](../../2026-11-11/source-lifecycle/README.md) (the prepared source
reused here, with its own sources and results), [risk coverage](../../../RISK_COVERAGE_20261112.md),
[production gates](../../../PRODUCTION_READINESS.md).

## Source identity

| Item | Value |
|---|---|
| Branch / worktree | `claude/release-integrity-20261112` / `.worktrees/claude-release-integrity-20261112` |
| Base (frozen) | `77f7edd6da9a2895a1277754c6144f893bcb8aea`, tree `ce62a6126e38ecfc6ed9557b87af3f3ad66d6223`, `git archive --format=tar` SHA-256 `32179f9e4186dddd074b3a26d8d2c0044df8e7581be67c83a24b290fe52d99ca`, 2 601 tracked files |
| Code head proposed for QA | `c7cc37e84ccc6798b5a667b4af98b5ee815b9336`, tree `423f0f982986cc07ce21fa8c416360e068ea254b`, `git archive --format=tar` SHA-256 `cb382852a755de23e8a9874bb73dfdd13ed60ea3d57a4b5f7303b4a19a3c529a`, 2 617 tracked files |
| Delivery commit | the `docs(evidence)` commit that adds this file, child of `c7cc37e`; it changes documentation only (this file and line 7 of both release guides). Its SHA is recorded in the private handoff index, not here |
| SHA actually tested | none (`NOT_RUN`) |

No lot test reads the release guides or this file, so a QA run on `c7cc37e` covers the code of the
delivery commit. Results on this branch are branch results, never results of a composed tree.

## Commits (base..delivery)

| Commit | Subject | Files |
|---|---|---|
| `09984a07ee621908c6688619ecb9540d41fae1a5` | feat(release): release metadata module tree and unit tests (not yet wired) | 11, +6 259 |
| `4cdb27c1ff2582928d83cecb0c7c3c578f335621` | test(release): release manifest and verify integration targets (RED until the wire) | 3, +2 228 |
| `5e7f19fe4170cf3d66b3c53810aaa3404579b6a0` | wire: release metadata seam W1-W3 (capture.rs, source_snapshot.rs, operations.rs) | 3, +175 −7 |
| `a93cde312c34c72fb9ccc1a97fb4e10d4bb650c0` | feat(release): declared image labels and EN/FR release guides | 4, +776 |
| `e70fdfaab0d3f03b72dca5056958a9afce580398` | fix(release): address gate S1-S3 review items | 9, +574 −102 |
| `9e0066a708a7e886d6007c46f0c8bfdde1beabc5` | fix(release): address final review items before format 1 is frozen | 8, +513 −77 |
| `c7cc37e84ccc6798b5a667b4af98b5ee815b9336` | test(release): a source divergence without a listed entry names the field | 1, +41 −1 |
| delivery | docs(evidence): release-integrity lot handoff record | this file and the two guides |

No `fix(release)` commit was made for a compile error: there has been no compile signal yet. The two
`fix(release)` commits answer review items.

## Files changed against the base

- Owned, new: `src/operations/source_snapshot/release_metadata.rs` and
  `release_metadata/{artifact,cargo_lock,compare,cyclonedx,declared,document,npm_lock,output,tests,toml_subset}.rs`;
  `tests/release_metadata.rs`, `tests/release_metadata_verify.rs`, `tests/release_metadata/support.rs`;
  `docs/setup/release/README.md`, `README.fr.md`; this file.
- Owned, modified: `images/control.Dockerfile` (one `LABEL`, `:15`) and `images/worker.Dockerfile` (one
  continued `LABEL`, `:12-14`). No `COPY`, `RUN`, `ARG`, provider install, credential, handoff comment,
  toolchain copy, `ai-guard-diagnose` or `USER` line changed.
- Wire (authorised seam, branch B only, commit `5e7f19f`): `src/operations/source_snapshot/capture.rs`
  (W1, additive: `DIGEST_LAYOUT` `:56-70`, `RetainedEntry` `:1012-1031`, `Snapshot::{digest,
  total_bytes, entries, read_retained}` `:1120-1192`), `src/operations/source_snapshot.rs` (W2:
  `mod release_metadata;` `:21`, `release()` `:74-78`, `Release` in `needs_interrupt_scope` `:492-503`),
  `src/operations.rs` (W3: `OperatorCommand::Release` `:164-169`, `ReleaseCommand` `:193-231`, comment
  `:362-365`, dispatch `:688`). Blobs after the wire: `c06f9ff1…`, `c739fc8b…`, `c70230f9…`; unchanged
  since.
- Unchanged (blob equal to the base): `src/main.rs`, `src/api.rs`, `src/lib.rs`, `src/audit.rs` and
  `src/audit/`, `src/control_fs.rs` and `src/control_fs/`, `src/runtime/` (including
  `retained_cli.rs`, `managed.rs` and `managed/relay.rs`), `src/runner/` (including
  `broker/**`, `relay_admission.rs`), `src/operations/source_snapshot/{tests,interrupt}.rs`,
  `Cargo.toml`, `Cargo.lock`, `.dockerignore`, `docs/api.md`, `docs/setup/README*.md`, `README*.md`,
  `AGENTS.md`, `PROMPT_PRODUCTION.md`, every other `docs/engineering/` file. No `build.rs`.

## What the code is written to do (operational code, unqualified)

- `agentsandbox release manifest --source DIST --output NEW_DIR [--artifact FILE]… [--codex-version V]
  [--claude-code-version V] [--expect-source-digest HEX64]` and `agentsandbox release verify --source DIST
  --manifest DIR [--artifact FILE]… [--source-only]`, reached through the existing `Operator` flatten and
  `operations::execute`, with no `src/main.rs` change.
- The only prepared source is `prepare_build` → `PreparedSource` (`Arc<Snapshot>`); everything read from
  the copy goes through `Snapshot::read_retained`, which calls `verify()` before the first and after the
  last read and returns bytes only. No `PreparedSource`/`Snapshot` is built from a pathname; no path into
  the copy leaves the seam.
- Strict readers of `Cargo.lock` v4 and `front/package-lock.json` v3 (a lock format 1 cannot resolve is
  refused, never downgraded), a bounded TOML subset, declared facts of the recipes with explicit
  `unknown` values, manifest format 1 (canonical `serde_json::to_vec_pretty` bytes, unknown fields
  refused), a CycloneDX 1.6 `pre-build` SBOM without serial number or timestamp, and `SHA256SUMS`.
- The build-input identity (`ai-guard:build-inputs:v1`, `release_metadata/document.rs:231-266`) names the
  source digest and the declared provider versions. It is declared, not a build identity: target,
  artefact profile, base-image digests and compiler are not observed.
- Artefacts are bound by basename, length and SHA-256 through an owned descriptor-relative reader
  (`release_metadata/artifact.rs`); output goes to a new 0700 directory under an existing 0700 parent
  owned by the effective UID, staged and published with `RENAME_NOREPLACE`/`RENAME_EXCL` and read back;
  nothing is overwritten or removed.
- `verify` classes: `MATCH`, `SOURCE MATCH; artifacts NOT VERIFIED` (only with `--source-only`),
  `DIVERGENT <category>` (ten categories), `UNVERIFIED: <reason>`; exit 0 only for the first two.
- Image labels `ai-guard.image.recipe` (both) and `ai-guard.provider.*.requested-version` (worker),
  declared by the builder.

The definition of manifest format 1 is the code at `c7cc37e` (CONTRACT §13 in the private handoff).

## Missing seam (Root-owned; proposed, not applied)

Patches with pre/post-image blobs are in the private handoff folder (`proposed-wire/PROPOSED_WIRES.md`):

| Id | Proposal | State |
|---|---|---|
| PW-1 | `--version` long text: declared build-input identity, compiled target and debug assertions, separately (`src/build_info.rs`, `src/lib.rs`, `src/main.rs`) | patch, applies on base and branch; not compiled |
| PW-2 | gateway `/healthz` `build_inputs_identity` and `build {target, debug_assertions}`; `docs/api.md`; one test assertion | patch, needs PW-1; not compiled |
| PW-3 | `build` computes the identity from its own prepared source and passes it to both image recipes as a build argument; label `ai-guard.build.inputs-identity` | patch on the branch head (needs W1 and the lot module); not compiled |
| PW-5 | guide index rows in `docs/setup/README{,.fr}.md` and the root `README{,.fr}.md` | patch |
| PW-6 | streaming trusted-input reader in `control_fs` (file ACL on macOS, removal of the owned `unsafe` later) | patch; must be composed with a lot-B adoption change not yet written (dead code alone) |
| PW-7 | governance status lines, pre-QA revision; the JSON risk register is left to Root on the composed tree | patch |
| PW-8 | licence text in the worker image and a third-party notice step in the release checklist (G12) | patch |

Contract proposals still open (CONTRACT §6): C-1 manifest from the same capture as `build`, C-3 image
IDs, C-4 signing/trust anchor/anti-rollback, C-5 base images by digest, C-6 provider clients from a
lock, C-7 compatibility checks. Root decisions requested: W2c (interruption scope for `release`) and
its engine-specific refusal text (`interrupt.rs:103-108`); confirmation that the new image labels are
stable in the runtime configuration fingerprint (Codex's private composition of
`src/runtime/managed.rs`, not read by this lot); PW-7 fingerprint revalidation (the lot changes
`images/*.Dockerfile` and `OperatorCommand`, cited by RC-P3-R11 and RC-P7-R04); R-3 (purl
specification). The coordination note records the purl specification as admitted reading; no purl is
emitted by format 1.

## Missing qualification

Every line below is `NOT_RUN`: no QA admission for this lot, no cargo/rustc/project binary was run on
the development Mac, no engine was used.

| Selector (proposed, see the private `QA_READY.r2.json`) | Expected | Result |
|---|---|---|
| `cargo check --frozen --offline --all-targets -j 1` @`c7cc37e` | RC 0 (first compile signal) | `NOT_RUN` |
| `cargo test --frozen --offline -j 1 --lib operations::source_snapshot::release_metadata:: -- --nocapture --test-threads=1` | 17 passed, 0 failed, 0 ignored | `NOT_RUN` |
| `cargo test --frozen --offline --test release_metadata -- --nocapture --test-threads=1` | 12 passed | `NOT_RUN` |
| `cargo test --frozen --offline --test release_metadata_verify -- --nocapture --test-threads=1` | 6 passed | `NOT_RUN` |
| `cargo test --frozen --offline -j 1 --lib operations::source_snapshot::tests:: -- --test-threads=1` (D regression) | 30 passed | `NOT_RUN` |
| `cargo test --frozen --offline --test source_lifecycle -- --test-threads=1` | 42 passed, 1 ignored (`paths.rs:824`, stays `NOT_RUN`) | `NOT_RUN` |
| `cargo test --frozen --offline --test runtime_program -- --test-threads=1` | 12 passed | `NOT_RUN` |
| `cargo clippy --frozen --offline --all-targets -j 1 -- -D warnings`, judged on owned files and wire hunks only | no diagnostic located in them | `NOT_RUN` |
| optional `cargo test --frozen --offline -j 1 --bin agentsandbox -- --test-threads=1` | every test passes; 49 `#[test]` attributes counted statically | `NOT_RUN` |
| `observe`: link count, inode, owner and mode of Cargo's `target/debug/agentsandbox` and `deps/agentsandbox-*`, ancestors and filesystem type | observation only | `NOT_RUN` |
| image build and label inspection (Docker, Podman) | not admitted | `NOT_RUN` |
| runtime with labelled images (Docker, Podman) | not admitted | `NOT_RUN` |
| CycloneDX schema validation, macOS host, reproducible builds | not admitted | `NOT_RUN` |

Unit tests (`operations::source_snapshot::release_metadata::tests::`), each `NOT_RUN`:
`toml_subset_reads_the_repository_manifests_and_refuses_constructs_outside_its_grammar`,
`cargo_lock_of_the_repository_resolves_every_dependency`,
`cargo_lock_refusals_name_the_line_and_the_reason`,
`npm_lock_of_the_repository_maps_each_integrity_to_sha512_hex`,
`npm_lock_refusals_cover_unknown_keys_nested_layouts_and_unsafe_urls`,
`recipes_reader_extracts_base_images_and_marks_shell_lines_unknown`,
`provider_version_rule_is_the_intersection_with_the_worker_recipe`,
`published_image_labels_are_declared_once_in_the_final_stage`,
`licence_expression_form_only_for_well_formed_expressions`,
`an_artifact_linked_during_hashing_is_refused`,
`an_artifact_name_is_opened_only_once_it_is_seen_as_a_regular_file`,
`publication_with_unknown_durability_is_reported_and_removes_nothing`,
`canonical_manifest_round_trips_and_refuses_unknown_fields`,
`a_source_divergence_without_a_listed_entry_names_the_differing_field`,
`verify_names_a_stop_request_or_a_lost_step_instead_of_an_unreadable_input`,
`retained_reads_return_the_captured_bytes_or_nothing`,
`digest_is_rederived_from_the_retained_entries_with_the_recorded_layout`.

`tests/release_metadata.rs`, each `NOT_RUN`:
`manifest_binds_the_digest_that_build_reports_for_the_same_unchanged_source`,
`manifest_entries_and_lock_hashes_are_those_of_the_original_bytes`,
`same_inputs_give_byte_identical_documents_whatever_the_path_umask_or_tmpdir`,
`outputs_hold_no_host_path_and_no_excluded_file_content`,
`incomplete_or_unknown_lock_content_is_refused_before_any_output`,
`output_must_be_a_new_directory_under_a_private_parent_outside_the_source`,
`an_interrupted_manifest_publishes_nothing`,
`expected_source_digest_is_checked_before_any_read_or_output`,
`sbom_lists_locked_components_with_their_registry_hashes_and_names_what_locks_omit`,
`declared_provider_versions_change_the_build_input_identity_and_are_marked_declared`,
`artifacts_are_bound_by_name_size_and_sha256_and_unsafe_ones_are_refused`,
`the_cli_just_built_by_cargo_is_bound_as_an_artifact` (an artefact-policy refusal of Cargo's output is
reported `ENVIRONMENT:` and fails; it is an environment fact, never a `PASS`).

`tests/release_metadata_verify.rs`, each `NOT_RUN`:
`verify_matches_an_unchanged_source_and_its_artifacts_from_another_location`,
`one_changed_byte_in_a_bound_input_is_refused_with_its_category`,
`verify_requires_every_listed_artifact_unless_source_only_is_explicit`,
`a_tampered_format_1_manifest_is_divergent_format`,
`a_manifest_this_cli_cannot_judge_is_unverified`,
`an_unreadable_input_is_unverified_not_divergent`.

Optional reproduction before the wire (`--test release_metadata` @`4cdb27c`, RED expected with clap's
unrecognised subcommand, exit 2): `NOT_RUN`. Strict Clippy on the whole tree is not a criterion here:
`PRODUCTION_READINESS.md` records strict Clippy failures on earlier bases.

## Static checks run for this handoff (no project code executed)

All on the committed code head `c7cc37e` unless stated; the worktree was clean at that head.

| Command | Result |
|---|---|
| `rustfmt --edition 2024 --check src/operations/source_snapshot/release_metadata.rs` (module tree: owned files only) | rc 0 |
| `rustfmt --edition 2024 --check tests/release_metadata.rs`, `tests/release_metadata_verify.rs`, `tests/release_metadata/support.rs` (check mode also reads the unchanged harness) | rc 0 each |
| `rustfmt --edition 2024 --emit stdout < <file> \| cmp -s - <file>` on the 17 `.rs` files of the lot and the wire (11 module files, 3 test files, `capture.rs`, `source_snapshot.rs`, `operations.rs`) | identical, 17/17; negative control (an unformatted input) differs |
| `git diff --check 77f7edd c7cc37e` | rc 0 |
| `git diff --name-status 77f7edd c7cc37e` | 21 paths: 16 added owned, 2 modified owned (Dockerfiles), 3 modified wire files; nothing outside the perimeter |
| `git diff --quiet 77f7edd c7cc37e --` the unchanged set listed above | rc 0 |
| `git diff --quiet 5e7f19f c7cc37e --` the three wire files; `git diff --quiet a93cde3 c7cc37e -- images` | rc 0, rc 0 |
| `git diff --full-index 77f7edd 5e7f19f --` wire files vs the handoff `wire/W1-W3.patch` | identical, SHA-256 `63a02837b068e8ffa1c9013b0e11270f9cc92dde9f710f3521bd17589f886001` |
| Review greps of the design checklist on the module (`capture::capture`, `PreparedSource {`, `.root()`, `.context()`, `env_file(`, `provenance_line`, `manifest.json`) | `capture::capture` and `.context()` only in `release_metadata/tests.rs:1751, 1800`; `manifest.json` hits are `release-manifest.json` only; none of the others |
| `pub`/`pub(crate)` items in the module | none (every item private or `pub(super)`) |
| `Utc::now`, `SystemTime::now`, `temp_dir()`, `current_dir`, `hostname`, `getuid(` in non-test module code | none |
| `unwrap`/`expect`/`panic!`/`unreachable!`/`todo!` in non-test module code | none |
| `build identity` in the module and guides | only the sentences saying the value is not one (`release_metadata.rs:15`, guides `:28`) |
| `nlink` in `artifact.rs` | only the before/after state comparison (`:234`); no link-count rule |
| `unsafe` in `artifact.rs` | 5 blocks: `openat` `:135`, `from_raw_fd` `:140`, `fstatat` `:165`, `assume_init` `:177`, `geteuid` `:244` |
| Diff grep for secret-like strings, absolute host paths and `.env` | no key, token, private key or credential pattern; no home or host path (the only `/private/…` mentions are the documented macOS aliases); `.env` occurs in documentation, the artefact-name guard list and synthetic canary fixtures (`SYNTHETIC=…`, `CANARY-…`) only |
| EN/FR guides | 393 lines, 110 table rows, 22 fences, 16 anchors each; backticked tokens and anchors identical; link targets identical modulo language, each resolves (this file included) |
| Guide message cross-check at the delivery state: 47 backticked messages of the `Verify` to `Data hygiene` sections, split at placeholders and `;` | every piece found in the product sources with string continuations joined; the 7 pieces composed at run time (`MAX_ARTIFACTS`, `FORMAT_VERSION`, `CARGO_TOML`, step names) checked by hand: equal |

Observed tooling limit: `rustfmt --check` reading standard input exits 0 even when it prints a
difference, so stdin results are judged by byte comparison only.

## Reviews and dispositions (private notes, outside the repository)

Private coordination folder `claude-next-lots-20261112/` (not versioned). SHA-256 at this handoff:

| Note | Verdict | Disposition |
|---|---|---|
| `notes/release-integrity/DESIGN_REVIEW.md` (`7c4ef5e5…a846`) | REQUEST_CHANGES (B1, B2, N1–N17) | all accepted, design revision 2 (`DESIGN.md` `98329156…f442`, §18) |
| `notes/release-integrity/GATE_S1.md` (`9bfb3159…f6e3`) | GO_WITH_CHANGES, 0 blocking | N1–N8 treated in `e70fdfa` |
| `notes/release-integrity/GATE_S2.md` (`c8e1a1c9…9abe`) | GO_WITH_CHANGES, 0 blocking | S2-N1–N4 treated in `e70fdfa`; W2c text left to Root |
| `notes/release-integrity/GATE_S3.md` (`69d482a2…ff29`) | GO_WITH_CHANGES, 0 blocking | N1–N11 treated in `e70fdfa`; N9/N10 requests to Root |
| `notes/release-integrity/FINAL_REVIEW_security.md` (`e8dc5191…cebe7`) | REQUEST_CHANGES, B1 | B1, N1–N5 fixed in `9e0066a`; N6 handed to this handoff |
| `notes/release-integrity/FINAL_REVIEW_correctness.md` (`13176abe…0243`) | REQUEST_CHANGES, B1 | B1, N1–N3, N5, N7 fixed in `9e0066a`/`c7cc37e`; N4/N6 handed to this handoff |

Handoff items of security N6 and correctness N4/N6 done here: this record and the guides' link to it,
the QA proposal revision 2 with the four repair test names and "runtime with labelled images (Docker,
Podman): `NOT_RUN`", the proposed-wire patches, and the guide message cross-check at the delivery
state. Still owed:

- the re-review of `9e0066a` and `c7cc37e` by the final reviewers;
- the **mandatory second review** of the owned `unsafe` in `release_metadata/artifact.rs` (five blocks,
  including the `fstatat` + `assume_init` block at `:163-180`) and of the `output.rs` publication path
  (CONTRACT §7). The final reviews read both for correctness; that is not the named second review.

## Limits and residuals

- No compiler signal at all: the module (7 042 lines, 1 848 of them unit tests), the three integration files (2 379 lines) and the wire meet the compiler first in QA.
- The build-input identity and every provider version are declared, not authenticated; no signature,
  trust anchor, SLSA provenance or anti-rollback.
- A manifest comes from its own capture; `--expect-source-digest` checks equal digests, not the same
  capture (C-1). Images are not bound (C-3); base images are referenced by tag (C-5).
- The SBOM carries no purl or CPE and is not ready for vulnerability scanners.
- The producer version is `0.1.0` for every build, so the producer check does not identify the CLI build.
- A name swapped between `fstatat` and `openat` can still be opened once (with `O_NONBLOCK`, `O_NOCTTY`)
  before the identity refusal; the path is operator-supplied, not a privilege boundary.
- A token placed in an npm `resolved` URL path is not detected and would be written as the distribution
  reference (documented in the guides).
- On macOS the extended ACL of an artefact file is not inspected (PW-6).
- `build` accepts provider versions the worker recipe refuses; only `release manifest` applies the
  intersection.
- No time bound: a read that never returns blocks the command until `SIGKILL`.
- `require_waitable_children` also refuses `release` under an ignored `SIGCHLD`, with an engine-specific
  message (W2c, Root).
- How Cargo links its output on the QA filesystem, and whether the QA target directory passes the
  artefact ancestor policy, are unknown until `observe`.

## Resources

Created by this lot: the worktree `.worktrees/claude-release-integrity-20261112` and the local branch
`claude/release-integrity-20261112` (12 November 2026, 18:30 UTC), and private notes/handoff files
outside the repository. No container, volume, image, push, merge, tag or deletion. Codex integrates,
pushes and retires the worktree.

## Contre-revue de la réparation finale / Re-review of the final repair (2026-11-13T00:00Z)

This section records the re-review of the repair commits `9e0066a` and `c7cc37e` (and of the
documentation commit `b25e6ae`), and the second review of the owned `unsafe` code and of the
publication path. It changes no code. **Nothing has been compiled or run since the handoff above**:
every test, Clippy, image build and runtime check is still `NOT_RUN`, and QA is `BLOCKED` until a QA
admission exists (`QA_ADMISSION.json` absent). A static reading is not a compile result and not a
`PASS`. Nothing here is production-ready; no gate (G01, G12) and no risk (RC-P3-R11, RC-P7-R07,
RC-P7-R04, RC-P7-R12) changes status.

### Source identity after the re-review

| Item | Value |
|---|---|
| Code head proposed for QA | `c7cc37e84ccc6798b5a667b4af98b5ee815b9336`, unchanged: no repair commit after the re-review |
| Previous delivery commit | `b25e6ae8b02ce901f1e4d728145d43e470e20f72` (documentation only: the handoff record above and line 7 of both guides) |
| Delivery commit | the `docs(evidence)` commit that adds this section, child of `b25e6ae`; it changes this file only. Its SHA is recorded in the private handoff index, not here |
| QA proposal | the private `QA_READY.r2.json` (`345bb949…3a20`), unchanged; there is no revision 3, because the code SHA did not change |
| SHA actually tested | none (`NOT_RUN`) |

The selectors and expected counts of "Missing qualification" above are unchanged: 17 unit and
12 + 6 integration functions of the lot, regressions 30, 42 (plus 1 ignored, which stays `NOT_RUN`)
and 12, each `NOT_RUN`.

### Reports (private notes, outside the repository)

| Note | Lens | Reviewed | Verdict |
|---|---|---|---|
| `notes/release-integrity/REREVIEW_security.md` (`961e664a…c9cc`) | security and contract boundary | `c7cc37e` (diff from `e70fdfa`), `b25e6ae` | GO_WITH_CHANGES, 0 blocking, RR-N1–RR-N6 |
| `notes/release-integrity/REREVIEW_correctness.md` (`5ca4f2c3…93be`) | correctness, compile realism, test quality | `c7cc37e` (diff from `e70fdfa`), `b25e6ae` | GO_WITH_CHANGES, 0 blocking, NB1–NB7 |
| `notes/release-integrity/SECOND_REVIEW_unsafe_output.md` (`5f358073…a2fc`) | second review of the owned `unsafe` and of the publication path, plus a re-review of the repair | `c7cc37e`, `b25e6ae` | GO_WITH_CHANGES, 0 blocking, NB1–NB6 |

Each reviewer is a Claude session that wrote none of the lot and worked read-only on the branch. None
ran a compiler, a test, an engine, the QA harness or a project binary; their native checks are
`rustfmt --check` on exported, hash-verified copies and Git reads. Two reports state that the project
skill `ai-guard-security-review` was not available and that the mandate and CONTRACT were applied
instead.

### Second review of the owned `unsafe` code and of the publication path (CONTRACT §7.3)

Recorded here by name, as security RR-N6 and correctness NB6 request:

- Reviewer and SHA: the independent second reviewer of `SECOND_REVIEW_unsafe_output.md`, on `c7cc37e`.
- Scope: the five `unsafe` blocks of `release_metadata/artifact.rs` (`openat` `:135`, `from_raw_fd`
  `:140`, `fstatat` `:165-172`, `assume_init` `:177`, `geteuid` `:244`; the unit test adds `mkfifo` at
  `tests.rs:1325`), the open flags, the order type check → open → identity check, and the `output.rs`
  publication path over the base `control_fs` primitives.
- Result, by static reading: the five blocks are sound as written. The publication path writes only to
  a new, exclusive destination, follows no symbolic link, removes nothing, and its durability claims
  match its system calls. Its NB1, NB2 and NB4 concern evidence and defence in depth, not undefined
  behaviour.
- Limits: a source review. It settles neither compile, nor Clippy, nor the descriptor identity on the
  QA filesystem (all `NOT_RUN`). Root decides whether it accepts this report as the second review
  required before composition.

### Finding dispositions

Findings of the final reviews, as checked by the three reports at `c7cc37e`:

| Finding | Status |
|---|---|
| Security B1 / correctness B1 (other legal TOML forms of the release profile and of the package) | closed; the added cases fail on `e70fdfa` |
| Security N1, N2, N4, N5 (N5 with the documented key-only deviation) | closed |
| Security N3 (artefact opened before its type is known) | closed in code; its test does not discriminate (below) |
| Security N3 residual (name swapped between `fstatat` and `openat`) | deferred_documented; accepted in substance by the second review, wording below |
| Correctness N1 (by refusal), N2, N3, N4, N5, N7 | closed |
| Security N6 / correctness N6 (handoff deliverables) | partially_closed: record, guide link, QA proposal revision 2, proposed-wire patches and message cross-check delivered; second review recorded above; the test additions below and QA remain |

Open non-blocking items of the re-review. None is treated in this delivery (no repair commit):

| Theme | Items | What stays open |
|---|---|---|
| The unit test `an_artifact_name_is_opened_only_once_it_is_seen_as_a_regular_file` cannot see whether a non-regular entry was opened | security RR-N1, correctness NB1, second review NB1 | an effect oracle (a FIFO writer that must stay blocked, mode-`0o000` nodes, or a Unix socket whose `open` fails). Until then a `PASS` of this test is no evidence that a device, FIFO or socket is never opened |
| `Entry::Replaced` and the text `was replaced while it was opened` have no test | correctness NB2, second review NB2 | a `#[cfg(test)]` hook between `fstatat` and `openat` (the shape of `AFTER_READ`), with a regular-file swap and a FIFO swap |
| Guide `:112` (EN/FR) says a non-regular node is refused without being opened, without the swap residual | security RR-N2, correctness NB4, second review NB3 | one sentence and one "Other limits" bullet EN/FR: the replacing entry may be opened once, without blocking and never as a controlling terminal; the root-owned sticky-directory case added to CONTRACT |
| The data-hygiene list of repeated build-input text (guide `:350`) reads as exhaustive | security RR-N3 | "for example", plus base-image references, versions, `[profile.release]` strings and the toolchain request |
| The `first_reason` ordering frozen in CONTRACT §13 has no two-reason case | security RR-N4 | one profile case with two reasons |
| Three branches of the B1/N1 fix have no case | correctness NB3 | `[[profile]]`, a non-scalar `[profile.release]` value, a root entry with only `peerDependenciesMeta` |
| `open_regular` and `open_at` do not check the one-component precondition quoted in their SAFETY comments | second review NB4 | a local `InvalidInput` refusal of an empty name, `.`, `..` or a name holding `/`; every current caller passes a safe name |
| The `NotPublished` message (`output.rs:176-181`) can name the wrong cause and the wrong place | second review NB5 | message text only; the behaviour is safe (nothing renamed or removed) |
| QA-only items | security RR-N5, correctness NB5 and NB7, second review NB6 | compile; Clippy `-D warnings`; whether `fstatat` and `fstat` give the same `(st_dev, st_ino)` on the QA filesystem (a mismatch refuses every artefact as replaced, and `the_cli_just_built_by_cargo_is_bound_as_an_artifact` then reports `FAIL:`, not `ENVIRONMENT:`; Root decides whether that text joins the environment list); `/dev` and the scratch ancestors in the QA sandbox; optional `AT_NO_AUTOMOUNT` on Linux |

No report found a definite compile error or a regression in `e70fdfa..c7cc37e`, or a break of a
mandate invariant. Compile stays unknown.

### Proposed-wire applicability, re-checked

`git apply --check` on 13 November 2026, for each patch of the private `proposed-wire/` folder against
the base it declares. No file was changed in any checkout.

| Patch | Declared base | Method | Result |
|---|---|---|---|
| PW-1, PW-2, PW-5, PW-6, PW-7 | `77f7edd` | on the worktree at `b25e6ae`: every touched path has the same blob as at the base, and the new files are absent at both | rc 0 each |
| PW-3 | `c7cc37e` | the 9 pre-images exported with `git cat-file -p` into a scratch directory outside any repository and checked with `git hash-object` (the guides differ at `b25e6ae`) | `--check` rc 0; once applied, the 9 post-images equal the blobs the patch declares |
| PW-8 | `77f7edd` | the same, 2 files (the worker recipe differs on the branch) | `--check` rc 0; post-images equal the declared blobs |

PW-3 and PW-8 also apply to scratch copies of `b25e6ae` (rc 0); their post-images then differ from the
declared blobs only on lines the lot changed (guide line 7, worker `LABEL`). No revised patch was
needed. The delivery commit of this section touches only this file, which no patch touches.

### Still open before composition

- QA admission, then the first compile signal and the selectors of `QA_READY.r2.json`, at `c7cc37e`.
- The non-blocking items above. Owned fixes would be new `fix(release)` or `test(release)` commits,
  followed by a new QA proposal revision.
- Root acceptance of the second review recorded above.
- Root decisions already listed: W2c, the labelled-image runtime fingerprint, PW-1…PW-8, R-3,
  C-1…C-7.

Resources: this section adds one local commit and private notes. The scratch pre-image copies are in
the session scratchpad, outside the repository. No container, volume, image, push, merge, tag or
deletion.

## Pre-QA fixes of the re-review items (2026-11-13T00:25Z)

This section records the owned fixes of the non-blocking items of the three reports above, made
before any QA admission so that a later source freeze is not invalidated. **Nothing has been
compiled or run**: every test, Clippy, image build and runtime check is `NOT_RUN`, and QA is
`BLOCKED` until a QA admission exists. A static reading is not a compile result and not a `PASS`.
Nothing here is production-ready; no gate (G01, G12) and no risk (RC-P3-R11, RC-P7-R07, RC-P7-R04,
RC-P7-R12) changes status.

### Source identity after the pre-QA fixes

| Item | Value |
|---|---|
| Starting point | `006f1522df88bd026646cb282461c4948251857f` (the re-review record above), worktree clean |
| Code head proposed for QA | `abfbf7cbe6e208d10ad403b0eade2af4adf4e20e`, tree `26199374629402d7d4fa5d157cbab5d82a32be51`, `git archive --format=tar` SHA-256 `958532d75d74aab712bfde4c6c90581a64acb150a070d018f874cd7568b65e49`, 2 618 tracked files. It supersedes `c7cc37e` as the code head for QA |
| Delivery commit | the `docs(evidence)` commit that adds this section, child of `462b6b5`; it changes this file only. Its SHA is reported to the coordinator, not here |
| QA proposal | the private `QA_READY.r2.json` names `c7cc37e` and 17 unit tests: it no longer describes the code head; a new revision for `abfbf7c` is the coordinator's decision |
| SHA actually tested | none (`NOT_RUN`) |

No lot test reads the release guides or this file, so a QA run on `abfbf7c` covers the code of the
delivery commit. `git diff --name-only 006f152 abfbf7c`: `release_metadata/{artifact,output,tests}.rs`
only. `tests/`, the wire files, `images/`, `Cargo.*` and every shared or Root-owned file are unchanged.

### Commits (006f152..delivery)

| Commit | Subject | Items |
|---|---|---|
| `4f55cb2bd2fcb593c6096e77d5001f367f60f3e3` | test(release): observe the opens and the name swaps of the artefact reader | security RR-N1, correctness NB1/NB2, second review NB1/NB2 |
| `b47a1b891311771f9121330d5d8a37af608bb525` | fix(release): check locally that an artefact entry name is one component | second review NB4 |
| `b739006acb9e9e666b01a43432f6639c3a2d5ef0` | test(release): cover the remaining profile forms, the first-reason order and a root peerDependenciesMeta | correctness NB3, security RR-N4 |
| `abfbf7cbe6e208d10ad403b0eade2af4adf4e20e` | fix(release): say only what is known when the staging directory is not renamed | second review NB5 |
| `462b6b5f9944a46861807d678ecbaaa913f7c02b` | docs(release): state the artefact swap residual and the exact NotPublished text | security RR-N2/RR-N3, correctness NB4, second review NB3/NB5 (guide row) |
| delivery | docs(evidence): this section | record |

### What changed, and why each new test should fail on its defect (prediction, `NOT_RUN`)

- **Open order of the artefact reader (RR-N1, NB1).** A `#[cfg(test)]` recorder, `OPENED`, lists the
  names passed to `openat` by `open_at`, the only `openat` call of the module. In
  `an_artifact_name_is_opened_only_once_it_is_seen_as_a_regular_file` the regular file is the positive
  control (one open recorded), and the symbolic link, directory, FIFO, Unix-domain socket and
  `/dev/null` must leave it empty. A second, kernel-level oracle: `open(2)` on a socket fails (ENXIO on
  Linux, EOPNOTSUPP on macOS), so an implementation that opens first gets an error, whatever code
  opens the name, and `digest` names another cause than `must be a regular file`.
- **`Entry::Replaced` (NB2).** A `#[cfg(test)]` hook, `BEFORE_OPEN`, runs between `fstatat` and
  `openat`. `an_artifact_name_replaced_after_its_type_was_read_is_refused` renames a regular file over
  the name (`Entry::Replaced`, then `was replaced while it was opened; nothing was bound` from
  `digest`) and, on a second thread with a 10 s bound, a FIFO (prompt `Entry::Replaced`; a missing
  `O_NONBLOCK` fails instead of hanging). Both cases record the one open of the replacing entry,
  which is the documented residual.
- **One-component names (second review NB4).** `open_at` and `open_regular` now call `component()`
  before their `unsafe` call: `InvalidInput` for an empty name, `.`, `..`, a name holding `/` or NUL.
  Their SAFETY comments say the check is local. `open_at` is now `pub(super)` (like `open_regular`) so
  that `an_artifact_entry_name_must_be_one_path_component` reaches its check; without it, `.`, `..` and
  `directory/` resolve to directories and `directory/agentsandbox` to a regular file. Every current
  caller already passed a safe name: this is defence in depth, not a reachable defect.
- **TOML and npm branches (NB3) and the first-reason order (RR-N4).** Three profile forms
  (`[[profile]]`, an array and an inline-table value in `[profile.release]`), one npm root with only
  `peerDependenciesMeta`, and two files with two reasons each, in both orders of the rules that give
  them. Removing a condition, or replacing `first_reason` by plain assignment in one arm, fails a case.
- **`NotPublished` text (second review NB5).** The message now reads `the staging directory was not
  renamed to <output> (the cause is not reported: …); ai-guard replaced and removed nothing and did not
  move the staging directory it created at <staging>`. A `#[cfg(test)]` hook, `BEFORE_PUBLISH`, runs
  after the last absence check and before the rename;
  `a_publication_that_did_not_rename_names_only_what_is_known_and_removes_nothing` creates an entry at
  the final name (kept with its content; the staging directory holds every document), then moves a
  staging directory away (the old text, `is kept at <staging>`, fails this case).
- **Guides (RR-N2, RR-N3, NB4, second review NB3).** EN/FR line 112 and one "Other limits" bullet state
  the swap residual, including another user's own entry in a root-owned sticky directory; the
  data-hygiene list of repeated build-input text is given as examples; the failure-table row quotes the
  new `NotPublished` text.

No production behaviour changes except the refusal of names that are not one component (unreachable
from current callers) and the `NotPublished` text. The hooks, the recorder and their types exist only
under `cfg(test)`. The five product `unsafe` blocks of `artifact.rs` are unchanged in content; they
moved from `:135, :140, :165, :177, :244` to `openat :169`, `from_raw_fd :174`, `fstatat :199`,
`assume_init :211` and `geteuid :285`, and the SAFETY comments of `openat` and `fstatat` now name the
local check. No `unsafe` block was added. No crate, feature or dependency was added.

### Missing qualification at `abfbf7c`

The selectors of "Missing qualification" above are unchanged except for the code SHA and the unit
count: `cargo test --frozen --offline -j 1 --lib operations::source_snapshot::release_metadata:: --
--nocapture --test-threads=1` now expects **20** passed (17 plus
`an_artifact_name_replaced_after_its_type_was_read_is_refused`,
`an_artifact_entry_name_must_be_one_path_component` and
`a_publication_that_did_not_rename_names_only_what_is_known_and_removes_nothing`); the integration
targets (12 and 6) and the regressions (30, 42 plus 1 ignored, 12) are unchanged. Every selector is
`NOT_RUN`.

Environment facts the new tests need, to be recorded as such if they fail (never as a product
`FAIL` without a reading): the QA scratch ancestors must pass the export ancestor policy (as for the
earlier artefact tests); `/dev/null` must exist; `AF_UNIX` sockets must be creatable under the scratch
and its path must fit `sun_path` (about 108 bytes on Linux); the FIFO case allows 10 s. The QA-only
items of the reports (compile, Clippy `-D warnings`, `fstatat`/`fstat` identity on the QA filesystem,
optional `AT_NO_AUTOMOUNT`) stay open.

### Static checks run (no project code executed)

| Command | Result |
|---|---|
| `rustfmt --check --edition 2024` (file mode, toolchain 1.99.1 of `rust-toolchain.toml`) on `release_metadata/artifact.rs`, `output.rs`, `tests.rs` at `abfbf7c` | rc 0 each; during editing the same command reported a real difference (rc 1), so file mode is a live oracle |
| `git diff --check 006f152 HEAD` | rc 0 |
| `git diff --name-only 006f152 HEAD` | the three `.rs` files above, both release guides and this file |
| EN/FR guides | 394 lines, 110 table rows, 22 fences, 16 anchors each; backticked tokens identical; the new row's message pieces found in `output.rs` with string continuations joined |
| Compile-realism by reading | every syntax form used has a compiled base precedent (`#[cfg(test)]` on a statement and on a block, `const` `RefCell<Option<…>>` thread-locals, `Some(Box::new(…))` into a boxed hook) |

### Still open

- QA admission, then the first compile signal and the selectors at `abfbf7c`.
- Out of this repair: correctness NB5 (Root decides whether `was replaced while it was opened` joins
  the environment list), the QA-only items (RR-N5, NB6/NB7, second review NB6), and the designation of
  the second review (RR-N6, coordinator).
- Root decisions already listed: W2c, the labelled-image runtime fingerprint, PW-1…PW-8, R-3,
  C-1…C-7.

Resources: this section adds local commits only. No container, volume, image, push, merge, tag or
deletion.

## Corrections avant QA / Pre-QA fixes (2026-11-13T00:25Z)

This section records the independent verification of the pre-QA fixes described in the section
above, the QA proposal revision that follows from the new code SHA, and the proposed-wire re-check.
It changes no code. **Nothing has been compiled or run**: every test, Clippy, image build and runtime
check is `NOT_RUN`, and QA is `BLOCKED` until a QA admission exists (`QA_ADMISSION.json` absent). A
static reading is not a compile result and not a `PASS`. Nothing here is production-ready; no gate
(G01, G12) and no risk (RC-P3-R11, RC-P7-R07, RC-P7-R04, RC-P7-R12) changes status.

### Source identity and QA proposal

| Item | Value |
|---|---|
| Code head proposed for QA | `abfbf7cbe6e208d10ad403b0eade2af4adf4e20e`, tree `26199374629402d7d4fa5d157cbab5d82a32be51`, `git archive --format=tar` SHA-256 `958532d75d74aab712bfde4c6c90581a64acb150a070d018f874cd7568b65e49`, 2 618 tracked files (recomputed natively, equal to the section above and to both verification reports). It supersedes `c7cc37e` |
| Previous delivery commit | `62d115b0663aa57a25a3420e6c031efd222c4ae7` (the section above); it differs from `abfbf7c` by documentation only (`462b6b5`: both release guides; `62d115b`: this file) |
| Delivery commit | the `docs(evidence)` commit that adds this section, child of `62d115b`; it changes this file only. Its SHA is recorded in the private handoff index, not here |
| QA proposal | the private `QA_READY.r3.json` (`b59b5d8a…2b3d`, `PROPOSED_ROOT_ADMISSION`, execution `NOT_RUN`), which supersedes `QA_READY.r2.json` (`345bb949…3a20`) and freezes `abfbf7c` with the revision-1 profile; revisions 1 and 2 are kept unchanged |
| SHA actually tested | none (`NOT_RUN`) |

No lot test or build input reads the release guides or this file (`git grep` over `src`, `tests` and
`.dockerignore` at `abfbf7c`: no reference; `docs/` is not a build input), so a QA run on `abfbf7c`
covers the code of the delivery commit.

### Items fixed and their verification

Detail and per-test predictions are in the section above. Status after the independent verification
below, each test `NOT_RUN`:

| Item | Commit | Status after verification |
|---|---|---|
| Open-order test without an effect oracle (security RR-N1, correctness NB1, second review NB1) | `4f55cb2` | closed; residual V-2: one defect variant (a FIFO opened without `O_NONBLOCK` outside `open_at`) hangs the test instead of failing it |
| No test of `Entry::Replaced` (correctness NB2, second review NB2) | `4f55cb2` | closed |
| One-component name not checked before the `openat`/`fstatat` calls (second review NB4) | `b47a1b8` | closed; new precision V-1/NB-1: `open_at` is now `pub(super)`, and its flag precondition (no `O_CREAT`/`O_TMPFILE`, so no mode argument is read) is neither checked nor stated; unreachable from current callers |
| Three uncovered TOML/npm branches (correctness NB3) | `b739006` | closed |
| `first_reason` order without a two-reason case (security RR-N4) | `b739006` | closed for the two arms it covers (first verifier); partially_closed (second verifier, NB-2): 2 of the 7 `first_reason` sites, and one of the two facts CONTRACT §13 freezes |
| `NotPublished` text could name a wrong cause and place (second review NB5) | `abfbf7c`; guide row `462b6b5` | closed; wording nit V-3/NB-3: the hedged list of causes includes a parent synchronisation, and the guide row omits a staging synchronisation failure |
| Swap residual absent from the guides and CONTRACT (security RR-N2, correctness NB4, second review NB3) | `462b6b5`; CONTRACT §14 | closed |
| Data-hygiene list read as exhaustive (security RR-N3) | `462b6b5` | closed |

### New and changed tests (`NOT_RUN`)

- New unit tests (`operations::source_snapshot::release_metadata::tests::`):
  `an_artifact_name_replaced_after_its_type_was_read_is_refused`,
  `an_artifact_entry_name_must_be_one_path_component`,
  `a_publication_that_did_not_rename_names_only_what_is_known_and_removes_nothing`.
- Changed: `an_artifact_name_is_opened_only_once_it_is_seen_as_a_regular_file` (recorder and socket
  oracles), `toml_subset_reads_the_repository_manifests_and_refuses_constructs_outside_its_grammar`
  (three profile forms, two two-reason files), `npm_lock_refusals_cover_unknown_keys_nested_layouts_and_unsafe_urls`
  (a root holding only `peerDependenciesMeta`).
- Expected counts at `abfbf7c`, recounted from the attributes: unit **20** (20 `#[test]`, no
  `#[ignore]`; 17 at `c7cc37e`); `--test release_metadata` 12; `--test release_metadata_verify` 6;
  regressions 30 (26 `#[test]` + 4 `#[tokio::test]`), 42 plus 1 ignored (`paths.rs:824`, stays
  `NOT_RUN`) and 12 (`#[tokio::test]`). The regression inputs, `tests/`, the wire files and `images/` are
  unchanged since `c7cc37e` (`git diff --quiet`, rc 0).
- That each new or changed test fails on its defect is a prediction from reading, shared by both
  verifiers. A `PASS` would show only that the current code passes it; no mutant run is proposed.
- Environment needs to record as environment facts if they fail (`QA_READY.r3.json`): an `AF_UNIX`
  bind under the QA scratch within `sun_path` (108 bytes on Linux), `mkfifo` on the scratch, `/dev`
  and `/dev/null`, the export ancestor policy on the scratch ancestors, `RENAME_NOREPLACE` on the
  scratch filesystem, equal `(st_dev, st_ino)` from `fstatat` and `fstat`, and the 10 s FIFO bound.

### Independent verification (private notes, outside the repository)

| Note | Lens | Reviewed | Verdict |
|---|---|---|---|
| `notes/release-integrity/VERIFY_preqa_unsafe_open_path.md` (`6917fa09…529d`) | owned `unsafe`, open order, publication path | `62d115b` (code `abfbf7c`; range from `006f152`) | GO_WITH_CHANGES, 0 blocking, V-1–V-6 |
| `notes/release-integrity/VERIFY_preqa_tests_docs.md` (`f678f868…00d4`) | tests, compile realism, documentation | same | GO_WITH_CHANGES, 0 blocking, NB-1–NB-6 |

Each verifier is a Claude session that wrote none of the lot, of the fixes or of the re-review reports,
and worked read-only on the branch. Neither ran a compiler, a test, an engine, the QA harness or a
project binary. Their native checks were Git reads, `git archive | shasum`, and `rustfmt --check` in
file mode on hash-verified copies. Both found the following, by static reading:

- the freeze facts of `abfbf7c` are exact;
- only owned files changed, and the commits after `abfbf7c` are documentation only;
- the five product `unsafe` blocks are unchanged in substance;
- every new hook and the recorder exist only under `cfg(test)`;
- the counts are 20 + 12 + 6;
- there is no definite compile error.

The first report states that the project skill `ai-guard-security-review` was not loaded and that
the mandate was applied instead.

Open non-blocking items. None is treated: there is no code change after `abfbf7c`, and both
verifiers state that none is needed before QA.

| Item | What stays open |
|---|---|
| V-1 / NB-1 | `open_at` refuses `O_CREAT` (and `O_TMPFILE` on Linux), or passes an explicit mode, and its SAFETY comment states it |
| V-2 | inspect the socket before the FIFO in the open-order test, or bound the FIFO inspection; until then `QA_READY.r3.json` asks for a hang of that test to be read, not presumed a harness limit |
| V-3 / NB-3 | wording of the `NotPublished` cause list (`output.rs`) and of the guide row EN/FR |
| V-4 (pre-existing since `c7cc37e`) | an inode number reused between `fstatat` and `openat` is not seen as `Replaced`; guide `:112` is then not strictly true; the case stays within the documented bound of one stray open |
| NB-2 | two-reason cases for the five remaining `first_reason` sites and for the package/licence fact |
| NB-4 | the sibling message `<output> appeared while the documents were staged` can name a wrong cause and place when the parent moved |
| V-5 / NB-6 | QA-only: compile (two coercion and move points named by the verifiers), Clippy `-D warnings`, every selector, the environment needs above; correctness NB5 (Root) |
| V-6 / NB-5 | the QA proposal for `abfbf7c`: done, `QA_READY.r3.json` |

### Proposed-wire applicability, re-checked

`git apply --check` on 13 November 2026 for each patch of the private `proposed-wire/` folder against
the base it declares. No existing patch file was modified; no file was changed in any checkout.

| Patch | Declared base | Method | Result |
|---|---|---|---|
| PW-1, PW-2, PW-5, PW-6, PW-7 | `77f7edd` | in the worktree at `62d115b`: every touched blob equals its base blob, new files absent at both | rc 0 each; status empty before and after |
| PW-3, revision 1 | `c7cc37e` | 9 pre-images exported with `git show` into the session scratchpad, checked with `git hash-object` | rc 0; 9/9 post-images equal the declared blobs. Also rc 0 at `abfbf7c`, but rc 1 at `62d115b`: `462b6b5` changed the context of its "Other limits" hunk (EN/FR) |
| PW-3, revision 2 (new, `0003-…r2.patch`, `968ab7fb…2257`) | `62d115b` (its code and recipe pre-images are the `abfbf7c` blobs) | in the worktree at `62d115b`; on a fresh scratch copy; then PW-8 on top | rc 0; 9/9 post-images equal the declared blobs; the `+`/`-` lines equal revision 1 for 9/9 files; PW-8 after it rc 0 |
| PW-8 | `77f7edd` | 2 pre-images exported the same way (the worker recipe differs on the branch) | rc 0; 2/2 post-images equal the declared blobs; also rc 0 in the worktree at `62d115b` |

Revision 2 of PW-3 changes only offsets, the context of the guide hunk and three `index` lines. It
applies to the QA freeze `abfbf7c` without its two guide files (`--exclude='docs/setup/release/*'`,
rc 0). The delivery commit of this section touches only this file, which no patch touches.

### Still open before composition

- QA admission for `QA_READY.r3.json` at `abfbf7c`, then the first compile signal and every selector.
- The non-blocking items above; correctness NB5 (Root); the designation of the second review (RR-N6,
  coordinator) and its acceptance by Root.
- Root decisions already listed: W2c, the labelled-image runtime fingerprint, PW-1…PW-8 (PW-3 now as
  revision 2), R-3, C-1…C-7.

Resources: this section adds one local commit. Outside the repository: `QA_READY.r3.json`, the PW-3
revision-2 patch, an appended note in `proposed-wire/PROPOSED_WIRES.md`, handoff index and work-log
entries, and scratch pre-image copies in the session scratchpad. No container, volume, image, push,
merge, tag or deletion.

## Final pre-QA round (2026-11-13T01:20Z)

This section records the last fix round before QA admission. The coordinator listed the items from
the two verification reports of the section above: V-1/NB-1, V-2, V-3/NB-3 and NB-4 are repaired,
and V-4 is stated as a residual. **Nothing has been compiled or run**: every test, Clippy, image build
and runtime check is `NOT_RUN`, and QA is `BLOCKED` until a QA admission exists. A static reading is
not a compile result and not a `PASS`. Nothing here is production-ready; no gate (G01, G12) and no
risk (RC-P3-R11, RC-P7-R07, RC-P7-R04, RC-P7-R12) changes status.

### Source identity

| Item | Value |
|---|---|
| Start | `4cb07a6bdf0ac92db95b3568af7ed9e97d997f8f` (delivery of the section above); `git status --porcelain=v1 --ignored` empty |
| Code head proposed for QA | `fdc0c87714ad0de30056825f95e5b195ef63af0d`, tree `a5ed42c9f0a83eb9904fea37e255a5720122e01e`, `git archive --format=tar` SHA-256 `a1185835b72367b3cfeac111ad49c2ce0aa13ad7fda260376fa22b965559d7bc`, 2 618 tracked files. It supersedes `abfbf7c` |
| Delivery commit | the `docs(evidence)` commit that adds this section, child of `2dba789`; it changes this file only. Its SHA is recorded in the private work log, not here |
| QA proposal | not revised by this round: `QA_READY.r3.json` still freezes `abfbf7c` and 20 unit tests. A revision for `fdc0c87` is owed by the coordinator before admission |
| SHA actually tested | none (`NOT_RUN`) |

### Commits (`4cb07a6`..delivery)

| Commit | Item | Files |
|---|---|---|
| `31317c53497e3b9a784127da65067222e55bc979` `fix(release): admit only the read-only lookup flags when an artefact entry is opened` | V-1 / NB-1 | `release_metadata/artifact.rs`, `release_metadata/tests.rs` |
| `25a00e88a93f574c7489fde811c51385f419ce46` `test(release): bound the open-order test so every open-first defect fails in time` | V-2 | `release_metadata/tests.rs` |
| `6619dd2a2306da1c964fcd2e2e7d801b01a9e556` `fix(release): list only the reachable causes when the staging directory is not renamed` | V-3 / NB-3 | `release_metadata/output.rs` |
| `fdc0c87714ad0de30056825f95e5b195ef63af0d` `fix(release): say only what is known when the final name is not confirmed absent` | NB-4 | `release_metadata/output.rs`, `release_metadata/tests.rs` |
| `2dba7897bef68244ca87771308f6b66e3226982c` `docs(release): state the reachable publication causes and the inode-reuse residual` | V-3, NB-4 (guide row), V-4 (residual) | both release guides |

Each commit's parent is the previous one, and each message ends with the exact trailer line.

### What changed, and why each test should fail on its defect (prediction, `NOT_RUN`)

- **V-1 / NB-1.** `artifact::open_at` now refuses any flag outside
  `LOOKUP_FLAGS = DIRECTORY_FLAGS | FILE_FLAGS` with `InvalidInput`. The refusal comes before the
  `openat` recorder and the `unsafe` call.
  - An allowlist is used rather than a denylist. In the pinned libc 0.2.191, `O_TMPFILE` is
    `0o20000000 | O_DIRECTORY` on every Linux env and arch and on Android, and Apple has no
    `O_TMPFILE`. A check written as `flags & O_TMPFILE != 0` would therefore refuse the directory
    lookups.
  - Two `const` assertions check at compile time that `LOOKUP_FLAGS` holds no `O_CREAT` and, on Linux
    and Android, not every bit of `O_TMPFILE`. These are the two cases where libc reads a mode from
    the variadic part of `openat`.
  - The SAFETY comment now states that precondition and how it is enforced. Every caller passes
    `DIRECTORY_FLAGS`, `FILE_FLAGS` or a subset of them, so no reachable input changes behaviour.
  - New unit test `an_artifact_open_admits_only_the_read_only_lookup_flags`. Without the refusal,
    the name reaches the recorder and `openat` creates an entry (or fails with another error kind).
    A check that refuses any `O_TMPFILE` bit fails its positive control, the directory lookup.
- **V-2.** `an_artifact_name_is_opened_only_once_it_is_seen_as_a_regular_file` now fails, rather than
  hangs, on every variant of an open made before the type check:
  - its checks run on a spawned thread, and the test thread bounds their end with
    `recv_timeout(30 s)`. This is a bound against a hang, not a latency measurement;
  - the socket is looked at before the FIFO in both loops, so an open-first defect fails at once on
    the socket oracle;
  - the assertions are unchanged.
- **V-3 / NB-3.** The `NotPublished` message and comment now list only reachable causes:
  - a revalidation of the parent or of the staging directory;
  - a synchronisation of the staging directory;
  - the rename itself: an entry at the final name, or any other refusal of the system.
  The parent is synchronised only after the rename, where a failure gives `DurabilityUnknown`. The
  message prefix and its known facts are unchanged. No test is added for a wording that a test could
  only copy.
- **NB-4.** The last absence check before the rename now reports: `<output> was not confirmed absent
  before the rename, so the staging directory was not renamed to it; ai-guard replaced and removed
  nothing and did not move the staging directory it created at <staging>, where it wrote and
  synchronised the documents`. Its cause still follows in the error chain.
  - A `#[cfg(test)]` hook `output::BEFORE_FINAL_CHECK` runs between the staging synchronisation and
    that check.
  - Two cases are appended to `a_publication_that_did_not_rename_names_only_what_is_known_and_removes_nothing`:
    an entry created at the final name, and the parent moved.
  - The old text claimed an appearance and "is kept at <staging>". It fails the moved-parent case.
- **V-4 (stated, not repaired).** Guide EN/FR: the artefact rule and a new "Other limits" bullet
  state that a replacing entry that reuses the inode number of the name that was read is not seen as
  a replacement. A regular file is then bound, as if the swap had been made before the read; another
  type is opened once, then refused. The bullet is added after the existing swap bullet, so the
  trailing context of PW-3 revision 2 is unchanged.

### Expected counts and environment facts at `fdc0c87`

- Unit tests: **21** `#[test]`, no `#[ignore]` (20 at `abfbf7c`). The name diff is exactly
  `an_artifact_open_admits_only_the_read_only_lookup_flags`.
- Integration targets: `--test release_metadata` 12, `--test release_metadata_verify` 6. `tests/` is
  unchanged since `c7cc37e` (`git diff --quiet`, rc 0).
- Product `unsafe` blocks: the same five in `artifact.rs` (`openat`, `from_raw_fd`, `fstatat`,
  `assume_init`, `geteuid`), none in `output.rs`. The test code has one (`mkfifo`).
- New facts to record as environment facts if they fail:
  - the 30 s bound of the open-order test;
  - the moved-parent case renames a directory inside the private scratch directory.
  - The `O_TMPFILE` case is refused before any system call, so it needs no `O_TMPFILE` support from
    the scratch filesystem.
- The earlier environment needs listed in the section above still apply.

### Static checks run (no project code executed)

| Check | Result |
|---|---|
| `rustfmt 1.10.0-stable --check --edition 2024`, file mode, on `artifact.rs`, `output.rs` and `tests.rs` before each commit that changed them | rc 0 each |
| `git diff --check 4cb07a6 2dba789` | rc 0 |
| `git diff --quiet 4cb07a6 2dba789 --` over the shared, Root and recipe set (`src/operations.rs`, `source_snapshot.rs`, `capture.rs`, `images`, `Cargo.*`, `src/{main,lib,api,audit}.rs`, `docs/api.md`, `src/runtime`, `src/runner`, `src/control_fs*`, `tests`, `.dockerignore`, `AGENTS.md`) | rc 0; no `build.rs` |
| Other `release_metadata` files (`declared`, `npm_lock`, `compare`, `document`, `cyclonedx`, `cargo_lock`, `toml_subset`, `release_metadata.rs`) against `abfbf7c` | byte-identical (`git diff --quiet`, rc 0) |
| Guides EN/FR at `2dba789` | 395 lines, 110 table rows, 22 fences and 16 anchors each; identical backtick-token sequences and per-line `|` counts |
| `git apply --check` of PW-1, PW-2, PW-3 revision 2, PW-5, PW-6, PW-7 and PW-8 on the tree at `2dba789` | rc 0 each |
| `cfg(test)` scope | `BEFORE_FINAL_CHECK` is referenced only in its `#[cfg(test)]` declaration and block and in the test module |

### Still open

- A QA proposal revision for `fdc0c87` (coordinator), then QA admission, the first compile signal and
  every selector.
- Not treated, as the coordinator directed:
  - NB-2: two-reason cases for the remaining `first_reason` sites;
  - V-4 code repair;
  - V-5 / NB-6 (QA-only);
  - correctness NB5 (Root);
  - RR-N6 (coordinator);
  - the Root decisions listed above.
- Observed, not changed:
  - the module text of `artifact.rs` (lines 11-13) does not name the inode-reuse exception;
  - the staging-failure message `what was written is kept at <staging>` names a place that is no
    longer exact if the parent moved during staging. It is the same class as NB-4, but was not listed.

Resources: this round adds six local commits. Outside the repository it adds appended dispositions
in the two verification reports, a CONTRACT addendum, a work-log entry and scratch copies in the
session scratchpad. No container, volume, image, push, merge, tag or deletion.

## Dernier tour avant QA / Final pre-QA round (2026-11-13T01:20Z)

This section records the independent verification of the final pre-QA round described in the
section above, the QA proposal revision that follows from the new code SHA, and the proposed-wire
re-check. It changes no code. **Nothing has been compiled or run**: every test, Clippy, image build
and runtime check is `NOT_RUN`, and QA is `BLOCKED` until a QA admission exists (`QA_ADMISSION.json`
absent). A static reading is not a compile result and not a `PASS`. Nothing here is production-ready;
no gate (G01, G12) and no risk (RC-P3-R11, RC-P7-R07, RC-P7-R04, RC-P7-R12) changes status.

### Source identity and QA proposal

| Item | Value |
|---|---|
| Code head proposed for QA | `fdc0c87714ad0de30056825f95e5b195ef63af0d`, tree `a5ed42c9f0a83eb9904fea37e255a5720122e01e`, `git archive --format=tar` SHA-256 `a1185835b72367b3cfeac111ad49c2ce0aa13ad7fda260376fa22b965559d7bc`, 2 618 tracked files (recomputed natively, equal to the section above and to the verification report). It supersedes `abfbf7c` |
| Previous delivery commit | `63511f0d13431eb1ad416b69d127b57874f98e3e` (the section above); it differs from `fdc0c87` by documentation only (`2dba789`: both release guides; `63511f0`: this file) |
| Delivery commit | the `docs(evidence)` commit that adds this section, child of `63511f0`; it changes this file only. Its SHA is recorded in the private handoff index, not here |
| QA proposal | the private `QA_READY.r4.json` (`e7863cfd…1368`, `PROPOSED_ROOT_ADMISSION`, execution `NOT_RUN`). It freezes `fdc0c87` with the revision-1 profile and supersedes `QA_READY.r3.json` (`b59b5d8a…2b3d`). Revisions 1, 2 and 3 are kept unchanged |
| SHA actually tested | none (`NOT_RUN`) |

No lot test or build input reads the release guides or this file (`git grep` over `src`, `tests` and
`.dockerignore` at `fdc0c87`: no reference; `docs/` is not a build input), so a QA run on `fdc0c87`
covers the code of the delivery commit.

### Items fixed and their verification

Detail and per-test predictions are in the section above. Status after the independent verification
below:

| Item | Commit | Status after verification |
|---|---|---|
| V-1 / NB-1: `open_at` flag precondition | `31317c5` | closed (`NOT_RUN`); test strength F-1 below |
| V-2: the open-order test could hang on one defect variant | `25a00e8` | closed (`NOT_RUN`) |
| V-3 / NB-3: `NotPublished` cause list | `6619dd2`; guide row `2dba789` | closed |
| NB-4: the sibling message could name a wrong cause and place | `fdc0c87`; guide row `2dba789` | closed (`NOT_RUN`) |
| V-4: an inode number reused between `fstatat` and `openat` | guides `2dba789`; CONTRACT §15 | stated as a residual, as directed; code not repaired |

### New and changed tests (`NOT_RUN`)

- New unit test (`operations::source_snapshot::release_metadata::tests::`):
  `an_artifact_open_admits_only_the_read_only_lookup_flags`.
- Changed:
  - `an_artifact_name_is_opened_only_once_it_is_seen_as_a_regular_file`: the checks run on a spawned
    thread with a 30 s bound, and the socket is inspected before the FIFO;
  - `a_publication_that_did_not_rename_names_only_what_is_known_and_removes_nothing`: two cases
    (`release-3`, `release-4`) through the `#[cfg(test)]` hook `output::BEFORE_FINAL_CHECK`.
- Expected counts at `fdc0c87`, recounted from the attributes:
  - unit **21** (21 `#[test]`, no `#[ignore]`; 20 at `abfbf7c`);
  - `--test release_metadata` 12 and `--test release_metadata_verify` 6 (`tests/` unchanged since
    `c7cc37e`);
  - regressions 30 (26 `#[test]` + 4 `#[tokio::test]`), 42 plus 1 ignored (`paths.rs:824`, stays
    `NOT_RUN`) and 12 (`#[tokio::test]`);
  - optional binary tests: 49, counted statically.
- That each new or changed test fails on its defect is a prediction from reading. A `PASS` would show
  only that the current code passes it; no mutant run is proposed.
- Environment facts added to `QA_READY.r4.json`, to record as such if they fail:
  - the 30 s bound of the open-order test;
  - a directory rename inside the private scratch directory (`release-2`, `release-4`);
  - the `O_TMPFILE` case is refused before any system call and needs no filesystem support.
- The earlier environment needs still apply: `AF_UNIX` bind within `sun_path`, `mkfifo`, `/dev/null`,
  the ancestor policy, `RENAME_NOREPLACE`, equal `(st_dev, st_ino)` and the 10 s FIFO bound.
- First compile signal to watch: the two `const` assertions of `artifact.rs:77-79` on the Linux target.

### Independent verification (private note, outside the repository)

| Note | Lens | Reviewed | Verdict |
|---|---|---|---|
| `notes/release-integrity/VERIFY_final_preqa.md` (`6b01f691…669a`) | `unsafe` and open path, tests, publication text, scope and identity | `63511f0` (code `fdc0c87`; range from `4cb07a6`) | **GO_WITH_CHANGES**, 0 blocking, F-1–F-5 and N-1 |

The verifier is a Claude subagent that wrote none of the commits under review and none of the earlier
reports. It worked read-only on the branch and ran no compiler, test, engine, QA harness or project
binary. Its native checks were:

- Git reads and `git archive | shasum`;
- `rustfmt --check` in file mode on hash-verified copies, with a negative control;
- the libc crate checksum against `Cargo.lock`;
- `git apply --check` of the proposed wires on exported copies.

It found the following by static reading:

- the four targeted items are closed in source, tests and text;
- the `openat` call is sound by construction for every flag value `open_at` accepts, on the targets
  the repository builds for;
- the freeze facts of `fdc0c87` are exact;
- only owned files changed, and the commits after `fdc0c87` are documentation only;
- the counts are 21 + 12 + 6.

It states that the project skill `ai-guard-security-review` was not loaded and that the mandate was
applied instead.

Open non-blocking items. None is treated in code: this was the last fix round before QA admission.

| Item | What stays open |
|---|---|
| F-1 | both refused cases of the V-1 test also carry an access mode outside `LOOKUP_FLAGS`, so the test does not isolate the creation bit; suggested case `O_RDONLY \| O_CREAT \| O_EXCL \| O_CLOEXEC`, with the next code change |
| F-2 | the QA proposal for `fdc0c87`: done, `QA_READY.r4.json` |
| F-3 | QA-only: compile, Clippy `-D warnings`, every selector and the environment facts above |
| F-4 | carried over: the V-4 code repair, the module text of `artifact.rs:11-13`, NB-2, correctness NB5 (Root) and RR-N6 (coordinator) |
| F-5 | the staging-failure message `what was written is kept at <staging>` (`output.rs:162-168`) can name a place that is no longer exact if the stage or parent moved; same class as NB-4 |
| N-1 | precision: the `fdc0c87` message and the section above say that the old text fails the moved-parent case; it fails both new cases |

### Proposed-wire applicability, re-checked

`git apply --check` on 13 November 2026 for each patch of the private `proposed-wire/` folder against
the base it declares. No existing patch file was modified; no file was changed in any checkout.

| Patch | Declared base | Method | Result |
|---|---|---|---|
| PW-1, PW-2, PW-5, PW-6, PW-7 | `77f7edd` | in the worktree at `63511f0`: every touched blob equals its base blob, new files absent at both | rc 0 each; status empty before and after |
| PW-3, revision 1 | `c7cc37e` | 9 pre-images exported with `git show` into the session scratchpad, checked with `git hash-object` | rc 0; 9/9 post-images equal the declared blobs |
| PW-3, revision 2 | `62d115b` | same method | rc 0; 9/9 post-images equal the declared blobs |
| PW-8 | `77f7edd` | 2 pre-images exported (the worker recipe differs on the branch) | rc 0; 2/2 post-images equal the declared blobs |
| PW-3, revision 3 (new, `0003-…r3.patch`, `dc6d8889…0dff`) | `63511f0` | in the worktree at `63511f0` and on a fresh scratch export; then PW-8 on top | rc 0; 9/9 post-images equal the declared blobs; the `+`/`-` lines equal revisions 1 and 2 for 9/9 files; PW-8 after it rc 0 |

Revision 3 of PW-3 differs from revision 2 only by three `index` lines (`release_metadata/tests.rs`
and both guides), whose pre-images changed on the branch, and by a header that declares its base.
Revision 2 still applies at `63511f0` (rc 0), but its declared pre-image blobs for those three files
are no longer the branch's. The name `…r2.patch` the coordinator asked for was already taken by
revision 2, which is kept unchanged. The delivery commit of this section touches only this file,
which no patch touches.

### Still open before composition

- QA admission for `QA_READY.r4.json` at `fdc0c87`, then the first compile signal and every selector.
- F-1, F-4 and F-5 above; the designation of the second review (RR-N6, coordinator) and its
  acceptance by Root.
- Root decisions already listed: W2c, the labelled-image runtime fingerprint, PW-1…PW-8 (PW-3 now as
  revision 3), R-3, C-1…C-7.

Resources: this section adds one local commit. Outside the repository: `QA_READY.r4.json`, the PW-3
revision-3 patch, an appended note in `proposed-wire/PROPOSED_WIRES.md`, handoff index and work-log
entries, and scratch pre-image copies in the session scratchpad. No container, volume, image, push,
merge, tag or deletion.
