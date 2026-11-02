# Configurer et utiliser le SSO ai-guard

[English](README.md) · [Index d’installation](../README.fr.md) · [Fournisseur Keycloak](../keycloak/README.fr.md) · [Accès local nominatif](../identity/README.fr.md)

Ce guide décrit la configuration OIDC Rust, la connexion Web et le CLI humain au premier plan. Utilisez-le avec le binaire ai-guard correspondant et le frontend compilé. Il ne configure pas l’authentification Codex ou Claude : choisissez séparément leur abonnement, clé API ou endpoint d’inférence approuvé dans le [guide fournisseurs](../providers/README.fr.md).

Les étapes supposent une installation de contrôle existante et un fournisseur d’identité approuvé par l’opérateur. Le profil Keycloak distribué prépare le fournisseur ; il n’initialise ni les comptes ai-guard, ni les memberships, ni les propriétaires des ressources, ni le raccordement réseau des conteneurs. L’interopérabilité réelle Web/CLI avec Keycloak, l’installation conteneurisée complète et l’exploitation de production demandent leurs qualifications propres. Consultez les [preuves datées d’implémentation et de validation](../../engineering/VALIDATION.md) et le [contrat produit d’identité](../../sso.md) pour la cible complète.

## Comprendre les connexions

| Composant | Responsabilité |
|---|---|
| Gateway Rust | Possède les transactions PKCE Web, valide les réponses du fournisseur et les access tokens API, admet les sessions, applique mapping et permissions du contrôle, puis conserve l’audit d’authentification. |
| Dashboard Rust | Sert le frontend compilé et relaie uniquement des routes d’authentification fixes vers son gateway configuré. Aucun cookie de session métier n’authentifie l’utilisateur. |
| Navigateur | Utilise le client Web public. Conserve les access/refresh tokens en mémoire après leur remise unique par le gateway. Un cookie opaque bref corrèle la connexion ; il ne contient ni token ni vérificateur PKCE. |
| CLI humain | Utilise un client public distinct, un navigateur externe et un listener loopback éphémère. Possède son échange PKCE ; le gateway valide et admet indépendamment l’access token. |
| Broker et worker | Gardent leurs identités d’exécution distinctes. Une connexion SSO humaine ne fournit aucun credential `runner` et ne place aucun token humain dans un worker. |

Le SSO permet actuellement la consultation et l’export humains autorisés. Ne transmettez pas son token à `broker submit` et ne déduisez pas d’une connexion le droit de lancer un agent. La délégation d’exécution humaine et la vue complète d’administration restent des exigences produit distinctes.

## Préparer l’installation

1. Préparez les images gateway/dashboard et le frontend compilé selon le [guide local](../local/README.fr.md), ou le déploiement d’entreprise approuvé. Préservez registre de tenants, fichiers de credentials, politique, volumes de données et identité des services existants.
2. Arrêtez gateway, brokers et redémarrages automatiques avant la maintenance d’identité hors ligne. `--confirm-control-stopped` est une attestation de l’opérateur ; ce drapeau ne bloque pas un autre processus.
3. Migrez explicitement chaque base tenant configurée vers le schéma5 pour les lectures humaines, avec une nouvelle sauvegarde vérifiée à chaque étape. Suivez la [procédure de stockage](../broker/README.fr.md) et l’[exemple de migration de l’accès humain](../identity/README.fr.md). Ni l’ajout OIDC ni la réouverture du gateway ne migre une base.
4. Obtenez l’issuer exact vérifié et le sujet stable de chaque utilisateur approuvé par la procédure d’administration d’identité. Le principal est `(issuer, sub)`. Un nom de connexion, email, nom affiché ou token collé par l’utilisateur n’établit pas une appartenance approuvée.
5. Inventoriez les accès depuis navigateur, CLI et gateway, ainsi que le trajet dashboard→gateway. Ils doivent conserver la même identité d’issuer. Depuis un conteneur, une adresse loopback hôte désigne ce conteneur ; l’accès du navigateur ne prouve aucun trajet conteneurisé.

