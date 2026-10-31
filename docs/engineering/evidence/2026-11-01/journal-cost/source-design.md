# P1 — point de mesure du coût durable Journal/Writer

Date : 1 novembre 2026. Inspection en lecture seule de `/Users/louis/Documents/ai-guard/.worktrees/broker-service`, HEAD vérifié `b32711dcebbc0aea120f4aee88362db14428cb7a`. État Git initial vide. Aucun changement applicatif, test, Cargo, moteur, processus gardien, QA, credential, claim, volume ou fournisseur exécuté/interrogé. Cette note privée est l'unique livrable écrit. Elle n'établit aucune cause de H10 et ne propose aucune optimisation.

## Références lues

AGENTS.md et RTK.md ; `docs/engineering/{ARCHITECTURE_CONTRACT,CODING_RULES,PRODUCTION_READINESS,TEST_STRATEGY}.md` ; suite H10 de `IMPLEMENTATION_PLAN.md:91-116` ; `broker-runtime/h10/core-diagnostic.md` et, uniquement comme contexte imposé, `d5-source/retirement-analysis.md`. Le mandat reste Rust hors front, barrières durables avant ACK, refus en cas d'intégrité/audit inconnus, préservation des ressources existantes, tests exécutés par Luna et aucun nouvel essai moteur/fournisseur/E2E. D5 est hors du périmètre de cette note.

## Ce que le vrai code fait

### Load et status d'un job accepté

`WriterClient::load/status` (`src/broker/writer.rs:529/548`) crée une commande typée, `ClientState::call` tente `sync_channel::try_send`, puis attend un oneshot avec timeout (`:232-252`). Le timeout du caller ne retire pas une commande déjà acceptée. `serve_backend` exécute le Journal dans l'unique thread writer (`:674-816`), puis `reply.send` livre le résultat.

`Journal::load` (`src/broker/journal.rs:824`) et `Journal::status` (`:572`) prennent le mutex partagé, appellent `check_writer` puis `status_locked`. Pour un accepté, `status_locked` (`:581`) inventorie une première fois la portée par `usage`, lit accepted/rejected, puis appelle `load_locked`. Il n'y a pas de raccourci de lecture pour status.

`load_locked` (`:1325`) refait `usage`, relit accepted/rejected, puis :

1. lit, valide et synchronise reservation puis accepted par `read` (`:1359-1368`, `:1488-1500`) ;
2. lit intent, contrôle secrets/format canonique/hash/contexte, synchronise son FD (`:1369-1385`) ;
3. lit bindings, vérifie format canonique et digest, synchronise son FD, puis recharge le binding retenu (`:1386-1400`) ;
4. lit et synchronise current par `read`, valide sa version, son identité, sa limite et son hash (`:1401-1411`) ;
5. lit **toutes les révisions 0..=head**, appelle `ChainVerifierV1::push` pour chacune et synchronise chaque FD (`:1412-1423`) ;
6. `finish` vérifie le head exact, le floor vivant doit encore appartenir à la chaîne, puis synchronise job, jobs et namespace, et met à jour floor/observed_jobs (`:1424-1443`).

`ChainVerifierV1::push` (`src/broker/journal/confirmed.rs:56`) contrôle les secrets, parse le JSON/version, reconstruit la forme canonique, recoupe operation/intent/revision/previous, valide chaque contexte/transition, recalcule le digest et reconstruit les reçus historiques. Il ne fait pas d'I/O lui-même. `usage` (`journal.rs:1569`) est un inventaire de métadonnées de **tous les jobs du namespace**, pas seulement du job sélectionné ; le paramètre selected gouverne le budget actif. Il ne lit pas les payloads des fichiers.

`check_writer` (`journal.rs:1461`) revalide le FD de lock et le directory, relit scope.json, recoupe son contenu canonique et les secrets. Ces opérations s'ajoutent au Full load.

### Les synchronisations contiennent d'autres lectures

`Directory::persistent_read` (`src/control_fs/persistent.rs:130`) fait une admission metadata/exclusions avant ouverture, lit les bytes bornés avec `read_to_end`, contrôle metadata/digest, puis appelle `persistent_revalidate`.

