# Kubernetes: authority and inspection

[English](README.md) · [Français](README.fr.md) · [Configuration index](../README.md)

The local qualification target is the single-node Kubernetes cluster in Docker
Desktop. Keep Docker/Podman execution for agent sessions: the Kubernetes Rust
adapter currently supplies HTTPS diagnostics and inspection, not worker launch.
There is no `--runtime kubernetes` launch option or Kubernetes dotenv profile.

## Run the read-only CLI diagnostic

Use `agentsandbox kubernetes doctor` with an explicit, retained authority file
and either a static Bearer token file or a client certificate/key pair. It
performs only three authenticated GET requests: the `kube-system` Namespace,
the exact tenant Namespace and `/version`. It does not create Pods, configure
RBAC, admit work, exec, attach or change the cluster. No kubeconfig, context,
credential plugin or environment discovery is performed by the product.

For the Docker Desktop target, first have the cluster owner approve the exact
API endpoint, CA, identity and read permissions. Using a separately authorized
operator tool, obtain the `metadata.uid` of `kube-system` and the intended tenant
Namespace. For example, replace both explicit placeholders in these operator
commands; `doctor` does not execute or import them:

```sh
kubectl --context <authorized-desktop-context> get namespace kube-system -o jsonpath='{.metadata.uid}'
kubectl --context <authorized-desktop-context> get namespace <tenant-namespace> -o jsonpath='{.metadata.uid}'
shasum -a 256 /absolute/private/control/kubernetes/ca.pem
```

The hash is over the exact prepared CA file bytes, including PEM whitespace;
Linux operators can use `sha256sum` for the same calculation. Retain a fresh
nonzero authority UUID assigned by the trusted control. Neither a Namespace
UID nor its labels authenticates a tenant. Do not invent UIDs, silently accept
a changed UID or use an administrator identity for convenience. A narrowly
authorized identity must be provisioned outside this diagnostic.

Prepare a **new** private directory and files without replacing existing
credentials or trust. The authority JSON uses the existing
`KubernetesAuthorityV1` contract, rejects unknown/duplicate fields and has a
16 KiB limit. The following values are illustrative and must all be replaced
with approved values; the hash placeholder is intentionally invalid:

```json
{
  "version": 1,
  "authority_id": "00000000-0000-4000-8000-000000000001",
  "endpoint": "https://127.0.0.1:6443/",
  "cluster_namespace_uid": "11111111-1111-4111-8111-111111111111",
  "namespace": "tenant-a",
  "namespace_uid": "22222222-2222-4222-8222-222222222222",
  "trust_sha256": "<64-lowercase-hex-sha256-of-prepared-ca-bytes>"
}
```

Use absolute paths without symlinks. Each input must be a regular, single-link
file owned by the effective UID with mode `0600`, under root/effective-UID
owned ancestors that do not permit group/other modification, subject to the
private-control ACL/filesystem checks. A root-owned sticky temporary directory
is the existing controlled exception. Prefer a new `0700` control directory;
the command never repairs permissions. CA/certificate files are limited to
64 KiB, the private key to 16 KiB and a token to 8 KiB. Tokens contain 32–8192
ASCII token characters with no newline or whitespace; a normal PEM
certificate/key pair is validated for matching keys and TLS compatibility.

```sh
agentsandbox kubernetes doctor \
  --authority-file /absolute/private/control/kubernetes/authority.json \
  --ca-file /absolute/private/control/kubernetes/ca.pem \
  --token-file /absolute/private/control/kubernetes/token
```

For mutual TLS, replace `--token-file` with the paired options:

```sh
agentsandbox kubernetes doctor \
  --authority-file /absolute/private/control/kubernetes/authority.json \
  --ca-file /absolute/private/control/kubernetes/ca.pem \
  --client-cert-file /absolute/private/control/kubernetes/client-chain.pem \
  --client-key-file /absolute/private/control/kubernetes/client.key
```

Both authentication choices together, or either half of a certificate/key
pair, are refused. Explicit CA trust and hostname validation are mandatory;
there is no insecure option, proxy discovery or fallback identity. The product
does not extract kubeconfig credentials or execute an authentication plugin.

Success exits `0` and writes one JSON report with `version: 1`,
`admission_available: false`, the actual `diagnostics` and an
`admission_gap_reasons` entry for every reported gap. The observations include
the authority fingerprint, server version, Namespace UIDs, Pod Security label
when recognized and observation time. Keep that metadata within its approved
audience. It is not a workload authorization, an enforcement proof or a
durable control-audit receipt. The three reads are successive, not atomic.
The existing per-request bounds below apply; several eight-second requests
can be needed, so eight seconds is not the total command deadline or a
latency benchmark on a pressured development machine.

An inspection/configuration failure exits `1` without a JSON report; incomplete
or conflicting CLI arguments exit `2`. Errors use closed categories without
paths, response bodies or transport/JSON error chains. Verify the private
files, retained identities, endpoint/CA/SAN or authorized read permissions
according to that category. If a typed observation contains retained
credential material, the whole report is refused; identities are never
replaced with invented redacted values. Correct the source privately before
retrying. Changed credentials require a newly prepared invocation.

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
cargo test --locked --test kubernetes_cli -- --test-threads=1
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
