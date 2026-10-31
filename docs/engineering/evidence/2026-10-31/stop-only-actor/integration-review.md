# Revue indépendante STOP-only — état WIP puis gel

31 octobre 2026, worktree `broker-service`, base `44324a7`. Lecture seule du core Journal/recovery/Writer et de la glue server/jobs/protocol/CLI/fanout. Aucun Cargo, test, moteur, fournisseur, environnement privé ou infrastructure exécuté ; aucune source modifiée. **Cette première section porte sur des lectures WIP, pas sur un gel qualifié.** L’orchestrateur a transmis les deux risques core à leur auteur ; relecture du correctif et hashes de gel encore attendus.

## Constats core à fermer avant gel

### P1 de contrat — plusieurs noyaux/queues possibles sous le même FD de lease

Source WIP : `journal/recovery.rs::admit_report(&StopRecoveryReportV1, ...)` recrée à chaque appel `Arc<Inner>` et `Mutex<Writer>` depuis les champs du même rapport ; `writer.rs::ProfileWriterCapabilities::from_recovery_report` public reçoit seulement un emprunt de ce rapport et démarre une nouvelle queue/thread. Le lock PrivateFile est cloné comme prévu, mais ce partage de FD ne protège pas deux Mutex/floors distincts.

Reproduction ciblée sans moteur : scanner un rapport existant une fois ; appeler deux fois from_recovery_report sur ce même rapport ; constater deux factories réussies. Elles peuvent ensuite budgéter simultanément deux jobs différents avant publication et perdre la sérialisation du budget de scope ; leurs floors ne sont pas non plus le même état vivant. Il faut une admission consommée/claim unique du rapport, ou un noyau unique réellement partagé, et empêcher de reprendre le même rapport force_stops=true puis false pour fabriquer une seconde capacité Full. La once-cell du service protège son chemin normal, mais ne ferme pas le contrat de la factory réutilisable elle-même.

Canari requis : seconde admission refusée, ou même cœur/Mutex garanti ; lease toujours Busy pour une ouverture indépendante jusqu’au dernier handle, sans fenêtre unlock/relock. Si deux façades sont autorisées, démontrer qu’elles partagent aussi les floors et la sérialisation du budget, pas seulement le FD.

### P1 de contrat à vérifier — admission Full existante moins durable que l’ancien open

Source WIP : `admit_report` construit la façade Full puis ne fait que check_writer et usage(None). Le reader de démarrage a synchronize_reads=false. Les persistent_sync du scope/lock et les barrières d’ancêtres effectuées dans Journal::open ne sont pas retracées dans ce nouveau chemin avant restitution de Full. Le plan promettait une admission Full strictement conservée sous la lease déjà acquise, sans re-flock.

La revue ne prétend pas avoir reproduit une panne électrique ou une perte de fichier ; il s’agit d’un écart de chemin de synchronisation à fermer/tester. Canari : observer après les vrais sync scope/parents les dev/ino, injecter une faute après barrière et vérifier qu’aucune façade Full n’est rendue sur échec ; StopOnly dégradé peut rester disponible. Conserver la lease et les arrêts prouvés. Le hook seul, sans vrai syscall avant lui, ne prouverait pas ces barrières.

## Glue WIP — contrôles retracés

