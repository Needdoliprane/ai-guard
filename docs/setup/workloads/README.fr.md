# Identités workload, registre des brokers et relais dirigé

[English](README.md) · [Index de configuration](../README.fr.md) · [Broker local](../broker/README.fr.md) · [Accès nominatif](../identity/README.fr.md) · [SSO](../sso/README.fr.md)

Ce guide décrit les contrats HTTP techniques du gateway Rust pour les credentials workload, les déclarations de brokers et les messages dirigés entre workers. Ils utilisent une base SQLite existante distincte par tenant. Ces endpoints ne démarrent pas de worker et ne connectent pas de broker distant. Les workers conservent `network: "none"` ; le transport privé Unix/runtime qui les raccorderait au relais n'est ni raccordé ni qualifié. Ne pas joindre un worker au réseau du contrôle pour rendre ces exemples accessibles.

Consulter [l'état constaté](../../engineering/CURRENT_STATE.md), [la validation](../../engineering/VALIDATION.md) et [les gates](../../engineering/PRODUCTION_READINESS.md) pour la qualification. Le backend HTTP seul ne démontre ni la recette à trois workers A1/A2/B1, ni la fédération, le fonctionnement multi-hôtes, une administration humaine complète ou une capacité mesurée de 100 workers.

## Préparer le stockage sans activer l'exécution

Utiliser le binaire de contrôle relu, la base existante du tenant et son compte privé de contrôle. Arrêter le gateway, tous les brokers et les autres consommateurs de la base, y compris les redémarrages automatiques. Préserver leurs ancres runtime/enforcement ; un gateway arrêté ne prouve pas qu'un worker est arrêté. `--confirm-control-stopped` est une attestation opérateur, pas un mécanisme de fencing runtime.

| Schéma | Frontière des lecteurs et fonctions |
|---|---|
| 2 | Contrôle local standard ; incompatible avec le contrôle de supervision du broker. |
| 3 | Supervision ; lecteur du protocole broker compatible. |
| 4 | Stockage des identités humaines ; lecteur du protocole broker compatible. |
| 5 | Audit obligatoire des lectures humaines ; lecteur du protocole broker compatible. |
| 6 | Bindings workload, registre des brokers, relais et plancher UTC durable ; lecteur du protocole broker compatible. Ces contrats workload exigent cette migration explicite. |
| 7 | Projection nominative distincte du registre brokers et marqueur de capacité d’audit ;6→7 explicite, sans grant ou activation automatique. |

La compatibilité broker exige aussi namespace, tenant, rôle runner et `broker_supervision_v1` configurés. Submit, approve, resume et export frais exigent une supervision active. **Migrer ou rouvrir une base ne l'active pas.** Une réouverture ordinaire des schémas 3–7 reste en maintenance inactive ; aucune procédure de réactivation sûre n'est livrée. Ne pas éditer les métadonnées, supprimer le stockage ou réutiliser l'initialisation exclusive d'une base neuve pour contourner cette condition. Voir [contrôle actif et reprise](../broker/README.fr.md#exiger-un-contrôle-compatible-et-actif).

La maintenance humaine hors ligne (`human provision`, `human resume-provision` et `human apply`) accepte une base existante validée de schéma 4–7 avec supervision inactive. Après la migration explicite 5→6, utiliser le même système de fichiers privé du contrôle, le binding du tenant et l'attestation `--confirm-control-stopped`. Ces commandes ne créent ni ne migrent la base et ne réactivent pas la supervision. Préserver credentials privés et fichiers de reprise ; reprendre uniquement la préparation exacte conservée, sans renouveler son credential ni écraser ses fichiers. Suivre [la création du compte et sa reprise](../identity/README.fr.md#créer-le-compte-et-conserver-sa-reprise) et conserver la même politique de capacité d'audit entre consommateurs.

Pour une base existante de schéma 5, remplacer chaque référence d'installation ci-dessous. La destination de sauvegarde doit être neuve ; son parent privé doit déjà exister. La migration vérifie une sauvegarde et avance d'une seule version. Sa cible par défaut reste 2 : indiquer explicitement 6 :

```sh
"/absolute/path/to/agentsandbox" migrate-control \
  --data "/absolute/private/ai-guard/audit.sqlite" \
  --tenant-id "<configured-tenant-id>" \
  --backup "/absolute/private/ai-guard/before-workloads-v6.sqlite" \
  --target-version 6 --max-events 100000 --confirm-control-stopped
```

Pour les sources plus anciennes, appliquer chaque migration suivante avec une sauvegarde neuve distincte : 3→4→5→6, suivant [la maintenance hors ligne](../../operations.md#répertoire-sqlite-privé). Mettre tous les lecteurs à jour ensemble ; les anciens binaires refusent les nouveaux schémas. Préserver binding du tenant, namespace, événements, claims consommés, credentials et sauvegardes vérifiées. Une édition SQLite de `user_version` ne remplace jamais une migration. Pour le stockage appartenant à un conteneur, utiliser l'UID/GID du contrôle existant et son volume existant inspecté, autres consommateurs arrêtés ; le [guide d'identité](../identity/README.fr.md#préparer-le-stockage-et-lautorité) explique cette frontière de maintenance. Ne pas deviner un nom de volume ni initialiser une base de remplacement.

## Activer la lecture nominative du registre brokers

Le schéma7 ajoute la projection humaine autorisée séparément de ce registre existant. Elle utilise la **même base SQLite du tenant**, les mêmes brokers et la capacité d'audit humain ; elle n'ajoute ni transport worker ni enregistrement automatique de broker. Mettre ensemble gateway, lecteurs broker/STOP, clients natifs et dashboard compilé à la version acceptant7. Ces lecteurs conservent les contrats historiques5/6 ; les anciens binaires ignorant7 ne peuvent pas le rouvrir. Migration, valeurs par défaut et rejeu d'ancienne préparation n'accordent pas la nouvelle capacité.

Une base issue de `setup prepare` est en5 : appliquer d'abord la commande explicite5→6 ci-dessus, vérifier son rapport de succès et conserver sa sauvegarde vérifiée du schéma5. Puis, tous consommateurs toujours arrêtés, migrer **la même** base6→7. Remplacer chemins/tenant et utiliser le véritable `--max-events` de l'installation ; ne pas rejouer5→6 sur une base7 :

```sh
"/absolute/path/to/agentsandbox" migrate-control \
  --data "/absolute/private/ai-guard/audit.sqlite" \
  --tenant-id "<tenant-configure>" \
  --backup "/absolute/private/ai-guard/before-human-brokers-v7.sqlite" \
  --target-version 7 --max-events 100000 --confirm-control-stopped
```

Pour une base appartenant au conteneur, suivre la [frontière hors ligne du guide identité](../identity/README.fr.md#préparer-le-stockage-et-lautorité) : UID/GID de contrôle existant, volume existant inspecté, aucun réseau/socket, image correspondante et autres consommateurs arrêtés. `/data/installation/tenants/<tenant>/audit.sqlite` concerne un volume SSO préparé au lieu du `/data/audit.sqlite` historique ; inspecter et conserver l'installation utilisée. Chaque sauvegarde utilise un nouveau chemin privé sur ce stockage. Ne pas remplacer le volume ou deviner son nom depuis cet exemple.

La migration crée et vérifie une sauvegarde neuve du6 contre sa tête d'audit exacte, puis ajoute atomiquement le marqueur chaîné `control.identity.brokers_access.genesis` et passe au7. Elle n'ajoute ni table/index, ne vide aucun budget, ne réécrit aucun grant, ne délivre aucun credential et n'active aucune exécution. Un échec de la transaction finale laisse version/tête source inchangées ; une sauvegarde créée peut subsister et doit être conservée. Vérifier le code de sortie et `from_version:6`, `to_version:7`, `backup_path`, `genesis_seq`, `genesis_hash` du rapport de succès avant de poursuivre. Aucune restauration/downgrade automatique n'est fournie. Ne pas éditer `user_version`, marqueurs, reçus ou projections pour contourner un refus.

La réouverture à froid ordinaire reste en maintenance inactive au7. Nouvelles admissions, arms et claims restent refusés ; cette procédure ne réactive pas la supervision et ne contourne aucun écart de fencing/reprise. Conserver ancres runtime/enforcement et claims consommés. Rouvrir pour lire ne donne aucun droit de lancer un worker de remplacement.

Ensuite [accorder explicitement `read_broker_registry`](../identity/README.fr.md#autoriser-la-lecture-nominative-du-registre-brokers) depuis le contrôle de confiance/hors ligne. OIDC exige aussi le [plafond sur claims vérifiés](../sso/README.fr.md#activer-explicitement-la-lecture-du-registre-brokers). `setup prepare` prépare toujours5 et refuse cette capacité avant publication : ne pas l'ajouter au manifeste setup. Conserver `--max-events` et la politique voulue identiques entre gateway et consommateurs d'identité hors ligne. `migrate-control` n'a pas d'option `--audit-capacity-file` ; cette option appartient au gateway et aux commandes d'identité hors ligne. La migration consomme l'audit d'identité obligatoire et ne contourne pas un journal plein.

La route humaine est `GET /api/v1/human/brokers`, avec preuve locale/OIDC admise et contexte administration. Elle diffère de `GET /api/v1/brokers` technique : aucun grant brut ou ID/digest de credential ne va au navigateur. Le [contrat API](../../api.md#registre-brokers-humain--human-broker-registry) définit query/réponse fermées. Les lignes apparaissent seulement après un véritable enregistrement autorisé du contrôle ; utiliser ci-dessous les contrats issue/grant/heartbeat pour le broker réel concerné. Ne pas publier d'observations de fixture ou d'événements d'audit synthétiques pour remplir le dashboard. Cette vue ne fournit ni inventaire runtime, ni fédération, ni qualification production.

## Fixer un budget d'audit workload borné

[workload-audit.json](examples/workload-audit.json) est un exemple V1 public sans secret, pour `--max-events 100000` avec les réserves contrôle/identité par défaut. Le copier dans un fichier régulier privé **neuf**, mode 0600 sous un parent de contrôle privé 0700, hors des arbres source/export. Le fichier de politique est limité à 16 Kio et chargé au démarrage ; son édition n'est pas un rechargement à chaud. Pour le gateway local mono-tenant, ajouter `--workload-audit-file /absolute/private/ai-guard/workload-audit.json` à sa commande de lancement existante. C'est une option gateway, pas un champ du registre broker ou un sélecteur d'environnement.

Cette option est incompatible avec `--tenants-file`. Avec un registre de tenants, placer plutôt le même objet JSON dans le champ facultatif `workload_audit_policy` de l'entrée concernée. Conserver ses `data`, références de credentials, quota et éventuel `audit_capacity_policy` existants ; ni un exemple public ni un corps de requête ne choisissent le tenant. Utiliser les mêmes `--max-events` et politiques de capacité prévues entre consommateurs.

| Budget de l'exemple fourni | Événements | Octets de payload d'audit préparé |
|---|---:|---:|
| Ensemble de l'audit workload du tenant | 25000 | 67108864 (64 Mio) |
| Un principal broker ou worker | 6250 | 16777216 (16 Mio) |
| Réserve contrôle existante par défaut | 25000 | 67108864 (64 Mio) |
| Réserve identité existante par défaut | 12500 | 33554432 (32 Mio) |

Les deux dimensions s'appliquent. Un budget principal doit être positif et strictement inférieur au budget tenant. Le budget workload du tenant doit tenir dans la capacité physique après les réserves contrôle et identité existantes. La capacité physique de payload préparé est de 256 Mio ; `--max-events` borne les événements. Ce sont des contrôles d'occupation partagée, pas des partitions de stockage indépendantes : les événements ordinaires et les lectures humaines déjà enregistrés consomment aussi de la place. L'exemple ne prouve ni capacité libre ni enregistrement possible d'une opération.

L'usage workload est cumulatif et durable, indexé par ID broker ou worker dans son tenant. Heartbeats, envois, ACK et reçus de lecture d'inbox consomment leur budget workload applicable ; une lecture d'inbox vide enregistre aussi un reçu. Rotation, nouveau credential, reconnexion, redémarrage ou changement de politique ne réinitialisent pas l'usage de ce principal. Les valeurs par défaut sont dérivées des réserves existantes ; une capacité automatique insuffisante désactive explicitement les nouvelles admissions d'audit workload. Les diagnostics de démarrage donnent politique résolue, source, empreinte et état du budget d'admission, sans promettre une capacité restante.

Sur `429 audit_workload_budget_exhausted` avec `retry_action:"contact_operator"`, préserver base, politique et reçus, puis examiner l'occupation. Les retries périodiques et la rotation ne rétablissent pas ce budget cumulatif. Une augmentation délibérée de politique aide seulement dans les limites physiques et des réserves protégées. Aucun archivage automatique, purge ou procédure d'archivage qualifiée n'est fourni. Ne pas supprimer des événements d'audit pour libérer de la place. L'épuisement physique ou l'indisponibilité du stockage obligatoire reste une erreur.

## Séparer quatre rôles techniques

| Preuve | Périmètre autorisé ici |
|---|---|
| `runner` | Contrôle de confiance uniquement : émettre/tourner/révoquer les bindings, gérer grants broker et relais dirigé, purger les corps de messages expirés. |
| `viewer` | Lire les métadonnées des brokers de son tenant authentifié ; aucune administration workload ni lecture des corps du relais. Le runner peut aussi utiliser cette route technique de lecture. |
| Workload `broker` | Lire sa propre entrée et transmettre son heartbeat authentifié avec binding et grant courants. Aucun droit runner, registre global ou message worker. |
| Workload `worker` | Envoyer avec un grant explicite source→destination/canal, lire son inbox et acquitter ses messages. Aucun accès au registre de contrôle ou aux métriques globales. |

Le gateway déduit tenant et rôle de la preuve Bearer présentée. Aucun header libre de tenant, sélecteur query ou `tenant_id` n'existe dans ces DTO workload. Les IDs JSON nomment des ressources dans un périmètre déjà authentifié ; ils ne prouvent pas l'identité. Un Bearer worker sur une route runner n'acquiert aucun droit runner.

Credentials humains locaux et sessions OIDC restent distincts, comme les credentials fournisseurs IA. Ni un administrateur IdP ni un rôle applicatif ai-guard ne créent de credential runner ou workload. Ces API n'établissent ni initiateur humain nominatif ni chaîne complète authentifiée agent/délégation. Suivre [l'accès nominatif](../identity/README.fr.md) ou [le SSO](../sso/README.fr.md) pour les accès humains.

## Préparer et enregistrer une opération de credential par CLI

Utilisez la CLI opérateur Rust depuis le périmètre de contrôle de confiance.
Construisez le binaire `agentsandbox` courant, préparez le schéma6 ou7
comme ci-dessus et conservez le credential runner courant dans un fichier
privé. La commande n'obtient pas de droit runner depuis un compte humain ou
un rôle IdP.
Le fichier runner suit le profil CLI gateway courant :32–512octets sans
blanc, avec au plus un LF final. L'opération ne remplace ni ne régénère ce
credential runner existant.

Copiez le [modèle de contrôle](examples/cli-control.json) et un modèle de
requête dans un répertoire privé de l'opérateur. Ces exemples sont des
entrées à configurer, pas des identités installées : remplacez tous les
`<...>` et origine/chemins d'exemple. Utilisez le tenant authentifié et le
namespace de contrôle réels du profil d'installation approuvé. Ce sont des
gardes d'égalité ; le serveur choisit toujours le tenant depuis le runner.

| Modèle de requête | Valeurs à préparer |
|---|---|
| [Émission](examples/cli-issue.json) | Broker/incarnation approuvés, expiry future, révision0. La CLI génère une seule fois l'ID du credential et le secret. |
| [Rotation](examples/cli-rotate.json) | Même ID de credential et sujet, révision courante explicite et nouvelle expiry future. Un nouveau secret est généré. |
| [Révocation](examples/cli-revoke.json) | ID du credential et révision courante explicite. Aucun nouveau secret n'est généré. |

Pour un sujet worker, utilisez ses `worker_id`, `attempt_id` et
`generation:1` approuvés sous `kind:worker`, conformément à la tentative
runtime déjà autorisée. Un ID fourni n'établit pas cette tentative et ne
démarre aucun worker. Ne mettez ni tenant, clé brute, digest, actor ou ID
d'opération dans le modèle de requête ; les champs inconnus sont refusés.

Gardez fichiers d'entrée et de contrôle privés, avec parents de confiance
existants et mode0600. Conservez le répertoire d'opération hors des projets
et répertoires importés dans les workers. Utilisez HTTPS avec vérification normale chaîne/nom,
ou une origine HTTP sur IP loopback numérique pour le contrôle local. Pour
une AC privée, donnez à `ca_bundle_file` le chemin absolu du fichier privé
contenant le bundle de certificats publics approuvés, au plus64Kio/huit
certificats. Ces racines explicites remplacent les racines intégrées. Ne
fournissez aucune clé privée d'AC et ne désactivez pas TLS.

Les chemins suivants sont des exemples choisis par l'opérateur. Le parent
doit déjà être privé ; le répertoire d'opération doit être neuf :

```sh
AIGUARD_BIN=/absolute/path/to/agentsandbox
AIGUARD_PRIVATE=/absolute/private/operator
"$AIGUARD_BIN" workload prepare \
  --input "$AIGUARD_PRIVATE/issue.json" \
  --control-file "$AIGUARD_PRIVATE/control.json" \
  --output-dir "$AIGUARD_PRIVATE/broker-key-issue"

"$AIGUARD_BIN" workload status \
  --operation-dir "$AIGUARD_PRIVATE/broker-key-issue"

"$AIGUARD_BIN" workload apply \
  --operation-dir "$AIGUARD_PRIVATE/broker-key-issue" \
  --control-file "$AIGUARD_PRIVATE/control.json"
```

`prepare` ne fait aucun HTTP. Il conserve requête exacte et, pour émission
ou rotation, credential aléatoire dans le fichier privé `credential` de
l'opération avant toute mutation distante. Il n'affiche jamais ce secret.
En conteneur, `--input -` et stdin évitent un bind hôte ; l'entrée reste
bornée16Kio/cinq secondes et fichiers de contrôle/destination conservent
leurs exigences de filesystem privé.

`apply` valide le runner courant explicitement fourni, l'origine, le tenant
et le namespace attendus, puis fait une requête de mutation. Aucun retry
automatique. Le succès annonce une opération historique enregistrée et son
reçu durable, avec `current_state:not_observed` ; il ne prétend pas que la
clé est toujours active. Un binding seul n'accorde ni grant broker, ni canal
de relais, distribution de clé ou démarrage/arrêt worker. Gardez ces
opérations distinctes selon les grants décrits ci-dessous. Ne montez jamais
le répertoire d'opération entier ni un credential runner/SSO dans un worker.

### Reprendre, tourner et révoquer sans perdre l'état

Après perte de réponse, préservez le répertoire privé d'opération complet.
Reprenez explicitement cette opération plutôt que préparer un remplacement :

```sh
"$AIGUARD_BIN" workload resume \
  --operation-dir "$AIGUARD_PRIVATE/broker-key-issue" \
  --control-file "$AIGUARD_PRIVATE/control.json"
```

La reprise garde IDs, bytes, secret, expiry et CAS. Une requête désormais
expirée peut encore récupérer son reçu antérieur. Le fichier de contrôle
courant peut référencer un runner autorisé remplaçant ou un bundle d'AC
approuvé, mais origine, tenant et namespace doivent correspondre à la
préparation. Un préflight puis POST n'est pas une comparaison atomique du
scope serveur pendant une reconfiguration concurrente du contrôle de
confiance ; conservez routage et attribution du credential de l'installation
pendant l'opération.

Pour rotation ou révocation, relisez le modèle correspondant, fixez la vraie
révision attendue et faites prepare/apply dans un **autre répertoire neuf**.
Un reçu local est historique et ne découvre pas la révision courante. Un
conflit409 doit être examiné ; ne devinez pas un CAS supérieur et n'éditez
pas une requête retenue. Distribuez une clé tournée seulement par la
configuration du consommateur autorisée et préservez l'ancienne opération
pour audit. Révoquer enregistre un refus du credential, pas l'arrêt d'un
processus en cours.

Une préparation partielle reste conservée et ne s'adopte/répare pas par
édition JSON. Une tentative apply/resume au résultat inconnu échoue jusqu'à
un reçu validé durable localement. `status` peut afficher avec succès cet
état local retenu en laissant explicitement l'issue distante non résolue.
Aucun secret, opération existante, base ou
reçu partiel n'est supprimé automatiquement. Le staging retenu est borné ;
une opération épuisée ou corrompue demande un diagnostic opérateur, sans
supprimer de preuve pour obtenir un statut réussi. Sur refus quota/horloge/
stockage, préservez l'état et utilisez les diagnostics ci-dessous avant une
reprise délibérée.

## Émettre, tourner et révoquer par l'API HTTP

La CLI ci-dessus gère la préparation durable pour l'opérateur. Un appelant d'intégration autorisé de confiance peut aussi préparer un credential Bearer privé à forte entropie, calculer localement son SHA-256 puis soumettre **uniquement le digest** dans le DTO d'émission. Le gateway ne génère ni ne renvoie le credential brut. L'authentification ultérieure le présente de façon privée sur le transport de contrôle approuvé ; ne pas le mettre en argv, JSON public, logs ou captures. Conserver sa source comme référence de fichier privé dans l'appelant de confiance. Aucun runner, jeton SSO, socket moteur ou environnement complet ne doit rejoindre un handoff worker.

Toutes les routes ci-dessous sont sous `/api/v1`. Les objets JSON sont fermés : les champs inconnus sont refusés. La limite de corps de requête du gateway est de 256 Kio, échappement/enveloppe JSON compris ; la limite propre à un corps ne la remplace pas. Utiliser `version:1`, UUID canoniques minuscules non nuls, expirations RFC3339 futures valides et digests SHA-256 hexadécimaux. Les Bearer workload font 32–512 octets sans espace blanc. Voici un **modèle de requête**, pas une identité installée. Remplacer chaque référence `<...>` par des IDs neufs, le vrai digest préparé et une expiration voulue ; ne pas envoyer les placeholders :

```json
{
  "version": 1,
  "operation_id": "<new-operation-uuid>",
  "credential_id": "<new-credential-uuid>",
  "expected_revision": 0,
  "subject": {
    "kind": "broker",
    "broker_id": "<broker-uuid>",
    "incarnation_id": "<new-incarnation-uuid>"
  },
  "sha256_digest": "<sha256-of-private-credential>",
  "expires_at": "<future-rfc3339-time>"
}
```

| Méthode et chemin | Champs runner / résultat |
|---|---|
| `POST /workload-bindings` | `version`, `operation_id`, `credential_id`, `expected_revision`, `subject`, `sha256_digest`, `expires_at`. Nouveau binding : révision 0 ; rotation/renouvellement : révision courante exacte. Renvoie 201, ou 200 au rejeu exact. |
| `POST /workload-bindings/{id}/revoke` | `version`, nouvel `operation_id`, `credential_id` égal à `{id}`, `expected_revision` positif. Renvoie le reçu durable. |

Le sujet worker alternatif est `{"kind":"worker","worker_id":"<worker-uuid>","attempt_id":"<attempt-uuid>","generation":1}`. Il doit désigner la tentative worker autorisée existante ; émettre une clé ne la crée ni ne la lance. V1 ne dispose pas de protocole de transition de génération : la rotation ne peut changer worker, tentative ou génération. Un binding ne peut pas changer de sujet. Au plus 64 IDs de credential sont retenus par sujet ; tourner le même binding conserve son credential ID et avance sa révision.

Conserver chaque nouvel `operation_id` et sa requête exacte avant transmission. Un retry exact renvoie le reçu d'origine ; un contenu durable différent sous le même ID d'opération est un conflit. Les reçus portent `version`, `credential_id`, `revision`, `audit_event_id`, `audit_seq`, `audit_hash` et `replayed`. Un reçu prouve l'enregistrement durable par le contrôle, pas la remise du credential ou l'exécution.

Une rotation exige un nouveau digest inutilisé ; l'ancienne clé brute cesse d'authentifier. Les digests historiques restent des preuves de propriété et ne peuvent être réintroduits après rotation, révocation ou expiration. Seul le renouvellement de la clé encore courante et active peut réutiliser son digest. La révocation exige la révision courante et l'avance. Ne pas réutiliser la clé d'un autre tenant, d'un humain ou du contrôle ; une propriété ambiguë échoue fermée. Préserver préparation privée, requête exacte et reçu après réponse perdue, sans émettre aveuglément une autre identité.

## Autoriser un broker et interpréter son heartbeat

Après préparation du binding broker, un runner accorde son périmètre d'autorité. Ce registre est distinct du Registry2 local de `broker serve --config`. Un objet autorité a la forme `{"authority_id":"<approved-authority-id>","runtime_kind":"docker"}`. Son `authority_id` est un libellé borné (1–256 octets, aucun caractère de contrôle), associé à `runtime_kind:docker|podman|kubernetes`. Ce n'est ni une connexion runtime ni un adaptateur Kubernetes installé. La propriété historique d'une autorité ne peut être transférée à un autre broker ou type de runtime par édition d'un grant.

| Méthode et chemin | Preuve et requête fermée |
|---|---|
| `POST /brokers/grants` | Runner : `version`, `operation_id`, `broker_id`, `expected_revision` (`null` initialement, puis révision courante positive exacte du grant), `credential_id`, `incarnation_id`, `authorities` (1–16 IDs d'autorité uniques), `lease_seconds` (1–300), `not_after` futur. Grant sérialisé ≤16 Kio. Renvoie 201, ou 200 au rejeu. |
| `POST /brokers/{id}/revoke` | Runner : `version`, `operation_id`, `broker_id` égal à `{id}`, `expected_revision` positif. |
| `POST /brokers/heartbeat` | Broker : `version`, `operation_id`, `expected_heartbeat_revision`, `incarnation_id`, `sequence`. |
| `GET /brokers/self` | Broker : aucun corps ni sélecteur de périmètre ; renvoie sa propre `BrokerViewV1`. |
| `GET /brokers?limit=50` | Viewer technique ou runner : limite 1–100, défaut 50 ; `cursor` facultatif est l'UUID broker exact renvoyé comme `next_cursor`. Renvoie `version`, `items`, `next_cursor`. |

Un nouveau grant utilise un UUID d'incarnation jamais employé et un binding broker correspondant. Remplacer un grant exige aussi nouvelle incarnation et sujet/binding correspondant ; tourner un credential seul n'invente pas une incarnation. Comparer la révision durable du grant, pas une supposition en cache. Un heartbeat porte la révision heartbeat courante exacte, l'incarnation du grant et la séquence suivante (d'abord 1, puis précédente+1). Son rejeu exige toujours binding authentifié courant et grant non expiré/non révoqué ; un ancien reçu ne rétablit pas l'autorité. Les résultats des mutations runner et heartbeats contiennent métadonnées broker, révisions grant/heartbeat, opération/empreinte d'entrée et séquence/empreinte d'audit, avec `replayed` explicite.

`last_heartbeat.heartbeat_provenance:"declared"` décrit la déclaration du broker. `reception_provenance:"observed"` et `received_at` décrivent sa réception par le gateway, pas un effet runtime. `lease_expires_at` est borné par réception plus lease configurée et par `not_after`. Les vues exposent `state` (`unseen`, `current`, `stale`, `expired`, `revoked`, `clock_uncertain`), `evaluated_at` et `runtime_inventory_complete:false`. Un broker périmé signifie que sa lease n'est plus courante ; cela ne signifie ni zéro worker, ni workers arrêtés, ni quota libérée ou inventaire runtime complet. Préserver les derniers faits connus et examiner la même autorité.

## Autoriser des messages dirigés

Un runner accorde explicitement un endpoint source → un endpoint destination différent + canal dans son tenant. Même tenant ne donne aucune permission ; le sens inverse exige un autre grant. Les endpoints portent `worker_id`, `attempt_id`, `generation:1`. Tous deux doivent avoir des bindings workload valides courants et être enregistrés Running avec identité de conteneur et quota tenue sous contrôle actif. Ces vérifications ne fournissent pas le transport runtime manquant.

| Méthode et chemin | Preuve et requête fermée |
|---|---|
| `POST /relay/grants` | Runner : `version`, `operation_id`, `grant_id`, `source`, `destination`, `channel`, `expires_at`. Canal 1–64 caractères ASCII alphanumériques/`-_.` ; expiration après maintenant et dans 24 heures. Renvoie 201, ou 200 au rejeu. |
| `POST /relay/grants/{id}/revoke` | Runner : `version`, `operation_id`, `grant_id` égal à `{id}`. La révocation bloque aussi la remise du contenu déjà en attente. |
| `POST /relay/messages` | Worker source : `version`, `operation_id`, `grant_id`, `ttl_seconds`, `body` (chaîne). Aucun sélecteur tenant/source. Renvoie 201, ou 200 au rejeu. |
| `GET /relay/messages?limit=32` | Worker destination : limite 1–32, défaut 32 ; `after` facultatif non négatif utilise le `next_after` renvoyé. Renvoie `version`, `items`, `next_after`. |
| `POST /relay/messages/{id}/ack` | Worker destination : `version`, `operation_id`, `message_id` égal à `{id}`. |
| `POST /relay/messages/{id}/prune` | Runner : `version`, `operation_id`, `message_id` égal à `{id}` ; uniquement après expiration du message. Retire le corps, conserve métadonnées/tombstones et audit. |

Par exemple, le **modèle public de DTO d'envoi** est `{"version":1,"operation_id":"<new-operation-uuid>","grant_id":"<authorized-grant-uuid>","ttl_seconds":60,"body":"Public example: report is available."}`. Remplacer les IDs avant utilisation. Une requête ACK ou prune prend son propre nouvel UUID d'opération ; `message_id` vient du `resource_id` du reçu d'envoi ou des métadonnées de l'inbox. Les reçus relais contiennent `version`, `operation_id`, `resource_id`, `audit_seq`, `audit_hash`, `replayed`.

| Borne du relais | Limite actuelle |
|---|---|
| Corps UTF-8 non vide | 65536 octets (64 Kio), avant échappement JSON |
| TTL message | 1–3600 secondes, aussi borné par l'expiration du grant |
| Nouveaux envois par grant | 60 dans les 60 secondes précédentes, vérification durable |
| Corps retenus par tenant | 1024 messages et 16 Mio, y compris corps acquittés/expirés jusqu'au prune explicite |
| Page d'inbox | Au plus 32 éléments et budget JSON encodé de 2 Mio |

Les corps sont des données métier du tenant dans sa base. L'audit enregistre métadonnées, empreintes et tailles du message, jamais une copie du corps. Les vérifications des secrets connus et de masquage structuré refusent les corps nécessitant un masquage, sans autoriser l'envoi de secrets ni garantir leur reconnaissance exhaustive. Ne pas mettre de credential ou jeton sensible dans les messages.

Les lectures d'inbox sont auditées durablement avant sortie du contenu du contrôle. Une page contient pour chaque message `sequence`, `metadata` et `body` ; les métadonnées identifient endpoints/canal, expiration, empreinte/taille du corps et état ACK/prune. Continuer avec le curseur renvoyé sous la même identité ; une page n'est ni snapshot ni total. La remise revérifie grant et autorité courante des endpoints. Grants expirés/révoqués et endpoints révoqués ou terminés bloquent aussi les messages en attente. Un ACK enregistre l'acquittement du destinataire, **pas une exécution ou un effet**, et ne purge pas le corps retenu.

Conserver IDs d'opération et requêtes exacts pour les retries. L'idempotence ne contourne pas les permissions : les rejeux send/ACK revérifient preuve worker, grant et endpoints courants. Un binding tourné avec le même credential ID conserve cette identité, mais un autre credential ID change l'identité send/ACK et ne peut reprendre son rejeu. Le rejeu d'un grant exige aussi des endpoints actuellement autorisés ; les revoke/prune runner conservent leurs propres règles de rejeu exact. Un ancien reçu ne contourne pas les refus courants. Aucun regrant automatique, redémarrage ou routage inter-tenant n'a lieu.

## Diagnostiquer, arrêter et préserver l'état

| Résultat HTTP | Interprétation et action suivante |
|---|---|
|401 | Preuve Bearer absente, header ambigu, preuve invalide, expirée ou révoquée. L'authentification workload emploie `authentication_required` ; une preuve worker sur route runner-only est refusée par le garde historique des clés de contrôle, dont l'erreur peut être une simple chaîne JSON sans ce code. Vérifier fichier privé et rôle de la route sans afficher la clé. |
|403 `workload_forbidden`, `relay_forbidden` ou `relay_expired` | Preuve, périmètre, grant, endpoint ou expiration n'autorise pas cet échange. Corriger l'autorité depuis le contrôle de confiance sans élargir le réseau. |
|400 requête workload/relais invalide | Vérifier champs fermés, UUID, IDs chemin/corps, révisions et limites. |
|409 `workload_conflict`, `broker_conflict`, `relay_conflict` | Révision CAS ou contenu d'opération différent, ou modification d'identité/autorité immuable. Préserver et comparer requête/reçu d'origine. |
|429 `audit_workload_budget_exhausted` / `audit_write_budget_exhausted` | Refus cumulatif avec `retry_action:"contact_operator"` ; préserver l'état et examiner occupation/réserves. |
|429 `relay_rate_limited`, `relay_capacity_exhausted`, `issuance_busy`, refus d'admission I/O | Respecter la borne de fréquence/concurrence concernée ; les corps retenus exigent un prune délibéré après expiration. Réessayer seulement la même opération prévue après résolution de la cause. |
|503 `control_clock_uncertain` | UTC précède les preuves d'autorisation durables ; `retry_action:"contact_operator"`. Préserver l'état et réparer/réconcilier l'horloge de confiance. Ne jamais interpréter cela comme une inbox vide ou aucun broker. |
|503 identité/stockage/schéma indisponible | Vérifier schéma 6/7, propriété non ambiguë des credentials, intégrité et stockage requis. Une source tenant indisponible ne prouve pas une absence ; aucune substitution d'identité ni repli viewer. |

Le plancher UTC des schémas 6/7 dérive des preuves durables de réception de l'audit. Les comparaisons d'autorisation/expiration emploient une précision UTC milliseconde ; cela ne démontre ni détection d'un recul entre requêtes dans une même milliseconde, ni horloge SSH/SSO de confiance ou garantie générale d'horloge. Les timestamps de réception réels restent des observations réelles ; le plancher ne fabrique pas un `received_at` ultérieur. Un refus d'horloge n'autorise pas l'édition des timestamps ou tables de plancher durables.

Avant maintenance, révoquer les autorités relais/broker et bindings workload concernés par leurs opérations autorisées distinctes, conserver requêtes/reçus puis arrêter les jobs par la [procédure broker prise en charge](../broker/README.fr.md#arrêter-redémarrer-et-maintenir-linstallation). Une révocation n'est ni une observation d'arrêt runtime ni une libération de quota. Préserver l'enforcement existant même si le contrôle devient indisponible. Redémarrer seulement la même installation voulue après vérification du propriétaire, du stockage et de l'horloge ; budgets, révocations et propriété historique des clés survivent au redémarrage. Conserver ensemble sauvegardes privées, bindings, journal broker, données tenant et preuves runtime. Aucune commande ici ne fournit fédération, installation automatique de service, réactivation sûre de supervision ou nettoyage destructif.

DTO et limites exacts sont dans [workload.rs](../../../src/workload.rs), [broker_registry.rs](../../../src/broker_registry.rs) et [relay.rs](../../../src/relay.rs) ; routes dans [api/workload.rs](../../../src/api/workload.rs), [api/brokers.rs](../../../src/api/brokers.rs) et [api/relay.rs](../../../src/api/relay.rs). Les consommateurs de politique sont [gateway_config.rs](../../../src/gateway_config.rs), [main.rs](../../../src/main.rs) et [audit/capacity.rs](../../../src/audit/capacity.rs).
