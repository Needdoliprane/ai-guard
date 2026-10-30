# RED — persistance DecisionRequest.execution_mode

- **PASS** `approval_for_batch_cannot_authorize_interactive_mode`.
- **FAIL attendu, comportemental** `interactive_decision_mode_is_durable_and_replayed_without_another_event`: `security.decision` retourne `execution_mode` absent (`null`) au lieu de `interactive_tty_v1`.
- **FAIL attendu, comportemental** `a_mode_that_would_be_masked_cannot_become_a_durable_decision`: la décision au mode masquable est acceptée (`Ok(None)`), alors que le test attend un refus.
- Résultat : 1 passed, 2 failed ; compilation réussie. Les deux FAIL sont les défauts RED ciblés, pas une erreur de compilation.

## Provenance/isolation

Copie privée `/tmp/ai-guard-interactive-audit-red.Y4p0Qz`, créée depuis `/tmp/ai-guard-engine-logs-final.mGhUZD/source` (archive SHA-256 `e58e387c47432e7335e73679e87deb1226e68b27827b406addec9b685b72cd18`). Seuls `src/model.rs` et `tests/decision_execution_mode.rs` proviennent du checkout WIP. Pour rendre la copie compilable, `execution_mode: None` a été ajouté mécaniquement aux anciens littéraux `DecisionRequest` de `src/runner.rs` et `src/audit/control.rs`; aucune logique de persistance n’a été ajoutée. `src/doctor.rs` n’existe pas dans cette source et aucun autre fichier n’a nécessité de changement. Aucune modification du checkout, base, moteur ou credential.

Hashes des fichiers importés : `src/model.rs` `2f5bf28b9264b5bc2df20a015cd7f8379d669620443b74b6328fe953ce838c93`; test `2c1f5ed5603507a0ae3ead8eee980afbf0dbe8f6b0532c3b3b26281eef4654a8`.

Commande : `cargo test --manifest-path /tmp/ai-guard-interactive-audit-red.Y4p0Qz/Cargo.toml --locked --test decision_execution_mode` avec PATH explicite et shell login désactivé. Sortie conservée dans `/tmp/ai-guard-interactive-audit-red.Y4p0Qz/test.log` (exit 101 attendu).
