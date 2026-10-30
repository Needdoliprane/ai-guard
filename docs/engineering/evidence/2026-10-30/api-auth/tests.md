# API authentication boundary — RED evidence

## Scope and source identity

Only the newly owned file `tests/api_auth_boundary.rs` was added/changed for this task. The application source was not edited by this test author. No `.env`, real key, Docker/Podman, external API, model, or full test suite was used.

Source hashes recorded after the RED run are in `/tmp/ai-guard-api-auth-boundary.4lP1m9/source-hashes-after-red.sha256`; the test source hash observed during the run was `6c07398cb6d68fe9430d0c90194374c38eb25cf389699963c97578407e47471c`.

## RED command

```sh
CARGO_INCREMENTAL=0 cargo test --locked --test api_auth_boundary -- --nocapture
```

Exit 101, duration 4 seconds. Six test functions ran; four behavioral groups failed and two groups passed. Full private output: `/tmp/ai-guard-api-auth-boundary.4lP1m9/red-final.log`, SHA-256 `0b5ab8c5ae4687a6f8736c2267cad6c48bfee524f7b2e3ee64f0abd6f0fa898e`. This is the valid behavioral RED after correcting an earlier test-harness compile issue; the final run compiled without warnings.

## Observed failures

- For four POST route families (`events`, `decide`, `workers/reservations`, `workers/{id}/transitions`) on both `/api` aliases, missing/invalid credentials and viewer writes caused the request body stream to be polled. `events` and `decide` returned 400 for malformed JSON before the expected 401/403; the reservation and transition handlers returned the expected statuses but only after polling the body.
- Bad `limit` query values on `/api/v1/events` and `/api/events` returned 400 to missing and invalid credentials instead of authenticating first and returning 401.
- Two `Authorization` headers containing different configured credentials were accepted by taking one value; the identity route returned 200 instead of 401.
- A 4097-byte bearer token on a write request was not rejected before JSON parsing: it returned 400 and polled the body twice.
- The real TCP child test sent only headers for `POST /api/v1/events` with `Content-Length: 1024`; no body bytes were sent. The server did not return a status within 2 seconds (timeout), demonstrating the pre-authentication body wait.

## Behaviors already passing

- `/healthz` stayed public; identity remained bound to A/B runner/viewer credentials across `/api/v1` and `/api`, ignoring a tenant query/header hint.
- Runner-authorized malformed JSON retained status 400 across the four write routes and did not change either tenant's SQLite audit event count.

The tests aggregate route cases before asserting so one known failure does not suppress results for the other cases. This evidence establishes the current API boundary gap; it does not qualify general request-size/DoS resistance.

## GREEN after the API core fix

After root froze the core fix and authorized GREEN, `rustfmt --edition 2024 tests/api_auth_boundary.rs` was run on this test file only. The original RED log and pre-format source hash above remain preserved.

1. `CARGO_INCREMENTAL=0 cargo test --locked --test api_auth_boundary -- --nocapture` — **PASS**, 6 passed / 0 failed / 0 ignored, 10 s total (8.62 s compile; 0.67 s tests). Log `green-new-target.log`, SHA-256 `e3541ad7da60f3070525c89773a1b39a4451a8f238650d50ff6d2bb1a44b6ece`.
2. One combined run of the eight authorized neighboring suites — **PASS**, 45 passed / 0 failed / 0 ignored, 8 s total:

   | Target | Passed | Failed | Ignored |
   |---|---:|---:|---:|
   | `api_security` | 4 | 0 | 0 |
   | `tenant_api` | 3 | 0 | 0 |
   | `gateway_tenancy` | 1 | 0 | 0 |
   | `workers_api` | 6 | 0 | 0 |
   | `decision_idempotence` | 9 | 0 | 0 |
   | `jev_credentials` | 14 | 0 | 0 |
   | `jev_contract` | 2 | 0 | 0 |
   | `provider_authorization` | 6 | 0 | 0 |

   Command: `CARGO_INCREMENTAL=0 cargo test --locked --test api_security --test tenant_api --test gateway_tenancy --test workers_api --test decision_idempotence --test jev_credentials --test jev_contract --test provider_authorization -- --nocapture`. Log `green-regressions.log`, SHA-256 `80efd8a4d262bb5a97f800d2b4eb5ab0af34b1fe7bca41135e4d59b63a17db9e`.

3. `cargo fmt --all -- --check` — **PASS**, 0 s; empty log SHA-256 `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`.
4. `cargo clippy --locked --all-targets -- -D warnings` — **PASS**, 6 s; log SHA-256 `1fe3d36381736ca7d47f415ffe3a2e6302558e2319791187415334d9f86ec2c5`.

Across these nine targets: **51 passed, 0 failed, 0 ignored**. No full suite, Docker/Podman, browser, external API/model call, or release build was run. Child gateway processes are exercised by their owning tests and are not counted as separate test cases.

Before/after hashes for the 11 involved source/test files are in `regression-before.sha256` and `regression-after.sha256`; `regression-source-diff.txt` is empty (comparison exit 0). Final formatted test source SHA-256: `1817710533dbf0f36e71b2be19022053a717e26fc70426146b4d64464d0bdd80`. Logs and manifests remain in `/tmp/ai-guard-api-auth-boundary.4lP1m9/`, mode 600, in a mode 700 directory.
