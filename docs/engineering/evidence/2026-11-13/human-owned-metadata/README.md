# Owned metadata reads — verified software scope

[Français](README.fr.md). Recorded on 13 November 2026. These results cover frontend source `810711bcf3d3158990c40f9290fdc80b878bf5f9` and backend source `858fc7dab3f26e8edfae8f07425868bc2201b2f6` within the scopes below. They do not establish production qualification.

## Delivered behavior

The owned identity/resources reads are extended with audited Journal metadata and descending recent-event pages. Authorization of the selected source and resources precedes the page limit. Required receipts commit before publication. A real SQLite allocation failure returns no page and rolls back that attempt's head, quota and usage changes. Accepted usage remains held until the actual work completes, including after HTTP cancellation.

Recent-event pages in this profile return `stream_mode:"snapshot_only"` and `live_cursor:null`. Once that profile is positively established, the interface starts no Journal/recent polling, SSE or automatic retry loop. Refresh is an explicit action. A 503 in the same context withdraws rows, receipts and cursors while retaining the restrictive profile; an explicit refresh can recover the read. Content, detail, export, streaming, HEAD on these metadata reads and other unadopted operations remain closed for this profile. These pages provide neither the complete audit nor a runtime inventory. See the [API contract](../../../../api.md) and [owned-sources guide](../../../../setup/identity/OWNED_SOURCES.md).

## Observed results

| Source or campaign | Actual result |
|---|---|
| Before implementation, Rust `82c9807` | Two real 503 responses at assertions expecting HTTP 200; each test process exited 101. Source integrity and exact-CID closure were checked. |
| Rust `195b437` | 72 PASS, 13 FAIL and 2 ignored; terminal RC 85. The fixture failures remain recorded as historical failures. |
| Rust `858fc7d` repairs | 14/14 selected parents passed, with no failures or ignored tests, after nine registration lists. The actual libtest was identified through Cargo JSON; all 2,788 source payloads and modes were unchanged. The fixture export was empty, OOM counters were zero and the exact CID was closed. |
| Node `e25b631` | 32 PASS and 1 skipped test; aggregate `FAIL_HARNESS_NOT_GREEN` despite raw RC 0. TypeScript and Vite RC 0 remain scoped results. |
| Real-wire Node `195b437` | 6/6 PASS using four responses actually serialized by Rust, without HTTP substitution. One unique function was added to the previous 32. |
| Browser R1 | `FAIL_QA_PREFLIGHT_MODULE_RESOLUTION_TARGET_NOT_RUN`: the harness failed importing `playwright-core` before Chromium or the application ran. Targets remained `NOT_RUN`; actual container stop, removal and exact-CID absence checks succeeded. |
| Browser R2, older dist | Three functional scenarios passed. Root viewed four actual PNGs and found unreadable selected navigation on hover. Visual acceptance remained open because of that defect. |
| Current frontend `810711b` | Offline installation, driver syntax, TypeScript and Vite completed with RC 0. All 25 recorded phases returned 0; 2,788 source files and 64 frontend working files were unchanged. The new four-file dist was verified, OOM counters were zero and the exact CID was closed. |
| Corrected component browser | Three scenarios passed, with four PNGs and eight contrasts calculated from actual browser styles. Each of seven successful page responses matched a SQL receipt; eight SQL receipts were exported in total. The fixture exited 0; servers and the source owner were drained, the SQLite lock was released, and all phases and closure returned 0. |

There are **85 unique passing Rust functions across two source snapshots** (72 + 14 − 1 repeated function), rather than 85 executed on `858fc7d`. There are **33 unique passing frontend functions across two Node sources** (32 + 6 − 5 overlaps), rather than 38. Browser scenarios are separate and are not added to those function counts. The SQLite-FULL fixtures bound `max_page_count`; they do not reproduce physical disk exhaustion or power failure.

## Browser and component provenance

The browser exercises the real Rust gateway/dashboard and private SQLite databases through an explicitly enabled libtest fixture that is ignored by default. Identities and data are synthetic; listeners are loopback-only inside isolated Docker QA. No provider, IdP or worker runtime is contacted. Root reviewed four actual captures: 1440 px in light and dark themes, 320 px in light, and 390 px in dark. The selected-navigation foreground/background were `rgb(23, 27, 7)` / `rgb(226, 244, 113)`. All eight normal/hover measurements yielded a contrast ratio of **14.555603859702387:1**, above the targeted 4.5:1 threshold. This targeted observation does not constitute a full [WCAG audit](https://www.w3.org/WAI/WCAG22/Understanding/contrast-minimum.html).

The backend component was the actual libtest built from `858fc7d`: SHA256 `235d0320ffc8b269fd4263d221cadbbc0a9de1993b8dcedb9930a9e0b27c1da0`, 296,616,648 bytes. It is distinct from the product's main backend executable. The frontend dist was built from `810711b`, SHA256 `42bc519b86c2ff26b19cf06fc3ee323f7ac02dbe4e8a6faa275dedf9cfa78daa`. All 2,724 paths outside the frontend, including 778 Rust files, are identical between those producer revisions. The dashboard serves the supplied dist at runtime. Only the CSS and browser driver differ; the changed frontend is not claimed byte-identical to `858fc7d`.

After the positive profile was established, the actual quiet window lasted 33,041 ms with no automatic Journal/recent/stream requests. One stream attempt before profile establishment remains recorded. A real SQLite lock, 503 refusal and explicit 200 recovery were observed; the 2,510 ms busy window is not a latency benchmark. Actual process status, same-CID kernel/network checks, bounded artifacts, source/mode checks and exact-CID closure are retained. Host file and directory fsyncs were observed after closure; they do not qualify power-loss recovery.

## Reproduce and limits

[SELECTIONS.json](SELECTIONS.json) lists the exact Rust parents, browser scenarios and source/proof digests. The QA built one libtest with `cargo test --frozen --offline --lib --no-run --message-format=json`, verified its actual registration and then ran the selected parents. No project code was executed natively on the host. Within admitted isolation, a contributor selector is `cargo test --frozen --offline --lib api::human_sources::tests::recent_contract_tests:: -- --test-threads=1`; an exact parent from `SELECTIONS.json` can be selected with `cargo test --frozen --offline --lib <FQN> -- --exact --test-threads=1`.

The real-wire frontend test requires `AIGUARD_SUPERVISION_WIRE_DIR` containing the four actual response captures. Without it, the real-wire case skips and remains unqualified. The browser driver requires `AI_GUARD_OWNED_BROWSER_FIXTURE=run-v1`, an actual libtest path, a built dist and separate fresh private control/artifact directories. Read its source before running it in admitted isolation. These are test entry points, distinct from the product's main backend executable.

`NOT_RUN` in this lot: the product's main backend executable on this source, real Docker/Podman/Kubernetes worker qualification, provider/IdP integration, restoration or power-loss recovery, native macOS behavior, the full suite, strict Clippy, 100-worker capacity and a complete accessibility review. These checks qualify neither billing nor complete broker/egress compatibility. No audit fingerprint or production gate is closed. The maintainer retains private primary logs and archives; only synthetic test code and scoped evidence are published.
