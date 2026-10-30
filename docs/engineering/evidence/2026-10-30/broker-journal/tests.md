# Broker private journal — Luna test evidence (updated)

Worktree: `/Users/louis/Documents/ai-guard/.worktrees/broker-supervision`, base `3372150fdb99d8bc71700a33dd1d49c0e75c45b5`. Contract: `docs/engineering/BROKER_JOURNAL.md` SHA256 `090cf39e1acfd6dbddd52daeb48a5304c7f40b5612d0a699c6fa270d3f5eec28`. Core implementation manifest: `/tmp/ai-guard-broker-journal-implementation.md` SHA256 `776eae0828c6b1e6852b4ecdd865199abd2a5973578b3eab2fd63bc202130978`. Luna changed only `tests/broker_journal.rs` and `src/broker/journal/tests.rs`; no application implementation, docs, commit or push by Luna.

## Final integration coverage

`tests/broker_journal.rs` final target: 9 passed, 0 failed, 0 ignored. The cases cover:

1. `CheckpointPayloadV1::from_plan_bytes` with the same valid legacy plan as the intent, including known `execution_mode: null`; typed `DecisionRequest` serialization twin; real approved request through `from_plan_bytes`; unknown null rejected with an opaque error; trailing second JSON rejected without echo.
2. None binding durable acceptance/reopen, exact replay without source, divergent Submit conflict, 0600/single-link record.
3. Reservation liveness: `InProgress`, independent opener `Busy`, divergent Submit conflict, ticket drop/reopen `Interrupted`.
4. Typed approved checkpoint CAS, linear revisions, exact historical receipt replay after later revisions, divergent payload conflict at the same expected revision.
5. Literal ControlSecrets and configured input caps rejected before job creation.
6. Manual ENV canonicalization, source/stored/handoff digest relationship, source removal, accepted replay.
7. Provider raw record copy, binding identity and handoff digest; escaped JSON secret rejected after decoding before binding persistence.
8. Symlink/hardlink replacement rejected without repair or change to external target.
9. Concrete capacity checks: a live partial reservation consumes `max_jobs`; `max_job_entries` blocks acceptance while preserving only its reservation; `max_revisions=1` blocks revision 1 without a write; a measured cap based on the actual intent/checkpoint sizes permits acceptance and refuses an oversized advance without publishing the pointer.

The known-null regression was captured before the codec fix: `from_plan_bytes` returned `InvalidInput` for the valid legacy plan at `/tmp/ai-guard-broker-journal-green.1496/checkpoint-builder.log` (exit 101). After the frozen `serde_ignored` correction, the focused builder test passed including its unknown-null, trailing-data, approval, and diagnostic opacity controls.

## Unit hook and crash scenarios

`src/broker/journal/tests.rs` contains cfg(test)-only fault hooks after real filesystem operations. The child helper is `#[ignore]` and is explicitly spawned by its parent with `--ignored`; it is not counted as an independent product pass. The parent owns and waits for each child, kills it at a private barrier, then reopens the real journal.

The final unit command reports 3 passed, 1 ignored, 86 filtered. The three active tests are: hook-dispatch self-test (harness only); real journal one-shot `Io` injected at reservation durability, acceptance `BeforeAck`, and Advance `CurrentPublished`, each followed by no-ACK and conservative recovery/load assertions; and parent-driven SIGKILL scenarios covering reservation/intent durability, accepted-written/accepted-durable/BeforeAck, plus Advance `RevisionDurable` orphan-not-promoted, `CurrentPublished`, `CurrentDurable`, and post-Advance `BeforeAck`. At the orphan boundary, the current pointer remains at revision 0 and the durable revision-1 orphan is not selected. The `CurrentPublished` callback runs after the filesystem helper’s rename and directory sync, so the inter-syscall interval remains untested. SIGKILL is not a power-loss simulation.

## Commands and final results

- `cargo test --locked --test broker_journal`: PASS, 9 passed, 0 failed/ignored, 0.76 s. Log: `/tmp/ai-guard-broker-journal-green.1496/integration-expanded-final.log`; result: `integration-expanded-final.result`.
- `cargo test --locked --lib broker::journal::tests`: PASS, 3 passed, 1 ignored, 86 filtered, 1.01 s test time (6 s command duration). Log: `/tmp/ai-guard-broker-journal-green.1496/unit-advance-error.log`; result: `unit-advance-error.result`.
- Focused codec test: PASS, 1 passed; log `checkpoint-opacity.log`.
- Focused configured capacity test: PASS, 1 passed; log `concrete-caps.log`.

Earlier compile/fixture failures are retained in the same private evidence directory; they were corrected as test-harness issues before final target runs. The initial absent-module RED was a missing-feature compile RED, not evidence of a runtime vulnerability. Final manifests before/after each completed command match; see `source-before-final-targets.sha256`, `source-after-final-targets.sha256`, `source-before-unit-advance-error.sha256`, `source-after-unit-advance-error.sha256`, and `final-source-expanded.sha256`.

## Limits

These are focused macOS filesystem/process tests. They do not qualify power loss, hostile same-UID/root races, a real SQLite alias-lock integration (owned by root’s separate `journal_sqlite.rs`), Linux behavior, 100-worker capacity, IPC, restart fencing, broker scheduling, workload start, E2E, or any model/provider call. These were not run. No static analysis or release build was run in this Luna validation phase.

## Post-version and writer-lock race follow-up (supersedes prior counts above)

