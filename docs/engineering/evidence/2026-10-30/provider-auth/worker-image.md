# Qualification ciblée — image worker avec bootstrap fournisseurs

Date : 2026-10-30 (Europe/Paris)

## Résultat

- **PASS — build candidat** : `agentsandbox-worker:qa-managed-providers-20261030-ccgpe3`
- **PASS — image** : `sha256:069ba7b703077238f22edbefc9295a11cc07e71e8dd490b8de7ab81da51ffa38`, `linux/arm64`, taille Docker 1,808,392,114 octets.
- **PASS — versions réelles locales dans l’image** : `codex-cli 0.165.0`; `2.1.140 (Claude Code)`.
- **PASS — aides de capacité** : `codex exec --help` contient les options requises `--json`, `--sandbox`, `--skip-git-repo-check`, `--ephemeral`, `--config`; `claude --help` contient `--setting-sources`, `--print`, `--output-format`, `--verbose`, `--no-session-persistence`, `--permission-mode`.
- **PASS — présence bootstrap** : `/usr/local/bin/ai-guard-provider` exécutable dans l’image.
- **PASS — refus sans credential** : démarrer le bootstrap avec handoff absent rend le diagnostic public `credential_missing: the private provider handoff is unavailable`, code 126. C’est le refus attendu, pas une validation d’authentification.
- **PASS — confinement de la sonde** : conteneur terminé, utilisateur `1000:1000`, `network=none`, rootfs read-only, `no-new-privileges`, toutes capacités supprimées, mémoire 1 GiB, CPU 0.5, PID max 64, driver de logs `none`, HOME et `/tmp` sur tmpfs privé.
- **PASS — intégrité des entrées** : les SHA-256 des sources Rust, manifestes/lockfile, toolchain, Dockerfile et `.dockerignore` avant/après build sont identiques.

## Environnement et commandes

Docker CLI 29.2.1; Docker Desktop 4.97.0 (244584); moteur 29.9.2, `linux/arm64`. Build via `/opt/homebrew/bin/docker --host unix:///Users/louis/.docker/run/docker.sock build --progress=plain --file images/worker.Dockerfile --tag agentsandbox-worker:qa-managed-providers-20261030-ccgpe3 --build-arg CODEX_VERSION=0.165.0 --build-arg CLAUDE_CODE_VERSION=2.1.140 .`; code retour 0. La whitelist `.dockerignore` ne réinclut que les manifestes de build et fichiers Rust/config autorisés, plus les entrées front/images explicitement listées; `.env` et `.env.example` ne sont pas inclus.

La sonde a utilisé un seul conteneur nommé `qa-managed-providers-20261030-ccgpe3-probe`, conservé arrêté pour preuve. Commandes exécutées : versions locales et aides seulement, puis bootstrap sans variable de handoff. Aucun prompt, appel modèle, credential, fichier `.env`, cache OAuth, keychain ou secret hôte n’a été utilisé ou lu. Aucune image antérieure n’a été modifiée ni supprimée.

## Limites et statuts hors périmètre

- **NOT_RUN — démarrage fournisseur après handoff valide**, setup d’authentification, conversation, abonnement, API payante et appel modèle. Aucun de ces éléments n’est qualifié par `--version`, l’aide ou le refus sans credential.
- **NOT_RUN — validation de parsing/bootstrap avec payload privé valide** ; la sonde n’a volontairement fourni aucun payload. Le test confirme seulement l’entrée et le refus sans credential.
- **NOT_RUN — image complète d’exécution, piles de contrôle, tests runtime/E2E, Podman, Linux x86_64 et Kubernetes**.
- L’image cible est `linux/arm64` exécutée par Docker Desktop sur macOS ARM64 ; elle ne prouve pas une qualification native Linux.
- npm a averti que le script postinstall de `@anthropic-ai/claude-code@2.1.140` n’était pas autorisé lors de l’installation globale (`install-scripts`). La version et l’aide fonctionnent néanmoins. Toute dépendance du postinstall au vrai lancement reste **NOT_RUN**.
- Le build a utilisé la configuration Docker CLI par défaut pour les bases publiques `rust:1.99.1-bookworm` et `node:24-bookworm-slim`; aucun secret/config Docker n’a été lu par l’agent. Les inspections post-build ont utilisé un `DOCKER_CONFIG` temporaire vide. Le build n’a fait aucun push.

## Preuves locales

Dossier privé mode 0700 : `/tmp/ai-guard-managed-worker.ccgPe3/`. Les logs, hashes et inspections sont en mode 0600 : `build.log`, `probe.log`, `sources-before.sha256`, `sources-after.sha256`, `sources.diff`, `container-inspect.txt`, `image-inspect.txt`. Rapport créé en mode 0600.
