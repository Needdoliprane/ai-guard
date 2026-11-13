# Kubernetes V2 session runtime: status, recovery mechanics and Docker Desktop plan

[English](README.md) · [Français](README.fr.md) · [Kubernetes guide](../kubernetes/README.md)

**No Kubernetes session can be launched with this product.** There is no
`--runtime kubernetes` option, no session CLI command and no production
activation. This guide describes the V2 session runtime code that exists in
the Rust sources, what an operator can do today, and the plan to qualify it
later on the single-node Kubernetes cluster of Docker Desktop. That plan has
**not been executed**. Every test of this work is `NOT_RUN` until it runs in an
admitted isolated QA environment; see the
[evidence record](../../engineering/evidence/2026-11-12/kubernetes-v2-runtime/README.md).
The binding contract is the [Kubernetes admission contract](../../engineering/KUBERNETES_ADMISSION.md).

## 1. Status by layer

| Layer | What exists | Who can call it today | Verified |
|---|---|---|---|
| A. Library entry with real inputs | `KubernetesRuntime::inspect_session_workspace_claim(name)`: DNS-label name checked before any request, then GET of the two namespace anchors, the PVC and the anchors again. An invalid name is reported as `InvalidAuthority` before any request, a claim that does not meet the requirements of section 3 as `InvalidResponse`, and an absent PVC as `ApiRejected(404)`. | Rust code holding a `KubernetesRuntime`. **No product binary calls it**; a read-only CLI entry, which would map the first two errors to dedicated messages, is only a proposed patch. | `NOT_RUN` |
| B. Pure mechanics (crate-internal) | Exact V2 Pod request renderer, admitted-response projection, exact observation parser, journal records. | Crate code only. | `NOT_RUN` |
| C. Before-effect lifecycle (crate-internal) | Journal retention chain, observe-only reopen, abandonment before any effect, successor recovery, receipt replay, observe-only reconcile report. | **Nobody in production.** The gateway Store issues the confirmations while the broker keeps the journal; the broker routes that would connect them do not exist yet (Root request R2). | `NOT_RUN` |
| D. Effect mechanics (closed) | Held CREATE of the exact bytes, gate-release PATCH with JSON-Patch `test` operations, DELETE with a UID precondition. | A sealed guard whose only implementation exists in tests and is bound to an owned loopback listener. No product path reaches an effect. | `NOT_RUN` |
| E. Missing seams and qualification | Typed requests R1–R11 to the control owner; this Docker Desktop plan. | — | not started |

Attach to a Pod, the worker waiting mode, effect permits, handoff, start,
terminal observation and quota settlement after an effect are **not
implemented**. Nothing in this guide is qualified, and no production gate is
closed by it.

## 2. What an operator can do today

