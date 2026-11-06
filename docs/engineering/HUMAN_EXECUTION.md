# Demande humaine et exécution attribuée — contrat proposé

6 novembre 2026 — **PHASE 1 révisée / SourceReady pour revue, aucune fonction nouvelle livrée.**
Référence source : `d3300641b1d053a02b4c9a3f6a6700dc6b5598f3`, branche `codex/human-execution-20261106`.
Ce contrat applique [architecture](ARCHITECTURE_CONTRACT.md), [code](CODING_RULES.md), [identité](../sso.md),
[UI](../ui.md), [supervision](BROKER_SUPERVISION.md) et [tenants](CLOUD_TENANCY.md).
Les [gates](PRODUCTION_READINESS.md) restent ouverts, notamment G09 FAIL ; aucun test/runtime n'est exécuté ici.

## Besoin et constat vérifié

Livrer le même parcours Web et CLI : identité locale/OIDC → demande humaine autorisée → broker autorisé →
vraie exécution isolée attribuée → résultat, fichiers et export visibles dans le périmètre personnel.
Une réponse HTTP acceptée, un job journalisé, un heartbeat ou un identifiant de session ne prouvent pas ce parcours.

| Source actuelle | Fait vérifiable et couture à compléter |
|---|---|
| [`identity/types.rs`](../../src/identity/types.rs), [`authorization.rs`](../../src/identity/authorization.rs) | Principaux locaux `(installation_id, account_id)` et OIDC `(issuer, subject)` distincts ; `HumanCapabilityV1` contient huit droits de lecture/administration, aucun lancement. `AuthorizedScopeV1` est un snapshot revalidé, pas une autorisation durable d'exécution. |
| [`api/human.rs`](../../src/api/human.rs), [`human_state.rs`](../../src/api/human_state.rs) | `VerifiedHuman`, revalidation local/OIDC, plafond de claims et contexte opaque `x-aiguard-context` existent ; routes humaines exclusivement GET. |
| [`audit/identity.rs`](../../src/audit/identity.rs), [`human.rs`](../../src/audit/human.rs) | Comptes/memberships/grants révisionnés et ACL avant pagination/SSE/export. La première attribution d'une ressource est réservée au contrôle ; elle exige déjà un événement la référençant. Ce n'est pas un contrat de création de session humaine. |
| [`api/human/workers.rs`](../../src/api/human/workers.rs), [`audit/human_workers.rs`](../../src/audit/human_workers.rs) | Schéma10, droit administratif `read_worker_registry`, projection whitelistée et reçu durable ; `session_provenance=declared`, sans propriétaire humain ni délégation authentifiée. |
| [`identity_cli.rs`](../../src/identity_cli.rs), [`identity_cli/oidc_session.rs`](../../src/identity_cli/oidc_session.rs) | Provisioning et lectures/export locaux ; `human sso` garde les tokens en mémoire au premier plan. Aucune commande humaine d'exécution. |
| [`broker_cli.rs`](../../src/broker_cli.rs), [`service/protocol.rs`](../../src/broker/service/protocol.rs), [`service/profile.rs`](../../src/broker/service/profile.rs) | IPC `Submit/Approve/Cancel/Resume/Export` réel ; `SubmitV1.project` et `Export.destination` sont des chemins opérateur, pas des entrées humaines sûres pour le Web. |
| [`service/mod.rs`](../../src/broker/service/mod.rs), [`local_ipc.rs`](../../src/broker/local_ipc.rs) | `VerifiedProfile::authenticate` exige rôle `runner`, tenant/namespace confirmés et protocole courant ; UID/socket privés ne prouvent pas le tenant. Un JWT humain n'est pas accepté par cette voie. |
| [`service/server.rs`](../../src/broker/service/server.rs), [`runner/broker/executor.rs`](../../src/runner/broker/executor.rs) | Préparation, journal/bindings, owners Full, décision/admission, helpers, armements, claims et observations sont raccordés. `require_starts`, fencing et capacité précèdent les nouveaux effets ; leur présence ne qualifie pas D5. `Execution::client` est aujourd'hui un `AuditClient` runner concret. |
| [`api/supervision.rs`](../../src/api/supervision.rs), [`runner.rs`](../../src/runner.rs) | Mutations de supervision sous `RunnerIdentity`/`AuditClient` ; `human_approved` est un booléen/hash de phase, sans principal humain authentifié. |
| [`workload.rs`](../../src/workload.rs), [`api/workload.rs`](../../src/api/workload.rs), [`api/brokers.rs`](../../src/api/brokers.rs) | Preuves broker/worker séparées, rotation/révocation et `VerifiedBroker` non forgeable ; registre/grants/heartbeat/identité existent, aucune distribution de demande humaine exécutable. |
| [`broker/presence.rs`](../../src/broker/presence.rs), [`broker_registry.rs`](../../src/broker_registry.rs) | Publication de présence indépendante ; autorité et runtime du registre restent déclarés. Un bail courant ne prouve ni capacité Starts/Full disponible, ni inventaire runtime. |
| [`model.rs`](../../src/model.rs), [`front/src/human-model.ts`](../../front/src/human-model.ts) | Les événements n'ont pas de chaîne typée initiateur/délégation authentifiée ; le front humain distingue déjà permissions, provenance, pages et fraîcheur. Aucun rattachement personnel par nom/session à ajouter. |

