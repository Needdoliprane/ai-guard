# Human worker/broker registry composition — 7 November 2026

Status: **19 distinct software functions PASS; all-target compilation RC0** on
`federation-fix03`. Seven functions are new: five integration checks and two
unit checks. The other twelve are existing worker-registry regression checks.
This qualifies the named software checks, not a frontend view, remote brokers,
a real worker runtime, capacity100 or a production release.

## Source, contract and review

Base: `0713344eca8e2aa3d59d2bce8bb1ccd28844b104`. Root froze the read-only
`federation-fix03` input and assigned its own target
`/scratch/target-federation-fix03-ad`. The original eight-entry manifest has
SHA256 `da4cebe1e85c5fe5edb2031128b7633f613ad9a1f451bf6543065462c43f55a3`.
This documentation pass checked all eight entries against the worktree: 8/8
match. The first eight entries of [source.sha256](source.sha256) reproduce
those Cargo/Rust/test anchors; Cargo.toml and Cargo.lock are unchanged dependency
anchors. The last three entries identify the API and English/French guides
changed since the base and clarified after the runs. Neither the prose nor this
evidence page is counted as an executed test.

The additive `GET /api/v1/human/federation/workers` requires administration
and both `read_worker_registry` and `read_broker_registry` before either
inventory read. It reauthenticates and checks both rights with the original
context/authorization version immediately before publication. Each existing
Store reader keeps its own checked transaction, receipt and durable human
tenant/principal budgets. A later refusal publishes no composite and does not
erase an earlier receipt. The old `/human/workers` permission and descriptive
associations remain unchanged.

