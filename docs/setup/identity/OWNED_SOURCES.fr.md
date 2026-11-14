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

Le credential admis est le bearer local privé déjà provisionné. Le premier GET `human identity` vérifie sa connexion ; aucun endpoint password/login n'est ajouté. Employer les [commandes identité/ressources existantes](README.fr.md#consulter-avec-le-cli), `--credential-file` privé et un contexte réellement reçu. Seules les lectures humaines `GET /api/v1/human/identity`, `GET /api/v1/human/resources`, `GET /api/v1/human/events` et `GET /api/v1/human/events/recent` sont adoptées ; les deux lectures d’événements sont limitées aux métadonnées. L'identité décrit la source authentifiante ; en présence d'autres sources tenants, `contexts_complete=false` signifie que leurs memberships restent inconnus. La sélection cross-source est refusée. La page de ressources porte son vrai `read_receipt` durable ; un candidat digest ne fournit ni identité, ni droit, ni reçu.

La lecture récente du Journal porte seulement sur les métadonnées. Une page réussie avec `stream_mode:"snapshot_only"` et `live_cursor:null` identifie ce profil owned actuel ; le marqueur n’accorde aucun droit et ne décrit pas un autre serveur/profil. Les curseurs des pages plus anciennes conservent leur sélection autorisée et leur borne descendante. L’interface affiche l’heure de lecture et le reçu réels de la dernière page lue. Direct, contenu, détail d’événement et export restent indisponibles pour ce profil, même si les permissions correspondantes figurent dans la projection des droits. Après une page Journal réussie annonçant ce profil, Contenu et Export expliquent cette limite et sont désactivés ; les droits accordés restent inchangés.

Utiliser **Actualiser** pour relire les événements récents. Le Journal n’ouvre ni flux, ni boucle de relance, ni lecture périodique ; le retour dans l’onglet conserve la revalidation ponctuelle normale. Une revalidation en échec retire les données affichées, reçus et positions de pagination du snapshot. Une erreur transitoire dans le même contexte conserve la restriction de profil déjà établie ; changement d’identité/contexte/autorisation ou fermeture de session retire ce constat. Un503 n’établit jamais un autre profil. Une demande explicite de contenu refusée reste une erreur avec retour explicite vers Ressources, sans repli silencieux metadata. Mettre à jour gateway et dashboard ensemble. La preuve logicielle datée en tête de ce guide conserve son périmètre d’origine de deux lectures.

Les méthodes connues non adoptées humaines, contrôle, workload, brokers, relais et OIDC, dont détail d’événement, export et flux, renvoient503 explicitement avant extracteurs legacy, corps ou Store. OIDC sélectionné dans le manifeste reste sélectionné et retenu, avec handlers/bearers fermés ; il n'est jamais annoncé désactivé. Les GET status/profile existants « disabled » sont admis seulement si OIDC est absent. La santé consulte la mémoire ; elle ne prouve pas la disponibilité de chaque source. Ces lectures n’activent ni approbation, ni lancement, ni permission réseau, ni contrôle egress modernes.

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

## A1 : opérations du registre de politiques

**Contrat A1 du registre de politiques.** Ces quatre commandes existent dans la [source aux preuves logicielles ciblées datées](../../engineering/evidence/2026-11-14/owned-egress-policy-api-cli/README.fr.md), `db2678d42cff9c8d6d15bf31284a989732cc6f0d`. Ces résultats ne se transfèrent pas à un autre HEAD et ne qualifient pas un parcours opérateur complet. L’acquisition du scope/CAS reste un écart ci-dessous ; `inspect` et `prepare --inspection-dir` ne sont pas implémentés. Le périmètre reste `policy_registry_only` : politique durable ou reçu `revoke_requested` ne prouvent ni filtrage réseau, permis de connexion, start worker/broker ni approbation humaine.

### Prérequis que ces commandes ne créent pas

