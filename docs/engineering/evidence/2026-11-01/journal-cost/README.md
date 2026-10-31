# Coût du journal et des gardiens — 1er novembre 2026

Base applicative `b32711d`, worktree broker-service. Ce lot mesure avant d'optimiser ; il ne qualifie ni le démarrage, ni l'arrêt/reprise, ni D5. **G09 reste FAIL et aucun gate n'est clos.** Aucun worker, modèle ou service nouveau lancé, aucune ressource existante arrêtée ou nettoyée.

## Observation passive du gardien H10

Le [protocole source](guard-observation-protocol.md) attribue passivement le processus à la racine QA retenue `/private/tmp/br-euAmXg` par les FD du verrou de rôle et du socket exact, sans connexion IPC ni lecture de capability. Les [commandes réellement exécutées](guard-observation/commands.txt) utilisent seulement find/lsof/ps et une attente20s. Les sorties sélectionnent les chemins connus et les champs scalaires ; aucun argv ou environnement collecté.

L'attribution initiale et finale lsof montre le même PID23300/PPID1, FD9 pour `.owner.lock` et FD14 pour `endpoints/i-a5818dc2b2df49fc/s`, sans autre candidat. Les snapshots [avant](guard-observation/ps-before.txt) et [après](guard-observation/ps-after.txt) portent le même démarrage `Sat Oct 31 22:32:40 2026`. CPU propre cumulé23:16.55→23:21.15, soit **+4,60s**, RSS **7664KiB** aux deux bornes. ELAPSED passe01:47:07→01:47:28 : l'attente demandée20s n'est pas un intervalle exact, les bornes arrondies indiquent environ21s. Aucun taux CPU précis n'en est annoncé. Le `%CPU` natif n'est ni une moyenne exacte de cette fenêtre, ni nécessairement une moyenne de toute la vie du processus ; la formulation « lifetime » du résumé brut est corrigée ici.

Aucun enfant direct n'est capturé à la première borne ; la seconde montre `./docker`, PID73225/PPID23300, CPU cumulé0:00.04 et RSS26160KiB. Les enfants brefs entre les bornes et le daemon/VM ne sont pas mesurés. Le CPU/RSS du parent n'est donc pas le coût global ni un coût par Arm. Le COMM du parent pointe vers `broker-supervision/target/release/agentsandbox` ; un chemin d'exécutable n'atteste pas le hash du code chargé, d'autant que ce target est réutilisé pour les builds. Les preuves H10 historiques restent distinctes du code actuel.

