# D5 composition — independent source review

## Recomposition after S2, 9 November

The candidate has now been recomposed on published integration commit
`a9f28ebb1b37a8b68a2a894b4de6e7e76db95c00`. It preserves the S2 gateway,
policy, authentication and usage paths. The only overlap in the 38 S2 paths
is `main.rs`: two Full3 CLI additions, with the gateway lifecycle unchanged.
Root independently materialized and checked every file in all three complete
source variants. No prior execution result transfers to this composition.

| New record | Files | SHA-256 |
|---|---:|---|
| D5 baseline manifest | 2 378 | `852bd12cc5bda87f3b4ed250d5a9d64e90077859065d9746e001766ac04a080e` |
| C1/C2 reproduction manifest | 2 381 | `7c1981adce24cdf84156681e82b237d7559ce133f57157f4b72e8c251374fff3` |
| Corrected candidate manifest | 2 381 | `9f16fb3306a9ddeaff920cf5755493213c102f3a47c0f7e1b4a858e042547c1c` |
| Root composition review | — | `467215e618c510aa258c219967411cbccb4c7c524396e6000bd8739ab0cfc158` |
| Completed artifact index | 392 entries | `61086218cec9d2c7740f74e158bb99d21b3e923cb1c73cb68c157e1e258427d2` |

The C1 candidate carries the immutable private-CA trust through the real worker
binding construction. Its semantic review and separate formatting review are
retained. The C2 candidate preserves the observed cause in two waiters and
`run_handoff`, without changing the V1 wire enum or extending the original
deadline. Its independent source review is retained. Other generalized error
paths are outside this correction's scope.

These are source candidates, not integrated fixes. The targeted execution
results below cover C1/C2 on the new composition. The historical
Full3 failure and the retirement continuation question remain open; neither
a source comparison nor a source review establishes runtime recovery.

## Targeted C1/C2 execution

Three decisive parents on reproduction manifest `7c1981ad…` return RC101 at
their expected assertions: C1 fails at the first retained worker Send (1.41 s);
the attachment clock case returns `Expired` instead of `Clock` (0.02 s);
the watcher cancellation case returns `Timeout/Expired` instead of
`AuthorityCancelled/Revoked` (0.02 s). Each command selects exactly one test.
These expected failures reproduce defects; they are not product PASS results.

An independent source/log review confirms the C1 preconditions and successful
cleanup checks by control-flow inference. HTTP200 and cleanup are not printed
markers. The transport error is opaque; the specific TLS cause is inferred
from the source. The review SHA-256 is
`ef91d6b5bb9427a19b319e5554337d970ee227df0f50b2a2ab8beb6473036129`.

The same tests on corrected manifest `9f16fb33…` give:

| Command scope | Observed result |
|---|---|
| Exact C1 TLS parent | 1 PASS, 0 FAIL, RC0; build 2m01s, test 1.37 s |
| C2 attachment/watcher cases | 6 PASS, 0 FAIL, RC0; tests 0.15 s |
| `cargo check --frozen --offline --all-targets` | RC0, 42.32 s |

The positive C1 path includes Send, Inbox, ACK and exact ACK replay after the
CA pathname was replaced. C2 verifies clock, cancellation and original expiry
in both components. These are seven unique software test functions, not real
worker runtime qualification. Warnings remain: 84 lib, 14 lib-test including
seven duplicates, three relay-handoff, four Full3 and two SSO-test warnings.
Clippy and the release profile remain **NOT_RUN** for this composition.

An initial rustup component-sync failure and a later wrong manifest pathname
both stop in setup before Cargo. Their original logs are retained; neither is
a behavioral RED. Both envelopes were corrected without a source, lockfile or
product-deadline change. The first QA was actually removed at 07:40:06 UTC,
later than its planned absolute stop: the wall-clock bound was not met, with
cause unknown. Its retained reproduction evidence index is
`69a31d1652a0d52e1b95b47fe5e5de258daee5a0136c12a7f24833c4e3e895bf`.
The corrected tests ran in a separately admitted QA with an absolute host-clock
watcher. These results do not close Full3, retirement continuation, D5
integration, physical fencing, engine/cluster qualification or a global gate.

## F1: confirmed checkpoint observation

The prerequisite canary initially selects one test and fails RC101 with a
journal identity/revision conflict. Its fixture constructs different namespaces
for the Journal and supervision; the unchanged product contract requires the
same namespace. The reviewed tests-only correction uses the real Journal's
namespace getter and adds fixed error contexts. Hash assertions, verifier,
application, polling and deadlines remain unchanged.

Full manifest `3fb5cf3f44c622b6b9c6d9c578f5fd8937394ed34619b44de492095970f448f4`
changes only `tests/support/broker_relay_full3.rs` from `9f16fb33…`.
All 2 381 files are verified before and after execution. The corrected canary
passes (one selected, RC0, build 1m18s, test 0.06 s). Its V1–V4 frame and
negative hash/version checks do not independently qualify V5 or release.

One Full3 observation then fails RC101 with `deadline has elapsed`, after
61.28 s. The original endpoint 60 s, helper 120 s and worker 90 s windows are
unchanged. After successful cleanup/join, the existing bounded ticket reads
only the previously IPC-confirmed selected frame:

