# Lecture humaine nominative du registre workers

Cette vue d’administration lit le registre workers/brokers existant du tenant. Elle exige une identité locale nominative ou un jeton d’accès OIDC admis, un contexte **administration** proposé et la capacité explicite `read_worker_registry`. Elle couvre les métadonnées d’infrastructure du tenant authentifié ; `read_metadata`, `read_tenant_metadata`, `read_broker_registry`, administration du realm et nom de compte ne donnent aucun accès implicite. La session humaine ne reçoit aucun credential runner/viewer/broker.

## Migrer explicitement le stockage existant

Arrêter tous les consommateurs et redémarrages automatiques. Préserver le volume tenant existant, les credentials, les ancres runtime/enforcement et les claims consommés. Employer une même version du produit pour gateway, dashboard, broker, clients workload/natifs et maintenance hors ligne. Les anciens binaires refusent le schéma10 inconnu. La migration n’active aucune exécution et n’accorde/réactive aucun droit.

Un schéma9 demande exactement9→10. Les versions antérieures exigent chaque migration suivante avec sa propre sauvegarde neuve vérifiée ; setup crée toujours5 et refuse les capacités de registres d’infrastructure. Suivre [5→6→7](../workloads/README.fr.md#activer-la-lecture-nominative-du-registre-brokers), puis les cibles explicites8 et9 livrées sur la même base, avec destinations de sauvegarde distinctes, avant10. Ces marqueurs de stockage intermédiaires n’autorisent ni ne qualifient une activité Kubernetes. Ne pas éditer `user_version`, réutiliser une sauvegarde ou initialiser un stockage de remplacement.

Depuis une7 validée, employer d’abord deux fois la même commande de maintenance ci-dessous : `--target-version 8` avec une nouvelle sauvegarde `before-held-creates-v8.sqlite`, vérifier le rapport7→8, puis `--target-version 9` avec une nouvelle sauvegarde `before-held-policy-v9.sqlite` et vérifier8→9. Conserver les mêmes `--data`, `--tenant-id`, capacité réels et frontière de consommateurs arrêtés. Arrêter sur tout refus ; ne jamais sauter de cible ou réutiliser une destination de sauvegarde déjà créée.

Pour une base9 validée, remplacer les chemins, tenant et capacité par ceux de l’installation existante :

```sh
"/absolute/path/to/agentsandbox" migrate-control \
  --data "/absolute/private/ai-guard/audit.sqlite" \
  --tenant-id "<configured-tenant-id>" \
  --backup "/absolute/private/ai-guard/before-human-workers-v10.sqlite" \
  --target-version 10 --max-events 100000 --confirm-control-stopped
```

Le stockage conteneurisé emploie son volume existant inspecté et les UID/GID réels du contrôle, sans socket/réseau et tous consommateurs arrêtés ; voir la [maintenance d’identité hors ligne](README.fr.md#préparer-le-stockage-et-lautorité). Le chemin tenant préparé est `/data/installation/tenants/<tenant>/audit.sqlite`, à distinguer de toute autre installation.

Vérifier le code de sortie et le rapport : `from_version:9`, `to_version:10`, chemin de sauvegarde et séquence/hash de genèse. La migration vérifie une sauvegarde9 exacte neuve, puis engage atomiquement `control.identity.workers_access.genesis` et la version10. Elle n’ajoute aucune table/index, ne réinitialise aucun budget, ne change aucun membership et n’active aucune exécution. Un refus de capacité d’audit laisse la source9 et son head inchangés ; conserver toute sauvegarde déjà créée.

Pour restaurer, conserver la source migrée et restaurer la sauvegarde vérifiée dans une nouvelle destination privée selon la procédure de fencing/reprise existante. Une sauvegarde9 reste9, sans capacité de lecture workers ; le nouveau binaire ne la transforme pas en10. La réouverture d’une10 vérifie marqueur, projection d’identité et audit. Ni restauration ni réouverture à froid n’autorisent le rejeu des starts ou le contournement d’une supervision inactive.

## Accorder seulement l’accès tenant approuvé

Employer la [procédure de mutation membership typée](README.fr.md#autoriser-la-lecture-nominative-du-registre-brokers), en ajoutant explicitement `read_worker_registry` au membership visé. Préserver principal, tenant, organisation, autres capacités relues, ancienne révision et révision suivante. Employer un nouvel UUID d’opération et le `--mutation-file` privé existant ; un `actor` appelant est refusé. Conserver les mêmes `--max-events` et politique d’audit que l’installation. Seul le contrôle de confiance/hors ligne peut introduire ou réactiver ce droit tenant ; un administrateur d’organisation peut le conserver/retirer, jamais l’ajouter/réactiver.

OIDC exige aussi le plafond explicite sur claims du jeton d’accès vérifié. Ceci est un **fragment de règle unique** dans les `gateway.mapping.rules` existantes relues, pas une configuration complète ni une commande de création de rôle :

```json
{
  "claim_path": ["resource_access", "ai-guard-api", "roles"],
  "equals": "guard-worker-reader",
  "capabilities": ["read_worker_registry"]
}
```

Employer le client/claim/rôle réellement approuvé ; modifier une règle correspondante au lieu de la dupliquer. Incrémenter `gateway.mapping.revision` et redémarrer avec les mêmes données/configuration. Une capacité administrative ne va jamais dans `default_capabilities`. Membership actif et plafond sont nécessaires ensemble ; un rôle seul ne donne aucun droit et ne sélectionne aucun autre tenant. Aucun manifeste distribué de découverte/par défaut n’accorde ce droit automatiquement.

Se réauthentifier, choisir le contexte administration proposé, puis ouvrir **Workers**. Un contexte limité à ce droit ouvre cette vue sans lecture ressources, événements, brokers, export ou SSE. Retirer le droit par nouvelle révision auditée du membership ou nouveau plafond/connexion. Un refus de contexte/identité retire page, curseur et détail ; une réponse tardive ne les rétablit pas. Une révocation ne retire pas les octets déjà reçus.

## Portée, erreurs et obligations ouvertes

`GET /api/v1/human/workers` accepte seulement `limit` (défaut25,1–100) et un curseur opaque. La réponse fermée est bornée à256Kio ; la traversée à1000pages/16Mio dans la validité du contexte (au plus300s). Chaque page réunit autorisation, projection et reçu d’audit dans une transaction vérifiée, suivie d’une réauthentification avant publication. Les pages vides partagent les budgets humains durables F1 tenant/principal. Le reçu prouve l’enregistrement d’une lecture, ni livraison ni exécution runtime. Les curseurs sont propres à Workers, l’identité, le contexte, la version d’autorisation et la limite.

La whitelist contient identifiants/révision, état/quota du contrôle, session déclarée, limites runtime/ressources sélectionnées et dernières métadonnées runtime observées. Elle exclut commandes, argv/env/contenu, références/noms de credentials, chemins/noms hôte/volume/conteneur et erreurs arbitraires. Les associations brokers sont la propriété historique d’un label d’autorité, éventuellement postérieure au fait worker ; elles ne prouvent pas quel broker/incarnation physique a exécuté ou inspecté. Inventaire runtime/usage inconnus restent inconnus ; aucun total tenant déduit d’une page ni facture n’est présenté.

Chargement, vide, erreur, interdit et lecture périmée restent distincts. Une panne ultérieure de transport/stockage garde la dernière page autorisée datée/périmée ; un refus d’identité/contexte/permission retire page et détails ouverts. L’actualisation de la page visible a lieu au plus toutes les30secondes, sans délai garanti de notification. Aucun canal SSE worker implicite n’est accordé. Sur `429 audit_read_budget_exhausted`/`contact_operator`, arrêter les lectures automatiques : conserver audit/politique/reçus, réparer la capacité puis utiliser le retry explicite après intervention. Un503 ne devient jamais un registre vide ou un repli viewer.

**La vue personnelle des workers reste un écart obligatoire ouvert.** Un `launch.session_id` déclaré n’établit aucune propriété humaine. La chaîne authentifiée initiateur/demande/délégation/admission doit être implémentée et qualifiée avant d’annoncer cette vue complète. Cette tranche d’administration ne qualifie ni transport des brokers distants, parcours unifié cloud/personnel complet, confinement runtime réel, capacité100 ou release production. Les fixtures logicielles ne qualifient pas un déploiement réel.

### Identifiants historiques, dates et codes de sortie exacts

Les quatre identifiants workers humains (`worker_id`, `request_id`, `attempt_id`, `session_id`) conservent le format UUID canonique du contrôle, y compris nil ; le curseur keyset Workers accepte la même position. Les identités broker, reçus de lecture et credentials gardent leurs propres contrats de validation.

Les dates runtime portent des états distincts `known|absent|unknown` : `observed_started_at_status`, puis `observed_at_status`, `started_at_status`, `finished_at_status` dans `last_observation`. `known` signifie une valeur fournie interprétable, sans preuve de chronologie cohérente, de fin réelle ou de couverture indépendante. Les valeurs connues sont en UTC canonique avec nanosecondes conservées ; les absences valent null/`absent`, les débuts runtime ininterprétables null/`unknown`, sans texte invalide brut. Le classificateur historique de début traite les débuts non positifs comme inconnus. `observed_at` n’a aucune nouvelle borne d’epoch positive ; une fin RFC3339 valide à epoch0 reste connue. Un rendu UTC connu doit tenir dans le domaine RFC3339 publié à quatre chiffres et restituer par aller-retour le même instant et les nanosecondes. Une source obligatoire valide dont l’UTC sort de ce domaine devient null/`unknown` ; un `observed_at` brut malformé reste une erreur de stockage, et `absent` n’est jamais permis pour cette source obligatoire. Une chronologie contradictoire conserve l’état InDoubt/quota du contrôle et la durée inconnue. L’UI ne calcule aucune durée depuis ces dates.

Le `exit_code` humain vaut null ou une **chaîne** décimale signée canonique bornée par i64 (`-9223372036854775808` à `9223372036854775807`). Le front valide avec BigInt et affiche la valeur exacte, sans conversion Number. Les enregistrements techniques WorkerV1/Observe originaux et leurs contrats entiers/dates restent inchangés.
