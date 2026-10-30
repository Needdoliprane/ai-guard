# Supervision HTTP API — implementation handoff

2026-10-30. Worktree only: `/Users/louis/Documents/ai-guard/.worktrees/broker-supervision`, branch `codex/broker-supervision`, base `e2cda5eb889b9fc8f8053085ccbaf6942792be67`. No product file in the main checkout was changed, and this implementer ran no test, build, commit, push, provider or runtime call. Parent owns integration/review; Luna owns test execution.

## Scope and provenance

Product files: `src/api.rs`, new `src/api/supervision.rs`, `docs/api.md`, new `schemas/supervision-protocol.schema.json`. No Store, domain DTO, default schema, migration CLI, credential configuration, dashboard proxy or runtime behavior changed.

Baseline copies `/tmp/ai-guard-supervision-api-baseline.OGWSTC`: api SHA `4db5d205e0571a5843e6b92edda61c727c3c14f296d6bbba3631f3d71116e393`, docs API SHA `40d0c16420d2f331691dca0a7fc979892d35cd11250a6ac8b127eacce604e709`.

Luna RED: `/tmp/ai-guard-supervision-api-red.dqtr1K/test.log`, three behavior groups failing because the nine routes were absent (protocol404, reservation405, other subpaths404). Authentication assertions on absent routes are not interpreted as authentication vulnerabilities. No product edit preceded that RED.

## Implemented HTTP contract

Only `/api/v1` merges the new module. Existing `/api` and `/api/v1` historical routes remain unchanged; the same wildcard `{id}` is reused to avoid Matchit conflicts. An absent new alias may still match a preexisting generic legacy route and return405/400 rather than404; there is no new supervision handler under `/api`.

- GET `control/protocol`: Reader first; returns typed ControlProtocolV1, Store capability facts plus tenant/role solely from authenticated identity.
- POST `workers/supervised-reservations`: Runner first; typed SupervisedAdmissionV1, write limiter, shape/redaction/canonical bounded Store-envelope preflight. Durable operation lookup precedes external policy; fresh requests require schema3 and active origin before fresh_policy. Mutable quotas remain transactionally checked in Store; no unsafe quota preflight substitutes for Jev. Acceptance201, replay200, durable policy403/quota429.
- POST `workers/{id}/arms`: ArmResultV1/200, explicit replay flag; body worker must match path.
- GET `workers/{id}/arms/{arm_id}`: Reader first; actual arm resolved inside identity Store and matched to canonical worker path. Scoped absence/wrong parent404, response200 ArmResultV1 with replayed:true from Store lookup.
- POST `workers/{id}/armed-start-claims`: MutationResultV1/200; matching path and actual stored arm before transactional claim.
- POST helper-start-claims/helper-observations/helper-cleanup: typed results200; each loads the actual arm in the authenticated Store and matches its actual parent to path before mutating. Receipt JSON is never trusted as proof of parent identity.
- POST export-requests: MutationReceiptV1/200, matching worker path and Store-authorized immutable destination intent.

All seven POST handlers place RunnerIdentity before Path/State/Json, use the existing tenant write limiter and normalize Result<Json<T>,JsonRejection> to generic400 without echo. GET handlers put ReadIdentity first. ControlFailure mappings are reused, including503 supervision_inactive. Store transactions remain authoritative: maintenance can return exact consumed replay/history but never a fresh EmitOnce. There is no fresh Bind route, activation switch, engine operation, provider-secret response or private runtime binding in this HTTP surface.

API docs describe the typed requests/responses, role/parent checks, errors, V1-only scope, provider-cost preflight and remaining operational limits. The schema follows the existing draft2020-12 style and explicitly covers only ControlProtocolV1; tenant syntax is aligned to actual `tenancy.rs` (1..128 ASCII alphanumeric/hyphen/underscore), active requires schema3/non-null namespace/broker_supervision_v1. Other contracts continue to be the strict Rust DTOs, not an invented complete OpenAPI schema.

## Initial source freeze

- api.rs `854e654f153f70eee1c9ee8532dab85bd4012aae8bae541f40a5d3a98e40b893`
- api/supervision.rs `295cc728975d18c55e5f053908bc05b50471b4abfafc6f9cb8db25722d4d81b2`
- docs/api.md `6a30f805da1c017de78471be349b50413f4c64590904d77d7fbc8f5a15a76509`
- schema `2a560d62f1f9fb3f61d58cad4b1df36b98efd2529de76c87e13b8a7db1ad20fd`

Rustfmt on owned source files only: PASS. Compilation, API GREEN, schema/document checks, Clippy, release and independent review: NOT_RUN at this freeze; Luna/parent coordinate after the independent Clock module is stable, to avoid intermediate compile failures. The earlier Store94/24 proof does not qualify this new API code.

Operational limits stay explicit: default schema2, no migration/activation CLI, no broker/IPC/gardian installed, no restart reactivation or D5 fencing proven, no real runtime/provider/client qualification added by HTTP tests. This is the next concrete control API lot, not a qualified end-to-end supervisor or production release.
