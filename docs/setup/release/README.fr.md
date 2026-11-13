# Métadonnées de livraison

[English](README.md) · [Index d'installation](../README.fr.md) · [Poste de travail local](../local/README.fr.md) · [Fournisseurs IA](../providers/README.fr.md)

Ce guide permet de décrire une source de build ai-guard dans un manifeste de livraison, un inventaire CycloneDX et un fichier `SHA256SUMS`, de lier à cette description les fichiers construits par nom, taille et SHA-256, puis de tout revérifier plus tard, hors ligne. Deux commandes opérateur du CLI Rust s'en chargent : `agentsandbox release manifest` et `agentsandbox release verify`. Elles ne lancent aucun moteur, n'ouvrent aucune connexion réseau, n'écrivent jamais dans la source et ne remplacent ni ne suppriment jamais un répertoire existant.

**État (13 novembre 2026).** Les deux commandes sont écrites sur la branche `claude/release-integrity-20261112` ; le code n'a pas encore été compilé et aucun de ses tests n'a été exécuté : ce que ce guide affirme de leur comportement est lu dans le code et reste `NOT_RUN` tant qu'une exécution de QA isolée ne l'a pas consigné. Le [dossier de preuves du lot](../../engineering/evidence/2026-11-12/release-integrity/README.md) liste les contrôles statiques effectués, les sélecteurs de tests proposés et tout ce qui reste `NOT_RUN`. Les images portant les labels décrits plus bas n'ont été ni construites ni inspectées (`NOT_RUN`). Ce n'est pas une procédure de livraison qualifiée, et aucune livraison n'a été publiée avec elle.

<a id="scope"></a>

## Ce que les commandes établissent, et ce qu'elles n'établissent pas

`release manifest` prépare `--source` exactement comme [`build` la prépare](../local/README.fr.md#prepared-source) : même contrôle structurel, même copie privée bornée sous `TMPDIR`, même confirmation et même ligne de provenance sur la sortie d'erreur. Elle lit ensuite `Cargo.lock`, `front/package-lock.json`, `Cargo.toml`, `rust-toolchain.toml`, `front/package.json` et chaque `images/*.Dockerfile` dans cette copie vérifiée, contrôlée de nouveau avant et après cette lecture. Elle analyse les deux locks avec des lecteurs stricts, empreinte les fichiers donnés par `--artifact` et publie trois fichiers dans un nouveau répertoire privé. `release verify` lit un tel répertoire, prépare de nouveau la source, dérive de nouveau les deux documents octet pour octet, empreinte les artefacts présentés et rend un résultat.

Un manifeste lie, par SHA-256 :

- les **entrées de build préparées** : le chemin relatif, la longueur et le SHA-256 de chaque fichier de la copie, ainsi que l'empreinte de source que `build` affiche pour les mêmes octets ;
- les **fichiers de lock** et ce qu'ils résolvent : les paquets Cargo et npm, leurs empreintes de registre et leurs graphes de dépendances ;
- les **faits de build déclarés** : nom, version et licence du paquet, toolchain demandée, réglages de `[profile.release]` et, pour chaque recette, ses images de base et ses invocations `cargo build` ;
- les **artefacts déclarés** : le nom de base, la longueur et le SHA-256 de chaque fichier nommé.

Il n'établit **pas** :

- une signature, une attestation de build, une provenance SLSA ou une mesure anti-rollback : quiconque peut écrire dans le répertoire du manifeste peut réécrire ensemble les documents et `SHA256SUMS` ;
- qu'un artefact ou une image a été construit depuis cette source, ni la plateforme cible ou le profil de build de l'un ou de l'autre ;
- un identifiant d'image, une empreinte d'image ou le contenu installé dans une image ;
- un commit approuvé, ni la confiance dans la distribution sélectionnée : une copie ne rend pas fiable une source qui ne l'est pas ;
- une identité de build. Le manifeste porte une [identité des entrées de build](#build-input-identity), qui identifie les entrées d'un build, non un build.

Toute valeur que les documents disent *déclarée* est ce qu'affirme un fichier ou l'opérateur ; rien ne l'authentifie.

<a id="before-you-start"></a>

## Avant de commencer

- **Un CLI construit depuis la distribution décrite.** Le CLI compile sa liste d'entrées de build et le SHA-256 du `.dockerignore` que cette liste reflète ; comme `build`, il refuse de préparer une distribution dont le `.dockerignore` diffère. Un manifeste consigne ce SHA-256 dans `producer.allowlist_sha256`, avec le nom et la version du producteur, et `release verify` ne juge que les manifestes dont le producteur est le sien. Le vérificateur est donc un CLI construit depuis la distribution qu'il vérifie : un CLI modifié peut afficher n'importe quoi, et rien dans ces fichiers ne prouve quel CLI les a écrits. Les vérifications possibles sans lui sont les [vérifications indépendantes](#independent-checks) : `SHA256SUMS` et les dispositions d'empreinte documentées.
- **Plateforme.** Linux ou macOS, avec la source, `TMPDIR`, le parent de sortie et chaque artefact sur un système de fichiers local pris en charge : ext2/3/4, XFS, Btrfs, tmpfs, overlayfs, ramfs ou F2FS sous Linux ; APFS ou HFS+ local avec propriété activée sous macOS. Aucun moteur ni réseau n'est nécessaire.
- **`TMPDIR`.** La copie privée suit les [règles du répertoire temporaire de `build`](../local/README.fr.md#prepared-source) : un répertoire absolu hors de la source, conforme à la politique de répertoire privé, avec de la place pour les entrées de build (256 Mio au plus).
- **Un parent privé pour la sortie.** `--output` désigne un répertoire qui n'existe pas encore, dans un parent qui existe déjà, vous appartient et a exactement le mode `0700`. Chaque répertoire au-dessus de ce parent doit appartenir à root ou à vous et ne pas être inscriptible par le groupe ou les autres, sauf un répertoire appartenant à root avec le sticky bit, comme `/tmp`. Aucun lien symbolique ne peut figurer dans ce chemin, sauf les alias macOS `/tmp`, `/var` et `/etc`, lus comme `/private/tmp`, `/private/var` et `/private/etc`. `mkdir -m 700` crée un tel parent quel que soit votre umask ; si le répertoire existe déjà, `mkdir` échoue : vérifiez vous-même son propriétaire et son mode, car ai-guard ne répare jamais un mode.
- **Une disposition `SIGCHLD` par défaut.** Comme `build`, `up` et `down`, les commandes de livraison s'exécutent dans la portée d'interruption de l'opérateur et refusent de démarrer dans un processus dont `SIGCHLD` est ignoré ou porte `SA_NOCLDWAIT`, avec le [message de ces commandes](../local/README.fr.md#interrupt-build-or-compose). Ce message parle de commandes du moteur, bien que `release` n'en lance aucune.

Exécuter ce qui suit depuis la racine de la distribution ai-guard décrite :

```sh
cargo build --release --locked
AIGUARD_BIN="$PWD/target/release/agentsandbox"
RELEASE_PARENT="$HOME/ai-guard-releases"
mkdir -m 700 "$RELEASE_PARENT"
```

`$HOME` et chaque répertoire au-dessus doivent eux aussi respecter la règle des ancêtres.

<a id="produce"></a>

## Produire un manifeste

```sh
"$AIGUARD_BIN" release manifest --source . --output "$RELEASE_PARENT/inputs-1"
```

| Option | Règle |
|---|---|
| `--source DIST` | Obligatoire, sans valeur par défaut. La distribution ai-guard à décrire, non un projet modifié par un agent. |
| `--output NEW_DIR` | Un nouveau répertoire dont le nom compte 1 à 96 caractères de `[A-Za-z0-9._-]` et ne commence pas par un point, dans un [parent privé](#before-you-start), hors de la source. Un chemin relatif est résolu depuis le répertoire courant ; `..` est refusé. |
| `--artifact FILE` | Répétable, 64 au plus. Un fichier construit à lier par nom de base, longueur et SHA-256 : voir [lier des artefacts](#bind-artefacts). |
| `--codex-version V`, `--claude-code-version V` | Les versions des clients fournisseurs demandées à l'image worker : voir [versions de fournisseurs déclarées](#provider-versions). |
| `--expect-source-digest HEX64` | La valeur `sha256=` d'une ligne de provenance de `build` : voir [relation avec `build`](#relation-to-build). |

La commande procède dans cet ordre et s'arrête au premier refus, avec le statut de sortie 1 :

1. Avant toute préparation : les versions de fournisseurs déclarées, la forme de `--expect-source-digest`, le nombre, les noms et l'unicité des artefacts, l'existence de `--source`, et la sortie (politique du parent, nom nouveau, aucun répertoire intermédiaire laissé par une exécution antérieure, hors de la source). Un refus à ce stade n'affiche aucune ligne de provenance et ne crée rien sous `TMPDIR`.
2. La préparation de la source, qui affiche `Prepared public distribution (build): sha256=<hex> …` sur la sortie d'erreur et refuse ce que `build` refuse.
3. `--expect-source-digest`, comparée à l'empreinte de cette copie.
4. La lecture des déclarations retenues et la lecture stricte des deux locks ; la copie préparée est ensuite libérée.
5. Chaque artefact, lu et empreinté.
6. Les documents, puis leur publication : un répertoire intermédiaire `.<leaf>.release-staging` est écrit dans le parent et synchronisé, renommé en `<leaf>` sans rien remplacer, puis listé et relu.

Le statut de sortie n'est 0 qu'une fois le répertoire publié et relu. Il contient exactement trois fichiers de mode `0600`, dans un répertoire de mode `0700` :

| Fichier | Contenu |
|---|---|
| `release-manifest.json` | Le manifeste, format `ai-guard.release-manifest` version 1, en JSON canonique (la forme indentée de `serde_json` suivie d'un saut de ligne). |
| `sbom.cdx.json` | L'inventaire, CycloneDX 1.6 JSON, cycle de vie `pre-build`. |
| `SHA256SUMS` | `<sha256>  release-manifest.json` et `<sha256>  sbom.cdx.json`, dans cet ordre. |

La sortie standard affiche ensuite un bloc. Illustration du format, non une sortie observée :

```text
release manifest: published <output as given>
manifest   sha256=<hex> format=ai-guard.release-manifest/1 producer=agentsandbox <version>
source     sha256=<hex> layout=ai-guard:public-distribution:v1 files=<n> bytes=<n>
locks      Cargo.lock sha256=<hex> packages=<n>; front/package-lock.json sha256=<hex> packages=<n>
sbom       sbom.cdx.json sha256=<hex> CycloneDX 1.6 pre-build
inputs identity <hex> (build inputs only; declared: CODEX_VERSION=<version|none>, CLAUDE_CODE_VERSION=<version|none>)
target     unknown (not observed)
artifacts  <k> bound by name, size and SHA-256 (declared, not attested)
limits     declared values are not authenticated; this is not a signature, a build attestation, SLSA provenance or anti-rollback
```

Avec `--expect-source-digest`, la ligne `expected source digest matched (declared by the operator)` précède `limits`. Aucun document ne contient de chemin ni d'heure : les mêmes entrées doivent donc donner des documents identiques octet pour octet, quels que soient le chemin de la source, l'umask ou `TMPDIR` ; le test de cette propriété est `NOT_RUN`.

<a id="bind-artefacts"></a>

### Lier des artefacts

```sh
"$AIGUARD_BIN" release manifest --source . --output "$RELEASE_PARENT/release-1" --artifact /absolute/path/to/a/built/file
```

Un artefact doit être :

- un fichier régulier appartenant à root ou à vous, non inscriptible par le groupe ou les autres, sans bit set-ID ni sticky bit, de 4 Gio au plus. Son type est lu avant son ouverture : un nœud de périphérique, une FIFO, un socket ou un répertoire est refusé sans être ouvert, et un nom remplacé entre cette lecture et l'ouverture est refusé, sauf si l'entrée de remplacement réutilise le numéro d'inode de l'entrée lue (un système de fichiers peut réutiliser aussitôt un numéro libéré) : un fichier régulier est alors lié, comme si le remplacement avait eu lieu avant la lecture, et un autre type est ouvert une fois, puis refusé. L'entrée de remplacement peut alors avoir déjà été ouverte une fois, sans blocage et jamais comme terminal de contrôle : quiconque peut écrire dans le répertoire peut faire ce remplacement, y compris un autre utilisateur propriétaire de l'entrée que vous désignez dans un répertoire appartenant à root avec le sticky bit, comme `/tmp`. Conserver les artefacts dans un répertoire où vous seul pouvez écrire ;
- atteint depuis `/` par de vrais répertoires appartenant à root ou à vous et non inscriptibles par le groupe ou les autres, sauf un répertoire appartenant à root avec le sticky bit, sur un système de fichiers local pris en charge et sans lien symbolique (hors alias macOS ci-dessus) ;
- nommé, par son nom de base, de 1 à 128 caractères de `[A-Za-z0-9._+-]`, sans point initial et sans terminaison `.env`, `.db`, `.sqlite`, `.sqlite3`, `-wal`, `-shm`, `.key`, `.pem` ou `.p12`. Cette règle de nom protège des erreurs ; ce n'est pas une frontière de sécurité. Deux artefacts de même nom de base sont refusés.

Un fichier à plusieurs liens physiques est accepté : un second nom ne donne à personne un autre moyen d'écrire le fichier, dont le propriétaire et le mode sont contrôlés. Les octets sont liés à un état observé du fichier ouvert (périphérique, inode, propriétaire, groupe, mode, nombre de liens, longueur et deux horodatages). Après la lecture, cet état est comparé de nouveau sur le fichier ouvert et sur le nom rouvert, et les répertoires sont parcourus de nouveau depuis `/`. Un fichier réécrit, renommé, lié ou délié pendant son empreinte, ou un répertoire au-dessus de lui remplacé, est refusé et rien n'est lié. Seuls le nom de base, la longueur et le SHA-256 entrent dans le manifeste ; le `profile` et la `target` de l'artefact y sont `unknown`. Sous macOS, l'ACL étendue du fichier d'artefact lui-même n'est pas inspectée ; celles de ses répertoires le sont.

<a id="bind-cli"></a>

### Lier le CLI construit

> **Observation en attente (`NOT_RUN`).** Cette procédure suit la conception du lecteur d'artefacts. Que le fichier écrit par Cargo passe ce lecteur là où Cargo l'écrit n'a pas été observé ; cette section sera confirmée ou corrigée d'après cette observation.

```sh
cargo build --release --locked
"$AIGUARD_BIN" release manifest --source . --output "$RELEASE_PARENT/cli-1" --artifact target/release/agentsandbox
```

D'après les sources de Cargo, sous Linux `target/release/agentsandbox` est un lien physique vers un fichier de `target/release/deps/` ; ce point n'a pas été vérifié ici, et le lecteur accepte un fichier lié comme un fichier unique. Le CLI peut lier son propre exécutable : il ne fait que le lire. Liez le fichier une fois Cargo terminé : un build qui le réécrit ou le relie pendant l'empreinte le fait refuser. Si le fichier est refusé, lisez la règle nommée sous `Caused by:` (par exemple un répertoire du checkout inscriptible par le groupe ou les autres, ou un système de fichiers non pris en charge) et corrigez cette condition là où se trouve le fichier. Ne copiez pas le binaire ailleurs pour le faire accepter : le manifeste doit décrire le fichier que vous distribuez, et la condition refusée resterait là où ce fichier a été construit.

<a id="provider-versions"></a>

### Versions de fournisseurs déclarées

`--codex-version` et `--claude-code-version` consignent les versions des clients fournisseurs demandées à l'image worker : les arguments de build `CODEX_VERSION` et `CLAUDE_CODE_VERSION` de `images/worker.Dockerfile`. Chacune est contrôlée avant toute préparation, par deux règles : la règle de version exacte de `build` (`X.Y.Z` avec suffixe facultatif de pré-version et de build, 100 octets au plus ; ni `latest`, ni intervalle, ni syntaxe shell) **et** le motif que la recette worker applique avant `npm install`, `^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$`. Une version qui échoue à l'une des règles est refusée, afin qu'aucun manifeste ne nomme une version avec laquelle aucune image worker ne peut être construite : `1.2.3`, `1.2.3-rc.1` et `1.2.3+b.5` sont acceptées ; `latest`, `1.2` et `1.2.3-rc.1+b.5` sont refusées. `build` accepte encore `1.2.3-rc.1+b.5`, et la recette worker échoue alors : utilisez une version acceptée par les deux règles.

Passez les mêmes versions à `build` et à `release manifest` : le manifeste les fait entrer dans l'identité des entrées de build et dans l'inventaire, et `build` les transmet à l'image worker, dont les [labels](#image-labels) les consignent. Elles sont consignées dans `build.provider_clients` comme déclarées par l'opérateur et non observées dans une image. Le nom du paquet de chaque client n'est consigné que si la recette worker contient son jeton d'installation exact (`"@openai/codex@$CODEX_VERSION"`, `"@anthropic-ai/claude-code@$CLAUDE_CODE_VERSION"`) ; sinon il est `unknown`.

<a id="read-manifest"></a>

## Lire le manifeste

Un fait déclaré par un fichier s'écrit `{"status": "declared", "value": …, "from": "<file>"}` ; un fait que le format 1 ne connaît pas s'écrit `{"status": "unknown", "reason": "…"}`. Une valeur inconnue n'est jamais remplacée par une valeur par défaut. Les champs, dans leur ordre :

| Champ | Contenu | Preuve |
|---|---|---|
| `format`, `format_version` | `ai-guard.release-manifest`, `1` | Fixe. |
| `producer` | Nom et version du paquet du CLI, et `allowlist_sha256`, le SHA-256 du `.dockerignore` que le CLI reflète. | Le CLI qui a écrit le fichier. La version est la version du paquet (`0.1.0`) de chaque build et n'identifie pas le build du CLI. |
| `statement` | `lifecycle: pre-build`, ce que le manifeste lie (`binds`) et ce qu'il n'établit pas (`does_not_establish`). | Texte fixe. |
| `source` | `scope: build`, `digest` (disposition `ai-guard:public-distribution:v1`), `files`, `bytes`, et `entries` (chemin, octets et SHA-256 de chaque fichier). | Observé sur la copie vérifiée. |
| `package` | Nom et version du paquet racine de `Cargo.lock` ; `license` issue de `Cargo.toml`. | Déclaré. Un nom ou une version de `Cargo.toml` différents de la racine du lock sont refusés comme lock périmé ; cette comparaison n'a lieu que si la table `[package]` les déclare tous deux comme chaînes simples. Un paquet écrit autrement (une clé `package` à la racine, `[[package]]`) laisse `license` `unknown`, avec sa ligne. |
| `locks.cargo`, `locks.npm` | Chemin, longueur, SHA-256, version du lock et nombres de paquets ; pour npm aussi `dev`, `optional` et `install_scripts`. | Octets observés sur la copie, puis analysés. |
| `build.inputs_identity` | L'[identité des entrées de build](#build-input-identity), avec ce qu'elle couvre (`covers`) et sa déclaration (`statement`). | Calculé. |
| `build.target` | Toujours `unknown`. | La plateforme d'un build n'est pas observée. |
| `build.toolchain` | Canal, profil, composants et cibles de `rust-toolchain.toml` ; une clé `components` ou `targets` absente s'écrit comme une liste vide. | Déclaré : la toolchain demandée, non le compilateur exécuté. |
| `build.release_profile` | Réglages scalaires d'une table `[profile.release]` dans `Cargo.toml`. | Déclaré. `unknown`, avec sa ligne, lorsque le profil release est écrit autrement : une clé `release` sous `[profile]`, une clé `profile` à la racine, un tableau de tables, une table imbriquée comme `[profile.release.package.<name>]` ou une valeur non scalaire. Une construction hors du sous-ensemble TOML du format 1, comme une clé pointée, rend `package.license` et ce champ `unknown`. |
| `build.recipes` | Pour chaque `images/*.Dockerfile` : chemin, SHA-256, `base_images` (référence, alias d'étape, étape interne, épinglage par empreinte) et `cargo_build_invocations` (argv, `profile`, `target`). | SHA-256 observé ; le reste déclaré par le texte de la recette, lu sans interprétation shell. |
| `build.provider_clients` | Pour chaque argument de build déclaré : `requested_version` et `package`. | Déclaré par l'opérateur ; non observé dans une image. |
| `sbom` | Chemin, format, version de spécification, longueur et SHA-256 de `sbom.cdx.json`. | Calculé. |
| `artifacts` | Pour chaque artefact, trié par nom : `name`, `bytes`, `sha256` ; `profile` et `target` `unknown`. | Empreinté lors de la création du manifeste ; provenance déclarée. |
| `uncovered` | Seize codes, chacun avec ses sujets et sa déclaration. | Ce que le format 1 ne couvre pas : voir [limites](#limits). |

Une recette est lue par instructions logiques : une ligne terminée par `\` se poursuit sur la suivante. Une directive d'analyseur avant la première instruction (`# syntax=…`, `# escape=…`), un `<<` susceptible d'ouvrir un heredoc, ou un commentaire ou une ligne vide au milieu d'une instruction continuée rend toute la recette `unknown`. Un `RUN` qui mentionne `cargo` hors de la forme simple `RUN cargo build <arguments>` donne une invocation `unknown`. Le `profile` d'une invocation est ce que disent ses jetons (`release` pour `--release` ou `-r`, `P` pour `--profile P`, `dev` sinon) ; sa `target` n'est déclarée que par `--target T`. La configuration Cargo d'une image de base, les valeurs `ENV` et `ARG` et l'environnement du build ne sont pas interprétés. Une recette sans `FROM` a des `base_images` `unknown`, non une liste vide, et une référence `FROM` contenant `$` est une image `unknown`.

<a id="build-input-identity"></a>

### Identité des entrées de build

```text
inputs_identity = SHA-256( "ai-guard:build-inputs:v1" ‖ 0x00
                           ‖ u64be(64) ‖ source digest (64 lowercase hex)
                           ‖ u8(p) ‖ u64be(length) ‖ CODEX_VERSION
                           ‖ u8(p) ‖ u64be(length) ‖ CLAUDE_CODE_VERSION )
```

`u64be` est un entier non signé de 64 bits gros-boutiste et `length` compte des octets. `p` vaut 1 pour une version déclarée et 0 pour une version absente, qui contribue alors une chaîne vide. L'empreinte de source couvre déjà `Cargo.toml` (version du paquet et profil release), `rust-toolchain.toml`, les locks et les recettes avec leur `cargo build --release` déclaré. La valeur nomme donc ces entrées de build et ces versions de fournisseurs déclarées, et rien de plus. Ce **n'est pas une identité de build** : un build amd64 et un build arm64, un build debug et un build release sur un hôte, ou deux builds d'image qui résolvent des images différentes derrière le même tag `FROM` la partagent. Le manifeste énonce chacun de ces faits comme `unknown` (`build.target`, `profile` et `target` des artefacts) ou non couvert (`build-target`, `base-image-digest`, `cargo-configuration`). Aucun binaire ni aucune image ne rapporte cette valeur aujourd'hui.

<a id="read-sbom"></a>

## Lire l'inventaire

`sbom.cdx.json` est un document CycloneDX 1.6 JSON de cycle de vie `pre-build` : il décrit ce que déclarent les locks et les recettes avant tout build. Ses `metadata.properties` reprennent l'empreinte de source, l'identité des entrées de build et leurs dispositions.

| Composants | Contenu |
|---|---|
| Le paquet décrit (`metadata.component`) | Le paquet racine de `Cargo.lock`, `bom-ref` `agentsandbox`, avec sa licence de `Cargo.toml` lorsqu'elle est déclarée, marquée comme déclarée : une expression SPDX lorsqu'elle est bien formée, sinon un nom de licence. |
| Crates Rust | Une `library` par paquet crates.io, `bom-ref` `cargo:<name>@<version>`, SHA-256 égal au checksum du lock de l'archive du registre, propriétés `ai-guard:scope=unresolved` et `ai-guard:cargo-source=crates.io` ; ni `scope`, ni licence. |
| Paquets npm | Une `library` par entrée du lock, `bom-ref` `npm:<name>@<version>`, le SHA-512 de son `integrity` écrit en hexadécimal, `scope: excluded` pour les entrées de développement, la licence déclarée, l'URL du registre comme référence `distribution`, et des propriétés `ai-guard:npm-*` pour les entrées facultatives, de plateforme et à script d'installation. |
| Le paquet frontend | La racine du lock npm, avec `ai-guard:first-party=true`. |
| Images de base | Un `container` par référence `FROM` externe, nom et tag séparés, `ai-guard:pinning=digest`, `tag` ou `none` avec `ai-guard:version-evidence=declared:digest`, `declared:tag` ou `declared:none` (un tag ne désigne aucun contenu ; un hash SHA-256 n'est écrit que pour une empreinte `@sha256:` de 64 caractères hexadécimaux minuscules), et `ai-guard:declared-by=<recipe>`. |
| Toolchain | `toolchain:rust@<channel>`, avec `ai-guard:version-evidence=declared:rust-toolchain.toml`. |
| Clients fournisseurs | Seulement s'ils sont déclarés : `provider:<package>@<version>`, avec `ai-guard:version-evidence=declared:build-argument`. |

Ce que l'inventaire omet, et pourquoi :

- Ni `serialNumber` ni `timestamp` : les mêmes entrées donnent les mêmes octets, et un numéro de série fixe contredirait l'unicité que la spécification lui demande.
- Ni `scope` ni licence pour les crates Rust : `Cargo.lock` ne résout ni les cibles ni les types de dépendances, et ne consigne aucune licence. Un consommateur qui lit un `scope` absent comme `required` (la valeur par défaut de CycloneDX) surestime ce que dit le lock ; `ai-guard:scope=unresolved` l'énonce.
- Une licence n'est écrite comme `expression` SPDX que lorsqu'elle est un identifiant bien formé ou une expression `AND`/`OR`/`WITH` bien formée, sinon comme `name` de licence ; les deux portent `acknowledgement: declared`. L'appartenance d'un identifiant à la liste SPDX n'est pas contrôlée.
- Des graphes de dépendances séparés : le paquet Rust, le paquet frontend, chaque image de base, la toolchain et chaque client fournisseur sont des racines distinctes. Aucun lock n'indique que le paquet Rust dépend du frontend, d'une image ou d'un client fournisseur, et les recettes ne font que déclarer l'assemblage des images : aucune arête n'est inventée. `ai-guard:dependency-graph` et les `compositions` (`incomplete` pour `agentsandbox`, `unknown` pour les images et les clients fournisseurs) le disent.
- Aucun artefact : les artefacts sont liés dans le manifeste, si bien que la même source et les mêmes versions déclarées donnent le même inventaire quels que soient les fichiers liés.
- **Aucun identifiant de paquet** (ni purl, ni CPE) : cet inventaire **n'est pas prêt pour les scanners de vulnérabilités**, qui rapprochent d'ordinaire les composants par purl, et une revue de la chaîne d'approvisionnement ne peut pas encore s'y appuyer pour un rapprochement automatique.

Valider le fichier avec un outil CycloneDX externe relève de votre choix ; aucun outil de ce type ne fait partie d'ai-guard, et aucun n'a été exécuté.

<a id="verify"></a>

## Vérifier

```sh
"$AIGUARD_BIN" release verify --source . --manifest "$RELEASE_PARENT/cli-1" --artifact target/release/agentsandbox
"$AIGUARD_BIN" release verify --source . --manifest "$RELEASE_PARENT/inputs-1" --source-only
```

`release verify` prend les versions de fournisseurs déclarées dans le manifeste ; elle n'a pas d'option de version. Elle procède dans cet ordre et rend le premier résultat applicable :

1. Elle lit `release-manifest.json`, `sbom.cdx.json` et `SHA256SUMS` dans `--manifest` : chacun doit être un fichier régulier à lien unique, appartenant à root ou à vous, non inscriptible par le groupe ou les autres et de 16 Mio au plus, dans une chaîne de répertoires conforme à la règle des ancêtres d'[avant de commencer](#before-you-start).
2. Elle lit l'en-tête du manifeste de manière tolérante. Un fichier qui n'est pas un manifeste de livraison ai-guard, un autre `format_version` ou un autre producteur donne `UNVERIFIED`, avant tout contrôle strict et avant la préparation de la source.
3. Elle contrôle `SHA256SUMS`, la forme canonique du manifeste et ses versions de fournisseurs déclarées.
4. Sauf avec `--source-only`, elle empreinte chaque `--artifact` selon les [règles des artefacts](#bind-artefacts).
5. Elle prépare `--source` (la ligne de provenance apparaît sur la sortie d'erreur) et compare l'empreinte de cette copie à `source.digest.value` : une différence donne `DIVERGENT source`, décidé avant toute lecture dans la copie. Sinon, elle dérive de nouveau les deux documents.
6. Elle compare, dans cet ordre : source, locks, faits déclarés, identité des entrées de build, inventaire, artefacts, puis les octets restants (`declared`, champ `manifest`).

`MATCH` et `SOURCE MATCH` forment la première ligne de la sortie standard ; `DIVERGENT` et `UNVERIFIED` sont affichés sur la sortie d'erreur.

| Première ligne | Sortie | Signification |
|---|---|---|
| `release verify: MATCH` | 0 | Toutes les comparaisons tiennent, et chaque artefact listé par le manifeste a été présenté et concorde. |
| `release verify: SOURCE MATCH; artifacts NOT VERIFIED (<k> listed)` | 0 | Avec `--source-only` seulement : source, locks, faits déclarés et inventaire concordent ; **aucun artefact n'a été lu ni vérifié**. `--source-only` ne se combine pas avec `--artifact`. |
| `Error: DIVERGENT <category>: …` | 1 | Ce CLI a jugé un manifeste au format 1 de son propre producteur et trouvé une différence, ou le répertoire n'est pas une publication au format 1 bien formée. |
| `Error: UNVERIFIED: …` | 1 | Ce CLI ne peut pas juger, ou n'a pas jugé : une entrée n'a pas pu être lue selon sa politique ; la source n'a pas pu être préparée, ou elle a l'empreinte que lie le manifeste mais ses locks ou ses déclarations n'ont pas pu être dérivés ; le fichier n'est pas un manifeste de livraison ai-guard, son `format_version` n'est pas 1, ou son producteur diffère de ce CLI ; `--source-only` a été combinée avec `--artifact` ; une demande d'arrêt a terminé la commande ; ou une étape s'est terminée sans résultat. Ce n'est ni une preuve de divergence, ni une concordance : la traiter comme une vérification échouée. Un manifeste modifié pour nommer un autre `format_version` ou un autre producteur est signalé `UNVERIFIED`, non `DIVERGENT`. |

Le bloc de réussite reprend les lignes de `release manifest`, avec `artifacts  <k>/<k> matched` ou, avec `--source-only`, `artifacts  NOT VERIFIED (<k> listed; --source-only)`. Le statut de sortie 2 est une erreur d'usage de l'analyseur d'arguments, par exemple une option mal orthographiée, non un résultat de vérification. Un script doit lire à la fois le statut de sortie et la première ligne de la sortie standard : seul `release verify: MATCH` signifie que les artefacts ont été vérifiés.

| Catégorie | Cause |
|---|---|
| `sums` | Un document ne correspond pas à sa ligne de `SHA256SUMS`. |
| `format` | `SHA256SUMS` n'est pas exactement ses deux lignes ; ou le manifeste contient un champ inconnu, n'est pas sous forme canonique, nomme un argument de build hors du format 1 ou déclare une version de fournisseur inadmissible. |
| `source` | L'empreinte de source diffère ; au plus dix chemins modifiés, ajoutés et supprimés sont nommés. Lorsqu'aucun chemin n'est listé, le premier champ différent de `source` est nommé à la place (`no entry differs, source.<field> differs`). C'est décidé juste après la préparation, avant la lecture des locks : un changement que le format 1 ne sait pas lire, comme une ligne de lock qu'il refuse, est signalé ici. |
| `lock` | Un champ de `locks.cargo` ou `locks.npm` diffère ; le champ est nommé. Un lock que le format 1 ne sait pas lire n'est jamais `lock` : il donne `source` lorsque la source diffère du manifeste, sinon `UNVERIFIED` (`--source`). |
| `declared` | Un fait déclaré diffère ; le champ est nommé. |
| `inputs-identity` | `build.inputs_identity` diffère. |
| `inventory` | `sbom.cdx.json` ou son enregistrement dans le manifeste diffère. |
| `artifact-changed` | Un artefact présenté a une autre longueur ou un autre SHA-256. |
| `artifact-missing` | Un artefact listé n'a pas été présenté. |
| `artifact-unlisted` | Un artefact présenté n'est pas listé. |

| Message `UNVERIFIED` | Action suivante |
|---|---|
| `<input> could not be read under its policy: <reason>`, où `<input>` est `--manifest`, l'un des trois fichiers, `--artifact` ou `--source` | Corriger la condition nommée : fichier absent, répertoire inscriptible par le groupe ou les autres, fichier de manifeste à liens physiques, source impossible à préparer, ou source ayant l'empreinte du manifeste dont le format 1 refuse les locks (mal formés, incomplets ou périmés). |
| `--source-only verifies no artefact and cannot be combined with --artifact; nothing was compared` | Relancer avec soit `--source-only`, soit les artefacts. |
| `interrupted by <signal> before a result; nothing was judged and nothing was published`, éventuellement suivi de `; the interrupted step reported: <reason>` | Relancer le moment venu. |
| `the <step> step ended without a result; <what may exist>; this CLI judged nothing` | Relancer ; ce n'est pas un résultat de vérification. |
| `not an ai-guard release manifest; this CLI judges nothing in it` | Le fichier n'est pas un manifeste de ce format. |
| `unsupported format_version <n>; this CLI judges format_version 1 only` | Utiliser un CLI qui connaît cette version de format. |
| `produced by <name> <version> (allowlist <sha>); this CLI is <name> <version> (allowlist <sha>). Verify with the CLI built from the distribution the manifest describes` | Construire le CLI depuis la distribution que décrit le manifeste et vérifier avec lui. |

La version du producteur est la version du paquet, `0.1.0` pour chaque build : deux builds différents du CLI portent le même producteur, et le contrôle du producteur ne les distingue pas. Une règle de dérivation ne peut changer qu'avec un nouveau `format_version` ; un build qui en changerait une sans lui signalerait des divergences sur les manifestes d'un autre build.

**Déplacer un manifeste.** Aucun document ne contient de chemin : la source comme le répertoire du manifeste peuvent être déplacés. Copiez le répertoire du manifeste, par exemple avec `cp -R` ; une copie par liens physiques est refusée comme `UNVERIFIED`, car chaque fichier doit avoir un lien unique. Les fichiers et répertoires copiés doivent rester non inscriptibles par le groupe et les autres.

<a id="independent-checks"></a>

**Vérifications indépendantes.** Sans le CLI, depuis le répertoire du manifeste, sous Linux (GNU coreutils) ou sous macOS respectivement :

```sh
sha256sum -c SHA256SUMS
shasum -a 256 -c SHA256SUMS
```

Aucune n'a été exécutée dans cette validation. Elles montrent seulement que les deux documents correspondent à `SHA256SUMS` : qui modifie un document peut aussi réécrire ce fichier. L'empreinte de source peut en outre être recalculée depuis `source.entries`, dans l'ordre listé, avec sa disposition :

```text
source digest = SHA-256( "ai-guard:public-distribution:v1" ‖ 0x00
                         ‖ for each entry: u64be(length of path) ‖ path ‖ u64be(bytes) ‖ sha256 (64 lowercase hex) )
```

Elle peut alors être comparée au `sha256=` d'une ligne de provenance de `build`, et chaque entrée au SHA-256 du fichier de votre checkout.

<a id="relation-to-build"></a>

## Relation avec `agentsandbox build` et avec les images

`release manifest` fait sa propre capture de la source. Elle ne réutilise pas la copie que `build` a remise au moteur, et rien ne consigne la copie à partir de laquelle une image a été construite ; deux captures d'une source inchangée ont la même empreinte. Pour rattacher un manifeste à la source rapportée par un `build`, copiez la valeur `sha256=` de la ligne `Prepared public distribution (build): sha256=<hex> …` affichée par `build`, et donnez-la au manifeste :

```sh
"$AIGUARD_BIN" build --runtime docker --source . --codex-version <X.Y.Z> --claude-code-version <X.Y.Z>
"$AIGUARD_BIN" release manifest --source . --output "$RELEASE_PARENT/build-1" --codex-version <X.Y.Z> --claude-code-version <X.Y.Z> --expect-source-digest <hex>
```

La valeur doit compter 64 caractères hexadécimaux minuscules. Elle est comparée à l'empreinte de la nouvelle copie après la préparation, avant toute lecture ou sortie, et n'est jamais écrite dans les documents. Une différence signifie que la source a changé depuis ce `build`, ou que la valeur provient d'un autre ; rien n'est alors écrit. Ce contrôle vérifie que deux captures contiennent les mêmes octets, tel que vous l'avez déclaré ; il ne prouve ni qu'une image a été construite depuis l'une de ces captures, ni que la ligne provient de ce build. Sans l'option, comparez vous-même cette valeur à `source.digest.value`.

<a id="image-labels"></a>

### Labels des images

Chaque recette d'image déclare des labels dans son étape finale :

| Image | Label | Valeur |
|---|---|---|
| Contrôle et worker | `ai-guard.image.recipe` | `images/control.Dockerfile` ou `images/worker.Dockerfile` : l'entrée de `build.recipes` dont provient l'image. |
| Worker | `ai-guard.provider.codex.requested-version` | L'argument de build `CODEX_VERSION` ; vide s'il n'est pas demandé. |
| Worker | `ai-guard.provider.claude-code.requested-version` | L'argument de build `CLAUDE_CODE_VERSION` ; vide s'il n'est pas demandé. |

Comparez les versions demandées à `build.provider_clients` dans le manifeste. Pour lire les labels après un build, avec Docker ou Podman respectivement (commandes standard du moteur, non exécutées dans cette validation) :

```sh
docker image inspect --format '{{json .Config.Labels}}' agentsandbox-worker:local
podman image inspect --format '{{json .Config.Labels}}' agentsandbox-worker:local
```

Les labels sont déclarés par qui construit l'image : n'importe qui peut construire ces recettes, ou d'autres, avec n'importe quel label. Un label nomme une recette et une version demandée ; il n'établit pas que l'image a été construite depuis une copie préparée ou depuis la source de ce manifeste, ni qu'un client de cette version est installé. Aucun identifiant ni empreinte d'image n'est consigné (`image-identity`), et aucun label ne porte l'identité des entrées de build.

<a id="failures"></a>

## Échecs et reprise

Chaque refus sort avec le statut 1 et son message commence par `Error: ` ; une cause rapportée par une politique ou par le système suit sous `Caused by:`. Un refus de `--output`, de `--artifact` ou d'une version de fournisseur commence par `<option> is refused; nothing was prepared`, et le tableau donne la cause qui le suit. En aucun cas ai-guard ne supprime ni n'écrase ce qui existe.

| Message | Action suivante |
|---|---|
| `the parent of --output must be an existing directory owned by you with mode exactly 0700, whose ancestors belong to root or to you and are not writable by group or others (create it with mkdir -m 700 <parent>): <parent>` | Créer un parent privé comme dans [avant de commencer](#before-you-start) et l'utiliser. |
| `--output already exists or cannot be inspected: <output>; release manifest writes only into a new directory and never replaces one` | Choisir un nouveau nom. |
| `--output must be outside the source directory: …` | Choisir un parent hors de la distribution. |
| `a staging directory of an earlier run exists or cannot be inspected: <parent>/.<leaf>.release-staging; inspect or remove it first, ai-guard never adopts or removes it` | Une exécution antérieure s'est arrêtée avant publication. Examiner ce répertoire ; le supprimer vous-même lorsqu'il n'est plus utile, ou choisir un autre nom. |
| `provider versions must be exact semantic versions; …` ou `provider version <v> is refused: the worker recipe accepts only versions matching …` | Utiliser une version acceptée par les deux règles : voir [versions de fournisseurs déclarées](#provider-versions). |
| `--expect-source-digest must be 64 lowercase hexadecimal characters, as printed after sha256= by build; nothing was prepared` | Recopier la valeur exactement. |
| `the prepared source digest <x> differs from the expected <y> (declared by the operator); …` | La source a changé depuis ce `build`, ou la valeur provient d'une autre source. Déterminer laquelle ; reconstruire les images si elles doivent correspondre à cette source. |
| `at most 64 artefacts can be bound; …`, `artefact name <name> is refused: …` ou `two artefacts have the name <name>; …` | Corriger le nombre d'artefacts, un nom ou un nom de base répété. |
| `artefact <path>`, causé par la règle d'artefact en échec ; ou `artefact <path> was replaced while it was opened …`, `artefact <path> changed while it was hashed …`, `… no longer names the file that was hashed …`, `the directories leading to … changed while it was hashed …` | Corriger la condition du fichier ou du répertoire, ou attendre qu'aucun processus n'écrive le fichier, puis relancer. |
| `<file>:<line>: <reason>; release manifest format 1 accepts only a complete lock it can resolve` | Le format 1 lit un `Cargo.lock` version 4 avec des paquets crates.io et un seul paquet local, et un `package-lock.json` plat version 3 avec des URL de registre `https` et une intégrité SHA-512, dont l'entrée racine et `front/package.json` ne déclarent que `dependencies` et `devDependencies`. Une autre source (dépendance Git, autre registre), une empreinte manquante, une clé inconnue, un champ d'un autre type (`a field has a type or value outside format 1 …` : la valeur trouvée n'est pas reprise), une entrée marquée `devOptional`, ou des `optionalDependencies` ou `peerDependencies` à la racine sont refusés ; rien n'est résumé. |
| `Cargo.toml declares package <name> <version> but the root package of Cargo.lock is <name> <version>: the lock is stale and is refused` | Mettre `Cargo.lock` à jour avec Cargo et relire le changement. |
| Un refus de la source ou de `TMPDIR` | Mêmes messages et mêmes actions que pour `build` : [sources préparées de build et Compose](../local/README.fr.md#prepared-source). |
| `interrupted by <signal> before publication; nothing was published and no staging directory was created; …` | Relancer le moment venu. Pendant la préparation elle-même, le message est celui de `build` : `interrupted by <signal> while the source was being prepared; …`. |
| `the derivation step ended without a result; nothing was published and no staging directory was created`, ou le même pour `the artefact hashing step` | Relancer. |
| `the staging directory <staging> could not be created or checked; if it exists, inspect it and remove it yourself, ai-guard never adopts or removes it; nothing was published` | Lire la cause ; si `<staging>` existe, l'examiner et le supprimer vous-même, puis relancer. |
| `the release documents could not be staged; what was written is kept at <staging> and nothing was published` | Lire la cause ; examiner le répertoire intermédiaire et le supprimer vous-même, puis relancer. |
| `the staging directory was not renamed to <output> (the cause is not reported: …); ai-guard replaced and removed nothing and did not move the staging directory it created at <staging>` ou `<output> was not confirmed absent before the rename, so the staging directory was not renamed to it; …` | Pour le premier message, une entrée peut exister à `<output>`, le parent ou le répertoire intermédiaire peut avoir changé ou ne pas avoir pu être contrôlé de nouveau, le répertoire intermédiaire peut ne pas avoir été synchronisé, ou le système peut avoir refusé le renommage. Pour le second, une entrée peut exister à `<output>`, ce nom peut ne pas avoir pu être examiné, ou le parent peut avoir changé ou ne pas avoir pu être contrôlé de nouveau ; la raison suit le message. ai-guard n'a pas déplacé le répertoire intermédiaire ; il n'est plus à `<staging>` si lui-même ou un répertoire au-dessus de lui a été déplacé. Examiner `<output>` et rechercher le répertoire intermédiaire ; rien n'a été remplacé ni supprimé. Supprimer vous-même le répertoire intermédiaire lorsqu'il n'est plus utile, puis relancer. |
| `the rename was performed but its durability is unknown; ai-guard removes neither name. After a crash the directory may be found at <parent>/<leaf> or at <parent>/.<leaf>.release-staging; run release verify --manifest <that directory> before using it` | Vérifier le répertoire là où vous le trouvez avant de l'utiliser. |
| `the publication step ended without a result; the output may exist at <parent>/<leaf> or at <parent>/.<leaf>.release-staging; ai-guard removes neither name; run release verify --manifest <that directory> before using it` | Regarder sous les deux noms ; vérifier le répertoire trouvé avant de l'utiliser. |
| `the documents were published at <output> but not confirmed (<cause>); the directory is kept, verify it with release verify --manifest before using it` | Lancer `release verify` sur ce répertoire. |

**Interruption.** `SIGINT` (Ctrl-C), `SIGTERM` et `SIGHUP` sont des demandes d'arrêt. `release manifest` les examine pendant la préparation, entre ses étapes, avant chaque artefact et après chaque tranche de 8 Mio d'un artefact, puis une dernière fois avant l'écriture des documents dans le répertoire intermédiaire. Ensuite, elle écrit trois petits fichiers, renomme un répertoire et les confirme sans examiner de demande d'arrêt : une interruption à ce stade ne pourrait que transformer une publication complète en publication partielle inexpliquée, et le résultat est donc maintenu. `release verify` les examine pendant l'empreinte des artefacts, la préparation de la source et sa dérivation, puis se termine par `UNVERIFIED: interrupted by <signal> before a result; …`. Elle ne publie rien ; sa seule écriture est la copie préparée temporaire sous `TMPDIR`, traitée comme pour `build`. Un signal déjà ignoré au démarrage du CLI le reste.

**Aucune borne de temps.** Comme la préparation de `build`, les deux commandes ont des bornes de taille (limites de capture, 8 Mio par fichier de déclaration lu, 64 artefacts de 4 Gio au plus) mais aucune borne de temps. Si une lecture de la source ou d'un artefact, ou une écriture sous `TMPDIR` ou dans le parent de sortie, ne revient pas (système de fichiers réseau, FUSE ou amovible qui ne répond plus), la commande ne se termine pas avant le retour de cet appel système ; seul `SIGKILL` l'arrête alors. Un CLI tué peut laisser une copie partielle sous `TMPDIR`, à traiter comme [un répertoire laissé par un CLI tué](../local/README.fr.md#prepared-source-retained), ou un répertoire `.<leaf>.release-staging`, qu'ai-guard n'adopte ni ne supprime jamais. Déduit du code ; `NOT_RUN`.

<a id="data-hygiene"></a>

## Hygiène des données

- Seules les [entrées de build](../local/README.fr.md#prepared-source) sont copiées : fichiers `.env`, `.claude/`, bases de données, `docs/`, `deploy/` et `tests/` ne sont pas des entrées de build et n'entrent jamais dans la copie ni dans les documents. Un secret écrit dans une entrée de build est copié comme ses autres octets, et les documents reprennent du texte des entrées de build, par exemple les arguments `cargo build` et les références d'images de base des recettes, les URL npm `resolved`, les noms et versions de paquets, les licences, les valeurs de chaîne de `[profile.release]` et la toolchain demandée.
- Aucun document ne contient de chemin de l'hôte (source, `TMPDIR`, copie, répertoire d'artefact ou parent de sortie), de numéro de périphérique ou d'inode, de nombre de liens, d'identifiant d'utilisateur, de nom d'hôte, d'heure, de valeur d'environnement ni la valeur de `--expect-source-digest`.
- Un artefact ne contribue que son nom de base, sa longueur et son SHA-256, jamais son contenu.
- Une URL npm `resolved` contenant des informations d'utilisateur (`user:password@`), une requête ou un fragment fait refuser le lock, si bien qu'aucune URL de ce type n'est écrite. Un jeton placé dans le chemin de l'URL n'est pas détecté et serait écrit avec l'URL.
- Lorsque `front/package-lock.json` ou `front/package.json` contient un champ d'un autre type, le refus nomme sa ligne et sa colonne, non la valeur trouvée.
- La sortie standard reprend `--output` tel que vous l'avez donné. La ligne de provenance sur la sortie d'erreur nomme le chemin, le périphérique et l'inode de la source, comme pour `build` : relisez les journaux avant de les partager.

<a id="limits"></a>

## Limites et points ouverts

La liste `uncovered` de chaque manifeste nomme ce que le format 1 ne couvre pas, avec les sujets concernés :

| Code | Non couvert |
|---|---|
| `build-target` | La plateforme ciblée par un build. |
| `base-image-digest` | L'empreinte qu'un build résout derrière un tag. |
| `base-image-content` | Le contenu des images de base. |
| `system-packages` | Les paquets installés par les recettes, comme les paquets APT. |
| `build-time-downloads` | Les paquets qu'un build d'image télécharge hors des locks, comme les clients fournisseurs. |
| `toolchain-content` | Le compilateur exécuté et ses composants. |
| `cargo-configuration` | La configuration Cargo des images de base, les valeurs `ENV` et `ARG` et l'environnement du build. |
| `rust-applicability` | Cibles et types de dépendances des crates. |
| `npm-installed-subset` | Les paquets de plateforme facultatifs effectivement installés. |
| `native-code-in-crates` | Le code natif que les crates construisent ou lient. |
| `rust-licenses` | Les licences des crates. |
| `install-scripts` | Le code exécuté par les scripts d'installation npm. |
| `frontend-bundle` | Le bundle frontend construit. |
| `package-identifiers` | Les identifiants purl ou CPE. |
| `artifact-provenance` | Qu'un artefact a été construit depuis cette source ; son profil et sa cible. |
| `image-identity` | Identifiants et empreintes d'images. |

Autres limites :

- Ni signature, ni ancre de confiance, ni anti-rollback, ni provenance SLSA ; les valeurs déclarées ne sont pas authentifiées.
- Un manifeste provient de sa propre capture : rien ne le rattache à la copie consommée par un moteur, et les images ne lui sont pas liées.
- Les images de base sont référencées par tag, non par empreinte, comme l'indiquent les [prérequis d'exploitation](../../operations.md#prérequis) ; les clients fournisseurs sont installés sans lock ni contrôle d'intégrité.
- Aucun binaire en cours d'exécution, aucun endpoint de santé ni aucune image ne rapporte l'identité des entrées de build.
- `build` accepte des versions de fournisseurs que la recette worker refuse.
- Sous macOS, l'ACL étendue d'un fichier d'artefact n'est pas inspectée.
- Un nom remplacé entre la lecture du type et l'ouverture d'un artefact peut être ouvert une fois, sans blocage et jamais comme terminal de contrôle, avant d'être refusé ; dans un répertoire appartenant à root avec le sticky bit, un autre utilisateur peut le faire avec une entrée qui lui appartient.
- Une entrée de remplacement qui réutilise le numéro d'inode du nom lu n'est pas vue comme un remplacement : un fichier régulier est lié, comme si le remplacement avait eu lieu avant la lecture ; un autre type est ouvert une fois, puis refusé.
- Aucun workflow d'intégration continue ne vérifie les livraisons : un tel workflow dépendrait d'actions tierces à épingler par une empreinte de commit vérifiée.
- Non exécutés : tous les tests de ces commandes, la construction des images et l'inspection des labels, la validation du schéma CycloneDX, les hôtes Docker, Podman et macOS, les builds reproductibles.

Les gates G01 et G12 restent `BLOCKED`, et les risques RC-P3-R11, RC-P7-R07 et RC-P7-R04 restent ouverts ([préparation à la production](../../engineering/PRODUCTION_READINESS.md), [couverture des risques](../../engineering/RISK_COVERAGE_20261112.md) et son [registre des risques](../../engineering/RISK_COVERAGE_20261112.json)). Ces commandes ne ferment aucun gate ni aucun risque.
