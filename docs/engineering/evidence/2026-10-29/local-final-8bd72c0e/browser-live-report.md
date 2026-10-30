# Parcours navigateur réel — résultats et portée

Date : 2026-10-29. Périmètre exclusif : nouveau `front/tests/browser_live.mjs`, script `test:browser:live` dans `front/package.json`. Aucun changement de dépendance, lockfile ou source UI. AGENTS.md, RTK.md et les quatre contrats engineering requis ont été lus ; les skills projet `.agents/skills`/`.claude/skills` restent absents, chargement automatique non confirmé. Revue indépendante `review_front` : constats initiaux de garde réseau et captures traités, puis correctifs transport et comparaison d'export relus ; aucun nouveau bloquant identifié sur la version finale `20c013fe...`.

Revue finale conservée : `/tmp/ai-guard-review-browser-live.md` (code final et neuf contrôles live relus, empreinte d'export recalculée).

## Statut d'exécution

**PASS — parcours réel exécuté, code de sortie 0.** Preuve finale : `/tmp/ai-guard-browser-live-real-20261029-8bd72c0e-after-export.log`, neuf contrôles réussis, zéro violation réseau, contexte et navigateur fermés. Gateway `http://127.0.0.1:48787`, dashboard `http://127.0.0.1:48400`, configuration QA explicite `/tmp/ai-guard-control-qa.mF6i7C/config`. Les checks HTTP, UI, SSE, reconnexion, export, mémoire seule et déconnexion ci-dessous ont été exécutés contre les services réels. Aucun serveur de fixtures n'a remplacé ces services.

Métadonnées du candidat fournies et vérifiées côté contrôle par l'orchestrateur : projet Docker `ai-guard-qa-7dce8f12`, image `agentsandbox-control:qa-20261029-8bd72c0e`, ID `sha256:70b34787cc12aa59e6558c1e391329bffa0acc0c6bc83363737a46589a02414e`, ingress/gateway/dashboard sains, seuls bindings hôte annoncés `127.0.0.1:48787` et `127.0.0.1:48400`. Le test navigateur a vérifié directement les services HTTP et l'identité du HTML/bundle servis avec `front/dist`, sans modifier Docker ni ouvrir l'egress des services.

**Historique BLOCKED conservé :** avant l'accès hôte, les deux origines ont refusé la connexion ; l'orchestrateur a confirmé des services sains mais non publiés sur un réseau exclusivement interne. Aucun sondage supplémentaire n'a été fait pendant l'attente, ni contournement par fixture. Le mainteneur a ensuite autorisé un ingress Rust distinct ; l'orchestrateur a fourni les origines accessibles avant lancement réel. Ce blocage historique est levé pour ce parcours précis et ne doit pas être effacé des preuves précédentes.

Deux échecs du **harnais**, reproduits et corrigés avant le PASS final :

| Preuve conservée | Constat | Correction vérifiée |
| --- | --- | --- |
| `...real-20261029-8bd72c0e.log` | FAIL à la reconnexion : `setOffline(true)` seul laisse un fetch SSE déjà ouvert vivant sur ce Chrome | Coupure explicite `Page.stopLoading` après offline ; probe loopback `sse-stop-probe.log` : état open → error ; CDP ignore seulement la continuation d'une réponse réseau déjà échouée |
| `...real-20261029-8bd72c0e-after-transport.log` | Reconnexion réelle PASS, puis FAIL au hash de comparaison d'export | `export-order-before.log` démontre des données identiques mais des ordres de clés différents entre listing JSON et JSONL ; comparaison profonde stricte des objets, sans dump de contenu |

Les trois lancements ont ajouté **cinq événements QA déclarés** dans trois projets UUID distincts (séquences 32 à 36), sans supprimer les traces des tentatives précédentes. Le run final porte sur le projet `qa-browser-774215b6-1fcd-475d-bf28-b3a07503ab06`, événements `0805f54f-60f0-4ab7-b65e-88ddac291d53` et `2f719071-d41d-4215-939a-43a88c0e42f7`, séquences 35/36. Source `qa.browser.live`, provenance `declared`, couverture `generic` ; aucun effet runtime ou fournisseur attesté. Le smoke concurrent de l'orchestrateur utilise d'autres projets ; l'export du test reste filtré sur son UUID propre.

## Contrôles implémentés

- Trois paramètres obligatoires et sans défaut implicite : `AI_GUARD_TEST_GATEWAY`, `AI_GUARD_TEST_DASHBOARD`, `AI_GUARD_TEST_CONFIG`. Origines loopback HTTP(S) seulement, sans credentials, chemin, query ni fragment. Credentials lus dans `secrets/viewer` et `secrets/runner` de la configuration explicite, non vides et distincts.
- Vrais endpoints `/healthz` avec nom de service attendu, en-têtes CSP/no-store/nosniff/DENY, HTML et JS compilé servis par le dashboard égaux au build local `front/dist`. Aucun serveur fake ni interception de réponses.
- Refus anonymes/invalide sur gateway et dashboard, runner refusé au dashboard, viewer accepté en lecture, POST viewer gateway refusé 403, POST proxy refusé 405, absence d'écriture après refus.
- Connexion UI invalide puis viewer valide, champ secret vidé, aucun appel authentifié avant connexion, jeton seulement en mémoire (localStorage/sessionStorage/IndexedDB/cookies vides). Le runner n'est jamais fourni au navigateur, seulement au fetch Node de contrôle autorisé.
- Deux événements QA uniques : source `qa.browser.live`, provenance `declared`, couverture `generic`, projet/session UUID dédiés. Ces événements n'attestent aucun effet runtime ou fournisseur. Le premier vérifie réception SSE sans nouvelle lecture d'historique et rendu d'une chaîne HTML malveillante comme texte.
- Coupure réseau limitée au contexte Playwright (`setOffline` + arrêt des chargements Chromium existants), second événement injecté par la vraie gateway, reconnexion avec Last-Event-ID puis rattrapage réel sans lecture d'historique et sans doublon. Cette interruption simule uniquement la liaison du navigateur, pas une panne de gateway ou de moteur.
- Export JSONL téléchargé puis comparé à la liste filtrée de la gateway (deux événements QA exactement), déconnexion et actualisation sans session restaurée, aucune erreur JavaScript, aucun secret dans les URL et aucun runner dans les requêtes navigateur.
- Garde installé avant toute page : seule l'origine exacte du dashboard et GET sont autorisés, credentials URL bruts/encodés refusés avant envoi, service workers bloqués, WebSockets fermées sans connexion réseau. Une seule page et son frame principal sont admis ; popups, iframes et requêtes sans frame sont refusés. CDP intercepte les réponses et refuse tout 3xx avant suivi, sans tamponner le corps SSE. Le parcours est bloqué si cette capacité Chromium manque. Tout essai interdit, même pendant la fermeture, fait échouer le résultat ; aucun chemin/URL/header interdit n'est enregistré dans les diagnostics.
- Fermeture du contexte et de Chromium dans finally, y compris après erreur ; un échec de nettoyage transforme le résultat en FAIL. **Aucun screenshot live**, même positif. Avant conservation explicite de l'export, son contenu est vérifié sans credentials bruts/encodés puis comparé aux deux événements QA déclarés. Seul cet export validé est prévu dans un répertoire temporaire OS neuf hors dépôt, avec chemin dans le résultat JSON.
- Sortie JSON expurgée : PASS, FAIL ou BLOCKED (code 2 pour prérequis manquants). Tokens bruts et encodés sont masqués avant affichage. Aucun HAR ni trace contenant des headers d'authentification n'est enregistré.

## Commandes et résultats observés

Node isolé **v24.22.2**, npm **11.13.0**, macOS ARM64.

| Statut | Vérification | Résultat |
| --- | --- | --- |
| PASS | `node --check tests/browser_live.mjs` | code 0 |
| PASS | `npm test` après dernière modification | 17/17, zéro échec, zéro ignoré ; `/tmp/ai-guard-browser-live-unit-after-export.log` |
| PASS | invocation sans les trois paramètres | BLOCKED code 2, indique le paramètre gateway manquant |
| PASS | origine non loopback explicitement fournie | BLOCKED code 2 avant tout accès réseau |
| PASS | répertoire credentials inexistant | BLOCKED code 2 avant tout accès réseau |
| PASS | credentials identiques en configuration temporaire de prérequis | BLOCKED code 2 ; valeur sentinelle absente des sorties |
| PASS | viewer vide en configuration temporaire de prérequis | BLOCKED code 2 ; valeur runner absente des sorties |
| PASS | reproduction du défaut initial de garde | listener `/api` seul : un appel vers le second serveur loopback est reçu sans être enregistré ; `guard-before.log` |
| PASS | reproduction du contournement redirect | context.route seul : redirection vers le second serveur reçue sans passage du handler ; `guard-redirect-before.log` |
| PASS | probe du garde final | 0 HTTP externe à l'origine de contrôle, 0 WS, 0 POST/credential URL livrés, 0 service worker ; popup et iframe bloquées avant accès ; `guard-final.log` |
| PASS | nettoyage du probe navigateur | contexte et Chromium fermés, pages=0, flux SSE actif terminé ; aucune ressource de contrôle conservée active |
| PASS | intégrité du lockfile | SHA-256 inchangé : `30d12765534dd2b2f89ae78b0b9f180ccc0fbfeb755fac33bd972608c70f2c44` |
| PASS | `npm run test:browser:live` contre services QA réels | code 0, neuf contrôles PASS dans le log final ; détails ci-dessus |
| PASS | preuve export et absence de credentials | fichier JSONL de 1689 octets, exactement deux événements QA du run final ; contrôle des credentials bruts/encodés dans l'export et les trois logs réels : aucun trouvé |
| NOT_RUN | recette trois workers A1/A2/B1, isolation inter-tenant, fournisseurs, Podman, Kubernetes et qualification de release | ce parcours dashboard ne les exécute ni ne les qualifie |
| NOT_APPLICABLE | nouveau build front pour ce seul changement | test hors bundle et ajout d'un script npm ; sources UI et dépendances inchangées |

Preuves des prérequis : `/tmp/ai-guard-browser-live-preflight.log`, `/tmp/ai-guard-browser-live-missing-env.log`, `/var/folders/ny/3tnyr3194xq7zt80kp9_g5bm0000gn/T/ai-guard-browser-live-preflight-X3CMgE/results.json`. Les fichiers de credentials de cette dernière preuve sont des valeurs sentinelles de prérequis, distinctes des secrets QA réels ; leurs valeurs ne sont pas reproduites dans les rapports.

Preuves du garde : `/tmp/ai-guard-browser-live-guard-before.log`, `/tmp/ai-guard-browser-live-guard-redirect-before.log`, `/tmp/ai-guard-browser-live-guard-final.log`. Probe final relançable : `/tmp/ai-guard-browser-live-guard-probe.mjs` avec le binaire Node 24 ci-dessous. Ces probes utilisent **deux serveurs de contrôle loopback temporaires sans credential QA réel**, exclusivement pour les régressions du garde et de son nettoyage. Ils ne sont pas le parcours live, ne remplacent jamais la gateway/dashboard et ne prouvent aucun comportement du produit. Le script live `front/tests/browser_live.mjs` ne crée aucun serveur de fixtures.

Empreintes SHA-256 finales après revue : `front/tests/browser_live.mjs` = `20c013fe5c1ed8b2f3b174c584c3c1b1886e4e65ebc8c9a835f334d974574c6a` ; `front/package.json` = `db0f728f2ea6b3a207af63541602c16779a1abc3ea54ff210da565d789ba5dcc` ; lockfile inchangé = `30d12765534dd2b2f89ae78b0b9f180ccc0fbfeb755fac33bd972608c70f2c44`.

Preuves réelles supplémentaires : `/tmp/ai-guard-browser-live-sse-stop-probe.log`, `/tmp/ai-guard-browser-live-export-order-before.log`, `/tmp/ai-guard-browser-live-guard-after-transport.log`, `/tmp/ai-guard-browser-live-unit-after-transport.log`, `/tmp/ai-guard-browser-live-unit-after-export.log`, `/tmp/ai-guard-browser-live-real-evidence-check.log`. Aucun log historique BLOCKED/FAIL n'a été supprimé.

Seul artefact du run final : `/var/folders/ny/3tnyr3194xq7zt80kp9_g5bm0000gn/T/ai-guard-browser-live-b78KI4/declared-qa-export.jsonl`, SHA-256 `6d0e425c7676cce477aaa23c2f93a6444283c106da97122e96674e2fb1ccde6a`. Aucune capture d'écran, trace ou HAR live.

La [documentation Playwright du routing](https://playwright.dev/docs/api/class-browsercontext#browser-context-route) précise la limite des service workers et recommande leur blocage. Le contournement de redirection et sa fermeture ont été vérifiés empiriquement par les probes locaux, sans dépendre d'une simple assertion sur l'outil.

## Commande de reproduction sur cette installation QA autorisée

```sh
export PATH=/Users/louis/.npm/_npx/387698761821791d/node_modules/node/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:$PATH
export CODEX_PRIMARY_RUNTIME_NODE_MODULES=/Users/louis/.npm/_npx/9833c18b2d85bc59/node_modules
export PLAYWRIGHT_CHROMIUM_EXECUTABLE='/Applications/Google Chrome.app/Contents/MacOS/Google Chrome'
export AI_GUARD_TEST_GATEWAY=http://127.0.0.1:48787
export AI_GUARD_TEST_DASHBOARD=http://127.0.0.1:48400
export AI_GUARD_TEST_CONFIG=/tmp/ai-guard-control-qa.mF6i7C/config
npm run test:browser:live
```

Versions réellement utilisées : Node `v24.22.2`, npm `11.13.0`, Playwright existant `1.64.0-alpha-1790635538000`, Google Chrome `156.0.8078.48`, macOS ARM64. Le script ne lit jamais un jeton depuis une variable imprimable de commande ni depuis une fixture ; aucun secret n'est embarqué dans le dépôt. Aucune dépendance ajoutée au projet pour Playwright ou ce test.

Acceptation de ce lot : diagnostics, garde réseau, parcours dashboard/gateway réel décrit, données QA explicitement déclarées, export filtré, mémoire seule, déconnexion et fermeture des ressources vérifiés ; revue traitée ; lockfile préservé. **Ce PASS ne qualifie ni une release production, ni la recette trois workers/tenants, ni les garanties runtime et fournisseurs.** Les autres obligations du produit restent des écarts ouverts dans la gouvernance et les rapports de l'orchestrateur.
