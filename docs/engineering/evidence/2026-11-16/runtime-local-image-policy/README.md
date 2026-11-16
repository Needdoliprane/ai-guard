# Local image policy — software evidence, 16 November 2026

The shared Docker/Podman container builder now requests `--pull=never` for worker and workspace-helper creation. The selected image must be prepared explicitly in the engine cache; creation errors retain their cause and explain the preparation step. The local setup guides are aligned in English and French.

## Verified source

- Base: `143dfafa6bf83787845d5d32c9e7db99746c41e4`.
- Frozen candidate: `d2745f7958f35b4534dd7bfe5ca09ec2d7392b9882b4c60b0eacf8c7606ea3e0`, 2,908 files.
- Five changed paths: `src/runtime.rs`, `src/runtime/managed.rs`, `tests/runtime_security.rs`, and the two local setup guides.
- Independent source review: `afe66bd049733c0d91b575327e80f065ba406a034d6d699548e0c5191caed613`.
- Root verified result: `128e1eae4c84651896036318c8834b5cdd66519c4e89b1ed066fe8bd62e4c6d0`. The private raw logs and immutable source archive are retained under `runtime-local-image-policy-native-qa-r1` and `runtime-local-image-policy-publication-r1` in the engineering recovery directory.

## Executed checks

Native macOS ARM64, Rust/Cargo 1.99.1, clean six-key process environment, locked offline dependencies. Dev/test profiles used opt-level 0, debug 0, 256 codegen units, incremental disabled, and debug/overflow checks enabled.

| Check | Observed result |
| --- | --- |
| Targeted rustfmt, edition 2024, `skip_children=true`, three changed Rust files | PASS, RC 0 |
| `cargo check --all-targets --frozen --offline --message-format=json -j1` | PASS, RC 0; 166 project artifacts `fresh:false`, zero compiler errors |
| `cargo test --test runtime_security --no-run --frozen --offline --message-format=json -j1` | PASS, RC 0; fresh executable selected from Cargo JSON |
| `every_worker_creation_requires_an_existing_local_image`, exact | PASS, one test; six builder combinations (Docker/Podman × Import/Run/Export) |
| `worker_arguments_confine_a_named_volume_and_preserve_literal_arguments`, exact | PASS, one test |
| `cargo clippy --test runtime_security --no-deps --locked --offline --message-format=json -j1` | RC 0, zero errors; 248 warnings outside the three modified Rust files; strict warning gate remains open |
| Source comparison after execution | 2,908 files match candidate bytes and modes |

The two tests were invoked once each on executable SHA-256 `c2202fc4280df3fc312b4451e1c5a2337304485f6d54d6ab85e35bb113fb5529`, using `--exact --test-threads=1`. The argument regression also keeps a command argument spelled `--pull=always` after the image separator, where it remains command data.

## Limits and next acceptance

These are software/argument checks. Actual missing-image refusal, absence of engine downloads, Docker/Podman worker confinement, Kubernetes, provider execution and image builds were NOT_RUN in this lot. No production gate is closed. The next runtime check must use the explicitly selected local image and endpoint, preserve existing preview/Claude resources, record the exact created handle, and retain ambiguous cleanup or terminal observations.

QA preparation errors (an initial zsh verifier variable and an attempted check of a patch already present in the composed source) are retained separately; neither was a product test result or required a source change.

Engineering evidence added at integration does not change the five application/setup candidate deltas tested above.
