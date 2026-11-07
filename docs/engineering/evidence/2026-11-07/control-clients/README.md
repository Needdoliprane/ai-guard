# Lot H — control clients SDK v2, 7 November 2026

Status: **22 targeted software functions PASS; all-target compilation RC0**.
The later [composition26](../control-recovery-composition/README.md) has its own
52-function software validation and all-target compilation. This is
neither runtime qualification nor closure of lot H or a global release gate.
The [transport contract](../../../INGRESS_CONTROL_TRANSPORT.md) describes the
fixed paths, roles, closed failure DTO and compatibility behavior.

## Frozen source and scope

Base: `b48c09551ac362bb66db2cdb102fadfbd59a8d19`, with the private Core18
`managed.rs` overlay. All selections below use the readonly `h-control-sdk-v2`
input and `/scratch/target-h-control-sdk-v2-aa` target. Root's original
19-entry manifest has SHA256
`c1fc021e7ffe56e91fd6e8424060750d26329ac6371d3c9cb72270647cf8cc84`.
The first 14 Cargo/Rust/test entries of the [source manifest](source.sha256)
are byte-identical to that gel. Its last five entries identify the guides
finalized after validation; they are not relabelled as tested input documents.
This evidence page and its manifest were also written after the runs.

The SDK selects Runner transport explicitly, retaining the authenticated
public Viewer protocol diagnostic. Presence uses only its three scoped routes
and preserves the exact durable Pending. Generic reads and declared capture
remain public; a refused control call has no public fallback. Monitor GETs
require the closed, versioned, status-consistent failure envelope. The SDK does
not extend the existing 60-second window, original deadline or STOP precedence,
and does not add mutation/start/claim retries. The start callback stays one
attempt. Mixed broker-outbox provenance remains OPEN.

Two independent source reviews accepted this bounded SDK change. Their scope
excluded Core18's private recovery-frame changes. Final proof and guide review
is also accepted; review does not execute the tests again or qualify a release.

## Observed runs and equivalent reproduction commands

Luna executed the named selections in QA AA. Raw stdout records test names,
counts, build times and test durations, but not the historical argv. The
following are **equivalent reproduction commands**, not reconstructed command
logs. Run them only in the admitted isolated validation environment against
the frozen source. Each selection starts with `cargo test --offline --locked`;
the table gives its remaining arguments. Single-thread execution follows the
Root/Luna validation instructions. No additional tests are counted from the
all-target check.

| Selection arguments | Functions | Test duration | Build/cache preparation | Log key |
|---|---:|---:|---:|---|
| `--lib runner::control_transport_tests:: -- --test-threads=1 --nocapture` | 4 PASS | 0.25 s | 1 min 56 s | clients |
| `--lib runner::event_transport_tests:: -- --test-threads=1 --nocapture` | 5 PASS | 4.20 s | 0.43 s | events |
| `--lib runner::managed::control_monitor::tests::private_worker_http_ -- --test-threads=1 --nocapture` | 3 PASS | 0.03 s | 0.33 s | http |
| `--lib broker::presence::http_tests::presence_ -- --test-threads=1 --nocapture` | 2 PASS | 0.31 s | 0.30 s | presence |
| `--test broker_control_info -- --test-threads=1 --nocapture` | 5 PASS | 0.29 s | 51.26 s | diagnostic |
| `--lib broker::service::server::cancel_retry_tests::stop_auth_tests::workload_schema6_keeps_fresh_broker_auth_and_inactive_admission_guards -- --exact --test-threads=1 --nocapture` | 1 PASS | 0.16 s | 0.35 s | stop-corrected |
| `--test runner_file_audit lost_file_ack_replays_exact_retained_event_without_runtime_or_duplicate_change -- --exact --test-threads=1 --nocapture` | 1 PASS | 23.52 s | 4.59 s | replay |
| `--test runner_file_audit command_started_callback_uses_one_attempt_and_stops_its_owned_instance -- --exact --test-threads=1 --nocapture` | 1 PASS | 17.18 s | 0.67 s | callback |

