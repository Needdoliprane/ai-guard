# Poste de travail local

[English](README.md) · [Index d'installation](../README.fr.md) · [Broker de confiance](../broker/README.fr.md) · [Fournisseurs IA](../providers/README.fr.md)

Ce guide permet de lancer les services de contrôle dans Docker ou Podman, d'exécuter une première commande dans un worker, d'examiner son audit et d'exporter ses changements. Le CLI Rust de confiance commande le moteur du poste. Gateway et dashboard occupent des conteneurs distincts ; les workers reçoivent une copie filtrée du projet, jamais un montage du projet original.

## Préparer le poste

Il faut Git, la toolchain Rust indiquée par `rust-toolchain.toml`, et Docker avec Compose ou Podman rootless avec un fournisseur Compose. La construction des images demande l'accès à leurs dépendances épinglées ; préparer les images avant de déconnecter une installation hors ligne. La construction locale est prise en charge ; aucune image publique ai-guard n'est imposée.

Exécuter ces commandes à la racine du dépôt. Remplacer le chemin du projet par un répertoire réel et existant. Garder la configuration hors de ce projet et hors de ce checkout.

```sh
cargo build --release --locked
AIGUARD_BIN="$PWD/target/release/agentsandbox"
AIGUARD_CONFIG="$HOME/.ai-guard"
AIGUARD_PROJECT="/absolute/path/to/your/project"
AIGUARD_GATEWAY="http://127.0.0.1:8787"
AIGUARD_DASHBOARD="http://127.0.0.1:4000"
"$AIGUARD_BIN" init --config-dir "$AIGUARD_CONFIG"
```

`init` crée des credentials runner et viewer distincts et conserve ceux qui existent. Le répertoire de configuration doit être privé (`0700`), avec des sources de credentials privées (`0600`). La commande prépare aussi les projections en lecture seule des conteneurs ; ne pas élargir les permissions des sources pour les rendre lisibles par un conteneur. Exclure configuration, bases, plans de session et credentials de Git, des captures et des exports de workspace.

Choisir un moteur pour cette installation :

```sh
"$AIGUARD_BIN" doctor --runtime docker
"$AIGUARD_BIN" build --runtime docker --source .
```

Avec Podman :

```sh
"$AIGUARD_BIN" doctor --runtime podman
"$AIGUARD_BIN" build --runtime podman --source .
```

Le fonctionnement normal exige un moteur rootless. Pour utiliser délibérément Docker Desktop ou un daemon qui ne déclare pas ce mode, ajouter `--allow-rootful-runtime` à `doctor`, `build` et à chaque `run`. Cette option autorise explicitement un daemon qui ne déclare pas le mode rootless ; une VM ne démontre pas une isolation équivalente à rootless. Le [diagnostic runtime](../../../src/runtime.rs) lit ce mode déclaré, mais ce parcours ne produit actuellement aucun événement `runtime.security_downgrade`. Sa preuve d'audit durable dédiée reste à implémenter et qualifier. Ne pas fournir de socket moteur ni de mode privilégié aux conteneurs applicatifs.

L'image worker par défaut permet les commandes ordinaires. Pour Codex ou Claude Code, construire les versions des clients décrites dans le [guide fournisseurs](../providers/README.fr.md) avant une session IA.

La création des workers et des helpers de workspace utilise `--pull=never` avec Docker et Podman. Préparer l'image sélectionnée dans le cache local de ce moteur avant la session ; une image indisponible fait échouer la création au lieu de déclencher un téléchargement. Construire explicitement l'image avec la commande adaptée ci-dessus, ou rétablir l'image délibérément sélectionnée par la procédure de distribution de l'installation, puis réessayer. Les contrôles d'identité d'image existants restent appliqués ; cette politique n'acquiert ni ne substitue d'image.

`build --source` désigne la distribution source d'ai-guard, pas le projet qu'un agent modifiera. Avant de contacter le moteur, le CLI vérifie les Dockerfiles, `.dockerignore`, manifestes/locks, entrées Rust/frontend et politique requis, ainsi que leurs répertoires parents. Une entrée absente, liée ou du mauvais type est signalée par son chemin relatif. Rétablir cette entrée dans la distribution sélectionnée puis réessayer ; ce refus ne nécessite aucune suppression d'image, de cache ou de données. La racine source explicitement choisie reste canonicalisée normalement.

Ce contrôle est structurel. `build` copie ensuite ses entrées avant la première commande du moteur et désigne cette copie au CLI du moteur à la place du checkout, comme le décrit [sources préparées de build et Compose](#prepared-source) : une modification du checkout postérieure à la copie n'atteint pas les deux builds. Aucune de ces deux étapes ne valide tous les imports ni le contenu des locks ; `.dockerignore` est comparé octet par octet au fichier que ce CLI reflète et n'est pas interprété ; et les deux builds ne sont pas atomiques : la première image peut encore réussir avant un échec ultérieur. Conserver les résultats réels du moteur, dont le CLI [n'affiche qu'une partie](#engine-output) ; un contrôle réussi et une copie privée ne qualifient ni un build reproductible ni la production.

<a id="build-version"></a>

### Identifier version et build

La version produit vient de `Cargo.toml`, actuellement 0.0.1. `--version` ou
`version --json` du CLI principal est statique ; aucun runtime ou fournisseur
n'est lancé. `build` vérifie les versions Cargo/package/lock frontend dans une
seule source capturée, puis fournit son digest de source préparée comme Build ID
commun aux stages frontend/Rust du contrôle et Rust du worker.
`--build-revision` facultatif reçoit explicitement 40 ou 64 caractères hex, par
exemple :

```sh
"$AIGUARD_BIN" build --runtime docker --source . --build-revision "$AIGUARD_BUILD_REVISION"
```

Donner à cette variable publique la révision relue, ou omettre l'option si elle
est inconnue. Aucun commit n'est découvert depuis Git. Les builds manuels sans
annotation gardent des IDs inconnus ; les différences ou anciennes identités
du frontend et des serveurs restent visibles. Le [guide version et mise à jour
manuelle](../release/README.fr.md#versions-and-updates) explique routes status,
builds partiels, IDs d'images de session retenus, sauvegardes/migrations et
basculement délibéré. Aucun updater automatique. Ce contrat candidat est lu
sur `e0372db` ; compilation, tests, images et déploiement du nouvel alignement
restent NOT_RUN dans ce lot documentaire.

<a id="prepared-source"></a>

### Sources préparées de build et Compose

Avant toute commande de build ou Compose du moteur, les commandes CLI `build`, `up` et `down` font une copie privée et bornée de ce qu'elles remettent au moteur :

- `build` copie une liste fixe d'entrées publiques de build : `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `LICENSE`, `.dockerignore`, `src/**/*.rs`, `policies/*.json`, cinq fichiers de `front/` (`package.json`, `package-lock.json`, `tsconfig.json`, `vite.config.ts`, `index.html`), `front/src/**/*.ts` et `*.css`, `front/public/favicon.svg`, et `images/*.Dockerfile`. Les deux builds d'image utilisent la même copie, et le contrôle structurel ci-dessus est rejoué sur elle.
- `up` et `down` copient exactement `deploy/<engine>/compose.yaml` et ajoutent un fichier env vide.

La liste des entrées de build est figée dans le binaire du CLI et reflète le `.dockerignore` de la distribution à partir de laquelle ce CLI a été construit ; le fichier est copié, pas interprété. `build` refuse, avant toute commande du moteur, une distribution sélectionnée dont le `.dockerignore` a d'autres octets (un checkout aux fins de ligne différentes compris) ou qui contient un fichier d'ignore par Dockerfile `images/<name>.Dockerfile.dockerignore` ; les deux messages figurent dans le tableau des entrées refusées ci-dessous. Une distribution adaptée pour envoyer un autre chemin au moteur doit modifier son `.dockerignore` ; elle est alors refusée par un CLI dont la liste n'a pas été modifiée avec lui : aucune entrée n'est écartée de la copie en silence. Construire une telle distribution avec le CLI construit à partir d'elle, après avoir modifié la liste et son SHA-256 épinglé en même temps que le fichier (`Scope::selects_file` et `BUILD_IGNORE_SHA256` dans `src/operations/source_snapshot/capture.rs` ; un test unitaire échoue tant qu'ils diffèrent), ou lancer directement le build du moteur, qui lit le checkout et non une copie préparée.

Le CLI du moteur ne reçoit ensuite en arguments que des chemins situés dans cette copie ; avec Docker, ils suivent `--host <endpoint>` :

```text
build --file <copy>/context/images/<name>.Dockerfile --tag <tag> [--build-arg <NAME>=<public-value>]… <copy>/context
compose --project-name agentsandbox --env-file <copy>/compose-empty.env --file <copy>/context/deploy/<engine>/compose.yaml up --detach --force-recreate
compose --project-name agentsandbox --env-file <copy>/compose-empty.env --file <copy>/context/deploy/<engine>/compose.yaml down
```

La source sélectionnée est lue sans suivre de lien, n'est jamais écrite et n'est jamais nommée dans ces arguments. Chaque préparation affiche une ligne sur la sortie d'erreur :

```text
Prepared public distribution (build): sha256=<hex> files=<n> bytes=<n> shared_writable=<n> device=<n> inode=<n> source=<path>
```

`up` et `down` affichent la même ligne avec `(compose)`. `sha256` couvre le chemin relatif, la longueur et le SHA-256 de chaque fichier préparé ; il identifie donc les octets préparés et n'est pas un digest d'image. `shared_writable` compte les originaux que le groupe ou les autres peuvent écrire : ce fait est rapporté, pas refusé. `device`, `inode` et `source` identifient le répertoire sélectionné. La copie est vérifiée fichier par fichier contre les longueurs et empreintes enregistrées à la fin de la préparation, juste avant le départ de chaque commande du moteur, puis après sa fin.

La copie est un nouveau répertoire sous le répertoire temporaire : `TMPDIR` s'il est défini, sinon la valeur par défaut de la plateforme, soit `/tmp` sous Linux. Ses répertoires sont en mode `0700` et ses fichiers en `0600` :

```text
<TMPDIR>/ai-guard-distribution-<random>/
  context/                arbre visible du moteur : le contexte de build, ou deploy/<engine>/compose.yaml
  compose-empty.env       `up` et `down` seulement
  manifest.json           chemin, device et inode de la source ; chemin, octets et SHA-256 de chaque fichier ; l'empreinte d'ensemble
  RETAINED-UNKNOWN.json   seulement lorsque la terminaison d'une commande du moteur est inconnue
```

Elle est supprimée dès que les commandes du moteur qui l'utilisent sont terminées, que la commande ait réussi, ait été refusée, ait échoué ou ait été interrompue ; `up` la supprime avant d'attendre les URL de disponibilité. La seule exception est une [terminaison inconnue](#prepared-source-retained). La suppression se fait par nom, et seulement tant que ce nom désigne encore le répertoire qui a été créé (même périphérique et même inode). Deux avertissements sur la sortie d'erreur signalent une copie qui n'a pas été supprimée :

```text
warning: prepared source at <path> could not be removed: <error>
warning: prepared source at <path> was not removed: the path no longer names the directory that was prepared (<another entry has that name | system error>); nothing was removed
```

Le premier laisse en place le répertoire, ou ce qu'il en reste. Le second signifie que la copie a été déplacée, ou qu'autre chose a pris son nom, pendant la commande : ni la copie, où qu'elle se trouve désormais, ni l'entrée présente à ce chemin ne sont touchées, et la commande a le plus souvent échoué avec `the prepared copy changed`. Dans les deux cas, suivre [un répertoire laissé par un CLI tué](#prepared-source-retained) avant de supprimer quoi que ce soit.

**Répertoire temporaire.** `build`, `up` et `down` exigent donc un répertoire temporaire conforme à la même politique de répertoire privé que le répertoire de configuration. `TMPDIR` doit être un chemin absolu situé hors de la source sélectionnée : les deux points sont vérifiés avant toute création, le second par périphérique et inode le long des répertoires qui y mènent, de sorte qu'aucune écriture du chemin ne masque la source. Chaque répertoire, de `/` jusqu'à lui, doit être un vrai répertoire, appartenir à root ou au compte qui lance le CLI, se trouver sur un système de fichiers local pris en charge (ext2/3/4, XFS, Btrfs, tmpfs, overlayfs, ramfs ou F2FS sous Linux ; APFS ou HFS+ local avec propriété activée sous macOS) et ne pas être inscriptible par le groupe ou les autres, sauf un répertoire appartenant à root avec le sticky bit, comme l'est habituellement `/tmp`. Un lien symbolique dans ce chemin est refusé, sauf les alias système macOS `/tmp` et `/var`, traduits vers `/private` (le répertoire temporaire par utilisateur de macOS se trouve par défaut sous `/var/folders`) ; une ACL qui permet à un autre principal de modifier l'un de ces répertoires est refusée elle aussi. Le `/tmp` usuel, appartenant à root avec sticky bit, sur l'un de ces systèmes de fichiers sur disque ou sur tmpfs, est donc accepté. Trois messages refusent un répertoire temporaire, chacun avant toute commande de build ou Compose du moteur :

```text
Error: the temporary directory must be an absolute path, but TMPDIR is "<value>"; set TMPDIR to an absolute directory outside the selected source
Error: the temporary directory <TMPDIR> (TMPDIR) is the selected source <source> or lies inside it; set TMPDIR to a directory outside the source
Error: cannot create the private prepared-source directory under <directory> (TMPDIR). Set TMPDIR to a directory owned by you or root, not writable by group or others, on a supported local filesystem, with no symbolic link in its path
```

Le premier refuse une valeur relative ou vide (`TMPDIR=` définit une valeur vide, il ne supprime pas la variable) : une telle valeur serait résolue par rapport au répertoire de travail, qui est la source sélectionnée dans les commandes de ce guide. Le deuxième refuse un `TMPDIR` qui est la source ou un répertoire situé sous elle, quelle que soit son écriture. Tous deux précèdent toute création. Le troisième refuse un répertoire temporaire sur ZFS, NFS ou un montage FUSE, atteint par un autre lien symbolique, ou inscriptible par le groupe ou les autres sans le sticky bit de root ; le motif propre à la politique suit sous `Caused by:` et nomme la règle enfreinte, pas le répertoire qui l'enfreint, et un répertoire privé vide créé pour ce contrôle est supprimé aussitôt. Un `TMPDIR` dont le chemin n'est pas UTF-8 n'est pas utilisable non plus : les chemins remis au moteur doivent être UTF-8 (`the prepared-source path must be UTF-8; choose a UTF-8 TMPDIR`).

Pour `build`, le moteur n'a pas été contacté lorsque ces refus, ou ceux du tableau suivant, surviennent. Pour `up` et `down`, ils viennent après les sondes du moteur : avec Docker, son contexte (sauf si `--runtime-endpoint` est fourni) et son identité ; pour `up`, avec l'un ou l'autre moteur, ses informations telles que `doctor` les lit ; un `down` avec Podman n'en fait aucune. `up` a aussi déjà créé ou rafraîchi le répertoire de configuration et ses projections de credentials, sans recréer les services : relancer `up` une fois la cause corrigée.

Rien n'est réparé et aucun mode n'est modifié à votre place. Choisir un répertoire existant et conforme, avec assez de place pour les entrées (256 Mio au plus), et le désigner pour cette commande :

```sh
TMPDIR="/absolute/path/to/a/conforming/directory" "$AIGUARD_BIN" build --runtime docker --source .
```

Pour trouver le répertoire qui enfreint la politique, examiner chaque répertoire de `/` jusqu'à celui qui est refusé : son propriétaire, son mode et son sticky bit, s'il s'agit d'un lien, et son type de système de fichiers. Sous Linux :

```sh
AIGUARD_TMP="/absolute/path/of/the/refused/temporary/directory"
namei -l "$AIGUARD_TMP"
stat -f -c %T "$AIGUARD_TMP"
```

Sous macOS, lancer `ls -lde` sur chaque répertoire tour à tour (il affiche aussi les ACL) et lire le type de système de fichiers dans la sortie de `mount`. Ce sont des outils standard ; ils n'ont pas été exécutés dans le cadre de cette validation. Si `/` lui-même, ou un autre répertoire commun à tous les chemins, enfreint la politique, aucun `TMPDIR` ne peut convenir et les commandes CLI `build`, `up` et `down` ne sont pas utilisables sur cet hôte ; la même politique protège déjà le répertoire de configuration.

**Entrées refusées.** Chaque refus ci-dessous précède la commande de build ou Compose du moteur et laisse la source intacte ; aucun fichier existant n'est modifié pour faire passer un contrôle. La cause rapportée par le système, lorsqu'il y en a une, suit sous `Caused by:`.

| Ce qui est refusé | Message |
|---|---|
| Un lien symbolique à un chemin que la commande sélectionne. Pour `build`, il s'agit de **tout** lien symbolique sous `src/` ou `front/src/`, quel que soit son nom, y compris un nom que la liste ne sélectionne pas, comme `src/notes`, ou un lien de verrou d'éditeur comme `src/.#main.rs`, et d'une entrée liée ailleurs, comme `policies/extra.json` ou `images/extra.Dockerfile`. Pour `up` et `down`, il s'agit de `deploy`, `deploy/<engine>` ou `compose.yaml` lui-même. Les liens situés à des chemins que la commande ne sélectionne pas sont ignorés. Une entrée requise liée est signalée plus tôt, par le contrôle structurel et avec son propre message. | `source input must not be a symbolic link: <path>` |
| Une FIFO, un périphérique ou un socket là où un fichier d'entrée est attendu | `source input must be a regular file: <path>` |
| Pour `build`, un `.dockerignore` dont les octets diffèrent de ceux que ce CLI reflète | `the build input list of this CLI mirrors a .dockerignore whose SHA-256 is <pinned>; the .dockerignore of the selected distribution has SHA-256 <actual>. Build with the CLI built from that distribution, or run the engine build directly` |
| Pour `build`, un fichier d'ignore par Dockerfile `images/<name>.Dockerfile.dockerignore` | `the build input list of this CLI mirrors .dockerignore only: the per-Dockerfile ignore file <path> of the selected distribution is not reconciled with it, and a builder may apply that file instead. Remove it, or run the engine build directly` |
| Plus de 16 384 entrées examinées. Pour `build` : les entrées de la racine de la source, tout ce qui se trouve sous `src/` et `front/src/`, et les entrées directes de `policies/`, `front/`, `front/public/` et `images/`. Pour `up` et `down` : les entrées de la racine de la source, de `deploy/` et de `deploy/<engine>/`. Un répertoire que la commande ne parcourt pas, comme `target/` ou `.git/`, compte pour une entrée. | `source exceeds the bound of 16384 entries in its selected directories` |
| Une entrée de plus de 32 Mio, ou un fichier Compose de plus de 1 Mio | `source input exceeds the per-file bound of <n> bytes: <path>` |
| Plus de 256 Mio d'entrées au total ; le chemin nommé est celui où la borne a été franchie, pas nécessairement le plus gros fichier | `source inputs exceed the total bound of 268435456 bytes of the prepared copy at: <path>` |
| Un chemin sélectionné de plus de 64 composants ou de plus de 4 096 octets, non UTF-8, ou contenant un caractère de contrôle | `source path exceeds the bound of 64 components or 4096 bytes: <path>`, `source path must be UTF-8: <path>` ou `invalid source path: "<path>"` |
| Une entrée ou un répertoire qui a changé, disparu, est devenu un lien ou n'a pas pu être lu pendant sa copie ou sa confirmation | `source file changed while it was being read: <path>`, `source changed while it was being prepared: <path>`, `source directory changed while it was being prepared: <path>`, `source input is unavailable or is a symbolic link: <path>`, `source input is unavailable: <path>`, `source input cannot be read: <path>` ou `source enumeration failed: <path>` |
| Le répertoire source sélectionné, ou un répertoire situé au-dessus de lui, qui ne peut pas être ouvert sans suivre de lien ; sous macOS, aussi un répertoire que le compte qui lance le CLI ne peut pas lire | `source directory is unavailable or reached through a symbolic link: <path>` |
| La limite de fichiers ouverts du processus ou du système atteinte pendant la préparation | ``the open-file limit of this process or of the system was reached while the source was being prepared; raise the descriptor limit (`ulimit -n`) and run the command again`` |

Dans ces messages, un nom de la source qui n'a pas été validé est affiché avec ses caractères de contrôle et ses barres obliques inverses échappés, et coupé après 1 024 caractères ; `invalid source path:` affiche le nom entre guillemets, échappé. Les caractères de format qui ne sont pas des caractères de contrôle (les surcharges bidirectionnelles, par exemple) ne sont ni refusés ni échappés.

Retirer le lien ou fermer l'éditeur qui tient le verrou, réduire l'entrée, ou attendre la fin de l'outil qui écrit dans la source ; puis réessayer. Pour un refus de fichier d'ignore, utiliser le CLI construit à partir de cette distribution ou retirer le fichier d'ignore par Dockerfile, comme le dit le message. Pour la limite de fichiers ouverts, la relever dans ce shell (`ulimit -n <number>`, dans la limite dure) et relancer : une préparation tient environ deux descripteurs par niveau de profondeur du répertoire sélectionné le plus profond, un par composant du chemin de la source et une vingtaine de plus, quel que soit le nombre de répertoires sélectionnés. Pour un répertoire source indisponible, lire la cause : sous Linux, les répertoires situés au-dessus de la source sélectionnée n'ont besoin que de la permission de traversée pour le compte qui lance le CLI, tandis que le répertoire source et les répertoires que la commande parcourt doivent être lisibles ; sous macOS, chaque répertoire de `/` jusqu'à la source sélectionnée doit aussi être lisible, sinon `Permission denied` suit sous `Caused by:`.

**Une copie qui a changé.** Si quoi que ce soit sous `context/` a été créé, modifié ou supprimé pendant l'utilisation de la copie, y compris par le CLI du moteur lui-même écrivant dans son contexte de build ou à côté du fichier Compose, la commande échoue. Avant la commande du moteur : `the prepared copy changed before docker build started (<detail>); docker build was not started`. Après elle : `the prepared copy changed while docker build was running (<detail>); the CLI process exited (<status>) but its result is not bound to the reported digest`. Lorsque cette commande a aussi été arrêtée par un signal ou à son délai, le message d'arrêt vient en premier et se termine par `The prepared copy also changed while it was running (<detail>)`. `<detail>` nomme ce qui a été constaté : `unexpected entry in the prepared copy: <path>`, `prepared copy content changed: <path>`, `prepared copy entry disappeared: <path>`, `prepared copy directory changed: <path>`, `prepared Compose environment file changed` ou, lorsque la copie elle-même a été déplacée ou remplacée, `control directory path identity changed; reopen the intended private configuration` ; le motif donné par la politique des fichiers privés peut le suivre, par exemple `private file exceeds the configured size limit` pour un fichier qui a grossi. Dans les deux derniers cas, la commande du moteur a déjà été exécutée et une image ou un changement de la pile peut exister. Sauf si un avertissement indique que la copie n'a pas été supprimée, elle n'existe plus quand vous lisez le message et ne peut pas être inspectée : chercher ce qui écrit sous `<TMPDIR>/ai-guard-distribution-*` pendant la commande, vérifier le résultat du moteur, puis relancer.

**Une commande qui n'a pas été lancée.** Tout autre refus entre la préparation et le lancement de la commande du moteur se lit `docker build was not started` (ou `podman compose was not started`, …), avec son motif sous `Caused by:` : par exemple un programme du moteur qui a changé depuis son approbation par le CLI, ou qui ne peut pas être exécuté (`docker is unavailable; install and start a container runtime. No local fallback exists`). La copie préparée est supprimée et rien n'a été lancé ; corriger la cause et relancer.

**Ce que cela établit et n'établit pas.**

- La copie est une suite de lectures stables, fichier par fichier, suivie d'une passe de confirmation ; ce n'est pas un instantané atomique de l'arbre. Un original est comparé par périphérique, inode, longueur, mode et ses deux horodatages : une réécriture qui conserve la longueur d'un fichier et tombe dans la précision de ces horodatages n'est pas vue, et un changement antérieur à la préparation est simplement ce qui est copié.
- Le propriétaire, les modes, le nombre de liens et le système de fichiers de la source ne sont pas jugés. `shared_writable` compte seulement les originaux que le groupe ou les autres peuvent écrire ; rien d'autre n'est rapporté à leur sujet. Une copie ne rend pas fiable une source qui ne l'est pas. Le compte qui lance le CLI, root et les administrateurs du stockage et du moteur restent de confiance.
- Le CLI du moteur reçoit ces chemins ; il n'y est pas confiné. Sa propre configuration et ses plugins sont lus comme auparavant.
- Cela s'applique seulement aux commandes CLI `build`, `up` et `down`. Une commande directe `docker build`, `podman build` ou Compose lit les chemins que vous lui donnez.
- La liste de build ne connaît que `.dockerignore`. `.containerignore` et `images/<name>.Dockerfile.containerignore` n'en font pas partie : ils ne sont ni copiés ni comparés, donc le moteur ne les reçoit pas par la commande CLI `build`. Le traitement de ces noms par un constructeur, et le fait qu'il applique un fichier d'ignore par Dockerfile à la place de `.dockerignore`, n'ont pas été observés sur un vrai moteur.
- La suppression de la copie se fait par nom après un contrôle d'identité. C'est un contrôle, pas un verrou : entre la comparaison et la suppression, un processus du compte du CLI ou de root peut encore échanger le répertoire ; les autres comptes ne le peuvent pas sous la politique du répertoire temporaire. L'identité enregistrée est celle de ce que le chemin nommait juste après la création, pas celle d'un descripteur.
- Métadonnées Compose après `up` (`NOT_RUN` sur un vrai fournisseur). `up` remet à Compose un fichier et un fichier env situés sous `<TMPDIR>/ai-guard-distribution-<random>/` et supprime ce répertoire à la fin de la commande. Un fournisseur qui enregistre ses fichiers de configuration, son répertoire de travail et son fichier env dans les labels du projet nomme alors des chemins qui n'existent plus, et, sous le `/tmp` par défaut de Linux, un autre compte local peut créer plus tard un répertoire de ce nom. ai-guard ne lit jamais ces labels : `down` prépare une nouvelle copie et s'adresse au projet par son nom, et les fichiers Compose livrés ne contiennent aucun chemin relatif. Un outil tiers ou une commande manuelle qui relirait le chemin porté par un label lirait ce qui s'y trouve ; aucun n'a été exécuté. S'adresser à la pile par `--project-name agentsandbox` ou par le fichier Compose du checkout, comme dans [suivre une pile démarrée par `up` du CLI](#cli-up-stack), jamais par le chemin d'un label, et, sur un hôte partagé avec d'autres comptes locaux, lancer `up` avec un `TMPDIR` privé (appartenant au compte, mode `0700`).
- Acceptation de `--env-file` par le fournisseur Compose (`NOT_RUN`). Les commandes CLI `up` et `down` ajoutent `--env-file <empty prepared file>` à la commande Compose ; avec Podman, la commande entière est transmise au fournisseur externe. Aucun fournisseur n'a été exécuté, et la fixture qui en tient lieu accepte n'importe quel argument. Les commandes Podman directes de ce guide utilisent déjà `podman compose --env-file` : c'est la même exigence envers le fournisseur, désormais nécessaire aussi au CLI. Si un fournisseur rejette l'option, `up` et `down` échouent avec le seul message `podman command failed (<status>)` ; la sortie d'erreur du fournisseur est capturée mais [pas affichée](#engine-output).
- Descripteurs. Exécuté sous Linux : 300 répertoires sélectionnés sont construits sous une limite de 128 descripteurs, et une chaîne sélectionnée profonde de 40 niveaux est refusée sous une limite de 64 avec le message qui nomme la limite. Calculé, non mesuré : environ 170 descripteurs pour la source acceptée la plus profonde (64 niveaux), sous la limite souple habituelle de macOS, 256.
- Répertoires situés au-dessus de la source. Exécuté sous Linux : une source placée sous un répertoire que le compte peut seulement traverser est préparée, avec un répertoire de ce même compte. `NOT_RUN` : un répertoire appartenant à un autre compte, et macOS, où une telle source reste refusée. Sur toute plateforme, le répertoire temporaire et, pour `up` et `down`, le répertoire de configuration exigent des ancêtres lisibles (politique de répertoire privé).
- Le comportement a été exercé avec des processus de fixture Rust tenant lieu de CLI du moteur, sous Linux ARM64, y compris une commande lancée sans `TMPDIR` sous le `/tmp` de ce système, appartenant à root avec sticky bit. Un vrai moteur Docker ou Podman, un vrai fournisseur Compose, macOS et un second compte local écrivant dans la source ou le répertoire temporaire sont `NOT_RUN`. En particulier, aucun vrai CLI de moteur n'a été observé lisant un contexte en `0700`/`0600`, et aucun vrai fournisseur Compose n'a été observé avec un `--env-file` vide et sans `--project-directory`.
- Documenté d'après le code, sans test exécuté : les refus du répertoire temporaire pour un système de fichiers non pris en charge (ZFS, NFS, FUSE), un autre propriétaire, une ACL et un chemin non UTF-8 ; un lien sur `deploy` ou `deploy/<engine>` (le cas `compose.yaml` a été exécuté) ; et les messages `source file changed while it was being read`, `source input is unavailable:`, `source input cannot be read:` et `source enumeration failed`. Par le binaire du CLI, `<engine> <verb> was not started` n'a été exécuté que pour une copie altérée avant le lancement ; ses autres causes l'ont été au niveau du consommateur retenu et sur des fins construites.

## Créer la configuration de la pile

L'[exemple local](../../../.env.exemple.local) règle l'interpolation Compose. Créer une **nouvelle** copie privée ; le sous-shell ci-dessous refuse d'écraser un fichier existant.

```sh
(
  umask 077
  set -C
  cat .env.exemple.local > "$AIGUARD_CONFIG/local.env"
)
```

Ouvrir ce nouveau fichier dans un éditeur. Garder `AGENTSANDBOX_CONFIG_DIR` identique au répertoire passé à `init`. `COMPOSE_PROJECT_NAME` identifie la pile et son volume d'audit nommé. `AI_GUARD_CONTROL_IMAGE` choisit l'image de contrôle ; la construction précédente crée `agentsandbox-control:local`. Un autre nom exige une image construite ou acquise par votre propre distribution de confiance.

`AI_GUARD_GATEWAY_PORT` et `AI_GUARD_DASHBOARD_PORT` modifient les ports hôte, toujours liés à `127.0.0.1`. Les ports conteneur publiés restent `8787` et `4000` ; le port de contrôle `8788` reste interne comme décrit ci-dessous. Après un changement, adapter aussi `AIGUARD_GATEWAY` et `AIGUARD_DASHBOARD` dans le shell. Le CLI Rust ne lit pas ce fichier dotenv. Ne jamais le charger par `source` ; le passer explicitement à Compose. Les variables du shell peuvent prendre priorité sur l'interpolation Compose : chercher une valeur héritée lorsqu'un réglage paraît incorrect.

Lancer les services Docker :

```sh
docker compose --env-file "$AIGUARD_CONFIG/local.env" -f deploy/docker/compose.yaml up -d
docker compose --env-file "$AIGUARD_CONFIG/local.env" -f deploy/docker/compose.yaml ps
"$AIGUARD_BIN" healthcheck --url "$AIGUARD_GATEWAY/healthz" --url "$AIGUARD_DASHBOARD/healthz"
```

Ou la variante Podman :

```sh
podman compose --env-file "$AIGUARD_CONFIG/local.env" -f deploy/podman/compose.yaml up -d
podman compose --env-file "$AIGUARD_CONFIG/local.env" -f deploy/podman/compose.yaml ps
"$AIGUARD_BIN" healthcheck --url "$AIGUARD_GATEWAY/healthz" --url "$AIGUARD_DASHBOARD/healthz"
```

Conserver le même compte moteur, contexte/endpoint, fichier env et fichier Compose. Utiliser ce parcours Compose direct pour les ports ou noms de projet personnalisés ; le parcours CLI `up/down` possède son propre projet fixe et ses URL de disponibilité par défaut. Les commandes CLI `up` et `down` remettent à Compose une [copie privée](#prepared-source) de `deploy/<engine>/compose.yaml` et un `--env-file` vide, sans `--project-directory`, dans un environnement qui porte `AGENTSANDBOX_CONFIG_DIR` issu de `--config-dir` et aucune des variables Compose de votre shell : le répertoire de cette copie ne contient pas de `.env`, donc un `.env` placé à côté de `deploy/<engine>/compose.yaml` n'est pas lu par ces deux commandes et les valeurs par défaut du fichier Compose s'appliquent. Les commandes directes `docker compose --env-file …` et `podman compose --env-file …` ci-dessus sont inchangées. Le traitement des arguments du CLI par un vrai fournisseur Compose n'a pas été vérifié.

<a id="cli-up-stack"></a>

### Suivre une pile démarrée par `up` du CLI

La commande CLI `up` crée sa pile sous le nom de projet Compose fixe `agentsandbox`, à partir d'une copie temporaire de `deploy/<engine>/compose.yaml` et d'un fichier env vide, tous deux supprimés dès la fin de la commande Compose, avant l'attente des URL de disponibilité. La seule variable qu'elle donne à Compose est `AGENTSANDBOX_CONFIG_DIR`, le chemin absolu de `--config-dir`. Pour cette pile, « la même configuration » désigne donc ce nom de projet, ce moteur et cette variable, pas un fichier qui existerait encore. Ce qui suit est écrit d'après les arguments et l'environnement que le CLI construit ; aucune de ces commandes n'a été exécutée contre Docker Compose ou un fournisseur Compose de Podman.

- **Projet.** S'adresser au projet par son nom, `--project-name agentsandbox`. Un fournisseur Compose qui enregistre le fichier Compose, le répertoire de travail et le fichier env d'un projet dans les labels de ses conteneurs nomme, pour cette pile, des chemins situés sous le répertoire supprimé `<TMPDIR>/ai-guard-distribution-<random>/` : ne jamais diriger une commande ou un outil vers un tel chemin (voir la limite sur les métadonnées Compose dans [sources préparées de build et Compose](#prepared-source)). Les commandes directes ci-dessus, lancées avec `--env-file "$AIGUARD_CONFIG/local.env"`, s'adressent au projet nommé par le `COMPOSE_PROJECT_NAME` de ce fichier (`ai-guard-local` dans l'exemple), pas à celui-ci.
- **Moteur.** Avec Docker, le CLI vise un seul endpoint, `--host <endpoint>` : la valeur de `--runtime-endpoint` lorsqu'elle a été fournie, sinon l'hôte Docker du contexte courant du compte, que le CLI lit par `docker context inspect --format '{{json .Endpoints.docker.Host}}'` dans un environnement reconstruit qui porte le répertoire personnel du compte et `DOCKER_CONFIG=<home>/.docker`, sans le `DOCKER_HOST` ni le `DOCKER_CONTEXT` de votre shell. `doctor` n'affiche pas cet endpoint. Une commande `docker` directe lancée dans un shell où ces variables sont définies peut s'adresser à un autre moteur : passer l'endpoint explicitement. Avec Podman, le CLI n'ajoute aucune option globale du moteur, et `--runtime-endpoint` est refusé pour ces commandes.
- **Variable.** Une commande directe à laquelle on donne le fichier Compose du checkout a besoin de `AGENTSANDBOX_CONFIG_DIR`, défini sur le même répertoire de configuration : le fichier l'exige (`${AGENTSANDBOX_CONFIG_DIR:?Run agentsandbox init first}`).

Pour Docker, lire l'endpoint comme le fait le CLI lorsqu'aucun `--runtime-endpoint` n'a été fourni. La première commande affiche une chaîne JSON telle que `"unix:///var/run/docker.sock"` ; l'utiliser sans ses guillemets :

```sh
env -u DOCKER_HOST -u DOCKER_CONTEXT -u DOCKER_CONFIG docker context inspect --format '{{json .Endpoints.docker.Host}}'
AIGUARD_ENDPOINT="unix:///the/endpoint/printed/above"
docker --host "$AIGUARD_ENDPOINT" compose --project-name agentsandbox ps
docker --host "$AIGUARD_ENDPOINT" compose --project-name agentsandbox logs --tail 100 gateway dashboard ingress
```

Pour Podman :

```sh
podman compose --project-name agentsandbox ps
podman compose --project-name agentsandbox logs --tail 100 gateway dashboard ingress
```

Lorsqu'un fournisseur a besoin d'un fichier Compose pour trouver le projet, ajouter le fichier du checkout et la variable ; `$AIGUARD_CONFIG` représente le répertoire donné à `up` par `--config-dir` :

```sh
AGENTSANDBOX_CONFIG_DIR="$AIGUARD_CONFIG" docker --host "$AIGUARD_ENDPOINT" compose --project-name agentsandbox --file deploy/docker/compose.yaml ps
AGENTSANDBOX_CONFIG_DIR="$AIGUARD_CONFIG" podman compose --project-name agentsandbox --file deploy/podman/compose.yaml ps
```

Pour arrêter ou recréer cette pile, utiliser les commandes CLI `down` et `up` avec les mêmes `--runtime`, `--runtime-program`, `--runtime-endpoint` et `--config-dir` : chacune prépare une nouvelle copie et s'adresse au projet par son nom. Examiner les logs avant tout partage pour en exclure les secrets.

<a id="http-listener-limits"></a>

### Bornes des connexions et en-têtes HTTP

Le dashboard, l'ingress et le gateway limitent chacun les connexions TCP acceptées à **128 par listener**, avec **15 secondes** pour recevoir les premiers en-têtes complets d'une requête. Au gateway, les listeners public et de contrôle ont des pools de connexions séparés. Les connexions keepalive inactives, SSE et mises à niveau occupent une place jusqu'à la fermeture de leur connexion réelle ; il ne s'agit pas d'une limite de requêtes ou de streams HTTP/2. Lorsqu'un pool est plein, une nouvelle connexion peut attendre avant son acceptation.

La première échéance part de l'acceptation, inclut la détection du protocole et ne redémarre pas à l'arrivée d'octets. Si les premiers en-têtes restent incomplets, la connexion est fermée à cette échéance. Hyper gère les lectures d'en-têtes HTTP/1 suivantes avec le même délai de 15 secondes. Ce contrôle ne couvre pas les nouveaux streams HTTP/2 après la première requête. Une fois les premiers en-têtes acceptés, l'échéance initiale n'expire ni réponse active, ni corps, ni flux SSE ; leurs politiques existantes restent appliquées, dont les bornes de corps et d'échanges de l'ingress ci-dessous.

Ces valeurs de compatibilité sont fixes dans les points d'entrée CLI. Aucune option CLI, variable d'environnement ou valeur dotenv ne permet de les régler. Les appelants Rust peuvent fournir des bornes validées par `ListenerBounds` et `serve_with_listener_bounds` du gateway ; ce sont des interfaces de code, pas des réglages opérateur. Ces valeurs n'ont pas été dimensionnées pour 100 workers ni pour le budget de descripteurs et de mémoire d'un processus. Une borne de sockets n'apporte ni authentification, ni quota tenant, ni garantie anti-DoS.

Si une connexion est fermée, laisser le client en ouvrir une nouvelle et reprendre une lecture autorisée lorsque la capacité est disponible. Avant de répéter un lancement ou une autre mutation dont le résultat est inconnu, réconcilier l'opération initiale. Identifier si le chemin concerné atteint le dashboard, l'ingress ou le listener public/de contrôle du gateway ; utiliser l'état Compose et les logs bornés déjà décrits plus haut, sans déduire d'une fermeture quelle borne a été atteinte. Une fermeture de connexion se distingue des refus HTTP 429 liés aux corps et échanges de l'ingress ci-dessous.

### Bornes du relais HTTP

L'ingress Rust public dispose de budgets indépendants pour **16 corps de requête en cours de lecture** et **64 échanges avec le service amont**. Chaque corps est limité à **256 Kio**, **5 secondes au total** et **2 secondes sans nouveau fragment**, à partir de sa lecture. Le corps complet est lu et son permis de lecture libéré avant de réserver un échange. Un corps connu vide par son état de flux passe sans permis de lecture ; la méthode HTTP ou le `Content-Length` déclaré ne décide pas de ce passage. Les routes publiques conservent leurs destinations fixes et contrôles d'authentification/autorisation amont. Ces bornes sont fixées dans le binaire ; le fichier dotenv et le CLI ne proposent aucun réglage pour les modifier.

Au plus **48 réponses SSE** peuvent retenir des places d'échange simultanément, comptées dans le total de 64. L'ingress reconnaît le véritable `Content-Type` amont avant de retirer les en-têtes hop-by-hop, sans distinction de casse du type et en acceptant les paramètres. Ni chemin de requête, ni `Accept`, ni tenant déclaré ne choisit ce budget. S'il est plein, une nouvelle réponse SSE renvoie **429** avec `Trop de flux simultanés.` ; consommation ou abandon du corps côté client et échec de la pompe libèrent ses permis. Cette limite est fixée dans le binaire.

Un budget de lecture plein peut répondre **429** à une requête avec corps. La seule rétention SSE laisse 16 places du total hors de son budget ; les attentes d'en-têtes amont et les autres réponses partagent toujours le total de 64 et peuvent le saturer. Ces bornes n'assurent ni équité des flux entre tenants ni protection générale contre les floods, et ne qualifient pas l'isolation runtime. Après démarrage acquitté, le moniteur de lecture du runner permet au plus 60 secondes pour les pannes transitoires classées, timeout de la première requête défaillante inclus ; l'échéance d'exécution initiale et un STOP observé restent prioritaires. Les erreurs d'authentification, d'identité, d'audit et les 503 non classés demandent toujours l'arrêt ciblé. Conserver la session enregistrée et la réconcilier après une interruption plutôt que lancer une autre instance pour masquer un résultat inconnu. Voir le [contrat du moniteur](../../engineering/GATEWAY_TRANSIENT_FAILURE_PROPOSAL.md) et ses limites de qualification.

### Transport explicite de contrôle

Les fichiers Compose mis à jour configurent `gateway --control-listen 0.0.0.0:8788` et `ingress --control-upstream http://gateway:8788`. Le flag gateway est facultatif ; celui de l'ingress est requis et n'a aucun repli vers l'amont public. Le port 8788 reste sur le réseau interne existant, sans nouveau port hôte ni credential propre au relais. Les bases Docker/Podman, leurs profils SSO et les deux surcharges TLS du modèle local Docker conservent ce raccord. Préserver données, credentials et bindings hôte loopback existants lors de l'adaptation d'un déploiement.

Le runner et les clients de contrôle typés choisissent explicitement `/control/`, avec la destination dédiée hors pools publics 16/64. Le gateway authentifie l'identité existante avant de lire le corps ou de réserver une des **16 places runner** ou **4 places présence broker** du tenant. Le [contrat de transport](../../engineering/INGRESS_CONTROL_TRANSPORT.md) donne les routes fermées, bornes de corps/réponse et erreurs. Garder `AIGUARD_GATEWAY`, `--gateway` et les valeurs du registre broker comme origines sans chemin ; les clients ajoutent le préfixe. Ne pas ajouter `/control` ni diriger les clients du poste vers le port 8788 réservé aux conteneurs. Aucun champ n'est ajouté aux plans stricts 1/2/3.

Identité de contrôle runner, admission, décisions, lectures/écritures de contrôle typées et faits de contrôle de confiance utilisent ce canal. L'ingestion ordinaire, les consultations publiques et le diagnostic authentifié du protocole restent publics. Le choix du contrôle runner est explicite ; son refus ne se rabat pas sur ce diagnostic. Le client de présence broker utilise ses trois routes bornées, sans acquérir de droit runner. L'outbox broker mixte demande encore une provenance durable et ne peut être promue depuis le kind/source des événements. Mettre à jour ensemble CLI, gateway et ingress : un ancien gateway sans route de contrôle est refusé sans repli public. Le gel SDK v2 passe [22 fonctions logicielles ciblées et la compilation toutes cibles](../../engineering/evidence/2026-11-07/control-clients/README.md), avec avertissements conservés. Les [contrôles composés du contrôle et de la reprise](../../engineering/evidence/2026-11-07/control-recovery-composition/README.md) passent 52 fonctions logicielles et la compilation toutes cibles. La qualification runtime reste ouverte.

`control_operation_capacity_exhausted` est distinct d'un 429 des pools publics ou des quotas d'audit. Une réponse fermée `control_transport_failure` rapporte un résultat **inconnu**, pas un rollback. Préserver l'identité d'opération et réconcilier son reçu existant. Seuls les GET admissibles du moniteur après ACK reconnaissent un DTO de transport validé avec 502/504 cohérent ; erreur de protocole, réponse malformée ou 503 d'audit restent des erreurs. Cela n'ajoute aucun réessai aux mutations, starts, claims ou audits obligatoires ; le callback de démarrage conserve une tentative.

## Ouvrir la vue de contrôle

Ouvrir `AIGUARD_DASHBOARD` dans le navigateur. L'URL gateway sert au CLI et aux API ; `/healthz` indique la disponibilité du service, pas l'autorisation de lancer un worker ni l'authentification auprès d'un fournisseur IA.

Les commandes `init` et Compose de base ci-dessus préparent l'accès technique runner/viewer, pas un compte nominatif. Pour une vue personnelle sans IdP, suivre l'[accès local nominatif](../identity/README.fr.md) : migrer explicitement la base tenant existante, créer un compte puis lui attribuer des ressources réelles avant connexion. Sélectionner un contexte d'administration proposé seulement lorsqu'on y est autorisé. Les ressources personnelles exigent un grant explicite de propriété ou de partage ; l'appartenance au tenant et un nom de session identique ne donnent ni accès ni attribution d'auteur. [Keycloak/OIDC](../sso/README.fr.md) est un choix d'identité distinct, indépendant du fournisseur IA.

Séparer les credentials techniques de cette identité : `viewer` donne son périmètre de lecture configuré et `runner` appartient au contrôle de confiance. Pour une connexion par clé viewer, consulter sa source dans un éditeur privé et saisir la valeur uniquement dans le formulaire de connexion de confiance. Ne jamais placer le credential runner dans un navigateur, un worker ou une conversation. Les événements anciens identifiés seulement par une clé partagée conservent cette provenance ; une connexion ultérieure ne prouve pas leur auteur humain.

Le dashboard présente provenance des sessions, décisions, observations runtime et changements. Connexion périmée, couverture partielle, refus et résultat vide sont des états distincts. Utiliser la dernière observation et le périmètre affichés pour choisir entre actualisation, reconnexion et demande d'accès.

Ouvrir un agent ou une sandbox pour consulter ses workers explicitement liés, puis un worker pour retrouver ses agents et sandboxes correspondants. Les liens utilisent les identifiants exacts de session ou de runtime reçus pour le tenant authentifié ; leur provenance est affichée et une session identique ne prouve ni exécution ni délégation. Le registre workers est paginé : parcourir ses pages avant de conclure qu'aucun enregistrement correspondant n'a été reçu. Une lecture échouée conserve un résultat périmé avec une action de reprise ; ce n'est pas un inventaire vide. Les liens rapprochent projections d'audit et registre, sans interroger le moteur ni découvrir des brokers distants.

Dans la table et le détail workers du parcours technique viewer, **Autorité déclarée du lancement** désigne l’autorité du lancement déclaré ; **Autorité de la dernière inspection enregistrée** désigne celle de la dernière inspection enregistrée, avec son runtime et sa date d’observation. Une autorité absente ou `null` reste inconnue, sans repli du lancement vers une inspection. Les identifiants fournis doivent être des chaînes non vides, conservées exactement et limitées à 256 octets UTF-8 sans caractère de contrôle ; une valeur invalide produit une erreur de contrat. Ces champs n’établissent ni l’hôte ou broker associé, ni une disponibilité actuelle ou un inventaire complet. Voir le [contrat de la vue workers](../../ui.md#workers-quotas-et-allocations).

Un refus 401/403 d'une lecture active ferme la session viewer et retire ses données, y compris les détails ouverts. Se reconnecter avec un credential autorisé ; des actualisations répétées ne rétablissent pas un credential refusé. Une erreur du service est affichée explicitement. Les vues workers, quotas et allocations conservent leurs dernières données réussies avec un avertissement de péremption, dans la même session et le même périmètre, jusqu'à une actualisation réussie.

## Exécuter une première commande

Les commandes suivantes utilisent le credential runner technique conservé dans `AIGUARD_CONFIG`, indépendamment d'une connexion humaine au dashboard. Elles ne créent pas d'initiation ou de délégation humaine authentifiée ; le [contrat d'exécution humaine via le broker](../../engineering/HUMAN_EXECUTION.md) reste proposé. Commencer par une petite commande hors ligne avec la politique stricte et le réseau du worker coupé :

```sh
"$AIGUARD_BIN" run --project "$AIGUARD_PROJECT" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY" --runtime docker --profile strict --network none --timeout 30 -- /usr/bin/id
```

Avec Podman, remplacer `--runtime docker` par `--runtime podman` dans chaque `run`. Conserver l'UUID de session retourné. Examiner la décision et la fin observée dans le dashboard, puis consulter le registre et vérifier la chaîne locale d'audit :

```sh
AIGUARD_SESSION="replace-with-the-returned-session-uuid"
"$AIGUARD_BIN" worker-status --session "$AIGUARD_SESSION" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY"
"$AIGUARD_BIN" verify-audit --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY"
```

`verify-audit` vérifie la chaîne de hash locale du gateway. Distinguer ce résultat des points de contrôle signés indépendants et de l'ancrage externe. Une admission réussie se distingue aussi du démarrage runtime et de la fin d'une commande.

Pour produire un changement volontaire uniquement dans la copie de travail :

```sh
"$AIGUARD_BIN" run --project "$AIGUARD_PROJECT" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY" --runtime docker --profile observe --network none --timeout 30 -- /usr/bin/touch /workspace/ai-guard-example.txt
```

Cette autre session reçoit une nouvelle copie du projet. `observe` ne signifie pas que tous les effets transitifs sont médiés ou compris. Choisir `review` lorsqu'une demande exige une approbation humaine : examiner la demande exacte puis reprendre son hash complet avec `--approve-hash` en la soumettant à nouveau. Des entrées modifiées nécessitent une nouvelle décision ; ne jamais approuver un hash sans rapport pour contourner un refus.

### Arrêt et fin d'un batch

Pour un batch non interactif, les demandes d'arrêt et l'échéance de travail restent prises en compte pendant une inspection du moteur ou l'attente de l'acquittement de démarrage. La fin exige des faits terminaux natifs pour l'instance créée. Le chemin runtime direct valide l'état de cette instance ; l'exécution gérée vérifie aussi l'autorité runtime retenue, la configuration de la sandbox et l'allocation. La sortie du CLI du moteur ou la fermeture d'un tube de sortie ne prouvent pas, seules, la fin du worker.

Dès que ces faits terminaux sont validés, l'échéance de travail est fermée avant d'attendre un éventuel acquittement de démarrage encore en cours. Un timeout ou un arrêt déjà sélectionné garde son motif initial. L'audit obligatoire, le même acquittement en cours et la collecte des entrées/sorties doivent encore se terminer ; cette exécution ne recommence pas un acquittement déjà tenté, même s'il a échoué. Un acquittement lent après une fin prouvée ne crée pas un nouveau timeout de travail. Les faits terminaux manquants, une collecte incomplète et les erreurs restent explicites, avec les données de session et les sorties partielles conservées pour réconciliation. Préserver session, configuration et volume worker ; utiliser le statut et la reprise du worker enregistré avant de tenter un autre démarrage.

Le timeout configuré n'est ni un budget global de temps écoulé ni une garantie d'arrêt temps réel, même réglé à une seconde. Inspection, contrôles d'autorité, audit et drainage peuvent prendre plus longtemps, notamment si le poste ou le moteur est bloqué. Ce comportement batch ne couvre ni les sessions terminal interactives ni le transport du broker armé par un gardien indépendant, et ne rend pas sûre l'annulation externe d'une tâche ou la terminaison brutale du CLI.

## Vérifier l'audit d'un tenant SetupV2

Dans le `tenants.json` de l'installation préparée, repérer le champ `viewer_token_file` du tenant autorisé. Utiliser le fichier privé correspondant tel qu'il est accessible depuis la machine qui exécute le CLI. Le gateway détermine le tenant à partir de ce credential.

```sh
AIGUARD_VIEWER_FILE="/absolute/private/installation/tenants/<tenant>/viewer"
"$AIGUARD_BIN" verify-audit --gateway "$AIGUARD_GATEWAY" --viewer-token-file "$AIGUARD_VIEWER_FILE"
```

Omettre `--viewer-token-file` pour lire `<config-dir>/secrets/viewer`. Un fichier explicite illisible fait échouer la commande. Pour ajouter le rapport signé hors ligne, suivre le [guide des checkpoints](../audit-signatures/README.fr.md) et choisir la base, les checkpoints et l'ancre du même tenant.

## Examiner et exporter les changements

Sélectionner la session qui a créé le fichier et utiliser son UUID. Le parent d'export doit exister et la destination doit être **neuve**, hors du projet original. Ne pas exporter dans une copie de travail existante.

```sh
AIGUARD_SESSION="replace-with-the-file-session-uuid"
AIGUARD_EXPORT="$HOME/ai-guard-export-new"
"$AIGUARD_BIN" export-workspace --session "$AIGUARD_SESSION" --output "$AIGUARD_EXPORT" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY"
```

Examiner les fichiers exportés et leur diff avant de reporter des changements choisis dans le projet. ai-guard ne réécrit pas l'original automatiquement. Un export en erreur n'est pas un diff vide ; si une destination a été publiée avant une erreur ultérieure, l'examiner et la conserver avant de choisir une autre destination neuve.

L'export omet les noms sensibles et de cache exclus à l'import et les métadonnées d'outils, notamment `.husky`, `.envrc`, `.idea`, `.vscode`, `.devcontainer`, `.direnv`, `.pre-commit-config.yaml`, `.gitattributes`, `.gitmodules`, `.mise.toml` et `.tool-versions`. La comparaison couvre variantes de casse et certains alias Unicode des noms réservés ; les noms Unicode ordinaires restent disponibles. Un arbre ayant la structure d'un répertoire Git (`HEAD` avec `objects`/`refs`, ou `HEAD` avec `commondir`) est refusé avant publication, y compris dans un sous-répertoire avec parents implicites. Configuration et hooks `.git` du worker ne sont pas restaurés. Les fichiers réguliers sont écrits en `0644`, même si le worker les avait rendus exécutables. Les entrées ignorées restent soumises aux contrôles de chemin, type, doublons et taille : le filtrage ne rend pas une archive invalide acceptable.

Le manifeste d'export décrit les fichiers et modes publiés ; le scan décrit les modes observés avant export. Les nouvelles observations peuvent donc distinguer un changement de permissions d'un changement de contenu. Un ancien manifeste sans mode conserve une couverture inconnue sur ce point ; un contenu inchangé ne prouve pas des permissions inchangées. `workspace.export.completed` indique `omitted_entries`, nombre d'entrées explicites de l'archive (fichiers et répertoires), sans compter les parents implicites. Un ancien événement sans ce champ ne signifie pas zéro omission.

Le contrôle émet un avertissement générique avec le nombre d'entrées omises, sans leurs noms. Si son écriture échoue, la publication est refusée. **Le contenu exporté reste non fiable.** Certains fichiers d'outils, dont `.cargo/config.toml`, `Makefile` et `package.json`, restent présents. La politique ne neutralise pas tous les outils et ne détecte pas tous les secrets. Examiner les fichiers comme du texte avant d'ouvrir le projet dans un outil pouvant exécuter sa configuration, d'installer des dépendances ou de rétablir volontairement des permissions exécutables.

La copie exclut des noms sensibles connus et des répertoires de dépendances/cache, sans constituer un détecteur universel de secrets. Examiner le contenu source avant import et garder les clés à l'extérieur. Les bornes comprennent 20 000 entrées, 128 Mio au total et 32 Mio par fichier ; liens et chemins dangereux sont refusés. Un dépassement exige de réduire l'entrée ou de faire évoluer une limite approuvée, pas de traiter silencieusement une copie partielle comme complète.

## Diagnostiquer un refus ou une interruption

| Symptôme | Vérification et action |
|---|---|
| Moteur absent, exécutable dangereux ou mauvais endpoint | Lancer `doctor` sous le même compte et répéter les choix explicites `--runtime-program`/`--runtime-endpoint` sur les commandes qui les acceptent. Ne pas exécuter la tâche sur l'hôte en repli. |
| Daemon non rootless refusé | Configurer rootless ou appliquer volontairement l'exception par commande décrite plus haut. |
| `build`, `up` ou `down` refuse une entrée de la source, un fichier d'ignore ou le répertoire temporaire | Rapprocher le chemin nommé et le motif des [sources préparées de build et Compose](#prepared-source) : lien, fichier spécial, borne, changement pendant la préparation, `.dockerignore` différent de celui que le CLI reflète, répertoire illisible au-dessus de la source, limite de fichiers ouverts ou règles de `TMPDIR`. Corriger le checkout ou choisir un `TMPDIR` conforme ; rien n'est réparé automatiquement et la source n'est jamais modifiée. |
| `build`, `up` ou `down` signale `the prepared copy changed` | Quelque chose a écrit dans la copie privée pendant son utilisation, peut-être le CLI du moteur lui-même. La commande du moteur a pu être exécutée : vérifier son résultat, trouver l'écrivain et relancer. Voir les [sources préparées de build et Compose](#prepared-source). |
| `build`, `up` ou `down` se termine par `docker command failed (<status>)` ou `podman command failed (<status>)` | La commande du moteur a échoué d'elle-même. Le CLI n'a rien affiché de sa sortie et la copie préparée est supprimée. Lire [ce que le CLI montre de la sortie du moteur](#engine-output) pour obtenir le diagnostic propre au moteur, puis relancer. |
| `build`, `up` ou `down` refuse de démarrer avec `SIGCHLD is ignored in this process` | Le programme qui a lancé le CLI ignore `SIGCHLD` ; rien n'a été préparé et aucune commande du moteur n'a été lancée. Lancer la commande avec la disposition par défaut : voir [interrompre un build ou une commande Compose, ou dépasser son délai](#interrupt-build-or-compose). |
| `build`, `up` ou `down` interrompu ou au-delà de son délai | Le message dit ce qui a été observé du processus CLI local du moteur ; voir [interrompre un build ou une commande Compose, ou dépasser son délai](#interrupt-build-or-compose). Vérifier l'état propre du moteur, puis relancer la même commande. Un `up` interrompu peut laisser une pile partielle : relancer `up` ou lancer `down`. |
| `termination is unknown`, un avertissement `was not removed` ou `could not be removed`, ou un répertoire `ai-guard-distribution-*` resté dans le répertoire temporaire | Suivre [reprendre après une source préparée conservée](#prepared-source-retained) avant de supprimer quoi que ce soit. ai-guard ne supprime pas ce répertoire. |
| Pile indisponible, ou `stack started but readiness did not pass within 45 seconds; inspect docker compose logs` | Consulter Compose `ps` et `logs --tail 100 gateway dashboard ingress` avec la même configuration : le même fichier env et le même fichier Compose pour le parcours direct, ou le projet `agentsandbox` adressé par son nom pour une pile démarrée par `up` du CLI, comme dans [suivre une pile démarrée par `up` du CLI](#cli-up-stack). Vérifier collisions de ports et projections privées des credentials. Examiner les logs avant toute publication pour exclure les secrets. |
| HTTP 429 via l'ingress | Distinguer les [budgets de lecture/échange](#bornes-du-relais-http) des quotas amont à l'aide du message retourné. Réduire les requêtes parallèles et réessayer une lecture lorsque la capacité est disponible. Vérifier la session enregistrée avant de répéter un lancement. |
| HTTP 408 ou 413 pendant l'envoi d'un corps | Vérifier les délais de 5 secondes au total/2 secondes sans fragment et la limite de 256 Kio. Réduire le contenu ou corriger l'émetteur bloqué ; modifier le fichier dotenv ne relève pas ces bornes. |
| HTTP 503 avec pénurie de descripteurs d'audit (`EMFILE`/`ENFILE`) | Conserver la session et l'identité de l'opération. Une fois les descripteurs disponibles, reprendre une lecture autorisée qui atteint la validation du Store ; consulter la [reprise des descripteurs d'audit](#reprise-des-descripteurs-daudit) avant de rejouer une mutation. |
| Non authentifié ou interdit | Se reconnecter avec l'identité et le tenant/périmètre autorisés. Ne pas remplacer une permission utilisateur manquante par une clé runner. |
| Refus de politique ou review | Lire le motif et le périmètre demandé. Adapter la demande ou obtenir une approbation pour cette demande exacte. |
| Copie ou export refusé | Vérifier taille, liens, nouveauté de la destination et permissions des répertoires. Préserver l'original et tout export déjà publié. |
| Runtime indisponible ou état inconnu | Conserver session, plans et volume. Consulter le statut et réconcilier l'autorité runtime enregistrée ; ne pas créer un autre démarrage pour masquer l'incertitude. |
| Connexion fournisseur ou client indisponible | Suivre le [diagnostic fournisseur](../providers/README.fr.md#diagnostiquer-les-problèmes-fournisseur) ; les contrôles de disponibilité ne valident pas cet accès. |

<a id="interrupt-build-or-compose"></a>

### Interrompre un build ou une commande Compose, ou dépasser son délai

Pendant `build`, `up` et `down`, un `SIGINT` (Ctrl-C), `SIGTERM` ou `SIGHUP` reçu par le processus du CLI est une demande d'arrêt, pas la fin immédiate du CLI. La commande de build ou Compose du moteur s'exécute dans son propre groupe de processus : le CLI relaie donc une fois le signal à ce groupe, attend jusqu'à 10 secondes (moins si le délai propre de la commande expire avant), puis envoie `SIGKILL` au groupe. Un nouveau signal, au moins une seconde après le premier, met fin aussitôt à cette attente ; les signaux reçus pendant la première seconde comptent comme la même demande. Le CLI n'annonce une commande réussie qu'après avoir récolté le vrai statut de sortie du processus CLI du moteur **et** atteint la fin de fichier sur ses deux tubes de sortie. Lorsqu'un arrêt a été demandé et que ces faits sont récoltés, il supprime la [copie préparée](#prepared-source), affiche ce qu'il a observé et se termine avec le statut **1**, pas `128 + signal`. Si l'un de ces faits reste inconnu après la récolte bornée qui suit `SIGKILL`, il rend au contraire une erreur `termination is unknown` et conserve la copie pour la [reprise](#prepared-source-retained), sans annoncer la fin du processus ni de ses consommateurs. Aucune autre commande du moteur de cette invocation n'est lancée : un premier build d'image interrompu n'est pas suivi du second.

| Moment où le signal est arrivé | Message, après `Error: ` | Copie préparée |
|---|---|---|
| Aucune commande de build ou Compose du moteur n'était en cours, par exemple pendant une sonde du moteur | `interrupted by SIGINT: no engine build or Compose command was running` | Supprimée s'il en existait une |
| Pendant la copie de la source | `interrupted by SIGINT while the source was being prepared; no engine build or Compose command was started` | La copie partielle est supprimée |
| Juste avant le lancement de la commande du moteur | `interrupted by SIGINT: docker build was not started` | Supprimée |
| Pendant la commande du moteur, terminée dans le délai d'attente | `interrupted by SIGINT: SIGINT was forwarded to the process group of the CLI process for docker build; that process exited (<status>) and both of its output pipes reached end of file` | Supprimée |
| Même cas, mais `SIGKILL` a été nécessaire | `interrupted by SIGINT: SIGINT was forwarded to the process group of the CLI process for docker build, then SIGKILL was sent; that process exited (signal: 9 (SIGKILL)) and both of its output pipes reached end of file` | Supprimée |
| La fin n'a pas pu être établie dans les 2 secondes suivant `SIGKILL` | `docker build termination is unknown: …` | **Conservée** : voir [reprendre après une source préparée conservée](#prepared-source-retained) |

Les messages nomment le signal reçu et la commande du moteur (`docker build`, `podman compose`, …). Un signal déjà ignoré au lancement du CLI, sous `nohup` par exemple, reste ignoré : le CLI poursuit et ne relaie rien. Lorsque la préparation elle-même se termine sans résultat, ce qu'aucune entrée connue ne provoque, le message est `the source preparation ended without a result; no engine build or Compose command was started` : chercher une copie restante comme pour [un répertoire laissé par un CLI tué](#prepared-source-retained), puis relancer.

**Un processus qui ne peut pas attendre ses enfants.** `build`, `up` et `down` refusent de démarrer dans un processus dont `SIGCHLD` est ignoré (hérité d'un superviseur, d'un script ou d'un lanceur de CI qui l'ignore) ou porte `SA_NOCLDWAIT` : le statut d'attente d'un processus CLI du moteur ne pourrait pas être obtenu, donc la fin d'une commande du moteur ne pourrait pas être établie.

```text
Error: SIGCHLD is ignored in this process, inherited from the program that started this CLI: the wait status of an engine CLI process could not be obtained, so the end of an engine command could not be established. Start this CLI with the default SIGCHLD disposition. Nothing was prepared and no engine command was started
```

Le statut de sortie est 1. Rien n'est copié, `up` ne crée ni ne rafraîchit le répertoire de configuration et aucune commande du moteur n'est lancée ; la disposition est lue, jamais modifiée. La variante pour le drapeau commence par `SIGCHLD has SA_NOCLDWAIT set in this process`. Lancer la commande depuis un programme qui laisse `SIGCHLD` à sa disposition par défaut, un shell interactif par exemple, ou corriger le lanceur. Le `env` de GNU coreutils peut la rétablir pour une commande, `env --default-signal=CHLD "$AIGUARD_BIN" build --runtime docker --source .` ; cet exemple n'a pas été exécuté dans le cadre de cette validation. Les autres commandes du CLI ne lisent pas cette disposition.

**Délais.** Chaque build d'image est borné à 30 minutes et chaque commande Compose à 180 secondes. Ces deux délais sont figés dans le binaire : aucune option, variable ou réglage dotenv ne les modifie. À l'échéance, `SIGKILL` est envoyé au groupe de processus et le résultat est rapporté une fois le même statut et les mêmes fins de fichier obtenus :

```text
Error: docker compose exceeded its 180 s bound: SIGKILL was sent to the process group of the CLI process for docker compose; that process exited (signal: 9 (SIGKILL)) and both of its output pipes reached end of file
```

Lorsque le CLI du moteur était déjà sorti et qu'un processus lancé par lui a tenu un tube de sortie ouvert jusqu'à l'échéance, le statut entre parenthèses est celui du CLI, par exemple `exit status: 0`, et le message ajoute `The CLI process had already exited when SIGKILL was sent; the command was not collected earlier, usually because a process it started kept an output pipe open`. Ce processus appartient au groupe signalé et est tué avec lui. La commande est tout de même rapportée en échec, et un `build` ne passe pas à sa seconde image.

<a id="engine-output"></a>

**Ce que le CLI montre de la sortie du moteur.** Pendant qu'une commande de build ou Compose du moteur s'exécute, le CLI n'affiche rien de sa sortie : les deux flux sont capturés. Après une commande réussie, il affiche la sortie standard de la commande, 256 Kio au plus ; un log de build plus long est coupé à cette taille et la coupure est annoncée par `Build log was truncated to 256 KiB; runtime reported successful completion.`, et une sortie Compose plus longue est coupée sans annonce. La sortie d'erreur du moteur est capturée, 64 Kio au plus, et n'est jamais affichée. Après un échec, un délai dépassé ou une interruption, aucun des deux flux n'est affiché. Une commande qui a échoué d'elle-même n'est rapportée que par son statut de sortie :

```text
Error: docker command failed (exit status: 1)
```

Ces tailles sont elles aussi figées dans le binaire. Pour lire le diagnostic propre au moteur, lancer directement la commande du moteur depuis le checkout, avec `podman` à la place de `docker` pour Podman, et avec le `--host <endpoint>` et les paires `--build-arg` que le CLI aurait employés :

```sh
docker build --file images/control.Dockerfile --tag agentsandbox-control:local .
docker build --file images/worker.Dockerfile --tag agentsandbox-worker:local .
```

Une commande directe lit le checkout vivant et non une copie préparée, n'applique aucun des contrôles des [sources préparées de build et Compose](#prepared-source) et n'a pas été exécutée dans le cadre de cette validation. Un build qui demande plus de 30 minutes peut rencontrer de nouveau le même délai lorsque la commande CLI est relancée telle quelle ; un build direct du moteur n'a pas ce délai.

**Que faire ensuite.** Le statut de sortie et le message décrivent seulement le processus CLI local du moteur. Vérifier l'état propre du moteur sous le même compte et le même endpoint (`docker image ls` et `docker ps --all`, ou `podman image ls` et `podman ps --all` ; pour une pile démarrée par `up`, les commandes de [suivre une pile démarrée par `up` du CLI](#cli-up-stack)), puis relancer la même commande avec les mêmes options. `build` reconstruit les deux images. Un `up` interrompu ou arrêté à son délai peut laisser la pile partiellement ou entièrement démarrée : relancer `up`, qui recrée les services, ou lancer `down`. Relancer un `down` interrompu. Ne supprimer ni image, ni volume, ni configuration pour reprendre.

**Limites.**

- Ce sont des faits locaux : le statut de sortie du processus CLI du moteur, la fin de fichier sur ses deux tubes de sortie et les signaux envoyés par le CLI. Rien n'est établi sur le travail d'un daemon du moteur, ni sur un processus lancé par la commande du moteur qui ne détient plus aucun de ses deux tubes de sortie, qu'il soit resté ou non dans le groupe de processus de la commande : lorsque la commande se termine d'elle-même ou sur le signal relayé, aucun autre signal n'est envoyé et le groupe n'est pas inspecté ; un tel processus peut donc encore s'exécuter, et lire la copie, quand le CLI la supprime. Après `SIGKILL`, il en va de même pour un processus qui a quitté le groupe ou que votre compte ne peut pas signaler. Exécuté avec des processus de fixture sous Linux ARM64, au niveau du consommateur retenu : un assistant lancé avec sa sortie sur `/dev/null`, dans le groupe de processus de la commande, est vivant après la récolte de la commande et la libération de son entrée, aussi bien lorsque la commande se termine d'elle-même que lorsqu'elle se termine sur un `SIGINT` relayé que l'assistant ignore. Qu'un tel processus lise la copie quand le CLI la supprime est la conséquence lue dans le code.
- Un CLI lui-même tué (`SIGKILL`, plantage, coupure de courant ; `SIGQUIT` garde son action par défaut) ne peut ni relayer un signal ni nettoyer. La commande du moteur, dans son propre groupe de processus, ne reçoit aucun signal d'ai-guard, et la copie reste sans marqueur : voir [reprendre après une source préparée conservée](#prepared-source-retained).
- Une suspension par le contrôle des tâches (Ctrl-Z) ne suspend que le CLI ; la commande du moteur continue. Tant qu'il est suspendu, le CLI ne lit rien de la sortie de la commande du moteur, qui peut donc se bloquer dès qu'un tube est plein, et le délai de 30 minutes ou de 180 secondes continue de courir ; à la reprise du CLI, une commande qui a dépassé son délai est tuée aussitôt et rapportée par `exceeded its … bound`. Déduit du code, non exécuté.
- La préparation a des bornes d'entrées et d'octets mais aucune borne de temps. Une demande d'arrêt (`SIGINT`, `SIGTERM`, `SIGHUP`) reçue pendant la préparation prend effet entre deux entrées de la copie ou deux contrôles de la confirmation ; un signal répété n'y provoque aucune escalade. Si une lecture de la source ou une écriture sous `TMPDIR` ne rend pas la main (réseau qui ne répond plus, système de fichiers FUSE ou amovible), le CLI ne se termine pas tant que cet appel système n'est pas revenu. Seul `SIGKILL` ou `SIGQUIT` y met fin, en laissant un répertoire `ai-guard-distribution-*` partiel, sans manifeste ni marqueur : le traiter comme [un répertoire laissé par un CLI tué](#prepared-source-retained). Déduit par lecture ; `NOT_RUN`.
- À l'échéance d'une commande, `SIGKILL` est envoyé à tout le groupe de processus du CLI du moteur, y compris lorsque ce processus CLI était déjà sorti avec le statut 0 et que seul un processus lancé par lui tient encore un tube de sortie ; la commande est alors rapportée en échec. Un signal de l'opérateur reçu dans cet état est relayé à ce même groupe : avec des processus de fixture, le détenteur se termine sur ce signal et la commande est récoltée avec le statut de sortie 0, sans `SIGKILL`. Qu'un vrai moteur laisse un tel processus après `up --detach` (premier candidat : Podman rootless avec un fournisseur Compose externe) est `NOT_RUN` ; si c'est le cas, `up` est rapporté en échec au délai de 180 secondes et ce processus est tué avec le groupe.
- La commande du moteur n'est pas dans le groupe de processus d'avant-plan du terminal et son entrée standard est `/dev/null`. Un assistant qui interroge le terminal, un assistant de credentials par exemple, n'est pas pris en charge par ces commandes et, d'après les règles du contrôle des tâches, resterait suspendu jusqu'à l'échéance : déverrouiller ou se connecter au moteur au préalable. Ce point est déduit, pas exécuté.
- Si le signal arrive pendant que `up` attend les URL de disponibilité, la commande Compose est déjà terminée : le message est le premier du tableau et les services lancés par Compose restent en l'état. Ce point découle du chemin de code et n'a pas été exercé par un test.
- Le refus `SIGCHLD` a été exécuté sous Linux ARM64 pour un `SIGCHLD` ignoré hérité (`down` avec un moteur de fixture de type Podman, `build`, `up`). Non exécuté : `SA_NOCLDWAIT` ; macOS, où il n'a pas été déterminé si un `SIGCHLD` ignoré le reste à travers `exec`.
- macOS `NOT_RUN` : il n'a pas été établi sous Darwin que `kill(2)` vers un groupe de processus dont le seul membre est le processus CLI du moteur, sorti et non récolté, est accepté (Linux l'accepte). Si Darwin le refuse, une terminaison inconnue se lit `<SIGNAL> was forwarded to process group N, SIGKILL was not sent` (ou `no signal was sent to process group N` lorsque le signal relayé a été refusé lui aussi), sans `2 s later`, et `RETAINED-UNKNOWN.json` enregistre `kill_sent: false` alors que `kill(2)` a été appelé. Une commande récoltée ne rapporte que les signaux acceptés (`no signal was accepted by the process group of the CLI process for …` lorsqu'aucun ne l'a été). La conservation n'est pas affectée : rien, chez le propriétaire de la commande, ne dépend de la valeur de retour de `kill`.
- Exercé avec des signaux envoyés à l'identifiant de processus du CLI et des processus de fixture Rust tenant lieu de CLI du moteur, sous Linux ARM64, y compris l'attente réelle de 10 secondes, une commande qui a échoué d'elle-même et un log de build de plus de 256 Kio. Le délai Compose réel de 180 secondes est exercé par un seul test, ignoré par défaut : il est `NOT_RUN` dans `cargo test` et dans toute sélection par défaut, et s'exécute par son nom en trois minutes environ :

  ```sh
  cargo test --test source_lifecycle -- --ignored --exact paths::r15_compose_command_whose_output_is_held_past_its_real_bound_is_killed_and_reported --nocapture --test-threads=1
  ```

  `NOT_RUN` : un Ctrl-C délivré par un vrai terminal à un groupe de processus d'avant-plan, un vrai moteur Docker ou Podman (réaction de son CLI au signal relayé ou à `SIGKILL`, arrêt du travail côté daemon), macOS et le délai de build de 30 minutes.
- Documenté d'après le code, sans test exécuté par le binaire du CLI : `<engine> <verb> was not started` après une interruption, la variante `SA_NOCLDWAIT`, le message d'une préparation qui se termine sans résultat, et le message d'une commande tuée à son échéance alors que son processus CLI s'exécutait encore, montré dans le bloc ci-dessus (le test du délai réel exécute la variante où le processus CLI était déjà sorti). Le premier et le dernier ont été exécutés au niveau du consommateur retenu et sur des fins construites.

<a id="prepared-source-retained"></a>

### Reprendre après une source préparée conservée

Lorsque la fin d'une commande du moteur n'a pas pu être établie, la commande se termine avec le statut 1 et nomme le répertoire qu'elle conserve. Le groupe de processus et le chemin ci-dessous sont des valeurs d'exemple :

Dans ces messages, les caractères de contrôle et les antislashs des chemins sont échappés. Un chemin de plus de 1 024 caractères est abrégé par `…` ; ce sont des règles de présentation, sans modification du nom du répertoire ni de la source enregistrée dans `manifest.json`. Ne pas employer un message échappé ou abrégé comme chemin de fichier : identifier le vrai répertoire sous le `TMPDIR` effectif avant de renseigner `AIGUARD_RETAINED` ci-dessous. Pour les commandes suivantes, un chemin temporaire court avec des noms imprimables facilite cette reprise.

```text
Error: docker build termination is unknown: SIGTERM was forwarded to process group 4242, then SIGKILL was sent and, 2 s later, stdout was still open and stderr was still open. The prepared copy is kept at /tmp/ai-guard-distribution-AbC123 and is not removed by ai-guard; see docs/setup/local/README.md#prepared-source-retained
```

Cela signifie que le CLI n'a pas pu établir la fin du processus CLI du moteur dans son temps d'observation : ce processus n'a pas été récolté, son statut de sortie n'a pas pu être obtenu, ou l'un de ses tubes de sortie était encore ouvert, le plus souvent parce qu'un processus lancé par lui s'exécute encore hors du groupe de processus signalé. Un processus peut donc encore lire la copie. ai-guard conserve le répertoire, n'envoie plus aucun signal et ne supprime rien automatiquement dans cet état, ni à la sortie du CLI ni lors d'une commande ultérieure. Un nettoyeur système de fichiers temporaires, s'il est configuré, agit indépendamment d'ai-guard.

La même procédure s'applique à `<engine> <verb> ended without a collected result. The prepared copy is kept at <path> and is not removed by ai-guard`, une défaillance interne du CLI pour laquelle la présence d'un marqueur n'est pas établie : sauter l'étape 2 lorsqu'il n'y en a pas.

1. Lire pourquoi la copie a été conservée. `manifest.json` donne la portée (`build` ou `compose`), la source et l'empreinte ; le marqueur donne les faits de l'arrêt : `version`, `written_at` (UTC), `control_pid`, `runtime`, `verb`, `leader_pid` et `process_group` (le même nombre), `stop_reason`, `forwarded_signal`, `kill_sent`, `leader_reaped`, `wait_uncertain`, `stdout_open`, `stderr_open`, `scope` et `sha256`. Il ne contient ni argument ni environnement.

   ```sh
   AIGUARD_RETAINED="/absolute/path/of/the/identified/directory"
   cat "$AIGUARD_RETAINED/RETAINED-UNKNOWN.json"
   cat "$AIGUARD_RETAINED/manifest.json"
   ```

   Si le marqueur lui-même n'a pas pu être écrit, le CLI a affiché `warning: RETAINED-UNKNOWN.json could not be written in <path>: <error>` ; le répertoire est conservé tout de même et le message porte le groupe de processus.

2. Lister les processus encore présents dans ce groupe de processus. Remplacer `4242` par le `process_group` enregistré. Le résultat est souvent vide, car le détenteur habituel a quitté le groupe.

   ```sh
   ps -A -o pid,pgid,stat,command | awk -v group=4242 '$2 == group'
   ```

3. Lister les processus dont la ligne de commande nomme la copie, ou qui tiennent un fichier ou un répertoire de travail sous elle. Ignorer la ligne de `grep` lui-même. `lsof` peut devoir être installé et, sans privilèges, ne montre que les processus de votre compte. Lorsqu'il n'est pas installé, sous Linux, les liens `/proc/<pid>/cwd` et `/proc/<pid>/fd/*` d'un processus que vous pouvez inspecter montrent son répertoire de travail et ses fichiers ouverts.

   ```sh
   ps -A -o pid,pgid,stat,command | grep -F -- "$AIGUARD_RETAINED"
   lsof +D "$AIGUARD_RETAINED"
   ```

4. Vérifier l'état propre du moteur sous le même compte et le même endpoint : `docker image ls` et `docker ps --all`, ou `podman image ls` et `podman ps --all` ; pour Docker, nommer l'endpoint visé par le CLI, comme dans [suivre une pile démarrée par `up` du CLI](#cli-up-stack). ai-guard n'en a rien observé ; l'image ou le changement de la pile peut exister ou non.

5. Si l'étape 2 ou 3 liste un processus, quelque chose lit peut-être encore la copie. `ps -A` liste les processus de tous les comptes : ce n'est donc pas nécessairement l'un des vôtres. Le laisser finir ou, une fois identifié comme un processus que vous pouvez arrêter, l'arrêter avec vos outils habituels, puis refaire ces deux étapes. Ne pas signaler un numéro de groupe de processus tiré d'un ancien marqueur sans le vérifier d'abord : les identifiants sont réutilisés.

6. Lorsque les deux étapes ne listent rien, supprimer ce répertoire exact et rien d'autre. Ces contrôles ne montrent que les utilisateurs actuels ; aucun ne prouve que rien ne lira le répertoire plus tard. Ne jamais supprimer d'images, de volumes, la source ou la configuration pour réparer cet état.

   ```sh
   rm -r -- "$AIGUARD_RETAINED"
   ```

7. Relancer la commande d'origine avec les mêmes options.

**Un répertoire laissé par un CLI tué.** Un CLI tué pendant `build`, `up` ou `down` laisse sa copie sans marqueur ni message ; une copie nommée par un avertissement `was not removed` ou `could not be removed` est dans le même état. Vérifier d'abord qu'aucun `build`, `up` ou `down` de votre compte n'est encore en cours : une commande en cours possède un tel répertoire. Chercher une ligne de votre compte dont la commande est `agentsandbox` avec `build`, `up` ou `down`, et ignorer la ligne de `grep` lui-même :

```sh
ps -A -o pid,user,stat,command | grep -F -- agentsandbox
```

Lister ensuite ce qui reste dans le répertoire temporaire utilisé par cette commande. Nommer ce répertoire explicitement : un préfixe `TMPDIR=… <command>` ne le définit que pour cette commande, donc le `TMPDIR` de votre shell peut désigner un autre répertoire. Lorsque la commande a été lancée sans `TMPDIR`, ce répertoire est `/tmp` sous Linux. Sous macOS, une session définit normalement `TMPDIR` sur un répertoire par utilisateur situé sous `/var/folders`, qu'affiche `getconf DARWIN_USER_TEMP_DIR` ; ce que le CLI y utilise lorsque `TMPDIR` n'est pas défini n'a pas été vérifié, et macOS n'a pas été exécuté.

```sh
AIGUARD_TMP="/absolute/path/of/the/temporary/directory/that/command/used"
ls -ld "$AIGUARD_TMP"/ai-guard-distribution-*
```

Ne considérer que les répertoires appartenant à votre compte : sous un `/tmp` partagé, la liste montre aussi les copies d'autres comptes, que vous ne pouvez ni lire ni supprimer. Lire le `manifest.json` de chaque répertoire si sa préparation était terminée, et lui appliquer les étapes 3 à 6 ; il n'y a aucun groupe de processus enregistré à rechercher.

La conservation et son message ont été exercés avec des processus de fixture Rust sous Linux ARM64. Les commandes d'inspection ci-dessus sont des outils standard et n'ont pas été exécutées dans le cadre de cette validation ; aucun vrai moteur, ni macOS, n'était impliqué.

### Reprise des descripteurs d'audit

Lorsqu'une vérification des fichiers de la base conserve une erreur OS typée `EMFILE` ou `ENFILE`, le Store refuse l'opération sans accusé de réussite ni autorisation d'exécution pour cette requête. Lorsqu'une route du gateway reçoit cette erreur, elle la traduit en HTTP **503** ; une pénurie de descripteurs peut aussi empêcher l'acceptation d'une connexion ou la transmission de la réponse. L'origine de supervision vivante et les connexions sœurs qui la partagent restent suspendues. Une fois les descripteurs disponibles, le prochain accès autorisé qui atteint la validation du Store vérifie automatiquement un même snapshot SQLite complet : identité des fichiers, schéma, liaison tenant, métadonnées/quota, chaîne d'événements, projections de contrôle et témoin vivant contre le recul de l'audit. Seule une validation réussie restaure cette origine partagée ; une connexion sœur peut effectuer la reprise. `/healthz` seul ne la déclenche pas.

SQLite peut avoir déjà committé avant l'échec du contrôle final des fichiers. Conserver l'identité de l'événement ou de l'opération et son contenu exact. Utiliser le chemin de rejeu idempotent existant de cette opération lorsqu'il existe : le résultat durable est réutilisé sans autre événement ni nouveau débit du quota. Une nouvelle identité ou session constitue une nouvelle opération, et un claim de démarrage consommé reste consommé. Une réponse en erreur ne signifie pas qu'aucune mutation n'a été committée.

`EIO`, une identité de fichier substituée, une corruption et un verrou de supervision empoisonné maintiennent l'invalidation permanente de l'origine. Ni l'apparente réparation des fichiers, ni les actualisations, ni `worker-resume` ne peuvent la réactiver ; aucune option opérateur ne le force. Préserver données et références de session pour le diagnostic. La reprise des descripteurs, le statut HTTP, les contrôles de santé et une chaîne d'audit valide ne prouvent ni la disponibilité du moteur ni l'arrêt d'un worker. Continuer à utiliser les observations runtime enregistrées ; les [preuves de validation datées](../../engineering/VALIDATION.md) délimitent la qualification obtenue.

### Réconcilier le worker enregistré

La reprise ordinaire utilise l'autorité moteur enregistrée pour la session :

```sh
"$AIGUARD_BIN" worker-status --session "$AIGUARD_SESSION" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY"
"$AIGUARD_BIN" worker-resume --session "$AIGUARD_SESSION" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY" --wait-seconds 30
```

La reprise réconcilie une session interrompue ; elle n'accorde pas une deuxième utilisation d'un claim de démarrage consommé. Admission interactive et attachement du terminal ont leurs propres exigences de continuité. Si l'autorité conservée demande un exécutable explicite, utiliser le chemin `--runtime-program` approuvé ; `--adopt-runtime-program` lie délibérément une autorité historique et ne constitue pas une option générique de relance.

Pour le parcours direct `run`/`worker-resume`, le scan fiable des fichiers précède la télémétrie déclarée par le worker. Examiner `workspace.diff.completed` et `observer.coverage` : le préfixe de fichiers est borné par 200 événements et le journal privé de 3 Mio ; les changements omis restent explicites. Des affichages masqués égaux conservent des identifiants opaques distincts et sont signalés ambigus dans ce préfixe. La couverture distingue événements préparés et ACK durables ; elle ne promet pas l'exhaustivité de l'activité fournisseur.

Conserver `sessions/<session>.postprocessing-v1.json` dans le `<config-dir>` privé d'origine, avec son plan et le volume du worker. La reprise rejoue les événements retenus avec leurs IDs et contenus d'origine. Sans batch préparé, une session terminale dont `changed_files` est inconnu peut être rescannée avec l'autorité moteur enregistrée ; c'est une nouvelle observation de reprise, pas une reconstruction des sorties historiques. Une capture déclarée perdue reste inconnue même si les fichiers sont récupérés. La CLI reste en échec pour un post-traitement non résolu ou un diagnostic d'exécution externe ; répéter la reprise ne transforme pas une capture absente en activité vide. Préserver le journal en cas de panne d'audit ou de stockage, sans l'éditer ou le supprimer pour forcer un succès.

Le nouveau CLI exige la route runner `POST /control/api/v1/control/events` sur l'origine publiée avant lancement. Mettre à jour CLI et gateway ensemble si elle manque. Ingestion et contrôle ont chacun une fenêtre distincte d'une seconde, de 200 écritures par tenant ; cela ne réserve pas la capacité durable de l'audit. Les événements rejouables réessayent uniquement un refus de débit typé, tandis que le callback obligatoire d'audit de démarrage garde une tentative par POST. Le [contrat API](../../api.md#ingestion-et-faits-du-contrôle--ingestion-and-control-facts) donne les limites exactes. Ces journaux du CLI direct ne migrent pas l'outbox V4 du broker permanent et ne changent pas les plans stricts 1/2/3. Leur présence ne prouve aucune qualification runtime, reprise après panne ou production.

## Arrêter, conserver les données et redémarrer

Demander l'arrêt de chaque worker actif avant celui de la pile de contrôle :

```sh
"$AIGUARD_BIN" worker-stop --session "$AIGUARD_SESSION" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY"
"$AIGUARD_BIN" worker-status --session "$AIGUARD_SESSION" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY"
docker compose --env-file "$AIGUARD_CONFIG/local.env" -f deploy/docker/compose.yaml stop
```

Une demande d'arrêt durable ne prouve pas l'état runtime terminal. Confirmer l'observation ou conserver l'état inconnu pour réconciliation. Avec Podman, utiliser la commande et le fichier Compose correspondants. Redémarrer la pile conservée par la même commande `up -d`, puis contrôler à nouveau disponibilité et sessions.

`stop` conserve conteneurs et volumes nommés. `down` sans `-v` retire conteneurs/réseaux de la pile en conservant l'audit nommé ; aucune de ces opérations ne remplace l'arrêt des workers. Ne pas utiliser `down -v`, `prune`, un autre nom de projet ou l'effacement d'une base pour réparer une erreur. Conserver ensemble `<config-dir>` (credentials, plans, bindings et état de session), volume d'audit de la pile et volumes workers retenus. Sauvegarder SQLite de façon cohérente à l'arrêt, avec l'état WAL pertinent ; ne pas copier une base active comme un fichier ordinaire. Consulter l'[exploitation entreprise](../enterprise/README.fr.md#exploiter-et-reprendre) avant migration ou restauration.
