# Gateway transient failures during an active worker

Status: **PROPOSED — maintainer decision required; no implementation authorized**.
Source examined: `0a41aac192f4139073a43830dbb71df01cb87abf`, 3 November 2026.
This proposal accompanies audit correction C; it is not part of that correction.

## Existing behavior

In `src/runner/managed.rs`, the execution monitor refreshes the worker registry
every 500 ms. Any refresh error requests a stop of the exact owned runtime
instance and ends the monitor. The `on_started` callback separately requires a
durable runtime observation and `command.started` acknowledgement. The
[operations contract](../operations.md#audit-sorties-et-couverture) documents a
targeted stop after a tracking error. A failed stop does not prove termination.

The planned correction C would separate incomplete uploads from the ingress
exchange pool and remove one anonymous trigger. It would not make gateway
failures harmless or change this runner contract. The audit's stop reproduction uses a fixture
engine, not a qualified Docker/Podman worker.

## Decision proposed

Permit a bounded retry window, at most **60 seconds**, for a transient failure
of a read-only registry refresh **after an acknowledged start**. During that
window the existing worker may continue within its original runtime limits.
Its control status is degraded/unknown; it is not declared healthy or renewed.
This deliberately increases the time before a stop is requested after loss of
control visibility. It must be accepted explicitly.

The window uses a monotonic deadline set at the first transient failure. A
retry, process restart or partial response must not reset it. Every request,
backoff and server retry hint fits inside the remaining window and original
execution deadline. No new launch, claim, permission, quota extension or
business mutation is admitted through this mechanism. An observed stop request
is executed immediately. Expiry requests a targeted stop and preserves an
unknown result until terminal runtime evidence and durable accounting exist.

A monotonic `Instant` is not a portable restart anchor. If the broker restarts
with an unfinished degraded interval, or cannot verify its original bound, it
requests a targeted stop and reconciliation; it does not grant a fresh window
or reconstruct one by assuming wall-clock continuity. Any richer recovery
policy would require its own design and evidence.

Only typed, explicitly retryable failures qualify: connection interruption,
bounded transport timeout, HTTP 429 and an explicit temporary-unavailability
response. Authentication/authorization failure, changed identity, malformed
response, audit-integrity/storage failure and an unclassified HTTP 503 retain
the immediate-stop path. The client currently erases some HTTP error detail;
typed classification is a prerequisite, not string matching on error text.

Record the degraded transition and recovery through the existing durable
control journal, with redacted diagnostics and no invented runtime evidence.
If that required local record fails, request the stop immediately. Reconcile
the same worker after recovery without repeating its start or duplicating
terminal usage. The retry mechanism cannot replace the independent lifetime
guardian, whose qualification remains separate.

The mandatory `on_started` audit callback is **outside this proposal's retry
window**. Its failure still requests a stop. Allowing execution for 60 seconds
before the required start audit is acknowledged would require a separate
decision and an explicit durable evidence design.

## Acceptance before changing the contract

- A short temporary refresh failure recovers for the same worker; no second
  start, renewed authority or fabricated healthy interval is recorded.
- A sustained failure reaches the fixed bound, requests a stop of only the
  owned instance and keeps quota/result unknown without terminal evidence.
- Authentication, integrity, identity and mandatory-audit failures retain
  their immediate refusal/stop behavior.
- Stop requests, original execution deadline and cancellation remain effective
  during backoff; slow requests and repeated 429 responses cannot reset the
  bound. Restart does not grant another window.
- Durable recovery and terminal usage remain idempotent. The required journal
  failure path is covered.
- Targeted fixture tests establish classification/state transitions; real
  runtime failure/recovery tests are still required for runtime guarantees.
- Update the English/French operator guides together with code, including the
  accepted interval of lost visibility and the limits of a failed stop.

No change to `src/runner/managed.rs`, `src/runtime/managed.rs` or the current
operations contract is made by this proposal.
