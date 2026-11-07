# Managed recovery stack — Core25, 7 November 2026

Status: **TARGETED SOFTWARE VALIDATION PASSED**. Source candidate based on
`a329153fb577eb689bea2291c3c0f1ad8eb195b3`; composition and publication remain
pending. This record covers the Core21 application change and Core25 adoption
harness together. It does not transfer earlier component results to the
integration branch or close a production gate.

The two Rust files and unchanged Cargo files are identified in
[source.sha256](source.sha256), matching the four-entry Core25 QA manifest.
Application source `managed.rs` remains
`ade65ac394c4e388ed7b2669d87abd8cecbda166eae194a205161c3ce21a624f`;
the monitor source, including the corrected test, is
`4ddc57f9317d0ff89c2c194f0a988253c3afd4c3de9464988e135711270e5374`.

## Application change and diagnosis

The authenticated recovery path is separated into six directly awaited phases:
authentication, already-cleaned terminal recovery, runtime binding, preparation,
owned-instance recovery and observation. A synchronous `#[inline(never)]`
factory constructs and boxes each phase outside the caller's poll frame. This
change adds no task spawn, OS thread, stack-size override or new execution
capability. The local STOP precheck still precedes authenticated recovery.
Existing guards, identity checks, credential ownership, durable transitions,
timeouts and consumed start claims are preserved. The private checkpoint reader
retains its bounded bytes and `serde_json::from_slice` path.

Future storage size alone did not explain the failure. An earlier diagnostic
measured 68,976 bytes for managed resume and 69,200 bytes for the public wrapper
(about 69 kB; output SHA-256
`2aaafe347bfc8f507ff08b1aa77bfa450d67b3248baa530e036f8ce2253ddd59`).
Diagnostic20 instead measured a 2,097,152-byte thread stack. Between resume entry
and the authenticated path's pre-lock probe, approximate remaining stack fell
from 1,888,263 to 691,319 bytes: about 1.2 million bytes, or 1.14 MiB, of additional
stack use around polling. A later checkpoint-decoder probe had only 194,023 bytes
remaining before the reported stack overflow. These probes localize cumulative
stack pressure; they are not an exact ELF frame-size measurement or a general
stack bound. The old failures reported SIGABRT/stack overflow, not OOM; the
relevant QA Z final inspection recorded `OOMKilled=false`.

## Preserved development history

The outputs below are private parent-captured logs, read and hash-verified by
Root. Raw logs and diagnostic artifacts are not published in this directory.
These development and harness failures neither add nor close external audit
findings.

| Source / check | Observed result | Output SHA-256 |
|---|---|---|
| Core18, exact local-STOP parent | PASS, 1/1, 8.10 s; build 1 min 53 s. This did not exercise authenticated adoption. | `a119c9e8d962e6092d414a2242678c5b4205373dd0eda38c676c38c9b75a8468` |
| Composition19, monitor module | First local-STOP parent PASS; adoption aborted with stack overflow/SIGABRT before its functional oracle. Module FAIL. | `2af3f1bb692a823fb8a56e75b9ce9937b938c698bfe495f414fbae25cc28e70e` |
| Diagnostic20, adoption parent | FAIL, same SIGABRT after the stack probes described above. No adoption oracle reached. | `66981a61af861e7adaa6a01d9c4924e709d6b8a2c7883833d7059a163f46cbc5` |
| Core21, unchanged adoption oracle | FAIL, RC101, 15.97 s; build 1 min 58 s. No stack abort; create/start count was 2 instead of 0. | `400c1bfb9f01c08b673ee8822196c02b9038a25caee67429101f5fa87543565c` |
| Diagnostic22, same zero-count oracle | FAIL, RC101, 16.63 s; build 1 min 54 s. Bounded records showed one Export create and start for the same separate helper; all four scan events and diff completion appeared once. | `728628603ac3f4abe9704da8ff12c7aaf56080aee07f871c429a3a71145cf19e` |
| Core24, strict helper/state/journal oracle | FAIL, RC101, 16.81 s; build 1 min 49 s. All preceding state, audit and journal assertions passed; only the final expectation of successful resume failed. | `bad5ebb27444792729bd7d5f3203a4c914ea88603463500aa38446298013f4b7` |

Core22 confirmed one Export helper create/start pair during trusted diff
recovery; it did not establish a business worker restart or independently
measure its order relative to STOP. The replacement oracle parses every create/start/kill record
with closed syntax. It permits exactly one Export create/start pair for the same
64-hex ID distinct from the original Run ID, rejects Run/Import or any other
purpose, duplicate or malformed invocation, and permits exactly one kill of the
original ID with no other kill. Independent review added the explicit
STOP-before-Export-create assertion in Core24; start must follow that create.

