# Kubernetes: authority, inspection, admission preflight and reconciliation

[English](README.md) · [Français](README.fr.md) · [Configuration index](../README.md)

The local qualification target is the single-node Kubernetes cluster in Docker
Desktop. Keep Docker/Podman execution for agent sessions: the Kubernetes Rust
adapter supplies HTTPS diagnostics, inspection and an audited admission dry-run,
not worker launch. Qualification against that real cluster remains `NOT_RUN`.
There is no `--runtime kubernetes` launch option or Kubernetes dotenv profile.

The internal held-CREATE path connects the broker to the authenticated control
API and retains its request, result candidate and ACK in a private journal.
One-shot CREATE permission requires an exact live201 received within the
original claim window; replay or GET responses cannot grant it. Recovery submits the
same retained result. When that result was lost before retention, recovery can
look up the exact historical claim and durably report unknown ownership.
`kubernetes reconcile-held-create` exposes this control reconciliation without
contacting Kubernetes. Production activation and worker launch remain closed.
Its HTTP/SQLite
and local TLS-peer tests do not qualify the Docker Desktop cluster or worker
confinement; see the [engineering contract](../../engineering/KUBERNETES_ADMISSION.md)
and [validation record](../../engineering/VALIDATION.md).

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
and either a static Bearer token file or a client certificate/key pair.
Without `--check-permissions`, it performs only three authenticated GET requests:
the `kube-system` Namespace,
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

### Inspect namespace readiness and the current identity's permissions

Add `--check-permissions` to the same explicit `doctor` command when you intend
to query the authorization API. This opt-in performs
`POST /apis/authorization.k8s.io/v1/selfsubjectaccessreviews`; it does not create
Pods, grant rights or configure RBAC. It uses the same retained TLS trust and
identity, without proxy, redirect or automatic retry. If that identity cannot
create SelfSubjectAccessReviews, the diagnostic fails with an opaque HTTP
error; it never broadens its permissions.

The report retains `version: 1` and `admission_available: false`, and adds a
closed `prerequisites` object with its own `version: 1`. Without this flag the
field is omitted and the original report is unchanged. Both retained Namespace
anchors are classified as `active`, `terminating` or `unknown`. A deletion
timestamp takes precedence over an `Active` phase. Missing or unrecognised
phase remains unknown. If either Namespace is not active, the report explicitly
sets `permission_review_status: skipped_namespace_not_active` and makes no
SSAR request. Preflight and held CREATE refuse these same inactive or unknown
Namespace conditions before a Pod POST; a matching name or UID alone is
insufficient.

For active anchors, each query uses the exact tenant Namespace and the current
identity, with no impersonation or caller-controlled scope. The twelve actions
are get/create/patch/update/delete Pods, create Pod binding/exec/attach, get Pod
logs, get/list Secrets, and create ServiceAccount tokens. Reviews expose only
the action, `allowed`/`denied`/`unknown`, whether an evaluation error occurred,
and the local observation time. Remote reason/error text is discarded. No
opinion or an evaluation error remains unknown; contradictory status, changed
scope, malformed data or transport failure fails the command without partial
JSON. The adapter rechecks both anchors after the reviews.

