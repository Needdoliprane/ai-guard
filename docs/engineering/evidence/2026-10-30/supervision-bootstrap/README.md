# Bootstrap frais du contrôle — 30 octobre 2026

Base `f46a23ecb8d17f098d325c017536e7c8a146ed83`, branche `codex/broker-service`. [Implémentation](implementation.md), [tests Luna](tests.md), [revue indépendante](review.md). Trois sources applicatives : `audit.rs`, `main.rs`, `gateway_config.rs`. Aucun nouveau composant d'infrastructure ou dépendance.

## Comportement livré

`Store::create_supervised_for_tenant` utilise la création exclusive existante avant toute ouverture SQLite. Il initialise le schéma2, conserve une sauvegarde2 vérifiée/exclusive, puis migre3 et ne retourne qu'avec une autorité fraîche active dans cette même origine. Une cible préexistante/restaurée est refusée ; reopen3 reste en maintenance. Le constructeur historique garde son défaut2.

Le gateway expose `--initialize-supervised-stores BACKUP_ROOT`, en local ou avec registre. Le nom `tenant-<SHA256 du tenant exact>-v2.sqlite` distingue les backups ; il n'authentifie personne. La racine privée est séparée des répertoires de credentials configurés runner/viewer/Jev par comparaison des identités de répertoires et de leurs ancêtres. Aucune valeur de clé n'est rouverte pour cette comparaison, aucun mode existant réparé. Références privées supplémentaires dans ConfiguredTenant, JSON du registre inchangé.

Le bootstrap multi-tenant n'est pas globalement atomique. Une erreur empêche serve ; nouveaux fichiers/répertoires partiels restent conservés et les fichiers existants sont préservés. L'option ne réactive pas une base après restart et ne fournit pas de broker permanent.

## Vérifications constatées

| Commande/cible | Résultat |
|---|---|
| `cargo test --locked --test supervision_bootstrap` | **PASS**, [7 fonctions](bootstrap-final/cargo.stdout), 1 helper enfant ignoré par défaut et invoqué explicitement ; [RC0](bootstrap-final/exit-code). |
| `storage_security`, `audit_integrity`, `gateway_tenancy` | **PASS**, 17+12+1 fonctions dans le [lot canari](canaries/targets.stdout). |
| `supervision_store` | Premier FAIL de fixture, reproduit sur baselinef46 sous umask022 ; destination clone désormais créée0600 avant SQLite. [Cible corrigée](clone-fixed/cargo.stdout) **9PASS**, sans relâcher l'application. Le rejeu exact1 est inclus dans ces9, pas additionné. |
| Formatage | **PASS**, [RC0](fmt-final/exit-code). |
| Clippy strict all-targets | [Premier diagnostic](clippy-initial/stderr) `manual_map` dans l'oracle Darwin, corrigé mécaniquement ; [dernier passage](clippy-final/stderr) **PASS**. |
| Release | **PASS**, [RC0](release-final/exit-code). Première construction36s, dernier passage cache0,18s après ajout du probe au test ; même binaire. |
| Contrôle conteneurisé | **PASS ciblé Docker Desktop/LinuxARM64**, [rapport](docker.md) : témoin refusé avant DB, image avec parent secrets préparé acceptée, HTTP authentifié A/B et backups privés. Aucun worker. |

Environnement macOS27.2/ARM64, Rust/Cargo1.99.1 ; cible Cargo partagée avec l'ancien worktree uniquement pour son cache, artefacts antérieurs préservés hors target. Le [manifeste112](source.sha256) est conforme aux fichiers courants et identique [avant](release-final/sources-before.sha256)/[après](release-final/sources-after.sha256) build : SHA `10d581fd0e445e9d3647e17b59b098245ea401d1d1b32619eb7f9cbbf4e44a68`. Les manifestes sont triés par ligne SHA ; l'ordre différent de git ls-files ne change aucun des112 enregistrements.

Couverture : Store neuf actif3 et backup2 intègre ; refus tenant invalide, cible/backup existants et capacité d'audit insuffisante après vraie copie ; partiels préservés. Parents DB/backup imbriqués initialement absents ; backup au même parent pour vérifier l'absence de deadlock du garde. Vrai gateway HTTP A/B, rôles/tenants et namespaces distincts, réponses protocole actives avant arrêt ; restart sans option garde les namespaces en maintenance ; retry du flag refuse les bases existantes ; mode ordinaire local reste2. Refus des racines backup identiques/ancêtres/descendantes des dirs credentials ; alias Darwin seulement si dev/ino identiques effectivement présents.

## Revue et limites de preuve

Revue finale indépendante **CLOSE**, sans constat bloquant résiduel dans ce lot ; [verdict et recoupements](review.md). Le reviewer n'a exécuté aucun nouveau test. La clôture couvre le bootstrap et ses preuves, pas la supervision autonome.

