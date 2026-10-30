# Qualification Linux ARM64 ciblée — broker journal

Date: 2026-10-30. Source finale fournie/gélée par le root après ses validations fmt/Clippy; ce rapport ne prétend pas avoir exécuté le reste de la matrice.

## Source et dépendance

Archive finale conservée dans le contexte privé `/tmp/ai-guard-broker-journal-linux.f2XItm/evidence/source.tar`.

- Archive SHA-256: `bc613e718b23bcdaac49233861a4bf13e93352b804ef2ce8150e1096c6204e14` (égale à l'archive remise).
- Manifeste `source.sha256` SHA-256: `cd9775b29d3fa75a67b08ddd395a081e980665c95369fcb2c84f5a326302d520`.
- 111 fichiers extraits; 111/111 empreintes vérifiées, sortie dans `evidence/source-manifest-verification.txt`.
- `serde_ignored` 0.1.14 : archive cache crates.io SHA-256 `115dffd5f3853e06e746965a20dcbae6ee747ae30b543d91b0e089668bb07798`, identique au checksum Cargo.lock; entrée sparse index publique SHA-256 `738f016a48f6ef0e2f98d1ba543db3d0f16d3a5dd59e4b8753e1c2960da8acee`. Les deux sont copiées dans l'image et Cargo les a compilées offline.

## Résolution de la base et build

Le serveur Docker est Linux ARM64 dans Docker Desktop sur hôte Apple ARM64. Version client/serveur constatée : `29.2.1|29.9.2|linux|arm64` (`docker-version-final.txt`). Image de base attendue : ID `sha256:8e48e6c9b949d58cef12237644a3b13e2ef8448666cf2d6b732473f2a96072aa`, `linux/arm64`, UID `1000:1000`, Rust/Cargo 1.99.1, `CARGO_HOME=/home/fsqa/.cargo`.

Le premier Dockerfile `FROM sha256:<image-id>` a échoué avant compilation : BuildKit l'a parsé comme `docker.io/library/sha256:<...>` et a tenté de résoudre les métadonnées distantes; accès refusé, aucune image ni couche tirée. La sortie est conservée dans `build.log`. La forme brute d'un ID n'est donc pas une référence FROM locale valide ici.

Un tag local propre, suffixé de l'ID complet, a été créé et vérifié par `docker image inspect` avant build :
`agentsandbox-broker-clock-linux:base-8e48e6c9b949d58cef12237644a3b13e2ef8448666cf2d6b732473f2a96072aa` → l'ID attendu et `linux/arm64`. Le Dockerfile utilise ce tag avec `docker build --pull=false --network=none --platform=linux/arm64`. Aucun accès réseau n'a eu lieu dans le build réussi.

Build no-run : `PASS`, image `ai-guard-broker-journal-linux:qa-20261030`, ID `sha256:bd409e1b1126ec4a0c5313b63f5829fc2d96037789b4481fc8e4e5ab61df4821`, plateforme `linux/arm64`, user image `1000:1000`. `cargo test --locked --offline --no-run --lib --test broker_journal --test journal_sqlite` a compilé les trois cibles; le log indique 19.83 s pour la compilation Cargo (`build-pinned-tag.log`). Le build n'avait pas de limites CPU/mémoire configurées; seules les limites du conteneur de test ci-dessous sont revendiquées.

## Conteneur de tests

Conteneur conservé, arrêté, sans `--rm`: ID `dfa958c76755bc7e5e43b023b30f0cc48439d481e1eeaeef674f35ec8ff0fd1d`; image ID ci-dessus. État final `exited`, code retour `0`. Le conteneur a tourné du `2026-10-30T20:17:08.823626171Z` au `2026-10-30T20:17:10.043390380Z` (1.219764209 s). `container-inspect-before.txt` et `container-inspect-final.txt` enregistrent la configuration avant/après.

Configuration inspectée avant et après : user `1000:1000`; réseau `none`; cap-drop `ALL`; `no-new-privileges:true`; privileged `false`; `NanoCpus=2000000000` (2 CPU); mémoire `2147483648` (2 GiB); `PidsLimit=256`; mounts `[]`; binds `null`; devices `[]`. Aucune source n'a été montée depuis l'hôte; aucune socket, credential, device ou secret injecté. La sortie a été collectée après arrêt dans `container.log`.

Environnement observé dans le conteneur :

```text
Linux dfa958c76755 7.0.14-linuxkit #1 SMP PREEMPT Fri Oct 16 11:55:50 UTC 2026 aarch64 GNU/Linux
rustc 1.99.1 (58cf8f9c3 2026-10-13)
cargo 1.99.1 (5f94df478 2026-08-27)
```

La configuration de ressources est prouvée par inspect; la consommation/les pics réels n'ont pas été mesurés.

## Résultats — périmètre ciblé seulement

Commande conteneur : `uname -a; rustc --version; cargo --version; cargo test --locked --offline --lib broker::journal::tests; cargo test --locked --offline --test broker_journal; cargo test --locked --offline --test journal_sqlite`.

- `broker::journal::tests` — **PASS** : 4 tests passés, 2 helpers ignorés, 83 filtrés. Le test parent a lancé `writer_lock_race_child` comme sous-processus; il n'est pas compté comme un PASS produit séparé. `crash_child_at_stage` reste ignoré sauf lorsqu'un parent le lance avec `--ignored`.
- `broker_journal` — **PASS** : 10 passés, 0 échec.
- `journal_sqlite` — **PASS** : 1 passé, 0 échec.
- Code retour conteneur : `0`.

Aucune suite complète, autre cible d'intégration, test E2E, modèle, API fournisseur ou runtime worker n'a été exécuté. Les tests unitaires qui injectent des défauts d'écriture/SIGKILL ne prouvent pas une coupure électrique.

## Limites

Cette preuve couvre ces trois groupes sur Linux ARM64 virtualisé dans Docker Desktop, sous la configuration inspectée. Elle ne qualifie pas un hôte Linux physique, macOS comme noyau, Podman, Kubernetes, cloud, les autres gates de production, ni la sûreté universelle contre une base SQLite inconnue avec `nlink=1`. Une compilation sans run ou ces tests ciblés ne constituent pas une qualification de release.

## Artefacts privés conservés

Contexte privé mode 0700 : `/tmp/ai-guard-broker-journal-linux.f2XItm`; tous ses fichiers mode 0600 et répertoires mode 0700. Y sont conservés `Dockerfile`, `.dockerignore`, source archive/manifeste, leur vérification, logs du build initial échoué et du build réussi, logs tests, inspect avant/après, inspect image/base et versions Docker. Image et conteneur arrêté restent présents dans Docker. Aucun commit/push.

## Contrôle final du contenu copié — sans redémarrage ni test

À la demande du root, le source de l'image a été copié depuis le conteneur arrêté `dfa958c76755bc7e5e43b023b30f0cc48439d481e1eeaeef674f35ec8ff0fd1d` avec :

```text
docker cp ai-guard-broker-journal-linux-qa:/build/journal-source/. /tmp/ai-guard-broker-journal-linux.f2XItm/extracted-from-container/
```

Le chemin réel est `/build/journal-source` selon le Dockerfile; `/build/source` appartient à l'image de référence antérieure et n'a pas servi à ce test. Copie vers un nouveau dossier 0700, fichiers remis en 0600 et répertoires en 0700. Vérification contre le manifeste final : 111 lignes, 111 fichiers présents, **111/111 SHA-256 conformes**. Résultat enregistré dans `/tmp/ai-guard-broker-journal-linux.f2XItm/evidence/container-source-manifest-verification.txt`, log SHA-256 `3f55fb5d878d186eeea125e592fa6eb3e9ad656526b5e9a7ee45fc82eda4eb78`. `diff -qr source extracted-from-container` est vide (contenu identique).

La vérification des 111 chemins depuis le worktree courant a aussi retourné RC 0 et le même log SHA (`3f55fb5d...`). Donc les fichiers applicatifs et tests retenus dans l'archive sont identiques entre archive, image copiée et worktree courant. Le manifeste ne couvre pas les documents engineering hors archive.

État Git courant relevé sans mutation : HEAD `3372150fdb99d8bc71700a33dd1d49c0e75c45b5`; aucun fichier staged. Modifiés : `Cargo.lock`, `Cargo.toml`, `docs/engineering/DECISIONS.md`, `docs/engineering/IMPLEMENTATION_PLAN.md`, `src/broker/mod.rs`, `src/control_fs.rs`, `src/provider.rs`, `src/runner.rs`, `src/runtime/managed.rs`. Untracked : `docs/engineering/BROKER_JOURNAL.md`, `docs/engineering/evidence/2026-10-30/broker-journal/`, `src/broker/journal.rs`, `src/broker/journal/`, `src/control_fs/`, `tests/broker_journal.rs`, `tests/journal_sqlite.rs`. Ces deltas sont les présents dans le worktree observé; le check SHA confirme que les chemins Rust/test inclus dans l'archive correspondent à l'image. Les changements docs/evidence engineering ne sont pas couverts par les 111 hashes.

Après la copie, le conteneur demeure `exited`, code 0; image et conteneur conservés. Aucun build, démarrage de conteneur, test ou modification de dépôt pendant ce contrôle.
