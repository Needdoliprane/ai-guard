# Private control candidates and worktree lifecycle — 12 November 2026

These are source-specific software results for private development candidates. They do not describe the current public composition as having passed these campaigns. Global format 14, `AuthLedger5`, `Ready` and the complete human approval workflow remain closed pending implementation, composition and their own validation. The published operator diagnostic has its [separate evidence](../operator-diagnostics/README.md). No production gate closes here.

## SQLite connection association: reproduced false refusal and bounded correction

The existing helper rejected every initialized TEMP namespace. A controlled diagnostic identified the rejecting schema-set predicate, but an empty TEMP filename alone did not prove that the namespace contained no objects. A new real SQLite/Store13 test explicitly creates/drops its TEMP object, verifies zero remaining objects, then demonstrates the unchanged helper refusing that exact main connection. Its actual result remains FAIL_EXPECTED_RED: RC 101, zero passes, one failed parent.

The corrected helper retains the main filename/canonical-path and physical-backing checks before and after association. It admits only main plus an optional already-initialized TEMP namespace with zero objects. TEMP objects, main-table shadowing and an attached third schema remain refused; DROP/DETACH restores acceptance without poisoning the healthy Store. Reading up to three database-list rows prevents an attachment from being hidden after main and TEMP. Independent bounded source review found no concrete defect.

| Campaign | Frozen input SHA-256 | Actual result |
|---|---|---|
| New tests with unchanged helper | `6d06a47dceced48853cdcd432e7d1bd8274c57ae9f5f76cea80170b80d0c33f7` | One exact parent FAIL_EXPECTED_RED; 0 PASS; build/list and closure RC 0; all-targets NOT_RUN |
| Corrected private Core candidate | `72ea2425a03f61da75a66a84ecb2c2d1ffe4eebf1cbd7763d8688d12ce2f9ac2` | 47 exact parents PASS; build/list and all-targets check RC 0; closure RC 0 |

Each freeze contains 2,525 regular files with mode 0644. The corrected freeze differs from Core r3 only in `src/audit/review_authority.rs` and its child tests; 2,523 other payloads/modes and Cargo.toml/lock are unchanged. All 44 previously selected parents remain in order, followed by the three new TEMP cases. This includes the original pinned-backup migration failure and the existing foreign-copy/backing-replacement checks. The full set passed with one exact pass and zero ignored cases per parent.

Actual build/check forms inside the admitted offline QA were:

```text
cargo test --frozen --offline --lib --no-run --message-format=json
<resolved-libtest> <exact-selector> --exact --nocapture --test-threads=1
cargo check --frozen --offline --all-targets --message-format=json
```

The exact new selectors share `audit::review_authority::tests::` and end in:

```text
an_initialized_empty_temporary_schema_keeps_the_exact_main_connection_valid
temporary_objects_refuse_until_dropped_without_poisoning_the_source
an_attached_schema_after_empty_temp_refuses_until_detached
```

Earlier failures are retained: initial missing test import prevented compilation; four newly created fixture directories needed explicit 0700 permissions; the pinned-backup parent then exposed the actual TEMP false refusal. Those sources and outcomes are not renamed PASS. Controller preflight errors in labels, Bash 3.2 dependent local declarations, count guards and cache-pin paths were corrected before the corresponding execution. No product assertion, guard, source identity or resource limit was weakened to obtain green.

| Retained artifact | SHA-256 |
|---|---|
| Tests-first QA evidence index, 152 entries | `6b02bc6465d78256a3ed4f1332b635052ac136af9b1823bc8968f2d5e2856c1c` |
| Tests-first actual live-source export | `384de0efca16b6990bce6ec65d78cc58fdd97c930a86e0deabe3d75d95267d6b` |
| Corrected QA evidence index, 761 entries | `5eef6ae9887f8f627ce28a956f36b76e4d8cb827df269f428e945b4019c9799e` |
| Corrected actual live-source export | `957e28501fb34b40e1e37836d6786462ae482578eefd5633f7308dfe55388ae9` |
| Root independent corrected-result receipt | `abd0498158c66a30c3390d2a051c0c12ee08bf1376b24245c264281f321e8c2a` |

