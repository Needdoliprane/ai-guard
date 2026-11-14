# Kubernetes attach composition — 14 November 2026

[Français](README.fr.md) · [Exact selections and receipts](SELECTIONS.json)

**Build/check and 11 targeted software parents passed on `1e30e5e`.**
Four contracts use the real owner/codec with synthetic peers. Seven tests
spawn the normal Rust `ai-guard-provider` binary with real stdin/stdout/stderr
pipes. Its internal waiting mode and argument handling actually executed;
no AI client or model, real worker, Kubernetes cluster or product network
control ran. No production activation is opened.

## Tested source and actual commands

Tested commit:
`1e30e5e8546ab614eba7849789a1a0468cbc2843`; tree:
`25f1b907c028a68679d4b9e02b8760e2aa4ea998`; input archive SHA-256:
`ec0216f959edd6189102378d0c3250270b26ebd60d8ed177ae74859df380281e`.
The later documentation/composition base
`3b538f7323141c4488baa031f93da5f9b0dc30cf` was not tested by this campaign.
The later [45179db inspection composition](../owned-policy-inspection/README.md)
has its own 47-parent evidence, including a repeat of these eleven checks.
This record remains eleven on `1e30e5e`; no counts or qualification transfer.

The admitted QA executed:

```console
/usr/local/cargo/bin/cargo test --frozen --offline --lib --test kubernetes_waiting_cli --no-run --message-format=json -j1
/usr/local/cargo/bin/cargo check --frozen --offline --all-targets -j1
```

Both returned RC0. No compiler-warning count is supplied by the primary
result, so none is inferred. Actual Cargo JSON binds three executables:
library libtest, `kubernetes_waiting_cli` integration libtest and the normal
`ai-guard-provider` binary with `profile_test:false`. The tests select that
normal binary through Cargo's `CARGO_BIN_EXE_ai-guard-provider`, without a
manual binary override. None of these opaque artifacts ran on the host.

Fourteen real lists precede execution: four global/ignored inventories,
two selected-stdio/ignored lists and eight individual-contract/ignored
lists. Four exact contract groups pass one parent each; the stdio group
passes seven exact names. All use `--test-threads=1 --nocapture`.
[SELECTIONS.json](SELECTIONS.json) retains names, argv order and artifact
hashes. There are **11 unique PASS, zero FAIL and zero selected ignored**;
this is neither a full-suite result nor a statement about unselected helpers.

## Observed contracts and subprocess outcomes

| Contract | Actual transport | Observed software behavior |
|---|---|---|
| 1. Initiated upgrade, caller lost | HTTPS upgrade request to an owned synthetic TLS loopback peer | HTTP work retains the attempt capacity until that work ends. Losing the caller is not an immediate capacity return. |
| 2. Blocked pong after Ready | `tokio::io::duplex(64)`, in memory | A real blocked write records a typed IO deadline. |
| 3. Cancelled blocked write after Ready | `tokio::io::duplex(64)`, in memory | The cancellation request remains a separate cause beside the observed IO deadline. |
| 4. Invalid close payloads | play→open→`tokio::io::duplex(pipe)`, in memory | The real stream reader refuses them. |

Only contract 1 exercises HTTPS/TLS over loopback. Contracts2–4 prove actual
owner/codec/reader behavior on memory streams, with no TCP/TLS proof. Those
peers are fixtures, not an API server or kubelet; no Pod instance is authenticated.

| Real normal-binary stdio case | Expected child exit / output |
|---|---|
| EOF before Hello | 66, no stdout |
| Hello then EOF | Exactly one correlated Ready, declared metadata; exit 66 |
| Extra argument | 64, no stdout |
| Non-UTF-8 extra argument | 64, no stdout |
| Truncated prefix then EOF | 64, no stdout |
| Prefix held while stdin remains open | 65, no stdout; writer still owned when the child is observed terminal |
| Non-UTF-8 first argument | 126, no stdout |

The seven cases use a cleared environment, a fresh private directory and
bounded pipes. The held-prefix oracle reads the actual terminal process
status while retaining its stdin writer; it does not create EOF to manufacture
the result. The child supervisor is 15 s and caps stdout at 16388 bytes and
stderr at 8192 bytes. Error cleanup kills and reaps the child before removing
only its fresh fixture. These are test bounds, not measured product latency.

The zero exit of each test is distinct from its expected nonzero child exit.
Ready remains an echo, not identity or business execution; EOF 66 is no
handoff, not a successful session. The stdio tests exercise no attach transport
or worker image. The four stream contracts are separate selections.

## Preservation, QA profile and verified receipts

Root verified the three Cargo artifacts and all 2805 source files' hashes,
sizes and modes. The post-run archive hash is
`b663b2ccc528e5142de8a9788c86baf1244ac388793fba7ae676a91e20ee5b7d`;
it is a different serialization from the input archive, not an assertion
that archive bytes match. The retained fixture export has zero regular
files and zero nodes; its archive hash is in the typed record.

All 56 recorded stage RCs are 0, including bounded producer/sink streams.
Closure returned 0 and the exact QA container is absent. The sleep holder
was stopped; that is not a worker crash or a test exit. The complete
host file/directory FD flush separately covers 493 regular single-link
files and 17 directories, 391272605 bytes, with no symlink. A flush does not
qualify power-loss or restoration behavior.

This was an isolated tool QA: pinned image
`sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`,
UID/GID 1000:1000, network none, read-only root, nonprivileged, capabilities
dropped and no-new-privileges; four CPUs, 9 GiB memory with no swap, 128 PIDs,
4 GiB scratch and 64 MiB noexec temporary storage, two exact read-only
input/public-cache binds. No engine socket, project, home or credential mount.
Measured peak 8960716800 bytes under 9663676416 bytes; OOM/OOM-kill 0.
These figures describe build/test QA, not worker memory, real confinement
or 100-session capacity.

| Primary receipt | SHA-256 |
|---|---|
| Actual result | `dd197af11edfd46da7c70de6ce1096320e349fb841b4e6f4ab96bf10418a4c74` |
| Complete host FD flush | `90e23f7b1986cbb90ba46fa2d3c9e6be1803f7fb9c9e5c67d9e99056e892b506` |
| Coverage precision addendum | `92a82687117f7c5eb56518a13506640a7398ee0f5f7d45044fd07b2df24ae90c` |

The coverage addendum clarifies each transport; it changes neither original
result/flush receipt nor counts, artifacts or tested source. The
profile-created receipt is also identified in
[SELECTIONS.json](SELECTIONS.json). Raw traces, fixture contents, credentials
and private paths are not published.

## Historical results and open limits

The 121 PASS on `be6b12dce7f74e2062f7ad5524e12818bf7313b5` are historical
and were not rerun on `1e30e5e`; they are not added to these 11. This evidence
does not transfer to `3b538f7` or another HEAD. The later inspection result
has its own source, commands and receipts in the link above.

Real Kubernetes, worker/image, AI client/model, product network enforcement,
Docker/Podman confinement, 100-worker capacity, strict Clippy and the full
suite remain NOT_RUN. S5, production effect permits, fencing, R4/R7/R10 and
production gates remain open. The sealed guard has no production
implementation and no product session path reaches attach.
The [EN guide](../../../../setup/kubernetes-runtime/README.md) and
[FR guide](../../../../setup/kubernetes-runtime/README.fr.md) retain the
closed activation, declared preamble, assumptions and qualification plan.
