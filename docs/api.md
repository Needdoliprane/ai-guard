# Contrat API locale — v0.1.0

Les routes historiques du tableau ci-dessous sont également exposées sous le préfixe versionné `/api/v1` ; `/api` reste leur alias compatible. Les nouvelles [routes de supervision](#supervision-centrale--protocole-v1) et la [route des faits du contrôle](#ingestion-et-faits-du-contrôle--ingestion-and-control-facts) existent uniquement sous `/api/v1`. `GET /api/v1/identity` (ou `/api/identity`) renvoie `{tenant_id,role}` depuis le jeton authentifié. Le gateway peut charger plusieurs identités et bases avec `--tenants-file` ; le [contrat multi-client](engineering/CLOUD_TENANCY.md) documente la configuration, la liaison durable et les limites. Les identifiants envoyés dans les requêtes ne choisissent jamais le tenant.

Gateway par défaut : `http://127.0.0.1:8787`. Le dashboard utilise les mêmes routes de lecture sous `http://127.0.0.1:4000`. La gateway et le dashboard ne fournissent aucune route d'exécution de commande HTTP : seul le CLI de confiance pilote Docker/Podman.

## Identités

Toutes les routes `/api/` exigent `Authorization: Bearer <secret>`. Sur les routes techniques, le secret viewer donne accès aux lectures et le secret runner aux écritures/lectures gateway. Le dashboard vérifie le rôle `viewer` avant chaque lecture technique relayée, même si son fichier de clé a été mal configuré. Par défaut, il accepte seulement le viewer configuré ; `dashboard --shared-tenants` accepte les viewers des tenants configurés par le gateway, chacun dans son périmètre. Cette option exclut `--viewer-token-file`. Les routes humaines `/api/v1/human/` ont leur authentification nominative séparée décrite ci-dessous, sans conversion en viewer. Aucun paramètre d'URL ne transporte un jeton.

Sur une route technique protégée reconnue, l'identité est extraite avant `Path`, `Query` et `Json` : Bearer absent/invalide →401, viewer sur mutation →403, même si son corps ou ses paramètres sont malformés. Ces refus n'imposent pas de lecture/désérialisation applicative du corps. Les erreurs d'une entrée authentifiée invalide restent celles de son contrat. Routage, transport et protections Origin/URI restent en amont : cette règle ne prétend pas empêcher toute réception d'octets réseau. [Preuves](engineering/evidence/2026-10-30/api-auth/README.md) et [ordre des extracteurs Axum](https://docs.rs/axum/latest/axum/extract/index.html#the-order-of-extractors).

Gateway et proxy dashboard refusent les champs Authorization multiples, y compris identiques. La syntaxe exacte `Bearer ` est conservée. **Les routes techniques** refusent un secret vide, contenant un espace blanc ou dépassant4096octets. Les constructeurs de bibliothèque appliquent cette même borne aux credentials techniques configurés : erreur pour `for_tenants`, état fail-closed503 pour `new`. Les lecteurs de fichiers gardent leurs contraintes existantes plus strictes côté gateway. Les empreintes techniques restent dans un index privé de configuration, sans copie complète du dictionnaire de secrets à chaque extraction. Les collisions entre classes d'identité restent recherchées dynamiquement dans les stores ; l'index ne remplace pas cette vérification. Cela n'est pas une preuve de résistance générale au DoS ou de lookup constant-time.

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

## Kubernetes held-CREATE HTTP v1

Les six routes suivantes du gateway exigent une preuve Bearer **broker**
courante et le schéma 9 ; elles n'ont aucun alias `/api`. Le tenant est résolu
par l'authentification, puis identité/grant sont revérifiés dans le segment
Store borné. Aucun header, query ou champ de corps ne sélectionne tenant,
politique ou quota ; les queries sont refusées. Ces routes contrôlent des
transactions SQLite, sans credentials Kubernetes ni appel cluster au gateway.

These six gateway routes require a current **broker** bearer proof and schema
9, with no `/api` alias. Authentication selects the tenant; the bounded Store
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
`reserve|claim|observe_claim|result|cancel`. Ce booléen indique une configuration,
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

Le client HTTP broker/ACK live, le Coordinator 9 et l'activation cluster
restent à raccorder et qualifier. Ces routes ne livrent ni workspace, ni
session Kubernetes, ni permission d'activation. Broker HTTP/live ACK,
schema-9 Coordinator, activation and real runtime qualification remain open.
Voir les guides [EN](setup/kubernetes/README.md) / [FR](setup/kubernetes/README.fr.md).

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
| `events/{event_id}` | `view` | Événement autorisé ou404 uniforme / Authorized event or uniform404. |
| `export` | Mêmes paramètres que events / Same as events | Page autorisée, droit export supplémentaire / Authorized page requiring additional export rights. |
| `stream` | Mêmes filtres et curseur opaque / Same filters and opaque cursor | SSE `human_audit`, page JSON, reprise opaque dans `id` / JSON pages with opaque resumption IDs. |
| `brokers` | Seulement `limit`, `cursor` / Only `limit`, `cursor` | Projection d'infrastructure du tenant, administration `read_broker_registry`, schéma7 / Tenant infrastructure projection, administration `read_broker_registry`, schema7; contrat distinct ci-dessous / separate contract below. |

`view` vaut `metadata|content` et `resource_kind` vaut `session|sandbox|agent|project`. Une page de ressources/événements/export contient `version:1`, `context_id`, `authorization_version` opaque64hex, `items`, `next_cursor`, `has_more`, `coverage:"authorized_selection"` et `read_receipt`. Les pages événements ajoutent `view`, l'export ajoute `selection_complete`. Les révisions d'objets autorisés sont des chaînes décimales ; la version d'autorisation est opaque et ne publie pas le compteur global. Aucun total ni position globale n'est déduit de la pagination.

Resource kinds and views are closed enums. Resource/event/export pages contain the fields above; event pages add their view and export adds `selection_complete`. Revisions of authorized objects are decimal strings, while `authorization_version` is an opaque64hex value, never a global revision counter. Metadata excludes commands, result/data and free-text decision reasons. Content requires content permission; export additionally requires export permission. The typed DTOs are in [identity/access.rs](../src/identity/access.rs).

Les limites sont1–100 éléments et256KiB pour l'enveloppe complète ; export serveur16MiB/1000pages/300s par branche. Pour l'export, les clients CLI/Web appliquent aussi25pages/4MiB/60s. Le registre est limité à4096 contextes,64 par credential/principal, durée absolue au plus300s et bornée par l'expiration de la preuve, sans éviction vivante. Les curseurs AEAD conservent précédents/retries pendant leur contexte, sans corps mis en cache ; ils restent liés à la sélection, à la limite, à l'identité et aux droits. Un retry peut produire un autre curseur et un nouveau reçu d'audit. Ces budgets ne sont pas un quota contre les relectures.

Limits apply to the complete envelope, not just items. For export, CLI/Web clients additionally cap traversal at25pages/4MiB/60s; those client export limits do not apply to broker registry pages. Contexts and cursor positions expire or are revoked; restarting the gateway invalidates them. SSE advances its resumption ID only after a complete validated frame. Retries may duplicate events; possession proves neither receipt nor download. Each returned batch requires durable nominative audit before restitution. Revocation does not retract bytes already emitted. A stream error is terminal; an empty poll invents no business receipt.

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

### Supervision centrale — protocole v1

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
