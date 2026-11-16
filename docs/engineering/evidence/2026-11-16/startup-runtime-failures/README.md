# Native17 startup and local Docker: observed failures, 16 November 2026

These are two separate validations on different pinned sources. Neither passes its complete acceptance case. No production gate or audit fingerprint is closed. The original logs, failed results and sources remain private and unchanged.

## Native17 diagnostic source R9

Source archive SHA-256: `510d45ab680f96348116df72487acb17a954c66dfca33a05b73ee72807cae9c2`; 3,404 files. This private diagnostic successor of R8 adds test-only instrumentation and fixes the two fixtures' directory mode to 0700. It is not the published integration tree and is not applied to the live session-control worktree.

Rust 1.99.1, native macOS ARM64, empty environment except PATH/HOME/TMPDIR/CARGO_HOME/CARGO_TARGET_DIR/LANG, locked public dependencies used offline. The libtest build returns RC 0 with a new project artifact (fresh:false), zero compiler errors and 728 warnings. Test profile: opt-level=0, debug=0, debug assertions and overflow checks enabled, codegen-units=256, incremental=false, one Cargo job. The binary SHA-256 is `0ec7e341950d06718e769a069e12704b74b081a1324d18c7e4a4ccf366e7147a`.

The build uses `cargo test --locked --offline --lib --no-run --message-format=json -j1` with those explicit profile settings. The real binary list contains 2,356 tests; only the following four non-ignored, uniquely listed cases are invoked, once each, using `--exact --test-threads=1 --show-output <FQN>`, with a 180-second observation limit per case.

| Exact test under the libtest module | Result | Observation |
|---|---|---|
| `audit::control17::broker_maintenance_backup::tests::complete_scans_and_copy_run_off_gate_then_exact_admission_refuses_foreign_source_and_backup_drift` | FAIL, RC 101 | Startup expires its unchanged two-second epoch; failed append span is 2,425 ms. |
| `audit::review_startup::execution17_startup::writer_tests17::native_startup_writer_keeps_exact_idempotence_and_real_separate_retention` | PASS, RC 0 | One successful writer observation. |
| `setup::native_profile17::public_control::tests::public_control_output_is_exclusive_private_and_retains_its_exact_data` | FAIL, RC 101 | After the directory mode correction: unsupported retained history bootstrap entry name. |
| `setup::native_profile17::public_control::tests::changed_original_public_control_fd_cannot_be_revalidated_as_same_output` | FAIL, RC 101 | Same production publication refusal, before the target FD assertion. |

The backup trace positively marks one completed COMMIT before post-COMMIT verification refuses. It must not be described as an operation that never committed. The writer contrast passes without a configured broker root; it does not replace the failed broker-root case. Nested durations overlap, unfinished spans are not successful durations, and observer overhead and host pressure are unknown. No SLO or speedup is established.

Root verified the four raw test/RC records, the retained binary, all 3,404 expected source contents/modes, and the unchanged 3,403-file live R8 source. R8's separate 75-case selection remains 41 PASS / 3 FAIL / 31 NOT_RUN; R9 duplicates are not added to that count. The broader configuration and session journeys remain open.

Private receipts (retained outside the checkout):

- `native-startup-measurement-native-qa-r9/EXECUTOR_RESULT.json`: `478a3acc76966b3198844a4cebb0918a2897cbc9d8ae82e6297155faa2a0d76b`.
- `native-startup-measurement-native-qa-r9/ROOT_VERIFIED_R1.json`: `dd207c7d70f2d6d3534430d28132a1d59d6f4c0fe1f3d22f98daeb6016f5eae3`.

## One real Docker generic-runtime case

The pinned private source has 2,910 files (archive `2d0b83cc3168fd95f815d16f0131af5e807425c5ec6ed4597ddd45a1c64e3fd8`). It composes the public local-image policy with `tests/runtime_local_confinement.rs`, SHA-256 `fdf39dd48ac8e7105803d91424c3ca609c34a4904cf753148667027dd02e2330`. The test is not yet a published regression. Binary SHA-256: `ae041a88b03983b4d74edcb5f314d03af07841fe628ea18ea4830aaa4045dfe9`.

One invocation only, using `--ignored --exact --nocapture --test-threads=1 real_local_engine_filtered_copy_confinement_timeout_and_export`, returns **FAIL / RC 101 after 523.12 seconds**, with `terminal timeout observation was too late`. There is no automatic rerun.

