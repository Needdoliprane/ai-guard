# Risques restant à valider

Les 19 constats ci-dessous ont passé une validation source indépendante puis une relecture finale. Aucun n'a été reproduit dans le sandbox OS complet exigé par la méthode.

Pour chacun, le verdict formel est `needs_validation` et la sévérité formelle reste non attribuée. L'impact est potentiel ; la confiance indiquée porte sur le chemin source, pas sur l'exploitation runtime.

## API, lectures et disponibilité

### `api/preauth-linear-tenant-credential-hash-amplification`

**Un Bearer invalide est rehaché pour chaque credential tenant avant tout quota**

- **Sources :** `src/api.rs:64`, `src/api.rs:195-213`, `src/api.rs:274`, `src/api.rs:300-315`, `src/gateway_config.rs:48`, `src/main.rs:104,197`.
- **Préconditions et chemin :** un client non authentifié atteint un gateway partagé. Axum désérialise d'abord le JSON, puis `authorize` balaie les deux credentials de chaque tenant. `token_matches` recalcule le SHA-256 du Bearer entier à chaque comparaison. Le quota tenant n'intervient qu'après authentification.
- **Impact potentiel :** consommation CPU pré-authentification et retard des décisions ou audits autorisés dans un gateway fortement multi-tenant.
- **Évaluation :** sévérité formelle non attribuée ; confiance statique élevée. L'impact dépend du nombre de tenants, de la longueur maximale d'en-tête, de l'exposition réseau et des threads disponibles.
- **Reproduction minimale :** avec 8, 16 puis 32 tenants factices, compter les octets hachés pour un unique Bearer invalide de 64 puis 4096 octets ; mesurer ensuite la progression d'une seule décision autorisée, sans charge répétée.
- **Correction :** borner le Bearer avant hachage, calculer son digest une seule fois, indexer les digests configurés et authentifier via un extracteur `FromRequestParts` exécuté avant `Json`.
- **Régression :** vérifier un nombre constant de hachages, le refus précoce des Bearer hors borne et l'absence de désérialisation métier avant `401`.

### `api/sse-graceful-shutdown-without-drain-deadline`

**Un flux SSE viewer peut prolonger sans deadline produit le drainage du gateway ou du dashboard**

- **Sources :** `src/api.rs:104`, `src/api.rs:426-464`, `src/api.rs:545`, `src/api.rs:600`, `src/main.rs:231-250,283`.
- **Préconditions et chemin :** un viewer autorisé conserve un SSE en consommant les heartbeats. La boucle ne reçoit aucun signal d'annulation ; le dashboard relaie le body sans timeout total. Gateway et dashboard utilisent `with_graceful_shutdown` sans deadline globale.
- **Impact potentiel :** arrêt ou remplacement retardé, puis éventuel arrêt forcé par le superviseur.
- **Évaluation :** sévérité formelle non attribuée ; confiance statique élevée sur l'absence de deadline, moyenne sur le blocage effectif.
- **Reproduction minimale :** ouvrir un SSE factice, déclencher séparément l'arrêt du gateway puis du dashboard et vérifier avec un watchdog si le processus termine avant une petite deadline ; fermer ensuite le client.
- **Correction :** propager un token d'arrêt aux générateurs SSE et au proxy, puis entourer le drainage d'une deadline explicite avec fermeture forcée bornée.
- **Régression :** maintenir un SSE actif, envoyer le signal d'arrêt et exiger la terminaison dans le budget, avec libération du permis et reprise correcte du curseur.

### `api/viewer-synchronous-reads-control-store-contention`

**Des lectures viewer synchrones peuvent retarder les décisions et écritures du contrôle**

