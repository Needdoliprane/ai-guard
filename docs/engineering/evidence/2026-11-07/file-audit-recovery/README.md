# File audit recovery — corrective lot I

Status: **TARGETED SOFTWARE VALIDATION PASSED**, 7 November 2026. Integration and
runtime qualification remain pending. Base `5e52807`; candidate
`audit-i-fix11` contains the five Rust files listed in [source.sha256](source.sha256),
with unchanged Cargo files. The six product documents are listed in the
same manifest; they are not inputs to the frozen Rust test source.

## Behavior and contracts

The trusted workspace scan and retained `file.*`/diff prefix precede provider
capture ingestion. The private, bounded postprocessing journal retains exact
redacted events and acknowledged cursors for retry. It is separate from the
closed RunPlan formats 1/2/3. Pending is synced before the existing result and
terminal processing; those two files are deliberately not claimed atomic.

`POST /api/v1/control/events` authenticates the runner before body parsing and
admits a closed list of trusted facts through a separate frequency window.
Declared capture remains on ingestion. The route, rather than `kind` or worker
assertions, selects the lane. Durable audit capacity remains a separate open
concern in corrective lot M.

Typed ingestion/control 429 retry retains exact bytes and event identity, with
at most three fifteen-second requests and two bounded pauses. Other refusals
do not fall back to the old gateway. The mandatory `command.started` callback
explicitly uses one request; the separately approved monitor window is not
implemented by this lot.

Recovery handles missing results and legacy results without a diff. Lost
capture remains unknown, with an explicit partial-coverage error. An external
monitor diagnostic is preserved through a durable summary-projection intent.
Redacted display collisions retain distinct random event UUIDs as `change_id`,
with redaction/ambiguity flags for the retained prefix. No raw sensitive name or
hash of that name is added. `file_delivery` version 1 distinguishes prepared
changes and durably acknowledged changes; a lost ACK is not an acknowledged
delivery even if the gateway committed it.

## Reproductions before the relevant corrections

All command outputs are parent-captured private logs; hashes below identify
those outputs. A PASS for a `residual_*` witness means the defect was present.
Primary external audit artifacts remain unchanged.

| Source / check | Observed result | Output SHA-256 |
|---|---|---|
| I02, declared refusal followed by resume | PASS, defect reproduced after correcting the initial harness's incorrect success expectation | `bba1befdf0ed40a4c23a4f69e8d440f0ee27d14b83facb128f14c3fec0c8fab1` |
| API shared-window witness, both ingestion aliases | PASS, defect reproduced | `47eef78ee6503dd56dd7c73989bc69d22d1d56943e86076d010a040bfe54634f` |
| I04, valid escaped path, pre-Pending loss, external diagnostic | Three PASS witnesses, defects reproduced | `638f3f2d8f0f92f94c74c5801b26d8ae8b29209ca6545adb3d05374a1a32db41` |
| I05, start callback retries while runtime fixture is running | PASS, defect reproduced | `a4c047aba3d1b804e8744f5e424301fe38bf97fb7aaf0c60d30ec3a46d5dd525` |
| I07, legacy result without diff or journal | PASS, defect reproduced | `8d2458567bf126d90fe0509197c76c79401a7592a7065975b4fda3763537ab4a` |
| I08, colliding redacted paths and positive neighbor control | PASS, defect reproduced | `f7cb8e2fde195793146fef93de16f44927a3d4a7cfa353b3c4ddb67901e73f19` |

The initial I01 resume witness failed before its final oracle because it wrongly
expected CLI success. I02 corrected that assertion without changing application
behavior. I03 later passed six file parents and failed one process-cut harness:
its first terminal-observation barrier could occur before the intended Pending
write. I05 corrected only the fixture/barrier, required Pending plus the result
and output-event digest, then passed the same retained-event recovery case.
These harness failures are not erased or counted as security reproductions.

The legacy I07 model renames and retains its journal to model an old checkpoint
that predates this journal; it does not demonstrate physical loss of an fsynced
file. Private journal/projection injections are controlled state models, not
claims of OS crash or storage-failure qualification.

## Candidate validation

Current candidate commands, run only inside the admitted network-isolated QA
with read-only source/toolchain/registry and scratch-local allowlisted environment:

```text
cargo test --offline --locked --test runner_file_audit file_prefix_respects_global_journal_bytes_and_reports_omissions -- --exact --nocapture --test-threads=1
cargo test --offline --locked --test runner_file_audit -- --skip file_prefix_respects_global_journal_bytes_and_reports_omissions --test-threads=1
cargo test --offline --locked --lib runner::event_transport_tests:: -- --test-threads=1
cargo test --offline --locked --lib runner::managed::tests::postprocessing_payload_without_projection_keeps_its_legacy_bytes -- --exact --test-threads=1
cargo check --offline --locked --all-targets
```