- `dispatch` authentifie chaque requête avant validate_registry/profile_writers : rôle runner, tenant et namespace configurés, schéma3 et protocole restent exigés. Cancel et RecoveryStatus n’exigent plus active ; les mutations métier continuent à l’exiger et passent par material/require_starts. Aucun bypass de périmètre trouvé à cette lecture.
- `profile_writers(..., false)` utilise les ouvertures existantes et ne crée pas de namespace pour Cancel/RecoveryStatus. La façade Stops n’expose ni full/load ni mutation générique ; le backend refuse aussi les commandes métier.
- Nouveau Cancel : génération invalidée avant le travail journal de la demande, identity_gate pour collision externe et append, puis fanout des seules cibles causales. Replay confirmé ne réinvalide pas et ne réappend pas. Unknown devient un état distinct, retry du même UUID sans nouvelle invalidation, sous la même queue ; NotQueued et Unknown restent distingués.
- Full.advance_checkpoint conserve son load_locked avant ET après publish_revision, avec contrôle de head final. Le test utile de post-load doit renommer le blob de fixture au hook CurrentDurable, après publication réelle : Full refuse le reçu, la façade STOP voit ensuite le Cancel et son floor. BeforeAck serait trop tard pour tester ce cas.
- Le publisher et le budget STOP sont communs/bornés : namespace partiel mais tailles totalement comptables peut muter ; entrée inaccessible, troncature ou sous-dossier inconnu refuse le nouveau Cancel. Rejouer un C1 déjà confirmé ne doit pas dépendre de ce budget, afin de conserver les arrêts préarmés.
- RecoveryStatus est un DTO v1 séparé : provenance ConfirmedJournal, runtime NotQueried, head, commandes ordonnées, Arms connus et pending, mode Full/StopOnly et incomplete. Les anciennes formes Status/List restent séparées ; aucune observation runtime ni executor_status n’est inventé.

## Vérifications encore manquantes au relevé WIP

**Auth réelle ciblée demandée à Root :** les nouvelles fixtures privées annoncent l’authentification hors de leur périmètre. Ajouter un vrai gateway Axum/Store loopback : runner A valide avec schema3/protocole et active=false peut atteindre Cancel/RecoveryStatus ; Submit/Resume/Approve restent refusés ; viewer, B, mauvais namespace, schéma/protocole incompatibles et credential manquant sont refusés avant toute création FS. Deux appels valides doivent provoquer deux contrôles identity frais. Aucun fournisseur ou moteur nécessaire.

**Factories/budget/fautes :** admission unique ; lease sans gap ; partial-comptable vs budget inconnu ; faute post-publication Full et résolution exacte Unknown ; stable C1/C2/ACK tardif et export ultérieur. L’exécution Luna et les preuves doivent être rattachées au prochain gel, sans convertir les RED en PASS par documentation.

**Portée du diagnostic :** Full désigne une capacité de façade, pas la preuve que chaque blob métier charge encore. reconciliation_required est calculé sur StopOnly/incomplete, pas une mesure de santé globale/enforcer ; false ne doit pas être présenté comme autorisation de démarrer. Un ancien serveur V1 rejettera la nouvelle variante JSON comme InvalidInput via son parseur ; le CLI actuel affiche ce code, sans code Unsupported distinct. Documenter cette compatibilité honnêtement.

## Captures WIP et limites

Hashes relevés pendant la lecture : journal `5a88d024…b6f9b0`, recovery `46be3841…e6bcc8`, writer `cfb64b31…ced7a`, server `058f2ea8…c6a1f`, jobs `e3150295…ef8cb`, protocol `1f79cb9a…8c0457`, CLI `eeb570b0…aa355c`, executor `7848d29b…7b8d4`. Ces fichiers étaient encore en cours d’édition : aucune conclusion finale sur le gel n’est revendiquée.

**NOT_RUN par le reviewer :** toutes les commandes et fautes, service réel, runtime, autres UID/plateformes. Les nouveaux résultats et la clôture des constats seront inscrits après gel relu ; aucun gate de production n’est fermé par ce rapport.

## Relecture du gel core — deux constats initiaux corrigés en source

Gel communiqué ensuite par Root : `journal.rs` SHA-256 `894c4a2d71041c71b72f69ea5666673b023a9728e048cd2784b3c209c9ff247b`, `journal/recovery.rs` `cf4edac96c6dc83ebcc54d7a4c497136febca22b8946139a503b2ca57430c4d6`, `writer.rs` `cfb64b31843031e4bbc607ba3089b916936b7cca8362f5e99927a37d390ced7a`. Hashes relus conformes. Les constats WIP ci-dessus sont conservés comme historique, pas comme défauts encore présents dans ces chemins corrigés.

