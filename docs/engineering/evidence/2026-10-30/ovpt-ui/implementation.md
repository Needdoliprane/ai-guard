# ai-guard — signature OVPT, favicon et thème sombre

Date : 30 octobre 2026. Périmètre exclusif `front/**`. Aucun Rust, documentation du dépôt, configuration de déploiement ou service modifié par cet agent. Aucun déploiement exécuté.

## Changements

- `front/src/main.ts` : logo typographique OVPT massif, séparateur et nom de produit ai-guard dans les en-têtes ; noms accessibles groupés « OVPT · ai-guard · Lecture seule » ; signature compacte OVPT/ai-guard dans les pieds de page ; devise « Omnia Vincit Progressus Technologicus » uniquement au pied du login. Les API, permissions, SSE, stockage du jeton et données métier restent inchangés.
- `front/src/theme.css` : remplacement de l’ancien sombre olive par des couches neutres : fond `#0B0B0B`, panneau `#151515`, surface secondaire `#1F1F1F`, survol `#262626`, navigation `#101010`, texte ivoire `#F2F1E9`, traits `#373737`, contours de contrôle `#707070` et contours du dock `#6B6B6B`. Le citron reste réservé à la sélection et à Suivant. Les verdicts gardent leurs teintes discrètes et leurs libellés.
- `front/src/style.css` : adaptation sobre du mot-symbole OVPT et du séparateur, Arial Black/Arial/Helvetica déjà locales ; aucun téléchargement de police ; pied de page adapté ; couleurs du dock pilotées par tokens pour conserver la charte claire et neutraliser le sombre. Les positions du header/dock, la grille et les cibles restent inchangées.
- `front/public/favicon.svg` : monogramme OV blanc sur noir, tracés SVG natifs sans police ni raster, viewBox32×32, lisible à16/32px. `front/index.html` utilise `/favicon.svg?v=ovpt-20261030` et `sizes="any"` pour donner à Chrome une nouvelle URL de ressource.
- `front/tests/browser.mjs` : signature et nom du produit, nom accessible du logo, ressource favicon servie et rendu Chromium16/32, captures claires/sombres, contrôle des3couches noires neutres distinctes, et conservation des parcours/états/clavier/identités existants.

Références lues visuellement, laissées intactes :

1. `/Users/louis/Downloads/Logo OVPT minimaliste sur fond noir-1.png`
2. `/Users/louis/Downloads/Bannière typographique minimaliste OVPT-2.png`

La demande récente du mainteneur et les instructions transmises par root autorisent OVPT ; elles remplacent l’ancienne interdiction d’emprunt de marque. ai-guard reste le nom du produit, dans le titre du document, les exports et la signature.

## Commandes et résultats

Environnement : Node24.22.2, npm11.13.0, Vite7.3.7, TypeScript5.9.3, Playwright1.64.0-alpha-1790635538000, GoogleChrome156.0.8078.76. PATH Node `/Users/louis/.npm/_npx/387698761821791d/node_modules/node/bin` ; Playwright `/Users/louis/.npm/_npx/9833c18b2d85bc59/node_modules` ; Chromium `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`.

| Statut | Commande dans `front/` | Résultat | Journal |
|---|---|---|---|
| PASS |`npm test`|27tests,27PASS,0FAIL,0ignoré|`/tmp/ai-guard-ovpt-node.log`|
| PASS |`npm run build`|TypeScript puis bundle Vite statique,10modules|`/tmp/ai-guard-ovpt-build.log`|
| PASS |`CODEX_PRIMARY_RUNTIME_NODE_MODULES=… PLAYWRIGHT_CHROMIUM_EXECUTABLE=… npm run test:browser`|parcours principal et identité terminés sans exception navigateur|`/tmp/ai-guard-ovpt-browser.log`|
| FAIL attendu puis PASS |signature avant implémentation|0signature OVPT au lieu de1 puis signature présente|`/tmp/ai-guard-ovpt-red.log`|
| FAIL attendu puis PASS |nom accessible du logo|0logo nommé au lieu de1 puis nom OVPT/ai-guard exposé|`/tmp/ai-guard-ovpt-accessible-red.log`|

`npm ci` avait été exécuté et passé sur ce même lockfile lors du lot précédent ; aucun paquet, dépendance ni lockfile n’est modifié par ce lot. Les trois commandes qualifiantes demandées ci-dessus sont fraîches après le dernier changement applicatif.

Artefacts finaux : `front/dist/assets/index-CeMZdRmd.css` (30.73kB), `front/dist/assets/index-BtGEDMoP.js` (57.92kB), `front/dist/favicon.svg`, `front/dist/index.html`. Le backend de déploiement reste du ressort de root ; ce build n’en modifie aucun binaire.

## Critères acceptés

PASS : OVPT en header/footer avec ai-guard lisible ; devise au footer ; aucune police distante, image de référence modifiée ou dépendance supplémentaire. Favicon réellement chargé et rasterisé par Chromium à16/32px, sur fond clair/sombre. La référence est adaptée à l’icône, sans réduire quatre lettres illisibles.

PASS : surfaces sombres mesurées en RGB neutre et distinctes pour le fond, le panneau et l’en-tête du tableau. Captures de données, chargement, vide, erreur, péremption et tableaux ; focus clair dans le dock et bleu dans les données. Charte claire et navigation stable conservées.

