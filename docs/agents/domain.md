# Documentation du domaine

ai-guard partage le vocabulaire du contrôle, des workers et des tenants entre l'exécution locale et cloud. Le frontend TypeScript et les adaptateurs Rust suivent ces mêmes contrats.

- Lire le [contrat d'architecture](../engineering/ARCHITECTURE_CONTRACT.md) et le [registre des décisions](../engineering/DECISIONS.md) pour les frontières, les invariants et l'historique des choix.
- Lire les contrats du parcours concerné, par exemple [identité et configuration](../sso.md), [interface](../ui.md), [supervision](../engineering/BROKER_SUPERVISION.md) ou [multi-client](../engineering/CLOUD_TENANCY.md).
- Utiliser les [règles de code](../engineering/CODING_RULES.md), le [cahier de tests](../engineering/TEST_STRATEGY.md) et les [gates de production](../engineering/PRODUCTION_READINESS.md) lors des revues.

Pour une précision de vocabulaire, compléter le contrat qui définit déjà le terme. Consigner une décision validée dans le registre existant. Si une proposition contredit le contrat, signaler le point et la décision à obtenir avant de modifier l'architecture.
