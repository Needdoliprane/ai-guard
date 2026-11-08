# D5 polling stack — baseline77 static evidence

Status: diagnostic evidence and scoped94b regression results; complete D5
integration and runtime qualification remain open.
The baseline77 parent previously aborted after entering Export. The separate
instrumented79 source aborted earlier and is not used to explain these frames.

## Exact input and method

Baseline77 source-manifest SHA-256:
`b763f5807542f399d4f8f4ea09b9ee1828451da9d9a251ec2974cbdf1c2ef5c5`.
Luna compiled in isolated QA AT with Linux ARM64 Rust 1.99.1:

```text
cargo test --locked --offline --lib --no-run
```

RC0; no test parent or runtime fixture executed. Build-log SHA-256:
`19e61e6c21f1d8da1bdf9380d5ffd2eaecc607d04868d437846dd7cdb4e26661`.
The 232,219,040-byte ELF remained inside QA; its SHA-256 was rechecked as
`58acf1afe7025ab5ee3000fa0664e3aab17f8e7bc74fe6683a43cb2e28f2b81b`.

Native `nm` selection was anchored immediately after address, size and symbol
type, excluding method names appearing only inside generic Tokio wrappers.
Twelve actual prologues were read with bounded `objdump` intervals of 384 bytes,
all RC0. Each tool invocation had a 40-second timeout; total text before the
provenance note was 122,733 bytes. No executable was copied onto the host.
The earlier AR captures selected wrappers and do not establish these values.

## Explicit AArch64 prologue reservations

| Function or poll | Address | Bytes reserved |
|---|---|---:|
| `run_export_oracle` | `0x3206d54` | 199,392 |
| join closure poll | `0x3121dc8` | 1,824 |
| Export wrapper poll | `0x3121790` | 225,424 |
| Export constructor | `0x3133e14` | 32 |
| `Execution::export` poll | `0x38dc7dc` | 516,544 |
| Helper constructor | `0x3133820` | 32 |
| `Execution::helper` poll | `0x2fa9c90` | 1,081,536 |
| `start_owned_prearmed_retained` poll | `0x1980350` | 160,128 |
| `execute_owned` poll | `0x197a1d0` | 163,424 |
| inner result poll | `0x19d30ec` | 210,384 |
| helper `on_created` poll | `0x2f65df4` | 220,064 |
| `arm_and_claim` poll | `0x2fa20a8` | 425,088 |

The source-inferred nested path runs from the synchronous oracle through its
join, Export wrapper, Export, Helper, retained runtime start, execution/result,
creation callback and arm/claim. Those ten polling/caller reservations sum to
3,203,808 bytes; constructors are excluded because they are a different phase.
This is static arithmetic on a source-inferred call path, **not a captured
backtrace**, actual thread stack limit, fault PC or measured maximum. It does
not prove that the failing77 run entered every listed frame. Tokio and other
callees are not included; future storage size is a different quantity.

## Next verification and retained limits

The candidate91 splits Helper preparation, retained transport and completion
into awaited heap-backed phases, with no new task, owner, deadline or stack
setting. Its compilation, new prologues and unchanged original Export parent
must be checked before crediting a correction. Smaller frames alone do not
prove durable abandonment, cleanup or absence of export success.

## Candidate91 result

Source-manifest SHA-256:
`1bc6de2f9cbb7be55d37135a33cad9ac9c1aa10bea951fbf01d6a6c5a65ab40a`.
Compilation with the same `--lib --no-run` command passes. Build-log SHA-256:
`6266479b815ee6ff9b2e3b8899c55bfd288b7ab28453c7c8462c555786a8af5c`.
The new AArch64 ELF stayed in QA, SHA-256
`0ea68299fea06b7dad98f4699c99f7fb54cd29d19eac96f50b71ad5c73887e6d`.

