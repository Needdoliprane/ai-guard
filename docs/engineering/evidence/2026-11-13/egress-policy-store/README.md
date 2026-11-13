# E1 policy Store15 — software evidence, 13 November 2026

[Français](README.fr.md) · [Engineering guide](../../../EGRESS_POLICY_STORE.md) · [Typed commands/results](TESTS.json) · [Full egress requirements](../../../EGRESS_CONTROL.md)

The tenant-local Rust registry stores immutable policy revisions, CAS heads, exact operation receipts and logical terminal reservations in the existing tenant SQLite database. Explicit backed-up maintenance installs format15 from format14. An authenticated machine-control actor performs these private operations; the human owner of its binding is not an approver. This tranche adds no human network permission, CLI command, HTTP route, connection permit, DNS lookup, socket or applied runtime revocation. No user project or existing user database was migrated.

The observed results are **14 exact parents PASS on R5**, followed by **13 exact parents PASS and an all-targets check PASS on R7**. They are separate runs on distinct frozen compositions. R7 does not claim 27 PASS. Its all-targets check emitted **320 warning diagnostics** and no errors; it is not a strict Clippy result. No production gate or historical audit finding is closed. G05/G07/G11 remain open; existing bridge networking remains unmediated.

## Sources and observed results

Both compositions contain 2,630 files over public base `ceb7b1f1c298a0f31044eca20a09cb2101c4d7c2`, the 29 owned E1 postimages and four reviewed human-source corrections. Those four corrections were separately published in `328f3bc5cc042ebf7f8924c54d4e717d1de94acb`; the QA records do not assert equality with a later published tree. R7 changes only the parent15 body in `src/audit/egress/tests.rs` from R5. APP, helpers, the four human files and the first 14 test bodies are unchanged, independently reviewed. The E1 worktree itself stays on its original base; it is not the composed QA source.

| Frozen record | Result |
|---|---|
| R5, E1 R4 + four human postimages | Lib-test build PASS; 14 E1 parents PASS; parent15 fixture FAIL101 before the case-variant loop. The remaining 12 parents and all-targets check were NOT_RUN. |
| R7, R5 + reviewed E1 R6 parent15 | Lib-test build PASS; corrected parent15 + four remaining E1 parents + eight human canaries: 13 PASS, zero failures/ignored. All-targets check RC0, zero error diagnostics, 320 warning diagnostics. The 14 R5 parents were not rerun. |

| Artifact retained privately | SHA256 |
|---|---|
| E1 R6 29-file manifest | `a8e87d5348df7863b96a96c57ddbcd55fd45536c04c49f1467c5614ad76ae5a6` |
| Scoped R6 source-review receipt | `b542662689630be40064e6101f61709e5a02c188a5433919f27e0b591a32eaa6` |
| R5 input source archive | `430965dfc57ac5002cde906187224840276eeddce42fcaba6721aec9cf2e5fe9` |
| R5 verified result receipt | `ef0b8870361e8652a3cb134978013991a4f61e9b3289e1cec24b58d7b23b4858` |
| R7 input source archive | `dd5f66a927e7ce61727530f0774eb4a4dc79e024dd316d91ff92fc58df8385ae` |
| R7 verified result receipt | `cca283a376daa86709e1b0f540c98b606051db3385e79f9cb7b8be6a6bf56296` |
| R7 bounded post-run source export | `6c766232b3d7c6f2899ee64367976e9cd79b7238dc77e9d93555c9a154b085fb` |
| R7 private raw-evidence manifest, 468 files | `49d31a23923b5abdf726bf880c2e8019951fa37ffb004f0e68a83f054edc5708` |

Archives, original reviews, manifests, receipts and raw traces stay private outside Git. This public record is a redacted summary, not their redistribution. [TESTS.json](TESTS.json) records additional historical hashes and exact per-parent argv without host paths, credentials or raw logs.

## Retained failure history

- **R1 NO_GO_SOURCE:** format15 reserved-family case handling and the saturation/active-owner refusal oracles required correction. R2's scoped independent recheck returned GO_SOURCE; it did not prove compilation or behavior.
- **R2 compilation FAIL:** 11 errors confined to the fixture; tests NOT_RUN. R3 corrected imports, incompatible `Eq` and checked SQLite integer conversions. This is not behavioral RED.
- **R3 fixture FAIL:** lib-test build passed, then the first parent failed at `reopened.integrity()`. Its earlier install, active-state assertion and genuine-source reopen had succeeded. The original result receipt misidentified the operation; its retained correction identifies the legacy-only integrity guard, not expiry or failed installation. The other 18 parents and check remained NOT_RUN. R4 replaces that diagnostic call with real scoped full integrity verification and preserves exact legacy/SourceB refusals and an unchanged snapshot.
- **R5 fixture FAIL after 14 PASS:** parent15 did not guarantee a private directory at creation. The effective failing numeric mode/UID was not captured; no runtime mode or umask is inferred. It failed at Store open, before testing reserved kinds.
- **R6 test-only correction:** a fresh `Builder.permissions(0700)` directory and actual-mode assertion precede Store open. Positive ordinary appends/integrity before and after, exact reserved-kind errors and unchanged format/head/hash/count/quota/no-egress-schema snapshots prevent a generic failure from satisfying the oracle. No existing path is repaired; no production guard, deadline or umask changes. The scoped review confirms only this test body differs.

