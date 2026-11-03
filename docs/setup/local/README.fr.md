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

Le fonctionnement normal exige un moteur rootless. Pour utiliser délibérément Docker Desktop ou un daemon qui ne déclare pas ce mode, ajouter `--allow-rootful-runtime` à `doctor`, `build` et à chaque `run`. Cette exception explicite est tracée comme une réduction de sûreté runtime ; une VM ne démontre pas une isolation équivalente à rootless. Ne pas fournir de socket moteur ni de mode privilégié aux conteneurs applicatifs.

L'image worker par défaut permet les commandes ordinaires. Pour Codex ou Claude Code, construire les versions des clients décrites dans le [guide fournisseurs](../providers/README.fr.md) avant une session IA.

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

`AI_GUARD_GATEWAY_PORT` et `AI_GUARD_DASHBOARD_PORT` modifient les ports hôte, toujours liés à `127.0.0.1`. Les ports internes restent `8787` et `4000`. Après un changement, adapter aussi `AIGUARD_GATEWAY` et `AIGUARD_DASHBOARD` dans le shell. Le CLI Rust ne lit pas ce fichier dotenv. Ne jamais le charger par `source` ; le passer explicitement à Compose. Les variables du shell peuvent prendre priorité sur l'interpolation Compose : chercher une valeur héritée lorsqu'un réglage paraît incorrect.

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

Conserver le même compte moteur, contexte/endpoint, fichier env et fichier Compose. Utiliser ce parcours Compose direct pour les ports ou noms de projet personnalisés ; le parcours CLI `up/down` possède son propre projet fixe et ses URL de disponibilité par défaut.

### Bornes du relais HTTP

L'ingress Rust dispose de budgets indépendants pour **16 corps de requête en cours de lecture** et **64 échanges avec le service amont**. Chaque corps est limité à **256 Kio**, **5 secondes au total** et **2 secondes sans nouveau fragment**, à partir de sa lecture. Le corps complet est lu et son permis de lecture libéré avant de réserver un échange. Un corps connu vide par son état de flux passe sans permis de lecture ; la méthode HTTP ou le `Content-Length` déclaré ne décide pas de ce passage. Les routes, destinations fixes et contrôles d'authentification/autorisation amont restent identiques. Ces bornes sont fixées dans le binaire ; le fichier dotenv et le CLI ne proposent aucun réglage pour les modifier.

Un budget de lecture plein peut répondre **429** à une requête avec corps, tandis que les lectures de contrôle sans corps peuvent encore utiliser le budget distinct des échanges. Des requêtes complètes ou des flux SSE peuvent encore saturer ce dernier. Cette séparation ne fournit pas de protection générale contre les floods et ne qualifie pas l'isolation runtime. Le runner demande toujours un arrêt ciblé du worker sur erreur de suivi du gateway, dans la limite de la disponibilité du moteur ; aucune fenêtre de réessai de 60 secondes n'est activée. Conserver la session enregistrée et la réconcilier après une interruption plutôt que lancer une autre instance pour masquer un résultat inconnu.

## Ouvrir la vue de contrôle

Ouvrir `AIGUARD_DASHBOARD` dans le navigateur. L'URL gateway sert au CLI et aux API ; `/healthz` indique la disponibilité du service, pas l'autorisation de lancer un worker ni l'authentification auprès d'un fournisseur IA.

Utiliser son compte ai-guard nominatif pour la vue personnelle et sélectionner explicitement un périmètre d'administration lorsqu'on y est autorisé. L'identité locale fonctionne sans IdP. [Keycloak/OIDC](../keycloak/README.fr.md) est un choix d'identité indépendant du fournisseur IA. La vue personnelle comprend ses propres sessions et les ressources explicitement partagées. L'appartenance au même tenant ne doit pas révéler les prompts, sorties ou fichiers d'un autre utilisateur.

Séparer les credentials techniques de cette identité : `viewer` donne son périmètre de lecture configuré et `runner` appartient au contrôle de confiance. Pour une connexion par clé viewer, consulter sa source dans un éditeur privé et saisir la valeur uniquement dans le formulaire de connexion de confiance. Ne jamais placer le credential runner dans un navigateur, un worker ou une conversation. Les événements anciens identifiés seulement par une clé partagée conservent cette provenance ; une connexion ultérieure ne prouve pas leur auteur humain.

