# Broker presence recovery — 6 November 2026

Source `9f3f96d8175b189776cc66e810b6a0e2bc3c0fa4`, base `d3300641`,
composed as `f0fa2ea8f6a33c7d1e28a7f87bd69927f5b8ca79` with the human TLS lot.
The five changed files match their tested source; all nineteen TLS/presence
files also match the fresh composition check. No new schema, role, configuration,
service or runtime operation is introduced.

## Changed behavior

The existing publisher stopped after four temporary gateway failures and did
not restart when the gateway returned. It now keeps the same actor, HTTP client,
stream, pinned binding and exact Pending. After the initial100/250/500ms waits,
it retries once every `min(interval_seconds,30)` seconds until successful ACK,
shutdown or a terminal refusal. A slow retry does not begin another short burst.
Network/IO permits are not held during the backoff.

Only `Transport` and generic `Unavailable` retry. Authentication, authorization,
revision/conflict, clock, protocol, capacity and unsafe local material remain
terminal. Identity/self/grant and retained files are checked before each POST.
A lost ACK replays the same operation, including when the gateway already
committed it. Existing audit, stream and grant limits still apply; this is not
a promise of indefinite unattended publication.

## Targeted evidence

Luna used the admitted offline Linux ARM64 sandbox, Rust1.99.1 and locked public
dependencies. Local HTTP, SQLite, filesystem and owned broker processes are
real. The tests launch no worker, engine operation, model, provider or external
service. The controlled gateway faults are test-only.

```sh
# Baseline: same first parent, before actor implementation, intentionally RED.
cargo test --locked --offline --lib broker::presence::http_tests::temporary_gateway_refusals_resume_in_the_same_actor_after_the_fourth_failure -- --exact --nocapture --test-threads=1
# GREEN: only the affected publisher module and its CLI integration target.
cargo test --locked --offline --lib broker::presence::http_tests:: -- --nocapture --test-threads=1
cargo test --locked --offline --test broker_presence_cli -- --nocapture --test-threads=1
cargo check --all-targets --locked --offline
```

| Check | Observed result | Retained file | SHA-256 |
|---|---|---|---|
| Baseline | Expected RED RC101 after valid compilation/setup: actor exits `Unavailable` after four503 | `broker-presence-recovery-red.log` | `faa3997a2d08f7b8f936d5a62ce15602549415231b5b186745f1c72af8245b5a` |
| HTTP module | PASS RC0,14 passed,1 ignored,41.51s tests | `broker-presence-recovery-green-http.log` | `cd4375aa62ee3c0e416b4ac4420246ea7688884be10940698d64b878209ccb87` |
| Broker CLI | PASS RC0,3 passed,25.20s tests | `broker-presence-recovery-green-cli.log` | `ece24b5d75573692d726963d54631a0841c0defb55d363c2cb68c2d0c738b938` |
| Five-file presence gel, all-targets | PASS RC0,37.49s;118 lib/3 lib-test warnings | `broker-presence-recovery-green-check.log` | `050f79517f1d36cbd685912a8c175b131c371174c66a384f94001e45dd86613a` |
| Actual TLS+presence composition, fresh target, all-targets | PASS RC0,51.86s;118 lib/3 lib-test/2 shared-TLS-fixture warnings | `presence-tls-composed-check.log` | `ad62bfb5f5eeefa47abd1ec34c4f3ada362f1ad508266b5cb75f6ac7c7b852ed` |

Seventeen unique functions passed. The ignored manual cost-observation parent
`publisher_cycles_one_real_heartbeat_at_history_depths_0_8_32` is NOT_RUN and
does not contribute to PASS. The pure validated-head canary is included among
the fourteen. No behavioral test was repeated for composition: sources are
byte-identical and the changes are disjoint. Clippy remains BLOCKED by the
missing component; compilation warnings remain an open G12 concern.

New oracles cover five refusals then recovery on the same actor without another
short burst, four lost ACKs after real commits with one central event, released
permits and no POST after joined shutdown, closed error categories and changed
private material. Existing canaries cover A/B scope, wrong commitment, reserved
receipt capacity, history substitution, incarnation, explicit rotation and
unresolved pending/grant changes. The real `broker serve` scenario uses the
same process across the outage and retains its operation before normal shutdown.

Root verified terminal logs/hashes, rustfmt/whitespace,24 guide links and no
known private key literal in five changed files. Independent source review found
no blocking application defect; guides [EN](../../../../setup/broker/PRESENCE.md)
and [FR](../../../../setup/broker/PRESENCE.fr.md) match this bounded behavior.

## Retention and limits

Private archive outside Git:
`/Users/louis/Documents/.ai-guard-engineering-20261104.RhClps/kubernetes-held-create-20261105/broker-presence-recovery-checkpoint/`.
`MANIFEST.sha256` has SHA-256 `c5a931cd036d2db2457365fe169d3279be53636332224c167c5b852e42837ef0`.
Root verified11 payloads,400 under a0700 parent: selected RED/GREEN source
archives, three file manifests, five parent-created logs and the review note.
No sandbox database, executable, wire response, credential or key is retained.

QA K used pinned image `sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`,
UID/GID1000/capabilities0/NNP/seccomp, networknone/loopback only, RO source,
toolchain and public registry, CPU2/memory6GiB/swap0/PIDs256, scratch3GiB/
65536inodes, bounded file/log sizes and50-minute owner lifetime. The terminal
RED compiler target alone was removed for capacity; source and logs remained.
All handles completed and no project process remained before stop/removal at
06:01:32.049367381UTC, PID0/exit143/OOMfalse; memory OOM counters were zero.
No user volume or persistent control data was removed.

Cadence was exercised at5s; the30s cap is source-checked. Rotation/revocation
during a slow wait, every HTTP/filesystem interruption, process loss and physical
durability are not proved by these tests. A visible receipt is not a power-loss
proof. Physical multi-host federation, worker inventory, runtime health, quota
release and start authority do not follow from a heartbeat. Full fencing, worker
relay, Kubernetes sessions and real runtime qualification remain open; G09 FAIL,
no global gate closed. See [validation](../../../VALIDATION.md).
