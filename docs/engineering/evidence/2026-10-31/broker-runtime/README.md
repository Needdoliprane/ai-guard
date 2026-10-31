# Broker permanent — essais Docker et diagnostics, 31 octobre 2026

**G09 FAIL ; aucun gate global fermé.** H4/H5 échouent après claim, H6 atteint la borne d'observation du harnais pendant la préparation, H7 enregistre une erreur préclaim. Aucun Running ni faute au point prévu. Le diagnostic D3 passe ses vérifications logicielles mais sa frontière typée n'a pas été atteinte ; son complément préclaim attend encore l'essai réel. Aucun fournisseur IA, SSO, frontend, cloud ou PKI utilisé par ce lot. Les revues indépendantes ne qualifient pas le produit.

## Périmètre et gels

Base `e7cc0bb25dd1b8ab81e63bb5ed44cf8c55661efa`, branche `codex/broker-service`, worktree isolé. Le checkout principal et ses modifications du mainteneur restent préservés. Les manifestes figent Cargo, toolchain, sources et tests, hors secrets et données QA. H4/H5 ont155 entrées ; D1/D2/D3 en ont158. Les archives privées et les binaires ne sont pas ajoutés à Git. Les chemins temporaires désignent les preuves de cet hôte, pas des artefacts garantis après sa suppression.

Hôte Darwin ARM64, Rust1.99.1 ; Docker CLI29.2.1 et daemon Desktop29.9.2 Linux/aarch64, endpoint Unix explicite. Daemon sans rootless annoncé, opt-in rootful du harnais explicite ; worker non privilégié. Image locale `sha256:7c64760cc4a5f29a40685cae2451afed914ce81671601166792cd087beab9db6`, sans pull ni qualification RepoDigest. Les services de contrôle de ce harnais sont des binaires natifs ; le déploiement conteneurisé du contrôle n'est pas qualifié par ces essais.

## Résultats observés

| Gel / commande | Statut | Portée constatée |
|---|---|---|
| H4 : test réel `broker_sigkill_preserves_original_deadline_without_second_start` | FAIL, RC101,47,39s | Aucun Running+Ready/prepared observé ; aucune faute SIGKILL injectée |
| H5 probe natif `inspect_retained_guard_after_runtime_failure` | PASS, RC0,0,02s | Lecture du seul gardien H4 existant : stopping, prepared_once, CID created, stop_failures0 ; aucune mutation/start/cancel |
| H5 retry : même test réel, observation HTTP légère | FAIL, RC101,46,59s | Même échec avant Running ; le retrait du polling writer ne suffit pas |
| D1 : `--lib diagnostics::tests` | PASS,5 | Métadonnées bornées, propagation typée et absence du texte privé dans le payload |
| D1 : `--lib runtime::prestart::tests` et `--lib broker::enforcer::client::stage_diagnostic_tests` | PASS,1+1 | Contextes runtime et prédicat final natif |
| D1 : fixture `runtime_prearmed` ciblée | PASS,1,15,79s | CLI runtime Rust de test, pas Docker ; contexte préserve l'erreur typée et l'arrêt pendant callback |
| D1 : fmt / Clippy strict | PASS / FAIL,RC101 | Dead code, taille d'erreur et placement du module de tests ; logs conservés |
| D2 : mêmes7 unités et fmt | PASS | Box du sample d'erreur, getters test-only, module déplacé ; application identique à D3 |
| D2 : Clippy strict | FAIL,RC101 | Un `collapsible_if` dans le harnais, corrigé par let-chain équivalente |
| D3 : fmt / Clippy strict all-targets / release | PASS,3RC0 | Aucun nouveau test rejoué après la seule correction syntaxique du harnais |
| Nouveau `broker.batch.failure` sur vrai moteur | NOT_RUN | Le raccord outbox est relu en source, pas encore exercé réellement |
| SIGKILL/deadline/reprise, disarm D5, recette A1/A2/B1, Podman/Linux, capacité100, TTY | NOT_RUN dans ce lot | Aucun résultat logiciel ne les remplace |

Le total diagnostic est **huit fonctions logicielles uniques**, sans additionner les sept replays D2, le probe natif ou les tests historiques STOP. Lire [la revue finale](d3/reviewer-prestart-diagnostic.md), les logs D1/D2/D3 et leurs deltas. Les échecs Clippy intermédiaires sont conservés.

