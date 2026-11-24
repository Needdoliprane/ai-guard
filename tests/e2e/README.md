# Rejouer les parcours de qualification

Le dossier de preuve conserve la révision, les modifications locales, les empreintes du binaire et des images ainsi que le moteur et la plateforme réellement utilisés. Chaque scénario garde son propre statut. Les essais qui démarrent des ressources utilisent une pile dédiée et des identifiants de recette. [Plan de validation](../../docs/engineering/TEST_STRATEGY.md)

## Rejouer le prévol viewer dans Chromium

Préparer le binaire et le bundle compilé de cette candidate, ainsi que Node, Playwright et Chromium. `CODEX_PRIMARY_RUNTIME_NODE_MODULES` indique le chemin absolu du `node_modules` de Playwright s’il est installé à part. `PLAYWRIGHT_CHROMIUM_EXECUTABLE` indique un Chromium existant.

```sh
umask 077
python3 -B tests/e2e/run_browser_viewer_preflight.py \
  --cli "$QA_BIN" --front-dir "$QA_FRONT_DIR" \
  --evidence-dir "$QA_EVIDENCE/viewer-preflight"
```

Le script crée deux services Rust sur des ports loopback éphémères et vérifie le viewer synthétique auprès du gateway, du proxy et de la Vue d’ensemble. Il utilise le moteur de décision Rust et ne lance aucun worker. Le nouveau dossier privé contient `result.json`, `browser.json` et, si disponibles en cas d’échec, le HTML assaini et la capture Chromium signalés par le conducteur.

## Rejouer le retrait de droits dans Chromium

Construire l’exemple Rust `supervision_stack` et le bundle du dashboard. La recette utilise les mêmes prérequis Chromium et Playwright que le prévol viewer. Elle révoque de vrais droits dans la base SQLite de recette, exerce le cycle de lecture du navigateur et vérifie l’effacement des données et la fermeture du flux SSE. Elle révoque ensuite le credential personnel et vérifie le retour à la connexion.

```sh
umask 077
AI_GUARD_SUPERVISION_SCENARIO=revoked \
AI_GUARD_SUPERVISION_STACK="$QA_SUPERVISION_STACK" \
AI_GUARD_SUPERVISION_FRONT_DIR="$QA_FRONT_DIR" \
AI_GUARD_SUPERVISION_ARTIFACTS="$QA_EVIDENCE/revoked" \
  node front/tests/browser_supervision.mjs
```

Le conducteur signale `scenario=revoked PASS` après les assertions et conserve, si la page du dashboard est capturable à l’échec, le HTML et le PNG assainis dans un dossier privé. Les workers et événements sont des entrées synthétiques du vrai service Rust.

### Rejouer l’ouverture du dashboard avec des droits partiels

Reprendre la commande avec `AI_GUARD_SUPERVISION_SCENARIO=partial` et un dossier neuf, par exemple `"$QA_EVIDENCE/partial"`. Le vrai gateway sert trois contextes de recette, avec uniquement Workers, uniquement Brokers ou uniquement Ressources personnelles. Chromium vérifie qu’une seule lecture de supervision est émise dans chaque contexte et que le dashboard explique les sources et associations indisponibles. La garde navigateur vérifie que les requêtes restent sur l’origine en loopback. Les contrôles de mise en page et les captures couvrent 320, 390 et 1 440 px, en clair et en sombre. Si une assertion échoue sur le dashboard authentifié, le conducteur produit le HTML et le PNG assainis dans le dossier privé.

### Rejouer la navigation au clavier

Reprendre la commande avec `AI_GUARD_SUPERVISION_SCENARIO=keyboard` et un dossier privé distinct. Le scénario suit le focus depuis le titre et la recherche jusqu’au panneau de détail, puis vérifie sa restauration avec Échap et l’ordre de Tab et Maj+Tab à 768, 390 et 320 px. À 320 px, il vérifie aussi l’accès à Fédération en mode mouvement réduit. La garde loopback et les captures assainies sont actives.

### Rejouer l’export nominatif

Reprendre la même commande avec `AI_GUARD_SUPERVISION_SCENARIO=human-export` et le binaire `supervision_stack` du même checkout. A1 exporte sa session en deux pages et télécharge le fichier JSON. Le conducteur compare le curseur et le contenu aux réponses HTTP, puis recoupe les deux reçus avec la chaîne SQLite. A2 conserve la lecture des métadonnées autorisées. Le bouton d’export est désactivé et le même appel au serveur reçoit HTTP 403. Le rapport conserve l’empreinte du fichier, les compteurs et le statut du refus.

### Rejouer l’expiration du contexte de supervision

Reprendre la commande Chromium avec `AI_GUARD_SUPERVISION_SCENARIO=ttl` et un dossier d’artefacts distinct, par exemple `"$QA_EVIDENCE/ttl"`. Prévoir environ cinq minutes pour l’expiration réelle. Le conducteur vérifie le retrait des données, l’absence de lecture protégée avant confirmation, puis l’obtention d’un nouvel identifiant de contexte sans perdre le filtre sélectionné. Après la reprise, le dashboard affiche le bail du broker comme périmé. Les données sont synthétiques et le gateway Rust applique la vraie durée de validité.

### Rejouer la séparation des clients après une déconnexion

Reprendre avec `AI_GUARD_SUPERVISION_SCENARIO=isolation` et un dossier d’artefacts propre au rejeu. Le conducteur retient une seule réponse HTTP 200 de la flotte A, après sa production par le vrai gateway. La déconnexion doit annuler cette lecture. La connexion B doit charger uniquement ses propres données, dans un contexte distinct. Les deux clients et leurs workers sont des entrées de la SQLite de recette.

### Rejouer le Journal en direct et à la reprise

Reprendre avec `AI_GUARD_SUPERVISION_SCENARIO=journal` et un dossier d’artefacts neuf. Le gateway Rust écrit de nouveaux événements dans la base SQLite de recette. Chromium vérifie leur ordre en tête, la reprise d’un seul flux à partir du curseur de la nouvelle page et la pagination jusqu’à 300 lignes. Le compte A2 ne reçoit pas les événements privés d’A1. Le conducteur conserve les captures assainies si une assertion échoue sur le dashboard authentifié.

## Broker et résultat durable

Les tests CLI lancent le binaire du checkout et utilisent un socket IPC de recette. Les tests serveur vérifient la projection du résultat, l’authentification et la retraite avant démarrage. Les tests du connecteur Jev utilisent HTTP loopback.

```sh
cargo test --locked --offline --test broker_run_cli
cargo test --locked --offline --lib broker::service::client_selection::tests:: -- --test-threads=1
cargo test --locked --offline --lib broker::service::result::tests:: -- --test-threads=1
cargo test --locked --offline --lib broker::service::server::result_auth_tests:: -- --test-threads=1
cargo test --locked --offline --lib broker::writer::status_projection_tests::result_read_ -- --test-threads=1
cargo test --locked --offline --test jev_contract --test jev_backends --test jev_typesafe_ambiguity
```

Les cas `installed_broker_registry_` du même fichier utilisent `run --broker-config` sur un registre privé V2 de recette. Ils vérifient que le CLI utilise le profil demandé et conserve le même UUID, refuse des sélections incomplètes ou ambiguës avant de se connecter, et arrête la lecture si le registre est remplacé après acceptation.

La recette de perte du client commence par `broker prepare-client` avec un registre de recette contenant deux profils. Elle attend le premier appel `Result` après l’acceptation de `run --broker-selection`, termine le seul processus CLI de la recette et vérifie la fermeture du socket sans nouvelle soumission ni annulation. `broker result --broker-selection` lit le même UUID depuis la même sélection. Le binaire CLI est réel et la réponse IPC du broker provient de la fixture. La recette Docker de perte de la CLI vérifie séparément l’échéance et l’instance du worker.

```sh
cargo test --locked --offline -j 1 --test broker_run_cli \
  lost_run_client_after_acceptance_can_read_the_same_operation -- --exact
```

La régression de reprise après perte d’accusé d’export utilise un vrai dossier et un vrai journal derrière la passerelle de recette sur loopback. Elle vérifie le rejeu du même reçu après réouverture du journal et le maintien de la destination publiée et du fichier original.

```sh
RUST_MIN_STACK=8388608 cargo test --locked --offline --lib \
  runner::broker::executor::journal::tests::published_export_replays_when_the_policy_audit_response_is_lost \
  -- --exact
```

