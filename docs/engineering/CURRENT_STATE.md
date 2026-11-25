# Synthèse courante — 25 novembre 2026

Le candidat local intègre la projection d’audit Claude Code, ses
régressions de compaction, les témoins Jev sur loopback et la recette
de migration et de restauration SQLite via le CLI. Le gel `4411e705`
apporte douze résultats ciblés réussis. Le gel `379265e8` vérifie
que le refus du CLI expose la règle et le motif audités. Le
candidat `0585be3c` reprend les sources et les tests du correctif.
[Résultats exacts et limites](VALIDATION.md).

G10 est validé au périmètre V1 sur `48f81d20`. Le quota accepte
100 réservations durables pour un tenant et la recette physique
utilise A1/A2 chez A et B1 chez B. G09 conserve son échec moteur.
Dix autres gates restent ouverts. [Grille de qualification](PRODUCTION_READINESS.md).

Le témoin Full3 laisse `CreateIssued` confirmé dans SQLite et encore
en attente dans le journal du broker. Le diagnostic distingue désormais
la réponse HTTP de la publication de l’acquittement local. Deux tests
de fixture, un par frontière, passent sur `d7d4f849`. La cause de
l’échec Full3 historique attend un rejeu instrumenté. Le contrat
E3 confie la libération au broker après ouverture durable. Son
intégration logicielle et la qualification moteur ont chacune leurs
preuves. [Résultats Full3](VALIDATION.md#25-novembre-2026--full3-sarrête-après-createissued-dans-sqlite) · [Décision E3](DECISIONS.md#25-novembre-2026--g05e3-libération-du-fournisseur-par-le-broker).

## Synthèse courante — 17 novembre 2026

## 17 novembre — contribution D7 intégrée, qualification ouverte

Le code D7 provient du commit `6a32771` : propriétaires STOP natifs réservés avant acquittement, capacité explicite et admission de reprise fermée ; un Pending déjà confirmé est relu strictement sans publier une révision vide. Les [preuves EN/FR et les FQN](evidence/2026-11-17/d7-native-stop-owners/README.fr.md) séparent chaque source : le gel Legacy R5 `6c5554cd…` (2 964 fichiers) passe check toutes cibles, build libtest puis **16 cas lib exacts PASS**, chacun invoqué une fois, aucun ignoré. Clippy toutes cibles rend **RC0 non strict, 561 émissions d’avertissement, zéro erreur** ; la gate stricte reste ouverte.

Le gel R4 `5b82fdd` conserve ses **4 PASS / 1 FAIL / 15 NOT_RUN**, dont un vrai ancien binaire V1 explicitement invoqué et réussi. Ces PASS ne sont pas transférés à R5 ; les erreurs antérieures restent datées. Réouverture métier froide, Pending sans owner retenu, retraite/Q Legacy, libération de quota, STOP physique, moteurs réels et capacité 100 restent ouverts. **Aucun fingerprint global d’audit ni aucun des 12 gates n’est clos.**

## 17 novembre — relevé runtime et finalisation tardive D1

[Douze cas logiciels ciblés](evidence/2026-11-17/runtime-observation-and-d1-late-finalizer/README.fr.md) passent sur le gel `c755fb52…db3b9`, dont quatre vrais CLI/Gateway/SQLite avec moteur Rust de fixture : terminal unique, faute Managed tardive et quota/instance conservés pour Created/Unknown. Le relevé facultatif est borné, sans attente dans STOP, et ne remplace pas l’audit durable. Le précédent FAIL de sonde reste historique ; la régression lit désormais la capture dans son véritable propriétaire d’erreur. Compilation des trois tests et six listes vérifiées ; Clippy toutes cibles de cette révision RC0 non strict, 528 émissions d’avertissement, 0 erreur ; 21 émissions supplémentaires restent suivies. Ni moteur réel, reprise froide, fournisseur, scénario A1/A2/B1 complet ni capacité 100 ne sont qualifiés. **0/12 gates clos**.

**G10 passe au périmètre V1 sur `48f81d20`.** G09 conserve son FAIL et les dix autres gates restent ouverts selon la grille de qualification. La V1 reste à qualifier. [Gates et écarts](PRODUCTION_READINESS.md).

Base de l’intégration D1 : `961045788784b277a7a9eaa5e8f411275d7147e0`, dont le code produit avant D1 est celui de `a3981316d5a0ff1678dee7d943971fa845991e84`. Ce changement intègre les huit fichiers Rust D1. Clippy D1 toutes cibles sur la source composée `74d60de1…b7b` : **RC0 non strict, 507 émissions d’avertissement et 0 erreur**. Reçu `dc43ad33…0e0a2` ; gate strict ouvert. Les 324 395 octets de l’ancien suivi restent intacts dans l’[archive adjacente](CURRENT_STATE_HISTORY_20261116.md), SHA-256 `3dfd04cc353bb1f3455ad4eb066778abe0c79862cd197927abdb1077f8eda254`. Les preuves détaillées restent dans [VALIDATION.md](VALIDATION.md).

## Relevé antérieur du 17 novembre — D7 et Native17 privés

Au relevé `2773bb2`, le code public restait `e794e45`. Les [gels privés et leurs reçus](evidence/2026-11-17/d7-native17-progress/README.fr.md) ne constituent pas du code livré sur cette branche : D7 R5 `653e2abe…` passe check/build libtest puis **28 PASS / 1 FAIL de pile / 5 NOT_RUN** ; R6 `8d174676…`, test-only, apporte **un nouveau PASS du seul cas 9**, sans transfert des 28 PASS. Le check Legacy Native+Ports R1 `0dce799c…` échoue RC101 sur un E0599 de fixture ; la correction `07c9679` n’est pas recompilée, les tests restent NOT_RUN et l’ancien cas 9 est obsolète.

Le commun Native17 R5 `a3e9ca09…` passe check toutes cibles et build libtest, puis ses deux diagnostics réels donnent **0 PASS / 2 FAIL RC101** sur l’époque inchangée de 2 s. La cause du backup est désormais typée et retenue ; OIDC refuse à `TransactionMutations`, 1 999 ms. Ces durées ne mesurent ni CPU ni attente de verrou. Deux remédiations étaient alors en cours, sans correction publique ni qualification déduite. Clippy strict reste ouvert ; **0/12 gates clos**.

## Faits vérifiés et portée — 16 novembre 2026

| Périmètre | État constaté et source |
|---|---|
| Composition publique R2 runtime/Kubernetes | **26 PASS**, une invocation par cas sur son gel propre ; format et Clippy toutes cibles RC0, **505 avertissements, non strict**. Deux binaires et sources revérifiés. Preuve logicielle : ni nouvel essai moteur, ni délai physique d’arrêt, ni cluster. [Relevé R2 et historiques séparés](evidence/2026-11-16/runtime-kubernetes-batch-composition/README.md). |
| Audit D1 pré-démarrage | Correctif logiciel : **14 PASS** sur la source `74d60de1…b7b` (2 932 fichiers), dont quatre vrais CLI ai-guard/Gateway/SQLite avec moteur Rust de fixture. La faute GET 429 passe de `starts=1` dans la baseline RED à `starts=0` dans le correctif, audits valides. Les huit Rust D1 sont exacts à cette source ; les deux fichiers D7 cfg(test) ne sont pas destinés au commit D1. Clippy toutes cibles **RC0 non strict, 507 émissions d’avertissement, 0 erreur** ; gate strict ouvert. Verdict primaire historique inchangé ; qualification globale ouverte. [Preuve, causes et limites](evidence/2026-11-16/control-monitor-prestart-fix/README.md). |
| Native17 configuré — privé | Source privée R2 `b13d6bdd…c0d2` : **11 PASS / 1 FAIL** sur 12 cas distincts ; le heartbeat du Broker enrôlé refuse avec une cause opaque. Reçu privé `84bc809ad25fd54308fd0bc61c171e4a0cb48165584bc02ec9430068c1a511f2`. Ni intégré ni qualifié ; aucun transfert au public, à Full ou au runtime réel. |
| D7 capacité de reprise | Ouvert. Phase R1 : setup TMPDIR/UDS en échec avant oracle, conservé. Après correction de cette seule configuration de harnais, R2 : canari cap2 **PASS RC0**, Restore cap1 **RED RC101 au vrai oracle ACK sans STOP/Cancel** ; troisième cas NOT_RUN. Reçu Root `5634d050…c487`. Aucun correctif produit D7 inclus dans D1. |

## Écarts ouverts et travaux privés

La faute tardive Managed jusqu’au finalizer Store/quota est désormais couverte logiciellement par les quatre CLI/Gateway/SQLite inclus dans les douze PASS du 17 novembre. Le vrai TTY interactif, les moteurs réels et les garanties d’abandon/reprise froide restent ouverts ; les unités Managed et PTY ne les qualifient pas. Les résultats Native17 privés restent séparés ; heartbeat/self ne remplace pas le checkpoint **Full**, son installation/profil, la reprise ou la supersession.

La cible V1 conserve les parcours complets, le contrôle réseau Rust, l’audit, l’arrêt/reprise, la restauration, les quatre parcours Codex CLI et Claude Code, Docker/Podman/Kubernetes et TLS. Pour G10, la recette A1/A2 chez A et B1 chez B et les 100 réservations durables chez A sont attestées sur `48f81d20`. La capacité physique à 100 reste une cible de dimensionnement. [Mandat](../../AGENTS.md), [contrat d’architecture](ARCHITECTURE_CONTRACT.md), [identité et configuration](../sso.md), [gates](PRODUCTION_READINESS.md), [CI G10](https://github.com/Needdoliprane/ai-guard/actions/runs/45344259411).

## Priorités de qualification

1. Qualifier les parcours D1 encore ouverts : moteurs réels, vrai TTY, abandon et reprise. Conserver la preuve logicielle du finalizer Store/quota, les causes, la possession, la révocation et le drainage.
2. Analyser les deux refus d’époque du commun Native17 R5 et vérifier les remédiations privées sur leurs propres sources, puis qualifier séparément les parcours restants et Full ; aucun ancien PASS n’est transféré.
3. Relier chaque lot relu à ses preuves réelles et qualifier les plateformes requises. Les [audits run-2](AUDIT_RUN_2_20261110.md), le [plan](IMPLEMENTATION_PLAN.md) et la [stratégie de tests](TEST_STRATEGY.md) gardent leurs fonctions distinctes.

## Index de compatibilité historique — gel bf31a743

Les titres ci-dessous reprennent exactement le texte, le niveau et l’ordre des 193 titres ATX historiques, H1 inclus, afin de conserver leurs repères entrants. Chaque renvoi ouvre le corps intégral dans l’archive ; les anciennes conclusions y restent datées. L’archive est dans le même dossier, ce qui conserve la base de ses liens relatifs.

# État constaté — suivi du développement

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 16 novembre — audit D1 : déclencheur pré-démarrage reproduit

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 16 novembre — R2 runtime/Kubernetes : 26 cas de composition vérifiés

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 16 novembre — Native17 R6 : prérequis configuré toujours en échec

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 16 novembre — Native17 compilable, prérequis configuré encore en échec

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 16 novembre — bornes HTTP vérifiées logiciellement

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 16 novembre — défauts mesurés au démarrage et sur Docker

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 16 novembre — callback SSO CLI borné

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 16 novembre — création avec image locale préparée

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 16 novembre — lot logiciel Kubernetes dédié intégré

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 16 novembre — tests natifs exécutés et raccords en correction

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 16 novembre — composition native : compilation réussie, comportements à vérifier

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 15 novembre — compilation du successeur natif R10

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 15 novembre — compilation du noyau natif complet R9

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 15 novembre — preuves ciblées et intégration native en cours

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 15 novembre — Kubernetes/fédération Claude : 19 tests logiciels vérifiés

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 14 novembre — version 0.0.1 et identité commune des builds validées

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 14 novembre — fédération C3 composée : 44 parents logiciels vérifiés

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 14 novembre — broker : convergence chaude Submitted/Pending vérifiée

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 14 novembre — inspection du registre et préparation CLI : composition vérifiée

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 14 novembre — attach Kubernetes composé : 11 PASS ciblés sur `1e30e5e`

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 14 novembre — A1 : API/CLI du registre egress, 74 PASS sur une source exacte

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 13 novembre — A0 : diagnostic réservé, 37 PASS ciblés sur `0114013`

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 13 novembre — lectures owned de métadonnées et interface vérifiées

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 13 novembre — sources humaines possédées et bootstrap opérateur

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 13 novembre — validation des sources humaines, composition publique réduite

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 13 novembre — noyau et protocole egress, tranche logicielle vérifiée

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 13 novembre — expiration du contrôle d’identité : erreur temporaire explicite

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 13 novembre — contrôle réseau : accord de développement, réalisation en cours

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 12 novembre — garanties opérateur : documentation corrigée, écarts produit conservés

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 12 novembre, 21:24 UTC — lecteur historique des événements : correction ciblée vérifiée

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 12 novembre — namespaces d'événements : admission validée, lecteur historique encore ouvert

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 12 novembre — rapport de couverture final reçu, suivi obligatoire créé

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 12 novembre — ports Store retenus : 22 tests et contrat séparé de cinq tests PASS

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 12 novembre — sources humaines : cinq compléments physiques et temporels PASS

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 12 novembre — union A+B+Auth : 17 régressions de migration et démarrage PASS

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 12 novembre — profil tenant14 : onze régressions de migration et replay PASS

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 12 novembre — reprise du relais : deux consommateurs sur composition courante vérifiés

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 12 novembre — stockage AuthLedger5 : 17 régressions logicielles PASS

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 12 novembre, 07:07 UTC — socle humain/SQLite composé : 61 parents logiciels PASS

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 12 novembre, 03:58 UTC — candidats privés : 47 parents Core et deux reprises du relais

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 12 novembre, 03:04 UTC — diagnostic opérateur E : validation logicielle composée

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 12 novembre, 00:12 UTC — ApproveLaunch préparatoire : quatre parents logiciels PASS

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 11 novembre, 22:44 UTC — maintenance des sources : 19 parents logiciels PASS

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 11 novembre, 21:47 UTC — primitives d'autorité : 34 parents logiciels PASS

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 11 novembre, 20:33 UTC — reprise Kubernetes stricte : cinq parents r8 PASS

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 11 novembre, 18:00 UTC — sources build/Compose : 14 parents logiciels PASS

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 11 novembre, 17:17 UTC — essai Core/schema13 arrêté et diagnostic ciblé

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 11 novembre, 15:44 UTC — compatibilité held-CREATE sous schéma12 corrigée

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 11 novembre,15:04 UTC — reprise du relais composée et vérifiée localement

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 11 novembre — historique Core/API/SDK validé, retrait local R2 vérifié séparément

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 11 novembre — deux écarts de reprise du propriétaire relais en correction

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 11 novembre — refus SendV2 publiés dans le client et les composants stdio

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 11 novembre — stockage durable et rétention du relais publiés

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 11 novembre — credential viewer explicite pour verify-audit

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 11 novembre — supervision Web publiée, backend et quatre parcours navigateur vérifiés

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 11 novembre — relais durable et refus typés vérifiés en composition

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 10 novembre — checkpoints d’audit signés : six parents ciblés vérifiés

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 10 novembre — frontend B : validations Node d'un candidat non fusionné

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 10 novembre — composition Kubernetes : cinq parents R1 et un parent R2

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 10 novembre — part native du registre OIDC bornée par tenant

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 10 novembre — préparation SetupV2 vérifiée en logiciel

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 10 novembre — demande Resume Core/HTTP vérifiée en logiciel

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 10 novembre — parts finies AuthV4 vérifiées en logiciel

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 10 novembre — source checkpoint du service fédération vérifiée en logiciel

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 10 novembre — rétention SSE bornée, D3 logiciel vérifié

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 10 novembre — capacité OIDC et sortie du propriétaire corrigées

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 10 novembre — capacité OIDC vérifiée, reprise complémentaire à corriger

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 10 novembre — répartition D8 intégrée et OIDC reproduit

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 10 novembre — première validation D8 et reprise du harnais

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 10 novembre — broker r3, capacité humaine et capture privée

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 10 novembre — réserve terminale OIDC et corrections parallèles

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 9 novembre — historique des audits conservé et suites parallèles attribuées

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 9 novembre — Core12 et composition fédération/bootstrap vérifiés en logiciel

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 9 novembre — Full3 reste en échec, frontière mieux observée

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 9 novembre — correctifs C1/C2 vérifiés sur le candidat D5

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 9 novembre — composition D5 après S2 et fermeture des worktrees du lot

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 9 novembre — récupération centrale S2 raccordée et vérifiée en logiciel

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 8 novembre, 23:32 UTC — défaut de drainage S2 reproduit

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 8 novembre, 23:20 UTC — S2 compilé, scénario de cycle de vie non concluant

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 8 novembre — stockage S1 et migration CLI11 intégrés

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 8 novembre — B1 historique reproduit ; mesure broker terminée

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 8 novembre — consommateurs du schéma 11 intégrés, six validateurs vérifiés

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 8 novembre — baseline de migration 11 reproduite, implémentation autorisée

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 8 novembre — AC privée du client broker intégrée et vérifiée

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 8 novembre — Full3 progresse, mais échoue aussi sur observation de 60 secondes

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 8 novembre — C4 progresse jusqu'à l'observation du helper ; handoff toujours en échec

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 8 novembre — cinq régressions composées passent ; pile d'import encore ouverte

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 8 novembre — profil Kubernetes V2 intégré ; défaut d'arrêt reproduit

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 8 novembre — Kubernetes V2 autorisé ; relais95 toujours FAIL

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 8 novembre — claim helper corrigé sur94b, parent Export vérifié

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 8 novembre — Export atteint le journal ; relais encore en débordement de pile

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 8 novembre — cadres D5 réduits ; deux diagnostics fonctionnels encore ouverts

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 8 novembre — lectures de métadonnées voisines corrigées et intégrées

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 8 novembre — sept régressions K passent ; raccord relais compilé, tests fonctionnels ouverts

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 8 novembre — deux défauts du candidat K reproduits ; trois travaux indépendants actifs

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 8 novembre — composition logicielle du relais validée ; audit K et D5 prioritaires

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 8 novembre — admission du relais intégrée, raccord broker en cours

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 7 novembre — réserve d'audit intégrée, composition D5 et raccord relais en validation

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 7 novembre — interface fédérée et helper relais publiés ; diagnostic D5 en correction

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 7 novembre — permission préflight Kubernetes vérifiée et raccord relais approuvé

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 7 novembre — API fédérée vérifiée, interface et reprises encore en qualification

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 7 novembre — client relais et reprise Kubernetes composés sur le gel27

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 7 novembre — client relais intégré, reprise Kubernetes et fédération en cours

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 7 novembre — composition du contrôle, de la reprise et du SSO validée

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 7 novembre — contrôle H/I/L composé, reprise et SSO en validation

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 7 novembre — export filtré et modes observés, lot J

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 6 novembre — récupération du Store après pénurie de descripteurs

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 6 novembre — cohérence des guides local, Keycloak et SSO

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 6 novembre — reprise de présence après panne temporaire du gateway

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 6 novembre — confiance HTTPS explicite du CLI humain

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 6 novembre — confirmation des ACK en quarantaine

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 6 novembre — registre workers pour les identités humaines

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 6 novembre — propriétaires Full et admission avant accusé de réception

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 6 novembre — projection centrale workers et brokers

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 6 novembre — prérequis Kubernetes consommés par le CLI

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 6 novembre — drainage des propriétaires d'audit à l'arrêt normal

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 6 novembre — propriétaire authentifié de l'audit retenu

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 6 novembre — ACK retenu sur l'acteur Stops

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 5 novembre — cohérence des faits terminaux retenus

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 5 novembre — capacité logique pour l'audit V4 retenu

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 5 novembre — bornes HTTP pendant une lecture en flux

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 5 novembre — admissions bornées du writer

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 5 novembre — reçu HTTP exact pour l'audit retenu

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 5 novembre — client broker HTTP et reprise du résultat Kubernetes

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 5 novembre — autorités distinctes dans la vue des workers

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 5 novembre — décisions Kubernetes V2 et six routes centrales authentifiées

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 5 novembre — format d'audit Kubernetes 9 et sauvegardes compatibles

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 5 novembre — politique Kubernetes centrale chargée par tenant

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 5 novembre — refus de résultat durable et provenance vérifiée

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 5 novembre — rétention et reprise communes des résultats Kubernetes

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 5 novembre — permission et transport CREATE vérifiés sur TLS local

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 5 novembre — fondation d'admission Kubernetes hors cluster, en cours

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 5 novembre — réconciliation après annulation, périmètre logiciel vérifié

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 5 novembre — documentation alignée sur les accès humains existants

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 4 novembre — direction Kubernetes locale approuvée, aucune admission ajoutée

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 4 novembre — autorisations broker depuis le CLI

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 4 novembre — build refusé avant moteur si le paquet est incomplet

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 4 novembre — canal de relais étudié, primitives Docker observées

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 4 novembre — diagnostic d'un journal de récupération absent

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 4 novembre — coût mesuré et relecture redondante retirée

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 4 novembre — publisher possédé par le service broker

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 4 novembre — portée courante authentifiée du broker

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 4 novembre — cible Kubernetes locale préparée en lecture

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 4 novembre — opérations CLI des identités techniques

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 4 novembre — parcours natif SSO vérifié dans son périmètre Linux

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 4 novembre — déconnexion OIDC explicite

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 4 novembre — découverte des jobs en récupération

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## Vue humaine des brokers — composition partiellement validée

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## Correctifs initiaux de l'audit en cours

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## Chantiers parallèles après les correctifs F

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## Historique du 2 novembre — D5 et SSO

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## Préparation SSO locale — intégrée, recette complète ouverte

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## G09 — essai courant refusé avant lancement

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## SSO Web/CLI — lots parallèles en intégration

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## Image de contrôle — permissions publiques vérifiées

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## Lots parallèles ouverts — identité, dashboard et registre

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## Diagnostic Jev — validation logicielle ciblée

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## Identité humaine — domaine et stockage vérifiés

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## Accès humain local — validation logicielle ciblée

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## Dashboard — navigation liée vérifiée

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## S0 — durée de vie du verrou gardien, correctif natif vérifié

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## Qualification Docker du juge TLS — démarrage en échec, 1er novembre

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## Récupération STOP3 et worktrees — 1er novembre

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## Moteurs Jev et masquage d'audit — 1er novembre

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## Transport d'arrêt restreint — chantier en validation, 1er novembre

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## Dashboard — refus de session, 1er novembre

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

### Cycle des worktrees — inventaire du 1er novembre

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## Authentification du gardien — 1er novembre

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## Diagnostic CLI du contrôle — 1er novembre

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## Manuel et configurations — lot publié du 1er novembre

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## Dernier lot vérifié — générateur PKI local

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## Lot précédent — journal et contrats canoniques

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## Exigences d'identité et de configuration — statut au 1er novembre 2026

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## Indicateur de pilotage — 30 octobre 2026

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## Derniers lots — fournisseurs dynamiques et décision locale

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## Candidat QA courant — journal moteur désactivé, 30 octobre 2026

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## Lot sécurité précédent — SQLite privé, 30 octobre 2026

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## Dernier lot visuel vérifié — OVPT et sombre, 30 octobre 2026

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## Inventaire initial

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## Cinq écarts prioritaires initiaux

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## Avancement du premier cycle

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## P1 gelé — logiciel, réel ciblé et rejeux QA PASS au périmètre nommé

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## Lot courant après le gel 147 — intégration du registre, non qualifiée

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

### Progrès du même lot — preuves intermédiaires distinctes

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

### Gel intégré 209 et nouveau déploiement QA

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

### Audit indépendant reçu après ce gel

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

### Gel261 — invocation du moteur et compatibilité des sessions

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

### 30 octobre — chantier filesystem en intégration

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

### Gel302 et refonte éditoriale — déployés en QA

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 13 novembre — registre de politiques egress, Store15 logiciel

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 13 novembre — broker/Full3, settlement normal sur public+E1

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).

## 13 novembre — composition des livraisons Claude Kubernetes V2 et release

[Texte archivé](CURRENT_STATE_HISTORY_20261116.md).
