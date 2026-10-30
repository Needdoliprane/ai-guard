# API authentication before extraction — proposed implementation

2026-10-30. Design only; no product source edited and no test executed by this agent. Source ownership is limited to `src/api.rs` and a new `src/api/auth.rs`. Root owns the implementation plan and Luna owns execution of the regression tests. Implementation waits for root's explicit signal after the RED reproduction.

## Confirmed cause

`GatewayState::authorize` currently loops over tenants and calls `token_matches` for each runner/viewer credential. Each comparison hashes both the received token and the configured secret. An unknown token is therefore rehashed twice per tenant. `TenantState` also retains and clones both raw control credentials for every successful request.

Protected handlers currently call authorization from their bodies. Axum has already extracted all function parameters at that point: `Json` consumes/deserializes the body and `Query`/`Path` may parse their input before authentication. The `Result<Json<...>, JsonRejection>` variants retain the error but still consume the body before entering the handler. This matters for unavailable/invalid identities and viewers posting a body.

The installed official source is Axum 0.8.9 with axum-core 0.5.6. `FromRequestParts` receives only request parts and state; the tuple extractor implementation in `axum-core/src/extract/tuple.rs` executes parameters left-to-right and processes the final body extractor afterwards. A first-parameter authentication extractor therefore rejects before application body consumption, including handlers that catch JSON rejection. It does not claim that Hyper has not received/buffered network bytes or parsed the URI/headers for routing.

## Proposed code

`src/api/auth.rs` will hold a small private credential index, strict bounded Bearer parsing, and two private `FromRequestParts<GatewayState>` extractors:

- A reader identity contains the selected `TenantState` and public role (`runner` or `viewer`).
- A runner identity contains the selected tenant only after write authorization.

Every protected handler takes its identity first, before any `Path`, `Query` or `Json`. A handler requiring policy or Jev state retains a subsequent `State<GatewayState>` parameter. Others no longer need state or authorization headers. SSE retains a `HeaderMap` solely for `Last-Event-ID`, after its identity extractor.

The index is constructed once from the trusted `TenantGateway` values: SHA-256 digest to `{tenant_index, role}`. Startup rejects duplicate digests, empty credentials and values exceeding 4096 bytes. Whitespace is rejected consistently with current file-based control-token loading. `GatewayState::new` keeps its existing invalid-configuration fail-closed contract: it returns a state whose protected requests return 503, not a partially working identity set. `for_tenants` continues returning a typed construction failure. Existing tenant IDs, store bindings, integrity validation, Jev credential separation and trusted quota assignment remain unchanged.

An authenticated request validates one Authorization header and a nonempty token of at most 4096 bytes, then hashes it once and performs one index lookup. Multiple Authorization fields, invalid encoding, missing/invalid `Bearer ` prefix and whitespace-containing tokens fail with 401. The public `token_matches` helper remains available for the dashboard/API compatibility; it is no longer the gateway's per-tenant lookup mechanism. Reusing the shared bounded Bearer parser also prevents ambiguous/oversized headers in the dashboard proxy. No claim of constant-time HashMap lookup, calibrated timing resistance or load capacity follows from this data structure.

`TenantState` no longer contains raw runner/viewer tokens; the trusted constructor inputs and caller-supplied redactor keep their current responsibilities. This reduces copies, without promising memory erasure or removing secrets from the trusted control process.

## Compatibility and errors

- `/healthz` stays public; `/api` and `/api/v1` retain the same routes and identity-derived tenant binding.
- Runner can read/write; viewer can read and gets 403 before write-body extraction. Neither query/body/headers nor an asserted tenant ID chooses a tenant.
- Missing/invalid/ambiguous/oversized identity gets 401 before protected handler parsing. Invalid identity configuration stays 503.
- A valid runner still receives existing JSON, query, path and body-limit errors; admitted mutations, decisions, hashes, audit transactions, quotas and Jev behavior are untouched.
- Unsupported routes/methods remain Router responsibilities; no new authentication middleware or duplicate handler authentication is added.
- No database/schema migration, dependency, endpoint, runtime, frontend, provider or credential-file mutation is introduced.
- File-based token readers are outside this change. The gateway main reader currently has a tighter limit than the runner's 4096-byte reader; the request bound preserves both. Library users configuring more than 4096 bytes will now fail closed, which must be documented.

## Acceptance and focused regression evidence

Luna should first reproduce the current behavior with a streaming body whose poll is observable: missing/unknown credentials and viewer write requests must return 401/403 without polling it. Run these cases against both aliases and all mutation route shapes, including caught JSON rejections. An authenticated runner malformed-body case must still consume/reject input normally. Malformed query/path with no valid identity must not supersede authentication.

Keep a real-HTTP incomplete-body regression: send headers declaring a body without delivering that body and confirm a prompt unauthorized/forbidden response; no Docker or provider call is needed. Use a Rust-owned gateway child so tool-session background cleanup cannot invalidate the harness.

Add bounded/multiple-header cases and preserve A/B isolation, duplicate-credential startup rejection and invalid-state refusal. Existing tenant API, gateway HTTP/restart, API security, decision idempotence, worker API and Jev credential tests are the relevant compatibility candidates; root/Luna choose the smallest useful set. The implementation agent does not run them or broaden into E2E/model tests.

The source-level reduction from per-tenant rehashing to one bounded digest/index lookup is not itself a performance benchmark or reproduction of a remotely exploitable denial of service. The audit's immutable verdict remains unchanged.
