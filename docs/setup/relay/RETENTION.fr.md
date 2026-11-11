# Budgets de rétention du relais

[English](RETENTION.md) · [Routes et contrat du relais](../workloads/README.fr.md#autoriser-des-messages-dirigés) · [Client Rust du relais worker](README.fr.md)

Le relais conserve le corps de chaque message accepté dans la base du tenant
jusqu'à son retrait. Ce guide explique ce qu'un tenant, un émetteur et un grant
dirigé peuvent garder retenu, et ce qu'un émetteur apprend lorsqu'une borne est
atteinte. Il décrit un comportement logiciel livré. Il ne qualifie ni un
transport réel de worker, ni un runtime de conteneurs, ni la cible de
100 workers.

## Ce qui compte comme retenu

Un corps retenu est un message dont la colonne `relay_messages.body` n'est pas
`NULL`. Il compte depuis l'envoi accepté jusqu'au passage de cette colonne à `NULL`, **y compris
après acquittement et après expiration**. Métadonnées, tombstones et faits
d'audit subsistent après le retrait et ne comptent pas.

Ces budgets comptent les lignes de messages retenus et les octets de leur
contenu. Ce ne sont pas des quotas d'espace disque ou d'inodes : pages SQLite,
index, tombstones, historique d'audit et WAL occupent aussi du stockage. Passer
un corps à `NULL` ne promet ni réduction des fichiers ni effacement sécurisé
de l'ancien contenu. Taille et capacité du système de fichiers exigent une
supervision et des limites d'exploitation distinctes.

L'occupation est lue sur les lignes retenues, dans la transaction SQLite qui
admet l'envoi. Il n'existe aucun compteur séparé à réconcilier. Le rejeu exact
d'un envoi accepté rend son reçu et ne consomme rien, même si l'émetteur est
plein. Les envois concurrents sont sérialisés par cette transaction : une part
n'est jamais dépassée par une course.

## Trois portées

| Portée | Corps comptés | Limite livrée |
|---|---|---|
| Tenant | Tous les corps retenus du tenant | 1024 messages et 16 Mio |
| Émetteur (`sender`) | Corps envoyés par un worker source, tous grants confondus | 128 messages et 2 Mio |
| Grant | Corps envoyés sur un grant dirigé (source → destination + canal) | 128 messages et 2 Mio |

Chaque portée est bornée en messages et en octets. Un envoi est comparé à
l'émetteur, puis au grant, puis au tenant ; la première portée dépassée est
celle qui est rapportée. L'émetteur est identifié par la preuve workload
authentifiée. Aucun champ de requête ne choisit un émetteur, le propriétaire
d'un grant ou un tenant.

Une part est un **plafond, pas une réservation**. Pour les nouveaux envois admis
sous les limites livrées, un émetteur seul ne peut pas épuiser son tenant : au
moins huit émetteurs doivent contribuer. Au-delà de `tenant ÷ sender` émetteurs saturés, un
émetteur encore sous sa propre part peut rencontrer le plafond du tenant.

La part d'un grant vaut par défaut celle de l'émetteur : elle ne change rien
tant qu'elle n'est pas resserrée. Une part de grant plus étroite empêche une
destination ou un canal lent de consommer toute la part de son émetteur.

Les limites livrées s'appliquent à chaque tenant sauf si l'opérateur
[configure une politique](#configurer).

## Configurer

La politique est un objet JSON fermé. Chaque champ est requis ; un champ
inconnu ou manquant est refusé. [relay-retention.json](examples/relay-retention.json)
est un exemple public, sans secret, égal à la politique livrée :

```json
{
  "version": 1,
  "tenant": {"messages": 1024, "bytes": 16777216},
  "sender": {"messages": 128, "bytes": 2097152},
  "grant": {"messages": 128, "bytes": 2097152},
  "automatic_prune_batch": 8,
  "release_on_ack": false
}
```

| Champ | Valeurs acceptées |
|---|---|
| `version` | `1` |
| `tenant` | Au plus 1024 messages et 16777216 octets, et au moins deux fois la part émetteur dans chaque dimension |
| `sender` | Au moins 1 message et 65536 octets, pour qu'un message de taille maximale tienne dans une part vide |
| `grant` | Au moins 1 message et 65536 octets, sans dépasser la part émetteur |
| `automatic_prune_batch` | 0 à 32 ; 0 désactive la libération automatique |
| `release_on_ack` | `false`, ou `true` après lecture de [sa condition](#libération-des-corps-retenus) |

Pour garantir strictement qu'aucun émetteur sous sa part ne rencontre le
plafond du tenant, régler `sender` à `tenant ÷ (nombre maximal de workers
émetteurs)`.

- **Registre des tenants.** Placer l'objet dans le champ optionnel
  `relay_retention_policy` de l'entrée du tenant dans `--tenants-file`. Chaque
  tenant a sa propre politique ; un champ absent signifie la politique livrée.
- **Gateway local mono-tenant.** Copier l'objet dans un fichier régulier privé
  neuf, mode 0600 sous un parent de contrôle privé 0700, d'au plus 16 Kio, et
  ajouter `--relay-retention-file /absolute/private/ai-guard/relay-retention.json`
  à la commande gateway existante. L'option est incompatible avec
  `--tenants-file`.

Toutes les politiques du registre sont validées avant l'ouverture ou la
création de la première base de tenant ; une politique invalide arrête le
gateway sans toucher à aucun store. Au démarrage, le gateway affiche une ligne
par tenant :

```text
Relay retention tenant=<id> source=derived_defaults|tenant_registry|private_policy_file release_on_ack=<state> policy=<json> fingerprint=<64 hex>
```

La politique n'est lue qu'au démarrage ; modifier le fichier ou le registre
n'est pas un rechargement à chaud. Une politique resserrée ne supprime ni
n'invalide aucun contenu : les émetteurs au-dessus de leur nouvelle part se
voient refuser de nouveaux envois jusqu'à la libération de corps, et le
[diagnostic](#diagnostiquer) montre `retained` supérieur à `limit`.

Un gateway redémarré rouvre ses stores avec une supervision inactive. Tant que
l'activité supervisée n'a pas repris par le contrôle de confiance, les envois
des workers sont refusés : la nouvelle politique ne régit donc aucun trafic et
la libération automatique ne s'exécute pas ; le diagnostic et le prune du
runner restent disponibles.

## Libération des corps retenus

Un corps cesse de compter lorsque sa colonne passe à `NULL`. Le retrait
conserve les métadonnées du message et écrit un fait d'audit `prune` sans
contenu ; le corps n'est jamais copié dans l'audit.

**Libération automatique à l'envoi.** Chaque envoi autorisé libère d'abord des
corps expirés, au plus 8 par envoi (`automatic_prune_batch`, 0 à 32) : les
corps expirés de l'émetteur d'abord, puis les plus anciens corps expirés du
tenant, de sorte que les corps laissés par un émetteur disparu sont drainés par
le trafic des autres. Un corps n'est libéré qu'à partir de son `expires_at` ;
il n'y a pas de délai de grâce.

- Chaque libération est un fait d'audit sans acteur, daté à l'expiration ou
  après. Son `correlation_id` vaut
  `relay-auto-prune:<operation_id>:<worker_id>` : l'opération d'envoi qui a
  déclenché la libération et le worker authentifié qui l'a émise. C'est une
  provenance, pas une imputation : ce worker n'est pas nécessairement
  l'émetteur du corps libéré, que le fait désigne dans ses propres
  métadonnées. Un prune du runner ne porte jamais ce préfixe.
- Une libération est conservée même si l'envoi qui l'a déclenchée est ensuite
  refusé. L'opération déclencheuse peut donc être absente de l'audit. Un
  nouvel essai de la même opération poursuit la libération au lieu de la
  répéter.
- La libération automatique est admise en classe d'audit ordinaire. Elle ne
  consomme jamais les réserves d'audit de contrôle et d'identité. Lorsque le
  budget d'audit ordinaire est épuisé, elle s'arrête et rien n'est retiré sans
  son fait.
- La libération n'a lieu que dans un envoi autorisé. Un tenant inactif garde
  ses corps expirés, tout comme un gateway redémarré : un store rouvert
  n'admet aucun envoi de worker tant que la supervision n'est pas de nouveau
  active.

**Libération sur acquittement (option explicite).** Par défaut, un acquittement
enregistre la réception et conserve le corps jusqu'à son expiration. Avec
`release_on_ack: true`, le fait d'acquittement du destinataire retire aussi le
corps : un seul fait, imputé au destinataire, sans prune séparé. Les corps non
acquittés attendent toujours leur expiration, et un corps expiré ne peut plus
être acquitté : avec `automatic_prune_batch: 0`, il reste alors jusqu'à un
prune du runner. C'est une **porte à sens unique** : une base contenant un tel fait est refusée par les binaires gateway
antérieurs à cette option. Faire une sauvegarde avant de l'activer ; revenir en
arrière signifie restaurer cette sauvegarde ou conserver cette révision. La
ligne de démarrage indique
`release_on_ack=enabled_history_unreadable_by_earlier_binaries`.

**Prune délibéré.** Le runner peut retirer un corps expiré par
[`POST /relay/messages/{id}/prune`](../workloads/README.fr.md#autoriser-des-messages-dirigés).
Il fonctionne avec la libération automatique désactivée, avec un tenant
inactif, après un redémarrage et lorsque le budget d'audit ordinaire est
épuisé, car il utilise la réserve d'audit de contrôle. Rejouer le même prune
rend son reçu. Purger à nouveau un corps déjà libéré ne libère rien deux fois :
la capacité est comptée sur les lignes.

## Refus

Un envoi au-delà d'une borne est refusé en HTTP 429. Le message n'est pas
stocké et aucun fait d'envoi n'est écrit. La seule trace que peut laisser un
envoi refusé est la libération bornée de corps expirés décrite ci-dessus,
chacune avec son propre fait d'audit.

| `error.code` | Signification | Champs ajoutés |
|---|---|---|
| `relay_sender_capacity_exhausted` | L'émetteur authentifié a atteint sa propre part. | `scope:"sender"`, `dimension`, `limit`, `retained`, `requested_bytes`, `next_release_at`, `retry_action` |
| `relay_grant_capacity_exhausted` | Ce grant dirigé a atteint sa part ; un autre grant du même émetteur peut encore avoir de la place. | Mêmes champs, `scope:"grant"` |
| `relay_capacity_exhausted` | Le plafond du tenant est atteint alors que cet émetteur est sous ses propres parts. | `scope:"tenant"` et `retry_action` uniquement |
| `relay_rate_limited` | Plus de 60 envois sur ce grant dans les 60 dernières secondes. | `scope:"grant"`, `retry_action:"retry_after_window"` |

`dimension` vaut `messages` ou `bytes`. `limit` et `retained` ont la forme
`{"messages":…,"bytes":…}` pour la portée rapportée, mesurés après la passe de
libération de cette tentative. `next_release_at` est la plus proche expiration
parmi les corps encore retenus de cette portée, ou `null`.

| `retry_action` | Que faire |
|---|---|
| `retry_after_release` | La politique permet de libérer de la capacité. Réessayer la même opération après une libération. |
| `retry_after_window` | Réessayer la même opération une fois la fenêtre de fréquence de 60 secondes avancée. |
| `contact_operator` | Demander à l'opérateur d'examiner l'occupation, de purger ou de modifier la politique. Au plafond tenant sans purge automatique, ce conseil est volontairement pessimiste. |

Le conseil de capacité suit la politique configurée et, pour un émetteur ou
un grant, les corps retenus dans cette portée.

| `automatic_prune_batch` | `release_on_ack` | Ce qui dégage la portée refusée | Réponse |
|---|---|---|---|
| supérieur à 0 | `false` | Les envois libèrent ses corps une fois expirés. | `retry_after_release` |
| supérieur à 0 | `true` | De même, et l'acquittement du destinataire libère un corps avant son expiration. | `retry_after_release` |
| 0 | `true` | Seulement l'acquittement d'un corps non expiré. Les envois ne libèrent rien, et un corps expiré ne peut plus être acquitté. | Émetteur/grant : `retry_after_release` tant que sa propre portée contient un corps ni expiré ni acquitté, `contact_operator` sinon. Tenant : toujours `contact_operator`. |
| 0 | `false` | Rien d'automatique. | `contact_operator` |

Le conseil est indicatif. Il reste identique si la passe de libération
s'arrête faute de capacité d'audit ordinaire. Le gateway bloque une libération
ou un envoi lorsque son fait d'audit obligatoire ne peut pas être écrit. Le
runner peut purger explicitement un corps expiré si la réserve d'audit de
contrôle permet l'écriture.

- **Refus émetteur ou grant, `automatic_prune_batch` supérieur à 0.** Réessayer
  à partir de `next_release_at`. Une valeur dans le passé indique qu'il reste
  des corps expirés. Un nouvel essai peut lancer une autre passe de libération
  bornée si la capacité d'audit le permet.
- **Refus tenant, `automatic_prune_batch` supérieur à 0.** Aucun
  `next_release_at` n'est donné : réessayer plus tard, après un délai de votre
  choix. Les corps des autres émetteurs expirent dans leur TTL, 3600 secondes
  au plus. Des envois autorisés peuvent ensuite les libérer dans la limite du
  lot configuré et sous réserve de l'audit obligatoire.
- **Émetteur ou grant, `automatic_prune_batch: 0` et `release_on_ack: true`.** `next_release_at`
  est une expiration, pas une date de libération : l'expiration ne libère rien.
  Un nouvel essai peut passer lorsque les acquittements ont libéré assez de
  capacité dans la portée refusée. Lorsque tous les corps de cette portée sont
  expirés ou déjà acquittés, la réponse devient `contact_operator` et seul un
  prune du runner dégage la portée.
- **Refus tenant, `automatic_prune_batch: 0`.** Toujours contacter l'opérateur,
  quels que soient `release_on_ack` et les corps vivants, expirés ou acquittés.
  Un acquittement peut encore libérer de la place, mais la réponse ne révèle
  pas si un voisin possède un corps susceptible d'être acquitté.
- En mode de libération par acquittement seul, `retry_after_release` peut
  rester affiché jusqu'à l'expiration d'un corps après la révocation de son
  grant ou l'arrêt de son destinataire. Le conseil ne vérifie pas si le
  destinataire peut encore acquitter ce corps.

Un émetteur ne reçoit que les chiffres de sa propre portée. Un refus de portée
tenant ne porte ni occupation ni échéance, car elles décriraient d'autres
émetteurs. Son `retry_action` dépend uniquement du réglage de la purge
automatique. Aucun refus ne contient un corps de message.

`scope` vaut toujours `tenant`, `sender` ou `grant`, et `retry_action` l'une
des trois valeurs ci-dessus ; les contrats Rust `RelayRefusalScopeV1` et
`RelayRetryActionV1` nomment exactement ces valeurs.

Exemple d'envoi refusé :

```json
{"error":{
  "code":"relay_sender_capacity_exhausted",
  "message":"This sender reached its retained relay share.",
  "scope":"sender","dimension":"bytes",
  "limit":{"messages":128,"bytes":2097152},
  "retained":{"messages":32,"bytes":2097152},
  "requested_bytes":65536,
  "next_release_at":"2026-11-10T12:00:42.000Z",
  "retry_action":"retry_after_release"
}}
```

Conserver la requête refusée exacte. Réessayer cette même opération, avec le
même `operation_id` et le même contenu, une fois la capacité libérée. Un nouvel
identifiant d'opération pour le même message métier serait un second message.

## Diagnostiquer

`GET /api/v1/relay/retention?limit=32` rend l'état de rétention du tenant
authentifié à son credential runner ou viewer. Une preuve worker est refusée
en 401. Le tenant vient du credential ; le seul paramètre de requête est
`limit`, de 1 à 100 (défaut 32), et tout autre paramètre est refusé en 400
`invalid_relay_request`. La lecture n'écrit aucun événement d'audit, ne rend
aucun corps de message et fonctionne supervision inactive, comme après un
redémarrage.

Garder le credential hors de la ligne de commande. Avec un fichier privé
contenant la seule ligne `Authorization: Bearer <credential viewer>` :

```sh
curl --fail-with-body --silent --show-error --max-time 30 --noproxy '*' \
  --header @/absolute/private/ai-guard/viewer-authorization.header \
  'http://127.0.0.1:8787/api/v1/relay/retention?limit=32'
```

```json
{
  "version": 1,
  "observed_at": "2026-11-10T12:00:00.000Z",
  "policy": {"version": 1, "tenant": {"messages": 1024, "bytes": 16777216}, "sender": {"messages": 128, "bytes": 2097152}, "grant": {"messages": 128, "bytes": 2097152}, "automatic_prune_batch": 8, "release_on_ack": false},
  "policy_fingerprint": "<64 hex>",
  "tenant": {
    "limit": {"messages": 1024, "bytes": 16777216},
    "retained": {"messages": 3, "bytes": 196608},
    "expired": {"messages": 1, "bytes": 65536},
    "acknowledged": {"messages": 1, "bytes": 65536},
    "next_expiry_at": "2026-11-10T11:59:42.000Z"
  },
  "senders": [{"worker_id": "<uuid>", "limit": {"messages": 128, "bytes": 2097152}, "retained": {"messages": 3, "bytes": 196608}, "expired": {"messages": 1, "bytes": 65536}, "next_expiry_at": "2026-11-10T11:59:42.000Z"}],
  "senders_total": 1,
  "grants": [{"grant_id": "<uuid>", "source_worker_id": "<uuid>", "destination_worker_id": "<uuid>", "channel": "tasks", "limit": {"messages": 128, "bytes": 2097152}, "retained": {"messages": 3, "bytes": 196608}, "expired": {"messages": 1, "bytes": 65536}, "next_expiry_at": "2026-11-10T11:59:42.000Z"}],
  "grants_total": 1,
  "prunable": [{"message_id": "<uuid>", "grant_id": "<uuid>", "source_worker_id": "<uuid>", "destination_worker_id": "<uuid>", "body_bytes": 65536, "expires_at": "2026-11-10T11:59:42.000Z", "acknowledged": false}],
  "prunable_total": 1
}
```

- `retained` compte les colonnes de corps non `NULL`. `expired` et
  `acknowledged` en sont des sous-ensembles, évalués à `observed_at`.
- `senders` et `grants` ne listent que les portées ayant au moins un corps
  retenu, par octets retenus décroissants puis par identifiant. `prunable`
  liste les corps expirés encore retenus, par expiration croissante : chaque
  `message_id` peut être donné au prune du runner.
- Chaque liste contient au plus `limit` lignes. Comparer sa longueur à
  `senders_total`, `grants_total` ou `prunable_total` : un total supérieur
  signifie que la liste est tronquée, pas que le reste est vide.
- `policy` et `policy_fingerprint` sont la politique appliquée par ce processus
  gateway.
- Un 429 `capacity_exhausted` ou un 503 est une erreur à réessayer ou à
  examiner, jamais un relais vide.

Cette route n'est servie que par l'API du gateway. Elle n'est pas publiée sous
`/control` et le dashboard ne la relaie ni ne l'affiche.

## Compatibilité

- Le schéma SQLite, les DTO du relais, les grants, messages et historiques
  d'audit existants sont inchangés. Aucune migration n'est requise.
- Les corps déjà retenus comptent dans les nouvelles parts dès l'exécution de
  cette révision. Un émetteur au-dessus de sa part se voit refuser de nouveaux
  envois jusqu'au retrait de corps ; rien n'est invalidé ni supprimé pour
  faire de la place.
- Nouveau comportement par défaut : un émetteur est refusé au-delà de
  128 messages retenus ou 2 Mio, un grant de même. Les révisions antérieures
  n'appliquaient que le plafond du tenant.
- Nouveau comportement par défaut : les corps expirés sont libérés
  automatiquement par les envois suivants. Les révisions antérieures gardaient
  chaque corps jusqu'à un prune du runner.
- Retour arrière : chaque fait de libération automatique respecte la règle que
  les révisions antérieures appliquent déjà à un prune (aucun acteur,
  expiration antérieure ou égale au fait, horodatage propre et postérieur).
  L'ouverture d'une telle base par un binaire antérieur n'a pas été exécutée
  en test ; faire une sauvegarde avant la mise à jour.
- `release_on_ack: true` est l'exception : ses faits d'acquittement sont
  refusés par les binaires antérieurs. L'option est désactivée tant que
  l'opérateur ne l'active pas.
- Le registre des tenants gagne le champ optionnel `relay_retention_policy`.
  Un gateway antérieur refuse un registre qui le contient ; retirer le champ
  avant de démarrer un binaire antérieur.

## Limites

- Le client Rust du worker lit les champs typés ci-dessus par
  [`send_with_refusal`](README.fr.md#lire-un-envoi-refusé). `send` et stdio V1
  conservent le `Capacity` générique. L'enveloppe explicite
  [V2 réservée aux envois stdio](README.fr.md#choisir-les-détails-de-refus-denvoi-sur-stdio)
  peut transporter le même détail fermé par un helper/contrôle compatibles,
  sans changer l'identité d'opération ni les résultats inconnus. Ce nouveau
  raccord reste source-only, sans QA comportementale ou qualification runtime.
- Le démarrage du binaire gateway avec ces options est couvert par un test
  logiciel sur loopback, avec des stores préparés par les migrations
  explicites : ligne de démarrage, politique appliquée lue par la route de
  diagnostic et configurations refusées. Ce n'est pas une qualification de
  déploiement.
- Le diagnostic n'est disponible ni pour les workers, ni sur `/control`, ni
  dans le dashboard.
- La fréquence d'envoi par minute reste par grant. Il n'existe pas de
  fréquence par émetteur.
- D'autres bornes partagées s'appliquent toujours à tous les workers d'un
  tenant et peuvent produire leur propre 429 : la porte d'admission d'E/S du
  tenant (`capacity_exhausted`) et les budgets d'audit workload
  (`audit_workload_budget_exhausted`).
- Les corps d'un grant révoqué ou d'un worker terminé continuent de compter
  jusqu'à leur expiration puis leur libération.
- Chaque message coûte à terme un événement d'audit de plus, sa libération.
  Cet événement est une écriture d'audit ordinaire, imputée à aucun budget
  d'audit workload : ni celui d'un worker, ni celui du tenant. Pour N messages
  qu'il envoie, un worker induit donc jusqu'à N faits de libération dans la
  part d'audit ordinaire, en plus de son propre budget workload : jusqu'à deux
  fois ce budget au total, et de même pour le budget workload du tenant. Les
  libérations n'entament jamais les réserves de contrôle et d'identité. La
  capacité d'audit à vie du store n'est pas étendue par ce lot.
- Un envoi effectue au plus 32 libérations et lit au plus 1024 métadonnées
  retenues dans sa transaction. C'est borné, mais sa durée n'a pas été mesurée
  sous charge.
- Il s'agit de tests logiciels sur le vrai routeur et SQLite, avec des faits
  runtime de worker synthétiques. Le transport réel des workers, Docker,
  Podman et Kubernetes restent non qualifiés pour ce comportement.
