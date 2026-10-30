# Qualification logs engine — test réel ciblé

Date : 30 octobre 2026. Un seul cas de test moteur, sans modèle ni fournisseur externe. Aucun changement de code applicatif; le seul fichier modifié est tests/runtime_terminal_persistence.rs.

## Commandes et versions

Toolchain : rustc 1.99.1 (58cf8f9c3 2026-10-13), cargo 1.99.1. Docker CLI 29.2.1, Engine 29.9.2. Image utilisée sans rebuild ni retag : agentsandbox-worker:qualification-20261029-runtime-resume-9be5e984, ID sha256:7c64760cc4a5f29a40685cae2451afed914ce81671601166792cd087beab9db6.

Contrôles précédant le runtime :
- cargo fmt --all -- --check — PASS.
- cargo test --locked --release --test runtime_terminal_persistence --no-run — PASS.
- shasum source test: 816f8294b0fae4dfc40cbdcbb6b04b202c3dd02778d25c8472c827d4da2fb0d9  tests/runtime_terminal_persistence.rs.

Exécution exacte (répertoire du dépôt, login:false, PATH Rust/Docker explicite) :

    AGENTSANDBOX_TEST_RUNTIME=docker
    AGENTSANDBOX_TEST_IMAGE=agentsandbox-worker:qualification-20261029-runtime-resume-9be5e984
    AGENTSANDBOX_TEST_ALLOW_ROOTFUL=1
    AGENTSANDBOX_TEST_RUNTIME_PROGRAM=/opt/homebrew/bin/docker
    AGENTSANDBOX_TEST_RUNTIME_ENDPOINT=unix:///Users/louis/.docker/run/docker.sock
    cargo test --locked --release --test runtime_terminal_persistence rejected_terminal_audit_keeps_the_real_terminal_instance_inspectable -- --ignored --exact --nocapture

Le test crée son serveur SQLite loopback réel, son projet temporaire et un fichier env factice privé à 0600 hors projet. Il génère une canary fictive pour TEST_CANARY dans le test; sa valeur n’est pas reproduite ici. Aucun credential réel, modèle ou appel réseau fournisseur n’est utilisé. Le lancement est provider=generic, observe, bridge explicite, timeout 20 s et capture activée. L’import workspace passe par son helper sans credential; le worker Docker est inspecté avec son CLI approuvé et son endpoint Unix explicite. Aucun champ Config.Env ni inspection intégrale du conteneur n’a été demandé.

## Résultats

- **PASS — scénario final corrigé** : test exit 0, 1 test passé. Sortie du test : container 7acd3db951f90935c0466e22a06c4b711c3ecbfa34beda26c2977cbe6ed5a0d6; session 0c19293e-1c34-4358-8f62-6eaf7341249c. Preuve privée conservée sous /var/folders/ny/3tnyr3194xq7zt80kp9_g5bm0000gn/T/ai-guard-terminal-audit-sP3sFc.
- **PASS — invariants terminal refusé** : le middleware injecte exactement un HTTP 503 avant que le gateway/Store ne voie l’accusé terminal. Le même container stopped/exited 0 reste inspectable; worker est InDoubt, quota held. Usage/allocation reste absent et la réservation correspondante demeure tenue. Vérification d’intégrité audit passe avant et après réouverture SQLite. Le projet original reste inchangé.
- **PASS — preset moteur** : docker inspect limité montre Image ID ci-dessus, State=exited, ExitCode=0, LogConfig.Type=none et LogConfig.Config={}. Le test vérifie aussi Tty=false, OpenStdin=false, StdinOnce=false pour le worker.
- **PASS — sortie et masquage** : stdout du worker est capturé; le checkpoint privé contient le champ de sortie marqué [REDACTED] et ne contient pas la canary fictive. La requête API d’événements n’expose pas la canary. La réponse d’erreur runner vérifiée non plus.
- **PASS — lecture du journal moteur** : appel du CLI Docker approuvé avec endpoint explicite, commande logs sur l’ID immuable. stdout de Docker logs = 0 octet; stderr = 79 octets; commande non-success, diagnostic attendu avec LogConfig none. Le contenu de stderr n’a pas été affiché ni sauvegardé. Aucun transcript worker ne peut donc être confondu avec ce diagnostic stderr.

## Échec initial conservé et diagnostic

Le premier lancement du même test utilisait /usr/bin/printenv TEST_CANARY, qui se termine presque immédiatement. Le processus est devenu terminal avant que le callback de démarrage ait terminé son observation. Le middleware a renvoyé HTTP 503 pendant cette phase préalable à finish_execution; le checkpoint privé montrait alors output_event absent, et l’assertion de capture échouait. Le worker avait déjà été créé et était inspectable, mais ce run n’a pas validé la persistance de capture. Aucune cause produit n’est établie par cet essai seul.

Pour la relance autorisée, seule la fixture de commande test a été retardée brièvement dans le worker avec Python déjà présent dans l’image, puis a écrit TEST_CANARY sans saut final. Le callback de démarrage a ainsi été acquitté avant l’observation terminale refusée. La capture masquée existait alors et tous les invariants sont passés. Ce timing n’établit pas une garantie de rendez-vous pour toutes les durées; il évite seulement la sortie immédiate de cette canary de test. Aucun retry supplémentaire ne sera effectué dans cette mission.

Preuve du premier essai, fail assertion et phase : /tmp/ai-guard-engine-logs-real-docker-20261030.log; container retenu b016daf45a1376ea4481404e0f13439feb2a7874e5ff157827b97e5a6200a303, session ebff7d48-896d-45e7-9cf7-f94dfe3e5cc2, preuve sous /var/folders/ny/3tnyr3194xq7zt80kp9_g5bm0000gn/T/ai-guard-terminal-audit-YxzDb3.

## Artefacts, rétention et portée

- Résumé final privé : /tmp/ai-guard-engine-logs-real-docker-retry-20261030.log.
- Compilation release ciblée : /tmp/ai-guard-engine-logs-real-compile-20261030.log.
- Format : vérification PASS sans log séparé.
- Les deux containers et volumes issus des tests restent retenus conformément au scénario ACK 503; aucun cleanup manuel, prune ou suppression n’a été fait.
- Les répertoires de preuve gardent leurs fichiers de configuration/envfactice privés. Le fichier env et les snapshots credentials sont des sources privées intentionnelles et n’ont pas été lus/scannés comme sorties publiques.
- Le résultat qualifie seulement ce test ciblé sur Docker Desktop ARM64 avec l’image ci-dessus. Podman, autres plateformes, tests généraux de logs, TUI et toute couverture transitive restent non qualifiés.


## Contrôle post-édition final

Après le test moteur, clippy ciblé a signalé un guard Mutex vivant jusqu'à l'await d'arrêt du serveur. Seule la portée lexicale du guard a été raccourcie dans le test; aucune assertion, commande worker, configuration ou production code n'a changé. Le contrôle a ensuite donné : cargo fmt --all -- --check PASS; cargo clippy --locked --test runtime_terminal_persistence -- -D warnings PASS. Aucun test ou moteur n'a été relancé. Hash final du seul fichier dépôt modifié : 816f8294b0fae4dfc40cbdcbb6b04b202c3dd02778d25c8472c827d4da2fb0d9 tests/runtime_terminal_persistence.rs. Logs post-édition : /tmp/ai-guard-engine-logs-postedit-fmt-20261030.log et /tmp/ai-guard-engine-logs-postedit-clippy-20261030.log.
