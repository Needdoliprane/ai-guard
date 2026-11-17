# Runtime diagnostics and late D1 finalization — 17 November 2026

[Français](README.fr.md) · [machine-readable proof](results.json) · [diagnostic contract](../../../RUNTIME_MONOTONIC_OBSERVATION.md)

The optional bounded runtime recorder marks actual Rust process, deadline,
capture and cleanup boundaries. It is process-local diagnostics, not durable
audit or an independent engine observation. Full or disconnected recording
never delays STOP or changes its deadline; incomplete recording remains visible.

**12 targeted software tests PASS**, one invocation each, on the frozen source
`c755fb525e98c9bfe768cb5f4fd2f13890318dfe99416e60e06939ad342db3b9`.
The selection has seven recorder/IO units, one error-retention harness case and
four D1 cases running the real ai-guard CLI, Gateway and SQLite with a Rust
engine fixture. The latter check terminal finalization once, the late Managed
failure path, and conservative retention of quota/instance for Created/Unknown.
The exact names, raw-output hashes and durations are in `results.json`.

Pinned Rust 1.99.1 on macOS ARM64 built the three test binaries with
`cargo test --lib --test runtime_confinement --test audit_run2_d1_late_finalizer --no-run --locked --offline -j1`;
QA profiles use opt-level 0, debug 0, debug assertions and overflow checks on,
256 codegen units and no incremental build. Six normal/ignored listings were
verified before the exact cases. The changed test leaf passes `rustfmt --check`.
The all-targets check belongs to the preceding source revision; it is not
transferred to this source. The all-targets locked/offline Clippy check for this
revision returns RC0 with **528 warning emissions and 0 errors**, non-strict.
The 21 additional emissions are one unused test helper, one redundant closure
and 19 needless borrows in the physical-test fixture; they remain tracked.
The existing strict gate stays open.

The previous probe test failed after two passes because it read `io.stderr`
after the actual nonzero-command error had taken ownership of that capture.
The revised test reads the retained typed error, preserving status, invocation,
both EOFs, both markers and the original deadline. The fixture flushes stderr
before emitting its stdout readiness marker. The historical FAIL is retained.
No loss of product capture was established by that failed assertion.

Private raw outputs, executable hashes and receipts are retained outside the
checkout. The executor initially wrote the raw case files into the development
worktree; they were copied byte for byte to the private evidence namespace,
then verified by the integrator. No cases were rerun to correct that placement.

**NOT_RUN:** real Docker/Podman/Kubernetes, provider clients, cold recovery,
the full A1/A2/B1 scenario and 100-worker capacity. The existing ignored physical
runtime case remains ignored. These tests close no production gate and do not
rewrite the audit's historical findings or the previous physical-runtime FAIL.
