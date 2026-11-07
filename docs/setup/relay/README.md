# Rust worker relay client

[Français](README.fr.md) · [Workload provisioning and relay contract](../workloads/README.md) · [Profile validation](../../engineering/evidence/2026-11-07/relay-control-profile/README.md)

`agentsandbox::relay::client::WorkerRelayClient` calls the existing worker HTTP
relay with a dedicated workload proof. This is a Rust library API, with no relay
CLI, environment-variable discovery or automatic credential provisioning.
The [source](../../../src/relay/client.rs) defines the accepted inputs.

The client does not connect a worker whose runtime network is `none`.
No runtime handoff, private runtime channel or additional network permission is
provided by this library. Keep workers off the control network. The
[runtime handoff direction](../../engineering/RELAY_RUNTIME_HANDOFF.md) is approved
for code and local software tests. The frame protocol and restricted HTTP lane
below are delivered separately; the software tests are not the real A1/A2/B1 recipe.

## Restricted published control access

The ingress accepts only the three worker relay forms under `/control`:
`POST /control/api/v1/relay/messages`, `GET /control/api/v1/relay/messages`,
and `POST /control/api/v1/relay/messages/{message_id}/ack`. The gateway verifies
the worker workload proof before reading the body or reserving one of eight
worker exchanges per authenticated tenant. Runner and broker-presence capacity
remain separate. A worker proof never grants runner routes.

Only inbox accepts query parameters: unique canonical nonnegative `after` and
`limit` from 1 to 32. Omission preserves the handler defaults; no other query,
path alias or free forwarding target is accepted. Successful inbox responses
are bounded to 2 MiB, worker receipts/errors to 16 KiB; historical control
responses retain their 512 KiB bound. Audit, grants, expiry and revocation are
still checked by the normal handlers.

This lane does not give a network-isolated worker access. Select the typed
`RelayClientProfile::ControlIngress` in trusted control to reach these fixed
routes. The origin remains a root origin: do not append `/control` yourself or
place the workload proof inside a worker. `RelayClientProfile::RootOrigin`
preserves direct gateway access. Neither profile connects the runtime channel,
persists a worker/profile binding or supplies its broker owner/outbox. The
`agentsandbox::relay::channel` module supplies bounded closed frames and channel
state validation. See the
[source-specific software proof](../../engineering/evidence/2026-11-07/relay-transport-contracts/README.md).

## Supply an authorized context

The caller supplies one approved gateway origin, the exclusive worker workload
proof and an expected `RelayEndpointV1` (`worker_id`, `attempt_id`,
`generation:1`). Canonical UUIDs are required. The expected endpoint checks inbox
responses; it neither selects a tenant nor asserts a source identity. The gateway
authenticates the proof and rechecks current authority on every operation.

