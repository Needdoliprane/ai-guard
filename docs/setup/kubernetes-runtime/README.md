# Kubernetes V2 session runtime: status, recovery mechanics and Docker Desktop plan

[English](README.md) · [Français](README.fr.md) · [Kubernetes guide](../kubernetes/README.md)

**No Kubernetes session can be launched with this product.** There is no
`--runtime kubernetes` option or production activation. The R2 session CLI
prepares, inspects and abandons reservations before any runtime effect
([section 12](#12-r2-broker-http-and-pre-effect-cli)).
This guide describes the V2 session runtime code that exists in
the Rust sources, what an operator can do today, and the plan to qualify it
later on the single-node Kubernetes cluster of Docker Desktop. That plan has
**not been executed against a real cluster**. The [13 November evidence](../../engineering/evidence/2026-11-13/claude-composition/README.md) records 144 selected Kubernetes software parents passing on its exact source, with global RC99 and an explicit subcase gap. The [original record](../../engineering/evidence/2026-11-12/kubernetes-v2-runtime/README.md) retains its dated delivery status.
The binding contract is the [Kubernetes admission contract](../../engineering/KUBERNETES_ADMISSION.md).

The current source composes stage S2b: a preamble-only attach, its hardening,
and the worker waiting-mode dispatch ([section 11](#11-preamble-only-attach-and-worker-waiting-mode-stage-s2b)).
The [14 November targeted evidence](../../engineering/evidence/2026-11-14/kubernetes-attach-composition/README.md)
records build/check RC0 and 11 unique parents PASS on
`1e30e5e8546ab614eba7849789a1a0468cbc2843`: four real-stream contracts with
synthetic peers and seven real subprocess-stdio cases of the normal
Rust `ai-guard-provider` binary. Only its waiting/argument handling ran; no
AI client or model ran. The separate 121 PASS on
`be6b12dce7f74e2062f7ad5524e12818bf7313b5` remain historical and were not
rerun on `1e30e5e`. A later [composed inspection run on `45179db`](../../engineering/evidence/2026-11-14/owned-policy-inspection/README.md)
re-executes these four contracts and seven stdio cases within its 47 selected
parents. It does not rerun all 121 historical tests or qualify a cluster;
no counts are added across these sources. Only contract 1 uses an HTTPS upgrade to a
synthetic TLS loopback peer; contracts 2–4 use in-memory duplex streams, with
no TCP/TLS proof. No product session path reaches the attach.

## 1. Status by layer

| Layer | What exists | Who can call it today | Verified |
|---|---|---|---|
| A. Library entry with real inputs | `KubernetesRuntime::inspect_session_workspace_claim(name)`: DNS-label name checked before any request, then GET of the two namespace anchors, the PVC and the anchors again. An invalid name is reported as `InvalidAuthority` before any request, a claim that does not meet the requirements of section 3 as `InvalidResponse`, and an absent PVC as `ApiRejected(404)`. | Rust code holding a `KubernetesRuntime`. **No product binary calls it**; a read-only CLI entry, which would map the first two errors to dedicated messages, is only a proposed patch. | Historical selection PASS on its own source; wider mechanics and cluster not qualified by the 45179db targeted run |
| B. Pure mechanics (crate-internal) | Exact V2 Pod request renderer, admitted-response projection, exact observation parser, journal records. | Crate code only. | Historical selection PASS on its own source; wider mechanics and cluster not qualified by the 45179db targeted run |
| C. Before-effect lifecycle | Journal retention chain, live HTTP confirmation and reopen, abandonment before any effect, successor recovery and receipt replay. | R2 broker HTTP and `kubernetes session prepare`, `status`, `abandon` for schema11–13. Native Source17 broker port remains unavailable. The separate runtime reconciliation helper remains internal. | R2 bindings and CLI: NOT_RUN; historical mechanics selections remain attached to their own source |
| D. Effect mechanics (closed) | Held CREATE of the exact bytes, gate-release PATCH with JSON-Patch `test` operations, DELETE with a UID precondition; S2b preamble-only attach. | A sealed guard whose only implementation exists in tests and is bound to an owned loopback listener. No product path reaches an effect. | Four attach contracts PASS on 1e30e5e and again on 45179db; other selections remain source-specific; real effects and cluster NOT_RUN |
| S2b worker waiting mode | `ai-guard-provider` dispatches the fixed mode before the provider path. It exchanges Hello/Ready, then refuses any further byte. | The existing binary's waiting-mode entry only; no session launcher. | Seven normal-binary stdio cases PASS on 1e30e5e and again on 45179db; worker image and cluster NOT_RUN |
| E. Missing seams and qualification | Typed requests R1–R11 to the control owner; this Docker Desktop plan. | — | not started |

Effect permits, handoff, start, terminal observation and quota settlement
after an effect are **not implemented**. S2b implements only the preamble and
waiting mode in source. No Kubernetes runtime qualification or production
gate is closed by it.

The 13 November results cover only the selections in the [typed record](../../engineering/evidence/2026-11-13/claude-composition/TESTS.json), not every combination within each layer. Global RC99: one release other-UID subcase remains NOT_RUN. S5 product effects remain closed. Q1/Q6 were decided for the S2b preamble code only; they do not authorize activation.

## 2. What an operator can do today

- **Close a reservation that never issued an effect** with the existing Runner
  `control-recovery` route of the central control. It is documented in
  [Configure session reservation V2 policies](../kubernetes/README.md#configure-session-reservation-v2-policies)
  and in the [API contract](../../api.md#récupération-contrôle-des-réservations-kubernetes-v2--kubernetes-session-control-recovery-v2).
  This work does not change that route.
- **Inspect the Kubernetes authority** with the existing read-only
  `agentsandbox kubernetes doctor` (see the [Kubernetes guide](../kubernetes/README.md#run-the-read-only-cli-diagnostic)).
- Prepare/reopen, inspect central state and abandon a before-effect session
  reservation through the [R2 CLI](#12-r2-broker-http-and-pre-effect-cli).
  Workspace-claim inspection and the runtime reconcile report remain library
  mechanics; these commands perform no Kubernetes API request.

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
  (`kubernetes-session-wait-v2`, fixed by Root decision Q6), never a
  business invocation. It has no environment, Secret, token, host path or
  service-account token, and a scheduling gate.
- **CREATE response.** A Kubernetes HTTP `201` must satisfy the exact
  correspondence checks against the retained Pod request before a typed,
  non-duplicable target can be produced. HTTP acceptance is not an observed
  `Running` state or independent proof of Namespace identity, cluster identity
  or RBAC. Production execution and the native Source17 broker path remain
  unavailable.
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
- **Attach (preamble only, in source, closed):** Q1 accepts the bracketed
  argument for the preamble code only. Only the non-secret Hello travels
  before the second observation. This is neither an atomic precondition nor
  an identity proof; section 11 states its assumptions and limits. Every
  initiated attempt must be treated as final for the Pod. No product path
  reaches it.
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
- A worker image built locally from the composed waiting-mode source, with
  the bootstrap's ownership and executable mode inspected, referenced by
  digest and present in the node image store (`imagePullPolicy: Never`). The
  dispatch is now wired in source; an image build and its execution remain
  NOT_RUN. The earlier proposed image-ownership patch is not applied here.
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

Steps 4–12 stay `BLOCKED` until the missing Root requests R1–R7, S5 effect
permits and an explicit authorization exist. Q1 and Q6 are decided for the
S2b preamble code only. The future suite is: step 4 held CREATE, including the
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
  enforcement, the waiting mode in a worker image, any effect permit, the durable
  deadline, handoff and start, quota settlement after an effect, and scale to
  100 workers.
- Loopback test peers answer with declared fixtures; they never qualify a
  Kubernetes API server, a provisioner or `ReadWriteOncePod`.
- S2b's software ceilings, WebSocket interoperability, `stdinOnce`, worker
  signal handling and node-log retention remain unmeasured on a cluster.
  Seven process-stdio cases passed on `1e30e5e`; they exercised only the
  normal Rust adapter's waiting/argument handling. The same seven cases pass
  again in the separate `45179db` inspection composition; no wider runtime
  qualification follows.
- Gates G05, G07 and G11 stay open, G11 `BLOCKED`. The product is not
  production-ready, and the absence of an effect path is the barrier that keeps
  Kubernetes launch closed.

## 10. Open items cited in this guide

These identifiers come from the lot design and contract, which are kept
outside this repository. Q1 and Q6 are decided for stage S2b only; the other
items below remain open. This guide makes no new control decision.

- **Root**: the owner of the central control contracts and of the files
  shared between work lots. It decides the requests R1–R11 and the questions
  Q1–Q9.
- **R1**: central V2 effect states and their migration, including the rule
  that a claim UID is never reused and the central binding of the request
  digest and of the claim UID.
- **R2**: broker HTTP routes and live confirmation now have a pre-effect
  caller for schema11–13 in source (section 12, tests NOT_RUN). The native
  Source17 authenticated broker/transaction port remains to implement.
- **R3**: a durable deadline armed before a gate release, independent of the
  CLI.
- **R4**: the frames after the waiting-mode `Ready` (handoff, start, declared
  result).
- **R5**: the session storage policy for workspace claims.
- **R6**: changes outside the original lot, delivered as proposed patches.
  The waiting-mode binary dispatch is now composed; the proposed image
  ownership and read-only claim CLI changes are not applied here.
- **R7**: the production activation capability, which stays closed.
- **R8**: coordination with the next central schema version (information
  request).
- **R9**: confirmation that `pods/exec` stays out.
- **R10**: refusal, at configuration load, of two tenants that share a
  Kubernetes authority or namespace.
- **R11**: a successor confirmation to reopen a journal of another
  incarnation.
- **Q1** (decided for S2b): the bracketed argument is accepted for the
  preamble code only, under the assumptions of section 11. It proves no
  identity and grants no effect permission; the guard stays sealed.
- **Q6** (decided for S2b): the waiting mode is in `ai-guard-provider`, selected
  by the sole argument `kubernetes-session-wait-v2` before reading the provider
  environment. Any extra argument is refused with exit 64 without fallback.
- **S5**: effect permits and a production implementation of the sealed guard,
  with a durable checkpoint before each effect; blocked on R1–R3 and R7.
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

## 11. Preamble-only attach and worker waiting mode (stage S2b)

The [attach owner](../../../src/runtime/kubernetes/session_attach.rs),
[WebSocket adapter](../../../src/runtime/kubernetes/session_attach_codec.rs)
and [worker waiting mode](../../../src/runtime/kubernetes/session_worker.rs)
exist in source. The dispatch is wired into the existing
[`ai-guard-provider` binary](../../../src/bin/ai-guard-provider.rs).
Four real-stream contracts and seven normal-binary process-stdio cases passed
on `1e30e5e`; [the evidence](../../engineering/evidence/2026-11-14/kubernetes-attach-composition/README.md)
records their exact scope. The 121 historical PASS on `be6b12d` were not
rerun. The separate `45179db` inspection composition repeats these eleven
checks; its [own evidence](../../engineering/evidence/2026-11-14/owned-policy-inspection/README.md) does not enlarge their runtime scope.
No cluster, real worker or AI client/model was exercised. The effect guard has no production implementation;
there is no session attach command, activation option or operator procedure.

### 11.1 One attempt: Hello, Ready, observations, close

The attach carries only a bootstrap preamble, with no business data.

1. Verify the `kube-system` and tenant Namespace UIDs, active and not deleting.
2. Observe the retained exact Pod UID and spec digest, node, running worker
   container ID, start time and image ID, restart count zero and no deletion
   timestamp. The Pod phase alone is not used as proof.
3. Check the sealed guard, cancellation flag and absolute deadline before
   initiating a request.
4. Initiate one WebSocket upgrade to the Pod's attach subresource by name,
   using the same HTTPS endpoint, CA and credential as the observations.
   Redirects are refused. Validate the 101 response and the sole subprotocol
   `v5.channel.k8s.io`; extensions and fallback protocols are refused.
5. Send only Hello to stdin; accept one matching Ready on stdout. Both carry
   non-secret identifiers and digests. Any extra bootstrap byte on stdout is
   refused; the worker refuses any further stdin byte.
6. Observe the Pod again and compare every field of step 2; verify the
   Namespace anchors again. Local request-slot contention retries these
   reads within the same deadline, without sending a refused request.
7. End the stream and retain the local result, regardless of the verdict.

The normal request order is anchors, Pod, attach, Pod, anchors. Matching
observations allow the preamble to be reported as **declared metadata**.
Ready is an echo: it proves neither process identity nor execution. The local
attempt report is not the missing independent durable audit of product effects.

Q1 accepts this argument only for the preamble code. It assumes UIDs are
never reused, names are unique, the API server and kubelet are honest, an
attach stays bound to its container, and the second observation is a
most-recent read. None of those assumptions is verified here. Attach has no
atomic Pod-UID precondition: it addresses a name between two separate reads.
If an assumption fails, Hello can reach another process. No secret,
credential, business argv or workspace content travels before or after it.

### 11.2 Ownership, cancellation and endings

`stdinOnce: true` makes every **initiated** attempt final for the Pod, even
if no successful upgrade answer returns. There is no reconnect or second
attach. Recovery requires a new reservation, Pod and per-attempt workspace
claim. An attempt refused before initiation has not touched the Pod.

From HTTP initiation, one task owns the pending upgrade, then the stream and
its capacity slot. Losing the caller does not cancel the request or return
the slot: the owner waits for the answer or handshake deadline, then for the
stream's effective end. Without the caller's second observation the ending
has no identity. Runtime shutdown drops what the owner holds and records a
stopped-owner cause; dropping an owner is never success.

Cancellation is checked before initiation and polled about every 50 ms while
waiting for Ready or the second observation. This is a source constant,
not a measured latency. It does not interrupt a pending upgrade or a write
that the peer is not absorbing. Such a write waits until the deadline and
ends as `AttachDeadline`, before or after Ready; a cancellation requested
during it is kept separately as the cause. A possibly cut frame makes the
stream unusable: no further frame is sent.

The transport checks the absolute deadline synchronously after request
construction and guard validation, immediately before the initiation mark
and first HTTP poll. An expired request returns `AttachDeadline` with
`request_initiated=false`, without spending the Pod's single attach. The
new transport regression for this boundary is `NOT_RUN` on the current
source; the historical results above do not cover this change.

On a usable stream the owner closes stdin with the v5 message `[255, 0]`,
then sends a WebSocket close and shuts down the transport. Each step has its
own one-second limit and may run after the bracket deadline. After a
well-formed peer close, only the library's close reply is written; after a
transport failure or cut frame, only bounded shutdown remains. A malformed
close rejected by the library leaves the write side usable, so stdin close
and close code 1002 are still attempted. The slot returns once the owner has
ended. None of these endings is an observed terminal proof for the Pod.

| Result | Meaning |
|---|---|
| `AttachHandshakeRefused` | The initiated upgrade returned an unexpected status, headers, subprotocol or extension. No stream or frame. |
| `AttachProtocol` | A refused frame, channel, status shape or exceeded content bound; any extra bootstrap byte after Ready. |
| `AttachIdentityUnproven` | No two equal observations around an accepted Ready: mismatch, observation failure, caller loss, cancellation, peer end, status arrival or other bootstrap failure. A deadline after Ready with no write pending also has this result. |
| `AttachCapacity` | Both per-runtime attach slots are occupied. This does not cover an allocator abort inside the codec. |
| `AttachDeadline` | Invalid or elapsed deadline, upgrade or pre-Ready handshake timeout, or a stream write still pending at its deadline. |
| Existing pre-request/transport errors | A local precondition, guard or early cancellation can return `EffectRefusedBeforeSend`; observations can return `InstanceReplaced`, `ApiRejected` or `AuthorityChanged`. A lost upgrade answer can return `TransportUnavailable` or `Timeout`; the initiation mark still makes that attempt final. |

A peer close counts as a peer end only with an empty body or code 1000 or
1001. Any other code, malformed body or invalid UTF-8 reason is refused.
Transport EOF or failure is also a peer end. None proves identity or success.

### 11.3 Configured software bounds, not measured capacity

| Bound | Value |
|---|---|
| WebSocket frame and reassembled message | 64 KiB each, including the channel byte |
| One read / initial read buffer | 4 KiB; the frame buffer can grow |
| Write queue | One client frame, 4,111 bytes; no write batching |
| Worker stderr | 16 KiB counted then discarded by ai-guard |
| Status document | 4 KiB, projected to a closed status and reason; no peer text retained |
| Bootstrap envelope body | 16 KiB, length checked before allocation |
| Upgrade and preamble exchange | 5 s each, within the whole bracket |
| Whole bracket | At most 15 s, including the observations |
| Closing after the end | At most three steps of 1 s each |
| Concurrent preamble attempts | Two per runtime; not a capacity for 100 sessions |

Only binary messages on stdout, stderr and status are admitted. An empty
stdout payload is allowed; a message without a channel byte or on stdin,
resize, close or an unknown channel is refused. Status is reduced to
`Success`/`Failure` and a closed reason (`NonZeroExitCode`, `InternalError`,
other or none). A status arriving before the observation bracket completes
does not prove a completed preamble. Real API-server/kubelet interoperability,
deadlines, memory and throughput remain unmeasured.

### 11.4 Maintained WebSocket library and H21 compatibility

`tokio-tungstenite` **0.30.0** is pinned exactly, with default features
disabled, and uses `tungstenite` 0.30.0 (MIT / MIT-or-Apache-2.0 respectively).
It wraps an already upgraded stream in the client role. It performs no
handshake, connection, TLS or redirect here. ai-guard retains upgrade
validation, channel policy and authorization. Library errors become closed
values; their text is not formatted or retained.

H21 explicitly retains two compatibility cases: a four-byte payload announced
in the 16-bit length form, and a short Ready message announced in the 64-bit
form, are accepted as the same payload. **Non-minimal lengths are accepted**;
this is not strict minimal-length enforcement of RFC 6455. The 64 KiB
declared-frame and aggregate-message bounds still apply. A 64-bit length
with its top bit set, or a declared frame above the bound, is refused from
the header as too big.

Reserved opcodes and oversized declared frames are refused from the header.
Other library refusals (reserved bits, masked server frames, invalid
fragmentation or control frames) occur after the bounded frame payload is
read. Text is read before ai-guard refuses it. A peer that withholds such a
payload can consume the deadline. The fragment that crosses the aggregate
bound can coexist in memory with the message being assembled.

These are content bounds, **not an exact heap bound**. Library buffers grow,
and its infallible allocations can abort the process on exhaustion; there is
no typed recovery guarantee. A library panic stops the owner and returns its
slot with the stopped-owner cause. The library emits debug/trace `log`
records that can include frames and payloads. No logger is installed by this
crate today; a future global logger at those levels could receive Hello,
Ready and peer-controlled bytes. A build-time logging cap is not implemented.

### 11.5 Worker waiting mode and real standard streams

The rendered Pod command is the existing binary with one fixed argument:

```text
/usr/local/bin/ai-guard-provider kubernetes-session-wait-v2
```

This is the Pod's bootstrap command, not a session-launch procedure. Raw
arguments select it before text decoding and before reading the provider
handoff environment. Any additional argument, including invalid UTF-8, exits
64 without building a runtime, reading stdin or falling back. Other first
arguments retain the provider path; invalid UTF-8 there remains exit 126.

The worker waits without a local timeout before the first byte and after
Ready; the Pod's `activeDeadlineSeconds` is the runtime bound, not a durable
broker deadline. The first byte arms five seconds to complete Hello and
flush Ready. No program is executed and no subsequent frame is accepted.
Only Ready goes to stdout; a fixed diagnostic without identifiers or frame
contents goes to stderr.

| Exit | Waiting-mode end |
|---|---|
| 64 | Protocol refusal: invalid, oversized, truncated or unexpected frame; any byte after Ready; any argument after the mode. |
| 65 | Hello or flushed Ready did not complete within five seconds of the first byte. |
| 66 | EOF before Hello or after Ready, without handoff. Expected for a preamble-only exchange, never session success. |
| 70 | Local read/write, capacity or runtime failure. |
| 0 | Never returned. |

The source calls `shutdown_background` after the exchange: Tokio's blocking
stdin reader can still be parked when a handshake deadline expires; waiting
for runtime destruction would otherwise prevent the exit from arriving.
Seven tests passed on the real normal Rust binary and pipes at `1e30e5e`,
including a partial Hello with stdin still open when exit 65 was observed.
This executes the internal provider adapter, not an AI client/model. These
stdio tests exercise no worker image, attach transport or cluster; the four
separate stream-contract tests use synthetic peers: one HTTPS/TLS loopback
contract and three in-memory duplex contracts. Their
[source-specific evidence](../../engineering/evidence/2026-11-14/kubernetes-attach-composition/README.md)
is repeated by the separate [45179db evidence](../../engineering/evidence/2026-11-14/owned-policy-inspection/README.md), with the same software-only scope.

The mode reads no credential or environment itself. Constructing its Tokio
runtime can read Tokio's `TOKIO_UNSTABLE_SHARDED_BLOCKING_QUEUE` setting;
extra-argument refusal constructs none. The Pod request has no environment.
The mode installs no signal handler: PID-1 SIGTERM behaviour, the active
deadline and the rendered 30-second termination grace remain to be observed.

Ready repeats the Hello's identifiers and four digests on stdout. Node
runtime logs can retain this session metadata and stderr outside ai-guard;
this has not been observed on a cluster here. ai-guard does not mask, bound
or delete those node logs. No secret may travel on this channel, and output
protection before runtime persistence remains open.

### 11.6 Closed prerequisites

- **R4:** no handoff, Start or declared-result frames after Ready.
- **R7:** no production activation or proven exclusivity of attach/exec
  permissions in the tenant Namespace.
- **R10:** rejecting shared Kubernetes authority/Namespace between tenants at
  configuration load remains a prerequisite, not a delivered check.
- **S5 / sealed guard:** no production effect permit or product attach path.
- **Cluster qualification:** upgrade/channel interoperability, effective RBAC
  verbs (EXT-8), feature gates, `stdinOnce`, node logs, signal handling, CNI,
  storage, deadlines and 100-worker capacity remain BLOCKED. Plan step 6 stays
  BLOCKED; source and loopback results do not qualify a cluster.

## 12. R2 broker HTTP and pre-effect CLI

These commands prepare, inspect and abandon reservations before any runtime
effect, using the existing schema11–13 ledger. The native Source17 broker
path remains unavailable: it requires `AuthenticatedExecutionBroker`,
current grants/source/policy and their same-transaction revalidation, and is
refused before reading the HTTP body. A shared `runner` is not a substitute
broker or cloud identity. Production execution remains unavailable. None of the
commands below creates a Pod, releases a gate, attaches a stream or executes
an AI client.

The gateway must already have the current broker workload identity, its
Kubernetes grant, tenant `session_reservation_policy` and quota configured
([existing policies](../kubernetes/README.md#configure-session-reservation-v2-policies)).
`session_recovery_policy` is a separate installation opt-in for resuming a
retained abandonment with a successor incarnation. It comes from the tenant
gateway configuration; the client sends no policy. Without it recovery is
refused.

The [JSON profile consumed by the CLI](examples/session-control-v2.json)
contains public example values only. Replace endpoint, tenant, control
namespace, broker, incarnation and credential with the installation values.
The token stays in its designated private file; no secret goes in arguments.
Profile, intent and credentials must be unchanged private regular files
without links; paths are absolute. Verified HTTPS is required outside an IP
loopback endpoint. Omit `gateway_ca_file` for public roots; a private CA
supplies its public chain only. R2 needs no Kubernetes access or credential.

A closed V2 intent is supplied by the existing Rust
`KubernetesSessionProfileV2::build` and
`KubernetesSessionReservationIntentV2::new` constructors. The file preserves
the same IDs/hashes on every retry; this lot adds no interactive profile or
authorization generator. Do not put a prompt, credential or business argv
in it: it carries only their commitments defined by the contract.

```console
agentsandbox kubernetes session prepare --control-file /private/path/control-v2.json --intent-file /private/path/intent-v2.json --journal-dir /private/path/session-journal
agentsandbox kubernetes session status --control-file /private/path/control-v2.json --journal-dir /private/path/session-journal
agentsandbox kubernetes session abandon --control-file /private/path/control-v2.json --journal-dir /private/path/session-journal
```

`prepare` reserves the exact intent, requires a live HTTP ACK of `Reserved`
with an exact nonce and broker scope, then prepares or reopens the private
journal. It may create the requested journal directory; `status` and
`abandon` require an existing directory. A lost response leaves reception
unknown: repeat `prepare` with the same intent file. The Store replays the
durable operation without another reservation or quota allocation.

`abandon` retains a stable operation UUID in `abandoned-v2.json` before any
close POST. A cut ACK after commit is resumed with the same command and
journal; no replacement UUID is generated. A successor incarnation may
submit only an already retained abandonment authorized by central policy.
Without a retained abandonment, use authorized control recovery. Saturated
audit capacity refuses the mutation and keeps quota; a failure or timeout
releases nothing.

`status` separates local phase from central state. Every report carries
`execution_available=false` and `runtime_state="not_observed"`. Errors remain
errors, without invented empty state or zero. The ACK is verified HTTP
provenance to the control, not independent proof of its storage, NamespaceUID,
PVC, confinement or Kubernetes process identity. S5, R1/R3/R4/R5/R7/R10 and
cloud qualification remain open.

The R2 HTTP contract exposes these routes under `/api/v1/brokers/kubernetes/`:

| Method and suffix | Contract |
|---|---|
| GET `session-reservation-protocol` | Authenticated broker identity, schema, policy presence, effects unavailable |
| POST `session-reservations` | Exact V2 intent; 201 fresh, 200 exact replay |
| GET `session-reservations/{id}` | Descriptive reservation state for the authenticated broker |
| POST `session-reservations/{id}/confirmations` | Version2, `request_id` nonce, `reservation_input_hash`; ACK bound to the same reserved state and broker |
| POST `session-reservations/{id}/abandonments` | V2 `preparation_abandoned` close, expected revision1 and retained UUID; central CAS/audit/replay |

The tenant comes from authenticated identity before body and path. Bodies
and ACKs are limited to64/128KiB, closed JSON without duplicate members. Each
control request has a12s timeout, without redirects, ambient proxies or
automatic retries; incoming body reading is limited to5s. Secrets are refused
before persistence/distribution and transport errors stay opaque.
