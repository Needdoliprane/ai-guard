# HTTP drain deadline — implementation candidate

2026-10-30. Edits are limited to `src/main.rs`, following the approved design and root's go-ahead after Luna's reinforced RED. API/auth and Luna's regression file were not edited. No compilation, test, signal, server or API/model execution was performed by this implementation agent.

## Change

A private `serve_http` helper now owns the gateway/dashboard listener and Router. It shares an owned shutdown future with Axum's graceful-shutdown hook and selects between ordinary server completion and the shutdown signal. Only the signal branch starts the five-second graceful drain timeout. A server without a signal receives no startup/lifetime timer from this helper. Existing server completion errors still propagate.

If draining expires, the helper emits a fixed diagnostic and returns successfully through the CLI branch. Main then exits and the Tokio runtime tears down remaining connection tasks. This is deliberately a binary-level exit path, not a public library claim that dropping a serve future alone aborts every stream. There is no worker/runtime-engine shutdown or new broker behavior in this change.

The Unix `shutdown_signal` constructor now registers both SIGINT and SIGTERM synchronously and returns a fallible result containing an owned wait future with `Output=()`. Registration errors receive fixed contexts and propagate before each service announces readiness. The SIGTERM setup `expect` is removed. The non-Unix ctrl-C fallback retains its previous semantics; no new platform qualification is inferred.

Gateway/dashboard construct the signal before binding/announcing their listener, then call the helper. Ingress only adapts to the constructor's `Result` and moves construction before binding/readiness; its existing dual-listener shared-signal/five-second drain flow remains intact. Its old comment is clarified to attribute connection-task termination to main/runtime teardown, rather than dropped futures alone.

Authentication, SSE data/cursors during normal operation, upstream checks, frontend, health responses, audit/SQLite and worker lifecycle code are unchanged. Streams still active after the grace window may be interrupted and use the existing reconnect/cursor mechanism.

## Scope and limits

The five-second value is the cooperative drain budget after a shutdown signal, not a hard wall-clock promise under OS suspension, blocking I/O/SQLite, runtime starvation or teardown delays. Errors while setting up Unix handlers are propagated; arbitrary fault injection for that OS API has not been performed. SIGINT/SIGTERM and ingress compatibility must be checked by the assigned Luna tests/reviewer. Nothing here qualifies worker supervision after broker death.

`rustfmt --edition 2024 src/main.rs` was applied. Compilation and GREEN remain pending root/Luna. The final format-only check is recorded separately in the tool history.

## Candidate identities

```text
48d73be68241017ef27ce36fd52a4ede72920f5f61ea0c7310fce4cd196893c1  src/main.rs
0d61502c45d2b25dec3c135b71495d11e024ec47b414525c48dea637ceaeceb8  src/api.rs (unchanged)
f1b1b30222ecbfb1220be5dc82191e03bf560cc4bf5e528e30033641a747ceee  src/api/auth.rs (unchanged)
6e48dbfb07adc3fafb6b7306991813828f8ba9a70028f4d92fb1563014134a44  tests/http_shutdown.rs (unchanged)
```

Original main, preserved by root at `/tmp/ai-guard-http-shutdown-baseline/main.rs`, matches pre-edit SHA-256 `dfc772e81dd26a19de7c842f834c38010894068ae10373fc21bc1282212f2822`. A direct source diff against that copy was read; its exit 1 means expected source differences and is not a test result.
