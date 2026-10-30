# Runtime stop snapshot regression evidence

Status: **RED reproduced** against the pre-change runtime source. This is a fixture-backed behavior test, not a Docker/runtime qualification.

## Scope

- Added only `tests/runtime_stop_snapshot.rs` (SHA-256 `63ad38fe0ee04311bc44937b134fb7e449540eb07a1d670ebc9e355d1b96cda9`).
- Production source stayed unchanged during the RED run: `src/runtime.rs` SHA-256 `7e878ebc1d7389c390f1d2cdfde6efadbc001677162515f1cf44ff48fae74a42`; `src/runtime/managed.rs` SHA-256 `411dd20cee645fcc80003b600e33ac55d1f8af5c9f00bfb483012e610d6d028a`.
- The test compiles a private Rust fixture derived from `tests/runtime_fixtures/managed.rs`; it makes the fixture report the worker as running while its privileged bit or logging driver diverges after creation. It does not change the shared fixture or any production file.

## Command and result

Command: `cargo test --locked --test runtime_stop_snapshot -- --nocapture`

Result: exit 101, one top-level test failed. Both parameter cases were run and failed independently:

- `config_changed`: the log contains `kill <immutable-container-id>` after the observed configuration diverged; state became `exited`, expected `running`.
- `logging_changed`: same behavior for the changed logging driver.

The exact captured output is `/tmp/ai-guard-runtime-stop-red.xe94YI/target-red-evidence.log` (private mode 600). An earlier harness compile error (`PathBuf` passed directly to `fs::write`) was corrected before this behavioral result; it is not counted as a RED finding.

## Source basis and limits

Current `ManagedRuntime::execute_owned` handles an error by calling `stop_owned(&handle)`. Current `stop_owned` checks the handle authority, then may call `kill_target` using the immutable container ID without validating that inspected configuration still matches the launch snapshot. The fixture result demonstrates this path for two post-create divergences.

**Not run:** exact-snapshot stop, unobservable/refused inspection, terminal `Created` no-allocation, and the new `stop_snapshot_owned` port; that API was not present in the RED source. No Docker, provider, model, or global test suite was used.

## GREEN after snapshot-bound stop port

The core port was frozen before validation and matched these hashes: `src/runtime.rs` SHA-256 `b2a371c95892fa5c6c85e2271b2c8073325d5e7d3f292bb611dee406652f134d`; `src/runtime/managed.rs` SHA-256 `6bae8e0b6c0fd3a7d38b832c8fcd556ad5bde6c587ee067c19c29b6e5a719766`; root runner call sites `src/runner.rs` SHA-256 `d8ee798b3df2a942e514b79b17eb1937cf812c7dbad01a478eae08b123082ea4`.

Commands and results:

- `CARGO_INCREMENTAL=0 cargo test --locked --test runtime_stop_snapshot -- --nocapture` — PASS, 1 top-level test; all six fixture cases passed: config divergence, logging divergence, exact running snapshot stop, refused inspection, unknown Running fact, and Created observation with divergent/unallocatable memory. Log: `/tmp/ai-guard-runtime-stop-green.UEbe3q/target-green.log` (mode 600).
- `CARGO_INCREMENTAL=0 cargo test --locked --test runtime_managed` — PASS, 14/14. Log: `/tmp/ai-guard-runtime-stop-green.UEbe3q/runtime-managed-green.log` (mode 600).
- `CARGO_INCREMENTAL=0 cargo test --locked --test runner_recovery --test runtime_terminal` — PASS, 16/16 and 1/1. Log: `/tmp/ai-guard-runtime-stop-green.UEbe3q/runner-recovery-terminal-green.log` (mode 600).
- `rustfmt --edition 2024 --check tests/runtime_stop_snapshot.rs` — PASS before GREEN.

Final test source SHA-256: `tests/runtime_stop_snapshot.rs` `6eb3380bbda0d3afa97170331b5bbbff474b7de5049b6c4a86bbd7edaecb1379`. Existing `tests/runtime_managed.rs` was left unchanged because its config/logging fixture cases already terminate naturally before recovery and do not assert that a kill occurred; the new running fixtures exercise the missing defect directly.

No full suite, ignored runtime tests, Docker, provider, model, or API test was run. This fixture evidence verifies the snapshot stop contract and selected recovery regressions only; it does not qualify real runtime behavior or a production release.

## Manifest correction / provenance note

The earlier reference to `src/runner.rs` SHA `d8ee798b3df2a942e514b79b17eb1937cf812c7dbad01a478eae08b123082ea4` identifies that unchanged file only; it is not the file containing the six caller replacements. The relevant file is `src/runner/managed.rs`. Root supplied its pre-GREEN hash as `f5869878a18089865d7b1d3204b6834ce7409d993092ed136a82fc89df5041b8`; a post-validation read gives the same SHA. This confirms the caller file did not change during these test runs. `src/runner.rs` remains SHA `d8ee798b3df2a942e514b79b17eb1937cf812c7dbad01a478eae08b123082ea4`.

The initial two-case RED source hash was captured as `63ad38fe0ee04311bc44937b134fb7e449540eb07a1d670ebc9e355d1b96cda9`, but that exact source file was not retained separately before expanding it to the six-case GREEN suite. The original RED log remains preserved and records both divergent cases and the emitted kills. Final six-case source hash is `6eb3380bbda0d3afa97170331b5bbbff474b7de5049b6c4a86bbd7edaecb1379`.
