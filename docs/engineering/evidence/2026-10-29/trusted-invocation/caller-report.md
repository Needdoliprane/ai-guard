# Callers P0 — programme moteur de confiance

Périmètre : `src/runner.rs`, `src/runner/managed.rs`, `src/operations.rs` et tests callers autorisés. Aucun commit, suppression de preuve, volume ou configuration existante. Le dépôt était intégralement non suivi au début ; cet état est préservé. Les skills de projet `ai-guard-verify` sont absents de `.agents/` et `.claude/` : chargement automatique non confirmé, workflow fondé sur les contrats lus.

## Changements

- `--runtime-program` absolu sur doctor/build/up/down/run et variantes opérateur resume/stop/export. Les API publiques existantes sont conservées ; leurs variantes `_with_program` portent explicitement la sélection et l'adoption. Aucun programme reçu par API worker.
- Un `Runtime` approuvé est résolu avant les préflights Docker context / Podman image, doctor et effets. Docker n'hérite plus de DOCKER_HOST/CONTEXT. Le contexte du compte OS est lu par ce même programme pour Run et les opérateurs ; Podman Run conserve local/Unix.
- Les programmes, entrées d'installation et maillons de liens dans le projet importé sont refusés avant premier spawn. Cible canonique et programme explicite divergent sont également refusés à la reprise.
- Les seeds neuves incluent un fingerprint d'invocation distinct dans leur nom ; les fichiers historiques sont conservés.
- Une ancienne binding sans invocation est consultable sans moteur. L'adoption explicite couvre aussi une ancienne policy1 ou un client remplacé : programme neuf approuvé, même autorité vérifiée en lecture seule, événement `runtime.invocation.bound` stable portant anciens/nouveaux fingerprints, accusé durable, puis seed/checkpoint avant effet. L'ancienne binding complète reste en fichier privé `invocation-history/<authority>-<oldfp>.json`. CID, worker, attempt et claim restent inchangés. Adopter une invocation déjà valide et identique ne crée pas de nouvel événement.
- Doctor/Build/Up/Down Docker conservent un `ManagedRuntime` ciblé Unix et son engine ID. Chaque Build/Compose/version revalide cette autorité en lecture seule puis utilise `--host` figé. `--runtime-endpoint` explicite permet d'éviter le contexte ; le flag est réservé Docker pour ces quatre opérateurs. Down n'exige ni image worker ni doctor rootless.
- Les fixtures Rust runner utilisent désormais le programme absolu explicite et une copie privée par scénario, avec métadonnées sidecar dérivées de current_exe. Aucun environnement de test n'est ajouté à l'allowlist de production.
- La fixture `runtime_program` vérifie maintenant le cwd égal au parent canonique du programme, selon le pinning FD policy2 gelé par core. Les chemins de projet/build/Compose/config restent absolus ; aucun cwd projet n'est introduit.
- Les tests moteurs ignorés acceptent `AGENTSANDBOX_TEST_RUNTIME_PROGRAM` côté harness seulement ; ils compilent avec le Runtime fallible et Clone.

## Preuves exécutées

`FAIL` attendu RED avant correction : `CARGO_INCREMENTAL=0 cargo test --locked --test runtime_program doctor_ignores_project -- --nocapture`. Le Rust docker du projet écrit son marqueur avec PATH vide. `/tmp/ai-guard-runtime-program-red.log`.

`FAIL` attendu régression complémentaire : entrée d'installation via alias macOS /var→/private puis chaîne hors→projet→hors. Les diagnostics n'étaient pas encore le refus attendu, sans exécution métier. `/tmp/ai-guard-caller-project-exclusion.log`, `/tmp/ai-guard-caller-chain-red.log`.

`PASS` première série ciblée : runner_recovery 7, runner_terminal_audit 1 (11 scénarios), runtime_authorization 6, runtime_operations 9, runtime_program 2. `/tmp/ai-guard-caller-tests.log`.

`PASS` seconde série : runner_recovery 11, dont remplacement binaire avant tout nouvel appel moteur, programme divergent, migration audit HTTP503 sans start puis reprise même CID, et compatibilité terminal status/stop/resume/export. `/tmp/ai-guard-caller-new-tests.log`. Le test projet de cette série avait révélé l'alias ci-dessus ; il est corrigé et ne doit pas être annoncé PASS d'après ce fichier.

`PASS` compilation des trois tests moteurs : `CARGO_INCREMENTAL=0 cargo check --locked --test runtime_confinement --test runtime_terminal_persistence --test runtime_smoke`. `/tmp/ai-guard-caller-realtest-check.log`.

`PASS` série finale après correction complète des liens et ajout arrêt actif legacy : `CARGO_INCREMENTAL=0 cargo test --locked --test runner_recovery --test runtime_program --test runner_terminal_audit --test runtime_authorization --test runtime_operations` : **31 tests PASS, zéro échec, zéro ignoré** (11+4+1+6+9), dont les 11 scénarios internes au test terminal. `/tmp/ai-guard-caller-final-tests.log`.

