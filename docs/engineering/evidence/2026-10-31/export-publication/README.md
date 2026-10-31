# Publication d'export — intégrité et barrières de durabilité

31 octobre 2026, macOS 27.2 / Darwin 27.2.0 ARM64, Rust 1.99.1 (`58cf8f9c3`). Base de développement `858f50e0ce21a807d9ae72e7f5af80c4684cd5b8`, branche isolée `codex/broker-service`. **PASS logiciel ciblé, revues traitées.** Aucun gate de production clôturé ; G09 conserve son FAIL historique.

## Défauts et référence RED

Le chemin legacy et le parent d'export autorisé publiaient leurs fichiers après `flush`/mode/rename, sans attendre la synchronisation des fichiers, répertoires et parent. L'allowlist et l'identité FD ne refusaient pas non plus un parent ou ancêtre modifiable par un autre UID. [Diagnostic de durabilité](durability-diagnosis.md), [revue initiale du namespace](parent-review-before.md).

Deux exports réels d'une archive synthétique vérifient les contenus, modes et identités des fichiers, puis exigent les barrières avant le succès. Les cinq refus de permissions vérifient de vrais changements de modes sur des répertoires temporaires possédés par les tests, sans réparation ni création de staging. Ils ne reproduisent pas une attaque avec un second UID.

| Commande ciblée | Résultat constaté avant correction |
|---|---|
| `cargo test --locked --lib workspace::export_durability_tests -- --nocapture` | **FAIL attendu** : les deux exports retournent Ok sans les barrières requises ; [journal](red-ordering.log). |
| `cargo test --locked --lib workspace::authorized::tests::authorized_export_ -- --nocapture` | **NOT_RUN** : filtre fourni erroné, zéro test sélectionné ; [journal conservé](red-empty-filter.log). Ce code de sortie zéro n'est pas un PASS. |
| `cargo test --locked --lib workspace::authorized::authorized_workspace_tests::authorized_export_ -- --nocapture` | **FAIL attendu** : cinq refus manquants ; cinq contrôles positifs/existants PASS ; [journal](red-parent-permissions.log). |

Le [manifeste RED](red-source.sha256) décrit 150 vrais fichiers Cargo/toolchain/src/tests, sans s'inclure. Snapshot, checkout avant/après et extraction d'archive : 150/150 vérifiés par Luna. Archive privée `/private/tmp/ai-guard-export-red-proof.fSn8SG/sources.tar.gz`, SHA-256 `699c9c9964cdce1fee768bed50e9a414d8162ca9540698fdc15dd72e991390fd`. Le diff historique omet le nouveau module non suivi ; l'archive contient celui-ci. [Empreintes des captures privées](red-private-evidence.sha256).

## GREEN et revue

Luna a exécuté séquentiellement les commandes suivantes sur le même gel, sans autre suite ni moteur/modèle/frontend. `CARGO_TARGET_DIR` désigne le cache existant `.worktrees/broker-supervision/target` ; les sources sont celles de `broker-service`.

| Commande | Résultat constaté |
|---|---|
| `cargo test --locked --lib workspace::export_durability_tests -- --nocapture` | **PASS 5/5**, 0,11 s : les deux exports, fautes avant/après publication, limites des répertoires/profondeur ; [journal](green-ordering-faults.log). |
| `cargo test --locked --lib workspace::authorized::authorized_workspace_tests -- --nocapture` | **PASS 11/11**, 0,04 s : sept nouveaux cas de droits et quatre canaris source/export ; [journal](green-workspace.log). |
| `cargo test --locked --test broker_enforcer public_client_bootstraps_real_enforcer_from_a_fresh_root_and_shuts_it_down -- --exact --nocapture` | **PASS 1/1**, 1,55 s : vrai bootstrap public dans une racine neuve, métadonnées privées sans lire la capability, état prêt, arrêt, refus du socket et libération de la lease avant nettoyage ; [journal](green-bootstrap.log). Le cas SIGSTOP existant est filtré. |
| `cargo fmt --all -- --check` | **PASS**, code 0 ; [journal vide](fmt.log). |
| `cargo clippy --locked --all-targets -- -D warnings` | **PASS**, aucun diagnostic ; [journal](clippy.log). |
| `cargo build --release --locked` | **PASS**, 1 min 14 s ; [journal](build-release.log). |