## Choix de médiation et frontière

| Approche réelle | Conséquences |
|---|---|
| IPC local recevant une preuve humaine et faisant valider la demande au gateway | Adaptable au CLI local, mais le navigateur ne joint pas ce socket. Un proxy conteneur→socket ajouterait une exposition et une seconde voie de lancement ; l'IPC courant exige runner. Ne résout pas seul le cloud, la demande sans broker ou la visibilité durable. |
| Demande durable dans le gateway, tirée par un broker authentifié | Même autorité SQLite par tenant pour Web/CLI/local/cloud, pas d'entrée réseau sur le broker ; reprise avec IDs stables et état sans broker explicite. Nécessite protocole de préparation/claim/reçu et raccord à l'executor, pas seulement une file ou un endpoint. **Recommandé.** |

Le gateway Rust reste autorité humaine/politique/audit ; le broker Rust reste propriétaire des effets runtime.
Le dashboard Rust relaie seulement des routes fixes et sert le TS compilé ; il ne reçoit ni clé runner ni socket broker/moteur.
La CLI humaine utilise les mêmes API que le Web ; l'IPC technique existant reste distinct et compatible.
Le broker tire les demandes avec sa preuve workload propre, jamais avec le JWT humain ou un rôle humain converti en runner.
Les éventuels credentials runner conservés pour les parcours historiques restent privés au contrôle et hors des demandes.

Le parcours humain et ses permissions sont déjà mandatés ; cela n'approuve pas par avance un nouveau port privilégié.
Le transport durable et les ports fermés ci-dessous restent une proposition soumise au gate Root avant tout code/scaffold.
Ils visent les binaires et SQLite existants, sans microservice, Postgres/NATS, échange OIDC contre runner ni nouvelle autorité d'identité.
Toute exposition du socket au dashboard, nouvelle frontière réseau, HA/stockage partagé ou activation cloud/PKI serait une décision structurelle séparée.
Le transport multi-hôtes et Kubernetes V2/réseau P1 restent à qualifier : les métadonnées existantes ne les rendent pas disponibles.

## Autorité humaine et ressources

PROPOSÉ : `HumanExecutionContextV1` et `HumanExecutionAccessV1`, distincts de `HumanContextV1/HumanCapabilityV1`, exposent des droits effectifs fermés :
`request_execution`, `approve_execution_review`, `cancel_execution`, `resume_execution`, `publish_workspace_export`.
Les lectures métadonnées/contenu/export d'audit conservent leurs droits existants ; publier un workspace est une autre mutation.
Ces droits ne découlent ni de l'administration ni de la propriété seule, et ne sont jamais ajoutés aux anciens défauts de provisioning.
Ils sont établis par grants de contrôle révisionnés, avec principal, membership/organisation, ressources/profils autorisés et expiration.
Le plafond OIDC d'exécution exige un mapping serveur explicitement approuvé ; absent/ambigu = aucun droit d'exécution.
Le mode local applique les mêmes grants/plafonds administrés sans IdP. `manage_memberships/manage_grants` n'autorise pas leur auto-promotion.
`request_execution` permet à l'initiateur d'autoriser son plan ; un autre reviewer est un acteur distinct explicitement habilité sur cette demande.
Un contexte d'exécution est proposé pour un compte/membership actif avec grant d'exécution seul : aucun `ReadMetadata` implicite ni passage par `authorize_context` de lecture.
Son handle opaque lie principal/preuve, membership, plafond OIDC/mapping et versions des grants ; le contexte de lecture reste indépendant.
Le statut minimal d'une demande propre expose ID, phase, motif/reprise et reçus autorisés, pas sorties, fichiers, prompts, commandes ou contenu tiers.
PROPOSÉ : `HumanExecutionConfirmationV1` est une vue séparée du plan exact de la demande qu'on peut autoriser/reviewer sous grant d'exécution ; cancel/resume seuls n'y donnent pas accès.
Projection fermée : commande/spécification liée à cette demande, références source/profil sans chemins privés, client/mode/modèle et coût API potentiel sans prix inventé,
ressources/durée/réseau/image/runtime, broker/autorité, couverture/motifs, hash/révisions/échéances/reçus ; aucun secret, contenu tiers, droit global `ReadMetadata` ou accès au registre.
ACL/contexte/plafond/révisions et audit sont vérifiés avant publication ; la confirmation engage le hash du plan exact présenté, jamais celui d'une autre projection masquée. Un champ d'autorité sensible impose refus/nouveau plan, pas masquage silencieux.
Les métadonnées de supervision et contenus exigent leurs grants/capacités de lecture distincts, même pour l'initiateur.

