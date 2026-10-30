# Revue indépendante ciblée — lancement et reprise du broker

29 octobre 2026. Périmètre : sources figées du runner, de son runtime géré, du dispatch CLI et des tests indiqués par le coordinateur. Cette revue est partielle et en lecture seule. Aucun fichier du dépôt, secret, volume, service ou état moteur n’a été modifié. Aucun build, test, processus cible, navigateur ou endpoint de déploiement n’a été exécuté par ce reviewer.

## Verdict

Aucun nouveau P0/P1 de frontière de sécurité établi dans le snapshot relu. Un défaut P2 de résultat CLI a été confirmé, reproduit par le coordinateur puis corrigé pendant cette revue : `worker-resume` pouvait réaliser un premier lancement récupéré, obtenir un résultat d’échec, puis sortir avec succès. Le correctif et sa régression sont relus ci-dessous ; la validation de l’ensemble du lot n’est pas revendiquée.

Les écarts P1 déjà connus — durée autonome après SIGKILL du broker et restauration/fencing des claims — restent **OUVERTS**. Cette revue ne les redécouvre pas comme défauts nouveaux et ne les clôture pas. Le replay Store en cours de correction dans `src/audit/control.rs` est hors de ce périmètre et hors de ce verdict.

## Constat F1 traité — résultat CLI positif après échec d’un lancement repris

- Priorité : **P2**, correction fonctionnelle du contrat CLI. Aucun contournement de confinement, de tenant ou d’audit démontré par ce constat.
- Source : `src/operations.rs:290–302`, particulièrement la seule vérification `result["worker"]["state"] == "in_doubt"` à la ligne299.
- Chemin atteignable : les branches Reserved, import récupéré et Created récupéré de `src/runner/managed.rs:2052–2066`, `2108–2123` et `2152–2171` exécutent `launch_worker`, puis retournent directement la sérialisation d’un `RunSummary`.
- Préconditions : une admission autorisée interrompue avant son premier start ; reprise autorisée réussissant à reprendre ce premier lancement ; commande terminant avec `exit_code != 0`, timeout, nettoyage non confirmé ou erreur de post-traitement. `launch_worker` retourne normalement un `RunSummary` pour ces résultats observés ; seule une erreur du protocole quitte par `Err`.
- Résultat : un `RunSummary` direct ne contient pas de champ `worker`. L’accès JSON testé vaut donc Null ; le dispatch `WorkerResume` atteint `Ok(())`. L’automatisation reçoit un code CLI nul malgré le résultat d’échec présent dans le JSON. Le dispatch `Run`, aux lignes257–276, traite déjà explicitement ces quatre états comme des échecs CLI.
- Correction minimale : réutiliser la même qualification de `RunSummary` dans le dispatch `Run` et dans les branches `WorkerResume` qui ont effectivement lancé une commande. Une réponse typée évite de distinguer implicitement deux formes via un accès JSON manquant. Conserver l’impression du résultat observé avant le diagnostic.
- Régression attendue : étendre une faute `create-response-lost` existante avec une fixture qui observe ensuite un exit7 ; exiger un seul create/start et un seul ledger, le JSON conservé et un code CLI non nul pour `worker-resume`. Ajouter une variante cleanup non confirmé ou post-traitement refusé si ces cas partagent le helper de qualification.
- Validation initiale : **CONFIRMED_SOURCE**, puis **FAIL comportemental fourni** dans `/tmp/ai-guard-resume-exit-red.log` : la nouvelle régression observe exit7, puis échoue sur « a failed recovered command must not exit zero ».

Correctif relu : `src/operations.rs:112–153` partage maintenant `require_successful_run` entre Run et WorkerResume. La reprise valide le résultat direct et le résultat sauvegardé d’une enveloppe de statut ; InDoubt reste un échec, et l’absence de résultat récupérable reste inconnue sans fabrication de résumé. Le module runner est importé au niveau du fichier. Le nouveau test `recovered_first_start_reports_a_failed_command_as_cli_failure` (`tests/runner_recovery.rs:283`) garde le JSON exit7, exige un code CLI non nul pour le premier lancement repris et son replay, un seul create/start, un ledger unique et le quota libéré après terminal durable. La fixture lit un code terminal de test uniquement pour le worker, sans transformer les helpers en échec.

