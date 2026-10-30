# Rapport de qualification ciblée — soumission broker / runtime préarmé

Date : 2026-10-31. Worktree : `/Users/louis/Documents/ai-guard/.worktrees/broker-service`, HEAD `7098e204a91182229c1a67bc8017271cb3d20918`. Hôte : Darwin 27.2.0 arm64 (T8122), rustc/cargo 1.99.1. Cache Cargo pour les commandes finales : `/Users/louis/Documents/ai-guard/.worktrees/broker-supervision/target`.

Ce lot couvre uniquement les coutures Rust de préparation/reprise d’une soumission broker et le runtime préarmé. Aucun moteur, fournisseur distant, modèle, worker réel, Docker ou parcours E2E n’a été lancé.

## Résultats finaux

- `tests/broker_submission.rs`, SHA-256 `1b5b7b31d4bb37bc0de5c9ae272b7cd076d7b7cfda546d07a6c76c2a4579954f` : PASS 5/5, 0,45 s. Commande : `cargo test --locked --test broker_submission -- --test-threads=1`. Sortie : `broker-submission-attempt3.log`.
- `tests/runtime_prearmed.rs`, SHA-256 `ac8901fcaac3d9944daaa40bebc41d86825dd1f12332b64584821c049c84a24f` : PASS 4/4, 13,94 s. Commande : `cargo test --locked --test runtime_prearmed -- --test-threads=1`. Sortie : `runtime-prearmed-final.log`.
- Canari `credential_handoff::shared_snapshot_survives_handoff_and_native_cli_reads_its_exact_prepared_bytes` : PASS 1/1, 0,38 s. `canary-credential-handoff.log`.
- Canari `runtime_managed` : PASS 14/14, 25,90 s. `canary-runtime-managed.log`.
- `cargo fmt --all -- --check` : PASS, RC 0 sur les SHA finaux (`fmt-final.log`). Le précédent `fmt-check.log` précède le dernier ajustement stylistique du test runtime.
- `cargo clippy --locked --all-targets -- -D warnings` : PASS, RC 0 (`clippy-all-targets-final.log`).
- `cargo build --release --locked` : PASS, RC 0, 38,14 s (`release-build.log`).
- `git diff --check` : PASS.

Les sources de test exactes sont archivées en mode 0600 dans `final/`. SHA applicatifs observés durant la validation : `src/runner.rs` `ba72d730ee8d611196bf1130ab8bfc9bd56ad1d3b7f9ea5996b1221daa5efac2`; `src/runner/broker.rs` `83bc1e6ab623bc6cc2b6314f0e0b32c4007b0c4c1bda02a0e693f297d1a8930b`; `src/runtime.rs` `8d63a1caaf3f3b2ef186813856f9213be029a8662fdb911552b96f5546500d22`; `src/runtime/managed.rs` `a2a5be94e7cfec58b440195805ab006c80d50807a273e100b6d03c53265b703e`.

## Oracles couverts

`broker_submission` exerce un vrai Journal à fichiers privés avec données synthétiques : UUID imposé, initialisation None-binding, reconstruction pure depuis le job, refus nil/approve_hash et littéraux hors borne avant IO, bindings manuel et fournisseur figés avant rotation de leurs sources, comparaison des octets du handoff et de leur SHA avec le manifeste durable, validation après drop des objets job/prepared, détection d’altération, et replay fournisseur sans `providers.json` ni répertoire legacy `credential-bindings`. Aucun Store SQLite n’est instancié dans cette cible. Les secrets de test ne sont pas imprimés.

`runtime_prearmed` utilise une fixture technique CLI Rust, pas un moteur produit. Elle couvre le guard avant spawn et l’ordre des opérations, refus stop déjà vrai/émetteur clos, erreur du guard, ObserveOnly, start récupéré, captures/chemin terminal. L’oracle EOF garde le premier futur `on_started` pending et vérifie qu’il n’est pas rappelé après EOF. La fixture envoie réellement SIGINT avec `libc::kill` au processus enfant de test qui possède son handler Tokio ; le signal n’est pas envoyé au processus hôte. Le watcher du service reçoit bien ce signal, tandis que le receiver stop de session reste inchangé. L’état moteur/enforcer et son transport restent simulés ; le test ne qualifie pas un moteur réel.

## Historique des essais intermédiaires

Ces rejouements et défauts de fixture ne sont pas additionnés aux résultats finaux.

- Le RED callback/EOF sur runtime GEL1 a reproduit le défaut produit : `on_started` était rappelé après EOF (deux appels), RC 101. Les preuves sont dans `/tmp/ai-guard-prearmed-gel1-callback-qa/`; le snapshot applicatif GEL1 dans `/tmp/ai-guard-submission-prearmed-gel1.57LMSX`. Le correctif GEL2 a passé ce même oracle dans la cible finale.
- Le premier `--no-run` runtime a échoué sur E0382 dans la fixture de test (emprunt après déplacement de `root`), avant toute exécution comportementale. Trace conservée dans `/tmp/ai-guard-prearmed-gel1-callback-qa/no-run.log`; corrigée dans le harnais avant la reproduction GEL1.
- Premier `broker_submission` avant canonicalisation des répertoires temporaires : 2 PASS / 3 FAIL, car les chemins `/var` symlinkés sur macOS sont correctement refusés par `Directory::open_existing`. Log `/tmp/ai-guard-broker-service-cycle-gel2/broker-submission.log`. Correction limitée aux racines synthétiques des tests, sans relâcher les contrôles produit.
- Premier runtime GEL2 : 3 PASS / 1 FAIL parce que l’oracle cherchait zéro événement `start` dans tout le log, qui contenait déjà un start antérieur valide. Log `/tmp/ai-guard-broker-service-cycle-gel2/runtime-prearmed.log`. L’assertion a été corrigée pour comparer avant/après l’action ; cible complète ensuite PASS.
- En ajoutant le replay fournisseur, un premier essai a échoué sur `DirectoryNotEmpty` parce que la fixture n’avait pas vidé tous les fichiers synthétiques du répertoire legacy. Cette sortie est restée dans la transcription d’outil et n’a pas été capturée en fichier brut ; seule la cause est rapportée, sans reconstituer un faux log. Le suivant, capturé dans `broker-submission-attempt2.log`, a échoué parce que le replay rouvrait le chemin de configuration via l’alias `/var`. La fixture supprime désormais uniquement les fichiers réguliers synthétiques, exige l’absence du répertoire, et rouvre le chemin canonique. Le run final PASS est `broker-submission-attempt3.log`.
- Un Cargo a été explicitement interrompu RC 130 après découverte d’un `CARGO_TARGET_DIR` mal formé avec un niveau `broker-service` supplémentaire. Aucun test n’a été compté et aucun fichier n’a été nettoyé. Log `/tmp/ai-guard-broker-service-cycle-gel2/runtime-prearmed-v4.log`; le cache partiel est conservé. Tous les runs finaux impriment le chemin cible absolu correct.
- Une tentative Clippy a détecté uniquement `collapsible_if` dans le test runtime. La condition a été réduite sans changement sémantique ; la cible runtime exacte a été rejouée (4/4) et le Clippy strict final a passé. Logs `clippy-all-targets.log` et `runtime-prearmed-final.log`.

## Limites

Qualification exécutée sur macOS arm64. Les fixtures CLI ne qualifient ni Docker/Podman ni un moteur réel, et ne démontrent pas le confinement d’un worker métier. Authentification réelle fournisseur, conversations, supervision durable complète et cloud sont hors de ce lot. Ce rapport n’est pas une qualification globale de production.
