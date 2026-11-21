# ai-guard — instructions communes

@AGENTS.md

Appliquer intégralement `AGENTS.md`, le mandat `PROMPT_PRODUCTION.md` et les contrats de `docs/engineering/`. Les consignes concernent également les sous-agents et les revues. Les commandes historiques et rapports datés ne constituent pas une preuve du checkout courant.

Ouvrir le [registre des travaux](docs/engineering/WORK_TRACKING.md) à chaque reprise et transmettre au coordinateur les changements d'état et leurs preuves. Pour toute création, intégration ou fermeture d'une branche, d'un worktree ou d'un cache, appliquer la [procédure commune de cycle de vie](docs/engineering/WORKSPACE_LIFECYCLE.md).

Pour le stockage, appliquer la [règle SQL du projet](.claude/rules/storage.md) et le [cahier de tests des trois profils](docs/engineering/TEST_STRATEGY.md#stockage-sql-configurable--à-qualifier).

La cible est un produit open source complet et soigné, local-first et cloud, dans l'architecture validée. Aucun découpage MVP/V2 ne permet d'écarter une exigence obligatoire. Distinguer ce qui est implémenté, ce qui est vérifié et ce qui reste bloqué ; ne jamais présenter un test ignoré ou une fixture comme une qualification runtime.

Les skills de projet sont dans `.claude/skills/` lorsqu'ils sont présents ; leurs copies Codex sont dans `.agents/skills/`. Si un skill annoncé est absent, le signaler et poursuivre les travaux indépendants à partir des contrats, sans prétendre l'avoir chargé.
