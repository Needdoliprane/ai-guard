# Dashboard — identité authentifiée et bascule de client

29 octobre 2026 ; `/Users/louis/Documents/ai-guard` ; périmètre frontend uniquement. Les contrats AGENTS/RTK/architecture/code/tests/UI et tenancy/usage/recette ont été lus. Les skills locaux `ai-guard-verify` n’ont pas été trouvés dans `.agents/skills/` ni `.claude/skills/` ; aucun chargement automatique revendiqué. Diagnostic systématique, tests avant comportement et vérification avant achèvement appliqués.

## Besoin et résultat

La connexion lit maintenant `GET /api/v1/identity`, route réelle du gateway, puis valide une identité `viewer` et un tenant de 1 à 128 caractères ASCII alphanumériques, `-` ou `_`, selon `src/tenancy.rs`. Le modèle garde uniquement `{tenant_id,role}`. Aucun tenant ne provient d’un sélecteur, paramètre ou header libre. Seule la route exacte est autorisée par le client API ; ses variantes avec query/hash ou suffixe sont refusées.

L’interface affiche le client authentifié dans le cadre de navigation et le pied de page. Le jeton reste en mémoire et le champ est vidé au début de la tentative. Une identité absente, incompatible ou non-viewer garde le formulaire de connexion et une erreur ; aucune donnée n’est lue avant validation de cette identité. Une erreur overview après validation ne bloque plus la session : les états inconnus et l’erreur de lecture sont affichés, avec les commandes de reprise existantes.

La déconnexion efface aussi l’identité et la date de dernière lecture. Le périmètre de 128 caractères revient à la ligne, y compris sur écran 390×844.

## Constat d’isolation

Aucune fuite A→B n’a été reproduite dans les gardes préexistantes. `refreshSummary` compare l’instance courante du client ; `loadEvents` compare client et numéro de requête ; SSE et export comparent le client ; `ControlDashboard.close()` marque l’ancienne instance fermée et efface ses collections, tandis que lecture/détail refusent ensuite ses retours. Ces gardes et `front/src/control-view.ts` ont été conservés sans modification.

Le nouveau parcours navigateur charge d’abord les collections et détails A, puis retient douze continuations A après réception du transport : overview, agents, sandboxes, intégrité, événements et pagination, pages workers et usage, détail worker, quota, export et fragment SSE. Le test se déconnecte, connecte B dont les données répondent HTTP500, puis libère A. Aucun marqueur, identifiant, compteur ou dialogue A n’apparaît sous B ; l’export A tardif ne déclenche aucun téléchargement. Une erreur B reste une erreur, sans écran vide présenté comme preuve d’absence. Une reprise B réussie, puis une nouvelle panne B, conserve uniquement la dernière lecture B avec son état périmé. Un refus A403 tardif ne ferme pas la nouvelle session B.

La barrière de test est injectée exclusivement dans le navigateur de fixture : le transport réel a terminé avant la retenue de sa continuation. Elle éprouve les gardes applicatives même lorsque l’annulation seule ne suffit pas. Aucun hook de test n’a été ajouté au produit.

## Fichiers

- `front/src/api.ts` : route exacte d’identité, validation du modèle et erreur explicite.
- `front/src/types.ts` : identité viewer.
- `front/src/main.ts` : connexion par identité, portée affichée, effacement de l’identité/date à la déconnexion.
- `front/src/style.css` : retour à la ligne du périmètre, largeur du bloc de navigation.
- `front/tests/api.test.mjs` : route authentifiée et refus de variantes, identité viewer valide/invalide et projection du modèle.
- `front/tests/browser.mjs` : fixture historique identité locale, exécution du parcours identité supplémentaire.
- `front/tests/browser_identity.mjs` : nouvelle fixture isolée A/B et barrières décrites ci-dessus.

Pas de dépendance ni de package/lock modifié. Pas de documentation globale modifiée, selon la propriété de fichiers convenue. Le checkout contient tout le projet non suivi par Git ; les comparaisons ont utilisé des copies sources avant changement dans `/tmp/ai-guard-dashboard-identity/`.

## Preuves avant changement