Chaque opération revalide compte, membership, contexte/version des droits, grant de ressource et plafond de preuve dans la transaction.
Le tenant provient uniquement de cette identité ; le corps ne contient pas de tenant, owner, initiator ou permission à faire confiance.
La configuration de contrôle autorise les sources, destinations d'export, brokers/autorités, profils, images et clients/modèles.
Le client choisit des références opaques proposées, pas une URL, un socket, un credential, un chemin hôte, un nom de volume ou un programme moteur.
Le broker résout les références dans sa configuration privée révisionnée et ouvre des capacités `AuthorizedSource/AuthorizedExportParent`.
`AuthorizedSource` retient des descripteurs/identités de répertoires ; il ne fige pas leurs contenus ([source](../../src/workspace/authorized.rs)).
La demande autorise explicitement une préparation en lecture bornée : sélection filtrée, plafond fichiers/octets/temps et exclusions privées ; aucun outil métier ni import.
Le manifeste proposé est optimiste, daté et hashé. Après autorisation, archive puis manifeste réellement importé doivent correspondre exactement à ce manifeste.
Vérifier l'archive avant tout helper Import, puis le manifeste importé avant le claim du worker ; une divergence ne peut lancer l'étape suivante.
Substitution, mutation concurrente ou divergence avant/après copie = nouveau plan et nouvelle approbation, aucun start métier ; aucune atomicité filesystem prétendue.
Les changements métier futurs restent limités à cette copie ; exporter crée une destination neuve, jamais le projet original.

L'intersection effective borne CPU/RAM/PIDs, concurrence par principal et tenant, durée, volumes, réseau, sortie capturée, clients et modes IA.
Quotas globaux restent ceux du tenant, sans plafond multiplié par broker ; les grants de ressources ne remplacent pas la réservation durable.
Sélection IA explicite prioritaire, résolution `auto` visible avant autorisation, abonnement/API distincts sans repli facturé implicite.
La référence et le digest du binding de credential sont figés ; seuls les bytes du mode IA retenu passent au handoff privé, hors intent public, argv et audit.
Un profil Internet non qualifié est refusé ; cette verticale ne déclare pas le contrôle des destinations P1 disponible.

## Objets et API proposés — aucun de ces nouveaux contrats n'est livré

`HumanExecutionRequestV1` : UUID opération client, intention utilisateur bornée, références source/profil/broker, sélection IA/commande structurée, bornes demandées, mode batch/TTY explicitement pris en charge et échéance de demande.
Le serveur fixe request/session/agent racine, principal initiateur, contexte/membership/révisions et hash canonique du contenu validé.
`ResolvedHumanPlanV1` : hash de demande, broker/incarnation/autorité et profil révisionnés, résolution IA, image/runtime, ressources/copie/limites/réseau, politique/version, hashes d'intention/bindings sans secret, compatibilité/couverture et dates.
`HumanExecutionAuthorizationV1` lie exactement ce plan au principal, aux grants/plafonds, à une fenêtre de lancement et à la durée maximum.
`BrokerHumanClaimV1` lie demande/autorisation/hash/namespace à un broker, incarnation, autorité et génération de livraison uniques.
Un DTO reçu n'est jamais une capacité : seuls les extractors authentifiés et validations transactionnelles construisent les handles privés.
Chaque commande possède UUID, hash d'entrée, révision attendue et reçu durable ; même identité divergente = conflit, y compris après redémarrage.
Le rejeu exige le même principal et les droits courants ; l'UUID seul ne permet ni lecture ni mutation et ne révèle pas une demande privée tierce.

