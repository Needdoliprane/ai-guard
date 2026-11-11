# Relay owner recovery follow-up — 11 November 2026

This follows a focused source review of the pending Full3 broker relay owner.
It is separate from the primary security audits and does not close their
fingerprints. The reviewed sources are preserved in local, unpublished seed
`f1ce30456bdf7cf46fe9c8139cd88fbe8d22182c`, whose parent is public
`ed3780506f07f809c98f32dbba282d4d162a4ecd`. The seed preserves 124 pending
working paths; it is not a qualified release or a public source delivery.

The standalone [SendV2 components](evidence/2026-11-11/relay-sendv2/README.md)
passed their own fourteen tests and all-targets check. Those results do not
validate this broker owner or its real runtime handoff.

| Item | Source observation | Required result | Current evidence |
|---|---|---|---|
| R1 — historical reconciliation | A reservation or possibly committed claim can retain an older local snapshot. Retirement then uses an incompatible state or stale CAS. Reopening conservatively avoids a second exec but does not complete retirement. | Reconcile exact durable identities and pending operations, choose a compatible transition at the confirmed revision, preserve unknown outcomes without new execution authority. | The absent historical route is reproduced by one HTTP404-versus-200 test after a real reserved-channel positive control. The new read/SDK and broker consumption remain separate implementation and validation work. |
| R2 — faithful reconciliation status | Retirement clears an error before close acknowledgement. A refused/lost close can appear as ordinary waiting with `reconciliation_required=false`. Publication in memory can also precede durable save. | Keep a truthful pending/unknown state until durable compatible acknowledgement; a failed save must not publish completion in memory. | Three exact unit RED results establish the status, error-retention and acknowledgement defects. The corrected journal case now separately reproduces the missing terminal compatibility after valid positive controls. The candidate includes two additional asynchronous IO regressions; independent review and GREEN remain open. |

The source review found no demonstrated unauthorized execution or cross-tenant
disclosure in this scope. Its runtime consequences are NOT_RUN. Four principal
sources and their immediate dependencies were examined; the private primary
audit consultation was partial. The raw review and source hashes remain in the
private engineering archive under `full3-relay-contract-review-20261111/`.

Baseline evidence is retained separately in the private engineering archive:
`qa-relay-history-20261111-red-r1/` contains the single historical HTTP RED
and the genuine 2,457-file post-run source export; the complete experiment and
cleanup returned RC0. `qa-relay-owner-retirement-20261111-red-r1/` contains
three classified REDs and one unrecognized fixture failure, so its overall
experiment remains INCOMPLETE. The 2,516-file source export and cleanup were
verified. The second journal RED uses the same production source and one
corrected test only; it does not repeat the three established REDs. It returned
the expected test RC101 at the `Scope` oracle, after successful Reserved/reopen
and Closed positive controls. Its experiment and cleanup returned RC0; the
genuine 2,516-file post-run export has SHA-256
`30b0b7826d391716cef1c93e9d6c88582def736fc041423997754f2d7e730b50`,
and every source payload matches its input manifest. No source fix or product
pass is inferred from a successful experiment harness exit.

Both source candidates are frozen for separate independent reviews. R1 adds
the typed historical read and its SDK checks; R2 preserves durable retirement
intent and includes real local-journal IO cancellation/capacity tests. Neither
candidate has a GREEN result at this checkpoint. The capacity test does not
establish behavior under a filesystem failure, and a synthetic acknowledgement
does not qualify the Core HTTP or runtime handoff.

## Implementation ownership

- Broker owner: isolated `codex/relay-owner-retirement-20261111` on the local
  seed above. R2 uses transition/status units without engine, CLI, child,
  socket or HTTP operations. The journal uses synthetic files and leases.
- Historical Core/API/SDK lookup: isolated
  `codex/relay-history-reconciliation-20261111` on public `ed37805`. The
  proposed exact route uses the existing authenticated runner boundary,
  tenant/namespace validation and checked Store transaction. It creates no
  reserve, claim, transition, Fresh claim or current dispatch authority.
- Root owns integration and source preservation; Luna executes admitted,
  targeted QA. Independent review remains required after the corrections.

Historical lookup must distinguish an exact recorded receipt, a deterministic
resolution at a verified monotonic snapshot, and an unresolved outcome. A
simple 404 is not a fencing proof. An absent admission can be resolved only
with the separately specified exact terminal worker/attempt observation and
Store serialization proof; otherwise it remains unknown. These are acceptance
requirements, not claims that the endpoint is available.

The old suspended operations are not resumed by this follow-up. Real worker
retirement, crash/restart, Docker/Podman/Kubernetes, A1/A2/B1 and the production
gates remain open. See [current state](CURRENT_STATE.md) and
[the primary audit register](AUDIT_FOLLOWUP.md#registre-des-audits).
