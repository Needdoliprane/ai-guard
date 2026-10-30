# Revue indépendante ciblée — registre, API et identité du dashboard

29 octobre 2026 ; `/Users/louis/Documents/ai-guard`. Revue en lecture seule avec `security-audit` en guidance ciblée. Aucun test, build, processus cible, navigateur, moteur, endpoint ou service distant exécuté par ce reviewer. Aucun fichier du dépôt modifié. Les seules écritures du reviewer sont ce rapport demandé, hors dépôt. Le checkout n’a aucun `HEAD` et son contenu est non suivi ; les empreintes ci-dessous identifient les sources réellement lues.

AGENTS.md, RTK.md, ARCHITECTURE_CONTRACT, CODING_RULES, PRODUCTION_READINESS, TEST_STRATEGY, CLOUD_TENANCY, USAGE_METERING et LOCAL_CLOUD_TESTS lus. Le skill projet `ai-guard-security-review` est absent de `.agents/skills` et `.claude/skills` ; chargement automatique non confirmé. Il ne s’agit ni d’un audit exhaustif ni d’une qualification production.

## Conclusions distinctes

- **Registre/replay : PASS source + preuves fournies pour les défauts précis identifiés et corrigés pendant cette revue.** La libération de quota sans terminal et le reçu d’admission sans décision liée ont été reproduits par l’auteur, corrigés puis relus au gel `c515f12…` / `f984711…`. Précondition forte conservée : écriture externe SQLite et recalcul des hashes. Aucun exploit par API, worker ou identité distante n’est établi. Ce PASS n’est pas une reproduction dynamique indépendante.
- **API/main/configuration : aucun défaut bloquant nouveau identifié dans les chemins relus.** Le correctif du refus des runner par le dashboard et le mode partagé sont cohérents avec les preuves fournies ; cela reste une validation source + logs de l’auteur.
- **Frontend identité : aucun défaut bloquant nouveau identifié.** Les gardes d’instance, les effacements à la déconnexion et les tests A→B fournis sont cohérents. Aucune ancienne fuite A→B n’est présentée comme reproduite.

## Findings confirmés par les preuves auteur, puis corrigés

**F1 — replay terminal sans preuve : medium, corrigé.** Sévérité limitée au contrat d’intégrité du stockage et à la précondition d’écriture SQLite externe ; aucune sévérité remote/worker revendiquée. Le défaut empêchait de faire confiance au verdict sain et à la réservation après réouverture. Racine historique dans `src/audit/control.rs:367` et `:409` du snapshot `ffa9ac86825700a73a78f86a9abc1305fce318c5cad01b74cf9ccc5a062ba7e7`.

Frontière examinée : détection d’une base/projection altérée à la réouverture, déjà revendiquée par le registre et éprouvée dans `tests/worker_registry.rs:527`. Un acteur disposant d’écriture SQLite peut préserver les identités immuables et révisions, réécrire le dernier événement `unknown` ainsi que sa projection, et faire accepter une libération de quota sans preuve terminale. Il ne s’agit pas d’une attaque à distance ni d’un acteur worker ayant acquis l’accès au fichier.

Reproduction historique de la source initiale, désormais conservée par `tests/worker_registry.rs:624` :

1. Créer admission puis mutation `Unknown`, comme la régression `projection_replay_rejects_prepare_and_created_records_without_their_exact_transition` (`tests/worker_registry.rs:527`). Fermer le Store avant altération.
2. Dans le dernier événement, conserver `data.action="unknown"`, `operation_id`, `input_hash`, `previous_revision` et toutes les identités. Changer seulement `data.worker.state="terminal"` et `data.worker.quota_held=false`. Laisser `data.observation=null` et `data.allocation=null`.
3. Recalculer `data.snapshot_hash` avec `content_hash("ai-guard:worker-snapshot:v1", worker)` ; conserver la sérialisation Rust normale pour `workers.snapshot_json`. Mettre à jour `workers.state`, `quota_held`, `snapshot_json`, `snapshot_hash` pour cette ligne. Aucun champ `worker_attempts` ne change dans ce delta minimal.
4. Recalculer `events.payload/hash` avec le même utilitaire SHA256 que le test existant (`tests/worker_registry.rs:557`) et ajuster `quota.bytes` de la différence de taille du payload. Comme l’événement est le dernier, aucun successeur n’est à réécrire.
5. Rouvrir. La condition quota↔state (`control.rs:319`) passe ; les identités/révisions (`:329`) passent ; le replay exact `prepare/created` (`:367`) est sauté ; le contrôle de preuve terminale (`:409`) est sauté car `allocation=None` ; les projections (`:486`) sont maintenant égales. Le résultat attendu du défaut est un Store sain et `held_workers=0` sans observation terminale ni allocation.

