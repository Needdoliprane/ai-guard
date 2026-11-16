# D1 prestart control — bounded software fix, 16 November 2026

[Français](README.fr.md) · [Audit register](../../../AUDIT_RUN_2_20261110.md) · [Validation](../../../VALIDATION.md) · [Production gates](../../../PRODUCTION_READINESS.md)

## Result and source boundary

**14 exact tests PASS**, one invocation per case: four actual ai-guard CLI/Gateway/SQLite cases, two Managed/PTY units, three PTY neighbours and five deadline neighbours. The CLI is ai-guard's Cargo binary `agentsandbox`, **not Codex CLI or Claude Code**. Only the engine is an existing Rust fixture; no real Docker/Podman daemon, provider call or cluster was exercised. Three executables and the indexed 2 932-file source were rehashed; Root checked the 42 raw output/RC files.

The tested source consists of the exact eight final D1 Rust files plus two D7 cfg(test) files. **The two D7 files are excluded from the D1 integration commit**; no D7 product fix is included. Those extra test registrations do not qualify D7. The eight D1 paths are:

- `src/runner/control_monitor.rs`
- `src/runner/control_monitor/prestart.rs`
- `src/runner/managed.rs`
- `src/runtime.rs`
- `src/runtime/managed.rs`
- `src/runtime/managed/legacy_completion_tests.rs`
- `src/runtime/terminal.rs`
- `tests/audit_run2_d1_prestart.rs`

The raw source review and the formatter-only preparation have separate records. Current integration base: `961045788784b277a7a9eaa5e8f411275d7147e0`. **D1 Clippy: RC0, non-strict; 507 warning emissions and 0 errors**, on the same composed source `74d60de1…b7b`. Receipt SHA-256: `dc43ad33095e9b30353ca6ed8424c62f9157dbc8a4da0cf4433f506b31e0e0a2`. The strict gate remains open. No whole-suite or strict Clippy result is claimed.

## What changes

A successful, validated first control GET is required before readiness. On a control failure, the same private owner revokes admission before awaiting STOP and keeps the original runtime, client, plan and attempt FD/flock. The veto restricts the existing first-start authority; it does not create a new execution grant, Native17 Current or prearmed capability. Its final consumption occurs at the actual spawn after the last checks. TTY preparation uses the joined `checked_blocking` path so it does not stall monitor polling; the veto follows PTY/stdio/pre-exec preparation, immediately before spawn.

A known native terminal, captured IO and the original cause are retained rather than erased by a later control error. A genuine same-operation fallback completion can reach the existing finalizer once; Created, incomplete or contradictory observations remain uncertain. A CLI exit alone is not a terminal proof. The **60-second post-ACK window is unchanged**, with no pre-ACK tolerance added; it is not a measured physical STOP bound.

## Baseline and corrected observations

The [earlier baseline](../control-monitor-prestart/README.md) remains immutable. Canary C1 passed. The prestart 429 fault then failed RC101 because the Rust engine fixture observed `starts=1` where zero was required; Root separately verified `audit_valid=true`. Assertions after that first failure were not reached and are not retroactively credited.

On the fixed source, the same prestart fault passes with `starts=0` and `audit_valid=true`. The last-authority-check fault also observes zero starts. The after-start/before-second-ACK contrast observes `starts=1`, a native STOP, a committed callback, owner exit and a valid audit. A control error does not erase that genuinely known result.

| Phase | Observed result |
|---|---|
| c60 canary | PASS; one legitimate start, exact stop, valid audit |
| c60 prestart fault | RED RC101; one start after failed control; audit valid; later custody/quota assertions not reached |
| Fixed prestart fault | PASS; zero starts, valid audit |
| Fixed final-authority fault | PASS; zero starts, valid audit |
| Fixed after-start contrast | PASS; native STOP, callback committed, owner exited, valid audit |

## Exact executed selection

| # | FQN | Result |
|---|---|---|
| 1 | `canary_cli_owner_enforces_durable_stop_without_prestart_fault` | PASS |
| 2 | `prestart_read_failure_refuses_cli_start_and_retains_audited_owner` | PASS |
| 3 | `prestart_failure_during_final_authority_check_refuses_first_start` | PASS |
| 4 | `control_failure_after_start_before_started_ack_preserves_terminal_known` | PASS |
| 5 | `runtime::managed::legacy_completion_tests::late_observe_failure_keeps_real_fallback_completion_and_original_cause` | PASS |
| 6 | `runtime::terminal::tests::final_veto_refuses_after_pty_preparation_before_transport_spawn` | PASS |
| 7 | `runtime::terminal::tests::guard_explicit_restore_and_drop_restore_presentation_and_attributes` | PASS |
| 8 | `runtime::terminal::tests::task_cancellation_restores_real_pty_attributes` | PASS |
| 9 | `runtime::terminal::tests::reaped_transport_cannot_hide_a_filter_failure_or_incomplete_control_in_its_tail` | PASS |
| 10 | `runtime::deadline_supervision_tests::deadline_remains_pollable_while_observation_pending` | PASS |
| 11 | `runtime::deadline_supervision_tests::stop_remains_pollable_while_callback_pending` | PASS |
| 12 | `runtime::deadline_driver_tests::deadline_progresses_stop_during_same_callback_and_preserves_late_causes` | PASS |
| 13 | `runtime::deadline_driver_tests::checked_blocking_keeps_control_pollable_and_allows_second_real_pool_check` | PASS |
| 14 | `runtime::managed::deadline_semantic_tests::terminal_proof_retains_late_callback_error_and_exempts_only_real_proof_from_fallback` | PASS |

