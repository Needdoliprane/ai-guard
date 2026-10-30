# Suivi de réalisation

Le produit demandé est complet, local-first et cloud, dans l'architecture validée par le mainteneur. Le découpage historique MVP/V2 ne définit plus le périmètre. La qualification production n'est pas acquise.

## Sources courantes

| Information | Source faisant autorité |
|---|---|
| Mandat, qualité produit et règles communes | [AGENTS.md](../AGENTS.md), [PROMPT_PRODUCTION.md](../PROMPT_PRODUCTION.md) |
| Architecture imposée | [ARCHITECTURE_CONTRACT.md](engineering/ARCHITECTURE_CONTRACT.md) |
| Fonctions et écarts du checkout | [CURRENT_STATE.md](engineering/CURRENT_STATE.md) |
| Commandes exécutées et résultats constatés | [VALIDATION.md](engineering/VALIDATION.md) |
| Ordre et acceptation des lots | [IMPLEMENTATION_PLAN.md](engineering/IMPLEMENTATION_PLAN.md) |
| Conditions de qualification et publication | [PRODUCTION_READINESS.md](engineering/PRODUCTION_READINESS.md), [RELEASE_CHECKLIST.md](engineering/RELEASE_CHECKLIST.md) |

Le présent index ne duplique pas les compteurs ou statuts volatils. Toute correction et sa revue doivent être intégrées et vérifiées avant d'apparaître comme acquises dans les preuves courantes. Un compte rendu de sous-agent ne remplace pas cette vérification.

## Référence historique du 29 octobre 2026

Le premier état décrivait une CLI/broker Rust, un gateway, un dashboard TypeScript servi par Rust, deux stacks Docker/Podman, un audit SQLite, une politique et des parseurs fournisseurs sur fixtures. Il indiquait des tests logiciels réussis dans son environnement, sans moteur ni session cloud réelle. Les résultats originaux sont conservés dans [validation.md](validation.md).

Ces observations datées ne qualifient pas le checkout actuel, les restrictions runtime, le scénario multi-client à trois workers, la capacité 100, Kubernetes ou une release. Les descriptions historiques « implémenté et testé » doivent être lues avec le périmètre exact du rapport, pas comme une acceptation de toutes les exigences.

## Règle de suivi

Après chaque lot : relier le besoin au code, aux erreurs traitées, aux tests pertinents, à la revue et aux procédures mises à jour. Conserver les écarts ouverts et leurs dépendances précises. Séparer `PASS`, `FAIL`, `BLOCKED`, `NOT_RUN` et `NOT_APPLICABLE` justifié. La compilation, une capture sur fixtures ou un test ignoré ne ferment aucun gate runtime.
