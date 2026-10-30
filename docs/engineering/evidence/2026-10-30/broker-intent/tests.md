# Immutable execution intent — initial RED

Worktree `/Users/louis/Documents/ai-guard/.worktrees/broker-supervision`, branch `codex/broker-supervision`, base `11eb871db40ca9893a4492547561ec604247257c`. Only authorized test files were added: `tests/broker_intent.rs` and three JSON fixtures under `tests/fixtures/`. No application code or unrelated docs were edited by this test work.

## RED evidence

Command: `cargo test --locked --test broker_intent`  
Result: compile failed as expected with `E0432: could not find intent in broker` for `agentsandbox::broker::intent::ExecutionIntentV1`. This is an absent-feature compile RED, not a runtime failure or a vulnerability in the existing broker. Log and original command context are mode 600 in `/tmp/ai-guard-broker-intent-red.WgIouT/test.log` and `context.txt`.

The test suite was enriched after capturing the initial RED, without rerunning against absent code. The final owned test source SHA before GREEN is `46986706813facad51726d62718df5d3763a215e7c06945cfdf4e84ad090cdad`; fixtures are `3f157fecd35cb42a5a8832d4cb26ca8e004c69bde3a9ad17e5db216a205200fb` (typed checkpoint), `02d5f2403249e157f8db4327ee8defac15488fcbfa3e0816c786a10f38a034f0` (plan), and `19ff06b8a30321d38f19e2816b30baa28cb1cd407d88aecb0b93941080dd4e20` (independent golden). These hashes are also in `/tmp/ai-guard-broker-intent-red.WgIouT/final-test-target-before-green.txt`. The initial RED-run hashes remain in `source-hashes-before.txt` and `source-hashes-after.txt`.

## Contracts represented in the target

- Independent v1 golden JSON and SHA-256 over `ai-guard.execution-intent.v1`, NUL, then recursively key-sorted compact UTF-8 JSON; golden hash: `f6f8fa6cc4991a3d03a00655b5d57a5bb186e3e02bb2a1b4ac07adede936c616`.
- Strict `from_bytes` roundtrip and fixed eight-field top-level wire shape; legacy plan fixture includes a provider binding reference, CR/LF and Unicode prompt, and a deliberately absent provider-source path that parsing must not consult.
- Typed `ManagedCheckpoint` fixture with `WorkerMutationV1`, plus mutations to pending mutation, runtime observation, and terminal result that must leave the immutable intent hash unchanged.
- Stable operation/session/options and binding metadata changes must change the hash.
- Refusal cases for a duplicate top-level intent field and unknown fields at intent, option, request, and provider-resolution levels; unknown versions; absent or nil plan-v1 operation ID; managed selectors in plans 1/2 (while retaining manual provider labels); pre-applied approval; bad digest/reference; provider input argv/provider/credential options and mismatched runtime/image/project/config root; explicit client/auth/profile/model incompatibility; session/command/profile/transport conflicts; and prompt/argv disagreement.
- Auto and omitted client/auth choices are accepted only with a frozen, compatible provider resolution; prompt bytes preserve Unicode and CR/LF exactly.
- Source and canonical size limits; opaque diagnostics; historical admission serialization and decision hash unchanged.

## Golden provenance

`tests/fixtures/broker_intent-v1.canonical.json` was independently generated from the documented eight-field wire using Ruby’s JSON parser, recursive key sorting, and compact UTF-8 JSON serialization. The SHA-256 constant was calculated separately over the stated domain prefix plus canonical bytes. It is not derived from `ExecutionIntentV1` implementation code.

## GREEN and targeted canaries

After root/core confirmed the frozen implementation hashes, `cargo test --locked --test broker_intent` exited 0: **5 passed, 0 failed, 0 ignored**. The output is preserved in `/tmp/ai-guard-broker-intent-green.HP3P8U/test.log`; before/after source, fixture, Cargo manifests, and exit status are in that private directory. All hashes were identical across the command and matched the frozen hashes supplied by core (`intent.rs` `cb1a12e4…`, `broker/mod.rs` `13bb6fc4…`, `runner.rs` `5131cf9e…`, `provider.rs` `354e6789…`).

Additional requested canaries, with unchanged before/after manifest, are in `/tmp/ai-guard-broker-intent-canaries.1aKEgv/`:

- `cargo test --locked --test provider_authorization --test supervision_contract`: **11 passed, 0 failed, 0 ignored** (6 authorization + 5 contract).
- `cargo test --locked --test worker_registry fixed_sandbox_v1_admission_bytes_and_hashes_are_historical_goldens`: **1 passed, 0 failed, 35 filtered out**. The filtered tests were not run or counted.

The first error in the shell wrapper was only that macOS has no `/bin/printf`; both Cargo commands had completed successfully, and exit-code files were then written with `/usr/bin/printf` without rerunning tests. Their logs preserve the original output and the before/after manifests compare identical.

The earlier RED remains classified as absent-feature compile failure, not a behavioral finding. No model, runtime, Docker/Podman, or E2E test was run for these projection/serialization contracts.

