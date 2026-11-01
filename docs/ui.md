# Interface de supervision

Ce document est la référence pour l'identité visuelle et les vues du dashboard. Le [périmètre livré](#périmètre-livré) reste une consultation `viewer` ; les [vues utilisateur et administration](#vues-utilisateur-et-administration-unifiée--cible-à-livrer) sont une cible obligatoire à implémenter et qualifier. Le [contrat d'identité et de configuration](sso.md) fixe les permissions et profils attendus. Les preuves visuelles datées ne qualifient pas les fonctions futures.

## Direction de conception

Le guide détaillé du mainteneur reçu le 30 octobre 2026 a guidé la refonte livrée en QA. Le front antérieur, avec barre latérale claire et accents turquoise, a été remplacé par le header clair et la navigation sombre fixes. La référence est transposée à **ai-guard**, à ses neuf vues et à ses données de supervision ; elle ne transforme pas le dashboard en page de vente ou en parcours fictif de dix chapitres. Les [preuves et captures réelles](engineering/evidence/2026-10-30/editorial-ui/README.md) identifient le rendu déployé.

### Contrat visuel et plan de réalisation

- Palette claire : fond `#FAFAF8`, texte `#101010`, navigation `#161714`, identité/action sombre `#111111`, citron `#E2F471`, survol citron `#EFFFA1`, texte sur citron `#171B07`, panneaux `#F0F0ED`, traits `#D4D4CF`, texte secondaire `#666662`, seconde lecture des titres `#8A8A84`. Focus général `#5555FF`, focus clair sur navigation sombre. Les verdicts gardent un sens textuel et leurs teintes discrètes ; ils ne deviennent pas des couleurs de marque.
- Arial/Helvetica/sans-serif pour texte et titres ; Courier New/monospace pour numéros, repères, dates et métadonnées. Corps16px/interligne1,55 ; grands titres `clamp(36px,4.2vw,62px)`, graisse500, interligne1 et espacement serré ; sections25px. Les logs et tableaux restent lisibles avec une densité adaptée. Aucune police distante.
- Conteneur1800px maximum, gouttière `clamp(20px,3.2vw,56px)`, traits1px, panneaux4–5px. Utiliser des lignes et de l'espace ; éviter une carte autour de chaque paragraphe. À la demande explicite du mainteneur et d'après ses deux visuels fournis, signature OVPT typographique forte, séparateur sobre et nom du produit ai-guard. Conserver les repères d'identité et de lecture seule ; aucune décoration orbitale.
- Remplacer la barre latérale générique par un header et une navigation sombre fixes en haut. Conserver les neuf destinations réelles, numérotées, le client authentifié et l'état du flux. Afficher la position de vue comme navigation, jamais comme progression de validation. Prévoir précédent/suivant avec destination annoncée et position stable. Actualisation, thème, export disponible et déconnexion gardent des emplacements identifiables ; aucun bouton sans effet.
- Adapter la hauteur réservée sous les commandes, compacter en paysage/écran court et garder les raccourcis numérotés accessibles sur mobile. Tests320px,390px,desktop et paysage : pas de débordement global, aucune donnée cachée sous les commandes. Le citron reste réservé à l'action et à la sélection. Les thèmes clair/sombre, le focus, le clavier, le mouvement réduit et les états réels restent obligatoires.
- Ajouter de la matière utile près des vues : ce qui est observé, déclaré, inconnu, périmé ou non facturable, et l'action de reprise disponible. Conserver les motifs serveur comme données. Les badges, tableaux, filtres, paginations, détails et erreurs restent fonctionnels ; aucun nouveau chiffre ni événement d'exemple dans le produit.

Réalisation initiale : tokens/CSS, marque typographique, login, shell et navigation dans `front/src/{theme.css,style.css,main.ts}`, sans modification API/identité/SSE/stockage du jeton. Les tests navigateur vérifient géométrie stable, thèmes,320/390px/paysage, clavier et états. `control-view.ts` est conservé et ses vues utilisent la nouvelle feuille de style. Au gel éditorial initial, les 27 tests sous Node24, le build, le parcours navigateur et la revue indépendante passent ; le build statique est servi par le dashboard Rust conteneurisé et le parcours réel passe10 contrôles. Le français existant est conservé ; aucun sélecteur de langue inactif n'est ajouté sous prétexte de reprendre l'exemple du guide. Les mesures de contraste et les tests clavier ne constituent pas un audit WCAG complet.