PASS : logo, champ et bouton d’accès entièrement visibles au premier écran320×568,390×844 et844×390 ; preuve initiale prise àscrollY0 avant Tab ; H1≥36px ; le jeton mène directement au bouton au clavier. La signature mobile ne déplace pas l’accès et n’empiète pas sur les commandes.

PASS : neuf vues, cibles44px, géométrie1440/1280/390/320/paysage, commandes fixes après50workers, pages bornées100workers, détails et restitution de focus après SSE, filtres et export, XSS, mémoire seule, allocations exactes/nonfacturables, erreurs distinctes du vide, rafale200événements, refus403. Le test d’identité conserve A→B avecB500 et12réponses/fluxA tardifs ignorés, export tardif sans téléchargement et refusA403 tardif sans déconnexion deB.

## Captures inspectées

Captures sous `front/tests/artifacts/`. Le dernier run régénère les vues éditoriales et les nouvelles captures `ovpt-*`.

- `ovpt-favicon-16.png`, `ovpt-favicon-32.png`, `ovpt-favicon-light-dark.png` : rendus à taille native réellement ouverts.
- `ovpt-login-{320-568,390-844,844-390}-initial-{light,dark}.png` : première arrivée àscrollY0, avant Tab ; logo, champ et action visibles.
- `editorial-login-1440-light.png` : header/footer OVPT et devise ; `editorial-view-01-1440-light.png` : charte claire.
- `editorial-overview-1440-dark.png`, `ovpt-overview-{320-568,390-844,844-390,1280-720}-dark.png` : nouvelles couches sombres.
- `ovpt-workers-loading-{1440,320,390}-dark.png` : état de chargement.
- `editorial-overview-error-1440-dark.png`, `editorial-overview-empty-320-dark.png`, états données périmées et tableaux mobiles régénérés.
- `editorial-allocations-data-{320-568,390-844,844-390,1280-720}-dark.png` : relevé après défilement, mêmes unités exactes et provenance.

Les captures de cette liste ont été produites par le navigateur de test ; des vues représentatives ont été ouvertes avec `view_image` par l’implémenteur. La revue indépendante contrôle le gel actuel séparément.

## Limites et preuve de gel

NOT_RUN par cet agent : déploiement du front sur le backend stable, vérification de l’icône dans la barre d’onglets d’un profil Chrome utilisateur déjà doté d’un cache, qualification runtime/cloud et audit complet avec lecteur d’écran. L’URL de favicon versionnée est incluse et son chargement Chromium est testé. Les captures et parcours utilisent uniquement les fixtures HTTP du harness ; aucun chiffre ou événement de fixture n’entre dans le bundle produit.

Skills projet `.agents/skills/` et `.claude/skills/` absents du checkout : vérifications directes selon AGENTS.md et TEST_STRATEGY, sans prétendre avoir chargé `ai-guard-verify`. AGENTS.md et docs/ui.md relus. Références de contrats déjà lues lors du lot précédent, aucune frontière technique modifiée.

SHA256 des sources gelées :

- main.ts `d32121eb57c4d07b8109e10cda321199d8fc0be2fbd3b8e7ca5b6ee59862d6c5`
- theme.css `e0456ecac5a68e2196593df32e5732146bdd9037ac9bac170007d8b0db06ba6f`
- style.css `13263abef2cc144a688bbbfce7e9396ef1243e21ee474a3dcfe4d66b0d5ce11b`
- index.html `affaccf9ed260a3845d2b9512e028b7492a57f5240c7ff1adc73e85da53c4e1e`
- favicon.svg `51a233c71906900b8589ebef7537429dbb42cc4bcb631f942658dc8601893853`

## Delta de revue — dernier état désactivé

La revue indépendante et root ont confirmé que l’opacité du citron donnait à Suivant désactivé une teinte olive sur Allocations. Correction ciblée : fond `#262626`, texte/destination `#BFBFBF`, contour existant et opacité1. Les coordonnées, la taille, le vrai état HTML disabled et la destination « Fin des vues » restent inchangés. La régression de fond neutre a échoué sur le bundle précédent (`/tmp/ai-guard-ovpt-disabled-red.log`) avant correction. Aucun autre changement de périmètre.

Le reviewer a confirmé90couples9vues×5tailles×2thèmes et les états/détails/contrastes avant ce seul delta. Le contrôle indépendant final de l’état désactivé est rattaché au même rapport de revue.

PASS revue indépendante finale : `/tmp/ai-guard-ovpt-independent-review.md`. La matrice initiale couvre90couples et112scènes ; le delta disabled est relu et contrôlé dans6couples clair/sombre×1440/390/320. Rectangles actif→désactivé strictement identiques, vraie borne disabled et texte « Fin des vues » conservés, aucun débordement, olive disparu sur les captures finales ouvertes. Les couleurs sombres, le logo, l’accès320, les états, détails et contrastes ainsi que les rendus favicon16/32 ont été inspectés indépendamment. Aucun défaut bloquant frontend restant reproduit.

Une erreur de harnais favicon indépendante (chargement HTTP depuisabout:blank) est conservée par le reviewer ; son contrôle favicon seul a été repris en dataURI SVG local et passe. Notre suite complète HTTP Chromium16/32 passe également. Ce point n’a nécessité aucune mutation du produit.
