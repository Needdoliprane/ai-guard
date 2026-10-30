# Runtime snapshot-stop software artifact checks

This report records selected fixture tests and the final software build for the snapshot-bound stop port and six root caller replacements. It is not a Docker runtime qualification.

## Source identity

- `src/runtime.rs`: SHA-256 `b2a371c95892fa5c6c85e2271b2c8073325d5e7d3f292bb611dee406652f134d`.
- `src/runtime/managed.rs`: SHA-256 `6bae8e0b6c0fd3a7d38b832c8fcd556ad5bde6c587ee067c19c29b6e5a719766`.
- `src/runner/managed.rs` (six caller replacements): SHA-256 `f5869878a18089865d7b1d3204b6834ce7409d993092ed136a82fc89df5041b8`, supplied before tests and re-read after with the same value.
- Contract preparation present in the same final build: `src/control.rs` SHA-256 `122e0d27d4bd184c11e6c4af18c942089a4c9d15ae07f3f31604808d37d82458`; `src/control/supervision.rs` SHA-256 `9658a67cc8b030beb9418ec76eb71df4316dd0c8b77f938f6bf8f4737002d10a`.

## Selected test evidence

- `CARGO_INCREMENTAL=0 cargo test --locked --test runtime_stop_snapshot -- --nocapture` — PASS, 1 top-level test covering six fixture cases. Log: `/tmp/ai-guard-runtime-stop-green.UEbe3q/target-green.log`.
- `CARGO_INCREMENTAL=0 cargo test --locked --test runtime_managed` — PASS, 14/14. Log: `/tmp/ai-guard-runtime-stop-green.UEbe3q/runtime-managed-green.log`.
- `CARGO_INCREMENTAL=0 cargo test --locked --test runner_recovery --test runtime_terminal` — PASS, 16/16 and 1/1. Log: `/tmp/ai-guard-runtime-stop-green.UEbe3q/runner-recovery-terminal-green.log`.
- The fixture suite verifies the prior unsafe kill on divergent configuration/logging snapshots as RED, then exact-snapshot stop, inspection refusal, unknown Running state, divergent Created allocation, and retention on divergence as GREEN. It does not exercise a real engine.

## Final gates and artifacts

- `cargo fmt --all -- --check` — PASS.
- `cargo clippy --locked --all-targets -- -D warnings` — PASS after the DTO enum was boxed. The earlier failed clippy run and its fix are documented in `/tmp/ai-guard-supervision-contract-tests.md`.
- `cargo build --release --locked` — PASS, duration 30s. Log: `/tmp/ai-guard-supervision-final.X7FQnv/build-release.log`.
- New `target/release/agentsandbox` SHA-256: `ac13f27c613c557e86dd27e48eaa2468deb7b1f048e91e61a7be40819a69e753`.
- New `target/release/libagentsandbox.rlib` SHA-256: `3cd4c21827b812b5e33af6c67b30e3a78ef79ff58408f26f220eb06ea0754c40`.
- Existing executable was copied privately before building; prior SHA-256 `91f0cc10b0e66c0b5633f4ca87ba22e5e527c6a5bb5cb3ffd0905abd6b2ae5a9`.
- The full Cargo/source/test manifest before final checks and after build matched exactly (SHA-256 `ca364caca5be9ec8b58caef22e369f62115d7afbe28a2f86f9e2755ba4723a7d`). It is stored under `/tmp/ai-guard-supervision-final.X7FQnv/` with private permissions.

No image was built; no Docker/Podman runtime, provider, model, full suite, or ignored runtime test was run. The build and fixtures do not establish production readiness or runtime qualification.