Pour une base appartenant à un conteneur, effectuez la maintenance avec l’UID/GID du contrôle existant et son volume existant, tous les autres consommateurs arrêtés. Le [guide d’accès local nominatif](../identity/README.fr.md) décrit le conteneur de maintenance isolé. Les mutations de comptes OIDC emploient la même autorité opérateur et ne nécessitent aucun credential humain local.

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
| Déconnexion | Endpoint end-session exact facultatif. L’opération implémentée envoie `client_id` et `id_token_hint` par POST ; elle ne réalise pas de redirect de déconnexion interactif. |

Le profil d’access token actuel est plus étroit qu’OIDC en général. Entra ID ou un autre fournisseur n’est utilisable que si ses tokens signés et réponses discovery satisfont réellement ce profil ; son nom ou sa conformité OIDC générale ne prouve pas sa compatibilité.

## Préparer le JSON privé du gateway

Partez d’un exemple public et copiez-le dans un nouveau fichier privé d’opérateur, hors des workspaces sources, des assets frontend et des exports workers :

| Exemple | Usage |
|---|---|
| [gateway.https.json](examples/gateway.https.json) | Format HTTPS avec adresses `example.invalid` volontairement non fonctionnelles. Remplacez issuer, endpoints, IDs clients, audience, origine, règle de rôle et UUID d’installation par les valeurs approuvées. |
| [gateway.keycloak-demo.json](examples/gateway.keycloak-demo.json) | Valeurs exactes par défaut du fournisseur HTTP DEMO isolé distribué. Nécessite une préparation distincte des comptes/memberships et un trajet gateway→fournisseur approuvé. Remplacez l’UUID d’installation d’exemple. |

Ces fichiers contiennent des valeurs publiques d’exemple, aucun credential ni identité existante. Générez un UUID d’installation canonique en minuscules, non nul et neuf pour un nouvel audit. Conservez-le lors des réouvertures de cet audit. Le parseur JSON ne développe **pas** les variables d’environnement : `${NAME}` n’est pas un mécanisme de configuration. Les variables Keycloak des `.env.exemple.*` configurent ses services Compose ; elles n’activent pas le SSO du gateway.

La configuration est en version 1 et refuse les champs inconnus dans chaque objet déclaré. Ses champs racine sont :

| Champ | Signification |
|---|---|
| `version` | Doit valoir `1`. |
| `gateway` | Objet version 1 : `provider`, `provider_label`, `web_origin`, `mapping`. |
| `authentication_audit` | `path`, `installation_id` et `policy`. Base privée d’authentification distincte de chaque base tenant. |
| `ca_bundle_file` | Facultatif/null, ou chemin vers les certificats PEM d’AC **publics** approuvés pour le fournisseur OIDC. |

L’objet provider des exemples montre tous les champs obligatoires. `end_session_endpoint` est facultatif/null ; omettre `limits` applique les valeurs ci-dessous. `transport` vaut exactement `{"kind":"https_only"}` dans une installation normale. La DEMO isolée utilise `{"kind":"isolated_demo_http","exact_issuer":"..."}` ; cet issuer exact doit correspondre à `issuer`. HTTP est limité aux issuers sur IP loopback littérale ou à l’issuer fixe de la DEMO distribuée. Son origine/redirect Web doit être une adresse loopback HTTP littérale. Cette exception n’approuve pas un fournisseur HTTP intranet arbitraire.

Les endpoints d’autorisation, token, JWKS et logout facultatif sont des URL exactes approuvées par l’opérateur. Discovery doit les confirmer ; il n’ajoute pas de destination. Les URL ne contiennent ni userinfo, query ni fragment. Le client HTTP du fournisseur désactive proxies ambiants, redirects et retries automatiques. Approuvez également DNS et destinations sortantes dans le déploiement : la liste d’URL n’est pas un pare-feu de conteneur.