- **Sources :** `src/api.rs:391-418`, `src/audit.rs:48`, `src/audit.rs:115-138`, `src/audit.rs:360,511`, `src/audit.rs:606-701`, `src/audit.rs:744-872`.
- **Préconditions et chemin :** un viewer consulte `overview`, `integrity`, les projections ou une recherche sur un historique déjà important. Ces opérations synchrones retiennent la connexion SQLite mutexée également utilisée par `append` et `decision`. `overview` comporte un `NOT EXISTS` corrélé sur un `request_id` JSON non indexé.
- **Impact potentiel :** retard d'une décision ou d'une trace obligatoire pour le même tenant ; effet inter-tenant possible seulement via CPU et threads partagés.
- **Évaluation :** sévérité formelle non attribuée ; confiance statique élevée, impact runtime non mesuré.
- **Reproduction minimale :** comparer 32, 64 et 128 reviews distinctes, instrumenter les étapes SQLite, puis exécuter une lecture et une décision concurrentes du même tenant ainsi qu'une lecture d'un second tenant.
- **Correction :** utiliser un pool bloquant dédié, une admission de lectures par tenant, des timeouts/interruption SQLite et un index ou état matérialisé pour les décisions pending.
- **Régression :** fixer un budget de progression pour une décision concurrente et contrôler le plan de requête ainsi que l'abandon d'un client.

### `ingress:forward:shared-preauth-exchange-pool`

**Le pool global de l'ingress est réservé avant lecture du corps et authentification**

- **Sources :** `src/ingress.rs:23`, `src/ingress.rs:74-81`, `src/ingress.rs:241`, `src/ingress.rs:288-305`, `src/ingress.rs:352`.
- **Préconditions et chemin :** un client atteint l'ingress, envoie des headers valides puis maintient le corps incomplet. Un des 64 permis globaux est pris avant le corps et avant l'authentification upstream ; une réponse longue conserve également son permis.
- **Impact potentiel :** réponses `429` pour des runners ou viewers légitimes, potentiellement d'autres tenants dans un ingress partagé.
- **Évaluation :** sévérité formelle non attribuée ; confiance statique élevée, portée dépendante du bind et de la topologie.
- **Reproduction minimale :** avec un sémaphore de taille 1 et des timeouts inférieurs à une seconde, maintenir un corps incomplet puis envoyer une requête autorisée ; vérifier le `429`, puis la libération après timeout.
- **Correction :** authentifier ou admettre avant la ressource globale lorsque possible, séparer les pools streaming/courts et réserver une capacité aux décisions et arrêts, avec budgets par identité.
- **Régression :** prouver qu'un corps incomplet ne consomme pas la capacité réservée au contrôle et que tous les permis sont libérés après timeout, erreur et abandon.

## SQLite, fichiers et frontières locales

### `audit/preexisting-database-permissions-not-enforced`

**Une base SQLite préexistante permissive peut être acceptée comme audit tenant**

- **Sources :** `src/audit.rs:68-108`, `src/audit.rs:275`, `src/audit.rs:456`.
- **Préconditions et chemin :** un utilisateur local non administrateur du stockage peut précréer la base dans un parent partagé. Le mode `0600` d'`OpenOptions` ne resserre pas un fichier existant et `verify` ne contrôle ni UID, mode, ACL ni parents.
- **Impact potentiel :** lecture ou modification filesystem de l'audit d'un tenant par un principal local non autorisé.
- **Évaluation :** sévérité formelle non attribuée ; confiance statique élevée. Le volume privé du Compose standard peut fermer la précondition.
- **Reproduction minimale :** avec deux UID factices, faire précréer une base permissive, ouvrir un `Store`, écrire un événement non sensible et vérifier si le créateur peut le relire.
- **Correction :** refuser une base préexistante qui n'appartient pas à l'euid attendu, n'a pas exactement les permissions privées ou présente des ACL inattendues ; vérifier aussi parents, WAL et SHM.
- **Régression :** test multi-UID refusant un fichier préexistant permissif et acceptant seulement une base privée sous un parent de confiance.

### `audit/sqlite-connection-path-reopen-identity-race`

**La connexion SQLite n'est pas liée au descripteur de base validé**

- **Sources :** `src/audit.rs:58-108`, `src/audit.rs:248-333`, `src/audit.rs:374`.
- **Préconditions et chemin :** dans un parent renommable, un utilisateur local présente A au premier open, B au second open de rusqlite, puis restaure A avant la vérification suivante. Les contrôles continuent de vérifier A, pas l'inode détenu par SQLite.
- **Impact potentiel :** lectures ou écritures d'audit sur une base contrôlée par l'attaquant, sous réserve que B satisfasse schéma, binding tenant et intégrité.
- **Évaluation :** sévérité formelle non attribuée ; confiance statique moyenne, car le comportement VFS/WAL/SHM reste décisif.
- **Reproduction minimale :** ajouter un hook déterministe entre les deux opens, effectuer A→B→A avec deux UID et vérifier si `Store::head` ou un append atteint B.
- **Correction :** lier SQLite au descripteur validé ou comparer, via une primitive VFS/file-control sûre, l'identité du fichier principal détenu par SQLite ; à défaut, exiger un parent privé non renommable.
- **Régression :** test à hook de substitution aller-retour, plus vérification des inodes du fichier principal, WAL et SHM.

