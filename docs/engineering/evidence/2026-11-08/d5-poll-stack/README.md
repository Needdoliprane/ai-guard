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

## Composed c4: helper observed, legacy handoff still fails

The complete immutable C4 QA input manifest has SHA-256
`b75c67ec39252291ee7c72f6eba1c7ed7ec524b0d6b455bdedecd53fcb929a31`.
Its executor source has SHA-256
`a4f73a0ab77a6f46793998544ee1736b1cf8a399cde1f868c2f01657b06a7b06`.
The source review covers the extracted caller phases; helper and worker
bodies, generation checks, audit order, deadlines and permissions remain
unchanged. This bundle remains separate from the integration branch.

```text
cargo check --all-targets --locked --offline
cargo test --test broker_relay_handoff --no-run --locked --offline
cargo test --test broker_relay_handoff --locked --offline software_broker_missing_receipt_expires_under_original_arm_without_exec -- --exact --nocapture --test-threads=1
```

The first two commands return RC0. Their log SHA-256 values are respectively
`fd0d619347439e5be78d73a1ebf6948208fe3420bcfd2b7e71f8d0af186601df`
and `f8f7a270c705c3a482dc180d5d55a77e59f77668a4ddc41c57d446eb1f111618`.
The unchanged parent is executed once: **one FAIL, RC101, 25.84 s**. Its log
has SHA-256
`dc832be1bcf75b293e9c815712b66cd31a420a6d7c1643e11b0e015effae8b62`;
its status capture has SHA-256
`ab66827896aa2bbe1dfd65c4c2c2e30346b13e9caa0e12eb7e77e76289df67b2`.

The 224-byte broker stderr contains four retirement-audit-pending notices and
no stack-overflow text. Its SHA-256 is
`7d4985b19b6dbeeb4702af324d95007aa2e484f3ce7e340d81d580d0656a2130`.
Whitelisted Core/SQLite facts show worker Preparing at revision 2 with quota
held, a 120-second helper Arm, a helper claim with its Arm receipt and a terminal
helper observation (exit 0, not running, no OOM). The durable acknowledgement
of that observation in the local broker draft remains unknown. No HelperCleanup,
relay or terminal-worker transition is recorded. The observation has no import
verification, which is a separate later operation. These facts show progress
beyond c3's pre-Arm point. The pending-retirement notices alone do not establish
why the parent expires.

The actual C4 CLI in isolated QA is 175,753,152 bytes, SHA-256
`5cfec2bdf4e107303eb027ab1f87cfd82dacd8cf75cffe9a958c7a8158525240`.
Sixteen bounded prologue captures (54,425 bytes) on this CLI are retained as text, SHA-256
`27781360160af7468a8a0e90e2c109b7440508cf4c2dc63fdfff9135c00b2ca4`.
The individual poll frames reserve 19,648 bytes for `BatchExecutor::run`,
26,880 for `Execution::open` and 592 for `execute_active`. The extracted
cancelled, runtime, import, worker and terminal phase poll frames respectively
reserve 198,576, 69,088, 272,800, 1,968 and 139,776 bytes. Their constructors
respectively reserve 55,184, 23,440, 61,104, 80 and 34,928 bytes. The constructors
of `run`, `open` and `execute_active` reserve no frame. These are individual
static reservations, not a sum of simultaneously active frames or a captured
fault stack. This is not a measured stack maximum, successful handoff or runtime proof.
An earlier metadata capture selected the wrong ELF and created an oversized
temporary symbol listing; it was discarded and is not used as evidence.

A distinct test-only Full3 overlay keeps the original guardian alive and
restarts only the broker. Its complete QA input manifest has SHA-256
`72570a3843a35a11c99cfb7a83deedf4f176208185e2817015af7e84d67564bb`.
It changes four test files and no application or Cargo source. Verification of
every manifest entry returns RC0 on the copied QA source, using its own target.
Its `broker_relay_full3 --no-run` invocation returns RC0 after 2m12s, log SHA-256
`2994d9ae9ba6f522abad434472c5dd9dcc0c74ef210e767b4ce594cbaa7721ea`.

```text
cargo test --test broker_relay_handoff --locked --offline software_runtime_witness_refuses_contradictory_create_flags_before_mutation -- --exact --nocapture --test-threads=1
cargo test --test broker_relay_full3 --locked --offline software_full3_restores_original_assignments_before_late_issue_two_endpoints_relay -- --exact --nocapture --test-threads=1
```