Le delta minimal ne nécessite pas de mettre en marche un conteneur ni de modifier un secret. Pour une démonstration d’impact d’admission, utiliser ensuite un second worker sous quota 1 dans la même fixture ; ce n’est pas nécessaire pour démontrer la libération erronée.

Correction relue au gel final : `apply_worker_transition` (`src/audit/control.rs:1186`) est l’unique logique pure pour les mutations fraîches et leur replay. `replay_worker_transition` (`:1439`) recompose l’entrée complète, vérifie `input_hash`, refuse les métadonnées d’admission et les actions inconnues, réapplique cette logique puis compare exactement worker, observation et allocation. Il refuse donc terminal sans allocation/observation et tout delta inventé ou oublié de claim, quota ou nettoyage. La forme initiale d’admission est strictement Reserved, sans preuve d’exécution ni nettoyage (`:367–402`). Aucune remise à zéro de claim n’a été ajoutée.

Preuve auteur lue : `/tmp/ai-guard-worker-replay-red.log`, quatre régressions FAIL avant fix, dont « rehashed unknown event released quota with no terminal evidence », « terminal accepted missing_allocation », action create_issued et admission ayant inventé un terminal. Le vert final est décrit ci-dessous. Les variantes de chaque boucle ne sont pas toutes démontrées rouges individuellement par le log, puisque la première assertion arrête le test ; elles sont toutes exécutées dans le vert.

**F2 — admission dont le reçu ne correspond à aucune décision exacte : medium, corrigé.** Même précondition d’écriture SQLite externe et recalcul cohérent des projections/hashes. Le premier fix de F1 contrôlait la forme du reçu sans recouper la décision durable. Changer seulement `worker.receipt.audit_hash` vers 64 zéros puis utiliser le helper `rewrite_last_control_record` laissait rouvrir un Store sain. Le log `/tmp/ai-guard-worker-replay-receipt-red.log` confirme « admission accepted receipt_hash ». Le risque concret était un verdict d’intégrité acceptant un worker sans lien vérifiable vers son autorisation historique ; aucun lancement à distance n’a été démontré.

Le correctif final `validate_admission_input` (`src/audit/control.rs:1367`) lit le seq du reçu, exige une décision `security.decision` gateway/observed Allow antérieure à l’admission avec hash exact, reconstruit tous les champs de DecisionRequest depuis cet événement, recoupe l’historique et la phase via `lookup_decision`, puis valide AdmissionV1 et son `input_hash` complet. `tests/worker_registry.rs:781` refuse hash/seq/phase/input altérés ; le positif `:807` conserve une admission Approved avec tous ses champs de corrélation après réouverture. La politique historique complète n’est pas réévaluée et n’est pas présentée comme telle. Les refus sans worker ne portent pas de receipt/request dans ControlRecord ; seule leur forme fermée est reconstruite, sans effet runtime ni réservation.

**PASS source + preuves auteur pour F1/F2 corrigés.** `/tmp/ai-guard-worker-replay-green.log` contient 31 registry + 2 usage + 4 migration = 37 PASS, zéro échec/ignoré. Le positif de toute l’histoire de transitions (`tests/worker_registry.rs:920`) exerce aussi la réouverture avec claims, terminal, allocation et cleanup. Ce correctif conserve les versions ControlRecord v1 et schéma v2 ; aucune migration de données silencieuse ajoutée. Aucun défaut bloquant nouveau trouvé dans ce gel.

## Contrôles registre/API constatés

