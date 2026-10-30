# Essai réel Claude Code batch — 2026-10-30

## Résultat ciblé

**PASS pour cet essai unique**, sur l'image et le broker indiqués. Un appel Claude Code en mode batch a abouti (sortie worker 0, délai respecté) et les deux fichiers exportés correspondent au contenu demandé. Ce résultat qualifie uniquement ce chemin Claude API batch; il ne qualifie pas Claude TUI, une autre image/architecture, ni le produit en général.

## Gel et environnement

- Binaire `agentsandbox 0.1.0`.
- Image existante, non reconstruite: `agentsandbox-worker:qa-claude-2.1.140-codex-0.165.0-20261030`, ID `sha256:d6b17dcb85e23a240fa0fef25e14247b117e62c5ce700712d5bef04573d1ae73`, `linux/arm64`.
- Docker Desktop: client `29.2.1`, serveur `29.9.2`; programme `/opt/homebrew/bin/docker`, socket explicite `unix:///Users/louis/.docker/run/docker.sock`.
- SHA-256: `src/runtime/terminal.rs` `5eef5abec5b351a079b7aa11badc46c8d46be4ff3c1e903772f75da7d847d823`; `src/runtime.rs` `7e878ebc1d7389c390f1d2cdfde6efadbc001677162515f1cf44ff48fae74a42`; test gelé `tests/runtime_interactive.rs` `a5f30e67a2b3078fbf8efa07fc1836f2f128445765e4371a3e6ea6683529cc4c`.
- Gateway SQLite neuf, loopback `127.0.0.1:18787`, dossier privé `/tmp/ai-guard-claude-batch.sxcDi7` (mode `0700`). Projet source neuf sous `project/`; export séparé sous `exported/`.
- Fichier de credential dédié hors projet: `/tmp/ai-guard-anthropic-private.azwq8v/anthropic-api.env`; mode `0600`, parent `0700`. Contrôle d'allowlist sans afficher sa valeur: une seule variable `ANTHROPIC_API_KEY`, valeur non vide. Le `.env` de la racine du dépôt n'a pas été utilisé. Aucun OAuth hôte n'a été transporté.
- `--secret-env-file` est une option du broker `agentsandbox run`; ce n'est pas une option de Claude Code. Le broker lit le fichier privé et injecte le credential au worker. Claude reçoit uniquement sa variable d'environnement dans le conteneur.

## Préflight auth et choix du modèle

- `claude --version` dans l'image: `2.1.140 (Claude Code)`.
- `claude --help` confirme `-p/--print`, alias de modèle, `--permission-mode`, `--tools`, `--max-budget-usd` et `--no-session-persistence`.
- Avec HOME vide, sans clé, sans réseau et sans modèle: `claude auth status` a terminé code 1; aucune sortie brute n'a été conservée. C'est l'absence attendue d'identité locale. Les aides `claude setup-token --help` et `claude auth status --help` terminent code 0; cette dernière expose `--json` et `--text`. Aucun setup-token/login n'a été exécuté.
- Modèle demandé avec alias CLI `haiku`, choix le moins coûteux disponible selon la page officielle Claude Haiku consultée le 2026-10-30. L'alias est enregistré; l'identifiant complet effectivement résolu par le fournisseur n'est pas indépendamment journalisé. Aucun coût fournisseur/facture n'a été lu. La limite CLI était `$0.50`.

## Invocation unique

Fin de l'invocation vers `2026-10-30 11:35:46 Europe/Paris` (heure de dernière écriture du résumé CLI). Elle était protégée par `timeout -k 5 150`; timeout demandé au worker: 120 secondes. Aucun retry.

```text
/usr/bin/timeout -k 5 150 target/release/agentsandbox run \
  --project /tmp/ai-guard-claude-batch.sxcDi7/project \
  --config-dir /tmp/ai-guard-claude-batch.sxcDi7/config \
  --gateway http://127.0.0.1:18787 \
  --runtime docker \
  --runtime-program /opt/homebrew/bin/docker \
  --runtime-endpoint unix:///Users/louis/.docker/run/docker.sock \
  --allow-rootful-runtime \
  --image agentsandbox-worker:qa-claude-2.1.140-codex-0.165.0-20261030 \
  --profile observe --network bridge --timeout 120 --provider claude \
  --secret-env-file /tmp/ai-guard-anthropic-private.azwq8v/anthropic-api.env \
  -- /usr/local/bin/claude -p 'Read input.txt. Create result.txt with exactly one line: observed=OVPT-CLAUDE-API-QUALIFICATION-20261030. Create receipt.txt with exactly these three lines: source=input.txt; result=result.txt; status=created (one field per line, without semicolons). Do not run shell commands, access the network yourself, or write any other files.' \
  --model haiku --permission-mode acceptEdits --tools Read,Write \
  --max-budget-usd 0.50 --no-session-persistence --output-format text
```