`PrivateFile::persistent_revalidate` (`persistent.rs:583`) revalide le parent, les metadata du FD, le hash du cache, **relit tout le FD retenu via read_at** et le rehache, recoupe les metadata après lecture puis l'identité du nom dans le parent. Il n'ouvre pas un nouveau FD vers le fichier source. Le parent revalidé fait `Anchor::revalidate -> walk` de ses ancêtres (`src/control_fs.rs:97`).

`PrivateFile::persistent_sync` (`persistent.rs:627`) fait revalidate, `File::sync_all`, puis revalidate. Son temps inclusif n'est donc pas le temps du seul appel sync_all. `Directory::sync` (`control_fs.rs:248`) fait revalidate puis `File::sync_all`. Aucun syscall particulier sous-jacent, notamment F_FULLFSYNC, n'a été observé par cette inspection : seuls les appels Rust sont établis.

## Mutation représentative déjà présente

Pour reproduire le coût de l'erreur locale observée dans H10 sans moteur, `WriterClient::append_command` avec `LocalCommandRequestV1::FailureObserved { phase: Worker, code: Unavailable }` est la couture existante (`writer.rs:577`, `:821`). Ce choix ne prétend pas reproduire la cause de H10. Il crée une vraie révision lifecycle et retourne son reçu local ; ce reçu n'est pas un ACK central HTTP.

Chemin neuf : `append_command -> Journal::load -> LocalCommandV1::new -> BrokerCheckpointV3::new -> advance_lifecycle -> Journal::advance_lifecycle_preserving_outbox -> status_locked -> load_locked -> advance_loaded_checkpoint -> publish_revision -> load_locked -> Journal::command_receipt -> status_locked -> load_locked`.

`advance_lifecycle_preserving_outbox` (`journal.rs:1018`) conserve le mutex du preload strict jusqu'au postload, les événements V4 et les règles de head/IDs. `advance_loaded_checkpoint` (`:1068`) vérifie contexte, encodage, CAS/replay et transition, inventorie le budget, publie, fixe immédiatement le floor conservateur, puis effectue un **postload strict complet** avant `BeforeAck`. Une erreur de revalidation après publication ne devient pas un ACK.

`publish_revision` (`journal.rs:291`) crée la nouvelle révision exclusivement et synchronise fichier+job ; crée current-next et le synchronise ; relit current et recoupe expected ; publie le pointeur via `persistent_publish` ; puis synchronise job. `Directory::persistent_publish` (`persistent.rs:234`) **resynchronise current-next**, revalide le FD ancien, fait le rename ciblé, vérifie le nouvel inode, synchronise le directory et vérifie encore le nom. Cette publication comprend donc trois appels persistent_sync et trois Directory::sync au total. Aucun de ces doublons apparents n'est déclaré supprimable.

## Décomptes source à confronter aux mesures

Hypothèses fermées pour ce tableau : succès sans conflit, un seul job accepté, head initial r, BindingInputs::None et aucun sous-répertoire bindings présent, aucune faute activée. Un Full load à head r contient r+1 révisions. Ces nombres sont dérivés du contrôle de flux ; ils ne sont pas des mesures.

| Appel | Full loads | persistent_sync | Directory::sync |
|---|---:|---:|---:|
| Journal/Writer load ou status accepté à r | 1 | r+6 | 3 |
| Writer advance_lifecycle r→r+1 jusqu'au reçu local | 2 | 2r+16 | 9 |
| executor commit_patch r→r+1 jusqu'au reçu | 3 | 3r+22 | 12 |
| Writer append_command neuf r→r+1 jusqu'au reçu local | 4 | 4r+29 | 15 |

Pour le premier rang, les cinq fichiers fixes synchronisés sont reservation/accepted/intent/bindings/current ; les r+1 autres sont les révisions. Le `load_binding` None ne synchronise rien si son sous-directory n'existe pas. Un binding Manual/Provider ajoute le FD du blob, ses validations réelles et son directory ; un bindings vide existant ajoute déjà un directory sync (`journal.rs:1231-1323`). Les chemins d'erreur/replay/conflit ont des décomptes différents.