Dans les chemins de mutation normaux, `src/audit/control.rs:1135` rejoue une identité de mutation avant tout nouveau CAS, recoupe action/input_hash, et retourne `AlreadyConsumed` pour le replay `start_claim`. La mutation fraîche exige attempt et révision exacts (`:1151`). `prepare_worker` (`:1511`) et `record_created_worker` (`:1526`) refusent un recul dès qu’un create/container/start antérieur existe ; ils gardent tentative, spec et quota. Le replay final recoupe maintenant toutes les actions, comme détaillé ci-dessus.

La terminalisation normale dans le helper (`:1223–1331`) conserve les incohérences d’allocation et le premier début observé, ne libère le quota que pour un terminal complet et produit allocation+observation+worker+audit dans la même transaction. `Unknown` et `Stop` gardent la réservation. Les défauts corrigés portent sur la validation ultérieure du replay altéré, pas sur une route HTTP normale qui permettrait directement ces deltas.

`src/api.rs:229` choisit exclusivement la base à partir d’un credential configuré. Les routes workers, opération, quota et usage (`:494–650`) appellent cette autorisation avant les accès Store ; le viewer est refusé pour les écritures, un autre tenant reçoit une recherche dans sa propre base. Aucun tenant libre n’est transféré au Store. Les requêtes de page sont fermées et bornées à 1–100 (`:477–492`), avec curseurs UUID contrôlés dans le Store.

L’admission (`api.rs:494`) valide le contrat puis effectue une politique fraîche sur une nouvelle opération ; le Store réévalue la politique déterministe sous transaction (`audit/control.rs:677`). Une reprise garde l’ancien résultat durable et ne remet pas en circulation un droit de start. Le contenu divergent d’une même opération est un conflit. Les erreurs ControlFailure sont mappées notamment à HTTP409 pour les anciens CAS (`api.rs:446`). Les namespaces `control`, `worker`, `usage.allocation` et décisions sont refusés à l’ingestion libre (`:362`).

`src/gateway_config.rs:36` lit un registre opérateur fermé, borné à 1 Mio, et valide les quotas. `main.rs:132` lit les credentials dans des fichiers réguliers bornés avec refus des liens finaux Unix ; les valeurs ne sont pas écrites dans le JSON du registre ni les diagnostics. Des credentials distincts sont imposés pour tous les couples tenant/rôle. Le tenant est durablement lié à la base ; ces fichiers restent dans le contrôle de confiance. Aucun accès fichier worker n’est établi.

La migration (`main.rs:275`, `audit/control.rs:990`) exige un fichier source existant et une attestation explicite d’arrêt. Elle crée une destination neuve, refuse les sidecars préexistants, vérifie la sauvegarde et recontrôle la tête source avant DDL transactionnelle. L’attestation n’est pas un fencing runtime ; la restauration sûre des v2 contenant des claims reste un écart publié, pas une garantie établie par ce code.

## API/dashboard : conclusion précise

`src/api.rs:824–856` n’accepte qu’un upstream fixe sans userinfo/query/fragment ni chemin ; client sans proxy d’environnement ni redirections. Le proxy (`:858`) est uniquement GET, avec allowlist explicite de routes et d’un détail worker UUID canonique ; les routes de mutation et de récupération d’opération ne sont pas proxifiées. Il reconstruit l’URL depuis l’origine configurée et ne transmet que le jeton authentifié et le curseur SSE nécessaire.

La vérification (`:928`) relit `/api/v1/identity` avec délai 3 secondes, réponse limitée à 4096 octets, rôle `viewer` obligatoire et tenant syntaxiquement valide. Elle s’applique aux modes fixe et partagé. Le mode fixe recoupe en plus le credential configuré. Un fichier du dashboard contenant par erreur une clé runner n’ouvre plus une session de lecture. `main.rs:108` rend `--shared-tenants` explicite et incompatible avec le fichier d’identité fixe ; `:308` sélectionne la construction correspondante.

**PASS source + preuves fournies pour ces comportements précis.** Les logs `dashboard-role-red` montrent 200 au lieu de 403 avant le fix ; `dashboard-role-green` montre les quatre tests d’alors verts. `dashboard-shared-cli-red` démontre l’option absente avant changement ; `dashboard-shared-green` montre 9 tests main et 5 tests API verts. `/tmp/ai-guard-dashboard-api-final.log` contient les 5 tests API finaux verts. Pas de reproduction dynamique indépendante.