Pour la panne entre la publication et le premier enregistrement local, lancer le même test avec `published_export_at_the_journal_limit_is_preserved_without_a_receipt` à la place du dernier segment du filtre. La limite d’écriture force le refus du journal. La recette vérifie le maintien du dossier et de l’original, avec un refus de republier au même nom et aucun reçu de succès.

Le grant d’export a une recette séparée. Après réouverture du contrôle, un rejeu identique retrouve le même reçu. Une destination divergente et un nouveau grant sont refusés. Une copie SQLite prise avant le grant reste inactive et refuse la demande d’origine.

```sh
RUST_MIN_STACK=8388608 cargo test --locked --offline --test supervision_api \
  supervised_api_arms_import_and_worker_claims_once_then_maintenance_blocks_new_effects -- --exact
```

La reprise du relais exerce deux bases SQLite et le vrai binaire gateway sur loopback. Elle injecte les observations des trois workers A1, A2 et B1, puis vérifie le relais dirigé, les quotas, l’allocation terminale non facturable de B1 et les exports après redémarrage. Les observations des conteneurs proviennent de fixtures.

```sh
RUST_MIN_STACK=8388608 cargo test --locked --offline --test worker_relay_api \
  reopened_gateway_keeps_three_worker_quotas_terminal_usage_and_directed_audit_separate -- --exact
```

La reprise broker ferme le premier service et le gateway de test après un accusé d’annulation. Le lecteur renvoie une indisponibilité pendant la coupure, puis retrouve le même résultat et le même accusé à la réouverture. Cette recette n’arme aucun worker.

```sh
cargo test --locked --offline --lib \
  broker::service::server::result_auth_tests::confirmed_cancel_replays_after_gateway_outage_and_broker_reopen \
  -- --exact
```

La recette suivante démarre puis relance le vrai broker avec son gardien natif. Elle utilise un journal de recette vide et le gateway Rust sur loopback. La relance conserve le verrou et le fichier d’authentification du gardien, obtient une nouvelle époque et refuse un nouvel armement. Le gateway ne reçoit aucun POST de contrôle et aucun worker n’est lancé.

```sh
cargo test --locked --offline -j 1 --test d7_legacy_cli_control \
  actual_service_cli_private_pipe_fresh_then_existing_cold_fence -- --exact
```

La recette du journal non vide enregistre des reçus d’armement, de réservation, d’annulation et d’exécution dans un Store SQLite et dans le journal du broker. Après fermeture et réouverture du journal, le lecteur d’arrêt vérifie la tête, les reçus, les identifiants de conteneur et les échéances consignées. Les échantillons d’horloge et l’autorité moteur sont synthétiques. Prévoir un dossier temporaire privé et un système de fichiers local.

```sh
cargo test --locked --offline -j 1 --test broker_checkpoint \
  sqlite_store_receipts_cover_all_supervision_operation_variants -- --exact --test-threads=1
```

La recette d’échéance lance un sous-processus de test du gardien avec une cible moteur synthétique. Après la réservation, elle rend le dossier privé indisponible en écriture. Le gardien doit réserver l’arrêt à l’échéance d’une seconde, conserver la cause d’échec et garder l’entrée d’admission.

```sh
cargo test --locked --offline -j 1 --lib \
  retained_deadline_proceeds_with_writer_unavailable_and_no_waiter -- --test-threads=1
```

