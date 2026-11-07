# Verified initial authentication proofs — 7 November 2026

This is a partial correction of audit lot M. The source is based on `a329153`,
composed with the explicit authentication-migration CLI. It does not close the
global audit-capacity requirement, an independent audit finding or a release
gate. The [audit register](../../../AUDIT_FOLLOWUP.md#audit-final-run-1) retains
the primary report and its limits.

## Behavior and compatibility

A verified initial access proof reserves its fingerprint and verified OIDC
principal in the durable authentication ledger before membership evaluation.
The reservation and its rejection allowance share one transaction. Replays,
including a pending attempt left by an interruption, cannot obtain a fresh
ledger write by requesting a new intent. Reopening the store preserves this
deduplication. The derived fingerprint index is rebuilt from the verified
chain; it is not a separate durable authority.

Failed and pending attempts consume a finite ledger-lifetime budget per
verified `(issuer, subject)`, across web/CLI clients. The count ceiling is
`clamp((max_records - 1) / 16, 1, 64)`. The serialized-byte ceiling is one eighth
of the configured ledger capacity, with room for at least one auditable attempt.
A durably admitted session releases its pending budget, while proof
deduplication remains. There is no time reset, automatic archive or policy
migration implied by this ceiling.

An exhausted subject can recover through a freshly established active
membership, followed by the normal audited authorization and session CAS.
Failure of a neighboring store does not discard a membership actually found
in a healthy store; failure to establish one remains closed. This check cannot
grant rights from a cached membership or a client-selected tenant. The web
delivery path preserves `authentication_subject_capacity_exhausted` as HTTP429
with `contact_operator`, instead of presenting it as an authentication failure.

The authentication ledger has an explicit V2 marker, SQLite `user_version`
`1000002`. Existing V1 entries retain their payloads, hashes, namespace, head,
policy and logical counters. V1 can be inspected but normal gateway startup
requires manual migration. The matching Rust CLI creates and verifies a new,
private coherent V1 backup before changing the source marker. Already-V2 and
existing backup destinations are refused. The operator's
`--confirm-control-stopped` attestation is not process fencing. See the
[English](../../../../setup/sso/README.md#migrate-an-existing-authentication-audit)
and [French](../../../../setup/sso/README.fr.md#migrer-un-audit-dauthentification-existant)
procedures.

## Reproductions and review

The first real reproduction reached a denial, then grew the shared ledger from
3 to 13 records by replaying the same proof. Its initial success-status oracle
expected 200 instead of the existing native-admission 201; that harness error
was corrected before the six-case baseline. The six baseline parents then
failed their functional assertions on the uncorrected source (0.93 s; build
1 min 46 s), output SHA-256
`75dee54f9bbe99e77abd6881dc721f58c9dc6bfe3da730e4ec200f60e59639c1`.

Independent source review found a linear fingerprint search on every replay
and an early neighboring-store error in membership recovery. The latter was
reproduced with four retained read permits in A and a real approved member in B:
fresh admission succeeded, exhausted recovery returned 429 instead of 201.
Output `7c7baf1d66aef1baf69509fea8c33d6a15b5d6804cf77a02c656b2386cb57d0d`.
The corrected recovery parent passed on composed02 (0.27 s; build 1 min 57 s),
output `f8f6d9a28d714bd25254e5e75e6c392c658d5fb469863f3f2d41a1cc46c63586`.
The derived index and recovery delta received a second source review.

Composed02 still had two integration failures: web delivery changed the budget
error into HTTP401, and a cold provider-discovery race hit its existing
nonwaiting capacity lock before the proof could reach the ledger. Diagnostic
gels preserved these failures. Composed03 fixes the web mapping and makes the
concurrency harness causal: it holds the real controlled issuer's key response,
checks the cold refusals and unchanged ledger, then separately requires exactly
one denial and three replay conflicts with ready metadata. No production lock,
timeout or permission was weakened to obtain that result.

## Validation

The source manifest is [source.sha256](source.sha256). Its eleven build/test
entries match composed03. The two guides were finalized afterward without
changing the tested Rust sources. Tests run through Luna
inside an admitted Docker QA container, with no external network, a read-only
toolchain/source/registry, an allowlisted environment and a dedicated Cargo
target for each gel. Rust 1.99.1 Linux ARM64, jobs1, incremental/debug information
disabled, no stack-size override. Effective bounds: 4 CPU, 8 GiB RAM without
swap, 256 PIDs, 4 GiB scratch/65,536 inodes, 512 MiB per file, 1,024 FDs and a
fixed host-clock deadline. This container hosts software tests; it does not
qualify the product's Docker, Podman or Kubernetes adapters.

```text
cargo test --offline --locked --test audit_auth_ledger_replay -- --test-threads=1
cargo test --offline --locked --test oidc_gateway_flow -- --test-threads=1
cargo test --offline --locked --lib api::oidc::gateway::admission_tests::exhausted_subject_recovers_in_healthy_b_when_first_tenant_read_is_saturated -- --exact --test-threads=1
cargo check --offline --locked --all-targets
```

Each invocation uses an explicit manifest and source-specific target.

| Source and check | Result | Parent output SHA-256 |
|---|---|---|
| Composed03, proof-budget integration | PASS 14/14, 1.84 s; build 1 min 48 s | `3bfa04c7d5a9fa8c780e241089b053447fc0e27befc71f8c3d300936e147f73d` |
| Composed03, existing web gateway flow | PASS 9/9, 34.10 s | `2ea657f15b50f1ca540911b80cf6a19884dca101c9396819fee10ec56f46a7bb` |
| Composed03, healthy-B recovery parent | PASS 1/1, 0.24 s; build 57.05 s | `71e588e70bd9cce7f3418018dfa4a28ee001a4028e7c36bc3352e7fab90568c7` |
| Composed03, all-target compilation | PASS RC0, 1 min 04 s | `9fb1c7d20aa8b45451ab8ca355121b28d45c0fc477e5404fc91fafb38af913be` |
| Composed02, migration CLI | PASS 5/5, 0.16 s | `8d35f3e496030fd9d7075de68b5fdd4a50303847e0161b350d62b9ead3eb45a` |
| Composed02, authentication start capacity | PASS 7/7, 0.31 s | `ed5a781db9883fc4fbd3707566e4b3dd5aa693ec88b17eb0e54d739cbf18a1bb` |
| Composed02, authentication audit | PASS 24/24, 0.42 s | `79d83d9573f81bbde5bdae6901cf8bb5baf037a00a0f5ccc2ead510512f76477` |
| Composed02, all-target compilation | PASS RC0, 1 min 05 s | `29169f6888fdde87113945d9d2e78d80d1f4f1b34e13b565b60236d398200921` |

The unchanged CLI, capacity and ledger suites are not rerun by reflex on03.
Their evidence remains explicitly attributed to02. Different configurations,
the integration branch and other worktrees do not inherit these results.
The composed03 check reports 119 library, two SSO-test and three library-test
warnings; the static-analysis gate remains open. Native Rustfmt/diff checks
pass on the six M Rust files. Independent source and final proof/guide reviews
are accepted. Publication and integration are tracked separately in the audit
register; these reviews do not execute the tests again.

## Remaining limits

The shared ledger remains finite. This patch does not reserve global space for
control/revocations, separate all human-read budgets, implement retention or
archive, validate storage-at-capacity recovery, or prevent all authorized
principals from collectively exhausting the shared budget. Migration does not
reset capacity or restore live tokens, sessions or execution authority. Its
backup does not provide independent rollback detection or signed checkpoints.

Real provider CLI/API, browser, Keycloak, Docker/Podman runtime, Kubernetes,
filesystem-full and multi-host qualification are NOT_RUN for this lot. The
complete product remains unqualified for production.