### Complément OVPT et thème sombre — demande du 30 octobre 2026

Les références du mainteneur sont le logo OVPT blanc sur noir et la bannière OVPT noire sur fond clair, avec séparateur et devise « Omnia Vincit Progressus Technologicus ». Leur adaptation est autorisée ; ai-guard conserve son nom de produit et ses contrats existants. Une signature typographique locale conserve la netteté aux tailles du header. Le favicon doit rester identifiable à16 et32px sur fond de navigateur clair et sombre ; une déclinaison compacte peut remplacer les quatre lettres à cette taille. Les images originales ne sont pas altérées. L'identité fournie ne désigne pas à elle seule un propriétaire légal ou un canal de support.

Le thème sombre précédent, dominé par des fonds olive, a été remplacé par des noirs neutres en couches distinctes, du texte ivoire, des métadonnées lisibles et des traits fins visibles. Le citron reste réservé à l'action ou à la sélection ; Suivant désactivé prend un fond gris neutre. Les mêmes surfaces et états que dans le clair ont été vérifiés, y compris authentification, tableaux denses, détails, focus et erreurs. Les commandes restent stables et le bouton de connexion visible à320×568 sans défilement initial. Le [lot OVPT livré](engineering/evidence/2026-10-30/ovpt-ui/README.md) rassemble les captures desktop/mobile/paysage, le contrôle du favicon16/32, les tests fonctionnels, la seconde revue et le parcours sur le dashboard Rust réel. La qualification globale du produit reste distincte.

