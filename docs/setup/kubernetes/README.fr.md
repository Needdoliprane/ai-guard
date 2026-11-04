# Kubernetes : autorité et inspection

[English](README.md) · [Français](README.fr.md) · [Index de configuration](../README.fr.md)

La cible locale de qualification est le cluster Kubernetes à un nœud de Docker
Desktop. Utiliser Docker/Podman pour les sessions d'agents : l'adaptateur Rust
Kubernetes fournit actuellement diagnostic HTTPS et inspection, pas le
lancement de workers. Il n'existe pas d'option de lancement
`--runtime kubernetes` ni de profil dotenv Kubernetes.

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
```

Elles exercent le vrai transport HTTPS Rust contre des fixtures TLS locales,
dont mTLS et annulation observée par le pair. Elles ne qualifient ni Docker
Desktop ni un autre cluster. Voir les
[preuves datées](../../engineering/VALIDATION.md#3-novembre--diagnostic-kubernetes-et-durée-de-vie-du-transport).

L'admission des workers exige encore transfert du workspace, exec/attach,
audit obligatoire du contrôle, échéances indépendantes, reprise et tests de
confinement réels. La présence d'une NetworkPolicy ne prouve notamment pas que
le CNI local isole workers et tenants. Ce diagnostic ne modifie aucune
configuration du cluster.