The canary returns **one PASS, RC0, 5.34 s**, log SHA-256
`d7739a7654a64547324fd2e122c4ac56d056b94f69672c64ceb3617ec5282fce`.
The single positive parent returns **one FAIL, RC101, 26.39 s**, log SHA-256
`2da688ab59c9533909850fa86e0acfca9c64c2ff1bf651de2e315f9952d87b02`
and status SHA-256
`90936e778edc3e8870e115b8447ebd35fa9b6ac39c9c70c4f34d375251729ed0`.
It expires before Running/Awaiting, before the broker-restart and relay
oracles. Core/SQLite shows two tenant-A workers Preparing at revision 2, quota
held, two helper Arms and claims, no helper observation or relay effect.
Tenant B has no worker. Broker stderr contains six pending-retirement notices
and no overflow text; the guardian stderr is empty. The whitelisted summary
has SHA-256
`9d16d2d8f1a167a5de49b6531224e6b15e1781747b53f231c49641d521788449`.
Local-draft ACKs and the exact timeout cause remain unknown. The harness
expiration alone does not establish a product latency defect. Neither negative
parent was run, and no retry or deadline increase is credited. The engine
witness remains synthetic; no Docker/Podman worker, cluster or global gate
is qualified.

## Full3 f4/f5: bounded diagnostic and longer harness observation

F4 changes two test-support files after F2, without changing application,
Cargo, assertions, polling or product deadlines. Independent review confirms
its fixed categories, 100 aggregate keys, 64 retained transitions and one
diagnostic emission of at most 16,384 bytes after prerequisite failure.
Gateway timings end at handler responses; witness timings describe synthetic
runtime handlers, and JobStatus is a broker projection. Local helper ACKs
remain explicitly UNKNOWN.

The complete F4 input manifest is
`bbf536e86559ec57217ce9b7a4128d855a753ccfe51b68c0377099a5da98e4a6`.
Its shared strict-parser canary returns one PASS, RC0, 0.01 s after 2m07s
compilation. The single original positive parent then returns one FAIL,
RC101, 26.22 s. Its two sequential IPC categories account for about 23.68 s
of the 25-second observation. The witness reaches its first Import start,
input completion and terminal mutation at about 25.085 s from trace setup.
These are progress observations, not a causal diagnosis or capacity result.

F5 therefore changes only the Full3 harness observation from 25 to 60 seconds,
after independent review. Arms, requested worker lifetime, assertions, polling
and application code remain unchanged. The historical handoff harness retains
25 seconds; the Legacy role inside the Full3 harness uses its new 60 seconds.
The complete F5 input manifest is
`f5bd629738b1e52a268a38b2bbdec34f4a4137f5ae5f8d1bbd214766caec191e`.

Both inputs are copied and fully manifest-verified, with separate Cargo targets
in the same isolated offline QA described in the [TLS proof](../broker-private-ca/README.md).
Each positive parent is invoked once:

```text
cargo test --test broker_relay_full3 --locked --offline software_full3_restores_original_assignments_before_late_issue_two_endpoints_relay -- --exact --nocapture --test-threads=1
```

F5 still returns **one FAIL, RC101, 61.24 s**. Its trace records two successful
Import start/input/terminal witness handlers and two successful gateway helper
observation/cleanup handlers. The last JobStatus projection is Preparing,
head revision17, pending RPC count0, no failure/error, executor running.
The last RelayStatus call is cancelled by the harness deadline. This neither
proves that call caused the failure nor proves any specific durable local ACK.
Running, broker restart and relay exchange oracles are not reached.

The F4 diagnostic block through the Error line is 7,444 bytes, with 19 metric
rows and 64 retained transitions. F5 is 7,908 bytes with 22 metric rows and
64 retained/526 evicted transitions. The earlier report of 4,348 bytes and
zero aggregates counted only a filtered subset and was corrected against
the raw F4 log. No raw log was changed. No further parent or retry is credited.

| Capture | SHA-256 |
|---|---|
| F4 canary log | `a75230fbf463c38b6cdf5612bd313dcc4c44b2394c1180cea9629b0ad33d117d` |
| F4 parent log | `66498951f3861c2121b128dafde0c589689d1ca3d8ddbec6ece4d59fcfd1cfac` |
| F4 parent status | `e9b6bcc18fd7dd34f2613ba838716da9a5d9e6d6562e4a84fcb20bad1015383f` |
| F5 parent log | `5b747b8c0055772c3b58788ea4499dc93f994639eec240c70e4098e42f3083af` |
| F5 parent status | `5b85031a432a9a75d0c3f4c199e39c58403fc5bb8430220ca3e9faebf42c67a7` |

Both postflights show no OOM. No model, real worker engine, physical federation,
Kubernetes session, performance threshold or global gate is qualified.
Future D5 composition must preserve the separately delivered explicit gateway
trust; these older C4 application sources do not contain that later change.

Baseline77 private provenance-note SHA-256:
`82486255cd72570ea5c96a75aa4d75b51e28ce94e78276c85a4182f47c8f9127`.
The source snapshots and bounded text captures remain in private engineering
evidence. No model, Docker/Podman worker or Kubernetes operation is qualified
by this diagnostic. D5 integration and the global gates remain open.
