# Plan de contrôle de fédération owned

[English](README.md) · [Index de configuration](../README.fr.md) · [Publisher de broker distant owned](../federation-broker/README.fr.md) · [Identités workload](../workloads/README.fr.md) · [Adopter les sources humaines](../human-sources-maintenance/README.fr.md)

> **Candidat d'intégration local, non activé et non qualifié.** Root a retenu
> la maintenance Rust IdentityAuthority hors ligne, le profil final fermé
> `broker_supervision_owned_v2` et le journal version 2. Feuilles Claude et
> coutures parentes sont composées ; C3 ajoute la reprise exacte après rotation
> avant première activation. Compilation, formatage, tests, QA, runtime,
> multi-hôte et fournisseurs : `NOT_RUN` sur ce candidat. Procédures ci-dessous
> avec emplacements fictifs. Root conserve revue, admission QA et activation.

## Périmètre

Le profil owned permet à un broker distant de publier sa présence déclarée
vers le plan de contrôle de son propre tenant, sous une identité émise par une
maintenance hors ligne et authentifiée dans la source retenue du tenant. Ce
guide couvre :

- le cycle de vie hors ligne de l'identité broker : `enroll`, `rotate`,
  `revoke`, `resume`, `status` (sous-commandes de
  `agentsandbox human-sources broker-identity`) ;