### `dashboard/static-root-symlink-follow-disclosure`

**Le serveur statique public peut suivre un lien symbolique hors de sa racine**

- **Sources :** `src/api.rs:551-559`, `src/main.rs:87,237`, `Cargo.lock:1611`.
- **Préconditions et chemin :** en mode hôte ou image dérivée, un principal moins fiable écrit sous `front_dir` et y crée un lien vers un fichier lisible par le dashboard. Le fallback public `ServeDir` bloque les traversées lexicales mais suit ce lien lors de l'ouverture.
- **Impact potentiel :** divulgation HTTP sans bearer d'un fichier accessible au processus dashboard.
- **Évaluation :** sévérité formelle non attribuée ; confiance statique élevée sur le chemin, précondition fortement réduite par l'image standard en lecture seule.
- **Reproduction minimale :** servir une racine factice contenant `probe.txt` lié vers un fichier factice extérieur, puis effectuer un unique `GET /probe.txt` sans bearer.
- **Correction :** embarquer les assets ou résoudre chaque composant sous un descripteur racine avec une primitive beneath/no-follow ; refuser tout symlink.
- **Régression :** exiger `403/404` pour un lien extérieur, tout en conservant les assets réguliers et le refus de `../`, `%2e%2e` et chemins absolus.

### `operations/config-path-reopen-secret-substitution`

**Le répertoire de configuration est rouvert par pathname après sa validation**

- **Sources :** `src/operations.rs:165-188`, `src/operations.rs:266`, `src/operations.rs:303-357`, `src/operations.rs:549-570`, `deploy/docker/compose.yaml:88`, `src/main.rs:159`, `src/api.rs:193`.
- **Préconditions et chemin :** un utilisateur local peut renommer `config_dir` via son parent. Après validation par descripteur, `Up` attend `doctor`, puis `compose` recanonicalise le pathname. Un nouvel arbre contenant des Bearer choisis peut être monté.
- **Impact potentiel :** obtention des rôles viewer ou runner, lecture de l'audit ou mutation/décision non autorisée.
- **Évaluation :** sévérité formelle non attribuée ; confiance statique élevée, exploitation conditionnée par un parent renommable et le handoff moteur effectif.
- **Reproduction minimale :** suspendre après `initialize_config`, remplacer l'entrée depuis un second UID, puis observer avec une CLI Compose factice les fichiers ouverts.
- **Correction :** conserver l'identité du répertoire validé jusqu'au handoff, comparer dev/inode avant lancement et imposer des parents privés non renommables ; éviter la recanonicalisation non liée.
- **Régression :** hook déterministe de substitution entre init et compose ; la commande doit refuser avant toute recréation de service.

### `operations/extended-acl-secret-projection-disclosure`

**Des ACL étendues peuvent contredire les seuls bits de mode contrôlés pour les projections de secrets**

- **Sources :** `src/operations.rs:266-278`, `src/operations.rs:347`, `src/operations.rs:405`, `src/operations.rs:485-496`, `src/api.rs:193`.
- **Préconditions et chemin :** une ACL effective accorde traverse/read à un autre UID tout en laissant les bits de mode acceptés. Les projections sont `0444` sous un parent jugé privé seulement par UID/mode.
- **Impact potentiel :** lecture locale d'un Bearer runner ou viewer, puis utilisation de ses droits si le gateway est joignable.
- **Évaluation :** sévérité formelle non attribuée ; confiance statique moyenne en raison des différences POSIX, NFSv4 et Darwin.
- **Reproduction minimale :** avec deux UID, créer une racine `0700` avec ACL nommée, exécuter l'initialisation et tenter de lire uniquement `mounted-secrets/viewer`.
- **Correction :** inspecter les ACL effectives par plateforme, refuser toute entrée inattendue ou les neutraliser avant publication ; vérifier également les ACL héritées des fichiers.
- **Régression :** matrice Linux/macOS/filesystems validant ACL absente, ACL restrictive et ACL donnant effectivement traverse/read.

