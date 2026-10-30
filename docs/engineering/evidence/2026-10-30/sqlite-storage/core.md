# SQLite core — périmètre logiciel local du 30 octobre 2026

Implémenteur : sous-agent sqlite_core. Ce rapport décrit un correctif de développement et ses régressions locales. Il ne clôture aucune finding formelle de l'audit indépendant et ne qualifie ni une release ni un moteur.

## Contrat et modifications

Fichiers détenus : `src/audit.rs`, nouveau `src/audit/storage.rs`, additions ciblées `src/control_fs.rs`, nouveau `tests/storage_security.rs`, `tests/audit_integrity.rs`, `tests/tenant_storage.rs`. `src/audit/control.rs` et `tests/control_migration.rs` sont le lot du collègue sqlite_backup. Aucun commit, changement de HEAD, suppression de données existantes, opération QA, Docker/Podman, cloud ou PKI.

Le contrat a été envoyé à root avant implémentation et accepté. `StoreStorage::{open,create_new}(&Path)` retourne `(StoreStorage, InitializationGuard)`. `path()` est une provenance, `verify_names()` fait uniquement les contrôles de namespace/métadonnées/ACL, `verify(&Connection)` ajoute `FileControl::HasMoved` et exige succès + false. `sync_closed(&guard)` est réservé à une destination neuve après fermeture de sa connexion SQLite et avant libération du guard.

- Le parent terminal est privé `0700/euid`, tous les ancêtres passent `control_fs` (propriétaires, modes, ACL et filesystem local supporté). Les anciens parents `0755` sont refusés et jamais réparés par Store.
- Main et sidecars préexistants (`-wal`, `-shm`, `-journal`) sont inspectés par `fstatat(AT_SYMLINK_NOFOLLOW)` : régulier, `0600/euid`, lien unique. macOS ajoute `acl_get_link_np` sans ouvrir le fichier ; aucune ACL privée étendue n'est qualifiée. Linux conserve le contrat POSIX ACL dont le masque est reflété par le mode, sous filesystem autorisé par le parent.
- Aucun FD brut main/SHM n'est conservé et aucun fichier main existant n'est ouvert/fermé pour une validation. Les descripteurs de ces inodes appartiennent au VFS SQLite.
- Un verrou flock distinct constant `.ai-guard-sqlite-open.lock`, privé600, sérialise les préparations dans tout le parent, y compris les aliases de casse/normalisation des noms main. Attente bornée à 5 secondes, erreur exploitable. Son type/mode/UID/nlink/ACL sont validés avant un raw open existant.
- Le nouveau main est créé exclusivement600, validé/synchronisé puis fermé AVANT toute ouverture VFS, pendant que le verrou distinct exclut les autres ouvertures coopérantes. Les fichiers préexistants ne sont jamais chmod/chown, remplacés ou effacés.
- Les noms main sont désormais ASCII exclusivement ; les parents Unicode restent autorisés. Les suffixes réservés et le préfixe lock sont refusés sans tenir compte de la casse ASCII. Cette restriction explicite évite les aliases Unicode APFS/HFS+ sans inventer un casefold de filesystem. Root a accepté ce contrat après le cas long-s/Kelvin du reviewer. Une base portant un nom Unicode nécessite une migration offline contrôlée à documenter ; aucun renommage automatique.
- Les contrôles sont appliqués avant SQL, aux frontières de transaction/commit/integrity et aux lectures publiques d'audit. Une violation constatée après ouverture verrouille l'état unhealthy. Le schéma, les événements, les quotas, les claims et les règles v1/v2 ne sont pas modifiés.
- Des sidecars orphelins sont préservés et bloquent la création d'une nouvelle base. Les métadonnées/types/ACL hostiles des sidecars sont refusés avant SQLite. Ce correctif n'est pas un parseur indépendant des contenus WAL/SHM/journal600 valides.

## Diagnostic et RED conservés

L'ancien `AuditFile` gardait un FD raw du main hors VFS. La fermeture de ce FD par un Store abandonné/refusé pouvait relâcher les verrous POSIX d'une autre connexion du même processus. Les contrôles par réouverture main/SHM auraient reproduit cette erreur.

1. `/tmp/ai-guard-storage-security-red.log` : avant correctif, cinq régressions échouent comme attendu (main0644, parent0755, sidecars0644, permissions changées non latchées, namespace réservé). L'ancienne seule sonde `PRAGMA journal_mode=DELETE` passait déjà sur SQLite récent ; elle n'est donc pas présentée comme preuve RED du défaut de FD.
2. Pour rendre la preuve POSIX déterministe, le test enfant a été renforcé par `F_GETLK` sur tout le main, sans acquisition de verrou. Une copie isolée de l'archive intacte `source.tar` a été créée dans `/tmp/ai-guard-sqlite-lock-red.nf9c9o`, avec sa propre target Cargo et la régression courante copiée. Le contrôle avant drop passe ; après drop de StoreA, le child voit `F_UNLCK=2`, ce qui fait échouer le test alors qu'une autre connexion SQLite du parent reste ouverte. Commande :

```sh
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ai-guard-sqlite-lock-red.nf9c9o/target cargo test --locked --manifest-path /tmp/ai-guard-sqlite-lock-red.nf9c9o/Cargo.toml --test storage_security dropping_or_refusing_another_store_preserves_sqlite_connection_locks -- --exact --nocapture
```