## Faits des deux échecs réels

Chaque demande utilise un gateway schéma3 et un stockage neufs, une source jetable, une politique autorisant uniquement `/bin/sleep 20`, un timeout5s, réseau none et quota1. Les15 opérations de supervision sont ACKées, dont import/vérification/helpers ; Arm worker ACKrev33, claim emit_once ACKrev35, puis worker/unavailable rev36 et37. Le worker central reste StartIssued, le conteneur Created, quota retenu, sans durée runtime ou usage terminal inventé. Aucun pending HTTP n'explique à lui seul l'échec.

| Élément | H4 | H5 retry |
|---|---|---|
| Racine privée conservée | `/private/tmp/br-dFE99I` | `/private/tmp/br-NBcV3F` |
| Worker | `b7ab7ad0-a9cb-4271-a4a4-5927ce14013a` | `c252e676-d039-4c02-bcf4-18f6fdd92c36` |
| CID | `a87643c3d8bcffff916f79807ecf7d09469c1b399ec8687951d6ff3b22d82ecb` | `e22601834084d4cee79a4ffb0bc700656a4b420e2c7f89bc25c2eab64bd60db9` |
| Arm | `771ffb47-1928-4350-9412-28d72549f2cd` | `1d078937-ad5a-4c3f-9231-a2367c916740` |
| Nanosecondes natives Arm / deadline | `339650512620458 / 339655512620458` | `340604839052166 / 340609839052166` |
| Sample du contrôle du claim | `339653528379708` | `340607474064125` |
| Temps restant à ce sample, avant dernières écritures | `1,984240750s` | `2,364988041s` |

Les samples proviennent de l'horloge continue Darwin ; `observed_utc=null`. Les mtimes du journal ne donnent pas une correspondance UTC exacte. Une expiration de durée ou de fraîcheur750ms est une hypothèse, **pas la cause établie**. Le prepared_once ultérieur du probe peut être établi par la restauration observe-only : il ne prouve pas le succès initial de Prepare. Aucun arrêt effectif du worker n'est revendiqué sur un CID jamais observé démarré.

Chaque essai ajoute un conteneur Created et son volume ; zéro conteneur/volume antérieur manquant dans les inventaires avant/après. Les gardiens non vides et leurs ressources restent conservés faute de disarm D5 qualifié. Aucun prune, kill global, effacement de claim, suppression de volume ou modification de la configuration existante. Les credentials frais et l'auth privée du gardien ne sont pas publiés.

## Diagnostic implémenté et limites

Les erreurs natives Prepare/garde conservent code fermé, Arm, deadline d'origine et sample existant éventuel. Les contextes runtime indiquent la frontière en échec et, seulement si valide, le CID complet. Le classifier sérialise uniquement ces types connus, sans formater la chaîne d'erreurs arbitraire. L'événement passe par masquage puis enqueue V4 durable avant HTTP/ACK, avec les limites existantes en cas d'échec du journal. Un message générique signale une livraison incomplète sans annoncer à tort sa durabilité ou son absence.

`runtime_effect=unknown` ne donne aucune autorité, preuve d'absence, terminal, fencing ou libération de quota. Wire IPC, LocalFailureV1, Status, formats persistés et dépendances inchangés. Le dernier garde reste immédiatement avant spawn ; aucune IO/attente/lecture d'horloge ajoutée au chemin de succès. Aucun réarmement, nouveau claim, rallonge de durée ou affaiblissement des synchronisations.

## H6 — limite d'observation atteinte pendant la préparation

Une tentative sur le produit D3, HEAD`99238e3`, utilise les158 mêmes fichiers source. **FAIL RC101,48,25s**, avant Arm worker. Le worker reste Preparing rev2 ;9/9 opérations sont ACKées, dernière HelperClaim ImportVerification rev22, aucun pending, aucune FailureObserved ni événement diagnostic dans le journal/SQLite. Ce résultat ne démontre pas un refus Prepare du worker. [Rapport Core en lecture seule](h6/core-diagnostic.md), [sortie](h6/runtime.log) et [scalaires Docker projetés](h6/container-scalars.txt).

