# Worker-local relay helper — 7 November 2026

Software component prepared from `316f95de3da944784ccfe0f6c6ba63612334db52`
in worktree `codex/relay-worker-helper-20261107`. The tested final input is the
composed frozen snapshot **relay-helper-register52**. Helper SHA-256:
`2b6cfdc611fe0c753eeabfc4a91f8df6c37d1715e4d2a6462cbd8f06c50f12bb`.
The eleven-test source is
`2849fef0d5c3656ce8e294fbf32c4de94f0ea3111c8e20efaa11dc5743277ef5`.
The [nine-entry manifest](source.sha256) covers this lot's five code/configuration
files, both Cargo inputs and the finalized guides
[EN](../../../../setup/relay/HELPER.md)/[FR](../../../../setup/relay/HELPER.fr.md).
Guides were written after tests and are not historical compiled inputs.
This proof README is not a build input.

This manifest is a lot-specific subset, not the complete composed checkout or
QA toolchain manifest. Matching these files does not transfer all results to a
new integration, worktree or image.

## Delivered component and limits

The internal `ai-guard-relay serve-v1` binary uses fixed non-TTY stdin/stdout
and `/home/agent/.ai-guard-relay`. It receives no workload proof, tenant,
control URL or engine invocation. The Dockerfile builds and copies the helper
to a root-owned `0555` image path; **that worker image was not built or run**.

Hello/ready completes before a fresh namespace/socket and immutable versioned
record are published. The helper reuses existing `PrivateListener` filesystem,
inode, peer, path and 128-generation guards, not its broker/STOP frame protocol.
Its relay codec keeps separate frame and DTO bounds. One async owner uses
nonblocking owned FDs, preserves partial reads across select cancellation,
rejects extra bodies while pending and closes local listener/clients at control
EOF/close. Existing entries remain.

The priority order is control, current IO/deadlines, extra accepts. This is
design hardening following independent review, **not a demonstrated starvation
vulnerability fix**. The bounded canary neither establishes starvation under
continuous traffic nor refutes its possibility.

Authenticated expected inbox endpoint, current workload authority, durable
mutation retention, mandatory audit and HTTP dispatch remain control-side work.
No endpoint or identity is invented by the helper. Runtime exec ownership,
binding/image/StartedAt admission, outbox, aggregate buffer reservations, stderr
drain and child lifecycle are not wired by this lot. EOF/timeout never proves
rollback, worker terminal state, quota release or an observed effect.

## Frozen sequence and observed results

All eleven test functions and oracles are identical in 47, 50 and 52.
The TMPDIR-respecting fixture was present before the 47 run. The earlier
hardcoded `/tmp` fixture was unsuitable for the admitted QA filesystem and was
corrected without application change; it supplies no behavioral RED.

| Frozen input | Check | Observed result | Parent output SHA-256 |
|---|---|---|---|
| 47, original select order and `AsyncFd::new` | Bounded pending-control-EOF canary only | PASS 1/1, 0.01 s; build 1 min 59 s | `31a766c56a6ca8d51e6d5ddcb5aaff7d52910ef5c9efbb596539f92a8de6744d` |
| 50, three select priorities hardened | Eleven helper tests | PASS 11/11, 2.10 s; build 2 min 04 s | `e58a6cf716b582863a07e97bf39904d6e9bbd9b347bb068df6c8cf2701bc952c` |
| 50 | All-target check | PASS RC0, 1 min 06 s; deprecated unsound `AsyncFd::new` warning present | `aae3df42c5cbe9454f6bca16697a348069546a2c54e86f266c1c733e7bf98799` |
| 52, explicit registration contract | Same eleven helper tests | PASS 11/11, 2.06 s; build 2 min 04 s | `59b13fa16fb8c83fcb1e4cbe7e6db043720509045b652089cffe505a40617edc` |
| 52 | All-target check | PASS RC0, 1 min 07 s; deprecated API warning absent | `42e51fb8ed68454a140e46c5af5ef2e629f622dd6faf421f28301736b9e4b30e` |

**Eleven unique helper functions pass on 52**, not twenty-two from adding
50/52 or another function for the historical canary. The other 617 library
tests were filtered out, not passed. Selected command forms:

```text
cargo test --offline --locked --lib relay::channel::helper::tests:: -- --test-threads=1
cargo check --offline --locked --all-targets
```

