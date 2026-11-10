# Identité individuelle, SSO et essai rapide

Décisions du mainteneur, 31 octobre 2026. **Contrat produit canonique, cible à implémenter et qualifier.** Il fixe l'identité individuelle, les axes de configuration, le SSO, l'inférence interne et la découverte ; [AGENTS.md](../AGENTS.md) et le [contrat d'architecture](engineering/ARCHITECTURE_CONTRACT.md) en conservent les invariants. Les exigences de vues sont dans [ui.md](ui.md#vues-utilisateur-et-administration-unifiée--cible-à-livrer), les lots dans le [plan](engineering/IMPLEMENTATION_PLAN.md) et les preuves attendues dans la [stratégie de tests](engineering/TEST_STRATEGY.md#identité-individuelle-sso-et-découverte--à-qualifier).

Le [quick start existant](quickstart.md) documente le parcours avec clés `runner`/`viewer`. L'[accès local nominatif](setup/identity/README.fr.md) dispose d'un guide distinct ; la configuration et les parcours OIDC candidats sont documentés en [anglais](setup/sso/README.md) et [français](setup/sso/README.fr.md). Les configurations locales Keycloak et Qwen sont distribuées avec leurs guides [anglais](setup/README.md) et [français](setup/README.fr.md) ; elles ne raccordent pas automatiquement ces services au produit. Les preuves datées dans [VALIDATION.md](engineering/VALIDATION.md) gardent leur périmètre ; cette spécification ne les étend pas.

## État livré et parcours disponibles

| Parcours | État constaté au 2 novembre 2026 |
|---|---|
| Clés locales `runner` / `viewer` | Présentes : le contrôle Rust associe chaque clé à un tenant et un rôle. Ces clés ne prouvent pas l'identité d'une personne. |
| Authentification locale nominative hors ligne | **Implémentée ; PASS logiciel ciblé et parcours Docker A/B API/CLI, export, redémarrage et révocation hors ligne** sur l'image `954246…` issue de `7afdd43`. Navigateur contre ce contrôle : NOT_RUN. Les clés statiques restent une autre classe d'identité. |
| Connexion OIDC du dashboard et du CLI, PKCE et autorisation humaine | **Implémentation candidate** Web/CLI et configuration privée versionnée. Sept flows gateway à issuer contrôlé, trois tests de désactivation, vingt-quatre tests du ledger et quatorze tests CLI humains locaux passent sur leurs gels documentés. Les vingt-quatre tests CLI SSO finaux passent ensemble, avec corrections de revue d'admission/rotation et fixtures causales ; revues traitées dans ce périmètre. Keycloak réel et navigateur : NOT_RUN ; bootstrap complet et qualification globale restent ouverts. |
| Profil Keycloak préconfiguré | **Configuration distribuée, qualification partielle** : Dockerfile épinglé, realm et Compose isolé dans [`deploy/keycloak-demo/`](../deploy/keycloak-demo/README.md), exemples `.env` et commandes dans les guides [EN](setup/keycloak/README.md) / [FR](setup/keycloak/README.fr.md). Le contrôle de configuration Compose ne qualifie pas le login Web/CLI ai-guard. |
| Raccordement OIDC entreprise | **Configuration et guide opérateur candidats disponibles ; qualification réelle NOT_RUN.** Le fournisseur doit satisfaire le profil signé exact accepté par Rust ; aucune compatibilité Entra ID ou autre OIDC arbitraire n'est présumée. Réseau, memberships et exploitation restent à préparer et qualifier. |
| Backend local d'inférence | **Configuration et Dockerfile distribués, build ARM64 vérifié** : [`deploy/local-ai/`](../deploy/local-ai/README.md) et guides [EN](setup/local-ai/README.md) / [FR](setup/local-ai/README.fr.md). L'image de ce build avec poids vérifiés est conservée localement, sans publication d'image ni nouveau lancement modèle dans cette preuve. |
| Profil administré d'inférence interne dans ai-guard | **À implémenter / NOT_RUN** ; ni le backend disponible ni les essais API fournisseurs ne qualifient son adaptation aux clients et à la politique entreprise. |

Le fonctionnement actuel est vérifiable dans [l'authentification Rust](../src/api/auth.rs), la [configuration gateway](../src/gateway_config.rs), les [routes OIDC](../src/api/oidc.rs) et le [client Web OIDC](../front/src/oidc-session.ts). `init` avec `--config-dir` prépare les clés techniques locales ; la saisie **Jeton de lecture** utilise la clé `viewer`. Ni cette initialisation ni `providers init/status`, réservés à Codex/Claude, ne configurent le SSO ai-guard. Le gateway accepte le fichier privé distinct `--oidc-config` ; `--initialize-oidc-audit` crée exclusivement un audit d'authentification neuf et se retire à la réouverture. Les champs fermés, memberships, commandes et limites sont dans le [guide SSO](setup/sso/README.fr.md) ; la [déclinaison technique](engineering/SSO_OIDC.md) conserve les exigences et la qualification restante.

Un gateway OIDC multi-tenant exige désormais un audit d’authentification V4 et le fichier privé version2 avec `authentication_audit.capacity_plan`, dont l’ensemble des tenants et le contenu correspondent exactement au plan durable. Les parts finies records/octets sont choisies côté Rust depuis les memberships vérifiés et ne donnent aucun droit ; aucun emprunt ni transfert d’une session à épuisement. La compatibilité V3/version1 reste explicite pour un seul tenant, sans garantie de partage de capacité. `setup prepare` version1 crée encore V3 : une préparation multi-tenant terminée doit suivre la [migration3→4 avant le premier démarrage](setup/sso/README.fr.md#migrer-un-audit-dauthentification-existant), consommateurs arrêtés, sauvegarde neuve et politique originale conservée. Un message « prepared » ne qualifie pas le démarrage ni le parcours SSO.

## Essayer sans configurer toute une infrastructure

L'objectif est qu'une personne découvrant ai-guard puisse démarrer la pile avec une commande Compose documentée, fournir une seule clé API du fournisseur choisi, ouvrir le dashboard, exécuter une première tâche non destructive et retrouver son audit. Aucun compte cloud ai-guard ni clé pour les autres fournisseurs ne doit être requis. Les modes abonnement doivent également être pris en charge selon leurs prérequis propres, sans basculement silencieux vers une API payante.

**Environ cinq minutes est une cible d'expérience, pas une promesse.** Mesurer le parcours sur une machine et un moteur identifiés, avec durée jusqu'à interface disponible puis jusqu'au premier résultat audité. Le projet fournit les Dockerfiles et les instructions de configuration ; le client prend en charge la construction, le registre et la distribution de ses images. La publication d'images préconstruites n'est pas un prérequis du produit. Distinguer cette préparation du démarrage avec images présentes et publier les étapes manuelles et erreurs rencontrées ; ne pas réduire les contrôles de sécurité pour gagner du temps.

La procédure doit fournir les prérequis et leur diagnostic, le lancement exact réellement essayé, les accès locaux, la saisie privée de la clé, une commande CLI utile, l'examen des résultats, l'arrêt et la reprise. Elle préserve les installations existantes. Une compilation Rust ou une configuration manuelle ne doit pas être présentée comme inutile si le chemin livré l'exige encore.

## Configuration par axes indépendants

La liste de cas d'usage du mainteneur est **illustrative et non exhaustive**. Le produit doit couvrir leurs combinaisons par une configuration commune, plutôt que coder un mode particulier pour chaque catégorie de client. La configuration ne dépend ni de la taille du client ni de l'étiquette « particulier » ou « entreprise ». Les capacités ci-dessous sont des exigences à implémenter et qualifier, pas un inventaire de fonctions déjà disponibles.

| Axe | Choix à rendre configurables |
|---|---|
| Exécution et exploitation | Sandboxes sur le poste de l'utilisateur, sur des hôtes administrés centralement, ou réparties entre plusieurs brokers ; gestion individuelle, déléguée ou centrale selon les permissions |
| Identité ai-guard | Authentification locale nominative sans SSO, ou fournisseur OIDC configurable : Keycloak personnel/interne, Microsoft Entra ID (Azure AD) ou autre fournisseur conforme au contrat qualifié |
| Périmètre des données | Tenant individuel ou organisation avec plusieurs utilisateurs et permissions sur les ressources ; un utilisateur peut avoir plusieurs appartenances établies par le contrôle, jamais une sélection libre de tenant |
| Client IA | Codex CLI, Claude Code ou autre agent via adaptateur qualifié, version et image choisies parmi celles approuvées pour le déploiement |
| Inférence et authentification IA | Abonnement personnel pris en charge par le client, clé API personnelle, accès API géré par l'organisation, ou serveur d'inférence local/interne compatible ; sélection distincte de l'identité ai-guard |
| Réseau et confiance | Réseau coupé, destinations Internet approuvées, intranet seul ou installation sans Internet ; DNS, URL, proxy si nécessaire et autorités de certification approuvés explicitement |
| Administration | Attribution locale des permissions ou mapping OIDC configurable : issuer, audience, clients Web/CLI, redirections, chemins de claims et valeurs de rôles/groupes ; droits et portée vérifiés côté Rust |
| Règles d'exploitation | Profils et paramètres autorisés, ressources, quotas, durées, communications, rétention et export selon la politique de l'opérateur, en conservant les invariants de sûreté |

L'absence de SSO doit laisser un parcours personnel simple, sans Keycloak ni compte cloud ai-guard obligatoire ; elle ne supprime pas l'identité ni l'audit. La connexion ai-guard ne connecte pas automatiquement l'abonnement ou l'API de l'agent. Le rôle administrateur de l'application reste distinct des permissions d'utilisation, du contrôle `runner` et de l'administration du fournisseur d'identité.

### Parcours à couvrir

| Exemple de déploiement | Identité ai-guard | Exécution et accès IA |
|---|---|---|
| Particulier avec compte ChatGPT ou Claude | Locale, sans SSO | Poste personnel ; abonnement via le client compatible et son mécanisme de connexion pris en charge |
| Particulier avec clé API | Locale, sans SSO | Poste personnel ; clé du seul fournisseur choisi |
| Particulier avec modèle local | Locale, sans SSO | Poste personnel ; inférence locale compatible, sans fournisseur cloud requis |
| Entreprise laissant chacun gérer son poste | Locale ou OIDC selon sa politique | Broker local ; abonnement ou API propre à l'utilisateur, paramètres administrés si nécessaire |
| Entreprise fournissant des sandboxes centrales | Locale ou OIDC | Hôtes administrés ; accès API d'entreprise ou abonnement individuel lorsque son fonctionnement est qualifié dans cette topologie |
| Entreprise centrale sans Internet et sans SSO | Locale nominative | Hôtes administrés ; inférence interne, images et dépendances préparées par le client |
| Particulier avec son propre Keycloak | OIDC personnel | Exécution locale ou sur son serveur ; abonnement, API ou inférence interne indépendamment du SSO |
| Entreprise sans Internet avec Keycloak interne | OIDC interne | Sandboxes et inférence internes ; confiance TLS et dépendances préparées sans service Internet obligatoire |
| Entreprise utilisant Entra ID ou un autre OIDC | OIDC configurable | Postes utilisateurs, hôtes centraux ou combinaison des deux ; fournisseur IA choisi séparément |
| Organisation avec sites ou brokers multiples | Locale ou OIDC selon le déploiement | Vue unifiée des ressources autorisées ; provenance, déconnexions et couverture partielle visibles |
| Poste partagé ou utilisateurs membres de plusieurs périmètres | Identités individuelles distinctes | Sessions, credentials, sandboxes et droits séparés ; changement de contexte validé côté contrôle |
| Automatisation non interactive | Identité de workload dédiée, bornée et révocable | Exécution autorisée et auditée sans réutiliser la session SSO d'une personne ni son identité comme preuve d'une instruction humaine |

Ces exemples ne ferment pas la liste : chaque combinaison documentée doit annoncer ses prérequis, capacités vérifiées et restrictions. Les profils d'installation simplifient les choix ; ils n'imposent pas SSO avec exécution centrale, abonnement avec usage personnel, ni inférence interne avec Keycloak. Pour un tenant par utilisateur, prévoir plusieurs sandboxes par tenant et des attributions d'administration explicites ; pour un tenant d'organisation partagé, vérifier aussi les permissions entre utilisateurs du même tenant.

### Validation et limites de la configuration

Une configuration effective typée et versionnée doit exposer ses choix résolus et leur provenance, avec secrets masqués. Documenter l'ordre de priorité des réglages ; les choix d'un utilisateur restent dans les permissions et paramètres délégués par l'administrateur. Un changement qui affecte identité, périmètre, destination, confiance, permissions ou facturation exige une nouvelle validation et, lorsque nécessaire, une nouvelle autorisation ; il ne modifie pas silencieusement une session autorisée.

Refuser explicitement les combinaisons impossibles ou non qualifiées, avec diagnostic et action de reprise : un abonnement dépendant d'un service public, un IdP public ou une API Internet ne satisfont pas un profil strictement sans Internet. Dans ce profil, authentification locale ou IdP interne, inférence interne et dépendances disponibles sur place sont nécessaires. Une panne IdP, une incompatibilité de protocole ou un credential manquant ne déclenche ni repli caché vers une autre identité, ni dépense API, ni ouverture réseau.

« Configurable » n'autorise pas à désactiver l'audit obligatoire, l'isolation des tenants, la validation TLS ou les frontières du contrôle. Aucun profil ne distribue de socket moteur, de secret du contrôle ou de jeton SSO humain aux workers. La matrice de qualification couvre les axes et leurs interactions critiques ; une combinaison non essayée reste annoncée comme telle, sans promesse de compatibilité universelle.

## SSO facultatif et fournisseur interchangeable

ai-guard intègre un fournisseur d'identité ; il ne devient pas un fournisseur SSO. Le SSO reste facultatif à l'installation. Un mode local nominatif, utilisable hors ligne, conserve attribution, permissions et audit ; désactiver le SSO ne désactive pas l'authentification.

Le connecteur utilise OpenID Connect, Authorization Code et PKCE S256 avec deux clients publics distincts pour le dashboard et le CLI. Aucun `client_secret` n'est embarqué dans le build ou distribué avec la configuration. `issuer`, identifiants clients et redirections sont des paramètres publics configurables au déploiement ; le vérificateur PKCE et les jetons restent privés. Les jetons Web restent en mémoire, hors URL, `localStorage`, `sessionStorage`, logs et exports ; la conservation privée de la transaction PKCE doit assurer la continuité au redirect et la consommation unique. La configuration et les droits sont validés côté Rust, avec refus des fournisseurs/redirections non approuvés. Un ID Token ne sert pas de bearer pour les API métier ai-guard. Une panne du fournisseur ne déclenche aucun repli automatique vers le mode local ou les anciennes clés.

L'identité humaine durable représente le couple OIDC vérifié `(iss, sub)` ou une identité locale nominative. L'adresse e-mail sert au profil, pas de clé d'attribution ; un même nom ou e-mail ne fusionne pas les identités d'issuers différents. Les appartenances aux tenants et les rôles ai-guard sont contrôlés côté serveur. L'administration du fournisseur n'accorde aucun droit applicatif implicite. Les droits humains permettent de demander les actions autorisées au broker de confiance ; aucun credential `runner` n'est distribué au navigateur ou aux workers.

Le rôle externe retenu pour le profil de découverte est **`admin-ai-guard`** ; son nom n'est pas imposé aux fournisseurs des autres installations. Le mapping de claims, groupes ou rôles externes vers les permissions internes est configurable et explicitement approuvé côté contrôle. Le serveur Rust l'applique au jeton d'accès après validation de signature, issuer, audience et validité ; un mapping absent ou ambigu ne confère aucun privilège. Pour la découverte Keycloak, prévoir un rôle client `admin-ai-guard` et le mapping explicite du claim du client API configuré. Un nom de compte, un header ou un champ envoyé par le navigateur ne confère pas ce rôle. Ses attributions sont administrées, jamais dérivées d'un attribut de profil modifiable par l'utilisateur. Sa portée administrative est limitée à l'organisation et aux tenants qui lui sont rattachés côté contrôle, sans accès global implicite aux autres clients. Ce rôle n'accorde ni administration du realm, ni credential `runner`, ni contournement des politiques ou de l'audit. Le mode local nominatif doit permettre les mêmes permissions sans IdP.

La documentation de raccordement entreprise doit préciser : fournisseur approuvé et découverte OIDC, clients Web/CLI et redirections exactes, PKCE, validation des jetons, correspondance utilisateurs/groupes/rôles/tenants, sessions et révocation, rotation des clés, diagnostic et retrait de la configuration. L'authentification ai-guard reste indépendante des abonnements et clés API Codex/Claude.

La propriété Web approuvée puis implémentée dans le candidat appartient au gateway Rust : transaction, callback et échange, avec proxy dashboard limité, paramètres privés en mémoire bornée et remise unique au navigateur. La CLI conserve sa propre transaction avec navigateur externe et listener `127.0.0.1` éphémère au chemin `/oidc/callback`. Continuité et consommation unique de `state`, `nonce` et du vérificateur, renouvellement/révocation, fin des flux devenus interdits et accès réseau approuvés restent à qualifier dans leurs parcours réels. Ces mécanismes respectent les frontières existantes ; un éventuel changement structurel reste soumis au mainteneur. Une implémentation ou une preuve logicielle ciblée ne constitue pas une qualification navigateur/IdP.

## Inférence interne et déploiement sans Internet

Complément du mainteneur,1er novembre : réutiliser pour les essais d'inférence d'entreprise le backend du petit modèle local Jev déjà essayé. Le [profil Qwen2.5-0.5B-Instruct Q4_K_M](engineering/evidence/2026-10-30/local-judge/README.md) a passé mémoire/protocole, mais échoué comme juge de sécurité. Mutualiser ce backend conserve des profils, autorisations, contexte, quotas et traces distincts ; aucune qualité de jugement ou compatibilité Codex/Claude n'est déduite de sa disponibilité. L'adaptation exacte du protocole et les parcours clients restent à implémenter et qualifier. Le [générateur local de certificats de test](demo-pki.md) fournit la chaîne sans infrastructure PKI supplémentaire ; il ne configure pas les transports des services ou des clients.

Le client construit et importe les images et dépendances nécessaires dans son environnement. ai-guard doit permettre un profil administré d'inférence interne : client et version, modèle, URL de base, format d'API compatible, référence de clé ou jeton et chaîne publique d'AC d'entreprise. La clé privée de l'AC n'est jamais nécessaire. La configuration du client et sa confiance TLS doivent être préparées dans le worker isolé, avec vérification de chaîne et de nom active, sans reprendre librement des fichiers du projet ni distribuer les secrets du contrôle.

La résolution du profil est versionnée et liée à l'autorisation : URL, modèle, format, confiance TLS et destinations réseau ne changent pas silencieusement après décision ou lors d'une reprise. Les secrets restent dans le handoff privé. Le client doit pouvoir intégrer d'autres outils dans ses images ; chaque nouvel agent exige un adaptateur et une qualification de ses capacités, sans installation Internet implicite au lancement.

« Compatible OpenAI » doit préciser le format et les capacités réellement proposés, notamment Responses ou Chat Completions ; leur interchangeabilité n'est pas présumée. Claude Code doit disposer du format compatible qu'il attend, directement ou via une adaptation qualifiée. Le réseau ne doit autoriser que les services internes approuvés ; aucun repli cloud, téléchargement implicite ou connexion d'abonnement externe n'est requis pour ce profil. URL, authentification, AC, DNS, streaming, appels d'outils et absence de dépendance Internet sont à tester avec les clients épinglés. Ce profil reste **à implémenter et qualifier** ; les presets actuels ne le livrent pas par leur seule configuration de clé API.

## Profil Keycloak de découverte

Le [profil distribué](../deploy/keycloak-demo/README.md) fournit un Compose explicitement activé, une version de Keycloak épinglée et l'import du realm initial. Les guides [anglais](setup/keycloak/README.md) et [français](setup/keycloak/README.fr.md) fixent ses fichiers, ports, commandes et contraintes de reprise. Keycloak est un service tiers de démonstration, sans ajout de logique métier ai-guard dans un autre langage. Le connecteur applicatif candidat existe ; le bootstrap complet des memberships et du raccordement réseau reste à livrer, et le parcours d'acceptation ci-dessous reste NOT_RUN. Les comptes et clients du realm ne constituent pas à eux seuls des identités autorisées par le contrôle ai-guard.

| Compte préconfiguré | Identifiants publics de démonstration | Usage |
|---|---|---|
| Administrateur Keycloak | `admin / admin` | Administrer Keycloak et le realm de démonstration |
| Administrateur ai-guard | `admin-ai-guard / admin-ai-guard` | Rôle applicatif `admin-ai-guard` présent dans les claims ; administrer le périmètre applicatif de découverte sans administrer le realm |
| Utilisateur normal 1 | `user-1 / user-1` | Se connecter au dashboard et au CLI sans rôle `admin-ai-guard` |
| Utilisateur normal 2 | `user-2 / user-2` | Se connecter au dashboard et au CLI sans rôle `admin-ai-guard` |

Le profil livré devra rendre clients publics, PKCE S256, redirections et permissions prêts à l'emploi, sans édition de realm ni intervention dans la console d'administration pour le premier essai. Les accès hôte de cette démo doivent être limités au loopback. Les identifiants connus ne sont jamais appliqués aux installations normales ni aux credentials existants. Aucun jeton SSO humain, credential d'administration Keycloak ou secret du contrôle ne rejoint un worker.

Le parcours d'acceptation démarre la pile depuis un état neuf, ouvre le dashboard, connecte `user-1`, connecte le CLI au même compte, lance une tâche réelle non destructive, puis retrouve demande, exécution et résultat dans l'audit. `user-2` permet de vérifier les refus d'accès aux ressources privées du premier utilisateur ; leurs périmètres sont établis côté contrôle. Le compte `admin-ai-guard` doit accéder à la vue d'administration de ce périmètre, tandis que les deux utilisateurs normaux en sont refusés côté serveur. Tester aussi le rôle absent, falsifié, retiré ou émis pour un autre client/issuer. Le compte `admin` du realm n'obtient aucun droit applicatif implicite. Les quatre comptes et le realm sont explicitement nommés « démonstration » ; aucune activité d'agent fictive n'est injectée dans le produit.

La recette réelle devra démontrer l'interopérabilité et faciliter l'évaluation du produit. Elle ne qualifie pas l'exploitation de Keycloak en production, qui appartient à l'opérateur ; ai-guard doit fournir une procédure de raccordement sans imposer ce fournisseur aux clients.

## Raccorder le SSO de l'entreprise

**Raccordement applicatif candidat implémenté ; qualification entreprise NOT_RUN.** Les guides SSO [anglais](setup/sso/README.md) et [français](setup/sso/README.fr.md) décrivent les fichiers et commandes acceptés, avec les limites d'exploitation. Ce parcours utilise le fournisseur approuvé de l'opérateur, sans comptes publics de démonstration ni import implicite d'un realm existant. La conformité au profil d'access token réellement accepté doit être vérifiée ; une conformité OIDC générale ou le nom Entra ID ne la démontre pas. Il reste distinct du profil de découverte :

1. Renseigner la [fiche opérateur](engineering/SSO_OIDC.md#fiche-opérateur-keycloak-à-renseigner) : issuer exact, clients publics Web/CLI, redirections et origines exactes, audience/profil d'access token, clés de confiance, durées et politique de révocation. Cette fiche d'inventaire reste distincte du [JSON fermé réellement accepté](setup/sso/README.fr.md#préparer-le-json-privé-du-gateway). Pour une AC privée du gateway HTTPS, fournir `--gateway-ca-file /chemin/absolu/vers/ac-publique.pem` au CLI local ou à `human sso` : ces racines explicites remplacent les racines intégrées pour ce seul trajet et restent figées pendant la session. Le bundle d'AC du fournisseur configure séparément l'IdP. Voir la [procédure TLS du CLI](setup/sso/README.fr.md) et ses règles de rotation/reprise.
2. Préparer les appartenances et permissions côté contrôle Rust, puis vérifier la connexion Web/CLI et les refus avec deux tenants. Un rôle d'administration du fournisseur n'accorde aucun privilège ai-guard implicite.
3. Vérifier les accès de chaque acteur : un issuer loopback visible dans le navigateur n'est pas automatiquement joignable depuis le gateway/dashboard conteneurisé sur le réseau interne sans route par défaut. Conserver le même issuer et qualifier les destinations nécessaires, sans réécriture implicite, désactivation de TLS ni accès Internet général. Traiter les origines et le callback sans assouplissement global des guards HTTP.
4. Documenter les commandes réellement livrées, les erreurs et actions de reprise, puis qualifier expiration, déconnexion, rotation et panne ; conserver les preuves sans secret. Aucun endpoint, credential ou accès d'entreprise n'est supposé disponible.

## Chaîne de traçabilité à livrer

La chaîne attendue est : **utilisateur initiateur → demande → session → agent → délégation → sous-agent → action/commande → décision → acceptation broker → observation runtime → résultat et modifications**.

- Le contrôle établit les identités et les liens parent/enfant. Chaque agent/sous-agent vérifié possède une identité d'exécution bornée, expirante et révocable, avec des permissions au plus égales à celles déléguées. Un nom dans un log ne suffit pas.
- Chaque événement distingue initiateur, acteur effectif, origine et parents causaux. Conserver les demandes/réponses visibles et les apports externes accessibles à l'intégration ; une action issue d'un contenu externe ne devient pas une instruction humaine explicite. Le raisonnement interne non exposé par un fournisseur ne peut pas être promis comme observable.
- Les commandes conservent arguments et contexte structurés. Demande d'outil, décision et version du contrôle, approbation, acceptation, exécution observée, sorties et changements de fichiers sont des faits distincts, liés sans réécrire l'historique.
- L'audit est durable et indépendant des workers, avec chaînage, points de contrôle signés et export vérifiable. Définir la protection des clés, la portée de la vérification et les limites face à un administrateur du stockage ; une signature ne prouve pas un effet runtime absent des observations.
- Les contenus sensibles ont des accès restreints, une protection adaptée et une rétention configurable. Masquer les secrets avant persistance/diffusion ; borner les volumes. Afficher explicitement masquage, troncature, données perdues et couverture partielle. Une chaîne incomplète n'est jamais certifiée complète.
- La consultation permet de remonter d'une commande à la demande initiale et de parcourir les délégations, décisions et résultats, selon les droits du tenant. Un audit obligatoire indisponible bloque l'action ; une déconnexion ne fabrique pas un résultat vide ou réussi.

Prévoir migrations et compatibilité avec les événements et credentials existants. Une ancienne clé partagée attribue au mieux une activité à son périmètre authentifié ; ne pas inventer une personne pour les événements historiques.

## Qualification et documentation

Le [contrat IHM par permissions](ui.md#vues-utilisateur-et-administration-unifiée--cible-à-livrer) fait partie du parcours : vue utilisateur adaptée, administration unifiée à 360° du périmètre autorisé, configuration effective visible selon les droits et invalidation des données après changement de rôle ou de contexte. L'autorisation et l'agrégation restent côté Rust ; une modification visuelle seule ne livre pas ce contrat.

Le [cahier de tests](engineering/TEST_STRATEGY.md#identité-individuelle-sso-et-découverte--à-qualifier) couvre le mode sans SSO, une connexion réelle Keycloak Web/CLI, les refus, pannes et révocations, la séparation des tenants, la chaîne de délégation et l'essai chronométré. Un second fournisseur OIDC peut compléter l'interopérabilité sans remplacer la preuve Keycloak. Aucun faux fournisseur permanent ni scénario d'affichage ne qualifie ces parcours.

Mettre à jour ensemble quick start, exemple Compose/realm, configuration, aide CLI, exploitation, dépannage et preuves. Fournir les deux entrées « essayer avec la démo » et « raccorder le SSO de l'entreprise ». Le support SSO doit être qualifié dans le produit même si un déploiement peut le laisser désactivé. Les gates globaux restent ouverts tant que leurs preuves manquent.

## Références officielles

- [OpenID Connect Core](https://openid.net/specs/openid-connect-core-1_0.html) : identité, claims et validation.
- [OAuth pour applications natives, RFC 8252](https://www.rfc-editor.org/rfc/rfc8252.html) : navigateur externe et PKCE pour le CLI.
- [Sécurité OAuth, RFC 9700](https://www.rfc-editor.org/rfc/rfc9700.html#section-2.1.1) : Authorization Code et PKCE S256.
- [Clients publics Keycloak](https://www.keycloak.org/securing-apps/javascript-adapter) : configuration sans secret client et redirections précises ; cette référence n'impose pas le SDK JavaScript au produit.
- [Keycloak avec Docker](https://www.keycloak.org/getting-started/getting-started-docker) et [conteneurs](https://www.keycloak.org/server/containers) : environnement de découverte et configuration conteneurisée.