The closed response declares `independent_snapshots`, `coverage:partial` and
`runtime_inventory:unknown`. Its workers page retains `snapshot_per_page`; its
brokers page retains `live_keyset`. Cursor domains, limits and positions are
independent. Each page remains bounded to256KiB; the aggregate bound derives
from the actual shared constant as `2 * HUMAN_MAX_PAGE_BYTES + 4096`, currently
528384bytes. No total, complete inventory or execution/incarnation proof is
inferred. The [API contract](../../../../api.md#composition-humaine-workersbrokers--human-federation-registry-composition)
and guides [EN](../../../../setup/identity/WORKERS.md#compose-worker-and-broker-pages)/[FR](../../../../setup/identity/WORKERS.fr.md#composer-les-pages-workers-et-brokers)
describe the same contract. Root reports the independent source review accepted
this bounded implementation; that review does not replace execution evidence.

## RED, oracle correction and GREEN

`federation-repro01` executed the exact composition function before the route
existed. It reached the discriminating assertion **HTTP404 instead of200**:
0 passed,1 failed,16 filtered out, test duration0.28s, build1min46s, RC101.
The retained RED log includes the invocation and isolated execution settings.
Its failure establishes the absent additive route, not a runtime defect.

Before GREEN, Root corrected a test oracle to preserve the existing per-page
wire values: workers `snapshot_per_page`, brokers `live_keyset`. The original
RED stops earlier at404 and remains valid. `federation-fix02` was frozen but
**NOT_RUN**; no test failure or PASS is attributed to that gel. Production code
was not changed by this oracle correction. The corrected test source is
`e118e4f22202553f1bfacd06c119a22138f4725a0a28b65f04c8ef83e19584dc`.

Luna ran GREEN in QA AD. The logs identify source, target, selections, test
names, counts and build/test durations; their headers are parent capture
metadata, not full historical argv. The following are **equivalent reproduction
commands** for the admitted isolated environment, not commands executed by this
documentation pass. Use the frozen manifest with `cargo test --offline --locked`
and the selections below, followed by `-- --test-threads=1 --nocapture`.

| Selection | Result | Test duration | Build time | Log key |
|---|---|---:|---:|---|
| `--test human_workers_api` | 17 PASS:5 new +12 historical | 5.14s | 1min46s | integration |
| `--lib api::human::workers::federation_tests::` | 2 new PASS;584 filtered out | 0.44s | 58.91s | unit |
| `cargo check --offline --locked --all-targets` | RC0; no tests added | — | 1min05s | check |

All selections use `--manifest-path /inputs/federation-fix03/Cargo.toml`.
No ignored tests are credited. RED and GREEN invocations of the same function,
loops within a test and compilation are not additional functions.

The five new integration functions cover two brokers/A1–A2 in tenant A, B1 in
tenant B, a registered broker without a worker row, revoked metadata without a
fabricated terminal/quota release, closed response and two separate receipts;
both capabilities before either read; invalid/authentication/tenant inputs;
cursor separation by route, leg, limit and context; and second-read budget
failure with no partial publication. The two unit functions cover the exact
dashboard proxy allowlist path and causal revocation of **each** capability
after both durable receipts, using an independent real SQLite handle. That
revocation loop remains one function. Router/HTTP/SQLite and authentication are
real software components; worker observations and identities are synthetic.

Warnings remain in the actual output:119 library warnings for integration and
all-target compilation, two `human_sso_gateway_trust` warnings during the latter,
and three library-test warnings for unit/all-target compilation. They concern
unused declarations, including held-create/recovery code and SSO fixture
helpers; no warning in these logs points to the changed federation files.
The three unit warnings name `HeldCreateClaimAttempt::begin`, the
`LiveHeldCreateClaimAck.identity` field and `PendingHeldCreateClaim::begin`.
No warning-free build, Clippy PASS or static-analysis gate closure is claimed.

## Retained artifacts

Raw outputs are retained privately as the following basenames. This pass read
their result/warning sections and independently verified their SHA256 values;
no raw database, credential or runtime artifact is distributed with this page.

| Artifact | SHA256 |
|---|---|
| `federation-repro01-ac.log` | `7e08a80e1480aad660bfc2eabc65508d3f80252e46c09d20f5a53663c40f8df0` |
| `federation-fix03-integration-ad.log` | `b87672d7104fba8cee2902124374d8eec17a7c4582812bed97500f213a04b4a8` |
| `federation-fix03-unit-ad.log` | `ed64e0e403c26e521184adfe37d3243093df12b11fac53d0b9814730032606cd` |
| `federation-fix03-check-ad.log` | `7541e705ac1711f2c84b9dd78518c07d37299cc560c723be0dd78caf3c69ccea` |
| `qa-ad-inspect.json` | `6b994d50362e8f50056ce0c676a3d85ab0b5e7c68bb43ce3549075e755ab1d43` |

## Environment and limits of the evidence

All three GREEN log headers identify QA AD container
`a249cc050ac1e253781444070f789bbd2cbf3cede30dcb2547baf59a7d5b8b79`.
The retained inspection, read for this page, identifies Linux platform and image
`sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`,
UID/GID1000, network`none`, no published ports, read-only root/source/registry,
all capabilities dropped, nonprivileged mode and `no-new-privileges`. Configured
limits are4CPU,8GiB RAM with no swap,256PIDs,4GiB/65536-inode scratch,512MiB per
file,1024FDs and no core dumps. These are configured validation limits, not
measured application needs or qualified worker confinement. The repository pins
Rust1.99.1; this pass did not execute a toolchain/version or kernel probe.

Root's validation profile specifies empty-environment, offline execution. The
inspection records an `env -i` entrypoint with fixed PATH; it is configuration
evidence, not a fresh inspection of each Cargo process environment. No new
container, engine call, Cargo command, test, provider request or network access
was performed in this documentation pass.

The source checks do not qualify a rendered frontend, browser/local/OIDC
deployment journey, authenticated remote-broker transport, real disconnection
and recovery, Docker/Podman/Kubernetes workers, personal ownership, resource
measurement,100-worker capacity or a cloud release. Unseen/revoked fixture
metadata is not a real remote-disconnection experiment. Runtime inventory and
consumption remain unknown, allocation remains nonbillable, and no global
[production gate](../../../PRODUCTION_READINESS.md) is closed by these results.
