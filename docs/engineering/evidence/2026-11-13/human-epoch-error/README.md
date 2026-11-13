# Human access expiry: compatible temporary-refusal classification

The API now maps the typed `HumanAuthorityFailure::EpochExpired` to `503 identity_unavailable` with `retry_action: retry`. It still refuses access. This is different from `credential_expired` (`401`, authenticate) and an opaque storage/audit failure (`503 audit_unavailable`, contact_operator). Existing capacity and identity error precedence is preserved. No deadline, authority check, permission or mutation replay was changed.

## Source and independent review

The releasable composition starts from public `21608aa27d98852f3ab1edd8a1ca266828e0621c` and changes only the mapper branch, its test-module declaration and three pure tests. The source review is GO_SOURCE; its hash is recorded in [RESULTS.json](RESULTS.json). The three tests exercise the actual mapper, including an error with context, credential expiry and all eleven other authority error classes plus opaque I/O failure. They do not run an identity server, provider or runtime.

## Observed validation

- RED: source `3a24537a56a7355fcc8da5fbcd5782999b3ab681b1167f207167f5bcd3d10c21` (2614 files), a larger private candidate. The selected test failed at the actual assertion `audit_unavailable != identity_unavailable`; the two canaries remained NOT_RUN after stop-first. This is not a RED execution of the smaller public tree. The old mapper function is byte-identical on the public base (SHA256 `a6ec147a4621c0e714ee6178ac0aed376ab4c7237e86136cea83139370a58114`).
- GREEN: the distinct public composition `51f91f7896f2dfef7cecfa3994eb0935a0a74fb40f9c6922148a9ad0d3b352f5` (2608 files) compiled its library tests and passed all three selected parents, none ignored. All other library tests were filtered, not executed.
- QA: offline Linux ARM64, pinned compiler/image, UID1000, no network, read-only root and inputs, private tmpfs/cache/target. All 2608 exported source payloads and modes matched; source export was bounded, OOM was not observed, closure returned 0 and the exact QA container was absent after removal. This is software evidence, not macOS/runtime qualification.

The actual compilation used `cargo test --frozen --offline --lib --no-run --message-format=json`; its JSON selected one real library-test artifact. Each of the following parents then ran once with `--exact --nocapture --test-threads=1` and a 30s outer bound:

```text
api::human::epoch_error_tests::expired_authority_is_a_transient_identity_read_refusal
api::human::epoch_error_tests::credential_expiration_keeps_its_authentication_refusal
api::human::epoch_error_tests::storage_and_other_authority_failures_keep_opaque_operator_refusals
```

On this composition, the equivalent targeted Cargo selection for reproduction is:

```console
cargo test --frozen --offline --lib api::human::epoch_error_tests:: -- --test-threads=1
```

Run it in the isolated QA environment required by the repository instructions. [RESULTS.json](RESULTS.json) retains source and evidence fingerprints; raw private QA artifacts are not distributed as product data.

## Limits and recovery

The cause and cost of repeated access-check expiry remain open. This change neither makes the check faster nor extends its two-second Core deadline. It does not establish login availability or repair an audit store. Refresh identity and explicitly retry the refused read; `retry` is not permission to replay a mutation or an ambiguous OIDC exchange. Existing bounded SSE GET recovery is unchanged. Follow the paired [English](../../../../setup/sso/README.md#diagnose-and-recover) and [French](../../../../setup/sso/README.fr.md#diagnostiquer-et-reprendre) guides.

All-targets check, Clippy, browser, real OIDC/Keycloak/provider and Docker/Podman/Kubernetes worker tests are NOT_RUN for this narrow lot. No audit finding or global gate is closed, and no qualification is transferred to the other private identity ports or broker work.
