# Revue indépendante — préparation, navigateur live et overrides smoke

Date : 2026-10-29. Revue READ-ONLY de `front/tests/browser_live.mjs`, du script npm associé et de `tests/runtime_smoke.rs`. Aucun moteur, endpoint QA ou probe navigateur relancé par le réviseur. Aucun secret lu, aucune ressource créée/supprimée. Diffs et sources examinés directement ; Git reste sans baseline suivie.

**État final après reprise autorisée : PASS pour le code revu et les neuf contrôles du parcours navigateur local décrit dans le complément final.** Le blocage HTTP initial est levé pour cet essai ; les constats BLOCKED/NOT_RUN ci-dessous conservent l'historique de préparation. Aucun fournisseur, lancement worker ou scénario multi-client n'est qualifié par ce parcours.

## Verdict initial — avant les essais live

**PASS pour la revue du code de préparation, après traitement des constats. BLOCKED pour le parcours produit live ; ses assertions restent NOT_RUN dans les preuves examinées.** Aucun défaut bloquant restant identifié dans les fichiers revus. Ce verdict ne vaut pas réussite d'un parcours UI réel, d'un lancement autorisé durable, d'une recette trois workers ou d'une release.

L'absence d'accès HTTP hôte aux services Compose sains mais non publiés a bloqué l'essai initial. Le relais Rust autorisé ensuite relève d'un lot séparé et n'est pas qualifié par cette revue. Les origines QA n'ont pas été sondées de nouveau par le réviseur.

## Constats et corrections relus