Racine privée `/private/tmp/br-6dPLTS`, CID helper `f3ed32a35cd3ed767beacda7c01029f1c8033353990915c3fb8052cf0f88218a`. Il démarre à`2026-10-31T18:58:07.705873553Z` et termine à`18:58:07.87673872Z`, exit0/nonOOM/restart0. La publication resultFAIL a un mtime`18:58:13.879424470Z` ; ce n'est pas un sample de l'instant exact de l'erreur. Le test retourne sur sa borne préRunning45s et Drop tue/reap ses seuls enfants broker/gateway. L'Arm du helper vaut120s, avec115,877s encore disponibles au sample du claim ; aucune Arm worker5s n'existe dans H6. Les sources ne permettent pas d'appeler cette observation un refus produit ou une expiration du helper.

Correction du seul harnais : observation préRunning120s, global240s et libellé cohérent. Les5s d'Arm worker, la faute après Running/Ready mais avant deadline d'origine, les contrôles nonOOM/137/durée<20s et les identités Arm/claim/start/CID restent inchangés. Une nouvelle expiration120s resterait un FAIL d'observation, sans cause produit établie ; cette fenêtre globale ne garantit pas l'expiration de chaque helper armé tardivement. [Revue de ce delta](h6/harness-review.md). Aucune source applicative modifiée, aucun réarmement ni retrait de synchronisation. Racine, gardien et ressources conservés ; zéro ressource baseline manquante. Les causes H4/H5 restent inconnues.

## H7 — erreur préclaim, pas de démarrage qualifié

Sur ce harnais corrigé, produit D3 inchangé : **FAIL RC101,122,78s**. Racine `/private/tmp/br-yEdg2N`. Worker `fecfaf08-5402-4f7d-bb26-afd4b008b0b5` Created rev4, CID `229b3c948ed9048001362df0acc192414af3a5cb603c038dd553fddfd314316a` Created, aucune date de démarrage/fin effective.14/14 opérations ACKées, WorkerArm `ac4dca97-3fcf-41b6-9d53-fd3ce82bbfc5` ACKrev33/seq16, puis première FailureObserved34 worker/unavailable. Aucun WorkerClaim, aucun pending, aucun `broker.batch.failure`. [Faits du journal/SQLite en lecture seule](h7/core-diagnostic.md) et [sortie du test](h7/runtime.log).

Arm native`343733635626041`, deadline`343738635626041` : cinq secondes. Mtimes pending32=`1793473574.161340113`, ACK33=`1793473579.058683342`, Failure34=`1793473585.507432199`. L'intervalle pending→ACK de4,897343229s indique un chemin coûteux ; sans sample natif de l'erreur et sans offset UTC exact, il ne prouve pas une expiration au point de refus. Register, relecture et Ready précèdent ici la première publication du claim et n'avaient pas encore de diagnostic codé D3.

Le delta runtime est fondé sur les IDs stables : un CID et un volume neufs, zéro ID antérieur manquant, H4/H5/H6 conservés. Une première comparaison de lignes complètes a confondu les libellés d'âge Docker avec des changements d'identité ; la capture initiale et la correction sont conservées en privé. Cela a entraîné la lecture supplémentaire des seuls scalaires de deux anciens CID, sans mutation ni contenu secret ; aucune inspection historique supplémentaire. Fmt et Clippy strict H7 PASS, sans rebuild du produit D3 inchangé. Ce résultat n'est pas une qualification du délai ou de la reprise.

## Complément préclaim et adaptation lifecycle

Les étapes fermées `broker_step` couvrent échange Arm, registration, relecture, Ready, clock, échange claim, génération et Prepare. Un contexte d'échange n'est pas une preuve d'envoi ou d'ACK ; le diagnostic natif imbriqué peut préciser le contrôle en échec. Register conserve son implémentation ; une factory valide ses seules métadonnées sur erreur. Le Ready public partage le corps diagnostiqué et conserve ordre, codes, health/freshness/génération/stop/clock/deadline. Aucun nouveau sample, texte arbitraire ou effet observé inventé ; l'événement garde `runtime_effect=unknown` et ne libère rien.

