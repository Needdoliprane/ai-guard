# Cycle de vie local durable et client d'admission supervisée

31 octobre 2026, Europe/Paris ; base `4890df13f56f2a61d9ee773f9d1dbd7cca929e72`, worktree `broker-service`. [Contrat](../../../BROKER_JOURNAL.md#extension-de-cycle-de-vie--contrat-du-lot-suivant31octobre2026), [implémentation core](implementation.md), [rapport Luna](tests.md), [revue indépendante](review.md) et [revue de consolidation](docs-review.md). **CLOSE logiciel/couverture ciblée/docs/preuves/artefacts** après correction de BLC-01 à BLC-04. Le cycle du service permanent reste ouvert.

## Comportement livré

Le journal peut rejeter durablement une soumission avant intention valide, retrouver ce rejet au replay exact et refuser une divergence. `status` et les pages bornées distinguent réservation, interruption, rejet et job accepté. Les planchers vivants et tous les IDs déjà observés sont revalidés, même hors de la page demandée. Cela ne garantit pas un anti-restore après redémarrage ni la validité des corps de jobs jamais observés hors page.

Une enveloppe V3 ajoute les commandes locales typées d'approbation, d'annulation et d'échec à la chaîne existante, sans modifier les octets/domaines V1/V2 ou les plans1/2/3. L'approbation exige un vrai Review déjà acquitté puis deux commits distincts. Cancel interdit les nouvelles actions concernées dès sa naissance ; une requête centrale inconnue reste à réconcilier exactement. Les commandes et échecs locaux ne constituent ni un terminal runtime ni une libération de quota.

Un export postmortem exige un nouvel ExportRequest né strictement après le **dernier** Cancel, puis son vrai ACK central. Un ancien grant, un ACK tardif, une demande co-née avec Cancel ou un ancien Arm ne suffisent pas. Les faits tardifs d'opérations déjà émises restent enregistrables sans autoriser une nouvelle émission.

La nouvelle méthode `AuditClient::admit_supervised` utilise la route existante `/api/v1/workers/supervised-reservations` et le credential explicite du client. Elle conserve les reçus des vrais refus Policy403/Quota429, valide le Worker initial accepté et laisse les erreurs d'authentification, transport ou protocole en erreur inconnue. Elle ne persiste rien et n'autorise aucun start ; scope, namespace attendu et état central doivent encore être recoupés par le service/journal.

La transaction centrale `CancelReserved` libère le quota **avant** l'ACK du checkpoint. Une réponse perdue peut donc laisser le journal pending avec un quota déjà libéré. Preparing sans CID et une cible jamais démarrée sans terminal admissible peuvent encore conserver quota/NeedsReconciliation : écart opérateur ouvert avant cutover.

## Validation ciblée

Exécution par Luna sur macOS ARM64, Rust/Cargo1.99.1, cache `CARGO_TARGET_DIR=/Users/louis/Documents/ai-guard/.worktrees/broker-supervision/target`. Les commandes suivantes sont exécutées depuis la racine du worktree.

| Commande | Résultat final constaté |
|---|---|
| `cargo test --release --locked --test broker_lifecycle -- --nocapture` | [**PASS6/6**](logs/lifecycle-final.log), RC0,1,41s ; source finale `a371631492665e8f593b1873f3136f3adac1dfad8fc1d5d61154c593625898c4`. |
| `cargo test --locked --test journal_lifecycle_floor -- --nocapture` | [**PASS2/2**](logs/floor-final.log), RC0 ; accepted disparu et réservation connue corrompue hors page. |
| `cargo test --locked --test broker_admission_client -- --nocapture` | [**PASS4/4**](logs/admission-final.log), RC0 ; vrai Axum et SQLite3, création/replay, refus Policy/Quota/replay, auth, mutation technique de deux champs initiaux d'un vrai DTO201. |
| `cargo test --release --locked --test broker_checkpoint -- --nocapture` | [**PASS6/6**](logs/checkpoint-final.log), RC0,32,23s ; test final `9cb7aed956c27e51569b582c928576894e800c27aa5139b5f8caaabe0cb2573c`. |
| `cargo test --locked --test broker_journal -- --nocapture` | [**PASS12/12**](logs/journal-canary.log), RC0 ; compatibilité, CAS, limites, bindings et planchers existants. |
| `cargo test --locked --lib broker::journal::tests -- --nocapture` | [**PASS5 parents**](logs/journal-units.log), RC0 ;2helpers ignorés au niveau supérieur et appelés par leurs parents. Pas de double décompte des enfants. |
| `cargo fmt --all -- --check` | [**PASS**](logs/fmt-final.log), RC0. |
| `cargo clippy --locked --all-targets -- -D warnings` | [**PASS**](logs/clippy-final.log), RC0 sur le test lifecycle final. |
| `cargo build --release --locked` | [**PASS**](logs/build-final.log), RC0 ; aucune source applicative modifiée ensuite. |

35 fonctions uniques ciblées au total, dont contrat d'API, diagnostic et auto-test du harnais. Ce nombre n'est pas un compteur de garanties runtime. Le dernier complément d'oracles ne modifie que `broker_lifecycle.rs` ; le graphe checkpoint n'a pas été rejoué après ce complément isolé.

Sont exercés : versions et champs inconnus nullable/EOF par parse canonique et Deserialize publique ; contexte opération/intent des hashes ;64 commandes puis refus65 ; vrais anciens replays V1/V2 après V3, replay historique V3 complet ; approbation historique directe V1→V3 et via V2, refus d'auto-approbation nouvelle ; rejet relu après reopen puis disparition/substitution contre son plancher observé ; marqueurs doubles/accept partiel ; Admission inconnue puis vrai Store Reserved→Cancelled ; dernier Cancel et grants Export. Les fixtures runtime du graphe sont **synthétiques**. Journal, Store, transactions, reçus et HTTP sont les composants réels. Les hooks d'erreur IO sont `cfg(test)` ; les SIGKILL du harnais journal ne qualifient pas un crash du service.

Les erreurs intermédiaires de compilation/fixture et oracles insuffisants sont décrits dans [tests.md](tests.md) et [review.md](review.md), avec leurs sources/logs privés conservés. Aucun ancien GREEN n'est transféré au SHA final. Les premières sorties seulement transcrites ne sont pas présentées comme des fichiers bruts disponibles. Trois allowances `too_many_arguments` sont locales aux helpers de test, motivées et sans effet sur le code applicatif.

## Défauts reproduits et corrections

| Défaut | Preuve avant correction | Correction vérifiée |
|---|---|---|
| Accepted confirmé redevenait Interrupted si son marqueur disparaissait | [RED base](red/accepted-floor.log), RC101, `Ok(Interrupted)` | Plancher checked avant les retours non accepted ; tous lecteurs concernés refusent la disparition connue. |
| BLC-01 : ancien grant Export réutilisable après Cancel | [RED GEL1](red/export-cancel.log), RC101 | Frontière historique dérivée du dernier Cancel, vérifiée lors de nouvelle préparation/Arm/claim Export. |
| BLC-02 : réservation connue corrompue ignorée hors page | [RED GEL1](red/observed-page.log), RC101 | Ensemble borné de tous les IDs observés, revalidation avant pagination. |
| BLC-03 : enum de rejet publique reflétait une valeur inconnue dans son erreur | [RED GEL1](red/rejection-enum.log), RC101 | Désérialisation opaque. Le lecteur Journal était déjà opaque ; aucune fuite d'un caller produit n'est inventée. |
| BLC-04 : client acceptait un Worker initial HTTP avec un fait de start ajouté | [RED client GEL1](red/initial-worker.log), RC101 | UUID/formes et tous les champs initiaux recoupés. Le test final couvre aussi attempt_id nil. |

Les RED de compilation [lifecycle](red/lifecycle-api.log) et [client](red/admission-api.log) montrent seulement l'absence des nouvelles API avant implémentation, pas une vulnérabilité.

Pour reproduire dans une **copie isolée du commit de ce lot**, jamais dans le checkout actif :

- BLC-01/02/03 : inverser `red/core-gel1-to-final.patch`, puis copier respectivement [export-cancel-test.rs.txt](red/export-cancel-test.rs.txt), [observed-page-test.rs.txt](red/observed-page-test.rs.txt), [rejection-test.rs.txt](red/rejection-test.rs.txt) vers `tests/broker_checkpoint.rs`, `tests/journal_lifecycle_floor.rs`, `tests/broker_lifecycle.rs`. Exécuter seulement la fonction nommée dans le log correspondant avec `--exact --nocapture` ; ajouter `--release` pour le graphe checkpoint. Attendu RC101 à l'oracle décrit.
- BLC-04 : depuis le gel final, inverser seulement `red/runner-gel1-to-final.patch`, copier [initial-worker-test.rs.txt](red/initial-worker-test.rs.txt) vers `tests/broker_admission_client.rs`, puis exécuter la fonction du log avec `--exact --nocapture`. Attendu RC101.
- Plancher préexistant : partir de la base4890df13, copier [accepted-floor-test.rs.txt](red/accepted-floor-test.rs.txt) vers `tests/journal_lifecycle_floor.rs` et exécuter la fonction du log. Attendu RC101.

Les deux patchs inverses passent `git apply --reverse --check` sur le gel final ; aucune inversion ni nouveau RED exécuté pour produire cette documentation. Source RED Export SHA `2607bbf4e950bea07d5aa2a6d7cb45c2502bcebb3204bb081c432bdb06406fd0`, client `8656370ca9b40dbc2d5d1386636d0507eae19c3803c0fff9d58b9b3b4500fd9f`, accepted-floor `7d570cf19b975fdce258f77f79a8a1043dbf75e7b91511f8ec36744835d3def8`. Pour l'enum, snapshot draft5 SHA `f68beaf60f332fe3d276d9d7ea6bec5e047fdc41fb4249a6451b7b0d83571bac`, oracle ligne347 et chronologie concordants ; draft6 est byte-identique. Aucun manifeste d'invocation ne permet de départager ces deux chemins identiques. Le log original reste distinct de cette corrélation de provenance.

## Gels et artefacts

[Manifeste des122sources Rust/Cargo/tests](SOURCE_SHA256SUMS.txt), SHA256 `130d00a58ac09177b7417af778fb2de8e7805c26f7b7e7267188a7e05c5e652f`. [Checkout](logs/source-verify.log) et [archive extraite](logs/archive-verify.log) :122/122 conformes, RC0. Archive privée `/tmp/ai-guard-broker-lifecycle-qualified.BSOxFm/source.tar`, SHA256 `2ca9dfb1bf59414ee7db2effaf585d2ff1dc5cb8b03b0ff4ba23f40b48c52aa8`,122entrées tar, sans AppleDouble (`COPYFILE_DISABLE=1`). Dossier700, archive/manifeste/artefacts600. Aucun credential, binding privé ou binaire ajouté à Git.

[Manifeste des huit artefacts natifs](ARTIFACT_SHA256SUMS.txt), préservés hors target dans ce même dossier privé. Binaire release macOS SHA256 `3d021998057c0415db59c8531d05efa637ab5a2296f08ee41f9115c65a2d95e5` ; test lifecycle `9bfce4d3978b7fc8e5c341e8767c1d45f318bdaf13c9a26fac9813d9a95fc0c3`, checkpoint `d510e4fb7e467421fc4d0f6732bb4a6b84bce46851b8a0829e9e24bcc4e7ac2e`. Ces SHA décrivent les octets observés, sans promesse de build bit-à-bit reproductible ou d'image qualifiée.

GEL1/GEL2 core conservés dans `/tmp/ai-guard-broker-lifecycle-gel1.6aXQOu` et `/tmp/ai-guard-broker-lifecycle-gel2.IGgtI8`, client avant/après dans `/tmp/ai-guard-broker-admission-client-gel1.1mPM9e`. Le [manifeste des pièces](SHA256SUMS.txt) exclut lui-même. Les scans du delta Git recherchent types de fichiers, clés connues et motifs ; ils ne constituent pas une DLP générale.

## Limites et suite

Service/IPC/enforcer concret, génération de cancel mémoire et dernier garde, helpers réels, gardien TTY, mort du CLI, D5 et reprise : **NOT_RUN/non livrés par ce lot**. Docker/Podman réels, autres plateformes, recette A1/A2/B1, Kubernetes/cloud et charge100 : **NOT_RUN** ici. Aucun E2E général, modèle, fournisseur, frontend ou déploiement rejoué. QA, défaut schéma2 et `billable=false` inchangés ; G09 reste **FAIL**,0/12gates fermés.

Prochaine priorité : raccorder le vrai service et son contrôleur préarmé aux ports et formats livrés. Le rappel Daybreak Blue demandé par le mainteneur reste lié à la stabilisation de ce service et aux vrais tests d'arrêt/reprise ; aucun audit global ni horaire automatique créé.
