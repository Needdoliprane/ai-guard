# Core12, broker checkpoints and Kubernetes preparation

Software evidence from 9 November 2026. These components do not qualify a
multi-host deployment, Pod execution, worker confinement or the product release.
G09 remains FAIL and none of the twelve global gates is closed.

## Sources and execution boundary

The integration base is `696efd18ed21f78df7429a20d8e16dbcac1c00d9`.
The tested source is a frozen dirty composition, not that commit alone:

| Source | Full SHA-256 manifest | Files | Scope |
|---|---|---:|---|
| Core12 baseline | `25daca4667f3f01171f1896af9448b85605c03487575359af9e835fb783c1397` | 2328 | Existing11→12 dispatcher plus the migration regression, before implementation |
| Core12 corrected | `53f5f40566ea2f2e7c3eb61d1936528da26e5f37986e3bdce5aeb6b4c883d351` | 2330 | Migration, schema guards and local S1/S2 reservation confirmations |
| HTTP metadata baseline r3 | `d7957c3623db6baf785a6575c86da5702819b6320edd1f89a3b652e7f5f4c3ac` | 2349 | Composed components plus two confidentiality regressions, before handler fix |
| Composed corrected r2 | `18dde1ad287b9a9815504e422fd083928138028a59afdc441cbf4d2c19b33db4` | 2349 | Core12/CLI/guards, checkpoint Store/HTTP/publisher and Kubernetes journal/bootstrap |

The latter composition excludes D5/cache, the new service checkpoint-source
adapter and subsequent migration-negative tests. No results transfer to those
later sources. `Cargo.lock` is unchanged, SHA-256
`3f34d8504293aa18ca1060b1c9d184904d3ce00fcc9c76c6f711f2603b1be4da`.

Execution used the already available pinned QA image
`sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`,
Rust/Cargo1.99.1 on Linux ARM64, with no external network. The UID1000 process
had no capabilities, no-new-privileges and seccomp enabled; root/source/toolchain
were read-only, scratch4GiB/65536inodes, CPU4/RAM8GiB/swap0/PIDs256, file512MiB,
core0 and descriptors1024. No engine socket, real provider or cluster was exposed.
The allowlisted empty environment and private public-dependency cache were
verified before Cargo; builds used a verified private source copy inside
scratch and were frozen/offline. An initial missing public
crate copy failed during setup and was corrected without a download or test.

## Reproductions and correction

- **Expected FAIL (RED):** the Core12 regression selected one
  test and failed at the unsupported migration only after verifying a real
  schema11 source, exact reopened backup, held quota and unchanged source.
  It returned RC101; this is not a successful migration.
- **Expected FAIL (RED):** the two HTTP regressions each selected
  one test and returned RC101. POST accepted a synthetic current proof in a
  valid hash field (200 instead of400); GET returned retained data containing
  the new current proof after a real fresh binding/incarnation/grant. The GET
  oracle is the named response-privacy assertion, not a separately captured
  HTTP status. No real credential or response body is published here.
- Earlier HTTP harness attempts are **nonconclusive**, not RED: one did not
  compile because it used private interfaces; another reused an incarnation
  and stopped at the grant conflict. Its repeated-a fixture also matched an
  existing static runner proof, so its initial400 did not discriminate the
  missing current-proof check. The corrected public fixtures preserve these
  failures rather than relabeling them.

The migration writes a strict unique schema12 marker through the existing
verified-backup transaction. It preserves records, tenant/namespace, quotas and
inactive reopening. Local confirmation objects are constructed only after
actual scoped Observe/commit; they contain reservation evidence and grant no
CREATE, start or release authority.

The two checkpoint handlers check the actual request redactor before persistence
(input, authenticated scope and commitment), then check the full POST/GET
envelope. They never replace a hash or identifier with masked content. An unsafe
response is withheld with a generic error; that does not imply rollback of a
committed operation. The [EN](../../../../setup/broker/CHECKPOINTS.md) and
[FR](../../../../setup/broker/CHECKPOINTS.fr.md) procedures retain that distinction.

## Targeted results