| Statut | Vérification | Résultat |
|---|---|---|
| FAIL attendu (RED) | `npm test` avec nouvelle lecture identité | 25 PASS, 1 FAIL : route `/api/v1/identity` non autorisée ; `node-red.log` |
| FAIL attendu (RED) | `node tests/browser_identity.mjs` avant changement produit | La session ouvre les données alors que l’identité fixture est indisponible ; l’erreur de connexion attendue n’apparaît pas ; `browser-red.log` |
| FAIL attendu (RED) | Identifiant de 128 caractères sur mobile, après premier affichage de portée | Débordement horizontal confirmé ; `browser-long-red.log` ; correction ciblée sur la classe du pied de page |

Ces FAIL concernent le comportement d’identité nouvellement demandé et le débordement de son nouveau libellé. Ils ne sont pas présentés comme preuve d’une ancienne fuite A→B.

Un essai intermédiaire de la nouvelle fixture (`browser-identity-green.log`, code 1) attendait un bouton Réessayer alors que la remontée sur Workers déclenche déjà une nouvelle lecture réussie. La fixture a été corrigée pour attendre cette lecture automatique ; le produit n’a pas été modifié pour cet échec de test. La réussite ciblée suivante est `browser-identity-final.log`.

## Vérification finale

Node 24.22.2 ; npm 11.13.0 ; Google Chrome 156.0.8078.48 ; Darwin ARM64 ; macOS 27.2. Versions brutes : `versions.log`. Toutes les commandes front ont Node24 en tête de PATH : `/Users/louis/.npm/_npx/387698761821791d/node_modules/node/bin`.

| Statut | Commande dans `front/` | Résultat brut |
|---|---|---|
| PASS | `npm run build` | tsc et Vite, code 0 ; JS 57,91 kB, CSS 30,72 kB ; `build-final.log` |
| PASS | `npm test` | 27/27, zéro échec/ignoré ; code 0 ; `node-final.log` |
| PASS | `npm run test:browser` | Parcours historique complet + nouveau parcours identité et identifiant long sur mobile ; code 0 ; `browser-complete-final.log` |
| PASS | Focus worker après remplacement SSE et disparition de ligne | Bouton stable puis résumé de page retrouvés ; mêmes logs navigateur |
| NOT_RUN | Nouvelle UI avec services Rust/conteneurs réels | À intégrer et qualifier par root avec le mode dashboard partagé ; ces fixtures ne prouvent aucun runtime |
| NOT_RUN | Audit WCAG/lecteur d’écran complet | Vérifications fonctionnelles du navigateur seulement |
| NOT_APPLICABLE | Cargo/clippy/tests Rust dans ce lot front | Aucun fichier Rust modifié par cet agent ; root vérifie son lot serveur |

Variables navigateur : `CODEX_PRIMARY_RUNTIME_NODE_MODULES=/Users/louis/.npm/_npx/9833c18b2d85bc59/node_modules` et `PLAYWRIGHT_CHROMIUM_EXECUTABLE=/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`.

Tous les logs mentionnés sont sous `/tmp/ai-guard-dashboard-identity/`. Les SHA256 finaux sont dans `sha256-final.txt`. `front/src/control-view.ts` reste `03f087ce5cf5d3ab99fa03253c313a31cba2fc16f261971d11ecd93c90a940b5` ; `front/package-lock.json` reste `30d12765534dd2b2f89ae78b0b9f180ccc0fbfeb755fac33bd972608c70f2c44`.

## Critères et limites

PASS logiciel sur ce périmètre : client issu de l’identité authentifiée ; refus avant lecture pour identité invalide/non-viewer ; fonctionnement sous B500 ; aucun ancien contenu A au changement de session malgré réponses tardives ; péremption limitée à une identité ; erreurs distinctes du vide ; navigation/focus préexistants conservés ; identifiant maximum utilisable sur mobile.

Aucun blocage frontend externe identifié. L’intégration du relais partagé/fixe et sa revue indépendante restent à root ; ce rapport ne qualifie aucune release ni moteur/confinement/cloud. La présence de ces tests ne remplace pas les permissions serveur.
