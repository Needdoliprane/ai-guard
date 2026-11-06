# Accès local nominatif

[English](README.md) · [Configuration](../README.fr.md) · [Poste local](../local/README.fr.md) · [OIDC](../keycloak/README.fr.md)

Un compte local nominatif permet de consulter les ressources explicitement autorisées sans fournisseur d'identité. Son credential est distinct des clés techniques runner/viewer et de l'authentification Codex ou Claude. Appartenir à un tenant ne donne pas accès à toutes ses sessions. Ce guide couvre la maintenance du compte, la consultation et l'export ; le [registre d'ingénierie](../../engineering/CURRENT_STATE.md) distingue implémentation et qualification.

## Préparer le stockage et l'autorité

Utiliser le même binaire Rust de confiance que le gateway, un répertoire de contrôle privé et la base déjà liée au tenant. Ne pas copier cette base dans un worker ni monter le socket moteur dans le gateway. Arrêter gateway, brokers et redémarrages automatiques avant la maintenance. `--confirm-control-stopped` est une attestation opérateur, pas un mécanisme de fencing. Une réouverture ordinaire ne restaure pas l'autorité d'exécution.

Ces exemples partent d'une base existante en schéma4. Remplacer chemins et tenant par les valeurs réelles de l'installation. `--tenant-id` vérifie son binding existant ; il ne choisit pas le périmètre d'un utilisateur distant. Pour une base plus ancienne, appliquer chaque étape exacte de la [procédure de migration](../broker/README.fr.md), avec une nouvelle sauvegarde vérifiée à chaque étape.

```sh
AIGUARD_BIN="/chemin/absolu/vers/agentsandbox"
AIGUARD_DATA="/chemin/prive/ai-guard/audit.sqlite"
AIGUARD_TENANT="tenant-configure"
AIGUARD_PRIVATE="/chemin/prive/ai-guard"
"$AIGUARD_BIN" migrate-control \
  --data "$AIGUARD_DATA" --tenant-id "$AIGUARD_TENANT" \
  --backup "$AIGUARD_PRIVATE/avant-acces-humain-v5.sqlite" \
  --target-version 5 --max-events 100000 --confirm-control-stopped
```

