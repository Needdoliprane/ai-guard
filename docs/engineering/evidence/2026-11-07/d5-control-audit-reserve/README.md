# Admission et réserve d'audit du contrôle — 7 novembre 2026

**19 fonctions uniques PASS sur les gels61/62 ; compilation toutes cibles62
PASS. Clippy strict FAIL.** Les résultats de composant ne qualifient ni D5 complet, ni moteur,
ni disponibilité sous charge. Aucun gate global n'est clos.

## Contrat et source

Base `410d4d24bd75c02dd6262864412ec637a0d45bed`. Le [manifeste](source.sha256)
couvre sept fichiers Rust, trois documents et les deux fichiers Cargo.
Les [guides EN](../../../../setup/identity/README.md)/[FR](../../../../setup/identity/README.fr.md)
et le [contrat API](../../../../api.md) expliquent les plafonds et la reprise.

Les trois faits `broker.abandon-before-claim.requested`,
`broker.retirement.pending` et `broker.retirement.final` sont admis uniquement
sur la route d'événements du contrôle, après authentification runner avant corps.
La même liste fermée est revérifiée par l'entrée Store privée ; ni nom libre,
payload, credential worker ou viewer ne sélectionne la classe de stockage.
L'ingestion ordinaire refuse exactement ces trois faits supplémentaires.

Le Store distingue désormais `OrdinaryWrite` et `ControlWrite`, classes privées
choisies par les méthodes Rust de confiance. Pour événements **et** octets :

| Admission | Plafond cumulatif |
|---|---|
| Ingestion ordinaire | physique − réserve contrôle − réserve identité |
| Contrôle typé et livraison runner fermée | physique − réserve identité |
| Maintenance identité | physique |

Lectures humaines et écritures workload conservent leurs budgets distincts.
Les décisions, mutations de contrôle, genesis et grants/bindings runner utilisent
leurs helpers typés ; send/ACK/heartbeat worker restent en classe workload et les
révocations en maintenance identité. Aucun champ Event ne promeut une ingestion
ordinaire. Un rejeu exact vérifie contenu et reçu avant toute nouvelle admission.

La capacité est cumulative, pas renouvelée par reconnexion ou redémarrage.
Aucune migration SQLite ou remise à zéro n'est ajoutée. Une occupation
historique supérieure au nouveau plafond ordinaire reste lisible, vérifiable
et rejouable ; elle n'est pas déclarée corrompue ou effacée. Les nouvelles
écritures restent soumises à leur plafond actuel. Ce changement ne reconstitue
pas une réserve déjà consommée.

## Reproduction et validation ciblée

Les deux parents nouveaux utilisent le vrai router Axum, SQLite privé et une
petite capacité explicite32/C8/I4. Ils n'effectuent ni flood ni campagne de
latence. Les identités et les faits d'exécution de fixture sont synthétiques.

| Gel / sélection Cargo (`--locked --offline`) | Résultat |
|---|---|
| 59, `test --test d5_control_audit` | RED : deux assertions atteintes, contrôle403 au premier fait et écriture ordinaire admise au total21. |
| 61, même cible | 2 PASS,0 FAIL ; compilation1 min51, tests0,25s. Oracles et source du nouveau parent inchangés. |
| 61, `test --lib audit::capacity_tests::` | 16 PASS,0 FAIL ; inclut événements/octets, budgets/rejeux, réouverture et occupation historique. Durée réelle59,93s, compilation comprise. |
| 61, `test --test supervision_api ordinary_exhaustion_keeps_control_capacity_and_control_exhaustion_keeps_revocation` | FAIL de préparation de l'oracle : CreateIssued sans les reçus d'import requis reçoit409 avant le contrôle de capacité. |
| 62, même canari | 1 PASS,0 FAIL ; compilation1 min50, tests0,42s. |
| 62, `check --all-targets` dans AL | RC0 en95,34s ; aucune fonction exécutée par ce check. |
| 62, `clippy --all-targets -- -D warnings` dans AL | RC101 : 89 diagnostics sur la bibliothèque, dont61 dead_code et28 autres lints. Les cibles suivantes ne sont pas annoncées analysées après cet échec. |

Le canari62 utilise une transition Unknown avec raison bornée, valide depuis
Preparing avec quota détenu ; ces préconditions sont assertées. Il conserve les
oracles : ingestion ordinaire fermée au total40, Prepare réel admis à41, rejeu
exact au total56, mutation de contrôle refusée429 sans publication au total57,
puis révocation d'identité durable. Il ne renumérote pas simplement un test rouge.
Le diagnostic HTTP a aussi été précisé pour mentionner contrôle ou identité.
Ce sont les deux seuls écarts61→62, sans changement de logique d'admission.