No shell tool was enabled for Claude; its configured tools were `Read,Write`. Network access was explicitly enabled through the requested observe/bridge configuration to reach Anthropic. This allows worker egress and is not a claim of application-level egress filtering.

## Résultats vérifiés

- Processus CLI: code 0; summary: `exit_code=0`, `timed_out=false`, `changed_files=2`, `output_captured=false`, cleanup `removed`, no postprocessing error.
- Session `ded69ee2-ba86-403b-b506-13f4e10181d5`; worker `a3b34d7e-b356-49cc-9d4f-6b661ad0f91d`; container `51b615fcc4ef320a715d4203703c56f4194ba1ca4c4894da57c97e6d79b66837`.
- Worker state `terminal`, last observation `exited`, cleanup removed, `quota_held=false`; one worker row and no active quota rows.
- SQLite allocation: exactly one row, provenance `observed`, `billable=false`; allocated duration `8,379,242,129 ns`. No provider billing measurement is inferred from this allocation.
- Audit verification after the run: `valid=true`, 26 events.
- Export destination contained `input.txt`, `result.txt`, and `receipt.txt`. `result.txt` bytes equal `observed=OVPT-CLAUDE-API-QUALIFICATION-20261030`; `receipt.txt` has exactly three lines `source=input.txt`, `result=result.txt`, `status=created`. Both generated files omit only a final newline; line contents and separators match exactly. The original project still contains only its unchanged `input.txt` fixture.
- Literal API-key scan over all regular files under the private evidence directory (logs, audit DB/WAL, config, project and export) returned `0` occurrences. Credential file itself was not copied into that directory.
- Private stdout summary: 460 bytes, mode `0600`; stderr: 0 bytes, mode `0600`. `output_captured=false`; no provider transcript was stored or printed. Worker-status, audit/export summaries and auth-help logs are also mode `0600`. Export files are mode `0644` within the private `0700` evidence directory.

## Preflight artifact logs

- Evidence directory: `/tmp/ai-guard-claude-batch.sxcDi7`.
- CLI summary: `cli.stdout`; stderr: `cli.stderr` (empty). These are broker summary logs, not raw Claude output.
- Auth help summary: `auth-design-help.stdout` and `auth-design-help.stderr`; full relevant help text is summarized above. The first extraction wrapper filtered only the generic help lines, so a second no-network help-only invocation provided the displayed options; it did not call a model or authentication endpoint.
- Gateway/audit evidence: `audit.sqlite`, `audit-verification.json`, `worker-status.json`, export command logs. The gateway remains local to loopback for the maintainer's read-only review; no Docker worker remains running.

## Limites

- One Claude API batch invocation only; no Claude TUI, Codex provider, second model call, setup-token flow, login, or browser test.
- The observed provider call and exact workspace artifacts are established; provider-side billing/cost and full resolved model ID are not.
- Runtime profile `observe` records execution but does not mediate arbitrary actions inside the provider CLI. No general production-readiness claim follows from this test.

Sources officielles du choix d'alias/coût et des options CLI: [Claude Haiku](https://www.anthropic.com/claude/haiku), [Claude Code CLI](https://docs.anthropic.com/en/docs/claude-code/cli-usage), [variables d'environnement Claude Code](https://code.claude.com/docs/en/env-vars).

## Maintenance après essai TUI

Le 2026-10-30, vérification ciblée des listeners privés: le port batch `18787` avait déjà `listener=none`; aucun PID n'a été signalé. Le port TUI `18789` correspondait au PID `32675`, dont les arguments complets identifiaient le gateway privé `/tmp/ai-guard-claude-tui.oO9z1b/audit.sqlite`; seul ce PID a reçu `SIGTERM`. Vérification postérieure: listener `18789` absent et PID absent. Les deux bases d'audit restent présentes et privées; aucun dossier, configuration, volume ni image n'a été supprimé. Le port partagé `48787` n'a pas été inspecté ni touché.
