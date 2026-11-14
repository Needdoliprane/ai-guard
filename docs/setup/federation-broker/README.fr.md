# Publier un broker distant avec le profil de supervision owned

[English](README.md) · [Configuration du broker](../broker/README.fr.md) · [Présence legacy](../broker/PRESENCE.fr.md) · [Checkpoints legacy](../broker/CHECKPOINTS.fr.md)

> **Statut : proposition, ni admise ni qualifiée.** Ce guide décrit le côté
> broker du profil proposé `broker_supervision_owned_v1`. Il dépend des routes
> du control plane et de la maintenance d'enrôlement du lot de contrôle associé,
> de la couture de service de Root et des décisions de Root sur le writer
> d'enrôlement et l'évolution du protocole. Compilation, tests, runtime,
> multi-hôte et infrastructure : `NOT_RUN`. Les commandes ci-dessous sont des
> **parcours à valider par Root** ; aucune n'a été exécutée pour ce guide. Les
> valeurs entre chevrons sont des emplacements réservés, jamais des identifiants
> ou secrets réels.

Le profil owned permet à un broker de confiance situé sur un autre hôte de
déclarer sa présence au control plane d'un tenant dont le Store utilise déjà le
format des sources humaines owned (exactement 14 ou 15). Le broker publie deux
éléments, chacun avec son journal privé :

- un **heartbeat** : déclaration du broker ; le control plane n'enregistre que
  son propre instant de réception (affirmation `declared`, réception
  `observed`) ;
- des **checkpoints** facultatifs : une liste `declared_partial` d'au plus
  100 têtes de jobs, 32 Kio, avec `has_more`, déjà acquittées par le writer Full
  admis du profil.

Aucun des deux n'est un inventaire runtime, un contrôle de santé de worker, une
observation terminale, une décision de fencing, une libération de quota ou un
fait d'usage. Une source absente ne déclare rien ; rien n'est jamais présenté
comme zéro. Le publisher ne détient aucun credential `runner`, n'envoie aucune
commande aux workers et n'en reçoit aucune.

## 1. Profils et compatibilité exacte

Un fichier de présence choisit exactement un profil par sa clé `profile` de
premier niveau. Il n'existe ni détection par contenu, ni repli, ni migration
entre profils.

