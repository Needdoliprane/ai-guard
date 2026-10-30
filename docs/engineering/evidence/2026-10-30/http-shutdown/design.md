# Bounded HTTP shutdown — implementation proposal

2026-10-30. Design only, source unchanged. Scope: `src/main.rs` exclusively; API/auth stays frozen. Root owns the plan/integration and Luna owns `tests/http_shutdown.rs` and all test execution. Wait for RED and root approval before implementation.

## Cause and intended behavior

Gateway and dashboard currently await Axum `with_graceful_shutdown` without an outer drain limit. A valid persistent SSE response can therefore keep either process alive after SIGTERM. The ingress branch already shares a shutdown future between its listeners and starts a five-second timeout only after that future resolves.

Reuse that exact sequence for the two single-listener servers in a private binary-only helper: construct the graceful server with a clone of the shared signal, select between server completion and signal, then wait at most five seconds for graceful completion after the signal. Normal completion/errors keep their existing result propagation. On timeout, emit one fixed diagnostic and return from main; the binary's Tokio runtime teardown terminates remaining connection tasks. Do not expose this as a library guarantee that dropping an Axum serve future alone aborts every spawned connection.

No startup lifetime timer: an open server without a signal must remain available beyond five seconds. Requests that finish promptly drain normally. A forced end after the window is an interrupted stream, whose client must reconnect with its existing cursor semantics; no new endpoint or audit operation is needed.

## Signal registration

Replace the Unix `expect` in the existing shared signal function with a fallible constructor returning an owned wait future. Synchronously register both SIGINT and SIGTERM using Tokio's installed Unix signal API before each branch advertises readiness. Propagate a registration error with a fixed context and do not announce readiness. The owned future resolves to `()` on either signal, so its existing futures-util `shared()` use remains straightforward and no error cloning workaround or dependency is required.

For non-Unix targets, keep the existing ctrl-C fallback semantics; the production target matrix here is Linux/macOS and no Windows qualification is inferred. The change removes the fallible Unix setup panic without introducing a platform refactor.

Gateway/dashboard prepare the signal before their readiness line and call the private bounded serve helper. Ingress prepares its signal before its existing readiness line and changes the call site to the fallible constructor; its dual-listener graceful/drain sequence remains otherwise unchanged. Authentication, upstream validation, headers, public health, frontend, database/audit and request handling are not edited.

## Acceptance and limits

Luna's RED should own real subprocesses and authenticated SSE connections, confirm the connection is open, send SIGTERM only to that child and detect the current unbounded wait. Keep a no-stream case that is already passing. GREEN must show both gateway and dashboard leave after the drain window, remain alive without a signal beyond five seconds, retain normal health/read authorization, and preserve rapid idle shutdown. The relevant existing ingress shutdown regression must pass because signal setup is shared.

The five-second value is the cooperative HTTP drain budget after receiving the signal, not a hard real-time bound under OS suspension, runtime starvation, blocking filesystem/SQLite work or process teardown delays. It does not supervise workers, stop runtime engines or repair the independent missing broker lifetime authority.

Baseline `src/main.rs` is preserved at `/tmp/ai-guard-http-shutdown-baseline/main.rs`, SHA-256 `dfc772e81dd26a19de7c842f834c38010894068ae10373fc21bc1282212f2822`; the live file matched that hash during design. No test, build, signal or process was launched by this agent.
