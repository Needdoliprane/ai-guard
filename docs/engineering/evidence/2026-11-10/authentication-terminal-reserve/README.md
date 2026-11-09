# NV-03: reserve refresh terminal capacity before an external effect

Scope: current authentication-audit writers, immutable existing policy, tracked
refreshes. The new audit's original source-only classification is preserved;
these are separate controlled software reproductions and regressions.

## Change and review

One validated `RefreshPending` now reserves one terminal record and the absolute
32,768-byte serialized entry allowance. Every new append accounts for all
outstanding allowances in the same `BEGIN IMMEDIATE` transaction. A validated
Rotate/Close consumes its own allowance once; exact replay neither allocates
another exchange nor releases another session's allowance. Reopen reconstructs
the allowances from validated history. No table, format, genesis, policy or
historical payload is changed.

Author: `d5_composition_review`; independent reviewer: Root. Review covered CAS
and phase validation before capacity checking, complete-entry serialization
bound, checked arithmetic, replay ordering, competing writes and legacy limits.
The two addon test files also received an independent source review before QA.
Sources start at `696efd18ed21f78df7429a20d8e16dbcac1c00d9`; the three modified
product files are byte-identical across the two corrected test compositions.
No guarantee is transferred to an untested worktree or runtime.

## Actual execution

Luna executed only approved commands in dedicated Docker QA: pinned image
`2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`, UID1000,
capabilities0, NNP, seccomp2, no external network, read-only original source,
toolchain and public cache mounts, allowlisted empty process environment,
4CPU/8GiB RAM/swap0/PID256, private 4GiB/65,536-inode scratch and bounded files.
Cargo used frozen/offline dependencies and its exclusive checkout/target;
revisions changed only after actual terminal and full source checks. No native
project execution or real provider/model/engine/cluster was used.

| Composition | Full source manifest SHA-256 | Result |
|---|---|---|
| First RED, 2,328 files | `e2471b0a3904dc90677e1da5c0ed196a7917f3078b999d2203c2ddc0e776468f` | 3 selected, 3 expected assertion failures, RC101 |
| First FIX, 2,328 files | `cc4115c0c067f64b645fe80ba69267820d7be38229690fea083785aa30395389` | 3 new parents, 2 existing storage and 1 controlled gateway parent PASS; all-targets check RC0 |
| Addon RED, 2,328 files | `2bbd0c5e2dcce7332b1bc96ca8b04a5b9beb8ab1bfeafcac08d8bd55b727e3e0` | 1 router/issuer parent then 2 multi-handle parents reach their expected assertions, RC101 |
| Addon FIX, 2,328 files | `51c842c3fd6678c086caed8ee72009ba4411165ba5b1d703c618d6ae0f1e071a` | Same 3 parents PASS; all-targets check RC0 |

**Nine unique test functions**, not a test matrix or a 100-worker measurement.
The first corrected composition has six functions (3+2+1), not seven; the
test-agent count error was corrected from the raw outputs without rerunning.

### Discriminating observations

The first RED accepts a Pending intention without its terminal record, permits
an unrelated append to spend reserved records, and permits byte-budget
consumption. The first parent combines record/byte policies: its record failure
prevents the later byte-only admission case from running in RED. Both policies
run in GREEN; separate byte-reserve parents supply independent RED observations.

The addon routes one legitimate refresh through the real Axum router and the
existing controlled loopback issuer. A real synthetic PKCE login seeds three
records, an Active durable binding and one `/token` exchange. On old accounting,
the refresh returns `503 audit_unavailable` **after** a second token exchange:
the assertion observes 2 requests instead of 1. On the fix it returns the same
503 before another token request; the ledger/head/binding remain the original
three records and Active generation. This is not a real Keycloak/provider or
TCP-peer cancellation qualification, nor a claim about volatile old-bearer use.

Two separately selected parents open both public SQLite handles before either
Pending and alternate transactions. They protect two record/byte allowances,
cross-complete sessions, replay exact receipts and reopen with both Closed.
They do not exercise simultaneous transactions or multiple processes.

### Commands

