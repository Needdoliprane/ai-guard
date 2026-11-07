# Transport HTTP de contrôle — raccord serveur du lot H

Statut au 7 novembre 2026 : **serveur et relais Rust implémentés dans le lot H,
avec validations logicielles ciblées ; raccord des consommateurs et composition
encore ouverts**. Ce document décrit le code du worktree H fondé sur
`982d215`, pas une qualification de release. Les gels, résultats et limites
de QA restent attribués dans [VALIDATION.md](VALIDATION.md) et le
[suivi de l'audit](AUDIT_FOLLOWUP.md#audit-final-run-1).

Sources du raccord : [admission gateway](../../src/api/control_transport.rs),
[relais](../../src/ingress/control.rs) et [options CLI](../../src/main.rs).

La [direction approuvée](INGRESS_CONTROL_CAPACITY_PROPOSAL.md) réserve la
capacité des opérations de contrôle après authentification. Les SDK, le runner
et les clients broker ne sont pas encore raccordés dans ce lot serveur. Leurs
appels actuels ne bénéficient donc pas automatiquement de cette séparation.
L'ingestion du lot I et les coutures I/L doivent être composées et vérifiées
avant de conclure sur le parcours complet H. Aucun gate global n'est fermé ici.

La reproduction initiale H01 employait un témoin **PASS = défaut présent** :
un Bearer factice et un compteur amont établissaient le refus du POST avant le
gateway sous pression des pools publics. Ce résultat historique ne prouve ni
mutation autorisée ni contournement d'authentification. Il reste distinct des
régressions positives sur vraies identités et reçus SQLite ; il n'est pas
transformé rétroactivement en PASS de correction ou livré comme test positif.

## Chemin de transport et autorité

L'origine publiée reste inchangée. Un appel de contrôle choisit explicitement
le chemin `/control/api/v1/…`. Par exemple, le relais reçoit
`/control/api/v1/workers/{id}/transitions`, retire uniquement `/control` et
transmet `/api/v1/workers/{id}/transitions` à son origine interne fixe de contrôle.
Il n'utilise ni les 16 places d'upload ni les 64 places d'échange de l'ingestion
publique. Le préfixe sélectionne le transport, jamais un tenant ou un droit.

Le relais transmet les en-têtes avant d'attendre un corps complet, puis relaie
le corps borné. Il conserve le filtrage des en-têtes hop-by-hop et retire Host
et les indications forwarded fournies par le client : la destination vient de
sa configuration fixe. Authorization transite sans interprétation de sa valeur.
Le relais ne possède, ne persiste et ne journalise aucun credential propre.
Il ne suit pas les redirections, ne réessaie pas l'appel et ne se rabat pas sur
l'ingestion publique.

Le gateway exécute l'extracteur Rust existant de la route **avant lecture du
corps et avant acquisition d'un slot d'opération**. `RunnerIdentity` est exigé
pour le contrôle runner ; `VerifiedBroker` n'est admis que pour la présence liée
à son grant courant. Ni anonyme, viewer ni worker n'acquièrent ces places. Le
gateway attend un corps complet dans ses bornes avant de dispatcher le handler.
Les handlers conservent leurs contrôles, transactions, refus d'audit et
revérifications d'identité ; aucune preuve d'authentification n'est mise en cache
pour éviter une révocation pendant l'upload. Un broker ne devient jamais runner.

`gateway_router(state)` expose aussi le nest `/control` pour les accès directs.
`gateway_control_router(state)` expose la racine privée utilisée par le listener
dédié. Le builder de base ne contient pas ce nest : il n'y a pas de récursion.
Des clones du même `GatewayState` partagent la même capacité par tenant.

## Liste fermée des routes

Les chemins suivants sont ceux de la **racine interne**, après retrait du
préfixe de transport. `{id}` et `{arm_id}` désignent un composant de chemin ;
les handlers gardent la validation de leur contrat après authentification.

| Identité | Méthode | Chemins |
|---|---|---|
| Runner | GET | `/api/v1/identity`, `/api/v1/workers/{id}`, `/api/v1/workers/operations/{id}` |
| Runner | POST | `/api/v1/decide`, `/api/v1/workers/reservations`, `/api/v1/workers/{id}/transitions` |
| Runner | GET | `/api/v1/control/protocol`, `/api/v1/workers/{id}/arms/{arm_id}` |
| Runner | POST | `/api/v1/workers/supervised-reservations`, `/api/v1/workers/{id}/arms`, `/api/v1/workers/{id}/armed-start-claims`, `/api/v1/workers/{id}/helper-start-claims`, `/api/v1/workers/{id}/helper-observations`, `/api/v1/workers/{id}/helper-cleanup`, `/api/v1/workers/{id}/export-requests` |
| Runner | POST | `/api/v1/workload-bindings`, `/api/v1/workload-bindings/{id}/revoke`, `/api/v1/brokers/grants`, `/api/v1/brokers/{id}/revoke`, `/api/v1/relay/grants`, `/api/v1/relay/grants/{id}/revoke`, `/api/v1/relay/messages/{id}/prune` |
| BrokerPresence | GET | `/api/v1/brokers/identity`, `/api/v1/brokers/self` |
| BrokerPresence | POST | `/api/v1/brokers/heartbeat` |
| Runner, couture I à composer | POST | `/api/v1/control/events` : classé par la table de transport ; son handler du lot I n'est pas présent dans la base H examinée |

Les deux premières lignes conservent aussi leurs alias historiques `/api/…`.
Les autres lignes sont uniquement versionnées `/api/v1/…`. La table commune
est dans [control_transport.rs](../../src/control_transport.rs).

Le chemin est classé sans décodage ou normalisation permissive : encodages `%`,
backslashes, segments vides, `.` et `..` sont refusés. Une cible absolue, une
autorité dans la cible ou une querystring ne constitue pas un appel de contrôle
valide. Un voisin de chemin, une autre méthode ou un préfixe répété ne donne
aucun accès générique à l'API de base.

`/api/events` et `/api/v1/events`, SSE, OIDC, API humaines, échanges worker et
routes Kubernetes restent hors de ce canal. Leurs accès publics existants et
leurs propres autorisations restent distincts. Une capture déclarée n'est pas
promue au contrôle selon `Event.kind`, `source` ou une donnée d'agent. Le tag
durable de provenance de l'outbox broker mixte reste un écart de raccord ouvert.

## Budgets et portée

| Surface | Borne livrée dans le code serveur |
|---|---|
| Opérations runner | 16 places par tenant, partagées entre les clones d'état et les deux accès gateway |
| Présence broker | 4 places par tenant, séparées des places runner |
| Corps de contrôle | 262 144 octets ; délai total 5 s et attente sans fragment 2 s, appliqués séparément par le relais et le gateway à leur phase de lecture |
| URI de contrôle | 4 096 octets maximum ; querystring refusée |
| En-têtes au relais de contrôle | 16 Kio pour la requête et pour la réponse |
| Amont du relais | connexion 3 s ; attente de réponse aux en-têtes 15 s |
| Réponse au relais | 512 Kio et 30 s au total ; EOF réel requis avant livraison d'un reçu complet, sans SSE |

Le relais lance son délai total d'upload à la préparation de son flux, avant un
éventuel premier poll tardif. Le gateway lance les siens après authentification
et admission. Ce ne sont pas une échéance partagée de bout en bout. Ces valeurs
sont fixées dans le binaire ; aucun flag ou réglage dotenv ne les modifie.

Les places gateway couvrent la lecture du corps et le handler ; annulation,
refus de corps et retour du handler les libèrent. Ce sont des budgets mémoire
locaux, pas des réservations durables de worker ou une qualification de capacité
100. Les transactions et leur audit obligatoire restent l'autorité de durabilité.

Le framing est vérifié puis réémis sans ambiguïté. Le parseur HTTP peut déjà
supprimer Content-Length quand Transfer-Encoding prévaut et fusionner des
Content-Length identiques ; le code applicatif ne prétend pas refuser des octets
qui ne lui sont plus présentés. Des longueurs contradictoires ou un chunk tronqué
ne constituent pas un corps applicatif complet. Une réponse amont tronquée ne
devient pas un reçu durable valide.

## Configuration réellement consommée

| Consommateur | Option | Comportement |
|---|---|---|
| `gateway` | `--control-listen <SocketAddr>` facultatif | Ajoute le listener privé à racine contrôle ; absent, seul le nest `/control` du listener habituel existe. Adresse distincte de `--listen`, règles `--container-listen` conservées. Tous les listeners configurés sont liés avant de servir. |
| `ingress` | `--control-upstream <origin>` requis | Origine fixe dédiée, distincte de `--gateway-upstream` ; aucun défaut vers le public. Absence ou origine invalide refuse la configuration. |

Comme les autres origines du relais, l'origine doit être HTTP(S), sans credential,
chemin autre que `/`, querystring ou fragment. La disponibilité de cette
destination n'est pas déduite de sa seule validation syntaxique.

Les six fichiers raccordés sont les Compose de base
[Docker](../../deploy/docker/compose.yaml) et [Podman](../../deploy/podman/compose.yaml),
leurs profils SSO [Docker](../../deploy/docker/compose.sso.yaml) et
[Podman](../../deploy/podman/compose.sso.yaml), et les surcharges TLS du modèle
local [fichiers](../../deploy/docker/compose.jev-local-tls.example.yaml) et
[volumes](../../deploy/docker/compose.jev-local-tls-volumes.example.yaml).
Ils utilisent `--control-listen 0.0.0.0:8788` au gateway et
`--control-upstream http://gateway:8788` à l'ingress. Le port 8788 reste interne,
sans nouveau port hôte. Les réseaux internes, projections de secrets existantes
du gateway et absence de secret propre au relais sont conservés.

Ne pas ajouter `/control` à `--gateway` ou à l'origine du registre broker : ces
champs restent des origines sans chemin. Le consommateur doit choisir le chemin
de contrôle explicitement une fois raccordé. Les plans fermés 1/2/3, origine,
tenant, autorité, bindings et claims consommés ne sont ni réécrits ni étendus.
Cette configuration n'active pas une supervision inactive et ne migre aucune base.

### Couture cliente à réaliser et vérifier

Le contrat retenu pour les consommateurs prévoit `control_url(Method, path)`,
vérifiant la table fermée avant construction d'URL. Identité de supervision,
admission, décision, envoi unitaire d'événement de contrôle et lectures/écritures
typées managées devront choisir `/control` explicitement. `post_event` et le
`get_json` générique d'ingestion/consultation publique, par exemple
`api/integrity`, restent publics. Cette séparation n'est pas encore raccordée
dans le checkout H décrit ici.

Les trois routes fixes de présence devront aussi choisir ce transport en
conservant le même Pending et son identité pendant la réconciliation, sans
nouvel ID ni fallback. L'outbox mixte ne peut pas être envoyée intégralement au
contrôle selon ses seules assertions `Event.kind`/`source` ; sa provenance
persistante reste à traiter.

Seuls les GET de suivi admissibles après ACK pourront reconnaître le DTO de
transport fermé, avec enveloppe unique, version, métadonnées, raison et
cohérence HTTP502/504 validées. Un 503 d'audit, une réponse de protocole ou une
erreur de décodage ne deviennent pas transitoires par leur seul statut. La
fenêtre du moniteur reste celle approuvée au lot L ; elle ne s'applique pas au
callback de démarrage ni aux mutations, starts, claims ou audits obligatoires.

## Refus et résultat inconnu

Après authentification, un budget d'opération plein retourne HTTP429 avec
`error.code=control_operation_capacity_exhausted`,
`phase=control_admission`, `outcome=not_dispatched`. Les erreurs de corps sont
distinctes : `control_body_incomplete`/400, `control_body_timeout`/408,
`control_body_too_large`/413. Authentification, permissions et erreurs d'audit
conservent leurs refus existants ; tout 503 n'est pas une panne transitoire.

Le relais n'émet le DTO suivant que pour une panne de transport classée :

```json
{"error":{"version":1,"code":"control_transport_failure","phase":"exchange","outcome":"unknown","retry_action":"reconcile_same_operation","reason":"upstream_connection_unavailable"}}
```

`reason` est limité à `upstream_connection_unavailable`, `upstream_timeout` et
`upstream_connection_interrupted`. Les champs inconnus du DTO sont refusés ; sa
version et ses champs fixes doivent aussi passer `ControlTransportFailureV1::validate`.
La seule valeur HTTP502/504 ne remplace pas cette validation fermée. Les messages
ne contiennent ni URL, ni credential, ni erreur brute du transport.

Une réponse ou un framing invalide, une erreur TLS non classée comme panne de
transport, une réponse trop grande ou incohérente utilisent la catégorie distincte
`control_transport_protocol_failure`, avec `retry_action=contact_operator`.
Cette catégorie n'est pas une instruction de réessai transitoire.

`outcome=unknown` ne signifie jamais rollback, absence d'effet ou permission de
réessayer un start. Préserver l'identité et le contenu exacts de l'opération et
réconcilier son reçu existant. Aucun repli, aucun réessai automatique de mutation,
de start, de claim ou d'audit obligatoire n'est ajouté par ce transport. Le raccord
du moniteur et sa fenêtre approuvée relèvent du [lot L](GATEWAY_TRANSIENT_FAILURE_PROPOSAL.md),
avec leurs conditions propres ; H n'en élargit pas la portée.

## Qualifications encore ouvertes

Les vérifications HTTP/SQLite locales ne prouvent ni admission TCP sous toute
pression, ni disponibilité globale en pénurie de descripteurs, ni absence de
couplage/coût d'authentification inter-stores. Les limites des lots K/L restent
distinctes. Les transports runtime, Docker/Podman réels, Kubernetes, multi-hôtes,
TLS/PKI d'entreprise, pannes physiques et charge 100 exigent leurs preuves.
Ce lot ne déploie aucune infrastructure cloud et ne lance aucun worker pour
établir ses preuves logicielles de transport et de reçus.
