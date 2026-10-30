# Tentative réelle Claude Code TUI — 2026-10-30

## Statut

**FAIL — qualification TUI à deux tours non réalisée**, en raison d'une erreur d'instrumentation de l'orchestrateur après le démarrage du worker. Aucun second message n'a été envoyé et aucun retry fournisseur n'a eu lieu. Ce résultat ne constitue pas un échec du code produit ni une preuve que Claude a répondu au premier prompt.

## Binaire, image et configuration

- Binaire gelé utilisé: `target/release/agentsandbox`, SHA-256 `7a86a4ffb494c706de197931ecfbd00da20be23263ce0d64bd60b5e692e1e3ed`.
- Sources confirmées: `src/runtime/terminal.rs` SHA-256 `5eef5abec5b351a079b7aa11badc46c8d46be4ff3c1e903772f75da7d847d823`; `src/runtime.rs` `7e878ebc1d7389c390f1d2cdfde6efadbc001677162515f1cf44ff48fae74a42`.
- Image immuable vérifiée: `agentsandbox-worker:qa-claude-2.1.140-codex-0.165.0-20261030`, ID `sha256:d6b17dcb85e23a240fa0fef25e14247b117e62c5ce700712d5bef04573d1ae73`, Linux/arm64; Docker client/server `29.2.1`/`29.9.2`.
- Le gateway antérieur sur `127.0.0.1:18787` ne répondait pas. J'ai créé un gateway SQLite privé neuf sur `127.0.0.1:18789`, sans réinitialiser ni rejouer l'ancien. Dossier de preuves `/tmp/ai-guard-claude-tui.oO9z1b`, mode `0700`; configuration, HOME conteneur et projet séparés. Projet fixture initial: seulement `FIRST_TURN.txt`.
- Clé dédiée hors projet: `/tmp/ai-guard-anthropic-private.azwq8v/anthropic-api.env`, fichier `0600`, parent `0700`; valeur jamais lue dans une sortie. Aucun `.env` du dépôt ni OAuth hôte n'a été transmis.
- `--secret-env-file` est une option du broker `agentsandbox run`, pas de Claude Code; le broker lit le fichier et l'injecte dans le worker.

## Préflight TUI

Dans l'image sans clé, sans réseau et sans modèle, `claude --version` a indiqué `2.1.140 (Claude Code)`. L'aide locale confirme que `claude [prompt]` démarre en mode interactif par défaut; `-p/--print`, `--max-budget-usd`, `--no-session-persistence` et `--output-format` sont explicitement limités au mode `--print`. Ces options print-only ont donc été écartées. `--model`, `--permission-mode`, `--tools` et `--bare` sont des options globales de démarrage. La configuration prévue utilisait l'alias `haiku`, `acceptEdits`, outils `Read,Write`, `--bare`, et seulement les sources de réglages `project`.

Le test local sans fournisseur a confirmé qu'un shell `exec_command` en mode login avec PTY fournit bien un controlling TTY. L'essai réel a configuré 120x40. L'aide officielle indique que `claude "query"` démarre une session interactive avec message initial positionnel et que `/exit` quitte la CLI. Aucun recours à Ctrl-D n'a été supposé. Sources: [référence CLI Claude Code](https://code.claude.com/docs/en/cli-usage), [commandes Claude Code](https://code.claude.com/docs/en/commands).

## Déroulement et cause

La commande a démarré le binaire gelé avec `--interactive`, observe/bridge, timeout worker 180 s, l'image exacte et le fichier dédié. Le premier prompt était l'argument positionnel de Claude, demandant une lecture du fixture et la création de `result.txt`/`receipt.txt`. L'enveloppe mémoire a ensuite levé `ReferenceError: TextEncoder is not defined` en calculant la longueur de capture, après le démarrage du processus et avant l'enregistrement du handle PTY dans `store()`. Le dump terminal n'a pas été écrit sur disque ni émis dans l'outil.

Le SQLite a confirmé un seul worker `b991dc80-233a-4f5f-9ccc-a1b0e82fb56a`, session `8ee20c25-cb1d-443d-81dd-fb2cbea5764b`, CID `a58462a3fdd63ceae367039a4a05b3218888a5451a4bc8afe0018a2ca78e12b7`. Le PID terminal exact `32810` était sur `ttys002`; le contrôle ultérieur n'a plus trouvé ce PID. L'audit montre `command.started`, puis une transition explicite `stop` demandée sur cette session owned, terminale après observation et cleanup. L'état terminal est `terminal`, observation `exited`, `timed_out=false`, quota relâché. Le résultat de commande est exit `137` après l'arrêt opérateur. Je n'ai pas observé le prompt TUI, son état idle, une réponse du premier tour, ni une sortie native `/exit`; ces critères restent **NOT_RUN**.

Conformément à l'autorisation d'arrêt du seul worker owned en cas d'état ambigu, `worker-stop` a été appelé une fois pour cette session exacte. Aucun autre worker/instance n'a été ciblé. Pas de boucle de test, pas de deuxième message, pas d'autre appel modèle.

## Résultats readonly

- Worker terminal, cleanup confirmé; quota tenu `false` / actifs `0`.
- Une allocation observée, `billable=false`; aucun coût fournisseur n'est déduit.
- Vérification d'intégrité après l'arrêt et après export: `valid=true`, 32 événements.
- Export conservé sous `exported/`: seulement `FIRST_TURN.txt`; `result.txt` et `receipt.txt` absents. Le fixture source est inchangé et son SHA-256 égale celui de la copie exportée.
- Scan littéral de la clé sur les fichiers de preuve: `0` occurrences.
- Pas de transcript PTY conservé ou imprimé. Les diagnostics CLI de stop/export sont privés, mode `0600`; aucun fichier du dépôt n'a été modifié.

## Preuves privées et limites

Dossier: `/tmp/ai-guard-claude-tui.oO9z1b`. Il contient la base SQLite et le nouvel export, les fichiers de statut et les résumés stop/export; aucune sortie terminal brute. Le gateway local reste disponible sur le port loopback `18789` pour revue. Le code 137 correspond à l'arrêt opérateur observé, pas à une sortie native Claude 0.

La cause est circonscrite au wrapper de test: l'environnement `functions.exec` n'expose pas `TextEncoder`. Pour un futur helper, conserver le handle via `store()`/`notify()` immédiatement avant tout formatage et compter `string.length`; valider le wrapper en mode purement local avant de démarrer un worker. La tentative actuelle reste un FAIL de qualification et ne doit pas être reclassée comme un test TUI à deux tours.

## Maintenance gateway après revue

Le 2026-10-30, `lsof` a lié le listener privé `127.0.0.1:18789` au PID `32675`; `ps` a confirmé les arguments du seul gateway TUI pointant vers `/tmp/ai-guard-claude-tui.oO9z1b/audit.sqlite`. Seul ce PID exact a reçu `SIGTERM`. Vérification après l'action: le listener `18789` est absent et le PID n'existe plus. Aucun worker ne tourne. Le gateway batch privé `18787` était déjà absent et n'a pas été signalé. Les bases SQLite, configs, exports, volumes et images restent conservés; le port partagé `48787` n'a pas été touché.

Addendum: le préflight local pur du wrapper V8 est consigné séparément dans `/tmp/ai-guard-claude-tui-wrapper-preflight.md` (**PASS wrapper seulement**, aucune exécution process/provider). Il a confirmé l'enregistrement `store()` immédiat, `string.length=16` en unités de code UTF-16 pour une sortie artificielle ASCII, et l'absence de `TextEncoder` dans functions V8. Le statut de qualification TUI reste FAIL.