## Exercised contracts

R5 uses actual private SQLite stores, 13→14→15 backups/migrations, retained owner/source epochs, a private credential envelope/index and live review bindings. It covers A/B separation, durable reopen/replay, byte-identical retry and divergent-operation conflict, CAS, active Session and historical tenant ceilings, terminal entry reservations, ordinary/control/identity event-count saturation, SQLite FULL rollback, redaction and projection corruption, stopped-owner exclusion, reserved-family replay and genesis ordering. The SourceB negative uses the other genuine source and verifies no mutation.

The saturation oracle observes positive accepted fill progress, the independently computed event boundary, the exact typed refusal, actual row count and serialized-byte/quota agreement, plus an identical effect-free retry. Revoking one of two heads preserves the other's reserve. It does not measure physical byte saturation. The maintenance oracle requires exact `OwnerContended` at acquisition, no backup/DDL/event effect and unchanged format14/schema/head/quota; `ConfigurationMismatch` is distinct. Releasing the same owner lets the same retained plan perform backup, migration, resume and reopen.

R7 exercises the corrected pre-migration reserved-kind parent, revoked-binding replay refusal, durable clock regression, integer/limit rejection and the pure expired-head terminal transition over a real retained projection. Its eight human-source neighbors cover physical parent/entry binding, substitution, structured parent/entry failure categories, a descriptor-shortage classification canary, private-entry/alias guards, expired-epoch isolation and retained-token revalidation under an existing write epoch. They do not perform real FD exhaustion or qualify a runtime.

## Execution and closure

The admitted offline QA container was Linux ARM64 (`7.0.14-linuxkit`), with Rust toolchain selection `1.99.1`, image ID `sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`, network `none`, read-only root, no privilege, private IPC/cgroup, dropped capabilities, no-new-privileges, 4 CPU, 9 GiB memory/no extra swap, 128 PIDs and a bounded 4 GiB scratch tmpfs. Source inputs and the public dependency cache were read-only. This is the QA profile, not worker/runtime qualification. Luna executed the admitted target campaign; Root verified the receipts and source/closure evidence.

The real compile/check argv were:

```console
/usr/local/cargo/bin/cargo test --frozen --offline --lib --no-run --message-format=json
/usr/local/cargo/bin/cargo check --frozen --offline --all-targets --message-format=json
```

The harness resolved the actual lib-test executable from Cargo JSON, freshly listed each reached selector with `--exact --list`, then ran it once with `--exact --nocapture --test-threads=1`, stopping at the first failure. [TESTS.json](TESTS.json) records those actual executable argv, selector order and timeouts. R5 never reached the check; R7 ran it only after all 13 remaining selections passed. These are reproduction records for the admitted QA environment, not permission to execute project code natively.

Both runs verified all 2,630 source files and modes after execution against their respective inputs and preserved bounded exports. R5 retained 518 raw evidence files; R7 retained 468. Both exact QA containers were stopped/removed with **closure RC0**, `oom=0`, `oom_kill=0` and `OOMKilled=false`. Peak observed cgroup memory was 6,530,342,912 bytes on R5 and 6,482,698,240 bytes on R7. The holder's recorded exit137 is separate from test/check exit codes and closure success. These measurements are not process-RAM bounds or capacity100 evidence.

## Open requirements

Format14 and legacy access stay fail-closed. Format15 requires explicit maintenance with consumers stopped, a fresh verified backup, retained source authority, explicit commit and reopen; no automatic migration or CLI is supplied. A terminal reservation is logical accounting, not disk preallocation: SQLite FULL, I/O or commit failure can still prevent a revoke. `revoke_requested` is not an observed connection closure.

Commit/WAL/I/O fault recovery, an elapsed-time expired-policy Store test, physical byte saturation, capacity100, human policy permission/delivery, HTTP/UI, durable connection permits, DNS/socket mediation, common-channel/bootstrap integration, applied revocation, independent direct-attempt observation, remote Web/MCP scope, four real provider modes and Docker/Podman/Kubernetes acceptance remain open. Strict Clippy, full suite, release build and native-host qualification were not run in these campaigns. No Internet activation, provider/IdP, sensor, MITM/PKI or cluster action occurred.