Le mainteneur retient [Nothin’](https://www.noth.in/?ref=siteinspire) comme référence visuelle, à suivre autant que possible. Sa page d'accueil a été consultée le 29 octobre 2026 : typographie affirmée, hiérarchie de tailles marquée, base noir/blanc, grandes respirations, compositions franches et accents ponctuels. Ces principes guident les prochains chantiers visuels de l'IHM de contrôle.

Pour ai-guard, les titres structurent les vues, les actions essentielles se repèrent immédiatement et les listes, tableaux, logs et détails conservent une densité utile. Les couleurs sémantiques restent explicites pour les décisions, erreurs, inconnues et provenances. L'identité du client authentifié et les états de lecture demeurent faciles à distinguer.

La revue d'un changement visuel compare les vues principales à cette référence et vérifie desktop/mobile, thèmes clair/sombre, navigation clavier, focus, contrastes, mouvement réduit et activité dense. Elle inclut les états chargement, vide, partiel, périmé, déconnecté, interdit et erreur. Les captures complètent les parcours fonctionnels avec données réelles ; les preuves de l'interface livrée restent dans [VALIDATION.md](engineering/VALIDATION.md).

## Navigation entre ressources du tenant

Les détails agent/sandbox chargent réellement le registre workers et permettent d'en parcourir les pages ; un détail worker présente les agents et sandboxes correspondants. Les relations reposent sur les identifiants exacts de session ou de runtime reçus, avec provenance explicite ou inconnue. Elles ne prouvent ni délégation ni exécution et n'attribuent aucune identité de broker. Chaque source indique lecture, fraîcheur, périmètre et erreur éventuelle ; l'absence de correspondance dans une page ne signifie pas absence du tenant. La reprise d'une source retire son erreur résolue sans masquer une erreur indépendante. Le focus reste dans le dialogue lors des changements de liens et des reprises.

Cette navigation utilise les API de lecture existantes du tenant `viewer`, sans nouvel endpoint ni accès moteur. Les vues humaines/administratives, l'identité des brokers et la fédération restent les exigences distinctes ci-dessous. [Validation ciblée et limites](engineering/VALIDATION.md#1er-novembre--navigation-liée-du-dashboard-et-reprises).

## Vues utilisateur et administration unifiée — cible à livrer

Complément du mainteneur, 31 octobre 2026 : l'IHM doit refléter les [axes de configuration](sso.md#configuration-par-axes-indépendants), l'identité, les permissions effectives et le périmètre autorisé. **À implémenter et qualifier.** Le front actuel ne prend en charge que l'identité `viewer` d'un tenant dans [ses types](../front/src/types.ts) et [son client API](../front/src/api.ts). Les preuves des neuf vues existantes ne qualifient pas les parcours administrateur ci-dessous.

Conserver une seule application frontend et une identité visuelle commune, avec des vues utilisateur et administrateur adaptées. La navigation, les informations et les commandes découlent d'un contrat typé d'identité, de périmètres et de capacités établi par le contrôle Rust ; le navigateur ne déduit pas des droits d'un nom de compte ou d'un claim non vérifié. Le nom externe de rôle reste configurable. Un même utilisateur peut disposer de permissions différentes selon son périmètre ; distinguer le contexte personnel et l'administration lorsque les deux sont autorisés.

| Vue | Informations et parcours attendus |
|---|---|
| Utilisateur | Ses sandboxes et les ressources explicitement partagées avec lui, agents/sous-agents, sessions, demandes, décisions, événements, résultats et exports autorisés ; limites applicables, état de connexion du client IA et paramètres qu'il peut réellement modifier |
| Administrateur | Vue unifiée de l'organisation et des tenants explicitement administrés : utilisateurs et appartenances, agents/sous-agents, sessions, sandboxes, workers, brokers/hôtes/runtimes, quotas, usage observé, politiques, décisions, audit et incidents ; gestion selon les seules permissions déléguées |
| Synthèse à 360° | Situation globale puis accès aux détails par utilisateur, tenant autorisé, projet, session, agent, worker ou broker ; filtres cohérents entre vues, liens de causalité et retour au contexte sans devoir consulter un dashboard par worker |
| Configuration et diagnostic | Profils IA effectifs, modèle, mode abonnement/API/interne, provenance des réglages, paramètres réseau/TLS et identité locale/OIDC selon les droits ; prérequis manquants, restrictions administratives et actions de reprise explicites, sans afficher de credential |

La vue à 360° couvre tout le périmètre autorisé, y compris plusieurs brokers ou plusieurs tenants rattachés à une organisation lorsque le contrôle l'autorise. Elle ne donne aucun accès implicite aux autres clients. Les droits sur métadonnées, contenus sensibles, export et mutations sont distincts ; voir une ressource ne donne pas automatiquement le droit d'en consulter tous les messages ou de l'administrer. La configuration personnelle simple sans SSO ne doit pas imposer les écrans de gestion d'entreprise.

Le control plane autorise et agrège les sources ; le frontend ne fusionne pas des périmètres en faisant autorité et ne reçoit aucun credential global. Les bases clients restent distinctes. Chaque synthèse précise portée, filtres, période, provenance, fraîcheur et sources absentes ou déconnectées. Un total global ne se déduit pas d'une page chargée ; aucune collecte partielle ne devient zéro activité, aucune réservation ne devient consommation réelle ou montant facturé. Les états et configurations affichés sont fournis par les API, sans compteur ni capacité inventés.

Le changement de rôle, de périmètre, la révocation, l'expiration ou la déconnexion doivent invalider les lectures et abonnements concernés et retirer les données devenues interdites, y compris détails ouverts et caches. Les URL directes, recherches, exports et flux temps réel respectent les mêmes autorisations côté serveur. Les refus, limitations et mises à jour manquantes restent visibles avec un moyen de reprise approprié. Le dashboard conserve les thèmes, la navigation clavier, les commandes stables et la lisibilité à 320/390 px et sous charge.

Les opérations de gestion passent par des contrats API Rust explicites, validés et audités avant effet ; aucun accès moteur depuis le navigateur. Ne pas ajouter de commande sans effet, d'écran de démonstration ni de sélection locale « admin » pour simuler ce comportement. La livraison exige ensemble les contrats serveur, l'agrégation autorisée, les vues frontend, la documentation et les [preuves de rôles et de fraîcheur](engineering/TEST_STRATEGY.md#identité-individuelle-sso-et-découverte--à-qualifier).

## Périmètre livré

Le dossier `front/` contient une interface TypeScript et Vite sans framework ni dépendance JavaScript d’exécution. Les neuf vues sont en français : vue d’ensemble, agents, actions, sécurité, fichiers, sandboxes, workers, quotas et allocations. La production utilise les fichiers statiques de `front/dist/`, servis par le service Rust dashboard. Aucun hébergement public ni service cloud n’est nécessaire.

La vue d’ensemble présente les comptes fournis par `/api/overview`, les décisions, le journal récent, la santé des services, les ressources disponibles et l’intégrité du journal. Une valeur absente ou `null` est affichée « Inconnu » ; zéro reste affiché comme zéro. Aucun pourcentage de progression, consommation CPU, coût, débit, agent ou conteneur de démonstration n’est généré dans l’application.

Le statut « Flux connecté » indique une connexion SSE ouverte. Il ne constitue pas une preuve que le moteur de conteneurs ou un agent est opérationnel. Les horodatages sont affichés dans le fuseau du navigateur, avec la date complète dans le titre de l’heure et dans le détail.

## Authentification et frontières de confiance

Fonctionnement livré du parcours `viewer`, distinct de l'identité nominative locale ou OIDC à livrer :

- Le formulaire demande le jeton **viewer**. Le champ est immédiatement vidé lorsqu’une tentative commence.
- Le jeton vit seulement dans une instance JavaScript en mémoire. Aucun cookie, paramètre d’URL, `localStorage`, `sessionStorage` ou IndexedDB ne le conserve.
- Les requêtes utilisent `Authorization: Bearer …`, `credentials: omit`, `cache: no-store` et `redirect: error`. Un refus 401/403 du client actif ferme la session, efface les données et détails, puis réaffiche la connexion, même si une autre lecture échoue simultanément ou si le détail a déjà été fermé. Une réponse tardive d'un ancien client ne déconnecte pas la nouvelle session.
- Le client n’accepte que les routes de lecture connues, sous `/api/` sur la même origine. Il ne contient aucune fonction de lancement, d’arrêt, de validation de politique ou d’écriture.
- Une actualisation de la page déconnecte la session. Le thème suit initialement le système, puis le bouton clair/sombre agit uniquement sur la page courante.
- Les commandes, noms d’agents, sorties et objets JSON sont traités comme du texte. Le DOM utilise `textContent`, `createTextNode` et des nœuds construits explicitement, sans `innerHTML`, `eval`, gestionnaire inline ou URL fournie par un agent.
- Le document inclut une CSP stricte : scripts et styles de même origine, aucune source externe, `object-src 'none'` et `base-uri 'none'`. Le service Rust ajoute ses en-têtes de sécurité, notamment la protection contre l’intégration en iframe. Aucune police ni icône n’est téléchargée depuis un CDN.

La CSP du serveur de développement est adaptée uniquement aux styles injectés par Vite : un nonce aléatoire par lancement est fourni à Vite et à la directive `style-src`. Les WebSockets sont limités aux adresses locales de développement. Le bundle de production conserve une politique sans scripts ni styles inline.

## Contrat HTTP

Les types partagés côté front sont dans `front/src/types.ts`.

| Route                          | Usage                                                  |
| ------------------------------ | ------------------------------------------------------ |
| `GET /healthz`                 | Disponibilité HTTP du dashboard, sans jeton            |
| `GET /api/overview`            | Comptes, état déclaré par le collecteur et ressources  |
| `GET /api/agents`              | Liste des identités et relations explicites            |
| `GET /api/sandboxes`           | Paramètres de conteneurs reçus                         |
| `GET /api/events`              | Journal paginé et filtré                               |
| `GET /api/stream`              | Événements `event: audit`, avec identifiant séquentiel |
| `GET /api/export?format=jsonl` | Export JSONL selon les filtres de la requête             |
| `GET /api/integrity`           | Résultat de vérification de la chaîne locale           |
| `GET /api/v1/workers?limit=50` | Registre durable des workers du client authentifié |
| `GET /api/v1/workers/{uuid}` | Relecture d’un worker et de sa dernière observation runtime |
| `GET /api/v1/quota` | Politique et réservations durables CPU/RAM/workers |
| `GET /api/v1/usage?limit=50` | Allocations terminales observées, méthode 1, non facturables |

La recherche transmet `q`, `agent_id`, `sandbox_id`, `project_id`, `kind`, `decision`, `from`, `to`, `limit=100` et éventuellement `before` via `URLSearchParams`. La requête d’historique retourne les événements en ordre décroissant. Le bouton « Charger les précédents » utilise `next_cursor`.

Le sélecteur de période propose toutes les périodes, la dernière heure, 24 heures et 7 jours. Il fixe une date `from` RFC3339 au moment du choix et l’affiche sous les filtres. Cette date reste identique lors d’une actualisation, d’un filtrage, d’une navigation, de la pagination et de l’export ; elle est recalculée uniquement au choix d’une autre période. L’absence de borne `to` conserve l’arrivée des nouveaux événements en direct. Le filtre SSE utilise `received_at`, comme l’API, et se replie sur `timestamp` seulement si l’horodatage de réception n’est pas disponible ou exploitable. Les types et le client prennent également en charge une borne `to` facultative.

Les vues Actions, Sécurité et Fichiers sont des projections des événements chargés ; leur pied de tableau indique le nombre affiché et le nombre chargé. La vue Fichiers se fonde sur les types `file.*`, `files.*`, `artifact.*`, `workspace.diff*`, `workspace.snapshot*`, `workspace.import*` et `workspace.export*`. Elle ne parcourt pas le système de fichiers. Un détail expose les données brutes reçues lorsque les chemins ou différences sont fournis dans l’événement.

L’export utilise un `fetch` authentifié, puis un `Blob` et une URL objet temporaire. Il reprend tous les filtres appliqués : `q`, `agent_id`, `sandbox_id`, `project_id`, `kind`, `decision` et les bornes de période `from`/`to`. Il inclut tous les résultats correspondants, sans reprendre `before`, `after` ou `limit` de pagination. Sans filtre appliqué, il contient le journal complet retourné par l’API. Les filtres sont figés au clic pour toute la requête d’export. Le jeton n’est jamais ajouté au lien de téléchargement.

## Temps réel et états incomplets

Le flux est lu avec `fetch`, `ReadableStream` et un `TextDecoder` UTF-8. Le parseur supporte les coupures entre fragments, CRLF/LF/CR, commentaires, champs `data` multiples et identifiants. Un message trop volumineux déclenche une reconnexion au lieu de laisser croître le tampon sans borne. Les objets ne possédant pas les champs essentiels d’un événement sont ignorés. Le curseur n’avance qu’après réception d’un événement valide.

Les événements sont dédoublonnés par `seq`. Après interruption, le client se reconnecte avec `Last-Event-ID` dans l’en-tête et un délai exponentiel de 1 à 30 secondes. Aucune authentification n’est passée en paramètre de l’URL SSE. La déconnexion annule les requêtes et la boucle de reconnexion. Les événements live reçus pendant une requête d’historique sont conservés lors de sa résolution.

Une reconnexion, un refus d’authentification et une API indisponible ont des états dédiés. Les erreurs de lecture ne sont pas présentées comme la preuve d’une liste vide. Les résumés sont relus après les événements, regroupés sur une fenêtre d’une seconde, et toutes les 30 secondes. Le tableau live conserve au plus 1 000 événements ; l’historique antérieur reste rechargeable par curseur.

## Workers, quotas et allocations

Ces trois vues lisent les contrats v1 du contrôle, sans déduire un worker des journaux fournisseurs ni des sandboxes historiques. Le périmètre est celui du client authentifié par le jeton ; aucun sélecteur, header ou paramètre ne choisit un autre client. Aucune commande de lancement, arrêt, réconciliation ou modification de quota n’est disponible dans le navigateur.

Le registre sépare l’état durable (`reserved`, `preparing`, `created`, `start_issued`, `running`, `stopping`, `terminal`, `in_doubt`, `cancelled`) de l’observation runtime datée. Un démarrage émis ne devient pas une exécution observée. Sans observation runtime, l’interface indique l’inconnue. Le détail relit l’identifiant réel et expose session, tentative, révision, runtime, ressources, limites, dernière observation, motif d’incertitude et résultat de nettoyage. Le modèle de consultation conserve uniquement ces champs nommés ; les commandes et métadonnées de credentials sont exclues du détail. Cette sélection d’affichage ne remplace pas la protection des réponses côté serveur. Un détail d’agent propose les workers de la page chargée dont la session ou l’identité runtime correspond exactement ; aucune relation n’est déduite des noms ou fournisseurs. L’absence de correspondance sur une page partielle ne prouve pas l’absence de worker.

Les quotas exposent les ressources réservées et les limites de la politique du client. Un quota peut rester retenu pour un worker incertain. Les octets exacts accompagnent les unités de mémoire lisibles. Une valeur inconnue n’est pas un quota nul ; les réservations ne représentent pas une mesure de CPU consommé.

Le relevé d’allocation utilise exclusivement les observations terminales du ledger. Les durées et produits CPU/RAM sont des chaînes décimales conservées sans conversion en nombre flottant : durée en secondes, CPU alloué en milliCPU·s et mémoire allouée en octet·ns. La provenance observée, le runtime et l’autorité fournis par la source accompagnent la période. Il affiche la méthode 1 et `billable=false`, sans prix, coût, facture ou total extrapolé à partir d’une page. L’absence d’allocation terminale laisse la durée inconnue, y compris pour un worker actif ; le signal durable d’allocation incohérente reste visible sur le worker. Un contrat incomplet, une source sans provenance observée, une méthode inconnue ou une unité invalide produit une erreur de lecture explicite.

Workers et allocations utilisent une page de 50 éléments et un curseur UUID opaque fourni par l’API, avec page suivante, page précédente et retour à la première page. Une seule page de données est conservée par vue ; la position de navigation est mémorisée pendant la session. Le résultat n’annonce aucun compte global à partir du nombre d’éléments chargés. Le focus clavier reste dans la vue lors d’une pagination et les tableaux peuvent défiler horizontalement sur petit écran.

Les états chargement, vide, erreur, lecture périmée et flux déconnecté sont distincts. Chaque lecture réussie porte sa date ; une erreur ultérieure conserve les dernières données avec leur avertissement et propose « Réessayer ». Un 401/403 ferme la session et efface les collections. Les vues se chargent à leur ouverture, puis sont relues toutes les 30 secondes lorsqu’elles sont ouvertes. Les événements `control.worker.*`, `worker.*`, `quota.*` et `allocation.*` invalident les données ; leur rafraîchissement est regroupé sur une seconde avec une seule requête en vol par ressource. Aucun appel de registre n’est envoyé pour chaque événement d’une rafale. Une reconnexion invalide également les données.

## Agents, provenance et couverture

Le tableau d’agents présente l’état, le fournisseur, le modèle, la provenance et la compatibilité reçus. La hiérarchie n’utilise que `parent_id` et les identifiants explicites. Elle ne déduit aucun parent à partir du nom, du modèle, du projet ou de la session. Un parent absent ou cyclique est signalé et l’agent reste visible. L’affichage imbriqué est limité à 30 niveaux ; le tableau reste disponible.

Les comptes principaux `agents` et `subagents` représentent les identités observées. Les comptes `declared_agents` et `declared_subagents`, lorsqu’ils sont fournis, sont ajoutés comme mentions séparées « + N déclarés » ; ils ne sont pas additionnés aux observations. Le détail d’un agent affiche `field_provenance` pour chaque champ renseigné, notamment modèle et tâche. La provenance globale de l’identité ne remplace donc pas celle de ses attributs. Les indications `parent_unresolved` et `hierarchy_warning` restent visibles même lorsque le collecteur a supprimé un lien parent invalide.

| Valeur technique        | Libellé français | Sens présenté                                   |
| ----------------------- | ---------------- | ----------------------------------------------- |
| `observed` (provenance) | Observé          | Mesuré ou constaté                              |
| `declared`              | Déclaré          | Fourni par un agent                             |
| `inferred`              | Inféré           | Déduit, à confirmer                             |
| `native` (couverture)   | Native           | Couverture native indiquée par la source        |
| `observed` (couverture) | Observée         | Couverture d’observation indiquée par la source |
| `generic`               | Générique        | Intégration générique indiquée par la source    |

Les badges de provenance et couverture sont distincts. Les valeurs techniques de provenance sont consultables dans le titre du badge. Aucun niveau n’est déduit de la marque d’un agent. La carte d’intégrité rappelle que le chaînage local ne protège pas contre une réécriture complète par un administrateur.

## Accessibilité et adaptation

L’interface propose une navigation sémantique, un lien d’évitement, des labels de formulaires, des états de flux annoncés, des erreurs annoncées, un focus clavier visible et des badges textuels indépendants de la couleur. Les détails utilisent un dialogue natif modal, refermable avec Échap. Le détail worker retrouve son bouton de déclenchement après une relecture du tableau pendant l’ouverture ; si la ligne disparaît, le focus rejoint le résumé de page, ou le titre de la vue lorsque le résumé n’existe plus. Les onglets de détail supportent les flèches, Début et Fin. Le tableau horizontal reste défilable dans son panneau sur petit écran, sans déborder la page. Le menu mobile fermé est retiré du parcours clavier. Les animations respectent `prefers-reduced-motion`.

Ces contrôles constituent une vérification fonctionnelle de base ; ils ne remplacent pas un audit complet WCAG avec lecteur d’écran.

## Développement et vérification

```bash
cd front
npm ci
npm test
npm run build
AGENT_SANDBOX_GATEWAY=http://127.0.0.1:8787 npm run dev
```

Vite écoute sur `http://127.0.0.1:5173` et relaie `/api` et `/healthz` vers la cible. Le port est strict pour conserver une origine de développement prévisible. `npm run preview` sert le build sur `127.0.0.1:4173` ; pour un usage complet, le serveur Rust dashboard fournit le relais de lecture.

`npm test` exécute les tests Node du client API, du modèle et du parseur SSE. Les fichiers `.mjs` importent directement les fichiers TypeScript grâce au support de Node 24 utilisé pour la validation. Node 24 est recommandé pour reproduire ces contrôles.

`npm run test:browser` exerce le bundle de production avec un serveur HTTP de fixtures isolé, limité à `127.0.0.1`. Playwright doit être disponible dans l’environnement de test, soit par résolution Node, soit dans `CODEX_PRIMARY_RUNTIME_NODE_MODULES`. `PLAYWRIGHT_CHROMIUM_EXECUTABLE` peut désigner un binaire Chromium existant. Les fixtures restent dans `front/tests/` et ne sont jamais importées par le bundle. Les captures dans `front/tests/artifacts/` sont des éléments de contrôle local, pas une démonstration de sessions Docker/Podman réelles.

Le parcours couvre le refus puis l’acceptation du jeton, le rendu d’une chaîne HTML malveillante, les détails de commande et JSON, la reconnexion avec curseur, la hiérarchie explicite, la recherche, l’export authentifié, l’absence de stockage de jeton, les thèmes, le format mobile, l’état vide et l’indisponibilité de l’API. Pour le contrôle v1, il vérifie également pagination et focus, détails réels de fixture, inconnue runtime, quotas, unités d’allocation au-delà de la précision flottante, lien explicite agent/worker, conservation des données périmées, refus d’un DTO incomplet, rafale de 200 événements et déconnexion sur 403. Ces données restent des fixtures de tests et ne qualifient pas une exécution réelle de workers.

La validation navigateur utilise un collecteur de test explicite ; elle n’atteste pas de l’exécution d’un agent réel ni d’un moteur Docker/Podman. La validation de la chaîne complète est décrite dans le dossier de validation général du projet.