| | Présence legacy V1 / V2 | Owned `broker_supervision_owned_v1` |
|---|---|---|
| Sélection | Pas de clé `profile` ; `version` 1 (heartbeat) ou 2 (checkpoints). Un fichier legacy contenant `profile` est refusé. | `"profile": "broker_supervision_owned_v1"` et `"version": 1`, tous deux obligatoires. Toute autre valeur de `profile` est refusée. |
| Source d'identité | UUID et autorité saisis dans la configuration | Remise publique d'enrôlement écrite par la maintenance du contrôle |
| Credential | `broker_token_file` | `bearer_file` (mêmes règles de fichier) |
| Routes de contrôle | `/api/v1/brokers/…` | `/api/v1/owned/brokers/self`, `/heartbeats`, `/checkpoints`, `/checkpoints/self` (la route `identity` existe mais le publisher ne l'utilise pas) |
| Format du Store du tenant | Formats legacy de [PRESENCE.fr.md](../broker/PRESENCE.fr.md) et [CHECKPOINTS.fr.md](../broker/CHECKPOINTS.fr.md) | Exactement 14 ou 15, égal au format enrôlé |
| Journaux | `state_dir`, `checkpoint_state_dir` | `heartbeat_journal`, `checkpoint_journal`, avec une disposition owned |

Aucun format de Store n'est servi par les deux profils. Lorsqu'un tenant est
adopté au format des sources humaines owned, ses publishers de présence legacy
sont refusés par le control plane ; basculez ce profil vers un nouvel
enrôlement owned et de nouveaux journaux owned. Un journal legacy n'est jamais
adopté, converti ou réutilisé, et une configuration owned refusée ne se replie
jamais sur le profil legacy.

## 2. Prérequis

1. Le control plane du tenant exécute une version dans laquelle Root a admis
   les cinq routes broker owned derrière le transport de contrôle authentifié.
   Sans elles, le gateway refuse les chemins owned et le publisher s'arrête
   avec `control_response: gateway_refused` ou `unrecognized`.
2. Le Store du tenant est au format 14 ou 15 via
   l'[adoption des sources humaines](../human-sources-maintenance/README.fr.md).
3. L'opérateur du contrôle a exécuté l'enrôlement broker owned **hors ligne**
   décrit dans le guide de contrôle du lot associé (emplacement proposé
   `docs/setup/federation-control/README.fr.md`) : un grant broker avec une
   incarnation neuve et exactement une autorité, puis un binding broker. Il
   produit deux fichiers privés pour ce broker : la remise publique
   d'enrôlement et le bearer. Transférez-les par un canal déjà approuvé pour
   les secrets de contrôle ; ne collez jamais le bearer dans une ligne de
   commande, un ticket ou un journal.
4. L'hôte du broker exécute le service de premier plan de confiance du
   [guide broker](../broker/README.fr.md) avec un profil Registry2 dont
   `gateway`, `tenant_id`, `control_namespace_uuid` et `runtime` correspondent à
   l'enrôlement.

## 3. Fichiers privés et permissions

Tous les chemins sont absolus et normalisés (sans `.`, `..` ni séparateur
répété). Les fichiers sont des fichiers réguliers privés du compte de service,
mode `0600`, dans des parents privés `0700` ; liens, substitutions et ancêtres
non sûrs sont refusés. Modifier l'un d'eux pendant que le service tourne n'est
pas un rechargement : la revalidation suivante ferme l'admission métier, comme
pour la présence legacy.

| Fichier ou répertoire | Borne et règles |
|---|---|
| Configuration de présence owned (`presence_file` du profil Registry2) | ≤ 16 Kio, clés fermées (section 5). |
| `enrollment_file` | Remise publique `OwnedBrokerEnrollmentV1`, ≤ 8 Kio. Elle ne contient ni bearer ni digest. Ne la modifiez pas : son engagement est épinglé dans les ancres des journaux. |
| `bearer_file` | Une ligne de 32 à 4096 octets ASCII visibles, suivie éventuellement d'un LF. Envoyé uniquement comme credential de transport ; jamais journalisé, loggé ni affiché. |
| `ca_bundle_file` (facultatif) | HTTPS uniquement ; PEM ne contenant que des certificats, ≤ 64 Kio, 1 à 8 certificats, remplaçant les racines intégrées pour ce client. Aucune clé privée d'AC ; TLS n'est jamais désactivé. |
| `heartbeat_journal.state_dir` | Répertoire privé `0700` existant, **vide au premier démarrage** et dédié à cet enrôlement. |
| `checkpoint_journal.state_dir` (facultatif) | Mêmes règles ; distinct du journal heartbeat. |

Les chemins de configuration, d'enrôlement, du bearer et de l'AC sont distincts
deux à deux. Le service refuse aussi, avant de créer le moindre verrou ou
ancre :

- un fichier protégé dans un journal, lexicalement ou physiquement, y compris
  les fichiers des autres profils, le registre et les credentials de contrôle
  du service ;
- deux journaux égaux (y compris par identité physique) ou imbriqués, dans un
  profil ou entre profils du même broker ;
- un journal égal à, contenu dans ou contenant la `root` du broker, une entrée
  `source_roots` ou une entrée `export_roots` d'un profil ;
- deux profils owned enrôlés pour le même broker, la même incarnation ou le
  même credential.

Le bearer, chaque secret littéral du service et les motifs de masquage sont
écartés de la configuration, de l'enrôlement, des chemins, des pins et de chaque
enregistrement de journal avant persistance. Les limites cumulées de protection
du [guide de présence](../broker/PRESENCE.fr.md) (64 fichiers protégés,
64 secrets littéraux) s'appliquent toujours ; le profil owned ajoute le fichier
d'enrôlement à ce décompte.

Créez les répertoires une fois, avant le premier démarrage (parcours à valider
par Root) :

```sh
mkdir -m 0700 /absolute/private/ai-guard/owned-heartbeat-journal
mkdir -m 0700 /absolute/private/ai-guard/owned-checkpoint-journal
chmod 0600 /absolute/private/ai-guard/owned-presence/config.json \
  /absolute/private/ai-guard/owned-presence/enrollment.json \
  /absolute/private/ai-guard/credentials/owned-broker-bearer
```

## 4. Remise d'enrôlement et pins

La remise porte le profil, l'origine canonique du gateway, la révision de
closure à l'émission, le sujet engagé (installation, closure, source, format du
Store, tenant, namespace, broker, incarnation, credential, révision de binding,
révision de grant), l'autorité unique, le lease, les échéances du grant et du
credential, l'instant d'émission et les reçus d'audit des étapes grant et
binding. Elle n'authentifie rien : le gateway authentifie toujours le bearer et
compare le sujet engagé à chaque opération.

