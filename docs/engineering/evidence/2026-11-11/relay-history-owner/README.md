# Relay full-prefix history and owner retirement — local Full3 composition

The composed correction has independent source review, two reproduced baseline defects and nineteen distinct passing regression parents. It remains part of the unpublished Full3 source, not a qualified public release. Public Core/API/SDK commit 300cb4a alone does not contain the whole broker owner.

## Behavior and review

The journal validates the entire chained V1/V2 prefix, preserves legacy bytes and immutable operation identities, and retains exact historical resolutions without constructing execution authority. Explicit retirement Begin fences ordinary writes. Finalization requires complete compatible evidence and joined local IO. A never-admitted local owner has a separate strict proof that admission was never prepared; it does not fabricate a Core acknowledgement.

The consumer preserves pending/unknown status across cancelled IO and lost cleanup ACKs, rereads through its initialized IO owner, and publishes completion only after durable finalization. The cold-read guard starts fenced. A historical claim or incomplete read keeps that fence; only a complete validated prefix proves no claim. Future-CAS histories that cannot be resolved by the current Core remain Unknown. Universal convergence is not claimed.

Two implementation worktrees used local seed 7cf3aa0, parent public 300cb4a, preserving 126 pending Full3 paths. The independently reviewed interface contract and eighteen exact source postimages are retained privately. A final test-only overlay corrects a counter to observe the actual local HTTP fixture, including identity lookups; it changes no production code. Root composed the tested postimages into local Full3 and verified all 134 protected resulting paths. No whole seed or development branch is published by this evidence.

## Observed validation

The combined 2,522-file baseline has two exact RC101 results after positive V1-reopen and actual Store/HTTP/SDK controls. These defects were reproduced before implementation. A successful baseline experiment is not a product pass. See the [recovery register](../../../RELAY_RECOVERY_REVIEW_20261111.md).

On 11 November 2026, one offline lib-test build returns RC0. Nineteen unique parents, selected from the actual built binary, each report one PASS, zero FAIL and zero ignored, from 15:02:09 to 15:02:21 UTC. The two baseline parents run first. The remaining parents cover prefix identity, proof, envelope, version and budget invariants; actual Store/HTTP historical resolution; cancelled intent and prefix reads; local IO admission; and durable finalization. Exact names and times are in [TESTS.json](TESTS.json). The all-targets Cargo check returns RC0. Independent final review finds no source blocker in this composition. Root separately verifies the raw oracles, returned codes and source payloads.

| Artifact | SHA-256 |
|---|---|
| Frozen source: 2,527 regular files, 28,299,264 bytes | de4c94cc3a7a169c324a4e2b774fc6af2dfe8bb1258a81451725439d77cd3db5 |
| Input manifest | 27df9744e5402b0d02b93708ce80cd9997e04a689ab039864a7a7b658cb49523 |
| Actual live source exported before stop: 28,467,200 bytes | b898b80b816f3e87882117c41d53a12f1d1b57e43461db5e390a225db8caebca |

All 2,527 paths and payload hashes match. The eighteen selected postimages are in [SOURCE.sha256](SOURCE.sha256). Input and post-run tar hashes differ because their headers and directory entries differ. Payload equality was verified independently.

Pipeline, source verification, bounded export, closure and final result each return RC0. The exact container is removed and its exact-ID inventory is empty. The QA cgroup peak is 7,069,716,480 bytes, with zero oom/oom_kill/max events and OOMKilled=false. This is compilation/QA memory, not worker consumption. The sleep holder exits 137 during controlled stop; target build, tests and check had already completed successfully.

## Commands and execution boundary

All target commands run under Linux ARM64/Rust 1.99.1 in a pinned local QA image: UID 1000, no external network, read-only root filesystem, dropped capabilities, no-new-privileges and seccomp filtering. Limits are four CPUs, 9 GiB RAM with container swap disabled, 128 PIDs, 1,024 FDs, 512 MiB per file and zero core files. Scratch is bounded at 4 GiB; the only binds are read-only public inputs and registry cache. No project code executes on the host in this experiment. The maintainer permits independent compilations in parallel; these frozen isolation limits remain.

One cargo test --frozen --offline --lib --no-run --message-format=json builds the actual artifact. Its --list resolves all nineteen names uniquely. Each direct parent uses --exact --nocapture --test-threads=1, with a 60-second bound. One cargo check --frozen --offline --all-targets --message-format=json follows. Build/check bounds are 1,200/900 seconds. Source is verified and streamed to a 32 MiB bounded export before exact-CID cleanup. No complete suite or provider matrix is repeated. Raw logs and complete source remain in the private engineering archive qa-relay-history-owner-20261111-green-r1.

## Limits

This demonstrates the software correction of the two named full-prefix/consumer defects with actual journal IO, SQLite and local HTTP/SDK where specified. It does not establish process death, filesystem/fsync failure, a real worker's terminal state or runtime join. IO-capacity refusal is not physical storage failure; cancelling a future is not killing the broker. These fixtures construct no Fresh/current-dispatch capability, AcceptedJob, CLI, model or worker engine.

The finite 48-record job budget includes 16 ordinary/legacy and 32 terminal records. Logical tenant reservations and 256 small facts do not prove 12,288 physical files, 192 MiB RSS, fsync latency or 100 workers. Descriptor protections remain; no file limit was raised to pass. Full3 integration, cold crash/restart, Docker, Podman, Kubernetes, A1/A2/B1, multi-host and all global production gates remain open. Primary audits are not closed by association.