### `operations/jev-control-credential-reuse`

**Une clé Jev réutilisant un credential de contrôle est envoyée au service externe**

- **Sources :** `src/operations.rs:311-324`, `src/gateway_config.rs:90`, `src/main.rs:163-174`, `src/jev.rs:109`.
- **Préconditions et chemin :** l'opérateur configure par erreur Jev avec la même valeur que runner ou viewer. L'initialisation et le démarrage ne détectent pas la collision, puis `JevClient` envoie cette valeur comme Bearer.
- **Impact potentiel :** divulgation d'un credential de contrôle au service Jev ; réutilisation possible si celui-ci peut joindre le gateway.
- **Évaluation :** sévérité formelle non attribuée ; confiance statique élevée, précondition dépendante d'une mauvaise configuration opérateur.
- **Reproduction minimale :** utiliser des valeurs factices avec Jev=runner, initialiser, puis envoyer une décision à une fixture loopback et observer un unique header factice.
- **Correction :** imposer l'unicité Jev/runner/viewer avant publication et à chaque démarrage, y compris multi-tenant et rotation.
- **Régression :** Jev=runner et Jev=viewer doivent être refusés avant création de projection et avant toute requête réseau.

### `runner/credential-env-path-reopen-substitution`

**Le fichier de credentials est rouvert par pathname après ses contrôles**

- **Sources :** `src/runner.rs:321`, `src/runner.rs:401-450`, `src/runtime.rs:58-69`, `src/runtime.rs:135-141`.
- **Préconditions et chemin :** un utilisateur local peut renommer la source entre `symlink_metadata` et `read_to_string`, ou remplacer le nom du tempfile avant l'ouverture `--env-file`. Le contenu substitué est injecté dans le worker autorisé.
- **Impact potentiel :** divulgation au worker d'un autre secret lisible par le broker.
- **Évaluation :** sévérité formelle non attribuée ; confiance statique moyenne. Des parents privés et un TMPDIR correctement protégé ferment les courses.
- **Reproduction minimale :** deux fichiers `NAME=value` factices, deux UID et un hook entre contrôle et lecture ; tester séparément la source et le handoff à une fausse CLI.
- **Correction :** ouvrir la source avec `O_NOFOLLOW`, vérifier et lire depuis le même descripteur ; créer la copie dans un répertoire de session privé ancré et revérifier son identité immédiatement avant l'effet moteur.
- **Régression :** substitutions déterministes de la source et du tempfile, fichier substitué surdimensionné et changement entre review et replay doivent tous être refusés.

## Secrets et audit

### `redaction/json-object-keys-preserved`

**Les clés d'objet JSON contournent le masquage des secrets**

- **Sources :** `src/runner.rs:714`, `src/adapters.rs:106-116,431-449`, `src/redaction.rs:84-100`, `src/audit.rs:349,456`, `src/api.rs:378,486`.
- **Préconditions et chemin :** un worker connaissant un credential injecté l'utilise comme clé d'un objet de télémétrie générique. `bounded_value` et les deux passages de `Redactor` recopient les clés sans les masquer.
- **Impact potentiel :** persistance du credential dans SQLite puis lecture/export par les viewers du même tenant.
- **Évaluation :** sévérité formelle non attribuée ; confiance statique élevée. Aucune fuite inter-tenant n'est revendiquée.
- **Reproduction minimale :** événement factice dont `data` porte le secret comme clé, double redaction, insertion dans une base jetable puis lecture et export.
- **Correction :** masquer ou rejeter récursivement les clés contenant une valeur connue, avec une politique collision-safe et stable.
- **Régression :** aucune occurrence du token factice dans les clés ou valeurs sérialisées après adaptation, audit, listing et export ; tester deux clés convergeant après masquage.

### `redaction/truncation-before-secret-masking`

**La troncature précédant le masquage peut conserver un préfixe de secret dans l'audit**

