# Supervision API tests — initial RED

Worktree: `/Users/louis/Documents/ai-guard/.worktrees/broker-supervision`  
Branch: `codex/broker-supervision` (base `e2cda5eb889b9fc8f8053085ccbaf6942792be67`)  
Scope: new test file `tests/supervision_api.rs` only. No product source, docs, Cargo files, commit, or push changed by this validation.

## Evidence

- Command: `cargo test --locked --test supervision_api`
- Result: test target compiled; 3 top-level tests ran; 0 passed, 3 failed; exit 101; runtime 0.13 s.
- Failure classification: expected feature-absent RED, not a security regression. Protocol GET returned 404 for all tested tenant/role/schema cases. The supervised reservation POST returned 405; the other six new write routes returned 404. The auth-before-body checks therefore could not exercise authorization on absent routes (they reported the missing route statuses and no body polling). The end-to-end supervised test stopped at the reservation route’s expected 201 vs observed 405.
- Preserved original output and command context, mode 600: `/tmp/ai-guard-supervision-api-red.dqtr1K/test.log` and `context.txt`.
- Before/after hashes are identical:
  - `Cargo.toml`: `55f7ee3de152374b42dd219ec98e004905804879b2677a641e3af434309ea13a`
  - `Cargo.lock`: `8f6a66b3ea75d5b26beb2365d78dec5e34479d19863fd29c22b4b37450f66d31`
  - `src/api.rs`: `4db5d205e0571a5843e6b92edda61c727c3c14f296d6bbba3631f3d71116e393`
  - `tests/supervision_api.rs`: `f03630aeac2e6b07aac5288a3b2c85395eadc76d60d279be3b15d1bb38b02fa1`

The test file uses a real Axum router and temporary per-tenant SQLite stores; its runtime observations are explicit fixtures only. It does not qualify Docker/Podman execution or broker supervision. Existing `/api` legacy alias comparisons are in the test but the first test’s aggregate assertion fails on the absent new routes before that behavior can count as a passing result.

## GREEN after implementation

After route implementation was frozen, the combined targeted command `cargo test --locked --test broker_clock --test supervision_api` completed with `supervision_api`: **3 passed, 0 failed, 0 ignored**. This includes authenticated tenant/role protocol reads, schema2 inactive state, body-polling authorization checks, legacy alias comparison, the SQLite-backed admission/helper/worker claim flow, and reopen maintenance refusals. A test-only baseline correction moved the no-append head capture after the fixture itself records a decision; no production source changed during that correction.

Final combined log and source manifests (mode 600): `/tmp/ai-guard-supervision-clock-final2.94temq/test.log` and `source-hashes-{before,after}.txt`. Final test file hash: `3344d9b43be1af08abaa5164cec709ba9ef8bcee8abf972b69a6afd094fc6116`. API implementation hashes match the frozen core report: `src/api.rs` `854e654f153f70eee1c9ee8532dab85bd4012aae8bae541f40a5d3a98e40b893`; `src/api/supervision.rs` `295cc728975d18c55e5f053908bc05b50471b4abfafc6f9cb8db25722d4d81b2`.

## Jev non-call proof and API canaries

Added one local transport-count test using the real `JevClient` with a loopback HTTPS URL, a QA-only dummy key, explicit model alias, and `allow_local_http=false`. The local `TcpListener` supplies no TLS/model response and only counts accepted connections. The test first observes one intentional client connection, resets the counter, then confirms zero new connections for (a) exact replay of a previously admitted operation after opening the same store in maintenance mode and (b) a fresh admission refused with `503 supervision_inactive`. No external network, TLS bypass, model endpoint, or real key is used.

- `cargo test --locked --test supervision_api`: **4 passed, 0 failed, 0 ignored**. Log: `/tmp/ai-guard-supervision-jev-test.JcKWiz/test.log`; before/after manifest: `/tmp/ai-guard-supervision-jev-test.JcKWiz/source-hashes-{before,after}.txt`.
- After tightening the namespace equality/distinctness oracle, the same API target was rerun and again passed **4/4** against the current test source hash `41ed177f0c7d851d80a51767e62cb494659f3ec9bb6d4628fe8438627db44f58`. Qualifying log and exact before/after manifest: `/tmp/ai-guard-supervision-api-jev-oracle.m5ba3n/test.log` and `source-hashes-{before,after}.txt`. The delta from the earlier `d5fe386f...` test source was test-only: require three schema3 namespace observations, assert viewer/runner same-tenant equality, tenant A/B inequality, and each UUID non-nil; then rustfmt. No product source changed.
- Targeted API regressions: `cargo test --locked --test api_auth_boundary --test tenant_api --test workers_api --test gateway_tenancy --test api_security`: **20 passed, 0 failed, 0 ignored** (6 + 3 + 6 + 1 + 4 respectively). Log: `/tmp/ai-guard-supervision-api-canaries.UrDIs8/test.log`; identical source manifests: `/tmp/ai-guard-supervision-api-canaries.UrDIs8/source-hashes-{before,after}.txt`.
- Final namespace oracle verifies all three schema3 observations, same tenant viewer/runner namespace equality, different tenant namespace inequality, and non-nil UUIDs.
- `cargo fmt --all -- --check`: PASS after final test edit.
- `cargo clippy --locked --all-targets -- -D warnings`: PASS after final test edit.
- Release build: **PASS** (`cargo build --release --locked`, exit 0; final log in `/tmp/ai-guard-supervision-api-checks-final.gqrSOo/release.log`).

Current test file SHA: `41ed177f0c7d851d80a51767e62cb494659f3ec9bb6d4628fe8438627db44f58`. Production API sources remained at the frozen hashes recorded above through these targeted test runs.

Release build completed: `cargo build --release --locked` exited 0. Final validation logs, contexts, source manifest and binary hash are in `/tmp/ai-guard-supervision-api-checks-final.gqrSOo/`. Release executable identity: `target/release/agentsandbox` SHA-256 `d707f22344c1fea43bcbb90eb64cd3a4cc516903f10efecee35f500b3a7faf31`. The final fmt and strict Clippy checks both exited 0 after the namespace assertion update. The one earlier build-launch attempt did not start cargo because the log-path helper command was unavailable; it is not counted as a build attempt.
