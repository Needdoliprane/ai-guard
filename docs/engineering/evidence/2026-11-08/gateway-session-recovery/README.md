# Gateway session recovery — software validation in progress

This record concerns an unintegrated S2 candidate, based on the integrated
S1/CLI11 sources. It does not qualify Kubernetes execution, a real worker,
provider, physical crash recovery or the complete product.

## First lifecycle attempt: setup failure

The immutable composition includes Core S2 r1, Auth r2, configuration r1,
Main lifecycle r2 and the independently reviewed late-panic test r2.
The application drain defect is deliberately unchanged for reproduction.

| Item | Observed identity |
|---|---|
| Source manifest, 2 324 files | `6b3c5f7d253ea93d4402b84490cc946da63b101fb5f5c6e23f394b371d529a30` |
| Cargo.lock | `3f34d8504293aa18ca1060b1c9d184904d3ce00fcc9c76c6f711f2603b1be4da` |
| Lifecycle test source | `97a936335e626e2c7ba6651357ddd63937a0b7f74eb1900c95fa5c9b4209c4bf` |
| Rust/Cargo | 1.99.1 |
| Selected / passed / failed / filtered | 1 / 0 / 1 / 710 |
| Cargo return code | 101 |

One command was run, from the verified private source working directory,
with an explicit, unique `CARGO_TARGET_DIR`, jobs=1, incremental=0 and
debug information disabled. No dependency download or lock edit occurred.

```sh
cargo test --locked --offline --lib gateway_server::tests::panic_during_grace_returns_error_only_after_connections_and_real_usage_drain -- --exact --test-threads=1 --nocapture
```

Compilation and test selection succeeded. The test then failed at
`gateway_server_tests.rs:27` while opening its fixture Store:

> private control directory requires effective-UID ownership and mode 0700; use a new private destination, existing paths are never repaired

This is **SETUP_FAIL**, before listener, SSE, authentication retirement,
Usage, commit, reopen or cleanup assertions. It is **not** the intended
application RED. No other S2 test or all-target check was executed.
The existing private-directory check remains required; the test fixture
must create the correct directory, without repairing an existing path or
weakening application checks.

The QA used a pinned local image, UID/GID 1000, network none, an empty
allowlisted execution environment, read-only root/toolchain/source inputs,
all capabilities dropped, no-new-privileges and seccomp. Limits were
4 CPUs, 8 GiB RAM with no swap, 256 PIDs, a 4 GiB scratch tmpfs with
65 536 inodes, bounded shared-memory mounts, file-size and descriptor
limits, and a 600-second command timeout. No host project execution,
engine socket, credential, model call, worker or cluster was admitted.

The parent watchdog stopped and removed this QA at 23:14:32 UTC on
8 November. Its handle returned RC0; the parent separately confirmed
the container no longer existed. Bounded text/JSON evidence remains
private outside the checkout. The disposable scratch and build output
were not retained or distributed.

## Fixture-corrected attempt: expected RED

Root independently reviewed a two-file fixture correction: new temporary
directories use `Builder.permissions(0700)`. Application code, assertions
and the 15-second harness / five-second product grace remain unchanged.

| Item | Observed identity |
|---|---|
| Corrected source manifest, 2 324 files | `92850e5adb1c7a77f1e2f78b0ead222d89dc1224ea45adf3c80a92dad7255709` |
| Lifecycle test source | `2571a4f0a84a97a0509e5b7909965778b355c11a6424b23747f6483fac726167` |
| Cargo.lock | unchanged `3f34d8504293aa18ca1060b1c9d184904d3ce00fcc9c76c6f711f2603b1be4da` |
| Selected / passed / failed / filtered | 1 / 0 / 1 / 710 |
| Cargo return code / test time | 101 / 5.05 seconds |

The same exact command above ran once in a new QA with the same isolation
and resource limits. Its own working directory, complete manifest, test,
lock, toolchain and unique Cargo target were recorded before execution.
It passes the fixture and reaches the intended final assertion:

> lifecycle B1: an internal connection panic during graceful drain must not become Ok

The test uses two real loopback acceptors, accepted TCP and SSE streams,
the real Auth bridge and retirement epoch, and a real blocking Store Usage.
Both listeners close before the intentional handler panic is released.
The remaining SSE survives the initial grace, then closes; HTTP tasks end
while the Usage still blocks completion. The Store head stays unchanged
until release, then commits, reopens and releases the last cleanup owner.
Only after those assertions does the incorrect `Ok` fail the final oracle.

This is the expected application **RED**, distinct from the earlier
fixture **SETUP_FAIL**. The application correction, GREEN, other S2 tests
and all-target check remain **NOT_RUN**. No original audit finding or
runtime qualification is closed by reproducing this candidate defect.

## Pre-fix routing gap (historical)

Source inspection of this same composition finds the POST session
control-recovery route in `gateway_base_router`, but absent from
`control_transport::control_route`. The gateway control gate and ingress
control relay therefore reject that path. Standard Compose configurations
point the control relay at the separate gateway control listener.