- **Sources :** `src/runtime.rs:455`, `src/runner.rs:693-704`, `src/redaction.rs:49,70-82`, `src/adapters.rs:64-74,263`, `src/audit.rs:350`, `src/api.rs:377`.
- **Préconditions et chemin :** un secret opaque chevauche la limite de 8192 ou 4096 octets. La troncature supprime sa fin avant le remplacement exact ; son préfixe reste dans l'événement.
- **Impact potentiel :** divulgation intra-tenant d'un fragment substantiel, sans preuve de reconstruction ou réutilisation du secret complet.
- **Évaluation :** sévérité formelle non attribuée ; confiance statique élevée.
- **Reproduction minimale :** 8161 octets neutres suivis d'un secret factice de 32 octets ; appliquer `clip(...,8192)` puis redaction et vérifier le préfixe de 31 octets.
- **Correction :** masquer avant de tronquer, ou utiliser un masque streaming conservant un chevauchement d'au moins la longueur maximale des secrets.
- **Régression :** tester toutes les positions autour des bornes et exiger qu'aucun préfixe significatif du secret n'apparaisse.

### `runner/decision-omits-known-worker-secret-redaction`

**La demande de décision contourne le dictionnaire des secrets worker connu du runner**

- **Sources :** `src/runner.rs:136,156-163,465-476`, `src/api.rs:335`, `src/jev.rs:104`, `src/audit.rs:531`.
- **Préconditions et chemin :** une commande fournie ou approuvée par le contrôle contient comme argument positionnel un secret worker chargé. `post_event` le masque, mais `decide` sérialise la requête brute ; le gateway ne connaît pas ce secret.
- **Impact potentiel :** persistance du secret dans `security.decision`, lecture/export viewer et, avec Jev, transmission externe.
- **Évaluation :** sévérité formelle non attribuée ; confiance statique élevée. Le worker ne modifie pas lui-même la commande.
- **Reproduction minimale :** secret opaque factice dans un argument positionnel, décision locale autorisée, puis comparaison de `command.requested`, `security.decision` et d'un body Jev loopback.
- **Correction :** refuser toute occurrence d'un secret worker connu dans la commande avant `/decide`, ou envoyer une représentation expurgée tout en liant la décision au hash original.
- **Régression :** le secret factice doit être absent de tous les événements, du stockage, de l'export et du body Jev ; une divergence de hash invalide l'autorisation.

## Moteur, arrêt et capacité

### `runtime/abrupt-broker-stop-leaves-worker-unfenced`

**Un arrêt brutal du broker peut laisser le worker actif sans borne ni audit terminal**

- **Sources :** `src/runtime.rs:54,306,461-488`, `src/operations.rs:228`, `src/main.rs:136,348`, `src/runner.rs:545,653,683`.
- **Préconditions et chemin :** le broker reçoit SIGTERM ou disparaît pendant un worker actif. `Operator::Run` ne branche pas le handler SIGTERM, la deadline et le cleanup vivent en mémoire et l'UUID runtime n'est pas durable. L'effet dépend de la survie du conteneur après disparition de `start --attach`.
- **Impact potentiel :** exécution au-delà de l'arrêt demandé, sans borne restante, observation terminale ni réconciliation.
- **Évaluation :** sévérité formelle non attribuée ; confiance statique moyenne, comportement moteur et superviseur décisif.
- **Reproduction minimale :** moteur à data-root jetable, worker factice sans secret, SIGTERM envoyé seulement au broker après `command.started`, puis inspection immédiate et cleanup ciblé.
- **Correction :** persister une identité runtime stable et un état `submitted/running/terminal/cleanup_pending/unknown`, gérer SIGTERM avec un token commun, appliquer une lease côté moteur et réconcilier au démarrage.
- **Régression :** interrompre le broker à chaque frontière et vérifier qu'aucune commande n'est relancée, que le worker est arrêté ou marqué `unknown`, et que l'audit terminal/cleanup est repris idempotemment.

### `runtime/doctor-effect-engine-identity-unbound`

**L'attestation doctor n'est pas liée à l'identité du moteur qui exécute ensuite les effets**

- **Sources :** `src/runtime.rs:278,302-318,351-383,449`, `src/runner.rs:27,582`, `src/operations.rs:123,173`.
- **Préconditions et chemin :** un principal local peut modifier un contexte ou socket moteur entre `doctor` et `create`. L'endpoint A déclare rootless ; les effets sont envoyés à B.
- **Impact potentiel :** projet ou env-file remis à un moteur non attesté, avec confinement différent.
- **Évaluation :** sévérité formelle non attribuée ; confiance statique moyenne. Le choix volontaire de l'opérateur reste dans le périmètre de confiance.
- **Reproduction minimale :** deux fixtures A/B et une configuration modifiable par un second UID ; basculer après `doctor` et vérifier quel endpoint reçoit `volume create`.
- **Correction :** résoudre et figer l'exécutable, le contexte et l'endpoint, attester une identité stable du daemon, l'inclure dans le plan durable et la revalider avant le premier effet.
- **Régression :** toute modification A→B entre `doctor` et effet doit bloquer le lancement et exiger une nouvelle décision.

