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
| R1 — historical reconciliation | A reservation or possibly committed claim can retain an older local snapshot. Retirement then uses an incompatible state or stale CAS. Reopening conservatively avoids a second exec but does not complete retirement. | Reconcile exact durable identities and pending operations, choose a compatible transition at the confirmed revision, preserve unknown outcomes without new execution authority. | The Core/API/SDK slice is independently reviewed and passes fourteen exact tests plus the all-targets check after its HTTP404-versus-200 baseline. Broker consumption and full-prefix local history remain open. |
| R2 — faithful reconciliation status | Retirement clears an error before close acknowledgement. A refused/lost close can appear as ordinary waiting with `reconciliation_required=false`. Publication in memory can also precede durable save. | Keep a truthful pending/unknown state until durable compatible acknowledgement; a failed save must not publish completion in memory. | Four baseline defects are reproduced across two separate experiments. The reviewed local Full3 correction passes six exact tests, including two asynchronous IO regressions, and the all-targets check. Full Core/runtime retirement is still unqualified. |

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

The two corrected candidates have separate GREEN results and source reviews:
[Core/API/SDK, fourteen parents](evidence/2026-11-11/relay-history-reconciliation/README.md)
and [local Full3 R2, six parents](evidence/2026-11-11/relay-owner-retirement/README.md).
These results do not validate their combined broker consumer. The capacity test
does not establish behavior under a filesystem failure, and a synthetic
acknowledgement does not qualify the Core HTTP or runtime handoff.

## Implementation ownership

- The Core/API/SDK slice is published in `300cb4a`; the R2 correction is composed
  into the protected local Full3 work. Historical author branches remain at
  `515c797` and `9eb4302`, while their two worktrees were closed normally after
  source, ignored-file and consumer checks. Private backups remain.
- The combined journal and consumer correction used separate
  `codex/relay-history-journal-20261111` and
  `codex/relay-history-consumer-20261111` worktrees with exclusive file owners.
  Local-only base `7cf3aa0` preserves 126 Full3 paths on public `300cb4a`;
  that seed and the whole development branches are not public deliveries.
  After the combined GREEN, the two worktrees were removed normally following
  verified eighteen-source backups, clean/ignored inventories and empty
  consumer observations. Local preservation commits 9bd733d and 0e07464 and
  their branches remain; the protected Root composition is retained.
- The read uses the existing authenticated runner boundary, tenant/namespace
  validation and checked Store transaction. It creates no reserve, claim,
  transition, Fresh claim or current dispatch authority.
- Root owns integration and source preservation; Luna executes admitted,
  targeted QA. Independent review remains required after the corrections.

## Full-prefix and consumer baseline

The combined 2,522-file baseline has two independently reviewed, exact RED
results. The current reader rejects a valid chained V2 fact after successful
V1 reopen; the current consumer cannot retire an old Close(CAS1) from Reserved
after a successful historical SDK `InapplicableState` control. Both return the
expected test RC101 at distinct oracles; build, experiment and cleanup return
RC0. A successful RED experiment is not a product pass.

Actual post-run source archive: 28,344,320 bytes, SHA-256
`2a6d4a29b0f344b24b50483e8073d154f082d27b2b99ab06619b912225467cae`.
All 2,522 payloads match input manifest
`4872891d52e994f5e3374ca7c4be8ae2e001bd15a47deb65f77dd30553af7f49`.
The two parents share one offline build; no all-targets or provider matrix was
repeated for this baseline. The exact container was removed without OOM.

The independently reviewed correction contract requires full-prefix V1/V2
validation, immutable intention identities and proofs, and explicit retirement
before finalization. Its finite budget is 48 total records per job: the first
16 include legacy/ordinary records, with 32 reserved for terminal progress.
This is an implementation contract, not a deployed configuration or a proven
disk/RSS bound. Consumer and journal fixes are being implemented in parallel;
their combined FIX, GREEN and runtime proof remain open.

Historical lookup must distinguish an exact recorded receipt, a deterministic
resolution at a verified monotonic snapshot, and an unresolved outcome. A
simple 404 is not a fencing proof. An absent admission can be resolved only
with the separately specified exact terminal worker/attempt observation and
Store serialization proof; otherwise it remains unknown. The public Core/API/SDK
slice implements these read semantics; broker recovery and runtime acceptance
remain separate obligations.

The old suspended operations are not resumed by this follow-up. Real worker
retirement, crash/restart, Docker/Podman/Kubernetes, A1/A2/B1 and the production
gates remain open. See [current state](CURRENT_STATE.md) and
[the primary audit register](AUDIT_FOLLOWUP.md#registre-des-audits).

## Composed correction —15:04 UTC

The journal and consumer now have a combined software GREEN: nineteen exact parents and the all-targets check return RC0 after the two baseline REDs. Independent final review covers the exact eighteen postimages and test-only counter correction. Root composes them into local Full3, preserving and verifying 134 paths. [Full evidence and scope](evidence/2026-11-11/relay-history-owner/README.md).

This supersedes the earlier implementation-pending status only for the two named software defects. Full3 remains unpublished; real cold recovery, terminal process/join observations, runtime transport, descriptor/RSS scale and production gates remain open. Unreachable historical CAS remains Unknown by contract; no new execution authority or universal convergence is claimed.
