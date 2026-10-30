# Revue indépendante SQLite — ai-guard, 30 octobre 2026

État : **revue ciblée source et logs achevée pour les empreintes ci-dessous ; aucun écart bloquant identifié dans ce lot**. Périmètre : ouverture du Store/audit, politique filesystem commune, sauvegarde de migration, comportement du SQLite/rusqlite en cache, puis adaptations de fixtures et documentation d’intégration décrites plus bas. La qualification runtime et l’exécution de la maintenance opérateur restent distinctes de cette revue. Ce document ne remplace ni ne modifie l’audit antérieur : ses **19 `needs_validation`, zéro vulnérabilité confirmée et absence de sévérité formelle** restent inchangés.

## Méthode et limites

Guidance lue : `/Users/louis/.agents/skills/security-audit/SKILL.md`, mode revue ciblée. Politique du dépôt, contrats d’architecture/code/tests/gates et `/tmp/ai-guard-audit-filesystem-delta.md` lus. Les skills projet `ai-guard-security-review` sont absents de `.agents/` et `.claude/` dans ce checkout ; leur chargement automatique n’est pas confirmé.

Lectures et recherches de sources uniquement ; aucun build, test, binaire cible, probe, moteur, base ou sidecar n’a été exécuté ou modifié par le reviewer. Aucun secret, service cloud ou PKI consulté. Le fichier du présent rapport est la seule écriture autorisée de cette revue. Le checkout est entièrement non suivi : aucun commit ne peut identifier le delta. Les empreintes des fichiers gelés seront ajoutées à la revue finale.

Les tests de développement exécutés par les implémenteurs sont décrits comme tels à partir de leurs logs. Ils ne constituent pas une validation formelle des hypothèses du rapport d’origine, notamment avec un second UID hostile et des ACL/montages réels.

## Constats transmis pendant la conception

1. **FD principal étranger au VFS.** La baseline `src/audit.rs:60–114` conserve un `File` brut du main en plus de la connexion SQLite. Un second Store du même PID peut ouvrir le même inode puis fermer son `File` lors d’un abandon, d’une erreur ou de sa destruction, tandis qu’une autre connexion travaille. La suppression du FD brut principal est donc nécessaire ; déplacer ce FD dans `PrivateFile` ne résout rien, puisque sa revalidation rouvre aussi le nom. Même vigilance pour SHM. Conséquence conditionnelle : perdre les verrous noyau sans que SQLite en soit informé. Régression proposée : une transaction du premier Store reste active ; ouverture/échec/destruction d’un second Store dans le même PID ; un troisième processus doit toujours constater les verrous attendus. Ne pas conclure depuis la seule absence de corruption visible.
2. **Collision de namespace du nouveau verrou proposé.** Avec un verrou `.NAME.storage-lock`, une base appelée `.foo.storage-lock` devient elle-même l’inode ouvert comme verrou du Store `foo`. Les noms `foo-wal`, `foo-shm`, `foo-journal` peuvent de même entrer en collision avec les sidecars de `foo`. Réserver et refuser ces noms avant toute mutation ; sur un filesystem insensible à la casse, tenir compte aussi de `foo-SHM`. Régression proposée : chaque nom réservé est refusé et les fichiers existants gardent identité/mode/contenu.
3. **Deux alias d’un nouveau main et deux verrous différents.** Un verrou dérivé du basename ne sérialise pas nécessairement `Foo.db` et `foo.db` sur un volume insensible à la casse, ni des représentations Unicode équivalentes. Un créateur peut encore fermer son FD initial après l’ouverture SQLite du même inode par une autre API coopérante. Un verrou unique constant par parent privé évite cette dépendance au modèle de normalisation du filesystem. Régression proposée : contrôle du verrou commun et test d’alias sur un filesystem dont la sémantique est établie ; déclarer `NOT_APPLICABLE` motivé sur un filesystem réellement sensible à la casse, plutôt que prétendre l’avoir testé.
4. **Verrou préexistant lui-même non conforme.** Valider type/propriétaire/liens/ACL du verrou existant par métadonnées avant tout raw-open. Un verrou hardlink vers main ou SHM, ouvert puis refusé après `fstat`, provoquerait précisément la fermeture dangereuse recherchée. Un changement concurrent volontaire du même UID reste hors frontière annoncée. Régression proposée : lock hardlink préexistant refusé avant open, sans perturber une connexion active.

