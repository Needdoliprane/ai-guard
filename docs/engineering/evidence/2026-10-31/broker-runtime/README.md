# Broker permanent — essais Docker et diagnostics, 31 octobre 2026

**G09 FAIL ; aucun gate global fermé.** H4–H10 n'établissent ni Running worker ni faute au point prévu. H9 livre un refus préclaim codé ; H10 enregistre un échec local plus tôt sans événement public, dans un contexte d'inventaire perturbé. S3/S4 corrige cette visibilité :8 tests Rust et checks PASS, puis modèle frontend12/12 et build statique PASS au complément final. Les résultats des gels D3/PR4/S1/S2 plus bas restent historiques ; leurs checks logiciels ne qualifient pas les workers. Aucun fournisseur IA, SSO, cloud ou PKI utilisé. Navigateur/QA visuelle NOT_RUN ; les revues indépendantes ne qualifient pas le produit.

## H10 — échec local durable sans diagnostic public

Gel `6c7b63ebcef4a92cadfc2ec95626033f48011576`, mêmes159 sources et binaire S2 vérifiés avant/après ; aucun rebuild applicatif. **FAIL RC101,600,04s**, borne globale sans marqueur `broker.batch.failure`. Aucun worker Arm/claim ni Running, donc aucune faute SIGKILL au point prévu. [Sortie exacte](h10/h10.log), [rapport sélectionné du journal/SQLite](h10/core-diagnostic.md). L'émission du snapshot helper ajouté en S1 reste NOT_RUN : le worker n'atteint pas cette frontière.

La QA `/private/tmp/br-euAmXg` conserve12/12 opérations supervision ACKées, zéro pending, Worker Preparing rev3/quota retenu, SQLite14 événements/aucune allocation. Deux helpers Import/ImportVerification ont leurs observations terminales et cleanups ACKés. Le journal publie ownership rev29 pour le CID `431d660f00012b1abcd7c34604529dfe915914b91880f16b3e2a25f7c7da6e70`, avec observation Created nonRunning retenue par le programme ; aucun Created central, Arm ou claim worker. FailureObserved30 worker/unavailable suit, sans V4 ni diagnostic public. Une observation locale retenue n'est pas une nouvelle inspection indépendante. La source borne la fenêtre à la publication/retour du commit ownership, sa relecture, l'inspection ou la préparation de Created ; aucun code ne choisit sa cause exacte.

Le mtime de Failure30 précède d'environ370,722s la publication du FAIL global. Ces mtimes datent des publications, pas l'instant de l'erreur ou la durée d'un syscall. La boucle du harnais consulte le diagnostic public ; cette erreur locale sans événement lui reste donc invisible et Preparing n'est pas terminal. **L'absence de diagnostic ne signifie pas absence d'erreur.** Le correctif de visibilité S3/S4 ci-dessous est justifié indépendamment de la cause initiale.

L'inventaire passe de37 IDs valides/uniques à3,34 absents et aucun nouveau au postflight ; la même commande en lecture seule confirme3 IDs, RC0/stderr vide. Le mainteneur signale un nettoyage de cache concomitant, sans commande/cible précise. L'[engine ID préflight](h10/qa-preflight-identity-projection.json) correspond à l'[ID actuel](h10/docker-server-whitelist.json) ; cela ne démontre pas une epoch continue. [Résumé sélectionné](h10/inventory-summary.json) et [provenance des commandes](h10/docker-command-provenance.txt). Aucun ancien CID inspecté ni nettoyage demandé par l'opérateur des captures. L'application conserve ses cleanups des seuls helpers du test. L'intervention et les absences ne prouvent ni la cause du refus, ni un terminal/fencing, ni une continuité des anciennes ressources ; H10 n'est pas une reproduction isolée de sa cause.

### Suite S3/S4 — publier l'observation sans inventer la cause

La classification spécialisée reste partielle. Un rapport distinct utilise son résultat connu ou le repli fermé `broker_step=failure_observed`, phase connue, `runtime_effect=unknown`, champs natif/runtime absents. Le texte libre de l'erreur n'est jamais sérialisé. `Execution.failure` tente toujours la publication par masquage/outbox/HTTP/ACK existants, sans annoncer le succès de son append lifecycle si celui-ci est inconnu. Si cette publication échoue, le message reste opaque ; aucun effet, terminal, droit ou quota n'est déduit du rapport. Cela couvre seulement les erreurs qui parviennent au handler, pas panic/abort, ouverture refusée ou timeout externe.

