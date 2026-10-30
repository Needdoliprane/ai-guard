# Provider final artifact validation

Scope: lint and release rebuild only; no tests, Docker, E2E, or external API calls.

Private logs: `/tmp/ai-guard-provider-artifacts.uTpyll` (directory mode 700, files mode 600).

Source manifest: 89 files before and after (see `source-before.sha256`, `source-after.sha256`). The latest Jev handoff guard is present at `src/runner.rs:923`; the earlier managed NoLogs admission guard is at `src/control.rs:436`. The pre-Jev-guard binary is preserved separately.

## fmt
Command: cargo fmt --all -- --check
Exit: 0; duration: 1s; log: /tmp/ai-guard-provider-artifacts.uTpyll/fmt.log

Status: PASS

## clippy
Command: cargo clippy --locked --all-targets -- -D warnings
Exit: 0; duration: 6s; log: /tmp/ai-guard-provider-artifacts.uTpyll/clippy.log

Status: PASS

## build
Command: cargo build --release --locked
Exit: 0; duration: 31s; log: /tmp/ai-guard-provider-artifacts.uTpyll/build.log

Status: PASS

Source manifest comparison exit: 0 (0 means unchanged).

Frozen and rebuilt binary hashes after build:
bb48545a6c7f6af0ab9b2b1e4b509927682ae6354d7372a979df1135d4058047  /tmp/ai-guard-managed-claude-api-third.yhQOet/frozen-binary/agentsandbox-bb48545a6c7f6af0
ce8e94ff4ba6437799101bf8b093b7737fb8c89ff778591b35a4f20bba655917  target/release/agentsandbox

Source manifests compared: 89; compare exit 0.
Frozen binary SHA verified unchanged from before: bb48545a6c7f6af0ab9b2b1e4b509927682ae6354d7372a979df1135d4058047
Tests: NOT_RUN (explicitly excluded). Docker, E2E, model/API: NOT_RUN.

## Artifact hashes and source identity

- Preserved pre-guard binary: `bb48545a6c7f6af0ab9b2b1e4b509927682ae6354d7372a979df1135d4058047`; its SHA stayed unchanged through the release build.
- Rebuilt `target/release/agentsandbox`: `ce8e94ff4ba6437799101bf8b093b7737fb8c89ff778591b35a4f20bba655917` (new artifact identity recorded; no expected binary SHA had been specified).
- `d8ee798b3df2a942e514b79b17eb1937cf812c7dbad01a478eae08b123082ea4` is the SHA-256 of `src/runner.rs`, the source file containing the guard. It matches the before and after source manifests and is not a binary hash.

The previous draft incorrectly compared the `src/runner.rs` source hash with the rebuilt executable. That comparison was invalid and is withdrawn. The commands `cargo fmt --all -- --check`, `cargo clippy --locked --all-targets -- -D warnings`, and `cargo build --release --locked` all remain PASS; no tests were run in this validation.
