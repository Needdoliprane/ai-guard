# Vérifications initiales CLI interactif — 2026-10-30

## Résultats

- **PASS** `cargo test --locked runtime::terminal::tests --lib` : 5 passed, 0 failed, 64 filtrés.
- **PASS** `cargo test --locked --test decision_execution_mode` : 3 passed, 0 failed, 0 ignorés.

Logs complets : `/tmp/ai-guard-interactive-terminal-lib.log` et `/tmp/ai-guard-interactive-decision-audit-integration.log`.

## Empreintes

SHA-256 avant et après les deux commandes (aucune différence constatée) :

- `src/audit.rs`: `c1f738d4805bbe8fe4cca8acc817e5294c8e6577bb0d994d621f5de0692301eb`
- `src/audit/control.rs`: `54dd3f42a0ae40dde629932e5d95a52a2e4c55b9d28174d8378d6055eaf72f26`
- `src/model.rs`: `2f5bf28b9264b5bc2df20a015cd7f8379d669620443b74b6328fe953ce838c93`
- `tests/decision_execution_mode.rs`: `2c1f5ed5603507a0ae3ead8eee980afbf0dbe8f6b0532c3b3b26281eef4654a8`
- `src/runtime/terminal.rs`: `0cda5b5331030b53eb6847e31d97f070b002194cb0393ca270d2889e49713d01`

Les empreintes avant/après sont aussi conservées dans `/tmp/ai-guard-interactive-initial-hashes-before.txt` et `...-after.txt`. Pas de fichier modifié par cette exécution. Périmètre : filtres demandés uniquement ; aucun moteur, fournisseur, navigateur ou suite globale.
