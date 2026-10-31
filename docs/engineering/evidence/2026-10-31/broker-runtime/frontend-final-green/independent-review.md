# Revue indépendante ciblée — frontend des échecs broker

Date locale : 1 novembre 2026. Checkout : `/Users/louis/Documents/ai-guard/.worktrees/broker-service`, branche `codex/broker-service`, HEAD observé `6c7b63ebcef4a92cadfc2ec95626033f48011576`.

## Périmètre et méthode

Lecture seule du delta contre HEAD de `front/src/model.ts`, `front/src/main.ts` et `front/tests/model.test.mjs`. Lecture d’AGENTS.md, du contrat d’architecture, des règles de code, des gates, de TEST_STRATEGY et du contrat UI ; correspondance ponctuelle avec les producteurs Rust de diagnostics, sans audit global ni avis sur les autres changements du worktree. Le skill requesting-code-review a été lu ; la présente mission est le second regard indépendant demandé par le coordinateur.

Aucun fichier du dépôt modifié. Aucun test, Cargo, npm, navigateur, moteur, fournisseur ou modèle lancé par le reviewer. Aucun `.env`, credential ou clé ouvert. Le présent rapport est le seul fichier créé, hors dépôt, mode 0600.

SHA256 observés avant et après la lecture :

- model.ts : `7ae01f246b70f25b8ab709c97ab53845403a7e6623c1efff43005391060a8ef1`
- main.ts : `2ae6c8d9099192f890e5cf08f9ecb8d94a4ce519c4423141c7863bb4460035a6`
- model.test.mjs : `e4a132f140280ecf7adb9ba4a3234aeaed71c62a19d3f4414290616c91c0f1cc`

## Résultat de revue

Aucun défaut P0 ou P1 identifié dans ce delta. Aucun défaut fonctionnel supplémentaire suffisamment étayé pour demander une correction du code n’a été trouvé. Cette conclusion est bornée à la lecture statique et aux logs nommés ci-dessous ; elle ne qualifie ni le rendu navigateur ni la production.

Constats vérifiables par lecture :

1. `front/src/model.ts:153` reconnaît exclusivement `broker.batch.failure`. La version externe doit être le nombre 1 (`:155`). Les champs arbitraires, versions absentes ou non supportées ne deviennent pas des libellés ; une version non supportée conserve le signal d’échec avec phase/étape inconnues. Les Map privées (`:112`, `:123`, `:138`, `:144`) évitent la résolution de noms hérités tels que `constructor`, `toString` et `__proto__`.
2. `front/src/model.ts:167` applique la priorité demandée : étape broker connue, sinon étape enforcer connue, sinon étape runtime connue, sinon inconnue. Une étape broker inconnue ne masque donc pas un diagnostic natif reconnu. `stage()` rejette les tableaux, primitives et nulls comme sources d’étape. Les étapes des producteurs actuels (`src/runner/broker/executor/diagnostics.rs`, `src/broker/enforcer/diagnostics.rs`, `src/runtime/prestart.rs`) ont des correspondances ; l’ancien `load_after_registration` reste une entrée fermée de compatibilité.
3. `front/src/model.ts:172` fixe toujours l’effet à « non confirmé », indépendamment de `runtime_effect`. `eventSummary()` (`:177`) traite l’échec avant les commandes, les motifs et les textes génériques ; des argv ou causes arbitraires ne contaminent pas ce résumé. Les libellés localisent le signalement sans prouver une requête, un ACK, un démarrage, un arrêt ou un terminal.
4. `front/src/model.ts:221` rend l’événement exact visible dans Actions même sans commande/décision. La projection Sécurité (`:226`) conserve ses conditions existantes ; ce type n’est pas ajouté comme nouvel incident. Les autres événements conservent leur chemin de résumé et leurs projections antérieurs.
5. `front/src/main.ts:1831` évite d’afficher commande, résultat, code de sortie ou sortie comme preuve d’exécution pour un échec broker. Le résumé détaillé (`:1883`) utilise le même helper que la liste, supprime le bloc Décision et son badge pour ce type (`:1915`), et conserve provenance, couverture, identifiants et chaînage. Le texte décrit explicitement la limite de l’observation. Les nouvelles valeurs sont injectées via `el` et `labelValue`, qui créent du texte ; aucun nouveau HTML interprété ou appel réseau/mutation n’est ajouté.
6. Les quatre nouvelles fonctions de test (`front/tests/model.test.mjs:228`, `:245`, `:255`, `:288`) portent sur le comportement public : phase/étape et effet, projection Actions/Sécurité, canaris privés, refus des métadonnées inconnues, étapes natives et ordre de priorité. La dernière couvre les dix étapes natives avec broker_step absent et null, les collisions de noms de prototype, les confusions entre tables, une version externe 2 et un kind suffixé. Ces boucles sont des cas au sein de quatre tests, pas des tests supplémentaires.