Le [résumé original](guard-observation/summary.txt), les RC0 et stderr vides sont conservés. Cette mesure établit un coût sur un processus vivant attribué, sans isoler la part de chaque boucle ni prouver une cause de H10. Les boucles stop et les slots retenus décrits dans [l'analyse D5](../../2026-10-31/broker-runtime/d5-source/retirement-analysis.md) ne deviennent pas retirables grâce à cette observation.

### Échec initial d'inventaire conservé

La [première capture](guard-observation-initial/summary.txt) annonçait zéro socket et s'arrêtait avant lsof/ps. Root constate ensuite par ls/stat un vrai socket au chemin ci-dessus ; l'inventaire corrigé `/usr/bin/find ... -type s -print` le retrouve, RC0. La commande initiale exacte n'a pas été retenue : sa cause précise n'est pas établie. Cette première capture est **FAIL de harnais d'inventaire**, pas preuve d'absence du gardien ou de coût nul. Aucun contenu d'annonce, credential ou socket n'a été interrogé pour corriger l'inventaire.

## Mesure Journal/Writer en cours

La [cartographie de source](source-design.md) identifie les Full loads, validations, lectures et synchronisations ainsi que les fixtures existantes. Ses décomptes restent des attentes à confronter aux mesures, sans cause H10 présumée. Le [plan actif](../../../IMPLEMENTATION_PLAN.md#mesure-active-du-coût-durable--1er-novembre-base-b32711d) prévoit un probe uniquement `cfg(test)`, un vrai Journal/Writer neuf sans moteur et les heads0/8 ; temps inclusifs et appel exact `File::sync_all` doivent rester séparés.

**État avant S5 — NOT_RUN :** cible de mesure Journal, régressions/checks du probe, performances release, autres plateformes/FS, D5/fencing et nouveaux essais moteurs. Les appels applicatifs et leurs barrières ne sont pas optimisés par cette documentation.

### S5 — RED du probe, distinct d'un échec métier

Au premier gel160sources, le collecteur et le test sont présents, les hooks encore absents. Compilation debug réussie4min13s, puis **RC101/test0,17s** : vrai Writer.load head0→0/ok, observation fermée imprimée, assertion `FileSync.calls=0` contre6 attendu. [Log exact](s5-red/test.log) et [manifeste vérifié avant/après](s5-red/SHA256SUMS). Le résumé brut parle de succeeded ; le code/assertion vise **calls**, succeeded étant également0 dans cette observation. Le RED prouve l'observation manquante du probe, pas une défaillance du Writer, ni la cause de H10. Le champ cpu_release constant de cette première version est remplacé au gel suivant par le booléen factuel debug_assertions ; la commande de compilation est la preuve du profil.

S6 branche les hooks uniquement cfg(test), conserve les oracles de comptage et renforce le canari CAS (mauvais hash sur la révision courante). Borne600s d'observation **coopérative** : un appel FS synchrone peut retarder son observation. Le timeout60s du Writer borne l'attente caller, sans retirer ni annuler une commande déjà acceptée. Aucun arrêt dur du processus ni borne disque n'est garanti. État au lancement S6 : cible GREEN en cours, sans résultat de performance revendiqué à ce point.

Une collecte optionnelle de piles par `sample` a été examinée dans son manuel local, puis laissée **NOT_RUN** : l'outil suspend/reprend périodiquement tous les threads et ses occurrences incluent des attentes, sans donner directement la répartition CPU de la fenêtre. Aucun sampling ni interruption du gardien retenu n'est ajouté à cette observation passive.

### Représentativité à préserver

Root relève uniquement les tailles par `stat -f '%z %N'` des fichiers `rev-*.json` du job H10 retenu, puis projette nom de révision et octets : [31 fichiers,866540octets logiques](h10-revision-sizes.txt), dernière entrée numérotée30 de47991octets. Aucun contenu n'est lu pour ce relevé ; ce n'est pas une nouvelle validation du chaînage, de son head sémantique ou de sa durabilité. Le probe heads0/8 sans binding ni supervision active est une première mesure des primitives, pas une reproduction de ce graphe et de ses délais. Un résultat rapide sur cette fixture ne réfute pas les lenteurs de H10 et n'autorise aucune extrapolation à100workers.

### S6 — observations branchées, résultats debug ciblés

Le test opt-in passe **1/1 en7,38s** sur le vrai Journal/Writer neuf, après compilation debug. Huit mesures séquentielles, pas huit tests, vérifient les mêmes heads, reçus locaux, CAS et compteurs ; aucun ACK central HTTP n'est simulé comme réel. Les quatre canaris `control_fs::descriptor_tests`, le recontrôle de binding après publication1/1 (1,56s) et le canari d'erreurs IO injectées1/1 (0,79s) passent aussi : **7 fonctions uniques** au total. Le filtre initial `control_fs::tests` donné par Root ne sélectionnait aucun test : **NOT_RUN**, log conservé, puis correction en descriptor_tests sans rejouer la mesure.

| Opération | Head avant→après | Caller ms | File sync_all ms | Directory sync_all ms | Directory revalidate ms, inclusif |
|---|---|---:|---:|---:|---:|
| Load1 | 0→0 | 22,863 | 3,815 | 0,009 | 14,291 |
| Load2 | 0→0 | 95,153 | 65,403 | 0,004 | 20,832 |
| Status | 0→0 | 59,370 | 36,353 | 4,454 | 14,234 |
| Advance lifecycle | 0→1 | 292,668 | 15,105 | 3,379 | 225,424 |
| Load1 | 8→8 | 188,903 | 84,646 | 0,002 | 25,991 |
| Load2 | 8→8 | 371,133 | 131,143 | 0,003 | 30,078 |
| Status | 8→8 | 320,708 | 73,941 | 9,286 | 36,465 |
| Advance lifecycle | 8→9 | 765,696 | 151,438 | 4,429 | 553,927 |

Les trois lectures à head0 comptent chacune6 file-sync et3 dir-sync ; à head8,14 et3. Les mutations comptent respectivement16/9 et32/9, avec2 créations et1 publication du pointeur. Aucun span abandonné, erreur ou span outstanding dans ces huit mesures terminées. Les compteurs concordent avec la source ; la variabilité entre deux lectures du même head interdit d'en tirer une distribution, un percentile ou une garantie de délai.

Les spans inclusifs se chevauchent : ne pas additionner leurs colonnes. Les octets comptés sont ceux retournés par les appels réussis, avec relectures et caches ; ils ne sont ni un volume de disque physique ni la taille logique unique du journal. Une lecture partielle échouée n'est pas une mesure complète de ses octets. La reconstruction/canonicalisation de chaîne n'est pas isolée : le résidu de temps ne prouve pas son CPU. L'instrumentation et l'environnement peuvent ajouter du coût, sans mesure séparée de cet effet.

Ce gel utilise `debug_assertions=true`, aucun résultat CPU/performance release. Les chemins hors tests développent les corps originaux ; aucune synchronisation, validation ou IO métier n'est supprimée. Les métriques localisent des coûts et justifient une mesure sur le graphe plus riche ; elles ne prouvent pas une correction de H10. État au début des checks S6 : fmt-check passe, Clippy/build release en cours ; le bilan final ci-dessous remplace ces statuts intermédiaires. Revue source indépendante sans blocage identifié, revue finale des logs et artefacts encore attendue.

### Piste D5 — source, sans permission de retrait

La [lecture ciblée du CLI Docker et de Moby](d5-normal-completion-source.md), recoupée par Root sur les sources primaires épinglées, distingue un POST unique à réponse normale reçue des annulations, réponses perdues et retries. Elle ne démontre pas l'identité des binaires déployés et ne construit aucun témoin. Le CLI reste qualifiable en principe ; aucune nécessité de le remplacer par un nouveau transport Rust n'est prouvée par cette lecture.

Un écart de preuve précis reste ouvert : la configuration Docker du compte de contrôle peut influencer les headers HTTP, sans contenu figé dans l'identité d'invocation actuelle. Certains headers peuvent rendre un POST rejouable dans le transport étudié. L'état de cette configuration n'a pas été lu ; l'intercalation d'une requête retardée est NOT_RUN, aucune cause H10 ou vulnérabilité exploitée n'en est déduite. Un futur témoin exige transport/configuration attestés, émetteur exact, absence de requête encore inconnue, terminal indépendant et ACK central ; aucun ancien slot ne peut être désarmé depuis cette note.

## Bilan final S6

**PASS limité au logiciel et aux observations nommées :**7 fonctions ciblées, fmt-check, Clippy strict all-targets et build release RC0 (12min16s de compilation). [Revue indépendante finale](s6-green/independent-review.md), aucun P0/P1 restant identifié dans les quatre fichiers et l'interprétation examinés. Le test ignoré a réellement été sélectionné avec `--ignored` ; le filtre vide est NOT_RUN, sans addition au compte. Environnement consigné : Rust/Cargo1.99.1, Darwin27.2.0 arm64, macOS27.2 ; type exact du FS **NOT_RECORDED**. Aucun E2E général, frontend, modèle ou nouveau moteur n'est exécuté.

Les [160 sources S6](s6-green/SHA256SUMS) concordent avec snapshot, checkout et archive extraite, vérifiés indépendamment par Root. Manifeste SHA`0f8e37354f14b9d177f250622807f0b3a7e520efdcfd30fd0ceb6ea9a43026b1`, archive privée SHA`e0e804536f7085d52355343e849865a2f2bb16ac668acf70a01fdb2f09f6ce55`. Les [copies binaires](s6-green/release-binaries.sha256), mode600 hors target et Git, sont vérifiées : agentsandbox`411a840f1cff7563921c5bca8f814340bcc433c093059d7512cdec724068d0d8`, provider`222e7e07ba94d2bc3e505fa839fb9bdbd808e24776c5b1d127500b709493c8a5`. Aucun déploiement de ces binaires ni promesse de reconstruction bit à bit.

Les [huit JSON](s6-green/cost-results.jsonl), le [tableau brut](s6-green/cost-measurements.tsv), les RC/logs et les deux erreurs de sélection/inventaire sont conservés. Les colonnes head du tableau brut suivent le résultat ; le tableau ci-dessus distingue avant→après. Les variantes H10, Linux/Podman/Kubernetes,100workers, perf CPU release, panne disque, arrêt/reprise et D5 sont **NOT_RUN / ouverts**, sans conversion en succès. Aucune qualification produit, aucun retrait de gardien, aucune cause H10 et aucun gate clos.

### Rejouer uniquement la mesure et ses canaris

Depuis la racine du dépôt avec la toolchain fixée :

```sh
CARGO_INCREMENTAL=0 cargo test --locked --lib broker::journal::tests::measure_durable_journal_cost_without_runtime -- --exact --ignored --nocapture --test-threads=1
cargo test --locked --lib control_fs::descriptor_tests -- --nocapture
cargo test --locked --lib broker::journal::tests::full_writer_rechecks_binding_after_real_publication_before_ack -- --exact --nocapture
cargo test --locked --lib broker::journal::tests::injected_journal_io_error_never_returns_ack_and_recovery_is_conservative -- --exact --nocapture
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo build --release --locked
```

La fixture du diagnostic est volontairement conservée même après échec ; fournir au processus un TMPDIR privé neuf pour la retrouver sans publier son chemin dans les métriques. N'utiliser ni racine de journal existante ni secrets réels. Les autres canaris gardent leur gestion de fixtures habituelle. L'option `--release` n'a pas été employée pour ces mesures : elle ne serait pas une nouvelle qualification par simple changement de commande.