- **Admission unique corrigée — `recovery.rs:531`.** actor_claimed.compare_exchange(false,true) précède la construction du noyau. Seconde admission du même rapport : Busy, quelle que soit la demande Full/Stops. Une erreur avant restitution du core remet le claim à false ; le rapport garde le même Arc lock, sans unlock/re-flock. Le canari `tests/broker_writer.rs:47` exige seconde factory Busy, dev/ino lock inchangés, ouverture indépendante Busy après drop du rapport et des capacités tant qu’une façade Stops subsiste, puis libération après dernier handle/queue drain.
- **Barrières d’admission Full corrigées — `recovery.rs:594`.** Avant de restituer Full : vrai durable_bootstrap du namespace et de ses ancêtres, persistent_sync du lock, lecture/revalidation et persistent_sync du scope, sync scope/jobs/scope puis contrôle partagé. Le hook NamespaceDurable arrive après ces opérations réelles. Le canari `journal/tests.rs:278` injecte ensuite Io, exige aucune façade Full, lease toujours Busy et retry exact admis une seule fois.
- **Strict Full post-publication conservé.** Le canari `journal/tests.rs:210` renomme le blob manuel de fixture uniquement au hook CurrentDurable, après le vrai publisher et sync. Il exige refus du reçu Full malgré current révision2, octets préservés dans une sauvegarde et lecture métier toujours refusée ; le snapshot STOP confirme révision2 avec incomplete=true. Ce test porte sur une publication Full, pas sur une panne électrique.
- **Budget incomptable et C1 durable distingués.** Le test cold-route traite d’abord un sibling regular privé entièrement comptable, puis ajoute un sous-dossier inconnu. Le nouvel append est refusé UnsafeFilesystem sans changer current ; le même Cancel déjà durable conserve son premier reçu et sa cible STOP, avec incomplete=true. Le diagnostic reste StopOnly/NotQueried et ne fabrique pas de terminal.

Hashes des canaris lus à ce stade : `tests/broker_writer.rs` `e5d634044ad761ed11075448871c29e99e895c89c4d2322e427bde0633a6cacc` ; `journal/tests.rs` `818d0e1d9f8a944a992e790cb2a6adb0f51051b0349473c48c62eda3c2f09c32`. Le fichier de tests service reçoit encore l’inclusion d’auth ; son gel final sera enregistré séparément.

**Résultat actuel :** les deux P1 initiaux sont fermés au niveau source et leurs régressions sont écrites. Leur validation exécutée est encore attendue de Luna ; aucun PASS de test n’est attribué par le reviewer. Auth réelle et un point supplémentaire de continuité des rejets au handoff restent à relire/résoudre avant conclusion finale.

### Point supplémentaire transmis — rejets lus pendant la même lease

Le rapport conserve observed_jobs et les heads accepted, mais read_job retourne None pour un rejet confirmé sans mémoriser son hash dans le rapport. admit_claimed_report reconstruit alors rejected_floors par défaut. À vérifier : scan d’un rejet → même lease transférée → substitution de fixture accepted valide ou rejet divergent → la façade doit conserver le Conflict attendu d’une observation vivante, pas seulement protéger la disparition du job. Il ne s’agit pas d’une reprise après fermeture/réouverture, où les floors mémoire sont historiquement réinitialisés. Aucun test ni exploitation externe exécuté ; qualification de ce point en attente du retour de l’orchestrateur et d’un canari ciblé.

## Revue finale de source — gel commun communiqué ensuite

**Les trois écarts de contrat signalés pendant cette revue sont corrigés dans la source gelée ; aucun autre P0/P1 confirmé à cette lecture ciblée. Les tests sont écrits, mais leurs résultats exécutés restent attendus de Luna. Cette conclusion ne qualifie ni le service complet ni une release.**