Twelve new prologues were inspected, all RC0. Helper orchestration reserves
12,864 bytes, its transport phase 91,920; Export reserves 316,480. Preparation
(519,392) and completion (239,248) are separate phases and are excluded from the
transport sum. The selected source-inferred transport chain, excluding the join
not re-read on91, changes from3,201,984 to1,418,192 bytes. This comparison has
the same static limits as the baseline; it does not measure actual peak stack.
New provenance-note SHA-256:
`be130d26cbdd524ea434acf9bfe9d9933ba6b2c5d973d1090037c5aee2e18824`.

Luna then ran the unchanged original parent, once. **FAIL, RC101**,171.22s;
no stack-overflow message was recorded. Export entered at90.039s, its watcher
failed at110.042s and Export reported `private enforcer start check failed`
at171.191s. Parent-log SHA-256:
`df51018854ffe58879b698aff1351d9ae72acec0e734b58acb380e4f68e6fb3c`.
The audit/cleanup oracle did not pass.

Native fixture metadata shows the real registration gate appeared about17.6s
after the watcher's20s budget had already expired; the release file remained
absent. That local watcher timer starts at Export entry, before durable
preparation. The closed enforcer error does not reveal its exact typed error
variant. This is not evidence of a permission/image refusal or a production
latency target. The fixture and text evidence were preserved without exporting
its SQLite database. Diagnostic-note SHA-256:
`b0a80f898c3f4f6adddc6d19abd32a262b6212dfa0e40ea1e080bb7ce529884d`.

Candidate92 changes only the fixture: it passes the existing global300s budget,
created at child entry, to the watcher. Parent budget, runtime Register/info,
Arm and audit-before-release conditions remain unchanged. The following AU
checkpoint supersedes its earlier NOT_RUN status. The smaller frames and
the absence of an overflow message in91 do not establish a completed D5 fix.

## Candidate92 and relay93 in QA AU

Candidate92 source-manifest SHA-256:
`fb32536a4797b31886b26f7fe14cfb112788dc19f8fe8b3dce20f9883eabc6e6`.
The initial compile observation handle became unavailable; no standalone RC is
credited to that invocation. The subsequent Cargo command printed `Finished`,
ran the original parent once and returned **RC101, one failure,221.56s**:

```text
cargo test --locked --offline --lib runner::broker::executor::journal::tests::export_retirement_tests::export_helper_abandonment_propagates_without_extraction_or_export_success -- --exact --nocapture --test-threads=1
```

Export entered at85.648s, the registration signal was audited at143.873s and
Export failed at221.540s. The retained-instance error names `Full3 assignment
publication requires reconciliation`; no parent timeout or stack-overflow
message was recorded. Parent-log SHA-256:
`77276db7a34820a017bbec273ddacf7d9c5c1c74be9cb27242d3913b9709a250`.

Whitelisted native JSON inspection of the retained revision42 confirms one
acknowledged WorkerClaim, one acknowledged Export HelperArm with the same
worker/attempt, no own HelperClaim and one acknowledged signal. Source review
finds that `retirement_checkpoint::ensure_unclaimed` rejects any WorkerClaim
for the attempt, including a helper's parent claim. This is a separate cause
from91's watcher expiry. Test-only candidate94a is prepared to discriminate
helper, worker and own-helper claims. Its exact positive test then reproduces
`Err(Conflict)` instead of `Ok(())` at the Export case, RC101,0.01s after a
2m17s compile. The two negative canaries pass, RC0 each, on the same source
and target. The code correction and original Export oracle remain open.
The test-only source-manifest SHA-256 is
`db35dbc40a60d6dd096250391e21e178b214fbce753b91549320550ccbf7c756`.

```text
cargo test --offline --locked --lib broker::retirement_checkpoint::tests::post_worker_export_and_verification_keep_their_own_unclaimed_subject -- --exact --nocapture
cargo test --offline --locked --lib broker::retirement_checkpoint::tests::worker_claim_still_blocks_the_worker_attempt_including_another_arm_id -- --exact --nocapture
cargo test --offline --locked --lib broker::retirement_checkpoint::tests::helper_claim_blocks_its_own_subject_without_blocking_a_sibling -- --exact --nocapture
```

