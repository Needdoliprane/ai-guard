# Gates de qualification production

## 17 novembre — D7 : preuve logicielle supplémentaire

La [contribution D7](evidence/2026-11-17/d7-native-stop-owners/README.fr.md) intègre le code issu de `6a32771`, avec 16 tests lib ciblés PASS sur son gel et Clippy RC0 **non strict** (561 émissions). Elle ne clôture aucun gate : reprise froide/retirement, quota, STOP physique, Docker/Podman/Kubernetes et dimensionnement restent ouverts. Les résultats de sources antérieures restent séparés. **0/12 gates clos** ; aucune release qualifiée.

## 17 novembre — relevé runtime et finalisation tardive D1

[Douze cas logiciels ciblés](evidence/2026-11-17/runtime-observation-and-d1-late-finalizer/README.fr.md) passent sur le gel `c755fb52…db3b9`, dont quatre vrais CLI/Gateway/SQLite avec moteur Rust de fixture : terminal unique, faute Managed tardive et quota/instance conservés pour Created/Unknown. Le relevé facultatif est borné, sans attente dans STOP, et ne remplace pas l’audit durable. Le précédent FAIL de sonde reste historique ; la régression lit désormais la capture dans son véritable propriétaire d’erreur. Compilation des trois tests et six listes vérifiées ; Clippy toutes cibles de cette révision RC0 non strict, 528 émissions d’avertissement, 0 erreur ; 21 émissions supplémentaires restent suivies. Ni moteur réel, reprise froide, fournisseur, scénario A1/A2/B1 complet ni capacité 100 ne sont qualifiés. **0/12 gates clos**.

## 16 novembre — D1 : quatorze PASS logiciels, aucune qualification globale

Le [lot D1](evidence/2026-11-16/control-monitor-prestart-fix/README.md) corrige l’admission préstart au premier effet et conserve le propriétaire, les causes/captures et le terminal natif connu. **14 PASS** vérifient quatre vrais CLI ai-guard/Gateway/SQLite avec moteur Rust de fixture et dix unités/voisins Managed, PTY et deadline sur leur source exacte. La fenêtre post-ACK reste 60 s ; aucun réglage de sûreté n’a été réduit. Les deux fichiers D7 cfg(test) de la composition sont exclus du commit D1.

Clippy D1 toutes cibles sur la source composée `74d60de1…b7b` : **RC0 non strict, 507 émissions d’avertissement et 0 erreur**. Reçu `dc43ad33…0e0a2` ; gate strict ouvert. Faute tardive exacte jusqu’au Store/quota, vrai TTY interactif, abandon/reprise froide, Docker/Podman, fournisseurs et cluster restent non qualifiés. Le verdict primaire D1 n’est pas réécrit ; D7 reste ouvert : le premier setup avant oracle est préservé ; après correction de la seule configuration TMPDIR, cap2 PASS et cap1 RED au vrai oracle de couverture sont vérifiés, sans correctif produit D7 ni qualification runtime. **0/12 gates G01–G12 clos**, G09 conserve son FAIL ; aucune release ni qualification production n’est déduite de ces tests.

## 16 novembre — R2 runtime/Kubernetes : preuve logicielle ciblée, gates ouverts

