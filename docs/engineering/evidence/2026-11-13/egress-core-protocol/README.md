# Egress policy and protocol: targeted software evidence

The composed Rust source on public base `970636e1141a8fd2eaf382e9eeb52e7282fc8a35` passed 34 selected pure parents and `cargo check --frozen --offline --all-targets --message-format=json`. It adds a policy evaluator, supplied-address validation, borrowed HTTP CONNECT parser and bounded pure payload/flow state. It adds no listener, durable connect issuer, egress API or runtime activation. The [English](../../../EGRESS_SOFTWARE.md) and [French](../../../EGRESS_SOFTWARE.fr.md) contracts separate these primitives from the required end-to-end control.

## Source, review and format

Canonical source SHA256 `8dddbb19e7f7253cffa28cdc8d877f06091d07faa8db3efbbea2c4a9c3c0d7cb`: 2618 regular files / 30490624 bytes. Eleven application postimages consist of eight new module/test files and the direct already-locked `httparse =1.10.1` dependency edge plus `pub mod egress`. No new crate version was fetched. Independent policy, corrected-protocol and final-composition reviews are GO_SOURCE, with fingerprints in [RESULTS.json](RESULTS.json).

Pinned rustfmt1.99.1 formatted the new postimages before the final QA; check succeeded. Five formatting derivatives and the original test/production freezes are retained privately. The formatted composition received its own build and tests; no earlier result is transferred to its source hash.

## Reproduction and correction

On uncorrected composed source `7dbf1a7121a569e95b8ee23ecb6b90dfa836bcf3e752bc745ef70a5391755155` (2615 files), two exact parents actually failed: HalfClose retained unusable credit and refused sibling credit; an oversized control returned Protocol before the required Size refusal. The first QA attempt had failed a metadata-key precondition before compilation and ran zero tests. It is a separate harness failure, not a reproduction.

The corrections retire only unused credit in the closed direction while preserving queued reservations, and replace the internally tagged allocating preflight with six borrowed RawValue slots and a closed flat shape. A nonallocating byte guard bounds syntax and rejects escaped keys before the standard parser. Frame/variant/string bounds precede the bounded owned payload copy. This is not a claim of zero allocations or measured RAM.

Both regressions passed in the final source alongside their neighbors. Policy and wire acceptance remain descriptive; none creates trusted authority.

## Observed final campaign

- 14 policy, 6 frontend, 11 protocol and 3 existing human-mapper parents: 34 PASS, zero failed/ignored. The three mapper parents are replayed within this composition; this is not 37 unique tests. Unselected library tests were listed/filtered only.
- One offline frozen library-test build, actual Cargo JSON artifact selection, then one exact invocation per parent with `--exact --nocapture --test-threads=1` and a 30s outer bound. Check all-targets ran once after all 34 PASS. No new egress compiler warnings were emitted.
- Pinned Linux ARM64 QA, UID1000, network none, read-only root/inputs, private scratch/cache/target, capability drop and resource limits. All 2618 post-export source payloads/modes matched. Source capture stayed under 32MiB, OOM was not observed, closure returned 0 and the exact QA container was removed.

The build used `cargo test --frozen --offline --lib --no-run --message-format=json`. The equivalent selection for the 31 egress parents is `cargo test --frozen --offline --lib egress:: -- --test-threads=1`, inside the isolated environment required by the repository. Actual parent names, source hashes and raw-evidence fingerprints remain in the private QA plan and the structured summary; raw private artifacts are not product data.

## Open requirements

This software result does not implement trusted binding/epoch ownership, Store/API policy administration, mandatory durable audit/permit ordering, real DNS or sockets, framing integration, bootstrap, grants, runtime revocation, direct-attempt observation or dashboard wiring. Capability V1 uses fixed exact flow limits; lower negotiated values are currently refused. The public-address classifier still requires contextual exclusions for public control addresses. Existing bridge workers remain unmediated.

CONNECT remains opaque TCP, with no guarantee of actual HTTPS, certificate validation or absence of tunnels. Provider credentials, remote Web/MCP coverage and all four real provider modes remain separate requirements. Docker/Podman/Kubernetes workers, actual duplex/FD closure, performance/capacity, browser and Clippy are NOT_RUN for this tranche. No finding or G05/G07/G11/global gate is closed.
