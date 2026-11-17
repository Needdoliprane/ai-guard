# D7 and Native17 — dated private software evidence

[Français](README.fr.md) · [Results and full SHA-256 references](TESTS.json)

The public product code remains `e794e45de243951f401a0ff0df741307cff01c00`.
The results below belong to distinct **private frozen sources**. This document
publishes evidence metadata, not their code or a qualification of the public
checkout. No build or test was run for this documentation update.

| Source | Observed result | Scope |
|---|---|---|
| D7 R5 `653e2abe…` | All-targets check RC0; libtest build RC0; **28 PASS / 1 FAIL / 5 NOT_RUN** | 20 first cases PASS, then eight PASS and case 9 aborts with actual process RC−6/SIGABRT, stack overflow before its oracle. No retry. |
| D7 R6 `8d174676…` | New libtest build RC0; **one case 9 PASS RC0** | Test-only heap boundary (`Box::pin`), no stack setting, clock, deadline or oracle change. The 28 R5 passes were not rerun or transferred; five cases remain NOT_RUN. |
| Legacy Native+Ports R1 `0dce799c…` | All-targets check **FAIL RC101**, one E0599; libtest build/list/tests **NOT_RUN** | Missing fixture method `has_local_slot`, `src/broker/enforcer/client_legacy_tests.rs:229`. Correction `07c9679` is not recompiled. The old case 9 is obsolete and NOT_ADMITTED on this source. |
| Common Native17 R5 `a3e9ca09…` | All-targets check RC0; libtest build RC0; **0 PASS / 2 FAIL RC101** | One invocation of each real software diagnostic, no harness failure or retry. Both fail closed on the unchanged two-second epoch. |

There is no combined “29 PASS” result across D7 R5/R6. The R6 success belongs
only to its new binary and source. The later Legacy composition changes the
construction contract: its retained old case 9 exercises neither genuine Fresh
construction nor an actual old V1 peer. That source conclusion does not rewrite
the historical R5 abort or R6 PASS. The replacement CLI regressions remain
private, under review and not compiled or executed. The Legacy check proof is
`67ab96678b2b7ab3c71e58583e402c720dfd713373c551fe983682c2502029a0`;
its command receipt is recorded in [TESTS.json](TESTS.json).

## Native17 observations and limits

The first R5 libtest build was interrupted for resources, actual RC−15, and is
preserved. A separately admitted build on the same source then returned RC0.
Root verified the executable, source and effective listings before releasing
the two cases. The new invocations both returned RC101:

- Enrolled Broker heartbeat: 60.523 s for the whole case. The retained backup
  postcheck now exposes the typed `human_authority_epoch_expired` cause while
  retaining the native owners.
- A17+B16/Auth5/OIDC admission: 54.590 s for the whole case. The writer refuses
  at `TransactionMutations`, elapsed 1,999 ms, with the original epoch error.

These observations do not measure CPU consumption or lock wait. Monotonic
phase intervals include scheduling, waits and instrumentation. The
`SourceEpochReleased` mark occurs after the drops, so the intervals are not a
mathematical lower bound on epoch ownership. The actual typed `epoch.check()`
result remains authoritative. No product epoch was extended. The two private
remediations are in progress; neither diagnostic establishes a successful
Native17 positive path, and no remediation code is published by this record.

The Native17 case proof is
`8a70c0410c924fe650d2521fe35972fc308b06ee0fe97670826b5b7bee943e80`;
the libtest/list proof is
`0dc396349778b5e58cd9c4d5091856b0963c3da9ba7b766ffee3b838d2b1c1df`.
The [JSON](TESTS.json) records full source, binary, command and Root proof
hashes, exact FQNs and actual return codes without private machine paths or
raw reports. The 19 previously dependent Native17 cases remain NOT_RUN.

## D1 and qualification

The existing [12-case D1 record](../runtime-observation-and-d1-late-finalizer/README.md)
already covers software Store/quota finalization through four real
CLI/Gateway/SQLite cases with a Rust engine fixture. The current summary no
longer lists that software fault injection as missing. Real engines, physical
TTY, abandonment and cold recovery remain separate qualification gaps.

Strict Clippy remains open. These private campaigns do not replace the
public revision's historical non-strict Clippy evidence. No engine/provider,
cluster, full A1/A2/B1, cold recovery or 100-worker qualification follows.
The primary audit verdicts and severities remain unchanged: **0/12 production
gates closed**, no functional completion percentage or qualified release.
[Gates](../../../PRODUCTION_READINESS.md) · [Audit run-2](../../../AUDIT_RUN_2_20261110.md).

## Recorded command forms

These portable forms describe recorded invocations; none was newly executed
for this document. Use the corresponding frozen source and pinned toolchain,
with the QA profiles recorded in [TESTS.json](TESTS.json): opt0/debug0,
debug assertions and overflow checks on, 256 codegen units and no incremental
build for dev/test. The commands also used those profile overrides.

```sh
cargo check --all-targets --locked --offline --message-format=json -j1
cargo test --lib --no-run --locked --offline --message-format=json -j1
"$AI_GUARD_LIBTEST" --exact "$AI_GUARD_FQN" --nocapture --test-threads=1
```

The outer case limit was 180 s and each selected case was invoked once. It
does not alter a product deadline. Source and raw evidence archives remain
intact; old results are not transferred to another source.
