# Owned egress policy API/CLI A1 — 14 November 2026

[Français](README.fr.md) · [Exact selections and receipts](SELECTIONS.json)

**74 targeted software parents passed on one exact source. The operator
journey remains incomplete.** A1 connects policy-registry replace/revoke to
the reserved technical API and the four CLI operations prepare/apply/resume/
status. It does not apply network policy, authorize a connection, start a
worker or establish human approval.

## Source and two distinct final QA runs

Both final runs used commit
`db2678d42cff9c8d6d15bf31284a989732cc6f0d`, tree
`79a378461d75d46a8d102c9126d04251422b4dda`, input archive SHA-256
`40719c46c00d1793cea3518a48950a71501f00982e56cc672480729060a2a88d`.
This is source-specific evidence. It does not transfer to another HEAD or a
later public composition. The documentation changes were prepared against
public `0dab5f78464931301183824eb81724488981ce2e`; that is not the tested source.

| Run | Observed result |
|---|---|
| GREEN R2 build/check | Test-artifact build, normal CLI build and all-targets check RC0; 156 compiler warning messages. The host Bash 3.2 harness then exited 1 on an empty local array under nounset before registration. Zero lists, all 74 tests NOT_RUN in this run. |
| Tests-only resume | Reimported the three original Cargo-bound executables, byte-exact at their original isolated paths. No rebuild, check or download. Eighteen fresh lists, then eight groups: 74 unique parents PASS, 73 ordinary plus one default-ignored parent explicitly executed. Zero failed and none selected left ignored; terminal 0. |

The failed build/check harness is preserved as a failure; the successful
resume is separate evidence. The resume inherits the prior actual RC0 build
and check, not a new compilation claim.

Actual build commands in the admitted offline QA:

```console
/usr/local/cargo/bin/cargo test --frozen --offline --lib --bin agentsandbox --no-run --message-format=json
/usr/local/cargo/bin/cargo build --frozen --offline --bin agentsandbox --message-format=json
/usr/local/cargo/bin/cargo check --frozen --offline --all-targets
```

The library libtest, binary libtest and normal CLI were resolved from actual
Cargo JSON. The normal CLI was invoked by the vertical parent. The 18 lists
are two global lists, eight exact-group lists and eight ignored-group lists.
Functional argv use `--exact` FQNs, `--test-threads=1 --nocapture`; the vertical
also uses `--ignored` and the checked normal-CLI artifact. Exact names,
argument order, artifact SHA-256 and Cargo JSON hashes are retained in
[SELECTIONS.json](SELECTIONS.json); no raw credential or private path is published.

## Actual selections and behavior

| Group | PASS |
|---|---:|
| Core API/SDK/Store contracts and ACK canary | 19 |
| Existing E1 policy-store canaries | 19 |
| Existing A0 canaries | 9 |
| Durable filesystem operation facade | 8 |
| Main CLI libtest | 10 |
| CLI absence-baseline canary | 1 |
| Shared modern readers | 7 |
| Normal CLI / HTTP vertical, default-ignored run explicitly | 1 |
| **Unique total** | **74** |

The actual Router/SDK/SQLite tests cover retained technical identity and live
source binding, format 15 before body polling, scope/CAS conflicts before
mutation/replay, bounded closed input, true post-body WriteEpoch work,
mutex/SQLite refusals and restoration of the 713 ms timeout. SQLite-FULL uses
a bounded logical database, not a physically filled disk. Observer loss
leaves accepted work and permits attached to the actual job until drain;
the ACK canary proves the waiter received its release rather than only a
send. A binding revoked after real commit cannot publish the receipt.

Filesystem/CLI selections retain immutable preparation, refuse unsafe or
changed records without repair, require explicit pending recovery and verify
file/operation-directory/parent synchronization. An injected failure after a
real sync is not claimed to be an observed fsync errno. The normal CLI
vertical loses a receipt after real commit, preserves the operation, replays
the same bytes/UUID and requests revocation. Its receipt is historical
`policy_registry_only`; `revoke_requested` is no observed runtime closure.

## Preserved failures and baseline

- The test-only baseline on
  `ae8d5210272f2af903da53f999ab03d63d1fd086` built and listed its two real
  callers. Both reached expected absence oracles after setup: reserved SDK
  diagnostic 200 then POST 404 instead of 200; existing control-info parsed but
  policy/status was an invalid subcommand. Each parent exited 101, zero PASS.
  These are expected feature-absence failures, not vulnerabilities or PASS.
- GREEN R1 on `0abb6efe045701e0d3f76548a15392db8159c41d` exited 101 at
  compilation with three Rust errors. Tests, normal CLI and check NOT_RUN.
- GREEN R2's later harness 1 remains recorded; only the separate tests-only
  resume supplies the 74 PASS. Overlapping runs are not added together.

## Preservation and bounded QA profile

Each final run preserved all 2813 source files, including 18 private 0600
files, with exact paths, contents and modes. Each completed seven collectors
and closure at RC0; the exact QA container was removed and OOM/OOM-kill were 0.
The different post-run archive hashes in the typed record describe their
own serializations; they are not assertions that input and output archive
bytes are identical.

The resume used a Linux ARM64 tool image pinned as
`sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`,
UID/GID 1000:1000, network none, read-only root, all capabilities dropped,
no-new-privileges and seccomp 2; two CPUs, 4 GiB memory, no swap, 128 PIDs,
2 GiB scratch and 64 MiB noexec temporary storage. Its measured memory peak was
1252507648 bytes, OOM 0. This measures the QA, not a product worker, throughput
or 100-worker capacity. Opaque executables were not run on the host.
Receipt and directory FD flushes were separately verified; this is not
power-loss or restoration qualification.

Primary result/flush SHA-256:

| Evidence | SHA-256 |
|---|---|
| Build/check result | `c24896a4bc8fa649d23891e165dd00b13b5e65c27d4af9c00418f99124eb100a` |
| Build/check host flush | `aaba0fd7ef4d38af2559b5e6d1342659565ac3ea162c760c4e5311d284ad81a3` |
| Tests-only result | `ae52ce89e84dc85102e9f3e555fd20fd5ff1275ede62c2a223ea50eda9d07a74` |
| Tests-only host flush | `c5c587f0f7ab1d1741c228651d73e88e7a46f4f1211fd4ddc51bd77d17ca8bff` |

## Open operator path and qualification limits

There is **no supported command to obtain fresh reserved scope and policy
head/CAS** for preparation. Existing control-info uses the public diagnostic,
closed in owned mode; status is local history. The four commands require
actual retained control evidence, never inferred scope, guessed CAS or
initialization values, or user-side SQL. `expected_scope` is a comparison assertion,
never a Store/source/tenant selector or authority.

`inspect` and `prepare --inspection-dir` are not implemented. Their five
later tests on `05a4451d08856ce5253bce0103acf8b3d873057b` remain NOT_RUN and
are excluded from the 74. The [operator guide](../../../../setup/identity/OWNED_SOURCES.md#owned-policy-a1-operator)
keeps that gap visible.

No durable policy or receipt establishes network enforcement, connection
permission, observed execution or human approval. Broker lifecycle 3–13 and
relay 6–13 remain closed. Real runtime/worker/egress network, provider/SSO,
cluster, 100 workers, full suite, strict Clippy, physical crash/restore and any
other composed HEAD remain NOT_RUN. No production gate or historical audit
fingerprint is closed by this evidence.
