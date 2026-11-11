# Supervision backend validation

The candidate composes the forty paths of lot B on public commit `e95724a0cedbf1b2cbfed6c1e7527c4a0cb57807`. Its 2,440 regular files have source manifest `b4ff38fe21732def570deada5745a6c2dc11c1dfced968ada9176f09a126d232`. Both setup guide indexes retain the signed checkpoints and unified supervision entries. The 61 frontend sources match the earlier [Node evidence](../../2026-11-10/frontend-B-node/README.md).

| Check | Observed result |
|---|---|
| `cargo test --frozen --offline --test human_recent_events --test supervision_stack_api --test human_access --no-run` | RC0 on Linux ARM64, 10 November 23:54:22–23:57:33 UTC |
| Direct test lists | RC0 for each of the three actual binaries |
| Exact test cases | 15 passed, 0 failed, 0 ignored, 10 November 23:59:44 to 11 November 00:01:29 UTC |
| `cargo build --frozen --offline --example supervision_stack` | RC0. The AArch64 example was exported and verified at SHA-256 `cac15b1adb59aa9e064dbf8c74d3b375e8c1d9aa87d97a4d38f2b07f10d46b8e` |
| Chromium preflight | First run stopped at Playwright module resolution. With the module path corrected and the same security profile, Chromium started and its zygote stopped on the `sys_chroot` check. RC2. A blank page could not open. |
| Browser application scenarios | NOT_RUN. The existing isolated profile must support Chromium sandbox startup before the four scenarios can execute. |

The 15 Rust cases use the actual Rust routes and SQLite fixtures. They cover the newest authorized events, pagination, resume positions, expired and revoked credentials, capability changes, exact proxy paths and tenant isolation. The registry case uses 100 synthetic worker records. Real worker load and runtime qualification remain open.

The Rust job used the pinned Linux ARM64 image, private scratch and target, UID1000, no network, a read-only root, zero capabilities and no-new-privileges. Source and five pins verified after the run. Its 2,440-file source export has SHA-256 `4f6ae4159b6238c501ea97782ea898c062c4daab3ecf8632f9d9c76b096ff9ad`. The first example export met the host file-size cap. The second matched the measured length and the in-container hash. The Rust and browser QA containers were removed after recording results. Raw results and binary exports remain in the private engineering archive.

The candidate is awaiting browser validation and integration. See the [delivery register](../../../CLAUDE_DELIVERIES_20261110.md).