Utiliser une installation owned déjà adoptée, avec **vrai Store15**, union complète retenue et credential technique runner déjà présent dans son registre privé **et binding actif vers ces source/tenant/namespace exacts**. Bearer humain nominatif, viewer, token workload ou rôle SSO ne fournit pas ce credential. A1 ne crée aucun credential/binding et ne migre rien. Le parcours d’adoption précédent convertit 13→14, pas 15 ; le [contrat de maintenance E1](../../engineering/EGRESS_POLICY_STORE.fr.md#format15-exact-et-maintenance) décrit le protocole de confiance existant, distinct. Ne pas fabriquer enveloppes, reçus ou lignes SQLite, ni employer les fixtures comme procédure d’installation. Sans ces vrais prérequis, s’arrêter ici et conserver les fichiers de l’installation.

> **Limite actuelle du parcours opérateur.** L’input exige tenant_id/control_namespace_uuid confirmés par un vrai diagnostic réservé authentifié et les faits exacts de head/CAS de politique. Ces quatre commandes n’acquièrent pas ces faits. Le broker control-info existant utilise le diagnostic public, fermé en owned ; status est un historique local, pas le head serveur courant. A1 ne fournit pas de commande d’acquisition supportée. Sans la vraie preuve de contrôle conservée, s’arrêter : ni déduction depuis noms/erreurs, ni copie d’un autre tenant, ni contournement Rust/SQLite utilisateur. Le raccord opérateur manquant reste une exigence distincte d’implémentation/qualification.

La valeur active du diagnostic est un fait de lease, pas un droit ; elle ne contient pas le head de politique. Un remplacement Session exige aussi les vrais plafond tenant et état actif de Session. Une autre opération peut périmer un reçu/status historique. Aucune lecture de politique courante n’est fournie ici. `expected_scope` est une assertion comparée, jamais un sélecteur de source/Store/tenant ou une autorité. Renseigner les préconditions exactes uniquement depuis une vraie preuve contrôlée ; aucun CAS par défaut ni réparation automatique. Choisir explicitement l’UUID d’une nouvelle opération ; tout retry/resume conserve son UUID et ses octets.

Conserver input et fichier token runner privés (0600), parent de destination privé (0700) déjà présent et destination d’opération absente. Son nom final est borné à 128 caractères ASCII alphanumériques/point/tiret/underscore, hors `.` et `..`. Input borné à 66 560 octets ; requête canonique 65 536 ; métadonnées/état local et reçu canonique 4 096 chacun. L’input peut contenir des métadonnées sensibles de destinations : le garder hors workspace utilisateur, frontend et artefacts publics. Le credential est lu dans son fichier privé, jamais passé comme valeur ou stocké dans l’opération.

### Renseigner l’input avant de l’utiliser

**Les deux objets JSON ci-dessous sont des modèles, pas des requêtes admises ni des exemples exécutables. Remplacer chaque champ null de scope/UUID/CAS/temps par la vraie valeur typée de l’installation.** Aucun exemple de CAS d’amorçage/par défaut n’est fourni ici. Pour replace, choisir une vraie fenêtre valide not-before/expiry en millisecondes Unix ; l’expiration suit le début. L’horloge durable et les règles du contrôle restent l’autorité. Le tableau de destinations vide illustre une politique de registre deny-all, pas un droit d’accès externe. Revoke ne porte ni policy/fenêtre ni digest attendu. Le sélecteur illustré est Tenant ; un remplacement Session exige aussi son UUID canonique, une Session active et le plafond tenant exact (révision/époque/digest), selon le [contrat API](../../api.md#owned-policy-a1).

```json
{
  "version": 1,
  "action": "replace",
  "request": {
    "version": 1,
    "expected_scope": {
      "tenant_id": null,
      "control_namespace_uuid": null
    },
    "operation_id": null,
    "selector": {
      "kind": "tenant"
    },
    "expected_revision": null,
    "expected_epoch": null,
    "policy": {
      "not_before_ms": null,
      "expires_at_ms": null,
      "destinations": [],
      "max_destinations": 128,
      "max_resolution_ips": 16
    },
    "tenant_ceiling": null
  }
}
```

```json
{
  "version": 1,
  "action": "revoke",
  "request": {
    "version": 1,
    "expected_scope": {
      "tenant_id": null,
      "control_namespace_uuid": null
    },
    "operation_id": null,
    "selector": {
      "kind": "tenant"
    },
    "expected_revision": null,
    "expected_epoch": null
  }
}
```

### Préparer, soumettre, lire l’historique local et reprendre explicitement

**Fixer d’abord ces variables aux vraies origines non secrètes et chemins privés ; le bloc n’est pas exécutable tel quel.** GATEWAY_ORIGIN est l’origine conservée à la préparation : HTTPS ou HTTP loopback, sans credentials URL/query/fragment ni chemin hors racine. PRIVATE_RUNNER_TOKEN désigne le fichier privé, pas son contenu. Les quatre commandes produit sont exactement :

```sh
agentsandbox broker policy prepare --gateway "$GATEWAY_ORIGIN" --input "$PRIVATE_INPUT_JSON" --output-dir "$NEW_OPERATION_DIR"
agentsandbox broker policy apply --operation-dir "$NEW_OPERATION_DIR" --token-file "$PRIVATE_RUNNER_TOKEN"
agentsandbox broker policy status --operation-dir "$NEW_OPERATION_DIR"
agentsandbox broker policy resume --operation-dir "$NEW_OPERATION_DIR" --token-file "$PRIVATE_RUNNER_TOKEN"
```

prepare n’appelle aucun réseau. Il valide version1/action/request fermés, canonicalise et conserve durablement origine/action/op/CAS/expected_scope/octets et hash de requête dans le nouveau répertoire avant soumission. Champ inconnu, UUID/limite invalide ou chemin unsafe refuse sans adopter/écraser d’anciennes données. Input/token et répertoire existant ne sont pas réparés. Après erreur, conserver l’opération complète et ses records partiels ; ne pas éditer son JSON pour forcer un retry.

apply exige l’état initial préparé. Il retient le vrai lock local, revalide fichiers/token, compare le diagnostic réservé réel au scope préparé, rend durable l’intention puis appelle le SDK de l’action au plus une fois. Un diagnostic réussi ne lie pas le POST ultérieur : le serveur vérifie expected_scope dans sa propre transaction. Aucune substitution origine/scope/UUID/CAS, retry automatique, redirect/proxy ou repli public/legacy. Le SDK garde ses bornes 15 s de requête/3 s de connexion. Format 14 exige maintenance explicite ; un vrai refus busy/source/clock/audit n’est ni absence ni permission de basculer.

status n’appelle aucun réseau et ne décrit que l’historique local. Un objet JSON et LF porte version, operation_id, action, phase, attempts, last_error, receipt_present, pending_state, résumé receipt, historical:true et coverage:policy_registry_only. Phases : prepared, remote_outcome_unresolved et receipt_recorded. attempts compte des intentions locales, pas des effets distants. Aucun credential, origine, scope réel, DNS, chemin privé, réponse/chaîne arbitraire ou champ runtime_applied n’est imprimé. Une commande en échec peut émettre l’état unresolved sûr seulement si persistance/sortie aboutissent ; sinon elle retourne une erreur fermée. Busy peut signifier qu’une autre commande détient le lock local, pas que l’opération est absente. Record partiel/unsafe reste une erreur, jamais un historique vide.

resume est explicite et conserve exactement la préparation. Il peut publier une **transition locale pending déjà retenue et validée** ; reopen/status ne le font jamais implicitement. Si un reçu canonique corrélé à l’action est déjà durable, resume le revalide/resynchronise avec le fichier token privé fourni puis retourne une preuve historique, sans POST ni état serveur fraîchement constaté. Sinon il effectue un nouveau diagnostic et au plus un POST de la même opération. Le vrai travail accepté peut finir après abandon de l’observateur ; ACK perdu, Transport/InvalidResponse, token changé ou erreur de sortie/publication locale ne prouve jamais un rollback. Conserver tous les records. Une erreur d’affichage ne supprime pas un reçu déjà stocké. Un ScopeConflict ultérieur n’efface pas un ancien essai unresolved ; ne pas changer scope/CAS ou générer un nouvel UUID pour le masquer.

### Reçu, refus et limites

Le SDK lit les octets E1 canoniques bornés et valide l’action. Replace corrèle opération/sélecteur/CAS, révision+1/époque+1, Active et digest recalculé. Revoke conserve la révision, exige époque+1/RevokeRequested et ne vérifie que la **forme** lowercase64hex du policy_digest, sans attente de valeur depuis un ancien reçu ou lecture de politique distante supplémentaire. revoke_requested est une demande au registre, pas une fermeture runtime constatée. Limites, réserves, horloge et audit restent côté contrôle ; fichier/DTO/status local ne fournit aucune autorité d’acteur.

Codes fermés : `egress_policy_scope_conflict` et `egress_policy_conflict` (conserver préparation, sans réparation CAS), `egress_policy_migration_required` (aucune migration automatique), `egress_policy_busy`, `egress_policy_clock_unavailable`, `egress_policy_unavailable` ou `egress_policy_capacity_exhausted` (aucun repli), `egress_policy_transport_unresolved`/`egress_policy_invalid_response` (préserver l’ambiguïté et reprendre explicitement). Les erreurs locales unsafe/incomplètes ou de durabilité refusent aussi sans cleanup. Détails HTTP/pré-body dans le [contrat API A1](../../api.md#owned-policy-a1) ; un scope d’erreur n’est pas une découverte faisant autorité.

La barrière FS in-process emploie vrais fichiers/locks retenus et synchronise fichiers, répertoire d’opération et parent contenant. Elle établit une nouvelle barrière locale, sans prouver le succès d’un ancien sync ni une reprise physique après perte de courant. Les limites de confiance UID/root existantes demeurent. Aucune mutation de credential humain, API de lecture de politique, médiation/révocation réseau runtime, qualification fournisseur/worker/cluster ou ouverture broker 3–13 / relais 6–13 n’en découle. La preuve datée couvre le logiciel ciblé sur sa source exacte. Acquisition fraîche du scope/CAS, cinq tests d’inspection ultérieurs, runtime/réseau réels et qualification production restent NOT_RUN ; le parcours opérateur incomplet reste une exigence ouverte.