| Surface PROPOSÉE | Contrat observable |
|---|---|
| `GET /api/v1/human/execution-access` | Proposer des contextes d'exécution et choix effectifs, y compris grants seuls, avec plafond/provenance/limites/version/fraîcheur/reçu ; aucun droit de lecture implicite. |
| `POST /api/v1/human/execution-requests` | Enregistrer une demande de préparation autorisée ; 201 neuve/200 rejeu exact, état non exécutable. |
| `GET /api/v1/human/execution-requests[/{id}]` | Statut minimal propre/partagé selon grant d'exécution ; autres détails de supervision/contenus seulement si droits de lecture distincts. ACL avant pagination, reçu obligatoire. |
| `GET /api/v1/human/execution-requests/{id}/confirmation` | Plan fermé de cette demande pour l'initiateur habilité à l'autoriser ou son reviewer autorisé, même sans droit de lecture général ; contrôles/publication ci-dessus, hash exact présenté. |
| `POST /api/v1/human/execution-requests/{id}/commands` | Union fermée `authorize_plan/approve_review/cancel/resume/export`, UUID/hash/révision ; droits propres et revalidation. `authorize_plan` porte le hash exact du plan affiché. |
| `GET /api/v1/brokers/self/human-requests` | Lecture bornée par `VerifiedBroker`, uniquement les demandes destinées à son tenant/broker/autorité et autorisées pour la phase demandée. Aucun claim par GET. |
| `POST /api/v1/brokers/self/human-requests/{id}/commands` | Union fermée `claim_preparation/record_plan/claim_execution/record_acceptance/record_result` et ACK exacts ; identité workload revalidée avant chaque transaction. |
| `POST /api/v1/brokers/self/human-requests/{id}/effects` | PROPOSÉ, gate Root : union fermée des opérations de supervision, référence de plan/claim/phase et input exact ; chaque mutation/replay exige une capacité transactionnelle opaque. Aucun chemin HTTP arbitraire ni route runner générique. |
| CLI `human execution …` | `access/request/list/status/authorize/approve/cancel/resume/export` pour credential local privé ; mêmes commandes dans la session `human sso`, tokens mémoire, contexte proposé seulement. Identités/reçus conservés pour reprise, jamais de token dans argv. |

Les noms restent proposés ; implémentation, aide CLI et guides EN/FR doivent évoluer ensemble.
Décodage fermé depuis bytes bornés : versions/champs/doublons/nombres/UUID invalides refusés, pas de normalisation qui masque une divergence.
Si le masquage altère un champ d'identité ou d'autorité, refuser avant persistance ; ne pas autoriser un hash d'intention publique masquée différent du travail exécuté.
Délais de corps, connexions, queue, requêtes préparées et tâches sont bornés ; permissions vérifiées avant lecture coûteuse du corps.
Le proxy dashboard n'autorise que ces routes/méthodes fixes, origine approuvée, corps borné et protection CSRF ; aucun proxy POST générique.
Les enveloppes humaines V1 actuelles et leurs huit capacités restent inchangées ; l'accès d'exécution est une réponse séparée négociée.
Les cursors lient principal/contexte/version/filtre/limite, distincts de ceux des registres ; les révisions/séquences sont des décimaux exacts.

## Port interne délégué proposé et appels concrets

`VerifiedBroker` authentifie seulement. `Store::with_human_execution_capability` ouvre la transaction vérifiée, revalide demande/plan/phase/tenant,
broker/incarnation/autorité, binding/grants/révisions/temps et construit `AuthorizedHumanMutation<'tx>` opaque, non sérialisable, sans constructeur public.
Le contrôle et la recherche de replay passent ensuite **dans cette même transaction** ; un callback exécuté seulement pour une mutation neuve est insuffisant.
Extraire les noyaux transactionnels nécessaires des méthodes Store existantes, sans appeler une méthode qui relocke/commit depuis cette transaction.
Les façades legacy conservent leur autorité propre ; aucune promotion broker→runner, fausse `VerifiedProfile`, fallback ou appel `AuditClient::new` avec JWT/workload.

| Appelant source → API interne PROPOSÉE | Raccord précis et autorité |
|---|---|
| `api/human/execution.rs` → `Store::execution_contexts/create_human_request/apply_human_command/minimal_human_status/human_plan_confirmation` | Preuve humaine validée, contexte d'exécution séparé, plafond/grant courant ; création/audit/statut réellement utiles même sans broker, confirmation du seul plan autorisable/reviewable. |
| `service/human_requests.rs` → `HumanDelegatedClient::claim_preparation/record_plan/claim_execution` | Transport workload vers routes fermées ; réserve owners/permits et rend durable le Pending local exact avec échantillon `HostClock` **avant** chaque claim central. |
| `ConfirmedHumanClaim::open_context` → `AuthenticatedBatchContext` → `BatchExecutor::run/export` | Constructeur humain privé après confirmation du Pending/reçu exact et configuration retenue ; constructeur legacy actuel conservé. Claim de livraison ≠ Arm ≠ `FreshStartPermission`. |
| `AuthenticatedBatchContext.client/client()` → `BrokerControlPort` → `Execution::open` | Porter l'union aussi dans le champ/retour aujourd'hui `AuditClient` ([service/mod.rs](../../src/broker/service/mod.rs)) et sa copie `clone().with_secrets(...)` ([executor/journal.rs](../../src/runner/broker/executor/journal.rs)) ; préserver variante/capacités/redactor, sans reconstruire un client runner. |
| `Execution::call` → `BrokerControlPort::{Legacy(AuditClient),Human(HumanDelegatedClient)}` | Remplacer le client concret par cette union privée et des méthodes typées ; conserver redactor/bornes/reçus. La variante Human n'expose ni `get_json(path)` ni `post_control(path)`. |
| `rpc_checked_at_head` → `BrokerControlPort::validate_replay` | Son retour anticipé actuel sur ACK ne prouve pas les droits courants : pour Human, contrôle transactionnel de la phase/reçu exact avant réutilisation autorisante ; sinon donnée historique seulement, aucun effet permis. |
| `Decision/Admission/WorkerMutation` → `decide/admit_supervised/mutate_worker` du port | Côté Store : noyaux de `decision`, `admit_supervised`, `mutate_worker`, sous `AuthorizedHumanMutation` ; revalider au commit après tout avis politique hors transaction. |
| `Arm/WorkerClaim/HelperClaim` → `arm/claim_worker/claim_helper` du port | Noyaux `arm_execution/claim_armed_start/claim_helper_start`, autorité et phase exactes ; préserver Pending/HostClock/armement et `EmitOnce/AlreadyConsumed` existants. |
| `HelperObservation/HelperCleanup/Export` → `observe_helper/cleanup_helper/request_export` du port | Noyaux `observe_helper/record_helper_cleanup/request_export`, permissions de phase, cibles/manifest exacts et audit atomique. |
| `results.rs`/outbox/reprise → `read_worker/emit_event/reconcile_result/authorize_recovery_scan` du port | Aucune lecture/ingestion générique ; `results::final_scan` conserve l'orchestration du helper sous capacité exacte ci-dessous, reçus/ACK conservés, droits de lecture UI indépendants. |

