# Broker / Full3 — five settlement parents, 13 November 2026

[Français](README.fr.md) · [Typed tests and commands](TESTS.json) · [Broker supervision contract](../../../BROKER_SUPERVISION.md) · [Broker guide](../../../../setup/broker/README.md) · [Relay handoff contract](../../../../setup/relay/BROKER_HANDOFF.md) · [API contract](../../../../api.md)

Five exact Rust library parents for normal retirement settlement passed on the frozen public+E1 composition. The lib-test build and subsequent all-targets check passed, with **35 build warning diagnostics and 343 check warning diagnostics**, respectively. These are software results, not strict Clippy, full recovery, fencing or real-runtime qualification. No production gate or historical audit finding is closed.

## Source and evidence

Candidate `223b699e959964bef4460f34857516cb8cdd37c8` is composed on public `4cd336c9dd51cc7fa8f81d50ab25788dc32b843f`. The 136-path broker input frame and earlier source review predate this E1 composition. Root's composition metadata records 134 effective broker changes, two unchanged owned paths and all 37 E1 paths preserved. The current result belongs to the following exact 2,707-file archive, including its retained modes; it does not transfer to another checkout or later publication commit.

| Record | SHA256 |
|---|---|
| Historical 136-path input frame | `d4e4402a898cc6ad728c1bcf807bf69e05ae7e13175c7485a64affd04d0e1a37` |
| Earlier bounded composition review (Markdown) | `32dfd746c74a0198fb9b471cab6a5db044136f42dd7bcd77de908c4a72ee622e` |
| Current independent plan review | `b6b0e8668c6f939e724e1c9d75ffc4d7347ec82fab91890fe9dea234dd3d694e` |
| Current input source, 2,707 files / 32,675,840 archive bytes | `d662f5c7f51b4ce8003614803ed87620484485933f39d251d7550b278136abfe` |
| Root-verified final result | `2bc5dab9d4934fc98b34013554c11a45592c08faf572f73c4435706f85773412` |
| Post-run source, 32,860,160 archive bytes | `1df33e3943dea9acf66e877a681fdff410576e0e5f787f11a04ef9246d7550c7` |
| Private raw-evidence manifest, 293 files | `db712410daaef48d3a4e67f94c793c43e77f5ff7e8ff8477eb30bf131934db08` |

Source archives, original reviews, raw traces, fixture data and credentials remain outside Git. This public summary contains no raw fixture content, private host path or API response. Its fixture statistics come from the final Root-verified receipt; the publication writer did not read, decode or extract fixture payloads.

## Five observed parents

All five returned RC0 with one passed, zero failed and zero ignored after a fresh exact-one list. Their full selectors and actual executable argv are in [TESTS.json](TESTS.json).

| Case | Bounded behavior exercised |
|---|---|
| Original submitted Final after an unavailable Q result | The original retirement settles without creating a new Final. |
| Same public Final from another core | Public data from a foreign core cannot query or clear the original keeper. |
| Missing local slot without opaque completion | Absence alone leaves the original keeper retained. |
| Already retained verified input | Another query wrapper is refused. |
| Confirmed cleanup after cancellation | Generation is preserved and admission remains closed. |

The admitted fixtures use real private Journal/SQLite/config/audit files, an owned loopback gateway, private Unix IPC and same-process actor joins. The test-only `SyntheticReady` substitution returns before the business runtime path. No worker, engine or provider command was invoked. Invocation data naming a program is not evidence that the program executed. These five cases do not exercise a complete broker restart or the whole Full3/relay recovery path.

## Commands and collection

Luna executed one admitted run; Root independently verified the result, source equality, captures and closure. In the offline QA environment, the actual compile/check argv were:

```console
/usr/local/cargo/bin/cargo test --frozen --offline --lib --no-run --message-format=json
/usr/local/cargo/bin/cargo check --frozen --offline --all-targets --message-format=json
```

The harness resolved the actual lib-test executable from Cargo JSON. It listed each exact selector with `--list --exact`, then invoked that selector once with `--exact --nocapture --test-threads=1`, bounded to 120 seconds. No main CLI or other parent was executed. The all-targets check ran only after all five parents passed. An all-targets compile result is not a strict Clippy result or a full-suite test result.

The QA container was Linux ARM64 (`7.0.14-linuxkit`), Rust selection `1.99.1`, image `sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`, UID/GID1000, network `none`, read-only root, dropped capabilities, no-new-privileges, private IPC/cgroup, 4 CPU, 9 GiB memory/no extra swap and 128 PIDs. It had three tmpfs entries, including private 4 GiB scratch, and only two read-only input/cache binds. These are QA facts, not a qualification of business workers or runtime adapters.

Six final collections completed: exact post-run source, bounded source export, regular-fixture inventory, private opaque fixture export, kernel and cgroup observations. Root verified all 2,707 post-run contents and modes. The private fixture capture contains **96 regular files / 2,117,346 payload bytes**, in a **2,201,600-byte mode0600 archive**, SHA256 `23251f802d3f06071ed7b6962df2472cff767008803596ce2bb8915092c319dd`. It is not distributed. Both source and fixture capture pipelines retain their 32 MiB bounds and overflow checks.

All six collection and stop/remove codes were zero; exact-CID closure was RC0, the container was removed, `oom=0`, `oom_kill=0` and `OOMKilled=false`. The holder's recorded exit137 is separate from the RC0 test/check and closure results. The runner's provisional green was accepted only after Root's verification.

## History and open requirements

The earlier prewire parent passed once on a different 2,668-file source (`0d393fff…4725`), with receipt `d0e3584d95ea67777a129696b7d6393d2d44ce84587c90c3c576ed31b5486877`. It tested a normal listener/prewire case with synthetic runtime observation. It remains separate and is not a sixth PASS here. The 136-path frame frozen at 04:06 UTC retained **five private settlement tests NOT_RUN** on its then-private source `ceda17d8…94bd`; the later current composed run does not rewrite that historical status. Previously excluded namespace/restoration and NativeResume operation families were neither executed nor reconstructed by this run.

The six guides describe wider contracts. The API's historical reconciliation read creates no new permit or join proof. The broker Resume audit event is only a request; it does not reconstruct an original owner or timer from JSON. Relay closure needs exact durable evidence and local joining; a missing handle or terminal worker alone does not establish it. This record closes none of those wider acceptance requirements.

General fencing, cold/full recovery, namespace restoration, relay transport, real worker/engine/model/provider execution, inter-tenant network confinement, Docker/Podman/Kubernetes acceptance, physical storage/power-loss recovery, capacity100 and the complete operator journey remain unqualified by these five parents. Strict Clippy, full suite and release builds were not run in this campaign. No timing SLO, platform-wide confinement guarantee or production readiness follows.
