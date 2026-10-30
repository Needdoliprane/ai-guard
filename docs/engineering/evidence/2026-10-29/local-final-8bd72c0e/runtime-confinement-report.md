# Qualification runtime reprise — 2026-10-29

## Portée exacte

Contrôles réels de l'adaptateur `Runtime::execute` sur **Docker Desktop macOS ARM64**, avec daemon Linux ARM64 dans sa VM et exception rootful explicitement autorisée. Ceci n'est ni une preuve rootless, ni Linux x86_64, ni Podman/Kubernetes, ni une qualification complète du produit. Aucun gateway/dashboard ni compte externe lancé. Aucun service existant perturbé ; aucune configuration Desktop changée. Aucun volume supprimé. Le projet original n'est jamais monté dans un worker.

Les répertoires `.agents/` et `.claude/` restent absents : chargement automatique des skills projet non confirmé. Le contrat d'architecture, CODING_RULES, PRODUCTION_READINESS, TEST_STRATEGY et les règles RTK ont été lus avant ce lot.

## Versions et artefacts observés

- Hôte de test : macOS ARM64 ; Rust 1.99.1 selon toolchain du dépôt.
- Docker CLI **29.2.1**, API 1.53, darwin/arm64.
- Docker Desktop **4.97.0 (244584)** ; daemon **29.9.2**, API 1.56, linux/arm64, overlay2.
- containerd **2.4.2**, runc **1.5.2**, docker-init **0.19.0**.
- SecurityOptions : `name=seccomp,profile=builtin`, `name=cgroupns`. `doctor` retourne `rootless:false`.
- `target/release/agentsandbox doctor --runtime docker` : refus attendu de l'absence rootless.
- `target/release/agentsandbox doctor --runtime docker --allow-rootful-runtime` : **PASS**, daemon joint, `rootless:false`.
- Inventaire initial : aucune image `agentsandbox*`, aucun conteneur actif. Aucun tag utilisateur remplacé.

Image construite : `agentsandbox-worker:qualification-20261029-runtime-resume-9be5e984`.

- ID image locale : `sha256:7c64760cc4a5f29a40685cae2451afed914ce81671601166792cd087beab9db6`, `linux/arm64`.
- RepoDigests vide : image construite localement, aucun digest de publication revendiqué.
- Base `rust:1.99.1-bookworm` : `sha256:9181b56c2a21ab1d5e73cffe04fc762426d30aa7fac3c91141dd033f49a7fc8b`.
- Base `node:24-bookworm-slim` : `sha256:c86ecead9e55a5210ac1727de38a4ca840bf03dcaf34dc9749addbf383b1920c`.
- Aucun Codex/Claude provider installé par argument de build.

Commande **PASS** (Dockerfile via stdin, contexte vide confirmé 2 B) :

```sh
docker build --tag agentsandbox-worker:qualification-20261029-runtime-resume-9be5e984 --label ai-guard.qualification=runtime-resume-20261029 - < images/worker.Dockerfile
```

Log build : `/tmp/ai-guard-worker-resume-build.log`. Handle de build attendu jusqu'à sortie 0, aucun build dupliqué après timeout d'observation.

## Changement de test et commande réelle

Nouveau fichier du dépôt : `tests/runtime_confinement.rs`. Test explicitement ignoré par défaut ; exige `AGENTSANDBOX_TEST_RUNTIME` et `AGENTSANDBOX_TEST_IMAGE`, et l'exception `AGENTSANDBOX_TEST_ALLOW_ROOTFUL=1` pour ce daemon. Il appelle le runtime Rust existant, ne substitue pas un moteur factice et ne change aucune règle de lancement.

```sh
AGENTSANDBOX_TEST_RUNTIME=docker \
AGENTSANDBOX_TEST_IMAGE=agentsandbox-worker:qualification-20261029-runtime-resume-9be5e984 \
AGENTSANDBOX_TEST_ALLOW_ROOTFUL=1 \
cargo test --locked --test runtime_confinement -- --ignored --nocapture
```

Résultat final : **PASS**, 1 test réel, 0 échec, 0 ignoré, durée 3.08 s. Log : `/tmp/ai-guard-runtime-confinement-second.log`.

