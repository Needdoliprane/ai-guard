# Sources humaines possédées : identité locale et ressources

Ce mode explicite du gateway rouvre une union possédée complète déjà existante. Le défaut reste `legacy`. La [validation logicielle datée](../../engineering/evidence/2026-11-13/human-sources-operator/README.fr.md) couvre l'adoption locale et les deux lectures adoptées sur une source Linux ARM64 identifiée ; elle ne qualifie ni production, ni runtime de worker, ni OIDC.

Suivre la [procédure d'adoption hors ligne des sources](../human-sources-maintenance/README.fr.md) pour préparer métadonnées/enveloppe privées, migrer l'union et publier le vrai manifeste de fermeture et l'activation du gateway. Ses cinq commandes sont `human-sources prepare`, `resume-prepare`, `apply`, `resume` et `check`. L'adoption exige un registre de tenants et des Stores13 existants, plus Auth4, configuration OIDCV2 et bundle public de confiance explicite si OIDC est choisi. Le coordinateur convertit l'union complète de Store13 à14 et d'Auth4 à5 ; les migrations monobases ordinaires s'arrêtent toujours à Store13/Auth4. Provisionner personnes, credentials et grants avant conversion avec la [procédure d'identité legacy](README.fr.md) ; l'adoption n'en crée aucun. Ne pas fabriquer de reçus, initialiser des bases de remplacement, relever les versions acceptées ou employer les fixtures comme procédure d'installation.

Après publication de l'activation complète par `apply` ou `resume`, l'utiliser pour la réouverture normale dans le lancement conteneurisé existant du gateway :

```text
gateway --human-sources-activation /control/human-preparation/activation.json
```

Conserver bind HTTP, TLS serveur et réglages de décision réels de l'installation. Ne pas combiner activation avec les options directes de sources, registre, credentials, révisions, capacité, initialisation ou OIDC. Le gateway reconstruit et rejoue la vraie union à chaque démarrage ; les fichiers d'activation seuls ne prouvent pas la disponibilité des sources. Arrêter tous les consommateurs avant préparation ou migration : `--confirm-control-stopped` est une attestation manuelle, sans fencing des anciens processus. Après interruption, conserver sur place sources, préparation privée, journal, sauvegardes et configuration concordante, puis employer l'étape de reprise correspondante du guide. `check` valide seulement fichiers/métadonnées, sans vérifier SQLite, authentification ou disponibilité.

Les options directes ci-dessous restent disponibles pour une installation dont manifeste/enveloppe retenus proviennent du vrai protocole de migration owned. Les employer comme alternative à l'activation, en conservant politique, montages, plafond d'événements et autres options réels :

```text
gateway --human-source-mode owned_sources
  --tenants-file /control/tenants.json
  --human-sources-manifest /control/existing-closure.json
  --human-credentials-envelope /control/existing-technical-credentials.json
  --human-control-config-revision <révision-contrôle-positive-réelle>
```

Ce sont des références vers des fichiers privés existants, jamais des tokens en arguments. Le démarrage ne génère ni manifeste ni enveloppe. Le registre donne les quotas et politiques effectifs. Le loader résout indépendamment les défauts et recalcule le digest public version1, sans credentials, digests de credentials ni chemins privés ; les physiques sont comparés séparément. Employer la révision conservée par l'opérateur, sans la deviner depuis une erreur. Avec OIDC, conserver `--oidc-config` et fournir `--human-oidc-mapping-revision` ; cette révision positive de fermeture est distincte de la chaîne `mapping.revision`. Fichier, limite, révision, format ou union absent/divergent refuse le démarrage. Les flags d'initialisation sont incompatibles. Conserver fichiers et commits connus après échec ; aucune migration, remise à zéro ou bascule legacy n'a lieu.

Employer le profil de présentation de confiance concordant :

```text
dashboard --shared-tenants --human-source-mode owned_sources
  --upstream http://gateway:8787 --front-dir /app/front
```