Au démarrage, en lecture seule, le publisher valide la remise, en dérive les
pins du journal et les compare à son profil Registry2 :

| Pin | Doit être égal à |
|---|---|
| Origine du gateway | Le `gateway` du profil (HTTPS, ou HTTP vers une IP loopback numérique ; sans chemin, requête ni utilisateur) |
| Tenant, namespace | `tenant_id`, `control_namespace_uuid` du profil |
| Type de runtime de l'autorité | Le `runtime` du profil (`docker` ou `podman`) |
| Lease | Au moins deux fois `interval_seconds` |

L'identifiant d'installation de la remise est celui de l'installation de la
closure de contrôle ; il n'est jamais comparé à l'`installation_id` du registre
du broker. Toute différence refuse le démarrage (`broker presence configuration
is invalid or unsafe`) avant l'ouverture d'un journal.

| Classe de champ | Règle |
|---|---|
| Pins du journal : profil, origine, installation, ID de closure, source, format, tenant, namespace, broker, incarnation, credential, révision de grant, autorité | Un changement exige un nouvel enrôlement, qui s'accompagne d'un nouveau grant et d'une nouvelle incarnation, et de nouveaux journaux vides. Les anciens journaux et leur Pending restent intacts. |
| Révision de binding | Engagée par opération : un Pending retenu n'est rejoué que sous la même révision ; les nouvelles opérations acceptent une révision renouvelée, plus élevée. |
| Révision de closure | Observée : elle peut avancer, jamais régresser sous la valeur enrôlée ou la dernière valeur acquittée. |

## 5. Référence de configuration

Copiez [owned-presence.json](examples/owned-presence.json) dans le répertoire de
configuration privé, remplacez chaque chemin et faites pointer le
`presence_file` du profil Registry2 vers ce fichier :

```json
"presence_file": "/absolute/private/ai-guard/owned-presence/config.json"
```

| Clé | Contrat |
|---|---|
| `profile`, `version` | Exactement `broker_supervision_owned_v1` et `1`. Les clés inconnues sont refusées. |
| `enrollment_file`, `bearer_file` | Fichiers privés obligatoires (section 3). |
| `ca_bundle_file` | Facultatif ; omettre ou `null` pour les racines intégrées. |
| `interval_seconds` | 30 par défaut, plage 5–150. Deux fois l'intervalle ne doit dépasser ni le lease enrôlé ni le lease du broker vu par le control plane (≤ 300 s). |
| `heartbeat_journal` | Obligatoire : `state_dir`, `max_operations` (1–32768), `max_state_bytes` (51200–134217728). Aucune capacité par défaut. |
| `checkpoint_journal` | Facultatif, mêmes clés, `max_state_bytes` 100352–134217728. Présent ⇒ les checkpoints sont publiés ; absent ⇒ heartbeats seuls. |

