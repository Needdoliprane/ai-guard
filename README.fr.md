# ai-guard

[English](README.md) · [Français](README.fr.md)

Exécuter Codex, Claude Code et d'autres agents IA dans des copies de travail isolées. Décider de leurs permissions, suivre leur activité dans un dashboard unique, examiner les changements et exporter les résultats vers une destination neuve.

ai-guard associe un contrôle et un broker Rust, un dashboard TypeScript statique et des workers Docker/Podman. Aucun compte cloud n'est nécessaire en local. Chaque tenant possède un périmètre authentifié et une base SQLite distincte ; les workers ne reçoivent ni socket du moteur de conteneurs ni projet original.

## Choisir son installation

Le manuel utilisateur décrit le **contrat du produit complet**. L'avancement, les résultats de qualification et les écarts restent dans le [suivi d'ingénierie](docs/engineering/CURRENT_STATE.md). Une cible documentée ne constitue pas une certification de production.

| Contexte | Guide | Exemple de configuration |
|---|---|---|
| Poste local, Docker ou Podman | [Installation locale](docs/setup/local/README.fr.md) | [.env.exemple.local](.env.exemple.local) |
| Codex / Claude Code, abonnement ou API | [Fournisseurs IA](docs/setup/providers/README.fr.md) | [.env.exemple.providers](.env.exemple.providers) |
| Keycloak local, identité Web et CLI | [Keycloak et OIDC](docs/setup/keycloak/README.fr.md) | [.env.exemple.local-keycloak](.env.exemple.local-keycloak) |
| Petit modèle local, inférence hors ligne | [IA locale](docs/setup/local-ai/README.fr.md) | [.env.exemple.local-qwen](.env.exemple.local-qwen) |
| Keycloak et Qwen locaux ensemble | [Combiner les contextes](docs/setup/README.fr.md#combiner-les-contextes-locaux) | [.env.exemple.local-keycloak-qwen](.env.exemple.local-keycloak-qwen) |
| Installation administrée, AC privée et inférence interne | [Configuration entreprise](docs/setup/enterprise/README.fr.md) | [.env.exemple.entreprise](.env.exemple.entreprise) |

Commencer par l'[index de configuration](docs/setup/README.fr.md) : composant qui lit chaque fichier, combinaison des réglages, emplacement des secrets et conservation d'une installation existante.

## Premier démarrage local

Depuis la racine du dépôt, avec la toolchain de `rust-toolchain.toml`, un moteur Docker démarré et Compose :

```sh
cargo build --release --locked
./target/release/agentsandbox init --config-dir "$HOME/.ai-guard"
./target/release/agentsandbox doctor --runtime docker
./target/release/agentsandbox build --runtime docker
docker compose --env-file .env.exemple.local -f deploy/docker/compose.yaml up -d
./target/release/agentsandbox healthcheck --url http://127.0.0.1:8787/healthz --url http://127.0.0.1:4000/healthz
```

Ouvrir `http://127.0.0.1:4000`. Pour le parcours local par clé de contrôle, utiliser le credential viewer privé créé dans `$HOME/.ai-guard/secrets/viewer` ; conserver le credential runner dans le contrôle de confiance. Le [guide local](docs/setup/local/README.fr.md) explique première commande, refus de politique, export, données, arrêt et reprise.

Le mode rootless est exigé par défaut. Docker Desktop et les moteurs qui ne déclarent pas ce mode demandent un choix explicite de l'opérateur avec `--allow-rootful-runtime` sur les commandes CLI concernées. Le guide couvre Podman, chemins de moteur, ports personnalisés et cette exception de sécurité tracée.

## Configuration et exploitation

- Les exemples contiennent uniquement des valeurs publiques. Copier le modèle vers un **nouveau** fichier privé avant de l'adapter ; conserver credentials et données existants.
- Compose lit les `.env.exemple.*` via `--env-file`. La CLI Rust ne source pas dotenv ; chaque guide précise ses options et fichiers typés.
- Construire les images depuis les Dockerfiles fournis. Après préparation des dépendances et images, les parcours locaux peuvent fonctionner hors ligne.
- Séparer identité, authentification du fournisseur IA, permissions réseau et permissions tenant. Un abonnement fournisseur ne connecte pas l'utilisateur à ai-guard.
- Le temps d'allocation de ressources n'est pas une facture de CPU consommé. Un avis modèle ne lève aucun refus obligatoire de politique, d'audit ou de runtime.

[Exploitation](docs/operations.md) · [API](docs/api.md) · [Sécurité](docs/security.md) · [Architecture](docs/architecture.md) · [Licence](LICENSE)

## Contribuer

Le code applicatif hors frontend est en Rust. [AGENTS.md](AGENTS.md) précise architecture, règles et vérifications ciblées. Un changement de comportement met à jour les guides anglais/français et les exemples dans le même lot. Les essais auxiliaires Keycloak, Qwen et AC de test restent locaux et ignorés ; les régressions de sécurité et de runtime du produit restent dans le dépôt.

Apache-2.0. Le canal privé de signalement des vulnérabilités doit être établi avant publication.
