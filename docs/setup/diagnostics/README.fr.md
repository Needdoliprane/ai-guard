# Diagnostic opérateur

[English](README.md) · [Index d'installation](../README.fr.md) · [Poste de travail local](../local/README.fr.md) · [Fournisseurs IA](../providers/README.fr.md) · [SSO ai-guard](../sso/README.fr.md)

`ai-guard-diagnose` explique une installation avant tout lancement : la disposition que contient un répertoire, les tenants et le plan de capacité que déclarent ses fichiers, les fichiers de credentials référencés et leur acceptation par leur lecteur, les profils de clients IA déclarés et celui que sélectionne un lancement géré, et ce que ce diagnostic n’examine pas. Il lit les fichiers que vous nommez et rien d'autre. Il ne démarre aucun agent, n'appelle ni fournisseur ni modèle, ne contacte aucun fournisseur d'identité, et ne crée ni ne migre rien.

Une option fait quitter ce mode hors ligne, et seulement quand vous la donnez : `--active oidc-discovery` demande au fournisseur d'identité d'un fichier OIDC inspecté son document de découverte et ses clés. Sans cette option, rien n'est contacté.

Un rapport est une aide au diagnostic. Un rapport sans erreur ne prouve aucun comportement à l'exécution et ne ferme aucun gate de qualification : lire ses deux derniers blocs, qui listent ce qui n'a pas été examiné.

## Ce que l'outil est, et ce qu'il n'est pas

- **Hors ligne par défaut.** Aucun socket n'est ouvert, sauf si l'option `--active` nomme une vérification active. Une URL donnée en ligne de commande est validée, jamais contactée.
- **Lecture seule.** Rien n'est créé, modifié, migré ni supprimé : ni base, ni fichier de verrou, ni credential, ni répertoire.
- **Aucun contenu de credential.** Aucun fichier que les fichiers inspectés désignent comme fichier de clé, de jeton, credential fournisseur, enregistrement de setup ou base n'est ouvert. Ils sont décrits par leurs seules métadonnées de fichier. Un fichier donné par option est ouvert comme ce que son option dit qu'il est : la section sur les fichiers jamais ouverts précise ce que cela laisse non couvert, et comment le rapport le dit.
- **Ligne de commande uniquement.** Aucune variable d'environnement ni fichier dotenv n'est lu, et aucun programme n'est cherché dans `PATH`. Le répertoire courant ne sert qu'à résoudre les arguments relatifs.
- **Chemins nommés uniquement.** L'outil regarde les chemins qu'on lui donne et ceux que référencent les fichiers inspectés. Il ne liste aucun répertoire et ne fouille aucun parent.
- **Un processus court.** Ce n'est ni un service ni une autorité : il n'accorde rien, et aucun autre composant ne consomme son résultat.

Il ne remplace pas les diagnostics existants. Il indique lequel lancer ensuite :

| Commande | Ce qu'elle fait et que cet outil ne fait pas |
|---|---|
| `agentsandbox doctor --runtime docker` ou `--runtime podman` | Exécute le moteur pour l'inspecter. |
| `agentsandbox providers status` | Montre les profils configurés par le CLI du produit, sans chemins privés ni valeurs de credentials. |
| `agentsandbox healthcheck --url URL` | Demande son état de santé à un service en cours d'exécution. |
| `agentsandbox verify-audit` | Demande au gateway sa vérification locale de chaîne. En ligne : le défaut lit le fichier viewer historique ; `--viewer-token-file FILE --gateway URL` sélectionne explicitement celui d’un tenant préparé. Cette lecture n’établit pas la continuité anti-rollback indépendante. |
| `ai-guard-audit-checkpoint verify` | Vérification hors ligne explicite des checkpoints signés d’un tenant avec une ancre tenue indépendamment ; comparaison facultative à la base décrite dans le [guide des checkpoints](../audit-signatures/README.fr.md). Cette commande séparée n’est jamais appelée par le diagnostic. |
| `agentsandbox kubernetes doctor` | Inspecte une autorité Kubernetes explicite en HTTPS. Elle n'admet aucun worker. |
| `agentsandbox broker recovery-status` | Lit les métadonnées de reprise confirmées du broker local. |

## Quick Start

### Construire l'outil avec le produit

Le binaire est construit depuis la même source, par la même commande, que `agentsandbox`. Exécuter ceci à la racine du dépôt :

```sh
cargo build --release --locked
AIGUARD_BIN="$PWD/target/release/agentsandbox"
AIGUARD_DIAGNOSE="$PWD/target/release/ai-guard-diagnose"
"$AIGUARD_DIAGNOSE" --version
"$AIGUARD_DIAGNOSE" --help
```

Utiliser un outil construit depuis la même source que le gateway dont on inspecte les fichiers : un outil plus ancien peut refuser un fichier plus récent. Chaque rapport imprime la version de paquet de l'outil. Dans les exemples de ce guide, `PACKAGE_VERSION` tient lieu de cette version.

Pour une installation sur l'hôte, lancer l'outil sur l'hôte, sous le compte propriétaire de l'installation. Une image de contrôle ne contient l'outil à côté de `agentsandbox` que si elle est construite depuis une source dont le [Dockerfile de contrôle](../../../images/control.Dockerfile) compile et copie `ai-guard-diagnose`. Une installation qui vit dans un volume de conteneur s'inspecte alors depuis cette image, comme le décrit plus bas « Inspecter une installation préparée dans son volume » ; les commandes de cette section n'ont pas été exécutées.

### Premier lancement, sans installation

Sans aucun argument, l'outil inspecte `.agentsandbox` dans le répertoire courant, la valeur par défaut de `init`, `run` et `providers` :

```sh
"$AIGUARD_DIAGNOSE"
```

Quand ce répertoire n'existe pas, le rapport le dit, dit comment donner une installation qui existe ailleurs, nomme les deux façons d'en créer une, et le processus se termine avec le code 3 :

```text
ai-guard-diagnose PACKAGE_VERSION - report version 1
mode       offline; nothing written; no file named as a credential was read; no network
read       no file content
inspected  config directory "<config>" (default; no target was given)
result     ERROR - 1 error, 0 unknown, 0 warning; 7 not checked, 12 not available

LAYOUT
  config directory "<config>"  absent
  [error] layout.target_absent
          No target was given and the default directory .agentsandbox does not exist in the current
          directory.
          scope    input config_dir
          basis    observed
          action   If an installation exists elsewhere, pass it with --config-dir DIR or
                   --installation-dir DIR. To create one here: for a local installation run
                   agentsandbox init --config-dir "<config>"; for an SSO installation run
                   agentsandbox setup prepare --input MANIFEST --output-dir NEW_DIR. If a
                   preparation was interrupted, pass the retained staging directory that setup
                   prepare printed as --installation-dir DIR.
```

`"<config>"` tient lieu de votre répertoire. Ajouter `--show-paths` pour imprimer les chemins absolus, dans les faits et dans les actions. La sortie complète est [report.first-run.txt](examples/report.first-run.txt).

### Un répertoire de configuration historique

C'est le répertoire créé par `agentsandbox init`, `$AIGUARD_CONFIG` dans le [guide local](../local/README.fr.md) :

```sh
"$AIGUARD_DIAGNOSE" --config-dir "$AIGUARD_CONFIG"
"$AIGUARD_DIAGNOSE" --config-dir "$AIGUARD_CONFIG" --json
```

Sur un répertoire que `init` vient de créer, le rapport n'a aucune erreur et le processus se termine avec le code 0 :

```text
ai-guard-diagnose PACKAGE_VERSION - report version 1
mode       offline; nothing written; no file named as a credential was read; no network
read       no file content
inspected  config directory "<config>" (from --config-dir)
inspected  provider profiles "<config>/providers.json" (default; in the config directory)
result     OK - 0 error, 0 unknown, 0 warning; 9 not checked, 13 not available
```

Chaque fichier de clé est décrit par ses métadonnées et par la règle de la commande qui le lit. Son contenu n'est jamais lu :

```text
CREDENTIAL REFERENCES
  runner of installation  "<config>/secrets/runner"
          present; regular file; mode 0600; owned by the inspecting account; single link; private
          rule satisfied; reader private file reader; content not read
```

`OK` signifie que rien n'a été refusé parmi ce qui a été contrôlé : ici la disposition et la posture des fichiers de clés. Les neuf éléments non vérifiés comprennent les services en cours d'exécution, les arguments du gateway, l'état du moteur et le contenu des clés. Les sorties complètes sont [report.local-fresh.txt](examples/report.local-fresh.txt) et [report.local-fresh.json](examples/report.local-fresh.json).

### Une installation préparée, sur l'hôte qui l'a préparée

C'est le répertoire publié par `agentsandbox setup prepare`, comme le décrit le [guide SSO](../sso/README.fr.md). Passer le répertoire lui-même ; son `tenants.json` et son `oidc.json` sont inspectés ensemble :

```sh
"$AIGUARD_DIAGNOSE" --installation-dir /absolute/private/ai-guard/new-installation
```

Le rapport montre les paramètres SSO publics, les tenants du registre, ceux du plan de capacité et leur correspondance exacte :

```text
TENANTS
  registry  1 tenant  "ai-guard-demo"
  capacity plan  1 tenant  "ai-guard-demo"
  correspondence  match
```

L'exemple a été produit sur un nouveau répertoire privé, depuis le manifeste public de démonstration. La préparation ne contacte rien et ne démarre rien :

```sh
AIGUARD_SSO_TRY="$HOME/ai-guard-diagnose-sso-try"
mkdir -m 700 "$AIGUARD_SSO_TRY"
(
  umask 077
  set -C
  cat docs/setup/sso/examples/local-demo.json > "$AIGUARD_SSO_TRY/manifest.json"
)
"$AIGUARD_BIN" setup prepare --input "$AIGUARD_SSO_TRY/manifest.json" --output-dir "$AIGUARD_SSO_TRY/installation"
"$AIGUARD_DIAGNOSE" --installation-dir "$AIGUARD_SSO_TRY/installation"
"$AIGUARD_DIAGNOSE" --installation-dir "$AIGUARD_SSO_TRY/installation" --json
```

Les sorties complètes sont [report.prepared-v2.txt](examples/report.prepared-v2.txt) et [report.prepared-v2.json](examples/report.prepared-v2.json). Le manifeste nomme des identités publiques de démonstration : réserver une telle installation aux essais. Son fichier OIDC déclare le transport de démonstration, donc le rapport porte l'avertissement `identity.sso_demonstration_transport` et sa ligne de résultat compte un avertissement ; un fichier OIDC sous le transport `https_only` n'a pas ce constat.

Une installation qui vit dans un volume de conteneur, comme celle de la DEMO SSO Compose, s'inspecte dans ce volume : voir la section suivante.

### Inspecter une installation préparée dans son volume

La procédure Compose du [guide SSO](../sso/README.fr.md) prépare l'installation dans un volume nommé, sous le compte des conteneurs de contrôle. L'outil s'exécute donc là où se trouve l'installation : dans un conteneur ponctuel de l'image de contrôle, avec le volume monté en lecture seule et sans aucun réseau.

Cette section ne s'applique qu'à une image de contrôle qui contient l'outil : une image construite depuis une source dont le [Dockerfile de contrôle](../../../images/control.Dockerfile) compile et copie `ai-guard-diagnose`. Une image construite plus tôt ne le contient pas, et le premier `run` ci-dessous échoue alors avant toute inspection.

Les commandes de cette section n'ont pas été exécutées. Ni la construction de l'image de contrôle ni aucune commande `docker` ci-dessous n'a été lancée pour ce guide : le montage du volume en lecture seule, le compte `1000:1000` dans le conteneur et le code de sortie rendu par le moteur sont déduits des fichiers Compose, du Dockerfile de contrôle et du code de ce dépôt. Vérifier chaque résultat à la première exécution.

Prendre les deux valeurs sur l'installation existante, sans deviner un nom. Avec `docker inspect` sur le conteneur du gateway, relever `.Image`, l'ID immuable de l'image qu'il exécute, et le `.Name` de son montage `/data`. Si cette image ne contient pas l'outil, construire l'image de contrôle depuis une source au moins aussi récente que ce gateway, et relever son ID avec `docker image inspect`. Le sous-shell s'arrête au premier échec : un nom de volume mal saisi s'arrête à `volume inspect`, avant que `run` ne puisse créer un volume de ce nom.

```sh
AIGUARD_CONTROL_IMAGE_ID="sha256:inspected-current-control-image-id"
AIGUARD_INSTALLATION_VOLUME="existing-inspected-installation-volume"
(
set -eu
docker volume inspect --format '{{.Name}}' "$AIGUARD_INSTALLATION_VOLUME"
docker run --rm --pull never \
  --user 1000:1000 --network none --read-only --cap-drop ALL \
  --security-opt no-new-privileges:true --cpus 1 --memory 384m --memory-swap 384m --pids-limit 128 \
  --entrypoint ai-guard-diagnose "$AIGUARD_CONTROL_IMAGE_ID" --version
docker run --rm --pull never \
  --user 1000:1000 --network none --read-only --cap-drop ALL \
  --security-opt no-new-privileges:true --cpus 1 --memory 384m --memory-swap 384m --pids-limit 128 \
  --mount "type=volume,source=$AIGUARD_INSTALLATION_VOLUME,target=/data,readonly,volume-nocopy" \
  --entrypoint ai-guard-diagnose "$AIGUARD_CONTROL_IMAGE_ID" --installation-dir /data/installation
)
```

Le premier `run` ne monte rien et imprime la version de l'outil. Le second écrit le rapport :

- **Même chemin.** Le volume est monté sur `/data`, là où le montent les fichiers Compose fournis : les fichiers d'une installation enregistrent leurs chemins définitifs. Montée ailleurs, l'installation est rapportée avec `layout.references_outside_inspected_root`.
- **Même compte.** `1000:1000` est l'utilisateur des services de contrôle dans les fichiers Compose fournis. La règle du fichier privé n'accepte que le propriétaire d'un fichier.
- **Lecture seule, aucun réseau.** Le système de fichiers racine du conteneur et le volume sont en lecture seule. Une vérification active n'obtient aucune réponse depuis ce conteneur : ne pas donner `--active`.
- **Services en cours d'exécution.** Les services peuvent rester démarrés : l'outil n'ouvre aucune des bases de l'installation.
- **Code de sortie.** `docker run` rend le code de sortie de l'outil : les codes de ce guide s'appliquent. Un échec du moteur lui-même n'est pas un rapport.

Ajouter `--json` ou `--show-paths` après le répertoire, comme sur un hôte ; les chemins imprimés sont ceux du conteneur. Pour une installation Podman, utiliser les commandes `podman` correspondantes sur son propre volume existant, et qualifier à part son mapping UID.

### Profils fournisseur

Le fichier de profils d'un répertoire de configuration est inspecté avec le répertoire. Un fichier donné par `--providers-file` est inspecté à sa place, comme le lit `agentsandbox run` :

```sh
"$AIGUARD_DIAGNOSE" --config-dir "$AIGUARD_CONFIG" --providers-file "$AIGUARD_CONFIG/providers-subscriptions.json"
```

L'entrée publique [providers.two-modes.json](examples/providers.two-modes.json) déclare Codex CLI et Claude Code, chacun avec un profil abonnement et un profil API, avec des références relatives au fichier de profils. Pour voir son rapport sans aucun credential réel, créer un nouveau répertoire et deux fichiers privés vides. L'outil n'ouvre aucun fichier qu'un profil désigne comme son credential, leur contenu est donc indifférent :

```sh
AIGUARD_TRY="$HOME/ai-guard-diagnose-try"
"$AIGUARD_BIN" init --config-dir "$AIGUARD_TRY"
(
  umask 077
  set -C
  cat docs/setup/diagnostics/examples/providers.two-modes.json > "$AIGUARD_TRY/providers.two-modes.json"
  : > "$AIGUARD_TRY/secrets/providers.env"
  : > "$AIGUARD_TRY/secrets/claude-subscription.env"
)
"$AIGUARD_DIAGNOSE" --config-dir "$AIGUARD_TRY" --providers-file "$AIGUARD_TRY/providers.two-modes.json"
"$AIGUARD_DIAGNOSE" --config-dir "$AIGUARD_TRY" --providers-file "$AIGUARD_TRY/providers.two-modes.json" --json
```

Le fichier d'abonnement Codex est laissé absent à dessein. Le rapport liste les quatre profils, puis le profil que sélectionne un lancement géré. Aucune option de sélection n'est donnée ici, chaque partie de la sélection est donc automatique : les profils abonnement passent avant les profils API, puis `auto_priority` désigne l'abonnement Codex.

```text
PROVIDERS
  profile file  "<providers-file>"  loaded
  automatic priority  "codex-subscription" "claude-subscription" "codex-api" "claude-api"
```

```text
  selection of a managed launch
          requested client auto; mode auto; profile auto; model auto
          resolved profile "codex-subscription"; revision 1; client codex; mode subscription; model
          not declared; chosen by auto
```

Le fichier de ce profil est absent, un lancement demanderait donc une connexion. Le processus se termine avec le code 3, et le rapport dit qu'aucun profil API ne répond à sa place :

```text
  [error] provider.login_required
          The profile that the requested selection resolves is a subscription profile and its
          credential file does not exist. A managed launch with this selection refuses with
          login_required and never tries an API profile.
          provider profiles  1 in total "codex-subscription"
```

Les sorties complètes sont [report.providers-two-modes.txt](examples/report.providers-two-modes.txt) et [report.providers-two-modes.json](examples/report.providers-two-modes.json).

### Le profil que sélectionne un lancement géré

Donner à l'outil les options de sélection de `agentsandbox run`, avec les mêmes valeurs : `--client`, `--auth-mode`, `--client-profile` et `--model`. Une option omise est résolue automatiquement, comme le fait `run`. Le choix est fait par la fonction du produit qu'emploie un lancement géré : l'outil ne réécrit aucune règle, et cette fonction ne lit aucun fichier de credential. `$AIGUARD_CODEX_MODEL` est la variable de modèle définie dans le [guide des fournisseurs](../providers/README.fr.md).

```sh
"$AIGUARD_DIAGNOSE" --config-dir "$AIGUARD_TRY" --providers-file "$AIGUARD_TRY/providers.two-modes.json" --client claude-code --auth-mode subscription
"$AIGUARD_DIAGNOSE" --config-dir "$AIGUARD_TRY" --providers-file "$AIGUARD_TRY/providers.two-modes.json" --client claude-code --auth-mode subscription --json
"$AIGUARD_DIAGNOSE" --config-dir "$AIGUARD_TRY" --providers-file "$AIGUARD_TRY/providers.two-modes.json" --client codex --auth-mode api --model "$AIGUARD_CODEX_MODEL"
"$AIGUARD_DIAGNOSE" --config-dir "$AIGUARD_TRY" --providers-file "$AIGUARD_TRY/providers.two-modes.json" --client-profile codex-subscription
```

Avec les deux premières commandes, le choix explicite d'un abonnement est conservé. Le profil abonnement Claude est résolu et le processus se termine avec le code 0. Le fichier Codex absent appartient désormais à un profil que cette sélection ne résout pas : c'est un avertissement.

```text
given      requested client claude-code (from --client)
given      requested authentication mode subscription (from --auth-mode)
```

```text
  selection of a managed launch
          requested client explicit; mode explicit; profile auto; model auto
          resolved profile "claude-subscription"; revision 1; client claude_code; mode subscription;
          model not declared; chosen by auto
```

Les sorties complètes sont [report.providers-explicit-subscription.txt](examples/report.providers-explicit-subscription.txt) et [report.providers-explicit-subscription.json](examples/report.providers-explicit-subscription.json).

| Situation | Constat | Code de sortie |
|---|---|---|
| La sélection résout un profil | `provider.selection_resolved` | inchangé |
| Le profil résolu est en mode API | `provider.api_billing_notice`, un avertissement : l'usage est facturé par le fournisseur | inchangé |
| Le profil résolu est un profil abonnement et son fichier de credential est absent | `provider.login_required`, et `provider.subscription_never_falls_back_to_api` quand le fichier déclare aussi des profils API | 3 |
| Le profil résolu est un profil API et son fichier de credential est absent | `provider.credential_missing` | 3 |
| Le fichier de credential du profil résolu n'est pas sûr : son type, son mode, son nombre de liens, ou un répertoire qui y mène | `provider.credential_unsafe` | 3 |
| Aucun profil déclaré ne répond à la demande, par exemple un abonnement demandé sur un fichier de profils API | `provider.selection_no_profile`. Aucun profil de l'autre mode n'est employé à sa place : pour un abonnement demandé sur un fichier qui déclare des profils API, `provider.subscription_never_falls_back_to_api` le dit. | 3 |
| Plusieurs profils correspondent et rien ne tranche, ou la demande contredit le profil demandé | `provider.selection_ambiguous` | 3 |
| Un identifiant de profil ou de modèle demandé est refusé par le validateur du produit | `provider.selection_invalid` | 3 |
| Des options de sélection sont données et aucun fichier de profils n'a été chargé | `provider.selection_not_resolved`, non vérifié | inchangé |
| Le fichier de credential d'un profil que la sélection ne résout pas est absent ou non sûr | `provider.unselected_profile_reference_absent` ou `provider.unselected_profile_reference_unsafe`, des avertissements | inchangé |
| Le fichier de credential d'un profil est conforme, et son répertoire n'est pas un répertoire privé 0700 | `reference.private_rule_not_evaluated`, non vérifié : seul son lecteur contrôle le système de fichiers et les entrées d'accès du fichier | inchangé |

Sans aucune option de sélection, le rapport est calculé pour le lancement géré entièrement automatique : la sélection que construit `agentsandbox run` quand `--client`, `--auth-mode`, `--client-profile` et `--model` sont tous omis, par exemple avec `--prompt` ou `--providers-file` seul. Quand le produit refuse cette sélection, parce que plusieurs profils correspondent et que ni `default_profile` ni `auto_priority` ne nomme l'un d'eux, ou parce qu'aucun profil ne correspond, le rapport porte `provider.selection_ambiguous` ou `provider.selection_no_profile`, le statut `error` et le code de sortie 3, alors qu'une sélection qui nomme un profil déclaré par `--client-profile ID` le résout. Donner à l'outil les options de sélection du lancement visé, pour obtenir le verdict de ce lancement.

La sélection n'établit ni la présence du client dans l'image du worker, ni sa version, ni la validité du compte ou de l'abonnement : ces points restent parmi les limites de tout rapport. Le contenu du credential sélectionné n'est pas vérifié non plus : `agentsandbox run` le valide avant l'autorisation.

### Autres entrées

Un registre ou un fichier OIDC peut être donné seul, tel qu'il est passé au gateway. L'exemple public de registre enregistre des chemins de conteneur : sur un poste de travail, les fichiers qu'il nomme sont rapportés `unknown` et le processus se termine avec le code 4 :

```sh
"$AIGUARD_DIAGNOSE" --tenants-file config/tenants.example.json
"$AIGUARD_DIAGNOSE" --tenants-file /absolute/private/ai-guard/tenants.json --oidc-config /absolute/private/ai-guard/oidc.json
```

Le programme du moteur est approuvé par la règle que le produit applique avant tout appel au moteur. Il est haché, jamais exécuté :

```sh
"$AIGUARD_DIAGNOSE" --config-dir "$AIGUARD_CONFIG" --runtime docker
"$AIGUARD_DIAGNOSE" --config-dir "$AIGUARD_CONFIG" --runtime podman --runtime-program /absolute/path/to/podman
```

Les éléments de confiance et le transport du moteur de décision sont des options du gateway, qui ne sont conservées nulle part. Donner les mêmes valeurs que le lanceur du gateway pour les faire valider ; rien n'est contacté. Une option que le lancement ne peut pas employer, comme une borne d'événements pour un registre sans politique qui en dépend, est rapportée comme non vérifiée :

```sh
"$AIGUARD_DIAGNOSE" --config-dir "$AIGUARD_CONFIG" --jev-url https://decision.example.invalid/v1 --jev-ca-file /absolute/path/public-ca.pem
"$AIGUARD_DIAGNOSE" --config-dir "$AIGUARD_CONFIG" --gateway-ca-file /absolute/path/public-ca.pem
"$AIGUARD_DIAGNOSE" --installation-dir /absolute/private/ai-guard/new-installation --max-events 5000
```

### Interroger le fournisseur d'identité, sur demande

Par défaut, le fournisseur d'identité d'un fichier OIDC n'est jamais contacté, et le rapport le dit :

```text
  [not_checked] identity.idp_reachability_not_checked
          The identity provider was not contacted: this tool opens no connection unless an active
          check is asked for. The gateway contacts the provider when a sign-in starts.
          scope    file oidc_file
          basis    observed
          requires  --active oidc-discovery, with an OIDC file that passes the offline rules
```

La vérification contacte les hôtes que nomme le fichier inspecté. Lancer d'abord l'outil sans `--active`, lire `issuer` et `keys endpoint` dans le bloc d'identité, puis n'ajouter `--active oidc-discovery` que pour un fournisseur que l'on entend contacter depuis cet hôte.

`--active oidc-discovery` lance la découverte du client OIDC du produit lui-même contre ce fournisseur, une fois, depuis l'endroit où l'outil s'exécute :

```sh
"$AIGUARD_DIAGNOSE" --installation-dir /absolute/private/ai-guard/new-installation --active oidc-discovery
"$AIGUARD_DIAGNOSE" --tenants-file /absolute/private/ai-guard/tenants.json --oidc-config /absolute/private/ai-guard/oidc.json --active oidc-discovery --json
```

Le rapport dit alors ce qui a été fait. Sa ligne `mode` indique `active checks run`, une ligne `active` suit avec `oidc discovery; network used by these checks only`, et le bloc d'identité indique `identity provider  contacted`. Dans le document JSON, `mode.network` vaut `active_checks_only`, `mode.active_checks_executed` vaut `oidc_discovery`, et `identity.sso.idp_contacted` vaut `true`.

| Résultat | Constat | Statut |
|---|---|---|
| Le fournisseur répond, confirme le fichier, et ses clés sont utilisables | `active.oidc_discovery_ok` | `ok` |
| Aucune réponse utilisable depuis l'endroit où l'outil s'exécute | `active.oidc_discovery_transport_unavailable` | `unknown`, code de sortie 4 quand il n'y a pas d'erreur |
| Une réponse qui n'est pas une découverte | `active.oidc_discovery_provider_rejected` | `error` |
| Le document de découverte ne confirme pas le fichier | `active.oidc_discovery_metadata_mismatch` | `error` |
| Le jeu de clés est refusé | `active.oidc_discovery_keys_invalid` | `error` |
| La vérification est demandée, et le fichier OIDC ne passe pas les règles hors ligne ou aucun fichier OIDC n'est inspecté | `identity.idp_reachability_not_checked`. Rien n'est contacté. | `not_checked` |

La vérification ne s'exécute que pour un fichier OIDC qui passe toutes les règles hors ligne. Ce qu'elle envoie, ses bornes et ce qu'elle n'imprime jamais sont décrits sous « La découverte active », dans la section sur l'hygiène des secrets.

## Lire le rapport

### Rapport texte

Le texte est brut, en anglais, sans couleur, sur 100 colonnes au plus sauf quand l'une de vos propres valeurs est plus longue. Une chaîne qui vient de vos fichiers ou de votre ligne de commande est toujours imprimée entre guillemets doubles.

| Ligne ou bloc | Contenu |
|---|---|
| Première ligne | Nom de l'outil, version du paquet, version du rapport. |
| `mode` | Ce que le lancement a fait : hors ligne, rien d'écrit, aucun fichier nommé comme credential n'a été lu, aucun réseau. « Nommé » signifie nommé par les fichiers inspectés dans ce lancement. La troisième partie devient `a file later named as a credential was read` quand `mode.credential_contents_read` vaut `detected_after_read`. Quand une vérification active s'est exécutée, la première partie devient `active checks run` et la dernière partie passe sur la ligne `active`. |
| `active` | Présente seulement quand une vérification active s'est exécutée : les vérifications qui ont contacté quelque chose, puis `network used by these checks only`. |
| `read` | La liste fermée des fichiers ouverts pour leur contenu pendant ce lancement, ou `no file content`. |
| `inspected`, `given`, `assumed` | Une ligne par entrée : son affichage et sa provenance. `assumed` marque une valeur que l'outil a fournie parce qu'aucune n'était donnée. |
| `result` | `OK`, `ERROR` ou `UNKNOWN`, puis le nombre d'erreurs, d'inconnus, d'avertissements, d'éléments non vérifiés et d'éléments non disponibles. |
| `omitted` | Présente dès que quelque chose qui a été évalué n'est pas listé : le nombre d'entrées de références, d'entrées de données et d'identifiants de tenants qui ne sont pas imprimés, parce qu'une liste a atteint sa borne ou parce que les listes de détail ont été vidées pour respecter la borne de taille. |
| Un bloc par axe | Les faits de l'axe, puis ses constats. Un axe sans fait ni constat n'est pas imprimé. |
| `NOT CHECKED` | Chaque élément que ce lancement n'a pas examiné, avec ce qui le résout. |
| `NOT AVAILABLE IN THIS BUILD` | Chaque élément que rien dans ce build ne sait examiner hors ligne. |
| Dernière ligne | `exit` et le code de sortie. |

Un constat imprime son statut et son code, son message, ses détails quand il en a, les tenants ou les profils qu'il concerne, puis :

| Libellé | Sens |
|---|---|
| `scope` | Ce sur quoi porte le constat : une entrée, les tenants, les profils fournisseur, un fichier lu pour son contenu, l'hôte, ou le build. |
| `basis` | `observed`, `declared` ou `inferred`, puis le fichier et le champ dont le constat est tiré. |
| `applies` | La commande qui applique la règle. |
| `assumes` | Les entrées supposées dont dépend le verdict. |
| `requires` | Pour un élément non vérifié : l'option de cet outil, la commande existante, ou l'endroit qui détient la réponse. |
| `action` | Ce qu'il faut faire ensuite. Une action ne propose jamais d'affaiblir TLS, le confinement ou l'audit. |

L'en-tête ne donne que des comptes : les constats se trouvent dans les blocs de leurs axes. Chacun commence par son statut entre crochets, puis son code, comme dans `[error] provider.login_required`. Pour trouver les erreurs et les observations non concluantes d'un long rapport, chercher `[error]` et `[unknown]` dans le texte, par exemple avec `grep -F '[error]'`, ou lire `findings` dans le document JSON et garder les entrées dont le `status` vaut `error` ou `unknown`.

### Statuts

| Statut | Sens | Code de sortie |
|---|---|---|
| `ok` | Un validateur du produit a accepté l'élément, ou les règles hors ligne reformulées dans cet outil n'ont trouvé aucune violation. Le message dit lequel. | inchangé |
| `info` | Un fait neutre. | inchangé |
| `warning` | Utilisable, mais déviant ou à risque. | inchangé |
| `error` | La commande consommatrice refuse cela de façon certaine, ou deux fichiers se contredisent, ou cet outil a retenu un fichier qu'il n'ouvre jamais : un fichier donné sous `secrets`, `mounted-secrets` ou `tenants` d'un répertoire inspecté, ou un fichier qu'un répertoire inspecté ou un fichier inspecté désigne comme fichier de clé, credential, enregistrement de setup ou base. C'est un refus de cet outil, pas de la commande consommatrice. | 3 |
| `unknown` | L'observation a été tentée et n'est pas concluante. | 4 en l'absence d'erreur |
| `not_checked` | Non examiné pendant ce lancement. `requires` nomme ce qui le résout. | inchangé |
| `not_available` | Ce diagnostic n’a pas de lecteur hors ligne pour cet élément. Une autre commande explicite du produit peut examiner un périmètre plus précis ; `requires` vaut `null`. | inchangé |

Un fait du système de fichiers conforme ne produit jamais de `ok` pour un credential : c'est une condition nécessaire, et seule la commande consommatrice contrôle le contenu.

### Base et portée

| `basis` | Sens |
|---|---|
| `observed` | Un fait du système de fichiers, ou le résultat d'une fonction du produit appelée par cet outil. |
| `declared` | Un fichier le dit. |
| `inferred` | La conséquence d'une règle citée ou d'une entrée supposée. Les verdicts sur le registre et sur le fichier OIDC sont de cette nature : cet outil reformule les règles structurelles du chargeur du gateway, et le chargeur lui-même n'a pas été exécuté. |

| `kind` de `scope` | Sens |
|---|---|
| `input` | Une entrée de `inputs`, nommée par `input`. |
| `tenants` | Les tenants du registre et du plan. |
| `provider_profiles` | Les profils fournisseur déclarés. |
| `file` | Un fichier lu pour son contenu, nommé par `file`. |
| `host` | L'hôte sur lequel tourne l'outil. |
| `build` | Une limite de ce build, listée dans `limits`. |

Les entrées qui partagent un code, une portée et les mêmes détails forment un seul constat. Les tenants ou les profils qu'il concerne sont ses `subjects` : un `total` exact, au plus 16 `ids`, et le nombre `omitted`.

### Axes

Les axes sont indépendants. Une faute sur un axe ne change pas ce qu'un autre axe rapporte : le SSO est inspecté sans aucun fournisseur IA, et un profil fournisseur sans aucun fichier SSO.

| Axe | Contenu |
|---|---|
| `layout` | La disposition que contient chaque répertoire inspecté, et l'état d'une préparation. |
| `topology` | Tenant local unique, ou tenants d'un registre. |
| `identity` | Références de clés techniques par présence ; paramètres publics d'un fichier OIDC. |
| `tenants` | Le registre, le plan de capacité et leur correspondance exacte. |
| `credential_references` | Chaque fichier de clé, credential fournisseur et enregistrement de setup référencé, par ses métadonnées. |
| `providers` | Le fichier de profils fournisseur et ses profils : client, mode d'authentification, source du credential. Le profil que sélectionne un lancement géré. |
| `inference` | Le transport du moteur de décision donné en ligne de commande. |
| `trust` | Les bundles d'AC : celui du fichier OIDC, et ceux donnés par option. |
| `engine` | Le programme du moteur, approuvé ou refusé, jamais exécuté. |
| `data` | Les bases, par présence et sidecars. Aucune base n'est ouverte. |
| `output` | Ce que le rapport lui-même a masqué ou omis. |

### Codes de sortie

| Code | Sens | Sortie |
|---|---|---|
| 0 | Un rapport a été écrit ; il n'a ni `error` ni `unknown`. | Rapport sur stdout ; stderr vide. |
| 3 | Un rapport a été écrit ; il a au moins un `error`. | Idem. |
| 4 | Un rapport a été écrit ; il n'a aucun `error` et au moins un `unknown`. | Idem. |
| 2 | Erreur d'usage. | Une phrase fermée sur stderr ; stdout vide. |
| 1 | Refus : aucun rapport. | Une phrase fermée avec une étape suivante sur stderr ; stdout vide. |

Le code de sortie 0 signifie que rien n'a été refusé parmi ce qui a été contrôlé. Il ne signifie pas que l'installation démarre : `warning`, `info`, `not_checked` et `not_available` ne changent jamais le code de sortie, et plusieurs d'entre eux figurent dans tout rapport. La ligne `result` dit combien d'éléments n'ont pas été examinés.

Sans `--client`, `--auth-mode`, `--client-profile` ni `--model`, les constats de l'axe fournisseur et le code de sortie sont ceux d'un lancement géré qui omet ces options. Pour conditionner une suite au lancement réellement effectué, donner à cet outil les mêmes options. `providers.selection.requested` dit quelles parties étaient automatiques : elles ne figurent ni dans `inputs` ni dans `assumes`.

### Refus et erreurs d'usage

Une erreur d'usage ne répète jamais votre argument. Elle imprime l'une de trois phrases, chacune suivie de `run ai-guard-diagnose --help` :

| Phrase | Cause |
|---|---|
| `unknown option or unexpected argument` | Une option qui n'existe pas, ou un argument positionnel. |
| `an option has a value outside its closed set` | Par exemple un `--runtime` autre que `docker` ou `podman`, un `--client` ou un `--auth-mode` hors de ses valeurs, ou un `--max-events` hors de 1 à 1000000. |
| `invalid command line` | Toute autre erreur : deux options qui s'excluent, ou une option donnée sans l'option qu'elle exige. La phrase ne dit pas quelle règle est enfreinte : les trois règles sont énoncées dans l'aide, à côté des options qu'elles concernent, et dans les notes de la section Options. |

Un refus n'écrit rien sur stdout :

| Texte sur stderr | Quand |
|---|---|
| `ai-guard-diagnose: an argument path cannot be used: it contains a parent traversal, or the current directory is unavailable. No report was written. Use a path without parent traversal.` | Un argument de chemin contient `..`, ou un argument relatif ne peut pas être résolu. |
| `ai-guard-diagnose: the report was withheld by the output guard. No report was written. Rerun with --json; if it persists, report a defect with the command line only.` | La garde de sortie finale réécrirait encore le rapport. Ce cas n'est pas attendu. |
| `ai-guard-diagnose: the report exceeds its size bound. No report was written. Inspect one target at a time.` | Le document dépasse 524288 octets même avec ses listes de détail vidées. |
| `ai-guard-diagnose: the report could not be written. Check the output destination.` | L'unique écriture sur stdout a échoué. |
| `ai-guard-diagnose: internal error. No report was written. Report a defect with the command line only.` | Un défaut de l'outil. |

Le premier et le quatrième de ces refus sont produits par des lancements de l'outil. La garde de sortie, le refus de taille et l'erreur interne sont implémentés et n'ont été produits par aucun lancement.

## Options

| Option | Défaut | Sens |
|---|---|---|
| `--config-dir DIR` | `.agentsandbox`, seulement quand aucune option de cible n'est donnée | Répertoire de configuration historique, tel que donné à `init`, `run` et `providers`. Son `providers.json` est inspecté aussi, sauf si `--providers-file` nomme un autre fichier. |
| `--installation-dir DIR` | aucun | Répertoire créé par `setup prepare`, ou son répertoire de staging conservé. Son propre registre et son propre fichier OIDC sont inspectés. |
| `--tenants-file FILE` | aucun | Registre de tenants, tel que donné au gateway. |
| `--oidc-config FILE` | aucun | Fichier OIDC, tel que donné au gateway. |
| `--providers-file FILE` | `providers.json` du répertoire de configuration | Profils fournisseur, tels que donnés à `run`. |
| `--client` `auto`, `codex` ou `claude-code` | `auto` | Client IA demandé à un lancement géré, tel que donné à `run`. |
| `--auth-mode` `auto`, `api` ou `subscription` | `auto` | Mode d'authentification demandé à un lancement géré, tel que donné à `run`. |
| `--client-profile ID` | aucun | Profil fournisseur demandé à un lancement géré, tel que donné à `run`. |
| `--model ID` | aucun | Modèle demandé à un lancement géré, tel que donné à `run`. Il remplace le modèle du profil. |
| `--max-events N` | 100000, rapporté comme supposé quand une politique de tenant en dépend | Borne d'événements du gateway, de 1 à 1000000, employée pour valider les politiques de capacité d'audit et d'audit des workloads des tenants. Une politique refusée à la borne supposée est rapportée `unknown`, par `registry.policy_refused_at_assumed_event_bound` ; refusée à une borne que vous donnez, elle est une `error`. |
| `--activity-max-events N` | 1000 | Budget d'événements par session de l'activité fournisseur live du registre du broker, de 1 à 5000. Avec `--max-events`, il dimensionne l'audit du tenant : en dessous de 50 sessions, le rapport avertit par `audit.live_activity_budget_capacity`. |
| `--activity-max-bytes OCTETS` | 2097152 | Budget d'octets par session de l'activité fournisseur live du registre du broker, de 65536 à 8388608. Le plafond physique de 256 Mio divisé par ce budget en dessous de 50 sessions est rapporté par `audit.live_activity_budget_capacity`. |
| `--jev-url URL` | aucun | URL du moteur de décision à valider. Jamais contactée. Pas avec `--jev-local-url`. |
| `--jev-local-url URL` | aucun | URL du moteur de décision local à valider. Jamais contactée. Pas avec `--jev-url`. |
| `--jev-allow-local-http` | inactif | Indique que le gateway est démarré avec sa permission explicite de HTTP local. Jugée seulement avec une URL de moteur de décision. |
| `--jev-ca-file FILE` | aucun | Bundle public d'AC donné au gateway pour le moteur de décision. |
| `--gateway-ca-file FILE` | aucun | Bundle public d'AC employé par les clients d'un gateway HTTPS. |
| `--runtime` `docker` ou `podman` | aucun | Moteur dont le programme est approuvé sans être exécuté. |
| `--runtime-program ABS` | les emplacements système fixes du produit | Programme absolu du moteur. Exige `--runtime`. |
| `--json` | inactif | Le document JSON versionné à la place du texte. |
| `--show-paths` | inactif | Chemins absolus à la place des emplacements symboliques. |
| `--active` `oidc-discovery` | aucun | Vérification active à exécuter. Rien n'est contacté sans elle. `oidc-discovery` demande au fournisseur d'identité du fichier OIDC inspecté son document de découverte et ses clés. |

- Les options de cible sont `--config-dir`, `--installation-dir`, `--tenants-file`, `--oidc-config` et `--providers-file`. Quand aucune n'est donnée, `.agentsandbox` est inspecté, même quand seuls `--runtime` ou une option du moteur de décision sont présents : son absence est alors une erreur.
- `--installation-dir` apporte ses deux fichiers. Il est refusé avec `--tenants-file` ou `--oidc-config`.
- `--jev-url` et `--jev-local-url` s'excluent. `--runtime-program` exige `--runtime`.
- `--client`, `--auth-mode`, `--client-profile` et `--model` disent pour quel lancement géré le rapport est calculé. Elles ne changent rien dans l'installation. Quand aucun fichier de profils n'est chargé, elles ne résolvent rien, et le rapport le dit.
- `--jev-allow-local-http` décrit le gateway. Elle n'assouplit rien : le validateur du produit décide toujours. Il n'accepte le HTTP en clair que pour une adresse loopback, et il refuse cette permission avec un bundle d'AC, quelle que soit l'URL.
- `--jev-allow-local-http` et `--jev-ca-file` ne sont jugées comme un transport qu'avec `--jev-url` ou `--jev-local-url`. Sans URL elles sont listées parmi les entrées, le rapport dit qu'aucun transport n'a été jugé, par `inference.decision_transport_inputs_without_url`, et un bundle n'est jugé que par sa grammaire. L'outil ne valide que le transport et le bundle : il ne vérifie pas le jeu d'options du gateway, qui exige une sélection de backend, un modèle et, pour le backend TypeSafe, un fichier de clé, et qui refuse un jeu incomplet ou mixte.
- `--max-events` n'est employé que si un tenant d'un registre inspecté déclare une politique de capacité d'audit ou d'audit des workloads ; un registre écrit par `setup prepare` n'en déclare aucune. Sinon la valeur est listée parmi les entrées, rien n'est validé avec elle, et le rapport le dit par `registry.event_bound_not_used`.
- Un argument de chemin qui contient `..` est refusé avant toute lecture.
- `--active` est la seule option qui fait ouvrir une connexion à l'outil. Elle exige un fichier OIDC qui passe les règles hors ligne ; sinon rien n'est contacté et le rapport le dit.

## Priorités de configuration

L'outil ne lit aucune configuration qui lui soit propre. Ce tableau indique d'où vient chaque valeur dans le produit, pour comparer un rapport à la bonne source. La première correspondance l'emporte.

| Réglage | Ordre |
|---|---|
| Interpolation Compose | L'environnement du shell, puis le fichier nommé par `--env-file`, puis le défaut écrit dans le fichier Compose. |
| Options de `agentsandbox` | Le flag, puis son défaut intégré. Le CLI ne lit jamais de fichier dotenv. |
| Options du moteur de décision du gateway | Le flag, puis sa variable nommée : `JEV_URL`, `JEV_API_KEY_FILE`, `JEV_MODEL`, `JEV_CA_FILE`. |
| Backend du moteur de décision du gateway | `--jev-backend`, puis `JEV_BACKEND`, puis une option d'URL historique seule, puis aucun moteur de décision. |
| Fichier de profils fournisseur | `--providers-file`, puis `providers.json` du répertoire de configuration. |
| Profil fournisseur sélectionné par un lancement | Un profil explicite, puis `default_profile` parmi les candidats, puis un candidat unique, puis `auto_priority`. En mode d'authentification automatique, les candidats abonnement excluent les candidats API. L'outil demande ce choix au produit, pour les options de sélection qui lui sont données. |
| Références de fichiers | Absolues telles qu'écrites. Relatives au répertoire du fichier qui les déclare : registre, fichier OIDC, fichier de profils fournisseur. |
| `ai-guard-diagnose` | Options de ligne de commande uniquement. `--max-events` est supposé valoir 100000 quand il n'est pas donné. |

Ce qui n'est pas conservé dans une installation ne peut pas y être lu : les arguments du gateway, le moteur de décision sélectionné, le chemin de la base d'un répertoire historique et l'activation du SSO sont rapportés comme non vérifiés. Un choix explicite d'abonnement reste un choix d'abonnement : ni la règle du produit ni cet outil ne le remplacent par un profil API. L'inverse n'est pas vrai : l'usage facturé d'une API ne demande pas de demande explicite. En mode d'authentification automatique, le produit peut résoudre un profil API quand aucun profil abonnement ne correspond au client demandé, par exemple avec `--client` seul ; quand il le fait, le rapport porte `provider.api_billing_notice`. Deux demandes écartent un profil API : `--auth-mode subscription`, et `--client-profile` quand il nomme un profil abonnement.

## États inconnus, non vérifiés et non disponibles

Une valeur qui n'a pas été observée n'est jamais rapportée comme un succès, ni comme zéro.

### `unknown` : observé, non concluant

| Code | Quand | Ce qui le résout |
|---|---|---|
| `layout.inspector_identity_differs` | Le répertoire appartient à un autre compte, ou n'a pas pu être examiné depuis celui-ci. Rien de ce qu'il contient n'est décrit. | Lancer l'outil sous le compte de contrôle propriétaire de l'installation. |
| `registry.policy_refused_at_assumed_event_bound` | Une politique de tenant dont le verdict dépend de la borne d'événements du gateway est refusée à la borne que cet outil a supposée, 100000, parce que `--max-events` n'a pas été donné. Le gateway peut être démarré avec une autre borne. | Donner la borne d'événements du gateway avec `--max-events N`. Un gateway démarré sans cette option emploie 100000 et refuse la politique. |
| `reference.not_found_from_here` | Un chemin référencé hors du répertoire inspecté n'a pas été trouvé. Il peut exister là où tourne son consommateur. | Lancer l'outil là où tourne le gateway, ou ajouter `--show-paths` pour voir le chemin enregistré. |
| `reference.inspector_identity_differs` | Un fichier référencé appartient à un autre compte. Son consommateur peut tourner sous ce compte : sa règle n'a donc pas été jugée. Le même code est rapporté pour un registre d'un autre compte que ce compte ne peut pas lire. | Lancer l'outil sous le compte de contrôle. |
| `reference.metadata_unavailable` | Les métadonnées d'un chemin référencé n'ont pas pu être lues, pour une raison autre que l'absence. | Vérifier les permissions des répertoires qui y mènent. |
| `active.oidc_discovery_transport_unavailable` | La découverte active n'a obtenu aucune réponse utilisable depuis l'endroit où l'outil s'exécute. Le gateway peut joindre le fournisseur depuis un autre réseau. | Lancer la vérification là où tourne le gateway ; vérifier que l'émetteur est joignable et que le bundle d'AC du fichier OIDC couvre son certificat. |

### `not_checked` : non examiné pendant ce lancement

`requires` est imprimé tel que montré ici.

| Code | Ce qui n'est pas examiné | `requires` |
|---|---|---|
| `topology.running_services_not_checked` | Si le gateway, le dashboard ou un worker est en cours d'exécution. | `agentsandbox healthcheck --url URL` |
| `topology.gateway_arguments_not_persisted` | Adresses d'écoute, fichier de politique et borne d'événements. | `the command line or the Compose file of the launcher` |
| `identity.sso_not_inspected` | Le SSO, quand aucun fichier OIDC n'a été donné. | `--oidc-config FILE or --installation-dir DIR` |
| `identity.sso_gateway_enablement_not_persisted` | Si le gateway est démarré avec le fichier OIDC. | `the command line of the gateway launcher` |
| `identity.idp_reachability_not_checked` | Si le fournisseur d'identité répond. | `--active oidc-discovery, with an OIDC file that passes the offline rules` |
| `oidc.loader_acceptance_not_checked` | L'acceptation du fichier OIDC par le gateway, qui le compare aussi à l'état durable d'authentification. | `the gateway checks the stores and the durable authentication state when it starts` |
| `registry.loader_acceptance_not_checked` | L'acceptation du registre par le gateway, qui lit aussi chaque clé et ouvre chaque base. | `the gateway checks token content, token uniqueness and the stores when it starts` |
| `registry.event_bound_not_used` | Quoi que ce soit avec la borne d'événements donnée par `--max-events`, quand aucun registre décodé ne déclare une politique qui en dépend. | `a registry whose tenants declare an audit-capacity or a workload-audit policy` |
| `tenants.correspondence_not_checked` | La comparaison des tenants et du plan, quand l'un des deux fichiers manque ou n'est pas décodé. Pour un répertoire de configuration historique et un fichier OIDC sans registre, l'action énonce la règle d'un gateway démarré sans registre : il sert le seul tenant `local`. | `both --tenants-file FILE and --oidc-config FILE, or --installation-dir DIR` |
| `reference.content_not_checked` | Le contenu de chaque fichier de clé, de jeton et de credential. | `the consuming command checks the content when it starts` |
| `reference.private_rule_not_evaluated` | Le système de fichiers et les entrées d'accès étendues d'un fichier privé conforme dont le répertoire n'est pas un répertoire privé 0700. | `the consuming command checks the file system and the access entries of the file when it opens it` |
| `provider.not_inspected` | Les profils fournisseur, quand aucun emplacement n'a été donné ou que le répertoire de configuration n'a pas pu être inspecté. | `--config-dir DIR or --providers-file FILE` |
| `provider.credential_content_not_checked` | Le contenu des credentials fournisseur. | `agentsandbox run validates the selected credential before authorization` |
| `provider.manual_launch_path_not_checked` | Le chemin de lancement manuel, qui n'emploie pas le fichier de profils. | `agentsandbox run` |
| `provider.selection_not_resolved` | La sélection, quand des options de sélection sont données et qu'aucun fichier de profils n'a été chargé. | `a provider profile file that the product loader accepts` |
| `inference.decision_engine_selection_not_persisted` | Le moteur de décision que sélectionne le gateway. | `the command line or the Compose file of the launcher` |
| `inference.decision_transport_not_checked` | Le transport du moteur de décision, quand le bundle d'AC donné avec lui n'a pas pu être utilisé et que l'URL seule n'est pas refusée. | `a CA bundle that the product accepts, given with --jev-ca-file FILE` |
| `inference.decision_transport_inputs_without_url` | Le transport du moteur de décision, quand la permission de HTTP local ou un bundle d'AC est donné sans URL. | `--jev-url URL or --jev-local-url URL` |
| `engine.program_not_inspected` | Le programme du moteur, quand aucun moteur n'a été nommé. | `--runtime docker or --runtime podman` |
| `engine.state_not_checked` | Si le moteur tourne, sa version et son mode rootless. | `agentsandbox doctor --runtime docker or agentsandbox doctor --runtime podman` |
| `data.location_not_persisted` | La base d'un répertoire de configuration historique : son chemin est une option du gateway. | `the command line or the Compose file of the launcher` |
| `data.signed_checkpoint_not_checked` | Checkpoints signés du tenant et continuité anti-rollback indépendante ; aucun checkpoint, ancre ou base n’est lu par ce diagnostic. | `explicit ai-guard-audit-checkpoint verify with tenant checkpoint evidence and an independently held trust anchor` |

### `not_available` : non examiné par ce diagnostic

| Code | Ce que ce diagnostic ne sait pas examiner hors ligne | Où la réponse est établie |
|---|---|---|
| `identity.local_named_accounts_not_available` | Les comptes locaux nominatifs et les appartenances : ils vivent dans les bases des tenants. | Rien dans ce build, hors ligne. |
| `tenants.durable_plan_comparison_not_available` | La comparaison du plan avec celui que conserve l'audit d'authentification. Elle est listée à côté d'une concordance comme d'un écart : deux fichiers qui concordent peuvent encore différer du plan conservé. | Le gateway, à son démarrage. |
| `inference.local_model_backend_not_available` | Image, poids, budget mémoire et disponibilité d'un backend de modèle local. | Le [guide IA locale](../local-ai/README.fr.md). |
| `inference.model_tls_outputs_not_available` | Les sorties du provisionnement TLS du modèle. | Rien dans ce build. |
| `trust.certificate_validity_period_not_available` | Les dates de validité : un certificat expiré n'est pas détecté. | L'outillage de votre autorité de certification. |
| `data.offline_inspection_not_available` | Ce diagnostic n’ouvre aucune connexion SQLite vers les bases tenant ou l’audit d’authentification ; schéma, liaison, format et intégrité n’y sont pas examinés. | Démarrage du gateway et contrôle `verify-audit` en ligne selon leur périmètre local ; la vérification signée hors ligne du tenant est une opération distincte. |
| `provider.qualified_versions_not_available` | Une liste de versions de clients qualifiées : ce build n'en contient aucune. | Votre propre relevé des versions construites dans l'image worker. |
| `provider.client_presence_not_available` | Si Codex CLI ou Claude Code est présent. | Le worker, à son démarrage. |
| `provider.client_version_not_available` | La version d'un client. | Le worker, à son démarrage. |
| `provider.account_validity_not_available` | Si un compte ou un abonnement est valide. | Le fournisseur, à la première utilisation. |
| `identity.arbitrary_oidc_provider_not_qualified` | La compatibilité avec un fournisseur d'identité donné. | Une connexion réelle, comme le décrit le guide SSO. |
| `inference.internal_profile_not_available` | Un profil géré d'inférence interne : un profil fournisseur ne porte ni URL, ni format, ni champ d'AC. | Le [guide entreprise](../enterprise/README.fr.md) indique ce qui est livré. |
| `topology.kubernetes_launch_not_available` | Le lancement de workers par l'adaptateur Kubernetes. | `agentsandbox kubernetes doctor` inspecte une autorité sans admettre de workers. |
| `layout.sibling_staging_not_inspected` | Un répertoire de staging laissé à côté d'une installation publiée : il n'est pas recherché. | Passer le répertoire de staging conservé en `--installation-dir`. |
| `tool.linux_macos_posture_policy_only` | Un système de fichiers réseau ou FUSE : la politique de posture le refuse sans l'examiner. | Garder l'installation sur un système de fichiers local pris en charge. |

Les neuf derniers sont des limites du build. Ils figurent dans la section `limits` de tout rapport, avec la portée `build`.

## Dispositions et commandes qui les lisent

Deux dispositions existent. Les chemins historiques ordinaires lisent la disposition historique ; le gateway lit une installation préparée par son registre et son fichier OIDC. Le contrôle en ligne `verify-audit` accepte aussi un fichier viewer explicite pour un tenant préparé.

| Disposition | Créée par | Reconnue par | Lue par |
|---|---|---|---|
| Répertoire de configuration historique | `agentsandbox init` | `secrets` ou `mounted-secrets` | `init`, `up`, `down`, `run`, `worker-status`, `worker-resume`, `worker-stop`, `export-workspace`, `verify-audit` et `providers` |
| Installation préparée | `agentsandbox setup prepare` | `preparation.json` ou `completion.json` | Le gateway, par `--tenants-file` et `--oidc-config` |

Un gateway démarré sans registre sert un seul tenant, nommé `local`. Conserver un répertoire de configuration historique distinct pour les commandes historiques. Pour un tenant préparé, suivre le [guide local](../local/README.fr.md) et `agentsandbox verify-audit --viewer-token-file FILE --gateway URL` ; cette voie en ligne lit le fichier viewer explicite, pas le registre préparé. Un registre donné seul par `--tenants-file` est rapporté comme `hand_written_registry`.

`ai-guard-diagnose` n’ouvre jamais de connexion SQLite. Pour vérifier hors ligne les preuves signées d’un tenant, lancer volontairement `ai-guard-audit-checkpoint verify` avec une ancre tenue indépendamment selon le [guide des checkpoints](../audit-signatures/README.fr.md). La comparaison facultative à la base ouvre un instantané cohérent en lecture seule et peut créer des sidecars SQLite et le fichier de verrou d’ouverture réservé. Ces effets appartiennent à cette opération explicite, pas au diagnostic. Chaîne locale, checkpoint signé et continuité anti-rollback indépendante sont trois contrôles distincts.

`layouts[].kind` vaut `absent`, `historical_config_dir`, `prepared_installation`, `setup_staging_directory`, `mixed`, `hand_written_registry` ou `unrecognized`. `preparation` vaut `complete`, `completion_marker_absent`, `incomplete` ou `no_intent`.

| Situation | Constat | Action |
|---|---|---|
| Le répertoire n'existe pas | `layout.target_absent` | Vérifier d'abord le chemin, ou passer le répertoire d'une installation qui existe ailleurs. Sinon créer une installation par `init` ou `setup prepare`. |
| Le répertoire ne contient aucune des deux dispositions | `layout.unrecognized` | Donner le répertoire passé à `init`, ou celui créé par `setup prepare`. Quand il contient `tenants.json` ou `oidc.json` sans enregistrement de setup, passer ces fichiers par `--tenants-file` et `--oidc-config` : le répertoire n'est pas inspecté comme une installation. Quand son nom a la forme d'un nom de staging, son registre a été lu pour dire ce qu'est le répertoire, et le message le dit. |
| Une installation préparée donnée en `--config-dir` | `layout.prepared_passed_as_config_dir` | La passer en `--installation-dir`. |
| Un répertoire historique donné en `--installation-dir` | `layout.historical_passed_as_installation_dir` | Le passer en `--config-dir`. |
| Les deux dispositions dans un même répertoire | `layout.mixed` | Préserver le répertoire. Ne pas l'initialiser de nouveau ; employer un répertoire distinct pour les commandes historiques. |
| Une installation publiée sans son marqueur de complétion | `layout.completion_marker_absent` | Arrêter les contrôles, puis lancer `agentsandbox setup resume --recovery-dir DIR --confirm-control-stopped`. |
| Un répertoire de staging conservé | `layout.setup_staging_directory` | Arrêter les contrôles, puis lancer le même `setup resume` sur ce répertoire. Ne pas régénérer de credentials. |
| Un répertoire de staging et son chemin final enregistré existent tous deux | `layout.staging_and_final_both_exist` | Préserver les deux et ne pas reprendre. Établir d'abord lequel détient l'installation. |
| Un nom de staging sans enregistrement de préparation | `layout.staging_without_intent` | Le préserver et préparer dans une autre destination neuve. |
| Les fichiers enregistrent des chemins hors du répertoire inspecté | `layout.references_outside_inspected_root` | Le gateway lit les chemins enregistrés. Lancer l'outil là où l'installation est montée à son chemin enregistré. |

Un répertoire de staging est reconnu à son nom, `.setup-` suivi de 24 chiffres hexadécimaux, quand il ne contient pas encore de registre ou que son registre enregistre des chemins hors de lui. Son registre et son fichier OIDC sont décodés et jugés, mais ils enregistrent les chemins finaux : les fichiers de clés et les bases qu'ils nomment ne sont pas décrits. Ils sont connus malgré cela : une option de fichier ou une référence d'AC qui nomme l'un d'eux n'est pas ouverte, à une exception près que laisse l'ordre des lectures. Un fichier de profils est lu avant le fichier OIDC : quand `--providers-file` nomme l'audit d'authentification qu'enregistre le fichier OIDC, cet audit est lu comme un fichier de profils s'il fait au plus 65536 octets, la borne du chargeur de profils, et le rapport le dit alors par `reference.file_read_before_it_was_named`. Rien d'autre au chemin final n'est connu : un fichier qui s'y trouve et que ni le registre ni le fichier OIDC du répertoire de staging ne nomment, tel qu'un enregistrement de setup du répertoire final, n'est pas reconnu. L'outil ne dérive jamais le nom d'un répertoire de staging et n'en recherche jamais.

Un répertoire qui n'est pas privé, qui est un lien symbolique ou qui n'est pas un répertoire est rapporté comme `layout.posture_refused`. Son message dit lequel : un chemin qui n'est pas un répertoire, un mode autre que 0700, ou, quand le mode est conforme, un répertoire au-dessus de lui, un lien dans son chemin ou son système de fichiers. Le mode et le propriétaire observés ne sont pas imprimés comme faits de la disposition, et les trois causes du second cas ne sont pas distinguées : `ls -ld` sur le répertoire et sur chaque répertoire au-dessus de lui montre les deux premières. Sous macOS le produit a une cause de plus, que le message nomme : une entrée d'accès étendue sur le répertoire lui-même, ou une entrée qui permet une modification sur un répertoire au-dessus de lui. Cela est lu dans le code du produit : aucun lancement n'a eu lieu sous macOS. Ni cet outil ni le produit ne réparent un chemin existant.

## Hygiène des secrets

### Fichiers lus pour leur contenu

`mode.files_read_for_content` est la liste fermée de ce qu'un lancement a ouvert. Rien d'autre n'est lu. Lire un fichier peut mettre à jour sa date d'accès, comme toute lecture ; rien d'autre ne change.

| Rôle | Fichier | Lu comme |
|---|---|---|
| `tenant_registry` | `tenants.json` d'une installation, ou `--tenants-file` | Comme le gateway le lit : un fichier régulier d'au plus 1 Mio, pas un lien symbolique. Ni propriétaire, ni mode, ni nombre de liens ne sont contrôlés. Un fichier qui n'est pas lu est distingué par ses métadonnées : absent, d'un autre compte, ou refusé par cette règle. |
| `oidc_file` | `oidc.json` d'une installation, ou `--oidc-config` | Un fichier privé d'au plus 1 Mio : mode 0600, lien unique, appartenant au compte de contrôle. |
| `oidc_ca_bundle` | Le `ca_bundle_file` du fichier OIDC | Un fichier privé, puis la grammaire de bundle du produit. |
| `provider_profiles` | `providers.json`, ou `--providers-file` | Un fichier privé d'au plus 65536 octets, par le chargeur du produit. |
| `decision_engine_ca_bundle` | `--jev-ca-file` | Un fichier régulier à lien unique que les autres comptes ne peuvent pas écrire, d'au plus 65536 octets. |
| `gateway_ca_bundle` | `--gateway-ca-file` | Idem. |
| `engine_program` | Le programme du moteur | Haché par l'approbation. Il est listé dès qu'une approbation a porté sur un programme existant, même si un contrôle ultérieur l'a refusé. |

Les fichiers sont ouverts dans un ordre fixe, et chacun est comparé, avant son ouverture, à tous les fichiers de clés, credentials, enregistrements de setup et bases connus à ce moment. Les fichiers aux noms fixes des répertoires inspectés viennent d'abord : `tenants.json` d'un répertoire d'installation, puis `providers.json` d'un répertoire de configuration. Les fichiers donnés par option suivent : `--tenants-file`, `--providers-file`, le fichier OIDC, les bundles, puis le programme du moteur. Le `tenants.json` d'un répertoire d'installation qui porte un nom de staging est lu pour dire ce qu'est le répertoire : il est listé comme lu même quand rien de ce répertoire n'est décrit.

### Fichiers jamais ouverts

Les fichiers de clés, de jetons, les credentials fournisseur, les enregistrements de setup (`secret-material.json`, `preparation.json`, `completion.json`, `authentication-creation.json`, `authentication-origin.json`) et chaque base avec ses sidecars sont décrits par leurs métadonnées : présence, type, mode, propriétaire, nombre de liens. `content` vaut `not_read`, sauf dans le seul cas que le rapport nomme lui-même : `read_before_named`, pour un fichier signalé par `reference.file_read_before_it_was_named`. Aucun fichier `.env`, cache OAuth ni keychain n'est lu. Les cinq enregistrements de setup d'une installation préparée sont listés avec le rôle `setup_material` ; `authentication-creation.json` n'existe que pour une préparation de version 2 et n'est pas listé quand il est absent.

Les lecteurs de fichiers privés jugent aussi les répertoires qui mènent à un fichier. L'outil demande ce verdict au lecteur lui-même, sans réécrire sa règle : le lecteur parcourt ces répertoires et il est arrêté avant d'y ouvrir quoi que ce soit. Des répertoires sont ouverts pour cette question, jamais le fichier. Un refus est signalé par `provider.credential_unsafe`, `provider.unselected_profile_reference_unsafe` ou `reference.private_rule_violated`, avec un message qui nomme le chemin. Pour un fichier conforme sous un répertoire que le lecteur accepte et qui n'est pas un répertoire privé 0700, l'outil ne vérifie ni le système de fichiers ni les entrées d'accès étendues du fichier lui-même : `reference.private_rule_not_evaluated` le dit, et seule la commande consommatrice les contrôle en ouvrant le fichier. Sur macOS, la marche du lecteur refuse aussi un répertoire dont les entrées d'accès étendues permettent une modification : le message nomme cette cause, qui est lue dans le code du produit et n'a pas été exécutée, tous les lancements de l'outil ayant eu lieu sous Linux.

Tout répertoire inspecté réserve `secrets`, `mounted-secrets`, `tenants`, son audit d'authentification et ses enregistrements de setup, quelle que soit l'option qui a donné le répertoire et quoi qu'il contienne en définitive. Une option de fichier ou une référence d'AC qui pointe là n'est jamais ouverte, et elle est rapportée à la place. `tenants.json` et `oidc.json` d'un répertoire d'installation sont comparés de la même façon avant leur ouverture sous leurs noms fixes :

| Option ou référence | Constat |
|---|---|
| `--tenants-file`, ou `tenants.json` d'un répertoire d'installation | `registry.file_unreadable` |
| `--oidc-config`, ou `oidc.json` d'un répertoire d'installation | `oidc.file_unavailable_or_not_private` |
| `--providers-file` | `provider.file_unavailable_or_not_private` |
| Une référence d'AC ou une option d'AC | `trust.ca_reference_is_a_credential_reference` |
| `--runtime-program` | `engine.program_is_a_credential_reference` |

Conséquence : un registre, un fichier OIDC, un fichier de profils, un bundle ou un programme du moteur conservé sous `secrets`, `mounted-secrets` ou `tenants` d'un répertoire inspecté n'est pas lu. Un second nom d'un fichier de clé, d'un enregistrement de setup ou d'une base que nomment les fichiers inspectés, tel qu'un lien dur, est reconnu lui aussi. Un chemin est comparé tel qu'il est écrit et tel qu'il se résout : un lien symbolique, ou un répertoire lié sur le trajet, qui mène à un tel fichier est reconnu. Une référence qui est elle-même un lien symbolique nomme aussi sa cible : son lecteur refuse le lien, et une option qui nomme directement la cible n'est pas ouverte. Résoudre un chemin n'ouvre aucun fichier et ne liste aucun répertoire.

Un fichier ainsi retenu est rapporté comme un `error` et le code de sortie est 3, quoi que la commande consommatrice en ferait : c'est un refus de cet outil, pas de cette commande. Pour qu'un tel fichier soit lu, le conserver hors de ces répertoires, ou lancer l'outil sans l'option de répertoire qui les réserve. Sans cette option, seuls les fichiers que nomment les fichiers inspectés restent protégés : les enregistrements de setup de ce répertoire, et tout autre fichier sous ces trois répertoires, sont inconnus du lancement.

Ce que cette protection ne couvre pas :

- Un fichier donné par option qui se trouve hors de tout répertoire inspecté, et qu'aucun fichier lu avant lui ne nomme, est ouvert comme ce que l'option dit qu'il est. Donner à chaque option le fichier qu'elle décrit. Le contenu n'est jamais imprimé : un fichier invalide donne une catégorie d'erreur, une ligne et une colonne.
- L'ordre des lectures ne protège qu'un sens. `--tenants-file` est lu avant `--providers-file` : une option de registre qui nomme un credential d'un fichier de profils donné par option est lue comme un registre. `providers.json` d'un répertoire de configuration est lu avant `--tenants-file` : si le registre nommait ce fichier lui-même comme fichier de clé, il aurait déjà été lu comme fichier de profils. Un fichier de profils est lu avant le fichier OIDC : une option de profils qui nomme l'audit d'authentification qu'enregistre ce fichier OIDC, à un chemin qu'aucun répertoire inspecté ne réserve, est lue comme un fichier de profils. Quand un fichier lu plus tard nomme un fichier déjà ouvert, le rapport le dit : `mode.credential_contents_read` vaut `detected_after_read`, la ligne `mode` du texte change, et l'erreur `reference.file_read_before_it_was_named` donne le rôle du fichier qui a été ouvert. Là où le rapport liste ce fichier, dans `credential_references` ou dans `data`, son entrée porte `content` `read_before_named` ; un fichier qui n'est pas décrit, tel qu'un fichier enregistré par un répertoire de staging, n'a pas d'entrée. Les deux constats sur le contenu, `reference.content_not_checked` et `provider.credential_content_not_checked`, nomment alors cette erreur au lieu de dire qu'aucun fichier de ce genre n'a été ouvert. `none` signifie qu'aucun fichier ouvert n'est nommé comme fichier de clé, credential, enregistrement de setup ou base par les fichiers inspectés dans ce lancement. Un credential qu'aucun fichier inspecté ne nomme ne peut pas être reconnu.
- `tenants.json` et `oidc.json` d'un répertoire d'installation sont ouverts par leurs noms fixes après cette comparaison. Un second nom d'un enregistrement de setup, de l'audit d'authentification ou d'un fichier de clé que nomme un fichier antérieur n'est pas ouvert. Un second nom d'un fichier de clé sous `tenants` n'est pas reconnu quand il tient lieu de `tenants.json` : seul le registre nomme ces fichiers, et l'outil ne liste aucun répertoire. `oidc.json` est lu par la règle du fichier privé, qui refuse un fichier ayant un second nom.
- Une référence d'AC qui pointe vers un fichier privé que l'outil ne peut pas reconnaître comme une clé, un enregistrement ou une base est lue comme le gateway la lirait. Son contenu n'est jamais imprimé.
- Un registre ou un fichier OIDC ne nomme rien quand il n'est pas lu ou pas décodé : les fichiers de clés, les bases et l'audit qu'il aurait nommés ne sont alors connus que par les noms réservés d'un répertoire inspecté.
- La comparaison avec les fichiers réservés et l'ouverture sont deux résolutions de chemin distinctes. Une modification faite par le même compte, ou par root, entre les deux (fichier renommé, répertoire lié remplacé) est hors de cette garantie, comme pour le produit. Quand le chemin ouvert mène encore à un fichier réservé à la fin du lancement, le rapport le dit par `reference.file_read_before_it_was_named` ; quand la modification a été annulée, il ne le dit pas. Cela découle du code : aucun lancement n'a exercé une telle modification.
- Un répertoire réservé est reconnu par son nom, tel qu'il est écrit et tel qu'il se résout, et un fichier nommé par son chemin et par son périphérique et son inode. Sur un volume qui ignore la casse, il n'a pas été vérifié qu'un fichier qu'aucun fichier inspecté ne nomme, conservé sous `secrets`, `mounted-secrets` ou `tenants` d'un répertoire inspecté et donné sous une autre casse de ce nom de répertoire, est refusé : aucun lancement n'a eu lieu sur un tel volume.

### Chemins, identifiants et URL dans la sortie

- Sans `--show-paths`, un chemin sous un répertoire inspecté est montré relativement à un emplacement symbolique : `<config>`, `<installation>`, `<tenants-file-dir>` ou `<oidc-config-dir>`. Un fichier donné par option est montré comme `<tenants-file>`, `<oidc-config>`, `<providers-file>`, `<jev-ca-file>`, `<gateway-ca-file>` ou `<runtime-program>`. La racine du système de fichiers n'est jamais un tel répertoire : quand `--config-dir` ou `--installation-dir` vaut `/`, ou qu'un registre ou un fichier OIDC donné seul se trouve directement sous `/`, aucun chemin n'est montré relativement à elle. Un chemin est alors montré relativement à un autre répertoire inspecté qui le contient, ou sinon par son rôle et un ordinal ; le fichier de profils à son nom fixe dans un répertoire de configuration qui vaut `/` est montré comme `[path not shown]`, ce qui est lu dans le code, aucun lancement n'ayant atteint ce cas.
- Tout autre chemin est montré par son rôle et un ordinal, jamais par un nom de fichier, par exemple `runner key reference #1 (outside the inspected directory)`. Un credential fournisseur est montré comme `provider reference #N`, avec son type de source et sa position dans ou hors du répertoire du fichier de profils. N est le rang de son profil parmi les identifiants triés.
- `--show-paths` imprime les chemins absolus. Ne partager un tel rapport que là où ces chemins peuvent l'être.
- Un rapport par défaut masque les chemins, pas les noms. Il imprime encore les identifiants de tenants, les paramètres publics d'un fichier OIDC (émetteur, URL des endpoints, identifiants de clients, audience, libellé du fournisseur, origine web, redirection web, chemin de callback de la CLI et révision du mapping des claims), les identifiants de profils, les modèles et les noms de variables de credentials, ainsi qu'une URL de moteur de décision donnée en ligne de commande : ne le partager que là où ces éléments peuvent l'être.
- Une URL est reconstruite depuis sa valeur analysée : schéma, hôte, port et chemin. Informations d'utilisateur, requête et fragment ne sont jamais imprimés. Une URL qui ne peut pas être analysée est montrée comme `[unparseable URL]`.
- Les caractères de contrôle et les contrôles bidirectionnels sont échappés, de sorte qu'aucune valeur ne peut forger une ligne. Le sont aussi les caractères invisibles par défaut (caractères de largeur nulle, trait d'union conditionnel, gluon de mots et opérateurs invisibles, caractères de remplissage, sélecteurs de variante, caractères d'étiquette) et toute espace autre que l'espace ASCII : deux valeurs qui ne diffèrent que par un tel caractère sont montrées différemment. Un libellé qui contient légitimement un tel caractère, par exemple une espace insécable, un liant sans chasse ou un sélecteur de variante, est lui aussi montré échappé. Un caractère échappé s'écrit `\u{…}` avec son point de code hexadécimal, et une barre oblique inverse est doublée. Une valeur est bornée à 512 octets, un chemin à 1024, avec une marque de troncature.
- Une valeur que le filtre de sortie du produit réécrirait est remplacée entière : `[masked value]`, `[masked path]`, ou un `[masked tenant identifier N]` ou `[masked profile identifier N]` numéroté. Le rapport porte alors l'avertissement `output.value_masked`. Un chemin qui serait réécrit reste masqué avec `--show-paths` aussi.
- Le texte d'un chargeur, d'un analyseur ou du système de fichiers n'est jamais recopié. Un refus est rapporté par un motif fermé, et un fichier JSON invalide par sa catégorie d'erreur et sa position.

### La découverte active

`--active oidc-discovery` est la seule opération réseau de l'outil. Elle appelle la découverte du client OIDC du produit lui-même, construit depuis le fichier OIDC exactement comme le gateway le construit, et ne change aucune de ses bornes :

- **Deux requêtes GET, au plus.** Le document de découverte, à `/.well-known/openid-configuration` sous l'émetteur, puis le `jwks_uri` du fichier. Les clés ne sont pas demandées quand le document de découverte ne confirme pas le fichier. Aucune autre URL n'est contactée : un endpoint annoncé par le fournisseur est comparé au fichier, jamais adopté.
- **Le fichier choisit les hôtes.** Les deux URL viennent du fichier OIDC inspecté. Pour un fichier au transport `https_only`, aucun filtre d'adresse ne leur est appliqué : un hôte d'un réseau privé est contacté comme un autre. Un fichier qui déclare le transport HTTP de démonstration ne passe les règles hors ligne que si son émetteur est une adresse loopback littérale ou l'émetteur de démonstration fixe `http://keycloak.ai-guard.test:18080/realms/ai-guard-demo`, avec ses endpoints sous cet émetteur. Rien n'est imprimé avant le contact. Lancer d'abord l'outil sans `--active`, lire `issuer` et `keys endpoint` dans le bloc d'identité, `issuer` et `jwks_uri` dans le document JSON, puis n'ajouter `--active oidc-discovery` que pour un fournisseur que l'on entend contacter depuis cet hôte.
- **Aucun credential.** Ni jeton, ni clé, ni secret de client, ni cookie n'est envoyé. L'endpoint de jetons n'est jamais contacté.
- **Bornée.** Ni proxy, ni redirection suivie, ni nouvelle tentative. Le délai et la borne de taille sont les `limits` du bloc fournisseur du fichier : 5000 millisecondes et 131072 octets quand le fichier n'en fixe pas.
- **TLS inchangé.** Les certificats sont vérifiés contre le bundle d'AC du fichier OIDC quand il en déclare un, contre les racines publiques sinon. Le HTTP en clair n'est employé que pour un fichier qui déclare le transport de démonstration isolé. Aucune option n'assouplit cela.
- **Rien de la réponse n'est imprimé.** Le client du produit rend un résultat fermé et rien d'autre : ni ligne de statut, ni en-tête, ni corps, ni texte de transport n'atteint cet outil, aucun ne peut donc atteindre un rapport. Le rapport porte un code fermé.
- **Rien n'est écrit.** La vérification ne conserve rien et ne change rien dans l'installation.

Un `ok` dit que ce fournisseur a répondu à ce client depuis cet hôte à cet instant. Il ne prouve ni une connexion d'utilisateur, ni que le gateway joint le fournisseur depuis son propre réseau, et il ne qualifie aucun fournisseur d'identité.

Ce qui a été exécuté : la vérification a tourné contre un émetteur contrôlé sur le loopback d'un conteneur de test, avec le seul transport HTTP de démonstration. Un lancement que l'émetteur a confirmé a envoyé deux requêtes en HTTP/1.1, le document de découverte puis les clés, avec les noms d'en-têtes `accept` et `host` et sans corps ; les valeurs des en-têtes n'ont pas été enregistrées. Non exécuté : une requête HTTPS, avec ou sans bundle d'AC, tout fournisseur d'identité réel, et les résultats `active.oidc_discovery_provider_rejected` et `active.oidc_discovery_keys_invalid`. Que le client n'emploie aucun proxy, ne suive aucune redirection et ne réessaie pas est lu dans le client du produit, pas observé.

### Lectures hôte du contrôle du moteur

`--runtime` lance l'approbation du produit, qui n'est pas exempte de lectures sur l'hôte : elle demande à la base des mots de passe le home du compte de contrôle, regarde `/run/user/<uid>` et les répertoires système du chemin des programmes auxiliaires, et hache le programme, jusqu'à 512 Mio. Cela peut prendre des secondes. Seuls quatre faits atteignent le rapport : le type de moteur, l'affichage de l'entrée approuvée, son SHA-256 et la version de la politique. L'environnement, le répertoire de travail, le home du compte et le chemin canonique restent en dehors.

## Limites et zones non qualifiées

- **Des fichiers, pas un système en cours d'exécution.** Le rapport ne dit rien des services en cours d'exécution, de l'état du moteur, de l'image worker ni du contenu d'une base.
- **La moitié du chargeur du gateway.** Le `ok` d'un registre ou d'un fichier OIDC couvre les règles structurelles reformulées dans cet outil et les validateurs du produit qu'il appelle. Il est toujours accompagné de `registry.loader_acceptance_not_checked` ou de `oidc.loader_acceptance_not_checked` : seul le gateway lit les clés, ouvre les bases et compare l'état durable. Les règles reformulées sont comparées aux chargeurs par des tests de ce dépôt, sur leurs noms de champs et sur un corpus de fichiers acceptés et refusés : un défaut, un alias ou un changement de type dans un chargeur qu'aucun cas n'exerce n'est pas détecté.
- **Une politique d'authentification, avant la première initialisation.** `oidc.authentication_policy_invalid` couvre deux contrôles du produit. Le gateway applique le second, que la politique finance le plan de capacité d'un fichier de version 2, seulement après avoir ouvert ou créé l'audit d'authentification. Comme observé par la fonction du chargeur du gateway, un gateway à qui l'on demande d'initialiser l'audit d'un tel fichier refuse après avoir créé le fichier d'audit, et laisse ce fichier en place ; le rapport ne parle pas de ce fichier. Cela n'a pas été exécuté par le binaire du gateway démarré avec `--initialize-oidc-audit`. L'outil signale cette politique hors ligne : inspecter un fichier OIDC neuf avant le premier démarrage du gateway avec cette option.
- **Les motifs fermés dépendent des textes du produit.** Un motif est reconnu en comparant le refus d'un validateur du produit à des textes connus. Un refus non reconnu est rapporté avec le motif `unclassified` ; le verdict lui-même reste exact. Deux refus du moteur gardent le motif `unclassified` par conception, une entrée d'accès étendue et un `/run/user/<uid>` refusé : aucun motif fermé ne les nomme, et chacun a son message et son action.
- **Une sélection, pas un lancement.** Le profil que sélectionne un lancement géré est le choix du produit, pour les options de sélection données à cet outil. Rien d'autre d'un lancement n'est établi : le credential du profil sélectionné n'est pas ouvert, et les règles que `run` applique ensuite, comme `--profile observe` avec `--network bridge` et la place d'un credential dans le projet, ne sont pas appliquées. Un credential manquant ou non sûr n'est une erreur que pour le profil que la sélection résout.
- **Aucun verdict de transport sans bundle utilisable.** Quand le bundle donné par `--jev-ca-file` est refusé ou illisible, le transport n'est jugé ni valide ni invalide, sauf si l'URL est refusée même sans bundle. Le rapport liste alors `inference.decision_transport_not_checked` parmi les éléments non vérifiés.
- **Aucune date de validité.** Un certificat expiré n'est pas détecté.
- **Les fichiers de clés par leurs métadonnées.** Un fichier de clé vide ou trop grand, et un même fichier référencé pour deux identités, sont des refus certains du gateway ou des commandes historiques que cet outil ne détecte pas : ils ne sont couverts que par `reference.content_not_checked` et `registry.loader_acceptance_not_checked`.
- **Un seul compte, des systèmes de fichiers locaux.** Les règles de posture refusent un autre propriétaire et un système de fichiers non pris en charge. C'est rapporté comme `unknown` ou `error`, jamais comme un résultat vide.
- **Volumes de conteneur.** Une installation dans un volume nommé ne peut être inspectée que là où l'outil existe : dans un conteneur d'une image de contrôle dont le Dockerfile compile et copie l'outil, comme le décrit le Quick Start. Ce parcours dépend de votre construction d'image et de votre moteur ; ses commandes n'ont pas été exécutées, et ce guide ne qualifie ni l'un ni l'autre.
- **Une vérification active, sur demande.** `--active oidc-discovery` est la seule opération qui ouvre une connexion. Sans elle, `mode.active_checks_executed` est vide et `mode.network` vaut `none`. Ni moteur, ni moteur de décision, ni fournisseur IA n'est jamais contacté : rien dans ce build ne les vérifie activement.
- **Plateformes.** La politique de posture est celle du produit, pour les systèmes de fichiers locaux Linux et macOS. Ce guide ne qualifie aucune plateforme, aucun moteur, aucun fournisseur d'identité et aucun fournisseur IA.
- **Sortie en anglais.** Les messages et les actions sont en anglais dans les deux guides.

### Ce qui n'a pas été exécuté

Les énoncés de ce guide reposent sur des lancements de l'outil là où un lancement était possible, et sur le code ailleurs. Ce qui suit n'a pas été exécuté :

- **Plateformes.** Tous les lancements de l'outil ont eu lieu sous Linux, sur ARM64, dans un conteneur sans réseau. Rien n'a été lancé sous macOS : les branches macOS de la politique de posture, entrées d'accès étendues comprises, sont seulement lues dans le code du produit.
- **Le build release.** `cargo build --release --locked`, la première commande de ce guide, n'a pas été lancée : l'outil a été construit et lancé avec le profil debug, hors ligne.
- **Volumes insensibles à la casse.** Aucun n'a été utilisé : voir le dernier élément de « Ce que cette protection ne couvre pas ».
- **Un second compte.** Un registre que le compte qui inspecte ne peut pas lire, et une base ou un répertoire de base qui appartient à un autre compte, n'ont pas été construits. Leurs verdicts, `reference.inspector_identity_differs` et `reference.metadata_unavailable`, reposent sur le classement des métadonnées de fichier, qu'un test unitaire couvre pour le registre seulement.
- **Bases.** L'axe des données a été exercé par l'outil, avec le gateway comme référence, pour une base sans sidecar, les sidecars `-wal` et `-shm` d'un gateway qui tourne ou qui a été arrêté sans fermeture propre, une base de tenant absente, un sidecar sans sa base, une base lisible par d'autres comptes, un répertoire de base qui n'est pas privé, un chemin de base avec traversée, un nom de base réservé ou non ASCII, et un audit d'authentification absent. Non exécuté : un sidecar `-journal`, et, pour l'audit d'authentification, tout refus autre que son absence. L'outil a été lancé une fois pendant qu'un processus gateway tenait les bases de l'installation : il est sorti avec 0 et a rapporté `data.sidecars_present`, et le processus du gateway tournait encore ensuite. Il n'a pas été vérifié que le gateway servait des requêtes pendant ce lancement.
- **Reprise.** Ce que fait `setup resume` sur une installation publiée dont l'audit d'authentification est absent n'a pas été exécuté : le message de `data.authentication_audit_missing_from_prepared_installation` énonce seulement le refus du gateway.
- **Moteurs.** Aucun moteur n'était présent aux emplacements système fixes : `--runtime` sans `--runtime-program` n'a été lancé que jusqu'à `engine.program_absent_from_fixed_entries`, et l'approbation a été exercée sur un programme marqueur donné par `--runtime-program`.
- **Fournisseurs et clients.** Aucun fournisseur IA, aucun modèle, ni Codex CLI ni Claude Code n'a été contacté ou lancé.

## Contrat JSON, version 1

`--json` écrit un seul document sur stdout, d'au plus 524288 octets. Son `schema` vaut `ai-guard-diagnose-report` et sa `version` vaut `1`.

```json
  "mode": {
    "network": "none",
    "mutation": "none",
    "credential_contents_read": "none",
    "files_read_for_content": [
      "provider_profiles"
    ],
    "active_checks_executed": []
  },
```

### Règle de compatibilité

Chaque champ est toujours présent : une section ou une valeur qui n'a pas été inspectée vaut `null`, jamais un mot de statut.

| Changement | Dans la version 1 |
|---|---|
| Ajouter un champ, un code, un type de `details` ou une valeur d'une liste fermée | Permis. Un consommateur ignore un champ qu'il ne connaît pas et traite un code inconnu par son `status`. |
| Retirer ou renommer un champ ou un code ; changer le statut, l'axe ou la base d'un code ; changer le sens d'un code de sortie | Interdit : cela change `version`. |
| Retirer un code du service | Le code n'est jamais réemployé. |

La première baseline publique de version 1 est le gel corrigé de l’intégration composée : ses champs, codes et valeurs fermées sont listés ici. Les axes existants sont conservés, dont `trust.decision_transport_invalid` sur `trust`. Les identifiants `provider.selection_resolution_not_available` et `identity.idp_reachability_not_available` ont été supprimés avant toute publication ; ils restent réservés et ne seront jamais réutilisés. Cet historique de prépublication n’implique aucune ancienne version 1 déployée.

Les types Rust de ce dépôt (`diagnostics::report::ReportV1`) ne décodent que les rapports de leur propre build : ils refusent un code ou une valeur fermée qu'ils ne connaissent pas, et ils abandonnent un champ qu'ils ne connaissent pas. Un consommateur du document JSON applique lui-même la règle ci-dessus. Aucun test ne décode un rapport d'un autre build.

### Document

| Champ | Contenu |
|---|---|
| `schema`, `version` | L'identité du contrat. |
| `tool` | `name` et `package_version`. |
| `mode` | `network`, `none` ou `active_checks_only` ; `mutation`, toujours `none` ; `credential_contents_read`, `none` ou `detected_after_read` ; `files_read_for_content` ; `active_checks_executed`, les vérifications actives qui se sont exécutées, quel que soit leur résultat : `oidc_discovery`. |
| `summary` | `status` (`ok`, `error` ou `unknown`), `exit_code`, `counts` par statut (`ok`, `info`, `warning`, `error`, `unknown`, `not_checked`, `not_available`), `truncated`, et `omitted` : `reference_entries`, `data_entries`, `tenant_ids`. |
| `inputs` | Une entrée par entrée : `name`, `origin` (`cli` ou `default`) et `display`. Les options de sélection sont les entrées `client`, `auth_mode`, `client_profile` et `model`, listées quand elles sont données. |
| `layouts` | Une entrée par répertoire ou registre inspecté : `input`, `kind`, `preparation`, `oidc_file_version`. |
| `topology` | `tenant_model` (`single_local` ou `registry`) et `tenant_count` ; `null` quand rien ne la décrit. |
| `identity` | `technical_key_references` comptées en `present`, `absent` et `unknown` ; `sso`, `null` quand aucun fichier OIDC n'a été décodé. |
| `tenants` | `registry` et `plan`, listes d'identifiants ; `registry_total` et `plan_total`, le nombre d'identifiants distincts que déclare chaque fichier ; `correspondence` (`match`, `mismatch` ou `null`). |
| `credential_references` | Une entrée par fichier référencé. |
| `providers` | `file` avec son `state` (`absent`, `refused`, `invalid` ou `loaded`) et `display` ; `default_profile` ; `auto_priority` ; `profiles` ; `selection`, `null` quand aucun fichier de profils n'a été chargé. |
| `inference` | `decision_engine_transport` : `kind` (`remote` ou `local`), `url`, `allow_local_http`, `ca_bundle_given` ; `null` quand aucune URL n'a été donnée. |
| `trust` | Une entrée par bundle d'AC : `role`, `display`, `read`, `certificate_count`. |
| `engine` | `kind`, `entry_display`, `sha256`, `policy_version` ; `null` sauf si un programme a été approuvé. |
| `data` | Une entrée par base. |
| `findings` | Les constats, ordonnés par axe, code et portée. |
| `limits` | Les limites du build, de la même forme qu'un constat. |

Un identifiant est un objet : `id` et `masked`. `masked` vaut `true` quand un emplacement numéroté tient lieu de l'identifiant.

`sso` porte les paramètres publics du fichier OIDC : `file_version`, `issuer`, `transport` (`https_only` ou `isolated_demo_http`), `provider_label`, `web_origin`, `web_client_id`, `cli_client_id`, `api_audience`, `authorization_endpoint`, `token_endpoint`, `jwks_uri`, `end_session_endpoint`, `web_redirect_uri`, `cli_callback_path`, `mapping` (`revision` et `rule_count`), `ca_bundle_declared`, et `idp_contacted`, `true` quand la découverte active s'est exécutée contre le fournisseur, quel que soit son résultat.

Un profil de `providers` porte `profile`, `revision`, `client` (`codex` ou `claude_code`), `auth_mode` (`api` ou `subscription`), `model`, `source_kind` (`api_env_file`, `codex_auth_file` ou `claude_oauth_env_file`) et `variable_name`. Sa référence est l'entrée de `credential_references` dont le `owner` est ce profil.

`selection` porte `requested` et `resolved`. `requested` dit comment chaque partie de la sélection a été demandée : `client`, `auth_mode`, `profile` et `model` valent chacun `explicit` ou `auto` ; les valeurs des options sont dans `inputs`. `resolved` vaut `null` quand le produit refuse la sélection. Sinon il porte `profile`, `revision`, `client`, `auth_mode`, `model` (le modèle demandé, sinon celui du profil, sinon `null`) et `chosen_by` : `profile` quand le profil a été demandé par son nom, `default_profile`, ou `auto`.

### Entrées de référence et de données

| Champ | Valeurs |
|---|---|
| `owner` | `kind` (`installation`, `tenant` ou `provider_profile`), `id`, `masked`. |
| `role` | Référence : `runner`, `viewer`, `decision_engine`, `projection`, `provider`, `setup_material`. Données : `tenant_store`, `authentication_audit`. |
| `consumer` | Le lecteur dont la règle s'applique : `gateway_token_reader`, `private_file_reader`, `compose_projection`, `setup_recovery`. |
| `location` | `inside_root` ou `outside_root`. |
| `display` | Le chemin, tel que le montrent les règles de sortie. |
| `presence` | `present`, `absent` ou `unknown`. |
| `entry_kind` | `regular_file`, `directory`, `symbolic_link`, `other`, ou `null`. |
| `mode` | Les bits de permission en quatre chiffres octaux, ou `null`. |
| `owner_is_inspector`, `single_link` | `true`, `false`, ou `null` quand l'entrée n'a pas été trouvée. |
| `private_rule` | `satisfied`, `violated` ou `not_evaluated`. `violated` couvre aussi un fichier dont le lecteur refuse un répertoire qui y mène. `not_evaluated` dépend du lecteur. Le lecteur de jetons du gateway ne contrôle ni propriétaire, ni mode, ni nombre de liens. Pour le lecteur de fichiers privés, le fichier est conforme et son répertoire n'est pas un répertoire privé 0700, donc le contrôle d'entrée du produit n'a pas tourné : `reference.private_rule_not_evaluated` dit ce qui reste non vérifié. Pour une projection et pour un enregistrement de setup, cet outil n'applique aucune règle de fichier privé. |
| `sidecars` | Données seulement : `wal`, `shm`, `journal`. |
| `content` | `not_read`, ou `read_before_named` pour un fichier signalé par `reference.file_read_before_it_was_named` : il a été ouvert comme une autre entrée avant qu'un fichier lu après lui ne le nomme. Pour une base, aussi quand l'un de ses sidecars l'a été. |

Les listes sont bornées dans ce qu'elles impriment, jamais dans ce qu'elles évaluent : au plus 512 entrées et 262144 octets pour chacune de `credential_references` et `data`, 256 identifiants par liste de tenants, 16 par sujet. Les entrées liées à une erreur, un inconnu ou un avertissement viennent en premier. Ce qui n'est pas imprimé est compté dans `omitted`, et chaque total reste exact ; le rapport texte porte alors sa ligne `omitted`. `truncated` à `false` ne signifie pas que chaque entrée est listée : lire `omitted`. Quand le document dépasse encore sa borne, les listes de détail sont vidées, `truncated` vaut `true` et le constat `output.detail_omitted` est ajouté.

### Constats

```json
      "details": {
        "kind": "would_refuse_with",
        "class": "login_required"
      }
```

| Champ | Contenu |
|---|---|
| `code`, `axis`, `status`, `basis` | Comme dans le catalogue ci-dessous. |
| `scope` | `kind`, avec `input` ou `file` quand le type en nomme un. |
| `origin` | `sources`, chacune un `file` et un `field` ; `consumer`, la commande qui applique la règle ; `assumes`, les entrées supposées. |
| `message`, `action` | Phrases en anglais du catalogue. Une action nomme votre propre entrée par son affichage. |
| `requires` | Une phrase pour `not_checked`, `null` sinon. |
| `subjects` | `kind` (`tenants` ou `provider_profiles`), `total`, `ids`, `omitted` ; ou `null`. |
| `details` | Une forme fermée choisie par `kind`, ou `null`. |

| `kind` de `details` | Champs |
|---|---|
| `tenant_set_difference` | `only_in_registry`, `only_in_plan`, `omitted`. |
| `json_error` | `category` (`syntax`, `data`, `eof` ou `io`), `line`, `column`. |
| `reason` | `reason`, l'un des motifs fermés ci-dessous. |
| `policy` | `policy` : `worker_quota`, `kubernetes_held_create`, `kubernetes_session_reservation`, `kubernetes_session_recovery`, `kubernetes_session_control_recovery`, `audit_capacity`, `workload_audit`, `relay_retention`. |
| `would_refuse_with` | `class` : `login_required` ou `credential_missing`. |
| `sidecars` | `sidecars`. |
| `engine_program` | `explicit_program` : `true` pour un programme donné par option, `false` pour les emplacements fixes. |

| `reason` | Rapporté par |
|---|---|
| `version_or_token_bound`, `web_origin_or_callback`, `claim_mapping_invalid`, `claim_mapping_ambiguous`, `url_unparseable`, `provider_block_invalid` | `oidc.gateway_block_invalid` |
| `duplicate_or_invalid_profile`, `unknown_or_duplicate_priority`, `default_profile_missing` | `provider.configuration_ambiguous` |
| `json_invalid`, `version_or_size`, `profile_identifier`, `model_identifier`, `source_for_mode`, `variable_for_client`, `path_not_absolute`, `path_traversal` | `provider.configuration_invalid` |
| `explicit_conflict`, `several_match_without_priority` | `provider.selection_ambiguous` |
| `requested_profile_not_configured`, `no_profile_matches` | `provider.selection_no_profile` |
| `profile_identifier`, `model_identifier`, pour une valeur donnée par option | `provider.selection_invalid` |
| `account_home_unavailable`, `helper_directories_refused`, `program_or_directory_refused` | `engine.program_refused` |
| `unclassified` | L'un de ces codes, pour un refus dont le texte n'est pas reconnu. |

`engine.program_refused` a trois motifs fermés. Son message et son action nomment la cause que le produit a donnée :

| Cause nommée par le message | `reason` | Étape suivante |
|---|---|---|
| Rien n'existe au chemin donné | `program_or_directory_refused` | Donner un programme existant par `--runtime-program`, ou omettre cette option. |
| Le chemin donné n'est pas absolu | `program_or_directory_refused` | Donner le chemin absolu. |
| La forme d'un chemin : une traversée de parent, un caractère de contrôle, un chemin qui n'est pas UTF-8, ou le répertoire home du compte | `program_or_directory_refused` | Donner un chemin absolu sans traversée ; vérifier le répertoire home du compte de contrôle. |
| Le chemin ne peut pas être résolu : un lien qui ne mène nulle part, ou un répertoire qui ne peut pas être traversé | `program_or_directory_refused` | Donner le chemin du programme lui-même. |
| Le chemin se résout en la racine du système de fichiers | `program_or_directory_refused` | Donner le chemin du fichier du programme. |
| Le répertoire qui contient le programme appartient à un autre compte, ou le groupe ou les autres peuvent y écrire | `program_or_directory_refused` | Employer un répertoire où seuls root ou le compte de contrôle peuvent écrire. |
| Le programme n'est pas un fichier régulier exécutable de root ou du compte de contrôle, ou le groupe ou les autres peuvent y écrire | `program_or_directory_refused` | Donner un tel fichier. |
| Le programme dépasse 512 Mio | `program_or_directory_refused` | Donner le programme lui-même. |
| Le moteur d'un emplacement fixe est un lien vers un programme hors des emplacements approuvés sans chemin explicite | `program_or_directory_refused` | Passer ce programme par `--runtime-program ABS`. |
| Le système de fichiers n'est pas pris en charge | `program_or_directory_refused` | Installer le moteur sur un système de fichiers local pris en charge. |
| Le répertoire du programme ne peut pas être ouvert sans suivre un lien | `program_or_directory_refused` | Lancer l'outil sous le compte de contrôle ; vérifier son accès à chaque répertoire du chemin. |
| Le programme ne peut pas être ouvert en lecture | `program_or_directory_refused` | Lancer l'outil sous le compte de contrôle ; vérifier qu'il peut lire le programme. |
| La base des mots de passe ne donne aucun répertoire home utilisable pour le compte | `account_home_unavailable` | Lancer l'outil sous le compte de contrôle. |
| `/usr/bin`, `/bin`, `/usr/sbin` ou `/sbin` n'appartient pas à root, ou le groupe ou les autres peuvent y écrire | `helper_directories_refused` | Rétablir la propriété de root sans permission d'écriture du groupe ni des autres. |
| `/run/user/<uid>` existe et n'est pas un répertoire privé du compte | `unclassified` | Rétablir sa propriété et le mode 0700. |
| Une entrée d'accès étendue sur le programme, sur son répertoire ou sur un répertoire système du chemin des programmes auxiliaires | `unclassified` | Employer une installation sans entrée d'accès étendue. |

Sept de ces seize causes ont été produites par des lancements de l'outil : les lignes 1, 2, 4, 6, 7, 11 et 12 du tableau. Les neuf autres sont liées aux textes du produit par un test qui lit sa source, et n'ont été produites par aucun lancement. Les refus qui concernent les emplacements système fixes n'ont pas été exécutés contre un vrai moteur, et l'entrée d'accès étendue n'existe que sous macOS : ce guide n'en qualifie aucun.

`oidc.gateway_block_invalid` nomme, sous `basis`, les champs que lit le contrôle du produit derrière son motif :

| `reason` de `oidc.gateway_block_invalid` | Champs nommés sous `basis` | Où le guide SSO énonce leurs règles |
|---|---|---|
| `version_or_token_bound` | `gateway.version`, `gateway.provider.limits.max_token_bytes`, `gateway.provider.limits.pkce_ttl_seconds` | « Préparer le JSON privé du gateway » et « Limites des requêtes et sessions » |
| `url_unparseable` | `gateway.web_origin`, `gateway.provider.web_redirect_uri` | « Enregistrer les clients publics et l’audience API » |
| `web_origin_or_callback` | `gateway.web_origin`, `gateway.provider.web_redirect_uri`, `gateway.provider.transport` | « Enregistrer les clients publics et l’audience API » et « Préparer le JSON privé du gateway » |
| `claim_mapping_invalid` | `gateway.provider_label`, `gateway.mapping.revision`, `gateway.mapping.rules`, `gateway.mapping.default_capabilities` | « Préparer le JSON privé du gateway » et « Configurer les permissions effectives » |
| `claim_mapping_ambiguous` | `gateway.mapping.rules` | « Configurer les permissions effectives » |
| `provider_block_invalid` | `gateway.provider` | « Enregistrer les clients publics et l’audience API » et « Préparer le JSON privé du gateway » |
| `unclassified` | `gateway` | Le bloc entier : le refus est un refus que cet outil ne reconnaît pas. |

Pour l'objet fournisseur, le validateur du produit rend une seule valeur fermée. `provider_block_invalid` couvre donc toutes les règles de `gateway.provider` : sa version, des identifiants de clients et une audience distincts, les schémas de l'émetteur et des endpoints pour le transport déclaré, des endpoints distincts, la redirection, le chemin de callback, les retours de déconnexion et les limites. L'outil ne dit pas laquelle a échoué. Un émetteur `http` sous le transport `https_only` est l'un de ces cas : comparer les lignes `issuer` et `transport` du bloc d'identité.

### Catalogue des codes

Un code a un seul statut, un seul axe et une seule base. Certains codes ont un second message pour une situation nommée, par exemple un fichier réservé ; le code et son statut ne changent pas.

#### Disposition

| Code | Statut | Axe | Base | Sens |
|---|---|---|---|---|
| `layout.historical_config_dir` | `info` | `layout` | `observed` | Le répertoire a la disposition historique créée par `init`. |
| `layout.prepared_installation` | `info` | `layout` | `observed` | Le répertoire a la disposition publiée par `setup prepare`. |
| `layout.hand_written_registry` | `info` | `layout` | `observed` | Un registre a été donné seul ; ses références sont résolues comme le gateway les résout. |
| `layout.target_absent` | `error` | `layout` | `observed` | Le répertoire inspecté n'existe pas. L'action commence par le chemin : un répertoire donné par erreur, ou une installation qui existe ailleurs. |
| `layout.unrecognized` | `error` | `layout` | `observed` | Le répertoire ne contient aucune des deux dispositions. Un répertoire qui contient `tenants.json` ou `oidc.json` sans enregistrement de setup a son propre message, et une action qui nomme les deux options de fichier. Le message ne dit rien de ce qui a été ouvert : la liste des fichiers lus le dit. Quand le registre d'un répertoire au nom de staging a été lu pour dire ce qu'est le répertoire, le message le dit. |
| `layout.posture_refused` | `error` | `layout` | `observed` | Le répertoire est refusé par la règle du répertoire privé, ou n'est pas un répertoire. Le message distingue un mode autre que 0700 d'un refus qui vient d'un répertoire au-dessus de lui, d'un lien dans son chemin ou de son système de fichiers ; il nomme les entrées d'accès étendues de macOS comme cause de plus, d'après le code du produit. |
| `layout.inspector_identity_differs` | `unknown` | `layout` | `observed` | Le répertoire appartient à un autre compte ou n'a pas pu être examiné. |
| `layout.mixed` | `error` | `layout` | `observed` | Le répertoire contient les deux dispositions. |
| `layout.prepared_passed_as_config_dir` | `error` | `layout` | `observed` | Une installation préparée ou un répertoire de staging a été donné en `--config-dir`. |
| `layout.historical_passed_as_installation_dir` | `error` | `layout` | `observed` | Un répertoire de configuration historique a été donné en `--installation-dir`. |
| `layout.legacy_commands_do_not_read_this_layout` | `info` | `layout` | `inferred` | Les commandes historiques lisent un répertoire historique, jamais une installation préparée. |
| `layout.references_outside_inspected_root` | `warning` | `layout` | `observed` | Les fichiers de l'installation enregistrent des chemins hors du répertoire inspecté. |
| `layout.setup_staging_directory` | `error` | `layout` | `observed` | Un répertoire de staging conservé : la préparation n'a pas été publiée. |
| `layout.staging_and_final_both_exist` | `error` | `layout` | `observed` | Le répertoire de staging et un chemin enregistré par son registre existent tous deux. |
| `layout.staging_without_intent` | `error` | `layout` | `observed` | Un nom de staging sans enregistrement de préparation. |
| `layout.completion_marker_absent` | `warning` | `layout` | `observed` | Une installation publiée sans son marqueur de complétion. |
| `layout.sibling_staging_not_inspected` | `not_available` | `layout` | `observed` | Limite : un répertoire de staging à côté d'une installation n'est pas recherché. |
| `tool.linux_macos_posture_policy_only` | `not_available` | `layout` | `observed` | Limite : la politique de posture couvre les systèmes de fichiers locaux Linux et macOS. |

#### Topologie

| Code | Statut | Axe | Base | Sens |
|---|---|---|---|---|
| `topology.single_local_tenant` | `info` | `topology` | `inferred` | Un gateway démarré sans registre sert un seul tenant, nommé `local`. |
| `topology.registry_tenants` | `info` | `topology` | `declared` | Le registre déclare les tenants comptés dans le rapport. |
| `topology.running_services_not_checked` | `not_checked` | `topology` | `observed` | Aucun service n'est démarré ni interrogé sur son état : les services en cours d'exécution ne sont pas vérifiés. |
| `topology.gateway_arguments_not_persisted` | `not_checked` | `topology` | `observed` | Les arguments du gateway et du dashboard ne sont pas conservés dans l'installation. |
| `topology.kubernetes_launch_not_available` | `not_available` | `topology` | `observed` | Limite : le lancement de workers par l'adaptateur Kubernetes n'est pas disponible. |

#### Identité

| Code | Statut | Axe | Base | Sens |
|---|---|---|---|---|
| `identity.sso_not_inspected` | `not_checked` | `identity` | `observed` | Aucun fichier OIDC n'a été donné. Le SSO est facultatif et n'est activé que par une option du gateway. |
| `identity.sso_file_valid_offline` | `ok` | `identity` | `inferred` | Le fichier OIDC passe les règles hors ligne du chargeur du gateway. Le verdict est tiré du seul fichier. |
| `identity.sso_gateway_enablement_not_persisted` | `not_checked` | `identity` | `observed` | Un fichier valide ne prouve pas que le gateway est démarré avec lui. |
| `identity.sso_demonstration_transport` | `warning` | `identity` | `declared` | Le fichier OIDC déclare le transport de démonstration isolé : une connexion en HTTP en clair, que le produit limite à un émetteur loopback et à l'émetteur de démonstration distribué. Pas pour une installation normale. |
| `identity.local_named_accounts_not_available` | `not_available` | `identity` | `observed` | Les comptes locaux nominatifs vivent dans les bases des tenants. |
| `identity.idp_reachability_not_checked` | `not_checked` | `identity` | `observed` | Le fournisseur d'identité n'a pas été contacté : aucune connexion n'est ouverte sans la vérification active. Rapporté aussi quand la vérification est demandée et ne peut pas s'exécuter. |
| `active.oidc_discovery_ok` | `ok` | `identity` | `observed` | Vérification active : le fournisseur a répondu, son document de découverte confirme le fichier OIDC, et son jeu de clés contient une clé de signature utilisable. |
| `active.oidc_discovery_transport_unavailable` | `unknown` | `identity` | `observed` | Vérification active : aucune réponse utilisable depuis l'endroit où l'outil s'exécute. Connexion, TLS, délai ou borne de taille ; aucun autre détail n'existe. |
| `active.oidc_discovery_provider_rejected` | `error` | `identity` | `observed` | Vérification active : le fournisseur a répondu, mais pas avec les documents JSON d'une découverte. Sa réponse n'est pas imprimée. |
| `active.oidc_discovery_metadata_mismatch` | `error` | `identity` | `observed` | Vérification active : le document de découverte ne confirme pas l'émetteur, les endpoints ou les capacités qu'exige le fichier OIDC. |
| `active.oidc_discovery_keys_invalid` | `error` | `identity` | `observed` | Vérification active : le jeu de clés du fournisseur est refusé par le client du produit. |
| `identity.arbitrary_oidc_provider_not_qualified` | `not_available` | `identity` | `observed` | Limite : la compatibilité avec un fournisseur d'identité donné n'est pas présumée. |
| `oidc.file_absent` | `error` | `identity` | `observed` | Le fichier OIDC n'existe pas au chemin donné. |
| `oidc.file_unavailable_or_not_private` | `error` | `identity` | `observed` | Le fichier OIDC est refusé par la règle du fichier privé, ou est un fichier réservé. Il n'a pas été lu. |
| `oidc.json_invalid` | `error` | `identity` | `inferred` | Le fichier OIDC n'est pas le document JSON fermé qu'attend le gateway. |
| `oidc.version_plan_pair_invalid` | `error` | `identity` | `inferred` | La version 1 ne porte aucun plan de capacité ; la version 2 en exige un. |
| `oidc.gateway_block_invalid` | `error` | `identity` | `observed` | Le bloc gateway est refusé par le validateur du produit, avec un motif fermé. Les champs que concerne le motif sont listés sous `basis` ; le tableau sous les motifs les nomme. |
| `oidc.capacity_plan_invalid` | `error` | `identity` | `observed` | Le plan de capacité est refusé par le validateur du produit. |
| `oidc.authentication_policy_invalid` | `error` | `identity` | `observed` | La politique d'audit d'authentification est refusée par un validateur du produit pour cet audit : le contrôle de politique du gateway, ou le contrôle que la politique finance le plan de capacité. |
| `oidc.reference_invalid` | `error` | `identity` | `inferred` | Une référence de fichier est vide ou sort de son répertoire par traversée de parent. |
| `oidc.loader_acceptance_not_checked` | `not_checked` | `identity` | `observed` | L'acceptation par le gateway au démarrage n'est pas vérifiée. |

#### Tenants

| Code | Statut | Axe | Base | Sens |
|---|---|---|---|---|
| `registry.file_unreadable` | `error` | `tenants` | `inferred` | Le registre n'existe pas, ne peut pas être lu comme le gateway le lit, ou est un fichier réservé. Un fichier absent donné par option et un répertoire d'installation sans son `tenants.json` ont chacun leur message et leur action. |
| `registry.json_invalid` | `error` | `tenants` | `inferred` | Le registre n'est pas le document JSON fermé qu'attend le gateway. |
| `registry.version_unknown` | `error` | `tenants` | `inferred` | La version du registre n'est pas 1. |
| `registry.empty` | `error` | `tenants` | `inferred` | Le registre ne déclare aucun tenant. |
| `registry.tenant_id_invalid` | `error` | `tenants` | `observed` | Un identifiant de tenant est refusé par le validateur du produit. |
| `registry.tenant_id_duplicate` | `error` | `tenants` | `inferred` | Un identifiant de tenant apparaît plus d'une fois. |
| `registry.reference_empty` | `error` | `tenants` | `inferred` | Un tenant a une référence de base ou de fichier de clé vide. |
| `registry.storage_invalid` | `error` | `tenants` | `observed` | Un tenant doit déclarer un seul sélecteur de stockage, `data` ou un profil `storage` valide. Corriger le registre fourni avec `--tenants-file`. |
| `registry.mysql_unavailable` | `error` | `tenants` | `observed` | Ce gateway ne peut pas servir le profil MySQL déclaré. Conserver la destination dans le registre fourni avec `--tenants-file` et utiliser un gateway qui prend en charge le profil. |
| `registry.policy_invalid` | `error` | `tenants` | `observed` | Une politique de tenant est refusée par le validateur du produit ; les détails nomment la politique. Une politique de capacité d'audit ou d'audit des workloads est jugée pour la borne d'événements donnée par `--max-events`. |
| `registry.policy_refused_at_assumed_event_bound` | `unknown` | `tenants` | `inferred` | Une politique de capacité d'audit ou d'audit des workloads est refusée à 100000 événements, valeur par défaut du gateway que cet outil suppose quand `--max-events` n'est pas donné. Un gateway démarré avec une autre borne peut l'accepter. |
| `registry.event_bound_not_used` | `not_checked` | `tenants` | `observed` | Une borne d'événements a été donnée par `--max-events` et rien n'a été validé avec elle : aucun registre décodé ne déclare de politique de capacité d'audit ou d'audit des workloads. |
| `audit.live_activity_budget_capacity` | `warning` | `tenants` | `inferred` | Avec le budget par session de l'activité fournisseur live, un audit de tenant contient moins de 50 sessions selon la borne d'événements du gateway ou selon le plafond physique de 256 Mio ; l'audit n'a pas encore de rotation et refuse toute écriture une fois plein. |
| `registry.offline_rules_passed` | `ok` | `tenants` | `inferred` | Le registre passe les règles hors ligne du chargeur du gateway. Le contenu des jetons n'est pas vérifié. |
| `registry.loader_acceptance_not_checked` | `not_checked` | `tenants` | `observed` | L'acceptation par le gateway au démarrage n'est pas vérifiée. |
| `tenants.plan_matches_registry` | `ok` | `tenants` | `inferred` | Le plan et le registre nomment exactement les mêmes tenants. |
| `tenants.plan_set_mismatch` | `error` | `tenants` | `inferred` | Le plan et le registre nomment des tenants différents ; les détails listent chaque côté. L'action est de ramener le registre à l'ensemble de tenants du plan : le plan que conserve un audit d'authentification de format 4 ne peut pas être changé, et le gateway refuse un fichier dont le plan en diffère. Le plan d'un fichier ne peut être changé que tant qu'aucun audit de ce format n'existe : avant la première initialisation de l'audit d'un fichier écrit à la main, ou pour la migration 3 vers 4 du guide SSO. |
| `tenants.plan_required_for_multi_tenant` | `error` | `tenants` | `inferred` | Plusieurs tenants avec un fichier OIDC en version 1, qui n'a pas de plan. |
| `tenants.single_tenant_without_plan` | `ok` | `tenants` | `inferred` | Un tenant et un fichier en version 1 : les deux fichiers sont compatibles. |
| `tenants.correspondence_not_checked` | `not_checked` | `tenants` | `observed` | Les tenants et le plan n'ont pas été comparés. Sans registre à côté d'un répertoire de configuration historique, l'action dit ce qu'exigent un fichier de version 1 et un fichier de version 2 pour le seul tenant `local`. |
| `tenants.durable_plan_comparison_not_available` | `not_available` | `tenants` | `observed` | Le plan conservé dans l'audit d'authentification ne peut pas être lu hors ligne. |

#### Références de credentials

| Code | Statut | Axe | Base | Sens |
|---|---|---|---|---|
| `reference.absent` | `error` | `credential_references` | `observed` | Un fichier référencé n'existe pas à son chemin enregistré, dans le répertoire inspecté. Pour une clé d'un répertoire de configuration historique, l'action nomme `agentsandbox init`, qui crée la clé manquante et préserve les clés existantes. |
| `reference.not_found_from_here` | `unknown` | `credential_references` | `observed` | Un chemin référencé hors du répertoire inspecté n'a pas été trouvé depuis ici. |
| `reference.not_a_regular_file` | `error` | `credential_references` | `observed` | Un chemin référencé est un lien symbolique ou n'est pas un fichier régulier. |
| `reference.private_rule_violated` | `error` | `credential_references` | `inferred` | Un fichier lu par la règle du fichier privé a un autre mode que 0600, plusieurs liens, ou des droits accordés à un autre compte ; ou son lecteur refuse un répertoire qui y mène. |
| `reference.private_rule_not_evaluated` | `not_checked` | `credential_references` | `observed` | Un fichier lu par la règle du fichier privé est conforme et son lecteur accepte les répertoires qui y mènent, mais son répertoire n'est pas un répertoire privé 0700 : le contrôle d'entrée du produit n'a pas tourné, et le système de fichiers et les entrées d'accès du fichier ne sont pas vérifiés. |
| `reference.token_file_accessible_to_other_accounts` | `warning` | `credential_references` | `observed` | Le lecteur de jetons du gateway ne contrôle aucun mode, et le mode de ce fichier de clé donne accès à son groupe ou à d'autres comptes. Son contenu n'a pas été lu : rien n'est dit de son acceptation. Un mode qui refuse en plus la lecture au compte du gateway reçoit le même avertissement, et le gateway ne peut pas ouvrir un tel fichier. |
| `reference.inspector_identity_differs` | `unknown` | `credential_references` | `observed` | Un fichier référencé appartient à un autre compte ; sa règle n'a pas été jugée. Aussi un registre d'un autre compte que ce compte ne peut pas lire. |
| `reference.metadata_unavailable` | `unknown` | `credential_references` | `observed` | Les métadonnées d'un chemin référencé n'ont pas pu être lues. |
| `reference.content_not_checked` | `not_checked` | `credential_references` | `observed` | Aucun fichier que les fichiers inspectés désignent comme fichier de clé, de jeton ou de credential n'est ouvert : le contenu n'est pas vérifié. Quand un fichier a été lu avant d'être nommé, le message nomme `reference.file_read_before_it_was_named` à la place. |
| `reference.setup_secret_material_present` | `info` | `credential_references` | `observed` | L'enregistrement de reprise du setup qui détient les jetons générés est présent. Il n'a pas été ouvert. |
| `reference.file_read_before_it_was_named` | `error` | `credential_references` | `observed` | Un fichier ouvert comme entrée est nommé, par un fichier lu après lui, comme fichier de clé, credential, enregistrement de setup ou base. Son contenu a été lu ; il n'est pas imprimé. Son entrée, là où le rapport liste le fichier, porte `content` `read_before_named`. |

#### Fournisseurs

| Code | Statut | Axe | Base | Sens |
|---|---|---|---|---|
| `provider.not_inspected` | `not_checked` | `providers` | `observed` | Aucun emplacement fournisseur n'a été donné, ou le répertoire de configuration n'a pas pu être inspecté. |
| `provider.file_absent` | `info` | `providers` | `observed` | Le répertoire de configuration n'a pas de `providers.json` : aucun profil géré n'est déclaré. |
| `provider.file_unavailable_or_not_private` | `error` | `providers` | `observed` | Le fichier de profils est refusé par la règle du fichier privé, est absent là où une option le nomme, ou est un fichier réservé. |
| `provider.configuration_valid` | `ok` | `providers` | `observed` | Le fichier de profils est accepté par le validateur du produit, qui lit ce seul fichier. |
| `provider.configuration_invalid` | `error` | `providers` | `observed` | Le fichier de profils est refusé par le validateur du produit, avec un motif fermé. |
| `provider.configuration_ambiguous` | `error` | `providers` | `observed` | Un profil en double, une priorité inconnue ou en double, ou un profil par défaut qui n'existe pas. |
| `provider.login_required` | `error` | `providers` | `inferred` | Le profil que la sélection résout est un profil abonnement et son fichier de credential est absent. Aucun profil API n'est essayé. |
| `provider.credential_missing` | `error` | `providers` | `inferred` | Le profil que la sélection résout est un profil API et son fichier de credential est absent. |
| `provider.credential_unsafe` | `error` | `providers` | `inferred` | Le fichier de credential du profil que la sélection résout est un lien, n'est pas un fichier régulier, ou viole la règle du fichier privé ; ou son chemin passe par une entrée qui n'est pas un répertoire ; ou son lecteur refuse un répertoire qui y mène. |
| `provider.unselected_profile_reference_absent` | `warning` | `providers` | `inferred` | Le fichier de credential d'un profil que la sélection ne résout pas est absent. |
| `provider.unselected_profile_reference_unsafe` | `warning` | `providers` | `inferred` | Le fichier de credential d'un profil que la sélection ne résout pas n'est pas sûr, son chemin passe par une entrée qui n'est pas un répertoire, ou son lecteur refuse un répertoire qui y mène. |
| `provider.credential_content_not_checked` | `not_checked` | `providers` | `observed` | Aucun fichier que le fichier de profils désigne comme credential fournisseur n'est ouvert : son contenu n'est pas vérifié. Quand un fichier a été lu avant d'être nommé, le message nomme `reference.file_read_before_it_was_named` à la place. |
| `provider.manual_launch_path_not_checked` | `not_checked` | `providers` | `observed` | Le chemin de lancement manuel n'emploie pas le fichier de profils. |
| `provider.launch_requires_observe_and_bridge` | `info` | `providers` | `inferred` | Un lancement géré avec des credentials fournisseur exige `--profile observe` et `--network bridge`. |
| `provider.selection_resolved` | `ok` | `providers` | `observed` | La sélection demandée résout un profil, par le choix du produit, qui ne lit aucun fichier de credential. |
| `provider.selection_ambiguous` | `error` | `providers` | `observed` | Le produit refuse la sélection comme ambiguë : plusieurs profils correspondent et rien ne tranche, ou la demande contredit le profil demandé. |
| `provider.selection_no_profile` | `error` | `providers` | `observed` | Aucun profil déclaré ne répond à la sélection. Aucun profil d'un autre mode d'authentification n'est employé à sa place. |
| `provider.selection_invalid` | `error` | `providers` | `observed` | Un identifiant de profil ou de modèle demandé est refusé par le validateur du produit. |
| `provider.selection_not_resolved` | `not_checked` | `providers` | `observed` | Des options de sélection ont été données et aucun fichier de profils n'a été chargé : rien n'a été résolu. |
| `provider.subscription_never_falls_back_to_api` | `info` | `providers` | `inferred` | Un abonnement est demandé ou résolu et le lancement refuse : les profils API du fichier n'ont pas été essayés. Le message dit de quel cas il s'agit : un profil résolu dont le credential est refusé, une demande d'abonnement refusée, ou un fichier qui ne déclare aucun profil abonnement. L'action énonce la règle automatique : un profil API peut être résolu quand aucun profil abonnement ne correspond au client demandé, et jamais pour une demande faite avec `--auth-mode subscription`. |
| `provider.api_billing_notice` | `warning` | `providers` | `inferred` | Le profil résolu est en mode API : l'usage est facturé par le fournisseur. |
| `provider.qualified_versions_not_available` | `not_available` | `providers` | `observed` | Limite : ce build ne contient aucune liste de versions de clients qualifiées. |
| `provider.client_presence_not_available` | `not_available` | `providers` | `observed` | Limite : la présence d'un client n'est sondée que dans le worker. |
| `provider.client_version_not_available` | `not_available` | `providers` | `observed` | Limite : la version d'un client n'est sondée que dans le worker. |
| `provider.account_validity_not_available` | `not_available` | `providers` | `observed` | Limite : seul le fournisseur établit la validité d'un compte. |

#### Inférence

| Code | Statut | Axe | Base | Sens |
|---|---|---|---|---|
| `inference.decision_engine_selection_not_persisted` | `not_checked` | `inference` | `observed` | Le moteur de décision est sélectionné par des options du gateway qui ne sont pas conservées. Le gateway refuse un jeu incomplet ou mixte de ces options, que cet outil ne vérifie pas. |
| `inference.decision_transport_valid` | `ok` | `inference` | `observed` | Le transport donné est accepté par le validateur du produit. Le moteur n'a pas été contacté, et le jeu d'options du gateway n'est pas vérifié. |
| `inference.decision_transport_not_checked` | `not_checked` | `inference` | `observed` | Le transport donné n'a pas été jugé : le bundle d'AC donné avec lui n'a pas pu être utilisé. |
| `inference.decision_transport_inputs_without_url` | `not_checked` | `inference` | `observed` | Une permission de HTTP local ou un bundle d'AC du moteur de décision a été donné sans URL : aucun transport n'a été jugé. |
| `inference.local_model_backend_not_available` | `not_available` | `inference` | `observed` | Aucune inspection hors ligne d'un backend de modèle local. |
| `inference.model_tls_outputs_not_available` | `not_available` | `inference` | `observed` | Aucun lecteur pour les sorties du provisionnement TLS du modèle. |
| `inference.internal_profile_not_available` | `not_available` | `inference` | `observed` | Limite : aucun profil géré d'inférence interne pour Codex CLI ou Claude Code. |

#### Confiance

| Code | Statut | Axe | Base | Sens |
|---|---|---|---|---|
| `trust.oidc_ca_bundle_accepted_by_grammar` | `ok` | `trust` | `observed` | Le bundle d'AC du fichier OIDC est accepté : des certificats seulement, au plus 8 et 65536 octets. |
| `trust.decision_engine_ca_accepted_by_grammar` | `ok` | `trust` | `observed` | Le bundle donné pour le moteur de décision est accepté par la grammaire de bundle. |
| `trust.gateway_ca_accepted_by_grammar` | `ok` | `trust` | `observed` | Le bundle donné pour les clients d'un gateway HTTPS est accepté par la grammaire de bundle. |
| `trust.ca_bundle_invalid` | `error` | `trust` | `observed` | Un bundle est refusé par la grammaire de bundle. Son contenu n'est pas imprimé. |
| `trust.ca_incompatible_with_transport` | `error` | `trust` | `observed` | Un bundle d'AC exige le transport `https_only`. |
| `trust.ca_unavailable_or_not_private` | `error` | `trust` | `observed` | Un bundle n'existe pas à son chemin, ou est refusé par la règle de fichier de son consommateur : chaque cas a son message. Il n'a pas été lu. |
| `trust.ca_reference_is_a_credential_reference` | `error` | `trust` | `observed` | Une référence d'AC pointe vers un fichier réservé. Il n'a pas été ouvert. |
| `trust.decision_transport_invalid` | `error` | `trust` | `observed` | Le transport du moteur de décision donné est refusé par le validateur du produit : la forme de l'URL, du HTTP en clair hors d'une adresse loopback ou sans la permission locale, ou cette permission avec un bundle d'AC, quelle que soit l'URL. |
| `trust.certificate_validity_period_not_available` | `not_available` | `trust` | `observed` | Les dates de validité ne sont pas évaluées. |

#### Moteur

| Code | Statut | Axe | Base | Sens |
|---|---|---|---|---|
| `engine.program_not_inspected` | `not_checked` | `engine` | `observed` | Aucun moteur n'a été nommé. |
| `engine.state_not_checked` | `not_checked` | `engine` | `observed` | Le moteur n'est jamais exécuté : son état n'est pas vérifié. |
| `engine.program_approved` | `ok` | `engine` | `observed` | Le programme passe la règle d'approbation du produit. Il a été haché, pas exécuté. |
| `engine.program_absent_from_fixed_entries` | `error` | `engine` | `observed` | Aucun programme de ce type n'existe aux emplacements système fixes. `PATH` n'est jamais parcouru. |
| `engine.program_refused` | `error` | `engine` | `observed` | Le programme est refusé par la règle d'approbation, avec un motif fermé. Le message et l'action nomment la cause que le produit a donnée : le tableau sous les motifs les liste toutes. |
| `engine.program_is_a_credential_reference` | `error` | `engine` | `observed` | Le programme du moteur donné est un fichier réservé. Il n'a été ni ouvert, ni approuvé, ni haché. |

#### Données

| Code | Statut | Axe | Base | Sens |
|---|---|---|---|---|
| `data.offline_inspection_not_available` | `not_available` | `data` | `observed` | Ce diagnostic n’ouvre jamais de connexion SQLite vers les bases ; schéma, liaison au tenant, format d’audit et intégrité n’y sont pas examinés. |
| `data.signed_checkpoint_not_checked` | `not_checked` | `data` | `observed` | Les checkpoints signés du tenant et la continuité anti-rollback indépendante exigent le vérificateur explicite séparé. |
| `data.location_not_persisted` | `not_checked` | `data` | `observed` | Le chemin de la base d'un répertoire historique est une option du gateway. |
| `data.store_absent` | `warning` | `data` | `observed` | Aucune base n'existe au chemin enregistré, dont le gateway accepte le nom. Le gateway en crée une vide à son démarrage. |
| `data.authentication_audit_absent` | `warning` | `data` | `observed` | Aucun audit d'authentification n'existe au chemin enregistré par un fichier OIDC donné seul. Le gateway refuse de démarrer, sauf initialisation explicite d'un nouvel audit. |
| `data.authentication_audit_missing_from_prepared_installation` | `error` | `data` | `observed` | L'audit d'authentification que `setup prepare` a créé pour l'installation inspectée est absent. Le démarrage documenté, sans `--initialize-oidc-audit`, refuse. |
| `data.store_posture_refused` | `error` | `data` | `observed` | Une base est refusée par la règle de l'entrée privée, son chemin enregistré contient une traversée de parent, ou son nom de fichier est un nom que le gateway refuse : non ASCII, ou un nom SQLite réservé. |
| `data.parent_directory_not_private` | `error` | `data` | `observed` | Le répertoire qui contient une base est refusé par la règle du répertoire privé : son propriétaire, le mode 0700, les répertoires au-dessus de lui, un lien dans son chemin ou son système de fichiers. Le message ne distingue pas ces causes. |
| `data.orphan_sidecars` | `error` | `data` | `observed` | Des fichiers sidecar existent sans leur base. |
| `data.sidecars_present` | `info` | `data` | `observed` | Des fichiers sidecar existent à côté d'une base : elle est en cours d'utilisation, ou n'a pas été fermée proprement. |

#### Sortie

| Code | Statut | Axe | Base | Sens |
|---|---|---|---|---|
| `output.value_masked` | `warning` | `output` | `observed` | Une valeur ressemble à un credential pour le filtre de sortie et est montrée par un emplacement symbolique. |
| `output.detail_omitted` | `info` | `output` | `observed` | Le rapport a dépassé sa borne de taille et ses listes de détail ont été vidées. Les totaux restent exacts. |
