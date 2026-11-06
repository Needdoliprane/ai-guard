[English](README.md) · [Français](README.fr.md)

# Keycloak et OpenID Connect

Ce guide prépare un fournisseur d'identité Keycloak local ou le raccord d'ai-guard à un fournisseur OIDC d'entreprise approuvé. Identité, authentification de l'agent et inférence sont des choix indépendants : se connecter à ai-guard ne connecte pas Codex ou Claude, ne choisit pas un modèle et ne fournit pas votre jeton d'identité à un worker.

Le profil local utilise des comptes publics de démonstration sur le loopback hôte. Pour une exploitation normale, utilisez votre propre fournisseur, vos comptes, HTTPS et votre politique d'accès. Le [contrat d'identité](../../sso.md) définit les parcours Web/CLI, les permissions et l'audit décrits ici. L'[index d'installation](../README.fr.md) couvre l'installation complète.

## Choisir la configuration

| Fichier | Fonction |
|---|---|
| [`.env.exemple.local-keycloak`](../../../.env.exemple.local-keycloak) | Fournisseur d'identité local, avec choix de l'agent et du fournisseur IA séparé. |
| [`.env.exemple.local-keycloak-qwen`](../../../.env.exemple.local-keycloak-qwen) | Mêmes paramètres d'identité avec le contexte d'inférence Qwen. L'identité ne modifie ni le rôle du modèle ni la politique de sûreté. |
| [`compose.yaml`](../../../deploy/keycloak-demo/compose.yaml) | Projet d'identité distinct, explicitement activé, avec Keycloak et son ingress Rust à destination fixe. |
| [`Dockerfile`](../../../deploy/keycloak-demo/Dockerfile) et [realm](../../../deploy/keycloak-demo/ai-guard-demo-realm.json) | Image Keycloak épinglée et realm DEMO initial. |

Les exemples contiennent des paramètres publics. Conservez les credentials fournisseurs dans la configuration privée de l'installation, hors du workspace source et des images. Ne donnez pas un fichier de credentials existant comme fichier d'environnement de cet IdP. Compose ne transmet que les entrées déclarées pour chaque service ; un exemple ne constitue pas un handoff de credentials aux workers.

Ces quatre variables configurent le fournisseur d'identité local :

| Variable | Valeur par défaut | Signification |
|---|---|---|
| `AI_GUARD_DEMO_IDP_HOST` | `keycloak.ai-guard.test` | Nom DNS public, sans schéma ni chemin ; également alias du réseau interne. |
| `AI_GUARD_DEMO_IDP_PORT` | `18080` | Même port HTTP public et conteneur. Choisir un port libre supérieur à 1024, différent de 18081. |
| `AI_GUARD_DEMO_WEB_ORIGIN` | `http://127.0.0.1:4000` | Origine exacte du dashboard, sans slash final, chemin, query ou fragment. |
| `AI_GUARD_DEMO_CONTROL_IMAGE` | `agentsandbox-control:local` | Image de contrôle ai-guard préparée, utilisée uniquement comme ingress DEMO. |

Utilisez le même fichier pour toutes les commandes ci-dessous. Pour associer Keycloak à Qwen, remplacez partout `.env.exemple.local-keycloak` par `.env.exemple.local-keycloak-qwen`. Ces variables configurent le fournisseur ; la configuration d'identité du produit est décrite séparément plus bas. Les valeurs de l'environnement du shell priment sur celles du fichier d'environnement Compose ; résolvez les réglages contradictoires avant le démarrage.

Conservez `-p ai-guard-keycloak` dans chaque commande Compose d'identité. Les exemples fixent `COMPOSE_PROJECT_NAME=ai-guard-local` pour l'application principale ; l'option de projet explicite maintient l'IdP séparé.

## Préparer le DNS, les images et un projet neuf

Configurez le DNS local pour que `keycloak.ai-guard.test` résolve uniquement vers le loopback hôte. Une entrée hosts administrée par l'opérateur peut utiliser `127.0.0.1 keycloak.ai-guard.test`. Préservez les entrées existantes. Navigateur, CLI et contrôle conteneurisé doivent utiliser le même nom public d'issuer ; remplacer ce nom par `localhost`, un nom de service conteneur ou une IP crée un autre issuer. Le réseau Compose fournit son alias interne indépendamment du DNS hôte.

Avec les valeurs par défaut, l'issuer exact est `http://keycloak.ai-guard.test:18080/realms/ai-guard-demo`. L'origine du dashboard est `http://127.0.0.1:4000` ; le port habituel du gateway est 8787. Le port 18081 est un second listener de l'ingress, lié au loopback de son propre conteneur et non publié.

Choisissez un nom de projet neuf. Les commandes emploient `ai-guard-keycloak` ; avant le premier démarrage, vérifiez qu'aucune ressource ne porte déjà son label de projet :

```sh
docker ps -a --filter label=com.docker.compose.project=ai-guard-keycloak
docker volume ls --filter label=com.docker.compose.project=ai-guard-keycloak
docker network ls --filter label=com.docker.compose.project=ai-guard-keycloak
```

Si le nom appartient à une autre installation, choisissez-en un autre et modifiez `-p` dans toutes les commandes. Réutilisez-le uniquement pour reprendre volontairement cette même DEMO. Ne fusionnez jamais ce fichier Compose avec le projet d'une application existante et ne reprenez ni ses credentials ni son volume de données.

Préparez l'image de contrôle selon le [guide d'installation locale](../local/README.fr.md). Pour le tag par défaut, la commande de construction est :

