# Bounded Rust worker relay client — 7 November 2026

Software candidate `relay-client-fix02`, based on
`238fddd9fcb79b192a5c6a503ba344044f4f83d0` in the dedicated relay worktree.
The [six-entry source manifest](source.sha256) matches the tested 02 Cargo,
application and test inputs. Guides [EN](../../../../setup/relay/README.md)/
[FR](../../../../setup/relay/README.fr.md) and the
[runtime handoff proposal](../../../RELAY_RUNTIME_HANDOFF.md) were finalized
afterward without changing those six inputs. These results do not transfer to
another checkout or qualify the main integration merely by sharing a base.

## Delivered scope

`WorkerRelayClient` calls only the existing authenticated worker send, inbox and
ACK routes. Its caller supplies an approved fixed origin, a dedicated workload
proof and expected endpoint. It neither provisions that proof nor obtains a
runner/human role. No operation ID, retry, redirect, proxy, runtime network or
new service is introduced. `with_ca_file` reuses the existing explicit public
trust loader; this lot contains no new real TLS/private-CA client test.

The client checks the full outgoing DTO for its known proof before transport,
including an ACK ID before URL construction. Responses use the existing shared
`json::decode_unique` helper, then typed decoding and equality with the complete
serialized V1 shape. A missing nullable cursor is refused. Request/response
bytes, JSON depth and exchange times are bounded; errors retain no remote body
or credential-bearing error chain. Lost responses leave mutations unknown and
require an explicit replay of the same retained operation.

The structured-body check also refuses JSON parseable by the ordinary decoder
but rejected for duplicate keys or depth past 64, even without a secret.
Non-JSON text remains accepted. The known-proof literal/structured checks are
not general secret detection or perfect DLP. Mandatory audit, current workload
authority, directed grants, revocation, quotas and tenant storage remain server
checks, not guarantees supplied by the client alone.

## Reproduction before correction

Independent review found that a valid canonical-UUID workload proof could pass
as a send or ACK identifier. The initial client checked only the send body.
The four regression cases each observed one HTTP request instead of zero;
send/ACK operation-ID cases also changed durable state. The proof was genuinely
bound and used for a successful authenticated inbox read before each oracle.
No proof or response identifier was printed in the failure diagnostics.

The same review found duplicate keys erased by Value-first parsing and a missing
`next_after` accepted as `None`. Four wire tests reached those acceptance defects
after real authentication/Store operations: duplicate receipt, duplicate inbox,
duplicate nested endpoint key and omitted nullable cursor. The separate
ACK 200-after-commit response-loss test already passed on the baseline and was
retained unchanged.

Frozen `relay-client-residual01` compiled and ran the same 13 client tests:
**5 PASS, 8 expected FAIL**, 7.78 s. Its client source is
`d32913e8af6134669f05c90914d6a8750345063ab07c15b4a58832dab0786b09`;
the other five manifest entries, including every test oracle, match 02.
Parent-captured output SHA-256:
`2b071864d48243a10dd410efcdcc38da94fdb6eb964fd742db8c9ab7361a7e45`.
This behavioral RED is retained; it is not counted as a successful validation.

## Corrected input and validation

Luna ran the following selections with locked/offline Cargo, an explicit frozen
manifest and a source-specific target. The first selection ran on both 01 and 02;
the remaining two ran on 02:

```text
cargo test --offline --locked --test worker_relay_api client_cases::
cargo test --offline --locked --test worker_relay_api -- --skip client_cases:: --test-threads=1
cargo check --offline --locked --all-targets
```

| Frozen input and check | Observed result | Parent output SHA-256 |
|---|---|---|
| 02, client cases | PASS 13/13, 8.04 s; 18 API cases filtered out | `9ab08c6264171643b7195ca726dc782364e37d3217c9c2343cf2581b3dc69a33` |
| 02, existing relay API cases excluding clients | PASS 18/18, 16.24 s; 13 client cases filtered out | `c763f06ad6bd5135e91ad22c1a9b2f862ae3c0f2d26b74087190a589c90ef96e` |
| 02, all-target compilation | PASS RC0, 1 min 07 s | `9e5555d0e0f748c3fc97ee13137e9fd09d4a3c748a2625fb2153591c0fd7f8dc` |

Thirty-one unique test functions passed. Client cases cover real bound-worker
HTTP authentication, A1→A2 grant use, foreign-worker/cross-tenant refusal,
revocation, send/ACK replay and divergent send bodies, lost send and ACK responses after
durable commits, local credential-ID refusal before transport, malformed and
oversized wire data, redirects not followed, inbox digest checking and complete
unique JSON. Existing API tests retain server-side authority/budget/clock/
expiry/revocation/reopen oracles; they are not thirteen additional client tests.

The independent Relay client source review accepted the correction, with the structured
JSON restriction above and the absence of a general DLP claim documented.
Compilation reports 119 library, two SSO-test and three library-test warnings.
Clippy and production-readiness gates are not closed by these results.

## Environment and retained material

QA AB used Rust 1.99.1 Linux ARM64 and pinned image
`sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`.
Root verified actual admission, not only requested configuration: UID/GID 1000,
capabilities 0, no-new-privileges, seccomp mode 2, network `none` with local loopback,
read-only root/source/toolchain/public registry, no host socket or port,
4 CPU, 8 GiB RAM without swap, 256 PIDs, 4 GiB scratch/65536 inodes,
512 MiB per file, 1024 FDs and core 0. Cargo concurrency was 1, debug information
and incremental compilation disabled, under an empty allowlisted environment.
A host deadline and earlier preventive watcher bounded the owner lifetime.
The retained inspection reports `OOMKilled:false`; this is not a worker memory
qualification.

At documentation freeze, Root retained and read the four parent-captured logs
under the private validation directory
`/private/tmp/ai-guard-cold-drain-logs-20261106.nXx9XJ/`:
`relay-client-residual01-ab.log` and
`relay-client-fix02-{regression,api,check}-ab.log`.
Their hashes above were rechecked during documentation. The private
`qa-ab-inspect.json` is beside them. Frozen inputs and their six-entry sibling
manifests are under
`/private/tmp/ai-guard-audit-followup-inputs-20261106.xu8TFf/inputs/`, named
`relay-client-residual01` and `relay-client-fix02`. These temporary validation
locations are not a product backup/archive or a public artifact delivery.
No database, executable, workload credential or generated runtime payload is
included in this evidence directory.

## Limits

HTTP loopback, SQLite and authenticated workload bindings are real. Fixtures
prepare synthetic supervised-worker records; their Running/container fields are
not independent runtime observations. Hosting QA in Docker does not qualify an
ai-guard Docker worker. The 31 tests do not implement or qualify a three-worker
runtime recipe, a shared Unix channel, Docker Desktop/macOS handoff, Podman,
Kubernetes, enterprise CA/mTLS, multi-host transport, provider clients, physical
crash durability or the 100-worker target.

The client has no durable request journal. Callers must supply retention and
reconciliation; replay remains subject to current authority. Workers remain
`network:none` without a route to this HTTP client. The separate
[handoff proposal](../../../RELAY_RUNTIME_HANDOFF.md) requires review before
implementation. No global gate is closed; G09 remains FAIL.