`complete` means that all twelve queries completed, not that the principal is
safe or sufficiently privileged. The explicit coverage is
`queried_actions_only`: answers do not prove named-resource grants, actor
exclusivity, admission, CNI isolation or future authorization. No access to
Secrets, tokens or Pod subresources is exercised by asking these questions.
These observations never create an activation capability or close a readiness
gate. Kubernetes documents SSAR as an authorization query for the current
identity. [Official authorization API](https://kubernetes.io/docs/reference/access-authn-authz/authorization/#checking-api-access).

The opt-in has a 30-second asynchronous diagnostic deadline; each request keeps
the existing eight-second timeout and three-second connection timeout. An SSAR
response is limited to 32 KiB and JSON depth 16, with duplicate fields rejected.
There are three initial GETs, twelve SSAR POSTs and two final GETs when all
queries complete. Reads are not an atomic snapshot. Local TLS-peer and CLI
regressions do not qualify a real cluster; real Docker Desktop permission and
admission effects remain `NOT_RUN` for this change.

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

Preflight also requires access to create a `SelfSubjectAccessReview`. It asks
only whether this same Kubernetes identity may `create` core `v1` `pods` in
the retained Namespace. The query uses the same pinned HTTPS authority, CA
and Kubernetes credentials as the dry-run, without impersonation or free
caller-supplied scope. No role or permission is provisioned by this command.

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
schema 3–10 with `broker_supervision_v1`. The gateway must be an HTTPS origin
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
seconds after acknowledgement. The adapter first GETs both Namespace anchors,
then sends the separate authorization query:

```text
POST /apis/authorization.k8s.io/v1/selfsubjectaccessreviews
```

Only an `Allowed` create/pods opinion without an evaluation error can proceed.
An explicit denial or an unknown opinion, including an evaluation error,
fails before guard consumption and before any Pod POST. The adapter rereads
both Namespace anchors after the review and consumes the guard immediately
before its single Pod dry-run POST:

```text
POST /api/v1/namespaces/{retained-namespace}/pods?dryRun=All&fieldValidation=Strict
```

Both anchors are read again after the Pod response. A normal successful path
therefore has eight Kubernetes requests: two initial GETs, one review POST,
two GETs before the guard, one Pod POST and two final GETs. Five requests
precede guard consumption; a rejected Pod POST ends after six. These are
sequential observations, not an atomic permission or NamespaceUID guarantee.
Permissions can change after `Allowed`, and errors from the actual Pod POST
remain errors. The existing per-request timeout and guard expiry still apply;
eight requests is not an eight-second total deadline.

The result audit uses `preflight_permission_denied` for an explicit denial and
`preflight_permission_unconfirmed` for an unknown opinion. Remote reason and
evaluation-error text are not retained or printed. These checks establish no
cleanup permission, absence of broader rights, CNI isolation, actor exclusivity
or workload activation. The GET-only `doctor` and the twelve reviews requested
by `doctor --check-permissions` keep their separate behavior.

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

## Configure the central tenant policy

The gateway accepts an optional `held_create_policy` in each tenant entry of
its existing `--tenants-file` registry. The central broker HTTP API uses this
configuration for Reserve/Claim decisions; the configuration alone is no
runtime permission. The broker still cannot launch Kubernetes workers, and
the diagnostic and dry-run commands above remain separate from this policy.

Merge this **partial JSON object into the intended existing tenant entry**,
preserving its database, credential references and other settings. It is not
a complete registry file or a field for the preflight control file. The
fingerprint and image below are fictitious; replace them with the approved
full authority fingerprint reported by `doctor` and exact immutable image:

```json
{
  "held_create_policy": {
    "version": 1,
    "decision_ttl_seconds": 30,
    "approvals": [
      {
        "authority_fingerprint": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "image": "registry.example.invalid/worker@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "profile": "kubernetes_held_pod_v1"
      }
    ]
  }
}
```

An absent or `null` policy leaves admission closed; `{}` is invalid. A policy
requires version 1, a decision TTL of 1–300 seconds and 1–64 distinct approval
tuples, within 16 KiB of compact JSON. Unknown fields and duplicate approval
tuples are refused. Each fingerprint is 64 lowercase hexadecimal characters,
not the CA hash or authority UUID. The image must be at most 256 bytes, with
an explicit repository name and `@sha256:` followed by 64 lowercase hexadecimal
characters; a tag alone is refused. The repository name starts with a lowercase
ASCII letter or digit, uses only lowercase ASCII letters, digits and `/._-:`,
and contains neither `..` nor `//`. The profile is exactly
`kubernetes_held_pod_v1`. Matching uses the complete fingerprint/image/profile
tuple: a different repository name with the same digest does not match.

The gateway validates and sorts approvals when loading the registry, without
normalizing their strings; entry order does not affect the policy hash.
`worker_quota` remains the tenant's separate durable quota configuration.
These checks validate configuration only; they do not qualify Namespace
ownership, cluster admission, RBAC or CNI enforcement.

## Maintain the audit database: schema 8 to 9

Schema 9 adds a durable held-create policy format marker to an existing,
validated schema-8 tenant database. It adds no table, enables no admission
and creates no Pod. The SQLite database and tenant policy remain with the
gateway; Kubernetes credentials remain with the trusted broker.

Stop the gateway and every broker using this database before maintenance.
`--confirm-control-stopped` is your attestation; it does not stop or fence
those processes. Adapt all example paths and the tenant ID to the existing
installation. Use its configured audit event limit (`10000` below is an
example), and a **new, nonexistent backup file** in private control storage:

```sh
agentsandbox migrate-control \
  --data /private/control/tenant.sqlite \
  --tenant-id tenant-a \
  --backup /private/control/before-policy9.sqlite \
  --target-version 9 \
  --confirm-control-stopped \
  --max-events 10000
```

The command creates, verifies and synchronizes the schema-8 backup, then
rechecks the source audit head before committing the marker and version 9.
Keep the backup and the JSON migration report, including the marker sequence
and hash. The existing control namespace, projections, quota reservations and
revision/epoch CAS values are preserved. Reopening the database does not
restore active supervision. An existing backup is refused, and a failed
attempt can leave a backup file: preserve it and inspect the failure before
selecting another new destination. Never overwrite a backup or force a
downgrade by changing `PRAGMA user_version` or rewriting audit data.

Historical V1 audit validation and projection reconstruction remain available.
On schema 9, the central Store now issues V2 Admission and Reconcile decisions.
V1 in-memory capabilities remain refused on 9; an appropriate current V2
capability can recover an exact historical V1 receipt and its associated
snapshot without appending or issuing Fresh permission. Preserve pending
journals and unknown quota reservations. Neither schema 9 nor a configured
policy enables Kubernetes worker launch.

## Use the central broker control API

The gateway exposes six broker-only HTTP routes under `/api/v1` for protocol
metadata, reservation, claim, historical claim lookup, result and cancellation;
see the [route and DTO contract](../../api.md#kubernetes-held-create-http-v1).
They authenticate the current broker and derive the tenant from its bearer
proof. No header, query or JSON field selects a tenant, policy or quota.
The protocol's `admission_policy_configured` reports configuration, not
admission availability or cluster qualification.

Reserve/Claim use the current immutable tenant policy and quota, plus current
identity, grant and active supervision. Each lookup/result/cancellation obtains
a separate Reconcile decision from the real original reservation and current
grant/quota; a withdrawn policy, reduced quota or inactive supervision does
not by itself prevent this reconciliation. Revoked or divergent identities
remain refused. Policy provenance is a declared configuration snapshot, not a
named human approval or confinement proof.

Claim returns `201` only with a matching first-commit Store proof and a live
emission window at response preparation. Replays and historical GETs return
`200`/`observe_only`, never Fresh. A JSON response alone is no runtime
permission. The broker HTTP client/live ACK channel and coordinator are connected
in the internal held-CREATE path. Activation, workspace transfer and Kubernetes
agent sessions remain unavailable. These control routes and preflight do not
create or admit a Pod in a cluster.

## Reconcile a retained held-CREATE journal

Use `agentsandbox kubernetes reconcile-held-create` from trusted control storage
with an existing private held-CREATE journal and a currently authorized broker
credential. The command reads control history and confirms a retained result.
It opens no Kubernetes connection and requires no Kubernetes credential,
activation capability or running worker. It cannot prepare a new journal or
create a reservation, claim or Pod.
The control must expose the broker held-CREATE protocol on schema 9 or 10.

Prepare a new private control JSON file, limited to 16 KiB. Replace every
placeholder with the existing installation's approved values:

```json
{
  "version": 1,
  "gateway": "https://gateway.example.invalid",
  "credential_id": "<current-broker-credential-uuid>",
  "broker_token_file": "/absolute/private/control/broker-token",
  "gateway_ca_file": "/absolute/private/control/gateway-ca.pem"
}
```

The format is closed: unknown or duplicate typed fields are refused. There is
no tenant, broker or incarnation field. The journal supplies scope expectations;
the authenticated control selects the tenant and must confirm the complete
tenant/control-namespace/broker/credential/incarnation tuple. A journal does not
grant access. Revoked credentials, grants or a changed incarnation remain refused.
Reuse the registered credential UUID associated with that token.

Use absolute paths and the private ownership, mode and ancestor checks described
above. Keep the control JSON, token and CA outside the journal, source and export
trees. The broker token contains 32–4096 ASCII token characters, with one optional
terminal LF; its file is limited to 4097 bytes. The optional gateway CA file is
limited to 64 KiB and eight certificates. For HTTPS, omitting it uses the client's
built-in roots; supplying it replaces those roots for this connection. HTTP is
accepted only for an explicitly configured loopback IP origin, without a CA
file. Redirects, ambient proxies and automatic retries remain disabled.

```sh
agentsandbox kubernetes reconcile-held-create \
  --journal-dir /absolute/private/control/held-create-journal \
  --control-file /absolute/private/control/reconcile-control.json
```

When the journal already contains a candidate, the command submits that exact
result with its original operation ID and CAS values. A lost central ACK can
therefore recover its historical receipt without another claim or Kubernetes
CREATE. The first retained refusal remains historical and unchanged.

For a reopened journal without a candidate, recovery uses only the exact
historical claim GET. If the claim exists, it retains that history and a result
`ownership_unknown` with reason `recovered_after_claim`, using the historical
revision and epoch. A response lost before candidate retention cannot establish
Pod ownership: no later lookup by Pod name or matching spec adopts an UID. A
missing claim fails without creating one. A central cancellation can make the
historical CAS stale; the candidate and refusal are retained without rebasing,
an ACK or quota release. A withdrawn policy or inactive supervision alone does
not prevent an authorized Reconcile operation. This recovery cannot activate
admission.

Exit `0` confirms the central result and the durable local ACK. The JSON report
contains the tenant, control namespace, session/create IDs, exact receipt and
`retained_result`; it always reports `current_runtime_state: "not_observed"` and
`admission_available: false`. An `inspection_handle` is returned only for an
already-retained `created_held` candidate confirmed by the control. This is a
retained broker report, not an independent or current runtime observation, and
the handle grants no mutation permission.

An error exits `1`; invalid arguments exit `2`. An absent, busy, partial or
inconsistent journal requires authorized operator inspection; preserve any
existing files. After a transport,
result-audit, ACK or output failure, central confirmation may already be durable:
retry the same reconciliation with the same journal. Do not edit the candidate,
CAS or first refusal to obtain an ACK. A missing historical claim or a stale
candidate needs inspection of the exact control audit; this command supplies no
adoption, repair, cleanup or quota-release procedure.

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
cargo test --locked --lib runtime::kubernetes::prerequisites_tests:: -- --test-threads=1
cargo test --locked --lib runtime::kubernetes::admission::remote_coordinator_tests:: -- --test-threads=1
cargo test --locked --lib kubernetes_cli:: -- --test-threads=1
```

Select targets for the changed boundary rather than rerunning the whole list.
They exercise actual Rust HTTP/SQLite/private-journal paths and local TLS fixtures,
including mutual TLS and cancellation observed by the peer. Held-CREATE recovery
tests cover lost acknowledgements or an unretained response, a missing claim,
central cancellation, withdrawn policy and inactive supervision. The TLS peer
returns declared Pod facts and starts no worker. These tests do not qualify
Docker Desktop or another cluster. See the
[validation record](../../engineering/VALIDATION.md) for each run's source,
date and scope; earlier diagnostics do not qualify the preflight.

Worker admission still requires workspace transfer, exec/attach, mandatory
launch audit/start authorization, independently enforced deadlines, recovery
and real confinement tests. In particular, a NetworkPolicy object alone is not evidence that the
local CNI isolates workers or tenants. No cluster configuration is changed by
these diagnostics or preflight. Real Docker Desktop single-node qualification,
including its CNI/RBAC/admission behavior, remains `NOT_RUN`.
