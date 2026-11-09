# Exploiter ai-guard

## Ce que lance le produit

Le binaire `agentsandbox` est un programme Rust. Sur le poste de confiance, il appelle exclusivement la CLI `docker` ou `podman` avec un tableau d'arguments ; il n'exécute aucune commande métier dans un shell hôte. Sans moteur disponible, la commande échoue. Gateway et dashboard sont des conteneurs distincts sur un réseau interne. Un troisième conteneur de contrôle, `ingress`, publie uniquement leurs accès loopback via un relais Rust sans secrets, données ni socket montés. Lui seul rejoint aussi un bridge de publication. Les inspections conservées ne montrent aucune route par défaut IPv4 dans les deux services internes ; les autres canaux de sortie restent à qualifier. Les workers sont créés séparément pour chaque session et ne rejoignent jamais ces réseaux de contrôle/publication.

Le projet original n'est pas monté dans un conteneur. Le runner en prépare une archive filtrée, crée le volume propre à la session identifié dans le registre et importe l'archive via stdin dans un worker non privilégié. Les modifications restent dans ce volume. `export-workspace` écrit une copie vérifiée dans un **nouveau répertoire extérieur au projet original**.

Le mode normal exige un moteur rootless. Docker Desktop peut annoncer un daemon non rootless dans sa VM ; `--allow-rootful-runtime` autorise explicitement ce cas. La présence d'une VM n'est pas interprétée comme une preuve d'équivalence au mode rootless. Chaque session utilisant cette exception émet `runtime.security_downgrade`. Podman cible ici son fonctionnement rootless, avec `keep-id:uid=1000,gid=1000` pour les workers.

**Statut de validation :** consulter `docs/engineering/VALIDATION.md` et `docs/engineering/CURRENT_STATE.md` pour le candidat et les versions réellement vérifiés. Le relais publie les accès loopback sur Docker Desktop 4.97.0/Engine 29.9.2 macOS ARM64 avec exception rootful explicite. Confinement/copie/export/timeout, smoke avec refus audité et parcours navigateur sur cette pile ont passé. Ils ne qualifient ni les quatre combinaisons plateforme/moteur, ni le scénario à trois workers répartis entre deux clients, ni le cloud. Les tests `runtime_smoke` et `runtime_confinement` sont ignorés par défaut et doivent être lancés explicitement ; leurs exécutions réelles sont consignées séparément.

## Prérequis

Choisir le mode d'identité séparément du fournisseur IA : [accès local nominatif](setup/identity/README.fr.md), [SSO Web/CLI OIDC avec PKCE](setup/sso/README.fr.md), ou credentials techniques runner/viewer pour leurs rôles de contrôle et de lecture. Le guide SSO décrit les paramètres effectivement pris en charge, la découverte Keycloak et le raccordement à un fournisseur existant ; le [contrat d'identité](sso.md) et les preuves d'ingénierie distinguent les exigences complètes des parcours qualifiés. Utiliser des versions correspondantes du gateway, du dashboard et du CLI. Aucun realm, compte ou accès distant n'est supposé ; le mode local hors ligne reste indépendant d'un IdP.

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

### Programme et cible du moteur

Le broker ne recherche pas Docker ou Podman dans le `PATH` ambiant. Il vérifie des emplacements d'installation connus ; une installation personnalisée demande `--runtime-program /chemin/absolu/vers/docker` (ou Podman). Le programme, ses liens et sa cible doivent rester hors du projet importé. Le fichier exécutable et son répertoire final doivent appartenir à root ou au compte de contrôle, sans écriture groupe/autres ; les ACL non qualifiées sont refusées. Un refus de permissions n'autorise aucun repli vers une autre commande du projet.

L'enfant reçoit un environnement reconstruit et son répertoire de CLI ancré, avec les chemins source/build/Compose absolus. Les variables ambiantes `DOCKER_HOST`, `DOCKER_CONTEXT`, les sélecteurs Podman et les variables de chargement natif ne sont pas reprises. Les configurations et plugins du compte de contrôle restent des dépendances de confiance ; cette règle ne qualifie pas leur contenu ni leurs mises à jour.

Docker utilise une cible Unix explicite via `--runtime-endpoint`, ou le contexte du compte OS lu par le programme approuvé. Les opérations conservent cette cible et vérifient l'identité du moteur avant les effets. Modifier le contexte entre diagnostic et action ne doit pas déplacer l'action. Les cibles SSH/TCP ne sont pas prises en charge par ce parcours.

L'invocation approuvée est enregistrée dans les nouvelles sessions. Un changement du programme, de son contenu ou de la politique d'invocation refuse les effets. Pour une session ancienne ou une mise à jour du client moteur, `worker-resume`, `worker-stop` et `export-workspace` proposent `--adopt-runtime-program`, avec `--runtime-program` si nécessaire. Cette adoption vérifie en lecture seule la même autorité moteur, exige l'audit durable et conserve worker, instance et claims consommés. Elle ne redémarre pas une instance déjà lancée. Une panne d'audit refuse l'effet ; conserver le checkpoint et reprendre après correction. `worker-status` reste disponible sans invocation moteur. Une ancienne seed Podman dépourvue de toute autorité vérifiable n'est pas adoptée par un bootstrap nouveau.

Ce parcours fait l'objet du chantier de validation courant ; les preuves nommées dans [VALIDATION.md](engineering/VALIDATION.md) définissent les versions et runtimes effectivement testés.

Le dashboard est disponible à `http://127.0.0.1:4000`. Ouvrir localement `.agentsandbox/secrets/viewer` et saisir son contenu dans l'écran de connexion. La CLI n'imprime jamais les clés. Le jeton runner reste réservé au broker ; le dashboard reçoit uniquement le jeton viewer et ne propose aucune exécution de commande.

`init` crée des secrets indépendants de 32 octets aléatoires, encodés en hexadécimal. Un second `init` conserve les valeurs existantes. Les répertoires de configuration, de secrets, de projections de montage, de sessions et d'approbations ont le mode `0700` et appartiennent à l'utilisateur courant. Un répertoire existant avec un autre propriétaire, des permissions différentes ou un lien symbolique est refusé, afin que l'opérateur corrige consciemment son emplacement ou ses permissions.

Les originaux `.agentsandbox/secrets/runner` et `.agentsandbox/secrets/viewer` restent en **`0600`**. Ils ne sont jamais montés dans les services. Les anciens originaux `0444` sont ramenés à `0600` sans rotation ni modification de leur contenu. Chaque secret doit être un fichier régulier, appartenir à l'utilisateur courant, ne pas avoir de lien physique ou symbolique et contenir une valeur valide bornée.

Les fichiers Compose Docker et Podman montent à la place `.agentsandbox/mounted-secrets/runner` et `viewer`, en lecture seule. Ces **projections séparées** sont `0444` dans un répertoire hôte `0700`. Le gateway reçoit les deux fichiers ; le dashboard reçoit uniquement `viewer`. Le répertoire parent, les originaux et les autres secrets ne sont pas montés. Les autres utilisateurs hôtes ne peuvent pas traverser ce parent privé. Ne pas déplacer les projections dans un dossier public ou les exposer par un autre montage.