Un enregistrement est borné par la limite de son contrat plus une enveloppe de
1 Kio : 17 Kio pour les heartbeats et 41 Kio pour les checkpoints. Avant chaque
Pending, le journal réserve la place de son acquittement, et la première limite
atteinte (opérations ou octets) arrête explicitement ce journal. Un heartbeat
est publié une fois par intervalle, et un checkpoint une fois par intervalle
d'échantillonnage tant qu'une source existe.

Le coût d'un journal croît avec son historique et n'est **pas mesuré**
(`NOT_RUN`) :

- Au démarrage, le publisher lit, valide et synchronise une fois chaque
  enregistrement et chaque répertoire d'opération retenus ; le temps de
  démarrage d'un profil croît donc avec son journal.
- Pendant l'exécution, chaque opération vérifie au plus deux fois
  l'historique complet en relisant chaque enregistrement, sans le synchroniser
  à nouveau : une nouvelle opération avant l'écriture de son Pending et avant
  celle de son acquittement, un Pending rejoué avant son envoi et avant son
  acquittement. Seuls l'enregistrement écrit, son répertoire et leurs ancêtres
  sont synchronisés.

L'exemple emploie des valeurs de départ prudentes, choisies pour que la limite
d'opérations soit atteinte avant celle d'octets même avec des enregistrements
maximaux : 2880 opérations heartbeat dans 96 Mio (environ 24 heures à 30 s) et
1440 opérations checkpoint dans 116 Mio (environ 12 heures à 30 s). Avant de
les augmenter, mesurez sur le système de fichiers cible le temps de démarrage
et la durée d'une opération à la taille choisie, et gardez une opération
nettement sous l'intervalle ; aucune mesure n'est encore publiée. Il n'existe
ni archivage, ni purge, ni compaction : prévoyez un nouvel enrôlement et de
nouveaux journaux avant l'épuisement d'un journal.

## 6. Choisir les journaux

- Utilisez des répertoires **neufs et vides** pour chaque enrôlement. Au premier
  démarrage, le publisher crée `.owned-owner.lock` et `owned-anchor.json` ; les
  opérations suivent sous `op-00000000000000000001/{pending.json, ack.json}`.
- L'ancre lie la portée du broker (installation du broker, ID de profil,
  gateway, tenant, namespace, runtime), les pins et l'engagement d'enrôlement,
  le type de journal et l'identité physique du répertoire. Les journaux
  heartbeat et checkpoint d'un profil appartiennent toujours au même
  enrôlement. Renommer le profil Registry2, modifier l'enrôlement ou copier le
  journal ailleurs le rend **étranger**.
- Un répertoire non vide sans le verrou et l'ancre owned (par exemple un
  `state_dir` legacy) ou portant une autre ancre est refusé comme étranger et
  laissé intact. Un nom inattendu ou un enregistrement illisible est refusé
  comme état non sûr.
