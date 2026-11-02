# Contrat API locale — v0.1.0

Les routes historiques du tableau ci-dessous sont également exposées sous le préfixe versionné `/api/v1` ; `/api` reste leur alias compatible. Les nouvelles [routes de supervision](#supervision-centrale--protocole-v1) existent uniquement sous `/api/v1`. `GET /api/v1/identity` (ou `/api/identity`) renvoie `{tenant_id,role}` depuis le jeton authentifié. Le gateway peut charger plusieurs identités et bases avec `--tenants-file` ; le [contrat multi-client](engineering/CLOUD_TENANCY.md) documente la configuration, la liaison durable et les limites. Les identifiants envoyés dans les requêtes ne choisissent jamais le tenant.

Gateway par défaut : `http://127.0.0.1:8787`. Le dashboard utilise les mêmes routes de lecture sous `http://127.0.0.1:4000`. La gateway et le dashboard ne fournissent aucune route d'exécution de commande HTTP : seul le CLI de confiance pilote Docker/Podman.

## Identités

Toutes les routes `/api/` exigent `Authorization: Bearer <secret>`. Sur les routes techniques, le secret viewer donne accès aux lectures et le secret runner aux écritures/lectures gateway. Le dashboard vérifie le rôle `viewer` avant chaque lecture technique relayée, même si son fichier de clé a été mal configuré. Par défaut, il accepte seulement le viewer configuré ; `dashboard --shared-tenants` accepte les viewers des tenants configurés par le gateway, chacun dans son périmètre. Cette option exclut `--viewer-token-file`. Les routes humaines `/api/v1/human/` ont leur authentification nominative séparée décrite ci-dessous, sans conversion en viewer. Aucun paramètre d'URL ne transporte un jeton.

Sur une route technique protégée reconnue, l'identité est extraite avant `Path`, `Query` et `Json` : Bearer absent/invalide →401, viewer sur mutation →403, même si son corps ou ses paramètres sont malformés. Ces refus n'imposent pas de lecture/désérialisation applicative du corps. Les erreurs d'une entrée authentifiée invalide restent celles de son contrat. Routage, transport et protections Origin/URI restent en amont : cette règle ne prétend pas empêcher toute réception d'octets réseau. [Preuves](engineering/evidence/2026-10-30/api-auth/README.md) et [ordre des extracteurs Axum](https://docs.rs/axum/latest/axum/extract/index.html#the-order-of-extractors).

Gateway et proxy dashboard refusent les champs Authorization multiples, y compris identiques. La syntaxe exacte `Bearer ` est conservée. **Les routes techniques** refusent un secret vide, contenant un espace blanc ou dépassant4096octets. Les constructeurs de bibliothèque appliquent cette même borne aux credentials techniques configurés : erreur pour `for_tenants`, état fail-closed503 pour `new`. Les lecteurs de fichiers gardent leurs contraintes existantes plus strictes côté gateway. Les empreintes techniques restent dans un index privé de configuration, sans balayage des tenants par requête ni copie complète du dictionnaire de secrets à chaque extraction. Cela n'est pas une preuve de résistance générale au DoS ou de lookup constant-time.

**Les routes humaines et les handlers OIDC** utilisent un parseur Bearer distinct : 1–16 384 octets, sans caractère de contrôle ni espace blanc. Le format local nominatif reste plus étroit ; cette borne ne change ni son format ni celle des clés runner/viewer. La configuration OIDC peut imposer une borne `max_token_bytes` inférieure, appliquée aux jetons fournisseur validés et aux jetons privés du corps de refresh/rotate. Le proxy d'authentification dashboard borne le header Authorization à 16 391 octets, préfixe compris ; le gateway valide ensuite le Bearer. Human/OIDC token bounds do not enlarge technical credentials or authorize an unadmitted JWT.

| Méthode et route | Identité | Résultat |
|---|---|---|
| `GET /healthz` | Aucune | État du composant ; le gateway refuse aussi les configurations invalides et les échecs d'audit déjà constatés |
| `POST /api/events` | Runner | Événement normalisé, expurgé puis stocké ; séquence et hashes |
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

`native-intent.action` vaut `{kind:"login"}`, `{kind:"refresh",authentication_id,generation}` ou `{kind:"finish",intent_id,outcome:"cancelled"|"failed"}`. Login ne requiert aucun Bearer et crée une intention durable valable 300 s avant les appels natifs au fournisseur ; `/admit` la consomme et vérifie le nouveau token signé pour le client CLI configuré. Refresh exige l'ancien Bearer CLI encore valide et marque durablement la génération pending avant l'appel fournisseur natif ; son intention et sa complétion sont bornées à 15 s. Finish consomme une intention abandonnée ; pour refresh, il exige aussi l'ancien Bearer exact et ferme l'admission, tandis qu'une intention login ne requiert aucun Bearer. Le résultat d'audit reste inconnu/interrompu, sans transformer une déclaration CLI en observation de PKCE, nonce ou échange fournisseur par le gateway.

A native refresh begun while valid may complete within its retained 15 s deadline after the old token expires, solely to validate/install a new signed access token of the same principal, issuer and client. `/rotate` consumes that exact intent and old generation/bearer; this exception grants no human read or new refresh under an expired bearer. The final install rechecks the deadline. Refresh pending blocks reads/SSE for the old generation. Successful refresh/rotation replaces it, revalidates memberships/mapping and requires fresh human contexts/cursors. Once refresh is pending, failure, cancellation, expiry or an ambiguous exchange closes local admission; no automatic exchange retry or fallback occurs.

Logout distingue trois résultats : `local_session:"closed"|"unknown"`, `provider_logout:"accepted"|"failed"|"unavailable"|"unsupported"|"not_attempted"` et `audit:"recorded"|"unavailable"`. Seuls référence, génération et Bearer exacts d'une session en mémoire permettent sa fermeture ; un token expiré peut servir à cette seule fermeture connue. Une session inconnue ou une preuve divergente renvoie `unknown/not_attempted/unavailable`, sans appel fournisseur. La fermeture locale précède l'audit/appel IdP ; le client efface aussi son état local avant son appel réseau. Logout natif ne contacte pas l'IdP. `accepted` décrit la réponse de l'endpoint pris en charge, sans promesse de révocation globale des JWT ou des autres sessions.

Authentication intentions, outcomes and token fingerprints are durable in a separate operator-private audit, not tenant business events or a public HTTP audit feed. Audit failure blocks admission/refresh; logout still reports local closure separately if its audit cannot be recorded. Gateway restart loses all private live admissions, transactions and handoffs. A durable `active` record cannot reconstruct a token owner: sign in again with a newly issued access token. Previously admitted access-token fingerprints remain remembered across restart and cannot be re-admitted/rotated as fresh authority. Tokens themselves are not recoverable from that audit; provider exchange and local audit are not one external transaction.

Les erreurs OIDC suivent `{error:{code,message,retry_action}}`, avec texte prédéfini :400 `invalid_request`,401 `credential_invalid`,403 `origin_denied|permission_denied`,409 `session_conflict`,429 `capacity_exhausted`,503 `oidc_not_configured|audit_unavailable|identity_unavailable|provider_unavailable`. La remise corrélée `/tokens` utilise401 pour ses échecs sauf403 `permission_denied` et503 `audit_unavailable` ; elle peut notamment nommer `credential_expired` ou `login_cancelled`. Les refus HTTP en amont gardent leur propre contrat ; ne pas déduire un statut global à partir du seul code corrélé.

## Consultation humaine nominative / Named human reads

Le [guide local](setup/identity/README.fr.md) et son [équivalent anglais](setup/identity/README.md) décrivent provisioning, fichiers privés, migration explicite vers5 et reprise ; les guides SSO ci-dessus couvrent l'autre méthode humaine. La preuve humaine est vérifiée à chaque requête/lot ; elle ne donne aucun rôle runner/worker/viewer. Le contrôle propose les contextes issus des memberships durables. Hors `/identity`, fournir un unique `X-AiGuard-Context` proposé ; aucun tenant libre ne choisit le périmètre. Le dashboard ne relaie que ces GET humains exacts, sans repli technique.

Human credentials are verified on every request/batch and never become runner/worker/viewer authority. The control offers contexts from durable memberships; all routes except identity require one offered `X-AiGuard-Context`. The dashboard forwards only the exact human GET allowlist. No caller-supplied tenant chooses scope and no failed human login falls back to technical reads.

`identity.authentication` contient `method`, `credential_id` opaque et `expires_at` RFC3339 ou `null`. `method:"local_credential"` correspond au principal `human_local` et au credential nominatif durable ; `method:"oidc_access_token"` correspond au principal `human_oidc` vérifié `(issuer,subject)` et à une admission OIDC vivante. Dans ce dernier cas, `credential_id` est la même référence opaque que `authentication_id`, pas le token, et l'expiration est celle de l'admission effective. La génération figure dans les DTO d'authentification OIDC ; `/human/identity` ne la publie pas comme un droit supplémentaire.

Human identity explicitly distinguishes local credentials from admitted OIDC access tokens. A signed token without a live admission is insufficient. OIDC capabilities are the intersection of the configured mapping over verified access claims and active control-side memberships, then context mode and resource permissions. Account names, realm administration and a token's caller-supplied tenant claim confer no authority. The mapping never creates a membership; its revision and capability ceiling remain bound during Store revalidation.

| GET `/api/v1/human/…` | Paramètres / Parameters | Résultat / Result |
|---|---|---|
| `identity` | Aucun / None | Principal, authentification et contextes proposés / Principal, authentication and offered contexts. |
| `resources` | `limit`, `cursor` | Ressources explicitement autorisées, propriétaire et droits distincts / Explicitly authorized resources, owner and separate permissions. |
| `events` | `view`, `resource_kind`, `resource_id`, `limit`, `cursor` | Événements de la sélection autorisée / Events from the authorized selection. |
| `events/{event_id}` | `view` | Événement autorisé ou404 uniforme / Authorized event or uniform404. |
| `export` | Mêmes paramètres que events / Same as events | Page autorisée, droit export supplémentaire / Authorized page requiring additional export rights. |
| `stream` | Mêmes filtres et curseur opaque / Same filters and opaque cursor | SSE `human_audit`, page JSON, reprise opaque dans `id` / JSON pages with opaque resumption IDs. |

`view` vaut `metadata|content` et `resource_kind` vaut `session|sandbox|agent|project`. Une page contient `version:1`, `context_id`, `authorization_version` opaque64hex, `items`, `next_cursor`, `has_more`, `coverage:"authorized_selection"` et `read_receipt`. Les pages événements ajoutent `view`, l'export ajoute `selection_complete`. Les révisions d'objets autorisés sont des chaînes décimales ; la version d'autorisation est opaque et ne publie pas le compteur global. Aucun total ni position globale n'est déduit de la pagination.

Resource kinds and views are closed enums. Pages contain the fields above; event pages add their view and export adds `selection_complete`. Revisions of authorized objects are decimal strings, while `authorization_version` is an opaque64hex value, never a global revision counter. Metadata excludes commands, result/data and free-text decision reasons. Content requires content permission; export additionally requires export permission. The typed DTOs are in [identity/access.rs](../src/identity/access.rs).

Les limites sont1–100 éléments et256KiB pour l'enveloppe complète ; export serveur16MiB/1000pages/300s par branche. Les clients CLI/Web appliquent aussi25pages/4MiB/60s. Le registre est limité à4096 contextes,64 par credential/principal, durée absolue au plus300s et bornée par l'expiration de la preuve, sans éviction vivante. Les curseurs AEAD conservent précédents/retries pendant leur contexte, sans corps mis en cache ; ils restent liés à la sélection, à la limite, à l'identité et aux droits. Un retry peut produire un autre curseur et un nouveau reçu d'audit. Ces budgets ne sont pas un quota contre les relectures.

Limits apply to the complete envelope, not just items. Contexts and cursor positions expire or are revoked; restarting the gateway invalidates them. SSE advances its resumption ID only after a complete validated frame. Retries may duplicate events; possession proves neither receipt nor download. Each returned batch requires durable nominative audit before restitution. Revocation does not retract bytes already emitted. A stream error is terminal; an empty poll invents no business receipt.

Les contextes/curseurs lient aussi méthode d'authentification, empreinte de la preuve, référence et révision/génération, révision du mapping OIDC, membership, organisation, mode et version d'autorisation. Chaque poll SSE revérifie l'admission et ces droits avant restitution : refresh pending, rotation, logout, expiration ou révocation termine l'ancien flux avec erreur. La nouvelle génération impose de demander `/identity` et sélectionner un contexte neuf ; aucun curseur ni export de l'ancienne génération ne se transfère. SSE and context bindings preserve the OIDC capability ceiling; a new generation cannot reuse the previous authorized view.

Les erreurs humaines sont `{error:{code,message,retry_action}}`.401 ferme la session humaine, même après changement local de contexte ;403 de contexte retire ses données/curseurs et exige une nouvelle sélection ; un refus de ressource ne déconnecte pas toute la session.404 ne distingue pas objet absent et inaccessible.400 refuse entrée/curseur,429 indique capacité,503 contrôle/audit indisponible ; `migration_required`503 demande `contact_operator`, sans repli viewer. Les actions sont `authenticate|select_context|review_permissions|reduce_page|retry|contact_operator`.

Human errors use the envelope above.401 closes the human session independently of a previous context change or an unreadable response body. Context revocation/expiry clears that context; an individual resource refusal does not log out the whole session.404 hides absence versus inaccessibility. Capacity and storage/migration failures remain explicit; neither is an empty result or a viewer fallback.

Pour les routes métier `/api/*`, le dashboard ne relaie que les lectures de sa liste explicite, dont les workers, quotas et allocations ; il n'expose pas la consultation des reçus d'opérations du broker ni les mutations. Les opérations d'authentification OIDC suivent leur allowlist distincte ci-dessus. Les lectures paginées workers/usage acceptent uniquement `cursor` (UUID canonique) et `limit` (1 à 100, 50 par défaut). Le curseur est le dernier worker ou la dernière tentative de la page. Un paramètre inconnu est refusé `400` ; le client ne choisit pas le tenant.

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

Le Store Rust du schéma3 de supervision est vérifié séparément et les routes ci-dessous exposent ses transactions réelles. Le schéma par défaut reste2 ; aucune nouvelle migration CLI, route d'activation ou installation de broker n'est ajoutée. Une base v3 rouverte/restaurée est en maintenance : les effets frais sont refusés avec `503 supervision_inactive`. La capacité d'origine neuve est non sérialisée ; le numéro de schéma seul n'autorise aucun start. Réactivation après restart et migration des anciens Created restent des exigences ouvertes, sans repli legacy. [Contrats, preuves et limites du Store](engineering/evidence/2026-10-30/supervision-store/README.md).

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

`SupervisedAdmissionV1` contient `version:1`, `protocol:"broker_supervision_v1"`, un `intent_hash` SHA-256 et l'objet `admission:AdmissionV1` complet. Son autorité est un UUID ; les nouveaux presets exigent NoLogs. Le serveur valide forme, taille et absence de masquage nécessaire avant l'évaluation externe. Il recherche d'abord l'opération durable ; un replay n'appelle pas Jev. Une demande fraîche vérifie d'abord schéma3 et activation ; leur refus connu ne déclenche pas d'appel Jev. Les quotas dynamiques et la décision fraîche sont revérifiés dans la transaction du Store : cette prélecture ne remplace ni la réservation atomique ni son refus durable. Les champs et hashes historiques de l'admission, de la résolution fournisseur et de la spécification restent inchangés.

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

## Recherche, filtres, reprise

Cette section concerne uniquement les lectures techniques `/api/events`, `/api/stream`, `/api/export` et leurs alias versionnés techniques. Elle ne s'applique pas à `/api/v1/human/*`, qui utilise ses DTO filtrés, limites et curseurs opaques décrits plus haut. This section applies only to technical reads and their technical versioned aliases, not human routes.

Paramètres de lecture : `q`, `project_id`, `sandbox_id`, `agent_id`, `kind`, `decision`, `before`, `after`, `from`, `to`, `limit`, `order`. `before` et `after` sont exclusifs ; `from` et `to` s'appliquent au temps de réception et doivent être RFC3339. Ordre par défaut décroissant, `order=asc` pour l'ordre croissant. `limit` est borné entre1 et1000. La recherche texte est une recherche littérale, pas une expression SQL ou regex.

Le flux SSE accepte `Last-Event-ID` ou `after`. Sans curseur, il commence après la tête actuelle. Un client charge d'abord l'historique, conserve son dernier `seq`, puis se connecte avec ce curseur. Au maximum32 flux par gateway ; le navigateur doit reconnecter avec temporisation et dédoublonner par séquence.

L'export accepte les mêmes filtres et `format=jsonl` ou `format=json`. Il fixe une tête de lecture au début afin que les nouvelles écritures n'allongent pas indéfiniment l'export. Une erreur interrompt le flux ; le client doit signaler l'échec et ne pas présenter un export interrompu comme complet.

## Erreurs et limites

Les codes et la rotation ci-dessous décrivent les routes/clés techniques. Le contrat d'erreur typé, l'expiration et la révocation des accès humains sont définis dans leur section dédiée. The following legacy errors/key lifecycle do not replace the typed human error and expiry contract above.

`401` : jeton absent/invalide. `403` : rôle insuffisant, origine refusée ou approbation sans revue/hash valable. `400/422` : paramètres ou JSON invalides. `409` : contenu divergent pour une identité de demande, ou historique ambigu. `413` : corps de plus de256Kio. `429` : débit d'écriture ou nombre de flux dépassé. `503` : audit indisponible, plein pour une nouvelle écriture ou déclaré incohérent. Le dashboard renvoie `502` si son collecteur est inaccessible. Le contenu renvoyé ne doit pas être rendu comme HTML.

Les clés techniques runner/viewer actuelles n'ont pas d'expiration automatique ; c'est une limite à traiter selon le profil annoncé, particulièrement pour une exposition distante. Leur rotation est explicite sur les fichiers de secrets, suivie du redémarrage des services concernés. Les credentials humains ont leur propre état/expiration et leurs contextes une durée bornée : ne pas leur appliquer cette règle des clés techniques. Ne pas mélanger rotation et suppression de l'audit. Cette limite ne dispense pas des exigences d'identité, de révocation et de transport des [gates de qualification](engineering/PRODUCTION_READINESS.md). Technical runner/viewer keys have no automatic expiry; human credential expiry/revocation and bounded contexts are separate.
