# Coût du journal et des gardiens — 1er novembre 2026

Travaux amorcés sur `b32711d`, poursuivis depuis `f6f93b3` dans le worktree broker-service : mesures, factorisation canonique et contrôles ciblés. Les sections gardent leurs gels et résultats datés ; la [synthèse finale](#résultats-finaux-du-lot) distingue validation logicielle et performance non démontrée. **G09 reste FAIL et aucun gate n'est clos.** Aucun worker, modèle ou service nouveau lancé, aucune ressource existante arrêtée ou nettoyée.

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

## S7 — mesure sur copie privée du graphe riche

**PASS du diagnostic**, base `f6f93b3` plus le probe `a9db5d10801144bafad2e24689b5a34d46b7499ece523ee531647f015ed8dde5`, sans correction applicative. La [revue avant exécution](s7-retained-before/preflight-review.md) couvre l'import borné de données de recherche. Luna sélectionne explicitement un journal retenu, copie uniquement ses cinq fichiers de contexte sans binding secret et ses31 révisions vers une nouvelle racine privée, puis utilise le vrai Journal/Writer et le vrai ChainVerifier. Aucune identité, autorisation ou action métier n'est restaurée ; aucun moteur, fournisseur ou gardien nouveau n'est lancé.

Le [log](s7-retained-before/01-retained-copy-cost.log), RC0 et [tableau numérique](s7-retained-before/cost-summary.tsv) portent cinq segments fermés :

| Segment | Temps mural caller, debug | Nombre d'appels File sync_all / Directory sync_all |
|---|---:|---:|
| Préparation de la copie, compteurs limités à la source | 5 905,02 ms | 0 / 0 |
| Premier load de la copie, head30 | 16 530,91 ms | 36 / 3 |
| Second load, même head | 17 696,93 ms | 36 / 3 |
| Status, même head | 16 921,05 ms | 36 / 3 |
| Validation de la chaîne depuis les bytes en mémoire | 13 571,29 ms | 0 / 0 |

**1 test exécuté,71,42s**,154 filtrés ; compilation séparée. Origine revalidée avant/après, zéro demande de création/publication/sync observée dans son collecteur ; l'ouverture source n'utilise pas Journal::open ni le verrou writer. Le verrouillage n'est pas instrumenté : ce dernier point est une propriété de source relue, pas une conclusion des compteurs. La préparation inclut les écritures de destination dans son temps caller, mais les compteurs ne couvrent que la source. La collecte mémoire ne voit aucune IO et vérifie réellement head/chaîne/payload ; son temps n'est pas du CPU et n'isole pas la seule sérialisation. Les durées inclusives se chevauchent ; ne pas les additionner ou les soustraire mécaniquement pour attribuer un coût.

Les [160 sources](s7-retained-before/SHA256SUMS) concordent avant/après ; Root vérifie aussi l'[archive extraite](s7-retained-before/root-archive-verify.txt), manifeste SHA `089639722e9db93b67c2154e148c118d10a94cb6f0e2925a125acdde8faa53c3`, archive privée SHA `afb448b645210ad6e13cb071de60c55893a4927572e5a478698a87040dfbb968`. Une première vérification Root trop large a refusé les fixtures publiques `tests/fixtures/` ; filtre corrigé, sans modifier l'archive. Archives, corpus, IDs et chemins source privés restent hors Git ; seule la [commande paramétrée](s7-retained-before/command-template.txt) est partagée. Darwin27.2.0 arm64/macOS27.2, FS exact **NOT_RECORDED**.

Cette mesure établit un coût important dans le traitement en mémoire de ce graphe, sans prouver la cause de H10, une latence release, un percentile ou une capacité. La [factorisation canonique proposée](checkpoint-canonical-design.md) retire un doublon local identifié en source ; sa validation et sa mesure après correction sont distinctes. Le probe n'est pas un import générique, une sauvegarde atomique ou un outil de reprise : les contre-exemples source malformée/symlink/hardlink ne sont pas exécutés ici. Les premiers niveaux de répertoires peuvent être ouverts pour leur inventaire avant refus, sans lecture des fichiers référencés. G09 reste FAIL, D5 ouvert et aucun gate clos.

## Correction canonique — premier contrôle

Les [trois tests avant correction](canonical-before/checkpoint-canonical-baselines.tsv) passent sur l'ancienne implémentation. Leurs empreintes sont figées avant le code : V2,1 183octets ; V3,922 ; V4,2 172. Les [logs et manifests](canonical-before/result-summary.txt) et l'[archive vérifiée par Root](canonical-before/root-archive-verify.txt) conservent cette provenance ; manifeste SHA `3a1708f1193b882e112f221bb75965b1db4f999002e3b4dfbf83d665883a5ac9`, archive privée SHA `714f501eee7dcd1196519e92c0eaab51e9b2a73a2d5986f8d838da8256d752e8`. Les cas négatifs portent sur erreurs, cardinalité et tailles ; l'entrée32MiB+1 teste la borne d'entrée, pas séparément la taille canonique de sortie.

La correction ajoute un helper privé dans chacun des checkpoints V2/V3/V4, qui restitue les bytes produits par la validation complète. Aucun champ, format, domaine de hash, API, cap ou ordre de refus ne change ; les caps de liste conservent leur court-circuit et tous les appelants de shape restent contrôlés. Aucun cache ni suppression de Full pré/post, CAS/floors, synchronisation, claim ou enforcement. La comparaison source indépendante est favorable ; la validation complète de ce lot reste ouverte.

**Premier contrôle :6 fonctions PASS,1 FAIL**, source phase2 gelée. Trois goldens, promotion du vrai journal, contexte/replay/caps lifecycle et conservation V4 passent une fois chacun. Le [septième test outbox](canonical-first-check/07-outbox-caps.log) échoue RC101 après11,15s : enqueue du lot positif512 → `Io` avant les autres assertions. Le nom « green » de sa capture privée n'est pas un statut. Les [résultats](canonical-first-check/result-summary.txt), commandes et160 sources avant/après sont conservés ; Root vérifie aussi l'[archive extraite](canonical-first-check/root-archive-verify.txt), manifeste SHA `74090ab4c5ae1dd000eb836d99fb51f40d967a0eddb0ef4616dc6e9b1a4507c3`, archive privée SHA `7a786f17f71e781824cb054cbaa530a7d3195e5a44d83c59673ed4ae66b48c03`. Le fichier des commandes a été ajouté après l'arrêt par transcription des arguments réels de l'outil, avec son PATH historique ; le reproducer séparé n'a pas été exécuté.

Le caller de la fixture est10s, mais timeout, oneshot fermé et erreur backend peuvent tous produire `Io`. Ce log ne tranche pas ; la factorisation ne peut être incriminée ou exonérée par cette seule durée. Un diagnostic Rust temporaire fermé est préparé à budgets et assertions identiques, avec conservation préalable de cette fixture entièrement synthétique. Une éventuelle relecture du journal après erreur sera datée comme observation ultérieure ; elle ne prouve ni le head au timeout ni la livraison de la réponse initiale. Tout marqueur sera retiré exactement après capture. **S7 après correction, fmt, Clippy et release : NOT_RUN** à ce premier arrêt ; aucun nouveau moteur/modèle/front/E2E lancé.

La [proposition source du prochain profil de lancement Docker](d5-launch-profile-design.md) reste non implémentée. Elle concerne un prérequis distinct de D5 et ne constitue ni témoin de fin normale ni preuve de fencing ; aucune ressource retenue n'est retirée sur cette base.

## Diagnostic à budgets inchangés

**FAIL reproduit, origine observée.** Une seule exécution instrumentée du même test, toujours avec caller10s, donne RC101 en20,01s après compilation4min04s. La [commande est écrite avant exécution](canonical-timeout-diagnostic/command-before-run.txt) ; les [marqueurs bruts](canonical-timeout-diagnostic/01-reproduction.log) ne contiennent que codes, nombres et booléens.

| Observation | Résultat |
|---|---|
| Branche de réception | `timeout` |
| Retour du caller | erreur après10,304650s, prévalidation comprise |
| Fin de commande backend | résultat réussi,512 reçus construits,14,120798s ; send non accepté par le oneshot |
| Relecture après erreur | succès, head1 et512 événements ; durée propre8,855521s |

Le diagnostic prouve cette branche de timeout et une réussite backend ultérieure. Il ne donne ni le head au timeout, ni une réponse lue par le caller, ni un ACK d'audit central. Les chronomètres ont des origines distinctes ; leur soustraction n'attribue pas un coût. La relecture peut attendre ou concurrencer des étapes du writer et possède ses propres synchronisations. Aucune cause temporelle de la factorisation ou de H10 n'est établie ; le cas512 n'avait pas de baseline pré-correction et ce diagnostic ne mesure pas la performance du code non instrumenté.

Les [160 sources](canonical-timeout-diagnostic/SHA256SUMS) concordent avant/après ; Root vérifie l'[archive extraite](canonical-timeout-diagnostic/root-archive-verify.txt), manifeste SHA `8bc17aadb12e14be06338118ec3294d7dfc4b2ab7c3a94d51956a0070bb56d9f`, archive privée SHA `6bf490e5aacbed6dd266267e8bcebf7d5e980078adef49049912495e4d59f606`. Les [deux](canonical-timeout-diagnostic/writer-diagnostic.patch) [deltas](canonical-timeout-diagnostic/test-diagnostic.patch) et leur [note](canonical-timeout-diagnostic/design.md) sont conservés comme preuves temporaires. Ils ont ensuite été retirés exactement : Writer retrouve SHA `47c72983851dc057c22d6731f55b99b0c208bbe8401d8333e00e582d217538d7`, test outbox retrouve d'abord son gel phase2 SHA `f9a34cfc7fbf3398bc81821af21860ce8958a0a36a4e2dc376c7faf1481ac7a5`. Aucun de ces marqueurs ne reste dans le code applicatif final.

La correction suivante porte seulement sur le harnais : constructeur privé avec attente paramétrée, défaut10s inchangé, et seul test fonctionnel512 porté à60s, plafond existant de Writer. Tous les oracles512/513, refus de contexte/secrets et absence d'écriture restent identiques. Aucun retry, attente artificielle, nouveau service ou délai de worker modifié. Ce choix permet de vérifier les bornes fonctionnelles en debug ; il ne qualifie pas une latence10s. Un canari existant de timeout/ownership sera exécuté avec les deux coutures outbox, puis S7 et les checks du nouveau gel. Leur statut est encore **NOT_RUN** à cette étape. Les deux échecs précédents restent visibles.

## Résultats finaux du lot

**PASS logiciel ciblé, neuf fonctions uniques après correction.** Les six fonctions du premier gel ont passé ; sur le dernier gel, le test fonctionnel512 passe en25,95s, la couture V4 est rejouée en10,01s, le canari `stop_append_unknown_retains_queued_identity_and_replays_once` passe en2,64s et S7 passe en71,35s. Cette couture V4 répétée n'est pas comptée deux fois. Le canari conserve ses10ms et vérifie réellement queue, résultat inconnu et replay ; le délai produit n'est pas relevé. Les trois baselines avant code, les deux FAIL et leur diagnostic restent dans les sections précédentes. Les [logs individuels](canonical-final/result-summary.txt), [commandes assainies](canonical-final/commands-sanitized.txt) et [revue indépendante finale](canonical-final/independent-review.md) conservent leurs périmètres.

`cargo fmt --all -- --check`, `cargo clippy --locked --all-targets -- -D warnings` et `cargo build --release --locked` terminent RC0 ; Clippy sans diagnostic, release11min52s. Les160 sources concordent avant/après ; Root vérifie le [checkout courant](canonical-final/root-current-checkout-verify.txt), l'[archive extraite](canonical-final/root-archive-verify.txt) et les deux copies binaires600 hors Git/cache. Manifeste source SHA `6467bd2c42262291164ddd66095ef2dea65f805b3fda86069ae3af9ac5960376`, archive privée SHA `49199ae33da6b80f014bd1bcdb43462142faddfbc4721354f5b41c194cc022f8`. Binaires : agentsandbox `879bffb4f1978fe0a069314cd1d89f392ccd5f4ac5e081e67d02a48bb480fcad`, ai-guard-provider `222e7e07ba94d2bc3e505fa839fb9bdbd808e24776c5b1d127500b709493c8a5`. Les enregistrements copiés, archives et binaires restent privés. Deux valeurs source sont remplacées dans la commande publique ; les arguments réels restent dans la capture privée.

Même probe et mêmes31 révisions sélectionnées, avant/après factorisation ; temps muraux caller en millisecondes, **debug uniquement** :

| Segment | Avant | Après |
|---|---:|---:|
| Préparation, compteurs limités à la source | 5 905,02 | 5 699,32 |
| Premier load | 16 530,91 | 16 952,07 |
| Second load | 17 696,93 | 16 901,40 |
| Status | 16 921,05 | 16 766,08 |
| Vérification en mémoire | 13 571,29 | 13 959,58 |

**Aucun gain de latence n'est démontré.** Les valeurs restent du même ordre et le temps du vérificateur augmente sur cet échantillon. Les compteurs lecture/sync restent identiques, notamment36 appels file-sync_all et3 directory-sync_all par load/status ; source et segment mémoire n'observent toujours aucune création/publication/sync. Le [relevé après](canonical-final/s7-cost-summary.tsv), nommé `.tsv`, utilise ici `|` comme séparateur ; les JSON bruts du [log](canonical-final/04-s7-retained-copy.log) font autorité. Les spans inclusifs ne sont pas additifs ; aucune isolation de charge, CPU consommé, performance release, percentile, capacité100 ou causalité H10 n'est prouvé. La suppression locale du doublon est établie par la source et les contrats ; le coût principal et le démarrage restent ouverts.

Deux erreurs documentaires sont aussi corrigées dans [BROKER_JOURNAL.md](../../../BROKER_JOURNAL.md) : le code n'emploie aucun domaine de hash propre à reservation/accepted ; il utilise le vrai submit_hash et les liens vérifiés. Aucun domaine fictif n'est ajouté au code. **NOT_RUN :** nouveaux Docker/Podman/Kubernetes, autres plateformes/FS, pannes runtime et reprise, matrice3/100workers, frontend/navigateur/fournisseurs/SSO. La conception du [profil de lancement suivant](../docker-launch-profile/README.md) reste un chantier distinct, sans témoin D5 ou autorisation de retirer les anciens slots. G09 FAIL et0/12 gates clos.