Le Pending durable conserve UUID/hash, plan/phase, broker/incarnation/autorité, révisions, échantillon de boot/horloge et fenêtre proposée avant RPC.
Réponse perdue : reprendre cet objet exact ; aucune nouvelle horloge/durée/génération au retry. Le gateway vérifie son propre temps durable.
L'Arm reste construit après inspection de la cible avec son propre échantillon exact ; aucun claim de livraison ne permet de sauter ce parcours.

## Séquence complète et obligations avant effets

1. Web/CLI authentifie l'humain et sélectionne le contexte d'exécution proposé ; aucun contexte/capacité de lecture n'est exigé implicitement.
2. `request` vérifie droit/grants/plafonds et écrit demande, initiateur, IDs et audit atomiquement. Aucun worker/helper/copie runtime lancé.
3. Un broker workload autorisé réserve owners/permits et Pending/HostClock avant claim préparation TTL/CAS. Sans broker : `waiting_for_broker`, expiration possible, aucun lancement hôte.
4. Il exerce uniquement la préparation en lecture autorisée, résout client/prérequis et manifeste filtré optimiste ; la configuration est engagée, les contenus source ne sont pas gelés.
5. Le contrôle vérifie le plan et ses engagements ; Web/CLI affiche choix IA, coût API potentiel sans prix inventé, limites, source, broker et couverture. Le plan n'est toujours pas exécutable.
6. L'humain autorise le hash exact. La transaction revalide droits/grants/politique/temps, crée l'autorisation et son audit ; tout changement de plan, politique applicable ou choix résolu exige une nouvelle autorisation.
7. Le broker réserve owners/permits et Pending exact/HostClock puis claim l'autorisation avec son identité courante ; contrôle transactionnel grant/binding/incarnation/autorité/TTL/révocation et audit avant réponse.
8. Il confirme ce Pending, persiste attribution/intention/bindings/checkpoint avant acceptation ; archive et import sont comparés au manifeste autorisé. Divergence = nouveau plan/approbation, sans start métier ; reçu perdu repris aux mêmes IDs.
9. Le port typé réutilise `BatchExecutor` ; décisions/quota et chaque mutation **ou replay** passent sous capacité transactionnelle de phase. Un receipt/hash ou `VerifiedBroker` seul n'autorise rien.
10. Helpers Import/ImportVerification/FinalScan/Export et worker suivent chacun create/inspect → Arm durable → claim irréversible → start ; la copie est l'effet du helper autorisé. Starts/Full/enforcer/cancel-generation/fences restent obligatoires ; un replay ne reconstruit jamais `FreshStartPermission`.
11. Review reste une décision distincte : approbation authentifiée du hash/plan/reçu exact, droits courants et audit avant reprise ; aucun booléen `human_approved` client ne suffit.
12. Observations runtime, résultat et changements recoupés rejoignent l'audit/ledger puis la vue personnelle ; export explicite après terminal admissible vers une nouvelle destination autorisée, avec manifest/reçu durable.

L'enregistrement crée atomiquement un événement de demande référençant la session puis son grant initial de contrôle au principal initiateur.
Adapter explicitement `ensure_resource_known`/projection à ce fait authentifié ; ne pas fabriquer un `session.started` pour satisfaire la condition.
Les mutations marquées humaines refusent tout chemin sans cette liaison, même lorsqu'un appelant technique authentifié invoque le contrôle.
Le chemin legacy reste une classe distincte sans initiateur humain inventé ; plans fournisseurs1/2/3, bindings et claims existants restent intacts.

## Temps, révocation, concurrence et reprise

