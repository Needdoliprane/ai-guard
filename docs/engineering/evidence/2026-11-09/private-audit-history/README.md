# Private audit history and workspace boundary

Maintainer request, 9 November 2026: retain historical audit reports under
ignored `.claude/audit/`, without interrupting independent implementation.
This evidence covers archival integrity and the narrow workspace filter. It
does not qualify the product or close any of the new audit's fifteen candidates.

## Scope and source

- Integration base: `codex/human-brokers-validation-20261104@696efd18ed21f78df7429a20d8e16dbcac1c00d9`, with explicitly uncommitted D5/Core/federation/bootstrap composition.
- Implementation owner: `d5_composition_review`, dedicated `workspace-private-audit-filter-20261109` worktree; only `src/workspace.rs` changed.
- Independent reviewer: Root, read the complete tests/fix and both authorized import/export call paths before integration.
- Base `workspace.rs`: `e7ee04074822a879f7307a40f22f971d4ae4553a00694d521a1706af01ef3699`.
- Tests-only RED file: `0619768cd505fa15c4cf487117f29e8cfccff51a882fd0125c6940b801d67561`.
- Corrected file: `a21157f676b208a7bfc03798c77bf7f118ef105f4de6b3902242ef34be733f9e`.
- The reviewed predicate excludes the component pair `.claude/audit` at any depth, using the existing export name folding. Direct/authorized private-root import is refused; publication omits private entries while strict scanning preserves observed archive facts. Agent rules/commands, ordinary `docs/audit` and `.claude/audit-notes` remain source.

This is a path boundary, not a detector for arbitrarily renamed private content.
Existing no-follow, archive type/path/size limits and durable export publication
are retained. A previously built binary does not gain this behavior from an
ignore-file change.

## Archival checks — PASS

Two available runs were copied without modifying their originals. Selected
readable pieces and complete compressed original directories are retained,
including the sixteen frozen snapshots and supplementary audit evidence.
The private index names the independent copy outside the worktree so future
worktree removal cannot silently discard this ignored history.

| Check | Observed result |
|---|---|
| SHA-256 against sixteen original snapshot manifests | All sixteen checks RC0; no target execution |
| Copied readable documents/JSON/manifests | 24 new-run pieces and 11 historical-run pieces match capture-time hashes |
| Complete new-run archive | `d3637e9301a794a1d2c921a69194d0b2b41cab53017504530754de3a948629a1` |
| Complete historical run-1 archive | `a992b829997f8393a01bb7ae6ee25e5e4482035f6148faf047784f9517417c40` |
| Copied compressed archives | Both SHA-256 checks and `gzip -t` RC0 |
| Git exclusion | `check-ignore -v --no-index .claude/audit/probe.txt` resolves `/.claude/`; `ls-files -- .claude` empty; status reports `!! .claude/` |
| Local permissions | Directories 0700, files 0600 |
| Format / whitespace | Explicit rustfmt 1.99.1 check and `git diff --check` RC0 |

The October 29 root reports were not found at their supplied path; the remaining
agent directory is not a replacement report. The historical registry keeps
this gap explicit. No source report script, provider or target code was run for
archival checks. Hash manifests verify bytes; they are not signed trust anchors.

## Product regression — RED then GREEN

Execution by Luna, isolated pinned Docker QA, no external network or provider:
UID1000, capabilities0, no-new-privileges, seccomp, read-only root/toolchain/cache,
empty allowlisted environment, private scratch only, 4 CPU, 8 GiB RAM/swap0,
256 PIDs, 4 GiB/65,536-inode scratch, bounded file/open-file and wall limits.
The same exclusive checkout/target was revised only after actual Cargo terminal
and a new full 2,410-file manifest check. No native project execution.

Command on both revisions:

```sh
cargo test --frozen --offline --lib workspace::tests::private_claude_audit_ -- --nocapture --test-threads=1
```

