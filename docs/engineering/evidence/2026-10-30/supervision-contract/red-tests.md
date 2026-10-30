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
