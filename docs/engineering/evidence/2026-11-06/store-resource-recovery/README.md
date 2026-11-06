# Store recovery after descriptor exhaustion

Source `6871634bf72afa27e521ceb8ee52059935cfba22`, base
`0843c41d5e71fcd0f31f3ace82974b6b93fd8c0f`, composition
`987898ec0b9ee1634b785eda855b8be56eedcf41`. Seven source/Cargo hashes match
the tested GREEN05 input. No dependency, schema, API or broker change.

## Accepted behavior

Only typed backing-file `EMFILE`/`ENFILE` suspend the shared live origin. The
current request fails without ACK. A later validating access checks a full
consistent snapshot before reopening it; `/healthz` alone does not recover it.
Unknown IO, corruption, changed identity and poisoned leases remain permanent.

After SQLite commit succeeds, the new floor/namespace are retained before
the names check. Capacity and activation wait for validation. A sibling can
recover, then the first connection replays the same event without another
row or debit. Activation comes from its private witness, never the schema.
Operating procedures: [EN](../../../../setup/local/README.md#audit-descriptor-recovery),
[FR](../../../../setup/local/README.fr.md#reprise-des-descripteurs-daudit).

## Observed results

| Input | Result |
|---|---|
| RED01, base plus first regression | RC101 at intended assertion: child EMFILE,121 FDs released, chain valid but Store unhealthy. |
| RED02, unchanged app plus test hooks |7 parents,4 PASS/3 FAIL on recovery; permanent-error/text/identity/poison controls pass. |
| GREEN04 | Compile FAIL E0277, SQLite getter u64; behavioral tests NOT_RUN. |
| GREEN05, getter changed to i64 with checked conversion |8 unit parents and2 real-FD parents PASS. One helper is ignored in the outer runner and invoked explicitly by each child parent. |
| GREEN05 all-targets check | RC0/59.78s,118 lib warnings,3 lib-test/2 shared TLS-fixture warnings. Clippy BLOCKED in image. |

Ten unique final parent functions; RED repetitions and child-helper executions
are not added. Unit build1m41s/tests0.22s, real-test build45.26s. These timings
are functional observations, not latency or capacity qualification.

Real children lower only their soft RLIMIT_NOFILE to128, observe EMFILE,
restore descriptors/limits before assertions. The second leaves one FD free
and checks the actual typed `openat` error. ENFILE/postcommit use explicit
test hooks; SQLite commits, chains, migrations and accounting remain real.

## Commands and environment

```sh
cargo test --offline --locked --lib audit::resource_tests:: -- --nocapture --test-threads=1
cargo test --offline --locked --test audit_fd_exhaustion -- --test-threads=1
cargo check --all-targets --locked --offline
```

Luna ran frozen RO sources in Linux ARM64/Rust1.99.1, pinned image
`sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`.
Empty environment with explicit tool paths, scratch-local HOME/TMPDIR/Cargo
cache/target, Cargo offline/incremental0/jobs1 and dev/test debug0. Separate
target per source. Nonroot1000, capabilities0, NNP/seccomp, private PID/IPC/
cgroup, no external network/socket/home/credential/port/device. RO source,
toolchain/root/public registry;2CPU/8GiB/swap0/256PIDs,3GiB scratch/65536inodes,
512MiB maximum file/core0/nofile1024, parent-controlled50-minute lifetime.
Each real child has30-second parent deadline and16KiB output limit.

## Retained evidence and review

Final input manifest SHA256
`2cab393e4ecd74d8a2aff81615c5c2f79d01ed719e37077aeeae8c57d3c95f3f`.
The private checkpoint outside the checkout retains source-only inputs and
parent-captured logs, no target database/binary/credential/wire dump.
Its15-payload manifest SHA256 is
`12359e754be7768c3dc7115b6368ce9aa67842352ffbf6dd0694b7ac740153d9`.

| Log | SHA256 |
|---|---|
| RED01 | `49ed4563fdf31897f790b79789b8b448779cdd77b96dd737ee95fc4eac36a521` |
| RED02 | `d64e6ead67aa0dfc5cfbf9964370797efbd6bd521060c8a90a0b145fa5bfd2fc` |
| GREEN04 compile | `8f2962f7613d71cc3011ef1d473c090c51f95cc7c204d2f86b32f455d6c42751` |
| GREEN05 units | `2bcd19a6039ef3a1205b688e1c26c7cf42c442320ecb348e1ca9586307ec40d1` |
| GREEN05 real FD | `896285e5c6dbfc7d6cfffeea2fd3fa7560ad28b62b871efbe9bed796433cf9f3` |
| GREEN05 check | `ccbb5eb15a060026304260806f95eabbe4d9c7745e2cf80a0647f58460f8ed95` |

Source archive SHA256
`5b7234315e7e0543aed0e07a236beee455f0905f99962b3f8ee2a40a6777fb39`;
patch `f542058a71f57a8e0809e1727d953e02e450ecc0b113908b03d0d32d69fd02fb`.
Independent source review found no blocking defect, then reviewed the
sibling-only debit/activation parent and guides. Root corrected the HTTP
wording:503 mapping after reaching a gateway route does not promise accepting
a connection or transmitting a response during FD exhaustion. Fmt, whitespace,
45 relative guide links and known-key scans pass.

**NOT_RUN:** actual ENFILE, macOS execution, TCP availability under shortage,
product workers/runtimes/providers, frontend/browser, physical storage faults
and100-worker capacity. External audit artifacts remain unchanged. G09 FAIL;
no global production gate is closed by this Store software result.

## Authenticated API coverage

Source `6fa1bcc`, composed in `4494760`, adds two test-only parents to the
published fix. Both pass on frozen API01: real gateway routes/authentication
and separate SQLite stores A/B, exercised with Tower. A read-only SQLite
observer sees the committed event before injecting the backing-file error.

EMFILE yields503 without a receipt; B retains its scope despite contradictory
tenant header/query. A's authenticated exact replay returns the original
receipt with one row and quota debit. After permanent EIO and removal of the
fault, three A routes remain refused while a fresh B POST succeeds.

Command: `cargo test --manifest-path /inputs/api-store-recovery01/Cargo.toml --lib --locked --offline audit::resource_tests::api_cases:: -- --nocapture --test-threads=1`.
Docker workdir was explicitly `/inputs/api-store-recovery01`. The earlier
invocation from `/workspace` failed before compilation and is retained as a
setup error, not a behavioral RED. Corrected run:2 PASS/530 filtered,0.09s,
build1m43s,3 lib-test warnings, no OOM. Same pinned sandbox/profile above,
with jobs1/dev+testdebug0 and a separate Cargo target; no provider calls.

Input manifest SHA256
`0a571bfa7e9e3fd2253ad80d2c3afa0dd2aa63e0c205448bb32dac5679cef315`;
raw PASS log `2d0cd7dedcedf63e9af47fedec5cfbd19381720d386601f1462adcc850b4f95e`;
setup log `75787515040b1e97cd1774d3cf8fd9570dc246a052fff1bd35617d3591938d79`.
Root verified nine hashes after testing/integration; independent review
confirmed the observer, failure phase, isolation and replay oracles.
The private API checkpoint retains source-only inputs and parent logs.

This is coverage of existing functionality, not a new feature. Faults are
injected and Tower opens no TCP connection. No actual worker, TCP availability,
provider or global production qualification is established.
