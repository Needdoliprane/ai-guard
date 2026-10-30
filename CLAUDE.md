# ai-guard — instructions communes

@AGENTS.md

Appliquer intégralement `AGENTS.md`, le mandat `PROMPT_PRODUCTION.md` et les contrats de `docs/engineering/`. Les consignes concernent également les sous-agents et les revues. Les commandes historiques et rapports datés ne constituent pas une preuve du checkout courant.

La cible est un produit open source complet et soigné, local-first et cloud, dans l'architecture validée. Aucun découpage MVP/V2 ne permet d'écarter une exigence obligatoire. Distinguer ce qui est implémenté, ce qui est vérifié et ce qui reste bloqué ; ne jamais présenter un test ignoré ou une fixture comme une qualification runtime.

Les skills de projet sont dans `.claude/skills/` lorsqu'ils sont présents ; leurs copies Codex sont dans `.agents/skills/`. Si un skill annoncé est absent, le signaler et poursuivre les travaux indépendants à partir des contrats, sans prétendre l'avoir chargé.