Le type interne BrokerStep remplace PreclaimStep sans changer les anciens labels. Quatre contextes ferment les frontières déjà présentes du callback : `ownership_commit`, `ownership_reload`, `created_inspection`, `created_exchange`. Aucune IO/lecture d'horloge/attente supplémentaire sur succès ni changement d'ordre, de garde ou de durabilité ; l'IO de publication supplémentaire concerne les erreurs auparavant non classées. Un label d'échange ne prouve ni envoi ni ACK. Le parseur de test accepte ces cinq nouveaux labels, conserve ses UUID/seq/origines validées et ses huit champs, sans texte privé.

S3 a deux RED avec compilation réussie : la régression réelle traverse `Execution.failure`, confirme FailureObserved, ferme son gardien vide puis échoue sur l'absence de V4 ; le test pur de marqueur échoue sur le label non accepté. [RED journal](s3-red/01-unknown-failure-debug.log), [RED parseur](s3-red/02-parser-release.log), résumé d'oracles conservé. Aucun test moteur ni modèle n'est lancé. **S4 PASS :8 fonctions uniques**, trois diagnostics étendus, une publication réelle Gateway/Store/Journal/Writer et quatre parseurs. Le test réel vérifie même Event.id et audit_seq ACK dans l'outbox et l'API v1 viewer, canari absent et zéro slot/opération/helper/managed. [GREEN publication](s4/02-unknown-failure-debug.log), [GREEN diagnostics](s4/01-executor-diagnostics.log), [GREEN parseurs](s4/03-parser-release.log).

Fmt-check, Clippy strict all-targets et build release **RC0**, build0,95s ; compilation release du test parseur14min39s, ses quatre fonctions0,00s. Les trois cibles ciblées n'additionnent ni helpers ignorés ni replays historiques. [Revue backend finale](s4/independent-review.md), aucun P0/P1 restant identifié dans ce delta. [159 sources S4](s4/SHA256SUMS) vérifiées avant/après, puis checkout/archive extraite indépendamment par Root ; manifeste SHA`cc08455585a3c35978cff25c820b27ceda0093fb785f381a512c696a30771fd8`, archive privée`9569f60ace38e3f054658657c9a4d42ad165fcce3c663318e7091452e1a588f3`. Copies binaires mode600 hors cache vérifiées : agentsandbox`2ffdec56820e9ea4d3f528274f414c3a465c6284a25c1bb6ffac5a4935ee5f71`, provider`222e7e07ba94d2bc3e505fa839fb9bdbd808e24776c5b1d127500b709493c8a5`. Archives/binaires hors Git, aucune promesse de rebuild byte-identique.

### Frontend — un signalement lisible et sans effet confirmé

Le kind exact apparaît dans Actions et conserve sa place dans le journal. Son résumé et son détail indiquent « Échec signalé », phase/étape fermées, effet sur l'exécution non confirmé ; métadonnées inconnues/version future restent inconnues. Aucune conversion en terminal, incident de sécurité ou compteur. Les Maps privées évitent l'héritage de labels __proto__/toString ; les nouvelles chaînes passent par les helpers DOM textuels existants. Styles/thèmes/logo inchangés. Le filtrage du résumé n'est pas un masquage serveur universel : l'onglet Données brutes conserve l'événement reçu.

Trois [RED sur assertions fonctionnelles](frontend-red/frontend-red.log) : titre générique, absence Actions, titre des inconnus ; l'assertion canari n'est pas atteinte dans ce RED. Après correction, le [fichier modèle complet11/11](frontend-green/01-model-tests.log), l'installation offline15paquets et le [build TypeScript/Vite](frontend-green/03-npm-build.log) passent RC0, Node25.6.1/npm11.13.0, lock inchangé. Seul ce fichier de tests frontend est rejoué ; aucun navigateur/E2E ni nouvelle exécution moteur. [Revue frontend finale](frontend-green/independent-review.md), aucun P0/P1 restant identifié dans ces trois fichiers.

Aux sept fichiers sélectionnés par Luna s'ajoute le [manifeste complet des24 fichiers frontend versionnés](frontend-green/full-source-SHA256SUMS), vérifiés24/24 checkout et archive extraite Root. Manifeste SHA`10c7724d467bfbff590f0168d54c3836b0e3f1a57b9fd18ccd39c67c39845a9f`, archive privée`d3e519eb8cfad7d3910ed0a32bf905e2cc650b5a276d18f44c92c47d6fc0a11f`. Les quatre fichiers dist sont vérifiés au [manifeste](frontend-green/dist-SHA256SUMS), SHA`d0acb8e2ad2b28e725d63ac411f12c3335e4895bd14216efb5de79ecf78337a4`.