Le troisième point est fermé dans `journal/recovery.rs` : le reader calcule le hash du rejet canonique déjà validé, le scan le conserve dans le rapport, `admit_claimed_report` le transfère au même Writer continu, et `read_locked` compare classe/hash avant de rendre Interrupted. Un rejet observé ne peut donc pas devenir un accepted, un rejet divergent ou une disparition sans Conflict sous cette lease. `tests/broker_writer.rs::recovery_handoff_retains_the_hash_of_an_already_observed_rejection` modifie vers un autre code valide après scan/transfert et exige Conflict dans Full.status ET Stops.recovery_status. Ce canari ne dépend pas d’un JSON invalide pour réussir.

Les deux premiers correctifs restent inchangés : claim unique par rapport, et vraies barrières d’admission Full sous le FD déjà retenu. Aucun chemin public Stops→Full, load métier ou mutation arbitraire n’a été ajouté à StopWriterClient/StopJournal. L’admission Full n’est produite que par la factory interne avant restitution de la capacité ; une admission Stops réussie consomme aussi le claim.

### Auth et protocole relus après leur gel

`stop_auth_tests.rs` utilise un vrai gateway Axum/Store sur loopback. L’état schema3 inactif est obtenu par création supervisée puis close/reopen, sans PRAGMA ni réponse identity fabriquée. Le compteur HTTP prouve une nouvelle vérification pour chaque requête de forme valide. Runner A inactive peut demander RecoveryStatus, persister/livrer Cancel puis le rejouer ; Submit/Resume/Approve restent IncompatibleControl. Viewer, runner B, credential inconnu, namespace discordant et schéma2 sont refusés ; credential absent est refusé localement sans HTTP. Les négatifs vérifient l’absence de publication de namespace, pas l’absence de toute lecture invisible, limite honnêtement annoncée dans le test. Le test schema2 couvre ensemble l’incompatibilité schéma/protocole réellement exposée par ce Store.

Le DTO RecoveryStatus reste séparé, strict et explicite sur ConfirmedJournal/NotQueried, mode et couverture. Les anciennes formes Status/List ne sont pas transformées en statuts partiels inventés. La documentation doit conserver les précisions déjà transmises : Full décrit la façade disponible ; un ancien serveur rejetant la nouvelle variante peut répondre InvalidInput, sans code Unsupported spécifique.

### Queue Cancel : la lacune de preuve WIP a son canari dédié

`writer.rs::tests::stop_append_unknown_retains_queued_identity_and_replays_once` prépare un vrai job Journal, pause une queue de capacité1, exige Unknown pour le premier Cancel déjà enqueued et vérifie son identité conservée. Le second UUID face à la queue pleine rend NotQueued avec son input ; aucun Cancel n’existe encore. Après drainage réel par le writer, le premier UUID rejoué possède une seule révision et une seule commande ; le second est ensuite soumis une fois et devient la deuxième révision. Ce test porte bien sur AppendCancel, distinct des anciens tests Reject. Les assertions Job Unknown et les canaris post-publication, budget inconnu, C1/C2 et cold-route gardent leurs portées propres.

### SHA-256 du gel final effectivement relu

