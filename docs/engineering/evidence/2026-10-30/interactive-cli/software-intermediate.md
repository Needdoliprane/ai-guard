# Tests logiciels ciblés CLI interactif — 2026-10-30

## Résultats du passage initial

- **FAIL, conservé** `cargo test --locked --lib runtime::terminal::tests` : 6 passed, 2 failed, 65 filtrés. Échecs reproductibles :
  - `runtime::terminal::tests::guard_explicit_restore_and_drop_restore_presentation_and_attributes`
  - `runtime::terminal::tests::task_cancellation_restores_real_pty_attributes`

  Les deux échouent dans l’assertion du test à `src/runtime/terminal.rs:877`, `left=536871369`, `right=457`. Le premier passage correspond au hash `b196e24eb5274527c3e2e5e0fb2b215e39fa25b6b46ce43a1f6adb4843d8fe23` de `src/runtime/terminal.rs`. L’implémenteur examine le delta de flags termios macOS (possiblement PENDIN) ; ce rapport ne qualifie pas le module terminal comme PASS et n’interprète pas ce résultat comme résolu.
- **PASS** `cargo test --locked --lib runtime::managed::log_policy_tests` : 4 passed.
- **PASS** `cargo test --locked --test decision_execution_mode` : 3 passed.
- **PASS** `cargo test --locked --test runtime_authorization` : 8 passed.
- **PASS** `cargo test --locked --test worker_registry` : 36 passed.
- **PASS** `cargo test --locked --test runtime_managed` : 14 passed.
- **PASS** `cargo test --locked --test runner_recovery` : 16 passed.

Les autres filtres se sont exécutés après le premier échec. L’empreinte de `src/runtime/terminal.rs` est passée à `12c1af6f8d79a9ced3ba963fc93165af3c9b87335425027653136f52663d1bed` au cours de ce passage, indiquant une édition concurrente. Je n’attribue donc pas un hash source unique à tout le lot. Les autres empreintes après passage figurent dans `/tmp/ai-guard-interactive-software-hashes-after-initial-pass.txt` et correspondent aux empreintes avant pour les autres fichiers/test ciblés.

## Logs et périmètre

Logs privés :

- `/tmp/ai-guard-interactive-software-terminal-lib.log` (6 PASS, 2 FAIL)
- `/tmp/ai-guard-interactive-software-managed-log-policy-lib.log`
- `/tmp/ai-guard-interactive-software-decision-execution-mode.log`
- `/tmp/ai-guard-interactive-software-runtime-authorization.log`
- `/tmp/ai-guard-interactive-software-worker-registry.log`
- `/tmp/ai-guard-interactive-software-runtime-managed.log`
- `/tmp/ai-guard-interactive-software-runner-recovery.log`

Les empreintes initiales sont dans `/tmp/ai-guard-interactive-software-hashes-before.txt`. Aucun test moteur, fournisseur, navigateur, test global, ni fichier modifié par ce passage. Le nouveau `tests/runtime_interactive.rs` n’a pas été exécuté. Le passage suivant doit attendre l’analyse du guard et se limiter aux tests concernés par le delta.

## Retest après analyse du guard

Cause revue par l’implémenteur : le helper de test conservait la configuration synthétique avant qu’elle soit acceptée par `tcsetattr`; Darwin/XNU ajoute `PENDIN` au readback lorsque `ICANON` est réactivé. Le helper a été corrigé pour prendre l’état accepté via `tcgetattr`, comme le produit. Il compare toujours tous les flags et ne masque pas `PENDIN`; aucune logique de restauration produit n’a été modifiée. Cette correction est expliquée dans `/tmp/ai-guard-interactive-implementation-report.md`.

Au hash final `src/runtime/terminal.rs`=`c065e5cc51cacdd95108ac8adda0ea635e4c64ca766749a770a321af6b39e117` et `src/runtime.rs`=`d6b6d20ec353870f35d709a405a34fdff0d285cbe7b6e66bbba10d431fcf09fa`, retests ciblés :

- **PASS** `cargo test --locked --lib runtime::terminal::tests` : 8 passed, 0 failed.
- **PASS** `cargo test --locked --test runtime_program unbound_runtime_refuses_tty_preset_before_engine_create_or_start -- --exact` : 1 passed, 0 failed. Ce garde refuse le preset TTY avant création/démarrage moteur dans le chemin public non managé.

Logs : `/tmp/ai-guard-interactive-final-terminal.log` et `/tmp/ai-guard-interactive-runtime-program-tty-guard.log`. Les hashes avant/après du retest sont identiques ; preuves dans `/tmp/ai-guard-interactive-final-hashes-before.txt` et `...-after.txt`. Les FAIL initiaux ci-dessus restent archivés comme résultat d’un helper de test corrigé ; ils ne sont pas effacés ni comptés comme le résultat final du module.

## Checks après correctifs queue/reset

Au hash final `runtime.rs` `f9e812c7d55809693a569aee0c2caadb703dfaa8dcb948623d275cebc14a29da` et `runtime/terminal.rs` `380e9b3ba973fc22a6724c9e5463918c2601c56d03143eece80c75a8c3ce6518` :

- **FAIL** `cargo test --locked --lib runtime::terminal::tests` : 9 passed, 2 failed (65 filtrés). Échecs exacts :
  - `reaped_transport_drains_actual_pty_tail_until_real_eof_and_masks_private_suffix` : stdout vide au lieu de `queued tail` suivi des marqueurs/suffixe filtré attendus (`terminal.rs:1083`).
  - `reaped_transport_cannot_hide_a_filter_failure_or_incomplete_control_in_its_tail` : `RuntimeStreamEnded` retourné au lieu de l’erreur attendue (`terminal.rs:1093`).
  Le nouveau test de reset des marges passe.
- **PASS** `cargo fmt --all -- --check` (log vide, exit 0).
- **FAIL** `cargo clippy --locked --all-targets -- -D warnings` : `manual_range_patterns` à `src/runtime/managed.rs:609` (`1 | 2 | 3`) et `src/runtime.rs:1414` (`1 | 2 | 3`), suggère `1..=3`. Aucune correction faite.
- **PASS** `cargo build --release --locked`.

Logs : `/tmp/ai-guard-interactive-final-terminal.log`, `...-fmt.log`, `...-clippy.log`, `...-release.log`. Les hashes de toutes les sources concernées et tests sont dans `/tmp/ai-guard-interactive-final-hashes-before.txt` et `...-after.txt`; source runtime et tests sont stables sur ce lot. Les deux fichiers de fixture ajoutés ont été hashés après l’exécution, mais n’avaient pas été inclus dans la liste initiale avant : leurs hashes après sont consignés uniquement dans le fichier `...-after.txt`.
