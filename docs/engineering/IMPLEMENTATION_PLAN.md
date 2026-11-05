# Plan d'industrialisation

## 6 novembre — composition de l'audit retenu

La référence d'entrée est désormais `02834eb`, avec les lots capacité logique,
cohérence terminale et ACK sur l'acteur Stops. Le raccord ServiceCold possède
son propriétaire authentifié et ses tests de vraie boucle et de reçu partiel.
Ses [preuves finales](VALIDATION.md#6-novembre--propriétaire-authentifié-et-boucle-audit-restreinte)
conditionnent l'intégration ; aucun ancien PASS de primitive ne le remplace.
Les binaires, formats, rôles et frontières de déploiement sont conservés.

Priorités suivantes : drainage explicite des propriétaires déjà acceptés à
l'arrêt du service, puis reprise/recomptage de la quarantaine du writer et
qualification froide avec pannes. Ne pas confondre audit livré, finalisation
gérée, retrait du gardien ou autorisation de remplacement. Les travaux D5
protégés restent indépendants et ne sont ni repris ni réécrits ici.

Relais runtime selon sa proposition, fédération multi-hôtes réelle, admission
Kubernetes complète et qualifications Docker/Podman/cloud restent obligatoires.
Les tâches indépendantes, y compris build/tests/revue, avancent en parallèle
avec sources, targets et données distincts. Une pression mesurée explique un
essai lent ou une limite d'infrastructure ; elle n'assouplit aucun invariant.
G09 FAIL, aucun gate global clos.

## Lots intégrés le 5 novembre et priorité suivante

La référence de validation est `bfa992c`, poussée et vérifiée après intégration
par avance rapide. Elle inclut la vue des autorités du dashboard (`6c2b7f7`),
le client HTTP/journal Kubernetes (`e22e8b9`) et le port HTTP exact pour l'audit
retenu (`bfa992c`). Les preuves détaillées sont dans [VALIDATION](VALIDATION.md) :
15 fonctions Node/API et TypeScript pour la vue, 17 fonctions Rust pour le
raccord Kubernetes, trois parents HTTP et un canari existant pour le port.
Les avertissements, Clippy absent, activation Kubernetes fermée et qualifications
runtime restent des écarts ; aucune gate globale n'est clôturée.

La priorité suivante est la reprise de publication des événements V4 déjà
durables d'un worker déjà terminal au contrôle, après fermeture réelle des
propriétaires du broker. Le mode restreint et le fencing restent conservés.
Le harnais de régression a reproduit l'absence de publication de la paire V4
après réouverture réelle des propriétaires broker, contrôle restant vivant.
Le port HTTP vérifie désormais l'événement retourné sans accorder de droit au
journal. Les réservations de queue et de journal, le recoupement du terminal
central et la composition au service restent à livrer sur le même acteur,
sans élargir Stops ou Full. La queue RAII est vérifiée par huit parents et
revue indépendamment ; son raccord au port froid et le budget durable restent
à implémenter. Une façade TDD non implémentée n'est jamais intégrable.

Le WIP outbox/D5 signalé plus bas n'est ni repris ni réécrit par cette préparation.
Le raccord writer devra coordonner ses fichiers et invariants avant intégration.
Le transport runtime du relais attend la décision sur la proposition existante ;
les autres travaux continuent. Reprise froide/fencing complets, fédération réelle,
Kubernetes exploitable et qualifications Docker/Podman/cloud restent obligatoires.

## P1 — réconciliation warm après annulation

Base `361bf69`, donneur `codex/cancelled-recovery-driver-20261104` :
réconcilier uniquement une observation terminale admissible du worker déjà
possédé après Cancel durable, puis reprendre la publication du résultat retenu
après perte d'ACK. Aucun nouveau start/Arm/claim, nettoyage moteur ou retrait
de gardien. Les quatre parents ciblés utilisent Store/HTTP/Journal réels et
un runtime synthétique ; [sélection et commande](TEST_STRATEGY.md#réconciliation-après-cancel--sélection-logicielle-warm).
Les quatre parents passent sur le gel propre, puis deux canaris et la compilation
toutes cibles passent après retrait des seuls diagnostics temporaires. Aucune
qualification moteur, reprise froide, fencing ou D5 n'en découle ; le
[bilan](VALIDATION.md#5-novembre--réconciliation-logicielle-après-annulation)
distingue les gels et les limites.

Le budget Writer de 60 s est explicite et limité à cette fixture métier.
Le produit et les autres fixtures conservent 10 s. Le refus à 10 s est identifié
comme timeout de réception ; le snapshot ultérieur retrouve les ACK durables,
sans dater le commit par rapport au timeout. Mesure inclusive de 10 303 ms sur
QA à 1 CPU/debug avec hôte M3 chargé, sans mesure CPU/fsync isolé/p95 ni preuve
de performance produit. P1 reste ouvert pour la fiabilité du budget produit de 10 s,
la vraie reprise et le fencing. Un succès métier à 60 s ne ferme aucun de ces
écarts ni G09 ; aucun mandat local/cloud ou de capacité n'est réduit.

## Registre multi-hôtes — provisionnement opérateur des grants broker

Base `b77edbf`, donneur `codex/broker-grants-cli-20261104`. Core : cinq
fichiers Rust applicatifs ; Luna : tests CLI ; Root : contrats, guides EN/FR,
preuves, intégration et cycle de vie ; revue source/tests indépendante.
Les deux actions fermées réutilisent les API runner existantes, les contrôles
privés et le workflow durable. Aucun nouveau service, transport, rôle ou
schéma. Les deux défauts de validation de reçu sont reproduits puis corrigés.
Onze fonctions uniques et compilation toutes cibles PASS ;
[preuves et limites](VALIDATION.md#4-novembre--grants-broker-durables-depuis-le-cli).

Les obligations suivantes restent ouvertes : reprise broker et retrait sûr
des gardiens, transport runtime du relais sous la décision déjà soumise,
fédération complète et admission Kubernetes, puis qualifications réelles.
Au gel de ce lot grants, le harnais distinct d'observation d'un worker annulé
était conservé inachevé, sans correction applicative ni reproduction métier
revendiquée. Le suivi P1 ci-dessus décrit sa reprise ; cet ancien échec de
setup ne bloque pas les travaux indépendants.

## Build — prérequis structurels avant appel moteur

Base `dbc5afa`, donneur `codex/build-input-preflight-20261104`. Core possède
`operations.rs`, Luna les tests `runtime_program`, Root le contrat, les guides
EN/FR, les preuves et le cycle Git/QA ; revue source indépendante.

Après RED réel du CLI acceptant un paquet sans Dockerfile worker, le contrôle
vérifie 7 répertoires et 19 fichiers avant `selected_runtime`. Les quatre
canaris Build conservent leurs parcours, avec fixtures structurelles complètes.
Le contrôle ne fige pas le contexte, ne prouve pas la compilation et ne rend
pas atomiques les deux builds. Les pins système, provenance/SBOM et G12
restent ouverts. [Preuves](VALIDATION.md#4-novembre--prérequis-de-build-avant-tout-appel-moteur).
Sept fonctions ciblées, compilation toutes cibles et revue PASS ; code
`7406e4f` intégré en `fb918c9` sans autre changement applicatif.
Guides/preuves `91de7c7` composés en `c51be48`, push vérifié. Propriétaire
QA et donneur fermés normalement ; sources et preuves conservées, aucun
arbre utilisateur/protégé modifié.

## Canal worker vers relais — décision de direction en attente

Sur `f7129cf`, analyse parallèle Core/tests Luna/revue indépendante : les
trois routes worker existent, sans handoff depuis le runtime. La proposition
`exec -i`/socket tmpfs/nouveau preset est précisée dans
[DECISIONS](DECISIONS.md#4-novembre--proposition-de-transport-des-workers-sans-réseau-décision-attendue).
Deux primitives Docker réelles sont observées, conteneur possédé fermé ;
elles ne constituent pas une verticale de workers.

Après accord : contrat transactionnel d'ownership par tentative, version
du preset/checkpoint et wrapper vérifié, pont Rust spécialisé et préparation
durable des mutations, tests ciblés/revue puis vraie recette A1/A2/B1.
L'activation reste refusée tant que les prérequis ne sont pas prouvés.
Aucun contournement du contrôle inactif ni modification de D5 externe.
Les corrections documentaires de tenancy/recette avancent indépendamment.
Proposition relue et preuves `5f853f3` composées en `a8c16e0`, push vérifié ;
demande de direction soumise au mainteneur, aucun accord reçu à ce gel.
Conteneur de probe et donneur fermés normalement, preuves/branches gardées.

## Lot intégré — diagnostic de récupération sans journal

Base `26ab9e7`, worktree dédié `codex/recovery-journal-diagnostic-20261104`.
Core possède `server.rs`, Luna les régressions ciblées ; Root possède
contrats, documentation, sandbox et intégration ; revue source indépendante.
Le RED réel précède le correctif. Le contrat conserve authentification,
refus des UUID nuls et inconnues, sans initialiser l'historique de récupération.
[Preuves et limites](VALIDATION.md#4-novembre--journal-requis-absent-et-diagnostic-de-récupération).
Sept parents ciblés, compilation toutes cibles et revue PASS ; code
`3f8b33d` composé en `0281bb1` sans delta applicatif supplémentaire.
Guides/preuves `6f7d398` composés en `36d9c39`, push et références distantes
vérifiés ; propriétaire QA et donneur fermés normalement, branches et
preuves conservées. Aucun chantier D5 externe modifié.

La prochaine priorité de reprise dépend du gel coordonné du travail D5
externe ; ses fichiers protégés restent préservés. Continuer les travaux
indépendants, sans déclarer le contrôle réactivable ou Kubernetes livré.

## Lot intégré — compteurs fiables et snapshot de présence

Base00876e3, donneur `codex/presence-cost-20261104`. Root intègre et possède
docs/sandbox/lifecycle ; Core possède instrumentation et getter/acteur,
Luna tests/exécution, reviewer source indépendant. Aucun arbre D5/FD modifié.

- [x] Reproduire le compteur manquant :1vs6, puis attribution des ancêtres
  au scope appelant et deux consommateurs concurrents PASS.
- [x] Baseline réelle N0/8/32 avant changement du getter, préparation hors
  fenêtre et contrôles durables après reopen ; aucun worker/modèle.
- [x] RED getter16fsyncvs0, puis cache non autorisant, contrôles complets
  avant effet préservés et sept parents ciblés uniques PASS sur les gels.
- [x] Mesure après changement : àN32,262→198sync fichiers et831→634sync
  répertoires, all-targets final PASS. Tmpfs/debug, aucun SLO physique.
- [x] Revue finale, guides/preuves, commit/push/composition et fermeture
  normale du donneur, en conservant sources/journaux privés :c9f5d49 intégré
  en6af1081, guides/preuves0416db4, références distantes conformes et retrait
  normal après inventaire et probe de consommateurs.

Le coût reste dépendant de N et la rétention n'est pas qualifiée. Full3
protégé contient un draft et un correctif outbox WIP non validé ; coordination
de sa reprise demandée au mainteneur, sans déduire activité d'un fichier.
Le contrôle froid reste volontairement inactif tant que continuité d'origine,
claims, exclusion de l'ancienne autorité et effets ambigus ne sont pas prouvés.
Kubernetes conserve admission fermée : canal/preset et attachement à la bonne
instance exigent conception/revue/décision avant réalisation. Aucun de ces
écarts n'est fermé par le coût publisher ; G09 reste FAIL.

## Lot intégré — présence émise par le vrai service broker

Basea76730c, donneur `codex/broker-publisher-20261104`. Ce raccord
publie la présence de `broker serve`, avec clé Broker distincte, périmètre
frais issu de la même preuve, ancre locale privée et pending/reçu durables.
Le service possède et joint ses acteurs ; panne réseau/auth/budget de
présence ne modifie ni admission ni STOP/quota. Des exclusions de secrets
devenues non fiables interdisent les nouvelles actions par leur contrôle
de sûreté existant. Le mode recovery-only n'ouvre pas la configuration
présence et ne démarre aucun publisher.

Root possède les raccords profil/serveur et les guides, CORE le module Rust
et le calcul partagé du hash exact, Luna le test de vraie CLI et l'exécution
isolée ciblée, reviewer indépendant la conception puis le delta. Les owners
sont distincts dans le donneur ; les checkouts D5/FD restent intacts.

- [x] RED sur la vraie CLI de base, config Registry2 avec `presence_file` ;
  vrai gateway/Store et aucun job/moteur/modèle. Puis implémenter le raccord.
- [x] Load en lecture seule, état0700 précréé, union de toutes les clés et
  protections avant persistance ; caps64 existants préflightés, sans
  élargissement ni troncature. Journaux bornés, FD historiques non retenus.
- [x] Pending complet synchronisé avant POST, réserve du reçu avant effet,
  reprise du même UUID sans nouveau battement/audit ; un quota atteint
  n'interdit pas l'ACK du pending déjà compté. Hash serde historique conservé.
- [x] Nouvelle incarnation : conserver la révision heartbeat globale et
  commencer séquence1 seulement sans heartbeat de cette incarnation. Une
  preuve tournée avec pending ambigu exige réconciliation explicite.
- [x] Tests pertinents FS/HTTP/Store et vrai cycle `serve`, contrôle composé,
  revue traitée, guides/exemple EN/FR et preuves :21fonctions uniques PASS,
  all-targets PASS, sans replay systématique des parents inchangés.
- [x] Commit/push, composition vérifiée et retrait sûr du donneur :8ead8cc
  intégré2949b54, guides/preuves8d9abc7, références distantes conformes et
  donneur retiré normalement après inventaire/consommateurs ; preuves conservées.

Le plafond local et les budgets d'audit sont cumulatifs : aucune présence
indéfinie ou purge qualifiée n'est annoncée. Worker relay, fédération complète,
reprise runtime et qualification production gardent leurs critères entiers.
Le lot suivant ci-dessus mesure le coût du flux et retire une relecture
redondante en conservant les contrôles avant POST et leurs régressions.
Le diagnostic Recovery sans journal reste à préciser. Kubernetes exige une
proposition de canal/preset avant réalisation et un attachement sûr à
l'instance autorisée ; un handle DTO ou GET suivi d'exec par nom ne le prouve pas.

## Lot intégré au4novembre — portée authentifiée du broker

Base1da19fd, donneur dédié `codex/broker-presence-20261104`. Le backend
possède déjà grant, heartbeat, lecture technique et projection humaine ;
à ce gel, `broker serve` ne publiait pas encore sa présence. Une clé Broker ne peut pas
utiliser le protocole runner et `GET /brokers/self` ne porte pas tenant,
namespace de contrôle ou révision du binding. Le prochain consommateur doit
obtenir ces faits de la même preuve authentifiée, sans les déduire d'un
autre credential ou de labels locaux.

- [x] Ajouter `GET /api/v1/brokers/identity`, via `VerifiedBroker` existant,
  avec DTO fermé version1 : tenant et namespace serveur, version du schéma,
  broker, incarnation, credential et révisions binding/grant. Une transaction
  contrôlée revalide identité, horloge et grant avant de produire le snapshot.
- [x] Reproduire l'absence de route avant code ; vérifier preuves Broker A/B
  avec IDs identiques, refus des autres rôles, rotation/révocation et portée
  inchangée. Les tests utilisent vrais Gateway/Store ; aucun heartbeat fictif
  n'est ajouté à un service existant.
- [x] Revue indépendante, Luna ciblé dans Docker isolé, contrôle composé des
  cibles et guides API/EN/FR synchronisés ; commit/push puis fermeture normale
  du donneur. Root possède intégration et docs, CORE les cinq fichiers
  API/domaine/store/tests, Luna seulement l'exécution.

Résultat :6fonctions uniques et all-targets PASS. Le RED de collision de
secret précède sa garde ; l'OOM de compilation mesuré au premier propriétaire
est conservé, puis trois unités passent sur source inchangée dans un
propriétaire neuf, sans replay des trois API. Codefc22ca9 intégré87e902d,
guides API/EN/FR synchronisés et donneur retiré normalement ;
[preuves et limites](VALIDATION.md#4-novembre--identité-broker-liée-au-même-périmètre-serveur).

Ce snapshot prouve la portée à sa lecture, pas une autorité durable, un
service vivant ou le succès d'un POST ultérieur. Le publisher lié au vrai
cycle de `serve`, sa pending operation durable, les changements de révision
et son arrêt restent à implémenter. Le replay heartbeat dépend aussi des
révisions de la preuve et du grant ; une rotation ne permet pas de deviner
une nouvelle séquence. Les budgets d'audit cumulatifs interdisent une boucle
de retry sans borne. Aucun endpoint ne ferme les gates de fédération.

En parallèle, l'inventaire opérateur du cluster Docker Desktop confirme
contexte local `docker-desktop`, serveur/nœud1.36.1, un nœud Ready,8CPU et
12290272Ki de mémoire, sans pression signalée. C'est une lecture de
métadonnées, pas l'exécution du produit. Doctor et dry-run réels restent
NOT_RUN, avec préparation d'une identité locale restreinte et isolement du
test à effectuer ; aucun besoin d'autorisation cloud n'en est déduit.
Le profil lecture public est ensuite créé sans remplacement ; les sept
décisions RBAC opérateur passent. Le profil Pod-create reste non appliqué,
sans Pod ni token émis ; [preuves et limites](VALIDATION.md#4-novembre--préparation-rbac-kubernetes-docker-desktop).

## Lot en cours au4novembre — CLI des identités techniques

Le lot logiciel ci-dessous est implémenté et vérifié :17fonctions uniques
ciblées, all-targets et release PASS, revue indépendante traitée, guides
et modèles EN/FR. Les RED et échecs de fixtures restent dans
[les preuves](VALIDATION.md#4-novembre--cli-durable-des-identités-techniques).
Clippy Linux est BLOCKED ; les limites de faute physique, transport et
fédération restent ouvertes. Les étapes suivantes conservent les critères
du cycle ; les cas réalisés et ceux encore ouverts sont distingués à
l'étape4 et dans les preuves, sans qualification implicite.

Base73707d1 dans un worktree dédié. Contrat existant issue/revoke et reçu
workload conservés ; [décision et limites](DECISIONS.md#4-novembre--provisionnement-des-identités-techniques-par-cli).
Propriétaires : CORE nouveaux `src/workload_cli.rs`, `src/workload_cli/*`,
`tests/workload_cli.rs` ; Root seul pour exports lib/main, intégration et
guides/preuves ; Luna exécute dans un OS isolé ; second regard indépendant.
Pas de compilation native, téléchargement ou campagne fournisseur/runtime.

1. RED : nouvelle CLI prepare invoquée par un test sur le binaire réel
   de73707d1 ; échec attendu commande absente, compilation du test réussie.
2. Implémenter types fermés et prepare locale : CAS/UUID/sujet/expiry,
   secrets CSPRNG durables, destination neuve, bornes, stdin, refus sûrs.
3. Implémenter apply/resume/status avec scope immuable et runner courant,
   un POST exact, erreurs opaques, résultat historique, publication du reçu
   et comparaison en normalisant seulement `replayed`. Une expiry passée
   en reprise ne bloque pas la récupération d'un reçu historique.
4. Vérifier par vraie CLI, gateway/SQLite et TLS possédés : préparation avant
   HTTP, perte de réponse après commit/restart/replay, rotation/révocation,
   replay après mutations/expiry, CAS/contenu divergent, isolement A/B,
   audit indisponible, reçu invalide/bornes, liens/substitutions/verrou,
   publication interrompue et confiance/nom TLS, sans mock métier permanent.
   Réalisés :17fonctions listées dans VALIDATION, dont réouverture propre,
   mutations historiques, réponses altérées, identité Worker projetée,
   refus A/B, scope, fichiers et TLS, puis expiration réelle avant replay,
   CAS divergent et capacité d'audit réellement indisponible. Encore NOT_RUN :
   faute fsync/medium et interruption brutale de publication. Les autres
   suites API existantes ne sont pas transformées en ces preuves CLI.
5. Rustfmt ciblé, tests sélectionnés avec comptage effectif et all-targets ;
   Clippy seulement si disponible sans installer. Revue indépendante puis
   traitement des constats et contrôle composé, sans rejouer la matrice.
6. Guides EN/FR et exemples de manifeste/control, rotation/reprise/révocation,
   capacités et limites ; preuves/gates actualisés, commit/push vérifiés,
   fermeture du worktree donneur après contrôle des consommateurs.

Ces critères ne livrent pas l'activation automatique d'un consommateur ni
le transport réseau des workers. Ils demeurent des chantiers obligatoires
distincts ; la cible produit complète et les gates restent inchangés.

Prochaine priorité : raccorder les preuves workload aux brokers/consommateurs
réels et au registre, puis qualifier leur visibilité et révocation sans
distribuer de credential de contrôle aux workers. Le transport worker et
sa frontière attendent leur décision structurelle existante ; ne pas la
contourner avec un endpoint générique, socket ou accès réseau général.
Reprise D5/arrêt/retrait sûrs, fédération multi-hôtes et Kubernetes complet
restent en parallèle selon les propriétaires et dépendances disponibles.

## Priorités au 4 novembre — validations et raccords parallèles

1. L'inventaire STOP est intégré dans la branche de validation par3acd5cc :
   découverte des UUID, auth fraîche, lecture partielle explicite et limite
   d'un scan en file ;24tests uniques ciblés et all-targets PASS. Le mode
   récupération froid est explicite. Cela ne ferme ni D5 ni G09.
2. Qualifier le préflight Kubernetes Rust sur la cible Desktop : identité
   serveur, audit avant POST dry-run et résultat avant succès sont intégrés
   par96bf8ed,24tests ciblés et all-targets PASS,
   revue sans blocage statique, guides/exemples EN/FR livrés ; compilation
   composée broker+Kubernetes PASS, aucun cluster sondé.
   Compléter ensuite le vrai lancement/reprise de pods, sans confondre cette
   inspection avec le runtime obligatoire.
3. Le vrai SSO CLI a franchi login/PKCE, identité/contexte et lecture autorisée.
   L'accusé OIDC explicite est intégré avec
   27tests produit ciblés PASS et revue ; profils/guides EN/FR sont intégrés,
   trois profils Rust/build/all-targets composés PASS. Préparation stdin PASS,
   chemin fichier RC1 à diagnostiquer séparément. Le cookie privé quoté passe11tests
   après RED, sans reclasser G. H passe ensuite sur le vrai Keycloak local :
   login/lecture/refresh/rotation/lecture/logout et audits,13phases ;17tests
   purs du harnais PASS avant cette unique scène LinuxARM64. Restent
   nouveau profil Web composé, CLI macOS/TTY, entreprise/TLS et refus adverses.
4. Poursuivre relais runtime, fédération multi-hôtes et qualifications réelles.
   Les worktrees D5 protégés restent intacts. Tests Luna ciblés et compilations
   sérielles sur le Mac sous pression ; aucune matrice E2E systématique.

Les [preuves](VALIDATION.md) et l'[état](CURRENT_STATE.md) bornent ces
résultats ; les exigences production et les douze gates restent entières.

## Priorités au 3 novembre — audit en cours

1. A/B/C intégrés et poussés jusqu'à `2eeaa0d`, avec RED/GREEN ciblés, revues et
   guides EN/FR. Fermer les contrôles qualité manquants sans transformer ces
   correctifs logiciels en qualification production ; [preuves](AUDIT_FOLLOWUP.md).
2. E intégré dans `808f08c` et clarification API `14d5eb8` : quatre RED avant
   correction, sept régressions et sept canaris OIDC GREEN, puis oracle renforcé
   exact PASS avec production inchangée ; revue et guides EN/FR acceptés.
   D intégré dans `b168784` après RED, corrections de revue, 59 fonctions
   uniques ciblées et contrôle composé de toutes les cibles PASS.
   Aucune sévérité ni performance déduite d'un délai sur le Mac sous pression.
3. F est intégré : SSE `297376f`, budgets `aa4eef9`, client `f5b035a`, après
   RED/GREEN ciblés et revues. Cinq tests du raccord composé et la compilation
   de toutes les cibles passent sur `aa4eef9`.
   Le risque FD est revalidé sur `b168784` ; concevoir son correctif et
   préserver le témoin du commit connu. La conception FD via Daybreak attend
   l'authentification matérielle de session, sans bloquer les autres travaux.
   Rapprocher tout nouveau constat du rapport final
   avec le code courant, y compris les lots déjà intégrés. Le retry du runner
   reste une proposition séparée sans décision du mainteneur.
4. Les trois chantiers parallèles sur `2c40097` ont livré un premier sous-lot
   Kubernetes readonly (`1b61f4d`) et le backend workload/registre/relais
   (`c762a24`), après86tests ciblés et contrôles composés sans avertissement.
   Les périmètres ci-dessous ne qualifient ni transport worker, ni fédération,
   ni lancement Kubernetes. Les worktrees D5 protégés restent intacts, et les
   exigences cloud/runtime/identité/exploitation/capacité100 restent entières.
   Les résultats du 2 novembre
   restent historiques ; les blocages DNS indiqués ci-dessous ont depuis été
   levés dans leur portée documentée.

## Cycle logiciel intégré — trois périmètres de développement

| Worktree | Propriétaire | Périmètre |
|---|---|---|
| `kubernetes-adapter-20261103` | Agent runtime | Nouveaux `runtime/kubernetes*` et tests dédiés ; premier sous-lot readonly, admission fermée tant que les garanties manquent. |
| `worker-relay-20261103` | Agent relais | Nouveaux `relay.rs`, `audit/relay.rs` et tests API/contrats ; aucun fichier global partagé. |
| `broker-registry-20261103` | Agent cœur | Nouveaux `broker_registry*`, `workload*`, audit/replay, migration6 et capacité workload ; helper historique worker et adaptation contrôlée des schémas. |
| Intégration | Root | Contrats transverses, API/auth, `lib.rs`, `main.rs`, configuration, composition et clôture. |

La migration publique est `migrate_workload_v6`, explicite5→6 avec
backup/rollback et validation de tous les objets. Elle ne réactive aucune
autorité de supervision perdue. Une seule migration commune initialise
registre, bindings et relais ; pas de schémas6 concurrents par agent.

Prochaine priorité : raccorder un vrai worker network:none au relais et
exécuter la recetteA1/A2/B1. La proposition privée du4novembre compare UDS et
exec non-TTY ; sa revue exige de fermer ordre Running/release, ambiguïté de
release, incarnation/epochs et rotation avant décision structurelle/code.
Le backend seul ne clôture pas ce chantier. Reprise D5, fédération/UI humaine,
admission Kubernetes, rétention/WSI-Q02 et qualification réelle restent au
plan ; aucune obligation n'est reportée sous une étiquette MVP/V2.

Au4novembre, la proposition du canal exec fermé est relue et soumise au
mainteneur dans [DECISIONS](DECISIONS.md#4-novembre--proposition-de-transport-des-workers-sans-réseau-décision-attendue).
Aucune implémentation/probe de ce canal avant sa décision. Deux travaux
indépendants avancent en parallèle : CLI Kubernetes readonly désormais
intégré (`bebdd90`, quatre tests et all-targets PASS après RED/revue), puis
conception de la lecture des brokers depuis un contexte humain autorisé.
Cette lecture doit conserver audit de lecture, budgets, pagination liée au
contexte et révocation ; aucun accès runner dans le front ni faux inventaire
multi-hôtes. Ses contrats/code/tests restent à livrer séparément.

Le [contrat humain du registre](DECISIONS.md#4-novembre--contrat-de-lecture-humaine-du-registre-brokers)
est maintenant relu : nouvelle capacité explicite sans élargissement des
grants historiques, audit distinct et migration7. Avant son code, intégrer
le correctif existant du proxy qui masquait le429 F1 en503/retry. Il est
intégré dans `167ff85` après RED/GREEN ciblés et revue. Ensuite propriétaires
disjoints backend Rust (API/audit/migration/compatibilité) et frontend
TypeScript (modèle/client/vue humaine/états), Root aux contrats/intégration,
Luna aux tests ciblés et second regard indépendant. Aucun raccord de workers
ou accès physique aux moteurs n'est implicitement livré par cette vue.

Commission depuis `5b6dfc1`, worktrees indépendants :

| Périmètre actif | Propriétaire | Livraison attendue |
|---|---|---|
| `human-brokers-core-20261104` | Agent Rust | Capacité non délégable, audit/migration7, API/pagination bornée, compatibilité historique et tests de contrats. |
| `human-brokers-front-20261104` | Agent TypeScript | Modèle/client/vue humaine, landing broker seul, états et purge/latch, tests ciblés. |
| Validation | Luna | RED/GREEN séparés, un Cargo lourd à la fois, montages source RO et limites OS ; aucune matrice fournisseur systématique. |
| Intégration | Root | Contrats, documentation EN/FR, revue indépendante, composition, push et retrait propre des worktrees. |

Ces livrables sont en développement, pas disponibles par la seule commission.
Le contrôle de manifeste setup est dans `src/setup.rs` : refuser la nouvelle
capacité avant publication sous5, sans modifier l'installation5 existante.

Checkpoints source poussés : backend `d3baf0c`, frontend `514ca12`, maintenant
composés avec les douze guides/index EN/FR sur la branche de validation
`codex/human-brokers-validation-20261104` à `9ee05cc`. Les
64fonctions Rust uniques ciblées,35Node et all-targets passent dans les gels
attribués par [VALIDATION](VALIDATION.md#4-novembre--vue-humaine-des-brokers-gels-en-cours).
Luna a vérifié54hashes, sans test supplémentaire. Les deux worktrees donneurs
sont fermés après contrôle, branches/proofs conservées ; le worktree de
validation possède le prochain cycle. Prochaine étape : préparer les outils
publics épinglés après l'accord déjà demandé, puis Clippy, build et navigateur
ciblé, enfin parcours Web composé et revue de composition. Retirer le worktree
de validation après intégration et vérification des consommateurs, sans perdre
branches ni preuves. Aucun lancement, fédération ou runtime n'est qualifié par
ces checkpoints. Les travaux indépendants du registre ne valent pas décision
sur le transport worker soumis au mainteneur.

En parallèle, préparer un seul scénario CLI SSO avec le Keycloak déjà en cache,
driver Rust privé et contrôles du sandbox vérifiés avant tout service ; les
essais auxiliaires restent hors distribution. Ce parcours ne dépend pas de
Playwright/Vite et ne doit pas réutiliser les services ou données existants.
L'analyse WSI-Q02 conserve les garanties et exige une caractérisation à
compteurs avant choix d'une optimisation ; ne pas échanger intégrité contre
un coût constant supposé. La capacité100 reste à qualifier réellement.

La parallélisation s'applique aussi à la validation et aux guides : l'auteur
traite un échec isolé pendant que Luna exécute les tranches indépendantes
sur un snapshot immuable, et un troisième agent prépare les guides EN/FR
dans les seuls fichiers documentaires attribués. Les revues restent
indépendantes des auteurs. Ne pas multiplier les compilations lourdes ni
les matrices E2E pour occuper les slots ; chaque test doit fermer un critère
du lot. Avant intégration, contrôler la composition backend/frontend,
traiter les constats, pousser un lot cohérent puis fermer normalement ses
worktrees après vérification des consommateurs et conservation des preuves.

Les preuves broker/worker sont opaques et revalidées en transaction avant
replay. Le tenant vient du credential ; la rotation ne crée ni nouveau worker
ni crédit d'audit. Génération v1 fixée à1 par tentative UUID tant qu'aucune
transition centrale durable n'existe. Le worker ne possède aucun runner/SSO
ni socket moteur. La phase d'émission préparatoire du credential ne donne
aucun accès avant Running observé, inspection, quota et lease active.

Le budget workload est distinct du budget de lecture humaine, sous C+I,
avec cumul par tenant et principal canonique worker/broker, cost UTF-8 exact
et projection du préfixe committé. Grants/issue de contrôle sont ordinaires,
révocations passent par une couture maintenance privée. Aucun JSON/kind ne
sélectionne une classe privilégiée. Les erreurs de DB/audit ne deviennent
pas un inbox vide ; ACK et heartbeat ne prouvent pas une exécution runtime.

Chaque gel reçoit tests Luna ciblés et revue indépendante avant commit/push.
Historique du premier gel Kube : huit parents PASS ; deux échecs préparatoires
du harnais (shim rustup puis scratch noexec) n'ont exécuté aucun test et sont
conservés séparément. Les compléments mTLS/cancellation portent ensuite le
total à11 ; le CLI a ses quatre tests propres. Les modules relais/registre
ont depuis été intégrés avec86fonctions ciblées et all-targets PASS, dans
leur seul périmètre logiciel. La qualification réelle A1/A2/B1,
Podman/Kubernetes, reprise broker et charge100 garde ses propres gates.

## Lot F — plan de correction après reproduction

> Pour les agents : appliquer `superpowers:subagent-driven-development` ou
> `superpowers:executing-plans`, avec tests Luna et revue indépendante.

**But :** un lecteur humain ne doit pas épuiser la capacité nécessaire aux
décisions, aux révocations ou aux autres lecteurs de son tenant.
**Architecture :** même chaîne et schéma5 ; classes d'admission internes,
compteurs reconstruits depuis les reçus validés et cache du seul préfixe
committé. Les flux ont une admission distincte, sans retenir les permis I/O D.
**Stack :** Rust, SQLite, Axum/Tokio existants, sans dépendance supplémentaire.
La conception privée `ai-guard-audit-f-budget-design-20261103.md` et sa revue
`ai-guard-audit-f-design-review-20261103.md` dans `/private/tmp` fixent les
risques ; les arbitrages applicables sont repris ici. Aucune réalisation de
ce plan n'est encore annoncée comme vérifiée.

Contraintes et arbitrages :

- Budgets cumulés événements/octets UTF-8 du payload préparé exact. Défauts :
  contrôle 1/4, identité 1/8, humain tenant 1/2, humain principal 1/8 des capacités
  physiques. Une configuration opérateur explicite peut les modifier après
  validation ; elle ne remet jamais le cumul à zéro.
- Admission par port Rust privé, jamais par `kind`, label ou JSON. Lecture
  humaine sous réserves contrôle+identité ; écriture ordinaire sous réserve
  identité ; maintenance d'identité sous plafond physique. Avant schéma5,
  conserver le contrat physique historique.
- Très petits plafonds : ouverture conservée, lectures humaines désactivées
  explicitement si les défauts ne sont pas représentables ; aucun relèvement
  de capacité. Une saturation physique existante reste 503.
- Refus logique humain 429 `audit_read_budget_exhausted`, action opérateur,
  sans `Retry-After`. Aucun rendu sans reçu ni effacement conseillé. Un profil
  voisin refusé reste partiel selon B ; ne pas promettre 429 pour toute
  agrégation OIDC.
- Flux humains : 4 par principal canonique, 24 par tenant, dans les 32 totaux
  existants. Les permis restent possédés jusqu'au drop réel du Body, même
  après erreur terminale. Aucun timer de restitution. Un client non pollé peut
  retenir sa part ; aucune borne de tous les sockets HTTP n'est revendiquée.

Points de revue à prouver : rotation/contexte ne rechargeant pas le principal ;
bytes nonASCII et limites inclusives ; rollback multi-append sans débit publié ;
sibling/externe/`integrity()` sans cache périmé ; Body jamais pollé ou retenu
après erreur sans fuite ni restitution anticipée.

### F1 — budgets durables

Propriétaire : agent core dans `audit-human-budget-20261103`, pour `audit.rs`,
`audit/{capacity,capacity_tests,human,identity}.rs`, `gateway_config.rs`,
`main.rs`, `identity_cli.rs` et la seule couture d'ouverture de
`identity_cli/offline.rs`. Aucun fichier API ou doc partagé.

- [x] Reproduire le défaut avec `tests/human_read_budget.rs` gelé, sans moteur
  ni oracle de latence : head64 et contrôles refusés, RC101.
- [x] Implémenter les classes privées, coûts exacts et projection committée ;
  conserver replay identique, conflits et témoins de D.
- [x] Exposer `AuditCapacityPolicyV1`, `AuditCapacityFailureV1` et
  `Store::with_capacity_policy` ; raccorder registre et fichier privé CLI,
  propager la politique aux siblings et afficher un diagnostic sans secret.
- [x] Faire exécuter par Luna le RED inchangé devenu GREEN et les unités
  count/bytes, réserves, rollback/replay, reopen/sibling/externe et coût stable.

### F2 — admission des flux, en parallèle

Propriétaire : agent API dans `audit-human-streams-20261103`, pour `api.rs`,
`api/human.rs`, `api/human_streams.rs` et `tests/human_stream_capacity.rs`.
La petite traduction des erreurs publiques F1 est raccordée après composition.

- [x] Écrire puis faire exécuter le RED : quatre Bodies A1 retenus, le
  cinquième doit être refusé ; le code actuel accepte encore ce cinquième.
- [x] Implémenter admission non bloquante, RAII et registre canonique purgé.
- [x] Vérifier autres credentials/contextes, A2, réserve technique, acquisitions
  partielles, drop avant poll et Body conservé après erreur terminale.

### Composition et clôture

- [x] Root compose les fichiers disjoints dans un staging dédié ; Luna exécute
  les canaris pertinents (dont72pages/73reçus), pas une matrice fournisseur.
- [x] Revue indépendante d'implémentation, correction de ses bloquants,
  formatage, contrôle des cibles touchées et guides EN/FR synchronisés.
- [x] Après preuves et revue seulement : commit, push, état/limites mis à jour,
  contrôle des consommateurs et fermeture normale des worktrees sources.

FD et le retry du runner ne font pas partie de cette correction. L'archivage
qualifié reste un écart d'exploitation : aucun quota fini ne promet des
lectures illimitées, ni une réserve historiquement consommée recréée.

## Priorités au 2 novembre

1. Nouvelle image SSO `sha256:72bfb9119fe552c6100ffaeb07cbbd8bd86f9449270b19f3904e97ca2dd8c5c7` déployée après le correctif `complete` ; parcours Web réels user-1 (7contrôles PASS) et user-2 (8contrôles PASS) consignés avec leur portée. L'essai administration a rencontré un oracle de lecture vide dont l'attente source est invalide ; le harnais corrigé est préparé, sa recette reste ouverte. Les24tests CLI et leurs checks précédents restent des preuves historiques attachées à leurs gels. [État courant](CURRENT_STATE.md#préparation-sso-locale--intégrée-recette-complète-ouverte).
2. Terminer la recette administration Keycloak/Web puis le CLI natif sur le bootstrap local réellement démarré. Le gateway reste propriétaire PKCE ; le DNS hôte manquant garde le CLI `BLOCKED` jusqu'à l'intervention administrateur. Les parcours humains Docker A/B du gel7afdd43, user-1/user-2 Web et un oracle préparé ne qualifient ni ce CLI ni un worker ; G09 reste FAIL.
3. G09 reste FAIL sur `d6cb136` :275,75s, refus avant claim, worker Created et Arm5s déjà expirée à la première vérification Ready ; aucun Running/SIGKILL ni effet d'arrêt autonome exercé. Instrumenter d'abord le chemin durable/reopen ; la compilation diagnostique isolée a été interrompue avant test pour priorité SSO, NOT_RUN. Reprendre ensuite une seule recette bornée avec le worker générique construit/inspecté `sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`, sur le gel courant. Préserver release, gardiens, volumes et preuves ; aucun budget Arm augmenté ni ancienne ressource utilisée pour nettoyer. [Preuve G09](VALIDATION.md#2-novembre--g09-release-courant-refus-avant-claim).
4. D5 reste ouvert : préparer puis implémenter le [vertical D5-U ci-dessous](#mise-en-œuvre-d5-u--original-inutilisé-2-novembre-conception-non-implémentée), preuve originale consommable → Pending audit-ACKé → Closing → tombstone → vrai join → Q rejouable → Final → capacité libérée une seule fois, avec reprise avant Restore. Un parseur V5 isolé ne constitue pas ce lot. Registre multi-hôtes, lancement délégué, relais worker et Kubernetes restent obligatoires et ouverts.

Ces priorités actualisent les points de reprise historiques ci-dessous ; elles ne suppriment aucune exigence. Un échec de test ou une dépendance indisponible est conservé avec sa portée, sans arrêter les tâches indépendantes.

## Mise en œuvre D5-U — original inutilisé, 2 novembre, conception non implémentée

> Exécution future : `superpowers:subagent-driven-development`, trois propriétaires ci-dessous, Luna pour les validations après gel et revue indépendante avant intégration. Cette section applique `writing-plans` au plan existant ; elle ne constitue ni une API disponible ni un résultat de test. Les noms et coutures ci-dessous sont des choix précis proposés à la validation technique de Root, dans l'architecture déjà approuvée ; aucune nouvelle approbation humaine du mandat n'est requise.

**But :** retirer sûrement une Arm d'un **nouveau** gardien lorsque son acteur original vivant n'a jamais consommé Prepare, réservé de claim/émission ou réservé de mutation STOP. L'abandon explicite avant claim doit aboutir à P→C→T→join→Q→F, sans réactivation au rejeu.

**Architecture et technique :** mêmes broker/gardien Rust, IPC Unix privé, journal révisionné et outbox ; SQLite par tenant et frontières local/cloud inchangés. Aucun composant/service, moteur de stockage, PKI ou gestionnaire OS ajouté. **Spécification normative :** [contrat de retrait](BROKER_SUPERVISION.md#contrat-de-retrait-du-gardien--conception-à-implémenter-1er-novembre), notamment preuves initiales, engagements, ordre vivant, même FD/budgets et reprise. Le plan en précise les coutures proposées, sans réduire ses acceptations.

### Contraintes communes et modes de défaillance à couvrir

- Préserver les plans1–3, bytes/domaines des checkpoints1–4 et les échanges V1/V2/Stop3 existants. Stop3 reste StopsOnly/Unverified, `admission_available=false`, retraits Unknown. Full3 est une **nouvelle connexion explicitement demandée**, refusée sans ledger Complete sur rôle neuf scellé ; aucun upgrade Stops→Full, repli V1 ou adoption implicite d'une racine existante.
- Conserver le même FD O_RDWR, description/flock/inode jusqu'au dernier acteur/opération ; vérifier owner/euid, mode0600, nlink1, ACL, parents et identité native. Le nouveau cache spécialisé Parsed/Tainted ne change pas `PrivateFile` général. Aucun reopen par chemin, replace/unlink/truncate, unlock/relock, reset d'auth/genesis ou nettoyage d'un suffixe invalide.
- Factory avec preuve opaque de création exclusive ; même `role_id/init_id`, genesis/current puis sceau/Floor0 synchronisés avant admission. Initialisation interrompue : mêmes bytes et init seulement sous preuve retenue ; preuve perdue ⇒ diagnostic, aucun réamorçage automatique.
- Records fermés/canoniques et domaines **exactement** ceux du contrat : identité/preuve, genesis/sceau, floor/delta/entrée, Closing/tombstone, FinalizationCore/reçu, Joined/Recovered distincts, checkpoint V5. Préfixes de domaines terminés par NUL. Aucun cycle de hash : Closing ne contient pas sa future tombstone, core/reçu ne contiennent pas leur futur head ; head séparé dans la réponse authentifiée.
- Caps gardien normatifs :1024 Arms,1Gio logique cumulé,4096révisions,8Mio/record ; lock4096floors,2Kio/frame BE32,8Mio cumulés. Compter records, cadrage, pointeurs, suffixes et réservations réelles ; ces maxima ne sont pas simultanément garantis. Avant Closing réserver aussi les deux deltas suivants, tombstone/floors/pointeurs promis. Budgets broker `JournalLimitsV1`/checkpoint/outbox distincts : réserver Final+événement final **avant Pending**, cumulativement, sans réduire le budget Cancel.
- Aucun IO, moteur, HTTP, fsync ou join sous le mutex Prepare/STOP/Closing ; acteurs STOP indépendants du mutex FS ledger. Erreur ledger ferme Full/finalisation/append pour l'epoch, mais transport structurellement vérifié et STOP connus demeurent. Horloge inconnue exige le STOP original, sans deadline neuve.
- Aucun destructor-retire, témoin depuis statut/JSON/terminal/ObserveOnly ou constructeur récupéré permissif. Q n'est ni terminal moteur, ni libération de quota/usage, ni Cleanup. Aucun arrêt/migration des sept vieux gardiens ; leur provenance source incomplète, binaires et ancres restent conservés.

| Défaillance prioritaire | Assertion d'acceptation / tâche propriétaire |
|---|---|
| Prepare/claim ou STOP gagne la course, ACK perdu, acteur restauré | D5-U refusé ; latch irréversible, aucun témoin recréé ; tâche3 |
| Append/fsync inconnu, ENOSPC, floor déchiré ou recul du seul ledger | Même FD/suffixe conservés, Tainted/Full fermé, autre STOP indépendant ; tâche2 |
| Tombstone T mais inspection/reopen/join bloqué | Aucun Q ni capacité libérée avant vrai retour du handle ; tâche3 |
| Q accepté puis ACK perdu/broker mort avant F, blob absent et Cancel concurrent | Même Q/F/event_id, P/Q→F avant Restore, port tardif sous Stops ; tâches4–5 |
| E1 Closing/Q→E2 Partial avec STOP possible→E3 Complete, ou tombstone B manquante | Ancien Q inutilisable si intervention non exclue ; B1 détecté, autres Arms avancent ; tâche5 |

### Tâche1 — figer les interfaces des trois propriétaires

**Consomme :** contrat approuvé et vrais `RegistrationV1`, `JournalScopeV1`, `RevisionRefV1`, `OutboxEventV1`, `CompletionFailure<T>`, `ProfileWriterCapabilities`, `EnforcerLeaseFile`, `NativeOwners`. **Produit :** signatures proposées ci-dessous et goldens/domaines relus, avant implémentation parallèle. Tous les noms nouveaux de cette section sont **proposés et absents** au gel `0ddd585` ; les signatures utilisent les aliases existants `EnforcerResult`/`JournalResult` et `anyhow::Result` pour le FS.

Fichiers nouveaux proposés : `src/broker/retirement.rs` (records partagés), `src/broker/enforcer/retirement.rs` (preuves vivantes et client Full3), `src/broker/enforcer/ledger.rs` (ledger), `src/broker/retirement_checkpoint.rs` (V5), `src/broker/journal/retirement.rs` (publisher metadata-only). Ajouter seulement leurs déclarations dans les parents concernés ; pas de refonte des modules existants.

| Contrat partagé proposé | Responsabilité fermée |
|---|---|
| `RetirementIdentityV1` | Lie scope/installation/job/intention, origin/tenant/namespace, Arm/reçu, engagement de la registration complète, epoch original/génération, sujet/worker/attempt/CID, autorité/invocation/profil/configuration, `retirement_id: Uuid`, motif fermé `UnusedPrepareRevoked`. Pas de secret/env/sortie brute ; engagements des données privées, sans les recopier. |
| `RetirementEvidenceV1`, `LedgerIdentityV1`, `LedgerHeadV1`, `ClosingAcceptanceV1`, `RetiredTombstoneV1`, `FinalizationCoreV1`, `RetiredReceiptV1` | Records Serde fermés, versionnés et bornés ; UUID canoniques non nuls, hashes complets. Identité/preuve/ACK immuables dans les transitions. `LedgerHeadV1` porte rôle/genesis/révision/hash séparément du reçu. |
| `UnusedPrepareRevoked`, `ClaimReservation`, `BeginUnusedRetirement`, `PendingUnusedRetirement`, `AuthorizedRetirement`, `VerifiedRetirementReceipt`, `JoinedQuiescence` | Wrappers privés, sans Clone/Serde ni constructeur public. Le premier vient du vrai acteur original révoqué ; le writer consomme cette preuve, puis l'ACK d'audit autorise Closing ; seul le client vérifié produit le reçu et seul le propriétaire du handle produit Joined. Données chargées ≠ ces permissions. |
| `SlotAuthorityV1`, `EnforcerLeaseOwnerV1` (internes, sans Serde) | Pour réutiliser `EnforcedSlot`/`SpawnGuard` sans conversion de droits : variantes `Legacy(EnforcerClient)` et `Full3(EnforcerFullClientV3)`. Pour `NativeOwners`, owner de durée de vie `Legacy(Arc<RoleLease>)` ou `Sealed(EnforcerRoleLeaseV2)` ; cette dernière retient la lease du ledger, pas une seconde acquisition. |
| `ExecutionEnforcerV1` / `ServiceEnforcerSelectionV1` (internes, sans Serde) | Dispatch conservé de bout en bout : `ExecutionEnforcerV1::{Legacy(EnforcerClient), Full3(EnforcerFullClientV3)}` ; sélection de démarrage `Legacy`, `Full3Fresh { role_name: String }`, `Full3Existing { role_name: String }` ou `StopsOnly { role_name: Option<String> }`. Aucune conversion Full3→client historique ni Stops→Full ; les méthodes de retrait n'existent que sur le client Full3 extrait explicitement. |
| `PreclaimControlV1`, `PreclaimSignalReservation`, `AbandonBeforeClaimRequest` (internes, sans Serde/Clone) | Le `Job` possède un arbitre borné par ses Arms ACKées ; le contexte en garde le même owner. Le service réserve un signal explicite ciblé sur opération/Arm/epoch/génération, jamais un témoin de révocation. L'appelant original consomme ensuite ce signal ; seule la réponse vivante de `revoke_unused` peut fournir la preuve. |
| `AbandonedBeforeClaim`, `ManagedPrearmedOutcome` (sans Serde/Clone, constructeurs crate-private) | Types opaques exportables seulement pour les signatures du runtime. `AbandonedBeforeClaim` lie la même Arm et la référence Final confirmée, sans terminal moteur. Le port préarmé retourne `ManagedPrearmedOutcome::{Executed(ManagedPrearmedExecution), AbandonedBeforeClaim { owned: OwnedContainerSnapshot, retirement: AbandonedBeforeClaim }}` ; aucun `ManagedPrearmedExecution` artificiel pour un start non émis. |
| `Full3RegistrationBindingV1` | Donnée V5 fermée : `version: u32`, `arm_id: Uuid`, `registration_hash: RetirementDigestV1`, `ledger: LedgerIdentityV1`, `original_epoch: Uuid`. Produite seulement depuis le Register Full3 authentifié, publiée avant claim, append-only et unique par Arm ; ce binding ne reconstitue ni original vivant ni droit de retrait. |
| `BrokerCheckpointV5 { version: u32, outbox: BrokerCheckpointV4, full3_registrations: Vec<Full3RegistrationBindingV1>, retirements: Vec<RetirementEntryV1> }` | Encapsule V4 sans réencoder l'historique. Les bindings Full3 ne modifient pas les anciens records Arm. Variantes `RetirementEntryV1::Pending(PendingRetirementV1)` et `Final(FinalRetirementV1)` : ordre append-only, une entrée/Arm, retirement_id unique/job, même Pending conservé en Final, aucun retour1–4. |
| `RetirementProjectionV1` | Variantes `RequiredStop(RecoveredStopV1)`, `Pending(PendingRetirementRefV1)`, `Final(FinalRetirementRefV1)` : projection propriétaire metadata-only ; les références persistées donnent identité/head/ACK, jamais témoin vivant ni permission de mutation. |

Le périmètre1 possède les données ledger ; le2 possède les wrappers vivants et IPC ; le3 possède Pending/Final/V5. Root coordonne uniquement `broker/mod.rs` et les imports communs ; un seul propriétaire par fichier. Proposition de schémas partagés ci-dessous, à relire par Root puis figer avec leurs goldens avant tout producteur/consommateur. Notation abrégée : `D` = nouveau `RetirementDigestV1` hex64 minuscule validé, `U` = UUID canonique non nul ; chaque record a `version: u32` vérifié à sa version et chaque Option est explicite. Cette notation ne crée aucun alias permissif dans le code.

| Record proposé | Membres proposés en plus de `version` |
|---|---|
| `RetirementIdentityV1` | `scope: JournalScopeV1`, `job_operation_id: U`, `intent_hash: D`, `arm: ArmV1`, `arm_receipt: ArmReceiptV1`, `registration_hash: D`, `original_epoch: U`, `generation: u64`, `retirement_id: U`, `reason: RetirementReasonV1` ; la seule variante acceptée dans D5-U est `UnusedPrepareRevoked`. La registration engagée lie snapshot/CID/autorité/invocation/profil/config ; recouper ces engagements avec Arm/reçu et le vrai slot. |
| `RetirementEvidenceV1` | `identity_hash: D`, `original_actor_nonce: U`, `prepare_revocation_nonce: U` ; engagement de la vraie révocation originale, jamais un constructeur du wrapper vivant. |
| `RetirementAuditAckV1` | `event_id: U`, `input_hash: D`, `audit_seq: i64` strictement positif ; données recoupées par writer/client, pas une permission autonome. |
| `GenesisCoreV1` / `LedgerIdentityV1` | Genesis : `installation_id: U`, `role_id: U`, `init_id: U`, `native_binding: [u64; 6]` provenant de la lease retenue. Identité : mêmes membres plus `genesis_hash: D`. Le binding natif stable est distinct du root_binding de transport lié à l'endpoint. |
| `RoleSealV2` / `LedgerHeadV1` / `LedgerFloorV1` | Sceau version2 : `ledger: LedgerIdentityV1`. Head : `ledger`, `revision: u64`, `hash: D`. Floor : `ledger`, `revision`, `delta_hash: Option<D>` (`None` seulement Floor0), `previous_floor_hash: Option<D>` (`None` seulement Floor0). |
| `ClosingAcceptanceV1` / `RetiredTombstoneV1` | Closing : `identity`, `evidence`, `audit_ack`, `closing_nonce: U`. Tombstone : `ledger`, `identity_hash: D`, `closing_hash: D`. Le hash attendu de tombstone accompagne BeginClosing et reste hors ClosingAcceptance. |
| `JoinedQuiescenceV1` / `FinalizationCoreV1` / `RetiredReceiptV1` | Données Joined : `identity_hash`, `closing_hash`, `tombstone_hash`, `original_epoch`, `original_actor_nonce`. Core : `identity_hash`, `closing_hash`, `tombstone_hash`, `joined: JoinedQuiescenceV1`. Reçu : `ledger`, `retirement_id`, `identity_hash`, `finalization: FinalizationCoreV1`. Ces records ne construisent jamais `JoinedQuiescence` ou `VerifiedRetirementReceipt` ; leur hash/reçu n'embarque pas le futur head. |
| `RetirementLedgerEntryV1` / `RetirementDeltaV1` | Entrée : `accepted: ClosingAcceptanceV1`, `expected_tombstone_hash: D`, `phase: RetirementLedgerPhaseV1` fermé `Closing`, `Tombstone { tombstone_hash: D }`, `Final { tombstone_hash: D, receipt: RetiredReceiptV1 }` ; préfixe conservé exactement, sans type récursif. Delta : `ledger`, `revision`, `previous_hash: D`, `arm_id: U`, `expected_entry_hash: Option<D>`, `transition` fermé BeginClosing/CommitTombstone/Finalize portant cette nouvelle entrée. |
| `PendingRetirementV1` / `FinalRetirementV1` | Pending : `identity`, `evidence`, `intent_event_id: U`, `intent_input_hash: D`, `final_event_id: U`, `audit_ack: Option<RetirementAuditAckV1>`, `reserved_final_checkpoint_bytes: u64`, `reserved_final_event_bytes: u64`. Final : `pending: PendingRetirementV1`, `receipt: RetiredReceiptV1`, `final_input_hash: D`. Références privées reader : `PendingRetirementRefV1 { operation_id: Uuid, head: RevisionRefV1, pending: PendingRetirementV1 }` et `FinalRetirementRefV1 { operation_id: Uuid, head: RevisionRefV1, final_entry: FinalRetirementV1 }`. |

Le gate/witness vivant, la réservation budgétaire ledger et le handle natif restent hors de ces records. Les membres abrégés `identity/evidence/ledger/accepted/closing/tombstone/receipt` ont les types nommés dans cette table ; ni `Value` arbitraire ni champ inconnu. La progression de l'ACK Pending est monotone ; Final conserve le même Pending audit-ACKé. V5 conserve sa propre version5, tous les recordsV1 leur version1.

- [ ] Revue source croisée des domaines, dépendances sans cycle, visibilité et sérialisation ; aucune compilation défaillante pour « type absent » ne compte comme RED.
- [ ] Premier test contrat stockage/factory : fonctionnalité absente au gel courant ; une erreur de compilation serait un FAIL de préparation, **pas** un RED comportemental. Après vrais producteurs de tâche2 : rôle scellé/floor déchiré ⇒ Full refusé, Journal/Cancel/STOP A conservés sur IPC réel. Le vrai B1 attend Q en tâche5.

### Tâche2 — rôle neuf et ledger durable, périmètre1 ledger/CFS

**Fichiers :** `control_fs/persistent.rs`, `broker/discovery.rs`, nouveau `enforcer/ledger.rs` et son `ledger_tests.rs`. **Consomme :** records partagés, `Directory`, `EnforcerLeaseFile`. **Produit :** lease scellée, couverture propriétaire et publications durables ; pas une livraison D5-U autonome.

Signatures proposées, en notation qualifiée `Type::méthode` (prototypes, pas code à copier) :

```rust
Directory::create_enforcer_root(&self, name: &str) -> anyhow::Result<ExclusiveEnforcerRoot>;
RetirementLedger::initialize_new(fresh: ExclusiveEnforcerRoot, installation_id: Uuid) -> EnforcerResult<Self>;
RetirementLedger::open_existing(lease: EnforcerRoleLeaseV2) -> EnforcerResult<Self>;
RetirementLedger::coverage(&self) -> EnforcerResult<LedgerCoverageV1>;
RetirementLedger::begin_closing(&self, accepted: ClosingAcceptanceV1) -> EnforcerResult<ClosingCommitted>;
RetirementLedger::commit_tombstone(&self, closing: &ClosingCommitted) -> EnforcerResult<TombstoneCommitted>;
RetirementLedger::finalize(&self, tombstone: &TombstoneCommitted, joined: JoinedQuiescence) -> EnforcerResult<RetiredReceiptV1>;
RetirementLedger::query(&self, identity: &RetirementIdentityV1) -> EnforcerResult<Option<RetirementLedgerEntryV1>>;
```

`RetirementLedger` est l'owner opaque du cache/même FD ; `ExclusiveEnforcerRoot` possède la Directory créée exclusivement (`mkdirat`, EEXIST refusé), sans Clone/Serde ni reconstruction sur existant. `LedgerCoverageV1` distingue Complete/Partial/Unverified avec identité/raison/phase ; inconnus≠0. `ClosingCommitted`/`TombstoneCommitted` sont des engagements internes liés au même ledger/head, sans faculté de construire Joined. `open_existing` n'initialise rien et ne transforme pas legacy Unverified en Complete.

- [ ] RED puis GREEN : même FD/cache avant/après append ; delta exclusif+sync fichier/parent → floor+sync du même lock → current-next/CAS+sync parent → ACK. Préfixe/EOF revalidés, écriture à EOF attendu, relecture/adoption par ce FD ; write partiel/fsync inconnu ⇒ Tainted conservé.
- [ ] RED puis GREEN : suffixe complet exact resynchronisable, partiel/divergent bloquant ; floor en avance n'autorise que le CAS exact déjà préparé. Trou/recul/hash divergent/orphelin/tombstone T ou Q absente ⇒ défaut B1, pas de reconstruction silencieuse. À C seulement, créer les bytes de tombstone déjà engagés.
- [ ] RED puis GREEN : caps réels/réservations cumulées, erreurs avant Closing, initialisation interrompue sans preuve retenue refusée, ancien binaire `max_bytes=0` refusant le sceau non vide ;400retraits/1200deltas pour formats/budgets, sans prétention de charge100.

### Tâche3 — Closing, join et replay authentifié, périmètre2 acteur/IPC

**Fichiers :** `broker/enforcer.rs`, `enforcer/{client,server,server_lifetime,recovery,recovery_v3}.rs`, nouveau `enforcer/retirement.rs` et `retirement_tests.rs`, raccord interne limité dans `src/main.rs`. **Consomme :** interfaces ledger de tâche2 et wrappers de tâche1. **Produit :** preuve originale, autorisation vérifiée, vrai join puis reçu Q rejouable ; pas de Final broker à ce stade. Le périmètre2 possède ces fichiers ; le périmètre3 raccorde les sélecteurs service/CLI de tâche4 après gel de leurs signatures, sans éditions concurrentes du même fichier.

Signatures proposées :

```rust
EnforcerFullClientV3::spawn_fresh(parent: &Directory, name: &str, executable: &Path, installation_id: Uuid, max_slots: u32) -> EnforcerResult<Self>; // async
EnforcerFullClientV3::connect_existing(root: &Directory, max_slots: u32) -> EnforcerResult<Self>; // async
EnforcerFullClientV3::register(&self, registration: RegistrationV1, generation: CancelGeneration) -> EnforcerResult<EnforcedSlot>; // async
EnforcedSlot::reserve_claim(&self, expected_generation: u64) -> EnforcerResult<ClaimReservation>;
EnforcedSlot::full3_registration_binding(&self) -> EnforcerResult<Full3RegistrationBindingV1>; // crate-private, depuis la réponse Register vérifiée
EnforcedSlot::revoke_unused(&self, retirement_id: Uuid) -> EnforcerResult<UnusedPrepareRevoked>; // async
EnforcerFullClientV3::retire_unused(&self, authorized: AuthorizedRetirement) -> EnforcerResult<VerifiedRetirementReceipt>; // async
EnforcerFullClientV3::query_retirement(&self, identity: &RetirementIdentityV1) -> EnforcerResult<Option<VerifiedRetirementReceipt>>; // async
NativeOwners::join_retiring(&self, closing: &ClosingCommitted, tombstone: &TombstoneCommitted) -> EnforcerResult<JoinedQuiescence>; // async
```

Le nouveau mode explicite de `__broker-enforcer` reçoit parent/name/installation_id et crée lui-même la racine exclusive ; aucune preuve opaque transmise en argv, aucune clé créée sur racine existante. Les modes historiques gardent leurs arguments/comportements. Full3 conserve préface2 et négociation3 authentifiée avant accord, avec records Full3 distincts ; seuls Full3 et son ledger Complete admettent Register/Prepare/Cancel, RevokeUnused, RetireUnused et QueryRetirement. Les trames Stop3 actuelles et leurs validateurs ne changent pas. Handshake6s, IO3s, frame4096/512Kio et domaines3 restent conservés ; aucun record ledger8Mio envoyé comme trame IPC. Les nouveaux DTO Full3 sont fermés, liés à session/rôle/epoch/request_id et aux engagements, jamais un JSON arbitraire routé vers Full.

Le gate du LocalSlot prend `Unclaimed → ClaimReserved` ou `Unclaimed → RetirementRevoked` irréversiblement. Reserve avant le pending claim ; échec/ACK inconnu ne le remet pas Unclaimed. Revoke ferme d'abord tous les anciens handles/guards du **slot**, puis révoque Prepare côté acteur original sous son mutex ; il ne modifie pas `CancelGeneration` partagé. La réponse authentifiée inclut le nonce vivant original et les engagements exacts ; une réponse chargée ne reconstruit pas le wrapper. ObserveOnly/restauré, Prepare gagné ou réserve STOP passée/inconnue sont inéligibles.

Retire revalide l'autorisation issue du writer, puis arbitre Closing avec Prepare et **toute réservation STOP mutative**, y compris échéance/reopen. Closing interdit Prepare, nouvel effet et reopen mutatif ; état de fermeture distinct, jamais écrasé par Ready/actor_failed. Hors mutex : C→T, réveil de sortie, drainage des inspections/reopen possédés, retour réel du handle enregistré par Arm chez `NativeOwners`, puis Q durable. Le coordinateur possède cette opération indépendamment du waiter IPC ; son abandon ne perd pas le handle ni ne déclenche un second join. Q seul permet de retirer l'acteur actif/décrémenter une fois ; l'entrée ledger reste pour le replay. Distinguer capacité active et nombre historique d'admissions : une ancienne limite monotone `actor_count` ne doit ni bloquer une capacité effectivement rendue ni effacer la borne cumulative du ledger. Un join bloqué n'immobilise ni les autres joins ni STOP.

- [ ] RED puis GREEN : original inutilisé accepté ; ObserveOnly/restauré/Prepare ACK perdu/claim réservé/STOP déjà réservé refusés ; courses avec un gagnant et aucun IO sous mutex. Test bloquant du vrai JoinHandle, pas un booléen « joined ».
- [ ] RED puis GREEN : T sans join ⇒ ni Q ni capacité libre ; autre STOP progresse. Après Q/ACK perdu, Query/Retire retrouvent le même reçu, sans nouveau thread ni décrément ; Register/Prepare/Restore de cette Arm refusés.
- [ ] RED puis GREEN : Full3 demandé explicitement seulement sur Complete ; Stop3/Full3 séparés, MAC/epoch/root/replay divergents refusés, bornes et compatibilité des vrais clients1/2/3 conservées. Une erreur ledger ferme Full pour l'epoch, sans réparation/réouverture automatique.

### Tâche4 — appelant avant claim, Pending audité et Final, périmètre3 journal/appelant

**Fichiers :** nouveaux `retirement_checkpoint.rs`, `journal/retirement.rs`, `journal/retirement_tests.rs` ; `journal{.rs,/types.rs,/confirmed.rs,/recovery.rs}`, `writer.rs`, `outbox.rs`, `runner/broker/executor{.rs,/helpers.rs,/journal.rs,/results.rs,/journal_tests.rs}`, `broker/service/{mod.rs,server.rs,jobs.rs,protocol.rs,client.rs}`, `broker_cli.rs`, `runtime.rs`, `runtime/managed.rs` et leurs tests ciblés. **Consomme :** wrappers/Full3 des tâches1–3 et vrais ports outbox/audit existants. **Produit :** entrée Full3 explicite, commande opérateur d'abandon avant claim et parcours vivant P→C→T→join→Q→F. Ce périmètre possède aussi les raccords service/CLI/runtime ; les tâches4/5 sur `server.rs` restent séquentielles.

Signatures proposées, méthodes writer asynchrones :

```rust
BeginUnusedRetirement::new(operation_id: Uuid, expected: RevisionRefV1, unused: UnusedPrepareRevoked) -> JournalResult<Self>;
WriterClient::begin_unused_retirement(&self, input: BeginUnusedRetirement) -> Result<PendingUnusedRetirement, CompletionFailure<BeginUnusedRetirement>>;
WriterClient::authorize_unused_retirement(&self, pending: PendingUnusedRetirement) -> Result<AuthorizedRetirement, CompletionFailure<PendingUnusedRetirement>>;
WriterClient::bind_full3_registration(&self, operation_id: Uuid, expected: RevisionRefV1, binding: Full3RegistrationBindingV1) -> JournalResult<CheckpointReceiptV1>;
StopWriterClient::finalize_retirement(&self, receipt: VerifiedRetirementReceipt) -> JournalResult<CheckpointReceiptV1>;
service::serve_selected(config_path: &Path, selection: ServiceEnforcerSelectionV1) -> anyhow::Result<()>; // async
BatchExecutor::new(writer: WriterClient, enforcer: ExecutionEnforcerV1, clock: Arc<dyn HostClock>) -> Self;
Job::request_abandon_before_claim(&self, input: AbandonBeforeClaimV1) -> Result<PreclaimSignalReservation, BrokerErrorCodeV1>; // synchrone, privé
Execution::retire_unused_before_claim(&self, slot: EnforcedSlot, request: AbandonBeforeClaimRequest) -> anyhow::Result<AbandonedBeforeClaim>; // async, privé
Execution::rpc_checked_claim(&self, pending: Operation, slot: &EnforcedSlot, reservation: ClaimReservation) -> anyhow::Result<(Operation, bool)>; // async, privé
```

`BeginUnusedRetirement` ne reçoit pas un scope libre : le writer dérive/recoupe l'identité depuis son `JournalScopeV1`, l'Arm ACKée et la registration engagée par le témoin. Il vérifie le head confirmé sans claim de cette Arm, réserve Final+événement puis publie Pending et intention outbox. `PendingUnusedRetirement` retient la preuve consommée et la réservation vivantes ; sérialiser Pending ne le reconstitue pas. `CompletionFailure` conserve la distinction NotQueued/Unknown : aucun token retourné comme « refus certain » si déjà en queue, aucun nouvel identifiant au retry.

#### Entrée productive Full3, du CLI au vrai executor

Le gel courant passe encore par `persistent_child("enforcer", !recovery_only)` et `EnforcerClient::connect_or_spawn` dans `service/server.rs:289–314`, puis `Service::executor` fournit ce client à `BatchExecutor`, dont le champ est historique (`executor.rs:51–54`). Ajouter une classe Full3 inutilisée ne ferme pas cette couture.

- [ ] Étendre `broker serve` avec deux options mutuellement exclusives `--enforcer-full3-new <role-name>` et `--enforcer-full3-existing <role-name>`, incompatibles avec `--recovery-only`. `role-name` est un seul nom enfant privé validé, jamais un chemin libre ; le parent vient exclusivement de la racine du registre retenu. Le CLI produit `ServiceEnforcerSelectionV1` puis appelle `serve_selected`. Sans ces options, les parcours historiques conservent leurs bytes/comportements ; aucune détection de fichier n'active Full3.
- [ ] Pour `Full3Fresh`, appeler directement `EnforcerFullClientV3::spawn_fresh(&root, role_name, ...)` : **ne pas** précréer ce rôle par `persistent_child(..., true)`. Exiger les journaux de profils de l'installation sans Arm antérieure, avec couverture confirmée ; refuser EEXIST, historique armé/inconnu ou initialisation non exclusive. Aucun takeover de `enforcer`, des vieux gardiens ou de leurs Arms. Pour `Full3Existing`, ouvrir le rôle existant sans création puis `connect_existing` : rôle scellé/installation attendue, identité ledger et Complete vérifiés avant Full3, nouvelle connexion explicite, aucun spawn/réamorçage si le pair manque. Après Register de l'Arm fraîche ACKée dans cette invocation, `slot.full3_registration_binding` rend seulement les données de la réponse authentifiée ; `bind_full3_registration` recoupe Arm/reçu/registration, publie le binding V5 **avant claim** et reconnaît son replay exact. Binding inconnu/non confirmé interdit claim. Toute mutation suivante conserve ces bindings au head courant avec Cancel/outbox/retraits ; aucun Draft V3 ne peut les écraser. Une Arm chargée/reprise sans binding exact de ce rôle/genesis/registration n'est jamais réenregistrée sous Full3 et conserve seulement sa voie STOP d'origine. Ce binding est une donnée d'affectation, pas une nouvelle preuve originale ; les anciens formats ne sont pas enrichis rétroactivement.
- [ ] `Service.enforcer`, `Service::full_enforcer`, `Service::executor`, `BatchExecutor::new` et son champ utilisent **le même** `ExecutionEnforcerV1`. Dispatcher explicitement readiness, Register, Restore, Prepare et Cancel vers sa variante ; `EnforcedSlot`/`SpawnGuard` conservent `SlotAuthorityV1`. Le client Full3 vérifié atteint donc réellement `arm_and_claim`, puis `retire_unused_before_claim`, sans recréer un `EnforcerClient` ni utiliser une méthode legacy pour obtenir un droit Full.
- [ ] La sélection Stops reste distincte et sans executor d'admission. Pour viser un rôle scellé existant en récupération, ajouter `--recovery-only --enforcer-role <role-name>` et le port `EnforcerStopClient::connect_or_spawn_existing_sealed` utilisant les ancres/lease spécialisées de tâches2–3, sans initialisation. Le défaut sans nom conserve le legacy. Ni récupération ni erreur ledger ne relance la sélection Full3 ; les STOP structurellement vérifiés continuent par leur client restreint, indépendamment de `require_starts()` et du ledger Complete. Ajouter les guides/configurations EN/FR de ces entrées dans le même lot d'activation.

#### Producteur d'abandon explicite et course avant claim

Le besoin opérateur est d'empêcher le prochain claim d'une Arm précise encore en cours de registration, puis de retirer son acteur original si sa preuve demeure admissible. Il ne s'agit pas d'un arrêt du job. Le producteur est la commande **proposée** `broker abandon-before-claim` de `broker_cli.rs`, transmise par `service::request` vers `BrokerCommandV1::AbandonBeforeClaim { request: AbandonBeforeClaimV1 }` et le handler authentifié `Service::abandon_before_claim`. DTO fermé version1 : `operation_id`, `arm_id`, `command_id`, `expected_epoch` sont des UUID non nuls ; `generation: u64` est bornée comme la génération existante. L'identité authentifiée choisit le profil/scope, jamais le DTO. Les variantes historiques et leurs encodages restent inchangés ; un ancien serveur refuse la nouvelle commande, sans repli vers Cancel.

Le vrai `broker recovery-status` expose déjà `known_arms[].arm_id` (`protocol.rs:179` et suite). Étendre ses cibles par une projection typée `preclaim` : epoch/génération et état `RegistrationInFlight|OriginalUnclaimed|ClaimReserved|StopReserved|Retiring|Unknown`, avec provenance processus-local et fraîcheur ; `Unknown` si owner absent/reprise/lecture incomplète. L'opérateur copie opération/Arm/epoch/génération dans `broker abandon-before-claim --operation-id … --arm-id … --expected-epoch … --generation … --command-id …` avec les options de connexion habituelles. Cette projection facilite le ciblage, sans constituer une preuve ni garantir que la course sera gagnée. La commande refuse les Arms absentes/hors scope, legacy, déjà claim-réservées, STOP-réservées, restaurées ou inconnues.

1. `arm_and_claim` publie auprès du `Job` l'identité de l'Arm ACKée et l'état `RegistrationInFlight` **avant** son vrai Register ; il n'ajoute aucune attente d'opérateur. `AuthenticatedBatchContext` porte le même `PreclaimControlV1`. Sous son seul arbitre mémoire, le handler peut sélectionner `AbandonRequested(command_id)` ; l'appelant peut sélectionner `ClaimReserved` seulement si aucun signal n'a gagné. Les deux transitions sont irréversibles. L'arbitre ne réalise aucun IO/HTTP/join et n'attend ni gate d'exécution ni mutex ledger.
2. Le handler authentifie les mêmes droits de mutation que Submit/Resume, recoupe l'Arm au head confirmé et vérifie epoch/génération. Une réservation d'abandon ferme immédiatement la voie locale au claim de **cette Arm**, sans invalider `CancelGeneration`. Il publie l'événement borné `broker.abandon-before-claim.requested` dans l'outbox existante, avec `event_id=command_id`, contenu/hash fixes et vrai ACK d'audit persistant ; ensuite seulement il transmet `AbandonBeforeClaimRequest` au consommateur. Le signal réservé qui attend cette publication est une inhibition locale, pas une preuve ; panne/inconnu interdit claim et Closing et conserve l'acteur/STOP. Le coordinateur appartient au Job, indépendamment du waiter IPC. Aucun témoin n'est reconstitué à partir de cet événement au restart.
3. Au retour Register, l'appelant recoupe la même Arm et le même acteur original. Si le signal a gagné, il consomme la requête (`retirement_id=command_id`), appelle `revoke_unused`, puis le parcours de retrait ; un simple échec de generation/readiness/horloge/Register, un Drop ou un retour ObserveOnly **ne crée jamais** ce signal ou ce témoin. Sans signal sélectionné, les contrôles actuels restent obligatoires puis `reserve_claim` gagne la même course mémoire. Sa réservation est consommée avant tout pending WorkerClaim/HelperClaim par `rpc_checked_claim`, qui réutilise contrôles/CAS de `rpc_checked_at_head`. Après cette réservation, même ACK perdu ou RPC non émis, aucun D5-U.
4. Le parcours d'abandon utilise le même slot/Arm, publie Pending, livre son intention par le vrai flux de `results.rs`, puis `acknowledge_events` persiste l'ACK exact event_id/input_hash. `authorize_unused_retirement` le recoupe avant le token single-use ; ACK absent/inconnu, témoin perdu ou STOP intervenu interdit Closing et conserve STOP/fence. Perdre le token ne permet pas de le recréer depuis Pending. Une échéance dépassée n'est jamais renouvelée pour terminer le retrait.
5. Le service rend `AbandonRequested` seulement après l'ACK du **signal**, en exposant sa portée ; cela ne signifie ni Closing ni retrait réussi. `recovery-status` distingue ensuite `Pending|Final|Ineligible|Blocked|Unknown` avec les références réellement confirmées. `Final` seul donne `AbandonedBeforeClaim`. Même command_id/contenu retrouve le même état ; divergence est un conflit et un ancien signal ne vise jamais une nouvelle Arm/epoch/génération. Si claim ou STOP gagne, le refus reste explicite et Cancel reste disponible ; aucune relance ni fenêtre de grâce n'est ajoutée.

**Ordre Cancel/STOP conservé :** aujourd'hui `Job::begin_cancel` invalide immédiatement la génération (`jobs.rs:144–170`), puis `Service::cancel` persiste Cancel avant `deliver_cancel` (`server.rs:1187–1232`), sans attendre le gate d'exécution. Garder cet ordre. Cancel ne devient pas un producteur de `AbandonBeforeClaimRequest` et n'attend jamais Pending, son ACK d'audit, Closing ou join. Il préempte localement claim et signal d'abandon, écrit la même Cancel puis livre ses cibles exactes indépendamment. Closing arbitre ensuite avec **toute** réservation STOP mutative, y compris deadline/reopen : si STOP gagne ou reste inconnu, retrait inéligible ; si Closing a déjà gagné, le STOP ciblé reçoit une réponse authentifiée de fermeture sans nouvel effet, le Cancel et ses autres cibles restent conservés. Aucune mutation STOP n'est retardée pour fabriquer une preuve de slot inutilisé.

**Résultat jusqu'au vrai appelant :** ajouter `PrearmedStartDirective::AbandonedBeforeClaim(AbandonedBeforeClaim)` et faire retourner `ManagedPrearmedOutcome` aux deux ports préarmés de `runtime/managed.rs`. Cette branche s'arrête avant la construction/émission de la commande start et avant l'exigence de terminal ; elle retient le même `OwnedContainerSnapshot` et la référence F. `Execution::worker`, `helper` puis `run` propagent un `BatchOutcome::AbandonedBeforeClaim` distinct d'ObserveOnly/Cancelled/Terminal ; ils n'appellent pas les fonctions de résultat/observation/cleanup d'une exécution. ObserveOnly conserve le refus de réémission actuel (`managed.rs:1332–1344`). L'instance Created et les quotas centraux restent connus dans leur état réel, avec reprise opérateur explicite ; aucun terminal, capture vide, usage0 ou libération centrale n'est déduit de ce résultat.

Après Q vérifié, `finalize_retirement` utilise **le même** acteur `Backend::Full` ou `Backend::Stops`, sans chargement d'`AcceptedJobV1`/blob ni `require_starts()`. Il ne peut que faire Pending audit-ACKé→Final avec identité/reçu exacts au head courant, préserver Cancel/V4 et publier l'événement final aux event_id fixés dès Pending. Le payload/hash final est figé à F ; son ACK ultérieur ne modifie pas le reçu Q. Retirer LocalSlot sous `recovery_gate` après F seulement, anciens handles toujours révoqués. Aucun nouveau Pending/claim/start/export/Cleanup par ce port.

- [ ] RED puis GREEN `d5_u_service_cli_selects_full3_without_legacy_adoption` : vraie entrée `broker serve`/CLI, rôle neuf puis connexion explicite au même rôle scellé, enum transmis au vrai `BatchExecutor` et Register Full3 atteint ; EEXIST/legacy/Partial/Unverified/connexion absente refusés sans réamorçage/repli, récupération Stops incapable de créer l'executor. Binding publié avant claim, rôle/genesis divergent ou binding absent au replay refusés ; mutations courantes/CAS conservent bindings/Cancel/outbox/retraits. Préparation ledger et acteurs restent parallélisables après interfaces gelées ; aucun ancien gardien touché.
- [ ] RED puis GREEN `d5_u_service_cli_abandons_original_before_claim` : vraie commande `recovery-status` puis `abandon-before-claim` sur service/Journal/Writer, vrai propriétaire de slot et IPC ; signal reçu pendant Register, P audit-ACKé→C→T→join→Q→F, aucun WorkerClaim/HelperClaim ni start, outcome borné propagé jusqu'au worker/helper/service. Synchroniser uniquement le harnais autour des vrais producteurs, sans mode spécial de produit ni attente opérateur. Sans signal, le parcours normal choisit immédiatement claim.
- [ ] RED puis GREEN `d5_u_preclaim_signal_races_claim_and_cancel` : claim réservé avant signal ⇒ refus explicite ; signal avant Register puis acteur restauré/Prepare ou STOP gagné/epoch ou génération inconnue ⇒ aucune preuve ; Cancel invalide avant writer bloqué et livre STOP sans attendre le retrait, autres cibles progressent. Audit du signal ou Pending indisponible/inconnu ⇒ aucun Closing ; Drop/erreur/ObserveOnly ne déclenchent aucun retrait. Vérifier les réponses Requested/Final/Ineligible/Blocked/Unknown, replay exact et divergence.
- [ ] RED puis GREEN : claim absent/inconnu recoupé sous writer, faux témoin impossible ; V5 encapsule V4 et interdit downgrade, entrée disparue/dupliquée/réattribuée ou Final sans même Pending.
- [ ] RED puis GREEN : limites journal/outbox proches des caps avec plusieurs retraits et Cancel, refus avant Pending si réserve inconnue ; Final promis toujours publiable ; résultat writer inconnu rejoué sous identité fixe.
- [ ] RED puis GREEN : Q vérifié→F une fois, événement final/audit ACK perdu rejoué sous même event_id ; anciens guards/handles refusés et aucune modification des quota/usage/Cleanup.

### Tâche5 — reprise avant Restore et acceptation composée, périmètre3 avec contrats1/2 gelés

**Fichiers :** `journal/recovery.rs`, `writer.rs`, `service/{server.rs,d5_ledger_tests.rs,recovery_epoch_tests.rs,recovery_multi_arm_tests.rs}`, nouveaux tests de reprise adjacent au journal. **Consomme :** projectionV5, QueryQ authentifié et port Final exact. **Produit :** chemin metadata-only avant **une seule** factory Full/Stops, plus Final tardif sous Stops ; cette tâche est nécessaire pour clôturer D5-U.

```rust
RecoveredJobV1::retirement_projection(&self) -> &[RetirementProjectionV1];
StopRecoveryReportV1::finalize_retirement(&self, receipt: VerifiedRetirementReceipt) -> JournalResult<CheckpointReceiptV1>; // sur le publisher/FD déjà retenu, avant factory
```

Ces deux interfaces n'exposent ni job métier ni nouveaux droits. Le publisher metadata est créé une seule fois lors de la reprise sous la lease retenue, avec son unique `Arc<Inner>`/mutex conservé par le report ; `admit_claimed_report` transfère ce **même** owner au lieu de reconstruire son `Writer`/mutex actuel après une finalisation. La méthode du report refuse après ce transfert. `restore_profile` traite d'abord Pending/Q sous ledger Complete, publie le même F puis **recharge** head/projection avant de restaurer seulement RequiredStop. Le transfert `ProfileWriterCapabilities::from_recovery_report` reste unique ; son `actor_claimed` et sa lease ne sont ni reconstruits ni doublés. Après transfert, le seul port tardif est `StopWriterClient::finalize_retirement` de tâche4. CAS conflict ⇒ relecture metadata et fusion du même résultat, jamais nouvel effet.

Partial/Unverified ferment Full/finalisations, exposent couverture/phase/erreur et conservent RequiredStop prouvés ; Closing/Q vérifié interdit un Restore de contournement de la même Arm. E1/E2/E3 sans exclusion d'un effet intervenu reste `InterveningStopNotExcluded`, y compris lorsque le ledger redevient Complete. Aucun constructeur RecoveredPriorEpoch fondé sur PID/flock/annonce/epoch ; cette preuve distincte de Joined exige sa future qualification dédiée. Closing mémoire puis FS inconnu reste fermé/inconnu, sans témoin reconstruit au restart.

- [ ] RED puis GREEN B1 : produire réellement Q, retirer seulement la tombstone B de la fixture ; défaut détecté avant Full, A/C restaurés, aucune nouvelle genesis. Recul du ledger seul détecté par le floor.
- [ ] RED puis GREEN B2 : broker enfant possédé mort après Q avant F, blob absent, Cancel concurrent, ACK tardif ; même F/reçu/event_id avant Restore, port tardif sous Stops et aucune seconde factory. Enfants/ancres créés par le test exclusivement ; aucun ancien gardien touché.
- [ ] RED puis GREEN E1/E2/E3 : STOP intermédiaire non exclu ⇒ Final bloqué ; autre inode/rôle/genesis ou owner encore vivant jamais converti en preuve de quiescence.

### Exécution Luna, gels et limites de livraison

Les nouvelles fonctions `d5_u_` sont **à créer**, les trois groupes canaris suivants existent ; toutes les commandes ci-dessous sont **NOT_RUN** pour ce plan. Après gel des interfaces, périmètres1/2/3 avancent sur fichiers distincts ;2/3 peuvent préparer leurs vrais producteurs/tests en parallèle de1 sans simuler un succès ledger. Un seul propriétaire intègre les déclarations/imports partagés. Aucun backend factice permanent, suppression de barrière ou test « type absent » ne qualifie une étape.

Luna conserve avant chaque RED/GREEN le gel exact, commande, toolchain, RC, nombre de tests **effectivement sélectionnés**, sources et binaires. Même cible Cargo propre au checkout ; aucune compilation lourde concurrente avec SSO. Pour éviter un filtre `--exact` sélectionnant zéro test, les nouvelles fonctions portent le préfixe `d5_u_` et les filtres sont des sous-chaînes :

```sh
CARGO_INCREMENTAL=0 cargo test --locked --offline --lib d5_u_ -- --test-threads=1
CARGO_INCREMENTAL=0 cargo test --locked --offline --lib broker::enforcer::recovery_v3 -- --test-threads=1
CARGO_INCREMENTAL=0 cargo test --locked --offline --lib broker::enforcer::server::lifetime_tests -- --test-threads=1
CARGO_INCREMENTAL=0 cargo test --locked --offline --lib broker::service::server::recovery_multi_arm_tests -- --test-threads=1
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo build --release --locked
```

Ordre : revue Root des contrats proposés → gels des trois interfaces → tests contrat puis RED/GREEN comportementaux sur vrais producteurs → intégration des trois périmètres → groupe composé `d5_u_` et canaris nécessaires une fois → revue indépendante sûreté/audit/lancement et corrections → checks stricts/release sur cible isolée sans binaire retenu → preuves et écarts actualisés. B1 exige Q réellement produit avant retrait de sa tombstone ; aucun stub/parseur seul ne clôture une tâche ou le vertical. Guides/configuration EN/FR et procédure de rôle neuf sont mis à jour dans le lot d'activation, sans livraison annoncée par ce cadrage.

**Seule clôture D5-U :** original inutilisé productif→Pending réellement audit-ACKé→Closing→tombstone durable→vrai join→Q authentifié/rejouable→Final et retrait local, capacité décrémentée une fois, refus des anciens handles/Register/Prepare/Restore, B1/B2 et reprise avant Restore démontrés. Les tâches préparatoires demeurent des composants en cours tant que ce groupe n'est pas satisfait. `DockerNormalCompletion`, starts ambigus/fencing, quiescence récupérée inter-epochs, migration legacy, powerloss/rétention, autres plateformes et capacité100 restent des écarts distincts obligatoires ; D5-U logiciel ne ferme pas G09 et ne donne aucune autorité d'arrêt sur les vieux gardiens.

Le complément humain local est intégré et vérifié dans ses périmètres logiciels : API/CLI/SQLite, migration5 et front nominatif. Les échecs de préparation et statiques sont conservés avec leurs corrections dans [VALIDATION](VALIDATION.md#1er-novembre--accès-humain-local-apicli-et-dashboard). La release native composée65f828a passe ; restent nouvelle image et parcours humain contre le contrôle conteneurisé, puis OIDC/PKCE et lancement délégué. Le mainteneur a approuvé le gateway propriétaire OIDC, le proxy auth limité et le raccord Keycloak opt-in ; les contrats transport et audit privé sont préparés dans des worktrees séparés, avant leurs RED ciblés Luna. En parallèle, le diagnostic Jev séparé passe ses24 fonctions ciblées et statiques sans nouvel appel modèle ; l'unique essai modèle réel demeure FAIL. D5 doit livrer un retrait complet avec vrai join/capacité, reprise avant Restore et protocoles durables explicites ; le cadrage n'est pas une implémentation. Aucun de ces lots ne clôture G09 ou la qualification cloud.

> Exécution autonome demandée par le mainteneur, avec agents spécialisés, périmètres exclusifs et revue indépendante. Les exigences sont dans AGENTS.md et ARCHITECTURE_CONTRACT.md ; aucune validation intermédiaire de plan n'est requise par la mission.

Objectif : satisfaire les exigences complètes local-first/cloud et produire des preuves reproductibles, sans assimiler un lot à une qualification du produit. Rust/SQLite et frontend TypeScript existants sont conservés. Les décisions structurelles restent soumises au mainteneur.

### Répartition active — identité, supervision et unification

- Identité : domaine humain/ACL, projections indexées, migration4 sauvegardée et compatibilité exacte supervision3/4 implémentés et vérifiés ;26 fonctions uniques pertinentes sur leurs gels, fmt/Clippy strict et revues traitées. Le premier binding historique reste opérateur ; aucun vocabulaire caché sous3. [Preuves](VALIDATION.md#1er-novembre--domaine-identité-et-migration4). Raccordement authentification/provisioning/HTTP/CLI et vues humaines réellement filtrées, OIDC public PKCE Web/CLI et lancement humain délégué restent obligatoires et ouverts ; pas de JWT→viewer tenant entier.
- Dashboard : liens exacts sandbox/agent/worker depuis les API existantes implémentés ; pagination/fraîcheur/provenance visibles et reprises vérifiées, trois défauts de revue corrigés après leurs RED. Build, unités et navigateur ciblés PASS sur leurs gels documentés. Aucun écran SSO factice ni compteur global fabriqué ; déploiement du bundle, vues humaines/admin et fédération restent ouverts.
- Registre : credential de broker distinct et lié au tenant côté serveur, ownership et heartbeat audités, fraîcheur observée distincte de liveness déclarée. Contrat coordonné avec la migration identité avant implémentation des coutures partagées ; pas d'assimilation d'une autorité moteur à un broker.
- Les tests produit ciblés sont exécutés par Luna après gel ; revues et intégration précèdent tout statut livré. Les trois worktrees dédiés sont suivis jusqu'à leur retrait contrôlé après publication, sans suppression de données privées ou de ressources vivantes.

Le composite publié cd3d0b5 passe ensuite une seule release native arm64, sans nouveau canari ni appel modèle. La verticale humaine locale productive continue en parallèle dans un worktree backend réutilisé et un nouveau frontend : API/Store, CLI et UI ont des périmètres exclusifs. Ce nouveau raccordement et sa revue/validation restent ouverts ; OIDC/PKCE et lancement humain ne sont pas livrés par le socle. L'image Linux et l'import TLS réels sont qualifiés séparément, sans compilation lourde concurrente.

### Lots indépendants et fermeture des worktrees — 1er novembre

- Dashboard : correctif de refus401/403 intégré et poussé sous `d1164bf`, référence distante vérifiée. Onze tests API/build et navigateur ciblé PASS sur le gel, revue traitée ; onze tests API et build repassent après intégration. Preuves conservées hors checkout, inventaire final traité et checkout secondaire supprimé par Git sans force ; branche conservée.
- Jev : sélection des trois backends et masquage immuable API/Store implémentés ; cinq régressions RED deviennent GREEN.62 fonctions ciblées, fmt, Clippy strict et compilation release PASS, revue traitée, guides EN/FR/configurations mis à jour. Commit58309ed poussé/vérifié, donneur synchronisé par fast-forward puis fermé proprement ; branche et preuves conservées. Modèle local consultatif seulement ; liaison conteneurisée HTTPS/AC privée en worktree distinct et qualification sémantique restent ouvertes.
- D5/TLS : STOP3 candidat1 revu, quatre écarts P2 deviennent GREEN,32 fonctions ciblées et compatibilité2/R8/R9 PASS. TLS T2 ferme ses deux RED avec sept vérifications et revue ; fmt/Clippy strict/release du checkpoint composé passent. D5a334 et TLS766 poussés/vérifiés ; donneurs fermés proprement après inventaire, branches/proofs conservés. Image LinuxARM64/Node24 construite, qualification conteneurisée en préparation privée ; ledger engagé, Closing/join/retraite/fencing restent absents et G09 ouvert. Anciens gels/échecs/limites de capture restent conservés.
- Le propriétaire clôt chaque worktree selon AGENTS.md. Une branche ou un checkout portant encore un binaire vivant, des ancres, des changements non intégrés ou des données privées reste conservé avec sa dépendance explicite ; aucune suppression forcée.
- Le donneur `docker-launch-profile` est fermé après égalité de ses19 contenus avec le lot intégré, sauvegarde de branche `427c9ba` poussée/vérifiée et retrait Git propre. `keycloak-sso` est fermé après conservation privée vérifiée de ses cinq variantes/essais, déplacement du target entier, réconciliation/fast-forward et retrait propre. `broker-supervision` reste retenu par sept gardiens et leurs ancres. Aucun test ni qualification supplémentaire déduit de ces opérations de conservation.

Lecture du plan : le [cycle broker permanent](#cycle-actif--broker-permanent-et-premier-batch-réel) reste un chantier applicatif distinct des [exigences d'identité et de configuration](#fonctionnalités-demandées--identité-sso-et-découverte-31-octobre). Les autres lots conservent leurs dates, gels et propriétaires historiques. Leurs chemins temporaires, noms d'agents et commandes exécutées ne sont pas des exigences durables ni des preuves d'un autre checkout. Le contrat produit canonique est [sso.md](../sso.md), les vues sont définies dans [ui.md](../ui.md) et l'avancement constaté dans [CURRENT_STATE.md](CURRENT_STATE.md).

Préparation identité du1er novembre, lecture seule : le premier parcours doit joindre principal humain, memberships/ACL durables et lectures/export réellement filtrés ; brancher un JWT sur le viewer tenant actuel exposerait trop de données. Les propriétaires Web/PKCE et routes IdP ne sont pas encore décidés. Une migration naïve au schéma4 ferait refuser la supervision3 actuelle : contrat/version/migration compatibles à figer avant code, jamais un test `>=3`. Local nominatif, OIDC Web/CLI et lancement humain délégué restent tous obligatoires. Le [contrat OIDC](SSO_OIDC.md) distingue désormais le profil IdP distribué de l'intégration applicative toujours absente ; aucune nouvelle capacité n'est annoncée par ce cadrage.

## Point de reprise — raccord permanent du 31 octobre, en cours

- [x] S0, rétention de la même lease native après disparition du listener : RED causal puis correctif `server.rs`/owner adjacent, harnais inchangé ; sept fonctions ciblées, fmt/Clippy strict/release arm64 PASS et revue traitée. Même FD et handles possédés, progression arrêtée/jointe sans attendre les STOP, inconnus conservés sans retrait/éviction. [Preuves et limites](VALIDATION.md#1er-novembre--s0-r2-green-ciblé). Ni format/auth/protocole/ledger ni moteur modifié ou qualifié ; le retrait complet ci-dessous reste obligatoire.
- [x] Implémentation logicielle d'import TLS selon le [contrat dédié](TLS_MATERIAL_PROVISIONING.md) : trois sources privées vérifiées, handoff stdin borné, deux volumes neufs, ACK/manifeste durables puis activation explicite. Onze fonctions actives ciblées, fmt/Clippy strict et revues PASS ; configurations et guides EN/FR distribués sans fallback filesystem ni clé d'AC transférée. [Preuves](VALIDATION.md#1er-novembre--import-tls-vers-volumes-validation-logicielle-r3).
- [ ] Qualification d'import TLS : release/image Linux, import dans les volumes Docker Desktop et contrôles effectifs FS/ACL, santé HTTPS puis une décision réelle bornée et panne/reprise ; Podman séparément. Les PASS logiciels ne ferment pas ce parcours.
- [x] Profil Docker minimal lié au parent accepté : contrat wire2/bindings2, blob privé exact et reprise STOP indépendante, correction du dépassement de blob ; gels, douze régressions, checks et intégration ciblée [documentés séparément](evidence/2026-11-01/docker-launch-profile/README.md). Ce lot ne retire aucun gardien ni ne ferme D5/G09.
- [x] Nom de source Claude par abonnement corrigé avec alias historique, sans réécriture ; dix tests de configuration, oracle EOF renforcé, formatage et Clippy strict passent dans une cible privée PRIMARY. Les échecs de harnais et du cache partagé restent conservés.
- [ ] Retrait du gardien : implémenter l'inventaire durable et le rapprochement pending avant choix Full/STOP, les preuves vivantes, la quiescence et la reprise idempotente selon le [contrat consolidé et relu](BROKER_SUPERVISION.md). Prochaine verticale : chemin original inutilisé → Closing → tombstone → vrai join → final durable et rejouable, avec son appelant productif avant claim. Formats V5 et deltas/floors, port metadata-only unique avant factory et tardif sous Stops, budgets et scénarios B1/B2 restent obligatoires. Pas d'éviction automatique ni retrait fondé sur un simple terminal ; un ancien Q ne couvre pas un STOP intermédiaire potentiellement émis.
- [x] Finaliser le lot logiciel StopRecovery : authentification existante, rôle StopsOnly distinct de Full, RequiredStop propriétaire avec identité/deadline originales et reprise admission fermée sont implémentés. Les dix fonctions ciblées R7 passent après les RED conservés ; R8 corrige les diagnostics Clippy, avec quatre canaris, deux intégrations CLI, fmt/Clippy strict et release RC0. Revues traitées et guides EN/FR synchronisés ; limites P3 conservées, aucune preuve moteur ou de service complet avec contrôle HTTP n'est déduite des fixtures IPC. [Résultats et limites](VALIDATION.md#r8--indirection-privée-et-conditions-équivalentes-validation-logicielle-finale).
- [ ] Préserver les échanges2 exacts et raccorder V3 par négociation authentifiée avant tout Grant selon le [contrat de supervision](BROKER_SUPERVISION.md). Premier RED : vraie connexion distincte, Negotiate3 lié à la préface2, absence de Challenge3 attendue sur R8 ; les erreurs de setup et de cleanup ne comptent pas comme ce RED. Les schémas Auth3/Grant3/Status3 complets, le parseur fermé, les preuves liées au ledger, les clients Stop3/Full3 distincts, les tests de refus/replay et la compatibilité du vrai client2 restent à livrer. Aucun simple nouveau message ne clôture cette étape.
- [ ] Raccorder ce canal au ledger : classification metadata-only par Arm, rapprochement Pending/final exact avant Restore de cette même cible, puis restauration des seules RequiredStop restantes. Les STOP indépendants n'attendent pas le défaut d'un autre retrait. Cache Parsed/Tainted du même FD, couverture partielle explicite, reprises P/Q→F et refus de l'ancien Q après un intervalle STOP ambigu restent à implémenter et qualifier. [Décision de reprise](DECISIONS.md#1er-novembre--reprise-stop-distincte-de-ladmission).
- [x] Préserver l'authentification d'une ancienne racine gardien : RED sur vrai flock, correctif limité au client, deux fonctions auth et bootstrap public du vrai child vide PASS, formatage/Clippy strict, revue et guides EN/FR synchronisés. Au gel de ce correctif d'authentification seul, le RED du protocole2 était encore en travail ; ses résultats ultérieurs sont séparés ci-dessus. [Preuves et limites de l'authentification](VALIDATION.md#1er-novembre--préserver-lauthentification-dun-rôle-gardien-existant).
- [ ] Réactivation du contrôle après reopen : le Store3 fraîchement activé ne survit pas au restart ; définir et vérifier une reprise sûre sans reconstruire d'autorité depuis une base restaurée seule. Ne pas contourner par suppression de base ou modification d'un flag.
- [x] Diagnostic de configuration indépendant : `broker control-info --gateway ORIGIN --token-file FILE` lit l'API existante `/api/v1/control/protocol`, sans registre ni broker démarré. Réutilisation d'AuditClient/PrivateFile/Redactor, aucune activation ou autorité nouvelle, sortie typée avec refus de fuite. RED réel puis5/5 fonctions finales, formatage/Clippy strict, revue et guides EN/FR synchronisés ; [gels et limites](VALIDATION.md#1er-novembre--diagnostic-authentifié-du-contrôle). Le défaut d'oracle schéma2 du premier4/5 est conservé et corrigé sans changer le produit. Ce diagnostic ne remplace ni D5 ni la réactivation Store3.
- [x] Générateur local PKI en une commande Rust : chaîne neuve, protections de fichiers/programme, refus et TLS réellement testés ; [preuves et limites](evidence/2026-11-01/demo-pki/README.md). Ce lot ne qualifie pas G11 ni le raccord HTTPS des services.
- [ ] Poursuivre indépendamment le profil Docker/reprise STOP et le raccord identité/SSO ; ni la PKI de test ni le profil Keycloak de découverte ne livrent ces parcours complets.

Base publiée `c4c4d2a7560d31768da08d8ad9335f4f08667344`, travail isolé dans `.worktrees/broker-service`. Service/CLI explicite, IPC, writer, outbox V4, capacités de source/export et gardien natif sont raccordés. Les premiers tests ciblés passent ; la nouvelle verticale n'est pas encore qualifiée sur moteur et aucun cutover du CLI n'est autorisé par ces résultats.

- [x] Compilation all-targets d'un gel intermédiaire sans avertissement ; tests IPC/CLI/writer, ownership de la queue et états/identités du service exécutés. Les échecs de compilation de fixtures sont conservés.
- [x] Vrai processus gardien vide : refus pendant SIGSTOP, reprise SIGCONT sur la même epoch, arrêt propre et absence de child résiduel. Le harnais crée sa seule capability privée neuve ; aucune clé réelle ni moteur.
- [x] Corriger en source et faire relire la reprise STOP indépendante des blobs/du chargement global, la causalité Cancel/Export, la santé avant admission et l'exclusion des identités pendant publication de l'input Export. Les tests de concurrence du verrou et les quatre capacités workspace passent ; revue source seule distincte des preuves runtime.
- [x] Tests V4 et de récupération de chaîne, dont sibling corrompu, blob perdu, ancien managed V1 et annulations multiples ; canaris, Clippy et build exécutés. [40 fonctions uniques, gels et limites](evidence/2026-10-31/broker-service/README.md). Le périmètre de validation demeure ciblé et non une qualification de la verticale runtime.
- [x] Qualifier le bootstrap public du gardien vide : vraie API `connect_or_spawn`, racine neuve sans capability préinstallée, état prêt puis arrêt/refus endpoint/libération de lease. [PASS ciblé du lot export](evidence/2026-10-31/export-publication/README.md), distinct du premier child directement lancé par le harnais.
- [ ] Qualifier le service complet avec un vrai contrôle3 local : authentification/rôle/tenant avant FS, perte du client, erreurs d'audit, replays et reprise des faits. Le bootstrap vide ne qualifie aucun worker ni échéance runtime.
- [x] Corriger et vérifier le retry d'une annulation durable sur writer vivant : frontière par commande exacte, anciennes Arms avec ACK tardif incluses, exports ultérieurs exclus, reçu historique et génération/gate conservés ; projection sans blob et port STOP/natif NotReady. [14 fonctions ciblées et revues traitées](evidence/2026-10-31/cancel-retry/README.md), sans preuve d'arrêt moteur.
- [x] Traiter en logiciel le premier Cancel sans blob, le writer strict indisponible et les résultats append inconnus par une capacité STOP durable indépendante du chargement métier. Admission unique et lease/floors/rejets conservés, diagnostic explicite et tests d'authentification avec vrai gateway/Store ; [33 fonctions ciblées, revues et limites](evidence/2026-10-31/stop-only-actor/README.md).
- [ ] Vérifier leur convergence réelle sur moteur, les chemins authentifiés de connexion, l'arrêt/reprise après perte du client/broker et la clôture du cycle du gardien ; ni les fixtures ni le canari Unix de refus vide ne ferment ces exigences. Docker est joignable avec images locales ; une nouvelle session isolée doit servir de preuve, sans réutiliser les ressources historiques.
- [ ] Vérifier les slots non vides, les générations restaurées, la panne/reprise du child et la course de publication du moniteur par des régressions contrôlées ; puis la verticale batch/helpers sur vrai Docker. Préserver le FAIL G09 historique tant que les vrais workers n'ont pas démontré la deadline après perte du CLI/broker.
- [ ] Gardien TTY, fencing/disarm D5, rétention/caps physiques et récupération métier restent obligatoires avant cutover. Le rappel Daybreak Blue attend une version stable et les scénarios réels d'arrêt/reprise de workers.
- [x] Conditionner le succès d'export aux synchronisations fichiers/modes, répertoires en postordre, staging et parent après rename ; conserver la destination lors d'une erreur post-publication ; valider les droits des ancêtres par capacités FD. RED/GREEN et revues traitées, [17 fonctions ciblées](evidence/2026-10-31/export-publication/README.md).
- [ ] Qualifier les pannes de stockage/power loss, vrais autres UID et matrice de plateformes ; harmoniser la limite64 de scan/export avec la collecte/import et borner le travail d'énumération total incluant les entrées exclues. Les seuls syscalls réussis ne clôturent pas ces garanties.

### P1 ajouté le 31 octobre — SSO et documentation d'installation

- [x] Enregistrer le [cahier produit canonique](../sso.md) et sa [déclinaison technique OIDC/PKCE](SSO_OIDC.md), sans annoncer une implémentation. Le suivi détaillé unique est la section [fonctionnalités demandées](#fonctionnalités-demandées--identité-sso-et-découverte-31-octobre) ; elle conserve les critères d'identité, traçabilité, découverte et qualification.

### Recette active — première verticale Docker du broker permanent

**Objectif :** vérifier l'arrêt d'un vrai worker après SIGKILL du broker propriétaire, puis la reprise des mêmes faits. Base logicielle `e7cc0bb`, sans appel modèle. Le lot précédent est un progrès vérifié (code, revue, tests et push) ; cette recette ne réinterprète pas ses preuves comme des effets runtime.

**Architecture :** vrais processus locaux gateway schéma3 neuf, broker et gardien existants ; seul le worker et ses helpers utilisent Docker. Aucun service OS installé, aucune infrastructure distante. Ce contrôle natif de test ne qualifie pas le déploiement conteneurisé du produit. Rust stable pour le harnais, frontend inchangé, aucune dépendance ajoutée.

**Fichiers et propriétaires :** agent runtime pour `tests/broker_runtime.rs` et petits helpers de test nécessaires ; root pour contrats/documentation/intégration ; Luna pour toutes les exécutions ; revue indépendante avant effet puis après résultat. Les corrections applicatives éventuellement révélées recevront un périmètre exclusif avant édition.

- [x] Revalider en lecture seule endpoint, version, image exacte et inventaire préexistant. Capture Luna `/private/tmp/ai-guard-runtime-preflight.9CD3lD` : Docker Desktop29.9.2 Linux ARM64, image locale `sha256:7c64760cc4a5f29a40685cae2451afed914ce81671601166792cd087beab9db6`. Les services QA antérieurs sont conservés.
- [x] Écrire une entrée Cargo ignorée par défaut, opt-in explicite pour programme/endpoint/image/rootful ; racine neuve courte privée et canonique, source jetable disjointe, credentials de contrôle neufs, environnement nettoyé et preuves privées. Politique dédiée autorisant uniquement la commande bornée de l'essai, jamais modification de la politique utilisateur. H2 compilé avec `--no-run`, H4 compilé puis exécuté ;155 sources vérifiées pour chaque gel.
- [x] Relire effets, bornes, processus possédés et oracles. Aucun kill par nom, prune, réutilisation de volume, suppression de claim ou affaiblissement d'authentification. La revue H4 fait corriger signal réel du broker, OOM/exit/runtime distinguant sleep20 et projection Mounts explicite. Un échec avant Ready/Running est un FAIL de parcours, pas un test moteur réussi.
- [ ] Exécuter une seule session `/bin/sleep 20`, timeout5, réseau none, image locale épinglée. Exiger vraie identité et admission, Arm d'origine, slot Ready/prepared et CID Running avant de tuer uniquement le processus broker possédé. Distinguer échéance native et sortie naturelle de20s ; consigner l'écart de temps observé sans inventer une borne absolue.
- [ ] Garder gateway/gardien vivants, vérifier arrêt du même CID, redémarrer le même broker et vérifier aucune nouvelle Arm/claim/start/deadline ; contrôler audit intègre, quota et usage idempotent seulement après faits terminaux admissibles. Une absence de lecture ou une réconciliation manquante reste inconnue/FAIL.
- [ ] Préserver la racine et le gardien non vide tant que D5/disarm n'est pas qualifié. Nettoyage uniquement par voie runtime contrôlée sur ressources exactes avec preuve terminale ; aucun effacement pour obtenir un PASS. Vérifier source d'origine inchangée et absence de mutation des ressources QA préexistantes.
- [ ] Reproduire/corriger le premier défaut réel s'il apparaît, conserver les échecs et rejouer seulement ce qui est nécessaire, puis revue, preuves, commit et push. G09 reste FAIL tant que ses critères ne sont pas établis ; A1/A2/B1, Podman/Linux, capacité100, TTY, SSO et autres gates restent obligatoires.

Résultats : **H4 FAIL réel**,47,39s, et **H5 FAIL réel**,46,59s, tous deux avant faute. Worker `StartIssued`, CID `Created`, Arm/claim durables mais aucun Running. Les15 opérations de supervision de chaque demande sont ACKées, échec `worker/unavailable` après le claim. H5 retire le polling RecoveryStatus concurrent du writer, sans résoudre le défaut. Le probe natif H4 constate ensuite `stopping/prepared_once/created` ; la restauration peut elle-même établir prepared_once, donc cette observation ne localise pas le premier refus. Conserver les deux FAIL et leurs ressources. Aucun réarmement ou relèvement du timeout pour faire passer le test.

- [x] Implémenter et relire des diagnostics typés Prepare/garde/runtime, sans sérialiser la chaîne d'erreur arbitraire, sans changer les codes IPC ni déplacer le dernier garde. Raccorder `broker.batch.failure` à l'outbox V4 existante, effet explicitement inconnu.
- [x] Vérifier huit fonctions logicielles uniques, fmt, Clippy strict et release ; conserver les échecs Clippy intermédiaires et les deltas mécaniques. [Preuves H4/H5/D1–D3](evidence/2026-10-31/broker-runtime/README.md).
- [x] Exécuter H6 sur le diagnostic : FAIL d'observation45s pendant ImportVerification, aucun refus produit ni Arm worker établi. Recouper9/9 ACK, helper réellement terminé exit0 et teardown des enfants du test ; corriger seulement l'observation120s/global240s après revue, sans changer les5s du worker. La livraison du diagnostic reste non exercée, pas qualifiée par cet essai.
- [ ] Exécuter une seule tentative réelle instrumentée sur le nouveau binaire ; vérifier la livraison réelle du diagnostic, identifier le point précis de refus puis corriger avec régression. Ni la fraîcheur750ms ni l'expiration5s ne sont une cause démontrée à ce stade.
- [ ] Après démarrage obtenu, reprendre les critères SIGKILL/deadline/reprise ci-dessus, puis cycle terminal/disarm D5 et recette multi-client. SSO/Keycloak/PKCE et documentation découverte/entreprise demeurent un chantier obligatoire distinct, sans statut livré par ces preuves.

### Lot actif après H7 : frontière préclaim et lecture redondante

H7 a atteint WorkerArm ACKrev33 puis FailureObserved34, sans WorkerClaim ni événement typé. Le registre/Ready préclaim ne possède pas encore le diagnostic D3. L'intervalle mtime pendingArm→ACK atteint4,897s ; sans sample natif de l'erreur, cela reste compatible avec une expiration, pas une preuve de sa cause. Ne plus relancer le moteur à l'identique.

Deux tâches internes indépendantes, sans changement d'architecture ni de contrat d'autorité :

- Agent native : `enforcer/client.rs` et `enforcer/diagnostics.rs`, corps Ready commun avec wrapper public gardant les codes, factory de refus Register appelée seulement sur erreur avec métadonnées validées. Root : `executor/helpers.rs`, `executor/journal.rs` et `executor/diagnostics.rs`, contextes d'étapes fermés sans texte arbitraire ; mêmes vérifications et samples, aucun nouveau travail externe sur succès.
- Agent core : `broker/writer.rs`, `broker/journal.rs`, régression outbox existante. Déplacer l'adaptation V3→V4 sous le mutex du préload déjà obligatoire pour éliminer le `Journal.load` complet redondant du writer. Garder tous les contrôles du préload/postload, CAS, barrières, floors, refus de downgrade et règle V4 expected==head ; aucun cache de confiance ni changement à load_locked/persistent_sync.

Critères : conservation byte-à-byte des événements/ACK V4 lors d'une mutation lifecycle, conflit obsolète et downgrade refusés sans mutation, recontrôle des bindings après publication toujours fail-closed ; diagnostics Registration/Ready et étapes génériques sans secret/attribution inventée. Luna exécute les seules régressions pertinentes et fmt/Clippy/release après gel commun ; revue indépendante avant intégration. Une réduction de lectures vérifiée en source ne constitue pas une mesure de performance ni une correction démontrée de G09. La prochaine tentative réelle attend ce lot vérifié.

- [x] Lot logiciel vérifié :16 fonctions uniques PASS, fmt/Clippy strict/release RC0. Constat de revue sur l'observation d'une réservation héritée reproduit RED puis corrigé GREEN ; nouveau port réutilise status_locked comme Journal.load, sans réintroduire un Fullload Accepted. Renommage Exchange pour ne pas présenter une frontière comme un ACK. Revues traitées,158 sources/artefacts vérifiés ; [preuves](evidence/2026-10-31/broker-runtime/README.md).
- [x] Exécuter H8 sur ce gel, observation120s/global240s, Arm worker5s inchangée : FAIL d'observation, sans refus précis disponible. Aucun gain ou correction de startup déduit des tests logiciels. Conserver les racines/gardiens/ressources et ne pas contourner D5. Après Running seulement, reprendre la faute SIGKILL, deadline et reprise idempotente, puis le cycle terminal/disarm.

### H8 : attendre un résultat explicite dans le budget global existant

H8 exécuté sur `af4a868` : FAIL d'observation120s, head33 WorkerArm apparaît vers118,303s après Submit, seulement2,234s avant publication du FAIL. Aucun FailureObserved, diagnostic ou WorkerClaim ; le retour du postload/register n'est pas établi. Ce résultat ne justifie pas de correctif applicatif causal ni de nouvelle deadline.

Conception du seul harnais : retirer la coupure préRunning120s, garder la borne globale240s déjà existante et toutes les limites/oracles du worker5s. Pendant l'attente, lire au plus une fois par seconde le premier événement `broker.batch.failure` via le filtre HTTP existant `events?kind=broker.batch.failure&limit=1&order=asc`. Une erreur réellement rapportée fait échouer immédiatement la recette nominale ; elle ne constitue ni terminal, ni preuve d'absence, ni libération de quota. Capturer uniquement seq positive, UUID canonique et labels fermés validés ; jamais data/cause/commande arbitraire. Un ACK local d'outbox peut encore être pending au moment de ce FAIL. Une expiration globale reste indéterminée, sans succès ni refus produit inventé.

- [x] Root : modifier seulement `tests/broker_runtime.rs` et `tests/support/broker_runtime.rs`, parseur de marqueur fermé et polling HTTP borné, aucune lecture du writer ni modification applicative. Tests purs de page vide, marqueur réel sélectionné, labels/UUID/seq invalides et canari privé exclu.
- [x] Luna : compiler/exécuter ces seuls tests sans moteur, fmt/Clippy ; pas de rebuild applicatif car sources produit identiques à PR4. Revue indépendante du harnais avant effet.
- [x] Une tentative H9 sur ce harnais validé, avec budget global240s et Arm worker5s, sans toucher les cinq racines/gardiens H4–H8. Lire ensuite le diagnostic exact avant correction. Le diagnostic a été obtenu avant le global ; une expiration sans diagnostic aurait demandé un profilage ciblé, pas une conclusion causale.

H9 a atteint un résultat explicite : FAIL codé après232,70s, `readiness_before_claim / before_claim / slot_phase_mismatch`, sans sample local, aucun WorkerClaim/start. Diagnostic SQLite seq17 durable et naissance V4 rev35, ACK local encore pending lors de la coupure du test. Cela établit l'état nonReady du snapshot, pas sa cause ni une expiration. Trois tests purs du marqueur, fmt/Clippy et revue du harnais passent ; aucune réponse API simulée ne qualifie le moteur.

### Suite P1 : données acquittées et preuve de snapshot

Le mainteneur précise ensuite que la borne globale du test n'a pas besoin d'être aussi stricte : le scénario passe à600s pour sa préparation/reprise. L'Arm et les oracles du worker restent5s, et le premier diagnostic réel reste fail-fast. Cette marge de test ne change ni durée/quota du produit ni garantie temps réel. La précédente consigne interne de ne plus ajuster cette borne ne s'oppose pas à cette instruction récente.

- [x] Agent native, `enforcer/client.rs` et `enforcer/diagnostics.rs` : sur les seules erreurs de prédicat, copier phase/prepared/génération et clock du snapshot déjà lu, source fermée `helper_slot`. Valider la clock avant copie ; invalide omise avec flag, manquante inconnue. Champ optionnel boxé, absent des anciens payloads ; native_sample garde son sens local et reste absent avant sa lecture. Aucun nouveau garde/sample/RPC ou changement de code/seuil.
- [x] Agent core, `executor.rs` et `executor/{journal.rs,helpers.rs,journal_tests.rs}` : un Draft privé attaché à son scope/opération/head peut être réutilisé uniquement après Receipt writer OK, pour supprimer le Fullload du caller après Register. C'est une projection de données au head engagé, pas une autorité/fraîcheur. Wrappers RPC existants, fastpath ACK→freshfalse, génération/Ready/clock, pending/ACK durables et EmitOnce restent inchangés. Chaque mutation conserve CAS et Full pré/post du Journal ; conflit recharge et préserve Cancel/outbox ; erreur ou Unknown ne confirme aucune projection. Pas de fusion des autres loads sans preuve de concurrence/freshness.
- [x] Root : fixture `AuthenticatedBatchContext::for_executor_test` seulement `cfg(test)`, avec vraie vérification gateway active/schema3/runner/tenant/namespace avant construction ; aucun port non vérifié en production. Tests utilisent vrai Gateway/Store/Journal/Writer et un gardien IPC vide, jamais un moteur/modèle ; admission registry/FS reste une qualification séparée.
- [x] Luna après gel :20 fonctions ciblées PASS S1. Clippy refuse le seul `.err().expect()` du nouveau test ; S2 le convertit en `expect_err()` sans autre changement, fmt/Clippy strict/release RC0. Aucun test rejoué pour cet idiome.159 fichiers source et deux copies binaires vérifiés ; revue indépendante du Draft et du delta d'assertion. Aucun gain temporel ni correction du démarrage déduit de ces résultats ; [preuves et FAIL conservés](evidence/2026-10-31/broker-runtime/README.md).
- [x] Exécuter H10 sur le nouveau binaire, global600s/Arm worker5s : FAIL d'observation à600,04s, sans Arm worker ni diagnostic public ; ownership29 puis FailureObserved30 conservés. Intervention de nettoyage de cache concomitante et inventaire37→3, sans causalité établie. Ne pas qualifier cet essai comme reproduction isolée, expiration du worker ou preuve de D5 ; aucune relance à l'identique.

### Après H10 : rendre visible tout échec local sans inventer sa cause

H10 expire RC101 après600,04s, sans `broker.batch.failure` visible. La lecture des faits conservés trouve pourtant une FailureObserved locale worker/unavailable rev30, après ownership rev29, avant Created central/Arm/claim. Aucun démarrage qualifié. L'inventaire moteur37→3 est une anomalie distincte à expliquer ; aucune cause de ce refus ne lui est attribuée. Ne pas ajouter un essai à l'identique ni prolonger encore la borne pour ce défaut de visibilité.

**Contrat du lot :** `Execution.failure` doit tenter de publier un événement typé pour tout échec qu'elle observe, via le masquage/outbox/HTTP/ACK existant. Si aucune étape native/runtime/broker n'est connue, `broker_step=failure_observed` signifie seulement qu'un échec a été observé dans la phase indiquée. Texte arbitraire, secrets, cause supposée, effet/terminal/fencing et libération de quota restent exclus. Conserver les anciens labels, le payload v1 et les champs d'événement existants. Une livraison impossible reste signalée opaque ; elle ne devient pas un succès ou une preuve d'absence. La classification spécialisée peut rester partielle ; son rapport public a ce repli explicite.

**Frontières ciblées :** contexte fermé autour des seules opérations déjà présentes du callback ownership : commit, relecture, construction inspection Created et échange Created. Le label d'échange ne prouve ni POST ni ACK. Aucune nouvelle lecture, attente, horloge, barrière ou opération moteur ; aucune permission/start/Arm/claim/durée modifiée. Renommer le type interne PreclaimStep en BrokerStep s'il porte ces étapes plus générales, sans changer les valeurs historiques.

- [x] Agent Core, propriétaire exclusif `src/runner/broker/executor.rs`, `executor/{diagnostics.rs,journal.rs,journal_tests.rs,helpers.rs}` : écrire d'abord une régression sur les vrais Gateway/Store/Journal/Writer et le gardien IPC vide existants : une erreur privée inconnue produit actuellement FailureObserved mais aucun événement public. Attendre le RED Luna, puis implémenter le rapport fermé et les contextes ; réutiliser les fixtures, aucun nouveau service/moteur/mock permanent.
- [x] Root, propriétaire de `tests/broker_runtime.rs` et `tests/support/broker_runtime.rs` : test de marqueur `failure_observed` avant prise en charge, RED attendu sur label non accepté ; puis allowlist des nouveaux labels uniquement, toujours huit champs et aucun corps libre. Les règles de UUID/seq/origine/source, deadline globale600 et worker5 restent identiques.
- [x] Luna seulement : deux RED puis8 fonctions ciblées GREEN, fmt/Clippy strict/build release RC0.159 sources et copies binaires vérifiées ; aucun ancien20tests, moteur, fournisseur ou E2E rejoué. Le complément frontend est vérifié séparément ci-dessous.
- [x] Revue indépendante : source/RED/GREEN et artefacts recoupés, aucun P0/P1 restant identifié dans le delta ; données réelles, cause inconnue, audit/durabilité et ordre des appels conservés. [Preuves et limitations](evidence/2026-10-31/broker-runtime/README.md#h10--échec-local-durable-sans-diagnostic-public). G09 et D5 restent ouverts ; coûts du journal/cycle des gardiens seront mesurés et corrigés séparément, sans suppression opportuniste de ressources.

**Complément de présentation du même parcours :** la revue du front montre que ce kind est conservé mais affiché comme une ligne générique, sa phase/portée restant dans le JSON ; il est absent d'Actions sans command/decision. Corriger uniquement l'affichage avec les composants existants : libellé « Échec signalé », phase/étape fermées et effet non confirmé dans le résumé détaillé. Inclure ce kind dans Actions, sans le transformer en incident de sécurité, état terminal ou compteur. Les valeurs inconnues restent inconnues et aucun texte d'erreur libre n'est repris. Conserver palettes, thèmes, logo, navigation et permissions. `src/main.rs` sert un répertoire statique ; aucune dépendance du build Rust aux sources/dist du frontend n'a été trouvée, donc ces fichiers disjoints n'altèrent pas le gel Rust S4.

- [x] Agent frontend : seuls `front/src/model.ts`, `front/src/main.ts`, `front/tests/model.test.mjs`, sans CSS/config/dépendance. Trois RED puis formateur/détail et inclusion Actions ; inconnus/canari couverts, aucun reclassement automatique sécurité, pas de données fictives dans le produit.
- [x] Luna après gel frontend : modèle11/11 et build statique RC0, dépendances installées hors ligne et lock inchangé, Node25.6.1 consigné. Revue indépendante traitée,24 sources et4 assets dist vérifiés séparément des159 sources Rust. Navigateur/QA visuelle et Node24 NOT_RUN, aucune qualification graphique annoncée.
- [x] Complément final : un RED ciblé reproduit l'étape native connue affichée inconnue ; tables fermées enforcer/runtime et priorité broker connue → enforcer → runtime. Modèle complet12/12, build RC0 et revue indépendante finale ;24 sources et4 assets du gel final vérifiés. Aucun effet confirmé ni nouvelle qualification navigateur/moteur.
- [ ] Prochain P1 : mesurer les opérations du chemin durable sur un journal réel borné et qualifier une fermeture sûre des slots ; aucune nouvelle tentative équivalente ni retrait de gardien sans les preuves nécessaires.

## Cycle actif : établir puis sécuriser la référence

### Mesure active du coût durable — 1er novembre, base b32711d

Complément actif sur base `f6f93b3` : le petit graphe ne reproduit pas H10. Un second diagnostic opt-in doit copier uniquement ses enregistrements JSON autorisés vers une racine privée neuve, sans lock/sync/écriture métier sur l'origine ni ouverture des chemins référencés. Scope et reçus importés sont des données de recherche, jamais une identité ou un permis. La copie et les bytes restent privés, hors Git ; l'origine est relue/revalidée sans prétendre à un snapshot atomique.

- [x] Conception ciblée :31révisions/866540octets, bindings None strict, whitelist/bornes/FD, destination disjointe et conservée ; mesurer deux load+status du vrai Writer puis le vrai ChainVerifier sur bytes en RAM, sans mutation/replay/start.
- [x] Agent Core : second probe uniquement dans `src/broker/journal/tests.rs`, JSON fermé sans path/identité/contenu, compteurs source zéro écriture/sync et segment mémoire zéro FS ; refuse contexte divergent plutôt que copier un secret ou réparer.
- [x] Revue indépendante de l'import de données privées avant première exécution ; Luna exécute une fois la cible exacte sur la QA H10 explicitement sélectionnée, en debug,600s coopératif/caller60s, sans moteur/modèle/navigateur. S7 PASS,71,42s ; deux loads16,53/17,70s et status16,92s, validation RAM13,57s sans IO. Origine sans demande de sync/création/publication,160 sources avant/après et archive vérifiées ; [preuves et limites](evidence/2026-11-01/journal-cost/README.md#s7--mesure-sur-copie-privée-du-graphe-riche).
- [x] Supprimer le doublon canonique local avec contrôles/erreurs conservés, goldens observés avant code, neuf fonctions uniques PASS, fmt/Clippy/release et revue indépendante. Les deux FAIL et le seul ajustement de harnais fonctionnel restent documentés.
- [ ] Identifier et qualifier le coût principal restant et la latence release représentative. La comparaison S7 ne démontre aucun gain ; ne pas poursuivre des optimisations supposées ni supprimer des barrières/cache de confiance pour obtenir un succès.

Factorisation bornée vérifiée : les checkpoints V2/V3/V4 réutilisent les bytes déjà validés, sans cache ni changement de wire, domaines, ordre d'erreur, caps ou contrôles. Luna a observé trois empreintes publiques sur l'ancien code, figées avant modification ; régressions, probe, checks et revue passent. La mémoire reste13,96s contre13,57s sur ces échantillons debug : aucun gain de latence ni cause H10 démontré. [Résultats et limites](evidence/2026-11-01/journal-cost/README.md#résultats-finaux-du-lot).

Premier contrôle après correction : six fonctions PASS, puis canari outbox512 sur `Io`. Reproduction à budgets inchangés : branche timeout du caller, puis résultat backend réussi et send refusé ; deux FAIL conservés. Marqueurs retirés exactement, seul test fonctionnel512 à60s déjà admises par Writer, autres fixtures10s et produit inchangés. Les deux coutures outbox, le canari timeout/ownership, S7 et les checks passent sur le gel final. Latence release, cause H10 et capacité100 restent à qualifier ; aucun nouveau moteur. [Preuves et limites](evidence/2026-11-01/journal-cost/README.md#diagnostic-à-budgets-inchangés).

Besoin : obtenir une mesure du chemin Journal/Writer avant toute optimisation ou répétition équivalente de H10. La lecture source trouve des relectures/validations/synchronisations répétées ; elle ne prouve pas leur coût ni la cause de H10. Les résultats n'autorisent aucun retrait de slot, suppression de barrière ou extension de deadline.

Architecture inchangée : instrumentation **uniquement `cfg(test)`**, vraie copie neuve de journal privé, Writer réel et mêmes fonctions d'IO. Aucun gateway, enforcer ou moteur nouveau. Les opérations d'IO, leur ordre, leurs erreurs, CAS, floors et synchronisations restent conservés. Un collecteur de mesure activé explicitement traverse le thread Writer ; ses compteurs fermés n'enregistrent aucun contenu, path, hash, identité ou credential. Distinguer temps inclusifs imbriqués et durée exacte des appels `File::sync_all`, sans nommer un syscall non observé.

- [x] Diagnostic source : cartographier Full load/status, pré/post de mutation, relecture post-reçu et publication du pointeur ; choisir les fixtures existantes plutôt qu'une simulation du stockage.
- [x] Agent Core, propriétaire exclusif de `src/control_fs.rs`, `src/control_fs/persistent.rs`, nouveau `src/control_fs/cost_probe.rs` et ajout ciblé à `src/broker/journal/tests.rs` : probes compilés seulement pour les tests, jamais activés implicitement ; réutiliser `intent_for`/`open` et un journal réel sans binding secret.
- [x] Ajouter une seule mesure opt-in ignorée par défaut, `broker::journal::tests::measure_durable_journal_cost_without_runtime`, lancée seule : deux lectures Writer au même head, status et mutation lifecycle acquittée aux heads0 et8. Vérifier head/chaîne/reçu et compteurs, sans seuil arbitraire de rapidité ni conclusion sur capacité100. Les temps d'installation du journal restent séparés des opérations mesurées.
- [x] Luna : exécuter la cible exacte une fois après gel et conserver commandes, versions, RC, mesures et manifeste avant/après ; sélectionner les régressions FS pertinentes, fmt/Clippy/build, sans E2E général, modèle ou moteur. Une erreur/timeout conserve les preuves et n'autorise aucun restart automatique du même essai.
- [x] Observation parallèle passive du gardien H10 retenu : attribution PID par FD de verrou et socket exacts, puis deux snapshots scalaires20s, sans argv/contenu/IPC/signal ; absence ou ambiguïté n'est pas zéro coût. Conserver les biais de collecte des enfants et ne pas en déduire un coût par Arm.
- [x] Revue indépendante des probes, chemins inchangés et interprétation traitée ;7 fonctions et fmt/Clippy/release RC0,160 sources et binaires vérifiés. [Mesures, erreurs de harnais et portée](evidence/2026-11-01/journal-cost/README.md). D5 et G09 restent ouverts.
- [ ] Prochain critère runtime : implémenter et qualifier le [profil Docker engagé au parent accepté](evidence/2026-11-01/docker-launch-profile/README.md), puis un témoin vivant de fin normale lié à l'émetteur exact, terminal/ACK et retrait durable. Le contrat du profil est relu ; premier RED attendu constaté dans le checkout frère docker-launch-profile après V1 positif, aucune implémentation disponible. La recherche primaire Docker reste conditionnelle, sans attestation des binaires ni témoin construit ; aucune réécriture des configurations/credentials ou retrait d'anciens slots sur cette base.

### Lot P1 livré — authentification avant traitement HTTP

Besoin constaté le 30 octobre dans `src/api.rs` : les extracteurs JSON/query s'exécutent avant l'autorisation placée dans le corps du handler ; une requête non authentifiée peut donc imposer lecture/désérialisation. Le balayage des tenants recalcule également les digests de la même entrée et des credentials configurés à chaque comparaison. Il s'agit de défauts à reproduire et corriger, pas d'une reproduction rétroactive du rapport de sécurité figé.

Conception retenue dans les frontières existantes : index privé des digests construit au démarrage, identité typée obtenue par `FromRequestParts` avant Path/Query/Json, rôle runner requis avant lecture de tout corps de mutation. L'identité choisit toujours tenant et Store ; aucun identifiant client ne les sélectionne. Pas de middleware avec deuxième authentification, endpoint nouveau, dépendance ou changement de stockage. `token_matches` public reste compatible pour le dashboard.

Propriétaires : agent core pour `src/api.rs` et un éventuel module `src/api/auth.rs` ; Luna pour `tests/api_auth_boundary.rs` et exécution des tests ; root pour contrats/documentation/intégration ; revue indépendante après delta gelé. Aucun accès à une clé réelle, moteur ou fournisseur n'est nécessaire.

- [x] Conserver un RED sur le code inchangé : corps non autorisé non consommé, rôle viewer refusé avant JSON, query invalide non analysée avant auth ; cas déjà sûrs conservés comme PASS.
- [x] Vérifier sur un vrai HTTP local que des headers non authentifiés obtiennent 401 sans attendre le corps annoncé ; un seul gateway enfant possédé par le test Rust, aucun processus en fond hors supervision du harnais.
- [x] Implémenter index et guards typés, borne de token 4096 octets, refus des headers Authorization ambigus, sans prétendre à un lookup constant-time ou à une résistance globale au DoS.
- [x] Préserver `/healthz` public, aliases `/api` et `/api/v1`, séparation A/B, rôles et quotas, 503 en configuration d'identité invalide et erreurs habituelles des entrées authentifiées invalides. Aucune mutation d'audit sur ces refus.
- [x] Exécuter la nouvelle cible et les régressions pertinentes : 6 + 45 PASS, fmt/Clippy/build et revue acceptés ; aucune campagne E2E/moteur/fournisseur systématique.
- [x] Synchroniser contrats, suivi des candidats d'audit et preuves ; prochain P1 indépendant : arrêt HTTP/SSE borné. Aucun gate global clôturé par ce lot.

En parallèle, la proposition de supervision permanente dans `BROKER_SUPERVISION.md` a été actualisée pour les plans fournisseurs v3, relue puis approuvée par le mainteneur. Cette direction est maintenant autorisée à l'implémentation. Aucune installation de service OS ni migration existante implicite.

### Lot P1 livré — arrêt HTTP/SSE borné

Diagnostic : gateway et dashboard attendent sans limite `with_graceful_shutdown` dans `main.rs`, alors qu'un SSE valide peut rester ouvert indéfiniment. L'ingress possède déjà une borne de drainage de 5 s. Reprendre ce comportement pour les deux services, dans le même binaire et sans changer la donnée d'audit ou le flux SSE en fonctionnement normal.

Propriétaires : core pour `src/main.rs` uniquement (helper privé si nécessaire), Luna pour `tests/http_shutdown.rs` et ses exécutions, root pour documentation et intégration. API/auth reste gelé. Baseline main et release auth préservées dans `/tmp/ai-guard-http-shutdown-baseline` avant édition.

- [x] RED avant modification : vrais gateway/dashboard avec SSE authentifiés effectivement ouverts, SIGTERM ciblé, sortie bornée absente ; processus possédés par le test Rust, arrêt forcé du seul child en dépassement et échecs conservés. Contrôle sans flux déjà passant.
- [x] Démarrer le délai de 5 s au signal, jamais au démarrage. Laisser finir les requêtes ordinaires puis sortir même si le SSE reste ouvert ; nommer la fermeture après drainage expiré sans promettre un arrêt temps réel sous blocage OS/IO global.
- [x] Préserver les signaux/arrêts rapides au repos, health publique, accès viewer et requêtes normales ; pas de nouvel endpoint, paramètre tenant, dépendance ou changement de stockage. Setup SIGINT/SIGTERM fallible avant readiness, plus d'expect dans ce chemin.
- [x] GREEN : deux tests HTTP/SSE, trois régressions ingress/tenants, fmt/Clippy/build et revue acceptés. Aucun Docker/Podman, navigateur ou modèle pour ce lot.
- [x] Documenter fenêtre de drainage, flux interrompus et reprise par curseur ; conserver séparément les limites keep-alive/clients lents/audit bloqué et la supervision runtime après mort du broker.

### Prochain chantier autorisé — supervision durable

Le mainteneur a approuvé l'implémentation de `BROKER_SUPERVISION.md` (broker permanent et gardien TTY du même binaire Rust). Les conditions de constructibilité BSR-01/02/03 sont traitées au niveau design. Installation/chargement OS, cloud/PKI et destruction de données ne sont pas autorisés par cet accord. Les exigences D5/fencing et récupération du service figé demeurent ouvertes jusqu'à preuves.

- [ ] Verrouiller le contrat central ArmTarget worker/helper, reçu/CAS/claim, migration sauvegardée et compatibilité legacy, avec interfaces existantes réellement identifiées dans le code.
- [ ] Verrouiller IPC/profils, identité avant sélection de profil, Intent/Checkpoint, horloge continue, ownership runtime/output et ordonnanceur indépendant avant édition transversale.
- [ ] Implémenter puis qualifier la verticale complète broker batch, y compris import/scan/export supervisés, perte du client, crash/restart et absence de double start ; ne pas activer un nouveau schéma obligatoire avant disponibilité de ses clients.
- [ ] Implémenter et qualifier le gardien et le parcours TTY complet sans fallback batch ou régression finale du transport actuel.
- [ ] Traiter fencing des starts ambigus, panne/gel du service, mises à jour/migrations et recette A1/A2/B1 ; conserver les limites et gates tant que non prouvés. Aucun composant simulé ou simple timer local ne remplace ces obligations.

Premier sous-lot immédiatement applicable : arrêt sur snapshot vérifié. La revue BPR-03 a établi que `stop_owned(handle)` ne compare pas le Created complet avant kill. Le nouveau port `stop_snapshot_owned(&OwnedContainerSnapshot)` doit valider CID, nom, image, fingerprint/version, restart et allocation avant effet, revérifier l'autorité moteur, puis comparer l'observation résultante. Une inspection manquante ou divergente refuse l'effet et conserve l'incertitude ; un Created non running ne devient pas une allocation terminale.

- [x] Luna : reproduction ciblée de l'ancien port, puis six cas de divergences/snapshot exact/erreur d'inspection avec fixture moteur Rust explicitement identifiée ; un test de fixture ne qualifie aucun moteur.
- [x] Runtime : `src/runtime.rs` et `src/runtime/managed.rs`, arrêt strict pour les chemins possédant une baseline, y compris timeout et annulation interactive. Contrat des anciens chemins sans snapshot distinct ; aucune garantie de supervision ajoutée.
- [x] Root : six arrêts de `src/runner/managed.rs` raccordés aux snapshots complets ; terminal/audit/usage/cleanup inchangés.
- [x] Luna : défaut initial reproduit sur Docker réel, CLI tué après Running avec worker de courte durée. Premier essai non concluant (comparaison erronée tag/image immuable), puis unique retry corrigé : Running après7,0019s malgré limite5s. Aucun modèle ni test global A1/A2/B1. Deux arrêts/réconciliations ciblés, audit/quota/usage vérifiés, preuves et volumes conservés ; [résultats](evidence/2026-10-30/cli-crash/README.md).
- [x] Revue indépendante acceptée, 32 tests ciblés, formatage/Clippy/release PASS ; [preuves et limites](evidence/2026-10-30/runtime-stop-snapshot/README.md). Ce sous-lot ne ferme pas la supervision permanente ni les gates de crash/reprise.

Préparation centrale : [DTO et helper policy](evidence/2026-10-30/supervision-contract/README.md) compilés/testés/relus, initialement sans API ni activation par défaut. Nil boot et lien révision claim/arm corrigés par RED/GREEN, hashes legacy conservés. Le [Store transactionnel v3](evidence/2026-10-30/supervision-store/README.md) est ensuite validé indépendamment : 94 tests uniques,24 concernés rejoués après mutex final, revue close et fmt/Clippy/release PASS. Avant tout premier start v3, import exact, manifeste concordant et autorité fraîche privée sont requis. Clone/reopen restent maintenance ; legacy Created n'est pas réactivable.

Sous-lot suivant [API et horloge native](evidence/2026-10-30/supervision-api-clock/README.md) : neuf routes v1 uniquement, guards avant corps, parent helper retrouvé dans le Store authentifié ; producteur d'horloge continue Rust avec boot retenu et arithmétique vérifiée. Le journal immuable, l'IPC, l'ordonnanceur et les consommateurs de cette horloge restent à intégrer. Aucun cutover avant batch/helpers/TTY qualifiés, aucune réactivation implicite du schéma3 ; les résultats et limites de validation restent dans le dossier du lot.

### Lot livré — intention d'exécution immuable

Spécification : [intention/checkpoint du design approuvé](BROKER_SUPERVISION.md#intention-immuable-checkpoints-et-secrets). Base `11eb871db40ca9893a4492547561ec604247257c`, worktree `codex/broker-supervision`. Besoin : le hash d'une exécution ne peut porter sur `RunPlan.managed`, modifié après chaque transition ; il doit conserver le choix fournisseur et la demande initiaux. Aucun nouveau service ni changement structurel hors de la direction déjà approuvée.

Fichiers : core possède `src/broker/intent.rs`, l'export de `src/broker/mod.rs` et une couture privée bornée dans `src/runner.rs`. Un helper pur limité à la comparaison du prompt dans `src/provider.rs` peut réutiliser le parseur bootstrap existant, sans modifier ses appels historiques. Luna possède `tests/broker_intent.rs` et ses éventuels JSON de référence dans `tests/fixtures/`. Root possède suivi, intégration et preuves. Les autres modules et les plans historiques restent inchangés.

Contrat public du composant : `ExecutionIntentV1::from_plan_bytes(&[u8])`, `from_bytes(&[u8])`, `validate()`, `canonical_bytes()` et `intent_hash()`, avec résultat et erreur typés. `from_plan_bytes` projette le vrai `RunPlan` via une fonction privée au crate, sans exposer son type ou ses credentials. Les données figées sont privées, sans setter ; getters de métadonnées et snapshots typés pour le futur journal. Aucun appel réseau, accès au disque, résolution Auto ou vérification supposée du contenu d'un binding par ces fonctions.

Le format1, distinct des plans1/2/3, contient version/plan_version/operation_id/session/options et demande initiales, provider_input et référence de binding facultatives. Projection d'options fermée, sans modifier rétroactivement le serde de `RunOptions`. Pas de `managed`, de FD, de bytes de credential ni d'approbation déjà appliquée. UUID non nil, versions connues, empreintes hexadécimales64, forme/cohérence commande/session/profil/transport/fournisseur vérifiées ; une entrée ancienne sans identité initiale suffisante est refusée, jamais réparée ou enrichie implicitement. Ce refus du nouveau composant ne modifie pas la lecture des anciens plans.

Limites : plan source32Mio au plus, intention canonique256Kio au plus ; commandes et métadonnées soumises aux bornes existantes du domaine, chaînes privées restantes bornées. Aucun path canonicalize ni lecture de source pour fabriquer le hash. Le JSON canonique trie récursivement les clés, conserve ordre des tableaux et texte ; le SHA256 porte sur `ai-guard.execution-intent.v1`, octet NUL, puis ces octets UTF-8. Les UUID typés ont leur représentation serde canonique explicite. Les hashes historiques de décision/admission/provider ne changent pas. Debug/Display/erreurs du composant restent opaques pour les valeurs privées.

Points de revue : options inconnues dans le nouveau format ; session/argv/provider incohérents ; approval préexistante ; binding tourné ou absent ; références privées révélées par diagnostics. Le binding désigne seulement des métadonnées à ce stade : sa présence/digest réels seront exigés avant l'ACK du journal, sans substitution ni extraction OAuth.

Portée précise du hash : identité de l'invocation et des références présentes dans le plan, sans attester les bytes d'un credential manuel. Le journal doit engager avant ACK un enregistrement privé immuable liant `intent_hash`, payload Submit exact, scope authentifié et bindings des handoffs manuel/fournisseur (identité, nom privé, digest, noms autorisés). En mode manuel, persister depuis le `PrivateFile` retenu initial, synchroniser fichier/parent, puis engager le marqueur d'acceptation ; un Snapshot éphémère unlink-on-drop ne suffit pas. Le rejeu relit cet engagement et le même binding, jamais `secret_env_file` ou Auto ; binding absent/altéré bloque admission/start sans nouvelle source. Ce contrat du journal est à implémenter/tester séparément ; il n'est pas livré par cette projection pure et ne change pas les plans1/2/3.

- [x] Luna : RED ciblé avant code, avec golden indépendant du domaine/octet NUL et JSON canonique ; ce RED préparatoire n'est pas une vulnérabilité du broker existant.
- [x] Core : format/validation/hash et projection privée ; mêmes octets/hash après mutations managed/pending/résultat ; changement de champs stables et de binding couvert.
- [x] Luna : roundtrip strict, champs/version/UUID/borne/cohérences refusés, source de credential inexistante non consultée, diagnostics opaques et canaris historiques ciblés. Six fonctions finales et douze canaris PASS, sans additionner les rejeux ; les canaris ne sont pas répétés après le seul delta d'opacité.
- [x] Revue indépendante : BIR-01 sur désérialisation publique des snapshots reproduit puis fermé ; oracles de bornes affinés. Fmt/Clippy strict et unique release PASS, [preuves](evidence/2026-10-30/broker-intent/README.md).
- [x] Root : preuves, limites et lot relu préparés pour commit/push ; intégration suivie par Git. Le [complément clock Linux ARM64](evidence/2026-10-30/broker-clock-linux/README.md) concerne le gel précédent et conserve sa portée distincte.

Prochaine priorité : journal privé réellement durable, avec bindings des handoffs manuel/fournisseur et rejet de toute acceptation incomplète ; puis IPC/service/ordonnanceur/gardien. Aucun ACK accepté, effet runtime, migration ou réactivation n'est délivré par le seul format d'intention.

### Lot validé pour intégration — journal privé durable et bindings

Base `3372150fdb99d8bc71700a33dd1d49c0e75c45b5`. Le [contrat du journal](BROKER_JOURNAL.md) précise la persistance requise avant ACK : scope, Submit exact, intention immuable, checkpoint typé et bindings des credentials retenus. Ce composant ne choisit pas l'identité authentifiée et n'accorde aucun lancement ; IPC, ordonnanceur, gardien et qualification du crash restent les étapes obligatoires suivantes.

Propriétaires : core pour `src/broker/journal.rs`, ses types, la couture checkpoint dans `src/runner.rs` et l'extraction pure des parseurs existants ; agent filesystem pour `src/control_fs/persistent.rs` ; Luna pour `tests/broker_journal.rs`, les hooks `cfg(test)` et la preuve SQLite séparée `tests/journal_sqlite.rs`. Root conserve contrat, dépendance, preuves et intégration ; revue indépendante sur sources gelées.

- [x] Implémenter création exclusive, fsync fichier/parents, verrou conservé, marqueur accepté final, révisions immuables et CAS du pointeur ; toute erreur conserve un état relisible ou explicitement interrompu.
- [x] Vérifier None/manuel/fournisseur, rejeu sans relecture de source, rotation/conflit, secrets littéraux décodés, plafonds logiques et diagnostics opaques.
- [x] Reproduire puis corriger le rejet d'un champ connu nullable dans le nouveau constructeur de checkpoint ; conserver les lecteurs historiques et refuser champs inconnus et JSON surnuméraire.
- [x] Exercer les vraies opérations filesystem sous erreurs injectées et neuf interruptions SIGKILL ; distinguer hooks du harnais, tests produit et intervalle rename/fsync non testé.
- [x] Fermer les écarts de versions imbriquées trouvés en revue, avec RED/GREEN sur un vrai checkpoint managed valide ; préserver les versions historiques connues.
- [x] Vérifier dans le même processus que Store le refus d'un hardlink SQLite avant ouverture, avec observation des locks par un enfant indépendant ; corriger et conserver les échecs du harnais.
- [x] Reproduire la course de première création du verrou avec deux vrais processus, puis rendre Busy sur AlreadyExists après une unique relecture contrôlée ; aucun retry des refus de sûreté.
- [x] Exécuter les26canaris pertinents des parseurs partagés, fmt/Clippy strict et release ; macOS et Linux ARM64 en VM ont leurs preuves distinctes, sans qualification de worker.
- [x] Traiter la revue finale, publier preuves et limites, préparer le lot cohérent pour commit/push suivi par Git, sans modifier le déploiement QA ni activer le schéma3.

Les plafonds d'admission du journal ne sont pas des quotas physiques bytes/inodes. SIGKILL ne prouve pas la résistance à la panne électrique ; aucun de ces tests ne clôture G09 ou la recette A1/A2/B1.

Suite obligatoire : le complément ci-dessous vérifie contexte managed et plancher mémoire sur rollback, puis le raccord IPC/service/ordonnanceur doit compléter la verticale batch/helpers. Le simple composant de stockage ne ferme pas les critères d'activation et ne justifie pas un cutover du CLI.

### Lot validé pour intégration — contexte produit par le runner et plancher du journal

Base `537f8d2c072139bb4ced0f3b7bd385f15fb1e9dd`. Luna possède les deux nouvelles fonctions dans `tests/broker_journal.rs` ; root possède le correctif éventuel de `src/runner.rs`, les preuves et l'intégration. Un agent Rust cartographie en lecture seule le futur service/IPC et ses coutures réelles ; aucun service n'est activé par ce travail.

Les premiers tests managed/plancher ont passé avec une fixture Store-backed. La cartographie a ensuite identifié une divergence : `worker_spec` place `plan.operation_id` dans le spec et le handle runtime, tandis que le nouveau validateur et sa fixture attendaient `worker.attempt.attempt_id`. Ces UUID ont des rôles distincts. Le défaut a été reproduit puis le consommateur corrigé selon le producteur, sans modifier les exécutions historiques ni accepter indistinctement les deux identités.

- [x] Vérifier initial rev0 obligatoire puis advance/load/reopen avec le checkpoint managed cohérent et les refus tenant/origine/spec/handoff/worker/identité physique ; pas de managed accepté directement en rev0.
- [x] Vérifier rollback du pointeur valide contre le writer vivant et ses clones, puis lecture conservée après drop/reopen sans prétendre à un anti-restore.
- [x] Conserver les sorties brutes et gels de chaque étape,12tests ciblés/fmt/Clippy/release PASS ; correctif et oracles relus indépendamment.
- [x] Mettre à jour les limites, fermer la revue et préparer le lot pour intégration/push suivi par Git ; poursuivre la verticale service/batch/helpers, sans modifier les JSON RunPlan1/2/3.

### Lot P1 validé — bootstrap explicite d'un contrôle de supervision frais

Base `f46a23ecb8d17f098d325c017536e7c8a146ed83`, worktree `broker-service`. Couture nécessaire à la verticale déjà approuvée : le gateway actuel ne crée que le schéma2 et une réouverture du schéma3 reste en maintenance. Ajouter une création exclusivement neuve avec sauvegarde vérifiée et activation dans cette seule origine ; ne pas changer le défaut2 ni réactiver une base existante/restaurée.

Core possède `src/audit.rs`, `src/main.rs` et la couture minimale de références de chemins dans `src/gateway_config.rs` ; Luna possède `tests/supervision_bootstrap.rs`, la correction privée du clone dans `tests/supervision_store.rs` et toutes les exécutions. Root garde `images/control.Dockerfile`, contrat, suivi, preuves et intégration. Factory implémentée : `Store::create_supervised_for_tenant(path, tenant_id, redactor, max_events, backup_path) -> Result<Store>`, via le vrai `StoreStorage::create_new` exclusif, puis la migration3 existante. Initialisation et garde réutilisés sans refonte du VFS. Le backup existant ne peut être écrasé et la cible DB préexistante est refusée avant ouverture SQLite.

Option gateway implémentée : `--initialize-supervised-stores BACKUP_ROOT`, explicitement demandée, utilisable en local ou avec registre A/B. Chaque backup reste privé, dans un fichier distinct dérivé du SHA256 du tenant exact ; ce nom ne constitue aucune identité authentifiée. Une erreur empêche serve ; les nouveaux partiels éventuels restent conservés et les stores existants intacts. Aucun bootstrap multi-base globalement atomique n'est promis.

- [x] Capturer RED de fonctionnalité absente avant code ; ce RED ne constitue pas une vulnérabilité.
- [x] Vérifier création neuve3 active, sauvegarde2 intègre, reopen maintenance et défaut2 inchangé ; barrières d'ancêtres ajoutées en revue.
- [x] Vérifier refus des cibles/backups existants sans écrasement, erreurs d'entrée et préservation des locks SQLite existants avant toute réacquisition possible par le parent.
- [x] Exercer le vrai gateway loopback sur A/B, identités/namespaces distincts, restart sans flag en maintenance, flag réutilisé refusé ; aucun worker ou modèle.
- [x] Vérifications natives ciblées7+39fonctions, fmt/Clippy/release ; fixture clone600 indépendante de l'umask. Complément Docker du contrôle A/B avec parent `/run/secrets` privé, sans qualifier worker ou build multi-stage complet.
- [x] Fermer la revue documentaire et préparer l'intégration du lot testé ; aucun constat bloquant résiduel.
- [ ] Poursuivre checkpoint V2/IPC/ordonnanceur/batch/helpers, puis démontrer l'arrêt autonome sur un vrai moteur après mort du CLI.

Ce lot ne lance aucun worker supervisé et ne corrige pas seul G09. Pas d'installation OS, modification QA, opération cloud ou nouvelle dépendance d'infrastructure.

### Lot P1 validé — checkpoint de supervision distinct V2

Base `3e2cffa86c0d0c4784507e78f289419a9e39890f`, worktree `broker-service` propre après push vérifié. Besoin : le payload runner V1 ne conserve pas les armements/claims/helpers du protocole3. Le futur service doit pouvoir réessayer une requête exacte sans régénérer son identité ou recharger un droit de start. Ce lot livre ce stockage typé ; IPC, ordonnanceur et effets runtime restent le chantier immédiatement suivant, sans clôturer G09.

Choix dans l'architecture autorisée : révisions V2 au sein du même journal privé et de sa publication durable, enveloppant le payload runner V1 et un état de supervision. Conserver tous les bytes/domaines des révisions V1, le rev0 initial V1, les plans1/2/3, bindings, scope, accepted/current et limites persistantes. Un champ JSON libre dans le payload historique cacherait le changement de contrat ; un fichier parallèle non transactionnel désynchroniserait les faits. Aucun des deux n'est retenu.

Core possède `src/broker/checkpoint.rs` nouveau et son sous-module privé `checkpoint/validation.rs`, l'export `src/broker/mod.rs`, `src/broker/journal.rs`, `src/broker/journal/types.rs`, une couture pure limitée de contexte dans `src/runner.rs`, puis le raccord craté au reducer existant dans `src/audit/control.rs` et son réexport `src/audit.rs` autorisés après revue. Luna possède `tests/broker_checkpoint.rs`, les éventuelles fixtures et toutes les exécutions Cargo. Root possède ce contrat, les décisions, preuves et intégration. Les autres sources, tests existants, défaut2, Dockerfiles et QA restent gelés. Revue indépendante avant intégration.

API fixée : `BrokerCheckpointV2::new(CheckpointPayloadV1, SupervisionCheckpointV1)`, `SupervisionCheckpointV1::new(Uuid, Vec<HelperPreparationV1>, Vec<SupervisionOperationV1>)`, `HelperPreparationV1::new(HelperExecutionPolicyV1, HelperPermissionV1, Option<OwnedContainerSnapshot>)` et `DecisionAnswerV1::new(Decision, DecisionReceiptV1)` retournent `JournalResult<Self>`. Versions2/1 implicites, champs wire privés et Debug opaque. Getters étroits `runner()`, `supervision()`, `control_namespace_uuid()`, `helpers()`, `operations()`, plus sérialisation canonique/from_bytes fermées pour l'enveloppe. `Journal::advance_supervised(Uuid, RevisionRefV1, &BrokerCheckpointV2)` retourne le reçu de checkpoint existant ; `AcceptedJobV1::broker_checkpoint_v2()` donne l'option typée, `checkpoint()` garde la vue runner V1.

L'état contient les vrais DTO pour décision, admission, mutation worker, armement avec parent historique/snapshot possédé, claim worker, claim helper, observation helper, cleanup helper et export. Réponse absente signifie inconnue, sans affirmer que le RPC n'a pas été envoyé. Maximum128opérations,32helpers,64réponses inconnues ; admission unique et décision limitée aux phases initiale/approuvée. Aucun booléen de permis, rôle restaurable, erreur HTTP libre ou nouveau `Value`. Helper préparé avec politique déterministe avant create ; `owned` progresse seulement de None à un snapshot immuable. Toute authenticité centrale reste à recouper par le service.

Invariants à appliquer : lecture stricte par version de chaque révision et son domaine de hash, chaîne `V1+ V2*`, jamais V2→V1. Promotion refusée si **une quelconque révision V1 confirmée** contient managed, même si la tête paraît vide ; pas d'adoption implicite d'un ancien worker. Première admission pending possible avec managed=None, sans inventer Worker/attempt. Recouper namespace/scope, intention, demande, launch, binding et les liens requête/réponse disponibles. Dès tout helper préparé, Arm ou owned, exiger le vrai `managed.binding=Some` ; aucune copie d'autorité supplémentaire. Une vue de contexte en références, limitée à la crate et sans IO, peut relier ces champs sans rendre ManagedCheckpoint public. Arm.parent doit correspondre intégralement à un Worker d'une réponse antérieure acquittée : il conserve cette révision historique, pas la dernière révision du worker. Les références causales viennent des opérations précédentes acquittées.

Requêtes et réponses connues sont immuables, listes append-only. Toute nouvelle opération apparaît avec response=None ; elle ne peut passer à Some que si cette même requête figurait dans la révision précédente. Tout nouveau helper apparaît avec owned=None puis acquiert un snapshot dans une révision suivante. Valider ces règles à la première V2 et sur toute la chaîne chargée, pas seulement lors d'un nouvel advance. Une décision initiale et sa phase approuvée restent distinctes même si leur request_hash est identique ; comparer chaque demande historique à l'intention normalisée, sans la forcer égale au current_request approuvé ultérieur. Dans V2, les anciens pending/uncertain/import du runner restent vides. Ces invariants rendent l'ordre de persistance vérifiable ; ils ne prouvent pas que l'appelant a réellement attendu le fsync avant d'émettre un RPC.

Précisions issues de la revue du premier gel : l'admission unique, pending ou acquittée, porte toujours la demande **exactement égale à runner.current_request**, même sans managed. Les anciennes opérations Decision restent normalisées et distinctes ; cette règle ne les force pas à changer de phase. Les payloads WorkerMutation suivent les formes/bornes réelles du Store (motif Unknown, inspection Created, observation Observe, cohérence CleanupResult). Les résultats complets sont dérivés du vrai parent historique et de la transition via le reducer pur existant, aussi pour le StartClaim interne de WorkerClaim, avec vérification séparée de son receipt sous le bon domaine. Couture limitée et cratée autorisée dans `audit/control.rs` et réexport `audit.rs` ; aucun SQL, effet, hash ou contrat historique changé, aucune duplication de machine d'états.

Les révisions worker acquittées sont uniques par worker/attempt/révision parmi Admission, WorkerMutation et WorkerClaim. Plusieurs requêtes pending concurrentes restent représentables ; deux ACK contradictoires issus du même parent ne deviennent pas deux branches valides. Les parents historiques d'Arm restent de simples références et ne créent pas de nouvel ACK. Vérifier aussi le refus des champs inconnus imbriqués derrière les enums internally tagged, par les voies publiques de désérialisation et par le loader. Ces vérifications ne présentent toujours pas le journal comme une autorisation de mutation ou une preuve d'authenticité centrale.

Le raccord au vrai runner conserve son état préparatoire : manifest/archive existent avant RPC d'admission. `managed=Some` avec `worker=None` est donc représentable pour une Admission enregistrée pending ou refusée, aussi bien que `managed=None`. Distinguer la requête demandée de l'admission acceptée : `managed.admission` égale exactement la requête persistée, alors que worker, helpers, owned, Arm et claims nécessitent toujours le vrai ACK accepté. Aucun second fichier d'archive, Worker fictif ou droit d'exécution n'est créé pour contourner cette étape.

CAS/replay : nouveau hash couvre tout V2. Rejeu exact confirmé retourne son reçu original même après des révisions suivantes ; vérifier ce cas avant la transition contre la tête actuelle. Après promotion, `advance` V1 refuse même un ancien replay V1 ; aucune donnée V1 réécrite. Publication exclusive/fsync/pointer/plancher et caps communs, sans doubler le protocole d'écriture. Orphelins conservés, aucune promotion automatique. Une réponse EmitOnce persistée n'est jamais un droit rechargeable ; ce stockage n'autorise ni start, ni terminal, ni libération de quota.

- [x] Capturer un RED minimal sur API absente : compilation ciblée RC101, module checkpoint/advance_supervised/getter absents ; aucun test exécuté et aucune vulnérabilité déduite.
- [x] Implémenter formats fermés, contexte/transition et journal mixte ; traiter BCP-01 à06, avec raccord limité au reducer pur existant et contrôle Owned réinstallé après buffering Serde.
- [x] Vérifier bytes/hash rev0 inchangés, pending/refus d'admission avec managed préparé, vraie chaîne/reopen, CAS/replay historique après V2 ultérieure et plancher via clones ; liens issus du vrai Store SQLite, faits runtime synthétiques explicites.
- [x] Vérifier ancien managed caché dans le préfixe V1, phases, versions/champs inconnus et divergences d'identité/reçus ; six fonctions release PASS et26canaris pertinents, sans matrice complète nouvelle de crash V2 ni moteur.
- [x] Fmt, Clippy strict, release et revue source acceptés ; sources/test115 et artefacts natifs préservés hors target. Benchmarks intermédiaires observés avec limite d'archive du test e0 non reconstruit, qualification finale sur source complète GEL5.
- [x] Fermer la revue documentaire : verdict indépendant CLOSE, aucun constat bloquant résiduel du lot ; préparer son intégration/push après vérifications des manifestes et du delta.
- [ ] Poursuivre IPC privé/ordonnanceur/batch/helpers jusqu'au test réel de mort CLI ; aucune TUI ou portée fournisseur qualifiée par le seul format, aucun test provider/navigateur/E2E général systématique.

### Cycle actif — broker permanent et premier batch réel

Base `7098e204a91182229c1a67bc8017271cb3d20918`, worktree `broker-service` propre après push vérifié. Objectif du cycle : véritable service du même binaire Rust, IPC Unix privé authentifié, Submit idempotent durable, exécution v3 avec tous les helpers et arrêt préarmé après mort du CLI. Le cycle n'est pas clos par ses sous-composants ; aucun cutover du run par défaut avant batch/helpers/TTY qualifiés. Pas d'installation de service OS ou d'action cloud. Le [contrat de supervision](BROKER_SUPERVISION.md) et ses obligations de cycle de vie avant admission restent impératifs.

Suivi mainteneur,31octobre2026 : rappeler le lancement de l'audit **Daybreak Blue** après stabilisation du broker permanent et de ses tests réels d'arrêt/reprise. Le mainteneur préfère encore attendre ce jalon ; aucune campagne globale n'est déclenchée implicitement. Les revues ciblées de chaque lot continuent. Ce rappel est lié au jalon de développement, sans planification horaire créée.

Découpage dans l'architecture déjà approuvée : préparation neuve/reconstruction sans relecture au replay ; transport runtime daemon séparé des signaux/timers CLI ; cycle de vie local durable (annulation/approbation/échecs, y compris préparation sans intention valide) ; writer, enforcer indépendant et IPC/CLI ; enfin raccord batch réel et preuves de panne. Aucun faux Worker, faux receipt ou statut en mémoire seulement ne remplace les faits locaux manquants. Toute extension du journal/checkpoint est explicitement versionnée et compatible avec les formats existants. Les commandes de service ne sont pas publiées comme fonctionnelles avant le raccord réel.

Propriétaires : core possède `src/runner.rs`, nouveau `src/runner/broker.rs` et, après contrat d'exécution, les coutures runner/managed nécessaires. Root possède runtime.rs/runtime/managed.rs, nouveaux modules broker de contrôle/service/IPC/enforcer, main/operations et intégration. Luna possède les nouveaux tests ciblés et toutes les commandes Cargo ; revue indépendante sans modification de code. Aucun chevauchement de fichiers. Le cache Cargo n'appartient qu'à Luna ; gels source avant chaque validation.

#### Première couture — préparation et reconstruction réelles

API fixée dans `runner::broker` : `prepare_broker_submission(options: RunOptions, operation_id: Uuid, control_literals: &[String]) -> Result<PreparedSubmission>`. Objet opaque non Serde avec `intent() -> &ExecutionIntentV1`, `binding_inputs() -> BindingInputs<'_>`, `initial_checkpoint() -> JournalResult<CheckpointPayloadV1>` et `config() -> &Directory`. UUID nil et approve_hash refusés avant IO. Le caller a déjà authentifié/configuré le profil et obtenu ReserveResult::New ; la méthode ne s'authentifie pas elle-même et n'accorde aucun droit. Elle impose l'UUID avant projection, conserve les vrais FD et réutilise les parseurs/choix existants, sans lire une autre clé de contrôle. Wrapper legacy choose_plan inchangé dans ses comportements.

`BrokerRunState::from_job(&AcceptedJobV1) -> JournalResult<Self>` reconstitue purement le RunPlan depuis intention et checkpoint courant, avec getters opération/session/demande/options ; aucun FS, résolution Auto, reset d'approbation ou écriture sessions.json. `credentials_from_job(job, config) -> Result<Option<BrokerCredentials>>` ne lit que le binding durable du job déjà validé et crée le Snapshot minimal existant ; `BrokerCredentials` opaque expose path/names/secrets/validate aux callers de confiance, pas de Serde/Debug de valeurs. Config doit correspondre à l'intention ; aucun accès au chemin source tourné ni aux credentials legacy de secours. Aucun runtime, policy RPC ou modèle dans cette couture.

- [x] Luna capture un RED d'API absente (compilation ciblée RC101,E0432), puis root autorise le code de cette couture. Aucun test exécuté par ce RED préparatoire.
- [x] Core implémente la factorisation étroite, la conservation FD au freeze fournisseur et la reconstruction pure ; gel1 qualifié, pas de Cargo par core.
- [x] Tests réels de fichiers/Journal : UUID imposé, nil/approbation/interactif avant IO refusés, bindingNone/manuel/fournisseur figé, source tournée et références legacy retirées après accept sans relecture, approbation conservée et diagnostics opaques. Cinq fonctions PASS ; handoff bytes/SHA/tamper et durée de vie FD vérifiés, aucune API modèle.

#### Deuxième couture — transport batch préarmé

Root ajoute un port runtime sans Ctrl-C global ni deadline métier reconstruite. `BeforeSpawnGuard: Send` est un garde synchrone consommable `check_before_spawn(self: Box<Self>, target: &str) -> Result<()>`, non Serde/non Clone. Le runtime prépare toute commande/vérification d'autorité avant de le consommer, puis appelle spawn sans await/IO/log intermédiaire. Le raccord futur doit fournir le garde concret relisant l'horloge native et l'état du contrôleur, sans writer, HTTP ou secret fournisseur ; ce garde/enforcer n'est pas encore livré par le port. Ce dernier contrôle n'est pas une atomicité moteur et ne ferme pas D5.

Le callback du nouveau port rend ObserveOnly ou un contrôle préarmé frais avec garde et signal indépendant de drainage du transport. Les captures et le terminal observé sont conservés ; résultat daemon sans booléen timed_out hérité du timer CLI. Expiration observée, stop demandé et terminal sont des faits distincts du contrôleur. Le chemin CLI actuel garde son comportement ; le port batch refuse l'interactif avant création. L'enforcer reste armé après erreur/cancellation/EOF et son retrait exige la preuve terminale/centrale admissible. Le transport est borné après signal indépendant, sans réinitialiser la durée métier.

API du port fixée pour code/tests : `PrearmedStartControl::new(Box<dyn BeforeSpawnGuard>, tokio::sync::watch::Receiver<bool>)` non Clone/non Serde, Debug opaque ; `PrearmedStartDirective::{ObserveOnly, EmitOnce(PrearmedStartControl)}`. Le signal est produit par le contrôleur indépendant, pas par une nouvelle durée locale ; vrai ou canal fermé avant émission interdit spawn. `ManagedRuntime::execute_prearmed_retained(spec, input, stdout_limit, on_created, on_started)` et `start_owned_prearmed_retained(spec, owned, input, stdout_limit, on_ready, on_started)` réutilisent les validations/inspections actuelles ; callback créé/ready retourne la nouvelle directive. Pas d'argument duration métier. `ManagedPrearmedExecution` opaque donne result/created/terminal/handle ; `PrearmedExecutionResult` expose stdout/stderr Capture, transport_exit_code Option<i32>, duration_ms u128, sans timed_out. Le terminal runtime distinct est l'autorité sur le résultat métier. Après un signal indépendant de stop, attente de fin du client bornée10s puis arrêt du seul transport et erreur si le terminal manque ; le port n'invente aucun résultat de timeout et ne libère jamais quota. Les délais techniques existants pour commandes/inspections/pipes restent bornés.

- [x] Figer les signatures du port et le RED avant édition runtime (compilation ciblée RC101,E0432) ; root implémente hors fichiers core. RPT-01 callback/EOF reproduit au gel1 puis corrigé au gel2 en conservant le même futur ; aucune qualification moteur.
- [x] Vérifier refus/dernier garde/ObserveOnly et absence de signal global sur fixtures techniques Rust : quatre fonctions PASS, puis15canaris sélectionnés, fmt/Clippy strict/release PASS et revue logiciel CLOSE. [Preuves et limites](evidence/2026-10-31/broker-submission-prearmed/README.md),118entrées source/artefacts hors target ; échecs du harnais conservés séparément.
- [ ] Qualifier ce transport sur vrai moteur avec le service entier. Aucune fixture ne compte qualification runtime.
- [ ] Définir/implémenter faits locaux durables et canaux privés, contrôle authentifié avant projet/fournisseur/job, writer et cible préarmée hors writer/HTTP ; tests pertinents et revue avant exposition CLI.
- [ ] Raccorder les étapes v3 réelles, observer un worker poursuivant son effet après SIGKILL du client puis arrêté à l'échéance originale, témoin intact, audit/usage idempotents et aucune reprise avec nouveau claim/durée.
- [ ] Compléter panne du broker/writer/HTTP, D5, gardien TTY et recette A1/A2/B1 ; conserver les écarts tant que leurs preuves manquent.

#### Troisième couture — cycle de vie local durable

Base `4890df13f56f2a61d9ee773f9d1dbd7cca929e72`. Le [contrat canonique du journal](BROKER_JOURNAL.md#extension-de-cycle-de-vie--contrat-du-lot-suivant31octobre2026) fixe reject avant intention valide, commandes locales typées et checkpoint V3, approbation sur deux commits après vrai Review, cancel monotone et export postmortem sous nouveau grant, statut/liste bornés et planchers. Les plans1/2/3 et bytes/domaines V1/V2 restent conservés. La V3 réutilise le même journal/CAS, sans fichier parallèle de statut ni nouveau stockage.

Core possède lifecycle.rs/sous-modules, journal.rs/types.rs, coutures checkpoint.rs/validation.rs et export broker/mod.rs ; aucune source runner/runtime/API/audit modifiée. Luna possède broker_lifecycle.rs, journal_lifecycle_floor.rs, fixtures et exécutions ; root contrats/intégration et raccord service indépendant, reviewer lecture seulement. Toujours trois agents d'implémentation au plus et aucun Cargo concurrent.

- [x] Reproduire le défaut existant : après accept confirmé, disparition du seul accepted.json faisait revenir reserve avec Ok(Interrupted) malgré le plancher vivant ; marqueur restauré avant assertion, source/logs RED conservés.
- [x] Capturer l'API manquante par compilation ciblée RC101 avant code ; puis core livre un GEL1 archivé. App compile après corrections test-only et régression du plancher 1/1 GREEN.
- [x] Qualifier rejets/status/list, chaîne V3/replays, vrais Review/approbation, Admission inconnue/cancel et refus d'actions co-nées ; rejets/failures ne sont ni terminal runtime ni libération locale. La transaction Store CancelReserved libère le quota avant son ACK local, pas la commande CancelRequested.
- [x] Traiter les écarts de revue GEL1 : ancien grant Export avant cancel ne permet plus de nouveau start après cancel ; jobs Reserved/Interrupted déjà observés revalidés hors page. RED puis GREEN, frontière historique du dernier cancel dérivée sans wire nouveau. Diagnostic enum public également fermé après RED.
- [x] Revue logicielle et canaris/fmt/Clippy/release au gel final ; [preuves ciblées](evidence/2026-10-31/broker-lifecycle/README.md). Aucun gate global fermé.
- [ ] Raccorder le vrai service et sa cible préarmée. Annulation de Preparing sans CID/absence de terminal admissible reste un écart Store opérateur explicite, pas un faux Cancelled.

#### Raccord HTTP en parallèle — refus d'admission durables

Défaut de raccord constaté dans AuditClient::response : toute réponse non2xx est rejetée avant lecture du DTO, alors que la route v1 de réservation supervisée rend de vrais AdmissionResultV1 avec reçu durable sur403Policy/429Quota. Root possède uniquement src/runner.rs pour la nouvelle méthode typée `AuditClient::admit_supervised(&SupervisedAdmissionV1) -> Future<Result<AdmissionResultV1>>` ; aucune ancienne méthode ni handler n'est remplacé. Luna possède tests/broker_admission_client.rs. Ce client ne persiste rien et ne donne aucun start ; le service doit enregistrer sa requête pending avant émission et publier l'ACK via le journal validé.

Contrat : endpoint déjà existant api/v1/workers/supervised-reservations, credential explicite du même AuditClient sans fallback, client HTTP existant (timeouts15s/connect3s, pas de proxy/redirect). Corps requête borné64KiB et réponse512KiB, parsing DTO fermé/EOF, erreurs opaques ; auth/503/autre réponse sans DTO ne sont jamais un refus métier durable. Statut201 accepté neuf/200 replay accepté ;403 exige refusalPolicy et429refusalQuota, accepted=false,worker=None,reçu sans worker/révision. Vérifier opération/input_hash/audit_seq/hash, cohérence état initial/révision/quota/identités/launch/spec/receipt du Worker accepté. Les validations de scope/contexte/chaîne du journal restent requises avant action.

- [x] RED absent E0599 par compilation ciblée avant code, log conservé ; root produit un premier gel sans Cargo.
- [x] Vérifier sur vrai gateway/Store loopback acceptation/replay et refus Policy/Quota/replay, auth et Worker initial altéré conservés en erreur. BLC-04 reproduit par mutation technique d'un vrai DTO201 puis corrigé ; aucun moteur/modèle ou endpoint inventé. Les bornes sont implémentées et relues, sans campagne exhaustive de mutations HTTP revendiquée.
- [x] Revue indépendante et validation au même gel applicatif que lifecycle, [preuves](evidence/2026-10-31/broker-lifecycle/README.md). L'identité/namespace attendus doivent encore être recoupés par le service avant effet ; le client seul ne fournit ni durabilité ni autorité de start.

#### Plan initial du raccord — base c4c4d2a, 31 octobre, conservé pour historique

État de cette liste au premier RED, avant les raccords et validations décrits dans le point de reprise en tête du document. Les cases ci-dessous ne sont pas le statut courant ; source/IPC/CLI sont maintenant raccordés, les qualifications runtime/TTY/D5 demeurent ouvertes.

Le lot lifecycle est intégré et poussé, sources122 vérifiées après fast-forward. Le [contrat de raccord concret](BROKER_SUPERVISION.md#raccord-en-cours--interfaces-dimplémentation-du-31-octobre) fixe les propriétaires et interfaces du vrai service. Revue de constructibilité du child interne stop-only : direction approuvée conservée, aucun service OS installé ; Ready exige capacité réelle et disarm exige un témoin non ambigu, pas un Ok générique. Les notes privées d'implémentation ne remplacent pas ce contrat.

- [x] Capturer RED comportement du vrai binaire : `broker --help` retourne exit2, sous-commande absente ; aucun service/moteur démarré.
- [x] Capturer RED API locale absente puis premier transport sur vrais sockets Unix ; trois fonctions ciblées passent. La revue demande portabilité du répertoire temporaire et oracles discriminants avant clôture des preuves.
- [ ] Finaliser transport/découverte/leases et thread writer borné, avec tests utiles et revue ; UID seul ne choisit aucun tenant.
- [ ] Raccorder profils de contrôle privés, identité/protocole frais, commandes IPC et CLI réelles, sans paramètre de contrôle libre ni fallback de credential.
- [ ] Raccorder stop child, capacité réservée, dernier garde et reprise aux mêmes cibles/échéances, sans déduire D5 d'un simple retour transport.
- [ ] Raccorder le moteur batch et tous ses helpers, approbation/cancel/export séparés et reprise des pending, puis qualifier la mort CLI sur moteur réel. Aucun cutover par défaut avant batch/helpers/TTY qualifiés.

Luna reste propriétaire des tests/Cargo ciblés ; aucun E2E global, modèle fournisseur ou frontend systématique. Les deux RED sont des preuves de manque actuel, pas une vulnérabilité ou une recette fonctionnelle. G09 reste FAIL et aucun gate global n'est fermé par cette préparation.

### Choix complémentaires du mainteneur — 30 octobre

- [x] Enregistrer les trois moteurs Jev : réel, Rust sans modèle avec limites annoncées, et petit modèle local Jev-like Docker/Podman ; choix/configuration explicites, aucun remplacement du confinement.
- [ ] Définir le contrat commun et les limites de chaque moteur, implémenter le moteur Rust et l'adaptateur local sans réduire les règles impératives, puis qualifier un essai réel borné. Pour le local : au plus4B, cible500Mo–1Go de RAM, modèle/quantification/contexte/limites configurables, mémoire réellement mesurée. Modèle d'embeddings suggéré à évaluer, sélection déléguée à l'orchestrateur.
- [x] Conserver la nouvelle clé Jev privée dans le `.env` ignoré0600, sans altérer les deux autres clés ; source dédiée également privée. Endpoint `https://api.typesafe.ai/v1/systemone` et modèle demandé `jev-latest` ensuite confirmés par le mainteneur. Aucun accès NAS supposé.
- [x] Renseigner uniquement les deux paramètres publics Jev dans le `.env` et l'exemple, sans activation réseau implicite ; vérifier les contrats officiels, garder `choice` et le long contexte désactivé.
- [x] Effectuer un premier essai local réel borné : mémoire/protocole PASS, sémantique FAIL pour Qwen2.5-0.5B Q4_K_M ; candidat refusé pour autoriser des actions. Conserver les deux requêtes et le pic réel, aucun appel répété pour chercher un PASS.
- [x] Vérifier le contrat Jev réel sur un unique petit état synthétique sans worker : réponse `jev-1.14.0`, audit réouvert valide, aucun retry/catalogue/projet. Limites sémantiques explicites ; ne clôture pas la qualification générale Jev.
- [x] Enregistrer la restriction tests : futurs essais Claude explicitement Haiku5.5 (`claude-haiku-5-5`), tests ciblés, aucun retry fournisseur automatique après défaut du harnais.

- [x] Inscrire dans AGENTS.md et les contrats les quatre parcours Codex/Claude Code × abonnement/API, avec résolution dynamique, sélection explicite prioritaire et absence de repli silencieux vers une API facturée.
- [x] Créer le `.env` local autorisé en mode0600, ignoré par Git ; vérifier la liste blanche du contexte Docker et l'exclusion `.env` de l'import. Les fichiers de credentials antérieurs sont préservés. Ce fichier n'est pas encore une preuve de chargement dynamique et ne doit jamais être transmis intégralement.
- [x] Implémenter configuration/résolution Rust, sous-ensemble minimal de credentials, bootstrap et binding aux autorisations avec compatibilité historique ; revue PRV-01/02/03 traitée, vérifications logicielles ciblées et complètes archivées.
- [x] Construire la nouvelle image worker bootstrap et contrôler versions/aides/refus sans credential hors réseau. Ce build ne qualifie pas les authentifications.
- [ ] Compléter les diagnostics de disponibilité avant lancement : status décrit la configuration ; probes clients seulement dans le worker après admission. Qualifier ses vrais lancements et réglages adversariaux.
- [x] Exercer un premier parcours géré Claude/API batch réel, modèle demandé `claude-haiku-5-5`, fichier/export exacts, audit après restart et quota libéré ; preuve limitée à ce binaire/image/compte. Préflights échoués sans modèle conservés.
- [ ] Qualifier les quatre parcours gérés complets, dont conversations interactives. Codex et Claude API batch manuels gardent leurs gels anciens ; le nouveau PASS Claude/API géré ne qualifie ni Codex géré, ni abonnements, ni TUI, ni paramètres adverses. Les abonnements requièrent une connexion officielle autorisée, sans extraction implicite du cache hôte.
- [x] Inscrire le dashboard cloud unique par client sur l'ensemble de ses brokers/hôtes/runtimes, sans dashboard par worker.
- [ ] Livrer cette agrégation avec identité distante, fraîcheur/états partiels, quotas et comptage globaux du tenant ; recette A/B et mesure100workers. Le mode partagé actuel ne constitue pas cette preuve.

- [x] Inventorier les fichiers et l'état Git ; constater les contradictions documentation/code.
- [x] Répartir audits indépendants Rust core, runtime/sécurité et frontend.
- [x] Installer la toolchain exacte manquante et lancer les vérifications de référence.
- [x] Achever la référence et conserver les défauts reproductibles.
- [x] Implémenter le premier lot P0 : identité authentifiée, base SQLite liée au tenant et lectures/écritures API isolées, avec compatibilité locale explicite.
- [x] Tester A/B, spoof d'identité, credentials ambigus et réouverture durable ; aucun worker n'obtient de rôle de contrôle.
- [x] Traiter les trois P0 reproduits par revue : schéma/trigger qui efface l'audit, corruption externe avant append, hardlinks multiprocessus/WAL divergents. Conserver les régressions et vérifier absence de mutation d'un schéma incompatible.
- [x] Faire relire indépendamment les changements, traiter les défauts bloquants, puis format/Clippy/tests/build et preuves courantes : 109 PASS, 1 smoke ignoré, re-probes HTTP et deux processus PASS.
- [x] Aligner la gouvernance persistante sur un vrai produit open source complet et soigné, sans réduction en MVP/V2 ; préserver les preuves historiques. Vingt documents relus, 93 liens/234 chemins vérifiés, copies synchronisées.

## Fonctionnalités demandées — identité, SSO et découverte, 31 octobre

Le [contrat canonique](../sso.md) fixe intégralement les exigences ; ce tableau organise leur livraison et leurs critères de preuve. **Tous ces comportements restent à implémenter et qualifier.** Le lot documentaire ne livre aucun d'entre eux et ne remplace pas le chantier de supervision en cours. Le SSO est facultatif à l'installation, mais son support fait partie du produit ; les exemples de parcours ne limitent pas les axes de configuration.

| Identifiant | Fonctionnalité à livrer | Critère d'acceptation |
|---|---|---|
| CFG-01 | Configuration par axes indépendants et profils de parcours non exhaustifs | Topologie, identité, périmètres, client/authentification IA et réseau combinables selon le [contrat](../sso.md#configuration-par-axes-indépendants) ; configuration typée, priorités/provenance visibles, contraintes administratives appliquées et incompatibilités refusées sans repli ; interactions critiques qualifiées sans assimiler une combinaison non testée à une garantie |
| INF-01 | Images construites/distribuées par le client et inférence interne | Dockerfiles et procédures sans image publique obligatoire ; URL, modèle, format compatible OpenAI précis/adaptation Claude, référence de credential et chaîne publique d'AC préparés dans le worker et liés à l'autorisation ; TLS actif, destinations internes approuvées, aucun secret de contrôle ou clé privée d'AC, aucun repli cloud ; vrais parcours sans Internet avec clients épinglés |
| IDN-01 | Utilisateurs individuels en local et via fournisseur OIDC configurable | Identité locale ou `(iss, sub)` vérifiée, appartenances tenant et rôles côté Rust ; compatibilité des anciennes clés sans réattribution fictive |
| SSO-01 | Clients publics Web et CLI, Authorization Code + PKCE S256 | Aucun secret client distribué ; connexion réelle, expiration/renouvellement, déconnexion/révocation, refus d'issuer/audience/signature/nonce et replay |
| SSO-02 | Administration applicative avec mapping de rôles/claims configurable | Rôle externe `admin-ai-guard` pour la découverte, noms et chemins configurables ailleurs ; claims du jeton d'accès validés côté Rust, mapping et périmètre explicites ; vue d'administration autorisée, refus utilisateurs normaux/autres tenants/claims falsifiés ou retirés, aucune promotion en administrateur du realm ou `runner` |
| UI-01 | Vues utilisateur et administrateur pilotées par les permissions effectives | Contrat typé Rust/frontend, navigation et actions adaptées aux droits/configuration, contexte personnel distinct, contrôles serveur sur lecture/contenu/export/mutation ; invalidation après révocation/changement de périmètre, refus aussi par URL directe et SSE |
| UI-02 | Synthèse administrateur unifiée à 360° | [Vue globale](../ui.md#vues-utilisateur-et-administration-unifiée--cible-à-livrer) des ressources, utilisateurs, incidents, décisions, quotas et configurations autorisés sur tous les brokers concernés ; filtres et causalité vers les détails, sources/fraîcheur/couverture partielle explicites, aucun accès inter-client implicite ni faux total de pagination ; parcours réel qualifié avec thèmes et mobile |
| TRC-01 | Identités d'exécution et délégations authentifiées | Parent, initiateur, acteur, mission et droits liés durablement ; pas d'élévation, d'usurpation ou de jeton SSO humain dans un worker |
| TRC-02 | Chaîne demandes/réponses visibles → outils/commandes → décisions → effets | Causalité conservée sous concurrence/reprise, messages masqués/bornés, couverture partielle explicite, audit obligatoire avant effet |
| TRC-03 | Recherche et export de preuves par utilisateur/session/agent/commande | Navigation des parents et résultats selon les droits ; audit indépendant, chaîné et points de contrôle signés, vérification et limites documentées |
| DEM-01 | Premier essai Compose avec une clé API du fournisseur choisi | Prérequis diagnostiqués, interface disponible et première exécution réelle auditée, sans autre clé fournisseur/SSO obligatoire ; temps et étapes manuelles mesurés |
| DEM-02 | Profil Keycloak de découverte prêt à lancer | Version épinglée, realm/clients/PKCE préconfigurés ; quatre comptes : `admin/admin` pour le realm, `admin-ai-guard/admin-ai-guard` avec le rôle `admin-ai-guard`, `user-1/user-1` et `user-2/user-2` sans ce rôle ; loopback et activation explicite, aucune donnée existante remplacée |
| DOC-01 | Documentation découverte et raccordement entreprise | Quick start, configuration, Compose/realm, commandes CLI, diagnostic, arrêt/reprise et migration cohérents avec les comportements réellement testés |

- [ ] Définir les contrats typés, migrations et règles de rattachement/revocation sans changer les frontières de confiance existantes.
- [ ] Implémenter les parcours individuels, le connecteur OIDC et les délégations/audits avec les refus et états incomplets.
- [ ] Livrer le packaging et les configurations de découverte ; aucune commande future n'est présentée comme exécutable avant validation.
- [ ] Exécuter la [matrice ciblée](TEST_STRATEGY.md#identité-individuelle-sso-et-découverte--à-qualifier) avec vrai Keycloak, vrais Web/CLI et trace d'exécution, puis mesurer la cible d'essai rapide. Aucune qualification globale par fixture ou par simple démarrage du fournisseur.
- [ ] Mettre à jour documentation et preuves au même gel, faire relire les changements de sécurité et traiter les écarts des gates G04/G06/G07/G08/G09/G12.

## Ordre de dépendance des lots

Cette séquence décrit les dépendances du chantier global. Les sous-lots déjà vérifiés ci-dessus ne sont pas à refaire systématiquement ; compléter leurs exigences et preuves encore ouvertes, ainsi que les fonctionnalités CFG/INF/IDN/SSO/UI/TRC/DEM/DOC, avant la revue de release.

1. Corriger les autres P0/P1 reproduits par les audits (audit, autorisations, import/export, lancement).
2. Compléter le registre durable, les quotas/réservations et transitions atomiques avec audit ; intégration broker, supervision et réconciliation qualifiées.
3. Relais Rust authentifié avec grants dirigés/révocation, limites/durée/rétention, ledger d'allocation `billable=false` idempotent.
4. Recette réelle trois workers, reprises/pannes et visibilité dashboard ; qualification séparée Docker/Podman par plateforme.
5. Versionnement API et schéma, migrations admises, sauvegarde/restauration, exploitation et chaîne de livraison.
6. Adaptateur Kubernetes et identité/réseau/stockage cloud, charge 100 workers réellement mesurée et intégrations fournisseurs natives selon leurs canaux démontrés.
7. Revue de release G01–G12, licence/canal privé décidés, matrice complète et aucun blocage obligatoire ouvert.

Chaque lot précise avant édition : fichiers propriétaires, besoin, risques, tests et critère d'acceptation. Des dépendances externes manquantes bloquent leurs preuves, sans interrompre les travaux indépendants.

## Lot P1 : décisions durables et idempotentes — clos au périmètre logiciel

Défaut initial confirmé par inspection : chaque POST decide créait un nouvel événement pour le même request_id ; le contenu n'était pas lié durablement à cette identité. Ce comportement a été reproduit puis corrigé.

Périmètre traité : `src/audit.rs`, `src/api.rs`, types de décision directement concernés et `tests/decision_idempotence.rs`. Aucun changement de moteur de stockage ni assouplissement des trois correctifs P0. Les chemins append et décision partagent les mêmes contrôles de schéma, chaîne, quota, inode et changement externe.

Critères :

- Même identité de demande, même contenu et même phase initiale/approbation : même décision et même audit_seq, sans nouvel événement, y compris concurrence et redémarrage.
- Même identité avec contenu différent : conflit explicite 409, aucune nouvelle autorisation.
- La phase approuvée exige une revue initiale durable et le hash exact ; un hash faux ne peut pas récupérer une autorisation en cache. Un refus initial ne peut être promu.
- Les événements et l'identité d'idempotence sont engagés dans la même transaction IMMEDIATE ; une panne d'audit ou une altération bloque aussi une reprise.
- Une réponse répétée de décision ne constitue pas une nouvelle preuve de démarrage ni un ticket de lancement consommé. La réservation/cycle de vie des workers et la consommation d'exécution durable restent le lot obligatoire suivant.

Tests : répétition, divergence, concurrence de deux connexions, redémarrage, progression review→approved, approbation sans revue/hash faux, refus non promu et panne/corruption d'audit. Revue indépendante et vérifications globales après intégration.

Statut de ce lot décisions : **terminé au périmètre logiciel**, neuf régressions HTTP et cinq probes indépendantes PASS, fmt/Clippy/tests/build intégrés. Le relevé global après ajout des trois helpers de confinement est 121 PASS/2 ignorés, avec une exécution réelle distincte du test de confinement PASS. Aucun ticket d'exécution consommé n'est livré par ce lot.

## Lot P1 : publication locale par relais Rust — clos pour le candidat local

Défaut reproduit : services Docker sains exclusivement sur `internal`, demandes de ports présentes dans HostConfig mais NetworkSettings.Ports vide et accès hôte impossible. Le mainteneur a **autorisé** le relais Rust distinct sans secrets/volumes, afin de conserver gateway/dashboard internes (décision motivée dans DECISIONS.md).

- [x] Implémenter destination fixe, streaming/auth/statuts, filtres hop-by-hop, limites de corps/URI/flux et délais, avec tests HTTP.
- [x] Ajouter CLI deux listeners et arrêt SIGTERM borné avec un client SSE actif ; intégrer les deux Compose sans toucher les réseaux workers.
- [x] Préparer le parcours navigateur réel avec garde réseau testé et sans captures de secrets ; conserver le BLOCKED initial avant publication effective.
- [x] Clôturer les constats de revue : deadline totale upload, progression aval indépendante du polling Body, garde normale jusqu'à EOF/drop aval ; sonde santé des deux destinations.
- [x] Revue finale, fmt/Clippy/suite/build ; candidat contrôle `qa-20261029-8bd72c0e`, ID `sha256:70b34787cc12aa59e6558c1e391329bffa0acc0c6bc83363737a46589a02414e`. Suite intégrée finale : 139 PASS/0 FAIL/2 ignorés ; les deux essais réels sont exécutés séparément.
- [x] Déployer uniquement la pile QA isolée et observer les bindings `127.0.0.1:48787`/`127.0.0.1:48400` ; conserver les observations réseau et leurs limites de preuve.
- [x] Exécuter le smoke réel (1 PASS) et le navigateur (9 contrôles PASS) sur cette pile ; corriger et relire les défauts du harnais, conserver les FAIL antérieurs, audit/volumes/preuves.
- [x] Redémarrer le seul gateway sans job actif ; audit avant/après identique (102 événements/même head), retour healthy puis `/healthz` final PASS sans second redémarrage. Aucune reprise de job/crash/daemon déduite.
- [x] Arrêter le seul dashboard QA : gateway encore 200, sonde combinée en échec ; redémarrer le dashboard et vérifier endpoints/sonde combinée en succès, pile rétablie sans suppression de données.

Preuves : [dossier final](evidence/2026-10-29/local-final-8bd72c0e/README.md) et [validation](VALIDATION.md). Le PASS du confinement est une exécution réelle distincte du smoke ; les deux restent ignorés dans la suite globale.

## Lot P1 : terminal runtime observé — clos au périmètre logiciel

- [x] Reproduire le résultat terminal inventé (ExitCode absent donnant zéro, état encore actif accepté), sans casser les commandes rapides déjà terminées.
- [x] Exiger Running/ExitCode, code non négatif, état terminal adapté au RuntimeKind et dates cohérentes ; conserver les codes métier non nuls.
- [x] Traiter les deux P1 de revue : Podman `stopped` valide et sentinelle ExitCode négative ; RED puis GREEN et probes indépendantes conservés.
- [x] Exécuter un test agrégateur de 36 sous-cas, 20 tests ciblés, Clippy/rustfmt et revue finale ; intégrer la suite globale 139 PASS.

Le harnais utilise des CLI Rust fixtures pour les cas Docker/Podman ; il ne qualifie pas un moteur Podman réel. Au gel logiciel 139, le cleanup n'avait pas encore été déplacé : la conservation jusqu'à l'audit terminal fait l'objet du lot suivant ; la reprise reste à traiter.

## Prochain chantier vertical obligatoire

### P1 autorité terminale/ingestion — clos au périmètre logiciel et Docker QA nommé

Avant le registre, les correctifs reproduits sont traités en périmètres exclusifs. Le [dossier de preuve](evidence/2026-10-29/p1-terminal-authority/README.md) distingue l'intermédiaire 145/3 ignorés et deux essais réels antérieurs, puis le gel logiciel **147 PASS/0 FAIL/3 ignorés**, fmt/Clippy/release, revue finale et rejeu réel ciblé **1 PASS en 1,25 s**. Les résultats intermédiaires ne qualifient pas le code corrigé ensuite ; le nouveau déploiement reste distinct.

- [x] API : refuser les nouvelles ingestions réservées aux décisions/contrôle/workers/allocations centrales ; conserver la compatibilité des traces libres. Revue ciblée et 21 tests pertinents corroborés par la suite intermédiaire. L'identité runner du même tenant était nécessaire au défaut ; aucun assainissement des décisions historiques ni droit de lancement rétroactif.
- [x] Runtime/runner : reproduire le cleanup avant ACK terminal, la perte de résultat après cleanup/audit d'erreur échoués et la confusion terminal inconnu ; conserver les RED/GREEN de leurs harnais Rust/HTTP.
- [x] Effectuer deux essais Docker indépendants avec gateway/SQLite réels et refus HTTP terminal par middleware ; le second vérifie aussi la réouverture DB. Résultats limités au code antérieur aux dernières corrections, aucun test de disque physiquement défaillant.
- [x] Reproduire les six cas de référence create acquittée inutilisable, où le fallback par nom pouvait agir sur un successeur non possédé.
- [x] Geler les corrections sans fallback par nom en ownership inconnu ; quatorze sous-cas dont les six cas RED corrigés, 22 tests ciblés et source gelée conservés.
- [x] Intégrer `error_audit=not_applicable|acknowledged|unacknowledged`, cleanup `UNCONFIRMED`, arrêt des helpers sans ACK et `changed_files` inconnu ; préserver le terminal connu, et séparer `postprocessing_error` du résultat métier.
- [x] Reproduire puis corriger les fuites de secrets connus dans chaîne d'erreur CLI, frontières stdout/stderr et projection JSON ; masquer après décodage JSON avant projection, y compris les clés. Conserver les RED sans valeur canary dans les journaux.
- [x] Recevoir le gel de l'orchestrateur et consigner fmt/Clippy/suite/release : 147 PASS/0 FAIL/3 ignorés, binaire et sources identifiés par SHA-256.
- [x] Consigner le rejeu réel du code final : HTTP terminal 503 par middleware, instance arrêtée conservée, audit valide avant/après réouverture SQLite ; pas une panne physique de disque.
- [x] Faire relire le lot de code gelé et ses preuves finales, traiter les P0/P1 et archiver le verdict ciblé PASS sans remplacer les snapshots antérieurs. Le reviewer ne revendique pas d'exécution personnelle des probes ou moteurs.
- [x] Identifier la nouvelle image `qa-20261029-security-09c7b861`, ID `sha256:b4239720250bd8a0e05c478d5bd289726041ea753dd701fa4780e795ae9150f2` ; trois services QA healthy, smoke 1 PASS en 3,62 s et navigateur 9 contrôles PASS sur ce candidat. Aucun transfert de l'ancienne image ou du service SQLite local de test.
- [x] Conserver audit avant/après déploiement identique (102 événements), puis audit après nouvelles écritures de tests valide (137 événements) ; volumes et preuves conservés. Clôture limitée à Docker Desktop rootful/macOS ARM64, sans recette trois workers ni qualification production.
- [x] Rejouer aussi le confinement du code final : 1 PASS en 3,78 s. Les trois essais réels courants restent séparés des trois ignorés de la suite normale ; conserver et vérifier le manifest complet de 78 fichiers sans promettre la reproductibilité des paquets/images.

Ce relais de publication n'est pas le relais métier inter-workers. Registre, quotas/réservations durables, identité runtime stable/fencing, état inconnu et ledger d'allocation non facturable restent le chantier vertical obligatoire suivant.

### Conceptions pour le prochain lot — non implémentées

Les [archives DESIGN](evidence/2026-10-29/p1-terminal-authority/design-next-not-implemented/README.md) conservent la synthèse core, son détail et le contrat runtime préparatoire. Elles ne livrent aucun nouvel endpoint, table, quota ou ledger ; leurs tests proposés restent NOT_RUN.

Concepts à transformer en lots avec critères propres : admission fraîche liant spécification complète et reçu de décision, sans faire confiance à un ancien allow ; état/quotas/audit dans la même transaction et projections vérifiées ; tentative/autorité/identité runtime stables et start consommé non rejouable ; observations inconnues conservatrices, terminal/allocation/libération atomiques `billable=false` et reprise sans second effet. Migration explicite, restauration sûre et tests de concurrence/pannes réels restent à implémenter et qualifier. Une fondation SQL ou un mock ne suffit pas à annoncer un registre utilisable.

### Cycle actif : registre et parcours du vrai broker

- [x] Implémenter contrats et projections SQLite v2, quotas/CAS, claim irréversible, mesure terminale et migration sauvegardée ; tests de stockage et relecture ciblée.
- [x] Reproduire puis fermer le contournement de schéma par triggers réservés, avec preuves quota et double claim.
- [x] Ajouter les premières API versionnées et leurs tests HTTP A/B/rôles/quota/pagination ; configuration des limites issue du contrôle.
- [x] Ajouter l'adaptateur à autorité liée, image immuable, inspections et callbacks avant start ; tests sur fixtures, aucun PASS moteur transféré.
- [x] Ajouter les vues Workers/Quotas/Allocations ; tests Node/build/navigateur sur fixtures et relecture du correctif de focus.
- [x] Intégrer `run`, checkpoints privés et verrou par tentative, reprises/arrêts/export sur le même worker et terminal durable avant cleanup ; parcours réels ciblés et reprises sur fixtures, limites ci-dessous.
- [x] Exécuter format/Clippy/suite intégrée après adaptation des tests au genesis v2 :209 PASS/0 FAIL/3 ignorés, release PASS ; corriger les défauts de revue.
- [ ] Faire relire les frontières d'intégration et rejouer le nouveau parcours sur Docker réel, y compris panne et réponse perdue, sans double start ni mesure.
- [x] Construire/déployer le candidat QA209, vérifier migration sauvegardée137→138, audit195 valide et dashboard réel fixe/partagé ; préserver image147, images intermédiaires, volumes et preuves.

Le garde-durée après crash du broker et la restauration sûre d'un état de claims restent des obligations explicites. Un état incertain retient son quota jusqu'à preuve suffisante ; il ne clôture pas à lui seul un parcours opérateur de reprise. Les étapes cochées concernent leur portée logicielle nommée, pas la recette A1/A2/B1, Podman/Kubernetes, charge 100 ou production.

### Priorités révisées après réception de l'audit externe

Les 19 candidats de l'instantané indépendant restent formellement à valider. [AUDIT_FOLLOWUP.md](AUDIT_FOLLOWUP.md) porte le rapprochement avec le checkout courant et les preuves applicables. Traiter d'abord les frontières potentiellement P0 : programme moteur choisi via PATH, ancrage configuration/credentials/SQLite et secrets. La supervision durable Rust du broker reste le prochain chantier P1, avec son design préparé ; aucun daemon ni mécanisme de durée autonome n'est encore livré. La qualification complète des reprises réelles et la recette à trois workers restent ouvertes, sans déplacement sous une étiquette de version future.

- [x] Rapprocher les19 candidats du checkout et conserver leurs verdicts originaux sans réécriture.
- [x] Corriger et faire relire collision Jev/contrôle et captures littérales tronquées, y compris bibliothèque, erreurs CLI et marqueurs privés ; gel235, essais Docker ciblés et nouvelle image QA nommée.
- [x] Supprimer la recherche PATH ambiante sur les parcours livrés et lier l'invocation aux reprises : politique2 ancrée par FD, migrations explicites auditables, gel261 et essais Docker Desktop/macOS ARM64 nommés. Revue et limites dans le [dossier de preuve](evidence/2026-10-29/trusted-invocation/README.md).
- [ ] Qualifier séparément les autres couples moteur/plateforme et compléter la liaison opérateur Podman ; traiter les limites d'installation/plugins/fencing sans transférer le PASS Docker.
- [x] Ancrer configuration, checkpoints broker et handoff de credentials, avec owner/modes/ACL, snapshots indépendants de TMPDIR, exclusion physique du contrôle de l'import et compatibilité des claims ; [gel302](evidence/2026-10-30/control-filesystem/README.md), revue et preuves ciblées.
- [x] Appliquer la frontière privée à SQLite/WAL/SHM/backup, supprimer le FD brut qui annulait des locks POSIX, contrôler noms/permissions/ACL et relire la migration sauvegardée ; [gel327 et QA avec591événements conservés](evidence/2026-10-30/sqlite-storage/README.md), Linux dev et deux UID réels compris. Le gel302 antérieur n'incluait pas ce sous-lot.
- [ ] Compléter la qualification de panne et restauration sûre des claims, les autres filesystems/plateformes et scénarios adverses restants ; le PASS du lot SQLite privé ne clôture pas G06.
- [ ] Poursuivre la supervision autonome, le relais et les qualifications obligatoires ; les étapes logicielles cochées ne remplacent pas leurs preuves.

### Lots locaux livrés et parcours CLI suivant — 30 octobre

- [x] Désactiver le journal brut des nouvelles créations moteur, inspecter avant claim/start et conserver les hashes/reprises historiques : [presetv2](evidence/2026-10-30/engine-logs-v2/README.md), tests ciblés Luna, Docker ACK503/capture masquée et nouveau run v2 PASS, revue indépendante traitée. Pas de qualification Podman ou disque déduite.
- [x] Exécuter un vrai Codex0.165.0 avec modèle demandé Luna : refus strict, premier FAIL bwrap explicite, seconde session avec fichier réel/export exact et original intact. Préparer l'image Claude2.1.140 sans transporter l'OAuth hôte ; [preuves et limites](evidence/2026-10-30/provider-cli/README.md).
- [x] Livrer [quick start](../quickstart.md), prérequis, commandes et configuration, relus contre le CLI/code ; pas de promesse TUI dans les recettes batch.
- [x] Livrer le transport générique `run --interactive` : mode lié à décision/admission, preset versionné, filtre/masquage streaming et gardes, JSON/hashes batch conservés. Régressions ciblées, terminal12/12, auth8/8, fmt/Clippy/release et revue traitée ; vrais Docker clavier/resize/exit7 puis doubleCtrlX PASS. [Preuves et FAIL conservés](evidence/2026-10-30/interactive-cli/README.md). Ce lot ne qualifie pas encore les clients fournisseurs ni une release.
- [ ] Qualifier Codex TUI avec une vraie authentification, deux tours modestes et effet/export réels. Claude exige un handoff autorisé dans le worker ; méthode demandée au mainteneur, ne pas extraire le cache ou le keychain hôte. Les versions/aides seules ne clôturent pas ce parcours.
- [ ] Compléter reprise attach sans second start, crash/enforcement autonome, puis les autres exigences ouvertes. Un terminal interactif ne fournit ni supervision autonome après SIGKILL ni autorisation indépendante de chaque commande transitive.

Le mainteneur reporte Daybreak après la nuit de travail ; aucune campagne globale ni planification automatique cette nuit. Les revues ciblées nécessaires aux lots restent exécutées.

### Direction UX retenue pour les chantiers visuels

- [x] Inscrire la référence Nothin’ demandée par le mainteneur dans AGENTS.md et [docs/ui.md](../ui.md), après consultation de sa page publique.
- [x] Décliner le guide détaillé du30oct dans les neuf vues : header clair, navigation sombre fixe, numéros, typographie éditoriale, citron réservé aux actions/sélection et login accessible dès320px. Node27/build/navigateur,45couples de vues/tailles et revue indépendante PASS ; image QA déployée,10contrôles navigateur réels PASS. Les [captures réelles et limites](evidence/2026-10-30/editorial-ui/README.md) distinguent cette réalisation de l'ancienne mention documentaire.
- [x] Adapter les deux visuels OVPT fournis au logo et favicon16/32, conserver ai-guard comme produit et reprendre le sombre en noirs neutres. Corriger aussi Suivant disabled devenu olive.27tests Node24/build/navigateur/revue puis dix contrôles réels PASS ; [lot OVPT livré et captures](evidence/2026-10-30/ovpt-ui/README.md). Backend ancien explicitement conservé pendant la qualification SQLite.
