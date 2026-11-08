# D5 composition — independent source review

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
