# HTTP listener bounds — targeted native validation, 16 November 2026

The shared Rust listener owns accepted TCP capacity before routing and applies an absolute deadline to the first complete request headers. Dashboard, ingress and gateway entry points use 128 connections / 15 seconds; gateway public/control pools are separate. Existing authentication, body/exchange policies, retirement order and genuine usage/connection drains remain in their owners. These defaults are not a measured 100-worker capacity.

## Source and execution

The tested private source is public commit 985daa160100528a78b763ddc0d10ed18e320066 plus five reviewed code/test paths, then official rustfmt on the five touched files. Archive SHA-256: c0ec1d20f46eb8f9511fbcb34b74452b4d16a92a702b04a75719b4cddab83f5b, 2,913 files; post-format source index 81aaf77c2f800931e2b7024954682f9cdcf1e41e714f28050a23be72e0cf7314. The publication is based on 3b17f5b and additionally carries documentation; the executable inputs are compared to the tested source, rather than claiming equal complete trees. Native17's separate parent overlay is not composed or qualified here.

Luna executes natively on macOS ARM64 with Rust 1.99.1, an empty environment plus PATH/HOME/TMPDIR/CARGO_HOME/CARGO_TARGET_DIR/LANG, private HOME/cache/target, locked public dependencies and Cargo offline. opt-level=0/debug=0, assertions and overflow checks enabled, codegen-units=256, incremental=false, two jobs. The initial rustfmt check finds differences; only the five owned paths are formatted in a private copy, then a new pinned archive is admitted. No failed initial check is rewritten.

| Check | Observed result |
|---|---|
| rustfmt check on five touched Rust files | PASS, RC0 on the new source |
| cargo check --all-targets --locked --offline --message-format=json -j2 | PASS, RC0, no compiler errors |
| cargo test --lib --no-run --locked --offline --message-format=json -j2 | PASS, RC0, fresh:false libtest artifact |
| cargo test --test http_listener_bounds --no-run --locked --offline --message-format=json -j2 | PASS, RC0, fresh:false integration artifact |
| Actual test lists and ignored lists | Eleven selected FQNs present once each, all active |
| Exact invocations, serial, once each | Eleven PASS, zero FAIL/ignored |
| cargo clippy --lib --test http_listener_bounds --no-deps --locked --offline -j2 | RC 0, 248 warnings outside the five modified files; no strict-Clippy claim |
| Postcheck | All 2,913 expected source names/contents/modes and both executable hashes unchanged |

The exact selection and binary identities are in [TESTS.json](TESTS.json). Each test executable uses --exact --test-threads=1 --show-output and a 180-second observation budget. Six integration cases cover invalid policy, capacity through keepalive, incomplete initial headers, a legitimate second request, graceful response/connection drain and a live streaming body. One private IO case covers capacity ownership across a cancelled join waiter. Four existing gateway cases preserve retirement before stop, blocking-usage/SQLite drain, caller cancellation and panic cleanup. Tests use at most two loopback clients; there is no flood or descriptor-exhaustion campaign.

Private immutable receipts outside the checkout:

- Executor: http-listener-bounds-native-qa-r1/evidence/admission-r2/RESULTS.json, SHA-256 54942a35f1c8f912b2c555e6388aef43c894257cb13f991626ffdd41d01b6c51.
- Root verification: same directory/ROOT_VERIFIED_R1.json, SHA-256 90997587a778e331205a92b474ff94a9b320f78c8f9a97bf9a7989868bb58f42.
- Source reviews: HTTP package 269cc88c161117c58433b9914a2625e8541c34039a1472f9b0bdafc6f941a7b1, final independent review b7e365423fa90293538ff24ff8b964df550e65bfa40cdfa0c0389b21600e72f7.

## Audit trace and remaining limits

Primary records were reread without modification: November 8 NV-07, fingerprint http:pre-request-admission:unbounded-raw-connections (findings SHA 3b27a6fe0f4ce8f8f5856514f7dfb836f2161ab267473dfe04d78dcbed3df212), and run-2 V4, fingerprint ingress/listener-no-connection-cap-or-header-timeout (findings SHA b099e15dbddcf0b96047f8a04debda02e6fed0b6c0221411c2d444612760baa0). Both primary verdicts remain needs_validation. Absence of these controls was checked against the old code; no adverse baseline, exhaustion or cross-principal availability effect was executed or claimed. The new bounds are observed through the shared real listener code in the targeted tests only.

The first header deadline includes protocol detection. Hyper supplies later HTTP/1 header deadlines; new HTTP/2 streams after the first request remain outside this control. Body/SSE policies remain separate; a completed initial head does not impose a lifetime limit on a streaming response. The IO owns its lease, including an upgrade, but no complete deployed upgrade route is qualified here.

No production service is restarted, no model/provider/engine/cluster/PKI is invoked, and no frontend build is repeated because frontend code is unchanged. Exposure, proxy/platform limits, process descriptor/memory budgets, workload contention, 100 workers, other platforms and Native17 composition remain unqualified. This contributes to G08; 0/12 production gates are closed.
