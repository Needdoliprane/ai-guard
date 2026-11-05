# Kubernetes: authority, inspection and admission preflight

[English](README.md) · [Français](README.fr.md) · [Configuration index](../README.md)

The local qualification target is the single-node Kubernetes cluster in Docker
Desktop. Keep Docker/Podman execution for agent sessions: the Kubernetes Rust
adapter supplies HTTPS diagnostics, inspection and an audited admission dry-run,
not worker launch. Qualification against that real cluster remains `NOT_RUN`.
There is no `--runtime kubernetes` launch option or Kubernetes dotenv profile.

## Prepare the Docker Desktop inspection identity

The public [inspection profile](../../../deploy/kubernetes/local-inspection.json)
creates a dedicated `ai-guard-preflight` Namespace, `ai-guard-inspector`
ServiceAccount with token automount disabled, and read permissions for only
that Namespace, `kube-system` and `/version`. It contains no credential or
worker workload. Pod Security `restricted` is pinned to `v1.36`, matching the
local qualification target; review this pin when changing cluster versions.
Confirm the context is your local Docker Desktop cluster before provisioning:

```sh
kubectl config current-context
kubectl --context docker-desktop --request-timeout=10s create -f deploy/kubernetes/local-inspection.json
```

Run from the repository root using a separately authorized operator identity.
`create` preserves existing resources by refusing replacements. A multi-object
creation can partially succeed: retain its output and inspect those exact
objects before continuing; do not delete/recreate a Namespace to clear an error.
If these names are already in use, prepare an owned copy with all Namespace,
subject, role and resource-name references changed consistently.

Only admission preflight needs the separate
[Pod-create grant](../../../deploy/kubernetes/local-preflight.json):

```sh
kubectl --context docker-desktop --request-timeout=10s create -f deploy/kubernetes/local-preflight.json
```

