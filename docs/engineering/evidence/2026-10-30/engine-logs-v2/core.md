# P0 engine output persistence — core contract

Date: 2026-10-30. Scope: versioned control preset, registry/API regressions, API compatibility documentation. No audit-wide Daybreak campaign, engine invocation, provider call, credential access, commit, staging, schema migration or data rewrite was performed by this agent.

## Changes

- `src/control.rs`: retained `SecurityPresetV1::FixedSandboxV1` with its historical `fixed_sandbox_v1` representation. Added `FixedSandboxV2` / `fixed_sandbox_v2`, public const methods `requires_no_engine_logs(&self) -> bool` and `configuration_fingerprint_version(&self) -> u32`, selecting false/1 for v1 and true/2 for v2. No serialized field or hash function changed.
- `tests/fixtures/control/admission-fixed-sandbox-v1.json`: canonical historical admission bytes with deterministic IDs and no credentials.
- `tests/worker_registry.rs`: golden v1 serialization/request/spec/admission hashes; version-one envelope with v2 preset and distinct golden hashes; unknown preset/version rejection; mixed v1/v2 persistent replay; preserved worker contents and consumed claim; same operation or same request/session identity cannot replace v1 with v2; old v1 worker can still stop/finalize after reopen.
- `tests/workers_api.rs`: normal API fixture now submits v2. HTTP regression checks both explicit v1 and v2 acceptance, preset/spec binding, legacy replay, `409` replacement conflict, `400` unknown preset and created-inspection mismatch without audit writes, and v2 created response.
- `docs/api.md`: documents unchanged API envelope, exact v2 batch/no-engine-journal contract, compatibility and coordinated upgrade requirement. A trusted runner can explicitly admit v1; the new broker rejects its first start. Admission alone is not an observed engine configuration. Old broker/client v1 is outside the no-engine-journal guarantee.

No `src/audit/control.rs` change was needed: the existing `inspection_matches` requires equality between the inspection preset and the admitted launch preset, and replay validates the typed launch/hash while preserving the stored identity. No worker-specific JSON schema exists in `schemas/`; the Rust contracts are authoritative. Frontend read-only check found `front/src/control.ts::parseWorker` does not reject or branch on `security_preset`; no frontend modification was needed for v2 readability.

## Verification and evidence

- PASS: prerequisites read: AGENTS/RTK, architecture, code, gates, test strategy, tenancy, local/cloud and metering contracts. The project verification skill was absent from `.agents/skills/` and `.claude/skills/`; its automatic loading was not confirmed and it was not claimed as applied.
- PASS: exact `src/control.rs` delta reviewed against frozen `source.tar`: only enum variants and nonserialized methods changed; historical hash implementation and struct field order remain unchanged.
- PASS: `rustfmt --edition 2024 src/control.rs tests/worker_registry.rs tests/workers_api.rs` completed with exit 0.
- FAIL expected (RED): Luna executed `CARGO_TARGET_DIR=/Users/louis/Documents/ai-guard/target cargo test --locked --test worker_registry fixed_sandbox_ -- --nocapture` in `/tmp/ai-guard-engine-logs-red.CImJ4W`, an isolated copy of `/tmp/ai-guard-sqlite-freeze.DezAml/source.tar` plus the new registry tests and fixture. Exit 101: v1 historical golden PASS; both v2 tests fail exactly at deserialization with unknown `fixed_sandbox_v2`. Log: `/tmp/ai-guard-engine-logs-red-20261030.log`.
- NOT_RUN: initial live RED attempt could not compile because runtime code had concurrently added references to the not-yet-added v2 variant/methods. This is not the product RED proof. Log: `/tmp/ai-guard-fixed-sandbox-red-20261030.log`.
- NOT_RUN pending integration: targeted GREEN `cargo test --locked --test worker_registry --test workers_api` delegated to root/Luna after runtime freeze. Recommended adjacent legacy coverage: `--test usage_ledger`.
- NOT_RUN by this agent: engine effects, Docker/Podman/Kubernetes, provider/model, whole suite, and release qualification. These are not inferred from the core test result.

The golden constants were independently calculated with SHA-256 over the exact canonical bytes, with `domain + NUL + JSON` for spec/admission; the v1 constants also passed the frozen old implementation. Spec v1: `fc12ab2b6757338a1c8a1c01a8cc4abaa67d8f90d5f50a1679d81088a18995fa`; admission v1: `5104ce0ad4918f3271e04e8a85ea564dc3a99fcf79fe82ea823c7fd02e4ba68c`. Spec v2: `10677993c133ad52c34a97e18e09c8f740db63702687063f819c00e70597fc06`; admission v2: `9f7afae6f8dec5e98915b8f007488b554f436c24eb8bebcfa0f363c428a750c0`.

## Limits

The root/runtime agent owns the actual driver=none enforcement, Docker/Podman inspection compatibility, versioned private runtime fingerprints, first-start v1 refusal, runtime tests and engine proof. The core does not convert the trusted runner into an independently verified engine. No universal no-logging floor is claimed for old brokers or callers deliberately submitting v1. All production gates remain subject to their existing, separate evidence.
