# Federation checkpoint service — 10 November 2026

The Rust broker service now supplies its authenticated checkpoint publisher
from an existing Full journal writer. Three discriminating REDs precede the
correction; independent source review and eight exact FIX parents establish
this software result. Other integration-worktree changes are excluded.

## Exact source and provenance

Public base: `8afcd32d9cc6a6c913a30ba059ee5e720e8fbd38`.
The tested composition contains Core50 dependencies, the R4 harness and the
three-path service FIX: 2363 files, full manifest
`e9e60e96293862d8d462e62263e0ab89f1ea649ae5906146f074db1ddcbcaa07`.
The full delta from that public base is 53 paths (36 modified, 17 added).
The FIX itself changes only `src/broker/service/server.rs`, the new
`src/broker/service/checkpoint_source.rs` and `src/broker/presence.rs`.
The three-path manifest is
`d8617318abc3c8a33105a2765bc355aba5a9fcd3e39fb67bcc3312d40b9436d3`.
The R4 child remains byte-identical:
`5a1e61ddd51115c64aa0bbbc16d0ac3ed5b3ad8c7d3ffec5513d1be20ee7651c`.

Independent source review `fed-service-fix-review-20261110/REVIEW.md`:
`7b4ebcd1eccc41bb2832ebac57cc7f6159dba255633ccc3aa577790035ba140d`,
GO_SOURCE bounded, without transferring a behavioral result from that review.
The final EN/FR operator-guide delta is separate from the compiled source.
See [checkpoints](../../../../setup/broker/CHECKPOINTS.md) and
[présence](../../../../setup/broker/PRESENCE.fr.md).

## RED and FIX results

On the separate R4 prefix (2362-file manifest `87622b31…`), three exact parents
each fail RC101 after the real fixture and cleanup: the first two reach
`service presence assembly exited before real publication: Some(Ok(()))`;
the third reaches `service shutdown returned while actual admitted FS remained blocked`.
They establish missing service assembly and abandoned drainage, not a runtime
or physical-crash reproduction. The R3 migration parent
`audit::broker_checkpoint_migration_tests::migration_11_to_12_preserves_held_quota_and_exact_backup`
passes once on its own source; its following service attempt stops at fixture
Conflict before the feature. That one migration PASS and the older 39 results
remain separate and are not added to the eight FIX parents.

The FIX uses these exact selections, one invocation and one PASS each:

```text
broker::service::server::checkpoint_source_tests::service_checkpoint_uses_real_full_scope_and_durable_head_without_inventing_empty_source
broker::service::server::checkpoint_source_tests::service_checkpoint_keeps_pending_one_until_lost_ack_replays_before_real_head_two
broker::service::server::checkpoint_source_tests::service_checkpoint_shutdown_retains_real_fs_and_cancelled_observer_until_rejoined
broker::presence::http_tests::run_owner_tests::stop_and_cancelled_join_waiter_retain_actual_fs_work_until_rejoined
broker::presence::http_tests::run_owner_tests::terminal_checkpoint_refusal_stops_sibling_but_waits_for_its_fs_join
broker::presence::http_tests::run_owner_tests::owner_start_without_tokio_is_a_typed_refusal
broker::presence::http_tests::presence_uses_only_its_three_control_routes_with_the_same_durable_pending
broker::presence::checkpoint_storage_tests::checkpoint_config_without_a_real_source_is_refused_before_network_publication
```

| Operation | Actual result |
|---|---|
| Initial cold lib build, 10:33:10–10:38:16 UTC | Cargo printed Finished/Executable; **Cargo RC UNKNOWN** because the host wrapper used an absent `/bin/printf` then unconditionally exited 0. The empty RC file and original raws remain. |
| Unchanged cached build verification, 10:42:52–53 UTC | Actual RC0 with corrected builtin printf and propagation of the tool's real RC; not another cold build. |
| Eight exact FIX parents, 10:43:41–10:46:31 UTC | Eight unique PASS/RC0, no ignored test or prefix selection. |
| All-targets check, 10:46:53–10:48:14 UTC | RC0, warnings retained. |
| Post-source | 2363 hashes OK, exact pathset/nonregular controls RC0; OOM/kill events zero. |

Commands were the frozen/offline lib no-run build, exact
`cargo test --frozen --offline --lib EXACT -- --exact --nocapture --test-threads=1`,
and `cargo check --frozen --offline --all-targets`, within Root's admitted
TERM/KILL and resource bounds. No application command ran on the maintainer's
host. The initial pathset comparison error is retained separately; the corrected
explicit NUL pathset check and final post-check are exact.

Private direct raws and `ROOT_TEST_INDEX.json` identify all eight names and
stdout hashes; the index SHA is
`d2cf62c327d73407dddbfb788638aca9af725cb275051ccc9ce135ca5f214476`.
The actual post-QA source export is 24176640 bytes, SHA
`3efbd616d7bb023e1f29adde5335a5a48e70e90e7092841e19354cbf6c808431`;
all 2363 regular members and contents were checked against the full manifest.
Closure report SHA:
`ab80f13585a267e053e54aebc460aa04cc229edb943d2d6d5eac90e938e3e3a2`.
After only the idle lifecycle process remained, Root stopped and removed the
owned QA normally at 11:17:59 UTC. Its explicit-stop exit137 is not a test
failure or OOM. Evidence, source and private configuration were preserved.

## Delivered contract and limits

The source samples only the profile's existing Full writer with
`list(None,100)`, matching installation/profile/tenant/namespace/canonical
gateway before and after its real ACK. No writer or job is created to populate
the view. Missing/foreign Full, failed/tainted reads or stopping/fenced control
stay unknown; a known empty source requires a successful list ACK. At most
100 sorted unique durable head references are declared, with conservative
has_more and always `coverage:declared_partial`. The FIFO cost is real and
no new latency/fairness or sustained-capacity guarantee is established.

POST `/api/v1/brokers/checkpoints` and GET
`/api/v1/brokers/checkpoints/self` use the existing `/control` transport,
a verified Broker workload proof and current matching grant. Technical
runner/viewer registry reads and the human GET `/api/human/brokers` with explicit
`read_broker_registry` and authorized tenant context remain distinct; they do
not authorize checkpoint writes. Activation uses Registry2 `presence_file`
with Presence V2 and explicit backed-up migration 11→12, without activating
execution or granting a new authority.

The real Journal/Writer/filesystem and loopback HTTP are exercised with
controlled barriers and an intentionally omitted ACK. The living service
retains source/publisher owners through the actual join; cancellation of a
join observer does not remove the owner. Waiting establishes **no physical
filesystem collection bound, fencing or recovery after process loss**. Drop
requests stop without proving a join. Joining the source future also does not
physically join the writer thread after an ACK timeout.

Linux ARM64/Rust1.99.1 isolated QA: 4 CPU, 9 GiB RAM, no external network,
UID1000, RO root, no engine socket or user credential. OOM0; observed memory
peak 5847695360 bytes. These eight tests do not qualify multi-host federation,
cold restart, real Docker/Podman/Kubernetes workers, transport/PKI matrix,
100 workers, throughput or physical crash. Clippy and compiler warnings remain
separate. **G09 FAIL; 0/12 global gates closed.**