La [composition publique R2](evidence/2026-11-16/runtime-kubernetes-batch-composition/README.md) passe format, Clippy toutes cibles **non strict (505 diagnostics d'avertissement)**, build et **26 cas exacts**, vérifiés sur ses sources et binaires propres. R1 et son Clippy FAIL restent historiques ; les PASS standalone ne sont pas transférés.

**0/12 gates G01–G12 clos ; aucune activation de production.** Aucun nouveau moteur, délai physique, Native17/S5, cluster ou capacité100 qualifié. MetadataFD reste FAIL avec quatre dépendants NOT_RUN ; l'audit D1 reste ouvert.

## 16 novembre — Native17 R6 reste non qualifié

Le contrôle R6 du démarrage configuré échoue ; la réussite du build ne qualifie pas le comportement et la rétention durable n’est pas établie. Aucun gate G01–G12 n’est clos (**0/12**). [Preuve R6 et périmètre](evidence/2026-11-16/startup-runtime-failures/README.md#native17-r6-one-configured-startup-oracle-remains-failing).


## 16 novembre — startup Native17 toujours non qualifié

La [preuve R11](evidence/2026-11-16/startup-runtime-failures/README.md#successors-r10-and-r11-software-results-remain-separate) ajoute 18 régressions réussies mais conserve un refus du démarrage configuré et six cas non exécutés. Les 13 cas DATA/FS ne qualifient pas la construction complète. Un nœud Kubernetes Ready ne qualifie pas les workers ni leurs frontières. **0/12 gates clos.**

## 16 novembre — contribution logicielle aux bornes HTTP

Le [lot des listeners](evidence/2026-11-16/http-listener-bounds/README.md) apporte 11 régressions réussies et ses guides EN/FR. G08 reste ouvert : seuils 128/15s non dimensionnés, streams HTTP/2 suivants et disponibilité déployée non qualifiés ; aucune clôture automatique des constats NV-07/V4. **0/12 gates clos.**

## 16 novembre — qualifications startup/runtime encore en échec

Le [lot de mesures](evidence/2026-11-16/startup-runtime-failures/README.md) laisse le démarrage Native17 et le cas Docker générique en échec. Le refus post-COMMIT et le délai d’arrêt exigent leurs corrections et régressions. Les observations positives de profil/nettoyage ne qualifient pas le cas complet ; export, recette A1/A2/B1 et autres moteurs restent ouverts. **0/12 gates clos.**

## 16 novembre — résilience locale du callback OIDC

Le [lot NV-09](evidence/2026-11-16/oidc-callback-correlation/README.md) vérifie corrélation, bornes et continuité d’une tentative CLI sur macOS. G04 et les autres gates restent ouverts : le SSO réel, Native17, les autres principals et plateformes ne sont pas qualifiés par ces 14 cas.

## 16 novembre — contribution logicielle aux contrôles runtime

Le [lot image locale](evidence/2026-11-16/runtime-local-image-policy/README.md) apporte des régressions du builder. Il ne qualifie pas le moteur, les sorties réseau ou la chaîne de livraison et ne clôture aucun gate G01–G12.

## 16 novembre — lot logiciel Kubernetes dédié intégré

Le [lot lifecycle/attach isolé](evidence/2026-11-16/kubernetes-dedicated-integration/README.md) passe son check toutes cibles et son build libtest, puis **17/17 tests logiciels exacts** sur `1d274954`. Après correction du formatage seul (`cb1ca7f`), le contrôle de format des 18 fichiers Rust et un nouveau check toutes cibles passent ; les 17 tests ne sont pas réexécutés. La revue indépendante ne signale aucun blocage confirmé dans ce périmètre pré-effet. Seuls les 21 chemins dédiés sont repris, avec la garde canonique de replay conservée. Native17/S5, le vrai cluster et la qualification complète restent ouverts : **0/12 gates clos**.

## 16 novembre — tests natifs exécutés et raccords en correction

Le [build natif macOS R16](evidence/2026-11-16/session-control/TARGETED_NATIVE_BUILD_R14_R16.md#native-macos-r16-successor--actually-executed) réussit, puis les 12 tests ciblés donnent **9 diagnostics PASS et 3 sauvegardes FAIL**, sans timeout ni relance. Les trois refus surviennent avant l’oracle de sauvegarde : `human_authority_epoch_expired` au démarrage ; leur cause reste à corriger ou expliquer. La composition suivante Config/E2, revue puis appliquée localement, échoue à son check toutes cibles avec **40 erreurs de compilation** ; ses 55 nouveaux tests sont NOT_RUN. Les preuves restent liées à chaque source. **0/12 gates clos ; aucun pourcentage fonctionnel fiable.**

## 16 novembre — compilation native R13

La [compilation R13](evidence/2026-11-15/session-control/COMPOSED_FULL_NATIVE_CHECK_R9.md#r11r13-successor-checks-2026-11-1516) est **PASS** sur son gel exact, avec 2 065 avertissements. Les tests de comportement, parcours sessions, OIDC/configuration/reprise, runtime et fournisseurs restent à prouver. Cette étape ne ferme aucun gate : **0/12 G01–G12 clos**.

## 15 novembre — compilation du successeur natif R10

Le [successeur corrigé R10](evidence/2026-11-15/session-control/COMPOSED_FULL_NATIVE_CHECK_R9.md#r10-successor-check-2026-11-15), 3 307 fichiers, a subi un nouveau check toutes cibles : **FAIL, RC 101, 7 erreurs codées**, 2 notes de résumé sans code et 60 avertissements. Une seule invocation Cargo a été exécutée ; aucun test. Collecte, gardes, source/cache et fermeture du CID exact sont vérifiées, sans hausse des compteurs OOM.

La compilation reste **FAIL** sur le seul gel `1c75fb67…`. Les postchecks et la fermeture réussis qualifient la collecte de cette campagne, sans clôturer une exigence produit. Les tests, le formatage, Clippy, les parcours natifs, la reprise et les moteurs/fournisseurs réels restent non qualifiés par R10. **0/12 gates G01–G12 clos** ; aucun pourcentage global et aucune qualification production n’en découlent.

## 15 novembre — compilation du noyau natif complet R9

Le [gel complet réellement appliqué R9](evidence/2026-11-15/session-control/COMPOSED_FULL_NATIVE_CHECK_R9.md), 3 291 fichiers, a été compilé une fois : **FAIL, RC 101, 64 erreurs codées**, 2 résumés distincts et 62 avertissements. Il expose des dépendances et raccords encore incomplets, sur un périmètre plus large que R3. Collecte, source/cache et fermeture sont vérifiées, sans OOM observée. Aucun test exécuté par cette campagne.

Trois lots corrigent séparément configuration, runtime/conversation et audit. Le correctif relu des trois agrégations d’authentification est appliqué après le gel R9, avec échéance inchangée et propriétaires retenus ; sa compilation et ses quatre régressions restent NOT_RUN. Les parcours natifs, OIDC, reprise réseau et configuration restent à terminer et qualifier. **0/12 gates production clos** ; pas de pourcentage fonctionnel fiable.

## 15 novembre — preuves ciblées et intégration native en cours

Le [frontend natif de nouvelle autorisation](evidence/2026-11-15/session-control/FRONTEND_NATIVE17_QA_R1.md) a réellement passé 7 nouvelles régressions et 46 canaris : **53 PASS, 0 FAIL**, build TypeScript/Vite PASS. Navigateur et parcours gateway/broker/fournisseur restent NOT_RUN pour ce delta.

Deux contrôles Rust distincts restent **FAIL** : le [baseline réparé R2](evidence/2026-11-15/session-control/COMPOSED_BASELINE_CHECK_R2.md), avec 4 erreurs codées, et le [candidat courant R3](evidence/2026-11-15/session-control/COMPOSED_LIVE_CHECK_R3.md), avec 11 erreurs de raccordement sur un périmètre plus large. Les correctifs de tailles/financement et d’API sont relus et appliqués après ce dernier gel, sans crédit de recompilation. Collecte, source/cache et fermeture des QA sont vérifiées ; aucune OOM observée lors de ces deux checks.

Le [lot Kubernetes lifecycle/attach](evidence/2026-11-15/session-control/KUBERNETES_LIFECYCLE_QA_R4_R5.md) a réellement passé son check toutes cibles et 16 de ses 17 cas logiciels sur le premier gel ; le cas restant échouait dans sa fixture. Après correction de cette seule fixture, le retest ciblé du successeur passe **2/2**, dont un canari inter-tenant. Les 16 PASS historiques ne sont pas transférés comme une suite réexécutée. Formatage et Clippy restent NOT_RUN, ainsi que tout essai cluster. Le noyau natif, le contrôle réseau et les parcours de session restent à composer puis vérifier. La révocation du sender après autorisation avant émission reste un écart explicite du contrat. **0/12 gates production clos** ; aucun pourcentage fonctionnel fiable n’est établi. Ce commit documentaire ne qualifie ni les candidats privés ni la branche publiée.

Fédération C3 composée du 14 novembre : 44 parents logiciels uniques PASS sur
`f117ceae016049b7274708197c3702218148e56b`, compilation fraîche, 14 listes
réelles et check toutes cibles RC0. Rotation/réconciliation, floors/journal
partagés, proxy/routes, page humaine Store14 réellement auditée avec refus,
canaris legacy et neuf B9 chauds sont testés dans ce seul run.
[Preuve bornée et historiques](evidence/2026-11-14/federation-composition/README.md).
Les 2876 sources restent exactes après run ; fixtures, OOM0, fermeture du CID
exact et flush séparé sont vérifiés. 50 postimages de format lié (49 historiques + 1 fixture)
et helper check courant ; 34/168 warnings lib/check conservés, Clippy strict
NOT_RUN. R1 compile 101/E0308 et R2 9 PASS/2 FAIL/33 NOT_RUN restent inchangés.
Le positif humain ne qualifie pas Store15/16. D5/fencing global, reprise froide,
Source16/E2 distinct, multi-hôtes, réseau et workers/moteurs/fournisseurs/cluster
réels, navigateur, capacité 100 et suite complète restent hors preuve.
Aucune nouvelle UI ou version 0.0.1 qualifiée ; `f117` reste le gel 0.1.0 testé.
G09 demeure FAIL ; aucun gate G01–G12 n'est clos (0/12).

Broker Submitted/Pending du 14 novembre : neuf parents logiciels uniques PASS
sur `a4340c4f`, format courant des trois sources, compilation et check toutes
cibles RC0, 169 avertissements hors des trois fichiers modifiés. Source après
exécution, fixtures, fermeture exacte/OOM0 et flush Root vérifiés ;
[preuve bornée](evidence/2026-11-14/broker-submitted-pending/README.md).
L'original retenu converge de Pending+Q vers Final puis retrait local sans
nouveau start/claim ni retransmission de RetireUnused. Le RED initial,
l'abort134 de R1 et la sonde de pile restent
leurs résultats datés ; la variation test-only qui rétablit le canari ne
démontre pas une correction de pile du produit. Clippy strict, Unsubmitted
audité, reprise froide complète, D5/fencing, familles namespace/restauration
et NativeResume, moteurs et capacité100 restent ouverts. Aucun terminal
worker ou quota libéré n'en découle ; G09 FAIL, 0/12 gates globaux clos.

Diagnostic opérateur E du 12 novembre : neuf parités binaires et 85 cas CLI
normaux PASS sur le gel composé `c2b907a8…`, check toutes cibles RC 0,
export vivant exact et fermeture RC 0/OOM 0. L’exporteur manuel ignoré reste
`NOT_RUN` dans cette campagne ; sa régénération séparée et la promotion des
neuf exemples ont leurs propres preuves. [Sources et limites](evidence/2026-11-12/operator-diagnostics/README.md).
Ce diagnostic passif, avec découverte OIDC explicitement demandée testée sur
loopback seulement, améliore les procédures opérateur sans qualifier leur
exécution réelle : moteurs, fournisseurs, IdP réel, macOS natif, image/release,
Clippy strict et suite complète restent hors de ce lot. Aucun gate G01–G12
fermé ; G09 conserve son FAIL et G12 reste BLOCKED.

Récupération centrale S2 du 9 novembre : 50 parents ciblés PASS sur leurs deux
gels et check toutes cibles RC0, revues traitées et sources composées vérifiées.
[Preuves et limites](evidence/2026-11-08/gateway-session-recovery/README.md#targeted-green-and-integration).
Ce raccord avant effet n'active ni Pod, bootstrap, worker, transport ou reprise
physique. Warnings/Clippy, D5, moteurs et autres qualifications restent ouverts.
G09 FAIL, aucun des 12 gates globaux fermé.

Profil descriptif Kubernetes V2 du8novembre : `c4d0cd6` composé dans `9af5e99`,
onze fonctions et check toutes cibles PASS sur k2, revue traitée.
[Preuves et portée](evidence/2026-11-08/kubernetes-session-profile-v2/README.md).
Activation, bootstrap, identité/stockage réels, lifecycle et Clippy k2 restent
ouverts ; aucun gate global fermé, G09 FAIL. Le RED préclaim98 a aussi révélé
une libération de lease avant la fin de l'audit : correctif et GREEN requis.

Réserve d'audit du7novembre : `1bb3bba` composé dans `d550c34`,19 fonctions
uniques et toutes cibles vérifiées, revue indépendante. [Preuves](evidence/2026-11-07/d5-control-audit-reserve/README.md).
Clippy strict est désormais exécuté : **FAIL89 diagnostics**, sans analyse
réussie des cibles suivantes ou suppression de lint. G12 demeure ouvert.
Les diagnostics V1/V2 et les deux consumers D5 passent dans une composition
séparée, mais Export n'a pas encore atteint son oracle. Owner/exec du relais
restent des composants privés à raccorder ; aucun transport runtime qualifié.
G09 FAIL,0/12 gates globaux clos.

Export du 7 novembre : filtrage renforcé, modes observés et compatibilité fermée V1
intégrés après RED et revue. 49 fonctions ciblées passent sur leurs gels et toutes
cibles du gel 03 compilent ; [preuves](evidence/2026-11-07/workspace-export/README.md).
Le harnais de reprise corrigé ne change aucun délai produit. APFS, moteurs réels,
migration de processus et pannes physiques restent NOT_RUN dans ce lot. Le contenu
exporté reste non fiable et les avertissements/Clippy ouverts. G09 FAIL, 0/12 gates clos.

Reprise FD du6novembre : dix fonctions Store ciblées, compilation et revues
passent ; [preuves](evidence/2026-11-06/store-resource-recovery/README.md).
Ce résultat ne qualifie ni disponibilité TCP sous pression, ni workers/moteurs,
ni autres plateformes. G06/G08 restent ouverts, G09 FAIL,0/12 gates clos.

Reprise de présence du6novembre : même acteur/Pending après panne temporaire,
dix-sept fonctions et check lot/composition toutes cibles PASS, revue traitée.
[Preuves](evidence/2026-11-06/presence-recovery/README.md). Cette disponibilité
de déclaration ne qualifie ni fédération physique, worker, runtime ou fencing.
Observation de coût ignorée NOT_RUN, G12 warnings/Clippy ouverts ; G09 FAIL,
0/12 gates globaux clos.

Confiance gateway du CLI humain du6novembre : option CA publique explicite,
snapshot local/OIDC immuable, reprise export liée au bundle, dix fonctions
ciblées et check toutes cibles PASS, revue traitée. Les
[preuves](evidence/2026-11-06/human-gateway-trust/README.md) restent logicielles
sur TLS/SQLite locaux. Aucune qualification G08/G11 d'IdP réel, navigateur,
PKI entreprise ou transport multi-hôtes ; G12 conserve avertissements et
Clippy absent, G09 FAIL et0/12 gates globaux clos.

Confirmation de quarantaine du 6 novembre : seize fonctions ciblées PASS sur
leurs gels, vraie boucle service, revue indépendante traitée et check toutes
cibles RC0,118 avertissements lib/3 lib-test. [Preuves](VALIDATION.md#6-novembre--confirmation-de-quarantaine-sans-republication).
Le refus d'une publication non scellée reste fermé ; la fin d'une pagination
n'empêche plus les autres profils d'avancer. Aucun nouveau droit d'exécution,
checkpoint publié, HTTP ou arrêt runtime n'est déduit d'une confirmation.
Perte de processus, reprise froide complète, D5/fencing et moteurs restent
ouverts ; G09 FAIL, aucun gate global clos.

Registre workers humain du 6 novembre : permission explicite, migration9→10,
projection/audit/curseurs et vue administrative intégrés. Quarante-cinq fonctions
Rust et quarante-six Node uniques PASS sur leurs gels, tsc et check de composition
RC0, revue indépendante traitée. [Preuves](VALIDATION.md#6-novembre--registre-workers-humain).
Le navigateur/build frontend complet restent BLOCKED ; aucune permission ou
preuve de transport distant, propriété personnelle, capacité100 ou moteur réel
n'est déduite. Attribution initiateur/délégation et parcours cloud complet restent
ouverts. G09 FAIL, aucun gate global clos.

Propriétaires Full du 6 novembre : quatre parents Full et cinq canaris Cold
PASS, compilation toutes cibles RC0 et revue indépendante traitée. Le refus de
capacité précède les mutations/ACK ; les futurs acceptés restent possédés pendant
l'arrêt normal. [Preuves](VALIDATION.md#6-novembre--propriétaires-full-et-admission-avant-ack).
Aucune borne maximale d'arrêt, observation terminale runtime, mort de processus,
reprise froide ou garantie D5/fencing n'en découle. La qualification des moteurs
réels reste obligatoire ; G09 FAIL, aucun gate global clos.

Projection centrale du 6 novembre :4 parents Rust (dont vrai worker refusé),
wire Rust→TS,24 fonctions Node et tsc PASS ; revue traitée, check toutes cibles
RC0 après augmentation du budget mémoire de la QA. Les PASS ne qualifient pas
la connexion multi-hôtes, le navigateur ni la vue SSO humaine.
[Preuves et limites](VALIDATION.md#6-novembre--projection-fédérée-du-registre).
P1 obligatoire supplémentaire : contrôle des destinations réseau depuis le
périmètre Rust de confiance, révocation et audit avant connexion, avec
qualifications Docker/Podman/Kubernetes et couverture G05/G07/G11. Ce contrôle
n'est pas livré par bridge, une variable proxy ou la seule politique Jev.
G09 FAIL,0/12 gates clos.

Prérequis Kubernetes du 6 novembre : CLI opt-in, avis SSAR bornés et refus des
namespaces non actifs raccordés au préflight/CREATE retenu. Vingt-deux fonctions
ciblées PASS, reprise du parent d'oracles corrigés PASS, revue source et check
toutes cibles. [Périmètre et preuves](VALIDATION.md#6-novembre--prérequis-kubernetes-explicites).
Ni SSAR ni peer TLS ne qualifient le cluster Docker Desktop, les sessions ou
le confinement. Sessions V2, permissions effectives et stockage exclusif
restent ouverts ; G09 FAIL,0/12 gates clos.

Drainage des owners d'audit du 6 novembre : six fonctions ciblées PASS,
compilation toutes cibles RC0 et revue indépendante traitée. Cette preuve
sur le même helper d'arrêt que serve ne qualifie ni SIGTERM/processus, ni
runtime réel, ni le drainage des exécutions Full. [Résultats](VALIDATION.md#6-novembre--drainage-des-propriétaires-daudit).
Sortie de quarantaine, D5/fencing et qualifications réelles restent ouverts.
G09 FAIL,0/12 gates globaux clos ; Clippy absent et avertissements inchangés.

Raccord du propriétaire authentifié du 6 novembre : deux parents finaux PASS,
compilation toutes cibles RC0 et revue indépendante traitée. Vraie boucle et
ACK partiel sont vérifiés avec HTTP/SQLite et moteur de seed synthétique.
118 avertissements bibliothèque/3 lib-test et Clippy absent restent ouverts.
[Preuves](VALIDATION.md#6-novembre--propriétaire-authentifié-et-boucle-audit-restreinte).
Le drainage ciblé figure ci-dessus ; sortie de quarantaine, D5/fencing et qualifications réelles
restent ouverts. G09 FAIL,0/12 gates globaux clos.

ACK retenu Stops du 6 novembre : 12 parents métadonnées et 5 acteur PASS,
compilation composée toutes cibles RC0, revue indépendante traitée. Les erreurs
de harnais ne sont pas créditées au comportement. Client/ServiceCold ont depuis
le raccord ciblé ci-dessus ; sortie de quarantaine et qualifications restent
ouvertes ; réserve logique sans
garantie physique. [Preuves](VALIDATION.md#6-novembre--ack-retenu-composé-au-writer-stops).
162 avertissements bibliothèque/6 lib-test et Clippy absent restent ouverts.
G09 FAIL, 0/12 gates globaux clos ; aucune qualification de release.

Contrôle des faits terminaux du 5 novembre : parent ciblé PASS après RED et
trois contre-exemples de revue corrigés, compilation toutes cibles RC0 et revue
traitée. Contrôle pur descriptif, sans identité ni droit d'ACK ; ServiceCold et
fencing ouverts. [Preuves](VALIDATION.md#5-novembre--cohérence-pure-des-faits-terminaux-retenus).
181 avertissements bibliothèque/8 lib-test et Clippy absent restent ouverts ;
G09 FAIL, 0/12 gates globaux clos, aucune qualification runtime nouvelle.

Préparation de capacité d'audit du 5 novembre : 7 parents ciblés PASS et compilation
toutes cibles RC0, revue indépendante sans blocage dans ce périmètre. Réserve
logique seulement ; writer/identité/terminal/ACK et boucle froide à composer.
Les 170 avertissements bibliothèque et 5 lib-test restent ouverts, Clippy absent.
[Preuves](VALIDATION.md#5-novembre--capacité-logique-de-laudit-v4-retenu).
G09 FAIL, 0/12 gates globaux clos ; aucune qualification runtime supplémentaire.

Admissions du writer du5novembre :8parents ciblés PASS, compilation toutes
cibles RC0, revue indépendante traitée ;139avertissements bibliothèque/3lib-test
restent ouverts. La réserve de queue n'est pas une réserve durable ni une
reprise froide. [Preuves](VALIDATION.md#5-novembre--admissions-raii-et-réservation-de-queue-du-writer).
G09 FAIL, aucun gate global clos ; aucun runtime supplémentaire qualifié.

Port HTTP d'audit retenu du5novembre :3nouveaux parents et1canari existant
PASS après RED, compilation toutes cibles RC0, revue indépendante traitée.
Les133avertissements bibliothèque/3lib-test et Clippy absent restent ouverts.
Ce port descriptif et son reçu exact ne sont pas encore composés au writer ;
la boucle froide reproduit toujours l'absence de publication de2événementsV4.
[Preuves et limites](VALIDATION.md#5-novembre--port-http-exact-de-laudit-retenu).
Aucun gate global clos ; G09 reste FAIL, aucun runtime ou cluster qualifié.

Raccord broker HTTP Kubernetes du5novembre :17fonctions ciblées uniques PASS
sur leurs gels, ACK live privé, journal borné et reprise exacte après perte
de lecture de réponse centrale. La vue workers distingue aussi les autorités
de lancement/inspection,15fonctions Node/API et contrôle TypeScript PASS.
[Preuves et limites](VALIDATION.md) ; ces lots ne ferment aucun gate global.
Le peer Kubernetes reste déclaré, activation/session de production fermées,
reprise froide/fencing, relais runtime et vraie fédération multi-hôtes ouverts.
Compilation composée toutes cibles PASS,115warnings bibliothèque et3lib-test,
garde sans avertissement non satisfaite ; Clippy Linux BLOCKED (composant absent).
G09 reste FAIL,0/12 gates globaux clos ; aucune qualification de release.

Réconciliation après annulation, 5 novembre : quatre parents fonctionnels
et deux canaris PASS, compilation toutes cibles et format ciblé PASS, revue
indépendante traitée. Le terminal retenu et sa publication convergent sans
nouveau start ; [preuves et limites](VALIDATION.md#5-novembre--réconciliation-logicielle-après-annulation).
La fixture métier choisit 60 s ; la fiabilité du Writer produit à 10 s reste
ouverte. Ni reprise froide, D5/fencing, runtime réel ou capacité qualifiés.
G09 reste FAIL, 0/12 gates clos ; Clippy Linux BLOCKED.

Grants broker depuis le CLI,4novembre : préparation/rejeu durable, contrôle
de périmètre, CAS, révocation et normalisation des reçus ; 11 fonctions
ciblées uniques, format et compilation toutes cibles PASS, revue traitée.
[Preuves et limites](VALIDATION.md#4-novembre--grants-broker-durables-depuis-le-cli).
Cela complète un parcours opérateur du registre, sans qualifier la fédération
multi-hôtes, le transport worker, Kubernetes ou les pannes réelles.
G09 reste FAIL,0/12 gates clos ; Clippy Linux BLOCKED.

Diagnostic broker du 4 novembre : journal requis absent distingué d'une
entrée invalide, sans création en récupération ; sept fonctions ciblées et
compilation toutes cibles PASS après RED, revue indépendante et guides EN/FR.
[Preuves et limites](VALIDATION.md#4-novembre--journal-requis-absent-et-diagnostic-de-récupération).
Ce résultat logiciel ne qualifie ni restauration, ni réactivation, ni runtime.
G09 reste FAIL, 0/12 gates globaux clos ; Clippy Linux BLOCKED.

Coût publisher du4novembre : compteur d'ancêtres corrigé après RED, sept
parents ciblés uniques PASS sur les gels reliés et all-targets final PASS.
Le getter non autorisant évite un scan ; les contrôles avant effet restent
complets. À N32,262→198 sync fichiers et831→634 sync répertoires observés
sur tmpfs, sans qualification de latence physique, rétention soutenue ou
capacité100. [Preuves](VALIDATION.md#4-novembre--compteurs-de-durabilité-et-snapshot-du-publisher).
G09 FAIL,0/12 gates clos ; aucun résultat runtime nouveau.

Publisher du4novembre :21 fonctions uniques ciblées PASS sur les gels
reliés (19bibliothèque,2vraies CLI), puis compilation composée PASS.
Les défauts de reprise/capacité/historique trouvés
en revue ont leurs RED et corrections. Au gel initial, l'historique était relu/synchronisé
quatre fois par cycle ; durée soutenue, rétention et effet runtime ne sont
pas qualifiés. [Preuves](VALIDATION.md#4-novembre--présence-émise-par-le-service-broker).
G09 FAIL,0/12 gates globaux clos.

Complément Broker du4novembre : portée de la même preuve serveur, transaction
unique et refus de réponse contenant un secret intégrés en87e902d ; six
fonctions ciblées et all-targets PASS, revue traitée. Le profil Kubernetes
local de lecture est provisionné avec sept décisions RBAC opérateur vérifiées,
sans Pod/token ni sonde produit. Longue durée du publisher, fédération, runtime, Clippy
et qualifications restantes conservent leurs écarts ; aucun gate clos.

Complément CLI workload du4novembre : préparation/clé privées et reprise
exacte via les API existantes,17fonctions ciblées uniques PASS, all-targets
et release PASS, revue source traitée, guides/exemples EN/FR.
L'état courant reste inconnu après un reçu historique ; aucune activation
ou transmission de clé n'est implicite. Les Store/TLS et sujets Worker
sont vérifiés dans des fixtures logicielles, sans worker réel. Clippy Linux
est BLOCKED, fautes physiques/publication et transport/handoff/fédération
restent ouverts. [Preuves](VALIDATION.md#4-novembre--cli-durable-des-identités-techniques).
G09 FAIL,0/12 gates globaux clos ; aucune qualification de release.

Mise à jour de la branche de validation au4novembre : l'inventaire STOP du
broker et le préflight Kubernetes audité sont intégrés, avec48fonctions
uniques ciblées PASS sur leurs gels et compilation composée96bf8ed PASS.
Le préflight inclut un POST dry-run autorisé/audité, sans lancement de pod ;
il ne se limite plus au diagnostic GET. La qualification du cluster Desktop,
les dix écarts d'admission et la reprise de vrais workers restent ouverts.
[Preuves et périmètres](VALIDATION.md#4-novembre--préflight-kubernetes-audité).

Le CLI réel avec Keycloak local a atteint login/PKCE, identité/contexte et
lecture autorisée au gel F ; son logout restait non qualifié. Le gel G refuse
plus tôt un cookie dans le harnais ; son résultat ne remplace pas F. Une
compatibilité de cookie quoté est reproduite puis corrigée hors ligne et
le raccord d'accusé de déconnexion passe27tests ciblés, avec profils/guides
EN/FR relus, trois profils validés et compilation composée PASS. Le setup
stdin passe ; son chemin fichier échoue sans cause établie. Aucun nouveau
fait IdP n'est déduit de ces contrôles logiciels. Séparément, la recette H
passe sur le vrai Keycloak local : connexion, lectures, refresh/rotation,
logout et audits durables,13phases après17tests du harnais. Le périmètre
LinuxARM64 isolé ne qualifie pas Web composé/macOS/TTY/entreprise ou runtime.
[Preuve réelle et limites](VALIDATION.md#4-novembre--sso-cli-réel-renouvellement-et-déconnexion).
Les paragraphes suivants conservent leurs résultats historiques ; ils ne
redéfinissent pas ces portées. G09 FAIL,0/12 gates globaux clos.

Le 4 novembre, le [backend workload/registre/relais](VALIDATION.md#4-novembre--backend-workload-registre-et-relais-intégré)
est intégré après86tests ciblés, revues et compilation composée sans
avertissement. Migrations, autorisation, budgets et échanges HTTP/SQLite
ont leurs preuves logicielles ; au gel correspondant, le [diagnostic Kubernetes](../setup/kubernetes/README.fr.md)
restait en lecture seule. Aucun transport worker, fédération ou confinement
runtime n'est qualifié par ces résultats. G09 FAIL,0/12 gates globaux clos.
La commande `kubernetes doctor` est désormais raccordée :
[quatre tests CLI/TLS ciblés, all-targets et revue PASS](VALIDATION.md#4-novembre--commande-cli-de-diagnostic-kubernetes).
Ce résultat n'ajoute aucune admission ni qualification de cluster.

Mise à jour du4novembre : la composition humaine9ee05cc conserve les preuves
logicielles ciblées, sans build/navigateur/Clippy qualifiés. Le DNS hôte
`keycloak.ai-guard.test` est résolu depuis le2novembre et revalidé en lecture
seule ; les mentions `BLOCKED DNS hôte/admin` des gels ci-dessous restent
historiques. Le CLI natif macOS n'est toujours pas qualifié. Un essai CLI
Linux avec Keycloak temporaire était alors préparé, **NOT_RUN** ; ni l'ancien Web,
ni l'inventaire d'images ne le remplacent. [État et prérequis actuels](VALIDATION.md#4-novembre--composition-conservée-et-prérequis-sso-cli).

Préparateur SSO intégré par `586c17e` : configuration Rust hors ligne, reprise durable, profils locaux et guides EN/FR. [Validation ciblée](VALIDATION.md#2-novembre--préparateur-sso-privé-et-profils-de-lancement) et revues passent dans leurs portées documentées. Après les échecs Web historiques, [image 72bfb911 depuis 96fe9be et recettes Keycloak réelles](VALIDATION.md#2-novembre--sso-web-réel-sur-la-page-de-fin-corrigée) : build/rollout RC0 sans réinitialiser le volume, user-1 PASS 7/7/RC0, user-2 PASS 8/8 dans la première recette de rôles, puis admin PASS 13/13/RC0 sur la même image ; connexion, lecture autorisée et logout prouvés pour les trois comptes. Les trois échecs administratifs antérieurs restent FAIL ; seul le prédicat impossible de la troisième capture est démontré/corrigé dans le harness privé. DNS limité au navigateur dédié, CLI natif BLOCKED DNS hôte/admin ; refus adverses de permissions, refresh/révocation, délégation et qualification SSO globale ouverts. G09 FAIL, 0/12 gates clos. Les paragraphes suivants conservent les gels précédents.

Dernier essai G09 sur d6cb136 : [FAIL avant claim](VALIDATION.md#2-novembre--g09-release-courant-refus-avant-claim), worker Created, Arm5s expirée avant première vérification Ready. Aucun Running ni SIGKILL au point prévu ; effet d'arrêt autonome NOT_RUN, pas de fail-open démontré. Release courante et preuves conservées, instrumentation du chemin d'armement prioritaire. Aucun gate clos par cette nouvelle tentative.

État logiciel SSO consigné dans d6cb136, le2novembre : les24tests CLI SSO passent ensemble après corrections d'admission/rotation et de préparation des fixtures ; revues de ces correctifs traitées, fmt et Clippy strict finaux PASS. Le test transport exact passe après correction de sa fixture, sans rejeu de la suite entière ; les autres résultats restent attachés à leurs gels. Image worker générique Docker construite/inspectée, aucun worker lancé par ce build. Qualification réelle SSO/runtime encore ouverte ; aucun gate global clos. [Chronologie](VALIDATION.md#2-novembre--composition-sso-résultats-comportementaux-et-revues).

Autres preuves du2novembre : la [recette humaine réelle Docker API/CLI A/B](VALIDATION.md#2-novembre--recette-humaine-réelle-docker-apicli-ab) passe sur l'image7afdd43, avec migrations sauvegardées3→4→5, permissions/contenu/export, restart et révocation hors ligne. Navigateur NOT_RUN, aucun worker lancé. Les cibles humain14, audit24, gateway3+7 de la [composition OIDC candidate](VALIDATION.md#2-novembre--composition-sso-résultats-comportementaux-et-revues) restent PASS sur leurs gels. Les résultats intermédiaires transport15/16 et CLI19/21 sont historiques : leurs défauts connus sont corrigés et vérifiés dans la portée finale ci-dessus. Keycloak réel, bootstrap, délégation et fédération restent non qualifiés ; les travaux SSO/setup du worktree distinct ne sont pas intégrés à ce gel. Les mentions historiques « absent » ci-dessous portent sur leurs gels datés. G09 demeure FAIL,0/12 gates globaux clos.

Historique du1er novembre : [accès humain local, API/CLI et dashboard](VALIDATION.md#1er-novembre--accès-humain-local-apicli-et-dashboard) vérifiés dans les gels logiciels ciblés, migrations/audit/permissions et revues traitées. À ce gel, nouvelle image/parcours humain réel, OIDC/PKCE et délégation restent ouverts ; la recette humaine du2novembre ci-dessus apporte depuis une preuve supplémentaire, sans fermer G04 ou G08. Le trajet HTTPS du modèle local et sa panne/reprise santé passent, mais l'unique tentative d'évaluation reste FAIL_MODEL de cause fine inconnue. G09 reste FAIL et aucun gate global n'est clos.

Le [générateur PKI local](evidence/2026-11-01/demo-pki/README.md) a ses trois tests ciblés, TLS réel et checks statiques PASS. Il fournit des certificats de test ; G11 reste BLOCKED pour les identités/transports et le cycle PKI des déploiements. Aucun gate n'est clos par ce lot.

Complément du1er novembre : [mesures du journal et factorisation canonique](evidence/2026-11-01/journal-cost/README.md#résultats-finaux-du-lot), neuf fonctions uniques et fmt/Clippy strict/release PASS, revue traitée,160 sources et binaires vérifiés. Deux FAIL de caller outbox sont conservés ; seule l'attente du test fonctionnel512 change, aucun délai produit. Le probe avant/après sur31 révisions ne démontre aucun gain de latence, performance CPU release, capacité100 ou cause H10 ; FS exact NOT_RECORDED. Les contrôles et compteurs de durabilité restent inchangés. L'observation passive précédente d'un gardien ne donne ni coût global ni permission de disarm. G09 reste FAIL, D5 ouvert et0/12 gates clos ; aucun résultat ci-dessous n'est élargi.

Les exigences détaillées G01–G12 sont conservées dans le [document racine](../../PRODUCTION_READINESS.md). Ses anciennes affirmations d'implémentation multi-client ont été contredites par l'inventaire du checkout ; elles ne constituent pas des preuves. L'[état courant](CURRENT_STATE.md) et les [résultats exécutés](VALIDATION.md) font autorité sur l'avancement.

| Gate | Exigence | État de qualification courant |
|---|---|---|
| G01 | Contrats et matrice des plateformes/artefacts | BLOCKED : candidat, versions et environnements non qualifiés |
| G02 | Confinement et isolation tenant | BLOCKED : identité/base API vérifiées ; confinement réel partiel Docker Desktop ARM64 PASS, recette workers A1/A2/B1 et autres moteurs/plateformes ouverts |
| G03 | Copie/export sûrs et séparés par tenant | BLOCKED : régressions logicielles et copie/export réels Docker Desktop PASS ; publication avec barrières fichiers/répertoires/parent et ascendance UID/modes/ACL vérifiée au gel du31octobre ; pannes stockage, autres UID, isolation de volumes A/B et matrice complète manquantes |
| G04 | Autorisation, identité, quotas | BLOCKED : registre/quota/CAS/claim durables et parcours Docker ciblés PASS au gel209 ; reprise/crash sur moteur réel, recette3workers et durée autonome encore ouverts ; candidats d'audit à traiter |
| G05 | Réseau, secrets et relais contrôlé | BLOCKED : entrée Rust relue et vérifiée sur Docker Desktop, restrictions runtime partiellement observées ; identités/grants/relais HTTP et SQLite vérifiés au4novembre, transport réel worker et cloud non qualifiés |
| G06 | Audit/usage durables, restauration, migrations | BLOCKED : schéma v2/ledger non facturable, migration/backup privés et gel327 PASS ; StoreStorage/locks/ACL vérifiés sur macOS et Linux dev, deux UID réels, maintenance QA avec591événements inchangés. Restauration sûre des claims, pannes et portée adverse/FS restante non qualifiées ; voir dossier SQLite |
| G07 | Fournisseurs et limites d'observation | BLOCKED : Codex0.165.0 et Claude2.1.140 API batch manuels PASS, puis Claude/API batch géré réel PASS ciblé sur nouvelle image ARM64 (demande explicite Haiku5.5, fichier/export/audit après restart). Autres combinaisons API/abonnement gérées et TUI deux tours non qualifiés. Paramètres adverses, refresh, collecte native et médiation avant action ouverts ; voir dossiers provider-auth/provider-cli/interactive-cli |
| G08 | API, dashboard et sécurité HTTP/SSE | BLOCKED : API A/B, guards avant contenu/index partagé vérifiés par51tests ciblés/TCP réel ; arrêt SSE gateway/dashboard observé vers5s après signal sur macOS, ingress compatible, repos SIGINT/SIGTERM PASS. Front27tests et contrôles navigateur historiques PASS. Charge lente/IO bloqués et disponibilité globale non qualifiés, multi-client3workers réel et relais absents ; candidats statique à valider |
| G09 | Résilience et exploitation | FAIL : perte d'échéance reproduite sur Docker Desktop après SIGKILL du CLI (worker encore Running après7s pour limite5s), arrêt/réconciliation ciblés ensuite vérifiés. Arrêt strict sur snapshot32tests/revue/build PASS, sans supervision autonome ; broker permanent/gardien/fencing, autres pannes et matrice plateforme ouverts |
| G10 | Capacité et ressources | NOT_RUN : la charge synthétique d'audit n'est pas la capacité 100 workers |
| G11 | Infrastructure, transport et PKI conditionnels | BLOCKED : identité workload, transport et déploiement cloud à compléter et qualifier ; aucune infrastructure externe utilisée |
| G12 | Publication OSS, supply chain et preuves de release | BLOCKED : licence Apache-2.0 ajoutée ; canal privé à décider, chaîne de livraison à qualifier, aucune release qualifiée |

Un statut porte uniquement sur la preuve nommée. Un lot logiciel réussi ne clôture pas les gates d'effets runtime. La publication production exige tous les gates applicables PASS, ou NOT_APPLICABLE justifié avant essais. Les quatre combinaisons Docker/Podman × Linux x86_64/macOS ARM64 restent exigées ; Kubernetes reste une cible complète distincte.

Complément H10/S3/S4/frontend du31octobre : H10 expire à600,04s avec FailureObserved locale avant Created/Arm/claim worker mais sans diagnostic public. Nettoyage de cache concomitant et inventaire37→3, cause exacte non établie ; aucun Running ni faute au point prévu. Le lot logiciel suivant publie un rapport fermé même sans provenance spécialisée, avec effet inconnu et limites de livraison explicites :8 tests Rust et checks stricts/release PASS. Frontend modèle12/12 et build statique PASS ; erreurs visibles dans Actions et résumé détaillé, sans qualification navigateur/visuelle ou Node24. Revues indépendantes traitées, aucune nouvelle preuve moteur du comportement. [État, RED et limites](evidence/2026-10-31/broker-runtime/README.md#h10--échec-local-durable-sans-diagnostic-public). Les analyses des slots et des sources Moby n'autorisent aucun disarm : D5, coût durable, reprise et qualification demeurent ouverts. G09 FAIL,0/12 gates clos.

Complément H8/H9/S1/S2 du31octobre : H8 expire pendant l'observation, H9 obtient réellement le premier `broker.batch.failure` durable (`slot_phase_mismatch` avant claim, sans sample local). Aucun Running ni faute au point prévu, aucune expiration native démontrée. S1 retire la seule relecture caller après Register via un Draft confirmé au head, sans permission ni suppression de barrières, et ajoute l'origine helper du snapshot d'erreur.20 fonctions ciblées PASS ; S2 corrige le seul lint d'assertion de test puis fmt/Clippy strict/release passent, sans replay de tests ou code applicatif modifié. Son effet runtime est NOT_RUN. Le budget global de test devient600s à la demande du mainteneur, Arm worker5s inchangée. [Preuves et suite](evidence/2026-10-31/broker-runtime/README.md). G09 reste FAIL,0/12 gates clos ; D5, reprise réelle, multi-workers et SSO restent ouverts.

Complément historique du31octobre, gels D3/PR4 avant H8/H9 : [verticale broker Docker H4–H7 et diagnostics](evidence/2026-10-31/broker-runtime/README.md). H4/H5 échouent après Arm/claim durables, H6 atteint une borne du harnais pendant la préparation, H7 enregistre un échec avant claim après Arm worker ACKée. Aucun Running ni faute au point prévu ; les causes H4/H5/H7 restent ouvertes. D3 a ses huit fonctions logicielles uniques ; le complément préclaim/adaptation lifecycle a16 fonctions ciblées, fmt/Clippy/release PASS et une régression partielle RED/GREEN corrigée en revue. À ces gels, son émission réelle et son effet temporel sont NOT_RUN. Ni correction de la deadline, ni disarm D5 qualifié ; G09 reste FAIL et0/12 gates sont clos. SSO/Keycloak/PKCE demeure absent et sa qualification NOT_RUN.

Complément logiciel du31octobre : [acteur STOP restreint et diagnostic](evidence/2026-10-31/stop-only-actor/README.md),33 fonctions ciblées et revue traitée, fmt/Clippy strict/release PASS. Premier Cancel sans blob, journal partiel, Unknown/replay, transfert continu de lease et rejets sont vérifiés dans leur périmètre logiciel ; vraie auth HTTP/A/B et refus vide sur connexion Unix inclus. Aucun worker ni échéance moteur n'est qualifié par ces preuves : G09 conserve son FAIL et0/12 gates sont fermés. Les limites de pile du harnais et de connexion restent explicites dans le dossier.

Compléments explicites du mainteneur,30octobre : G07 exige Codex CLI et Claude Code, chacun avec abonnement et clé API, sélection dynamique annoncée, aucun repli silencieux vers une API facturée et aucun cache OAuth extrait implicitement. G08 exige une vue Web unique de tous les agents/workers du tenant sur ses brokers/hôtes/runtimes, avec provenance, fraîcheur et états partiels ; pas un dashboard par worker. G04/G06/G10 couvrent quotas et comptage globaux du tenant et capacité100mesurée. Aucun de ces compléments n'est clôturé par sa seule inscription ou par le `.env` local créé.

Trois approches Jev exigées : API réelle, règles Rust explicitement incomplètes, petit modèle local configurable Docker/Podman ≤4B et cible 500 Mo–1 Go. Le premier essai local 0,5B respecte 1 Go sur Docker Desktop (pic 777 912 320 octets) mais échoue sémantiquement sur une suppression ; candidat refusé pour autoriser des actions. Cela ne clôture ni G04 ni G07/G10. Sélection commune et adaptateur local restent à livrer ; refus impératifs et confinement indépendants. [Preuves et limites](evidence/2026-10-30/local-judge/README.md).

Le [smoke de réutilisation du1er novembre](evidence/2026-11-01/local-inference/README.md) confirme un seul appel Chat Completions et un pic788877312 octets sans OOM, après correction du probe fragmenté et refus tronqué/silence. Le serveur est arrêté ; le jugement de sécurité reste FAIL. Aucun adaptateur produit, client Codex/Claude, transport TLS ou déploiement entreprise n'est qualifié par ce résultat ; aucun gate clos.

Compléments du 31 octobre : le [contrat canonique d'identité et de configuration](../sso.md) et les [vues selon les permissions](../ui.md#vues-utilisateur-et-administration-unifiée--cible-à-livrer) s'ajoutent aux critères ci-dessus. **À implémenter et qualifier**, sans modification des résultats historiques :

| Gates concernés | Preuves obligatoires supplémentaires |
|---|---|
| G04 | Identité locale nominative hors ligne ou OIDC public PKCE S256, mapping configurable des claims vérifiés, permissions et périmètres côté Rust, délégations sans élévation ; les anciennes clés ne prouvent pas l'identité humaine |
| G05/G07/G11 | Inférence interne avec URL/modèle/format exact/adaptation Claude, credential minimal et chaîne publique d'AC, TLS validé et réseau interne approuvé sans repli cloud ; combinaisons incompatibles explicitement refusées |
| G06/G07 | Causalité individuelle, couverture réelle d'observation, audit indépendant protégé, durable, vérifiable et exportable avec points de contrôle signés ; limites et pertes déclarées |
| G08 | Parcours Web/CLI et vues utilisateur/administration à 360° du seul périmètre autorisé, agrégation Rust et retrait des données après changement de droits/contexte |
| G09/G12 | Installation documentée depuis les Dockerfiles, construction/distribution des images par le client sans image publique obligatoire, profil Keycloak local opt-in à quatre comptes et procédure de raccordement entreprise ; parcours réel chronométré, sans garantie des cinq minutes |

Le produit doit qualifier son support SSO même si une installation le laisse désactivé. La [matrice de tests](TEST_STRATEGY.md#identité-individuelle-sso-et-découverte--à-qualifier) couvre les axes indépendants et leurs interactions critiques ; elle ne promet pas toutes les combinaisons. Cette mise à jour documentaire ne ferme aucun gate.

La déclinaison [technique OIDC/PKCE](SSO_OIDC.md) exige aussi les preuves de découverte/JWKS et de transport IdP bornés (G05/G11), de renouvellement/expiration/logout/révocation et fin SSE (G08), de migrations et de préservation des identités historiques (G06). Le gateway propriétaire de la transaction Web est approuvé le1er novembre ; au2novembre, son implémentation candidate possède les preuves logicielles ciblées ci-dessus. La qualification IdP réel/navigateur et les scénarios de déploiement restent NOT_RUN ; les écarts de qualification restants ne sont pas transformés en succès par cette mise à jour.
