# Client Rust du relais worker

[English](README.md) · [Provisionnement workload et contrat du relais](../workloads/README.fr.md) · [Validation des profils](../../engineering/evidence/2026-11-07/relay-control-profile/README.md)

`agentsandbox::relay::client::WorkerRelayClient` appelle les routes HTTP worker
existantes avec une preuve workload dédiée. Il s'agit d'une API de bibliothèque
Rust, sans CLI relais, découverte de variables d'environnement ni
provisionnement automatique de credential. La
[source](../../../src/relay/client.rs) définit les entrées acceptées.

Ce client ne raccorde pas un worker dont le réseau runtime est `none`.
La bibliothèque ne fournit ni handoff runtime, ni canal runtime privé, ni
permission réseau supplémentaire. Conserver les workers hors du réseau de
contrôle. La [direction du raccord](../../engineering/RELAY_RUNTIME_HANDOFF.md)
est approuvée pour code et tests logiciels locaux. Protocole de frames et voie
HTTP fermée sont livrés séparément ; ces tests ne constituent pas la recette
réelle A1/A2/B1.

## Accès de contrôle publié et limité

L'ingress accepte seulement les trois formes worker sous `/control` :
`POST /control/api/v1/relay/messages`, `GET /control/api/v1/relay/messages`
et `POST /control/api/v1/relay/messages/{message_id}/ack`. Le gateway vérifie
la preuve workload avant lecture du corps et réservation de l'un des huit
échanges worker du tenant authentifié. Les capacités runner et présence broker
restent séparées. Une preuve worker ne donne jamais accès aux routes runner.

Seule l'inbox admet une query : `after` non négatif et `limit` de 1 à 32,
décimaux canoniques uniques. Leur omission conserve les valeurs du handler.
Aucune autre query, alias de chemin ou destination libre n'est admis. Une inbox
réussie est bornée à 2 Mio, les reçus/erreurs worker à 16 Kio ; les réponses
historiques du contrôle gardent 512 Kio. Audit, grants, expiration et révocation
restent vérifiés par les handlers habituels.

Cette voie ne raccorde pas le réseau coupé d'un worker. Choisir le profil typé
`RelayClientProfile::ControlIngress` dans le contrôle pour atteindre ces routes
fixes. L'origine reste une origine racine : ne pas ajouter `/control` soi-même
ou placer la preuve dans un worker. `RelayClientProfile::RootOrigin` conserve
l'accès direct au gateway. Aucun profil ne raccorde le canal runtime, ne conserve
le binding worker/profil ou ne fournit son owner broker/outbox. Le module
`agentsandbox::relay::channel` fournit frames fermées bornées et validation d'état.
[Preuve logicielle attachée aux sources](../../engineering/evidence/2026-11-07/relay-transport-contracts/README.md).

## Admission par le contrôle de confiance

