# Held-CREATE capability compatibility under schema12

The V2 authorization issuer already supports schemas 9–12. Its consumer omitted (12,2), so a genuinely authorized Reserve or Cancel failed with MigrationRequired under a valid schema12 Store. The correction adds only that exact pair. It does not admit V1 under 12, change any role or scope, relax Reconcile, add recovery rights or enable Kubernetes execution.

## Reproduction and correction

The [combined baseline](../kubernetes-held-create-recovery/README.md) passes two canaries before the two exact schema12 RC101 failures. The production issuer and real migrations succeed before each failure. The regression bodies remain byte-identical after correction.

Independent source review verifies the one-line product delta and real Store/migration/authentication/SQLite tests. GREEN runs three distinct parents exactly once at 15:34:43–44 UTC on 11 November 2026: schema/scope refusals, V2 admission with one durable allocation and exact replay, and V2 reconciliation of an actual migrated V1 reservation after inactive reopen. Each reports 1 PASS, 0 FAIL and 0 ignored. One no-run build and the all-targets Cargo check return RC0. Root independently checks the raw oracles and source equality before composing the two postimages.

Frozen source: 2,462 files, 26,527,232 bytes, SHA256 ebb094458963fed61207a846545964173df04c1d886a6cdb89e0cac750d8673f.
Manifest: 159f4b6c69aa37f44cbbec50ebc938868242bc548e9d569f5423f3f87e06bc70.
Actual live post-run export: 26,695,680 bytes, SHA256 46d499adff577a394ea081f7fba5308dd724636a43cdd6f04eba83a80c2b270a. Every source path/payload matches the manifest. The selected source hashes are in [SOURCE.sha256](SOURCE.sha256).

The QA source also retains the test-only D6 baseline, which is not run again. It adds no production behavior and is not a prerequisite of the three schema12 tests. These import the pre-existing kubernetes_admission_state_tests fixture. Root's two source postimages match the tested source exactly and preserve the 134 unrelated Full3 paths.

## Execution and limits

One cargo test --frozen --offline --lib --no-run --message-format=json, actual artifact --list, three direct parents with --exact --nocapture --test-threads=1 and one cargo check --frozen --offline --all-targets --message-format=json. Build/check bounds are 1,200/900 seconds; each parent 60 seconds. No full suite, provider or D6 reproduction is repeated.

Same [isolated QA profile](../relay-history-owner/README.md): Linux ARM64/Rust 1.99.1, no external network, pinned image, UID 1000, read-only root/public inputs, dropped capabilities, NNP/seccomp and bounded resources. Source is actually streamed to a 32 MiB capped export before stop, never reconstructed. Experiment/closure/final RC0, exact container removed, zero OOM and OOMKilled=false. QA memory peak 5,847,482,368 bytes is not worker usage. Exit 137 is the controlled sleep-holder stop after successful target commands. Raw evidence is retained privately in qa-kubernetes-schema12-20261111-green-r1.

This verifies the software compatibility correction only. Successor recovery D6, explicit schema 13 migration, old-reader/backup/restore, runtime engines, multi-host, 100 workers and all production gates remain open. No new migration command or HTTP recovery route is delivered by this small correction.
