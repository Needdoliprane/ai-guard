# Durable relay retention and capacity advice

Twenty paths compose the relay storage, HTTP/configuration and operator slice
on `e95724a0cedbf1b2cbfed6c1e7527c4a0cb57807`. Tenant, authenticated sender
and directed-grant budgets count retained bodies and bytes. Cleanup and sends
retain their durable audit requirements. Exact accepted replays do not consume
another share. The policy and maintenance procedures are documented in
[English](../../../../setup/relay/RETENTION.md) and
[French](../../../../setup/relay/RETENTION.fr.md).

Independent source review found two issues before publication: capacity advice
could depend on a cleanup audit outcome, and reconstruction accepted a body
release by ACK at/after expiry although the write path refused it. The revised
advice uses only the relevant policy/own scope; the ACK transition now enforces
the same strict expiry condition. Existing audit refusal and legacy ACK cases
remain preserved. The second review accepted both changes; the remaining
public enum comment was then aligned. No primary audit finding is closed by
these review corrections or by the earlier Claude results.

## Executed checks

The build and test commands below ran in the admitted Linux ARM64 QA.

```text
cargo test --frozen --offline --lib --test relay_fairness --test relay_retention_startup --no-run --message-format=json
<actual-test-binary> <exact-selector> --exact --nocapture --test-threads=1
cargo check --frozen --offline --all-targets
```

| Phase | Observed result, 11 November UTC |
|---|---|
| Build once | RC0, 11:06:11–11:12:04 |
| Three binary lists and nine exact selector resolutions | RC0, before the first test |
| Nine selected parents, once each | Each RC0, 1 passed, 0 failed, 0 ignored; 11:12:04–11:12:28 |
| All-targets check | RC0, 11:12:28–11:13:59 |
| Post-run source and live export | RC0; 2,422 exact regular source files, checked before stopping |
| Root's trusted rustfmt on the staged source export, and Git staged diff check | RC0 on the host; no project code executed there |
| Complete job including cleanup | Application RC0, closure RC0, final RC0 |

Exact selectors in `relay_fairness`:

- `small_budgets_bound_each_sender_and_grant_without_touching_the_neighbor_or_tenant_b`
- `byte_budget_is_enforced_and_tenant_refusal_hides_neighbor_usage`
- `refused_send_commits_bounded_cleanup_and_the_retry_progresses`
- `audit_exhaustion_blocks_cleanup_and_send_without_untraced_mutation`
- `tenant_ceiling_advice_stays_pessimistic_with_live_expired_and_mixed_neighbor_bodies`

In `relay_retention_startup`:

- `registry_policy_is_applied_to_its_own_tenant_only`
- `refused_retention_configuration_stops_the_gateway_before_any_store_exists`

In the library:

- `audit::relay::retention::tests::acknowledgement_advice_depends_on_the_refused_own_scope`
- `audit::relay::tests::message_update_rule_never_restores_a_body_and_keeps_release_on_ack_narrow`

## Source, execution and integration

The corrected R3 source manifest is
`1873f6a4e0615561eb93dd97d700a75539253d1e9a3d153b520abf59b38a6939`;
input archive
`23cd499c5d017dbc8e9620c6b277845c2df73757c839f78bdcd7b3ebc3259412`.
The genuine post-run archive, exported while the container was alive, is
25,743,360 bytes at SHA-256
`6dfaac6f8e46d2a4e8e1f5ee0671685ae4d8460e4bf9cef41fd8bf0607cf065c`.
Root independently parsed its USTAR entries after termination and verified the
exact pathset and every source payload against the frozen manifest.

The pinned Rust 1.99.1 image ran as UID/GID1000 with read-only root/two public
inputs, no network, no capabilities, no-new-privileges, default seccomp and
private IPC/cgroup. Limits were CPU4, RAM9GiB with no swap, PID128,
file-size512MiB, nofile1024 and scratch4GiB/131072inodes. Each app command used
`timeout`, `env -i`, scratch-local HOME/cache/target, jobs1, no incremental
build and dev/test debug0. Five dependency pins were verified before building;
no network fetch, provider call or worker runtime was used. Peak cgroup memory
was 6,620,934,144 bytes, with zero OOM events. The exact QA container was stopped
after evidence collection, inspected PID0/OOMKilled=false, removed and verified
absent at 11:14:16 UTC. Its final137 was the explicit stop of the idle holder,
not a test exit or OOM result.

Earlier attempts remain separate: R3 built but expired before tests; R4 omitted
the jobs/debug settings and failed its file-size limit at linkage. No R4 test
ran, and its reconstructed input copy is not a post-run export. R5 restored
the reviewed settings without raising the 512MiB limit. The nine actual passes
above are counted once and are not added to historical results from other gels.

All twenty indexed postimages match the tested R3 manifest. The independent
124 working changes were preserved; `src/main.rs` received only the scoped
CLI hunks, and its additional broker work was kept outside the index.
Raw logs, source manifests, reviews and cleanup evidence remain in the private
engineering archive.

These budgets count retained content, not physical SQLite/WAL bytes or inodes.
Capacity advice is indicative, not a promise that a later audit/write succeeds.
The worker transport, typed stdio response path and complete broker recovery
retain separate integration and runtime qualifications. No real Docker/Podman
worker, Kubernetes session, 100-worker load, restoration or global production
gate is qualified here. The [earlier composed software proof](../relay-full3-composition/README.md)
keeps its distinct source identity and limits; NV-12 remains open.
