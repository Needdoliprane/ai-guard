# Audit K — residual reproduction, 8 November 2026

Status at checkpoint82: **FAIL; candidate not integrated**. The later
[integrated component and its36 passing functions](../tenant-metadata-isolation/README.md)
provide the current bounded result. This is development evidence for
the residual work tracked in [AUDIT_FOLLOWUP](../../../AUDIT_FOLLOWUP.md#audit-final-run-1),
not a new external audit or a change to its thirteen findings.

## Source and execution

The private frozen source `audit-k-review-repro82` derives from `bc29d0c` plus
the uncommitted metadata-isolation candidate and its regression tests. Its full
source manifest has SHA-256
`e80849cdc932070e91034c915e4704a8b55cbafee3f3cc6da4d5c00635927276`.
It is distinct from the integration branch and from the later corrective work.

Luna ran, once, in QA AR from `/scratch/k82`:

```text
cargo test --offline --locked --lib audit::collision_isolation_tests -- --nocapture --test-threads=1
```

The pinned Linux Rust 1.99.1 toolchain used a separate Cargo target and an
offline public dependency cache. The container had no network except its
private loopback, no engine socket, no credentials, a read-only root and inputs,
UID 1000, no capabilities, no-new-privileges and seccomp. Limits were four CPUs,
8 GiB memory with no additional swap, 256 PIDs, 4 GiB/65,536 scratch inodes,
1,024 file descriptors and no core dumps. No provider or worker engine ran.

## Observed result

RC 101: **two PASS, five FAIL**, no ignored tests; the test phase took 0.19 s.
The complete parent log has SHA-256
`ba50738c8905ee56fdcfc75b129ededb8efd2aeefe6208a1a9995f65a207aec9`.

| Check | Result and interpretation |
|---|---|
| Changed metadata remains unavailable until source verification | PASS |
| Validated metadata can be read while an external SQLite writer holds its transaction | PASS |
| Neighbour mutex, lease and SQLite-writer cases | Three FAIL: unknown workload credential returned 503 rather than the fixture's required 401. This oracle must allow fail-closed 401 or 503 when the neighbour is unavailable; it must still require the authenticated tenant B operations to succeed and the unknown request body to remain unread. No production correction is credited from these failures. |
| Historical collision after sibling issue/rotation/revocation with a held lease | FAIL: 200 rather than required 503. The candidate's positive-binding cache shares the lease mutex and becomes unavailable under that lock. |
| Durable commit followed by injected descriptor failure, recovery and exact replay | FAIL: 200 rather than required 503. The candidate can lose publication of its committed positive-binding snapshot. The test injects an existing filesystem-check fault; it is not an actual OS descriptor-exhaustion campaign. |

The two collision failures reproduce the independent review's blockers. Their
503 assertions remain mandatory. The next correction must preserve historical
positive owners without using the cache to authenticate a credential or treating
an unavailable store as proof of absence. Own-store failures remain fail-closed.

The preceding `audit-k-review-repro81` did not execute these oracles: compilation
failed on a SQLite pragma read typed as `u64: FromSql`. Its source manifest is
`092274b6bf93ad467460ee31349885efd0524b3c806e8851dd526f190eb53ad6`;
parent log SHA-256
`14fcdcd9a602fe19b48627ead881e578cee2ed696c4f2bcd888750b32fc889d1`.
Only that integer-conversion error changed before reproduction82.

## Follow-up85 — seven regressions pass, one residual reproduced

The next candidate separates the positive cache from the lease with pinned
`arc-swap =1.9.4`, publishes after a successful SQLite commit before the fallible
pathname check, and reconstructs it during verified recovery. Offline Cargo
resolution added only that package and its root dependency; the existing
`rustversion` lock remained unchanged. Resolved lock SHA-256:
`9542b23f442f0b4216b2ca4937399a20bf85c57a4e8d9e4d3a09972670c31807`.

Frozen source85 manifest SHA-256:
`fb3eb3a244c92d10a4c47228874899e00b7879ccab91cdd7b7b61728255b6fc1`.
The same targeted Cargo command now selects eight tests. Its first execution
reached no functional oracle: all eight failed because the QA's configured
`/scratch/tmp` did not exist. That setup-invalid log is retained separately,
SHA-256 `4b2cd38cee6af49b7968f1ebb8882682ad0fe49de39e76b5f25e6451f11216e2`.

After creating the private temporary directory, one justified rerun of the
unchanged source and lock finished RC101: **seven PASS, one FAIL**, 0.27 s test
phase. Parent log SHA-256:
`b53beecb460217e326efa807201e1a654233c88b82f9199244cfac1dc1303013`.
Both prior collision regressions pass. The remaining regression receives 200
instead of required 503 after an independently opened Store commits a typed
binding and the gateway Store validates it through `integrity()`. That path
advances its validation markers without refreshing the positive cache.

The next frozen candidate87 corrects that path and adds a distinct regression
for dropping a checked read after validation of an external commit. Its test
and compatibility results are not implied by85. At checkpoint85 the candidate
remained unmerged; the later component proof records integration. This follow-up
does not close audit K or any production gate.

## Retention and limits at checkpoint82

Both frozen sources, manifests, parent logs and QA metadata are retained in
the private engineering checkpoint `audit-k-d5-checkpoint-20261108-0815`, with
fifteen verified SHA-256 entries. Logs are mode 0600. The stopped QA container
was removed after retention; no executable, database or credential was promoted
to the host. Its final inspection reports OOMKilled=false.

The corrected candidate, targeted compatibility tests, compilation of all
targets and a second independent review are **NOT_RUN** at this checkpoint.
There is no latency, real-runtime or whole-product qualification from this run.