- **P1 fermé — sorties navigateur non contrôlées.** Le listener initial ignorait les requêtes hors `/api/` et ne vérifiait pas l'origine. Le probe sans credentials QA `/tmp/ai-guard-browser-live-guard-before.log` reproduit une requête reçue par un second serveur loopback et zéro requête enregistrée. Le garde final est installé avant toute navigation (`front/tests/browser_live.mjs:61`, `:276`) : seule l'origine dashboard explicite est admise, méthode GET uniquement, credentials URL et jetons bruts/encodés en URL refusés. Service workers bloqués ; WebSockets fermées sans connexion amont ; pages supplémentaires, workers sans page et sous-frames refusés.
- **P1 fermé — redirection contournant le premier garde.** Le probe `/tmp/ai-guard-browser-live-guard-redirect-before.log` constate un destinataire de redirection atteint sans repasser dans `context.route`. Le contrôle final CDP intercepte les réponses avant suivi et refuse tout 3xx (`:96`), sans tamponner le corps SSE. Une capacité Playwright/CDP manquante bloque le parcours. Le probe final confirme zéro requête reçue au destinataire interdit, y compris par redirection. Le blocage des service workers concorde avec la [documentation officielle Playwright](https://playwright.dev/docs/api/class-browsercontext#browser-context-route).
- **P2 fermé — capture ou artefact pouvant conserver un secret en cas de régression.** La capture live a été supprimée. Avant `saveAs`, les octets exportés sont vérifiés sans viewer/runner, bruts ou encodés (`:378`). Les diagnostics masquent aussi ces formes ; aucune trace/HAR ou capture d'erreur n'est créée. Cela évite une copie de preuve persistante si une réponse régressée contient une clé.
- **P2 fermé — nouveau gateway smoke trop large pour la recette locale.** Le premier override acceptait indirectement une origine HTTPS distante via le client produit. `local_gateway` (`tests/runtime_smoke.rs:9`) impose maintenant HTTP(S) loopback sans userinfo, chemin, query ou fragment. La validation précède `runner::run` et donc la lecture des jetons (`:63`). Les défauts historiques restent `http://127.0.0.1:8787` et `agentsandbox-worker:local`. Le même gateway est utilisé pour lancement, lecture et export. Le client applicatif n'a pas été modifié pour cette restriction de test.
- **Nettoyage et fin de parcours relus.** `closeBrowserResources` (`browser_live.mjs:124`) ferme contexte et Chromium ; un échec fait FAIL. Les violations réseau sont vérifiées après fermeture (`:427`), incluant les tentatives de pagehide/unload. Le probe confirme la fermeture d'un flux SSE encore ouvert. Aucun nettoyage global, suppression de volume ou mutation de configuration n'est ajouté.

## Contrat du parcours préparé

Les trois paramètres gateway/dashboard/config sont obligatoires. Les deux URL Node sont des origines loopback contrôlées et leurs redirects refusés (`:37`, `:146`). Les fichiers viewer/runner sont lus côté Node, non vides et distincts ; le runner n'est jamais fourni au navigateur. Aucun serveur de fixtures n'est créé par le script live.

Les contrôles préparés correspondent aux services réels : identités `/healthz`, en-têtes CSP/no-store/nosniff/DENY, hashes HTML/JS servis égaux au build local, refus anonymes/invalide/écritures viewer, proxy dashboard, connexion et stockage vide, SSE et reprise Last-Event-ID sans relecture d'historique, XSS rendu comme texte, export filtré et logout/reload. Les deux événements injectés portent une source QA unique, provenance `declared` et couverture `generic` ; aucun effet runtime ou fournisseur n'est déduit. Toutes ces assertions produit restent **NOT_RUN**, notamment la validité réelle de leurs sélecteurs et de leurs délais sur les services.

## Commandes et preuves examinées

| Statut | Preuve | Résultat constaté |
| --- | --- | --- |
| PASS documenté | Node 24.22.2 `node --check tests/browser_live.mjs` | Code 0 consigné dans le rapport auteur final. |
| PASS | `/tmp/ai-guard-browser-live-unit.log` | 17 tests réussis, zéro échec/ignoré ; log postérieur à la dernière édition du script. |
| PASS prérequis | `/tmp/ai-guard-browser-live-preflight.log` et rapport auteur | Paramètres absents, origine distante, credentials absents/identiques/vides refusés sans réseau ; aucune valeur sentinelle conservée dans les résultats lus. |
| PASS garde uniquement | `/tmp/ai-guard-browser-live-guard-final.log` et probe source relu | HTTP hors origine=0, WS=0, POST/URL secrets livrés=0, service worker=0 ; popup/iframe bloquées ; contexte/pages/navigateur/flux fermés. |
| PASS logiciel smoke | `/tmp/ai-guard-ingress-cli-green.log` | `smoke_gateway_is_a_loopback_origin_before_any_credentials_are_loaded` réussit ; vrai smoke ignoré dans cette commande. Les autres tests du log sont hors périmètre de cette revue. |
| FAIL historique | `/tmp/ai-guard-real-smoke.log` | `empty host`, échec en 0.01 s avant lancement ; aucune qualification runtime. |
| BLOCKED / NOT_RUN | Navigateur et smoke sur services QA | Aucun parcours produit réussi dans les preuves examinées ; accès HTTP hôte manquant au constat. |
| NOT_APPLICABLE | Nouveau build UI pour ces seules modifications | Le nouveau script de test n'entre pas dans le bundle ; aucune dépendance ajoutée par ce lot. |

Les deux serveurs loopback du probe du garde sont des contrôles temporaires sans credentials QA, distincts de la gateway/dashboard et du parcours produit. Ils prouvent le garde et sa fermeture, pas les propriétés de l'application. Probe conservé : `/tmp/ai-guard-browser-live-guard-probe.mjs`. Rapport auteur final : `/tmp/ai-guard-browser-live-report.md`.

## Identité des fichiers et limites

- `front/tests/browser_live.mjs` : `d7e16ba4e9867bead63a9c7b9556cc4e1fa6fe7cbc7e107a9cbed2643a58d044`.
- `front/package.json` : `db0f728f2ea6b3a207af63541602c16779a1abc3ea54ff210da565d789ba5dcc` ; seul ajout du lot : `test:browser:live`. Le changement Vite déjà revu appartient au lot antérieur.
- `front/package-lock.json` : `30d12765534dd2b2f89ae78b0b9f180ccc0fbfeb755fac33bd972608c70f2c44`, inchangé.
- `tests/runtime_smoke.rs` : `dee00fbff6ede7324daae43522f7f01c6dfb63acf5dbfe8fd116ddf5811e9bcc`, diff comparé à `/tmp/ai-guard-decision-baseline.woj2zP/tests/runtime_smoke.rs`.

AGENTS/RTK, architecture, règles de code et stratégie de tests relus. Les skills projet restent absents ; chargement automatique non confirmé. Aucun audit global produit, Podman ou Kubernetes n'est inclus. À cette étape initiale, le blocage restant était la disponibilité des origines QA autorisées, puis l'exécution complète et le traitement de ses résultats réels.

## Complément final — essais réels et corrections du harnais

Relecture indépendante de la version finale `front/tests/browser_live.mjs` : SHA-256 **`20c013fe5c1ed8b2f3b174c584c3c1b1886e4e65ebc8c9a835f334d974574c6a`**, recalculé par le réviseur. Aucun changement des sources UI. Les empreintes du manifeste, du lock et du smoke indiquées précédemment restent celles de leurs sous-lots revus.

Les défauts du harnais ont été diagnostiqués pendant la vraie exécution, sans changer le comportement produit pour obtenir un vert :

1. Le premier live `/tmp/ai-guard-browser-live-real-20261029-8bd72c0e.log` passe services/auth/headers/bundle/UI/SSE et XSS, puis **FAIL** en attendant l'état retrying. Le probe `/tmp/ai-guard-browser-live-sse-stop-probe.log` reproduit un fetch SSE encore ouvert après `setOffline(true)`. `interruptBrowserTransport` ajoute l'arrêt réel des chargements de la page via CDP `Page.stopLoading`, avec détachement de sa session en finally. Les assertions retrying, Last-Event-ID, rattrapage sans relecture et absence de doublon sont conservées ; aucune transition UI n'est fabriquée. Le [protocole officiel Page](https://raw.githubusercontent.com/ChromeDevTools/devtools-protocol/master/pdl/domains/Page.pdl) décrit l'arrêt des navigations et ressources en attente.
2. La coupure émet une erreur réseau CDP avec un InterceptionId déjà avorté. Le catch du garde ignore désormais l'échec de commande seulement si le protocole fournit `responseErrorReason` ou si la page est fermée. Le [protocole officiel Fetch](https://raw.githubusercontent.com/ChromeDevTools/devtools-protocol/master/pdl/domains/Fetch.pdl) identifie ce champ comme erreur de réponse. Les 3xx restent signalés/refusés et les autres erreurs CDP sur page ouverte restent des violations. Le probe coupure est **PASS**, zéro violation et cleanup PASS ; le garde complet garde ses destinataires interdits à zéro.
3. Le deuxième essai passe la reprise SSE, puis la comparaison des hashes de `JSON.stringify` échoue sur l'ordre des propriétés. `/tmp/ai-guard-browser-live-export-order-before.log` constate deux événements profondément égaux mais sérialisés avec des ordres de clés différents. `isDeepStrictEqual` remplace cette comparaison : les valeurs, types et ordre des événements restent comparés. `assert.ok` conserve un diagnostic sans dump du contenu. Aucune divergence de données n'est acceptée par cette correction.

**Verdict final ciblé : PASS. Aucun nouveau défaut bloquant identifié dans ces ajustements.**

| Statut | Preuve finale examinée | Résultat |
| --- | --- | --- |
| PASS | `/tmp/ai-guard-browser-live-unit-after-export.log` | 17 tests, 17 réussis, zéro échec/ignoré ; syntaxe finale également PASS dans le rapport auteur. |
| PASS | `/tmp/ai-guard-browser-live-real-20261029-8bd72c0e-after-export.log` | `npm run test:browser:live`, exit 0, neuf checks PASS : services, auth/read-only, bundle, UI/mémoire, SSE/XSS, reconnexion, export, logout/reload et cleanup. |
| PASS | Identité d'exécution dans ce log | Node v24.22.2, Chrome 156.0.8078.48 ; gateway `http://127.0.0.1:48787`, dashboard `http://127.0.0.1:48400`. |
| PASS | Export conservé | SHA-256 recalculé par le réviseur sur `ai-guard-browser-live-b78KI4/declared-qa-export.jsonl` : `6d0e425c7676cce477aaa23c2f93a6444283c106da97122e96674e2fb1ccde6a`, identique au résultat du script. Aucun contenu secret affiché ou lu dans la réponse de revue. |
| PASS limité | Données réelles traversant gateway/dashboard | Projet QA `qa-browser-774215b6-1fcd-475d-bf28-b3a07503ab06`, séquences 35 et 36, source `qa.browser.live`, provenance `declared`. |
| NOT_RUN dans ce parcours | Fournisseurs, workers A1/A2/B1, isolation inter-tenant, quota/relais/usage, reprise daemon, Podman/Kubernetes | Aucune réussite déduite des neuf contrôles navigateur. Le smoke moteur reste un sous-lot distinct. |

Les FAIL initiaux et le blocage HTTP historique sont conservés. La réussite finale porte sur le parcours navigateur local avec injection QA déclarée et backend réel ; elle ne transforme pas ces événements en effets runtime ni en preuve d'un produit complet. Aucun endpoint ou navigateur n'a été relancé par le réviseur.
