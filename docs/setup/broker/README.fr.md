# Broker local de confiance

[English](README.md) · [Index de configuration](../README.fr.md) · [Installation locale](../local/README.fr.md) · [Fournisseurs IA](../providers/README.fr.md)

Le broker Rust permanent possède les jobs batch dans le périmètre de contrôle de confiance. Un client soumet une demande par IPC local privé ; quitter ce client n'annule pas un job accepté. Le gateway, le dashboard et les workers restent séparés. Ni un worker ni le dashboard ne reçoivent le socket moteur, le registre du broker ou un credential runner.

Ce manuel décrit le contrat du produit complet et les commandes implémentées dans la CLI actuelle. Ce n'est pas un rapport de qualification. Consulter [l'implémentation et les écarts actuels](../../engineering/CURRENT_STATE.md), [la validation](../../engineering/VALIDATION.md) et [les gates de production](../../engineering/PRODUCTION_READINESS.md) avant de choisir une installation. En particulier, le prérequis de contrôle actif ci-dessous interdit de considérer le démarrage Compose ordinaire comme une installation complète du broker permanent.

## Préparer le périmètre de confiance

Utiliser un compte de contrôle Unix avec un binaire `agentsandbox` déjà construit, une installation Docker ou Podman choisie délibérément, les images worker préparées et un accès au gateway correspondant. Suivre le [guide local](../local/README.fr.md) pour construire les images et préparer les credentials privés. Docker et Podman sont des runtimes distincts ; un résultat sur l'un ne qualifie pas l'autre.

Le broker s'exécute sur le poste ou l'hôte de contrôle de confiance de la plateforme. Le gateway et le dashboard restent conteneurisés. Ne pas installer le broker dans un worker applicatif, monter le projet source dans un worker ou donner le moteur aux conteneurs applicatifs. Le broker exécute le programme moteur approuvé avec des arguments séparés ; les commandes métier s'exécutent dans la copie de travail du worker.

Avant de démarrer le service, préparer ces valeurs d'installation :

| Valeur | Exigence |
|---|---|
| Binaire et programme moteur | Chemins absolus du binaire relu et de l'exécutable moteur approuvé. L'identité, le contenu et les permissions du répertoire de l'exécutable sont vérifiés ; le `PATH` ambiant ne constitue pas une approbation. |
| Registre | Nouveau fichier JSON régulier privé, `0600`, dans un répertoire privé `0700` appartenant au compte de contrôle. Aucun lien ou ACL non prise en charge. Maximum 256 Kio. |
| `root` du broker et `config_dir` | Chemins absolus dédiés et bornés, hors du projet. Préserver l'état existant. Leurs répertoires privés et les identités retenues sont vérifiés. |
| Origine gateway | Origine réellement autorisée : HTTPS, ou HTTP loopback pour une installation locale délibérée, sans credential, chemin, querystring ou fragment. |
| Tenant et namespace | Véritables `tenant_id` et `control_namespace_uuid` renvoyés par le contrôle authentifié, comme indiqué ci-dessous. Les chaînes du registre sont des attentes, pas des droits d'identité. |
| Fichiers runner | Sources de credentials privées `0600` existantes, avec parents privés. Référencer leurs chemins ; ne jamais afficher leur contenu ou le placer dans les arguments d'une commande. |
| Endpoint moteur | URL explicite de socket Unix local absolu pour Docker, par exemple `unix:///absolute/engine/docker.sock`. Podman accepte aussi son endpoint explicite `local`. Utiliser le compte/contexte prévu pour les jobs. |
| Racines source et export | Répertoires autorisés existants, avec propriétaire, permissions et ascendance sûrs. Les arbres source et export sont disjoints. Les racines d'export sont aussi physiquement séparées du stockage broker/configuration. Éviter les racines larges contenant des données de contrôle. |

Le fonctionnement rootless est l'exigence normale. `allow_rootful_runtime: true` est une exception délibérée pour une installation qui en a besoin, y compris certaines configurations Docker Desktop qui ne déclarent pas de fonctionnement rootless. Ce n'est pas une équivalence d'isolation. Garder `network: "none"` sauf si un profil de job relu séparément exige un accès sortant.

## Exiger un contrôle compatible et actif

Chaque requête authentifie de nouveau son credential **présenté** avec `GET /api/v1/control/protocol`. Le broker exige `role: "runner"`, le tenant et le namespace configurés, `schema_version` 3, 4, 5, 6 ou 7 et le protocole `broker_supervision_v1`. Submit, approve, resume et export exigent aussi `active: true`. Status, list, recovery-status et cancel peuvent s'authentifier auprès d'un contrôle compatible inactif ; leurs autres dépendances et vérifications de sûreté restent applicables. Un même UID Unix donne accès au transport, pas une authentification tenant.

Lire les vrais tenant, namespace, rôle et état de supervision avant de rédiger le registre. Remplacer le binaire, l'origine et le chemin de credential privé par les valeurs de l'installation :

```sh
"/absolute/path/to/agentsandbox" broker control-info \
  --gateway "http://127.0.0.1:8787" \
  --token-file "/absolute/private/ai-guard/credentials/runner"
```

Cette lecture ne nécessite ni registre ni broker démarré. Elle affiche les métadonnées validées du contrôle en JSON, dont `schema_version`, `active` et les `protocols` disponibles. Un succès confirme une lecture authentifiée des métadonnées ; vérifier séparément les prérequis de contrôle actif ci-dessus. Un credential viewer peut lire les métadonnées de son tenant avec le rôle `viewer`, tandis que le broker exige `runner`. Le schéma2 possède déjà un namespace et est annoncé inactif. Authentification, fichier de credential non sûr et réponse incompatible/indisponible produisent un code non nul ; le credential n'est jamais affiché. Si une métadonnée contient ce credential, la commande refuse la sortie au lieu de réécrire l'identité.

