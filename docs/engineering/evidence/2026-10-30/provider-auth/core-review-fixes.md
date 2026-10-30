# Delta core après revue — PRV-02 / PRV-03

Date : 30 octobre 2026. Ce document complète le [premier gel core](/tmp/ai-guard-provider-core-implementation.md) et la [revue indépendante](/tmp/ai-guard-provider-security-review.md). Les preuves de la campagne complète historique de388tests restent attachées à leur gel antérieur ; aucune campagne complète, commande moteur ou session fournisseur n'a été relancée pour ce delta.

## Changements

**PRV-02** : `provider::load_config_source(&PrivateFile) -> Result<ProviderConfigV1>` est désormais public. La fonction commune revalide le handle, désérialise ses octets, ancre les références relatives au parent de ce même fichier, valide la configuration puis revalide la source. `load_config(path)` ouvre une fois la source privée et délègue à cette passerelle. Le runner peut donc conserver ses contrôles physiques d'exclusion et son handle sans désérialiseur parallèle ni double ouverture indépendante. L'export fonctionnel a été signalé à la racine et à l'agent runner avant les autres travaux, pour débloquer leur compilation.

La régression existante des chemins relatifs vérifie maintenant l'égalité entre les deux points d'entrée et le refus du handle retenu après substitution/réécriture. Les traversées restent refusées. La reproduction RED antérieure de la divergence loader/runner, exécutée par Luna dans une copie immuable puis par le propriétaire runner, est conservée dans [le rapport de reproduction](/tmp/ai-guard-provider-review-repros.md). L'agent runner a séparément rapporté11tests ciblés PASS après intégration ; ce delta ne se les attribue pas comme exécution propre.

**PRV-03** : `run_from_environment` ne contient plus de mutation globale de l'environnement ni d'`unsafe` associé à `remove_var`. La documentation explique son rôle d'entrée autonome qui remplace le processus sur succès et l'absence de promesse de nettoyage mémoire ou de `/proc`. Les sondes, le login et l'exécution finale conservent leurs `env_clear` et leurs environnements reconstruits : la variable interne n'est transmise à aucun de ces descendants. Le parent garde son entrée jusqu'à son remplacement ; l'ancienne suppression n'assurait pas un effacement des copies mémoire/kernel.

Une nouvelle régression lance uniquement le binaire de tests Rust en processus isolé, avec un canari puis un argv invalide. Elle vérifie que l'API retourne une erreur sans modifier l'environnement du processus appelant. Le processus est borné à5secondes, ses sorties à une taille attendue inférieure à8KiB et aucun canari n'y est exposé. Aucun test ne tente de provoquer une UB ou une concurrence sur l'environnement ; aucun client fournisseur n'est lancé.

## Commandes et résultats constatés

Toutes les commandes utilisent `login=false`, le PATH demandé et la toolchain du dépôt.

| Vérification | Résultat |
|---|---|
| `CARGO_INCREMENTAL=0 cargo test --locked --test provider_bootstrap bootstrap_error_preserves_the_callers_environment_in_an_isolated_process -- --exact` avant correction PRV-03 | **RED constaté** :1échec attendu sur l'invariance de l'environnement, sans appel fournisseur. |
| `CARGO_INCREMENTAL=0 cargo test --locked --test provider_config --test provider_bootstrap --no-fail-fast` après corrections | **PASS** :9tests config et6tests bootstrap,0FAIL. Une fixture interne bootstrap ignorée, explicitement non comptée comme test produit réussi ; elle est invoquée par la régression isolée. |
| `CARGO_INCREMENTAL=0 cargo test --locked --lib provider::bootstrap::tests` | **PASS** :2tests de bornes/timeout sur processus Rust natifs ;1fixture interne ignorée, non comptée comme réussite produit. |
| `rustfmt --check --edition 2024` sur les7fichiers core possédés | **PASS**, exit0. |
| Clippy final intégré | **NOT_RUN par cet agent** : réservé à Luna après les trois gels, conformément à la coordination de la racine. Le PASS Clippy du premier gel n'est pas présenté comme preuve de ce delta. |
| Matrice globale / E2E / moteur / fournisseur / login réel | **NOT_RUN**, volontairement hors périmètre de ce delta. Aucun token réel ni modèle utilisé. |

Fichiers de code effectivement changés pour ce delta : `src/provider.rs`, `src/provider/selection.rs`, `src/provider/bootstrap.rs`, `tests/provider_config.rs`, `tests/provider_bootstrap.rs`. Aucun changement de runner, modèle, admission, politique, Cargo, image ou documentation du dépôt par cet agent. Aucun nouveau test global ou dépendance ajouté.

## Critères et limites

- **PASS logiciel ciblé** : point d'entrée de parsing unique sur source retenue, références relatives cohérentes, substitution refusée, erreur bootstrap sans mutation globale et descendants sans variable interne.
- **Revue de clôture** : à recouper par la racine/relecteur sur les hashes ci-dessous ; la revue initiale reste datée de son propre gel.
- Les qualifications réelles des quatre authentifications, TUI, routes/settings natifs adverses, rotation/refresh et masquage des tokens OAuth nouveaux restent ouvertes. Aucun gate production fermé par ce delta.
- La préférence nouvelle du mainteneur pour les futurs essais Claude `claude-haiku-5-5` est respectée : aucun appel Claude actuel n'a été exécuté, ni alias de modèle choisi.

## SHA-256 du gel correctif

```text
5ea5a80ab74c51fc9d66a633e2202031569769591a749bd8a6473ba2058436b9  src/provider.rs
f478c5dd7bd1378f71015c616715be36b5755871d90c7e2af49f90a6e1a3b0da  src/provider/selection.rs
0e8ba1329b72d337ed07a6038dbcfd069a76ff3f594ba91c77cbebde3d1a54cd  src/provider/credentials.rs
3a74372d2fd8be2e90218dae6d8b359037de589c204311ee48216543d9085921  src/provider/bootstrap.rs
336a29277c9692b4755c6bab82797b2ea7014d19897d5b2ea731056d1e559572  src/bin/ai-guard-provider.rs
86de4f8219510e621213461f67b07f35cb00ee9ecacff60e16e06c92e15fc637  tests/provider_config.rs
ca8a3a3c1eddfbf86cbf639c3f51a4dc1478724f1831ed32516546a7a99037d4  tests/provider_bootstrap.rs
```
