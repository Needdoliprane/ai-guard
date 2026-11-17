# Stratégie de qualification

## 17 novembre — sélection D7 et portée des sources

Le [relevé D7 EN/FR](evidence/2026-11-17/d7-native-stop-owners/README.fr.md) et son JSON donnent 16 FQN lib réellement réussis sur le gel `6c5554cd…` / commit `6a32771`, après reproduction du défaut de révision vide sur R4. Les 15 cas alors non exécutés ont été lancés sur R5 avec le cas corrigé ; les quatre PASS R4, dont l’ancien V1 explicitement invoqué, restent attachés à R4. Check toutes cibles, build libtest et Clippy toutes cibles RC0 non strict vérifiés ; 561 émissions d’avertissement, gate stricte ouverte. Les listes contiennent 1 772 tests **dont** 24 ignorés ; ces 24 ne s’ajoutent pas au total et aucun n’est compté PASS par défaut.

Le gel privé Native17 R8 `a44ad948…`, distinct du public et de D7, passe quatre régressions de transport Root mais conserve deux FAIL natifs d’époque sur de nouvelles fixtures : heartbeat à `grant_apply`, OIDC à `TransactionMembershipAppend`. Ni leur issue durable ni une performance suffisante ne sont déduites. Les essais moteur, reprise froide, restauration et capacité restent à qualifier ; une fixture de métadonnées ne démontre pas un STOP de worker.

## 17 novembre — relevé runtime et finalisation tardive D1

[Douze cas logiciels ciblés](evidence/2026-11-17/runtime-observation-and-d1-late-finalizer/README.fr.md) passent sur le gel `c755fb52…db3b9`, dont quatre vrais CLI/Gateway/SQLite avec moteur Rust de fixture : terminal unique, faute Managed tardive et quota/instance conservés pour Created/Unknown. Le relevé facultatif est borné, sans attente dans STOP, et ne remplace pas l’audit durable. Le précédent FAIL de sonde reste historique ; la régression lit désormais la capture dans son véritable propriétaire d’erreur. Compilation des trois tests et six listes vérifiées ; Clippy toutes cibles de cette révision RC0 non strict, 528 émissions d’avertissement, 0 erreur ; 21 émissions supplémentaires restent suivies. Ni moteur réel, reprise froide, fournisseur, scénario A1/A2/B1 complet ni capacité 100 ne sont qualifiés. **0/12 gates clos**.

## 16 novembre — D1 : séparer preuve CLI, récupération Managed et TTY

La [sélection D1](evidence/2026-11-16/control-monitor-prestart-fix/README.md) compte **14 PASS réels** : quatre invocations du binaire ai-guard `agentsandbox` avec Gateway/SQLite et moteur Rust de fixture, deux unités Managed/PTY, trois PTY et cinq deadline. Chaque FQN est invoqué une fois ; sources et trois exécutables sont revérifiés. Ni Codex CLI ni Claude Code fournisseur ne sont impliqués. Les deux fichiers D7 cfg(test) de la source commune sont hors commit D1 et leurs cas ne contribuent pas aux quatorze PASS.

Conserver canari puis RED baseline, le refus préstart corrigé avant tout start, la faute au dernier contrôle d’autorité et le contraste après effet avec terminal réellement connu. L’unité de fallback vérifie le vrai producteur Managed et sa cause ; elle ne remplace pas la faute exacte à travers runner/Store/quota. Les voisins PTY/deadline ne qualifient pas une session TTY physique ni le délai d’arrêt d’un moteur réel. Abandon, reprise froide et autres plateformes restent ouverts. Clippy D1 toutes cibles sur la source composée `74d60de1…b7b` : **RC0 non strict, 507 émissions d’avertissement et 0 erreur**. Reçu `dc43ad33…0e0a2` ; gate strict ouvert.

Une source commune D1/D7 économise un build ; elle ne fusionne ni qualification ni ordre des résultats. Le premier canari D7 échoue au setup TMPDIR/UDS avant l’oracle : ce résultat non concluant reste conservé. La phase R2 corrige seulement cette configuration de harnais, vérifie cap2 PASS RC0 puis atteint le vrai oracle cap1 ACK sans STOP/Cancel, RED RC101. Le troisième parent reste NOT_RUN et aucun correctif produit D7 n’est intégré. Aucun ancien PASS, helper ignoré ou résultat Native17 privé n’est transféré à ce lot.

## 16 novembre — R2 : sélection logicielle de composition et reprise tracée

Les **26 sélections R2** sont exécutées en série, une fois chacune : 18 runtime, quatre Kubernetes et quatre intégrations `runtime_managed`. Elles passent sur les deux nouveaux artefacts produits depuis la même source R2 ; listes actives/ignorées, sorties/RC et sources/binaires sont vérifiés par Root. Les helpers ignorés ne sont pas comptés. [FQN, commandes réelles, profils, empreintes et limites](evidence/2026-11-16/runtime-kubernetes-batch-composition/README.md).

Conserver la première tentative de build R2 **RC130 pour ressources**, puis sa reprise explicitement admise avec le seul changement d'orchestration `-j2` vers `-j1` ; code, profil, assertions, deadlines et epochs restent identiques. Aucun cas n'est relancé automatiquement. Clippy toutes cibles RC0 avec **505 diagnostics d'avertissement** n'est pas un gate strict. R1 garde ses 26 PASS puis son Clippy FAIL ; les 18+4 standalone restent deux autres preuves.

La sélection ne qualifie ni un moteur réel ni la durée physique d'arrêt. MetadataFD demeure un essai distinct en échec (phase append de 2 331 ms, COMMIT marqué, `full_post` incomplet, quatre dépendants NOT_RUN) ; aucun gain n'en découle. Native17, les variantes interactive/préarmée, l'annulation externe, le vrai caller A06 et la faute OS tardive complète demandent leurs preuves propres.

## 16 novembre — R6 : conserver le résultat du positif configuré

