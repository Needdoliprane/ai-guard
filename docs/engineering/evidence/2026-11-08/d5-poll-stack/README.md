# D5 polling stack — baseline77 static evidence

Status: diagnostic evidence, **no fix or passing Export oracle credited**.
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
Arm and audit-before-release conditions remain unchanged. Its compilation and
original parent are **NOT_RUN** at this checkpoint. The smaller frames and
the absence of an overflow message in91 do not establish a completed D5 fix.

Full private provenance-note SHA-256:
`82486255cd72570ea5c96a75aa4d75b51e28ce94e78276c85a4182f47c8f9127`.
The source snapshots and bounded text captures remain in private engineering
evidence. No model, Docker/Podman worker or Kubernetes operation is qualified
by this diagnostic. D5 integration and the global gates remain open.