Each command also has an explicit manifest path and the same candidate-specific
`CARGO_TARGET_DIR`. Rust 1.99.1 Linux ARM64 is pinned; Cargo is offline, jobs 1,
incremental disabled, debug information disabled. No `RUST_MIN_STACK` is used.
QA X limits are 4 CPU, 8 GiB RAM without swap, 256 PIDs, 4 GiB scratch with
65,536 inodes, 512 MiB per file and 1,024 FDs. Root verified effective controls;
the QA deadline is fixed independently of target code. No host target execution.

Twenty unique functions PASS on I11; the byte-prefix parent is not repeated in
the fourteen-parent command. Outputs are parent-captured private logs.

| Candidate check | Status | Output SHA-256 |
|---|---|---|
| Byte-prefix parent | PASS, 1/1, 151.12 s | `ea5f063bb776da03caaf5071395175662327f24835f0d312550ad751c7fcecfb` |
| Other file-audit parents | PASS, 14/14, 340.34 s | `99ed9ca46d9cd0413ce974be42a8a93ae220bffcb20b27fb4cf43171e382c1b3` |
| Event-transport units | PASS, 4/4, 4.15 s | `6d4a8e8d925efd43bba19f70b826a474ab38b58d89c518ed456199ba192ad251` |
| Legacy journal-byte unit | PASS, 1/1 | `f70fac071752341ff858ed028036a6b8e53cb6f31c7fcff56df0dddd5c5f148a` |
| All-target compilation | PASS, RC0, 1 min 02 s | `ce54145ef7676b13c43b9e3b4bbd4619adca65b80edd19329541a5f84cb78f4a` |
| Independent source/document review | ACCEPTED; final reader/harness delta also reviewed | No reviewer test results transferred |
| Rustfmt and diff checks | PASS before source freeze | Trusted native tools |
| Composition with export J / monitor L | NOT_RUN | Pending integration |

The check reports 118 library, 2 SSO-test and 3 library-test warnings. Clippy
remains open. The sandbox reported no OOM. No provider call or runtime matrix was
run for this change.

## Preserved timeouts and bounded-reader correction

I09 compiled in 1 min 46 s with 118 library warnings. Its fifteen-parent run
ended RC101 after 428.74 s; output SHA-256 is
`dd6b376e53cc4989ffca2afd4c10fe3e0a6fdb4d305c0a93e387d624384d91df`.
The failure is `bounded CLI observation expired` at the harness's 90 s output wait,
before the byte-budget parent's functional assertions. It is not attributed to
machine pressure or to a particular product phase without further evidence.
The source manifest for this run is retained as [source-09.sha256](source-09.sha256).

A test-only [I10 diagnostic](source-10.sha256) kept the same data/oracles and observed at most
240 s. It also timed out: output
`194ad88830addb1da7f604340ab9f8907d80574e0c44b411ab5ca32ebff8d675`.
Snapshots showed continued file delivery: 119 prepared entries, 78 durable ACKs
and 79 distinct POST attempts at 210 s, without diff/coverage acknowledgement.
That is progress, not a demonstrated functional deadlock. Snapshots are not
atomic; the initial harness could also conflate worker-read failure with no
worker and did not explicitly join its diagnostic. I11 corrects both points.

Source inspection found that the private reader passed a `File` directly to
`serde_json::from_reader`. The locally locked serde_json 1.0.154 explicitly uses
unbuffered `reader.bytes()` for that path. I11 reads at most `maximum + 1` bytes
from the same validated FD into a bounded buffer, refuses oversize before
`from_slice`, and retains all identity checks, revalidations, hashes and durable
writes. No cache or new dependency is introduced.

I11's unchanged functional byte-prefix assertions pass with 119/119 ACKs,
diff/coverage acknowledgement and a 3,015,772-byte journal. Its 190 long paths,
4,096-byte path limit, 3 MiB journal bound and 240 s observation remain unchanged,
as do product deadlines. The fourteen other parents were rerun once because the
shared reader changed. Their original 90 s observations were preserved.

Earlier W used 2 CPU/3 GiB scratch; X used 4 CPU/4 GiB scratch. These timings are
not a controlled speedup measurement or a performance qualification. Repeated
whole-batch validation/serialization remains a potential cost; capacity 100,
release-mode performance and other environments still need their own evidence.

## Limits

Tests use the actual CLI, Rust HTTP/SQLite paths and explicit Rust engine
fixtures. They establish neither Docker/Podman/Kubernetes confinement nor a
provider conversation. No real runtime, model, financial billing, physical crash,
rollback resistance or recovery from journal deletion is qualified. Broker
outbox replay is a separate path; this direct-CLI journal does not migrate it.
The control ingress, inter-store coupling, durable capacity and broker fencing
remain open. No production gate or external audit verdict is closed here.
