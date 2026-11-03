# Kubernetes : autorité et inspection

[English](README.md) · [Français](README.fr.md) · [Index de configuration](../README.fr.md)

La cible locale de qualification est le cluster Kubernetes à un nœud de Docker
Desktop. Utiliser Docker/Podman pour les sessions d'agents : l'adaptateur Rust
Kubernetes fournit actuellement diagnostic HTTPS et inspection, pas le
lancement de workers. Il n'existe pas d'option de lancement
`--runtime kubernetes` ni de profil dotenv Kubernetes.

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
