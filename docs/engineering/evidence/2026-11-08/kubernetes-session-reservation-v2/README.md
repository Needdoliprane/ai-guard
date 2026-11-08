# Session reservation V2: migration baseline

Date: 8 November 2026. Status: feature-absence RED observed; migration 11 and
the reservation/recovery behavior are not yet implemented or qualified.

## Source and execution

The test-only input is based on
`4919abb4791bb5f75f792bfaa50ed3c3295c436a`. It adds a cfg(test) declaration
and one migration test; production code remains unchanged. The
[reproduction patch](tests-only-r1.patch) has SHA-256
`1dd5b9ef7bf951da231638064355c0a7b4f3ab4186ecae1cfe9bf2f3359c4314`.
Its complete 2,302-file source manifest has SHA-256
`06f3d5a3a71b147cbb6905cbf12a71c0c4dc74ba4236b3d830a6f69e6b2dd909`;
the retained source tar has SHA-256
`96b9bda49b0be449658319ac4e3467a56d0e2d21e9582724b2cfbc4efe9056bc`.
The patch is evidence against that base, not an additional test registered
in the integration checkout.

Root checked all archive paths, regular-file types and extracted source hashes.
Luna checked its separate source copy and target before one invocation:

```text
cargo test --locked --offline --lib audit::kubernetes_session_migration_tests::migration_10_to_11_preserves_a_held_v1_reservation_and_inactive_reopen -- --exact --test-threads=1 --nocapture
```

Execution took place in a fresh isolated Linux ARM64 QA with the pinned
`sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`
image and Rust 1.99.1. UID1000, no capabilities, no-new-privileges, seccomp,
read-only root/inputs, network-none, four CPUs, 8 GiB/no swap, PID256 and
4 GiB bounded scratch were verified. No host project binary, real worker,
model, cluster, external credential, existing database or volume was used.

## Observed result and limits

Compilation completes in 2m11s, then **one selected test FAILS, RC101, 0.17s**
with no ignored case and 672 filtered cases. Before the missing-feature oracle,
the existing production Store methods establish:

- A genuine V1 reservation with no CREATE claim, migrated through schemas9/10.
- One held allocation of 500 CPU millis and 134,217,728 memory bytes, a retained
  control namespace and one unchanged version10 worker-permission marker.
- Inactive reopen and matching audit rows, quota and namespace.
- A new real SQLite backup created by `migrate_control_version(backup,10,11)`,
  validated through Store integrity and compared with the baseline.
- `ControlFailure::InvalidRequest` for the unsupported target11, with source
  integrity and its logical snapshot unchanged.

The exact marker `schema11_red_preconditions_verified` appears before the
expected `schema11_missing_migration` panic. Independent source review checked
that backup creation precedes the unsupported-target dispatch. This is the
requested feature-absence RED, not a compilation or fixture failure and not
a demonstrated security vulnerability.

The backup comparison covers exact SQL event columns, schema, activity,
namespace and quota. It is not a raw SQLite-file comparison or a qualified
restore. Post-success assertions for the schema11 genesis, tables, index,
quota, historical prefix and reopen are not reached and receive no credit.

| Capture | SHA-256 |
|---|---|
| Test log | `af65cb42882caef39cd45397d47a3a18bec50a8ad22dbe9d050491758f2292d4` |
| Command/status | `f1f104db4867d52b490fc7dab7e8e25928cd8b03eb9e81047b99640b35d09433` |

Postflight finds no OOM and no remaining Cargo process. A missing private
Linux cache archive was repaired from an already supplied public input before
Cargo; that setup incident is separate from this test outcome. Root read the
log/status and authorized the implementation after this RED. Store/API,
recovery, migrations and runtime qualification remain open; no gate is closed.

## Bounded schema11 consumers

Component `a0f75f3fbbf8f663ccb01d0d7be3764dc0baf204` is composed in
`e2dcf81d360d1f0b3f17d0faf55f63f099b67ab4`. Sixteen explicit guard changes
and one diagnostic update in eleven files accept11 while retaining closed
upper bounds, protocol versions, roles, namespace/activation checks and version10
markers. No Store migration, session authority or Kubernetes execution is added.

The base is `4919abb4791bb5f75f792bfaa50ed3c3295c436a`. Candidate s11c1 has
2,301 files and manifest SHA-256
`113f03ebdfa6ea1b7568a1a1ab0525385041c384f9562b3c7e31573f58ca966a`;
the tests-only manifest is
`e3b8a0cdf319ba1458fc68edd570b25036b047b7accae543863f3f6be2df01d2`.
Independent review checked all paths, guard meanings, negative controls and
unchanged historical formats before execution.

