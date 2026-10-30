# Préparation QA Claude Code dans le worker

Date : 2026-10-30 (Europe/Paris)  
Périmètre : préparation du binaire et vérification statique sous contraintes Docker. Aucun appel modèle, aucune authentification fournisseur dans le worker, aucun fichier de credentials ouvert ou affiché.

## Client hôte et authentification

- `/Users/louis/.local/bin/claude --version` : `2.1.140 (Claude Code)`.
- `claude --help` : disponible; le CLI expose le mode non interactif `-p/--print`. Cela confirme les options, pas le fonctionnement du TUI ni un appel fournisseur.
- `claude auth status` : code de sortie 0. Sortie traitée uniquement en mémoire et réduite à des marqueurs booléens : OAuth présent, marqueur API key absent. Aucune identité, adresse, valeur de token ou ligne brute conservée.
- `ANTHROPIC_API_KEY` hôte : absente (vérification de présence seulement).
- Note de procédure : ces trois appels hôte ont malheureusement été lancés avec le dépôt comme cwd, au lieu d'un cwd privé extérieur au dépôt. Aucun login, setup-token, fichier d'auth ou keychain n'a été consulté manuellement; aucun changement au dépôt n'a été demandé par ces commandes. Je n'ai pas relancé d'appel hôte.

## Voie d'authentification pour un worker

La voie minimale déjà prévue et officiellement documentée est une clé `ANTHROPIC_API_KEY` fournie au worker via `--secret-env-file`, dans un fichier privé (`0600`) situé hors du projet. Le runner n'accepte que des lignes littérales `NAME=value`, masque les valeurs avant diffusion des événements et ne transmet le fichier qu'au worker métier. Cette injection exige explicitement `--profile observe --network bridge`; le réseau est alors sortant sans filtrage applicatif et permet une exfiltration possible de données confiées au worker. Pas de clé API fournie à cette préparation, donc cette voie n'a pas été exercée.

La session OAuth hôte n'est pas transportée par le worker : celui-ci a un HOME temporaire vide, et aucun home Claude/keychain n'est copié. Le seul compte hôte constaté est OAuth; aucune clé API hôte n'est disponible. Je n'ai pas tenté d'extraire un token OAuth. Si le maintien de cette voie est souhaité, le propriétaire doit d'abord autoriser un mécanisme explicite et maîtrisé; ne pas copier `~/.claude` ni tenter une extraction implicite. Pour une qualification initiale, fournir séparément une clé API Anthropic dans un fichier privé dédié est la voie documentée par le contrat.

Le code Rust permet des variables à nom majuscule/chiffres/underscore, refuse les variables de contrôle du chargeur (`PATH`, `HOME`, `LD_PRELOAD`, `NODE_OPTIONS`, etc.), exige une valeur d'au moins 8 caractères sans caractères de contrôle, limite le fichier à 64 KiB et rejette les doublons. `ANTHROPIC_API_KEY` convient à cette grammaire. Sources locales : `src/runner.rs` (validation/snapshot) et `src/runtime.rs` (injection seulement sur worker `Run` en réseau bridge).

Documentation officielle consultée : [variables d'environnement Claude Code](https://code.claude.com/docs/en/env-vars) — `ANTHROPIC_API_KEY` sert à l'authentification API et prend priorité en mode `-p` lorsqu'elle est définie. Ce fait ne qualifie pas une compatibilité de compte ou de version par appel réel.

## Image QA et preuve runtime

- Construction depuis `images/worker.Dockerfile`, avec arguments exacts `CODEX_VERSION=0.165.0` et `CLAUDE_CODE_VERSION=2.1.140`.
- Tag neuf, sans remplacer les tags existants : `agentsandbox-worker:qa-claude-2.1.140-codex-0.165.0-20261030`.
- Docker explicite : `/opt/homebrew/bin/docker --host unix:///Users/louis/.docker/run/docker.sock`.
- Image : `sha256:d6b17dcb85e23a240fa0fef25e14247b117e62c5ce700712d5bef04573d1ae73`, plateforme `linux/arm64` (Docker Desktop sur macOS ARM64).
- La construction a réussi. npm a signalé que le script postinstall du paquet Claude n'était pas couvert par `allowScripts`; malgré cela, le binaire embarqué répond correctement aux vérifications suivantes.
- Sous Docker `network=none`, `user=1000:1000`, `cap-drop=ALL`, `no-new-privileges`, rootfs en lecture seule et tmpfs seulement pour `/home/agent` et `/tmp` : Claude `2.1.140`, Codex `0.165.0`, et les deux commandes `--help` terminent avec code 0. Les avertissements Codex sur les alias PATH non inscriptibles sont compatibles avec le rootfs readonly et n'empêchent pas `--version`/`--help`.
- `docker inspect` du conteneur de vérification a confirmé `readonly=true`, `network=none`, `user=1000:1000`, `capdrop=[ALL]`, `no-new-privileges` et tmpfs montés; ce conteneur jetable, créé pour cette vérification, a été supprimé par son nom exact. Aucun volume ni tag existant n'a été supprimé.

## Statuts

- **PASS** : versions exactes et disponibilité `--version` / `--help` pour Claude et Codex dans l'image arm64; paramètres de confinement Docker Desktop macOS ARM64 montrés inspectés.
- **PASS** : statut hôte vérifié sans publier d'identifiant ou de secret; méthode OAuth uniquement rapportée.
- **NOT_RUN** : authentification API dans le worker, appel modèle fournisseur, réseau fournisseur, session Claude TUI, parcours `agentsandbox run` bout en bout et recette worker/runtime complète.
- **BLOCKED** : authentification modèle réelle tant qu'une clé Anthropic dédiée n'est pas mise à disposition dans un fichier privé selon les consignes, ou qu'un mécanisme OAuth explicite n'est pas autorisé et spécifié.
- **Limite** : ce résultat qualifie uniquement la disponibilité CLI et l'application des paramètres Docker Desktop macOS ARM64 montrés; il ne qualifie ni la protection du secret dans un worker agent, ni Linux, Podman, multi-client, le réseau egress, ni les appels au fournisseur.