Observed engine: Docker Desktop 29.9.2, Linux ARM64, eight CPUs and 12,480,589,824 bytes of RAM. The chosen cached image is `sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`; no image pull or build occurs. This uses an explicitly admitted rootful Desktop VM and a worker configured as 1000:1000; it is not a rootless-engine qualification. The process environment contains the same six public QA keys plus the five explicit AGENTSANDBOX_TEST runtime/program/endpoint/image/rootful settings, and no provider credentials.

Ten sequential operations produce matching Created/profile/terminal/cleanup records: import, process, cgroups, readonly, secrets, routes4, routes6, network, mutate-copy, timeout. The original executor summary omitted cgroups and said nine; it remains unchanged, with a separate Root correction. Each observed profile has network=none, no source bind, capabilities dropped, no-new-privileges, read-only root, 1 GiB memory, two CPU cores and 256 PIDs. These observations precede a failed overall case.

The timeout request is one second against a 60-second sleep. Docker reports exit 137, no OOM, and 17.223701869 seconds between StartedAt and FinishedAt. The test's 30-second guard includes creation, inspection and cleanup as well as execution. It cannot by itself isolate stop-delivery latency. Deadline-arm and stop-delivery monotonic timestamps were not captured; Docker wall timestamps do not replace that oracle. Neither the product deadline nor the assertions have been relaxed.

All ten native terminal cleanups returned OK. Root's later read-only inventory finds the ten protected pre-existing containers in the same states and no container for this test session. The named workspace volume and all evidence are retained. No volume, user data or unrelated container is removed. **Export and complete.json are NOT_RUN/absent** because the timeout guard fails first.

Root receipt: `runtime-local-confinement-docker-r1/ROOT_VERIFIED_R1.json`, SHA-256 `8dc735f54e3488933e8f73ec078301421b432bd7f2b6d08197a27ff6506a6416`. It links the admission and raw logs and corrects the phase count without overwriting them. This is a generic runtime test receipt, not the broker's durable action audit.

## Follow-up and limits

Three independent corrections are in progress: reduce repeated broker-root decoding while retaining the original epoch and all verification boundaries; publish public control as DATA without widening HistoryBootstrap authority; and keep deadline/STOP pollable during retained runtime observation and callback work. Their software regressions and reviews must pass before claiming a correction.

Native17 full startup, dashboard session creation/instructions, broker durable audit/STOP, fencing and cold recovery, A1/A2/B1, providers, Podman, Kubernetes, restoration, supply chain and 100-worker capacity are not qualified here. These observations contribute to G02/G03/G04/G06/G09 investigation; **0/12 production gates are closed**.


## Successors R10 and R11: software results remain separate

R10 source a87491fe07b71981e4a6840ea1e9e441da731b5469956cc6f6e5ae67e00f0126 (3,406 files) composes the DATA publication fix and the scoped BrokerRoot candidate, with official formatting only. Its all-targets check fails with six libtest compilation errors; all 21 selected tests are NOT_RUN. The original source, diagnostics and receipt remain unchanged (executor fb89ee928aa6096dc40701a7c5bfd4c59b988305aa63f3ec46ad624d41b46f89).

R11 source fde62901c3c6c09ee660abe4dd95e9b17c98300524386b45bd77ed967bec22ad keeps those changes and repairs two caller/type contracts. The old unit Result<()> facade delegates to the unchanged failure policy; a genuine borrowed scope is added to an existing startup helper. That helper is not registered as a test and has no calling test parent; it is never counted as a PASS. Cargo.lock SHA-256 is 47bbcf729c118a3a65380ce9f3990f5049bb76dbd9a74ec67b816cc117166e24, distinct from the public legacy source.

On R11, targeted rustfmt on 11 files, all-targets check and libtest build pass. The new libtest artifact is fresh:false, SHA-256 be28f6f832b383be7062d3611bb70eeae8e0b4d7c040be0562d15339c2310373. The profile remains opt0/debug0 with assertions/overflow enabled, codegen256 and incremental=false. All 25 selected FQNs are verified against the actual active/ignored lists before invocation.

