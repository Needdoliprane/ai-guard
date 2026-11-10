# Kubernetes session composition — 10 November 2026

Software evidence only. Workload admission remains closed. No Pod, worker
start, release, cluster operation or runtime qualification is established.

## Sources and distinct results

The thirteen Kubernetes postimages were composed on public `2cc29ee`. Their
publication preimages are public `8697e54f1efd3bd1e811862cb900e2e9dee71663`;
changes outside those paths, including the independent OIDC delivery, are
preserved. The results below remain bound to their complete QA sources rather
than being transferred to every later checkout or unrelated work in progress.

| Freeze | Full source manifest SHA-256 | Observed result |
|---|---|---|
| R1, 2384 files | `9a6d04a879fa50db98125355c288e75cd109245cb330517345d444b348ff3831` | Build/list RC0; parents 01–05 PASS; parent 06 FAIL RC101 before its intended cross-Store oracle. |
| R2, 2384 files | `0a6919437e4cb1f3148dd66bca1366a6e9525d6f8163efb01978e9b55e6072c9` | Build/list RC0; only parent 06 rerun, PASS RC0; all-target check RC0. |

R2 changes only the function
`s2_confirmation_cannot_cross_from_the_authenticated_tenant_to_another_store`
in `confirmation_tests.rs`. No production code changes between R1 and R2.
The five R1 parents were not rerun on R2: these are **five R1 passes plus one
R2 pass**, not six passes executed on R2.

R1 parent 06 stopped at `Arc::ptr_eq`: the authenticated Store has an additional
redaction wrapper and intentionally differs from the original raw Arc. R2
checks the selected tenant and exact initial ledger, retaining the real A
confirmation, foreign usage-lease refusal and unchanged A/B ledger/quota
oracles. The retained fixture failure does not demonstrate a product flaw.

## Exact selectors

Each selected parent ran once on the freeze indicated; none was ignored.

| Parent | Exact library selector | Source/result |
|---|---|---|
| 01 | `audit::kubernetes_session::tests::confirmation_retains_only_a_real_reserved_observation_without_new_event` | R1 PASS |
| 02 | `audit::kubernetes_session::tests::confirmation_refuses_closed_reservations_and_a_close_authorization` | R1 PASS |
| 03 | `audit::kubernetes_session::tests::schema12_preserves_v2_reservation_quota_and_before_effect_confirmation` | R1 PASS |
| 04 | `runtime::kubernetes::bootstrap::tests::cancelled_waiter_does_not_drop_the_owned_exchange_or_its_capacity` | R1 PASS |
| 05 | `audit::kubernetes_session::tests::s2_confirmation_tests::s2_confirmation_rechecks_a_real_retired_epoch_without_changing_the_reservation` | R1 PASS |
| 06 | `audit::kubernetes_session::tests::s2_confirmation_tests::s2_confirmation_cannot_cross_from_the_authenticated_tenant_to_another_store` | R1 FAIL; R2 PASS |

R1 build RC0: 19:42:46–19:45:05 UTC. R2 build RC0:
19:53:45–19:57:59 UTC; R2 parent 06 RC0: 19:59:04–19:59:05 UTC.
`cargo check --all-targets` on R2 returned RC0 at 20:00:10–20:01:52 UTC,
with 11 warnings, including four duplicates. These checks do not establish
general Clippy cleanliness or execution of every listed test.

Independent source reviews returned bounded GO_SOURCE:
`7fcf8defd351355ec20bde541b70b17891dc471f61d1ea1a2242b73955453a46`
for the thirteen composed paths, then
`cdec40ed45205f3de36b455b318634c422cd99800d66a8e72802f8582d213e9e`
for the R2 test-only correction. They are not runtime approvals.

## Post-source, export and QA retirement

Root verified all 2384 R2 source hashes, exact pathset, zero nonregular source
entries and the four admitted cache files. Actual regular-only source exports:

- R1: `bf0d1ce0b9a8ec35c418328a2f929e4c0d48c313f8541324a0b8669c4c3bf876`.
- R2: `7c33bf49566532a5cc95177334530fcd2ae7fb73790d6fa84a8ae0bc41efe70f`, 24555520 bytes.

The owned QA was idle with only PID1 sleep before the explicit stop, then was
removed normally; exact-container lookup was absent afterwards. Its exit137
belongs to that idle stop, not the tests. The watch later returned terminal0.
The closure record hashes to
`3f24c41ac9e807955049f358fcc5c17d91f3d33309ffb4622534408b37a8d9b9`.
Peak memory was 6575865856 bytes, OOM count0; scratch used
2057236480/4294967296 bytes and29745/131072 inodes. These observations are not
capacity or performance qualifications. Raw results, reviews, manifests and
actual exports remain private outside worktrees; do not add those files to Git.

## Remaining limits

The fourteen historical journal/bootstrap passes remain attached to their
historical source; they were not transferred or rerun by this lot. The
historical deadline RED remains `NOT_RUN`. The local software fixtures do not
qualify a real Kubernetes client, identity/namespace/PVC, admission/network
confinement, Arm/handoff/release, runtime observation, stop/reconciliation,
physical filesystem protections or quota settlement. An epoch retirement is
not a distributed grant-expiry measurement; A→empty B is not an active-worker
or bidirectional network test. No audit finding or global gate is closed.
