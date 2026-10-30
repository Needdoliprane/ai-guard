# Tests ciblés — bootstrap supervision schema 3

## Périmètre

Worktree `/Users/louis/Documents/ai-guard/.worktrees/broker-service`, branche `codex/broker-service`, base `f46a23ecb8d17f098d325c017536e7c8a146ed83`. Les fichiers de test possédés pour ce lot sont `tests/supervision_bootstrap.rs` et, sur autorisation distincte, la préparation du clone dans `tests/supervision_store.rs`. Aucun code applicatif n’a été modifié par ce testeur.

Les tests utilisent SQLite temporaires, le vrai binaire gateway et des identités synthétiques. Aucun `.env` ou credential réel n’a été consulté. Les enfants gateway sont détenus par un guard RAII avec arrêt et attente bornés, y compris lors d’une assertion en panique.

## RED avant implémentation

1. Première commande library : `CARGO_TARGET_DIR=/Users/louis/Documents/ai-guard/.worktrees/broker-supervision/target cargo test --locked --test supervision_bootstrap`.

Résultat : échec de compilation attendu, 4 erreurs E0599 car `Store::create_supervised_for_tenant` était absent. Code 101, aucun test exécuté ; ce n’était pas un constat de défaut runtime. Logs/manifeste : `/tmp/ai-guard-supervision-bootstrap-red-20261030/`. Test SHA au RED : `6ea8ab876cbe5e067bdad2287b01e799071c5f965524d6831bd72fa3079265a8`; `src/audit.rs` `112b1044b2be813f47a3ab6adcfda94942fee05702bbb04d57313b99dfe877a3`; `src/main.rs` `48d73be68241017ef27ce36fd52a4ede72920f5f61ea0c7310fce4cd196893c1`.

2. Avant l’implémentation du CLI, le test d’overlap a échoué sur `unexpected argument '--initialize-supervised-stores'`, pas sur le diagnostic attendu. Code 101, un test exécuté. Log/manifeste : `/tmp/ai-guard-supervision-bootstrap-cli-red-20261030/`; au RED, `main.rs` `48d73be…893c1`, `gateway_config.rs` `e75156a…8e9f`.

## Cible bootstrap finale après gel app

Commande : `CARGO_TARGET_DIR=/Users/louis/Documents/ai-guard/.worktrees/broker-supervision/target cargo test --locked --test supervision_bootstrap`.

Résultat final : **7 PASS, 0 FAIL, 1 ignored helper**. Le helper enfant F_GETLK/`PRAGMA journal_mode=DELETE` est explicitement invoqué par les tests parents avant et immédiatement après les refus ciblés ; il n’est pas compté comme réussite produit. Dans le refus factory, le probe post-refus est la première action après `Err`, avant tout appel Store. Le probe du retry CLI suit aussi immédiatement le processus refusé.

Couverture exécutée : bootstrap library neuf avec backup schema 2 et Store vivant schema 3 actif ; réouverture en maintenance inactive ; création des parents cible/backup imbriqués absents avec modes privés ; refus tenant invalide avant création ; refus de DB ou backup préexistants sans mutation ni perte du verrou SQLite ; erreur post-copie avec `max_events=1` préservant cible/backup partiels lisibles en schema 2 ; gateway réel A/B avec réponses `/api/v1/control/protocol` authentifiées runner confirmant tenant, rôle, schema 3 actif et namespaces distincts ; backupRoot sibling accepté ; restart sans flag gardant les namespaces et l’état inactif ; retry du flag sur DB existantes refusé sans artefact de backup ; gateway single-tenant legacy sans flag restant schema 2.

Les chemins backupRoot identiques, ancêtres et descendants des répertoires credentials des deux tenants sont refusés avec diagnostic explicite avant création DB. Un alias Darwin `/var` ↔ `/private/var` est essayé uniquement si la machine vérifie device/inode identiques ; cette branche ne s’exécute pas si l’alias n’existe pas.

