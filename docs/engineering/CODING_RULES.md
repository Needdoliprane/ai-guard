# Règles de développement

Voir [AGENTS.md](../../AGENTS.md) et le [contrat d'architecture](ARCHITECTURE_CONTRACT.md), qui priment sur les plans historiques.

- Rust stable hors frontend ; TypeScript pour le frontend existant. Configuration déclarative autorisée ; pas de logique applicative nouvelle en shell, Python ou JavaScript hors frontend.
- Entrées non fiables validées, erreurs propagées, opérations externes bornées et assorties de timeouts. Pas de `panic!`, `unwrap` ou `expect` sur un échec métier possible ; les invariants doivent être justifiés.
- Autorisation côté serveur, refus par défaut, tenant issu de credentials authentifiés, aucune confiance dans les labels ou identifiants d'un worker.
- Les mutations critiques et leur audit sont atomiques et durables. Les reprises conservent une identité ; un contenu divergent est rejeté. Une inconnue n'est jamais remplacée par zéro ou une preuve fictive.
- Exécution moteur par programme et arguments séparés. Les restrictions du runtime doivent être inspectées et testées par leurs effets.
- Préserver les fichiers, secrets, volumes et travaux existants. Une correction commence par une reproduction et garde une régression utile. Les tests ignorés restent `NOT_RUN`.
- Les propriétaires de fichiers sont explicites entre agents. Les changements de sécurité, lancement, audit et import/export exigent un second regard indépendant et le traitement de ses défauts bloquants.
- Synchroniser contrats, documentation et preuves après chaque lot. Distinguer `PASS`, `FAIL`, `BLOCKED`, `NOT_RUN` et `NOT_APPLICABLE` justifié.

## Acceptation produit

Un changement est évalué sur le parcours utilisateur et opérateur qu'il rend possible. Définir avant édition le besoin, le comportement observable, ses états d'échec et le critère d'acceptation. Les interfaces n'embarquent pas de données de démonstration ; une valeur inconnue reste inconnue, une erreur de lecture ne devient pas une liste vide, une donnée périmée porte son état.

Les chemins heureux, refus d'autorisation, erreurs récupérables, déconnexions et reprises doivent produire des messages exploitables sans exposer de secret. Les contrôles d'interface déclenchent une fonction réelle et les permissions sont vérifiées côté serveur. Préserver navigation clavier, focus, libellés, contrastes et adaptation aux petits écrans. Vérifier le comportement sous la volumétrie du parcours concerné.

Installation, évolution des contrats, migrations, sauvegarde/restauration et diagnostic font partie d'une fonctionnalité durable lorsqu'elle les affecte. Mettre à jour les procédures et la compatibilité avec le code. Une documentation de fonction future ou un mock permanent ne remplace pas son implémentation.

Les lots organisent le travail ; ils ne suppriment aucune exigence obligatoire. Identifier explicitement les extensions facultatives et leur justification. Les lacunes non résolues restent visibles dans les gates et l'état courant, même lorsqu'un lot logiciel passe ses vérifications.
