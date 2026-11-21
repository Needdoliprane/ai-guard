# Recette trois workers — état et cible

La recette obligatoire sur le poste du mainteneur lance trois vrais workers en parallèle, A1/A2 chez A et B1 chez B, avec contrôle séparé. Trois à cinq workers suffisent à la validation fonctionnelle locale. La mesure de capacité à 100 workers actifs par tenant relève d’un environnement dimensionné pour cette charge. [Critères de la recette](TEST_STRATEGY.md)

Observer les trois workers actifs pendant une même période, avec leur identifiant d’exécution, un volume de travail propre et un résultat distinct pour chacun. Arrêter A1 pendant que A2 et B1 poursuivent leur exécution, puis vérifier leurs résultats et la portée des traces côté CLI et dashboard. Conserver les vérifications de rôles, de refus inter-tenant et du relais A1→A2 soumis à autorisation.

## Rejouer la concurrence sur Docker

Le test opt-in utilise trois projets synthétiques, deux bases tenant et le vrai CLI. Il vérifie le chevauchement des intervalles observés par Docker, les quotas actifs, la fin normale d’A1 pendant qu’A2 et B1 restent actifs, les volumes et exports distincts, les allocations non facturables et les deux chaînes d’audit. Chaque viewer ouvre le détail de ses workers et reçoit HTTP 404 pour les identifiants des workers de l’autre tenant. Les plafonds cumulés des workers sont de 6 vCPU et 3 Gio de RAM, hors contrôle. L’arrêt explicite, le relais et le dashboard font partie de la recette complète et restent à vérifier séparément.

Fournir le chemin absolu du client Docker, son socket Unix local et l’identifiant SHA-256 complet d’une image QA déjà présente, avec `/bin/sleep`, `/bin/tar` et le profil de worker. Le compte courant doit être reconnu par le prévol de contrôle. Sur un moteur rootful approuvé, ajouter `AGENTSANDBOX_TEST_ALLOW_ROOTFUL=1` à l’environnement du test.

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

Le [document racine](../../LOCAL_CLOUD_TESTS.md) conserve une proposition détaillée, mais cite des commandes/tests et fonctions absents du checkout initial. **Ne pas exécuter ces commandes en supposant qu'elles existent.** Les nouveaux tests `tenant_api`, `tenant_storage` et `gateway_tenancy` couvrent l'identité/base d'audit A/B ; ils ne lancent pas de worker. Le seul smoke moteur livré initialement est `runtime_smoke`, à exécuter explicitement selon ses prérequis.

Le registre, les quotas, le ledger d'allocation non facturable et le dashboard partagé existent désormais ; leurs preuves logicielles et parcours moteur ciblés sont dans [CURRENT_STATE.md](CURRENT_STATE.md). Les contrats HTTP/SQLite d'identité worker et de relais dirigé, grants, ACK et révocation ont aussi leurs preuves logicielles, détaillées dans les guides workloads [EN](../setup/workloads/README.md)/[FR](../setup/workloads/README.fr.md). Ils ne constituent pas la recette complète à trois workers : le handoff, le canal depuis les conteneurs sans réseau, le déploiement de cette recette et ses douze critères réels, dont la visibilité dashboard, restent ouverts. Les observations de workers construites par les fixtures sont déclarées, pas des inspections moteur. Chaque scénario réel conserve son moteur, version, OS/architecture, mode rootless/rootful, image/digest, commandes, observations, erreurs et volumes de preuve. Aucun volume existant n'est supprimé pour faire passer la recette.

Docker, Podman et Kubernetes ont des qualifications séparées. Un daemon absent bloque seulement la preuve correspondante. Une configuration cloud ou une fixture ne prouve pas le lancement d'un pod ni l'application d'une politique réseau.
