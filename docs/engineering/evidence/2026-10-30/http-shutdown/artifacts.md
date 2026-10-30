# HTTP shutdown release artifact

## Results

- `cargo fmt --all -- --check`: **PASS**, exit 0.
- `cargo build --release --locked`: **PASS**, exit 0, 19.48 s (`release` optimized profile).
- Source manifest (`Cargo.toml`, `Cargo.lock`, and 31 files under `src/`; 33 paths total): identical SHA-256 results before and after build.
- Frozen production sources matched the approved candidate before and after: `src/main.rs` `48d73be68241017ef27ce36fd52a4ede72920f5f61ea0c7310fce4cd196893c1`, `src/api.rs` `0d61502c45d2b25dec3c135b71495d11e024ec47b414525c48dea637ceaeceb8`, `src/api/auth.rs` `f1b1b30222ecbfb1220be5dc82191e03bf560cc4bf5e528e30033641a747ceee`.
- The pre-build release binary matched the preserved baseline: SHA-256 `b7565088f8e10c05d24b5cc3b28c33bc993760b35cc3479322af9d63d72ec191`. The prior copy remains at `/tmp/ai-guard-http-shutdown-baseline/agentsandbox-auth-b756` (mode 600).
- Newly built `target/release/agentsandbox` SHA-256: `91f0cc10b0e66c0b5633f4ca87ba22e5e527c6a5bb5cb3ffd0905abd6b2ae5a9`. This is the observed identity of this new binary; no new hash was predicted.

## Scope and evidence

Exactly one final format check and one release build were run. No tests, Clippy, Docker/runtime, provider, or model calls were run for this artifact step. The focused HTTP shutdown test and regression results remain in `/tmp/ai-guard-http-shutdown-tests.md` and were not rerun here. This artifact build does not qualify global release readiness.

Private evidence is in `/tmp/ai-guard-http-shutdown-artifacts.7E2bfN/` (directory mode 700; evidence files mode 600): original command logs and exit codes, 33-path source manifest before/after, and old/new binary hashes and metadata. No `.env` or credentials were read.
