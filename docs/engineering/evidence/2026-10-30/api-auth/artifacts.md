# API auth artifact build

## Result

- `cargo build --release --locked`: **PASS**, exit 0, 29.85 s (`release` optimized profile).
- Source manifest: `Cargo.toml`, `Cargo.lock`, and 31 paths under `src/` (33 entries total) had identical SHA-256 manifests before and after the build. No source drift was observed.
- Rebuilt `target/release/agentsandbox` SHA-256: `b7565088f8e10c05d24b5cc3b28c33bc993760b35cc3479322af9d63d72ec191`.
- Preserved pre-build binary SHA-256: `ce8e94ff4ba6437799101bf8b093b7737fb8c89ff778591b35a4f20bba655917`; the frozen copy matches the recorded pre-build hash.

## Scope

This was one release build only. No tests, formatting, clippy, Docker/runtime, provider, or model calls were run for this artifact task. The already reported API auth boundary and focused regression results remain in `/tmp/ai-guard-api-auth-boundary-tests.md`; they were not rerun here. This build establishes compilation of the current release artifact, not production qualification.

Private evidence directory: `/tmp/ai-guard-api-auth-artifacts.Q3s1Bh/` (directory mode 700; logs and manifests mode 600), including the original build log, before/after source hashes, and binary hashes. No `.env` or credentials were read.
