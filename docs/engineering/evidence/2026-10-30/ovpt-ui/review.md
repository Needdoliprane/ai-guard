# Revue indépendante — OVPT et thème sombre ai-guard

30 octobre 2026. Résultat : **PASS dans le périmètre frontend vérifié ; aucun défaut bloquant restant.** Aucun fichier applicatif ou test du dépôt modifié par le reviewer. Les scripts et captures indépendants restent dans `/tmp/ai-guard-ovpt-review*`.

## Contrat et références

Les nouvelles consignes `AGENTS.md` remplacent les précédentes : OVPT devient la signature visuelle et ai-guard reste le produit. Le complément `docs/ui.md` impose une adaptation des deux visuels du mainteneur, le favicon lisible16/32px et un sombre composé de noirs neutres, de surfaces distinctes et de texte ivoire. Les contrats architecture/code/gates/tests avaient été lus lors de la revue éditoriale et sont conservés dans le périmètre de cette revue.

Les deux images originales ont été ouvertes sans altération : `/Users/louis/Downloads/Logo OVPT minimaliste sur fond noir-1.png` et `/Users/louis/Downloads/Bannière typographique minimaliste OVPT-2.png`. L'adaptation livrée conserve les lettres massives et nettes, le séparateur et le nom ai-guard. Aucune attribution juridique ou URL externe n'en est déduite.

## Vérification indépendante du gel OVPT

Environnement exécuté : Node 24.22.2, Playwright 1.64.0-alpha-1790635538000, Google Chrome 156.0.8078.76, macOS local headless. Le build statique `front/dist` est servi par le serveur de fixtures isolé existant, repris dans le script temporaire ; les fixtures ne sont pas ajoutées à l'application.

Commande applicative : Node exécute `/tmp/ai-guard-ovpt-review-run.mjs`, avec le répertoire Playwright et le binaire Chrome explicitement sélectionnés. Une seule matrice complète a été exécutée après le premier gel OVPT.

| Critère | Résultat constaté |
|---|---|
|9 vues×2 thèmes×5 tailles | PASS :90 couples1440×1050,1280×720,390×844,320×568,844×390 ; aucun débordement global, H1 sous les commandes fixes, focus H1 après navigation,9 destinations et1 sélection active |
| Connexion initiale | PASS :8 couples clair/sombre×1440/390/320/paysage, scrollY0, marque/champ/CTA entièrement visibles avant interaction, titres≥36px, ordre H1→formulaire→faits, Tab du jeton au CTA |
| Identité visuelle | PASS : OVPT et ai-guard présents ; signature et produit restent lisibles sur mobile ; nom accessible « OVPT · ai-guard · Lecture seule » vérifié dans le navigateur |
| Contrastes textuels | PASS :112 scènes sans valeur sous4,5:1 pour le texte normal ou3:1 pour les grands caractères, hors états désactivés exemptés |
| Sombre réel | PASS : fonds noirs neutres, panneau et en-tête de tableau distincts, texte ivoire ; boutons secondaires actifs, champs, séparateurs et lignes de rangée examinés sur captures |
| États et détails | PASS : chargement, vide, erreur HTTP 503, données périmées conservées, détail worker390 et détail événement, dans les deux thèmes ; fermeture Échap et retour du focus worker |
| Densité | PASS :50 workers et500 événements avec identifiants longs ; pas de débordement global320 ; focus clavier d'un événement central visible sous le dock |
| Commandes | PASS : coordonnées d'Actualiser identiques entre les9 vues pour chaque couple thème/viewport |
| Exceptions navigateur | PASS : aucune exception applicative enregistrée |

Données détaillées : `/tmp/ai-guard-ovpt-review/results.json`. Les API/SSE/contrôle ne changent pas : empreintes `api.ts` `dd6e69d…`, `sse.ts` `a6b34c5…`, `control-view.ts` `03f087c…`, identiques à la revue précédente.

## Rendu effectivement inspecté

