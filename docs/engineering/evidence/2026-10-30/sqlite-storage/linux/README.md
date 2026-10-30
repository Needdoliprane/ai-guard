# Stockage SQLite — essais de développement Linux ARM64 du 30 octobre 2026

Résultat : **PASS pour 41 tests d'intégration distincts, 1 unité VFS et 1 test supplémentaire d'ACL POSIX**, plus une sonde locale à deux UID. Aucun fichier du dépôt n'a été modifié. Ce résultat porte sur les tests logiciels du stockage dans un conteneur de développement Docker Desktop ; il ne qualifie ni worker, ni broker, ni déploiement Linux de production.

## Périmètre, gel et environnement

Instructions AGENTS récentes (OVPT compris), RTK, contrat d'architecture, règles de code, gates et stratégie de tests relus. Snapshot autorisé par root après gel Rust : `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `src/` et `tests/`, y compris `tests/support/mod.rs` et les fixtures adaptées. Aucun frontend, répertoire de travail original, secret, configuration opérateur ni socket moteur n'a été monté dans les conteneurs.

Artefacts conservés : `/tmp/ai-guard-sqlite-linux.8QMTSx`. Archive `source.tar`, manifeste `SOURCE_SHA256SUMS.txt`, contexte de build, Dockerfile et logs restent en place. La vérification `shasum -a 256 -c SOURCE_SHA256SUMS.txt` sur le checkout après les essais et `sha256sum -c` dans l'image ont toutes deux terminé avec code 0 ; logs `host-source-verification.log` et `image-source-verification.log`. Le contenu Rust testé correspond au gel et au checkout à l'instant de ce contrôle, sans déduire un commit d'un dépôt entièrement non suivi. Pendant la relecture ultérieure, root a ajouté le mode privé 0700 à la création du tempdir dans `tests/runtime_terminal_persistence.rs` : ce fichier n'appartient pas aux quatre suites exécutées ici. La relecture constate alors 71/72 entrées du manifeste identiques au checkout courant ; le code applicatif et les quatre suites Linux sont inchangés. L'archive et l'image restent le périmètre exact de cette preuve.

- Client explicite : `/opt/homebrew/bin/docker --host unix:///Users/louis/.docker/run/docker.sock`.
- Docker client 29.2.1, serveur Engine 29.9.2, Docker Desktop 4.97.0 (244584).
- Serveur Linux ARM64, noyau `7.0.14-linuxkit`, image Debian 12 bookworm ; `uname` observé `aarch64`.
- Rust `1.99.1 (58cf8f9c3 2026-10-13)`, Cargo `1.99.1 (5f94df478 2026-08-27)`.
- Cible filesystem observée pour `/tmp` et `/sqlite-qualification` : `overlayfs`.
- Image de cache existante : `agentsandbox-fs-tests:qa-20261030-8c19d640`, ID `sha256:beb9719c84aca478d800123cdd98ece83ef8b59dc10afbc936e988d6e7003941`.
- Image de ce lot : `agentsandbox-sqlite-tests:qa-20261030-8QMTSx`, ID `sha256:07f9d4821dc1acc9c882378cee7e64ff0f69d94b73d2e8c05719913610317f24`.

L'image reprend le cache de compilation et les dépendances déjà présents. Le snapshot courant est copié dans `/sqlite-qualification` ; `/build/target` sert uniquement de cache Cargo. Le build `--network none` compile avec `--locked --offline`. Aucun accès au réseau public n'a finalement été nécessaire.

Les essais utilisent UID/GID `1000:1000`, `--network none`, `--cap-drop ALL`, `--security-opt no-new-privileges`, limites de processus/mémoire/CPU, aucun montage, aucun port publié et `privileged=false`. Les inspections enregistrées dans `containers.txt` et `two-uid-container.txt` utilisent le libellé abrégé `caps` pour **HostConfig.CapDrop**, pas pour CapAdd. Le relevé structuré `containers-bounded.jsonl` conserve explicitement les champs `CapDrop=["ALL"]`, `CapAdd=null`, `Privileged=false`, `NetworkMode="none"`, `SecurityOpt=["no-new-privileges"]`, `Mounts=[]`, `PortBindings={}`, `ReadonlyRootfs=false`, limites et commande de chacun des cinq conteneurs. Le principal a PidsLimit 256, Memory 2147483648, NanoCpus 2000000000 ; les auxiliaires ont PidsLimit 128, NanoCpus 1000000000, Memory 1073741824, sauf ACL à 2147483648. Le rootfs est inscriptible pour les compilations et fixtures locales. Les conteneurs sont conservés, sans `--rm` et sans suppression de volume ou d'image.

## Commandes et résultats

