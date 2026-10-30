# GREEN — persistance DecisionRequest.execution_mode (audit isolé)

- **PASS** `approval_for_batch_cannot_authorize_interactive_mode`.
- **PASS** `a_mode_that_would_be_masked_cannot_become_a_durable_decision`.
- **PASS** `interactive_decision_mode_is_durable_and_replayed_without_another_event`.
- Résultat : 3 passed, 0 failed, 0 ignored.

## Provenance et périmètre

Nouvelle copie `/tmp/ai-guard-interactive-audit-green.N7g3Kv`, faite depuis la copie RED privée `/tmp/ai-guard-interactive-audit-red.Y4p0Qz`, sans recopier son `target`. Deux seuls fichiers remplacés depuis le checkout courant, propriétaires root/stables :

- `src/audit.rs`: SHA-256 `c1f738d4805bbe8fe4cca8acc817e5294c8e6577bb0d994d621f5de0692301eb`
- `src/audit/control.rs`: SHA-256 `54dd3f42a0ae40dde629932e5d95a52a2e4c55b9d28174d8378d6055eaf72f26`

`tests/decision_execution_mode.rs` est identique au RED, SHA-256 `2c1f5ed5603507a0ae3ead8eee980afbf0dbe8f6b0532c3b3b26281eef4654a8`. Les `execution_mode: None` mécaniques ajoutés au runner et au littéral de reconstruction historique restent les seuls ajustements de compatibilité dans la copie. Le test ne qualifie que le raccord audit isolé, pas l’intégration TTY complète.

Commande exécutée avec PATH explicite et shell login désactivé :

```sh
cargo test --manifest-path /tmp/ai-guard-interactive-audit-green.N7g3Kv/Cargo.toml --locked --test decision_execution_mode
```

Log complet : `/tmp/ai-guard-interactive-audit-green.N7g3Kv/test.log`. Aucun changement du dépôt, moteur, fournisseur, projet ou credentials.
