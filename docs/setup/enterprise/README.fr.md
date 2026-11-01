# Déploiement en entreprise

[English](README.md) · [Index d'installation](../README.fr.md) · [Installation locale](../local/README.fr.md) · [Fournisseurs](../providers/README.fr.md)

Déployer le même contrôle Rust avec l'identité, les règles tenant, le réseau de confiance et les services IA de l'organisation. Conserver un dashboard autorisé par tenant pour tous ses workers et brokers. Les services de contrôle partagés ne sont pas des workers clients et une installation commune n'autorise pas les échanges entre tenants.

## Définir les frontières de l'installation

Consigner ces choix avant de créer des credentials ou de lancer des tâches. Les valeurs sont propres au déploiement ; le dépôt ne fournit ni noms d'hôte entreprise, ni autorité d'AC, ni accès NAS, ni administrateur d'identité.

| Décision | Éléments à consigner |
|---|---|
| Topologie | Hôte/plateforme de contrôle, endpoint Docker ou Podman, propriétaire des brokers, données persistantes et mode de déploiement. |
| Identité humaine | Comptes locaux nominatifs ou issuer OIDC, identifiants de clients publics Web/CLI, URI de retour exactes, audiences et mapping des claims vérifiés. |
| Organisations et tenants | Autorité d'appartenance, permissions personnelles/partagées/admin, bases par tenant et politique de ressources/rétention. |
| Clients de développement | Codex/Claude Code, abonnement/API/inférence interne, profils explicites, images et versions épinglées. |
| Moteur de décision | Rust, Jev réel ou modèle local Jev-like, politique et comportement en cas de panne. |
| Transport | URL publiques, noms DNS/SAN, terminaison TLS, chaînes publiques de confiance, identité de service et destinations autorisées. |
| Exploitation | Responsable sauvegarde/restauration, rotation, mises à jour, incidents, export d'audit et alertes disque/ressources. |

Topologie locale, SSO et inférence sont des axes indépendants. Un modèle interne n'exige pas un IdP et un IdP externe n'autorise pas le trafic vers un modèle externe. Refuser une combinaison incompatible plutôt que d'affaiblir silencieusement TLS, isolation ou audit.

Utiliser la procédure Docker/Podman à hôte unique ci-dessous comme socle. Kubernetes exige son adaptateur runtime, ses identités de workload, ses transports et sa reprise ; une métadonnée `runtime=kubernetes` ou `deployment_mode=cloud` ne crée ni pods ni infrastructure. Ajouter la haute disponibilité ou changer le stockage SQLite par tenant exige une migration documentée, pas un deuxième processus visant les mêmes fichiers.

## Préparer les images et la configuration privée

Construire le CLI release et les images selon le [guide local](../local/README.fr.md), avec les versions exactes des clients nécessaires. Hors ligne, construire ou importer à l'avance toutes les images, tous les modèles et toutes les dépendances ; conserver un inventaire des versions, digests et licences. Utiliser son propre registre ou processus de transfert hors ligne. Aucun runtime ne doit télécharger un remplacement ou basculer implicitement vers un service cloud public.

Préparer un répertoire privé appartenant au compte de contrôle de confiance. Le parent doit déjà convenir à ce compte ; ne pas résoudre un problème de propriétaire en exécutant toutes les opérations en root. À la racine du dépôt, initialiser puis créer un nouveau fichier env :

```sh
AIGUARD_BIN="$PWD/target/release/agentsandbox"
AIGUARD_CONFIG="/srv/ai-guard/config"
"$AIGUARD_BIN" init --config-dir "$AIGUARD_CONFIG"
(
  umask 077
  set -C
  cat .env.exemple.entreprise > "$AIGUARD_CONFIG/enterprise.env"
)
```

Adapter ce fichier privé au chemin, à l'image et aux ports réels. L'[exemple entreprise](../../../.env.exemple.entreprise) configure le **transport Compose**, avec URL/modèle Jev non secrets facultatifs. Il ne crée ni utilisateurs, ni appartenances, ni politique OIDC, ni routage de modèle interne. Ceux-ci relèvent de configurations distinctes validées pour identité, tenants et inférence. Ne pas inventer des variables d'environnement en supposant que le CLI les consomme.

Lancer les services de base derrière l'entrée de confiance choisie :

```sh
docker compose --env-file "$AIGUARD_CONFIG/enterprise.env" -f deploy/docker/compose.yaml up -d
docker compose --env-file "$AIGUARD_CONFIG/enterprise.env" -f deploy/docker/compose.yaml ps
"$AIGUARD_BIN" healthcheck --url http://127.0.0.1:8787/healthz --url http://127.0.0.1:4000/healthz
```

Avec Podman, utiliser `podman compose` et `deploy/podman/compose.yaml`. Adapter les URL de contrôle si les ports hôte ont changé. Le socle ne publie que du HTTP loopback ; ne pas l'exposer directement en modifiant l'adresse d'écoute. Placer l'entrée HTTPS de l'organisation dans une architecture de transport explicitement configurée. Garder mêmes fichier env, nom de projet, endpoint moteur et surcharges pour chaque commande de cycle de vie.

## Configurer les tenants et les permissions

Créer des identités tenant et des credentials runner/viewer distincts ; conserver une base SQLite distincte par tenant. L'[exemple public de registre](../../../config/tenants.example.json) contient les tenants A/B, les chemins internes et les quotas exacts. Générer leurs quatre credentials distincts et créer une nouvelle copie privée du registre :

```sh
"$AIGUARD_BIN" init --config-dir "$AIGUARD_CONFIG/tenant-a"
"$AIGUARD_BIN" init --config-dir "$AIGUARD_CONFIG/tenant-b"
(
  umask 077
  set -C
  cat config/tenants.example.json > "$AIGUARD_CONFIG/tenants.json"
)
```

Le gateway sélectionne le registre opérateur de confiance par `--tenants-file`. Chaque entrée suit ce schéma version1 ; le fichier public fournit les deux entrées :

```json
{
  "version": 1,
  "tenants": [
    {
      "tenant_id": "tenant-a",
      "data": "/data/tenant-a.sqlite",
      "runner_token_file": "/run/secrets/tenant-a-runner",
      "viewer_token_file": "/run/secrets/tenant-a-viewer",
      "worker_quota": {
        "version": 1,
        "max_active_workers": 3,
        "max_cpu_millis": 6000,
        "max_memory_bytes": 3221225472,
        "max_timeout_seconds": 3600
      }
    }
  ]
}
```

Ces chemins sont **internes au conteneur** gateway. Dans la configuration de déploiement relue, monter `$AIGUARD_CONFIG/tenants.json` en lecture seule à `/run/config/tenants.json`, les quatre projections suivantes comme secrets en lecture seule, et un stockage durable aux chemins des bases :

| Projection privée hôte | Chemin du secret gateway |
|---|---|
| `<config-dir>/tenant-a/mounted-secrets/runner` | `/run/secrets/tenant-a-runner` |
| `<config-dir>/tenant-a/mounted-secrets/viewer` | `/run/secrets/tenant-a-viewer` |
| `<config-dir>/tenant-b/mounted-secrets/runner` | `/run/secrets/tenant-b-runner` |
| `<config-dir>/tenant-b/mounted-secrets/viewer` | `/run/secrets/tenant-b-viewer` |

Ne jamais recopier le même secret vers une autre identité. Remplacer la commande gateway du socle par `gateway --listen 0.0.0.0:8787 --container-listen --tenants-file /run/config/tenants.json --policy /app/policies/default.json`, en retirant les options incompatibles mono-tenant `--data`, `--runner-token-file`, `--viewer-token-file` et les quotas locaux. Remplacer ses montages de secrets mono-tenant par les quatre projections. Le mode `--shared-tenants` du dashboard transmet le périmètre de chaque viewer autorisé au lieu d'utiliser un token viewer fixe ; retirer son option `--viewer-token-file` et le montage fixe devenu inutile en choisissant ce mode. Ne pas combiner des configurations mutuellement exclusives. Conserver les fichiers de déploiement relus pour chaque arrêt/démarrage ; la commande de base précédente sans adaptation lance une pile mono-tenant.

Le registre décrit credentials techniques tenant et quotas, pas les appartenances humaines. Les permissions humaines proviennent séparément de l'identité vérifiée. Un utilisateur consulte ses sessions et les ressources explicitement partagées ; métadonnées de tout le tenant, contenus sensibles, exports, administration et exécution sont des capacités distinctes. Le tenant vient de l'appartenance authentifiée, jamais d'un header libre, d'une query, d'un corps de requête ou d'un label worker. Appliquer les restrictions dans Rust aux listes, détails, exports et flux. Un changement de droits doit retirer les données devenues interdites des vues et caches.

Garder `runner` dans le contrôle de confiance ; ni rôle IdP ni utilisateur navigateur ne le reçoit. Les workers n'utilisent que leur identité bornée d'exécution/relais. Identités humaines, brokers, workers et fournisseurs IA restent distinctes. Tracer initiateur, acteur effectif et causalité sans prendre une action d'agent pour la preuve d'une instruction humaine explicite. Les événements historiques à clé partagée conservent leur provenance.

Définir les quotas au niveau du tenant, pas une fois par broker. L'exemple admet trois workers ; une vérification locale utile comporte A1/A2 chez A et B1 chez B. Le plafond de configuration de 100 workers par tenant est une cible de dimensionnement, pas une garantie de capacité sur tout hôte. Nombre de workers, allocation CPU/RAM et durée d'admission ont des limites distinctes. Un changement de quota n'arrête pas à lui seul un worker existant. Le temps d'allocation n'est ni du CPU consommé mesuré, ni un prix, ni une facture ; conserver `billable=false` pour cette mesure.

## Raccorder l'identité locale ou un IdP entreprise

L'identité locale nominative conserve permissions et attribution sans dépendre d'Internet. Activer OIDC lorsque l'organisation choisit le SSO. Utiliser le [guide Keycloak/OIDC](../keycloak/README.fr.md) pour configurer le fournisseur ; realm et comptes publics de découverte constituent un contexte séparé opt-in et ne doivent jamais initialiser une installation normale.

Pour OIDC entreprise, configurer deux clients **publics** Web et CLI avec Authorization Code + PKCE S256. Ne distribuer aucun `client_secret`. Enregistrer URI de retour exactes et origines permises ; éviter les callbacks à jokers larges. Rust possède vérificateurs privés éphémères, corrélation state/nonce, échange du code et validation des jetons. La connexion Web doit revenir à la transaction du navigateur initiateur et à la vue locale demandée, en consommant le callback une seule fois. Le frontend ne s'autorise pas avec un contenu de jeton non vérifié.

Utiliser issuer et subject comme identité externe stable. Vérifier signature, issuer, audience prévue, validité et claims pertinents du jeton d'accès avant de mapper organisations, tenants et permissions. Configurer le mapping pour l'IdP au lieu de supposer un nom de rôle universel. Un administrateur du realm ou un compte nommé `admin-ai-guard` n'est pas automatiquement administrateur ai-guard. Dans l'acceptation du déploiement, vérifier explicitement utilisateur ordinaire, administrateur autorisé, tenant interdit et appartenance révoquée.

Le même issuer doit être joignable sous la même identité depuis navigateur, CLI et composant Rust chargé de discovery, JWKS et échange du code. Préparer DNS et routes internes/externes en conséquence ; une URL visible du navigateur ne prouve pas l'accès depuis un conteneur. Le réseau de contrôle est interne par défaut. Son raccord à l'IdP exige une décision réseau explicite, des AC publiques de confiance et des destinations approuvées bornées. Ne pas ajouter une sortie Internet générale seulement pour résoudre un issuer. Des JWKS publics permettent la vérification sans distribuer de clés privées de signature, mais ne remplacent pas la route d'échange du code.

## Installer TLS de confiance et les identités de service

Faire émettre les certificats par l'opérateur PKI pour les vrais noms et usages des endpoints. Consigner SAN, chaîne émettrice, usages de clé, validité, renouvellement et révocation. Distribuer uniquement la **chaîne publique d'AC** aux magasins de confiance navigateur, contrôle et worker concernés. Les clés privées d'AC restent auprès de l'AC ; les workers ne les reçoivent jamais. Une AC locale de démonstration ne remplace pas la politique d'émission entreprise.

Maintenir la vérification TLS pour IdP, modèles et contrôle. Ne pas utiliser `--insecure`, désactiver les certificats ou remplacer un nom DNS par une IP sans rapport. Vérifier nom prévu et chaîne complète depuis chaque appelant réel. Le bundle public doit atteindre le client qui ouvre la connexion ; l'installer sur l'hôte ne configure pas un conteneur ou un CLI fournisseur.

Utiliser des identités de service/workload distinctes pour brokers→contrôle et relais worker, avec périmètre, rotation et révocation. mTLS authentifie un pair de service ; il ne lui accorde pas à lui seul de rôle humain ou d'accès tenant. Ne jamais envoyer de jetons SSO humains ou de credentials runner globaux aux workers. Terminaison, sauts internes, proxies et listes de destinations doivent former une politique de déploiement explicite ; une entrée HTTPS ne prouve pas le chiffrement de chaque saut.

## Configurer l'IA interne et les services de décision

Pour un modèle de développement interne, consigner URL, modèle/révision exacts, format d'échange, client/adaptateur compatible, **référence** du credential, chaîne publique d'AC et destinations approuvées. Lier les réglages résolus à l'autorisation lorsqu'ils influencent l'exécution. Séparer identité, fournisseur et modèle. Utiliser les [profils fournisseurs](../providers/README.fr.md) pour l'authentification Codex/Claude abonnement/API prise en charge ; ne pas réutiliser un credential modèle comme connexion utilisateur ai-guard.

« Compatible OpenAI » exige un protocole précis : Chat Completions, Responses, streaming/événements et outils ont des contrats distincts. Une réponse Chat Completions ne démontre ni Responses ni Claude Messages. Choisir explicitement l'adaptateur pris en charge et refuser les fonctions incompatibles. Ne pas déduire un changement de routage de variables `OPENAI_BASE_URL`/`ANTHROPIC_BASE_URL` arbitraires dans un dotenv.

Hors ligne, vérifier que client, authentification et modèle choisis fonctionnent sans connexion externe, refresh ou téléchargement. Si un service externe requis est inaccessible, arrêter sur ce prérequis au lieu de basculer vers une API publique payante. Ne fournir aux workers que credential sélectionné, confiance publique et destinations autorisées. Examiner les réglages fournisseur qui peuvent ouvrir connecteurs, outils ou autres upstreams.

Les services de décision restent un choix distinct : règles Rust, Jev réel authentifié ou modèle local Jev-like. Aucun moteur ne peut lever un refus impératif d'audit, d'isolation ou de politique. Pour Jev, suivre la [procédure séparée clé brute/surcharge Compose](../providers/README.fr.md#choisir-le-moteur-de-décision) ; sa sortie réseau par défaut n'est pas filtrée et exige une revue réseau. Ne pas placer la clé Jev dans un worker ou un serveur modèle.

Pour le petit modèle local, suivre le [guide IA locale](../local-ai/README.fr.md), épingler runtime et poids et configurer contexte, sortie, délai, CPU et mémoire conteneur. La famille comporte au plus 4B paramètres avec une cible de 500 Mo–1 Go de RAM ; mesurer pic cgroup et OOM réels pour la configuration retenue. La taille des poids n'est pas la RAM consommée. Valider décisions structurées bornées et incertitude ; une génération de texte réussie ne prouve ni qualité du jugement ni équivalence avec Jev réel.

## Exploiter et reprendre

Avant admission, vérifier disponibilité du contrôle, périmètre authentifié, audit durable disponible, autorité runtime, quota, fournisseur sélectionné et destinations approuvées. Vérifier une tâche autorisée inoffensive et sa trace attribuée, puis un refus de permission attendu. Un endpoint de santé ne remplace pas ces contrôles. Réunir événements et workers du tenant dans un dashboard avec identité du broker, fraîcheur et états partiels/déconnectés.

| Situation | Procédure |
|---|---|
| IdP ou modèle indisponible | Afficher la dépendance en erreur et conserver refus/état inconnu. Séparer le mode local autorisé de la politique SSO ; ne pas contourner une connexion exigée ni changer de modèle/moteur implicitement. |
| Broker déconnecté | Présenter sa dernière observation comme périmée. Conserver échéances/claims durables et réconcilier son autorité avant un autre démarrage. L'absence d'événements frais n'est pas une activité nulle. |
| Arrêt demandé | Persister `worker-stop`, puis contrôler l'état terminal observé. Un reçu d'intention durable ne prouve pas l'arrêt du processus. Conserver les issues inconnues sans faux succès. |
| Credential ou appartenance révoqués | Empêcher les nouvelles actions interdites, invalider accès/flux concernés et appliquer la politique de révocation d'exécution. Renouveler par le propriétaire ; ne pas réécrire l'audit. |
| Limite disque/audit atteinte | Bloquer les actions dont l'audit obligatoire ne peut être persisté. Appliquer rétention/export approuvés ; ne pas effacer de preuves pour réussir l'admission. |
| Export ou restauration incertains | Conserver source et destination publiée. Réconcilier identités/claims avant réactivation ; ne pas rejouer un démarrage consommé. |

Appliquer des politiques distinctes d'accès/rétention aux contenus utilisateur, payloads du relais, logs et métadonnées d'audit. Masquer les secrets avant persistance ou diffusion. Points de contrôle signés et vérification d'export conservent les preuves indépendantes ; l'ancrage externe dépend de la politique configurée. Signaler explicitement toute donnée manquante, tronquée ou partiellement observée.

Pour sauvegarde ou mise à jour, arrêter les admissions, réconcilier les workers, arrêter les services de façon cohérente et conserver données SQLite/état WAL de chaque tenant avec configuration, plans, bindings et volumes correspondants. Chiffrer/restreindre les sauvegardes selon leur contenu. Ne pas placer une base SQLite WAL active sur un NAS comme un fichier partagé ordinaire. Un NAS peut être une destination de sauvegarde explicitement autorisée sans devenir base active ou service PKI supposé.

Restaurer dans une destination contrôlée et vérifier compatibilité schéma/version, continuité d'audit, credentials, identité broker/runtime, échéances et claims consommés avant d'autoriser l'exécution. Une autorité inconnue reste indisponible pour l'exécution jusqu'à réconciliation ; restaurer une base ne donne pas un nouveau budget de démarrage. Conserver un point de retour avant mise à jour et utiliser une procédure explicite compatible ; une migration en erreur n'implique pas un rollback automatique sûr.

Pour le socle Docker, un arrêt planifié conserve les données :

```sh
docker compose --env-file "$AIGUARD_CONFIG/enterprise.env" -f deploy/docker/compose.yaml stop
```

Inclure toutes les surcharges relues utilisées au démarrage, dont Jev si choisi ; utiliser Podman lorsque pertinent. Redémarrer avec mêmes projet/env/fichiers, vérifier disponibilité et reprise, puis admettre de nouvelles tâches. Éviter `down -v`, `prune`, écrasement d'un realm/configuration existants et suppression d'une base comme procédure de réparation.
