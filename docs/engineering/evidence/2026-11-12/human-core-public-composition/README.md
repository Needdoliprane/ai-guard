# Human review core — public composition, 12 November 2026

Public application baseline `7170b86607acb15bab40b4def731dd54c7c4247f` plus 17 frozen Core postimages passed 61 exact software parents. The intervening public commit `3cc7694bb830b57abe017a393f8f44487e34c064` changes four engineering documents only. Publication adds the same 17 application postimages and this documentation; neither mutable tenant14/AuthLedger5 candidate nor private runtime composition is included.

The change supplies explicit review-policy/resource bindings, checked runner-credential and explicit human ACL primitives, retained source-authority and migration helpers, and an exact SQLite main-connection verifier. Approval mutation remains refused outside format 14. This lot does not activate format 14, AuthLedger5, Ready, review endpoints, or the full human launch flow. Technical ownership or an administration role does not grant personal approval.

The SQLite correction accepts an initialized, empty TEMP schema alongside the exact retained main database. Additional schemas, TEMP objects, source replacement and foreign connections remain refused. A foreign connection does not permanently poison the healthy retained source. The prior tests-first run reproduced the empty-TEMP defect on source `6d06a47d…` (one failed parent, zero PASS); separate private fixed source `72ea2425…` passed 47. Those remain historical sources. This composition was rebuilt and its 61 parents executed separately.

## Observed validation

- Linux ARM64, pinned image `sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`; Rust 1.99.1, UID/GID 1000, no network, read-only root, dropped capabilities, no-new-privileges, seccomp 2. Fresh Cargo target/home, offline frozen lock, jobs 1, 4 CPU / 9 GiB, zero swap. Only frozen inputs and public dependency cache mounted read-only.
- Build of lib/bin/human_access test artifacts and their three actual listings: PASS. Each of 47 Core, nine diagnostic binary-parity and five human_access parents: one exact PASS, zero failed/ignored. All-targets check: PASS.
- Root independently verifies all 956 raw evidence hashes, 2 600 static-preparation hashes, each exact test identity/verdict, and all 2 566 post-run file paths/payloads/modes. No source changes or export overflow. Run/closure RC 0, exact container removed and empty exact ps, no OOM.

Commands used inside the admitted QA (executable paths resolved from successful Cargo compiler-artifact JSON):

```text
cargo test --frozen --offline --lib --bin agentsandbox --test human_access --no-run --message-format=json
<resolved-lib/bin/human_access-test-artifact> <exact-selector> --exact --nocapture --test-threads=1
cargo check --frozen --offline --all-targets --message-format=json
```

The exact 61 selectors, source identity and receipts are in [TESTS.json](TESTS.json).

## Evidence and limits

Private evidence package: `qa-human-core-public-composition-20261112-r2` in the maintainer engineering archive. Runtime manifest and frozen static preparation remain separate. Input SHA256 `5e1189c150ed748e7297654cafd3c21e651304561321f4ecfb1440c6373b2d8f`; post-run SHA256 `8b23f187cfb51c67bee191857958c0dde5c0a0a434cbabe761440faf48c95b8d` (29 634 560 bytes). Root receipt SHA256 `a98d10e64167279bac3f100c9d9679b35007263068f6a5a8a6ced716055bc9f1`.

R1 remains a pre-Cargo harness failure: Linux mode text 644 differed from prepared 0644; all 61 product parents were NOT_RUN. R2 corrected only that representation and the review mirror, retaining exact application source and selectors. An initial host attempt to execute the non-executable RUN file failed before its claim; the actual admitted invocation used /bin/bash once. No failed run is relabelled PASS.

Peak cgroup memory 7 316 529 152 bytes measures compilation/QA, not agent usage. Runtime isolation, the three-worker tenant recipe, brokers/relay recovery, IdP/provider connections, native macOS and capacity 100 remain unqualified by this software slice. Private format 14/AuthLedger5 candidates require their own composition, controlled regressions and review. No audit finding or global production gate is closed.