| Revision / full manifest SHA-256 | Selected and outcome | Raw log SHA-256 |
|---|---|---|
| RED `61c0bc2bbe19ed6acf6cb7b746030c9af3db128cd06e3c4a6a247e02052ce016` | 2 selected, 2 fail, RC101; compilation 1m28s, tests 0.01s | `005348015d7bfe4ea90acf26f6520e8bc950d70bdae450a54eb39b08f8985382` |
| FIX `a187e73369168826c3b878c2db65b503162dd45fb25f82494d838f749da86565` | 2 selected, 2 pass, RC0; compilation 1m31s, tests 0.02s | `cf989d0f9e0078f344a39c87dbccdd6b4cfe0396e8870ca297cbf854c558f20e` |

Root read the actual RED diagnostics: import includes
`.claude/audit/report.md`; export reports zero omitted entries instead of four.
These are the intended behavioral assertions, not compiler or fixture failures.
Fixtures contain synthetic canaries only. GREEN verifies manifest and archive
exclusion, preserved useful rules/source, private-root refusal, case/folding
variants, implicit/late archive parents and unchanged original contents.
Sixteen lib-test warnings remain; Clippy is NOT_RUN (actual pinned component
absent), no claim that the source is warning-free.

Changed-module regression commands on that same FIX revision:

```sh
cargo test --frozen --offline --lib workspace::tests:: -- --nocapture --test-threads=1
cargo test --frozen --offline --lib workspace::export_policy_regression_tests:: -- --nocapture --test-threads=1
```

The first command selected **10/10 PASS**, RC0, 0.03s; raw SHA-256
`c141b00af982080353d98c6c0750b5decec13ad5d3416b6bbae43f465e9fe543`.
Root initially expected sixteen from an incorrect count across sibling modules;
Luna stopped on that discrepancy. Root checked both module declarations and
admitted only the six remaining parents, without repeating the ten. The second
command selected **6/6 PASS**, RC0, 0.01s; raw SHA-256
`ff10ead6657b51965d1e1d9f9fba02b105c95d9e20e1a13ca64f1e3acc75654b`.
Total: **sixteen unique parents**, including the two new regressions. The count
prediction error is a harness scope error, not a product failure or missing test.

## Limits and continuity

After these regressions, one independently reviewed test-only correction changes
the presence fixture URI to `/control/api/v1/brokers/heartbeat`, matching the real
client. The previous failure had not injected any refusal; it was not a product
recovery RED. On final manifest
`2ddb5664d544f39a0c65c5433c7b1fa2a571761a517736968ca7578206766077`,
the corrected single presence parent passes, four checkpoint HTTP parents pass,
`cargo check --frozen --offline --all-targets` returns RC0 in 50.04s, and the exact
Schema11/12/13 contract parent passes. Raw hashes respectively:
`e4029019db49eecd727e0c6746e6ce8bda5bf90f6f58d2c8da403074e48eb573`,
`39c0d2a696a85a0a17db14f6ed0dbaa29b0dc70c694537897de71df9beac1bc7`,
`785bb0083b0c889fc0c4ac1dd4f2aa08138891caa7fb70967b0c34ffc22efa9c`,
`cf6c9f97d94c2fa22cbedca9e79aeeb5bb0b0b705077bd749ae120eb85ae623c`.
These are separately scoped composition checks; the private filter's RED/GREEN
manifests above remain unchanged. No prewire r3 code is in this execution.

The QA was stopped after actual terminal of every admitted command: PID0,
OOMKilled=false, exit137 from explicit QA stop at22:27:16UTC. The independent
watcher observed this and ended RC0; removal and absence were confirmed.
Full source, raw logs and the retained synthetic presence fixture were preserved
outside the worktree. Its socket nodes are not serialized. Four zombie child
entries were observed under this QA's sleepPID1, not live workers; no external
container, volume or service was stopped.

The new audit's target execution remains NOT_RUN; these archive/filter checks
do not promote source-only findings to demonstrated vulnerabilities. The
[audit registry](../../../AUDIT_FOLLOWUP.md#registre-des-audits) records all runs,
fingerprints, priorities, owners and follow-up dependencies. Broker recovery,
identity reserve, federation, Kubernetes and real-runtime gates remain open.
The independent broker presence fixture path mismatch is documented separately;
it is not a failure of this archive filter. No volume, credential, historical
audit, user file or active worktree was deleted during this change.
