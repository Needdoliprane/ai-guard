# Raccord privé du relais aux workers — direction proposée le 7 novembre 2026

**Direction et code/tests logiciels locaux approuvés le 7 novembre 2026**, sur
la proposition publiée à `c1748a2`, fondée sur le checkout `f12f266`. Une
campagne réelle du nouveau transport reste hors de cet accord. Aucun canal
runtime décrit ici n'est implémenté ou qualifié. Le choix retenu
est un helper Rust dans le worker, accessible aux applications par un socket
Unix **interne au worker**, et relié au broker de confiance par un flux
stdin/stdout non TTY d'un exec moteur ciblant l'instance exacte. La preuve
workload reste exclusivement dans le contrôle. Le worker conserve
`network:none` ; aucun socket moteur, réseau de contrôle ou credential runner
ne lui est remis.

Cette proposition remplace la comparaison sans choix de ce document. Le
nouveau socket applicatif dans le worker et la capability d'exec restreinte
étendent la frontière de transport autorisée jusqu'ici. La décision à prendre
et sa portée figurent en fin de document ; l'accord reçu ne constitue pas une
preuve d'implémentation.

## Besoin et coutures constatées

Le [client Rust](../setup/relay/README.fr.md) et les
[routes HTTP/SQLite](../setup/workloads/README.fr.md#autoriser-des-messages-dirigés)
existent. Les 31 tests logiciels du lot client utilisent des enregistrements
synthétiques de workers supervisés ; ils ne raccordent pas trois workers
réels. Leur portée reste celle des [preuves datées](evidence/2026-11-07/relay-client/README.md).

Le besoin est de rendre ces trois opérations utilisables depuis A1/A2/B1 sans
ouvrir leur réseau, en gardant identité, grant dirigé, audit obligatoire et
reprise exacte. Fournisseurs, Internet, fédération distante et capacité 100
conservent leurs qualifications distinctes.

| Source du checkout | Couture réellement disponible | Conséquence pour le choix |
|---|---|---|
| [runtime.rs](../../src/runtime.rs), `ContainerSpec::create_args` | Réseau `none`, racine readonly, UID/GID `1000:1000`, logs moteur `none`, `/home/agent` en tmpfs privé, volume `/workspace`, limites CPU/RAM/PID | Le socket peut rester dans le tmpfs Linux existant, sans bind hôte ni nouveau volume. Inspecter ces restrictions sur chaque moteur réel. |
| [runtime.rs](../../src/runtime.rs), `Runtime::command`, `cli`, `capture_cli` | Programme moteur approuvé, environnement reconstruit ; capture bornée, stdin fermé par défaut | Ces méthodes ne constituent pas un transport bidirectionnel permanent. Un port dédié est nécessaire. |
| [managed.rs](../../src/runtime/managed.rs), `RuntimeAuthority`, `ContainerHandle`, `ManagedRuntime::command`, `verify_authority`, `observe` | Autorité figée, invocation retenue, ID complet immuable ; inspection de cette instance | Base du ciblage de l'exec. Aucun ID, contexte moteur, nom ou endpoint fourni par le worker ne doit être accepté. |
| [invocation.rs](../../src/runtime/invocation.rs), `InvocationBindingV1::pin`, `InvocationAnchor::verify` | Identité et contenu du programme moteur vérifiés | Conserver cette vérification, sans recherche PATH ni shell hôte. |
| [managed.rs](../../src/runtime/managed.rs), `prearmed_command` | Profil Docker retenu et revérifié pour le start strict ; `command` seul ne retient pas ce profil | Retenir le profil nécessaire au nouvel exec. Ne détourner aucun permis start consommé et ne modifier aucune dépendance de STOP. |
| [runtime.rs](../../src/runtime.rs), `execute_created`, `execute_created_prearmed` ; [terminal.rs](../../src/runtime/terminal.rs), `TerminalSession::spawn` | `start --attach` sert au programme principal : stdin d'import, captures batch ou PTY fournisseur | Aucun multiplexage du relais dans ces flux. Le PTY fusionne les octets et les captures peuvent tronquer ; aucun ne devient une frame fiable. |
| [local_ipc.rs](../../src/broker/local_ipc.rs), `PrivateListener::bind_new`, `EndpointRecordV1::connect_checked`, `read_frame`, `write_frame` | IPC local avec namespace neuf, inode/UID/epoch contrôlés ; frames de 512 Kio maximum | Motifs réutilisables. Ne pas exposer le protocole contrôle/STOP au worker, ni relever sa limite globale à 2 Mio. Aucune preuve de socket macOS → VM. |
| [client.rs](../../src/relay/client.rs), `WorkerRelayClient::{send,inbox,ack}` | Origine racine fixe, Bearer workload, DTO fermés et réponses vérifiées ; pas de journal ni retry automatique | Réutiliser le client dans le contrôle. Il ne sait actuellement ni employer le préfixe `/control` ni parler à un socket worker. |
| [control_transport.rs](../../src/control_transport.rs), `control_route` ; [gate gateway](../../src/api/control_transport.rs), `gate` ; [ingress](../../src/ingress/control.rs), `forward` | Allowlist `Runner`/`BrokerPresence`, aucune des trois routes worker ; toute query refusée ; réponse ingress limitée à 512 Kio | L'accès publié actuel ne transporte pas l'inbox à 2 Mio. Son extension fermée fait partie du raccord. Ajouter seulement un helper serait incomplet. |
| [workload.rs](../../src/api/workload.rs), `VerifiedWorker`, `Proof::blocking` ; [relay.rs](../../src/api/relay.rs), `send`, `inbox`, `ack` | Preuve authentifiée et revérifiée sous la portée tenant ; Store et audit existants | Garder ce parcours HTTP. Ne construire aucun `WorkerRelayIdentity` depuis une frame et n'appeler aucun Store avec une identité inventée. |
| [kubernetes.rs](../../src/runtime/kubernetes.rs), `KubernetesRuntime::{doctor,observe_owned}` | Inspection/préflight/held CREATE fermé ; aucun port exec/attach ni lanceur worker qualifié | Aucun support du raccord Kubernetes ne découle des adaptateurs Docker/Podman. |

Il n'existe pas de trait générique d'exec worker dans ces sources. Le seul
`BeforeSpawnGuard` existant contrôle un start : il ne devient pas un permis
d'exec. Les ports et DTO nouveaux nommés ci-dessous sont des propositions.

## Choix recommandé et faisabilité par topologie

```text
application Rust dans A1
  → AF_UNIX dans le tmpfs /home/agent du même A1
  → helper Rust non privilégié dans A1
  → stdin/stdout dédiés, sans TTY, d'un exec moteur vers le CID exact
  → propriétaire du canal dans le broker Rust de confiance
  → WorkerRelayClient avec preuve A1 retenue au contrôle
  → ingress fixe /control → gateway → VerifiedWorker → Store du tenant A
```

Le helper est un mode interne d'un binaire Rust livré par le produit, copié
dans l'image worker à un chemin root-owned sur la racine readonly. Le
[Dockerfile worker](../../images/worker.Dockerfile) sait déjà construire et
copier un bootstrap Rust ; il ne livre pas ce helper relais. L'image et le
profil autorisés doivent attester présence et version du helper avant le
lancement du worker ; aucune copie tardive dans le workspace ou le home.

Le broker ouvrirait un exec non TTY avec stdin/stdout en pipes, programme et
arguments de protocole constants, UID/GID `1000:1000`, sans shell, mode
privilégié, secret d'environnement ou argv métier. Le helper partage les
limites du worker ; aucun worker, sidecar ou service réseau supplémentaire
n'est requis. Son stderr porte seulement des diagnostics fermés. Il ne lit ni
sortie fournisseur ni terminal. Les dialectes exacts Docker/Podman de cet exec
restent à implémenter et à vérifier, pas à copier aveuglément entre moteurs.

| Candidat / topologie | Appui source et intérêt | Limite et choix proposé |
|---|---|---|
| UDS direct broker hôte → worker, Linux natif | IPC privé déjà codé ; maintien possible de `network:none` | Exigerait une nouvelle projection/monture et sa garde inter-périmètres, absentes du profil runtime. Non retenu pour le raccord commun ; faisabilité runtime non établie ici. |
| UDS projeté depuis macOS vers Docker Desktop ou VM Podman | Aucun code de transport de socket entre noyaux livré | Un pathname visible ne prouve pas un socket utilisable dans Linux. Non retenu ; aucune traversée UDS macOS → VM ou équivalence Docker/Podman présumée. |
| **UDS intra-worker + exec dédié, Docker Linux x86_64** | `ManagedRuntime` fige moteur et CID ; le flux moteur éviterait la projection UDS hôte | Direction retenue pour une verticale, mais exec bidirectionnel, EOF, ownership et arrêt sont **NOT_RUN**. Aucun port produit ne l'implémente. |
| **Même raccord, Docker Desktop macOS ARM64** | Frontière hôte → moteur via invocation contrôlée ; socket uniquement dans Linux | Hypothèse à prouver par la vraie CLI/VM/image. Le `start --attach` historique ne qualifie pas un exec séparé. **NOT_RUN**. |
| **Même raccord, Podman Linux x86_64** | `ManagedRuntime::command` distingue `--remote=false` et endpoint explicite ; binding stockage/UID/marker vérifié | Adapter et vérifier Podman séparément : stdin, EOF, logs et propriété des processus ne découlent pas de Docker. **NOT_RUN**. |
| **Même raccord, Podman macOS ARM64/VM** | Sélection remote explicite dans le code, sans preuve de cette topologie | Port stdio, autorité distante et lifecycle à établir. Aucune VM déclarée joignable ou qualifiée. **NOT_RUN**. |
| Kubernetes | Observation d'un Pod exact ; pas d'ouverture de ce flux | **BLOCKED** sur lanceur worker effectif, supervision/confinement qualifiés et transport exec/attach restreint à concevoir/revoir. Aucun `kubectl`, cluster ou droit RBAC implicite. |

Réutiliser le PTY, les logs fournisseur, une queue fichier dans le workspace ou
un proxy TCP libre ne fournit pas la séparation requise. Aucune ouverture
réseau générale du worker n'est proposée. Le contrôle des sorties réseau P1 et
la couverture des outils Web/MCP distants restent des chantiers distincts.

## Autorité du canal et garde de la preuve

Le port privé proposé `ManagedRuntime::open_relay_channel` prendrait un
`OwnedContainerSnapshot` retenu et un permis relais propre à l'opération,
jamais un argv ou un CID libre. Ce permis distinct est consommé après
préparation durable et dernière validation de l'autorité. L'ouverture du
transport est un effet audité, pas une nouvelle autorisation de start.

Avant admission, le contrôle recoupe :

1. Tenant/namespace authentifiés du profil de contrôle, session, worker,
   attempt et `generation:1`. Un ID de label/frame ne donne aucune appartenance
   et un binding ne crée pas un worker Running.
2. Binding workload courant : même credential ID, révision, endpoint et
   échéance ; fichier privé hors source/export/workspace. Le helper ne reçoit
   ni preuve ni fichier, ni runner, SSO, credential fournisseur ou `.env`.
3. Autorité/invocation moteur et handle durables : ID complet, session,
   opération, empreinte d'autorité et version d'empreinte existantes. Refaire
   `observe`, comparer `validate_identity_allocation` au snapshot retenu puis
   exiger Running, `restart_count:0`, même `StartedAt` valide et configuration
   autorisée. La méthode actuelle ne compare pas `StartedAt` : ajouter cette
   vérification pour le canal, sans la prétendre déjà livrée.
4. Image immuable contenant le helper, profil non TTY, réseau `none`, montages
   et limites. Aucun binding historique réécrit pour adopter un helper absent
   ou une configuration différente.

Le contrôle retient un enregistrement versionné : `channel_id` UUID, `epoch`
monotone, endpoint, credential ID/révision/référence privée, autorité, handle,
empreinte de configuration, `StartedAt`, expiry et état. Aucun secret brut
n'entre dans ce record. L'epoch de canal est distincte de la génération
workload V1, toujours égale à 1. Un seul canal actif par endpoint et credential
ID ; remplacement explicite après fermeture, jamais réutilisation d'une epoch
ou d'un permis start. L'expiry ne dépasse pas celles du binding et de la
session autorisée.

La V1 conserve le verrou exclusif du propriétaire existant
([`RoleLease`](../../src/broker/discovery.rs), acquis par le service broker).
Réouvrir un canal exige la fermeture et la jonction confirmées de l'ancien
propriétaire et de ses tâches, avec conservation de ce verrou pendant leur
durée de vie. Si cette fin n'est pas confirmée, la réouverture reste refusée.
L'epoch ne protège que le transport local : les routes HTTP ne la vérifient
pas et son changement n'annule aucune requête déjà partie au serveur. Une
ancienne requête dispatchée peut encore committer après fermeture du canal.

Le helper crée un répertoire neuf `0700` et un socket `0600` sous le tmpfs
privé de `/home/agent`, puis un record de découverte versionné. Adapter les
gardes d'inode, de liens et de peer de l'IPC existant au noyau Linux du worker.
Aucun socket/record préexistant adopté, écrasé ou supprimé pour réussir
l'ouverture ; FDs retenus, refus du remplacement, limite de chemin vérifiée,
au plus 128 namespaces conservés par incarnation avant refus explicite.
La découverte ne donne aucune autorisation et aucune ancienne génération
n'est réactivée automatiquement.

Tous les processus d'un worker restent non fiables. UID local et record UDS
ne prouvent ni tenant ni agent : le **flux d'exec exact retenu par le contrôle**
détermine l'endpoint. Un processus de A1 peut demander les trois opérations
comme A1, jamais choisir A2/B1. Aucun initiateur humain, sous-agent ou effet
exécuté n'est authentifié par cette seule demande. Un worker peut perturber son
helper ; le résultat est une indisponibilité visible, sans élévation. Un ancien
helper n'a aucun credential permettant de joindre directement le gateway.

## Dispatch fermé et accès HTTP publié

Le propriétaire du canal appelle seulement les méthodes ci-dessous avec sa
preuve workload retenue et l'endpoint attendu. Extraire des validations pures
communes au client HTTP et au transport local : refuser les secrets connus et
les JSON ambigus **avant toute persistance** dans le contrôle.

| Opération de frame | Appel et destination construite par Rust |
|---|---|
| `send` + `RelaySendV1` | `WorkerRelayClient::send` → `POST /api/v1/relay/messages` |
| `inbox` + `after: Option<i64>, limit: u32` | `WorkerRelayClient::inbox` → `GET /api/v1/relay/messages`, query exclusivement construite depuis ces deux nombres |
| `ack` + `RelayAckV1` | `WorkerRelayClient::ack` → `POST /api/v1/relay/messages/{message_id}/ack`, UUID validé et corrélé |

Aucun champ URL, méthode HTTP, chemin, header, tenant, source, CID ou preuve
n'est accepté. Grants/revoke/prune et routes runner restent hors dispatch.
Grant métier et capability de transport sont distincts ; même tenant et même
socket ne donnent aucune communication implicite.

Pour l'installation publiée actuelle, ajouter un profil HTTP **typé** interne
au client : origine racine approuvée et préfixe constant `/control`, sans
chemin fourni par l'appelant. Le profil public racine reste compatible.
L'ingress conserve destination de contrôle fixe et absence de credentials
persistants ; aucun port/service/réseau worker supplémentaire. Trois deltas
sont nécessaires, au lieu de présenter le raccord comme déjà accessible :

- `ControlRouteIdentity::WorkerRelay` pour les seules trois formes V1 ;
  `VerifiedWorker` avant corps/slots, lane worker distincte et bornée à huit
  opérations par tenant. Le handler existant revérifie encore la preuve sous
  sa portée normale. Aucun changement des slots runner/présence.
- Validation partagée de query seulement pour inbox : clés uniques `after`
  et `limit`, nombres décimaux canoniques dans les bornes actuelles ; aucun
  champ/doublon supplémentaire ni query send/ACK. Reconstruire la query après
  validation dans l'ingress ; conserver le refus historique ailleurs.
- Budget de réponse inbox 2 Mio, reçu 16 Kio et erreur fermée 16 Kio. Ne pas
  relever globalement les 512 Kio de réponse des routes historiques. Conserver
  headers/upload bornés, refus d'upgrade et redirections/proxies/retries
  désactivés ; une perte garde sa classification, sans faux rollback.

Le tronçon contrôle → gateway conserve origine et snapshot de confiance
approuvés : HTTP uniquement sur IP loopback littérale ou HTTPS vérifié selon
le client actuel, bundle de certificats publics explicitement approuvé si
nécessaire. Aucun endpoint d'entreprise, certificat ou accès distant établi
ici ; le [contrat PKI](INFRASTRUCTURE_PKI.md) s'applique aux essais concernés.

## Frames V1, bornes et backpressure proposés

Un codec dédié cadre UDS et pipes, sans modifier le protocole broker/STOP.
Chaque frame : longueur `u32` big endian puis un objet JSON UTF-8 fermé.
Contrôler la longueur avant allocation ; zéro, EOF partiel, UTF-8 invalide,
doublons récursifs, profondeur >64, champs/types/versions inconnus ferment le
canal sans dispatch. Ni saut de ligne, octet de terminal ou log ne délimite une
frame. Les erreurs de décodage n'embarquent pas les bytes fautifs.

| Frame | Champs et corrélation |
|---|---|
| `hello` contrôle → helper ; `ready` helper → contrôle | `version:1`, type, `channel_id`, `epoch`, limites résolues, version helper. Le contrôle a déjà choisi le canal ; l'écho ne prouve pas une identité. Publier l'UDS après handshake complet. |
| `request` application → helper → contrôle | `version:1`, type, `channel_id`, `epoch`, `correlation_id` UUID canonique non nil, `operation:send|inbox|ack`, payload exact. Le contrôle associe l'autorité à son FD, jamais à ces champs. |
| `response` contrôle → helper → application | Même canal/epoch/correlation/opération, `result:ok|error`, exactement un DTO validé ou un code d'erreur fermé. Reçu send/ACK aussi corrélé à `operation_id` et `resource_id`. |
| `close` contrôle → helper | Canal/epoch et raison fermée ; aucun nouveau travail. Ni terminal worker ni ACK métier ; EOF coupe aussi l'admission si cette frame ne peut être livrée. |

`correlation_id` identifie un échange, distinct de `operation_id` send/ACK.
Toute réutilisation dans une epoch est refusée avant second dispatch. Le replay
métier explicite garde le DTO et son UUID d'opération exacts avec une nouvelle
correlation. Refuser une réponse ancienne ou sans requête en cours. Inbox ne
fait ni ACK, exécution ou avancement implicite du curseur.
Conserver au plus 1024 correlations par epoch, lectures comprises ; au plafond,
fermer l'admission et demander une nouvelle epoch explicite. Ce changement ne
réinitialise aucun budget durable du tenant, binding ou grant.

Ces limites sont des **choix de conception proposés**, pas des observations de
performance, des budgets déjà autorisés pour un essai ou une capacité qualifiée :

| Frontière | Borne et comportement proposés |
|---|---|
| DTO métier existant | Send UTF-8 65536 octets, requête encodée ≤262144, TTL 1–3600 s ; inbox 1–32 éléments et ≤2 Mio ; reçu ≤16384 octets. Aucun plafond serveur relevé. |
| Frame, enveloppe comprise | Requête ≤278528 octets ; réponse ≤2113536 ; handshake/erreur/close ≤16384. Contrôler aussi le plafond du DTO, qui ne reçoit pas l'espace réservé à l'enveloppe. |
| Concurrence | Un échange en cours par canal, aucune file de corps ; un canal par endpoint sous quota tenant. Si une seconde frame se présente pendant l'échange, fermer la connexion concernée avant de lire son objet JSON, sans réponse corrélée ni second dispatch. Aucun spool non borné. |
| Buffers du contrôle | Réservation de 3 Mio de bytes encodés par échange avant admission, cumul ≤32 Mio par tenant et ≤128 Mio dans le propriétaire de transport ; aucune admission sans capacité. Borner séparément la mémoire de décodage. Ces plafonds ne sont pas une mesure ou limite globale de RSS ; pics et copies restent à vérifier. |
| Délais | Handshake 5 s ; assemblage/écriture de frame 5 s absolues et 2 s d'inactivité ; opération 45 s absolues incluant revérifications et HTTP. Ne renouveler aucune échéance worker. Timeout après dispatch : issue `unknown`. |
| HTTP/runtime | Garder connexion/lecture/total 3/5/12 s du client ; inspections bornées par leurs délais existants. L'expiration du budget de canal peut interrompre l'attente, sans conclure que l'instance est arrêtée. |
| Backpressure/arrêt | Ne lire aucun nouveau corps sans sortie/capacité disponible. À saturation fermer l'admission, borner à 5 s la fermeture/jonction du transport local ; un échec reste `needs_reconciliation`. STOP/enforcer ne dépend pas de ce drainage. |
| Diagnostics | Métadonnées/codes fermés ; stderr helper drainé sans rétention brute du texte, plafond 16 Kio puis fermeture. Aucun corps, preuve, URL complète ou sortie fournisseur dans erreurs/captures. |

Le contrôleur possède et joint tâches, pipes et child de transport même si le
demandeur disparaît. Le helper ferme listener/clients à EOF de contrôle.
Tuer la CLI d'exec ne prouve pas la mort du helper ou du worker. Un worker
hostile peut conserver un socket inutile : le retrait d'autorité est appliqué
côté contrôle/gateway, pas déduit de son comportement. Aucun arrêt forcé du
worker n'est autorisé par un simple timeout de ce canal.

La frame JSON est monolithique : avant lecture de son corps, sa correlation
est inconnue. Le refus d'admission ou de seconde frame ferme donc la connexion
sans réponse `busy` corrélée. Si une requête précédente a déjà été dispatchée
et que sa réponse est perdue dans cette fermeture, son issue reste `unknown` ;
le refus de lire la nouvelle frame ne prouve aucun rollback de la précédente.

## Durabilité, replay et lifecycle

Avant la première mutation HTTP, conserver le send/ACK exact déjà validé,
identité workload, empreinte et état de dispatch dans un outbox privé du
contrôle. Direction : extension typée/versionnée du journal fichier du broker,
avec les primitives [Directory](../../src/control_fs.rs) et barrières du
[Journal](../../src/broker/journal.rs), sans nouvelle base SQLite ni moteur de
stockage. Définir le format `relay_v1` et sa compatibilité avant code. Ne pas
réutiliser l'outbox d'événements d'audit comme message métier, ni réinterpréter
les checkpoints d'exécution historiques.

Le payload de reprise est une donnée métier privée, `0600` sous parent `0700`,
exclue des copies/export, distincte de l'audit. La preuve brute reste dans son
fichier privé référencé, jamais dans l'outbox. Les validations refusent avant
écriture les secrets connus/JSON ambigus ; aucune DLP exhaustive promise.
L'audit indépendant conserve IDs, empreintes, tailles, décision et résultat,
sans copier le corps. Réserver la capacité requête/résultat/audit avant effet ;
une panne de rétention obligatoire bloque le dispatch.

Plafond initial proposé : une mutation non résolue par endpoint, 1024 identités
d'opérations conservées et 64 Mio encodés par tenant, toutes epochs confondues.
Au plafond, refuser les nouvelles mutations avant effet, conserver les replays
exacts dans leur capacité réservée et demander une maintenance explicite.
Reconnexion/rotation ne réinitialisent rien. Rétention/sauvegarde/restauration
doivent préserver conflits et opérations inconnues : aucune purge automatique
d'un pending ni suppression d'audit pour libérer de l'espace. Coût physique et
durée d'exploitation de ces plafonds restent inconnus avant mesure.

| État | Autorité et reprise |
|---|---|
| Canal `preparing` → `open` | Liaison/intention durables, validations runtime/binding et handshake. CLI créée sans `ready` ne signifie pas canal utilisable. |
| Mutation `prepared/not_dispatched` | Requête durable, soumission non commencée selon son propriétaire. Reprise explicite du même DTO sous autorité courante ; aucune nouvelle identité. Un refus local ne prouve pas le rollback d'un ancien essai. |
| `dispatching` → `confirmed` | Début de dispatch durable avant HTTP ; reçu vérifié puis conservé avant réponse locale. Confirmation du reçu, jamais exécution du destinataire. |
| `dispatching` → `unknown` | Perte, timeout, crash ou réponse invalide : mutation possible. Même DTO/credential ID pour rapprochement ; pas de nouveau send, grant, start, TTL ou délai pour contourner l'inconnue. |
| Tentative courante `refused` | Code fermé auth/permission/conflit/capacité ; corriger la cause au contrôle. Ce refus ne résout pas une opération antérieure `unknown` ; conserver son état et sa capacité réservée sans preuve supplémentaire. `outcome_unknown:false` décrit seulement la tentative courante, pas un rollback antérieur. |
| Canal `closing` → `closed` / `needs_reconciliation` | Arrêt, révocation, expiry, audit indisponible ou identité/configuration/autorité inconnue ferment l'admission. Anciennes frames/epochs refusées localement. Aucun nouveau canal avant fermeture et jonction confirmées de l'ancien propriétaire sous le verrou exclusif ; aucune annulation serveur déduite. |

Chaque dispatch revérifie état/epoch, révocation locale et observation runtime
admissible de la même incarnation. L'HTTP revérifie ensuite binding, endpoints,
grant, horloge et audit côté serveur. L'inspection ne rend pas atomiques vie du
processus et requête ; aucun résultat du relais ne prouve activité ou terminal
runtime. Le point d'effet d'une révocation concurrente est la transaction
d'autorisation serveur ; un corps déjà remis ne peut être retiré. Les grants
ou bindings expirés/révoqués bloquent nouveaux échanges et replays selon le
contrat existant.

Si le replay est refusé après expiration ou révocation, ce résultat concerne
la tentative courante : il ne permet pas de savoir si le premier dispatch a
committé. L'opération antérieure reste `unknown`, avec requête et capacité de
reprise réservées, jusqu'à obtention d'une preuve admissible supplémentaire.
Ne pas la convertir en `refused`, libérer sa réservation ou lui donner un
nouvel UUID sur la seule base de ce refus.

L'attribution locale conserve tenant authentifié, endpoint, credential ID,
type et `operation_id`. Cette clé de suivi n'est pas un espace d'idempotence
serveur distinct. Dans [`relay_operations`](../../src/audit/relay.rs), la clé
primaire est **`operation_id` seul dans la base du tenant**, toutes actions
relais et tous credentials confondus. `replay` recherche cet UUID puis compare
action et empreinte ; les empreintes send/ACK incluent le credential ID. Une
collision intertypes ou intercredentials est donc un conflit, pas une nouvelle
opération permise par une autre attribution locale. Un contenu divergent pour
le même UUID est également un conflit.

Rotation du même credential ID : nouvelle révision/proof validée et nouvelle
admission de canal ; un autre credential ID ne reprend pas le replay du
précédent. Un reçu conservé reste historique, jamais une nouvelle livraison
autorisée après révocation.

Inbox retient curseur/identité et métadonnées, pas de cache durable de corps au
broker. Après perte, une nouvelle lecture explicite peut être réauditée et
différer de la page précédente ; aucun snapshot promis. Déconnexion signifie
vue partielle/périmée, pas inbox vide. Après restart contrôle, canaux fermés et
dispatchs incomplets inconnus. La réouverture d'un canal reste soumise au verrou
exclusif et à la fin confirmée de l'ancien propriétaire ; elle n'annule pas
ses requêtes HTTP déjà dispatchées. La réouverture de la base en maintenance
inactive ne réactive ni supervision, binding, canal ou worker.

Publier provenance et dernière réussite dans le contrôle autorisé. Requêtes du
worker `declared`, réception/reçu Store/inspection runtime restent des faits
distincts. Ni EOF, révocation ou helper terminé ne libèrent le quota ni ne
créent une observation terminale. Durée et usage inconnus sans preuve adéquate ;
ledger d'allocation toujours `billable=false`, sans prix ni CPU consommé inventé.

## Verticale Rust et validations après décision

Ces lots ont des acceptations distinctes. Les nouvelles coutures/tests nommés
sont des travaux futurs, **pas des commandes déjà disponibles**.

| Lot / fichiers pressentis | Acceptation et tests futurs pertinents |
|---|---|
| 1. Codec/client UDS sous `src/relay/`, mode helper Rust et `images/worker.Dockerfile` | Vrais pipes/sockets locaux : fragmentation, taille avant allocation, doublons/profondeur, versions, corrélation/epoch, substitution et saturation. App peut envoyer/lire/ACK sans Bearer ; aucun processus du worker présenté comme agent authentifié distinct. Pas de moteur/modèle requis. |
| 2. Publication dans `src/control_transport.rs`, `src/ingress/control.rs`, `src/api/control_transport.rs`, `src/api/workload.rs`, profil interne de `src/relay/client.rs` | HTTP/Axum/SQLite A/B réels : identité avant corps/slots, rôles refusés, query fermée, inbox >512 Kio jusqu'à 2 Mio, reçus/audit indisponible et aucune route runner depuis frame. Choisir régressions `worker_relay_api`, cas client et tests control/ingress touchés. |
| 3. Port dans `src/runtime.rs`, `src/runtime/managed.rs`, propriétaire broker et journal relay versionné | Moteur témoin Rust : argv constants/full CID, environnement/FDs/profil, refus autorité/image/CID/StartedAt changés, zéro exec avant permis, child possédé, STOP indépendant, aucune réouverture avant fermeture/jonction confirmées sous verrou. Vrais fichiers : crash préparation/dispatch/reçu, replay exact/divergent, collisions intertypes/credentials, refus de replay conservant l'ancien unknown/réservé, seconde frame fermée sans réponse corrélée, saturation et panne d'audit. Canaris `runtime_managed`, `runtime_program`, `runtime_prearmed` selon delta ; aucune qualification moteur déduite. |
| 4. Docker Desktop/macOS ARM64, puis Docker Linux x86_64 et deux topologies Podman séparées | D'abord un worker réel sans fournisseur : inspecter réseau/image/montages/UID/limites/logs, prouver UDS interne + octets exec dédiés et fermeture. Au premier oracle défaillant, conserver phase/preuves et corriger hors ligne avant nouvel essai. Aucun fallback réseau/socket hôte/provider. Autres combinaisons NOT_RUN. |
| 5. A1/A2 chez A, B1 chez B ; guides EN/FR et procédures synchronisés | Recette ci-dessous avec audit persistant recoupé au dashboard autorisé, arrêt/reprise et états partiels. Revue indépendante sécurité/audit/lancement avant intégration. [Gates](PRODUCTION_READINESS.md) et [recette trois workers](LOCAL_CLOUD_TESTS.md) restent ouverts hors preuve exécutée. |

La recette emploie vrais volumes/IDs distincts et deux bases tenant séparées ;
aucune fixture Running ne remplace une inspection. Elle doit couvrir :

- A1→A2 autorisé ; refus sans grant, sens inverse sans grant, A1→B1, mauvaise
  tentative/epoch/credential. Une frame A1 ne sélectionne jamais la preuve
  A2/B1, y compris après reconnexion.
- Send/page/ACK explicites ; perte après commit send/ACK puis replay sans second
  effet ; conflit divergent, expiration/révocation bloquant aussi les messages
  en attente. ACK distinct d'une action exécutée par A2.
- Audit indisponible bloquant mutation/livraison, outbox pleine, flux tronqué,
  réponse lente/saturée et client disparu. Dernières observations/inconnues
  conservées, action de reprise visible, aucune fausse inbox vide.
- Mort helper/CLI d'exec/worker/broker, restart moteur et reprise froide : mêmes
  CID/StartedAt/claims recoupés, aucun nouveau start implicite, ancien canal
  actif ou transfert de preuve. Enforcer préservé ; retrait d'autorité, arrêt
  demandé et terminal inspecté distingués.
- Inspection et effets confirmant réseau `none`, absence de monture moteur,
  projet original et credentials runner/SSO/fournisseur/preuve workload dans
  worker/frames/logs ; pas d'accès au contrôle général ou aux autres tenants.
  Vues A/B autorisées, données séparées, quota tenu sur inconnue, allocation
  terminale unique non facturable.

Avant campagne réelle, fixer commandes existantes, binaires/hashes, versions
moteur/OS/architecture, image/digest, mode rootless/rootful choisi, ressources
et plafonds CPU/RAM/disque/délai du harnais. Ces budgets d'essai ne sont pas
établis ici. Exécution ciblée par Luna selon la politique du mainteneur ; arrêt
au premier oracle discriminant, plafond ou refus. Conserver erreurs/ressources
possédées, sans relance fournisseur/E2E réflexe ni destruction de volume
existant. Charge 100, Kubernetes et fédération multi-hôtes gardent leurs
protocoles et autorisations propres.

## Décision reçue et portée

**Approuvée le 7 novembre pour code/configuration et tests logiciels locaux :** direction commune
Docker/Podman « helper Rust dans l'image worker, UDS uniquement dans son tmpfs
Linux, exec stdin/stdout non TTY vers son CID exact, preuve workload retenue au
contrôle », avec permis relais distinct, outbox de reprise versionné et
extension fermée des trois routes worker sur l'accès `/control` existant.
Conserver moteur dans le contrôle, `network:none`, audit obligatoire et aucune
nouvelle permission d'exécution métier. L'accord antérieur broker/TTY ne couvre
pas implicitement cette capability supplémentaire.

L'accord reçu couvre l'implémentation Rust, ses configurations et les tests
logiciels locaux dans les harnais isolés déjà autorisés. Il ne couvre pas une
campagne du nouveau transport sur des workers réels, un build/lancement d'image
pour cette campagne, un fournisseur, un accès réseau externe, le cloud, une PKI
ou une installation OS. Les opérations de sandbox QA déjà autorisées conservent
leur propre périmètre ; elles ne deviennent pas des essais du transport livré.
Toute campagne réelle conserve ses prérequis, son périmètre et son budget explicites.

Cet accord ne qualifie ni UDS hôte/VM, Kubernetes exec/RBAC, accès
distant/PKI, installation OS ou fonctionnement runtime. Une impossibilité
constatée revient en décision d'architecture, sans fallback TCP ni ouverture
du réseau worker.

Le lot de conception publié à `c1748a2` se limitait à la lecture source et
à la conception documentaire ; aucune commande
Cargo, binaire projet, moteur, réseau, fournisseur ou cluster exécuté. Les
checks statiques du document ne valent pas test du protocole. G05/G06/G08/G09/
G11 et recette réelle restent ouverts ; aucun gate clos.
