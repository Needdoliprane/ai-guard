# Broker central lot — implementation record

2026-10-30. This is preparation of the approved supervision vertical, not activation or runtime qualification. Parent `/root` owns integration, documentation and API; Luna owns test execution. No provider/model/runtime call was made by this implementer.

## Source and scope

Private baseline `/tmp/ai-guard-broker-core-baseline.YbL9P1` preserves exact `control.rs` SHA `7f2422c1bd507f66793b03085310e4b433e90c9feb0909b834f02711448b05b6`, `audit.rs` SHA `a6e967bc89a8da3354de0d4de92e0f30c0dff441fb969713d8a69ebbca3f86c7` and `audit/control.rs` SHA `7ed543d65959ff9fd6a8d3500f4b0f95caeb2ea588ebda65eff5d454b8d2e9e7`.

Only application files changed by this lot: `src/control.rs`, new `src/control/supervision.rs`, `src/audit.rs`, `src/audit/control.rs`, new `src/audit/supervision.rs`. No API, CLI, config default, runner, runtime, dependency, provider context or existing credential file was changed.

DTO freeze: control `122e0d27d4bd184c11e6c4af18c942089a4c9d15ae07f3f31604808d37d82458`; control/supervision `9658a67cc8b030beb9418ec76eb71df4316dd0c8b77f938f6bf8f4737002d10a`.

Initial Store freeze: audit `d65534460a5028ec5ee0e063f36d5b8e225351ac24d66395283cf3f8bbe80832`; audit/control `34802e9f29ed6e1f0d81472d4ec569da8f1bc51096e3db26e85f7bff7a3dc852`; audit/supervision `567de58ad51910728a3144872fdae47114dfcf13c1cc037c32a49833521f811c`.

Pre-build static correction: runtime `running` is `Option<bool>`, so credible helper terminal requires `running == Some(false)`; `None` remains unknown. No build had run on the preceding candidate, as confirmed by Luna. Resulting audit/supervision SHA `eedb9253420e582a0f570d8267497d2662af2ea08ceffbd554da7b10e69ceb55`; other Store hashes unchanged.

## Implemented boundaries

- Strict DTOs; native-clock checked canonical decimal arithmetic; non-nil boot UUID; deadline not refreshed. Arm/claim receipt and parent shape validation; explicit identity-free Store metadata (`supervision_state`). Store never fabricates an HTTP role.
- Shared helper policy with separate domain `ai-guard:helper-execution-policy:v1`, parent worker/attempt/spec/session/manifest binding, exact immutable image/volume/name, fixed argv/resources/network/preset2 and no credentials. Helper Arm must carry this helper hash; worker Arm carries its parent worker hash.
- Explicit schema2→3 migration through the same guarded backup, fsync and exact preflight-head comparison as schema1→2. New databases still initialize schema2. No new migration CLI is exposed.
- Five-table supervision overlay plus helper index; exact SQL/internal-index allowlists and reconstruction from strict events. The old ControlRecord version1 and old inner hashes are unchanged. Record2 carries the complete typed new input plus the existing worker result record when applicable.
- Admission/bind/arm/worker and helper claims/helper observation/cleanup/export use one checked SQLite transaction with mandatory audit and final complete projection verification. Operation IDs are global in worker_mutations. Identical replay returns original receipts; consumed claims return AlreadyConsumed.
- Fresh legacy admissions and first legacy StartClaim are denied on schema3. Historical consumed replay remains read-only; existing read/observe/stop/cleanup contracts remain available. No standalone timer or caller boolean substitutes for central Arm/claim.
- ImportVerification is a distinct central purpose tied to the exact import helper Arm, successful credible terminal observation, admitted manifest and durable receipt; its permission is revalidated at claim. Worker CreateIssued and first armed claim require a credible ImportVerification report where expected == observed == admitted manifest. No Created-state exception bypasses this gate.
- Helpers retain separate observations; they do not invent worker allocations or release parent quota. Terminal helper evidence is sticky. Export binds an opaque destination UUID/hash to an immutable request; concrete path handling remains the broker's private responsibility.

## Validation evidence and limits

DTO preparation: Luna captured compile/behavior RED then five contract tests GREEN; the two corrected new-contract failures were nil boot UUID and claim revision mismatch. Historical worker_registry 36/36, including FixedSandboxV1 byte/hash golden, passed before the single Box representation fix. That fix preserves Serde wire/hashes; subsequent five contract tests, formatting, strict Clippy and one release build passed per Luna. Implementer ran only rustfmt.

Initial Store freeze: tests/build/review **NOT_RUN** at this writing; sent to Luna for real SQLite migration/backup/replay/corruption/idempotence/concurrency/worker+helper claims/audit-failure tests and targeted legacy canaries. No Store success is inferred from DTO results.

Remaining vertical: fresh IPC AuthContext, independently scheduled immutable stop targets, real native-clock/emission ordering, credential-safe broker recovery, helper filesystem effects and TTY guardian are outside this central scope. Runtime E2E, crash/fencing, Docker/Podman, quota bytes/inodes and capacity qualification are not established by this code. Restores/clones/rollback remain maintenance-only until external runtime authorities and in-flight starts are conservatively fenced; the migration method does not prove that operational condition. No financial billing is enabled.
