# Revue ciblée — identité owned et contexte du journal

État final : revue ciblée CLOSE, sans constat bloquant résiduel sur code, oracles, documentation ou preuves du périmètre annoncé. Aucun test exécuté par le reviewer.

Base vérifiée `537f8d2c072139bb4ced0f3b7bd385f15fb1e9dd`, worktree `/Users/louis/Documents/ai-guard/.worktrees/broker-supervision`. Fichier externe privé0600. Revue source uniquement ; aucun test/cargo/runtime/modèle, édition du dépôt ou commit exécuté par le reviewer.

## Verdict source initial

La correction de `runner.rs:644` est nécessaire et correspond au code producteur réel. Le handle owned du worker porte l'identité d'opération du plan/de l'intention, pas l'identité de tentative du Store.

Chaîne réelle relue, indépendamment de la fixture :

- `runner/managed.rs:1109-1119` construit le ContainerSpec du worker avec `plan.operation_id`.
- `runtime/managed.rs:867-873` crée le ContainerHandle et recopie `spec.operation_id()` dans son champ operation ; le chemin create/observe possède la même règle à `runtime/managed.rs:716`.
- `runtime.rs:67-73` reçoit cette opération séparément de la session et des noms gérés.
- `audit/control.rs:745-746` crée worker_id et attempt_id séparément ; `worker.admission_operation_id` conserve l'opération d'admission/plan (`754`). L'égalité attempt==plan n'est jamais une règle produit.
- La récupération existante recoupe déjà `owned.handle.operation_id()==plan.operation_id` (`runner/managed.rs:2234`). Le nouveau validateur doit reconnaître ce même handle.
- La branche import/helper reste distincte : `owned.handle.operation_id()==import.operation_id` (`runner.rs:671`). Le correctif ne modifie pas cette identité ni les producteurs helpers.

Avant correction, un vrai checkpoint owned avec plan.operation_id distinct de worker.attempt.attempt_id était refusé lors de Journal::advance alors que rev0 initiale sans managed avait pu être acceptée. La fixture précédente copiait à tort attempt_id dans son handle et masquait donc ce défaut du nouveau validateur. Ce constat remplace toute interprétation antérieure selon laquelle le contexte owned aurait été pleinement exercé par les seules régressions de versions.

Delta applicatif : une comparaison remplacée par `intent.operation_id()`, sans changement de producteur, Store, plans1/2/3, labels, runtime, claim, autorisation ou deadline. Portée du risque : un valideur reconnaît maintenant le handle réellement produit et refuse un handle erronément lié à la tentative. Aucun nouveau start n'est créé par le journal.

## Oracles à confirmer au gel des tests

- Fixture owned alignée sur le producteur, assertion explicite plan.operation_id distinct d'attempt_id ; ne pas seulement adapter une fixture à la nouvelle comparaison.
- Rev0 exige initial(intent)/managedNone ; un payload managed ne peut être accepté comme rev0.
- Chemin réellement contextualisé par Journal : accept initial → advance managed valide → load → drop/reopen → load identique. Le builder seul ne prouve pas ce parcours.
- Négatifs à partir d'un baseline positif : tenant, origin, spec, handoff, worker et owned.operation=attempt_id doivent rester parsables et être refusés par Journal::advance ; tête confirmée inchangée et absence de reçu valide.
- Restauration de l'ancien pointeur exact : graphe encore cohérent, rejet par writer vivant et clone (load/reserve/advance) ; après drop/reopen, aucune anti-restauration n'est promise et une lecture conservatrice de l'ancien préfixe reste permise.

Lecture initiale transitoire : la table contenait deux entrées identiques « owned operation ». Parent et agent de tests avertis ; doublon retiré. La table actuelle comporte six cas distincts, pas sept. Tests à ce passage : SHA `4c5249a252b39f7e1fa182308dc47e5d1d08c841f1a1ad5b5e73b693c7ab5839`. Runner correctif vérifié : SHA `9f5861e9e2114846f852525a393554c98e1e5b91de3f534a8c70a6dfe7b4eabb`.

## Oracles et premières preuves relus

Les deux nouvelles fonctions testent effectivement Journal, au-delà du builder :

