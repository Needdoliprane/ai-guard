# Claude deliveries — 10 November 2026

This register records source deliveries and their integration. It does not transfer historical test results to a new composition, close an audit finding, or qualify a release. Codex owns composition, review, validation, publication and worktree retirement. Original Claude branches remain preserved until that cycle is complete.

## First delivery: three independent lots

All three original branches start at `7672078831c74047b98ce85babf91adb625e2b29`. The maintainer delivered their handoffs on 10 November. References below were checked locally against Git.

| Lot | Original branch and head | Integration candidate | Current status |
|---|---|---|---|
| A — signed audit checkpoints | `claude/audit-signatures-20261110`, `ad9367d29607f54114d518632e97e61a137d4ccc` | `codex/integrate-audit-signatures-20261110`, `197b246587fadd339b3b795da7b5e439224103b0` | Composed S4 RED/FIX and six unique FIX parents passed; all-targets check RC0 on the 2,403-file freeze. Independent source review accepted the bounded changes. [Exact source and limits](evidence/2026-11-10/audit-signatures/README.md). Offline signature slice published; deployment and full NV-01 qualification remain open. |
| B — unified supervision UI | `claude/unified-control-ui-20261110`, `6b7aea7e4f34c5eb1ec351343095e30309e97947` | Forty paths composed on `e95724a`, including the timer fix | Published after79 unique Node passes,15 exact Rust passes and4 browser scenario passes on the corresponding frozen sources. The original Chromium profile failure is preserved; a reviewed QA seccomp variant enabled the later run. [Node evidence](evidence/2026-11-10/frontend-B-node/README.md) · [Backend, browser and limits](evidence/2026-11-11/frontend-B-backend/README.md). |
| C — durable and fair relay | `claude/relay-fairness-20261110`, `d0d496f387d77db9cd824a2762ccabe350652f1d` | Core20 composed on `e95724a`, then standalone SendV2 on `fb956b7` | Core storage/HTTP/config published after nine R3 parents; typed client/stdio components published after fourteen distinct parents and an all-targets check of public B + Setup + Core + wire. Genuine post-run sources verified. [Core evidence](evidence/2026-11-11/relay-core-retention/README.md) · [SendV2 evidence](evidence/2026-11-11/relay-sendv2/README.md). The earlier [Full3 composition](evidence/2026-11-11/relay-full3-composition/README.md) keeps its separate identity. Production admission, complete broker recovery and runtime qualification remain open. |

The new integration candidates use public `2cc29ee42ae3eb3cc8203538bf7a09aee2e26b57`, preserving current contracts rather than copying old central modules wholesale. No original branch has been merged, rebased, pushed or removed by this intake.

Important remaining limits:

- A: CLI checkpoint verification remains explicit and the HTTP integrity route remains hash-chain verification. A separate [SetupV2 viewer-file regression](evidence/2026-11-11/verify-audit-setup-v2/README.md) now passes with the real gateway and two stores, preserving the legacy default. That test does not exercise signed SetupV2 operation. Independent rollback continuity, image/Compose execution and the control-genesis uniqueness observation retain their own open qualifications. Four guide links resolve to curated composition evidence; historical raw records remain private.
- B: the timer is counted once among79 unique passed Node titles; the Node batch retains its historical skipped backend parent. A later matching composition passed15 Rust parents and4 browser scenarios with a real Rust/SQLite fixture stack. These fixtures do not qualify worker runtimes, capacity100, an actual identity provider or the full browser/accessibility matrix. No historical Claude pass is transferred. The pre-refusal page-size observation outside the four backend wire lines remains open.
- C: the core and the proposed typed client wire have separate source identities. Passing core tests will not establish the worker transport or the complete refusal path. Runtime, retention and physical disk/inode limits retain their own qualifications.

## Evidence preservation

Original versioned handoffs remain on each original branch under `docs/engineering/evidence/2026-11-10/`. Private intake is retained outside the checkout in the engineering archive `claude-deliveries-intake-20261110/`:

| Original handoff copy | SHA-256 |
|---|---|
| `A/HANDOFF.original.md` | `1a49c2155965a70604b08f0749f65c34985679fc7a6ebcb4b9b7d30a01ac434b` |
| `B/HANDOFF.original.md` | `d07ac17e96c691f6d07966b8f4b8b0808e8b96a0f299ffefd3199fa25af42bc4` |
| `C/HANDOFF.original.md` | `ee2ba7c741748c6adf3ca5326f702c0b5de28797ccaf756a8d3ba8525c926256` |

The ephemeral QA directory was copied as data: 9,035 regular files, 103,657,457 bytes, verified after copying. The preserved manifest `EPHEMERAL_PRESERVED.sha256` hashes to `163a51e3ee8aa54977fb293e8ab279c529492601b7f4a7604dc16a68305ad163`. It includes private copies of uncommitted development files. **Do not add these raw files or the private archive to Git.** Original records and scopes are retained; no target execution was performed to copy them.

This delivery register complements the [audit register](AUDIT_FOLLOWUP.md#registre-des-audits). Claude source review and historical test claims are distinct from the primary audit records and from Codex composition validation.

## Active scopes: source lifecycle and operator diagnostics

A two-lot `/goal` prompt is prepared at the private engineering path `claude-next-goals-20261110/CLAUDE_GOAL.md`, SHA-256 `4da6921ada7482bf32abc08b2ff70372a7db8339821a34aa0c556f015ca4d8d9`. It reserves these independent scopes on public `2cc29ee`:

- D: `claude/source-lifecycle-20261110` — Rust source preparation and the bounded build/Compose handoff, including the NV-06 source-lifecycle risk. Narrow build/runtime wire hunks require Codex coordination.
- E: `claude/operator-diagnostics-20261110` — a Rust operator diagnostic with actionable findings and English/French procedures, without modifying the installation or contacting providers by default.

The maintainer has now confirmed D and E as active. Both start from public
`2cc29ee42ae3eb3cc8203538bf7a09aee2e26b57`. At this record, the observed D ref
is `b14b89a9b2b8abecda73335275e1b5307d04055d` and the E ref is still `2cc29ee`.
These activity/ref observations do not accept a source delivery or establish
validation. Preserve their ownership and check with the coordinator before
duplicating either scope.

## 11 November — committed D/E source observed for integration

These newer local references are pinned for source preservation; they do not accept a completed delivery or transfer a historical test result. No external Claude worktree, branch, running QA container or live file was changed.

| Lot | Observed commit | Changed paths from 2cc29ee | Protected Root intersection |
|---|---|---:|---|
| D | 15b83b7f6b2b6a48638475f575c1d28805b5b2f9 | 25 | src/runtime.rs |
| E | d1a367347f08a1171af6eb496c5eefa4dbd7209f | 43 | None in the protected 134-path set |

D provides the Rust source snapshot and retained build/Compose handoff; E provides the Rust operator diagnostic and EN/FR procedures. Their committed trees are retained locally under refs/ai-guard/intake/claude-d-20261111 and claude-e-20261111, with the observation inventory in the private engineering archive claude-deliveries-intake-20261111. These local references are not pushed by this intake.

Composition against the current public source, independent review and new targeted validation remain NOT_RUN. In particular, the absence of a protected-WIP intersection for E does not prove that it composes with intervening public changes. D must preserve the existing Full3 changes in runtime.rs. Root continues this intake while the distinct Kubernetes Core, schema compatibility and targeted test work proceed in parallel.
