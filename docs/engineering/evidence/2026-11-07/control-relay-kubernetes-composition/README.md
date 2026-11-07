# Composition du client relais et de la reprise Kubernetes — 7 novembre 2026

**PASS logiciel ciblé : 25 fonctions uniques et compilation toutes cibles RC0.**
Cette preuve porte sur les sélections ci-dessous au gel27. Aucun verdict de
release, moteur worker, cluster ou gate global n'en découle.

## Source, composition et revue

Le gel `control-relay-kubernetes-composition27` part de la base d'intégration
`8a5c4217cf28b385d4d2610bf5507c29af72873c`, qui contient le client relais
`8c38ea5` composé dans `914786ca1723248041fe89d439def869347cd004`.
Il ajoute les trois sources Kubernetes du gel03 : `src/kubernetes_cli.rs`,
`src/runtime/kubernetes/admission_remote_coordinator.rs` et ses tests.
Le composant publié `81a9e7501adf921a23098741dd0518e5d5287660` est intégré
dans `f12f2663f897a1350661aa076fdebd474e44edf8`.

Le [manifeste source](source.sha256) contient 419 entrées Rust/Cargo, y compris
les probes Rust historiques présents dans les preuves. Son SHA-256 est
`01f0a24cc60d55d4f116b206594914795dadc4f12b4c02748641b9e187ad9fcd`.
Root vérifie 419/419 entrées avant les captures ; chaque en-tête de capture
porte cette vérification. Cette passe documentaire vérifie de nouveau les 419
empreintes sur le gel privé et sur le checkout composé `f12f266`, RC0 dans
les deux cas. Ce manifeste ne couvre pas tous les artefacts du dépôt et ne
prouve pas que chaque source a été exercée.

Les revues indépendantes source et preuves du
[client relais](../relay-client/README.md) et de la
[reprise Kubernetes](../kubernetes-held-recovery/README.md) sont acceptées.
Leurs reproductions, corrections et validations propres restent attribuées à
leurs gels respectifs. Le check composé apporte une compilation de l'ensemble
Rust/Cargo identifié ; il ne transfère aucun résultat runtime ou de fédération.

Les 52 fonctions de la [composition26](../control-recovery-composition/README.md)
restent attachées à son manifeste de 417 entrées et à ses captures. Elles n'ont
pas été rejouées ici et ne sont pas additionnées en une campagne de 77 fonctions
au gel27.

## Recettes observées

Les commandes suivantes sont équivalentes aux sélections observées. Les
captures identifient le répertoire de travail et les tests exécutés, mais ne
conservent pas un argv complet historique. Elles supposent le même gel et la
QA explicitement admise ; cette passe documentaire ne lance aucune recette.

```sh
cargo test --offline --locked --manifest-path /inputs/control-relay-kubernetes-composition27/Cargo.toml --test worker_relay_api client_cases:: -- --nocapture --test-threads=1
cargo test --offline --locked --manifest-path /inputs/control-relay-kubernetes-composition27/Cargo.toml --lib runtime::kubernetes::admission::remote_coordinator_tests:: -- --nocapture --test-threads=1
cargo test --offline --locked --manifest-path /inputs/control-relay-kubernetes-composition27/Cargo.toml --lib kubernetes_cli:: -- --nocapture --test-threads=1
cargo check --offline --locked --manifest-path /inputs/control-relay-kubernetes-composition27/Cargo.toml --all-targets
```

| Sélection capture02 | Résultat observé | Compilation / tests | SHA-256 de la sortie |
|---|---|---|---|
| Client relais, `client_cases::` | RC0, 13 PASS, 0 FAIL, 18 filtrés | 0,44 s / 7,97 s | `05a519c92d620e912e6ebb1b78d3ecad2852366362b5803a8cecb9a61fd9c99e` |
| Coordinateur Kubernetes distant | RC0, 9 PASS, 0 FAIL, 581 filtrés | 0,22 s / 4,98 s | `f21925269f77f479d1615b9f729761fbdaa0ca2a16acae51f41592a455c7542f` |
| Module CLI Kubernetes | RC0, 3 PASS, 0 FAIL, 587 filtrés | 0,23 s / 0,40 s | `cef0ea737d53c284bd79123fc304e0a57929a1e1998d315f9cfea05e373f66b9` |
| Check toutes cibles | RC0, aucun test compté | 1 min 06 s | `ed27d1d53bd940d1dd30905435dcdc5bde60befefd97926be043bbe4d6f7bf61` |

Les temps de compilation des tests sont ceux des captures finales sur cible
déjà préparée, pas ceux d'un premier build à froid. Les trois sélections ont
zéro test ignoré et forment 25 fonctions distinctes. Le parent CLI de reprise
n'est pas recompté séparément des trois fonctions du module.

Les 13 clients conservent authentification worker liée, grants dirigés,
refus inter-tenant, révocation, rejeux exacts et pertes de réponses send/ACK,
refus local des IDs contenant la preuve, JSON unique/fermé, bornes et refus
des redirections. Les neuf parents du coordinateur gardent absence de second
claim/CREATE/adoption, CAS/epoch historique après cancel, résultat inconnu et
rejeu après ACK perdu. Le module CLI vérifie la reprise du journal sans
credential Kubernetes et le scope, dont schéma 10 admis et 11 refusé.