The first four rows are the integration target `audit_run2_d1_prestart`; the remaining ten are the library test binary. The executed argv use `--exact <FQN> --nocapture --test-threads=1`, not a provider client. Cases reused the same stage-A artifacts; no fresh build is inferred from a case invocation. The preparatory receipt verifies format, check and test build on this source. A separate Clippy invocation used `--all-targets --locked --offline --message-format=json -j1` with the same explicit profiles; it did not rerun the tests. Six warning emissions concern four spans in D1 files. The two `collapsible_if` emissions at `runtime/managed.rs:1753` are new; the other four D1 emissions already exist in the preimage. The existing `await_holding_lock` belongs to a blocking-commit test with preserved ordering, rather than a new production wait. This debt remains open. No strict Clippy result or new test run is claimed.

## Receipts

Private originals remain outside Git. The names and hashes below identify the evidence without publishing private reports, paths, credentials or raw logs.

| Evidence | SHA-256 / scope |
|---|---|
| Fixed composed source / Source composée corrigée | `74d60de1c9348615b7c1a34bf8df55fa91bf5448b6c0b1d2197ef3aef0b02b7b` — 2 932 files / fichiers |
| D1 result receipt / Reçu des cas D1 | `7aaee770698c99434e0920e16e8d11d8ebf92da22f50ae44b3ab647a03595526` |
| Root result verification / Vérification Root | `e2dea73b8bd75d05a86f19fc4855518c7e110c8dc9c4e7ac9b9fa794bbe45145` |
| Root preparatory format/check/build verification / Vérification préparatoire | `884abb68dd2f8a6cec1c6881d7873933f8757104b3c35e4ac5368d5c35c3dcd8` |
| Raw D1 R2 source review / Revue source brute D1 R2 | `ed778d68f67fe3949e3eb8a67716448e0806eded418417aecb23c9aa8279aaab` |
| Baseline c60 source / Source baseline c60 | `c60b0cb088491416bcfb4ca09936a4c81d99020b737145004f6794cbae24cc53` |
| Baseline canary C1 / Canari C1 baseline | `c8cf72d5308f5f42d17bcd3c0efcb2a8d5a8cb59a11999fb78dd789ec1207cc0` |
| Baseline fault + Root verification / Faute baseline et vérification Root | `579bca8cf2b24d2ada7ab4cc6d579f8b0ebde6d3a1a7e6c99225c556e59457be` ; `369bb8cb42b6a2512ae84165b0e6d3750f082bfd21314efe8be96442a07721ab` |

## Limits and open work

- The Managed late-observation unit exercises the real completion producer and affine recovery with its original cause. It **does not prove the exact late fault through runner → Store finalizer → quota**; that end-to-end injection remains open.
- The negative PTY helper and three neighbours cover local software PTY/RAII behaviour. A real controlling user TTY/provider session, other platforms and physical runtime STOP timing are not qualified.
- External abort/panic/cancellation and cold recovery/fencing retain their open requirements; the owner's Drop is not proof of asynchronous STOP or terminal completion.
- D7 remains open, with two separate phases. R1 stopped at a TMPDIR/UDS-path setup prerequisite before the oracle (`6420d89778e218f4efcec5be3550c332934aacbbcf72fe3643ace36d3fe6e84e`); that non-conclusive failure is preserved. R2 changes only this harness configuration: cap2 passes RC0, then Restore cap1 reaches the genuine ACK-without-STOP/Cancel coverage assertion and fails RED RC101. The third parent remains NOT_RUN. Root verification `5634d05010bfc45376a9420ee62f3697dd22d7fcdbb8d05ac4ee498c0314c487` pins result `f712a664fcfa4027bf48cf1792ae8b4c523ad2fced32290f992cf92f6a25b971`. These D7 observations do not merge with the fourteen D1 PASS, and no D7 product fix, real-engine proof or new severity is claimed.
- No Codex/Claude provider flow, real Docker/Podman/Kubernetes, real cluster, 100-worker capacity or production release is qualified. The original audit verdict/severity is unchanged; this is a software contribution to its D1 fingerprint only. **0/12 production gates are closed**.
