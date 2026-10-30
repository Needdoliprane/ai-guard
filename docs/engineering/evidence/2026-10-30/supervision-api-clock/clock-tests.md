# Native broker clock — targeted tests

Worktree: `/Users/louis/Documents/ai-guard/.worktrees/broker-supervision`  
Branch: `codex/broker-supervision`  
Host: macOS (from the task environment; Linux target was not run).  
Scope: `tests/broker_clock.rs` plus test-only harness adjustment; no release, daemon, Docker/Podman, model, provider, or external service.

## RED before integration

`cargo test --locked --test broker_clock` failed to compile with `E0433: could not find broker in agentsandbox`, because the candidate module was not yet integrated. This is a feature-absent compile RED, not a runtime defect. Original log: `/tmp/ai-guard-broker-clock-red.Zm8IWp/test.log`. The test file was formatted after this evidence was captured.

## Targeted result

Final combined command:

```text
cargo test --locked --test broker_clock --test supervision_api
```

Result: clock target **5 passed, 0 failed, 1 ignored**; API target **3 passed, 0 failed, 0 ignored**. The ignored test is the private child-process probe function; the parent test explicitly launches it through the same Rust test executable and consumes its JSON over a private stdout pipe. It is not counted as a product test pass. Rustfmt `--check` on both owned test files passed immediately before the final run.

The first run exposed a test-harness parsing issue (libtest status text shares the child stdout pipe); I changed only the child pipe framing to a marked JSON line. The next run exposed a fixture oracle that counted a fixture-created decision audit event as an endpoint mutation; I moved the head baseline after that event. These were test-only corrections. Their logs remain in `/tmp/ai-guard-supervision-clock-green.JVlvr5/test.log` and `/tmp/ai-guard-supervision-clock-green-retry.rygAXC/test.log`. Final log and command context (mode 600): `/tmp/ai-guard-supervision-clock-final2.94temq/test.log` and `context.txt`.

## What this covers

- Checked integer `timespec` and Darwin timebase conversion boundaries, including invalid readings and overflow.
- Checked exact deadline arithmetic, zero timeout rejection, boot UUID normalization and invalid forms.
- Native `HostClock: Send + Sync`, v1 DTO fields, `observed_utc=None`, canonical non-nil host boot ID, and repeated monotonic samples.
- Two independent native processes on this macOS host: same method and boot ID, child counter between parent samples, with a five-second timeout and child cleanup guard.

## Evidence hashes

Before/after source manifests are identical in `/tmp/ai-guard-supervision-clock-final2.94temq/source-hashes-{before,after}.txt`:

- `src/lib.rs`: `788835958faaea3f1cda206bde430dd8e758ea448e1105c32dcb8694cfab0e67`
- `src/broker/mod.rs`: `8dff47d0124b1612bd576da30518ba5a9a7c87132ea62f8a9ae2b47b40b1d8ff`
- `src/broker/clock.rs`: `ec4b279749e502692dfd58b958efa53b2046ee58f651d5e26f7745e3190edd33`
- `tests/broker_clock.rs`: `96828d05b2ee6ee299a4b96f87c0cc644dd8234d708edeb935b58fd6a20be44a`
- Cargo files unchanged: `Cargo.toml` `55f7ee3de152374b42dd219ec98e004905804879b2677a641e3af434309ea13a`; `Cargo.lock` `8f6a66b3ea75d5b26beb2365d78dec5e34479d19863fd29c22b4b37450f66d31`.

## Limits

This is a targeted producer-clock contract test on the current macOS host. Linux execution, sleep/wake, reboot, syscall failure injection, namespace changes, deadline enforcement by a broker consumer, fencing, and watchdog/process supervision remain **NOT_RUN**. A sample producer cannot make a blocked or suspended consumer execute.