## Preuves lues, sans réexécution

- RED initial : `/private/tmp/ai-guard-frontend-red-20261031-232803/summary.txt` et `frontend-red.log`. La commande filtrée sélectionne trois tests ; RC 1, trois échecs d’assertion portant sur le résumé absent et la projection Actions absente. Aucun échec de chargement/compilation dans ces logs.
- GREEN initial historique : `/private/tmp/ai-guard-frontend-green-20261031-233407/final-summary.txt`. Tests modèle, npm ci offline et build RC 0 ; ce gel antérieur ne remplace pas le gel final.
- RED complémentaire : `/private/tmp/ai-guard-frontend-final-red-20261031-235936/summary.txt` et `frontend-red.log`. Un test ciblé sélectionné, RC 1, assertion `enforcer.register` échouée ; pas d’erreur de loader ou compilation.
- GREEN final : `/private/tmp/ai-guard-frontend-final-green-20261101-000748/01-model-tests.log` et `.rc`. Commande annoncée `node --test --test-isolation=none tests/model.test.mjs` : 12 tests, 12 PASS, 0 FAIL, 0 ignoré, RC 0. Ce total inclut les tests préexistants et les quatre nouvelles fonctions ; ne pas ajouter les boucles, assertions ou exécutions RED/GREEN comme tests uniques.
- Build final : même dossier, `02-npm-build.log` et `.rc` : `npm run build`, contrôle TypeScript et Vite 7.3.7 réussis, RC 0. Les dix modules transformés ne sont pas des tests.
- Environnement final consigné : Node v25.6.1, npm 11.13.0. Manifestes sources avant/après et manifeste du checkout portent les trois SHA attendus. Les logs de vérification des sept fichiers sources et des quatre artefacts copiés affichent OK. Le dossier et les deux logs principaux sont observés privés (0700/0600).

## Limites et statuts

- PASS observé dans les logs : 12 tests du modèle et build statique sur ce gel, avec Node 25.6.1. Ce reviewer n’en revendique pas une nouvelle exécution.
- NOT_RUN : Node 24, suite npm complète, navigateur/validation visuelle, clavier/focus et thèmes sur les branches modifiées, livraison réelle via dashboard Rust et flux API/SSE. Les tests modèle importent uniquement model.ts (`front/tests/model.test.mjs:3`) ; ils n’exécutent pas les nouvelles branches de showEvent. Le build vérifie leurs types, sans prouver leur rendu. Il s’agit d’une limite de preuve connue, pas d’un défaut de comportement démontré.
- La protection par listes fermées concerne les résumés et le détail spécialisé. L’onglet Données brutes (`front/src/main.ts:1826`) continue d’exposer le JSON reçu, selon le contrat UI existant ; il n’est pas un filtre de secrets. Les canaris absents des résumés ne prouvent donc pas un masquage de l’ensemble du payload ni une garantie serveur. Aucun nouvel accès brut n’est introduit dans le delta.
- NOT_RUN dans cette revue : Rust, runtime, fournisseurs, isolation tenant et qualification globale. Aucun avis sur le delta Rust, le contenu des autres documents modifiés ou la fermeture d’un gate. Les preuves frontend ne démontrent aucun effet moteur, aucune correction de deadline, aucun terminal ni qualification production.
- Blocage propre à la lecture/revue : aucun. Les validations Node 24 et navigateur restent explicitement non exécutées.
