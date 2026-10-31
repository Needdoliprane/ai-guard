# Qualification ciblée du cycle de vie Broker — 31 octobre 2026

## État final

Sur GEL2 core + GEL runner `d559105e83f755bffbe9d749b7b2f03447a4437ebe3fb456e351d75052d402ce`, les cibles retenues passent :

- `cargo test --locked --test broker_lifecycle -- --nocapture` — 5/5 PASS, RC 0. Log : `/tmp/ai-guard-broker-lifecycle-final/lifecycle-gel2-corrected.log`.
- `cargo test --locked --test journal_lifecycle_floor -- --nocapture` — 2/2 PASS, RC 0. Log : `/tmp/ai-guard-broker-lifecycle-final/floor-gel2-final.log`.
- `cargo test --locked --test broker_admission_client -- --nocapture` — 4/4 PASS, RC 0. Log : `/tmp/ai-guard-broker-lifecycle-final/admission-client-gel2-final.log`.
- `cargo test --release --locked --test broker_checkpoint sqlite_store_receipts_cover_all_supervision_operation_variants -- --exact --nocapture` — 1/1 PASS, RC 0, 23,66 s de temps de test. Log : `/tmp/ai-guard-broker-lifecycle-final/export-cancel-release.log`.

Le graphe de release utilisait la source test `tests/broker_checkpoint.rs` SHA `40ca17d3027914a79d16437562d161493257223d13a7c1444988b50e8cd9111c` et les sources core GEL2. Il teste la fermeture du grant Export avant Cancel puis accepte le contrôle positif avec un nouveau ExportRequest et reçu Store réellement créés après Cancel.

Manifeste de sources GEL2/tests : `/tmp/ai-guard-broker-lifecycle-final/gel2-final-validation.sha256`. Le test lifecycle a ensuite reçu une correction de cardinalité de fixture (2 commandes Approval + Cancel), snapshot exact `/tmp/ai-guard-broker-lifecycle-final/lifecycle-corrected/broker_lifecycle.rs`, SHA `e44c7761320ecaad2da6869d1ceaa52ced560ec0669d2de6fb4b2070c7aabc30`; manifeste avec hashes app `/tmp/ai-guard-broker-lifecycle-final/lifecycle-corrected.sha256`.

## Régressions reproduites avant correction

- Perte du plancher accepted si `accepted.json` manque : ancien RED `RC101`, `/tmp/ai-guard-journal-lifecycle-floor-red/`.
- `list(limit=1)` ne revalide pas une réservation déjà observée hors page : RED `RC101`, `/tmp/ai-guard-broker-lifecycle-gel1-tests/list-observed-outside-page-diagnostic.log`.
- `RejectionPhaseV1`/`RejectionCodeV1` reflètent la valeur enum inconnue dans l’erreur Serde : RED `RC101`, `/tmp/ai-guard-broker-lifecycle-gel1-tests/rejection-enum-red.log`. `JournalEntryKindV1` n’a pas Deserialize public.
- Une HelperClaim Export après Cancel sous ancien reçu Export était acceptée : RED release `RC101`, `/tmp/ai-guard-broker-lifecycle-export-red/gel1-export-cancel-release.log`; source test RED archivée SHA `2607bbf4e950bea07d5aa2a6d7cb45c2502bcebb3204bb081c432bdb06406fd0`.
- Avant l’ajout de méthode, `AuditClient::admit_supervised` était absente : no-run RED `E0599`, `/tmp/ai-guard-broker-lifecycle-export-red/admission-client-api-red.log`.
- Le client HTTP acceptait un DTO 201 provenant d’un vrai Gateway/Store quand `attempt.start_operation_id` était muté en UUID : RED `RC101`, `/tmp/ai-guard-broker-lifecycle-final/initial-worker-response-integrity-red.log`. Le middleware ne remplace pas le Store ; il modifie uniquement sa réponse loopback.

## Reprises de fixture, non défauts produit

