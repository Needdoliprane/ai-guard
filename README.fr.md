# ai-guard

[English](README.md) · [Français](README.fr.md)

ai-guard est un contrôle open source, local first, pour exécuter Codex, Claude Code et d’autres commandes dans des copies de projet isolées. Demander l’autorisation de la politique, suivre l’activité dans un dashboard unique, examiner les changements et exporter les résultats vers une destination neuve.

La CLI et le broker Rust de confiance contrôlent les workers Docker/Podman. Un gateway Rust séparé porte les API et l’audit ; un dashboard Rust conteneurisé sert l’interface TypeScript statique. Chaque tenant possède son périmètre authentifié et sa base SQLite. Les workers ne reçoivent ni socket du moteur de conteneurs ni montage du projet original.

L’usage local ne demande aucun compte cloud ai-guard. L’opérateur construit et distribue ses images depuis les Dockerfiles fournis ; aucune image publique ai-guard préconstruite n’est obligatoire. L’[index du manuel](docs/setup/README.fr.md) couvre le contrat du produit complet ; l’[état d’implémentation et les preuves](docs/engineering/CURRENT_STATE.md) distinguent ce qui est livré, qualifié ou encore manquant.

## Démarrer en local

Il faut Git, la toolchain Rust sélectionnée par [rust-toolchain.toml](rust-toolchain.toml), un moteur Docker démarré avec Compose et un répertoire de projet à copier. Le mode rootless est exigé par défaut. Pour une installation Docker Desktop ou rootful délibérée, ajouter `--allow-rootful-runtime` à `doctor`, `build`, `up` et chaque `run` ; ce choix trace une dégradation de sécurité. Pour Podman, suivre le [guide local](docs/setup/local/README.fr.md) avec un moteur rootless et un fournisseur Compose compatible.

Exécuter depuis la racine de la distribution source. Remplacer `AIGUARD_PROJECT` par un chemin absolu existant. Garder la configuration privée hors de ce projet et de ce checkout. Préparer les dépendances de construction et les images avant de déconnecter une installation hors ligne.

```sh
cargo build --release --locked
AIGUARD_BIN="$PWD/target/release/agentsandbox"
AIGUARD_DIAGNOSE="$PWD/target/release/ai-guard-diagnose"
AIGUARD_CONFIG="$HOME/.ai-guard"
AIGUARD_PROJECT="/absolute/path/to/your/existing/project"
AIGUARD_GATEWAY="http://127.0.0.1:8787"
"$AIGUARD_BIN" init --config-dir "$AIGUARD_CONFIG"
"$AIGUARD_BIN" doctor --runtime docker
"$AIGUARD_BIN" build --runtime docker --source .
```

`init` crée les credentials techniques runner/viewer et conserve ceux qui existent. Leurs sources privées restent sous `secrets/` ; les projections destinées aux conteneurs sont préparées séparément. Conserver configuration, credentials et données privés. Un répertoire créé par `setup prepare` pour le SSO suit une autre disposition : utiliser son [guide SSO](docs/setup/sso/README.fr.md) au lieu de l’initialiser par ce parcours.