## Frontend identité : conclusion précise

`front/src/api.ts:37–80` borne les routes au même origin, transmet le credential seulement dans Authorization, refuse les redirections, valide le modèle `{tenant_id,role:"viewer"}` et n’accepte aucune variante query/hash/suffixe de `/api/v1/identity`. `front/src/main.ts:303–319` attend l’identité avant de créer la session ou de lire les données ; le champ du jeton est vidé dès la tentative.

La portée affichée vient de cette identité (`main.ts:448–452`, `:514`, `:917`) et passe par la construction DOM textuelle. La déconnexion ferme l’ancienne instance, efface identité/date/collections/curseurs/erreurs et ferme le détail (`:559–588`). `refreshSummary` (`:659`) compare l’instance de client, `loadEvents` (`:693`) compare client+numéro de requête, SSE (`:627–657`) et export (`:2055`) recoupent l’instance. `ControlDashboard.close` (`front/src/control-view.ts:87`) ferme l’ancienne instance et vide les ressources ; lecture et détail refusent les retours tardifs (`:151`, `:157`, `:314`, `:344`).

Le découplage identité/overview est réel : une erreur overview est gérée par `refreshSummary` et ne fait pas revenir silencieusement au login lorsque l’identité reste valide. Les résultats inconnus, les erreurs et les valeurs périmées des vues de contrôle restent distincts. Le CSS `.authenticated-scope` (`style.css:248`) permet le retour à la ligne du périmètre maximal.

**PASS source + preuves fournies, aucun défaut bloquant nouveau trouvé dans ce périmètre.** Le nouveau test `browser_identity.mjs` valide le refus avant données pour identité indisponible/invalide/runner, puis retient les continuations A après transport terminé, connecte B sous HTTP500, libère douze retours A et vérifie DOM/détail/export/compteurs, l’ancien refus403, la péremption du même tenant et l’identifiant 128 sur mobile. La barrière est injectée exclusivement dans la fixture ; aucun hook produit n’apparaît. Le scénario teste donc les gardes de l’application en plus de l’annulation transport. Il ne prouve pas une ancienne fuite A→B.

Preuves brutes lues sous `/tmp/ai-guard-dashboard-identity/` : `node-final.log` 27 PASS, zéro échec/ignoré ; `browser-complete-final.log` parcours historique et identité PASS, douze retours tardifs ; `build-final.log` tsc+Vite PASS. `versions.log` rapporte Node24.22.2/npm11.13.0/Chrome156.0.8078.48/DarwinARM64/macOS27.2. Ces versions sont celles de l’auteur, non réexécutées ici.

## Autres preuves confrontées et limites

- **PASS fourni**, `/tmp/ai-guard-worker-prepare-recovery-green.log` : 24 registry + 2 usage + 4 migration, 30 réussites, aucun échec/ignoré. Ce résultat précède le finding replay général ci-dessus et ne le ferme pas.
- **FAIL historique fourni**, `/tmp/ai-guard-worker-prepare-recovery-replay-red.log` : la mutation de l’action vers `prepare` sans delta est acceptée avant le fix ciblé, régression ensuite verte. La généralisation à toutes les actions est le travail demandé maintenant.
- **NOT_RUN par le reviewer** : tous tests/builds et toutes exécutions cible, faute de profil OS complet d’exécution établi pour security-audit. Aucun bypass ni permission n’a été demandé.
- **FAIL intégré fourni / relance requise** : `/tmp/ai-guard-worker-replay-clippy.log` montre Clippy arrêté à la compilation sur deux fonctions temporairement absentes de l’édition parallèle `src/operations.rs:260,283`. Aucun Clippy global PASS revendiqué par cette revue ; root doit fournir le résultat après intégration.
- **NOT_RUN dans les preuves de cette revue** : UI courante contre un vrai service Rust partagé/fixe, nouvelle pile de conteneurs, recette réelle A1/A2/B1, Docker/Podman/Kubernetes/cloud, crash réel, restauration/fencing runtime et qualification release. Les fixtures CLI/frontend ne qualifient pas ces effets. Le Docker positif signalé par root est intermédiaire, antérieur au correctif replay final ; il ne constitue pas une preuve finale de ce rapport.
- **BLOCKED pour qualifier une release** : gates G01–G12 restent ceux publiés dans PRODUCTION_READINESS ; cette revue partielle ne clôt aucun gate global. Aucun finding source bloquant restant n’a été établi dans le périmètre final relu, mais les checks intégrés et preuves runtime finaux restent à root.

