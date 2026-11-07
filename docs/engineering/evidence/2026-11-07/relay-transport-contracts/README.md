# Relais : frames et voie HTTP fermée — 7 novembre 2026

**50 fonctions logicielles uniques PASS et toutes cibles RC0 sur le gel43**,
après revue source indépendante des deux composants. Aucun moteur, worker réel,
fournisseur ou transport inter-hôtes n'est qualifié par ces résultats.

## Source et acceptation

Base d'intégration `7a583ece3943098584ba51669bbf13afa892b8a9`, composant frames
`316f95de3da944784ccfe0f6c6ba63612334db52` composé dans `b938e2f`, puis huit
fichiers de voie HTTP. Le gel privé `relay-contracts43` est créé avant tests
par archive Git et copie explicite des seuls huit fichiers. Le manifeste
contient dix-sept ancres Cargo/code/tests de ce gel et les deux guides finalisés
ensuite. Documentation et manifeste ne sont pas des entrées des tests passés.

Le codec vérifie la longueur BE32 avant allocation, refuse JSON ambigu,
champs/versions inconnus, profondeur ou complexité structurelle excessives,
et ferme son lecteur après erreur/EOF. Les limites des DTO restent distinctes
de l'espace d'enveloppe. Handshake exact, epoch, une requête en cours,
corrélations non réutilisables et plafond1024 sont vérifiés. La réponse est
liée à la requête/opération/IDs ; l'inbox est recoupée à l'endpoint attendu
fourni par le contrôle. Aucune frame ne construit une preuve d'identité.
Les validations pures extraites du client HTTP conservent ses refus de secret,
formes exactes et résultats inconnus. Elles ne constituent pas une DLP exhaustive.

La voie `WorkerRelay` n'admet que send/inbox/ACK V1. `VerifiedWorker` précède
corps et capacité, avec huit échanges par tenant authentifié, séparés des
lanes runner/présence. La query canonique unique est propre à l'inbox et
reconstruite par l'ingress. UUID métier canoniques, y compris nil, restent
compatibles avec le contrat existant ; canal/corrélation exigent non nil.
Inbox réussie2Mio, reçu/erreur worker16Kio, réponse historique512Kio : aucune
augmentation globale. Les handlers gardent leur revérification transactionnelle.

Les régressions utilisent de vrais UnixStream et des pairs HTTP/Axum avec
SQLite de fixtures privées : fragmentation, invalidité/poison, corrélation et
replay, refus avant lecture du corps, saturation8 et séparation A/B/runner/broker,
révocation pendant upload, inbox de dix messages64Kio via ingress, ACK/replay
et refus d'audit. Les workers Running de ces fixtures sont synthétiques ; ils
ne remplacent pas l'inspection de trois runtimes réels.

## Exécutions Luna

Commandes équivalentes aux sélections capturées, chaque fois `--locked --offline`
et tests `--test-threads=1 --nocapture` :

| Sélection Cargo | PASS | Build / tests | Journal privé |
|---|---:|---|---|
| `test --lib relay::channel::tests::` | 22 | 2 min04 / 0,07s | `relay-contracts43-frames-af.log` |
| `test --test control_worker_relay_transport` | 3 | 56,97s / 0,04s | `relay-contracts43-routes-af.log` |
| `test --test worker_relay_api control_lane_cases::` | 3 | 5,44s / 2,87s | `relay-contracts43-lane-af.log` |
| `test --test ingress_control_transport` | 9 | 3,82s / 0,01s | `relay-contracts43-ingress-af.log` |
| `test --test worker_relay_api client_cases::` | 13 | 0,63s / 7,82s | `relay-contracts43-client-corrected-af.log` |
| `check --all-targets` | — | 1 min09 / aucun test | `relay-contracts43-check-af.log` |

Une commande parent erronée `--test worker_relay_client` est refusée RC101 :
cette cible n'existe pas et aucun test n'a tourné. Son journal est conservé ;
le nom a été corrigé depuis la déclaration réelle `client_cases` sans modifier
le code ni relancer les quatre sélections déjà vertes. Ce refus d'argv n'est
pas présenté comme un défaut produit ou un RED de sécurité.

Les compilations signalent 61 avertissements bibliothèque. Le check garde aussi
deux avertissements `human_sso_gateway_trust` et deux bibliothèque-test.
Les 68 lignes d'en-tête d'avertissement comprennent les récapitulatifs ; elles
ne sont pas un nombre de défauts distincts. Aucun Clippy ou build sans warning
n'est crédité. Aucun test fournisseur/E2E complet n'est relancé.

## Empreintes et environnement

Journaux parent privés sous `ai-guard-cold-drain-logs-20261106.nXx9XJ`, sans
publication de corps, credentials, bases, binaires ou sortie de scratch :

| Journal | SHA256 |
|---|---|
| frames | `77b0255a6513264b0d2c0969a5b5a05155ddb29bd62acc8af96ab8f04ed49803` |
| routes | `3394f33de8e364900a42a01285127ef3fddf10e5869d095c3a9a71e4aadbcec1` |
| lane | `74b37f7b599822e330b7f0a5c00467e12a156b7f2d270b40e4f7aa3d3a8e28f3` |
| ingress | `8d221e2139faef5df95dc70f5ea06e5ab0b67fd0fffee4a2fd7540989b6e1eb5` |
| client cible absente | `3f122b7083efddd456c685cb3dac7f4c539baf4aca0b19f3873d013160696341` |
| client corrigé | `36ca217525d31aba9d8d50e4473cc69db3968a10477dc7272293f194bf9808df` |
| check | `cd9d3cc9b8f90dc9ddcd447094a817c718a336516154f7010cc304861f69781f` |

QA AF admise : image Rust/Node épinglée
`sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`,
UID1000, caps0, NNP/seccomp, réseau none/loopback privé, racine/source/outils et
registry RO, environnement allowlist, quatre CPU,8Gio sans swap,256PIDs,
scratch4Gio/65536inodes, fsize512Mio/nofile1024/core0. Cible dédiée
`/scratch/target-relay-contracts43-af`, Rust1.99.1, jobs1/debug0/incremental0.
Inspection initiale SHA256
`eb6af82da8dee9d93a046793cda786fd3c993fc40d3d4595bfec79dbaf81f0ab`.
Pas d'OOM ni processus Cargo/test restant observé après les commandes.
Les limites de QA ne mesurent pas les besoins ni la capacité du produit.

## Limites ouvertes

Le helper, découverte UDS livrée, port exec runtime, propriétaire/jonction,
outbox durable, deadlines et budgets mémoire agrégés restent des travaux
distincts. Le profil client typé `/control` n'est pas ajouté par ce lot.
Une autorisation d'implémentation n'est pas une campagne réelle autorisée.
Révocation, timeout ou EOF ne prouvent ni rollback d'une requête partie, arrêt
du worker ou libération quota. Aucun gate global n'est clos ; G09 reste FAIL.
Voir la [direction approuvée](../../../RELAY_RUNTIME_HANDOFF.md) et les guides
[EN](../../../../setup/relay/README.md)/[FR](../../../../setup/relay/README.fr.md).