- Deux écritures interrompues sont complétées au démarrage suivant sans rien
  réécrire ni supprimer, car aucun POST ne peut précéder l'enregistrement
  manquant : un répertoire ne contenant que le `.owned-owner.lock` vide
  (initialisation arrêtée avant l'ancre) reçoit son ancre, et un dernier
  répertoire `op-*` vide (opération arrêtée avant son `pending.json`) est
  réutilisé par l'opération suivante. Un répertoire `op-*` antérieur vide ou un
  enregistrement incomplet reste refusé.
- Le verrou est exclusif : un second processus sur le même journal est refusé.
- Un journal checkpoint n'est utile que si le profil exécute de vrais jobs
  Full ; sans writer Full admis, il ne déclare rien.

## 7. Démarrer et arrêter (proposé)

Démarrez le service de premier plan existant (parcours à valider par Root) :

```sh
"$AIGUARD_BIN" broker serve --config "$AIGUARD_BROKER_CONFIG"
```

Tous les fichiers de présence sont d'abord chargés en lecture seule, puis les
protections cumulées sont vérifiées, puis chaque journal est ouvert. Une erreur
de configuration ou de matériel privé refuse le démarrage. Un refus au niveau
d'un journal (étranger, état non sûr, capacité, verrou détenu) n'écarte que ce
publisher, est journalisé et conserve ses octets ; l'admission métier continue
lorsque le matériel de contrôle est sûr.

`--recovery-only` n'ouvre aucun fichier de présence ni journal. `SIGTERM`/
`SIGINT` arrêtent chaque publisher : une attente réseau est interrompue, le
travail filesystem admis est joint, et le service joint chaque publisher avant
de libérer son rôle local. Un publisher arrêté conserve son Pending pour le
démarrage suivant.

## 8. Diagnostiquer

Le service écrit une ligne JSON par événement sur la sortie d'erreur, sans
secret ni prose distante :

| Événement | Signification |
|---|---|
| `broker_owned_presence` / `publication_started` | Le profil owned a été sélectionné et l'acteur du journal tourne. Cela ne prouve rien sur le control plane, un runtime ou un worker. |
| `broker_owned_presence` / `publication_retrying` | Les essais courts d'une opération sont épuisés (coupure de transport, issue perdue, refus temporaire) ; la même opération est réessayée toutes les `min(interval, 30)` s. |
| `broker_owned_presence` / `publication_waiting_for_operator` | Le control plane exige une action opérateur ; la même opération est réessayée toutes les `max(interval, 60)` s. |
| `broker_owned_presence` / `publication_resumed` | L'opération retenue a été acquittée après l'un de ces états lents. |
| `broker_owned_presence` / `checkpoint_source_unavailable`, `checkpoint_source_available` | Le service est froid, fencé, a un matériel de contrôle incomplet, aucun writer Full admis ou aucune tête acquittée ; rien n'est déclaré entre-temps. |
| `broker_presence` / `publication_stopped` | Le publisher s'est arrêté ; `reason` nomme la cause fermée et `runtime_state` reste `unknown`. |

Les motifs owned ont la forme `owned broker supervision is blocked (<catégorie>: <détail>); <action>` :

| Catégorie | Détails | Action opérateur |
|---|---|---|
| `control_response` | un code `owned_*`, `transport_outcome_unknown`, `gateway_capacity_busy`, `gateway_not_dispatched`, `gateway_refused`, `source_coverage_unavailable`, `audit_budget_exhausted`, `unrecognized` | Voir la section 9. |
| `handshake` | `forbidden` (révoqué, expiré ou lease trop court), `clock_uncertain`, `protocol` ou un nom de pin | Corriger le grant, l'horloge du contrôle ou l'enrôlement ; un nom de pin exige un nouvel enrôlement. |
| `identity` | un nom de pin, `binding_revision`, `binding_regression`, `closure_regression` | Section 11. |
| `acknowledgement` | `commitment`, `receipt`, `snapshot`, `conflict`, `protocol` | La réponse du contrôle n'acquitte pas le Pending retenu ; conserver le journal et rapprocher avec l'opérateur du contrôle. |
| `contract` | un code `owned_*` | L'opération n'a pas pu être préparée selon le contrat commun. |
| `remote_head` | `without_local_history`, `diverged` | Section 13. |
| `journal` | `foreign` | Configurer un répertoire privé vide ; conserver le répertoire étranger. |

Les motifs génériques gardent leur libellé legacy, par exemple `broker presence
evidence requires operator reconciliation` (état de journal non sûr) ou
`broker presence retained capacity is exhausted` (limite de journal).

## 9. Réponses du contrôle et coupures de transport

Seul un code connu accompagné de son statut HTTP exact est reconnu. La prose
distante n'est jamais conservée.

| Réponse | Comportement du publisher |
|---|---|
| Échec de connexion, de TLS ou du flux de corps ; enveloppe relais `control_transport_failure` (502/504) ; `owned_response_withheld` (503) | Issue inconnue : la même opération est rapprochée par auto-lecture, puis renvoyée avec les mêmes octets. |
| `owned_supervision_unavailable` (503) ; refus gateway `not_dispatched` pour capacité (429) ou corps (408/400) | Même opération après backoff. |
| `owned_source_rebuild_required`, `owned_audit_unavailable`, `owned_response_unpublishable`, `owned_credential_ambiguous` (503) ; `identity_unavailable` (503) | Attente de l'opérateur du contrôle à cadence lente, Pending conservé. |
| 401, 403, 400, 409, `owned_migration_required`, budgets d'audit (429), refus gateway de route, méthode ou taille | Arrêt avec le Pending retenu. |
| Tout autre corps, y compris un 503 nommé inconnu, une redirection, un corps non JSON ou trop grand | Arrêt (`unrecognized` ou protocole). Un 503 inconnu n'est jamais traité comme temporaire. |

Les opérations réessayées attendent 100, 250 et 500 ms, puis `min(interval, 30)` s
entre les tentatives, jusqu'à l'acquittement ou un refus d'arrêt ; l'entrée dans
cette cadence lente journalise une fois `publication_retrying`. Une longue
coupure de transport est donc traversée sans redémarrage ni nouvelle opération,
et le publisher ne perd ni ne remplace jamais le Pending non acquitté. Une
erreur de confiance TLS est aussi un échec de transport : elle est réessayée,
vérifiez donc `ca_bundle_file` si l'événement persiste. Le client n'utilise ni
proxy, ni redirection, ni réessai transparent ; HTTP/1 uniquement, délais
3/5/12 s.

## 10. Reprise du Pending et de l'acquittement

Chaque opération est écrite et synchronisée comme `pending.json` **avant** le
POST ; chaque réessai, expiration et redémarrage envoie exactement les octets
durables, le même identifiant d'opération et la même séquence. Un acquittement
n'est accepté que si la réponse est exactement le successeur attendu (ou si une
auto-lecture exacte prouve que cette opération est committée), et il est
synchronisé comme `ack.json` avant toute nouvelle opération ou coalescence de
checkpoint.

