# Correction de la relecture des transitions de contrôle — 29 octobre 2026

Ce complément remplace le gel partiel Prepare/Created comme preuve finale du code core. Les anciens rapports et essais Docker restent des résultats intermédiaires datés. Ce rapport ne qualifie ni release ni moteur réel.

## Reproduction et frontière

La revue indépendante a signalé que les deltas exacts n'étaient rejoués que pour Prepare et Created. Un terminal n'était revalidé que lorsqu'un objet allocation était déjà présent. Le journal et ses projections pouvaient donc être cohérents entre eux tout en décrivant une transition interdite.

Reproduction effective : après admission puis Unknown, une connexion SQLite externe conserve l'action unknown mais remplace le snapshot par state=terminal/quota_held=false, sans observation ni allocation. Le test recalcule le snapshot hash, le payload et le hash du dernier événement, quota.bytes et les projections exactes workers/worker_attempts/worker_mutations. Avant correction, Store ouvre healthy=true et ce quota est libéré sans preuve terminale.

Précondition : accès direct en écriture à la base et recalcul de ses hashes non signés. Ce n'est pas un accès disponible via l'API ou un worker. Aucun secret ni moteur manipulé dans la reproduction.

## Changements

Seuls `src/audit/control.rs` et `tests/worker_registry.rs` ont été modifiés. Le codec ControlRecord version1, les DTO publics, le schéma SQLite v2 et la migration sont inchangés.

- La logique existante de toutes les transitions est extraite vers `apply_worker_transition`, fonction déterministe sans accès SQL ni runtime, appelée par la mutation réelle et la relecture du journal. Préconditions, latches de preuve, claims, quota et calcul exact d'allocation ont une seule implémentation.
- La relecture ferme la liste des actions. Elle recompose le WorkerMutationV1 original à partir de l'opération, du worker précédent, de l'action, de l'inspection créée, de l'emitter, du motif Unknown, de l'observation ou du résultat de cleanup selon l'action. Son input_hash complet doit correspondre au journal.
- Après application de la logique commune, le snapshot complet, l'observation et l'allocation doivent être exactement ceux du journal. Aucune mutation n'accepte des champs de refus ou d'admission. Unknown/Stop/etc ne peuvent ainsi libérer un quota, fabriquer un claim ou produire un terminal ; Observe ne peut omettre son allocation calculée ; Cleanup conserve un terminal déjà justifié sans créer un second ledger.
- Une admission initiale doit rester Reserved, quota tenu, revision1, avec identité d'admission liée à l'opération, reçu version1, aucun claim/CID/inspection, aucune preuve d'exécution, d'incohérence, d'Unknown ou de cleanup, aucun ledger et aucun refus. Une admission refusée reste sans worker/snapshot/révision/observation/allocation et porte un refus enum fermé.
- La relecture de l'admission acceptée recoupe également son reçu avec un événement security.decision antérieur exact, hash identique, source gateway/provenance observed, résultat Allow et bonne phase. Tous les champs de DecisionRequest sont reconstruits depuis cet événement ; lookup_decision vérifie son historique et la phase, puis AdmissionV1.validate et son input_hash complet doivent correspondre au record. Le reçu forgé a été reproduit avant ce complément de correction.
- Les reprises InDoubt ajoutées au lot précédent gardent leurs conditions restrictives ; CancelReserved reste interdit après une création.

## Vérifications constatées

Toolchain Rust/Cargo 1.99.1 ; PATH Rust/Homebrew explicite et login:false.

