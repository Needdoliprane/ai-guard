# Signature OVPT et thème sombre — QA locale du 30 octobre 2026

**PASS pour le lot visuel livré. Aucune qualification production globale.** Le mainteneur a fourni les deux références OVPT et demandé le logo, le favicon Chrome et la reprise du sombre. Les originaux dans Downloads n'ont pas été altérés. OVPT devient la signature ; ai-guard reste le produit.

Le header utilise le mot-symbole massif, un séparateur et ai-guard. Le favicon est une déclinaison vectorielle **OV**, sans police, lisible à16/32px, avec URL versionnée pour renouveler la ressource en cache. Le sombre utilise fond `#0B0B0B`, panneaux `#151515`, surfaces secondaires `#1F1F1F`, texte ivoire `#F2F1E9`, traits gris et accent citron rare. Le bouton Suivant désactivé est gris neutre : son ancienne opacité produisait encore un rectangle olive, corrigé après revue.

## Artefact réellement servi

- Dashboard local : <http://127.0.0.1:48400> ; gateway : <http://127.0.0.1:48787>.
- Seul le dashboard a été recréé, avec l'image `agentsandbox-control:qa-20261030-ovpt-ui-final-5b92c7`, ID `sha256:c841cca6a5e8789a2d0fcf41932193ae1e130e214a4e08ed171ba847b314564a`. Service sain, `mounts=[]` ; aucun secret monté.
- Overlay frontend sur le backend déjà validé `agentsandbox-control:qa-20261030-editorial-fs-6f4d8b02`, ID `sha256:d8179e589f7104072f2b237860d809a254acaf474a7593b380f0a91cdf52ca7f`. Gateway et ingress conservent cette image. Les nouveaux changements SQLite du checkout **ne font pas partie de cet artefact**.
- Binaire Rust dans les deux images : SHA256 `a200e714e55e0e7eac5ee6a110b242e7f4c85e913b6383fe647b9dadd3903959`, égalité vérifiée par `sha256sum` et `cmp` dans des conteneurs sans réseau ni montage. [Dockerfile QA](qa-overlay.Dockerfile), [build](image-build.log).
- Bundle final : `index-CeMZdRmd.css`, `index-BtGEDMoP.js`. [Empreintes](source.sha256). Source frontend gelée : `/tmp/ai-guard-ovpt-final.c58I6z/front-source.tar`, SHA256 `6cc6bcccfdacf5f5f0dfec81b7fa65841c6afd54cb652eee7dca7cd5de01d685`.
- Compose QA : `/tmp/ai-guard-registry-qa.40IImn/compose-ovpt-ui.yaml`, projet `ai-guard-qa-7dce8f12`. Ce gel local et son tag de base ne sont pas une distribution publique reproductible hors de ce poste. Le build produit complet reste `images/control.Dockerfile` et exige ses propres preuves.

## Commandes et résultats

| Statut | Contrôle exécuté | Preuve et portée |
|---|---|---|
| PASS | `npm test` sous Node24.22.2 |27 tests,0échec,0ignoré ; [journal](ai-guard-ovpt-node.log) |
| PASS | `npm run build` | TypeScript et bundle statique ; [journal](ai-guard-ovpt-build.log) |
| PASS | `npm run test:browser` avec Playwright et Chrome156.0.8078.76 | Connexion, XSS, SSE, identité A→B, refus, export, pagination, données denses, clavier et thèmes ; [journal final](ai-guard-ovpt-browser.log), [rapport](implementation.md) |
| PASS | Revue indépendante |90 couples vue/thème/viewport et112 scènes de contraste sur le premier gel ; défaut disabled corrigé, puis6 contrôles ciblés sur le delta final ; [revue](review.md) |
| PASS | Overlay Docker, `compose up --detach --no-deps --wait dashboard` | Services sains, ancien backend conservé, aucune migration ni modification du volume |
| PASS | `node front/tests/browser_live.mjs` avec adresses QA/configuration explicites |10 contrôles sur Rust et HTTP réels ; [résultat](browser-live.json). Deux événements QA déclarés590/591, aucun lancement de worker ou appel fournisseur par ce test |
| PASS | `node live-captures.mjs` sur le déploiement |10 captures réelles, deux thèmes, login320×568 àscrollY0 avec CTA entier, desktop1440, mobile390, paysage844 ; favicon HTTP200 identique au build ; [résultat](live-capture-result.json) |
| PASS | Audit avant/après déploiement |589 événements et tête `a1a2d354f4d9a712d4792474b39abf583e6c0882e56e3ad25ba9752150a71bb4` identiques ; [avant](audit-before.json), [après](audit-after-deploy.json) |
| PASS | Audit après navigateur |591 événements, chaîne valide ; augmentation limitée aux deux événements déclarés du harnais ; [résultat](audit-after-browser.json) |

Captures ouvertes par l'orchestrateur : [vue d'ensemble sombre réelle](overview-desktop-dark-real.png), [workers mobile sombre réels](workers-mobile-dark-real.png), ainsi que les captures login et la planche de favicon du harnais. Les [icônes16/32](favicon-16-32.png) sont rendues par Chrome ; cette planche ne contient aucune donnée métier. Les captures `*-real.png` proviennent des données durables de QA, pas des fixtures HTTP.

## Limites

`NOT_RUN` : profil Chrome utilisateur déjà ouvert et son cache ; recharger la page charge la nouvelle URL du favicon. Les contrôles headless et de contraste ne constituent pas un audit complet avec lecteur d'écran. Ce lot ne qualifie ni Podman, ni Kubernetes, ni le cloud, ni de nouvelles garanties runtime. Le dashboard reste en lecture seule et ne masque aucun écart fonctionnel du produit. Les corrections SQLite font l'objet d'un lot distinct, encore non déployé à ce stade ; leurs tests ne sont pas attribués à ce backend ancien.

L'itération Docker initiale `qa-20261030-ovpt-ui-4e53a9` a été conservée et n'a jamais été déployée ; elle précède le correctif du bouton disabled. Les rapports distinguent ce gel et le delta final.
