# Typed relay refusal: client and stdio components

This seventeen-path slice adds explicit SendV2 capacity refusals to the Rust
client and helper protocol. V1 keeps its existing wire and error behavior.
V2 is send-only, preserves the V1 business payload/operation identity, and
requires the exact response version, channel, epoch, correlation and operation.
Closed decoding discards remote free text and rejects invalid scope/advice
combinations. Tenant/grant-rate refusals disclose no neighboring occupancy.
An explicit replay does not erase an earlier unresolved dispatch.

The source composes public `fb956b764115613de48f2f2678925fa19e059dc8`
(supervision B, SetupV2 CLI and Core20 R3) with patch
`2d1e01798236da5b8e98d787a1783b5f02d2f8ac719be1f612b47734c3e362ec`.
Independent source review accepted the compatibility, validation, identity and
unknown-result behavior. No Full3 authority was imported into this slice:
the control module remains private and production admission still refuses
`AuthorityUnavailable`. This publication does not activate a worker transport.

## Executed validation

In the admitted Linux ARM64 QA:

```text
cargo test --frozen --offline --lib --test relay_fairness --no-run --message-format=json
<actual-test-binary> <exact-selector> --exact --nocapture --test-threads=1
cargo check --frozen --offline --all-targets
```

Two actual binary lists resolved all fourteen unique selectors before the
first test. Each selected parent ran once: RC0, 1 passed, 0 failed, 0 ignored,
11 November 11:38:39–11:38:51 UTC. The all-targets check then passed RC0,
11:38:51–11:40:22 UTC, on the complete public B + Setup + Core + wire source.
Root's trusted host rustfmt check of the twelve indexed Rust sources and Git
staged-diff check also passed; no project code ran on the host.

The library selectors were:

- `relay::channel::tests::send_v2_refusal_is_closed_opt_in_and_keeps_v1_capacity_exact`
- `relay::channel::tests::send_v2_requires_exact_response_version_without_changing_business_identity`
- `relay::channel::tests::send_v2_never_accepts_own_figures_for_a_neighbor_scope_or_rate`
- `relay::channel::helper::tests::helper_requires_send_opt_in_and_exact_response_version`
- `relay::refusal::tests::published_refusals_are_read_into_closed_values`
- `relay::refusal::tests::anything_else_yields_no_refusal_and_never_a_neighbor_figure`
- `relay::refusal::tests::a_failure_keeps_the_client_error_and_reads_a_refusal_only_from_a_capacity_answer`
- `relay::channel::control::tests::refusal_wire_tests::typed_send_reaches_the_actual_helper_without_erasing_a_prior_unknown`
- `relay::channel::tests::ready_must_echo_exact_binding_and_limits_before_requests`
- `relay::channel::tests::response_must_match_pending_operation_and_mutation_resource`
- `relay::channel::control::tests::explicit_replay_after_reopen_preserves_unknown_when_current_attempt_is_refused`
- `audit::relay::retention::tests::acknowledgement_advice_depends_on_the_refused_own_scope`

The `relay_fairness` selectors were:

- `worker_client_tells_a_retry_from_an_operator_call_and_learns_nothing_about_neighbors`
- `audit_exhaustion_blocks_cleanup_and_send_without_untraced_mutation`

The HTTP client case uses the real Rust gateway and SQLite with synthetic
worker facts. The helper/UDS case uses the actual software components with
declared HTTP replies. Neither is an executed container worker. The last two
Core regressions protect the current R3 audit/advice rules; historical passes
from earlier compositions are not added to this batch.

## Source and integration evidence

The freeze contains 2,456 regular files. Source manifest:
`e45a19238aef429b2ecaaa4e0d8ae020c4557d05aa3ee4ee3bcdc72b09e240a6`;
input archive:
`d6fabc59d4b75a10cf509d77f3816edde8788832092cdd550133308bbd817b10`.
The genuine source export, collected before stopping, is 26,583,040 bytes at
`de78426565ef595b23f056837a92f1694a8010c0117719209c341de6cfd23362`.
Root independently checked every exported payload and the exact pathset after
termination. The complete job, collection and exact-container cleanup returned
RC0; the stopped process had PID0 and OOMKilled=false, and its container was
removed and verified absent.

The QA retained the pinned Rust1.99.1 image, UID1000, read-only root/public
inputs, no network, zero capabilities, NNP, seccomp and private IPC/cgroup.
Limits remained CPU4, RAM9GiB/no swap, PID128, file-size512MiB, nofile1024 and
scratch4GiB/131072inodes. Launches used timeout and an explicit empty environment,
jobs1/debug0, scratch HOME/cache/target and five verified public dependency pins.
No provider, cloud service or original project mount participated.

The seventeen indexed postimages match the tested freeze. Four intersecting
Full3 working files were composed separately, preserving their authority, TLS,
deadline and journal additions. The public index retains the tested standalone
versions; the other 120 working changes are unchanged. The resulting Full3
working composition is not qualified by this standalone run. Raw manifests,
results, source reviews and preservation evidence remain private.

The new public Rust enum variant can affect exhaustive downstream matches, as
documented in the guides. This slice does not qualify production admission,
broker recovery, real helper retirement, Docker/Podman/Kubernetes workers,
A1/A2/B1, capacity100 or a release. The separate
[Core20 evidence](../relay-core-retention/README.md) and
[earlier Full3 composition](../relay-full3-composition/README.md) retain their
own source identities and limitations. NV-12 remains open.