L'[API d'admission](../../api.md#admission-du-canal-runtime--runtime-channel-admission)
réservée au runner réserve un canal, consomme un claim exec distinct et unique,
puis conserve ses faits de cycle. Les [DTO fermés](../../../src/relay/admission.rs)
lient worker, tentative, génération, binding workload, Arm initiale et expiration.
Le runner authentifié sélectionne le tenant. Les credentials worker, présence
broker et humains ne donnent aucun accès à ces routes ; la preuve workload reste
dans le contrôle de confiance.

Une réservation ne permet aucun exec. Seule une réponse ClaimExec fraîche et
validée peut produire la capability du SDK interne ; un rejeu ne la recrée pas.
Expiration ou révocation refusent une nouvelle autorité, en conservant le cleanup
sûr. Un canal claimé inconnu reste retenu jusqu'à preuve terminale suffisante ;
EOF local et fin du CLI ne libèrent pas le quota worker. Le journal borne
actuellement l'admission à 100 endpoints actifs et 4096 faits par tenant ; son
épuisement refuse l'admission sans purge automatique des preuves. Rétention et
compaction restent à développer ; conserver les preuves et respecter les limites du
[contrat d'admission](../../engineering/RELAY_ADMISSION.md).

### Rapprocher une opération de contrôle incertaine

Le contrôle de confiance peut appeler le SDK interne
`reconcile_relay_channel(&RelayChannelReconcileV1)` pour lire les faits
historiques d'un canal. Il emploie
`POST /control/api/v1/relay/channels/{channel_id}/reconcile` avec l'admission
exacte conservée et, s'il existe, le claim ou la transition pendante exacte.
Le pending garde son UUID d'opération, son type, son contenu/hash canonique et
sa révision attendue. Ces champs servent à comparer les identités ; ils ne
demandent aucune mutation. L'UUID du chemin est canonique, non nul et égal au
canal de l'admission. Aucune query ni alias non versionné n'est accepté.

Le SDK ne prend aucun tenant en argument. Il obtient tenant, namespace et rôle
runner depuis l'identité de supervision authentifiée, puis recoupe la réponse
avec l'admission conservée. Le serveur résout le tenant depuis le credential
runner et contrôle le namespace dans la transaction vérifiée, y compris si
l'admission est absente. La lecture historique reste accessible après terminal
du worker, expiration/révocation du workload ou désactivation de l'admission du
namespace. Le runner doit rester authentifié et autorisé ; un mode d'admission
inactif ne contourne pas une authentification révoquée. Les credentials worker,
viewer, présence broker et humains n'acquièrent aucun accès.

Une réponse HTTP 200 valide est un `RelayChannelReconciliationV1` contenant
`version`, le `tenant_id` authentifié, `control_namespace_uuid`,
`admission_input_hash`, `audit_seq`, `audit_hash` et un `outcome` fermé.
Le pending de la requête emploie `kind:claim|transition` et `request` ; les
outcomes et résolutions du pending emploient `kind`. Pour une admission exacte,
`historical` contient le snapshot historique courant et `admission_receipt`,
séparés du pending exact et de sa résolution :

| Résultat du pending | Sens et action de reprise |
|---|---|
| `recorded` | Le type, l'UUID d'opération et le contenu exacts sont enregistrés ; conserver son reçu réel. |
| `inapplicable_cas` | Le pending exact est absent et le snapshot vérifié du même canal porte une révision strictement supérieure à sa révision attendue. Son ancien CAS ne peut plus committer. Persister cette résolution déterministe avant de remplacer l'intention. |
| `inapplicable_state` | Le pending exact est absent, sa révision CAS égale celle du snapshot et les règles partagées des mutations l'interdisent dans cet état. Tout changement d'état augmente la révision : cette intention exacte ne pourra jamais s'appliquer. Persister la résolution avant de la remplacer. |
| `unresolved` | Aucun reçu ni preuve déterministe d'inapplicabilité n'est disponible. Conserver l'intention et ses ressources. |

Par exemple, une ancienne `CloseExactWorkerTerminal` avec révision attendue 1
face à `Reserved`/révision 1 produit `inapplicable_state`, même avec le terminal
worker exact. Close est interdit dans Reserved ; atteindre un état compatible
augmente la révision et invalide cet ancien CAS. La lecture recherche d'abord
le reçu exact et ne fabrique ni reçu de refus ni remplacement de l'intention.

Un UUID d'opération enregistré avec un autre type ou contenu est un conflit.
Une admission absente produit ordinairement `admission_unresolved` : l'absence
au snapshot observé laisse le propriétaire `Unknown` et ne prouve pas qu'un
ancien Reserve ne peut plus committer.

L'outcome plus fort `admission_never_recorded_exact_terminal` exige l'absence de
l'admission exacte et de faits de canal/opération divergents, ainsi que le
terminal durable exact du même worker, tentative, session, inspection et
StartedAt, tous vérifiés dans la même transaction sérialisée. Il porte
`admission_operation_id`, `channel_id`, `epoch`, `endpoint`, `worker_revision`
et `terminal_observation_receipt` : le vrai `MutationReceiptV1` de l'observation
terminale. Aucun reçu de canal n'est créé. Une Reserve ultérieure ne peut plus
satisfaire son exigence Running et Observe ne peut pas réanimer un worker
terminal. Une preuve terminale incomplète ou divergente produit
`admission_unresolved`. Cet outcome ne prouve aucune jonction locale ni
fermeture de transport.

Un 404, une route non prise en charge, une réponse perdue, un refus
d'authentification, une erreur d'audit ou une réponse invalide ne fournit aucune
preuve de clôture. Conserver l'opération initiale et résoudre la cause sans
changer d'identité ni rejouer Reserve ou Claim comme moyen de consultation.

La réponse lie ces faits à la séquence/hash réels de tête d'audit de la même
transaction vérifiée. Cette tête est un repère de snapshot, pas un nouveau reçu
de mutation ni une signature. Cette lecture technique étroite de métadonnées
suit `worker_operation` : elle reconstruit les faits existants sans ajouter de
fait de canal, claim ou reçu d'accès. Ce régime de lecture ne s'applique ni aux
lectures humaines ni à l'inbox. Les corps de requête et de réponse sont bornés
à 16 Kio. Ce port ne construit aucune capability exec,
n'ouvre aucun transport, ne renouvelle aucun délai, ne libère aucun quota et ne
prouve aucune jonction de tâches locales. Les contrôles d'admission, claim et
dispatch actifs restent applicables. Le journal local conserve l'intention
initiale et persiste son reçu exact ou sa résolution admissible avant de
préparer un remplacement ; cette lecture ne migre pas ce journal.

Le raccord broker/runtime est une étape d'intégration distincte. Aucun nouveau
CLI relais ou canal exec automatiquement activé n'est fourni. La
[validation attachée aux sources](../../engineering/evidence/2026-11-07/relay-admission/README.md)
distingue ce qui a été exercé des écarts encore ouverts.

## Fournir un contexte autorisé

L'appelant fournit une origine gateway approuvée, la preuve workload exclusive
du worker et un `RelayEndpointV1` attendu (`worker_id`, `attempt_id`,
`generation:1`). Les UUID doivent être canoniques. L'endpoint attendu contrôle
les réponses d'inbox ; il ne choisit aucun tenant et n'établit pas l'identité
source. Le gateway authentifie la preuve et revérifie l'autorité courante à
chaque opération.

Le contrôle de confiance provisionne bindings et grants dirigés selon le
[guide workload](../workloads/README.fr.md#autoriser-des-messages-dirigés).
Un grant autorise une source → destination + canal ; le sens inverse exige son
propre grant. Les deux endpoints doivent conserver leurs bindings valides et
l'état supervisé requis. Credentials humains/SSO, runner, broker et fournisseur
IA restent distincts. Fournir une preuve runner au constructeur ne la transforme
pas en preuve worker.

Transmettre uniquement le credential workload nécessaire par le handoff privé
approuvé. Ne le placer ni dans le code, les argv, messages, URL, logs, exemples,
ni dans un fichier d'environnement complet. Le client reçoit une chaîne de
32–512 octets ASCII sans blanc/caractère de contrôle ; il ne lit ni fichier de
credential ni cache. Il n'implémente pas `Debug` et ses erreurs fermées ne
conservent ni texte distant, URL, corps ou chaîne d'erreur contenant la preuve.
Cela ne protège pas un appelant qui imprime ses propres entrées.

`new(origin, workload_credential, expected_endpoint)` emploie les racines TLS
normales et choisit `RootOrigin`, comme le constructeur historique `with_ca_file`.
Utiliser `with_profile(origin, workload_credential, expected_endpoint, profile)`
pour une sélection explicite. Le profil est un enum fermé, sans préfixe libre
ni repli automatique. Lors du raccord runtime, l'owner devra conserver ce choix
avec le binding autorisé ; la bibliothèque ne le fait pas.
L'origine est limitée à 2048 octets, sans information utilisateur,
chemin autre que la racine, query, fragment, échappement `%` ou antislash.
HTTPS est accepté ; HTTP clair est limité à une IP loopback littérale, jamais
`localhost` ni une adresse LAN privée. L'origine ne doit pas contenir le
credential workload. Redirections, proxies d'environnement et retries
automatiques sont désactivés.

Pour des racines privées explicitement approuvées, utiliser
`with_ca_file(origin, workload_credential, expected_endpoint, ca_file)` avec
HTTPS. Le lecteur existant de fichiers projetés de confiance charge au plus
64 Kio et huit certificats publics. Ce snapshot immuable remplace les racines
intégrées pour ce seul client ; une édition du fichier ne le recharge pas.
La vérification de chaîne/nom reste active. Le client ne fournit ni clé privée
d'AC, installation de confiance système, repli HTTP ni configuration de
certificat client/mTLS. Lire le [point d'entrée PKI](../../engineering/INFRASTRUCTURE_PKI.md)
avant une opération distante réelle. Ce lot ne qualifie pas un nouvel essai
réel HTTPS/AC privée avec ce client.
Pour associer profil explicite et racines privées, utiliser
`with_profile_and_ca_file(origin, workload_credential, expected_endpoint, profile, ca_file)`.
Choisir route et confiance approuvées avant construction ; ne pas changer de
profil pour reprendre une mutation dont le résultat est incertain.

## Employer des requêtes conservées

Ce fragment d'intégration reçoit toute son autorité et la requête déjà conservée
depuis son appelant. Il n'obtient pas de credential et ne prépare aucun runtime :

```rust
use agentsandbox::relay::{
    RelayEndpointV1, RelayReceiptV1, RelaySendV1,
    client::{RelayClientError, RelayClientProfile, WorkerRelayClient},
};

async fn send_prepared(
    approved_origin: &str,
    workload_credential: &str,
    expected_endpoint: RelayEndpointV1,
    prepared_send: &RelaySendV1,
) -> Result<RelayReceiptV1, RelayClientError> {
    let client = WorkerRelayClient::with_profile(
        approved_origin, workload_credential, expected_endpoint,
        RelayClientProfile::ControlIngress,
    )?;
    client.send(prepared_send).await
}
```

| Méthode | Route worker existante et comportement |
|---|---|
| `send(&RelaySendV1)` | `POST /api/v1/relay/messages` ; version 1, UUID d'opération conservé, UUID de grant autorisé, TTL et corps non vide. Renvoie le reçu corrélé : 201 pour un nouvel envoi ou 200 au rejeu exact. L'ID de l'opération send identifie aussi son message. |
| `send_with_refusal(&RelaySendV1)` | Même requête, même route et mêmes contrôles que `send`. Son échec est un `RelaySendFailureV1` : `error` est la valeur rendue par `send`, et `refusal` ce qu'a dit un [refus typé du relais](#lire-un-envoi-refusé), lorsque la réponse en était un. |
| `inbox(after: Option<i64>, limit: u32)` | `GET /api/v1/relay/messages` ; curseur non négatif et limite 1–32. Renvoie une seule page vérifiée ; une erreur ne devient jamais une inbox vide. L'appelant conserve et avance explicitement son curseur. |
| `ack(&RelayAckV1)` | `POST /api/v1/relay/messages/{message_id}/ack` ; l'ACK conservé possède son propre UUID d'opération et l'UUID du message reçu. Renvoie un reçu 200 corrélé. L'ACK atteste un acquittement, pas une exécution ou un effet observé. |

Le tableau décrit `RootOrigin`. `ControlIngress` ajoute le préfixe fixe
`/control` aux trois routes, query d'inbox et ACK compris. Cela ne change ni
credential, rôle, endpoint, DTO, TLS, délai ou sémantique de résultat.

Aucune méthode ne gère grant, révocation, purge, rôle runner, URL arbitraire ou
sélecteur de tenant. Le contenu de l'inbox est une donnée métier, pas une
instruction à exécuter. Le client ne l'acquitte et ne l'exécute pas
automatiquement. Le reçu est corrélé et sa structure vérifiée ; son hash ne
constitue pas une vérification indépendante de toute la chaîne d'audit par le
client.

Conserver durablement la requête exacte et son ID d'opération **avant** l'appel
à `send` ou `ack`. La bibliothèque ne fournit ni journal, nouvel ID d'opération,
retry automatique ou daemon de réconciliation. Si `error.outcome_unknown()`
vaut vrai (`Transport`, `Protocol` ou `Unavailable`), une mutation peut déjà
être engagée. Préserver sa requête puis reprendre explicitement cette même
opération et ce même contenu après résolution de la cause. Le rejeu revérifie
preuve, endpoints et grant courants ; expiration/révocation peuvent donc le
bloquer. Ne pas remplacer une requête incertaine par un nouvel ID, ni prendre
`outcome_unknown()` faux pour une preuve de rollback.

## Bornes, erreurs et audit

| Frontière | Limite |
|---|---|
| Connexion / lecture de réponse / échange total | 3 s / 5 s / 12 s |
| Corps non vide avant échappement JSON / requête encodée | 65536 octets / 262144 octets |
| TTL du message | 1–3600 s, également borné par le grant côté serveur |
| Reçu / réponse inbox encodée | 16384 octets / 2 Mio |
| Headers de réponse contrôlés | Au plus 64 champs et 32768 octets de noms+valeurs |
| Décodage JSON | Clés uniques récursivement, profondeur bornée à 64, forme V1 fermée et complète ; `next_after` doit être présent, y compris explicitement `null` |

Le serveur continue d'appliquer limites durables de débit par grant, de contenu
retenu par tenant, par émetteur et par grant dirigé
([budgets de rétention](RETENTION.fr.md)) et budgets d'audit obligatoires du
[contrat relais](../workloads/README.fr.md#autoriser-des-messages-dirigés).
Une reconnexion ne les réinitialise pas. Les lectures d'inbox, même vides, sont
auditées avant livraison du contenu. Une panne d'audit bloque l'opération.
Une page ne constitue ni inventaire complet du tenant ni snapshot stable entre
plusieurs pages.

Le client refuse son credential workload connu dans tout le DTO send/ACK et
l'URL finale construite avant transport, chemins fixes du profil et query compris.
Ce dernier refus local est un `InvalidRequest` opaque ; il ne résout pas le
résultat d'une tentative antérieure. Il contrôle aussi chaînes/clés
du corps structuré décodé et les réponses. Un corps JSON que le décodeur normal
peut lire mais que le décodeur strict refuse — doublons de clés ou profondeur
supérieure à 64, par exemple — est refusé **même sans secret**. Le texte ordinaire
non JSON reste accepté. Ce contrôle littéral/structuré ne constitue ni détection
générale des secrets ni DLP parfaite ; ne pas envoyer de credentials ou tokens
sensibles dans les données métier.

`InvalidConfiguration` concerne origine/preuve/endpoint/confiance locaux.
`InvalidRequest` peut être un refus local ou HTTP 400/413. `Authentication` et
`Forbidden` demandent de corriger l'autorité worker avec l'opérateur ; ils ne
déclenchent aucun autre rôle ou credential. `NotFound` et `Conflict` demandent de
préserver et vérifier l'identité de la ressource/requête. `Capacity` couvre
HTTP 429, y compris les budgets d'audit ; contacter l'opérateur plutôt que tourner
une preuve pour réinitialiser un budget, sauf si un
[refus typé du relais](#lire-un-envoi-refusé) indique qu'un nouvel essai peut
passer. `Unavailable`, `Transport` et `Protocol`
laissent le résultat d'une mutation incertain. Aucun texte d'erreur ne reprend
le diagnostic distant. Les [diagnostics workload](../workloads/README.fr.md#diagnostiquer-arrêter-et-préserver-létat)
conservent les distinctions côté serveur.

### Lire un envoi refusé

Un envoi au-delà d'une part retenue ou de la fréquence de son grant reçoit un
HTTP 429 typé ([budgets de rétention](RETENTION.fr.md#refus)). `send` le
rapporte comme `Capacity`, comme tout autre 429. `send_with_refusal` rapporte
le même `error` et ajoute un `RelaySendRefusalV1` dans `refusal` :

| Champ | Contenu |
|---|---|
| `scope` | `Tenant`, `Sender` ou `Grant` (`RelayRefusalScopeV1`) |
| `retry_action` | `RetryAfterRelease`, `RetryAfterWindow` ou `ContactOperator` (`RelayRetryActionV1`) |
| `own` | Pour une part émetteur ou grant seulement : `dimension`, `limit`, `retained`, `requested_bytes` et `next_release_at`, qui décrivent tous l'émetteur refusé lui-même. `None` pour un refus tenant ou de fréquence. |

```rust
use agentsandbox::relay::{
    RelayReceiptV1, RelayRetryActionV1, RelaySendFailureV1, RelaySendV1,
    client::WorkerRelayClient,
};

/// `Ok(None)` : garder la requête conservée et réessayer cette même opération plus tard.
async fn send_or_keep(
    client: &WorkerRelayClient,
    prepared_send: &RelaySendV1,
) -> Result<Option<RelayReceiptV1>, RelaySendFailureV1> {
    match client.send_with_refusal(prepared_send).await {
        Ok(receipt) => Ok(Some(receipt)),
        Err(failure) => match failure.refusal.as_ref().map(|refusal| refusal.retry_action) {
            Some(RelayRetryActionV1::RetryAfterRelease | RelayRetryActionV1::RetryAfterWindow) => {
                Ok(None)
            }
            // `ContactOperator`, ou un échec que le relais n'a pas typé.
            _ => Err(failure),
        },
    }
}
```

- `refusal` vaut `None` pour tout échec qui n'est pas un refus publié du
  relais : budget d'audit, porte d'admission d'E/S, tout autre statut. Rien
  n'est déduit d'une telle réponse.
- Le client ne lit que les champs fermés ci-dessus. Il ne rend jamais le texte
  du message distant et n'accepte pas un refus tenant ou de fréquence
  accompagné de chiffres : ils décriraient d'autres émetteurs.
- La bibliothèque ne réessaie toujours rien. Un envoi refusé n'a pas été
  stocké : conserver la requête exacte et réessayer cette même opération,
  comme décrit dans le [guide de rétention](RETENTION.fr.md#refus).
- Un envoi stdio V1 garde le simple `capacity`. L'enveloppe V2 réservée aux
  envois et explicitement choisie ci-dessous peut transporter le même refus
  fermé par un helper et un contrôle compatibles. Ce nouveau raccord logiciel
  reste source-only jusqu'à ses exécutions QA identifiées.

### Choisir les détails de refus d'envoi sur stdio

Conserver hello/ready V1 validés, version du helper, découverte, canal, epoch et
limites résolues. Pour un envoi, passer seulement la **version externe de la
requête** à `2` ; `RequestPayloadV1::Send(prepared_send.clone())` conserve le
payload V1 exact et l'UUID d'opération retenu. Les types Rust existants
`RequestV1`/`FrameV1` représentent cette extension explicite d'échange. Une
nouvelle corrélation par échange explicite ne remplace pas l'UUID métier.

La réponse doit porter les mêmes version/canal/epoch/corrélation/opération.
Seule l'erreur `capacity` d'un envoi V2 peut ajouter `refusal`, contenant
exactement `scope`, `retry_action` et `own`. Les chiffres propres sont requis
pour la capacité émetteur/grant, et `null` explicite pour tenant/fréquence du
grant. Champs inconnus/dupliqués/manquants, combinaisons portée/action invalides
et échéances non canoniques sont refusés. Le texte distant est écarté. Un
refus générique d'audit/E/S/autre capacité reste `capacity` sans détail déduit.
Voir le [schéma documentaire d'échange V2](SEND-REFUSAL.schema.json) ; Rust
conserve les contrôles d'octets, identité, reçus et trames qu'un schéma JSON ne
peut établir seul.

| Enveloppe application | Helper/contrôle | Résultat |
|---|---|---|
| Envoi V1 | Anciens ou nouveaux | Réponse stricte existante, aucun champ de refus ajouté |
| Envoi V2 | Tous deux compatibles avec cette extension | Réponse V2 corrélée, détail fermé seulement s'il est publié |
| Envoi V2 | Au moins un pair antérieur à l'extension | Protocole incompatible, aucun fallback ni second dispatch automatique |
| Inbox/ACK/hello/ready/close/découverte V2 | Nouveau | Refus ; ces contrats restent V1 |

Un ancien helper ne possède que le diagnostic fixe `relay_helper:protocol`,
aucun code rétroactif `unsupported_version`. Une panne de connexion n'est ni
un reçu ni une preuve de rollback : après transfert, conserver tout résultat
incertain et réconcilier l'opération exacte retenue. Un refus de capacité
ultérieur n'efface pas un dispatch antérieur inconnu. Cette extension n'ajoute
ni retry, pruning, rotation de secret, autorité worker, transport runtime ou
option CLI applicative.

Le wire V1 reste compatible. Un consommateur Rust qui met la bibliothèque à
jour doit traiter le nouveau variant `ResponseResultV1::SendRefusal` dans ses
matchs exhaustifs ; `ErrorCodeV1` et ses sens existants restent inchangés.

Les [preuves des profils](../../engineering/evidence/2026-11-07/relay-control-profile/README.md)
consignent six nouveaux tests de profil et treize tests client existants passant
sur leur source figée, avec HTTP/SQLite réels et enregistrements synthétiques de
workers supervisés. La [preuve antérieure](../../engineering/evidence/2026-11-07/relay-client/README.md)
conserve sa propre source et ses résultats. Handoff runtime réel,
workers Docker/Podman/Kubernetes, transport multi-hôtes, exécution fournisseur et
cible 100 workers ne sont pas qualifiés par ces tests.