```sh
docker build -f images/control.Dockerfile -t agentsandbox-control:local .
```

Choisissez un tag distinct et adaptez `AI_GUARD_DEMO_CONTROL_IMAGE` si ce tag est déjà utilisé par une autre installation. La construction peut télécharger des dépendances ; une image déjà préparée ne demande pas une reconstruction à chaque démarrage.

Le Dockerfile Keycloak épingle la version 26.8.1 et le digest multi-plateforme ci-dessous. Acquérez explicitement cette image de base publique, puis préparez l'image locale optimisée :

```sh
docker pull quay.io/keycloak/keycloak:26.8.1@sha256:3f3c5f8e03c614774b4bd9173b88391fefa4153bfc010d8012d59ad825377332
docker compose --env-file .env.exemple.local-keycloak -p ai-guard-keycloak \
  -f deploy/keycloak-demo/compose.yaml --profile keycloak-demo-http config --quiet
docker compose --env-file .env.exemple.local-keycloak -p ai-guard-keycloak \
  -f deploy/keycloak-demo/compose.yaml --profile keycloak-demo-http build keycloak-demo
```

Pour une installation hors ligne, préparez ou importez les images et dépendances nécessaires avant d'entrer dans l'environnement isolé. Conservez les identités réelles des images et leur architecture dans l'inventaire d'installation ; un tag local mutable ne suffit pas comme provenance.

## Démarrer le fournisseur d'identité local

Depuis la racine du dépôt, après préparation des images :

```sh
docker compose --env-file .env.exemple.local-keycloak -p ai-guard-keycloak \
  -f deploy/keycloak-demo/compose.yaml --profile keycloak-demo-http \
  up -d --no-build --pull never keycloak-demo keycloak-demo-ingress
docker compose --env-file .env.exemple.local-keycloak -p ai-guard-keycloak \
  -f deploy/keycloak-demo/compose.yaml --profile keycloak-demo-http ps
```

Le profil demande `--profile keycloak-demo-http`. Ses deux services démarrent indépendamment du gateway et du dashboard du produit. Keycloak utilise un volume d'identité neuf et un réseau interne. L'ingress ne publie que `127.0.0.1:18080` ; aucun de ces services ne monte un socket moteur, le projet original ou des credentials de contrôle existants. Keycloak est limité à 1 Gio de mémoire, 2 CPU et 256 PID ; l'ingress à 192 Mio, 0,5 CPU et 128 PID.

Ouvrez `http://keycloak.ai-guard.test:18080/admin/` pour administrer le fournisseur. Le document public de découverte est disponible à `http://keycloak.ai-guard.test:18080/realms/ai-guard-demo/.well-known/openid-configuration`. Vérifiez son `issuer` exact avant de configurer un client. Un conteneur démarré ne constitue pas une connexion réussie au produit.

Ce mode HTTP est explicitement limité à la DEMO locale isolée. Gardez ses credentials connus sur le loopback. Configuration HTTPS et confiance des clients constituent des étapes distinctes ; changer un nom d'hôte ou générer un certificat ne transforme pas ce profil Compose en HTTPS.

## Comptes, clients et mapping des jetons