Les chemins `authentication_audit.path` et `ca_bundle_file` sont absolus ou relatifs au parent du fichier OIDC. Traversées parent et liens symboliques sont refusés. Préparez les fichiers sous l’UID réel du contrôle, fichiers privés `0600`, répertoires privés `0700` et ancêtres/ACL protégés. Le parent de la base d’authentification doit être inscriptible par cet UID ; rendre le fichier de configuration en lecture seule ne rend pas son parent entier adapté à un audit en lecture seule. L’entrée OIDC est limitée à 1 Mio.

Pour une AC d’entreprise, donnez à `ca_bundle_file` un fichier vérifié contenant uniquement les certificats publics approuvés, au plus 64 Kio et 8 certificats. Ces racines remplacent les racines intégrées du seul client OIDC, avec contrôle de chaîne, nom et validité conservé. Le gateway publie aussi ces certificats publics dans le profil CLI. Cette option ne change ni le magasin de confiance du navigateur ni la confiance TLS du CLI humain pour le **gateway**. `human sso` n’expose actuellement aucun drapeau d’AC gateway spécifique ; utilisez un certificat gateway déjà reconnu et qualifiez ce trajet. Ne fournissez aucune clé privée et ne désactivez pas TLS.

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

## Établir comptes, memberships et ressources

L’opérateur doit créer un compte OIDC actif et un membership dans chaque base tenant voulue. Une connexion ne les provisionne pas. Utilisez ces modèles publics dans l’ordre :

1. [account.json](examples/account.json) : principal `human_oidc` approuvé et nom affiché.
2. [membership.json](examples/membership.json) : même principal, liaison tenant réelle, organisation et capacités permises.
3. [resource-grant.json](examples/resource-grant.json) : ressource déjà connue dans ce tenant, avec propriétaire et organisation approuvés. L’exemple ne partage rien.

Remplacez chaque UUID d’exemple par un UUID canonique neuf en minuscules et non nul pour son opération ou membership ; conservez chaque identité pour les retries exacts. Remplacez `REPLACE_WITH_VERIFIED_SUBJECT`, issuer, tenant, organisation et ID ressource de façon cohérente. Pour la DEMO, reprenez son issuer HTTP exact dans les trois modèles. Ces fichiers ne préattribuent aucun ID sujet. Un compte sans membership n’accorde aucun accès ; un membership seul n’attribue pas les ressources historiques.

Enregistrez chaque mutation relue dans un fichier privé sous l’autorité de contrôle existante. Pour une base schéma4/5 existante à supervision inactive, lancez la commande suivante, puis répétez-la avec `oidc-membership.json` et `oidc-resource-grant.json` après la réussite de chaque commande précédente :

```sh
AIGUARD_BIN="/absolute/path/to/agentsandbox"
AIGUARD_DATA="/absolute/private/ai-guard/tenant.sqlite"
AIGUARD_TENANT="actual-configured-tenant"
AIGUARD_PRIVATE="/absolute/private/ai-guard"
"$AIGUARD_BIN" human apply \
  --data "$AIGUARD_DATA" --tenant-id "$AIGUARD_TENANT" \
  --mutation-file "$AIGUARD_PRIVATE/oidc-account.json" \
  --confirm-control-stopped
```

L’entrée accepte seulement `version`, `operation_id` et `change` typé ; un champ `actor` est refusé. L’autorité vient de l’opérateur effectif du filesystem. Chaque mutation et son audit sont atomiques ; les trois commandes distinctes ne forment pas une seule transaction. Après un résultat ambigu, préservez le fichier et rejouez exactement opération et octets. Un sens différent exige un nouvel UUID d’opération. `expected_revision:0`/`revision:1` désigne une création ; modifier un enregistrement existant exige sa révision précédente exacte et la suivante. N’éditez pas directement les projections SQLite.

### Configurer les permissions effectives