Le check signale 61 avertissements bibliothèque, deux pour
`human_sso_gateway_trust` et deux bibliothèque-test. La sélection client signale 61
avertissements bibliothèque ; coordinateur et CLI signalent chacun deux
bibliothèque-test. Aucun résultat Clippy ni compilation sans avertissement
n'est crédité à cette composition.

## Incidents de capture conservés

Avant capture02, Luna avait observé 13+9 PASS, puis 13 PASS dans les sorties
d'outils sans redirection vers un journal parent archivé. Ces observations
ne fournissent pas les quatre pièces retenues ici et ne sont pas comptées en
plus. Root réexécute les trois sélections et le check avec capture complète.

La première capture Root, `composition27-relay-ac.log`, termine RC1 :
`unexpected argument '--manifest-path' found`. L'option était placée au
niveau Cargo avant la sous-commande. Aucun test n'est exécuté dans cette
tentative ; ce défaut d'invocation n'est pas un RED produit. Son SHA-256 est
`66534939e7aaa02052fb4d5dc7fbbb85d2c4f36078a5de8353d8644a539382a4`.
La capture02 corrige l'invocation sans modifier la source.

Après les quatre fins RC0 et le contrôle du manifeste, Root arrête
explicitement le conteneur QA avec `docker stop --time 1`, puis retient son
inspection finale avant retrait du seul CID de QA. Celle-ci indique état
`exited`, code 137 et `OOMKilled:false`. Ce code de fin de QA ne remplace pas
les codes des quatre recettes et ne constitue pas l'arrêt d'un worker produit.
Un watcher préventif était aussi programmé ; aucune fin par sa borne n'est
déduite de l'inspection.

## Environnement et pièces privées

QA AC emploie Linux ARM64/Rust 1.99.1 et l'image épinglée
`sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`.
Root a contrôlé son profil d'admission : UID/GID 1000, capacités nulles,
`no-new-privileges`, seccomp, réseau `none`, racine/source/toolchain/registre
public readonly, aucun socket moteur ni port publié. Limites : quatre CPU,
8 Gio RAM sans swap, 256 PIDs, scratch 4 Gio/65536 inodes, 512 Mio par fichier,
1024 FD et core 0. Cargo utilise un job, sans incrémental ni information de
debug, environnement vidé puis limité aux entrées admises, cible dédiée
`/scratch/target-control-relay-kubernetes-composition27-ac` et aucune surcharge
`RUST_MIN_STACK`. Ce profil de QA ne qualifie pas un confinement worker.

Les journaux privés se trouvent sous
`/private/tmp/ai-guard-cold-drain-logs-20261106.nXx9XJ/` :

- `composition27-capture02-relay-ac.log` ;
- `composition27-capture02-kubernetes-coordinator-ac.log` ;
- `composition27-capture02-kubernetes-cli-ac.log` ;
- `composition27-capture02-check-ac.log` ;
- `composition27-relay-ac.log`, échec d'invocation ci-dessus ;
- `composition27-root-manifest-check.txt`, 419 lignes, SHA-256
  `64f91a20ae2b800dfc6ef8fb80c59840c9e5d410d3538620b6b2afcedf84de03` ;
- `qa-ac-inspect.json`, SHA-256
  `542ec22182cc8c18fccc9ac7931e3a7bc6703845932b1e50b625efd2dd0a50dd` ;
- `qa-ac-final-inspect.json`, SHA-256
  `9d47a07695ec7b66f31b61ad372bb8004448647c81365db145f5333c740f6886`.

Le gel et son manifeste sibling sont sous
`/private/tmp/ai-guard-audit-followup-inputs-20261106.xu8TFf/inputs/`, nommés
`control-relay-kubernetes-composition27` et
`control-relay-kubernetes-composition27-files.sha256`.
Les empreintes des pièces ci-dessus sont relues dans cette passe documentaire.
Ces emplacements temporaires ne sont pas une sauvegarde produit ni une
livraison d'archives publiques. Aucun journal brut, base, credential,
certificat, exécutable ou payload runtime n'est copié dans ce dossier Git.

## Limites et écarts conservés

HTTP/SQLite/fichiers et les identités de test sont réels ; les fixtures de
workers et le pair Kubernetes TLS produisent des faits runtime déclarés.
Aucun Pod ou worker métier n'est lancé. La reprise confirme un résultat broker
retenu et un ACK durable, avec `current_runtime_state: not_observed` et
`admission_available: false`. Une issue inconnue conserve son quota ; aucune
durée, consommation ou mesure d'usage zéro n'est inventée.

Le [handoff réel du relais](../../../RELAY_RUNTIME_HANDOFF.md) et le raccord
réseau restent ouverts. Un worker `network:none` ne gagne aucune route par la
présence de la bibliothèque. Docker/Podman/Kubernetes réels, recette A1/A2/B1,
100 workers, admission/activation/start/release Kubernetes, stockage exclusif,
arrêt/quiescence et reprise physique restent non qualifiés. Les fournisseurs,
la PKI entreprise et le parcours fédéré UI/API complet ne sont pas qualifiés
par ces tests.

Fencing D5 complet, réserves/rétention d'audit et contrôle des sorties réseau
conservent leurs écarts. Les avertissements et Clippy restent ouverts ;
**G09 reste FAIL et aucun des 12 gates globaux n'est clos**. Les
[gates](../../../PRODUCTION_READINESS.md) et la
[stratégie de test](../../../TEST_STRATEGY.md) gardent leur autorité.