**PASS source + preuve ciblée fournie**, pas reproduction indépendante : `/tmp/ai-guard-resume-exit-green-final.log` contient 7 tests recovery réussis et 1 test terminal/matrice9 réussi, zéro échec et zéro ignoré. F1 est donc **TRAITÉ dans ce périmètre source et ces tests**. Les quatre états d’échec passent par le même helper de qualification ; la nouvelle régression de reprise exerce directement exit7. La suite globale et la qualification moteur après les dernières corrections restent à fournir par le coordinateur.

## Contrôles de sûreté constatés dans les sources

| Contrôle | Constat et portée |
|---|---|
| Checkpoints privés | `src/runner/managed.rs:52–285` ouvre les répertoires 0700 avec nofollow et CLOEXEC, opère par FD avec openat/renameat, exige des fichiers réguliers0600 possédés par euid et sans hardlink, borne les données, synchronise fichier et répertoire. L’archive est ouverte par FD puis son digest recoupé avant import/reprise. Ce sont des protections observées dans le code ; aucune épreuve OS de substitution n’a été exécutée ici. |
| Propriété exclusive de tentative | `:286–361` garde un flock exclusif non bloquant et l’identité dev/ino ; `locked_session` relit après acquisition (`:1878–1903`). Le checkpoint conserve cette identité. Les tests fournis ont un vrai contrôle multiprocessus CLI/flock et un test local de substitution du lock ; aucune preuve de fencing multi-hôtes n’en découle. |
| Tenant, gateway et autorité | `validate_checkpoint`, `load_session` et `check_identity` recoupent origine, racine privée, tenant authentifié, admission, commande, image, runtime et manifeste (`:499–547,1757–1846`). Le binding privé recoupe l’autorité runtime. Les labels d’instance servent au recoupement de l’artefact, jamais à sélectionner le tenant. |
| Create/Created avec ACK perdu | Le pending Created exact est conservé par `mark_unknown` (`:995–1034`) ; `reconcile_pending` rejoue sa même identité (`:594–625`). `recover_owned` conserve l’ID acquis ou emploie un inventaire ciblé strict (`:1904–1956`). `launch_worker` ne produit son premier start que via le callback Created et le claim durable (`:1241–1300`). |
| Claim irréversible | Le replay d’un pending StartClaim exige AlreadyConsumed (`:612–616`). Les branches de reprise ne démarrent pas si un claim durable ou un claim incertain existe (`:2147–2171`). Une instance Created sans preuve terminale garde son quota ; elle n’est pas relancée pour résoudre l’ambiguïté. |
| Reprise helper sans réimport | `recover_import` (`:2473–2598`) exige absence de création/start client, archive privée inchangée, même opération/helper, configuration/ID recoupés, terminal exit0. Après émission antérieure sans completion acquittée, le contenu réel du volume est comparé au manifeste via scan. Il ne rejoue pas le start d’un helper dont `start_issued` est durable. |
| Arrêt concurrent | `worker-stop` écrit une intention privée puis exige son reçu durable avant de tenter l’arrêt (`:2213–2325`). Le propriétaire actif observe Stopping et arrête son ID ; un second resume ne prend pas le verrou (`:1345–1367`). Les erreurs de verrou ne donnent pas un droit de takeover. |
| Terminal avant cleanup | `finish_execution` conserve le résultat réel, acquitte l’observation terminale et vérifie Terminal/quota libéré avant événement compatible et cleanup (`:1653–1726`). `finalize_recovered` compare le terminal au snapshot Created durable avec empreinte présente avant toute finalisation (`:1979–2016`). Le contrat d’intégration laissé ouvert dans la précédente revue runtime est donc constaté dans ce caller. |
| Résultat inconnu | Une reprise pure renvoie le registre, les faits runtime et des sorties inconnues après interruption ; elle ne reconstruit pas stdout/stderr ni un `RunSummary` fictif (`:1957–1978,2187–2209`). Une observation absente ou non terminale ne produit pas d’allocation zéro et ne libère pas la réservation. |
| Secrets et commande | `src/runner.rs:548–591` refuse l’injection d’un credential de contrôle connu et les identités/argv modifiées par masquage avant HTTP ; `private_credentials` refait les contrôles et le digest avant premier start récupéré (`managed.rs:1847–1863`). Le runtime reçoit programme et arguments séparés ; le fichier temporaire d’injection provider est transmis par son chemin, sans valeur dans argv. Les faits inspect persistés excluent Env et State.Error. Aucun secret n’a été lu/imprimé pour cette revue. |

