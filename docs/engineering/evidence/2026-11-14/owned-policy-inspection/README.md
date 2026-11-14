# Owned policy inspection — composed evidence, 14 November 2026

[Français](README.fr.md) · [47 exact selections, argv and hashes](SELECTIONS.json)

**Inspection, historical CAS preparation and the real internal Rust CLI passed
on the complete `45179db` source.** An operator can explicitly retain reserved
registry metadata, review the private historical report, then derive a local
replace/revoke operation. The final A1 transaction remains authoritative for
identity, scope/CAS and the Session replacement ceiling. It never repairs an
old report or ambiguous operation. [Operator guide](../../../../setup/identity/OWNED_SOURCES.md#owned-policy-a1-operator)
· [API contract](../../../../api.md#owned-policy-inspection).

## Source and actual execution

Commit `45179db23205ff2a534bcadfafd694830f2a62ce`, tree
`810080f9933ac0f0ef630f3c1b99c600e9f804a5`, input archive SHA-256
`5379954066640a38ec91753d7adabd65577748145ba087a5347452e82423c1e5`.
The admitted QA executed these commands once:

```console
/usr/local/cargo/bin/cargo test --frozen --offline --lib --bin agentsandbox --test kubernetes_waiting_cli --no-run --message-format=json -j1
/usr/local/cargo/bin/cargo check --frozen --offline --all-targets -j1
```

Both returned 0. One fresh Cargo graph binds five distinct executables: library
libtest, agentsandbox bin-test, waiting integration, normal agentsandbox and
normal ai-guard-provider. Exact SHA/size/tuple receipts are in the typed record;
no prior executable was imported or run on the host. Two CLI source texts had
nonmutating stdin checks before Cargo; the corrected JSON test has a separate
exact-byte formatter receipt. Formatting is not a behavioral result.

Twenty-four real lists verify the selected names and ignored classes before
execution. Default --list includes ignored tests, so it is correlated with
--list --ignored. Each group uses --exact with all its names, then
--test-threads=1 --nocapture. Only cli-ignored appends --ignored and receives
the freshly bound normal CLI path/SHA. The typed record gives every actual
argument vector. All three initially ignored CLI parents ran; no selected
parent was left ignored, and unselected helpers are not counted separately.

| Group | Actual PASS | Scope |
|---|---:|---|
| baseline-api | 3 | Real observed heads/scope, early refusals and unavailable/stale observations |
| cli-ignored | 3 | Real internal CLI inspect/prepare/apply, retained conflict and lost-receipt recovery |
| core | 9 | Closed metadata DTO/SDK, reserved API slots, redaction and no-write observation |
| cli | 13 | Inspection parsing/report/draft, CAS/ceiling/revoke and publication failures |
| e1compat | 2 | Existing Session resource/ceiling and terminal compatibility |
| a1auth | 3 | Existing role/body/method boundaries |
| fs | 3 | Protected publication and retained physical ancestry/taint |
| kube-contracts | 4 | One HTTPS/TLS loopback contract; three in-memory duplex contracts |
| stdio | 7 | Actual normal Rust provider subprocess pipes/argument/EOF boundaries |

Total: **47 unique PASS, 0 FAIL, 0 selected NOT_RUN**,27 libtest / 13 bin-test / 7 integration;
44 ordinary and 3 default-ignored explicitly executed. Tests use real gateway,
SDK, SQLite and filesystem operations in owned fixtures. These are the product's
internal Rust CLIs, not Codex or Claude sessions. The provider's waiting dispatch
is actual Rust binary execution, without an AI client or model.

## Preservation and precise limits

All 2836 source files retain hashes, sizes and modes. Seven collectors, OOM 0,
exact-container closure/absence and a separate host FD/file/directory flush are
verified. No regular fixture file is retained; the primary inventory records
nine empty group directories/nodes. The regular-file tar is empty. The result
summary's fixtures.nodes=0 is not the full inventory count; both original
records remain unchanged and the inventory SHA is retained in SELECTIONS.
Per-phase files and a bounded terminal excerpt remain available; the full raw
orchestration transcript was not persisted or reconstructed.

The case-alias oracle actually emitted **NOT_APPLICABLE_CASE_ALIAS** on the
case-sensitive Linux filesystem. The physical report/REPORT alias branch was
not exercised; this does not qualify macOS or case-insensitive filesystems.
The two physical-ancestry FS regressions remain separate from that missing branch.

The four Kube contracts are one real HTTPS upgrade to an owned synthetic TLS
loopback peer and three memory duplex streams. They do not qualify an API server,
kubelet, Pod or cluster. The seven stdio parents do not exercise a worker image
or attach transport. The [earlier Kube evidence](../kubernetes-attach-composition/README.md)
retains its own 11-parent source. This run repeats those 11 within 47; it does not
rerun the separate 121 be6 parents or add historical counts.

The tool QA used Linux ARM64 in Docker, pinned image, UID 1000, network none,
read-only root, dropped capabilities/NNP,4 CPU / 9 GiB without swap and128 PIDs.
Measured peak 9209622528 bytes is build/test QA memory, not worker usage or a
100-worker benchmark. No real engine/worker, product network filtering, model,
Codex/Claude, cluster, strict Clippy, full suite or physical failure/restore was
qualified. E2/flow16 and Claude federation remain separate static branches,
not integrated or qualified by this source/run. **0/12 production gates closed.**

## Receipts and preserved history

| Primary record | SHA-256 |
|---|---|
| Actual composed result | `8da8e0a8a345a7993374f4170234d774771aed97434db75a13dee7639203204a` |
| Host FD/file/directory flush | `7ea97381c88406a8cb28b1bc1833268f6efabe8eccd4b79ae2172b46d2f1c51f` |
| Exact JSON text formatter result | `68df78f022cda7c1fa7a963af417ff0748e6442e66984b382eb78acf150b80a2` |
| Formatter flush | `f0aa6dc1fbb32bab276be9026f0cc893b5c96fded8913dde293ce3abe4e5496d` |

R1/e54 failed compilation with three E0308 errors before tests. R2/944 built/checked,
then 15 parents yielded 14 PASS / 1 test-oracle FAIL and 32 NOT_RUN. That oracle matched
the substring name inside the required control_namespace_uuid key; R3 changes
only the test to inspect exact JSON keys. No product wire leak is demonstrated
by that failure. R2's FINAL counter 6 summed passing tests only from its two fully successful groups; the failing
Core group's eight individual successes stay in its original summary. Earlier
records are neither rewritten nor relabelled as green.

The [74-parent A1 record](../owned-egress-policy-api-cli/README.md), published
via `1a9`, keeps its tested `db2678d` source. Raw traces, fixture contents, credentials,
private personal paths and opaque binaries are not published with these hashes.