Le schéma5 ajoute le protocole d'audit durable des lectures humaines. Modifier `user_version` manuellement ne remplace pas la migration. Mettre à jour tous les lecteurs ensemble : les anciens binaires refusent les nouveaux schémas. Aucun downgrade, restauration, activation ou compte n'est créé automatiquement. Provision, resume-provision et apply acceptent les schémas4–10 validés avec supervision inactive ; les lectures HTTP humaines ordinaires prennent en charge5–10 et renvoient `migration_required` avant leur audit sur une base plus ancienne. La lecture du registre brokers et sa nouvelle capacité exigent7. Le [guide workload](../workloads/README.fr.md#activer-la-lecture-nominative-du-registre-brokers) décrit5→6→7, y compris pour une base préparée en5 par `setup prepare`.

Pour une base appartenant à un conteneur, exécuter la maintenance avec l'UID/GID du contrôle sur son volume existant, après arrêt de tous les autres consommateurs. Le CLI hôte ne peut pas supposer qu'il possède un volume Docker Desktop. Le conteneur ponctuel n'a besoin que de ce volume et de son image de confiance, sans socket, credential runner, secret fournisseur ou réseau. Conserver l'identité du projet Compose et de son volume ; ne pas initialiser une autre base pour contourner une migration refusée.

Pour l'installation Docker standard, relever `.Name` du montage `/data` existant sur le gateway arrêté avec `docker inspect`. Construire l'image de contrôle à jour puis relever son ID immuable avec `docker image inspect --format '{{.Id}}' agentsandbox-control:local` ; le gateway arrêté peut encore désigner une ancienne image. Définir les valeurs réelles ci-dessous, sans deviner le nom du volume. Ces commandes utilisent une base schéma4 existante et conservent les deux conteneurs ponctuels. Leurs noms et chemins de sauvegarde/sortie doivent être neufs ; le sous-shell s'arrête au premier échec :

```sh
AIGUARD_CONTROL_IMAGE_ID="sha256:identifiant-image-controle-a-jour-inspectee"
AIGUARD_AUDIT_VOLUME="volume-audit-existant-inspecte"
(
set -eu
docker volume inspect --format '{{.Name}}' "$AIGUARD_AUDIT_VOLUME"
docker run --name ai-guard-offline-human-v5 --pull never \
  --user 1000:1000 --network none --read-only --cap-drop ALL \
  --security-opt no-new-privileges:true --cpus 1 --memory 384m --memory-swap 384m --pids-limit 128 \
  --tmpfs /tmp:rw,noexec,nosuid,nodev,size=16m,mode=1777 \
  --mount "type=volume,source=$AIGUARD_AUDIT_VOLUME,target=/data,volume-nocopy" \
  "$AIGUARD_CONTROL_IMAGE_ID" migrate-control --data /data/audit.sqlite \
  --tenant-id "$AIGUARD_TENANT" --backup /data/before-human-access-v5.sqlite \
  --target-version 5 --max-events 100000 --confirm-control-stopped
docker run --name ai-guard-offline-human-alice --pull never \
  --user 1000:1000 --network none --read-only --cap-drop ALL \
  --security-opt no-new-privileges:true --cpus 1 --memory 384m --memory-swap 384m --pids-limit 128 \
  --tmpfs /tmp:rw,noexec,nosuid,nodev,size=16m,mode=1777 \
  --mount "type=volume,source=$AIGUARD_AUDIT_VOLUME,target=/data,volume-nocopy" \
  "$AIGUARD_CONTROL_IMAGE_ID" human provision --data /data/audit.sqlite \
  --tenant-id "$AIGUARD_TENANT" --name Alice --organization engineering \
  --output-dir /data/human-alice --max-events 100000 --confirm-control-stopped
)
```

Contrôler chaque résultat avant de continuer ; préserver les preuves d'échec et reprendre seulement la préparation exacte correspondante. L'entrypoint de l'image standard est le CLI Rust. Pour Podman, utiliser le même compte/autorité et les commandes `podman` sur son propre volume existant, avec qualification distincte du mapping UID. Copier uniquement `/data/human-alice/credential` depuis le conteneur de provisioning conservé vers une **nouvelle destination hôte privée** de l'utilisateur avec `docker cp` sans `--archive`, puis vérifier propriétaire/mode0600. Ne pas afficher le contenu ni distribuer les reçus opérateur. Ne pas exécuter ensuite le provisioning hôte sur une autre base : les deux parcours de maintenance sont alternatifs.

## Créer le compte et conserver sa reprise

Le parent de sortie doit déjà être privé (`0700`). Choisir un nouveau répertoire ; la commande refuse d'écraser ou d'adopter un répertoire existant.

```sh
"$AIGUARD_BIN" human provision \
  --data "$AIGUARD_DATA" --tenant-id "$AIGUARD_TENANT" \
  --name "Alice" --organization "engineering" \
  --output-dir "$AIGUARD_PRIVATE/human-alice" \
  --max-events 100000 --confirm-control-stopped
```

La commande prépare les fichiers privés `credential`, `recovery.json` et `completion.json`. Elle génère un credential aléatoire sans l'afficher. Compte, binding du credential et membership initial sont validés dans une transaction avec leur audit. La sortie présente des identifiants et chemins, jamais le credential. Les capacités initiales sont lecture des métadonnées, lecture du contenu et export ; les grants de ressources restent nécessaires. `--capabilities read-metadata` permet de limiter ce choix.

Après une interruption ou un résultat ambigu, préserver exactement la préparation et la base, puis reprendre :

```sh
"$AIGUARD_BIN" human resume-provision \
  --data "$AIGUARD_DATA" --tenant-id "$AIGUARD_TENANT" \
  --output-dir "$AIGUARD_PRIVATE/human-alice" \
  --max-events 100000 --confirm-control-stopped
```

La reprise vérifie namespace, empreinte du credential, chemins, fichiers et identités d'opérations conservés. Elle ne remplace pas une préparation manquante et ne renouvelle pas le credential. Protéger les reçus comme des données de contrôle. Transmettre uniquement son credential à l'utilisateur par votre canal privé, sans distribuer toute la préparation opérateur. Son fichier doit rester `0600` sous un parent privé, hors du projet exécuté.

## Autoriser explicitement les ressources

Les anciennes sessions ne sont pas automatiquement attribuées au nouveau compte. L'opérateur de confiance lie une session, sandbox, agent ou projet connu à une organisation et à un propriétaire humain. Des partages peuvent donner séparément métadonnées, contenu et export. Un nom affiché, un projet correspondant ou un membership ne prouvent ni auteur ni autorisation.

`human apply --data ... --tenant-id ... --mutation-file ... --confirm-control-stopped` lit un objet JSON privé avec `version`, un nouvel `operation_id` et un `change` typé. Un champ `actor` est refusé : l'autorité opérateur vient du filesystem de contrôle vérifié et de l'UID effectif. Les changements sont `account`, `credential`, `membership` ou `resource_grant`, avec révision précédente attendue et révision suivante du record. Le premier binding d'une ressource historique appartient au contrôle de confiance. Un replay exact conserve identifiant et contenu ; changer le sens exige un nouvel identifiant. Le [modèle d'identité typé](../../../src/identity/types.rs) définit les champs complets.