- **Close a reservation that never issued an effect** with the existing Runner
  `control-recovery` route of the central control. It is documented in
  [Configure session reservation V2 policies](../kubernetes/README.md#configure-session-reservation-v2-policies)
  and in the [API contract](../../api.md#récupération-contrôle-des-réservations-kubernetes-v2--kubernetes-session-control-recovery-v2).
  This work does not change that route.
- **Inspect the Kubernetes authority** with the existing read-only
  `agentsandbox kubernetes doctor` (see the [Kubernetes guide](../kubernetes/README.md#run-the-read-only-cli-diagnostic)).
- Abandonment, successor recovery, workspace-claim inspection and the
  reconcile report are **library mechanics awaiting R2**. They are not
  operator procedures, and no command is documented for them.

Do not edit, rename or delete files in a V2 journal directory to "unblock" a
session. The journal is evidence; a refusal means reconciliation is needed.

## 3. Concepts

- **Reservation.** The central tenant Store holds a V2 reservation in state
  `Reserved` (quota held, effect fence `NeverIssued`) or `ClosedBeforeEffect`
  (quota released). There is no central state for an issued effect yet
  (Root request R1).
- **Preparation journal.** A private broker directory that retains, from a real
  central confirmation, the exact reservation intent and its audit position.
  Reopening needs a new real confirmation of the same reserved state.
- **Per-attempt workspace claim.** The workspace is an operator-provisioned
  PersistentVolumeClaim named `ai-guard-ws-<attempt id, 32 hex digits>`. It must
  be `Bound`, use `ReadWriteOncePod`, be a filesystem volume, have no data
  source and not be deleting. A claim is single-use: it belongs to one attempt,
  and reuse of a claim UID by another reservation is refused once the central
  contract exists (R1). The first retained observation stays authoritative:
  a later reading of the same PVC with a new `resourceVersion` is not retained
  again. The request is rendered from the retained claim, and offering a newer
  reading of the same PVC is a `Conflict`.
- **Exact request.** The Pod request is rendered deterministically from the
  confirmed intent, the preparation digest and the retained claim. Its command
  is the waiting mode of the existing `ai-guard-provider` binary
  (`kubernetes-session-wait-v2`, spelling pending Root decision Q6), never a
  business invocation. It has no environment, Secret, token, host path or
  service-account token, and a scheduling gate.
- **Effect chain.** CREATE (held behind the gate), release, attach, start and
  stop each need a separate central proof that does not exist yet. "Allowed to
  create a Pod" never means "allowed to start it".

## 4. Journal inventory r2

| File | Bound | Content | Requires |
|---|---|---|---|
| `.session-v2.lock` | 0 bytes | writer lock (`flock`) | — |
| `prepared-v2.json` | 96 KiB | confirmed intent, commitments, central audit position | lock |
| `workspace-v2.json` | 8 KiB | the claim retained for this attempt | preparation |
| `pod-v2.json` | 32 KiB | exact request bytes; valid only if re-rendering reproduces them | workspace claim |
| `abandoned-v2.json` | 4 KiB | the retained before-effect close (`preparation_abandoned`, revision 1) | preparation |

Rules:
- Every file is created exclusively and synced; nothing is ever overwritten,
  renamed or removed by the product. An unknown name, including a future
  effect checkpoint, makes the journal `Corrupt`, and it is preserved.
- A live owner of a prepared journal makes every other opener `Busy`, never
  `Corrupt`. A lock without `prepared-v2.json` is judged without taking its
  lock: during another process's creation it is a transient `Corrupt`; left
  alone it stays `Corrupt` and is never repaired.
- An observe-only reading creates no file and releases the lock before any
  network request. An empty directory is not a journal. While it copies the
  files it holds the lock, so a concurrent reopen by the owner can be
  transiently `Busy`; retry.
- Each record is checked against its predecessors and the renderer: a record
  for another attempt or another preparation, or a request that re-rendering
  does not reproduce, is `Corrupt` and preserved. The journal cannot
  authenticate a record rewritten by the same operating-system user; for
  example the claim UID in `workspace-v2.json` is not carried by the request
  bytes, which bind the claim by name only.
- After `abandoned-v2.json`, no claim or request is retained. A retained close
  is a local fence, not a central close: the quota stays held until the Store
  commits the close.
- An older binary refuses a journal that contains the newer names (`Corrupt`).
  There is no migration or rewrite of a journal.

## 5. Abandonment before any effect and successor recovery (library mechanics)

Abandonment first retains the close locally with a stable operation id, then
revalidates the whole journal and calls the existing Store API. The incarnation
given by the caller only selects which existing authorization is tried; the
Store re-checks the authenticated broker, tenant, namespace and history.

For a successor, the Store verifies only that the recovery policy it is given
approves the authenticated broker, and records the policy hash. **The recovery
policy is trusted input:** the Store does not compare it with any
configuration. A production caller (R2) must take it from the tenant's
`session_recovery_policy` in the gateway configuration, never from a broker
request, a journal file or a worker.

| Outcome | Meaning | Central effect |
|---|---|---|
| `Closed` | This call committed the before-effect close. | `ClosedBeforeEffect` revision 2; the quota is released by the committed compare-and-set. |
| `Replayed` | The same close was already committed; the historical receipt is returned (same incarnation, or successor receipt lookup). | none, no new event |
| `SuccessorRequiresRecoveryPolicy` | The journal belongs to another incarnation and no tenant recovery policy was given. | no Store call; nothing retained |
| `NoRetainedAbandonment` | The observe-only view has no retained close. A successor cannot retain one today (R11); use the Runner route. | no Store call |
| `ClosedCentrallyByAnotherOperation` | Same incarnation: one observation shows the reservation closed by another operation, whose id is reported. | already closed by that operation |
| `SuccessorLookupNotFound` | Successor: the Store knows neither the reservation nor the retained operation. A successor cannot observe a closed reservation, so this stays ambiguous. | none |

An effect checkpoint name or a replaced record refuses with `Corrupt` before any
Store call. Another broker of the same Store is refused by the Store
(`PermissionDenied`). A retry after a lost response is safe: the operation id
is retained. A timeout, an absent Pod or a missing answer never releases quota.

## 6. Observe-only reconcile report (library mechanics)

The report reads one journal observe-only, refuses another Kubernetes authority
before any request (`InvalidContext`), and inspects the session names of a
retained request with GET requests only (anchors, Pod, PVC, anchors). It makes
no Store call, mutation, adoption or quota change. A journal without a retained
request issues no request.

| Interpretation | Meaning |
|---|---|
| `NoRequestRetained` | No exact request and no close are retained, so the session names were **not inspected**: whether an object exists there is unknown. |
| `NoEffectIssuedLocally` | The exact request is retained and its names were inspected: nothing at the Pod name, and the retained claim UID, not being deleted, at the claim name. This directory holds no effect checkpoint. Another directory or process could exist for the same reservation; the central fence stays authoritative. |
| `ObjectPresentAtSessionName` | An object exists under the derived Pod name. Its ownership is unknown; nothing is adopted. |
| `WorkspaceClaimChanged` | The PVC at the claim name is absent, has another UID than the retained claim, or is being deleted. |
| `AbandonmentRetained` | A close is retained locally; resume the abandonment. The central state is not observed by the report. |

The `names` field of the report is present only when the names were
inspected; otherwise it is absent from the serialized report (not `null`).

## 7. Security model

- **What is not an identity:** a label, a Pod name, a nonce echo, a re-read
  record, a boolean or a declared UID. A Pod observation binds the exact UID,
  spec digest and container identity; another UID at the same name is another
  instance.
- **Attach (proposal only, not implemented):** a preamble-only attach bracketed
  by two exact observations, sending only the non-secret Hello before the second
  check. It is not an atomic precondition and rests on stated assumptions (UID
  never reused, name uniqueness, honest API server and kubelet, most-recent GET).
  Every attach would end the worker. It waits for Root decision Q1.
- **Node logs persist worker output**, including preamble metadata. No secret
  may travel from the worker to the broker.
- **Storage:** one `ReadWriteOncePod` claim per attempt; no host path in the Pod
  spec. On Docker Desktop the default provisioner is expected to back each
  volume with a directory on the node host filesystem (to verify, EXT-10b), so
  workspace data sits on the host. `ReadWriteOncePod` and the observed capacity
  are not byte or inode enforcement.
- **Network:** the V2 profile declares no network; `dnsPolicy: None` with
  `127.0.0.1` is not enforcement. This work creates and verifies no
  NetworkPolicy: without an enforced default-deny ingress and egress policy in
  the tenant Namespace, a released Pod can reach any address, including
  cluster services, the node, other tenants and metadata endpoints.
  Enforcement stays unproven (G05/G07/G11), and plan step 5 stays `BLOCKED`
  until the observed CNI enforcement probe of plan step 4a passes.
- **Credentials:** the broker keeps the Kubernetes credentials; the Pod has no
  token automount, environment, Secret or argv secret.
- **Future broker permissions** (plan step 1 below, not provisioned by this
  work): get on the two Namespaces, get on PersistentVolumeClaims, get/create/patch/delete on
  Pods in the tenant Namespace, and the attach permission whose verb must be
  confirmed (EXT-8). No exec, logs or Secrets. RBAC cannot restrict create by
  object name or spec.
- **Tenancy:** the tenant comes only from the authenticated identity. Refusing
  two tenants that share an authority or a namespace at configuration load
  (R10) is a prerequisite of any attach.

## 8. Future Docker Desktop single-node qualification plan (not executed)

Nothing below has been run. **Every step that changes the cluster needs an
explicit maintainer authorization**, which this work does not grant. Each step
records its oracle and evidence, and stops at the first discriminating oracle,
refusal or ceiling.

### 8.1 Prerequisites to record

- Host OS, version and architecture (for example macOS ARM64); Docker Desktop
  version with Kubernetes enabled.
- `kubectl config current-context` must print `docker-desktop`; record the
  `kubectl version` client and keep it within the official
  [version skew policy](https://kubernetes.io/releases/version-skew-policy/) (EXT-13).
- Server `gitVersion` from `agentsandbox kubernetes doctor`. The local
  inspection profile pins Pod Security `v1.36`
  ([profile](../../../deploy/kubernetes/local-inspection.json)); any other server
  version needs the pins reviewed first, and activation stays `BLOCKED`.
- The CNI plugin and its version, and whether it enforces NetworkPolicy:
  facts to verify against official sources, never assumed. A documented
  enforcement is not an observation; plan step 4a observes it. Without
  enforcement, steps 5–12 stay `BLOCKED`.
- A worker image built locally with the proposed patches 0001 (waiting mode) and
  0002 (provider ownership) applied, referenced by digest and present in the
  node image store (`imagePullPolicy: Never`).
- `agentsandbox` built only in the admitted isolated QA environment.

### 8.2 Storage prerequisites

- A StorageClass whose provisioner supports `ReadWriteOncePod`. The Kubernetes
  [access modes documentation](https://kubernetes.io/docs/concepts/storage/persistent-volumes/#access-modes)
  is expected to restrict it to CSI volumes (EXT-10a, to verify for v1.36).
  Docker Desktop's default `hostpath` class is expected to use a non-CSI
  provisioner (EXT-10b, to verify), so step 3 is likely `BLOCKED` on the
  default class.
- [`volumeBindingMode`](https://kubernetes.io/docs/concepts/storage/storage-classes/#volume-binding-mode)
  must be `Immediate`. With `WaitForFirstConsumer`, a claim is never `Bound`
  before the request is rendered, so the plan records `BLOCKED`. Binding after
  CREATE would change the request commitment and is Root question Q9.

### 8.3 Preparation (operator mutations, authorization required) and read-only product checks

Plan steps are numbered 0 to 12, plus step 4a, which must pass before step 5;
they are unrelated to the decisions Q1–Q9 of section 10. Steps 0 and 2 are read-only;
steps 1 and 3 change the cluster, and each requires an explicit maintainer
authorization.

| Step | Action | Oracle and evidence | Stop |
|---|---|---|---|
| 0 | Record the environment of 8.1. | Versions and context retained. | Context other than `docker-desktop`. |
| 1 | (Requires an explicit maintainer authorization, not granted by this work.) Provision the tenant Namespace with Pod Security `restricted` `v1.36`, a default-deny ingress and egress NetworkPolicy, a ServiceAccount without token automount and a Role with the permissions of section 7, from a template reviewed at that time. `delete` is granted only for the later stop step. | Created objects and their UIDs retained. A created NetworkPolicy is not proof of enforcement (step 4a). | Any partial creation: inspect, never delete and recreate. |
| 2 | `agentsandbox kubernetes doctor --check-permissions` with the broker identity. | Report retained. Its twelve fixed access reviews include `patch pods`, `update pods`, `delete pods` and `create pods/attach`; whether `create` is the verb checked for a WebSocket attach upgrade is still to confirm (EXT-8). `delete pods` must stay denied until the stop step; `update pods`, exec, logs, Secrets, bindings and token requests must be denied. | Any refusal of the reviews, or any of those denied permissions allowed. |
| 3 | (Requires an explicit maintainer authorization, not granted by this work.) The operator creates one `ReadWriteOncePod` PVC named for an attempt; `inspect_session_workspace_claim` returns it `Bound` with its UID. This needs proposed patch 0004; the admitted QA profile has no network (`--network none`), so a QA run against a cluster would need a separately admitted network profile. | Claim projection retained. | Class not RWOP-capable or `WaitForFirstConsumer`: `BLOCKED`. |

### 8.4 Effect suite (all BLOCKED)

Steps 4–12 stay `BLOCKED` until the Root requests R1–R7, the decisions Q1 and
Q6 and an explicit authorization exist: step 4 held CREATE, including the
check that the admitted 201 passes the closed admitted-response projection
(omitted false booleans and quantity forms, EXT-14); step 5 release with
preconditions and a stale-`resourceVersion` refusal, which also needs step 4a;
step 6 preamble-only attach with worker exit 66 observed; step 7 replaced-Pod
and namespace-recreation detection; step 8 a second attacher and gate removal
by another identity; step 9 network confirmation from the released session
Pod (DNS, direct IP, IPv6, tunnel; gates G05/G07/G11); step 10 stop with a UID
precondition and terminal observation; step 11 quota settlement; step 12
exact cleanup of Pod and PVC by UID.

Step 4a observes CNI enforcement before any release. It uses no product
effect path: it needs step 1 (its policy), not the Root requests and decisions
above. It stays `BLOCKED` until its own explicit maintainer authorization
exists, separate from those of steps 1 and 4, and it must pass before step 5.

| Step | Action | Oracle and evidence | Stop |
|---|---|---|---|
| 4a | (Requires its own explicit maintainer authorization, not granted by this work.) The operator, not the broker identity, creates two non-session Pods from the same manifest and image, reviewed at that time, which differ only in name and Namespace. Neither has a service-account token or session content, and both are named outside the `ai-guard-session-` prefix: a probe Pod in the tenant Namespace, under the default-deny policy of step 1, and a baseline Pod in a separate baseline Namespace with the same Pod Security level and no NetworkPolicy. From both Pods it probes the targets listed below, with the same tool, protocol, port and timeout. Removing both Pods and the baseline Namespace by their UIDs, once the evidence is retained, belongs to the same authorization request. | A target is discriminated only if the baseline Pod reaches it. Step 4a passes when every target is refused from the probe Pod, the positive controls succeed, and the DNS (53/UDP and 53/TCP), `kubernetes` Service, node and IPv4 targets are all discriminated. A target that the baseline Pod does not reach is recorded as not discriminated, never as evidence of enforcement. The evidence listed below is retained. | Any target reached from the probe Pod: the policy is not enforced for that destination, and steps 5–12 stay `BLOCKED`. A failed positive control, or a required target not discriminated: step 4a has not passed, and steps 5–12 stay `BLOCKED`. Any partial creation: inspect, never delete and recreate. |

Step 4a targets, probes and evidence:

- **Targets**, each with a listener known before the step: the cluster DNS
  Service IP (usually the `kube-dns` Service, to confirm on the cluster) on
  53/UDP and 53/TCP; the `kubernetes` Service IP on 443/TCP; the node
  InternalIP on one port with a known listener; one IPv4 and one IPv6 address
  of a listener outside the tenant Namespace, chosen and reviewed at that time;
  and `169.254.169.254` on 80/TCP. The address, port and protocol of each
  target are recorded. Any object created for such a listener belongs to the
  same authorization request and is removed by its UID. A cluster without an
  IPv6 path, or without a listener at `169.254.169.254`, leaves that target
  not discriminated: the fact is recorded, and step 9 still covers it.
- **Baseline.** Each target is probed from the baseline Pod, under the same
  authorization request and close in time to the attempt from the probe Pod.
  Without a recorded baseline, a target is not discriminated, and its failure
  is never evidence of enforcement.
- **Probes** are a connection or a handshake only. A TCP probe opens a
  connection and closes it without sending application data: no request path,
  header or body, and never a request to a metadata credentials path at
  `169.254.169.254`. The 53/UDP probe, which has no connection, sends one DNS
  query for a fixed name reviewed at that time. Each attempt records its
  result: connection established or answer received, refusal, or timeout.
- **Positive controls.** Inside the probe Pod, the same tool probes a listener
  of that Pod with each protocol that the probes use. A control only shows
  that the tool works, never that a target is reachable. Whether a
  NetworkPolicy applies to the loopback traffic of a Pod to itself, or to
  traffic between a Pod and its node, is a fact to verify against the official
  [NetworkPolicy documentation](https://kubernetes.io/docs/concepts/services-networking/network-policies/),
  never assumed; whatever that documentation states, a target reached from the
  probe Pod is a stop. Pod Security `restricted` is expected to require
  dropping every capability, `NET_RAW` included (to verify against the
  official [Pod Security Standards](https://kubernetes.io/docs/concepts/security/pod-security-standards/)),
  so no probe or control uses ICMP.
- **Evidence.** Retained: the UIDs of the tenant and baseline Namespaces; the
  list of NetworkPolicies of each Namespace, with the UID and
  `resourceVersion` of each policy; both Pod UIDs; each command and its
  output. Every output is reviewed before it is retained, and any credential,
  token or other secret-like content is masked; the record names each masking.

`BLOCKED` without a further decision: an independent cluster audit, CNI
enforcement and actor exclusivity. Step 5 depends on step 4a: it stays
`BLOCKED` until step 4a has passed, because a created NetworkPolicy alone does
not allow a release. Just before the release, the operator re-reads the tenant
Namespace and its NetworkPolicies, read-only; step 5 refuses, and step 4a is
re-run under a new authorization, if the Namespace UID, the list of
NetworkPolicies or any of their UIDs or `resourceVersion` values differs from
what step 4a retained. That re-read is not an atomic precondition of the
release; actor exclusivity stays `BLOCKED`. Step 9 stays the confirmation from
the released session Pod itself, including for the targets that step 4a left
not discriminated.

## 9. Limits and open gates

- Not qualified: any real Kubernetes behaviour (CREATE, gate, patch, attach,
  delete, Pod Security, RBAC verbs, `ReadWriteOncePod`, omitted-boolean encoding,
  quantity forms, log persistence), Docker Desktop, NetworkPolicy, CSI and cgroup
  enforcement, the waiting mode in an image, any effect permit, the durable
  deadline, handoff and start, quota settlement after an effect, and scale to
  100 workers.
- Loopback test peers answer with declared fixtures; they never qualify a
  Kubernetes API server, a provisioner or `ReadWriteOncePod`.
- Gates G05, G07 and G11 stay open, G11 `BLOCKED`. The product is not
  production-ready, and the absence of an effect path is the barrier that keeps
  Kubernetes launch closed.

## 10. Open items cited in this guide

These identifiers come from the lot design and contract, which are kept
outside this repository. Every item is open: this work decides or delivers
none of them.

- **Root**: the owner of the central control contracts and of the files
  shared between work lots. It decides the requests R1–R11 and the questions
  Q1–Q9.
- **R1**: central V2 effect states and their migration, including the rule
  that a claim UID is never reused and the central binding of the request
  digest and of the claim UID.
- **R2**: broker HTTP routes and a live confirmation received over HTTP; they
  would give the lifecycle mechanics a production caller.
- **R3**: a durable deadline armed before a gate release, independent of the
  CLI.
- **R4**: the frames after the waiting-mode `Ready` (handoff, start, declared
  result).
- **R5**: the session storage policy for workspace claims.
- **R6**: changes to files outside this lot, delivered as proposed patches
  only.
- **R7**: the production activation capability, which stays closed.
- **R8**: coordination with the next central schema version (information
  request).
- **R9**: confirmation that `pods/exec` stays out.
- **R10**: refusal, at configuration load, of two tenants that share a
  Kubernetes authority or namespace.
- **R11**: a successor confirmation to reopen a journal of another
  incarnation.
- **Q1**: whether a preamble-only attach bracketed by two exact observations
  is an acceptable attach identity argument.
- **Q6**: the spelling and entry point of the worker waiting mode
  (`kubernetes-session-wait-v2`).
- **Q7**: the per-attempt claim name rule and the never-reuse rule for claim
  UIDs.
- **Q9**: what to do when the only StorageClass uses `WaitForFirstConsumer`.
- **EXT-8**: the RBAC verb checked for the WebSocket attach upgrade.
- **EXT-10a/b**: `ReadWriteOncePod` restricted to CSI volumes; the provisioner
  type of Docker Desktop's default `hostpath` class and its node directory
  backing.
- **EXT-13**: the kubectl version skew policy.
- **EXT-14**: how Kubernetes v1.36 encodes omitted false booleans and
  canonical quantities in an admitted Pod.