| R11 selection | Result | Scope |
|---|---|---|
| Eleven DATA tests and two existing FS canaries | 13 PASS | Real private files, exclusive publication, original FD/bytes/cause across refusal and retry, unchanged closed HistoryBootstrap. Fault injection follows real operations; not a crash/full-disk qualification. |
| Configured BrokerRoot startup positive | 1 FAIL, RC101 | During fixture construction before the retention callback; incarnation_append17 reports 2257 ms and human_authority_epoch_expired. Whole case 24.45 s. |
| Three startup mutations/expiry and three backup cases | 6 NOT_RUN | The configured positive prerequisite failed; no replay of that same missing prerequisite. |
| Writer without configured BrokerRoot and four Owner typed-failure canaries | 5 PASS | Independent paths; typed shortage tests are not a real descriptor-exhaustion campaign. |

Total: 18 PASS / 1 FAIL / 6 NOT_RUN, 19 invocations, no ignored case promoted. Clippy lib-only returns RC0 with 2,152 warning diagnostics; the all-targets check reports 2,359 warnings. This is not a strict lint gate. The legacy startup_tests.rs test module is outside that lib-only Clippy scope.

Root rereads the 19 raw test/RC records, the two Cargo JSON streams, the retained binary and all 3,406 expected source contents/modes. The writer case's observation tool omitted its yielded session handle; its running PID was observed and allowed to finish naturally, without restart or kill. Final output and RC0 establish that case's single PASS; this observer limitation remains recorded. Executor receipt 169dc1e9edd9d63155757a14cd3c9992ec5b260bd0ad3997faff05c0d961d2a8 and Root receipt a17c57e8cd3838474d9192f969325229854500de69dfbdbaf1d5f51d972b4a31 are retained under native-data-and-startup-native-qa-r11 outside the checkout.

The 2257 ms span includes the test-only finished17 observation after the scoped verifier has dropped. It is not a writer-only CPU measurement. This non-instrumented run does not independently mark COMMIT, so its outcome is not borrowed from R9. Source review establishes that a light scope path is used, but counts and cost attribution still require the separate probe. No speedup, hardware attribution or SLO is derived from these single observations. No R11 code is applied to the live R8 integration by this documentation. The newly source-reviewed empty-foreign-memory-writer test remains NOT_RUN; a compatible foreign schema and the deeper exact-main guard remain open.

## Runtime scheduling baseline: two expected final failures

A separate private source dac43a7722fb84422ea4111373ffb5d9ce79837bb28970ed133a8bce4fa0f544 (2,913 files, public 985daa1 plus a behavior-preserving extraction and two software oracles) compiles its libtest successfully. The extracted next_event helper is called by the generic runtime and retains the old await-inside-tick behavior. Two deterministic oneshot/manual-poll cases each fail once, RC101, on their final deadline/STOP pollability assertion AFTER release, completion and real drain. Compile/setup/timeout/zero-test failures are not used as evidence.

Actual binary SHA-256 is 1c52cd33fe5318f87d2a73429e9becb1586094ec1fd259f84a34b7b6a4d1d199; both selected names are present once and non-ignored. Source and binary are unchanged afterward. Root receipt c3af3027869d4ffd1d17819d296160d749547fd1ad028ef3f13b5d46a16ef12f and executor bef967d2686aa0850d769b79e0f088dfb8165a18e7c954c7d8a7687e1e3630d4 are retained under native-runtime-deadline-baseline-native-qa-r1. These are reproduced scheduling failures, not a product PASS or an attribution of all Docker 17.224 seconds to this one cause. The batch fix is in progress; interactive/prearmed paths and external caller cancellation remain explicitly separate. No real engine test is repeated here.

## Docker Desktop Kubernetes: read-only readiness snapshot

A separately recorded observation selects the real docker-desktop kubeconfig context and verifies its loopback server https://127.0.0.1:53145 without exposing credentials. kubectl client and kubelet are v1.36.1; /readyz returns ok. One node, desktop-control-plane, is Ready with 8 CPU and 12188076 Ki memory allocatable. StorageClasses hostpath and standard both report rancher.io/local-path and WaitForFirstConsumer. No workload or configuration is created or changed. Private receipt kubernetes-desktop-readiness-20261116-r2/RESULT.json SHA-256 94cbabd592bf1ac30a4a3619ebc4cc7d39a21980b961882e195cc8de56955e1f. This snapshot does not prove worker operation, RBAC, network policy, storage provisioning, isolation or capacity.

All previous observations remain attached to their own sources and profiles. Native17 startup, session creation/instructions, real runtime matrices and production gates remain open.