- RED : `cargo test --locked --test worker_registry rehashed_database_`, 4 tests échouent sur les assertions métier attendues avant correction : Unknown terminal sans preuve, admission initiale terminale, CreateIssued sans claim, terminal sans allocation. Log brut `/tmp/ai-guard-worker-replay-red.log`.
- RED complément reçu : `cargo test --locked --test worker_registry rehashed_database_admission_requires_its_exact_prior_allow_and_input`, échec métier attendu `admission accepted receipt_hash`. Log brut `/tmp/ai-guard-worker-replay-receipt-red.log`.
- PASS : `cargo test --locked --test worker_registry --test usage_ledger --test control_migration`, registry31 + usage2 + migration4 = 37 tests, 0 échec, 0 ignoré. Log brut `/tmp/ai-guard-worker-replay-green.log`.
- PASS intermédiaire avant le dernier complément reçu : `cargo clippy --locked --all-targets -- -D warnings`, exit0.
- BLOCKED temporaire au dernier gel : même commande Clippy exit101 pendant l'édition parallèle de `src/operations.rs` ; fonctions `require_successful_run` ligne260 et `require_successful_resume` ligne283 encore absentes de cette édition. Aucune erreur des fichiers possédés relevée ; ne vaut pas PASS final. Root informé, relance après son gel. Log brut `/tmp/ai-guard-worker-replay-clippy.log`.
- PASS : `rustfmt --edition 2024 --check src/audit/control.rs tests/worker_registry.rs`, exit0.

Les négatifs recouvrent les actions prepare/created/create_issued/start_claim/stop/cancel_reserved/observe/cleanup_result/inconnue, quotas, claims, admission initiale déjà exécutée, terminal sans observation/allocation et input_hash divergent, avec réouverture et contrôle du latch. Pour les mutations reconstructibles, le test ajuste aussi l'input_hash pour tester le delta, pas seulement un hash resté ancien.

Le positif rejoue après fermeture/réouverture chaque étape de Prepare→CreateIssued→Created→StartClaim→Observe(Running)→Stop→Observe(Terminal)→Cleanup échoué→Cleanup réussi, puis un second worker CancelReserved. Il vérifie intégrité, reçus idempotents, replay StartClaim AlreadyConsumed, allocation unique exacte de 2 secondes, billable=false et quota final0. Les régressions précédentes de quotas, redémarrage, audit plein, observation contradictoire et migration restent vertes.

Un autre positif crée Review puis Approved avec project/sandbox/agent/session/correlation/trace renseignés, admet, ferme et rouvre, puis vérifie le reçu rejoué et le quota conservé. Les négatifs reçus changent hash, séquence, phase et input d'admission sans casser les projections/hashes de la base.

Empreintes du gel transmis au reviewer :

- `src/audit/control.rs` : `c515f12bef44db458b8efcccb2a0b5d6282558537ffbdb16273bb99df1d68e0d`.
- `tests/worker_registry.rs` : `f984711fad3d2564562c45631a9516e9ca9575aaae09e3f5492ba4e9ae87d56c`.

## Limites et suivi

Les mutations v1 et les admissions acceptées sont entièrement reconstructibles pour leur hash d'entrée en recoupant les décisions de la même base. Une admission refusée ne conserve ni worker ni lien reçu/request dans ControlRecord ; seule sa forme fermée est reconstruite, sans prétendre recalculer son input_hash ou la décision de refus depuis des inputs absents. Le snapshot complet de politique historique et l'autorité de Jev ne sont pas réévalués par cette correction.

La validation sémantique n'authentifie pas un historique complet réécrit de manière cohérente par un auteur disposant d'un accès SQLite et capable de recalculer des hashes non signés. Elle refuse les contradictions de la machine à états et des projections. Aucun nouveau mécanisme de signature ni changement de frontière de confiance n'a été introduit.

NOT_RUN par cet agent après ce gel : suite globale intégrée, build release, front et essais moteurs réels. Root reprend ces validations. Le test moteur ignoré ne devient jamais PASS. Le verrou OS, l'identité tenant/origin, les observations runtime et la conservation d'archive/manifest relèvent toujours du broker/runtime.

PASS revue indépendante source : `review_registry_integration` a relu les empreintes finales ci-dessus, le RED reçu et le GREEN37, puis confirmé l'absence de nouveau blocage ; lien Allow antérieur, phase/historique et hash d'admission réassemblée jugés cohérents. Les vérifications globales de root restent distinctes. Aucun fichier d'un autre propriétaire, configuration, volume ni travail préexistant effacé ; aucun commit effectué.