Conserver l'upstream interne et le répertoire statique réels de l'installation. Le repli sur un fichier viewer est incompatible. Le gateway reste l'autorité d'autorisation même si la présentation est mal configurée. Sur les routes du proxy API du dashboard, HEAD moderne est refusé avant vérification viewer ou conversion en GET. La route de santé du gateway conserve GET/HEAD ; elle est distincte du diagnostic de contrôle.

Le credential admis est le bearer local privé déjà provisionné. Le premier GET `human identity` vérifie sa connexion ; aucun endpoint password/login n'est ajouté. Employer les [commandes identité/ressources existantes](README.fr.md#consulter-avec-le-cli), `--credential-file` privé et un contexte réellement reçu. Les lectures humaines de base comprennent `GET /api/v1/human/identity`, `GET /api/v1/human/resources`, `GET /api/v1/human/events` et `GET /api/v1/human/events/recent`. Les deux listes d’événements sont limitées aux métadonnées. L'identité décrit la source authentifiante ; en présence d'autres sources tenants, `contexts_complete=false` signifie que leurs memberships restent inconnus. La sélection cross-source est refusée. La page de ressources porte son vrai `read_receipt` durable ; un candidat digest ne fournit ni identité, ni droit, ni reçu.

Pour le détail exact, utiliser `GET /api/v1/human/events/{id}?view=metadata` avec l’UUID canonique de l’événement en minuscules avec tirets et un contexte `X-AiGuard-Context` proposé par le contrôle. Cette lecture couvre les Stores SQLite owned 14 et 15. Elle applique les mêmes contrôles de source authentifiée, d’epoch d’autorisation et de droits de métadonnées que la liste, dont l’accès à la ressource de l’événement. Le succès renvoie une seule entrée de métadonnées et un `read_receipt` durable pour l’action d’audit `event`. Avec le gateway et le dashboard de la même version, ouvrir **Métadonnées** pour la ressource autorisée dans **Ressources**, puis **Détails** sur l’événement dans **Événements**.

**Vue 360°** s’ouvre en contexte d’administration et **Mon espace** en contexte personnel lorsque le catalogue indique `summary_read` comme accordé et disponible. La lecture utilise le Store 14 ou 15 local retenu avec le marqueur des droits humains et exige un audit durable. Les sections workers, quotas et usage portent `profile_unavailable` lorsque le droit correspondant est accordé.

La lecture récente du Journal porte seulement sur les métadonnées. Une page réussie avec `stream_mode:"snapshot_only"` et `live_cursor:null` identifie ce profil owned actuel ; le marqueur n’accorde aucun droit et ne décrit pas un autre serveur/profil. Les curseurs des pages plus anciennes conservent leur sélection autorisée et leur borne descendante. L’interface affiche l’heure de lecture et le reçu réels de la dernière page lue. Le direct, le contenu et l’export sont indisponibles pour ce profil. Le catalogue indique `profile_unavailable` pour `events_read_content`, `events_export` et `events_stream`, même si le droit correspondant est accordé. Après une page Journal réussie annonçant ce profil, Contenu et Export expliquent cette limite et sont désactivés ; les droits accordés restent inchangés.

Utiliser **Actualiser** pour relire les événements récents. Le Journal n’ouvre ni flux, ni boucle de relance, ni lecture périodique ; le retour dans l’onglet conserve la revalidation ponctuelle normale. Une revalidation en échec retire les données affichées, reçus et positions de pagination du snapshot. Une erreur transitoire dans le même contexte conserve la restriction de profil déjà établie ; changement d’identité/contexte/autorisation ou fermeture de session retire ce constat. Un503 n’établit jamais un autre profil. Une demande explicite de contenu refusée reste une erreur avec retour explicite vers Ressources, sans repli silencieux metadata. Mettre à jour gateway et dashboard ensemble. La preuve logicielle datée en tête de ce guide conserve son périmètre d’origine de deux lectures.

Les méthodes connues non adoptées humaines, contrôle, workload, brokers, relais et OIDC, dont HEAD sur le détail d’événement, export et flux, renvoient 503 explicitement avant extracteurs legacy, corps ou Store. OIDC sélectionné dans le manifeste reste sélectionné et retenu, avec handlers/bearers fermés ; il n'est jamais annoncé désactivé. Les GET status/profile existants « disabled » sont admis seulement si OIDC est absent. La santé consulte la mémoire ; elle ne prouve pas la disponibilité de chaque source. Ces lectures n’activent ni approbation, ni lancement, ni permission réseau, ni contrôle egress modernes.