Le nouveau `front/tests/browser_live.mjs` a aussi été relu sans être exécuté. Il compare workers proxy/gateway, attend un worker terminal QA explicite, affiche son CID dans le détail, compare le quota UI et lie une allocation observed/non facturable à ce CID. Le mode dashboard fixe/partagé détermine le refus runner401/403 attendu. Les identités worker/attempt/CID/observation sont conservées dans le résultat, sans credential. Le check reste `NOT_RUN` dans ce rapport ; il ne devient pas PASS par sa seule présence. La qualification de sa provenance runtime dépend du smoke et de l’identifiant QA fournis explicitement par root.

## Empreintes source lues

```text
c515f12bef44db458b8efcccb2a0b5d6282558537ffbdb16273bb99df1d68e0d  src/audit/control.rs
48eb7af0d120f583ffe976fb895c4c91a61cab7145266437952644da440b8a7a  src/api.rs
83376200385179168e60f63e6ffdf5174fba490b36fd2274f897858acb61694e  src/main.rs
e75156a82f6e0701acb85dfeda7a1da5b92cd5a94928049a726217d4700f8e9f  src/gateway_config.rs
692ce0be2d85eff93fb9ce3ae4d51f6275056187c04ce0a3a2132923f791e96a  src/model.rs
16bfdf72dc0e4d883818b03006c60ceaae4be3d04c15ef7cd554d8e1df31cabe  src/control.rs
755ead518da7d3b4d8089e7545acd9705e5ebe26836db12adfe769d030f82b10  tests/workers_api.rs
cbdf74f2e3812c8e66f6359b9dbc0e34a0b368c1be9178233da79bb8675bf3e6  tests/control_cli.rs
f984711fad3d2564562c45631a9516e9ca9575aaae09e3f5492ba4e9ae87d56c  tests/worker_registry.rs
dd6e69d26aa45fe9a48b5b19251bb13a8a56d47308163a53e834a6dc2abad304  front/src/api.ts
da8715d8f815deef54d4c23a35f23be34ed936ce529abc5c7e01453968d7bca2  front/src/main.ts
c4cc03a39c65d761af3fe59676bd7ab4ccb4b0134df20080a3969547bd6ee340  front/src/types.ts
e5f2c43d581708a0f854ef43fe4cffb260a3281af495180a2207c76b55fb398f  front/src/style.css
03f087ce5cf5d3ab99fa03253c313a31cba2fc16f261971d11ecd93c90a940b5  front/src/control-view.ts
bf3a0eb33185823e907b626883b6844f9829155dc27a9f58f8afae3b7d2bc86d  front/tests/api.test.mjs
47683f7f88113049268664914e0834a3c212ae71ed240a8ed7c5920543a7352d  front/tests/browser.mjs
4cd546f328464fba6b3f3704951e39ba5afec13b79abad2a62e4d4b6fb7dcef0  front/tests/browser_identity.mjs
3c6cefd8d8a7273c5c26f28ddf1f937317c3d175f082141fbfd063a250194eac  front/tests/browser_live.mjs (relecture source, NOT_RUN)
30d12765534dd2b2f89ae78b0b9f180ccc0fbfeb755fac33bd972608c70f2c44  front/package-lock.json
```

Les neuf empreintes front du lot identité correspondent exactement à `sha256-final.txt` de l’auteur ; `browser_live.mjs` a son empreinte de relecture séparée. Les empreintes API/main/configuration/model/control et tests API/CLI sont restées inchangées après réception du correctif de registre. Les empreintes finales registre correspondent au gel reçu de l’auteur et ont été recalculées en lecture seule. La preuve navigateur live et les checks intégrés ultérieurs restent à confronter séparément par root.
