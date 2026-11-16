# Dedicated Kubernetes lifecycle integration

This development integration contains the 21 dedicated paths from the lifecycle/attach lot, without importing its 103 unqualified Source17 ancestors. The software test source is commit `1d2749549e59ab96f4bf51e6d6ae4b1b7b7b8ebc`, tree `598861b9f22e46a30c7ad29374d489208b0f53b3`, parent `990ee5b2eab80ec0cdd33a45c3a1bab81e9f23f0`. Twenty original postimages match the earlier lot; the audit parent deliberately retains the canonical replay guard 11–15 and mutation guard 11–13. No Cargo, root API/lib/main/audit or Source17 ancestor delta was imported.

The code covers before-effect reservation, confirmation and abandonment, exact replay/CAS and retained local state after lost acknowledgements, plus attach cancellation/deadline ownership. The owned Native17 broker port and S5 effect permit remain closed. This is not complete Kubernetes execution or cluster qualification.

## Actual native software validation

Rust/Cargo 1.99.1 on `aarch64-apple-darwin`, private HOME/CARGO_HOME/TMPDIR/target, locked dependencies and frozen/offline builds. Native execution was authorized by the maintainer; it was not an OS security sandbox.

| Command or scope | Observed result |
|---|---|
| `cargo check --all-targets --frozen --offline --message-format=json -j1` | PASS, RC 0, zero compiler errors, 166 warnings |
| `cargo test --lib --no-run --frozen --offline --message-format=json -j1` | PASS, RC 0, zero compiler errors, 36 warnings |
| Actual libtest listing | 17 selected FQNs uniquely present |
| 17 exact tests, one case/process with `--test-threads=1` | 17 PASS, 0 FAIL, 0 ignored |

The 17 cases include real Store/API quota and audit refusal, same-request replay after ACK loss, private confirmation/nonce binding, authenticated inter-tenant refusal, successor abandonment, control restart, the before-effect CLI, and attach expiry/caller-loss/cancellation ownership. They use local SQLite/loopback software fixtures. No container engine, real Pod, provider or model was exercised.

Root receipt SHA-256: `47ede2be6e93166ba4bba66260c0fd4b8c397caa2e4325be0b5b0bc8778bae60`. Libtest binary SHA-256: `1fb2d37ef8f6f7a73b9c8bb0160c1fe42d66b986dca7603eae76fec151403b1e`. Raw Cargo JSON, listing and 17 individual stdout/stderr/RC sets remain private and retained. The source worktree was clean, including ignored files, before and after these tests. The historical R4/R5 proofs remain attached to their earlier sources; they were not transferred.

## Review and formatter successor

An independent review of the 21-path composition and a separate bounded logic review found no confirmed blocking defect within this before-effect scope. The logic review examined identity/tenant checks before body parsing, durable audit and quota/CAS, private confirmation, abandonment and lost-ACK replay, plus blocking/attach ownership. Logic review receipt SHA-256: `c1777df18dd0ccc7f424840cdaa5b1cde20fefa445be5ada86357e55a44bb73f`. This source review is not an executed test or full-product audit.

The initial format check failed on 11 of 18 changed Rust files. Root applied only the output of Rustfmt 1.10.0/edition 2024 in commit `cb1ca7f1935a4b40acc421d2b605974ee7a7a028`. The resulting 18-file format check and a new all-target Cargo check both passed. The 17 tests were not rerun after this formatter-only change; their exact source remains `1d274954`. Post-format Root receipt SHA-256: `d993a30684d7821a84bccf99bd0add519fe195d6906e17f526e88e2c9da6e482`.

The canonical integration merges this formatter successor and preserves the canonical branch own documentation commits. Application inputs are compared against `cb1ca7f` at merge. No software result is credited to a different native session-control or Config/E2 source. The guides and configuration example in this lot are kept together in English/French.

## Open qualification

Real Docker/Podman/Kubernetes, a real identity provider, Native17/S5 effect activation, scale 100, fault/recovery campaigns, full-suite and Clippy qualification remain NOT_RUN or open for this lot. A passing software fixture and source review do not prove runtime confinement. No production gate is closed. Existing proofs, volumes, credentials and unmerged branches are preserved.