Pour le premier binding d'une session, enregistrer ce **modèle** dans un nouveau fichier JSON privé0600 sous le parent privé. Remplacer chaque `<...>` : l'opération utilise un nouvel UUID canonique minuscule non nul, la session doit déjà être connue de ce tenant et les deux UUID du propriétaire viennent du résultat de son provisioning. L'organisation correspond à son membership. Cet exemple ne partage rien avec d'autres personnes.

```json
{
  "version": 1,
  "operation_id": "<nouvel-uuid-operation>",
  "change": {
    "action": "resource_grant",
    "expected_revision": 0,
    "grant": {
      "version": 1,
      "tenant_id": "<tenant-configure>",
      "organization_scope": "engineering",
      "resource": {"kind": "session", "resource_id": "<session-connue>"},
      "owner": {
        "kind": "human_local",
        "installation_id": "<uuid-installation-du-compte>",
        "account_id": "<uuid-compte>"
      },
      "revision": 1,
      "active": true,
      "shares": []
    }
  }
}
```

```sh
"$AIGUARD_BIN" human apply \
  --data "$AIGUARD_DATA" --tenant-id "$AIGUARD_TENANT" \
  --mutation-file "$AIGUARD_PRIVATE/grant-session-alice.json" \
  --max-events 100000 --confirm-control-stopped
```

Désactiver compte, credential, membership ou grant par une nouvelle mutation auditée, en conservant son identité et en augmentant sa révision. Pour ce grant resté en révision1, la révocation utilise un nouvel UUID d'opération, `expected_revision:1`, grant `revision:2` et `active:false`, avec mêmes tenant, organisation, ressource et propriétaire. Un conflit de révision est un refus : examiner le record opérateur conservé plutôt que deviner. Ne pas modifier directement les projections ou réutiliser le credential d'une autre personne. Après changement de droits, reprendre un contexte actuellement proposé. Créer un compte ne lance aucun worker et ne délègue pas le rôle runner.

## Autoriser la lecture nominative du registre brokers

`read_broker_registry` est une capacité d'administration distincte sur les métadonnées d'infrastructure de tout le tenant authentifié. Elle n'expose ni contenus de ressources, ni grants brokers bruts, ni credentials, ni inventaire workers, ni export. `read_tenant_metadata` conserve son filtre d'organisation. Les anciens memberships, un nom d'administrateur ou un rôle IdP n'acquièrent jamais automatiquement ce nouveau droit.