Résultat attendu et constaté : FAIL. Log `/tmp/ai-guard-sqlite-lock-red.log`. L'archive `/tmp/ai-guard-editorial-fs-freeze.ZOKZf3` n'a pas été modifiée. Cette caractérisation même PID ne démontre pas un franchissement autre UID.
3. `/tmp/ai-guard-sqlite-unicode-red.log` : avant restriction ASCII, `audit-ſhm` était accepté. La régression demande maintenant un refus avant toute entrée créée. Elle ne prétend pas, à elle seule, avoir observé l'équivalence de noms APFS ni une corruption par alias.

Deux erreurs temporaires de compilation du test `F_UNLCK` (inférence/casts Darwin) et un premier GREEN bloqué par les anciens tempdirs0755 ont été corrigés. L'ancien test de parent symlink permissif a été ajusté au nouveau contrat de refus de tous les liens non OS fixes. Les fixtures v1 et schéma inconnu précréent explicitement leurs nouveaux main600, afin de continuer à tester le schéma plutôt qu'un refus de permissions.

## Vérification finale de ce lot

Hôte constaté : Darwin arm64 ; `rustc 1.99.1 (58cf8f9c3 2026-10-13)`, `cargo 1.99.1 (5f94df478 2026-08-27)`. Tous les appels shell utilisent login=false et le PATH explicite convenu.

| Statut | Commande / preuve | Résultat constaté |
|---|---|---|
| PASS | `CARGO_INCREMENTAL=0 cargo test --locked --test storage_security --test audit_integrity --test tenant_storage` ; `/tmp/ai-guard-storage-security-green.log` | 32 tests : 17 storage + 12 audit_integrity + 3 tenant_storage ; 0 échec, 0 ignoré |
| PASS | `CARGO_INCREMENTAL=0 cargo test --locked --lib unavailable_vfs_identity_control_is_refused` ; `/tmp/ai-guard-sqlite-core-vfs.log` | 1 test, 0 échec ; une connexion sans contrôle HAS_MOVED supporté est refusée |
| PASS | `cargo fmt --all -- --check` ; `/tmp/ai-guard-sqlite-core-fmt.log` | Code0, aucun écart |
| PASS | `cargo clippy --locked --lib --test storage_security --test audit_integrity --test tenant_storage -- -D warnings` ; `/tmp/ai-guard-sqlite-core-clippy.log` | Code0, aucun warning |
| NOT_RUN dans ce sous-lot | Suite globale, all-targets, release | Centralisés par root après adaptation des autres fixtures0755 ; ne pas transférer ce PASS ciblé à l'ensemble du produit |
| NOT_RUN | Linux ACL/moteurs/plateformes réelles ; autre UID réel/mauvais propriétaire | Aucun second UID autorisé ni environnement Linux exécuté par ce sous-lot |
| NOT_APPLICABLE | Frontend et navigateurs pour ce sous-lot de stockage | Aucun frontend modifié par sqlite_core |

Critères couverts : refus main0644 sans réparation ; parent0755/mutable ancestor sans création SQL ; refus préservant main/sidecars FIFO, symlink, hardlink, répertoire ; ACL macOS présentes sur main et chaque sidecar refusées et préservées ; permissions parent/main/WAL/SHM changées après ouverture latchées ; sidecars orphelins conservés ; noms réservés ASCII et aliases Unicode refusés avant création ; parent Unicode accepté ; premier open concurrent 4 threads avec tous les événements ; 3 processus légitimes + parent avec audit cohérent ; drop/refus d'un Store ne relâche pas les verrous POSIX d'une connexion voisine ; lock hardlink vers un autre SQLite actif refusé sans rouvrir cet inode ; timeout d'initialisation borné et base préservée ; tenant binding et schémas v1/v2 historiques conservés.

## Revue, limites et intégration

Le reviewer indépendant sqlite_review a suivi la conception et les sources : suppression des raw FDs, réservation de namespace, lock unique (aliases macOS), precheck du lock avant raw open, API safe HAS_MOVED. Ses propositions de correction ont été appliquées. Sa conclusion finale et l'intégration globale restent sous responsabilité root ; ne pas transformer un état intermédiaire en approbation finale.

La frontière est autre UID sous DAC/ACL/filesystem supportés ; même UID du contrôle, root et administrateurs stockage/moteur restent hors garantie. Les revalidations détectent les remplacements accidentels ; elles ne fournissent aucune atomicité contre ces acteurs. L'API sync_closed suppose que les ouvertures de la destination neuve passent par le guard ; elle n'autorise pas l'ouverture raw d'un main/SHM avec une connexion SQLite active.

La QA existante `/data0755` n'a pas été touchée. L'installation et la préparation offline explicite des permissions, les autres fixtures, les images et les documents produit appartiennent à root. Le produit global et les gates restent non qualifiés.

Mise à jour de gel : un nouveau `cargo fmt --all -- --check` après rédaction a constaté un FAIL uniquement dans `tests/control_cli.rs:legacy` (nouvel OpenOptions en une ligne pendant l'édition concurrente des fixtures par root). Ce fichier n'est pas détenu par sqlite_core et n'a pas été modifié ici. Root est informé. Le contrôle ciblé `rustfmt --edition 2024 --check src/audit.rs src/audit/storage.rs src/control_fs.rs tests/storage_security.rs tests/audit_integrity.rs tests/tenant_storage.rs` est PASS, log `/tmp/ai-guard-sqlite-core-fmt-owned.log`. Le PASS fmt global du tableau est la commande antérieure ; la validation globale doit être rejouée par root après la fin de ses éditions.
