# Recette trois workers — état et cible

La recette obligatoire sur le poste du mainteneur lance trois vrais workers en parallèle, A1/A2 chez A et B1 chez B, avec contrôle séparé. Trois à cinq workers suffisent à la validation fonctionnelle locale. La mesure de capacité à 100 workers actifs par tenant relève d’un environnement dimensionné pour cette charge. [Critères de la recette](TEST_STRATEGY.md)

Observer les trois workers actifs pendant une même période, avec leur identifiant d’exécution, un volume de travail propre et un résultat distinct pour chacun. Arrêter A1 pendant que A2 et B1 poursuivent leur exécution, puis vérifier leurs résultats et la portée des traces côté CLI et dashboard. Conserver les vérifications de rôles, de refus inter-tenant et du relais A1→A2 soumis à autorisation.

## Rejouer la concurrence sur Docker

Le test opt-in utilise trois projets synthétiques, deux bases tenant et le vrai CLI. Il vérifie le chevauchement des intervalles observés par Docker, les quotas actifs, la fin normale d’A1 pendant qu’A2 et B1 restent actifs, les volumes et exports distincts, les allocations non facturables et les deux chaînes d’audit. Chaque viewer ouvre le détail de ses workers et reçoit HTTP 404 pour les identifiants des workers de l’autre tenant. Les plafonds cumulés des workers sont de 6 vCPU et 3 Gio de RAM, hors contrôle. L’arrêt explicite, le relais et le dashboard font partie de la recette complète et restent à vérifier séparément.

