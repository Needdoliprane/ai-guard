# Revue indépendante du front éditorial ai-guard

Date : 30 octobre 2026. Périmètre : UX, rendu, adaptation, clavier et maintien des contrats de lecture du frontend. Aucun fichier applicatif ou test modifié par le reviewer.

## Contrats lus

- `AGENTS.md` et `/Users/louis/.codex/RTK.md`.
- `docs/ui.md`, version contenant le contrat visuel du 30 octobre 2026.
- `docs/engineering/{ARCHITECTURE_CONTRACT,CODING_RULES,PRODUCTION_READINESS,TEST_STRATEGY}.md`.
- `.agents/skills/` absent du checkout : aucune prétention d'application d'un skill projet absent.

## Baseline observée

La capture `front/tests/artifacts/overview-light.png` a été ouverte avec `view_image` avant le gel des nouveaux changements. Elle montre la barre latérale claire, l'ancienne marque « Agent Sandbox », des cartes arrondies répétées et les accents turquoise. Elle corrobore l'écart visuel décrit par le mainteneur ; cette ancienne capture n'est pas une preuve du nouveau rendu.

Lecture du code existant : le client conserve le jeton dans un champ privé en mémoire, impose les routes de lecture de même origine et les requêtes GET avec Authorization, `credentials: omit`, `cache: no-store`, `redirect: error`. Les retours tardifs des lectures principales, événements, contrôle et détails sont protégés contre un changement d'identité. Le scénario navigateur `browser_identity.mjs` contient déjà des réponses/flux A retenus puis libérés sous B et une vérification de l'export tardif. Ces constats de code ne sont pas encore une preuve d'exécution après refonte.

## Premier gel visuel — preuves avant delta CTA Suivant

PASS : les nouvelles captures suivantes ont été ouvertes avec `view_image`, et non seulement déduites des constantes CSS : `editorial-login-1440-light.png`, `editorial-overview-1280-720-light.png`, `editorial-overview-320-568-light.png`, `editorial-overview-844-390-light.png`, `editorial-workers-1280-720-light.png`, `editorial-workers-stale-1440-light.png`, `editorial-allocations-390-844-light.png` et `editorial-overview-error-1440-dark.png`. La marque ai-guard, la composition en lignes, la nav sombre numérotée, les grands titres, les surfaces neutres, le citron rare et les états métier sont effectivement visibles. Les captures login320 et workers-loading320 du premier procédé ont été exclues après signalement d'un artefact de tuilage fullPage.

PASS : vérification indépendante `/tmp/ai-guard-editorial-review-run.mjs`, construite à partir du serveur de fixtures frontend existant et d'un corps de contrôle séparé dans `/tmp`. Node 24.22.2 et Google Chrome local headless, sans changement d'application ni de test committable. Les 9 vues ont été ouvertes à 1440×1050, 1280×720, 390×844, 320×568 et 844×390, soit 45 couples : aucun débordement global, titre sous les zones fixes, focus H1 après navigation. Connexion 320/390/paysage, dialogue worker390, 500 événements et longs identifiants sans débordement global. Résultats : `/tmp/ai-guard-editorial-review/geometry.json`.

PASS : contrôle complémentaire `/tmp/ai-guard-editorial-focus-run.mjs`. Aucun contraste de texte visible inférieur aux seuils 4,5:1 normal / 3:1 grand dans le login clair, overview et workers clair/sombre. Le défilement horizontal de la table workers répond aux flèches au clavier (`scrollLeft` 647→676). Tab puis Shift+Tab placent Page suivante et Détails à y430–474 sous le dock fixe de hauteur314 ; le focus de Détails porte un contour de2px `#a6a6ff`. Le focus de l'événement250/500 reste visible sous le dock à320px. Résultats : `/tmp/ai-guard-editorial-review/accessibility.json`. Captures de contrôle réellement ouvertes : `dense-1440.png`, `dense-320.png`, `dense-midpoint-320.png`, `worker-dialog-390.png`, `worker-keyboard-scroll-320.png` dans ce même dossier.

Les fichiers `api.ts` et `sse.ts` ont des empreintes SHA256 identiques avant/après ce gel. Le scénario A→B avec réponses tardives reste dans les tests navigateur de l'implémenteur ; il ne suffit pas de conserver le code pour compter son exécution indépendante ici.

## Gel final après delta CTA Suivant

PASS : le delta demandé par l'agent principal est réalisé visuellement. Suivant est l'action principale citron avec destination secondaire et flèche ; Précédent est une action outline ; Actualiser et Export sont secondaires. Les9 captures finales `editorial-view-01-1440-light.png` à `editorial-view-09-1440-light.png` ont toutes été ouvertes avec `view_image`. Ont également été ouvertes après remplacement les captures `editorial-overview-390-844-light.png`, `editorial-overview-844-390-dark.png` et `editorial-login-320-light.png`. Aucun artefact de tuilage n'est présent dans ces nouvelles captures viewport.