Premier essai **FAIL**, conservé dans `/tmp/ai-guard-runtime-confinement.log`, car la nouvelle assertion imposait exactement `lo` dans `/sys/class/net`. Diagnostic réel : Docker Desktop crée aussi des devices tunnel sans route ; cette hypothèse de test était invalide. Le test final vérifie l'absence de routes IPv4, routes IPv6 uniquement sur `lo`, `NetworkMode=none` inspecté et une tentative de connexion refusée (curl exit 7 vers TEST-NET-1 `192.0.2.1`, sans DNS/proxy). Le code de production n'a pas été modifié pour obtenir ce résultat. Diagnostic conservé `/tmp/ai-guard-network-diagnosis.log`.

## Critères acceptés par preuves réelles

| Critère | Résultat et observation |
|---|---|
| Identité / privilèges | **PASS** : utilisateur inspecté `1000:1000` ; `/proc/self/status` UID/GID réels/effectifs/sauvegardés/filesystem 1000 ; CapEff/CapPrm/CapBnd zéro ; NoNewPrivs=1 ; Seccomp=2 |
| Racine readonly | **PASS** : `ReadonlyRootfs=true` inspecté et `touch /readonly-probe` refusé « Read-only file system » |
| Privilèges / mounts | **PASS** : Privileged=false, CapDrop ALL, sécurité NNP inspectée ; aucune bind mount ; volume nommé exact sous `/workspace`, seules destinations supplémentaires autorisées `/tmp` et `/home/agent` ; `/run/secrets` absent |
| Réseau none | **PASS** pour les effets testés : NetworkMode none, aucune route IPv4, IPv6 sur lo, connexion HTTP au réseau documentation refusée exit7 ; aucun filtrage bridge testé |
| Limites CPU/RAM/PID | **PASS allocation** : HostConfig Memory=1073741824, NanoCpus=2000000000, PidsLimit=256 ; cgroupv2 `memory.max=1073741824`, `pids.max=256`, `cpu.max=200000 100000` ; saturation/OOM/PID stress **NOT_RUN** |
| Timeout broker | **PASS** : `sleep 60` avec délai 1 s arrêté, `timed_out=true`, exit nonzero, retour <30 s ; volume retenu ; arrêt brutal daemon **NOT_RUN** |
| Copie/export | **PASS** : archive filtrée importée dans volume, fixture `.env` exclue ; fichier créé dans worker exporté vers destination neuve ; source intacte et sans fichier généré |
| Audit/autorisation E2E | **NOT_RUN** : test d'adaptateur direct dans contrôle de confiance, sans gateway. Aucune promesse de lancement autorisé durable dérivée de ce test |

## Ressources conservées

- Essai final : session `691610ec-9200-4572-aae3-ff402eb7eea5`, volume `agentsandbox-work-691610ec-9200-4572-aae3-ff402eb7eea5` ; source et export sous `/var/folders/ny/3tnyr3194xq7zt80kp9_g5bm0000gn/T/ai-guard-runtime-confinement-eSmKQf`.
- Premier essai : session `3042d51c-b50b-4ae4-b560-b0eb4bc7861b`, volume `agentsandbox-work-3042d51c-b50b-4ae4-b560-b0eb4bc7861b` ; source sous `/var/folders/ny/3tnyr3194xq7zt80kp9_g5bm0000gn/T/ai-guard-runtime-confinement-j57ASl`.
- Conteneur diagnostic arrêté, conservé : `ai-guard-net-diagnosis-20261029-9be5e984`, Exited(0).
- Les conteneurs de chaque opération sont nettoyés par le runtime normal ; inventaire final n'a aucun conteneur actif portant le label de session. Les volumes restent présents.

## Vérifications logicielles de ce fichier

- `cargo test --locked --test runtime_confinement` : compilé, 1 ignoré par défaut (**NOT_RUN runtime**, comme voulu).
- `cargo clippy --locked --test runtime_confinement -- -D warnings` : **PASS**.
- `rustfmt --edition 2024 tests/runtime_confinement.rs` : appliqué ; contrôle global et suite complète à intégrer par le parent.

## Écarts restant ouverts