The follow-up added one integration test, `managed_checkpoint_rejects_unknown_runtime_schema_versions`, using a valid checkpoint assembled from a real SQLite decision/admission/worker transition sequence and a synthetic, non-running runtime authority. The baseline checkpoint itself parses successfully through `CheckpointPayloadV1::from_plan_bytes`. The useful pre-fix RED run showed seven unsupported-version variants accepted: pending mutation 2, uncertain mutation 2, invocation policy 999, owned handle fingerprint 999, owned observation fingerprint 999, import-owned handle fingerprint 999, and import-owned observation fingerprint 999. After that RED, the oracle was expanded with `workspace_control_exclusion_version` 999 and positive checks for invocation policy 1/2 and owned/observation fingerprints 1/2/3. Those additions were first run after the codec fix: the final integration test refused all eight unsupported values and accepted the supported values. Do not attribute the eighth case or the positive checks to the pre-fix RED run. No process, runtime, provider, or model was started.

A deterministic true-process writer-lock race test pauses an owned child after its real `.writer.lock` read reports absence and before `O_EXCL` creation. The parent then opens the same journal scope and holds the created lock before releasing the child. RED returned `JournalError::Io` where the second first-opener must return `Busy`. Core’s typed `AlreadyExists` re-read fix was then applied; the same process race passed. It does not use probabilistic stress or a fake filesystem.

Final focused commands and results:

- `cargo test --locked --test broker_journal`: PASS, 10 passed, 0 failed, 0 ignored. Log `/tmp/ai-guard-broker-journal-postfix/integration.log`.
- `cargo test --locked --lib broker::journal::tests`: PASS, 4 active parent tests passed, 2 child helpers ignored by the normal filter. The hook self-test is harness-only. The SIGKILL parent explicitly runs `crash_child_at_stage` for nine crash cases; the lock-race parent explicitly runs `writer_lock_race_child` once. Neither child helper is an independent product-test pass. Log `/tmp/ai-guard-broker-journal-postfix/unit.log`.
- `cargo test --locked --test credential_handoff --test provider_runner --test control_fs`: PASS, 26 passed total (7 + 4 + 15), 0 failed/ignored. Log `/tmp/ai-guard-broker-journal-postfix/canaries.log`.
- `cargo fmt --all -- --check`: PASS, final log `/tmp/ai-guard-broker-journal-postfix/fmt.log`. The first check found formatting differences only in the two owned test files; they were rustfmt-formatted before the passing check.
- `cargo clippy --locked --all-targets -- -D warnings`: initial attempt exposed three `collapsible_if` warnings in `src/runner.rs`; core applied equivalent let-chain syntax only. Final rerun PASS, log `/tmp/ai-guard-broker-journal-postfix/clippy.log`.
- `cargo build --release --locked`: PASS, log `/tmp/ai-guard-broker-journal-postfix/release.log`, exit 0. Release executable SHA256 `39d88ea07a6ec7de50288d726ce263647cbde20bd7fbfd3cce28483a8efd22a4`; `libagentsandbox.rlib` SHA256 `1419376473a47dcc2d02cdc18e2129e7524144f6c844844c43e493803536ab92`.

The complete manifest covers Cargo metadata, toolchain file, and all tracked/untracked non-ignored files under `src` and `tests` (111 entries). Pre/post-release manifests are byte-identical (SHA256 `cd9775b29d3fa75a67b08ddd395a081e980665c95369fcb2c84f5a326302d520`): `/tmp/ai-guard-broker-journal-postfix/source-hashes-before-release.txt` and `source-hashes-after-release.txt`. It includes the separate `tests/journal_sqlite.rs` owned by root; that test was not run by this agent. Final relevant hashes: journal `9de3129179300bee2fbb4558ef5fbf5c4e1269713b91a312790094316c685836`; runner `3b7f949519bc7f8bcd2f1b504e97e09ad1950f48de7c03c80aed0e660deb3805`; runtime managed `588bd096a37058e8747f4c3cbc27c29a860015537af1098761c23f4b4c523b14`; integration test `813561bf5a9c0d6ed2ef1fda09b85383d1fa5f8bafaebad186d5d20795fb8079`; journal unit tests `dbd5d4116e448a2aaeeb819ce834f3d9d7b182c6b849433c6b6981a3ebb4f7eb`.

### RED-log retention clarification

Raw redirected log files were not created for the seven-case version RED or the lock-race RED; their command outputs and exit 101 results are in the execution transcript, not a filesystem log. The version RED used test-source SHA256 `3965947a5d9e6fa88f3ce89d8a588e52667c612f609e26ce23835a846615718b`. The lock RED used journal source SHA256 `9a2c9fd4683e2033b1a627ed29715a1d4a96558a3df936067b85d850a3dbf3ba` and unit-test source SHA256 `2191c6fd0c591bf70f9e61059492ced68090e87117e604b27aeaddbb83c7a47a`. Their green counterparts are recorded in `integration.log` and `unit.log` above. The first version-test attempts had invalid fixture oracles and are not counted as RED. The first fmt and Clippy failure outputs were replaced by the final passing logs; only those initial diagnostics in the conversation transcript remain. `/tmp/ai-guard-broker-journal-red.kyXIpo/test.log` is the earlier missing-module compile RED, not either behavioral RED described here.

This is targeted macOS software validation and a release-profile build, not a runtime or production qualification. The separate gateway-origin mismatch case was not added in this follow-up. Existing limits above remain.