Le marqueur Bearer du bundle correspond au header Authorization existant avec variable token ; le scan Root des trois clés connues ne trouve aucune correspondance sur les quatre fichiers, sans garantie sur tout secret inconnu/encodé. Sources archivées et assets compilés restent hors Git. Node24, rendu navigateur, responsive, focus/clavier, contrastes/thèmes et nouvelle émission sur moteur : **NOT_RUN**.

#### Complément final — étapes natives

Le complément frontend final du 1er novembre conserve la preuve historique11/11 ci-dessus. Un [RED supplémentaire](frontend-final-red/frontend-red.log) reproduit l'étape native connue affichée inconnue. Les tables fermées reconnaissent ensuite les quatre étapes enforcer et six runtime ; priorité broker reconnue → enforcer reconnue → runtime reconnue, sinon inconnue, effet toujours non confirmé. Le même fichier modèle complet passe [12/12](frontend-final-green/01-model-tests.log), puis [build TypeScript/Vite RC0](frontend-final-green/02-npm-build.log), sans nouvelle installation ni test navigateur. Les boucles de cas ne sont pas comptées séparément. [Revue indépendante finale](frontend-final-green/independent-review.md), aucun P0/P1 identifié dans le delta ; branche DOM détaillée non exécutée par ces tests modèle.

Le [manifeste final des24 sources](frontend-final-green/full-source-SHA256SUMS), SHA`b5f9db8df84b328b4a239ebf462178c5d9bfb73c3a8bdcab4edb6e06e579ec09`, est vérifié checkout et archive extraite ; archive privée SHA`f5f5e2917950de1ac10124e334d9318182cbd8800593eed11a086a142d1e6a43`. Les quatre assets dist sont vérifiés contre leur [manifeste final](frontend-final-green/dist-SHA256SUMS), SHA`0f3a68bcc91151ad109d483bb13c40d094f09ef85eaee7c88f7551317c62576e`. Scan des trois clés connues : zéro correspondance sur ces quatre assets, sans garantie sur tout secret inconnu ou encodé. Node25.6.1/npm11.13.0 ; Node24, navigateur/visuel et raccord réel du dashboard **NOT_RUN**. Les sources Rust S4 et leurs binaires demeurent inchangés.

### D5 — preuves encore nécessaires

La [lecture indépendante du cycle des slots](d5-source/retirement-analysis.md) confirme que même les helpers nettoyés restent ciblés/restaurés et qu'aucune primitive de retrait n'est livrée. Le débit d'inspection n'est pas mesuré par cette lecture. Les [sources Moby du commit public correspondant au préfixe observé](d5-source/moby-source.md) fournissent des pistes conditionnelles, sans attestation du binaire ni qualification du fencing : requêtes tardives, nom égal à un ancien CID, restart et durabilité exigent des preuves séparées. La réalisation du contre-exemple de nom est NOT_RUN. Aucun gardien n'est retiré et aucune ressource effacée sur cette seule analyse. Avant un nouvel essai équivalent, traiter le coût du chemin durable et le cycle D5 ; ne pas ajouter des gardiens à l'aveugle. Les [limites du snapshot natif](h10/native-interpretation.md) restent explicites.

## H8/H9 — observation et premier refus codé

Application `af4a8682edf1bfcb18dcd08113e86f9724e9bcc1`, binaire `ec99e7817efc53ccb912d99bd5d5130c24bc89c9e7a59ef5d24cfb782338e9f6`, même image Docker Desktop ARM64 que H4–H7. H8 utilise encore la coupure préRunning120s/global240s : **FAIL RC101,122,63s**, sans FailureObserved, événement diagnostic ou WorkerClaim. Arm ACK33 apparaît environ118,303s après Submit, seulement2,234s avant le résultat. Le retour Writer/Register n'est pas établi ; ce n'est pas une preuve de refus applicatif. [Rapport Core](h8/core-diagnostic.md), [sortie](h8/h8.log).

Le harnais H9 supprime uniquement la coupure120s et consulte au plus une fois par seconde le premier `broker.batch.failure` via `events?kind=broker.batch.failure&limit=1&order=asc`. Une erreur durable fait échouer immédiatement le parcours nominal. Le marqueur garde huit métadonnées validées : UUID, seq positive, kind/phase/étapes fermées et effet inconnu ; ni corps, erreur libre, commande ou credential. La page vide ne prouve pas une absence globale. Un dernier GET est effectué avant de sortir de l'observation Running, toujours dans le budget global ; ses délais peuvent faire échouer l'oracle strict, sans lui donner de marge supplémentaire.