Cette séparation traite le décalage d'UID : en [Docker rootless, les fichiers de l'utilisateur hôte sont généralement vus comme appartenant à root dans le conteneur](https://docs.docker.com/engine/security/rootless/uid-gid-mapping/), alors que les services s'exécutent en UID 1000. Pour une source de secret de type fichier, [Compose ne peut pas appliquer les remappages `uid`, `gid` et `mode` au bind mount](https://docs.docker.com/reference/compose-file/services/#secrets). Les projections possèdent donc réellement leur permission de lecture nécessaire, sans passer le service en root. Le démarrage effectif des services UID 1000 a constaté leur lisibilité sur Desktop 4.97.0/Engine 29.9.2 ; les autres moteurs, mappings rootless et politiques SELinux restent à qualifier séparément.

`init`, et donc `up`, rafraîchit les projections par création temporaire `0600`, passage à `0444` puis remplacement atomique relatif au descripteur du répertoire protégé. Un processus utilisant déjà l'ancien inode continue à lire l'ancienne projection. **`up` recrée les services de contrôle** avec `--force-recreate` pour remonter les nouveaux inodes et recharger les clés au démarrage ; les workers et les volumes ne sont pas supprimés. Après une modification manuelle des sources, utiliser `up` avant de lancer de nouvelles commandes.

**Répéter `up` redémarre le gateway et le dashboard, même sans rotation de clé.** Les connexions HTTP/SSE sont interrompues ; l'audit persiste dans son volume. Une session active peut échouer à sa prochaine écriture d'audit pendant cette recréation : `up` n'est donc pas sans effet sur un job en cours. Pour une rotation, attendre la fin des jobs, remplacer la valeur du fichier source privé en conservant son mode `0600`, puis relancer `up` avec les mêmes options de runtime. Attendre ses contrôles de disponibilité avant un nouveau job et reconnecter le dashboard avec le nouveau jeton viewer si celui-ci a changé.

### Relais HTTP : corps et échanges

L'ingress dispose de deux budgets indépendants : **16 corps de requête à lire** et **64 échanges avec le service amont**. Chaque corps est limité à **256 Kio**, avec **5 secondes au total** et **2 secondes sans nouveau fragment** à partir de sa lecture. Le corps complet est lu avant de réserver un échange ; le permis de lecture est libéré avant l'appel au service. Un corps connu vide par son état de flux passe sans permis de lecture, indépendamment de la méthode HTTP ou du `Content-Length` déclaré. Les routes et les destinations fixes ne changent pas ; l'authentification et les autorisations restent appliquées par le gateway ou le dashboard.

Un pool de lecture plein produit un HTTP **429** pour une requête ayant un corps à lire, avant tout appel au service. Un dépassement de délai de lecture produit **408** ; un corps trop grand produit **413**. Le pool des échanges peut aussi produire 429, et les services amont conservent leurs propres refus. Distinguer ces causes par le message retourné et consulter les mêmes commandes Compose `ps`/`logs --tail 100 gateway dashboard ingress` que pour un diagnostic de disponibilité, en vérifiant les logs avant partage. Ces capacités et délais sont fixés dans le binaire ; aucune variable `.env` ou option CLI ne les règle.

Un upload incomplet ne réserve donc plus un échange nécessaire au GET de suivi d'un worker. Les requêtes complètes et les flux SSE peuvent encore occuper les 64 échanges : cette séparation n'est ni une protection générale contre les floods, ni une qualification de confinement runtime. Après démarrage acquitté, le moniteur de lecture du runner direct tolère certaines erreurs transitoires pendant au plus 60 secondes, avec visibilité dégradée et échéance initiale prioritaire. Les erreurs d'authentification, d'identité, d'audit et les HTTP 503 non classés demandent toujours l'arrêt ciblé immédiat. Voir le [contrat de suivi](#suivi-de-lecture-après-démarrage-acquitté) et ses limites de reprise avant toute nouvelle tentative de lancement.

## Registre des workers et changement de version

Les bases neuves utilisent le schéma de contrôle v2. Une base v1 existante reste lisible pour l'audit, mais les nouvelles opérations workers sont refusées avec `503 migration_required`. `up` ne migre pas silencieusement les données. Prévoir une maintenance avant d'utiliser le nouveau broker : arrêter les brokers et les services de contrôle, et vérifier qu'aucune ancienne session n'est encore en activité. Le flag de confirmation ci-dessous est une attestation opérateur, **pas un mécanisme de fencing**.

Le durcissement des fichiers de contrôle ajoute aussi une preuve privée d'exclusion de la configuration lors de la préparation d'une archive. Une archive ancienne sans cette preuve reste conservée et consultable ; elle ne peut plus préparer un **premier démarrage**, même si un ancien accusé était favorable. Ne pas modifier le checkpoint pour ajouter ce marqueur, ni réécrire l'archive sous le même claim. Créer une nouvelle session après réconciliation de la réservation ancienne. Un start déjà consommé permet toujours observation/arrêt/terminal, sans second start. Ce changement ne migre aucune table SQLite et ne libère pas un quota incertain par supposition.

Les sources de credentials et les fichiers privés du broker doivent être réguliers, à lien unique, appartenir au compte de contrôle et être0600 ; les racines sont0700 et leur chaîne de parents ne doit pas être modifiable par un autre UID. Les ACL non supportées, symlinks arbitraires et filesystems non supportés sont refusés sans modifier les fichiers. Les alias système macOS `/tmp`, `/var`, `/etc` sont reconnus ; pour un chemin contenant `..`, fournir sa forme absolue sans traversée. Les snapshots préparés restent sous la configuration, pas dans TMPDIR. Même si cette configuration utilise un nom personnalisé dans le projet, sa racine physique et les snapshots résiduels reconnus sont exclus de la copie worker. Les données arbitraires contenant des secrets restent à examiner avant partage : ce filtre n'est pas une DLP générale.

### Preset batch et journal moteur

Les nouveaux lancements du broker utilisent `fixed_sandbox_v2`, avec journal moteur désactivé et configuration inspectée avant claim/start. Les helpers ont aussi `--log-driver none`. Les captures explicitement demandées au broker restent bornées et masquées ; elles ne proviennent pas de `docker logs`. Le [lot qualifié sur Docker Desktop](engineering/evidence/2026-10-30/engine-logs-v2/README.md) conserve ses limites de plateforme.

Une session historique v1 non démarrée ne reçoit plus de premier start ; conserver ses preuves et réconcilier sa réservation plutôt que modifier le preset/checkpoint. Une session v1 déjà partie reste observable, arrêtable et finalisable, sans second démarrage. Cette compatibilité ne prouve pas rétrospectivement l'absence de journal pour les anciens workers.

Mettre à jour ensemble gateway et broker avant de revendiquer la garantie v2. Ne pas revenir à un ancien binaire qui ne reconnaît pas les nouveaux presets après leur admission : sa lecture/reprise peut être refusée. Une sauvegarde conservée ne justifie ni retour automatique des claims ni suppression des nouveaux enregistrements pour contourner ce refus. La restauration sûre avec fencing reste non qualifiée.

### Répertoire SQLite privé

Le stockage exige désormais un répertoire parent dédié, appartenant à l'UID effectif avec mode `0700`, et une chaîne de parents non modifiable par un autre utilisateur. Le fichier principal et les fichiers présents `-wal`, `-shm`, `-journal` doivent être réguliers, à lien unique, appartenir au même UID et être `0600`, sans ACL non admise. Un répertoire absent est créé privé ; un emplacement existant non conforme est refusé sans réparation automatique. Les nouveaux volumes initialisés depuis l'image de contrôle héritent du mode `0700` de `/data` ; reconstruire l'image ne modifie pas un volume existant.

Le **nom final du fichier SQLite est limité à ASCII** pour éviter les alias de noms réservés sur les filesystems dont la comparaison Unicode diffère. Les noms de répertoires peuvent rester Unicode. Les suffixes `-wal`, `-shm`, `-journal` et le préfixe `.ai-guard-sqlite-` sont réservés, quelle que soit la casse ASCII. Le verrou d'initialisation séparé est propre au répertoire et son attente est bornée à5secondes. Ne pas supprimer ce verrou pour contourner une erreur. Une base existante avec nom non ASCII reste conservée : préparer une migration hors ligne contrôlée, avec sauvegarde SQLite vérifiée et conservation de ses fichiers associés, avant de choisir un nouveau nom. Le produit ne la renomme pas automatiquement.

Avant une mise à jour depuis un répertoire `0755`, arrêter le gateway **et tous les brokers/consommateurs de cette base** ; suspendre leurs redémarrages automatiques et vérifier cet arrêt. Relever l'UID, les modes, les ACL, les liens et le filesystem de l'emplacement réel. Conserver une sauvegarde SQLite cohérente et la tête d'audit de référence. Si le seul écart est le mode du répertoire, son propriétaire peut appliquer explicitement `chmod 0700 /chemin/absolu/prive` pendant cet arrêt. Ne pas utiliser `chmod -R`, ne pas suivre un lien arbitraire et ne pas changer des propriétaires, ACL ou fichiers sans investigation. Un mode de fichier non conforme peut signaler une exposition antérieure ; réduire ses permissions ne suffit pas à qualifier son historique. Redémarrer ensuite avec le même tenant, vérifier intégrité et continuité de la tête avant toute admission. Cette maintenance ne supprime ni donnée, ni sidecar, ni credential.

SQLite conserve la propriété de ses descripteurs actifs : ouvrir puis fermer le fichier principal depuis un autre descripteur du même processus peut relâcher ses verrous POSIX. Le contrôle des noms utilise les métadonnées et la vérification VFS `HAS_MOVED`, sans conserver de descripteur brut supplémentaire du fichier principal ou SHM. Cela ne protège pas contre le compte de contrôle lui-même, root ou un administrateur du stockage. Voir les limites décrites par [SQLite sur la corruption et les verrous](https://www.sqlite.org/howtocorrupt.html) et [WAL](https://www.sqlite.org/wal.html). La qualification Linux, les autres UID et les pannes doivent conserver leurs preuves distinctes.

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

La cible est explicite avec `--target-version 2`, `3` ou `4` ; le défaut reste `2` pour conserver les commandes existantes. Chaque étape exige exactement la version précédente et sa propre destination de sauvegarde neuve :1→2 registre,2→3 supervision,3→4 projections d'identité humaine et permissions. Une migration ne saute pas de version et ne répare pas un schéma endommagé. Pour une source déjà en3, pendant le même arrêt vérifié du contrôle :

```sh
agentsandbox migrate-control \
  --data /chemin/prive/audit.sqlite \
  --tenant-id local \
  --backup /chemin/prive/audit-avant-identites-v4.sqlite \
  --target-version 4 \
  --confirm-control-stopped
```

Mettre à jour ensemble les lecteurs gateway/broker avant d'ouvrir une base4 ; un ancien binaire refuse cette version. Le namespace, la liaison tenant, les événements et les claims consommés sont préservés. Ni migration ni réouverture ne réactivent une autorité inactive. Le stockage4 ne crée aucun compte, attribution rétroactive, credential humain ou connexion OIDC ; les clés techniques gardent leur classe et leur périmètre. Conserver tous les backups et reçus ; une restauration n'est jamais un raccourci pour rejouer un lancement.

Après redémarrage, vérifier l'intégrité, l'identité, le quota et l'absence d'erreur de migration avant une nouvelle admission. La restauration d'une sauvegarde v2 avec des claims déjà consommés **n'est pas qualifiée** : elle peut ressusciter un droit de démarrage si elle est employée sans inventaire/fencing/réconciliation. Ne pas relancer des sessions à partir d'une ancienne sauvegarde comme si les moteurs étaient vierges. La procédure complète reste un gate ouvert.

Le mode local réserve par défaut trois workers, 6 000 milliCPU et 3 Gio au total, avec une durée admise maximale de 3 600 secondes. Les options gateway et la configuration par client sont dans [CLOUD_TENANCY.md](engineering/CLOUD_TENANCY.md). Le dashboard fixe reste inchangé à l'usage. Pour partager une présentation entre clients, lancer `dashboard --shared-tenants` vers le gateway configuré, sans `--viewer-token-file` ni secret monté dans ce dashboard ; les identités viewer sont vérifiées côté serveur. Les Compose livrés restent mono-client.

Les politiques Kubernetes de session V2 se configurent uniquement dans les entrées tenant du `--tenants-file` existant, y compris en mono-tenant : `session_reservation_policy`, `session_recovery_policy` et `session_control_recovery_policy`. Absente ou `null`, chaque option reste désactivée ; liste vide ne donne aucun droit. Les types fermés/version2, TTL1–60secondes, listes distinctes d'au plus64entrées et plafond canonique16384octets sont validés avant ouverture des bases, sans réécriture du registre ni rechargement à chaud. Conserver quotas et politiques de capacité d'audit distincts. Les [guides Kubernetes EN](setup/kubernetes/README.md#configure-session-reservation-v2-policies) / [FR](setup/kubernetes/README.fr.md#configurer-les-politiques-de-réservation-de-session-v2) donnent le fragment public, la migration explicite10→11 hors ligne avec sauvegarde neuve et attestation d'arrêt, et les limites d'exécution ; la migration par défaut reste2 et n'active rien.

La récupération contrôle exige le runner technique partagé du tenant et une permission explicite pour le broker créateur historique. Elle borne Observe/Close/Lookup à une réservation avant effet ; elle ne donne ni Reserve, CREATE, start ou identité humaine. L'acteur `control_shared_runner` reste distinct du créateur broker. Close exige l'audit durable et débite le budget workload historique broker/tenant ; une panne ou saturation conserve le quota retenu. Reconfigurer explicitement une capacité dans la marge physique avant construction/partage d'un nouveau Store ne remet aucun compteur à zéro. Changer la politique ou le secret runner sur disque ne révoque pas un gateway vivant : arrêter, retirer/drainer le contexte puis construire son remplaçant. Conserver les opérations et reçus exacts ; aucun effacement d'audit, libération sans trace ou délai physique de drainage n'est autorisé par ces options.

### Pénurie transitoire de descripteurs d'audit

Lorsqu'une vérification des fichiers de la base conserve une erreur OS typée `EMFILE` ou `ENFILE`, le Store refuse l'opération sans accusé de réussite ni autorisation d'exécution pour cette requête. Lorsqu'une route du gateway reçoit cette erreur, elle la traduit en HTTP **503** ; une pénurie de descripteurs peut aussi empêcher l'acceptation d'une connexion ou la transmission de la réponse. L'origine de supervision vivante et les connexions sœurs qui la partagent restent suspendues. Ce traitement est limité à ces deux causes lors du contrôle des fichiers ; une erreur d'IO quelconque n'est pas considérée récupérable.

Une fois les descripteurs disponibles, le prochain accès autorisé qui atteint la validation du Store lance automatiquement une vérification complète sur un même snapshot SQLite : identité des fichiers, schéma, liaison tenant, métadonnées et quota, chaîne d'événements, projections de contrôle et témoin vivant contre le recul de l'audit. Cette origine retrouve son autorité seulement si tous ces contrôles réussissent. Une connexion sœur peut effectuer cette réconciliation pour l'origine partagée. Une simple lecture de `/healthz` ne la déclenche pas ; reprendre une lecture autorisée dans le même périmètre, sans créer une nouvelle session pour sonder la reprise.

Le commit SQLite peut avoir réussi avant l'échec du contrôle final des fichiers : **une réponse en erreur ne prouve donc pas l'absence de mutation durable**. Conserver l'identité de l'événement ou de l'opération et son contenu exact. Utiliser le chemin de rejeu idempotent de cette opération lorsqu'il existe : il retrouve le résultat durable sans ajouter un autre événement ni débiter à nouveau le quota. Créer une nouvelle identité ou répéter un lancement comme une nouvelle session peut produire une autre opération. Le témoin du commit reste conservé ; un ancien état de base ne peut pas servir à récupérer l'autorité, et un claim de démarrage consommé reste consommé.

Une erreur `EIO`, une identité substituée, une corruption ou un verrou de supervision empoisonné maintiennent l'invalidation permanente de l'origine. Ni l'apparente remise en état des fichiers, ni une actualisation, ni `worker-resume` ne la réarment ; aucune option opérateur ne force cette réactivation. Préserver les données et les références de session pour diagnostiquer la cause. Cette reprise des descripteurs ne prouve ni l'état du moteur, ni l'arrêt d'un worker, ni la disponibilité d'un fournisseur. Les observations runtime restent distinctes d'un HTTP 503, d'un état de santé ou d'une chaîne d'audit valide ; les [preuves datées](engineering/VALIDATION.md) délimitent la qualification effectivement obtenue.

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

Sans terminal, la demande est conservée dans `.agentsandbox/pending/<hash>.json` et aucun conteneur n'est lancé. Répéter les **mêmes options**, et le même argv pour une commande manuelle, avec `--approve-hash <hash>` reprend ces IDs. Un changement de projet, d'image, de réseau, de runtime, de fournisseur, de timeout ou de commande invalide cette reprise. Le fichier est privé et contient les arguments exacts ; ne placer aucune clé d'API dans l'argv ou le prompt. Une session possédant déjà son checkpoint durable exige `worker-resume`, sans second lancement par `run`.

Pour une sélection fournisseur gérée, le plan3 conserve les options demandées, la résolution et le binding privé préparés avant décision. L'approbation ne relit ni `providers.json` ni le fichier de credential et ne résout pas Auto à nouveau. La reprise avant premier démarrage exige les mêmes octets que ceux admis ; un snapshot modifié bloque ce démarrage. Les plans1/2 gardent leur sens historique et ne sont pas convertis en plans3. Un claim de démarrage consommé ne peut pas être réarmé.

Une autorisation ne rend pas les dépendances disponibles hors ligne. Les workers génériques incluent Bash, Git, Python 3, Node/npm, curl, les outils de compilation système et la toolchain Rust ; ils n'incluent pas les dépendances propres à chaque projet. Préparer une image avec les dépendances approuvées, ou utiliser observe + bridge pour une session qui en a explicitement besoin.

## Agents cloud et clés d'API

### Image et sélection gérée

Le checkout construit un bootstrap Rust `/usr/local/bin/ai-guard-provider` dans l'image worker. Le candidat `agentsandbox-worker:qa-managed-providers-20261030-ccgpe3` a passé build et sondes versions/aides/bootstrap sans credential sur Docker Desktop/macOS ARM64 ; ces contrôles ne qualifient pas l'authentification. L'image QA précédente et le binaire des essais manuels restent des artefacts historiques. L'image générique n'installe aucun CLI fournisseur par défaut. Leur installation exige une **version sémantique exacte** sélectionnée par l'opérateur :

```text
agentsandbox build --runtime docker --codex-version X.Y.Z --claude-code-version A.B.C
```

`X.Y.Z` et `A.B.C` sont des espaces réservés, pas des versions testées. `latest`, `*`, les plages et les options supplémentaires sont refusés. Les paquets utilisés sont `@openai/codex` et `@anthropic-ai/claude-code`, documentés dans leurs sources officielles : [Codex](https://github.com/openai/codex) et [installation Claude Code](https://code.claude.com/docs/en/setup#install-with-npm). Les versions exactes doivent être contrôlées contre la documentation et les exigences du fournisseur choisi.

Préparer un fichier privé régulier `0600`, possédé par le compte de contrôle, sous une chaîne de répertoires admise. Le format API utilise des lignes littérales `NAME=value`, sans `export`, interpolation ou `source`. Le chemin géré accepte le `.env` racine explicitement référencé dans le projet : la copie l'exclut. Les autres sources internes au projet sont refusées par le runner. La configuration fournisseur elle-même doit être extérieure au projet ou dans la racine physique du contrôle exclue de la copie.

```sh
agentsandbox providers init --config-dir /chemin/prive/controle \
  --env-file /chemin/projet/.env
agentsandbox providers status --config-dir /chemin/prive/controle --json
agentsandbox run --config-dir /chemin/prive/controle --project /chemin/projet \
  --profile observe --network bridge --timeout 120 \
  --client claude-code --auth-mode api --client-profile claude-api \
  --model claude-haiku-5-5 --prompt "Inspecte le projet sans modifier les fichiers"
```

Ces commandes sont une procédure à qualifier sur l'image construite depuis ce checkout, pas la reproduction du PASS Claude historique. Ajouter les options runtime/image propres à l'installation. Les prochains essais Claude utilisent explicitement `claude-haiku-5-5`, sans alias ni basculement silencieux.

Une invocation ultérieure du chemin géré Claude/API batch a passé sur l'image candidate ARM64 : un fichier exporté exact, original intact, terminal/quota libéré et audit36 valide après restart gateway. [Rapport et commande](engineering/evidence/2026-10-30/provider-auth/managed-claude/real-run.md). Le modèle demandé est `claude-haiku-5-5`, le modèle servi n'est pas attesté. Le preset n'ajoute pas `--max-budget-usd` : `--timeout` borne le worker, pas les dépenses fournisseur. Ce résultat ne qualifie pas l'ensemble des modes, comptes ou plateformes.

`providers init` crée `providers.json` sans lire le fichier de credentials, sans écrasement et sans login. Il crée deux profils API : `codex-api` référence `OPENAI_API_KEY`, `claude-api` référence `ANTHROPIC_API_KEY`, avec la priorité Auto déclarée Codex puis Claude. `providers status` affiche les profils, modes et types de source, jamais leur chemin privé ni leur valeur ; `credentials_verified` et `worker_capabilities_verified` restent faux. Le défaut est `<config-dir>/providers.json` ; `providers status --providers-file` et `run --providers-file` choisissent un autre fichier privé.

`--client codex|claude-code|auto`, `--auth-mode api|subscription|auto`, `--client-profile`, `--model` et `--prompt` pilotent la résolution. Un prompt est obligatoire en batch et facultatif en TTY. Le choix CLI explicite doit correspondre au profil sélectionné ; `--model` remplace le modèle du profil. Sans profil explicitement sélectionné, Auto considère les profils abonnement configurés avant les profils API, puis le défaut compatible et l'ordre `auto_priority` départagent. Une ambiguïté sans priorité est refusée. Un abonnement sélectionné manquant ou refusé ne déclenche aucune tentative API. Une variable ambiante ou un CLI hôte ne sélectionne aucun profil.

Le choix non secret est affiché avant décision ; une API est annoncée comme facturée par le fournisseur. `--client-profile` désigne un profil fournisseur, distinct de `--profile observe`. Les credentials exigent toujours observe + bridge explicites. Les options gérées ne se combinent pas avec un argv manuel, `--secret-env-file` ou un normaliseur `--provider` non générique. Voir [la configuration et les limites fournisseur](engineering/PROVIDER_AUTH.md#état-livré-et-limites--30-octobre-2026-après-revue).

### Abonnements, diagnostic et conservation

Les profils `codex_auth_file` et `claude_oauth_env_file` référencent respectivement un `auth.json` ChatGPT explicitement préparé et un fichier contenant `CLAUDE_CODE_OAUTH_TOKEN`. Leurs credentials doivent avoir été obtenus volontairement par les mécanismes officiels, dans un contexte isolé. Aucun cache OAuth/keychain hôte n'est exploré ou modifié ; un assistant opérateur `auth login/import/status` n'est pas livré. Ne pas configurer un profil abonnement indisponible en espérant un repli API.

Le bootstrap ne transmet que le matériel du client/mode choisi : clé Codex/API, `auth.json` Codex/abonnement, clé Claude/API ou token Claude/abonnement. Il prépare un home privé temporaire, retire son payload interne de l'environnement enfant et impose les options du preset. Les versions/aides du client sont sondées avec bornes et délais après admission, avant l'appel modèle. `doctor` reste un diagnostic moteur ; aucun `doctor providers` autonome avant admission n'est livré. Une sonde d'aide réussie ne prouve pas l'authentification ou les effets du preset.

Les plans3, `credential-bindings/` et les checkpoints sont privés. Le binding contient le bootstrap sélectionné et son dictionnaire de masquage ; son digest n'est pas public. Une rotation du fichier source n'altère pas une autorisation en attente : préparer une nouvelle demande pour un nouveau credential. Sauvegarder ces éléments avec la même confidentialité que les secrets ; ne pas les publier, modifier ou supprimer pour forcer une reprise. La rétention automatisée, le refresh OAuth et le masquage de tokens renouvelés restent des écarts ouverts. Une suppression ou un changement du binding bloque un nouveau premier démarrage ; les opérations de lecture/arrêt ne créent pas de nouvelle authentification.

### Chemin manuel et preuves historiques du 30 octobre 2026

Le chemin avancé reste `--provider codex|claude|claude-code|generic --secret-env-file /chemin/prive/agent.env -- argv...`. Il conserve son sens de normalisation, sans résolution d'authentification rétroactive. Son fichier de credentials doit être extérieur au projet ; il injecte toutes les variables acceptées, donc préparer un fichier dédié. Les variables de chargeur/exécuteur (`PATH`, `HOME`, `LD_PRELOAD`, `NODE_OPTIONS`, etc.), `JEV_API_KEY` et `JEV_API_KEY_FILE` sont refusées avant snapshot. Les collisions par valeur avec les credentials du contrôle sont également refusées. Ne jamais transmettre le `.env` combiné par ce chemin.

Codex/API batch reconnaît `CODEX_API_KEY` ([documentation non interactive](https://learn.chatgpt.com/docs/non-interactive-mode)) ; Claude/API reconnaît `ANTHROPIC_API_KEY` ([variables Claude Code](https://code.claude.com/docs/en/env-vars)). Ces mécanismes et le bootstrap ne gardent pas la clé secrète face au code worker.

Codex CLI `0.165.0` a réellement créé un fichier et permis son export neuf, avec le modèle demandé `gpt-6-luna`, sur Docker Desktop/macOS ARM64. Les [preuves](engineering/evidence/2026-10-30/provider-cli/README.md) distinguent la tentative initiale et le parcours corrigé. Choisir un modèle disponible pour son propre compte ; ce test ne garantit pas l'accès de tous les comptes à ce modèle.

```text
agentsandbox run --project "/chemin/projet" --config-dir "/chemin/prive/controle" \
  --image agentsandbox-worker:local --profile observe --network bridge \
  --provider codex --secret-env-file "/chemin/prive/codex.env" --timeout 300 -- \
  codex exec --json --model gpt-6-luna --sandbox danger-full-access \
  -c 'approval_policy="never"' --ephemeral --skip-git-repo-check \
  "Create only result.txt with the exact text: Codex local smoke test completed. Do not edit any other path or run tests."
```

L'image choisie doit contenir la version exacte installée, et les options du moteur nécessaires au poste doivent être ajoutées comme dans le [quick start](quickstart.md). Le `CODEX_API_KEY` reste dans le fichier privé, jamais dans la commande. Le mode interne `danger-full-access` est prévu par OpenAI pour un conteneur contrôlé : ici les restrictions ai-guard (UID, caps, seccomp, rootfs readonly, copie/volume et limites) assurent le confinement. Les actions internes restent observées, sans médiation universelle. Ce mode n'est pas une recette pour exécuter Codex directement sur l'hôte.

Sur cet environnement, le mode interne `workspace-write` a terminé avec exit0 mais sans créer le fichier : `bwrap` a reçu un refus de création de namespace utilisateur. Aucun sysctl ou privilège Docker n'a été modifié pour le corriger. Vérifier l'effet réel, le diff et l'export ; le code de sortie du client ne suffit pas à prouver sa tâche accomplie. Le quick start contient la vérification exacte du contenu exporté.

Sur l'ancienne image QA combinée (Claude `2.1.140`, Codex `0.165.0`), un essai **manuel Claude/API batch a passé** avec deux fichiers exportés vérifiés et audit cohérent. Il utilisait l'alias `haiku`, sans identifiant complet résolu établi : aucun PASS Haiku5.5 n'en est déduit. La tentative manuelle Claude TUI reste **FAIL de qualification par instrumentation** après démarrage ; aucune réponse du premier tour ni conversation à deux tours n'est prouvée. Le wrapper a ensuite été vérifié hors ligne, sans retry fournisseur. Au gel build/sondes de la nouvelle image bootstrap, les quatre combinaisons natives **gérées** sont **NOT_RUN et non qualifiées** ; voir [VALIDATION.md](engineering/VALIDATION.md).

### Terminal interactif et limites

Le canal interactif exige de vrais stdin/stdout/stderr de terminal et un terminal de contrôle au premier plan ; les pipes/redirections et `--capture-output` sont refusés avant admission/import. Il ne donne aucun socket moteur ou cache d'authentification hôte au worker. Le mode appartient à l'autorisation durable : une session batch ne se convertit pas. Ctrl-C tapé est destiné au client distant ; Ctrl-X deux fois demande un arrêt du worker, pas un détachement durable. Une perte du transport demande arrêt/inspection ; un résultat inconnu conserve son quota et ses preuves. `worker-resume --interactive` ne peut premier-démarrer qu'une session TTY autorisée sans claim consommé ; le réattachement après claim consommé est explicitement refusé. Employer les commandes existantes observation/arrêt pour réconcilier cet état.

Le broker restaure les modes du terminal à ses sorties contrôlées. SIGKILL ne permet pas d'exécuter cette restauration et aucun timeout autonome n'est encore fourni après mort du broker. Pour récupérer manuellement un terminal perturbé, lancer `stty sane`, puis la remise en présentation ci-dessous dans ce terminal ; cela n'arrête pas un worker. Vérifier séparément son état via `worker-status`/`worker-stop` depuis un autre terminal fiable. Les séquences sont des constantes de récupération du contrôle, jamais du contenu worker à rejouer :

```sh
stty sane
printf '\033[?2004l\033[?1049l\033[r\033[?25h\033[0m'
```

Les valeurs ne sont pas placées dans l'argv, l'image ou le projet. Le runner prépare une copie temporaire privée et transmet son chemin via `--env-file`. Seul le worker métier la reçoit ; les helpers d'import/export n'ont aucune clé. Les valeurs connues sont masquées localement avant l'envoi des événements, puis le gateway applique son masquage avant persistance. **L'agent et le code exécuté dans ce worker peuvent lire leurs variables d'environnement.** Ce mécanisme ne les protège pas d'un agent malveillant ; la protection des credentials du périmètre annoncé reste à implémenter et qualifier lorsqu'elle exige de les garder hors du worker. Le daemon et l'administrateur hôte restent aussi des autorités de confiance : l'environnement d'un conteneur peut être accessible à l'opérateur du runtime via inspect.

`--network bridge` est accepté uniquement avec `--profile observe`. Il émet un avertissement et `network.unrestricted`. Il permet un accès sortant **sans filtrage applicatif**, y compris l'exfiltration possible de données volontairement confiées à ce worker. Le produit ne présente ni ce réseau ni les logs fournisseurs comme un proxy de sécurité universel.

En batch, stdin métier est fermé ; en TTY, le terminal de contrôle est transmis par le transport autorisé. Le home temporaire disparaît avec le conteneur. Le bootstrap installe le credential préparé avant le client ; il ne fournit pas d'assistant de connexion interactif ni de persistance OAuth hôte. Le transport TTY générique et les appels batch historiques ne qualifient pas une conversation native Codex/Claude ou les abonnements.

## Audit, sorties et couverture

Le runner attend un accusé de stockage durable avant toute action conteneurisée : `command.requested`, décision centrale, éventuelle approbation, import audité et `runtime.submitted`. Une décision sans `audit_seq`, un événement sans `seq`, une réponse malformée, un HTTP d'erreur ou un gateway inaccessible bloque la suite. L'observation durable du démarrage et l'accusé de `command.started` restent obligatoires ; leur callback ne bénéficie pas de la fenêtre de suivi ci-dessous. Sans observation terminale, `runtime.execution.error` laisse le résultat inconnu.

### Suivi de lecture après démarrage acquitté

Sur le parcours runner direct (`run` et `worker-resume`), seul le rafraîchissement en lecture du registre après les accusés Running et `command.started` peut réessayer une interruption de connexion typée, un délai de transport borné ou un HTTP 429. La fenêtre maximale de **60 secondes commence au début du premier poll défaillant**, timeout de cette requête inclus. L'échéance d'exécution initiale reste prioritaire ; le contrôle conserve une borne conservatrice dès l'appel au runtime. Réessais et `Retry-After` ne renouvellent aucune durée, autorisation, réservation ou claim. Pendant cet intervalle, la visibilité du contrôle est dégradée/inconnue ; la poursuite du worker ne prouve pas un suivi sain.

Avant le premier GET, l'armement durable de l'epoch après ces accusés est borné à **15 secondes**, ou à l'échéance d'exécution si elle arrive avant. La borne commence avant de lancer la création et inclut son attente et sa synchronisation disque. Son expiration demande le STOP ; elle ne donne pas une fenêtre supplémentaire de 60 secondes en l'absence de GET.

STOP observé, annulation et échéance préemptent les attentes HTTP ou de journal. Une erreur d'authentification, de permission, d'identité, de révision, de format ou d'audit demande l'arrêt ciblé immédiatement ; un HTTP 503 sans classification temporaire explicite reste dans ce cas. La reprise de visibilité exige une réponse complète de la requête courante, pour le même worker et la même tentative, sans recul des révisions, puis un enregistrement durable de reprise. Le résultat et le quota restent inconnus sans leurs preuves terminales et leur transaction centrale.

Le journal privé séparé `worker-control-monitor/<session>.json` conserve l'epoch, les identités d'écriture et au plus **64 intervalles**, dans **128 Kio**. Cette limite porte sur les octets du fichier avant décodage, espaces compris. Refus d'écriture ou capacité épuisée demande l'arrêt ; ne pas supprimer ou modifier ce fichier pour réarmer une fenêtre. Sa clôture après fin d'exécution ne retient pas la finalisation : une écriture encore en cours conserve son identité et reste non confirmée jusqu'à son commit. Annuler son attente ne prouve aucun rollback.

Après interruption du propriétaire, aucun `Instant` d'origine n'est recréé depuis une date murale. Une epoch ouverte, absente ou invérifiable, même après un dernier poll sain, ne donne pas une nouvelle fenêtre. Un `worker-resume` d'une instance encore active utilise ses preuves privées strictement liées au tenant/origin, worker/tentative, verrou et autorité runtime pour demander le STOP du **seul CID conservé avant de dépendre d'un gateway disponible**. Il n'adopte aucune autre autorité et ne libère pas le quota. Si l'arrêt est observé, le statut local indique toujours que terminal et comptage restent à réconcilier ; réexécuter la même reprise après rétablissement du gateway pour obtenir leur accusé durable. Une preuve locale incomplète ou un arrêt non confirmé conserve l'inconnu, sans cible déduite par son nom.

Si l'invocation historique doit être remplacée et que l'adoption est explicitement demandée, un candidat valide conserve le parcours d'adoption authentifié existant : le gateway doit être disponible, le même moteur doit être vérifié et l'adoption auditée avant l'arrêt du CID conservé. Le chemin local ne réalise jamais cette adoption. Un refus d'identité, un fichier non sûr ou un moteur divergent reste un refus, sans nouvelle permission.

Ce comportement ne qualifie ni un arrêt à temps réel après mort du processus, ni le gardien indépendant, ni Docker/Podman/Kubernetes. Les RPC du service broker conservent leurs propres exigences d'authentification fraîche ; cette fenêtre ne s'applique ni à un lancement, ni à un claim, ni au callback d'audit obligatoire.

Le worker métier terminé reste inspectable jusqu'à l'accusé d'enregistrement de `command.completed` ou `command.failed`. Un refus ou une perte de réponse à ce stade laisse le conteneur et son volume en place ; aucune nouvelle exécution n'est tentée. Après cet accusé, le nettoyage explicite cible la référence retournée par la création. La sortie JSON de `run` distingue le résultat métier (`exit_code`, `timed_out`) de `cleanup` (`container`, `state` = `removed` ou `unconfirmed`, `error`). Un échec de nettoyage conserve le résultat audité et fait sortir la CLI en erreur ; la présence du conteneur reste inconnue si la réponse a été perdue après sa suppression. Le rapport `runtime.cleanup.failed` est tenté et `cleanup.error_audit` distingue `acknowledged`, `unacknowledged` et `not_applicable` lorsqu'il n'y a pas d'erreur à rapporter. Sans cet accusé, le résumé conserve le résultat terminal, les changements restent inconnus et aucun autre helper n'est lancé. Le registre durable, l'autorité moteur et `worker-resume` permettent une réconciliation manuelle de la même tentative sans rejouer un claim consommé ; ils ne remplacent pas une supervision autonome après mort du broker.

`command.started` est émis pendant l'exécution, après qu'un polling de `inspect` (environ 200 ms) a observé un `StartedAt` valide. Le lancement du processus CLI Docker/Podman ne sert pas de preuve de démarrage. Pour une commande très courte, l'inspection finale conserve cette preuve et la date du runtime dans `data.runtime_started_at`.

Les faits du contrôle passent par `POST /api/v1/control/events`, réservé au runner, avec un budget de débit distinct de l'ingestion des fichiers et des déclarations. Un ancien gateway sans cette route refuse un nouveau `run` avant lancement ; aligner les versions du CLI et du gateway, sans contourner le refus. L'audit obligatoire de démarrage reste à une tentative de 15 secondes par POST : son refus demande l'arrêt ciblé, sans fenêtre de retry d'événements. Le [contrat API](api.md#ingestion-et-faits-du-contrôle--ingestion-and-control-facts) distingue ces refus des quotas et de la capacité durable de l'audit.

Le runner draine stdout/stderr pour ne pas bloquer le processus. Il conserve au plus **256 Kio par flux**. La persistance du texte brut est désactivée par défaut. `--capture-output` l'active, avec un maximum de **8 Kio par flux** dans le résultat ; les secrets connus sont masqués avant cette troncature. Les nombres d'octets et les indicateurs de troncature restent présents. Les erreurs retournées par le parcours d'exécution sont expurgées avant affichage, y compris leur chaîne de causes. Le masquage repose sur les clés connues et des motifs reconnaissables ; aucun détecteur ne peut garantir l'élimination de tout secret arbitraire inconnu ou encodé.

Les normaliseurs Codex/Claude/generic traitent le stdout retenu **après la fin** du worker. Ils produisent au plus **500 événements** par session, toujours en provenance `declared` pour les déclarations des outils. Les lignes inconnues ou dépassant 64 Kio sont ignorées. Le JSON borné est décodé puis expurgé avant les projections tronquées, afin de conserver une télémétrie valide même avec des chaînes échappées. Le bilan `observer.coverage` rend ces limites explicites. Désactiver le texte brut n'empêche pas un outil de déclarer des arguments, chemins ou sorties dans sa télémétrie structurée : ces événements restent soumis au masquage, y compris leurs clés JSON. Les appels internes ne sont ni interceptés ni autorisés par ce parseur.

Une panne de collecte après une fin déjà auditée conserve le résultat métier dans le résumé JSON et renseigne `postprocessing_error`, avec un code CLI en erreur. Elle n'invente ni nouveaux événements ni nombre de fichiers : `changed_files` reste inconnu si le parcours d'observation n'a pu finir.

Dans le parcours direct `run`/`worker-resume`, le scan fiable et les événements de fichiers précèdent les déclarations du worker ; un refus de ces déclarations ne supprime pas le bilan de fichiers déjà acquitté. Le préfixe de `file.*` est limité à 200 événements et au budget global de 3 Mio du journal privé, avec une réserve pour les diagnostics et la couverture. `workspace.diff.completed` indique le total observé, les changements émis/omis, la troncature et ses raisons ; le préfixe peut être plus court avec de longs noms échappés. Les chemins sont masqués ; un UUID opaque distingue chaque changement et les collisions d'affichage du préfixe sont signalées, sans conserver le nom sensible.

`observer.coverage` distingue la collecte déclarée et le diff fiable. `file_delivery` version1 donne la phase, le nombre préparé et le nombre d'ACK durables reçus. Après perte de réponse, un événement peut être dans la base sans ACK local. `postprocessing_complete` concerne les données retenues et bornées : il ne promet ni couverture exhaustive des outils ni absence de troncature. Après interruption sans capture retenue, les déclarations perdues ne sont pas recréées et leurs compteurs restent inconnus. Une couverture refusée reste en attente locale ; un audit indisponible ne peut garantir un marqueur central.

Conserver `.agentsandbox/sessions/<session>.postprocessing-v1.json` avec le plan et le volume. Ce journal privé versionné retient les événements exacts et leurs ACK sous le verrou de tentative ; il n'ajoute pas de champ aux plans stricts 1/2/3 et ne prouve pas la terminaison du worker. Un journal invalide, trop grand ou inaccessible fait échouer la reprise ; ne pas l'éditer ou le supprimer pour obtenir un succès. Sa publication et celle du plan sont deux écritures distinctes, sans garantie d'atomicité globale, de réserve disque ou de résistance à une restauration ancienne. Les [guides locaux EN](setup/local/README.md#reconcile-the-recorded-worker)/[FR](setup/local/README.fr.md#réconcilier-le-worker-enregistré) décrivent la reprise. Le service broker permanent conserve son outbox distincte.

Le diff de fichiers compare les empreintes SHA-256 de contenu avant/après. Il émet au plus **200 changements**, puis un total et un indicateur de troncature. Une modification de permissions seule n'est pas un changement de contenu. Si un scan échoue, le nombre de changements reste inconnu. CPU, mémoire instantanée, réseau et attribution par sous-agent ne sont pas mesurés actuellement ; les mesures demandées restent des écarts ouverts et une attribution individuelle exige une preuve propre. Une limite cgroup configurée n'est pas une mesure d'utilisation.

```sh
./target/release/agentsandbox verify-audit
```

Cette commande vérifie la chaîne locale via le gateway. Elle détecte une altération vérifiable de l'audit local, pas une réécriture intégrale par un administrateur capable de remplacer base et chaîne. Sauvegarde/restauration et rétention doivent être implémentées et qualifiées pour le périmètre annoncé. Les points de contrôle signés et l'export vérifiable du [contrat de traçabilité](sso.md#chaîne-de-traçabilité-à-livrer) sont obligatoires, à implémenter et qualifier ; `verify-audit` ne les livre pas. Un ancrage externe éventuel reste à évaluer selon la menace et les gates ; aucune de ces protections n'est présentée comme active.

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

À l'écriture, l'export applique la [politique détaillée](setup/local/README.fr.md#examiner-et-exporter-les-changements) : noms sensibles et métadonnées d'activation omis, variantes de casse et certains alias Unicode comparés, structures de répertoire Git refusées avant publication. Les fichiers réguliers publiés sont en `0644`. Le manifeste contient les fichiers et modes écrits ; le scan conserve les modes observés, avec couverture inconnue pour les anciens baselines sans mode. Les entrées filtrées restent validées et comptées dans les bornes. L'avertissement générique indique les omissions, sans les noms ; son échec refuse la publication. Les événements `workspace.export.completed` du CLI et `workspace.export.policy` du broker conservent le nombre de headers omis ; une absence historique ne vaut pas zéro. L'accusé IPC V1 et l'ancien fait `broker.batch.export` conservent leur schéma fermé. Le contenu reste non fiable, notamment `.cargo/config.toml`, `Makefile` et `package.json` encore présents : examiner les fichiers avant d'autoriser configuration, dépendances ou exécution.

## Arrêt, interruption et conservation

```sh
./target/release/agentsandbox down --runtime docker
```

`down` arrête uniquement la stack Compose de contrôle et n'utilise jamais `-v`. Les volumes de session et d'audit sont conservés. Le nettoyage d'un conteneur de session suit les acquittements durables requis ; un conteneur peut rester présent si l'audit ou le nettoyage n'est pas confirmé. Consulter `worker-status` et son état `cleanup` avant d'annoncer sa suppression. Le volume de travail reste disponible.

Le code courant du gateway, dashboard et ingress prévoit un drainage HTTP de cinq secondes **après** SIGINT/SIGTERM, sans limite de vie au démarrage. Un flux SSE encore ouvert peut être interrompu à l'expiration ; le client se reconnecte avec son dernier curseur pour récupérer les événements durables. Une déconnexion ne prouve pas qu'une décision a échoué ou qu'un export est complet : vérifier son état/contenu avant une reprise. L'enregistrement Unix des signaux est fallible et précède la readiness. [Recette locale](engineering/evidence/2026-10-30/http-shutdown/README.md) : arrêts SSE observés vers cinq secondes et arrêts au repos en 1 ms sur macOS ARM64. Blocage OS/IO/SQLite, autres environnements et configuration du gestionnaire de services restent à qualifier ; conserver un délai opérateur suffisant pour ce drainage. Ce délai HTTP n'arrête pas à lui seul un worker.

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

Pour une session terminale dont le post-traitement manque ou a échoué, `worker-resume` rejoue les faits préparés avec les mêmes IDs et contenus. S'il n'existe pas de batch préparé, il peut rescanner le volume conservé avec l'autorité enregistrée et marque cette observation comme une reprise après la fin. Il ne rejoue ni commande métier ni conversation fournisseur. Un ancien résultat présent avec `changed_files` inconnu ne suffit pas à établir le diff. La réparation des fichiers peut réussir tout en laissant une erreur pour capture déclarée irrécupérable ou diagnostic d'exécution externe ; une seconde reprise ne transforme pas cette perte en succès. `worker-status` expose le journal de post-traitement lorsqu'il existe, sans en déduire une nouvelle observation runtime.

**Limite ouverte :** le timer du broker courant ne survit pas à sa mort brutale. [Défaut reproduit sur Docker Desktop](engineering/evidence/2026-10-30/cli-crash/README.md) : même worker encore Running après7s malgré une limite5s, puis arrêt/réconciliation ciblés. La reprise manuelle et le quota conservé ne remplacent pas la supervision autonome attendue. Le port d'arrêt sur snapshot est durci ; broker permanent, gardien et reprise autonome restent à intégrer et qualifier.

Les procédures ci-dessus emploient le schéma2 livré par défaut. Les API v1 de supervision et un bootstrap explicite de Storev3 neuf existent ; le raccord au broker permanent reste à livrer. L'activation fraîche ne survit volontairement pas à la fermeture du processus, et une réouverture/restauration conserve la maintenance et le refus d'exécution. Aucun fresh bind d'ancien Created ne lui donne une nouvelle durée ou un start. Conserver bases/plans/volumes et réconcilier l'autorité avant la future réactivation qualifiée ; aucune édition manuelle du schéma ou d'un flag ne la remplace. [Preuves Store](engineering/evidence/2026-10-30/supervision-store/README.md), [bootstrap neuf](engineering/evidence/2026-10-30/supervision-bootstrap/README.md).

### Initialiser un contrôle de supervision neuf

Cette étape prépare les API du broker permanent. Le parcours actuel de `run`/`worker-resume` conserve le schéma2 : ces clients ne fournissent pas encore les armements v3. Utiliser des bases distinctes et neuves pour le nouveau contrôle ; le composant journal et le bootstrap ne constituent pas une supervision autonome.

L'option gateway `--initialize-supervised-stores BACKUP_ROOT` exige que chaque fichier DB soit absent, crée une sauvegarde2 vérifiée et engage le schéma3 dans cette seule origine. Elle fonctionne avec un contrôle local ou un registre multi-client existant. Conserver les fichiers de clés privés déjà configurés : ils ne sont ni recréés ni déplacés.

```sh
agentsandbox gateway \
  --data /chemin/neuf/controle-local/audit.sqlite \
  --runner-token-file /chemin/controle/secrets/runner \
  --viewer-token-file /chemin/controle/secrets/viewer \
  --initialize-supervised-stores /chemin/prive/supervision-backups
```

Pour un registre, utiliser `--tenants-file /chemin/controle/tenants.json` avec cette même option et des destinations `data` toutes neuves. Le JSON du registre ne change pas. La racine de backup est privée0700, dédiée, et séparée des répertoires de clés runner/viewer/Jev connus ; les aliases ordinaires du même répertoire sont refusés. En mode bootstrap, les parents des fichiers de clés doivent être privés0700. Les fichiers de sauvegarde0600 sont nommés `tenant-<SHA256 du tenant exact>-v2.sqlite` ; ce nom ne constitue aucune authentification.

`GET /api/v1/control/protocol`, avec l'identité authentifiée habituelle, expose tenant, rôle, namespace, schéma et état du contrôle. `active=true` décrit l'autorité fraîche du Store et ne prouve pas qu'un ordonnanceur fonctionne. Les [tests HTTP A/B et limites](engineering/evidence/2026-10-30/supervision-bootstrap/README.md) vérifient cette différence.

Au redémarrage ordinaire sans l'option, les bases3 et leurs namespaces sont conservés en maintenance inactive. Réutiliser l'option sur ces fichiers refuse le démarrage ; elle n'est pas une commande de reprise/restauration. Une erreur empêche serve et conserve les nouveaux partiels pour diagnostic. Le bootstrap de plusieurs bases n'est pas globalement atomique : si un tenant ultérieur échoue, les nouveaux fichiers antérieurs restent en place. Garder ces fichiers et les backups ; aucune suppression ou réparation automatique ne débloque le contrôle. La qualification de panne électrique, la réactivation sûre et les parcours workers restent ouverts.

## Jev optionnel

Le gateway démarre avec le moteur Rust déterministe, sans Jev et sans accès sortant via le réseau Compose interne. Ce moteur n'offre pas une compréhension sémantique exhaustive. Le client Jev réel optionnel utilise le contrat TypeSafe ; le petit modèle local Jev-like demandé reste un chantier distinct, non livré par cette configuration. Aucun de ces moteurs ne peut lever un refus impératif, une erreur d'audit ou une restriction tenant/runtime.

L'endpoint configuré est `https://api.typesafe.ai/v1/systemone`, avec modèle explicite `jev-latest`. L'adaptateur emploie une question `choice` nommée `execution_policy`, attend un choix reconnu et applique son seuil de confidence ; le [contrat API local](api.md#adaptateur-jev-réel-optionnel) détaille les champs et les refus. Références officielles : [System One](https://docs.typesafe.ai/api/system-one), [Choice](https://docs.typesafe.ai/primitives/choice), [modèles](https://docs.typesafe.ai/models). Aucun header `TypeSafe-Danger-Zone-Long-Context` n'est envoyé par défaut. `jev-latest` est un alias évolutif ; conserver le modèle effectivement retourné, sans déduire une calibration propre à ai-guard du seul seuil.

La clé est distincte de toutes les identités runner/viewer et reste dans le contrôle. Dans un éditeur privé, placer **la clé brute seule**, sans `JEV_API_KEY=` ni autre variable, dans `<config-dir>/secrets/jev`, fichier régulier `0600`, puis relancer `init` avec le même `--config-dir`. Le gateway lit ce fichier par `JEV_API_KEY_FILE` ou `--jev-key-file` ; il ne charge pas le `.env` combiné, ne fait pas de `source` et n'accepte aucun secret dans l'argv. Les noms `JEV_API_KEY` et `TYPESAFE_API_KEY` peuvent désigner la même clé de compte dans les documentations ; cela ne crée pas d'alias d'environnement automatique dans le produit. Ne transmettre aucun de ces credentials au worker métier ou à un modèle local.

`init` prépare la projection `mounted-secrets/jev` en `0444` dans le parent privé `0700`, sans jamais générer de clé Jev. Si la source facultative est retirée, le prochain `init` retire sa projection devenue obsolète. `deploy/docker/compose.jev.example.yaml` monte cette projection et requiert les réglages non secrets `JEV_URL=https://api.typesafe.ai/v1/systemone` et `JEV_MODEL=jev-latest` ; dans le gateway, `JEV_API_KEY_FILE=/run/secrets/jev`. Il s'applique avec la base Docker ou Podman, sous réserve des capacités du fournisseur Compose.

L'activation exige un réseau sortant ajouté **au gateway seulement**. L'override ne fournit pas de filtrage egress et peut transmettre à TypeSafe les données minimales de décision expurgées. Vérifier leur portée avant activation. `agentsandbox up` ne charge pas automatiquement cet override : après `init`, passer explicitement la base et l'override au même Compose, avec `up --detach --force-recreate`, pour recharger montages et clés. L'indisponibilité, une réponse malformée ou une confidence insuffisante de Jev refuse le lancement ; aucun fallback de moteur ne doit être supposé. Cette procédure et les sources officielles ne constituent pas une preuve d'appel réel depuis cette stack.

## Vérifications sur le poste cible

```sh
cargo test --locked
AGENTSANDBOX_TEST_RUNTIME=docker \
AGENTSANDBOX_TEST_CONFIG=/chemin/absolu/vers/.agentsandbox \
cargo test --test runtime_smoke -- --ignored --nocapture
```

Le test ignoré exige des images construites, un gateway déjà démarré et un moteur rootless. Il lance de vraies sessions, vérifie l'UID observé, une modification dans le volume, l'absence de modification du projet source et un export sûr. Il conserve les volumes de ces sessions comme le produit normal. Pour Docker Desktop, définir aussi `AGENTSANDBOX_TEST_ALLOW_ROOTFUL=1` après avoir choisi explicitement cette exception.

Compléter cette recette par des essais sur chaque moteur et OS ciblé : audit arrêté/plein, timeout, Ctrl-C, réseau none, quota mémoire/PID, fichier protégé absent du volume, rejet d'archive malveillante, arrêt du dashboard sans perte de collecte, redémarrage/persistance et fournisseur cloud choisi. Les tests unitaires Rust couvrent les décisions et les constructions d'arguments ; ils ne remplacent pas ces essais de confinement réel.
