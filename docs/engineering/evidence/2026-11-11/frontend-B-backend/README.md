# Supervision backend validation

The candidate composes the forty paths of lot B on public commit `e95724a0cedbf1b2cbfed6c1e7527c4a0cb57807`. Its 2,440 regular files have source manifest `b4ff38fe21732def570deada5745a6c2dc11c1dfced968ada9176f09a126d232`. Both setup guide indexes retain the signed checkpoints and unified supervision entries. The 61 frontend sources match the earlier [Node evidence](../../2026-11-10/frontend-B-node/README.md).

| Check | Observed result |
|---|---|
| `cargo test --frozen --offline --test human_recent_events --test supervision_stack_api --test human_access --no-run` | RC0 on Linux ARM64, 10 November 23:54:22–23:57:33 UTC |
| Direct test lists | RC0 for each of the three actual binaries |
| Exact test cases | 15 passed, 0 failed, 0 ignored, 10 November 23:59:44 to 11 November 00:01:29 UTC |
| `cargo build --frozen --offline --example supervision_stack` | RC0. The AArch64 example was exported and verified at SHA-256 `cac15b1adb59aa9e064dbf8c74d3b375e8c1d9aa87d97a4d38f2b07f10d46b8e` |
| Chromium preflight, initial profile | First run stopped at Playwright module resolution. With the module path corrected and the same security profile, Chromium started and its zygote stopped on the `sys_chroot` check. RC2. A blank page could not open. These failures remain retained. |
| Chromium preflight, reviewed QA variant | RC0 with `chromiumSandbox:true`; a blank page opened. Node26.13.0, Playwright1.64.1, Chrome156.0.8078.80. |
| Browser application scenarios | `journal`, `focus`, `resume`, `rights`: four distinct RC0/PASS results, no page JavaScript error. The complete job ended RC0 at 11 November 10:49:46 UTC. |

The 15 Rust cases use the actual Rust routes and SQLite fixtures. They cover the newest authorized events, pagination, resume positions, expired and revoked credentials, capability changes, exact proxy paths and tenant isolation. The registry case uses 100 synthetic worker records. Real worker load and runtime qualification remain open.

The Rust job used the pinned Linux ARM64 image, private scratch and target, UID1000, no network, a read-only root, zero capabilities and no-new-privileges. Source and five pins verified after the run. Its 2,440-file source export has SHA-256 `4f6ae4159b6238c501ea97782ea898c062c4daab3ecf8632f9d9c76b096ff9ad`. The first example export met the host file-size cap. The second matched the measured length and the in-container hash. The Rust and browser QA containers were removed after recording results. Raw results and binary exports remain in the private engineering archive.

## Browser scope and QA profile change

The four scenarios start the actual Rust supervision example with synthetic
SQLite data and the matching built frontend. Journal covers newest-first
pagination, bounded dense rendering and live resumption without replay. Focus
covers keyboard continuity, announcements and recovery after HTTP errors
injected at the browser boundary. Resume drives browser time and visibility,
requires confirmation and discards late responses. Rights changes are applied
by the real control and verify removal of forbidden views, details and reads,
including after credential revocation. This is not a real 300-second control
expiry test. The 24/120 worker counts are registry fixtures, not running workers.

The reviewed seccomp variant has SHA-256
`b44b83f4eb9655ed3f3201c670b72ed518dfef17730cdd7aa11325e17aececbd`.
It preserves the original 27 rules/default-deny action and adds an unconditional
`chroot` allow rule for **all QA processes**. It is not the unchanged original
profile or a rule restricted to Chromium. The browser QA retains UID1000,
zero outer capabilities, no-new-privileges, no network, read-only root and two
read-only inputs, private IPC/cgroup, CPU1, 2GiB RAM with no swap, PID256,
file-size256MiB, nofile1024 and scratch3GiB/65536inodes. Kernel checks passed
before browser execution. A successful requested-sandbox launch is a
compatibility observation, not an independent exhaustive inspection of the
browser's internal namespaces/filters or a worker-isolation qualification.

The mounted Rust binary matches
`cac15b1adb59aa9e064dbf8c74d3b375e8c1d9aa87d97a4d38f2b07f10d46b8e`
before each scenario. The harness and built assets match the frozen inputs.
Twelve fixed journal screenshots at 1440/390/320px in light/dark themes were
exported before stopping, then independently parsed as regular PNG entries,
checked against the manifest and bounded to 2MiB/file and 24MiB total. Their
archive is `34c432bbfd03b289f92a1086d29dc0207a416fc2ada9ce12f88575cacd34bcac`.
Root visually inspected the 1440px light journal and 320px dark list. This is
not a complete visual/accessibility audit. Peak QA cgroup memory was
1,082,839,040bytes, with zero OOM events. The exact QA container was stopped,
inspected PID0/OOMKilled=false, removed and verified absent.

Independent source and timer reviews preceded these executions; a separate
evidence review accepted the four targeted browser results and their limits.
All forty indexed integration postimages match the tested source manifest.
The existing independent `docs/api.md` working changes were composed with the
thirteen-line addition, while only the public addition entered the index.
Other working changes remain preserved. The source slice is published; real
workers, providers, IdP, remote federation, Podman/Kubernetes, capacity100 and
release qualification retain their own open gates. See the
[delivery register](../../../CLAUDE_DELIVERIES_20261110.md).
