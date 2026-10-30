# Sauvegarde et migration SQLite — lot local du 30 octobre 2026

Périmètre exclusif : `src/audit/control.rs`, uniquement `Store::migrate_control_v2`, et `tests/control_migration.rs`. Le module commun `src/audit/storage.rs`, `src/audit.rs` et `src/control_fs.rs` appartiennent à l'agent sqlite_core ; opérations, image et documentation transverses appartiennent à root. Aucun commit, rollback global, moteur, cloud, NAS ou PKI utilisé. Checkout entièrement non suivi dans Git au démarrage ; comparaison de la zone modifiée avec `source.tar` du gel `/tmp/ai-guard-editorial-fs-freeze.ZOKZf3` : seul `migrate_control_v2` change dans `control.rs`.

## Changements

La destination de sauvegarde utilise `StoreStorage::create_new` et le même garde de namespace que `Store`. Le parent dédié doit être privé 0700, détenu par l'euid, et ses ancêtres doivent satisfaire la politique commune ; les liens, fichiers non réguliers, hardlinks, modes/propriétaires/ACL non conformes et noms réservés sont refusés. Une destination ou un sidecar préexistant est conservé et refusé. Les fichiers existants ne sont ni durcis ni supprimés par la migration.

La connexion destination reçoit `verify(&Connection)` avant/après l'API backup et avant sa fermeture. Pendant l'emprunt mutable de cette connexion par `rusqlite::backup::Backup`, `verify_names()` examine les noms sans rouvrir/fermer de FD main/SHM ; la source est également revalidée. La sauvegarde fraîche est synchronisée après `destination.close()`, pendant que son `InitializationGuard` exclut les autres ouvertures coopérantes. Aucune ABI VFS privée ni chemin magique de FD n'est utilisé.

La vérification du schéma, du tenant, de la chaîne et de la tête sauvegardée, puis `quick_check`, restent avant DDL. Le head source est revérifié dans la transaction de migration. L'échec d'audit annule les tables et le genesis tout en conservant la sauvegarde vérifiée. Une seconde tentative sur une source v2 ne crée ni sauvegarde supplémentaire ni nouveau genesis. Le refus des schémas futurs/partiels est conservé.

Les fixtures créent les répertoires 0700 et les fichiers SQLite 0600 dès création, sans affaiblir le code pour les anciens modes de fixture.

## Preuves exécutées

Environnement : macOS 27.2 ARM64, `rustc 1.99.1 (58cf8f9c3 2026-10-13)` ; tous les shells `login:false`, PATH explicite `/Users/louis/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin`.

| État | Commande / résultat constaté |
|---|---|
| PASS baseline | `cargo test --locked --test control_migration` avant édition : 4 PASS, 0 FAIL, 0 ignoré. |
| FAIL attendu RED | `cargo test --locked --test control_migration backup_` avant implémentation : 3 FAIL attendus (parent 0755, ancêtre mutable 0777 et alias symlink acceptés par l'ancienne migration), 2 PASS, 0 ignoré. |
| FAIL fixture intermédiaire | Première intégration du garde commun : 11 FAIL au setup, parce que `tempfile::tempdir()` crée 0755 ici. Corrigé par `Builder.permissions(0700)` dès création ; aucune exception du produit ajoutée. |
| PASS ciblé | Dernier `cargo test --locked --test control_migration` : 11 PASS, 0 FAIL, 0 ignoré ; log `/tmp/ai-guard-sqlite-backup-tests.log`, sortie 0, durée tests 0.32 s. |
| PASS format | `rustfmt --check --edition 2024 tests/control_migration.rs src/audit/control.rs` puis `cargo fmt --all -- --check` : sorties vides, code 0. |
| FAIL compilation transverse intermédiaire | `CARGO_INCREMENTAL=0 cargo test --locked` : E0282 dans `tests/storage_security.rs:125`, cast `libc::F_UNLCK as _`. Fichier possédé par sqlite_core, défaut transmis pour correction. Suite non considérée réussie. |
| NOT_RUN par cet agent après correction | sqlite_core a corrigé le cast en `libc::c_short` et annoncé ses suites ciblées PASS ; la suite complète et Clippy consolidés sont confiés à root pendant l'adaptation des autres fixtures. Le FAIL intermédiaire ci-dessus n'est pas présenté comme le résultat final du lot global. |

Les 11 tests ciblés comprennent : migration/réouverture source v2 et backup v1 avec événement/hash historique conservé ; refus de répétition ; rollback DDL après audit plein et réouverture des deux bases ; refus de schémas futurs/partiels ; parents 0755/0770/0777 sans réparation ; parent privé sous ancêtre 0777 ; alias symlink ; ACL macOS réellement appliquée sur parent privé et conservée ; destination identique à la source ; six types préexistants (régulier 0600, régulier 0644, répertoire, symlink, hardlink, FIFO) pour le main puis chacun des sidecars `-wal`, `-shm`, `-journal`. Les entrées refusées conservent identité, propriétaire, mode, liens et contenu/cible applicables. L'absence de création main et l'état v1/hash source sont contrôlés après refus.

## Critères et limites

- PASS logiciel macOS local : consommation du garde commun, sauvegarde neuve privée, refus et préservation des entrées préexistantes, sauvegarde logique vérifiée avant DDL, source et backup réouvrables, rollback DDL et non-répétition du genesis.
- Revue indépendante source : sqlite_review a relu `migrate_control_v2` et les tests ; aucun écart bloquant trouvé. Log final ciblé et hashes transmis pour son bilan consolidé.
- NOT_RUN ici : test déterministe de divergence pendant backup, écriture concurrente hostile avec second UID, qualification Linux/POSIX ACL, moteur réel Docker/Podman, crash matériel/stockage et coupure d'alimentation. L'échec sans altération source est démontré par audit plein ; cela ne remplace pas une qualification crash/disque.
- La sûreté contre un autre UID repose sur la chaîne privée commune. Les processus hostiles du même UID, root et administrateurs de stockage/moteur restent hors de cette frontière. Le lock d'initialisation ne coordonne que les ouvertures du produit ; il ne prétend pas contrôler un outil SQLite brut lancé volontairement par le même compte.
- G06 et les autres gates de qualification ne sont pas fermés par ces seuls tests. Les hypothèses de l'audit ne deviennent pas des vulnérabilités confirmées par ce correctif.
- Les skills projet `ai-guard-verify` et `ai-guard-security-review` sont absents de `.agents/skills` et `.claude/skills` ; leur chargement automatique n'est pas confirmé et aucun usage n'est revendiqué. Les instructions AGENTS/contrats/règles/tests/gates, le delta d'audit et les skills génériques systematic-debugging, TDD et verification-before-completion ont été lus.

Empreintes au dernier ciblé PASS :

```text
d034aafd0a376a6e3de05472b3430e53986e8b926742b3d1b9297dd3d2fba722  src/audit/control.rs
fd720c786d51edb64aaae5c38da2b4917d7d135409ce41287da4e081e75444cf  tests/control_migration.rs
15423caadb73a41b8c9af668fcf0902b401437112101f8b4c4e80a175d8c5c23  /tmp/ai-guard-sqlite-backup-tests.log
```