17 fonctions ciblées distinctes sur ce gel ; les répétitions RED/GREEN et le premier essai bootstrap ne sont pas additionnés. [Revue indépendante globale](independent-review.md) et [revue croisée de l'ascendance](ancestry-review.md) : aucun défaut bloquant P0/P1 établi dans le périmètre gelé, avec limites explicites. Les auteurs de ces revues ne revendiquent pas les exécutions de Luna.

Contrôle documentaire : 377 cibles de liens locaux dans 13 Markdown, aucune manquante sur ce checkout ; ce contrôle ne valide ni les ancres, ni les réponses HTTP, ni la portabilité des chemins absolus des captures de revue. Contrôle avant push sur33 fichiers indexés : aucun binaire, symlink, chemin interdit, pattern suspect ou clé connue ; aucun chunk de scanner en erreur. Whitespace vérifié sur sources et documentation éditées ; les journaux bruts conservent leurs lignes finales. Le checkout principal et ses éditions du mainteneur sont préservés.

Le [manifeste GREEN](green-source.sha256), SHA-256 `3770a90e12c423b6ad51c970d50b4d285ef776ad937e803964fade60c63e8905`, décrit 150 fichiers. Snapshot et checkout vérifiés avant/après ; root a revérifié le checkout et l'archive extraite : [150/150](verify-checkout.log), [150/150](verify-archive.log). Archive privée `/private/tmp/ai-guard-export-green.LxgMHx/source-archive.tar.gz`, SHA-256 `1280fa17c11e5f78f8c6960df1c8ea363bbd2dd78907e89b9676caed72f4a027`. Copies natives privées hors cache dans `/private/tmp/ai-guard-export-final-proof.jCRSzI/bin/`, [empreintes](binaries.sha256). `agentsandbox` : `2753bb10cce7ad13b885a343d524804fa62f0ecd121a87f11a18bcf78e7c49c5`. Aucune promesse de compilation byte-identique. Une première recherche d'artefact dans `broker-service/target` était incorrecte ; le fichier réel et les empreintes proviennent bien du cache déclaré, sans refaire le build.

## Contrat et limites

Le correctif attend les données et modes finaux des fichiers, les répertoires créés en postordre et la racine de staging, puis le parent après renommage exclusif. Les capacités du parent et de ses ancêtres restent retenues ; leur politique UID/modes/ACL est revalidée. Après renommage, toute erreur conserve la destination et exige réconciliation. L'outbox de succès ne naît qu'après retour Ok de l'extraction ; aucune transaction globale filesystem/journal/HTTP n'est revendiquée.

Les répertoires explicites et implicites sont bornés séparément à `max_files`, les chemins d'archive à 64 composants. Scanner et extracteurs appliquent le même précontrôle avant la première création de chaque entrée. L'extension de cette limite aux parcours de collecte/import initiaux reste ouverte : un ancien volume plus profond peut être refusé au scan/export. Aucun contenu existant n'est supprimé pour contourner cette limite.

Les hooks sont limités à `cfg(test)`, après les opérations réelles. Une faute de contrôle après une barrière ne reproduit ni EIO/ENOSPC retourné par le disque, ni panne électrique. Les tests ne qualifient pas les ancêtres externes récemment créés et non synchronisés, un adversaire root/même UID, Linux/ACL ou un second UID macOS. La matrice de stockage et de runtimes reste ouverte.

**NOT_RUN :** vraie perte électrique, erreur matérielle de synchronisation, adversaire sous un autre UID sur ce gel, Docker/Podman/Kubernetes, service complet et workers actifs après crash, SSO/Keycloak. Le bootstrap est un processus natif vide, sans worker : il ne clôture pas le FAIL G09. La prochaine priorité reste la convergence des annulations et la verticale réelle du broker avec deadline après perte du client/broker. Le travail total d'énumération des sources, incluant les entrées exclues, reste un écart distinct.

Références primaires : [contrat Rust File::sync_all](https://doc.rust-lang.org/stable/std/fs/struct.File.html#method.sync_all), [implémentation Unix Rust](https://doc.rust-lang.org/stable/src/std/sys/fs/unix.rs.html#1264-1275), [fsync Linux et parent](https://man7.org/linux/man-pages/man2/fsync.2.html), [limites matérielles Apple](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fsync.2.html). La version Rust examinée utilise `F_FULLFSYNC` sur Apple ; cela ne remplace pas une qualification du stockage effectif.