| Compte public | Mot de passe public | Accès prévu |
|---|---|---|
| `admin` | `admin` | Administration bootstrap dans **master**. Aucune identité applicative importée ni permission ai-guard implicite. |
| `admin-ai-guard` | `admin-ai-guard` | Realm DEMO, rôle client `ai-guard-demo-api/admin-ai-guard`. Le périmètre applicatif vient toujours des appartenances approuvées par le contrôle. |
| `user-1` | `user-1` | Realm DEMO, sans rôle administrateur applicatif. |
| `user-2` | `user-2` | Realm DEMO, sans rôle administrateur applicatif. |

Le bootstrap s'applique à une base DEMO neuve. Ces valeurs ne sont jamais les valeurs par défaut d'une installation d'entreprise. L'administration de Keycloak et celle d'ai-guard ont des responsabilités distinctes.

| Client | Contrat |
|---|---|
| `ai-guard-demo-web` | Public ; Authorization Code ; PKCE S256 obligatoire ; retour de connexion enregistré `${AI_GUARD_DEMO_WEB_ORIGIN}/auth/oidc/callback` ; origine Web exacte. Les retours logout explicites conservent `${AI_GUARD_DEMO_WEB_ORIGIN}/` et ajoutent `https://127.0.0.1/oidc/logout-complete/web`. |
| `ai-guard-demo-cli` | Client public distinct ; Code et S256 obligatoire ; référence de connexion enregistrée `http://127.0.0.1/oidc/callback` ; aucune origine CORS. La connexion native emploie un port loopback éphémère, la même IP et le chemin fixe, avec prise en charge de l'exception de port par le fournisseur. Son retour logout explicite est `https://127.0.0.1/oidc/logout-complete/cli`, sans héritage de port. |
| `ai-guard-demo-api` | Serveur de ressources bearer-only et audience des access tokens ; aucun flux de connexion ou compte de service. |

Aucun `client_secret` n'est distribué par les clients publics. Implicit, grants mot de passe, device flow, CIBA, comptes de service et token exchange sont désactivés. Ne remplacez pas un échec du flux navigateur par un grant mot de passe ou une copie manuelle de jeton.

Enregistrez la référence CLI sous la forme canonique ci-dessus, sans `:80` explicite. Keycloak 26.8.1 retire le port HTTP par défaut lors de sa comparaison loopback native ; un littéral enregistré avec `:80` peut empêcher cet appariement. Le CLI écoute toujours sur un port local éphémère et l'inclut dans la demande d'autorisation réelle. Conservez l'IP littérale, le chemin exact `/oidc/callback` et S256 obligatoire, sans joker. Cette configuration seule ne prouve pas une connexion CLI réussie. L'import au démarrage saute les realms existants ; suivez la procédure de conservation des données ci-dessous pour modifier un enregistrement existant.

### Enregistrer les retours d'accusé de déconnexion

Le [modèle de realm](../../../deploy/keycloak-demo/ai-guard-demo-realm.json) utilise l'attribut client `post.logout.redirect.uris`. Keycloak 26.8.1 conserve plusieurs valeurs dans une chaîne séparée par `##` : la valeur Web est `${AI_GUARD_DEMO_WEB_ORIGIN}/##https://127.0.0.1/oidc/logout-complete/web`. Le CLI a la seule valeur explicite `https://127.0.0.1/oidc/logout-complete/cli`. N'utilisez pas `+`, l'héritage des redirects de connexion, des jokers ou le port de connexion éphémère du CLI. L'ancien enregistrement logout Web est conservé. Voir l'[encodage multivaleur épinglé](https://github.com/keycloak/keycloak/blob/26.8.1/services/src/main/java/org/keycloak/protocol/oidc/AbstractClientConfigWrapper.java#L60-L71), le [séparateur](https://github.com/keycloak/keycloak/blob/26.8.1/server-spi-private/src/main/java/org/keycloak/models/Constants.java#L140-L147) et le [traitement des enregistrements logout](https://github.com/keycloak/keycloak/blob/26.8.1/services/src/main/java/org/keycloak/protocol/oidc/OIDCAdvancedConfigWrapper.java#L439-L469).