Préparation : archive locale filtrée, extraction dans un contexte neuf et `LinuxTests.Dockerfile` dérivé du Dockerfile de développement du lot `control-filesystem`. Toutes les commandes hôte ont été exécutées avec `login:false` et le PATH explicite de la mission.

Build :

```text
docker --host unix:///Users/louis/.docker/run/docker.sock build --progress plain --network none -f LinuxTests.Dockerfile -t agentsandbox-sqlite-tests:qa-20261030-8QMTSx .
```

Ce Dockerfile exécute sous UID 1000 :

```text
cargo test --locked --offline --no-run --lib --test storage_security --test audit_integrity --test tenant_storage --test control_migration
```

Build PASS, code 0, compilation et couche finale conservées dans `build.log`.

La commande principale est lancée avec les restrictions ci-dessus dans `ai-guard-sqlite-linux-8qmtsx` :

```text
cargo test --locked --offline --test storage_security --test audit_integrity --test tenant_storage --test control_migration -- --nocapture
```

| État | Cible | Résultat observé |
|---|---|---|
| PASS | `audit_integrity` | 12 PASS, 0 FAIL, 0 ignoré |
| PASS | `control_migration` | 10 PASS, 0 FAIL, 0 ignoré |
| PASS | `storage_security` | 16 PASS, 0 FAIL, 0 ignoré |
| PASS | `tenant_storage` | 3 PASS, 0 FAIL, 0 ignoré |
| PASS | `cargo test --locked --offline --lib audit::storage::tests::unavailable_vfs_identity_control_is_refused -- --exact --nocapture` | 1 PASS, 0 FAIL, 0 ignoré, 56 unités filtrées |
| PASS | `cargo test --locked --offline --test control_fs linux_named_posix_acl_is_bounded_by_the_mode_mask -- --exact --nocapture` | 1 PASS, 0 FAIL, 0 ignoré, 12 tests filtrés |

Logs : `tests.log`, `vfs-tests.log`, `acl-tests.log`. Les 3 invocations enfants du test de verrouillage sont des probes du même test ; elles ne sont pas recomptées dans les 41 intégrations. Deux tests exclusivement macOS (ACL de migration et ACL main/sidecar) sont exclus par `cfg` sur Linux, ce qui explique les nombres différents des suites macOS ; cette absence n'est pas un succès Linux de ces tests.

Le test de verrous effectue réellement `F_GETLK` depuis un autre processus alors que SQLite conserve son WAL, puis tente et refuse `PRAGMA journal_mode=DELETE`. Il passe avant/après le drop d'un autre Store et après une ouverture de mauvais tenant refusée. Les tests couvrent également liens/spéciaux main et sidecars, parents et permissions, noms réservés et Unicode, first-open concurrent, attente bornée du lock d'initialisation, ouvertures/append multiprocessus légitimes, sidecars orphelins et hardlink du lock vers un autre inode SQLite vivant. La migration conserve sauvegarde v1, événement/hash historique et rollback DDL sur audit plein, avec réouverture des deux bases.

L'essai ACL écrit une vraie ACL POSIX nommée avec `setxattr` sur le filesystem du conteneur. Avec masque nul, le mode reste 0600 et l'ouverture conserve l'ACL byte pour byte ; avec masque de lecture, le mode devient 0640 et le lecteur privé refuse sans réparer le fichier. C'est une preuve réelle d'ACL/mode dans cet environnement, pas une simulation du service cloud.

## Sonde à deux UID

Une sonde Rust jetable `two_uid_probe.rs`, hors dépôt, a été liée au `libagentsandbox-5506ce63024bc455.rlib` compilé depuis le gel. Elle ouvre un vrai Store, ajoute un événement factice `linux.private.fixture`, vérifie l'intégrité et maintient la connexion active. Un témoin public séparé fournit le contrôle positif. Aucun corps d'événement ou contenu de DB n'est affiché.

Le conteneur `ai-guard-sqlite-linux-two-uid-8qmtsx` garde les mêmes restrictions ; les commandes négatives utilisent `docker exec --user 1001:1001`, sans capacité supplémentaire. `two-uid-results.json` conserve commande, identité, code et sortie de chaque tentative.

| État | Observation |
|---|---|
| PASS | UID 1000 : parent 0700, main/WAL/SHM réguliers, propriétaire 1000:1000, mode 0600, nlink 1 ; les trois fichiers existent pendant l'essai. |
| PASS | UID 1000 : lecture d'un octet main vers `/dev/null` réussie depuis un processus distinct de SQLite. |
| PASS | UID 1001:1001 observé, lecture d'un octet du témoin public vers `/dev/null` réussie. |
| PASS | UID 1001 : chaque lecture main/WAL/SHM échoue avec `Permission denied` et code 1. |
| PASS | UID 1001 : rename du parent privé sous `/tmp` root/sticky échoue avec `Operation not permitted`, code 1. |
| PASS | UID 1001 : précréation de `audit.sqlite-journal` dans le parent privé échoue avec `Permission denied`, code 1. |
| PASS | Métadonnées privées après tentatives identiques à celles relevées avant. |