The three log SHA-256 values, in command order, are
`a7cef361428b2d4d1ac605d9a1c3e7b8abb406e3142c51433b047800150e3bb5`,
`cf1eaac0b4ffbf37b97f232db7f20b5fb43755f7daa3eea39f91cef6b9d728f2`
and `83e8cfd1fb1e8ecc23bb3a88c9ee6c23e27c046158d4aac51a587e08a6f8da20`.

Relay candidate93 source-manifest SHA-256:
`ab27c3c95ad5ef4b9f12bcd5a511fdfd4fa7ab18ae803c8160b43e87bc10b9f0`.
Its compilation commands return RC0:

```text
cargo check --all-targets --locked --offline
cargo test --test broker_relay_handoff --locked --offline --no-run
```

Check-log SHA-256:
`2cc73d67be5cbefc89b7f575b616091b8642ce9b1c8c2bee1d0be9bd18c36527`;
no-run-log SHA-256:
`ff7829a74dc1b3e909fb88f0f2627b156d95c74b1718e854549d3d02e178d867`.
The single functional invocation returns **RC101, one PASS and five FAIL,
78.76s**:

```text
cargo test --test broker_relay_handoff --locked --offline -- --test-threads=1 --nocapture
```

Only the contradictory-create-flags canary passes. All five broker stderr
files report stack overflow after acceptance, before checkpoint progression;
deadline/reply errors alone do not establish latency or permission failures.
The candidate's heap construction change has not fixed the observed defect.
Functional-log SHA-256:
`39416389ee9223a8677761661ffd96b5046e312570e2e684b8dc418c02664c5b`.
The AU result/provenance checkpoint including94a has SHA-256:
`d75eca7d2413d9cbf53bd40a4edf3a031bbe8b5b993869aad12294411060dab9`.
The retained native93 forensic manifest has SHA-256
`e1bc620ced0041a5d0c9b9cafff57b0c75c7989b1254a9e09cbc20b230e55823`;
it covers the five stderr, revision markers and bounded prologue captures.
No rerun, model call, real worker engine or global qualification is credited.

## Candidate94b: discriminant fix and original Export parent

Source-manifest SHA-256:
`33f6beabf546217ae3454f33dedca691bb6fc9bb0708359335a0c0391d3aa960`.
Only the WorkerClaim predicate changes after94a: it additionally requires an
Arm Worker. The HelperClaim branch and binding/identity validations remain
unchanged. Independent source review covers its publication, retirement and
V5 callers, including the receipt hash committing the complete Arm.

```text
cargo test --offline --locked --lib broker::retirement_checkpoint::tests:: -- --nocapture --test-threads=1
cargo test --locked --offline --lib runner::broker::executor::journal::tests::export_retirement_tests::export_helper_abandonment_propagates_without_extraction_or_export_success -- --exact --nocapture --test-threads=1
```

The module invocation actually selects **seven functions, all PASS, RC0,
0.06s after2m13s compilation**. It includes the three required claim-subject
regressions and four codec/binding canaries. Log SHA-256:
`bf43437d626e7377f3e769f3714a107d7e7c692630e35d959a258983e37f8671`.
The original Export parent then passes once, **one PASS, RC0,200.21s**,
on the same source/target. Log SHA-256:
`e15b9481fdb1867409114b3687d272f0f93a49e587c0fd3a9a532558c58d6481`.

These results belong to94b, not92 or the Root composition. Its single changed
file is composed in the D5 candidate, but the bundle remains unintegrated.
The separate preclaim-shutdown test482 compiles, then fails before reaching
its HTTP barrier; that precondition failure is not an ownership RED. Its
test-only diagnostic96, relay95 and the broader recovery/engine qualifications
retain their separate statuses. No global gate or external finding is closed.

## Composed c3: ownership regressions pass, Import transport still overflows

The immutable c3 full-source manifest is
`c34c6ad753e00a93cbfe260b93e3876d428dda0130bc9908619f4a64529d710e`;
the independently generated QA input manifest is
`ef6fddd089486dbfe3318b6054b614882d74ddb5221beee7d01b1a7c61278eee`.
The bundle remains unintegrated. Owner-task changes passed source review
before execution; c3 then adds three discriminant tests and strengthens98.