Les deux clients publics fixent explicitement `logout.confirmation.enabled` à la chaîne `false`, valeur par défaut de cette version épinglée ([clé de l'attribut](https://github.com/keycloak/keycloak/blob/26.8.1/server-spi-private/src/main/java/org/keycloak/protocol/oidc/OIDCConfigAttributes.java#L65), [défaut](https://github.com/keycloak/keycloak/blob/26.8.1/services/src/main/java/org/keycloak/protocol/oidc/OIDCAdvancedConfigWrapper.java#L369-L374)). La politique fournisseur doit permettre une fin directe avec l'ID-token hint fourni ; ce réglage seul ne prouve pas la réponse. Une confirmation HTML interactive reste non confirmée par ai-guard. Préservez la politique approuvée de l'installation sans l'assouplir globalement.

Le [manifeste local](../sso/examples/local-demo.json) et l'[exemple gateway](../sso/examples/gateway.keycloak-demo.json) correspondants sélectionnent ces deux URI HTTPS comme seuls accusés de protocole. ai-guard compare un `302`/`303` direct, un `Location` exact unique contenant uniquement son state de requête neuf et un corps vide ; il ne suit jamais l'URI, n'ouvre pas de navigateur pour elle et n'installe pas de callback logout. Aucun listener TLS, certificat, entrée DNS ou service OS n'est nécessaire ni créé sur ces chemins. C'est le profil de réponse pris en charge par ai-guard, pas une règle de corps OIDC universelle. Issuer et endpoint end-session restent ceux de la DEMO HTTP isolée ; des URI d'accusé HTTPS ne qualifient pas ce déploiement en HTTPS d'entreprise. Voir le [contrat complet de réponse et compatibilité](../sso/README.fr.md#configurer-les-accusés-de-déconnexion-fournisseur). Le [parcours CLI natif réel du4novembre](../../engineering/VALIDATION.md#4-novembre--sso-cli-réel-renouvellement-et-déconnexion) passe sur LinuxARM64 isolé avec Keycloak26.8.1, refresh et accusé logout inclus. La qualification du nouveau profil Web, de macOS/TTY natif et de HTTPS d’entreprise reste ouverte.

Les deux clients publics ont exactement `basic` et `profile` comme client scopes par défaut, sans scope facultatif. `basic` fournit le mapper du sujet des access tokens Keycloak ; le jeton signé doit contenir un `sub` non vide. Son absence est une erreur de configuration du fournisseur, pas un motif d'assouplissement du validateur. `basic` peut être absent du champ textuel `scope` du jeton : il s'agit d'un client scope par défaut dont le mapper est appliqué par le fournisseur.

Deux mappers dédiés ajoutent `aud=ai-guard-demo-api` et le tableau `resource_access.ai-guard-demo-api.roles` aux seuls access tokens. Des scope mappings de rôle explicites et `fullScopeAllowed=false` limitent cette projection. Seul `admin-ai-guard` reçoit le rôle administrateur applicatif. Un ID token établit la connexion OIDC ; il ne sert jamais de bearer API.

## Contrat de configuration de l'identité du produit

Consignez les éléments suivants dans une configuration OIDC d'installation JSON distincte et versionnée. Ce contrat de référence est séparé du registre de tenants version 1 ; n'ajoutez pas ces champs au registre et ne déduisez aucune option de démarrage depuis ce guide. L'exemple de configuration et son mécanisme d'application documenté doivent rester synchronisés avec ce contrat.

| Paramètre | Valeur DEMO ou règle |
|---|---|
| Issuer | Issuer exact ci-dessus ; même écriture pour navigateur, CLI et contrôle. |
| IDs clients | IDs Web/CLI distincts ci-dessus ; Code+S256 obligatoire. |
| Retour de connexion Web | Origine et callback configurés exactement ; aucun joker. |
| Retour de connexion CLI | IP loopback littérale, port d'écoute éphémère, chemin fixe `/oidc/callback` ; fermeture après succès, erreur ou délai. |
| Retours d'accusé logout | `gateway.provider.web_post_logout_redirect_uri` et `cli_post_logout_redirect_uri` facultatifs, exigeant chacun `end_session_endpoint` ; enregistrements HTTPS statiques canoniques exacts ci-dessus, sans query, fragment, userinfo ou joker. Distincts des callbacks de connexion. |
| Validation API | RS256, issuer et audience exacts, `exp`, `iss`, `sub`, `aud` obligatoires, `nbf` contrôlé s'il est présent ; tolérance d'horloge bornée. Profil DEMO : header JWT `typ=JWT`, payload `typ=Bearer`, `azp` limité aux deux clients publics. |
| Scopes demandés | `openid` et informations de profil minimales ; client scopes par défaut `basic`+`profile` côté fournisseur. |
| Mapping administrateur | Chemin de claim de l'access token vérifié `resource_access.ai-guard-demo-api.roles`, valeur `admin-ai-guard` ; application aux seuls périmètres applicatifs explicitement attribués. |
| Appartenances | Associations administrées côté contrôle, indexées par `(issuer, sub)` vérifiés. Ni email, ni nom de compte, ni header libre, ni profil modifiable par l'utilisateur ne choisit un tenant. |
| Réseau et confiance | Destinations discovery/JWKS/token approuvées, requêtes et caches bornés, confiance explicite dans l'AC publique pour HTTPS. Pas d'accès Internet général pour les services de contrôle internes. |
| Politique de session | Durée des jetons, rotation et réutilisation des refresh tokens, changements de mapping, logout et bornes de révocation publiés pour cette installation. |

Pour le profil local, le fournisseur fixe les access tokens à 120 s, les codes d'autorisation à 30 s, l'inactivité SSO à 900 s et les sessions absolues à 3600 s. La rotation des refresh tokens est activée, sans réutilisation autorisée. Ces valeurs ne rendent pas immédiate la révocation d'un JWT validé hors ligne. Approuvez les bornes réelles des caches et de la révocation dans la politique d'installation.

Rust vérifie les signatures et le profil de l'access token avant de mapper rôles ou appartenances. Les transactions lient issuer, client, retour, state, nonce et vérificateur privé, avec expiration et consommation unique. Les jetons Web restent en mémoire, hors stockage navigateur, URL et logs. Le broker conserve sa propre identité de workload ; une connexion humaine ne fournit jamais de credential `runner` et n'envoie aucun jeton humain dans un worker.

## Utiliser l'application avec ces identités

Configurez les appartenances explicites et le périmètre d'administration de la DEMO dans le contrôle. Ouvrez le dashboard à l'origine Web configurée et authentifiez `user-1` auprès du fournisseur. Connectez le CLI par son flux navigateur natif avec le même compte ; vérifiez que les deux identifient le même `(issuer, sub)` et le contexte autorisé.

Consultez seulement les ressources réelles explicitement attribuées ou partagées avec ce principal par le contrôle, selon la [procédure de comptes, memberships et grants de ressources OIDC](../sso/README.fr.md#établir-comptes-memberships-et-ressources). Conservez le principal exact `(issuer, sub)` vérifié dans ces enregistrements ; un nom d'utilisateur ne le remplace pas. La préparation DEMO neuve ne crée ni grant de ressource ni session : une liste vide autorisée est donc attendue. Avec une ressource existante autorisée, vérifiez métadonnées, contenu et export sous leurs permissions respectives ; exportez dans une destination neuve et conservez les reçus de lecture. Avec `user-2`, vérifiez le refus des ressources privées et des contextes non autorisés, puis utilisez `admin-ai-guard` pour consulter son seul périmètre d'administration attribué. Le compte master ne doit pas obtenir d'accès applicatif implicite.

La soumission d'une tâche sous identité humaine locale/OIDC et son attribution authentifiée via le broker restent [proposées](../../engineering/HUMAN_EXECUTION.md). Les commandes techniques `run` et broker existantes exigent une autorité de contrôle de confiance distincte ; une connexion ne les autorise pas et n'attribue pas leurs exécutions à ce principal humain.

À la déconnexion, effacez l'identité locale et les transactions privées, annulez les lectures en attente et fermez les abonnements. Confirmez séparément le logout du fournisseur ; la déconnexion locale doit rester possible pendant sa panne. Un retrait de rôle ou d'appartenance doit aussi retirer les données et abonnements auparavant visibles dans la borne de révocation configurée. Un échec SSO ne change jamais d'identité ni de fournisseur d'inférence facturé.

Lisez séparément `local_session` (fermeture de l'admission ai-guard), `audit` (audit du contrôle) et `provider_logout` (réponse fournisseur prise en charge). Dans la sortie du CLI natif, `control_provider_logout:not_attempted` signifie que le gateway n'a pas tenté le logout fournisseur : le CLI natif conserve le hint en privé et décrit son propre appel dans `provider_logout`. Un accusé `accepted` n'affirme ni révocation JWT globale ni fermeture de toutes les autres sessions. Le [guide SSO](../sso/README.fr.md#se-connecter-dans-le-navigateur) explique les résultats échoués ou inconnus.

## Raccorder un fournisseur d'entreprise et HTTPS

Obtenez auprès de l'administrateur d'identité un issuer approuvé, les retours Web/CLI exacts, des clients publics, l'audience API, le profil de jeton, le mapping des claims et la politique de session. Utilisez un realm normal distinct ou la configuration du fournisseur, sans importer les comptes ou noms de rôles de cette DEMO. Gardez l'identité locale/hors ligne et les choix abonnement/API des agents indépendants d'OIDC.

Employez le même issuer HTTPS à l'intérieur et à l'extérieur du réseau conteneurisé. Accordez DNS, SAN des certificats, chaîne et AC publique de confiance. Servez le certificat feuille et la chaîne intermédiaire au point TLS approuvé ; transmettez uniquement les ancres publiques aux clients. Aucune clé privée d'AC dans une image, un worker, un dépôt ou un rapport. Pour préparer des certificats locaux, consultez le [guide d'AC locale](../../demo-pki.fr.md) ; configurez explicitement la confiance de chaque client, sans installation globale automatique ni contournement insecure.

Approuvez le chemin réseau du contrôle vers le fournisseur avant d'activer le raccord. Gateway et dashboard conservent leur frontière réseau interne ; un navigateur joignant un issuer ne prouve pas son accessibilité depuis les conteneurs. N'autorisez que les destinations d'identité nécessaires et préservez les guards d'origine/callback. Discovery, redirects et URL fournis par un JWT ne doivent pas devenir un proxy général ou un mécanisme de récupération arbitraire de JWKS.

Avant d'admettre les utilisateurs, exercez connexion Web/CLI, deux périmètres applicatifs distincts, retrait de rôle, expiration, refus de réutilisation du refresh, panne du fournisseur et rotation des clés. Contrôlez la nouvelle clé via l'issuer configuré et refusez une clé inconnue après rafraîchissement borné. Documentez la reprise sans assouplir issuer, audience, signature, TLS ou validation tenant. Le [guide d'installation entreprise](../enterprise/README.fr.md) couvre les choix de déploiement associés.

## Conserver les données et reprendre

Conservez le nom du projet, le fichier d'environnement, les identités des images et la référence du volume dans le dossier d'installation. Un redémarrage conserve utilisateurs et données du fournisseur DEMO :

```sh
docker compose --env-file .env.exemple.local-keycloak -p ai-guard-keycloak \
  -f deploy/keycloak-demo/compose.yaml --profile keycloak-demo-http stop
docker compose --env-file .env.exemple.local-keycloak -p ai-guard-keycloak \
  -f deploy/keycloak-demo/compose.yaml --profile keycloak-demo-http start
```

L'import au démarrage saute un realm existant. Modifier le JSON du realm, changer l'origine Web ou reconstruire une image ne migre pas ce realm. Préparez une modification explicite et bornée du fournisseur, ou un nouveau projet DEMO pour une autre configuration. Ne supprimez jamais un volume existant pour réparer une connexion.

Avant une mise à jour du fournisseur, conservez une sauvegarde cohérente et les références exactes des images/configurations précédentes selon sa procédure de sauvegarde. Un export JSON de realm seul n'est pas une sauvegarde complète de la base. Restaurez d'abord dans une destination isolée neuve, puis vérifiez issuer, utilisateurs, mappings et politique d'autorisation de l'application. Conservez les anciennes données et identités jusqu'à confirmation de la reprise.

### Mettre à jour les enregistrements logout existants

1. Gardez realm, utilisateurs, sujets, credentials, mappings et volumes actuels. Inventoriez les réglages des deux clients et réalisez les sauvegardes cohérentes fournisseur/contrôle décrites ci-dessus et dans le [guide de reprise SSO](../sso/README.fr.md#diagnostiquer-et-reprendre).
2. Par l'interface d'administration Keycloak approuvée par l'opérateur, ajoutez l'URI d'accusé Web exacte dans **Valid post logout redirect URIs** de ce client, en préservant son retour Web exact existant. Ajoutez l'URI d'accusé CLI explicite à la liste post-logout du client natif. Pour la DEMO, utilisez les valeurs HTTPS littérales ci-dessus ; remplacez `${AI_GUARD_DEMO_WEB_ORIGIN}/` par l'origine réelle de l'installation suivie de `/` lors de l'édition d'un client stocké. `##` est le séparateur de l'attribut sérialisé, pas une partie des URI saisies séparément dans la console. Préservez les autres entrées exactes approuvées ; n'utilisez ni `+`, héritage de la liste de connexion ou jokers. Ne modifiez ni enregistrement natif canonique de connexion, ni drapeaux de client public, Code/S256, grants désactivés ou mappers de tokens.
3. Vérifiez la politique de fin directe approuvée des clients (`logout.confirmation.enabled:false` dans la DEMO distribuée). Ne changez pas de politique de session globale sans rapport. Préparez une configuration OIDC privée relue avec `web_post_logout_redirect_uri` et `cli_post_logout_redirect_uri` correspondants et l'endpoint end-session exact existant.
4. Activez uniquement avec des binaires gateway et CLI natif mis à jour correspondants, selon la [procédure de mise à jour existante](../sso/README.fr.md#mettre-à-jour-la-configuration-logout-sans-remplacer-les-données), l'arrêt/redémarrage approuvé et une nouvelle authentification. Les anciens binaires refusent ces nouveaux champs ; les omettre conserve le fonctionnement antérieur. Ne rejouez pas la préparation neuve, ne réinitialisez pas le realm, ne supprimez pas de volume et ne remplacez pas d'audit pour appliquer ce changement. Vérifiez séparément fermeture, audit et réponse fournisseur ; modifier ces réglages ne fournit aucun résultat réel de logout.

## Diagnostiquer une connexion refusée

| Symptôme | Vérification et reprise |
|---|---|
| Nom introuvable ou connexion refusée | DNS hôte, alias interne, port configuré, listener loopback publié et bon projet. Rétablir le chemin approuvé. |
| Issuer différent | Discovery, nom d'hôte, schéma, port et realm doivent correspondre exactement. Corriger la configuration ; ne pas réécrire l'issuer reçu. |
| Sujet absent | Vérifier `basic` par défaut sur les deux clients publics et son `oidc-sub-mapper`. Conserver `sub` obligatoire. |
| Audience ou rôle refusé | Vérifier audience de l'access token, mapper de rôle API et appartenances approuvées. L'administration master n'est pas une autorisation applicative. |
| Callback refusé | Origine/chemin exacts, client choisi, expiration de transaction, state et nonce. Commencer une nouvelle connexion ; ne pas rejouer un code. |
| TLS refusé | SAN, chaîne, validité et bundle d'AC explicite du client. Réparer confiance ou certificats ; garder la validation TLS active. |
| Fournisseur indisponible ou session expirée | Afficher l'erreur de connexion, préserver la provenance d'audit et demander une authentification si nécessaire. Une vue déconnectée n'est pas une absence d'activité. |
| Logout non confirmé | Vérifier URI post-logout exacte par client et politique de fin directe. Une page de confirmation ou un redirect différent/à corps non vide n'est pas un accusé accepté. Conserver le résultat local/audit distinct ; ne jamais suivre ni journaliser Location, state ou hint reçus. |

Les diagnostics ne conservent ni bearer, code d'autorisation, vérificateur, cookie ou query de callback. Ne persistez pas de réponse brute du fournisseur ou de dump complet d'environnement/inspection. Les harnais de tests auxiliaires locaux ne sont pas des commandes distribuées du produit.

## Références

- [Conteneurs Keycloak](https://www.keycloak.org/server/containers), [import de realm](https://www.keycloak.org/server/importExport), [configuration hostname](https://www.keycloak.org/server/hostname).
- [Source du client scope basic Keycloak 26.8.1](https://github.com/keycloak/keycloak/blob/26.8.1/services/src/main/java/org/keycloak/protocol/oidc/OIDCLoginProtocolFactory.java#L563-L579), [traitement des redirects loopback](https://github.com/keycloak/keycloak/blob/26.8.1/services/src/main/java/org/keycloak/protocol/oidc/utils/RedirectUtils.java) et [constructeur d'URI canonique](https://github.com/keycloak/keycloak/blob/26.8.1/common/src/main/java/org/keycloak/common/util/KeycloakUriBuilder.java).
- [OIDC Core](https://openid.net/specs/openid-connect-core-1_0.html), [PKCE RFC 7636](https://www.rfc-editor.org/rfc/rfc7636.html), [clients natifs RFC 8252](https://www.rfc-editor.org/rfc/rfc8252.html), [sécurité OAuth RFC 9700](https://www.rfc-editor.org/rfc/rfc9700.html).
