# Gates de qualification production

Les exigences détaillées G01–G12 sont conservées dans le [document racine](../../PRODUCTION_READINESS.md). Ses anciennes affirmations d'implémentation multi-client ont été contredites par l'inventaire du checkout ; elles ne constituent pas des preuves. L'[état courant](CURRENT_STATE.md) et les [résultats exécutés](VALIDATION.md) font autorité sur l'avancement.

La [reprise de l’audit fonctionnel du 18 novembre](VALIDATION.md#18-novembre-2026--reprise-de-laudit-fonctionnel) consigne les résultats du checkout principal et les écarts encore ouverts. Les états de qualification ci-dessous restent ouverts.

| Gate | Exigence | État de qualification courant |
|---|---|---|
| G01 | Contrats et matrice des plateformes/artefacts | BLOCKED. Sur `99cb885e`, la construction locale sans tag et l’installation temporaire de l’archive macOS ARM64 passent avec six binaires, notices, BUILD-INFO, empreinte vérifiée et identité du CLI installé. Le CLI installé depuis cette archive passe aussi le parcours nominal Docker Desktop 29.9.3 ARM64 avec un worker strict, réseau coupé, export neuf et audit valide. L’inventaire moteur revient à sa valeur initiale. Le même CLI passe le parcours nominal en Podman 6.1.4 rootless avec les mêmes assertions. Les ressources créées sont retirées et les ressources préexistantes préservées. L’installation temporaire de l’archive Linux x86-64 passe sur `1c9e8e01`. La recette d’export Docker Linux x86-64 passe sur le gel distinct `ca3c161b`. La qualification des parcours Docker et Podman à partir d’un même CLI installé sur Linux x86-64 reste ouverte. [Installation Linux](https://github.com/Needdoliprane/ai-guard/actions/runs/45348160190) · [Export Docker Linux](https://github.com/Needdoliprane/ai-guard/actions/runs/45347625066) |
| G02 | Confinement et isolation tenant | BLOCKED. Les recettes physiques à trois workers sur Docker Desktop ARM64 et Podman rootful/rootless vérifient deux tenants, trois volumes distincts et les refus HTTP 404 des détails hors tenant. Le dashboard vérifie aussi ces refus dans la recette terminale Podman rootless. Avec le CLI macOS archivé `99cb885e`, Podman 6.1.4 rootless et Docker Desktop 29.9.3 en mode rootful explicitement autorisé rendent chacun `HostConfig.ReadonlyRootfs=true` pour le conteneur que le registre durable rattache au worker nominal actif. Les reçus d’inspection couvrent la configuration rapportée par les moteurs. La lecture du montage racine dans les workers Podman rootless et Docker Desktop est attestée ci-dessous. La qualification complète du confinement et les autres plateformes restent ouvertes. [Recette Docker](evidence/2026-11-19/functional/native17-and-local-workers.md#trois-workers-a1-a2-et-b1-après-redémarrage) · [Podman rootful](evidence/2026-11-19/functional/native17-and-local-workers.md#podman-rootful-à-trois-workers-sur-deux-tenants) · [Podman rootless](evidence/2026-11-19/functional/native17-and-local-workers.md#parcours-cli-et-dashboard-terminal-sur-podman-rootless) |
| G03 | Copie/export sûrs et séparés par tenant | BLOCKED. Sur `ca3c161b`, le parcours Docker Linux x86-64 rootful passe la copie filtrée, le worker audité et l’export vers une destination neuve. L’original reste intact et le retrait des ressources est attesté. [Export Docker](https://github.com/Needdoliprane/ai-guard/actions/runs/45347625066) Sur `f0f76763`, la recette Podman rootless Linux ARM64 sous libkrun passe les trois exports A1/A2 chez A et B1 chez B. Les exports concordent avec les sources, les trois lectures hors tenant renvoient 404 et les trois conteneurs workers identifiés ont été retirés. Les volumes de travail sont encore présents au relevé après essai. Le prévol Ubuntu x86-64 reste ouvert. Les trois workers Docker Desktop ARM64 et Podman rootful/rootless ont des volumes et exports distincts. La soumission, l’audit et l’export par CLI passent aussi pour un worker Podman rootful, avec source intacte et exclusion du `.env` de recette. La matrice des autres plateformes reste à qualifier. [Recette Docker](evidence/2026-11-19/functional/native17-and-local-workers.md#trois-workers-a1-a2-et-b1-après-redémarrage) · [CLI Podman](evidence/2026-11-19/functional/native17-and-local-workers.md#podman--soumission-et-export-par-le-vrai-cli) · [Podman rootless](evidence/2026-11-19/functional/native17-and-local-workers.md#parcours-cli-et-dashboard-terminal-sur-podman-rootless) Historique candidat : publication avec barrières fichiers/répertoires/parent et ascendance UID/modes/ACL vérifiée au gel du 31 octobre. |
| G04 | Autorisation, identité, quotas | BLOCKED. Sur `ca3c161b`, Keycloak DEMO Web et CLI passent. Le CLI de `user-1` renouvelle sa session de 1 à 2, retrouve le reçu de lecture dans l’audit durable et enregistre la déconnexion. [Recette Keycloak](https://github.com/Needdoliprane/ai-guard/actions/runs/45346854856) Les quotas actifs, leur libération et la portée des viewers passent avec trois workers réels sur Docker Desktop ARM64 et sur Podman rootful dans le parcours CLI. Le registre, les CAS et les claims ont leurs contrôles ciblés. Keycloak DEMO vérifie les rôles Web et la lecture CLI personnelle avec audit. Le lancement attribué à une identité humaine, la reprise sur moteur et les autres constats restent ouverts. [Recette Docker](evidence/2026-11-19/functional/native17-and-local-workers.md#trois-workers-a1-a2-et-b1-après-redémarrage) · [Découverte Keycloak](evidence/2026-11-18/functional/v1-keycloak-discovery-control-and-journal.json) · [Recette Podman à trois workers](evidence/2026-11-19/functional/native17-and-local-workers.md#podman-rootful-à-trois-workers-sur-deux-tenants) |
| G05 | Réseau, secrets et relais contrôlé | BLOCKED. L’entrée Rust et une partie des restrictions runtime sont vérifiées sur Docker Desktop. Les fixtures SQLite vérifient la corrélation durable des observations et le Journal rend les décisions, les observations et la fermeture du canal selon les droits de métadonnées. Le rendu de fermeture passe sur `233a91cb` dans le test ciblé et la compilation de production. Le scénario Chromium `a4745250` montre la fermeture synthétique en clair sur ordinateur et en sombre sur mobile, avec la provenance et la couverture attendues, et confirme son absence chez le lecteur sans partage. Le raccordement de l’autorité de connexion au superviseur et l’observation de bout en bout sur un runtime restent ouverts. [Proposition E3](G05_E3_BOOTSTRAP_PROPOSAL.md) Le contrôle du réseau et du relais sur des workers réels et la qualification cloud restent ouverts. [Résultats et limites](VALIDATION.md#19-novembre-2026--native17-et-recette-locale-à-trois-workers) Historique candidat : identités/grants/relais HTTP et SQLite vérifiés au 4 novembre ; transport réel worker et cloud non qualifiés. |
| G06 | Audit/usage durables, restauration, migrations | BLOCKED. Les six oracles SQLite owned 14/15 et le témoin R2 passent hors ligne sur le gel `bc59cf6b`, soit sept tests. En fixture, la restauration préserve les faits durables. Après réadoption, une nouvelle incarnation rend le même reçu de claim consommé et refuse un démarrage pré-claim, avec quota et audit préservés. Sur le même gel, le témoin R2 restitue sur une copie SQLite le reçu d’autorisation d’export, conserve le quota et vérifie l’archive d’audit. Sur `4c141f25`, le CLI Store15 refuse une copie d’identité physique différente au chargement de la clôture précédente. Le test vérifie que les deux fichiers SQLite, l’activation et la destination restent intacts. Le parcours opérateur complet de restauration et de remise en service reste à qualifier. [Proposition de reprise](G06_OWNED_SQLITE_RESTORE_PROPOSAL.md) La restauration MySQL Legacy a passé les deux profils TLS/mTLS sur `decb2d34`. [MySQL Legacy](https://github.com/Needdoliprane/ai-guard/actions/runs/45344891924) Les deux chaînes d’audit et les allocations non facturables des trois workers passent sur Docker Desktop ARM64 et sur Podman rootful dans le parcours CLI. Les migrations et sauvegardes internes ont des preuves ciblées. La reprise après saturation, le refus après erreur d’E/S injectée après commit et la validation du préfixe durable passent sur des bases SQLite de recette. La réconciliation d’un claim Kubernetes après perte d’ACK passe sur libtest archivé et SQLite, dont deux parcours HTTP loopback. La restauration depuis sauvegarde et les pannes physiques restent à qualifier. Les deux prévols MySQL 8.4.13 passent sur Docker Desktop ARM64. Le diagnostic Rust passe 12 cas réels dans le worktree SQL. L’audit ordinaire passe la recette Rust sur les deux profils MySQL, avec droits d’écriture limités, concurrence, rejeu et refus d’un préfixe raccourci ou divergent sur la même instance. La décision et son reçu indexé passent aussi sur les deux profils, avec rejeu et rollback après refus d’insertion du reçu. Le contrôle de la réserve des décisions par nombre d’événements et par octets passe sur les deux profils. Le Store Rust prolonge l’historique déjà présent sur les deux profils MySQL. La réservation concurrente et le cycle de vie V2 passent sur les deux profils, avec observations synthétiques, rollback d’allocation, maintien puis libération du quota et rejeu. Le vrai CLI refuse une sélection ambiguë et neuf régressions HTTP SQLite passent sur la composition antérieure au lot de réservation. La lecture paginée de l’audit passe sur les deux profils, avec refus d’une première page si le journal diverge plus loin. Le bootstrap atomique du marqueur de supervision passe sur les deux profils et la réouverture du Store reste sans témoin d’origine. La lecture d’admissions supervisées préparées, la vérification des quotas et l’ajout d’un événement sans réécriture de l’historique passent sur les deux profils MySQL. Sur les deux profils, le Store réserve sous le témoin d’origine, restitue le reçu après réouverture et reprend le même appel après un rollback confirmé. La présence d’une liaison humaine sur deux bases MySQL et les six régressions Store15 passent dans leurs gels respectifs. Le DDL workload s’installe sur les deux profils physiques. Cinq scénarios du Store passent sur les deux profils MySQL. La synthèse du dashboard rejoint la collision des projections, la réserve de contrôle, l’intégrité et les entités. [Synthèse MySQL](evidence/2026-11-19/functional/native17-and-local-workers.md#synthèse-overview-sur-mysql-tls-et-mtls) Le détail et la pagination des workers passent sur les deux profils du Store MySQL. Les pages d’usage non facturable et les reçus d’opération par UUID passent également en TLS et mTLS. [Usage et reçus MySQL](evidence/2026-11-19/functional/native17-and-local-workers.md#usage-des-workers-et-reçus-dopération-sur-mysql) La sonde de registre vérifie les deux projections d’identité sur TLS et mTLS et constate leur absence avec un refus. [Sonde stricte MySQL](evidence/2026-11-19/functional/native17-and-local-workers.md#sonde-stricte-des-projections-didentité-mysql) [Lectures MySQL des workers](evidence/2026-11-19/functional/native17-and-local-workers.md#détail-et-pagination-des-workers-sur-mysql) La reprise de l’autorité de réservation, le quota Kubernetes et l’intégration au gateway restent ouverts. [Rejeu physique du Store MySQL](evidence/2026-11-19/functional/native17-and-local-workers.md#quatre-scénarios-du-store-mysql-sous-tls-et-mtls) [Recette Docker](evidence/2026-11-19/functional/native17-and-local-workers.md#trois-workers-a1-a2-et-b1-après-redémarrage) · [Saturation](evidence/2026-11-18/functional/v1-release-archive-and-control-recovery.json) · [Recette Podman à trois workers](evidence/2026-11-19/functional/native17-and-local-workers.md#podman-rootful-à-trois-workers-sur-deux-tenants) · [Injections de panne SQLite](evidence/2026-11-19/functional/native17-and-local-workers.md#rejeu-ciblé-des-pannes-du-store-daudit) · [Réconciliation des claims](evidence/2026-11-19/functional/native17-and-local-workers.md#réconciliation-des-claims-kubernetes-sous-sqlite) · [Écriture sur la cible SQLite](evidence/2026-11-19/functional/native17-and-local-workers.md#écriture-dans-la-base-sqlite-configurée) · [Prévol MySQL](evidence/2026-11-19/functional/native17-and-local-workers.md#prévol-des-deux-profils-mysql-réels) · [Diagnostic Rust](evidence/2026-11-19/functional/native17-and-local-workers.md#transport-mysql-du-diagnostic-rust) · [Audit ordinaire MySQL](evidence/2026-11-19/functional/native17-and-local-workers.md#droits-décriture-et-préfixe-confirmé-sur-les-deux-profils-mysql) · [Transaction de décision MySQL](evidence/2026-11-19/functional/native17-and-local-workers.md#transaction-de-décision-et-reçu-sur-mysql) · [Réserve des décisions](evidence/2026-11-19/functional/native17-and-local-workers.md#réserve-des-décisions-sur-mysql) · [Écriture et réservation MySQL](evidence/2026-11-19/functional/native17-and-local-workers.md#écriture-sur-cible-mysql-préparée-et-réservation-de-worker) · [Cycle V2 MySQL](evidence/2026-11-19/functional/native17-and-local-workers.md#cycle-de-vie-v2-sur-mysql) · [Lecture paginée MySQL](evidence/2026-11-19/functional/native17-and-local-workers.md#lecture-paginée-de-laudit-mysql) · [Origine de supervision MySQL](evidence/2026-11-19/functional/native17-and-local-workers.md#bootstrap-de-lorigine-de-supervision-mysql) · [Admission supervisée préparée](evidence/2026-11-19/functional/native17-and-local-workers.md#admission-supervisée-et-format-partagé) · [Réservation supervisée](evidence/2026-11-19/functional/native17-and-local-workers.md#réservation-supervisée-mysql-et-reprise-après-rollback) · [Projection humaine sur MySQL](evidence/2026-11-19/functional/native17-and-local-workers.md#présence-dune-liaison-humaine-sur-deux-bases-mysql) · [Six régressions Store15](evidence/2026-11-19/functional/native17-and-local-workers.md#six-régressions-store15-sur-le-gel-post-intégration) · [Matrice SQL](TEST_STRATEGY.md#stockage-sql-configurable--à-qualifier) Historique candidat : schéma v2/ledger non facturable, migration/backup privés et gel 327 PASS. |
| G07 | Fournisseurs et limites d'observation | BLOCKED. Sur `70668b05`, `providers check` passe hors ligne pour Codex abonnement et Claude API avec référence de credential absente ou présente mais non vérifiée. Ces essais couvrent le résultat du prévol CLI sans authentification ni appel fournisseur. [CI du prévol](https://github.com/Needdoliprane/ai-guard/actions/runs/45353536913) Codex 0.165.0 et Claude 2.1.140 passent en batch API manuel. Claude/API batch géré dispose d’une preuve ciblée sur une image ARM64 avec demande Haiku 5.5. Le cas Codex/API supervisé avec CLI 0.170.0 rend FAIL après la décision `Allow` durable et avant toute admission enregistrée. L’appel au fournisseur reste indéterminé. Un canari hors ligne passe sur un gel archivé pour distinguer le retour Writer et le contrôle de génération. Le premier rejeu synthétique du vrai `broker submit` inscrit une admission supervisée et un worker réservé. Le second isole `broker status` en `NeedsReconciliation` avant le refus de recette. Les deux nettoyages sont confirmés. [Défaut de statut V3](evidence/2026-11-19/functional/native17-and-local-workers.md#défaut-de-statut-du-broker-sur-un-checkpoint-v3) Le rejeu synthétique après correction passe avec le vrai CLI et la réponse de statut vérifiée. [Rejeu corrigé](evidence/2026-11-19/functional/native17-and-local-workers.md#rejeu-synthétique-après-correction-du-statut-v3) [Rejeu broker submit](evidence/2026-11-19/functional/native17-and-local-workers.md#premier-rejeu-du-broker-submit-codex-api-synthétique) [Canari du batch Codex](evidence/2026-11-19/functional/native17-and-local-workers.md#retour-writer-et-contrôle-de-génération-du-batch-codex) Les scénarios navigateur TypeSafe et modèle local simulés passent sous Linux ARM64. Une réponse synthétique de six secondes réussit en direct et reçoit HTTP 504 via le relais Qwen. Avec le nouveau réglage de huit secondes, une autre recette TLS accepte la réponse de six secondes et refuse celle de neuf secondes. Une décision Qwen réelle passe sur Docker Desktop ARM64 via le relais TLS, avec reçu et rejeu CLI, export Chromium et pic cgroup de 323 Mo. Le déploiement Compose complet du juge, Podman, Kubernetes et la reprise à froid restent à qualifier. Les autres combinaisons gérées et les TTY deux tours restent à qualifier, ainsi que les paramètres adverses, le refresh, la collecte native et la médiation. [Cas Codex/API](evidence/2026-11-19/functional/native17-and-local-workers.md#prévol-codex-avec-le-vrai-compagnon-de-diagnostic) · [Décisions simulées et relais Qwen](evidence/2026-11-19/functional/native17-and-local-workers.md#décisions-simulées-dans-chromium-et-délai-du-relais-qwen) · [Décision Qwen réelle](evidence/2026-11-19/functional/native17-and-local-workers.md#décision-qwen-réelle-sur-docker-du-cli-à-chromium) Le TTY API Codex Luna avec 03a3 passe sur le CLI et le bundle P6b archivés. Claude Code API TTY échoue avant le premier reçu. Les abonnements restent manuels. [TTY API réels](evidence/2026-11-19/functional/native17-and-local-workers.md#tty-api-réels-avec-le-conducteur-intégré) Le scénario TTY partagé passe sur Docker et Podman rootless. [Rejeu Docker partagé](evidence/2026-11-19/functional/native17-and-local-workers.md#rejeu-docker-du-scénario-tty-partagé) Le rejeu Podman est également vérifié dans Chromium. [Échéance TTY Podman et Chromium](evidence/2026-11-19/functional/native17-and-local-workers.md#échéance-tty-sur-podman-rootless-et-chromium) |
| G08 | API, dashboard et sécurité HTTP/SSE | BLOCKED. La recette Docker en activité passe avec les deux vues tenant, l’inspection native des trois workers après le rendu, les deux déconnexions et la fermeture du navigateur. Le dashboard terminal passe sur des données réelles Docker et Podman rootful et dans la recette intégrée Podman rootless. Le rejeu Chromium mono-processus passe le retrait au changement de contexte, l’annulation du GET de A à la déconnexion et la lecture exclusive de B, puis le navigateur sort sur signal. Le rejeu multiprocessus du navigateur en cache crée son contexte puis échoue à créer la première page. Le canari distinct avec Chrome installé échoue au lancement, avant toute page. Les assertions du parcours restent à rejouer. Le prévol viewer et le parcours décisions Rust, audit, export et séparation tenant passent sur un gel privé Linux ARM64 avec Chromium multiprocessus. Le même bundle vérifie aussi le reçu Qwen réel, la tête d’audit du CLI et l’export sur une recette Docker distincte. Le témoin loopback `1678b2f0` vérifie pour un lecteur humain authentifié la lecture HTTP ordinaire pendant une pause applicative SSE, la libération des permis après fermeture et la reprise sans trou ni doublon au dernier curseur livré. Le parcours Journal Chromium `a4745250` vérifie le rendu de la fermeture du canal et le périmètre du lecteur sur fixture Rust Store15. Sur `9f0e26c7`, le dashboard Podman rootless en activité montre deux workers chez A et un chez B, puis confirme par inspection les trois conteneurs encore actifs et les deux déconnexions. La contre-pression TCP, les I/O bloquées, la coupure abrupte en conditions réelles et l’agrégation restent à qualifier. [Docker en activité](evidence/2026-11-19/functional/native17-and-local-workers.md#déconnexion-des-deux-lecteurs-après-inspection-en-direct) · [Rejeu indépendant](evidence/2026-11-19/functional/native17-and-local-workers.md#rejeu-indépendant-du-retrait-des-données-après-déconnexion) · [Docker terminal](evidence/2026-11-19/functional/native17-and-local-workers.md#dashboard-terminal-des-trois-workers-docker) · [Podman rootful](evidence/2026-11-19/functional/native17-and-local-workers.md#dashboard-terminal-des-trois-workers-podman) · [Podman rootless](evidence/2026-11-19/functional/native17-and-local-workers.md#parcours-cli-et-dashboard-terminal-sur-podman-rootless) · [Chromium Linux](evidence/2026-11-19/functional/native17-and-local-workers.md#prévol-viewer-et-décisions-rust-dans-chromium-linux-arm64) Le bundle courant compile sous Node 24.22.2 sur Darwin ARM64 et les quatre assets concordent avec le build Node 25. La suite locale donne 247 PASS et 8 SKIP en attente de captures Rust. Le viewer Chromium Linux du bundle courant passe avec le backend Rust archivé. [Suite frontend locale](evidence/2026-11-19/functional/native17-and-local-workers.md#recettes-navigateur-indexées-et-suite-frontend-locale) Chromium affiche le résultat d’échéance de la même instance que le CLI dans les rejeux Docker et Podman rootless. [TTY Docker et Chromium](evidence/2026-11-19/functional/native17-and-local-workers.md#rejeu-docker-du-scénario-tty-partagé) [Échéance TTY Podman et Chromium](evidence/2026-11-19/functional/native17-and-local-workers.md#échéance-tty-sur-podman-rootless-et-chromium) |
| G09 | Résilience et exploitation | FAIL. Sur le gel `c7f6c966`, le pilote Docker Linux x86-64 échoue lors d’une nouvelle réservation de scan final, avec état `pending` et code enforcer `not_ready`. Les deux événements de diagnostic sont acquittés au contrôle central. Le reçu du runner relève deux conteneurs et un volume supplémentaires, sans nettoyage moteur attesté. Les tests hors ligne `b34c528e` et `9cee7b7c` couvrent les refus d’horloge à l’entrée et avant ACK. [Diagnostic Legacy](G09_LEGACY_RESERVATION_DIAGNOSTIC.md) [Pilote Docker](https://github.com/Needdoliprane/ai-guard/actions/runs/45353401742) Le rejeu Docker à un worker du correctif de nettoyage expire à 600,15 s. Le checkpoint durable enregistre `import_verification: unavailable` après l’acquittement du claim du helper, sans résultat de vérification. Le worker reste `Preparing`. L’inspection Docker du helper conservé confirme l’état `created` sans horodatage de démarrage. La base d’audit de la recette contient zéro événement `broker.batch.failure`. La candidate distingue désormais le cas `observe_only` et le dashboard le rend avec un effet moteur non confirmé. Le rejeu sur moteur du nouveau diagnostic reste à faire. Avant ce correctif, un rejeu expirait avec le worker à `Preparing` et un nettoyage d’import en attente d’ACK. Le rejeu précédent avait atteint `Created` puis acquitté l’Arm worker et un essai antérieur avait atteint `StartIssued`. Les deux derniers n’atteignent pas le SIGKILL prévu. Avant correction, le canari relève la prise en charge à 4,339 s puis expire avant le premier HTTP observé. Après suppression de la double lecture stricte avant le premier CAS du nettoyage, les deux régressions passent sur un gel privé. Dans le scénario distinct de perte de la CLI, un worker restait historiquement actif à 20,01 s pour une limite de 15 s. Le gardien TTY, le fencing et les autres pannes restent à qualifier. [Rejeux instrumentés](evidence/2026-11-19/functional/native17-and-local-workers.md#g09-docker-avec-instrumentation-du-claim) · [Échéance historique](evidence/2026-11-18/functional/docker-crash-autonomous-deadline.json) Historique candidat : perte d'échéance reproduite sur Docker Desktop après SIGKILL du CLI (worker encore Running après 7 s pour une limite de 5 s), arrêt/réconciliation ciblés ensuite vérifiés. |
| G10 | Capacité et ressources | PASS au périmètre V1 accepté sur `48f81d20`. Les deux jobs attestent 100 réservations SQLite sans démarrage et le parcours Docker A1/A2 chez A et B1 chez B, avec trois exports et quota final nul. [Reçus du gel](https://github.com/Needdoliprane/ai-guard/actions/runs/45344259411) La recette locale de trois workers passe avec 20,08 s de chevauchement sur Docker Desktop ARM64, 16,298 s sur Podman rootful et 18,312 s sur Podman rootless. Les plafonds sont de 6 vCPU et 3 Gio de RAM pour les workers. Le critère V1 combine la recette A1/A2 chez A et B1 chez B et la capacité de réservation pour un tenant. Le gateway a retrouvé 100 réservations après redémarrage et refusé la 101e. [Docker](evidence/2026-11-19/functional/native17-and-local-workers.md#trois-workers-a1-a2-et-b1-après-redémarrage) · [Podman rootful](evidence/2026-11-19/functional/native17-and-local-workers.md#podman-rootful-à-trois-workers-sur-deux-tenants) · [Podman rootless](evidence/2026-11-19/functional/native17-and-local-workers.md#parcours-cli-et-dashboard-terminal-sur-podman-rootless) · [Contrôle des réservations](evidence/2026-11-18/functional/v1-keycloak-discovery-control-and-journal.json) |
| G11 | Infrastructure, transport et PKI conditionnels | BLOCKED. Le CLI Kubernetes passe la réservation et son abandon rejouable avant effet sur un gateway Rust de recette. Docker Desktop rapporte un cluster en démarrage et son backend confirme 2 701 sondes en échec vers la même origine que le kubeconfig. Le relevé du 21 novembre indique toujours `Waiting for node to be ready` et EOF à la négociation TLS de `/readyz`. [Relevé Kubernetes](evidence/2026-11-19/functional/native17-and-local-workers.md#disponibilité-kubernetes-docker-desktop-le-21-novembre) Le 24 novembre, Docker Desktop Kubernetes v1.36.1 répond `ok` sur `/readyz` et le nœud est `Ready`. Le relevé en lecture seule sur `f0f76763` confirme la classe `ai-guard-rwop`, le pilote CSI enregistré et les politiques de refus déclarées. La CLI s’arrête avant effet et le driver de session n’est pas raccordé à un lancement opérateur. [Proposition de raccordement Source17](G11_SOURCE17_KUBERNETES_PROPOSAL.md) Les revues Kubernetes `can-i` sur `ab97129e` confirment pour chacun des deux brokers 15 opérations autorisées, 29 refus du contrat et 5 refus supplémentaires de listes globales. Sur `30ceede5`, le CLI valide l’admission à blanc avec un jeton court de recette, deux événements d’audit corrélés et zéro pod avant et après. Les ressources de recette sont retirées. Sur `3be925ba`, le PVC et le PV de recette sur `ai-guard-rwop` deviennent `Bound` en `ReadWriteOncePod` avec 1 Mi. Le retrait du PVC entraîne celui du PV selon la politique `Delete`, sans pod créé. Le montage RWOP, l’identité d’un workload en exécution, le lancement, l’application des politiques, les transports et le déploiement cloud restent à qualifier. [Preuve opérateur](evidence/2026-11-18/functional/v1-setup-recovery-operator-contracts.json) · [Disponibilité locale](evidence/2026-11-19/functional/native17-and-local-workers.md#disponibilité-de-kubernetes-docker-desktop) |
| G12 | Publication OSS, supply chain et preuves de release | BLOCKED. Sur `1c9e8e01`, transfert des quatre archives et du SBOM vérifié avec refus d’une altération. Installation temporaire Linux x86-64 et macOS ARM64 sur les mêmes archives, avec identité du CLI vérifiée. Le basculement manuel macOS par lien privé passe de `1c9e8e01` à `99cb885e` puis revient à `1c9e8e01`, entre deux builds de la même version. Une recette séparée restaure une sauvegarde Legacy SQLite schéma 2 en destination neuve au retour vers `1c9e8e01`. L’ancien CLI retrouve la tête et les deux événements sauvegardés, puis ajoute un troisième événement. Les deux builds déclarent `0.0.1` et le schéma reste inchangé. [Transfert](https://github.com/Needdoliprane/ai-guard/actions/runs/45344993418) · [Installation](https://github.com/Needdoliprane/ai-guard/actions/runs/45348160190) Licence Apache-2.0 ajoutée. Les simulations vérifient les sommes et les déclarations tag/commit/cible, ainsi que la version et la révision embarquées du CLI. Les deux workflows et leurs entrées ciblées sont indexés localement sur la candidate. Sur leurs sources courantes, 34 cas de garde, 10 tests du prévol fournisseur et 3 scénarios du tag manuel passent hors réseau. Sept tests frontend ciblés passent également. Au 24 novembre, le dépôt GitHub est public et l’API confirme que le signalement privé est activé. Le parcours de traitement d’un signalement reste à valider. Aucune release ni tag `v0.0.1` n’apparaît au relevé. [Procédure de signalement](../../SECURITY.md) Les reçus couvrent le transfert contrôlé du SBOM et des archives de recette. Le contrat actuel lie les artefacts au manifeste et à `BUILD-INFO` par `SHA256SUMS` et documente l’absence d’attestation cryptographique. [Périmètre de provenance](RELEASE_CHECKLIST.md) [Contrôle des archives](evidence/2026-11-18/functional/v1-release-archive-and-control-recovery.json) · [Contrôle du binaire](evidence/2026-11-18/functional/v1-setup-recovery-operator-contracts.json) · [Chaîne GitHub](evidence/2026-11-19/functional/native17-and-local-workers.md#chaîne-github-de-test-et-de-release)  Les 12 tests de métadonnées passent sur le CLI Linux ARM64 du gel P5. [Tests Linux de métadonnées](evidence/2026-11-19/functional/native17-and-local-workers.md#rejeux-cli-p5-et-état-de-santé-sqlite) |

Le TTY générique Docker Desktop ARM64 atteint sa borne d'exécution avec le CLI lié de la candidate. Le test rapproche l'arrêt acquitté, la sortie du worker, le quota libéré et le terminal restauré. Chromium relit le même événement observé depuis la copie de la base et le dashboard Rust correspondant. Cette preuve G07/G08 couvre la sonde générique sur ce moteur. [Rejeu du CLI et de Chromium](evidence/2026-11-19/functional/native17-and-local-workers.md#rejeu-docker-de-léchéance-tty-sur-le-cli-lié-de-la-candidate)

Le scénario Chromium général passe sur le bundle de la candidate avec la fixture HTTP locale. Le contrôle d'identité confirme douze réponses ou événements du tenant précédent et deux refus tardifs après la connexion du suivant. La suite Node passe 255 cas avec seize réponses de backends Rust archivés. G08 reste ouvert pour les captures filaires du backend courant, les parcours d'administration et la composition finale. [Bascule viewer et suite Node](evidence/2026-11-19/functional/native17-and-local-workers.md#bascule-viewer-avec-réponses-tardives-sous-chromium)

Le scénario local de garde des archives natives G12 confirme la présence et le type des six binaires Cargo de la candidate. Les 36 cas de brouillon simulé couvrent notamment le refus des archives incomplètes et la reprise d'un dépôt partiel. Les archives de livraison restent à qualifier. [Garde des compagnons](evidence/2026-11-19/functional/native17-and-local-workers.md#contrôle-des-compagnons-dans-les-archives-de-release)

Un statut porte uniquement sur la preuve nommée. Un lot logiciel réussi ne clôture pas les gates d'effets runtime. La publication production exige tous les gates applicables PASS, ou NOT_APPLICABLE justifié avant essais. Les quatre combinaisons Docker/Podman × Linux x86_64/macOS ARM64 restent exigées ; Kubernetes reste une cible complète distincte.

**G01 — harnais d’export avec un CLI installé.** Sur `c16ca486`, le
[témoin nominal](../../tests/runtime_user_journey.rs) passe sur Docker
Desktop ARM64 et Podman rootless avec le même CLI extrait de l’archive
macOS `99cb885e`. L’empreinte du CLI est vérifiée avant le prévol.
Le parcours confirme l’audit, les reprises et l’export en destination
neuve. Les volumes attribués aux recettes sont retirés et les deux
inventaires retrouvent leur état initial. Reçus de test
`g01-harness-macos-c16ca486/run.log` et
`g01-harness-podman-c16ca486/run.log`, SHA-256 respectifs
`d59636a9552c4d11cfbb0413ee6f1a3e10c772ddd5278cd378a3082667640266`
et `f1b96120b44724d43c1905a7c76bd0844356bcee1d844016e5b008aa5b3bb4e2`.
L’exécution du harnais avec l’archive Linux x86-64 reste à qualifier.

Le [workflow Linux x86-64](../../.github/workflows/g01-linux-archive-runtime.yaml)
est intégré localement depuis `5063391f`. Il sélectionne le même binaire
extrait de l’archive `1c9e8e01` pour Docker rootful et Podman rootless,
puis vérifie le retour des inventaires de conteneurs et volumes. Il
atteste séparément le retrait de l’étiquette d’image QA et relève le
nombre d’images de base ajoutées. Les contrôles locaux de syntaxe et de
projection des reçus passent. Aucun job Linux de ce workflow n’a été
exécuté. Le prévol Podman doit confirmer le mode rootless sur un runner
compatible avant tout lancement.

**G02 — montage racine observé dans le worker.** Avec le CLI macOS archivé
`99cb885e` sur Podman 6.1.4 rootless ARM64, la commande auditée
`findmnt --noheadings --output OPTIONS --target /` lit le jeton `ro` pour
la racine du worker strict, sans jeton `rw`. Le processus sort avec le
code 0, l’audit est valide, le quota est libéré et l’inventaire moteur
retrouve ses trois conteneurs et 26 volumes préexistants. Le témoin traite
le montage racine de ce worker, sans tentative d’écriture. La qualification
complète du confinement reste ouverte.
Reçu `g02-podman-findmnt-99cb885e.txt`, SHA-256
`d07bdd9ae43449212272f5ad7c62fa7b0cf6c3547bb7944904e72b19c11498d1`.

Le même CLI et la même image passent le témoin sous Docker Desktop 29.9.3
ARM64 avec l’exception rootful explicite. La commande auditée lit le jeton
`ro` sans `rw` pour `/`. Le gateway est fermé, le quota est nul
et les inventaires avant et après sont identiques (aucun conteneur, trois
volumes préexistants). Reçu `g02-docker-findmnt-99cb885e.txt`, SHA-256
`beffec4f888c2b9d6356da1bed16385673fbc473733f70069ea8821de8de4c39`.

Deux exécutions distinctes sur le même CLI et la même image lisent le
jeton `rw` sans `ro` pour `/workspace` sur Docker Desktop et Podman
rootless, sans écriture de sonde. Les audits sont valides, les quotas
sont libérés et les inventaires de chaque moteur retrouvent leur état
initial. Reçus `g02-docker-workspace-99cb885e.txt` et
`g02-podman-workspace-99cb885e.txt`, SHA-256 respectifs
`0e6a2d8e07fdeb556ae16bfb27cb8f6c6da7be5beac6f2654fce683e2ef13f69`
et `dc78dbc22b6b4178103bb0b6a8369c77e2a6617418bf9d730a42f980fd0ecbe0`.

Sur les deux moteurs, un worker rejoue la lecture auditée des fichiers
cgroup v2 et observe `memory.max=1073741824`, `pids.max=256` et
`cpu.max=200000 100000`. Les audits sont valides, les quotas reviennent
à zéro et les inventaires retrouvent leur état initial. Ces lectures
établissent les limites exposées au worker. Les effets à saturation
restent à qualifier. Reçus `g02-docker-cgroup-99cb885e.txt` et
`g02-podman-cgroup-99cb885e.txt`, SHA-256 respectifs
`0e8d57a5db996096552b5535c3f3589a57f6cd8d21dd4e51132c7b72e79d1e18`
et `ac11c2ccc1097d297a7021aa9f8b9040137f072ec91e8f80811e2a856c21a02c`.

Sur les mêmes deux moteurs et le même CLI, une commande auditée lit
`/proc/self/status` dans un worker strict sans réseau. Les quatre
valeurs UID et GID sont à 1000, les cinq masques de capacités sont nuls,
`NoNewPrivs=1` et seccomp est en mode filtre (`Seccomp=2`). Les audits
sont valides et les inventaires reviennent à leur état initial. Reçus
`g02-docker-proc-99cb885e.txt` et `g02-podman-proc-99cb885e.txt`, SHA-256
respectifs `51c458b1efe32d2856baafb41cc13d0a8ec381900ddd966b6bb3a4b51ba6aaed`
et `b9a52f4baf2547ff048dd5d12bf513733e208803ddb36739f4156d3f5aa5ff78`.

Une lecture auditée des tables réseau des workers en mode `none` sur
`99cb885e` observe zéro route IPv4 et zéro route IPv6 via une interface
autre que loopback sur les deux moteurs. Le premier oracle Docker exigeait
aussi l’absence d’autres interfaces et a échoué sur neuf interfaces de
tunnel. La relecture hors ligne de la même sortie confirme le résultat
sur les routes et conserve l’échec initial. Le témoin Podman ne voit que
loopback. Aucun trafic émis. Audits valides, quotas libérés et inventaires
inchangés. Reçus de relecture `g02-docker-netnone-route-recheck-99cb885e.txt`
et `g02-podman-netnone-route-recheck-99cb885e.txt`, SHA-256 respectifs
`50cc7243f0164fa46fc82901bcb3fa3b649571f16aa46085d4b090a33ad009d0`
et `7a59d6ffdbc0434b4bf55efbe7f037885252a4f71e71fa288c9840f0fb292967`.

**G06 — refus d’une copie SQLite Store14.** Sur `fc8dba50`, le
[témoin CLI](../../tests/human_sources_bootstrap.rs) lit le tenant A avec
la base originale et ferme le gateway. Après une sauvegarde SQLite Online
Backup en destination neuve et substitution au chemin attendu, la même
commande refuse la copie d’identité physique différente. Les deux bases
et les fichiers d’activation surveillés restent intacts. Le test ciblé
passe en 8,86 s et deux régressions des fonctions communes passent. La remise
en service d’une copie restaurée reste ouverte. Reçu
`g06-store14-fc8dba50/store14.log`, SHA-256
`a24c79d8d9defcefc3020083075950404e2f5cc6c773298ec663d0395b723c30`.

**G07 — sélection et préparation hors ligne.** Sur `5063391f`, le
[prévol de recette](../../tests/e2e/test_run_provider_preflight.py) passe
13 tests dont 16 sous-cas du compagnon de diagnostic macOS installé
`99cb885e` (quatre couples client/mode, deux transports, référence
synthétique présente ou absente). Le
[test Rust de préparation](../../tests/provider_bootstrap.rs) passe 1/1
et couvre les huit plans, les répertoires privés et le choix exclusif du
mode d’authentification. Aucun fournisseur ou terminal réel n’est lancé.
Les connexions et conversations réelles restent à qualifier. Reçus
`g07-provider-preflight-5063391f.log` et `g07-bootstrap-5063391f.log`,
SHA-256 respectifs `bcaac8ceb80871fca69fc70b281a3d4c30319667a56d639388d5b4625d75a1e3`
et `3f39a706000723482cd4643197d5e23c660ed55b30ddf231b6a396063887e8cf`.

Le prochain témoin Codex API vise une soumission batch supervisée bornée.
Le résultat doit lier, pour la même tentative, l’autorisation durable,
l’admission, le démarrage inspecté, la décision réseau précédant la
connexion, puis le terminal et la libération du quota. La recette attend
le raccord du canal au superviseur et l’arbitrage E3 de G05. Le profil
réseau `none` et la médiation approuvée sont exigés sur le moteur choisi.
Ce témoin n’a pas été lancé sur `5a6cd5c3`.

**G08 — composition du registre.** Sur `26e19e84`, le
[test d’agrégation du gateway](../../tests/supervision_stack_api.rs) passe
1/1 en 21,09 s avec SQLite. Le parcours restitue les 120 lignes synthétiques
du tenant A sur plusieurs pages sans doublon, montre ses trois brokers
(`current`, `revoked`, `unseen`) et exclut les lignes de B. La couverture
reste `partial` et l’inventaire runtime `unknown`. Aucun broker ou worker
réel n’est lancé.
Reçu archivé `g08-aggregation-26e19e84.log`, SHA-256
`75b57c868e93320aa414f7fe355238aa434de494b4d886497e32a69a07b00e9f`.

Sur `07e13386`, le [test des deux brokers](../../tests/broker_presence_cli.rs)
confirme deux présences durables `current` dans une même lecture humaine
Rust pendant que les deux processus `broker serve` sont actifs. Il vérifie
leur sortie et la libération des verrous des gardiens. La suite de présence
passe 4/4 avec `TMPDIR=/private/tmp` sous macOS. Avec le répertoire macOS
par défaut, les trois cas préexistants échouent avant disponibilité. Les
workers ne sont pas lancés et l’inventaire runtime reste `unknown`.
L’agrégation de workers observés par plusieurs brokers reste à qualifier.
Reçu PASS `g08-presence-suite-short-tmp-07e13386.log`, SHA-256
`248531ff2f99a4b1803952a9d5773dde421f7cded7b684b01604b4e2e2180cef`.

Compléments explicites du mainteneur,30octobre : G07 exige Codex CLI et Claude Code, chacun avec abonnement et clé API, sélection dynamique annoncée, aucun repli silencieux vers une API facturée et aucun cache OAuth extrait implicitement. G08 exige une vue Web unique de tous les agents/workers du tenant sur ses brokers/hôtes/runtimes, avec provenance, fraîcheur et états partiels ; pas un dashboard par worker. G04/G06/G10 couvrent les quotas et le comptage globaux du tenant. Pour G10, l’arbitrage du 24 novembre porte sur 100 réservations durables chez A et une recette sur moteur avec A1/A2 chez A et B1 chez B. Aucun de ces compléments n'est clôturé par sa seule inscription ou par le `.env` local créé.

Le gateway distingue désormais API TypeSafe, règles Rust et modèle local consultatif. Un premier essai historique Qwen 0,5B a renvoyé `allow` pour une suppression, ce qui maintient le candidat hors qualification pour autoriser une exécution. Le parcours actuel évalue un avis via Rust, maintient le veto impératif et permet une relecture durable après arrêt du modèle. Cette preuve couvre un gateway natif avec modèle Docker Desktop. Une seconde recette vérifie une décision Qwen avec gateway et relais TLS conteneurisés ainsi que son reçu et son export dans Chromium. Le corpus modèle, Podman, Kubernetes et le déploiement Compose complet relèvent de G04/G07/G10. [Essai historique](evidence/2026-10-30/local-judge/README.md) · [Essai intégré](evidence/2026-11-18/functional/qwen-local-gateway.json)

Compléments du 31 octobre : le [contrat canonique d'identité et de configuration](../sso.md) et les [vues selon les permissions](../ui.md#vues-utilisateur-et-administration-unifiée--cible-à-livrer) s'ajoutent aux critères ci-dessus. **À implémenter et qualifier**, sans modification des résultats historiques :

| Gates concernés | Preuves obligatoires supplémentaires |
|---|---|
| G04 | Identité locale nominative hors ligne ou OIDC public PKCE S256, mapping configurable des claims vérifiés, permissions et périmètres côté Rust, délégations sans élévation ; les anciennes clés ne prouvent pas l'identité humaine |
| G05/G07/G11 | Inférence interne avec URL/modèle/format exact/adaptation Claude, credential minimal et chaîne publique d'AC, TLS validé et réseau interne approuvé sans repli cloud ; combinaisons incompatibles explicitement refusées |
| G06/G07 | Causalité individuelle, couverture réelle d'observation, audit indépendant protégé, durable, vérifiable et exportable avec points de contrôle signés ; limites et pertes déclarées |
| G08 | Parcours Web/CLI et vues utilisateur/administration à 360° du seul périmètre autorisé, agrégation Rust et retrait des données après changement de droits/contexte |
| G09/G12 | Installation documentée depuis les Dockerfiles, construction/distribution des images par le client sans image publique obligatoire, profil Keycloak local opt-in à quatre comptes et procédure de raccordement entreprise ; parcours réel chronométré, sans garantie des cinq minutes |

Le produit doit qualifier son support SSO même si une installation le laisse désactivé. La [matrice de tests](TEST_STRATEGY.md#identité-individuelle-sso-et-découverte--à-qualifier) couvre les axes indépendants et leurs interactions critiques ; elle ne promet pas toutes les combinaisons. Cette mise à jour documentaire ne ferme aucun gate.

## Preuves datées de la candidate — 15 au 17 novembre 2026

Entrées conservées de la candidate V1 intégrée. L'état de qualification courant est celui du tableau ci-dessus ; ces entrées restent attachées à leurs gels, sources et environnements et ne le contredisent pas.

### 17 novembre — D7 : preuve logicielle supplémentaire

La [contribution D7](evidence/2026-11-17/d7-native-stop-owners/README.fr.md) intègre le code issu de `6a32771`, avec 16 tests lib ciblés PASS sur son gel et Clippy RC0 **non strict** (561 émissions). Elle ne clôture aucun gate : reprise froide/retirement, quota, STOP physique, Docker/Podman/Kubernetes et dimensionnement restent ouverts. Les résultats de sources antérieures restent séparés. **0/12 gates clos** ; aucune release qualifiée.

### 17 novembre — relevé runtime et finalisation tardive D1

[Douze cas logiciels ciblés](evidence/2026-11-17/runtime-observation-and-d1-late-finalizer/README.fr.md) passent sur le gel `c755fb52…db3b9`, dont quatre vrais CLI/Gateway/SQLite avec moteur Rust de fixture : terminal unique, faute Managed tardive et quota/instance conservés pour Created/Unknown. Le relevé facultatif est borné, sans attente dans STOP, et ne remplace pas l’audit durable. Le précédent FAIL de sonde reste historique ; la régression lit désormais la capture dans son véritable propriétaire d’erreur. Compilation des trois tests et six listes vérifiées ; Clippy toutes cibles de cette révision RC0 non strict, 528 émissions d’avertissement, 0 erreur ; 21 émissions supplémentaires restent suivies. Ni moteur réel, reprise froide, fournisseur, scénario A1/A2/B1 complet ni capacité 100 ne sont qualifiés. **0/12 gates clos**.

### 16 novembre — D1 : quatorze PASS logiciels, aucune qualification globale

Le [lot D1](evidence/2026-11-16/control-monitor-prestart-fix/README.md) corrige l’admission préstart au premier effet et conserve le propriétaire, les causes/captures et le terminal natif connu. **14 PASS** vérifient quatre vrais CLI ai-guard/Gateway/SQLite avec moteur Rust de fixture et dix unités/voisins Managed, PTY et deadline sur leur source exacte. La fenêtre post-ACK reste 60 s ; aucun réglage de sûreté n’a été réduit. Les deux fichiers D7 cfg(test) de la composition sont exclus du commit D1.

Clippy D1 toutes cibles sur la source composée `74d60de1…b7b` : **RC0 non strict, 507 émissions d’avertissement et 0 erreur**. Reçu `dc43ad33…0e0a2` ; gate strict ouvert. Faute tardive exacte jusqu’au Store/quota, vrai TTY interactif, abandon/reprise froide, Docker/Podman, fournisseurs et cluster restent non qualifiés. Le verdict primaire D1 n’est pas réécrit ; D7 reste ouvert : le premier setup avant oracle est préservé ; après correction de la seule configuration TMPDIR, cap2 PASS et cap1 RED au vrai oracle de couverture sont vérifiés, sans correctif produit D7 ni qualification runtime. **0/12 gates G01–G12 clos**, G09 conserve son FAIL ; aucune release ni qualification production n’est déduite de ces tests.

### 16 novembre — R2 runtime/Kubernetes : preuve logicielle ciblée, gates ouverts

La [composition publique R2](evidence/2026-11-16/runtime-kubernetes-batch-composition/README.md) passe format, Clippy toutes cibles **non strict (505 diagnostics d'avertissement)**, build et **26 cas exacts**, vérifiés sur ses sources et binaires propres. R1 et son Clippy FAIL restent historiques ; les PASS standalone ne sont pas transférés.

**0/12 gates G01–G12 clos ; aucune activation de production.** Aucun nouveau moteur, délai physique, Native17/S5, cluster ou capacité100 qualifié. MetadataFD reste FAIL avec quatre dépendants NOT_RUN ; l'audit D1 reste ouvert.

### 16 novembre — Native17 R6 reste non qualifié

Le contrôle R6 du démarrage configuré échoue ; la réussite du build ne qualifie pas le comportement et la rétention durable n’est pas établie. Aucun gate G01–G12 n’est clos (**0/12**). [Preuve R6 et périmètre](evidence/2026-11-16/startup-runtime-failures/README.md#native17-r6-one-configured-startup-oracle-remains-failing).


### 16 novembre — startup Native17 toujours non qualifié

La [preuve R11](evidence/2026-11-16/startup-runtime-failures/README.md#successors-r10-and-r11-software-results-remain-separate) ajoute 18 régressions réussies mais conserve un refus du démarrage configuré et six cas non exécutés. Les 13 cas DATA/FS ne qualifient pas la construction complète. Un nœud Kubernetes Ready ne qualifie pas les workers ni leurs frontières. **0/12 gates clos.**

### 16 novembre — contribution logicielle aux bornes HTTP

Le [lot des listeners](evidence/2026-11-16/http-listener-bounds/README.md) apporte 11 régressions réussies et ses guides EN/FR. G08 reste ouvert : seuils 128/15s non dimensionnés, streams HTTP/2 suivants et disponibilité déployée non qualifiés ; aucune clôture automatique des constats NV-07/V4. **0/12 gates clos.**

### 16 novembre — qualifications startup/runtime encore en échec

Le [lot de mesures](evidence/2026-11-16/startup-runtime-failures/README.md) laisse le démarrage Native17 et le cas Docker générique en échec. Le refus post-COMMIT et le délai d’arrêt exigent leurs corrections et régressions. Les observations positives de profil/nettoyage ne qualifient pas le cas complet ; export, recette A1/A2/B1 et autres moteurs restent ouverts. **0/12 gates clos.**

### 16 novembre — résilience locale du callback OIDC

Le [lot NV-09](evidence/2026-11-16/oidc-callback-correlation/README.md) vérifie corrélation, bornes et continuité d’une tentative CLI sur macOS. G04 et les autres gates restent ouverts : le SSO réel, Native17, les autres principals et plateformes ne sont pas qualifiés par ces 14 cas.

### 16 novembre — contribution logicielle aux contrôles runtime

Le [lot image locale](evidence/2026-11-16/runtime-local-image-policy/README.md) apporte des régressions du builder. Il ne qualifie pas le moteur, les sorties réseau ou la chaîne de livraison et ne clôture aucun gate G01–G12.

### 16 novembre — lot logiciel Kubernetes dédié intégré

Le [lot lifecycle/attach isolé](evidence/2026-11-16/kubernetes-dedicated-integration/README.md) passe son check toutes cibles et son build libtest, puis **17/17 tests logiciels exacts** sur `1d274954`. Après correction du formatage seul (`cb1ca7f`), le contrôle de format des 18 fichiers Rust et un nouveau check toutes cibles passent ; les 17 tests ne sont pas réexécutés. La revue indépendante ne signale aucun blocage confirmé dans ce périmètre pré-effet. Seuls les 21 chemins dédiés sont repris, avec la garde canonique de replay conservée. Native17/S5, le vrai cluster et la qualification complète restent ouverts : **0/12 gates clos**.

### 16 novembre — tests natifs exécutés et raccords en correction

Le [build natif macOS R16](evidence/2026-11-16/session-control/TARGETED_NATIVE_BUILD_R14_R16.md#native-macos-r16-successor--actually-executed) réussit, puis les 12 tests ciblés donnent **9 diagnostics PASS et 3 sauvegardes FAIL**, sans timeout ni relance. Les trois refus surviennent avant l’oracle de sauvegarde : `human_authority_epoch_expired` au démarrage ; leur cause reste à corriger ou expliquer. La composition suivante Config/E2, revue puis appliquée localement, échoue à son check toutes cibles avec **40 erreurs de compilation** ; ses 55 nouveaux tests sont NOT_RUN. Les preuves restent liées à chaque source. **0/12 gates clos ; aucun pourcentage fonctionnel fiable.**

### 16 novembre — compilation native R13

La [compilation R13](evidence/2026-11-15/session-control/COMPOSED_FULL_NATIVE_CHECK_R9.md#r11r13-successor-checks-2026-11-1516) est **PASS** sur son gel exact, avec 2 065 avertissements. Les tests de comportement, parcours sessions, OIDC/configuration/reprise, runtime et fournisseurs restent à prouver. Cette étape ne ferme aucun gate : **0/12 G01–G12 clos**.

### 15 novembre — compilation du successeur natif R10

Le [successeur corrigé R10](evidence/2026-11-15/session-control/COMPOSED_FULL_NATIVE_CHECK_R9.md#r10-successor-check-2026-11-15), 3 307 fichiers, a subi un nouveau check toutes cibles : **FAIL, RC 101, 7 erreurs codées**, 2 notes de résumé sans code et 60 avertissements. Une seule invocation Cargo a été exécutée ; aucun test. Collecte, gardes, source/cache et fermeture du CID exact sont vérifiées, sans hausse des compteurs OOM.

La compilation reste **FAIL** sur le seul gel `1c75fb67…`. Les postchecks et la fermeture réussis qualifient la collecte de cette campagne, sans clôturer une exigence produit. Les tests, le formatage, Clippy, les parcours natifs, la reprise et les moteurs/fournisseurs réels restent non qualifiés par R10. **0/12 gates G01–G12 clos** ; aucun pourcentage global et aucune qualification production n’en découlent.

### 15 novembre — compilation du noyau natif complet R9

Le [gel complet réellement appliqué R9](evidence/2026-11-15/session-control/COMPOSED_FULL_NATIVE_CHECK_R9.md), 3 291 fichiers, a été compilé une fois : **FAIL, RC 101, 64 erreurs codées**, 2 résumés distincts et 62 avertissements. Il expose des dépendances et raccords encore incomplets, sur un périmètre plus large que R3. Collecte, source/cache et fermeture sont vérifiées, sans OOM observée. Aucun test exécuté par cette campagne.

Trois lots corrigent séparément configuration, runtime/conversation et audit. Le correctif relu des trois agrégations d’authentification est appliqué après le gel R9, avec échéance inchangée et propriétaires retenus ; sa compilation et ses quatre régressions restent NOT_RUN. Les parcours natifs, OIDC, reprise réseau et configuration restent à terminer et qualifier. **0/12 gates production clos** ; pas de pourcentage fonctionnel fiable.

### 15 novembre — preuves ciblées et intégration native en cours

Le [frontend natif de nouvelle autorisation](evidence/2026-11-15/session-control/FRONTEND_NATIVE17_QA_R1.md) a réellement passé 7 nouvelles régressions et 46 canaris : **53 PASS, 0 FAIL**, build TypeScript/Vite PASS. Navigateur et parcours gateway/broker/fournisseur restent NOT_RUN pour ce delta.

Deux contrôles Rust distincts restent **FAIL** : le [baseline réparé R2](evidence/2026-11-15/session-control/COMPOSED_BASELINE_CHECK_R2.md), avec 4 erreurs codées, et le [candidat courant R3](evidence/2026-11-15/session-control/COMPOSED_LIVE_CHECK_R3.md), avec 11 erreurs de raccordement sur un périmètre plus large. Les correctifs de tailles/financement et d’API sont relus et appliqués après ce dernier gel, sans crédit de recompilation. Collecte, source/cache et fermeture des QA sont vérifiées ; aucune OOM observée lors de ces deux checks.

Le [lot Kubernetes lifecycle/attach](evidence/2026-11-15/session-control/KUBERNETES_LIFECYCLE_QA_R4_R5.md) a réellement passé son check toutes cibles et 16 de ses 17 cas logiciels sur le premier gel ; le cas restant échouait dans sa fixture. Après correction de cette seule fixture, le retest ciblé du successeur passe **2/2**, dont un canari inter-tenant. Les 16 PASS historiques ne sont pas transférés comme une suite réexécutée. Formatage et Clippy restent NOT_RUN, ainsi que tout essai cluster. Le noyau natif, le contrôle réseau et les parcours de session restent à composer puis vérifier. La révocation du sender après autorisation avant émission reste un écart explicite du contrat. **0/12 gates production clos** ; aucun pourcentage fonctionnel fiable n’est établi. Ce commit documentaire ne qualifie ni les candidats privés ni la branche publiée.

Fédération C3 composée du 14 novembre : 44 parents logiciels uniques PASS sur
`f117ceae016049b7274708197c3702218148e56b`, compilation fraîche, 14 listes
réelles et check toutes cibles RC0. Rotation/réconciliation, floors/journal
partagés, proxy/routes, page humaine Store14 réellement auditée avec refus,
canaris legacy et neuf B9 chauds sont testés dans ce seul run.
[Preuve bornée et historiques](evidence/2026-11-14/federation-composition/README.md).
Les 2876 sources restent exactes après run ; fixtures, OOM0, fermeture du CID
exact et flush séparé sont vérifiés. 50 postimages de format lié (49 historiques + 1 fixture)
et helper check courant ; 34/168 warnings lib/check conservés, Clippy strict
NOT_RUN. R1 compile 101/E0308 et R2 9 PASS/2 FAIL/33 NOT_RUN restent inchangés.
Le positif humain ne qualifie pas Store15/16. D5/fencing global, reprise froide,
Source16/E2 distinct, multi-hôtes, réseau et workers/moteurs/fournisseurs/cluster
réels, navigateur, capacité 100 et suite complète restent hors preuve.
Aucune nouvelle UI ou version 0.0.1 qualifiée ; `f117` reste le gel 0.1.0 testé.
G09 demeure FAIL ; aucun gate G01–G12 n'est clos (0/12).

Broker Submitted/Pending du 14 novembre : neuf parents logiciels uniques PASS
sur `a4340c4f`, format courant des trois sources, compilation et check toutes
cibles RC0, 169 avertissements hors des trois fichiers modifiés. Source après
exécution, fixtures, fermeture exacte/OOM0 et flush Root vérifiés ;
[preuve bornée](evidence/2026-11-14/broker-submitted-pending/README.md).
L'original retenu converge de Pending+Q vers Final puis retrait local sans
nouveau start/claim ni retransmission de RetireUnused. Le RED initial,
l'abort134 de R1 et la sonde de pile restent
leurs résultats datés ; la variation test-only qui rétablit le canari ne
démontre pas une correction de pile du produit. Clippy strict, Unsubmitted
audité, reprise froide complète, D5/fencing, familles namespace/restauration
et NativeResume, moteurs et capacité100 restent ouverts. Aucun terminal
worker ou quota libéré n'en découle ; G09 FAIL, 0/12 gates globaux clos.

Diagnostic opérateur E du 12 novembre : neuf parités binaires et 85 cas CLI
normaux PASS sur le gel composé `c2b907a8…`, check toutes cibles RC 0,
export vivant exact et fermeture RC 0/OOM 0. L’exporteur manuel ignoré reste
`NOT_RUN` dans cette campagne ; sa régénération séparée et la promotion des
neuf exemples ont leurs propres preuves. [Sources et limites](evidence/2026-11-12/operator-diagnostics/README.md).
Ce diagnostic passif, avec découverte OIDC explicitement demandée testée sur
loopback seulement, améliore les procédures opérateur sans qualifier leur
exécution réelle : moteurs, fournisseurs, IdP réel, macOS natif, image/release,
Clippy strict et suite complète restent hors de ce lot. Aucun gate G01–G12
fermé ; G09 conserve son FAIL et G12 reste BLOCKED.

Récupération centrale S2 du 9 novembre : 50 parents ciblés PASS sur leurs deux
gels et check toutes cibles RC0, revues traitées et sources composées vérifiées.
[Preuves et limites](evidence/2026-11-08/gateway-session-recovery/README.md#targeted-green-and-integration).
Ce raccord avant effet n'active ni Pod, bootstrap, worker, transport ou reprise
physique. Warnings/Clippy, D5, moteurs et autres qualifications restent ouverts.
G09 FAIL, aucun des 12 gates globaux fermé.

Profil descriptif Kubernetes V2 du8novembre : `c4d0cd6` composé dans `9af5e99`,
onze fonctions et check toutes cibles PASS sur k2, revue traitée.
[Preuves et portée](evidence/2026-11-08/kubernetes-session-profile-v2/README.md).
Activation, bootstrap, identité/stockage réels, lifecycle et Clippy k2 restent
ouverts ; aucun gate global fermé, G09 FAIL. Le RED préclaim98 a aussi révélé
une libération de lease avant la fin de l'audit : correctif et GREEN requis.

Réserve d'audit du7novembre : `1bb3bba` composé dans `d550c34`,19 fonctions
uniques et toutes cibles vérifiées, revue indépendante. [Preuves](evidence/2026-11-07/d5-control-audit-reserve/README.md).
Clippy strict est désormais exécuté : **FAIL89 diagnostics**, sans analyse
réussie des cibles suivantes ou suppression de lint. G12 demeure ouvert.
Les diagnostics V1/V2 et les deux consumers D5 passent dans une composition
séparée, mais Export n'a pas encore atteint son oracle. Owner/exec du relais
restent des composants privés à raccorder ; aucun transport runtime qualifié.
G09 FAIL,0/12 gates globaux clos.

Export du 7 novembre : filtrage renforcé, modes observés et compatibilité fermée V1
intégrés après RED et revue. 49 fonctions ciblées passent sur leurs gels et toutes
cibles du gel 03 compilent ; [preuves](evidence/2026-11-07/workspace-export/README.md).
Le harnais de reprise corrigé ne change aucun délai produit. APFS, moteurs réels,
migration de processus et pannes physiques restent NOT_RUN dans ce lot. Le contenu
exporté reste non fiable et les avertissements/Clippy ouverts. G09 FAIL, 0/12 gates clos.

Reprise FD du6novembre : dix fonctions Store ciblées, compilation et revues
passent ; [preuves](evidence/2026-11-06/store-resource-recovery/README.md).
Ce résultat ne qualifie ni disponibilité TCP sous pression, ni workers/moteurs,
ni autres plateformes. G06/G08 restent ouverts, G09 FAIL,0/12 gates clos.

Reprise de présence du6novembre : même acteur/Pending après panne temporaire,
dix-sept fonctions et check lot/composition toutes cibles PASS, revue traitée.
[Preuves](evidence/2026-11-06/presence-recovery/README.md). Cette disponibilité
de déclaration ne qualifie ni fédération physique, worker, runtime ou fencing.
Observation de coût ignorée NOT_RUN, G12 warnings/Clippy ouverts ; G09 FAIL,
0/12 gates globaux clos.

Confiance gateway du CLI humain du6novembre : option CA publique explicite,
snapshot local/OIDC immuable, reprise export liée au bundle, dix fonctions
ciblées et check toutes cibles PASS, revue traitée. Les
[preuves](evidence/2026-11-06/human-gateway-trust/README.md) restent logicielles
sur TLS/SQLite locaux. Aucune qualification G08/G11 d'IdP réel, navigateur,
PKI entreprise ou transport multi-hôtes ; G12 conserve avertissements et
Clippy absent, G09 FAIL et0/12 gates globaux clos.

Confirmation de quarantaine du 6 novembre : seize fonctions ciblées PASS sur
leurs gels, vraie boucle service, revue indépendante traitée et check toutes
cibles RC0,118 avertissements lib/3 lib-test. [Preuves](VALIDATION.md#6-novembre--confirmation-de-quarantaine-sans-republication).
Le refus d'une publication non scellée reste fermé ; la fin d'une pagination
n'empêche plus les autres profils d'avancer. Aucun nouveau droit d'exécution,
checkpoint publié, HTTP ou arrêt runtime n'est déduit d'une confirmation.
Perte de processus, reprise froide complète, D5/fencing et moteurs restent
ouverts ; G09 FAIL, aucun gate global clos.

Registre workers humain du 6 novembre : permission explicite, migration9→10,
projection/audit/curseurs et vue administrative intégrés. Quarante-cinq fonctions
Rust et quarante-six Node uniques PASS sur leurs gels, tsc et check de composition
RC0, revue indépendante traitée. [Preuves](VALIDATION.md#6-novembre--registre-workers-humain).
Le navigateur/build frontend complet restent BLOCKED ; aucune permission ou
preuve de transport distant, propriété personnelle, capacité100 ou moteur réel
n'est déduite. Attribution initiateur/délégation et parcours cloud complet restent
ouverts. G09 FAIL, aucun gate global clos.

Propriétaires Full du 6 novembre : quatre parents Full et cinq canaris Cold
PASS, compilation toutes cibles RC0 et revue indépendante traitée. Le refus de
capacité précède les mutations/ACK ; les futurs acceptés restent possédés pendant
l'arrêt normal. [Preuves](VALIDATION.md#6-novembre--propriétaires-full-et-admission-avant-ack).
Aucune borne maximale d'arrêt, observation terminale runtime, mort de processus,
reprise froide ou garantie D5/fencing n'en découle. La qualification des moteurs
réels reste obligatoire ; G09 FAIL, aucun gate global clos.

Projection centrale du 6 novembre :4 parents Rust (dont vrai worker refusé),
wire Rust→TS,24 fonctions Node et tsc PASS ; revue traitée, check toutes cibles
RC0 après augmentation du budget mémoire de la QA. Les PASS ne qualifient pas
la connexion multi-hôtes, le navigateur ni la vue SSO humaine.
[Preuves et limites](VALIDATION.md#6-novembre--projection-fédérée-du-registre).
P1 obligatoire supplémentaire : contrôle des destinations réseau depuis le
périmètre Rust de confiance, révocation et audit avant connexion, avec
qualifications Docker/Podman/Kubernetes et couverture G05/G07/G11. Ce contrôle
n'est pas livré par bridge, une variable proxy ou la seule politique Jev.
G09 FAIL,0/12 gates clos.

Prérequis Kubernetes du 6 novembre : CLI opt-in, avis SSAR bornés et refus des
namespaces non actifs raccordés au préflight/CREATE retenu. Vingt-deux fonctions
ciblées PASS, reprise du parent d'oracles corrigés PASS, revue source et check
toutes cibles. [Périmètre et preuves](VALIDATION.md#6-novembre--prérequis-kubernetes-explicites).
Ni SSAR ni peer TLS ne qualifient le cluster Docker Desktop, les sessions ou
le confinement. Sessions V2, permissions effectives et stockage exclusif
restent ouverts ; G09 FAIL,0/12 gates clos.

Drainage des owners d'audit du 6 novembre : six fonctions ciblées PASS,
compilation toutes cibles RC0 et revue indépendante traitée. Cette preuve
sur le même helper d'arrêt que serve ne qualifie ni SIGTERM/processus, ni
runtime réel, ni le drainage des exécutions Full. [Résultats](VALIDATION.md#6-novembre--drainage-des-propriétaires-daudit).
Sortie de quarantaine, D5/fencing et qualifications réelles restent ouverts.
G09 FAIL,0/12 gates globaux clos ; Clippy absent et avertissements inchangés.

Raccord du propriétaire authentifié du 6 novembre : deux parents finaux PASS,
compilation toutes cibles RC0 et revue indépendante traitée. Vraie boucle et
ACK partiel sont vérifiés avec HTTP/SQLite et moteur de seed synthétique.
118 avertissements bibliothèque/3 lib-test et Clippy absent restent ouverts.
[Preuves](VALIDATION.md#6-novembre--propriétaire-authentifié-et-boucle-audit-restreinte).
Le drainage ciblé figure ci-dessus ; sortie de quarantaine, D5/fencing et qualifications réelles
restent ouverts. G09 FAIL,0/12 gates globaux clos.

ACK retenu Stops du 6 novembre : 12 parents métadonnées et 5 acteur PASS,
compilation composée toutes cibles RC0, revue indépendante traitée. Les erreurs
de harnais ne sont pas créditées au comportement. Client/ServiceCold ont depuis
le raccord ciblé ci-dessus ; sortie de quarantaine et qualifications restent
ouvertes ; réserve logique sans
garantie physique. [Preuves](VALIDATION.md#6-novembre--ack-retenu-composé-au-writer-stops).
162 avertissements bibliothèque/6 lib-test et Clippy absent restent ouverts.
G09 FAIL, 0/12 gates globaux clos ; aucune qualification de release.

Contrôle des faits terminaux du 5 novembre : parent ciblé PASS après RED et
trois contre-exemples de revue corrigés, compilation toutes cibles RC0 et revue
traitée. Contrôle pur descriptif, sans identité ni droit d'ACK ; ServiceCold et
fencing ouverts. [Preuves](VALIDATION.md#5-novembre--cohérence-pure-des-faits-terminaux-retenus).
181 avertissements bibliothèque/8 lib-test et Clippy absent restent ouverts ;
G09 FAIL, 0/12 gates globaux clos, aucune qualification runtime nouvelle.

Préparation de capacité d'audit du 5 novembre : 7 parents ciblés PASS et compilation
toutes cibles RC0, revue indépendante sans blocage dans ce périmètre. Réserve
logique seulement ; writer/identité/terminal/ACK et boucle froide à composer.
Les 170 avertissements bibliothèque et 5 lib-test restent ouverts, Clippy absent.
[Preuves](VALIDATION.md#5-novembre--capacité-logique-de-laudit-v4-retenu).
G09 FAIL, 0/12 gates globaux clos ; aucune qualification runtime supplémentaire.

Admissions du writer du5novembre :8parents ciblés PASS, compilation toutes
cibles RC0, revue indépendante traitée ;139avertissements bibliothèque/3lib-test
restent ouverts. La réserve de queue n'est pas une réserve durable ni une
reprise froide. [Preuves](VALIDATION.md#5-novembre--admissions-raii-et-réservation-de-queue-du-writer).
G09 FAIL, aucun gate global clos ; aucun runtime supplémentaire qualifié.

Port HTTP d'audit retenu du5novembre :3nouveaux parents et1canari existant
PASS après RED, compilation toutes cibles RC0, revue indépendante traitée.
Les133avertissements bibliothèque/3lib-test et Clippy absent restent ouverts.
Ce port descriptif et son reçu exact ne sont pas encore composés au writer ;
la boucle froide reproduit toujours l'absence de publication de2événementsV4.
[Preuves et limites](VALIDATION.md#5-novembre--port-http-exact-de-laudit-retenu).
Aucun gate global clos ; G09 reste FAIL, aucun runtime ou cluster qualifié.

Raccord broker HTTP Kubernetes du5novembre :17fonctions ciblées uniques PASS
sur leurs gels, ACK live privé, journal borné et reprise exacte après perte
de lecture de réponse centrale. La vue workers distingue aussi les autorités
de lancement/inspection,15fonctions Node/API et contrôle TypeScript PASS.
[Preuves et limites](VALIDATION.md) ; ces lots ne ferment aucun gate global.
Le peer Kubernetes reste déclaré, activation/session de production fermées,
reprise froide/fencing, relais runtime et vraie fédération multi-hôtes ouverts.
Compilation composée toutes cibles PASS,115warnings bibliothèque et3lib-test,
garde sans avertissement non satisfaite ; Clippy Linux BLOCKED (composant absent).
G09 reste FAIL,0/12 gates globaux clos ; aucune qualification de release.

Réconciliation après annulation, 5 novembre : quatre parents fonctionnels
et deux canaris PASS, compilation toutes cibles et format ciblé PASS, revue
indépendante traitée. Le terminal retenu et sa publication convergent sans
nouveau start ; [preuves et limites](VALIDATION.md#5-novembre--réconciliation-logicielle-après-annulation).
La fixture métier choisit 60 s ; la fiabilité du Writer produit à 10 s reste
ouverte. Ni reprise froide, D5/fencing, runtime réel ou capacité qualifiés.
G09 reste FAIL, 0/12 gates clos ; Clippy Linux BLOCKED.

Grants broker depuis le CLI,4novembre : préparation/rejeu durable, contrôle
de périmètre, CAS, révocation et normalisation des reçus ; 11 fonctions
ciblées uniques, format et compilation toutes cibles PASS, revue traitée.
[Preuves et limites](VALIDATION.md#4-novembre--grants-broker-durables-depuis-le-cli).
Cela complète un parcours opérateur du registre, sans qualifier la fédération
multi-hôtes, le transport worker, Kubernetes ou les pannes réelles.
G09 reste FAIL,0/12 gates clos ; Clippy Linux BLOCKED.

Diagnostic broker du 4 novembre : journal requis absent distingué d'une
entrée invalide, sans création en récupération ; sept fonctions ciblées et
compilation toutes cibles PASS après RED, revue indépendante et guides EN/FR.
[Preuves et limites](VALIDATION.md#4-novembre--journal-requis-absent-et-diagnostic-de-récupération).
Ce résultat logiciel ne qualifie ni restauration, ni réactivation, ni runtime.
G09 reste FAIL, 0/12 gates globaux clos ; Clippy Linux BLOCKED.

Coût publisher du4novembre : compteur d'ancêtres corrigé après RED, sept
parents ciblés uniques PASS sur les gels reliés et all-targets final PASS.
Le getter non autorisant évite un scan ; les contrôles avant effet restent
complets. À N32,262→198 sync fichiers et831→634 sync répertoires observés
sur tmpfs, sans qualification de latence physique, rétention soutenue ou
capacité100. [Preuves](VALIDATION.md#4-novembre--compteurs-de-durabilité-et-snapshot-du-publisher).
G09 FAIL,0/12 gates clos ; aucun résultat runtime nouveau.

Publisher du4novembre :21 fonctions uniques ciblées PASS sur les gels
reliés (19bibliothèque,2vraies CLI), puis compilation composée PASS.
Les défauts de reprise/capacité/historique trouvés
en revue ont leurs RED et corrections. Au gel initial, l'historique était relu/synchronisé
quatre fois par cycle ; durée soutenue, rétention et effet runtime ne sont
pas qualifiés. [Preuves](VALIDATION.md#4-novembre--présence-émise-par-le-service-broker).
G09 FAIL,0/12 gates globaux clos.

Complément Broker du4novembre : portée de la même preuve serveur, transaction
unique et refus de réponse contenant un secret intégrés en87e902d ; six
fonctions ciblées et all-targets PASS, revue traitée. Le profil Kubernetes
local de lecture est provisionné avec sept décisions RBAC opérateur vérifiées,
sans Pod/token ni sonde produit. Longue durée du publisher, fédération, runtime, Clippy
et qualifications restantes conservent leurs écarts ; aucun gate clos.

Complément CLI workload du4novembre : préparation/clé privées et reprise
exacte via les API existantes,17fonctions ciblées uniques PASS, all-targets
et release PASS, revue source traitée, guides/exemples EN/FR.
L'état courant reste inconnu après un reçu historique ; aucune activation
ou transmission de clé n'est implicite. Les Store/TLS et sujets Worker
sont vérifiés dans des fixtures logicielles, sans worker réel. Clippy Linux
est BLOCKED, fautes physiques/publication et transport/handoff/fédération
restent ouverts. [Preuves](VALIDATION.md#4-novembre--cli-durable-des-identités-techniques).
G09 FAIL,0/12 gates globaux clos ; aucune qualification de release.

Mise à jour de la branche de validation au4novembre : l'inventaire STOP du
broker et le préflight Kubernetes audité sont intégrés, avec48fonctions
uniques ciblées PASS sur leurs gels et compilation composée96bf8ed PASS.
Le préflight inclut un POST dry-run autorisé/audité, sans lancement de pod ;
il ne se limite plus au diagnostic GET. La qualification du cluster Desktop,
les dix écarts d'admission et la reprise de vrais workers restent ouverts.
[Preuves et périmètres](VALIDATION.md#4-novembre--préflight-kubernetes-audité).

Le CLI réel avec Keycloak local a atteint login/PKCE, identité/contexte et
lecture autorisée au gel F ; son logout restait non qualifié. Le gel G refuse
plus tôt un cookie dans le harnais ; son résultat ne remplace pas F. Une
compatibilité de cookie quoté est reproduite puis corrigée hors ligne et
le raccord d'accusé de déconnexion passe27tests ciblés, avec profils/guides
EN/FR relus, trois profils validés et compilation composée PASS. Le setup
stdin passe ; son chemin fichier échoue sans cause établie. Aucun nouveau
fait IdP n'est déduit de ces contrôles logiciels. Séparément, la recette H
passe sur le vrai Keycloak local : connexion, lectures, refresh/rotation,
logout et audits durables,13phases après17tests du harnais. Le périmètre
LinuxARM64 isolé ne qualifie pas Web composé/macOS/TTY/entreprise ou runtime.
[Preuve réelle et limites](VALIDATION.md#4-novembre--sso-cli-réel-renouvellement-et-déconnexion).
Les paragraphes suivants conservent leurs résultats historiques ; ils ne
redéfinissent pas ces portées. G09 FAIL,0/12 gates globaux clos.

Le 4 novembre, le [backend workload/registre/relais](VALIDATION.md#4-novembre--backend-workload-registre-et-relais-intégré)
est intégré après86tests ciblés, revues et compilation composée sans
avertissement. Migrations, autorisation, budgets et échanges HTTP/SQLite
ont leurs preuves logicielles ; au gel correspondant, le [diagnostic Kubernetes](../setup/kubernetes/README.fr.md)
restait en lecture seule. Aucun transport worker, fédération ou confinement
runtime n'est qualifié par ces résultats. G09 FAIL,0/12 gates globaux clos.
La commande `kubernetes doctor` est désormais raccordée :
[quatre tests CLI/TLS ciblés, all-targets et revue PASS](VALIDATION.md#4-novembre--commande-cli-de-diagnostic-kubernetes).
Ce résultat n'ajoute aucune admission ni qualification de cluster.

Mise à jour du4novembre : la composition humaine9ee05cc conserve les preuves
logicielles ciblées, sans build/navigateur/Clippy qualifiés. Le DNS hôte
`keycloak.ai-guard.test` est résolu depuis le2novembre et revalidé en lecture
seule ; les mentions `BLOCKED DNS hôte/admin` des gels ci-dessous restent
historiques. Le CLI natif macOS n'est toujours pas qualifié. Un essai CLI
Linux avec Keycloak temporaire était alors préparé, **NOT_RUN** ; ni l'ancien Web,
ni l'inventaire d'images ne le remplacent. [État et prérequis actuels](VALIDATION.md#4-novembre--composition-conservée-et-prérequis-sso-cli).

Préparateur SSO intégré par `586c17e` : configuration Rust hors ligne, reprise durable, profils locaux et guides EN/FR. [Validation ciblée](VALIDATION.md#2-novembre--préparateur-sso-privé-et-profils-de-lancement) et revues passent dans leurs portées documentées. Après les échecs Web historiques, [image 72bfb911 depuis 96fe9be et recettes Keycloak réelles](VALIDATION.md#2-novembre--sso-web-réel-sur-la-page-de-fin-corrigée) : build/rollout RC0 sans réinitialiser le volume, user-1 PASS 7/7/RC0, user-2 PASS 8/8 dans la première recette de rôles, puis admin PASS 13/13/RC0 sur la même image ; connexion, lecture autorisée et logout prouvés pour les trois comptes. Les trois échecs administratifs antérieurs restent FAIL ; seul le prédicat impossible de la troisième capture est démontré/corrigé dans le harness privé. DNS limité au navigateur dédié, CLI natif BLOCKED DNS hôte/admin ; refus adverses de permissions, refresh/révocation, délégation et qualification SSO globale ouverts. G09 FAIL, 0/12 gates clos. Les paragraphes suivants conservent les gels précédents.

Dernier essai G09 sur d6cb136 : [FAIL avant claim](VALIDATION.md#2-novembre--g09-release-courant-refus-avant-claim), worker Created, Arm5s expirée avant première vérification Ready. Aucun Running ni SIGKILL au point prévu ; effet d'arrêt autonome NOT_RUN, pas de fail-open démontré. Release courante et preuves conservées, instrumentation du chemin d'armement prioritaire. Aucun gate clos par cette nouvelle tentative.

État logiciel SSO consigné dans d6cb136, le2novembre : les24tests CLI SSO passent ensemble après corrections d'admission/rotation et de préparation des fixtures ; revues de ces correctifs traitées, fmt et Clippy strict finaux PASS. Le test transport exact passe après correction de sa fixture, sans rejeu de la suite entière ; les autres résultats restent attachés à leurs gels. Image worker générique Docker construite/inspectée, aucun worker lancé par ce build. Qualification réelle SSO/runtime encore ouverte ; aucun gate global clos. [Chronologie](VALIDATION.md#2-novembre--composition-sso-résultats-comportementaux-et-revues).

Autres preuves du2novembre : la [recette humaine réelle Docker API/CLI A/B](VALIDATION.md#2-novembre--recette-humaine-réelle-docker-apicli-ab) passe sur l'image7afdd43, avec migrations sauvegardées3→4→5, permissions/contenu/export, restart et révocation hors ligne. Navigateur NOT_RUN, aucun worker lancé. Les cibles humain14, audit24, gateway3+7 de la [composition OIDC candidate](VALIDATION.md#2-novembre--composition-sso-résultats-comportementaux-et-revues) restent PASS sur leurs gels. Les résultats intermédiaires transport15/16 et CLI19/21 sont historiques : leurs défauts connus sont corrigés et vérifiés dans la portée finale ci-dessus. Keycloak réel, bootstrap, délégation et fédération restent non qualifiés ; les travaux SSO/setup du worktree distinct ne sont pas intégrés à ce gel. Les mentions historiques « absent » ci-dessous portent sur leurs gels datés. G09 demeure FAIL,0/12 gates globaux clos.

Historique du1er novembre : [accès humain local, API/CLI et dashboard](VALIDATION.md#1er-novembre--accès-humain-local-apicli-et-dashboard) vérifiés dans les gels logiciels ciblés, migrations/audit/permissions et revues traitées. À ce gel, nouvelle image/parcours humain réel, OIDC/PKCE et délégation restent ouverts ; la recette humaine du2novembre ci-dessus apporte depuis une preuve supplémentaire, sans fermer G04 ou G08. Le trajet HTTPS du modèle local et sa panne/reprise santé passent, mais l'unique tentative d'évaluation reste FAIL_MODEL de cause fine inconnue. G09 reste FAIL et aucun gate global n'est clos.

Le [générateur PKI local](evidence/2026-11-01/demo-pki/README.md) a ses trois tests ciblés, TLS réel et checks statiques PASS. Il fournit des certificats de test ; G11 reste BLOCKED pour les identités/transports et le cycle PKI des déploiements. Aucun gate n'est clos par ce lot.

Complément du1er novembre : [mesures du journal et factorisation canonique](evidence/2026-11-01/journal-cost/README.md#résultats-finaux-du-lot), neuf fonctions uniques et fmt/Clippy strict/release PASS, revue traitée,160 sources et binaires vérifiés. Deux FAIL de caller outbox sont conservés ; seule l'attente du test fonctionnel512 change, aucun délai produit. Le probe avant/après sur31 révisions ne démontre aucun gain de latence, performance CPU release, capacité100 ou cause H10 ; FS exact NOT_RECORDED. Les contrôles et compteurs de durabilité restent inchangés. L'observation passive précédente d'un gardien ne donne ni coût global ni permission de disarm. G09 reste FAIL, D5 ouvert et0/12 gates clos ; aucun résultat ci-dessous n'est élargi.

### Compléments datés du 31 octobre et du 1er novembre

Complément H10/S3/S4/frontend du31octobre : H10 expire à600,04s avec FailureObserved locale avant Created/Arm/claim worker mais sans diagnostic public. Nettoyage de cache concomitant et inventaire37→3, cause exacte non établie ; aucun Running ni faute au point prévu. Le lot logiciel suivant publie un rapport fermé même sans provenance spécialisée, avec effet inconnu et limites de livraison explicites :8 tests Rust et checks stricts/release PASS. Frontend modèle12/12 et build statique PASS ; erreurs visibles dans Actions et résumé détaillé, sans qualification navigateur/visuelle ou Node24. Revues indépendantes traitées, aucune nouvelle preuve moteur du comportement. [État, RED et limites](evidence/2026-10-31/broker-runtime/README.md#h10--échec-local-durable-sans-diagnostic-public). Les analyses des slots et des sources Moby n'autorisent aucun disarm : D5, coût durable, reprise et qualification demeurent ouverts. G09 FAIL,0/12 gates clos.

Complément H8/H9/S1/S2 du31octobre : H8 expire pendant l'observation, H9 obtient réellement le premier `broker.batch.failure` durable (`slot_phase_mismatch` avant claim, sans sample local). Aucun Running ni faute au point prévu, aucune expiration native démontrée. S1 retire la seule relecture caller après Register via un Draft confirmé au head, sans permission ni suppression de barrières, et ajoute l'origine helper du snapshot d'erreur.20 fonctions ciblées PASS ; S2 corrige le seul lint d'assertion de test puis fmt/Clippy strict/release passent, sans replay de tests ou code applicatif modifié. Son effet runtime est NOT_RUN. Le budget global de test devient600s à la demande du mainteneur, Arm worker5s inchangée. [Preuves et suite](evidence/2026-10-31/broker-runtime/README.md). G09 reste FAIL,0/12 gates clos ; D5, reprise réelle, multi-workers et SSO restent ouverts.

Complément historique du31octobre, gels D3/PR4 avant H8/H9 : [verticale broker Docker H4–H7 et diagnostics](evidence/2026-10-31/broker-runtime/README.md). H4/H5 échouent après Arm/claim durables, H6 atteint une borne du harnais pendant la préparation, H7 enregistre un échec avant claim après Arm worker ACKée. Aucun Running ni faute au point prévu ; les causes H4/H5/H7 restent ouvertes. D3 a ses huit fonctions logicielles uniques ; le complément préclaim/adaptation lifecycle a16 fonctions ciblées, fmt/Clippy/release PASS et une régression partielle RED/GREEN corrigée en revue. À ces gels, son émission réelle et son effet temporel sont NOT_RUN. Ni correction de la deadline, ni disarm D5 qualifié ; G09 reste FAIL et0/12 gates sont clos. SSO/Keycloak/PKCE demeure absent et sa qualification NOT_RUN.

Complément logiciel du31octobre : [acteur STOP restreint et diagnostic](evidence/2026-10-31/stop-only-actor/README.md),33 fonctions ciblées et revue traitée, fmt/Clippy strict/release PASS. Premier Cancel sans blob, journal partiel, Unknown/replay, transfert continu de lease et rejets sont vérifiés dans leur périmètre logiciel ; vraie auth HTTP/A/B et refus vide sur connexion Unix inclus. Aucun worker ni échéance moteur n'est qualifié par ces preuves : G09 conserve son FAIL et0/12 gates sont fermés. Les limites de pile du harnais et de connexion restent explicites dans le dossier.

Trois approches Jev exigées : API réelle, règles Rust explicitement incomplètes, petit modèle local configurable Docker/Podman ≤4B et cible 500 Mo–1 Go. Le premier essai local 0,5B respecte 1 Go sur Docker Desktop (pic 777 912 320 octets) mais échoue sémantiquement sur une suppression ; candidat refusé pour autoriser des actions. Cela ne clôture ni G04 ni G07/G10. Sélection commune et adaptateur local restent à livrer ; refus impératifs et confinement indépendants. [Preuves et limites](evidence/2026-10-30/local-judge/README.md).

Le [smoke de réutilisation du1er novembre](evidence/2026-11-01/local-inference/README.md) confirme un seul appel Chat Completions et un pic788877312 octets sans OOM, après correction du probe fragmenté et refus tronqué/silence. Le serveur est arrêté ; le jugement de sécurité reste FAIL. Aucun adaptateur produit, client Codex/Claude, transport TLS ou déploiement entreprise n'est qualifié par ce résultat ; aucun gate clos.

La déclinaison [technique OIDC/PKCE](SSO_OIDC.md) exige aussi les preuves de découverte/JWKS et de transport IdP bornés (G05/G11), de renouvellement/expiration/logout/révocation et fin SSE (G08), de migrations et de préservation des identités historiques (G06). Le gateway propriétaire de la transaction Web est approuvé le1er novembre ; au2novembre, son implémentation candidate possède les preuves logicielles ciblées ci-dessus. La qualification IdP réel/navigateur et les scénarios de déploiement restent NOT_RUN ; les écarts de qualification restants ne sont pas transformés en succès par cette mise à jour.