Le contexte d’administration local peut lire `GET /api/v1/human/workers` sur SQLite 14 et 15 avec `read_worker_registry`. L’identité annonce `worker_registry:"human_worker_registry_owned_v1"`. Chaque page porte un reçu durable de la source authentifiée. Le dashboard affiche la vue Workers autorisée et son heure de lecture. La fédération et l’agrégat workers de la synthèse restent indisponibles. [Registre workers](../supervision/README.fr.md#profil-de-supervision-des-brokers-owned)

Le contenu, l’export et le flux d’événements ainsi que les écritures d’administration via le dashboard restent exigés en V1.

Un mutex ou writer SQLite occupé chez A ne déclenche aucun SQL chez A lors de l'authentification/lecture de B. La propre source occupée renvoie503. Busy n'est ni absence ni mismatch permanent. Un mismatch physique/configuration/marqueur/incarnation réellement comparé conserve l'invalidation globale de l'owner. La revalidation physique couvre toujours l'union entière : syscall bloqué ou epoch Write partagé gardent leurs limites de disponibilité. L'index privé de candidats est borné avant accumulation à65536 entrées/16Mio de représentation ; le dépassement refuse toute publication complète. Ces bornes ne mesurent pas la RAM du processus.

Pour les lectures humaines adoptées, le runtime compare la vraie révision d'autorisation d'identité et le maximum de l'historique des bindings workload de sa propre source avant/après l'opération adoptée. Ces marqueurs couvrent les mutations d'identité et l'émission/rotation/révocation de bindings ; l'audit des lectures ne les déplace pas. Un mismatch comparé exige une reconstruction admission fermée. Ce n'est pas une génération universelle des writers : les writers HTTP d’identité/workload restent fermés, et une mutation brute du stockage voisin ne peut être détectée instantanément sans l'observer.

## A0 candidat : diagnostic technique de contrôle réservé

**Contrat candidat A0, 13 novembre 2026 : code et schéma JSON alignés ; validation logicielle ciblée sur `0114013` : 37 tests réussis sans échec ni test ignoré, compilation et vérification toutes cibles réussies.** Ces résultats logiciels ne qualifient ni l’isolation runtime ni un déploiement en production. La validation des métadonnées humaines déjà liée dans ce guide conserve sa source et son périmètre ; elle ne prouve pas A0. Les commandes de démarrage et fichiers de configuration ci-dessus sont inchangés.

Le candidat adopte uniquement le GET exact existant `/control/api/v1/control/protocol` sur le transport réservé, sans query ni nouveau flag de démarrage. Il exige une vraie entrée technique `runner` du `ReviewCredentialRegistry` retenu, un candidat technique unique dans l’index complet de démarrage et les vrais binding/source/namespace/génération. Un bearer humain nominatif, viewer, credential workload, `HumanSourceAccess` ou DTO ne fournit pas cette identité d’acteur. Le GET public `/api/v1/control/protocol`, HEAD sur ce diagnostic et les mutations owned connues restent fermés ; chemins/méthodes inconnus gardent 404/405.

La transaction checked `Business` vérifie le binding avant les faits et le revalide avant commit sur le Store choisi. Credential, source et epoch sont revalidés avant publication. L’usage accepté et les permis d’opération/IO restent attachés au travail réel jusqu’à sa fin, y compris après abandon du futur HTTP ; cette propriété est couverte par la validation logicielle ciblée. La propre source occupée refuse sans attente ; le timeout SQLite précédent est restauré, sans SQL voisin ni repli legacy. A0 ne provisionne ni credential ni binding et n’ajoute aucune commande opérateur, service, format SQLite, record ou reçu d’audit.

Le validateur Rust et le schéma JSON du candidat concordent sur les faits existants de `ControlProtocolV1` pour les formats 14/15 ; `active` reste le fait réel de la lease. Un futur 200 de `AuditClient::control_supervision_identity` ne qualifierait ni lifecycle broker ni permis de connexion. Les gardes broker 3–13 et relais 6–13 sont inchangées ; 14/15 rencontrent toujours `IncompatibleControl` avant les effets broker. Aucun worker, lancement humain ou droit egress n’est ouvert.

La couverture est l’index W0 complet au démarrage et la génération de la propre source sélectionnée, sans cohérence multiprocessus ni génération universelle des writers. Dans le périmètre de la source A0 examinée, les writers HTTP owned restent fermés. Le candidat A1 ci-dessous ne raccorde que replace/revoke du registre de politiques ; les writers internes d’identité/workload à source/`WriteEpoch` restent sans caller serveur post-démarrage. Un futur writer modifiant les credentials, workloads ou leur couverture doit fermer l’admission par `require_rebuild` **avant l’effet**, puis reconstruire l’union complète. Une lecture B ne peut pas déduire une nouvelle collision chez A sans producteur d’invalidation global ; le vrai binding propre reste néanmoins vérifié à chaque diagnostic. Voir le [contrat API candidat](../../api.md#owned-control-a0). Ce sont des limites à préserver avant toute adoption d’un writer, sans nouvelle procédure utilisateur.

Le serveur réserve son superviseur de cleanup avant builders/binds faillibles. Arrêt et Drop ferment admission humaine/contextes, arrêtent HTTP, observent les vrais joins HTTP, puis attendent usages blocking acceptés/en queue et epochs. Une erreur de drain reste un échec, avec l'erreur primaire de bind/serve éventuelle. Un drain non établi n'est jamais un succès ; les vrais handles d'usage/epoch restants retiennent owner et descripteurs. Terminaison du runtime/processus et panne d'hôte restent hors de cette garantie in-process.

Le `HumanApiClient.close()` local efface le token, annule les requêtes et écarte les réponses tardives. Il ne révoque pas le credential durable. Mutation moderne des credentials, découverte fraîche des memberships des autres sources et sessions/révocation SSO restent des exigences ouvertes. Le parcours d'adoption consigne ses résultats logiciels et ses [limites de qualification restantes](../human-sources-maintenance/README.fr.md#périmètre-de-validation). Arrêter les consommateurs et conserver configuration, bases, backups et preuves avant réconciliation ; ne jamais supprimer l'historique pour obtenir un démarrage vert.

<a id="owned-policy-a1-operator"></a>

## A1 : inspecter et préparer les opérations du registre de politiques

Les [preuves inspection et CLI interne](../../engineering/evidence/2026-11-14/owned-policy-inspection/README.fr.md) vérifient le parcours composé sur `45179db` ; le rapport reste historique et le serveur revérifie la mutation finale. Cette preuve logicielle locale ne qualifie ni client IA, modèle, worker ou filtrage réseau.

Utiliser `broker policy inspect` pour conserver le scope réel et les métadonnées de politique, puis préparer une requête localement depuis ce rapport. Les opérations de registre gardent la portée `policy_registry_only` ; l’inspection porte sur `policy_registry_metadata`. Un rapport, une politique durable ou un reçu `revoke_requested` ne prouvent ni approbation humaine, ni filtrage réseau, permis de connexion ou exécution worker/broker. Les [preuves logicielles A1 antérieures datées](../../engineering/evidence/2026-11-14/owned-egress-policy-api-cli/README.fr.md) conservent leur source et leur périmètre exacts ; elles ne qualifient ni une autre source ni un déploiement en production.

### Prérequis que ces commandes ne créent pas

Utiliser une installation owned déjà adoptée, avec **vrai Store15**, union complète retenue et credential technique runner déjà présent dans son registre privé **avec un binding actif vers ces source/tenant/namespace exacts**. Un bearer humain nominatif, viewer, token workload ou rôle SSO ne fournit pas ce credential. Ces commandes ne provisionnent ni compte, credential, binding, Session ou worker et ne migrent rien. Le parcours d’adoption précédent convertit 13→14, pas 15 ; le [contrat de maintenance E1](../../engineering/EGRESS_POLICY_STORE.fr.md#format15-exact-et-maintenance) décrit le protocole de confiance existant, distinct. Ne pas fabriquer enveloppes, reçus ou lignes SQLite, ni utiliser les fixtures comme procédure d’installation. Sans ces vrais prérequis, s’arrêter ici et conserver les fichiers de l’installation.

Conserver le token runner et les inputs draft/enveloppe complète privés (0600), hors workspace utilisateur, frontend et artefacts publics. Chaque sortie exige un parent privé (0700) déjà présent et une **destination neuve absente**. Séparer rapport et opération ; aucune opération ne peut être préparée dans son rapport, y compris via un alias physique de casse/Unicode. Les noms finaux sont bornés à 128 caractères ASCII alphanumériques/point/tiret/underscore, hors `.` et `..`. Chemins et ancêtres sont contrôlés sans suivre de liens arbitraires ; aucun fichier, mode ou répertoire existant n’est réparé silencieusement.

**Fixer d’abord les variables ci-dessous aux vraies origines non secrètes et chemins privés de l’installation.** `GATEWAY_ORIGIN` est une origine HTTPS ou HTTP loopback, sans credential URL, query, fragment ni chemin hors racine. `PRIVATE_RUNNER_TOKEN` désigne le fichier privé, jamais son contenu. `SESSION_ID`, si sélectionné, est le vrai UUID Session canonique non nul. Le credential détermine le tenant ; aucun sélecteur ne choisit un autre client.

### Inspecter un sélecteur de politique

Choisir l’une de ces alternatives, avec un `NEW_INSPECTION_DIR` neuf :

```sh
agentsandbox broker policy inspect --gateway "$GATEWAY_ORIGIN" --token-file "$PRIVATE_RUNNER_TOKEN" --tenant --output-dir "$NEW_INSPECTION_DIR"
```

```sh
agentsandbox broker policy inspect --gateway "$GATEWAY_ORIGIN" --token-file "$PRIVATE_RUNNER_TOKEN" --session "$SESSION_ID" --output-dir "$NEW_INSPECTION_DIR"
```

Exactement un flag `--tenant` ou `--session` est requis. L’inspection effectue un seul POST réservé explicite de métadonnées ; elle ne soumet ni remplacement ni révocation. Elle valide la réponse et revalide le token privé autour de la requête et de la publication durable. Le succès n’affiche que version, kind, observed_at, historical:true et coverage:policy_registry_metadata. Ce n’est ni une lecture complète de politique ni une vue runtime courante.

Le rapport privé contient exactement quatre entrées : `request.json` (requête canonique du sélecteur), `metadata.json` (`version:1`, `kind:"policy_inspection"`, gateway et SHA-256 de requête/observation), `current.json` (observation canonique) et `owner.lock`. Les records sont 0600 et le répertoire 0700. Sa gateway est l’origine canonique validée **sans son unique slash de chemin racine** ; la réouverture exige cette forme exacte. Les métadonnées de l’opération A1 dérivée conservent leur normalisation d’origine existante. Le rapport n’accepte ni état pending ni reçu et ne peut pas servir de répertoire d’opération à apply/resume/status.

`current.json` conserve le vrai scope authentifié, le sélecteur demandé, un head explicitement absent ou présent, les métadonnées du plafond tenant pour Session, le flag courant de ressource d’identité et observed_at fourni par le contrôle. `session_resource` est une **chaîne** : `"not_applicable"` pour Tenant, sinon `"active"`, `"inactive"` ou `"absent"`. Les états du registre n’établissent ni éligibilité de l’intervalle ni droits effectifs ; le flag de ressource n’est pas un état runtime. Voir les [champs API exacts](../../api.md#owned-policy-inspection).

L’observation est datée, historique, non signée et modifiable par son propriétaire local. Les hashes corrèlent les octets locaux ; ils n’authentifient aucun droit et ne garantissent aucune fraîcheur. Source et audit checked sains sont requis, sans nouvel événement ou reçu d’audit par inspection. Une source inconnue, occupée ou endommagée ne devient jamais un head absent. Une autre opération peut modifier head ou namespace immédiatement après l’inspection.

### Rédiger un draft fermé et préparer localement

Un draft replace contient seulement version, action et policy. **Ceci est un modèle : remplacer les deux temps null par une fenêtre valide explicitement choisie en millisecondes Unix avant utilisation.** L’expiration suit le début ; l’horloge durable et les règles du contrôle restent l’autorité. Le tableau de destinations vide exprime une politique de registre deny-all, pas un accès réseau.

```json
{
  "version": 1,
  "action": "replace",
  "policy": {
    "not_before_ms": null,
    "expires_at_ms": null,
    "destinations": [],
    "max_destinations": 128,
    "max_resolution_ips": 16
  }
}
```

Un draft revoke ne porte ni policy ni fenêtre temporelle :

```json
{
  "version": 1,
  "action": "revoke"
}
```

Enregistrer le draft choisi dans un fichier privé, puis préparer une **nouvelle** opération :

```sh
agentsandbox broker policy prepare --inspection-dir "$NEW_INSPECTION_DIR" --input "$PRIVATE_DRAFT_JSON" --output-dir "$NEW_OPERATION_DIR"
```

La préparation est purement locale et retient le verrou du rapport jusqu’à sa fin. Elle copie origine, scope et sélecteur du rapport, emploie 0/0 **seulement pour un head explicitement absent**, conserve sinon les révision/époque exactes et génère un seul UUID pour cette nouvelle requête locale. Un remplacement peut utiliser un head présent `revoke_requested` avec son vrai CAS. Un remplacement Session exige ressource d’identité et plafond tenant actifs, puis copie les révision/époque/digest exacts du plafond. Revoke exige un head cible actif ; ressource ou plafond inactif/absent et fenêtre de l’ancienne politique ne bloquent pas la préparation locale du nettoyage. Autorité A1 finale, scope/CAS et contrôles d’horloge applicables restent obligatoires à la soumission. La révocation n’exige ni ressource Session/plafond tenant actif ni ancienne politique non expirée.

Le draft n’accepte ni `operation_id`, `scope`/`expected_scope`, `selector`, CAS, plafond, enveloppe request ou champ inconnu. Input borné à 66 560 octets ; requête canonique 65 536 ; observation du rapport, métadonnées/état local et reçu 4 096 chacun ; fichier token 4 096. Hashes, forme canonique, sélecteur et origine du rapport sont contrôlés ; rapport/draft/ancêtres sont revalidés autour de la publication. Un échec local ne répare, n’écrase ni ne supprime les preuves.

### Soumettre, lire l’historique local et reprendre explicitement

```sh
agentsandbox broker policy apply --operation-dir "$NEW_OPERATION_DIR" --token-file "$PRIVATE_RUNNER_TOKEN"
agentsandbox broker policy status --operation-dir "$NEW_OPERATION_DIR"
agentsandbox broker policy resume --operation-dir "$NEW_OPERATION_DIR" --token-file "$PRIVATE_RUNNER_TOKEN"
```

apply exige l’état initial préparé. Il retient le verrou local, revalide fichiers/token, compare un vrai diagnostic réservé au scope préparé, rend l’intention durable puis tente l’action préparée une fois. La transaction A1 finale revérifie scope authentifié et CAS courant, ainsi que le plafond pour un remplacement Session. Aucune inspection cachée, surcharge origine/scope/CAS, relance automatique, nouvel UUID, redirect/proxy ou repli public/legacy. Le SDK garde ses bornes 15 s/requête et 3 s/connexion. Le format 14 exige une maintenance explicite.

status est une preuve historique locale, pas une lecture du head serveur. Son JSON conserve operation_id, action, phase, attempts, last_error, receipt_present, pending_state et résumé receipt, avec historical:true et coverage:policy_registry_only. Phases : prepared, remote_outcome_unresolved et receipt_recorded ; attempts compte les intentions locales, pas les effets distants. Aucun token, vrai scope, origine, DNS, chemin privé ou champ runtime_applied n’est affiché. Un record incomplet/unsafe reste une erreur, jamais un historique vide.

resume conserve exactement UUID et octets de requête. Il peut publier une transition locale pending déjà retenue et validée ; reopen/status ne la réparent jamais implicitement. Un reçu canonique corrélé à l’action déjà présent est revalidé/resynchronisé et retourné comme historique sans autre POST ni nouvelle observation serveur. Sinon resume effectue un nouveau diagnostic réservé et au plus un POST de la même opération. Le travail accepté peut finir après perte de l’observateur ; ACK perdu, timeout, token changé, erreur transport/réponse invalide ou publication/sortie locale ne prouvent aucun rollback. Préserver tous les records, y compris les preuves partielles/unresolved.

### Refus, observations périmées et compatibilité de l’enveloppe complète

| Refus | Action opérateur |
|---|---|
| Credential absent/invalide, non-runner ou mauvais binding/source | Vérifier le credential technique et l’installation owned existants. Connexion humaine/rôle SSO ne s’y substitue pas. |
| `egress_policy_session_resource_unavailable`, `egress_policy_tenant_ceiling_unavailable`, `egress_policy_revoke_requires_active_head` | Examiner l’observation retenue et l’action demandée. La préparation n’invente ni ressource, plafond ou head. |
| `egress_policy_scope_conflict` / `egress_policy_conflict` (409 A1 final) | Conserver rapport et opération inchangés. Aucune réparation, nouvelle lecture ou relance automatique d’une observation périmée. |
| `egress_policy_migration_required` | S’arrêter et suivre la maintenance de confiance distincte ; aucune migration automatique. |
| Busy/capacité/horloge/source/audit indisponible | Conserver les preuves et diagnostiquer le vrai refus. Busy local peut signifier un verrou détenu sur rapport/opération, jamais une absence. |
| Records locaux unsafe/modifiés/incomplets, sortie dans le rapport ou publication/affichage unresolved | Préserver les fichiers et éventuelles preuves de staging. Ne pas éditer les records, relâcher les contrôles de modes, supprimer un reçu ou réutiliser une destination pour forcer un succès. |

Si des faits courants sont nécessaires après conflit, demander une **nouvelle inspection explicite**, l’examiner, puis décider séparément d’une nouvelle préparation dans un nouveau répertoire. Cela ne réécrit ni ne résout une ancienne opération ambiguë ; conserver ses preuves et utiliser la reprise correspondante si nécessaire. Le serveur reste l’autorité même pour un rapport modifié par son propriétaire mais aux hashes cohérents. Les états des reçus décrivent le registre : `revoke_requested` ne constate aucune fermeture runtime. Les enveloppes d’erreur sont décrites dans le [contrat API](../../api.md#owned-policy-a1).

L’ancien parcours par enveloppe complète reste disponible lorsqu’une vraie requête A1 complète est déjà retenue :

```sh
agentsandbox broker policy prepare --gateway "$GATEWAY_ORIGIN" --input "$PRIVATE_FULL_INPUT_JSON" --output-dir "$NEW_OPERATION_DIR"
```

Cet input est l’enveloppe version1/action/request fermée existante avec UUID d’opération, expected_scope, sélecteur et CAS explicites (plus policy/plafond pour replace). Voir le [contrat de requête A1](../../api.md#owned-policy-a1). Utiliser uniquement les vrais faits contrôlés ; ni scope/CAS devinés, ni requête SQLite comme contournement utilisateur. `--gateway` et `--inspection-dir` sont mutuellement exclusifs. Une enveloppe complète n’est pas un draft et aucun parcours ne bascule silencieusement vers l’autre format d’input. Les répertoires d’opération existants restent compatibles avec apply/resume/status.

Les barrières FS utilisent les fichiers/verrous retenus et synchronisent fichiers, répertoire d’opération et parent ; elles ne prouvent pas une reprise physique après coupure de courant. Les limites de confiance UID/root existantes demeurent. Ni mutation de credential, lecture complète du contenu d’une politique, médiation/révocation réseau runtime, qualification fournisseur/worker/cluster ou ouverture des gates broker 3–13/relais 6–13 ne découlent de ce parcours. Les preuves de qualification restent liées à leur source et périmètre réels.