Le gel `434c1400` de la candidate a vérifié les trois workers A1/A2/B1 en parallèle sur Docker Desktop ARM64, avec deux tenants et 20,08 s de chevauchement. A1 s’achève pendant qu’A2 et B1 sont actifs. Les exports, quotas, allocations, chaînes d’audit et refus d’accès aux détails inter-tenant passent. L’arrêt explicite, le relais et le dashboard sous la même charge gardent leurs propres recettes. [Résultats et empreintes](evidence/2026-11-19/functional/native17-and-local-workers.md#trois-workers-a1-a2-et-b1-après-redémarrage)

Fournir le chemin absolu du client Docker, son socket Unix local et l’identifiant SHA-256 complet d’une image QA déjà présente, avec `/bin/sleep`, `/bin/tar` et le profil de worker. Le compte courant doit être reconnu par le prévol de contrôle. Sur un moteur rootful approuvé, ajouter `AGENTSANDBOX_TEST_ALLOW_ROOTFUL=1` à l’environnement du test.

Sélectionner une image QA locale par son SHA-256 complet. Elle fournit `/bin/sleep` et `/bin/tar`. Le répertoire `/workspace` existe dans l’image et appartient à `1000:1000`, comme dans le Dockerfile worker. Le volume neuf hérite de ce répertoire à l’import. Dans l’image de contrôle testée, `tar` a reçu un refus d’écriture en UID 1000. L’image QA préparée sans réseau à partir de Debian local a passé l’extraction sur un volume neuf.

```sh
AGENTSANDBOX_TEST_RUNTIME=docker \
AGENTSANDBOX_TEST_RUNTIME_PROGRAM="$QA_DOCKER_BIN" \
AGENTSANDBOX_TEST_RUNTIME_ENDPOINT="$QA_DOCKER_UNIX_ENDPOINT" \
AGENTSANDBOX_TEST_IMAGE="$QA_WORKER_IMAGE_ID" \
  cargo test --locked --offline --test runtime_user_journey \
  real_cli_runs_three_independent_workers_across_two_tenants \
  -- --ignored --exact --nocapture
```

Le test affiche son verdict et conserve ses dossiers privés après le prévol. `BLOCKED` au stade `doctor` indique que la recette n’a démarré aucun worker. Un essai sans moteur est exclu des preuves de concurrence.

Sur Docker Desktop rootful approuvé, fournir `AGENTSANDBOX_TEST_ALLOW_ROOTFUL=1` au processus de test. Les workers de la recette utilisent le profil strict et le réseau coupé.

### Vérifier l’arrêt explicite d’A1 par le CLI

Avec les mêmes variables moteur, sélectionner `real_cli_explicit_stop_releases_only_a1_across_two_tenants` et laisser les variables Chromium absentes. Le test attend les quotas 2/1, le démarrage audité d’A1 et l’epoch ouvert du moniteur de cette tentative. Il appelle une seule fois `worker-stop` et rapproche son reçu de l’événement durable de contrôle. Le même conteneur doit sortir avant l’échéance et avant la fin naturelle prévue. A2 et B1 gardent leur quota et finissent normalement. Les trois exports, les allocations et les audits restent vérifiés.

Pour isoler un refus `account_home_unavailable`, lancer le diagnostic passif avec le même compte que la recette. Le répertoire `no-config` doit être absent dans le dossier de diagnostic neuf.

```sh
qa_account_probe=$(mktemp -d)
target/debug/ai-guard-diagnose --json --runtime docker \
  --runtime-program "$QA_DOCKER_BIN" --config-dir "$qa_account_probe/no-config"
```

Avec ce refus, le compagnon rend le code de sortie 3. Il indique `engine.program_refused` avec la raison `account_home_unavailable`, puis `engine.state_not_checked`. La commande lit l’autorité du compte et du programme sans démarrer Docker. Refaire le même diagnostic dans une session de connexion du même compte permet de distinguer un écart du contexte de lancement d’un compte indisponible dans les deux sessions.

Après le prévol, le test imprime le chemin du dossier privé retenu. Il conserve les trois configurations et exports. La réussite exige que le CLI et le registre confirment le retrait des conteneurs. Les volumes de travail sont conservés pour l’export. En cas d’interruption, reprendre uniquement les sessions et ressources identifiées dans ce dossier, avec la même autorité moteur et l’origine gateway enregistrée. [Réconciliation et rétention](../operations.md#consulter-arrêter-et-reprendre-un-worker)

### Vérifier le dashboard des trois workers dans Chromium

Dans `front`, exécuter `npm ci` puis `npm run build` avec le Node du dépôt. Faire pointer `QA_FRONT_DIR` vers le dossier `front/dist` de ce même checkout. Fournir le programme Node absolu, le dossier `node_modules` contenant Playwright et le programme Chromium absolu. L’installation de Playwright et Chromium est distincte de celle du bundle. Ajouter ces variables à l’environnement de la recette Docker. Le conducteur utilise le sandbox natif de Chromium et la garde Playwright vers le dashboard loopback. Sur macOS, un profil de processus supplémentaire peut bloquer l’IPC ou l’initialisation du sandbox natif.

```sh
export AGENTSANDBOX_TEST_BROWSER_DIST="$QA_FRONT_DIR"
export AGENTSANDBOX_TEST_BROWSER_NODE="$QA_NODE"
export CODEX_PRIMARY_RUNTIME_NODE_MODULES="$QA_PLAYWRIGHT_MODULES"
export PLAYWRIGHT_CHROMIUM_EXECUTABLE="$QA_CHROMIUM"
```

Le conducteur `front/tests/browser_runtime_journey.mjs` démarre après la fin des workers, avec le même gateway. Il vérifie les deux vues tenant, trois refus 404, les quotas, les allocations et la correspondance de six événements avec les reçus CLI. Le test conserve le verdict `browser` et écrit le HTML et le PNG masqués en cas d’échec sur une page authentifiée. La fenêtre de chevauchement réelle reste contrôlée par le harnais Rust.

Pour vérifier aussi ce que Chromium affiche pendant le chevauchement, rejouer la même recette en mode distinct avec `AGENTSANDBOX_TEST_BROWSER_LIVE=1`. Le navigateur prépare deux vues privées avant la soumission. Au signal du harnais Rust, il relie l’horodatage du registre rendu à une seule réponse complète du bon tenant. Il vérifie les lignes A1/A2 et B1, leur session, leur état et leur quota. Si une ligne requiert de défiler, il fait défiler la page puis vérifie à nouveau la date et la visibilité réelle des champs. Toute relecture lors de la mesure demande une nouvelle observation dans le délai prévu. Une seule inspection Docker après le rendu confirme les trois conteneurs encore actifs et leur continuité avec les observations finales. Après l’inspection positive, les deux viewers se déconnectent dans leur page. Le test vérifie le retour au formulaire vierge, le retrait des données et l’annulation du flux de chacun. Les déconnexions et la fermeture du navigateur doivent rendre leur verdict dans les 15 secondes. Un échec sur la page authentifiée conserve le HTML assaini si la capture est disponible. Pour les reçus, les allocations et les refus 404 dans Chromium, utiliser le mode terminal du paragraphe précédent.

## Rejouer la concurrence sur Podman

Utiliser le client Podman absolu, le socket Unix sélectionné et le digest complet d’une image QA locale fournissant `/bin/sleep` et `/bin/tar`. Choisir `QA_PODMAN_MODE=rootless` ou `QA_PODMAN_MODE=rootful` selon le moteur. Pour le moteur rootful approuvé, ajouter `AGENTSANDBOX_TEST_ALLOW_ROOTFUL=1`.

```sh
AGENTSANDBOX_TEST_RUNTIME=podman \
AGENTSANDBOX_TEST_PODMAN_MODE="$QA_PODMAN_MODE" \
AGENTSANDBOX_TEST_RUNTIME_PROGRAM="$QA_PODMAN_BIN" \
AGENTSANDBOX_TEST_RUNTIME_ENDPOINT="$QA_PODMAN_ENDPOINT" \
AGENTSANDBOX_TEST_IMAGE="$QA_WORKER_IMAGE_ID" \
  cargo test --locked --offline --test runtime_user_journey \
  real_cli_runs_three_independent_workers_across_two_tenants \
  -- --ignored --exact --nocapture
```

Le prévol vérifie le mode attesté et le digest sur le socket exact. Le parcours CLI vérifie le chevauchement, la libération de quota, les refus inter-tenant, les allocations et les exports liés aux trois workers.

Pour vérifier aussi le dashboard après la fin des workers, ajouter les [quatre variables Chromium](#vérifier-le-dashboard-des-trois-workers-dans-chromium) à la commande Podman. Le navigateur vérifie le runtime `podman` dans le détail de chaque worker et le rapproche des allocations et des inspections enregistrées. Le rapport `browser` conserve le runtime et le verdict. Laisser `AGENTSANDBOX_TEST_BROWSER_LIVE` absente pour cette recette.

Pour la recette en direct, sélectionner `QA_PODMAN_MODE=rootless`, ajouter les mêmes variables Chromium et lancer la commande avec `AGENTSANDBOX_TEST_BROWSER_LIVE=1`. Après le rendu des deux vues tenant, le harnais inspecte ensemble les trois identifiants Podman affichés et rapproche leurs démarrages du registre terminal. Il vérifie les deux déconnexions et la fermeture des flux. Une exécution en mode rootful est refusée avant la création de la fixture live.

## Rejouer un worker Podman avec audit et export

Sur macOS, le conducteur autonome lance une seule commande `/usr/bin/id` via le vrai CLI, avec une échéance de 30 s et le réseau `none`. Il prépare son propre gateway loopback et sa source synthétique. Utiliser Python 3.11 ou ultérieur, un CLI déjà compilé avec son empreinte, un socket Unix explicite et l’identifiant complet d’une image locale en `1000:1000`. Choisir une destination absolue encore inexistante.

```sh
python3 -B tests/e2e/run_one_podman_cli.py \
  --cli "$QA_BIN" --cli-sha256 "$QA_CLI_SHA256" \
  --policy "$PWD/policies/default.json" --podman "$QA_PODMAN_BIN" \
  --endpoint "$QA_PODMAN_ENDPOINT" --image "$QA_WORKER_IMAGE_ID" \
  --output "$QA_OUTPUT"
```

Ajouter `--allow-rootful` pour le seul moteur rootful approuvé. Le conducteur rattache le départ audité, le terminal, l’allocation, l’export et le retrait du conteneur à la même session. Il conserve les codes du CLI et les rapports privés dans `QA_OUTPUT`. Un export qui réussit après reprise conserve le verdict de la soumission initiale.

### Suite Podman avec refus et capture

Utiliser un dossier de contrôle neuf, un gateway Rust sur loopback, un programme Podman approuvé et une image QA déjà présente sur le moteur choisi. L’image doit fournir `/usr/bin/id` et `/bin/tar`, avec `/workspace` inscriptible en `1000:1000`. Préférer un socket Unix explicite. Pour un moteur Podman natif Linux ou FreeBSD, `QA_PODMAN_ENDPOINT=local` sélectionne le mode local.

```sh
qa_config=$(mktemp -d)
cargo build --locked --offline -j1 --bin agentsandbox
target/debug/agentsandbox init --config-dir "$qa_config"
target/debug/agentsandbox gateway --listen 127.0.0.1:0 \
  --data "$qa_config/audit.sqlite" \
  --runner-token-file "$qa_config/secrets/runner" \
  --viewer-token-file "$qa_config/secrets/viewer" \
  --policy policies/default.json
```

Dans un autre terminal, reprendre le chemin du dossier de contrôle et l’origine annoncée par le gateway. Le test lit son propre credential de recette. Ajouter `AGENTSANDBOX_TEST_ALLOW_ROOTFUL=1` uniquement si l’exception rootful a été approuvée. Pendant la comparaison des volumes avant/après le refus, garder le moteur de recette libre d’autres créations de volumes.

```sh
AGENTSANDBOX_TEST_CONFIG="$QA_CONFIG_DIR" \
AGENTSANDBOX_TEST_GATEWAY="$QA_GATEWAY_ORIGIN" \
AGENTSANDBOX_TEST_RUNTIME=podman \
AGENTSANDBOX_TEST_RUNTIME_PROGRAM="$QA_PODMAN_BIN" \
AGENTSANDBOX_TEST_RUNTIME_ENDPOINT="$QA_PODMAN_ENDPOINT" \
AGENTSANDBOX_TEST_IMAGE="$QA_WORKER_IMAGE_ID" \
  cargo test --locked --offline -j1 --test runtime_smoke \
  real_worker_copy_uid_and_export_preserve_the_original_project \
  -- --ignored --exact --nocapture
```

Cette suite vérifie l’identité non root, la fin et la reprise de la session, le relevé non facturable, le refus avant effet et l’export en préservant la source. Elle peut démarrer jusqu’à trois workers sans fournisseur, successivement, dont le cas de capture en réseau `bridge`. La recette distincte `runtime_terminal_persistence` injecte un refus d’audit terminal et demande une image contenant également `/usr/bin/python3`.

Le [document racine](../../LOCAL_CLOUD_TESTS.md) conserve une proposition détaillée, mais cite des commandes/tests et fonctions absents du checkout initial. **Ne pas exécuter ces commandes en supposant qu'elles existent.** Les nouveaux tests `tenant_api`, `tenant_storage` et `gateway_tenancy` couvrent l'identité/base d'audit A/B ; ils ne lancent pas de worker. Le seul smoke moteur livré initialement est `runtime_smoke`, à exécuter explicitement selon ses prérequis.

Le registre, les quotas, le ledger d'allocation non facturable et le dashboard partagé existent désormais ; leurs preuves logicielles et parcours moteur ciblés sont dans [CURRENT_STATE.md](CURRENT_STATE.md). Les contrats HTTP/SQLite d'identité worker et de relais dirigé, grants, ACK et révocation ont aussi leurs preuves logicielles, détaillées dans les guides workloads [EN](../setup/workloads/README.md)/[FR](../setup/workloads/README.fr.md). Ils ne constituent pas la recette complète à trois workers : le handoff, le canal depuis les conteneurs sans réseau, le déploiement de cette recette et ses douze critères réels, dont la visibilité dashboard, restent ouverts. Les observations de workers construites par les fixtures sont déclarées, pas des inspections moteur. Chaque scénario réel conserve son moteur, version, OS/architecture, mode rootless/rootful, image/digest, commandes, observations, erreurs et volumes de preuve. Aucun volume existant n'est supprimé pour faire passer la recette.

Docker, Podman et Kubernetes ont des qualifications séparées. Un daemon absent bloque seulement la preuve correspondante. Une configuration cloud ou une fixture ne prouve pas le lancement d'un pod ni l'application d'une politique réseau.
