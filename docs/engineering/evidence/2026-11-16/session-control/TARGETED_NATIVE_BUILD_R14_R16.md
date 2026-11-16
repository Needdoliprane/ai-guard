# Targeted native test builds R14–R16

These are software build results, not runtime or production qualification. All three attempts stopped before a libtest binary was emitted or listed. **For these Linux attempts, all 12 selected tests are NOT_RUN.**

| Attempt | Source archive SHA-256 | Relevant delta | Actual result | Executed tests |
|---|---|---|---|---|
| R14 | `be3c991b9642deba07779f014701e151f2b114934e1625a9ebf0e2a60e0ea5f8` | Exact R13 source; first libtest build | Cargo 101, rustc SIGKILL, cgroup OOM | 0/12 |
| R15 | `be3c991b9642deba07779f014701e151f2b114934e1625a9ebf0e2a60e0ea5f8` | Explicit 256 codegen units for dev/test | Cargo 101, rustc SIGKILL, cgroup OOM | 0/12 |
| R16 | `92bf19d91ac5ca2b2e86cd15b9b03a5a2daa79acabed78b9420884b0aecfd52e` | Three reviewed cfg(test) fixture setup extractions, preserving native callbacks and assertions | Cargo 101, rustc SIGKILL, cgroup OOM | 0/12 |

Each attempt used one `cargo test --lib --no-run --frozen --offline --message-format=json -j1` invocation in Linux ARM64 QA. Rust/Cargo 1.99.1;4 CPU;10 GiB resident-memory limit;5 GiB scratch tmpfs. Incremental compilation, debug information and optimization were disabled; debug assertions and overflow checks remained enabled. R14/R15 contain 3321 source files; R16 also contains 3321 files. The selected cases were three real backup/recovery tests and nine diagnostic closure/debt tests, with identical names, groups and order across attempts.

The10 GiB cgroup peak and increased oom/oom_kill counters establish the resource failure. Each build emitted 656 warnings, zero coded compiler errors and one unsuccessful build-finished record; this is **not** a successful compilation. Source/cache postchecks passed. Raw evidence was captured, while the collector deliberately reported FAIL for the OOM; capture success is not test success. Root independently verified the raw archives and exact-container removal.

| Attempt | Raw evidence archive SHA-256 | Root verification receipt SHA-256 |
|---|---|---|
| R14 | `feb14bedaf6a121456d27fc28813f273d7ca609c61b65ba8a4b73d76c4ffe154` | `b799a47db64eafc47021cd8a52c6c5e1408c673999fc7ebcea216ba1d67c276f` |
| R15 | `fcb0efea26d15e5667d67e4dcf74295fe7b61540dead8004991f7ff927d5c5f2` | `b97fab87ed374a7660de5b15c1325bbd0c7fe167bf4cd49fe699315f3f81456c` |
| R16 | `bc1a771f254ef61bfc730b39cb9131c568e09ba485c2fe683917a96581239530` | `e972cf6e8327ce30b566c33a72d228d9cd696070e1d8950b5273c864f11782f6` |

The initial R14 host marker incorrectly said build NOT_RUN; its immutable raw Cargo output and Root erratum record the actual failed build. The terminal R15/R16 records retain the distinction correctly. Private raw evidence and original failed records remain preserved; no persistent data or volume was removed to change a result.

The maintainer subsequently authorized native macOS tests without Docker. A separate native validation starts from the exact R16 source with a private environment and target, locked dependencies and offline build. Its outcomes must be recorded separately; none is inferred here. This avoids repeating the same 10 GiB container limit, without claiming that the cause or required memory has been established outside that limit.

The derived QA tools image also passed six version-only commands: rustc/cargo 1.99.1, rustfmt 1.10.0 and Clippy 0.1.99. This verifies tool availability only. Project formatting, Clippy, the twelve behavioral tests, provider calls and real Docker/Podman/Kubernetes qualification remain uncredited by this record. **No production gate is closed.**

## Native macOS R16 successor — actually executed

On the exact R16 source archive `92bf19d9…`, Rust/Cargo 1.99.1 Apple ARM64 built libtest successfully in 239.591 s: RC 0,986 valid JSON records, zero compiler errors and 658 warnings. Root independently verified the binary SHA-256 `e225631ba089ae6c5289f0742e1eb0299d8c556c4139f67d95e52915a44c2c02`, its listing and all 3321 source files after execution. The target, HOME, CARGO_HOME and TMPDIR were private; this was native execution, not an OS security sandbox. The build was frozen/offline.

All 12 names were each run once with `--exact --test-threads=1`: **9 diagnostic tests PASS; 3 backup tests FAIL**, no ignored test, timeout or retry. The three failures return `human_authority_epoch_expired` during `execution17_startup` phase `union_redactor17`, before their intended backup oracle. Their root cause remains under investigation; this record does not infer a broken backup guarantee or dismiss the failure as host pressure. The 9 diagnostic cases establish the targeted cancellation/closure/debt behavior only; their erased-admission fixture does not qualify a real runtime permit or provider.

Root verification receipt SHA-256: `90300e3d88fa6990a69133f3db2d78a5cbb601171b195bd81bcf6c6a007ad5fc`. The raw Cargo JSON, listing and 12 individual stdout/stderr/RC records remain private and retained.

## New Config/E2 composition — compilation failure

The subsequent 3393-file source archive `2faffe97ea17bd32d218e092dae19e68a2c55dd445ec02704006c9bbaae36a95` adds 163 reviewed configuration/authorization/causal-history paths to R16. Its native `cargo check --all-targets --frozen --offline --message-format=json -j1` is **FAIL, RC 101: 40 compiler errors and 71 warnings**. Missing imports, missing methods and visibility/type/ownership mismatches remain to repair. The 55 newly indexed tests are NOT_RUN; no libtest build was attempted after the failed check. An earlier isolated cache-layout failure is retained separately as a harness defect, with no compiler result credited.

Root verification receipt SHA-256: `3836a91e7442dfc52d4da3e8807e3f635e5b732386206c520db461961ec10517`. The source-only reviews did not prove compilation. The R16 build and 9 test PASS above are not transferred to this changed source, the public branch or any runtime/provider. No production gate is closed.
