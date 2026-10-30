# Refonte éditoriale — front déployé en QA

30 octobre2026. Le guide détaillé du mainteneur a été transposé dans l'IHM ai-guard : header clair, navigation sombre fixe, neuf vues numérotées, grands titres Arial/Helvetica, repères Courier New, fond `#FAFAF8`, texte `#101010`, citron `#E2F471` pour Suivant et la sélection, traits fins et panneaux sobres. Marque, login, navigation et surfaces remplacent effectivement l'ancienne présentation ; l'ajout à la gouvernance n'est plus la seule réalisation.

Le formulaire et son bouton sont visibles dès l'arrivée à320×568,390×844 et en paysage. Suivant annonce sa destination et conserve sa position, les autres commandes restent accessibles, et les tableaux défilent localement. Les états inconnus, chargement, vide, erreur, périmé, interdit et les allocations non facturables gardent leur signification. Le français est conservé ; aucun sélecteur anglais sans traduction n'a été ajouté.

## Preuves

| Vérification | Résultat |
|---|---|
| `npm ci` / Node24.22.2 | PASS,15packages, audit npm0vulnérabilité signalée |
| `npm test` | PASS27/27 |
| `npm run build` | PASS TypeScript/Vite ; assets `index-De0rVJnH.css` et `index-g65sqHFT.js` |
| `npm run test:browser` | PASS, parcours frontend et identité A→B ; fixtures uniquement dans les tests |
| Revue indépendante |45couples9vues×5tailles, contrastes5scènes, clavier, modal,500événements et IDs longs PASS après delta Suivant |
| Dernier correctif login | PASS ciblé indépendant sur4tailles ; scrollY0, marque/champ/CTA visibles avant interaction et ordre H1→form→faits |
| Navigateur sur services Rust réels | PASS10 contrôles, Chrome156.0.8078.76, mode partagé ; `browser-real.log` |
| Captures QA réelles | PASS, ouvertes par root ; `live-capture-result.json` et quatre PNG ci-dessous |

Les [rapports implémentation](frontend-report.md) et [revue indépendante](independent-review.md) distinguent les gels successifs et les défauts traités : hiérarchie Suivant/Export, accès initial au formulaire mobile et artefacts de captures après redimensionnement. Les captures « first-screen » après focus ne remplacent pas les mesures avant interaction. Les captures finales réelles attendent deux frames après changement de taille ; le brouillon de tuilage est conservé dans `/tmp`, hors jeu retenu.

Les tests préservent XSS rendu comme texte, jeton en mémoire seule, absence d'écritures viewer, SSE/reconnexion/curseur, filtres et export, pagination/focus et12réponses ou flux A tardifs ignorés sousB. Aucun mécanisme API/auth/SSE n'a été remplacé pour obtenir le rendu. Les contrastes mesurés et le clavier ne sont pas un audit WCAG complet.

## Rendu réellement servi

- [Vue d'ensemble desktop](overview-desktop-real.png).
- [Workers desktop](workers-desktop-real.png).
- [Workers mobile](workers-mobile-real.png).
- [Login mobile320px, avant focus](login-mobile-real.png).

Les captures lisent les vrais services QA et leurs événements/workers durables, sans données de démonstration ajoutées dans le produit. Le harness navigateur injecte aussi deux événements **QA déclarés** pour son propre scénario ; leur provenance reste distincte des observations runtime. Le worker du parcours navigateur `884223dc-2617-46b7-9717-c1e075015dab` est une observation réelle antérieure conservée.

Image déployée `agentsandbox-control:qa-20261030-editorial-fs-6f4d8b02`, ID `sha256:d8179e589f7104072f2b237860d809a254acaf474a7593b380f0a91cdf52ca7f`, Linux ARM64. Trois services healthy, projet `ai-guard-qa-7dce8f12`, Compose courant `/tmp/ai-guard-registry-qa.40IImn/compose-editorial-fs.yaml`, dashboard `http://127.0.0.1:48400`, gateway `http://127.0.0.1:48787`. Le dashboard reste Rust, statique, partagé et sans montage de secret. Les volumes et anciennes images sont conservés. L'image intermédiaire avant correction login n'a pas été déployée.

Audit avant/après remplacement identique :512événements et head `5a13a49af8594eb38ef6c64c7e21106b174d352d66620eef869388f02078d13c`. Après nouveaux essais :589événements, head `a1a2d354f4d9a712d4792474b39abf583e6c0882e56e3ad25ba9752150a71bb4`, valide. Les JSON et logs de déploiement sont conservés ici.

Le même artefact intègre le [lot filesystem](../control-filesystem/README.md) :302tests Rust/0FAIL/3ignorés, fmt/Clippy/release PASS. Docker réel en release : confinement11,84s, refus d'audit terminal10,02s et smoke déployé42,04s PASS. Le smoke conserve source/export/volumes, vérifie refus, reprise idempotente et masquage d'une valeur synthétique ; aucun compte ou appel fournisseur réel. Ces tests restent distincts du test navigateur.

## Portée et limites

`SOURCE_SHA256SUMS.txt` identifie les sources/configurations/tests du candidat commun, sans inclure les PNG générés. Les jeux de captures et les logs ont leur propre provenance ci-dessus. Pas de nouveau service Node en production, dépendance frontend ou police distante. Aucune publication cloud ou Git n'a été réalisée.

NOT_RUN : Safari/Firefox, lecteur d'écran et appareils physiques. À320/390px certains libellés de raccourcis sont tronqués visuellement ; les noms accessibles complets, l'état courant et le focus restent vérifiés. Le dock mobile réserve314px conformément aux dimensions retenues ; la version paysage est compactée. La traduction anglaise n'est pas livrée. Qualification3workers multi-tenant,100workers, Podman/Kubernetes et production globale restent ouvertes ; ce lot ne les transforme pas en succès.
