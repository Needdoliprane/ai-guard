# Validation des sources humaines — 13 novembre 2026

[English](README.md)

**PASS ciblé sur la candidate publique réduite :** huit canaris exacts et check toutes cibles. Ce résultat appartient au gel ci-dessous. Le résultat du GET d’identité privé reste distinct et ne lui est pas transféré.

## Sources et résultats

| Composition | SHA256 de l’archive source | Résultat vérifié |
|---|---|---|
| Ancien diagnostic de coût privé, 2614 fichiers | `9dfe8a38ea7c1d11d9c16ea67b0e2b5aab1d0c11e1ab3fc5a072a5c0bfe1271c` | Capture diagnostic complète ; HTTP503 métier, pas un PASS de disponibilité |
| Baseline privée, 2614 fichiers | `866a0e5527a34d55aa4766229abd51960ab18b7cd5f96e07c3c7c346f276cfd0` | Un vrai GET d’identité : HTTP503, assertion attendue en échec, parent RC101 |
| Candidate privée corrigée, 2614 fichiers | `3de3abe4c891c3a050d523cfe4545d6e375481eccdc3303f1b5f6a62b30e27a3` | Même parent GET figé : HTTP200 ; neuf PASS dont huit canaris ; check toutes cibles PASS |
| Candidate publique réduite, 2622 fichiers | `8c97f261f299922123ff161f1b4938e916e1a17b6751e318650dcb81708c9769` | Huit canaris exacts PASS, zéro échec/ignoré ; check toutes cibles PASS |

La candidate publique est la base `ceb7b1f1c298a0f31044eca20a09cb2101c4d7c2` plus quatre postimages relues. Elle ne contient ni le module API owned privé ni son parent d’acceptation GET. Son résultat ne qualifie pas ce trajet HTTP. Le zéro de l’orchestration baseline signifiait que l’échec attendu RC101 était classé ; ce n’était pas un test PASS.

Le GET instrumenté antérieur a pris **2301.7 ms** autour du helper complet en mémoire et rendu `503 identity_unavailable`. Il a enregistré **9498 appels de revalidation de fichier**, **131556 appels de revalidation de répertoire** et **19 entrées `BeginChecked`**. Ce sont des appels, pas des fichiers/répertoires distincts ou des commits réussis. Les durées Store ne sont pas mesurées. Les spans FS sont inclusifs et se chevauchent : ne pas additionner durées ou octets pour décomposer le temps ou déduire l’IO physique. Cette observation unique suggère du travail répété ; elle n’établit ni sa cause temporelle complète ni sa fréquence.

## Changement candidat et contrôles conservés

Le trajet de validation de source relu passe de **14 parcours d’ancêtres à 7**, comme fait du code. Les observations encore fraîches sont transmises dans la même validation composée, au lieu d’être répétées entre helpers. Ce changement n’ajoute ni cache de succès ni retry et n’établit aucun délai garanti.

| Fichier | SHA256 de la postimage relue |
|---|---|
| [src/control_fs.rs](../../../../../src/control_fs.rs) | `13f583213890b23d891c42f8eab6d561a886b2d3c94b336122fa1dde08601447` |
| [src/control_fs/source_owner.rs](../../../../../src/control_fs/source_owner.rs) | `1d649dd776d81fc53b79a90b1df0507f57e214b37111dec10fc185d3707cb3f5` |
| [src/human_authority.rs](../../../../../src/human_authority.rs) | `21474eb338b1444bb4c6af9aa792672a60d75ae6a933a96c3d3aedc0566ad3e2` |
| [src/human_authority/tests.rs](../../../../../src/human_authority/tests.rs) | `2bfb62a0c960376cc6f29df38981321842e4af37e9a634b219208f29b5e0a139` |

Liens, DAC/ACL, identités parent/entrée et doubles observations du domaine de montage restent contrôlés. Échéance d’epoch de deux secondes, autorisations et refus conservateurs sont inchangés. L’échec de l’observation I/O initiale du parent garde la branche indisponible (`StorageUnavailable` à l’acquisition) ; les échecs de binding entrée/montage gardent `StorageBindingMismatch`. La chaîne d’erreurs originale reste accessible au classement structuré EMFILE/ENFILE. La pénurie de descripteurs refuse toujours sans ajouter de latch d’invalidation. Ses canaris injectent des erreurs structurées ; aucun épuisement réel des descripteurs n’a été réalisé.