**H9 FAIL RC101,232,70s :** événement SQLite seq17 `ed0b84a8-7c4d-48a1-bdfe-13710373b3dc`, `worker / readiness_before_claim / before_claim`, raison native **`slot_phase_mismatch`**. `native_sample=null`, deadline d'origine347579793810375ns, Arm5s ; aucune expiration native démontrée. Register et l'ancienne relecture caller ont terminé avant ce refus.14 opérations supervision ACKées, aucun WorkerClaim/start. Naissance du même événement V4 rev35 durable, `audit_seq=null` local au moment de la coupure après le GET : ACK pending, pas une perte d'événement. Worker Created rev4, quota retenu, aucune allocation ou durée inventée. [Marqueur fermé](h9/h9-marker-eight-fields.json), [rapport Core](h9/core-diagnostic.md), [sortie](h9/03-h9-runtime.log), [revue du harnais](h9/harness-review.md).

| Essai | QA privée conservée | Nouveau CID Created/nonRunning |
|---|---|---|
| H8 | `/private/tmp/br-FV9BGW` | `800ec51a21e5cb8996a4f3de811fe447211408118e5c1bc5a561790c5d51381b` |
| H9 | `/private/tmp/br-BSL68s` | `44788e90678f0e2e614e0a96ae5fa5be12124e4fe5195123b975cbe761a06c4f` |

Les racines, gardiens non vides, CID et volumes restent conservés faute de disarm D5 qualifié. Zéro ID antérieur manquant dans les inventaires ; aucun ancien CID inspecté ou muté par H8/H9. Les seuls broker/gateway enfants du test sont arrêtés par son teardown ; aucun SIGKILL au point de faute prévu après Running. Les scalaires de ces deux CID ne donnent ni terminal ni fencing. Aucun service OS installé, appel modèle ou nettoyage de données existantes.

Les trois tests purs `batch_failure_marker_*` ont été écrits avant le lecteur : stub initial compilé puis **RED RC101** à l'assertion de présence du marqueur, puis trois **GREEN**. La revue du harnais a conduit à un GET final, un refus des diagnostics sans origine et un message nommant la faute prévue ; les assertions privées restent inchangées. Les replays après corrections ne sont pas additionnés. L'API n'est pas simulée : ces tests ne qualifient que le parseur ; H9 vérifie séparément son raccord HTTP réel. Les bornes de harnais240s de H9 sont historiques : la suite S1 passe à600s sur instruction du mainteneur, Arm worker5s inchangée.

## S1 — Draft acquitté et snapshot helper

Cette section conserve le bilan au gel S2, avant H10. H10 exerce ensuite le raccord Arm/claim pour ses deux helpers ; il ne qualifie ni le démarrage du worker, ni un gain temporel, ni l'émission du nouveau snapshot d'erreur.

Deux changements internes après H9 : réutiliser le candidat exact acquitté au head du Writer pour retirer la seule relecture Full caller après Register ; enrichir les seuls refus de prédicats avec le snapshot helper déjà lu. Le Draft est privé, lié à son opération/scope/head, opaque en Debug et sans capacité de start. Erreur/Unknown ne retourne aucune projection, conflit recharge et conserve Cancel/outbox ; ACK historique reste `fresh=false`. Ce head n'est pas annoncé latest. Une révision centrale devenue obsolète peut refuser conservativement ; les contrôles Ready/génération/clock, pending/ACK, EmitOnce, Full pré/post, CAS et durabilité restent appliqués. Aucun changement du Journal/Writer, format persistant ou API publique dans S1.

Le snapshot optionnel boxé a une source fermée `helper_slot` : phase, prepared_once, génération et clock validée de ce slot. Clock invalide omise avec flag ; absente inconnue. Il ne remplace jamais `native_sample`, qui reste absent lorsqu'aucun check local n'en a lu. Aucun nouvel échantillon, RPC, seuil ou travail externe sur succès. Son effet reste inconnu, sans preuve terminale/fencing ou libération de quota. Le producteur obsolète `load_after_registration` disparaît, le parseur conserve ce label pour les événements historiques.

Les tests Core utilisent de vrais Journal/Writer/filesystem, conflit CAS avec Cancel/outbox et vrai gateway/Store SQLite : Decision réellement POSTée, rejeu sans deuxième HTTP, binding propre à la fixture déplacé avant la mutation et refus avant HTTP/projection. Le constructeur de contexte est uniquement cfg(test) et vérifie réellement l'identité gateway active/schema3/runner/tenant/namespace ; il ne qualifie pas l'admission registry/FS. Un processus natif IPC vide est possédé, arrêté par ShutdownIfIdle et attendu ; il n'invoque aucun moteur et son helper enfant ignoré n'ajoute pas un test. Le replay prouvé est celui de Decision sur le port partagé, pas un claim ou start physique. Les tests de snapshot/prédicats n'établissent pas la transition temporelle réelle Ready→nonReady.

