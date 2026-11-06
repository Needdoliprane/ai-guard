# Suivi de l'audit indépendant — 29 octobre 2026

<a id="audit-final-run-1"></a>

## Rapport final run-1 et comparaison au code du 6 novembre 2026

**Ce registre est une entrée obligatoire des prochaines itérations.** Lire le
rapport primaire et les consignes du lot avant de modifier le code ; confronter
chaque constat à la source courante et reproduire son déclencheur. Les preuves
historiques ci-dessous conservent leur périmètre et ne clôturent pas les défauts
résiduels du rapport final. Aucun verdict de qualification production n'est acquis.

Les pièces primaires restent privées dans
`~/security-audit-skill/ai-guard/run-1/` : `REPORT.md` (notamment section 10),
`PROMPT-CORRECTIFS-SUITE.md`, `FINDINGS-DETAIL.md`, `NEEDS-VALIDATION.md`,
`findings.json` et `comparison-5e52807.json`. Ce registre en conserve la provenance
et les suites ; il ne publie ni leurs reproductions ni leurs traces intégrales.
Si ces pièces ne sont pas accessibles, signaler cette limite avant de prétendre
avoir vérifié un constat. Ne pas reconstruire une conclusion depuis le seul
résumé du chat.

### Provenance et seuil de preuve

L'audit porte sur `main@c4c4d2a`, avec une extension limitée au diff de
`codex/broker-service@d6cb136`. La comparaison
ultérieure porte sur `codex/human-brokers-validation-20261104@5e52807`, 135 commits
après la base de correctifs `0a41aac`. **Comparé ne signifie pas audité** : le
nouveau code, notamment workloads, relais, Kubernetes et registre des brokers,
demande encore une revue propre. Les instantanés et les pièces externes ne sont
pas modifiés par les correctifs du produit.

Empreintes SHA-256 des pièces reçues le 6 novembre :

| Pièce | SHA-256 |
|---|---|
| `REPORT.md` | `b3fd166102292135e426b007ef217bf41d6574ac4dfaabcbdc2ba2505c117296` |
| `PROMPT-CORRECTIFS-SUITE.md` | `77bd0538b71e89536620ccfb65d7a2c923e701378a67d9ea3e984970b80ea0f9` |
| `FINDINGS-DETAIL.md` | `5ccc8a8e66b8c7e5c6230085bb38218dae0479f0f319d9d3b455bafe7ea61e3d` |
| `NEEDS-VALIDATION.md` | `eb6609e6dc362b5eae87344e54360a6d7acba233e9a89ef78f29251a4ce3a695` |
| `comparison-5e52807.json` | `a8fc63d69aa87dc582e2d0149bdef13f102dedf5a0af5eaa52d8372b99f4f41a` |
| `findings.json` | `70699c25107b7d00bb8e7393c9442936bafb19025f697c72b0d0a3c04861d888` |

Le rapport démontre 13 vulnérabilités sur les instantanés audités : une haute,
dix moyennes et deux faibles ; il distingue deux risques à vérifier, seize
exigences absentes à cette date et un candidat rejeté. Il n'établit pas de fuite
inter-tenant, de contournement d'authentification ou d'exécution HTTP non autorisée
dans ce périmètre. Cette absence de constat ne garantit pas le code ajouté depuis.

Sur `5e52807`, le déclencheur d'annulation du broker est corrigé ; sept correctifs
restent partiels (export, collision, lectures synchrones, moniteur, descripteurs,
capacité des lectures humaines et ledger OIDC), cinq défauts restent présents
(trois pièges de quota, perte d'audit des fichiers et limiteur d'écritures partagé).
Les seize exigences initiales ne sont pas seize absences inchangées : la
comparaison distingue cinq absences, le contrôle réseau encore au stade de
conception et dix implémentations partielles. Restent notamment absents
l'approbation humaine authentifiée, la signature des enregistrements broker et
des checkpoints, l'attribution des délégations et la sortie auditée des réservations dont
l'absence de démarrage est prouvée.

### Lots de correction et acceptation

Les lots G à M de `PROMPT-CORRECTIFS-SUITE.md` remplacent les anciennes listes
incomplètes comme point de départ, sans invalider leurs preuves datées. Chaque
lot exige reproduction avant correction, régression conservée, tests ciblés par
Luna, revue indépendante, documentation des contrats EN/FR concernés, puis
intégration et push. Aucun succès de test ne change seul le verdict externe.

| Lot | Problème et preuve attendue | État au démarrage du suivi final |
|---|---|---|
| G | Trois réservations sans issue : rejeu idempotent des mutations et claims, absence de double démarrage, sortie auditée seulement si jamais démarré est prouvé. | `OPEN` ; dépendances avec le chantier D5 à coordonner, sans écriture concurrente dans les mêmes fichiers. |
| H | Ingress : les pools anonymes résiduels ne doivent plus priver les opérations de contrôle de leur capacité. | `OPEN` ; isoler la capacité de contrôle sans supposer qu'une adresse de pair est une identité. |
| I | Sortie déclarée refusée ou saturée : scan/diff fiable conservé, couverture partielle explicite, reprise et budget de contrôle indépendants. | `SOURCE_READY`, worktree `codex/audit-i-file-audit-20261106` depuis `5e52807` ; reproductions préparées, exécution `NOT_RUN`, aucun correctif applicatif encore accepté. |
| J | Export : dépôt nu, métadonnées exécutables et alias de chemins écartés, modes publiés sûrs, omissions visibles et détection des changements de mode conservée. | `SOURCE_READY`, worktree `codex/audit-j-export-20261106` depuis `5e52807` ; régressions préparées, exécution `NOT_RUN`, aucun correctif applicatif encore accepté. |
| K | Un store voisin ne doit pas bloquer l'autorisation ni les lectures d'un autre tenant ; budgets et lectures coûteuses bornés. | `OPEN` ; les scans inter-stores et listes de reviews doivent être revérifiés après composition, pas seulement les handlers déplacés hors Tokio. |
| L | Pénurie de descripteurs : admission HTTP bornée, récupération sans trafic authentifié et comportement du moniteur explicite. | `PARTIAL` ; correction Store publiée, disponibilité TCP et moniteur non qualifiés. Fenêtre de réessai soumise à décision distincte. |
| M | Capacité d'audit : lecteurs, preuves SSO répétées et écritures ordinaires ne doivent pas épuiser la réserve indispensable aux actions et révocations. | `OPEN` ; budgets durables et déduplication à reprendre, sans effacement de ledger pour obtenir un test vert. |

Le correctif Store `6871634` est composé dans `987898e`. Les deux régressions API
de `6fa1bcc` sont déjà composées dans `4494760`, ancêtre de `5e52807` : ne pas les
réintégrer une seconde fois. Leur fichier `api_cases.rs` conserve l'empreinte
`ef33d945d2176995b1d797550de41ad163eaf87a4703fe4e3feb17580d512175`.
Ces dix tests Store et deux tests API ne prouvent pas la disponibilité HTTP ni
l'absence d'arrêt d'un worker sous pression FD.

### Décision et qualifications encore ouvertes

La [proposition de fenêtre transitoire du moniteur](GATEWAY_TRANSIENT_FAILURE_PROPOSAL.md)
reste **PROPOSED** : aucune autorisation explicite du mainteneur n'est enregistrée
pour changer le comportement documenté. Les autres lots avancent indépendamment.
Ne pas étendre cette fenêtre aux lancements, claims ou audits obligatoires.

`NOT_RUN` dans l'audit : moteurs réels Docker/Podman, confinement, réseaux,
`--env-file`, disque plein et matrices Linux/macOS. Les deux risques dépendant du
déploiement ou du CLI restent à valider : volume worker face au stockage de
contrôle et amplification de capture face à l'outbox. Le test checkpoint dépassant
520 s sur machine calme reste à diagnostiquer ; le test outbox précédemment lent
a passé en 8,1 s sur machine calme, sans qualifier les autres scénarios.