L'adaptation lifecycle V3→V4 est déplacée sous le verrou du Journal, à partir du preload strict déjà nécessaire. Le writer n'effectue plus son Fullload d'adaptation séparé : trois chargements complets deviennent deux par mutation lifecycle, sans supprimer preload, postload, fsync, recontrôle du binding, CAS, floors ou validation des transitions. Les bytes d'événements/ACK V4 sont conservés ; expectedV4 obsolète reste Conflict, downgrade public refusé. Ce comptage est une propriété du code, pas une mesure de performance.

La revue a trouvé une régression dans cette optimisation en cours : appeler directement load_locked au lieu de la classification status_locked oubliait l'observation d'une réservation héritée partielle. Un nouveau test de filesystem réel réserve, ferme/réouvre sans prélecture, tente l'avancement, puis déplace le seul job de fixture sans effacer ses bytes ; l'observation doit empêcher sa disparition silencieuse et sa réadmission. Le test **RED RC101** échoue à l'assertion status→Conflict, compilation correcte. Correction limitée au préload du nouveau port : même match status_locked que Journal.load, sous le même mutex ; Accepted garde un seul Fullload, les autres états gardent leurs erreurs historiques. Le test **GREEN RC0** passe sans modification de ses assertions. Cette régression n'a pas été publiée. Les revues et logs finaux sont conservés avec le lot ; aucune correction réelle de startup n'est revendiquée.

| Gel logiciel | Résultat et portée |
|---|---|
| PR1 |9 diagnostics,2 prédicats natifs,1 canari postpublication et1 outbox V4 :13 fonctions PASS |
| PR2 |1 contrat lifecycle et1 promotion V1→V2 : total15 fonctions uniques PASS ;3 tests Root rejoués pour le seul renommage Exchange, non additionnés ; fmt/Clippy PASS |
| RED |1 nouvelle régression partielle FAIL RC101 sur le port optimisé avant correction ; [log](preclaim/red/journal-partial-red.log) et manifeste conservés |
| PR3 |Cette régression GREEN RC0 et V4 rejoué PASS ; total16 fonctions uniques. Fmt FAIL sur le seul format du nouveau test, sans échec métier |
| PR4 |Seul test `journal/tests.rs` reformaté ; logique/assertions et toutes sources métier PR3 inchangées. Fmt/Clippy strict et release PASS RC0, build6min20s ; aucun test rejoué après cette transformation |

Les sorties sont dans `preclaim/pr1`, `preclaim/pr2`, `preclaim/red`, `preclaim/pr3` et `preclaim/pr4`. Les manifests et deltas distinguent chaque gel : PR1→PR2 renomme deux frontières ; RED→PR3 corrige seulement le préload du port ; PR3→PR4 ne change que le format du test. Les huit fonctions D3 historiques ne sont pas ajoutées à ces16. Les canaris utilisent filesystem/Journal/Writer réels, pas un moteur de conteneurs ; les modèles et tests E2E n'ont pas été relancés.

Cibles de reproduction du complément, exécutées par Luna avec `CARGO_TARGET_DIR=../broker-supervision/target` (les replays et commandes de format/build sont séparés dans les logs) :

```sh
cargo test --locked --lib diagnostics::tests -- --nocapture
cargo test --locked --lib broker::enforcer::client::stage_diagnostic_tests -- --nocapture
cargo test --locked --lib broker::journal::tests::full_writer_rechecks_binding_after_real_publication_before_ack -- --exact --nocapture
cargo test --locked --test broker_outbox v4_survives_command_and_lifecycle_updates_and_direct_v3_downgrade_is_refused -- --exact --nocapture
cargo test --locked --test broker_lifecycle lifecycle_wire_context_replay_caps_and_legacy_approval_boundaries -- --exact --nocapture
cargo test --locked --test broker_checkpoint real_journal_promotes_v1_rev0_to_pending_decision_v2_without_rewriting_rev0 -- --exact --nocapture
cargo test --locked --lib broker::journal::tests::lifecycle_adapter_retains_observation_of_an_inherited_partial_job -- --exact --nocapture
```

