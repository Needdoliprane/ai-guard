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

## Routing boundary still open

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
