# Immutable execution intent — implementation freeze

2026-10-30. Worktree only `/Users/louis/Documents/ai-guard/.worktrees/broker-supervision`, base `11eb871db40ca9893a4492547561ec604247257c`. Scope: new `src/broker/intent.rs`, one export in `src/broker/mod.rs`, a bounded private bridge in `src/runner.rs`, and the root-approved pure prompt helper in `src/provider.rs`. No docs/tests/Cargo/other product module/main checkout, commit or push edited by this implementer. Existing runner/provider/mod baselines are preserved privately in `/tmp/ai-guard-broker-intent-baseline.mToxa0`.

Root approved the existing design plus the concrete active-plan section. Luna captured preparatory RED before product code: `/tmp/ai-guard-broker-intent-red.WgIouT/test.log`, E0432 missing broker::intent, not a runtime vulnerability or useful compilation result. Luna owns tests/fixtures; test SHA at freeze `46986706813facad51726d62718df5d3763a215e7c06945cfdf4e84ad090cdad`.

## Implemented contract

- Immutable private ExecutionIntentV1, eight serialized top-level fields exactly: version, plan_version, operation_id, session, initial_options, initial_request, provider_input, provider_credential. Option top fields serialize as null when absent. New intention version1 is distinct from source plan versions1/2/3.
- Public from_plan_bytes/from_bytes/validate/canonical_bytes/intent_hash return IntentResult with unit-variant IntentError and fixed opaque diagnostics. Public Serde deserialization validates through strict typed wire structs before any Value conversion. Duplicate/unknown keys are rejected without retaining arbitrary values.
- Source plans deserialize as the actual private RunPlan through a crate-only runner function. New InitialOptionsV1 is a closed projection mirroring RunOptions's actual fields and Serde omissions; RunOptions/RunPlan historical Serde and hashes are unchanged. Managed/credential_source never enter the projection.
- Source bytes are bounded before parsing at32MiB; from_bytes before parsing at256KiB; validated canonical intention at256KiB. Metadata, paths, image/argv, timeout/profile/network and identifiers are bounded. No path canonicalization, disk/network call, TTY validation, source credential read, Auto resolution or provider handoff occurs.
- Canonical JSON orders object keys recursively, preserves array order and strings; SHA256 domain is exactly `ai-guard.execution-intent.v1` followed by one NUL then canonical UTF-8 bytes. UUID-typed fields serialize canonically; textual path/argv/prompt values are not normalized.
- Non-nil source operation/session/request IDs, known versions, unapplied approvals, matching command/profile/session when present and transport are enforced. Optional historical business IDs retain their domain metadata semantics (length/control bounds, no invented UUID requirement). Source operation_id nil/missing is refused without repair; old reader unchanged.
- Existing pure validate_provider_plan is reused via a private runner projection; its implementation is unchanged. New validation also checks reference basename/UUID/lowerhex digest, provider_input equality to options except resolved argv/provider, original input empty argv/generic/no manual secret file/no applied approval, and compatibility of explicit client/auth/profile/model selection. Auto/absence remain legitimate with the frozen resolution. Plan1/2 reject every managed selector because choose_plan would have produced version3; manual provider-only legacy modes remain possible.
- The root-approved provider helper calls the existing private parse_bootstrap_argv and compares the frozen prompt exactly. No bootstrap parser duplication; Unicode/multiline arguments/prompts remain exact.
- Public getters expose new options/request/binding snapshots with private fields and opaque Debug, plus typed metadata. Raw historical RunOptions/DecisionRequest access is restricted to the crate for the pure runner validator. No setters or public mutable references.

## Initial source freeze

- intent.rs `cb1a12e42c923a67619632b92ff61145c88a768e217f76d21f7be8b3fa1a8c28`
- broker/mod.rs `13bb6fc40b56fae11905cfdaadeb2d311d467b3bc4ae420c608c0a76c935c094`
- runner.rs `5131cf9e8938d2e6ad222f15fd977b45f2e41281417e4010d81aa4dddf069d39`
- provider.rs `354e6789535d9ab8aff0c641354a90661b26b978e0c35b0a45590f57363f15bd`

Implementer ran rustfmt on owned files only. GREEN/Clippy/release/review remain NOT_RUN at this freeze; source sent to Luna/root/reviewer and will remain unchanged during their commands. Unrelated engineering-document modifications in this shared worktree belong to root, not this scope.

## Limits

The hash identifies invocation metadata and stored references. It does not attest a manual credential's bytes or prove that a provider binding file exists/matches its digest. The future journal must bind Submit/scope/intent plus actual manual/provider snapshots durably before ACK, using retained private sources without re-resolution. The old mutable RunPlan is neither rewritten nor automatically migrated.

Generic Serde deserialization enforces typed shape and canonical-size validation; the explicit byte constructors enforce raw input-byte limits before parse. Serialization deliberately contains private invocation metadata for a future private journal; Debug and component errors do not reveal it. No journal, accepted-job ACK, tenant authorization, activation, runtime effect, deadline enforcement, restart recovery or D5 fencing is delivered by this format.

## Review correction BIR-01

Reviewer identified public snapshot Deserialize diagnostics as an opacity-contract gap (no production caller or runtime credential disclosure established). Luna captured RED before correction: `/tmp/ai-guard-broker-intent-redaction-red-final.z3caW3/test.log`, unknown Options key/invalid enum and unknown Binding key appeared in serde error Display/Debug. Invalid Binding UUID, InitialRequest and top ExecutionIntent controls were already opaque. Final test SHA `daf7eaa0a27d6304dce8ca2b114895bf852417768430dbc06e9b1fe352d0e529`.

The only product correction wraps InitialOptionsV1 and ProviderCredentialRefV1 transparently around private strict data structs, keeping one field list; public Deserialize maps every inner error to a fixed message. Debug/getters/wire/canonical representation and business validation remain unchanged. No sandbox_id/session equality was added. New frozen intent SHA `73078447289411744e410b8c2e7ee1db97444dde4cff4863c9455775795adbf3`; other three hashes unchanged. Rustfmt only by implementer. Post-fix GREEN, re-review, strict analysis and release are pending; no earlier results are retroactively transferred to this delta.
