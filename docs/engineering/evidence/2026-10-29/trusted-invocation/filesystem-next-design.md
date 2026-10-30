# Prochain vertical filesystem — configuration et credentials

**Recommandation : traiter d’abord le handoff configuration/credentials, puis SQLite avec la même primitive de racine fiable.** Le premier parcours reste fondé sur des réouvertures explicites de noms et concerne les credentials du contrôle ; la course SQLite dépend davantage du comportement VFS/WAL encore non observé. Cette priorité ne ferme ni ne dépriorise les écarts SQLite obligatoires.

Lecture seule du 29 octobre 2026 : AGENTS actuel, contrats d’architecture/code/gates/tests et [delta antérieur](/tmp/ai-guard-audit-filesystem-delta.md) relus. Guidance `security-audit` appliquée ; aucun test, probe, moteur, cible ou changement d’ACL exécuté pour ce travail. Aucun source ni document du dépôt modifié. Les constats formels d’audit restent `needs_validation`, sans nouvelle sévérité.

## Ce qui est déjà corrigé et ce qui demeure

- `src/runner.rs:590–618` ouvre désormais la source avec `O_NOFOLLOW|O_NONBLOCK|O_CLOEXEC`, contrôle **ce FD**, puis lit depuis lui. L’ancienne séparation metadata/read_to_string du rapport ne décrit plus ce tronçon. Ne pas refaire ce correctif.
- Les valeurs sont normalisées en un `NamedTempFile::new()` (`runner.rs:658–666`). Le snapshot dépend donc du répertoire temporaire du broker ; seul son pathname rejoint le spec. `runner/managed.rs:967–974` rouvre même ce nom pour calculer le digest.
- `worker_spec` (`runner/managed.rs:1069–1088`) passe `credentials.file.path()` à `ContainerSpec::with_secret_env_file`. Celui-ci vérifie un fichier régulier absolu puis conserve un `PathBuf` (`runtime.rs:93–104`). `create_args` émet ce nom dans `--env-file` (`:187–193`). Le FD conservé dans `CredentialFile` ne force pas la CLI à le lire.
- `private_credentials` (`runner/managed.rs:1991–2006`) relit la source, vérifie l’absence de credentials du contrôle et compare son digest à l’admission. Ce garde de reprise est utile ; son digest doit cependant provenir des bytes/FD réellement préparés, pas d’une réouverture du snapshot.
- `operations.rs:292–315` initialise puis attend le runtime/doctor ; `compose` recanonicalise ensuite `config_dir` (`:814–847`). Les projections sont locales et atomiques par FD, mais le handoff Compose redevient un nom.
- `Directory` du broker (`runner/managed.rs:61–102`) et `secret_projections` (`operations.rs:619` et suivantes) vérifient le propriétaire/mode du répertoire terminal, pas toute la chaîne susceptible de remplacer ce nom ni les ACL macOS.

La policy2 moteur ancre le **programme et son cwd** par FD ; elle ne transforme pas un argument `--env-file /chemin` en FD. Son parcours `openat` peut inspirer le helper, mais sa validation ne qualifie pas automatiquement tous les ancêtres d’un chemin de données (`runtime/invocation.rs:255–308`). **Ne pas modifier `invocation.rs` ni sa politique dans ce lot.**