- Le premier broker_lifecycle run GEL2 a eu 2 PASS/3 FAIL : compteur pending d’une entrée rejected attendait `Some(0)` au lieu de `None`, fixture Review utilisait `id` (Allow au lieu de Review), et le chemin legacy `admit_worker` était utilisé sur Store schema3. Logs conservés `/tmp/ai-guard-broker-lifecycle-final/lifecycle.log` puis `/tmp/ai-guard-broker-lifecycle-final/lifecycle-gel2-revised.log`.
- Le premier client run utilisait une décision déjà Deny comme reçu et recevait HTTP400 ; fixture corrigée pour enregistrer un vrai reçu Allow sous policy initiale puis évaluer avec une policy Gateway modifiée. Log conservé `/tmp/ai-guard-broker-lifecycle-final/admission-client.log`.
- Après ces corrections, une assertion attendait 1 commande après avoir enregistré Approval et Cancel ; corrigée à 2, puis la cible entière a passé.

## Couverture / limites

Les 3 tests admission utilisent un vrai Gateway Axum loopback et Store SQLite v3 : création/replay, refus Policy 403 après changement réel de policy, refus quota 429 et replay, erreur d’auth qui reste Err. Le test de réponse altérée conserve la réservation créée par le vrai Store et vérifie les champs `start_operation_id` et `attempt_id=nil`.

Les inspections/runtime snapshots hérités des tests broker checkpoint restent explicitement synthétiques ; Store/journal, reçus, décisions, admission, export, Arm, claims, observations et cleanup sont exercés avec les vrais composants applicatifs. Aucun moteur, modèle, fournisseur, Docker ou worker n’a été lancé.

## Canaris finaux et validation standard

Après gel final, les canaris demandés passent : `cargo test --locked --test broker_journal -- --nocapture` 12/12 ; `cargo test --release --locked --test broker_checkpoint -- --nocapture` 6/6 ; `cargo test --locked --lib broker::journal::tests -- --nocapture` 5 parents PASS, 2 tests-enfants ignorés par harness et couverts par leurs parents qui passent.

Validation standard : `cargo fmt --all -- --check` PASS, `cargo clippy --locked --all-targets -- -D warnings` PASS, `cargo build --release --locked` PASS. Un premier Clippy a relevé uniquement trois helpers de fixture avec trop d’arguments ; des allowances ciblées et motivées dans le test ont résolu ce lint. Les logs finaux sont sous `/tmp/ai-guard-broker-lifecycle-final/final-after-lints/` et `/tmp/ai-guard-broker-lifecycle-final/final-frozen/`.

Bundle final exact des sources avant/après le dernier lint : `/tmp/ai-guard-broker-lifecycle-final/final-after-lints/sources.sha256` (fichiers 0600, répertoire privé) ; le manifeste a été vérifié sans divergence. Le hash final de `tests/broker_checkpoint.rs` est `51690b191d32c782beceff67f1bd93db88ccc9f33c36913918da6209ed1b44af`.

Aucune full suite, E2E, moteur, modèle, fournisseur ou qualification production n’a été exécuté dans ce lot.

## Extension de couverture lifecycle — gel GEL2, 31 octobre 2026

Cette section complète l’état initial ci-dessus et fait autorité pour les deux cibles étendues. Elle n’ajoute pas les rejeux de diagnostic au décompte des tests.

