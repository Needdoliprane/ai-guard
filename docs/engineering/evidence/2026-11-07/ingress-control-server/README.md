# Lot H — server control transport, 7 November 2026

Status: **software server checkpoint; lot H remains partial**. Base `982d215`.
The maintainer approved the direction and local code; no cloud operation is
included. The [transport contract](../../../INGRESS_CONTROL_TRANSPORT.md) describes
the actual listener, routing, authorization, limits and closed failure DTO.

## Scope and result

The ingress sends `/control/` requests to one fixed internal gateway origin,
without using its public upload/exchange permits. It owns no credential and
does not select a tenant. Gateway Rust authenticates the route's existing runner
or broker identity before reading its body or taking the tenant's reserved
capacity. The handlers retain their existing authorization and durable receipts.

The immutable H05 gel passes **14 functions**: two real router/SQLite supervision
tests, nine wire tests, two ingress CLI tests and one main option test. Four
gateway unit tests passed on H03; their application files are unchanged in H05.
Those four are separate evidence, not a repeated run on the final gel. The
earlier seven wire functions are not added again to these counts.

The supervision tests retain 16 public uploads and 64 public exchanges, then
require an authorized transition and claim with real durable receipts, exact
replay and verification after reopening. A valid worker relay proof must first
succeed on the public inbox, then fail on the control routes without a body poll
or audit mutation. Runtime observations used to prepare workers are synthetic.
This proves software routing and authorization, not container confinement or
resistance to unlimited TCP connections or FD pressure.

## Commands and observed results

Luna ran the commands below inside the admitted, network-isolated QA from
immutable sources. Cargo used `--offline --locked`, one build job, disabled
incremental compilation and debug information, and a target directory unique
to each gel. `HOME`, temporary directories and Cargo cache were scratch-local;
the public dependency registry and sources were readonly. No actual credential,
provider model or container engine was available to the target.

| Gel | Selection, in addition to `cargo test --offline --locked` | Observed result |
| --- | --- | --- |
| H03 | `--lib api::control_transport::tests:: -- --test-threads=1` | 4 PASS, 2.86 s; build 1 min 50 s |
| H05 | `--test supervision_api control_transport:: -- --test-threads=1 --nocapture` | 2 PASS, 0.96 s; build 1 min 47 s |
| H05 | `--test ingress_control_transport -- --test-threads=1` | 9 PASS, 0.01 s |
| H05 | `--test ingress_cli -- --test-threads=1` | 2 PASS, 5.08 s |
| H05 | `--bin agentsandbox tests::ingress_requires_an_explicit_control_upstream -- --exact --nocapture` | 1 PASS, 21 filtered |

H05 `cargo check --offline --locked --all-targets`: **RC0**, 1 min 03 s;
119 library warnings, two SSO trust-test warnings and three library-test warnings.
Clippy is **NOT_RUN** for this checkpoint; compilation warnings are not waived.

QA Y: Linux ARM64, pinned Rust 1.99.1, image
`sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`.
UID/GID 1000, no capabilities, no-new-privileges, seccomp, private namespaces,
network `none`, no published port or engine socket, readonly root/toolchain and
inputs. Limits: 4 CPUs, 8 GiB RAM, zero swap, 256 PIDs, 4 GiB scratch/65,536
inodes, 512 MiB maximum file, 1,024 FDs, no core dump, strict one-hour lifetime.
Two other concurrent library-test compilations were OOM-killed in the same QA;
they are not product REDs or H successes. H05 completed without a further OOM
counter increase. Subsequent large compilations were staggered.

## Reproduction and intermediate failures

- H01: two residual witnesses pass because the public budget refuses the dummy
  control probe before the gateway fixture. This is reproduction of the defect,
  not successful remediation. The dummy token proves no authorized mutation.
- H02: seven relay tests pass. They do not prove gateway identity admission.
- H03: four API units pass, then the supervision target fails to compile because
  the test called a private identity API. Its functional scenarios are NOT_RUN.
- H04: the corrected public probe compiles but two fixture preparations fail:
  duplicate `intent_hash` returns 409; a populated legacy origin is deliberately
  not reactivated by migration and its worker proof returns 403.
- H05 fixes only those fixtures: distinct intent and an active current origin
  before worker creation, using real Store APIs for arm/claim/observation. No
  server authorization, expected status, body-poll oracle or timeout is relaxed.
- The first main filter selects zero tests; it is not counted PASS. The exact
  fully qualified test above is then run successfully on the same gel/cache.

The historical residual source is retained privately, SHA256
`005e95ebb0a4747625d49631929879aa149ab52566aec88e361a0e6f1e0daa34`.
It is excluded from shipped tests, which retain positive regressions instead.
The [source manifest](source.sha256) covers the final application/test/config
files and Cargo inputs. H05 also contained that historical residual file;
its removal is the only test-tree difference after the validation gel.

## Parent log fingerprints

Raw stdout/stderr are retained privately by the orchestrator. SHA256:

| Parent log | SHA256 |
| --- | --- |
| H03 API units | `9367e33c9c6cece90c8a39db8a34c23f87282aaf9c27d5e67a05e6c8eb33e726` |
| H03 private-API compile failure | `89b651e9a185460ab5208ad04a9fe1bedf1198cb7d6e38a2aad6cf44c2cb3d64` |
| H04 fixture failures | `2c1138d5ef504abae701a0c0131dce327d8d4cc77f726c1c8c69b244f00317d9` |
| H05 supervision | `dca2eb277381235f03c7706460922617c8757f8898013d47fa3eb1ca78121678` |
| H05 wire | `f61c41cb9e6a0c187cdd470cf61ecc8e3963abd1dfe5f97579522a34f7e3d68c` |
| H05 ingress CLI | `7d5806c0f97f61be8d3460fbf6025548f5ea6efbcfdabed5edd7c69db1504bf8` |
| H05 first main, zero selected | `7a6e8f3dd98e956fc540bcc49264c36a18fe44ce3d84e33c8672c591e1af94e0` |
| H05 exact main | `b70bba03115ed5588a5ea61b7850609dbe6ea28dcf6b58e8415363eb9b40782b` |
| H05 all-target check | `af15e2c9d46de8121a159270b2cbb073aebda1af281721127369536574847088` |

## Review and open acceptance

The H03 server/identity/listener/config code and the H04→H05 fixture delta have
independent static reviews. The final independent review also accepts this
evidence and the EN/FR guides within the server checkpoint: 21 source hashes,
nine log fingerprints, the 14 H05 results, options and six Compose files were
checked against the sources. No blocking defect was reported in that scope;
this does not qualify the open acceptance items below.

Client SDK routing, mixed broker outbox provenance, FD/TCP admission and costly
cross-tenant identity scans remain open. An event's asserted `kind` or `source`
cannot choose the trusted channel. The 60-second monitor direction does not
authorize retrying a start, claim or mandatory audit callback. Docker/Podman
Compose execution, real workers, Kubernetes, host/platform matrices, provider
CLIs and cloud transport qualification are **NOT_RUN** by this checkpoint.
G09 remains FAIL; no global production gate is closed.