- `src/broker/journal.rs` : `894c4a2d71041c71b72f69ea5666673b023a9728e048cd2784b3c209c9ff247b`.
- `src/broker/journal/recovery.rs` : `964b93c7b584d922842cd7f16fc405efa7bd3ff069133060229830fb4299170a`.
- `src/broker/writer.rs` : `2d62eafdc825d1cb1032aaf979c00724bc3c11da616ee6c7217973166dba5310`.
- `src/broker/service/server.rs` : `058f2ea8f939d1115e29e365b3bea4cb660afb50e492a2996dda0d1eee0c6a1f`.
- `src/broker/service/jobs.rs` : `e3150295540381d4942108197f68ff4c666f0e5a7de653dc82632b79a55ef8cb`.
- `src/broker/service/protocol.rs` : `1f79cb9ac4f42be8eca671f5c0ddcef8408c37d8d9e8cf2c1a3bfc682f8c0457`.
- `src/broker_cli.rs` : `eeb570b0ff43cda8af11e19606972e5c5bd864797cc40cc3515a3241eeaa355c`.
- `src/runner/broker/executor.rs` : `7848d29b18554cb4367a5c03961ddda2331de67a38f98c09708568916c47b8d4`.
- `src/broker/service/stop_auth_tests.rs` : `dffa2df08c2f6d1aee2969d1ffefaa528337b3ea77070b06130fc8a65b8f106f`.
- `src/broker/service/cancel_retry_tests.rs` : `718313ec0913c0138cfdf5e4d326dbd27fb2077ed21e73125ce3720b28edc26e`.
- `src/broker/journal/tests.rs` : `818d0e1d9f8a944a992e790cb2a6adb0f51051b0349473c48c62eda3c2f09c32`.
- `tests/broker_writer.rs` : `fe5bc779f36f887217990c16092c5e7ff90b07d6078b8795c07003169da9f8f9`.

Aucune commande Cargo ou exécution produit par le reviewer. Aucun PASS de test attribué sans logs ; les preuves Luna et leurs éventuels FAIL/NOT_RUN doivent accompagner ce gel. Les tests de routage emploient une seam de livraison ; ils ne démontrent ni Docker/Podman, ni arrêt physique, ni durabilité face à panne électrique. Les limites de portée ne suppriment aucun gate obligatoire du produit.

## Conclusion indépendante après dernière correction de fixtures

**Aucun P0/P1 restant confirmé dans le périmètre de source STOP-only relu.** Les trois défauts de contrat identifiés sont corrigés : unicité d’admission du rapport, barrières réelles avant façade Full et conservation du hash des rejets observés pendant la lease continue. Leurs canaris vérifient les propriétés concernées ; aucun garde de sûreté n’a été desserré pour obtenir ce résultat de revue.

Dernières corrections de tests relues : les bases temporaire/fresh-root/registry et les chemins source/sauvegarde du canari post-publication sont maintenant canoniques sur macOS. Elles changent la provenance des chemins de fixture, pas l’authentification ni les assertions de publication/lease. Les tests d’auth conservent le vrai gateway/Store inactif et leurs négatifs. Les SHA de production communiqués au gel restent inchangés.

SHA de tests remplaçant les versions antérieures de ce rapport :

- `src/broker/service/stop_auth_tests.rs` : `bc0456d1ef15fa8e91cdd85f543c84759f250f50c2da92f2da4f59035cb38595`.
- `src/broker/service/cancel_retry_tests.rs` : `9726ee463e58f51456d766432bd0edc1988179a5851998b44023fd295e626b90`.
- `src/broker/journal/tests.rs` : `7b4a6e189a5d411e1fd8710f616164bd97472e82095f070f3e9a03bdba6c38f0`.

La revue de source est terminée pour ce gel. Les campagnes Luna et leurs logs constituent la validation exécutée séparée ; je n’ai lancé aucune commande Cargo et je ne transforme pas les résultats encore attendus en PASS. Les échecs de préparation précédents doivent rester dans les preuves. Cette conclusion ne prétend pas qualifier un arrêt Docker/Podman réel, une panne électrique, les autres UID/plateformes ou une release, et ne ferme aucun gate par elle-même.

## Revue incrémentale des trois tests — gel suivant, production inchangée

SHA effectivement relus : `stop_auth_tests.rs` `bbd32fedc03a122be2b1a3edbd00ed0ad6c1913de5abae9d59162fd6042f6e98` ; `tests/broker_writer.rs` `828bef17d84a3b8019ea233327d3f8fd05e99cf3afe0be1567331fefbd71311f` ; `tests/broker_outbox.rs` `74eed5adb8932f3af94f2d6c75b0adce6b06abbee893ccc5db7aa4c6c73bd53e`.

