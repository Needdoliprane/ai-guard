# Ingress SSE capacity — 10 November 2026

The controlled D3 exchange-retention defect has its software correction:
RED observed, independent source review, ten exact FIX parents PASS and
all-targets check RC0. This is not a production or general availability claim.
Other integration-worktree changes are excluded from the tested source.

## Exact sources

Public base `2e667898c94b6dab94abbbbec21ebbdf39997eb0` plus four test-only
paths. Baseline2345-file manifest:
`a3cf44b6581a3a81bda0c04dfda6795b7b28d623034382aa9bb89c7f5605a5b6`.
Actual Linux pathset and hashes verified before/after execution. Actual RED
source export:
`7452a2d2db96b532f5b0b35efb8bfc370d1815a18240aea1895bdcc8855814c8`.

FIX changes two Rust paths only: `src/ingress.rs` and the new field in an
existing `tests/ingress_admission/slow_uploads.rs` initializer. Full2345:
`28b2baf12cf8d430b1958bf6a4461393d67496826b5b49a8ae19d9900da88d7d`;
two-path delta manifest:
`7f2af2b6af58ee889d68ef94cc087cc4ed8b6d8c21daa77cf06e20731b6524a7`.
Actual post-FIX source export:
`c86d52be847b116e919731fa799ae79e4fc79cab1bac16bf96c11510519da509`.
The final local EN/FR manual paragraphs are a separately reviewed data-only
delta; they are not represented as having been compiled in this source gel.

## Before correction

Two exact API parents each PASS: retaining the real tenant4 or shared32 Read
permits causes a structured Capacity event and EOF after HTTP200, then an
explicit reconnect succeeds after release. These characterize the Read gates;
they do not identify the historical R4 failure or reproduce ingress D3.

The third parent is
`ingress::sse_recovery_tests::parked_authenticated_human_bodies_leave_unary_ingress_available_and_recover_after_drop`.
It selects1 and fails RC101 at the final `D3_POOL` oracle,07:48:31–37UTC.
All64 authenticated stream requests are accounted for across three pressure
tenants plus a separate victim. Real human response Bodies and leases remain
parked; explicitly synthetic fixture comments keep their transports active.
Both marker barriers, direct gateway200, the independent authenticated control
channel's actual SQLite receipt, recovered public200 and complete common
cleanup succeed first. During retention the public unary read instead returns
the exact ingress429. No worker or usage is created.

The paused Body schedule is synthetic: this is not a test of64 naturally
polling human SSEs. The earlier R4 heartbeat setup remains nonconclusive and
is not rewritten as a successful reproduction by these new tests.

RED stdout SHA:
`e53dd1601ee75c2b87eb50daf10db209bfa91567a9fbb144bdad35076f44209c`;
stderr:
`d7f7f0cd912bcfddca3d063f2b68a0765f64af437b1887829d9094c8bd7ad3c0`.

## Correction and FIX results

At most48 actual SSE responses retain slots within the unchanged64 exchange
cap. Content-Type comes from the fixed upstream, is checked before hop-header
removal and supports case/parameters. Request paths, Accept and claimed tenant
do not choose that budget. A full SSE budget returns the distinct429 message
`Trop de flux simultanés.`. Both permits belong to the existing Body/Pump lease
and follow its release/poison/cancellation/error paths. Upload16 and every
product timeout, destination and authorization rule remain unchanged.

Commands use the frozen offline cache and exact full test names as one argv:

```text
cargo test --frozen --offline --lib --no-run
cargo test --frozen --offline --lib EXACT_NAME -- --exact --nocapture --test-threads=1
cargo check --frozen --offline --all-targets
```

| FIX operation | Actual result |
|---|---|
| Lib no-run build |RC0,08:00:26–08:06:48UTC|
| D3_POOL parent |1 PASS,08:07:14–22UTC|
| Real HTTP media/budget/drop parent |1 PASS,08:07:43–44UTC|
| Six existing header/chunk/upload/EOF/body-lease parents |6 PASS, one exact selection each|
| Two existing upload-admission parents |2 PASS, one exact selection each|
| All-targets check |RC0,08:10:59–08:13:38UTC|
| Post-FIX manifest |RC0,2345 hashes,08:13:59–08:14:00UTC|

Root independently reconciled all ten selected names, RC0 and the individual
one-pass summaries. One canary prints fixture status between its name and
`ok`; an initial overly strict same-line parser was corrected, without a
test rerun or fabricated result. The two unchanged API characterization
parents stay attached to the baseline, not ten additional FIX tests.

FIX D3 stdout SHA:
`338b09400324814d8dd81d8b4e70b7bb80241292764da53bee3d1386c33d2b25`;
HTTP component stdout:
`6fcf2d28d2280ec086e18b1e100d5a21d5b75a1c9af1de033be49a698d870f4a`;
all-targets stderr:
`f35cd5963d8607400e1b0ae41dc69d4f7fa29368ca25299517a06d61493975fe`.
The65-entry FIX evidence manifest SHA is
`8feee578504244977a6ea8aca96321365bbe5754f653093f6e0edcdc9cf57c1f`.
Independent source review SHA750ac92f… and EN/FR paragraph review13ae442d…
are retained with the full private reports and frozen inputs outside WTs.

## Environment and limits

Linux ARM64, Rust/Cargo1.99.1 in the pinned disposable Docker QA; UID1000,
ROroot, capabilities0, NNP/seccomp, networknone, CPU6/RAM9GiB/swap0/PIDs128,
scratch3GiB/65536inodes. Only public inputs/cache mounts are RO; the extracted
scratch source is writable but fully verified before/after each phase. No
engine socket, home, user credential, provider or external IdP is mounted.
Stdout/stderr persist directly to the host from each command's start.

The first baseline build expires via automatic GNUtimeout900 (RC124), with
no test selected. Its host wall span exceeds the requested guest timer;
cause is not established. The unchanged source builds successfully with a
larger predeclared bound. No timeout is counted as a product RED. Later builds
and tests retain automatic TERM/KILL bounds and the original raw evidence.
A transient public-cache copy permission error was corrected only in the
QA's private copy before Cargo; source and pinned cache checks then passed.

This fix bounds labeled SSE retention, not every long response or pending
upstream-header exchange. It neither provides tenant stream fairness nor
general flood protection. No throughput, latency or100-worker claim follows.
**NOT_RUN**: natural64-SSE stability, worker Docker/Podman/Kubernetes matrix,
real provider/IdP, browser, physical crash, Clippy and global production gates.
Compiler warnings remain. G09 FAIL; zero global gates closed.
