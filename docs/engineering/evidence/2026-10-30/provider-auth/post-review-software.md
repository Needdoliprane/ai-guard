# Provider post-review software validation

Scope: final reduced software checks after PRV1/PRV2/PRV3; runtime/provider qualification excluded.

Logs (private mode 600): `/tmp/ai-guard-provider-post-review-logs`. Final source hash manifest: 22 selected files; pre-check manifest capture failed, as detailed below.

## fmt
Command: cargo fmt --all -- --check
Exit: 0; duration: 1s; log: /tmp/ai-guard-provider-post-review-logs/fmt.log

Status: PASS

## clippy
Command: cargo clippy --locked --all-targets -- -D warnings
Exit: 0; duration: 6s; log: /tmp/ai-guard-provider-post-review-logs/clippy.log

Status: PASS

## tests
Command: env CARGO_INCREMENTAL=0 cargo test --locked --test provider_authorization --test provider_config --test provider_bootstrap --test provider_runner
Exit: 0; duration: 3s; log: /tmp/ai-guard-provider-post-review-logs/tests.log

Status: PASS

## build
Command: cargo build --release --locked
Exit: 0; duration: 36s; log: /tmp/ai-guard-provider-post-review-logs/build.log

Status: PASS


Overall: all applicable reduced software checks passed. Docker, provider/model calls, and runtime qualification NOT_RUN.

## Targeted provider test accounting

`provider_authorization`: 6 passed, 0 failed, 0 ignored. `provider_config`: 9 passed, 0 failed, 0 ignored. `provider_bootstrap`: 6 passed, 0 failed, 1 ignored. `provider_runner`: 4 passed, 0 failed, 0 ignored. Total: **25 passed, 0 failed, 1 ignored (NOT_RUN)**. The ignored fixture test is reported by `provider_bootstrap`; no provider or model call occurred.

Final source SHA-256 values for the 22 selected code/config/test files are recorded in `/tmp/ai-guard-provider-post-review-logs/source-after.sha256`. The initial pre-check hash capture command had a zsh scalar expansion error and left an empty manifest, so no valid before/after hash comparison is claimed. The root had declared the 3-fix snapshot frozen; this agent made no source edits. All command logs and this report are private.
