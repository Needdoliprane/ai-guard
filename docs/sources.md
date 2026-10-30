# Registre des sources officielles

Consultation documentaire : **29 octobre 2026**. Les URLs sont des sources primaires. Les versions des CLI doivent être vérifiées séparément sur la machine de déploiement. Une documentation courante ne garantit pas la présence d'une fonction dans une ancienne version.

## Codex

| ID | Source | Usage dans le projet |
|---|---|---|
| C1 | [OpenAI — Hooks](https://learn.chatgpt.com/docs/hooks) | Événements, provenance des IDs, confiance des hooks, couverture et exceptions. |
| C2 | [OpenAI — Mode non interactif](https://learn.chatgpt.com/docs/non-interactive-mode) | `codex exec --json`, flux JSONL et authentification d'automatisation. |
| C3 | [OpenAI — Types d'événements du SDK](https://github.com/openai/codex/blob/main/sdk/typescript/src/events.ts) | Formats JSONL utilisés par les fixtures Codex. |
| C4 | [OpenAI — Types d'items du SDK](https://github.com/openai/codex/blob/main/sdk/typescript/src/items.ts) | Commandes déclarées et changements de fichiers. |

## Claude Code

| ID | Source | Usage dans le projet |
|---|---|---|
| A1 | [Anthropic — Hooks reference](https://code.claude.com/docs/en/hooks) | Hooks avant/après outil, sous-agents, codes de sortie, timeout et `onFailure`. |
| A2 | [Anthropic — Run Claude Code programmatically](https://code.claude.com/docs/en/headless) | `claude -p`, formats de sortie, mode `--bare` et chargement des intégrations. |
| A3 | [Anthropic — Streaming output](https://code.claude.com/docs/en/agent-sdk/streaming-output) | Messages complets et deltas du flux Claude. |
| A4 | [Anthropic — Handle tool calls](https://platform.claude.com/docs/en/agents-and-tools/tool-use/handle-tool-calls) | Blocs `tool_use` et `tool_result`. |
| A5 | [Anthropic — Advanced setup](https://code.claude.com/docs/en/setup) | Installation npm officielle et prérequis actuels. |

## Gemini CLI

| ID | Source | Usage dans le projet |
|---|---|---|
| G1 | [Google — Hooks reference](https://geminicli.com/docs/hooks/reference/) | Contrats `BeforeTool` et `AfterTool`, refus et structure des hooks. |
| G2 | [Google — Observability with OpenTelemetry](https://geminicli.com/docs/cli/telemetry/) | Traces, métriques, événements et options de collecte. |
| G3 | [Google — Headless mode reference](https://geminicli.com/docs/cli/headless/) | Exécution sans TUI et formats de sortie. |
| G4 | [Dépôt officiel — Hook reference](https://github.com/google-gemini/gemini-cli/blob/main/docs/hooks/reference.md) | Source de la documentation et suivi de son évolution. |

## OpenCode

| ID | Source | Usage dans le projet |
|---|---|---|
| O1 | [OpenCode V1 — Plugins](https://docs.opencode.ai/docs/plugins/) | Hooks d'outils et événements V1. |
| O2 | [OpenCode V1 — Server](https://opencode.ai/docs/server) | Serveur HTTP, OpenAPI, authentification et événements. |
| O3 | [OpenCode V2 — Plugins](https://opencode.ai/v2/docs/build/plugins/) | SDK de plugins V2 et migration depuis V1. |
| O4 | [OpenCode V2 — Permissions](https://opencode.ai/v2/docs/permissions) | Règles de permissions V2 et différences de noms avec V1. |

## Aider et Continue

| ID | Source | Usage dans le projet |
|---|---|---|
| D1 | [Aider — Scripting](https://aider.chat/docs/scripting.html) | CLI `--message` et statut non stable de l'API Python. |
| D2 | [Aider — Options reference](https://aider.chat/docs/config/options.html) | Fichiers d'historique et options configurables. |
| D3 | [Aider — Analytics](https://aider.chat/docs/more/analytics.html) | Journal JSONL local des analytics, indépendamment de leur envoi. |
| N1 | [Continue — Headless Mode](https://docs.continue.dev/cli/headless-mode) | `cn -p`, JSON et comportement non interactif. |
| N2 | [Continue — Tool Permissions](https://docs.continue.dev/cli/tool-permissions) | Niveaux de permission, priorités et modes globaux. |

## TypeSafe Jev

| ID | Source | Usage dans le projet |
|---|---|---|
| J1 | [TypeSafe — Jev with coding agents](https://docs.typesafe.ai/introduction/coding-agents) | Rôle décisionnel de Jev ; absence de fonction d'exécution de commandes. |
| J2 | [TypeSafe — OpenAPI officiel](https://api.typesafe.ai/openapi.json) | Endpoint, authentification, schéma requête/réponse et découverte des modèles. |
| J3 | [TypeSafe — Choice](https://docs.typesafe.ai/primitives/choice) | Question à options et structure `answers`, `choice`, `confidence`, `probabilities`. |
| J4 | [TypeSafe — Enterprise](https://typesafe.ai/enterprise) | Options de données et question VPC/on-premises ; pas de procédure d'installation confirmée par le contenu récupéré. |
| J5 | [TypeSafe Help — Data use and retention](https://help.typesafe.ai/articles/5974925716-how-is-submitted-data-used-or-retained) | Différence entre non-entraînement, traitement et rétention contractuelle. |
| J6 | [TypeSafe — Quick start](https://docs.typesafe.ai/introduction/quickstart) | Démarrage API/SDK et exemples officiels. |

## Conteneurs et permissions

| ID | Source | Usage dans le projet |
|---|---|---|
| R1 | [Docker — Rootless](https://docs.docker.com/engine/security/rootless/) | Daemon et conteneurs non privilégiés ; choix prioritaire de déploiement. |
| R2 | [Docker — UID/GID mapping](https://docs.docker.com/engine/security/rootless/uid-gid-mapping/) | Effet du remappage sur les fichiers appartenant à l'utilisateur hôte. |
| R3 | [Docker Compose — Secrets](https://docs.docker.com/reference/compose-file/services/#secrets) | Les sources de type fichier utilisent un bind mount ; les attributs uid/gid/mode ne remappent pas ces fichiers. |
| R4 | [Docker — Seccomp](https://docs.docker.com/engine/security/seccomp/) | Profil par défaut et périmètre du filtrage des appels système. |
| R5 | [Podman — Manuel](https://docs.podman.io/en/stable/markdown/podman.1.html) | Mode rootless et dépendance aux capacités du système hôte. |

## Limites de la recherche

Les noms de produits proches de Jev trouvés sur des domaines tiers n'ont pas été utilisés pour définir l'intégration. Les résultats de forums et les agrégateurs ne servent pas de preuve d'auto-hébergement. Aucune clé n'a été créée et aucun compte fournisseur n'a été contacté pendant cette recherche.

Les termes **confirmé**, **choix du projet** et **à confirmer** séparent les faits documentés des décisions d'architecture et des éléments nécessitant une validation en environnement réel.
