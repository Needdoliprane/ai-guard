# SSO OIDC : contrat et qualification

État au 31 octobre 2026. Le [cahier produit canonique — identité individuelle, SSO et essai rapide](../sso.md) fixe les exigences du mainteneur. Ce document en est la déclinaison technique : il précise les mécanismes, refus et preuves sans réduire la traçabilité individuelle, le mode local nominatif, les points de contrôle signés ou la découverte demandés. En cas d'écart, le cahier produit et AGENTS.md font autorité.

Le SSO est facultatif à l'installation, mais son support et sa qualification font partie du produit demandé. ai-guard est un client OpenID Connect d'un fournisseur configurable, dont Keycloak ; il ne devient pas fournisseur d'identité. Cette déclinaison conserve les frontières du [contrat d'architecture](ARCHITECTURE_CONTRACT.md) et de l'[isolation par tenant](CLOUD_TENANCY.md).

**OIDC, PKCE et l'intégration Keycloak sont ABSENTS du code actuel.** Aucun endpoint de connexion, option CLI SSO, schéma de configuration OIDC ou profil Compose Keycloak exécutable n'est livré par ce document. Une spécification ne clôture aucun gate de qualification.

## État constaté

| Périmètre | Fonctionnement actuel | Écart obligatoire |
|---|---|---|
| API Rust | Index de clés statiques `runner`/`viewer`, associé à un tenant ; extraction avant les entrées protégées dans [auth.rs](../../src/api/auth.rs) et autorisation dans [api.rs](../../src/api.rs). | Validation OIDC, identité humaine nominative, permissions et révocation à implémenter. |
| Configuration | Registre fermé de tenants, bases SQLite et fichiers de clés dans [gateway_config.rs](../../src/gateway_config.rs). | Configuration OIDC versionnée et migration explicite à concevoir ; ne pas ajouter des champs supposés utilisables. |
| Dashboard | Proxy Rust avec clé viewer fixe ou validation du viewer présenté ; `/api/v1/identity` renvoie seulement `tenant_id` et `role`. | Connexion humaine OIDC et autorisation correspondante à implémenter. |
| Navigateur | Saisie « Jeton de lecture » dans [main.ts](../../front/src/main.ts) ; bearer gardé en mémoire, requêtes et SSE par `fetch`, effacement à la fermeture dans [api.ts](../../front/src/api.ts). | Flux PKCE, renouvellement, logout et fin de session SSO à implémenter. Ce stockage mémoire existant ne prouve pas ces parcours. |
| Broker CLI | `VerifiedProfile` confirme une identité `runner` auprès du contrôle dans [service/mod.rs](../../src/broker/service/mod.rs), via [AuditClient](../../src/runner.rs). | Un token humain PKCE n'est pas déjà accepté par `broker submit`. Délégation et permissions humaines explicites à implémenter. |
| Local sans IdP | Des clés statiques permettent le fonctionnement hors ligne. | L'authentification locale **nominative** et son audit individuel restent à implémenter et qualifier. |
| Découverte Keycloak | Aucun profil Keycloak dans les configurations inspectées. | Profil local explicitement activé, configuration et recette réelle à livrer. |

Les anciennes clés partagées ne prouvent pas l'identité d'une personne. Leur historique conserve sa provenance ; aucune migration ne doit réattribuer rétroactivement leurs actions à un utilisateur nommé. L'OAuth et les abonnements de Codex/Claude sont distincts du SSO ai-guard : une connexion à l'un ne connecte pas à l'autre.

## Connexion Web et CLI