Luna a exécuté les cinq cibles S1 ci-dessous : **20 fonctions uniques PASS**, aucun test ignoré ou replay additionné. Fmt-check RC0. Clippy strict S1 échoue RC101 sur l'unique idiome `.err().expect()` du nouveau test Core ; aucun build release applicatif n'est alors lancé. S2 corrige uniquement cet idiome en `expect_err()` avec la même condition et le même message, sans source applicative modifiée. Les20 tests ne sont pas rejoués pour cette réécriture ; fmt/Clippy/build S2 **RC0**. [Sorties S1](s1/01-diagnostics.log), [deux tests Core](s1/03-executor-journal.log), [parseur](s1/05-parser.log), [FAIL Clippy conservé](s1/07-clippy.log), [delta S2](s2/s1-s2-journal-test.diff), [Clippy final](s2/02-clippy.log), [build final](s2/03-build-release.log).

```sh
cargo test --locked --lib diagnostics::tests -- --nocapture
cargo test --locked --lib broker::enforcer::client::stage_diagnostic_tests -- --nocapture
cargo test --locked --lib runner::broker::executor::journal::tests -- --nocapture --test-threads=1
cargo test --locked --lib broker::journal::tests::full_writer_rechecks_binding_after_real_publication_before_ack -- --exact --nocapture
cargo test --release --locked --test broker_runtime batch_failure_marker -- --nocapture
```

Le total20 correspond à12 diagnostics,2 prédicats natifs,2 tests Core,1 recontrôle postpublication et3 marqueurs. La compilation release de la cible parseur prend13min01s ; ses trois tests prennent0,00s. Elle ne compte ni comme essai moteur ni comme le `cargo build --release` final (RC0,0,54s). Les deux Core prennent72,05s. Aucun E2E global ou appel modèle n'est rejoué.

Les [159 sources S2](s2/SHA256SUMS) sont vérifiées avant/après, puis root vérifie indépendamment [checkout](s2/s2-checkout-check.txt) et [archive extraite](s2/s2-extracted-check.txt). Manifeste SHA`d5e9b6a60442a8588ff26660b1513890747f68c7ea9768564e246f1fff5679a9`, archive privée`9af03ffa0296ac4ab95aff5ef69bcd313ac2e0ba3dfbf0ce0886e9694abb599c`. [Copies binaires](s2/release-binaries.sha256) hors cache en mode600, vérifiées par Luna et root : agentsandbox`eff5eae403294b8dc62af4644bcb1c3f0a6ddead704cbe6f82c2ce13f32d1769`, provider`f3c0380c12dbc27d32593d297bcdb4318cbea642b7f5abd41d8956d8ae99eb09`. Originaux privés : `/private/tmp/ai-guard-s1-20261031-220416` et `/private/tmp/ai-guard-s2-20261031-222447`. Archives/binaires restent hors Git ; aucune promesse de reconstruction byte-identique.

La [revue indépendante finale](s2/independent-review.md) recoupe sources, delta S2, logs,159 sources et hashes des binaires ; aucun P0/P1 identifié dans son périmètre. Elle distingue le replay Decision, le gardien IPC vide, les prédicats logiciels et le runtime non qualifié. Les copies de données ont un coût ; aucun gain de durée mesuré, aucun correctif causal de H9, aucune émission du nouveau snapshot sur moteur ne sont revendiqués. Au gel S2, le prochain critère est H10 global600s/worker5s. Aucun moteur supplémentaire n'est lancé pendant cette validation logicielle.

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

Les essais réels Docker utilisent la sélection suivante, **propre à cet hôte**, et conservent des ressources ; ce n'est pas une commande de quick start ordinaire :

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

Prochain critère courant : mesurer le coût du chemin durable et traiter le cycle des slots/D5 avant un nouvel essai équivalent. Ne pas ajouter de gardiens ou prolonger encore la borne pour contourner l'absence de diagnostic. Running worker, panne SIGKILL, deadline d'origine, reprise idempotente et cycle terminal/disarm restent requis. Les captures H4–H10 sont conservées ; la continuité des ressources moteur anciennes n'est plus certifiée après H10. SSO/Keycloak/PKCE reste obligatoire mais absent, qualification NOT_RUN ; [contrat et documentation](../../../SSO_OIDC.md). Le rappel Daybreak Blue attend ces parcours réels stabilisés.
