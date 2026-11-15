# Full native composition — Rust check R9

Date: 2026-11-15. This is a compiler result on the exact applied development candidate, not a release, runtime test or qualification of the published branch.

## Frozen source

Base `e8886bd1d97664eda5971a2d7f64c8e933807816` plus Claude session-control delivery `e8ef52f9626afc8d79b761fd59c6f952f380391d`, reviewed native dependencies and the actor coverage correction. The source is a filesystem composition rather than a Git commit. Root verified 238 planned postimages and froze all 3,291 public source files.

- Source archive: 42,822,656 bytes; SHA-256 `4ad336c6eb82dae5b8ac23c28b0abc449414d8d57c1c3e705dc601d21b4b9bbf`.
- Applied composition receipt: `9e56e530a8a019235e49c754863dbea50ee486bd5311e088ad11bd7676f25c66`.
- Bounded source reviews: `fdbf388e34155ffbee01d329c558d313723090f43893070032d04a110776f60c`, `563c8b92d6869159343d1f7dfd791fa3a4a814f7a2d2ecd87c35d3b9fccdf339`; actor correction `de2028dbf09aaa0b31867cf25ff7b03d2c42f48c6474b202a7d57fd37ae70f69`. These are source reviews, not executed behavior.

## Observed result

One actual invocation ran in the admitted isolated Linux ARM64 QA, with the public registry mounted read-only and network disabled:

```text
cargo check --all-targets --frozen --offline --message-format=json -j1
```

**FAIL, RC 101**: 64 coded compiler errors, 2 separate failure-note summaries without codes, and 62 warnings. Raw Cargo JSON is complete, with one `build-finished` message whose success is false. No test was executed.

The errors expose missing configuration/history/filesystem/signing dependencies, mismatched native versus legacy runtime types, internal visibility seams, missing generation input and enum exhaustiveness. They are assigned to three disjoint correction lots: configuration dependencies, runtime/conversation and audit seams. No retry of the same source is credited. This broader native source differs from the earlier R3 candidate; comparing 64 errors to its 11 is not a same-source regression measurement.

Source/cache/lock postchecks, all three sandbox guard phases, collector integrity and the three terminal execution slots passed. No OOM was observed; measured peak QA memory was 5,499,924,480 bytes under the 10 GiB cap. The exact QA container was closed and removed, and Root independently verified its absence.

- Evidence archive: 3,126,784 bytes / 63 regular files; SHA-256 `d0928aa12731efe2357570f4d9686050b08b2d7c38845b050b9abe8251c5320c`.
- Root verification receipt: `adc54e04e642e3dc77bd55e9ae15826f5a884b069b86020a45bc93308278997c`. Root reparsed the archive, matched the raw compiler JSON digest and classified coded errors separately from failure-note summaries.
- Luna execution handle: `92036`, terminal RC 101. Private raw evidence is retained in the engineering recovery directory under `native-full-core-check-qa-r9/`.

## Successors and limits

After this source was frozen, the independently reviewed auth startup segmentation correction was applied: 8 postimages, including 4 new leaves. Its receipt is `eb0f4a7861bfd13ce0a790fd275a8b253699d5b7ba6f645fb71b3a104c6d349c`, review `baeed14fdb5782932d1d61966414d0d8211279fcc7ae7ffaa4a0178f838fe43e`. It keeps the original two-second epoch limit, retains the original source/head/generation/owners, processes each source separately and checks the same cohort at final publication. This successor is **NOT_RUN**; neither this R9 failure nor earlier startup failures are transferred as a result for it.

Real native configuration completion/cold recovery, final startup composition, OIDC causal peers, E2 reduction/rebinding and sender authorization still have required open work. Rustfmt/Clippy binaries were unavailable in the QA image; formatting and Clippy analysis remain **NOT_RUN**. Browser, real engines/clusters/providers, recovery and capacity are outside this check.

**0/12 global production gates closed.** No reliable aggregate functional percentage or production qualification follows from these records.

## R10 successor check (2026-11-15)

This addendum records the later R10 check. All R9 source, results and limits above remain unchanged as historical observations. In particular, the earlier NOT_RUN statement for the post-R9 auth startup successor describes the earlier R9 report, before the R10 check; R10 includes that successor in a different corrected source, but executes none of its regressions.

The exact Root-applied R10 filesystem composition contains 3,307 regular files, including 3 files with mode 0600. Its source archive is 42,992,128 bytes, SHA-256 `1c75fb6748018e4ae62ac82cf85c5ecb18aad8d5214dd2d9d7a66dc72e22a29b`; Root source descriptor SHA-256 `7e85e9963b1b5e8a700b0d964073f182a46d6b67e9f3b202135aff5106019dca`. This identity is not a Git commit. It includes the reviewed compiler corrections, auth startup segmentation, E2 reduction successor and restored native broker admission fences. Provider readiness and the further OIDC, final startup, configured rebind and ConfigComplete/N6 successors remain outside this frozen source.

One actual invocation ran in the separately admitted isolated Linux ARM64 QA, with network disabled and the public registry mounted read-only:

```text
cargo check --all-targets --frozen --offline --message-format=json -j1
```

**FAIL, RC 101**: 7 coded errors, 2 separate `failure-note` summaries without codes, and 60 warnings. The coded errors are one `E0609` for the Auth5 receipt field and six `E0599` for validation calls on the existing closed ID/digest types. Raw Cargo JSON contains no invalid rows and one `build-finished` with `success=false`. **No test was executed.** This different corrected source is not a same-source rerun of R9; comparing 64 errors with 7 does not establish a correction rate or functional progress percentage.

Preparation, collection, terminal execution-slot checks and source/cache postchecks returned 0. Sandbox guards passed, source paths/modes and Cargo.lock were unchanged, and OOM counters did not increase. The exact QA container was closed and removed; Root verified its absence. These observations qualify this evidence collection, not the native behavior of the product.

- Raw Cargo JSON SHA-256: `9f01d898d625ef23380678f58c8baaf1ae34a8015711d43c5d8b89eb66d3293c`.
- Classified diagnostics SHA-256: `90ba5e44ba265eedaa14d80c3c9aad42e9286578f1c2a95d2d2f32e2f517c704`.
- Evidence archive: 2,952,192 bytes / 63 regular files; SHA-256 `2125746b5cac43c54695b253f07b1f0495cc468634d5267e79691b3a87d8ca4b`.
- Root verification receipt SHA-256: `0964c6f0bb0922b7f539c15365121584a0efb4caa16d3f10d62689395b222fdd`, recorded at `2026-11-15T21:59:51.865Z`. Root reparsed the strict archive, matched raw JSON and classified coded errors separately from failure-note summaries. Private raw evidence remains retained under the R10 QA record.

Compilation still fails. Tests, formatting and Clippy analysis are **NOT_RUN** in this campaign; no Store, native runtime, provider, worker or cluster behavior is qualified. Any correction after this freeze requires its own source identity and verification. Neither this R10 result nor earlier PASS results apply automatically to the current integration worktree or published branch. **0/12 global production gates closed.**