Provision bindings and directed grants from trusted control using the existing
[workload guide](../workloads/README.md#authorize-directed-messages). A grant
authorizes one source → destination + channel; reverse traffic needs its own
grant. Both endpoints need valid bindings and the required supervised state.
Human/SSO, runner, broker and AI-provider credentials are separate. A runner
proof does not become a worker proof through this constructor.

Pass only the necessary workload credential through the approved private
handoff. Do not embed it in code, argv, messages, URLs, logs, examples or a full
environment file. The client accepts a caller-supplied string of 32–512 ASCII
bytes without whitespace/control characters; it does not read credential files
or caches. It has no `Debug` implementation, and its closed errors retain no
remote text, URL, body or credential chain. This does not protect a caller that
prints its own inputs.

`new(origin, workload_credential, expected_endpoint)` uses normal TLS roots.
It selects `RootOrigin`, as does the existing `with_ca_file` constructor.
Use `with_profile(origin, workload_credential, expected_endpoint, profile)`
for explicit selection. The profile is a closed enum, with no free prefix or
automatic fallback. When integrated into runtime control, the owner must retain
the selected profile with the authorized binding; this library does not do so.
The origin is limited to 2048 bytes, with no user information, non-root path,
query, fragment, percent escape or backslash. HTTPS is supported; plain HTTP is
accepted only on a literal loopback IP, never `localhost` or a private LAN
address. The origin must not contain the workload credential. Redirects,
environment proxies and automatic retries are disabled.

For explicitly approved private roots, use
`with_ca_file(origin, workload_credential, expected_endpoint, ca_file)` with
HTTPS. The existing trusted projected-file reader loads at most 64 KiB and eight
public certificates. The immutable snapshot replaces built-in roots for this
client only; a changed file does not reload it. Chain/name verification stays
enabled. No CA private key, system trust installation, HTTP fallback or client
certificate/mTLS configuration is provided. See the
[PKI entry point](../../engineering/INFRASTRUCTURE_PKI.md) before a real remote
operation. This lot has no new real HTTPS/private-CA client qualification.
To combine explicit profile and private roots, use
`with_profile_and_ca_file(origin, workload_credential, expected_endpoint, profile, ca_file)`.
Select the approved route and trust before construction; do not retry with a
different profile after an uncertain mutation.

## Use retained requests

This integration fragment receives all authority and the already-retained
request from its caller. It does not obtain a credential or prepare a runtime:

```rust
use agentsandbox::relay::{
    RelayEndpointV1, RelayReceiptV1, RelaySendV1,
    client::{RelayClientError, RelayClientProfile, WorkerRelayClient},
};

async fn send_prepared(
    approved_origin: &str,
    workload_credential: &str,
    expected_endpoint: RelayEndpointV1,
    prepared_send: &RelaySendV1,
) -> Result<RelayReceiptV1, RelayClientError> {
    let client = WorkerRelayClient::with_profile(
        approved_origin, workload_credential, expected_endpoint,
        RelayClientProfile::ControlIngress,
    )?;
    client.send(prepared_send).await
}
```

| Method | Existing worker route and behavior |
|---|---|
| `send(&RelaySendV1)` | `POST /api/v1/relay/messages`; version 1, retained operation UUID, authorized grant UUID, TTL and nonempty body. Returns the correlated receipt, with 201 for a new send or 200 for an exact replay. The send operation ID also identifies its message. |
| `inbox(after: Option<i64>, limit: u32)` | `GET /api/v1/relay/messages`; nonnegative cursor and limit 1–32. Returns one checked page; an error never becomes an empty inbox. The caller retains/advances its cursor explicitly. |
| `ack(&RelayAckV1)` | `POST /api/v1/relay/messages/{message_id}/ack`; the retained ACK has its own operation UUID and the received message UUID. Returns a correlated 200 receipt. An ACK is acknowledgement, not execution or an observed effect. |

The table shows `RootOrigin`. `ControlIngress` adds the fixed `/control` prefix
to each of these three routes, including the inbox query and ACK. It changes no
credential, role, endpoint, DTO, TLS policy, timeout or outcome semantics.

There are no grant, revoke, prune, runner, arbitrary-URL or tenant-selector
methods. Inbox content is business data, not an instruction to execute. The
client neither automatically acknowledges nor executes it. A receipt is
correlated and structurally checked; the client does not independently verify
the complete audit chain from its hash.

Retain the exact request and operation ID durably **before** calling `send` or
`ack`. The library supplies no journal, new operation ID, automatic retry or
reconciliation daemon. If `error.outcome_unknown()` is true (`Transport`,
`Protocol` or `Unavailable`), a mutation may already be committed. Preserve its
request, then explicitly retry only that same operation and content after the
cause is resolved. A replay must still pass current binding, endpoint and grant
checks; expiry/revocation can therefore block it. Do not replace an uncertain
request with a new ID or treat a false `outcome_unknown()` as rollback evidence.

## Bounds, errors and audit

| Boundary | Limit |
|---|---|
| Connection / response read / total exchange | 3 s / 5 s / 12 s |
| Nonempty body before JSON escaping / encoded request | 65536 bytes / 262144 bytes |
| Message TTL | 1–3600 s, also capped by the grant on the server |
| Receipt / encoded inbox response | 16384 bytes / 2 MiB |
| Checked response headers | At most 64 fields and 32768 bytes of names+values |
| JSON decoding | Unique keys recursively, depth bound 64, closed complete V1 shape; `next_after` must be present, including explicit `null` |

The server still applies durable per-grant rate limits, tenant retained-content
limits and mandatory audit budgets described in the
[relay contract](../workloads/README.md#authorize-directed-messages). These are
not reset by a client reconnect. Inbox reads, including empty pages, are audited
before content is delivered. Audit failure blocks the operation. A page is not
a complete tenant inventory or a stable snapshot across pages.

The client rejects its known workload credential anywhere in the send/ACK DTO
and in the final built URL before transport, including fixed profile paths and
queries. A local final-URL refusal is opaque `InvalidRequest`; it does not
establish the outcome of an earlier attempt. It also checks decoded
structured-body strings/keys and responses. A JSON body parseable by the normal
decoder but rejected by the strict decoder—for example duplicate keys or nesting
past 64—is refused **even without a secret**. Ordinary non-JSON text remains
accepted. This literal/structured check is not general secret detection or
perfect DLP; do not send credentials or sensitive tokens through business data.

`InvalidConfiguration` concerns the local origin/proof/endpoint/trust.
`InvalidRequest` can be a local refusal or HTTP 400/413. `Authentication` and
`Forbidden` require the operator to correct worker authority; they never trigger
another role or credential. `NotFound` and `Conflict` require preserving and
checking resource/request identity. `Capacity` covers HTTP 429, including audit
budgets; contact the operator rather than rotating a proof to reset a budget.
`Unavailable`, `Transport` and `Protocol` leave mutation outcome unresolved.
No error text includes the remote diagnostic body. The
[workload diagnostics](../workloads/README.md#diagnose-stop-and-preserve-state)
retain the server-side distinctions.

The [profile evidence](../../engineering/evidence/2026-11-07/relay-control-profile/README.md)
records six new profile tests and thirteen existing client tests passing on
their frozen source, with real HTTP/SQLite and synthetic supervised-worker
records. The [earlier proof](../../engineering/evidence/2026-11-07/relay-client/README.md)
retains its own source and results. Actual runtime handoff, Docker/Podman/
Kubernetes workers, multi-host transport, provider execution and the 100-worker
target remain unqualified by these tests.