## BIR-01 snapshot diagnostic redaction regression

Added one focused test, `public_snapshot_deserialization_errors_do_not_echo_canary`, after review found that public `InitialOptionsV1` and `ProviderCredentialRefV1` derived `Deserialize` could expose raw serde diagnostics. The final expanded test source SHA before core changes is `daf7eaa0a27d6304dce8ca2b114895bf852417768430dbc06e9b1fe352d0e529` (formatted; production source stayed at the frozen hashes above).

Before the core fix, the final single filtered command `cargo test --locked --test broker_intent public_snapshot_deserialization_errors_do_not_echo_canary` exited 101. The aggregate oracle observed leaks in both Display and Debug for an unknown `InitialOptionsV1` key, an invalid runtime enum, and an unknown `ProviderCredentialRefV1` key. The invalid credential UUID and opaque `InitialRequestV1` / top-level `ExecutionIntentV1` controls passed. The final run and identical source manifests are preserved in `/tmp/ai-guard-broker-intent-redaction-red-final.z3caW3/`. An earlier complete run without the binding-key case remains in `/tmp/ai-guard-broker-intent-redaction-red-full.eMfnmn/`; the initial shorter RED log is also retained in `/tmp/ai-guard-broker-intent-redaction-red.7gOG1l/`. These are stages of the same test development, not separate behavior claims.

This is a contract failure in error redaction for the public snapshot type, not evidence of an observed application call path or runtime secret disclosure. Core then added opaque public deserializers without changing wire/golden behavior; frozen `src/broker/intent.rs` SHA is `73078447289411744e410b8c2e7ee1db97444dde4cff4863c9455775795adbf3`. The single authorized post-fix rerun, `cargo test --locked --test broker_intent`, exited 0: **6 passed, 0 failed, 0 ignored**. Raw output and unchanged before/after manifest are in `/tmp/ai-guard-broker-intent-redaction-green.c5oRZT/`. The previously run authorization/contract and legacy golden canaries were not repeated because the reviewed correction changed diagnostic opacity only, not wire or behavior contracts.

After the first BIR-01 GREEN, `cargo fmt --all -- --check` and `cargo clippy --locked --all-targets -- -D warnings` both exited 0. Their logs are in `/tmp/ai-guard-broker-intent-static.AEbM5T/`. That command's initial before/after manifest used `git ls-files` and did not include untracked new files; an explicit current selected-file hash manifest is saved as `selected-current-hashes.txt`. After the later boundary-test refinement, both checks were rerun and their explicit before/after manifest (including new source, test, fixtures and Cargo files) is in `/tmp/ai-guard-broker-intent-final-static.gNbFiX/`.

## Final boundary-oracle refinement

The reviewer noted the previous size guards did not distinguish the intended size error from earlier JSON/path parsing failures. The test was refined without changing production or fixtures (final test source SHA `2bf54a9c45b113cb815ce03b306c3d23b13bf1fc826fe7a8a6c3330941e09bca`): a valid plan plus trailing whitespace exceeds 32 MiB and must return `IntentError::SourceTooLarge`; a valid golden intent plus trailing whitespace exceeds 256 KiB and must return `IntentError::IntentTooLarge`; and a valid 15 KiB U+0001 prompt replicated in options/provider input/argv/request keeps argv JSON at or below 96 KiB and input plan below 32 MiB, while the canonical intent exceeds 256 KiB and must return `IntentTooLarge`.

The one allowed test rerun after this test-only edit, `cargo test --locked --test broker_intent`, exited 0: **6 passed, 0 failed, 0 ignored**, with unchanged before/after manifest in `/tmp/ai-guard-broker-intent-bounds-green.WjYb4I/`. Then `cargo fmt --all -- --check` and `cargo clippy --locked --all-targets -- -D warnings` both exited 0; their explicit selected-file before/after manifest and logs are in `/tmp/ai-guard-broker-intent-final-static.gNbFiX/`.

Following favorable reviewer authorization, one `cargo build --release --locked` exited 0 in 34.83 seconds. The before/after SHA-256 manifests for all Rust files under `src/` and `tests/`, Cargo manifests, lockfile, toolchain, and the intent fixtures match exactly (`/tmp/ai-guard-broker-intent-release.IF4htX/source-hashes-before.txt` and `source-hashes-after.txt`). The new `target/release/agentsandbox` hash is `831a3e19854e29f35dade316f71cb463c43faacf49f5f96204277b5ac85b8e1a`; `target/release/libagentsandbox.rlib` is `7edfc144d56f656bcc9980ec68dd83a42e2554b4a4b2bdce210d2fc8c1b6c280`. The prior binary and rlib were preserved privately with hashes `d707f22344c1fea43bcbb90eb64cd3a4cc516903f10efecee35f500b3a7faf31` and `b4f7c6a28d0a7ce904995e0512749b061fb0ffca63d738b523eb0c5c8c708e21`. This is a successful software build, not a product release or production-readiness qualification.