The public ingress path reads the body before gateway authentication;
a direct-router test cannot establish authentication before body reading
for that published path. A minimal classification change and targeted
control-entry regression are required. The dashboard remains read-only
for API proxying and does not gain runner mutations.

These are implementation and validation gaps. Neither source review,
successful compilation nor a corrected fixture closes a production gate.

## Targeted GREEN and integration

The reviewed drain fix retains the first internal JoinError in both drains;
only its own post-abort cancellations are expected. The exact v1 POST session
recovery route is added to the Runner allowlist. Other methods, aliases,
noncanonical IDs and roles remain closed.

| Selection | Source | Passed | Build / tests |
|---|---|---:|---|
| `--lib gateway_server::tests::` | r2 | 8 | 1m59s / 20.21s |
| `--test session_control_transport` | r2 | 4 | 57.40s / 0.61s |
| `--lib api::auth::control_recovery::tests::` | r3 | 12 | 1m53s / 0.85s |
| `--lib audit::kubernetes_session::control_recovery::tests::` | r3 | 8 | 0.16s / 3.68s |
| `--test kubernetes_session_control_recovery` | r3 | 6 | 54.32s / 0.34s |
| `--bin agentsandbox gateway_config::policy_tests::` | r3 | 10 | 4.17s / 0.08s |
| `--bin agentsandbox gateway_main_tests::` | r3 | 2 | 0.15s / 0.03s |

Each uses `cargo test --locked --offline` with
`-- --test-threads=1 --nocapture`, verified cwd/source/toolchain and explicit
unique target. All return RC0 with the stated counts, zero failed/ignored.
These are 50 unique targeted parents, not a whole suite or production gates.

| Record | SHA-256 |
|---|---|
| r2 source, 2 325 files | `d7cd234935a06f21accb9c7519f541fa742baa1472c50b79e0c483d78ba68dcd` |
| r3 source, 2 325 files | `6ab7eb135b4b62a91f4f9c7977aae68c87506f1f6ec605e48e9c8bc05d645f7d` |
| Final Auth test | `1d9232f8e4d50968af6baa5ff07f8f2ff3e9d336e6e430fe10ca61c868664956` |
| Root application patch | `0b8b7e6acc16d457405a809f8c470df7602bd524225d2664a2cd1131bdf7d11c` |
| Retained evidence manifest, 45 entries | `d832f7aef4b1f6edfdfbb927c2ed87004cf7fc18ff436737777ce1e58683e6d1` |
| Parent receipt-label correction | `7008323f7fdb7d039d06cc3052697fb746da34d5da1273e2d5e8ca63bf39f3b8` |

Only `api/control_recovery_auth_tests.rs` differs between r2/r3: a positive
0700 fixture and a revoked-binding oracle requiring exactly the existing
typed `Unauthorized`, refusing all other Err/Ok. Missing digests return
`Ok(None)`; known revoked bindings fail. No application or policy changed.
The 12 r2 PASS were not rerun on r3; their actual source remains explicit.

`cargo check --locked --offline --all-targets` on r3 returns RC0 in 1m06s.
Warnings remain: 80 lib, eight lib-test (four duplicates), two in
`human_sso_gateway_trust`; Clippy NOT_RUN here. Root compares integrated
`src/tests/Cargo` byte-for-byte with r3 without claiming a new Root test run.
Configuration, operator guides and API contracts are synchronized EN/FR.

Earlier 0700, HASH_C and PATH-before-Cargo failures remain nonconclusive.
Auth initially had 11 fixture failures and one incorrect test oracle.
Config10 had timeout600 instead of the then-required300; it finished in
4.17s plus 0.08s and was not repeated. Root adjusted only its soft scheduling
cutoff with shorter command bounds, retaining the environment hard deadline.
These incidents and the timeout configuration deviation remain documented.

Root observed only PID1 before closing QA
`b26547b8ee2a7c5981d6e640e1438cb086fb1cd328f41dd505811e03ca2da6f8`:
Exited, Running=false, ExitCode137, OOMKilled=false, finished
2026-11-09T00:09:22.430100846Z. Normal removal used no force or volume removal.
Peak memory7052521472 bytes, zero OOM counters. Forty-five bounded text/JSON
records were retained and independently hash-checked. Two receipt labels
(container name and SSO target) have a separate parent correction; actual
CID, sources, counts and outcomes are unchanged. No raw scratch, source tree,
DB or binary was promoted. The host backstop watcher94639 subsequently finishes
RC0 at 00:18:14 UTC; its probes find the already-removed CID, as expected after
the parent's earlier normal cleanup. No QA or watcher from this run remains active.

No real worker/runtime, physical crash/restore, network confinement, provider,
multi-host transport, 100-worker capacity, signed audit or production release
is qualified. S2 recovers centrally proven before-effect reservations only.