## Sélection exacte et formes des commandes

La sélection comprend quatre nouveaux canaris `human_authority` R1/R2, deux canaris `source_owner` existants et deux voisins epoch/token existants. Il ne s’agit pas de huit nouveaux tests.

```text
human_authority::tests::source_mount_validation_keeps_exact_parent_and_entry_bindings
human_authority::tests::an_accepted_epoch_rejects_parent_substitution_after_a_successful_source_check
human_authority::tests::source_mount_failures_keep_parent_unavailable_and_entry_binding_categories
human_authority::tests::initial_parent_observation_phase_keeps_structured_descriptor_shortage_causes
control_fs::source_owner::tests::same_mount_private_source_exposes_actual_parent_and_entry_identities
control_fs::source_owner::tests::same_mount_source_keeps_private_entry_alias_and_permission_guards
human_authority::tests::an_expired_epoch_does_not_latch_unrelated_future_operations
human_authority::tests::retained_token_revalidates_under_an_existing_write_epoch_without_reacquiring_gate
```

Formes enregistrées, uniquement dans la QA isolée admise ; `BIN_LIB` désigne le vrai artefact de test de bibliothèque du JSON Cargo, contrôlé par nom/kind/profile/chemin. `SELECTOR` est un nom exact de la liste. Chaque sélecteur a été lancé une fois après sa liste exacte ; zéro test sélectionné ne compte jamais PASS. Les plafonds du harnais sont 1200 s pour build/check, 8 s par liste et 45 s par parent ; le délai métier est inchangé.

```sh
cargo test --frozen --offline --lib --no-run --message-format=json
"$BIN_LIB" "$SELECTOR" --exact --list
"$BIN_LIB" "$SELECTOR" --exact --nocapture --test-threads=1
cargo check --frozen --offline --all-targets --message-format=json
```

Le check public donne `build-finished: success=true`, zéro erreur et **353 diagnostics d’avertissement**. Clippy strict n’a pas été exécuté. Les 2622 payloads/modes post-run correspondent au gel ; l’export source porte le SHA256 `6c308d094459e68d40860b109d43476d57e53abe266cf3788c99d92320ffac0f` (égalité des contenus/modes, pas du packing tar). Root a vérifié 337 empreintes de pièces brutes. Les commandes de collecte/fermeture rendent zéro, aucun OOM n’est enregistré et le conteneur possédé est ensuite absent. Le code 137 du holder reste distinct des sorties des tests.

## Identifiants de preuve et limites

Les pièces privées restent privées ; seuls leurs identifiants SHA256 vérifiés sont publiés ici.

| Pièce | SHA256 |
|---|---|
| Reçu du diagnostic de coût | `9dab51495bf4294fae5e03ed3110ca09f3587a63451023af745d19f96c7c55ed` |
| Reçu baseline | `6e6fadf10fa12d48869138b97c2ae747a4450f3c056c1606213c5bdb92876b43` |
| Reçu privé corrigé | `db786d559aed13a560c41f795c8765b2c62f8411a8058e0c6b1e16535ce3ed4d` |
| Revue source R2 ciblée, GO_SOURCE | `6623413665ce93105fb8161aa38a10aa321233446b83d244a78ab631c1e5319d` |
| Reçu de la composition publique réduite | `3dda47fd774a11765e07d7ff5284af5ac8a86fc341814ca78ed0c21aa267a97f` |

Le NO_GO R1 et les anciens champs NOT_RUN de la revue R2 restent inchangés. Les exécutions ultérieures ont leurs preuves distinctes. Aucune qualification générale de latence, disponibilité, panne de stockage ou production n’en découle. SSO/IdP réels, fournisseurs, workers/runtimes métier, suite complète et Clippy restent NOT_RUN dans ce lot. Aucun fingerprint d’audit ni gate de production n’est fermé. Voir le [contrat d’architecture](../../../ARCHITECTURE_CONTRACT.md), la [stratégie de tests](../../../TEST_STRATEGY.md) et les [gates de production](../../../PRODUCTION_READINESS.md).
