# Prepare the Docker Desktop Kubernetes platform for ai-guard

[English](README.md) · [Français](README.fr.md) · [Setup index](../README.md) · [Docker/Podman setup](../local/README.md)

Prepare and verify a basic **single-node** Kubernetes platform for ai-guard on your workstation. Use the inspection commands below to check its configuration and health. They use the `docker-desktop` context explicitly and leave your global context unchanged.

This guide covers cluster preparation, access, resources, images and diagnosis. Follow the [local setup](../local/README.md) for ai-guard installation, Docker/Podman agent sessions and broker operations.

## Prepare Docker Desktop and kubectl

Install Docker Desktop using the official guide for [macOS](https://docs.docker.com/desktop/setup/install/mac-install/), [Windows](https://docs.docker.com/desktop/setup/install/windows-install/) or [Linux](https://docs.docker.com/desktop/setup/install/linux/). Keep existing installations, configuration and data. Have the required image downloads available through your approved network or registry before planning offline operation.

Open Docker Desktop's Kubernetes view. If creation is already in progress, let it finish. For a new cluster, select the basic single-node configuration; Docker documents `kubeadm` as its single-node provisioner. Record the provisioner and Kubernetes version offered by your installed Desktop version. [Docker Desktop Kubernetes instructions](https://docs.docker.com/desktop/use-desktop/kubernetes/).

Make `kubectl` available through Docker Desktop or the [official Kubernetes installation procedure](https://kubernetes.io/docs/tasks/tools/). Resolve any competing installations deliberately, keeping a client compatible with the server version.

## Check the context and TLS

Inspect only the named context and its public connection metadata:

```sh
kubectl --context docker-desktop config get-contexts docker-desktop
kubectl --context docker-desktop config view --minify \
  -o jsonpath='{.clusters[0].cluster.server}{"\n"}{.clusters[0].cluster.insecure-skip-tls-verify}{"\n"}'
```

The second command prints the server URL, then the TLS-verification bypass setting. Require an HTTPS URL matching your Docker Desktop installation and a bypass setting that is absent or `false`. Stop if the context points elsewhere or the setting is `true`; repair the Desktop context through the installation's administration procedure. Keep the kubeconfig, credentials and private keys private. These filtered commands display no token or certificate material. See [context selection](https://kubernetes.io/docs/concepts/configuration/organize-cluster-access-kubeconfig/) and [filtered config output](https://kubernetes.io/docs/reference/kubectl/generated/kubectl_config/kubectl_config_view/).

Then contact the selected API with TLS verification retained:

```sh
kubectl --context docker-desktop --request-timeout=10s version --output=yaml
```

Require a successful server response. A context name or configuration listing alone does not verify connectivity or trust.

## Check one node and its resources

List the nodes first. Continue only when the list contains exactly one node belonging to this installation, then wait for its Ready condition:

```sh
kubectl --context docker-desktop --request-timeout=10s get nodes -o wide
kubectl --context docker-desktop --request-timeout=10s wait --for=condition=Ready nodes --all --timeout=180s
kubectl --context docker-desktop --request-timeout=10s get pods --namespace kube-system
kubectl --context docker-desktop --request-timeout=10s get nodes \
  -o custom-columns=NAME:.metadata.name,CPU:.status.allocatable.cpu,MEMORY:.status.allocatable.memory,PODS:.status.allocatable.pods
```

Treat a timeout as a request to diagnose; it is not a successful readiness check. Review unavailable system pods separately from completed jobs. The [wait command](https://kubernetes.io/docs/reference/kubectl/generated/kubectl_wait/) tests a condition; [node conditions](https://kubernetes.io/docs/concepts/architecture/nodes/) describe the node's reported state.

Budget resources for Kubernetes, ai-guard control services, workers, image builds and any local model running together. In Docker Desktop, review CPU, memory and disk allocations under **Settings → Resources**. Keep room for the host and persistent data. See [Docker resource settings](https://docs.docker.com/desktop/settings-and-maintenance/settings/#resources).

The last command reports allocatable capacity, not current consumption. Size workload requests and limits against that capacity and measure actual usage during the intended workload; see [Kubernetes resource management](https://kubernetes.io/docs/concepts/configuration/manage-resources-containers/). One Ready node establishes neither high availability nor capacity for 100 workers.

## Prepare images

Build ai-guard images with the [local guide](../local/README.md) and, for Codex or Claude Code, the [provider guide](../providers/README.md). Preserve the selected tags, image identities and architecture. No public prebuilt ai-guard image is required.

Check the provisioner's supported image store before planning pod image delivery. Docker Desktop's `kind` provisioner requires its containerd image store; switching stores hides the inactive store's images and containers while retaining them on disk. See [Docker's image-store guide](https://docs.docker.com/desktop/features/containerd/) and [provisioner compatibility](https://docs.docker.com/desktop/use-desktop/kubernetes/#cluster-provisioning-method).

A Docker build alone does not establish that a Kubernetes node can resolve the image. Prepare exact image references and an approved delivery route for the chosen node runtime. Kubernetes image availability and `imagePullPolicy` jointly determine whether a pod can start; see [local and pulled images](https://kubernetes.io/docs/concepts/containers/images/). Keep registry credentials in the operator's approved credential mechanism.

## Diagnose a preparation failure

| Symptom | Action |
|---|---|
| Missing `docker-desktop` context | Check creation progress in Docker Desktop and the intended kubectl installation/configuration. Preserve other contexts. |
| TLS or connection error | Check the selected server, Desktop status, certificate trust and workstation time. Keep verification enabled. |
| Node absent or not Ready | Check Desktop's creation status, available memory/disk and required image access; repeat the bounded readiness check after correcting the cause. |
| Unavailable system pod | Inspect the named pod's state in Docker Desktop's Kubernetes view and obtain scoped diagnostics. Keep raw logs private and redact any shared excerpt. |
| Image pull failure | Check the exact reference, architecture, selected image store and approved registry access. Preserve existing images and volumes. |

Record Desktop/client/server versions, provisioner, node count, selected context and the specific error. Share only reviewed metadata. Keep kubeconfig files, tokens, authentication URLs and full environment dumps out of reports.

## Use and maintain the environment

Keep ai-guard control data and credentials separate from Kubernetes administration. Follow the [Docker/Podman procedure](../local/README.md) for local agent sessions, and [enterprise configuration](../enterprise/README.md) for the identity, storage and transport requirements of a managed installation.

Evaluate platform readiness against the intended workload: available resources, image access, storage preservation, enforced network restrictions and recovery after interruption. Verify network-policy enforcement with the selected networking implementation; listing a NetworkPolicy does not establish isolation. [Kubernetes network-policy requirements](https://kubernetes.io/docs/concepts/services-networking/network-policies/).

Before planned Desktop maintenance, stop active ai-guard workers through their broker and preserve control data. After reopening Desktop, repeat the TLS and readiness checks against the same context. Preserve the existing cluster and volumes while diagnosing; schedule any cluster replacement or upgrade separately with an explicit data-preservation procedure.