La [revue indépendante finale](preclaim/independent-review.md) conserve le constat, sa reproduction et sa correction ; aucun P0/P1 restant identifié dans le périmètre. Le manifeste final vérifie158 fichiers avant/après, puis root vérifie indépendamment checkout et archive extraite. Copies de binaires privées hors cache : agentsandbox `ec99e7817efc53ccb912d99bd5d5130c24bc89c9e7a59ef5d24cfb782338e9f6`, provider `e50f668b1aafd5928df9b0ff364348b18e5514883308db2d2dabc045743046c9`. Source archive `41f76fecc4b5cfbb9f41420a947af34bef12c6abcb6a19412882d59b7cdeaf8f`, manifeste `0089c36c457625ff01d342954dccd4e3dfcc94452246084c5de396c9153ded9d` ; archives/binaires non publiés dans Git, aucune promesse de rebuild byte-identique. Aucun H8 exécuté dans ce lot.

## Commandes exécutées

Toutes les commandes Cargo sont exécutées par Luna depuis le worktree, avec `CARGO_TARGET_DIR=../broker-supervision/target`. Les filtres unitaires restent ciblés :

```sh
cargo test --locked --lib diagnostics::tests -- --nocapture
cargo test --locked --lib runtime::prestart::tests -- --nocapture
cargo test --locked --lib broker::enforcer::client::stage_diagnostic_tests -- --nocapture
cargo test --locked --test runtime_prearmed prearmed_stop_interrupts_pending_started_callback_without_global_ctrl_c -- --exact --nocapture
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo build --release --locked
```

Les deux essais réels utilisent la sélection suivante, **propre à cet hôte**, et conservent des ressources ; ce n'est pas une commande de quick start ordinaire :

```sh
AGENTSANDBOX_TEST_RUNTIME=docker \
AGENTSANDBOX_TEST_RUNTIME_PROGRAM=/opt/homebrew/bin/docker \
AGENTSANDBOX_TEST_RUNTIME_ENDPOINT=unix:///Users/louis/.docker/run/docker.sock \
AGENTSANDBOX_TEST_IMAGE=sha256:7c64760cc4a5f29a40685cae2451afed914ce81671601166792cd087beab9db6 \
AGENTSANDBOX_TEST_ALLOW_ROOTFUL=1 \
AGENTSANDBOX_TEST_RETAIN_ENFORCER=1 \
cargo test --release --locked --test broker_runtime broker_sigkill_preserves_original_deadline_without_second_start -- --ignored --exact --nocapture
```

Le probe n'appelle ni sélection Docker ni serveur HTTP et refuse de créer un gardien de remplacement. Il lit la capability privée existante sans l'afficher :

```sh
AGENTSANDBOX_TEST_RETAINED_ROOT=/private/tmp/br-dFE99I \
AGENTSANDBOX_TEST_RETAINED_ARM=771ffb47-1928-4350-9412-28d72549f2cd \
cargo test --release --locked --test broker_runtime inspect_retained_guard_after_runtime_failure -- --ignored --exact --nocapture
```

## Artefacts et suite

Les captures privées originales sont `/private/tmp/ai-guard-runtime-h4.1Y3B4Z`, `/private/tmp/ai-guard-runtime-h5.4pwJ7e`, `/private/tmp/ai-guard-runtime-h5-retry.fxob46`, `/private/tmp/ai-guard-d1-software.3ipHYm`, `/private/tmp/ai-guard-d2-software.DkH0QN` et `/private/tmp/ai-guard-d3-final.UPraQc`. Root vérifie indépendamment158 fichiers de l'archive finale extraite et158 fichiers du checkout contre le manifeste D3, plus les deux copies de binaires. SHA agentsandbox D3 : `709b70eee959fb23bb309164ae5e7f8e32a5dcc6dfd96fd91673aa8fef8221ae` ; provider : `2cb846f02db229a673c95b3c35d287db6f0d0d5f16029deb3d580df664483390`.

Prochain critère : une seule tentative réelle H8 instrumentée sur PR4 pour livrer/lire le diagnostic préclaim complété, localiser l'erreur et corriger avec régression avant répétition. Puis établir Running, panne SIGKILL, arrêt sur deadline d'origine, reprise idempotente et cycle terminal/disarm. Les quatre racines et gardiens non vides H4–H7 restent conservés, aucun effacement pour obtenir un PASS. SSO/Keycloak/PKCE reste obligatoire mais absent, qualification NOT_RUN ; [contrat et documentation](../../../SSO_OIDC.md). Le rappel Daybreak Blue attend ces parcours réels stabilisés.
