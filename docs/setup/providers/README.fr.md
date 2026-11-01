# Codex, Claude Code et moteurs de décision

[English](README.md) · [Index d'installation](../README.fr.md) · [Installation locale](../local/README.fr.md)

Configurer trois choix indépendants : identité ai-guard, client de développement et son authentification abonnement/API, puis moteur de décision du contrôle. Une connexion à l'IdP ne connecte pas Codex ou Claude Code. Une clé Jev autorise un service de décision du contrôle ; elle n'appartient jamais à un fichier de credentials worker.

## Préparer les clients et une petite première tâche

Lancer la [pile locale](../local/README.fr.md) et conserver ses variables shell, son moteur et son URL gateway. Choisir un projet sans credentials privés. Sélectionner un modèle disponible pour son compte et borner la tâche et sa durée avant de commencer : une requête API peut être facturée par le fournisseur, tandis qu'un abonnement suit ses règles d'éligibilité et d'usage.

Construire le worker avec des versions exactes des clients. Ces versions sont des références explicites, pas une consigne de choisir la dernière publication :

```sh
"$AIGUARD_BIN" build --runtime docker --source . --codex-version 0.165.0 --claude-code-version 2.1.140
```

Utiliser `--runtime podman` avec Podman. Appliquer l'exception rootful du guide local uniquement si elle a été choisie pour l'installation. Ni plage, ni joker, ni dernière version implicite dans une image approuvée. Construire un client ne l'authentifie pas. Pour une autre version prise en charge, reconstruire volontairement et conserver version résolue et identité d'image avec la configuration de session.

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

## Choisir le moteur de décision

| Moteur | Usage et limites |
|---|---|
| Rust déterministe | Décisions sans modèle ni credential de modèle externe. Les règles ont une couverture sémantique incomplète ; intentions inconnues et effets transitifs doivent rester explicites. |
| Jev réel | Service de décision TypeSafe authentifié avec provenance du modèle. Son credential reste dans le contrôle. |
| Modèle local Jev-like | Petit modèle conteneurisé avec poids/runtime épinglés, sortie validée et bornée, incertitude et délais explicites. Il est distinct de Jev réel et ne peut lever un refus impératif. |

Choisir et tracer le moteur indépendamment de Codex/Claude et du SSO ai-guard. Ne pas changer de moteur silencieusement après refus, panne ou expiration. Tous conservent autorisation centrale, isolation tenant, audit obligatoire et confinement runtime. Voir [IA locale](../local-ai/README.fr.md) pour préparation et limites de ressources, et [entreprise](../enterprise/README.fr.md) pour inférence interne et déploiement hors ligne.

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

## Diagnostiquer les problèmes fournisseur

| Résultat | Action |
|---|---|
| Client absent ou incompatible | Reconstruire le worker approuvé avec une version exacte compatible et vérifier l'image résolue. Un client hôte n'installe pas celui du worker. |
| Credential absent, dangereux ou connexion requise | Vérifier fichier explicitement référencé, propriétaire, permissions privées et mode choisi. Suivre la connexion officielle ou renouveler la clé API sélectionnée. Ne jamais substituer un autre mode silencieusement. |
| Modèle indisponible ou compte refusé | Vérifier modèle exact et droits du compte auprès du fournisseur ; faire un nouveau choix explicite et demander une nouvelle autorisation. |
| Auto ambigu | Choisir explicitement un profil ou définir défaut/priorité compatibles dans une nouvelle configuration relue. |
| Erreur réseau ou TLS | Vérifier destination approuvée et chaîne publique d'AC de confiance. Ne pas désactiver TLS ni ouvrir toutes les destinations workers pour réparer un certificat. |
| Délai dépassé ou sortie partielle | Examiner l'état session/runtime avant de réessayer. Conserver tentative et incertitude ; une nouvelle requête API peut être facturée. |
| Jev indisponible | Examiner configuration du service de décision et sortie réseau du contrôle. Conserver le refus ; ne pas lancer sur l'hôte ni changer implicitement de moteur. |

Après une tâche, utiliser `worker-status`, l'audit et la [procédure d'export vers une destination neuve](../local/README.fr.md#examiner-et-exporter-les-changements). Un code de sortie client ou un texte généré ne prouve pas tous les changements demandés. Les événements fournisseur décrivent ses déclarations ; observations runtime et audit indépendant gardent leur provenance propre. Conserver les indicateurs d'omission/troncature au lieu de présenter une capture bornée comme une transcription complète.

Arrêter les sessions actives par `worker-stop` et confirmer l'état terminal observé avant d'arrêter les services de contrôle. Révoquer ou renouveler les credentials auprès de leur fournisseur si nécessaire. Conserver les bindings de session avec la même confidentialité que les credentials ; ne pas les effacer pour fabriquer une reprise propre.