Effectuer d'abord la [migration explicite vers7](../workloads/README.fr.md#activer-la-lecture-nominative-du-registre-brokers), tous consommateurs arrêtés. Sur schéma7 validé avec supervision inactive, l'opérateur de confiance du système de fichiers peut créer un **nouveau** lecteur local avec cette seule capacité :

```sh
"$AIGUARD_BIN" human provision \
  --data "$AIGUARD_DATA" --tenant-id "$AIGUARD_TENANT" \
  --name "Lecteur infrastructure" --organization "engineering" \
  --capabilities read-broker-registry \
  --output-dir "$AIGUARD_PRIVATE/human-infrastructure-reader" \
  --max-events 100000 --confirm-control-stopped
```

Remplacer `100000` par la limite réelle de l'installation si différente. Avec une politique explicite d'audit humain, transmettre aussi le même `--audit-capacity-file` qu'aux autres commandes d'identité hors ligne. Les capacités de création par défaut restent `read-metadata,read-content,export` ; la nouvelle option n'est jamais implicite. La demander avant7 échoue avant création du répertoire de préparation. `resume-provision` peut rejouer une ancienne préparation exacte après migration sans ajouter de grant ni renouveler son credential.

Pour un compte local ou OIDC **existant**, utiliser `human apply` avec son véritable membership. Le JSON fermé suivant est un **modèle pour un membership actuellement en révision1 possédant seulement `read_metadata`**. Remplacer tous les paramètres, conserver principal, tenant, organisation et ID exacts, ainsi que toutes les capacités existantes encore voulues. Pour une autre révision, employer son véritable `expected_revision` et sa révision suivante :

```json
{
  "version": 1,
  "operation_id": "<nouvel-uuid-operation>",
  "change": {
    "action": "membership",
    "expected_revision": 1,
    "membership": {
      "version": 1,
      "membership_id": "<uuid-membership-existant>",
      "principal": {
        "kind": "human_local",
        "installation_id": "<uuid-installation-existant>",
        "account_id": "<uuid-compte-existant>"
      },
      "tenant_id": "<tenant-configure>",
      "organization_scope": "engineering",
      "revision": 2,
      "active": true,
      "capabilities": ["read_metadata", "read_broker_registry"]
    }
  }
}
```

Pour OIDC, la même position `principal` vaut `{"kind":"human_oidc","issuer":"<issuer-exact-approuve>","subject":"<subject-verifie>"}`. Conserver les octets du principal existant : changer principal ou organisation d'un membership est refusé. Un premier membership utilise `expected_revision:0` et `revision:1`, après création de son compte. Enregistrer la mutation relue dans un nouveau fichier privé0600, sans champ `actor` :

```sh
"$AIGUARD_BIN" human apply \
  --data "$AIGUARD_DATA" --tenant-id "$AIGUARD_TENANT" \
  --mutation-file "$AIGUARD_PRIVATE/broker-reader-membership.json" \
  --max-events 100000 --confirm-control-stopped
```