Pour Docker, le parseur CLI appelle `ReadKVEnvStrings`, puis `kvfile.Parse` ouvre le fichier par `os.Open`; Podman `env.ParseFile` fait également un `os.Open(path)`. Il faut donc sécuriser le nom lisible par le **client hôte**, sans supposer un montage du tempfile dans la VM/worker. Ces sources amont étayent la conception, pas la version effectivement installée : [Docker opts](https://github.com/docker/cli/blob/master/opts/parse.go), [Docker kvfile](https://github.com/docker/cli/blob/master/pkg/kvfile/kvfile.go), [Podman env](https://github.com/containers/podman/blob/main/pkg/env/env.go). Les versions réelles restent à vérifier séparément.

## Garantie visée et acteurs

| Acteur | Garantie réaliste du lot |
|---|---|
| Worker confiné | Aucun accès ajouté aux fichiers hôtes. UID 1000 dans le conteneur ne donne aucun chemin hôte en l’absence de montage. Il reçoit seulement les variables fournisseur explicitement autorisées ; pas les tokens du contrôle. |
| Projet moins fiable | Les noms/contenus de projet ne peuvent fournir une racine de contrôle ou un snapshot injecté. Une racine sous un parent de projet modifiable par d’autres UID est refusée ; l’exclusion actuelle des sources de credentials dans le projet reste appliquée. |
| Autre UID local | Ni lecture des sources/snapshots ni remplacement de leur chaîne de noms, sous modes/ACL/filesystem qualifiés. C’est la frontière que le helper doit rendre effective. |
| Même UID du contrôle, root, administrateur moteur/stockage | Pas d’isolation DAC promise : ils peuvent normalement lire/modifier les fichiers du compte et/ou l’environnement runtime. Retenir un FD ou ajouter un hash ne supprime pas leur autorité. Un test de rename au même UID peut caractériser un bug, pas démontrer à lui seul une vulnérabilité inter-principaux. |

## Correctif concret minimal

1. Ajouter un petit module Rust `control_fs`, avec handles RAII de répertoire/fichier et opérations `openat`/`mkdirat`/`renameat`/`unlinkat`. Ouvrir depuis `/` ou un ancrage déjà fiable, refuser les liens à chaque composant, contrôler propriétaires et droits effectifs de **chaque parent**. Racine terminale : euid, `0700`, sans ACL permissive. Ancêtres : root/euid, aucun rename/modification par un autre UID ; exception explicite pour un parent root/sticky et son enfant appartenant au contrôle. Ne pas imposer `0700` à `/` ou à tous les ancêtres, ni rejeter aveuglément les tempdirs privés sous `/tmp` sticky.
2. Rendre l’initialisation interne capable de retourner un `ConfigDirectory` retenu. Garder `initialize_config(&Path)->Result<()>` comme wrapper compatible ; `Up` utilise la variante ancrée et la transmet à `compose`. `Down`, lectures de tokens, checkpoints et reprises ouvrent également une configuration par ce helper. Le chemin sérialisé demeure une provenance ; chaque nouvelle opération l’ouvre et le valide avant confiance.
3. Le parser de credentials conserve ses bytes canoniques `NAME=value\n`, noms, valeurs de masquage et digest calculé **sur ces bytes**. Il crée sa copie via FD sous un sous-répertoire privé de la configuration, pas dans le `TMPDIR` ambiant. Le fichier neuf est exclusif, `0600`, régulier, à lien unique ; son FD et son répertoire restent vivants jusqu’à la fin de `create`. Le digest de reprise garde exactement l’algorithme et la représentation actuels.
4. Pour `ContainerSpec`, remplacer le champ privé pathname seul par un handle partagé opaque portant FD + racine privée + chemin hôte stable. Garder si possible `with_secret_env_file(&Path)` en lui faisant ouvrir/valider un tel handle ; le chemin ne devient accessible au code CLI qu’après validation. Le Debug du handle est opaque. Cette modification reste indépendante de l’InvocationBinding policy2.
5. Garder les chemins Compose et `--env-file` ordinaires. Leur sûreté repose sur l’impossibilité pour **un autre UID** de remplacer les noms pendant le handoff, plus les handles/revalidations pour détecter les changements accidentels. Ni « recheck inode juste avant spawn » ni FD conservé seul ne crée une atomicité contre un writer encore autorisé.
6. ACL par plateforme : reprendre le wrapper Darwin FD déjà relu comme référence, avec refus d’une inspection indéterminée. POSIX Linux : le masque suit les bits groupe ; ne pas prétendre que toute ACL contourne `0700`. Ne pas qualifier implicitement NFSv4/SMB/FUSE. Les ACL de refus sûres des ancêtres macOS doivent avoir une politique explicite ; une installation refusée reçoit un diagnostic sans secret, jamais une suppression automatique d’ACL.
7. Nettoyage : unlink relatif au FD du répertoire du snapshot, limité au fichier créé par cette opération. Après crash, créer un snapshot frais depuis la source revalidée et ne jamais réarmer un start. Prévoir diagnostic des snapshots résiduels et rétention ciblée par identité ; aucune suppression récursive d’un répertoire de configuration, d’un volume ou d’une entrée inconnue.

Une transmission `/proc/self/fd/N`, `/dev/fd/N` ou `--env-file -` exigerait un contrat d’héritage de FD, de CLOEXEC et de lecture par chaque CLI, sur Linux et macOS. Elle n’est pas prouvée ici et ne doit pas devenir un raccourci de compatibilité. Elle ne résout pas non plus le handoff des sources de secrets Compose.

## Découpage et ownership proposés

| Lot | Propriétaire exclusif / fichiers | Acceptation avant intégration |
|---|---|---|
| A, primitive | Un implémenteur : nouveau `src/control_fs.rs`, export minimal `src/lib.rs`, nouveau test filesystem | Contrat FD/parents/ACL figé, erreurs actionnables, chemins privés existants acceptés, parents hostiles refusés. |
| B, vertical utilisateur | Après gel A, un implémenteur : `src/runner.rs`, `src/runner/managed.rs`, `src/operations.rs`, changement localisé `src/runtime.rs`, tests `runtime_authorization`, `runtime_operations` et handoff natif dédié | Init→Up et run→create/reprise consomment les mêmes fichiers préparés ; aucun secret dans args/logs/audit, aucun original remplacé. Ne pas paralléliser deux auteurs dans ces modules. |
| Revue/QA | Second regard indépendant ; parent conserve docs et QA Docker/Podman | Revue des handoffs, erreurs et cleanup ; moteurs réels séparés. Les fixtures ne qualifient pas le runtime. |
| C, SQLite | Après preuve de A/B : auteur distinct `src/audit.rs`, `src/audit/control.rs`, tests audit/migration/tenant_storage ; image et docs au parent | Réutiliser le parent privé, refuser UID/mode/ACL/sidecars incompatibles avant SQL, conserver données/backup/WAL ; pas de VFS nouvelle dans le premier correctif. |

## Régressions RED à écrire avant correction

- **Digest/snapshot** : hook de test substituant le nom du tempfile après préparation ; le digest doit rester celui des bytes préparés. Aujourd’hui `fs::read(path)` peut lire le remplaçant. Ce test de caractérisation au même UID ne suffit pas à qualifier l’exploitation.
- **Handoff natif** : CLI Rust de fixture approuvée attend entre spec et ouverture de `--env-file`. Vérifier que les bytes ouverts sont exactement ceux du digest et du masquage, ou que l’opération refuse avant effet. Variante avec `TMPDIR` hostile dans un processus de test isolé : aucun snapshot sensible créé à cet endroit après correction.
- **Configuration** : racine `0700` dans parent partagé non sticky writable → refus avant doctor/Compose ; parent privé et racine temp privée sous root/sticky → acceptation. Deux UID factices pour attester les droits de rename, sans déduire cette preuve du seul mode de test.
- **ACL et types** : macOS ACE permettant search/read malgré modes privés, Linux ACL nommée masquée, symlink d’ancêtre/leaf, hardlink, FIFO, mauvais propriétaire. Refus borné avant lecture, contenu préexistant inchangé.
- **Cycle de vie** : init idempotent ; renommage de parent simulé pour vérifier cleanup relatif sans toucher un successeur ; snapshot disparu pendant create → erreur/reprise sans nouveau start ; source fournisseur changée → admission nouvelle exigée, comme aujourd’hui.
- **Régressions existantes** : `credential_snapshot_matches_the_exact_values_used_for_masking`, `runtime_authorization`, `runtime_operations`, reprises gérées et smoke fournisseur **factice** déjà présent (`tests/runtime_smoke.rs:273–310`). Rejouer séparément Docker/Podman avec versions consignées, sans API fournisseur réelle.

## Compatibilité et qualification

Pas de nouveau service, stockage, schéma SQLite ou migration d’InvocationBinding. Les options CLI et les noms de variables restent inchangés. Le digest existant reste compatible si les bytes canoniques restent identiques. Les FD ne sont jamais sérialisés. Les configurations/sources dans un namespace modifiable ou une ACL non supportée seront désormais refusées : donner l’action de placement/copie dans une destination privée neuve, préserver tous les fichiers et valeurs existants et ne pas les déplacer/chmod automatiquement.

SQLite reste ensuite un lot obligatoire : le double open `AuditFile`/rusqlite et celui de la sauvegarde demeurent (`audit.rs:65–109,336–341`). Le helper de parents privés retire la précondition autre UID, sans promettre une adoption native du FD par SQLite. WAL, SHM, journal, connexions multiples et API backup doivent garder leur cohérence ; ne jamais remplacer SQLite par un chemin de FD non qualifié ni supprimer ses sidecars pour rendre un test vert.

**PASS** ici : lecture ciblée et proposition reliée aux chemins courants. **NOT_RUN** : toutes les régressions proposées, aucune nouvelle preuve runtime. **BLOCKED pour clôture formelle des hypothèses** : sandbox de validation et scénarios multi-UID/ACL/CLI réels non exécutés dans cette mission. Les logs historiques 209 et le smoke sans course ne ferment pas ces écarts ; la QA Docker policy2 actuellement conduite par le parent concerne une autre frontière.
