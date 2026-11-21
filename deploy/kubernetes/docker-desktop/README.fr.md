# Recette Docker Desktop pour l'adaptateur de session Kubernetes (test local)

[English version](README.md)

Cette **recette de test locale** sert à qualifier l'adaptateur Kubernetes
d'ai-guard sur le cluster Kubernetes intégré à Docker Desktop. **Ce n'est pas
un déploiement de production.** Le stockage installé est un stockage de test
local, sans limite d'octets appliquée. Le client exploite ses clusters de
production et choisit son stockage, son plugin réseau et ses identités de
production.

L'application de cette recette ne qualifie rien à elle seule. Les étapes de
qualification, leurs oracles et leur statut figurent dans le
[guide du runtime Kubernetes](../../../docs/setup/kubernetes-runtime/README.fr.md)
(modèle de sécurité, plan Docker Desktop et points ouverts). Le fichier
d'autorité et le diagnostic `doctor` sont décrits dans le
[guide Kubernetes](../../../docs/setup/kubernetes/README.fr.md#lancer-le-diagnostic-cli-en-lecture-seule).

Dans ce plan, la recette fournit les objets de tenant de l'étape 1 et le
prérequis de stockage de la section 8.2. Elle n'exécute ni la sonde
d'application CNI de l'étape 4a, ni aucune étape ultérieure.

Lancer chaque commande depuis la racine du dépôt, uniquement sur le contexte
`docker-desktop`, avec l'identité d'administration de Docker Desktop. Chaque
modification du cluster ci-dessous est une action de l'opérateur.

## Contenu

| Répertoire | Objets |
|---|---|
| [`csi-hostpath/`](csi-hostpath/) | Namespace `ai-guard-csi-hostpath` (Pod Security `privileged`), CSIDriver `hostpath.csi.k8s.io`, StatefulSet `csi-hostpathplugin` (conteneurs `hostpath`, `node-driver-registrar`, `liveness-probe`, `csi-attacher`, `csi-provisioner`), son ServiceAccount, les ClusterRoles des sidecars, les Roles d'élection de leader et leurs bindings. Copie amont avec les changements listés dans [PROVENANCE.md](csi-hostpath/PROVENANCE.md). |
| [`storage/`](storage/) | StorageClass `ai-guard-rwop` : provisionneur `hostpath.csi.k8s.io`, `Immediate`, `Delete`, sans extension, jamais classe par défaut. |
| [`control/`](control/) | Namespace `ai-guard-control` (Pod Security `restricted` `v1.36` ; aucun Pod n'y tourne) et ServiceAccounts de broker `broker-tenant-a` et `broker-tenant-b`, sans montage automatique de token. |
| [`tenant/base/`](tenant/base/) | Objets communs des tenants : Namespace avec Pod Security `restricted` `v1.36` (enforce, audit, warn), ServiceAccount `default` sans montage automatique de token, NetworkPolicy `ai-guard-default-deny` (tous les Pods, Ingress et Egress, aucune règle) et Role `ai-guard-session-broker`. Jamais appliqué seul. |
| [`tenant/tenant-a/`](tenant/tenant-a/), [`tenant/tenant-b/`](tenant/tenant-b/) | Namespace `ai-guard-tenant-a` ou `ai-guard-tenant-b`, RoleBinding vers le broker de ce tenant, ClusterRole et ClusterRoleBinding `ai-guard-session-broker-tenant-<x>`. |

Les images du pilote CSI sont épinglées par digest. Ne sont pas inclus : le
`deploy.sh` amont, le Pod de test socat et son Service NodePort, la classe de
snapshot et les sidecars snapshotter et resizer.

### Droits d'un broker de tenant

`broker-tenant-a` reçoit exactement les droits ci-dessous, et aucun droit dans
`ai-guard-tenant-b` ; `broker-tenant-b` reçoit les mêmes pour le tenant B.

| Portée | Ressource | Verbes |
|---|---|---|
| `ai-guard-tenant-a` | `pods` | `get`, `create`, `patch`, `delete` |
| `ai-guard-tenant-a` | `pods/attach` | `create` |
| `ai-guard-tenant-a` | `persistentvolumeclaims` | `get`, `create`, `delete` |
| `ai-guard-tenant-a` | `networkpolicies.networking.k8s.io` | `get`, `list` |
| `ai-guard-tenant-a` | `serviceaccounts` nommé `default` | `get` |
| Cluster | `namespaces` nommés `kube-system` et `ai-guard-tenant-a` | `get` |
| Cluster | `storageclasses.storage.k8s.io` nommée `ai-guard-rwop` | `get` |
| Cluster | `csidrivers.storage.k8s.io` nommé `hostpath.csi.k8s.io` | `get` |

Aucun droit n'est accordé pour exec, logs, Secrets, `update`, `list` ou
`watch` des Pods, conteneurs éphémères, port-forward, binding ou demande de
token. Kubernetes accorde aussi à toute identité authentifiée ses droits par
défaut de découverte et d'auto-vérification (par exemple
`selfsubjectaccessreviews`), qu'ai-guard utilise pour ses contrôles de
permissions. RBAC ne peut pas restreindre `create` par nom ou contenu d'objet ;
Pod Security `restricted` et les requêtes fermées d'ai-guard sont les autres
barrières.

## 1. Prérequis et relevés

Relever ces faits avant toute modification et les conserver avec les preuves :

```sh
kubectl config current-context
kubectl --context docker-desktop version
kubectl --context docker-desktop get nodes -o wide
docker version --format '{{.Server.Platform.Name}}'
kubectl --context docker-desktop -n kube-system get daemonsets -o wide
kubectl --context docker-desktop get storageclasses
kubectl --context docker-desktop get csidrivers
```

- La première commande doit afficher `docker-desktop`. S'arrêter sur tout
  autre contexte.
- La recette épingle Pod Security `v1.36` et a été rendue avec kubectl v1.36.1
  (Kustomize v5.8.1). Une autre version du serveur impose de revoir d'abord ces
  épingles.
- Relever le plugin CNI dans la liste des DaemonSets. Son application des
  NetworkPolicy est observée plus tard (étape 4a du plan), jamais supposée.
- Aucun de ces noms ne doit déjà exister : Namespaces `ai-guard-csi-hostpath`,
  `ai-guard-control`, `ai-guard-tenant-a` et `ai-guard-tenant-b` ; CSIDriver
  `hostpath.csi.k8s.io` ; StorageClass `ai-guard-rwop` ; ClusterRoles
  `external-provisioner-runner`, `external-attacher-runner` et
  `ai-guard-session-broker-tenant-a`/`-b`. L'application fusionnerait avec un
  objet existant : s'arrêter si l'un d'eux existe.

  ```sh
  kubectl --context docker-desktop get namespace ai-guard-csi-hostpath ai-guard-control ai-guard-tenant-a ai-guard-tenant-b
  kubectl --context docker-desktop get csidriver hostpath.csi.k8s.io
  kubectl --context docker-desktop get storageclass ai-guard-rwop
  kubectl --context docker-desktop get clusterrole external-provisioner-runner external-attacher-runner ai-guard-session-broker-tenant-a ai-guard-session-broker-tenant-b
  ```

  Chaque commande doit répondre `NotFound` pour chaque nom.
- Le nœud tire les cinq images CSI de `registry.k8s.io` par digest : la VM de
  Docker Desktop a besoin de la résolution DNS et d'un accès HTTPS à ce
  registre. L'installation hors ligne n'est pas couverte par cette recette.

## 2. Rendre et relire

Rendre chaque répertoire et lire la sortie avant de l'appliquer :

```sh
kubectl kustomize deploy/kubernetes/docker-desktop/csi-hostpath
kubectl kustomize deploy/kubernetes/docker-desktop/storage
kubectl kustomize deploy/kubernetes/docker-desktop/control
kubectl kustomize deploy/kubernetes/docker-desktop/tenant/tenant-a
kubectl kustomize deploy/kubernetes/docker-desktop/tenant/tenant-b
shasum -a 256 deploy/kubernetes/docker-desktop/csi-hostpath/csi-hostpath-driverinfo.yaml
```

Attendu : chaque ligne `image:` du rendu CSI se termine par
`@sha256:<digest>` du tableau de [PROVENANCE.md](csi-hostpath/PROVENANCE.md) ;
aucun rendu de tenant ne contient `ai-guard-tenant-unset` ; l'empreinte du
driverinfo vaut
`464307265464b77189bd4b339774cd2de69589458a62a740028b248f248c7e9a`.
Le rendu ne contacte aucun cluster.

## 3. Installer

Appliquer dans cet ordre :

```sh
kubectl --context docker-desktop apply --server-side --field-manager=ai-guard-docker-desktop-recipe -k deploy/kubernetes/docker-desktop/csi-hostpath
kubectl --context docker-desktop -n ai-guard-csi-hostpath rollout status statefulset/csi-hostpathplugin --timeout=300s
kubectl --context docker-desktop apply --server-side --field-manager=ai-guard-docker-desktop-recipe -k deploy/kubernetes/docker-desktop/storage
kubectl --context docker-desktop apply --server-side --field-manager=ai-guard-docker-desktop-recipe -k deploy/kubernetes/docker-desktop/control
kubectl --context docker-desktop apply --server-side --field-manager=ai-guard-docker-desktop-recipe -k deploy/kubernetes/docker-desktop/tenant/tenant-a
kubectl --context docker-desktop apply --server-side --field-manager=ai-guard-docker-desktop-recipe -k deploy/kubernetes/docker-desktop/tenant/tenant-b
```

L'application côté serveur crée ou met à jour chaque objet en une requête.
Elle évite une course avec le contrôleur qui crée le ServiceAccount `default`
d'un nouveau Namespace. Une application de plusieurs objets peut réussir
partiellement : conserver sa sortie, inspecter les objets qu'elle nomme et
relancer la même commande après avoir corrigé la cause. Ne jamais supprimer et
recréer un Namespace pour effacer une erreur.

## 4. Vérifier

Chaque contrôle indique son résultat attendu. S'arrêter à la première
différence.

### 4.1 Pilote CSI

```sh
kubectl --context docker-desktop get csidriver hostpath.csi.k8s.io -o jsonpath='{.spec.attachRequired} {.spec.fsGroupPolicy} {.spec.podInfoOnMount}{"\n"}'
kubectl --context docker-desktop get csinode desktop-control-plane -o jsonpath='{.spec.drivers[*].name}{"\n"}'
kubectl --context docker-desktop -n ai-guard-csi-hostpath get pod csi-hostpathplugin-0 -o jsonpath='{.status.phase}{"\n"}{range .status.containerStatuses[*]}{.name} {.ready} {.restartCount} {.imageID}{"\n"}{end}'
```

Attendu : `true File true` ; le CSINode liste `hostpath.csi.k8s.io` ; le Pod
est `Running` et ses cinq conteneurs sont à `true` avec un nombre de
redémarrages de `0`. Chaque `imageID` se termine par le digest épinglé pour
cette image :

| Conteneur | Digest attendu |
|---|---|
| `hostpath` | `sha256:3af7d1b51d7af6e1858fcb0f08d87c89b4cc64cc709adfa9d75320da8f2f175c` |
| `node-driver-registrar` | `sha256:b7fefd08651f00ac4df1a196ed4c621c2451fbb899297fe9fad9e36c667af0c0` |
| `liveness-probe` | `sha256:19f2cf2f40e1987c7943945deca5f65cd4e4bf5cc0657fc78a5df131861578e8` |
| `csi-attacher` | `sha256:d1a26170efed45491074ca4545c81844eee2b9043fb885ac01fb4aed6f8b72d2` |
| `csi-provisioner` | `sha256:6ebe60fd8ed67a7f8a90d34402e9e706b274e53612d6fa5fbb8285728db622e2` |

Tout autre digest impose l'arrêt. Aucune API VolumeSnapshot n'est installée ;
relever toute ligne de log de `csi-provisioner` à son sujet avec
`kubectl --context docker-desktop -n ai-guard-csi-hostpath logs csi-hostpathplugin-0 -c csi-provisioner`.

### 4.2 StorageClass

```sh
kubectl --context docker-desktop get storageclass ai-guard-rwop -o jsonpath='{.provisioner} {.volumeBindingMode} {.reclaimPolicy} {.allowVolumeExpansion} {.metadata.annotations.storageclass\.kubernetes\.io/is-default-class}{"\n"}'
kubectl --context docker-desktop get storageclasses
```

Attendu : `hostpath.csi.k8s.io Immediate Delete false false`, et la classe par
défaut relevée à l'étape 1 est inchangée.

### 4.3 Namespaces, ServiceAccounts et NetworkPolicies

```sh
kubectl --context docker-desktop get namespaces ai-guard-csi-hostpath ai-guard-control ai-guard-tenant-a ai-guard-tenant-b -o custom-columns='NAME:.metadata.name,UID:.metadata.uid,ENFORCE:.metadata.labels.pod-security\.kubernetes\.io/enforce,VERSION:.metadata.labels.pod-security\.kubernetes\.io/enforce-version,AUDIT:.metadata.labels.pod-security\.kubernetes\.io/audit,WARN:.metadata.labels.pod-security\.kubernetes\.io/warn'
kubectl --context docker-desktop -n ai-guard-control get serviceaccounts broker-tenant-a broker-tenant-b -o custom-columns='NAME:.metadata.name,AUTOMOUNT:.automountServiceAccountToken'
kubectl --context docker-desktop -n ai-guard-tenant-a get serviceaccount default -o jsonpath='{.automountServiceAccountToken}{"\n"}'
kubectl --context docker-desktop -n ai-guard-tenant-b get serviceaccount default -o jsonpath='{.automountServiceAccountToken}{"\n"}'
kubectl --context docker-desktop -n ai-guard-tenant-a get networkpolicies -o name
kubectl --context docker-desktop -n ai-guard-tenant-b get networkpolicies -o name
```

Attendu : `ai-guard-csi-hostpath` est `privileged` ; les trois autres
Namespaces sont `restricted` `v1.36` pour enforce, audit et warn. Les deux
comptes de broker et les deux comptes `default` des tenants affichent `false`.
Chaque Namespace de tenant a exactement une politique,
`networkpolicy.networking.k8s.io/ai-guard-default-deny`. Relever les UID des
Namespaces : le fichier d'autorité en a besoin. Une politique créée ne prouve
aucune application (voir les limites ci-dessous).

### 4.4 Matrice des permissions

Ces contrôles empruntent l'identité des comptes de broker (impersonation)
depuis l'identité d'administration. Ils examinent les règles RBAC ; ils
n'authentifient aucun token.

```sh
A=system:serviceaccount:ai-guard-control:broker-tenant-a
B=system:serviceaccount:ai-guard-control:broker-tenant-b
kubectl --context docker-desktop auth can-i --list -n ai-guard-tenant-a --as="$A"
```

Chacune des commandes suivantes doit afficher `yes` :

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

Chacune des commandes suivantes doit afficher `no` :

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

Répéter les deux listes pour le tenant B : remplacer `"$A"` par `"$B"` et
échanger `ai-guard-tenant-a` et `ai-guard-tenant-b`. La commande sort avec `0`
pour `yes` et `1` pour `no`. L'étape 2 du plan du guide runtime contrôle
ensuite les mêmes droits avec le `doctor --check-permissions` d'ai-guard et le
vrai token.

## 5. Credentials des brokers

Chaque broker de tenant utilise un token TokenRequest de courte durée pour son
propre ServiceAccount. Préparer un nouveau répertoire de contrôle privé `0700`
par tenant, hors des arbres source et export, puis demander le token
directement dans un nouveau fichier privé, sans l'afficher :

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

- La durée par défaut de cette recette est `1h`. Elle peut être portée
  jusqu'à `24h` (`--duration=24h`) ; ne jamais demander davantage. Le serveur
  API choisit la durée effective. Relever l'heure de la demande.
- Vérifier le code de sortie. Après un échec, garder privé tout fichier
  incomplet et utiliser un autre chemin neuf ; `set -C` refuse d'écraser un
  fichier existant. JSONPath n'écrit pas de saut de ligne terminal.
- Faire de même pour `broker-tenant-b` dans son propre répertoire. Un token ne
  sert jamais deux tenants.
- Ne jamais afficher le token, le passer en argument, le mettre dans une
  variable d'environnement ou un Secret token permanent, ni donner le
  kubeconfig opérateur au produit.

**Renouvellement.** Avant l'expiration relevée, demander un nouveau token dans
un nouveau fichier avec la même commande et un nouveau nom de fichier, faire
pointer le chemin de token Kubernetes du broker (par exemple `--token-file`)
vers le nouveau fichier, puis supprimer le fichier expiré. Cette recette ne
fournit aucun renouvellement automatique. Un token expiré est refusé par le
serveur API. Ces tokens ne sont liés à aucun objet et ne peuvent pas être
révoqués un par un avant expiration ; retirer le RoleBinding et le
ClusterRoleBinding du tenant, ou supprimer le ServiceAccount, coupe l'accès.

**Bundle d'AC.** Extraire seulement l'AC publique du cluster depuis l'entrée
cluster du kubeconfig vers un nouveau fichier privé, puis calculer son
empreinte pour le fichier d'autorité :

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

`--raw` est nécessaire pour lire les données de l'AC ; le JSONPath ne
sélectionne que le certificat public. Ne jamais afficher la vue brute
complète : elle contient la clé client de l'administrateur. Si l'entrée
cluster désigne plutôt un fichier `certificate-authority`, copier ce fichier.

**Endpoint.** Relever l'endpoint API du contexte :

```sh
kubectl config view --minify --context docker-desktop -o jsonpath='{.clusters[0].cluster.server}{"\n"}'
```

Le reporter, avec les UID des Namespaces `kube-system` et du tenant, le nom du
Namespace du tenant et l'empreinte de l'AC, dans le fichier d'autorité de ce
tenant, comme décrit dans le
[guide Kubernetes](../../../docs/setup/kubernetes/README.fr.md#lancer-le-diagnostic-cli-en-lecture-seule).
Docker Desktop peut publier le serveur API sur un autre port local après un
redémarrage. ai-guard refuse alors l'autorité relevée jusqu'à ce qu'un nouveau
fichier d'autorité enregistre le nouvel endpoint. Une réinitialisation du
cluster change aussi l'AC et les UID des Namespaces.

## 6. Charger l'image worker dans le nœud

Les Pods de session utilisent `imagePullPolicy: Never` et une image
référencée par digest : l'image worker doit déjà être dans le magasin d'images
containerd du nœud (namespace `k8s.io`). C'est une procédure d'opérateur ;
ai-guard ne l'exécute pas.

1. Relever l'image locale : `docker image inspect --format '{{.Id}} {{json .RepoTags}} {{json .RepoDigests}}' <image-worker>`.
2. Vérifier l'accès au conteneur caché du nœud **sans modifier aucun
   réglage** :

   ```sh
   docker exec desktop-control-plane ctr --namespace k8s.io version
   ```

3. Seulement si l'étape 2 échoue parce que le conteneur n'est pas visible :
   relever les valeurs de `ShowKubernetesSystemContainers` et
   `UseContainerdSnapshotter` dans le magasin de réglages de Docker Desktop
   (sur macOS, `~/Library/Group Containers/group.com.docker/settings-store.json` ;
   vérifier dans la documentation Docker Desktop de votre version), activer
   **Settings → Kubernetes → Show system containers (advanced)** dans
   l'interface de Docker Desktop, relever à nouveau les deux valeurs, puis
   refaire l'étape 2. Ne modifier aucun autre réglage :
   `UseContainerdSnapshotter` reste `false`. Ne jamais éditer le fichier de
   réglages à la main.
4. Importer l'image :

   ```sh
   docker save <image-worker@sha256:digest> | docker exec -i desktop-control-plane ctr --namespace k8s.io images import --digests -
   ```

   Utiliser la référence par digest lorsque l'étape 1 montre un digest de
   dépôt. Une image construite localement sans digest de dépôt est
   sauvegardée par son tag, après avoir vérifié que ce tag désigne toujours
   l'ID d'image relevé à l'étape 1.
5. Vérifier par digest :

   ```sh
   docker exec desktop-control-plane ctr --namespace k8s.io images ls
   docker exec desktop-control-plane crictl inspecti <depot@sha256:digest>
   ```

   La liste montre les enregistrements importés et leur digest.
   `crictl inspecti` résout la référence comme le kubelet ; son ID d'image doit
   être égal à l'ID relevé à l'étape 1. La référence qu'il résout est celle à
   retenir dans le profil de session. Si la référence par digest ne se résout
   pas, le Pod échouerait avec `ErrImageNeverPull` : s'arrêter et le signaler.
   Ne jamais passer à un tag ou à une autre politique de tirage. Si `crictl`
   est absent du nœud, le relever ; la résolution reste alors non vérifiée.

## 7. Pod bloqué par le finalizer `ai-guard.io/session-terminal`

ai-guard ajoute le finalizer `ai-guard.io/session-terminal` à ses Pods de
session (`ai-guard-session-<tentative>`) et d'export
(`ai-guard-export-<tentative>`). Il conserve l'objet Pod jusqu'à ce que le
broker ait lu l'état terminal par UID, puis retire lui-même le finalizer. Si le
broker est perdu, un Pod peut garder ce finalizer, ou rester `Terminating`
après une suppression. La procédure d'opérateur est la suivante :

1. Lire le Pod et conserver son JSON après relecture (par conception, il ne
   contient ni Secret, ni token, ni environnement) :

   ```sh
   kubectl --context docker-desktop -n ai-guard-tenant-a get pod ai-guard-session-<tentative> -o json
   kubectl --context docker-desktop -n ai-guard-tenant-a get pod ai-guard-session-<tentative> -o jsonpath='uid={.metadata.uid}{"\n"}finalizers={.metadata.finalizers}{"\n"}deletion={.metadata.deletionTimestamp}{"\n"}phase={.status.phase}{"\n"}node={.spec.nodeName}{"\n"}gates={.spec.schedulingGates}{"\n"}{range .status.containerStatuses[*]}{.name} running={.state.running} exit={.state.terminated.exitCode} finished={.state.terminated.finishedAt}{"\n"}{end}'
   ```

2. Continuer seulement si l'un de ces cas est établi :
   - **terminé** : le conteneur `worker` a un état `terminated` (code de
     sortie et heure de fin) et la phase est `Succeeded` ou `Failed` ;
   - **jamais planifié** : `spec.nodeName` est vide, la porte de planification
     `ai-guard.io/session-v2` est toujours présente et il n'existe aucun
     statut de conteneur.

   Si le conteneur tourne, ou si l'état est incertain, s'arrêter. Terminer une
   session en cours ne fait pas partie de cette procédure.
3. Confirmer que le broker du tenant est réellement arrêté, pour que deux
   acteurs n'agissent pas sur le même Pod.
4. Retirer seulement ce finalizer, avec un JSON Patch qui teste l'UID relevé à
   l'étape 1 et la liste exacte des finalizers :

   ```sh
   kubectl --context docker-desktop -n ai-guard-tenant-a patch pod ai-guard-session-<tentative> --type=json \
     -p '[{"op":"test","path":"/metadata/uid","value":"<uid-du-pod>"},{"op":"test","path":"/metadata/finalizers","value":["ai-guard.io/session-terminal"]},{"op":"remove","path":"/metadata/finalizers"}]'
   ```

   Un `test` refusé signifie qu'une autre instance de Pod porte ce nom ou que
   le Pod porte un autre finalizer : s'arrêter et inspecter à nouveau.
5. Vérifier le résultat. Un Pod qui était `Terminating` disparaît ; sinon le
   Pod terminal reste, sans finalizer.
6. Consigner l'action avec le JSON de l'étape 1. C'est une action d'opérateur,
   pas une observation du broker ; la réconciliation de cette tentative doit
   en tenir compte.

**Ne jamais forcer la suppression** (`--grace-period=0 --force`). Elle retire
l'objet sans confirmation de l'arrêt du conteneur, et un conteneur encore actif
pourrait garder le volume `ReadWriteOncePod`.

## 8. Désinstaller

Retirer dans cet ordre. Le pilote CSI doit continuer de tourner jusqu'à la
disparition du dernier volume de `ai-guard-rwop`.

1. Vérifier qu'il ne reste aucun Pod ai-guard dans chaque Namespace de tenant.
   La liste doit être vide ; traiter un Pod bloqué avec la section 7.

   ```sh
   kubectl --context docker-desktop -n ai-guard-tenant-a get pods -o custom-columns='NAME:.metadata.name,UID:.metadata.uid,PHASE:.status.phase,FINALIZERS:.metadata.finalizers,DELETING:.metadata.deletionTimestamp'
   ```

2. Lister les claims de session et relever nom, UID et volume :

   ```sh
   kubectl --context docker-desktop -n ai-guard-tenant-a get persistentvolumeclaims -o custom-columns='NAME:.metadata.name,UID:.metadata.uid,VOLUME:.spec.volumeName,CLASS:.spec.storageClassName,PHASE:.status.phase'
   ```

3. Supprimer chaque claim par UID, puis attendre la disparition de son
   PersistentVolume (la politique de récupération `Delete` fait supprimer le
   répertoire du volume par le pilote) :

   ```sh
   printf '%s' '{"kind":"DeleteOptions","apiVersion":"v1","preconditions":{"uid":"<uid-du-claim>"}}' \
     | kubectl --context docker-desktop delete --raw /api/v1/namespaces/ai-guard-tenant-a/persistentvolumeclaims/ai-guard-ws-<tentative> -f -
   kubectl --context docker-desktop wait --for=delete persistentvolume/<nom-du-volume> --timeout=120s
   ```

   Un UID différent est refusé (HTTP 409) : s'arrêter et inspecter à nouveau.
   Ne jamais se rabattre sur une suppression par nom.
4. Répéter les étapes 1 à 3 pour `ai-guard-tenant-b`, puis vérifier qu'aucun
   volume de la classe ne subsiste ; la commande ne doit rien afficher :

   ```sh
   kubectl --context docker-desktop get persistentvolumes -o jsonpath='{range .items[?(@.spec.storageClassName=="ai-guard-rwop")]}{.metadata.name}{"\n"}{end}'
   ```

5. Retirer les overlays de tenant, le Namespace de contrôle, la StorageClass et
   le pilote :

   ```sh
   kubectl --context docker-desktop delete -k deploy/kubernetes/docker-desktop/tenant/tenant-a --timeout=300s
   kubectl --context docker-desktop delete -k deploy/kubernetes/docker-desktop/tenant/tenant-b --timeout=300s
   kubectl --context docker-desktop delete -k deploy/kubernetes/docker-desktop/control --timeout=300s
   kubectl --context docker-desktop delete -k deploy/kubernetes/docker-desktop/storage --timeout=300s
   kubectl --context docker-desktop delete -k deploy/kubernetes/docker-desktop/csi-hostpath --timeout=300s
   ```

6. Le répertoire du nœud `/var/lib/csi-hostpath-data/` (`state.json` du pilote
   et tout répertoire de volume restant) demeure sur le nœud Docker Desktop.
   Le supprimer est une action distincte sur le nœud, avec sa propre décision.
   Supprimer les fichiers privés de token, d'AC et d'autorité de la section 5
   lorsqu'ils ne sont plus nécessaires.

## 9. Limites

- **Pas de production.** Cette recette ne prouve rien sur un cluster de
  production et n'annonce aucune aptitude à la production. Le gate G11 reste
  ouvert tant que les étapes du plan du guide runtime n'ont pas leurs propres
  preuves.
- **Stockage.** Chaque volume de système de fichiers est un simple répertoire
  du système de fichiers du nœud, dans la VM de Docker Desktop, créé avec le
  mode `0777` ; le kubelet applique le `fsGroup` du Pod
  (`fsGroupPolicy: File`). Aucune limite d'octets ou d'inodes n'est
  appliquée : la taille demandée est seulement comparée au maximum du plugin
  (1 Tio par défaut). Un seul nœud ; ni snapshot, ni extension, ni sauvegarde.
- **ReadWriteOncePod.** Kubernetes v1.36 applique `ReadWriteOncePod` aux
  volumes CSI, et le pilote hostpath prend en charge le mode d'accès à
  écrivain unique. Cela reste à observer sur le cluster, y compris pour le Pod
  d'export qui monte le claim en lecture seule après l'état terminal du Pod de
  session, alors que le finalizer conserve encore l'objet Pod de session.
- **Réseau.** Un objet NetworkPolicy n'est pas une application. Son
  application par le plugin CNI doit être observée par l'étape 4a du plan du
  guide runtime ; d'ici là, les étapes 5 à 12 restent `BLOCKED`.
- **Pilote privilégié.** Le plugin CSI tourne en mode privilégié avec des
  montages de l'hôte dans `ai-guard-csi-hostpath`. Quiconque peut créer des
  Pods dans ce Namespace obtient un accès au nœud : le réserver à
  l'administrateur du cluster.
- **Permission d'attach (point ouvert EXT-8).** Le Role accorde seulement
  `create` sur `pods/attach`. Il reste à confirmer si le serveur API contrôle
  aussi `get` pour une montée WebSocket d'attach sur ce cluster. Un attach
  refusé est consigné et signalé ; le Role n'est pas élargi sans décision.
- **API de snapshot.** Aucune CRD VolumeSnapshot n'est installée. Le
  comportement de `csi-provisioner` v6.4.0 sans elle reste à observer dans ses
  logs.
- **Credentials.** Un token TokenRequest est un credential porteur jusqu'à son
  expiration. Le garder uniquement dans son fichier privé.