Audit/autorisation E2E, contrôle conteneurisé, reprise après panne, saturation ressources, refus avec absence d'effet, A1/A2/B1, quota 0–100 par tenant, registre/relais/usage : **NOT_RUN** ou **BLOCKED** par les fonctions non encore livrées ; ce test ne les simule pas. Podman absent : **BLOCKED**. Kubernetes sans adaptateur/cluster autorisé : **BLOCKED**. Aucune qualification production globale revendiquée.

## Complément après revue indépendante — 2026-10-29

Deux défauts du harnais (pas du runtime de production) ont été reproduits puis corrigés dans `tests/runtime_confinement.rs` :

1. Une erreur quelconque de `ls /run/secrets` était acceptée comme absence. Le test utilise désormais `/bin/ls -ld /run/secrets` et exige exit 2 plus diagnostic complet, non tronqué et exact `/bin/ls: cannot access '/run/secrets': No such file or directory` dans la locale configurée C.UTF-8. Une permission refusée, erreur I/O ou sortie tronquée ne prouve pas l'absence. Le libellé accepté est **« /run/secrets absent »**, sans prétendre une absence universelle de tout secret dans l'image ou tout chemin.
2. Le callback `on_started` utilisait des assertions et pouvait paniquer avant le chemin de nettoyage de `Runtime::execute`. `validate_configuration` retourne désormais `Result`/`ensure`, sans unwrap/assert dans ce callback. Un contrôle négatif réel vérifie qu'une mauvaise attente de volume après démarrage retourne l'erreur attendue et que le conteneur de cette opération n'existe plus ; il réutilise le volume de la session d'essai.

Reproduction sans moteur ni volume : `/tmp/ai-guard-runtime-review-red.log`, **2 FAIL**, permission/I/O acceptée et panique sur mauvais utilisateur. Vérification finale : `/tmp/ai-guard-runtime-review-green-final.log`, **3 PASS / 1 ignored** (le moteur ignoré est NOT_RUN dans cette commande). Les cas testent permission/I/O, ENOENT complet vs tronqué, et rejet config sans unwind.

Un essai intermédiaire réel `/tmp/ai-guard-runtime-review-real.log` a **FAIL** sur l'empreinte trop courte `ls:` ; le vrai argv0 est `/bin/ls:`. Le diagnostic exact est conservé `/tmp/ai-guard-enoent-diagnosis.stderr` ; l'assertion n'a pas été assouplie pour accepter d'autres erreurs.

Rejeu réel final avec la même image/digest ci-dessus : `/tmp/ai-guard-runtime-review-real-final.log`, **1 PASS**, 0 échec/ignoré, **3.74 s**. Tous les critères réels précédents passent, plus le rejet `on_started` avec nettoyage et le diagnostic ENOENT strict. `cargo clippy --locked --test runtime_confinement -- -D warnings` et `rustfmt --check --edition 2024 tests/runtime_confinement.rs` : **PASS**.

Empreinte source relue : `4ca3796f2f29835b3ee73c7d46c92ae848e3b94c39d135977cfed3fd84fa3e8a`. `/root/review_front` a confirmé cette empreinte, relu les logs finaux et fermé les deux constats sans nouveau bloquant. Aucun fichier `src/runtime.rs` modifié par ce lot.

Ressources supplémentaires conservées :

- Session intermédiaire `1f7f57d4-b728-4540-b6b9-c376f520a45d`, volume `agentsandbox-work-1f7f57d4-b728-4540-b6b9-c376f520a45d`, preuves sous `/var/folders/ny/3tnyr3194xq7zt80kp9_g5bm0000gn/T/ai-guard-runtime-confinement-HHNQwh`.
- Session finale `1dd5b077-61cb-42c9-a74d-df5ea04e6179`, volume `agentsandbox-work-1dd5b077-61cb-42c9-a74d-df5ea04e6179`, source/export sous `/var/folders/ny/3tnyr3194xq7zt80kp9_g5bm0000gn/T/ai-guard-runtime-confinement-FumSvO`.
- Conteneur diagnostic ENOENT arrêté et conservé : `ai-guard-enoent-diagnosis-20261029-9be5e984`.

Ces résultats finaux remplacent la qualification limitée du premier harnais sur les deux points ci-dessus ; les premiers FAIL et leurs logs restent conservés. Les limites audit E2E/multi-client/plateformes restent inchangées.