Le plan de reprise pour un gestionnaire de services se vérifie sur les deux formats en Rust. Le conducteur d’intégration lance aussi le vrai CLI avec un registre V2 de recette et un rôle Full3 absent. Il exige un refus avant toute publication dans la racine. Le rejeu nominal du plan a lieu sur l’OS annoncé, avec un compte de contrôle résolu et un rôle privé existant. [Guide opérateur](../../docs/setup/broker/README.fr.md#planifier-la-reprise-du-broker-sous-un-gestionnaire-de-services)

```sh
cargo test --locked --offline -j 1 --lib broker::service::unit_plan::tests:: -- --test-threads=1
cargo test --locked --offline -j 1 --bin agentsandbox broker_service_plan_ -- --test-threads=1
cargo test --locked --offline -j 1 --test broker_run_cli \
  broker_service_plan_refuses_an_absent_full3_role_without_publishing -- --exact
```

La recette de migration 12→13 démarre la CLI de migration et deux vrais gateways sur loopback. Elle compare la tête v13 et celle de la copie neuve du backup v12. Les deux autorités restent inactives et la sauvegarde est conservée. [Procédure opérateur](../../docs/operations.md#passer-du-schéma-12-au-schéma-13)

```sh
cargo test --locked --offline --test schema13_protocol_contract \
  cli_schema13_and_the_served_protocol_keep_the_same_inactive_namespace -- --exact --nocapture
```

### Rejouer une commande Docker hors ligne

Fournir le client Docker approuvé, le socket Unix local et l’ID immuable d’une image QA préparée localement. L’image contient `/usr/bin/id` et `/bin/tar`, avec `/workspace` et `/home/agent` pour l’UID 1000. Activer `AGENTSANDBOX_TEST_ALLOW_ROOTFUL=1` lorsque le moteur le nécessite. Le test commence par `doctor` sous le compte qui exécutera le parcours. Après un prévol réussi, il lance son gateway loopback, exécute un seul worker en réseau `none`, vérifie le terminal et le quota, puis compare l’export neuf au projet de recette.

```sh
AGENTSANDBOX_TEST_RUNTIME=docker \
AGENTSANDBOX_TEST_RUNTIME_PROGRAM="$QA_DOCKER_BIN" \
AGENTSANDBOX_TEST_RUNTIME_ENDPOINT="$QA_DOCKER_UNIX_ENDPOINT" \
AGENTSANDBOX_TEST_IMAGE="$QA_WORKER_IMAGE_ID" \
  cargo test --locked --offline --test runtime_user_journey \
  real_cli_copies_a_project_and_exports_after_one_audited_worker -- --ignored --exact --nocapture
```

Si le budget du harnais expire, l’arrêt de son enfant CLI laisse l’état du worker à vérifier. Le dossier privé de recette est conservé. Utiliser le seul checkpoint dans `control/sessions` et l’origine de son gateway pour reprendre ou arrêter la session concernée.

### Vérifier la cible du diagnostic Podman

Le test de sélection partagé couvre le mode natif sur Linux et FreeBSD, le refus de ce mode sur macOS et les chemins Unix acceptés. Sur un compte de contrôle approuvable, le test du vrai CLI emploie un exécutable Rust de fixture qui journalise exactement ses appels. La recette exige la même cible pour `info` et `version` et le refus rootful avant `version`.

```sh
cargo test --locked --offline --lib \
  runtime::managed::podman_diagnostics_and_managed_commands_share_local_targets -- --exact
cargo test --locked --offline --test runtime_program \
  podman_doctor_targets_one_endpoint_and_keeps_rootful_refusal_read_only \
  -- --ignored --exact --nocapture
```

Sur macOS, la recette suivante vérifie le refus du vrai CLI avant l’ouverture de la configuration lorsque le socket n’a pas été fourni.

```sh
cargo test --locked --offline --test runtime_program \
  new_podman_run_requires_explicit_socket_on_client_only_platforms -- --exact
```

La recette suivante vérifie le bootstrap d’une autorité neuve avec une fixture Rust. Une réponse `info` rootful sans exception se termine par le refus attendu. Le journal de la fixture contient un seul `info` et aucun appel de volume. Le scénario rootless vérifie aussi la réouverture de l’autorité et la perte du marqueur. L’exécutable de fixture passe la même validation de compte que les programmes runtime approuvés.

```sh
cargo test --locked --offline --test runtime_managed \
  podman_authority_bootstrap_is_explicit_and_marker_loss_remains_unknown -- --exact
```

### Rejouer la perte de la CLI sur Docker

Préparer le programme Docker approuvé, une adresse de socket `unix:///…` et l’ID immuable d’une image locale Linux contenant `/bin/sleep`. Le moteur doit pouvoir créer un volume neuf. Le harnais crée ses propres données et credentials de recette, conserve les preuves et garde son gardien joignable après l’essai. Activer l’exception rootful uniquement pour un moteur qui la requiert.

Exécuter d’abord `agentsandbox doctor --runtime docker --runtime-program "$AGENTSANDBOX_TEST_RUNTIME_PROGRAM" --runtime-endpoint "$AGENTSANDBOX_TEST_RUNTIME_ENDPOINT"` avec le même compte OS. Le compte doit avoir un répertoire personnel absolu résolu par le système. La présence du socket moteur ne vérifie pas ce prérequis du broker.

```sh
AGENTSANDBOX_TEST_RUNTIME=docker \
AGENTSANDBOX_TEST_RUNTIME_PROGRAM="$QA_DOCKER_BIN" \
AGENTSANDBOX_TEST_RUNTIME_ENDPOINT="$QA_DOCKER_UNIX_ENDPOINT" \
AGENTSANDBOX_TEST_IMAGE="$QA_WORKER_IMAGE_ID" \
AGENTSANDBOX_TEST_ALLOW_ROOTFUL=1 AGENTSANDBOX_TEST_RETAIN_ENFORCER=1 \
AGENTSANDBOX_TEST_BROKER_NATIVE_START=1 AGENTSANDBOX_TEST_BROKER_TIMEOUT_SECONDS=30 \
cargo test --release --locked --test broker_runtime \
  broker_run_cli_sigkill_preserves_original_deadline_without_second_start -- --ignored --exact --nocapture
```

Le test attend le worker actif, interrompt uniquement la CLI qu’il a lancée, puis contrôle l’arrêt à l’échéance du même worker, l’unicité du démarrage, le résultat durable, le quota et la chaîne d’audit. Utiliser `broker_sigkill_preserves_original_deadline_without_second_start` avec une nouvelle recette séparée pour interrompre le broker lui-même. La commande `cargo test --locked --offline --test broker_runtime` compile les deux scénarios moteur et exécute les fixtures de diagnostic.

Dans la recette qui interrompt le broker, le harnais ouvre le flux d’événements du daemon avant la soumission et compare les événements `create` et `start` au relevé historique de la même session après reprise. Il exige une seule création et un seul démarrage observés du CID initial. Le daemon doit servir le flux HTTP/1.1 sur le socket Unix sélectionné et le clore normalement à la borne demandée. La collecte peut durer près de dix minutes. Le reçu `docker_event_continuity=unknown` conserve l’incertitude sur d’éventuelles omissions communes aux deux relevés. Les fixtures hors moteur se sélectionnent avec `cargo test --locked --offline --test broker_runtime docker_worker_`.

Dans la recette de perte du CLI, `broker prepare-client` publie la sélection du profil de recette. Le lancement et les deux lectures du résultat reçoivent `AGENTSANDBOX_BROKER_SELECTION` dans l’environnement de leurs seuls processus enfants. Le reçu `broker-client-selection.json` garde le mode, le profil et l’empreinte de la sélection. Les sondes indépendantes contrôlent le worker et son échéance.

Les reçus distinguent l’échéance de l’Arm durable avant lancement et la durée du processus entre les deux timestamps observés par Docker. Après perte du broker, ils exigent aussi la réception du terminal du gardien dans la tolérance d’observation de trois secondes. Le rapprochement des horloges utilise une mesure Docker après la panne et suppose la continuité de son horloge.

### Rejouer une annulation après perte de réponse

Une recette Rust indépendante vérifie la reprise d’une annulation de réservation après perte de la réponse HTTP avant ou après le commit SQLite. Elle impose le même identifiant d’opération et le même reçu lors du rejeu après réouverture du gateway. Le quota est libéré une seule fois. Aucune exécution moteur n’est nécessaire pour ce scénario de réservation.

```sh
RUST_MIN_STACK=8388608 cargo test --locked --offline --lib \
  runner::managed::stop_replay_tests::reserved_stop_replays_the_same_intent_across_gateway_restart \
  -- --exact --nocapture
```

### Reprendre les décisions après saturation du journal

Le test lance le vrai gateway avec deux bases SQLite neuves et une limite de deux événements par tenant. Une nouvelle décision reçoit un refus HTTP 503 pour A pendant que B obtient encore sa décision. Le test réouvre les mêmes bases avec une limite supérieure, réessaie la demande originale et vérifie les deux chaînes et leurs reçus. Le test utilise uniquement des commandes de décision synthétiques sur HTTP loopback.

```sh
cargo test --locked --offline --test gateway_tenancy \
  new_decisions_resume_after_audit_capacity_is_raised_and_the_gateway_reopens -- --exact
```

### Vérifier 100 réservations après redémarrage du contrôle

La passerelle Rust accepte 100 réservations de recette pour A via HTTP, puis répond `429` avec le motif métier `Quota`. Après arrêt forcé et redémarrage sur les mêmes fichiers SQLite, le test vérifie les mêmes totaux, les mêmes identifiants sur deux pages et le même reçu pour le refus rejoué. B peut encore réserver et le ledger d’usage reste vide. Une campagne distincte doit mesurer 100 workers actifs sur un moteur réel.

```sh
cargo test --locked --offline --test gateway_tenancy \
  configured_worker_quota_at_hundred_survives_gateway_restart -- --exact --test-threads=1 --nocapture
```

### Pilote G10 de capacité Docker sur un seul tenant

Le sélecteur ignoré par défaut lance trois `/bin/sleep 35` sur le même tenant, via les parcours CLI et gateway habituels. Il exige une image QA déjà présente avec `/bin/sleep` et `/bin/tar`, un endpoint Docker Unix local explicite et l’opt-in ci-dessous. L’opt-in rootful existant s’applique si le moteur sélectionné l’exige.

```sh
AGENTSANDBOX_TEST_CAPACITY_PILOT=1 \
AGENTSANDBOX_TEST_RUNTIME=docker \
AGENTSANDBOX_TEST_RUNTIME_PROGRAM="$QA_DOCKER_PROGRAM" \
AGENTSANDBOX_TEST_RUNTIME_ENDPOINT="$QA_DOCKER_UNIX_ENDPOINT" \
AGENTSANDBOX_TEST_IMAGE="$QA_WORKER_IMAGE_ID" \
cargo test --locked --offline --test runtime_user_journey \
  same_tenant_capacity::real_docker_same_tenant_capacity_pilot -- \
  --ignored --exact --test-threads=1 --nocapture
```

Le test rapproche les lectures du quota qui encadrent le lot d’inspections Docker par CID complet. Il vérifie le mode réseau `none` rapporté par Docker et l’intersection stricte des intervalles natifs du même CID sans redémarrage. Les commandes `sleep` n’exercent aucun échange réseau.

Le harnais attend les trois CLI même si l’observation du pic échoue. Avant de fermer le gateway, il vérifie le retour du quota à zéro et l’absence des CID relevés pendant le suivi ou dans l’inventaire final. Il conserve l’erreur initiale et signale les échecs de ces vérifications. Un CID absent des deux sources reste hors de ce contrôle. Les vérifications Docker sont en lecture seule. Le dossier privé affiché est conservé pour le diagnostic et la reprise. La liaison aux noms de volumes est comparée entre le CLI et le registre, et la présence des volumes après exécution reste à vérifier.

Un verdict réussi exige aussi trois états terminaux cohérents et une chaîne d’audit valide. Le pilote ne vérifie pas la présence de chaque événement attendu pour chaque session.

Le profil géré impose 2 vCPU, 1 Gio et 256 PID par worker, avec un profil fixe pour les auxiliaires d’import. Le pilote borne donc la concurrence à trois avec 6 vCPU et 3 Gio de plafonds cumulés pour les workers. Une campagne légère à 100 exige de rendre les plafonds inférieurs configurables et liés au lancement autorisé, de borner la concurrence des auxiliaires et de valider la pagination ou une lecture complète du registre à l’échelle du tenant. À 100, les seuls plafonds actuels des workers totaliseraient 200 vCPU et 100 Gio. Le test hors moteur de l’intersection utilise 100 intervalles synthétiques. La mesure de 100 workers physiques reste `NOT_RUN`.

## Dialogue TTY hors ligne

```sh
python3 -I -B tests/e2e/interactive_driver.py --self-test
```

Le contrôle utilise un vrai pseudo-terminal et un programme synthétique. Il exige deux reçus visibles, une sortie native et des modes termios restaurés. Les résultats Codex et Claude à deux tours sont établis sur leurs sessions propres.

## Vérifier une installation native transférée

La recette produit un manifeste local, transfère les fichiers attendus dans un dossier privé neuf et vérifie le manifeste avec son empreinte côté préparation. Elle lance le CLI et le diagnostic installés depuis un dossier de travail vide, puis rejette un remplacement qui garde le même nom et la même taille. L’installation d’origine reste vérifiable. [Guide de livraison](../../docs/setup/release/README.fr.md)

```sh
cargo test --locked --offline --test release_metadata_verify \
  verified_native_files_run_from_a_fresh_private_installation -- --exact --nocapture
```

## Relier un cas fournisseur au dashboard

Utiliser une [pile de recette séparée](../../docs/setup/local/README.fr.md) avec ses deux ports loopback, une configuration de contrôle initialisée et un profil fournisseur dédié. Construire et charger l’image du worker sur le moteur ciblé, puis fournir son ID local complet. Le dossier de preuves doit être neuf. Le harnais utilise le parcours historique de la CLI pour cette recette.

```sh
python3 -B tests/e2e/run_provider_case.py \
  --bin "$QA_BIN" --config-dir "$QA_CONFIG" \
  --gateway "$QA_GATEWAY" --dashboard "$QA_DASHBOARD" \
  --runtime docker --runtime-endpoint "$QA_RUNTIME_ENDPOINT" --image "$QA_WORKER_IMAGE_ID" \
  --client claude-code --auth-mode subscription --client-profile "$QA_CLAUDE_PROFILE" \
  --providers-file "$QA_PROVIDERS_FILE" --transport batch \
  --evidence-dir "$QA_EVIDENCE" --worker-provenance
```

Ajouter `--allow-rootful-runtime` si le moteur le nécessite. Le même fichier privé de profils est utilisé pour le diagnostic et le lancement. Le harnais vérifie le profil, le mode et le modèle demandés avec le compagnon `ai-guard-diagnose` placé à côté de `--bin` ou désigné par `--diagnose-bin`. Ce prévol vérifie uniquement les métadonnées locales de la référence de credential. Une référence manquante bloque le cas avant Chromium et `run`, même lorsque l’autre mode est configuré. Adapter le client et le mode à chaque case. En TTY, réserver la configuration de recette au cas. Le harnais vérifie la session et le worker, l’image du lancement et de l’inspection, les fichiers exportés et le journal JSONL affiché dans Chrome. Les échecs navigateur peuvent conserver des captures HTML et PNG privées et masquées.

Pour rejouer seulement le prévol sur les huit combinaisons client, mode et transport, utiliser les deux binaires de la candidate côte à côte. Le test vérifie une référence sélectionnée présente puis absente, avec l’autre mode toujours configuré. Il crée ses propres fichiers vides et s’arrête avant Chromium, le moteur et l’appel fournisseur.

```sh
AI_GUARD_QA_PROVIDER_CLI="$QA_BIN" \
  python3 -IB tests/e2e/test_run_provider_preflight.py InstalledDiagnosticTests -v
```

La relecture d’un TTY expiré vérifie le libellé **Délai dépassé** et, si l’audit contient un motif, le **Motif de l’arrêt**. Le statut du parcours reste `FAIL` lorsque le dialogue a dépassé son délai.

Pour un batch supervisé, ajouter `--broker-root "$QA_BROKER_ROOT" --broker-profile "$QA_BROKER_PROFILE" --broker-token-file "$QA_RUNNER_FILE" --broker-source-root "$QA_SOURCE_ROOT" --broker-export-root "$QA_EXPORT_ROOT"`. Le profil actif autorise les clients gérés et les deux racines disjointes. Omettre `--runtime-endpoint` et `--runtime-program`. La recette conserve les UUID avant le lancement unique, relit `broker result` et vérifie les octets et le manifeste dans un nouvel export. Le navigateur recoupe la décision, le résultat, le scan et l’export avec les reçus et le worker exact. Il lit le nettoyage par séquence dans l’API et vérifie le retrait dans **Workers**. La variante conserve `BLOCKED` si cette preuve manque. [Guide broker](../../docs/setup/broker/README.fr.md)

Sur la même pile et le même profil, le fichier privé `broker-browser-proof.json` permet de rejouer Chrome sans relancer le fournisseur. Les chemins de configuration et de preuve sont absolus.

```sh
AI_GUARD_TEST_GATEWAY="$QA_GATEWAY" AI_GUARD_TEST_DASHBOARD="$QA_DASHBOARD" \
AI_GUARD_TEST_CONFIG="$QA_CONFIG" AI_GUARD_TEST_BROKER_PROOF="$QA_EVIDENCE/broker-browser-proof.json" \
AI_GUARD_TEST_ARTIFACT_DIR="$QA_EVIDENCE" node front/tests/browser_provider_live.mjs
```

L’agrégateur Rust lit les rapports CLI et navigateur déjà produits. Il vérifie les empreintes et les correspondances entre cases. Sa propre suite tourne hors ligne.

```sh
cargo test --locked --offline --example provider-matrix
cargo run --locked --offline --example provider-matrix -- \
  --host Darwin/arm64 --runtime docker --cli-sha256 "$QA_CLI_SHA256" \
  --worker-image-sha256 "$QA_WORKER_IMAGE_ID" --rootful-exception true \
  --cli-report "$QA_EVIDENCE/result.json" --require-pass
```

Répéter `--cli-report` pour chaque case et `--browser-report` pour les relectures autonomes. Adapter les axes aux artefacts et à la plateforme mesurés. `--execution-path legacy` sélectionne huit cases par défaut. `--execution-path broker-batch` sélectionne quatre cases batch sur Docker ou Podman. Une session présente dans plusieurs rapports CLI bloque la case concernée, y compris quand une de ces entrées est écartée par les axes de la candidate. Avec `--require-pass`, l’outil écrit le rapport JSON puis échoue si une case sélectionnée est `FAIL`, `BLOCKED` ou `NOT_RUN`. Le verdict couvre ces parcours fournisseur. La [qualification V1](../../docs/engineering/RELEASE_CHECKLIST.md) examine aussi les autres gates.

Une relecture autonome liée à la même session peut compléter un navigateur initialement `BLOCKED`. Un navigateur en échec conserve son statut `FAIL`, avec le résultat CLI et la preuve de relecture consultables séparément.

Pour le scénario déterministe de provenance Worker sur fixtures loopback, construire le front puis utiliser Playwright installé dans l’environnement de test.

```sh
cd front
npm run build
AI_GUARD_UNIFIED_SCENARIO=worker-provenance node tests/browser_unified.mjs
node tests/browser_unified.mjs
node tests/browser_command_timeout.mjs
node tests/browser_broker_provider_simulated.mjs
```

Le dernier script utilise les assets compilés et le même oracle Chromium que la recette réelle. Ses événements, reçus et registre sont synthétiques. Son rapport porte le mode `synthetic_broker_dashboard` et reste une preuve du harnais et de l’interface.

## Rejouer le diagnostic et le prévol Kubernetes sur Docker Desktop

Le test Rust suivant lance cinq fois le vrai CLI contre un gateway de recette sur loopback et un Store SQLite migré. Il vérifie `prepare`, `status`, `abandon` et le rejeu. La clôture porte le même identifiant d’opération, libère le quota et laisse les rapports à `execution_available=false` et `runtime_state=not_observed`. Le canari réseau confirme l’absence de connexion à l’autorité Kubernetes configurée.

```sh
cargo test --locked --offline --test kubernetes_cli \
  operator_session_cli_reports_and_replays_the_before_effect_reservation -- --exact --test-threads=1
```

Utiliser le contexte local `docker-desktop` avec Kubernetes `Ready` et une API HTTPS loopback. Préparer une recette dédiée comme dans le [guide Kubernetes](../../docs/setup/kubernetes/README.fr.md) à partir de `deploy/kubernetes/local-inspection.json` et des `Role`/`RoleBinding` de `deploy/kubernetes/local-preflight.json`. Réécrire ensemble le nom du namespace neuf sous la forme `aig-v1-…`, les noms des ressources cluster de recette, la cible de la règle `namespaces` et les sujets de liaison. Conserver le label `restricted`. Utiliser le compte de service de cette recette avec un jeton de courte durée, l’AC publique, l’UID réel du namespace `kube-system` et celui de la recette. Le binaire ne reçoit que les chemins privés explicitement choisis.

```sh
"$QA_BIN" kubernetes doctor --authority-file "$QA_KUBE_AUTHORITY" \
  --ca-file "$QA_KUBE_CA" --token-file "$QA_KUBE_TOKEN" --check-permissions
AI_GUARD_QA_KUBE_AUTHORITY="$QA_KUBE_AUTHORITY" AI_GUARD_QA_KUBE_CA="$QA_KUBE_CA" \
AI_GUARD_QA_KUBE_TOKEN="$QA_KUBE_TOKEN" AI_GUARD_QA_KUBE_IMAGE="$QA_KUBE_IMAGE" \
  cargo test --locked --offline --test kubernetes_preflight_cli real_loopback_cluster_preflight \
  -- --ignored --exact --test-threads=1 --nocapture
kubectl --context docker-desktop -n "$QA_KUBE_NAMESPACE" get pods
```

Choisir une référence d’image avec digest pour le Pod de prévol. Le test démarre son gateway SQLite privé et exige les reçus audit de la demande et du résultat Kubernetes, ainsi qu’un registre de workers vide. Contrôler aussi que le namespace neuf contient zéro Pod avant et après. Retirer uniquement les ressources possédées par cette recette. Ce contrôle couvre l’admission à blanc par le serveur et ne télécharge ni n’exécute l’image.

Pour un workspace de recette déjà provisionné, la commande d’inspection lit le claim par son nom, sous une autorité qui permet le GET du PVC. Elle renvoie la projection validée avec son UID et `admission_available: false`. Le test Rust exerce cette commande sur une fixture TLS locale. [Prérequis de stockage](../../docs/setup/kubernetes-runtime/README.fr.md#82-prérequis-de-stockage)

```sh
"$QA_BIN" kubernetes inspect-workspace --claim-name "$QA_KUBE_CLAIM" \
  --authority-file "$QA_KUBE_AUTHORITY" --ca-file "$QA_KUBE_CA" --token-file "$QA_KUBE_TOKEN"
cargo test --locked --offline --test kubernetes_session_runtime
```

La fixture TLS locale vérifie aussi les diagnostics quand le claim est Pending, ne demande pas uniquement `ReadWriteOncePod`, ou ne nomme pas de classe de stockage. Elle vérifie le diagnostic générique pour une observation malformée et le refus prioritaire si un ancrage change avant l’affichage. Pour cibler ces messages et le parcours nominal du vrai CLI, utiliser `cargo test --locked --offline --test kubernetes_session_runtime inspect_workspace_cli_ -- --test-threads=1`.

### Observer un PVC Pending sur Docker Desktop

Utiliser un fichier `KUBECONFIG` privé et stable pendant toute la recette. Le contexte `docker-desktop` doit pointer vers une adresse HTTPS loopback littérale et fournir l’AC publique embarquée. Le certificat doit couvrir cette adresse pour le CLI Rust. Le compte opérateur peut créer et retirer le namespace et les liaisons RBAC de recette et demander le jeton temporaire du compte de service.

```sh
KUBECONFIG="$QA_KUBE_CONFIG" AI_GUARD_QA_KUBECTL="$QA_KUBECTL" \
AI_GUARD_QA_KUBE_WORKSPACE_PENDING=1 \
  cargo test --locked --offline --test kubernetes_cni_live \
  docker_desktop_workspace_pending_diagnostic -- --ignored --exact --test-threads=1 --nocapture
```

Le test crée un namespace neuf et un PVC de 1 Mio en mode `ReadWriteOncePod` avec un sélecteur unique et une classe volontairement absente. Le compte utilisé par le vrai CLI reçoit des droits GET sur le PVC exact et ses deux namespaces d’ancrage. Le reçu confirme le diagnostic, le même UID du PVC, zéro Pod et l’absence de liaison à un volume avant et après la lecture. Le nettoyage contrôle les UID des ressources temporaires. Ce scénario vérifie le chemin de diagnostic du PVC en attente.

### Vérifier l’application des NetworkPolicy locales

Sur Docker Desktop uniquement, cette recette crée deux namespaces privés `restricted`, deux pods CoreDNS sur le même nœud et des policies limitées à ces pods. Elle utilise l’image publique CoreDNS v1.14.2 déjà présente et vérifie son identité à partir du CoreDNS système. L’image requiert `NET_BIND_SERVICE` pour son exécutable. Cette capacité est ajoutée aux deux pods de recette après suppression des autres capacités. Le test cible l’API HTTPS loopback du contexte `docker-desktop` et vérifie TCP ingress et egress entre les pods, avec des témoins positifs et un témoin de reprise après chaque restauration. Le reçu contient le verdict et le résultat du nettoyage. [Image CoreDNS v1.14.2](https://github.com/coredns/coredns/blob/v1.14.2/Dockerfile) · [Profil Restricted](https://kubernetes.io/docs/concepts/security/pod-security-standards/#restricted)

```sh
AI_GUARD_QA_KUBE_CNI=1 AI_GUARD_QA_KUBECTL="$QA_KUBECTL" \
  cargo test --locked --offline --test kubernetes_cni_live docker_desktop_tcp_network_policy \
  -- --ignored --exact --test-threads=1 --nocapture
```

## Relier une décision du moteur Rust à Chromium

Préparer le binaire de la candidate, les assets issus de `npm run build` dans `front/` et Playwright avec Chromium. La recette démarre la passerelle et le dashboard Rust sur deux ports loopback éphémères et crée des bases SQLite privées séparées pour A et B. Elle soumet un accord et un refus bénins à la politique réelle, exécute `verify-audit`, ouvre les deux détails dans **Sécurité** et vérifie le JSONL exporté avec le jeton viewer de A. Le parcours vérifie l’absence du reçu de B sous l’identité A. Aucun worker ni modèle ne démarre.

```sh
python3 -B tests/e2e/run_browser_decision.py --cli "$QA_BIN" \
  --front-dir front/dist --evidence-dir "$QA_EVIDENCE"
```

Le dossier de preuve doit être neuf. Il contient le rapport JSON, les coordonnées des décisions et, en cas d’échec navigateur, le HTML masqué et le PNG privés si la page est disponible.

Rejouer la même recette avec `--judge-mode local-fixture` et un autre dossier neuf pour simuler le raccord Qwen. Le gateway Rust réel reçoit un avis modèle « Autoriser » et conserve un refus effectif. La recette vérifie aussi le veto Rust sans POST et le refus durable après HTTP 503 de la fixture. Chromium rapproche l’évaluateur, l’avis et les états de chaque décision, puis compare le JSONL complet au journal authentifié. L’endpoint modèle synthétique est limité à loopback.

L’option `--judge-mode typesafe-fixture` utilise un endpoint System One local et un credential synthétique neuf. Elle vérifie le POST `choice` avec le modèle demandé `jev-latest`, puis un accord, un veto Jev et un refus Rust sans POST. L’identifiant modèle retourné par la fixture est distinct de l’alias. Chromium vérifie les deux identifiants affichés, l’avis, la provenance « Ligne de commande » et le JSONL complet. Chaque mode utilise ses propres bases neuves.

## Qualifier une décision TypeSafe et son rejeu

La recette CLI et Chromium partage la même décision SQLite. Elle demande un avis TypeSafe pour `/usr/bin/id`, arrête le gateway qui possède le credential, puis rouvre la même base avec le moteur Rust. Elle vérifie le reçu original avec `verify-audit`, le détail Sécurité et l’export JSONL. Le test utilise deux tenants neufs, et les refus locaux et le rejeu de A conservent le compteur de requêtes.

Pour valider le harnais sur loopback, utiliser un dossier de preuve neuf par variante. Le mode de fixture accepte `allow`, `deny`, `review` ou `unavailable`. La dernière variante vérifie un refus durable après HTTP 503.

```sh
python3 -B tests/e2e/run_browser_decision.py --cli "$QA_BIN" \
  --front-dir "$QA_FRONT_DIR" --evidence-dir "$QA_EVIDENCE" \
  --judge-mode typesafe-replay-fixture --fixture-typesafe-outcome unavailable
```

Pour l’essai réel, fournir un fichier de clé TypeSafe privé de mode `0600` et un nouveau dossier de preuve. L’endpoint HTTPS officiel est fixé par le harnais. L’essai prévoit un seul appel TypeSafe, puis recoupe le résultat réel dans le CLI et le navigateur.

```sh
python3 -B tests/e2e/run_browser_decision.py --cli "$QA_BIN" \
  --front-dir "$QA_FRONT_DIR" --evidence-dir "$QA_EVIDENCE" \
  --judge-mode typesafe-real --typesafe-key-file "$QA_TYPESAFE_KEY_FILE"
```

Préparer le binaire du checkout et un fichier privé en mode `0600` contenant uniquement la clé Jev de recette. La sonde envoie une décision synthétique à TypeSafe en demandant `jev-latest`. Elle publie le modèle retourné et vérifie le même reçu après redémarrage du gateway sur la même base, avec TypeSafe désactivé. La preuve JSON peut être enregistrée dans le dossier privé de recette.

```sh
python3 -B tests/e2e/run_jev_probe.py --bin "$QA_BIN" --key-file "$QA_JEV_KEY_FILE"
```

## Observer le registre des politiques egress

La première recette vérifie le refus par défaut et l’intersection tenant/session dans le moteur de politique. La deuxième utilise le vrai gateway loopback et une base de recette au format 15 pour exercer le remplacement, le reçu idempotent et le passage à `revoke_requested`. La troisième commande lance le vrai CLI de la même compilation pour inspecter, préparer et appliquer une politique. Indiquer un chemin absolu canonique et l’empreinte du binaire. Ces recettes observent le registre sans contacter les destinations des politiques.

```sh
cargo test --locked --offline --lib \
  egress::policy_tests::empty_policies_deny_and_both_policies_must_allow_the_exact_destination -- --exact
cargo test --locked --offline --lib \
  runner::egress_policy::tests::real_reserved_sdk_preserves_e1_receipts_retries_and_revoke -- --exact
AI_GUARD_A1_CLI_TEST_BIN="$QA_BIN" AI_GUARD_A1_CLI_TEST_SHA256="$QA_CLI_SHA256" \
  cargo test --locked --offline --lib \
  api::human_sources::owned_policy_inspection_cli_tests::real_cli_inspect_prepare_apply_keeps_observed_scope_cas_and_operation \
  -- --ignored --exact --nocapture
```

La recette suivante perd le premier accusé HTTP après l’écriture de la politique. Le vrai CLI reprend la même opération et le test compare le reçu aux octets conservés par le gateway, puis vérifie la demande de révocation et son événement SQLite.

```sh
AI_GUARD_A1_CLI_TEST_BIN="$QA_BIN" AI_GUARD_A1_CLI_TEST_SHA256="$QA_CLI_SHA256" \
  cargo test --locked --offline --lib \
  api::human_sources::owned_policy_cli_tests::actual_cli_preserves_operation_after_lost_receipt_then_revokes_without_runtime_claim \
  -- --ignored --exact --nocapture
```

### Rejouer le Journal des politiques egress dans Chromium

Compiler le bundle du dashboard et le libtest Rust du même checkout avec `cargo test --locked --offline -j 1 --lib --no-run`. Indiquer le chemin absolu du libtest indiqué par Cargo dans `QA_OWNED_LIBTEST` et le chemin absolu du bundle dans `QA_FRONT_DIR`. Préparer Playwright et Chromium comme pour les autres recettes navigateur. La fixture autorise Linux ARM64 sous UID/GID 1000 et macOS ARM64 sous compte effectif non-root.

```sh
umask 077
QA_TMP=$(mktemp -d "${TMPDIR:-/tmp}/aig-e1-browser.XXXXXXXX")
QA_TMP=$(cd "$QA_TMP" && pwd -P)
mkdir -m 700 "$QA_TMP/control" "$QA_TMP/artifacts"
TMPDIR="$QA_TMP" AI_GUARD_OWNED_BROWSER_FIXTURE=run-egress-v1 \
AI_GUARD_OWNED_BROWSER_LIBTEST="$QA_OWNED_LIBTEST" \
AI_GUARD_OWNED_BROWSER_DIST="$QA_FRONT_DIR" \
AI_GUARD_OWNED_BROWSER_CONTROL="$QA_TMP/control" \
AI_GUARD_OWNED_BROWSER_ARTIFACTS="$QA_TMP/artifacts" \
  node front/tests/browser_owned_snapshot.mjs
```

La fixture utilise une vraie migration Store15 et le gateway Rust sur loopback. Le propriétaire de la session voit le remplacement de la politique puis la demande de révocation après un rafraîchissement. Le second compte n’a aucun partage de cette session. Les reçus des mutations et des lectures sont vérifiés dans SQLite. Les données sont synthétiques et la politique ne lance aucune connexion. Le dossier privé contient le verdict et les captures assainies si une assertion échoue sur le dashboard authentifié.

## Vérifier les profils locaux de Qwen et Keycloak

```sh
docker compose --env-file .env.exemple.local-qwen -f deploy/local-ai/compose.yaml config --quiet
docker compose --env-file /dev/null -f deploy/keycloak-demo/compose.yaml config --services
docker compose --env-file /dev/null -f deploy/keycloak-demo/compose.yaml \
  --profile keycloak-demo-http config --services
```

La configuration Qwen sépare le modèle et son relais loopback. La configuration Keycloak active deux services de découverte sous le profil explicite. Les mesures mémoire/OOM après construction et la connexion nominative restent des essais de déploiement. [Inférence locale](../../docs/setup/local-ai/README.fr.md) · [Découverte SSO](../../docs/setup/sso/README.fr.md)

### Rejouer Qwen avec le modèle réel

Depuis la racine de la candidate V1, utiliser son binaire `QA_BIN`, son bundle compilé `QA_FRONT_DIR` et une image de relais Rust construite sur la même source (`QA_CONTROL_IMAGE`). Cette recette utilise Docker avec un socket Unix explicite, cgroup v2 et un espace de noms cgroup privé pour le modèle.

Préparer Python 3.11 ou ultérieur, Node.js, Playwright et Chromium. Installer Playwright dans `front/node_modules` ou indiquer le chemin absolu d’un autre `node_modules` dans `CODEX_PRIMARY_RUNTIME_NODE_MODULES`. Le conducteur exécute le navigateur avec un `HOME` isolé. Indiquer le chemin absolu du binaire dans `PLAYWRIGHT_CHROMIUM_EXECUTABLE` ou du cache des navigateurs dans `PLAYWRIGHT_BROWSERS_PATH`.

Choisir un nom de projet neuf `QA_QWEN_PROJECT`, avec le préfixe `aig-v1-qwen-` suivi de 32 chiffres hexadécimaux minuscules, un port loopback libre `QA_QWEN_PORT` et un tag local dédié `QA_QWEN_IMAGE_REF`. `QA_DOCKER_BIN` et `QA_DOCKER_SOCKET` sont des chemins absolus. Créer un dossier privé neuf `QA_EVIDENCE` en mode `0700`.

```sh
export AI_GUARD_CONTROL_IMAGE="$QA_CONTROL_IMAGE"
export AI_GUARD_LOCAL_AI_IMAGE="$QA_QWEN_IMAGE_REF"
export AI_GUARD_LOCAL_AI_PORT="$QA_QWEN_PORT"
export AI_GUARD_LOCAL_AI_MODEL=qwen2.5-0.5b-q4km
export AI_GUARD_LOCAL_AI_CONTEXT=512 AI_GUARD_LOCAL_AI_CPUS=2.0
export AI_GUARD_LOCAL_AI_MEMORY_BYTES=1000000000 AI_GUARD_LOCAL_AI_DEFAULT_TOKENS=64
qwen_docker() { "$QA_DOCKER_BIN" --host "unix://$QA_DOCKER_SOCKET" "$@"; }
qwen_compose() {
  qwen_docker compose --env-file .env.exemple.local-qwen -p "$QA_QWEN_PROJECT" \
    -f deploy/local-ai/compose.yaml "$@"
}
```

Le [Dockerfile](../../deploy/local-ai/Dockerfile) épingle llama.cpp par digest et les poids Qwen2.5-0.5B-Instruct Q4_K_M par révision et checksum. Si l’image exacte est déjà disponible localement sous `QA_QWEN_IMAGE_REF`, passer directement au bloc de démarrage. Sinon, préparer l’image avec la commande suivante selon le [guide local](../../docs/setup/local-ai/README.fr.md#construire-et-démarrer).

```sh
qwen_compose build model
```

```sh
QA_QWEN_IMAGE_ID=$(qwen_docker image inspect --format '{{.Id}}' "$QA_QWEN_IMAGE_REF") &&
  qwen_compose up --no-build --pull never -d --wait
```

Prendre les deux mesures sur le modèle du même projet. Chaque relevé Rust vérifie l’ID image, l’identité du conteneur pendant la mesure, le confinement, la limite, le pic du cgroup, le swap et les OOM. `route-*.txt` relève le modèle puis le relais et leurs liaisons réseau sur le même socket Docker. La publication du relais doit être exactement `127.0.0.1:$QA_QWEN_PORT`.

```sh
qwen_capture() (
  phase=$1
  model_id=$(qwen_compose ps -q model) || exit
  relay_id=$(qwen_compose ps -q local-ai-ingress) || exit
  test -n "$model_id" && test -n "$relay_id" || exit 1
  published=$(qwen_compose port local-ai-ingress 8080) || exit
  test "$published" = "127.0.0.1:$QA_QWEN_PORT" || exit 1
  {
    printf '%s\n' "$published"
    qwen_docker inspect --type container --format \
      '{{.Id}} {{.Image}} {{.State.StartedAt}} {{.RestartCount}} {{range $name, $net := .NetworkSettings.Networks}}{{$name}}={{$net.NetworkID}} {{end}}{{json .NetworkSettings.Ports}}' \
      "$model_id" "$relay_id"
  } > "$QA_EVIDENCE/route-$phase.txt" || exit
  "${QA_QWEN_EVIDENCE_BIN:?binaire du conducteur Rust local requis}" \
    --docker "$QA_DOCKER_BIN" --socket "$QA_DOCKER_SOCKET" --project "$QA_QWEN_PROJECT" \
    --image-id "$QA_QWEN_IMAGE_ID" --confirm-qa > "$QA_EVIDENCE/memory-$phase.json"
)

(
  umask 077
  qwen_capture before || exit
  browser_rc=0
  python3 -B tests/e2e/run_browser_decision.py --cli "$QA_BIN" --front-dir "$QA_FRONT_DIR" \
    --evidence-dir "$QA_EVIDENCE/web" --judge-mode local-real --model-port "$QA_QWEN_PORT" \
    --model-timeout-seconds 12 || browser_rc=$?
  qwen_capture after || exit
  cmp "$QA_EVIDENCE/route-before.txt" "$QA_EVIDENCE/route-after.txt" || exit
  exit "$browser_rc"
)
```

Rapprocher le `model.container_id` des deux JSON avec l’ID de la ligne modèle dans les relevés de route. Le modèle et le relais doivent partager le réseau d’inférence dédié, et seul le relais publie le port relevé. Comparer `image_id`, `started_at` et `restart_count`. Le pic est celui de la durée de vie de ce cgroup. Les deux mesures et le résultat navigateur doivent porter `PASS` pour conclure à cette preuve. Conserver les résultats en cas d’échec sans relancer le modèle pour compléter une mesure manquante.

Le conducteur démarre le vrai gateway et le dashboard de la candidate. Il sélectionne `--jev-backend local-openai` et transmet un seul POST Qwen depuis un relais de recette loopback. Il vérifie le refus effectif, le reçu exact au rejeu et la tête d’audit via `verify-audit`. Dans Chromium, il vérifie le détail de la même décision, l’export JSONL et l’isolation du tenant. Les rapports sont `web/result.json`, `web/decisions.json` et `web/browser.json`. Cette recette couvre le contrôle et la lecture de l’audit sans lancement de worker. Le CLI, le bundle et le harnais doivent venir de la même source. Une autre base utilisant `--decision-engine` demande son propre parcours.

Le rejeu se fait sur le même gateway pendant que le projet modèle tourne. Le compteur confirme l’absence d’appel Qwen pour le rejeu et les refus impératifs. Le conducteur utilise des bases temporaires. Le scénario après arrêt physique requiert une recette distincte qui conserve la même base QA et la demande exacte, arrête le modèle, rejoue le même reçu, puis vérifie qu’une demande fraîche échoue sans créer de worker. Le mode `local-fixture` couvre déjà le refus durable après HTTP 503 sur loopback.

Le rapport `result.json` conserve `model_relay_timeline` pour `local-real`. Les jalons donnent la durée monotone depuis le POST et distinguent l’envoi amont, la réception des en-têtes et la lecture bornée du corps. Un dépassement est associé à l’étape en attente dans le relais de recette. Le contenu de la requête et de la réponse reste dans le transport. Le délai de décision du gateway et le verdict E2E restent les mêmes.

Si le gateway atteste un refus après réponse indisponible ou invalide, le conducteur continue avec le rejeu du même reçu, `verify-audit`, la lecture Chromium et l’export JSONL. La vérification comprend l’isolement des deux tenants et un seul POST au relais. Le rapport peut alors porter `fail_closed_proof_verified: true`. La recette nominale rend toujours `FAIL` et le code 1 tant que Qwen n’a pas rendu un avis exploitable.

Après les deux captures, arrêter le projet de recette avec `qwen_compose stop`. La preuve couvre Docker/cgroup v2 et HTTP loopback. Podman, le profil TLS et la qualité du jugement restent à qualifier séparément.

### Rejouer la connexion Web avec Keycloak

Préparer la [pile DEMO locale neuve](../../docs/setup/sso/README.fr.md#préparer-une-demo-locale-neuve) avec le dashboard compilé et le gateway de la même candidate. Installer Playwright et Chromium dans l’environnement de test. Le script vérifie les trois comptes applicatifs publics et le refus du compte `admin` du realm maître dans le formulaire du realm de découverte. Il exerce le profil OIDC Web legacy. Les profils de sources humaines possédées et l’identité CLI ont des recettes séparées.

```sh
AI_GUARD_TEST_DASHBOARD=http://127.0.0.1:4000 \
AI_GUARD_TEST_OIDC_ISSUER=http://keycloak.ai-guard.test:18080/realms/ai-guard-demo \
node front/tests/browser_oidc_live.mjs
```

Ajouter `--check-config` pour contrôler les adresses et les comptes sans réseau. `AI_GUARD_TEST_OIDC_ACCOUNTS` limite le rejeu à une liste de comptes publics séparée par des virgules. Un dossier de captures `AI_GUARD_TEST_ARTIFACT_DIR` privé permet de conserver le HTML assaini et le PNG en cas d’échec sur le dashboard. Les pages du fournisseur et du callback ne sont pas capturées.

`node front/tests/browser_oidc_live.mjs --check-transport` démarre des serveurs HTTP jetables. Le prévol vérifie les trois adresses loopback du navigateur et le refus d’une route hors recette avant toute connexion Keycloak.

### Rejouer la connexion CLI avec Keycloak

Utiliser le même realm DEMO sur un projet dédié et son gateway loopback prêt. Le conducteur lance une connexion du vrai CLI pour `user-1`, pilote le navigateur dans le realm DEMO et vérifie le callback PKCE, la lecture du contexte personnel, le renouvellement et la déconnexion. Son JSON fournit les références d’audit sans conserver l’URL transitoire, les codes ou les jetons. Installer Playwright et Chromium comme pour la recette Web.

```sh
AI_GUARD_TEST_CLI="$PWD/target/debug/agentsandbox" \
AI_GUARD_TEST_GATEWAY="$QA_GATEWAY" \
AI_GUARD_TEST_OIDC_ISSUER=http://keycloak.ai-guard.test:18080/realms/ai-guard-demo \
  node front/tests/browser_oidc_native_live.mjs
```

Depuis le conteneur Keycloak de recette, `kcadm.sh get realms/ai-guard-demo --fields realm,enabled --no-config --server http://keycloak.ai-guard.test:18080 --realm master --user admin` vérifie séparément la lecture par le compte maître. Saisir son mot de passe public au prompt. Le test Web ai-guard vérifie le refus de ce compte dans le formulaire du realm DEMO.

### Tester la pile SSO Docker sur des ports de recette

Partir d’un projet Keycloak et d’un projet de contrôle neufs. Si les ports hôte 8787 et 4000 sont occupés, conserver le port Keycloak canonique 18080 et utiliser [l’override Docker de recette](compose.sso.override.yaml) en second fichier Compose du projet de contrôle. Celui-ci conserve les destinations internes et publie les deux ports choisis uniquement sur IPv4 loopback. Choisir des ports libres et renseigner les trois valeurs dans le fichier d’environnement neuf avant l’import du realm.

```dotenv
AI_GUARD_TEST_SSO_GATEWAY_PORT=18780
AI_GUARD_TEST_SSO_DASHBOARD_PORT=14080
AI_GUARD_DEMO_WEB_ORIGIN=http://127.0.0.1:14080
```

Dans une copie relue du [manifeste public DEMO](../../docs/setup/sso/examples/local-demo.json), ajuster `gateway.web_origin` à la nouvelle origine et `gateway.provider.web_redirect_uri` à cette origine suivie de `/auth/oidc/callback`. Le fournisseur importe la même origine au premier démarrage. Relever le réseau interne d’identité comme dans le guide SSO et vérifier le rendu Compose. Fournir le manifeste adapté sur stdin au service `setup` de l’override, puis démarrer `gateway dashboard ingress`. Les clients de recette utilisent les nouvelles origines loopback.

Pour la reprise, exécuter la commande du [CLI Keycloak](#rejouer-la-connexion-cli-avec-keycloak) avec l’option `--check-gateway-restart` et une entrée standard pilotée par le conducteur. Après le JSON `WAITING`, arrêter et redémarrer le seul `gateway` du projet de recette. Dès que son `/healthz` répond de nouveau, transmettre la ligne `gateway_restarted` à l’entrée du conducteur avant `wait_ms`. Le CLI déjà ouvert doit refuser son ancienne admission avant l’expiration. Rejouer ensuite la recette nominale pour établir une admission neuve et comparer l’audit avant et après redémarrage, sur le même volume.

### Vérifier la reprise de la préparation SSO sur un volume Docker neuf

Sélectionner le programme Docker par son chemin absolu, son socket Unix et l’ID SHA-256 local d’une image construite avec `images/control.Dockerfile`. Le harnais crée un volume de recette neuf, prépare le manifeste public V2 et met le seul reçu de complétion de côté dans ce volume. Deux appels à `setup resume` doivent reconstituer le même reçu avec les mêmes liaisons et credentials. Un nouveau `setup prepare` doit refuser la destination existante. Les objets privés restent dans le volume, que le harnais conserve pour inspection.

```sh
rustc --edition=2024 tests/e2e/setup_recovery_docker.rs -o "$QA_EVIDENCE/setup-recovery-docker"
"$QA_EVIDENCE/setup-recovery-docker" --docker "$QA_DOCKER_PROGRAM" \
  --endpoint "$QA_DOCKER_ENDPOINT" --image "$QA_CONTROL_IMAGE_ID" \
  --manifest "$PWD/docs/setup/sso/examples/local-demo.json"
```

### Vérifier le retrait du rôle dans Chromium

La recette Rust de renouvellement utilise un émetteur OIDC sur loopback. Elle vérifie que le retrait du rôle d’administration conserve la lecture du contexte personnel et invalide le contexte antérieur.

```sh
cargo test --locked --offline --test oidc_gateway_flow \
  successful_web_refresh_withdraws_administration_and_preserves_personal_scope -- --exact
```

Utiliser le front compilé dans `front/dist` ou fournir un chemin absolu vers ses assets. Le test démarre le dashboard, le gateway Rust et un émetteur OIDC signé de recette sur trois ports loopback. Chromium suit le vrai parcours PKCE S256 de client public et laisse le timer renouveler la session. Il vérifie la disparition du contexte et des données d’administration, la fermeture de l’ancien flux et la conservation de la seule ressource personnelle autorisée. Le test vérifie aussi les cinq enregistrements du journal d’authentification. Un dossier privé optionnel `AI_GUARD_TEST_ARTIFACT_DIR` conserve des captures assainies pour les échecs sur le dashboard.

```sh
AI_GUARD_TEST_FRONT_DIR="$QA_FRONT_DIR" RUST_MIN_STACK=8388608 \
  cargo test --locked --offline --test oidc_gateway_flow \
  chromium_oidc_refresh_withdraws_administration_and_keeps_personal_resources \
  -- --ignored --exact --nocapture
```

### Vérifier la révocation OIDC dans la CLI

Cette recette utilise une vraie commande `human sso` en mode navigateur manuel, un gateway Rust, un émetteur signé et deux bases SQLite de recette. L’émetteur vérifie l’échange PKCE sur loopback. Le test obtient un reçu de lecture, retire l’appartenance puis vérifie le refus. L’entrée standard reste ouverte jusqu’à la fermeture auditée après expiration du jeton. Le callback manuel est piloté par le test.

```sh
cargo test --locked --offline --test oidc_gateway_flow \
  native_cli_reads_then_observes_durable_membership_revocation_and_expiry \
  -- --ignored --exact --nocapture
```

### Rejouer une panne de lecture et le retrait des droits dans le front

Le gateway Rust continue à servir Workers et Brokers pendant que Playwright interrompt une seule lecture Ressources. La recette vérifie l’heure de dernière lecture, le statut périmé et le retour à HTTP 200 via « Réessayer ». Lors d’une seconde panne, la réduction des droits purge le détail et les données précédentes. La sélection du contexte réduit continue à charger le registre. Préparer le même front compilé et un répertoire de captures privé de mode `0700`. Playwright et Chromium doivent être disponibles dans l’environnement de test.

```sh
cargo build --locked --offline --example supervision_stack
AI_GUARD_SUPERVISION_SCENARIO=resource-recovery \
AI_GUARD_SUPERVISION_STACK="$PWD/target/debug/examples/supervision_stack" \
AI_GUARD_SUPERVISION_FRONT_DIR="$QA_FRONT_DIR" \
AI_GUARD_SUPERVISION_ARTIFACTS="$QA_EVIDENCE" \
  node front/tests/browser_supervision.mjs
```

Rejouer la même commande avec `AI_GUARD_SUPERVISION_SCENARIO=correlation-recovery` pour interrompre la première lecture Workers et vérifier l’état « correspondance inconnue » dans la ligne et le détail d’une session. Après « Réessayer », le détail doit présenter le worker déclaré dans la fenêtre chargée.

Utiliser `AI_GUARD_SUPERVISION_SCENARIO=resource-stream-recovery` pour vérifier la reprise du flux. Chromium masque l’onglet, puis simule une panne au retour. Les dernières lignes restent visibles et portent l’état périmé. Une fois la connexion rétablie, le bouton « Actualiser » relit les droits, rouvre un seul flux et conserve le focus. Une erreur du navigateur conserve le HTML et la capture dans le dossier de recette.

Les variantes `resource-stream-late-denial` et `resource-stream-visibility-race` retardent une réponse de lecture ou d’identité. Le refus tardif d’Événements doit revenir aux Ressources en conservant la suspension. Après deux retours de visibilité pendant une lecture d’identité, seule la réponse du dernier retour peut réautoriser le flux, les lectures de liste, les détails et l’export.

Utiliser `AI_GUARD_SUPERVISION_SCENARIO=journal-catch-up` pour le Journal. Le test coupe le SSE dans Chromium, vérifie que le HTTP 200 de reconnexion laisse les données périmées et attend une nouvelle tranche d’événements issue du gateway Rust. Il répète ensuite la panne sans nouvel événement et vérifie que « Relire depuis les plus récents » rétablit les données avec une lecture explicite.

Le mode `journal-older-recovery` fait échouer une seule lecture d’une page plus ancienne. Au clavier, « Réessayer » doit redemander le même curseur, conserver les lignes déjà lues et ajouter la page sans rouvrir le flux. Les tests Node de `front/tests/journal-store.test.mjs` couvrent aussi la reprise proposée lorsque le contrôle refuse ce curseur.

### Vérifier la pagination de la supervision par tenant

La recette démarre le vrai gateway et le dashboard avec deux registres SQLite de test. Chromium traverse les deux pages de A, puis filtre les lignes déjà chargées. Après déconnexion et purge, le second compte affiche seulement B. Les reçus auditent chaque lecture composée. La mesure porte sur la consultation de 120 lignes de registre.

```sh
AI_GUARD_TEST_FRONT_DIR="$QA_FRONT_DIR" RUST_MIN_STACK=8388608 \
  cargo test --locked --offline --test supervision_stack_api \
  chromium_reads_bounded_supervision_window_without_cross_tenant_data \
  -- --ignored --exact --nocapture
```
