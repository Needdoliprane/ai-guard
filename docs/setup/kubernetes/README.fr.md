# Kubernetes : autorité, inspection et préflight d'admission

[English](README.md) · [Français](README.fr.md) · [Index de configuration](../README.fr.md)

La cible locale de qualification est le cluster Kubernetes à un nœud de Docker
Desktop. Utiliser Docker/Podman pour les sessions d'agents : l'adaptateur Rust
Kubernetes fournit diagnostic HTTPS, inspection et dry-run d'admission audité,
pas le lancement de workers. La qualification sur ce cluster réel reste
`NOT_RUN`. Il n'existe pas d'option de lancement
`--runtime kubernetes` ni de profil dotenv Kubernetes.

Le chemin interne de CREATE retenu relie le broker à l'API authentifiée du
contrôle et conserve demande, candidat résultat et ACK dans un journal privé.
La permission exige un201 live exact reçu dans la fenêtre du claim original ;
un replay ou GET ne peut pas la donner. La reprise soumet le même résultat
sans répéter CREATE Kubernetes. Ce composant n'a pas encore de constructeur
d'activation de production ni de commande utilisateur de lancement. Ses tests
HTTP/SQLite et peer TLS local ne qualifient ni le cluster Docker Desktop ni
le confinement des workers ; voir le [contrat d'ingénierie](../../engineering/KUBERNETES_ADMISSION.md)
et les [preuves](../../engineering/VALIDATION.md).

## Préparer l'identité d'inspection Docker Desktop

Le [profil public de lecture](../../../deploy/kubernetes/local-inspection.json)
crée le Namespace dédié `ai-guard-preflight`, le ServiceAccount
`ai-guard-inspector` sans automount de token, et les seules lectures de ce
Namespace, de `kube-system` et de `/version`. Il ne contient ni credential ni
worker. Pod Security `restricted` est épinglé à `v1.36`, cible locale de
qualification ; revoir cette version lors d'un changement de cluster.
Confirmer le contexte Docker Desktop local avant de provisionner :

```sh
kubectl config current-context
kubectl --context docker-desktop --request-timeout=10s create -f deploy/kubernetes/local-inspection.json
```

Depuis la racine du dépôt, utiliser une identité opérateur autorisée séparée.
`create` refuse de remplacer une ressource existante. Une création multi-objets
peut réussir partiellement : conserver sa sortie et examiner ces objets exacts
avant de poursuivre ; ne pas supprimer/recréer un Namespace pour effacer une
erreur. Si ces noms sont déjà utilisés, préparer une copie possédée et adapter
ensemble Namespace, sujets, rôles et noms de ressources.

Seul le préflight d'admission exige le
[grant distinct de création Pod](../../../deploy/kubernetes/local-preflight.json) :

```sh
kubectl --context docker-desktop --request-timeout=10s create -f deploy/kubernetes/local-preflight.json
```

Cela permet réellement `create pods` dans ce Namespace, **pas seulement un
dry-run**. Aucun droit API direct exec, logs, Secrets, autre Namespace ou
suppression n'est ajouté. Créer un Pod peut néanmoins permettre de consommer
des Secrets ou service accounts existants du Namespace selon son admission.
Le réserver à cet essai ; automount désactivé sur le ServiceAccount ne confine
pas tous les Pods. RBAC Kubernetes ne limite pas une création principale au nom
de l'objet ou au dry-run ; conserver cette identité dans le contrôle de
confiance. Voir les [règles RBAC officielles](https://kubernetes.io/docs/reference/access-authn-authz/rbac/).

Préparer un nouveau répertoire privé `0700` hors des arbres source/export,
puis demander un token court directement dans un nouveau fichier privé :

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

Vérifier le code de sortie ; garder toute sortie incomplète privée et choisir
un autre chemin neuf après échec. JSONPath évite le saut de ligne terminal.
Le serveur choisit la durée effective ; aucun renouvellement automatique
n'est fourni. La [commande TokenRequest](https://kubernetes.io/docs/reference/kubectl/generated/kubectl_create/kubectl_create_token/)
utilise par défaut l'audience du serveur API. Ne pas créer de Secret token
permanent ni donner le kubeconfig opérateur au produit. Renseigner l'AC
approuvée et les UID exacts dans le fichier d'autorité ci-dessous, avec
`namespace:ai-guard-preflight`. Ces profils ne prouvent ni propriété tenant,
confinement effectif ou lancement de worker. Leur application et les sondes
réelles du produit sont des étapes de qualification distinctes.

## Lancer le diagnostic CLI en lecture seule

Utiliser `agentsandbox kubernetes doctor` avec une autorité retenue explicite
et soit un fichier de jeton Bearer statique, soit une paire certificat/clé
client. La commande effectue seulement trois GET authentifiés : Namespace
`kube-system`, Namespace tenant exact et `/version`. Elle ne crée aucun Pod,
ne configure pas RBAC, n'admet aucune charge, n'exécute ni exec ni attach et
ne modifie pas le cluster. Le produit ne découvre aucun kubeconfig, contexte,
plugin de credentials ni configuration d'environnement.

Pour la cible Docker Desktop, faire d'abord approuver par le propriétaire du
cluster l'endpoint API exact, l'AC, l'identité et les permissions de lecture.
Obtenir avec un outil opérateur autorisé séparément le `metadata.uid` de
`kube-system` et du Namespace tenant voulu. Remplacer les deux placeholders
explicites de ces commandes opérateur ; `doctor` ne les exécute ni ne les
importe :

```sh
kubectl --context <authorized-desktop-context> get namespace kube-system -o jsonpath='{.metadata.uid}'
kubectl --context <authorized-desktop-context> get namespace <tenant-namespace> -o jsonpath='{.metadata.uid}'
shasum -a 256 /absolute/private/control/kubernetes/ca.pem
```

L'empreinte porte sur les octets exacts de l'AC préparée, espaces PEM compris ;
sous Linux, `sha256sum` permet le même calcul. Retenir un UUID d'autorité neuf
non nul attribué par le contrôle de confiance. Ni UID ni label de Namespace
n'authentifie un tenant. Ne pas inventer d'UID, accepter silencieusement son
changement ou choisir une identité administrateur par commodité. L'identité
restreinte doit être provisionnée hors de ce diagnostic.

Préparer un **nouveau** répertoire privé et ses fichiers sans remplacer les
credentials ni la confiance existants. Le JSON d'autorité suit le contrat
existant `KubernetesAuthorityV1`, refuse champs inconnus/doublons et reste
limité à 16 Kio. Toutes les valeurs suivantes sont illustratives et doivent
être remplacées par les valeurs approuvées ; le placeholder d'empreinte est
volontairement invalide :

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

Utiliser des chemins absolus sans symlinks. Chaque entrée est un fichier
ordinaire à lien unique, propriété de l'UID effectif et en `0600`, sous des
ancêtres appartenant à root/UID effectif sans écriture groupe/autres, avec les
contrôles ACL/filesystem du stockage privé. Un temporaire sticky de root est
l'exception contrôlée existante. Préférer un nouveau répertoire de contrôle
`0700` ; la commande ne répare aucun droit. AC/certificat limités à 64 Kio,
clé privée à 16 Kio, jeton à 8 Kio. Le jeton contient 32–8192 caractères ASCII
de jeton, sans espace ni saut de ligne ; la paire PEM certificat/clé est
validée pour la correspondance des clés et la compatibilité TLS.

```sh
agentsandbox kubernetes doctor \
  --authority-file /absolute/private/control/kubernetes/authority.json \
  --ca-file /absolute/private/control/kubernetes/ca.pem \
  --token-file /absolute/private/control/kubernetes/token
```

Pour mTLS, remplacer `--token-file` par les deux options liées :

```sh
agentsandbox kubernetes doctor \
  --authority-file /absolute/private/control/kubernetes/authority.json \
  --ca-file /absolute/private/control/kubernetes/ca.pem \
  --client-cert-file /absolute/private/control/kubernetes/client-chain.pem \
  --client-key-file /absolute/private/control/kubernetes/client.key
```

Les deux authentifications ensemble, ou une seule moitié de paire
certificat/clé, sont refusées. AC explicite et validation du nom d'hôte sont
obligatoires : aucune option insecure, découverte de proxy ou identité de
repli. Le produit n'extrait aucun credential kubeconfig et ne lance aucun
plugin d'authentification.

Le succès sort en `0` et écrit un rapport JSON unique avec `version: 1`,
`admission_available: false`, les vrais `diagnostics` et une entrée
`admission_gap_reasons` pour chaque écart rapporté. Les observations incluent
empreinte d'autorité, version serveur, UID des Namespaces, label Pod Security
reconnu et date d'observation. Réserver ces métadonnées à leur audience
autorisée. Ce n'est ni une autorisation de charge, ni une preuve d'application
du confinement, ni un reçu d'audit durable du contrôle. Les trois lectures
sont successives et non atomiques. Les bornes par requête ci-dessous
s'appliquent ; plusieurs requêtes de huit secondes peuvent être nécessaires.
Huit secondes n'est donc ni la borne totale de la commande ni un benchmark
de latence sur une machine de développement chargée.

Une erreur d'inspection/configuration sort en `1` sans rapport JSON ; arguments
incomplets ou contradictoires sortent en `2`. Les erreurs utilisent des
catégories fermées sans chemin, corps de réponse ni chaîne d'erreur
transport/JSON. Selon la catégorie, vérifier fichiers privés, identités
retenues, endpoint/AC/SAN et droits de lecture autorisés. Si une observation
typée contient un credential retenu, le rapport entier est refusé : aucune
identité n'est remplacée par une valeur masquée inventée. Corriger la source
en privé avant de réessayer. Des credentials modifiés exigent une nouvelle
invocation préparée.

## Lancer un préflight d'admission audité

`agentsandbox kubernetes preflight` soumet un profil Pod fermé à la chaîne
d'admission de l'API sans persister de Pod ni exécuter image, commande ou
worker. Il utilise les mêmes fichiers explicites d'autorité Kubernetes, d'AC
et d'authentification que `doctor`, plus un plan privé et une configuration
privée du contrôle. Il ne provisionne pas RBAC, n'installe aucun service et
n'active pas le lancement de sessions Kubernetes.

Retenir d'abord un rapport `doctor` réussi pour l'autorité approuvée. Faire
provisionner par le propriétaire du cluster les lectures existantes et
`create` sur `pods` dans le Namespace exact. Kubernetes associe ce POST à
`create` et applique la même autorisation au dry-run qu'à une création réelle.
Ce credential peut donc aussi créer de vrais Pods avec d'autres outils ;
ce **n'est pas un credential limité au dry-run**. Le garder dans le stockage
du contrôle de confiance, hors des workers. Voir les contrats officiels
d'[autorisation dry-run](https://kubernetes.io/docs/reference/using-api/api-concepts/#dry-run-authorization)
et de [correspondance des verbes](https://kubernetes.io/docs/reference/access-authn-authz/authorization/#determine-the-request-verb).

Préparer deux **nouveaux** fichiers à partir des
[exemples de plan](examples/preflight-plan.json) et de
[contrôle](examples/preflight-control.json). Ce sont des modèles, pas des
configurations utilisables telles quelles : remplacer chaque placeholder
entre chevrons. Ils ne contiennent ni autorité, ni digest d'image, UUID,
endpoint ou credential réels. Fichiers et credentials restent en `0600`, avec
les contrôles de propriété, d'ancêtres et d'ACL décrits ci-dessus ; chemins
absolus sans symlinks. Plan et contrôle JSON sont limités chacun à 16 Kio.
Champs inconnus et doublons de champs typés sont refusés. Ne pas remplacer
une configuration existante pour préparer cet essai.

Le plan version 1 contient :

- `authority_fingerprint` : la valeur exacte
  `diagnostics.authority_fingerprint` de `doctor` pour ce fichier d'autorité
  inchangé, pas l'empreinte de l'AC.
- `session_id` et `operation_id` : UUID neufs non nuls pour cet essai. L'UUID
  d'opération identifie l'événement d'audit demandé ; sa réutilisation avec un
  contenu divergent est un conflit, pas une autre permission de dry-run.
- `image` : une vraie référence de dépôt choisie par l'opérateur, avec
  `@sha256:` immuable et digest hexadécimal minuscule de 64 caractères. Un tag
  seul est refusé ; ne placer ici ni secret ni credential. Un préflight réussi
  ne prouve pas que l'image est disponible ou exécutable ; ce profil ne la
  télécharge ni ne l'exécute et déclare `imagePullPolicy: Never`.
- `resources` : `cpu_millis` de 1 à 200000, `memory_mib` de 1 à 102400,
  `workspace_mib` et `temporary_mib` positifs dont la somme est strictement
  inférieure à `memory_mib`, et `deadline_seconds` de 1 à 86400. L'exemple
  déclare 2000 milliCPU, 1024 Mio de mémoire, 128/64 Mio de workspace/tmp et
  60 secondes. Ces bornes et déclarations ne qualifient ni capacité ni délai.

Le fichier de contrôle version 1 ne contient que `gateway`,
`control_namespace_uuid` et `runner_token_file`, avec `version: 1`. Obtenir le
namespace du contrôle existant authentifié, par exemple avec cette commande
après remplacement du gateway et du chemin d'exemple :

```sh
agentsandbox broker control-info \
  --gateway https://gateway.example.invalid \
  --token-file /absolute/private/control/runner
```

Utiliser son `control_namespace_uuid` exact, une identité runner et un schéma
compatible 3–9 avec `broker_supervision_v1`. Le gateway est une origine HTTPS
ou une origine HTTP loopback explicitement choisie ; ce fichier n'ajoute pas
d'options AC privée ou mTLS à la connexion gateway. `runner_token_file` est
un chemin absolu vers un fichier privé contenant au plus 4 Kio, pas la valeur du jeton. Le runner du
contrôle et les credentials Kubernetes sont distincts. Il n'existe **aucun
champ `tenant_id`** : le serveur authentifié fournit le tenant. L'empreinte de
l'autorité Kubernetes choisie est liée à la demande auditée ; cela ne prouve
ni propriété exclusive de ce Namespace par ce tenant, ni droit de lancement.

Lancer l'essai préparé :

```sh
agentsandbox kubernetes preflight \
  --authority-file /absolute/private/control/kubernetes/authority.json \
  --ca-file /absolute/private/control/kubernetes/ca.pem \
  --token-file /absolute/private/control/kubernetes/token \
  --plan-file /absolute/private/control/kubernetes/preflight-plan.json \
  --control-file /absolute/private/control/kubernetes/preflight-control.json
```

Pour mTLS Kubernetes, remplacer `--token-file` par les deux options de
certificat ; le fichier de contrôle et son credential runner restent requis :

```sh
agentsandbox kubernetes preflight \
  --authority-file /absolute/private/control/kubernetes/authority.json \
  --ca-file /absolute/private/control/kubernetes/ca.pem \
  --client-cert-file /absolute/private/control/kubernetes/client-chain.pem \
  --client-key-file /absolute/private/control/kubernetes/client.key \
  --plan-file /absolute/private/control/kubernetes/preflight-plan.json \
  --control-file /absolute/private/control/kubernetes/preflight-control.json
```

Le contrôle vérifie d'abord l'identité/namespace du serveur et un reçu durable
exact de `runtime.kubernetes.preflight.requested`. Ensuite seulement, un guard
privé consommable lié à la cible et au reçu peut permettre le dry-run. Il
revérifie les fichiers retenus d'autorité, de plan, de contrôle et de jeton
runner et expire 60 secondes après l'accusé. L'adaptateur effectue les GET des
deux ancres Namespace avant et après son unique POST :

```text
POST /api/v1/namespaces/{retained-namespace}/pods?dryRun=All&fieldValidation=Strict
```

Le profil Pod comporte une commande fixe `/usr/bin/id`, sans argv ni
environnement métier fournis par l'appelant, UID/GID 1000, racine en lecture
seule, capacités supprimées, sans élévation de privilèges, seccomp
RuntimeDefault, sans namespaces hôte ni
jeton de service account monté, avec workspace/tmp mémoire bornés et limites
déclarées de ressources et de durée. Il n'accepte pas de manifeste Pod libre.
Les injections ou changements d'admission hors des défauts relus sont
refusés : sidecars, init containers, credentials, volumes hôte et sûreté
affaiblie notamment. Seuls l'alias de service account par défaut, les défauts
de priorité/préemption et les deux tolérances NoExecute standard de 300
secondes sont admis comme ajouts ; les défauts booléens faux omis sont
normalisés. Examiner un refus avec le propriétaire du cluster sans contourner
le profil. Le Pod envoyé utilise la validation API stricte. Le reçu d'audit
exige son enveloppe fermée complète, refuse les champs racine inconnus et les
membres JSON dupliqués à toute profondeur.

Selon le [contrat dry-run Kubernetes](https://kubernetes.io/docs/reference/using-api/api-concepts/#dry-run),
admission et valeurs par défaut sont évaluées sans stockage du Pod. Cette
commande ne télécharge ni n'exécute d'image. L'UID retourné n'est pas retenu
comme handle de Pod possédé. Les vérifications successives de Namespace
détectent un remplacement mais ne sont pas une précondition UID atomique du
POST. Une admission dry-run ne prouve ni isolation CNI ni intégrité des
webhooks d'admission tiers.

Avant d'annoncer un succès, le contrôle doit recevoir un reçu durable exact de
`runtime.kubernetes.preflight.result`. Les deux événements ont la source
`trusted-control`, la provenance `declared` et la couverture `partial` :
l'API accuse réception durable, pas une observation runtime indépendante.
Le code `0` renvoie un JSON avec tenant serveur, namespace de contrôle, deux
reçus, observation et **`admission_available: false`**. Les dix écarts
d'admission restent ouverts.

Une erreur sort en `1` sans JSON de succès ; arguments CLI invalides en `2`.
Aucun retry automatique. Un reçu de demande absent ou divergent bloque le
dry-run, même si l'événement a déjà pu être stocké. Après une erreur runtime,
d'audit du résultat ou de rapport final, l'admission a pu recevoir le dry-run :
l'erreur ne prouve pas zéro POST. Examiner l'audit autorisé par UUID
d'opération et réconcilier cette réception possible avant de préparer un
nouvel essai explicite avec un UUID d'opération neuf. Garder inconnus les
effets inconnus ; un résultat dry-run ne libère aucun quota et n'enregistre
pas d'usage worker.

## Configurer la politique centrale du tenant

Le gateway accepte une option `held_create_policy` dans chaque entrée tenant
de son registre existant `--tenants-file`. L'API HTTP centrale des brokers
utilise cette configuration pour les décisions Reserve/Claim ; elle seule
n'est pas une permission runtime. Le broker ne peut toujours pas lancer de
workers Kubernetes ; diagnostic et dry-run ci-dessus restent distincts de
cette politique.

Insérer cet **objet JSON partiel dans l'entrée tenant existante visée**, en
conservant sa base, ses références de credentials et ses autres réglages.
Ce n'est ni un registre complet, ni un champ du fichier de contrôle du
préflight. L'empreinte et l'image ci-dessous sont fictives ; les remplacer
par l'empreinte complète d'autorité approuvée rapportée par `doctor` et
l'image immuable exacte :

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

Une politique absente ou `null` laisse l'admission fermée ; `{}` est invalide.
Elle exige version 1, un TTL de décision de 1 à 300 secondes et 1 à 64 tuples
d'approbation distincts, dans 16 Kio de JSON compact. Champs inconnus et tuples
dupliqués sont refusés. Chaque empreinte contient 64 caractères hexadécimaux
minuscules ; ce n'est ni l'empreinte de l'AC ni l'UUID d'autorité. L'image fait
au plus 256 octets, avec un nom de dépôt explicite et `@sha256:` suivi de
64 caractères hexadécimaux minuscules ; un tag seul est refusé. Le nom de dépôt
commence par une lettre ASCII minuscule ou un chiffre, ne contient que lettres
ASCII minuscules, chiffres et `/._-:`, et ne contient ni `..` ni `//`. Le profil est
exactement `kubernetes_held_pod_v1`. La correspondance porte sur le tuple
complet empreinte/image/profil : un autre nom de dépôt avec le même digest
ne correspond pas.

Le gateway valide et trie les approbations au chargement du registre, sans
normaliser leurs chaînes ; leur ordre ne change pas l'empreinte de politique.
`worker_quota` reste la configuration distincte du quota durable du tenant.
Ces contrôles valident seulement la configuration ; ils ne qualifient ni
propriété du Namespace, admission cluster, RBAC ou confinement CNI.

## Maintenir l'audit : du schéma 8 au schéma 9

Le schéma 9 ajoute un marqueur durable de format de politique held-create à
une base tenant existante et validée au schéma 8. Il n'ajoute aucune table,
n'active aucune admission et ne crée aucun Pod. SQLite et la politique tenant
restent au gateway ; les credentials Kubernetes restent au broker de confiance.

Arrêter le gateway et tous les brokers utilisant cette base avant maintenance.
`--confirm-control-stopped` est votre attestation ; il n'arrête pas ces processus
et ne garantit pas leur exclusion. Adapter tous les chemins d'exemple et
l'identifiant tenant à l'installation existante. Utiliser sa limite d'événements
d'audit configurée (`10000` ci-dessous est un exemple), et un **fichier de sauvegarde neuf,
inexistant** dans le stockage privé du contrôle :

```sh
agentsandbox migrate-control \
  --data /private/control/tenant.sqlite \
  --tenant-id tenant-a \
  --backup /private/control/before-policy9.sqlite \
  --target-version 9 \
  --confirm-control-stopped \
  --max-events 10000
```

La commande crée, vérifie et synchronise la sauvegarde au schéma 8, puis
revérifie le head d'audit source avant de commiter le marqueur et la version 9.
Conserver la sauvegarde et le rapport JSON de migration, dont séquence et
empreinte du marqueur. Le namespace de contrôle, les projections, les quotas
réservés et les valeurs CAS de révision/epoch existants sont préservés.
Rouvrir la base ne réactive pas la supervision. Une sauvegarde existante est
refusée, et une tentative échouée peut laisser un fichier de sauvegarde : le
conserver et examiner l'échec avant de choisir une autre destination neuve.
Ne jamais écraser une sauvegarde ni forcer un downgrade par modification de
`PRAGMA user_version` ou réécriture de l'audit.

La validation de l'audit V1 historique et la reconstruction des projections
restent disponibles. Au schéma 9, le Store central produit maintenant les
décisions V2 Admission et Reconcile. Les capacités mémoire V1 restent refusées
sur 9 ; une capacité V2 courante appropriée peut récupérer le reçu historique
V1 exact et son snapshot associé, sans append ni permission Fresh. Conserver
les journaux en attente et les quotas des inconnues. Ni le schéma 9 ni la
politique configurée n'activent le lancement de workers Kubernetes.

## Utiliser l'API centrale de contrôle des brokers

Le gateway expose six routes HTTP réservées au broker sous `/api/v1` :
métadonnées de protocole, réservation, claim, lookup historique du claim,
résultat et annulation ; voir le [contrat des routes et DTO](../../api.md#kubernetes-held-create-http-v1).
Elles authentifient le broker courant et déduisent le tenant de sa preuve
Bearer. Aucun header, query ou champ JSON ne choisit tenant, politique ou quota.
`admission_policy_configured` décrit la configuration, pas une admission
disponible ni une qualification cluster.

Reserve/Claim utilisent politique et quota courants du tenant immuable, avec
identité, grant et supervision active. Chaque lookup/résultat/annulation obtient
une décision Reconcile distincte depuis la vraie réservation originale et les
grant/quota courants ; retrait de politique, baisse de quota ou supervision
inactive n'interdisent pas seuls cette réconciliation. Une identité révoquée
ou divergente reste refusée. La provenance de politique est un snapshot de
configuration déclaré, pas une approbation humaine nominative ni une preuve
de confinement.

Claim renvoie `201` seulement avec la preuve Store du premier commit exact et
une fenêtre d'émission encore valide à la préparation de la réponse. Replays
et GET historiques renvoient `200`/`observe_only`, jamais Fresh. Un JSON seul
n'est pas une permission runtime. Le client HTTP broker/canal ACK live et le
Coordinator au schéma 9 restent à raccorder ; activation, transfert workspace
et sessions d'agents Kubernetes restent indisponibles. Ces routes de contrôle
et le préflight ne créent ni n'admettent de Pod dans un cluster.

## Configurer l'intégration Rust

Le contrôle de confiance utilise
[`runtime::kubernetes`](../../../src/runtime/kubernetes.rs). Il fournit une
origine HTTPS explicite, un UUID d'autorité non nul, l'UID retenu du namespace
`kube-system`, le nom et l'UID du namespace tenant, et le SHA-256 des octets de
l'AC préparée. Le contrôle authentifié choisit le tenant ; nom de namespace,
UID et labels de Pod ne sont pas des credentials tenant.

Ouvrir les credentials avec `control_fs::PrivateFile` : AC et fichier de jeton
Bearer, ou certificat client et clé privée. Conserver ces fichiers dans le
stockage privé du contrôle. Utiliser une identité restreinte aux lectures
nécessaires : `GET /version`, les deux ressources Namespace exactes et le Pod
exact à observer. L'adaptateur ne provisionne pas cette identité et ne configure
pas RBAC. Ne transmettre ni ces credentials ni un kubeconfig administrateur à
un worker.

`KubernetesRuntime::connect` valide la configuration locale et construit le
client. Il ne lit ni kubeconfig ambiant, ni contexte courant, plugin exécutable
ou proxy. Son AC explicite remplace les racines intégrées ; validation du nom
d'hôte et des certificats conservée. Redirections et retries automatiques sont
désactivés. La rotation des fichiers retenus exige une nouvelle connexion
préparée ; leur remplacement n'est pas un rechargement transparent.

## Diagnostiquer et interpréter les observations

`doctor().await` lit les deux ancres de namespace et la version du serveur. Il
restitue empreinte d'autorité, métadonnées observées et écarts d'admission. Un
label Pod Security est une configuration observée, pas une preuve de
confinement. `require_workload_admission()` refuse l'admission.

`observe_owned(handle).await` exige un handle retenu par le contrôle, lié à
l'autorité, la session, l'opération, l'UID exact du Pod et l'engagement de sa
spec après admission. L'intégration doit établir la propriété avant de retenir
ce handle. Un changement d'UID, de spec, de namespace ou l'injection d'un
conteneur supplémentaire est refusé. Les lectures successives ne forment pas
un snapshot Kubernetes atomique. Un 404 est une erreur API ; il ne prouve pas
l'arrêt d'un worker et ne justifie pas une mesure terminale d'usage.

État du processus, redémarrages, identifiants d'image/conteneur et dates gardent
leur valeur observée ou inconnue. Les limites CPU/RAM configurées ne sont pas
une consommation mesurée. Chaque requête a un timeout de huit secondes,
trois secondes pour la connexion et une réponse limitée à 1 Mio, avec quatre
requêtes simultanées par instance runtime. Un diagnostic fait plusieurs
requêtes : huit secondes n'est pas sa borne totale.

## Vérification et travail restant

Les cibles de régression produit sont :

```sh
cargo test --locked --test kubernetes_adapter -- --test-threads=1
cargo test --locked --test kubernetes_transport -- --test-threads=1
cargo test --locked --test kubernetes_transport_lifetime -- --test-threads=1
cargo test --locked --test kubernetes_cli -- --test-threads=1
cargo test --locked --test kubernetes_preflight -- --test-threads=1
cargo test --locked --test kubernetes_preflight_cli -- --test-threads=1
```

Elles exercent le vrai transport HTTPS Rust contre des fixtures TLS locales,
dont mTLS et annulation observée par le pair. Elles ne qualifient ni Docker
Desktop ni un autre cluster. Consulter les
[preuves datées](../../engineering/VALIDATION.md) pour les sources, dates et
portées de chaque exécution ; les anciens diagnostics ne qualifient pas le préflight.

L'admission des workers exige encore transfert du workspace, exec/attach,
audit du lancement/autorisation de start, échéances indépendantes, reprise et
tests de confinement réels. La présence d'une NetworkPolicy ne prouve notamment pas que
le CNI local isole workers et tenants. Ce diagnostic ne modifie aucune
configuration du cluster, pas plus que le préflight. La qualification réelle
Docker Desktop single-node, dont les effets CNI/RBAC/admission, reste `NOT_RUN`.