On the corrected Core12 source, **5 unique functions PASS/RC0**: the backed-up
migration, two S1 confirmation cases, quota/confirmation after12 and the S2
scope/policy/origin/deadline case extended with its actual confirmation wrapper.
`cargo check --frozen --offline --all-targets` returned RC0 in34.66s, with85 lib,
2SSO-test and10lib-test warnings (5duplicates). This is not a Clippy result.

On the corrected composed r2 source, the following commands all returned RC0.
Test commands used `--frozen --offline`, `--nocapture --test-threads=1`, and the
CLI parent additionally used `--exact`:

| Cargo selection | Selected/PASS |
|---|---:|
| `--test broker_identity_api checkpoint_http_` | 4/4 |
| `--lib broker_registry::checkpoint_tests::` (domain and Store) | 6/6 |
| `--lib broker::presence::checkpoint_storage_tests::` | 4/4 |
| `--lib broker::presence::http_tests::checkpoint_http_tests::` | 3/3 |
| `--lib broker::presence::http_tests::run_owner_tests::` | 3/3 |
| `--lib runtime::kubernetes::session::tests::` | 6/6 |
| `--lib runtime::kubernetes::bootstrap::tests::` (including absolute deadlines) | 8/8 |
| `--lib schema11_compatibility_tests` | 4/4 |
| `--bin agentsandbox tests::migration_cli_accepts_12_and_preserves_default_and_upper_bound` | 1/1 |

These are **39 unique functions**, not ignored cases or compiler invocations.
The final `cargo check --frozen --offline --all-targets` returned RC0 in40.07s:
125lib,2SSO-test and11lib-test warnings (4duplicates). Clippy, release and a
full software matrix were **NOT_RUN** in this batch; warnings remain open.

The tests exercise A/B identity and role refusals, exact replay and revoked
replay, body limits, mandatory audit rollback, private Pending/restart/ACK,
retained owners and real FS joins, actual central reservation-backed local
preparation, bounded duplex handshakes and absolute-deadline refusal. Duplex
peer assertions remain declared; no peer workload identity is established.

## Reviews, retained proofs and limits

Independent source reviews: Core12 `95f763a7ebfd4491d357d652a42f44a511aec693ffdbdf56f140d23caffa5ded`;
federation owner correction `e7e07446ec582edf19b5456f00733a0a9a83cba7efee17850e28bb038f9d962a`;
HTTP fix `60dc5561c970f447cf195a8efb98e4b1f0693d6572c12a222d45c389300d08a0`.
The Kubernetes absolute-deadline review found a late-ready acceptance in its
initial source. Its correction and two discriminating deadline parents are
included in the eight observed bootstrap passes above.

Minimum private text/JSON proofs, raw compiler/test output, full manifests and
admissions are retained under the coordinator's engineering directory:
`qa-parallel-20261109-r1` and `qa-fed-kube-compose-20261109-r1`. The latter QA
stopped at21:01:35.753UTC, PID0/OOMfalse; its watcher observed that terminal
state at21:02:01UTC and the exact container was removed. No volume or unrelated
resource was deleted. Worktrees remain until integration, remote verification
and private/ignored/consumer checks are complete.

The first D5+STATUS+HEADS composition is a separate source,
`172b1d22be52e0628d70049baa019c27720f3570ce2557207d7e0995a2bd2a22`
(2407files). Its first test invocation failed to compile at `writer.rs:1605`
because an accepted job was not Clone. No STATUS/HEADS test was selected; their
26 planned functions and that composition's check are **NOT_RUN**. A borrowed
metadata projection correction is under review, without cloning the capability.
Full3 remains FAIL on its prior frozen source, not on this compilation attempt.

Still open: service source adapter and retained service-level publisher owner,
authenticated remote reservation confirmation, Kubernetes orchestration/PVC/
workload identity/admission/lifecycle, broker prewire retention and fencing,
physical multi-host transport, network policy, migration edge validation,
Clippy/release and real Docker/Podman/Kubernetes qualification. No allocation
meter becomes a financial bill, and no global production gate is closed.
