# ai-guard — refonte éditoriale du frontend, 30 octobre 2026

## Périmètre et changements

Implémentation du contrat visuel validé dans `docs/ui.md`, sur la base des règles d’AGENTS.md et des contrats architecture/code/tests/qualification lus avant modification. Le checkout n’a pas de HEAD et tous ses fichiers sont non suivis : aucun commit, reset, worktree, effacement global ou déploiement effectué.

Fichiers modifiés exclusivement dans le périmètre attribué :

- `front/src/theme.css` : palette exacte claire, navigation sombre, citron rare, couleurs de verdict discrètes ; thème sombre neutre ; focus bleu sur contenu et blanc dans le dock.
- `front/src/style.css` : remplacement du CSS antérieur (2177 → 434 lignes), Arial/Helvetica, Courier New pour repères/dates, titres éditoriaux, conteneur1800px et gouttières adaptatives, traits1px et rayons4–5px ; indicateurs séparés par des lignes ; panneaux/logs/tableaux lisibles ; aucun gradient ni ombre décorative.
- `front/src/main.ts` : identité typographique `ai-guard`, login sans diagramme fictif, formulaire avant les explications détaillées sur mobile et dans le DOM ; en-tête clair et dock sombre fixes ; neuf liens numérotés réels ; position `Vue NN / 09` ; précédent/suivant avec destination annoncée et bornes désactivées ; « Suivant » citron16px/700, précédent en contour, actualisation/export secondaires ; commandes à coordonnées stables ; navigation avec retour en haut et focus sur titre visible ; titre du document et nom d’export `ai-guard`.
- `front/index.html`, `front/public/favicon.svg` : identité ai-guard et favicon typographique noir, sans cube turquoise.
- `front/tests/browser.mjs` : nouveau parcours sans ancien drawer ; géométrie et cibles44px, commandes fixes après50workers, précédent/suivant effectifs, clavier et focus, défilement horizontal natif ArrowLeft/ArrowRight, mouvement réduit, galerie9vues et captures chargement/vide/périmé/erreur/dense,320/390/1280/1440/paysage844.
- `front/tests/browser_identity.mjs` : protections existantes conservées ; tests et captures d’identité client128caractères à320/390/paysage, stabilité des commandes et titre visible après navigation.

`control-view.ts`, `api.ts`, `model.ts`, les modèles d’identité, les transports SSE et tout Rust restent inchangés par cet agent. Les commandes du dashboard restent en lecture seule ; aucun faux sélecteur de langue, compteur ou événement ajouté au produit. Les fixtures restent exclusivement dans les tests.

## Commandes et résultats constatés

Environnement explicitement utilisé : Node24.22.2, npm11.13.0, Vite7.3.7, TypeScript5.9.3, GoogleChrome156.0.8078.76, Playwright1.64.0-alpha-1790635538000. PATH Node : `/Users/louis/.npm/_npx/387698761821791d/node_modules/node/bin`. Playwright : `/Users/louis/.npm/_npx/9833c18b2d85bc59/node_modules`. Chromium : `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`.

Dans `front/` :

| Statut | Commande | Résultat | Journal |
|---|---|---|---|
| PASS | `npm ci` |15paquets installés, audit npm0vulnérabilité annoncée |`/tmp/ai-guard-editorial-ci.log`|
| PASS | `npm test` |27tests,27PASS,0FAIL,0ignoré |`/tmp/ai-guard-editorial-node.log`|
| PASS | `npm run build` |TypeScript + bundle statique Vite ;10modules ; JS57.47kB/CSS29.24kB |`/tmp/ai-guard-editorial-build.log`|
| PASS | `CODEX_PRIMARY_RUNTIME_NODE_MODULES=… PLAYWRIGHT_CHROMIUM_EXECUTABLE=… npm run test:browser` |parcours principal + import du parcours d’identité terminés,0exception navigateur |`/tmp/ai-guard-editorial-browser.log`|
| FAIL attendu puis PASS |régression dock permanent avant implémentation |ancien front :0dock au lieu de1 ; nouvelle structure vérifiée |`/tmp/ai-guard-editorial-red.log`|
| FAIL attendu puis PASS |régression accès authentifié avant les explications détaillées sur mobile |ancien ordre :input après facts ; nouvel ordre DOM et visuel vérifié |`/tmp/ai-guard-editorial-login-red.log`|

Un essai intermédiaire a demandé un attribut `tabindex=0` sur le tableau horizontal. L’attribut était absent ; la revue a ensuite vérifié que le défilement natif fonctionne depuis le bouton Worker/Détails. Cette assertion d’implémentation a été remplacée par un vrai test ArrowLeft/ArrowRight, qui passe ; aucun changement inutile de `control-view.ts`.

Le premier appel npm build depuis la racine a renvoyé ENOENT faute de package.json ; les commandes qualifiantes ci-dessus ont ensuite été exécutées depuis `front/` avec code0.

## Critères acceptés et preuves

PASS :9vues réelles accessibles en permanence, aucun ancien menu mobile inerte ; vue courante et destination de navigation identifiables ; bornes début/fin explicites. Géométrie vérifiée à1440×1050,1280×720,390×844,320×568,844×390. Hauteurs réservées100+166desktop,90+166intermédiaire,120+194mobile,64+96paysage court. Aucun débordement global. Les tableaux se déplacent dans leur propre conteneur horizontal. Les cibles vérifiées font au moins44px.

PASS :coordonnées de précédent/suivant/actualiser/emplacement export identiques entre vues, après50workers, après défilement et lorsque l’export est inapplicable. Les trois vues de contrôle n’affichent aucun export de journal ambigu. Focus H1 visible après navigation au clic/clavier ; détails modaux et restitution de focus après remplacement SSE/disparition d’un worker préservés. Mouvement réduit vérifié.