### `runtime/path-resolves-project-executable`

**Le CLI moteur est résolu par le PATH ambiant depuis le répertoire courant**

- **Sources :** `src/runtime.rs:184`, `src/runtime.rs:303-318,351`, `src/runner.rs:552`, `src/operations.rs:121`.
- **Préconditions et chemin :** ai-guard est lancé depuis un projet moins fiable avec `PATH` contenant `.`, un segment vide ou relatif prioritaire. Un exécutable `docker` ou `podman` du projet est lancé sur l'hôte dès `doctor`.
- **Impact potentiel :** exécution de code arbitraire avec l'identité OS du broker.
- **Évaluation :** sévérité formelle non attribuée ; confiance statique élevée, mais la configuration PATH dangereuse doit être établie.
- **Reproduction minimale :** faux exécutable natif sous un projet factice, `PATH=.:<trusted>`, un seul appel `doctor`, puis contrôle négatif avec PATH absolu privé.
- **Correction :** exiger un chemin moteur absolu approuvé, vérifier son identité/propriétaire, utiliser `env_clear` avec allowlist et un cwd de confiance.
- **Régression :** un faux `docker` dans le projet ne doit jamais être lancé ; refuser les PATH vides, relatifs ou modifiables par le projet.

### `runtime/worker-output-engine-log-retention-unbounded`

**Les sorties worker peuvent échapper aux bornes du broker dans les journaux du moteur**

- **Sources :** `src/runner.rs:654`, `src/runtime.rs:86-145`, `src/runtime.rs:211`, `src/runtime.rs:444-455,488`, `deploy/docker/compose.yaml:24`.
- **Préconditions et chemin :** le moteur persiste stdout/stderr avec son driver par défaut sur un stockage partagé non borné. `capture_stream` limite seulement la copie mémoire et continue de drainer tous les octets.
- **Impact potentiel :** épuisement du stockage moteur et indisponibilité du contrôle ou d'autres tenants.
- **Évaluation :** sévérité formelle non attribuée ; confiance statique élevée sur l'absence de `LogConfig`, impact dépendant du driver et des quotas externes.
- **Reproduction minimale :** moteur jetable, petite chaîne fixe, inspection de la configuration et de la taille de logs, sans remplir le disque.
- **Correction :** imposer une politique de logs explicite non persistante ou strictement bornée pour chaque conteneur dynamique et refuser le lancement si elle ne peut pas être appliquée.
- **Régression :** inspecter le `LogConfig` réel des workers Docker et Podman et vérifier rotation/quota sur une sortie minimale.

### `runtime/workspace-volume-no-byte-quota`

**Le volume de travail du worker n'a aucune borne produit en octets ou inodes**

- **Sources :** `src/runner.rs:653`, `src/runtime.rs:124`, `src/runtime.rs:389-400`, `src/runtime.rs:444,449`, `src/workspace.rs:18`.
- **Préconditions et chemin :** le driver place les volumes sur un stockage partagé sans quota externe. Le volume est monté RW, sans taille ni quota d'inodes, et conservé après la session.
- **Impact potentiel :** épuisement du data-root, avec impact potentiel sur le contrôle et d'autres tenants.
- **Évaluation :** sévérité formelle non attribuée ; confiance statique élevée sur l'absence de borne produit, impact dépendant du backend.
- **Reproduction minimale :** data-root jetable à capacité faible, worker factice écrivant quelques MiB au-delà d'un petit quota attendu, sans approcher la capacité totale.
- **Correction :** utiliser un backend borné par session en octets et inodes, comptabiliser/réserver la capacité avant lancement, appliquer une politique de rétention et refuser les moteurs sans mécanisme vérifiable.
- **Régression :** avec un quota minuscule de fixture, l'écriture excédentaire doit échouer sans affecter un second tenant ; vérifier aussi libération, rétention et comptage après cleanup.