BSB-01 : barrière des ancêtres absente dans le premier gel, corrigée dans la factory neuve par `Directory::durable_bootstrap` sur les parents cible/backup avant retour utilisable. Garde source conservé pendant init2, relâché avant le garde du backup. Fermeture **par inspection source et tests de chemins imbriqués**, sans simulation de coupure électrique.

BSB-02 : le premier oracle ne sondait les locks qu'avant refus. Le gel final place F_GETLK/PRAGMA dans un enfant immédiatement après le refus de la factory dans le PID propriétaire, avant tout nouvel appel SQLite parent. BSB-03 : les gateways de test sont possédés par un guard RAII kill/wait borné sur échec. Les anciens PASS sans ces assertions ne sont pas attribués à ces preuves.

Les RED [library](library-red/target.stderr) et [CLI](cli-red/cargo.stderr) établissent des fonctionnalités absentes, pas des vulnérabilités. Le [FAIL du clone sur baseline](clone-baseline/cargo.stderr) vient d'une fixture dépendant de l'umask ; la préparation0600 corrige le test, pas la politique de permissions. Les tentatives de harnais et tous leurs chemins sont répertoriés dans le rapport ; les archives retenues sont copiées sans réécriture. [Manifeste des preuves](SHA256SUMS) distinct, whitespace vérifié sur code/suivi/ce README.

Artefacts privés dans `/tmp/ai-guard-supervision-bootstrap-qualified.zZ7woB`, fichiers600 : `agentsandbox-macos` `3188c3a6848925a2995d7096fe2f5433d4a609e1eb7daa2c009e70dea5a19b39`, `libagentsandbox-macos.rlib` `10d191a1c4e457b57bc358ae9698241ec69b0701036c5da46ee60f87e09376d4`, source tar `79407466e2f2417389e7d32faad67207349959024f82080d61c07990a8339275`.

## Complément Docker du même code

Le parent `/run/secrets` absent de l'ancienne image est créé par Docker en root0755 lors des montages de fichiers ; le nouveau bootstrap le refuse. `images/control.Dockerfile` prépare désormais ce répertoire en1000:1000/0700 avec `/data`, sans changer les projections0444 readonly. SHA du Dockerfile `11b5d0ac5c444b702f77782454a58607a2a0724ec79d3645018c84a6626b74b3`. Deux images comparatives utilisent le même binaire Linux release et le même probe Rust ; la préparation du parent est leur différence fonctionnelle. Les [Dockerfiles](docker/Dockerfile.prepared) et [configurations inspectées](docker/runtime-inspect.json) sont archivés.

Le [probe](docker/probe.stdout) lit les clés par chemin et vérifie les réponses du vrai gateway A/B, puis les métadonnées de deux backups. Il ne relit ni leur schéma SQLite ni leur intégrité et ne recalcule pas la correspondance nom/tenant ; le format2 est établi par le code et la vérification native distincte. Les [modes observés](docker/corrected-metadata.stdout) et les [hashes des binaires recopiés des conteneurs arrêtés](docker/runtime-binaries.sha256) recoupent l'inspection. Rootfs readonly, réseau none, caps ALL retirées, NNP, UID1000,2CPU/512MiB/256PID ; DATA est seul writable, registre/4secrets sont readonly et aucun port publié. Consommation/latence sous charge non mesurées. Pas d'image de QA remplacée.

Image corrigée avec probe `sha256:63139bcbb9afdc13f3401d5c9472fe91f45b539853e3eceed8589ee91df87698`, conteneur arrêté0 `6481d50b8e05cfddf8379f373c8791d231f50c9876bd10cecc9239e39a4509cb`. Même binaire Linux dans témoin/corrigé : `764b91d0297669c0c07db094dd46fb4874f5145bb8966f330a62138b9cc94e9e`, probe `9db646573aa00457b4f1fb7b6c775969d3a3057ae73d1ec1b0587b9daed8c80e`. Source112 recopiée depuis le builder conforme au manifeste natif. Artefacts Linux sous `linux-artifacts/` et copies runtime sous `runtime-binaries/` du dossier privé ; aucune confusion avec les Mach-O macOS.

Le premier builder avec --rm a perdu les artefacts dans son volume anonyme ; un second builder possédé a conservé le résultat. Le snapshot Docker n'incluant pas les volumes, le probe a requis la copie des dépendances et le même profil ThinLTO que Cargo. Ces incidents du harnais sont décrits, pas présentés comme défauts produit ou preuves conservées. Les images sont des overlays du runtime local vérifié ; le Dockerfile multi-stage complet n'a pas été reconstruit et sa chaîne de livraison reste à qualifier.

**NOT_RUN** : coupure électrique, hôte Linux natif/x86_64, Podman, worker/confinement, modèle, navigateur/E2E général, charge100, cloud/Kubernetes, service/IPC/ordonnanceur/gardien et fencing/reprise. La séparation HTTP A/B de ce contrôle ne qualifie pas les volumes ou workers A1/A2/B1. `active=true` décrit l'autorité du Store, pas la présence d'un enforcer. QA, défaut2 et facturation inchangés ; G09 demeure FAIL et aucun gate global fermé.