À chaque intégration, compléter ici le lot, ses sources exactes, commandes,
résultats `PASS`/`FAIL`/`BLOCKED`/`NOT_RUN`, revue, limites et worktree fermé ou
consommateur restant. Préserver la distinction entre défaut reproduit, correction
logicielle et qualification runtime. G09 reste `FAIL` ; aucun gate global clos.

## Pénurie FD du Store — 6 novembre 2026

Le latch est reproduit sur `0843c41` avec EMFILE réel limité à un enfant : après
restitution des FDs, chaîne valide et origine fermée. `6871634`, composé dans
`987898e`, conserve le refus courant puis réconcilie automatiquement la suspension
typée, sans réarmer les invalidations permanentes. Le témoin du commit précède
la dernière vérification des noms ; rejeu/débit/activation ont leurs régressions
avec connexion sœur. Dix fonctions finales, compilation et revues passent dans
leur [périmètre documenté](evidence/2026-11-06/store-resource-recovery/README.md).

Les FAIL intermédiaires sont conservés. Ce résultat logiciel ne prouve pas une
exposition TCP ni sa disponibilité sous pression. Les verdicts/fichiers externes
restent inchangés ; les attentes Daybreak ci-dessous sont historiques. Aucun
nouvel audit global ou gate production clôturé.

## Confirmation de quarantaine — 6 novembre 2026

Le lot `7b630ff` apporte une confirmation metadata-only sur l'input exact du
même acteur Stops, sans republication ni réarmement. L'API était absente sur
la base : ses tests de contrat écrits d'abord ne constituent pas un RED
comportemental avant compilation. La revue du premier raccord découvre ensuite
un retour trop large qui prive les profils suivants de progression.

