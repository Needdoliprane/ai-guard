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

## Retest après correction XNU/drainage

Empreintes stables avant/après : `src/runtime/terminal.rs` `851a9612118db27bf75ab89f8b05bc9c3fb7fb12800867ceddc146ad62d0e980`, `src/runtime.rs` `7e878ebc1d7389c390f1d2cdfde6efadbc001677162515f1cf44ff48fae74a42`, `src/runtime/managed.rs` `411dd20cee645fcc80003b600e33ac55d1f8af5c9f00bfb483012e610d6d028a`. Les tests runtime interactif et ses deux fixtures ont aussi des hashes before/after identiques dans les manifestes du lot.

- **PASS** `cargo test --locked --lib runtime::terminal::tests` : 11 passed, 0 failed.
- **PASS** `cargo fmt --all -- --check` : exit 0, sortie vide.
- **FAIL** `cargo clippy --locked --all-targets -- -D warnings` : trois diagnostics dans `tests/runtime_interactive.rs` : deux `await_holding_lock` (lignes 515, 670; guards `inspections` restent vivants à travers `server.await` lignes 555, 700), un `bool_comparison` ligne 539 (`usage[0].billable == false`). Aucun fichier corrigé.
- **BLOCKED** `cargo build --release --locked` : l’outil linker ne trouve pas le SDK macOS (`xcrun --sdk macosx --show-sdk-path`, “No such file or directory”) et échoue ensuite avec `linker cc not found`. Pas de build release qualifié pour cet environnement de commande.

Logs : `/tmp/ai-guard-interactive-kernel-fixed-terminal.log`, `...-fmt.log`, `...-clippy.log`, `...-release.log`. Les hashes avant/après sont dans `...-hashes-before.txt` et `...-hashes-after.txt`. Les résultats antérieurs RED/FAIL sont conservés dans les sections précédentes ; ce lot ne lance pas le moteur interactif.

### Vérification du build release après contrôle PATH

Le premier appel `cargo build --release --locked` avait utilisé un PATH qui omettait `/usr/bin`; l’erreur `cc not found` était donc un défaut d’invocation, pas une absence d’outilchain. Avec PATH explicite corrigé (`/Users/louis/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin` puis PATH hérité), `command -v cc` renvoie `/usr/bin/cc`, `xcrun --sdk macosx --show-sdk-path` renvoie `/Library/Developer/CommandLineTools/SDKs/MacOSX.sdk`, et la relance unique **PASS** `cargo build --release --locked` termine en 19,04 s. Log : `/tmp/ai-guard-interactive-kernel-fixed-release-retry.log`. Le premier log d’invocation erronée reste conservé.

## Dernier contrôle statique intégré

Après correction des trois avertissements dans le test runtime interactif :

- **PASS** `cargo fmt --all -- --check` (sortie vide).
- **PASS** `cargo clippy --locked --all-targets -- -D warnings`.
- Hashes avant/après inchangés : `src/runtime/terminal.rs` `851a9612118db27bf75ab89f8b05bc9c3fb7fb12800867ceddc146ad62d0e980`, `src/runtime.rs` `7e878ebc1d7389c390f1d2cdfde6efadbc001677162515f1cf44ff48fae74a42`, `src/runtime/managed.rs` `411dd20cee645fcc80003b600e33ac55d1f8af5c9f00bfb483012e610d6d028a`, `tests/runtime_interactive.rs` `185c7849a6fadc057f51937b2316560208a8845dc50a063158b730fe013db0a1`.

Logs : `/tmp/ai-guard-interactive-final-static-fmt.log` et `/tmp/ai-guard-interactive-final-static-clippy.log`. PATH des deux commandes inclut explicitement `/usr/bin`. Le module terminal n’a pas été relancé; son 11/11 PASS et le build release PASS après correction PATH restent les résultats ciblés précédents.

## Régression du vrai device contrôlant et gates associés

Au gel `terminal.rs` `5eef5abec5b351a079b7aa11badc46c8d46be4ff3c1e903772f75da7d847d823`, `runtime.rs` `7e878ebc1d7389c390f1d2cdfde6efadbc001677162515f1cf44ff48fae74a42`, `managed.rs` `411dd20cee645fcc80003b600e33ac55d1f8af5c9f00bfb483012e610d6d028a` et `tests/runtime_interactive.rs` `ad328aa39708d26cecb56e6d670767d4cbe0060c46d4bee52aea051cc9082d99` :

- **PASS** `cargo test --locked --lib runtime::terminal::tests` : 12 passed, 0 failed.
- **PASS** `cargo test --locked --test runtime_authorization` : 8 passed, 0 failed, incluant le refus programmatique avant filesystem/network et le refus CLI avant import/admission.
- **PASS** `cargo fmt --all -- --check`.
- **PASS** `cargo clippy --locked --all-targets -- -D warnings`.
- **PASS** `cargo build --release --locked` avec PATH explicite incluant `/usr/bin`.

Logs dans `/tmp/ai-guard-interactive-real-device-{terminal,runtime-authorization,fmt,clippy,release}.log`. Hashes avant/après identiques dans `...-hashes-before.txt` / `...-hashes-after.txt`. `tests/runtime_interactive.rs` n’a pas été exécuté ; aucun moteur/provider/browser ni autre suite.

Les échecs des anciens gels et le RED EINVAL précédent restent historiques et consignés ci-dessus ; ils ne sont pas effacés. Le nouveau gel passe les seules vérifications listées ici.