Ne pas deviner le namespace, recopier un UUID de test, choisir un autre tenant dans le registre ou coller un runner dans le dashboard. Le `runner_token_file` configuré sert à la réconciliation autonome du broker ; il ne remplace pas l'authentification d'une requête client. Le `--token-file` du client désigne le credential présenté pour cette requête.

Les fichiers Compose locaux standards démarrent le contrôle schéma2. `/healthz` établit seulement la disponibilité du service. Ni une stack saine ni `init` n'établissent le protocole de supervision ou une autorité d'exécution fraîche. Le [bootstrap exclusif existant pour des bases de contrôle supervisé neuves](../../operations.md#initialiser-un-contrôle-de-supervision-neuf) utilise l'option gateway `--initialize-supervised-stores BACKUP_ROOT`, avec destinations de bases absentes, sauvegardes privées vérifiées et fichiers de credentials existants. Il doit être intégré au déploiement conteneurisé du contrôle par l'opérateur ; ce guide broker ne fournit pas de profil Compose schéma3.

Une limite essentielle concerne la reprise : une réouverture ordinaire d'une base schéma3–7 conserve son namespace en maintenance inactive. Réutiliser l'option de bootstrap sur cette base refuse le démarrage. Une procédure de réactivation sûre n'est pas actuellement livrée. Préserver bases, sauvegardes et preuves runtime ; ne pas supprimer une base, modifier les métadonnées d'activation ou créer des identités de remplacement pour contourner cet état. `broker resume` réconcilie un job et ne peut pas réactiver le contrôle. La CLI actuelle n'a pas de commande `broker init`, `broker doctor` ou d'installation de service OS.

### Mettre à jour le stockage explicitement

Arrêter le gateway et tous les brokers/consommateurs de la base du tenant, y compris les redémarrages automatiques. `migrate-control --target-version` accepte la version suivante exacte2,3,4,5,6 ou7 ; son défaut reste2. Utiliser une sauvegarde neuve distincte pour chaque étape. Le schéma5 ajoute l'audit nominatif obligatoire des lectures, décrit dans le guide [accès local nominatif](../identity/README.fr.md). Le schéma6 ajoute bindings workload, registre des brokers, relais dirigé et plancher UTC durable ; suivre le [guide workload](../workloads/README.fr.md) pour la migration explicite5→6, les budgets d'audit protégés et les contrats HTTP par rôle. Cela ne raccorde pas les workers à un transport runtime et n'active pas la supervision. Pour une source existante schéma3 :

```sh
"/absolute/path/to/agentsandbox" migrate-control \
  --data "/absolute/private/ai-guard/audit.sqlite" \
  --tenant-id "<configured-tenant-id>" \
  --backup "/absolute/private/ai-guard/audit-before-identities-v4.sqlite" \
  --target-version 4 --max-events 100000 \
  --confirm-control-stopped
```

Mettre à jour ensemble les lecteurs gateway et broker avant d'ouvrir le schéma4. La sauvegarde vérifiée, les anciens événements, le namespace, la liaison tenant et les claims consommés sont préservés ; un ancien binaire refuse le nouveau schéma. Migration/réouverture n'activent pas une supervision inactive. Le schéma4 prépare le stockage d'identité humaine et de permissions ; il ne crée pas de compte, n'attribue pas les anciens événements à une personne, ne transforme pas une clé technique en credential humain et n'effectue pas de connexion OIDC. Conserver backups et rapports après un échec et enquêter avant de choisir une nouvelle destination. Voir la [maintenance hors ligne](../../operations.md#répertoire-sqlite-privé) pour la sauvegarde vérifiée et les limites de restauration.

### Lire les brokers enregistrés après migration7

Le schéma7 ajoute la lecture humaine du registre brokers existant du tenant par `GET /api/v1/human/brokers`. Suivre la [procédure exacte5→6→7 avec sauvegardes](../workloads/README.fr.md#activer-la-lecture-nominative-du-registre-brokers), y compris pour une base setup SSO en5. La migration utilise la même base tenant et ajoute son marqueur de capacité d'audit sans activer la supervision. Les lecteurs acceptant7 conservent la compatibilité5/6 ; mettre gateway, clients broker/STOP et dashboard à jour ensemble avant d'ouvrir7. Cela ne délivre aucun nouveau claim et ne contourne ni réouverture inactive ni écarts de fencing/reprise.

La vue **Brokers** exige la capacité administration explicitement accordée `read_broker_registry` ; le [guide nominatif](../identity/README.fr.md#autoriser-la-lecture-nominative-du-registre-brokers) couvre création, mutation du membership, retrait et reprise. Elle montre l'infrastructure du tenant indépendamment de l'accès aux ressources `read_tenant_metadata`, filtré par organisation. SSO exige membership et plafond configuré sur claims. Ne jamais envoyer de credential runner au navigateur.

Les lignes proviennent de vraies [opérations authentifiées binding/grant/heartbeat](../workloads/README.fr.md#autoriser-un-broker-et-interpréter-son-heartbeat). Pour publier la présence du service au premier plan, configurer explicitement `presence_file` dans son profil wire2 selon le [guide de présence](PRESENCE.fr.md) et l'[exemple typé](examples/presence.json). Les profils sans ce champ ne publient pas automatiquement. Un bail courant enregistre une déclaration du broker et sa réception par le gateway, pas une inspection runtime ou la santé des workers ; une entrée périmée/révoquée n'arrête pas un runtime et ne libère pas de quota. Préserver incarnation et heure de réception d'un ancien heartbeat pour le diagnostic.

Ce registre humain diffère de Registry2 privé ci-dessous et de la liste de jobs IPC `broker list`. Il n'expose ni grant brut ni ID/digest de credential, ne fournit ni export, fédération workers ou accès moteur au frontend. Un refus cumulatif d'audit exige de préserver base/politique et réparer la capacité avant une tentative explicite, sans supprimer l'audit ou réinitialiser le contrôle. Cette vue ne qualifie pas la production et ne réactive pas la supervision.

## Écrire le registre opérateur privé

`broker serve --config` est le seul lecteur de ce registre. La CLI ne charge pas de dotenv et n'y fusionne pas les réglages du shell. Le registre fixe runtime, image, politique, racines de fichiers et références fournisseurs ; un client de soumission ne peut pas remplacer ces contrôles. `--profile` sur les commandes clientes du broker sélectionne un `id` du registre, tandis que la politique de décision est son champ `policy_profile`.

Ce qui suit est un **template complet du registre wire version2** pour une nouvelle installation Docker. Remplacer toutes les valeurs `<...>` et toutes les références `/absolute/...` par celles de l'installation relue avant de le charger. Les deux placeholders UUID doivent devenir des UUID non nuls ; l'UUID d'installation est nouvellement attribué à cette installation, tandis que le namespace et le tenant viennent du véritable contrôle authentifié. Les chemins et l'image sont des références illustratives, pas des ressources installées. Aucune valeur de credential ne doit figurer dans ce JSON.

```json
{
  "version": 2,
  "installation_id": "<new-installation-uuid>",
  "root": "/absolute/private/ai-guard/broker-state",
  "max_enforcer_slots": 8,
  "profiles": [
    {
      "id": "strict-docker",
      "gateway": "<actual-gateway-origin>",
      "tenant_id": "<tenant-id-returned-by-control>",
      "control_namespace_uuid": "<namespace-uuid-returned-by-control>",
      "config_dir": "/absolute/private/ai-guard/config",
      "runner_token_file": "/absolute/private/ai-guard/credentials/runner",
      "runtime": "docker",
      "runtime_program": "/absolute/approved/docker",
      "runtime_endpoint": "unix:///absolute/engine/docker.sock",
      "image": "<prepared-worker-image-reference>",
      "policy_profile": "strict",
      "network": "none",
      "allow_rootful_runtime": false,
      "max_timeout_seconds": 30,
      "allow_capture_output": false,
      "managed_clients": false,
      "providers_file": null,
      "manual_credential_file": null,
      "source_roots": ["/absolute/data/projects"],
      "export_roots": ["/absolute/data/exports"],
      "prearmed_start_profile": {
        "version": 1,
        "mode": "docker_minimal"
      }
    }
  ],
  "additional_control_secret_files": []
}
```

Tous les objets registre/profil/profil de démarrage refusent les champs inconnus. Registry2 exige `prearmed_start_profile` dans **chaque** profil. Sa forme fermée contient exactement `version: 1` et `mode: "docker_minimal"` ou `mode: "legacy"`. Choix absent, version/mode/champ inconnu et `docker_minimal` avec Podman sont des erreurs, jamais des demandes de repli.

Registry1 conserve son schéma fermé initial et le comportement Legacy ; ne pas lui ajouter le nouveau champ. Registry2 peut sélectionner Legacy explicitement avec le **fragment suivant à l'intérieur d'un profil**, qui n'est pas un registre complet :

```json
"prearmed_start_profile": { "version": 1, "mode": "legacy" }
```

Pour une nouvelle configuration Docker, sélectionner explicitement `docker_minimal`, comme dans le template complet. Il donne au start Docker préarmé sa propre configuration cliente minimale privée au lieu de réutiliser silencieusement celle du Docker hôte. Le broker crée et lie ce matériel avant acceptation ; l'opérateur ne crée ni ne modifie ses fichiers internes. Ce choix ne retire ni autorisation centrale, ni audit obligatoire, ni contrôles moteur ou confinement, et ne promet pas le fencing d'un start déjà ambigu.

| Champ du registre | Sens et bornes |
|---|---|
| `installation_id`, `root` | Identité stable d'installation et racine privée d'état durable. Les remplacer ne reprend pas une installation existante. |
| `max_enforcer_slots` | 1–1024 cibles d'arrêt retenues, partagées par le service ; les helpers consomment aussi cette capacité. Ce n'est pas un réglage de nombre de workers qualifié. |
| `profiles`, `id` | 1–32 profils ; IDs uniques de 1–80 lettres/chiffres ASCII/`_`/`-`. |
| `policy_profile`, `network` | `strict`, `review` ou `observe` ; `none` ou `bridge`. Bridge est une connectivité sortante, pas une liste de destinations autorisées. |
| `max_timeout_seconds`, `allow_capture_output` | Durée de 1–86400 secondes ; la capture doit être activée ici pour qu'un client puisse la demander. Sans durée demandée, Submit utilise le minimum entre 300 secondes et cette limite. |
| `managed_clients`, `providers_file` | La sélection gérée exige `true` et un chemin absolu explicite de registre fournisseur. Voir le [guide fournisseur](../providers/README.fr.md). |
| `manual_credential_file` | Entrée facultative absolue et privée de credentials pour commandes manuelles ; `null` n'en fournit aucune. Ce n'est pas un fichier runner. La sélection gérée utilise plutôt son credential fournisseur sélectionné. |
| `source_roots`, `export_roots` | 1–8 chemins absolus bornés dans chaque liste, soumis aux contrôles d'autorisation et du filesystem physique. |
| `additional_control_secret_files` | Au plus 16 références absolues supplémentaires de credentials privés de contrôle pour exclusions et contrôles de secrets. Ni liste de credentials worker ni scanner universel de secrets. |
| `presence_file` | Chemin absolu privé facultatif vers le JSON Presence V1, profils wire2 seulement ; voir [autorité, stockage et reprise](PRESENCE.fr.md). N'émet pas de credential, n'accorde pas d'autorité et n'active pas la supervision. |

Tous les chemins de contrôle sont absolus, de 4096 octets et 64 composants au plus. Garder registre, credentials de contrôle, entrées fournisseurs et données broker hors des arbres source/export. Un credential de contrôle manquant ou dangereux ferme les nouveaux travaux métier ; emprunter la clé d'un autre profil ne le répare pas. La configuration est chargée au démarrage et les fichiers retenus sont revalidés. Modifier un registre vivant n'est pas un rechargement à chaud et peut faire échouer les requêtes.

Le choix du profil de démarrage ne sert qu'à une **nouvelle acceptation**. Une opération déjà Accepted, y compris en attente de Review, conserve son binding de lancement accepté lors du replay exact de Submit, de l'approbation et de resume. Changer la sélection du registre ne peut ni l'upgrader ni la downgrader. Les autres paramètres de profil figés, l'exécutable approuvé et son identité doivent toujours correspondre ; une divergence est un refus, pas un nouveau fingerprint. Registre wire2, profil de lancement version1 et bindings/Accepted persistants version2 sont des formats versionnés distincts. Ne pas en déduire qu'un ancien binaire lit les nouveaux jobs ou qu'un rollback est possible pendant que des jobs restent actifs.

## Démarrer le service au premier plan

Renseigner ces variables du shell avec l'installation préparée ci-dessus. Ce sont des arguments de commande, pas des réglages consommés automatiquement depuis l'environnement. La variable de credential contient **uniquement un chemin**.

```sh
AIGUARD_BIN="/absolute/path/to/agentsandbox"
AIGUARD_BROKER_CONFIG="/absolute/private/ai-guard/operator/registry.json"
AIGUARD_BROKER_ROOT="/absolute/private/ai-guard/broker-state"
AIGUARD_BROKER_PROFILE="strict-docker"
AIGUARD_RUNNER_FILE="/absolute/private/ai-guard/credentials/runner"
AIGUARD_PROJECT="/absolute/data/projects/your-project"
"$AIGUARD_BIN" broker serve --config "$AIGUARD_BROKER_CONFIG"
```

`serve` s'exécute au premier plan. Le conserver dans un contexte de contrôle géré délibérément et utiliser un autre terminal de confiance pour les clients. Il acquiert une lease broker, découvre ou démarre explicitement l'enforcer interne d'arrêt depuis le même binaire, restaure les cibles d'arrêt prouvées et publie un endpoint Unix privé. La découverte cliente ne démarre jamais un broker absent. Les enregistrements IPC et sockets sont des données d'implémentation possédées par le service ; ne pas les supprimer, remplacer ou adopter pour réparer un service occupé ou indisponible.

Cette commande n'installe aucune unité systemd ou tâche launchd et ne revendique aucune qualification après logout, reboot, gel vivant ou pour les échéances autonomes. Un endpoint périmé ou une lease occupée n'autorise pas de takeover. Le [contrat de supervision](../../engineering/BROKER_SUPERVISION.md) distingue ces obligations de la commande au premier plan implémentée.

L'authentification IPC privée du gardien appartient à l'état du broker et reste distincte des credentials runner/viewer. Une authentification existante est lue sans réécriture. Si elle manque dans un rôle possédant déjà un état, le démarrage refuse d'en générer une autre. Préserver la racine du rôle, son historique et les sauvegardes privées cohérentes ; récupérer l'authentification originale sous maintenance contrôlée. Effacer l'historique pour obtenir une racine apparemment vide n'est pas une procédure de reprise.

### Reprendre les arrêts autorisés avec admission fermée

Pour une installation existante qui doit uniquement restaurer ses cibles d'arrêt prouvées, démarrer explicitement le service restreint :

```sh
"$AIGUARD_BIN" broker serve --config "$AIGUARD_BROKER_CONFIG" --recovery-only
```

Ce mode exige les racines privées, verrous de rôle, authentification IPC et namespaces de découverte existants. Il n'initialise aucune nouvelle installation et ne régénère pas une authentification manquante. Une lease occupée ou un enforcer vivant incompatible reste un refus ; cette commande ne prend jamais le contrôle d'un processus existant et ne l'arrête pas. Préserver son état et utiliser sa connexion prise en charge lorsqu'elle est disponible.

Le service demande explicitement la capacité `StopsOnly` du protocole2. Il restaure uniquement les cibles prouvées par des lecteurs de journal retenus, conserve leur identité de conteneur et leur échéance originales, et retient les annulations durables pour les rejouer après reconnexion. L'admission reste fermée pendant toute la vie de ce service : Submit, approve, resume et export ne peuvent pas obtenir de capacité Full. Status, list, recovery-status et cancel conservent leurs dépendances normales d'authentification et de données ; ce mode ne réactive pas un gateway inactif.

Pour chaque lot de reprise, le broker retient les cibles validées dans sa capacité configurée avant d'attendre leurs confirmations individuelles de contrôle. La disparition du client attendant cette demande ne les efface pas tant que le broker fonctionne. Les cibles indépendantes continuent après l'échec d'une cible ; le résultat signale une couverture partielle. Une confirmation porte sur l'instance du gardien et la révision de cible acquittées à cet instant, pas sur un worker observé arrêté. Après un crash du broker, la reprise exige toujours le journal durable conservé.

Une réservation d'arrêt ou une annulation acceptée ne confirme ni effet runtime ni résultat terminal. Une couverture partielle du journal reste partielle. Le transport de reprise ne prouve pas le drainage d'un ancien démarrage, ne finalise pas un retrait et n'autorise pas un nouveau worker à remplacer une cible ambiguë. Des racines, authentifications, verrous ou sockets manquants ou dangereux nécessitent une récupération contrôlée de ces ancres exactes ; une nouvelle racine ou un nouveau credential ne les remplace pas.

### Transmettre l'audit terminal retenu

Le service restreint retente aussi les événements V4 existants
`observer.coverage` et `broker.batch.result` d'un worker dont le terminal est
prouvé. Configurer `runner_token_file` du profil avec son credential runner de
confiance. Chaque essai authentifie ce même client auprès du gateway configuré
et recoupe tenant, namespace, worker/tentative retenus et observation terminale.
Le worker central doit déjà être terminal, son quota libéré et cohérent. Un
contrôle inactif peut servir ces opérations d'audit authentifiées ; cela ne
réactive pas l'exécution.

Le broker transmet les événements retenus avec leurs IDs et contenus d'origine,
puis inscrit uniquement les séquences d'audit exactes reçues. Si le second envoi
échoue ou qu'un arrêt normal est observé après le premier reçu, il soumet le
sous-ensemble obtenu. L'autre événement reste en attente pour un retry borné
ultérieur. Un refus d'identité ou une incohérence terminale laisse les événements
sans ACK. Ce chemin ne fait aucune observation runtime, connexion fournisseur,
exécution, export ou suppression.
La transmission modifie uniquement les ACK de l'outbox locale ; elle ne finalise
pas le résultat géré et ne retire pas son cycle de supervision.

Le même budget HTTP initial de 45 secondes couvre identité, lecture du worker
et envois. La tâche détachée survit à la disparition de l'observateur de la boucle
tant que le processus et le runtime asynchrone restent vivants ; cela ne garantit
pas une reprise après coupure électrique ou mort du processus. Une erreur writer
n'est pas un ACK. Préserver journal et IDs pour la reprise ; ne pas réinitialiser
sa racine pour forcer un retry. Le [registre de validation](../../engineering/VALIDATION.md)
précise les scénarios logiciels, de panne et runtime réellement exécutés.

## Soumettre et consulter un job

Dans le terminal client, renseigner les mêmes variables de chemins. Choisir **une seule fois** un UUID d'opération neuf non nul, le conserver et garder les mêmes entrées lors d'un retry. La valeur ci-dessous doit être remplacée avant exécution.

```sh
AIGUARD_OPERATION="<new-operation-uuid>"
"$AIGUARD_BIN" broker submit --root "$AIGUARD_BROKER_ROOT" --profile "$AIGUARD_BROKER_PROFILE" --token-file "$AIGUARD_RUNNER_FILE" --operation-id "$AIGUARD_OPERATION" --project "$AIGUARD_PROJECT" --timeout-seconds 30 -- /usr/bin/id
"$AIGUARD_BIN" broker status --root "$AIGUARD_BROKER_ROOT" --profile "$AIGUARD_BROKER_PROFILE" --token-file "$AIGUARD_RUNNER_FILE" --operation-id "$AIGUARD_OPERATION"
"$AIGUARD_BIN" broker list --root "$AIGUARD_BROKER_ROOT" --profile "$AIGUARD_BROKER_PROFILE" --token-file "$AIGUARD_RUNNER_FILE" --limit 20
```

Submit accepte soit l'argv manuel après `--`, soit une sélection gérée avec `--client codex|claude-code|auto`. La sélection gérée exige `--prompt` ; ses options facultatives sont `--auth-mode auto|api|subscription`, `--client-profile` et `--model`. Ne pas combiner options gérées et argv manuel. Le registre par défaut ci-dessus désactive les clients gérés. Pour en utiliser un, configurer un profil autorisé avec `managed_clients: true`, un `providers_file` absolu, l'image préparée adaptée et un réseau explicitement approuvé ; puis sélectionner ce profil avec `--profile`. Par exemple, après cette préparation distincte :

```sh
AIGUARD_BROKER_PROFILE="<configured-managed-broker-profile-id>"
AIGUARD_OPERATION="<another-new-operation-uuid>"
"$AIGUARD_BIN" broker submit --root "$AIGUARD_BROKER_ROOT" --profile "$AIGUARD_BROKER_PROFILE" --token-file "$AIGUARD_RUNNER_FILE" --operation-id "$AIGUARD_OPERATION" --project "$AIGUARD_PROJECT" --timeout-seconds 30 --client claude-code --auth-mode api --client-profile claude-api --model claude-haiku-5-5 --prompt "Report the top-level file names only. Do not modify files, run tests, or start subagents."
```

C'est une autre possibilité de nouveau job, pas un deuxième Submit de l'UUID de l'exemple manuel. Attribuer d'abord un nouvel UUID d'opération. La résolution fournisseur sélectionnée est renvoyée avec Accepted et affichée par la CLI ; un binding accepté n'est pas résolu de nouveau au replay. L'authentification fournisseur reste distincte de l'authentification ai-guard. Ne jamais placer de credential dans un prompt ou l'argv. `--capture-output` est facultatif et exige la permission du profil ; la sortie capturée a ses propres bornes et couverture, sans transcript complet présumé. Cette CLI broker est batch uniquement et n'a pas d'option `--interactive`.

Sans `--operation-id`, la CLI en génère un et affiche `Operation: UUID` avant la requête. Le conserver même après une erreur. Les octets du payload Submit engagent projet, exécution, durée demandée et choix de capture. Le replay exact reprend la même identité d'opération et les mêmes entrées ; un contenu modifié sous cet UUID est un conflit. Une réponse Accepted confirme l'acceptation locale durable, pas Allow, Running, la fin ou une modification de fichier demandée. Rejected est un résultat de préparation enregistré ; examiner ses champs `phase`, `code`, `record_hash` et `replayed`.

Status et list exposent les faits journal/contrôle mis en cache. `accepted`, `reserved_in_process`, `interrupted`, `rejected` et `absent` sont des états locaux de job, pas des états terminaux runtime. Examiner ensemble `last_decision`, `review_request_hash`, `pending_rpc_count`, `worker_state`, `quota_held`, `runtime_status`, `last_observed_at` et `executor_error`. Les faits null ou indisponibles restent inconnus. `executor_task_running: false` ne prouve pas qu'un conteneur est arrêté.

La limite de list va de 1 à 100, par défaut20. Continuer avec `--after <next_after-uuid>` seulement si la réponse renvoie ce curseur. Une page n'est ni un total du tenant ni un snapshot entre pages. Corruption, état incomplet et erreur d'accès ne sont jamais une activité vide.

## Approuver, annuler et reprendre avec des identités stables

Chaque mutation vise l'opération d'origine. Attribuer un `--command-id` neuf non nul distinct à chaque approve, cancel, resume ou export délibéré. Le conserver avant envoi ; après une réponse perdue, réessayer cette même commande avec le même UUID et contenu. Ne pas réutiliser un UUID d'approval/cancel/resume/export pour une autre commande ou destination. S'il est omis, la CLI génère et affiche `Command: UUID` ; garder cette valeur pour la reprise.

Pour Review, examiner la demande exacte en attente et sa décision, puis utiliser le `review_request_hash` complet de status. L'approbation enregistre une soumission d'approbation pour cette demande et exige encore une décision centrale approuvée ; ce n'est pas un Allow global. Un credential runner technique seul n'établit pas un auteur humain nominatif.

```sh
AIGUARD_APPROVAL="<new-approval-command-uuid>"
AIGUARD_REVIEW_HASH="<exact-outstanding-review-request-hash>"
"$AIGUARD_BIN" broker approve --root "$AIGUARD_BROKER_ROOT" --profile "$AIGUARD_BROKER_PROFILE" --token-file "$AIGUARD_RUNNER_FILE" --operation-id "$AIGUARD_OPERATION" --command-id "$AIGUARD_APPROVAL" --request-hash "$AIGUARD_REVIEW_HASH"
```

Pour demander une annulation et examiner les faits connus :

```sh
AIGUARD_CANCEL="<new-cancel-command-uuid>"
"$AIGUARD_BIN" broker cancel --root "$AIGUARD_BROKER_ROOT" --profile "$AIGUARD_BROKER_PROFILE" --token-file "$AIGUARD_RUNNER_FILE" --operation-id "$AIGUARD_OPERATION" --command-id "$AIGUARD_CANCEL"
"$AIGUARD_BIN" broker recovery-status --root "$AIGUARD_BROKER_ROOT" --profile "$AIGUARD_BROKER_PROFILE" --token-file "$AIGUARD_RUNNER_FILE" --operation-id "$AIGUARD_OPERATION"
"$AIGUARD_BIN" broker status --root "$AIGUARD_BROKER_ROOT" --profile "$AIGUARD_BROKER_PROFILE" --token-file "$AIGUARD_RUNNER_FILE" --operation-id "$AIGUARD_OPERATION"
```

Un ACK d'annulation n'est ni une observation terminale runtime ni une preuve de libération de quota ou d'absence d'un start encore en vol. Si l'ACK est perdu ou si la réponse est `needs_reconciliation`, conserver la même identité cancel. Une annulation durable peut exister malgré une réponse en erreur. Elle ne peut pas être annulée pour redémarrer le worker.

`recovery-status` lit les métadonnées confirmées du journal même si le chargement métier complet est indisponible. Sa `source` est `confirmed_journal` et sa `runtime_evidence` est `not_queried` : il **n'inspecte pas le moteur**. `journal_mode: "stop_only"`, `incomplete`, `reconciliation_required`, les Arms pending et les échéances originales décrivent les preuves retenues, pas de nouvelles observations ou une permission d'exécution. Des champs ou cibles absents n'établissent pas leur absence du runtime.

Si l'UUID de l'opération a été perdu, découvrir les jobs acceptés dans le profil authentifié :

```sh
"$AIGUARD_BIN" broker recovery-inventory --root "$AIGUARD_BROKER_ROOT" --profile "$AIGUARD_BROKER_PROFILE" --token-file "$AIGUARD_RUNNER_FILE" --limit 20
```

La découverte à froid exige un broker existant explicitement démarré avec `broker serve --config "$AIGUARD_BROKER_CONFIG" --recovery-only`. Un service normal dont le profil n'est pas encore ouvert répond `needs_reconciliation` sans ouvrir ce journal ni changer son admission. Une queue Full/STOP déjà retenue se lit sur place. Ne pas lancer de second propriétaire ni remplacer sa racine pour contourner un verrou. Le mode récupération conserve l'admission fermée ; cette commande ne démarre ni ne reprend de worker.

Si le répertoire des journaux, le profil ou le namespace du journal requis est absent, les lectures de récupération et demandes d'annulation valides répondent `needs_reconciliation`. Ce n'est pas un inventaire vide et cela ne prouve pas l'absence de worker. Vérifier l'installation sélectionnée et préserver son journal original, son propriétaire et ses sauvegardes cohérentes ; ne pas initialiser un journal de remplacement pour effacer l'erreur. Les UUID nuls ou bornes de pagination invalides répondent toujours `invalid_input` après authentification ; une trame malformée peut être refusée avant authentification. Une opération valide mais inconnue dans un journal existant conserve son refus actuel.

La réponse contient les UUID et têtes confirmés des jobs acceptés, sans activité runtime, contenu métier ou total de workers. Utiliser un `operation_id` retourné avec `recovery-status` ci-dessus. La pagination accepte `--limit` de 1 à 100 et un UUID `--after` facultatif ; poursuivre seulement avec le `next_after` retourné. Un scan partiel fournit des problèmes explicites et aucune garantie de continuation ; des données absentes ne prouvent pas l'inactivité du runtime. La perte d'une ancre commune du journal refuse la lecture. Le scan peut encore lire jusqu'aux bornes configurées du journal ; une petite page ne garantit pas une faible latence. Un seul inventaire est en file ou en cours par writer retenu, même après timeout du demandeur. Cela ne préempte pas un scan en cours et ne réserve pas de place aux annulations.

Pour une réconciliation explicite du même job accepté, quand contrôle et runtime requis sont disponibles :

```sh
AIGUARD_RESUME="<new-resume-command-uuid>"
"$AIGUARD_BIN" broker resume --root "$AIGUARD_BROKER_ROOT" --profile "$AIGUARD_BROKER_PROFILE" --token-file "$AIGUARD_RUNNER_FILE" --operation-id "$AIGUARD_OPERATION" --command-id "$AIGUARD_RESUME"
```

Resume conserve intention acceptée, credentials, autorité runtime, échéances originales et claims consommés. Il ne donne ni autre start ni renouvellement de durée. Une capacité source fraîche n'est admise que par les contrôles produit de source figée pour l'étape pré-managed concernée ; ne pas modifier source, binding ou checkpoint pour les faire passer. Une dépendance indisponible, un runtime ambigu ou une réconciliation refusée reste une erreur explicite.

Après une annulation durable, la réconciliation d'un worker précédemment Running ou Stopping utilise uniquement son autorité runtime retenue et son identifiant de conteneur immuable. Une observation terminale complète doit correspondre à l'ownership conservé, à la configuration, aux limites de ressources et au démarrage observé centralement. Seule la transaction centrale durable enregistrant Terminal libère le quota d'allocation. Le relevé reste non facturable ; répéter la réconciliation n'ajoute pas d'allocation. Une observation indisponible, encore active ou divergente conserve le quota et signale une erreur de réconciliation.

Cette observation n'émet aucun nouveau start, helper, cleanup ou retrait du gardien, et ne renouvelle aucune échéance. Consulter `status` après l'accusé de commande : accepter Resume n'est pas un résultat terminal. Resume exige toujours le contrôle compatible actif et les prérequis Full indiqués plus haut ; `--recovery-only` ne l'active pas et ne réactive pas un contrôle inactif.

Le code couvre Running et Stopping ; les essais ciblés portent actuellement sur Running uniquement, avec des observations runtime synthétiques et un budget Writer de fixture de 60 secondes. La fiabilité sous le budget produit normal de réponse du Writer de 10 secondes reste non qualifiée. Une erreur ou une perte d'accusé peut suivre une transition terminale durable : examiner `status` avant de conclure à l'échec ou de réessayer. Ces résultats ne fournissent pas de reprise froide d'un contrôle inactif. Voir le [périmètre de validation](../../engineering/TEST_STRATEGY.md#réconciliation-après-cancel--sélection-logicielle-warm).

## Exporter vers une destination neuve

Après examen des décisions, résultats observés et changements, demander un export audité du job accepté. Le répertoire parent doit déjà exister sous une racine d'export permise ; la destination doit être **neuve**, hors du projet source et du stockage privé du contrôle. L'original n'est jamais réécrit automatiquement.

```sh
AIGUARD_EXPORT_COMMAND="<new-export-command-uuid>"
AIGUARD_EXPORT="/absolute/data/exports/new-reviewed-result"
"$AIGUARD_BIN" broker export --root "$AIGUARD_BROKER_ROOT" --profile "$AIGUARD_BROKER_PROFILE" --token-file "$AIGUARD_RUNNER_FILE" --operation-id "$AIGUARD_OPERATION" --command-id "$AIGUARD_EXPORT_COMMAND" --destination "$AIGUARD_EXPORT"
```

Conserver ensemble l'UUID de commande et la destination exacte. Un export acquitté renvoie son identité de requête, le digest du manifeste et le nombre de fichiers. Relire le contenu exporté et le diff avant d'appliquer des changements au projet. Après erreur ou ACK perdu, une destination peut avoir déjà été publiée : la préserver, examiner status et ne réessayer que la même requête. Un arbre existant inconnu ne peut pas être adopté comme export terminé. Choisir une autre destination neuve seulement comme export délibéré distinct avec une nouvelle identité de commande, après résolution du résultat précédent.

Un export explicite après annulation exige son propre grant audité ultérieur et tous les prérequis d'export ; cancel ne l'autorise pas implicitement. Un grant ou ACK ne donne pas de permission pour redémarrer le worker. Liens, traversées, identité du parent modifiée, permissions dangereuses et preuves indisponibles causent un refus plutôt qu'un succès partiel caché en diff vide.

## Diagnostiquer les erreurs et préserver les preuves de reprise

La CLI affiche les réponses JSON et sort en erreur pour `error` et `rejected`. Les erreurs de transport distinguent une requête non envoyée d'un résultat inconnu ou d'une réponse indisponible. Préserver les identités opération/commande avant retry ; un exit client non nul ne prouve pas l'absence de mutation durable.

Le client HTTP de contrôle du runner désactive les réessais implicites, les
redirections et la découverte de proxy. Sa connexion est limitée à 3 secondes
et chaque requête à 15 secondes. Une réponse perdue peut suivre une mutation
engagée : examiner l'état durable et rejouer seulement la même identité
d'opération admissible. Ces limites ne redémarrent rien automatiquement,
ne prolongent aucune échéance worker et ne réactivent pas le contrôle.

| Code ou symptôme | Sens et prochaine action |
|---|---|
| `unauthorized` | Le credential présenté n'est pas authentifié. Vérifier référence de fichier privé et gateway/profil choisi sans exposer le contenu. |
| `forbidden` | Rôle, tenant, namespace ou ressource permise ne correspond pas. Corriger le périmètre autorisé de l'installation ; modifier le registre ne donne pas d'appartenance. |
| `incompatible_control` | Schéma/protocole incompatible, ou exécution fraîche exigeant un contrôle actif. Suivre le prérequis contrôle et conserver les bases existantes. |
| `unavailable` | Une dépendance requise ou un service est indisponible. Rétablir la même dépendance et consulter l'opération d'origine. |
| `busy` | Une ressource bornée du service, writer ou job est occupée. Examiner puis réessayer la même requête ; ne pas créer de doublons pour contourner la capacité. |
| `invalid_input` | Vérifier options, IDs, configuration du profil, chemins et bornes demandées. Un nouveau sens exige une nouvelle identité, pas un contenu changé sous l'existante. |
| `conflict` | Une identité, entrée immuable, autorité ou révision diffère. Comparer la requête enregistrée ; préserver les preuves et le refus. |
| `interrupted` | La préparation n'est pas terminée. Consulter l'opération d'origine ; une réservation interrompue n'autorise ni second ticket ni start. |
| `limit_exceeded` | La capacité configurée ou les bornes du journal durable sont épuisées. Examiner travaux retenus et capacité approuvée ; ne pas effacer de preuves pour libérer de l'espace. |
| `needs_reconciliation` | L'opération ne peut pas établir sûrement l'étape suivante. Utiliser status et recovery-status, puis le même resume/cancel explicite lorsqu'il est admissible. L'inconnu reste inconnu. |
| Broker absent ou découverte dangereuse | Démarrer `broker serve` explicitement seulement après vérification de l'installation et du propriétaire existant. Les clients ne le démarrent pas automatiquement. Préserver leases, endpoints et état. |

La source de ces réponses et champs fermés est [le protocole IPC](../../../src/broker/service/protocol.rs) ; les options implémentées sont dans [la CLI](../../../src/broker_cli.rs) et les champs du registre dans [son parseur](../../../src/broker/service/profile.rs). Les fichiers internes du journal peuvent contenir credentials et contenu privé. Ni signalement public d'erreur ni export de preuves ne doit inclure registre brut, bindings, tokens, contenu de bases ou sorties non relues.

## Arrêter, redémarrer et maintenir l'installation

Demander l'annulation des jobs concernés, puis vérifier leur état durable et observé avant d'arrêter les services de contrôle. Quand une observation manque, conserver son incertitude et l'état d'enforcement. Ne pas assimiler sortie client, Created, absence runtime, timeout, `kill` ou `rm` au fencing d'un start en vol. Ne pas terminer/désarmer un enforcer retenu, réarmer un start consommé, supprimer ses fichiers de découverte, retirer des volumes ou effacer des preuves pour obtenir un état propre.

Le broker au premier plan traite SIGINT/SIGTERM en fermant les admissions et en invalidant ses générations de start ; son enforcer distinct conserve les cibles déjà armées et leurs échéances originales. Ce n'est pas une procédure générale de nettoyage ou de désarmement. Redémarrer le broker au premier plan avec les mêmes binaire/configuration/racine et un contrôle compatible seulement après investigation du précédent propriétaire. La restriction de contrôle actif ci-dessus reste applicable après redémarrage du gateway.

Préserver ensemble registre privé, identité d'installation, journal broker, enregistrements d'entrées d'export, configuration et bindings de credentials, bases gateway et sauvegardes cohérentes, autorités runtime retenues et volumes worker. La sauvegarde/restauration doit tenir compte du WAL SQLite, de la confidentialité et de l'autorité des claims en cours. Restaurer d'anciens fichiers ne restaure pas une permission d'exécution fraîche. Ce guide n'établit ni rétention automatique ni rollback sûr ; suivre [l'exploitation](../../operations.md) et le [contrat journal/reprise](../../engineering/BROKER_JOURNAL.md) pour les frontières applicables.

Le produit complet exige aussi réactivation sûre du contrôle, cycle de vie géré par l'OS, fencing des starts ambigus, continuité du terminal et qualification réelle des plateformes/pannes. Leur état courant et les preuves requises restent dans [engineering](../../engineering/PRODUCTION_READINESS.md), avec [le contrat de supervision](../../engineering/BROKER_SUPERVISION.md). Ne pas remplacer ces résultats par les commandes au premier plan ou un exemple de configuration.