Ce défaut distinct est reproduit avec contrôles valides, A encore retenu et B
sans ses deux événements, après tous les nettoyages. Le correctif passe le même
oracle strict, avec les canaris de rétention/publication et une revue traitée.
[Sources et résultats](VALIDATION.md#6-novembre--confirmation-de-quarantaine-sans-republication).
Ce lot ne modifie pas les pièces d'audit externes ; aucun candidat outbox ou
fencing externe n'est clôturé. Non scellé, processus perdu et moteurs restent
ouverts ; G09 FAIL.

## Registre workers humain — 6 novembre 2026

Le lot `1bede3b` ajoute une permission explicite et une projection administrative
bornée, auditée et révoquée côté Rust. La revue a trouvé quatre incompatibilités
historiques : UUID nil, date runtime non interprétable, i64 arrondi et UTC hors
format. Les contre-exemples sont reproduits avant correction ; les trois réponses
HTTP des cas legacy sont aussi lues par TypeScript. Les [preuves](VALIDATION.md#6-novembre--registre-workers-humain)
séparent ces défauts du premier endpoint absent et de l'erreur de syntaxe TS.
Migration, limites de lecture et interdiction d'auto-promotion ont leurs canaris.
Revue traitée et composition RC0, sans verdict nouveau sur les pièces externes,
le transport multi-hôtes, le navigateur ou les moteurs. La vue personnelle liée
à l'initiateur reste obligatoire et ouverte ; aucun gate global clos.

## Propriétaires Full et admission — 6 novembre 2026

Le lot `5026f07`, intégré dans `485eb6b`, corrige le retour d'arrêt normal
avant fin de la future Full, puis la course d'admission découverte en revue.
Les deux défauts ont un RED comportemental ; les erreurs de harnais antérieures
restent distinctes. Quatre parents Full et cinq canaris Cold passent, avec
compilation du lot et de sa composition, guides EN/FR et contre-revue traitée.
Le refus de saturation précède mutation/ACK, sans événement Resume ajouté ni
ticket Submit neuf ; les replays exacts à capacité pleine restent admis.
[Commandes, sources et limites](VALIDATION.md#6-novembre--propriétaires-full-et-admission-avant-ack).
Ce résultat logiciel ne clôture pas le candidat outbox externe et ne qualifie
ni SIGTERM réel, ni crash, ni fencing. La confirmation de quarantaine, la
reprise froide et les moteurs réels restent ouverts ; G09 FAIL, aucun gate clos.

## Drainage des tâches d'audit — 6 novembre 2026

Retour prématuré du même chemin shutdown que serve reproduit avant correction :
le producteur était fini, un vrai POST avait committé, sa réponse restait retenue.
Le registre borné et son drainage passent six fonctions ciblées après correction,
avec revue et compilation toutes cibles. Les erreurs de harnais de compilation
sont séparées des RED/PASS comportementaux dans les [preuves](VALIDATION.md#6-novembre--drainage-des-propriétaires-daudit).
Ce résultat ne confirme pas le candidat outbox externe et ne qualifie pas un
SIGTERM/processus ou moteur réel. Les pièces externes ne sont pas modifiées.
Quarantaine, D5/fencing et pannes restent ouverts ; G09 FAIL,aucun gate clos.

## Propriétaire d'audit et ServiceCold — 6 novembre 2026

Le défaut logiciel de publication est rejoué sur `02834eb` : vraie boucle
restreinte, état V4 terminal, HTTP/SQLite vivants, aucun nouvel effet runtime.
Le raccord conserve Starts/Full fermés et distingue identité, contrôle terminal,
reçus descriptifs et ACK durable. Le parent partiel discrimine une mutation qui
abandonne un reçu déjà obtenu ; ce contrôle n'est pas une nouvelle vulnérabilité.
Les [preuves du lot](VALIDATION.md#6-novembre--propriétaire-authentifié-et-boucle-audit-restreinte)
séparent vrais RED, erreurs de harnais, limites d'espace/mémoire et deux parents
finaux PASS, avec check toutes cibles RC0 et revue indépendante traitée.
Aucun moteur réel ni crash processus n'est qualifié.

Les pièces externes ont été rehashées le6novembre sans modification : findings
`8e1b85a7b8a561d4d023fda8de943be1e742a95c0afb916841345cca3bec387b`, metadata
`6865e7e4ffd5dac4b4a35e05b6e66db23488697ce4470d70db3df84fefe296a6`.
Leur audit reste distinct. Ce lot ne confirme ni ne clôture le candidat outbox
externe ; tout rapport final doit encore être rapproché du code courant.
G09 FAIL, aucun gate global clos.

## Audit retenu et reprise logicielle — 6 novembre 2026

Les contrôles purs de cohérence terminale, puis le merge d'ACK restreint et son
acteur ont leurs reproductions et régressions distinctes. Les écarts de revue
portent sur Claim/namespace, séquences, disparition de job et rollback d'un
voisin sous un floor connu. Les montages de corruption appartiennent aux
fixtures privées du contrôle ; aucun franchissement worker/tenant ni nouvelle
vulnérabilité distante n'est établi. Le lecteur historique complet avait déjà
les contrôles Claim/namespace.

Le dernier gel passe 17 parents ciblés après les RED et les corrections de
harnais, avec compilation toutes cibles RC0 et revue indépendante traitée.
[Commandes, résultats et limites](VALIDATION.md#6-novembre--ack-retenu-composé-au-writer-stops).
La boucle ServiceCold, son owner authentifié, D5/fencing et les moteurs restent
ouverts. Cela ne clôture ni le candidat outbox externe, ni l'audit indépendant,
dont ce chantier ne modifie pas les fichiers originaux.
G09 FAIL, aucun gate global clos ; aucune qualification de produit annoncée.

## Reprise après annulation — 5 novembre 2026

Défaut d'état reproduit sur le code courant, puis défaut distinct de publication
après perte de réponse identifié en revue et reproduit avant correction.
Le broker réconcilie l'observation terminale de l'autorité retenue et reprend
le résultat durable sans nouvelle émission runtime. Code `f738662` composé en
`7cdb969`, quatre nouveaux parents et deux canaris PASS, toutes cibles et
format ciblé PASS ; revue indépendante et guides EN/FR traités.
[Reproductions, gels, commandes et limites](VALIDATION.md#5-novembre--réconciliation-logicielle-après-annulation).

Cette preuve fonctionnelle à 60 s ne qualifie pas le Writer produit à 10 s,
la reprise froide, D5 ou un vrai moteur. Elle ne confirme pas le candidat outbox
du rapport externe. Sa proposition parallèle de réduction des lectures est
rejetée après revue, sans modifier les contrôles tardifs. Les SHA des pièces
externes revérifiés le 5 novembre restent ceux consignés ci-dessous ; audit
externe encore distinct, aucun résultat ou fichier original modifié.
G09 reste FAIL, aucun gate global clos.

## Diagnostic opérateur après intégration — 4 novembre 2026

Le refus trompeur d'un journal requis absent est reproduit puis corrigé dans
la frontière existante du broker, avec revue et régressions ciblées
[consignées séparément](VALIDATION.md#4-novembre--journal-requis-absent-et-diagnostic-de-récupération).
Il s'agit d'un défaut de diagnostic, sans nouvelle vulnérabilité confirmée
ni nouvel audit global. Les métadonnées et empreintes des pièces externes
revérifiées le 4 novembre restent inchangées (`findings` SHA `8e1b85a7b8a561d4d023fda8de943be1e742a95c0afb916841345cca3bec387b`,
metadata SHA `6865e7e4ffd5dac4b4a35e05b6e66db23488697ce4470d70db3df84fefe296a6`).
Tout rapport final doit encore être rapproché du code courant. FD/Daybreak,
WSI-Q02 et qualifications runtime restent ouverts ; aucun gate global clos.

## Raccord produit après correctifs — 4 novembre 2026

Le backend workload/registre/relais est intégré par `c762a24`, après86tests
ciblés, revues indépendantes et contrôle composé de toutes les cibles sans
avertissement. Les constats supplémentaires de ces revues sont détaillés dans
[VALIDATION](VALIDATION.md#4-novembre--backend-workload-registre-et-relais-intégré),
avec leurs RED et limites : indexation, recul UTC, collision de classes,
association de schéma et contrat publié. Ils ne sont pas transformés en
nouvel audit global ni en vulnérabilités distantes formellement confirmées.

Les pièces externes restent inchangées en lecture seule : findings SHA
`8e1b85a7b8a561d4d023fda8de943be1e742a95c0afb916841345cca3bec387b`,
metadata `in_progress`. Tout rapport final reste à rapprocher de ce code plus
récent, pas de correction spéculative. FD/Daybreak, WSI-Q02 et qualification
runtime restent ouverts ; G09 FAIL,0/12 gates globaux clos.

## Correctifs initiaux de l'audit en cours — 3 novembre 2026

L'audit courant reste ouvert. Ses sources sont `main@c4c4d2a` et
`codex/broker-service@d6cb136` ; les correctifs repartent de
`origin/codex/broker-service@0a41aac192f4139073a43830dbb71df01cb87abf`.
Les pièces originales de `~/security-audit-skill/ai-guard/run-1/` restent en
lecture seule. Les sévérités fournies sont provisoires ; les constats futurs
seront rapprochés du code courant, y compris des lots déjà intégrés.

Trois worktrees dédiés portent les lots export, tenants et ingress ; un
quatrième porte leur intégration. Le checkout principal et les travaux
`enforcer-full3`/`broker-supervision` ne sont pas modifiés. La page OIDC
`complete`, déjà corrigée par `96fe9be`, n'est pas reprise.

| Lot | Code revérifié et critère | État constaté |
|---|---|---|
| A — export | Métadonnées actives filtrées à l'écriture, contrôles/limites conservés, fichiers 0644 et scan brut distinct. | Deux RED comportementaux, puis 11 fonctions ciblées GREEN ; revue indépendante acceptée. Code intégré au candidat `357b6e0`, guides EN/FR synchronisés. |
| B — panne d'un tenant | Erreur voisine isolée, store propre fermé et refus des collisions préservés ; projection humaine partielle explicite. | Deux RED comportementaux, puis 7 tests Rust et 24 tests frontend GREEN ; TypeScript PASS. Revue indépendante acceptée, manifeste et guides EN/FR corrélés ; intégré dans `b4c2fee`. |
| C — ingress | Pool upload distinct, lecture bornée avant acquisition de l'échange ; corps vides admis sans occuper le pool upload. | RED avec 64 uploads actifs et GET 429 ; deux régressions et six canaris ingress GREEN. Trois tests model_ingress locaux supplémentaires sélectionnés par le filtre, sans modèle ; revue indépendante et guides EN/FR acceptés, intégré dans `57fae74`. |
| E — initiation SSO | État initial volatil borné ; intention durable après preuve vérifiée, avant lecture tenant/admission. | Quatre RED comportementaux puis sept régressions et sept canaris OIDC GREEN au premier gel ; oracle final renforcé PASS séparément. Revue et guides EN/FR acceptés ; intégré dans `808f08c`, clarification API `14d5eb8`. |
| D — stockage HTTP | I/O hors Tokio, lanes bornées par store, droits revérifiés au point d'I/O et intégrité incrémentale. | RED puis corrections des constats de revue ; 59 fonctions uniques, revue finale et contrôle composé PASS. Intégré dans `b168784` ; aucune qualification de performance/runtime. |

Les tests de reproduction de l'audit passent lorsque le défaut existe : ils
ne constituent pas des régressions sans inversion des assertions. Une erreur
de compilation, zéro test sélectionné ou un dépassement du harnais n'est pas
un RED comportemental. Les tests ciblés sont confiés à Luna ; chaque lot
exige aussi une revue indépendante avant intégration, commit et push.

### Réception intermédiaire du 3 novembre, 09 h 40

Les fichiers externes `findings.json` et `HARDENING-AND-GUARANTEES.md` ont
évolué pendant les corrections ; `REPORT.md` et `FINDINGS-DETAIL.md` portent
encore leur version de 02 h 36. Une copie privée comparée aux sources est
conservée dans `/private/tmp/ai-guard-audit-incoming-20261103.EgVnob/`.
SHA-256 du JSON :
`8e1b85a7b8a561d4d023fda8de943be1e742a95c0afb916841345cca3bec387b`.
Ses 16 entrées portent les verdicts externes suivants : 13 `confirmed`,
2 `needs_validation`, 1 `rejected`. Ces verdicts concernent les instantanés
audités ; ils ne prouvent ni la présence ni la correction de chaque défaut
dans le candidat courant.

La confrontation reste ouverte, notamment pour les refus/reprises de claim,
les rafales de stdout, le verrouillage après épuisement des descripteurs et
les volumes non bornés. Aucun correctif de ces sujets n'est déduit de leur
seul intitulé. L'outbox reste `needs_validation` dans ce JSON. Les lots D/E
en cours et la préparation de F continuent ; les contrats et reproductions
seront recontrôlés avant modification, sans changer les fichiers de l'audit
ni les travaux D5 en parallèle. Aucun nouveau gate n'est fermé.

Un triage statique ciblé sur `2bacbdb` retrouve le latch de pénurie FD et
repère une perte de l'erreur OS typée dans un contrôle `openat`. Il ne vaut
pas reproduction courante : **NOT_RUN**. La réparation exige aussi de garder
le témoin du commit si une vérification de noms échoue après le commit durable ;
ignorer seulement le latch pourrait affaiblir le refus de rollback. Rapport
privé : `/private/tmp/ai-guard-audit-fd-triage-20261103.md`. Ce chantier attend
le gel/intégration D, qui modifie les mêmes coutures. Aucun explorateur Python
de l'audit n'est lancé sur l'hôte ni ajouté au produit.

Pour B, la conception suit la correction proposée dans le rapport provisoire :
une erreur voisine ne choisit aucune identité et ne rend pas ses credentials
authentifiables ; toute collision lue dans un store sain reste refusée, y
compris après mutation du provisioning. Une absence de collision dans une base
illisible ne peut pas être prouvée. Cacher volontairement un binding dans une
telle base nécessite déjà l'accès aux fichiers du contrôle de confiance, qui
permet aussi de supprimer ce binding. Cette limite sera conservée dans la
preuve et la revue, sans prétendre à une vérification exhaustive d'une base
indisponible. Un index construit au démarrage ne suffirait pas à suivre les
mutations ultérieures. Les tests du candidat B préservent cette distinction ;
sa revue et son intégration ne qualifient pas la latence des scans voisins.

### Lot A — résultat intégré au candidat

Source d'implémentation `54546d5`, reprise avec traçabilité dans `357b6e0`.
Les deux RED ont exécuté chacun une fonction et échoué sur la présence de
`.git` après extraction, respectivement par le chemin public et le chemin
autorisé. Les erreurs antérieures de préparation du sandbox sont conservées
comme `NOT_RUN` dans les preuves, sans être comptées comme reproductions.

Après correction, commandes ciblées hors ligne :

```text
cargo test --offline --locked --test audit_workspace_export
cargo test --offline --locked --lib workspace::export_policy_regression_tests::authorized_export_filters_host_metadata_while_scan_keeps_observed_files -- --exact
```

Résultats naturels RC0 : six fonctions d'intégration, une fonction d'export
autorisé, puis quatre canaris exacts `workspace::export_durability_tests::`
sur les barrières succès, faute avant publication et faute après publication.
Onze fonctions uniques PASS ; aucun test ignoré additionné. Le canari de
profondeur hors sélection fsync n'a pas été exécuté dans cette campagne.

La revue indépendante confirme filtres, validation et consommation des entrées
ignorées, manifeste limité aux fichiers écrits, scan brut conservé et ordre
flush/mode/fsync/publication. L'avertissement est émis sur le stderr du
contrôle, sans noms ni contenus ; sa livraison au client IPC/dashboard n'est
pas démontrée. Sa branche d'échec IO reste `NOT_RUN`, avec propagation avant
publication vérifiée à la lecture. Aucun assainissement universel des projets
n'est annoncé. Les guides locaux EN/FR et opérations décrivent ces limites.

Preuves privées préservées sous
`/private/tmp/ai-guard-audit-corrections-20261103.Wv9egg/test-a-retry.T2xgVv9X/` :
snapshots RED/GREEN distincts et rapports `report/red-results.txt` et
`report/green-results.txt`. Les trois empreintes GREEN correspondent aux
fichiers intégrés ; `Cargo.lock` reste
`915e887ad3869bc20ed82cabad59ad16c2c58d9d26a2ac8acef9ae50c6afff98`.
Tests Linux ARM64 dans Docker isolé, Rust1.99.1 ; aucun payload Git exécuté,
worker réel, Podman, macOS, build release ou qualification production déduit.

Le lot A et ses guides ont été poussés dans `197e394`, référence distante
vérifiée. Son worktree a été fermé par `git worktree remove`, sans force,
après inventaire suivi/non suivi/ignoré vide, équivalence du patch intégré et
absence de consommateur ouvert. La référence source `54546d5` et les preuves
hors checkout restent conservées ; aucun volume ni donnée utilisateur supprimé.

### Lots B et C — résultats intégrés au candidat

B : source `f1b4122`, intégration `b4c2fee`. Les deux RED ont observé 503 au
lieu de200 pour B, après corruption de l'audit A puis sous verrou SQLite A.
Après correction, `cargo test --offline --locked --test tenant_api` passe4/4,
dont le refus503 pour le store propre verrouillé et la complétude rétablie
après reprise. Trois filtres exacts passent chacun1/1 :

```text
--test human_access credential_collisions_between_tenants_and_classes_never_select_a_tenant
--test oidc_gateway_flow oidc_subjects_read_only_their_durable_tenant_even_with_a_forged_tenant_claim
--test sso_cli signed_native_session_reads_exports_resumes_and_rotates_without_token_files
```

Ces filtres sont exécutés avec `-- --exact`, pas comme une suite SSO entière.
Le scénario OIDC vérifie aussi l'admission B malgré A indisponible, les
contextes partiels et le refus503 d'un sujet dont les appartenances restent
inconnues. Les collisions ajoutées après création du gateway restent refusées.
Frontend : quatre fichiers ciblés, 24/24 PASS, et TypeScript5.9.3 `--noEmit`
RC0 ; aucune vérification navigateur/DOM ou SSO réel déduite. Les nouveaux
guides et le contrat API exigent la mise à jour coordonnée des clients.

Preuves B : `test-b/reports/baseline-red.txt`, `green.txt` et
`green-source-hashes-final.txt` sous le répertoire privé des corrections.
Le manifeste complet est une vérification APRÈS tests, corrélée aux sept
empreintes annoncées avant exécution et au worktree ; aucune preuve antérieure
n'a été fabriquée. La copie GREEN est protégée de l'UID1001 par propriété1000,
répertoires0555/fichiers0444 et parent1777 appartenant à root (sticky bit,
caps effectives nulles, `no-new-privileges`). L'entrée RED bind readonly reste
inchangée. Les tentatives de préparation ratées restent séparées des résultats.
Les scans O(n) et attentes SQLite voisines subsistent : ce lot ne qualifie
ni l'équité ni la latence ; le lot D doit traiter cette exécution synchrone.

C : source `80fa688`, intégration `57fae74`. Le RED a gardé64 uploads en
lecture et observé429 pour le monitor attendu200. GREEN :
`cargo test --offline --locked --lib ingress::admission_tests:: -- --nocapture`
passe2/2 ; le filtre `ingress::tests::` passe les six canaris visés et trois
`model_ingress::tests::` supplémentaires par correspondance de sous-chaîne
(neuf fonctions au total, aucun vrai modèle ni accès externe). Aucun rejeu
inutile après ce constat ; les comptes restent distincts.
Preuves : `test-c/baseline-corrected/` et `test-c/green/` dans le même
répertoire privé. Les empreintes GREEN sont
`3019844dd3074e33ce1b0e4f8bee3a17a601f4c98aac475c1900455616f53f6d`
pour `src/ingress.rs` et
`4278626804449bab857178712bc39181661da0c798822b8763ebc28a621def5c`
pour la régression. Les preuves RED et les erreurs de montage `noexec`
`NOT_RUN` restent préservées séparément.

C ne neutralise pas toute saturation : des requêtes avec corps peuvent encore
recevoir429 du pool upload ; requêtes complètes/SSE et connexions lentes ont
leurs autres limites. Le runner conserve son arrêt sur erreur de suivi. La
recette historique d'arrêt utilisait un moteur fixture ; aucun worker réel ni
retry60s n'est qualifié ici. Un contrôle composé du candidat intégré reste
distinct des tests exécutés sur chaque gel de lot.

### Lot D — résultat intégré au candidat

Source `3820bec`, intégrée avec traçabilité dans `b168784`. Les 14 fichiers
Rust/tests et le lock correspondent au gel revu ; le merge documentaire
conserve la correction E du paragraphe `native-intent.action`.

Après les RED de thread et de voisinage, les trois RED de revue observent
7/4/1 transactions malgré saturation, avec fixture et preuve OIDC vérifiées.
Le correctif lie réellement permis, store et lane. Source/contexte sont
dédupliqués ; les métadonnées ont leur borne par store, les profils utilisent
Read et le runner résolu garde Control. Aucun index de démarrage ne devient
autorité ; indisponibilité et absence restent distinctes, collisions lisibles
toujours refusées dynamiquement.

Les cinq filtres lib passent 12 fonctions. Les canaris finaux ajoutent47 :
intégrité12, sécurité3, tenants4, humain5, initiation SSO7, flow OIDC7, CLI3,
frontière d'authentification6. **59 uniques PASS**, RC0 naturels. Le rejeu
d'une unité après `let _ =` dans le contrôle de test n'est pas additionné.
Aucun warning sur ce rejeu ni les canaris finaux. [Commandes et preuves](VALIDATION.md#3-novembre--stockage-http-borné-et-déporté).

La revue indépendante accepte spécification, qualité, preuves et guides
EN/FR. Cleanup OIDC réservé avant mutation, slot conservé jusqu'à fin réelle ;
shutdown peut empêcher un job encore en queue, sans rendre Pending/rotating
utilisable ni promettre un terminal durable. Les permis des pages ne
traversent ni attente fournisseur/réseau ni retour de l'extracteur.

`cargo check --offline --locked --all-targets` passe ensuite sur archive Git
exacte `b168784`, cible neuve privée, sans warning et sans exécuter de test.
Clippy Linux et build release restent NOT_RUN. Aucun worker/fournisseur réel,
isolation moteur, capacité100 ni panne stockage physique n'est qualifié.
Équité complète et absence de contention ne sont pas promises. F, FD, TTL
initial et rapport externe restent ouverts.

Après publication des branches source et intégration, le worktree D est
retiré normalement : fichiers suivis/non suivis/ignorés propres, patches
équivalents dans l'intégration et aucun consommateur détecté. La référence
distante source est `3820bec`. Son dernier conteneur de test, déjà expiré
RC0/nonOOM, est retiré par ID exact sans volume ; tous snapshots et preuves
restent privés hors de ces ressources. Reçus dans le dossier de preuves D.

### Suite F et FD — reproductions, correctifs encore ouverts

**F-SSE intégré** : `297376f`, source `279b5f2`, après RED réel du cinquième
Body admis 200 au lieu de 429. Limites fixes par tenant : 4 par principal
canonique, 24 humains dans le total 32 partagé. La lease demeure avec le Body
jusqu'au drop, y compris après erreur/EOF ; pas de timer de restitution.
Registre Weak purgé et acquisitions partielles annulées. Revue indépendante
sans bloquant ; 3 nouvelles intégrations, 2 unités et 3 canaris passent.
Ces preuves Router/SQLite/Body ne qualifient pas tous les sockets HTTP ni
l'éviction d'un client non pollé. Le budget durable F est intégré dans
`aa4eef9`, source `4095586` : cumul tenant/principal événements et octets du
payload exact, réserves contrôle/identité, classes internes et projection du
préfixe committé, même chaîne/schéma5. La revue a demandé une prévalidation
globale avant bootstrap et le raccord du refus logique dans le mapper de
contrôle. Ces deux RED ont été atteints séparément puis corrigés, relus et
vérifiés. **21 nouvelles fonctions et 30 canaris PASS**, guides EN/FR et
exemple JSON relus. Cinq tests de la composition finale F1/SSE et le contrôle
de toutes les cibles passent sur `aa4eef9`, sans warning. Worktrees F1/SSE
fermés normalement, références source poussées et preuves préservées.

Raccord client Web dans `f5b035a` : après un RED de parsing, 14 fonctions
passent, dont conservation des codes/actions de capacité SSE et de budget
de lecture, sans nouvelle autorité ni relance. La validation frontend ne
constitue pas une preuve d'émission backend du budget durable. Worktree SSE
source fermé normalement, branche et preuves conservées.

F est reproduit sur `b168784` avec le vrai routeur et deux stores : un humain
remplit les64événements A par48lectures après baseline16, puis empêche les
contrôles et les lectures d'un pair ; B reste disponible. Le RED exige le
head physique atteint après contrôles, pas un délai ou un throttle D.
Sur le correctif, A1 est refusé au head23/429 ; décision, ingestion, lecture
du pair et révocation restent durables, B200. Ce n'est pas une promesse de
capacité illimitée : les réserves sont finies, les bytes ne bornent pas le
fichier/WAL et aucun archivage qualifié n'est livré. Une base déjà pleine
n'acquiert aucune place par déploiement du correctif.

Le RED FD est aussi revalidé sur `b168784`, enfant seul en EMFILE réel puis
FD restaurés : chaîne valide, Store interdit et append impossible. La revue
de conception via Daybreak est suspendue par la demande d'authentification
matérielle de la session ; une reprise après la réponse du mainteneur a
renvoyé le même message. Aucune garde contournée, aucun code FD corrigé.
Les [preuves et limites](VALIDATION.md) distinguent ces deux composants de
leurs impacts réseau/runtime encore non qualifiés.

### Lot E — résultat intégré au candidat

Source `0fce510`, intégrée avec traçabilité dans `808f08c` ; clarification du
paragraphe natif API source `17ada11`, intégrée dans `14d5eb8`. Les quatre RED
naturels RC101 constatent des écritures durables avant preuve : démarrage Web,
intent/annulation natif, callback rejeté et saturation exacte du ledger.
Une erreur préparatoire ou un autre motif de503 ne compterait pas comme RED.

Premier gel :

```text
cargo test --offline --locked --test authentication_start_capacity
cargo test --offline --locked --test oidc_gateway_flow
```

Les deux commandes finissent RC0, sept fonctions chacune. Les empreintes de
production sont `d505903d…` pour `oidc.rs` et `aac25ba9…` pour `gateway.rs`.
Après renforcement du seul test/fixture, commande exacte :

```text
cargo test --offline --locked --test authentication_start_capacity verified_proof_cannot_bypass_missing_durable_audit_capacity -- --exact
```

RC0, une fonction PASS, six filtrées, aucun warning. Le compte OIDC actif sans
membership aurait produit un reçu de lecture tenant ; l'échec d'enregistrement
de l'intention avec capacité1 laisse son `event_count` inchangé et l'intégrité
valide, après vérification cryptographique de la preuve. Cela ne prétend pas
qu'un échec plus tardif d'admission ne peut jamais être précédé d'un reçu tenant.
Production inchangée ; les six autres fonctions de ce fichier et les sept
canaris ne sont pas rejoués sur cette dernière fixture. Le rejeu n'est pas une
quinzième fonction unique. L'inventaire antérieur sélectionnant zéro unité lib
reste un inventaire, jamais un test réussi.

Preuves privées : `/private/tmp/ai-guard-audit-e-luna-20261103/proofs/`, rapports
`red-result.txt`, `green-results.txt` et `green-v2/result.txt`. Test final
SHA-256 `0705555c32c33bdd664959ce1661d42b9612978394d0c4efce069fadd9e7df13` ;
manifest complet1991entrées identique avant/après. Rust1.99.1/LinuxARM64 dans
le sandbox Docker borné sans réseau externe, secret, socket ou checkout monté.
Revue indépendante spécification/qualité acceptée après deux précisions API ;
guides EN/FR synchronisés. Format durable, politique et bases non réécrits.

Une tentative non vérifiée expirée/annulée ne consomme plus le ledger. Après
preuve, l'audit obligatoire et les empreintes/fencing restent requis ; refresh,
logout et expiration de session conservent leurs règles. Les marqueurs
d'échange gardent la référence permettant de fermer une session installée
avant interruption de remise. Un registre déjà plein n'est pas réparé.
IdP réel, navigateur, CLI natif externe, moteurs workers, TTL initial complet,
Clippy Linux et qualification SSO globale ne sont pas couverts par ce lot.

Après push de l'intégration `2bacbdb` et de la branche source `17ada11`, les
références distantes exactes sont vérifiées. Le worktree E, propre y compris
les fichiers ignorés, sans consommateur détecté et dont les deux patches sont
équivalents dans l'intégration, est retiré normalement. Son conteneur de test
déjà expiré, RC0/nonOOM, est retiré par ID exact, sans volume. Branches,
snapshots et preuves privées sont conservés ; reçus dans le dossier `proofs`.
Le fichier `worktree-close-status-confirmed.txt` contrôle explicitement E ; le
premier fichier de statut capturait l'intégration et ne sert pas de preuve E.

### Préparation vérifiable des régressions

Pour C, la régression a été renforcée et relue indépendamment : un compteur
RAII prouve que **tous** les corps entrés en lecture restent incomplets avant
et après chaque oracle HTTP, client de contrôle et SSE. Toute expiration,
même d'un seul upload, rend le harnais inconclusif au lieu de produire un faux
GREEN en libérant un slot. Le fichier préparé
`tests/ingress_admission/slow_uploads.rs` a pour SHA-256
`216dd33af61d68a36bddbc498e178a82d8340c57a03826063c0d96fb9d5c7663`.
Ce gel préparatoire a produit le RED attendu : 64 lectures actives, GET monitor
429 attendu200, RC101 sans timeout ni OOM. Après correction, le nouveau test
SHA-256 `4278626804449bab857178712bc39181661da0c798822b8763ebc28a621def5c`
garde 16 uploads actifs et observe 48 refus par le budget dédié ; monitor,
client de contrôle, POST vide et SSE passent. Les anciens snapshots restent
distincts, sans effacer leurs preuves.

Pour B, deux tests du **nouveau contrat de complétude frontend** ont pu être
exécutés sans dépendance téléchargée, sur le parser inchangé de `0a41aac` :

```text
node --test --test-isolation=none tests/human-identity-coverage.test.mjs
Node v24.22.2 ; RC 1 ; 2 tests ; 0 PASS ; 2 FAIL attendus (RED).
```

Le parser perd `contexts_complete=false` et accepte l'absence du champ au lieu
de refuser une réponse incompatible. Ce RED prépare la projection partielle
requise par le correctif B ; **il ne reproduit ni ne confirme le défaut
backend de collision/disponibilité**. Le gateway de base `0a41aac` n'émet pas
ce champ. Le candidat B associe désormais DTO et parser stricts ; le nouveau
contrat exige des versions coordonnées du gateway, du dashboard et des clients
CLI stricts. Les quatre fichiers de tests frontend ciblés passent ensuite
24/24, RC0 ; TypeScript5.9.3 `--noEmit` passe également. Navigateur/DOM et
qualification SSO globale restent `NOT_RUN` pour ce lot.

Exécution par Luna dans une copie publique minimale et un conteneur dédié :
réseau/IPC désactivés, racine/source en lecture seule, UID1000, aucune
capability, `no-new-privileges`, CPU1, RAM512MiB, PID64, fichiers8MiB,
scratch64MiB/8192inodes, environnement vide complété par allowlist, délai45s.
Aucun secret, socket moteur, montage original, fournisseur, installation ou
E2E. Conteneur attribué arrêté/supprimé après le résultat naturel ; preuves
privées conservées dans `/private/tmp/ai-guard-luna-front-EHNOca/proofs`.
SHA-256 du parser :
`65edb8ad1027664f2d37b226ac7f2e46e4c994218dc4716a0ddf80ccd9c9bff5` ;
du test :
`e3378e86bd74712939fd7ede9bb6a39fbbd812f0bfcd89d4d67dfbb344919a64`.

### Dépendance de validation

Le sandbox utilise une image Rust locale Linux/ARM64, sans réseau,
secrets, socket moteur ou montage du dépôt original, avec source et toolchain
en lecture seule et scratch borné. La préparation hors ligne du cache échoue :
`cargo vendor --offline --locked` manque `android_system_properties 0.1.6`, et
`cargo metadata --offline --locked --filter-platform aarch64-unknown-linux-gnu`
manque `linux-raw-sys 0.12.1`. Les images locales sondées ne contiennent pas de
cache Cargo exploitable. Ce blocage historique a été levé après l'accord
explicite du mainteneur : copie privée des seuls répertoires publics de
registre, puis `cargo fetch --locked --target aarch64-unknown-linux-gnu` depuis
un environnement parent vide. Seul `linux-raw-sys 0.12.1` a été téléchargé ;
la résolution complète Linux `--offline --locked` passe sans modifier le lock.
Cache privé : `/private/tmp/ai-guard-linux-cache-20261103.DOKqBt/` ; aucun
credential/configuration du home n'a été copié. Les builds/tests restent sans
réseau, avec cache et cible propres à chaque lot, jamais sur l'hôte.

Après redémarrage signalé par le mainteneur, Docker29.9.2 annonce 8CPU et
`12585238528` octets de RAM ; le nœud Kubernetes local est Ready, mémoire
`12290272Ki`, stockage `129759836Ki`. Cela confirme le réglage annoncé12GoRAM/
128Godisque, pas un adaptateur Kubernetes ai-guard. Le MacBook Air M3/16Go est
sous pression : aucun dépassement de délai de harnais n'est une preuve de
vulnérabilité et aucun chiffre de performance n'est qualifié par ces tests.

Validation documentaire seule : `git diff --check` et vérification des liens
relatifs PASS (13 cibles présentes sur les deux documents). Le scan des deux
fichiers candidats ne détecte aucun chemin interdit ni secret connu. La revue
indépendante de la proposition est traitée : reprise conservatrice après
redémarrage et formulation conditionnelle du correctif C. Aucun de ces
contrôles ne remplace les reproductions et tests Rust encore `NOT_RUN`.

Analyse statique de formatage distincte : le `rustfmt --check --edition 2024`
de la toolchain macOS1.99.1 disponible passe sur `src/workspace.rs`,
`tests/audit_workspace_export.rs`, `tests/tenant_api.rs` et
`tests/ingress_admission/slow_uploads.rs` (trois commandes RC0, aucune édition
ni exécution du code projet). Les sondes dans l'image Linux ont en revanche
montré l'absence du composant rustfmt : formatage Linux `NOT_RUN`, sans
installation. Ces lectures statiques ne débloquent pas la compilation ou les
reproductions comportementales Rust dans leur sandbox.

Les huit anciens gardiens de tests orphelins cités par le mainteneur ont été
identifiés par PID, propriétaire, naissance, programme et rôle, puis arrêtés
par SIGTERM ciblé avec son autorisation. Aucun volume, worker, credential,
preuve ou binaire conservé n'a été supprimé ; aucun autre processus n'a été
arrêté. Ce nettoyage manuel n'est pas une preuve de supervision autonome.

### Frontière de comportement et suite

Le runner conserve son arrêt ciblé sur erreur de suivi. La
[proposition de fenêtre transitoire](GATEWAY_TRANSIENT_FAILURE_PROPOSAL.md)
est soumise séparément ; elle n'est ni approuvée ni implémentée.
Après intégration de B, D et E avancent dans des worktrees distincts depuis
`2eeaa0d` ; E est désormais intégré après revue et F attend D.
D a reproduit l'exécution de l'intégrité
sur le thread HTTP après vérification de sa fixture et d'un contrôle
`spawn_blocking` : RC101 sur l'assertion de thread, sans seuil de latence.
Six fonctions ciblées passent sur un gel intermédiaire ; la revue relève un
couplage d'admission d'un profil voisin. Une nouvelle reproduction HTTP passe
la baseline complète et le contrôle source saturée429, puis échoue RC101 sur
voisin saturé429 au lieu du200 partiel attendu ; preuve privée
`ai-guard-luna-D-20261103.dgTRqE/proofs/red-auth-neighbor.log`.
Ce premier couplage est corrigé, puis le raccord OIDC et l'oracle de cleanup
borné sont ajoutés. Les trois commandes lib ciblant
`api::read_execution_tests::`, `api::store_io::tests::` et
`audit::incremental_integrity_tests::` passent RC0 : respectivement 2, 3 et 3
fonctions, huit uniques, sans warning. Preuves privées
`ai-guard-luna-D-20261103.dgTRqE/proofs/d-final-{1,2,3}.log` ; manifeste et
mtimes des 244 sources identiques avant/après. Ce sont les unités de ce gel,
pas des canaris d'intégration ni une preuve de capacité 100.

La revue finale source demande encore `D-REVIEW-01` : résolution de credential
local, revalidation de la source B pour un contexte A, et profils OIDC réalisent
des transactions sans permis du store effectivement consulté. Les bornes
globales restent réelles ; le 429 terminal ne prouve pas l'absence d'I/O avant
admission. Rapport privé
`/private/tmp/ai-guard-d-final-review-20261103.3l6hSq/REVIEW.md`.
Les trois régressions ont ensuite atteint leurs assertions exactes, RC101 :
7 transactions de découverte source, 4 de revalidation source/contexte et
1 de profil OIDC malgré les permis retenus. Baselines, preuve cryptographique
OIDC et intention durable préalable sont vérifiées ; aucune gravité ni
performance déduite. Logs privés `proofs/d-review-01-red-{1,2,3}.log` dans
le même dossier D ; sources et empreintes avant/après identiques. Les
modifications du fixture OIDC partagé et de ses deux consommateurs sont
limitées aux imports pour réemploi en unité. Un import test inutilisé est
signalé, sans être confondu avec un échec de compilation.
Le [contrat des lanes](DECISIONS.md#lot-d--périmètre-des-permis-de-lecture-décision-retenue)
borne explicitement découverte et profils par store, sans nouvel index
d'autorité. Ces RED précèdent la correction puis les unités/canaris et la
revue acceptée du [lot D intégré](#lot-d--résultat-intégré-au-candidat).
Exploitation déployée et sévérité mesurée ne sont pas déduites de ces tests.
La [stratégie de test](TEST_STRATEGY.md#latence-et-pression-de-la-machine-de-développement)
conserve les contraintes du MacBook Air M3/16Go, sans transformer la pression
de la machine en verdict de sécurité.
L'outbox et les autres candidats non établis restent hors correction sur la
seule base de ce rapport provisoire.

Les worktrees B/C sont également fermés proprement après vérification de la
référence distante, des fichiers suivis/non suivis/ignorés et des consommateurs.
Leurs références source restent conservées. Les trois derniers conteneurs de
tests B/C, ne contenant plus qu'un `sleep`, sont retirés par ID exact ; aucune
preuve, volume, donnée utilisateur ou ressource D5 n'est supprimée. Le reçu
privé `test-c/cleanup-receipt.txt` distingue cette fin de cycle des tests.
Le [contrôle composé](VALIDATION.md#3-novembre--correctifs-audit-abc-et-compilation-composée)
passe toutes les cibles Rust sur `2eeaa0d`, puis le seul test-cible de fixture
après correction de deux warnings Linux, sans exécuter de tests supplémentaires.

### Point SSO préexistant à valider séparément

La revue E relève un point de contrat sur les TTL initiaux : les transactions
Web et intentions natives sont contrôlées avant l'échange token/JWKS ; le point
d'installation vérifie aussi le marqueur d'échange borné. La borne de bout en
bout depuis l'intention initiale n'est pas démontrée. Ce comportement existait
avant E et le déplacement de l'audit après preuve ne change pas ces horloges.
Statut **NEEDS_VALIDATION**, sans sévérité attribuée : confronter le contrat de
TTL par étape et reproduire avec une barrière locale avant tout correctif.
Le canari existant de deadline de refresh authentifié ne qualifie pas à lui
seul ce cas initial. Ce point reste ouvert dans le produit ; aucun SSO global
ou délai d'authentification complet n'est déclaré qualifié par E.

**Les cinq lots A/B/C/D/E sont intégrés au candidat, après reproductions,
tests ciblés et revues indépendantes. Aucun test worker/runtime réel exécuté
pour ces lots, aucune qualification production déduite.** Les résultats historiques ci-dessous
conservent leurs dates et leurs périmètres.

---

L'[audit importé](evidence/2026-10-29/independent-audit/REPORT.md) porte sur `/private/tmp/ai-guard-audit-20261029.n8ZxPB`,169 fichiers ; manifest annoncé `77876e6de40964096f90f458b07b872b7848f1a05257e2f763e5f57679641d34`. Ses originaux restent hors dépôt et inchangés. La copie de `findings.json` conserve **0 confirmé,19 needs_validation,0 rejeté**. Le workflow [Cloudflare security-audit](https://github.com/cloudflare/security-audit-skill) distingue confirmation, fait restant à vérifier et candidat réfuté ; aucune sévérité formelle n'est ajoutée ici.

Le checkout a évolué indépendamment. Le candidat209 et ses essais Docker/navigateur ne sont pas une reproduction contrôlée des19 candidats. Ce suivi distingue le delta du code, les preuves de développement disponibles et la validation de sécurité manquante. Une correction n'authentifie pas rétroactivement l'instantané ni ses données. Le statut BLOCKED de l'audit concerne son environnement de validation, pas l'ensemble des travaux produit autorisés.

## Rapprochement en cours

| Empreinte originale | État du code courant et prochaine preuve |
|---|---|
| `api/preauth-linear-tenant-credential-hash-amplification` | Correctif de développement : index privé partagé, digest d'entrée borné calculé une fois, pas de copie complète des secrets/politique au clonage de l'état. Guards typés avant JSON/query/path. RED4FAIL/2PASS puis51PASS ciblés, vraie réponse401 sans attendre le corps et revue du delta acceptée. [Preuves](evidence/2026-10-30/api-auth/README.md). Impact/charge DoS et transport lent non qualifiés ; verdict original needs_validation conservé. |
| `api/sse-graceful-shutdown-without-drain-deadline` | Correctif gateway/dashboard : drainage cinq secondes après signal, pattern ingress conservé, setup Unix fallible avant readiness. Vrais flux SSE maintenus >6 s avant SIGTERM puis sorties en5007/5010ms, SIGINT au repos vérifié ; revue et build PASS. [Preuves](evidence/2026-10-30/http-shutdown/README.md). IO/teardown bloqués, charge lente et autres environnements restent hors preuve ; verdict d'audit initial inchangé. |
| `api/viewer-synchronous-reads-control-store-contention` | Constat historique : lectures Store synchrones sur les futures HTTP et connexion partagée. Sur `5e52807`, le déplacement de handlers hors des threads Tokio est partiel : les scans de collision vers des stores voisins et certaines lectures coûteuses restent à traiter dans le lot K du [suivi final](#audit-final-run-1). Les preuves historiques ne clôturent pas ce résidu. |
| `audit/preexisting-database-permissions-not-enforced` | StoreStorage refuse parent non privé et main/WAL/SHM/journal non600/UID/lienunique/ACLadmise sans réparation. Gel327, tests macOS/Linux, ACL POSIX et deux UID réels, backup commun relu et maintenance QA explicite dans le [dossier SQLite](evidence/2026-10-30/sqlite-storage/README.md). Filesystems/ACL/UID non couverts et pannes restent ouverts ; verdict historique needs_validation inchangé. |
| `audit/sqlite-connection-path-reopen-identity-race` | Parents privés ancrés/revalidés, métadonnées par nom et HAS_MOVED obligatoire ; verrou distinct par parent et noms main ASCII évitent les collisions du namespace. Suppression du FD brut main après RED/GREEN F_GETLK : sa fermeture annulait les locks d'une autre connexion du même processus. VFS conserve ses FD actifs ; migration/backup utilisent le même garde. Handoff pathname contre sameUID/root/admin et substitution A→B→A non qualifiés ; [preuves et limites](evidence/2026-10-30/sqlite-storage/README.md), aucun verdict original réécrit. |
| `dashboard/static-root-symlink-follow-disclosure` | ServeDir configurable reste à examiner ; l'image QA readonly avec dashboard sans montage réduit certaines préconditions, sans qualifier tout front-dir hôte ou image dérivée. |
| `ingress:forward:shared-preauth-exchange-pool` | Pool global encore acquis avant lecture complète/authentification upstream. Timeouts bornent la rétention, sans démontrer disponibilité sous contention ; réservation du contrôle et test borné requis. |
| `operations/config-path-reopen-secret-substitution` | Initialisation par descripteurs puis montage Compose par pathname ; course de parent et identité du répertoire à analyser jusqu'au moteur, pas seulement jusqu'à canonicalize. |
| `operations/extended-acl-secret-projection-disclosure` | Bits de mode et propriétaire ne constituent pas une preuve portable contre une ACL étendue. Diff Linux/macOS et essai à deux UID requis ; aucune ACL existante supprimée. |
| `operations/jev-control-credential-reuse` | Correctif de développement gel235 relu : refus avant publication/init, avant client/startup et à l'assemblage GatewayState local/multi-tenant ; clé normalisée, politique conservée. Tests positifs/négatifs avec absence de connexion fournisseur. Verdict de l'audit original inchangé. |
| `redaction/json-object-keys-preserved` | Code modifié : clés JSON masquées avec self.text ; régressions antérieures disponibles. Relecture différentielle et limites de collision/projection en cours ; verdict original conservé. |
| `redaction/truncation-before-secret-masking` | Correctif de développement gel235 relu : union des occurrences littérales et suffixes connus sur octets originaux avant décodage, y compris périodicité/chevauchement ; métadonnées d'erreur CLI préservées, marqueurs privés fusionnés. Régressions et capture Docker synthétique passent. Les encodages/fragments arbitraires ne sont pas une garantie de cette API ; verdict original inchangé. |
| `runner/credential-env-path-reopen-substitution` | Lecture du fichier source liée à un FD O_NOFOLLOW/fstat ; ancienne course du lien final corrigée dans la source. Parents, snapshot et réouverture --env-file restent à qualifier. |
| `runner/decision-omits-known-worker-secret-redaction` | decide refuse une requête que son dictionnaire enrichi modifierait, avant HTTP. Tests d'autorisation existants ; ne pas confondre ce refus avec une garantie universelle sur secrets inconnus/encodés. |
| `runtime/abrupt-broker-stop-leaves-worker-unfenced` | Identités/checkpoints/verrou/reprise manuelle livrés ; aucune deadline autonome après mort du CLI. [Nouvelle preuve de développement Docker Desktop/macOS ARM64](evidence/2026-10-30/cli-crash/README.md) : SIGKILL du seul CLI, même worker encore Running après7s pour une limite5s, puis arrêt/réconciliation/audit/quota/usage vérifiés. Premier défaut de harnais conservé, unique retry corrigé. Supervision permanente approuvée mais non livrée ; cette reproduction sur un nouveau gel ne réécrit pas le verdict de l'audit original. |
| `runtime/doctor-effect-engine-identity-unbound` | Docker doctor/build/up/down utilisent désormais une cible Unix et un EngineID liés, revérifiés avant effet avec le même programme approuvé ; contexte modifié et ID divergent couverts par régressions. Le contrôle et l'effet restent deux connexions sans fencing atomique. Podman opérateur ne reçoit pas cette preuve d'identité par ce lot ; qualification distincte ouverte. |
| `runtime/path-resolves-project-executable` | Recherche PATH ambiant supprimée sur les parcours livrés. Politique2 : entrée d'installation connue ou approbation explicite absolue hors projet, chemin canonique ouvert par composants sans liens, FD du répertoire final retenu pour exec relatif, owner/modes/ACL vérifiés, hash borné, environnement reconstruit et invocation durable. Migration explicite readonly/auditée sans réarmer les claims. Gel261 et preuves Docker ciblées dans le dossier trusted-invocation ; aucune fermeture générale des plugins, dépendances, sameUID/root ou déplacements d'arbres de contrôle par un administrateur de namespace. Le verdict original reste needs_validation. |
| `runtime/worker-output-engine-log-retention-unbounded` | [Lot presetv2](evidence/2026-10-30/engine-logs-v2/README.md) : `--log-driver none` imposé et inspecté avant claim/start, compatible avec historiques v1. Régressions ciblées, revue et Docker réel canary/ACK503 PASS ; déploiement QA avec audit668 conservé puis690 après nouveau run v2. Baseline réelle : driver `local` avec rotation native, pas de rétention illimitée ni fuite confirmée sur l'instance testée. Garantie conditionnelle au broker/v2 inspecté ; anciens brokers, Podman/autres plateformes et pression disque générale hors preuve. Verdict historique needs_validation inchangé. |
| `runtime/workspace-volume-no-byte-quota` | Limites d'import/export et RAM ne limitent pas les octets/inodes écrits dans un volume. Quotas effectifs et réserve du contrôle restent obligatoires. |

Les analyses détaillées, en lecture seule et sans exécution des probes, sont archivées : [runtime](evidence/2026-10-29/independent-audit/ai-guard-audit-runtime-delta.md), [filesystem](evidence/2026-10-29/independent-audit/ai-guard-audit-filesystem-delta.md), [secrets et statique](evidence/2026-10-29/independent-audit/ai-guard-audit-secret-delta.md). Les quatre candidats API/ingress sont relus par l'orchestrateur aux sources courantes ; leurs effets restent NOT_RUN.

Après ce rapprochement, les correctifs de développement capture/masquage et collision Jev/credentials sont intégrés au [gel235](evidence/2026-10-29/audit-followup/README.md), avec revue, vérifications logicielles et essais Docker nommés. Leurs tests ne changent pas les verdicts du rapport figé. L'image209 et ses preuves restent distinctes du nouveau candidat ; aucune qualification production globale n'est déclarée.

## Ordre de travail

1. Fermer le choix du programme moteur et les handoffs de fichiers de contrôle, sans croire qu'un simple chemin canonicalisé lie un effet à un inode.
2. Traiter les frontières de capture/masquage et la réutilisation Jev des credentials ; conserver des régressions pertinentes, sans publier de canary ou secret réel.
3. Implémenter la supervision durable du broker, avec armement de durée avant start et reprise sans réarmer un claim. Le design préparatoire n'est pas une fonctionnalité livrée.
4. Qualifier stockage/logs et contention/drainage ; poursuivre la recette trois workers, relais et cibles runtime/cloud. Aucune de ces exigences n'est rendue facultative.

Les corrections sont soumises à la même méthode : comportement reproduit si possible, test correctif, second regard, vérifications intégrées et preuves adaptées à la portée. Un refus d'exécuter une probe d'audit sans sandbox conforme demeure un manque de preuve ; il ne devient ni un rejet du candidat, ni une qualification du produit.