The 47 selector was
`relay::channel::helper::tests::pending_control_eof_joins_with_bounded_ready_extra_connections`.
It observes one real Request on the control socket, then uses a test-only owner
polling gate to make real control EOF and sixteen fixture UDS connections ready
together. The parent owns those connections and at most 16 KiB of encoded
requests. After resuming the owner it checks a five-second join bound, no second
dispatch and closed clients; timeout cleanup aborts and joins the task.
There is no remote traffic, detached producer or continuous-load campaign.
The original-order PASS remains PASS, never RED.

The final eleven cover publication after handshake, a real local
inbox-request/closed-error round trip, EOF and matching close, extra connections
and a second frame refused while pending, partial control prefixes across select
branches, real inactivity expiry, wrong response epoch, discovery-record symlink
substitution, generation cap/exclusive ownership and an actual OS pipe reaching
EOF through the nonblocking adapter. The substitution parent mutates the record;
it does not establish every socket-substitution race despite its broader
historical function name.

Independent review accepted the guards, priority hardening and registration
delta. On 52 the test compile still reports two dead-code warnings. The
all-target check reports 61 library, two SSO-test and two library-test warnings.
This is not a zero-warning or Clippy result. No Clippy, release binary,
worker-image build or runtime transport test is claimed.

## Locked Tokio contract

`Cargo.lock` selects Tokio **1.54.1**. Its primary registry source was read at
`/Users/louis/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.54.1/src/io/async_fd.rs`
(SHA-256 `8d15003d40135ba61efb13a3403f086e4df3a5b59431662465dc5af300f7ae55`).

Lines 224–241 specify **unsafe**
`register(inner: T) -> Result<Self, AsyncFdRegisterError<T>>`: the same valid FD
must stay open throughout registration. READABLE|WRITABLE interests match the old
constructor. Lines 903–913 mark the old safe `new` unsound/deprecated and delegate
to `register`; lines 304–310 and 978–981 deregister before dropping the owned
inner object.

52 has an explicit unsafe block and justification: `File` owns the valid,
nonblocking FD; the private wrapper never replaces, extracts or closes it while
registered. Stdio duplication remains CLOEXEC. Error/drop handling still owns
and closes the File. No warning suppression, dependency change or raw-FD
lifetime extension was added. Eleven unchanged tests, including the real pipe
parent, ran again on 52 instead of borrowing the 50 result.

## Environment and retained material

Luna ran the isolated offline software checks; the source worktree executed no
native Cargo, project binary, Node or build.rs. Root's AJ admission retained
4 CPU, 8 GiB RAM without swap, 4 GiB private scratch, UID/GID 1000,
capabilities 0, no-new-privileges, seccomp mode 2, network `none` and readonly
root/source/public registry. No engine socket or external service was exposed
to the tests. Parent inspection `qa-aj-inspect.json` has SHA-256
`27b96dd937c512b6fe74c1c261f4b48b3a7db489532522a9cdda1f9948165ee5`
and records `OOMKilled:false`. QA resource admission and this observation do
not measure a real helper worker's RSS or runtime capacity.

Root retained these files under the private directory
`/private/tmp/ai-guard-cold-drain-logs-20261106.nXx9XJ/`; their hashes were checked
again during documentation:

- `relay-helper-repro47-eof-af.log`
- `relay-helper-hardened50-tests-aj.log`
- `relay-helper-hardened50-check-aj.log`
- `relay-helper-register52-tests-aj.log`
- `relay-helper-register52-check-aj.log`
- `qa-aj-inspect.json`

Frozen inputs remain under
`/private/tmp/ai-guard-audit-followup-inputs-20261106.xu8TFf/inputs/`,
named `relay-helper-repro47`, `relay-helper-hardened50` and
`relay-helper-register52`. Temporary private retention is not a product backup
or public evidence archive. No credential, database, executable, generated
worker payload or raw private log is copied into this evidence directory.

## Qualification left open

Sockets and pipe are real; the peer is a software test, not a worker reached by
Docker/Podman exec. The round-trip parent receives a closed error, not an
HTTP/Store inbox page. These eleven do not establish helper-to-HTTP send/page/ACK,
authenticated A1/A2/B1 delivery, audit/outbox crash recovery, stderr saturation,
runtime incarnation fencing, host/VM transport, Podman, Kubernetes, multi-host
operation, provider execution, 100-worker capacity or production operation.
Hosting software QA in a container does not qualify an ai-guard worker image.

The [approved direction](../../../RELAY_RUNTIME_HANDOFF.md) still requires
separate integrations and campaigns. Guides and image instructions do not make
them PASS. No production gate closes; the existing G09 failure is not resolved.
