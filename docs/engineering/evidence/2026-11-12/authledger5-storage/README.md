# AuthLedger5 storage — controlled regressions and fixed candidate

This software slice implements internal generation 5 authentication storage with exact SQLite user_version 1000005. It preserves plan 4 capacity/debt/terminal promises, source/namespace/owner bindings, retained epochs, known commit floors and refusal of legacy handles after conversion. The private startup capability validates an actual source before an incarnation is committed and retained; a descriptive record is not runtime authority. These APIs do not construct an OIDC RAM registry from durable session history.

The two source defects found during review of the development candidate were reproduced before correction. B1 accepted the source as its own migration backup. B2 returned a new receipt after the real epoch expired inside the last backing check. The corrections require a physically distinct backup parent and inode during prepare/resume/revalidation, and recheck retained access after the last physical validation on append/retry/read/backup. A committed floor survives a late refusal; expiry does not permanently invalidate a healthy owner.

## Exact observed validation

Base input is the public 7170b866 + Core 17 composition with SHA256 `5e1189c150ed748e7297654cafd3c21e651304561321f4ecfb1440c6373b2d8f`. Nine frozen B payloads and two Root support ports form input `4d071fbd7c43787072a2440239b826eaad70504ae738812fc08957644cd01f88`: 2 571 regular files, mode 0644. Public parent `da024611f2868b0d5ade8efb8000ad7917d74f76` contains the same application baseline; intervening engineering evidence is handled as documentation-only differences. No tenant 14 or private Full3 runtime payload is included.

- Actual lib build, artifact resolution and compiled test listing: PASS.
- Each of 17 exact selectors: one PASS, zero failed/ignored. The fixed B2 parent completes all four serial operation branches. Its earlier failing run reached append only.
- `cargo check --frozen --offline --all-targets --message-format=json`: PASS.
- Root independently verifies 365 raw evidence hashes, every exact parent result, and all 2 571 post-run paths/payloads/modes. Export SHA256 `a942de27aabfc0897eceb4d208b002045b48a9826503dee9b1bd765ede320544`, 29 767 680 bytes, no overflow.
- Run and closure RC 0. Exact CID removed; exact ps empty. Stop exit 137 after 15 s is recorded separately from OOMKilled=false and all OOM counters zero. Compilation/QA peak 6 472 175 616 bytes under 9 663 676 416 bytes is not worker usage.

Commands inside the admitted offline QA:

```text
cargo test --frozen --offline --lib --no-run --message-format=json
<resolved-lib-test-artifact> <exact-selector> --exact --nocapture --test-threads=1
cargo check --frozen --offline --all-targets --message-format=json
```

Linux ARM64, Rust 1.99.1, UID/GID 1000, 4 CPU / 9 GiB, swap 0, PID 128, no network, read-only root, dropped capabilities, no-new-privileges and seccomp 2. Fresh Cargo target/home; only frozen source inputs and pinned public cache mounted read-only. The exact 17 selectors and receipts are in [TESTS.json](TESTS.json).

## Source review and historical failures

The original review identified exactly AUTH5-B1 and AUTH5-B2. Independent reviews accepted their bounded fixes; final composition review remains attached to the exact candidate. These are internal development review identifiers, not closure of similarly named external audit findings.

On original source `f205332b31eda7f1f23646ce3e66d8ac218e5046738ad263cbe52d79d245fe14`, B1 failed RC101 at its same-source assertion. The following host classifier failed because /usr/bin/cat was absent, so that overall run remains FAIL_HARNESS and B2 was NOT_RUN there. A separate run of B2 alone failed RC101 at its expired-ACK assertion, reaching append only. Source and exact container closure were verified independently in both runs. The initial RED packet with renamed dotfiles was rejected statically before CREATE; no test result is attributed to it.

The GREEN stopped container was created before Root finished one pin-count precheck; it remained stopped until complete source/profile admission. The check had assumed all five pins were in one file rather than three crate pins plus two index pins. All five were verified before any target execution. This orchestration deviation is preserved separately in ROOT_ORCHESTRATION_ORDER.json.

Private package: `qa-human-authledger5-fixed-20261112-r1`. Raw manifest SHA256 `4f60f935fbd59bbd0024e05ee5776eff7504584010ada4b364a904ebdd40f7dd`; Root results receipt SHA256 `7a8ac1021ec46ae9d278dccefb10a0af7677136c125d876b06ab623614c38c8f`. Frozen static preparation and runtime manifests remain separate.

## Remaining acceptance

This is an internal storage migration/startup/epoch slice. It does not activate global tenant 14, the complete A+B+Auth source closure, Ready, human approval endpoints, a user-facing migration command or a full human launch. No existing user database is migrated by this publication. The four-operation deadline regression uses a genuine WRITE epoch; a dedicated READ-epoch deadline test is not supplied by that parent. A persisted Active projection is not proof of a restored live OIDC session; this campaign does not qualify that full startup scenario.

Physical crash/restore, old frozen executable, distributed processes, actual IdP/providers and Docker/Podman/Kubernetes workers remain unqualified by these software results. The separate global authentication capacity risk remains open. No prior external audit fingerprint or production gate is closed.