The four scan events must each occur once, in order and in the original session;
the created event must identify that Export helper and a non-nil operation UUID.
One diff completion must report zero changed files and
`post_completion_recovery`. The valid retained journal must acknowledge that
same diff event and its coverage event, preserve unknown declared-delivery
counts and explicitly retain `UnavailableAfterInterruption`. Worker, attempt,
container and start-operation identities remain unchanged; the worker is
terminal, quota released, cleanup durably acknowledged and both fixture
instances removed. Exactly one adoption event and the exact candidate binding
are required. No `RunSummary` is fabricated: the fixture's missing result remains
missing.

The final Core24 assertion was inconsistent with the
[existing file-audit recovery contract](../file-audit-recovery/README.md): a
legacy capture error remains a refusal even when trusted diff and coverage are
acknowledged. Core25 changes only the harness to require the declared error
`legacy declared capture is unavailable; delivery counts remain unknown` and
the exact returned error
`postprocessing recovery incomplete: legacy declared capture is unavailable; delivery counts remain unknown`.
Timeout, successful resume and every other error fail. The passing Core25 parent
therefore observes that exact refusal together with all prior safety oracles;
application behavior and the preceding assertions were not weakened.

## Core25 validation

The recorded commands below ran against the read-only `/inputs/core-stack-fix25`
source in the admitted QA, with the common target
`/scratch/target-core-stack-fix25-aa`:

```text
cargo test --manifest-path /inputs/core-stack-fix25/Cargo.toml --lib runner::managed::control_monitor::tests::authenticated_resume_can_adopt_valid_replacement_before_stopping_same_instance -- --nocapture --test-threads=1
cargo test --manifest-path /inputs/core-stack-fix25/Cargo.toml --lib runner::managed::control_monitor::tests:: -- --nocapture --test-threads=1
cargo check --manifest-path /inputs/core-stack-fix25/Cargo.toml --all-targets --locked --offline
```

Rust 1.99.1 Linux ARM64, Cargo offline, jobs 1, incremental and debug information
disabled, one test thread. The safe JSON command/settings headers in the Core25
logs record these commands and target. The environment is allowlisted with
scratch-local home/tmp/cache; no `RUST_MIN_STACK`, provider credential or external
network. Root verified the effective QA controls: read-only root/source/toolchain/
registry, network `none`, 4 CPU, 8 GiB RAM without swap, 256 PIDs, 4 GiB scratch
with 65,536 inodes, 512 MiB per file and 1,024 FDs. A fixed host-side wall-clock
deadline bounds the QA. No host target execution was used.

| Check | Result | Output SHA-256 |
|---|---|---|
| Exact adoption parent | PASS, 1/1, 17.09 s; build 1 min 52 s | `bfcdb9b3b0fc1f99b2f059e58a33deba4db1513e08f21db5d970c9de29918503` |
| Monitor module, including that parent | PASS, 22/22, 39.43 s; incremental build 0.69 s | `9eee08a4412e716c7ab5a3c56d2a6ad0b9abfee2758944e1c8c972fca2291102` |
| All-target compilation | PASS, RC0, 1 min 04 s | `c57df2852481c779b3185c845ac7ca7a6fad7fb43699476c1aba89d7c3d17d09` |
| Independent Core21 application / Core25 harness source review | ACCEPTED before QA | No reviewer test results transferred |
| Rustfmt and source diff checks | PASS before the Core25 freeze | Trusted native tools; no target execution |

There are **22 unique passing functions**, not 23: the separately selected
parent is included in the module run. The check reports 119 library warnings,
2 `human_sso_gateway_trust` warnings and 3 library-test warnings. Clippy remains
open. These timings are observations in this debug QA, not a controlled speedup,
memory benchmark or performance qualification.

## Limits and pending work

The tests use real loopback HTTP, SQLite, private journals and the actual resume
path with an explicitly selected native Rust engine fixture. Runtime process
state is declared by that fixture. They do not qualify Docker, Podman,
Kubernetes, physical crashes/storage failures, a provider conversation or an
independent lifetime guardian. A stop request alone remains distinct from
observed termination.

The integration branch must compose and validate its own frozen source. No
result is transferred to the separate H SDK, M authentication-budget or K
inter-store work. Independent evidence review is accepted. Composition and
publication are tracked separately by the audit register. The earlier
[monitor record](../control-monitor/README.md) retains its
own source/date scope. G09 remains FAIL and no
[production gate](../../../PRODUCTION_READINESS.md) is closed by Core25.