Log/manifeste avant/après : `/tmp/ai-guard-supervision-bootstrap-lock-probe-postfix-20261030/`, code 0, 4 secondes. Test SHA : `2ebddf5510e16954f043764ed071d98e236ad01bcb648777dd0acb6372097959`.

## Canaris ciblés Store et gateway

Commande groupée : `CARGO_INCREMENTAL=0 cargo test --locked --test storage_security --test audit_integrity --test supervision_store --test gateway_tenancy` a échoué avec code 101 après 20 secondes. `audit_integrity` (12 tests), `gateway_tenancy` (1 test) et `storage_security` (17 tests) ont passé. `supervision_store` avait 8 PASS et un FAIL : `pre_claim_sqlite_clone_cannot_reauthorize_armed_start` échouait à l’ouverture du clone SQLite créé avec les permissions par défaut du processus.

Reproduction historique sans édition sur le worktree baseline `f46a23ecb8d17f098d325c017536e7c8a146ed83`, sous `umask 022`, avec ce seul test exact : le même échec de mode 0600 s’est reproduit. Log : `/tmp/ai-guard-supervision-store-clone-baseline-20261030/`. La fixture dans `tests/supervision_store.rs` crée maintenant explicitement la destination en mode 0600 avant l’ouverture SQLite, sans élargir le code app.

Sous le même `umask 022`, la commande exacte a passé : `cargo test --locked --test supervision_store pre_claim_sqlite_clone_cannot_reauthorize_armed_start -- --exact` (1/1). Puis `cargo test --locked --test supervision_store` a passé 9/9. Logs : `/tmp/ai-guard-supervision-store-clone-fixed-exact-20261030/` et `/tmp/ai-guard-supervision-store-rerun-20261030/`. Les groupes précédemment verts n’ont pas été relancés.

## Formatage, Clippy et build release

`cargo fmt --all -- --check` : PASS, 0 seconde, manifeste complet avant/après égal.

`cargo clippy --locked --all-targets -- -D warnings` : première tentative FAIL seulement sur `manual_map` dans l’oracle Darwin de `tests/supervision_bootstrap.rs`. Correction mécanique de la fixture, puis nouvelle fmt check et Clippy : PASS, 1 seconde, manifestes complets avant/après égaux. La première tentative reste conservée dans `/tmp/ai-guard-supervision-bootstrap-final-validation-20261030/clippy/` ; réussite dans `clippy-final2/`.

`cargo build --release --locked` final : PASS, 0,18 seconde, manifeste complet de 112 fichiers avant/après identique. Binaire `agentsandbox` SHA-256 `3188c3a6848925a2995d7096fe2f5433d4a609e1eb7daa2c009e70dea5a19b39`; `libagentsandbox.rlib` SHA-256 `10d191a1c4e457b57bc358ae9698241ec69b0701036c5da46ee60f87e09376d4`. Preuves finales : `/tmp/ai-guard-supervision-bootstrap-final-validation-20261030/release-final/`.

Sources produit gelées : `src/audit.rs` `862d81fc2e74b032825c942aecbfeef73ed26e71f41d110c64ce3a639ec27bfb`; `src/main.rs` `6466e3a55232be117d32aa392d14fa0c8f479daf4d46f04412bede671129ffaf`; `src/gateway_config.rs` `0307bfb462f30f6e24288d67ec39e0523125338dc77ce13252957dc995ace89f`. Tests finaux : bootstrap `2ebddf5510e16954f043764ed071d98e236ad01bcb648777dd0acb6372097959`, supervision_store `b0f39559ba01682f64b91240a3bdbcfc3a2f12db1432c924eb10d175268422ad`.

## Limites

Ces preuves couvrent ces parcours locaux, pas la survie à une coupure électrique, le runtime worker, Docker/Podman, cloud ou la release complète. Aucun modèle, moteur ou service fournisseur n’a été lancé. Les alias Darwin conditionnels et les plateformes non testées restent hors preuve si non observés.
