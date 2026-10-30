# PTY pump flood regression

## Finding

`PtySession::pump` previously kept reading the PTY until `EAGAIN` before reading the control socket. Its 2 MiB capture ceiling bounded storage but did not bound read work. A continuously writing producer could therefore keep refilling the PTY while pump drained it, delaying broker status/deadline handling.

## Regression

Added one local Rust test, `pty_pump_services_control_during_flood_and_caps_capture`, to `tests/runtime_interactive.rs`. It uses a private real `openpty` PTY and a Rust producer thread that writes nonblocking until stopped; no shell, Docker, provider, model or helper image is involved. The producer is explicitly stopped and joined on normal and error paths. The test primes and re-saturates the PTY with more bytes than the test read budget, makes the private control socket readable with a broker status, then runs the pump while the producer remains active. It checks that the pump returns, parses control status, never reads beyond the per-call budget, and caps capture. The local test uses a 64-byte read budget and 16 KiB capture ceiling to exercise those same limit branches quickly; normal harness sessions keep a 4 KiB per-call budget and 2 MiB capture cap.

## RED / GREEN

RED: temporarily disabled the per-call budget in the test implementation to represent the prior drain-until-`EAGAIN` behavior. With a confirmed PTY queue and a continuously retrying Rust producer, the single pump call exceeded the fixed test budget. Original test stdout/stderr is in `/tmp/ai-guard-interactive-harness-flood-red.log` (mode 0600).

```text
running 1 test
Error: one pump call exceeded its fixed PTY read budget
test pty_pump_services_control_during_flood_and_caps_capture ... FAILED
test result: FAILED. 0 passed; 1 failed
```

GREEN: `pump` now reads at most `read_budget` bytes per invocation and proceeds to the control socket in the same poll turn. Repeated calls remain individually bounded while the producer is active; the private control status is parsed, overflow is flagged, and captured storage stays under its cap. Original test stdout/stderr is in `/tmp/ai-guard-interactive-harness-flood-green.log` (mode 0600).

```text
running 1 test
PASS PTY flood regression: pump returned and parsed control status under active flood; read budget and capture cap held
test pty_pump_services_control_during_flood_and_caps_capture ... ok
test result: ok. 1 passed; 0 failed
```

The test uses readiness/progress coordination for the pass criteria. A single monotonic 10-second watchdog bounds producer progress across the cap exercise, and a bounded receive guards against a hung first pump; these are failure safeguards, not latency thresholds. The producer thread is stopped and joined before final assertions.

## Verification

- **PASS** — `cargo fmt -- tests/runtime_interactive.rs`.
- **PASS** — `CARGO_INCREMENTAL=0 cargo test --locked --test runtime_interactive pty_pump_services_control_during_flood_and_caps_capture --no-run`.
- **PASS** — the single local regression, `cargo test --locked --test runtime_interactive pty_pump_services_control_during_flood_and_caps_capture -- --exact --nocapture` (one test; 0.01 s).
- **PASS** — `CARGO_INCREMENTAL=0 cargo clippy --locked --test runtime_interactive -- -D warnings`.
- **PASS** — `CARGO_INCREMENTAL=0 cargo test --locked --release --test runtime_interactive --no-run`.
- **NOT_RUN** — no Docker, provider, model or E2E scenario was run for this correction.

Final SHA-256:

- `tests/runtime_interactive.rs`: `a5f30e67a2b3078fbf8efa07fc1836f2f128445765e4371a3e6ea6683529cc4c`
- `tests/runtime_fixtures/interactive_probe.rs` unchanged: `9f4cb01878a24d0edbb6a7df91c525e376396007876a666decce2ffbd6c60860`
- `src/runtime/terminal.rs` unchanged: `5eef5abec5b351a079b7aa11badc46c8d46be4ff3c1e903772f75da7d847d823`
- `src/runtime.rs` unchanged: `7e878ebc1d7389c390f1d2cdfde6efadbc001677162515f1cf44ff48fae74a42`
