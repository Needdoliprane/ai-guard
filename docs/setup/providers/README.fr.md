# Codex, Claude Code et moteurs de décision

[English](README.md) · [Index d'installation](../README.fr.md) · [Installation locale](../local/README.fr.md) · [Broker de confiance](../broker/README.fr.md)

Configurer trois choix indépendants : identité ai-guard, client de développement et son authentification abonnement/API, puis moteur de décision du contrôle. Une connexion à l'IdP ne connecte pas Codex ou Claude Code. Une clé Jev autorise un service de décision du contrôle ; elle n'appartient jamais à un fichier de credentials worker.

## Préparer les clients et une petite première tâche

Lancer la [pile locale](../local/README.fr.md) et conserver ses variables shell, son moteur et son URL gateway. Choisir un projet sans credentials privés. Sélectionner un modèle disponible pour son compte et borner la tâche et sa durée avant de commencer : une requête API peut être facturée par le fournisseur, tandis qu'un abonnement suit ses règles d'éligibilité et d'usage.

Construire le worker avec des versions exactes des clients. Ces versions sont des références explicites, pas une consigne de choisir la dernière publication :

```sh
"$AIGUARD_BIN" build --runtime docker --source . --codex-version 0.165.0 --claude-code-version 2.1.140
```

Utiliser `--runtime podman` avec Podman. Appliquer l'exception rootful du guide local uniquement si elle a été choisie pour l'installation. Ni plage, ni joker, ni dernière version implicite dans une image approuvée. Construire un client ne l'authentifie pas. Pour une autre version prise en charge, reconstruire volontairement et conserver version résolue et identité d'image avec la configuration de session. `build` remet au moteur une copie privée préparée d'une liste fixe d'entrées, pas le checkout lui-même, n'affiche la sortie du moteur qu'après une réussite et borne chaque build d'image à 30 minutes. Le guide local décrit son exigence de répertoire temporaire et ses refus, dont un `.dockerignore` différent de celui que le CLI reflète, dans [sources préparées de build et Compose](../local/README.fr.md#prepared-source) ; l'[interruption, les délais et la sortie du moteur](../local/README.fr.md#interrupt-build-or-compose) et la [reprise d'une copie conservée](../local/README.fr.md#prepared-source-retained) ont leurs propres sections.

La sélection gérée construit l'invocation fournisseur ; aucun argv fournisseur n'est à fournir. `--client-profile` choisit le profil fournisseur, tandis que `--profile` choisit la politique de décision ai-guard. Ne pas combiner les options gérées avec un argv manuel, un `--provider` non générique ou `--secret-env-file`.

Pour les deux modes d'authentification Codex, choisir un modèle disponible pour le compte retenu. Cette valeur shell passe par `--model` ; ai-guard ne la lit pas comme réglage d'environnement :

```sh
AIGUARD_CODEX_MODEL="replace-with-your-approved-model-id"
```

## Credentials API

Créer une nouvelle entrée privée hors du projet. Le parent suivant est créé par `init` ; créer le fichier sans rien écraser :

```sh
(
  umask 077
  set -C
  cat .env.exemple.providers > "$AIGUARD_CONFIG/secrets/providers.env"
)
```

L'ouvrir dans un éditeur privé et renseigner uniquement les clés voulues. Garder le fichier régulier en `0600` et son parent en `0700`. Le format est littéral `NAME=value` : ni `export`, ni interpolation, ni guillemets shell autour des valeurs. Ne pas le charger par `source`. L'exemple contient `OPENAI_API_KEY` et `ANTHROPIC_API_KEY` vides ; une entrée non sélectionnée vide ne fournit aucun credential. Ne jamais placer une clé dans des arguments, un prompt, une capture ou Git.

Créer les références de profils et examiner les choix configurés :

```sh
"$AIGUARD_BIN" providers init --config-dir "$AIGUARD_CONFIG" --env-file "$AIGUARD_CONFIG/secrets/providers.env"
"$AIGUARD_BIN" providers status --config-dir "$AIGUARD_CONFIG" --json
```

`providers init` crée `providers.json` avec `codex-api` et `claude-api` ; il préserve une configuration existante et ne lit pas les credentials. `status` examine la configuration, pas la validité du compte ou les capacités du worker. Ces prérequis doivent être vérifiés avant la tâche réelle ; un contrôle de disponibilité du service n'authentifie pas un fournisseur.

Lancer la tâche API avec ce modèle explicite :

```sh
"$AIGUARD_BIN" run --project "$AIGUARD_PROJECT" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY" --runtime docker --profile observe --network bridge --client codex --auth-mode api --client-profile codex-api --model "$AIGUARD_CODEX_MODEL" --timeout 120 --prompt "Report the top-level file names only. Do not modify files, run tests, or start subagents."
```

Pour Claude Code/API, choisir le modèle exact demandé plutôt qu'un alias :

```sh
"$AIGUARD_BIN" run --project "$AIGUARD_PROJECT" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY" --runtime docker --profile observe --network bridge --client claude-code --auth-mode api --client-profile claude-api --model claude-haiku-5-5 --timeout 120 --prompt "Report the top-level file names only. Do not modify files, run tests, or start subagents."
```

Adapter le runtime pour Podman et ajouter volontairement l'exception rootful si nécessaire. `observe + bridge` est un choix explicite pour ces sessions avec credential. Bridge autorise le trafic sortant ; ce n'est pas une liste de destinations approuvées. Considérer les données et le credential choisi comme accessibles au client fournisseur, au code du worker et à l'autorité moteur. Le masquage des sorties n'empêche pas cet accès.

## Credentials d'abonnement

Obtenir volontairement les credentials d'abonnement par le parcours officiel de chaque fournisseur, dans un contexte dédié hors du projet. Ne pas copier tout le répertoire personnel, chercher dans des caches OAuth ou extraire le trousseau du système. Exclure les clés API des entrées abonnement pour garder le mode sélectionné sans ambiguïté.

Pour Codex, préparer un nouveau `CODEX_HOME` privé et demander le stockage fichier des credentials. Le parcours officiel par appareil dépend des réglages du compte et de l'organisation :

```sh
mkdir -m 700 "$AIGUARD_CONFIG/secrets/codex-subscription" &&
  CODEX_HOME="$AIGUARD_CONFIG/secrets/codex-subscription" codex -c 'cli_auth_credentials_store="file"' login --device-auth
```

Utiliser un CLI Codex hôte compatible uniquement pour cette connexion volontaire. Conserver un répertoire existant au lieu de l'écraser ; choisir un autre répertoire privé neuf si nécessaire. Garder `auth.json` privé (`0600`) et le traiter comme un mot de passe. Le profil doit référencer un credential abonnement ChatGPT sans clé API non nulle. Voir l'[authentification Codex](https://learn.chatgpt.com/docs/auth?surface=cli).

Pour Claude Code, utiliser le parcours officiel de token d'abonnement dans un terminal privé non enregistré :

```sh
claude setup-token
```

Placer la valeur retournée dans un nouveau fichier privé `<config-dir>/secrets/claude-subscription.env`, avec pour seule entrée `CLAUDE_CODE_OAUTH_TOKEN=value`. Le créer/modifier en privé en mode `0600` ; ne pas mettre la valeur dans une commande ni capturer l'écran du token. Vérifier éligibilité et expiration auprès du fournisseur. Ce token couvre le mécanisme d'authentification Claude Code pris en charge ; il ne donne pas tous les services du compte. Voir l'[authentification Claude Code](https://code.claude.com/docs/en/authentication).

Créer un **nouveau** fichier privé `<config-dir>/providers-subscriptions.json`, en préservant les profils API existants. L'[exemple JSON fourni](../../../config/providers-subscriptions.example.json) contient les références non secrètes ci-dessous ; adapter le chemin Codex si nécessaire :

```json
{
  "version": 1,
  "auto_priority": ["codex-subscription", "claude-subscription"],
  "profiles": [
    {
      "id": "codex-subscription",
      "revision": 1,
      "client": "codex",
      "auth_mode": "subscription",
      "credential": {
        "kind": "codex_auth_file",
        "path": "secrets/codex-subscription/auth.json"
      }
    },
    {
      "id": "claude-subscription",
      "revision": 1,
      "client": "claude_code",
      "auth_mode": "subscription",
      "credential": {
        "kind": "claude_oauth_env_file",
        "path": "secrets/claude-subscription.env"
      }
    }
  ]
}
```

Garder aussi cette configuration en `0600`. Les références relatives partent de son parent, pas du projet ou du répertoire courant. Utiliser des chemins ordinaires sans lien ni traversée. Le nom canonique de la source Claude est `claude_oauth_env_file` ; l'ancienne graphie `claude_o_auth_env_file` reste acceptée et le chargement ne réécrit pas le fichier. Les anciens binaires peuvent refuser un fichier écrit avec le nom canonique. Examiner explicitement ce fichier, puis lancer la session abonnement souhaitée :

```sh
"$AIGUARD_BIN" providers status --config-dir "$AIGUARD_CONFIG" --providers-file "$AIGUARD_CONFIG/providers-subscriptions.json" --json
"$AIGUARD_BIN" run --project "$AIGUARD_PROJECT" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY" --runtime docker --profile observe --network bridge --providers-file "$AIGUARD_CONFIG/providers-subscriptions.json" --client codex --auth-mode subscription --client-profile codex-subscription --model "$AIGUARD_CODEX_MODEL" --timeout 120 --prompt "Report the top-level file names only. Do not modify files, run tests, or start subagents."
"$AIGUARD_BIN" run --project "$AIGUARD_PROJECT" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY" --runtime docker --profile observe --network bridge --providers-file "$AIGUARD_CONFIG/providers-subscriptions.json" --client claude-code --auth-mode subscription --client-profile claude-subscription --model claude-haiku-5-5 --timeout 120 --prompt "Report the top-level file names only. Do not modify files, run tests, or start subagents."
```

Il s'agit de tâches distinctes ; ne lancer que celle voulue. Utiliser `--interactive` lorsqu'un véritable terminal autorisé est nécessaire, en conservant les choix explicites client/mode/profil. `--interactive` ne se combine pas avec `--capture-output`. La continuité du terminal et la reprise de session ne signifient pas qu'un démarrage consommé peut être attaché à nouveau.

## Sélection explicite et Auto

Client, mode et profil explicites sont prioritaires. Choix public, modèle, image et provenance de configuration doivent être visibles avant autorisation. `--model` remplace le modèle du profil ; il doit être disponible pour le compte et le mode choisis. Une demande d'abonnement ne devient jamais silencieusement une requête API facturée.

`--client auto --auth-mode auto` n'examine que les profils du fichier sélectionné. Les abonnements configurés passent avant les profils API, puis un `default_profile` compatible et l'ordre explicite `auto_priority` départagent les choix. Une ambiguïté restante est refusée. Pour proposer les deux modes, créer une configuration relue regroupant leurs profils, avec identifiants uniques et priorité explicite. Une clé API ambiante ou une connexion CLI hôte n'ajoute pas de profil.

Les références de credentials sont des données privées du contrôle. Seul le credential sélectionné est préparé pour le worker. Modifier la source ne réécrit pas une demande en attente déjà autorisée : obtenir une nouvelle décision pour d'autres credentials ou réglages d'exécution. Conserver plans et bindings en attente pour réconciliation au lieu de les modifier pour forcer un redémarrage.

### Quota abonnement épuisé

Si le fournisseur signale une limite d'usage de l'abonnement, examiner la session existante avant de réessayer ; conserver son plan/binding et réconcilier un runtime incertain. Credential API, budget de facturation et quota API disponible sont des prérequis distincts. Ni le refus de l'abonnement ni `providers status` ne prouvent la disponibilité de l'API.

Si vous choisissez explicitement l'API facturée, préparer les [credentials et profils API](#credentials-api) privés, puis examiner la configuration API elle-même :

```sh
"$AIGUARD_BIN" providers status --config-dir "$AIGUARD_CONFIG" --providers-file "$AIGUARD_CONFIG/providers.json" --json
```

Utiliser le fichier qui contient effectivement `claude-api` ou `codex-api` ; un fichier limité aux abonnements ne les fournit pas et `providers init` ne fusionne pas de profils dans un fichier existant. Préserver les entrées d'abonnement existantes. Pour la nouvelle tâche, reprendre la commande API ci-dessus avec `--providers-file` désignant ce fichier examiné et les choix explicites `--client claude-code --auth-mode api --client-profile claude-api` (ou `--client codex --auth-mode api --client-profile codex-api`). Garder un modèle approuvé et un budget de tâche. Cette nouvelle tâche passe par la décision habituelle du policy engine ai-guard ; cela ne reprend pas la session d'abonnement précédente et ne permet aucun repli automatique si l'API est refusée.

## Choisir le moteur de décision

| Moteur | Usage et limites |
|---|---|
| Rust déterministe | Décisions sans modèle ni credential de modèle externe. Les règles ont une couverture sémantique incomplète ; intentions inconnues et effets transitifs doivent rester explicites. |
| Jev réel | Service de décision TypeSafe authentifié avec provenance du modèle. Son credential reste dans le contrôle. |
| Modèle local Jev-like | Petit modèle conteneurisé avec poids/runtime épinglés, sortie validée et bornée, incertitude et délais explicites. Il est distinct de Jev réel et ne peut lever un refus impératif. |

Choisir et tracer le moteur indépendamment de Codex/Claude et du SSO ai-guard. Ne pas changer de moteur silencieusement après refus, panne ou expiration. Tous conservent autorisation centrale, isolation tenant, audit obligatoire et confinement runtime. Voir [IA locale](../local-ai/README.fr.md) pour préparation et limites de ressources, et [entreprise](../enterprise/README.fr.md) pour inférence interne et déploiement hors ligne.

Le sélecteur du gateway est `--jev-backend typesafe|rust-rules|local-openai`. Le flag explicite prime sur `JEV_BACKEND` ; le CLI ne lit pas de fichier dotenv. Sans sélecteur, une URL Jev historique choisit TypeSafe ; sinon la politique Rust obligatoire fonctionne seule. Le backend supplémentaire `rust-rules` est un catalogue de veto fini, appliqué après cette politique. Des paramètres d'un autre backend sont refusés ; retirer les anciennes variables Jev avant de changer de mode.

| Backend | Options du gateway | Résultat et provenance |
|---|---|---|
| `typesafe` | `--jev-url`, `--jev-model`, `--jev-key-file` ; `--jev-min-confidence` facultatif (défaut `0.85`) | Score et modèle déclarés par le fournisseur, modèle demandé distinct et compteurs de tokens |
| `rust-rules` | `--jev-rules-catalog` facultatif, fichier JSON privé | Catalogue argv exact ou structure de bootstrap géré validée ; inconnu soumis à revue ; aucun score ni compteur de tokens |
| `local-openai` | `--jev-local-url`, `--jev-local-model` ; `--jev-local-key-file` distinct et facultatif | Avis structuré ; un Allow fournisseur reste Review effectif et bloque l'exécution ; aucune confiance calibrée |

L'URL locale est la route **Chat Completions** complète, par exemple `http://127.0.0.1:18090/v1/chat/completions`. `--jev-allow-local-http` autorise explicitement HTTP loopback ; les autres destinations exigent HTTPS vérifié. Requêtes masquées et réponses sont bornées à 65 536 octets sérialisés, sans troncature silencieuse de l'entrée. Les délais de connexion et de requête complète sont de 2 et 5 secondes par défaut. Seul `local-openai` accepte `--jev-local-timeout-seconds` de 1 à 12. Les redirections et proxies ambiants sont désactivés. Panne, réponse invalide et incertitude du juge bloquent l'action protégée.

Pour TypeSafe ou local-openai HTTPS avec AC privée, configurer `gateway --jev-ca-file /chemin/absolu/ac-publique.pem` (ou `JEV_CA_FILE`, le flag prime). Seul le bundle public explicite est approuvé dans ce mode ; sans lui, les racines WebPKI embarquées existantes s'appliquent, pas le magasin OS. Chaîne, SAN et validité restent vérifiés. Bundle vide/clé privée/malformé/trop grand, catalogue Rust+AC, HTTP+AC et opt-in HTTP+AC sont refusés avant ouverture des credentials. L'empreinte publique d'AC participe à la provenance de configuration, sans prouver un handshake réussi. Sa rotation exige un redémarrage du composant. Le [guide TLS conteneurisé](../local-ai/TLS.fr.md) fournit projections et diagnostic exacts. Cette option configure le juge du gateway, pas Codex/Claude, OIDC ou tous les services.

Les nouvelles décisions conservent backend résolu, source de sélection et empreinte de configuration non secrète. Les replays historiques gardent leur décision et provenance initiales. Scores, modèles et tokens restent des données déclarées par le fournisseur. Les [types officiels de réponse TypeSafe](https://github.com/typesafe-ai/typesafe-sdk-js/blob/main/src/types.ts) acceptent null pour les compteurs entrée/sortie ; une inconnue ne devient jamais zéro.

Pour le catalogue Rust intégré, lancer la pile conteneurisée avec l'[exemple rules](../../../.env.exemple.local-rules) et la surcharge explicite :

```sh
docker compose --env-file .env.exemple.local-rules -f deploy/docker/compose.yaml -f deploy/docker/compose.jev-rules.example.yaml up -d
```

Garder son répertoire de configuration identique à celui passé à `init`. Avec Podman, utiliser `podman compose` et `deploy/podman/compose.yaml`, en conservant la même surcharge rules. Aucun modèle, montage de credential ou réseau sortant n'est ajouté. Les commandes exactes intégrées sont `/usr/bin/id`, `/usr/bin/true` et `/bin/pwd`, sans argument. Le bootstrap géré Codex/Claude n'est reconnu qu'après validation complète résolution/argv/mode/modèle et politique obligatoire. Cela valide la structure d'invocation, pas le prompt ni les actions transitives.

Choisir une seule surcharge de juge. Pour passer du catalogue Rust à TypeSafe ou inversement, remplacer ce fichier dans la commande Compose ; leur combinaison conserve des paramètres incompatibles et est refusée.

Un catalogue personnalisé suit cette forme versionnée :

```json
{"version":1,"allow_argv":[["/usr/bin/id"],["/usr/bin/true"],["/bin/pwd"]],"deny_argv":[]}
```

Utiliser un fichier existant privé (`0600`, ascendance sûre), référencé explicitement par `--jev-rules-catalog`. Il est retenu puis revalidé. Garder ses entrées non secrètes : au plus 128 entrées, 32 arguments et 8 192 octets par entrée, dans un fichier de 65 536 octets. Champs inconnus, entrées dupliquées/contradictoires, exécutables relatifs et caractères de contrôle sont refusés. La correspondance est exacte, sans expansion shell, préfixe ou compréhension sémantique. Aucun match ne lève la politique obligatoire.

Pour Jev réel, créer un nouveau fichier de clé brute privé `<config-dir>/secrets/jev`, mode `0600`. Y placer **uniquement la clé brute** dans un éditeur, sans `JEV_API_KEY=`. Ne pas la placer dans `providers.env`. Actualiser la projection de contrôle :

```sh
"$AIGUARD_BIN" init --config-dir "$AIGUARD_CONFIG"
```

Dans le fichier env Compose privé, ajouter les réglages non secrets `JEV_URL=https://api.typesafe.ai/v1/systemone` et `JEV_MODEL=jev-latest`. Redémarrer avec la surcharge explicite en gardant l'identité de la pile :

```sh
docker compose --env-file "$AIGUARD_CONFIG/local.env" -f deploy/docker/compose.yaml -f deploy/docker/compose.jev.example.yaml up -d --force-recreate
```

Avec Podman, remplacer `docker compose` et le fichier de base par `podman compose` et `deploy/podman/compose.yaml` ; le chemin de surcharge Jev reste identique. Celle-ci monte la projection Jev uniquement dans le gateway et ajoute une **connectivité sortante non filtrée à ce service**. Examiner ce choix réseau séparément de celui des workers. Le CLI `up` ne charge pas automatiquement la surcharge. La conserver à chaque démarrage de pile qui nécessite Jev.

L'adaptateur utilise l'[API System One](https://docs.typesafe.ai/api/system-one) authentifiée et une [question Choice](https://docs.typesafe.ai/primitives/choice) pour `allow`, `deny` ou `review`. `jev-latest` est un alias évolutif : conserver le modèle réellement retourné avec le résultat. Un score de décision n'est pas une probabilité de sûreté calibrée. Une panne d'audit ou de décision bloque l'action protégée ; retirer une clé ne constitue pas un repli automatique sûr vers un autre moteur.

## Suivre l'activité du fournisseur en direct

ai-guard collecte ce que Codex CLI et Claude Code exposent officiellement pendant la session. La livraison en direct au journal du dashboard nécessite un gateway en mode legacy et un runner autorisé. Un gateway en mode owned renvoie `503 identity_unavailable` avant de lire le corps du lot. Le transmetteur laisse alors le lot en attente dans le spool local. Tout ce qu'un client rapporte reste **déclaré**. ai-guard consigne séparément ses observations sur le cycle de vie du conteneur, le code de sortie, les changements de l'espace de travail, les décisions et les canaux de collecte. [Contrat de livraison des lots](../../api.md#provider-activity-event-batch).

- **Sessions batch.** Sous le broker, la sortie structurée du client (`claude --output-format stream-json`, `codex exec --json`) est lue enregistrement par enregistrement, au fil de l'eau : messages visibles, résumés de raisonnement, appels d'outils et résultats, commandes (comme texte d'affichage, jamais comme argv exécutable), changements de fichiers, plans et délégations à des sous-agents. Un lancement géré hors broker (`run`) collecte ses hooks en direct mais normalise encore sa sortie structurée après l'exécution.
- **Sessions batch et interactives.** L'image du worker installe des configurations de hooks gérées sur sa racine en lecture seule : `/etc/claude-code/managed-settings.json` (avec `allowManagedHooksOnly`) et `/etc/codex/requirements.toml` (avec `allow_managed_hooks_only`). Les hooks appellent `/usr/local/bin/ai-guard-activity`, qui remet chaque charge au broker par un canal `exec` dédié. Ce canal ne transporte aucun credential et n'utilise pas le réseau. Un `.claude/settings.json` ou un `.codex/config.toml` de l'espace de travail ne peut ni ajouter ni retirer ces hooks. Le bootstrap refuse de démarrer un client dont la configuration gérée manque ou a été modifiée (`activity_unavailable`).
- **Blocage en cas de perte d'audit.** `PreToolUse` et `UserPromptSubmit` ne sont acquittés qu'une fois leur enregistrement durable dans le spool du broker. Si le spool est indisponible, si l'arriéré non livré dépasse sa borne ou si le budget de la session est épuisé, ces hooks refusent l'action, et la lecture du stdout d'une session batch est suspendue pour une durée bornée. Cela vaut lorsque le client de hook s'exécute. Claude Code 2.1.295 et suivants bloquent aussi lorsque le hook lui-même échoue (`onFailure: "block"`) ; avec Claude Code 2.1.140 et Codex, un hook qui plante ou expire laisse l'outil s'exécuter.
- **Budgets et pertes.** Par défaut, chaque session conserve au plus 1 000 événements et 2 Mio, avec un arriéré non livré de 200 événements ou 512 Kio. Le champ `provider_activity` (`max_events`, `max_bytes`) du registre du broker les relève jusqu'à 5 000 événements et 8 Mio ; l'arriéré suit au cinquième des événements et au quart des octets. Toute perte est explicite : événements `activity.loss`, puis un bilan `activity.coverage` en fin de session avec ses compteurs par canal.
- **Dimensionner l'audit du tenant.** La rétention est une opération hors ligne pour les Stores SQLite au format 13 accessibles depuis l'hôte. La rotation automatique reste indisponible. Prévoir au moins 50 sessions avant d'atteindre l'une des limites de l'audit, en événements ou en octets. Les limites par défaut de 100 000 événements et 256 Mio permettent 100 sessions à 1 000 événements chacune ou 128 à 2 Mio chacune. `ai-guard-diagnose` avertit en dessous de 50. [Rétention de l'audit et déploiements pris en charge](../audit-retention/README.fr.md).
- **Sous-agents.** Un `spawn_agent` Codex nomme le fil parent. Un lien explicite peut toujours être résolu après rétention SQLite hors ligne si les deux fiches d'agent sont disponibles au lecteur. Les hooks Claude déclarent les sous-agents. Le lecteur legacy SQLite peut inférer un parent si l'appel nomme son émetteur et si les deux événements concordent par ID d'appel exact dans une même session fournisseur. Les deux doivent être relus après la borne de rétention, et l'enfant doit y être déclaré pour la première fois. Une preuve manquante, des sessions fournisseur contradictoires dans le même contexte ou un dépassement du budget de lecture laissent le parent inconnu, sauf si un lien explicite est résolu. L'agent reste déclaré. [Hiérarchie des sous-agents après rétention](../../adapters.md#reprise-de-la-hiérarchie-après-rétention-sqlite).

| Capacité | Claude Code 2.1.140 | Claude Code 2.1.329 | Codex CLI 0.165.0 / 0.170.0 |
|---|---|---|---|
| Messages, outils et commandes en batch | Implémenté, non qualifié | Implémenté, non qualifié | Fil principal seulement ; sous-agents par les hooks |
| Activité interactive (TTY) | Hooks : consignes, outils, sous-agents, dernier message de chaque tour | Idem | Idem ; recherche web hébergée non couverte |
| Blocage si l'audit est indisponible | Quand le client de hook s'exécute | Aussi si le hook échoue | Quand le client de hook s'exécute |

« Non qualifié » signifie des parseurs testés sur fixtures, sans session authentifiée pour cette version. Une version absente de ce tableau est inconnue. Aucune ligne n'est une médiation : le client et ses hooks s'exécutent dans le worker non fiable, sous le même utilisateur que l'agent. Non couverts : OpenTelemetry, transcriptions et fichiers rollout des clients, app-server de Codex et actions qu'un agent effectue hors des points de hook du client. Voir le [contrat des adaptateurs](../../adapters.md#collecte-live) pour les correspondances et limites des enregistrements.

## Diagnostiquer les problèmes fournisseur

Avant de lancer, reprendre les mêmes sélecteurs de client, de mode d’authentification, de profil et de modèle avec `providers check`. Ajouter `--interactive` pour une session TTY et reprendre `--providers-file` si un fichier explicite a été choisi.

```sh
"$AIGUARD_BIN" providers check --config-dir "$AIGUARD_CONFIG" \
  --client claude-code --auth-mode api --client-profile claude-api \
  --model claude-haiku-5-5 --json
```

Si `check` s’arrête avant de produire un rapport, lire la première ligne d’erreur. Pour le fichier `providers.json` par défaut, utiliser `providers init` pour créer les profils API manquants, ou vérifier le propriétaire et les permissions si le fichier existe. Avec `--providers-file`, vérifier le fichier sélectionné et préserver les profils existants. Une fois le rapport produit, `credential_status` décrit la référence de credential du profil résolu.

Le rapport affiche le choix résolu et `credential_status`. `present_unverified` indique un fichier régulier à lien unique, possédé par votre compte et de mode `0600`. `absent` indique une entrée manquante sous un répertoire vérifié. Pour `suspect`, vérifier la chaîne de répertoires, le propriétaire, le mode et les liens. Ce contrôle hors ligne examine uniquement les métadonnées de la référence. L’accès au compte, le contenu du credential et les capacités du client dans le worker restent à vérifier.

| Résultat | Action |
|---|---|
| Client absent ou incompatible | Reconstruire le worker approuvé avec une version exacte compatible et vérifier l'image résolue. Un client hôte n'installe pas celui du worker. |
| Credential absent, dangereux ou connexion requise | Vérifier fichier explicitement référencé, propriétaire, permissions privées et mode choisi. Suivre la connexion officielle ou renouveler la clé API sélectionnée. Ne jamais substituer un autre mode silencieusement. |
| Modèle indisponible ou compte refusé | Vérifier modèle exact et droits du compte auprès du fournisseur ; faire un nouveau choix explicite et demander une nouvelle autorisation. |
| Quota abonnement épuisé | Suivre la [procédure API explicite](#quota-abonnement-épuisé) seulement si vous choisissez sa facturation et ses prérequis distincts. |
| Auto ambigu | Choisir explicitement un profil ou définir défaut/priorité compatibles dans une nouvelle configuration relue. |
| Erreur réseau ou TLS | Vérifier destination approuvée et chaîne publique d'AC de confiance. Ne pas désactiver TLS ni ouvrir toutes les destinations workers pour réparer un certificat. |
| Délai dépassé ou sortie partielle | Examiner l'état session/runtime avant de réessayer. Conserver tentative et incertitude ; une nouvelle requête API peut être facturée. |
| Jev indisponible | Examiner configuration du service de décision et sortie réseau du contrôle. Conserver le refus ; ne pas lancer sur l'hôte ni changer implicitement de moteur. |

Après une tâche, utiliser `worker-status`, l'audit et la [procédure d'export vers une destination neuve](../local/README.fr.md#examiner-et-exporter-les-changements). Un code de sortie client ou un texte généré ne prouve pas tous les changements demandés. Les [événements fournisseur](../../../src/adapters.rs) ont `provenance=declared` ; leur `coverage=observed` décrit la capture d'une déclaration, pas une observation indépendante de son effet. L'autorisation du lancement ne prouve pas une médiation avant les commandes imbriquées ou les actions Web/MCP distantes ; voir [audit et couverture](../../operations.md#audit-sorties-et-couverture). Les observations runtime gardent leur propre provenance, et une trace d'audit durable ne transforme pas une déclaration en effet observé. Conserver les indicateurs d'omission/troncature au lieu de présenter une capture bornée comme une transcription complète.

Arrêter les sessions actives par `worker-stop` et confirmer l'état terminal observé avant d'arrêter les services de contrôle. Révoquer ou renouveler les credentials auprès de leur fournisseur si nécessaire. Conserver les bindings de session avec la même confidentialité que les credentials ; ne pas les effacer pour fabriquer une reprise propre.

## Rejouer la recette CLI et Chromium

Depuis la racine du dépôt, utiliser Python 3.11 ou plus sur un système Unix avec un pseudo-terminal. Le premier test vérifie les refus de prévol et leur restitution côté navigateur avec des fixtures. Le second conduit deux tours dans un pseudo-terminal avec un programme synthétique et contrôle la restauration du terminal. Prévoir au moins une minute pour les deux.

```sh
python3 -B tests/e2e/test_run_provider_preflight.py ProviderPreflightTests ProviderBrowserStageTests
python3 -IB tests/e2e/interactive_driver.py --self-test
```

Construire `agentsandbox` et `ai-guard-diagnose` depuis le même checkout. Pour le prévol suivant, placer les deux binaires côte à côte et renseigner `QA_BIN` avec le chemin absolu d’`agentsandbox`. Le test exerce les huit combinaisons client, authentification et transport avec des références de recette présentes puis absentes.

```sh
cargo build --locked --bin agentsandbox --bin ai-guard-diagnose
QA_BIN="$PWD/target/debug/agentsandbox"
AI_GUARD_QA_PROVIDER_CLI="$QA_BIN" \
  python3 -IB tests/e2e/test_run_provider_preflight.py InstalledDiagnosticTests -v
```

Pour un essai fournisseur réel, préparer une pile de recette avec gateway et dashboard sur boucle locale, le bundle du même checkout, Node, Playwright et un Chromium utilisable. Les versions des clients des essais de référence sont Codex `0.170.0` et Claude Code `2.1.329`. Construire l’image avec ces versions exactes et renseigner son ID local immuable. Utiliser un fichier privé de profils et le mode d’authentification réellement configuré. Le dossier de preuves indiqué doit être neuf et son parent doit exister.

```sh
umask 077
python3 -B tests/e2e/run_provider_case.py \
  --bin "$QA_BIN" --config-dir "$QA_CONFIG" \
  --gateway "$QA_GATEWAY" --dashboard "$QA_DASHBOARD" \
  --runtime docker --runtime-endpoint "$QA_RUNTIME_ENDPOINT" --image "$QA_WORKER_IMAGE_ID" \
  --client claude-code --auth-mode api --client-profile "$QA_CLAUDE_API_PROFILE" \
  --providers-file "$QA_PROVIDERS_FILE" --transport batch \
  --evidence-dir "$QA_EVIDENCE" --worker-provenance
```

Le harnais sélectionne `claude-haiku-5-5` pour Claude Code et `gpt-6-luna` pour Codex. Utiliser `--client codex` avec son profil dédié. `--auth-mode subscription` exige le profil d’abonnement et sa connexion fournisseur. Utiliser `--transport tty` sur une configuration de recette réservée à cet essai. Pour Podman, utiliser `--runtime podman --runtime-endpoint local`. Ajouter `--allow-rootful-runtime` selon l’exception choisie dans le guide local. Si Playwright est installé à part, `CODEX_PRIMARY_RUNTIME_NODE_MODULES` indique son répertoire `node_modules` absolu. `PLAYWRIGHT_CHROMIUM_EXECUTABLE` permet de désigner un Chromium local.

Conserver `result.json`, la provenance du worker et la preuve du dashboard pour chaque combinaison. En cas d’échec navigateur, conserver le HTML et le PNG assainis quand la capture est disponible. [Installation locale de recette](../local/README.fr.md) · [Suivi des validations](../../engineering/TEST_STRATEGY.md)