Ces points sont des observations/recommandations de conception, sans sévérité formelle. Ils ont été envoyés à `/root` et/ou `/root/sqlite_core` avant intégration. La lecture finale du nouveau code constate leur traitement : verrou unique `.ai-guard-sqlite-open.lock` par parent, préfixe réservé et suffixes de sidecars refusés sans distinction ASCII de casse, prévalidation du verrou sans raw-open, absence de FD brut main/SHM dans `StoreStorage`.

**Complément Unicode avant gel.** Le seul `to_ascii_lowercase` ne couvre pas une politique de noms arbitraires sur tous les filesystems admis. Apple décrit APFS comme normalisant et comparant la casse selon Unicode 9 ; son ancien HFS+ ignore aussi certains caractères de formatage. [Apple APFS FAQ](https://developer.apple.com/library/archive/documentation/FileManagement/Conceptual/APFS_Guide/FAQ/FAQ.html), [Apple TN1150](https://developer.apple.com/library/archive/technotes/tn/tn1150.html). La table primaire Unicode 9 contient U+017F→`s` et U+212A→`k` : `.ai-guard-ſqlite-open.locK` donne un cas concret à distinguer du préfixe byte ASCII ; `audit-ſhm` est analogue pour le suffixe. [Unicode 9 CaseFolding, unicode-org/icu](https://raw.githubusercontent.com/unicode-org/icu/release-58-2/icu4c/source/data/unidata/CaseFolding.txt). L’alias effectif sur le volume APFS de développement n’a pas été observé par le reviewer : c’est une inférence de conception, pas un nouvel impact confirmé.

Root a accepté un contrat plus petit : **basenames des main/backup limités à ASCII**, avec parents Unicode toujours admis, refus avant création de Directory/lock et aucune migration automatique. La correction et les régressions correspondantes ont été relues : `audit-ſhm`, le nom du verrou avec long-s/Kelvin, et un basename accentué sont refusés sans aucune entrée créée ; le parent accentué avec main ASCII fonctionne. Cette politique évite de maintenir un émulateur incomplet des comparaisons de noms propres au filesystem. La documentation et la procédure offline des anciens noms restent sous responsabilité root.

## Deuxième regard sauvegarde après intégration

`src/audit/control.rs` et `tests/control_migration.rs` relus après intégration. Aucun écart bloquant identifié dans ce consumer : `StoreStorage::create_new` est partagé avec l’ouverture normale ; les sidecars sont examinés avant création du main ; `verify_names()` s’applique pendant l’emprunt mutable par Backup ; `HAS_MOVED` est demandé avant/après ; la connexion destination est fermée avant le raw-open de synchronisation, pendant que le verrou commun empêche une autre ouverture coopérante. Les vérifications de tête, schéma, tenant, chaîne et `quick_check` restent avant DDL. Une nouvelle transaction compare la tête source sauvegardée avant migration.

Log lu, sans rejeu : `/tmp/ai-guard-sqlite-backup-tests.log`, **11 PASS, 0 FAIL, 0 ignoré**, dernier ciblé `cargo test --locked --test control_migration` (PATH explicite et shell `login:false` consignés dans `/tmp/ai-guard-sqlite-backup-report.md`). Le log contient le résultat de Cargo, pas un scénario moteur ou multi-UID. Le test d’ACL applique réellement une ACL macOS à un parent de fixture et vérifie son refus/préservation ; il ne teste pas la lecture par un second UID.

Les empreintes source/test constatées par cette revue correspondent à celles du rapport implémenteur :

```text
d034aafd0a376a6e3de05472b3430e53986e8b926742b3d1b9297dd3d2fba722  src/audit/control.rs
fd720c786d51edb64aaae5c38da2b4917d7d135409ce41287da4e081e75444cf  tests/control_migration.rs
```

La divergence déterministe pendant la copie, le crash/disque, Linux/POSIX ACL et les moteurs réels restent `NOT_RUN` dans cette preuve. Aucune perte ou compromission n’est déduite du seul double-open historique.

## Base factuelle SQLite

Source locale effectivement consultée : `rusqlite 0.41.1`, `libsqlite3-sys 0.39.1`, SQLite `3.54.1`, déjà dans le cache Cargo. Aucun téléchargement de dépendance ni compilation.

La documentation officielle avertit qu’un `close()` extérieur à SQLite peut annuler les verrous POSIX du processus ; les défenses WAL récentes ne suppriment pas cette contrainte. Elle décrit également le danger des noms multiples et de la séparation du main et de son journal. [SQLite, How To Corrupt](https://www.sqlite.org/howtocorrupt.html).

`HAS_MOVED` fournit une observation du déplacement, et non l’adoption atomique d’un FD applicatif. Dans le VFS Unix en cache (`sqlite3.c:42277`), la comparaison porte sur `st_ino`, sans comparaison de `st_dev` à cet endroit ; l’application doit conserver sa propre identité et la garantie du namespace privé. La branche `databaseIsUnmoved` (`:63152`) tolère historiquement `SQLITE_NOTFOUND` ; le nouveau contrat applicatif ne doit pas reprendre ce repli. `rusqlite::Connection::file_control` propage bien l’échec. [Contrat officiel des file-controls](https://www.sqlite.org/c3ref/c_fcntl_begin_atomic_write.html).

Le WAL fait partie de l’état durable et doit être conservé pour la récupération. Les noms SHM sont dérivés du main ; `unixOpenSharedMemory` en cache (`:45606`) ouvre SHM avec `O_NOFOLLOW` et les permissions du main (`:45687`). `findCreateFileMode` (`:47100`) dérive celles du WAL/journal du main ; le mode par défaut d’un nouveau main SQLite est toutefois `0644` (`:40820`). D’où la création exclusive `0600` proposée, fermée avant toute ouverture SQLite coopérante du nouvel inode, sous verrou d’initialisation. [SQLite WAL](https://www.sqlite.org/wal.html).

## Vérification du code gelé

- Parent terminal `0700` euid ; ancêtres non remplaçables par un autre UID, exception sticky root bornée ; refus avant SQL des parents/entrées/sidecars non conformes, sans chmod silencieux.
- ACL Darwin et mounts `noowners` : inspection des parents et noms admis, erreur d’inspection refusée. La politique Linux fondée sur le masque POSIX et les types de filesystem admis doit rester décrite comme politique de code, sans qualification runtime implicite.
- Métadonnées main/sidecars sans raw-open main/SHM ; identité dev/inode ; `HAS_MOVED` requis et faux avant initialisation/migration et aux frontières de mutation.
- Sauvegarde : même helper, destination neuve uniquement, sidecars préexistants refusés et préservés ; validation logique, fermeture SQLite puis synchronisation sous le verrou encore retenu ; aucune suppression de résidu à l’échec.
- Audit et claims conservés ; détection d’erreur latchée et aucune autorisation de lancement après échec de persistance ; historique/quota/schéma non réparés arbitrairement.
- Les lectures `head/events/entities/overview` passent désormais aussi par la revalidation et latchent l’échec ; les lectures de diagnostic d’une chaîne déjà connue corrompue restent possibles, sans réautoriser une écriture.
- Deux connexions, deux processus, WAL actif, recovery après fin brutale, checkpoints et backup restent des essais différents ; une suite logicielle ne qualifie pas Docker/Podman ou les ACL réelles.

Les invariants ci-dessus ont été tracés dans `src/audit/storage.rs`, les additions de `src/control_fs.rs`, l’ouverture/transaction/commit/integrity et les lectures publiques de `src/audit.rs`, puis le consumer backup. `FileControl::HasMoved` utilise maintenant l’API sûre de rusqlite. Aucun repli sur contrôle non supporté n’est admis. Les contrôles Darwin `noowners`/filesystem sont présents mais leur test lit une politique sur valeurs factices ; il ne constitue pas l’observation d’un volume monté `noowners`.

`HAS_MOVED` ne devient pas une garantie contre un acteur pouvant renommer à volonté : la barrière reste DAC/ACL/chaîne privée. Main et sidecars `0600` ne sont pas validés par un parseur de format indépendant. Les ouvertures arbitraires par le même compte avec un autre outil SQLite ne sont pas coordonnées par le lock du produit et demeurent hors de la garantie annoncée.

## Logs de développement examinés

| Statut | Preuve lue sans rejeu | Résultat et limite |
|---|---|---|
| FAIL attendu RED | `/tmp/ai-guard-sqlite-lock-red.log`, copie `/tmp/ai-guard-sqlite-lock-red.nf9c9o` dont l’ancien `AuditFile` a été relu | Probe enfant initial PASS, puis `F_GETLK` renvoie `F_UNLCK=2` après destruction d’un second Store alors qu’une connexion voisine reste ouverte. Échec de régression déterministe ; aucune corruption démontrée. |
| FAIL attendu RED | `/tmp/ai-guard-sqlite-unicode-red.log` | `audit-ſhm` était accepté avant restriction ASCII ; preuve de politique, pas preuve d’alias APFS effectif. |
| PASS ciblé | `/tmp/ai-guard-storage-security-green.log` | 17 `storage_security` + 12 `audit_integrity` + 3 `tenant_storage` = **32 tests de niveau supérieur**, 0 FAIL, 0 ignoré. Les sous-processus de probe ne sont pas recomptés comme tests supplémentaires. |
| PASS ciblé | `/tmp/ai-guard-sqlite-core-vfs.log` | **1 test**, contrôle VFS indisponible refusé avant mutation du main. |
| PASS ciblé | `/tmp/ai-guard-sqlite-backup-tests.log` | **11 tests**, 0 FAIL, 0 ignoré ; consumer/test identifiés par les empreintes ci-dessus. Le module commun a encore reçu le refus des noms Unicode après ce log ; la réserve a ensuite été levée par le run global d’intégration ci-dessous. |
| PASS ciblé | `/tmp/ai-guard-sqlite-core-clippy.log` | Clippy strict sur lib + trois cibles du lot core, terminé sans warning ; commande exacte dans le rapport core. |
| NOT_RUN par le reviewer | fmt global, clippy all-targets, suite globale, release, moteurs | Le reviewer ne rejoue aucune de ces commandes ; les logs globaux ensuite transmis par root sont examinés séparément ci-dessous. |

Le test de hardlink du verrou vise bien **un autre main actif** que le candidat ouvert après corruption du lock : la prévalidation du main candidat ne peut donc pas masquer l’absence de prévalidation du verrou. Les probes `F_GETLK` avant/après refus sont pertinentes. Les tests supplémentaires couvrent quatre premières ouvertures en threads, trois processus légitimes, sidecars orphelins, ACL de fixtures macOS, timeout du lock et maintien du latch après restauration des modes. Ils ne constituent pas une recette à deux UID hostiles ni une qualification de crash/recovery/checkpoint.

Empreintes finales recoupées avec `/tmp/ai-guard-sqlite-core-sha256.txt` et le rapport backup :

```text
09b2dda3efcd01775fba3691d682ac340e10f1664be8d11f571e1b360ffdae07  src/audit.rs
3350e37ed4be5b0f8c33648f8adf744b5686e351db00fd047c45b07b0698d1d6  src/audit/storage.rs
861109aaaf495c4751482e321dee2c9e9cf5bf3b0d7ce9eec3b4a4d75b733627  src/control_fs.rs
d034aafd0a376a6e3de05472b3430e53986e8b926742b3d1b9297dd3d2fba722  src/audit/control.rs
a098d5cbd1ff092e93ca0ac4d6d3cb8416c22c0f0be55afa65eae2f7d47d1c4b  tests/storage_security.rs
7e57e7fc0552ae432a7de17c80235bc33ae800642a644785f660ab8b79bd54a7  tests/audit_integrity.rs
7ff67ce50cfa56c441d658ddb4bbe6c4e0cdde0c0aed0edb6f8d5d7edc437057  tests/tenant_storage.rs
fd720c786d51edb64aaae5c38da2b4917d7d135409ce41287da4e081e75444cf  tests/control_migration.rs
```

## Revue d’intégration finale — fixtures, image, procédure et logs globaux

Nouvelle lecture demandée par root après son intégration, toujours sans exécution cible ni mutation de données par le reviewer. Les huit empreintes core/backup ci-dessus ont été recalculées : **identiques au gel relu**.

Les diffs complets contre la copie de l’archive avant correction ont été examinés pour 17 cibles : `api_security`, `audit_load`, `audit_security`, `control_cli`, `decision_event_authority`, `decision_idempotence`, `gateway_tenancy`, `jev_contract`, `jev_credentials`, `review_regressions`, `runner_recovery`, `runner_terminal_audit`, `runtime_uncertainty`, `tenant_api`, `usage_ledger`, `worker_registry`, `workers_api`. Avec `tests/support/mod.rs`, cela fait 18 fichiers d’adaptation de fixtures. Le helper demande `0700` dès création via `tempfile::Builder`; le main legacy de `control_cli` est créé exclusivement `0600` puis fermé avant la première connexion SQLite ; les racines de scénarios recovery/terminal emploient `DirBuilder` avec `0700` dès création. **Aucune assertion supprimée ou affaiblie, aucun contrat de résultat modifié** dans ces diffs. Les fixtures de refus conservent leurs modes/liens/ACL explicites ; le produit n’a reçu aucune exception pour rendre les tests verts.

Le delta Dockerfile se limite à `chmod 0700 /data` après création et `chown 1000:1000`, pendant la construction de l’image. Il ne contient ni réparation au démarrage ni modification d’un volume préexistant. La section `docs/operations.md`, « Répertoire SQLite privé », a été lue intégralement : arrêt de tous les consommateurs et de leur redémarrage automatique, relevé des métadonnées/ACL/filesystem réels, sauvegarde cohérente et tête d’audit de référence, `chmod 0700` explicite seulement si le mode du répertoire est le seul écart, pas de récursion ni de suppression de sidecar, redémarrage avec même tenant et vérification de continuité avant admission. Les noms ASCII/réservés, le verrou borné et l’absence de migration automatique sont documentés. La restauration v2/claims reste explicitement non qualifiée. Cette procédure est cohérente avec le code ; elle n’a pas été exécutée par la revue et ne constitue pas une qualification du volume QA.

| Statut du run root, log lu | Preuve | Constat indépendant sur le log |
|---|---|---|
| PASS global logiciel | `/tmp/ai-guard-sqlite-final-test-2.log`, `CARGO_INCREMENTAL=0 cargo test --locked` | **327 tests de niveau supérieur PASS, 0 FAIL, 3 ignorés**. Les 44 résumés donnent 330 PASS bruts ; les trois probes enfants `F_GETLK` avec 16 tests filtrés sont retirées du total. `control_migration` apparaît bien avec 11 PASS après intégration du gate ASCII. |
| PASS build release | `/tmp/ai-guard-sqlite-final-release-2.log`, `cargo build --release --locked` | Compilation terminée en profil release, aucune erreur dans le log. |
| PASS fmt et Clippy rapportés par root, logs cohérents | `/tmp/ai-guard-sqlite-final-fmt-2.log`, `/tmp/ai-guard-sqlite-final-clippy-2.log` | Fmt vide ; Clippy terminé sans warning. Commandes annoncées : `cargo fmt --all -- --check` et `cargo clippy --locked --all-targets -- -D warnings`. Le reviewer n’a pas rejoué ces commandes. |
| NOT_RUN, toujours | Les trois tests moteurs ignorés | Confinement, smoke et persistance terminale réels ne deviennent pas PASS dans la suite logicielle. |

Le run complet contient les scénarios recovery et terminal avec moteur fixture Rust ; ils ne prouvent pas les effets d’un moteur Docker/Podman. Les preuves Linux reçues ensuite sont traitées dans la section suivante avec leur périmètre exact. Root annonce que `/data` QA reste `0755` et que le nouveau Rust n’est pas déployé : aucune preuve de durcissement ou de continuité du stockage QA n’est revendiquée ici.

Empreintes des trois nouveaux points d’intégration relus :

```text
6b3d78dc37d698138de2d3e6c7cda099d0d14b8e006bcbd75ed85e971cd32d26  tests/support/mod.rs
9d84ace8d64972c1486511e948c8509addc96c3deac2944a9154649a1bc5f274  images/control.Dockerfile
79eb0451a3323ed8ce2cafb795aac8b42bf5ba6d102243076f720522f7f06038  docs/operations.md
```

## Complément final Linux et dernière fixture moteur

Relecture du rapport `/tmp/ai-guard-sqlite-linux-report.md` et des artefacts `/tmp/ai-guard-sqlite-linux.8QMTSx`, sans rejeu. Les résultats sont bien **41 intégrations distinctes** (12 audit + 10 migration + 16 storage + 3 tenant), **1 unité VFS** et **1 test POSIX ACL** ; les trois résultats enfants du test F_GETLK sont exclus du total. Les deux tests d’ACL exclusivement macOS sont absents par `cfg`, pas « réussis » sur Linux.

L’environnement observé est **Linux ARM64 dans Docker Desktop**, noyau `7.0.14-linuxkit`, Debian 12, overlayfs, Rust/Cargo 1.99.1, serveur Docker Engine 29.9.2. L’image testée est `sha256:07f9d4821dc1acc9c882378cee7e64ff0f69d94b73d2e8c05719913610317f24`. Le Dockerfile et le build consignent `--network none`, `--locked --offline`, source copiée dans une nouvelle image et cache de dépendances déjà disponible. Les valeurs explicites de `containers-bounded.jsonl` confirment UID/GID 1000, NetworkMode none, CapDrop ALL, CapAdd null, no-new-privileges, privileged false, aucun mount/port et les limites CPU/mémoire/PID ; le rootfs est **inscriptible**, pour les compilations et fixtures de ces essais de développement. Ce dispositif n’est pas présenté comme le sandbox d’un nouvel audit formel ni comme le profil d’un worker livré.

Les neuf empreintes principales du rapport Linux (archive, manifeste, Dockerfile, build/tests/VFS/ACL, source de sonde et résultats JSON) ont été recalculées et correspondent. Les logs de vérification source hôte et image sont cohérents. Un nouveau contrôle du manifeste sur le checkout donne maintenant **71/72 identiques** : l’unique différence est l’ajout ultérieur de `.permissions(...0700)` à `tests/runtime_terminal_persistence.rs`. Tous les fichiers applicatifs et les quatre suites exécutées sous Linux correspondent toujours au gel. Le rapport de l’agent a été précisé pour dater cette correspondance ; aucune image ancienne n’est attribuée à la fixture moteur modifiée ensuite.

Le test ACL utilise réellement `setxattr(system.posix_acl_access)` : une entrée nommée avec masque nul conserve le mode 0600 et ses octets d’ACL ; le lecteur privé accepte. Le masque de lecture rend le mode 0640, puis le lecteur refuse sans réécrire l’ACL ou le contenu. Ce test confirme l’interprétation du masque POSIX dans l’overlayfs observé ; il ne prouve ni toutes les variantes de filesystem ni une ACL active de bout en bout au travers de tous les parents.

La source de la sonde Rust hors dépôt ouvre un vrai Store du gel, ajoute un événement factice, valide l’intégrité, constate main/WAL/SHM réguliers, puis reste en attente. Le READY final est consigné. `two-uid-results.json` montre UID 1001:1001 effectivement observé, un octet du témoin public lisible, puis les lectures main/WAL/SHM refusées, le rename du parent refusé et la précréation du journal refusée ; UID 1000 peut lire un octet du main depuis un processus distinct. Les relevés avant/après sont identiques pour **UID/GID, modes et liens** : parent 0700, main/WAL/SHM 0600 et lien unique. Ils ne prétendent pas mesurer le hash des données ou comparer dev/inode. Les codes 1 attendus sont bien des refus, pas des essais passés sans vérifier leur résultat. Le stop final de ce conteneur (exit 137) n’est pas compté comme une preuve de recovery.

Conclusion de ce complément : **aucun écart bloquant identifié dans la provenance, les comptages ou le bornage de ces preuves**. PASS de stockage logiciel Linux ARM64/overlayfs et de cette frontière DAC à deux UID ; **aucune qualification de broker/worker Linux, Linux x86_64 natif, cloud, Kubernetes, Podman, crash/disque ou restauration de claims**. L’attaque A→B→A, le mauvais propriétaire préexistant et la matrice ACL/mount plus large restent non exécutés. Les 19 `needs_validation` historiques conservent leur verdict.

La dernière adaptation de `runtime_terminal_persistence.rs` a été comparée à l’archive initiale **et** au snapshot Linux : exactement un ajout du mode 0700 au Builder de l’evidence, avant le VFS ; aucune assertion ni commande métier changée. Empreinte actuelle :

```text
3ee3e01ce4ea1d5ab90be1f38db0a0639a97a60d625fe504a0e276ea41113615  tests/runtime_terminal_persistence.rs
```

Root exécute séparément les essais Docker release ; tant que leurs logs ne sont pas lus ici, ils restent hors de la présente conclusion. `/data` QA 0755 n’a toujours pas été modifié par cette revue ni par les essais Linux sans montage.

## Conclusion bornée et statuts

- **PASS, revue source ciblée et intégration** : pas d’écart bloquant restant identifié dans les empreintes gelées ; avertissements de conception traités, consumer backup relu indépendamment, fixtures sans assertion relâchée et procédure offline cohérente.
- **PASS logiciel global, logs root** : 327 tests de niveau supérieur, build release, fmt et Clippy all-targets ; ce sont des preuves de développement, pas de qualification runtime/release.
- **PASS logiciel Linux borné, logs relus** : 41 intégrations + 1 unité VFS + 1 ACL POSIX, puis sonde DAC à deux UID sur overlayfs Docker Desktop ARM64 ; aucun broker Linux qualifié.
- **FAIL attendu puis PASS ciblé** : la régression locale de fermeture de FD main est caractérisée par les logs RED/GREEN ; cela ne démontre aucun franchissement autre UID.
- **NOT_RUN** : toute exécution par le reviewer ; mauvais propriétaire préexistant, matrice ACL/ancêtres complète, mounts réels `noowners`, substitution A→B→A, crash/recovery/checkpoint, adaptateurs Docker/Podman et qualification de release. Les preuves DAC à deux UID et POSIX ACL Linux ci-dessus lèvent seulement leurs anciens NOT_RUN sur ce périmètre précis.
- **BLOCKED / hors de cette conclusion** : déploiement sur l’ancien `/data` annoncé `0755` tant qu’une opération opérateur explicite offline n’a pas été faite ; qualification runtime et de release. La documentation ASCII et la vérification logicielle globale ont été relues dans l’intégration finale. Aucun durcissement silencieux du volume n’a été effectué par cette revue.
- **NOT_APPLICABLE à ce lot** : frontend/navigateurs, cloud, PKI/NAS ; aucun de ces chemins n’a été modifié ou exercé par le reviewer.

Les gates de production, notamment G06, restent ouverts. Cette conclusion est un deuxième regard logiciel borné et ne constitue ni une release qualifiée ni une modification du verdict de l’audit antérieur.

