# Docker Desktop recipe for the Kubernetes session adapter (local test)

[Version française](README.fr.md)

This is a **local test recipe** for qualifying ai-guard's Kubernetes adapter
on the Kubernetes cluster built into Docker Desktop. **It is not a production
deployment.** The storage it installs is local test storage without byte
enforcement. The client operates its production clusters and chooses its
production storage, network plugin and identities.

Applying this recipe qualifies nothing by itself. The qualification steps, their
oracles and their status are in the
[Kubernetes runtime guide](../../../docs/setup/kubernetes-runtime/README.md)
(security model, Docker Desktop plan and open items). The authority file and the
`doctor` diagnostic are described in the
[Kubernetes guide](../../../docs/setup/kubernetes/README.md#run-the-read-only-cli-diagnostic).

Within that plan, this recipe provides the tenant objects of plan step 1 and
the storage prerequisite of section 8.2. It runs neither the CNI enforcement
probe of step 4a nor any later step.

Run every command from the repository root, against the `docker-desktop`
context only, with the Docker Desktop administrator identity. Every cluster
change below is an operator action.

## Contents

| Directory | Objects |
|---|---|
| [`csi-hostpath/`](csi-hostpath/) | Namespace `ai-guard-csi-hostpath` (Pod Security `privileged`), CSIDriver `hostpath.csi.k8s.io`, the `csi-hostpathplugin` StatefulSet (containers `hostpath`, `node-driver-registrar`, `liveness-probe`, `csi-attacher`, `csi-provisioner`), its ServiceAccount, sidecar ClusterRoles, leader-election Roles and bindings. Vendored from upstream with the changes listed in [PROVENANCE.md](csi-hostpath/PROVENANCE.md). |
| [`storage/`](storage/) | StorageClass `ai-guard-rwop`: provisioner `hostpath.csi.k8s.io`, `Immediate`, `Delete`, no expansion, never the default class. |
| [`control/`](control/) | Namespace `ai-guard-control` (Pod Security `restricted` `v1.36`; no Pod ever runs there) and the broker ServiceAccounts `broker-tenant-a` and `broker-tenant-b`, without token automount. |
| [`tenant/base/`](tenant/base/) | Shared tenant objects: Namespace with Pod Security `restricted` `v1.36` (enforce, audit, warn), ServiceAccount `default` without token automount, NetworkPolicy `ai-guard-default-deny` (all Pods, Ingress and Egress, no rule) and Role `ai-guard-session-broker`. Never applied alone. |
| [`tenant/tenant-a/`](tenant/tenant-a/), [`tenant/tenant-b/`](tenant/tenant-b/) | Namespace `ai-guard-tenant-a` or `ai-guard-tenant-b`, the RoleBinding to that tenant's broker, and the ClusterRole and ClusterRoleBinding `ai-guard-session-broker-tenant-<x>`. |

The images of the CSI driver are pinned by digest; upstream `deploy.sh`, the
socat testing Pod and its NodePort Service, the snapshot class and the
snapshotter and resizer sidecars are not included.

### Rights of a tenant broker

`broker-tenant-a` receives exactly the rights below, and no right in
`ai-guard-tenant-b`; `broker-tenant-b` mirrors them for tenant B.

| Scope | Resource | Verbs |
|---|---|---|
| `ai-guard-tenant-a` | `pods` | `get`, `create`, `patch`, `delete` |
| `ai-guard-tenant-a` | `pods/attach` | `create` |
| `ai-guard-tenant-a` | `persistentvolumeclaims` | `get`, `create`, `delete` |
| `ai-guard-tenant-a` | `networkpolicies.networking.k8s.io` | `get`, `list` |
| `ai-guard-tenant-a` | `serviceaccounts` named `default` | `get` |
| Cluster | `namespaces` named `kube-system` and `ai-guard-tenant-a` | `get` |
| Cluster | `storageclasses.storage.k8s.io` named `ai-guard-rwop` | `get` |
| Cluster | `csidrivers.storage.k8s.io` named `hostpath.csi.k8s.io` | `get` |

No exec, logs, Secrets, `update`, `list` or `watch` of Pods, ephemeral
containers, port-forward, binding or token request is granted. Kubernetes also
grants every authenticated identity its default discovery and self-review
rights (for example `selfsubjectaccessreviews`), which ai-guard uses for its
permission checks. RBAC cannot restrict `create` by object name or content;
Pod Security `restricted` and ai-guard's closed requests are the other
barriers.

## 1. Prerequisites and records

Record these facts before any change and keep them with the evidence:

```sh
kubectl config current-context
kubectl --context docker-desktop version
kubectl --context docker-desktop get nodes -o wide
docker version --format '{{.Server.Platform.Name}}'
kubectl --context docker-desktop -n kube-system get daemonsets -o wide
kubectl --context docker-desktop get storageclasses
kubectl --context docker-desktop get csidrivers
```

- The first command must print `docker-desktop`. Stop on any other context.
- The recipe pins Pod Security `v1.36` and was rendered with kubectl v1.36.1
  (Kustomize v5.8.1). Another server version needs the pins reviewed first.
- Record the CNI plugin from the DaemonSet list. Whether it enforces
  NetworkPolicy is observed later (plan step 4a), never assumed.
- None of these names may already exist: Namespaces `ai-guard-csi-hostpath`,
  `ai-guard-control`, `ai-guard-tenant-a` and `ai-guard-tenant-b`; CSIDriver
  `hostpath.csi.k8s.io`; StorageClass `ai-guard-rwop`; ClusterRoles
  `external-provisioner-runner`, `external-attacher-runner` and
  `ai-guard-session-broker-tenant-a`/`-b`. Apply would merge into an existing
  object, so stop if one exists:

  ```sh
  kubectl --context docker-desktop get namespace ai-guard-csi-hostpath ai-guard-control ai-guard-tenant-a ai-guard-tenant-b
  kubectl --context docker-desktop get csidriver hostpath.csi.k8s.io
  kubectl --context docker-desktop get storageclass ai-guard-rwop
  kubectl --context docker-desktop get clusterrole external-provisioner-runner external-attacher-runner ai-guard-session-broker-tenant-a ai-guard-session-broker-tenant-b
  ```

  Each command must report `NotFound` for every name.
- The node pulls the five CSI images from `registry.k8s.io` by digest, so the
  Docker Desktop VM needs DNS resolution and HTTPS access to that registry.
  Offline installation is not covered by this recipe.

## 2. Render and review

Render every directory and read the output before applying it:

```sh
kubectl kustomize deploy/kubernetes/docker-desktop/csi-hostpath
kubectl kustomize deploy/kubernetes/docker-desktop/storage
kubectl kustomize deploy/kubernetes/docker-desktop/control
kubectl kustomize deploy/kubernetes/docker-desktop/tenant/tenant-a
kubectl kustomize deploy/kubernetes/docker-desktop/tenant/tenant-b
shasum -a 256 deploy/kubernetes/docker-desktop/csi-hostpath/csi-hostpath-driverinfo.yaml
```

Expected: every `image:` line of the CSI rendering ends with
`@sha256:<digest>` from the table in [PROVENANCE.md](csi-hostpath/PROVENANCE.md);
no tenant rendering contains `ai-guard-tenant-unset`; the driverinfo hash is
`464307265464b77189bd4b339774cd2de69589458a62a740028b248f248c7e9a`.
Rendering contacts no cluster.

## 3. Install

Apply in this order:

```sh
kubectl --context docker-desktop apply --server-side --field-manager=ai-guard-docker-desktop-recipe -k deploy/kubernetes/docker-desktop/csi-hostpath
kubectl --context docker-desktop -n ai-guard-csi-hostpath rollout status statefulset/csi-hostpathplugin --timeout=300s
kubectl --context docker-desktop apply --server-side --field-manager=ai-guard-docker-desktop-recipe -k deploy/kubernetes/docker-desktop/storage
kubectl --context docker-desktop apply --server-side --field-manager=ai-guard-docker-desktop-recipe -k deploy/kubernetes/docker-desktop/control
kubectl --context docker-desktop apply --server-side --field-manager=ai-guard-docker-desktop-recipe -k deploy/kubernetes/docker-desktop/tenant/tenant-a
kubectl --context docker-desktop apply --server-side --field-manager=ai-guard-docker-desktop-recipe -k deploy/kubernetes/docker-desktop/tenant/tenant-b
```

Server-side apply creates or updates each object in one request. It avoids a
race with the controller that creates the `default` ServiceAccount of a new
Namespace. A multi-object apply can partially succeed: keep its output, inspect
the objects it names and re-run the same command after fixing the cause. Never
delete and recreate a Namespace to clear an error.

## 4. Verify

Each check states its expected result. Stop at the first difference.

### 4.1 CSI driver

```sh
kubectl --context docker-desktop get csidriver hostpath.csi.k8s.io -o jsonpath='{.spec.attachRequired} {.spec.fsGroupPolicy} {.spec.podInfoOnMount}{"\n"}'
kubectl --context docker-desktop get csinode desktop-control-plane -o jsonpath='{.spec.drivers[*].name}{"\n"}'
kubectl --context docker-desktop -n ai-guard-csi-hostpath get pod csi-hostpathplugin-0 -o jsonpath='{.status.phase}{"\n"}{range .status.containerStatuses[*]}{.name} {.ready} {.restartCount} {.imageID}{"\n"}{end}'
```

Expected: `true File true`; the CSINode lists `hostpath.csi.k8s.io`; the Pod is
`Running` and its five containers are `true` with a restart count of `0`. Each
`imageID` ends with the digest pinned for that image:

| Container | Expected digest |
|---|---|
| `hostpath` | `sha256:3af7d1b51d7af6e1858fcb0f08d87c89b4cc64cc709adfa9d75320da8f2f175c` |
| `node-driver-registrar` | `sha256:b7fefd08651f00ac4df1a196ed4c621c2451fbb899297fe9fad9e36c667af0c0` |
| `liveness-probe` | `sha256:19f2cf2f40e1987c7943945deca5f65cd4e4bf5cc0657fc78a5df131861578e8` |
| `csi-attacher` | `sha256:d1a26170efed45491074ca4545c81844eee2b9043fb885ac01fb4aed6f8b72d2` |
| `csi-provisioner` | `sha256:6ebe60fd8ed67a7f8a90d34402e9e706b274e53612d6fa5fbb8285728db622e2` |

Any other digest is a stop. No VolumeSnapshot API is installed; record any
`csi-provisioner` log line about it with
`kubectl --context docker-desktop -n ai-guard-csi-hostpath logs csi-hostpathplugin-0 -c csi-provisioner`.

### 4.2 StorageClass

```sh
kubectl --context docker-desktop get storageclass ai-guard-rwop -o jsonpath='{.provisioner} {.volumeBindingMode} {.reclaimPolicy} {.allowVolumeExpansion} {.metadata.annotations.storageclass\.kubernetes\.io/is-default-class}{"\n"}'
kubectl --context docker-desktop get storageclasses
```

Expected: `hostpath.csi.k8s.io Immediate Delete false false`, and the default
class recorded in step 1 is unchanged.

### 4.3 Namespaces, ServiceAccounts and NetworkPolicies

```sh
kubectl --context docker-desktop get namespaces ai-guard-csi-hostpath ai-guard-control ai-guard-tenant-a ai-guard-tenant-b -o custom-columns='NAME:.metadata.name,UID:.metadata.uid,ENFORCE:.metadata.labels.pod-security\.kubernetes\.io/enforce,VERSION:.metadata.labels.pod-security\.kubernetes\.io/enforce-version,AUDIT:.metadata.labels.pod-security\.kubernetes\.io/audit,WARN:.metadata.labels.pod-security\.kubernetes\.io/warn'
kubectl --context docker-desktop -n ai-guard-control get serviceaccounts broker-tenant-a broker-tenant-b -o custom-columns='NAME:.metadata.name,AUTOMOUNT:.automountServiceAccountToken'
kubectl --context docker-desktop -n ai-guard-tenant-a get serviceaccount default -o jsonpath='{.automountServiceAccountToken}{"\n"}'
kubectl --context docker-desktop -n ai-guard-tenant-b get serviceaccount default -o jsonpath='{.automountServiceAccountToken}{"\n"}'
kubectl --context docker-desktop -n ai-guard-tenant-a get networkpolicies -o name
kubectl --context docker-desktop -n ai-guard-tenant-b get networkpolicies -o name
```

Expected: `ai-guard-csi-hostpath` is `privileged`; the three other Namespaces
are `restricted` `v1.36` for enforce, audit and warn. Both broker accounts and
both tenant `default` accounts print `false`. Each tenant Namespace has exactly
one policy, `networkpolicy.networking.k8s.io/ai-guard-default-deny`. Record the
Namespace UIDs; the authority file needs them. A created policy proves no
enforcement (see the limits below).

### 4.4 Permission matrix

These checks impersonate the broker accounts with the administrator identity.
They review the RBAC rules; they do not authenticate a token.

```sh
A=system:serviceaccount:ai-guard-control:broker-tenant-a
B=system:serviceaccount:ai-guard-control:broker-tenant-b
kubectl --context docker-desktop auth can-i --list -n ai-guard-tenant-a --as="$A"
```

Each of the following commands must print `yes`:

```sh
kubectl --context docker-desktop auth can-i get pods -n ai-guard-tenant-a --as="$A"
kubectl --context docker-desktop auth can-i create pods -n ai-guard-tenant-a --as="$A"
kubectl --context docker-desktop auth can-i patch pods -n ai-guard-tenant-a --as="$A"
kubectl --context docker-desktop auth can-i delete pods -n ai-guard-tenant-a --as="$A"
kubectl --context docker-desktop auth can-i create pods --subresource=attach -n ai-guard-tenant-a --as="$A"
kubectl --context docker-desktop auth can-i get persistentvolumeclaims -n ai-guard-tenant-a --as="$A"
kubectl --context docker-desktop auth can-i create persistentvolumeclaims -n ai-guard-tenant-a --as="$A"
kubectl --context docker-desktop auth can-i delete persistentvolumeclaims -n ai-guard-tenant-a --as="$A"
kubectl --context docker-desktop auth can-i get networkpolicies.networking.k8s.io -n ai-guard-tenant-a --as="$A"
kubectl --context docker-desktop auth can-i list networkpolicies.networking.k8s.io -n ai-guard-tenant-a --as="$A"
kubectl --context docker-desktop auth can-i get serviceaccounts/default -n ai-guard-tenant-a --as="$A"
kubectl --context docker-desktop auth can-i get namespaces/kube-system --as="$A"
kubectl --context docker-desktop auth can-i get namespaces/ai-guard-tenant-a --as="$A"
kubectl --context docker-desktop auth can-i get storageclasses.storage.k8s.io/ai-guard-rwop --as="$A"
kubectl --context docker-desktop auth can-i get csidrivers.storage.k8s.io/hostpath.csi.k8s.io --as="$A"
```

Each of the following commands must print `no`:

```sh
kubectl --context docker-desktop auth can-i update pods -n ai-guard-tenant-a --as="$A"
kubectl --context docker-desktop auth can-i list pods -n ai-guard-tenant-a --as="$A"
kubectl --context docker-desktop auth can-i watch pods -n ai-guard-tenant-a --as="$A"
kubectl --context docker-desktop auth can-i create pods --subresource=exec -n ai-guard-tenant-a --as="$A"
kubectl --context docker-desktop auth can-i get pods --subresource=log -n ai-guard-tenant-a --as="$A"
kubectl --context docker-desktop auth can-i get pods --subresource=attach -n ai-guard-tenant-a --as="$A"
kubectl --context docker-desktop auth can-i patch pods --subresource=ephemeralcontainers -n ai-guard-tenant-a --as="$A"
kubectl --context docker-desktop auth can-i create pods --subresource=portforward -n ai-guard-tenant-a --as="$A"
kubectl --context docker-desktop auth can-i create pods --subresource=binding -n ai-guard-tenant-a --as="$A"
kubectl --context docker-desktop auth can-i get secrets -n ai-guard-tenant-a --as="$A"
kubectl --context docker-desktop auth can-i list secrets -n ai-guard-tenant-a --as="$A"
kubectl --context docker-desktop auth can-i create serviceaccounts --subresource=token -n ai-guard-tenant-a --as="$A"
kubectl --context docker-desktop auth can-i list serviceaccounts -n ai-guard-tenant-a --as="$A"
kubectl --context docker-desktop auth can-i update persistentvolumeclaims -n ai-guard-tenant-a --as="$A"
kubectl --context docker-desktop auth can-i create rolebindings.rbac.authorization.k8s.io -n ai-guard-tenant-a --as="$A"
kubectl --context docker-desktop auth can-i list namespaces --as="$A"
kubectl --context docker-desktop auth can-i get namespaces/ai-guard-tenant-b --as="$A"
kubectl --context docker-desktop auth can-i get storageclasses.storage.k8s.io/standard --as="$A"
kubectl --context docker-desktop auth can-i get persistentvolumes --as="$A"
kubectl --context docker-desktop auth can-i create serviceaccounts --subresource=token -n ai-guard-control --as="$A"
kubectl --context docker-desktop auth can-i get secrets -n ai-guard-control --as="$A"
kubectl --context docker-desktop auth can-i get pods -n ai-guard-tenant-b --as="$A"
kubectl --context docker-desktop auth can-i create pods -n ai-guard-tenant-b --as="$A"
kubectl --context docker-desktop auth can-i delete pods -n ai-guard-tenant-b --as="$A"
kubectl --context docker-desktop auth can-i create pods --subresource=attach -n ai-guard-tenant-b --as="$A"
kubectl --context docker-desktop auth can-i get persistentvolumeclaims -n ai-guard-tenant-b --as="$A"
kubectl --context docker-desktop auth can-i create persistentvolumeclaims -n ai-guard-tenant-b --as="$A"
kubectl --context docker-desktop auth can-i list networkpolicies.networking.k8s.io -n ai-guard-tenant-b --as="$A"
kubectl --context docker-desktop auth can-i get serviceaccounts/default -n ai-guard-tenant-b --as="$A"
```

Repeat both lists for tenant B: replace `"$A"` by `"$B"` and exchange
`ai-guard-tenant-a` and `ai-guard-tenant-b`. The command exits `0` for `yes`
and `1` for `no`. Plan step 2 of the runtime guide then checks the same rights
with ai-guard's own `doctor --check-permissions` and the real token.

## 5. Broker credentials

Each tenant broker uses a short-lived TokenRequest token for its own
ServiceAccount. Prepare a new `0700` private control directory per tenant,
outside source and export trees, then request the token directly into a new
private file without displaying it:

```sh
(
  umask 077
  set -C
  kubectl --context docker-desktop --request-timeout=10s \
    --namespace ai-guard-control create token broker-tenant-a \
    --duration=1h --allow-missing-template-keys=false \
    -o jsonpath='{.status.token}' > /absolute/private/control/kubernetes/tenant-a/token-20261121T1800Z
)
```

- The default lifetime of this recipe is `1h`. It may be raised up to `24h`
  (`--duration=24h`); never request more. The API server chooses the actual
  lifetime. Record the time of the request.
- Check the exit code. After a failure, keep any incomplete file private and
  use another new path; `set -C` refuses to overwrite an existing file.
  JSONPath writes no trailing newline.
- Do the same for `broker-tenant-b` into its own directory. One token never
  serves two tenants.
- Never print the token, pass it as an argument, put it in an environment
  variable or a permanent token Secret, or give the operator kubeconfig to the
  product.

**Renewal.** Before the recorded expiry, request a new token into a new file
with the same command and a new file name, point the broker's Kubernetes
token path (for example `--token-file`) to the new file, then delete the
expired file. This recipe
provides no automatic renewal. An expired token is refused by the API server.
These tokens are not bound to an object and cannot be revoked one by one
before expiry; removing the tenant's RoleBinding and ClusterRoleBinding, or
deleting the ServiceAccount, cuts access.

**CA bundle.** Extract only the public cluster CA from the kubeconfig cluster
entry into a new private file, then hash it for the authority file:

```sh
(
  umask 077
  set -C
  kubectl config view --raw --minify --context docker-desktop \
    -o jsonpath='{.clusters[0].cluster.certificate-authority-data}' \
    | base64 --decode > /absolute/private/control/kubernetes/ca.pem
)
shasum -a 256 /absolute/private/control/kubernetes/ca.pem
```

`--raw` is needed to read the CA data; the JSONPath selects only the public
certificate. Never print the whole raw view: it contains the administrator's
client key. If the cluster entry names a `certificate-authority` file instead,
copy that file.

**Endpoint.** Record the API endpoint of the context:

```sh
kubectl config view --minify --context docker-desktop -o jsonpath='{.clusters[0].cluster.server}{"\n"}'
```

Put it, the Namespace UIDs of `kube-system` and of the tenant Namespace, the
tenant Namespace name and the CA hash in that tenant's authority file, as
described in the
[Kubernetes guide](../../../docs/setup/kubernetes/README.md#run-the-read-only-cli-diagnostic).
Docker Desktop can publish the API server on another local port after a
restart. ai-guard then refuses the recorded authority until a new authority
file records the new endpoint. A cluster reset also changes the CA and the
Namespace UIDs.

## 6. Load the worker image into the node

Session Pods use `imagePullPolicy: Never` and an image referenced by digest,
so the worker image must already be in the node's containerd image store
(namespace `k8s.io`). This is an operator procedure; ai-guard does not run it.

1. Record the local image: `docker image inspect --format '{{.Id}} {{json .RepoTags}} {{json .RepoDigests}}' <worker-image>`.
2. Check access to the hidden node container **without changing any
   setting**:

   ```sh
   docker exec desktop-control-plane ctr --namespace k8s.io version
   ```

3. Only if step 2 fails because the container is not visible: record the
   values of `ShowKubernetesSystemContainers` and `UseContainerdSnapshotter`
   from Docker Desktop's settings store (on macOS,
   `~/Library/Group Containers/group.com.docker/settings-store.json`; check
   the Docker Desktop documentation for your version), enable **Settings →
   Kubernetes → Show system containers (advanced)** in the Docker Desktop
   interface, record both values again, and repeat step 2. Change no other
   setting: `UseContainerdSnapshotter` stays `false`. Never edit the settings
   file by hand.
4. Import the image:

   ```sh
   docker save <worker-image@sha256:digest> | docker exec -i desktop-control-plane ctr --namespace k8s.io images import --digests -
   ```

   Use the digest reference when step 1 shows a repository digest. A locally
   built image without one is saved by its tag, after checking that the tag
   still points to the image ID recorded in step 1.

5. Verify by digest:

   ```sh
   docker exec desktop-control-plane ctr --namespace k8s.io images ls
   docker exec desktop-control-plane crictl inspecti <repository@sha256:digest>
   ```

   The listing shows the imported records and their digest. `crictl inspecti`
   resolves the reference the way the kubelet does; its image ID must equal
   the image ID recorded in step 1. The reference it resolves is the one to
   retain in the session profile. If the digest reference does not resolve,
   the Pod would fail with `ErrImageNeverPull`: stop and report. Never switch
   to a tag or to another pull policy. If `crictl` is absent from the node,
   record it; the resolution then stays unverified.

## 7. Pod stuck with the `ai-guard.io/session-terminal` finalizer

ai-guard adds the finalizer `ai-guard.io/session-terminal` to its session Pods
(`ai-guard-session-<attempt>`) and export Pods (`ai-guard-export-<attempt>`).
It keeps the Pod object until the broker has read the terminal state by UID,
then removes the finalizer itself. If the broker is lost, a Pod can stay with
this finalizer, or stay `Terminating` after a deletion. The operator procedure
is:

1. Read the Pod and retain its JSON after review (by design it has no Secret,
   token or environment):

   ```sh
   kubectl --context docker-desktop -n ai-guard-tenant-a get pod ai-guard-session-<attempt> -o json
   kubectl --context docker-desktop -n ai-guard-tenant-a get pod ai-guard-session-<attempt> -o jsonpath='uid={.metadata.uid}{"\n"}finalizers={.metadata.finalizers}{"\n"}deletion={.metadata.deletionTimestamp}{"\n"}phase={.status.phase}{"\n"}node={.spec.nodeName}{"\n"}gates={.spec.schedulingGates}{"\n"}{range .status.containerStatuses[*]}{.name} running={.state.running} exit={.state.terminated.exitCode} finished={.state.terminated.finishedAt}{"\n"}{end}'
   ```

2. Continue only if one of these holds:
   - **terminated**: the `worker` container has a `terminated` state (exit
     code and finish time) and the phase is `Succeeded` or `Failed`;
   - **never scheduled**: `spec.nodeName` is empty, the scheduling gate
     `ai-guard.io/session-v2` is still present and there is no container
     status.

   If the container is running, or the state is unclear, stop. Ending a
   running session is not part of this procedure.
3. Confirm that the tenant's broker is really stopped, so that two actors do
   not act on the same Pod.
4. Remove only this finalizer, with a JSON Patch that tests the UID recorded in
   step 1 and the exact finalizer list:

   ```sh
   kubectl --context docker-desktop -n ai-guard-tenant-a patch pod ai-guard-session-<attempt> --type=json \
     -p '[{"op":"test","path":"/metadata/uid","value":"<pod-uid>"},{"op":"test","path":"/metadata/finalizers","value":["ai-guard.io/session-terminal"]},{"op":"remove","path":"/metadata/finalizers"}]'
   ```

   A failed `test` means another Pod instance has this name or the Pod carries
   another finalizer: stop and inspect again.
5. Check the result. A Pod that was `Terminating` disappears; otherwise the
   terminal Pod remains without finalizer.
6. Record the action with the JSON of step 1. It is an operator action, not a
   broker observation; the reconciliation of that attempt must account for it.

**Never force-delete** (`--grace-period=0 --force`). It removes the object
without confirmation that the container stopped, and a running container could
keep the `ReadWriteOncePod` volume.

## 8. Uninstall

Remove in this order. The CSI driver must keep running until the last volume
of `ai-guard-rwop` is gone.

1. Check each tenant Namespace for remaining ai-guard Pods. The list must be
   empty; handle a stuck Pod with section 7.

   ```sh
   kubectl --context docker-desktop -n ai-guard-tenant-a get pods -o custom-columns='NAME:.metadata.name,UID:.metadata.uid,PHASE:.status.phase,FINALIZERS:.metadata.finalizers,DELETING:.metadata.deletionTimestamp'
   ```

2. List the session claims and record name, UID and volume:

   ```sh
   kubectl --context docker-desktop -n ai-guard-tenant-a get persistentvolumeclaims -o custom-columns='NAME:.metadata.name,UID:.metadata.uid,VOLUME:.spec.volumeName,CLASS:.spec.storageClassName,PHASE:.status.phase'
   ```

3. Delete each claim by UID, then wait for its PersistentVolume to disappear
   (the `Delete` reclaim policy makes the driver remove the volume directory):

   ```sh
   printf '%s' '{"kind":"DeleteOptions","apiVersion":"v1","preconditions":{"uid":"<claim-uid>"}}' \
     | kubectl --context docker-desktop delete --raw /api/v1/namespaces/ai-guard-tenant-a/persistentvolumeclaims/ai-guard-ws-<attempt> -f -
   kubectl --context docker-desktop wait --for=delete persistentvolume/<volume-name> --timeout=120s
   ```

   A UID mismatch is refused (HTTP 409): stop and inspect again. Never fall
   back to a deletion by name.
4. Repeat steps 1 to 3 for `ai-guard-tenant-b`, then check that no volume of
   the class remains; the command must print nothing:

   ```sh
   kubectl --context docker-desktop get persistentvolumes -o jsonpath='{range .items[?(@.spec.storageClassName=="ai-guard-rwop")]}{.metadata.name}{"\n"}{end}'
   ```

5. Remove the tenant overlays, the control Namespace, the StorageClass and the
   driver:

   ```sh
   kubectl --context docker-desktop delete -k deploy/kubernetes/docker-desktop/tenant/tenant-a --timeout=300s
   kubectl --context docker-desktop delete -k deploy/kubernetes/docker-desktop/tenant/tenant-b --timeout=300s
   kubectl --context docker-desktop delete -k deploy/kubernetes/docker-desktop/control --timeout=300s
   kubectl --context docker-desktop delete -k deploy/kubernetes/docker-desktop/storage --timeout=300s
   kubectl --context docker-desktop delete -k deploy/kubernetes/docker-desktop/csi-hostpath --timeout=300s
   ```

6. The node directory `/var/lib/csi-hostpath-data/` (driver `state.json` and
   any volume directory left behind) remains on the Docker Desktop node.
   Removing it is a separate node action with its own decision. Delete the
   private token, CA and authority files of section 5 when they are no longer
   needed.

## 9. Limits

- **Not production.** This recipe proves nothing about a production cluster
  and makes no production-readiness claim. Gate G11 stays open until the plan
  steps of the runtime guide have their own evidence.
- **Storage.** Each filesystem volume is a plain directory on the node
  filesystem, inside the Docker Desktop VM, created with mode `0777`; the
  kubelet applies the Pod's `fsGroup` (`fsGroupPolicy: File`). There is no
  byte or inode enforcement: the requested size is only compared with the
  plugin maximum (1 TiB by default). Single node only; no snapshot, expansion
  or backup.
- **ReadWriteOncePod.** Kubernetes v1.36 enforces `ReadWriteOncePod` for CSI
  volumes, and the hostpath driver supports the single-writer access mode.
  This is still to observe on the cluster, including the export Pod that
  mounts the claim read-only after the session Pod's terminal state while the
  finalizer still keeps the session Pod object.
- **Network.** A NetworkPolicy object is not enforcement. Its enforcement by
  the CNI plugin must be observed by plan step 4a of the runtime guide; until
  then steps 5 to 12 stay `BLOCKED`.
- **Privileged driver.** The CSI plugin runs privileged with host mounts in
  `ai-guard-csi-hostpath`. Anyone able to create Pods in that Namespace gains
  node access: keep it to the cluster administrator.
- **Attach permission (open item EXT-8).** The Role grants `create` on
  `pods/attach` only. Whether the API server also checks `get` for a
  WebSocket attach upgrade on this cluster is to confirm. A refused attach is
  recorded and reported; the Role is not widened without a decision.
- **Snapshot API.** No VolumeSnapshot CRD is installed. How
  `csi-provisioner` v6.4.0 behaves without it is to observe in its logs.
- **Credentials.** A TokenRequest token is a bearer credential until it
  expires. Keep it in its private file only.