`PASS` formatage des fichiers possédés : `rustfmt --edition 2024 --check` sur les trois sources et neuf fichiers de tests/fixture modifiés. Le check global reste à root après gel transversal.

Revue indépendante readonly `caller_review` terminée : une régression matérielle trouvée puis reproduite RED : recanonicaliser le projet original à chaque reprise empêchait stop/resume après renommage de l'original (`/tmp/ai-guard-caller-original-rename-red.log`). Correction relue : comparaison au chemin absolu normalisé mémorisé avant import ; la reprise ne dépend plus de l'existence de la source. Le test de crash renomme maintenant l'original avant la reprise moderne puis vérifie l'adoption/arrêt legacy et le CID inchangé. `PASS` revalidation après correction : runner_recovery **11/11**, runtime_program **4/4**, zéro ignoré ; `/tmp/ai-guard-caller-review-fix-green.log`. `PASS` rustfmt owned répété après correction. Aucun autre défaut matériel nouveau identifié par la revue source.

`FAIL` attendu RED opérateurs : contexte changé après premier info envoyé vers le second endpoint, et engine ID changé accepté avant correction. `/tmp/ai-guard-operator-binding-red.log`.

`PASS` lot opérateurs avant pinning FD policy2 : recovery **11**, opérations **9**, programme **8**, zéro échec/ignoré ; `/tmp/ai-guard-operator-binding-final-tests.log`. Les cas couvrent doctor/build/up/down, changement de contexte après préflight, ID après doctor puis entre deux builds, endpoint explicite et refus SSH/TCP avant spawn. Une assertion initialement liée à la position du verbe build a été corrigée pour compter sémantiquement les commandes malgré le préfixe `--host` ; `/tmp/ai-guard-operator-binding-green.log` conserve cet échec intermédiaire, il n'est pas une preuve finale PASS.

`PASS` série après policy2 + extension adoption Some : **36 tests**, zéro échec/ignoré (recovery12, programme8, terminal1, autorisation6, opérations9), `/tmp/ai-guard-caller-policy2-final-tests.log`. Les modes policy1 Some et client remplacé sont exécutés dans le nouveau test paramétré, avec audit503, ID d'événement stable, CID conservé, historique complet et seconde adoption via export sans événement supplémentaire.

Un dernier garde ferme le repli vers l'ancien programme encore valide lorsque l'adoption d'un programme alternatif attend son audit. `PASS` cas dédié après ce garde : `pending_program_change_cannot_fall_back_to_the_old_valid_program_after_audit_failure`, `/tmp/ai-guard-caller-pending-adoption-green.log` ; deux retries ancien programme (+/- adoption) n'appellent aucun moteur, le même candidat reprend ensuite le même CID avec un seul start. La cible runner_recovery contient donc désormais **13 tests**. Le check global de root doit reprendre l'ensemble à ce gel.

Second regard du lot complémentaire effectué par `trusted_runtime_core/invocation_review` en lecture seule : aucun défaut matériel identifié dans OperatorRuntime, l'adoption Some, la préservation de l'historique ou les gardes de pending audit. La politique1 garde son fingerprint historique lorsque le champ directory est absent. Les vérifications moteur et effets sont des connexions séparées, explicitement hors promesse d'atomicité.

## Limites et points transmis à root

- `NOT_RUN` par cet agent : moteurs réels Docker/Podman, recette A1/A2/B1, Linux/macOS matrice complète, full suite avant gel transversal, clippy/release globales. Les tests ignorés ne sont pas comptés PASS.
- `BLOCKED` confirmation d'audit formelle : aucun sandbox OS complet correspondant au profil du skill security-audit n'a été établi. Les marqueurs bornés et tests logiciels ne qualifient ni résistance à un programme administrateur hostile, ni ACL, ni substitution atomique check→spawn.
- Les vérifications engine ID et effets restent des connexions distinctes. Le préfixe Unix figé et l'ID attendu n'offrent pas d'atomicité contre un administrateur qui remplace l'endpoint entre check et effet. Podman opérateur conserve son comportement antérieur ; la liaison endpoint/marker Docker ajoutée ne lui est pas attribuée.
- Une ancienne seed Podman sans binding et sans invocation ne dispose d'aucune autorité historique vérifiable : refus avant bootstrap, confirmé par arbitrage root. Une ancienne seed ayant une binding utilise le même helper d'adoption explicite ; aucun nouveau bootstrap n'est présenté comme continuité historique.
- L'export v1 exige une approbation explicite et un programme sûr, mais son historique n'offre pas de liaison moteur/CLI durable complète. Le chemin Podman v1 exige une binding privée préexistante ; la migration complète de cet historique n'est pas revendiquée.
- Le second regard source indépendant est traité. Les validations globales et limites ci-dessus demeurent à root. Ce lot ne ferme aucun gate production ni risque de fencing, logs ou quota disque.
