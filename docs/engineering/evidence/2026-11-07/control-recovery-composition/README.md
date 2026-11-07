# Control, recovery and authentication composition — 7 November 2026

Status: **52 targeted software functions PASS; all-target compilation PASS**.
This record qualifies the named software checks on one frozen source. It does
not qualify a release, a real worker runtime or any global production gate.

## Source and review

The frozen `core-hilm-sdk-composition26` starts from integration commit
`238fddd9fcb79b192a5c6a503ba344044f4f83d0`, including the published I/L/H-server
and M authentication-budget changes. It adds the Core25 recovery changes and
explicit H control-client selection. [source.sha256](source.sha256) records all
417 Rust/Cargo files in that source. It is a Rust/Cargo manifest, not a claim
that every repository artifact was exercised.

The merged managed source is
`305a0a43f0825c2224813254f9dfc391b986add9675da0dcc01c2883e08a442a`;
the monitor is
`762735a3122ea106f53f7e10806f2ef4486b162bebaf03dd6161efa6a3e323f7`;
the main binary source is
`fb75bb8a17c07bb2fe4259290bb1e595bae06d896d87d21b08457779014210f2`.
Independent integration review checked that the managed file differs from
Core25 only by the three explicit SDK wrappers, the monitor retains the entire
strict Core25 adoption test, and the main H/M merge preserves both commands.
No cached identity, public fallback or new retry authority was introduced.

The [Core25](../recovery-stack/README.md),
[client SDK](../control-clients/README.md) and
[authentication-budget](../authentication-proof-budget/README.md) records retain
their own sources, failures and results. Their PASS results were not substituted
for the composition checks below. Later documentation was finalized without
changing these frozen Rust files. The new relay client and the separate D5
fencing development worktree are not part of this source.

## Observed checks

Each test command is equivalent to
`cargo test --offline --locked --manifest-path /inputs/core-hilm-sdk-composition26/Cargo.toml`,
followed by the selection below and `-- --nocapture --test-threads=1`.
The parent logs also retain safe invocation/settings metadata and actual test
names. The all-target command uses `cargo check --all-targets --locked --offline`
with that manifest. No full E2E or provider matrix was run by reflex.

| Selection | Distinct functions | Observed duration | Output SHA-256 |
|---|---:|---:|---|
| `--lib runner::managed::control_monitor::tests::` | 23 PASS | 40.33 s; build 1 min 58 s | `2193f6afefdf26615906d47b8b4ce39438348c31ae790424e8cb5c3acf0e0ecf` |
| `--lib api::control_transport::tests` | 4 PASS | 2.87 s | `16d88932cb06899649fafe7e5d95353d0dd474f5614e9b51d403ce898918b8a1` |
| `--lib api::ingestion_window_tests` | 3 PASS | 0.11 s | `52bb4232a4d9f1dc2d2e2771997060f0e4dee2531e9adc2dc46e11ff0d9bb7ff` |
| `--test supervision_api control_transport` | 2 PASS | 0.94 s | `41a79c27c93d4759d1537741e5fe6cf84db9fba90e6ebc5b253ef262a3a3415c` |
| `--test authentication_migration_cli` | 5 PASS | 0.14 s | `473a3e852a9895ffe0d5ac66d3270a00f148bf779d17fdc0e16652b31db579b3` |
| `--test audit_auth_ledger_replay` | 14 PASS | 1.95 s | `c07154f9ac42b811efcb0bdcd98e888725bccedd53c8de50ccd32656174cf699` |
| `--test runner_file_audit lost_file_ack_replays_exact_retained_event_without_runtime_or_duplicate_change` | 1 PASS | 24.70 s | `faad980e1d6179ffaa34be3a4dde90f08b78ad63f556391c873dd064599e0ccf` |
| `check --all-targets` | RC0, no tests counted | 1 min 22 s | `f051ea3326a2a640d97e597ca1288999d92ff7be765ef4914cdd59aaf73a7e4e` |

The first seven selections ran in QA AA. Its cutoff left the all-target check
NOT_RUN there; the header-only AA check log is not a successful build. QA AB
used the identical read-only source and a fresh source-owned target to finish
that check. The 52 passing functions were not repeated. The build reports
119 library warnings, two SSO-test warnings and three library-test warnings;
Clippy and the static-analysis gate remain open.

## Environment and practical limits

Root verified each local QA's full configuration and effective kernel controls:
Linux ARM64, pinned Rust 1.99.1 and validation-image digest, UID/GID1000,
capabilities zero, no-new-privileges, seccomp, private namespaces, network none,
read-only source/toolchain/registry/root, no engine socket or published port.
Limits were 4 CPU, 8 GiB RAM with no swap, 256 PIDs, 4 GiB scratch/65,536 inodes,
512 MiB per file and 1,024 FDs. Builds used one Cargo job, disabled incremental
and debug information, and no stack-size override. No OOM was observed in these
composition selections. The QA lifecycle used a host-clock deadline; AB also
has a host watchdog and an earlier container lifetime cutoff.

These tests exercise real Rust clients, HTTP handlers, SQLite, private journals
and selected CLI subprocesses with synthetic identities and a Rust engine
fixture. Its observations remain fixture declarations. They do not demonstrate
Docker/Podman/Kubernetes containment, real three-worker A1/A2/B1 behavior,
100-worker capacity, remote federation, provider subscriptions/API, enterprise
PKI, power-loss recovery or filesystem-full behavior.

The integration does not resolve mixed broker-outbox provenance, the complete
D5 owner/fencing lifecycle, all cross-tenant availability coupling, global audit
capacity/retention, authenticated human approvals, signed checkpoints or
network-egress control. No external audit finding or
[production gate](../../../PRODUCTION_READINESS.md) is closed by these results.