La préparation de cette sonde a connu deux erreurs de harness, conservées : E0463 quand `rustc` n'avait pas encore le chemin explicite du rlib (`two-uid-build.log`), puis exec 126 pendant que l'édition de liens était encore en cours (`two-uid-ready.log`). La compilation finale avec chemin rlib explicite est code 0 (`two-uid-build-final.log`) ; l'exécution réelle affiche READY dans `two-uid-ready-final.log` avant les observations ci-dessus. Aucun code produit n'a été changé pour ces erreurs.

Après les observations, seul ce conteneur de sonde a été arrêté (`docker stop --time 2`) ; le processus maintenu volontairement vivant sort 137 lors de cet arrêt. Ce n'est pas compté comme un test de récupération après crash. Le conteneur est conservé et son répertoire factice main/WAL/SHM/lock a été copié sans ouverture SQLite dans `two-uid-state/` pour préserver les artefacts. Aucun fichier de DB, WAL, SHM, backup ou preuve existant n'a été supprimé pour obtenir un résultat vert.

## Limites et statuts restants

- PASS dans le périmètre observé : stockage logiciel Linux ARM64 sur overlayfs Docker Desktop, refus sûrs de la matrice testée, vrais verrous POSIX multiprocessus, consommation de la politique commune par migration et Stores, ACL POSIX testée, refus DAC à deux UID sur une DB vivante privée.
- NOT_RUN : attaque déterministe A→B→A entre opens, acteur hostile dans un parent mutable avec hook, DB précréée par un autre propriétaire, ACL donnant une autorisation active à l'UID 1001 sur tous les parents, crash/disque/power-loss, sauvegarde de migration relue par second UID et matrice de filesystem ext4/XFS/Btrfs native. Les tests de permission et d'ACL ne couvrent pas automatiquement tous ces cas.
- NOT_RUN / hors périmètre de ce lot : Docker/Podman en tant qu'adaptateurs worker/broker, recette A1/A2/B1, Linux x86_64, hôte Linux natif, Kubernetes, cloud, NAS et PKI. Aucun accès externe ni infrastructure supplémentaire utilisé.
- La frontière testée vise un autre UID ordinaire. Aucun isolement hostile même UID, root ou administrateur Docker/stockage n'est revendiqué. Les préconditions et limites de l'audit restent à examiner ; ce résultat ne transforme pas les candidats historiques en vulnérabilités confirmées.
- Les gates de production ne sont pas fermés par ce rapport. Aucune release n'est qualifiée ici.

Revue indépendante : sqlite_review a recoupé logs, comptages, 9 empreintes principales, paramètres Docker bornés et limites de provenance. Sa conclusion dans `/tmp/ai-guard-sqlite-review.md` ne relève aucun écart bloquant. Les 19 candidats historiques `needs_validation` ne sont pas reclassés par ces essais.

## Empreintes principales

```text
12ebed306bc1e4c3399fe3262b89df76f2bb509589beb1a6852e3c65ff841d4a  source.tar
aca1f640ffedfb2622be16f7971a19a91d1055dd42fb79a087c3a086554149c1  SOURCE_SHA256SUMS.txt
00c4d5c2a6c74e05754bb86734e3c9be93bfefbad73fcb40422c0ebbed939370  context/LinuxTests.Dockerfile
f7c31e1c711639b616034ccd2aac71ea79b138d1173267e0e769b93ae24ead48  build.log
3f846d21c6fa814eda2ce91f2f4e2eb3c581069d5152e3d0d1252cc4c0b2c993  tests.log
f4190a539132c0372c374019f024e52db5e491f1260a06be09a1eed3a8581318  vfs-tests.log
cfba89a69d9c58ff44631701afd40d317c8b5efa1f1b7b1695878e56af6655db  acl-tests.log
86cfae2add64e4a71e526b30957df4c3c3b1f7c38215d942ba89a708c4cf6f83  two_uid_probe.rs
24da4b5e1e32e5635be9108f990eb80711d08b06355ab07b2e55a28a89baf517  two-uid-results.json
```

Manifeste machine complet des artefacts : `ARTIFACT_SHA256SUMS.txt`. Les versions et paramètres observés sont dans `docker-version.json`, `environment.log`, `image.txt`, `containers.txt`, `two-uid-container.txt` et `containers-bounded.jsonl`.
