# Historical relay reconciliation — Core/API/SDK

Scope: the existing authenticated runner boundary, tenant Store and Rust SDK.
The read returns exact historical receipts, deterministic CAS/state resolution,
or an explicit unresolved outcome. It creates no reservation, claim, mutation,
execution capability or process-join proof. Namespace maintenance mode does not
replace credential authentication or queued authorization revalidation.

## Source and review

Public base: `ed3780506f07f809c98f32dbba282d4d162a4ecd`. Ten postimages were
composed, including the two-line internal module/visibility correction after
the first candidate failed to compile. The index matches all ten tested
postimages; the independent Full3 changes in `docs/api.md` stay outside it.
Independent source review and its FIXr2 addendum found no blocking defect.
API and relay guides EN/FR are updated in the same slice.

Frozen source: 2,457 regular files. Archive SHA-256:
`cccd2cb582f39501573c8cc1343b27aad3b52b556eb172dbf9e10c279c2bf688`.
Source manifest:
`05d54904cc59d2b844940eb4db630f04d602631bb10f55d0d5576db0bc4a1a22`.
The corrected candidate changes exactly two lines in two files relative to
GREENr1; its other 2,455 source payloads remain identical.

## Observed validation

- Baseline: one exact HTTP test returned RC101 for HTTP404 versus200 after
  a successful Reserved-channel control. The four historical tests are retained.
- GREENr1: **FAIL_BUILD**, E0433 in the receipt helper path; zero tests run.
  Its genuine post-run export and successful cleanup remain preserved.
- GREENr2: build RC0; **14 distinct tests PASS**, zero failed/ignored, each
  selected by exact binary name, between 13:28:56 and13:29:08 UTC.
- `cargo check --frozen --offline --all-targets`: RC0 at13:30:36 UTC.
- Source post-verification, actual pre-stop export and cleanup: RC0. The exact
  QA container was stopped and removed, without OOM.

The selection covers ten `runner::relay_admission::tests` parents, one
`control_transport::reconciliation_tests` parent and three relay-admission
parents from `worker_relay_api`. It includes receipt-first resolution, inactive
namespace/revocation distinctions, exact terminal absence, divergent responses,
the old Close(CAS1)/Reserved case and unchanged active claim/transition guards.

The build used:

```text
cargo test --frozen --offline --lib --test worker_relay_api --no-run --message-format=json
```

Tests ran directly from the resolved artifacts, once per exact selector, with
`--exact --nocapture --test-threads=1` and a60s external bound. Rust1.99.1 ran
offline from an empty allowlisted environment, jobs1, incremental0 and debug0.
LinuxARM64 QA used an immutable image, UID1000, network none with private
loopback, read-only root and public inputs, capabilities dropped, NNP/seccomp,
CPU4, RAM9GiB/swap0, PID128, file-size512MiB, nofile1024 and scratch4GiB with
131,072 inodes. No checkout, host home, credential or engine socket was mounted.
The build overlapped the independent broker candidate's all-targets check.

Genuine post-run archive: 26,654,720 bytes, SHA-256
`cd37c2436f03bc15f6669915c8c54db58df0ba6aa160f31a092df1dca1058f91`.
All 2,457 payloads match the input manifest. Private raw logs, selectors,
profile admission and Root verification remain in the engineering archive;
they are not distributed with the product.

## Limits

These are real Rust/SQLite/router/HTTP/SDK tests with synthetic runtime facts.
Broker consumption, full-prefix local history migration, crash recovery,
Docker/Podman/Kubernetes, physical multi-host behavior and workload confinement
remain separate work. No security-audit fingerprint or production gate closes
from this slice alone. See the [recovery register](../../../RELAY_RECOVERY_REVIEW_20261111.md).
