# Relay composition software evidence

The 2,459-file composition combines the broker Full3 development snapshot, the durable relay CoreC20 and the typed SendV2 wire. Its source manifest is `90d8eb66add84b1e4ce62c1fb2a7414a1a4530132520efd72d6690c43ed07d78`. The source is a candidate. The public branch retains its independent source identity.

| Check | Actual result |
|---|---|
| `cargo check --frozen --offline --all-targets` | RC0 on r1, 10 November 22:17:31–22:19:47 UTC, one successful Cargo build-finished record |
| `cargo test --frozen --offline --lib --test relay_fairness --no-run` | RC0 on r2, 10 November 23:49:36–23:54:46 UTC |
| Direct binary lists | RC0 for both targets, 1,035 library tests and 21 fairness tests listed |
| Exact selectors | Twelve unique parents passed once each, 0 failures and 0 ignored, 10 November 23:57:17–23:57:27 UTC |

Nine parents cover opt-in SendV2, unchanged V1 capacity responses, version and request correlation, scoped refusal details, closed parsing and the worker helper exchange. The integration parent uses the actual gateway and SQLite with synthetic worker facts. Three canaries cover the binding and limits handshake, the identity of a pending response and preservation of an earlier unknown result after a later refusal. The HTTP replies in the helper fixture are controlled responses. The direct audit-head assertion in the client parent covers the Grant case.

Each parent used the exact name resolved from the built binary list and ran with `--exact --nocapture --test-threads=1`. Both jobs used the pinned Linux ARM64 Rust 1.99.1 image `sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`, UID1000, no network, read-only root and public mounts, no capabilities, no-new-privileges and private bounded scratch. CPU4, RAM9GiB, swap0 and PID128 limits were observed. Each job had its own target and five verified public cache entries.

The actual source exports from r1 and r2 each contain the 2,459 verified regular files and have SHA-256 `ddadf9f09aafde01092928225a314ca6a3bec68a0465ff8804a98a2cd0640706`. The r1 watcher handle was absent after a session rollover. The idle r1 container was found beyond its intended deadline, its source was preserved and it was removed at 23:48 UTC. R2 completed and was removed after verification at 00:00 UTC. Its cgroup recorded zero OOM events. Raw results and closure records remain in the private engineering archive. The r2 result index SHA-256 is `613528656a0e72fe9f209a499b41eeae8d7a04b31bb2c28991f0a6b408023a7b`. The r2 closure SHA-256 is `371b8001c65f970723ba637a3a28b8e18f579106f800716242e1c9e1ffc145a8`.

The durable storage and HTTP/config tranche has a separate integration on the current public base. Full3 lifecycle, runtime transport, real Docker/Podman/Kubernetes, physical storage limits, 100 real workers and complete NV-12 qualification remain open. See the [delivery register](../../../CLAUDE_DELIVERIES_20261110.md) and the [audit register](../../../AUDIT_FOLLOWUP.md#registre-des-audits).