Le dashboard présente provenance des sessions, décisions, observations runtime et changements. Connexion périmée, couverture partielle, refus et résultat vide sont des états distincts. Utiliser la dernière observation et le périmètre affichés pour choisir entre actualisation, reconnexion et demande d'accès.

Ouvrir un agent ou une sandbox pour consulter ses workers explicitement liés, puis un worker pour retrouver ses agents et sandboxes correspondants. Les liens utilisent les identifiants exacts de session ou de runtime reçus pour le tenant authentifié ; leur provenance est affichée et une session identique ne prouve ni exécution ni délégation. Le registre workers est paginé : parcourir ses pages avant de conclure qu'aucun enregistrement correspondant n'a été reçu. Une lecture échouée conserve un résultat périmé avec une action de reprise ; ce n'est pas un inventaire vide. Les liens rapprochent projections d'audit et registre, sans interroger le moteur ni découvrir des brokers distants.

Un refus 401/403 d'une lecture active ferme la session viewer et retire ses données, y compris les détails ouverts. Se reconnecter avec un credential autorisé ; des actualisations répétées ne rétablissent pas un credential refusé. Une erreur du service est affichée explicitement. Les vues workers, quotas et allocations conservent leurs dernières données réussies avec un avertissement de péremption, dans la même session et le même périmètre, jusqu'à une actualisation réussie.

## Exécuter une première commande

Commencer par une petite commande hors ligne. Elle utilise la politique stricte et coupe le réseau du worker :

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

## Examiner et exporter les changements

Sélectionner la session qui a créé le fichier et utiliser son UUID. Le parent d'export doit exister et la destination doit être **neuve**, hors du projet original. Ne pas exporter dans une copie de travail existante.

```sh
AIGUARD_SESSION="replace-with-the-file-session-uuid"
AIGUARD_EXPORT="$HOME/ai-guard-export-new"
"$AIGUARD_BIN" export-workspace --session "$AIGUARD_SESSION" --output "$AIGUARD_EXPORT" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY"
```

Examiner les fichiers exportés et leur diff avant de reporter des changements choisis dans le projet. ai-guard ne réécrit pas l'original automatiquement. Un export en erreur n'est pas un diff vide ; si une destination a été publiée avant une erreur ultérieure, l'examiner et la conserver avant de choisir une autre destination neuve.

L'export omet les noms sensibles et de cache déjà exclus à l'import, ainsi que `.husky`, `.envrc`, `.idea` et `.vscode/tasks.json` ; le contrôle s'applique aux composants du chemin sans distinction de casse ASCII. La configuration et les hooks `.git` produits par le worker ne sont donc pas restaurés. Les fichiers réguliers exportés sont en `0644`, même si le worker les avait rendus exécutables. Le manifeste d'export décrit les fichiers effectivement écrits ; le scan brut du workspace reste distinct. Les entrées ignorées restent soumises aux contrôles de chemin, type, doublons et taille : le filtrage ne rend pas une archive invalide acceptable.

Le contrôle émet un avertissement générique avec le nombre d'entrées omises, sans exposer leurs noms. Si cet avertissement ne peut pas être écrit, la publication est refusée. **Le contenu exporté reste non fiable.** Cette liste ne neutralise pas la configuration de tous les outils et ne détecte pas tous les secrets. Examiner les fichiers comme du texte avant de les ouvrir avec un outil qui peut exécuter une configuration de projet, d'installer des dépendances ou de rétablir volontairement des permissions exécutables.

La copie exclut des noms sensibles connus et des répertoires de dépendances/cache, sans constituer un détecteur universel de secrets. Examiner le contenu source avant import et garder les clés à l'extérieur. Les bornes comprennent 20 000 entrées, 128 Mio au total et 32 Mio par fichier ; liens et chemins dangereux sont refusés. Un dépassement exige de réduire l'entrée ou de faire évoluer une limite approuvée, pas de traiter silencieusement une copie partielle comme complète.

## Diagnostiquer un refus ou une interruption

