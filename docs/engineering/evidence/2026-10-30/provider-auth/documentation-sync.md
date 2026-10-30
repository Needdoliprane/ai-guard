# Synchronisation documentaire fournisseurs — 30 octobre 2026

Périmètre produit modifié exclusivement : `docs/engineering/PROVIDER_AUTH.md`, `docs/quickstart.md`, `docs/operations.md`, `docs/api.md`. Aucun fichier gouvernance, preuve historique ou code produit modifié dans cette tâche.

## Faits corrigés

- PROVIDER_AUTH conserve la proposition initiale, signalée comme historique, et ajoute une section datée d'état livré : CLI gérée, configuration privée et références relatives, quatre sources d'authentification, sélection explicite/Auto sans fallback, diagnostics et limites.
- Quickstart et opérations présentent `providers init/status`, `--client`, `--auth-mode`, `--client-profile`, `--providers-file`, prompt/modèle et politique observe + bridge. `status` ne prétend pas vérifier credentials ou image ; le doctor fournisseur autonome et l'assistant login/import/status ne sont pas présentés comme livrés.
- API décrit `provider_resolution` typé, omission historique contre `null` refusé, égalité demande/admission, binding/noms/transport/preset, exigence NoLogs des parcours gérés et plan3 privé sans nouvelle résolution à l'approbation/reprise. Les claims consommés et plans1/2 conservent leur sens.
- Le PASS Claude/API batch manuel de l'ancien binaire/image QA est séparé du FAIL d'instrumentation TUI. L'ancien alias `haiku` ne qualifie pas Haiku5.5 ; les nouveaux exemples Claude utilisent `claude-haiku-5-5` explicitement.
- Dernier fait transmis par root intégré : build et sondes sans credential de l'image `agentsandbox-worker:qa-managed-providers-20261030-ccgpe3` PASS sur Docker Desktop/macOS ARM64, ID exact consigné dans PROVIDER_AUTH. Aucune authentification native anticipée. Le postinstall npm Claude non autorisé reste une limite du vrai lancement à ce gel.
- Jev : endpoint System One, modèle explicite `jev-latest`, question `choice` et refus bornés décrits ; aucun Long Context par défaut. La clé reste un fichier brut `JEV_API_KEY_FILE`, sans dotenv complet ni shell source. JEV/TYPESAFE désignent documentairement la même clé de service, sans créer d'alias produit. Les variables Jev interdites dans le handoff manuel et les collisions par valeur sont documentées.
- Deux formulations d'exploitation périmées corrigées dans le même périmètre : nom de volume désormais issu du registre et réconciliation manuelle existante, sans promesse de supervision autonome après mort du broker.

## Vérifications et sources

PASS — recoupement statique des commandes/champs avec `src/operations.rs`, `src/runner.rs`, `src/provider.rs`, `src/provider/selection.rs`, `src/provider/credentials.rs`, `src/model.rs`, `src/policy.rs`, `src/control.rs`, `src/jev.rs`, `src/main.rs` et l'override Jev. Existence des chemins locaux liés vérifiée ; nouveaux titres/ancres recoupés par lecture.

Rapports consultés : `/tmp/ai-guard-provider-core-implementation.md`, `/tmp/ai-guard-provider-runner-implementation.md`, `/tmp/ai-guard-provider-post-review-software.md`, `/tmp/ai-guard-claude-api-real.md`, `/tmp/ai-guard-claude-tui-real.md`, puis `/tmp/ai-guard-managed-worker-image.md` après steering root. Les nombres de tests et résultats antérieurs sont des preuves rapportées, pas des tests réexécutés par cette tâche.

Sources TypeSafe publiques ouvertes et citées : [System One](https://docs.typesafe.ai/api/system-one), [Choice](https://docs.typesafe.ai/primitives/choice), [modèles](https://docs.typesafe.ai/models).

NOT_RUN — aucun test, build, E2E, appel modèle ou appel API produit. Aucun `.env` réel ni credential hôte ouvert. Les quatre parcours natifs gérés restent NON QUALIFIÉS / NOT_RUN au gel build/sondes ; aucun résultat de l'invocation Claude en cours ailleurs n'est anticipé. Root ajoutera son résultat après ce gel documentaire.

## Empreintes du gel documentaire

```text
e887de735aa7f78be8b1f2cf7033f5fb0a04d8eeb001c3626a6c337ebe72fce3  docs/engineering/PROVIDER_AUTH.md
8a56d638b6a64c08786eca188a876e724a56e9348784ec889d0f82c1d292fcd6  docs/quickstart.md
0bce85aad0ee415cbfca43b8584404719292fb1d34747ec6b53c7c4015147d2d  docs/operations.md
3722c023f448053cba602d994feb50c7df8c350565b1a708e39818b0cec71d96  docs/api.md
```