- Le payload managed d'entrée reste construisible ; rev0 préremplie est refusée par accept avec Conflict, accepted.json absent et load Interrupted. Le chemin positif accepte initial(intent), avance managed à rev1, compare le payload chargé puis le relit après drop/reopen identique.
- Chaque négatif démarre d'un nouveau job initial accepté et d'une mutation unique du baseline positif. Le builder doit réussir, puis Journal::advance doit retourner Conflict et load doit rester à rev0. Les six cas couvrent tenant, origin du managed, spec_hash worker, handle.operation=attempt_id, credential_digest inattendu et pending.worker_id étranger. Un scope d'origin distinct refuse également l'intention dès l'accept initial.
- Le test de plancher conserve les octets exacts du pointeur rev0, publie rev1 puis rev2 et remet rev0 par écriture en place. Le préfixe rev0 reste structurellement valide et les révisions ultérieures restent présentes ; seul le plancher mémoire doit provoquer Conflict. Les mêmes trois opérations (load/reserve/advance) sont refusées sur writer et clone, donc aucune mutation de plancher ne masque le rollback entre appels. Après fermeture des deux handles, la nouvelle ouverture lit rev0 et reserve renvoie Accepted plutôt qu'un nouveau ticket : frontière anti-restore après reopen explicitement non promise.

La fixture est dérivée de vraies décisions/admission/transitions du Store SQLite ; ses champs runtime restent synthétiques et aucun conteneur n'est créé. Le lien plan.operation du handle est vérifié ici par lecture indépendante du code producteur ; ce test ne prétend pas avoir exécuté le producteur/runtime réel.

Répertoire de preuves relu `/tmp/ai-guard-journal-ownedop-red-20261030` :

- `target.*` : première tentative exit101, échecs de compilation du harnais (taille de tableau et String déplacée). Ce n'est pas le RED comportemental du produit.
- `retry.*` : exit101, deux tests compilés exécutés, deux échecs Conflict précisément à l'advance du managed positif. C'est le RED utile de l'écart owned.operation sur le vrai parcours Journal.
- `green-two.*` : exit0, deux tests filtrés PASS après correctif. Résultat intermédiaire avant déduplication finale ; ne pas l'attribuer à un gel test ultérieur sans le résultat final correspondant.

Aucun défaut bloquant supplémentaire dans ces oracles après retrait du doublon.

## Résultats et gels finaux source/tests relus

Rapport Luna `/tmp/ai-guard-journal-context-tests.md` SHA256 `d1c013351a96499685636b2d47a596cf26bbff5d0882d8cd66f5107a3c97e555`, logs RED/GREEN et manifests correspondants relus. Les premiers tests utilisant owned.operation=attempt ne sont explicitement plus utilisés comme preuve du checkpoint produit ; le rapport les distingue du nouveau RED utile.

- RED `retry` : runner ancien `3b7f949519bc7f8bcd2f1b504e97e09ad1950f48de7c03c80aed0e660deb3805`, tests `c7e65ab5cdab196798e13a538fd7abe9b236ed2cce2310f62d81764726b87121`, manifests avant/après constants ; 2FAIL Conflict à l'advance managed positif, exit101.
- Suite journal complète : **12PASS**,0fail/ignored,1,36s ; runner corrigé `9f5861e9e2114846f852525a393554c98e1e5b91de3f534a8c70a6dfe7b4eabb`, tests dédoublonnés `4c5249a252b39f7e1fa182308dc47e5d1d08c841f1a1ad5b5e73b693c7ab5839`, manifests suite avant/après identiques. Ce résultat couvre aussi les dix fonctions préexistantes de la cible, pas seulement les deux nouvelles.
- fmt global PASS, Clippy all-targets strict PASS après correction d'un type_complexity dans le test, release PASS35,61s ; logs exit0 lus. Seul delta après les12tests : nommer le type exact du tableau de six mutations (`ContextMutation`), sans changement d'assertion, condition ou code applicatif. Le report final doit garder cette chronologie, sans attribuer le run12 à un hash test postérieur.
- Source de test finale `c5e5c002859356d404866578bc545d48377df7e8c9da093f7eae4f6195d23536`, runner inchangé `9f5861…abb`, empreinte manifeste release111fichiers avant/après `96c711ac7ebad5fbd78e9cf5214ec24ef1ab5f8407e02c61c1351942400ff8d2` identique, recomputée par cette revue sur les deux fichiers de manifests.
- Artefacts annoncés dans les logs/rapport : exécutable `b884e84305f125cdaae610881ecc4e747f3ede0a5e634e4cf3391b5a01482ec6`, rlib `3c520d88d91cc2fd465d9435097c9c55cb02814ae18921a24ae31d10edaac670`. Ils restent à recouper avec le dossier final conservé fourni par root ; ce reviewer ne les a pas encore rehashés à ce passage.

