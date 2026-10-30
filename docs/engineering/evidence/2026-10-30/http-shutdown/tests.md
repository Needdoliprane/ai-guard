# HTTP/SSE shutdown boundedness: RED baseline

## Scope and setup

- New test file only: `tests/http_shutdown.rs` (SHA-256 `3cb5567a394e24eff226b154223bda7434cc8b15aa22c82902f113782a21e9ce`).
- Frozen behavior under test: `src/main.rs` SHA-256 `dfc772e81dd26a19de7c842f834c38010894068ae10373fc21bc1282212f2822`, matching the assigned pre-change baseline.
- Both scenarios start the actual gateway/dashboard executable as Rust-owned children with `kill_on_drop`, loopback ephemeral ports, a private SQLite audit database, a minimal private `index.html`, and only synthetic 48-character `v`/`r` credentials. Child environments are cleared of shell variables. No `.env`, real credential, provider, model, Docker, or browser was accessed.

## Initial RED result

Command: `CARGO_INCREMENTAL=0 cargo test --locked --test http_shutdown -- --nocapture` (with the configured Rust toolchain PATH).

Result: **exit 101**, 1 PASS / 1 FAIL across two top-level tests:

- `gateway_and_dashboard_without_streams_exit_promptly_after_sigterm`: PASS.
- `gateway_and_dashboard_drain_open_sse_then_exit_within_eight_seconds`: FAIL for both services independently. The dashboard did not exit within 8 seconds; the harness killed and reaped only that child, then continued. The gateway likewise did not exit within 8 seconds and was independently killed/reaped. The still-open direct gateway and dashboard SSE connections delivered a previously queued synthetic event after SIGTERM while the services were draining.

The 8-second bound includes the expected 5-second nominal drain plus 3 seconds of CI margin. The RED reproduces the unbounded wait with active SSE and demonstrates prompt idle shutdown. It is a focused SIGTERM/SSE contract test, not a claim about keep-alive handling, global I/O stalls, or OS scheduling bounds.

## Evidence

Private evidence directory `/tmp/ai-guard-http-shutdown.rR1hlb/` is mode 700. The final behavioral log and source manifests are mode 600:

- `red-final.log` and `red-final.exit`
- `source-before-final-red.sha256`, `source-after-final-red.sha256`
- `source-before.sha256` preserves the earlier initial test hash record; `red.log` records an initial harness compile error, not a behavioral RED.
- `red-behavior.log` records an environment PATH/linker setup failure, also not a behavioral RED.

The final RED run had no source changes during execution. Only `tests/http_shutdown.rs` was added; `src/main.rs` stayed at the baseline hash above.

## Strengthened RED confirmation

The initial RED above is preserved unchanged. Before any core edit, the harness was strengthened to remove the SSE client's total timeout (only a 2s connect timeout remains; short health, append, and chunk waits are separately bounded). The active-stream test now leaves both SSE responses open for 6 seconds before SIGTERM, then rechecks health and confirms both child processes remain alive. The SSE stream holds no self-imposed 8s/12s lifetime that could create a false PASS.

The strengthened command was again `CARGO_INCREMENTAL=0 cargo test --locked --test http_shutdown -- --nocapture`, exit 101: idle shutdown PASS; active SSE shutdown FAILs independently for dashboard and gateway at 8 seconds, each with targeted child cleanup successful. The run took 24.14s. `src/main.rs` remained `dfc772e81dd26a19de7c842f834c38010894068ae10373fc21bc1282212f2822`; reinforced test source SHA-256 is recorded in `source-after-reinforced-red-final.sha256` in the private evidence directory. The corrected harness did not change the original RED evidence.

## GREEN and focused regressions

Core freeze used for GREEN: `src/main.rs` SHA-256 `48d73be68241017ef27ce36fd52a4ede72920f5f61ea0c7310fce4cd196893c1`. The test was adjusted to anchor the 8-second active-stream and 3-second idle limits at each SIGTERM instant; completion durations are emitted by the test. No production source was changed by this test owner.

- `CARGO_INCREMENTAL=0 cargo test --locked --test http_shutdown -- --nocapture`: **PASS**, 2/2 top-level tests, 0 ignored. Active SSE: dashboard 5006 ms, gateway 5008 ms from SIGTERM to exit. Idle: dashboard 0 ms, gateway 2 ms. The open SSE response objects delivered their prepared events during their respective drain windows.
- `CARGO_INCREMENTAL=0 cargo test --locked --test ingress_cli --test gateway_tenancy`: **PASS**, `ingress_cli` 2/2 and `gateway_tenancy` 1/1; 3 top-level tests, 0 ignored.
- `cargo fmt --all -- --check`: **PASS**.
- `cargo clippy --locked --all-targets -- -D warnings`: **PASS**.

Toolchain: `rustc 1.99.1 (58cf8f9c3 2026-10-13)`, `cargo 1.99.1 (5f94df478 2026-08-27)`. Current hashes: `src/main.rs` `48d73be68241017ef27ce36fd52a4ede72920f5f61ea0c7310fce4cd196893c1`; formatted `tests/http_shutdown.rs` `e874ca8030caae11629a85983f0f6a47b6d367bb63b6ec68d51768f9e28521ad`; `Cargo.toml` `c9da0dee8e4c680581e417ba0f043aec174d9e7270f420c3ca7154c1441bcc48`; `Cargo.lock` `8f6a66b3ea75d5b26beb2365d78dec5e34479d19863fd29c22b4b37450f66d31`.

Private logs and before/after hashes are in `/tmp/ai-guard-http-shutdown.rR1hlb/` (directory mode 700; evidence files mode 600). No release build was run; it awaits the independent review. This focused software evidence does not qualify global release readiness, runtime/container behavior, keep-alive edge cases, or scheduling/I/O stalls.

## Final harness hardening and signal coverage

Reviewer-requested test-only delta was applied after the earlier GREEN: the harness now checks `signal_at.elapsed() <= max` even if `timeout_at` returns an already-ready child future after the deadline. The idle case sends SIGTERM to the dashboard and SIGINT to the gateway, exercising the gateway's Unix `ctrl_c` path. The active-stream case continues to send SIGTERM to both services. No extra test function/service fixture was added.

Final command: `CARGO_INCREMENTAL=0 cargo test --locked --test http_shutdown -- --nocapture` — **PASS**, 2/2, 0 ignored, 18.13 s. Signal-to-exit observations: idle dashboard SIGTERM 1 ms; idle gateway SIGINT 1 ms; active dashboard SIGTERM 5007 ms; active gateway SIGTERM 5010 ms. All satisfy their 3-second idle or 8-second active limits measured from the corresponding signal instant. `cargo clippy --locked --test http_shutdown -- -D warnings` also **PASS** after formatting the test file. No prior `ingress_cli`, `gateway_tenancy`, all-target Clippy, or fmt-all results were rerun.

Final source hashes: frozen production `src/main.rs` `48d73be68241017ef27ce36fd52a4ede72920f5f61ea0c7310fce4cd196893c1`; final `tests/http_shutdown.rs` `7d0a817cdfd1b405f0c2704975e05a6e5ca2b2129474504bb099b1e3317cf942`. Logs `green-final.log` and `clippy-http-only.log` are mode 600 in the private evidence directory. The operating-system signal-handler installation failure path is not injected by these tests; startup/readiness and normal SIGINT/SIGTERM delivery are exercised. No production build, container, provider, model, or external API was used.
