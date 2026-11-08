# Tenant identity metadata isolation — 8 November 2026

This component addresses the neighboring-store collision lookups in audit lot K,
on base `bc29d0ce651c45ec3d78e0d600c75a74f1e7730c`. The twelve anchors in
[source.sha256](source.sha256) match the tested gel88: ten Rust files and the
Cargo manifest/lock. The [audit register](../../../AUDIT_FOLLOWUP.md#audit-final-run-1)
retains the primary report and its scope. This evidence does not close the whole
lot, reclassify an independent finding or qualify a release gate.

## Behavior and publication

Neighboring identity metadata uses nonwaiting DB and origin-lease acquisition,
a SQLite deferred read transaction and a temporary zero busy timeout. The
previous timeout is restored. A changed, busy, suspended or unverifiable
snapshot read reports `Unavailable`; it is distinct from a checked `Absent`
result. A separate known-positive lookup can still preserve a collision.
These lookups do not acquire a neighboring tenant's read/discovery permit or
spend its expensive-read frequency budget. Global admission and the selected
source's checked Store operations remain bounded and fail closed.

A shared positive index preserves known local bindings and historical workload
digest owners while a neighboring DB mutex or lease is held. It cannot establish
absence or authenticate a source. A discovered source still undergoes its own
checked credential authentication and class checks. An unknown workload proof
may return HTTP 401 or HTTP 503 when metadata is unavailable; it never acquires
tenant authority or causes its request body to be read. Readable collisions,
including revoked/rotated workload owners, remain refused by the human/control
authentication guards with HTTP 503.

Publication follows the actual source of the data:

- Opening, recovery, `integrity()` and `begin_checked()` prepare positive
  metadata from the same verified snapshot. Previously committed ownership is
  published before the corresponding validation markers advance. Dropping a
  checked read without a later commit cannot discard that knowledge.
- A credential mutation prepares its replacement index within its real SQL
  transaction. It publishes only after COMMIT, before a later fallible
  filesystem observation. This publication supplies no acknowledgement or
  execution authority: a descriptor shortage still suspends the source until
  full recovery. Recovery reconstructs the verified positives before clearing
  suspension; exact replay adds no second event.
- Sibling connections share one origin/index. An independently opened Store
  on the same backing has a distinct origin; its typed mutations become known
  to the gateway through the gateway's own verified snapshot.

The positive snapshot is outside the long-lived lease mutex. The pinned
`arc-swap = 1.9.4`, with default features disabled, provides atomic `Arc` snapshot
publication and `load_full()` reads without a local unsafe implementation or a
blocking cache-lock fallback. Its official API documents this read as lock-free
and wait-free; this is not a measured end-to-end latency bound.
[API](https://docs.rs/arc-swap/1.9.4/src/arc_swap/lib.rs.html),
[release and Rust-version policy](https://docs.rs/crate/arc-swap/1.9.4).
The branch-1 policy supports edition-2018-capable compilers without experimental
features; this component was tested with Rust 1.99.1. The official registry record
for 1.9.4 is non-yanked, published 7 November 2026, with crate SHA-256
`db4c94f57a570fce5d381be3289b3f79aef6332d72948fe82ab5772e423ccf92`.
[Registry source](https://raw.githubusercontent.com/rust-lang/crates.io-index/master/ar/c-/arc-swap).
The only newly resolved package is arc-swap; its rustversion dependency was
already locked. The lock was prepared separately in QA, without running Cargo
on the development host.

## Reproduction and review history

The nine final parents use real Axum/SQLite Stores, typed mutations and
channel-held locks. Completion is checked before releasing a neighbor's lock;
the 30-second harness deadline bounds cleanup rather than defining a hardware
performance target. Holders and request tasks are released/joined before the
oracle. The external-writer parent separately requires checked `Absent`
metadata under a real SQLite writer and timeout restoration: a zero busy-handler
counter alone would be insufficient after changing the busy timeout.

The filesystem fault is explicitly injected `EMFILE` at the existing test-only
postcommit boundary. Its fourth call and a separate SQLite connection confirm
that the real mutation committed before the error. It is not process-wide FD
exhaustion, real EIO/ENOSPC or a runtime failure campaign.

All outputs below remain private; their names and hashes preserve provenance.
Repeated parents are not added together as distinct product tests.

| Gel and output | Observed result | Output SHA-256 |
|---|---|---|
| 80, `k80-red-aq.log` | **FAIL / RED** 2/2: B waits for A's retained mutex; neighboring SQL lookup requests 9 conflicting write-lock attempts instead of 0. | `86cbe6b21f4cb693226f566296fa0415584d87044057754bf38dd47fb688e5b9` |
| 82, `k82-review-red-ar.log` | **FAIL** 2 PASS / 5 FAIL. Two functional review REDs return 200 instead of 503: known ownership behind a held lease, and committed ownership lost after the injected postcommit fault. The other three failures are unknown-proof status oracles, corrected to allow 401/503 without body polling. | `ba50738c8905ee56fdcfc75b129ededb8efd2aeefe6208a1a9995f65a207aec9` |
| 85, `k85-harness-fixed-as.log` | **FAIL / RED** 7 PASS / 1 FAIL: independent-origin ownership verified by `integrity()` is still missing from the gateway's positive index. | `b53beecb460217e326efa807201e1a654233c88b82f9199244cfac1dc1303013` |
| 87, `k87-checked-drop-red-as.log` | **FAIL / RED** 8 PASS / 1 FAIL: dropping a checked transaction after validation loses already committed external ownership; response 200 instead of 503. | `e3ef3291e540380175d5f1aed1e19a85db5489f29fae340a52bafa3db86a93e0` |
| 88, `k88-green-as.log` | **PASS** 9/9, 0 FAIL, 0 ignored; 0.31 s of tests, 1 min 58 s build, RC0. | `95a189f367dbc4f2a41c289dd11520aa42e64066da5ac78b0bf1154fea2e53a3` |

Invalid harness/build attempts are retained separately:

- 78 (`k78-red-aq.log`, `c9c60e32ebeea373bec2ce95d0d68393d0e7f27b4670ab036eb868330136748a`)
  instrumented the pre-redaction Store rather than the gateway's replacement
  sibling. Its mutex PASS is invalid; the external-SQLite failure is partial
  baseline evidence. The corrected fixture opens both Stores with the complete
  synthetic redactor and checks that redaction attachment retains their Arcs.
- 81 (`k81-review-red-ar.log`, `14fcdcd9a602fe19b48627ead881e578cee2ed696c4f2bcd888750b32fc889d1`)
  failed compilation because SQLite `FromSql` does not implement `u64`.
  No parent executed. The correction reads `i64` then converts it checked.
- The initial 85 setup (`k85-integrity-red-as.log`,
  `4b2cd38cee6af49b7968f1ebb8882682ad0fe49de39e76b5f25e6451f11216e2`)
  had no prepared temporary directory and reached no functional oracle.
  The host prepared private scratch home/tmp directories before rerunning the
  identical source and lock. No product timeout, stack or assertion was weakened.

Independent source review identified the lease, postcommit and validation-marker
publication gaps before acceptance. The corresponding REDs were executed before
their fixes. Final source review of 88 reports no new blocking issue within this
component; memory, retention and global qualification remain open.

## Validation environment and command

The exact final invocation from `/scratch/k88` was:

```text
cargo test --locked --offline --lib audit::collision_isolation_tests -- --nocapture --test-threads=1
```

Luna ran the gel in the separately admitted QA AS container, Rust 1.99.1 Linux
ARM64, UID/GID 1000, no capabilities, no-new-privileges and seccomp filtering.
Network was disabled with a private loopback; PID/IPC/cgroup namespaces were
private. Root, source inputs, registry and the public dependency cache were
read-only, with no credential, engine socket or published port. Bounds were
4 CPU, 8 GiB memory with no additional swap, 256 PIDs, 4 GiB private scratch with
65,536 inodes, 512 MiB per file, 1,024 FDs and zero core dumps. Scratch was writable,
executable, `nosuid,nodev`, UID 1000/mode 0700. Shared-memory and message-queue
mounts were read-only and bounded. Fixed host-clock deadlines were configured;
the observed lifecycle limits below restrict the timing claim.
This Docker QA container does not qualify the product's Docker worker adapter.

The complete private gel88 source manifest has SHA-256
`99249600ce3e06ee64ecd1113d6166f14a5319910e741cd3311d5832b8e95fa3`.
The admitted environment's inspection and preflight hashes are respectively
`e15f384c9678d17cd1685cc37a580ee5dee66f4b2dd773f1c59dc87bf6cf1bbc` and
`9b4e2fd7ec439e3eb21960258f919aadfd7f72c5000096c7b5e2958db2a24b2b`.
The twelve published component anchors match that complete source manifest.

The remaining targeted commands ran on the same gel88 and lock:

```text
cargo test --locked --offline --lib api::workload::tests -- --nocapture --test-threads=1
cargo test --locked --offline --lib api::read_execution_tests -- --nocapture --test-threads=1
cargo test --locked --offline --lib api::human::workload_collision_tests -- --nocapture --test-threads=1
cargo test --locked --offline --lib api::human::admission_tests -- --nocapture --test-threads=1
cargo test --locked --offline --test tenant_api -- --nocapture --test-threads=1
cargo test --locked --offline --test human_access credential_collisions_between_tenants_and_classes_never_select_a_tenant -- --nocapture --test-threads=1
cargo test --locked --offline --test broker_identity_api -- --nocapture --test-threads=1
cargo test --locked --offline --lib audit::resource_tests -- --test-threads=1
cargo check --all-targets --locked --offline
```

| Gel88 check | Observed result | Output SHA-256 |
|---|---|---|
| Workload API admission | PASS 2/2, 0.53 s | `e838066070ec00675c04b0c4b0405f3b03d1d41af6bcda0ee0f4716be7101f31` |
| Read execution/admission | PASS 4/4, 1.28 s | `3815d57511d65f80bbaa93b3fac3d557c4cd0007f0658406420425ab65dc328a` |
| Human/workload collision | PASS 2/2, 0.29 s | `7451f951e715937bdd48a4e32e05de6c963173e2845fd155a3e8ab200d6700c6` |
| Human source/context admission | PASS 1/1, 0.14 s | `c0facaed4fc1c005d5469df06cb0fd9dbc0d3cd35a41d7e40c5b3303583da33c` |
| Tenant API | PASS 4/4, 10.48 s | `486ac0f2e9410141a7c95a644e5248f9bd63020164d4955c10800023b7e7ea70` |
| Human-access collision filter | PASS 1/1, 0.11 s | `c716de0d4e488f8d9164cae2858686c9a4dd9cc6e616d3e7029b1c8d7d440b4b` |
| Broker identity API | PASS 3/3, 0.60 s | `6ea3d6607b179491446bdd9257083b23a8f275f0034b53b92acbee655cc01ed6` |
| Store resource recovery | PASS 10/10, 0.26 s | `ce0485d71636a5ceff0239a44450bba85c3b751861871a53ea9bfd0c63c777c4` |
| All-target compilation | PASS RC0, 1 min 06 s | `1aac3bd15a6dbc82b2492e1660f8ab17135ae8682330879e100a52a2c3910fe4` |

These runs cover **36 distinct passing functions**: nine component parents,
17 API/integration canaries and ten Store-resource canaries. No ignored test is
counted. The all-target check reports 61 library warnings, two warnings in
`human_sso_gateway_trust` and two library-test warnings. Rustfmt 1.99.1 on the
ten changed Rust files and `git diff --check` pass. Clippy on 88 is **NOT_RUN**;
the static-analysis gate remains open. D5 static checks were not run in this
component and acquire no result from these commands.

The profile configured fixed host-clock deadlines. The QA later exited 137 with
OOM=false; an unexplained UTC clock jump affects its lifecycle timestamps.
Neither those timestamps nor a nominal session window prove an elapsed-duration
guarantee. The source anchors, completed command outputs and their individual
durations above are the evidence used here.

## Limits

This component covers collision metadata availability and consistency. It does
not bound every tenant's own integrity scan, pending-review query, human context
projection or the complete request path under sustained load. It supplies no
new tenant selector, permission, HTTP route or user configuration.

The positive index retains local credential records and all workload digest
owners represented by the persisted historical tables, one snapshot per live
origin shared by its siblings. Publication may temporarily retain old snapshots
until readers release them. Its heap peak, reconstruction cost and latency under
large histories have not been measured; no independent RAM limit or calibrated
performance claim is established. Retention/archive and global audit capacity
remain separate open requirements.

Real OS-wide FD pressure, EIO/ENOSPC, crash/power-loss recovery, provider clients,
browser/SSO journeys, Docker/Podman workers, Kubernetes, multi-host operation and
capacity 100 are **NOT_RUN** for this component. Software QA and bounded source
review do not qualify production or close the primary audit findings.