`gateway.mapping` définit un plafond de capacités à partir des claims vérifiés de l’access token. Il part de `default_capabilities`, puis ajoute les capacités des `rules` correspondantes. Chaque `claim_path` est un tableau de clés d’objets ; sa feuille doit être un tableau de chaînes bornées. `equals` correspond à une valeur exacte. Un claim absent ne correspond pas ; un type mal formé refuse l’authentification. Deux règles de mêmes chemin/valeur sont refusées.

Les droits effectifs sont l’intersection de ce plafond et du membership actif côté contrôle, puis du contexte choisi et des permissions de ressource. Les exemples permettent `read_metadata`, `read_content` et `export` en contexte personnel. La règle administrateur ajoute `read_tenant_metadata` ; le membership de l’administrateur voulu doit aussi le contenir explicitement. Les autres memberships ne doivent pas l’acquérir par la seule organisation commune. Les capacités administratives sont interdites dans `default_capabilities`.

Les capacités acceptées sont `read_metadata`, `read_content`, `export`, `read_tenant_metadata`, `manage_memberships` et `manage_grants`. Une capacité configurable ne prouve pas l’existence d’un parcours Web complet de mutation ou d’administration. Les contextes personnel et administration sont distincts et doivent être proposés par Rust. Administration du realm, noms de comptes et rôles d’un autre client ne donnent aucun droit applicatif ou accès inter-tenant implicite.

Changez `mapping.revision` à chaque évolution du mapping approuvé et redémarrez le gateway avec la configuration relue. Pour retirer membership/compte/grant, appliquez une nouvelle mutation hors ligne auditée avec révision incrémentée et `active:false` selon le cas. Arrêtez tous les consommateurs, puis redémarrez et imposez une nouvelle authentification. Un changement de rôle IdP seul ne révoque pas immédiatement un token émis ; les rôles changent avec de nouveaux claims vérifiés, tandis que les lectures humaines revalident les permissions du contrôle.

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

Les listeners Rust sont HTTP ; une origine publique HTTPS ne crée pas de terminaison TLS. Préparez et qualifiez le transport approuvé avant l’exposition de l’installation. Le [guide Keycloak existant](../keycloak/README.fr.md) couvre projet fournisseur, DNS et comptes publics DEMO. Les Compose principaux ne raccordent pas automatiquement le gateway à son réseau d’identité interne. L’opérateur doit préparer ce raccordement gateway restreint en conservant le même issuer et en laissant dashboard/workers hors de ce réseau ; ce guide ne prétend pas qualifier cette topologie.

## Se connecter dans le navigateur

Ouvrez l’origine exacte du dashboard configuré. L’écran vérifie l’état SSO et affiche libellé du fournisseur et issuer. Choisissez **Se connecter avec …**, terminez la connexion chez le fournisseur, puis choisissez un contexte renvoyé par ai-guard. Un compte fournisseur valide sans membership applicatif approuvé est refusé. Une connexion réussie sans ressource autorisée peut afficher une page vide ; un défaut d’authentification, de stockage ou de réseau doit rester une erreur.

Le gateway consomme son callback et envoie le navigateur vers `/auth/oidc/complete` sans transmettre les paramètres code/state. Les tokens sont remis une seule fois puis gardés en mémoire, hors URL, Web Storage, logs et exports. Rechargement, fermeture de page ou redémarrage gateway exigent une nouvelle connexion explicite. Ne recopiez pas un token des outils développeur dans un fichier ou une autre commande CLI. Le navigateur coordonne le refresh lorsqu’un refresh token est disponible ; un refus ou résultat ambigu ferme la session locale. Une nouvelle génération remplace la vue autorisée au lieu de conserver l’ancien contexte.

Utilisez l’action de déconnexion de l’application. L’état local du navigateur est effacé même si contrôle/fournisseur ne peuvent confirmer le logout. Lisez séparément les résultats `local_session`, `provider_logout` et `audit` lorsqu’ils sont disponibles. `provider_logout:accepted` décrit la réponse de l’endpoint pris en charge ; ce n’est ni une révocation JWT globale ni la preuve de fermeture des autres sessions navigateur/client.