`commit_patch` ajoute le caller `load_draft` au Writer advance. Un append neuf ajoute encore la lecture du reçu historique. `enqueue_events` et `acknowledge_events` (`writer.rs:908/949`) font eux aussi leur load, un advance_outbox avec preload/postload, puis un load pour reconstruire les reçus : ce sont de bons seconds probes si le premier coût ne suffit pas à répondre. Leur ACK avec un seq arbitraire fixture ne doit jamais être présenté comme un échange HTTP central réel.

## Harnais minimal recommandé — deux fichiers, aucun changement de comportement

Fichiers proposés : `src/control_fs.rs` et `src/control_fs/persistent.rs` uniquement. Dans le premier, ajouter le collecteur cfg(test) et les points de chronométrage du vrai `Directory::sync` / `Anchor::revalidate`. Dans le second, ajouter les points de mesure des vraies méthodes persistent et un test opt-in unique collé à ce module. Le test construit le vrai Journal/Writer avec le JSON existant `tests/fixtures/broker_intent-plan-v3.json` et les mêmes transformations legacy/generic/None que les fixtures déjà présentes ; aucune nouvelle abstraction de stockage, aucun mock, service, runtime moteur ou cache.

Référence de fixture la plus sobre : `tests/broker_outbox.rs::Fixture::{new,accept}` (`:37-96`) ; elle crée un répertoire privé neuf, Journal réel, Writer réel (queue16/timeout10s), scope synthétique et BindingInputs::None, sans gateway/enforcer. Ses helpers sont privés à leur cible : on ne peut pas les appeler depuis un test lib sans modifier leur visibilité/déplacer du code. Recopier les quelques lignes de construction nécessaires dans le test de mesure est moins intrusif pour ce lot borné qu'une extraction commune. La fixture executor existante (`executor/journal_tests.rs::Fixture`) crée Gateway/SQLite et est donc superflue ici.

La mesure qui traverse le Writer doit vivre sur le thread qui effectue réellement le Journal. Un thread_local ouvert sur le thread async caller ne verra **aucune** opération du writer. Avec les deux fichiers FS seulement, utiliser un collecteur cfg(test) de processus, activé pour la racine privée unique du test et une seule étape à la fois ; filtrer en mémoire les handles par cette racine, sans jamais la publier. Les commandes du test restent séquentielles. Ce filtre évite la contamination d'autres tests même en cas d'exécution accidentelle concurrente, et conserve le vrai passage de queue. L'alternative thread_local exige un hook d'installation/retour dans `writer::serve_backend` et un troisième fichier ; ne pas ajouter ce hook par réflexe.

Séquence la plus courte :

1. Construire/accepter le job hors mesure (r=0) ; garder son head attendu.
2. Capturer séparément deux `writer.load(operation)` au même head, puis `writer.status(operation)`, sans concurrence. Vérifier résultat accepté et head identique après la capture. Les deux loads sont une répétition limitée pour constater le coût réexécuté, pas une distribution de performance.
3. Capturer un `writer.append_command(FailureObserved)` neuf jusqu'au reçu retourné et vérifier ensuite r+1, commande exacte et état non terminal. Deuxième commande hors fenêtre mesurée facultative si un second head est nécessaire.
4. Seulement si utile, une capture load au nouveau head pour vérifier le delta d'une révision. Le test n'a pas besoin de croître jusqu'aux31révisions de H10 pour établir le nombre d'opérations par révision.

Le timeout du Writer reste celui de la fixture. Un timeout mesure une issue caller inconnue ; il n'autorise ni retry aveugle ni rapport tronqué des I/O comme coût complet. En ce cas la campagne est FAIL/BLOCKED pour la mesure de cette étape, avec sa borne constatée. Aucun allongement du test/runtime n'est inclus dans cette note.

Champs de sortie fermés : étape, head_before/head_after numériques, durée caller en ns, outcome fermé, nombre de calls/succès/erreurs, bytes numériques et durées pour chaque primitive. Aucun UUID, chemin, scope, tenant, command/request, payload, argv, credential, erreur libre, contenu ou hash de contenu. Les erreurs se résument à un code fermé. Log et manifeste privés mode0600.

Points à séparer dans le collecteur :

