# Common product and build identity — software validation

Résumé : la version produit est alignée sur **0.0.1**. Le front et les outils/services Rust partagent des métadonnées compilées. Le constructeur distribué fixe un ID commun depuis sa copie préparée. Cette preuve logicielle ne qualifie ni des images ni les services déjà installés.

## Behaviour and boundary

Cargo.toml supplies the product version. The frontend package and both lock roots must agree; disagreement is rejected before an engine command starts. One retained PreparedSnapshot supplies the same version, ID and optional explicit revision to both image commands and their frontend, Rust and provider stages. This descriptive source ID is separate from resolved bases, provider versions, target platform, image digests and release attestations.

The UI shows its compiled identity and distinct observations from the dashboard and gateway. Missing annotations, mismatches, timeout and partial observations remain explicit. GET/HEAD `/api/v1/status` returns local compiled metadata; the dashboard route `/api/v1/gateway/status` contacts its fixed upstream without caller credentials or redirects. Existing `/healthz` behaviour is retained. The six own Rust tools accept `--version`/`-V` before operational configuration; `agentsandbox version --json` provides the closed DTO. These endpoints perform no migration or automatic service update.

## Sources and actual results

Tested commit: `24be056e6c9a3bd4242f0bac872150062863c49f`, tree `d745eff3928bb5b3b23ad7b56f484df015e99a33`. The frozen QA source contains 2,888 regular files; its archive SHA256 is `3a038cbf56079b54dc285529cc6bc294438e94db5bd59c75f2d6757381b270f4`. Both compilations used product version `0.0.1`, this same QA annotation ID and this exact revision. The archive annotation is not an image digest or the distribution PreparedSnapshot digest.

| Selection | Observed result |
|---|---|
| Fresh Rust build | 9 executable artifacts: 1 libtest, 2 integration tests, 6 own tools; RC0 |
| Registration | 18 actual lists, 70 unique active tests; 1 ignored lifecycle test excluded |
| DTO / HTTP / build plan / route canary / real CLI / source lifecycle | 12 / 5 / 7 / 1 / 3 / 42 PASS; total 70, no failure, unknown or selected ignored test |
| All Rust targets | cargo check RC0; 157 build warnings and 168 check warnings remain |
| Formatting | 21 initial pinned stdin/idempotence postimages plus 1 corrected fixture: 22 current exact bindings |
| Frontend build | Offline installation and TypeScript/Vite build RC0; 4 dist files, 317,667 bytes; embedded version/ID/revision verified |
| Historical frontend tests | 115 PASS on fd81601; frontend source projection unchanged at 24be056, without new test credit |
| Final collection | Source, fixture, process and OOM checks passed; both exact QA CIDs absent; separate Root flush receipts |

Root rehashed all Rust source bodies and modes after execution. The Rust fixture inventory contains 12 empty directories and no regular payload, so its regular-file tar has zero members. Complete final process scans observed zero target processes; init was enabled and zombies were not excluded. Peak cgroup memory was 9,365,565,440 bytes for Rust compilation/tests and 460,607,488 bytes for the front build. These are QA measurements, not worker consumption or capacity benchmarks.

The fresh compilation command was:

```sh
cargo test --frozen --offline --lib --test build_identity_cli --test source_lifecycle --no-run --message-format=json -j1
```

The resulting artifacts were listed and the six selected groups ran with one test thread. After all 70 tests passed:

```sh
cargo check --frozen --offline --all-targets --message-format=json -j1
```

Frontend build: `npm run build` (`tsc --noEmit && vite build`), after offline lock-bound installation of 14 public packages. All commands ran in admitted Linux ARM64 QA profiles, not natively on the developer Mac.

## Retained failures and review

R1 built 9 artifacts but failed a list-summary expectation before any product test: 70 NOT_RUN. R2 corrected that protocol mismatch and returned 69 PASS / 1 FAIL: the mutation fixture overwrote Cargo.toml before preparation with an invalid manifest. Its collector also correctly returned 96 for three zombie fixtures adopted by the QA holder. These outcomes remain preserved.

The reviewed test correction retains valid prepared Cargo bytes, checks them exactly in both build contexts, and still proves all four live canaries changed after preparation. A separate QA profile change adds an inspected init reaper; the collector still requires a complete scan and zero target processes. R3 passed the same 70 oracles. No product validation was relaxed and no product isolation defect is inferred or closed by these fixture/profile corrections.

Independent reviews covered composition, duplicate-key parsing, live-region placement, the corrected canary and final harness profiles. An additional review confirmed the actual closed R3 receipts (SHA256 `0c0917a795aa8112827c64e8ef8ea781fab08440c35f18eeac9cf8ece503c6c4`). Root separately checked results and exported bodies. Counts, hashes and private receipt fingerprints are in [PROOFS.json](PROOFS.json); raw logs and archives stay outside the distribution.

## Limits

Clippy strict, the browser/live-region canary, image builds and deployed HTTP services, actual Docker/Podman workers, providers, Kubernetes, 100 workers and the full repository suite are NOT_RUN in this lot. The existing readonly UI was not redeployed. Source17 authority/runtime work is independent. No production gate or historical security finding is closed: **0/12 gates**.

Guides: [EN](../../../../setup/release/README.md) / [FR](../../../../setup/release/README.fr.md), [API](../../../../api.md#build-status), [UI](../../../../ui.md).
