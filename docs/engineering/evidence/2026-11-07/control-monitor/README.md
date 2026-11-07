# Bounded post-start control monitor — 7 November 2026

Software candidate `audit-l-fix08`, based on `d12a16a`. The
[approved contract](../../../GATEWAY_TRANSIENT_FAILURE_PROPOSAL.md) was recorded
in `f9c3728`; its approval-only text is preserved in this branch. This record
establishes targeted software validation, not runtime or production qualification.
Composition with the integration branch is still pending.

## Implemented behavior

After Running and `command.started` acknowledgements, read-only observation
allows a fixed window of at most 60 seconds from the start of the first failing
request. That request's timeout counts inside the window; the original execution
deadline and an observed STOP still win. No launch, claim or mandatory start
audit gains a retry window. Only classified connection interruption, timeout or
HTTP 429 qualifies. No temporary-unavailability DTO exists here: unclassified
503, audit/storage, authentication, identity and malformed-response failures
retain the immediate targeted-stop path.

A private epoch journal records degradation and recovery with exact operation
identities, bounded at 128 KiB and 64 intervals. Its creation has a separate
15-second arming bound, sampled before starting the write and limited by the
original deadline. Completion does not wait behind an uncertain closing write;
the writer retains its intent, and cancellation is never treated as rollback.

Restart grants no fresh window. The exact locally bound instance can be stopped
before depending on a healthy gateway, without local adoption, a new start or
quota release. An explicitly requested replacement invocation follows the
existing authenticated adoption path. Terminal accounting remains dependent on
its durable evidence. The English/French broker guides and operations describe
the remaining unknown states and reconciliation steps.

## Reproductions and review

The original loop was extracted without adding retry behavior. Its transient
failure and blocked-refresh completion tests failed, with the STOP canary
passing: output `f62af4c6d26eb3e7b1a79d093f5f083d70aa6a92726b8cd3e15cd4da21b4d8c5`.
Independent reviews then found the following cases before their corrections:

| Source | Reproduced behavior | Output SHA-256 |
|---|---|---|
| L03 | Closing journal blocks completion | `335655e85bb334a6d5f76f50835c5ad12abdc7af5b0232cb78d6389bca01fd1a` |
| L03 | Real resume entry cannot reach targeted STOP while gateway stays unavailable | `78f7dfd882133877fefbbd23ac913ec96500e94fe2b6d0e1e4542525b8ebc022` |
| L04 | Higher-revision terminal DTO accepts held quota or missing observation | `4464ef9e10b95a58b4159b62e3ac6ce79a6852daa5cbeb8570b260a03abb0daa` |
| L06 | Raw journal of 131,073 bytes accepted and rewritten as 553 bytes | `5231d81ddf26008b4e67ab24cfeb824cee04cbe32ef517f41c63b656437db842` |
| L06 | Epoch creation pending beyond its intended arming bound | `21b543f4e3853717764ac0cb52a397b593f4ce7436c84ac38b8d7a6d92446ba9` |
| L07 | Healthy explicit adoption blocked before audited stop | `98b7779bf8bb14ba198924383452291b3c7121f923e2e01ade8734ab50916bd9` |

The DTO test is defensive validation of a client input. It does not demonstrate
that the current Store emits inconsistent terminal snapshots. These development
residues do not change the independent report's count of thirteen findings.

L06's first adoption test aborted with stack overflow before its oracle:
`b3d5ec27212b4fd3f97f579ea1c3e7e176961bdcd5d11ef8805d22805fe575e3`.
It is **NOT_REACHED**, not an adoption reproduction. L07 split and boxed only
the test setup/futures, preserving the application prefix byte-for-byte
(`60a2486a06e6211c9ef6945c5ea233227511af42d83b36745a592e1194f9cccb`).
It then reached the expected functional failure. No stack-size override, larger
rescue thread, weakened assertion or production timeout change was used; the
precise cause of the original test stack overflow was not otherwise measured.

## L08 validation

Frozen sources: module
`f415220dc6ebe83f44d75275bef0bb6709fc21e9bc54f497b309c34b94da9be0`, managed
`097e50a7c1f638d3bd6c247bfdf0f3ff46441211d2bf9b02a0058d810b689aa5`.
Cargo and the existing Rust engine fixture are unchanged. The full source and
document list is in [source.sha256](source.sha256).

```text
cargo test --offline --locked --lib runner::managed::control_monitor::tests:: -- --test-threads=1
cargo check --offline --locked --all-targets
```

Both commands use an explicit manifest and the same source-specific Cargo
target. Rust 1.99.1 Linux ARM64, jobs 1, incremental/debug information disabled,
empty allowlisted environment with scratch-local home/tmp/cache. No
`RUST_MIN_STACK`. Root verified the effective sandbox: read-only source,
toolchain and registry; no external network; 4 CPU, 8 GiB RAM without swap,
256 PIDs, 4 GiB scratch/65,536 inodes, 512 MiB per file, 1,024 FDs and a fixed
wall-clock deadline. Earlier W results used 2 CPU/3 GiB scratch; their timings
are not a controlled performance comparison.

| Check | Result | Output SHA-256 |
|---|---|---|
| Monitor module | PASS, 22/22, 34.30 s; build 1 min 55 s | `883d86a787f84b7906186489fac80fa2a15be173589752555194991a8ac6b8cc` |
| All-target compilation | PASS, RC0, 1 min 06 s | `01778b73f1bd631c9294d9e96d58a63095dfc512851effd0d27b6bc68c03af8a` |
| Independent final source/document review | Accepted statically after all six corrections | No test results transferred by the reviewer |
| Rustfmt and diff checks | PASS | Trusted native formatter, no host target execution |

Warnings remain: 118 library, 2 SSO-test and 3 library-test warnings in the
all-target check. Clippy and the global readiness gates are not closed. Root
read and verified the private parent-captured logs; the sandbox reported no OOM.

## Limits

Tests exercise the real HTTP/SQLite and resume paths with explicit Rust engine
fixtures and controlled state injection. They do not qualify Docker, Podman,
Kubernetes, actual process crashes, physical storage failures or an independent
lifetime guardian. A failed or unconfirmed stop is not termination.

The callback's single-attempt control-lane change comes from lot I and must be
preserved and verified during composition. I's subsequent reader optimization
is not part of these L08 sources. HTTP connection/header/idle admission,
anonymous auth/store scans, FD availability and inter-store coupling remain
open. This monitor change does not close corrective lot L as a whole, any
external audit verdict or a production gate.
