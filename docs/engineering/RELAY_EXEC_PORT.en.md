# Private relay exec port — software segment

This segment implements the byte transport in
[`src/runtime/managed/relay.rs`](../../src/runtime/managed/relay.rs). It opens
no product workflow: `RelayExecPermit` has no production constructor. Wiring
still needs current admission backed by durable audit, the workload binding
and the registry retaining the exclusive `RoleLease`. A software fixture does
not establish those properties. The [approved handoff](RELAY_RUNTIME_HANDOFF.md)
remains the architecture authority; [français](RELAY_EXEC_PORT.fr.md).

## Integration contract

- The trusted owner consumes a permit distinct from start. No worker frame
  supplies a CID, argv or identity.
- The permit retains `OwnedContainerSnapshot`, authorized specification,
  engine, invocation and profile. Before spawn the port checks full CID,
  session, operation, immutable image, v2/v3 configuration, allocation,
  Running, zero restarts and the same positive `StartedAt`. It inspects the
  specification's restrictions, including `network:none`, then revalidates
  the anchored invocation and retained profile with no further await before
  spawn. Inspection and process creation are not an atomic engine transaction.
- Docker requires the retained private minimal profile. Podman requires the
  retained storage/marker binding and environment selection. Existing start
  and STOP commands are unchanged. Kubernetes has no implemented exec port
  and cannot be selected through this Docker/Podman API.
- `io_pair()` lends `AsyncRead`/`AsyncWrite` wrappers compatible with
  `ControlOwner::serve`. No FD, child or stderr leaves its owner. The control
  owner enforces framing, capacity and 5/2 s absolute/idle deadlines and calls
  `close_and_join()` after service return or cancellation. The runtime port
  never reads stdout concurrently or introduces a competing codec.
  Writer `shutdown()` closes the actual retained stdin pipe while leaving
  stdout open: Unix `ChildStdin`'s native shutdown does not close its FD. The
  echo regression therefore waits for real EOF and the child's final bytes.
- The registry must retain transport and lease until confirmed join. Closing
  drops stdin/stdout, waits two seconds, requests death of only the CLI if
  necessary, and bounds the complete local join to five seconds. Unjoined
  handles remain retained for an explicit further wait. The destructor is a
  fail-closed fallback, never evidence that joining completed.
- CLI exit zero, EOF and local join still leave the engine helper
  `OutcomeUnknown`. The only supplied reconciliation reobserves the exact
  worker terminal with unchanged identity/configuration/`StartedAt`. It does
  not stop/remove that worker, commit a terminal, settle quota or record usage.
  This observation never automatically opens a replacement channel.

## Dialects and environment

Exec invokes only `/usr/local/bin/ai-guard-relay serve-v1`, as UID/GID
`1000:1000`, with stdin open and TTY/privileges explicitly disabled. Podman
also disables its configurable detach sequence. Docker installs detach-key
handling only for TTY. These are code and reviewed-source properties, not
qualification of an installed engine version. See the
[Docker exec](https://docs.docker.com/reference/cli/docker/container/exec/) and
[Podman exec](https://docs.podman.io/en/latest/markdown/podman-exec.1.html)
manuals and official [Docker hijack source](https://github.com/docker/cli/blob/master/cli/command/container/hijack.go)
and [Podman exec source](https://github.com/containers/podman/blob/main/cmd/podman/containers/exec.go).

Docker inherits container environment. The current provider handoff places
`AI_GUARD_PROVIDER_BOOTSTRAP` in `Config.Env`; the bootstrap subsequently builds
Codex/Claude's private environment. Omitting `--env` would therefore not prove
that the helper has no credential. Seven constant `--env=NAME=` arguments clear
the known bootstrap, OpenAI/Codex/Anthropic key, Claude OAuth token and
Codex/Claude configuration-directory names. No credential value enters argv.
The official [Docker replacement source](https://github.com/moby/moby/blob/master/daemon/container/env.go)
and [Podman parser source](https://github.com/containers/podman/blob/main/pkg/env/env.go)
retain an explicitly supplied empty value.

Transient inspection rejects additional names, duplicates and unrecognized
public values. Fixed Dockerfile values, fixed `TERM` and `container=podman`
are accepted; `NODE_VERSION` requires a short numeric major-24 version and
`YARN_VERSION` major 1. This profile does not qualify the image: the future
issuer must attest the immutable digest containing the root-owned helper on
the readonly root. Processes within a worker remain untrusted and share its
UID; this port does not isolate the helper from provider files already inside
that worker. It performs no global home cleanup.

Stderr is drained without raw retention; only counters and closed codes
remain. Any stderr activity interrupts channel IO; more than 16 KiB closes
that FD. Errors never contain arbitrary stderr or OS messages.

## Validation and limits

Tests under `managed/relay/tests.rs` cover closed arguments, incarnation and
configuration mismatches, inherited environment, real pipes and bounded Rust
children, opaque stderr, and CLI kill/join. The ignored internal child entry is
invoked only by its parent tests and is not counted as another product test.
QA execution results must be recorded separately: test presence and rustfmt
do not constitute PASS.

Freeze67 retained an echo-parent FAIL: read-to-end timed out after readiness
had succeeded. Source investigation identified delegation to shutdown without
closing the pipe. The correction drops that FD through its owner's borrowed
`Option`; neither product nor test deadlines increase. Freeze73 passed its
seven parent tests in isolated QA AM, then all-target compilation in QA AO.
The ignored child is not counted as another PASS. See the
[source-specific evidence](evidence/2026-11-07/relay-runtime-exec/README.md).

Real Docker/Podman, VM topologies, the helper in an actual image, runtime
credential absence, remote termination and recovery under the durable registry
remain `NOT_RUN`/`BLOCKED` according to their dependencies. This segment does
not authorize an engine, provider, cluster, OS service or download. No gate is
closed.
