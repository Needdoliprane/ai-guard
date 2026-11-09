# Configurer et utiliser le SSO ai-guard

[English](README.md) · [Index d’installation](../README.fr.md) · [Fournisseur Keycloak](../keycloak/README.fr.md) · [Accès local nominatif](../identity/README.fr.md)

Ce guide décrit la configuration OIDC Rust, la connexion Web et le CLI humain au premier plan. Utilisez-le avec le binaire ai-guard correspondant et le frontend compilé. Il ne configure pas l’authentification Codex ou Claude : choisissez séparément leur abonnement, clé API ou endpoint d’inférence approuvé dans le [guide fournisseurs](../providers/README.fr.md).

Déployez des versions correspondantes du gateway, du dashboard compilé et du CLI natif pour adopter le champ obligatoire `contexts_complete` des réponses d’identité humaine. Un nouveau client strict refuse un champ absent ou mal typé ; un ancien client strict peut refuser le champ supplémentaire. Coordonnez la mise à jour des composants en conservant configuration, credentials et audits existants. Cette évolution du contrat de réponse n’ajoute aucune migration de base et ne justifie pas de remplacer un store existant.

Les exemples activent aussi l’accusé de déconnexion fournisseur via `web_post_logout_redirect_uri` et `cli_post_logout_redirect_uri`. Installez des versions mises à jour correspondantes du gateway et du CLI natif avant d’activer ces champs : les anciens binaires refusent les champs de configuration/profil inconnus. Les configurations existantes qui omettent ces deux options conservent le comportement de déconnexion antérieur. Suivez la [procédure de mise à jour de configuration](#mettre-à-jour-la-configuration-logout-sans-remplacer-les-données) pour une installation existante.

Choisissez la **DEMO locale neuve** ci-dessous pour préparer les nouvelles données du contrôle et les trois comptes applicatifs importés, ou la [procédure manuelle](#préparer-une-installation-existante-ou-dentreprise) avec un fournisseur existant. Utilisez une image de contrôle actuelle et son frontend compilé correspondants ; le [suivi d’ingénierie](../../engineering/VALIDATION.md#2-novembre--sso-web-réel-sur-la-page-de-fin-corrigée) distingue les parcours Web connexion/lecture/logout observés des trois comptes des qualifications CLI natif, refus de permissions et cycle de session restantes. Un mapping de nom propre au navigateur ne prouve pas la résolution DNS hôte nécessaire au CLI natif. Préparation et résultats historiques gardent leur périmètre daté ; le [contrat d’identité](../../sso.md) décrit la cible produit complète.

## Préparer une DEMO locale neuve

Ce parcours utilise un nouveau projet Keycloak, un nouveau projet ai-guard, deux images construites localement et un nouveau volume privé de contrôle. Il crée le stockage tenant schéma5, l’audit d’authentification séparé, les credentials techniques, comptes et memberships du manifeste public [local-demo.json](examples/local-demo.json). Il ne crée ni sessions, grants de ressources, activité worker ou délégation d’exécution humaine. Une liste personnelle de ressources autorisées vide après préparation est attendue.

Les entrées [Docker](../../../deploy/docker/compose.sso.yaml) et [Podman](../../../deploy/podman/compose.sso.yaml) sont des piles autonomes explicitement activées. Ne les combinez pas avec l’ancien `deploy/*/compose.yaml` ou ses montages de secrets hôte. Le conteneur de préparation n’a pas de réseau ; seul le gateway rejoint ensuite le réseau interne d’identité sélectionné. Dashboard et workers ne le rejoignent pas. Aucun conteneur de préparation ou applicatif ne reçoit de socket moteur ou le projet exécuté.

### 1. Vérifier les prérequis et choisir des noms neufs

Utilisez une installation locale Docker ou Podman prise en charge, Compose disponible, un terminal et le checkout source actuel. Dépendances de compilation et images de base doivent être disponibles ; préparez-les avant un usage hors ligne. Node sert à construire le frontend dans l’image, pas à fournir un serveur de production. L’image de contrôle précédemment validée commençant par `954246…` précède `setup prepare` : construisez une nouvelle image depuis les sources contenant cette commande. Aucune image publique ai-guard préconstruite n’est requise.

Avant démarrage, faites résoudre `keycloak.ai-guard.test` vers `127.0.0.1` par votre configuration DNS hôte habituelle pour navigateur et CLI. Vérifiez cette résolution sur le poste. Ce guide ne modifie ni DNS ni confiance OS. L’alias du réseau d’identité donne au conteneur son propre trajet vers le même issuer ; ne remplacez pas celui-ci par `localhost`, un nom de service ou une autre IP.

Les ports 8787, 4000 et 18080 doivent être libres sur le loopback hôte. Le preset neuf fixe ces cinq paramètres publics :

| Variable | Valeur requise |
|---|---|
| `AI_GUARD_DEMO_IDP_HOST` | `keycloak.ai-guard.test` |
| `AI_GUARD_DEMO_IDP_PORT` | `18080` |
| `AI_GUARD_DEMO_WEB_ORIGIN` | `http://127.0.0.1:4000` |
| `AI_GUARD_GATEWAY_PORT` | `8787` |
| `AI_GUARD_DASHBOARD_PORT` | `4000` |

`--demo-defaults` vérifie ces valeurs et refuse les surcharges incompatibles ; il ne réécrit pas les endpoints. Le démarrage normal conserve les ports/origine fixes du Compose SSO. Pour une autre topologie, préparez un manifeste générique relu et les services correspondants plutôt que de modifier seulement ces valeurs annoncées.

Depuis la racine du dépôt, sélectionnez moteur, Compose SSO correspondant et nouveaux noms. Les exemples emploient Docker ; pour Podman, choisissez ensemble `AIGUARD_ENGINE="podman"` et `AIGUARD_SSO_COMPOSE="deploy/podman/compose.sso.yaml"`. Conservez ces choix pendant tout le cycle de vie :

```sh
AIGUARD_ENGINE="docker"
AIGUARD_SSO_COMPOSE="deploy/docker/compose.sso.yaml"
AIGUARD_SSO_PROJECT="ai-guard-sso-fresh"
AIGUARD_IDP_PROJECT="ai-guard-keycloak-sso-fresh"
AIGUARD_SSO_ENV="/absolute/private/ai-guard-demo/sso.env"
```

Vérifiez qu’aucun des noms de projet n’appartient à des ressources existantes. Les filtres et le format d’inspection ci-dessous emploient les labels Docker Compose. Avec Podman, vérifiez les labels de projet/réseau et champs d’inspection du fournisseur Compose réel, puis adaptez les commandes d’inventaire s’ils diffèrent ; un filtre vide sans correspondance ne prouve pas qu’un nom est libre. Le déploiement Podman demande sa propre qualification réelle.

```sh
"$AIGUARD_ENGINE" ps -a --filter "label=com.docker.compose.project=$AIGUARD_SSO_PROJECT"
"$AIGUARD_ENGINE" volume ls --filter "label=com.docker.compose.project=$AIGUARD_SSO_PROJECT"
"$AIGUARD_ENGINE" network ls --filter "label=com.docker.compose.project=$AIGUARD_SSO_PROJECT"
"$AIGUARD_ENGINE" ps -a --filter "label=com.docker.compose.project=$AIGUARD_IDP_PROJECT"
"$AIGUARD_ENGINE" volume ls --filter "label=com.docker.compose.project=$AIGUARD_IDP_PROJECT"
"$AIGUARD_ENGINE" network ls --filter "label=com.docker.compose.project=$AIGUARD_IDP_PROJECT"
```

Si un nom est occupé, choisissez-en un autre ; ne supprimez pas les ressources. Créez un **nouveau** fichier opérateur à `AIGUARD_SSO_ENV`, en copiant [`.env.exemple.local-keycloak-sso`](../../../.env.exemple.local-keycloak-sso) sans écraser un fichier existant. Son parent doit déjà être privé. Ce fichier contient des paramètres publics de déploiement ; gardez-le hors d’un workspace exécuté. Utilisez partout les mêmes options `--env-file` et `-p` explicite. L’environnement du terminal prime sur l’interpolation Compose : retirez ou résolvez les surcharges contradictoires avant de continuer ; ne faites pas `source` du fichier.

### 2. Construire vos images de contrôle et d’identité

Choisissez des tags locaux inutilisés pour préserver la référence d’image d’une installation existante. L’exemple env public sélectionne :

```dotenv
AI_GUARD_CONTROL_IMAGE=agentsandbox-control:local-sso
AI_GUARD_DEMO_CONTROL_IMAGE=agentsandbox-control:local-sso
AI_GUARD_DEMO_IDP_IMAGE=ai-guard-keycloak-demo:local-sso
```

Si un tag est occupé, choisissez-en un nouveau dans le fichier env neuf et les commandes de build. Les deux premiers réglages doivent désigner votre nouvelle image de contrôle ; le second fournit le relais IdP à destinations fixes. Construisez les deux images depuis ce checkout :

```sh
"$AIGUARD_ENGINE" build -f images/control.Dockerfile -t agentsandbox-control:local-sso .
"$AIGUARD_ENGINE" build -f deploy/keycloak-demo/Dockerfile -t ai-guard-keycloak-demo:local-sso deploy/keycloak-demo
```

Le [Dockerfile de contrôle](../../../images/control.Dockerfile) compile Rust et le frontend statique. Le [Dockerfile Keycloak](../../../deploy/keycloak-demo/Dockerfile) épingle Keycloak 26.8.1 et son digest amont, puis copie le realm de ce checkout dans l’image locale pour son import au démarrage. Une ancienne image locale n’inclurait pas les sujets DEMO explicites. Conservez les IDs réels des images dans l’inventaire d’installation. `AI_GUARD_DEMO_IDP_IMAGE` sélectionne le tag IdP local ; les anciens profils qui l’omettent conservent leur défaut précédent.

### 3. Démarrer le nouvel IdP et relever son réseau

Utilisez le projet séparé et le profil HTTP DEMO explicite :

```sh
"$AIGUARD_ENGINE" compose --env-file "$AIGUARD_SSO_ENV" -p "$AIGUARD_IDP_PROJECT" \
  -f deploy/keycloak-demo/compose.yaml --profile keycloak-demo-http \
  up -d --no-build --pull never keycloak-demo keycloak-demo-ingress
"$AIGUARD_ENGINE" compose --env-file "$AIGUARD_SSO_ENV" -p "$AIGUARD_IDP_PROJECT" \
  -f deploy/keycloak-demo/compose.yaml --profile keycloak-demo-http ps
"$AIGUARD_ENGINE" network ls --filter "label=com.docker.compose.project=$AIGUARD_IDP_PROJECT"
```

Inspectez les noms de réseau retournés ; donnez le nom réel à cette commande, sans inventer de nom dérivé du projet :

```sh
"$AIGUARD_ENGINE" network inspect ACTUAL_NETWORK_NAME \
  --format '{{.Name}} internal={{.Internal}} logical={{index .Labels "com.docker.compose.network"}}'
```

Sélectionnez le réseau dont le label logique Compose vaut `identity_demo` et la valeur `Internal` est vraie, puis inscrivez son `.Name` exact dans `AI_GUARD_SSO_IDENTITY_NETWORK` du nouveau fichier env. Ne choisissez ni `publication_demo` ni le réseau d’une autre installation. Le Compose SSO le traite comme réseau externe et refuse un sélecteur vide. Il ne déduit ni ne crée le réseau d’identité à partir du nom de projet.

L’issuer exact est `http://keycloak.ai-guard.test:18080/realms/ai-guard-demo` ; son document discovery est à cette URL suivie de `/.well-known/openid-configuration`. L’administration fournisseur est à `http://keycloak.ai-guard.test:18080/admin/`. Gardez cette DEMO HTTP à mots de passe publics sur le loopback hôte. Keycloak reçoit `AI_GUARD_DEMO_WEB_ORIGIN` dans son conteneur pour la substitution d’environnement prise en charge par l’import du realm ; la définir seulement dans le terminal hôte ou omettre l’environnement conteneur ne prépare pas le même realm.

### 4. Préparer une seule fois le volume privé du contrôle

Après sélection du réseau, validez la configuration des services sans diffuser un rendu brut dans des logs partagés :

```sh
"$AIGUARD_ENGINE" compose --env-file "$AIGUARD_SSO_ENV" -p "$AIGUARD_SSO_PROJECT" \
  -f "$AIGUARD_SSO_COMPOSE" config --quiet
"$AIGUARD_ENGINE" compose --env-file "$AIGUARD_SSO_ENV" -p "$AIGUARD_SSO_PROJECT" \
  -f "$AIGUARD_SSO_COMPOSE" --profile sso-prepare run --no-deps --pull never -T \
  setup setup prepare --input - --output-dir /data/installation --demo-defaults \
  < docs/setup/sso/examples/local-demo.json
```

Le premier `setup` sélectionne le service Compose ; le second est la sous-commande Rust après l’entrypoint de l’image. La redirection transmet le **manifeste public**, aucun fichier de credentials. N’utilisez pas de terminal/TTY ou collage manuel : stdin est limité à 262144 octets et doit se terminer en 5 secondes. Le processus utilise UID/GID 1000:1000 et un nouveau volume `installation_data` dont le parent `/data` est privé. Il crée les fichiers privés sous cette identité au lieu de monter un OIDC 0600 appartenant à un autre UID hôte. La préparation ne contacte ni réseau/fournisseur/moteur/modèle.

Continuez seulement après un exit 0 et le statut JSON `prepared` pour `/data/installation`. Conservez le conteneur de préparation et son résultat pour diagnostic. Une préparation réussie publie le répertoire final après ses étapes durables. Ses principaux fichiers sont :

| Emplacement sous `/data/installation` | Fonction |
|---|---|
| `tenants.json`, `oidc.json` | Registre gateway privé et configuration OIDC référençant les chemins finaux. |
| `authentication.sqlite` | Audit d’authentification séparé initialisé. |
| `tenants/<tenant>/audit.sqlite` | Stockage lié au tenant, schéma5, avec sauvegardes de migration et reçus d’identité conservés. |
| `tenants/<tenant>/runner`, `viewer` | Credentials techniques générés ; jamais credentials de navigateur ou worker. |
| `preparation.json`, `completion.json` et enregistrements d’origine/liaison | Identités de reprise, empreintes, liaisons et reçus de fin privés. |
| `secret-material.json` | **Matériel privé de reprise contenant les credentials générés. Ne jamais l’afficher, le journaliser, l’exporter dans un rapport ou le distribuer.** |

Les fichiers restent 0600 et les répertoires 0700 sous l’UID du contrôle. Préservez toute l’installation privée et son matériel de reprise. N’exécutez pas `init` dessus, ne répétez pas `prepare` sur la destination finale et ne régénérez pas des credentials manquants. La reprise est décrite plus bas.

### 5. Démarrer ai-guard et utiliser les comptes préparés

Le démarrage normal rouvre les bases tenants terminées et l’audit d’authentification. Il ne répète pas le provisioning et ne porte **aucun drapeau `--initialize-oidc-audit`** :

```sh
"$AIGUARD_ENGINE" compose --env-file "$AIGUARD_SSO_ENV" -p "$AIGUARD_SSO_PROJECT" \
  -f "$AIGUARD_SSO_COMPOSE" up -d --no-build --pull never gateway dashboard ingress
"$AIGUARD_ENGINE" compose --env-file "$AIGUARD_SSO_ENV" -p "$AIGUARD_SSO_PROJECT" \
  -f "$AIGUARD_SSO_COMPOSE" ps
```

Ouvrez `http://127.0.0.1:4000` pour le dashboard, ou utilisez le gateway `http://127.0.0.1:8787` depuis le CLI au premier plan décrit plus bas. Un conteneur sain n’est pas une connexion réussie. Terminez une vraie connexion avant de conclure au fonctionnement du trajet IdP ; une erreur n’est pas une liste d’activité vide.

| Compte/mot de passe public DEMO | Accès applicatif préparé |
|---|---|
| `admin / admin` | Administration **master** Keycloak seulement. Aucun compte ou membership ai-guard préparé pour lui. |
| `admin-ai-guard / admin-ai-guard` | Compte applicatif DEMO avec membership explicite incluant `read_tenant_metadata` ; le rôle client vérifié correspondant doit aussi satisfaire le mapping serveur. |
| `user-1 / user-1` | Contexte personnel DEMO, capacités métadonnées/contenu/export ; les ressources demandent des grants distincts. |
| `user-2 / user-2` | Mêmes capacités personnelles, sans capacité d’administration applicative. |

Les trois sujets applicatifs sont des IDs explicites partagés par l’import du realm neuf et `local-demo.json` ; ils ne sont pas déduits des noms de connexion. Leur import a été vérifié avec le CLI officiel Keycloak sur un realm neuf. Les realms existants ne sont **ni** réinitialisés, adoptés ou migrés par cette procédure ; l’import de démarrage ignore un realm existant. Reconstruire l’image ne change pas ses IDs sujets persistés. Pour un realm existant, fournissez un manifeste relu contenant ses vrais `(issuer, sub)` vérifiés et utilisez la préparation générique dans une autre destination neuve. Ne donnez pas de membership par correspondance de nom et ne recopiez pas les IDs de la DEMO neuve dans un realm sans rapport.

### Arrêter, redémarrer et reprendre une préparation échouée

Conservez moteur, fichier env, noms de projet, images, réseau externe et volumes. Arrêtez seulement les services de cette installation :

```sh
"$AIGUARD_ENGINE" compose --env-file "$AIGUARD_SSO_ENV" -p "$AIGUARD_SSO_PROJECT" \
  -f "$AIGUARD_SSO_COMPOSE" stop ingress dashboard gateway
"$AIGUARD_ENGINE" compose --env-file "$AIGUARD_SSO_ENV" -p "$AIGUARD_IDP_PROJECT" \
  -f deploy/keycloak-demo/compose.yaml --profile keycloak-demo-http stop
```

Après une installation normalement terminée, reprenez ses services existants avec les mêmes commandes de démarrage ci-dessus. Ne relancez pas la préparation. Un redémarrage gateway perd les admissions humaines vivantes ; navigateur/CLI doivent se reconnecter explicitement. Audit et memberships préparés sont conservés, sans restauration de tokens, autorité d’exécution ou sessions depuis les enregistrements de reprise.

Après un échec de préparation, relevez le répertoire conservé annoncé et arrêtez tous les contrôles/brokers/redémarrages automatiques susceptibles de consommer cette installation avant reprise. Un échec avant publication conserve généralement `/data/.setup-…` ; un échec après publication peut conserver `/data/installation` malgré une erreur de commande. Utilisez le répertoire exact annoncé/existant, pas un hash deviné. Remplacez la commande du service de préparation pour reprendre le matériel conservé :

```sh
"$AIGUARD_ENGINE" compose --env-file "$AIGUARD_SSO_ENV" -p "$AIGUARD_SSO_PROJECT" \
  -f "$AIGUARD_SSO_COMPOSE" --profile sso-prepare run --no-deps --pull never -T \
  setup setup resume --recovery-dir /data/RETAINED_DIRECTORY_FROM_ERROR --confirm-control-stopped
```

`--confirm-control-stopped` est une attestation opérateur, pas un mécanisme de fencing. La reprise utilise les fichiers conservés exacts, leurs liaisons inode/propriétaire, credentials et identités d’opération, sur les **mêmes volume, UID et chemin `/data`**. Déplacer ou copier l’arbre de reprise ne conserve pas ces liaisons. Elle ne relit pas un nouveau manifeste, ne régénère pas de secret et n’adopte pas un répertoire final sans rapport. Intention manquante/partielle, fichier substitué, staging et destination finale présents ensemble ou reçu contradictoire imposent de préserver le matériel et résoudre la cause signalée. Ne supprimez pas de répertoire/volume, n’éditez pas le JSON de reprise, n’utilisez pas `down -v` et ne rejouez pas `prepare` pour forcer une réussite. Si une intention incomplète impose une autre destination, préservez l’ancien matériel et préparez délibérément une installation distincte ; le chemin Compose fixe doit alors être adapté explicitement.

## Comprendre les connexions

| Composant | Responsabilité |
|---|---|
| Gateway Rust | Possède les transactions PKCE Web, valide les réponses du fournisseur et les access tokens API, admet les sessions, applique mapping et permissions du contrôle, puis conserve l’audit d’authentification. |
| Dashboard Rust | Sert le frontend compilé et relaie uniquement des routes d’authentification fixes vers son gateway configuré. Aucun cookie de session métier n’authentifie l’utilisateur. |
| Navigateur | Utilise le client Web public. Conserve les access/refresh tokens en mémoire après leur remise unique par le gateway. Un cookie opaque bref corrèle la connexion ; il ne contient ni token ni vérificateur PKCE. |
| CLI humain | Utilise un client public distinct, un navigateur externe et un listener loopback éphémère. Possède son échange PKCE ; le gateway valide et admet indépendamment l’access token. |
| Broker et worker | Gardent leurs identités d’exécution distinctes. Une connexion SSO humaine ne fournit aucun credential `runner` et ne place aucun token humain dans un worker. |

Le SSO permet actuellement la consultation et l’export humains autorisés. Ne transmettez pas son token à `broker submit` et ne déduisez pas d’une connexion le droit de lancer un agent. La délégation d’exécution humaine et la vue complète d’administration restent des exigences produit distinctes.

## Préparer une installation existante ou d’entreprise

Les sections suivantes sont le parcours manuel pour un stockage existant ou un fournisseur approuvé par l’opérateur. Elles constituent une alternative à la DEMO neuve ci-dessus. Préservez les identités et données réelles de cette installation ; n’y importez pas les utilisateurs publics DEMO.

1. Préparez les images gateway/dashboard et le frontend compilé selon le [guide local](../local/README.fr.md), ou le déploiement d’entreprise approuvé. Préservez registre de tenants, fichiers de credentials, politique, volumes de données et identité des services existants.
2. Arrêtez gateway, brokers et redémarrages automatiques avant la maintenance d’identité hors ligne. `--confirm-control-stopped` est une attestation de l’opérateur ; ce drapeau ne bloque pas un autre processus.
3. Migrez explicitement chaque base tenant configurée vers le schéma5 pour les lectures humaines, avec une nouvelle sauvegarde vérifiée à chaque étape. Suivez la [procédure de stockage](../broker/README.fr.md) et l’[exemple de migration de l’accès humain](../identity/README.fr.md). Ni l’ajout OIDC ni la réouverture du gateway ne migre une base.
4. Obtenez l’issuer exact vérifié et le sujet stable de chaque utilisateur approuvé par la procédure d’administration d’identité. Le principal est `(issuer, sub)`. Un nom de connexion, email, nom affiché ou token collé par l’utilisateur n’établit pas une appartenance approuvée.
5. Inventoriez les accès depuis navigateur, CLI et gateway, ainsi que le trajet dashboard→gateway. Ils doivent conserver la même identité d’issuer. Depuis un conteneur, une adresse loopback hôte désigne ce conteneur ; l’accès du navigateur ne prouve aucun trajet conteneurisé.

Pour une base appartenant à un conteneur, effectuez la maintenance avec l’UID/GID du contrôle existant et son volume existant, tous les autres consommateurs arrêtés. Le [guide d’accès local nominatif](../identity/README.fr.md) décrit le conteneur de maintenance isolé. Les mutations de comptes OIDC emploient la même autorité opérateur et ne nécessitent aucun credential humain local.

### Préparer un stockage neuf pour un fournisseur existant approuvé

`setup prepare` générique peut créer une installation distincte neuve depuis un manifeste relu de sujets réels vérifiés. Il n’adopte jamais une base tenant existante et ne vérifie pas l’IdP en ligne. Son entrée version1 fermée est distincte de l’exemple d’exécution `gateway.https.json` :

| Champ d’entrée | Contenu |
|---|---|
| `version` | `1`. |
| `gateway` | `OidcGatewayConfigV1` approuvé : fournisseur, origine Web exacte, libellé et mapping de rôles. |
| `authentication_policy` | Quotas d’audit et exactement les deux clients publics de l’issuer configuré. |
| `ca_bundle_pem` | Facultatif/null, bundle PEM de certificats publics seuls, au plus 64 Kio / 8 certificats ; aucune clé privée. |
| `tenants` | Tableau de `tenant_id`, `organization_id`, `worker_quota` facultatif et `identities`. |
| Chaque identité | `principal` avec `kind:"human_oidc"`, `issuer` exact approuvé et `subject` vérifié, puis `display_name` et `capabilities`. |

Exécutez sous l’UID réel du contrôle de confiance, vers une nouvelle destination sous un parent privé existant. Forme CLI :

```sh
AIGUARD_BIN="/absolute/path/to/agentsandbox"
"$AIGUARD_BIN" setup prepare --input /absolute/private/ai-guard/approved-manifest.json \
  --output-dir /absolute/private/ai-guard/new-installation
```

Pour un stockage conteneurisé, exécutez cette même commande Rust dans un conteneur ponctuel sans réseau sous l’UID du contrôle et avec son volume privé préparé ; `--input -` et stdin public évitent un bind dont le propriétaire hôte ou le filesystem est incompatible. L’entrée fichier exige toujours les sémantiques locales de filesystem/ACL prises en charge par le contrôle ; une projection non prise en charge est refusée même montée en lecture seule. Stdin conserve validation du manifeste, bornes de taille/durée et contrôles de destination privée. Omettez `--demo-defaults` pour un manifeste générique. Cela n’adapte **pas** les ports, origine ou réseau fournisseur codés dans le Compose DEMO autonome ; utilisez une configuration de services correspondante relue. L’`oidc.json` généré référence `ca-bundle.pem` seulement s’il est fourni. Ce bundle public configure la confiance fournisseur, pas la confiance OS du navigateur ou le trajet CLI→gateway distinct. Un stockage existant suit plutôt les sections de maintenance/mutation explicites ci-dessous.

## Enregistrer les clients publics et l’audience API

Enregistrez deux clients publics Web et CLI distincts, avec Authorization Code et PKCE S256 obligatoire. Ne distribuez aucun `client_secret`. Désactivez les flux implicite et mot de passe. La connexion actuelle demande `openid` ; ce fichier ne configure pas de scopes, algorithmes ou formats de tokens arbitraires.

| Réglage | Valeur requise |
|---|---|
| Issuer | URL exacte, incluant protocole, hôte, port et realm/chemin. Discovery est lu à `<issuer>/.well-known/openid-configuration`. |
| Retour Web | Exactement `<web_origin>/auth/oidc/callback`. L’origine configurée n’a ni chemin ni barre finale. |
| Retour CLI | `http://127.0.0.1:<port-éphémère>/oidc/callback`. Enregistrez l’exception de port loopback natif prise en charge par le fournisseur, avec IP littérale et chemin exact conservés. Le CLI ouvre son listener ; `localhost` et chemins avec joker ne les remplacent pas. |
| Audience API | Identifiant distinct des deux IDs clients, présent dans `aud` de l’access token. |
| Profil d’access token | RS256 ; header `typ=JWT`, `kid` non vide ; claims `typ=Bearer`, `iss`, `sub`, `aud`, entiers positifs `iat` et `exp` ; `azp` est l’un des deux clients publics configurés. `nbf` est contrôlé s’il est présent. Un ID token n’est pas un bearer API. |
| Projection de rôle | Tableau de chaînes administré par l’opérateur dans l’access token vérifié, au chemin de claim configuré. Un attribut de profil modifiable par l’utilisateur ne doit pas conférer de permission. |
| Renouvellement | Rotation/protection contre la réutilisation côté fournisseur et tokens de durée adaptée et courte. ai-guard conserve aussi les générations d’admission et de refresh durablement ; configurez et qualifiez séparément la politique du fournisseur. |
| Endpoint de déconnexion | Endpoint end-session exact facultatif. Le POST privé contient `client_id` et `id_token_hint` ; il n’ouvre pas de page de déconnexion interactive. |
| Retours de déconnexion | Lorsqu’ils sont configurés, enregistrez le `web_post_logout_redirect_uri` HTTPS statique exact sur le client Web et le `cli_post_logout_redirect_uri` sur le client CLI. Ils sont distincts des callbacks de connexion et du port éphémère du CLI. Voir les [accusés de déconnexion](#configurer-les-accusés-de-déconnexion-fournisseur). |

Le profil d’access token actuel est plus étroit qu’OIDC en général. Entra ID ou un autre fournisseur n’est utilisable que si ses tokens signés et réponses discovery satisfont réellement ce profil ; son nom ou sa conformité OIDC générale ne prouve pas sa compatibilité.

## Préparer le JSON privé du gateway

Partez d’un exemple public et copiez-le dans un nouveau fichier privé d’opérateur, hors des workspaces sources, des assets frontend et des exports workers :

| Exemple | Usage |
|---|---|
| [gateway.https.json](examples/gateway.https.json) | Format HTTPS avec adresses `example.invalid` volontairement non fonctionnelles. Remplacez issuer, endpoints, les deux URI de retour logout, IDs clients, audience, origine, règle de rôle et UUID d’installation par les valeurs approuvées. |
| [gateway.keycloak-demo.json](examples/gateway.keycloak-demo.json) | Valeurs exactes par défaut du fournisseur HTTP DEMO isolé distribué. Nécessite une préparation distincte des comptes/memberships et un trajet gateway→fournisseur approuvé. Remplacez l’UUID d’installation d’exemple. |

Ces fichiers contiennent des valeurs publiques d’exemple, aucun credential ni identité existante. Générez un UUID d’installation canonique en minuscules, non nul et neuf pour un nouvel audit. Conservez-le lors des réouvertures de cet audit. Le parseur JSON ne développe **pas** les variables d’environnement : `${NAME}` n’est pas un mécanisme de configuration. Les variables Keycloak des `.env.exemple.*` configurent ses services Compose ; elles n’activent pas le SSO du gateway.

La configuration est en version 1 et refuse les champs inconnus dans chaque objet déclaré. Ses champs racine sont :

| Champ | Signification |
|---|---|
| `version` | Doit valoir `1`. |
| `gateway` | Objet version 1 : `provider`, `provider_label`, `web_origin`, `mapping`. |
| `authentication_audit` | `path`, `installation_id` et `policy`. Base privée d’authentification distincte de chaque base tenant. |
| `ca_bundle_file` | Facultatif/null, ou chemin vers les certificats PEM d’AC **publics** approuvés pour le fournisseur OIDC. |

L’objet provider des exemples montre tous les champs obligatoires. `end_session_endpoint`, `web_post_logout_redirect_uri` et `cli_post_logout_redirect_uri` sont facultatifs/null ; tout retour logout configuré exige `end_session_endpoint`. Omettre `limits` applique les valeurs ci-dessous. `transport` vaut exactement `{"kind":"https_only"}` dans une installation normale. La DEMO isolée utilise `{"kind":"isolated_demo_http","exact_issuer":"..."}` ; cet issuer exact doit correspondre à `issuer`. HTTP est limité aux issuers sur IP loopback littérale ou à l’issuer fixe de la DEMO distribuée. Son origine/redirect Web doit être une adresse loopback HTTP littérale. Cette exception n’approuve pas un fournisseur HTTP intranet arbitraire.

Les endpoints d’autorisation, token, JWKS et logout facultatif sont des URL exactes approuvées par l’opérateur. Discovery doit les confirmer ; il n’ajoute pas de destination. Les URL ne contiennent ni userinfo, query ni fragment. Le client HTTP du fournisseur désactive proxies ambiants, redirects et retries automatiques. Approuvez également DNS et destinations sortantes dans le déploiement : la liste d’URL n’est pas un pare-feu de conteneur.

Les chemins `authentication_audit.path` et `ca_bundle_file` sont absolus ou relatifs au parent du fichier OIDC. Traversées parent et liens symboliques sont refusés. Préparez les fichiers sous l’UID réel du contrôle, fichiers privés `0600`, répertoires privés `0700` et ancêtres/ACL protégés. Le parent de la base d’authentification doit être inscriptible par cet UID ; rendre le fichier de configuration en lecture seule ne rend pas son parent entier adapté à un audit en lecture seule. L’entrée OIDC est limitée à 1 Mio.

Pour une AC d’entreprise, donnez à `ca_bundle_file` un fichier vérifié contenant uniquement les certificats publics approuvés, au plus 64 Kio et 8 certificats. Ces racines remplacent les racines intégrées du seul client OIDC, avec contrôle de chaîne, nom et validité conservé. Le gateway publie aussi ces certificats publics dans le profil CLI. Cette option ne change ni le magasin de confiance du navigateur ni la confiance TLS du CLI humain pour le **gateway**. Ne fournissez aucune clé privée et ne désactivez pas TLS.

Pour un gateway possédant sa propre AC privée approuvée, transmettez explicitement son bundle public distinct :

```sh
"$AIGUARD_BIN" human sso --gateway "https://approved-gateway.example.invalid" \
  --gateway-ca-file "/chemin/absolu/approuve/gateway-ca.pem" --browser manual
```

Remplacez cette origine d'exemple non fonctionnelle et ce chemin par les valeurs approuvées. `--gateway-ca-file` accepte uniquement des certificats PEM, au plus 65536 octets / 8 certificats, suivant les [règles de fichier vérifié de l'accès humain](../identity/README.fr.md#consulter-avec-le-cli). Ces racines remplacent WebPKI intégré pour le seul gateway HTTPS. Les racines indépendantes du fournisseur OIDC viennent toujours de `ca_bundle_file` et du profil CLI vérifié. Les confiances système et navigateur restent inchangées. HTTP avec cette option échoue avec `--gateway-ca-file requires an HTTPS gateway origin` ; un fichier invalide, indisponible, trop gros ou contenant une clé échoue avec `gateway CA configuration is invalid or unavailable` avant connexion.

Un snapshot immuable du gateway couvre profil, intentions natives, admission, lectures/export métier, refresh et logout ai-guard pendant toute la session au premier plan. Remplacer son fichier source ne recharge pas la confiance. Terminez la session puis lancez une nouvelle connexion explicite pour adopter une rotation approuvée ; aucun repli automatique n'est effectué. Un bundle modifié ne reprend pas un export conservé avec une autre confiance. OIDC exige en plus la même admission au premier plan et la même génération, comme décrit plus bas. Cette option ne fournit ni mTLS ni interrogation active de révocation des certificats ; la qualification du transport et de la révocation du déploiement reste distincte.

### Configurer les accusés de déconnexion fournisseur

Placez les champs facultatifs dans `gateway.provider`, avec des enregistrements fournisseur exacts sur leurs clients publics respectifs :

| Champ | Valeur de la DEMO distribuée |
|---|---|
| `web_post_logout_redirect_uri` | `https://127.0.0.1/oidc/logout-complete/web` |
| `cli_post_logout_redirect_uri` | `https://127.0.0.1/oidc/logout-complete/cli` |

Chaque URI doit être canonique, statique, HTTPS, sans userinfo, query, fragment ou joker, et distincte du callback de connexion Web. Ne déduisez pas la valeur native de son callback de connexion ou de son port d’écoute éphémère. Le modèle HTTPS emploie `https://guard.example.invalid/oidc/logout-complete/web` et `/cli` ; remplacez ces deux exemples par les enregistrements approuvés. Le [guide Keycloak](../keycloak/README.fr.md#enregistrer-les-retours-daccusé-de-déconnexion) décrit les entrées explicites par client.

Pour un client dont le retour est configuré, ai-guard envoie le POST privé end-session avec `client_id`, son `id_token_hint` conservé, `post_logout_redirect_uri` et un `state` privé neuf. La confirmation exige un `302` ou `303` direct, un corps vide et un seul `Location`, identique octet par octet à l’URI configurée suivie uniquement de `?state=<state de cette requête>`. Un `200`, `204`, une page HTML de confirmation, un autre retour, un paramètre supplémentaire, un `Location` absent/dupliqué ou un mauvais state ne confirme rien. Aucun repli vers l’acceptation antérieure n’a lieu tant que le retour de ce client est configuré. Ces règles strictes de réponse/corps sont le profil produit pris en charge par ai-guard, pas une exigence OIDC universelle ; voir [RP-Initiated Logout](https://openid.net/specs/openid-connect-rpinitiated-1_0.html).

ai-guard compare cette URI d’accusé, sans jamais la suivre ni la contacter, ouvrir un navigateur pour elle ou traiter un callback logout. Aucun listener HTTPS, certificat, changement DNS ou service OS n’est créé pour ces valeurs DEMO. Le transport issuer/end-session reste celui de la DEMO HTTP isolée ; une URI d’accusé HTTPS ne la rend pas conforme au profil HTTPS d’entreprise. La politique fournisseur doit permettre une fin directe avec l’ID-token hint fourni ; une page de confirmation interactive reste non confirmée. N’assouplissez pas cette politique pour toute l’installation afin de forcer un résultat réussi.

Lorsque l’option de ce client est absente/null, l’opération antérieure n’envoie ni URI de retour ni state et conserve son profil de réponse non interactif : `204` vide, ou `200` à corps vide ou objet JSON vide. Un redirect ou du HTML interactif reste non confirmé. Le [parcours réel Keycloak du4novembre](../../engineering/VALIDATION.md#4-novembre--sso-cli-réel-renouvellement-et-déconnexion) passe pour le protocole CLI natif sur LinuxARM64 isolé, avec refresh, rotation du contexte, accusé logout et audit durable. Ce scénario ne qualifie ni le nouveau profil Web, ni macOS/TTY natif ou HTTPS d’entreprise ; les observations historiques gardent leur portée.

### Mettre à jour la configuration logout sans remplacer les données

L’import au démarrage saute un realm Keycloak existant. Modifier le modèle de realm ou reconstruire son image ne met pas à jour les clients stockés. Suivez la [procédure manuelle d’enregistrement des clients](../keycloak/README.fr.md#mettre-à-jour-les-enregistrements-logout-existants), sans supprimer de realm ou de volume.

Avant activation, inventoriez enregistrements clients existants, configuration OIDC privée et versions exactes des composants. Conservez des sauvegardes cohérentes du fournisseur/contrôle selon leurs procédures approuvées, dont identité/politique de l’audit d’authentification, credentials et appartenances. Préparez les deux enregistrements clients bornés et un fichier OIDC privé relu avec les mêmes retours ; ne rejouez pas `setup prepare`, ne modifiez pas ses enregistrements de reprise et ne remplacez pas `authentication.sqlite`. Une préparation inachevée doit conserver son entrée d’origine et suivre sa [procédure de reprise](#arrêter-redémarrer-et-reprendre-une-préparation-échouée).

Déployez des composants gateway/dashboard/CLI mis à jour correspondants et appliquez la configuration relue par la procédure d’arrêt/redémarrage existante de l’installation. Gardez UID, chemins de données, UUID/politique d’audit et volumes. Un redémarrage gateway perd admissions vivantes et connexions en attente : les utilisateurs doivent se reconnecter. Ces deux options n’introduisent aucune migration de base. Conservez la configuration antérieure pour un retour coordonné ; un ancien binaire exige l’absence de ces champs et ne doit pas consommer un profil CLI public plus récent. Vérifiez séparément fermeture locale, audit durable et accusé fournisseur avant de consigner un résultat ; la qualification réelle reste ouverte.

### Limites des requêtes et sessions

Si `gateway.provider.limits` est présent, fournissez l’objet fermé complet défini par [OidcLimitsV1](../../../src/oidc/config.rs). Valeurs par défaut :

| Réglage | Défaut |
|---|---|
| `request_timeout_ms` | `5000` |
| `max_response_bytes` / `max_token_bytes` | `131072` / `16384` |
| `max_jwks_keys` / `max_inflight` | `32` / `8` |
| `metadata_ttl_seconds` / `jwks_ttl_seconds` | `300` / `300` |
| `unknown_kid_cooldown_seconds` | `30` |
| `pkce_ttl_seconds` | `300` |
| `clock_skew_seconds` | `5` |
| `max_token_lifetime_seconds` | `3600` |

Le gateway borne en plus les tokens à 16 Kio et les transactions PKCE à 300 secondes. L’admission est bornée par l’expiration de l’access token, 1 heure absolue et 15 minutes d’inactivité ; un refresh ne prolonge pas l’échéance absolue. La remise Web doit être consommée en 30 secondes. Ce sont les bornes actuelles d’implémentation, pas des garanties mesurées de révocation de bout en bout ou de disponibilité.

### Admission du travail et refresh annulé

Les lectures Store HTTP, mutations et contrôles d'authentification/audit s'exécutent dans des tâches bloquantes hors des workers Tokio. Le gateway revérifie identité, session, mapping, expiration et permissions dans le segment concerné au point d'I/O, après son éventuelle attente. Chaque poll SSE/page d'export repasse ces contrôles. Les permis d'I/O sont libérés avant le transport au client ; annuler une requête ne libère pas le permis d'un segment déjà lancé avant sa fin réelle et n'annule pas un commit durable.

Un nouveau travail est refusé avec `429 capacity_exhausted` lorsque sa borne de concurrence ou de fréquence de lecture est atteinte. Réduisez les lectures/flux parallèles et la fréquence des polls, puis réessayez explicitement une lecture refusée. Le contrôle runner dispose d'une voie distincte et ne consomme pas le budget des lecteurs viewer. Les [bornes d'admission API actuelles](../../api.md#erreurs-et-limites) distinguent lectures par tenant, travail partagé, authentification et nettoyage. Ces bornes internes sont distinctes de `provider.limits` et ne garantissent ni latence, ni disponibilité, ni capacité de 100 workers. Un flux/export interrompu reste partiel. Un refresh ambigu exige une nouvelle connexion, sans retry automatique de l'échange.

Read admet quatre leases Store simultanés par tenant pour les profils/pages et une source humaine connue. Source et contexte sélectionné doivent tous deux obtenir leur admission ; s'il s'agit du même Store, le segment réutilise son permis vivant. Les métadonnées initiales de résolution/collision ont une borne Discovery distincte de quatre sondes par tenant consulté. Le pool partagé d'authentification initiale admet seize tâches. Les sondes Discovery voisines effectuées dans des segments Control, Read ou cleanup conservent la borne globale de leur appelant ; elles ne repassent pas par le pool initial. Leur borne Discovery par Store reste applicable. Une sonde refusée reste indisponible, sans prouver l'absence d'un binding. Les collisions lisibles restent vérifiées dynamiquement ; un voisin indisponible ne donne aucune authentification implicite de la source. Le runner résolu utilise son lease Control existant pour son propre Store sans dépendre de la capacité Discovery de sa source. Ces classes sont distinctes : quatre permis Read ne couvrent pas toutes les I/O d'authentification.

Le gateway réserve avant mutation une capacité bornée de complétion/nettoyage du refresh, y compris pour les tâches qui attendront un worker bloquant. La complétion normale réutilise cette réserve. Si le traitement du refresh par le gateway est annulé, la génération pending/rotating reste refusée pendant le nettoyage différé. Celui-ci vérifie session, génération, opération et reçu d'audit exacts pour qu'une tâche retardée ne ferme pas une rotation plus récente. Un arrêt du runtime peut empêcher une tâche en queue de s'exécuter : l'annulation ne garantit pas de résultat terminal durable et ne déclenche aucun accès SQLite synchrone de secours. L'autorité pending reste inutilisable. Après redémarrage, les admissions vivantes sont perdues : reconnectez-vous avec un nouveau token sans réutiliser une ancienne génération pending. Ces mécanismes ne purgent ni ne réparent un audit plein.

### Capacité d'audit terminal du refresh

Avant qu'un refresh suivi puisse contacter le fournisseur d'identité, sa
transition Pending durable doit conserver la place d'un record terminal et
de 32 768 octets de payload sérialisé au maximum. L'admission tient compte des
réserves de chaque session Pending : `503 audit_unavailable` peut donc survenir
avant que les compteurs de records/octets physiques atteignent leurs plafonds.
Les autres écritures ne peuvent pas dépenser cette réserve. Une rotation ou
fermeture validée consomme sa propre réserve une seule fois ; un replay exact
ne donne ni nouvel échange ni libération de la réserve d'une autre session.

La réouverture reconstruit ces réserves depuis l'historique validé sans modifier
format d'audit, genesis, politique immuable ou records existants. Mettez tous les
writers à jour avant de compter sur cette règle ; le mélange d'anciens et de
nouveaux writers n'est pas pris en charge pour cette garantie. Un Pending
historique déjà sans place terminale ne se répare ni par réouverture ni par
hausse d'une valeur de configuration. Conservez le ledger original et traitez
ce cas comme un incident nécessitant un plan de reprise explicite. Cette
réservation ne renouvelle pas le quota global cumulatif, ne fournit pas de
rétention/migration de politique et n'isole pas cette capacité globale par tenant.

### Connexion en attente et audit durable

Une connexion Web initiale ou une connexion CLI native réserve un état borné et expirable en mémoire du gateway. Son cookie, sa transaction PKCE ou son intention native corrèle une tentative ; il ne constitue pas une identité authentifiée. Démarrage, annulation, refus ou expiration d’une tentative avant preuve vérifiée par access token n’ajoutent aucun enregistrement à l’audit d’authentification durable. Un redémarrage du gateway perd les tentatives en attente : recommencez la connexion au lieu de rejouer l’ancien callback ou l’ancienne intention.

Le format V2 réserve durablement la preuve vérifiée et son intention d’authentification avant la lecture auditée des memberships ou l’admission d’une session. Son empreinte reste mémorisée après refus, interruption et redémarrage. Réutiliser une preuve consommée renvoie `session_conflict` (HTTP `409` pour l’admission native), sans nouvelle écriture dans le ledger d’authentification. Un ledger V1 existant exige la [migration explicite](#migrer-un-audit-dauthentification-existant).

Les admissions initiales vérifiées échouées ou inachevées partagent un budget durable par `(issuer, sub)`, tous clients Web/CLI et types de session confondus. Le plafond de tentatives vaut `max(1, min(64, floor((max_records - 1) / 16)))`. Le plafond d’octets est un huitième de `max_serialized_bytes`, ou l’espace réservé pour une tentative s’il est supérieur ; cette réservation comprend son intention et son résultat de refus borné. Ces limites sont dérivées de la politique d’audit existante, sans nouveaux champs de configuration. Les plafonds globaux d’enregistrements et d’octets du ledger restent applicables.

Ce budget dure toute la vie du ledger : attente, expiration du token, redémarrage ou changement de client ne le réinitialisent pas. Une admission réussie durable libère sa réservation imputée aux tentatives échouées ou en attente, tout en conservant historique et déduplication de la preuve. Avec une preuve fraîche, une appartenance active actuelle peut permettre la reprise après épuisement ; autorisations habituelles et audit obligatoire doivent toujours réussir. Un tenant voisin indisponible ne donne aucune autorité : le membership autorisé doit être vérifié dans son propre store sain.

Une capacité globale d’audit épuisée ou un échec d’écriture obligatoire renvoie `503 audit_unavailable` ; aucune session n’est admise. L’audit des refresh, déconnexions et expirations de session reste obligatoire. Préservez le ledger, même déjà plein, et sa politique originale. Ne supprimez ni ne remplacez le ledger et ne le faites pas tourner pour contourner un refus. Aucun reset/archivage automatique ni commande CLI de changement de cette politique, d’archivage du ledger ou de gestion d’une réserve globale de contrôle n’est livré ; ces écarts d’exploitation restent ouverts.

## Établir comptes, memberships et ressources

Dans le parcours manuel, l’opérateur doit créer un compte OIDC actif et un membership dans chaque base tenant voulue. Une connexion ne les provisionne pas. Le préparateur de DEMO neuve réalise déjà ces mutations de compte/membership pour son manifeste relu. Pour un stockage existant, utilisez ces modèles publics dans l’ordre :

1. [account.json](examples/account.json) : principal `human_oidc` approuvé et nom affiché.
2. [membership.json](examples/membership.json) : même principal, liaison tenant réelle, organisation et capacités permises.
3. [resource-grant.json](examples/resource-grant.json) : ressource déjà connue dans ce tenant, avec propriétaire et organisation approuvés. L’exemple ne partage rien.

Remplacez chaque UUID d’exemple par un UUID canonique neuf en minuscules et non nul pour son opération ou membership ; conservez chaque identité pour les retries exacts. Remplacez `REPLACE_WITH_VERIFIED_SUBJECT`, issuer, tenant, organisation et ID ressource de façon cohérente. Pour la DEMO, reprenez son issuer HTTP exact dans les trois modèles. Ces fichiers ne préattribuent aucun ID sujet. Un compte sans membership n’accorde aucun accès ; un membership seul n’attribue pas les ressources historiques.

Enregistrez chaque mutation relue dans un fichier privé sous l’autorité de contrôle existante. Pour une base schéma4–10 existante validée à supervision inactive, lancez la commande suivante, puis répétez-la avec `oidc-membership.json` et `oidc-resource-grant.json` après la réussite de chaque commande précédente :

```sh
AIGUARD_BIN="/absolute/path/to/agentsandbox"
AIGUARD_DATA="/absolute/private/ai-guard/tenant.sqlite"
AIGUARD_TENANT="actual-configured-tenant"
AIGUARD_PRIVATE="/absolute/private/ai-guard"
"$AIGUARD_BIN" human apply \
  --data "$AIGUARD_DATA" --tenant-id "$AIGUARD_TENANT" \
  --mutation-file "$AIGUARD_PRIVATE/oidc-account.json" \
  --max-events 100000 --confirm-control-stopped
```

L’entrée accepte seulement `version`, `operation_id` et `change` typé ; un champ `actor` est refusé. L’autorité vient de l’opérateur effectif du filesystem. Chaque mutation et son audit sont atomiques ; les trois commandes distinctes ne forment pas une seule transaction. Après un résultat ambigu, préservez le fichier et rejouez exactement opération et octets. Un sens différent exige un nouvel UUID d’opération. `expected_revision:0`/`revision:1` désigne une création ; modifier un enregistrement existant exige sa révision précédente exacte et la suivante. N’éditez pas directement les projections SQLite.

### Configurer les permissions effectives

`gateway.mapping` définit un plafond de capacités à partir des claims vérifiés de l’access token. Il part de `default_capabilities`, puis ajoute les capacités des `rules` correspondantes. Chaque `claim_path` est un tableau de clés d’objets ; sa feuille doit être un tableau de chaînes bornées. `equals` correspond à une valeur exacte. Un claim absent ne correspond pas ; un type mal formé refuse l’authentification. Deux règles de mêmes chemin/valeur sont refusées.

Les droits effectifs sont l’intersection de ce plafond et du membership actif côté contrôle, puis du contexte choisi et des permissions de ressource. Les exemples permettent `read_metadata`, `read_content` et `export` en contexte personnel. La règle administrateur ajoute `read_tenant_metadata` ; le membership de l’administrateur voulu doit aussi le contenir explicitement. Les autres memberships ne doivent pas l’acquérir par la seule organisation commune. Les capacités administratives sont interdites dans `default_capabilities`.

Les capacités acceptées sont `read_metadata`, `read_content`, `export`, `read_tenant_metadata`, `manage_memberships`, `manage_grants`, `read_broker_registry` et `read_worker_registry`. Une capacité configurable ne prouve pas l’existence d’un parcours Web complet de mutation ou d’administration. Les contextes personnel et administration sont distincts et doivent être proposés par Rust. Administration du realm, noms de comptes et rôles d’un autre client ne donnent aucun droit applicatif ou accès inter-tenant implicite.

Changez `mapping.revision` à chaque évolution du mapping approuvé et redémarrez le gateway avec la configuration relue. Pour retirer membership/compte/grant, appliquez une nouvelle mutation hors ligne auditée avec révision incrémentée et `active:false` selon le cas. Arrêtez tous les consommateurs, puis redémarrez et imposez une nouvelle authentification. Un changement de rôle IdP seul ne révoque pas immédiatement un token émis ; les rôles changent avec de nouveaux claims vérifiés, tandis que les lectures humaines revalident les permissions du contrôle.

### Activer explicitement la lecture du registre brokers

Les manifestes setup/exemples distribués n'accordent **pas** `read_broker_registry`. `setup prepare` produit le schéma5 et refuse une identité demandant cette capacité avant publication des artefacts d'installation. Laisser la DEMO neuve et les memberships ordinaires existants inchangés tant que l'opérateur n'approuve pas délibérément cet accès à l'infrastructure du tenant. Le rôle public `admin-ai-guard` et l'administrateur du realm ne l'acquièrent pas automatiquement.

Pour chaque tenant voulu, arrêter tous consommateurs de sa base et redémarrages automatiques, conserver installation/volume préparés existants, puis appliquer [5→6→7 avec une nouvelle sauvegarde vérifiée par étape](../workloads/README.fr.md#activer-la-lecture-nominative-du-registre-brokers). Une base6 n'exige que6→7. Le chemin préparé est `/data/installation/tenants/<tenant>/audit.sqlite` ; utiliser UID/GID du conteneur et volume inspecté réels, sans base hôte différente. La migration n'ajoute ni compte, ni grant de membership, ni activation d'exécution ; la réouverture à froid reste en maintenance inactive. Ne pas employer bootstrap neuf, édition de métadonnées ou stockage de remplacement pour réactiver.

La lecture exige **à la fois** un membership actif côté contrôle contenant explicitement `read_broker_registry` et une règle explicite de `gateway.mapping.rules` dont le plafond sur claims vérifiés l'inclut. La [procédure de membership typé](../identity/README.fr.md#autoriser-la-lecture-nominative-du-registre-brokers) montre `human apply` avec `version`, `operation_id`, `change.action:"membership"`, révisions précédente/suivante exactes, principal/organisation immuables et aucun `actor` caller. Pour OIDC, conserver issuer/subject `human_oidc` exacts. Employer les mêmes `--max-events` et politique explicite `--audit-capacity-file` que l'installation. Seul le contrôle authentifié/hors ligne de confiance peut ajouter/réactiver cette capacité ; `manage_memberships` ne permet pas de promouvoir soi-même ou autrui.

Dans une copie privée relue du JSON OIDC existant, voici un **modèle d'une seule règle dans `gateway.mapping.rules`** :

```json
{
  "claim_path": ["resource_access", "ai-guard-api", "roles"],
  "equals": "guard-broker-reader",
  "capabilities": ["read_broker_registry"]
}
```

Remplacer chemin/client et rôle par le véritable claim vérifié administré par l'opérateur. Ce fragment n'est ni un fichier OIDC complet ni une commande de création de rôle IdP. Préserver URLs fournisseur, identité/politique d'audit et autres règles. Si le même couple `claim_path`/`equals` existe, modifier sa liste de capacités délibérément au lieu d'ajouter un doublon. Incrémenter `gateway.mapping.revision` en conservant son format chaîne, par exemple `company-policy-2-brokers`. Ne jamais mettre une capacité administration dans `default_capabilities`. Un claim brut/rôle correspondant sans membership local explicite n'accorde aucun accès ; un membership sans plafond correspondant n'autorise pas davantage la lecture. Aucun mapping ne choisit un autre tenant ou ne transforme le navigateur en runner.

Redémarrer gateway/dashboard correspondants avec configuration/données existantes, sans option d'initialisation. Se reconnecter et choisir un contexte administration actuellement proposé portant la capacité. **Brokers** ouvre **Brokers enregistrés du tenant.** ; un contexte possédant ce seul droit y arrive sans lecture ressources/événements/export/SSE. `read_tenant_metadata` conserve son ancienne portée ressources filtrée par organisation. Ni `human` ordinaire ni CLI SSO au premier plan n'ont de commande brokers ; utiliser cette vue ou le [contrat GET humain typé](../../api.md#registre-brokers-humain--human-broker-registry).

Pour retirer l'accès, l'opérateur applique une nouvelle révision auditée du membership retirant la capacité ou le désactivant, ou révise le plafond approuvé et impose une nouvelle connexion. Un administrateur d'organisation peut conserver/retirer un droit existant ou le garder dormant, jamais l'ajouter ou réactiver son membership inactif. Un refus reconnu de contexte/permission retire données, détails et curseurs ; les réponses tardives ne les restaurent pas. La révocation ne rappelle pas les octets déjà reçus et le déclenchement visible toutes les30secondes ne borne pas le délai global de retrait.

La page exige l'audit humain durable obligatoire, même vide, et partage les budgets F1 tenant/principal des lectures identité/ressources. Le dashboard préserve `429 audit_read_budget_exhausted`/`contact_operator` du préflight identité comme de la page. Il arrête les lectures automatiques et conserve toute dernière page encore permise datée/périmée. Préserver base tenant, politique et reçus, examiner occupation/réserves et réparer la cause ; ne pas réinitialiser l'audit ou renouveler les credentials pour retrouver de la capacité. Après intervention, **Relire après intervention de l’opérateur** déclenche un retry explicite. Capacité temporaire et erreurs transport restent distinctes. Un503 `migration_required` demande la migration hors ligne, sans repli viewer. Voir [l'accès nominatif](../identity/README.fr.md#autoriser-la-lecture-nominative-du-registre-brokers) pour états, reçus et limites de fraîcheur.

De vraies opérations d'enregistrement/heartbeat doivent déjà avoir alimenté le registre réel. Un registre vide est valide ; ne pas insérer d'observations de fixture. Cette vue ne qualifie ni déploiement réel navigateur/OIDC, ni fédération, ni inventaire runtime complet, ni release production. Les preuves d'ingénierie correspondantes indiquent ce qui a réellement été exécuté.

## Démarrer ou rouvrir les services existants

Ajoutez les arguments suivants au service gateway existant, en conservant ses vrais registre, politique et réglages de moteur de décision. Il s’agit du contrat d’arguments pour un déploiement conteneurisé préparé, **pas** d’une nouvelle pile Compose complète. Les chemins privés conteneur `/control/*` sont à remplacer. Avec l’entrypoint standard de l’image, la commande du service commence à `gateway` :

```json
["gateway", "--listen", "0.0.0.0:8787", "--container-listen",
 "--tenants-file", "/control/tenants.json", "--policy", "/control/policy.json",
 "--oidc-config", "/control/oidc.json", "--initialize-oidc-audit"]
```

Utilisez `--initialize-oidc-audit` uniquement pour une base d’authentification exclusivement neuve au chemin configuré. Le drapeau refuse un fichier existant, même vide ou partiel. Un échec peut conserver un fichier neuf partiel : préservez-le pour diagnostic ; l’initialisation n’est pas une réparation. Le drapeau ne crée/migre ni identités ni bases tenants.

À chaque redémarrage normal, **retirez `--initialize-oidc-audit`** et conservez `--oidc-config`. La réouverture exige l’audit valide existant, son UUID d’installation original et sa politique d’audit identique, y compris ordre des clients approuvés et quotas. Éditer quotas ou clients n’est pas une migration implémentée de la politique d’audit. Conservez les enregistrements et résolvez explicitement les changements de capacité/configuration ; ne remplacez pas la base pour lever un refus. Sans `--oidc-config`, le gateway n’active aucun SSO et n’appelle aucun fournisseur OIDC.

Activez le proxy fixe du dashboard avec la même origine Web publique exacte que `gateway.web_origin` :

```json
["dashboard", "--listen", "0.0.0.0:4000", "--container-listen",
 "--upstream", "http://gateway:8787", "--shared-tenants",
 "--front-dir", "/app/front", "--oidc-web-origin", "https://guard.example.invalid"]
```

Adaptez l’upstream au vrai trajet interne gateway et vérifiez le chemin des assets compilés de votre image. `--shared-tenants` enlève l’exigence d’un fichier viewer fixe ; il n’accorde pas tous les tenants aux utilisateurs. Pour conserver un déploiement à viewer fixe, gardez plutôt sa configuration existante de fichier viewer. En DEMO, donnez à `--oidc-web-origin` la valeur `http://127.0.0.1:4000`. Le dashboard a seulement besoin de son trajet gateway fixe, pas d’un accès direct à l’IdP. Aucun des deux services n’a besoin d’un socket moteur ou du projet exécuté monté.

Les listeners Rust sont HTTP ; une origine publique HTTPS ne crée pas de terminaison TLS. Préparez et qualifiez le transport approuvé avant l’exposition de l’installation. Le [guide Keycloak existant](../keycloak/README.fr.md) couvre projet fournisseur, DNS et comptes publics DEMO. Les anciens Compose du socle ne raccordent pas le gateway au réseau d’identité. Les fichiers SSO autonomes ci-dessus le font via le réseau externe explicitement inspecté. Pour un déploiement manuel, préparez ce raccordement gateway restreint en conservant le même issuer et en laissant dashboard/workers hors de ce réseau ; une topologie déclarée ne prouve pas une interopérabilité réussie.

## Se connecter dans le navigateur

Ouvrez l’origine exacte du dashboard configuré. L’écran vérifie l’état SSO et affiche libellé du fournisseur et issuer. Choisissez **Se connecter avec …**, terminez la connexion chez le fournisseur, puis choisissez un contexte renvoyé par ai-guard. Un compte fournisseur valide sans membership applicatif approuvé est refusé. Une connexion réussie sans ressource autorisée peut afficher une page vide ; un défaut d’authentification, de stockage ou de réseau doit rester une erreur.

Le gateway consomme son callback et envoie le navigateur vers `/auth/oidc/complete` sans transmettre les paramètres code/state. Les tokens sont remis une seule fois puis gardés en mémoire, hors URL, Web Storage, logs et exports. Rechargement, fermeture de page ou redémarrage gateway exigent une nouvelle connexion explicite. Ne recopiez pas un token des outils développeur dans un fichier ou une autre commande CLI. Le navigateur coordonne le refresh lorsqu’un refresh token est disponible ; un refus ou résultat ambigu ferme la session locale. Une nouvelle génération remplace la vue autorisée au lieu de conserver l’ancien contexte.

Utilisez l’action de déconnexion de l’application. L’état local du navigateur est effacé même si contrôle/fournisseur ne peuvent confirmer le logout. Distinguez les résultats :

| Résultat | Signification |
|---|---|
| `local_session` | `closed` confirme la fermeture de cette admission ai-guard par le contrôle ; `unknown` ne la confirme pas. Effacer la mémoire du navigateur/processus est une action locale distincte. |
| `audit` | `recorded` ou `unavailable` décrit l’audit obligatoire du logout côté contrôle ; une réponse fournisseur ne le remplace pas. |
| `provider_logout` | `accepted` signifie que le composant tentant le logout fournisseur a reçu la réponse prise en charge configurée. `failed`, `unavailable`, `unsupported` ou `not_attempted` ne confirme pas ce logout. |
| `control_provider_logout` | Résultat fournisseur du gateway rapporté par le CLI natif. Il reste `not_attempted` pour les sessions natives : le CLI conserve l’ID-token hint en privé et réalise son propre appel fournisseur, décrit par `provider_logout`. Le gateway ne récupère ni ne reçoit ce hint pour le logout natif. |

Un accusé accepté ne prouve ni révocation JWT globale, fermeture de toutes les autres sessions navigateur/client ou livraison d’un redirect navigateur. Les JWT validés hors ligne peuvent rester valides selon la politique de durée/révocation de l’installation.

### Lorsque la liste des contextes est partielle

Un profil voisin qui ne peut pas obtenir son admission de lecture rend aussi la réponse d'identité partielle. La consultation sautée n'est pas réconciliée comme un profil vide et ne révoque pas les contextes déjà émis. Leur prochaine utilisation vérifie toujours store sélectionné, autorisation et limites. La saturation de la source d'un credential local ou du contexte sélectionné renvoie `429`, sans fausse liste vide ; un store obligatoire indisponible reste une erreur. Actualisez les contextes après résolution de la charge ou de la panne de stockage.

La réponse d’identité contient le booléen obligatoire `contexts_complete`. `true` signifie que toutes les consultations des profils des tenants configurés pour cette réponse ont réussi. `false` indique qu’au moins une source est indisponible : les contextes proposés proviennent des sources consultables, sans nommer les tenants indisponibles ni deviner leurs appartenances. Cela concerne les credentials locaux nominatifs comme les identités OIDC admises. Aucun tenant fourni dans la requête ni panne voisine ne change votre périmètre.

Le dashboard affiche **Liste des contextes partielle** et **Actualiser les contextes**. Utilisez cette action après réparation du store affecté par l’opérateur. Un contexte absent, voire une liste partielle vide, ne prouve pas une révocation d’accès. Choisissez seulement un contexte actuellement proposé par le contrôle. Son autorisation et son audit obligatoire sont revérifiés à l’utilisation ; un store sélectionné indisponible donne `503`, pas une liste de ressources vide. Le store source d’un credential local doit également rester consultable pour vous authentifier.

Une connexion OIDC peut réussir lorsqu’une appartenance active et autorisée est établie dans une source saine. Si aucune appartenance autorisée ne peut être établie alors qu’une source est indisponible, l’erreur reste `503` ; ce n’est pas un résultat définitif `permission_denied`. Préservez le store et son audit, réparez la panne puis réessayez explicitement. Une absence d’appartenance établie sur des sources consultables reste un refus.

## Se connecter et travailler dans le CLI au premier plan

Compilez un CLI natif hôte depuis la même révision source si vous n'en avez pas déjà un. L'image de contrôle contient un binaire Linux ; ne l'extrayez pas pour un poste macOS. Depuis la racine du dépôt, avec la toolchain Rust du dépôt :

```sh
cargo build --locked --bin agentsandbox
```

Si vous définissez `CARGO_TARGET_DIR`, utilisez le binaire de ce répertoire. Conservez un binaire natif correspondant à la version installée du gateway.

Exécutez le CLI de confiance sur le poste utilisateur, vers l’**origine gateway**, sans `/api` ni autre chemin. C’est la connexion ai-guard, distincte de la connexion du CLI fournisseur :

```sh
AIGUARD_BIN="$(pwd)/target/debug/agentsandbox"
"$AIGUARD_BIN" human sso --gateway https://guard-api.example.invalid
```

Utilisez l’adresse réelle du gateway. Un gateway local isolé peut employer `http://127.0.0.1:8787`. Le CLI vérifie HTTPS, désactive proxies ambiants, redirects et retries automatiques ; HTTP est limité à une IP loopback littérale. Si le lanceur navigateur système pris en charge est indisponible, recommencez explicitement avec `--browser manual`. Cette option affiche une URL d’autorisation transitoire à ouvrir dans un navigateur externe ; ce n’est pas un flux headless/device/mot de passe. Gardez cette URL privée et hors des logs. Le navigateur doit revenir au listener éphémère de ce même poste.

Gardez le processus et son entrée standard ouverts. Après `session_ready`, saisissez `help` ou ces commandes ; remplacez les IDs en majuscules par les valeurs exactes des lectures précédentes :

```text
identity
context CONTEXT_ID_RETURNED_BY_IDENTITY
resources --limit 25
events --view metadata --resource-kind session --resource-id AUTHORIZED_SESSION_ID --limit 25
export --view metadata --resource-kind session --resource-id AUTHORIZED_SESSION_ID --output-dir /absolute/private/ai-guard/new-export
refresh
identity
logout
```

`context` accepte seulement un ID proposé à cette identité. Les types de ressource sont `session`, `sandbox`, `agent` et `project` ; choisissez explicitement `metadata` ou `content`. Contenu et export nécessitent des permissions distinctes. La pagination reprend le `next_cursor` opaque exact avec sélection inchangée ; une page ne donne pas un total. Les commandes acceptent les guillemets mais aucun développement shell/environnement, substitution ou exécution. Ne préfixez pas les commandes au premier plan par `human` et ne collez aucun bearer.

Vérifiez `contexts_complete` dans le résultat `identity` avant de considérer sa liste de contextes comme complète. Après reprise d’un store voisin, relancez `identity` puis choisissez un contexte proposé. Une liste partielle ne révoque pas une appartenance absente ; elle n’autorise pas non plus l’usage d’un contexte inventé ou non vérifié.

Tokens et vérificateur PKCE restent dans ce processus ; aucun cache durable de tokens ou fichier de credential n’est créé. `refresh` est explicite dans le CLI. Après sa réussite, demandez `identity` et choisissez un nouveau contexte proposé ; anciens contextes/curseurs et générations d’export sont invalides. Un refresh échoué ou ambigu ferme la session au premier plan, sans retry automatique. `logout`, `quit`, `exit`, EOF et Ctrl-C effacent l’état local ; la connexion suivante est une nouvelle session. Un redémarrage gateway perd les admissions vivantes malgré l’audit durable.

Le CLI natif utilise son propre ID-token hint conservé et le retour logout CLI configuré ; le listener de connexion n’est pas réutilisé. Lisez `provider_logout` avec `control_provider_logout`, `local_session` et `audit` selon les [significations distinctes](#se-connecter-dans-le-navigateur). Un accusé fournisseur échoué ne restaure pas la session au premier plan et ne déclenche aucun repli navigateur/callback.

L’export demande un parent privé existant et une destination finale **neuve**. Son budget est de 25 pages, 4 Mio et 60 secondes depuis la préparation initiale. Après interruption, conservez le répertoire `.preparing` adjacent. `resume-export --output-dir /absolute/private/ai-guard/new-export` exige cette préparation exacte, le même contexte, la session au premier plan encore vivante et sa génération d’admission, dans l’échéance initiale. Refresh, arrêt/redémarrage du processus, autorisation expirée ou budget épuisé imposent un nouvel export explicitement autorisé vers une autre destination neuve ; conservez la préparation non résolue. Un reçu de lecture prouve l’enregistrement du contrôle, pas la livraison des octets ni la vérification d’un audit signé complet.

## Migrer un audit d’authentification existant

Le ledger d’authentification séparé possède son propre format, indépendant des migrations du schéma tenant. Le démarrage normal exige le format V2 ; ouvrir un ledger V1 signale la migration explicite requise, sans le migrer ni le remplacer. Les nouveaux ledgers d’authentification utilisent V2. Conservez UUID d’installation, clients approuvés, politique, credentials et bases tenants existants pour cette migration.

Arrêtez le gateway et tous les autres consommateurs de ce ledger d’authentification, puis exécutez le binaire Rust correspondant sous l’UID du contrôle dans un conteneur ponctuel sans réseau, avec accès au volume privé de contrôle existant. Préparez un parent de sauvegarde privé existant (`0700`) et choisissez un chemin final de sauvegarde **neuf**. Dans ce conteneur :

```sh
agentsandbox migrate-authentication \
  --oidc-config /absolute/private/ai-guard/oidc.json \
  --backup /absolute/private/backups/authentication-before-v2.sqlite \
  --confirm-control-stopped
```

`--confirm-control-stopped` est l’attestation de maintenance de l’opérateur, pas une vérification ni une preuve indépendante d’arrêt des consommateurs. La commande n’arrête aucun service et ne clôture pas l’accès d’un autre writer. Elle lit le JSON OIDC privé fermé et ses références locales existantes d’audit/d’AC publique facultative, sans découverte fournisseur, échange de jeton ni requête réseau. Les références relatives du JSON restent relatives à son parent. Aucun credential fournisseur, token runner tenant ou `.env` n’est nécessaire.

La migration vérifie le ledger V1 existant et une sauvegarde V1 cohérente, privée et exclusivement neuve avant d’activer le marqueur V2 de preuve initiale vérifiée. Elle conserve namespace, head, enregistrements historiques et compteurs d’octets sérialisés ; elle n’efface pas l’historique, ne réinitialise pas la capacité, ne change pas la politique et ne fabrique pas d’enregistrements pour les preuves passées. Le rapport JSON contient les valeurs de vérification conservées, sans jeton ni réponse fournisseur. Un ledger déjà V2 est refusé avant création d’une autre sauvegarde. Une destination de sauvegarde existante, une liaison/politique invalide, une référence locale non fiable ou une vérification échouée sont refusées ; préservez source et sauvegardes puis corrigez la précondition signalée avant un retry explicite. Aucun repli vers une initialisation ou un remplacement automatique.

Après succès, conservez la sauvegarde V1 avec configuration correspondante et inventaire des binaires, puis redémarrez les consommateurs correspondants sur la source migrée. La sauvegarde reste V1 et exige cette migration explicite avant un démarrage normal V2 si elle est restaurée. Validez toute reprise dans une destination isolée privée neuve avant bascule ; n’écrasez jamais la source migrée pour tester une restauration. La migration ne restaure ni tokens, sessions vivantes, autorité d’exécution ou détection indépendante du rollback. Cette commande opérateur ne qualifie ni IdP, release, archivage ou procédure de rétention.

## Diagnostiquer et reprendre

`GET /auth/oidc/status` donne activation, issuer et libellé fournisseur ; `GET /auth/oidc/cli-profile` fournit le profil public CLI du fournisseur. Ils ne sondent pas la disponibilité IdP. Un processus sain ou un état activé ne prouve donc pas une connexion fonctionnelle. Utilisez configuration relue, métadonnées publiques de certificats et erreurs masquées ; ne collectez ni queries de callback, cookies, headers d’autorisation, réponses fournisseur brutes ou environnements complets.

| Résultat ou symptôme | Action |
|---|---|
| SSO désactivé / `oidc_not_configured` | Vérifiez `--oidc-config` explicite du gateway et `--oidc-web-origin` du dashboard, avec le binaire correspondant. Aucun repli local/viewer automatique. |
| Configuration invalide / démarrage refusé | Vérifiez champs version1 fermés, URL exactes, clients/audience distincts, propriétaire/ACL des fichiers et chemin d’audit séparé. Aucune interpolation d’environnement. |
| `origin_denied` / callback refusé | Ouvrez l’origine exacte configurée ; vérifiez callback et clients enregistrés. Recommencez une connexion après expiration/annulation ; ne rejouez jamais callback/code. |
| `permission_denied` | Vérifiez `(issuer, sub)`, compte/membership actifs et intersection des capacités mappées, puis organisation/grants. Administrer l’IdP ne donne pas autorité sur l’application. |
| `credential_invalid` / `session_conflict` | Admission expirée, révoquée, renouvelée, consommée ou perdue au redémarrage. Effacez la vue et reconnectez-vous. Ne rejouez pas un échange ambigu. |
| `migration_required` | Arrêtez les contrôles, conservez les sauvegardes et réalisez la migration de stockage suivante explicite. N’initialisez pas une base de remplacement. |
| Échec TLS, discovery, signature ou fournisseur | Vérifiez DNS/routes approuvés, issuer/endpoints, chaîne publique/SAN/horloge, profil des tokens et rotation des clés. Réparez la source ; conservez validation et destinations fixes. |
| Logout fournisseur `failed` / `unavailable` | Vérifiez enregistrement post-logout exact par client, politique de fin directe, endpoint end-session et transport approuvé. HTML, mauvais `Location`/state ou redirect à corps non vide reste non confirmé ; conservez les résultats de fermeture locale et d’audit. Ne suivez pas l’URI reçue, ne journalisez ni state/hint et ne réessayez pas via un repli interactif. |
| `audit_unavailable` / HTTP `503` | Audit d’authentification/lecture obligatoire non fiable, non inscriptible ou capacité globale d’audit épuisée. Préservez les données et diagnostiquez stockage, intégrité et capacité avant retry. Remplacer ou faire tourner le ledger n’est pas un contournement disponible. |
| Contextes partiels / `contexts_complete:false` | Préservez le store affecté et réparez sa disponibilité. Utilisez **Actualiser les contextes** ou relancez `identity` au premier plan ; une liste incomplète ne prouve ni absence ni révocation d’accès. Chaque contexte sélectionné exige toujours son propre store sain. |
| Champ d’identité humaine absent ou incompatible | Déployez ensemble des versions correspondantes du gateway, du dashboard compilé et du CLI. Ne supprimez pas `contexts_complete`, ne substituez pas un tenant libre et ne remplacez pas la base pour contourner l’erreur de contrat. |
| `capacity_exhausted` / HTTP `429` | Admission du travail, fréquence de lecture, authentification vivante ou nettoyage bornés. Réduisez lectures/flux parallèles et polling ; laissez le travail lancé finir ou les tentatives expirer avant un retry explicite. Annuler ne libère pas immédiatement les permis d'I/O actifs. Ne créez pas un autre audit pour dépasser la borne ; un refresh ambigu exige une nouvelle connexion. |
| `authentication_subject_capacity_exhausted` / HTTP `429`, `retry_action: contact_operator` | Budget durable d’admissions échouées/en attente du sujet vérifié épuisé. Attendre ne le réinitialise pas. Faites vérifier par l’opérateur compte, membership et mapping des claims vérifiés ; une preuve fraîche après provisioning approuvé peut permettre la reprise. Conservez ledger et historique. |
| Connexion perdue / données périmées | Gardez un état déconnecté/périmé explicite. Reconnectez-vous et choisissez un contexte actuel avant reprise ; une perte n’est pas une activité nulle. |

Conservez fichier OIDC, identités exactes de binaires/images, registre tenants, identités/grants privés et bases tenants avec l’inventaire de l’audit d’authentification séparé. Cet audit conserve des résultats typés et empreintes de tokens, sans tokens récupérables. Il n’est pas atomique avec l’IdP externe : un échange fournisseur suivi d’un crash peut laisser une issue inconnue nécessitant une nouvelle connexion.

`migrate-authentication` crée la sauvegarde V1 neuve vérifiée nécessaire à sa migration V1→V2 explicite. Aucune commande autonome de sauvegarde/restauration, d’archivage ou de rétention de l’audit d’authentification n’est exposée. La bibliothèque Rust possède aussi une sauvegarde neuve vérifiée ; n’inventez pas de wrapper shell et ne prenez pas la copie du seul fichier SQLite actif pour une sauvegarde cohérente. Avant sauvegarde/reprise, arrêtez tous les consommateurs et préservez un ensemble SQLite cohérent, identité d’installation, politique d’audit et configuration existante selon la procédure de stockage approuvée. Conservez l’existant et validez la reprise dans une destination isolée neuve avant bascule. La reprise ne restaure ni tokens, sessions vivantes ou autorité d’exécution.

L’audit vérifie sa chaîne et son plancher en processus vivant, mais détecter un rollback complet de base entre redémarrages demande un témoin indépendant. Cette configuration ne prouve ni anti-rollback, points de contrôle externes signés ou qualification de release. Les quotas comptent les enregistrements, genesis inclus, et les octets sérialisés ; pages SQLite, WAL/SHM, sauvegardes et usage filesystem s’y ajoutent. Un audit épuisé refuse de nouvelles opérations obligatoires ; une procédure de migration/rétention de sa politique reste à fournir avant d’atteindre cette limite en exploitation.

### Activer explicitement la lecture du registre workers

`read_worker_registry` est une capacité administrative distincte acceptée ; les valeurs distribuées ne l’accordent pas et setup5 la refuse. Elle exige un membership actif explicitement approuvé par le contrôle après migration9→10 et un plafond explicite sur claims du jeton d’accès vérifié. Le [guide workers synchronisé](../identity/WORKERS.fr.md) précise fragment de règle, migration/restauration, consommateurs appariés, révocation/purge et écart obligatoire d’attribution personnelle. `read_broker_registry` et `read_tenant_metadata` n’impliquent aucun droit workers.