- `cargo test --release --locked --test broker_lifecycle -- --nocapture` — 6/6 PASS, RC 0, 1,26 s de tests. Log brut privé : `/tmp/ai-guard-broker-lifecycle-final/coverage-expansion/final3/broker_lifecycle-release.log` (SHA-256 `196bf9e96b7ffcdd7a05c7bb829573c8eff21fab6a69a8dd865d48810b25c1d5`). Source test : `tests/broker_lifecycle.rs` SHA `d8d7c6354f7e181d13180578edbd38513468a2b7a642ae384da7ba569b75fc10`.
- `cargo test --release --locked --test broker_checkpoint -- --nocapture` — 6/6 PASS, RC 0, 32,23 s de tests. Log brut : `/tmp/ai-guard-broker-lifecycle-final/coverage-expansion/broker-checkpoint-final.log` (SHA-256 `2956428e9a7ae7ab542cd8cefd7f08d71d79b3c7e48232f55e421a1ad0beb7eb`). Source test : SHA `9cb7aed956c27e51569b582c928576894e800c27aa5139b5f8caaabe0cb2573c`.
- `cargo fmt --all -- --check` — PASS, RC 0. Log `/tmp/ai-guard-broker-lifecycle-final/coverage-expansion/fmt-final.log`, SHA `bde294368bfed77c2cddf8cec271d398aee9cdbab3b26e1059281bd33adb0120`.
- `cargo clippy --locked --all-targets -- -D warnings` — PASS, RC 0. Log `/tmp/ai-guard-broker-lifecycle-final/coverage-expansion/clippy-final.log`, SHA `9fc7aea6a366f005521f3e0997ddf0bbd19fdef16ec670cdb1f6cde454aed816`.
- `cargo build --release --locked` — PASS, RC 0. Log `/tmp/ai-guard-broker-lifecycle-final/coverage-expansion/build-final.log`, SHA `c07fb8223fee64fd1e14615ba73e44067ecc197b0fb4ba0f13fe2e96cbb22a53`.

Les assertions ajoutées éprouvent le refus des versions inconnues, des champs inconnus nullable (parse canonique et Deserialize public), des données tronquées ou suivies de JSON, le refus d’un input hash valide en forme mais falsifié ou recopié entre jobs, les 64 commandes acceptées et 65 refusées, le replay V3 historique exact et le refus des replays V1/V2 après promotion. Les contrôles d’approbation grand-père couvrent V1→V2→V3. La cancellation passe d’un `CancelReserved` réel pending à la mutation et à l’ACK Store réels. Les rejets sont relus après reopen avant les mutations de marqueur ; disparition/substitution sont vérifiées contre le plancher observé, les marqueurs terminaux doubles sont refusés et un marqueur synthétique de partial accept reste intact après refus. Le scénario Export vérifie une deuxième Cancel, refuse une demande co-née avec cette frontière, bloque l’ancien grant post-premier-Cancel et autorise le grant réel créé après le dernier Cancel.

Les huit hashes app gelés restent : runner `d559105e83f755bffbe9d749b7b2f03447a4437ebe3fb456e351d75052d402ce`; journal `ead8b0726fd18c96df8f936027d7f33ac121d5a2f12289144a6a17c18f07f6b3`; lifecycle `c6b75bd0c7eba175cf1c1bfaa5d5095cae939fc775f44eca97bca22abbb09e7f`; checkpoint `2319c6a3867496f0fbf90b7fe97b8992b9c300d23bd38917e46f075d2bf8f104`; journal types `172f76f83dc262ea90bc4ad21ff4179da6d1b4a08e4cae7582abc3e25236bac3`; broker mod `51012997653d6f4c53d25de956a8a8d718ca14bc93e3000a58898a182a073504`; validation `5733a07b6fc34ccdac6512fc366202291a61cb48f428ece3832ec9c486d3cf15`; journal tests `067e36fd7da20a360d3661b8263f7ca8776221e2da491ad5a460812553787915`. Manifeste complet tests+app : `/tmp/ai-guard-broker-lifecycle-final/coverage-expansion/final-manifest.sha256` (SHA `c69c9aa5ddccbdc4a272c473661b4029dd98b87d4eab510c95c965c3b8ae17ae`). Les sources, logs et metadata sont dans des dossiers privés ; fichiers en mode 0600.

