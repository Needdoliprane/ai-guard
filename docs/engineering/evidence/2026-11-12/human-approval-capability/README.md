# Capacité personnelle ApproveLaunch — preuve ciblée du 12 novembre 2026

**Résultat : PASS logiciel borné sur quatre parents ciblés.** Cette preuve ne rend pas l’approbation humaine disponible et ne clôt aucun gate global. Voir [TESTS.json](TESTS.json) pour les RC et empreintes des quatre oracles, et [SOURCE_MANIFEST.json](SOURCE_MANIFEST.json) pour les archives, postimages et limites.

## Source et revue

Base publique `711ba1378672f5c66099b2dd78daee2b5e35019f`, plus exactement trois postimages gelées par le propriétaire. Archive owner `SOURCES.ustar` SHA-256 `674c3df14b9c1a727a4bc95eadfbb666cda49f23ca5b4537cfd6410e58f6328f`; source composée utilisée par QA SHA-256 `0b85b740279e59b285593e4730e4d983cacc43678aff1a370d96478c7c9cfdbe`, 2 513 fichiers réguliers `0644`. Revue indépendante `GO_SOURCE_BOUNDED` : `REVIEW_r1.md` SHA-256 `7d456d891f7315e48180cbed78f9d7ff578e6e45d59797d10dce078b2164e587`, `REVIEW_r1.json` `bb0a097653f22f75bac000ffd241787c0df4d07c361da677f4880c11ba0f19aa`. La revue source seule déclarait tests et compilation `NOT_RUN`.

## Ce qui a été vérifié

Les quatre tests Rust exacts réussissent avec un test passé, aucun échec ni test ignoré et aucun panic par parent :

1. `approval_capability_is_personal_and_not_a_legacy_resource_permission` — `ApproveLaunch` reste une capacité personnelle distincte; ce n’est pas une permission ressource ni une capacité d’administration.
2. `approval_membership_requires_exact_format_fourteen_even_when_inactive` — la barrière refuse les versions autres que 14, y compris une appartenance inactive. Il s’agit du helper de format, pas d’un test d’ouverture ou migration du format 14.
3. `legacy_provision_and_idempotent_mutation_refuse_approval_before_any_write` — les refus préservent l’état durable et le replay d’un ancien operation ID ne contourne pas la barrière.
4. `sqlite_full_during_identity_ddl_rolls_back_the_exact_schema_three` — régression SQLite voisine sur le rollback de DDL en cas de disque plein.

Le build lib `--no-run` et `cargo check --all-targets --frozen --offline` passent. L’export vivant vérifié est SHA-256 `fdb85c4633229eac56725c58c7111e8ecd6c342dbe7b0479f0c909592f7bf5cc`, 27 863 040 octets, mêmes 2 513 fichiers `0644` et 326 répertoires. Fermeture du conteneur exact RC 0; aucun OOM. Pic mémoire cgroup 6 233 681 920 octets sur 9 663 676 416; compteurs OOM à zéro. Profil Linux ARM64 épinglé, UID 1000, réseau absent, racine RO, deux binds RO, capabilités supprimées, NNP/seccomp 2, 4 CPU, swap 0, PID 128, scratch 4 Gio, Rust 1.99.1 hors ligne, jobs 1.

## Portée restante

`ApproveLaunch` est une capacité préparatoire/réservée, pas un parcours d’approbation disponible. Aucun opener, migration, activation du format 14, route, helper d’approbation, `Ready` ou flag CLI n’est ajouté. `ResourcePermissionV1` reste inchangée; aucun schéma JSON Identity n’est ajouté. Les trois PASS de capacité ne prouvent ni l’autorisation nominative complète ni l’exécution d’une approbation.

Les essais sont séparés : r1 a été rejeté comme archive QA invalide (PAX/AppleDouble, sans résultat produit); r2 a échoué au préflight des modes avant Cargo, avec quatre tests `NOT_RUN` et fermeture agrégée RC 1 (stop/stopped/remove/exact-ps composantes RC 0); seul r3 reçoit le PASS ci-dessus.

La preuve ne couvre pas Core epoch `7033`, Claude E `a710`, les nouveaux wires CAF2 identity/binding/store, ni provider, modèle, worker, cluster ou gate global.

`LUNA_EVIDENCE.sha256` du run r3 : SHA-256 `85acf9d1fa20ad25c81b481fa4a82095678930fc0cf1eb16318acd05075b1d30`. La vérification indépendante Root `ROOT_RESULTS_VERIFIED.json` SHA-256 `3f3fb8c8caf7a33fa523c0a4d00296be3a0ca1ad9fb37b396b50bc8c655d92b6` a été créée après ce manifeste et est citée séparément, sans prétendre qu’elle y figure.