This grants actual `create pods` inside the dedicated Namespace, **not only
dry-run**. It grants no direct API permissions for exec, logs, Secrets,
other namespaces or deletion. Pod creation can nevertheless consume existing
Secrets or service accounts in that Namespace, subject to admission. Keep it
dedicated to this test; disabled ServiceAccount automount is not general Pod
confinement.
Kubernetes RBAC cannot restrict top-level create by object name or dry-run;
keep this identity in trusted control. See the official
[RBAC rules](https://kubernetes.io/docs/reference/access-authn-authz/rbac/).

Prepare a new `0700` private control directory outside source/export trees,
then request a short-lived token directly into a new private file:

```sh
(
  umask 077
  set -C
  kubectl --context docker-desktop --request-timeout=10s \
    --namespace ai-guard-preflight create token ai-guard-inspector \
    --duration=10m --allow-missing-template-keys=false \
    -o jsonpath='{.status.token}' > /absolute/private/control/kubernetes/token
)
```

Check the exit code; keep any incomplete output private and prepare a different
new path after failure. JSONPath avoids a trailing newline. The server chooses
the actual lifetime; this is not an automatic renewal service. The
[TokenRequest command](https://kubernetes.io/docs/reference/kubectl/generated/kubectl_create/kubectl_create_token/)
defaults to the API-server audience. Do not create a permanent token Secret or
pass the operator kubeconfig to the product. Supply the approved CA and exact
Namespace UIDs in the authority file below, with `namespace:ai-guard-preflight`.
These profiles do not establish tenant ownership, effective confinement or a
worker launcher. Applying them and the product's real cluster probes remain
separate qualification steps.

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

## Run an audited admission preflight

`agentsandbox kubernetes preflight` submits a closed Pod profile to the API
admission chain without persisting a Pod or running an image, command or worker.
It uses the same explicit Kubernetes authority, CA and authentication files as
`doctor`, plus a private plan and a private control configuration. It does not
provision RBAC, install a service or enable Kubernetes session launch.

First retain a successful `doctor` report for the approved authority. Ask the
cluster owner to provision the existing read permissions plus `create` on
`pods` in the exact target Namespace. Kubernetes maps this POST to `create`,
and dry-run authorization is identical to real creation authorization. This
credential can therefore also create real Pods when used by other tools; it
is **not a dry-run-only credential**. Keep it in trusted control storage and
out of workers. See the official [dry-run authorization contract](https://kubernetes.io/docs/reference/using-api/api-concepts/#dry-run-authorization)
and [request-verb mapping](https://kubernetes.io/docs/reference/access-authn-authz/authorization/#determine-the-request-verb).

Prepare two **new** files using the
[plan example](examples/preflight-plan.json) and
[control example](examples/preflight-control.json). These are templates, not
working configurations: replace every angle-bracket placeholder. They contain
no real authority, image digest, UUID, endpoint or credential. Keep the files
and credentials `0600`, with the safe ownership, ancestor and ACL checks
described above; use absolute paths without symlinks. Plan and control JSON
are each limited to 16 KiB. Unknown fields and duplicate typed fields are
refused. Do not replace an existing configuration to prepare this attempt.

The version-1 plan contains:

- `authority_fingerprint`: the exact `diagnostics.authority_fingerprint` from
  `doctor` for this unchanged authority file, not the CA hash.
- `session_id` and `operation_id`: fresh nonzero UUIDs for this attempt. The
  operation UUID identifies the requested audit event; divergent reuse is a
  conflict, not another dry-run permission.
- `image`: a real operator-selected repository reference with immutable
  `@sha256:` and a 64-character lowercase hexadecimal digest. A tag alone is
  refused; keep secrets and credentials out of this field. A successful preflight
  does not prove that the image is available or runnable; this profile neither
  pulls nor executes it and declares `imagePullPolicy: Never`.
- `resources`: `cpu_millis` from 1 to 200000, `memory_mib` from 1 to 102400,
  positive `workspace_mib` and `temporary_mib` whose sum is strictly less than
  `memory_mib`, and `deadline_seconds` from 1 to 86400. The example declares
  2000 milliCPU, 1024 MiB memory, 128/64 MiB workspace/tmp and 60 seconds.
  These bounds and declarations are not capacity or deadline qualification.

The version-1 control file contains only `gateway`,
`control_namespace_uuid` and `runner_token_file`, alongside `version: 1`.
Obtain the control namespace from the authenticated existing control, for
example with the command below after replacing the example gateway and path:

```sh
agentsandbox broker control-info \
  --gateway https://gateway.example.invalid \
  --token-file /absolute/private/control/runner
```

Use its exact `control_namespace_uuid`, a runner identity and compatible
schema 3–8 with `broker_supervision_v1`. The gateway must be an HTTPS origin
or an explicitly selected HTTP loopback origin; this file does not add private
CA or mTLS options to the gateway connection. `runner_token_file` is an
absolute path to a private file containing at most 4 KiB, not a token value. The control runner
and Kubernetes credentials are separate. There is **no `tenant_id` field**:
the authenticated server supplies the tenant. The chosen Kubernetes authority
fingerprint is bound to the audited request; this does not prove exclusive
ownership of that Namespace by that tenant or grant a workload start.

Run the prepared attempt:

```sh
agentsandbox kubernetes preflight \
  --authority-file /absolute/private/control/kubernetes/authority.json \
  --ca-file /absolute/private/control/kubernetes/ca.pem \
  --token-file /absolute/private/control/kubernetes/token \
  --plan-file /absolute/private/control/kubernetes/preflight-plan.json \
  --control-file /absolute/private/control/kubernetes/preflight-control.json
```

For Kubernetes mTLS, replace `--token-file` with both certificate options;
the control file and its runner credential are still required:

```sh
agentsandbox kubernetes preflight \
  --authority-file /absolute/private/control/kubernetes/authority.json \
  --ca-file /absolute/private/control/kubernetes/ca.pem \
  --client-cert-file /absolute/private/control/kubernetes/client-chain.pem \
  --client-key-file /absolute/private/control/kubernetes/client.key \
  --plan-file /absolute/private/control/kubernetes/preflight-plan.json \
  --control-file /absolute/private/control/kubernetes/preflight-control.json
```

The control first verifies the server identity/namespace and an exact durable
receipt for `runtime.kubernetes.preflight.requested`. Only then can a private,
consumed guard bound to the target and receipt permit the dry-run. It rechecks
the retained authority, plan, control and runner-token files and expires 60
seconds after acknowledgement. The adapter GETs both Namespace anchors before
and after its only POST:

```text
POST /api/v1/namespaces/{retained-namespace}/pods?dryRun=All&fieldValidation=Strict
```

The Pod profile has one fixed `/usr/bin/id` command, no caller-supplied business
argv or environment, UID/GID 1000, read-only root, dropped capabilities, no
privilege escalation, seccomp RuntimeDefault, no host namespaces, no mounted service
account token, bounded memory-backed workspace/tmp and declared resource and
deadline limits. It does not accept a free Pod manifest. Admission injections
or changes outside the reviewed defaults are refused, including sidecars,
init containers, credentials, host volumes and weakened security. Only the
default service-account alias, priority/preemption defaults and the two
standard 300-second NoExecute tolerations are allowed as additions; omitted
false boolean defaults are normalized. Review a refusal with the cluster
owner rather than bypassing the profile. The outgoing Pod uses strict API field
validation. The audit receipt requires its complete closed envelope, rejects
unknown root fields and rejects duplicate JSON members at every depth.

Under the [Kubernetes dry-run contract](https://kubernetes.io/docs/reference/using-api/api-concepts/#dry-run),
admission/defaulting run without storing the Pod. No image is pulled or
executed by this command. The returned UID is not retained as an owned Pod
handle. The successive Namespace checks detect replacement but are not an
atomic UID precondition on the POST. Dry-run acceptance proves neither CNI
isolation nor the integrity of third-party admission webhooks.

Before reporting success, the control must receive an exact durable receipt
for `runtime.kubernetes.preflight.result`. Both events have source
`trusted-control`, provenance `declared` and coverage `partial`: the API
acknowledges durable reception, not independent runtime observation. Exit `0`
returns a JSON report with the server tenant, control namespace, both receipts,
observation and **`admission_available: false`**. All ten admission gaps remain.

An error exits `1` without a success JSON report; malformed CLI arguments exit
`2`. No automatic retry occurs. A missing or divergent request receipt blocks
the dry-run, although the request event may already have been stored. After
any runtime error, result-audit failure or final report failure, admission may
have received the dry-run: an error does not prove zero POSTs. Review the
authorized audit by operation UUID and reconcile the possible reception before
preparing an explicit new attempt with a fresh operation UUID. Keep unknown
effects unknown; a dry-run result never releases quota or records worker usage.

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
cargo test --locked --test kubernetes_preflight -- --test-threads=1
cargo test --locked --test kubernetes_preflight_cli -- --test-threads=1
```

They exercise the actual Rust HTTPS transport against local TLS fixtures,
including mutual TLS and cancellation observed by the peer. They do not
qualify Docker Desktop or another cluster. See the
[validation record](../../engineering/VALIDATION.md) for each run's source,
date and scope; earlier diagnostics do not qualify the preflight.

Worker admission still requires workspace transfer, exec/attach, mandatory
launch audit/start authorization, independently enforced deadlines, recovery
and real confinement tests. In particular, a NetworkPolicy object alone is not evidence that the
local CNI isolates workers or tenants. No cluster configuration is changed by
these diagnostics or preflight. Real Docker Desktop single-node qualification,
including its CNI/RBAC/admission behavior, remains `NOT_RUN`.
