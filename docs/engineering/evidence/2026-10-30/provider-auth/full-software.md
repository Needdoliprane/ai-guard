# Provider contract final software verification

Snapshot manifest: `/tmp/ai-guard-provider-review.fL91yK/candidate.sha256` (all 20 listed files verified OK before checks).

Logs in private directory: `/tmp/ai-guard-provider-final-logs`.

## fmt
Command: cargo fmt --all -- --check
Exit: 0; duration: 0s; log: /tmp/ai-guard-provider-final-logs/fmt.log

Status: PASS

## clippy
Command: cargo clippy --locked --all-targets -- -D warnings
Exit: 0; duration: 7s; log: /tmp/ai-guard-provider-final-logs/clippy.log

Status: PASS

## test
Command: env CARGO_INCREMENTAL=0 cargo test --locked
Exit: 0; duration: 180s; log: /tmp/ai-guard-provider-final-logs/test.log

Status: PASS

## build
Command: cargo build --release --locked
Exit: 0; duration: 30s; log: /tmp/ai-guard-provider-final-logs/build.log

Status: PASS

Overall: all four checks PASS. Ignored tests remain NOT_RUN where Cargo reports them ignored; no runtime or external provider qualification performed.

## Test accounting

Cargo test summary accounting (harness summaries, without re-counting filtered child-process fixture runs): **388 passed, 0 failed, 8 ignored**. Unit targets: `src/lib.rs` 83 passed / 1 ignored; `src/main.rs` 9 passed; provider binary and doc tests 0 tests. Ignored remain NOT_RUN: bounded probe internal fixture (1), runtime confinement (1), runtime interactive (4), runtime smoke real-engine case (1), runtime terminal persistence (1). No Docker/Podman or provider/model execution was performed.

Integration target results:

| Target | Passed | Ignored | Failed |
|---|---:|---:|---:|

| `api_security.rs` | 4 | 0 | 0 |
| `audit_integrity.rs` | 12 | 0 | 0 |
| `audit_load.rs` | 1 | 0 | 0 |
| `audit_security.rs` | 3 | 0 | 0 |
| `control_cli.rs` | 2 | 0 | 0 |
| `control_fs.rs` | 15 | 0 | 0 |
| `control_migration.rs` | 11 | 0 | 0 |
| `credential_handoff.rs` | 7 | 0 | 0 |
| `decision_event_authority.rs` | 3 | 0 | 0 |
| `decision_execution_mode.rs` | 3 | 0 | 0 |
| `decision_idempotence.rs` | 9 | 0 | 0 |
| `declared_agents.rs` | 12 | 0 | 0 |
| `gateway_tenancy.rs` | 1 | 0 | 0 |
| `ingress.rs` | 6 | 0 | 0 |
| `ingress_cli.rs` | 2 | 0 | 0 |
| `jev_contract.rs` | 2 | 0 | 0 |
| `jev_credentials.rs` | 14 | 0 | 0 |
| `provider_authorization.rs` | 6 | 0 | 0 |
| `provider_bootstrap.rs` | 5 | 0 | 0 |
| `provider_config.rs` | 9 | 0 | 0 |
| `provider_runner.rs` | 4 | 0 | 0 |
| `redaction_boundaries.rs` | 11 | 0 | 0 |
| `review_regressions.rs` | 5 | 0 | 0 |
| `runner_recovery.rs` | 16 | 0 | 0 |
| `runner_terminal_audit.rs` | 1 | 0 | 0 |
| `runtime_authorization.rs` | 8 | 0 | 0 |
| `runtime_confinement.rs` | 3 | 1 | 0 |
| `runtime_interactive.rs` | 2 | 4 | 0 |
| `runtime_managed.rs` | 14 | 0 | 0 |
| `runtime_operations.rs` | 10 | 0 | 0 |
| `runtime_program.rs` | 9 | 0 | 0 |
| `runtime_races.rs` | 3 | 0 | 0 |
| `runtime_retention.rs` | 1 | 0 | 0 |
| `runtime_security.rs` | 12 | 0 | 0 |
| `runtime_smoke.rs` | 1 | 1 | 0 |
| `runtime_terminal.rs` | 1 | 0 | 0 |
| `runtime_terminal_persistence.rs` | 0 | 1 | 0 |
| `runtime_uncertainty.rs` | 1 | 0 | 0 |
| `storage_security.rs` | 17 | 0 | 0 |
| `tenant_api.rs` | 3 | 0 | 0 |
| `tenant_storage.rs` | 3 | 0 | 0 |
| `usage_ledger.rs` | 2 | 0 | 0 |
| `worker_registry.rs` | 36 | 0 | 0 |
| `workers_api.rs` | 6 | 0 | 0 |

Snapshot manifest rechecked after all commands; all 20 entries remained `OK` (see `snapshot-after.txt`). No Docker/Podman or provider/model execution was performed; runtime qualification remains open.