Les captures indépendantes ont été ouvertes avec `view_image`, notamment : login320clair/sombre ; overview1440clair/sombre ; overview et allocations sombres paysage ; workers et allocations sombres desktop ; détail worker390 ; détail événement ; chargement, vide, péremption et erreur sombres ; journal dense1440 ; puis les captures finales du delta ci-dessous. Les conclusions visuelles ne se fondent donc pas sur les seules constantes CSS.

Les captures se trouvent dans `/tmp/ai-guard-ovpt-review/`. Les motifs et inconnues demeurent visibles ; les états d'erreur ne sont pas rendus comme une absence d'activité. Les bordures actives renforcées `#707070` et `#6B6B6B` sont lisibles dans le rendu sombre. Les traits de structure plus fins restent volontairement discrets, sans empêcher la lecture des rangées et des panneaux examinés.

## Favicon

PASS : le SVG natif local, avec titre « OVPT · ai-guard », a été rendu à16 et 32 px sur surfaces claire et sombre ; la déclinaison OV reste identifiable. La planche `/tmp/ai-guard-ovpt-review/favicon-16-32-light-dark.png` a été ouverte. Elle inclut les tailles natives et des agrandissements signalés pour inspection. Mesures : `favicon.json` dans le même dossier.

Le premier générateur de planche, qui chargeait les images HTTP loopback depuis `about:blank`, a expiré après la réussite des contrôles applicatifs. La commande complète a donc rendu un code 1 pour cette erreur de harnais, conservée dans `failure.json` et expliquée dans `results.json`. La seule preuve favicon a été refaite à partir du contenu SVG local encodé en dataURI : chargement et capture PASS. Aucun correctif applicatif n'a été demandé pour cette erreur de harnais, et la matrice n'a pas été répétée.

## Défaut relevé puis fermé : Suivant désactivé olive

FAIL au premier gel OVPT : dans Allocations, l'opacité générale 0,45 transforme le citron du bouton Suivant désactivé en olive. Le défaut est visible dans les captures indépendantes `view-dark-9-1440x1050.png` et `view-dark-9-320x568.png`. Le reviewer et l'implémenteur l'ont signalé avant le déploiement.

PASS après le delta autorisé par l'agent principal : seules2 règles CSS ciblant `.view-next:disabled` et sa destination ajoutent fond neutre `#262626`, texte `#BFBFBF`, bordure existante et opacité1. Ni taille ni comportement HTML ne changent. Contrôle indépendant `/tmp/ai-guard-ovpt-targeted-run.mjs` :6 couples clair/sombre×1440/390/320 PASS, rectangles strictement identiques actif→désactivé, état disabled et mention « Fin des vues » conservés, aucun débordement global. Les captures finales `disabled-final-dark-390.png`, `disabled-final-dark-1440.png` et `disabled-final-light-320.png` ont été ouvertes ; l'olive a disparu. Résultats : `targeted-final.json`.

Les90 couples et112 scènes restent la preuve du gel précédant ces2 règles ; le gel final est couvert par cette relecture CSS et le contrôle ciblé, sans répétition inutile de toute la matrice. SHA256 final `style.css` : `13263abef2cc144a688bbbfce7e9396ef1243e21ee474a3dcfe4d66b0d5ce11b`.

## Limites et blocages

BLOCKED : aucun blocage frontend restant dans ce périmètre.

NOT_RUN par ce reviewer : services runtime réels, déploiement conteneurisé, Safari/Firefox, lecteur d'écran, audit WCAG complet. Les tests API/A→B complets post-delta sont rapportés PASS par l'implémenteur ; ils ne sont pas présentés comme une nouvelle exécution indépendante ici. Les fixtures et captures ne qualifient ni Docker/Podman, ni le cloud, ni une release production.

À320/390px, certains raccourcis de navigation restent tronqués visuellement, avec noms accessibles complets et destinations Précédent/Suivant explicites. Le dock réserve314 px sur écran mobile568 px ; le titre, le défilement et le focus ont été contrôlés sous cette zone. Aucun nouveau défaut bloquant n'en a été reproduit.
