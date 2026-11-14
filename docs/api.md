# Contrat API locale — v0.1.0

Les routes historiques du tableau ci-dessous sont également exposées sous le préfixe versionné `/api/v1` ; `/api` reste leur alias compatible. Les nouvelles [routes de supervision](#supervision-centrale--protocole-v1) et la [route des faits du contrôle](#ingestion-et-faits-du-contrôle--ingestion-and-control-facts) existent uniquement sous `/api/v1`. `GET /api/v1/identity` (ou `/api/identity`) renvoie `{tenant_id,role}` depuis le jeton authentifié. Le gateway peut charger plusieurs identités et bases avec `--tenants-file` ; le [contrat multi-client](engineering/CLOUD_TENANCY.md) documente la configuration, la liaison durable et les limites. Les identifiants envoyés dans les requêtes ne choisissent jamais le tenant.

Gateway par défaut : `http://127.0.0.1:8787`. Le dashboard utilise les mêmes routes de lecture sous `http://127.0.0.1:4000`. La gateway et le dashboard ne fournissent aucune route d'exécution de commande HTTP : seul le CLI de confiance pilote Docker/Podman.

## Identités

Toutes les routes `/api/` exigent `Authorization: Bearer <secret>`. Sur les routes techniques, le secret viewer donne accès aux lectures et le secret runner aux écritures/lectures gateway. Le dashboard vérifie le rôle `viewer` avant chaque lecture technique relayée, même si son fichier de clé a été mal configuré. Par défaut, il accepte seulement le viewer configuré ; `dashboard --shared-tenants` accepte les viewers des tenants configurés par le gateway, chacun dans son périmètre. Cette option exclut `--viewer-token-file`. Les routes humaines `/api/v1/human/` ont leur authentification nominative séparée décrite ci-dessous, sans conversion en viewer. Aucun paramètre d'URL ne transporte un jeton.

Sur une route technique protégée reconnue, l'identité est extraite avant `Path`, `Query` et `Json` : Bearer absent/invalide →401, viewer sur mutation →403, même si son corps ou ses paramètres sont malformés. Ces refus n'imposent pas de lecture/désérialisation applicative du corps. Les erreurs d'une entrée authentifiée invalide restent celles de son contrat. Routage, transport et protections Origin/URI restent en amont : cette règle ne prétend pas empêcher toute réception d'octets réseau. [Preuves](engineering/evidence/2026-10-30/api-auth/README.md) et [ordre des extracteurs Axum](https://docs.rs/axum/latest/axum/extract/index.html#the-order-of-extractors).

Gateway et proxy dashboard refusent les champs Authorization multiples, y compris identiques. La syntaxe exacte `Bearer ` est conservée. **Les routes techniques** refusent un secret vide, contenant un espace blanc ou dépassant4096octets. Les constructeurs de bibliothèque appliquent cette même borne aux credentials techniques configurés : erreur pour `for_tenants`, état fail-closed503 pour `new`. Les lecteurs de fichiers gardent leurs contraintes existantes plus strictes côté gateway. Les empreintes techniques restent dans un index privé de configuration, sans copie complète du dictionnaire de secrets à chaque extraction. En mode legacy, les collisions entre classes d'identité restent recherchées dynamiquement dans les Stores ; l'index technique ne remplace pas cette vérification. Le profil owned conserve la couverture de son index complet de démarrage et la revalidation de sa propre source, sans scan SQL voisin à chaque requête ; la limite du [raccord candidat A0](#owned-control-a0) reste explicite. Cela n'est pas une preuve de résistance générale au DoS ou de lookup constant-time.

**Les routes humaines et les handlers OIDC** utilisent un parseur Bearer distinct : 1–16 384 octets, sans caractère de contrôle ni espace blanc. Le format local nominatif reste plus étroit ; cette borne ne change ni son format ni celle des clés runner/viewer. La configuration OIDC peut imposer une borne `max_token_bytes` inférieure, appliquée aux jetons fournisseur validés et aux jetons privés du corps de refresh/rotate. Le proxy d'authentification dashboard borne le header Authorization à 16 391 octets, préfixe compris ; le gateway valide ensuite le Bearer. Human/OIDC token bounds do not enlarge technical credentials or authorize an unadmitted JWT.

| Méthode et route | Identité | Résultat |
|---|---|---|
| `GET /healthz` | Aucune | État du composant ; le gateway refuse aussi les configurations invalides et les échecs d'audit déjà constatés |
| `POST /api/events` | Runner | Événement normalisé, expurgé puis stocké ; séquence et hashes |
| `POST /api/v1/control/events` | Runner | Fait du contrôle de la liste fermée ci-dessous ; même reçu, budget distinct de l'ingestion ; aucun alias `/api` |
| `POST /api/decide` | Runner | Décision tracée, hash de demande et `audit_seq` |
| `GET /api/events` | Viewer ou runner | Historique paginé et `next_cursor` |
| `GET /api/stream` | Viewer ou runner | SSE `event: audit`, `id: <seq>`, JSON d'événement |
| `GET /api/overview` | Viewer ou runner | Compteurs observés/déclarés séparés et état des composants |
| `GET /api/agents` | Viewer ou runner | `{items:[...]}` ; relations connues et provenance |
| `GET /api/sandboxes` | Viewer ou runner | `{items:[...]}` ; informations réellement recueillies |
| `GET /api/export` | Viewer ou runner | JSONL ou tableau JSON, borné par la tête de l'audit au début |
| `GET /api/integrity` | Viewer ou runner | Résultat de contrôle, nombre vérifié et hash de tête |
| `POST /api/workers/reservations` | Runner | Admission fraîche, réservation et reçu d'audit durables |
| `POST /api/workers/{id}/transitions` | Runner | Mutation CAS du worker et reçu d'audit ; aucun appel moteur dans le gateway |
| `GET /api/workers` | Viewer ou runner | Page `{items,next_cursor}` des workers du tenant |
| `GET /api/workers/{id}` | Viewer ou runner | Worker du tenant ; `404` s'il n'en fait pas partie |
| `GET /api/workers/operations/{id}` | Viewer ou runner, gateway uniquement | Reçu durable d'une opération du tenant |
| `GET /api/quota` | Viewer ou runner | Politique et réservations CPU/RAM/workers tenues |
| `GET /api/usage` | Viewer ou runner | Page `{items,next_cursor}` d'allocations terminales, `billable=false` |

## Identités workload, registre et relais / Workload identities, registry and relay

Le schéma6 ajoute les bindings de credentials workload, le registre des brokers
et les échanges dirigés entre workers. Ces nouvelles routes existent uniquement
sous `/api/v1`, sans alias `/api`. Le [guide français](setup/workloads/README.fr.md)
et le [guide anglais](setup/workloads/README.md) donnent les routes, DTO, limites,
reçus, erreurs et procédures de migration/configuration. Le tenant vient de la
preuve authentifiée ; un credential humain, worker ou broker ne devient pas
une clé runner. Les collisions lisibles entre classes sont refusées, y compris
pour les anciennes empreintes workload conservées.

Schema6 introduces workload credential bindings, broker registration and directed
worker messaging. These routes are available only under `/api/v1`; see the
[English contract and operator guide](setup/workloads/README.md). The gateway
persists authorized operations and mandatory audit, and does not execute container
commands. Broker heartbeat assertions are separate from runtime observations.
The HTTP backend alone does not connect a `network:none` worker, deliver remote
federation or qualify a unified cloud dashboard. Those paths require their own
transport, authorization and runtime evidence.

### Admission du canal runtime / Runtime channel admission

Ces routes V1 sont réservées au runner authentifié du contrôle. Le tenant vient
de son identité ; aucun champ du corps ne choisit un tenant. Le transport
`/control` les classe dans la voie runner, sans les ajouter aux trois routes worker.

| Méthode / Method | Route | Entrée / Input | Effet durable / Durable effect |
|---|---|---|---|
| POST | `/api/v1/relay/channels/admissions` | `RelayChannelAdmissionV1` | Réserve sans permis exec / Reserves without an exec permit |
| POST | `/api/v1/relay/channels/{channel_id}/claims` | `RelayChannelClaimV1` | Claim exec unique / One-shot exec claim |
| POST | `/api/v1/relay/channels/{channel_id}/transitions` | `RelayChannelTransitionV1` | Transition de cycle / Lifecycle transition |
| POST | `/api/v1/relay/channels/{channel_id}/reconcile` | `RelayChannelReconcileV1` | Lecture historique exacte / Exact historical lookup |

Admission et claim frais répondent 201 ; leurs rejeux identiques répondent 200.
Une transition répond 200. Ces trois mutations renvoient des
`RelayChannelResultV1` fermés.
Le binding courant, l'instance worker, son Arm et l'audit durable sont recoupés
avant acceptation. Une réservation ou un reçu rejoué ne donne aucun nouveau
permis moteur. Une erreur de persistance refuse l'acceptation.

Fresh admission and claim return 201; identical replays return 200, as do lifecycle
transitions. A reservation does not authorize exec, and replay never issues
another fresh capability. Runner authentication precedes body parsing. The
current workload binding, worker incarnation, original Arm and mandatory audit
must agree. These control APIs alone do not connect a worker to the relay.
See the [typed contract](engineering/RELAY_ADMISSION.md),
[English guide](setup/relay/README.md#trusted-control-admission) and
[French guide](setup/relay/README.fr.md#admission-par-le-contrôle-de-confiance).

Le rapprochement répond 200 avec un `RelayChannelReconciliationV1` fermé.
Sa requête versionnée contient l'admission immuable exacte et, facultativement,
le claim ou la transition pendante exacte : type, UUID d'opération, contenu/hash
canonique et CAS. Ces champs servent uniquement à comparer les faits conservés.
Le `channel_id` du chemin est un UUID canonique non nul égal à celui de
l'admission ; aucun alias `/api`, query ou sélecteur de tenant n'est admis.
Tenant et namespace sont recoupés côté serveur dans le snapshot vérifié.

L'enveloppe porte `version`, `tenant_id`, `control_namespace_uuid`,
`admission_input_hash`, `audit_seq`, `audit_hash` et `outcome`. Le pending est
fermé par `kind:claim|transition` et `request`. Pour l'outcome `historical`, la
réponse sépare `admission_receipt` du résultat du pending : `recorded` porte son
propre reçu réel ; `inapplicable_cas` exige son absence et une révision du même
canal strictement supérieure à son CAS ; `inapplicable_state` exige son absence,
la révision exactement attendue et un état incompatible selon les mêmes règles
que les mutations. Un changement d'état augmente toujours la révision : une
ancienne Close au CAS1 face à Reserved/rev1 ne pourra donc jamais s'appliquer.
`unresolved` conserve l'intention inconnue lorsque ces preuves manquent.
Les résultats et résolutions utilisent un discriminant `kind`. Un UUID
enregistré avec un autre type ou contenu produit un conflit, jamais une
résolution sans effet. La tête d'audit seq/hash accompagne les faits du même
snapshot ; elle ne constitue ni nouveau reçu de mutation ni signature.

Reconciliation returns a closed `RelayChannelReconciliationV1` with HTTP 200.
It compares the exact immutable admission and optional typed pending claim or
transition, without applying either. An exact recorded pending returns its own
historical receipt. An absent pending is `inapplicable_cas` only when the same
verified snapshot has a channel revision strictly above its expected revision.
At the exact expected revision, an incompatible state yields `inapplicable_state`
using the mutation's shared state rules. Every state change advances the revision,
so a retained Close at CAS1 against Reserved/rev1 cannot later apply. Without
either proof it remains `unresolved`. A divergent operation identity is a conflict.
Preserve the original intention and persist its receipt or deterministic
resolution before replacing it. With no admission, `admission_unresolved` leaves
the owner `Unknown`. Only `admission_never_recorded_exact_terminal` establishes
that the exact admission was not recorded and can no longer be accepted: the
same checked transaction verifies the same worker/attempt/session, inspection,
StartedAt and durable terminal observation. It returns the real
`MutationReceiptV1` of that observation with the admission/channel/epoch/endpoint
identities and worker revision; it fabricates no channel receipt. A later
Reserve requires Running, and a terminal worker cannot be revived by Observe.
Incomplete or mismatched terminal evidence leaves `admission_unresolved`.
HTTP 404, an unavailable route, lost response or failed verification supplies
no such proof.

Cette lecture technique étroite de métadonnées suit le régime de
`worker_operation` : transaction et contrôles d'intégrité existants, sans créer
de fait de canal, de claim ou de reçu d'accès. Ce régime ne s'étend pas aux
lectures humaines ni à l'inbox. Elle reste
accessible après terminal, expiration/révocation du workload ou désactivation de
l'admission du namespace, avec un runner toujours authentifié et autorisé.
Les requêtes/réponses du SDK sont bornées à 16 Kio ; le SDK déduit le tenant de
l'identité de supervision et ne prend aucun tenant libre. Aucun repli vers
Reserve, Claim, une autre route ou une autre identité n'est effectué.

This metadata read requires a currently authenticated, authorized runner and
remains historical after workload termination, expiry or revocation. Inactive
namespace admission is not revoked runner authentication. It creates no channel
fact or exec capability, opens no transport, renews no deadline, releases no
quota and proves no local process join. Active admission/claim/dispatch methods
keep their existing checks. The SDK bounds request and response bodies to
16 KiB and never falls back to an active mutation or another identity.

### Projection workers et brokers / Worker and broker projection

`GET /api/v1/federation/workers` est une lecture technique gateway pour viewer
ou runner authentifié. Le proxy dashboard accepte seulement un viewer. Les
preuves humaines, worker et broker n'acquièrent aucun accès à cette route.
Il n'existe ni alias `/api/federation/workers`, ni sélecteur de tenant/autorité.
Query fermée : `limit` facultatif1–100, défaut50 ; `cursor` facultatif UUID
canonique non nul, correspondant au dernier worker émis. Champs inconnus,
doublons et valeurs invalides sont refusés. La réponse est limitée à512Kio ;
une première entrée trop grande est une erreur, jamais une page vide simulée.

L'enveloppe version1 contient `source:"tenant_worker_broker_registry"`,
`consistency:"snapshot_per_page"`, `evaluated_at`, `runtime_inventory:"unknown"`,
`registry_status`, `items` et `next_cursor` nullable. Chaque item conserve le
`worker:WorkerV1` historique et ajoute `launch_broker` et `observation_broker`.
Ces associations portent `status`, `broker`, `runtime_kind`, `runtime_matches`
et `in_current_grant`. Les quatre derniers champs sont null pour
`authority_unknown`, `unregistered` ou `registry_unavailable`. Avec `registered`,
le broker expose seulement `broker_id`, `current_incarnation_id`, `state`,
révisions `grant_revision`/`heartbeat_revision` en chaînes décimales, et dates
`last_received_at`/`lease_expires_at` nullables. Aucun credential ou chemin hôte.
Les [DTO Rust](../src/broker_registry/federation.rs) font foi pour les types.

Une transaction vérifiée par page recoupe workers et registre du tenant
authentifié. Les pages successives sont des lectures live, sans inventaire
complet ni snapshot global garanti. Les schémas2–5 conservent leurs workers
avec `registry_status:"migration_required"` et associations
`registry_unavailable` ; les schémas6–9 utilisent `available`. Une erreur
d'intégrité/I/O reste une erreur503, sans repli vers cette limite structurelle.
Les gardes d'identité et de lecture conservent leurs refus401/429 ; une query
authentifiée invalide renvoie400. L'association historique peut être enregistrée
après le fait worker : elle ne prouve pas quel broker l'a exécuté, sa santé,
une incarnation au lancement ou une observation runtime actuelle.

The gateway read accepts technical viewer/runner credentials; the dashboard
proxy accepts viewers only. The separate bounded version1 page preserves
WorkerV1, reports historical authority ownership and current registry freshness,
and keeps runtime inventory unknown. Legacy schemas2–5 retain real workers
with an explicit migration limit; read failures never become empty pages.
The [English guide](setup/workloads/README.md#read-workers-across-registered-brokers)
documents operation and migration. This route supplies no runtime transport,
human SSO permission, physical multi-host qualification or billing authority.

### Identité du broker / Broker identity

`GET /api/v1/brokers/identity` utilise exclusivement le Bearer Broker courant,
sans sélecteur de tenant. Son DTO fermé version1 contient `version`,
`tenant_id`, `control_namespace_uuid`, `schema_version`, `broker_id`,
`incarnation_id`, `credential_id`, `binding_revision`, `grant_revision`.
Le Store sélectionné par l'authentification revalide ces faits dans une même
transaction avec l'horloge et le grant. Les autres rôles n'accèdent pas à
cette route ; une lease heartbeat périmée ne révoque pas à elle seule une
identité dont binding et grant sont encore valides.

This is a current server-authenticated scope snapshot, separate from the
non-deserializable internal authority proof. It does not issue a credential,
publish a heartbeat, renew a lease, prove runtime health or authorize a later
POST. Compare the expected tenant, control namespace and revisions using the
same Broker proof; a successful runner request under another key is not a
substitute. Clock rollback, invalid Store state and revoked/expired authority
fail closed. See the [operator guide](setup/workloads/README.md#read-the-authenticated-broker-scope)
for lifecycle limits and the [French guide](setup/workloads/README.fr.md#lire-le-périmètre-authentifié-du-broker).
An envelope containing retained credential material is refused as opaque
`503 identity_unavailable`, without masked identifiers or a partial snapshot.

### Checkpoints déclarés des brokers / Declared broker checkpoints

Le schéma12 ajoute deux routes réservées au Bearer **Broker** courant, sans
alias `/api`. Elles sont également admises sur le transport `/control` de la
même origine ; aucun credential du relais n'est ajouté.

| Route canonique / Canonical route | Demande / Request | Réponse / Response |
|---|---|---|
| `POST /api/v1/brokers/checkpoints` | `BrokerCheckpointV1`, au plus32768 octets / at most32768 bytes | `200 BrokerCheckpointResultV1`, première écriture ou replay exact / first write or exact replay |
| `GET /api/v1/brokers/checkpoints/self` | Aucun corps / No body | `200 BrokerCheckpointViewV1`, publication optionnelle / optional publication |

Le DTO fermé de demande contient `version:1`, `operation_id`, `incarnation_id`,
`previous_sequence`, `sequence` et `snapshot`. Le snapshot fermé contient
`version:1`, `authority`, `jobs` et `has_more` ; chaque job comporte `job_id`,
`revision`, `checkpoint_hash`. Les UUID sont canoniques, les hashes sont64
caractères hexadécimaux minuscules et les références sont triées, uniques et
limitées à100. La séquence vaut exactement précédente+1. Tenant, broker,
credential, binding et grant proviennent de l'identité vérifiée, jamais du
corps ; même tenant ne permet pas de publier pour un autre broker.

The Store revalidates the current binding/grant before an exact replay and
commits the operation with mandatory workload audit. The same operation with
different content conflicts. An ACK loss preserves the same Pending and sequence;
it does not authorize replacement identity or a new operation. The response
contains the submitted checkpoint, its receipt and `replayed`. The self view
contains `version:1`, the broker view and optional checkpoint observation with
receipt, reception/expiry timestamps, freshness, `in_current_grant`, provenance
and `coverage:declared_partial`, including when `has_more:false`.

Ces références sont des métadonnées déclarées de têtes locales confirmées,
pas un inventaire runtime, une preuve de zéro activité ou les points de contrôle
signés exigés par le contrat global. Leur réception est observée ; elles ne
renouvellent ni heartbeat, grant, quota, durée d'exécution ou permission.
Une source indisponible n'est pas remplacée par un snapshot vide.

Known credentials in input, authenticated scope or commitment are refused
before mutation. Both response envelopes are checked with the current proof's
redactor; unsafe retained output returns generic `503 broker_checkpoint_unavailable`
without substituting identifiers/hashes. A withheld POST response does not prove
rollback: reconcile that same operation. A wrong role or revoked authority is
refused; body bounds, conflicts and tenant workload budgets remain enforced.
See [EN](setup/broker/CHECKPOINTS.md)/[FR](setup/broker/CHECKPOINTS.fr.md) for
configuration and migration, and the [software evidence](engineering/evidence/2026-11-09/control-12-federation-bootstrap/README.md)
for exact tested sources and remaining service/runtime qualification.

## Kubernetes held-CREATE HTTP v1

Les six routes suivantes du gateway exigent une preuve Bearer **broker**
courante et un schéma de 9 à 13 ; elles n'ont aucun alias `/api`. Le tenant est résolu
par l'authentification, puis identité/grant sont revérifiés dans le segment
Store borné. Aucun header, query ou champ de corps ne sélectionne tenant,
politique ou quota ; les queries sont refusées. Ces routes contrôlent des
transactions SQLite, sans credentials Kubernetes ni appel cluster au gateway.

These six gateway routes require a current **broker** bearer proof and schema
9 through 13, with no `/api` alias. Authentication selects the tenant; the bounded Store
segment revalidates identity and grant. No header, query or body field selects
tenant, policy or quota; queries are refused. The gateway performs control
transactions, holds no Kubernetes credentials and makes no cluster request.

| Méthode et route / Method and route | Entrée / Input | Réponse / Response |
|---|---|---|
| `GET /api/v1/brokers/kubernetes/held-create-protocol` | Aucun corps / No body | `200 KubernetesHeldCreateProtocolV1` |
| `POST /api/v1/brokers/kubernetes/held-creates` | `KubernetesCreateIntentV1` | `201` première réservation, `200` replay exact / first reservation, exact replay ; `KubernetesCreateMutationResponseV1` |
| `POST /api/v1/brokers/kubernetes/held-creates/{create_id}/claims` | `KubernetesCreateClaimV1` | `201` premier claim encore éligible, sinon `200` / first eligible claim, otherwise `200` ; `KubernetesCreateClaimResponseV1` |
| `GET /api/v1/brokers/kubernetes/held-creates/{create_id}/claims/{operation_id}` | IDs canoniques, aucun corps / Canonical IDs, no body | `200 KubernetesCreateClaimResponseV1`, toujours / always `observe_only` |
| `POST /api/v1/brokers/kubernetes/held-creates/{create_id}/results` | `KubernetesCreateResultV1` | `200 KubernetesCreateResultResponseV1`, ACK exact / exact ACK |
| `POST /api/v1/brokers/kubernetes/held-creates/{create_id}/cancellations` | `KubernetesCreateCancelV1` | `200 KubernetesCreateMutationResponseV1` |

DTO fermés : [intent/claim/result/cancel et reçu/snapshot](../src/control/kubernetes_admission.rs),
[enveloppes HTTP](../src/control/kubernetes_admission/http.rs). Les POST exigent
un seul `Content-Type: application/json` (paramètres MIME admis), au plus
64 Kio de corps, profondeur 64, sans membre dupliqué à aucune profondeur ni
contenu après le JSON. Champ inconnu, forme invalide ou borne dépassée donne
un refus fermé `400 invalid_held_create_request` ; les désaccords path/body
utilisent `400 invalid_workload_request`. La lecture du corps est bornée à 5 s
et ne conserve pas de slot Store pendant l'upload.

Closed DTOs are linked above. POST requires one JSON Content-Type header,
a body up to 64 KiB and depth 64, with no duplicate members at any depth or
trailing content. Unknown fields, malformed/oversized input fail with closed
`400 invalid_held_create_request`; path/body mismatches use
`400 invalid_workload_request`. Body upload has a five-second bound without
holding a Store slot.

Les réponses fermées portent `version:1` : mutation =
`{version,receipt,snapshot,replayed}`, claim =
`{version,receipt,snapshot,disposition}`, résultat = `{version,receipt,snapshot}`.
Le snapshot appartient au reçu de l'opération, pas nécessairement à l'état
courant. Le protocole contient schéma/tenant/namespace/broker/incarnation,
révisions binding/grant, `admission_policy_configured` et les opérations
`reserve|claim|observe_claim|result|cancel`, complétées sur 13 par
`recover|observe_recovery`. Ce booléen indique une configuration,
pas une admission garantie. Claim `201` exige une preuve Store Fresh du premier
commit, liée aux claim/reçu/snapshot exacts, et une fenêtre encore valide avant
restitution ; sinon `200 observe_only`. Replay/GET ne rendent jamais Fresh.
Les reçus JSON ne reconstruisent aucune capacité runtime.

Responses use the version-1 shapes above; snapshots belong to the operation's
receipt and may be historical. Protocol metadata reports configured policy,
not admission availability. Claim `201` requires a matching first-commit Store
Fresh proof and a live emission window at response preparation; otherwise it
returns `200 observe_only`. Replays/GETs never issue Fresh. A JSON receipt is
not a consumable runtime capability, nor a guarantee of arrival before expiry.

Reserve/Claim obtiennent une décision V2 Admission depuis politique et quota
COURANTS du `TenantState`, après authentification ; pas de politique fournie
par le broker. Lookup/résultat/annulation obtiennent chacun une nouvelle V2
Reconcile liée au Reserve ORIGINAL exact et au grant/quota courants, même
après retrait de politique, baisse de quota ou réouverture inactive. Révocation,
scope divergent et incarnation différente restent refusés. La provenance de
politique est un snapshot déclaré de configuration, pas une approbation humaine
nominative. Les capacités mémoire V1 sont refusées sur 9 avant replay ; une
V2 appropriée peut relire un reçu/snapshot V1 exact sans append ni Fresh.

Reserve/Claim use the current server-resolved tenant policy/quota. Each
lookup/result/cancel obtains a separate Reconcile decision from the exact
original Reserve and current grant/quota; policy withdrawal, lower quota or
inactive supervision alone do not block it. Revoked/divergent identities stay
refused. Policy provenance is declared configuration, not named human approval.
V1 in-memory capabilities are refused on 9; an appropriate V2 can return an
exact historical V1 receipt/snapshot without appending or issuing Fresh.

Entrées et sorties sont criblées avec les secrets connus, dont le Bearer
broker courant, clés JSON comprises. Le même dictionnaire couvre les données
Store dérivées avant persistance. Toute modification nécessaire d'une réponse
exacte ou réponse dépassant 128 Kio donne `503 held_create_unavailable`, sans
troncature, masque de substitution ou corps partiel. Le commit peut déjà être
durable : une erreur ou perte HTTP ne prouve pas son absence. Garder identités,
candidat et CAS exacts pour la reprise autorisée, sans fabriquer un nouvel état.

Input/output screening includes the current broker bearer and JSON keys;
derived Store data uses the same secret set before persistence. A response
requiring redaction or exceeding 128 KiB is refused with
`503 held_create_unavailable`, never truncated or replaced with masked facts.
A commit can precede that refusal or loss of its HTTP ACK; preserve the exact
operation/candidate/CAS for authorized recovery.

Erreurs typées de ces handlers / Typed handler errors :

| HTTP | Code fermé / Closed code |
|---|---|
| `401` | `authentication_required` |
| `403` | `held_create_forbidden` ou / or `workload_forbidden` |
| `404` | `held_create_not_found` |
| `409` | `held_create_conflict`, `stale_epoch`, `stale_revision` |
| `429` | `held_create_quota_exceeded` ; limites I/O/audit via leurs codes existants / existing I/O/audit limit codes |
| `503` | `held_create_unavailable` ; les erreurs d'authentification/Store héritées gardent leur mapping opaque / inherited authentication/Store failures keep their opaque mapping |

Les erreurs de contrôle ont `{error:{code,message}}` ; les fallbacks d'audit
existants peuvent avoir `{error:message}`. Aucun corps Kubernetes, chemin ou
chaîne d'erreur brute n'est exposé. Source effective du mapping :
[`api/kubernetes.rs`](../src/api/kubernetes.rs) et
[`api/workload.rs`](../src/api/workload.rs), pas un `500` générique supposé.
Control errors have a closed code/message envelope; existing audit fallbacks
may use an opaque error string. Neither format exposes raw error chains.

Le client HTTP broker/ACK live et le coordinateur interne sont raccordés ;
l'activation et la qualification cluster restent ouvertes. Ces routes ne
livrent ni workspace, ni session Kubernetes, ni permission d'activation.
The broker HTTP/live-ACK client and internal coordinator are connected;
activation and real runtime qualification remain open.
Voir les guides [EN](setup/kubernetes/README.md) / [FR](setup/kubernetes/README.fr.md).

### Close-only held-CREATE recovery v3 / Récupération sans effet

Ces deux routes exigent la migration explicite et sauvegardée 12→13 et une
identité **broker courante**. Aucun droit runner, worker, CREATE, inspection,
suppression ou activation Kubernetes n'est accordé. Le serveur déduit tenant,
acteur et quota ; le corps ne contient aucune preuve d'autorité.
These routes require the explicit backed-up 12→13 migration and a current
**broker** identity. They grant no runner, worker or Kubernetes runtime right.
Tenant, actor and quota come from authenticated server state.

| Method / méthode | Request / requête | Response / réponse |
|---|---|---|
| `POST /api/v1/brokers/kubernetes/held-creates/{create_id}/recoveries` | `KubernetesCreateRecoveryV3` | `201` first close / première fermeture, `200` exact replay / rejeu exact ; `KubernetesCreateRecoveryResultV3` |
| `GET /api/v1/brokers/kubernetes/held-creates/{create_id}/recoveries/{operation_id}` | No body or query / aucun corps ni query | `200 KubernetesCreateRecoveryResultV3`, `replayed=true` ; absent `404`, never a mutation / jamais une mutation |

Le POST fermé contient exactement / the closed POST contains exactly :

```json
{
  "version": 3,
  "operation_id": "00000000-0000-4000-8000-000000000002",
  "create_id": "00000000-0000-4000-8000-000000000001",
  "intent_hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "expected_revision": 1,
  "expected_create_epoch": 1
}
```

Ces valeurs sont illustratives : retenir les vrais UUID/hash de la réservation
et une identité d'opération unique avant l'échange. UUID canoniques, non nuls
et distincts ; create_id du path et du corps égaux. Champs inconnus/dupliqués,
query ou forme invalide sont refusés avec les limites JSON et erreurs fermées
ci-dessus. Le résultat est `{version:3,recovery,receipt,snapshot,replayed}`.
Le reçu reste V1, descriptif, sans fenêtre d'émission. Le snapshot exact est
`cancelled_before_emission`, révision/epoch2, quota libéré, sans claim/résultat,
avec le `last_cancel` descriptif équivalent.

Values above are illustrative. Retain the real reservation IDs/hash and one
operation UUID before exchange. Canonical nonnil UUIDs must differ and body/path
create IDs must agree. The strict JSON bounds, timeouts and typed errors above
apply. The version-3 result carries the exact request, a descriptive V1 receipt
without an emission window and the cancelled snapshot at revision/epoch2.
It cannot reconstruct Fresh or runtime authority.

Seul l'état initial complet, prouvé par le Reserve durable et son historique,
peut être fermé : aucun claim, fenêtre, résultat ni annulation antérieure.
Le broker courant peut être le créateur ou son successeur du **même broker
stable**, tenant, namespace et authority autorisée ; l'intention et son
incarnation historique restent inchangées. Reconcile V1/V2 demeure strict.
Un claim concurrent, un contenu divergent ou un autre broker est refusé.
Révocation, remplacement ou expiration sont revérifiés avant tout ancien reçu.
L'audit obligatoire utilise le budget workload du broker ; une capacité épuisée
refuse l'opération en conservant le quota, sans réserve implicite ni édition SQLite.

Only the complete original never-claimed reservation can close. A current
creator or successor must retain the same stable broker, tenant, control
namespace and authorized historical authority. The original intent/incarnation
remains historical; legacy Reconcile is unchanged. A concurrent claim or
conflicting request denies recovery. Current identity, grant, binding and
expiry are revalidated before replay. Mandatory workload-audit failure denies
the close and preserves quota; no hidden reserve or database repair is used.

Après perte d'ACK, conserver **la même requête et le même operation_id**. GET
lit son reçu exact ; POST identique retrouve le reçu avant le test d'état
initial, sans quota ni événement de fermeture supplémentaire. GET absent ne
crée rien. Le client Rust interne expose `recover(intent,request)` et
`observe_recovery(intent,request)` sur ces routes fixes, valide entièrement
le résultat et ne génère aucun UUID en reprise. Aucune commande CLI nouvelle ;
`reconcile-held-create` garde son contrat historique.

After an uncertain ACK, retain **the same request and operation_id**. GET reads
that receipt; exact POST replays it without another close event or quota release.
Missing GET never mutates. Internal Rust methods `recover(intent,request)` and
`observe_recovery(intent,request)` use only these fixed routes and validate the
whole descriptive result. No recovery UUID is regenerated and no new CLI is
introduced. The existing `reconcile-held-create` command remains unchanged.

Validation dynamique de ce nouveau raccord : **NOT_RUN** à sa livraison source ;
QA composée et revue indépendante requises. This source delivery is not a
runtime qualification or closure of audit run-2 D6.

## Récupération contrôle des réservations Kubernetes V2 / Kubernetes session control recovery V2

La route fixe est **`POST /api/v1/kubernetes/session-reservations/{id}/control-recovery`**,
sans alias `/api`. Elle exige le Bearer **runner technique partagé** du contrôle,
pas une identité humaine/OIDC, viewer, broker ou worker. Dans le handler,
l'authentification précède validation du Path et lecture du corps ; elle choisit
le tenant et le Store exact, puis est revérifiée après admission Store et avant
chaque usage.
Aucun header, query, label ou corps ne choisit tenant, rôle, politique ou sujet
à facturer. Le contrôle requiert le schéma 11, l'historique Reserve validé et
la permission actuelle pour son broker créateur historique, même après
décommission de toutes ses autorités. Ce chemin ne réactive aucun grant.

The fixed route above has **no `/api` alias**. Its credential is the control's
**shared technical runner**, never a human/OIDC, viewer, broker or worker
identity. Within the handler, authentication precedes Path validation and body
consumption, selects the tenant and exact Store, and is repeated after Store
admission and before each use. No caller input chooses tenant, role, policy or
charged principal.
Schema 11, validated original Reserve history and current policy permission for
its historical creator broker are required; an active successor broker is not.
This path does not reactivate a grant.

### Corps fermé et résultats / Closed body and results

`{id}` est un UUID non nul canonique en minuscules. Un seul
`Content-Type: application/json` est requis (paramètres MIME admis), sans query.
Le corps UTF-8 fait au plus 81920 octets (80 Kio), profondeur 64, sans membre
dupliqué à aucune profondeur, champ inconnu ou contenu après le JSON.
La lecture directe du handler est bornée à 5 secondes sans slot Store retenu
pendant l'upload. Tous les champs `version` valent 2. Les hashes sont 64 hex
minuscules ; `operation_id` est un UUID canonique non nul distinct de `{id}`.
Ces trois exemples publics de forme ne sont pas des opérations à exécuter :
reprendre le vrai hash de réservation et conserver les IDs/opérations exacts.

`{id}` is a canonical lowercase non-nil UUID. Require one JSON Content-Type
header (MIME parameters allowed), no query, at most 81920 UTF-8 body bytes,
depth 64, no duplicate members, unknown fields or trailing JSON content. Direct
handler body reading has a five-second bound without holding Store slots.
Every `version` is 2; hashes are 64 lowercase hexadecimal characters. A canonical
non-nil `operation_id` must differ from `{id}`. The shared public shape examples
below are not runnable operations: use the retained reservation hash and exact
operation identities.

```json
{
  "action": "observe_reserved",
  "version": 2,
  "reservation_input_hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
}
```

```json
{
  "action": "close_before_effect",
  "version": 2,
  "close": {
    "version": 2,
    "operation_id": "22222222-2222-4222-8222-222222222222",
    "reservation_id": "11111111-1111-4111-8111-111111111111",
    "reservation_input_hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "expected_revision": 1,
    "reason": "operator_cancelled"
  }
}
```

```json
{
  "action": "lookup_close_receipt",
  "version": 2,
  "reservation_input_hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "close_input_hash": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
  "operation_id": "22222222-2222-4222-8222-222222222222"
}
```

Pour Close, `close.reservation_id` correspond exactement au Path,
`expected_revision` vaut exactement 1 et `reason` vaut
`operator_cancelled|preparation_abandoned`. Les actions sont distinctes :
Observe vise seulement Reserved/révision 1/quota tenu avec
`effect_fence:{state:"never_issued",revision:0}` et aucune identité legacy liée.
Close peut alors engager ClosedBeforeEffect/révision 2/quota libéré une fois.
Lookup exige le Close exact déjà retenu. Observe n'autorise pas Close et ne
relit pas une réservation fermée. Aucun de ces JSON ou reçus ne donne Reserve,
CREATE, start, credential workload, workspace, transport ou effet Kubernetes.

Close requires the exact Path reservation ID, `expected_revision:1`, and one
of the two reasons above. Observe requires Reserved/revision 1/held quota,
NeverIssued revision 0 and no linked legacy identity. Close then commits
ClosedBeforeEffect/revision 2 and releases allocation once. Lookup requires the
exact retained Close. Observe grants no Close and cannot observe closed state.
These requests/receipts grant no Reserve, CREATE, start, workload credential,
workspace, transport or Kubernetes effect.

| Action | HTTP 200 et objet exact / HTTP 200 exact shape |
|---|---|
| `observe_reserved` | `KubernetesSessionReservationCurrentV2` = `{version,snapshot,last_event_seq,last_event_hash}` |
| `close_before_effect` | `KubernetesSessionReservationMutationResponseV2` = `{version,receipt,snapshot,replayed}` ; `false` au premier Close, `true` au replay exact / first Close, exact replay |
| `lookup_close_receipt` | Même mutation, `replayed:true` / Same mutation shape, always replayed |

Le reçu contient `{version,operation,operation_id,input_hash,control_namespace_uuid,audit_seq,audit_hash}`,
avec `operation:"close_before_effect"` et `input_hash:K` pour Close. Le snapshot
porte `{version,intent,input_hash,session_hash,attempt_hash,profile_hash,state}` ;
`state` contient `{state,revision,quota_held,effect_fence,last_close}`. Les types
réels sont dans [kubernetes_session.rs](../src/control/kubernetes_session.rs).
Une réponse exacte dépassant 131072 octets (128 Kio) ou nécessitant un masquage
est refusée sans troncature, avec le code partagé réel `held_create_unavailable`.
Une entrée nécessitant masquage est refusée. Aucun secret/digest runner n'est
ajouté aux réponses, engagements ou traces descriptives.

The receipt and snapshot contain exactly the fields above. Close receipts use
`operation:"close_before_effect"` and `input_hash:K`. Exact responses exceeding
131072 bytes or requiring redaction fail without truncation under the shared
serializer's actual `held_create_unavailable` code. Input requiring redaction
is refused. No runner secret/digest is added to responses, commitments or
descriptive evidence.

### Reprise, politiques et engagements / Retry, policies and commitments

Après perte de réponse, réauthentifier et soumettre le même Close ou son Lookup
exact R/K/operation_id. Le contrôle revalide les permissions actuelles et rend
le reçu/snapshot historiques, sans append, seconde libération ou nouveau débit.
Un UUID réutilisé avec contenu divergent est un conflit ; ne modifier ni CAS,
motif, ID ou hash pour forcer le replay. Le reçu n'a pas de champ `actor` :
`audit_seq/audit_hash` désignent l'événement original, qui conserve acteur et
créateur sans réattribution au runner de la reprise. L'acteur contrôle
`control_shared_runner` est une origine technique observée, pas une personne ;
le créateur reste le broker historique. Close consomme son budget workload
broker+tenant cumulé ; audit indisponible ou saturé bloque Close.

After a lost response, authenticate anew and submit the same Close or exact
R/K/operation_id Lookup. Current permissions still apply. Replay returns the
historical receipt/snapshot with no append, second allocation release or new
charge. A reused operation UUID with divergent content conflicts; do not edit
CAS, reason, IDs or hashes to force replay. The receipt has no `actor` field:
its audit sequence/hash refer to the original event, preserving its actor and
creator rather than assigning them to the retry's runner. `control_shared_runner`
is observed technical authentication, not a named human; the creator remains
the historical broker. Close consumes that broker's cumulative tenant/workload
budget and cannot bypass unavailable/full mandatory audit.

Les trois politiques serveur sont indépendantes, immuables au démarrage et
chargées depuis l'entrée tenant de confiance `--tenants-file`, jamais depuis
ce corps :
`session_reservation_policy` (approbations exactes des huit champs),
`session_recovery_policy` (broker courant de même ID historique) et
`session_control_recovery_policy` (runner partagé et broker historique autorisé).
Absente/null, chaque politique est désactivée ; une liste vide ne donne rien.
Version 2, TTL 1–60 secondes, au plus 64 entrées distinctes et 16384 octets canoniques
s'appliquent à chacune. Voir les détails/exemples/migration hors ligne dans les
guides [EN](setup/kubernetes/README.md#configure-session-reservation-v2-policies)
/ [FR](setup/kubernetes/README.fr.md#configurer-les-politiques-de-réservation-de-session-v2).
Une capacité est bornée par le TTL policy et les échéances UTC/monotone de
l'authentification (au plus 60 secondes), recontrôlées à l'usage. Ni fichier modifié
sur disque ni ancien reçu ne révoque ou ne renouvelle un contexte vivant.

The three independent startup-immutable server policies above come from trusted
tenant configuration, never this body. Missing/null disables each; empty lists
grant nothing. Each uses version 2, a 1–60 second TTL, at most 64 distinct entries
and 16384 canonical bytes. Reservation, broker recovery and shared-runner recovery
remain distinct permissions. The linked guides specify their fields and offline
migration. Capabilities obey policy TTL and both UTC/monotonic authentication
deadlines (at most 60 seconds), checked again at use. Editing files or replaying
old receipts neither revokes nor renews a live authentication context.

`C(x)` est le JSON UTF-8 compact canonique : clés d'objets triées récursivement,
ordre des tableaux conservé, sans flottants. `H` est SHA-256 hex minuscule et
`\0` ci-dessous désigne **un octet NUL**, pas deux caractères. `S/A/P` viennent
du snapshot/intent validé (session, tentative, profil) ; `F` est l'empreinte
d'autorité. `C(x)` is compact canonical UTF-8 JSON with recursively sorted object
keys, retained array order and no floating numbers; `H` is lowercase SHA-256.
`\0` denotes one NUL byte. `S/A/P` are validated session/attempt/profile hashes;
`F` is the authority fingerprint. Exact request-relevant preimages:

```text
R = H(b"ai-guard:kubernetes-session-reservation:v2\0" ||
      C([2,reservation_id,worker_id,1,S,A,F,P]))
K = H(b"ai-guard:kubernetes-session-reservation-close:v2\0" ||
      C([2,operation_id,reservation_id,R,1,reason]))
Qctrl = H(b"ai-guard:kubernetes-session-control-recovery-policy:v2\0" || C(policy_canonicalized))
```

Les preuves mémoire ne sont ni reçues ni reconstruites depuis ces hashes.
L'engagement interne BeforeEffect lie acteur/namespace/créateur, Reserve
historique, R/S/A/P, head, état Reserved1/NeverIssued0, action/cible,
snapshot/hash de politique et dates. L'engagement ReceiptLookup distinct lie
acteur/namespace/créateur, R/K, séquence/hash du Close, head, hash de politique
et dates ; un lookup n'est pas une autorisation Close. Les domaines exacts sont
`ai-guard:kubernetes-session-control-recover-before-effect:v2\0` et
`ai-guard:kubernetes-session-control-recovery-receipt:v2\0` ; préimages ordonnées
dans [control_recovery.rs](../src/audit/kubernetes_session/control_recovery.rs).
Ces engagements décrivent l'audit ; le replay ne recrée aucune autorité passée.

Hashes cannot construct/restore memory authority. BeforeEffect commits actor,
namespace, creator, historical Reserve, R/S/A/P, head, Reserved1/NeverIssued0,
action/target, policy snapshot/hash and dates. The separate ReceiptLookup
commitment binds actor, namespace, creator, R/K, Close sequence/hash, head,
policy hash and dates. Their distinct domains and ordered source preimages
are linked above; lookup is no Close authorization. Replay validates historical
evidence without recreating past authority.

### Erreurs et accès publiés / Errors and published access

| HTTP | Code ou forme réelle / Actual code or shape |
|---|---|
| `400` | `invalid_session_control_recovery_request` : Path/body/JSON/version/hash/UUID/borne/délai/query invalides / invalid protected input, bounds or body deadline |
| `401` | Bearer absent/invalide/ambigu, humain/broker/worker ; garde héritée `{error:message}` / inherited control guard |
| `403` | `session_control_recovery_forbidden`, ou garde runner/époque héritée `{error:message}` / current policy/scope refusal or inherited runner/epoch guard |
| `404` | `session_control_recovery_not_found` dans le seul tenant authentifié / only in the authenticated tenant |
| `409` | `session_control_recovery_conflict` : contenu/état exact divergent / conflicting retained content/state |
| `429` | `session_control_recovery_quota_exceeded`, `audit_workload_budget_exhausted`, autres budgets I/O/audit existants / existing I/O/audit budget codes |
| `503` | Audit/Store/schéma/identité indisponible via garde opaque héritée ; encodage exact : `held_create_unavailable` / inherited opaque unavailability; exact serializer code |

Les erreurs typées ont `{error:{code,message}}` ; les gardes héritées peuvent
avoir `{error:message}` et les budgets un `retry_action`. Aucun détail brut
d'audit ou secret ne sort. Un commit peut précéder une perte HTTP ou un refus
d'encodage : aucune erreur de réponse ne prouve rollback. Typed errors use the
code/message envelope; inherited guards may use an opaque string and budget
errors may add `retry_action`. HTTP loss/encoding refusal can follow a commit;
preserve the exact operation for authenticated reconciliation.

**Accès par le transport contrôle** : sur l'origine gateway publiée par les
profils Compose Docker/Podman standard et SSO, utiliser
`POST /control/api/v1/kubernetes/session-reservations/{id}/control-recovery`.
Le relais retire le préfixe de transport `/control` et cible le listener
contrôle fixe du gateway. La liste fermée de
[control_transport.rs](../src/control_transport.rs) classe cette seule forme
POST v1 avec UUID canonique dans la voie Runner ; le classifieur ne prouve
aucune identité. Le vrai gateway refuse anonyme/worker avec 401 et viewer
avec 403 avant lecture du corps. Le listener contrôle dédié reçoit le chemin
`/api/v1/...` ; `gateway_router` propose aussi le préfixe `/control`.
Un runner authentifié atteint les trois DTOs fermés et reçoit 403
`session_control_recovery_forbidden` si la politique manque. L'UUID nil reste
classable syntaxiquement mais donne 400 dans le handler de session.

Le chemin ordinaire `/api/v1/...` reste exposé par `gateway_router` : l'ingress
public lit alors le corps avant l'authentification gateway et applique ses
budgets publics. Il ne donne pas la réserve de la voie contrôle et ne garantit
pas son ordre authentification/corps. Le raccord logiciel ne qualifie pas
un déploiement Compose ou un cluster réel.
Le dashboard ne relaie que des GET métier et n'ouvre pas cette mutation runner.
Les profils standard utilisent `--data` sans ces politiques ; les profils SSO
utilisent leur registre tenant. Aucun profil livré ne les active par défaut :
la configuration explicite et la migration11 restent des prérequis séparés.

**Control transport access**: on the published gateway origin of standard/SSO
Docker and Podman Compose profiles, use the exact prefixed POST above. The relay
removes `/control` and targets the fixed gateway control listener. The closed
allowlist classifies only this v1 POST with a canonical UUID as Runner;
classification proves no identity. The real gateway refuses anonymous/worker
credentials with 401 and viewers with 403 before reading the body. The dedicated
control listener receives `/api/v1/...`; `gateway_router` also supplies the
`/control` transport prefix. An authenticated runner reaches all three closed
DTOs and receives 403 `session_control_recovery_forbidden` when policy is absent.
Canonical nil remains syntactically classifiable but receives 400 from the
session handler.

The ordinary `/api/v1/...` gateway route remains exposed: public ingress reads
its body before gateway authentication and applies public budgets. That path
does not supply reserved control capacity or the control path's authentication
ordering. Software routing does not qualify a real Compose deployment or
cluster. The dashboard's business proxy remains GET-only and does not open
the runner mutation.
Standard profiles use `--data` without these policies; SSO profiles use their
tenant registry. Distributed profiles do not enable them by default: explicit
configuration and migration11 remain separate prerequisites.

Sources : [handler/DTO](../src/api/kubernetes_sessions.rs),
[authentification](../src/api/auth/control_recovery.rs),
[routeurs gateway/dashboard](../src/api.rs), [relais contrôle](../src/ingress/control.rs).
Ce contrat n'est ni une preuve d'arrêt/drainage, de qualification HTTP réelle,
de lancement/confinement Kubernetes ou de release production. This contract
provides no evidence of shutdown/draining, live HTTP qualification, Kubernetes
launch/confinement or production release readiness.

## Authentification OIDC Web et CLI / Web and CLI OIDC authentication

Le contrat HTTP ci-dessous décrit **l'implémentation candidate**, configurable explicitement, au 2 novembre 2026. La qualification Keycloak réel et navigateur reste `NOT_RUN` ; aucune qualification production n'en découle. Les guides SSO [français](setup/sso/README.fr.md) et [anglais](setup/sso/README.md) décrivent configuration, profil signé accepté, memberships et exploitation. Les [routes Rust](../src/api/oidc.rs) et leurs [handlers](../src/api/oidc/gateway.rs) font autorité sur ces DTO.

This is the explicitly configured OIDC candidate's HTTP contract. Actual Keycloak/browser qualification is pending. OIDC admits human reads/exports; it neither converts a human into runner/viewer/worker authority nor exposes a human agent-launch API. ai-guard login remains separate from Codex/Claude authentication.

Ces routes ont le préfixe fixe `/auth/oidc`, sans alias `/api` ou `/api/v1`. Aucun champ caller ne sélectionne tenant, issuer ou client. Le gateway possède PKCE, callback et échange Web ; le dashboard relaie uniquement les six routes marquées ci-dessous vers son gateway fixe. Le Web utilise le client public `web_client_id` configuré ; le CLI joint directement le gateway et ses admissions/rotations exigent le client public distinct `cli_client_id`. `/auth/oidc/complete` sert seulement la page frontend du dashboard ; ce n'est pas un endpoint JSON du gateway.

| Méthode et suffixe / Method and suffix | Accès / Access | Requête / Request | Réponse / Response |
|---|---|---|---|
| `GET /status` | Public ; proxy dashboard / Public; dashboard proxy | Aucun Bearer / No bearer | `200 {version:1,enabled:false}` ou / or `{version:1,enabled:true,issuer,provider_label}`. |
| `GET /cli-profile` | Public ; gateway seulement / Public; gateway only | Aucun Bearer / No bearer | `200 {version:1,enabled:false}` ou / or `{version:1,enabled:true,provider,ca_bundle_pem?}`. |
| `POST /start` | Web ; proxy dashboard / Web; dashboard proxy | `{version:1}` ; cookie de corrélation / correlation cookie | `200 {version:1,authorization_url}`. |
| `GET /callback` | Retour IdP ; proxy dashboard / IdP return; dashboard proxy | Cookie, `state` et `code` ou `error` / Cookie, state and code or error | `303 Location: /auth/oidc/complete` après résultat corrélé / after a correlated result. |
| `POST /tokens` | Web ; proxy dashboard / Web; dashboard proxy | `{version:1}` ; même cookie / same cookie | `200 TokenDelivery` ou erreur corrélée à usage unique / one-shot correlated error. |
| `POST /refresh` | Web ; proxy dashboard / Web; dashboard proxy | Bearer Web admis + `{version:1,authentication_id,generation,refresh_token}` / Admitted Web bearer plus body | `200 TokenDelivery`. |
| `POST /native-intent` | Natif ; gateway seulement / Native; gateway only | `{version:1,action}` ; variantes ci-dessous / variants below | `200 {version:1,intent_id,expires_at}` ou / or `{version:1,status:"finished"}`. |
| `POST /admit` | Natif ; gateway seulement / Native; gateway only | Nouveau Bearer CLI + `{version:1,intent_id}` / New CLI bearer plus body | `201 Session`. |
| `POST /rotate` | Natif ; gateway seulement / Native; gateway only | Ancien Bearer admis + `{version:1,intent_id,authentication_id,generation,access_token}` / Old admitted bearer plus new access token in body | `200 Session`. |
| `POST /logout` | Type de la session ; proxy dashboard / Session kind; dashboard proxy | Bearer admis + `{version:1,authentication_id,generation}` / Admitted bearer plus body | `200 {version:1,local_session,provider_logout,audit}`. |

Les objets POST refusent les champs inconnus et exigent `version:1`, sans querystring. Un unique Content-Type vaut exactement `application/json` ou `application/json; charset=utf-8` ; corps limité à 35 000 octets et lecture à 5 s. Ces limites d'authentification ne remplacent pas les limites des API métier. Les réponses applicatives portent `Cache-Control: no-store` et `Referrer-Policy: no-referrer`.

Web POSTs require one exact configured `Origin`. Optional Fetch Metadata must be `Sec-Fetch-Site: same-origin` and `Sec-Fetch-Mode: cors|same-origin`, without duplicate fields. Native routes reject any Origin and accept absent Fetch Site or `none`; native logout follows the same rule for a known CLI session. The configured GET callback alone accepts the cross-site IdP return, then enforces correlation. Other routes retain their existing HTTP guards.

`GET /status` et `/cli-profile` ne demandent aucune identité et ne sondent pas l'IdP : `enabled:true` ne prouve pas sa disponibilité. Le profil CLI est une projection publique allowlistée du provider (`OidcConfigV1`) et, si configuré, des seuls certificats d'AC publics ; il ne publie ni mapping, configuration privée, chemin, clé ou audit. Le dashboard sans proxy OIDC configuré refuse ses routes d'authentification avec `503 oidc_not_configured`, même si le gateway dispose d'un statut public.

Le statut Web activé accepte Origin absent ou exactement configuré ; Fetch Site absent, `same-origin` ou `none`. Il initialise seulement si absent le cookie privé `aiguard_oidc` : 64 caractères hex minuscules, `HttpOnly`, `SameSite=Lax`, `Path=/auth/oidc`, durée 300 s, `Secure` en HTTPS. Le statut ne remplace ni ne renouvelle un cookie valide présent ; `/start` réémet celui de sa transaction. Un cookie dupliqué/malformé ou un champ Cookie de plus de 4096 octets est refusé. Ce cookie corrèle une transaction ; il n'authentifie aucune API métier.

Browser login requires that correlation cookie for start, callback and token handoff. Callback accepts one `state`, exactly one of `code`/`error`, and only the optional parameters `error_description`, `error_uri`, `session_state`, `iss`; duplicate/unknown parameters are refused and a supplied issuer must match. A matching transaction is consumed before provider exchange. Uncorrelated callbacks cannot replace another pending result. The 303 redirect carries no code/state query. `/tokens` consumes the result once within 30 s and clears the cookie, including on a correlated failure; provider error text is never reflected. Web PKCE lifetime is configured and capped at 300 s.

`TokenDelivery` contient `version:1`, `authentication_id`, `generation`, `token_type:"Bearer"`, `access_token`, `refresh_token` facultatif, `expires_at` RFC3339 et `expires_in_seconds`. Aucun ID token n'est livré. `Session` contient seulement `version:1`, `authentication_id`, `generation`, `expires_at`. La référence d'authentification est un UUID opaque, jamais un credential ou un choix de tenant. La génération est un entier positif `u32`, initialement 1 ; une rotation conserve la référence et incrémente sans boucler. À épuisement, la session est fermée et une nouvelle connexion explicite est requise. L'admission est bornée par l'expiration du token, 1 h absolue et 15 min d'inactivité ; refresh n'allonge pas l'échéance absolue.

`native-intent.action` vaut `{kind:"login"}`, `{kind:"refresh",authentication_id,generation}` ou `{kind:"finish",intent_id,outcome:"cancelled"|"failed"}`. Login ne requiert aucun Bearer et réserve en mémoire une intention bornée, expirable, de durée nominale 300 s avant les appels natifs au fournisseur ; ce TTL ne garantit pas le délai de connexion de bout en bout. `/admit` consomme cette intention et vérifie le nouveau token signé pour le client CLI configuré ; l'intention d'authentification n'est enregistrée durablement qu'après cette preuve vérifiée. Refresh exige l'ancien Bearer CLI encore valide et marque durablement la génération pending avant l'appel fournisseur natif ; son intention et sa complétion sont bornées à 15 s. Finish consomme une intention abandonnée ; pour refresh, il exige aussi l'ancien Bearer exact et ferme l'admission, tandis qu'une intention login ne requiert aucun Bearer et ne persiste rien avant preuve. Le résultat d'audit inconnu/interrompu concerne le refresh authentifié ; une déclaration CLI ne constitue pas une observation de PKCE, nonce ou échange fournisseur par le gateway.

A native refresh begun while valid may complete within its retained 15 s deadline after the old token expires, solely to validate/install a new signed access token of the same principal, issuer and client. `/rotate` consumes that exact intent and old generation/bearer; this exception grants no human read or new refresh under an expired bearer. The final install rechecks the deadline. Refresh pending blocks reads/SSE for the old generation. Successful refresh/rotation replaces it, revalidates memberships/mapping and requires fresh human contexts/cursors. Once refresh is pending, failure, cancellation, expiry or an ambiguous exchange closes local admission; no automatic exchange retry or fallback occurs.

Logout distingue trois résultats : `local_session:"closed"|"unknown"`, `provider_logout:"accepted"|"failed"|"unavailable"|"unsupported"|"not_attempted"` et `audit:"recorded"|"unavailable"`. Seuls référence, génération et Bearer exacts d'une session en mémoire permettent sa fermeture ; un token expiré peut servir à cette seule fermeture connue. Une session inconnue ou une preuve divergente renvoie `unknown/not_attempted/unavailable`, sans appel fournisseur. La fermeture locale précède l'audit/appel IdP ; le client efface aussi son état local avant son appel réseau. Logout natif ne contacte pas l'IdP. `accepted` décrit la réponse de l'endpoint pris en charge, sans promesse de révocation globale des JWT ou des autres sessions.

Les démarrages de connexion initiaux Web et CLI réservent uniquement un état volatil, borné et expirable ; cookie et intention native ne prouvent pas une identité. Avant preuve vérifiée par access token, démarrage, annulation, refus et expiration ne consomment pas l’audit d’authentification durable. Après vérification, une intention doit être durable avant consultation du compte/memberships et admission. Si cette intention ne peut pas être enregistrée, le gateway renvoie `503 audit_unavailable`, sans admission ni audit de lecture de profil tenant. Refresh authentifié, logout et expiration de session conservent leurs audits obligatoires. Ce comportement ne purge ni ne répare un audit déjà plein : conservez sa base et sa politique originales. Un redémarrage perd les tentatives en attente ; recommencez la connexion.

Initial Web/CLI login attempts reserve only bounded, expiring volatile state; a cookie or native intent is not identity proof. Start, cancellation, rejection and expiry before verified access-token proof do not consume durable authentication audit capacity. After verification, a durable intention is required before account/membership reads and admission. If that intention cannot be recorded, the gateway returns `503 audit_unavailable`, without admission or tenant profile-read audit. Authenticated refresh, logout and session expiry keep their mandatory auditing. Existing full ledgers are neither purged nor repaired; preserve their databases and original policy. Restart discards pending login attempts: begin again.

Verified authentication intentions, outcomes and token fingerprints are durable in a separate operator-private audit, not tenant business events or a public HTTP audit feed. Audit failure blocks admission/refresh; logout still reports local closure separately if its audit cannot be recorded. Gateway restart loses all private live admissions, transactions and handoffs. A durable `active` record cannot reconstruct a token owner: sign in again with a newly issued access token. Previously admitted access-token fingerprints remain remembered across restart and cannot be re-admitted/rotated as fresh authority. Tokens themselves are not recoverable from that audit; provider exchange and local audit are not one external transaction.

Les erreurs OIDC suivent `{error:{code,message,retry_action}}`, avec texte prédéfini :400 `invalid_request`,401 `credential_invalid`,403 `origin_denied|permission_denied`,409 `session_conflict`,429 `capacity_exhausted`,503 `oidc_not_configured|audit_unavailable|identity_unavailable|provider_unavailable`. La remise corrélée `/tokens` utilise401 pour ses échecs sauf403 `permission_denied` et503 `audit_unavailable` ; elle peut notamment nommer `credential_expired` ou `login_cancelled`. Les refus HTTP en amont gardent leur propre contrat ; ne pas déduire un statut global à partir du seul code corrélé.

Le gateway réserve avant mutation une capacité distincte de complétion/nettoyage du refresh. Si le traitement d'un refresh par le gateway est annulé, la génération `pending`/`rotating` reste refusée pendant le nettoyage différé. Celui-ci vérifie session, génération, opération et reçu exacts ; il ne peut fermer une rotation plus récente. La réserve couvre aussi les tâches bloquantes en attente. Un arrêt du runtime peut empêcher l'exécution d'une tâche encore en queue : aucune clôture durable n'est garantie par l'annulation, et aucun accès SQLite synchrone de secours n'est exécuté sur Tokio. Un ancien `pending` ne redevient pas une autorisation ; après redémarrage, reconnectez-vous avec un nouveau token, sans réutiliser l'ancienne génération.

The gateway reserves separate refresh completion/cleanup capacity before mutation. If gateway processing of a refresh is cancelled, the `pending`/`rotating` generation remains refused while deferred cleanup runs. Cleanup checks the exact session, generation, operation and receipt, so it cannot close a newer rotation. The reservation also covers queued blocking tasks. Runtime shutdown can prevent a queued task from running: cancellation does not guarantee durable terminal closure, and performs no synchronous SQLite fallback on Tokio. An old pending record never restores authority; after restart, sign in with a new token rather than reusing the old generation.

## Consultation humaine nominative / Named human reads

### Profil owned / Owned source profile

Le défaut reste `legacy`. Le mode explicite `owned_sources`, préparé par les [commandes d’adoption](setup/human-sources-maintenance/README.fr.md) puis `gateway --human-sources-activation`, conserve les DTO humains version1 et adopte seulement les lectures humaines **GET `/api/v1/human/identity`, GET `/api/v1/human/resources`, GET `/api/v1/human/events` et GET `/api/v1/human/events/recent`** (événements en métadonnées seulement). La source retenue et authentifiée détermine l’autorité, jamais un tenant fourni dans la requête. Le credential humain local reste nominatif ; ce profil ne crée aucun endpoint login/password, credential runner ou permission supplémentaire.

La découverte vérifie sa propre source. Si un autre Store tenant est volontairement non consulté, `contexts_complete=false` signifie que ses appartenances restent inconnues même s’il est sain. Sans autre source tenant à consulter, `true` reste possible. Les contextes sont opaques ; les ressources portent un reçu durable. Un Store propre occupé renvoie 503 et un budget de lecture épuisé conserve 429, sans repli legacy ni SQL voisin.

Les méthodes connues des routes non adoptées, y compris HEAD humain, sont refusées 503 `identity_unavailable` avant les extracteurs legacy/Store. Les chemins/méthodes inconnus gardent le routage 404/405. GET/HEAD `/healthz` est une observation mémoire. Les GET existants `/auth/oidc/status` et `/auth/oidc/cli-profile` peuvent annoncer désactivé seulement si OIDC est absent ; sélectionné, il reste retenu avec handlers fermés, sans être annoncé désactivé. Contenu, détail d’événement, export, SSE, écritures, SSO complet et lancement humain de ce profil restent fermés. [Limites opérateur](setup/identity/OWNED_SOURCES.fr.md) · [Preuves logicielles](engineering/evidence/2026-11-13/human-sources-operator/README.fr.md).

Pour `events/recent` owned, l’enveloppe version1 ajoute seulement `stream_mode:"snapshot_only"`, avec `view:"metadata"`, `order:"newest_first"` et `live_cursor:null` sur toutes les pages. Ce marqueur qualifie le profil owned metadata actuel, sans permission supplémentaire. `live_cursor:null` en première page est accepté uniquement avec ce marqueur exact et metadata ; présence undefined, valeur inconnue, content, position live opaque ou pagination incohérente restent refusés. Les curseurs older restent opaques et liés au contexte, à la vue, à la limite, à la ressource et à la borne ; aucune position live n’est construite ou scellée. Le même sélecteur checked conserve ACL avant LIMIT, masquage, bornes et reçu `events` durable avant publication. Identité et contexte sont revérifiés après commit, sans SQL voisin ni repli legacy. Le legacy omet `stream_mode` et conserve sa première position live opaque.

The default remains `legacy`. Explicit `owned_sources`, prepared with the [adoption commands](setup/human-sources-maintenance/README.md) and started through `gateway --human-sources-activation`, retains human DTO version1 and adopts only the human reads **GET `/api/v1/human/identity`, GET `/api/v1/human/resources`, GET `/api/v1/human/events` and GET `/api/v1/human/events/recent`** (event metadata only). A retained authenticated source determines authority, never a caller-supplied tenant. The existing named local human credential remains distinct from technical credentials; this profile creates no login/password endpoint, runner credential or extra permission.

Discovery verifies its own source. When another tenant Store is intentionally not queried, `contexts_complete=false` means its memberships remain unknown even if it is healthy. With no other tenant source to query, `true` remains possible. Contexts stay opaque and resource reads have durable receipts. Own-Store contention returns 503 and an exhausted read budget retains 429, without legacy fallback or neighboring SQL.

Known methods of non-adopted routes, including human HEAD, return 503 `identity_unavailable` before legacy extractors/Store access. Unknown paths/methods retain ordinary 404/405 routing. GET/HEAD `/healthz` observes service memory. Existing GET `/auth/oidc/status` and `/auth/oidc/cli-profile` may report disabled only if OIDC is absent; selected configuration/ledger stays retained with closed handlers, rather than being reported disabled. Content, event detail, export, SSE, writes, complete SSO and human launch in this profile remain closed. [Operator limits](setup/identity/OWNED_SOURCES.md) · [Software evidence](engineering/evidence/2026-11-13/human-sources-operator/README.md).

Owned `events/recent` adds only `stream_mode:"snapshot_only"` to the v1 envelope, with `view:"metadata"`, `order:"newest_first"` and `live_cursor:null` on every page. This marker identifies the current owned metadata profile and grants no permission. A first-page `live_cursor:null` is accepted only with that exact marker and metadata; explicit undefined, unknown values, content, an opaque live position and inconsistent pagination remain refused. Older cursors remain opaque and bound to context, view, limit, resource and bound; no live position is created or sealed. The same checked selector retains ACL before LIMIT, redaction, bounds and the durable `events` receipt before publication. Identity and context are revalidated after commit, without neighboring SQL or legacy fallback. Legacy omits `stream_mode` and retains its opaque first live position.

### Mode legacy / Legacy mode


Le [guide local](setup/identity/README.fr.md) et son [équivalent anglais](setup/identity/README.md) décrivent provisioning, fichiers privés, migration explicite vers5 et reprise ; les guides SSO ci-dessus couvrent l'autre méthode humaine. La preuve humaine est vérifiée à chaque requête/lot ; elle ne donne aucun rôle runner/worker/viewer. Le contrôle propose les contextes issus des memberships durables. Hors `/identity`, fournir un unique `X-AiGuard-Context` proposé ; aucun tenant libre ne choisit le périmètre. Le dashboard ne relaie que ces GET humains exacts, sans repli technique.

Human credentials are verified on every request/batch and never become runner/worker/viewer authority. The control offers contexts from durable memberships; all routes except identity require one offered `X-AiGuard-Context`. The dashboard forwards only the exact human GET allowlist. No caller-supplied tenant chooses scope and no failed human login falls back to technical reads.

`HumanIdentityV1` conserve son enveloppe `version:1` et exige le booléen `contexts_complete`, en plus de la liste `contexts`. `true` indique que toutes les consultations des profils des tenants configurés pour cette réponse ont réussi ; `false` indique qu’au moins une source est indisponible. La liste contient alors seulement les contextes établis par les sources consultables, sans nommer les tenants indisponibles ni deviner des appartenances. Une liste vide avec `contexts_complete:false` n’est pas une preuve d’absence de droits ou de révocation. Le dashboard affiche l’état partiel et propose **Actualiser les contextes** ; la CLI expose le même champ dans sa réponse `identity`.

`HumanIdentityV1` retains its `version:1` envelope and requires the boolean `contexts_complete` alongside `contexts`. `true` means all configured tenant profile lookups for this response succeeded; `false` means at least one source is unavailable. Only contexts established from readable sources are offered; unavailable tenant identifiers and guessed memberships are not disclosed. An empty partial list proves neither absent access nor revocation. The dashboard displays the partial state with **Actualiser les contextes** (refresh contexts); the CLI exposes the same field in its `identity` response.

Ce contrat s’applique aux credentials locaux nominatifs comme aux preuves OIDC admises. Une panne voisine ne sélectionne aucun périmètre. Le store source d’un credential local et le store de chaque contexte sélectionné restent obligatoirement vérifiés ; leur indisponibilité donne `503`, jamais un profil vide. En OIDC, une appartenance active et autorisée établie dans une source saine permet l’admission ; si aucune ne peut être établie et qu’une source est indisponible, le résultat est `503`, pas un refus définitif d’appartenance. Une absence établie sur des sources toutes consultables reste `403`.

The same contract applies to named local credentials and admitted OIDC proofs. A neighboring failure never chooses a scope. A local credential's source store and each selected context's own store must still be checked; their unavailability returns `503`, not an empty profile. OIDC admission may use an approved active membership established in a healthy source. If none is established while a source is unavailable, it returns `503` rather than asserting absent membership; a definitive absence across readable sources remains `403`.

Une saturation de l'admission d'un profil voisin produit aussi cette couverture partielle. La consultation sautée ne réconcilie pas le voisin comme un profil vide et ne révoque pas ses contextes déjà émis. Le store source d'un credential local et le store du contexte sélectionné restent soumis à leurs propres vérifications et limites : leur saturation renvoie `429`, sans produire de liste vide.

A neighboring profile's admission saturation also produces partial coverage. Skipping its lookup neither reconciles it as an empty profile nor revokes its previously issued contexts. A local credential's source store and the selected context's own store still enforce their checks and admission bounds: their saturation returns `429`, never an empty list.

Mettre à jour ensemble gateway, dashboard compilé et CLI natif pour ce contrat : un nouveau client strict refuse le champ manquant ou mal typé, et un ancien client strict peut refuser le champ supplémentaire. Cette évolution de réponse n’ajoute aucune migration de base. Deploy matching gateway, built dashboard and native CLI versions together: a new strict client rejects a missing or malformed field, while an older strict client may reject the added field. This response update introduces no database migration.

`identity.authentication` contient `method`, `credential_id` opaque et `expires_at` RFC3339 ou `null`. `method:"local_credential"` correspond au principal `human_local` et au credential nominatif durable ; `method:"oidc_access_token"` correspond au principal `human_oidc` vérifié `(issuer,subject)` et à une admission OIDC vivante. Dans ce dernier cas, `credential_id` est la même référence opaque que `authentication_id`, pas le token, et l'expiration est celle de l'admission effective. La génération figure dans les DTO d'authentification OIDC ; `/human/identity` ne la publie pas comme un droit supplémentaire.

Human identity explicitly distinguishes local credentials from admitted OIDC access tokens. A signed token without a live admission is insufficient. OIDC capabilities are the intersection of the configured mapping over verified access claims and active control-side memberships, then context mode and resource permissions. Account names, realm administration and a token's caller-supplied tenant claim confer no authority. The mapping never creates a membership; its revision and capability ceiling remain bound during Store revalidation.

| GET `/api/v1/human/…` | Paramètres / Parameters | Résultat / Result |
|---|---|---|
| `identity` | Aucun / None | Principal, authentification, `contexts_complete` et contextes proposés / Principal, authentication, `contexts_complete` and offered contexts. |
| `resources` | `limit`, `cursor` | Ressources explicitement autorisées, propriétaire et droits distincts / Explicitly authorized resources, owner and separate permissions. |
| `events` | `view`, `resource_kind`, `resource_id`, `limit`, `cursor` | Événements de la sélection autorisée / Events from the authorized selection. |
| `events/recent` | Mêmes paramètres que events / Same as events | Mêmes événements, du plus récent au plus ancien, avec `order`, et `live_cursor` sur la première page / Same events, newest first, with `order`, and `live_cursor` on the first page; contrat ci-dessous / contract below. |
| `events/{event_id}` | `view` | Événement autorisé ou404 uniforme / Authorized event or uniform404. |
| `export` | Mêmes paramètres que events / Same as events | Page autorisée, droit export supplémentaire / Authorized page requiring additional export rights. |
| `stream` | Mêmes filtres et curseur opaque / Same filters and opaque cursor | SSE `human_audit`, page JSON, reprise opaque dans `id` / JSON pages with opaque resumption IDs. |
| `brokers` | Seulement `limit`, `cursor` / Only `limit`, `cursor` | Projection d'infrastructure du tenant, administration `read_broker_registry`, schéma7 / Tenant infrastructure projection, administration `read_broker_registry`, schema7; contrat distinct ci-dessous / separate contract below. |

`view` vaut `metadata|content` et `resource_kind` vaut `session|sandbox|agent|project`. Une page de ressources/événements/export contient `version:1`, `context_id`, `authorization_version` opaque64hex, `items`, `next_cursor`, `has_more`, `coverage:"authorized_selection"` et `read_receipt`. Les pages événements ajoutent `view`, l'export ajoute `selection_complete`. Les révisions d'objets autorisés sont des chaînes décimales ; la version d'autorisation est opaque et ne publie pas le compteur global. Aucun total ni position globale n'est déduit de la pagination.

Resource kinds and views are closed enums. Resource/event/export pages contain the fields above; event pages add their view and export adds `selection_complete`. Revisions of authorized objects are decimal strings, while `authorization_version` is an opaque64hex value, never a global revision counter. Metadata excludes commands, result/data and free-text decision reasons. Content requires content permission; export additionally requires export permission. The typed DTOs are in [identity/access.rs](../src/identity/access.rs).

Les limites sont1–100 éléments et256KiB pour l'enveloppe complète ; export serveur16MiB/1000pages/300s par branche. Pour l'export, les clients CLI/Web appliquent aussi25pages/4MiB/60s. Le registre est limité à4096 contextes,64 par credential/principal, durée absolue au plus300s et bornée par l'expiration de la preuve, sans éviction vivante. Les curseurs AEAD conservent précédents/retries pendant leur contexte, sans corps mis en cache ; ils restent liés à la sélection, à la limite, à l'identité et aux droits. Un retry peut produire un autre curseur et un nouveau reçu d'audit. Ces budgets ne sont pas un quota contre les relectures.

Limits apply to the complete envelope, not just items. For export, CLI/Web clients additionally cap traversal at25pages/4MiB/60s; those client export limits do not apply to broker registry pages. Contexts and cursor positions expire or are revoked; restarting the gateway invalidates them. SSE advances its resumption ID only after a complete validated frame. Retries may duplicate events; possession proves neither receipt nor download. Each returned batch requires durable nominative audit before restitution. Revocation does not retract bytes already emitted. A stream error is terminal; an empty poll invents no business receipt.

`GET /api/v1/human/events/recent` lit la même sélection autorisée que `events`, du plus récent au plus ancien dans l'ordre de réception par le contrôle. La query fermée accepte `limit` (défaut25,1–100), `view` (`metadata` par défaut, ou `content`), `resource_kind` et `resource_id` (les deux ou aucun) et `cursor` opaque ; aucun corps, aucun autre champ. La page ajoute `order:"newest_first"` et `live_cursor` à l'enveloppe des pages événements ; `has_more` vaut `next_cursor != null` et signale des événements autorisés plus anciens. La première page (sans `cursor`) fixe une borne à la tête de l'audit lue dans sa transaction : les pages suivantes lisent strictement sous le plus ancien élément rendu, sans trou ni répétition, et un événement enregistré pendant la pagination n'y entre pas. `live_cursor` n'existe que sur cette première page : `stream` l'accepte avec exactement les mêmes `limit`, `view` et ressource, et ne livre alors que les événements enregistrés après cette borne, sans rejouer l'historique. Aucune position d'audit n'est publiée. Un curseur est lié à sa route : `next_cursor` de `events/recent` est refusé par `events`, `export` et `stream`, leurs curseurs sont refusés par `events/recent`, et `live_cursor` n'est accepté que par `stream` ; toute autre limite, vue ou ressource répond400 `invalid_cursor`.

Autorisation, masquage et bornes sont ceux de `events` :1–100 éléments,256KiB pour l'enveloppe complète, une page pouvant porter moins d'éléments que `limit` avec `has_more:true`. Le budget de traversée (1000pages,16MiB, durée du contexte) vaut **par traversée**, c'est-à-dire par chaîne de curseurs, comme pour `events` : une requête sans `cursor` ouvre une nouvelle traversée et `live_cursor` une traversée de flux neuve. Ce budget n'est donc pas un plafond par contexte ; la borne cumulée est le budget de reçus de lecture humaine du principal, avec la fréquence et les places de lecture du tenant. Son dépassement répond429 `capacity_exhausted`, action `select_context`.

Autorisation, projection et reçu durable `control.identity.access` d'action `events` partagent une transaction du Store, page vide comprise ; le reçu compte dans les budgets humains et ne distingue pas cette lecture de la lecture `events`. La route réauthentifie avant publication. Un reçu peut subsister lorsqu'un refus postérieur à cette transaction (révocation, expiration, budget de traversée) empêche la publication : il ne prouve pas une remise. Erreurs :400 `invalid_request|invalid_cursor` ;401 ;403 `context_required|context_invalid|context_expired|context_revoked` (action `select_context`) ou `permission_denied` ;404 `not_found` lorsque la ressource filtrée est absente ou non accordée ;429 `capacity_exhausted|audit_read_budget_exhausted` ;503. Après une mutation d'identité du tenant, une révocation ou une expiration, la route, un `next_cursor` déjà émis et un `live_cursor` déjà émis sont refusés sans élément, et un flux ouvert se termine par une erreur. Route additive, sans migration : un gateway qui ne la sert pas répond404 `not_found` (`recent` n'est pas un identifiant d'événement). Le dashboard la relaie à ce chemin exact.

`GET /api/v1/human/events/recent` reads the same authorized selection as `events`, newest first in the order the control received the events. The closed query accepts `limit` (default25,1–100), `view` (`metadata` by default, or `content`), `resource_kind` with `resource_id` (both or neither) and an opaque `cursor`; no body and no other field. The page adds `order:"newest_first"` and `live_cursor` to the event page envelope; `has_more` equals `next_cursor != null` and reports older authorized events. The first page (no `cursor`) fixes a bound at the audit head read in its transaction: later pages read strictly below the oldest returned item, without gap or repeat, and an event recorded during pagination never enters them. `live_cursor` exists on that first page only: `stream` accepts it with exactly the same `limit`, `view` and resource, then delivers only events recorded after that bound, without replaying history. No audit position is published. A cursor is bound to its route: a `next_cursor` of `events/recent` is refused by `events`, `export` and `stream`, their cursors are refused by `events/recent`, and `live_cursor` is accepted by `stream` only; any other limit, view or resource returns400 `invalid_cursor`.

Authorization, masking and bounds are those of `events`:1–100 items and256KiB for the complete envelope, so a page may carry fewer items than `limit` with `has_more:true`. The traversal budget (1000pages,16MiB, context lifetime) applies **per traversal**, that is per cursor chain, as for `events`: a request without `cursor` starts a new traversal and `live_cursor` a fresh stream traversal. It is therefore not a per-context ceiling; the cumulative bound is the principal's human read receipt budget, with the tenant's read frequency and read slots. Exceeding it returns429 `capacity_exhausted` with `select_context`.

Authorization, projection and the durable `control.identity.access` receipt with action `events` share one Store transaction, including for an empty page; the receipt counts against the human budgets and does not distinguish this read from the `events` read. The route reauthenticates before publication. A receipt may remain when a refusal after that transaction (revocation, expiry, traversal budget) prevents publication: it does not prove delivery. Errors:400 `invalid_request|invalid_cursor`;401;403 `context_required|context_invalid|context_expired|context_revoked` (`select_context`) or `permission_denied`;404 `not_found` when the filtered resource is absent or not granted;429 `capacity_exhausted|audit_read_budget_exhausted`;503. After an identity mutation of the tenant, a revocation or an expiry, the route, an issued `next_cursor` and an issued `live_cursor` are refused without any item, and an open stream ends with an error. Additive route, no migration: a gateway that does not serve it returns404 `not_found` (`recent` is not an event identifier). The dashboard forwards it at this exact path.

Les flux SSE humains partagent une limite de 4 par principal canonique et
24 par tenant, dans le total de 32 flux techniques/humains de ce tenant.
Changer de credential ou de contexte ne crée aucune nouvelle part. Un refus
renvoie `429 capacity_exhausted` avec `retry_action:"close_unused_streams"` ;
fermer les flux inutiles avant reprise. Un échec interne d'admission renvoie
`503 stream_admission_unavailable`, action `contact_operator`. Les permis
restent dans le Body admis jusqu'à son drop, même après erreur terminale/EOF.
Un client non pollé peut retenir sa part. Ces permis ne sont pas ceux des
segments I/O Store et ne bornent pas tous les sockets HTTP.

Human SSE shares a limit of 4 streams per canonical principal and 24 per
tenant, within that tenant's shared total of 32 technical/human streams.
Changing credential or context does not create another share. Rejection is
`429 capacity_exhausted` with `retry_action:"close_unused_streams"`; close
unused streams before reconnecting. Internal admission failure returns
`503 stream_admission_unavailable` with `contact_operator`. The admitted Body
retains its permits until drop, including after a terminal error/EOF. An
unpolled client can occupy its share. These permits are separate from Store
I/O segments and do not bound all HTTP sockets.

Les reçus humains ont aussi un budget cumulatif par tenant et principal,
distinct des limites de pages, de flux et de fréquence. Identité, page vide
REST, export et lot SSE livré consomment chacun un reçu ; un poll SSE vide
n'en consomme pas. Le coût est celui du payload préparé persisté, compté en
événements et octets UTF-8. Rotation, contexte neuf et redémarrage ne rendent
aucun crédit. Une lecture refusée pour budget reçoit
`429 {error:{code:"audit_read_budget_exhausted",message:...,retry_action:"contact_operator"}}`,
sans données ni `Retry-After`. Les nouvelles écritures ordinaires devant la
réserve d'identité reçoivent de même `audit_write_budget_exhausted`. Une base
physiquement pleine reste503. Les profils voisins suivent encore le contrat
partiel ci-dessus ; en OIDC, si aucun profil n'est lisible, la réponse peut
rester503. Voir les
[budgets et procédures EN](setup/identity/README.md#audit-budgets-and-stream-capacity)
et [FR](setup/identity/README.fr.md#budgets-daudit-et-capacité-des-flux).

Human receipts also have cumulative tenant/principal budgets, separate from
page, stream and frequency limits. Identity, an empty REST page, export and a
delivered SSE batch each consume a receipt; an empty SSE poll does not. Cost
is the exact prepared stored payload, in events and UTF-8 bytes. Rotation,
new contexts and restart do not replenish usage. Budget rejection returns
`429 {error:{code:"audit_read_budget_exhausted",message:...,retry_action:"contact_operator"}}`
without data or `Retry-After`. New ordinary writes reaching the identity
reserve similarly return `audit_write_budget_exhausted`. Physical exhaustion
remains503. Neighboring profiles retain the partial-coverage contract above;
OIDC with no readable profile can still return503. Follow the linked budget
configuration/recovery guides instead of retrying a cumulative refusal.

Les contextes/curseurs lient aussi méthode d'authentification, empreinte de la preuve, référence et révision/génération, révision du mapping OIDC, membership, organisation, mode et version d'autorisation. Chaque poll SSE revérifie l'admission et ces droits avant restitution : refresh pending, rotation, logout, expiration ou révocation termine l'ancien flux avec erreur. La nouvelle génération impose de demander `/identity` et sélectionner un contexte neuf ; aucun curseur ni export de l'ancienne génération ne se transfère. SSE and context bindings preserve the OIDC capability ceiling; a new generation cannot reuse the previous authorized view.

Les erreurs humaines sont `{error:{code,message,retry_action}}`.401 ferme la session humaine, même après changement local de contexte ;403 de contexte retire ses données/curseurs et exige une nouvelle sélection ; un refus de ressource ne déconnecte pas toute la session.404 ne distingue pas objet absent et inaccessible.400 refuse entrée/curseur,429 indique capacité,503 contrôle/audit indisponible ; `migration_required`503 demande `contact_operator`, sans repli viewer. Les actions sont `authenticate|select_context|review_permissions|reduce_page|retry|contact_operator|close_unused_streams`.

Human errors use the envelope above.401 closes the human session independently of a previous context change or an unreadable response body. Context revocation/expiry clears that context; an individual resource refusal does not log out the whole session.404 hides absence versus inaccessibility. Capacity and storage/migration failures remain explicit; neither is an empty result or a viewer fallback.

Pour les routes métier `/api/*`, le dashboard ne relaie que les lectures de sa liste explicite, dont les workers, quotas, allocations et la projection humaine brokers ; il n'expose pas la consultation technique des reçus d'opérations du broker ni les mutations. Les opérations d'authentification OIDC suivent leur allowlist distincte ci-dessus. Les lectures paginées workers/usage acceptent uniquement `cursor` (UUID canonique) et `limit` (1 à 100, 50 par défaut). Le curseur est le dernier worker ou la dernière tentative de la page. Un paramètre inconnu est refusé `400` ; le client ne choisit pas le tenant.

## Registre brokers humain / Human broker registry

`GET /api/v1/human/brokers` exige une preuve humaine locale/OIDC admise et un unique `X-AiGuard-Context` proposé par `/human/identity`. Le contexte doit être `administration` et porter `read_broker_registry`. Cette capacité explicite vise l'infrastructure de tout le tenant authentifié ; l'ancien `read_tenant_metadata` garde sa portée ressources filtrée par organisation. Aucune promotion des anciens grants/mappings, aucun rôle IdP implicite et aucun credential runner/viewer/worker ne donnent cette lecture humaine. OIDC exige membership actif **et** plafond explicite sur claims vérifiés. Seul le contrôle de confiance authentifié/hors ligne peut ajouter/réactiver ce droit ; un administrateur d'organisation peut le conserver dormant ou le retirer, jamais l'ajouter/réactiver. Principal et organisation du membership restent immuables.

This human GET requires an admitted local/OIDC proof and one offered `X-AiGuard-Context`, with administration mode and explicit `read_broker_registry`. It covers infrastructure metadata for the authenticated tenant; historical organization-scoped resource permissions stay unchanged. Existing accounts/mappings gain nothing automatically. OIDC needs both an active local membership and an explicitly configured verified-claim ceiling. Organization administrators cannot introduce/reactivate the right; trusted control/offline authority is required. No technical credential or caller tenant selector replaces the human proof.

La query fermée accepte seulement `limit` (défaut25,1–100) et `cursor` opaque facultatif. Aucun corps, filtre tenant/organisation, `view`, `resource_kind`, `after` ou accès de détail/export/stream brokers. Champs inconnus/curseur invalide sont refusés. Le dashboard relaie ce GET vers son gateway fixe et transmet preuve/contexte ; il ne contacte aucun moteur Docker/Podman/Kubernetes.

The closed query accepts only optional `limit` (default25,1–100) and opaque `cursor`. There is no broker detail, export or stream endpoint in this human slice. Unknown query fields are refused. The dashboard forwards this GET with human proof/context to its fixed gateway; it neither receives runner keys nor accesses an engine.

| Champ / Field | Contrat / Contract |
|---|---|
| `version` | `1` ; enveloppe fermée / closed envelope. |
| `context_id`, `authorization_version` | Chaînes opaques64hex correspondant au contexte courant / Opaque64hex strings matching the current selected context. |
| `source`, `consistency`, `runtime_inventory` | Exactement / Exactly `"tenant_broker_registry"`, `"live_keyset"`, `"unknown"`. |
| `evaluated_at` | Heure RFC3339 d'évaluation du registre par le serveur / Server registry evaluation time, RFC3339. |
| `items` | Au plus100 projections `HumanBrokerItemV1` / At most100 typed broker projections. |
| `has_more`, `next_cursor` | Booléen et chaîne opaque si suite, sinon `null` / Boolean and opaque continuation string, otherwise `null`. |
| `read_receipt` | Objet fermé `request_id`, `audit_event_id` (UUIDs), `received_at` (RFC3339) / Closed object with read request/event UUIDs and receipt time. |

Chaque ligne contient seulement `broker_id`, `incarnation_id`, `grant_revision`, `heartbeat_revision`, `state`, `authorities`, `granted_at`, `not_after`, `revoked_at` et `last_heartbeat`. Révisions et séquence heartbeat sont des **chaînes décimales canoniques**, pas des nombres flottants ; `heartbeat_revision` peut être `"0"`. Les IDs broker/incarnation sont des UUIDs canoniques. `authorities` contient1–16 objets fermés `{authority_id,runtime_kind,authority_id_redacted}` : runtime `docker|podman|kubernetes`, ID texte avec `false`, ou `null` avec `true` si masqué. Un identifiant obligatoire ou timestamp nécessitant masquage bloque la restitution au lieu d'être remplacé par une identité inventée.

Each row contains only the named fields above. Grant/heartbeat revisions and heartbeat sequence are canonical decimal **strings**; heartbeat revision may be `"0"`. `authorities` contains1–16 closed records with runtime `docker|podman|kubernetes`. A masked authority ID is `null` with `authority_id_redacted:true`; required identity/time fields that need redaction fail closed. Raw grants, credential IDs/digests, paths, secrets and arbitrary configuration are excluded.

`revoked_at` vaut RFC3339 ou `null`. `last_heartbeat` vaut `null` ou l'objet fermé `{incarnation_id,sequence,received_at,lease_expires_at,heartbeat_provenance:"declared",reception_provenance:"observed"}`. Les timestamps décrivent la réception du contrôle et le bail enregistré ; un heartbeat d'une ancienne incarnation reste visible et ne devient pas celui de l'incarnation courante. Les états du registre sont :

`revoked_at` is RFC3339 or `null`. The optional closed heartbeat object above separates the broker's declaration from observed control reception. An older incarnation's last heartbeat remains visible; it is not reassigned to the new incarnation.

| `state` | Interprétation / Interpretation |
|---|---|
| `unseen` | Aucun heartbeat de l'incarnation courante / No heartbeat for the current incarnation. |
| `current` | Bail enregistré courant / Recorded lease is current. |
| `stale` | Bail non courant / Lease is stale. |
| `expired` | Autorisation expirée / Grant expired. |
| `revoked` | Autorisation révoquée / Grant revoked. |
| `clock_uncertain` | Évaluation incompatible avec l'horloge / Clock prevents a reliable lease evaluation. |

Aucun de ces états n'est une observation d'exécution, d'arrêt, de nombre de workers, de consommation ou de quota libéré. `runtime_inventory:"unknown"` reste obligatoire. Une autorité `kubernetes` est déclarée, sans lancement de pod ou adaptateur qualifié déduit. Les lignes sont celles des vraies mutations du registre ; l'ingestion d'événements ne constitue pas un enregistrement de broker.

None of these states observes a running/stopped worker, usage or quota release. Runtime inventory stays unknown. A declared Kubernetes authority proves no pod launch or qualified adapter. Only real authorized registry operations create rows; audit-event ingestion is no registration substitute.

La lecture trie les IDs broker en ordre croissant et utilise une pagination **live keyset**, sans snapshot, watermark global ou total. Une réponse complète est bornée à256Kio et peut contenir moins que `limit` pour respecter les octets. Un registre/page vide réussi est distinct d'une erreur. Le curseur brokers a son domaine typé et reste lié à la preuve, contexte, version d'autorisation et limite ; il ne s'utilise pas pour ressources, événements, export ou SSE. Une traversée est bornée à1000pages/16Mio et à l'expiration absolue du contexte (au plus300s). Un retry produit son propre reçu ; aucun curseur/reçu ne prouve livraison ou complétude.

Paging uses ascending broker IDs and a **live keyset**: rows may change between pages. There is no snapshot or total. The complete response is bounded to256KiB and may contain fewer rows than the requested limit. Broker cursors cannot be reused on resource/event/export/stream routes or with another proof/context/version/limit. A traversal is bounded to1000pages/16MiB within the context's maximum300s lifetime. Retries have their own receipts and establish no delivery/completeness proof.

Le Store revalide l'autorisation, construit la projection bornée et engage le reçu obligatoire `control.identity.brokers.access` dans une transaction. La route revérifie preuve, mapping, membership et contexte avant publication. Un reçu durable peut donc subsister si une révocation interdit ensuite la restitution ; il ne certifie pas une livraison atomique. Les budgets humains F1 tenant/principal sont partagés avec identité/ressources, même pour une page vide. Panne d'audit, masquage impossible ou stockage indisponible ⇒ aucune page. Le reçu n'est ni une signature externe ni une preuve runtime.

Authorization, bounded projection and mandatory durable `control.identity.brokers.access` receipt share one Store transaction; the route reauthenticates before publication. A receipt may remain when a subsequent revocation prevents publication; this is not atomic delivery. Empty pages share the existing tenant/principal human F1 budgets. Mandatory audit/storage failure returns no page. The receipt is neither an external signature nor runtime evidence.

| Résultat / Result | Action |
|---|---|
| `403 permission_denied` | Droit administration absent ; faire vérifier membership/plafond, sans repli / Verify explicit membership/ceiling; no fallback. |
| `401` ou contexte refusé / or refused context | Effacer les données concernées, se reconnecter/sélectionner un contexte proposé / Clear affected data, authenticate/select an offered context. |
| `503 migration_required`, `contact_operator` | Migration6→7 hors ligne sauvegardée / Explicit backed-up offline6→7 migration. |
| `429 audit_read_budget_exhausted`, `contact_operator` | Refus cumulatif ; arrêter les retries, conserver base/politique/reçus et réparer la capacité / Cumulative refusal; stop retries, preserve database/policy/receipts and repair capacity. |
| Autre429 / Other429 | Limite temporaire d'admission ou traversée ; suivre son `retry_action` / Temporary admission or traversal bound; follow its action. |
| `503 audit_unavailable` / source indisponible | Réparer le stockage obligatoire ; jamais une liste vide / Repair mandatory storage; never an empty list. |

La migration explicite6→7 conserve la même SQLite, vérifie un backup neuf et ajoute atomiquement son marqueur chaîné de capacité. Les nouveaux lecteurs acceptent encore5/6 ; le nouveau droit et cette lecture sont refusés avant7. Aucune activation ni attribution implicite. Les guides [EN](setup/workloads/README.md#enable-named-broker-registry-reads)/[FR](setup/workloads/README.fr.md#activer-la-lecture-nominative-du-registre-brokers) couvrent5→6→7 depuis setup5. Le registre humain ne qualifie ni fédération, transport brokers/workers, agrégation complète, capacité100 ou production ; l'usage non observé reste inconnu et `billable=false` inchangé.

Explicit6→7 migration verifies a fresh backup and atomically adds the capability marker in the same tenant SQLite. New readers retain5/6 compatibility, but the new right/read requires7. No automatic activation or grant occurs. Follow the linked EN/FR procedures, including setup5. This view qualifies no federation, broker/worker transport, complete aggregation, capacity100 or production release; unknown usage and the existing nonbillable contract remain unchanged.

### Santé et provenance du juge / Health and judge provenance

Le gateway conserve la réponse saine `200 {status:"ok",service:"gateway",version:...}`. Il renvoie `503` avec `status:"unavailable"` et `reason:"configuration_or_audit_unavailable"` si ses credentials sont invalides, aucun Store n'est utilisable ou un échec d'intégrité a déjà été verrouillé. Ce contrôle mémoire ne lance ni requête Jev ni vérification complète du stockage. Un juge configuré n'est pas déclaré joignable par cette sonde.

The gateway preserves its healthy `200` response. Invalid credentials, no usable Store or an already latched audit failure produce `503` with the fixed reason above. This in-memory probe does not contact a judge or perform a fresh durability/integrity scan; configured does not mean reachable.

Le gateway attache à ses Stores une vue immuable réunissant les masques du contrôle, du backend et des Stores configurés. Le Store original fourni par l'appelant n'est pas modifié ; les événements déjà persistés ne sont ni remasqués ni réécrits. The gateway uses immutable Store siblings with the combined redaction dictionary; caller-owned Store instances and previously persisted events remain unchanged.

La réponse de décision et son événement durable portent une métadonnée `jev` additive, ou `null` sans juge supplémentaire. Les replays rendent l'ancienne métadonnée, sans réévaluation ni réattribution. Les nouvelles évaluations distinguent :

| Champ / Field | Contrat / Contract |
|---|---|
| `backend` | `typesafe`, `rust_rules`, `local_openai` ; les sélecteurs CLI des deux derniers utilisent des tirets |
| `selection_source` | `cli`, `environment`, `legacy` ou `programmatic` |
| `status` | `configured`, `skipped_local_policy`, `evaluated` ou `unavailable_or_invalid` ; aucune preuve d'effet runtime |
| `config_sha256`, `configuration` | Empreinte et paramètres non secrets résolus ; aucun chemin de credential ni valeur de clé |
| `requested_model`, `model` | Modèle demandé et modèle déclaré par la réponse, distincts ; `null` lorsque non disponible |
| `confidence`, `usage` | Données fournisseur validées ou `null` ; Rust/local n'inventent aucun score, une inconnue ne devient pas zéro |
| `provider_outcome`, `outcome` | Pour le modèle local, avis fourni et résultat effectif distincts ; Allow consultatif devient Review, puis refus API |

Decision responses and their durable events carry the additive metadata above; historical replays preserve the original record. Provider-reported scores/tokens do not become observed resource consumption or a bill. The mandatory Rust policy runs first. Unknown Rust-catalogue input, local advisory uncertainty and judge errors cannot authorize execution.

Le transport TypeSafe valide les doublons de champs décisionnels/score/usage avant conversion en JSON générique. Son usage réseau est normalisé aux compteurs connus entrée/sortie, chacun entier non négatif ou `null` ; les extensions non utilisées ne sont pas restituées. Le type public Rust `JevAnswer.usage` reste un `Value`. Le helper historique `parse_answer(Value)` conserve cette valeur fournie par l'appelant, mais ne peut retrouver des doublons déjà éliminés en amont. Pour la configuration et les limites, voir [le guide FR](setup/providers/README.fr.md#choisir-le-moteur-de-décision) et [the English guide](setup/providers/README.md#choose-the-decision-engine).

L'ingestion `POST /api/events` (et son alias `/api/v1/events`) refuse avec `403` les racines `security.decision`, `control`, `worker` et `usage.allocation`, ainsi que leurs descendants séparés par un point. Les décisions centrales sont produites par `/decide` ; un événement fourni par le runner ne peut créer une autorisation en cache. Les déclarations fournisseurs telles que `usage.reported` restent admises. Ce contrôle s'applique aux nouvelles ingestions : il ne permet pas d'authentifier rétroactivement un événement usurpé dans une ancienne base. L'historique n'est ni supprimé ni réécrit automatiquement ; une future admission d'exécution ne doit pas convertir ces anciens événements en droits de lancement.

## Diagnostic de transport Jev / Jev transport diagnostics

Le champ historique `jev.error` reste inchangé. Certains refus `transport_unavailable` ajoutent `jev.diagnostic` : `hop:"configured_endpoint"`, `phase:"send"|"response_body"`, `kind:"timeout"|"connection_error"|"http_status"|"transport_error"` et, si observé, `http_status`. Aucun corps, URL, header ou erreur reqwest brute n'y est conservé. Un502 reçu d'un relais décrit ce relais, pas nécessairement le statut ou la cause du modèle derrière lui. L'absence de diagnostic n'autorise aucune conclusion de succès.

The legacy error code is preserved. Optional diagnostics report only the configured endpoint's observed failure phase/category/status, without upstream bodies or secret-bearing details. They do not establish a model's hidden cause, successful evaluation, or resource usage. Existing decision replays retain their original metadata; they are not diagnosed again.

Pour le backend local, configuration/fingerprint incluent les paramètres fixes effectivement envoyés (256tokens maximum, température0, sans streaming, formatJSON) et le hash du prompt. Une collision de masque avec une clé/rôle/prompt/paramètre fixe refuse avec `invalid_configuration` avant POST. Les données utilisateur restent masquées normalement. Délais5s/connexion2s, absence de retry/repli et plafond consultatif Review restent inchangés. For an admitted local call, the fixed-frame mask check keeps the recorded prompt hash consistent with the sent prompt; it never disables redaction to obtain a response.

## Demande de décision

```json
{
  "request_id": "0169b1de-e9d3-4eed-8f27-af2f5fd97493",
  "profile": "strict",
  "command": {"argv": ["/usr/bin/id"], "cwd": "/workspace"},
  "project_id": "project-example",
  "sandbox_id": "sandbox-example",
  "agent_id": "agent-example",
  "session_id": "session-example",
  "correlation_id": "correlation-example",
  "trace_id": "trace-example",
  "human_approved": false,
  "approval_hash": null
}
```

Une approbation conserve cette demande, ses IDs, son profil et sa commande. Seuls `human_approved` et `approval_hash` changent. Le hash est calculé sur la représentation sérialisée de la demande avec ces deux champs remis à leurs valeurs neutres. Le CLI utilise la même fonction Rust que la gateway. Ne pas recalculer le hash en supposant qu'un JSON réordonné est équivalent au format canonique du type Rust.

Le contrat du checkout ajoute `execution_mode` facultatif : absent signifie batch et conserve exactement les JSON/hashes historiques ; la seule valeur présente reconnue est `"interactive_tty_v1"`. Elle fait partie du hash de demande et de l'événement durable `security.decision`. Une approbation batch ne peut donc pas autoriser un canal interactif. Modifier ce mode sous la même identité constitue un conflit, pas une nouvelle décision implicite. Les valeurs inconnues sont refusées. Ce nouveau transport reste [en qualification](engineering/evidence/2026-10-30/interactive-cli/README.md), distinct du candidat batchv2 déployé en QA.

`provider_resolution` est un second champ facultatif, réservé à la sélection fournisseur gérée. **Son omission** conserve les octets/hashes historiques ; **`null` explicite est refusé**, comme une structure ou une variante inconnue. Exemple de résolution publique, sans credential :

```json
{
  "version": 1,
  "client": "claude_code",
  "auth_mode": "api",
  "transport": "batch",
  "profile_id": "claude-api",
  "profile_revision": 1,
  "model": "claude-haiku-5-5",
  "credential_binding_id": "ccaf8b98-84f5-4346-a030-c30246145821",
  "credential_names": ["AI_GUARD_PROVIDER_BOOTSTRAP"],
  "invocation_preset_version": 1,
  "provenance": {"client": "cli", "auth_mode": "cli", "profile": "profile", "model": "cli"}
}
```

Les clients sérialisés sont `codex` et `claude_code`, les modes `api` et `subscription`, les transports `batch` et `interactive_tty`. Les provenances sont `cli`, `profile`, `default_profile` ou `auto`. Le modèle est facultatif et omis lorsqu'il n'est pas fixé. Le binding est un UUID opaque ; ni source privée ni digest de credential ne figurent dans cette structure. L'argv autorisé est celui du bootstrap Rust avec ses seules options publiques ; le prompt en fait partie et ne doit contenir aucun secret.

La résolution entière participe au hash, à l'événement durable et à la vérification de l'admission ; un changement client/mode/transport/profil/révision/modèle/binding exige une nouvelle demande. Le rejeu d'audit relit ce type strict sans rétrograder une valeur invalide en demande historique. L'identité tenant runner/viewer reste indépendante de l'authentification fournisseur. Les [commandes et profils livrés](engineering/PROVIDER_AUTH.md#état-livré-et-limites--30-octobre-2026-après-revue) ne constituent pas une nouvelle API HTTP de secrets ou de lancement.

La réponse est de la forme :

```json
{
  "decision": {"outcome": "allow", "engine": "deterministic", "rule": "exact_allowlist", "reason": "Invocation exacte présente dans la politique locale."},
  "request_hash": "<sha256>",
  "jev": null,
  "audit_seq": 12,
  "audit_hash": "<sha256>",
  "receipt": {
    "version": 1,
    "request_id": "0169b1de-e9d3-4eed-8f27-af2f5fd97493",
    "request_hash": "<sha256>",
    "phase": "initial",
    "audit_seq": 12,
    "audit_hash": "<sha256>"
  }
}
```

Le broker refuse une réponse sans acquittement d'audit durable ou ne correspondant pas au hash demandé. Un refus n'est pas transformé en autorisation par une confirmation humaine. La gateway n'exécute jamais le contenu du champ `command`.

Une demande possède une identité durable `request_id` dans la base de son tenant. Répéter le même contenu et la même phase (`human_approved=false` ou `true`) restitue la décision, le résultat Jev et `audit_seq` du premier événement engagé, y compris après redémarrage ou concurrence. Modifier le contenu en conservant cette identité renvoie `409`. Pour une nouvelle évaluation, créer une nouvelle demande avec un nouvel UUID ; une décision déjà engagée n'est pas réévaluée implicitement après changement de politique ou indisponibilité Jev.

La phase approuvée exige une décision initiale `review` déjà durable et antérieure à l'approbation, ainsi que son hash exact. L'absence de revue ou un faux hash renvoie `403`, même si une autorisation approuvée existe déjà en cache. La phase initiale répétée continue à restituer la revue initiale ; seule la phase approuvée peut restituer la décision approuvée. Un historique ancien contenant plusieurs décisions pour une même phase ou des métadonnées incomplètes renvoie `409`, sans fusion ni modification des preuves.

Une décision déjà durable peut être relue lorsque la capacité d'audit est pleine ; une nouvelle décision reste refusée `503`. Un audit corrompu, un schéma inattendu ou une substitution du fichier bloque aussi les reprises. Une identité dont les métadonnées seraient altérées par le masquage des secrets est refusée `400` avant persistance. La réponse idempotente n'est pas un ticket de lancement : l'admission ci-dessous exige une nouvelle vérification.

## Admission et cycle de vie des workers

Les contrats sérialisés stricts font autorité dans [src/control.rs](../src/control.rs). Une réservation `AdmissionV1` contient `version:1`, un `operation_id` UUID, la demande de décision originale, son `receipt` exact et une spécification `launch` complète. Cette dernière lie session, commande/profil, autorité et type de runtime, référence et SHA-256 de l'image, réseau, ressources, durée, profil de sûreté, absence de restart, empreinte du manifeste de copie, version d'import, fournisseur et noms de credentials. Elle ne transporte aucune valeur de secret. Le profil livré fixe actuellement 2 CPU, 1 Gio et 256 PID ; il ne prétend pas offrir des limites arbitraires.

Pour un fournisseur géré, `launch.provider_resolution` est exactement égal à celui de `decision_request`, avec omission historique ou refus de `null` explicite selon la même règle. `launch.credential_binding_id` et `launch.credential_names` doivent correspondre à la résolution, les noms étant exactement `["AI_GUARD_PROVIDER_BOOTSTRAP"]`. `launch.provider` conserve le normaliseur (`codex` ou `claude`). Client, mode, transport, modèle et forme canonique de l'argv sont vérifiés côté serveur ; observe + bridge sont exigés. La validation refuse une nouvelle admission gérée en presetv1 et exige NoLogs avec batchv2 ou TTYv3 cohérent avec le transport. Une invocation dont `argv[0]` est le chemin canonique `/usr/local/bin/ai-guard-provider` sans résolution gérée est également refusée ; ce test textuel n'atteste pas l'identité de tous les alias ou programmes transitifs.

Le champ `launch.security_preset` accepte `fixed_sandbox_v1`, `fixed_sandbox_v2` et, dans le nouveau contrat interactif, `fixed_sandbox_interactive_v3` ; toute autre valeur est refusée. Les nouveaux lancements batch préparés par le broker utilisent `fixed_sandbox_v2`. Ce preset conserve le sandbox batch sans terminal ni entrée standard (`Tty=false`, `OpenStdin=false`, `StdinOnce=false`) et exige que le moteur ait désactivé son journal de sorties (`LogConfig.Type=none`). Le broker inspecte cette configuration avant le démarrage et lors des observations ; l'inspection publique porte le même `security_preset` que la spécification autorisée. L'admission seule n'est pas une preuve de cette configuration moteur. La capture de sorties demandée au broker reste un mécanisme distinct du journal moteur.

Le preset interactif exige la correspondance exacte avec `execution_mode=interactive_tty_v1`, `capture_output=false`, TERM fixé par le contrôle et aucun credential nommé TERM. TTY/stdin sont ouverts, journal moteur désactivé ; Docker attend `StdinOnce=true`, différence établie sur CLI29.2.1, et la forme Podman attend false sans qualification moteur acquise. Le fingerprint privév3 lie ces faits et TERM sans diffuser les autres valeurs Env ; les algorithmes v1/v2 restent inchangés. Aucune route HTTP de terminal n'est créée : le transport appartient au CLI de confiance. Les frappes et écrans ne sont pas des événements d'audit ; les compteurs stdout/stderr séparés sont inconnus (`null`) et les événements fournisseur TUI indisponibles. Lifecycle et diff conservent leur provenance générique.

`AdmissionV1`, `LaunchSpecV1`, les observations et les routes restent en version 1. La sérialisation et les hashes historiques de `fixed_sandbox_v1` sont conservés, sans migration de base ni réécriture de worker. L'API de contrôle accepte encore une admission explicite v1 d'un runner de confiance **sans résolution fournisseur gérée** ; le nouveau broker refuse son premier démarrage. Les workers v1 déjà démarrés restent observables, arrêtables et finalisables. Une tentative de changer le preset d'une opération déjà admise est un conflit `409`, pas une mise à niveau implicite.

La garantie de désactivation du journal moteur requiert un preset qui l'exige (batchv2 ou interactifv3) et un broker/moteur ayant effectué l'inspection correspondante. Elle ne couvre pas un ancien broker ou client de contrôle utilisant le preset v1. Les composants de contrôle doivent donc être mis à jour ensemble avant une publication revendiquant cette garantie ; une lecture d'historique v1 n'atteste pas rétroactivement l'absence de sorties dans le journal moteur. La preuve Docker batchv2 reste distincte de la qualification TTYv3 encore ouverte.

L'enveloppe version1 est conservée avec variantes explicitement versionnées. Un ancien serveur qui ne connaît pas le champ/mode/preset interactif le refuse ; aucun repli batch n'est autorisé. Coordonner la mise à jour du gateway et du broker. Le réattachement TTY après claim consommé n'est pas implémenté : refus explicite, sans start/restart/exec ; observation et arrêt restent les procédures disponibles. Cette limite et celle du timeout après mort du broker restent des écarts obligatoires de production.

Une nouvelle admission réévalue la politique courante et, si configuré, Jev ; un ancien `allow` n'autorise pas à lui seul le lancement. L'audit, le worker et la réservation sont engagés dans une transaction. La réponse `AdmissionResultV1` contient `accepted`, `refusal`, `worker`, `receipt` et `replayed`. Première acceptation : `201` ; replay identique : `200` ; refus politique durable : `403` ; refus quota durable : `429`. Même `operation_id` avec contenu divergent : `409`. Rejouer un refus restitue son résultat ; une nouvelle tentative exige une nouvelle identité d'opération.

Le contrôle attribue les identités du worker/de tentative et les noms du conteneur/volume ; un nom de session fourni par le client ne sélectionne pas un volume existant. Une mutation `WorkerMutationV1` porte `version:1`, `operation_id`, `worker_id`, `attempt_id`, `expected_revision` et `transition` avec discriminant `action`. Les actions sont `prepare`, `create_issued`, `created`, `start_claim`, `observe`, `unknown`, `stop`, `cancel_reserved`, `cleanup_result`. Le worker et son audit évoluent ensemble. Une révision périmée ou un état incompatible est refusé `409`.

Le claim de démarrage renvoie `disposition:emit_once` seulement lors de sa consommation initiale durable. Son replay renvoie `already_consumed` : il ne permet jamais de réémettre un start. Les états `reserved`, `preparing`, `created`, `start_issued`, `running`, `stopping`, `terminal`, `in_doubt`, `cancelled` distinguent les preuves connues. `in_doubt` conserve sa réservation ; un timeout de lease ne constitue pas une preuve d'arrêt. Une observation terminale complète et cohérente produit atomiquement l'allocation non facturable et la libération du quota. Les données manquantes, redémarrages ou écarts de configuration restent à réconcilier.

Ces API sont des contrats du contrôle de confiance ; une déclaration par le runner ne prouve pas à elle seule un confinement. Le broker inspecte le vrai moteur avant start et lors des observations. La qualification du parcours, les limites de reprise et les obligations encore ouvertes sont dans [CURRENT_STATE.md](engineering/CURRENT_STATE.md). Les anciennes bases v1 restent lisibles mais les routes du registre renvoient `503` avec `migration_required` jusqu'à une migration explicite sauvegardée ; voir [les opérations](operations.md).

Les schémas3 à7 conservent le protocole `broker_supervision_v1` ; les routes ci-dessous exposent les transactions du Store Rust. Le schéma par défaut reste2. Les [migrations CLI explicites](setup/broker/README.fr.md#mettre-à-jour-le-stockage-explicitement) progressent d'une version avec sauvegarde neuve ; elles ne sont pas une route d'activation ou une installation de service OS. Une base3 à7 rouverte/restaurée est en maintenance : les effets frais sont refusés avec `503 supervision_inactive`. La capacité d'origine neuve est non sérialisée ; le numéro de schéma seul n'autorise aucun start. Réactivation après restart et migration des anciens Created restent des exigences ouvertes, sans repli legacy. [Preuves historiques du schéma3 et limites du Store](engineering/evidence/2026-10-30/supervision-store/README.md).

<a id="owned-control-a0"></a>

### Diagnostic technique owned A0 / Owned technical diagnostic A0

**Contrat candidat A0, 13 novembre 2026 — code et schéma JSON alignés ; validation logicielle ciblée sur `0114013` : 37 tests réussis sans échec ni test ignoré, compilation et vérification toutes cibles réussies.** Ces résultats logiciels ne qualifient ni l’isolation runtime ni un déploiement en production. Les preuves déjà publiées des lectures humaines de métadonnées conservent leur source et leur périmètre ; elles ne qualifient pas A0.

Le raccord candidat admet seulement le GET exact `/control/api/v1/control/protocol` sur le transport de contrôle réservé existant, sans query. La classification provient du gateway ; un header client ne peut pas la fabriquer. Le GET public `/api/v1/control/protocol`, HEAD sur ce diagnostic et les mutateurs owned connus restent fermés. Les chemins et méthodes inconnus conservent leurs refus de routage 404/405. Le mode legacy et ses rôles existants sont conservés ; aucun listener, endpoint ou flag de démarrage n’est ajouté.

L’acteur technique `runner` est issu du vrai `CredentialIndex` et du `ReviewCredentialRegistry` privé effectivement retenu. L’entrée doit être technique et unique dans l’index complet de démarrage ; collisions, credentials humains/viewer/workload, état périmé, source absente ou binding refusé ne deviennent pas une identité de contrôle. Tenant, source, namespace et génération viennent de cette autorité, sans choix de tenant par le caller. `HumanSourceAccess` prouve l’accès à une source, pas l’identité d’un acteur ; aucun DTO humain ou diagnostic ne devient un permis.

Le vrai binding est vérifié avant lecture des faits, dans la même transaction checked `Business` sur le Store retenu. Le lecteur partagé conserve les vérifications du Store, l’entrée propre sans attente et la restauration du timeout SQLite antérieur ; aucun SQL voisin ni repli legacy n’est utilisé. Le binding est revalidé avant commit, puis credential, source et epoch avant publication. Usage accepté et permis d’opération/IO restent attachés au travail réel jusqu’à sa fin, y compris si le futur HTTP est abandonné ; cette propriété est couverte par la validation logicielle ciblée. Le diagnostic conserve son contrat d’audit existant : aucun nouveau record, événement ou reçu d’audit n’est ajouté.

La réponse reste `ControlProtocolV1`, `version:1`, avec les seuls faits `schema_version`, `control_namespace_uuid`, `tenant_id`, `role`, `protocols` et `active`. Dans le source A0 candidat, le validateur Rust et le schéma JSON décrivent les formats 1–15, dont 14/15 pour ce raccord owned. `active` reste le fait réel de la lease, sans activation forcée, champ Ready ou permission implicite. Un futur diagnostic 200 décodé par `AuditClient::control_supervision_identity` ne qualifierait pas le broker : sa garde 3–13 conserve `IncompatibleControl` pour 14/15, avant les effets de lifecycle ; la garde du relais 6–13 ne change pas. Aucun lancement humain, worker, egress ou permis de connexion n’est adopté.

La couverture W0 reste bornée à l’index complet construit au démarrage et à la génération du seul Store sélectionné, comparée au snapshot de découverte dans la transaction. Une divergence impose `require_rebuild`, ferme l’admission et exige une reconstruction contrôlée ; elle ne signifie ni identité absente ni autorisation de repli. Les writers HTTP owned restent fermés. Les ports internes à `WriteEpoch`/source, notamment `mutate_identity_scoped`, `provision_local_human_scoped`, `provision_review_binding` et `revoke_review_binding`, restent de vrais writers ; aucun caller serveur post-démarrage n’y est raccordé dans cette source. Un futur writer qui modifie les credentials, workloads ou leur couverture doit fermer l’admission avec `require_rebuild` **avant l’effet**, puis reconstruire l’union complète. Une lecture de B ne devine pas une nouvelle collision chez A sans producteur d’invalidation global. La révocation du binding propre est, elle, relue à chaque diagnostic. Ce contrat ne garantit aucune cohérence multiprocessus.

**A0 candidate contract, 13 November 2026 — code and JSON Schema aligned; targeted software validation on `0114013`: 37 tests passed with no failures or ignored tests, and compilation and all-target checking passed.** These software results do not qualify runtime isolation or production deployment. Existing published human-metadata evidence retains its own source and scope and does not qualify A0.

The candidate admits only exact GET `/control/api/v1/control/protocol` on the existing reserved control transport, with no query. Gateway classification cannot be minted by a client header. Public GET `/api/v1/control/protocol`, HEAD on this diagnostic and known owned mutations remain closed. Unknown paths/methods retain ordinary 404/405 routing. Legacy and its existing roles remain unchanged; no listener, endpoint or startup flag is added.

The technical `runner` actor comes from the actual `CredentialIndex` and retained private `ReviewCredentialRegistry`. Its entry must be the unique technical candidate in the complete startup index. Collisions, human/viewer/workload credentials, stale state, a missing source or refused binding cannot become control identity. Tenant, source, namespace and generation come from that authority, never caller selection. `HumanSourceAccess` proves source access, not actor identity; neither a human DTO nor a diagnostic becomes a permit.

The real binding is verified before reading facts in the same checked `Business` transaction on the retained Store. The shared reader keeps Store checks, own-source no-wait entry and prior SQLite timeout restoration, with no neighboring SQL or legacy fallback. Binding revalidation precedes commit; credential, source and epoch are revalidated before publication. Accepted usage and operation/IO permits stay with the actual work until it finishes, including after HTTP-future abandonment; this behavior is covered by the targeted software validation. Existing diagnostic auditing is retained without adding an audit record, event or receipt.

The response remains `ControlProtocolV1`, `version:1`, with only `schema_version`, `control_namespace_uuid`, `tenant_id`, `role`, `protocols` and `active` facts. In the candidate A0 source, the Rust validator and JSON Schema describe formats 1–15, including 14/15 for this owned path. `active` remains the actual lease fact, with no forced activation, Ready field or implied permission. A future 200 diagnostic decoded by `AuditClient::control_supervision_identity` would not qualify the broker: its 3–13 gate still returns `IncompatibleControl` for 14/15 before lifecycle effects, and the relay 6–13 gate is unchanged. No human launch, worker, egress or connection permit is adopted.

W0 coverage remains the complete startup index plus the selected Store's own generation, compared with the discovery snapshot in the transaction. A mismatch requires `require_rebuild`, closed admission and controlled reconstruction, never an absent-identity interpretation or fallback. Owned HTTP writers remain closed. Internal source/`WriteEpoch` ports such as `mutate_identity_scoped`, `provision_local_human_scoped`, `provision_review_binding` and `revoke_review_binding` remain real writers, with no post-startup server caller connected in this source. Any future writer changing credentials, workloads or candidate coverage must call `require_rebuild` **before the effect** and reconstruct the complete union. A read from B cannot discover a new collision in A without a global invalidation producer. Its own binding revocation is still reread on every diagnostic. This is not a multiprocess consistency guarantee.

<a id="owned-policy-a1"></a>

### Administration owned du registre de politiques A1 / Owned policy-registry administration A1

**Contrat A1 du registre de politiques.** Les POST de mutation replace/revoke existent dans la source `db2678d42cff9c8d6d15bf31284a989732cc6f0d`, qui possède une [preuve logicielle ciblée datée](engineering/evidence/2026-11-14/owned-egress-policy-api-cli/README.fr.md). Cette preuve reste liée à cette source, pas à un autre HEAD ni à une qualification production. Les mutations restent de portée `policy_registry_only`. Le [parcours opérateur](setup/identity/OWNED_SOURCES.fr.md#owned-policy-a1-operator) acquiert explicitement les métadonnées par `inspect`, puis dérive localement une requête avec `prepare --inspection-dir`. Ce raccord conserve un rapport historique `policy_registry_metadata`, sans permission ni verrou de fraîcheur ; les contrôles A1 finaux restent obligatoires.

**A1 policy-registry contract.** The replace/revoke mutation POSTs exist in source `db2678d42cff9c8d6d15bf31284a989732cc6f0d`, with [dated targeted software evidence](engineering/evidence/2026-11-14/owned-egress-policy-api-cli/README.md). That evidence belongs to this source, not another HEAD or production qualification. The mutations have `policy_registry_only` scope. The [operator path](setup/identity/OWNED_SOURCES.md#owned-policy-a1-operator) explicitly obtains metadata with `inspect`, then derives a local request with `prepare --inspection-dir`. This adds a historical `policy_registry_metadata` report, not permission or a freshness lock; final A1 checks remain mandatory.

| Méthode et cible exactes / Exact method and target | Identité / Identity | Corps / Body | Succès / Success |
|---|---|---|---|
| `POST /control/api/v1/egress/policies/inspect` | Même runner/source réels / Same actual runner/source | `EgressPolicyInspectionRequestV1` | `200`, métadonnées canoniques historiques / canonical historical metadata |
| `POST /control/api/v1/egress/policies/replace` | Runner technique lié à la vraie source / Technical runner bound to the actual source | `EgressReplacePolicyRequestV1` | `200`, reçu E1 canonique / canonical E1 receipt |
| `POST /control/api/v1/egress/policies/revoke` | Même autorité / Same authority | `EgressRevokePolicyRequestV1` | `200`, même format de reçu / same receipt format |

<a id="owned-policy-inspection"></a>

#### Inspection réservée des métadonnées / Reserved metadata inspection

Parcours composé vérifié sur `45179db` : [preuve FR](engineering/evidence/2026-11-14/owned-policy-inspection/README.fr.md) / [EN evidence](engineering/evidence/2026-11-14/owned-policy-inspection/README.md). Les limites de cette preuve logicielle restent distinctes des garanties d’exécution réseau / Software evidence does not establish network execution or enforcement.

`POST /control/api/v1/egress/policies/inspect`, sans query, est une lecture technique distincte des writers replace/revoke. Il exige le vrai runner technique, son candidat unique dans l’index complet, son binding actif, la source retenue saine et le **format15 déjà installé**. Aucun alias public/legacy, GET/HEAD ou champ client ne fournit cette autorité. L’identité/source/capacité sont admises avant lecture du corps. Le corps fermé est borné à65 536 octets déclarés et lus, avec upload5s total/2s inactif. Exiger un seul `Content-Type: application/json` exact ; l’extracteur le contrôle après buffering, pas avant corps.

The exact reserved POST, without query, is a technical read separate from replace/revoke writers. It requires the actual technical runner/unique complete-index candidate, active binding, healthy retained source and **already installed format15**. Public/legacy aliases, GET/HEAD or caller fields grant no authority. Identity/source/capacity precede body polling. The closed body is capped at65,536 declared/streamed bytes, with5s total/2s idle upload limits. Require one exact `Content-Type: application/json`; the extractor checks it after buffering.

Le corps ne porte que `version:1` et `selector`. Tenant ne sélectionne que la politique du tenant authentifié ; Session porte un UUID canonique non nul. Tenant/scope/principal/acteur/clock/operation_id/CAS/plafond et champs inconnus sont refusés. The body has only version and selector; neither variant selects another authenticated tenant. Session UUID must be canonical and nonnil; unknown or authority/CAS fields are refused.

Exemples de forme uniquement ; remplacer l’UUID Session par celui réellement choisi. Wire-shape examples only; use the actual selected Session UUID.

```json
{"version":1,"selector":{"kind":"tenant"}}
```

```json
{"version":1,"selector":{"kind":"session","session_id":"11111111-1111-4111-8111-111111111111"}}
```

`200` fournit uniquement `EgressPolicyInspectionV1` canonique, ≤4 096 octets, `Content-Type: application/json`, `Cache-Control: no-store`. Les champs suivent cet ordre canonique : `version`, `schema_version`, `scope`, `selector`, `head`, `tenant_ceiling`, `session_resource`, `observed_at`, `coverage`, `historical`. The closed canonical response is metadata only, bounded to4,096bytes with both exact headers; version is1, schema_version15, coverage `policy_registry_metadata`, historical true.

| Champ / Field | Forme fermée et sens / Closed shape and meaning |
|---|---|
| `scope` | Vrais tenant_id du Store authentifié et control_namespace_uuid durable / Actual authenticated Store tenant and durable namespace, using `EgressPolicyExpectedScopeV1`. |
| `selector` | Exactement celui de la requête / Exact requested selector. |
| `head` | `{"presence":"absent"}` ou/or `{"presence":"present","revision":…, "epoch":…, "state":"active"}` ; state peut aussi être/may also be `revoke_requested`. Révision/époque présentes positives≤i64::MAX / Present counters positive≤i64::MAX. |
| `tenant_ceiling` | Tenant : `{"status":"not_applicable"}`. Session : `{"status":"absent"}`, `{"status":"inactive","revision":…, "epoch":…}` ou/or `{"status":"active","revision":…, "epoch":…, "digest":"<lowercase64hex>"}`. Compteurs présents positifs≤i64::MAX / Present counters positive≤i64::MAX. |
| `session_resource` | **Chaîne/string**, jamais objet/never object. Tenant : `"not_applicable"`. Session : `"active"`, `"inactive"` ou/or `"absent"`, issu du grant d’identité courant / from the current identity grant, never runtime status. |
| `observed_at` | Horloge du contrôle dans la même transaction, UTC RFC3339 canonique en millisecondes / Control time from the same transaction, canonical UTC RFC3339 milliseconds. |

Exemple structurel uniquement, sans preuve d’installation ni valeur à réutiliser / Structural example only, not installation evidence or values to reuse:

```json
{"version":1,"schema_version":15,"scope":{"tenant_id":"example-tenant","control_namespace_uuid":"22222222-2222-4222-8222-222222222222"},"selector":{"kind":"tenant"},"head":{"presence":"absent"},"tenant_ceiling":{"status":"not_applicable"},"session_resource":"not_applicable","observed_at":"2026-11-14T00:00:00.000Z","coverage":"policy_registry_metadata","historical":true}
```

`active`/`revoke_requested` décrivent le registre, sans prouver validité temporelle, autorisation effective ou filtrage. Aucun corps de politique, DNS/destination, credential, owner, binding ou reçu n’est retourné ; seul le plafond actif expose son digest nécessaire au CAS Session. Une absence est réellement lue, jamais déduite d’un refus. La transaction checked `Business` vérifie source/audit/binding/découverte et utilise le replay borné. Elle n’ajoute ni événement/reçu d’audit par lecture, ni mutation de politique ou avancement du clock floor. Après vrai commit et restitution SQLite/epoch, credential/source/binding/génération sont revalidés sans rafraîchir le head. Le travail accepté conserve ses permis après abandon de l’observateur jusqu’à sa fin réelle. Une génération périmée exige reconstruction admission fermée, jamais une réponse partielle utilisable.

Registry states do not establish interval eligibility, effective authorization or filtering. No full policy, DNS/destination, credential, owner, binding or receipt is returned; only an active tenant ceiling exposes its required Session CAS digest. Absence is observed, never inferred from a refusal. The checked Business transaction requires healthy source/audit/binding/discovery and bounded replay, without a per-read audit event/receipt, policy mutation or clock-floor advancement. After actual commit and SQLite/epoch release, final identity/source/generation checks do not refresh the observed head. Accepted work retains permits until completion after observer loss. Stale discovery requires closed-admission rebuilding; no partial usable observation is returned.

Les refus HTTP de la table ci-dessous s’appliquent aussi à l’inspection, avec409 `egress_policy_migration_required` pour format14 ; les conflits scope/CAS concernent les mutations A1 finales. Les classes SDK Transport/InvalidResponse ne sont pas des statuts HTTP. Le SDK d’inspection impose15s/requête et3s/connexion, vérifie les deux en-têtes, bornes, octets canoniques et corrélation du sélecteur, sans proxy/redirect/retry/fallback. Une enveloppe d’erreur héritée peut encore porter `retry_action:"resume_same_operation"` : elle ne crée **aucune opération d’inspection à reprendre**. L’utilisateur peut demander une nouvelle inspection explicite, sans réparation automatique.

The refusal table below also applies to inspection: its format14 refusal is migration_required409; scope/CAS conflicts belong to final A1 mutations. Transport/InvalidResponse are SDK classes, not HTTP statuses. The inspection SDK keeps15s/request,3s/connect, exact headers, size/canonical/selector validation and no proxy/redirect/retry/fallback. An inherited error-envelope resume_same_operation marker creates **no resumable inspection operation**. A new inspection must be explicitly requested; none repairs a retained request.

Le [parcours CLI](setup/identity/OWNED_SOURCES.fr.md#owned-policy-a1-operator) conserve quatre records privés avec `kind:"policy_inspection"`, origine canonique sans slash racine et hashes requête/observation. Ce rapport est historique, non signé et modifiable localement ; il ne fournit ni permission, identité humaine, reçu d’audit ou garantie de fraîcheur. Préparer depuis ce rapport reste local ; le POST A1 final revalide scope/CAS et, seulement pour un remplacement Session, le plafond tenant. La révocation garde ses contrôles d’autorité/scope/CAS/horloge, sans exiger de ressource Session/plafond actif ni d’ancienne politique non expirée. The [CLI guide](setup/identity/OWNED_SOURCES.md#owned-policy-a1-operator) describes the closed private report and local derivation. Its owner-editable hashes confer no authority; final A1 submission rechecks scope/CAS and the tenant ceiling only for Session replacement. Revocation retains authority/scope/CAS/clock checks without requiring an active Session resource/ceiling or an unexpired former policy. `status` remains operation history, not inspection or server-head refresh.

#### Autorité, corps et transaction / Authority, body and transaction

Le transport réservé Rust fournit le marqueur privé méthode/chemin/action ; aucun header ou champ JSON ne le fabrique. Il exige l’entrée technique `runner` du vrai registre retenu, le candidat unique de l’index W0 et le binding actif de la source authentifiée. Un humain nominatif, viewer, workload, rôle SSO ou `HumanSourceAccess` ne devient pas cet acteur. L’acteur de l’événement E1 reste `machine_control` ; le propriétaire humain du binding n’est pas qualifié d’approbateur. Aucun provisioning de credential ou binding n’est ajouté.

Avant de lire le corps, le serveur vérifie la véritable identité/source, le format **15 déjà installé** et sa capacité d’opération. Le format 14 renvoie une migration explicite requise, sans migration automatique. La source absente/divergente, un binding refusé ou une contention peuvent refuser plus tôt. Ces deux POST réservés sont les seuls writers de politique adoptés ; le POST inspect séparé lit seulement les métadonnées. Ni alias public non réservé, legacy, GET/HEAD de politique, autre mutation ou lecture du corps de politique n’obtient de writer. Les refus de routage 404/405 des cibles/méthodes inconnues restent distincts des refus de couverture 503 des routes connues non adoptées ; la santé GET/HEAD est indépendante.

Exiger `Content-Type: application/json` exact, sans doublon. Le corps JSON fermé est borné à **65 536 octets**, par déclaration puis pendant le flux ; ambiguïté de framing ou corps incomplet refuse. L’upload garde 5 s au total et 2 s d’inactivité. L’identité précède ces vérifications de framing et la lecture ; le Content-Type est contrôlé par l’extracteur après buffering, pas annoncé comme un contrôle pré-body.

Les deux corps contiennent `version:1`, `expected_scope`, `operation_id`, `selector`, `expected_revision` et `expected_epoch`. `expected_scope` est obligatoire, non null et fermé : `tenant_id` canonique (1–128 caractères ASCII alphanumériques, `-` ou `_`) et `control_namespace_uuid` UUID canonique minuscule non nul. `operation_id` est aussi un UUID canonique non nul ; une autre orthographe est refusée, pas convertie en nouvelle opération. Le sélecteur existant est `{"kind":"tenant"}` ou `{"kind":"session","session_id":"<UUID canonique non nul>"}`. Les compteurs restent dans les bornes SQLite `i64`, avec incréments vérifiés.

Replace ajoute `policy` et `tenant_ceiling`. Son CAS doit être une paire cohérente issue du vrai head/préfixe contrôlé, avec les règles distinctes de création et de modification du DTO ; aucune valeur d’amorçage par défaut n’est fournie. `policy` porte `not_before_ms`, `expires_at_ms`, `destinations` (0–128 autorités DNS exactes sans doublon, nom ≤ 253 octets, port valide, transport `connect_tcp_opaque`), `max_destinations` (1–128) et `max_resolution_ips` (1–16). La fenêtre doit être valide ; le tableau vide reste deny-all. Révision et digest de politique sont dérivés, jamais des champs fournis dans policy. Le plafond tenant est absent/null pour Tenant et obligatoire pour Session : révision/époque positives et digest lowercase64hex exacts de la politique tenant retenue. Le Store vérifie la Session active et ce plafond ; aucun lancement de Session/worker n’est impliqué. Revoke ne porte ni policy, plafond, ancien reçu ni digest attendu ; son CAS est strictement positif.

`expected_scope` est une **assertion comparée**, jamais un sélecteur de Store/source/tenant ou une preuve d’autorité. Après lecture bornée, une vraie WriteEpoch fraîche et la transaction checked `IdentityAuthority15` comparent tenant du Store authentifié et namespace durable de cette même transaction avant replay/recherche d’opération/CAS/clock, puis avant commit, y compris sur rejeu exact. Un tuple bien formé divergent produit `409 egress_policy_scope_conflict` sans mutation de cette opération, ni SQL voisin, ni invalidation causée par la seule mauvaise assertion. Une vraie génération périmée demeure un autre refus : rollback et reconstruction admission fermée. Réserves, horloge durable, limites, audit, binding, tenant ceiling et `commit_authorized` restent côté contrôle.

Entrée propre sans attente et restauration du timeout SQLite précédent sont conservées. La WriteEpoch est acquise dans le vrai travail après corps/queue, sans epoch/SQLite retenus sur un await HTTP ; le gate global peut refuser même si le Store voisin est disponible. Usage accepté, registre/source et permis opération/IO restent attachés au travail réel après abandon de l’observateur. Après commit et restitution SQLite/epoch, credential, source, binding et génération sont revalidés avant réponse. Perdre cette réponse n’annule pas le commit. W0 ne devient ni génération universelle des writers ni cohérence multiprocessus ; les writers d’identité/workload restent fermés.

The reserved Rust transport supplies the private method/path/action marker; no client header or JSON field can mint it. Admission requires the actual retained technical runner registry entry, unique W0 candidate and live binding to the authenticated source. A named human, viewer, workload, SSO role or HumanSourceAccess is not that actor. E1 records `machine_control`; the binding's human owner is not asserted to be an approver. No credential/binding provisioning is added.

Before polling the body, the server admits actual identity/source, already installed format **15** and operation capacity. Format 14 requires explicit migration, never automatic conversion; source/binding/contended admission may refuse earlier. These two reserved POSTs are the only adopted policy writers; the separate inspect POST reads metadata only. No public unreserved alias, legacy writer, policy GET/HEAD, other mutation or full-policy-content read gains a writer. Ordinary 404/405 routing, known unadopted-route 503 coverage refusals and health GET/HEAD remain separate.

Require exactly one `Content-Type: application/json`. The closed JSON body is bounded to **65,536 bytes**, both declared and streamed; ambiguous/incomplete framing refuses. Upload limits stay 5 s total/2 s idle. Identity precedes framing checks and body polling; Content-Type is checked by the extractor after buffering, not claimed as a pre-body check.

Both requests carry version 1, mandatory non-null closed expected_scope, canonical nonnil operation UUID, the existing Tenant/Session selector and expected revision/epoch. Scope has canonical tenant_id (1–128 ASCII alphanumeric/`-`/`_`) and lowercase canonical nonnil namespace UUID. Alternate UUID spelling is refused rather than becoming a new operation. Counters fit SQLite i64 with checked increments. Replace requires a coherent CAS pair from real controlled head/prefix evidence, with the DTO’s distinct creation/update rules and no default bootstrap value supplied here; it adds policy and the tenant ceiling iff Session. Policy has a valid not-before/expiry window, 0–128 exact DNS authorities (≤ 253 bytes/name, valid port, `connect_tcp_opaque`), max_destinations 1–128 and max_resolution_ips 1–16. Empty remains deny-all; revision/digest are derived. Session replacement requires its active resource and the actual tenant ceiling revision/epoch/lowercase64hex digest. Revoke requires positive CAS and carries no policy, ceiling, prior receipt or expected digest.

expected_scope is a comparison assertion, never Store/source/tenant selection or authority. After the bounded body, a real fresh WriteEpoch and checked IdentityAuthority15 transaction compare authenticated Store tenant and that transaction's durable namespace before replay/operation lookup/CAS/clock and again before commit, including exact retry. Well-formed mismatch returns scope_conflict 409 without this operation's mutation, neighboring SQL or invalidation caused solely by the wrong assertion. Actual stale generation instead rolls back and requires closed-admission rebuilding. Reserves, durable clock, limits, audit/binding/ceiling checks and commit_authorized remain control-owned.

Own-source no-wait entry and prior SQLite timeout restoration remain. The actual job acquires WriteEpoch after body/queue, retaining no epoch/SQLite across HTTP awaits; its shared gate may refuse despite a neighboring Store being free. Usage, registry/source and operation/IO permits follow real work after observer loss. Credential/source/binding/generation revalidation follows actual commit and SQLite/epoch release before response; a lost response does not undo a commit. W0 gains neither universal writer generation nor multiprocess coherence, and identity/workload writers remain closed.

#### Reçus A1, erreurs et reprise / A1 receipts, errors and recovery

Le `200` de replace/revoke retourne seulement les octets canoniques E1 existants, ≤ **4 096 octets**, `Content-Type: application/json`, `Cache-Control: no-store` : `version`, `operation_id`, `action`, `selector`, `revision`, `policy_digest`, `before_epoch`, `after_epoch`, `state`, `event_id`, `event_seq`, `event_hash`, `received_at`. Aucun champ replayed, expected_scope ou autorité n’est ajouté. Le rejeu exact rend le reçu d’origine, sans nouvel événement ni nouvelle décision d’effet ; le même UUID avec un autre contenu est un conflit. Source/binding/clock et scope courants sont encore vérifiés avant ce rejeu.

Le SDK garde un timeout de 15 s/connexion 3 s, sans retry, redirect, proxy ou repli public/legacy. Il valide les octets canoniques, action/opération/sélecteur, event_id égal à l’UUID, séquence positive, hash et timestamp canoniques et époque+1. Replace exige révision+1, Active et digest recalculé depuis l’entrée validée. Revoke garde la révision, exige RevokeRequested et ne valide **que la forme** lowercase64hex du digest : aucune corrélation de valeur, politique distante ou ancien reçu n’est inventé. `revoke_requested` signifie demande de révocation du registre, pas fermeture runtime constatée.

For replace/revoke, a 200 contains only existing canonical E1 receipt bytes (≤4,096), application/json and no-store, with the fields above and no replayed/expected_scope/authority addition. Exact retry returns the original receipt without another event/effect decision; changed content under the same UUID conflicts, with current source/binding/clock/scope checked before replay. The SDK keeps its 15 s request/3 s connect bounds with no retry, redirect, proxy or public/legacy fallback. Correlation covers canonical bytes, action/op/selector, event_id, positive sequence, hashes/time and epoch+1. Replace adds revision+1/Active and recomputed digest equality. Revoke retains revision, requires RevokeRequested and checks digest **shape only**: no value expectation, extra policy read or prior receipt. It does not attest runtime closure.

| Statut / Status | Causes bornées / Bounded causes |
|---|---|
| 400 | Cible/framing/corps/Content-Type invalide : `invalid_control_target`, `control_body_incomplete` ou `egress_policy_invalid_request` selon la couche / Invalid target/framing/body/type or closed DTO, with the corresponding layer code. |
| 401 / 403 | Guards d’authentification/rôle existants / Existing authentication/role guards. |
| 408 / 413 | `control_body_timeout` / `control_body_too_large` ; pas d’exception à la borne / Upload deadline/body limit. |
| 409 | `egress_policy_scope_conflict`, `egress_policy_conflict`, `egress_policy_migration_required` ; pas de réparation CAS/scope / No CAS/scope repair. |
| 429 | `control_operation_capacity_exhausted` : places d’opération du transport / transport-operation slots ; `capacity_exhausted` : admission I/O partagée/tenant de la lane Control / shared/tenant Control-lane I/O admission ; `egress_policy_capacity_exhausted` : capacité E1 Store/audit / E1 Store/audit capacity. Enveloppes distinctes ; le SDK classe tout 429 Capacity / Distinct envelopes; SDK maps every 429 to Capacity. |
| 503 | `egress_policy_busy`, `egress_policy_clock_unavailable`, `egress_policy_unavailable`, ou refus de couverture existant / Busy, clock, source/binding/audit/coverage unavailable. |

Les enveloppes historiques des gates pré-body ne sont pas toutes l’enveloppe E1. Les erreurs E1 fermées portent `error.code`, `outcome:"unresolved"`, `retry_action:"resume_same_operation"`, sans body/credential/chemin ou scope réel recopié. Le SDK ne transforme ni un409 inconnu/malformé ni une réponse non canonique en succès ; Transport/InvalidResponse sont des classes client, sans statut HTTP inventé. Timeout/ACK perdu/erreur après commit peuvent laisser un effet durable : conserver la préparation et reprendre explicitement la **même** opération. Un refus récent n’efface pas l’ambiguïté d’un ancien essai. Ni rollback, ni contrôle réseau, permis de connexion, arrêt de runtime, approbation humaine ou qualification production ne se déduit d’un reçu.

Historical pre-body gate envelopes are not all the E1 envelope. Closed E1 errors carry code/unresolved/resume_same_operation without copying bodies, credentials, paths or actual scope. Unknown/malformed 409 or noncanonical responses cannot become SDK success; Transport/InvalidResponse are client classifications, not invented HTTP statuses. Lost ACK, timeout or post-commit error may leave a durable effect: preserve preparation and explicitly resume the **same** operation. A later refusal does not erase earlier ambiguity. A receipt proves neither rollback nor network enforcement, connection permit, runtime stop, human approval or production qualification.

Guide opérateur / operator guide : [EN](setup/identity/OWNED_SOURCES.md#owned-policy-a1-operator) · [FR](setup/identity/OWNED_SOURCES.fr.md#owned-policy-a1-operator). [Contrat E1](engineering/EGRESS_POLICY_STORE.fr.md) · [E1 contract](engineering/EGRESS_POLICY_STORE.md) · [DTO/validators](../src/control/egress_policy.rs) · [SDK](../src/runner/egress_policy.rs) · [DTO inspection](../src/control/egress_policy/inspection.rs) · [SDK inspection](../src/runner/egress_policy/inspection.rs).

### Supervision centrale — protocole v1

Le tableau ci-dessous décrit le lifecycle legacy. Le profil owned garde ces mutations fermées ; ses routes techniques adoptées sont décrites séparément sous [A0](#owned-control-a0) et [A1](#owned-policy-a1). The table below describes legacy lifecycle behavior. Owned keeps these mutations closed; its adopted technical routes are documented separately under A0 and A1.

Ces routes du gateway exigent les mêmes guards d'identité que les routes historiques, avant l'extraction du chemin et du corps. Elles ne sont pas ajoutées à l'alias `/api` ni à la liste de relais du dashboard. Elles n'appellent aucun moteur : un reçu ou une permission centrale ne prouve pas un effet Docker/Podman.

| Méthode et route sous `/api/v1` | Identité | Corps de requête | Réponse |
|---|---|---|---|
| `GET /control/protocol` | Viewer ou runner | Aucun | `200 ControlProtocolV1` |
| `POST /workers/supervised-reservations` | Runner | `SupervisedAdmissionV1` | `AdmissionResultV1` ; première acceptation201, replay200, refus politique403 ou quota429 |
| `POST /workers/{id}/arms` | Runner | `ArmV1` | `200 ArmResultV1` ; `replayed` explicite |
| `GET /workers/{id}/arms/{arm_id}` | Viewer ou runner | Aucun | `200 ArmResultV1`, `replayed:true` ; absence ou autre parent404 |
| `POST /workers/{id}/armed-start-claims` | Runner | `ArmedStartClaimV1` | `200 MutationResultV1` |
| `POST /workers/{id}/helper-start-claims` | Runner | `HelperStartClaimV1` | `200 HelperClaimResultV1` |
| `POST /workers/{id}/helper-observations` | Runner | `HelperObservationV1` | `200 MutationReceiptV1` |
| `POST /workers/{id}/helper-cleanup` | Runner | `HelperCleanupV1` | `200 MutationReceiptV1` |
| `POST /workers/{id}/export-requests` | Runner | `ExportRequestV1` | `200 MutationReceiptV1` |

Les types et leur validation sont définis dans [src/control/supervision.rs](../src/control/supervision.rs). Les objets JSON refusent les champs inconnus. Les sept POST normalisent les rejets JSON en `400 invalid_worker_request`, sans reproduire le corps ni l'erreur détaillée du parseur. Ils appliquent le limiteur d'écriture du tenant. Aucun champ tenant, credential, chemin de configuration privé ou programme de moteur n'est accepté par ces contrats.

`ControlProtocolV1` ajoute `tenant_id` et `role` de l'identité authentifiée aux seuls faits du Store : `version`, `schema_version`, `control_namespace_uuid`, `protocols`, `active`. Son [schéma JSON](../schemas/supervision-protocol.schema.json) décrit cette réponse de lecture ; les autres contrats restent les types Rust stricts. Une base2 renvoie `active:false` et `protocols:[]`. Le schéma3 peut annoncer `broker_supervision_v1` tout en restant inactif. `active:true` n'atteste ni ordonnanceur, ni horloge native, ni confinement runtime : c'est la capacité transactionnelle de cette origine vivante.

Le validateur Rust et le schéma JSON du candidat A0 décrivent `schema_version` 1–15. Pour 3–15, `control_namespace_uuid` est requis et `protocols` vaut `["broker_supervision_v1"]`, même lorsque `active:false` ; pour 1/2, `active:false` et aucun protocole. Ces contraintes de DTO/JSON ne prouvent ni activation, ni Ready, ni autorisation de lancement humain ; le raccord HTTP du périmètre A0 en 14/15 reste limité au [diagnostic réservé candidat A0](#owned-control-a0), dont la preuve A0 reste liée à `0114013` ; les deux POST A1 candidats sont décrits séparément ci-dessus. Les méthodes Store privées scoped doivent recevoir l’owner, la source retenue et l’epoch réels ; les callers legacy restent fermés aux formats 14/15. [Preuves bornées](engineering/evidence/2026-11-12/human-scoped-ports/README.md).

The candidate A0 Rust validator and JSON Schema describe schema versions 1–15. Versions 3–15 require the namespace and supervision protocol even when inactive; 1/2 remain inactive with no protocol. This representation grants no start authority; the A0 14/15 HTTP scope remains limited to the [candidate reserved A0 diagnostic](#owned-control-a0), with A0 evidence bound to `0114013`; the two candidate A1 POSTs are separate above. Private scoped Store methods require the real retained owner/source and borrowed epoch; legacy callers stay closed on 14/15.

`SupervisedAdmissionV1` contient `version:1`, `protocol:"broker_supervision_v1"`, un `intent_hash` SHA-256 et l'objet `admission:AdmissionV1` complet. Son autorité est un UUID ; les nouveaux presets exigent NoLogs. Le serveur valide forme, taille et absence de masquage nécessaire avant l'évaluation externe. Il recherche d'abord l'opération durable ; un replay n'appelle pas Jev. Une demande fraîche vérifie d'abord la compatibilité du schéma3 à7 et l'activation ; leur refus connu ne déclenche pas d'appel Jev. Les quotas dynamiques et la décision fraîche sont revérifiés dans la transaction du Store : cette prélecture ne remplace ni la réservation atomique ni son refus durable. Les champs et hashes historiques de l'admission, de la résolution fournisseur et de la spécification restent inchangés.

Un `ArmV1` lie worker/tentative/intention, inspection déclarée, empreintes d'autorité/configuration, horloge d'hôte et échéance absolue. Un replay conserve exactement ces octets, l'échéance et le reçu. Les nanosecondes sont des chaînes décimales canoniques vérifiées sans débordement ; le serveur ne prouve pas qu'un caller a réellement lu cette horloge. Pour un helper, `inspection.spec_hash` doit être le hash de la politique helper dérivée côté Rust, pas celui du worker. Le Store exige une importation puis une vérification exacte avant création/claim du worker ; un helper de vérification encore pending, inconnu ou divergent bloque la continuation.

Le chemin `{id}` est comparé au worker réel de l'arm chargé dans le Store de l'identité, notamment pour les helpers ; le `worker_id` d'un reçu JSON n'est pas une preuve de parenté. Une arm d'un autre tenant ou d'un autre parent n'est pas diffusée. Les reçus, permissions et observations sont ensuite vérifiés transactionnellement. La première consommation d'un claim renvoie `emit_once` ; son replay exact renvoie uniquement `already_consumed`. Cela ne signifie pas « effet moteur exactement une fois ».

Après réouverture ou restauration, la maintenance permet de relire les arms/reçus et de rejouer un claim déjà consommé, sans nouvelle permission. Les observations/cleanup conservateurs restent soumis à leurs preuves. Nouvelle admission, arm, claim ou destination d'export renvoie `503 supervision_inactive` avant append lorsque l'origine est inactive ; schéma antérieur non migré renvoie `503 migration_required`. Un JSON ou reçu invalide peut être refusé plus tôt en400 ; une identité réutilisée avec un autre contenu renvoie409. Il n'existe aucune route de fresh Bind d'un ancien Created. Les parcours de fencing, reprise après panne, broker/IPC/gardien et qualification moteur restent ouverts.

Le plan privév3 fige la sélection avant décision. Approbation et reprise réutilisent ses octets privés sans nouvelle résolution ni rotation implicite ; un binding modifié bloque un premier démarrage. Les plans1/2 ne se convertissent pas en plans3 et un claim consommé ne permet jamais un second start. Ce plan et son dictionnaire de masquage ne sont pas des ressources HTTP publiques. Le build et les sondes sans credential de la nouvelle image bootstrap ont passé sur Docker Desktop/macOS ARM64, puis un [Claude/API batch géré réel](engineering/evidence/2026-10-30/provider-auth/managed-claude/real-run.md) a produit/exporté le fichier exact avec audit valide après restart. Les autres authentifications, TUI et réglages adverses restent non qualifiés. Les anciennes preuves QA ne leur sont pas transférées.

## Adaptateur Jev réel optionnel

Le gateway peut consulter TypeSafe avec `POST https://api.typesafe.ai/v1/systemone`, un modèle explicite tel que `jev-latest` et une question `choice`. L'enveloppe contient `model`, `state` et `questions`; chaque question décrit `type`, `instructions` et `criteria`. Ces formes sont documentées dans [System One](https://docs.typesafe.ai/api/system-one) et [Choice](https://docs.typesafe.ai/primitives/choice).

L'adaptateur Rust envoie la question `execution_policy` avec les choix `allow`, `deny`, `review`. Son état minimal expurgé contient profil, commande, identifiant de projet et portée du workspace copié ; il n'atteste pas les effets transitifs. Il exige `answers.execution_policy.type="choice"`, un choix reconnu, une confidence valide atteignant le seuil configuré et un `model` de réponse présent. Il conserve le modèle retourné et l'usage déclaré. Une erreur, une réponse inconnue ou incertaine bloque le lancement ; une réponse favorable ne lève aucun refus local ni contrôle d'audit.

`jev-latest` est un alias qui peut évoluer ; le modèle retourné reste la provenance de la réponse. Le client livré n'ajoute pas le header `TypeSafe-Danger-Zone-Long-Context`, donc n'active pas Long Context implicitement. Voir [modèles et Long Context](https://docs.typesafe.ai/models). Le seuil configuré ne démontre pas une calibration indépendante d'ai-guard.

La clé Jev est un secret du contrôle, distinct des credentials runner/viewer et absent des workers. Le produit lit un fichier brut désigné par `JEV_API_KEY_FILE` (`--jev-key-file`), pas un fichier dotenv complet. Les appellations `JEV_API_KEY` et `TYPESAFE_API_KEY` peuvent désigner la même clé de service dans la documentation des comptes ; elles ne créent aucun alias de configuration automatique dans ce gateway. Voir [l'activation explicite](operations.md#jev-optionnel).

## Événements

Le format brut de cette section est réservé aux routes techniques. Les routes humaines projettent seulement les champs autorisés de leur DTO et n'exposent ni `seq`, ni hashes de chaîne globale. This raw event format applies to technical routes only; human DTOs do not expose global sequence or chain hashes.

Le schéma d'ingestion est dans `schemas/event.schema.json`. Les champs optionnels sont absents ou `null`. Le stockage ajoute :

| Champ | Sens |
|---|---|
| `seq` | Séquence monotone locale utilisée pour pagination et reprise SSE |
| `received_at` | Horloge UTC du collecteur à l'enregistrement |
| `prev_hash` | Hash du précédent événement de la chaîne |
| `hash` | SHA-256 de la séquence, réception, hash précédent et payload masqué |

`command.requested`, `security.decision`, `runtime.submitted`, `command.started`, `command.completed` et `command.failed` désignent des étapes différentes. Un `tool.result` de provider n'est pas une observation indépendante du noyau ; sa provenance reste `declared`.

`result` peut contenir `exit_code`, `duration_ms`, `timed_out`, `oom_killed`, tailles et indicateurs de troncature. `stdout` et `stderr` n'apparaissent que si la capture a été demandée. Les fichiers portent des empreintes avant/après, jamais une attribution automatique à un sous-agent.

### Ingestion et faits du contrôle / Ingestion and control facts

`POST /api/events` et `/api/v1/events` partagent un budget d'ingestion de **200 écritures par fenêtre d'une seconde et par tenant**. `POST /api/v1/control/events`, décisions et mutations du contrôle utilisent un autre budget de **200 écritures par seconde et par tenant**. La route choisit le budget ; des valeurs `kind`, `source` ou `provenance` imitant le contrôle dans `/events` ne changent pas cette affectation. Ces budgets de débit ne réservent ni espace d'audit durable ni disponibilité sous charge.

`POST /api/events` and `/api/v1/events` share an ingestion budget of **200 writes per one-second window per tenant**. `POST /api/v1/control/events`, decisions and control mutations use a separate **200-write window per tenant**. The route selects the budget; worker-controlled `kind`, `source` or `provenance` values in `/events` cannot promote a request to the control budget. These rate budgets reserve neither durable audit space nor availability under load.

#### Namespaces réservés / Reserved namespaces

On both ordinary ingestion aliases, reserved namespace checks ignore **ASCII case** and match only the namespace root or a descendant separated by `.`. An otherwise valid event such as `Control.relay.x`, `SECURITY.decision` or `Relay_Channel.v1` is refused with `403` before an audit write. Authentication, event validation and rate limits still apply.

Sur les deux alias d'ingestion ordinaire, la comparaison des namespaces réservés ignore la **casse ASCII** et reconnaît uniquement la racine ou un descendant séparé par `.`. Un événement par ailleurs valide tel que `Control.relay.x`, `SECURITY.decision` ou `Relay_Channel.v1` reçoit `403` avant écriture dans l'audit. L'authentification, la validation de l'événement et les limites de débit restent applicables.

| Frontière / Boundary | Namespaces réservés / Reserved namespaces |
|---|---|
| `POST /api/events`, `POST /api/v1/events` | `security.decision`, `control`, `worker`, `usage.allocation`, `relay_channel` |
| Écriture générique interne / Internal generic `Store::append` | `control`, `relay_channel` |

The Store guard is deliberately narrower than the API guard: it does not copy the API's deny-list onto internal security, worker or usage writers. Separately, ordinary HTTP ingestion rejects the four exact D5 kinds listed below, while `Store::append` retains its exact `broker.retirement.resume.requested` guard. These literal checks remain case-sensitive; namespace matching alone ignores ASCII case. The closed kind list on `/api/v1/control/events` remains exact; a similar name grants no control authority.

La garde du Store est volontairement plus étroite que celle de l'API : elle n'applique pas la liste de refus HTTP aux écrivains internes de sécurité, workers ou usage. Séparément, l'ingestion HTTP ordinaire refuse les quatre littéraux D5 exacts ci-dessous, tandis que `Store::append` conserve sa garde exacte `broker.retirement.resume.requested`. Ces comparaisons littérales restent sensibles à la casse ; seule la comparaison des namespaces ignore la casse ASCII. La liste fermée de `/api/v1/control/events` exige toujours un nom exact ; une ressemblance ne confère aucune autorité du contrôle.

This comparison never normalizes `Event.kind`. Ordinary names such as `Fixture.MixedCase`, `ControlPlane.relay.x` and `Security.decision_reported` retain their bytes and case, subject to the existing event contract. The dot boundary leaves `control_extra` and `control-other` outside the `control` namespace. This adds no route, configuration flag or tenant access. It governs admission of new events; historical-reader compatibility is a separate check and performs no implicit history rewrite or repair. [Evidence and open limits](engineering/evidence/2026-11-12/event-kind-namespace/README.md).

Cette comparaison ne normalise jamais `Event.kind`. Les noms ordinaires tels que `Fixture.MixedCase`, `ControlPlane.relay.x` et `Security.decision_reported` conservent leurs octets et leur casse, selon le contrat existant. La frontière `.` laisse `control_extra` et `control-other` hors du namespace `control`. Aucune route, option de configuration ou autorisation de tenant n'est ajoutée. Cette garde porte sur l'admission des nouveaux événements ; la compatibilité du lecteur historique se vérifie séparément, sans réécriture ni réparation implicite de l'historique. [Preuves et limites ouvertes](engineering/evidence/2026-11-12/event-kind-namespace/README.md).

La route `/api/v1/control/events` exige `runner` **avant la lecture du JSON** : viewer → `403`, credential absent/invalide ou workload worker → `401`. Le tenant vient de cette identité, jamais des métadonnées de session. Le corps suit le même schéma `Event` et le succès renvoie le même `StoredEvent` que l'ingestion. Le `kind` doit appartenir exactement à la liste suivante ; tout autre genre donne `403`. Cette écriture descriptive n'accorde aucun droit de lancement et ne remplace pas les mutations typées de décision/worker/usage.

`/api/v1/control/events` requires `runner` **before parsing JSON**: viewers receive `403`; absent, invalid or worker-workload credentials receive `401`. The authenticated identity selects the tenant. It accepts the same `Event` schema and returns the same `StoredEvent` receipt as ingestion. Only the exact kinds below are accepted; other kinds receive `403`. This descriptive write grants no launch authority and does not replace typed decision, worker or usage mutations.

| Groupe / Group | Genres admis / Accepted kinds |
|---|---|
| Commande / Command | `command.requested`, `command.started`, `command.completed`, `command.failed` |
| Runtime | `runtime.submitted`, `runtime.invocation.bound`, `runtime.terminal.attach.authorized`, `runtime.cleanup.failed` |
| Import | `workspace.import.requested`, `workspace.import.created`, `workspace.import.started`, `workspace.import.completed`, `workspace.import.recovered`, `workspace.import.cleanup.failed` |
| Scan | `workspace.scan.requested`, `workspace.scan.created`, `workspace.scan.started`, `workspace.scan.completed` |
| Export | `workspace.export.requested`, `workspace.export.completed` |
| Bilan / Summary | `workspace.diff.completed`, `observer.coverage` |
| Retrait D5 / D5 retirement | `broker.abandon-before-claim.requested`, `broker.retirement.pending`, `broker.retirement.final`, `broker.retirement.resume.requested` |

Les quatre faits D5 sont refusés `403` sur l'ingestion ordinaire, même pour un runner.
Leur nom ne promeut aucun viewer/worker et leur reçu ne prouve pas le retrait.
En schéma5–10, l'ingestion ordinaire préserve les réserves contrôle et identité ;
les écritures de contrôle authentifiées et les mutations typées peuvent utiliser
la part contrôle en préservant l'identité. Événements et octets persistés sont
bornés séparément. Un rejeu exact ne consomme pas une nouvelle admission durable,
même après réouverture ; le débit HTTP reste appliqué. Aucun reset ni migration
de l'historique n'est implicite. Voir les [budgets EN](setup/identity/README.md#audit-budgets-and-stream-capacity)
et [FR](setup/identity/README.fr.md#budgets-daudit-et-capacité-des-flux).

The four D5 facts receive `403` on ordinary ingestion, including for a runner.
Their names promote no viewer/worker and their receipt does not prove retirement.
On schema5–10, ordinary ingestion preserves control and identity reserves;
authenticated control writes and typed mutations may use the control share while
preserving identity capacity. Persisted events and bytes have independent limits.
Exact replay costs no new durable admission, including after reopening; HTTP rate
admission still applies. No history reset or migration is implicit. See the
[EN budgets](setup/identity/README.md#audit-budgets-and-stream-capacity)
and [FR budgets](setup/identity/README.fr.md#budgets-daudit-et-capacité-des-flux).

`file.*` et les déclarations fournisseurs restent dans l'ingestion. Le nouveau CLI refuse un gateway dépourvu de cette route (`404`) à sa première trace `command.requested`, avant lancement ; aucun repli silencieux vers `/events`. Les anciens clients conservent leurs alias d'ingestion. Un même ID avec le même contenu permet le rejeu ; un contenu divergent reste refusé par le Store (`503` actuel pour un conflit d'événement, distinct du `409` des demandes de décision).

`file.*` and provider declarations remain on ingestion. A new CLI refuses an older gateway missing this route (`404`) at its first `command.requested`, before launch, without falling back to `/events`. Existing ingestion aliases remain available to older clients. Exact event replays retain their ID and content; divergent content is refused by the Store (currently `503` for event conflicts, distinct from decision-request `409` responses).

Un refus de **débit d'écriture** porte uniquement cette preuve de reprise : HTTP `429`, `error.code: "write_rate_limited"` et un unique `Retry-After: 1`. Le texte de `message` n'est pas un discriminant. Les écritures d'événements rejouables du CLI conservent exactement leurs octets, ID et timestamp : au plus trois tentatives, 15 s par requête, deux pauses d'une seconde, borne totale de 47 s. L'accusé obligatoire `command.started` utilise **une seule tentative de 15 s**, sans attendre une fenêtre supplémentaire ; son refus reste un échec du callback de démarrage. Les transitions workers ne reçoivent pas ce retry d'événements.

A **write-rate** refusal is retryable only with HTTP `429`, `error.code: "write_rate_limited"` and exactly one `Retry-After: 1` header. Diagnostic text is not a discriminator. Replayable CLI event writes retain their exact bytes, ID and timestamp: at most three attempts, 15 seconds per request and two one-second pauses, within 47 seconds overall. The mandatory `command.started` acknowledgement uses **one 15-second attempt** without a rate retry window; refusal still fails the start callback. Worker transitions do not acquire this event retry policy.

Un `429` générique, un refus de quota, `capacity_exhausted` ou `audit_write_budget_exhausted` ne satisfait pas ce contrat de retry. Les erreurs d'authentification, de transport ou d'audit ne sont pas réessayées par ce mécanisme. Les réserves cumulatives et l'intégrité de l'audit restent indépendantes ; conserver les données lorsqu'elles refusent une écriture.

Generic `429`, quota, `capacity_exhausted` and `audit_write_budget_exhausted` responses do not meet this retry contract. Authentication, transport and audit errors are not retried by this mechanism. Cumulative audit capacity and integrity remain separate constraints; preserve the data when they refuse a write.

### Couverture des fichiers et de la capture / File and capture coverage

Pour le parcours direct `run`/`worker-resume`, le scan fiable et le préfixe de `file.*` précèdent les déclarations du worker. `workspace.diff.completed` donne `changed_files`, `emitted_changes`, `omitted_changes`, `truncated` et `omission_reasons`. Le préfixe est limité à 200 événements et au journal privé de 3 Mio, avec 128 Kio réservés à sa croissance ; des noms fortement échappés peuvent donc réduire ce préfixe avant 200. Le client borne chaque événement à 120 Kio, en dessous du plafond HTTP de 256 Kio. Le bilan ne transforme jamais les changements omis en zéro changement.

For direct `run`/`worker-resume`, trusted scanning and the retained `file.*` prefix precede worker declarations. `workspace.diff.completed` reports `changed_files`, `emitted_changes`, `omitted_changes`, `truncated` and `omission_reasons`. The prefix is bounded by 200 events and the 3 MiB private journal, with 128 KiB reserved for later journal growth; escaped names can shorten it before 200. The client limits each event to 120 KiB, below the 256 KiB HTTP limit. Omitted changes never become zero changes.

Chaque nouveau `file.*` porte `data.change_id`, l'UUID opaque de son événement, ainsi que `path_redacted` et `path_display_ambiguous`. Deux chemins masqués peuvent partager le même affichage sans fusionner leurs changements. L'ambiguïté est calculée sur le préfixe retenu ; elle ne décrit pas les fichiers omis. Aucun nom brut ou hash de nom n'est ajouté pour résoudre cette collision. Les anciens batches à chemins affichés uniques restent lisibles sans ces champs.

New `file.*` events carry `data.change_id`, their opaque event UUID, plus `path_redacted` and `path_display_ambiguous`. Equal masked display paths do not merge distinct changes. Ambiguity is calculated within the retained prefix, not over omitted files. No raw name or name hash is added to resolve it. Older batches with unique display paths remain readable without these fields.

`observer.coverage.data.postprocessing_complete` décrit la livraison des données **retenues et bornées**, pas une couverture exhaustive. `trusted_diff: "complete"` signifie que le bilan du diff est acquitté. `file_delivery` version1 distingue `prepared_changes` du curseur durable `acknowledged_changes` ; les deux restent `null` sans batch préparé. Ses phases sont `not_prepared`, `files_pending`, `summary_pending` et `complete`. Un événement peut être dans SQLite après perte de réponse sans compter comme ACK reçu. `trusted_diff_changed_files` et `trusted_diff_events_truncated` donnent le total connu et sa portée, ou restent `null`. Une livraison bornée peut finir avec `coverage: "partial"`, par exemple si des fichiers ou du stdout sont tronqués.

`observer.coverage.data.postprocessing_complete` describes delivery of the **retained, bounded data**, not exhaustive coverage. `trusted_diff: "complete"` means the diff summary was acknowledged. Version1 `file_delivery` separates `prepared_changes` from the durable `acknowledged_changes` cursor; both remain `null` before preparation. Its phases are `not_prepared`, `files_pending`, `summary_pending` and `complete`. SQLite may contain an event after a lost response without a received acknowledgement. `trusted_diff_changed_files` and `trusted_diff_events_truncated` expose the known total and scope, or stay `null`. Bounded delivery may finish with `coverage: "partial"`, including when file events or stdout were truncated.

Les nouvelles couvertures n'émettent plus `trusted_diff_emitted_changes`. Sa présence dans une ancienne couverture ne prouve pas un compteur d'ACK. Un événement ancien ou déjà retenu est rejoué à l'identique ; une nouvelle révision porte le nouveau bilan, sans réécrire l'ancien.

New coverage events no longer emit `trusted_diff_emitted_changes`. Its presence in older coverage is not proof of an acknowledgement count. Historical or already retained events replay unchanged; a new revision carries the updated summary.

La collecte déclarée distingue ACK connus, événements omis, livraison inconnue et événements non tentés. Après interruption sans capture retenue, son état est `unavailable_after_interruption` et les compteurs non établis restent `null` ; la collecte fournisseur en TTY reste `unavailable_for_tty`. Un échec de post-traitement conserve `postprocessing_error` et un code CLI en erreur. Une couverture refusée reste à rejouer localement : si l'audit central est indisponible, aucun marqueur central n'est garanti. Voir la [reprise EN](setup/local/README.md#reconcile-the-recorded-worker)/[FR](setup/local/README.fr.md#réconcilier-le-worker-enregistré). Les événements historiques et les formats stricts des plans 1/2/3 ne sont pas réécrits.

Declared collection distinguishes known acknowledgements, omitted events, unknown delivery and unattempted events. After interruption without retained capture, its state is `unavailable_after_interruption` and unestablished counters stay `null`; TTY collection remains `unavailable_for_tty`. Postprocessing failure retains `postprocessing_error` and a failing CLI status. A refused coverage event stays locally pending; an unavailable central audit cannot guarantee a central marker. See the linked recovery guides. Historical events and strict plan formats 1/2/3 are not rewritten.

## Recherche, filtres, reprise

Cette section concerne uniquement les lectures techniques `/api/events`, `/api/stream`, `/api/export` et leurs alias versionnés techniques. Elle ne s'applique pas à `/api/v1/human/*`, qui utilise ses DTO filtrés, limites et curseurs opaques décrits plus haut. This section applies only to technical reads and their technical versioned aliases, not human routes.

Paramètres de lecture : `q`, `project_id`, `sandbox_id`, `agent_id`, `kind`, `decision`, `before`, `after`, `from`, `to`, `limit`, `order`. `before` et `after` sont exclusifs ; `from` et `to` s'appliquent au temps de réception et doivent être RFC3339. Ordre par défaut décroissant, `order=asc` pour l'ordre croissant. `limit` est borné entre1 et1000. La recherche texte est une recherche littérale, pas une expression SQL ou regex.

Le flux SSE accepte `Last-Event-ID` ou `after`. Sans curseur, il commence après la tête actuelle. Un client charge d'abord l'historique, conserve son dernier `seq`, puis se connecte avec ce curseur. Au maximum 32 flux par tenant, partagés avec les flux humains ; le navigateur doit reconnecter avec temporisation et dédoublonner par séquence.

L'export accepte les mêmes filtres et `format=jsonl` ou `format=json`. Il fixe une tête de lecture au début afin que les nouvelles écritures n'allongent pas indéfiniment l'export. Une erreur interrompt le flux ; le client doit signaler l'échec et ne pas présenter un export interrompu comme complet.

## Erreurs et limites

Les codes et la rotation ci-dessous décrivent les routes/clés techniques. Le contrat d'erreur typé, l'expiration et la révocation des accès humains sont définis dans leur section dédiée. The following legacy errors/key lifecycle do not replace the typed human error and expiry contract above.

`401` : jeton absent/invalide. `403` : rôle insuffisant, origine refusée ou approbation sans revue/hash valable. `400/422` : paramètres ou JSON invalides. `409` : contenu divergent pour une identité de demande, ou historique ambigu. `413` : corps de plus de256Kio. `429` : admission de travail, débit d'écriture ou nombre de flux dépassé. `503` : audit indisponible, plein pour une nouvelle écriture ou déclaré incohérent. Le dashboard renvoie `502` si son collecteur est inaccessible. Le contenu renvoyé ne doit pas être rendu comme HTML.

Les accès Store des handlers HTTP, lectures, mutations et contrôles d'authentification/audit compris, s'exécutent dans des tâches bloquantes hors des workers Tokio. L'admission n'attend pas de permis : une saturation refuse le nouveau segment avec `429 capacity_exhausted`. L'identité et les autorisations sont revérifiées dans le segment au point d'I/O, après son éventuelle attente dans le pool bloquant. Chaque poll SSE/page d'export repasse ses vérifications ; les permis d'I/O sont libérés avant le transport au client. Annuler une requête ne libère pas le permis d'un segment déjà lancé avant sa fin réelle et n'annule pas un commit durable.

Le lease est lié au Store exact et à la classe de travail. Read couvre profils/pages et source humaine connue, y compris son authentification après résolution et sa revalidation ; si source et contexte diffèrent, les deux leases sont nécessaires, sinon le même permis vivant est réutilisé. Les sondes initiales de résolution/collision utilisent une lane Discovery distincte sur chaque tenant consulté. Un lookup refusé reste indisponible, jamais une absence de binding ; les collisions lisibles restent vérifiées dynamiquement, sans index d'autorité figé au démarrage. Le runner déjà résolu utilise Control pour son propre Store, et Discovery pour les sondes voisines : saturer Discovery sur sa source ne retire pas sa réserve Control.

Leases bind the exact Store and work class. Read covers profiles/pages and a known human source, including authentication after resolution and later revalidation. Distinct source/context Stores need both leases; an identical Store reuses its live permit. Initial resolution/collision probes use a separate Discovery lane on each consulted tenant. A refused lookup remains unavailable, never an absent binding; readable collisions are checked dynamically without a startup authority index. An already-resolved runner uses Control for its own Store and Discovery for neighboring probes, preserving its Control reserve when its source's Discovery lane is saturated.

Les bornes internes actuelles sont définies dans [l'admission Rust](../src/api/store_io.rs) et [le gateway OIDC](../src/api/oidc.rs). Elles sont distinctes des options fournisseur `provider.limits` :

| Travail admis / Admitted work | Borne actuelle / Current bound |
|---|---|
| Lease Read du Store / Store Read lease | 4 par tenant pour profils/pages et source humaine connue / 4 per tenant for profiles/pages and known human source |
| Tâches de consultation partagées / Shared read tasks | 32 par gateway ; les phases initiales/OIDC ont leurs classes ci-dessous / 32 per gateway; initial/OIDC phases use their classes below |
| Fréquence des lectures coûteuses / Expensive read frequency | 200 admissions par fenêtre d'une seconde et par tenant ; segments, pas requêtes HTTP / 200 admissions per one-second window per tenant; segments, not HTTP requests |
| Contrôle runner / Runner control | Voie distincte, 128 segments simultanés partagés / Separate lane, 128 shared concurrent segments |
| Métadonnées d'identité du Store / Store identity metadata | Discovery : 4 sondes simultanées par tenant / Discovery: 4 concurrent probes per tenant |
| Authentification/découverte humaine ou OIDC partagée / Shared human or OIDC authentication/discovery | 16 segments simultanés par gateway ; les profils consultés prennent aussi Read sur leur Store / 16 concurrent segments per gateway; consulted profiles also acquire their Store's Read lease |
| Complétion/nettoyage de refresh / Refresh completion/cleanup | Réserve distincte de 32 obligations par gateway, tâches en queue incluses / Separate reserve of 32 obligations per gateway, including queued tasks |

Ces classes ne sont pas un plafond unique de quatre I/O d'authentification par tenant. Les profils OIDC lus dans une tâche d'authentification ou de complétion prennent leur propre lease Read, en plus de la borne partagée de cette tâche. These classes are not one four-I/O authentication ceiling per tenant. OIDC profiles read inside authentication/completion work acquire their own Read lease in addition to that task's shared bound.

Discovery16 borne le pool initial partagé. Les sondes Discovery voisines effectuées depuis Control, Read ou cleanup conservent la borne globale de leur segment appelant ; elles ne réacquièrent pas ce pool initial. Leur borne Discovery4 par Store reste applicable. Discovery16 bounds the shared initial pool. Neighboring Discovery probes made from Control, Read or cleanup retain their caller's global bound and do not reacquire that initial pool; their per-Store Discovery4 bound still applies.

HTTP Store reads, mutations and authentication/audit checks run in blocking tasks outside Tokio workers. Admission takes no permit waiters; saturation refuses new work with `429 capacity_exhausted`. Identity and authorization are rechecked inside the segment at I/O, after any blocking-pool queueing. SSE polls/export pages repeat their checks and release I/O permits before client transport. Cancelling a request does not release a running segment's slot before it finishes, or undo a durable commit.

Réduisez les lectures/flux concurrents et leur fréquence, puis réessayez explicitement une lecture refusée. Un flux ou un export déjà commencé peut être interrompu avec erreur ; son contenu reste partiel et suit sa procédure de reprise. Un refresh ambigu exige une nouvelle connexion, sans retry automatique. Ces bornes ne garantissent ni disponibilité, ni latence, ni capacité de 100 workers. Elles ne réparent pas un audit plein : `503 audit_unavailable` impose de préserver l'audit et de traiter sa cause, selon les [guides SSO EN](setup/sso/README.md#diagnose-and-recover)/[FR](setup/sso/README.fr.md#diagnostiquer-et-reprendre).

Reduce concurrent reads/streams and their frequency, then retry a refused read explicitly. An already-started stream/export may terminate with an error; treat its content as partial and follow its recovery procedure. An ambiguous refresh requires a new sign-in without automatic retry. These bounds guarantee neither availability, latency nor 100-worker capacity. They do not repair a full audit: preserve it and diagnose `503 audit_unavailable` under the linked SSO recovery guides.

Les clés techniques runner/viewer actuelles n'ont pas d'expiration automatique ; c'est une limite à traiter selon le profil annoncé, particulièrement pour une exposition distante. Leur rotation est explicite sur les fichiers de secrets, suivie du redémarrage des services concernés. Les credentials humains ont leur propre état/expiration et leurs contextes une durée bornée : ne pas leur appliquer cette règle des clés techniques. Ne pas mélanger rotation et suppression de l'audit. Cette limite ne dispense pas des exigences d'identité, de révocation et de transport des [gates de qualification](engineering/PRODUCTION_READINESS.md). Technical runner/viewer keys have no automatic expiry; human credential expiry/revocation and bounded contexts are separate.

## Registre workers humain / Human worker registry

`GET /api/v1/human/workers` requires an admitted local/OIDC human proof and one offered `X-AiGuard-Context` in administration mode with explicit `read_worker_registry`. Active control membership and the OIDC verified-claim ceiling both apply. The right covers infrastructure metadata of that authenticated tenant; `read_metadata`, `read_tenant_metadata`, `read_broker_registry`, an IdP role alone and technical credentials confer no access. Only trusted control/offline authority introduces/reactivates it. La lecture humaine exige les mêmes conditions côté Rust ; aucun tenant appelant ni droit historique n’élargit cette portée.

The closed query accepts only `limit` (default25,1–100) and opaque `cursor`. Schema10 requires explicit backed-up9→10 migration; earlier ordinary readers stay supported without the new capability. The matched product consumers recognize exactly10 after its checked capability genesis. No migration, default or replay grants a right or activates execution. Guides [EN](setup/identity/WORKERS.md)/[FR](setup/identity/WORKERS.fr.md) cover local/OIDC approval, restart and restore. Une sauvegarde9 restaurée reste9 et ne possède pas la nouvelle capacité.

The closed `HumanWorkerRegistryPageV1` contains `version:1`, opaque64hex `context_id`/`authorization_version`, `source:"tenant_worker_broker_registry"`, `consistency:"snapshot_per_page"`, RFC3339 `evaluated_at`, `runtime_inventory:"unknown"`, `registry_status:"available"`, `items`, `has_more`, opaque `next_cursor` or null, and a closed `read_receipt` with request/audit UUIDs and received time. One page uses one checked snapshot; successive ascending worker-ID keyset pages are live and provide neither a frozen traversal nor a global total. La réponse entière est bornée à256Kio ; traversée1000pages/16Mio dans un contexte au plus300s. Les curseurs Workers refusent autre route, preuve, contexte, version d’autorisation ou limite.

Each closed item contains `worker_id`, `request_id`, `attempt_id`, decimal-string `revision`, `state`, `quota_held`, `session_id`, `session_provenance:"declared"`, `runtime_kind`, `cpu_millis`, decimal-string `memory_bytes`, `pids_limit`, `timeout_seconds`, `network`, nullable `observed_started_at` with `observed_started_at_status`, `allocation_inconsistent`, nullable `cleanup_removed`, optional `last_observation`, `launch_broker` and `observation_broker`. The last observation contains only `observed_at`/`observed_at_status`, status, nullable running/start/finish/exit/OOM/restart fields, `started_at_status`/`finished_at_status` and `provenance:"observed"`; exit is a canonical decimal string. Association objects retain the technical federation's closed descriptive association shape, without raw authority identifiers. They may postdate worker facts and never prove which physical broker/incarnation performed launch/inspection. Aucun `WorkerV1` brut, argv/env/commande/contenu, référence/nom de credential, chemin/nom hôte/volume/conteneur ou erreur arbitraire n’est restitué. Un identifiant obligatoire nécessitant masquage bloque la page.

Mandatory `control.identity.workers.access` receipts share durable human F1 tenant/principal budgets, including empty pages. Authorization/revision/ceiling revalidation, projection and receipt commit in one transaction; credentials/mapping/membership/context are rechecked before publication. A receipt may remain when revocation prevents publication and proves no delivery/runtime effect. Audit, redaction or storage failure returns no page. Refus:401 identity;403 permission/context (no implied authority);400 query/cursor;429 durable budget/traversal;503 migration/audit/unavailable. Les refus d’identité/contexte/permission retirent données, détails et curseurs ; les erreurs de transport gardent seulement la dernière lecture autorisée datée/périmée. Polling visible au plus30s, aucun SSE worker implicite ni délai de notification garanti.

The personal worker view is an obligatory open gap until an authenticated initiator/request/delegation/admission chain establishes ownership. `launch.session_id` alone is declared and cannot do so. Cette tranche administrative ne qualifie pas le parcours cloud/personnel complet, les transports brokers distants, les runtimes réels, la capacité100 ou une release ; consommation inconnue et `billable=false` restent inchangés. The technical `/api/v1/federation/workers` DTO/512KiB contract is preserved.

### Compatibilité des valeurs workers / Worker value compatibility

The human worker/request/attempt/session IDs and Workers cursor position accept canonical UUIDs including nil, as the historical control contract does. Broker, credential, context and read-receipt validation is independent. The human DTO does not impose the broker registry's nonnil UUID constraint on worker facts. Les quatre identifiants de faits workers et leur curseur conservent le contrat UUID canonique historique.

Each runtime date has a typed state: item `observed_started_at_status` and observation `observed_at_status`, `started_at_status`, `finished_at_status`, using `known|absent|unknown`. Values are canonical UTC with nanoseconds preserved when known, null for absent/unknown. The mandatory raw `observed_at` uses the original Observe RFC3339 contract without a new epoch bound: malformed raw input remains a storage error. Its human value is nullable, with `observed_at_status` limited to `known|unknown`, never `absent`. `known` also requires a four-digit UTC RFC3339 rendering that roundtrips the same instant and nanoseconds; an otherwise valid source outside that output domain is null/unknown, never clipped. Start values use the historical credible-start interpretation (`timestamp>0`); invalid or unset starts are null/unknown. A valid RFC3339 finish at epoch0 remains known. Known means interpretable supplied value, not chronological consistency or a proven terminal effect. La provenance `observed` reste distincte de ces états de validité ; un intervalle contradictoire ne devient aucune durée calculée ou réussite. Les valeurs invalides brutes ne sont pas diffusées ; les états InDoubt/quota restent ceux du contrôle.

Human `exit_code` is `null` or a canonical signed-i64 decimal **string**, bounded inclusively by `-9223372036854775808` and `9223372036854775807`; plus signs, leading zeroes and negative zero are rejected. Frontend validation uses BigInt and preserves the exact string for display, including `9007199254740993`. Les types et observations techniques WorkerV1/Observe historiques restent inchangés ; cette projection de lecture ne réécrit pas leur stockage.

## Composition humaine workers/brokers / Human federation registry composition

`GET /api/v1/human/federation/workers` adds a closed composition of the existing human worker and broker pages. It requires one admitted local/OIDC human proof and one offered administration context with **both** `read_worker_registry` and `read_broker_registry`, after active membership and verified OIDC claim-ceiling validation. Rust checks both rights before either inventory read and rechecks both with the original context/authorization version before publication. The authenticated context alone selects the tenant. La route historique `/api/v1/human/workers` conserve son contrat et ses associations descriptives sur `read_worker_registry` seul ; cette composition n’élargit ni ne retire ce droit.

The closed query accepts only `worker_limit`, `worker_cursor`, `broker_limit` and `broker_cursor`. Each limit defaults to25 and accepts1–100. Both opaque cursor positions are validated before either inventory receipt is appended. Each cursor binds the identity, context, authorization version, its leg’s limit and the new route/leg domain; existing Workers/Brokers cursors and opposite-leg cursors are rejected. Omitting a cursor rereads that leg’s first page. Every successful request reads and audits both legs, including empty or already consulted pages. Les deux traversées restent indépendantes, chacune bornée à1000pages/16Mio dans le même contexte d’au plus300s ; aucun curseur ni total global ne les fusionne.

The closed v1 envelope contains exactly `version:1`, `source:"tenant_federation_registry"`, `consistency:"independent_snapshots"`, `coverage:"partial"`, `runtime_inventory:"unknown"`, `workers:HumanWorkerRegistryPageV1` and `brokers:HumanBrokerRegistryPageV1`. The two pages preserve their closed contracts, separate `evaluated_at`, `has_more`, `next_cursor` and mandatory `read_receipt`: `workers.consistency` stays `"snapshot_per_page"`, while `brokers.consistency` stays `"live_keyset"`. Each page is at most `HUMAN_MAX_PAGE_BYTES` (256KiB); the serialized aggregate is bounded by `2 * HUMAN_MAX_PAGE_BYTES + 4096` (528384bytes). Chaque page est cohérente dans sa transaction ; les deux pages et leurs suites ne constituent jamais un snapshot global. L’évaluation d’une jambe ne date pas l’autre.

Registered brokers remain visible even when they have no worker row in the current worker page. Unseen/stale/revoked broker metadata and missing observations never establish zero activity, termination or released quota. Historical authority associations may postdate worker facts and identify no proven execution broker/incarnation. The composition adds no worker contents, credentials, runtime access, usage estimate or bill. Une erreur de l’une des lectures refuse toute la composition, sans liste vide ni succès partiel ; un premier reçu déjà durable reste conservé si la deuxième lecture ou la publication est refusée. Both receipts consume the existing shared human F1 tenant/principal budgets and prove reads recorded, not delivery or execution. The existing401/403/400/429/503 human error contracts apply.

The Rust dashboard proxy accepts this exact human API path and preserves authentication/context checks. The administrative **Fédération** tab now presents both registries, with separate pagination, timestamps and receipts, and publishes both pages only after the complete composition validates. A current403 on this endpoint or identity revalidation removes cached pages, cursors and open details without waiting for the error body; the human credential remains available for an explicit access reread and a newly offered context. Other human routes retain their existing403 body handling. Les guides [EN](setup/identity/WORKERS.md#compose-worker-and-broker-pages)/[FR](setup/identity/WORKERS.fr.md#composer-les-pages-workers-et-brokers) décrivent cette vue sans nouvelle migration, rôle ou service. Les [preuves logicielles et navigateur sur fixtures](engineering/evidence/2026-11-07/federated-worker-ui/README.md) couvrent ce périmètre ; les transports brokers distants, l’attribution personnelle, la déconnexion/reprise runtime et la capacité100 restent à qualifier séparément. Source, software tests and fixture browser runs do not qualify a multi-host deployment.
