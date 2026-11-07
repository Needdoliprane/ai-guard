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
retenu par tenant et budgets d'audit obligatoires du
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
une preuve pour réinitialiser un budget. `Unavailable`, `Transport` et `Protocol`
laissent le résultat d'une mutation incertain. Aucun texte d'erreur ne reprend
le diagnostic distant. Les [diagnostics workload](../workloads/README.fr.md#diagnostiquer-arrêter-et-préserver-létat)
conservent les distinctions côté serveur.

Les [preuves des profils](../../engineering/evidence/2026-11-07/relay-control-profile/README.md)
consignent six nouveaux tests de profil et treize tests client existants passant
sur leur source figée, avec HTTP/SQLite réels et enregistrements synthétiques de
workers supervisés. La [preuve antérieure](../../engineering/evidence/2026-11-07/relay-client/README.md)
conserve sa propre source et ses résultats. Handoff runtime réel,
workers Docker/Podman/Kubernetes, transport multi-hôtes, exécution fournisseur et
cible 100 workers ne sont pas qualifiés par ces tests.