```sh
cargo test --frozen --offline --test authentication_terminal_reservation -- --nocapture --test-threads=1
cargo test --frozen --offline --test oidc_authentication_audit ledger_refresh_pending_is_durable_and_has_one_exchange_winner -- --exact --nocapture --test-threads=1
cargo test --frozen --offline --test oidc_authentication_audit ledger_close_is_terminal_even_for_a_late_refresh_response -- --exact --nocapture --test-threads=1
cargo test --frozen --offline --test oidc_gateway_flow cancelling_owned_web_refresh_future_cleans_pending_capacity_and_fences_late_response -- --exact --nocapture --test-threads=1
cargo test --frozen --offline --test oidc_gateway_flow web_refresh_refuses_terminal_capacity_before_contacting_the_token_endpoint -- --exact --nocapture --test-threads=1
cargo test --frozen --offline --test authentication_terminal_reservation two_open_handles_ -- --nocapture --test-threads=1
cargo check --frozen --offline --all-targets
```

The first command selected the original three parents before the addon; the
addon used only its exact one-plus-two selections. Checks ran at the end of
each corrected composition, RC0 in 37.27s and 23.95s. Warnings remain (lib80,
lib-test8 including duplicates, trust test2); Clippy remains NOT_RUN because the
pinned component is absent. No warning-free or production verdict is claimed.

### Raw evidence fingerprints

| Raw result | SHA-256 |
|---|---|
| Original RED3 | `235a324da5bfae8a22444b15699666b8bb24bdc7723ee1f770f43112aef0d5e7` |
| Original GREEN3 | `17eb5499d18a2124f1fad0b95eafb5055f7d83e988449da0cdac0dc571f7f89b` |
| Existing pending | `e0ade84c0ac484222f716be699dc47117e648a62d475ee91aa897c2a74a073c0` |
| Existing close | `de7a8e4fe0e19148e57ba145a4521f725ba9433aa00d7eb86a6c3bb80caf4d7a` |
| Existing controlled cancellation | `75b64b16b91daf4b1da455e44f028383b901bced0a871f9e2c434d4560f77ec4` |
| First all-targets check | `0b09858c4a75a0f4e6cef5ffa1d215b589d92cc72d394d186d8c18c539836be0` |
| Addon RED router/issuer | `a250f32c5a2824a4da806467c3c77e7d6ee85d8f7838cd7dedac6353ab871fd0` |
| Addon RED2 handles | `f2fe68d77841ba74add26e1344290c404b8324c70a7889817e0a81ad50d4bc53` |
| Addon GREEN router/issuer | `c93621bdcadd25159fb495ce713e05316bb254974b63979c11fcf87a303a9e22` |
| Addon GREEN2 handles | `ec2989ee98e75faf6e11262512cc8be6c3a2885db5f3f211e1bdceae10ca8736` |
| Addon all-targets check | `d10addfa6c564c8a34f15d42f8d6c40545be9c956cd9ee48db6a3c357702eed3` |

Private source patches, reviews, frozen RED/FIX archives and bounded raw outputs
are retained outside the worktrees. Final source archive:
`372f114d5a0f6fff7d75e56dc4149fac78ade5249333f8dd789446e496d2d54a`.
The QA was explicitly stopped after every Cargo terminal, PID0/OOMfalse,
23:14:49UTC; its independent watcher ended RC0, then removal/absence was verified.
No user data, volume or unrelated service was removed.

## Remaining limits

- Historical underfunded Pending still needs an old-writer/new-reader fixture under the identical policy; reopen is not a repair.
- Mixed old/corrected writers, simultaneous transactions, multi-process and physical crash/disk-fault scenarios are not qualified here.
- NV-02 cumulative global capacity, NV-04 lookup/replay and NV-10 anonymous intent limits remain open. The per-refresh allowance is not tenant budget isolation or retention/rotation.
- Actual provider, Docker/Podman/Kubernetes, 100-worker, signed checkpoint and production gates remain separate requirements.

The [audit registry](../../../AUDIT_FOLLOWUP.md#audit-multiworktree-20261108)
records this as software progress with residual gaps, without rewriting the
primary audit or closing unrelated findings. EN/FR SSO guides describe the
new capacity behavior and the unchanged immutable-policy/recovery limitations.
