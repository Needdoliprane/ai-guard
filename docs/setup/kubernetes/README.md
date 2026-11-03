# Kubernetes: authority and inspection

[English](README.md) · [Français](README.fr.md) · [Configuration index](../README.md)

The local qualification target is the single-node Kubernetes cluster in Docker
Desktop. Keep Docker/Podman execution for agent sessions: the Kubernetes Rust
adapter currently supplies HTTPS diagnostics and inspection, not worker launch.
There is no `--runtime kubernetes` launch option or Kubernetes dotenv profile.

## Configure the Rust integration

Trusted control code uses
[`runtime::kubernetes`](../../../src/runtime/kubernetes.rs). It supplies an
explicit HTTPS API origin, a nonzero authority UUID, the retained UID of the
`kube-system` namespace, the tenant namespace name and UID, and the SHA-256 of
the prepared CA bytes. The authenticated control selects the tenant; a
namespace name, UID or Pod label is not a tenant credential.

Open credentials through `control_fs::PrivateFile`: the CA and either a Bearer
token file or a client certificate and private key. Keep these files in private
control storage. Use a narrowly authorized identity for the required reads:
`GET /version`, the two exact Namespace resources, and the exact Pod resource
when observing a Pod. The adapter does not provision this identity or configure
RBAC. Never give these credentials or a Kubernetes administrator kubeconfig to
a worker.

`KubernetesRuntime::connect` validates the local configuration and constructs
the client. It reads no ambient kubeconfig, current context, executable plugin
or proxy configuration. Its explicit CA replaces built-in roots; hostname and
certificate validation remain enabled. Redirects and automatic retries are
disabled. Rotating retained files requires preparing a new connection;
replacement is not a transparent credential reload.

## Diagnose and interpret observations

`doctor().await` reads both namespace anchors and the server version. It
reports the retained authority fingerprint, observed metadata and the
remaining admission gaps. A Pod Security label is reported as configuration,
not as proof of confinement. `require_workload_admission()` refuses admission.

`observe_owned(handle).await` requires a control-retained handle bound to the
authority, session, operation, exact Pod UID and post-admission spec commitment.
The integration must establish ownership before retaining that handle. A
changed UID, spec, namespace or injected extra container is refused. The
successive reads are not an atomic Kubernetes snapshot. A 404 is an API error;
it does not prove a stopped worker or justify terminal usage accounting.

Process state, restart count, image/container IDs and timestamps retain their
observed or unknown values. Configured CPU and memory limits are not measured
consumption. Each request has an eight-second timeout, a three-second connect
timeout and a 1 MiB response limit, with four simultaneous requests per runtime
instance. A diagnostic performs several requests, so eight seconds is not a
total diagnostic deadline.

## Verification and remaining work

The product regression targets are:

```sh
cargo test --locked --test kubernetes_adapter -- --test-threads=1
cargo test --locked --test kubernetes_transport -- --test-threads=1
cargo test --locked --test kubernetes_transport_lifetime -- --test-threads=1
```

They exercise the actual Rust HTTPS transport against local TLS fixtures,
including mutual TLS and cancellation observed by the peer. They do not
qualify Docker Desktop or another cluster. See the dated
[validation record](../../engineering/VALIDATION.md#3-novembre--diagnostic-kubernetes-et-durée-de-vie-du-transport).

Worker admission still requires workspace transfer, exec/attach, mandatory
control audit, independently enforced deadlines, recovery and real confinement
tests. In particular, a NetworkPolicy object alone is not evidence that the
local CNI isolates workers or tenants. No cluster configuration is changed by
these diagnostics.
