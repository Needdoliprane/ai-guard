# Supervision Store v3 — artefacts et contrôles finaux

Date : 2026-10-30. Revue indépendante clôturée avant cette passe. Aucun code n’a été modifié pendant les contrôles. Les logs et manifestes sont conservés en privé sous `/tmp/ai-guard-supervision-store-artifacts.oPgh5A` (dossier 700, fichiers 600).

## Vérifications après revue

| Commande | Statut | Durée mesurée |
|---|---:|---:|
| `cargo fmt --all -- --check` | PASS | 0 s |
| `cargo clippy --locked --all-targets -- -D warnings` | PASS | 7 s |
| `cargo build --release --locked` | PASS | 32 s |

Logs originaux : `fmt.log`, `clippy.log`, `release-build.log`; codes et durées : fichiers `.status` dans le répertoire privé ci-dessus. Aucun test n’a été relancé pendant cette passe d’artefact.

## Provenance du code

Manifestes complets `Cargo.toml`, `Cargo.lock`, `src/**/*.rs`, `tests/**/*.rs` avant/après dans `source-hashes-before.txt` et `source-hashes-after.txt`. Ils sont identiques (`diff -u` sans sortie).

Fichiers clés au gel :

- `src/audit.rs` : `112b1044b2be813f47a3ab6adcfda94942fee05702bbb04d57313b99dfe877a3`
- `src/audit/control.rs` : `2d80c7855dc3e4ab0622c4a41076607ad008b8a107a683dd48efaac60cc895c1`
- `src/audit/supervision.rs` : `e5fe84ede6ae136e4298c6323aa928dd22b485725cb4b024412ef6aa1053600e`
- `src/audit/storage.rs` : `ac381df73de99def79aa441853c44ac4ac7b1265d776e2c684e27587490b493c`
- `src/control.rs` : `afe40dc8477fbb19be6de784a91c135230d2723fef5135499ae9ba7656657d8f`
- `src/control/supervision.rs` : `1a65c3cd5e9bc10cc8ec98df832a073a0fed1f24ae9fdff45a3631aec2f2484c`
- `src/api.rs` : `4db5d205e0571a5843e6b92edda61c727c3c14f296d6bbba3631f3d71116e393`
- `tests/supervision_store.rs` : `9eaf15f96d47730513e154f1d2962fe88ebc0851c0ee94037801097571eb3537`

## Release artefacts

Les SHA ci-dessous identifient les nouveaux artefacts compilés ; aucune identité binaire n’a été prédite ou comparée à un SHA source :

- `target/release/agentsandbox` : `8168c30213966093dd907a1c9b290968cda45dca4a3be908341bd911998f59d0`
- `target/release/libagentsandbox.rlib` : `66f607c8fae328251488305e7994fb5e618fdc68ba040f083f538b6d8b8b9e6e`

Les SHA sont également dans `release-artifacts.sha256`.

## Statut des tests (sans double comptage)

- **94/94 PASS** sur le gel précédent, documentés dans `/tmp/ai-guard-supervision-store-tests.md`.
- Après le seul delta de sérialisation `verify_diagnostic_backing`, les **24 tests concernés** ont été rejoués et ont tous PASS : supervision Store 9, audit integrity 12, audit security 3. Ce résultat chevauche les 94 tests et ne s’y additionne pas. Le rapport de validation conserve les RED avant correction et les deux manifestes.
- Aucun full suite, runtime Docker/Podman, E2E, modèle ou fournisseur n’a été exécuté.