| Situation | Ce qui se passe |
|---|---|
| Le contrôle a committé, la réponse est perdue | La tentative suivante lit la vue de soi, reconnaît l'opération committée et enregistre l'ACK (`self_read`). |
| La réponse est arrivée mais l'écriture de l'ACK a échoué | Le publisher s'arrête ; le Pending reste. Au démarrage suivant, l'auto-lecture le reconnaît et l'ACK est enregistré. |
| Arrêt ou coupure de courant avant l'existence d'un enregistrement (verrou sans ancre, dernier répertoire d'opération vide) | Complété au démarrage suivant (section 6) ; rien n'a été envoyé et rien n'est réécrit. |
| Coupure de courant pendant l'écriture d'un enregistrement | Un enregistrement incomplet peut subsister ; le journal refuse alors de s'ouvrir (état non sûr) et est conservé. Aucune réparation automatique n'existe ; la procédure qualifiée reste ouverte. |
| Une action de journal échoue de façon inattendue (panique interne) | Le publisher s'arrête avec `broker presence evidence requires operator reconciliation` ; le Pending reste et le démarrage suivant revérifie tout le journal. |
| Redémarrage avec un Pending | Même sujet exigé (même révision de binding) ; sinon le journal est bloqué. |

Ne modifiez, ne supprimez et ne recréez jamais un Pending, un ACK, le verrou ou
l'ancre.

## 11. Rotation, révocation et évolution de la closure

- **Renouvellement de binding** (même credential, révision de binding plus
  élevée ; produit par `broker-identity rotate` côté contrôle, qui remplace la
  clé du bearer) : accepté pour les nouvelles opérations avec le même
  enrôlement. Remplacez le fichier bearer uniquement service arrêté et
  conservez le fichier d'enrôlement d'origine et les deux journaux ;
  n'installez pas le `enrollment.json` écrit par `rotate` (simple preuve : son
  nouvel engagement rendrait ces journaux étrangers). Ordre (parcours à valider
  par Root) : arrêtez ce publisher pendant que le gateway sert encore ;
  vérifiez en lecture seule que, dans chaque journal owned, le dernier
  répertoire `op-*` contenant un `pending.json` contient aussi un `ack.json`
  (sinon, redémarrez le publisher, gateway en service, jusqu'à son
  acquittement, puis arrêtez-le) ; ensuite seulement, arrêtez le gateway et
  lancez la rotation ; remplacez le bearer ; démarrez le gateway, puis le
  publisher. Un Pending engagé sous la
  révision précédente est bloqué (`identity: binding_revision`) et exige
  aujourd'hui un nouvel enrôlement et de nouveaux journaux ; une supplantation
  sûre est proposée à Root, non implémentée.