- `persistent_read` inclusif, nombre/bytes et temps du vrai `read_to_end` ;
- `persistent_revalidate` inclusif, nombre/bytes et temps des vrais appels `read_at` ;
- `persistent_sync` inclusif **et** span exact de `file.sync_all`, chacun avec appels/succès/erreurs ;
- `Directory::sync` inclusif **et** span exact de son `file.sync_all` ;
- `Anchor::revalidate` inclusif pour rendre visible le coût de walk/metadata/ACL des ancêtres ;
- `persistent_create/persistent_publish` appels/durées/bytes et nombre de publication réellement atteinte.

Les temps inclusifs sont imbriqués : ne pas les additionner. La durée caller comprend queue, reconstruction/validation de chaîne, allocations et livraison ; le résidu après soustraction de spans I/O exclusifs n'est **pas** une mesure de CPU de ChainVerifier. Sans instrumentation spécifique du Journal, la reconstruction canonique/contextuelle reste une rubrique non isolée, explicitement NOT_MEASURED. Si elle domine le résidu, la prochaine mesure ciblée sera alors justifiée ; elle n'est pas anticipée par un nouveau validateur ou cache.

## Décision Root reçue après la cartographie

Root retient une implantation plus lisible à **quatre fichiers**, à la place de la variante minimale à deux : nouveau `src/control_fs/cost_probe.rs` exclusivement cfg(test), hooks dans `src/control_fs.rs` et `src/control_fs/persistent.rs`, et test ignoré unique dans `src/broker/journal/tests.rs` réutilisant directement `open/intent_for`. Aucun élargissement de visibilité ni duplication de fixture n'est alors nécessaire. Collecteur global RAII, une mesure activée à la fois, aucune activation hors de ce test. Aucun payload/path/hash/CID dans la sortie. Les appels normaux restent strictement identiques.

Le premier échantillon borné porte sur heads0 et8, avec deux Writer.load au même head, status et une `WriterClient::advance_lifecycle` neuve jusqu'au reçu local. Les mesures vérifient vrai head/CAS/ACK et compteurs non nuls ; aucune limite arbitraire de vitesse ne constitue un oracle PASS. Pour BindingInputs::None sans sousdirectory bindings, les décomptes source attendus sont6 puis14 file-sync pour load/status et16 puis32 pour l'advance mesurée ; les dirs restent respectivement3 et9. Construire les huit révisions est hors fenêtre mesurée, sur la même fixture neuve, avec les vraies transitions, jamais par fabrication d'une chaîne de fichiers. Aucun GO d'édition source n'est inclus dans ce retour.

## Critères d'acceptation et suite Luna

- Une seule cible lib exacte, opt-in/ignorée pour ne pas imposer un benchmark à chaque test ; Luna exécute ce test explicitement, en release, `--exact --ignored --nocapture --test-threads=1`. Un test ignoré par défaut ne vaut pas PASS. Une borne totale explicite est définie par l'implémenteur avant lancement.
- Vrai filesystem privé neuf, vrai Journal, vrai Writer, aucun runtime/enforcer/fournisseur ; versions/OS/architecture/FS effectivement observés consignés. Ne pas annoncer de disque froid, durabilité électrique ou syscall interne non observé.
- Les counts pour deux loads au même head et status concordent avec le contrôle de flux ; la mutation retourne un reçu et une vraie nouvelle révision. Les décomptes source ci-dessus sont vérifiés, ou leur écart expliqué à la ligne réelle.
- File-sync exact, revalidation et lecture sont distingués ; aucune addition de durées imbriquées, aucune latence percentile extrapolée de quelques samples, aucune cause H10 inférée.
- Le probe est strictement cfg(test), ne change aucun ordre/erreur/valeur des appels réels, ne saute aucune opération et garde Full pré/post, CAS, floor vivant, exclusions et FD bindings. Aucune création de nouveau store ni cache.
- Un second regard relit les hooks placés autour des véritables appels, l'absence de fuite dans la sortie, les différences de code hors cfg(test), et la cohérence résultat/compteurs. Les canaris existants strict-postload/binding-loss/floor/CAS sont sélectionnés par risque par Root/Luna ; aucune matrice entière répétée ici.

Statut actuel : PASS pour cette cartographie statique et l'identification du harnais ; NOT_RUN pour toute mesure/test et pour les décomptes empiriques ; NOT_APPLICABLE pour moteurs/fournisseurs/E2E dans ce lot de mesure ; aucune qualification de G09, D5 ou production obtenue.