Le test exact R6 est unique et actif dans la liste réelle ; exécuté une fois, il échoue au démarrage. Les autres cas ne sont pas crédités, et les résultats R11 restent séparés. Ne pas déduire la rétention durable du marqueur COMMIT. [Sélection, profil et limites](evidence/2026-11-16/startup-runtime-failures/README.md#native17-r6-one-configured-startup-oracle-remains-failing).


## 16 novembre — prérequis observés et profils distincts

Le [plan R11](evidence/2026-11-16/startup-runtime-failures/README.md#successors-r10-and-r11-software-results-remain-separate) exécute les cas DATA/FS, writer et Owner indépendants, puis conserve six dépendants NOT_RUN après le refus du positif configuré. Le timer inclut un oracle cfg(test) ; une mesure séparée doit distinguer corps et observation. Le nouveau test foreign-writer n’est pas exécuté sans ce prérequis. Aucun changement de 2 s/profil ne transforme le FAIL historique en succès.

## 16 novembre — admission TCP et lecture des en-têtes

La [sélection HTTP](evidence/2026-11-16/http-listener-bounds/TESTS.json) réunit six tests d’intégration, dont cinq cas TCP loopback, une unité de détention du permis et quatre canaris gateway de retrait/drainage réels. Onze fonctions passent une fois chacune ; aucun flood, épuisement de descripteurs, campagne moteur ou suite frontend inutile. Autres plateformes, streams HTTP/2 suivants, budgets globaux et Native17 demandent leurs preuves distinctes.

## 16 novembre — conserver les oracles en échec

Les [preuves R9/Docker](evidence/2026-11-16/startup-runtime-failures/README.md) restent attachées à leurs deux sources distinctes. Le contrôle startup garde son époque de deux secondes ; le test moteur conserve timeout1s/sleep60s et son garde global30s. Avant nouvel essai moteur, corriger et vérifier hors ligne la supervision des observations/callbacks, puis distinguer armement, envoi d’arrêt et terminal observé. Aucun agrégat avec les doublons R8 ni relance systématique.

## 16 novembre — corrélation du callback CLI

La [sélection NV-09](evidence/2026-11-16/oidc-callback-correlation/README.md) exécute dix unités du vrai listener et quatre intégrations Rust/CLI loopback, sans IdP externe. Les quatre cas sélectionnés ne constituent pas une réussite des 25 tests enregistrés de `sso_cli`, ni du SSO complet.

## 16 novembre — régression de la préparation des images

La cible `runtime_security` couvre la politique `--pull=never` et la séparation des arguments pour Docker/Podman, via les builders réels. [Preuve et limites](evidence/2026-11-16/runtime-local-image-policy/README.md). La qualification d’une image absente et le confinement demandent toujours les essais moteurs distincts.

## 16 novembre — lot logiciel Kubernetes dédié intégré

Le [lot lifecycle/attach isolé](evidence/2026-11-16/kubernetes-dedicated-integration/README.md) passe son check toutes cibles et son build libtest, puis **17/17 tests logiciels exacts** sur `1d274954`. Après correction du formatage seul (`cb1ca7f`), le contrôle de format des 18 fichiers Rust et un nouveau check toutes cibles passent ; les 17 tests ne sont pas réexécutés. La revue indépendante ne signale aucun blocage confirmé dans ce périmètre pré-effet. Seuls les 21 chemins dédiés sont repris, avec la garde canonique de replay conservée. Native17/S5, le vrai cluster et la qualification complète restent ouverts : **0/12 gates clos**.

## 16 novembre — tests natifs exécutés et raccords en correction

Le [build natif macOS R16](evidence/2026-11-16/session-control/TARGETED_NATIVE_BUILD_R14_R16.md#native-macos-r16-successor--actually-executed) réussit, puis les 12 tests ciblés donnent **9 diagnostics PASS et 3 sauvegardes FAIL**, sans timeout ni relance. Les trois refus surviennent avant l’oracle de sauvegarde : `human_authority_epoch_expired` au démarrage ; leur cause reste à corriger ou expliquer. La composition suivante Config/E2, revue puis appliquée localement, échoue à son check toutes cibles avec **40 erreurs de compilation** ; ses 55 nouveaux tests sont NOT_RUN. Les preuves restent liées à chaque source. **0/12 gates clos ; aucun pourcentage fonctionnel fiable.**

## 16 novembre — passer de la compilation aux comportements natifs

Après la [compilation R13](evidence/2026-11-15/session-control/COMPOSED_FULL_NATIVE_CHECK_R9.md#r11r13-successor-checks-2026-11-1516) toutes cibles réussie, la validation suivante cible 3 vrais tests de sauvegarde/reprise et 9 de fermeture/dette des diagnostics, avec construction libtest, liste et noms exacts contrôlés avant exécution. Ils restent **NOT_RUN** ; ni compilation, ni liste, ni oracle paramétré non appelé ne vaut PASS. Aucun appel modèle ni essai moteur/cluster n’est inclus.

## 15 novembre — compilation du successeur natif R10

Le [successeur corrigé R10](evidence/2026-11-15/session-control/COMPOSED_FULL_NATIVE_CHECK_R9.md#r10-successor-check-2026-11-15), 3 307 fichiers, a subi un nouveau check toutes cibles : **FAIL, RC 101, 7 erreurs codées**, 2 notes de résumé sans code et 60 avertissements. Une seule invocation Cargo a été exécutée ; aucun test. Collecte, gardes, source/cache et fermeture du CID exact sont vérifiées, sans hausse des compteurs OOM.

Le check `cargo check --all-targets --frozen --offline --message-format=json -j1` ne remplace aucun test natif : aucune liste ou exécution libtest n’est créditée ici. Les sept erreurs et les deux notes de résumé restent distinctes. Après correction et nouveau gel, compiler puis choisir les régressions réelles pertinentes sous admission séparée ; les anciens PASS ne se transfèrent pas. Le programme, la collecte et la fermeture vérifiés ne qualifient aucun comportement de Store, broker, runtime ou fournisseur.

## 15 novembre — compilation du noyau natif complet R9

Le [gel complet réellement appliqué R9](evidence/2026-11-15/session-control/COMPOSED_FULL_NATIVE_CHECK_R9.md), 3 291 fichiers, a été compilé une fois : **FAIL, RC 101, 64 erreurs codées**, 2 résumés distincts et 62 avertissements. Il expose des dépendances et raccords encore incomplets, sur un périmètre plus large que R3. Collecte, source/cache et fermeture sont vérifiées, sans OOM observée. Aucun test exécuté par cette campagne.

Trois lots corrigent séparément configuration, runtime/conversation et audit. Le correctif relu des trois agrégations d’authentification est appliqué après le gel R9, avec échéance inchangée et propriétaires retenus ; sa compilation et ses quatre régressions restent NOT_RUN. Les parcours natifs, OIDC, reprise réseau et configuration restent à terminer et qualifier. **0/12 gates production clos** ; pas de pourcentage fonctionnel fiable.

## 15 novembre — preuves ciblées et intégration native en cours

Le [frontend natif de nouvelle autorisation](evidence/2026-11-15/session-control/FRONTEND_NATIVE17_QA_R1.md) a réellement passé 7 nouvelles régressions et 46 canaris : **53 PASS, 0 FAIL**, build TypeScript/Vite PASS. Navigateur et parcours gateway/broker/fournisseur restent NOT_RUN pour ce delta.

Deux contrôles Rust distincts restent **FAIL** : le [baseline réparé R2](evidence/2026-11-15/session-control/COMPOSED_BASELINE_CHECK_R2.md), avec 4 erreurs codées, et le [candidat courant R3](evidence/2026-11-15/session-control/COMPOSED_LIVE_CHECK_R3.md), avec 11 erreurs de raccordement sur un périmètre plus large. Les correctifs de tailles/financement et d’API sont relus et appliqués après ce dernier gel, sans crédit de recompilation. Collecte, source/cache et fermeture des QA sont vérifiées ; aucune OOM observée lors de ces deux checks.

Le [lot Kubernetes lifecycle/attach](evidence/2026-11-15/session-control/KUBERNETES_LIFECYCLE_QA_R4_R5.md) a réellement passé son check toutes cibles et 16 de ses 17 cas logiciels sur le premier gel ; le cas restant échouait dans sa fixture. Après correction de cette seule fixture, le retest ciblé du successeur passe **2/2**, dont un canari inter-tenant. Les 16 PASS historiques ne sont pas transférés comme une suite réexécutée. Formatage et Clippy restent NOT_RUN, ainsi que tout essai cluster. Le noyau natif, le contrôle réseau et les parcours de session restent à composer puis vérifier. La révocation du sender après autorisation avant émission reste un écart explicite du contrat. **0/12 gates production clos** ; aucun pourcentage fonctionnel fiable n’est établi. Ce commit documentaire ne qualifie ni les candidats privés ni la branche publiée.

## 14 novembre — sélection inspection composée : API, CLI historique et FS

Sur `45179db`, une seule no-run offline/frozen avec lib, bin agentsandbox et
intégration kubernetes_waiting_cli fournit cinq exécutables frais liés au JSON
Cargo. Un check toutes cibles est suivi de 24 listes réelles : global/default et
ignored sur trois harnesses, puis exact/ignored des neuf groupes. Le default
`--list` comprend aussi les ignorés : corréler les ensembles réels. Les 47 FQN
uniques sont 27 libtest, 13 bin-test et sept intégrations ; 44 actifs et trois
CLI ignorés par défaut, explicitement exécutés avec le normal CLI de même build
et son SHA. Aucun helper hors sélection n'est compté comme un PASS.

Les neuf groupes passent, sans addition avec les campagnes antérieures :
API3, CLI ignorés3, Core9, CLI13, E1compat2, A1auth3, FS3, Kube4 et stdio7.
Les noms et argv exacts restent dans
[SELECTIONS.json](evidence/2026-11-14/owned-policy-inspection/SELECTIONS.json).
Exiger le nom réellement exécuté et le résumé exact, pas seulement RC0.
Conserver chaque groupe partiel : R2 avait huit PASS dans son groupe Core en
échec, bien que son compteur des tests PASS issus des deux groupes entièrement réussis restât 6.
R1 compilation101 et R2 faux positif d'oracle restent des échecs datés, pas des
réussites reconstruites. Le correctif R3 vérifie les clés JSON, sans mutation produit.

Les tests traversent le vrai HTTP/SDK/SQLite et le CLI Rust interne : scope
observé, rapport historique, préparation/durabilité, CAS final, conflits sans
réparation, reprise et contrôles d'autorité/FS. Les nouveaux contrôles de parent
physique restent distincts d'une qualification case-insensitive : l'oracle
report/REPORT est NOT_APPLICABLE sur le filesystem Linux de cette campagne.
Ne pas transformer cette branche non exercée en preuve macOS.

Les quatre contrats Kube sont un HTTPS/TLS loopback et trois duplex mémoire ;
les sept stdio exécutent le vrai provider Rust interne avec pipes, sans client
Codex/Claude, modèle ou worker. Préserver source/modes, cinq SHA, sept collectes,
OOM0, source intacte, absence du CID exact et flush séparé. Les fichiers par phase
et l'extrait terminal borné sont conservés ; la transcription brute complète
n'est pas persistée ni reconstruite. Ces tests ne qualifient ni runtime/réseau
produit, E2/flow16, fédération Claude, capacité100 ou production ; 0/12 gates fermés.
[Preuves EN](evidence/2026-11-14/owned-policy-inspection/README.md) /
[FR](evidence/2026-11-14/owned-policy-inspection/README.fr.md).

## 14 novembre — Kubernetes attach : 4 contrats et 7 processus stdio

Le gel `1e30e5e8546ab614eba7849789a1a0468cbc2843` est compilé une fois
avec libtest et `--test kubernetes_waiting_cli`, puis vérifié all-targets,
offline/frozen et `-j1`. Les vrais messages Cargo identifient trois
artefacts : libtest, intégration stdio et binaire Rust normal
`ai-guard-provider` (`profile_test:false`). Quatorze listes réelles
précèdent les cinq groupes : quatre parents de contrat chacun en exact et
sept noms stdio exacts ensemble. Total réel : 11 PASS uniques, zéro FAIL,
zéro sélectionné ignoré ; les 121 PASS historiques be6 ne sont pas rejoués.
[FQN et argv](evidence/2026-11-14/kubernetes-attach-composition/SELECTIONS.json).

Les contrats utilisent les vrais owner/codec/lecteur avec peers synthétiques.
Le contrat 1 exerce une requête d'upgrade HTTPS vers un peer TLS loopback ;
les contrats 2/3 emploient `tokio::io::duplex(64)` et le contrat 4
play→open→duplex mémoire. Les trois derniers ne prouvent aucun TCP/TLS.
Ils couvrent le travail initié et son slot, le pong bloqué après Ready,
l'annulation gardée avec le délai d'IO et les closes invalides, sans serveur
d'API Kubernetes. L'addendum de couverture est lié aux reçus des preuves.
Les sept intégrations
lancent réellement le binaire normal, avec environnement nettoyé, répertoire
privé neuf et pipes bornés. Exiger sa fin terminale observée, stdout/stderr
bornés et kill/reap sur erreur ; le cas préfixe retenu doit observer 65 alors
que le writer stdin reste ouvert, sans fabriquer EOF à l'échéance.

Le test Hello vérifie exactement un Ready déclaré corrélé puis sortie 66 ;
ce n'est pas une identité ou une exécution métier. Les refus d'arguments
supplémentaires, UTF-8 invalide et troncature gardent 64/126. Le code interne
du provider est exécuté pour attente/arguments, aucun client ou modèle IA.
La bibliothèque WebSocket épinglée, le confinement et le protocole ne gagnent
aucune qualification cluster par le seul passage des fixtures.

Conserver les 2805 contenus/tailles/modes, les trois SHA Cargo, les 56 RC
d'étapes, exports bornés, fermeture exacte, OOM 0 et flush FD séparé. Ne pas
exécuter les opaques sur l'hôte. Les [preuves EN](evidence/2026-11-14/kubernetes-attach-composition/README.md) /
[FR](evidence/2026-11-14/kubernetes-attach-composition/README.fr.md)
restent propres à `1e30e5e`. La campagne inspection `45179db`, décrite
ci-dessus, répète séparément ces onze contrôles parmi ses 47 parents, sans
rejouer les 121 historiques ni additionner ces campagnes. Pas de relance automatique des 121
parents historiques, de toute la suite ou de modèle : choisir les nouvelles
régressions utiles, sans transformer l'économie de tests en qualification.
S5/permis/fencing/R4/R7/R10, runtime/réseau/cluster réels, capacité 100,
Clippy strict et gates production restent ouverts ; activation fermée.

## 14 novembre — A1 : sélection API/CLI du registre egress

Le gel `db2678d42cff9c8d6d15bf31284a989732cc6f0d` possède une compilation
réelle des libtests et du CLI normal, puis un check toutes cibles RC0. Son
premier harnais échoue avant les listes ; la reprise tests-only importe les
trois opaques byte-exacts, sans rebuild. Exiger les listes réelles des bons
artefacts, les FQN exacts et leurs résultats :18 listes, huit groupes,
74 parents uniques PASS (73 ordinaires, un default-ignored exécuté
explicitement), aucun FAIL. Ne pas additionner les baselines ou le build
aux PASS, ni déplacer cette preuve sur le gel suivant.

Les deux libtests et le CLI normal sont résolus depuis les messages Cargo
JSON. Les sélections couvrent contrats scope/CAS et reçus, vrai SDK/HTTP et
Store15, refus avant corps, source/binding/génération, contention/timeout713,
rollback SQLite-FULL logique, abandon du seul observateur, maintien des
permits et drain, façade FS durable et parsing du CLI. Le parent vertical
ignoré par défaut est exécuté une fois avec le vrai CLI normal et ses pipes :
commit dont le reçu est perdu, mêmes UUID/octets à la reprise, reçu durable
puis révocation demandée. Ce n'est pas un scénario runtime réseau.

Conserver les ACK qui prouvent la réception du rendez-vous, les vraies
observations plutôt qu'un délai attendu, les refus sans réparation des
fichiers et les barrières file/dir/parent. Une faute injectée après un vrai
sync n'est pas un errno fsync observé ; SQLite-FULL ne remplit pas le disque
physique. Les fsync des preuves ne qualifient ni coupure électrique ni reprise
de machine. Les FQN/argv exacts et huit groupes se trouvent dans
[SELECTIONS.json](evidence/2026-11-14/owned-egress-policy-api-cli/SELECTIONS.json).

Les commandes réellement utilisées en QA sont les builds offline/frozen
lib+bin en no-run, le build du CLI normal et le check all-targets consignés
dans les [preuves EN](evidence/2026-11-14/owned-egress-policy-api-cli/README.md) /
[FR](evidence/2026-11-14/owned-egress-policy-api-cli/README.fr.md).
La reprise n'en répète aucune. Aucun nouveau harnais n'est requis par cette
documentation. Sur cette preuve A1, l'acquisition scope/CAS restait ouverte
et les cinq parents ultérieurs `05a4451` étaient NOT_RUN. Le parcours
`inspect` puis `prepare --inspection-dir` est vérifié séparément sur `45179db`
par la sélection inspection ci-dessus ; ne pas lui transférer les 74 PASS. Runtime/réseau/worker,
fournisseurs/SSO, cluster, charge100, suite complète, Clippy strict et tous
les gates production restent hors qualification ; broker/relais restent fermés.

## 13 novembre — A0 : sélection ciblée exécutée sur `0114013`

GREEN R3 compile et passe le check toutes cibles à 0 (157 warnings de compilation), vérifie 20 listes puis exécute les 37 FQN uniques en neuf groupes : 34 parents lib (SDK 14/15, refus, abandon/drain, DTO et canaris metadata) et trois `control_protocol_schema`, tous PASS, zéro FAIL/ignoré. Les noms et résultats réels par groupe figurent dans [SELECTIONS.json](evidence/2026-11-13/owned-control-diagnostic/SELECTIONS.json). Conserver séparément les deux vrais oracles avant correction sur a47 et son harnais 127, le build 0 suivi du harnais 88 de GREEN R1, puis les 14 PASS / 1 fixture FAIL / 22 NOT_RUN de GREEN R2 sur `452d` ; aucune addition de parents recouvrants.

Les commandes ciblées sont `cargo test --frozen --offline --lib --test control_protocol_schema --no-run --message-format=json` et `cargo check --frozen --offline --all-targets`. Les positifs traversent le vrai HTTP/SDK ; la fixture réparée prouve le Worker réservé et le binding réel, pas un relais Running. Le canari Router couvre un abandon logiciel, pas une déconnexion TCP. SQLite FULL est une limite logique de la base de fixture, pas un disque physique rempli. Les sept collectes, source/modes, artefacts opaques, RC et fermeture exacts sont conservés ; la garde ne donne pas une qualification réseau produit. Les 17 stdin/idempotences du formatter `6c62ad0` → `0114013` restent une preuve distincte de formatage. Pas de délai/epoch produit élargi ni de gate broker 3–13/relais 6–13 ouvert. Suite complète, Clippy strict, fournisseurs, moteurs réels, cluster, pannes physiques et capacité 100 restent NOT_RUN ; aucun gate production clos. [EN](evidence/2026-11-13/owned-control-diagnostic/README.md) · [FR](evidence/2026-11-13/owned-control-diagnostic/README.fr.md).


## 13 novembre — contrat de test des lectures owned de métadonnées

Les sélections suivent les sources réellement exécutées : Rust `195b437`, puis 14 réparations sur `858fc7d` ; Node `e25b631`, puis un seul fichier real-wire sur `195b437` ; enfin build frontend `810711b` et navigateur composé backend `858fc7d` / dist `810711b`. Les noms exacts et empreintes figurent dans [SELECTIONS.json](evidence/2026-11-13/human-owned-metadata/SELECTIONS.json). Ne pas compter deux fois les fonctions répétées ni additionner les scénarios navigateur aux fonctions de test. Un ancien agrégat FAIL ou un test ignoré reste inchangé.

Avant une régression, compiler une fois le vrai libtest dans la QA isolée admise et vérifier sa sortie Cargo JSON ainsi que ses listes réelles. Les fixtures de ressources créent les événements avant leurs grants. Les erreurs de persistance sont de vraies allocations SQLite-FULL bornées : absence de DTO, rollback des changements de tête/quota/usage, `busy_timeout` restauré à 713 ms et état indisponible conservé après restitution de capacité. Aucun trigger inconnu ni erreur de harnais ne remplace cet oracle. Le canari de concurrence retient le vrai read gate sans réutiliser une ancienne epoch au-delà de sa durée ; aucun délai produit n’est élargi pour le test.

La régression frontend utilise quatre réponses réellement sérialisées par Rust ; l’absence de captures impose un test explicitement ignoré. La fixture navigateur utilise le vrai gateway/dashboard et SQLite dans une QA isolée, avec données synthétiques et aucune requête fournisseur/worker. Exiger la connaissance positive du profil `snapshot_only` avant d’observer l’absence de polling/SSE/boucle de relance, l’actualisation explicite, la provenance de chaque reçu, le vrai verrou SQLite avec refus 503 puis reprise 200, le maintien du focus et le retrait des données. Les quatre captures et huit mesures de contraste calculées depuis les styles normaux/au survol complètent les assertions fonctionnelles ; elles ne constituent pas un audit général d’accessibilité.

Conserver les collectes bornées, sources/modes, preuve de garde dans le même CID, absence d’OOM, arrêts effectifs et absence du CID exact. Les fsync des preuves ne qualifient ni restauration ni disque plein physique. Aucun modèle, moteur ou IdP n’est relancé par réflexe ; les campagnes production obligatoires restent des gates distincts ouverts. [Preuves EN](evidence/2026-11-13/human-owned-metadata/README.md) · [FR](evidence/2026-11-13/human-owned-metadata/README.fr.md).

## Diagnostic opérateur E — ciblage du 12 novembre

La validation du diagnostic distingue le miroir de configuration du binaire et le parcours CLI réel sur des fichiers synthétiques. Construire lib, `--bin agentsandbox` et `--test diagnose_cli`, résoudre leurs vrais artefacts et listes, puis exécuter les neuf sélecteurs `gateway_config::diagnostics_parity_tests::…` dans le libtest du **binaire**, chacun en `--exact`. La parité inclut la politique de rétention du relais (absente/null/valide/refusée), les enveloppes et validateurs OIDC, les références/AC et la règle d’authentification. Le contenu des tokens et stores reste hors du périmètre passif restitué.

La suite normale `diagnose_cli --nocapture --test-threads=1` couvre fichiers/dispositions et refus, absence de mutation/réseau par défaut, masquage et bornes, JSON/catalogue, choix fournisseur sans repli d’abonnement vers API, découverte OIDC explicitement demandée sur loopback et cohérence des guides/exemples. Exiger les identités réelles et leurs oracles, pas seulement le RC : au gel final, 85 PASS, zéro FAIL/filtré, un exporteur manuel nommé ignoré et `NOT_RUN`. Les neuf parités et le check toutes cibles passent aussi. [Gel, commandes et résultats](evidence/2026-11-12/operator-diagnostics/README.md).

La régénération d’exemples est une opération distincte, explicitement admise : l’exporteur ignoré écrit dans un nouveau TMPDIR privé, jamais dans les sources ; Root vérifie les 19 membres capturés et leurs contenus avant de promouvoir les neuf normalisés. Une nouvelle composition doit ensuite passer `first_run_states`, `fresh_historical_directory_is_explained`, `documented_examples_are_the_real_output` et `guides_follow_the_binary`. Ils passent sur le gel final du 12 novembre ; leurs anciens `NOT_RUN`, comme le `FAIL_HARNESS` r1 de tableau vide Bash 3.2, restent conservés. Pas de régénération répétée sans changement utile. Les peers loopback, fichiers synthétiques et choix déclarés ne qualifient ni IdP/fournisseur réel, moteur, image, macOS natif ou parcours complet.

## Reprise du publisher de présence

Choisir le module `--lib broker::presence::http_tests::` et la cible
`--test broker_presence_cli` pour cette modification de boucle :17 fonctions
uniques PASS, une observation de coût ignorée NOT_RUN. Les nouveaux parents
attendent des refus observés, puis contrôlent même acteur/opération, ACK perdu
après commit réel, permits et arrêt ; le vrai service reste dans le même processus.
Les canaris existants de scope, capacité, protocole, historique et rotation
restent dans cette sélection. [Gels et limites](evidence/2026-11-06/presence-recovery/README.md).
Pas de matrice E2E/modèle/moteur ; cadence5s, plafond30 vérifié en source,
fédération physique et reprise de processus à qualifier séparément.

## Confiance gateway du CLI humain

Les nouvelles cibles `human_gateway_trust` et `human_sso_gateway_trust`
exercent CLI/TLS/SQLite réels sur loopback, avec PKI de fixture uniquement.
Quatre parents couvrent CA/default/SAN/validité et export ; un parent SSO
remplace le fichier CA après profil et avant admission, puis exerce
lectures/export/refresh/logout. Deux négatifs `human_cli human_gateway_ca_`
refusent les configurations invalides avant credentials ; trois canaris
préservent export/default, origine/redirect et session native/rotation.
Les dix fonctions, RED et check toutes cibles ont leurs
[gels et limites](evidence/2026-11-06/human-gateway-trust/README.md).
Ce lot n'ajoute pas de suite de qualification du générateur PKI : il utilise
le binaire existant uniquement pour les certificats éphémères du test produit.
Pas de modèle ou matrice E2E ; exclusion WebPKI vérifiée en source, IdP réel,
navigateur, révocation de certificats et runtimes qualifiés séparément.

## Confirmation de quarantaine sans republication

Sélections : `--lib broker::writer::cold_audit_tests::` (8),
`--lib broker::journal::recovery::cold_audit::tests::completion_tests::` (5),
les parents service `retained_reconcile_confirms_quarantine_before_any_fresh_owner_or_post`
et `cold_stop_only_reconcile_loop_publishes_pending_v4_without_start_capability`,
puis `unconfirmable_quarantine_in_first_profile_does_not_starve_second_profile_audit`.

Le dernier appelle la vraie réconciliation de deux profils construits dès
l'origine avec la même installation et des scopes distincts. A est présent dans l'inventaire
mais sa confirmation exacte échoue ; B a deux événements pending admissibles.
La précondition validate_control est enregistrée avant l'appel et vérifiée après
les restaurations/drains ; le seul oracle produit final exige les deux commits
B et leur ACK exact. Un refus global ou un échec de nettoyage n'est pas ce RED.
Les helpers de scope sont opt-in, sans état global ; les defaults historiques
restent inchangés.

Garder les preuves des canaris antérieurs au changement de portée : les sources
writer/journal sont identiques, les chemins service normal/monoprofil préservés.
Rejouer le parent deux profils qui discrimine le changement et le check, sans
répéter ces quinze fonctions. Les seeds runtime/enforcer restent déclarés et
les requêtes HTTP/Stores/journaux sont réels. [Détails](VALIDATION.md#6-novembre--confirmation-de-quarantaine-sans-republication).

## Registre workers humain

`--test human_workers_api` vérifie identité/contextes A/B, permission explicite,
projection whitelistée, budgets durables, révocation et refus d'auto-promotion.
Les trois parents legacy utilisent de vraies admissions et transitions, une
migration et le routeur : UUID nil canonique, date runtime non interprétable
et grand i64. Le hook privé opt-in `AIGUARD_HUMAN_WORKERS_WIRE_DIR` enregistre
trois réponses exclusives et bornées dans la QA ; `human-workers-wire.test.mjs`
lit ces réponses réelles, sans leur substituer des objets JavaScript.

`--test human_workers_migration` utilise le CLI réel, sauvegarde exacte9,
réouverture10, restauration9 dans une destination neuve et refus de capacité
d'audit. Les filtres lib couvrent SQLite FULL, contrat des dates/UUID, curseur
workers distinct et droits révoqués après le commit de lecture. Les canaris
protocole, fédération, broker humain et budget sont exécutés une fois sur les
coutures de schéma/permission inchangées. Une correction localisée de date
justifie ses deux parents, le vrai wire, Node/tsc et le check ; ne pas répéter
les migrations ou la matrice fournisseur sans nouvelle raison.

Les tests frontend ciblent modèle/API/workers. Le scénario navigateur Workers
prépare les tailles320/390/desktop et thèmes, mais sa présence ne vaut pas un
PASS. [Gels, commandes et statuts](VALIDATION.md#6-novembre--registre-workers-humain).

## Propriétaires Full et admission avant ACK

Sélection : `cargo test --locked --lib full_owner_tests -- --test-threads=1 --nocapture`.
Les deux parents de registre couvrent fermeture, échec retenu et observateur
annulé. Les deux parents de service utilisent le vrai journal, HTTP et SQLite :
arrêt bloqué avant retour de la future/lease conservée, et refus `Busy` avant
Resume avec absence explicite de nouvelle commande. Le second vérifie aussi
replays acceptés/refusés exacts à capacité pleine, conflit sur divergence et
Submit neuf sans ticket. Une barrière possédée étend déterministement la fenêtre
entre libération d'une capacité et fin du handle ; elle ne mesure pas sa durée
sur un ordonnanceur réel. Les cinq canaris Cold du mécanisme commun gardent
leurs gels et résultats séparés. Ne pas répéter ces canaris pour une assertion
Full seule ; rejouer le parent concerné et le check toutes cibles.

Le seed runtime et le peer enforcer sont déclarés par le harnais. Aucun moteur,
fournisseur, signal OS ou fencing après crash n'est qualifié par ces tests.
[Commandes, RED, GREEN et limites](VALIDATION.md#6-novembre--propriétaires-full-et-admission-avant-ack).

## Projection centrale workers et brokers

`--test federation_workers_api` couvre les réponses réelles du gateway A/B,
historique d'autorités et schéma2. Le parent dirigé existant de
`worker_relay_api` vérifie aussi le refus avec une identité worker réelle.
Le hook optionnel privé `AIGUARD_FEDERATION_WIRE_DIR` conserve trois réponses
dans la même QA pour `front/tests/federation-wire.test.mjs` ; aucune réponse
de fixture JavaScript ne remplace ce raccord Rust→TS. Aucun fichier n'est
produit sans l'option. Le dossier doit être privé et neuf, les noms exclusifs.
Les tests Node control/api/federation et tsc sont ciblés ; navigateur, build
complet et moteurs ont leurs statuts séparés dans les
[preuves](VALIDATION.md#6-novembre--projection-fédérée-du-registre).

## Prérequis Kubernetes explicites

Sélections ciblées : `--lib runtime::kubernetes::prerequisites_tests::`,
`--test kubernetes_cli`, `--test kubernetes_transport dry_run_`, plus les
canaris exacts préflight CLI et CREATE retenu. Le peer TLS déclare ses réponses ;
les requêtes réellement observées vérifient chemins, corps, scope et absence
d'effet Pod après refus du namespace. Les mutations doublon/profondeur gardent
une enveloppe SSAR valide, afin de discriminer les gardes correspondantes.
Ces tests n'établissent ni les permissions du cluster ni son confinement.
Commandes, gels et limites : [validation](VALIDATION.md#6-novembre--prérequis-kubernetes-explicites).

## Admissions et réservation privée du writer

Sélection ciblée : `cargo test --locked --lib broker::writer:: -- --test-threads=1`.
Les quatre nouveaux parents vérifient capacités0/257refusées et1/256admises,
C ordinaires pleins avec une complétion réservée supplémentaire, FIFO,
capacité rendue au dequeue même si T reste détenu, absence de contrainte Clone,
réserve unique entre clones, drop et valeurs rendues à la fermeture.
Les quatre canaris existants protègent Reject plein/Unknown, Cancel Unknown
et le permis d'inventaire conservé après timeout. Ils utilisent une queue
mise en pause et de vrais fichiers de journal, sans moteur ou fournisseur.

Le seul nouveau watchdog de120secondes concerne l'admission d'une barrière
de ce harnais, auparavant envoyée par un send bloquant. Il ne modifie pas
le timeout produit du writer et ne prouve aucune latence de durabilité.
Après un delta du chemin commun, vérifier cette sélection et la compilation
toutes cibles. Une réservation de queue ne valide pas le coût filesystem,
l'ownership HTTP ou la reprise froide complète.

## Port HTTP exact de l'audit retenu

Sélection ciblée : `cargo test --locked --lib runner::cold_audit_http::tests::`.
Les cinq parents couvrent le vrai gateway et deux Stores SQLite pour A/B,
la lecture Worker, l'ingestion exacte et son replay sans second append ; un
peer HTTP possédé couvre les reçus incomplets, divergents, inconnus, dupliqués,
surdimensionnés, les statuts et redirections, et le refus local sans envoi.
L'oracle des corps compare le vecteur avant/après ; le compteur des requêtes
inclut aussi les GET. Un parent expire synthétiquement le même objet entre
appels et vérifie zéro nouvel envoi. Deux parents supplémentaires vérifient
un Worker désérialisable dépassant512Kio en chunked sans Content-Length et
une échéance synthétique de3secondes pendant un corps incomplet après un
premier fragment de64octets émis par le peer. Le budget original est conservé ; les tâches
du peer et du client sont fermées et jointes. Aucun délai45s réel, SLO ou
comportement réseau après fermeture du peer n'est qualifié.

Les copies de contrôle privées retirent séparément la garde de taille ou
le timeout de lecture : chaque parent doit alors échouer après le trajet
HTTP utile. Ne jamais committer ces copies. Utiliser une cible neuve pour
comparer des snapshots de contenu différent, ou prouver l'invalidation et
la recompilation du paquet. Une simple invocation Cargo sans compilation
ne rattache pas un ancien binaire à une nouvelle source ; conserver les
hashes et déclarer non concluant un essai dont la fraîcheur n'est pas établie.
[Contrôles et limite du cache](VALIDATION.md#5-novembre--bornes-http-en-flux-et-fraîcheur-des-preuves).

Un changement du constructeur HTTP partagé justifie le canari existant
`unavailable_or_unacknowledged_audit_never_yields_allow` de
`tests/runtime_authorization.rs`, puis une compilation toutes cibles.
Ne pas relancer modèles, navigateur ou matrice moteur pour ce seul port.
Le scénario froid complet exige encore identité/terminal recoupés, réservation
de queue et capacité durable sur le même acteur, puis ACK exact du journal.
HTTP/StoredEvent seuls ne constituent pas cette preuve.

## Chantiers et validations en parallèle

Pour le drainage cold, sélectionner les trois parents
`broker::service::server::retained_audit::owners::tests::`, puis le parent
`broker::service::server::retained_audit::shutdown_tests::normal_shutdown_waits_for_a_retained_owner_to_ack_its_committed_receipt`
et les deux canaris de propriétaire partiel/boucle complète. Le registre seul
emploie des futures contrôlées par Notify et des polls explicites, sans sleep.
Le parent d'arrêt retient une vraie réponse HTTP après commit SQLite ; le même
producteur est déjà terminé et les autres JoinSets sont vides. L'attente ne
peut donc pas venir d'eux. La réponse est toujours libérée avant l'assertion
RED finale ; les ACK sont recoupés via snapshot du writer vivant puis vraie
réouverture de lease. Le snapshot est séquencé, pas une mesure atomique de
l'instant du retour. [Gels, résultats et limites](VALIDATION.md#6-novembre--drainage-des-propriétaires-daudit).
Ce test interne ne qualifie ni vrai signal, runtime mort ou moteur Docker/Podman.

Le raccord d'audit terminal possède deux parents ciblés :
`broker::service::server::cancel_retry_tests::cold_audit_tests::cold_stop_only_reconcile_loop_publishes_pending_v4_without_start_capability`
et `broker::service::server::retained_audit::tests::retained_owner_submits_first_real_receipt_when_stopping_before_second_post`.
Ils utilisent un vrai gateway/Store, une vraie lease et le writer Stops ; le
moteur du seed reste synthétique. Le premier vérifie la vraie boucle fenced,
deux commits exacts, ACK après réouverture et conservation des autres faits.
Le second refuse réellement une identité, puis déclenche l'arrêt après le
premier reçu HTTP et exige ACK partiel et second événement pending après perte
des observateurs. Son hook ne fabrique ni reçu, ACK ou résultat. Une mutation
qui abandonne ce reçu avant submit doit échouer sur l'ACK absent.

Ces tests current_thread doivent rendre la main à Tokio pendant l'attente de
lease. Store visible ne signifie pas réponse déjà reçue par l'owner : le test
attend aussi le hook. Les assertions et budgets restent indépendants du poids
de compilation. Ne pas inférer expiration45s intégrée, erreur503 du second
POST, contrôle inactif, arrêt processus ou confinement réel de ces seuls parents.
[Commandes, gels, résultats et limites](VALIDATION.md#6-novembre--propriétaire-authentifié-et-boucle-audit-restreinte).

Pour le lot ACK retenu, sélectionner les douze parents
`broker::journal::recovery::cold_audit::tests::` et les cinq
`broker::writer::cold_audit_tests::`. Les snapshots de baseline précèdent les
POST : un Store neuf contient son genesis, son head ne vaut pas forcément zéro.
Les invariants testés doivent distinguer explicitement Unknown et NotQueued,
compter les deux reçus attendus et préserver les lignes antérieures. Les montages
de disparition/rollback gardent les originaux en backup et les restaurent avant
relecture complète ; le reader brut ne doit pas modifier le floor observé.
[Gels, corrections du harnais et limites](VALIDATION.md#6-novembre--ack-retenu-composé-au-writer-stops).
Ces tests passent sans modèle ni moteur métier et ne qualifient pas ServiceCold.

Le parent `broker::cold_terminal::tests::retained_terminal_checker_matches_real_facts_and_rejects_divergent_clones`
construit une seule fixture terminale journal/HTTP/SQLite, puis recoupe les
variantes en mémoire et l'absence de mutation des snapshots suivis. Les
contre-exemples couplés sont tous évalués avant l'assertion agrégée : un premier
échec ne doit pas masquer les autres résultats. Le moteur y est synthétique ;
ne pas créditer ce parent à la qualification runtime ou à la boucle ServiceCold.
[Gels et résultats](VALIDATION.md#5-novembre--cohérence-pure-des-faits-terminaux-retenus).

Pour la préparation de capacité d'audit retenu, sélectionner
`broker::journal::recovery::cold_audit::tests::` : sept parents emploient les
vrais reader/journal/encodeur et des copies de taille non persistées. Leur
périmètre inclut V1/V2/V4, bytes/entrées/révisions, plafonds imbriqués,
réservation unique, rejets/incomplets et Cancel concurrents. Cela ne remplace
pas les tests du même writer actor, du client authentifié, du batch ACK ni de
la boucle froide. [Preuves et limites](VALIDATION.md#5-novembre--capacité-logique-de-laudit-v4-retenu).

Consigne du mainteneur du5novembre : exploiter les tâches indépendantes,
y compris les compilations et tests, sans imposer une sérialisation globale
pour ménager le poste. Chaque chantier garde ses propriétaires de fichiers,
son worktree et ses preuves. Les environnements d'exécution restent admis
séparément, bornés et privés ; seuls sources et caches publics vérifiés peuvent
être partagés en lecture seule. Deux commandes Cargo ne partagent pas un
répertoire cible mutable ou une base de test.

La pression du MacBookAirM3/16Go peut allonger un essai de30secondes à2minutes.
Les budgets du harnais doivent le permettre ; un timeout d'observation n'arrête
pas le processus et ne justifie pas son redémarrage. Reprendre le même handle.
Les échéances métier, fenêtres d'émission, contrôles d'identité et barrières
de durabilité restent inchangés. Une durée observée sous charge ne qualifie
ni SLO, ni capacité, ni confinement. Conserver le périmètre exact des PASS/FAIL
et distinguer une panne du produit d'une limite d'infrastructure démontrée.

## Client held-CREATE et reprise du résultat

Pour le client interne, sélectionner les neuf parents de
`broker::held_create_client::tests::`, les deux `bounded_io_` de
`runtime::kubernetes::admission::journal::tests::` et les parents de
`runtime::kubernetes::admission::remote_coordinator_tests::`. Ils utilisent
HTTP/SQLite/fichiers réels et un peer Kubernetes TLS explicitement déclaré,
sans cluster ou fournisseur. Le canari legacy de permission
`pending_claim_samples_before_the_real_commit_and_replay_never_mints_permission`
et celui du coordinateur `coordinator_commits_unknown_result_without_publishing_a_handle`
protègent le chemin Store8 lors d'un changement de provenance du permis.

Le premier défaut doit être reproduit sur son vrai trajet avant correction.
Une compilation échouée ou une assertion antérieure aux compteurs utiles
ne démontre pas un refus distant. Distinguer précriblage local sans Result
HTTP,400 réellement reçu, perte de lecture de réponse après commit central
et reprise exacte du candidat. Vérifier candidat/premier refus inchangés,
ACK seulement après snapshot complet et absence de second CREATE. Le test
de perte d'attente async garde capacité/closure jusqu'à terme ; aucune
qualification d'arrêt runtime/processus ou de panne physique n'en découle.
Conserver les gels par source et les commandes/résultats dans
[VALIDATION](VALIDATION.md#5-novembre--client-broker-http-et-journal-kubernetes-composé).

## Prérequis du paquet source avant build

Pour ce chemin, sélectionner dans `tests/runtime_program.rs` les trois
fonctions `build_refuses_missing_worker_dockerfile_before_any_engine_call`,
`build_refuses_malformed_public_source_inputs_before_any_engine_call` et
`build_accepts_complete_source_and_forwards_provider_versions`, ainsi que
les quatre canaris Build existants : changement d'identité avant effet,
identité entre les deux images, endpoint retenu et chemins absolus.

Les fixtures copient uniquement 19 fichiers publics réels, sans parcourir
node_modules, dist, caches ou credentials. Elles satisfont le contrôle
structurel, sans constituer un projet compilable complet. Le vrai CLI lance
un moteur témoin Rust dans le sandbox ; les marqueurs prouvent l'absence
d'appel ou les arguments/effets attendus, pas une construction d'image réelle.
Le RED précède le code. Après un delta, exécuter ces sélections et vérifier
la compilation concernée ; ne pas rejouer modèles, navigateur ou recette
moteur pour ce seul préflight. [Résultats et limites](VALIDATION.md#4-novembre--prérequis-de-build-avant-tout-appel-moteur).

## Diagnostic de récupération sans historique

Pour un delta sur l'ouverture d'historique du broker, sélectionner les six
parents de `broker::service::server::cancel_retry_tests::stop_auth_tests::recovery_inventory_tests`
et le canari exact `inactive_real_gateway_allows_fresh_stop_and_recovery_metadata_only`
du module parent. Ils couvrent les absences à trois niveaux, le chemin normal
sans namespace de journal, les refus d'identité et d'UUID, le retry de la
cellule lazy puis un vrai journal vide, ainsi que la récupération/annulation
existante. Le gateway HTTP et SQLite sont réels ; le gardien natif de fixture
est détenu et fermé sans lancer de worker. Ce périmètre logiciel ne qualifie
ni panne de disque ni reprise runtime. Les commandes et gels sont dans
[VALIDATION](VALIDATION.md#4-novembre--journal-requis-absent-et-diagnostic-de-récupération).

## Réconciliation après Cancel — sélection logicielle warm

Le chemin de reprise vise un job durablement annulé avec worker `Running`
ou `Stopping`, sans nouvelle autorisation de démarrage. Les quatre parents
de `runner::broker::executor::journal::tests::cancelled_recovery_tests`
amorcent un worker `Running` : terminal exact avec Owned initial Running,
terminal exact avec Owned initial Created, refus d'observations divergentes,
et perte de réponse HTTP après commit terminal puis replay du résultat.
Vérifier quota tenu sur inconnue/divergence, libération seulement par la
transaction terminale admissible, allocation unique non facturable, même CID/Arm/claim,
UUID et reçu d'audit du résultat conservés, puis acquittement local idempotent.
Ces parents ne constituent pas une preuve distincte de l'état `Stopping`.

Store SQLite, HTTP, Journal et Writer sont réels ; les observations moteur
proviennent d'un programme Rust synthétique. Le seed séquentiel utilise un
seul runtime de test et un `spawn_blocking` joint, sans augmentation de pile.
Aucune qualification Docker/Podman, reprise froide, fencing ou D5 n'en découle.
La sélection finale dans la QA confinée passe : quatre parents, zéro échec,
zéro ignoré, RC0. Les temps et gels exacts, ainsi que le nettoyage des seuls
diagnostics temporaires, sont consignés dans
[VALIDATION](VALIDATION.md#5-novembre--réconciliation-logicielle-après-annulation) :

```sh
cargo test --locked --lib runner::broker::executor::journal::tests::cancelled_recovery_tests:: -- --nocapture --test-threads=1
```

Ce groupe demande explicitement un timeout Writer de fixture de 60 s pour
isoler le comportement métier. Les autres fixtures et le produit conservent
10 s. Le diagnostic à 10 s confirme un timeout de réception ; le snapshot
ultérieur retrouve les ACK durables, sans dater le commit par rapport au timeout.
Un appel `acknowledge_events` a ensuite été mesuré à
10 303 ms de temps mural sur QA à 1 CPU/debug, avec hôte M3 chargé. Ce temps inclusif
ne mesure ni CPU consommé, ni fsync isolé, ni p95 ou performance produit.
Un résultat à 60 s ne qualifie pas le budget produit de 10 s. Sa fiabilité/performance
et la vraie reprise/fencing restent des écarts P1 ouverts ; voir les
[gates](PRODUCTION_READINESS.md) et le [contrat de supervision](BROKER_SUPERVISION.md).

## Coût de la présence — compteur et échantillons manuels

Le compteur test attribue chaque sync d'ancêtre de `durable_bootstrap` au
répertoire appelant ; un scope physique d'ancêtre ignorerait la plupart des
appels. `EXCLUSIVE` sérialise les producteurs du collecteur global :
`blocking_lock` seulement dans les unités synchrones, `.lock().await` dans
les helpers async, guard avant Measurement et conservé jusqu'à finish/drop.
Pour vérifier ses deux consommateurs synchrones, exécuter les deux filtres
exacts avec `--test-threads=2`, comme dans [VALIDATION](VALIDATION.md#4-novembre--compteurs-de-durabilité-et-snapshot-du-publisher).

Le canari actif `validated_head_snapshot_does_not_resynchronize_history`
exige zéro sync/rescan de records dans la lecture du snapshot, sans exiger
zéro IO des ancres. La donnée cachée n'autorise aucun effet. Après un delta
sur ce chemin, sélectionner les canaris historique live, capacité pending
et ACK perdu/restart ; éviter le replay de tous les parents inchangés.

Le parent manuel
`broker::presence::http_tests::cost_tests::publisher_cycles_one_real_heartbeat_at_history_depths_0_8_32`
est ignoré par défaut et doit être exécuté explicitement avec `--ignored
--nocapture --exact`. Il prépare N0/8/32 reçus authentiques hors fenêtre,
mesure un seul POST réel puis draine IO/joint l'acteur avant finish ; un
reopen/Store replay après fenêtre prouve le reçu et l'absence de doublon.
Un drainage inconnu provoque une sortie enfant86 avant Drop, avec mesure
incomplète/nonzero et conservation des fichiers jusqu'au contrôle Root de
la fin réelle ; aucun échantillon suivant. Ce chemin n'est pas injecté par
le lot et n'est pas présenté comme une qualification de panne physique.

Le temps inclut fixture HTTP, middleware O(N) et polling5ms, pas la cadence
ou le seeding. Les syncs sont réels mais la cible est tmpfs, non disque
physique. RSS/FD sont des observations du processus entier. Ni percentile,
gain statistique de durée, CPU consommé, prix ou capacité100 déduit.

## Présence du service broker — validation ciblée

`broker::presence::` couvre le vrai gateway/Store et les fichiers privés :
replay après commit/réponse perdue, CAS et hash exact, nouvelle incarnation,
rotation explicite, grant changé/révoqué, budgets, tampering historique et
capacité réservée avant un POST repris. Les corruptions passent par des
entrées de fixture locales ; elles ne démontrent pas une panne électrique.
Les trois `broker::service::server::presence_material_tests` vérifient les
exclusions de copie réelles, la capacité cumulée avant ancre et la séparation
entre flux indisponible et matériel privé dangereux.

`tests/broker_presence_cli.rs` lance le vrai binaire `broker serve`, un
gateway SQLite isolé et un gardien du même binaire. Aucun moteur, job,
fournisseur ou modèle n'est lancé. Le parcours Full exige heartbeat durable,
arrêt/reap du service et absence d'avancement après son arrêt ; le parcours
Recovery exige un RPC authentifié et aucune ouverture/publication de
présence quand sa configuration est absente. Un refus sûr de démarrage ne
remplace pas ce contrôle positif. Voir les échecs et résultats actuels dans
[VALIDATION](VALIDATION.md).

Ne rejouer que les parents concernés par un delta : une correction de
classification HTTP justifie les canaris de refus/200, pas toute la suite.
Une erreur de compilation de fixture n'est pas un défaut produit reproduit.
Les délais de garde90s/45s des enfants ne modifient ni drain produit15s ni
échéance worker ; aucune performance n'est déduite de cette machine chargée.

## Portée de la preuve Broker et préparation Kubernetes

Pour une modification de `GET /api/v1/brokers/identity`, sélectionner
`cargo test --locked --test broker_identity_api -- --test-threads=1` et
`cargo test --locked --lib audit::broker_registry::identity_view_tests -- --test-threads=1`.
Les trois parents API vérifient Store A6/B7 avec UUID identiques, classes
de preuve interdites et refus d'une identité contenant le Bearer. Les trois
unités Store vérifient preuve déjà extraite puis rotation/révocation,
présence unseen/stale et grants remplacés, puis plancher d'horloge durable.
Le Worker utilise une projection déclarée de fixture ; aucun moteur n'est
appelé. Les expirations de binding/grant et fautes physiques restent des
qualifications distinctes si non exécutées dans le lot.

Les profils Kubernetes publics sont des configurations. Leur parsing et
leur revue sont distincts de leur application. Les sept décisions opérateur
`kubectl auth can-i` sous impersonation ne comptent ni comme tests Rust ni
comme authentification ServiceAccount réelle, lancement Pod ou confinement
CNI. Conserver endpoint, contexte, versions, UID des objets possédés et
refus attendus. Pas de création de100workers, d'appel modèle ni de campagne
E2E pour ces deux coutures ; [preuves](VALIDATION.md).

## CLI workload — sélectionner selon la couture modifiée

`tests/workload_cli.rs` exerce la vraie CLI, des gateways Axum/Store SQLite
et une AC TLS privée possédée. Le lot du4novembre a16parents CLI et1unité
filesystem uniques passés sur les gels reliés ; le test TLS rejoué avec son
contrôle positif n'est pas recompté. Les commandes, sélections, RED,
échecs de fixture et hashes figurent dans [VALIDATION](VALIDATION.md#4-novembre--cli-durable-des-identités-techniques).

Sélectionner `--test workload_cli` pour une modification de ce parcours.
L'unité exacte `workload_cli::storage::tests::resume_resynchronizes_complete_files_before_returning`
vérifie les appels `File::sync_all` réels après réouverture de fichiers
réécrits sans flush ; ne pas en déduire panne électrique ou fsyncError.
Les altérations de réponses sont des middlewares de fixture après le vrai
handler/commit ; aucune logique métier simulée ne remplace le Store.
Le parent Worker utilise des projections déclarées de contrôle, sans moteur.

Un changement de tests/oracle ne justifie pas le replay des autres parents
inchangés. Réserver les scénarios réels Docker/Podman/A1A2B1 à leurs gates
distincts ; ni ce client ni ses17tests ne qualifient ces runtimes, PKI
entreprise, transport réseau, handoff ou fédération. La compilation release
est effectuée une fois sur la composition applicative finale ; Clippy Linux
reste indisponible dans l'image de contrôle épinglée.

## Séparer un échec du harnais du comportement produit

Un échec pendant la préparation, avant l'oracle métier, ne reproduit pas le
défaut recherché. Conserver la phase effectivement atteinte, le gel source,
la commande et le résultat ; un dépassement de pile ou un timeout de setup
n'est ni un RED métier ni une preuve de confinement. Vérifier la trace
complète avant d'attribuer l'échec à un composant.

Borner chaque investigation du harnais avec une hypothèse et un diagnostic
discriminant. Si les essais n'atteignent toujours pas le comportement visé,
préserver le travail incomplet et poursuivre un lot indépendant utile au
produit, avec des propriétaires distincts. Ne pas monopoliser l'objectif
global, augmenter les piles ou délais à l'aveugle, ni remplacer une preuve
runtime par une fixture présentée comme terminée. Reprendre le diagnostic
avec les observations conservées plutôt que relancer la même matrice.

## Latence et pression de la machine de développement

La machine locale de référence du mainteneur est un MacBook Air M3 avec
16 Go de RAM. Docker Desktop utilise une VM configurée à 12 Go ; ce réglage
ne constitue pas une mémoire supplémentaire pour l'hôte. Les modèles locaux,
dont Qwen, et les autres tâches peuvent fortement ralentir les essais.

Privilégier les barrières, notifications, états durables et compteurs pour
vérifier un comportement. Un délai de garde du harnais borne l'essai ; son
expiration seule ne démontre ni vulnérabilité ni échec du comportement visé.
Noter la phase atteinte et distinguer préparation, compilation, attente et
inférence. Ne pas relâcher un délai contractuel du produit pour obtenir un
résultat vert. Une mesure de performance exige son propre protocole, les
versions, les ressources et la charge concomitante ; elle ne se déduit pas
d'un test fonctionnel sur cette machine sous pression.

Limiter les compilations lourdes concurrentes à une sur cette machine ;
continuer en parallèle le code, les revues et la documentation. Pour une
correction sans effet sur l'inférence, ne pas démarrer de modèle local ni
relancer une recette fournisseur ou E2E complète.

## Préparation SSO locale — ciblage du 2 novembre

`setup_prepare` couvre les vrais sous-processus de préparation et leurs fichiers/SQLite : publication neuve, refus, reprise exacte et stdin borné. `setup::tests::` vérifie les interruptions durables, la saturation d'audit et la reprise d'une vraie base>32Mio ; `closed_entry_sync_` vérifie la barrière fichiers sans élargir les lectures. Le parent temporaire des canaris est canonique pour respecter la même politique nofollow sur macOS et Linux. Choisir uniquement les cas touchés ; les résumés8+24 et les commandes finales4+2/fmt/Clippy ont des preuves distinctes, décrites dans [VALIDATION](VALIDATION.md#2-novembre--préparateur-sso-privé-et-profils-de-lancement).

Les rendus Compose, l'import réel des trois IDs Keycloak et la revue statique des commandes ne remplacent ni la préparation du volume de contrôle conteneurisé ni le parcours réel gateway/Web/CLI. Les outils auxiliaires IdP restent privés, hors dépôt. Aucun appel fournisseur ou E2E global n'est requis pour rejouer une correction purement fichiers ou stdin.

## SSO candidat — ciblage du 2 novembre

La cible `oidc_gateway` comprend désormais6fonctions de frontières HTTP : configuration désactivée/callback et page publique de fin de redirection. Le filtre `dashboard_oidc_completion` sélectionne3fonctions par préfixe ; ne pas lui ajouter `--exact`. Les tests servent un vrai fichier statique et distinguent navigation cross-site autorisée des actions/tokens/API/subresources/query/SSOoff refusés, sans IdP ni moteur. Le trajet navigateur avec Keycloak exige une preuve distincte après construction de l'image ; HTTPstatus sain et callback303 ne prouvent pas une remise de tokens réussie. Cette preuve existe pour les parcours personnels `user-1`, `user-2` et le contexte d'administration `admin-ai-guard` sur l'image issue de96fe9be, avec DNS dédié au navigateur. Pour les ressources, corréler le vrai GET `resources?limit=25` sans paramètre `view` au contexte et à sa version des droits, vérifier couverture/reçu et comparer le nombre d'éléments au rendu ; l'administration n'implique pas une liste vide. Les échecs de préparation et de filtrage du harnais restent datés séparément. Les essais auxiliaires restent privés ; ni CLI natif, renouvellement réel, refus entre utilisateurs, worker ni qualification globale ne sont déduits de ces connexions. [Résultats](VALIDATION.md#2-novembre--sso-web-réel-sur-la-page-de-fin-corrigée).

Les cibles `oidc_authentication_audit`, `oidc_gateway`, `oidc_gateway_flow`, `oidc_transport` et `sso_cli` vérifient le produit avec SQLite durable, HTTP/TLS contrôlé, JWT signés de test et vrais sous-processus CLI. Elles ne qualifient pas Keycloak, le navigateur, un fournisseur payant ni un moteur. `human_access` conserve les canaris de plafond de permissions ; `human_cli` couvre la compatibilité locale. Exécuter les régressions touchées, puis les contrôles statiques, sans rejouer systématiquement tous les parcours E2E. [Résultats par gel](VALIDATION.md#2-novembre--composition-sso-résultats-comportementaux-et-revues).

État logiciel consigné dans d6cb136 : les24tests `sso_cli` passent ensemble ; fmt et Clippy strict finaux PASS. Le test transport exact passe après correction de sa fixture ; les15autres résultats restent ceux du gel précédent, sans suite transport entière rejouée. Les résultats intermédiaires CLI19/21 et transport15/16 restent dans la chronologie, sans décrire des défauts encore ouverts après ces corrections. Les travaux SSO/setup du worktree distinct ne sont pas intégrés à ce gel et ne qualifient aucun bootstrap.

Après copie de sources depuis une archive, ne pas supposer Cargo frais sur les seules dates restaurées : renouveler le mtime des entrées exactement copiées, vérifier leurs bytes, puis contrôler que les tests sélectionnés s'exécutent réellement. Zéro test ou un test ignoré ne constitue pas un PASS. Un échec pendant login/PKI préparatoire ne reproduit pas un oracle de refresh qui n'a pas encore été atteint.

## Diagnostic Jev — ciblage et absence d'appel modèle

`cargo test --locked --test jev_diagnostics` exécute dix intégrations contrôlées TCP loopback/SQLite ; l'unité exacte `jev::local::tests::final_local_checkpoint_rejects_late_masks_of_the_whole_fixed_frame_without_connecting` couvre les masques introduits après construction. Deux attentes testent le vrai budget5s, sans le modifier. Canaris retenus : `jev_contract`, `jev_backends`, `jev_store_redaction`, `decision_idempotence`, soit13 fonctions existantes ; fmt/Clippy strict complètent ce périmètre. Les24 fonctions passent sur le gel composé documenté, sans rejouer humains/navigateur ni appeler un modèle/fournisseur. [Résultats et limites](VALIDATION.md#1er-novembre--diagnostic-jev-fermé-et-trame-locale).

Les réponses des serveurs de test vérifient protocole et refus du produit ; elles ne qualifient pas TypeSafe ou le jugement Qwen. Toute nouvelle génération réelle relève d'un essai explicite distinct, avec ses limites et preuves. Le FAIL_MODEL antérieur ne devient pas PASS parce que les diagnostics natifs passent.

## Accès humain local — ciblage du 1er novembre

Les cibles Rust pertinentes sont `api::human_state::tests`, `human_access`, `human_access_migration`, `human_cli` et `human_redaction`. Elles utilisent de vrais fichiers/SQLite/routeurs et sous-processus CLI avec entrées de test explicites. `human_access` inclut70batches SSE : réponse66 perdue/rejouée, snapshot drainé, deux nouvelles arrivées et73 reçus comparés à l'audit durable. Cette couverture est distincte de la pagination REST72pages et de la recette Docker3workers.

Exécuter les cibles touchées, puis canaris/statiques appropriés. Les résultats couvrent plusieurs gels : assertion expiry mal préparée, fixture CLI accordée avant fait connu et correction Clippy sont conservées dans la [chronologie](VALIDATION.md#1er-novembre--accès-humain-local-apicli-et-dashboard), sans compter les rejeux comme nouvelles fonctions. La modification finale du prédicat d'expiration ne rejoue que son test exact et fmt/Clippy ; aucune suite globale n'est déduite.

Dans `front/`, `node --test --test-isolation=none tests/human-model.test.mjs tests/human-api.test.mjs` cible les17 unités humaines. Le navigateur ciblé utilise `AI_GUARD_HUMAN_SCENARIO=focus node tests/browser_human.mjs` ou `AI_GUARD_HUMAN_SCENARIO=access-modes node tests/browser_human.mjs` ; variable absente pour le parcours humain et ses captures. Ces fixtures front ne qualifient pas le backend/runtime. Ne pas relancer ces parcours automatiquement pour une correction sans effet sur eux. Source composée/build vérifiés sur Node25.6.1 ; tests Node24 et parcours humain5 en vrais conteneurs NOT_RUN au gel du1er novembre. La [recette Docker API/CLI A/B du2novembre](VALIDATION.md#2-novembre--recette-humaine-réelle-docker-apicli-ab) passe depuis sur son image figée, sans navigateur ni worker.

S0,1er novembre : RED natif de lease après abandon listener, puis sept fonctions ciblées PASS (parent IPC/acteur/idle inchangé, trois cas owner, deux intégrations du vrai binaire et retained Cancel). Fmt/Clippy strict/release arm64 PASS et revue source traitée. Deux fences Unknown de test conservées ; marqueur payload≠join, aucun crash processus/OS ou moteur qualifié. Sources auxiliaires/ignorées hachées avant Clippy, sans réussite PKI/fournisseur attribuée au lot. [Gels, commandes et limites](VALIDATION.md#1er-novembre--s0-r2-green-ciblé). Ledger/retrait durable, G09 et recette Docker/Podman restent ouverts ; pas de répétition des32STOP3 ou d'E2E global pour ce correctif.

Les résultats nouveaux sont dans [VALIDATION.md](VALIDATION.md). Les résultats historiques de [../validation.md](../validation.md) ne qualifient pas ce checkout sur macOS ARM64. Les critères détaillés existants sont dans [../acceptance.md](../acceptance.md) ; leur ancien découpage MVP/V2 ne réduit pas la mission production.

## Navigation liée du dashboard

Le script frontend `node tests/browser_unified.mjs`, avec les prérequis Playwright du navigateur existant, cible les liens sandbox/agent/worker, pages du registre, sources en erreur/périmées, provenance et réponses tardives/changement de client. Les sélections `AI_GUARD_UNIFIED_SCENARIO=usage-links`, `retry-focus`, `removed-link-focus` et `summary-notice` isolent leurs régressions ; la variable absente lance le parcours lié et ses captures. Utiliser un dossier privé `AI_GUARD_UNIFIED_ARTIFACTS` pour les images. Ces fixtures restent dans les tests et ne remplacent aucune recette du gateway Rust ou du runtime réel. Rejouer seulement les groupes touchés par un changement ; [gels et résultats](VALIDATION.md#1er-novembre--navigation-liée-du-dashboard-et-reprises).

## Transports STOP3 et TLS — vérifications ciblées du 1er novembre

STOP3 candidat1 :21 fonctions V3 (14 initiales,4 régressions et3 groupes hardening),6 de compatibilité2,1 auth-binding et4 canaris de service passent, sans moteur. Les tests ancrent vraiment leurs sockets/leases ; les cas numériques partent des bytes originaux et les vecteurs MAC utilisent des littéraux indépendants des serializers. La fragmentation exerce un budget restant positif, sans prétendre couvrir tout le slowloris6s. Les longues fixtures de service ne définissent pas la latence du protocole. Ledger/Closing/join/retraite/fencing restent hors de ces preuves.

TLS T1 :34 fonctions ciblées passent (10 nouvelles et24 canaris), puis deux fonctions séparées reproduisent PEM imbriqué et rétention du shutdown après abandon. Elles nettoient leurs fixtures avant les oracles ; ne pas transformer les sous-cas en nombre de tests ou prendre T1 pour une clôture. Les correctifs suivants doivent vérifier ces deux fonctions et les chemins TLS/PEM/shutdown effectivement touchés, sans rejouer par défaut toute la matrice Jev ou les longs canaris D5 inchangés. [Résultats par gel](VALIDATION.md).

T2 ferme ces deux RED avec sept fonctions pertinentes (les cinq intégrations TLS, deux unités PEM/clé), sans rejouer34 T1 ni32 D5. Fmt, Clippy strict et une release composée passent ensuite ; les résultats/limites du sidecar ignoré sont explicités dans VALIDATION. Les certificats éphémères et serveurs TLS/HTTP sont des fixtures de comportement **produit**, pas les essais auxiliaires AC/Qwen/Keycloak exclus du dépôt. Docker/Podman, lecteur réel UID1000, réseaux et modèle exigent leurs preuves distinctes ; le probe stat UID1000/0444 ne les remplace pas. Ne pas relancer les campagnes globales à chaque RED.

## Jev — sélection et frontières de données

Le [lot logiciel J3b](VALIDATION.md#1er-novembre--jev-j2j3b-corrections-et-validation-ciblée) possède62 fonctions ciblées réellement passées, avec ses gels et limites. Les six cibles `jev_backends`, `jev_request_bounds`, `jev_masking`, `jev_typesafe_ambiguity`, `jev_store_redaction` et `jev_protocol` vérifient protocole HTTP contrôlé, API/Store réels, catalogue exact, avis local, bornes, doublons, masquage et replay. Les canaris d'authentification, décision et supervision sont sélectionnés selon ces risques ; aucun appel fournisseur ni moteur n'est requis pour ces régressions.

Les filtres des deux unités CLI exigent le préfixe `tests::` avec `--exact`. Un RC0 avec zéro fonction ne compte pas PASS. Le corpus ne qualifie ni jugement Qwen, ni consommation réelle du modèle, ni TLS privé conteneurisé ; conserver ces campagnes séparées. Un cas de dictionnaires Store A/B distincts et de replay après changement de backend reste une extension de preuve ciblée, pas un PASS déduit des tests mono-Store.

## Domaine d'identité et stockage4

`tests/identity_migration.rs` vérifie les données réelles SQLite/migrations/backup et les scopes/grants ; `identity::tests::` contient six unités de domaine et `audit::identity::tests::` la panne SQLite FULL. Attention au filtre sous-chaîne Rust : `identity::tests::` sélectionne aussi cette dernière ; ne pas compter deux fois le même test ni le rejouer ensuite par automatisme. Les canaris de migration/bootstrap/protocole/CLI sont choisis selon les coutures touchées, et le fixture API ne doit contenir aucun de ses credentials dans l'enveloppe métier positive. [Gels et26 fonctions uniques constatées](VALIDATION.md#1er-novembre--domaine-identité-et-migration4).

Cette couverture ne remplace pas les requêtes humaines réellement authentifiées, l'ACL avant pagination/SSE/export, la révocation pendant flux, le provisioning local ou OIDC ci-dessous. Les méthodes Store exigent un appelant de contrôle déjà authentifié ; sérialiser un principal ne le prouve pas. Le prochain lot doit livrer le parcours productif avec ses tests HTTP/CLI/UI, sans fausse connexion ni promotion en runner/viewer global.

## Identité individuelle, SSO et découverte — à qualifier

Exigences approuvées le 31 octobre, détaillées dans [le contrat canonique d'identité et de configuration](../sso.md). Cette matrice fixe les preuves d'acceptation ; ses libellés ne sont pas des noms de suites déjà disponibles. Les preuves logicielles, Docker API/CLI locales et connexions Web réelles ci-dessus couvrent seulement leurs scénarios nommés. La matrice complète, le CLI contre le vrai IdP, la découverte jusqu'à une exécution et la délégation restent ouverts. Le SSO facultatif à l'installation ne dispense pas le produit de qualifier son connecteur et sa documentation. Les comptes préconfigurés servent à l'environnement explicite de découverte ; les événements d'exécution doivent être réels.

| Cas | Preuve et refus attendus |
|---|---|
| Axes de configuration | Couvrir les [parcours non exhaustifs](../sso.md#parcours-à-couvrir) et les interactions critiques entre topologie, identité locale/OIDC, périmètres, abonnement/API/inférence interne et réseau ; vérifier priorités, contraintes administratives, choix résolus/provenance masquée et refus explicites des combinaisons incompatibles ou non qualifiées |
| Sans Internet | Vrais services internes et confiance TLS configurée ; aucune dépendance publique obligatoire, aucun téléchargement, télémétrie ou repli caché ; refuser le profil exigeant abonnement/API/IdP publics sans connectivité ; tester préparation des images/dépendances client et diagnostics d'indisponibilité |
| Inférence interne | Images construites/importées par le client, versions épinglées ; URL, modèle, format exact compatible OpenAI et adaptation Claude qualifiée, credential minimal et chaîne publique d'AC préparés dans le worker ; tests positifs/négatifs de chaîne et nom TLS, DNS, streaming et outils selon les capacités annoncées, destinations internes approuvées et absence de repli cloud ; aucune clé privée d'AC |
| Sans SSO | Installation locale hors ligne, utilisateur nominatif et attribution conservée ; aucune requête obligatoire vers un IdP, aucun contournement de l'authentification |
| Vrai fournisseur | Compose neuf Keycloak épinglé, realm importé automatiquement, clients publics sans secret ; dashboard et CLI utilisent réellement Authorization Code + PKCE S256 avec `user-1/user-1` et `user-2/user-2` |
| Rôles distincts | `admin/admin` administre le realm sans droit applicatif implicite ; `admin-ai-guard/admin-ai-guard` reçoit le rôle `admin-ai-guard` dans les claims vérifiés et la vue d'administration de son périmètre ; les deux utilisateurs normaux en sont refusés côté Rust, sans credential `runner` distribué |
| Claim administrateur | Mapping configurable des chemins de claims et noms externes, sans dépendance au nom de découverte `admin-ai-guard` ; refus de mapping absent/ambigu, rôle absent, falsifié, révoqué, émis par un autre issuer ou pour un autre client ; un rôle valide n'ouvre pas les organisations/tenants non autorisés, un nom de compte n'accorde aucun rôle |
| Vues selon les permissions | Vrais parcours utilisateur/admin avec données attribuées et configuration effective ; navigation, contenus et commandes adaptés ; refus serveur pour URL/API/export/SSE hors droits, séparation des permissions de consultation/contenu/mutation, thèmes clair/sombre et petit écran conservés |
| Vue administrateur à 360° | Agrégation des seules ressources autorisées de plusieurs utilisateurs/brokers ; filtres et liens vers détails cohérents, sources manquantes/périmées visibles, aucune donnée d'un client tiers, aucun total global extrapolé d'une page ni usage inconnu présenté comme nul ; qualification réelle distincte des fixtures |
| Changement de contexte et révocation | Retrait des données, détails ouverts et caches devenus interdits, arrêt/revalidation des abonnements aux flux concernés et refus des requêtes en cours devenues non autorisées ; pas de résidu de l'ancien périmètre après changement de rôle, utilisateur, tenant ou expiration |
| Réponses adverses | Refus PKCE absent/incorrect, state/nonce incohérents, code rejoué, signature/issuer/audience/expiration invalides, redirection non approuvée et confusion entre ID Token et accès API ; refus côté Rust |
| Sessions et incident | Renouvellement, expiration, déconnexion, révocation et rotation de clés ; panne IdP visible, aucun repli silencieux ni nouvelle autorisation ; politique explicite et bornée pour sessions/identités déjà établies |
| Tenants | Identité humaine et tenant issus du contrôle ; même nom/e-mail chez deux issuers ne fusionne pas les comptes, aucune fuite de session, événement, commande ou résultat A/B |
| Délégation et causalité | Utilisateur → demande → agent → sous-agent → commande → décision → effet réel ; actor/initiator distincts, droits non élargis, falsification du parent/tenant refusée, sous-agent déclaré clairement distinct d'une identité vérifiée |
| Audit et reprise | Demandes/réponses visibles, arguments, décisions et observations liés après concurrence/crash/restart ; audit obligatoire indisponible bloque l'effet, aucune duplication ou fausse attribution ; export et points de contrôle vérifiés |
| Contenus et secrets | Aucun jeton SSO humain ni credential de contrôle dans un worker ; clé API limitée au worker/client/mode autorisé par handoff privé ; aucun de ces secrets dans les argv, logs, événements ou captures ; accès/rétention bornés, masquage/troncature et perte de couverture visibles |
| Découverte chronométrée | Suivre uniquement la documentation depuis un état neuf jusqu'à l'interface puis à une première exécution auditée ; une clé du fournisseur choisi, aucun paramétrage manuel du realm ; distinguer téléchargement/build et démarrage à chaud, publier étapes/versions/durées sans transformer cinq minutes en promesse |
| Préservation et documentation | Activation explicite et loopback, isolation des volumes/configs de démo, credentials existants préservés ; commandes réellement copiables, arrêt/reprise, démo et raccordement entreprise documentés séparément |

Préférer les tests ciblés de contrats pour les nombreux refus et réserver les parcours réels Keycloak/Web/CLI aux jalons et changements d'authentification concernés. Les essais fournisseur restent bornés selon le budget du mainteneur ; un échec de harnais se corrige hors ligne avant répétition. Les quatre comptes de découverte servent aussi à vérifier les séparations de rôles et d'accès entre utilisateurs ; des identités supplémentaires restent propres au harnais si nécessaires. Un IdP simulé ou des transcripts synthétiques ne remplacent pas la qualification réelle. Les nouvelles automatisations du projet restent en Rust, les tests frontend en TypeScript ; les imports JSON et fichiers Compose sont des configurations.

La couverture n'est pas limitée à la liste initiale de profils : maintenir les capacités par axe, les incompatibilités et les interactions à risque, avec configuration/versions/preuves pour chaque parcours qualifié. Ne pas relancer tous les croisements ni les appels fournisseurs à chaque changement ; sélectionner les cas selon le risque et conserver `NOT_RUN` pour les combinaisons sans preuve. L'ajout d'un profil ne qualifie pas automatiquement ses variantes.

## Vérifications logicielles

Le [probe de coût durable](evidence/2026-11-01/journal-cost/README.md#rejouer-uniquement-la-mesure-et-ses-canaris) est un test lib opt-in ignoré par défaut : vrai Journal/Writer, heads0/8, reçu local/CAS/chaîne vérifiés, aucun moteur ou store simulé. Les compteurs valident la collecte, pas un seuil de rapidité. Sept fonctions ciblées, fmt/Clippy/release passent au gel S6 ; les huit échantillons ne sont pas huit tests et le filtre vide reste NOT_RUN. L'instrumentation est uniquement cfg(test), les temps inclusifs se chevauchent, le timeout est coopératif et n'annule pas une commande acceptée. Mesures debug sur macOS ARM64, FS exact NOT_RECORDED : aucun percentile, performance release,100workers, panne disque ou cause H10. Ne pas relancer navigateur/fournisseur/E2E pour ce seul probe.

Pour la présentation des échecs broker, les tests modèle frontend vérifient les labels fermés, les valeurs inconnues, l'effet non confirmé, la présence dans Actions sans incident Security automatique et l'absence de contenu privé dans le résumé spécialisé. Le complément natif conserve la priorité broker connue → enforcer connue → runtime connue, sans confondre leurs espaces de noms. RED ciblés puis fichier complet12/12 et build TypeScript/Vite ; ces tests n'exécutent pas le DOM détaillé et ne qualifient ni navigateur ni livraison via dashboard. [Gels et limites](evidence/2026-10-31/broker-runtime/README.md#complément-final--étapes-natives).

H10/S3/S4 : un échec local avant Created/Arm worker a pu rester invisible au filtre de diagnostic public ; le global600s ne prouve pas son instant ou sa cause. Pour ce correctif, régression réelle ciblée passant par `Execution.failure`, vrai Gateway/SQLite/Journal/Writer, lecture API v1 viewer et correspondance Event.id/audit_seq ACK avec l'outbox ; canari privé absent et gardien IPC vide arrêté. RED attendu sur V4 absent, puis GREEN après rapport fermé de repli. Un deuxième RED/GREEN vérifie les cinq nouveaux labels du marqueur ; les trois unités diagnostics et les quatre parseurs couvrent privacy, provenance connue et rejets sans rejouer les vingt tests du précédent lot. Aucun moteur/modèle/frontend supplémentaire. Le nettoyage de cache concomitant n'est pas une faute injectée contrôlée ; ne pas l'utiliser pour qualifier panne/reprise ou inférer la cause H10. [Preuves et limites](evidence/2026-10-31/broker-runtime/README.md#h10--échec-local-durable-sans-diagnostic-public).

Évolution H8/H9/S1 du31octobre : supprimer la coupure préRunning120s après H8, puis porter le budget global240→600s à la demande du mainteneur. Cela laisse plus de marge au harnais, sans toucher à l'Arm worker5s ou aux contrôles de faute/arrêt. H9 observe le diagnostic préclaim par le vrai endpoint filtré ; son marqueur ne conserve que huit champs validés. Les trois tests purs du lecteur couvrent champs privés, page vide et métadonnées invalides, sans prétendre qualifier HTTP ou le runtime. S1 ajoute des tests sur vrais Journal/Writer, conflit CAS avec Cancel/outbox, gateway/SQLite pour Decision puis replay sans second HTTP, perte de binding refusée, et snapshot helper distinct du sample local. L'IPC natif de ce test reste vide et aucun moteur n'est invoqué. Les tests ignorés moteur et le helper enfant ne sont pas comptés comme nouvelles réussites. [Gels, commandes et limites](evidence/2026-10-31/broker-runtime/README.md). La prochaine tentative réelle attend le gel logiciel/revue ; D5 et G09 restent ouverts.

La cible ignorée `tests/broker_runtime.rs` utilise un vrai gateway schéma3 neuf, le broker/gardien natifs et un moteur explicitement sélectionné. Elle n'appelle aucun fournisseur modèle. Les tentatives Docker Desktop ARM64 H4–H7 du31octobre échouent avant Running ; elles ne comptent ni comme faute au point prévu ni comme preuve de délai autonome. H6 révèle une borne d'observation trop courte pendant la préparation : elle passe à120s/global240s, sans changer les5s de l'Arm worker ou ses oracles. Une expiration d'observation ne prouve pas un refus du produit. H7 localise un refus préclaim que D3 ne diagnostiquait pas encore. Le probe `inspect_retained_guard_after_runtime_failure` lit un gardien déjà présent, sans start/cancel et sans remplacer son processus. Son PASS ne qualifie pas la supervision d'un worker. Les commandes opt-in, ressources conservées, revues et logiciels ciblés du diagnostic sont dans les [preuves du lot](evidence/2026-10-31/broker-runtime/README.md). Un prochain essai doit exploiter le diagnostic complété avant toute répétition ; aucune augmentation de durée d'exécution ni nouveau claim pour contourner un refus. D5/disarm ouvert interdit de présenter ces ressources retenues comme un cycle de nettoyage terminé.

Avec Rust `1.99.1`, fixé dans `rust-toolchain.toml` :

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
CARGO_INCREMENTAL=0 cargo test --locked
cargo build --release --locked
```

Avec Node 24 dans `front/` :

```sh
npm ci
npm test
npm run build
npm run test:browser
```

Consigner les versions réellement utilisées. Le navigateur exige Playwright et Chromium. Un changement Rust de frontière de sécurité conserve un test négatif : identité fausse, lecture/écriture d'un autre tenant, panne d'audit, replay divergent ou substitution de chemin selon le périmètre.

Conformément à la préférence du mainteneur, confier l'exécution des tests aux agents Luna et sélectionner les cibles selon le changement ; ne pas relancer toute la matrice E2E à chaque lot. Les gates obligatoires restent à qualifier. Les [preuves de journalisation moteurv2](evidence/2026-10-30/engine-logs-v2/README.md) illustrent cette sélection : contrats/reprises ciblés, un scénario Docker503 et un run normal sur le candidat, sans nouveau navigateur puisque le frontend est inchangé. Le [vrai Codex batch](evidence/2026-10-30/provider-cli/README.md) garde son FAIL fonctionnel initial ; un exit0 fournisseur sans effet attendu n'est pas un PASS.

`api_auth_boundary` couvre l'ordre identité/rôle avant lecture des corps et extraction des paramètres, les deux alias, l'absence de mutation d'audit, la borne/ambiguïté Bearer et un vrai gateway enfant HTTP. Le client TCP annonce un corps de 1024 octets sans l'envoyer et exige un premier statut401 avant expiration du délai ; ce n'est pas un test de drainage ou de keep-alive. Le harnais Rust possède son child jusqu'à la fin (`kill_on_drop`), sans gateway de fond dépendant d'une session d'outil. Les [preuves RED/GREEN et régressions](evidence/2026-10-30/api-auth/README.md) restent distinctes d'une qualification de charge ou de runtime ; aucun moteur/modèle appelé.

`supervision_api` exerce les neuf routes v1 sur le vrai routeur et des bases SQLite privées A/B : guards avant body, protocole tenant/rôle/namespace, admission/armements/helpers/claims/export, liens au parent réel et maintenance après réouverture. Une sonde TCP loopback comptée vérifie le non-appel de Jev pour rejeu exact et admission fraîche inactive, après contrôle positif avec le vrai client ; aucun modèle ni contournement TLS. `broker_clock` vérifie calculs/UUID et échantillons natifs du macOS courant, dont un enfant Rust du même exécutable via pipe privé. Son entrée enfant ignorée est invoquée par le test parent et ne compte pas comme test produit supplémentaire. [Commandes, gels et limites](evidence/2026-10-30/supervision-api-clock/README.md) ; Linux natif, reboot/suspend et consommateurs d'échéances restent à qualifier séparément.

Le [complément clock Linux ARM64](evidence/2026-10-30/broker-clock-linux/README.md) exécute une fois cette même cible sur le noyau de la VM Docker Desktop, depuis l'archive figée du lot, sans réseau ni montage : cinq PASS, helper non recompté, sources recopiées et revue de preuve. Cette preuve de producteur sous Linux en conteneur ne vaut pas qualification d'un hôte Linux natif/x86_64, de Podman, des workers ou de l'ordonnanceur. Aucun besoin de rejouer cette cible sur chaque modification pure du format d'intention.

`broker_intent` est une cible pure sans moteur, modèle ou IO : projection stable du vrai RunPlan, golden indépendant de bytes/hash, choix fournisseur et prompt cohérents, refus de métadonnées invalides et diagnostics opaques. Le test de bornes utilise des JSON valides paddés pour les limites d'entrée et un prompt non-NUL valide pour dépasser uniquement la limite canonique ; il exige les erreurs typées attendues. [Six fonctions finales et canaris ciblés](evidence/2026-10-30/broker-intent/README.md), avec RED/GREEN de l'écart d'opacité trouvé en revue. Un parsing réussi ne prouve aucun fichier de credential présent, aucune autorisation tenant ou durabilité de journal.

Le [journal privé du broker](evidence/2026-10-30/broker-journal/README.md) se vérifie sans moteur ou modèle avec `cargo test --locked --test broker_journal`, `cargo test --locked --lib broker::journal::tests` et `cargo test --locked --test journal_sqlite`. Les hooks sont uniquement cfg(test), après les vraies opérations ; leurs deux helpers enfants ignorés sont explicitement lancés par les tests parents, sans double comptage. Le probe SQLite garde Journal et Store dans le même PID et observe les locks depuis des enfants indépendants. La preuve Linux en VM est séparée de macOS et des runtimes workers. Le [complément contexte/plancher](evidence/2026-10-30/journal-context/README.md) couvre le batch/generic/None avec Store réel et snapshots synthétiques, corrige l'identité du handle selon le vrai producteur, et distingue le refus de rollback vivant de la lecture permise après reopen. Autres modes et vrais runtimes restent à qualifier ; SIGKILL et caps applicatifs ne prouvent ni panne électrique ni quotas physiques.

`supervision_bootstrap` vérifie la factory exclusivement neuve et le vrai gateway A/B : sauvegarde2, origine3 active, reopen inactif, erreur post-copie, racines de credentials séparées physiquement, défaut2 et refus de réutilisation. Le probe SQLite enfant intervient avant et immédiatement après le refus dans le PID propriétaire, sans requête parent intermédiaire ; les gateways sont possédés par RAII. [Sept fonctions et canaris associés](evidence/2026-10-30/supervision-bootstrap/README.md). Un complément Docker Desktop/LinuxARM64 compare le parent `/run/secrets` non préparé et préparé, avec mêmes binaires release/probe, projections readonly et DATA neuf ; il qualifie ce contrôle conteneurisé, pas des workers ou la chaîne multi-stage complète. Aucune matrice fournisseur ni nouveau navigateur pour ce lot.

`broker_checkpoint` exerce le format V2 sur de vrais fichiers Journal et le vrai Store SQLite : compatibilité rev0 V1 ancrée au binaire antérieur, CAS/replay historique/plancher partagé, promotion refusée après un ancien managed, admission pending/refusée et graphe des neuf variantes avec Import/ImportVerification/FinalScan/Export. Les faits runtime et horloges sont synthétiques, les reçus Store réels. [Commandes et six fonctions finales](evidence/2026-10-30/broker-checkpoint-v2/README.md). Le graphe est coûteux en debug ; pour ce chantier, cible entière exécutée avec `cargo test --release --locked --test broker_checkpoint` (6PASS,17,59s de test sur macOSARM64), cinq fonctions rapides aussi en debug avec `-- --skip sqlite_store_receipts_cover_all_supervision_operation_variants`. Le filtré ne compte pas PASS et le graphe reste actif par défaut. Les 26canaris pertinents restent distincts, sans compter en plus les helpers ignorés ou les rejeux. Ni IPC/enforcer, ni crash V2 exhaustif/panne électrique, ni autres plateformes ou capacité100 ne sont qualifiés par cette cible.

`broker_submission` vérifie les coutures de préparation/reconstruction du vrai Journal, avec bindings None/manuel/fournisseur, approbation conservée et sources/references legacy retirées au replay. Les assertions portent sur les bytes/SHA du handoff, sa survie après drop des propriétaires initiaux, son altération et les diagnostics opaques. Aucun modèle ou API fournisseur. `runtime_prearmed` exerce le port batch sur des programmes CLI Rust explicitement sélectionnés : garde consommé après autorité, refus/ObserveOnly, reprise possédée, callback en attente face au signal et EOF sans nouvelle invocation. Le signal au processus concerne un child propriétaire du test ; aucun signal au terminal utilisateur. [Preuves, RED/GREEN RPT-01 et limites](evidence/2026-10-31/broker-submission-prearmed/README.md). Le drainage technique10s explique une partie de la durée de la cible ; ces fixtures ne prouvent ni enforcer effectif, ni Docker/Podman, ni échéance conservée après SIGKILL CLI. Aucun lancement de la matrice E2E globale pour ces coutures sans frontend ni déploiement modifié.

Le lot [lifecycle V3 et admission HTTP du 31 octobre](evidence/2026-10-31/broker-lifecycle/README.md) vérifie rejets/replays/partiels/planchers, commandes locales hashées et causales, status/list hors page et compatibilité V1/V2/V3. `broker_lifecycle` (source `a3716314…`, 6/6 release) et le graphe Store `broker_checkpoint` (source `9cb7aed9…`, 6/6 release, 32,23s) couvrent notamment l'approbation en deux commits, plusieurs cancels/grants Export et les faits tardifs. Floor 2/2, client 4/4 sur vrais Axum/SQLite3, journal 12/12 et cinq parents unitaires complètent 35 fonctions uniques ciblées ; contrat API, diagnostics et self-test inclus, deux helpers enfants ignorés non additionnés. BLC-01 à04 ont leurs RED/GREEN ; fmt/Clippy all-targets strict/release PASS. Le test distingue CancelRequested de la transaction Store CancelReserved qui libère la quota centrale avant l'ACK local : une réponse perdue peut laisser un pending local avec quota déjà libérée. Une failure locale ne prouve aucun terminal ; Preparing sans CID et cible jamais démarrée sans terminal admissible peuvent rester NeedsReconciliation/quota tenue. Aucun frontend/moteur/modèle/E2E/cloud exécuté ; service/IPC/enforcer et cutover restent à qualifier. G09 FAIL, 0/12 gates fermés ; QA/schéma2/`billable=false` inchangés.

### Service permanent et reprise locale

Le [checkpoint du service permanent](evidence/2026-10-31/broker-service/README.md) documente 40 fonctions ciblées : IPC et queue réels, vrai child gardien vide gelé/repris, états/identités du service, capacités FD source/export, outbox500, reader STOP, bindings et canaris du journal. Le graphe Store/checkpoint et l'outbox sont exécutés en release ; les quatre capacités workspace sont exécutées en debug. Les premiers échecs de fixtures et la correction d'un manifeste auto-référent restent consignés. Les deux helpers unitaires ignorés ne sont pas comptés en plus des parents. Ces preuves ne couvrent ni bootstrap parent, ni slots moteur non vides, ni service complet authentifié A/B, ni deadlines de workers après panne. Après corrections Clippy mécaniques, seuls IPC/service/outbox ont été rejoués. Aucun fournisseur, frontend ou E2E général pour ce checkpoint ; G09 reste FAIL.

Le [lot publication d'export](evidence/2026-10-31/export-publication/README.md) complète cette référence : cinq tests `workspace::export_durability_tests`, onze `workspace::authorized::authorized_workspace_tests` et le nouveau bootstrap public exact passent sur le même gel. Le filtre erroné sélectionnant zéro test est conservé et n'est pas compté PASS. Tests sur filesystem réel, hooks uniquement après les vraies opérations ; ordre fichiers/modes/répertoires/rename/parent, fautes avant/après publication, destination préservée au retry et politique UID/modes/ACL sont vérifiés dans cette portée. Fmt/Clippy strict/release passent. Aucun E2E général, moteur, modèle ou frontend rejoué ; panne électrique, vrai EIO/ENOSPC et autres UID/plateformes restent NOT_RUN. Ce bootstrap natif vide ne qualifie pas les deadlines de workers.

Le [lot Cancel exact et livraison STOP](evidence/2026-10-31/cancel-retry/README.md) ajoute une régression handler sur vrai Journal/Writer et une preuve child/IPC avec acteurs volontairement NotReady. Le ciblage C1/C2, l'ACK tardif d'une ancienne Arm, le plancher live et la perte d'un blob manuel sont exercés par deux fonctions exactes `broker_checkpoint` en release, avec Store SQLite réel et observations runtime synthétiques. 11 fonctions service,1 parent natif et2 checkpoint constituent14 fonctions uniques sur les gels documentés ; le helper enfant ignoré n'est pas ajouté. Les canaris inchangés ne sont pas relancés après correction de la seule fixture manuelle ; manifeste et diff relient les gels. Fmt/Clippy strict/release PASS. L'ACK prouve la livraison à des cibles connues, pas un terminal moteur ; premier Cancel sans blob et writer strict absent restent ouverts.

Le [lot acteur STOP restreint](evidence/2026-10-31/stop-only-actor/README.md) complète les deux précédents : union33 fonctions, dont18 service sur deux gels reliés (auth3 finale),7 parents journal avec2 helpers ignorés non ajoutés,3 writer,3 intégrations writer,1 aide CLI et1 canari V4 en release. Filesystem/Journal/queue réels ; HTTP/Axum/SQLite avec deux tenants et vraie connexion Unix pour refus vide, sans moteur. Les deux défauts RED, les refus de chemins de fixtures macOS, le SIGABRT de matrice debug et l'appel de helper privé en compilation sont conservés. Boxing local du test, pas de pile globale augmentée ; les autres chemins de connexion du produit restent à qualifier. Fmt/Clippy strict/release PASS. Le delta final est limité à trois fichiers de tests ; les15 autres service et7 journal inchangés ne sont pas relancés. Aucun fournisseur, frontend, navigateur ou E2E général. La première convergence réelle sur Docker et D5 demeurent prioritaires, G09 FAIL.

### Invocation et mises à jour du moteur

Les intégrations `runtime_managed`, `runtime_program` et `runner_recovery` couvrent le choix du programme hors PATH ambiant, l'environnement reconstruit, l'ancrage du répertoire de CLI, le refus des ACL non qualifiées et des fichiers non réguliers/surdimensionnés, ainsi que l'adoption explicite d'une ancienne invocation. Les tests de migration doivent garder le même moteur/worker/CID et les claims consommés, réessayer le même événement d'adoption après un refus d'audit et ne créer aucun événement supplémentaire pour une adoption déjà acquise. Le changement de contexte ou d'ID Docker entre diagnostic et effet est testé séparément pour les commandes opérateur.

Les programmes de substitution de ces tests sont des fixtures Rust natives explicitement sélectionnées, avec métadonnées privées ; ils ne qualifient ni Docker ni Podman. Pour les trois essais réels ci-dessous, `AGENTSANDBOX_TEST_RUNTIME_PROGRAM` permet de choisir explicitement le programme absolu de l'installation à qualifier. Son absence utilise les emplacements connus du produit ; le PATH du harness ne constitue pas une approbation. Consigner programme/politique réellement testés et conserver une preuve distincte pour la reprise d'une session antérieure.

## Qualification par runtime

Docker et Podman, Linux x86_64 et macOS ARM64 exigent quatre rapports distincts. Les trois essais batch ci-dessous et les cas moteurs interactifs sont ignorés par défaut ; leur absence d'exécution vaut `NOT_RUN`, jamais `PASS`.

| Cible Cargo | Prérequis et portée |
|---|---|
| `runtime_smoke` | Image worker et pile de contrôle démarrée, configuration et endpoints QA explicites ; exécution/refus, copie et export selon [../operations.md](../operations.md). |
| `runtime_confinement` | Image worker et moteur explicites ; configuration inspectée, effets de confinement ciblés, timeout, copie/export. Ne qualifie pas les contrats multi-client. |
| `runtime_terminal_persistence` | Image worker et moteur explicites ; crée sa configuration privée et un vrai gateway SQLite loopback. Un middleware refuse uniquement l'accusé terminal HTTP : vérifie conservation du conteneur, cohérence de l'observation, absence de faux terminal et intégrité après réouverture. Ce n'est pas une panne disque ni la réconciliation du produit. |
| `runtime_interactive` | Qualification ciblée Docker Desktop/macOS ARM64 avec image probe Rust : clavier/TTY/resize/exit7, doubleCtrlX et octets raw exacts CR/LF/Ctrl-C/UTF-8/flèche. PTY physique privé, driver maintenu jusqu'à vérification, aucune clé ni modèle. Trois cas réels PASS ; CtrlX rejoué une fois après revue avec motif opérateur durable exact, absence de timeout et délai propre <30s. Résultats, limites et FAIL préalables conservés dans le [dossier](evidence/2026-10-30/interactive-cli/README.md). Entrée `interactive_host_driver` interne ignorée, non comptée comme test produit ; `local_pty_host_driver_help_smoke` qualifie seulement le harnais sans moteur. |

Exemple de lancement ciblé (image déjà construite, aucun credential imprimé) :

```sh
AGENTSANDBOX_TEST_RUNTIME=docker \
AGENTSANDBOX_TEST_IMAGE=mon-image-worker:test \
cargo test --locked --test runtime_terminal_persistence -- --ignored --nocapture
```

Ajouter `AGENTSANDBOX_TEST_ALLOW_ROOTFUL=1` uniquement pour une exception rootful explicitement choisie. Les répertoires de preuve, credentials QA privés, volumes et instances diagnostiques créés par le test terminal restent en place. Les fixtures CLI Rust des tests logiciels d'erreur ne qualifient aucun moteur.

La recette obligatoire à trois workers doit couvrir A1/A2 chez A et B1 chez B : démarrage/arrêt, données séparées, grant A1→A2, refus A1→B1, révocation, quotas, audit persistant, usage idempotent, redémarrage contrôle, arrêt brutal, dashboard et refus d'accès aux secrets/privilèges du contrôle. Ce scénario n'existe pas encore dans le code initial. La charge 100 workers est un essai séparé sur de vrais workers.

Kubernetes exige un adaptateur effectif, un cluster autorisé, un CNI appliquant les politiques et des preuves RBAC/identités/volumes/réseau/reprise. Aucune simulation ne qualifie cette plateforme. Les comptes fournisseurs et services distants exigent autorisation et versions réelles.