- **Nouveau credential, nouveau grant, nouvelle incarnation** : produits
  seulement par `broker-identity enroll` côté contrôle, toujours avec une
  incarnation neuve, de sorte que de nouveaux journaux vides ne voient aucune
  tête distante de leur incarnation ; conservez les anciens journaux comme
  preuves. Un nouveau credential sur la même incarnation n'est pas un parcours
  pris en charge (de nouveaux journaux refuseraient les têtes distantes
  existantes).
- **Révocation ou expiration** : le handshake ou le gateway refuse ; le
  publisher s'arrête en conservant son Pending et ne le rejoue jamais sous un
  credential révoqué.
- **Avancée de la révision de closure** : acceptée, y compris pendant un
  Pending.
- **Régression de la révision de closure** (par exemple une sauvegarde de
  contrôle plus ancienne) : `closure_regression`, arrêt.
- **Nouvelle closure, source, format de Store (14→15), tenant ou namespace** :
  le handshake signale le pin ; un nouvel enrôlement et de nouveaux journaux
  sont nécessaires.

## 12. Sauvegarde et restauration

Sauvegardez la configuration, l'enrôlement et les journaux uniquement service
arrêté, comme preuves. L'ancre lie le journal à son répertoire d'origine : une
copie restaurée dans un autre répertoire est étrangère, et un historique plus
ancien restauré sur place diverge de la tête du contrôle. Une sauvegarde de
journal n'est donc pas un état reprenable. Après la perte d'un journal,
conservez chaque octet restant et demandez un nouvel enrôlement avec une
nouvelle incarnation.

Une sauvegarde de contrôle restaurée en retard sur l'état acquitté par le broker
est détectée comme une régression de closure ou une tête divergente ;
rapprochez d'abord côté contrôle.

## 13. Aucune adoption distante

Un journal sans historique qui rencontre une tête de contrôle de son
incarnation s'arrête avec `remote_head: without_local_history` ; une tête de
contrôle différente du dernier acquittement local s'arrête avec
`remote_head: diverged` (par exemple un second publisher utilisant le même
enrôlement sur un autre hôte). Le publisher n'adopte, ne réécrit et ne
réinitialise jamais une tête. Exécutez exactement un publisher par enrôlement ;
le démarrage local refuse deux profils owned du même broker, mais un autre hôte
ne peut être détecté que par cette divergence.

## 14. Limites et statut

- Admission, compilation, formatage, tests, runtime, multi-hôte, fournisseurs
  et infrastructure : `NOT_RUN`. Les décisions de Root sur le writer
  d'enrôlement hors ligne et sur le Pending bloqué par un renouvellement de
  binding restent ouvertes.
- Aucun rechargement à chaud, aucun archivage ni purge, aucune réparation
  automatique après une écriture interrompue, aucun horizon de rétention
  qualifié, aucun coût mesuré du journal au démarrage ou par opération
  (section 5).
- Les vues, la fraîcheur et les permissions côté contrôle sont décrites par le
  guide de contrôle associé ; un heartbeat plus ancien que son lease montre
  seulement que la dernière déclaration a expiré.