- le modèle de confiance de la remise d'enrôlement ;
- les sept routes fermées de supervision owned sur le transport de contrôle ;
- la page humaine des brokers owned sur `GET /api/v1/human/owned/brokers`
  (API seulement : le dashboard ne l'affiche pas encore) ;
- le diagnostic des refus fermés, la sauvegarde/restauration et la reprise.

Il ne couvre pas le publisher, qui a son propre
[guide](../federation-broker/README.fr.md). Rien dans ce profil n'admet un
worker, un relais, un runtime Kubernetes ou une sortie réseau, n'envoie de
commande distante, ne fence un processus, ne libère un quota ni ne mesure un
usage. Un heartbeat est une déclaration dont la réception est observée ; un
checkpoint est une liste `DeclaredPartial` d'au plus 100 têtes déclarées
(32 Kio, `has_more`). Une liste vide ne prouve jamais une activité nulle.

## Ce qui existe et ce qui n'existe pas

| Élément | État |
|---|---|
| Contrat final v2 et journal v2 | Source intégrée ; ancien profil refusé, aucune conversion v1 implicite |
| Routes Store/API, page humaine et cycle IdentityAuthority hors ligne | Source composée ; validation et activation en attente |
| Réconciliation exacte après rotation et Superseded append-only financé | Source candidate implémentée ; tests écrits NOT_RUN |
| Sélecteur humain de protocoles de lecture | Broker owned v1 disponible ; worker/fédération explicitement indisponibles, sans permission implicite |
| Intégration dashboard | Livraison frontend distincte ; aucune qualification navigateur déduite ici |
| Writer d'identité online, invalidation ciblée d'index | Non admis ; maintenance hors ligne seulement |
| Compilation, formatage, tests, runtime, qualification multi-hôte | NOT_RUN sur ce candidat |

## Prérequis

1. L'union des tenants est déjà adoptée avec la
   [maintenance des sources humaines](../human-sources-maintenance/README.fr.md) :
   chaque Store de tenant est au format **14 ou 15** et son descripteur retenu
   exige exactement ce format ; Auth5 si l'OIDC est sélectionné. Un Store au
   format 6–13 conserve les voies de présence legacy et est refusé ici
   (`migration_required`).
2. L'activation privée (`activation.json`) ou les options explicites de
   sources owned utilisées par le gateway sont disponibles.
3. **Tous** les consommateurs sont arrêtés : gateway, dashboard, ingress et
   brokers. `--confirm-control-stopped` est l'attestation de l'opérateur. La
   commande prend aussi les vrais verrous propriétaires : un gateway en marche
   la fait refuser avec `owned_union_owner_contended`. Un ancien processus
   n'est pas fencé.
4. Un répertoire parent privé (propriétaire seul, mode `0700`) existe pour le
   nouveau répertoire de plan, hors de tout répertoire de source, projet et
   export.

L'ouverture de l'union enregistre une incarnation de contrôle dans **toutes**
les sources, exactement comme un démarrage du gateway. Chaque sous-commande,
`status` compris, écrit donc cet enregistrement : aucune n'est un diagnostic
SQLite en lecture seule.

## Modèle de confiance et remise

La maintenance génère elle-même le bearer du broker (32 octets du générateur
du système, même format `aigw1_` que le CLI workload). Le bearer n'apparaît
jamais dans un argument, le plan, un reçu, la remise, la sortie standard ou
un journal. Seule son empreinte SHA-256 atteint le Store. Avant toute
écriture, l'empreinte est comparée à chaque credential technique
runner/viewer et à chaque credential humain et workload, courant ou
historique, de chaque source ; une collision refuse le plan.

Un `enroll` ou un `rotate` terminé laisse deux fichiers privés dans le
répertoire de plan :

| Fichier | Contenu |
|---|---|
| `bearer` | Le secret et un retour à la ligne, mode `0600`, créé exclusivement avant le plan. |
| `enrollment.json` | `OwnedBrokerEnrollmentV2` public : profil, origine canonique du gateway, révision de closure à l'émission, sujet engagé, autorité unique, lease, échéance du grant, échéance du credential, heure d'émission et reçus d'audit du grant et du binding. Ni bearer ni empreinte. |

La remise est construite uniquement depuis des reçus committés, puis
recoupée en authentifiant le nouveau bearer par le même chemin Store scopé
que le gateway. Elle **n'authentifie rien par elle-même** : à chaque
opération, le gateway authentifie le bearer dans la source retenue et compare
le sujet engagé. Après `enroll`, transférer les deux fichiers vers l'hôte du
broker par un canal privé et les garder en `0600` ; suivre le
[guide du publisher](../federation-broker/README.fr.md) (livré avec le lot
publisher) pour les installer avec de nouveaux journaux owned. Après
`rotate`, seul le nouveau `bearer` part vers l'hôte du broker ; voir
[Faire tourner le bearer](#faire-tourner-le-bearer). Le champ
`publisher_action` du rapport indique l'action exacte de chaque commande.

Le sujet engagé comprend : installation, ID de closure, source, format de
source, tenant, namespace de contrôle, broker, incarnation, credential,
révision de binding et révision de grant. La révision de closure est
seulement observée : elle peut avancer, mais une valeur inférieure à celle
déjà acquittée est une régression. Tout changement d'un champ épinglé exige
une nouvelle remise et un nouveau journal publisher.

## Compatibilité et migration

| Profil | Format du Store | Accès |
|---|---|---|
| Présence legacy | 6–13 heartbeat, 12–13 checkpoint | Routes legacy, inchangées |
| `broker_supervision_owned_v2` | Exactement 14 ou 15, égal au descripteur retenu | Routes owned scopées uniquement |

Aucun format n'est servi par les deux profils. Une migration 14→15 change un
pin et exige un nouvel enrôlement. Un gateway qui ne sert pas le profil owned
refuse les routes owned : par `/control`, `404 unknown_control_route` avec
`outcome:not_dispatched` ; par le routeur public sans le marqueur du gate,
`503 identity_unavailable`. Un ancien binaire ne connaît pas la
sous-commande `broker-identity` ; conserver le binaire qui a créé un plan pour
le reprendre.

## Prendre la preuve de sauvegarde

Tous les consommateurs étant arrêtés, prendre une copie cohérente du Store du
tenant sélectionné avec la procédure de sauvegarde SQLite existante, par
exemple `sqlite3 <store-du-tenant> ".backup '<nouveau-fichier-de-sauvegarde>'"`
(à valider dans votre environnement). Ensuite :

- la garder comme fichier régulier à lien unique, en mode `0600`, dans un
  répertoire privé qui n'est le répertoire **d'aucune** source, et hors des
  fichiers de configuration de l'union ;
- vérifier qu'aucun fichier `-wal`, `-journal` ou `-shm` ne se trouve à côté ;
- relever son SHA-256 (`shasum -a 256 <nouveau-fichier-de-sauvegarde>` ou
  `sha256sum <nouveau-fichier-de-sauvegarde>`).

Avant la première mutation, la commande vérifie le fichier et son empreinte
(avant et après), l'ouvre en lecture seule et immuable, et contrôle format,
schéma, liaison et quota du tenant, namespace de contrôle, intégrité de la
chaîne et `quick_check`. Sa tête d'audit doit être **exactement** l'état de la
source avant l'enregistrement d'incarnation de cette session. Une sauvegarde
prise avant un autre démarrage du gateway ou une autre session de maintenance
est donc refusée : en prendre une nouvelle. Une reprise exige le même fichier
et la même empreinte, et sa tête doit rester un préfixe de la chaîne vivante.

## Enrôler un broker

```sh
agentsandbox human-sources broker-identity enroll \
  --human-sources-activation /chemin/absolu/controle/human-preparation/activation.json \
  --confirm-control-stopped \
  --backup-file /chemin/absolu/prive/sauvegardes/<tenant>-avant-enrolement.sqlite \
  --backup-sha256 <64-hex-minuscules> \
  --tenant-id <tenant-id> \
  --new-broker \
  --authority-id <libelle-autorite> \
  --runtime-kind docker \
  --lease-seconds <1-300> \
  --grant-not-after <heure-RFC3339> \
  --gateway-origin https://<hote-controle> \
  --plan-dir /chemin/absolu/prive/identite-broker/<nouveau-plan>
```

- Au lieu de l'activation, les options explicites de sources owned du gateway
  sont acceptées : `--human-sources-manifest`, `--human-credentials-envelope`,
  `--tenants-file`, `--human-control-config-revision`, et éventuellement
  `--oidc-config` avec `--human-oidc-mapping-revision` et `--max-events`.
- Utiliser `--broker-id <uuid-broker>` au lieu de `--new-broker` pour un
  broker existant ou un ID choisi par l'opérateur. Si ce broker a encore un
  grant actif, ajouter `--replace-active-grant` : un nouveau grant utilise
  toujours une nouvelle incarnation.
- `--authority-id` est un libellé court (`A-Z a-z 0-9 . _ : -`, au plus
  128 octets) ; `--runtime-kind` vaut `docker` ou `podman`. C'est un périmètre
  déclaré, pas un adaptateur installé ; aucun broker Kubernetes n'est admis
  par ce profil.
- Le grant expire au plus 366 jours plus tard. `--credential-expires-at` est
  facultatif, jamais après le grant, et vaut le grant par défaut. Les heures
  sont conservées en UTC à la milliseconde.
- `--gateway-origin` est l'origine canonique qu'utilisera le broker : HTTPS,
  ou HTTP uniquement vers une IP littérale de loopback, sans utilisateur,
  chemin, query ni fragment.

Le répertoire de plan est neuf et n'est jamais adopté s'il existe. Il reçoit,
dans l'ordre, `bearer`, `plan.json` (tous les IDs d'opération fixés à
l'avance, les pins de source et la preuve de sauvegarde), un reçu
`step-<n>.json` par étape committée, puis `enrollment.json`. Étapes : un grant
avec une incarnation et un ID de credential neufs, puis un binding broker par
empreinte seule. Chaque étape s'exécute sous un epoch d'écriture neuf dans une
transaction `IdentityAuthority`. En cas de succès, la sortie standard est un
rapport JSON avec `state:"complete"`, l'engagement d'enrôlement, le sujet
public et `candidate_index:"rebuilt_at_next_gateway_start"`.

Redémarrer ensuite normalement le gateway : il reconstruit l'index des
candidats de l'union. Un gateway en marche devrait de toute façon le
reconstruire, car un changement de binding committé fait avancer la
génération de découverte owned.

## Faire tourner le bearer

```sh
agentsandbox human-sources broker-identity rotate \
  --human-sources-activation /chemin/absolu/controle/human-preparation/activation.json \
  --confirm-control-stopped \
  --backup-file /chemin/absolu/prive/sauvegardes/<tenant>-avant-rotation.sqlite \
  --backup-sha256 <64-hex-minuscules> \
  --tenant-id <tenant-id> \
  --broker-id <uuid-broker> \
  --gateway-origin https://<hote-controle> \
  --plan-dir /chemin/absolu/prive/identite-broker/<nouveau-plan>
```

La rotation conserve credential, broker, incarnation et grant ; elle lie une
nouvelle empreinte de bearer et fait avancer la révision de binding. Le grant
doit être courant et le binding courant non révoqué et non expiré ;
l'échéance n'est jamais prolongée sur place (un grant ultérieur est un nouvel
enrôlement). L'ancien bearer cesse d'authentifier.

Sur le publisher, **remplacer seulement le fichier bearer, publisher arrêté**,
et conserver le fichier d'enrôlement d'origine et les deux journaux owned :
les pins de cet enrôlement admettent une révision de binding supérieure pour
les nouvelles opérations. Le `enrollment.json` écrit par `rotate` sert
uniquement de preuve des reçus de rotation et ne doit **pas** être installé
sur le publisher : son nouveau reçu de binding change l'engagement
d'enrôlement, donc les journaux existants seraient refusés comme étrangers, et
de nouveaux journaux vides refuseraient les têtes existantes du contrôle pour
la même incarnation (une tête distante n'est jamais adoptée). Le rapport
indique `publisher_action: replace_only_the_bearer_file_while_stopped_keep_enrollment_and_journals`.

Le rapport ne liste que `bearer` dans `files` et `enrollment.json` dans
`evidence_files`.

Ordre (parcours à valider par Root) : (1) arrêter le publisher owned pendant
que le gateway sert encore, afin qu'un Pending retenu puisse encore être
acquitté ; (2) vérifier en lecture seule que, dans chaque journal owned, le
dernier répertoire `op-*` contenant un `pending.json` contient aussi un
`ack.json` (un répertoire `op-*` final vide n'est pas un Pending) ; sinon un
Pending est retenu : redémarrer le publisher, gateway toujours en service,
jusqu'à son acquittement, l'arrêter de nouveau, ou le conserver et accepter le
cas bloqué ci-dessous ; (3) arrêter le gateway ;
(4) lancer `rotate` ; (5) remplacer le fichier bearer ; (6) démarrer le
gateway, puis le publisher. Un Pending conservé n'est jamais rejoué avec une identité/hash nouveaux.
La reprise v2 ci-dessous garde enrollment/journaux d'origine, puis enregistre
l'ACK historique exact ou supplante durablement une opération exactement
absente après barrière hors ligne. Conflit/unknown restent bloqués.

## Révoquer

```sh
agentsandbox human-sources broker-identity revoke \
  --human-sources-activation /chemin/absolu/controle/human-preparation/activation.json \
  --confirm-control-stopped \
  --backup-file /chemin/absolu/prive/sauvegardes/<tenant>-avant-revocation.sqlite \
  --backup-sha256 <64-hex-minuscules> \
  --tenant-id <tenant-id> \
  --broker-id <uuid-broker> \
  --scope all \
  --plan-dir /chemin/absolu/prive/identite-broker/<nouveau-plan>
```

`--scope all` (défaut) révoque le grant, puis chaque binding non révoqué de
son incarnation courante ; `grant` ou `binding` ne révoque qu'un côté. Le
Store lie chaque révocation de binding au broker du plan : une étape de plan
qui désigne un credential d'un autre broker (ou d'un worker) est refusée avec
`broker_identity_durable_conflict` et ne révoque rien.
L'historique et les empreintes sont conservés : une empreinte révoquée reste
une collision et n'est jamais réémise. Après le démarrage suivant du gateway,
l'opération suivante du publisher est refusée (`owned_broker_forbidden` ou
`owned_broker_authentication_required`). La révocation ne prouve pas que les
workers se sont arrêtés, ne libère pas de quota et n'envoie aucune commande ;
suivre les procédures d'arrêt du broker et de réconciliation runtime.

## Reprendre une maintenance partielle

```sh
agentsandbox human-sources broker-identity resume \
  --human-sources-activation /chemin/absolu/controle/human-preparation/activation.json \
  --confirm-control-stopped \
  --plan-dir /chemin/absolu/prive/identite-broker/<plan-existant>
```

La reprise exige des pins de source égaux (installation, ID de closure,
source, tenant, format, namespace) et une révision de closure sans
régression, le même fichier de sauvegarde et la même empreinte, et une tête de
sauvegarde qui reste un préfixe de la chaîne vivante. Elle rejoue ensuite
chaque étape dans l'ordre avec son ID d'opération fixé :

- une étape committée renvoie son reçu original (`replayed`), comparé au reçu
  enregistré ;
- une étape dont le reçu a été perdu après commit est récupérée par ce même
  rejeu ;
- un contenu divergent pour la même opération est un conflit ;
- un reçu enregistré qui n'est plus dans la chaîne durable (par exemple après
  restauration d'une sauvegarde contrôle plus ancienne) arrête **avant**
  toute écriture.

Un `enrollment.json` existant est vérifié contre les reçus et jamais
réécrit. Un refus indique `state=partial_maintenance` avec `completed_steps`
et `total_steps` lorsque cette exécution a confirmé au moins une étape.
Conserver le répertoire de plan ; ne jamais le modifier, le renommer ni le
réutiliser. Si une étape ne peut plus s'appliquer (par exemple le grant a
expiré avant son binding), révoquer puis créer un nouveau plan.

## Lire l'état et l'historique

```sh
agentsandbox human-sources broker-identity status \
  --human-sources-activation /chemin/absolu/controle/human-preparation/activation.json \
  --confirm-control-stopped \
  --tenant-id <tenant-id> \
  --broker-id <uuid-broker> \
  --history-limit 20
```

Le rapport montre `evaluated_at`, la vue du broker (révision de grant, état
`unseen|current|stale|expired|revoked|clock_uncertain`, lease, échéance,
`runtime_inventory_complete:false`), les bindings de l'incarnation courante et
une page d'historique d'un credential, du plus récent au plus ancien, sans
empreinte (`--credential-id`, par défaut celui du grant ;
`--before-revision` pour la page suivante).
`broker_presence:"absent_from_complete_durable_read"` est une absence durable
complète dans ce Store de tenant. Une page d'historique porte
`count_scope:"page"` ; une page vide ne prouve pas l'absence d'historique.

## Routes de supervision owned (côté contrôle)

Après admission des coutures par Root, le transport de contrôle classe
exactement ces cibles à la racine du gateway (après retrait de `/control`) :

| Opération | Méthode et chemin | Corps | Réponse |
|---|---|---|---|
| identité | `GET /api/v1/owned/brokers/identity` | aucun | ≤ 4 Kio |
| vue de soi | `GET /api/v1/owned/brokers/self` | aucun | ≤ 16 Kio |
| heartbeat | `POST /api/v1/owned/brokers/heartbeats` | ≤ 4 Kio | ≤ 16 Kio |
| checkpoint | `POST /api/v1/owned/brokers/checkpoints` | ≤ 36 Kio | ≤ 40 Kio |
| vue de soi checkpoint | `GET /api/v1/owned/brokers/checkpoints/self` | aucun | ≤ 40 Kio |
| réconciliation heartbeat (lecture) | `POST /api/v1/owned/brokers/heartbeats/reconcile` | ≤ 96 Kio | ≤ 128 Kio |
| réconciliation checkpoint (lecture) | `POST /api/v1/owned/brokers/checkpoints/reconcile` | ≤ 96 Kio | ≤ 128 Kio |

La réconciliation authentifie le binding courant actif avant le lookup global
d'operation_id, dans la même transaction de source que l'historique exact
r→r+1, la tête et les reçus. Le reçu réel d'incarnation du propriétaire courant,
strictement après rotation, prouve la barrière de réouverture exclusive. Un
autre broker/type/hash sous cet ID est un conflit, jamais une absence. Lecture
incomplète, indisponible, masquée ou périmée ne devient jamais NonCommitted.

CommittedExact conserve identité d'opération r et reçu séparés de
l'observation r+1, sans write métier ni POST. NonCommitted exige absence
globale exacte et prédécesseur local exact ; alors seulement le publisher
finance et synchronise Superseded append-only avec un remplacement/UUID
complet unique, mêmes séquence/CAS et snapshot checkpoint. Guard et rejeu
ordinaires restent exacts. Aucune migration de journal v1, adoption distante,
exception de credential révoqué ou nouvelle autorité d'exécution. Voir le
[contrat de reprise publisher](../federation-broker/README.fr.md#reprise-exacte-après-rotation).

Octets exacts uniquement : ni décodage, ni query, ni alias HEAD, ni préfixe.
Le gate résout le bearer dans l'index complet des candidats construit au
démarrage (exactement un candidat workload désigne la seule source qui peut
l'authentifier ; aucun SQL sur un autre tenant) et l'authentifie dans une
transaction contrôlée de cette source, puis prend le permis de présence du
tenant et attache un marqueur privé ; un handler sans ce marqueur répond
`503 identity_unavailable` avant tout corps, Store ou audit.

Chaque opération s'exécute ensuite dans **une seule** transaction contrôlée
de la même source : la génération de découverte relevée au démarrage est
comparée à la génération courante, le même bearer est authentifié à nouveau
et doit rester la preuve broker du gate, l'identité engagée est reconstruite,
le sujet et les engagements de la requête sont comparés, le hash d'entrée
privé du registre est recalculé contre le miroir commun avant tout rejeu ou
mutation, et la génération est comparée de nouveau avant le commit. Une
génération changée (par exemple un binding modifié depuis le démarrage)
refuse avec `owned_source_rebuild_required` **sans** mutation et ferme les
contextes humains jusqu'au redémarrage du gateway. Une réponse positive n'est
publiée qu'après commit et écran de masquage, jamais altérée ; un refus
après un commit possible est `owned_response_withheld`. Les opérations de
présence partagent les quatre permis de présence du tenant, et la voie de
lecture Store du gateway avec les lectures humaines ; leur latence sous la
recette à trois workers n'est pas mesurée (`NOT_RUN`).

## Page humaine des brokers owned

Sur un gateway owned, `GET /api/v1/human/owned/brokers` (query `limit` de 1
à 100, 25 par défaut, et `cursor` scellé) sert la page owned. Sa forme n'est
pas celle de la page legacy du registre : elle a donc son propre chemin et
porte `profile:"human_broker_registry_owned_v1"` ; le chemin legacy
`GET /api/v1/human/brokers` y reste fermé (`503 identity_unavailable`), comme
avant, et le dashboard existant ne reçoit jamais la forme owned. Le dashboard
n'affiche pas encore la page owned : son parseur, ses états et son chemin de
proxy ne sont pas implémentés. Un gateway legacy refuse le chemin owned avant
toute authentification ou travail Store.

La page utilise une preuve humaine locale dans un contexte Administration
avec la capacité `ReadBrokerRegistry`, revalidé dans la transaction de
lecture de la source propre de la preuve. L'audit de lecture obligatoire est
ajouté avant toute page, même vide ; un échec d'audit ne renvoie aucune page.
La page porte `count_scope:"page"`, jamais un total du tenant ; les
fraîcheurs heartbeat et checkpoint sont indépendantes ; `clock_uncertain` est
explicite. Les IDs de job, hashes, IDs d'opération et de credential et
libellés d'autorité ressemblant à un chemin sont exclus. Le curseur est
scellé pour ce profil owned et n'ouvre jamais une page legacy.

`checkpoint.status` vaut `received`, `absent` ou `unknown`, d'après deux
lectures d'index bornées : d'abord les 128 opérations de registre les plus
récentes du broker ; si elles ne contiennent aucune publication et que des
opérations plus anciennes existent, les 4096 publications de checkpoint les
plus récentes du tenant (lues au plus une fois par page). `absent` n'est
indiqué que depuis un historique complet : toutes les opérations du broker,
ou toutes les publications du tenant. `unknown` signifie qu'aucune des deux
bornes ne contient de publication de ce broker et qu'un historique plus
ancien existe, par exemple pour un broker qui ne publie que des heartbeats,
dans un tenant de plus de 4096 publications de checkpoint ; il n'est jamais
présenté comme absent. La vue `checkpoints/self` du broker lit sa dernière
publication sans cette borne.

## Diagnostiquer les refus fermés

Les refus de maintenance sont une ligne sur la sortie d'erreur :
`broker_identity_refused; reason=<code>; state=<état>; completed_steps=<n>; total_steps=<m>; action=<action>; command=<commande>`.

| Motif | Signification | Action |
|---|---|---|
| `owned_union_owner_contended` | Un gateway détient encore les verrous propriétaires. | L'arrêter, puis réessayer. |
| `owned_union_open_refused`, `owned_union_activation_refused`, `owned_union_tenants_refused` | L'union, l'activation ou le registre ne s'est pas ouvert comme pour le gateway. | Vérifier options et fichiers ; les conserver. |
| `owned_union_selection_required`, `owned_union_options_incomplete`, `control_stop_attestation_required`, `broker_selection_required` | Il manque exactement une voie d'union, ses options complètes, l'attestation d'arrêt ou exactement l'un de `--broker-id`/`--new-broker`. | Corriger les arguments. |
| `broker_identity_invalid_input` | Un argument est invalide (heure, lease, origine, autorité, ID). | Le corriger ; aucune étape de plan n'a été écrite. |
| `broker_identity_union_mismatch` | Tenant absent, pins de source changés ou closure en régression. | Vérifier la configuration de l'union et la closure. |
| `broker_identity_migration_required` | Le Store du tenant n'est pas au format 14/15. | Migrer d'abord hors ligne. |
| `broker_identity_backup_refused` | Sauvegarde absente, altérée, en alias, avec fichier compagnon, ou pas exactement l'état d'avant la session. | Prendre une nouvelle sauvegarde, contrôle arrêté. |
| `broker_identity_plan_conflict` | Répertoire existant, fichier divergent, reçu non durable, entrée étrangère. | Conserver le répertoire ; réconcilier avec `status`. |
| `broker_identity_plan_incomplete` | Fichier de plan absent (par exemple un bearer sans plan). | Aucune de ses opérations n'a été écrite ; créer un nouveau plan. |
| `broker_identity_plan_busy` | Un autre processus de maintenance détient le plan. | Attendre sa fin. |
| `broker_identity_state_refused` | L'état durable n'admet pas l'action (grant actif sans `--replace-active-grant`, rien à révoquer, binding révoqué…). | Consulter `status`. |
| `broker_identity_credential_collision` | L'empreinte générée existe déjà. | Créer un nouveau plan ; signaler la collision. |
| `broker_identity_durable_conflict` | Conflit de révision, de séquence ou de contenu dans le Store. | Consulter `status` ; ne reprendre que le même plan. |
| `broker_identity_clock_uncertain` | L'horloge du contrôle précède le plancher durable. | Réconcilier l'horloge, puis reprendre. |
| `broker_identity_audit_unavailable` | La capacité ou l'intégrité de l'audit a refusé l'écriture. | La rétablir, puis reprendre. |
| `broker_identity_unavailable` | Temporaire : source occupée, epoch, stockage. | Reprendre avec le même plan. |
| `broker_identity_handoff_refused` | Le nouveau bearer n'authentifie pas comme prévu. | N'installer aucune remise ; consulter `status` ; reprendre. |
| `broker_identity_output_refused`, `broker_identity_output_unavailable` | Le rapport n'a pas pu être encodé, filtré ou écrit. | Le résultat durable est inchangé ; lancer `status` ou `resume`. |

`state=no_step_confirmed_by_this_run` signifie que cette exécution n'a
confirmé aucune étape du plan (l'ouverture de l'union a tout de même
enregistré son incarnation) ; `state=partial_maintenance` signifie qu'au
moins une étape est durable et que le plan doit être repris, jamais
abandonné.

Les refus des routes owned utilisent l'enveloppe fermée
`{"error":{"code","message","retry_action","profile"}}` :

| Code | HTTP | Classe de reprise du publisher |
|---|---|---|
| `owned_broker_authentication_required` | 401 | arrêt |
| `owned_broker_forbidden` | 403 | arrêt |
| `owned_invalid_request`, `owned_protocol_mismatch` | 400 | arrêt |
| `owned_identity_mismatch`, `owned_operation_conflict`, `owned_sequence_conflict`, `owned_clock_uncertain` | 409 | arrêt |
| `owned_migration_required` | 503 | arrêt |
| `owned_source_rebuild_required`, `owned_audit_unavailable` | 503 | attente opérateur |
| `owned_response_unpublishable`, `owned_credential_ambiguous` | 503 | attente opérateur |
| `owned_supervision_unavailable` | 503 | même opération après backoff borné |
| `owned_response_withheld` | 503 | réconciliation de la même opération |

`owned_source_rebuild_required` signifie que l'index ou la source retenue a
changé depuis le démarrage : redémarrer le gateway. `owned_credential_ambiguous`
signifie que l'empreinte présentée est revendiquée par plusieurs classes de
credential ou plusieurs sources. Les empreintes historiques sont conservées
pour toujours (une empreinte révoquée reste une collision), donc ce bearer ne
pourra plus jamais s'authentifier, même après révocation et redémarrage :
traiter le secret dupliqué comme compromis, donner au broker un nouveau bearer
(`rotate`) ou un nouvel enrôlement, et rechercher l'origine du doublon (par
exemple un Store copié ou restauré).
`owned_response_unpublishable` est un refus déterministe de publier une réponse
positive (collision de redaction ou invariant serveur). L'écran du Store
s'exécute avant le commit et annule la transaction ; l'écran du gateway
s'exécute après le travail Store et peut refuser une opération déjà durable.
Dans les deux cas, un nouvel essai ne peut rien changer : le publisher
conserve son Pending et, une fois la configuration du contrôle corrigée, le
rejeu exact renvoie le reçu d'origine. Un refus de présence legacy ou un corps
nommé inconnu n'est pas un code owned.

## Reprise après réception durable et réponse perdue

Un heartbeat ou un checkpoint est committé avec son événement d'audit et sa
projection dans une seule transaction. Si la réponse est ensuite perdue
(connexion coupée, délai dépassé, `owned_response_withheld`), le publisher
réessaie la **même** opération : le rejeu exact renvoie le reçu original avec
`replayed:true`, après revalidation de l'identité sous l'autorité courante.
La vue de soi heartbeat porte le reçu de la dernière opération heartbeat, lu
dans la même transaction que l'identité, pour qu'un publisher ne reconnaisse
que son opération exacte committée. Un contenu divergent avec le même ID
d'opération est `owned_operation_conflict` ; une identité révoquée, expirée ou
remplacée refuse même un rejeu exact.

Côté maintenance, une réponse perdue après une étape committée est récupérée
par `resume` : la même opération renvoie son reçu original.

## Sauvegarde et restauration

- Conserver la sauvegarde d'avant maintenance et son empreinte au moins
  jusqu'à la fin du plan et au redémarrage du gateway ; `resume` les exige.
- Conserver les répertoires de plan : ils contiennent le bearer de la dernière
  remise et les reçus committés. Les protéger comme des credentials.
- Restaurer une sauvegarde contrôle plus ancienne peut faire disparaître des
  étapes d'identité committées. Un `resume` ultérieur d'un plan dont les reçus
  ne sont plus durables s'arrête avant toute écriture ; réconcilier avec
  `status`. La restauration elle-même n'est pas une procédure qualifiée : elle
  peut faire revivre un grant ou un binding révoqué. Préférer la révocation
  puis un nouvel enrôlement à une restauration.
- Un journal ou une remise broker restaurés n'adoptent jamais une tête
  distante ; suivre le guide du publisher.

## Coutures Root non admises

| Patch | Fichier | Rôle |
|---|---|---|
| P00 | `src/broker_registry.rs` | Déclarer le module du contrat commun. |
| A01–A06 | `src/audit/…`, `src/audit.rs` | Déclarer les feuilles Store owned ; extraire les corps partagés du registre et des bindings ; visibilité et réexports. |
| A10–A15 | `src/api/…`, `src/control_transport.rs` | Candidats indexés par source, classement des routes, branche et marqueur du gate, routeur, route de la page humaine owned. |
| A20–A22 | `src/human_authority/maintenance.rs`, `src/human_authority.rs`, `src/human_sources_cli.rs` | Déclarer la maintenance, la rendre atteignable par le binaire, ajouter la sous-commande. |
| A23 | `docs/setup/README*.md` | Entrée d'index de ce guide. |

Ces coutures sont admises ensemble, en une seule intégration : appliquées en
partie, elles laissent des entrées inutilisées ou cassent des tests
existants. Les tests que Root adapte et exécute, les décisions D1/D2 et
l'ordre d'activation relèvent de l'intégration, pas de ce guide.

L'identité humaine annonce `read_profiles` seulement sur le vrai gateway
owned : broker_registry=`human_broker_registry_owned_v1`,
worker_registry=`unavailable`, federation_registry=`unavailable`. Objet omis
en legacy. Ces valeurs fermées choisissent un protocole ; ReadBrokerRegistry
reste obligatoire et revalidé. La vue complète workers/fédération reste une
exigence produit ouverte ; indisponible ne signifie jamais zéro activité.

## Qualification

`NOT_RUN` : compilation, formatage, Clippy, tests unitaires et
d'intégration, exécutions du CLI, vérification de sauvegarde sur de vrais
fichiers, démarrage du gateway avec les routes owned, interopérabilité avec
le publisher, multi-hôte, Docker/Podman/Kubernetes, fournisseurs, TLS et
infrastructure. Ce guide ne ferme aucun gate de production.

La compatibilité Source/Control16 est un **raccord suivant obligatoire** pour
le parcours commun fédération et egress. Ce profil refuse actuellement 16.
Contrat sémantique/versionné explicite et qualifications logicielles/runtime
sont requis ; aucun widening numérique ni qualification production globale
ne découle de l'implémentation 14/15 actuelle.
