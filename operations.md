# Exploiter Agent Sandbox

## Ce que lance le produit

Le binaire `agentsandbox` est un programme Rust. Sur le poste de confiance, il appelle exclusivement la CLI `docker` ou `podman` avec un tableau d'arguments ; il n'exécute aucune commande métier dans un shell hôte. Sans moteur disponible, la commande échoue. Gateway et dashboard sont des conteneurs distincts sur un réseau interne. Un troisième conteneur de contrôle, `ingress`, publie uniquement leurs accès loopback via un relais Rust sans secrets, données ni socket montés. Lui seul rejoint aussi un bridge de publication. Les inspections conservées ne montrent aucune route par défaut IPv4 dans les deux services internes ; les autres canaux de sortie restent à qualifier. Les workers sont créés séparément pour chaque session et ne rejoignent jamais ces réseaux de contrôle/publication.

Le projet original n'est pas monté dans un conteneur. Le runner en prépare une archive filtrée, crée un volume nommé `agentsandbox-work-<UUID>` et importe l'archive via stdin dans un worker non privilégié. Les modifications restent dans ce volume. `export-workspace` écrit une copie vérifiée dans un **nouveau répertoire extérieur au projet original**.

Le mode normal exige un moteur rootless. Docker Desktop peut annoncer un daemon non rootless dans sa VM ; `--allow-rootful-runtime` autorise explicitement ce cas. La présence d'une VM n'est pas interprétée comme une preuve d'équivalence au mode rootless. Chaque session utilisant cette exception émet `runtime.security_downgrade`. Podman cible ici son fonctionnement rootless, avec `keep-id:uid=1000,gid=1000` pour les workers.

**Statut de validation :** consulter `docs/engineering/VALIDATION.md` et `docs/engineering/CURRENT_STATE.md` pour le candidat et les versions réellement vérifiés. Le relais publie les accès loopback sur Docker Desktop 4.97.0/Engine 29.9.2 macOS ARM64 avec exception rootful explicite. Confinement/copie/export/timeout, smoke avec refus audité et parcours navigateur sur cette pile ont passé. Ils ne qualifient ni les quatre combinaisons plateforme/moteur, ni le scénario à trois workers répartis entre deux clients, ni le cloud. Les tests `runtime_smoke` et `runtime_confinement` sont ignorés par défaut et doivent être lancés explicitement ; leurs exécutions réelles sont consignées séparément.

## Prérequis

- Rust **1.99.1**, fixé dans `rust-toolchain.toml`, pour compiler la CLI hôte. La compilation de l'image de contrôle utilise la même version.
- Docker Engine avec Compose, ou Podman rootless avec un fournisseur `podman compose` installé. `doctor` interroge réellement `info` et `version` ; une installation de binaire sans moteur fonctionnel ne suffit pas.
- Node **24** est utilisé dans les étapes de build des images. Il n'est pas nécessaire sur l'hôte pour construire celles-ci. Le build de l'interface utilise son lockfile npm.
- Les ports locaux `8787` et `4000` doivent être libres. Ils sont publiés sur `127.0.0.1` uniquement.
- Le code de copie et de secrets nécessite Unix. Les quatre couples Linux x86_64/macOS ARM64 × Docker/Podman exigent leurs propres preuves ; le résultat partiel Desktop ne qualifie pas les autres. WSL et les autres plateformes ne sont pas implicitement qualifiés.

Les tags des images de base suivent une version Rust précise, une version majeure Node et une suite Debian. Ils ne constituent pas un verrouillage par digest. Pour une livraison reproductible jusqu'aux paquets système, fixer et vérifier les digests et les dépôts APT dans la chaîne de build de l'équipe ; ce durcissement de supply chain reste distinct du lockfile Rust/npm livré.

## Démarrage

Depuis la racine du dépôt :

```sh
cargo build --release --locked
./target/release/agentsandbox init
./target/release/agentsandbox doctor --runtime docker
./target/release/agentsandbox build --runtime docker
./target/release/agentsandbox up --runtime docker
```

