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
