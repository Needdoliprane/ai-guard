# Prepared supervision contract tests

Status: **RED for two confirmed validation defects** on the frozen DTO candidate. No Store schema, API route, default activation, runtime, provider, or model work was performed in this test task.

## Scope and frozen source

- New test file only: `tests/supervision_contract.rs`, SHA-256 `93c16d04077ca7f1a649a9aeb5f8f2c2617a00d10ad2974941320f183d20fd30`.
- Frozen implementation hashes before and after RED: `src/control.rs` `122e0d27d4bd184c11e6c4af18c942089a4c9d15ae07f3f31604808d37d82458`; `src/control/supervision.rs` `ee928524b1ddee65f65b35b583c8b3502dfb0ff74db081d83587e6ed39b0a697`.
- This is a pure DTO/factory fixture suite; no database or engine is used.

## Command and result

`CARGO_INCREMENTAL=0 cargo test --locked --test supervision_contract -- --nocapture`

Exit 101: 5 top-level tests executed, 3 passed and 2 failed on the intended regressions:

1. `clock_counter_and_boot_identity_are_canonical` — a nil UUID is accepted as `HostClockSampleV1.boot_id`; the test requires rejection.
2. `armed_claim_must_match_arm_revision_and_receipt_input_hash` — a claim whose `expected_revision` differs from the arm is accepted by `validate_for_arm`; the test requires rejection. A matching revision and a forged receipt input hash are also checked.

The other 3 tests passed: canonical `u128` decimal boundaries and deadline overflow/method/boot/expiration checks; strict DTO unknown-field rejection and protocol wire name; fixed tar helper policies for all four purposes, independent policy hashes, no credentials, V2 preset under a V3 parent, and rejection of a worker-hash substitution in the runtime inspection.

Exact output is in `/tmp/ai-guard-supervision-contract-red-final.GLsPVo/target-red.log` (private mode 600). An earlier attempt exposed an invalid TTY parent fixture; that fixture was corrected before this final RED run. It was a test-data issue, not a production finding.

## Limits

Legacy hash/golden tests, Store schema3/replay/migration, API behavior, runtime execution, and the full suite were not run in this pass. No existing contract source or test was modified.

## Corrected GREEN and final software gates

Core changed only `ArmSubjectV1::Helper.permission` to `Box<HelperPermissionV1>` to satisfy clippy's large-enum warning. Serde representation remains the same. Frozen post-fix hashes: `src/control.rs` `122e0d27d4bd184c11e6c4af18c942089a4c9d15ae07f3f31604808d37d82458`; `src/control/supervision.rs` `9658a67cc8b030beb9418ec76eb71df4316dd0c8b77f938f6bf8f4737002d10a`.

- `CARGO_INCREMENTAL=0 cargo test --locked --test supervision_contract` — PASS, 5/5 after the boxed-field correction. Log: `/tmp/ai-guard-supervision-final.X7FQnv/contracts-green-boxed.log` (mode 600).
- Earlier canary command `CARGO_INCREMENTAL=0 cargo test --locked --test supervision_contract --test worker_registry -- --nocapture` — PASS, 5/5 and 36/36, including historical fixed-sandbox-v1 admission bytes/hash golden. Log: `/tmp/ai-guard-supervision-contract-green.geiQtY/target-green.log` (mode 600). Not repeated after the box-only correction; legacy source/tests and serde wire shape were unchanged.
- `cargo fmt --all -- --check` — PASS, 0s; final rerun log `/tmp/ai-guard-supervision-final.X7FQnv/fmt-final.log`.
- `cargo clippy --locked --all-targets -- -D warnings` — first run FAIL, 5s, due `large_enum_variant` at `ArmSubjectV1`; original diagnostic retained at `/tmp/ai-guard-supervision-final.X7FQnv/clippy.log`. After the box-only correction the same command PASS, 8s; log `/tmp/ai-guard-supervision-final.X7FQnv/clippy-final.log`.
- `cargo build --release --locked` — PASS, 30s. Log `/tmp/ai-guard-supervision-final.X7FQnv/build-release.log`.

The source/Cargo/test manifest before final checks and after release build is byte-identical (both SHA-256 `ca364caca5be9ec8b58caef22e369f62115d7afbe28a2f86f9e2755ba4723a7d`); manifest files are private in the same evidence directory. Existing executable was preserved before the authorized build: prior `target/release/agentsandbox` SHA `91f0cc10b0e66c0b5633f4ca87ba22e5e527c6a5bb5cb3ffd0905abd6b2ae5a9`; new executable SHA `ac13f27c613c557e86dd27e48eaa2468deb7b1f048e91e61a7be40819a69e753`; new `libagentsandbox.rlib` SHA `3cd4c21827b812b5e33af6c67b30e3a78ef79ff58408f26f220eb06ea0754c40`.

These checks validate software compilation and the listed contract/regression fixtures. They do not implement or qualify the Store schema3 transactions, API routes, broker supervisor, helper runtime, TTY guardian, or release readiness.