Le contrat cible utilise Authorization Code avec **PKCE S256**, avec deux clients publics distincts pour le Web et la CLI. Aucun `client_secret` n'est distribué dans le frontend, le binaire CLI ou leurs configurations. Les flux implicite et mot de passe sont désactivés ; un échec ne change ni flux ni identité. PKCE lie l'échange du code au vérificateur privé de la transaction. Références : [RFC 7636](https://www.rfc-editor.org/rfc/rfc7636.html) et [OAuth Security BCP, sections 2.1 et 2.4](https://www.rfc-editor.org/rfc/rfc9700.html).

Chaque connexion possède `state`, `nonce` et vérificateur aléatoires, privés, bornés et expirables. Ils sont liés au client, à l'issuer et au redirect attendus ; leur consommation est unique, y compris après erreur ou deux callbacks concurrents. Refuser callback non sollicité, doublon, transaction expirée, mauvais `state`, mauvais `nonce`, code ou vérificateur rejoué. Valider aussi l'ID token utilisé pour établir la connexion selon [OIDC Core, section 3.1.3.7](https://openid.net/specs/openid-connect-core-1_0.html#IDTokenValidation). Un ID token ne sert jamais de bearer pour l'API métier.

Les redirects et origines autorisés sont exacts, sans joker ou redirection ouverte. Le lot Web doit figer le propriétaire de la transaction, du callback et de l'échange, ainsi que leur continuité lors du retour du navigateur. Il doit prouver la conservation privée puis la consommation des paramètres : une variable mémoire perdue lors d'un rechargement n'est pas une solution. Ce document ne décide pas automatiquement d'un BFF ou d'une nouvelle session par cookie ; un changement de frontière ou de stockage exige une proposition motivée avant réalisation.

Pour la CLI, la cible est un navigateur externe et un callback loopback éphémère lié uniquement à une adresse IP loopback, avec chemin fixé et fermeture du listener après succès, erreur ou délai. L'exception de port variable des redirects loopback doit être testée avec le fournisseur ; elle n'autorise aucun joker de chemin ou d'hôte. Voir [RFC 8252, sections 7.3 et 8.3](https://www.rfc-editor.org/rfc/rfc8252.html). Le mode sans navigateur est une capacité distincte, **ABSENTE / NOT_RUN** : ne pas proposer un flux mot de passe, une copie de token ou un flux device supposé disponible comme repli implicite.

L'identité humaine CLI demande une action au contrôle avec ses permissions explicites et son attribution d'audit. Le broker garde son identité de workload indépendante. Aucun échange silencieux du token humain contre une clé `runner`, aucune promotion automatique humain → `runner`, et aucun credential humain dans un worker ne sont autorisés.

## Validation et autorisation côté Rust

Avant tout accès protégé, l'API doit résoudre une identité vérifiée et son périmètre. Le frontend, une validation locale de la CLI et le proxy dashboard ne remplacent pas ce contrôle serveur.

| Contrôle cible | Refus attendu |
|---|---|
| Signature et clés | Algorithmes explicitement autorisés, clé compatible, signature valide ; refuser `none`, confusion de type de clé et algorithme non configuré. |
| Émetteur et destination | `iss` exact, audience de l'API présente, client émetteur autorisé selon le profil ; refuser token d'un autre realm, client ou service. |
| Temps et type | `exp` requis, `nbf` contrôlé s'il est présent, tolérance d'horloge bornée et publiée ; distinguer access token et ID token, refuser token expiré ou prématuré. |
| Profil de token | Figer champs et valeurs attendus, dont `typ`, `azp` ou `client_id` selon le contrat retenu ; refuser profil ambigu ou token d'une autre classe. |
| Autorisation | Résoudre memberships et permissions administrés côté serveur, puis appliquer le contrôle à chaque API, export, commande et flux SSE. |

Le [profil JWT access token RFC 9068](https://www.rfc-editor.org/rfc/rfc9068.html#section-4) impose notamment un type d'access token spécifique ; ne pas supposer qu'un token Keycloak par défaut suit ce profil. La fiche opérateur doit arrêter le profil réellement accepté et ses règles `azp`/audience. Les règles d'ID token OIDC ne sont pas transposées aveuglément à tous les access tokens.

Discovery et JWKS proviennent uniquement de la configuration de confiance de l'opérateur. Vérifier la cohérence exacte de l'issuer, TLS et les destinations autorisées ; borner taille, délais, redirections, concurrence et caches. Aucun `jku`, `x5u`, issuer ou autre URI fourni par un JWT utilisateur ne déclenche de requête réseau. Un `kid` inconnu ne permet qu'un rafraîchissement borné des clés de l'issuer déjà configuré, puis un refus si la clé reste inconnue. Le cache ne doit pas devenir un moyen de prolonger indéfiniment une clé. Voir [OIDC Discovery](https://openid.net/specs/openid-connect-discovery-1_0.html#ProviderConfig).

Le gateway et le dashboard actuels sont sur un réseau Docker interne sans route par défaut. Ils ne peuvent donc pas être supposés joindre un issuer externe. Le raccord exige soit un accès TLS strictement borné aux destinations IdP/discovery/JWKS nécessaires, soit une distribution contrôlée des clés publiques avec rotation qualifiée. Tout changement de frontière réseau doit être proposé avant réalisation ; aucun accès Internet général, proxy libre ou affaiblissement TLS n'est autorisé pour obtenir une connexion. Cette dépendance reste ouverte avant la qualification réelle.

L'identité humaine stable est le couple `(issuer, sub)` ; email et nom affiché restent des attributs modifiables. Le tenant et les permissions viennent d'associations administrées par le contrôle. Un attribut de profil éditable par l'utilisateur, un header, un paramètre ou un label worker ne choisit jamais le tenant. Une sélection de contexte ne peut choisir que parmi les memberships déjà vérifiés. Les groupes ou rôles signés de l'IdP ne valent que selon un mapping serveur explicite ; être administrateur Keycloak n'accorde aucun rôle ai-guard.

Séparer utilisateur initiateur, acteur effectif et identité du broker/agent/sous-agent. Toute délégation lie parent, mission, tenant, permissions, expiration et révocation sans élévation implicite. Les workers n'obtiennent ni token SSO humain ni clé `runner` ; leur identité reste limitée au relais autorisé. Une action d'agent n'est pas automatiquement une instruction explicite de l'utilisateur. Une filiation seulement déclarée reste `declared`.

L'audit indépendant conserve identités, causalité, décision et effet observé avec leur provenance, sans bearer, code, vérificateur ou secret. Les contenus sont masqués avant persistance/diffusion ; accès, rétention, tailles et pertes sont explicites. L'indisponibilité de l'audit obligatoire refuse l'action, y compris une action déclenchée après connexion SSO.

Les points de contrôle signés et l'export vérifiable suivent le cahier produit ; leur mécanisme, la protection des clés et les limites de vérification restent à concevoir, implémenter et qualifier. Le chaînage historique des événements ne prouve pas à lui seul cette nouvelle exigence ni une observation runtime manquante.

## Durée de session, panne et déconnexion

Les tokens Web restent en mémoire, jamais dans URL, `localStorage`, `sessionStorage`, logs ou exports. L'état de transaction PKCE est privé, éphémère, borné puis effacé ; son mode de conservation dépend du propriétaire de la transaction à figer et doit assurer la continuité au redirect. Le code d'autorisation et `state` du callback sont transitoires : les consommer puis retirer les paramètres de l'historique et empêcher leur fuite dans logs, télémétrie ou referrers. Le vérificateur reste privé. Le mode de conservation CLI ne doit pas introduire de cache de tokens durable implicite. Le guide [Keycloak JavaScript adapter](https://www.keycloak.org/securing-apps/javascript-adapter) décrit le stockage mémoire des tokens et les contraintes des navigateurs ; aucune bibliothèque ou version n'est sélectionnée ici.

Le renouvellement est borné, coordonné entre requêtes et soumis à la session encore autorisée ; une réponse tardive ne reconnecte pas une session déconnectée. La politique des refresh tokens publics doit traiter rotation ou liaison cryptographique au client, ainsi que la détection de réutilisation, avec tests de concurrence. Expiration, refus de refresh et logout effacent l'état privé, annulent les requêtes et ferment les SSE. Le serveur vérifie la validité pendant les flux et les termine à expiration ou perte de permission selon une borne mesurée et publiée ; reconnecter exige une nouvelle vérification, avec curseur du même tenant autorisé. Une coupure produit un état déconnecté/périmé, jamais un faux zéro activité.

La déconnexion locale doit fonctionner même si l'IdP est indisponible ; distinguer son succès d'une fermeture confirmée de la session IdP. Configurer le logout standard et ses redirects exacts, sans annoncer qu'un appel a révoqué toutes les copies d'un access token. Les possibilités discovery, clés, logout et révocation sont décrites par [Keycloak OIDC layers](https://www.keycloak.org/securing-apps/oidc-layers). Son introspection exige un client confidentiel ; elle ne justifie jamais de distribuer un secret au Web ou à la CLI publics.

Une validation JWT hors ligne ne garantit pas une révocation immédiate. Définir durées des tokens, validité du cache, rotation, révocation des mappings locaux et délai maximal de prise en compte. En panne IdP, un token encore valide et une clé de confiance encore valide ne peuvent être acceptés que selon cette politique explicite ; clé inconnue, cache périmé ou renouvellement impossible n'ouvrent aucun accès. Aucune panne SSO n'active automatiquement les anciennes clés ou un mode local. Le mode local nominatif hors ligne demeure un choix d'installation distinct, avec les mêmes contrats d'autorisation et d'audit.

## Fiche opérateur Keycloak à renseigner

Cette fiche est un inventaire de décisions, **pas un format de configuration actuellement accepté**. Toutes les valeurs restent à renseigner pour le déploiement ; ne pas importer de credentials ou de realm existants implicitement.

| Élément | Valeur et preuve attendues |
|---|---|
| Serveur | Version et image/digest testés, realm, issuer exact, discovery, certificats/autorités de confiance ; issuer joignable avec la même identité depuis navigateur, CLI et contrôle conteneurisé. |
| Clients | IDs Web et CLI distincts ; clients publics, `Client authentication` désactivé, Standard flow activé, PKCE S256 imposé, implicit et Direct Access Grants désactivés. Aucun secret distribué. |
| API | Audience exacte, mappers d'audience, profil d'access token, algorithmes et champs de client autorisés ; token de compte/administration ou ID token refusé. |
| Retours | Redirects de connexion/logout, origines Web, callback CLI et politique de port loopback ; aucune origine universelle. |
| Permissions | Scope `openid` et scopes minimaux, mappers et attributs non modifiables par l'utilisateur ; mapping serveur `(issuer, sub)`/groupes → memberships/permissions, procédure d'approbation et de retrait. |
| Durées | Transactions PKCE, access/refresh tokens, session inactive/maximale, rotation/réutilisation, tolérance d'horloge, borne de fin SSE et délai de révocation. |
| Clés et réseau | Rotation avec chevauchement mesuré, durée/cache JWKS, comportement `kid` inconnu, limites HTTP/TLS et procédure de panne ; aucune désactivation de TLS. |
| Déconnexion | Redirects et mécanisme de logout réellement supportés, révocation et éventuelles notifications vérifiées ; distinguer déconnexion locale et fermeture IdP. |
| Exploitation | Comptes de test A/B, responsables de configuration à désigner, sauvegarde/restauration et rotation, preuves sans secrets, procédure entreprise distincte de la découverte. |

Les réglages publics, redirects et origines sont décrits dans le [guide officiel Keycloak](https://www.keycloak.org/securing-apps/javascript-adapter#_keycloak_server_configuration). La version et les écrans réellement testés doivent accompagner la fiche ; aucune recommandation de version ou compatibilité n'est déduite de ce document.

## Deux procédures à livrer

**Découverte locale — ABSENTE.** Livrer un profil Compose explicitement activé, limité au loopback, avec Keycloak conteneurisé épinglé, realm de démonstration identifié et clients publics PKCE préconfigurés. Les comptes publics demandés sont `admin / admin` pour administrer ce Keycloak et `user / user` pour essayer ai-guard en Web et CLI. Ils restent propres à cette découverte, sans rôle ai-guard implicite pour l'administrateur Keycloak, et ne deviennent jamais les valeurs par défaut d'une installation normale. Ne remplacer aucun credential, base ou volume existant. Ne pas corriger un issuer incohérent par réécriture arbitraire ou désactivation de TLS ; les exceptions HTTP restent limitées au profil local explicitement isolé et au callback natif loopback prévu par le contrat.

La procédure devra diagnostiquer les prérequis, afficher les accès et leur disponibilité, connecter réellement `user`, puis exécuter une action autorisée non destructive avec sa trace individuelle et son résultat réel. L'accès du broker doit passer par la délégation explicite décrite plus haut. Mesurer le parcours, les interventions manuelles et séparément téléchargement/construction à froid et images présentes ; environ cinq minutes est une cible ergonomique, pas une durée garantie. Cette recette ne qualifie pas l'exploitation de Keycloak en production.

**Fournisseur d'entreprise — ABSENTE.** Faire renseigner et valider la fiche opérateur ; préparer les memberships côté contrôle ; tester discovery, audience et mappings avec deux tenants ; connecter Web et CLI ; vérifier refus, déconnexion, rotation et panne ; documenter exploitation et retrait d'accès. Aucun endpoint, compte, certificat ou accès d'entreprise n'est supposé disponible. Le support d'autres fournisseurs repose sur le même contrat et leurs preuves, sans changement silencieux de flux.

## Intégration et critères de qualification

L'intégration doit étendre l'authentification Rust existante avant les entrées protégées, puis les contrôles gateway/dashboard, la connexion Web et le parcours CLI humain. Conserver le frontend TypeScript statique servi par Rust conteneurisé, le broker de confiance et une base SQLite distincte par tenant. Aucun service métier supplémentaire, BFF, changement de stockage ou haute disponibilité n'est décidé par ce contrat. Versionner la configuration et les identités d'audit ; définir compatibilité des anciennes clés, activation explicite et retour arrière sans confondre comptes humains et workloads.

| Preuve réelle requise | Cas positifs et négatifs à conserver | Statut au 31 octobre 2026 |
|---|---|---|
| Web et CLI navigateur | Code+S256 avec Keycloak réel, deux clients distincts ; refus connexion annulée/expirée, callbacks croisés et second onglet. | NOT_RUN ; implémentation absente |
| Transaction | Bon `state`/`nonce`/verifier ; mauvais, absent, expiré ou rejoué, double callback et réutilisation du code refusés. | NOT_RUN |
| Validation API | Access token du profil accepté ; mauvaise signature, algorithme, issuer, audience, type, `azp`/client, temps ou ID token refusés. | NOT_RUN |
| Isolement A/B | A ne lit/commande/exporte que A ; B reste inaccessible par header, corps, URL, curseur SSE ou attribut éditable ; retrait de membership appliqué. | NOT_RUN |
| Permissions et acteurs | Viewer sans écriture ; action humaine explicitement permise et attribuée ; admin Keycloak sans privilège ai-guard implicite ; worker hors API de contrôle ; pas de conversion humaine en runner. | NOT_RUN |
| Clés et entrées réseau | Rotation réelle, ancienne/nouvelle clé selon politique ; `kid` inconnu, JWKS périmé, TLS invalide et panne bornés ; aucune requête vers `jku`/URI utilisateur. | NOT_RUN |
| Session | Refresh, concurrence/réutilisation, expiration, logout local/IdP, réponse tardive, perte de permission et fermeture SSE selon bornes publiées ; panne sans fallback. | NOT_RUN |
| Confidentialité et audit | Tokens Web en mémoire, jamais en Web Storage ; état PKCE privé éphémère conservé puis effacé selon son propriétaire ; code/state de callback transitoires nettoyés ; aucun token/code/verifier en logs, exports ou worker ; audit nominatif/causal, masquage avant persistance et refus si audit obligatoire indisponible. | NOT_RUN |
| Local et migration | Connexion locale nominative hors ligne, sélection explicite ; anciennes clés restent attribuées comme telles, pas de réattribution ni changement de tenant. | NOT_RUN ; identité locale nominative absente |
| Découverte et entreprise | Installation neuve, absence de remplacement des données, comptes publics confinés, parcours réel chronométré ; fiche entreprise et procédures distinctes. | NOT_RUN ; profil absent |
| CLI sans navigateur | Capacité et protocole explicites, menace et refus propres au mode headless ; aucune déduction depuis le test navigateur. | NOT_RUN ; capacité absente |

Les fixtures peuvent vérifier parsing et refus, mais ne qualifient ni l'interopérabilité IdP, ni le navigateur, ni la CLI réelle. Consigner versions, hôtes, procédures, résultats `PASS`/`FAIL`/`BLOCKED`/`NOT_RUN` et limites sans secrets. Tant que ces parcours et leurs preuves manquent, le SSO, l'identité individuelle complète et la découverte Keycloak restent des écarts ouverts du produit.