Les tentatives intermédiaires sont conservées séparément et ne sont pas comptées comme succès : le premier compile a trouvé un move de fixture (`E0382`), puis les rejeux lifecycle ont corrigé les oracles de floor observé après reopen et de version refusée sans exiger un variant d’erreur non contractuel, puis les fixtures d’accept markers ont reçu le mode privé attendu et la séquence 64 commandes a conservé son préfixe confirmé. Logs bruts disponibles pour RC101 : `/tmp/ai-guard-broker-lifecycle-final/coverage-expansion/final2/broker_lifecycle-release.log` (5/6, SHA de test `98fbbadb8ce752686166878a637bee5363a975815f3f401f006c0f9d2bf5bbf6`), avec les étapes antérieures et leur cause conservées en snapshots `frozen/`, `retry/` et `final/`. Les sorties des premiers échecs et du premier run combiné de checkpoint n’avaient été capturées que dans la transcription, aucun fichier brut n’est revendiqué pour eux. Le graphe checkpoint n’est compté qu’une fois dans le relevé final ci-dessus ; son rejeu a été réalisé pour créer le log brut manquant.

### Dernières assertions de revue

Après revue, la cible lifecycle a reçu un dernier complément sans changement app : avance V1 réelle avant V2 et replays V1/V2 avec leurs références CAS d’origine après V3 ; replay V3 comparé sur opération, révision et hash ; approbation V1→V3 directe et via V2 par boucle ; refus d’une auto-approbation V3 avec Decision Approved pending mais sans commande `ApprovalSubmitted` ; version de commande inconnue et champ nullable supplémentaire sur `CancelRequested`, vérifiés via `from_bytes` et Deserialize public ; hash d’une commande nouvelle liée au bon job mais à un intent erroné.

Le gel lifecycle final `tests/broker_lifecycle.rs` SHA `a371631492665e8f593b1873f3136f3adac1dfad8fc1d5d61154c593625898c4` passe `cargo test --release --locked --test broker_lifecycle -- --nocapture` 6/6, RC 0 (1,41 s de tests). Log brut `/tmp/ai-guard-broker-lifecycle-final/reviewer-final/broker-lifecycle-release.log`, SHA `fb69734f7d17e490c38910f7eefed36cc02d33e358ee631897a31411f9168259`. Sur ce gel final, `cargo fmt --all -- --check` et `cargo clippy --locked --all-targets -- -D warnings` passent, logs `/tmp/ai-guard-broker-lifecycle-final/reviewer-final/fmt-final.log` et `clippy-final.log`. Le checkpoint n’a pas été relancé après cette extension isolée du test lifecycle; son log final précédent reste `/tmp/ai-guard-broker-lifecycle-final/coverage-expansion/broker-checkpoint-final.log`, test SHA `9cb7aed956c27e51569b582c928576894e800c27aa5139b5f8caaabe0cb2573c`, 6/6 et 32,23 s. Manifeste final app/tests : `/tmp/ai-guard-broker-lifecycle-final/reviewer-final/final-manifest.sha256` (SHA `ea9d63527d1a300e58f051a2e47e4c1a7550a52a2cea0a26d9aaab8beb839f75`).

### Correction sur la durée de quota et chemins des binaires

Correction documentaire du 31 octobre 2026, sans changement des logs historiques : le quota central est libéré par la transaction Store `CancelReserved` avant l’ACK du checkpoint. Si la réponse est perdue, l’opération locale peut rester pending alors que le quota central est déjà libéré. L’oracle vérifie séparément qu’avant l’appel Store, le pending n’a pas libéré le quota, puis que la vraie mutation Store le libère ; il ne prétend pas que l’ACK déclenche la libération.

Les binaires de test présents sous `/Users/louis/Documents/ai-guard/.worktrees/broker-supervision/target` sont :

- Lifecycle release final : `release/deps/broker_lifecycle-8863d2b5e78080fc`.
- Checkpoint release : `release/deps/broker_checkpoint-fbf2367c2d397b9c`.
- Floor debug : `debug/deps/journal_lifecycle_floor-ee81ce73269bf1a5`.
- Admission client debug : `debug/deps/broker_admission_client-75cbe5c50ac6910c`.
- Journal broker debug : `debug/deps/broker_journal-b36c0a76d0e7000e`.
- Unit tests journal debug : `debug/deps/agentsandbox-31f20f78acafd423`.
