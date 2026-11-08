# Relay software composition — 8 November 2026

Frozen source76 is Root `f5c3f879ab099503ff8ee2b9fe670eeab6da7f6e`:
durable admission, SDK tests, private IO owner and sealed runtime transport
are compiled and tested together. The [selected manifest](source.sha256)
covers their fourteen Rust files and Cargo entries. Later documentation
changes do not change this code. The complete frozen manifest has SHA-256
`7a296d1806cece16ce7fa772f6c75bae988ce4d86e264e85b45b2fc043dd4775`.

This is a verified software composition, not the broker runtime handoff.
Production admission factories remain closed. The separate handoff worktree
and D5 candidate receive no qualification from these results.

| Selection, one target per frozen input | Observed result | Parent log SHA-256 |
|---|---|---|
| `cargo check --all-targets --locked --offline` | RC0; 82.99s | `944063545b68b42e60b20b5caad5aa6c997aa6422d2b020e52168f2349463f99` |
| `relay::channel::control::tests::` | 12 PASS; 0.58s test; RC0 | `5165951efd66f7a445701ab11da2c4221579b1b67385395a89dcab1677e2218d` |
| `runtime::managed::relay::tests` | 7 PASS, one child ignored; 27.51s test; RC0 | `211bd740ec4f70dc89fb1b3a018c30b3446d3f0ece351d27aabd55df8868bfe9` |
| `runner::relay_admission::tests::` | 3 PASS; 6.46s test; RC0 | `547dcb7bc5db94a32e94b5236f07c1fa4fffd9af4aaa5aaa4d45000825e84685` |
| `worker_relay_api`, `relay_admission_cases` | 3 PASS; 0.96s test; RC0 | `b15c51a47c37bcb05539fc0734ee23cad255b6b09264dffe4d9d8b1e4ea1eb9c` |

There are 25 distinct functions executed on this composition. They are not
25 new tests or additional historical unique coverage. Filtered tests and the
ignored child are not PASS. Unit selections used `cargo test --lib`; the final
selection used `cargo test --test worker_relay_api`. All used
`--locked --offline` and `-- --test-threads=1 --nocapture`, issued by Luna.
Existing warnings remain; strict Clippy and a complete suite were not run.

The component evidence records actual socket/file IO, HTTP/SQLite transactions,
claim replay and revocation, real Rust child pipes and descriptor closure:
[owner](../../2026-11-07/relay-control-owner/README.md),
[runtime](../../2026-11-07/relay-runtime-exec/README.md),
[admission](../../2026-11-07/relay-admission/README.md),
[SDK](../relay-admission-sdk/README.md). Runtime facts remain synthetic, and
the owner's declared HTTP peer is distinguished from the SDK's real gateway.

QA AP used Linux ARM64, Rust1.99.1 and pinned image
`sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`.
UID1000, capabilities removed, NNP/seccomp, private loopback with network none,
readonly root/input/toolchain, 4CPU/8GiB memory/256PIDs and bounded4GiB scratch
with65536inodes were inspected. Cargo used a private writable copy of the
public registry, with its one missing Linux archive checked against Cargo.lock.
All compilation/tests stayed offline inside QA, with a source-specific target.
Postflight reported zero OOM events. The final trusted control inspection has
SHA-256 `926652c48e79add1e92d029b905712e6458c57a06abac189e166d0219372da9c`.
Root closed this QA after observing only its sleep process. Its exit state is
separate from test RC0 and does not establish a worker terminal.

Export77 was a different frozen input and failed with stack overflow; it is
not hidden by the successful composition. Audit K, cold D5 recovery, mandatory
audit retention, original-deadline/RoleLease integration and real engine
qualification remain open. No Docker/Podman/Kubernetes worker campaign,
provider call, performance qualification or global gate is credited here.