La preuve humaine doit être valide lors de la demande/autorisation ; logout et expiration de token ferment les nouvelles actions de cette session.
La délégation durable est une autorité séparée, bornée explicitement au plan/ressources/temps ; son statut et son échéance sont affichés avant confirmation.
`launch_not_after` borne les nouveaux starts métier selon demande/grants/politique/broker ; `delegation_not_after` borne le travail déjà accepté.
TTL de livraison, fenêtre de lancement, délégation et deadline Arm/runtime sont distincts ; aucun refresh/retry ne les prolonge.
Logout/expiration de token ne valent ni révocation de cette délégation indépendante ni arrêt ; proposer l'annulation explicite et garder cette règle visible.
Retrait de compte/membership/droit/grant source ou broker : révoquer la délégation, refuser nouvelles phases métier et demander STOP durablement.
`Store::with_recovery_capability` revalide une identité de contrôle courante et un grant de récupération exact, distinct des droits retirés :
demande/plan/worker/attempt/CID/volume/Arm/phase/révisions, opérations et bornes de temps/volume ; aucune adoption automatique d'une preuve broker révoquée.
Cette capacité permet STOP, observation/terminal, résultats/outbox/ACK et réconciliation sans nouveau start métier ; elle ne dépend pas d'un token humain vivant.
**FinalScan après révocation** est explicitement autorisé après terminal admissible comme helper de lecture bornée du volume de session retenu, sans source hôte/réseau/provider/export,
avec politique/Arm/claim/audit propres et contrôle Starts/Full applicable ; ce start de helper n'est jamais une reprise de worker/Import ni un élargissement de mission.
Sans capacité de récupération, preuve de cible ou disponibilité enforcer : état partiel/`in_doubt`, résultats différés et quota conservé ; aucun terminal inventé.
Un claim émis avant révocation peut encore avoir un effet tardif : STOP/D5 restent nécessaires, sans promesse de révocation instantanée du moteur.
Une panne du contrôle interdit nouvelle décision/claim ; les arrêts précisément préarmés restent indépendants et les observations attendent l'ACK.

| Situation | Réponse et preuve requise |
|---|---|
| Absence de broker, bail périmé, capacité Starts/Full fermée | État d'attente ou blocage motivé, échéance visible, reprise/cancel possibles selon droits ; aucun faux démarrage ni auto-activation. |
| Audit/budget/stockage indisponible | Refus avant nouvel effet ; budget humain par principal/tenant et budget workload explicites, réserves STOP/identité conservées. Une réserve logique ne prouve pas capacité disque. |
| Double clic, réponse perdue, deux brokers | Même UUID/contenu rejoue le même reçu ; CAS/claim unique ; conflit divergent. Réserver avant préparation/acceptation, sans multiplier copie, quota, modèle ou start. |
| Broker perdu après claim/acceptation | Même request/session/worker/attempt/Arm/claim à réconcilier ; expiration du bail ne prouve pas absence d'effet. Aucun réassignement/second start sans preuve de fencing admissible. |
| Droits retirés pendant réponse/flux | Réauthentifier avant publication, fermer flux et purger données/détails/caches ; le reçu peut exister sans livraison. STOP demandé reste distinct de terminal observé. |
| Runtime inconnu/Created/start RPC ambigu | `in_doubt`, quota et cibles conservés, dernière observation datée ; ni absence, ni timeout, ni ACK Cancel ne donnent succès/terminal/libération. |
| Annulation avant effet prouvée | Transaction liée à la même identité, réservation libérée uniquement selon les règles existantes ; aucun recyclage de l'UUID. |
| Export interrompu ou destination existante | Reprise exacte du même export/manifest ou conflit explicite ; aucune fusion/écrasement/adoption d'un arbre inconnu. |

## Attribution, lecture personnelle et couverture

Le contrôle fixe `initiator_principal`, `human_request_id`, session, agent racine et autorisation ; chaque événement distingue
acteur effectif, parent causal, décision, acceptation, observation et résultat. Les champs d'agent ne peuvent remplacer ces liens de contrôle.
Une délégation vérifiée lie parent/enfant, tenant, mission/hash, permissions sous-ensemble, ressources, durée et révocation ; pas d'élévation.
Le credential d'exécution est propre, court et limité à sa médiation ; aucun JWT humain ou runner dans worker/agent/sous-agent.
Un sous-agent sans identité et canal de médiation prouvés reste `declared` ; son log ne prouve ni instruction humaine explicite ni contrôle transitif.
Les événements nouveaux portent une attribution versionnée séparée ; ne pas modifier les hashes historiques ni promouvoir `declared` en `observed`.
Conserver demandes/réponses visibles, outils/argv structurés, motifs, sorties et changements disponibles, avec masquage avant persistance,
contrôle d'accès/rétention, bornes et marqueurs de troncature/perte ; aucun raisonnement fournisseur caché promis comme observable.
L'audit indépendant, ses exports vérifiables et points de contrôle signés restent des obligations G06/G07, pas un résultat de ce document.