Seuls le contrôle de confiance authentifié ou cette autorité hors ligne vérifiée peuvent introduire ou réactiver ce droit tenant-wide. Un administrateur d'organisation possédant `manage_memberships` peut le conserver sur un membership déjà actif, le garder dormant sur un membership inactif, le retirer ou désactiver le membership dans ses droits existants. Il ne peut ni l'ajouter, ni le restaurer après retrait, ni réactiver un membership inactif qui le porte, pour lui-même ou autrui. Ne pas créer un second membership pour changer son principal/organisation immuable ou contourner cette frontière. OIDC exige aussi le plafond explicite sur claims vérifiés décrit dans le [guide SSO](../sso/README.fr.md#activer-explicitement-la-lecture-du-registre-brokers).

Après réouverture des services de contrôle voulus, se connecter normalement et choisir un contexte **administration** actuellement proposé contenant `read_broker_registry`. Le dashboard propose **Brokers** et **Brokers enregistrés du tenant.** ; un contexte possédant cette seule capacité ouvre directement cette vue. Il ne lit automatiquement ni ressources, ni événements, ni export, ni leur flux SSE. Il n'existe ni `human brokers` ni commande brokers dans le CLI SSO au premier plan ; utiliser le dashboard ou le [contrat HTTP humain](../../api.md#registre-brokers-humain--human-broker-registry). Ne jamais saisir une clé runner dans le navigateur.

Le tableau et **Détails du broker** montrent heure d'évaluation serveur, incarnation courante, révisions d'autorisation/heartbeat, autorités déclarées, dernière réception, expirations et reçu durable. **Bail courant** signifie que le bail enregistré est courant ; cela ne prouve pas un worker en fonctionnement. **Heartbeat d’une ancienne incarnation** conserve la dernière déclaration reçue sans l'attribuer à la nouvelle incarnation. Une page vide réussie signifie aucune ligne enregistrée à cette position et heure d'évaluation, jamais une activité runtime nulle. Les lignes proviennent des vraies opérations d'enregistrement/heartbeat du contrôle décrites dans le [guide workload](../workloads/README.fr.md#autoriser-un-broker-et-interpréter-son-heartbeat), sans événement synthétique ni édition manuelle de SQLite.

La pagination est live, avec au plus100 lignes et256Kio par réponse ; le dashboard en demande25. Les lignes peuvent changer entre pages, sans total ni snapshot. Conserver le curseur opaque exact, le contexte, la version d'autorisation et la limite. Le serveur borne une traversée à1000pages/16Mio dans la durée absolue du contexte, au plus300secondes. Ce n'est pas un crédit d'audit renouvelable.

**Actualiser** relit la vue sélectionnée. L'actualisation automatique est déclenchée au plus toutes les30secondes lorsque Brokers est visible ; latence réseau, files d'attente, navigateur suspendu ou contrôle déconnecté peuvent prolonger l'attente. Ce n'est ni un délai de réponse ni une borne de retrait des droits. Un403 retire lignes, détails et curseurs et propose **Relire mes accès**. Changer de contexte, quitter Brokers ou se déconnecter annule ses lectures et retire sa page ; les réponses tardives ne peuvent repeupler l'ancienne vue.

Chaque page, même vide, exige un reçu durable dans les budgets humains existants du tenant/principal. Autorisation, projection bornée et reçu sont engagés dans une transaction Store ; la route réauthentifie avant publication. Le reçu constate la lecture, sans prouver livraison, exécution ou checkpoint signé externe. Une panne d'audit ne rend aucune page. Un503 `migration_required` exige la migration hors ligne ; un429 `audit_read_budget_exhausted` avec `contact_operator` exige une intervention, pas des retries périodiques. Le dashboard conserve la dernière page datée/périmée et arrête les lectures automatiques, même après changement de visibilité ou resélection. Préserver base, politique et reçus, examiner occupation/réserves puis réparer la cause. Après intervention, **Relire après intervention de l’opérateur** permet une tentative explicite ; seul un succès courant permet de reprendre l'actualisation. Les pannes temporaires de capacité/transport restent distinctes. Ne jamais supprimer l'audit, renouveler les credentials ou réinitialiser le stockage pour récupérer de la capacité.

Cette vue ne qualifie ni transport multi-brokers, ni fédération, ni agrégation workers/sessions, ni complétude runtime, ni capacité100, ni release production. Elle ne change aucun contrat de quota ou facturation : l'allocation reste non facturable et l'usage non observé reste inconnu. Consulter les preuves d'ingénierie pour la portée réelle de validation des sources correspondantes.

## Consulter avec le CLI

Redémarrer uniquement les services concernés avec le binaire à jour et les données existantes. Indiquer l'origine du gateway, sans chemin `/api`. HTTPS est vérifié ; HTTP local exige une IP loopback littérale, par exemple `127.0.0.1`. Ce client désactive proxy ambiant, redirections et retries automatiques.

Pour un gateway HTTPS signé par une AC privée approuvée, ajouter `--gateway-ca-file /chemin/absolu/approuve/gateway-ca.pem` à chaque commande `human identity`, `resources`, `events`, `export` et `resume-export`. Employer un bundle PEM public vérifié ne contenant que des certificats, au plus 65536 octets et 8 certificats, dans un répertoire protégé. Un fichier privé `0600` ou une projection en lecture seule `0444` est pris en charge ; alias modifiables, liens, propriétaires/ancêtres non fiables et ACL non prises en charge sont refusés. Ce bundle remplace les racines intégrées du seul client gateway, avec contrôle de chaîne, SAN et validité conservé. Sans l'option, la confiance WebPKI intégrée existante reste inchangée. L'option exige HTTPS et ne change ni confiance système, ni navigateur, ni fournisseur OIDC.

```sh
AIGUARD_GATEWAY="https://approved-gateway.example.invalid"
AIGUARD_GATEWAY_CA="/chemin/absolu/approuve/gateway-ca.pem"
AIGUARD_CREDENTIAL="$AIGUARD_PRIVATE/human-alice/credential"
"$AIGUARD_BIN" human identity \
  --gateway "$AIGUARD_GATEWAY" --gateway-ca-file "$AIGUARD_GATEWAY_CA" \
  --credential-file "$AIGUARD_CREDENTIAL"
```

Remplacer cette origine d'exemple non fonctionnelle et le chemin d'AC par les valeurs approuvées par l'opérateur. Le CLI charge un snapshot immuable de confiance par invocation ; remplacer le fichier ne change pas une commande en cours. Un export conservé lie les octets exacts du bundle, en plus du gateway, du credential et du contexte. Garder les mêmes octets pour `resume-export`, même si le bundle passe dans un autre chemin vérifié. Un bundle modifié exige un nouvel export explicite vers une destination neuve ; préserver la préparation non résolue. `gateway CA configuration is invalid or unavailable` demande de corriger fichier ou PEM avant de réessayer. `--gateway-ca-file requires an HTTPS gateway origin` indique un transport incompatible. Un refus de connexion/TLS ne désactive jamais la vérification et ne bascule vers aucune autre source de confiance.

```sh
AIGUARD_GATEWAY="http://127.0.0.1:8787"
AIGUARD_CREDENTIAL="$AIGUARD_PRIVATE/human-alice/credential"
"$AIGUARD_BIN" human identity \
  --gateway "$AIGUARD_GATEWAY" --credential-file "$AIGUARD_CREDENTIAL"
```

Recopier un identifiant exact de contexte proposé. Le contrôle déduit tenant, organisation, mode et capacités de l'identité authentifiée. Un contexte n'authentifie pas son porteur. Les modes personnel et administration sont distincts ; l'administration n'est proposée que si autorisée. La durée maximale d'un contexte est300secondes, sans renouvellement implicite.

```sh
AIGUARD_CONTEXT="identifiant-exact-propose-par-human-identity"
"$AIGUARD_BIN" human resources \
  --gateway "$AIGUARD_GATEWAY" --credential-file "$AIGUARD_CREDENTIAL" \
  --context "$AIGUARD_CONTEXT" --limit 25
```

Utiliser un véritable identifiant de ressource retourné et choisir explicitement métadonnées ou contenu :

```sh
AIGUARD_SESSION="identifiant-session-autorisee"
"$AIGUARD_BIN" human events \
  --gateway "$AIGUARD_GATEWAY" --credential-file "$AIGUARD_CREDENTIAL" \
  --context "$AIGUARD_CONTEXT" --resource-kind session \
  --resource-id "$AIGUARD_SESSION" --view metadata --limit 25
"$AIGUARD_BIN" human export \
  --gateway "$AIGUARD_GATEWAY" --credential-file "$AIGUARD_CREDENTIAL" \
  --context "$AIGUARD_CONTEXT" --resource-kind session \
  --resource-id "$AIGUARD_SESSION" --view metadata \
  --output-dir "$AIGUARD_PRIVATE/export-session-alice"
```

La pagination utilise le `next_cursor` opaque exact et une sélection inchangée ; une page ne donne pas le total. Chaque lot restitué exige son reçu nominatif durable, preuve de l'enregistrement par le contrôle, pas de réception ou de vérification complète de la chaîne d'audit. Les métadonnées excluent commandes, prompts/résultats et motifs libres des décisions ; le contenu exige un droit distinct. L'export exige aussi son propre droit.

L'export publie une destination privée neuve, avec budget client25pages,4MiB et60secondes depuis la préparation initiale ; la reprise ne renouvelle pas cette échéance. Une sortie incomplète reste explicite. Préserver son répertoire voisin `.preparing` après échec. `human resume-export` reprend avec les mêmes gateway, fichier de credential, contexte et destination finale, puis vérifie la sélection conservée. Après60secondes, un export incomplet ne peut plus reprendre même si son contexte300secondes reste valide. Budget expiré ou contexte refusé/expiré exigent une nouvelle autorisation et un export explicite vers une autre destination neuve, sans effacer la préparation non résolue.

## Dashboard et erreurs

Choisir l'accès local nominatif dans le formulaire de connexion, saisir seulement le credential personnel depuis votre lecteur privé, puis sélectionner un contexte proposé. La connexion viewer historique reste distincte. Changer d'identité, mode ou contexte retire données, curseurs et exports précédents. Ressources et événements temps réel passent par les mêmes autorisations Rust que le CLI ; le navigateur n'accède jamais au moteur.

| Résultat | Action |
|---|---|
|401, authentification refusée | La session humaine se ferme. Vérifier état du credential/compte puis se reconnecter explicitement. |
| Contexte expiré/révoqué | Retirer ses données et curseurs, relire l'identité puis choisir un contexte proposé. |
| Ressource introuvable/refusée | Vérifier les droits explicites. Un identifiant inaccessible ne révèle pas son existence chez autrui. |
|503 `migration_required` | Arrêter les contrôles, conserver une sauvegarde vérifiée et migrer explicitement ; aucun repli viewer. |
|429 `audit_read_budget_exhausted`, `contact_operator` | Arrêter les retries automatiques et faire examiner le budget durable d'audit par l'opérateur. |
| Autre refus 429, capacité ou taille de page | Suivre l'action de retry/réduction indiquée, sans élargir les permissions. |
| Audit ou stockage indisponible | Les lectures exigeant un audit sont refusées. Réparer capacité/stockage avant reprise. |

Le client SSE reprend seulement après une trame complète validée. Un retry peut dupliquer des événements ; posséder un curseur ne prouve ni réception ni téléchargement. La révocation bloque les lectures futures, pas les octets déjà reçus. Identité locale, identité OIDC, identité de workload et authentification fournisseur IA restent indépendantes.

## Budgets d'audit et capacité des flux

Les lectures humaines d'identité, ressources, événements et exports exigent leur
propre reçu durable. Les lots SSE livrés aussi ; un poll SSE vide ne crée aucun
reçu. Ces budgets cumulatifs sont partagés par tous les credentials, contextes
et sessions du même principal canonique dans un tenant. Reconnexion, rotation du
credential et redémarrage du gateway ne rendent aucun crédit. Identités locale
et OIDC restent distinctes ; le produit ne déduit pas qu'elles désignent la même
personne.

Sans politique explicite, chaque capacité événements et octets de payload définit :

| Admission | Plafond ou réserve par défaut |
|---|---|
| Ensemble des reçus humains d'un tenant | Moitié de la capacité physique |
| Reçus humains d'un principal | Un huitième de la capacité physique |
| Réserve contrôle, protégée des lectures humaines | Un quart de la capacité physique |
| Réserve maintenance d'identité, protégée des écritures ordinaires | Un huitième de la capacité physique |

Les réserves se soustraient à l'occupation totale réelle ; les événements
ordinaires déjà présents consomment toujours de la place. La maintenance
d'identité comprend révocations et autres modifications autorisées : cette
réserve est elle aussi finie. Les octets sont ceux du payload UTF-8 préparé
exactement persisté, pas ceux de la réponse ni du fichier SQLite. Le plafond
physique de payload est 256 Mio ; `--max-events` règle celui des événements.
Les capacités automatiques trop petites pour représenter les budgets protégés
désactivent explicitement les nouvelles lectures humaines, sans relever le
plafond physique ni invalider le stockage existant. Les lectures exigent aussi
un schéma5–10 validé ; les lectures brokers exigent7–10, les lectures workers exigent10 et partagent les mêmes budgets humains cumulatifs.

[audit-capacity.json](examples/audit-capacity.json) donne les défauts résolus pour
`--max-events 100000`. Copier ce fichier dans le répertoire de contrôle privé
existant avec le mode0600. Pour le gateway local, ajouter
`--audit-capacity-file /absolute/private/ai-guard/audit-capacity.json` à sa
commande habituelle. Cette option est incompatible avec `--tenants-file` : avec
un registre, placer plutôt le même objet JSON dans le champ optionnel
`audit_capacity_policy` de chaque tenant concerné. Conserver les autres champs
et les chemins de données existants. Les commandes hors ligne `human provision`,
`resume-provision` et `apply` acceptent la même option de fichier. Employer la
même politique et le même `--max-events` voulus entre consommateurs ; le défaut
du plafond d'événements est `1000000` hors ligne et `100000` pour le gateway.
Changer la politique ne remet jamais l'usage enregistré à zéro. Le diagnostic
de démarrage indique politique résolue, provenance et empreinte, sans prouver
qu'il reste de la capacité disponible.

`429 audit_read_budget_exhausted` avec `retry_action:"contact_operator"` est un
refus cumulatif : ne pas relancer sur un timer. Conserver base, politique et
exports incomplets, puis faire examiner occupation et politique par l'opérateur.
Une augmentation explicite peut permettre des lectures seulement si capacité
physique et réserves le permettent. Aucune procédure d'archivage/purge qualifiée
n'est fournie ici ; supprimer des événements ou redémarrer ne constitue pas une
reprise. L'épuisement physique et l'indisponibilité du stockage obligatoire
restent503. Un profil voisin indisponible peut donner une réponse d'identité
partielle ; elle ne prouve pas que des droits ont disparu.

Le dashboard conserve ce 429 et l'action `contact_operator` lorsque son
précontrôle d'identité humaine rencontre ce refus, y compris lorsque la capacité
configurée désactive les lectures humaines. Il fournit un message local fixe,
sans en-tête `Retry-After`. Les pannes temporaires de transport et les erreurs de
précontrôle non reconnues restent 503 `identity_unavailable` avec `retry` ; elles
ne prouvent pas l'épuisement du budget durable.

L'admission SSE humaine permet quatre flux vivants par principal canonique et
24 flux humains par tenant, dans le total32 partagé existant. Huit places
restent disponibles aux flux techniques lorsque le pool humain est plein. Ces
bornes fixes sont distinctes des permis I/O Store et des budgets de reçus.
Fermer les flux inutiles avant de réessayer un refus de concurrence. Un Body
retenu garde ses permis jusqu'au drop, même après erreur terminale ou EOF
observé ; un client qui ne poll pas peut immobiliser sa part. Chaque poll
revérifie expiration et révocation. Ces bornes ne prouvent ni un plafond de tous
les sockets HTTP ni une éviction autonome.

## Autoriser la lecture nominative du registre workers

La capacité administrative distincte `read_worker_registry` exige la migration explicite9→10 et l’accord du contrôle sur le membership. Suivre la [lecture nominative des workers](WORKERS.fr.md) pour la procédure locale/OIDC, les budgets d’audit partagés, la restauration, les limites des données et l’écart obligatoire d’attribution personnelle. Aucun droit brokers/ressources existant n’accorde ce droit.
