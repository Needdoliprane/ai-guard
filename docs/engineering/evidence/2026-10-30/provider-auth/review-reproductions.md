# Reproductions ciblées des constats fournisseur

## Environnement et périmètre

- Snapshot immuable source : `/tmp/ai-guard-provider-review.fL91yK` (manifest `candidate.sha256`, SHA-256 `ecd4be78628aaee7e4e282a6322a318b9380939f81c738cbbe623d5ebd8faf22`). Les hashes déclarés pour `tests/provider_authorization.rs` et `src/runner.rs` sont respectivement `1a9952377bd9a3c093872e68c8ef00f1b2915d398d69c59dc2437640f0abe572` et `0c24595173ee31c9263233625d3e82787ced77f8dc5e5bd77fccb4a3ab8d131f`.
- Copie privée de reproduction : `/tmp/ai-guard-provider-review-private/snapshot`. Elle contient le snapshot et les ajouts `rust-toolchain.toml`, `policies/`, `deploy/` copiés du dépôt. Aucun `.env` réel, aucune config réelle et aucun credential de l’hôte n’ont été copiés. Les canaris de test sont des valeurs synthétiques temporaires, jamais émises par les logs.
- La compilation s’est faite depuis le chemin de la copie privée ; cache partagé via `CARGO_TARGET_DIR=/Users/louis/Documents/ai-guard/target`. `rustc 1.99.1 (58cf8f9c3 2026-10-13)`, `cargo 1.99.1 (5f94df478 2026-08-27)`.
- Aucune connexion provider, décision HTTP, commande moteur, E2E ou campagne globale n’a été exécutée. Le dépôt racine n’a pas été édité.

## Constat 1 — admission managed batch avec `FixedSandboxV1`

**Résultat : REPRODUIT.** Dans la copie privée, le preset de `managed_decision_and_worker_reopen_with_exact_provider_identity` a été changé de `FixedSandboxV2` à `FixedSandboxV1`. La validation de l’admission passe et `Store::admit_worker` retourne `accepted=true`; le test termine `ok`. Cela confirme l’admission trop permissive au niveau typed managed batch. Ce résultat ne démontre pas un démarrage moteur : aucune exécution runtime n’a été faite.

Commande :

```sh
cd /tmp/ai-guard-provider-review-private/snapshot
CARGO_TARGET_DIR=/Users/louis/Documents/ai-guard/target cargo test --locked --test provider_authorization managed_decision_and_worker_reopen_with_exact_provider_identity -- --exact --nocapture
```

Log conservé : `/tmp/ai-guard-provider-review-private/repro1.log` — SHA-256 `b55287c3a7567dc41ef8c83c95cade3dc0eaf96fbb912298737a79e41a10105b`.

Le fichier de fixture original a été sauvegardé dans `/tmp/ai-guard-provider-review-private/provider_authorization.original.rs` (SHA-256 `1a9952377bd9a3c093872e68c8ef00f1b2915d398d69c59dc2437640f0abe572`). Version modifiée de la copie : SHA-256 `72fafa261c6a45dc621998c2d4dd9e0c3f8178d4e3f9f1577054b0e843902d98`.

## Constat 2 — rebase de chemin incohérent entre loader et runner

**Résultat : REPRODUIT.** Un test unitaire ajouté uniquement dans la copie privée crée un `providers.json` privé avec `credential.path="provider.env"`, et le fichier privé adjacent à la configuration. `provider::load_config` rebase le chemin vers le répertoire de configuration ; `provider::prepare` lit le profil et réussit. Le même JSON brut est ensuite lu dans `runner::choose_plan`, qui échoue sur `credential source requires an absolute path`. Le refus a lieu avant toute décision HTTP et sans lancer le moteur.

Commande :

```sh
cd /tmp/ai-guard-provider-review-private/snapshot
CARGO_TARGET_DIR=/Users/louis/Documents/ai-guard/target cargo test --locked --lib runner::tests::relative_provider_source_is_rebased_by_loader_but_not_runner -- --exact --nocapture
```

Log conservé : `/tmp/ai-guard-provider-review-private/repro2.log` — SHA-256 `d321df79de93c73db999d91a2c93353758441e6978090b9586130181899b30e9`.

Le test repro ajouté à `src/runner.rs` dans la copie a SHA-256 `f28326265d2ff3af388adf14430f7df611363d07836426e2b92cbaad22b3ff1e`. Baseline snapshot du fichier : SHA-256 `0c24595173ee31c9263233625d3e82787ced77f8dc5e5bd77fccb4a3ab8d131f`.

## État des preuves

- Constat 1 : comportement d’admission reproduit (`PASS` pour la reproduction; défaut confirmé).
- Constat 2 : divergence loader/runner reproduite (`PASS` pour la reproduction; défaut confirmé).
- Limite : seul le snapshot figé est testé ; aucun résultat de qualification de runtime ni de fournisseur réel n’est revendiqué.