Luna executed only these filters on each immutable source:

```text
cargo test --locked --offline --lib schema11_compatibility_tests -- --test-threads=1 --nocapture
cargo test --locked --offline --lib kubernetes_cli::identity_scope_tests:: -- --test-threads=1 --nocapture
```

| Source | First filter | Second filter |
|---|---|---|
| Tests-only RED | RC101, four selected/four failures at the expected11 acceptance guards | RC101, one pass/one failure rejecting supported11 |
| Candidate GREEN | RC0, four passes | RC0, two passes |

These are **six unique GREEN tests**, not twelve. The negative scope test
already passed before the change. Build durations are1m56s and2m00s; test
durations are below one second. Two existing Kubernetes dead-code warnings
remain; Clippy is not qualified by this run.

The fresh QA R2 uses the same pinned image/toolchain and confinement described
above, with actual kernel controls and read-only bounded shm/mqueue verified,
empty execution environments and private caches. Its input trees and targets
are distinct. RED used its default private `source-red/target` because
`CARGO_TARGET_DIR` was omitted; GREEN used the explicit dedicated target. The
actual ELF paths were checked; no target was shared. An initial setup-only
container lacking shm/mqueue bounds was closed without any cache/Cargo command.
No host project execution, model, real worker, cluster or existing database
was used. Root read the logs and verified component/integration source equality;
these are frozen-source results, not a separate Root rerun.

| Capture | SHA-256 |
|---|---|
| RED first filter | `f1ab2107e04df4af08a8ac0d12edca1771db45ebb14f7ea196307f0b5138b41a` |
| RED second filter | `bc83518bb1057fefe2619627104ffd0fada39b8103a91a1f4849a783007f125b` |
| GREEN first filter | `6cd2c896ee109fd5057d9fee26fff5ce95cab758bdf7a3c3b215c28e6a206cde` |
| GREEN second filter | `2604027387d8c9ffed480d1d79b745899c47118129831f22b8b68af3e11df74d` |
| Commands/status | `af38deca495abde83153196dd80613e78dccfdca70e1182f50e67ba3d90dc0a5` |

Compatibility with a genuine migrated Store11, authenticated consumers and the
Core11 all-target build remains NOT_RUN here. S1 storage source was returned
for a historical worker-quota replay correction before its execution. Neither
these pure tests nor the descriptive schema range closes a production gate.

## Historical worker quota baseline B1

Independent review of the unmerged S1 storage candidate found that worker
replay validated its durable decision/input but did not check the retained
quota at the admission prefix. Current normal writers already counted V2;
no ordinary writer exceeding its quota was demonstrated by that review.

The compiled B1 tests-only r2 composition includes S1, the reviewed consumers
and the public-import correction caught before QA. Its2,313-file manifest is
`327438cab459b586469b6fa19547e804cfd9b38ecb3941777d02b032f929960b`.
The actual test file has SHA
`34e052eab81cab47b08dc0068f28f5836000d55aebe6a85412761660e72e28b8`.
One invocation, in the same isolated QA as the consumers/C8, uses a distinct
explicit target:

```text
cargo test --locked --offline --lib audit::control::kubernetes_session_quota_tests::worker_admission_replay_counts_v2_at_its_prefix_after_close_and_reopen -- --exact --test-threads=1 --nocapture
```

**Observed RED: RC101, one selected test fails in0.31s** at the named
`schema11_worker_historical_quota_missing` oracle. Prior assertions establish
a real Store11, authenticated broker binding/grant, V2 reserve500mCPU/128MiB,
worker admission2000mCPU/1024MiB under quota2/2500mCPU/1152MiB/90s, followed by
V2Close, inactive reopen and unchanged audit. The real worker record validates,
and its relaxed quota clone still validates. Four memory-only clones tightening
count, CPU, RAM or duration are incorrectly accepted at that original prefix.
No database row, credential, receipt or audit hash is rewritten for this test.

| Capture | SHA-256 |
|---|---|
| Bounded test evidence | `461cc599595ec2900dec473bb12cdbcdf8ae588dd2f0f3df43aa45f0b599c159` |
| Command/status | `73144bc5dbefa54a97dcbab0894dccaaf003eee575793b21fa63268fd692fc61` |

The three-file correction uses a shared historical workers/V1/V2 quota helper
and has an independent GO_SOURCE review. Its GREEN composition has manifest
`45c4c05b20d14e57c0b4385deaf0c04213edbdd403fc8bd6018e3e194a0f4b41`;
the test is unchanged. GREEN remains NOT_RUN at this checkpoint. This is a
candidate storage validation gap, separate from the external run-1 findings;
no production, complete migration or runtime qualification is inferred.
