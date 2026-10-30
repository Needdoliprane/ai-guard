# API authentication boundary — implementation candidate

2026-10-30. Product source ownership: `src/api.rs` and the new `src/api/auth.rs` only. The implementation followed root's approval after Luna's captured RED on the unchanged API. No test, compilation, Docker or provider/API call was executed by this implementation agent. `rustfmt --edition 2024 src/api.rs src/api/auth.rs` completed with exit 0.

## Implemented change

- Private `ReadIdentity` and `RunnerIdentity` implement the installed Axum `FromRequestParts<GatewayState>` contract. Each protected gateway handler takes its guard before `Path`, `Query` or `Json`; only SSE retains Authorization-independent header access for its cursor. No additional authentication middleware or second handler authorization was added.
- The gateway constructs a credential-digest index once, mapping a SHA-256 digest to trusted tenant index and role. A request validates the Bearer field and hashes its token once, rather than rehashing it and every configured credential during a tenant scan. Missing, duplicated, invalid, whitespace-containing or over-4096-byte Bearer values are refused. The shared Bearer parser also protects the existing dashboard proxy; its routing/forwarding logic is unchanged.
- `TenantState` no longer retains runner/viewer token strings. Credentials are still trusted constructor inputs and may remain in the caller-supplied redactor; no memory-erasure claim is made.
- The credential index, policy, redactor and optional Jev client are immutable `Arc` fields in `GatewayState`. Axum state cloning therefore does not deep-clone the index or the full configured secret dictionary. The public constructors retain their signatures. The successfully selected tenant identity is still cloned after authentication; no claim that every string copy vanished is made.
- Index entries retain vector indices; `with_worker_quotas` preserves that vector's order while changing only trusted quotas. Lookup uses checked `get`, not an indexing panic. Duplicate/malformed configured credentials return an error in `for_tenants`; `new` substitutes an empty index with `valid_credentials=false`, so a partially built index never authenticates a request and invalid-configuration requests still return 503.
- The write gate explicitly requires `CredentialRole::Runner`, rather than allowing every role except viewer. Public `token_matches`, `/healthz`, aliases, tenant/store checks, Jev credential separation, decisions, quota/audit paths and schemas are preserved.

## Compatibility and limits

Authentication now precedes application-level extraction on protected matching routes. This does not claim that the HTTP transport/routing layer has not parsed headers/URI or buffered received bytes. Existing cross-origin middleware keeps its earlier rejection order. Validly authenticated malformed input still reaches its existing rejection path; viewer writes fail before consuming that input.

Configured library credentials exceeding 4096 bytes or containing whitespace now fail closed. File-based readers are unchanged and already impose limits at least as strict for their current paths. Duplicate Authorization fields are now ambiguous even if the first was valid. No new endpoint, dependency, storage migration, worker role, frontend behavior or provider capability is introduced.

An indexed lookup is not a constant-time lookup and is not a measured DoS/load qualification. The immutable historical audit verdict is unchanged. Compilation and GREEN regression execution remain pending root/Luna at this candidate freeze.

## Candidate hashes

```text
0d61502c45d2b25dec3c135b71495d11e024ec47b414525c48dea637ceaeceb8  src/api.rs
f1b1b30222ecbfb1220be5dc82191e03bf560cc4bf5e528e30033641a747ceee  src/api/auth.rs
6c07398cb6d68fe9430d0c90194374c38eb25cf389699963c97578407e47471c  tests/api_auth_boundary.rs (Luna-owned, unmodified)
```

The original `src/api.rs` hash was recalculated immediately before editing: `823451d1a68c91e4985237d24934bd896c10a808b40df25844bff3b8a61c0b79`. This agent did not copy that original file; root recovered an exact matching historical source separately. No reconstructed original is presented as evidence.