`build`, `up` et `down` préparent des copies privées bornées des entrées source sélectionnées avant de les remettre au moteur. Conserver le digest source affiché et le résultat réel de la commande. La [procédure de source préparée](docs/setup/local/README.fr.md#prepared-source) explique la liste fixe d’entrées, les exigences du répertoire temporaire, les refus et les copies conservées après une terminaison inconnue. Cette préparation ne prouve ni des images reproductibles ni une qualification runtime.

Avant de démarrer les services, inspecter la configuration :

```sh
"$AIGUARD_DIAGNOSE" --config-dir "$AIGUARD_CONFIG"
```

Le [diagnostic](docs/setup/diagnostics/README.fr.md) fonctionne en lecture seule et hors ligne par défaut : il ne lance rien, décrit les références de credentials par leurs métadonnées et signale ce qui n’est pas vérifié ou disponible. Il ne valide ni le contenu des credentials, ni l’accès au compte, ni l’intégrité SQLite, ni les checkpoints signés. Résoudre les erreurs et inconnues et lire les limites même lorsque le rapport affiche `OK`. Ajouter `--json` pour le rapport structuré.

```sh
"$AIGUARD_BIN" up --runtime docker --source . --config-dir "$AIGUARD_CONFIG"
"$AIGUARD_BIN" healthcheck --url "$AIGUARD_GATEWAY/healthz" --url http://127.0.0.1:4000/healthz
```

Ouvrir `http://127.0.0.1:4000`. Pour ce parcours technique viewer, lire la valeur dans `$AIGUARD_CONFIG/secrets/viewer` avec un éditeur privé et la saisir uniquement dans le formulaire de connexion de confiance. Conserver le credential runner dans le contrôle de confiance. L’accès personnel nominatif suit sa procédure d’[identité locale](docs/setup/identity/README.fr.md) ou de [SSO](docs/setup/sso/README.fr.md).

Cette pile CLI utilise le nom de projet fixe `agentsandbox` et les ports loopback par défaut ci-dessus. Conserver le même compte/contexte moteur, répertoire de configuration et source pendant son cycle de vie. Pour des ports, noms de projet, fichiers env Compose personnalisés et les logs, suivre le [guide local](docs/setup/local/README.fr.md#cli-up-stack) ; ne pas mélanger les commandes de piles différentes. Une réponse de santé établit seulement que le service est joignable.

## Lancer, suivre et examiner une session

Commencer par une petite commande sans compte fournisseur et sans accès réseau du worker. La CLI de confiance utilise son credential technique runner pour demander l’autorisation de la politique avant l’exécution :

```sh
"$AIGUARD_BIN" run --project "$AIGUARD_PROJECT" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY" --runtime docker --profile strict --network none --timeout 30 -- /usr/bin/id
```

Conserver l’UUID de session retourné. Dans le dashboard, examiner décision, observations runtime et couverture des changements de fichiers. Admission, démarrage observé et fin d’exécution sont des faits distincts ; un résultat refusé ou inconnu demande un diagnostic.

```sh
AIGUARD_SESSION="replace-with-the-returned-session-uuid"
"$AIGUARD_BIN" worker-status --session "$AIGUARD_SESSION" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY"
"$AIGUARD_BIN" verify-audit --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY"
```

`verify-audit` demande au gateway de vérifier sa chaîne de hachage locale. Les [checkpoints signés et vérifications anti-rollback avec ancre indépendante](docs/setup/audit-signatures/README.fr.md) constituent des opérations séparées. Pour modifier volontairement un fichier dans la copie de travail, suivre les [exemples de session locale](docs/setup/local/README.fr.md#exécuter-une-première-commande).

Si le suivi est interrompu, conserver la session et son volume worker, garder les services de contrôle disponibles et réconcilier cette session enregistrée :

```sh
"$AIGUARD_BIN" worker-resume --session "$AIGUARD_SESSION" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY" --wait-seconds 30
```

La reprise n’accorde pas un second usage d’un claim de démarrage consommé. Suivre la [procédure de reprise](docs/setup/local/README.fr.md#réconcilier-le-worker-enregistré) si le résultat reste inconnu ; ne pas créer une autre tâche pour le masquer.

Exporter la session sélectionnée uniquement vers une destination **neuve**, hors du projet original, dont le parent existe :

```sh
AIGUARD_EXPORT="$HOME/ai-guard-export-new"
"$AIGUARD_BIN" export-workspace --session "$AIGUARD_SESSION" --output "$AIGUARD_EXPORT" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY"
```

Examiner les fichiers exportés et leur diff avant d’appliquer soi-même des changements. Leur contenu reste non fiable ; l’inspecter avant d’ouvrir des outils qui exécutent la configuration du projet. ai-guard ne réécrit jamais automatiquement le projet original.

Avant d’arrêter la pile de contrôle, demander l’arrêt de chaque session active et vérifier son état runtime enregistré :

```sh
"$AIGUARD_BIN" worker-stop --session "$AIGUARD_SESSION" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY"
"$AIGUARD_BIN" worker-status --session "$AIGUARD_SESSION" --config-dir "$AIGUARD_CONFIG" --gateway "$AIGUARD_GATEWAY"
"$AIGUARD_BIN" down --runtime docker --source . --config-dir "$AIGUARD_CONFIG"
```

Une demande d’arrêt durable ne prouve pas à elle seule que le worker s’est arrêté. Conserver les états inconnus pour rapprochement. `down` garde les données d’audit nommées ; conserver ensemble configuration, plans de session et volumes workers. Redémarrer avec la même commande `up`, puis refaire les contrôles de santé et de session. Suivre les [procédures d’exploitation et de reprise](docs/setup/local/README.fr.md#arrêter-conserver-les-données-et-redémarrer) avant sauvegarde ou migration ; ne pas supprimer de volumes pour réparer une erreur.

## Choisir identité, IA et déploiement

L’[index de configuration](docs/setup/README.fr.md) précise les consommateurs de fichiers, les exemples publics et leurs combinaisons. Identité, authentification fournisseur, moteur de décision et permissions réseau sont des choix indépendants.

| Besoin | Guide |
|---|---|
| Docker/Podman, configuration personnalisée, première session et reprise | [Poste local](docs/setup/local/README.fr.md) |
| Diagnostiquer une configuration avant lancement | [Diagnostic opérateur](docs/setup/diagnostics/README.fr.md) |
| Codex / Claude Code, abonnement ou API | [Fournisseurs IA](docs/setup/providers/README.fr.md) |
| Comptes locaux nominatifs sans IdP | [Identité locale](docs/setup/identity/README.fr.md) |
| SSO Web/CLI ai-guard et DEMO Keycloak locale explicitement activée | [Configuration SSO](docs/setup/sso/README.fr.md) · [Fournisseur Keycloak](docs/setup/keycloak/README.fr.md) |
| Petit modèle de décision local et TLS vérifié | [IA locale](docs/setup/local-ai/README.fr.md) · [TLS](docs/setup/local-ai/TLS.fr.md) |
| Tenants administrés, AC privées, inférence interne et exigences cloud | [Configuration entreprise](docs/setup/enterprise/README.fr.md) |
| Autorité Kubernetes et inspection en lecture seule | [Kubernetes](docs/setup/kubernetes/README.fr.md) ; lancement de workers indisponible |

Pour une tâche IA, construire les versions épinglées des clients et préparer uniquement le credential du fournisseur choisi en suivant son guide. Sélectionner délibérément `--client`, `--auth-mode` et `--client-profile`. Une demande d’abonnement ne devient jamais silencieusement une requête API facturée. La connexion fournisseur reste distincte de la connexion ai-guard ; l’administration d’un IdP ne donne aucune permission ai-guard implicite. Le guide entreprise précise les limites de livraison propres à l’intégration d’un modèle de code interne et à la topologie cloud.

Les exemples ne contiennent que des valeurs publiques. Créer un **nouveau** fichier de configuration privé pour en adapter un, conserver credentials et données existants et référencer les secrets par leur chemin privé. Compose lit les exemples env via `--env-file` ; la CLI Rust ne source pas dotenv. Aucun credential dans argv, Git, captures d’écran ou workspace exporté. Un relevé d’allocation de ressources n’est pas une facture de CPU consommé ; un avis modèle ne lève aucun refus obligatoire de politique, d’audit ou de runtime.

[Exploitation](docs/operations.md) · [API](docs/api.md) · [Sécurité](docs/security.md) · [Architecture](docs/architecture.md) · [Licence](LICENSE)

## Contribuer

Le code applicatif hors frontend est en Rust. [AGENTS.md](AGENTS.md) précise architecture, règles et vérifications ciblées. Un changement de comportement met à jour les guides anglais/français et les exemples dans le même lot. Les essais auxiliaires Keycloak, Qwen et AC de test restent locaux et ignorés ; les régressions de sécurité et de runtime du produit restent dans le dépôt.

Apache-2.0. Le canal privé de signalement des vulnérabilités doit être établi avant publication.

## Guides complémentaires

- [Runtime Kubernetes V2 : mécanismes logiciels, effets produit fermés et plan de qualification cluster réel](docs/setup/kubernetes-runtime/README.fr.md).
- [Métadonnées release : manifest/verify hors ligne, provenance déclarée et limites de qualification](docs/setup/release/README.fr.md).