La vue personnelle relie uniquement la demande contrôlée à ses sessions/agents/workers et grants propres/partagés ; aucun join libre par nom.
L'administrateur utilise ses permissions distinctes ; `read_worker_registry` n'accorde ni contenu privé ni lancement ni propriété.
Afficher demande/plan/autorisation/acceptation/start observé/terminal/changements comme faits séparés, source/broker/autorité et dernière fraîcheur.
Chargement, vide confirmé, préparation, Review, refus, erreur, partiel/périmé et déconnecté ont chacun motif et action réellement disponible.
REST/détails/recherche/export/SSE appliquent les mêmes ACL avant sélection ; aucune fuite via compte global, curseur ou URL directe.
Le changement de droits/contexte invalide lectures et générations ; une réponse tardive ne repeuple pas la vue abandonnée.
Préserver OVPT, thèmes, clavier/focus, mobile320/390 et logs denses ; les captures doivent correspondre aux vrais reçus et IDs.
TTY Web/CLI exige son protocole d'attach autorisé borné et la supervision/gardien existants ; le batch ne qualifie pas une conversation interactive.

## Stockage et compatibilité proposés

Une migration explicite **10→11 proposée, numéro non réservé**, est nécessaire : registre durable des demandes/autorisation/claims/liaisons et grants d'exécution,
index de projection personnelle, journal d'opérations et marker de capacité ; ces faits n'existent pas au schéma10.
Même SQLite par tenant ; transaction vérifiée et audit atomique, reconstruction/reopen recoupés contre l'audit et namespace.
Sauvegarde privée exacte, contrôle arrêté confirmé, destination neuve et refus audit plein/rollback ; aucune migration/activation implicite.
Les événements et propriétaires historiques restent inchangés ; schéma<11 refuse le nouveau protocole avec `migration_required`, `SubmitV1` n'acquiert aucun droit humain.
Versionner également les enveloppes privées d'intention/attribution et le protocole broker délégué, sans changer les bytes des plans existants.
Préserver lecture/replay legacy ; binaire ancien incapable du11 refuse proprement, retour au10 uniquement par restauration sauvegardée et fencing.
L'API d'accès séparée évite de glisser des capacités inconnues dans `HumanContextV1` ; la configuration/mapping d'exécution reçoit sa version explicite.

## Prérequis H1a vérifiés en source — aucune admission du schéma11

L'inventaire au gel `f0fa2ea` relève trois contraintes supplémentaires à traiter
avant tout code qui accepte11. Ce sont des travaux proposés, sans preuve runtime :

- L'admission OIDC actuelle exige une intersection non vide de `HumanCapabilityV1`
  ([gateway](../../src/api/oidc/gateway.rs)). Un nouveau contexte ne suffit pas
  aux grants seuls : prévoir plafond, mapping/configuration, admission, refresh
  et revalidation d'exécution distincts, sans droit de lecture compensatoire.
  Un mapping V1 existant ne donne aucun droit d'exécution. Le format du ledger
  d'authentification est indépendant : sa compatibilité reste à examiner.
- Les nouvelles demandes/statuts/replays exigent une classe et une politique
  explicites dans [capacity](../../src/audit/capacity.rs), avec plafonds événements
  et octets par principal et tenant, reconstruction du cache et propagation aux
  clones. Aucune consommation de la réserve `IdentityMaintenance` ni faux
  événement de lecture ; absence de politique explicite = exécution désactivée.
