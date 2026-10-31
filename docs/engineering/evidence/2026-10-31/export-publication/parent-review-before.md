# Revue ciblée des parents et de la publication d'export

31 octobre 2026 — lecture seule du cycle `858f50e0ce21a807d9ae72e7f5af80c4684cd5b8` dans `.worktrees/broker-service`.

Périmètre : `src/workspace.rs` (`read_workspace_archive`, `anchored_export`), `src/workspace/authorized.rs`, `src/broker/service/targets.rs`, plus validation des racines dans `service/profile.rs` et appel final dans `runner/broker/executor/results.rs`. Aucun audit global, aucune édition de source, aucun Cargo/test/processus cible, moteur, fournisseur ou donnée utilisateur. L'unique modification Git observée au départ est `tests/broker_enforcer.rs`, laissée intacte.

## Conclusion

**Le parent d'export n'est pas soumis à la politique UID/modes/ACL du stockage privé de contrôle.** Le code accepte une racine d'export allowlistée et physiquement disjointe dont le parent est modifiable par un autre UID. Dans ce cas, l'ancrage du parent et la comparaison du staging avant rename ne lient pas atomiquement l'entrée source du rename au FD du staging vérifié. Le chemin peut publier un autre inode et annoncer le manifeste de l'archive initiale. Ce défaut est confirmé par lecture du chemin et des contrôles Linux ; la reproduction multi-UID et les effets de panne sont **NOT_RUN**.

Ce n'est pas une affirmation sur tous les déploiements. Si un ancêtre privé empêche l'autre UID de traverser, ou si les permissions effectives du parent lui interdisent de remplacer l'entrée, la précondition est absente. Aucun parent réel n'a été inspecté. Aucun contournement de l'authentification HTTP, du projet original ou du tenant n'est déduit de cette revue.

Le défaut de barrières de durabilité est déjà connu du coordinateur. Ajouter des `fsync` est nécessaire à sa correction, mais ne résout pas la substitution de namespace décrite ci-dessous.

## F1 — Substitution du staging dans un parent partagé accepté