| Symptôme | Vérification et action |
|---|---|
| Moteur absent, exécutable dangereux ou mauvais endpoint | Lancer `doctor` sous le même compte et répéter les choix explicites `--runtime-program`/`--runtime-endpoint` sur les commandes qui les acceptent. Ne pas exécuter la tâche sur l'hôte en repli. |
| Daemon non rootless refusé | Configurer rootless ou appliquer volontairement l'exception par commande décrite plus haut. |
| Pile indisponible | Consulter Compose `ps` et `logs --tail 100 gateway dashboard ingress` avec la même configuration. Vérifier collisions de ports et projections privées des credentials. Examiner les logs avant toute publication pour exclure les secrets. |
| HTTP 429 via l'ingress | Distinguer les [budgets de lecture/échange](#bornes-du-relais-http) des quotas amont à l'aide du message retourné. Réduire les requêtes parallèles et réessayer une lecture lorsque la capacité est disponible. Vérifier la session enregistrée avant de répéter un lancement. |
| HTTP 408 ou 413 pendant l'envoi d'un corps | Vérifier les délais de 5 secondes au total/2 secondes sans fragment et la limite de 256 Kio. Réduire le contenu ou corriger l'émetteur bloqué ; modifier le fichier dotenv ne relève pas ces bornes. |
| Non authentifié ou interdit | Se reconnecter avec l'identité et le tenant/périmètre autorisés. Ne pas remplacer une permission utilisateur manquante par une clé runner. |
| Refus de politique ou review | Lire le motif et le périmètre demandé. Adapter la demande ou obtenir une approbation pour cette demande exacte. |
| Copie ou export refusé | Vérifier taille, liens, nouveauté de la destination et permissions des répertoires. Préserver l'original et tout export déjà publié. |
| Runtime indisponible ou état inconnu | Conserver session, plans et volume. Consulter le statut et réconcilier l'autorité runtime enregistrée ; ne pas créer un autre démarrage pour masquer l'incertitude. |
| Connexion fournisseur ou client indisponible | Suivre le [diagnostic fournisseur](../providers/README.fr.md#diagnostiquer-les-problèmes-fournisseur) ; les contrôles de disponibilité ne valident pas cet accès. |

La reprise ordinaire utilise l'autorité moteur enregistrée pour la session :

```sh
"$AIGUARD_BIN" worker-status --session "$AIGUARD_SESSION" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY"
"$AIGUARD_BIN" worker-resume --session "$AIGUARD_SESSION" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY" --wait-seconds 30
```

La reprise réconcilie une session interrompue ; elle n'accorde pas une deuxième utilisation d'un claim de démarrage consommé. Admission interactive et attachement du terminal ont leurs propres exigences de continuité. Si l'autorité conservée demande un exécutable explicite, utiliser le chemin `--runtime-program` approuvé ; `--adopt-runtime-program` lie délibérément une autorité historique et ne constitue pas une option générique de relance.

## Arrêter, conserver les données et redémarrer

Demander l'arrêt de chaque worker actif avant celui de la pile de contrôle :

```sh
"$AIGUARD_BIN" worker-stop --session "$AIGUARD_SESSION" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY"
"$AIGUARD_BIN" worker-status --session "$AIGUARD_SESSION" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY"
docker compose --env-file "$AIGUARD_CONFIG/local.env" -f deploy/docker/compose.yaml stop
```

Une demande d'arrêt durable ne prouve pas l'état runtime terminal. Confirmer l'observation ou conserver l'état inconnu pour réconciliation. Avec Podman, utiliser la commande et le fichier Compose correspondants. Redémarrer la pile conservée par la même commande `up -d`, puis contrôler à nouveau disponibilité et sessions.

`stop` conserve conteneurs et volumes nommés. `down` sans `-v` retire conteneurs/réseaux de la pile en conservant l'audit nommé ; aucune de ces opérations ne remplace l'arrêt des workers. Ne pas utiliser `down -v`, `prune`, un autre nom de projet ou l'effacement d'une base pour réparer une erreur. Conserver ensemble `<config-dir>` (credentials, plans, bindings et état de session), volume d'audit de la pile et volumes workers retenus. Sauvegarder SQLite de façon cohérente à l'arrêt, avec l'état WAL pertinent ; ne pas copier une base active comme un fichier ordinaire. Consulter l'[exploitation entreprise](../enterprise/README.fr.md#exploiter-et-reprendre) avant migration ou restauration.