Pour Podman, remplacer `--runtime docker` par `--runtime podman`. Pour Docker Desktop avec daemon non rootless, ajouter `--allow-rootful-runtime` à `doctor`, `build`, `up` et aux commandes `run` concernées. Ce choix est explicite pour chaque invocation ; aucun réglage global ne l'active discrètement.

Le dashboard est disponible à `http://127.0.0.1:4000`. Ouvrir localement `.agentsandbox/secrets/viewer` et saisir son contenu dans l'écran de connexion. La CLI n'imprime jamais les clés. Le jeton runner reste réservé au broker ; le dashboard reçoit uniquement le jeton viewer et ne propose aucune exécution de commande.

`init` crée des secrets indépendants de 32 octets aléatoires, encodés en hexadécimal. Un second `init` conserve les valeurs existantes. Les répertoires de configuration, de secrets, de projections de montage, de sessions et d'approbations ont le mode `0700` et appartiennent à l'utilisateur courant. Un répertoire existant avec un autre propriétaire, des permissions différentes ou un lien symbolique est refusé, afin que l'opérateur corrige consciemment son emplacement ou ses permissions.

Les originaux `.agentsandbox/secrets/runner` et `.agentsandbox/secrets/viewer` restent en **`0600`**. Ils ne sont jamais montés dans les services. Les anciens originaux `0444` sont ramenés à `0600` sans rotation ni modification de leur contenu. Chaque secret doit être un fichier régulier, appartenir à l'utilisateur courant, ne pas avoir de lien physique ou symbolique et contenir une valeur valide bornée.

Les fichiers Compose Docker et Podman montent à la place `.agentsandbox/mounted-secrets/runner` et `viewer`, en lecture seule. Ces **projections séparées** sont `0444` dans un répertoire hôte `0700`. Le gateway reçoit les deux fichiers ; le dashboard reçoit uniquement `viewer`. Le répertoire parent, les originaux et les autres secrets ne sont pas montés. Les autres utilisateurs hôtes ne peuvent pas traverser ce parent privé. Ne pas déplacer les projections dans un dossier public ou les exposer par un autre montage.