La précédente revue avait signalé que `resume_terminal` s’auto-comparait dans la primitive runtime. Le caller relu utilise ici `observed.validate_terminal(kind, &owned.observation)` et exige l’empreinte Created. Cela résout le contrat du caller dans ce périmètre, sans modifier le verdict de la primitive publique pour d’autres callers éventuels.

## Limites de deux preuves de test

L1 — **Limite de preuve initiale, corrigée dans la source ; nouveau smoke réel NOT_RUN à ce stade** : l’ancien `tests/runtime_smoke.rs:225–226` cherchait `runtime::volume_name(denied_session)`, alors que le runner géré utilise le nom de tentative serveur `aig-…-work`. Le code modifié capture maintenant l’inventaire réel des volumes avant le refus et compare exactement les deux ensembles après, avec contrôle positif du volume admis et consigne de QA sans mutation concurrente des volumes. Cette correction traite le mauvais nom dans le test. Le smoke réel antérieur ne prouve pas l’exécution de cette nouvelle assertion.

L2 — **Fixture initialement moins fidèle, corrigée dans la source ; suite après cette correction NOT_RUN à ce stade** : la fixture ne reconnaissait que `name=^/…$`, alors que `ManagedRuntime::lookup_created` émet `name=^…$` (`src/runtime/managed.rs:586`). Le parsing relu reconnaît désormais `name=^`, retire un slash éventuel puis le suffixe `$`, et filtre sur le nom réel (`tests/runtime_fixtures/runner_terminal.rs:318–330`). Cela traite l’incohérence ; aucun bypass produit n’a été établi. Un cas avec un conteneur étranger resterait une preuve utile de sélection parmi plusieurs artefacts. La source primaire Moby accepte les deux formes de nom avec/sans slash ; aucune incompatibilité Docker n’a été déduite de la seule différence de fixture : [Moby daemon/list.go](https://raw.githubusercontent.com/moby/moby/master/daemon/list.go).

## Preuves fournies effectivement lues

- **PASS fourni, pas réexécuté** : `/tmp/ai-guard-runner-recovery-final-suite.log`, 201 tests réussis, zéro échec ; 3 tests moteurs ignorés. Le détail contient recovery 6, terminal 1 avec matrice de fautes, authorization 6. Ce journal est celui de l’intégration intermédiaire fournie ; les corrections ultérieures Store et F1/L1/L2 ne sont pas couvertes automatiquement.
- **PASS fourni, pas réexécuté** : `/tmp/ai-guard-runner-recovery-final-clippy.log`, clippy all-targets terminé sans erreur sur cette intégration intermédiaire.
- **PASS Docker fourni, pas réexécuté** : `/tmp/ai-guard-registry-qa.40IImn/smoke.log`, 1 test réel PASS en 10,82 s, zéro ignoré, copie/UID/export/refus et source initiale vérifiant ledger identique après deux resume. La limite L1 s’applique à l’ancienne sous-affirmation « aucun volume après refus » ; la nouvelle assertion réelle n’a pas encore été rejouée.
- **PASS Docker fourni, pas réexécuté** : `/tmp/ai-guard-workers-v2-real-terminal-initial.log`, 1 test réel PASS en 3,68 s, zéro ignoré : observation terminale refusée HTTP503, même conteneur terminal conservé, audit intact après réouverture, ledger absent et quota tenu. Il s’agit d’un refus HTTP ciblé, pas d’une panne disque ni d’une preuve générale de reprise.
- **FAIL puis PASS ciblés fournis pour F1**, pas réexécutés : `/tmp/ai-guard-resume-exit-red.log` et `/tmp/ai-guard-resume-exit-green-final.log`. Le fichier `/tmp/ai-guard-resume-exit-green.log` est un essai intermédiaire échouant à la compilation sur l’import de runner ; son nom ne vaut pas PASS et la source corrigée plus récente a été relue.
- Rapports de contexte lus : `/tmp/ai-guard-worker-runtime-report.md`, `/tmp/ai-guard-runner-recovery-report.md`, `/tmp/ai-guard-worker-runtime-review.md`. Leurs affirmations ont été comparées aux branches correspondantes, et ne servent pas de reproduction indépendante.

**NOT_RUN par ce reviewer** : toute exécution cible et tout moteur. La revue n’a pas établi le profil OS complet exigé par le skill pour exécuter du code cible ; l’inspection et les journaux suffisent aux constats source ci-dessus. **NOT_RUN / BLOCKED pour qualification globale** : Podman réel, matrice plateformes complète, reprise moteur sous faute Create/Created/claim, scénario A1/A2/B1, charge100, Kubernetes/cloud, PKI/NAS, fencing/restauration et release. Les fixtures Rust ne sont jamais comptées comme un moteur réel.

## Snapshot et méthode

Instructions lues : AGENTS.md, RTK.md, ARCHITECTURE_CONTRACT, CODING_RULES, PRODUCTION_READINESS, TEST_STRATEGY et contrats tenancy/usage/recette. Skill `/Users/louis/.agents/skills/security-audit/SKILL.md` appliqué en guidance ciblée sous le périmètre et le format de revue attribués par le coordinateur, sans full audit. Les skills projet `.agents/skills/ai-guard-security-review` et `.claude/skills/ai-guard-security-review` sont absents ; chargement automatique non confirmé.

`git status --short` montre les fichiers préexistants non suivis ; aucun diff de commit fiable n’est disponible. Les commandes du reviewer ont été des lectures `cat`, `sed`, `nl`, `rg`, `wc`, `git status`, des empreintes `shasum` et une consultation de source primaire publique. Le seul fichier écrit est ce rapport externe, à la destination attribuée.

Empreintes SHA256 du snapshot initial ayant fondé les constats :

```text
50c88cd08de65a81e7bc4946ee750453b29305aafb4370a795962ab24fb9920d  src/runner.rs
171708ec584a33bd70cc56dcacf25df8ce177b14c91e018973c7f3f6b33d7c21  src/runner/managed.rs
21cbb8f09a8103e275282457672c5c60dcf8b8abaf6981f567328a9c82e87dba  src/operations.rs
ae5e0cba0533b166059c10232351294f6d8efb6ad6df0a46184a547cdb8c1a3d  src/runtime.rs
b96427b3cc1b407aac35542e66a66ed0f891d2607055a39184791d15fe33e61d  src/runtime/managed.rs
e3de49e48fbb9e83a86f317a1829e0d93d53a08e9f9f3253cd7e8bb4d6b0c753  tests/runner_recovery.rs
17dc22150d8620595c1f14706c7a1b1ac94eab5bf9fbfd271499f7020b9e49da  tests/runner_terminal_audit.rs
7e49abf079b4f744c81c2e65906766eb29f5b4f2b8b4ab6f5ff2eb323c11d21e  tests/runtime_terminal_persistence.rs
9a05ffb19fe7730fcb662fa0ac583424ee36d30643a2d35696e2869c48c87440  tests/runtime_authorization.rs
94383841680f9200413bed6864c7bfcb6725ae82e9a0e97092fa615b52fe5232  tests/runtime_smoke.rs
7225c0c0dedefdd7b84d01733af5d0f3bde35859685df331ebc8b055cac04f02  tests/runtime_fixtures/runner_terminal.rs
```

Corrections F1/L1/L2 relues ensuite, empreintes les plus récentes des quatre fichiers modifiés par le coordinateur :

```text
646d3cbde67683459c279b704320ac80734166ab42c702eff8688ba847ebc982  src/operations.rs
d34d70a8a8ef0aab35c0e84191ecf6f79168885c902aab4ef516c45513b4414e  tests/runner_recovery.rs
18d00026097804c2fc6dad7f94dfa8285253c1276c004f21bbcd00b68ba1d8e7  tests/runtime_smoke.rs
77bc7f2a93efb58db4dc81fb786b6c35eb4b4a25f361b273ebad4470ce43ce03  tests/runtime_fixtures/runner_terminal.rs
```

Les autres sources du périmètre conservent leurs empreintes initiales lors de cette vérification. Le verdict porte sur ces empreintes et ces chemins. Il ne qualifie ni le produit complet, ni un gate global, ni les sources modifiées ensuite.
