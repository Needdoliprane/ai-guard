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
  --target-version 5 --confirm-control-stopped
```

Le schéma5 ajoute le protocole d'audit durable des lectures humaines. Modifier `user_version` manuellement ne remplace pas la migration. Mettre à jour tous les lecteurs ensemble : les anciens binaires refusent les nouveaux schémas. Aucun downgrade, restauration, activation ou compte n'est créé automatiquement. Provision/apply acceptent les schémas4 et5 ; les lectures HTTP humaines exigent5 et renvoient `migration_required` avant d'émettre leur audit de lecture sur une ancienne base.

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
  --target-version 5 --confirm-control-stopped
docker run --name ai-guard-offline-human-alice --pull never \
  --user 1000:1000 --network none --read-only --cap-drop ALL \
  --security-opt no-new-privileges:true --cpus 1 --memory 384m --memory-swap 384m --pids-limit 128 \
  --tmpfs /tmp:rw,noexec,nosuid,nodev,size=16m,mode=1777 \
  --mount "type=volume,source=$AIGUARD_AUDIT_VOLUME,target=/data,volume-nocopy" \
  "$AIGUARD_CONTROL_IMAGE_ID" human provision --data /data/audit.sqlite \
  --tenant-id "$AIGUARD_TENANT" --name Alice --organization engineering \
  --output-dir /data/human-alice --confirm-control-stopped
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
  --confirm-control-stopped
```

La commande prépare les fichiers privés `credential`, `recovery.json` et `completion.json`. Elle génère un credential aléatoire sans l'afficher. Compte, binding du credential et membership initial sont validés dans une transaction avec leur audit. La sortie présente des identifiants et chemins, jamais le credential. Les capacités initiales sont lecture des métadonnées, lecture du contenu et export ; les grants de ressources restent nécessaires. `--capabilities read-metadata` permet de limiter ce choix.

Après une interruption ou un résultat ambigu, préserver exactement la préparation et la base, puis reprendre :

```sh
"$AIGUARD_BIN" human resume-provision \
  --data "$AIGUARD_DATA" --tenant-id "$AIGUARD_TENANT" \
  --output-dir "$AIGUARD_PRIVATE/human-alice" \
  --confirm-control-stopped
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
  --confirm-control-stopped
```

Désactiver compte, credential, membership ou grant par une nouvelle mutation auditée, en conservant son identité et en augmentant sa révision. Pour ce grant resté en révision1, la révocation utilise un nouvel UUID d'opération, `expected_revision:1`, grant `revision:2` et `active:false`, avec mêmes tenant, organisation, ressource et propriétaire. Un conflit de révision est un refus : examiner le record opérateur conservé plutôt que deviner. Ne pas modifier directement les projections ou réutiliser le credential d'une autre personne. Après changement de droits, reprendre un contexte actuellement proposé. Créer un compte ne lance aucun worker et ne délègue pas le rôle runner.

## Consulter avec le CLI

Redémarrer uniquement les services concernés avec le binaire à jour et les données existantes. Indiquer l'origine du gateway, sans chemin `/api`. HTTPS est vérifié ; HTTP local exige une IP loopback littérale, par exemple `127.0.0.1`. Ce client désactive proxy ambiant, redirections et retries automatiques.

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
|429, capacité ou taille de page | Suivre l'action de retry/réduction indiquée, sans élargir les permissions. |
| Audit ou stockage indisponible | Les lectures exigeant un audit sont refusées. Réparer capacité/stockage avant reprise. |

Le client SSE reprend seulement après une trame complète validée. Un retry peut dupliquer des événements ; posséder un curseur ne prouve ni réception ni téléchargement. La révocation bloque les lectures futures, pas les octets déjà reçus. Identité locale, identité OIDC, identité de workload et authentification fournisseur IA restent indépendantes.
