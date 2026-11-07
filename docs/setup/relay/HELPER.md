# Worker-local Rust relay helper

[Français](HELPER.fr.md) · [HTTP client](README.md) · [Dated evidence](../../engineering/evidence/2026-11-07/relay-worker-helper/README.md)

The [helper](../../../src/relay/channel/helper.rs) bridges a Unix socket inside
the worker to a dedicated framed stdin/stdout stream. It receives no workload
proof, runner credential, tenant selector, gateway URL or engine socket.

This software component is delivered; the runtime channel is not connected.
The broker exec owner, runtime admission/binding checks and durable mutation
outbox in the [approved direction](../../engineering/RELAY_RUNTIME_HANDOFF.md)
are not wired to it. Keep workers on `network:none`, outside the control network.
Do not add a TCP proxy, host socket mount or manual credential handoff to bypass
that missing integration. The real Docker/Podman transport campaign and A1/A2/B1
recipe remain **NOT_RUN** for this component.

## Image and invocation

The [worker Dockerfile](../../../images/worker.Dockerfile) builds
`ai-guard-relay` with the pinned Rust toolchain and copies it to
`/usr/local/bin/ai-guard-relay`, owned by root with mode `0555`. The worker's
readonly root belongs to its runtime confinement; image placement alone does
not prove it. Do not copy a replacement into `/workspace` or `/home/agent`, or
adopt an old image that lacks the helper.

From the repository root, an operator can build the image with the usual
installation permissions and available pinned dependencies:

```sh
docker build --file images/worker.Dockerfile --tag ai-guard-worker:relay-local .
```

This is an instruction, not a build executed or qualified by this lot. Clients
build and distribute their own images; no public prebuilt image is required.
Offline preparation and a Podman build need their own verified prerequisites.

The only internal mode is the fixed invocation
`/usr/local/bin/ai-guard-relay serve-v1`. This is not a user credential CLI.
It requires non-TTY pipe/socket stdin and stdout; other arguments, regular files
and terminals are refused. The binary uses the fixed root
`/home/agent/.ai-guard-relay`, not an environment or argv pathname.
The future trusted exec owner must supply and own these streams. An interactive
shell invocation is not a supported way to connect a worker.

The helper does not read provider output or discover environment credentials.
Keep the workload proof and its private reference in trusted control. Human,
runner and provider credentials do not become relay authority in a frame.

## Publication and local access

The intended worker profile provides a private `/home/agent` tmpfs. Inspect
actual UID/GID, modes, image, mounts, limits and network before runtime
qualification; these software tests do not inspect a real ai-guard worker.

The helper receives a valid V1 `hello`, echoes channel, epoch, resolved limits
and helper version in `ready`, and completes that write before binding its
listener or publishing discovery. A directory flock excludes another helper
using the same retained root. Every channel creates a fresh namespace:

| Entry | Behavior |
|---|---|
| Generation directory | New `0700` directory; at most 128 retained namespaces |
| `s` | New `0600` Unix socket; retained inode, link, peer UID and path-length guards |
| `relay-v1.json` | New immutable `0600` versioned record with channel/epoch and local endpoint facts; no credential |

No existing socket or record is adopted, overwritten, repaired or deleted.
Closing the helper leaves its entries in place. At a limit, preserve the
namespace for explicit operator investigation. Deleting records or rotating an
epoch does not resolve an unknown mutation.

The library API `discover(root, generation)` opens one explicit generation and
retains the record FD. `DiscoveredHelper::connect_checked(root)` revalidates
record and endpoint around connection. It neither scans for “latest” nor
reactivates old generations. Local UID and discovery are transport checks,
not authenticated tenant, human or agent identities.

Applications use the closed V1 `send`, `inbox` and `ack` frames. There is no URL,
free argv, tenant, source identity or credential field. The helper checks
channel/epoch, correlation, operation and mutation receipt IDs. The
authenticated expected inbox endpoint remains a control-side check; the helper
invents none. Inbox content is data, not an instruction or implicit ACK.

## Bounds and closure

| Boundary | Implemented limit |
|---|---|
| Handshake | 5 s total |
| Frame assembly/write | 5 s absolute, 2 s inactivity after progress begins |
| Pending operation | 45 s absolute, forwarding and local response delivery included |
| Encoded request / response frame | 278528 / 2113536 bytes, envelope included |
| Handshake, error or close frame | 16384 bytes |
| Business payload | Existing separate request, body, TTL, receipt and inbox bounds; envelope space cannot enlarge them |
| Correlations | One exchange in progress; at most 1024 per epoch, including reads |

Lengths are checked before payload allocation. The shared codec rejects invalid
UTF-8, partial frames, recursive duplicate keys, excessive depth and unknown
fields/types/versions. The async reader retains partial prefix/body positions
and deadlines across select cancellation. Control has priority, then current
IO/deadlines, then extra accepts.

There is no queue of decoded request bodies. During a pending exchange,
additional connections are closed without reading their body. A second frame
on the active connection closes that client after at most one prefix byte,
without a correlated busy response or second dispatch. The first exchange
continues until its control response, closure or timeout.

Control EOF or a matching `close` ends admission and closes owned local
clients/listener. The single async owner spawns no detached task or blocking
stdin thread. Its nonblocking owned FDs use Tokio's explicit
`unsafe AsyncFd::register` contract: the same valid FD stays open until
deregistration/drop. The [evidence](../../engineering/evidence/2026-11-07/relay-worker-helper/README.md)
records the locked API and tests.

Stdout carries frames only. On failure the binary emits at most one fixed line,
at most 32 bytes, to nonblocking stderr and exits with code 126. Codes are
`protocol`, `timeout`, `unavailable`, `unsafe_filesystem` and `capacity`,
prefixed by `relay_helper:`. No body, URL, credential or raw IO error is retained.
Several filesystem refusals, including the generation cap, share
`unsafe_filesystem`; preserve entries rather than repairing them automatically.
The future broker still needs its own owned stderr drain, 16 KiB cap and child
join; this helper does not supply that lifecycle.

After forwarding, loss or timeout can leave an effect unknown. Preserve exact
send/ACK operation and content in trusted control. Do not invent a new ID or
treat EOF as rollback, worker death, quota release or a terminal observation.
Durable replay, audit-before-effect, current binding, grants/tenant checks and
reconciliation remain control responsibilities.

The eleven helper tests use real local UDS and an OS pipe in isolated Linux QA.
They include an inbox-request/closed-error round trip, closure, substitution,
fragmentation and limits. They do not prove send/page/ACK through HTTP, workload
authority or a runtime. The bounded EOF canary does not refute starvation under
continuous traffic. RSS, aggregate tenant buffers, Docker Desktop/Podman VM,
Kubernetes, providers and production gates keep their separate qualifications.
