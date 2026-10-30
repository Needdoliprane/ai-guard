# CLI Codex / Claude réellement interactive — conception implémentable

30 octobre 2026. **DESIGN seulement ; aucun code produit modifié, aucun test ni appel fournisseur effectué par cette étude.** Propriétaire de cette étude : sous-agent runtime/infra. Les choix ci-dessous sont des propositions techniques, pas des décisions attribuées au mainteneur. Aucun credential ni cache d'authentification utilisateur n'a été lu. Le dépôt et ses fichiers non suivis ont été préservés.

## Résultat attendu et constat

Le parcours doit réellement ouvrir la TUI d'un CLI dans le worker : saisie, réponses aux dialogues, navigation, rendu, redimensionnement, interruption d'une requête, arrêt et récupération. Un `codex exec` ou un `claude -p` qui termine correctement ne prouve pas ce parcours.

La documentation officielle distingue [Codex TUI / exec](https://learn.chatgpt.com/docs/developer-commands?surface=cli) et [Claude / claude -p](https://code.claude.com/docs/en/cli-reference). Les sources publiques ont été consultées seulement en lecture. Elles ne qualifient pas les versions réellement embarquées.

Constats directs du checkout :

- `src/runner.rs:21` : `RunOptions` n'a pas de mode interactif. `choose_plan` garde les options dans le checkpoint et compare exactement les options d'une approbation en attente.
- `src/runtime.rs:158` : `ContainerSpec::create_args` ne donne `--interactive` qu'au helper Import ; Run ne reçoit ni stdin ouvert ni TTY.
- `src/runtime.rs:491` : le constructeur de commande moteur met stdin à null. `execute_created` (`:779`) ne le remplace que par un pipe d'archive ; il draine stdout/stderr vers des captures et attend un terminal runtime.
- `src/runner/managed.rs:1388` : lancement réellement possédé, Created inspecté, checkpoint privé, puis StartClaim durable ; `EmitOnce` seul autorise start. Le monitor traite les intents d'arrêt.
- `src/runner/managed.rs:2205` : reprise déjà sûre vis-à-vis des claims consommés ; elle observe un worker vivant mais ne réattache pas son terminal.
- `src/runtime/managed.rs:1086` : le fingerprint v1 inclut commande, montages et sandbox mais pas `Tty`, `OpenStdin`, `StdinOnce` ou le logging. `src/audit/control.rs:311` recalcule le hash de LaunchSpec et des snapshots en relisant l'audit : ajouter un champ sérialisé par défaut casserait l'histoire.
- `images/worker.Dockerfile` : versions fournisseurs exactes sur choix opérateur, utilisateur 1000, HOME temporaire. Les états du CLI sont absents au premier démarrage. `docs/operations.md:165` indique explicitement l'absence de login interactif aujourd'hui.

Critère : un utilisateur atteint le prompt réel puis échange deux tours, commande une modification dans la copie, l'examine, arrête proprement et exporte vers une destination neuve ; l'original est identique. Le registre, le quota, l'audit et le résultat restent cohérents en cas de refus/panne.

## Approches et extension recommandée

1. **Hériter directement des trois stdio vers `docker start -ai`** : peu de code et bonne compatibilité TUI, mais sortie du worker non fiable directement exécutée par le terminal hôte, pas de frontière pour masquer les secrets connus, pas de contrôle des séquences OSC52/DCS/APC/PM. À écarter pour le produit demandé.
2. **Pont PTY Rust local devant la CLI moteur approuvée** : extension recommandée. Le broker conserve la création, les claims et les observations ; un petit module terminal ne transporte que des octets en mémoire, filtre l'affichage et gère la restauration du terminal. Aucun nouveau service, moteur de stockage ou canal web.
3. **Client direct de l'API moteur / terminal web** : plus gros changement du transport et de sa qualification ; inutile pour obtenir ce CLI local. À ne pas introduire ici.

Le pont PTY s'insère dans le `runner` existant. Il ne donne pas le socket moteur au worker, ne monte ni projet original ni caches hôte et ne transforme pas les logs fournisseurs en médiation transitive.

## Contrat CLI et contrats durables

Proposition d'usage, à implémenter :

```text
agentsandbox run --interactive --project /projet --provider codex \
  --profile observe --network bridge --secret-env-file /prive/codex.env -- codex

agentsandbox run --interactive --project /projet --provider claude \
  --profile observe --network bridge --secret-env-file /prive/claude.env -- claude

agentsandbox worker-resume --interactive --session UUID
```

Le flag choisit un transport TTY ; il ne réécrit jamais `argv` pour remplacer `codex` par `exec`, ni `claude` par `-p`. Les commandes non interactives restent inchangées. On peut donner un TTY à un autre outil ; la preuve fournisseur reste séparée. Le réseau reste none par défaut ; les exemples réseau exigent observe + bridge et conservent la déclaration de couverture réelle.

Prévalidation avant admission/import : Unix supporté ; stdin et stdout sur un terminal valide ; accès au terminal contrôlant ; dimensions valides ; préparation du PTY et disponibilité du preset fournisseur. Un pipe ou une redirection avec `--interactive` est refusé avec une action claire. Ne pas se rabattre silencieusement sur un mode non interactif. Refuser `--capture-output` avec interactif : la TUI n'est pas un flux JSONL, ses sorties et son echo ne sont jamais un transcript d'audit.

Champs indispensables dans une **nouvelle variante versionnée** de LaunchSpec : mode `batch | interactive_tty`, preset de terminal/affichage et politique de transcript `disabled`, politique de logging moteur `none`, et preset de bootstrap si utilisé. Le contenu est lié au hash d'admission/spec et aux options privées ; une reprise ou approbation ne peut changer le mode. Une dimension de fenêtre est mutable et ne doit pas invalider la spécification à chaque resize. Le mode interactif doit être visible dans command.requested/runtime.submitted et dans la revue humaine ; la décision reste explicitement une autorisation de lancement de ce processus avec ce canal, pas une autorisation indépendante de chaque action saisie.

Compatibilité à préserver explicitement :

- Lire LaunchSpec v1 et RunPlan v2 historiques avec leur forme sérialisée historique exacte, leurs domaines de hash et leurs claims inchangés. Ne jamais ajouter des champs par défaut dans le calcul des hashes historiques.
- Employer une variante LaunchSpec v2 (ou enveloppe équivalente clairement versionnée) pour les nouvelles sessions interactives ; une table de dispatch versionnée recalcule chaque hash selon son contrat d'origine. Le champ Rust peut être optionnel et omis pour la variante ancienne, mais cette commodité ne remplace pas la validation de version.
- Introduire un fingerprint de configuration v2 pour les nouvelles sessions, contenant TTY, OpenStdin, StdinOnce et log driver. Conserver le calcul v1 pour les anciens snapshots, avec un marqueur de version privé explicite ; aucun rehash silencieux d'un Created historique. Les versions inconnues bloquent les effets.
- Négocier la capacité terminal avant soumission à un gateway ancien. Les DTO `deny_unknown_fields`, les réponses workers et les clients existants imposent une évolution de protocole explicite ; ne pas livrer de nouveaux champs incompatibles sous une réponse prétendument ancienne. Aucun champ tenant libre ne choisit le périmètre. Le besoin exact de migration SQL dépend du stockage JSON déjà utilisé : à établir par replay mixte réel, pas à présumer par renommage.
- Une session historique batch ne devient jamais interactive par `worker-resume --interactive`. Nouveau mode incompatible : message exigeant une nouvelle session auditée, sans toucher à l'ancienne.

## Cycle runtime sans second start

Créer le worker Run avec les options durcies existantes plus `--interactive --tty --log-driver none` et un `TERM` fixe compatible avec le preset qualifié. Ne pas propager l'environnement de l'hôte. Pour l'input réattachable, vérifier réellement `Config.OpenStdin=true`, `Config.Tty=true`, `Config.StdinOnce=false` et `HostConfig.LogConfig.Type=none` ; ne pas présumer la valeur issue des seuls flags. Helpers Import/Scan/Export inchangés, sans TTY ni credential.

Avant start, l'inspection whitelist valide ces quatre faits, le preset, l'image immuable, argv/bootstrap, uid, volumes et toutes les restrictions actuelles. Pas de dump inspect/env. Le callback Created et StartClaim demeure l'unique frontière d'émission ; échec d'audit/claim/inspection = aucun start. Seul `EmitOnce` produit la commande moteur approuvée et ses arguments séparés.

Premier start : commande `start --attach --interactive --detach-keys=ctrl-x,ctrl-x CID`, stdio branchés au **slave PTY local** ; ce n'est pas un terminal exposé depuis le worker à l'hôte. Le parent conserve le master et le terminal physique. Le code moteur et sa sélection restent ceux de l'invocation ancrée existante ; aucun shell hôte. Docker publie [start attach + stdin](https://docs.docker.com/reference/cli/docker/container/start/). Son [code start](https://raw.githubusercontent.com/docker/cli/master/cli/command/container/start.go) comporte le suivi de taille ; le [streamer](https://raw.githubusercontent.com/docker/cli/master/cli/command/container/hijack.go) configure le terminal. Ce mécanisme proposé doit être vérifié sur la version installée, pas déduit de master.

Le loop d'observation existant reste indépendant de la pompe d'octets : un écran lent ou inactif ne bloque ni le stop, ni l'audit obligatoire, ni le délai. Une sortie d'attach n'est pas un terminal du worker. Si le worker est encore vivant après une perte de transport, demander un arrêt ciblé et inspecter ; si l'observation est indisponible, conserver `InDoubt`, quota et instance. Ne pas convertir l'exit 0 du client moteur ou un EOF en succès métier.

Le terminal observé passe par l'enregistrement atomique actuel, usage `billable=false`, puis ACK et cleanup ; diff et export sûrs restent les chemins existants. En mode TTY, stdout/stderr du processus métier sont fusionnés : compte unique de transport si mesuré, flux séparés indisponibles et capture désactivée. Ne pas afficher deux zéros inventés. Ne pas appeler `parse_redacted_capture` sur une TUI ; `observer.coverage` décrit lifecycle générique et observation de fichiers, événements fournisseur indisponibles, autorisation imbriquée false.

## Terminal Rust : état, signaux et affichage non fiable

Petit module `src/runtime/terminal.rs`, avec `libc` déjà dépendance et types de ressources possédés :

- Ouvrir le terminal contrôlant avec des descripteurs possédés, sauvegarder termios et flags, vérifier le groupe de premier plan. Ne pas changer les flags partagés d'un stdin du shell à son insu.
- `TerminalGuard` RAII possède la restauration idempotente. Passage raw seulement après revue humaine et au début de l'attachement ; restauration explicite avant tout diagnostic/résumé, puis Drop de secours. Les erreurs de restauration sont visibles après tentative sûre. Éviter tout `process::exit` pendant cette portée.
- PTY master non bloquant, slave transmis à la CLI moteur approuvée. Le sous-processus a son groupe/session de transport ; les opérations pre_exec sont limitées aux appels système nécessaires, justifiés et sans allocation. Ne pas appliquer de kills au groupe du shell utilisateur.
- Entrée réelle -> master ; master -> filtre -> masquage -> terminal. Files bornées avec backpressure, aucune croissance liée à la durée, annulation des tâches et fermeture de tous les descripteurs à chaque issue. Ne pas faire lire stdin par `tokio::io::stdin` si une lecture bloquante survivrait à l'annulation : fd non bloquant + readiness/cancellation explicite.
- Taille initiale copiée avant le start ; SIGWINCH lit la taille et applique TIOCSWINSZ sur le PTY puis réveille la CLI moteur si nécessaire. Docker doit propager jusqu'au TTY du worker. Coalescer les rafales, rejeter dimensions invalides ; aucune commande métier `exec` pour simuler un resize. Une absence de propagation est un FAIL de fonctionnalité.
- Ctrl-C **tapé** reste un octet vers le TTY fournisseur pour interrompre sa requête selon ses règles. Désactiver la branche actuelle `ctrl_c => kill` pour ce cas. Un signal SIGINT/SIGTERM/SIGHUP externe au broker est un événement opérateur distinct : arrêt ciblé, restauration et observation. Ne pas double-transmettre un même Ctrl-C.
- Ctrl-D tapé reste un octet ; sa signification dépend de l'état distant. EOF/HUP physique est une perte du transport, pas une preuve de fin. La séquence réservée `Ctrl-X Ctrl-X`, expliquée avant entrée raw et interceptée avant la CLI moteur, demande un **arrêt du worker**, pas un détachement laissant croire au timeout autonome. Le choix explicite du même detach-key Docker évite de dépendre de sa configuration ambiante ; si une séquence atteint malgré tout la CLI, sa fin d'attach est inspectée et traitée sans faux terminal.
- Si le job control est géré : signal de suspension externe => restaurer avant suspension et remettre en état sur SIGCONT après validation du worker ; sinon refuser/supprimer cette suspension explicitement dans le transport et documenter le comportement. Ctrl-Z saisi est envoyé au fournisseur. Tester l'absence de shell hôte laissé raw.

La frontière d'affichage est obligatoire. Aucun octet de worker ne va directement au terminal hôte, y compris stderr du client moteur. Filtre streaming Rust à états et allowlist, pas regex par chunk :

1. Texte UTF-8 valide et CR/LF/HT/BS ; texte invalide remplacé de façon contrôlée. Décodage incrémental : ne pas confondre un octet continuation UTF-8 avec un contrôle C1 ; contrôler aussi les représentations UTF-8 des caractères C1.
2. Autoriser uniquement les CSI utiles et précisément paramétrées : SGR couleurs/styles bornés, déplacements curseur/effacement, régions de défilement, visibilité curseur, écran alternatif et bracketed paste. Autoriser modes souris/focus/clavier uniquement lorsqu'un besoin des versions réelles l'établit et avec remise en état. Inconnus refusés ; pas d'autorisation générale de tout `CSI ... h/l`.
3. Rejeter OSC (notamment OSC52 clipboard et OSC8 lien), DCS, APC, PM, SOS, contrôles C1 et autres fonctions non allowlistées ; arrêter ces chaînes sur BEL/ST selon la grammaire. Interdire notamment impression `CSI ... i`, manipulation de fenêtre `CSI ... t`, modification clavier/titres/clipboard et réponses arbitraires. Si une TUI exige une requête de couleur/capacité, synthétiser une réponse fixe du preset ou autoriser précisément une requête examinée : aucune ouverture générique OSC.
4. Taille stricte d'un CSI et nombre/valeurs de paramètres ; chaînes refusées consommées sans accumuler le corps ; délai/nombre d'octets maximal d'une séquence non terminée puis erreur contrôlée et arrêt du transport. Après dépassement, aucune réémission brute pour « récupérer » l'écran. Une séquence scindée sur n'importe quel octet demeure bloquée, y compris ESC/ST scindés.
5. Émettre des tokens sûrs, puis masquer les secrets littéraux connus avec une fenêtre mémoire bornée conservant les suffixes possibles entre chunks. Aucun flush sur timeout ne doit libérer un préfixe sensible. Cela ne promet pas de détecter tout encodage ou reconstruction par déplacement curseur d'un secret inconnu. Les tokens de contrôle du filtre ne doivent pas être corrompus par un remplacement textuel naïf.
6. Compter éventuellement les catégories de séquences supprimées, sans contenu brut, sans extrait de prompt. Afficher un avis générique hors écran TUI si le filtre empêche un rendu essentiel. Il faut prouver la compatibilité avec les vrais Codex/Claude ; désactiver le filtre pour les faire passer est un échec.

À la sortie, le guard émet uniquement ses propres séquences de remise en état : curseur visible, SGR normal, sortie écran alternatif, désactivation des modes souris/focus/paste/clavier qu'il a autorisés, puis restaure termios/flags. La termios seule ne répare pas un écran alternatif ni un curseur caché. SIGKILL d'un processus ne permet pas d'exécuter son Drop : ne jamais prétendre une restauration garantie par RAII pour SIGKILL. Fournir une commande de récupération documentée et vérifier le cas via le superviseur/terminal de test ; pas de `reset` shell exécuté automatiquement par le produit.

## Secrets et authentification

Réutiliser la source privée ancrée, le snapshot, le digest et les refus de collision contrôle existants. Seul le worker métier reçoit le credential fournisseur ; ni token runner/viewer/Jev, ni répertoire de contrôle, ni cache hôte `.codex`/`.claude`. Les clés ne vont pas dans argv, image, volume workspace, audit ou IPC public. L'environnement worker reste accessible à son code et au daemon de confiance : aucune promesse de secret caché à l'agent.

Pour Codex, la [documentation d'authentification](https://learn.chatgpt.com/docs/auth) décrit un login API lisant la clé sur stdin et un cache sous CODEX_HOME. Ne pas transposer le comportement documenté de CODEX_API_KEY pour exec à la TUI. Si la version épinglée requiert ce login, un **bootstrap Rust dans le même worker** : crée sous le HOME tmpfs privé la configuration minimale, invoque le vrai binaire par programme/arguments séparés, transmet la clé sur un pipe stdin privé à `codex login --with-api-key`, attend avec timeout et masque ses diagnostics, puis remplace son processus par le CLI interactif. Pas de shell d'automatisation, pas de dump d'auth.json, pas de cache dans /workspace. Le digest/preset bootstrap fait partie de LaunchSpec et l'inspection sait que l'entrypoint réel est ce bootstrap. La clé initiale demeure dans le handoff explicite déjà admis. Ne pas fabriquer à la main un auth.json non documenté pour une version inconnue.

Le [login Claude](https://code.claude.com/docs/en/authentication) reconnaît ANTHROPIC_API_KEY avec confirmation interactive possible. Laisser cette confirmation et les dialogues de confiance dans la TUI ; ne pas imposer une désactivation globale des permissions pour les supprimer. La fonctionnalité doit permettre la réponse clavier réelle.

Le profil seccomp, cap-drop, uid, read-only, no-new-privileges, limites et absence de host mounts restent identiques. Si un sandbox interne Codex exige un syscall interdit, conserver le refus, identifier précisément son erreur et choisir un mode fournisseur officiellement compatible avec le confinement extérieur, explicitement documenté et testé. Ne pas mettre le worker en privileged, ajouter des caps ou désactiver seccomp pour obtenir un écran.

Audit : seulement mode, preset, IDs, état/horodatages observés et motifs d'arrêt ; aucune frappe, prompt, réponse ou echo. `--log-driver none` et son inspection empêchent un transcript Docker implicite pour ce nouveau mode ; vérifier réellement qu'une sonde d'echo sensible ne se retrouve pas dans `docker logs`, audit, checkpoint, erreurs ou preuves. Les historiques privés créés par le fournisseur restent sous HOME tmpfs et disparaissent avec l'instance ; leur existence ne justifie aucun export automatique.

## Reprise, limites de durée et blocages

`worker-resume --interactive` réutilise le chargement authentifié tenant/origin, le verrou AttemptGuard, la même autorité, le CID et les checks de configuration. Worker Created jamais parti et claim absent : chemin de premier start actuel, sous la même admission. Claim consommé/incertain : **aucun start**, même si Created paraît intact. Worker vivant avec claim durable : méthode nouvelle `attach_owned_interactive` ne contient que `attach` et les contrôles d'identité, jamais `start`, `restart` ou `exec`. Worker terminal : finalisation/cleanup actuelle, aucune TUI recréée. Instance manquante ou incohérente : quota détenu, diagnostic et stop/récupération guidés. Owner vivant : message de verrou réel, aucun takeover fondé sur un heartbeat.

Avant tout attach de reprise, committer l'événement d'attachement autorisé sans transcript. Sa perte d'ACK n'arme aucune permission de start. Un seul écrivain actif ; l'OS lock ne peut pas être contourné pour une seconde TUI. Les octets perdus entre transports restent inconnus ; on ne reconstitue pas un historique fournisseur depuis des logs.

**Blocage P1 préexistant :** le timeout actuel appartient au processus CLI (`Instant` dans execute_created). SIGKILL de ce broker le supprime. Ajouter TTY ou reattach ne fournit pas une durée autonome et ne ferme pas G04/G09. La conception de [supervision durable existante](/Users/louis/Documents/ai-guard/docs/engineering/evidence/2026-10-29/worker-registry/ai-guard-broker-supervision-design.md) est elle-même DESIGN non approuvé. Cette étude ne crée pas de microservice et ne la présente pas comme livrée. Sans enforcement indépendant qualifié, ne pas annoncer détachement durable ni limite conservée après crash ; cet écart reste obligatoire et bloque la qualification complète.

Une reprise ne doit en aucun cas programmer `now + timeout` après un claim. La bonne durée se lie à un armement durable avant premier start avec horloge/boot qualifiés ; sa reprise garde la même échéance. `StartedAt + timeout` sur horloge calendrier est un diagnostic conservateur possible, pas une preuve équivalente d'enforcement. Le pont peut être implémenté et relu séparément, mais le parcours de panne complet attend la fermeture explicite de cet écart. Un watchdog dans le worker, tuable par ce worker, ne remplace pas le contrôle de confiance.

## Propriétaires possibles et ordre d'intégration

| Lot / propriétaire exclusif | Fichiers | Acceptation et risque dominant |
|---|---|---|
| Contrats/core | `src/control.rs`, `src/audit/control.rs`, `src/api.rs`, DTO front concernés | Mode et preset immuables, hashes historiques identiques, négociation/version explicites, replay mixte et isolation A/B. Ne pas écrire en parallèle dans le lot SQLite du root. |
| Runtime/terminal | `src/runtime.rs`, `src/runtime/managed.rs`, nouveau `src/runtime/terminal.rs` | PTY, inspection des flags/logging, filtre streaming, restauration et resize ; aucune nouvelle émission start. |
| Runner/CLI | `src/runner.rs`, `src/runner/managed.rs`, `src/operations.rs` | Options, refus non-TTY, auth/preset, start/reattach, erreurs/stop, résumé sans faux compteurs et sans parsing TUI. |
| Worker auth si indispensable | nouveau binaire Rust worker bootstrap, `images/worker.Dockerfile`, `Cargo.toml` seulement si nécessaire | Login dans tmpfs et exec réel, timeout bootstrap, aucun secret dans argv/logs/volume ; image immuable qualifiée. |
| Luna / tests ciblés | nouveaux tests terminal Rust + `tests/runtime_managed.rs`, `tests/runner_recovery.rs`, test réel ignoré `runtime_interactive` | Preuves ci-dessous ; pas de mocks multipliés pour prétendre qualifier le moteur. |
| Revue indépendante + preuves | `docs/operations.md`, `docs/api.md`, TEST_STRATEGY, VALIDATION et état/gates | Revue de lancement/audit/secrets par un second regard ; résultats et écarts conservés, aucune qualification globale par renommage. |

Ordre : reproduire le défaut sur TTY réel sans fournisseur -> figer contrat et compat -> filtre/PTY avec régressions -> intégrer au cycle managed -> auth réelle minimale -> parcours Docker+Codex puis Claude -> revue indépendante et preuves. Les lots techniques n'exemptent aucune obligation de produit ni la durée après panne.

## Vérifications prioritaires confiables à Luna

Toutes les vérifications proposées ci-dessous sont **NOT_RUN par cette étude**. Pas de suite front ni matrice complète par réflexe ; lancer les cibles touchées après intégration. L'agent d'exécution des tests est Luna conformément à AGENTS.md.

**Priorité 1 — tests réels locaux sans fournisseur, harness Rust sous PTY.** Un helper Rust de test dans une image dédiée écrit ses observations sous /workspace : isatty des fd0/1/2, dimension, caractères reçus, réponse à Ctrl-C et fermeture. Ce n'est pas une fixture fournisseur. Vérifier dans un vrai Docker :

- image/CID/mounts/limits/profile stricts et Tty/OpenStdin/StdinOnce/log-driver observés ; prompt bidirectionnel et resize 80x24 -> 120x40 réellement vus par le processus ; UTF-8, touches, paste, Ctrl-C en cours d'activité et Ctrl-D à vide.
- worker qui quitte avec 7 => terminal réel 7 ; attach transport interrompu alors que worker tourne => aucun faux terminal ; timeout/worker-stop => arrêt du seul CID, ledger final unique après ACK ; autre worker témoin intact.
- canary non sensible simulant une clé, envoyé uniquement au runtime de test et echo volontaire : absent des persistences/erreurs/preuves après contrôle booléen qui n'imprime pas sa valeur ; `docker logs` ne doit contenir aucun transcript. Le fichier workspace de test ne doit pas être confondu avec le transcript interdit.
- séquences malveillantes OSC52/DCS/APC/PM/SOS/C1, presse-papiers/fenêtre/impression, taille énorme et séquences sans ST : aucune exécution de commande terminal hôte, aucune fuite brute ; ANSI autorisé visible et TUI de test utilisable. Les assertions comparent des tokens/effets sûrs, jamais une capture brute de contenu sensible.
- termios et état de présentation restaurés après sortie normale, erreur create/attach/inspect, timeout, SIGTERM/HUP, panne audit, annulation de tâche et stderr malveillant. SIGKILL garde sa limite explicite ; le harness doit pouvoir récupérer son PTY sans endommager le terminal de l'opérateur.

**Priorité 2 — tests logiciels ciblés utiles.** Nouveau `terminal_filter` : partitions sur chaque octet des familles dangereuses et des CSI autorisées, UTF-8/C1, limites mémoire, secret littéral split/overlap/fin partielle et pas de flush sensible ; réel PTY local pour RAII/resize/signaux. Étendre managed/recovery avec terminal flags/log-driver divergents refusés avant claim, mode incompatible, hashes v1 stables après réouverture et snapshot réel mixte, ACK claim perdu sans second start, reattach n'appelant jamais start/restart/exec, deuxième owner refusé, tenant A/B/rôles et panne d'audit attach. Cibles pertinentes après écriture :

```text
cargo fmt --all -- --check
cargo test --locked --test runtime_managed
cargo test --locked --test runner_recovery
cargo test --locked --test terminal_filter
cargo clippy --locked --all-targets -- -D warnings
cargo build --release --locked
```

`terminal_filter` et `runtime_interactive` sont des noms proposés, inexistants aujourd'hui. Si de vrais contrats API/stockage changent, ajouter les cibles worker_registry/workers_api et replay/migration concernées. Élargir à la suite intégrée au jalon de contrat transversal, pas à chaque correction cosmétique de TTY.

**Priorité 3 — vraie qualification fournisseur Docker.** Image exacte, version Codex puis Claude, arch/runtime/rootful exception/gateway/config explicites. Sous le harness PTY Rust (pas `exec` ou `-p`) : accès au prompt réel, authentification minimale, une première requête, un second tour après navigation/redimensionnement, dialogue de confiance/permission, interruption Ctrl-C puis nouvelle saisie, changement dans copie et diff/export neuf, vérification source inchangée, stop et audit réouvert. Pas de capture des écrans d'auth, pas de clé dans les commandes partagées. Une clé Codex disponible ne donne ni compte ni autorisation Claude : Claude est BLOCKED précisément si l'auth manque. Ne pas fabriquer un PASS à partir de la simple apparition de l'écran d'accueil.

**Priorité 4 — panne et durée.** SIGKILL broker après start, reprise même CID via attach sans second start, terminal/audit unique après réconciliation ; ne pas réarmer la durée. La limite autonome après crash exige le lot d'enforcement réel nommé plus haut et son test indépendant ; tant qu'il manque, résultat BLOCKED pour cette exigence même si la TUI fonctionne.

Rapport distinct Docker/macOS ARM64 pour l'environnement réellement testé. Podman = NOT_RUN tant que transport TTY/logging/resize/reattach ne sont pas qualifiés sur Podman réel ; ne pas inférer sa compatibilité depuis des fixtures Docker. Kubernetes = BLOCKED, adaptateur non livré. Aucun PASS translaté entre plateformes.

## Bilan de cette étude

**PASS (lecture seulement)** : défaut de transport confirmé par source ; extension réalisable dans la frontière broker existante, contrats/risques et critères définis. **NOT_RUN** : implémentation, tests Rust/PTY/moteurs/fournisseurs et revue d'un patch. **BLOCKED pour la qualification complète** : durée autonome après perte du broker ; auth Claude si absente ; toute incompatibilité du sandbox fournisseur avec le profil durci ; protocole/fingerprints non migrés ; compatibilité TUI sous filtre non démontrée. **NOT_APPLICABLE pour cette étude** : opération cloud/PKI/NAS, publication et destruction de données.

Ce document n'est ni une preuve de support interactif livré, ni une release qualifiée.