The first STOP filter,
`broker::service::stop_auth_tests::workload_schema6_keeps_fresh_broker_auth_and_inactive_admission_guards`,
selected **zero tests**: the intended check was **NOT_RUN**, not PASS. Its log
is retained as `stop`. The corrected historical invocation used the unique
function suffix without `--exact`; stdout identifies the complete module path
shown in the equivalent command above. It selected one function on the same
source, without weakening its assertions or changing product code.

`cargo check --offline --locked --all-targets` completed RC0 in **1 min 05 s**.
Warnings remain: **119 library, two `human_sso_gateway_trust`, three library-test**.
No Clippy result or warning-free build is credited to this checkpoint. All 22
functions are distinct; the zero-selection STOP attempt adds none.

## Retained log fingerprints

Root read and hashed the raw stdout, retained privately as
`h-sdk-v2-<key>-aa.log`. This documentation pass independently checked the same
hashes. No raw logs, database, token, callback query or runtime artifact is
published here.

| Log key | SHA256 |
|---|---|
| clients | `2dbe97a16938aecd0712d520c6bafb9d91126a8aef0b4ec0b475deabcef4302f` |
| events | `a58e83ce6d6ff0c4ea02b9871a7b3bb28a33420cf2cc4accc1f12fb1bd37fa67` |
| http | `bb23b73af0822907b703e9d3a555cfb514214abdaa79465374f33ad6eb18c6ef` |
| presence | `6bec27ba49b4ad63859309e42d662e9ef3c1c2f1062972746b41e3dacce399f2` |
| diagnostic | `037d641772847953fa5564541e84e8213adc3062f2069cb239d7b29512594ee1` |
| stop, zero selected | `906780813c5b7fb4aff21fead9ddb8ec6937a51a640166d4673fde21483d6547` |
| stop-corrected | `08ccf5b242783bc2ed319a6b7ce4dd8b9249e87c57d376b79ebe2ab0b77f1c4f` |
| replay | `e6b7657d4dbc8883e3bf44783bb3c1d82d732b6e9b9f3d5bfdf8062447dcf096` |
| callback | `f09eeca8404705095f7d6166f20f0ee139f3e0282a75a627c6a509496a9010ec` |
| check | `31999142c17b1fb7626b54a911d0fcfb8a539eed020a504ce9ee1d6d10a3edb6` |

## Environment and limits

QA AA used the local Linux ARM64 Docker validation image
`sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`
with the pinned Rust 1.99.1 toolchain. Its retained configuration has UID/GID
1000, network `none`, readonly root and source/registry mounts, dropped
capabilities and no-new-privileges. Configured limits are 4 CPUs, 8 GiB RAM
without swap, 256 PIDs, 4 GiB/65,536-inode scratch, a 512 MiB file-size limit,
1,024 FDs and no core dumps. These are the QA container settings, not measured
resource needs or a qualification of ai-guard worker confinement.
Configuration evidence `qa-aa-inspect.json` has SHA256
`e435f93a272e9d930740d2261a6f010c9564d69d36eff7c5704e4389218b17a7`.

The selections exercise real Rust clients, HTTP routes and SQLite receipts,
with synthetic identities, observations and local fixtures. They do not
qualify Docker/Podman/Kubernetes workers, cloud, enterprise TLS/PKI, provider
clients, multi-host routing, unlimited TCP/FD pressure or 100-worker capacity.
The earlier server H proofs retain their own source and are not added to the
22 client functions.

Core18's historical local STOP check is separate; its adoption path ended in
SIGABRT and remains unqualified. These runs give no result to Core24,
Root238/M or another checkout. They do not qualify broker-outbox provenance,
the full monitor recovery lifecycle or a release. The later composition checks
linked above are separate observations on their own source.
