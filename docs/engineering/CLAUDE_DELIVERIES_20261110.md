# Claude deliveries — 10 November 2026

This register records source deliveries and their integration. It does not transfer historical test results to a new composition, close an audit finding, or qualify a release. Codex owns composition, review, validation, publication and worktree retirement. Original Claude branches remain preserved until that cycle is complete.

## First delivery: three independent lots

All three original branches start at `7672078831c74047b98ce85babf91adb625e2b29`. The maintainer delivered their handoffs on 10 November. References below were checked locally against Git.

| Lot | Original branch and head | Integration candidate | Current status |
|---|---|---|---|
| A — signed audit checkpoints | `claude/audit-signatures-20261110`, `ad9367d29607f54114d518632e97e61a137d4ccc` | `codex/integrate-audit-signatures-20261110`, `197b246587fadd339b3b795da7b5e439224103b0` | Independent source review accepted the bounded S4, CLI wire and schema-12 changes. New composition execution and publication remain open. |
| B — unified supervision UI | `claude/unified-control-ui-20261110`, `6b7aea7e4f34c5eb1ec351343095e30309e97947` | `codex/integrate-unified-ui-20261110`, unmerged candidate on `2cc29ee`: forty-path composition plus the targeted timer fix | Node QA records 79 unique passed titles and one real-backend SKIP; TypeScript/Vite passed on the 61-source frontend freeze. This candidate is not public code. Backend Rust and browser execution remain NOT_RUN; review, integration and publication of B remain open. [Exact evidence and capture gaps](evidence/2026-11-10/frontend-B-node/README.md). |
| C — durable and fair relay | `claude/relay-fairness-20261110`, `d0d496f387d77db9cd824a2762ccabe350652f1d` | `codex/integrate-relay-fairness-20261110`, `d2dea1eba4b5ad77b0429856ca1b113d990e6e67` | Core candidate reviewed. Typed client/stdio refusal wire remains a separate, required integration task. Composition tests and publication remain open. |

The new integration candidates use public `2cc29ee42ae3eb3cc8203538bf7a09aee2e26b57`, preserving current contracts rather than copying old central modules wholesale. No original branch has been merged, rebased, pushed or removed by this intake.

Important remaining limits:

- A: CLI checkpoint verification is an explicit option using the existing configuration layout. The HTTP integrity route remains hash-chain verification. Setup V2 integration, independent rollback continuity, image/Compose execution and the control-genesis uniqueness observation remain open. Four historical guide links need their evidence curated before publication.
- B: only the recorded Node frontend slice has been executed. Its timer passed alone and again in the batch; it is counted once among 79 unique passed titles. The backend-dependent parent was skipped, and browser/backend Rust execution remains NOT_RUN. No historical B pass is transferred. The pre-refusal page-size observation outside the four backend wire lines remains open.
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
