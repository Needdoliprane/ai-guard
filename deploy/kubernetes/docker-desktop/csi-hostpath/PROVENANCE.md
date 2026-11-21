# Provenance of the vendored CSI hostpath files

Local test storage for Docker Desktop, not production storage. The client
chooses and operates its production storage.

## Upstream sources

| Vendored file | Upstream project, tag and path | SHA-256 of the original |
|---|---|---|
| `csi-hostpath-driverinfo.yaml` | [kubernetes-csi/csi-driver-host-path](https://github.com/kubernetes-csi/csi-driver-host-path) `v1.19.0`, `deploy/kubernetes-1.35/hostpath/csi-hostpath-driverinfo.yaml` | `464307265464b77189bd4b339774cd2de69589458a62a740028b248f248c7e9a` |
| `csi-hostpath-plugin.yaml` | same tag, `deploy/kubernetes-1.35/hostpath/csi-hostpath-plugin.yaml` | `c3d075ee1dd17fa2b93d0ea216cd94d47b57f1f825210d0ca101607c58f9fdfa` |
| `external-provisioner-rbac.yaml` | [kubernetes-csi/external-provisioner](https://github.com/kubernetes-csi/external-provisioner) `v6.4.0`, `deploy/kubernetes/rbac.yaml` | `0ee8427b746a1d3b695705b74c2d7fb165121110b1a10c0b6e204918d93e814f` |
| `external-attacher-rbac.yaml` | [kubernetes-csi/external-attacher](https://github.com/kubernetes-csi/external-attacher) `v4.13.0`, `deploy/kubernetes/rbac.yaml` | `3a38b5198b0496927c0ce26c63e71d43e2263e2dbbb93a83acdec71d7c90a766` |

- The `v1.19.0` tag of csi-driver-host-path points to commit
  `6fab18ae4fc4f486193b618114eeb817d9929242`. In that commit,
  `deploy/kubernetes-latest` and `deploy/kubernetes-1.36` are symbolic links
  that resolve to `deploy/kubernetes-1.35`, which holds the real files.
- The Git blob identifiers of the two saved csi-driver-host-path files match
  that commit's tree: `0250a52c1a84d680cc4c2761f7976acb251337f2` (driverinfo)
  and `122a1680f6e6f3ed83a2d326241e852662c077a3` (plugin).
- The sidecar RBAC files come from the tags matching the sidecar images that
  the plugin manifest uses (`csi-provisioner:v6.4.0`, `csi-attacher:v4.13.0`),
  which is what the upstream deploy script selects.
- Sources were saved on 21 November 2026 and checked against the SHA-256
  values above before vendoring. They were never executed. The upstream
  `deploy.sh` is not used by this recipe.

## Licence

The upstream files are Copyright The Kubernetes Authors and licensed under the
Apache License, Version 2.0, the same licence as this repository (see
`LICENSE` at the repository root). The upstream YAML files carry no licence
header of their own. The upstream csi-driver-host-path tree at that commit
contains a `LICENSE` file and no `NOTICE` file. Each modified file starts with
a notice that names its origin and states that it was changed.

## Changes

`csi-hostpath-driverinfo.yaml`: none. It is a byte-identical copy, so
`shasum -a 256 csi-hostpath-driverinfo.yaml` prints the original hash. The
`CSIDriver` object `hostpath.csi.k8s.io` is unchanged.

`csi-hostpath-plugin.yaml`:

1. Added the origin and modification notice at the top.
2. Replaced the namespace `default` by `ai-guard-csi-hostpath` in the
   ServiceAccount, the StatefulSet and every binding subject, and wrote the
   namespace explicitly on the two RoleBindings and their subjects.
3. Removed the `csi-snapshotter` container and the bindings
   `csi-hostpathplugin-snapshotter-cluster-role`,
   `csi-hostpathplugin-snapshot-metadata-cluster-role` and
   `csi-hostpathplugin-snapshotter-role`. No VolumeSnapshot CRD, snapshot
   controller, snapshot class or snapshot-metadata service is installed.
4. Removed the `csi-resizer` container and the bindings
   `csi-hostpathplugin-resizer-cluster-role` and
   `csi-hostpathplugin-resizer-role`: the `ai-guard-rwop` StorageClass sets
   `allowVolumeExpansion: false`, so the resizer would only add cluster-wide
   rights without use.
5. Changed the `hostpathplugin` tag from `v1.18.0` (the value in the `v1.19.0`
   manifest) to `v1.19.0`, the image of the release. `kustomization.yaml`
   pins every image by digest.

`external-provisioner-rbac.yaml` and `external-attacher-rbac.yaml`:

1. Added the origin and modification notice at the top.
2. Removed the unused ServiceAccount (`csi-provisioner`, `csi-attacher`), its
   ClusterRoleBinding (`csi-provisioner-role`, `csi-attacher-role`) and its
   RoleBinding (`csi-provisioner-role-cfg`, `csi-attacher-role-cfg`). The
   plugin manifest binds the same roles to `csi-hostpathplugin-sa`.
3. Replaced the namespace `default` of the leader-election Role by
   `ai-guard-csi-hostpath`.
4. Wrote explicitly the two labels that the upstream deploy script adds
   (`app.kubernetes.io/instance: hostpath.csi.k8s.io`,
   `app.kubernetes.io/part-of: csi-driver-host-path`).
5. In `external-provisioner-rbac.yaml`, removed one trailing space at the end
   of a comment line.

The rules of the ClusterRoles `external-provisioner-runner` and
`external-attacher-runner` and of the Roles `external-provisioner-cfg` and
`external-attacher-cfg` are unchanged, including the provisioner's
`snapshot.storage.k8s.io` rules, which have no effect while no snapshot CRD is
installed.

Not vendored: `csi-hostpath-testing.yaml` (socat Pod and NodePort Service),
`csi-hostpath-snapshotclass.yaml`, `csi-snapshot-metadata-sidecar.patch`, the
snapshotter and resizer RBAC files, `deploy.sh` and `destroy.sh`.

## Image pins

Each digest is the SHA-256 of the multi-architecture index manifest that
`registry.k8s.io/sig-storage` served on 21 November 2026; each index lists a
`linux/arm64` child.

| Image | Tag | Index digest |
|---|---|---|
| `registry.k8s.io/sig-storage/hostpathplugin` | `v1.19.0` | `sha256:3af7d1b51d7af6e1858fcb0f08d87c89b4cc64cc709adfa9d75320da8f2f175c` |
| `registry.k8s.io/sig-storage/csi-node-driver-registrar` | `v2.18.0` | `sha256:b7fefd08651f00ac4df1a196ed4c621c2451fbb899297fe9fad9e36c667af0c0` |
| `registry.k8s.io/sig-storage/livenessprobe` | `v2.20.0` | `sha256:19f2cf2f40e1987c7943945deca5f65cd4e4bf5cc0657fc78a5df131861578e8` |
| `registry.k8s.io/sig-storage/csi-attacher` | `v4.13.0` | `sha256:d1a26170efed45491074ca4545c81844eee2b9043fb885ac01fb4aed6f8b72d2` |
| `registry.k8s.io/sig-storage/csi-provisioner` | `v6.4.0` | `sha256:6ebe60fd8ed67a7f8a90d34402e9e706b274e53612d6fa5fbb8285728db622e2` |

To update a pin, record the new upstream tag, commit, file hashes and index
digest here, review the diff of the vendored files against the new originals,
and render the recipe again with `kubectl kustomize`.