PASS : les45 couples vue/viewport, les3 formats de connexion, le dialogue390, les500 événements et les longs identifiants ont été réexécutés sur le build du gel final. Les fichiers `/tmp/ai-guard-editorial-review/{geometry,accessibility}.json` portent les résultats du gel final. Le calcul des rectangles d'Actualiser confirme des coordonnées exactement identiques entre les9 vues pour chacun des5 viewports, y compris quand l'export disparaît des vues Workers/Quotas/Allocations. Les contrastes des5 scènes, le défilement horizontal aux flèches et le parcours natif Tab/Shift+Tab ont aussi été réexécutés avec succès après le delta.

PASS : l'ordre DOM du login est H1 et introduction → section d'accès et formulaire → faits sur identité/périmètre/preuves. Le placement CSS de ces faits à gauche sur desktop ne déplace pas le formulaire avant son titre dans l'ordre accessible. Le rendu320 montre bien le titre avant la section d'accès et les faits sont désormais après le formulaire.

PASS : exécution indépendante finale de `node tests/browser_identity.mjs` dans `front/`, avec Node24.22.2, Playwright et Google Chrome local. Résultat constaté : identité viewer validée avant données ; indisponibilité, identité invalide et rôle runner refusés ; A→B avec B500 ;12 réponses/flux A tardifs ignorés ; pagination, détail, export et A403 tardif ; péremption limitée au même client. L'API/SSE conservent leurs empreintes ; cette exécution confirme les gardes du nouveau shell.

FAIL : aucun défaut bloquant reproduit sur le gel final dans ce périmètre.

BLOCKED : aucun blocage du contrôle frontend demandé. Les qualifications produit/runtime/cloud restent hors de cette revue.

## Limites observées du rendu

À320/390px, plusieurs libellés de raccourcis nav sont visuellement tronqués pour garder les9 destinations et les cibles44px dans la grille. Leurs noms accessibles et titres restent complets ; Précédent/Suivant annoncent la destination réelle. Le dock fixe mobile laisse moins de place au contenu (314px réservés sur568px), mais le défilement du contenu et le focus clavier ont été mesurés visibles sous le dock. Cela n'a pas produit de défaut bloquant dans les parcours vérifiés.

## Suivi : accès immédiat à la connexion mobile

FAIL reproduit par inspection de la capture `editorial-login-320-light.png` du gel précédent et signalé par la revue principale : à320×568, ni champ de jeton ni CTA d'entrée n'est visible dans le premier viewport. Le titre et l'introduction occupent cet espace. La revue précédente mesurait l'ordre DOM et le débordement sans appliquer ce critère de visibilité immédiate.

PASS après correction : le formulaire est placé immédiatement après H1 dans l'ordre accessible, l'introduction et les faits sont déplacés ensuite. Les captures finales `editorial-login-{320-568,390-844,844-390}-first-screen.png` ont toutes été ouvertes. Le champ et le bouton sont entièrement visibles dans chacun des3 premiers viewports, sans réduire les titres.

PASS indépendant sur le build après ce dernier delta : `/tmp/ai-guard-editorial-login-final-run.mjs`, Node24.22.2/Chrome local. Mesures avant toute action : à320×568, champy360,19–410,19 et CTAy485,78–535,78 ; à390×844, champy360,19–410,19 et CTAy464,98–514,98 ; à844×390, champy162,69–212,69 et CTAy267,48–317,48 ; à1440×1050, champy308,73–358,73 et CTAy413,53–463,53. Titre36px minimum, aucun débordement global, ordre DOM H1→formulaire→faits, Tab du champ vers le bouton, refus avec jeton vidé puis authentification viewer acceptée : tous PASS. JSON et captures indépendants dans `/tmp/ai-guard-editorial-review/login-final*` ; la capture indépendante320×568 a été ouverte avec `view_image`.

Le défaut d'accès immédiat est fermé. Les45 contrôles shell précédents restent datés du gel antérieur à cette dernière correction limitée au login ; ils ne sont pas présentés comme une nouvelle exécution du shell après ce delta.

## Preuve initiale distincte de la preuve clavier

PASS : contrôle supplémentaire de preuve uniquement, sans changement d'application ni de test du dépôt. `/tmp/ai-guard-editorial-login-initial-run.mjs` recharge chaque viewport puis capture après2 repaints et avant tout focus/Tab. À320×568 : `scrollY === 0`, élément actif BODY, marqueai-guard y30–59, champ y360,19–410,19, CTAy485,78–535,78. Les vérifications initiales390×844 et844×390 donnent aussi `scrollY === 0`, marque/champ/CTA entièrement visibles et BODY actif.

Les3 nouvelles captures `front/tests/artifacts/editorial-login-{320-568,390-844,844-390}-initial-unfocused.png` ont été ouvertes et montrent la marque et le CTA dans le même premier viewport. Elles constituent la preuve du rendu initial ; les captures précédentes avec contour focus constituent la preuve clavier distincte. Mesures : `/tmp/ai-guard-editorial-review/login-initial-unfocused.json`. Aucun nouveau défaut applicatif constaté.

## Limites

Les captures et fixtures frontend qualifient le frontend dans leur périmètre ; elles ne qualifient ni Docker/Podman, ni le cloud, ni une release production, ni un audit WCAG complet avec lecteur d'écran. NOT_RUN dans cette revue : Safari/Firefox, lecteur d'écran et services runtime réels. Ces essais n'ont pas été transformés en PASS à partir des fixtures.