**Qualification : confirmé par source, conditionnel à la configuration ; gravité moyenne (intégrité d'un export local). Effet sur un vrai filesystem à deux UID : NOT_RUN.**

Principal adverse : autre UID local, non privilégié, distinct du compte de contrôle ; aucun accès aux credentials, au broker privé, au moteur ou au contenu du staging initial n'est nécessaire. Ressource affectée : destination neuve d'un export légitime et fidélité de son manifeste/completion.

### Préconditions nécessaires

1. L'opérateur a allowlisté une racine d'export et l'appel légitime choisit un parent `P` dessous. Les racines source/export/contrôle sont disjointes et les composants ne sont pas des symlinks.
2. L'autre UID peut traverser jusqu'à `P`, observer le nom de staging et modifier les entrées de `P` : par exemple `P` en `0777` sans sticky bit, ou un groupe/une ACL accordant les droits effectifs nécessaires. Aucun répertoire privé supérieur ne coupe cet accès.
3. Les protections du filesystem/LSM n'interdisent pas les opérations de rename concernées. Le scénario minimal Linux garde tous les noms dans **le même parent**.
4. L'autre UID est exécuté pendant l'export et gagne la fenêtre entre la dernière comparaison du staging et le rename. Un hook déterministe est requis pour une régression, plutôt qu'une course statistique.

Une racine définie dans un fichier privé est une configuration de confiance, mais cela ne prouve pas que chaque répertoire qu'elle autorise est exclusif au compte de contrôle. Aucun invariant documentaire ou contrôle effectif trouvé dans les sources ciblées n'impose cette exclusivité aux parents export.

### Mécanisme précis

- [service/profile.rs:191](/Users/louis/Documents/ai-guard/.worktrees/broker-service/src/broker/service/profile.rs:191) borne et normalise les listes de racines ; la validation suivante porte sur leur disjonction lexicale. Elle n'ouvre pas les parents export pour vérifier propriétaire, mode ou ACL.
- [service/targets.rs:121](/Users/louis/Documents/ai-guard/.worktrees/broker-service/src/broker/service/targets.rs:121) appelle `AuthorizedExportParent::open`, puis lie l'intention immutable à `(parent_device, parent_inode)` et au nom cible. Les synchronisations de l'input privé aux lignes 188–193 ne protègent pas le namespace export.
- [authorized.rs:34](/Users/louis/Documents/ai-guard/.worktrees/broker-service/src/workspace/authorized.rs:34) construit l'identité avec `is_dir`, `dev`, `ino` uniquement. `AuthorizedSource::open` retient la chaîne depuis `/` ; sa revalidation à [authorized.rs:105](/Users/louis/Documents/ai-guard/.worktrees/broker-service/src/workspace/authorized.rs:105) recoupe identités et liens d'ascendance. Aucun UID/mode/ACL n'est inspecté.
- [authorized.rs:207](/Users/louis/Documents/ai-guard/.worktrees/broker-service/src/workspace/authorized.rs:207) réutilise ce même mécanisme pour le parent et les racines export. La disjonction physique et la revalidation ne changent pas ce constat.
- [workspace.rs:639](/Users/louis/Documents/ai-guard/.worktrees/broker-service/src/workspace.rs:639) vérifie seulement que le FD parent est un répertoire, crée `.agentsandbox-export-<UUID>` en `0700`, puis ouvre ce nom sans suivi de symlink. Le mode du staging protège son contenu ; il ne protège pas son entrée dans un parent modifiable.
- [workspace.rs:699](/Users/louis/Documents/ai-guard/.worktrees/broker-service/src/workspace.rs:699) recoupe le parent absolu, ouvre le staging sous son nom et compare son inode au FD retenu aux lignes 706–708. Le rename utilise ensuite `self.name` aux lignes 714/724, pas le FD du staging. L'exclusivité `RENAME_NOREPLACE`/`RENAME_EXCL` ne lie pas cet ancien nom à l'inode précédemment vérifié.
- À [workspace.rs:736](/Users/louis/Documents/ai-guard/.worktrees/broker-service/src/workspace.rs:736), le code marque `published=true` et retourne sans ouvrir la destination pour vérifier `destination == self.directory`. `AuthorizedExportParent::extract` retourne ensuite le manifeste initial sans nouvelle revalidation. [results.rs:475](/Users/louis/Documents/ai-guard/.worktrees/broker-service/src/runner/broker/executor/results.rs:475) transforme ce résultat en faits d'export et digest du manifeste.

Interleaving Linux minimal : le broker a extrait les octets factices dans `P/S`, tient leur FD, et a validé `P/S == FD`. Pendant le hook juste avant rename, l'UID adverse renomme `P/S` en `P/saved` puis renomme son propre répertoire `P/replacement` en `P/S`. `P` conserve son identité et ses ancêtres. Le broker renomme alors `P/S` en `P/result`, obtient succès, et renvoie le manifeste de `saved`, alors que `result` contient le marqueur adverse. Le FD `current_staging` resté ouvert n'empêche pas ces renommages.

Le [code Linux v6.12, `vfs_rename`](https://github.com/torvalds/linux/blob/v6.12/fs/namei.c#L4626-L4664) contrôle l'écriture du répertoire source pour changer `..` lorsque les parents diffèrent ; la séquence ci-dessus reste dans le même parent. Les [man-pages Linux `rename(2)`](https://man7.org/linux/man-pages/man2/rename.2.html) décrivent le maintien des FD ouverts, le contrôle sticky et la portée de `RENAME_NOREPLACE`. Il n'est donc pas nécessaire que l'autre UID puisse lire ou écrire les octets sous le staging `0700` pour cette substitution de nom sous Linux.

**Limite plateforme :** l'[ancienne documentation Apple de `rename(2)`](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/rename.2.html) indique une restriction supplémentaire sur les répertoires non accessibles en écriture, y compris sans changement de parent. Cette archive ne qualifie pas le comportement APFS actuel de `renameatx_np`. Aucune exploitation macOS/ACL Darwin n'est affirmée ; ces variantes exigent leur propre test multi-UID. La présence d'une ACL ne prouve pas à elle seule les droits effectifs nécessaires.

### Cas qui excluent le scénario

- Parent et ancêtres effectivement non modifiables par d'autres UID ; un ancêtre privé peut aussi empêcher la traversée.
- Répertoire sticky protégeant une entrée staging appartenant au broker, si l'adversaire n'est ni propriétaire du parent ni propriétaire de cette entrée et ne possède pas de privilège de contournement. La seule présence du bit sticky ne suffit donc pas si le parent appartient à l'adversaire.
- Politique de plateforme/LSM qui refuse effectivement la substitution ; cela doit être établi pour la configuration qualifiée.

Les contrôles UID/modes/ACL de [control_fs.rs:559](/Users/louis/Documents/ai-guard/.worktrees/broker-service/src/control_fs.rs:559) s'appliquent au stockage de contrôle. `AuthorizedExportParent` n'utilise pas cette validation pour ses FDs : les garanties du broker privé ne doivent pas être attribuées par transitivité aux répertoires d'export.

### Fenêtres associées, même invariant

L'absence de droits sûrs sur les ancêtres permet aussi à un autre UID de déplacer `P` après le dernier recoupement absolu mais avant/après rename. Le FD retient le bon répertoire ; la destination publiée peut alors ne plus correspondre au chemin annoncé. Il s'agit d'une perte de liaison au chemin, sans preuve de lecture de contenu privé.

Une fenêtre antérieure existe entre `mkdirat(S,0700)` et l'ouverture de `S` (lignes 646–650). Sous les mêmes droits effectifs Linux, une substitution à cet instant ferait retenir un autre répertoire, car l'ouverture n'en vérifie pas l'UID/mode/ACL. Ce n'est pas une seconde vulnérabilité indépendante présentée comme reproduite ; cela montre qu'un simple contrôle après le rename final ne suffit pas à établir tout le cycle.

## Conséquences pour le correctif de durabilité

1. Définir et faire respecter la politique des parents export et de leurs ancêtres avant la première création. Elle doit exclure les autres UID capables de modifier le namespace, via propriétaire, modes et ACL effectives sur les filesystems supportés. Ne pas supposer que l'allowlist lexicale suffit. Ne pas réparer automatiquement des chemins existants par chmod/ACL. Une réutilisation factorisée des contrôles de confiance peut être envisagée, sans imposer par accident tous les formats privés aux exports utilisateur.
2. Le FD retenu doit rester l'autorité des écritures, de la synchronisation et de la publication. Revalider cette politique et les mappings aux derniers points nécessaires. Un check `destination == FD` après rename est une vérification utile, mais pas une solution atomique à un namespace hostile : l'adversaire pourrait déplacer des noms entre les observations.
3. Synchroniser chaque fichier **après** ses écritures et son mode final, puis les répertoires de bas en haut, y compris les répertoires implicites/vides et la racine staging, avant publication. Le code actuel ne fait que `flush` puis `set_permissions` à [workspace.rs:514](/Users/louis/Documents/ai-guard/.worktrees/broker-service/src/workspace.rs:514). `flush` seul n'établit pas la durabilité disque.
4. Après rename exclusif, vérifier les faits requis et synchroniser le parent de publication avant tout succès d'export. Une sync de fichier ne rend pas nécessairement son entrée de répertoire durable : voir [`fsync(2)`](https://man7.org/linux/man-pages/man2/fsync.2.html). Les sémantiques de stockage/plateforme restent à qualifier, sans confondre test de syscall et panne physique.
5. Traiter explicitement l'échec **après** rename : le résultat peut déjà exister. Ne pas retourner succès, ne pas supprimer/recréer aveuglément une destination publiée, ne pas réexporter sous une nouvelle autorisation par déduction. Préserver l'état partiel à réconcilier et éviter que `Drop` efface la destination après une erreur de sync. Le booléen `published` devra refléter le fait du rename, distinct de sa durabilité acquise.

Synchroniser le FD initial ne prouve pas que le nom déplacé est celui de ce FD ; synchroniser `P` peut rendre durable le mauvais inode publié si F1 reste ouvert. Les deux invariants doivent être traités séparément avant de qualifier le succès durable.

## Régressions requises — toutes NOT_RUN par cette revue

| Cas | Montage contrôlé et critère |
|---|---|
| Contrôle positif | Archive bornée avec un fichier texte et un répertoire vide, parent sûr, export réel sans moteur ; destination neuve et manifeste concordants. |
| Parent partagé Linux | Deux UID factices dans un sandbox OS jetable, aucun credential/donnée réelle. Parent autorisé `0777` nonsticky et traversable ; gate entre comparaison du staging et rename. L'UID adverse substitue une seule fois un répertoire à marqueur. Le correctif doit refuser l'autorisation du parent ou l'opération, jamais retourner succès avec un manifeste différent. |
| Contrôles négatifs de droits | Même fixture, parent privé/non modifiable ; puis parent sticky appartenant au contrôle/root et staging appartenant au broker. Vérifier que la substitution du second UID est refusée et que l'export légitime fonctionne. Ne pas compter un test dont le second UID ne peut déjà pas traverser l'ancêtre du harnais. |
| Ancêtres et création | Gate après dernier recoupement du parent, puis gate entre mkdir staging et open ; seul namespace factice touché. Refus ou liaison exacte, aucun succès sous chemin divergent. |
| ACL et plateformes | Séparément Linux/ACL POSIX et macOS/APFS/ACL Darwin : consigner droits effectifs, UID, modes, sticky, filesystem et résultat du rename factice. Ne pas extrapoler une combinaison à l'autre. |
| Durabilité et erreurs | Vérifier ordre de sync des fichiers/modes/répertoires/parent ; injecter refus avant rename et après rename. Une erreur post-publication conserve les faits réels et ne transforme pas l'état ambigu en succès ou en absence. Les essais SIGKILL ne prouvent pas une panne d'alimentation. |

Le harnais doit être en Rust, borné, sans réseau, sans moteur et sans création/modification de comptes sur l'hôte. La disponibilité d'un sandbox avec deux UID est un prérequis à fournir par Luna/coordonateur, pas une autorisation d'utiliser d'autres utilisateurs ou données de cette machine. La validation s'arrête au marqueur factice et aux métadonnées nécessaires.

## Référence figée des sources lues

| Fichier | SHA-256 |
|---|---|
| `src/workspace.rs` | `9a692f10f059a0eb02e921d6be2ab73456912e51977e502789fb16a6b295f0a1` |
| `src/workspace/authorized.rs` | `5d2beaafa25159cb07d09615344bfad690539d7cd3497294e6bec26fcfe084de` |
| `src/broker/service/targets.rs` | `890cf28b5b7361ee57055ba5cf0991b6afa0af6f9c3ac87886e1e7aead8e4dba` |
| `src/broker/service/profile.rs` | `aac35b496451ee4317a3adb1edcbbe1f7f7b8f2585c09d04b82eebbcad54b532` |

Qualification limitée : chemin source relu et contrat Unix/Linux recoupé ; aucune reproduction exécutée et aucune qualification de durabilité physique. Aucun nouveau défaut de durabilité n'est revendiqué au-delà du défaut déjà transmis par le coordinateur.