| Selection | Observed result | Log SHA-256 |
|---|---|---|
| `broker::service::server::full_signal_owner_tests::` | RC0,3 PASS,0.01s | `c6931a47f29d1b4feef2eba431b0094caf3548da7c372c35e64ce1587958310e` |
| exact preclaim-shutdown98 parent | RC0,1 PASS,10.26s | `129d82af0d65b0d9a9d9bc40aa12471ba0aa11b8576731740502b196e79e0b84` |
| exact original Export parent | RC0,1 PASS,198.96s | `826ff3bbc691499d69f3ca25d9f18b26689c764653dddb3e0cac7f88e044d4e1` |
| exact `software_broker_missing_receipt_expires_under_original_arm_without_exec` | RC101,1 FAIL,25.81s | `1fd8de924cc66c6ee161cb3e48ebf1ace79dcd4e0839314fea8dd393fbfb9ade` |

The three new tests cover independent signal admission while the run pool is
full and joining the signal despite a failed or panicked run.98 observes
destruction of the actual IPC waiter and now requires successful shutdown
after signal completion, as well as the physical lease and audit conditions.
It reports `shutdown_result_ok=true`, `response_waiter_cancelled=true`,
`role_lease_held=true`, no ACK after release and zero commits without the lease.
The STOP-after-confirmation versus earlier-interruption distinction still
has no separate live parent in this selection.

The failing legacy parent's216-byte broker stderr contains two retirement
audit-pending notices followed by a Tokio worker stack overflow. Native
whitelisted journal inspection shows revision8/V3, Worker Preparing with quota
held, one Import helper with ownership recorded and only Decision, Admission
and Prepare acknowledged. No Arm or claim is present. This establishes the
Import callback as the next path to investigate; it does not establish entry
into `execute_worker`. No raw journal/database was promoted onto the host.

The actual c3 CLI ELF remained in QA:175,739,160 bytes, SHA-256
`ed0768630d930d2a1ec1935a7550fc00dbcfbdf4e7bbc767abd677a9f7ddf60b`.
Twenty-six exact prologues were inspected natively,384 bytes each, with
40-second per-operation bounds and233,230 bytes total retained text. The
selected Import transport chain reserves1,949,904 bytes statically, excluding
the already completed helper preparation. `Execution::run` reserves345,072
and `execute_active`596,672 bytes. Helpers, runtime, callback and Arm are
included only where selected on the source-inferred path. Tokio, wrappers
and other callees remain outside that sum. This is neither a captured fault
PC/backtrace nor a measured stack maximum or product capacity threshold.
The diagnostic report SHA-256 is
`1416114e3bd1bb6682066ff8d8c99789868704160825fd716763071be5fa7a6f`.

C4 therefore separates the two shared caller phases while preserving every
load/reload, generation check, owner, helper and worker body. Its source
review is favorable; smaller frames and successful compilation alone cannot
credit the original handoff oracle. The legacy parent also cannot qualify
the distinct Full3 assignment-restoration path. No global gate is closed.

The complete c3 source and bounded diagnostic texts are retained outside
temporary directories. The source tar has SHA-256
`6c146ba770b59428a0828c5c75beee917ec881d90d251814a677c0d21dd68711`;
its57-entry retention manifest has SHA-256
`625f32cf88dc9acd46e7417b799c8006a84d62eefe37f595f4c3523d8def36ec`.
QA AX was stopped and removed after targets completed and native metadata
was retained. No executable/database was exported; ephemeral fixture state
is no longer available after QA closure. Runtime qualification remains open.

Baseline77 private provenance-note SHA-256:
`82486255cd72570ea5c96a75aa4d75b51e28ce94e78276c85a4182f47c8f9127`.
The source snapshots and bounded text captures remain in private engineering
evidence. No model, Docker/Podman worker or Kubernetes operation is qualified
by this diagnostic. D5 integration and the global gates remain open.