Verdict code/tests à ce gel : **aucun constat bloquant résiduel identifié**. Le correctif traite le contrat du vrai producteur, les oracles sont adéquats pour le parcours ciblé et leur comptage est honnête après dédoublonnage. Les tests runtime producteurs, helpers réels, variantes fournisseur/TTY, Linux, reprise/fencing et anti-restore après reopen restent hors de ces preuves.

Les résultats ci-dessus ont ensuite été rapprochés des preuves archivées et artefacts conservés ; voir clôture ci-dessous. Aucun test ni compilation relancé par le reviewer.

## Clôture des documents, preuves et artefacts

Relecture finale du delta staged : une seule comparaison applicative dans runner, nouveaux parcours/regressions dans la cible broker_journal et documents de suivi avec dossier `docs/engineering/evidence/2026-10-30/journal-context/`. Le code et les tests restent aux SHA annoncés, sans changement de producteur, Store, plan1/2/3, helper, claim ou start.

Vérifications read-only effectivement exécutées sur la clôture :

- `shasum -a 256 -c docs/engineering/evidence/2026-10-30/journal-context/source.sha256` : **111/111 OK**, exit0, manifeste `96c711ac7ebad5fbd78e9cf5214ec24ef1ab5f8407e02c61c1351942400ff8d2`.
- Manifeste `SHA256SUMS` du dossier de preuves avant rafraîchissement de cette revue : **32/32 OK**, exit0. Root doit régénérer ce manifeste après copie du présent verdict final ; il ne s'agit pas d'un hash circulaire de lui-même.
- Rapport de tests archivé `tests.md` : SHA `d1c013351a96499685636b2d47a596cf26bbff5d0882d8cd66f5107a3c97e555`, identique au rapport Luna relu. Manifests release avant/après identiques au manifeste final111.
- Artefacts `/tmp/ai-guard-journal-context-qualified.j5GGRo` rehashés et modes0600 vérifiés : binaire `b884e84305f125cdaae610881ecc4e747f3ede0a5e634e4cf3391b5a01482ec6`, rlib `3c520d88d91cc2fd465d9435097c9c55cb02814ae18921a24ae31d10edaac670`, source.tar `132a63f665a3847679acca6279b16dfe4a14de7cb6eef346a16f63d18f6ff966`. Ces hashes correspondent au README et au suivi VALIDATION.

README, contrat BROKER_JOURNAL, CURRENT_STATE, DECISIONS, IMPLEMENTATION_PLAN, TEST_STRATEGY et VALIDATION distinguent correctement : fixture initiale erronée, RED comportemental à advance, rev0 initiale obligatoire, baseline batch/generic/None avec Store réel mais runtime synthétique, identité corroborée par le vrai producteur, six cas négatifs distincts, rollback refusé seulement dans le writer vivant et ses clones, ancien préfixe permis après reopen. La chronologie12PASS avant seul alias mécanique du type de test est explicite. Aucune preuve Linux, canari ou runtime antérieur n'est attribuée au nouveau binaire. Les deux anciens NOT_RUN sont complétés seulement à ce périmètre nommé.

La cartographie service-mapping est un document de conception future et une preuve de lecture du producteur, pas une livraison de service/IPC/scheduler. Les anciens dossiers de preuves restent historiques et inchangés. Une petite amélioration narrative non bloquante a été signalée au parent dans le plan actif : employer « attendaient avant correctif » pour le consommateur et sa fixture ; l'état corrigé est déjà explicite dans le README, le contrat, les décisions et les preuves.

**Verdict final : aucun défaut bloquant résiduel identifié dans le périmètre de cette revue.** La comparaison owned est corrigée conformément au producteur réel et les régressions adéquates sont constatées PASS par Luna ; leurs oracles et leur provenance ont été relus indépendamment. L'intégration de ce lot peut poursuivre son workflow normal sans transformer ce verdict en qualification d'exécution.

**Limites maintenues :** aucun worker/runtime réel, import/helper réel, variante fournisseur/TTY, Linux/Podman, modèle, E2E/navigateur, IPC/scheduler/gardien, panne électrique, reprise/fencing de service, capacité100 ou anti-restore après reopen qualifié par ce lot. G09 demeure FAIL, aucun gate global clos, aucune activation OS, cloud/PKI ou modification QA/facturation couverte par ce verdict.

## État

PASS statique : chaîne du vrai producteur et correction ciblée cohérentes.
NOT_RUN par reviewer : tous les tests et effets runtime.
Clôture code/oracles/documents/preuves : PASS ciblé de revue. Aucun verdict d'activation ou de qualification production. Root conserve la copie du rapport, régénération du manifeste des preuves et intégration/commit/push ; le reviewer n'a modifié que cette note externe privée0600.