**Aucun nouveau P1 confirmé dans ce delta de tests.** Les correctifs de source STOP-only précédemment revus ne sont pas modifiés.

La matrice d’auth utilise Box::pin sur son appel de dispatcher afin de borner sa propre future de test. Son compteur distingue désormais exactement la route identity `/api/v1/control/protocol` et exige +1 par requête de forme valide ; les credentials vides exigent zéro HTTP total. Ce boxing du harnais ne démontre pas une correction ni une absence générale de risque de stack dans le produit. Le SIGABRT de la matrice précédente reste une preuve historique à conserver.

Le troisième canari `empty_credential_is_refused_on_the_actual_connection_without_namespace_creation` traverse réellement Service::connection dans une tâche Tokio, un UnixStream::pair et les fonctions de framing, sans boxing explicite du dispatcher ni augmentation de stack. Les factories de future utilisées pour size_of ne sont jamais invoquées : elles ne construisent pas d’abord une future volumineuse pour la mesurer. Les oracles sont version1, Unauthorized, zéro HTTP et root vide, puis fin bornée de la tâche. C’est une preuve ciblée du refus de credential vide par cette entrée, pas de tous les chemins authentifiés, charges/plateformes/stacks, ni du listener de découverte/contrôle UID. La mesure de taille de future est un diagnostic, pas une limite validée ou une qualification production.

Les corrections writer/outbox retirent l’appel à un helper de canonicalisation privé en utilisant serde_json::to_vec. Le rejet de fixture modifie un champ existant, garde son ordre de clés et exige toujours Conflict dans Full et Stops ; un JSON invalide ne suffit pas à cet oracle. Pour V4, le test compare la même sérialisation des événements avant/après, garde les collisions et le reçu de replay ; aucune API de production n’a été rendue publique pour faire compiler les tests.

La conclusion de revue de source reste : pas de P0/P1 restant confirmé dans le périmètre relu. Les exécutions Luna du gel suivant, dont le nouveau canari connection, les intégrations et les checks/release, restent une validation séparée à rattacher aux logs. Aucun Cargo exécuté ni PASS anticipé par le reviewer.

## Clôture de la revue et résultats Luna recoupés en lecture seule

Le dossier `/private/tmp/ai-guard-stop-gel4.m2XvQt` a été consulté sans exécuter Cargo : sept fichiers RC à 0 ; logs GEL4 avec 3 auth, 4 intégrations, 3 writer et 1 V4 release réussis, zéro FAIL/ignored dans ces cibles. fmt, Clippy strict et build release ont RC0. Les vérifications de sources avant/après comportent chacune 153 lignes OK. Les tailles mesurées dans ce build debug sont dispatch107488 octets et connection107600 octets : ce sont des diagnostics de ces futures, pas une qualification générale de stack.

La campagne GEL3 transmise par l’orchestrateur conserve service17 et journal7 parents ; ses deux helpers ignorés ne comptent pas comme fonctions produit supplémentaires. Après déduplication des fonctions reprises en GEL4 et ajout du troisième canari connection, l’union annoncée est **33 fonctions uniques**. Les nouvelles vérifications GEL4 et leurs RC/logs ont été recoupés directement ; l’exécution reste celle de Luna, distincte de cette revue de source. Les premiers FAIL de fixtures/compilation et le SIGABRT de matrice non boxée doivent rester dans le dossier de preuve.

**Conclusion finale : aucun P0/P1 restant identifié dans le périmètre STOP-only gelé et relu ; les trois défauts trouvés ont leurs corrections et canaris ciblés.** Le refus de credential vide via connection est vérifié ; il ne couvre pas PrivateListener, tous les chemins authentifiés, la charge ou toutes les stacks. Aucune qualification Docker/Podman, panne électrique, autre UID/plateforme ou production n’est revendiquée. Aucun test supplémentaire, source applicative, service ou compte n’a été créé/modifié par le reviewer. Rapport final conservé en mode600, prêt à copier dans les preuves du lot.