| Fact in the selected snapshot | Observed value |
|---|---|
| Frame | Revision 17, V5, 29 049 bytes |
| Import Arm, claim, observation and cleanup | Each 1 request and 1 local acknowledgement |
| Import-verification helpers / owned helpers | 0 / 0 in this snapshot |
| Comparable helper deadline at capture | 71.230282728 s remaining |
| Worker deadline, full chain and binding payload validation | Unknown |
| Ticket errors / truncation | 0 / false |

The trace's observation/cleanup ACKs remain unknown; its gateway successes
are distinct from the local acknowledgements in the confirmed frame. The
post-cleanup inventory is complete for the two bounded job directories, but
only the selected payload is read. Reader timings measure that diagnostic
after cleanup, not live writer cost. This narrows the observed boundary to
acknowledged Import cleanup before published Import verification; it does
not establish the root cause or absence of progress after that snapshot.
No performance qualification, runtime outcome or automatic retry follows.

The actual parent launches at 18:26:15 UTC after the preflight time check,
15 s beyond the prescribed launch guard; this deviation is retained.
The QA is actually stopped and removed at 18:28:58 UTC, before its 18:31
absolute stop. Final inventory contains only PID1; OOM counters are zero,
observed memory peak 7 106 428 928 bytes. The watcher observes closure and
terminates RC0. The retained 27-entry text/JSON evidence index has SHA-256
`bece2b4f238fb866639bf61540459fbe868c8ff9c5942b99a424e820b5a67561`.
No database, key or worker volume is removed or promoted. Disposable compiled
targets are reclaimed after consumer checks; no ELF or target cache is promoted.

Full3 remains **FAIL**, D5 remains unintegrated, and all applicable production
gates remain open. The next diagnosis must distinguish the executor's
continuation from publication and Status observation; a snapshot ACK alone
does not prove that its caller has resumed.

## Original composition and reviews

The private candidate composes the D5 broker/relay work on integration commit
`4a0d4c617be23dd58f63deb7325b9306d91c7ade`, preserving S1, CLI11 and TLS
changes. The 100-path delta contains 49 modifications and 51 additions;
the complete immutable source has 2 366 files. This is an unintegrated
candidate. Compilation and tests on this composition remain **NOT_RUN**.

| Record | SHA-256 |
|---|---|
| Complete source manifest | `d091bd683b8e1ec71dbbb09f419d7e352ab30ba9dcf469d6f707a493fbf19adf` |
| Root-to-candidate patch | `515a2f6a747f1da748e24ec122516bb448189143260f6d218de048846f3a0b56` |
| Composition receipt and artifacts manifest | `f10d74c098f5ed4e956e6fc0526e94f012c328c814b091b271c9c08813fcd9c0` |
| Independent service/relay review | `be23944720f288d149ddf396bede346bb18f76fc7d541024bfe8aa8c73d5c71e` |
| Independent journal/ledger/writer review | `3b171b126774cda0c413f07a0022ff9f1512fda06f14e01d2d28e05b03960585` |
| Root review of selected retirement seams | `a88e0bab401de68876d3a8c7c5971c89b89d1cc13eca5fbef8b6114e38e48ddc` |

Private reports and source remain outside the product checkout. These hashes
identify source reviews, not executed tests or a new comprehensive audit.

The main reviewer independently checked the 31-artifact manifest and all
2 366 source entries. Of 62 paths changed in Root since the D5 base, 57 remain
byte-identical; five shared files are composed explicitly. All 61 Kubernetes
source files remain identical to Root. The journal reviewer identifies its
scope and does not claim a second complete manifest verification.

## C1: TLS handoff blocks integration

**NO-GO_SOURCE.** The profile captures immutable `ClientTrust` for
`AuditClient`, but `RetainedWorkerBinding::client` calls
`WorkerRelayClient::with_profile`, which selects default trust. The
broker relay path therefore loses an explicitly configured private CA.
The source trace includes `workload_cli/broker_binding.rs`,
`relay/channel/control.rs`, its `io_owner.rs`, and `relay/client.rs`.

This is a broken configured HTTPS path, with no downgrade or fail-open
demonstrated. The correction must carry the same captured trust to the
real binding client. It must not reread the CA path during dispatch or
add default roots when explicit roots were chosen. Acceptance requires
a real loopback TLS regression through that construction path, including
snapshot preservation after CA-file replacement and a wrong-trust refusal.
The reproduction and GREEN are **NOT_RUN**.

## C2: observed error cause is lost

In the late-binding wait and the channel control watcher,
`wait_expiration` can return `Clock`, `Cancelled` or `Expired`, but its
result is discarded and reported as expiration. Preserve the actual
variant and its explicit outcome mapping, without extending deadlines.
The regression must change the clock identity or cancellation after
entering the wait and verify the correct cause. **NOT_RUN**.

## Retirement continuation: needs validation

The selected Root review finds `AuthorizedRetirement::into_parts`
consuming the unique revocation wrapper before the enforcer client
connects. A pre-submission failure returns through `?` without the
explicit retention used in adjacent phases. A server-side coordinator
cannot retain a request it has not received.

This is a source-grounded recovery question, with no unauthorized start,
lease release or quota corruption reproduced. A targeted software test
must distinguish an unsubmitted failure from a submitted unknown result,
retain fixed identities and preserve STOP and the original deadline.
Serialized evidence must not reconstruct live authority. **NOT_RUN**.

The journal/ledger/writer review reports no new blocker in its stated
scope. It does not override C1 or approve the enforcer producer. The
historical Full3 failure remains unresolved; its cause and job sizes
remain unknown. No runtime, physical crash, performance or production
qualification follows from these source checks.