Cette séparation traite le décalage d'UID : en [Docker rootless, les fichiers de l'utilisateur hôte sont généralement vus comme appartenant à root dans le conteneur](https://docs.docker.com/engine/security/rootless/uid-gid-mapping/), alors que les services s'exécutent en UID 1000. Pour une source de secret de type fichier, [Compose ne peut pas appliquer les remappages `uid`, `gid` et `mode` au bind mount](https://docs.docker.com/reference/compose-file/services/#secrets). Les projections possèdent donc réellement leur permission de lecture nécessaire, sans passer le service en root. Le démarrage effectif des services UID 1000 a constaté leur lisibilité sur Desktop 4.97.0/Engine 29.9.2 ; les autres moteurs, mappings rootless et politiques SELinux restent à qualifier séparément.

`init`, et donc `up`, rafraîchit les projections par création temporaire `0600`, passage à `0444` puis remplacement atomique relatif au descripteur du répertoire protégé. Un processus utilisant déjà l'ancien inode continue à lire l'ancienne projection. **`up` recrée les services de contrôle** avec `--force-recreate` pour remonter les nouveaux inodes et recharger les clés au démarrage ; les workers et les volumes ne sont pas supprimés. Après une modification manuelle des sources, utiliser `up` avant de lancer de nouvelles commandes.

**Répéter `up` redémarre le gateway et le dashboard, même sans rotation de clé.** Les connexions HTTP/SSE sont interrompues ; l'audit persiste dans son volume. Une session active peut échouer à sa prochaine écriture d'audit pendant cette recréation : `up` n'est donc pas sans effet sur un job en cours. Pour une rotation, attendre la fin des jobs, remplacer la valeur du fichier source privé en conservant son mode `0600`, puis relancer `up` avec les mêmes options de runtime. Attendre ses contrôles de disponibilité avant un nouveau job et reconnecter le dashboard avec le nouveau jeton viewer si celui-ci a changé.

## Registre des workers et changement de version

Les bases neuves utilisent le schéma de contrôle v2. Une base v1 existante reste lisible pour l'audit, mais les nouvelles opérations workers sont refusées avec `503 migration_required`. `up` ne migre pas silencieusement les données. Prévoir une maintenance avant d'utiliser le nouveau broker : arrêter les brokers et les services de contrôle, et vérifier qu'aucune ancienne session n'est encore en activité. Le flag de confirmation ci-dessous est une attestation opérateur, **pas un mécanisme de fencing**.

Depuis le périmètre de contrôle de confiance, avec la base et un chemin de sauvegarde **neuf** accessibles au même utilisateur :

```sh
agentsandbox migrate-control \
  --data /chemin/prive/audit.sqlite \
  --tenant-id local \
  --backup /chemin/prive/audit-avant-registre.sqlite \
  --confirm-control-stopped
```

Pour un registre multi-client, répéter avec le tenant et la base configurés, sans changer leur liaison. Pour un volume conteneurisé, exécuter le même binaire dans un conteneur de maintenance arrêté du point de vue applicatif, avec uniquement ce volume accessible ; aucun socket moteur ni secret fournisseur n'est nécessaire. Conserver cette maintenance et son rapport comme preuves. Ne pas copier une base SQLite active en ignorant ses fichiers WAL/SHM.

La commande exige une source régulière existante, refuse une sauvegarde déjà présente et réalise une sauvegarde SQLite vérifiée avant modification. Elle compare l'intégrité et la tête de chaîne, ajoute un genesis de contrôle et conserve les anciens événements/hashes. Le rapport donne versions, chemin de sauvegarde, séquence et hash de genesis. Un échec peut laisser une sauvegarde de diagnostic : la conserver et choisir un nouveau chemin après investigation. Ne pas supprimer la base pour contourner un refus. La sauvegarde v1 et la migration ne fournissent pas un inventaire des anciennes exécutions hors registre.

Après redémarrage, vérifier l'intégrité, l'identité, le quota et l'absence d'erreur de migration avant une nouvelle admission. La restauration d'une sauvegarde v2 avec des claims déjà consommés **n'est pas qualifiée** : elle peut ressusciter un droit de démarrage si elle est employée sans inventaire/fencing/réconciliation. Ne pas relancer des sessions à partir d'une ancienne sauvegarde comme si les moteurs étaient vierges. La procédure complète reste un gate ouvert.

Le mode local réserve par défaut trois workers, 6 000 milliCPU et 3 Gio au total, avec une durée admise maximale de 3 600 secondes. Les options gateway et la configuration par client sont dans [CLOUD_TENANCY.md](engineering/CLOUD_TENANCY.md). Le dashboard fixe reste inchangé à l'usage. Pour partager une présentation entre clients, lancer `dashboard --shared-tenants` vers le gateway configuré, sans `--viewer-token-file` ni secret monté dans ce dashboard ; les identités viewer sont vérifiées côté serveur. Les Compose livrés restent mono-client.

## Première commande confinée

```sh
./target/release/agentsandbox run \
  --runtime docker \
  --project /chemin/vers/projet \
  --capture-output \
  -- /usr/bin/id
```

L'argument `--` sépare les options du broker de l'argv du worker. Une chaîne contenant `;`, `$()` ou `--privileged` reste un argument littéral si l'exécutable choisi ne l'interprète pas. Un shell demandé explicitement dans le conteneur peut interpréter son propre argument `-c` en mode review/observe ; aucun shell hôte n'est ajouté par le broker.

Les valeurs usuelles par défaut sont : profil `strict`, réseau `none`, image `agentsandbox-worker:local`, timeout de **300 secondes**, mémoire **1 Gio**, **2 CPU**, **256 PID**, tmpfs `/tmp` de **256 Mio**, tmpfs `/home/agent` de **64 Mio**, utilisateur **1000:1000**, racine en lecture seule, aucune capacité et `no-new-privileges`. Le profil seccomp par défaut du moteur reste actif. Aucun home, socket, périphérique, namespace PID hôte ou réseau hôte n'est monté/activé.

La sortie finale fournit l'UUID de session, le nom du volume conservé, le code de sortie, le timeout éventuel et le nombre de fichiers modifiés si le scan a réussi. Le code de sortie du processus conteneur vient de `inspect`. Le code de la CLI d'attachement est conservé séparément dans l'événement de résultat.

## Profils et approbation

| Profil | Politique de lancement | Réseau |
|---|---|---|
| `strict` | Invocation exacte dans `policies/default.json` ; les arguments supplémentaires sont refusés. | `none` uniquement. |
| `review` | Une invocation absente de l'allowlist nécessite une confirmation humaine liée à son condensat. Les refus explicites restent des refus. | `none` uniquement. |
| `observe` | Lancement audité, sans contrôle universel des commandes transitives. Les exécutables explicitement interdits restent refusés. | `none` par défaut ; `bridge` sur choix explicite. |

L'allowlist initiale est volontairement étroite : `id`, `pwd`, `echo sandbox-ok` et une invocation précise de `printf`. `cargo test`, `npm test`, un interpréteur ou un script de build peuvent exécuter du code arbitraire du projet ; leur lancement exige review ou observe. La validation d'une commande de haut niveau ne devient jamais une autorisation indépendante de chacun de ses descendants.

Exemple de demande à approuver :

```sh
./target/release/agentsandbox run --project /chemin/projet --profile review -- cargo test --offline
```

Le terminal affiche la commande, la raison et le SHA-256 de la demande. L'opérateur doit recopier **le condensat complet**. Il n'existe pas de `--yes` global. Le runner renvoie la même identité de demande, le même argv et `approval_hash` au gateway ; celui-ci recalcule le condensat et confirme ou refuse.

Sans terminal, la demande est conservée dans `.agentsandbox/pending/<hash>.json` et aucun conteneur n'est lancé. Répéter les **mêmes options et le même argv** avec `--approve-hash <hash>` reprend exactement ces IDs. Un changement de projet, d'image, de réseau, de runtime, de provider, de timeout ou de commande invalide cette reprise. Le fichier est privé et contient les arguments exacts ; ne placer aucune clé d'API dans l'argv. Une approbation consommée ne peut pas être rejouée : le runner consomme atomiquement son fichier et crée le dossier de session sans écrasement.

Une autorisation ne rend pas les dépendances disponibles hors ligne. Les workers génériques incluent Bash, Git, Python 3, Node/npm, curl, les outils de compilation système et la toolchain Rust ; ils n'incluent pas les dépendances propres à chaque projet. Préparer une image avec les dépendances approuvées, ou utiliser observe + bridge pour une session qui en a explicitement besoin.

## Agents cloud et clés d'API

L'image générique n'installe aucun CLI fournisseur par défaut. Leur installation est optionnelle et exige une **version sémantique exacte** sélectionnée par l'opérateur :

```text
agentsandbox build --runtime docker --codex-version X.Y.Z --claude-code-version A.B.C
```

`X.Y.Z` et `A.B.C` sont des espaces réservés, pas des versions testées. `latest`, `*`, les plages et les options supplémentaires sont refusés. Les paquets utilisés sont `@openai/codex` et `@anthropic-ai/claude-code`, documentés dans leurs sources officielles : [Codex](https://github.com/openai/codex) et [installation Claude Code](https://code.claude.com/docs/en/setup#install-with-npm). Les versions exactes doivent être contrôlées contre la documentation et les exigences du fournisseur choisi.

Placer les clés dans un fichier **extérieur au projet source**, privé à son propriétaire (`0600`), avec des lignes littérales `NAME=value`. Le format ne prend pas en charge `export`, interpolation de shell ou chargement d'un profil utilisateur. Les variables modifiant le chargeur/exécuteur, par exemple `PATH`, `HOME`, `LD_PRELOAD` et `NODE_OPTIONS`, sont refusées.

```text
agentsandbox run --project /chemin/projet --profile observe --network bridge \
  --provider claude --secret-env-file /chemin/prive/agent.env \
  -- claude -p "Inspecte le projet sans modifier les fichiers" --output-format stream-json --verbose
```

Les variables directes documentées sont `CODEX_API_KEY` pour `codex exec --json` ([documentation non interactive](https://learn.chatgpt.com/docs/non-interactive-mode)) et `ANTHROPIC_API_KEY` pour `claude -p` ([variables Claude Code](https://code.claude.com/docs/en/env-vars)). Vérifier leur comportement avec la version épinglée. Ne mettre aucune valeur réelle dans un exemple partagé.

Les valeurs ne sont pas placées dans l'argv, l'image ou le projet. Le runner prépare une copie temporaire privée et transmet son chemin via `--env-file`. Seul le worker métier la reçoit ; les helpers d'import/export n'ont aucune clé. Les valeurs connues sont masquées localement avant l'envoi des événements, puis le gateway applique son masquage avant persistance. **L'agent et le code exécuté dans ce worker peuvent lire leurs variables d'environnement.** Ce mécanisme ne les protège pas d'un agent malveillant ; la protection des credentials du périmètre annoncé reste à implémenter et qualifier lorsqu'elle exige de les garder hors du worker. Le daemon et l'administrateur hôte restent aussi des autorités de confiance : l'environnement d'un conteneur peut être accessible à l'opérateur du runtime via inspect.

`--network bridge` est accepté uniquement avec `--profile observe`. Il émet un avertissement et `network.unrestricted`. Il permet un accès sortant **sans filtrage applicatif**, y compris l'exfiltration possible de données volontairement confiées à ce worker. Le produit ne présente ni ce réseau ni les logs fournisseurs comme un proxy de sécurité universel.

Les sessions n'ont pas de login interactif fournisseur : stdin est fermé pour la commande métier et le home temporaire disparaît avec le conteneur. Utiliser le mode non interactif et les variables d'API officiellement reconnues par la version installée. Aucun appel cloud, authentification ou compatibilité fournisseur n'a été testé dans cet environnement.

## Audit, sorties et couverture

Le runner attend un accusé de stockage durable avant toute action conteneurisée : `command.requested`, décision centrale, éventuelle approbation, import audité et `runtime.submitted`. Une décision sans `audit_seq`, un événement sans `seq`, une réponse malformée, un HTTP d'erreur ou un gateway inaccessible bloque la suite. Une erreur de suivi après démarrage déclenche un arrêt ciblé, dans la limite de la disponibilité du daemon, et conserve l'instance créée pour diagnostic. Sans observation terminale, `runtime.execution.error` laisse le résultat inconnu.

Le worker métier terminé reste inspectable jusqu'à l'accusé d'enregistrement de `command.completed` ou `command.failed`. Un refus ou une perte de réponse à ce stade laisse le conteneur et son volume en place ; aucune nouvelle exécution n'est tentée. Après cet accusé, le nettoyage explicite cible la référence retournée par la création. La sortie JSON de `run` distingue le résultat métier (`exit_code`, `timed_out`) de `cleanup` (`container`, `state` = `removed` ou `unconfirmed`, `error`). Un échec de nettoyage conserve le résultat audité et fait sortir la CLI en erreur ; la présence du conteneur reste inconnue si la réponse a été perdue après sa suppression. Le rapport `runtime.cleanup.failed` est tenté et `cleanup.error_audit` distingue `acknowledged`, `unacknowledged` et `not_applicable` lorsqu'il n'y a pas d'erreur à rapporter. Sans cet accusé, le résumé conserve le résultat terminal, les changements restent inconnus et aucun autre helper n'est lancé. La réservation durable, l'identité d'autorité moteur et la commande de réconciliation après crash restent à implémenter ; cette conservation ne constitue pas encore une reprise automatisée.

`command.started` est émis pendant l'exécution, après qu'un polling de `inspect` (environ 200 ms) a observé un `StartedAt` valide. Le lancement du processus CLI Docker/Podman ne sert pas de preuve de démarrage. Pour une commande très courte, l'inspection finale conserve cette preuve et la date du runtime dans `data.runtime_started_at`.

Le runner draine stdout/stderr pour ne pas bloquer le processus. Il conserve au plus **256 Kio par flux**. La persistance du texte brut est désactivée par défaut. `--capture-output` l'active, avec un maximum de **8 Kio par flux** dans le résultat ; les secrets connus sont masqués avant cette troncature. Les nombres d'octets et les indicateurs de troncature restent présents. Les erreurs retournées par le parcours d'exécution sont expurgées avant affichage, y compris leur chaîne de causes. Le masquage repose sur les clés connues et des motifs reconnaissables ; aucun détecteur ne peut garantir l'élimination de tout secret arbitraire inconnu ou encodé.

Les normaliseurs Codex/Claude/generic traitent le stdout retenu **après la fin** du worker. Ils produisent au plus **500 événements** par session, toujours en provenance `declared` pour les déclarations des outils. Les lignes inconnues ou dépassant 64 Kio sont ignorées. Le JSON borné est décodé puis expurgé avant les projections tronquées, afin de conserver une télémétrie valide même avec des chaînes échappées. Le bilan `observer.coverage` rend ces limites explicites. Désactiver le texte brut n'empêche pas un outil de déclarer des arguments, chemins ou sorties dans sa télémétrie structurée : ces événements restent soumis au masquage, y compris leurs clés JSON. Les appels internes ne sont ni interceptés ni autorisés par ce parseur.

Une panne de collecte après une fin déjà auditée conserve le résultat métier dans le résumé JSON et renseigne `postprocessing_error`, avec un code CLI en erreur. Elle n'invente ni nouveaux événements ni nombre de fichiers : `changed_files` reste inconnu si le parcours d'observation n'a pu finir.

Le diff de fichiers compare les empreintes SHA-256 de contenu avant/après. Il émet au plus **200 changements**, puis un total et un indicateur de troncature. Une modification de permissions seule n'est pas un changement de contenu. Si un scan échoue, le nombre de changements reste inconnu. CPU, mémoire instantanée, réseau et attribution par sous-agent ne sont pas mesurés actuellement ; les mesures demandées restent des écarts ouverts et une attribution individuelle exige une preuve propre. Une limite cgroup configurée n'est pas une mesure d'utilisation.

```sh
./target/release/agentsandbox verify-audit
```

Cette commande vérifie la chaîne locale via le gateway. Elle détecte une altération vérifiable de l'audit local, pas une réécriture intégrale par un administrateur capable de remplacer base et chaîne. Sauvegarde/restauration et rétention doivent être implémentées et qualifiées pour le périmètre annoncé. Signatures et ancrage externe sont des protections supplémentaires à évaluer selon la menace et les gates ; ils ne sont pas présentés comme actifs.

## Copie, bornes et export

La copie refuse les liens symboliques et liens physiques, les FIFO, périphériques et autres fichiers spéciaux. Sur Unix, l'ouverture de chaque composant utilise des descripteurs relatifs et `O_NOFOLLOW` ; aucun fichier n'est lu en suivant un lien symbolique du projet. Les répertoires/fichiers de secrets connus sont exclus avant lecture : `.git`, `.env`, `.env.*`, `.ssh`, `.aws`, `.azure`, `.gnupg`, `.kube`, `.agentsandbox`, `secrets`, `mounted-secrets`, `credentials`, `.npmrc`, `.pypirc`, `.netrc`, clés privées usuelles, `.pem`, `.key`, `.p12`, `.pfx`, `.keystore`, ainsi que `node_modules`, `target`, `.venv` et `venv`. L'exclusion de `mounted-secrets` s'applique aussi lorsqu'un `--config-dir` personnalisé est placé dans le projet.

Il s'agit d'une liste de noms connus, pas d'un détecteur de contenu secret universel. Vérifier le projet transmis et ne pas y conserver des clés sous des noms arbitraires. La liste des exclusions est auditée avec un compteur ; le projet original n'est jamais modifié par cette préparation.

| Borne | Valeur |
|---|---:|
| Entrées importées/exportées, répertoires compris | 20 000 |
| Données de fichiers cumulées | 128 Mio |
| Un fichier | 32 Mio |
| Chemin relatif | 4 096 octets UTF-8 |
| Archive capturée pour un export | 128 Mio + 20 000 × 2 048 octets + 1 Mio |
| Import/export conteneurisé | 120 s chacun |
| Timeout métier configurable | 1 à 86 400 s |

Le dépassement d'une borne entraîne une erreur ; aucun résultat partiel n'est présenté comme un export complet. Les dépendances créées dans la session, notamment des caches ou des `node_modules` avec des liens, peuvent rendre le scan/export complet impossible. Cela ne modifie pas le projet original ; conserver le volume et examiner les résultats avec une procédure contrôlée.

```text
agentsandbox export-workspace --session UUID_DE_LA_SESSION --output /autre/repertoire/resultat
```

Le parent du répertoire de sortie doit déjà exister. La cible doit être absente et extérieure au projet original. L'export vérifie les chemins, rejette `..`, chemins absolus, formes Windows ambiguës, doublons, liens et fichiers spéciaux. Il extrait dans un répertoire temporaire privé avant publication sans remplacement. Aucun fichier préexistant de la destination n'est remplacé. Une archive hostile fait échouer l'opération et le répertoire temporaire est supprimé.

## Arrêt, interruption et conservation

```sh
./target/release/agentsandbox down --runtime docker
```

`down` arrête uniquement la stack Compose de contrôle et n'utilise jamais `-v`. Les volumes de session et d'audit sont conservés. Chaque worker d'import, de commande ou d'export est supprimé après son utilisation ; le volume reste disponible.

Un timeout ou Ctrl-C demande l'arrêt de **la seule session concernée**, puis son nettoyage. Il n'existe aucun `prune`, effacement global, suppression de projet original ou destruction implicite de volume. Si le daemon devient indisponible, le runner signale l'échec de nettoyage ; contrôler alors les conteneurs `agentsandbox-<UUID>-*` et intervenir uniquement sur les noms de cette session. SIGKILL du runner, panne du daemon ou arrêt du poste ne permettent pas de garantir un nettoyage immédiat.

Après export et vérification, un administrateur peut supprimer **un volume choisi explicitement** avec `docker volume rm "$VOLUME_NAME"` ou son équivalent Podman, après avoir fixé `VOLUME_NAME` au nom exact conservé dans `worker.attempt.volume_name`. Les nouvelles sessions utilisent un nom attribué par le contrôle ; l'ancien nom `agentsandbox-work-<UUID>` concerne les sessions historiques. Cette suppression est irréversible ; aucun automatisme actuel ne la réalise. Une politique de rétention ne doit jamais supprimer un volume, une preuve ou une donnée existante sans autorisation appropriée.

## Consulter, arrêter et reprendre un worker

Utiliser l'identifiant `session_id` du résultat de `run`, avec la même configuration privée et la même origine gateway. Par exemple, après avoir défini `SESSION_ID` avec cet UUID :

```sh
agentsandbox worker-status --session "$SESSION_ID"
agentsandbox worker-stop --session "$SESSION_ID"
agentsandbox worker-resume --session "$SESSION_ID" --wait-seconds 30
```

Ajouter `--config-dir` et `--gateway` si les valeurs locales par défaut ne sont pas celles de la session. Ces commandes appartiennent au contrôle de confiance, avec son credential runner ; elles ne sont pas des API d'exécution accessibles depuis le dashboard. `worker-status` consulte l'état durable et ne le présente pas comme une nouvelle observation du moteur.

`worker-stop` écrit un intent durable avant de demander l'arrêt. Si le broker possède encore son verrou, il traite cet intent ; le résultat peut confirmer la demande sans prouver déjà le terminal. Consulter `state` et le message, puis reprendre la lecture jusqu'à une observation terminale. `worker-resume` prend le verrou de tentative, vérifie identité, origine, autorité et preuves locales, puis réconcilie la même instance. Une reprise avant premier start peut exécuter la commande déjà admise ; un claim consommé ne donne jamais un second start. Un accusé ambigu peut donc laisser `in_doubt`, avec quota retenu et explication.

Le JSON conserve le résultat connu même lorsque la CLI retourne en échec pour code de sortie non nul, timeout, nettoyage non confirmé ou post-traitement échoué. Après interruption, les sorties attachées indisponibles restent inconnues ; aucune sortie vide ni durée zéro n'est fabriquée. Un code CLI zéro pour une consultation/réconciliation ou un intent d'arrêt ne signifie pas à lui seul que le worker est terminé. L'export reste réservé à une session terminale durable vers une destination neuve.

**Limite ouverte :** le timer du broker courant ne survit pas à sa mort brutale. La reprise manuelle et le quota conservé ne remplacent pas la supervision autonome de durée attendue du produit. Le mécanisme Rust durable est le prochain chantier P1 ; ne pas exploiter ce lot comme une garantie de durée après crash.

## Jev optionnel

Le gateway démarre sans Jev et sans accès sortant via le réseau Compose interne. L'activation requiert URL HTTPS, modèle explicite, clé distincte et réseau sortant ajouté **au gateway seulement**. Placer la clé existante dans `.agentsandbox/secrets/jev` avec le mode `0600`, puis relancer `init`. Celui-ci prépare la projection `mounted-secrets/jev` en `0444` dans le répertoire privé, sans jamais générer de clé Jev. Si la source facultative est retirée, le prochain `init` retire sa projection devenue obsolète. `deploy/docker/compose.jev.example.yaml` monte cette projection ; il fonctionne comme exemple avec la base Docker ou Podman, sous réserve des capacités du fournisseur Compose.

L'override ne crée pas de filtrage egress. L'activation peut transmettre les données minimales de décision documentées à un service externe. Vérifier le contrat, le modèle et la politique de l'équipe avant de configurer ces valeurs. Les commandes de base `agentsandbox up` ne chargent pas automatiquement cet override : après `init`, l'opérateur doit passer explicitement les deux fichiers à Compose avec `up --detach --force-recreate` pour recharger les montages et les clés. Les refus locaux ne sont jamais élargis par Jev ; l'indisponibilité ou une réponse non concluante de Jev refuse le lancement.

## Vérifications sur le poste cible

```sh
cargo test --locked
AGENTSANDBOX_TEST_RUNTIME=docker \
AGENTSANDBOX_TEST_CONFIG=/chemin/absolu/vers/.agentsandbox \
cargo test --test runtime_smoke -- --ignored --nocapture
```

Le test ignoré exige des images construites, un gateway déjà démarré et un moteur rootless. Il lance de vraies sessions, vérifie l'UID observé, une modification dans le volume, l'absence de modification du projet source et un export sûr. Il conserve les volumes de ces sessions comme le produit normal. Pour Docker Desktop, définir aussi `AGENTSANDBOX_TEST_ALLOW_ROOTFUL=1` après avoir choisi explicitement cette exception.

Compléter cette recette par des essais sur chaque moteur et OS ciblé : audit arrêté/plein, timeout, Ctrl-C, réseau none, quota mémoire/PID, fichier protégé absent du volume, rejet d'archive malveillante, arrêt du dashboard sans perte de collecte, redémarrage/persistance et fournisseur cloud choisi. Les tests unitaires Rust couvrent les décisions et les constructions d'arguments ; ils ne remplacent pas ces essais de confinement réel.
