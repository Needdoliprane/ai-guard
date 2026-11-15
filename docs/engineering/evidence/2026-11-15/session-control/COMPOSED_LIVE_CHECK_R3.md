# Current composed candidate — Rust check R3

Date: 2026-11-15. This report applies only to the frozen candidate described below; the source was not a Git commit or a production release.

- Source: ac8b5245f68ab605350e45cd18bae247b1f6e1ef10166bc110f3b9fb8f0c6103, 3,157 files / 40,678,400 bytes, based on e8886bd plus the delivered Claude session-control commits and the explicitly applied native patches.
- Command: cargo check --all-targets --frozen --offline --message-format=json -j1. One actual Cargo invocation.
- Result: FAIL, RC101. Eleven coded diagnostics: one missing sizing export, one missing broker-presence module, one result-type conversion, one private data helper, one sequence/digest type mismatch, and six missing metadata sizing/comparison methods. Two code-less summary messages are not extra defects. Forty warnings; build-finished=false. No test was executed.
- Scope differs from the earlier baseline R2: it includes API R3, GrantR4, the native frontend delta and seven restored journal funding hooks. Comparing 11 to the earlier four errors is not a same-source regression comparison.
- After this run, the actual sizing producers/pre-action funding calls and three API corrections were independently source-reviewed and applied. Their successor content has not yet been recompiled. Broker-presence composition still requires the real native maintenance/Source17/E2 dependency chain; no stub was inserted to make a build appear successful.
- Evidence USTAR: ae39ec0aa40440b474f6ac632d471967359dd9fa2f8b3ef13be3210377206cdd, 2,857,984 bytes, 54 regular files, strictly parsed by Root.
- Root receipt: 5e3476c8e2cb1ad1ed21fe71086f6fdad791c054d5765256724f697067b2024b. Luna terminal handle22429, RC101. Raw compiler JSON, actual slot return codes0/101/0 and closure were independently inspected.
- Source/cache, three sandbox guard phases and collector integrity PASS. No OOM observed; measured peak 4,865,011,712 bytes under the 10GiB cap. Exact QA container closed/removed and its absence independently checked.

The earlier [native frontend selection](FRONTEND_NATIVE17_QA_R1.md) remains source-specific: 53 active tests and its build passed. Backend tests and real runtime/provider paths remain NOT_RUN for this candidate.

At the time of this R3 compiler check, Kubernetes lifecycle/attach QA was pending. Its later source-specific results are recorded in [Kubernetes R4/R5](KUBERNETES_LIFECYCLE_QA_R4_R5.md). Those results do not activate Source17/S5 or qualify a real cluster.

The source contracts still require a genuine sender authorization/revocation boundary at message emission; the execution/STOP/FD guard alone does not prove that boundary for a different sender. The proposed contract direction has not been approved or applied.

0/12 global production gates closed; no reliable aggregate functional percentage is established. These records are development evidence, not qualification of the published branch or the full product.