PASS :régressions existantes connexion/refus, XSS, données textuelles malveillantes, SSE/reconnexion/curseur, filtres/période/export, mémoire seule, hiérarchie, pagination100workers en2pages bornées, allocations exactes/nonfacturables, état périmé/invalide/vide, rafale200événements et refus403. Identité viewer exigée avant données ; indisponibilité/invalide/runner refusés ;12réponses/flux A tardifs ignorés sousB ; exportA tardif sans téléchargement ; refusA403 tardif sans déconnexion deB ; aucune donnéeA conservée sousB.

Captures générées sous `front/tests/artifacts/editorial-*.png` :56PNG au dernier run. Les captures éditoriales utilisent le viewport après deux requestAnimationFrame ; certaines captures fullPage intermédiaires de Chrome répétaient des tuiles à320px. Les fichiers éditoriaux ont été régénérés et inspectés après correction du procédé de capture. Les PNG hérités sans préfixe editorial ne sont pas le jeu visuel de référence final.

Captures ouvertes avec `view_image`, dont versions finales :

- `editorial-login-1440-light.png`, `editorial-login-320-light.png`, `editorial-login-form-320-light.png` ;
- `editorial-view-01-1440-light.png`, vues04/05/07 à1440 ;
- `editorial-overview-320-568-light.png`, `editorial-overview-390-844-light.png`, `editorial-overview-390-844-dark.png`, `editorial-overview-1280-720-light.png`, `editorial-overview-844-390-light.png`, `editorial-overview-844-390-dark.png` ;
- `editorial-workers-loading-content-320-light.png`, `editorial-workers-data-320-568-light.png`, `editorial-allocations-data-390-844-light.png`, `editorial-long-identity-320.png` ;
- états erreur1440sombre et chargement/vide/périmé/dense générés par le même parcours.

## Revue et limites

PASS :revue indépendante finale `editorial_review` sur le delta Next. Elle confirme45couples9vues×5tailles, focusH1, connexion/mobile/paysage, modal390,500événements avec IDs longs, Actualiser stable9vues×5tailles, navigation flèches/Tab/ShiftTab et0échec de contraste sur5scènes. Neuf captures finales1440 et les captures mobile/paysage/login320 réellement ouvertes ; ordre h1→form→facts confirmé. `browser_identity.mjs` rejoué indépendamment PASS. Aucun défaut bloquant reproduit. Rapport : `/tmp/ai-guard-editorial-independent-review.md` (alias `/tmp/ai-guard-editorial-review.md`).

NOT_RUN dans cette mission :déploiement, navigateur contre les vrais services Rust, qualification Docker/Podman et recette multi-client3workers réelle. Les tests navigateur ci-dessus utilisent des fixtures HTTP exclusivement dans le harness ; ils prouvent la présentation et les gardes du client, pas les effets runtime. Aucune qualification de release ou affirmation « production-ready ».

BLOCKED :les skills projet `ai-guard-verify` attendus dans `.agents/skills/` ou `.claude/skills/` sont absents du checkout. Chargement automatique non confirmé ; vérification directe effectuée avec les contrats et TEST_STRATEGY.

La référence distante Nothin’ n’était pas accessible au tool web ; la spécification détaillée du mainteneur et son contrat local `docs/ui.md` ont fait autorité.

## Correctif final — accès immédiat au login mobile

Après la relecture du premier écran320×568 par le mainteneur, le formulaire et le bouton étaient encore sous la ligne de flottaison. Une nouvelle régression a reproduit ce défaut avant correction (`/tmp/ai-guard-editorial-login-priority-red.log`).

Changement limité à `main.ts`, `style.css` et au test navigateur : introduction générale et paragraphe d’authentification placés après le formulaire dans le DOM ; espacements du login mobile resserrés à20/16px sans réduction des polices ; repère redondant « Session de consultation » masqué sur mobile et écran court ; login paysage compacté. Le titre, le formulaire et ses informations de mémoire/provenance sont conservés. Aucun lien CTA supplémentaire ni nouvelle logique d’authentification.

PASS final : le champ de jeton et le bouton noir sont entièrement visibles dès l’arrivée à320×568,390×844 et844×390. À320, le bouton se termine à536px ; sa zone cliquable est complète. Depuis le champ, Tab atteint directement le bouton. Ordre DOM : H1 → formulaire → introduction et faits. Captures finales inspectées : `editorial-login-320-568-first-screen.png`, `editorial-login-390-844-first-screen.png`, `editorial-login-844-390-first-screen.png`.

PASS après ce correctif :27testsNode, buildTypeScript/Vite et suite navigateur complète + identitéA→B. Journaux finaux aux mêmes chemins ci-dessus. Nouveau bundle à déployer : `dist/assets/index-De0rVJnH.css` (29.49kB), `dist/assets/index-g65sqHFT.js` (57.47kB). Les noms de bundle plus haut décrivent le gel intermédiaire antérieur.

SHA256 final : main.ts `eaeed86d930243b2cf4d2b637fcc2fec4d9394b65cfacf4d35db8564924415ab` ; style.css `2ae7b6d64bd5be0c32eed4b04a13dd3db878371721595419644b4dd34b5e7a6d`.

PASS revue indépendante du correctif final : champ et CTA visibles à320×568 (CTA485.78–535.78),390×844,844×390 et1440×1050 ; H1≥36px ; ordreDOMH1→form→facts ; aucun débordement ; Tabjeton→CTA ; mauvais jeton vidé puisvieweraccepté. Les trois nouvelles captures et une capture indépendante320 ont été ouvertes par le reviewer. Défaut d’entrée mobile fermé ; rapports indépendants mis à jour aux chemins indiqués.
