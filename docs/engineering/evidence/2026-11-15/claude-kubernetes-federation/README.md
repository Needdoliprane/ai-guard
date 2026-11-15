# Claude Kubernetes / federation: composed software validation

On 15 November 2026, the already composed code passed **19 targeted software tests** after a fresh offline Rust compilation. Tested commit: `24be056e6c9a3bd4242f0bac872150062863c49f`; tree: `d745eff3928bb5b3b23ad7b56f484df015e99a33`. Its `src/`, `tests/`, Cargo manifests/lock and Rust toolchain file match published `161e8ce` byte for byte. The in-progress Source17 composition is a separate, untested source.

| Group | Individual PASS | Scope |
|---|---:|---|
| Federation | 5 | Closed owned profile; authenticated reconciliation across rotation; journal reopen/supersession; shared publication gate |
| Broker outbox | 1 | V4 payload survives real journal command/lifecycle updates; direct V3 downgrade refused |
| Kubernetes attach | 6 | Bounded stream/frame handling, cancellation, deadlines and capacity with one HTTPS/TLS loopback peer and five in-memory duplex cases |
| Waiting stdio | 7 | Actual provider executable in its internal waiting mode: EOF, truncation, arguments, non-UTF-8 and correlated Ready |

## Commands and evidence

One fresh build, in an isolated Linux ARM64 Docker QA with network disabled:

```text
cargo test --frozen --offline --lib --test broker_outbox --test kubernetes_waiting_cli --no-run --message-format=json -j1
```

Nine fresh Cargo artifacts were observed: six normal executables and three test harnesses. Fourteen actual libtest lists establish 19 unique active selected names and no selected ignored test. Four direct test-harness invocations execute precisely those selections (one test thread per group). Individual output, process codes and verdict parsers all agree: 19 PASS, zero FAIL, unknown or uninvoked selected tests. The exact names and raw-log hashes are in [PROOFS.json](PROOFS.json). The five private listing-protocol probes run no test bodies and receive no product credit.

Root independently checked all 105 phase/command return-code files (zero), unchanged hashes/sizes/modes for all 2,888 source files, the exported source archive and fixture inventory. The fixture regular-file export is empty; metadata records eight directories (some contain an ipc subdirectory), with no regular files. A complete non-atomic process scan reads five of five processes, with no target observed. QA peak memory is 9,542,557,696 bytes against a 9,663,676,416-byte cgroup limit; OOM counters remain zero. These are QA resource observations, not worker sizing measurements. The exact QA container is stopped and removed; private raw evidence has a separate Root file/directory fsync receipt.

Attempt R1 is preserved as a preparation failure caused by stale cache-pin paths, before Cargo or any of the 19 test invocations. R2 fixes the QA paths and supplies the actual results above; it does not turn R1 into a pass.

## Limits

There is no real Kubernetes cluster or Docker/Podman worker in these tests. Provider/model calls, real OIDC, browser, multi-host deployment, 100-worker capacity, release/image builds and full-suite qualification are **NOT_RUN**. The waiting executable is the product's internal mode, not a vendor session. Existing all-targets/format evidence keeps its earlier scope and gets no new pass. Owned federation remains a supervision contract; this result does not open worker relay, remote commands, Source16/17 or fencing permissions. No audit fingerprint or production gate is closed: **0/12 gates closed**.