Root independently rehashed both indexes, all 47 corrected oracles and all 2,525 payloads/modes in each actual export. Corrected export is 28,067,840 bytes, below the 32 MiB cap. The separate Root receipt was written after the QA evidence index and is not covered by it. Raw logs, source archives, overlays and independent reviews remain private.

The pinned Linux ARM64 QA used Rust 1.99.1, offline Cargo with jobs 1, UID 1000, no network, a readonly root, dropped capabilities, no-new-privileges, private IPC/cgroup namespaces, 4 CPU, 9 GiB memory, no container swap, PID limit 128 and two readonly inputs/registry binds. Corrected peak memory was 6,319,284,224 bytes; OOM counters were zero. The stopped holder recorded exit 137 with `OOMKilled=false`; stop/removal and exact-ID absence checks returned 0. These are compilation/test observations, not a worker exit or resource-usage qualification. The budget retained a descriptive baseline archive-byte count; the measured corrected input is 27,893,760 bytes, hash-bound above, under the unchanged cap.

## Independent relay recovery consumers

A separate freeze `94628626a4c5b4a95be9411fb272a9dfeb645fbfa4149dfe5c972925ac108b16` (2,583 files) passed two lost-ACK consumer regressions and the all-targets check. These use real Store, journal, gateway and SDK loopback; runtime observations remain explicitly synthetic. One covers a lost reservation acknowledgement without a local snapshot, the other a lost claim acknowledgement with a retained Reserved snapshot. Unknown reconciliation stays unknown until confirmed; no complete AcceptedJob, runtime crash, fencing or Full3 qualification is inferred.

Root verified 175 raw hashes, both exact oracles and all 2,583 live-source payloads/modes. QA evidence index SHA `6745efaaeac10c933fd368eb95b2e9191091e9eef65cde391a4db937b2844a75`; live export `d59659426b76ad78937596dee07a468fa8476a262193d933097fe2715f32715d`; separate Root receipt `647002e7f7d964dad5b53c9db1d571ed0ec93dfc4123d1d7ae4775473d8a80c4`. The two tests depend on 134 private Full3 paths and are not published as an orphan test without those dependencies. Their results are not combined with the Core campaign.

## Completed worktree lifecycle

Four delivered worktrees were removed using normal Git worktree removal after fresh full-content/mode, clean-state (including ignored files), identity and open-file checks. Original Claude D/E refs remain unchanged; Codex E/README compositions have local-only retention commits. Complete verified bundles and all private evidence remain retained. No force/prune, process termination, volume/cache removal or main-checkout mutation was used.

| Retained branch | Exact head |
|---|---|
| Claude source lifecycle | `a2ef2fbc87b8dd00723f85c2c0aab0dcf78b02b1` |
| Claude operator diagnostics | `cf272b20ff126d75074d7347852c51454df58562` |
| Codex E composition, local retention only | `84947152ce5797ff7ca25716fe4ba3677202bf30` |
| Codex README composition, local retention only | `b31f44f750eb16ce1adb6a95c21ae2e41ce9cfa5` |

Root closure-receipt digests are `e6b43bb2b547957a71c133799b8c106d106e0f91ec0463ab28966857cfe0066d` (originals) and `be2fcd90d0cb13c65e7af0b52e984885a03ef3d150748a99ccdf557c0cc0e945` (compositions). Prepared indexes were preserved; closure receipts are separate later records. Active Core and relay worktrees remain available. Claude risk-coverage analysis is still IN_PROGRESS, final report NOT_DELIVERED; drafts were not read or treated as accepted audit input.

## Remaining scope

No real worker/provider/IdP/runtime/cluster, complete approval flow, native platform matrix, format 14/AuthLedger5 activation, multi-host federation, capacity for 100 workers, restoration or production gate is qualified by these software checks. Audit fingerprints keep their existing status; a neighbouring guard correction does not close them.