- Le Pending [présence](../../src/broker/presence/storage.rs) conserve l'identité
  exacte, dont `schema_version`. Après10→11, l'égalité contrôlée par
  [l'acteur](../../src/broker/presence/actor.rs) échoue pour un Pending10, même si
  le hash heartbeat reste compatible. Résoudre exactement sous10 avant upgrade ;
  Pending non résolu = maintien10/réconciliation, sans réécriture ni omission de
  version. Pour un broker distant, silence et attestation opérateur ne prouvent
  pas l'absence de Pending. Un franchissement compatible demande son propre
  contrat et ses preuves ; il reste NOT_RUN.

## Lots verticaux, propriétaires et vérifications ciblées

| Lot dépendant | Fichiers concrets et acceptation ; QA Luna après autorisation Root |
|---|---|
| H1 — demander, préparer, autoriser et relire | Nouveaux `src/control/human_execution.rs`, `src/audit/human_execution.rs`, `src/api/human/execution.rs` ; coutures `identity/{types,authorization}.rs`, `audit/{identity,capacity}.rs`, `audit.rs`, `api/human.rs`, `oidc/config.rs`, `main.rs` et `gateway_config.rs`. Inclure migration11, grants/plafonds, demande/plan et ACL personnelles ; nouveaux tests intégrés HTTP/SQLite/migration A/B + deux utilisateurs du même tenant. Aucun endpoint « launch » réputé livré. |
| H2 — consumer broker jusqu'à effet réel | Nouveau `src/broker/service/human_requests.rs`, adaptation bornée `service/{mod,profile,server}.rs`, `runner.rs`, `runner/broker/{executor.rs,executor/*}`, `api/supervision.rs` et ports Store. Relier claim/attribution/executor/bindings/owners, annulation/reprise/export ; préserver API legacy et refus Starts/Full/STOP/D5. Tests vrais journal/HTTP/SQLite : réponse perdue, conflit, révocation à chaque frontière, tâches abandonnées possédées, zéro second start. |
| H3 — Web et CLI utilisables ensemble | `identity_cli.rs`, `identity_cli/{http,oidc_session,export}.rs`, `api.rs` proxy fixe, `front/src/human-{api,model,view}.ts`, `human.css` et tests ciblés. Demande/plan/confirmation/refus/suivi/arrêt/export réellement branchés, mêmes receipts/IDs au Web et CLI ; purge/déconnexion/erreurs/fraîcheur accessibles. |
| H4 — qualifier et documenter le parcours entier | Guides identité/SSO/local EN/FR, `docs/{sso,ui,api}.md`, `CURRENT_STATE`, `VALIDATION`, `TEST_STRATEGY`, gates et preuves datées. Recettes locales et Keycloak Web+CLI avec vraie tâche non destructive, refus user2/admin sans droit, restart/retrait/reprise/export ; données et runtime réels requis. |

H1/H2 peuvent avancer par propriétaires de modules disjoints après gel du wire ; H3 sur contrats figés, revue indépendante en parallèle.
Premier sous-lot Rust proposé, **H1a après gate Root seulement** : créer/rejouer une demande de préparation et relire son statut minimal durable `waiting_for_broker`, avec contexte grants-seuls local/OIDC et refus A/B, sans consumer ni runtime.
Propriété disjointe proposée : nouveaux `src/control/human_execution.rs`, `src/audit/human_execution.rs`, `src/api/human/execution.rs`, `tests/human_execution_requests.rs`, `tests/human_execution_migration.rs`.
Root conserve les raccords partagés `control.rs/audit.rs/api/human.rs/main.rs`, schéma11 complet/versionné, budgets et mapping OIDC ; le lot ne s'accepte qu'une fois ces raccords intégrés, pas comme types ou routes morts.
Avant migration, H1a inventorie exhaustivement les consommateurs de version/protocole/capacité/budget, y compris gardes `2..=10`, listes explicites et projections/clients/reprises ; chaque compatibilité reçoit sa décision et son canari pertinent.
Une augmentation globale des bornes est interdite : admission/supervision, identité, grants, namespaces, capacités et réserves d'audit doivent accepter ou refuser11 selon leur contrat réel, sans modifier les garanties10.
Preuves Luna ciblées : vrai routeur/SQLite, création+replay après reopen, faux contexte/classe/tenant, grants seuls sans métadonnées, retrait/plafond OIDC, audit FULL et migration sauvegardée ; aucun broker/start présenté comme livré.
Les fichiers partagés sont attribués par Root ; pas de cache Cargo mutable partagé entre worktrees ni mutation runtime concurrente non coordonnée.
Luna exécute seulement les sélections utiles : nouveaux parents puis canaris `human_access`, `human_workers_api`, `broker_identity_api`,
`broker_submission`, `broker_lifecycle` et owners Full selon les coutures modifiées ; migration/protocole/redaction/TTY si touchés.
Tester audit FULL, preuve humaine/worker/runner fausse classe, contexte périmé, faux broker/autorité/incarnation, TTL/révocation,
substitution source/export, double livraison, crash avant/après claim, absence de terminal et refus closed-Full sans nouvelle mutation.
Les pairs/moteurs synthétiques qualifient seulement leurs coutures ; une seule recette réelle ciblée par environnement utile, pas de matrice répétée.
Les quatre parcours fournisseurs gardent leurs qualifications distinctes ; un appel réel borné exige critère et preuve, Claude explicitement `claude-haiku-5-5`.
G09/D5, P1 réseau et Kubernetes restent des dépendances de qualification ; aucun lancement interdit ou contournement pour obtenir un PASS.

Acceptation finale : depuis Web puis CLI, un humain local et un humain OIDC retrouvent la même chaîne authentifiée jusqu'à un effet runtime et un résultat/changement réels, avec refus d'un autre utilisateur, arrêt/reprise et export neuf démontrés.
Cloud/multi-hôtes, Docker/Podman/Kubernetes, TTY, signatures et capacité100 conservent leurs preuves distinctes et leurs écarts ouverts.
Décisions réellement restantes : validation Root du transport durable et du wire proposé ; choix de schéma11 à réserver lors de l'intégration.
Aucune préférence utilisateur supplémentaire ni autorisation cloud/PKI n'est présumée ; aucune activation de lancement n'est décidée ici.
