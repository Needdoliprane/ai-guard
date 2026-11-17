# D7: native STOP ownership — software evidence

[Français](README.fr.md) · [Results and fingerprints](TESTS.json) ·
[D7 contract](../../../D7_STOP_RECOVERY.md). Recorded 17 November 2026.

Source R5 is commit `6a32771ce76961f45d163767879d4ba08eeabcec`, frozen as
2,964 files. Its archive SHA256 is
`6c5554cd211c93abf44584b353dfef0bba04f9c975ec0ef920536dcfb6c24a19`.
The source review is GO_SOURCE_ONLY; execution results have separate receipts.

A genuinely fresh Legacy reservation receives its ACK after the real native
thread and runtime context, exact STOP snapshot and role lease are installed.
Bind keeps that owner rather than creating another start actor. One registry limits fresh
Original capacity to S and total native owner cells to S+W. With W>0, one catch-up
cell leaves S+max(W−1,0) permanent cells. Counts and transport health do not
prove that a container stopped or its IO was collected.

New cold/existing Legacy roles start fenced. An active-coverage acknowledgement requires
the corresponding reserved native STOP owner; refusal or unknown coverage
does not permit a new start and does not replace the old daemon. The R5 correction publishes Pending only on
first creation: retry strictly rereads and matches the existing Pending without
an empty revision. Missing/divergent state refuses before Arm RPC. It claims
no CAS atomicity across the native await.

**Sixteen selected R5 library tests actually ran and passed**, each with its
exact one-test oracle. Compilation and binary listings were independently
verified. This is not the full test matrix; results from different sources are
not added together.

| Campaign | Selection | PASS | FAIL | NOT_RUN |
|---|---|---:|---:|---:|
| R5, `6a32771` | Targeted library cases | 16 | 0 | 0 |
| R4, `5b82fdd` | Separate CLI/library campaign | 4 | 1 | 15 |
| R2, `a3c5370` | Historical library campaign | 24 | 1 | 17 |

R4 includes a real, explicitly invoked old-V1 compatibility case that passed;
it was not rerun on R5. Earlier FAILs and uncaptured individual values remain
historical, with no inferred replacement values. The exact FQNs and receipt
fingerprints are in TESTS.json.

R5 all-targets Clippy returned 0 in non-strict mode: 561 warning emissions and no errors. Without a comparable baseline, none is
classified as new. The strict Clippy gate remains open.

**This evidence closes 0/12 production gates and no global audit fingerprint.**
Automatic cold business reopen, Pending without a retained owner, Legacy
retirement/Q and quota release remain open. Physical STOP/deadline/IO effects,
real Docker/Podman engines, Kubernetes and 100-worker scale are not qualified.
Byte/read bounds also provide no synchronous stderr latency or writer-volume
guarantee. These software results do not establish production readiness.