Les 19 fonctions sont uniques : 2 +16 sur61 et le canari sur62. Elles ne sont
pas annoncées comme dix-neuf nouvelles exécutions sur62. Les tests utilisent
`--test-threads=1 --nocapture`, des cibles Cargo propres à chaque gel et un seul
Cargo à la fois.

## Journaux parent et environnement

Journaux privés sous `ai-guard-cold-drain-logs-20261106.nXx9XJ`, sans publication
de bases, binaires, corps de messages ou credentials :

| Journal | SHA-256 |
|---|---|
| `diagnostic59-audit-baseline-ak.log` | `533f6935760a62bdcefd80355314d95d861ea97d286c9ce63f0e359d5a62b12e` |
| `audit61-d5-ak.log` | `2f4cf5046536751594da39885e4099850b18ba8bd72b2d739c41e2628c4b6bb7` |
| `audit61-capacity-ak.log` | `95fcda6ec599c769b2cede14372801d466a9bb77a54cd02dd7f4f9f1e73d8664` |
| `audit61-supervision-ak.log` | `684bde2600c27dc50685200717c884c0e8313df89e8022842c8e18b6a6c07aba` |
| `audit62-supervision-ak.log` | `977a656c4e46c77cc645fa88cd0f8aac9c3e222f11c4b561f020a9347488aa9a` |
| `audit62-check-al.log` | `35c60b2c540f3488fbffa293185b95824308c4ec5d7512ae8e97470da6a99452` |
| `audit62-clippy-al.log` | `cfc3db60bb87d09ab4adb77c5cd7b3ed521013559e34da7929aa18af6ff151f6` |

Manifestes privés complets Rust/Cargo61 et62 respectivement SHA
`d1c3f2f4e8abb98bd6547e2ad13c603b41219540f58641e8e12a95c0e46f2653`
et `10b4ae2c354aa997e26b63668004067371823217800a7cc9cbdca7b5069e0bf0`.

QA AK Linux ARM64, Rust1.99.1, image épinglée
`sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`.
UID/GID1000, capacités supprimées, NNP/seccomp, réseau coupé/loopback privé,
racine/outils/sources/registry RO, scratch4Gio/65536inodes,4CPU,8Gio sans swap
additionnel,256PIDs, fichier512Mio, nofile1024 et core0. Environnement vide puis
allowlist avec HOME/tmp/Cargo scratch et offline ; aucune dépendance téléchargée
par les builds. Les postflights constatent OOM0 et absence de processus de test
résiduel. Après conservation des journaux, seuls les caches de compilation
attribués et inutilisés56/58/59 ont été retirés pour libérer le scratch ; sources,
preuves de fixture et données existantes n'ont pas été nettoyées pour obtenir un PASS.

AK est arrêté volontairement par Root à `2026-11-07T20:51:00.633079843Z`,
code137, OOMKilled=false, puis son conteneur exact est supprimé. Le code d'arrêt
n'est pas le code des tests. Inspection finale privée SHA
`89f8636a47f9219ef035c7ff558109cbd35775eb9e388b8b3134b0d0047555c9`.
Le check62 passe dans une nouvelle QA AL de même profil, sans transfert de
cible Cargo ni artefact produit. Inspection initiale AL SHA
`ad1324793b83cf28048dafadf08ba149215369f0e7e3edbd1064a32a3621f39d`.
Clippy1.99.1 est préparé depuis l'archive officielle publique autorisée et
vérifiée ; version compatible avec rustc1.99.1. Le run strict échoue et aucun
lint n'est supprimé. Les61 dead_code étaient déjà signalés par le check. Les28
autres diagnostics couvrent tailles de Result/enum, conditions, plages,
cast, nombre d'arguments et emprunt mutable inutile. L'un de `audit.rs` concerne
la vérification de lease préexistante, hors des hunks de cette correction ;
aucun verdict exhaustif de non-régression Clippy n'en découle. G12 reste ouvert,
aucun build sans warning ni test fournisseur n'est crédité.

## Revue et limites d'intégration

La revue indépendante a recoupé les classes, leurs appelants, les deux plafonds,
l'identité et les rejeux. La précision du message HTTP demandée est appliquée
sur62. Le correctif du canari ne change pas le comportement du produit.

**Dépendance obligatoire :** les producteurs D5 doivent utiliser le transport
de contrôle, y compris leur replay lié aux identités durables, avant composition
de leur bundle. Leurs anciens `post_event` sont désormais refusés403. Ce raccord
est un composant séparé en cours de revue ; ce présent lot ne le prétend pas livré.

Les dix-neuf fonctions ne constituent ni disponibilité sous charge, ni fin des
lots H/I/M, ni validation de pannes physiques, runtime Docker/Podman/Kubernetes,
workers réels, reprise complète ou qualification de release. Les signaux D5 sans
preuve durable de réservation après perte de leur owner restent un écart à
traiter, sans publication ordinaire ou autorité inventée.
