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