## Se connecter et travailler dans le CLI au premier plan

Exécutez le CLI de confiance sur le poste utilisateur, vers l’**origine gateway**, sans `/api` ni autre chemin. C’est la connexion ai-guard, distincte de la connexion du CLI fournisseur :

```sh
AIGUARD_BIN="/absolute/path/to/agentsandbox"
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

Tokens et vérificateur PKCE restent dans ce processus ; aucun cache durable de tokens ou fichier de credential n’est créé. `refresh` est explicite dans le CLI. Après sa réussite, demandez `identity` et choisissez un nouveau contexte proposé ; anciens contextes/curseurs et générations d’export sont invalides. Un refresh échoué ou ambigu ferme la session au premier plan, sans retry automatique. `logout`, `quit`, `exit`, EOF et Ctrl-C effacent l’état local ; la connexion suivante est une nouvelle session. Un redémarrage gateway perd les admissions vivantes malgré l’audit durable.

L’export demande un parent privé existant et une destination finale **neuve**. Son budget est de 25 pages, 4 Mio et 60 secondes depuis la préparation initiale. Après interruption, conservez le répertoire `.preparing` adjacent. `resume-export --output-dir /absolute/private/ai-guard/new-export` exige cette préparation exacte, le même contexte, la session au premier plan encore vivante et sa génération d’admission, dans l’échéance initiale. Refresh, arrêt/redémarrage du processus, autorisation expirée ou budget épuisé imposent un nouvel export explicitement autorisé vers une autre destination neuve ; conservez la préparation non résolue. Un reçu de lecture prouve l’enregistrement du contrôle, pas la livraison des octets ni la vérification d’un audit signé complet.

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
| `audit_unavailable` | Audit d’authentification/lecture obligatoire non fiable, non inscriptible ou capacité épuisée. Préservez les données et réparez stockage/configuration avant retry. Aucun contournement. |
| `capacity_exhausted` | Capacité d’authentification vivante bornée. Attendez l’expiration des tentatives puis réessayez explicitement ; ne créez pas un autre audit pour dépasser la borne. |
| Connexion perdue / données périmées | Gardez un état déconnecté/périmé explicite. Reconnectez-vous et choisissez un contexte actuel avant reprise ; une perte n’est pas une activité nulle. |

Conservez fichier OIDC, identités exactes de binaires/images, registre tenants, identités/grants privés et bases tenants avec l’inventaire de l’audit d’authentification séparé. Cet audit conserve des résultats typés et empreintes de tokens, sans tokens récupérables. Il n’est pas atomique avec l’IdP externe : un échange fournisseur suivi d’un crash peut laisser une issue inconnue nécessitant une nouvelle connexion.

Aucune commande CLI de sauvegarde/restauration de l’audit d’authentification n’est exposée ici. La bibliothèque Rust possède une sauvegarde neuve vérifiée ; n’inventez pas de wrapper shell et ne prenez pas la copie du seul fichier SQLite actif pour une sauvegarde cohérente. Avant sauvegarde/reprise, arrêtez tous les consommateurs et préservez un ensemble SQLite cohérent, identité d’installation, politique d’audit et configuration existante selon la procédure de stockage approuvée. Conservez l’existant et validez la reprise dans une destination isolée neuve avant bascule. La reprise ne restaure ni tokens, sessions vivantes ou autorité d’exécution.

L’audit vérifie sa chaîne et son plancher en processus vivant, mais détecter un rollback complet de base entre redémarrages demande un témoin indépendant. Cette configuration ne prouve ni anti-rollback, points de contrôle externes signés ou qualification de release. Les quotas comptent les enregistrements, genesis inclus, et les octets sérialisés ; pages SQLite, WAL/SHM, sauvegardes et usage filesystem s’y ajoutent. Un audit épuisé refuse de nouvelles opérations obligatoires ; une procédure de migration/rétention de sa politique reste à fournir avant d’atteindre cette limite en exploitation.
