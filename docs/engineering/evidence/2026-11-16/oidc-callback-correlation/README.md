# Native OIDC callback recovery — 16 November 2026

## Behavior and provenance

A malformed or uncorrelated loopback request no longer consumes the pending native CLI login immediately. The listener processes at most eight peers sequentially under the original absolute monotonic login deadline. Each parse is bounded by two seconds and each response/shutdown by one second, truncated to that original deadline. Correlation is checked before an HTTP200 or a terminal provider refusal. The existing strict parser, `complete_login` PKCE/state/provider checks and single token exchange remain in place.

- Primary run: `ai-guard-20261108-multiworktree-deep-1`, fingerprint `oidc-cli:loopback-first-peer:single-attempt-denial`.
- Primary findings SHA256: `3b27a6fe0f4ce8f8f5856514f7dfb836f2161ab267473dfe04d78dcbed3df212`; primary verdict remains `needs_validation`.
- Original callback/caller on `143dfafa6bf83787845d5d32c9e7db99746c41e4` were rechecked. The original controlled same-principal loopback oracle observed HTTP400, termination and failure to connect a follow-up peer. This reproduces the availability behavior; it establishes no separate-principal access, token capture or authentication bypass.
- Original fixture archive: `3a3cab088095adcbf9de630ec0853904b63c6231272aae607100628fd88a538f`.
- Corrected candidate R2: `3ccb652c0d0be72bc5cd72406bc806b28f6ad00fe685b459d21c21968f8a826f`; independent source review `d0aacb198466b2daa22c00fba21ea04ea07b947e37ef8d75eceae5f82ebe8b97`.
- Tested composition: published image-policy base `7e61988cd6342a0d883f5e4fd1b5c286959d3de3` plus the seven callback candidate paths; archive `31d83794fd845de96cbb9d3b94b8db8ccb36e43d45b5ae08f527b0eadb185892`, 2910 files.
- Code commit `f393df71584f240816f23e5cf1c252d9f3c9a9b9`, merged as `20f9d65e197b23237023c1d781d4b81276e9b54f`; all2910 tested content inputs match the composed tree. Existing more restrictive checkout modes on unchanged old files are preserved; Git modes match.

## Executed evidence

Native macOS ARM64, Rust/Cargo1.99.1, private HOME/TMPDIR and cleared six-key environment, locked offline dependencies. Dev/test profiles: opt0/debug0, assertions/overflow enabled, incremental disabled, codegen256.

| Check | Result |
| --- | --- |
| Original real listener oracle, exact, on the original libtest | REPRODUCED_ORIGINAL_BEHAVIOR; RC0; not a correction PASS |
| `cargo check --all-targets --locked --offline -j1` | PASS; 166 project artifacts `fresh:false`, zero compiler errors |
| `cargo test --lib --no-run --locked --offline --message-format=json -j1` | PASS; new libtest artifact `fresh:false` |
| `cargo test --test sso_cli --no-run --locked --offline --message-format=json -j1` | PASS; new integration artifact `fresh:false` |
| Ten exact listener unit tests | 10 PASS, zero ignored/fail |
| Four exact `sso_cli` integration tests | 4 PASS, zero ignored/fail |
| `cargo clippy --lib --test sso_cli --no-deps --locked --offline --message-format=json -j1` | RC0, zero errors, 248 warnings outside the four modified Rust files; no strict warning gate closed |
| Rustfmt check, edition2024, `skip_children=true`, four modified Rust files | RC0, unchanged source |
| Post-execution comparison | 2910/2910 source contents and modes exact |

The ten listener cases cover invalid then valid, wrong state code/refusal, duplicate/issuer rejection, correlated refusal, valid eighth peer, exhaustion after eight invalid peers, original deadline, already-expired deadline, a slow first peer and cancellation. The four CLI cases are `duplicate_callback_parameter_is_refused_before_exchange`, `unrelated_callbacks_preserve_one_pending_login_until_exact_completion`, `native_pkce_provider_cancellation_closes_callback_without_token_exchange` and `eof_cancels_pending_native_login_and_closes_its_listener`. The duplicate400/zero-exchange assertion is preserved before explicit cancellation.

Root re-read all14 raw results and compiler JSON. Root receipt: `b69adfb2078e7f650ccffca38c4f958cfa8f989242cf4f391292f859dbcfe698`; executor receipt: `0515a3d336689666336c4038296c87481fb257cffbd888c9a939883ba29604e2`. Raw logs, both source archives and the original binary are retained privately under `oidc-callback-original-native-qa-r1`. The initial attempt to list the unit in the bin harness found no selector; it executed no test and remains a separate preparation record, before the correct lib harness. Authoring-time NOT_RUN comments refer to delivery before this executed QA.

## Residual scope

Eight occupied peers can still exhaust one pending attempt; this is bounded resilience, not general local saturation protection. Separate OS principals and sandbox-to-host loopback reachability were not tested. The cancellation unit observes listener/pending socket closure without a deterministic accepted-parse handshake, so cancellation specifically inside an accepted parse is not claimed.

Live Keycloak/Entra, other platforms, Native17 owned identity/SSO, real providers, worker runtimes and the complete SSO product are NOT_RUN in this lot. No formal audit severity or primary verdict is rewritten, and no production gate closes. English/French operator guides and the SSO contract describe the same bounded behavior.
