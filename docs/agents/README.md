# Skills Matt Pocock dans ai-guard

Le projet intègre les 27 skills des groupes `engineering` et `productivity` distribués par le plugin amont 1.3.2. Chaque import comprend ses fichiers de référence et ses métadonnées Codex lorsqu'elles existent. [Révision amont `abeeaf95660ba9e8b7c753eb38f441ca707a8823`](https://github.com/mattpocock/skills/tree/abeeaf95660ba9e8b7c753eb38f441ca707a8823) · [Licence MIT de Matt Pocock](../../.agents/skills/LICENSE.mattpocock)

Les fichiers sont disponibles depuis ce checkout sous [`.agents/skills/`](../../.agents/skills/) pour Codex et [`.claude/skills/`](../../.claude/skills/) pour Claude Code. Utiliser par exemple `$tdd` dans Codex ou `/tdd` dans Claude Code. En cas d'homonyme ou d'absence du skill dans la liste du client, indiquer le chemin du `SKILL.md` du projet.

La découverte des 27 skills a été vérifiée hors réseau dans Codex CLI 0.170.0 et Claude Code 2.1.330. La commande `/code-review` de Claude Code résout le skill du projet dans ai-guard.

| Besoin | Skills |
|---|---|
| Isoler un défaut et conserver la recette | `diagnosing-bugs`, `tdd` |
| Clarifier un contrat ou découper une fonctionnalité | `grill-with-docs`, `to-spec`, `to-tickets` |
| Avancer par lots indépendants et coordonner l'intégration | `implement-spec`, `implement` |
| Relire un lot par rapport au besoin et aux règles du dépôt | `code-review` |
| Reprendre un travail entamé par un autre agent | `handoff` |

La configuration du projet est déjà définie dans les guides [tickets](issue-tracker.md), [triage](triage-labels.md) et [domaine](domain.md). Les tests et revues conservent les critères de [`AGENTS.md`](../../AGENTS.md) et de la [stratégie de validation](../engineering/TEST_STRATEGY.md). Pour une régression utilisateur, réutiliser le script CLI ou navigateur reproductible qui exerce le comportement concerné. Les revues de conformité se fondent sur le contrat et la candidature effectivement testée.

Les workflows d'implémentation respectent les propriétaires des lots et préservent les worktrees des tâches en cours. Les opérations Git et les scripts proposés par les skills suivent le périmètre de la demande et les règles du dépôt. Une action sur GitHub ou GitLab requiert une autorisation explicite, y compris si le workflow amont prévoit un push ou une PR.

Pour mettre à jour, choisir un commit amont explicite, relire les instructions et scripts qui changent, importer les mêmes dossiers dans les deux emplacements et actualiser la référence de provenance. `diff -qr .agents/skills .claude/skills` permet de vérifier la parité tant que les deux répertoires contiennent le même jeu de skills.
