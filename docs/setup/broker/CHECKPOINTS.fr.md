# Publier les checkpoints déclarés vers le contrôle commun du tenant

[English](CHECKPOINTS.md) · [Présence](PRESENCE.fr.md) · [Installation](README.fr.md)

Ce parcours étend le publisher de présence authentifié. Il transmet des
références bornées de heads locaux confirmés, sans conversation, commande,
contenu de workspace ni inventaire runtime. Plusieurs brokers configurés
peuvent viser le même gateway du tenant, avec identités Broker, grants,
autorités et états locaux privés distincts. Le gateway choisit le tenant par
la preuve vérifiée ; une requête ne choisit aucun autre tenant ou broker.

C'est le parcours d'intégration du schéma 12. Le build doit comprendre ensemble
Store, routes gateway, transport de contrôle et source du service broker.
Une source préparée ne qualifie pas un déploiement multi-hôtes. Régressions
logicielles, matrice des transports/runtimes et qualification restent séparées
dans les preuves d'ingénierie.

## Configurer chaque broker de confiance

Après arrêt de tous les writers du contrôle et brokers, l'étape suivante
explicite depuis une base existante en schéma 11 est :

```sh
"/absolute/path/to/agentsandbox" migrate-control \
  --data "/absolute/private/tenant.sqlite" \
  --tenant-id "<authenticated-tenant-id>" \
  --backup "/absolute/private/tenant-before-checkpoints-v12.sqlite" \
  --target-version 12 --max-events 100000 --confirm-control-stopped
```

Employer la capacité d'audit réelle de l'installation et une destination de
sauvegarde neuve. La migration conserve les anciens records, le namespace, les
réservations retenues et quotas ; elle ajoute le marqueur du schéma 12 sans
activer la supervision. Conserver son rapport et la sauvegarde vérifiée en 11.
Une nouvelle tentative sur 12 ou sur un chemin de sauvegarde existant refuse ;
ne pas supprimer source ou sauvegarde pour obtenir un autre résultat. Les
schémas antérieurs exigent leurs migrations consécutives et sauvegardes neuves.

1. Arrêter le writer de contrôle selon la maintenance normale, conserver sa
   base et réaliser la migration 11→12 explicite avec sauvegarde. Un ancien
   binaire doit refuser 12 avant de l'ouvrir comme une ancienne base. Les
   records de présence V1 et les plans fermés 1/2/3 gardent leurs formats.
2. Préparer le [credential Broker et son grant existants](PRESENCE.fr.md).
   Conserver namespace du tenant, broker, credential, incarnation et autorité
   unique exacte autorisés pour ce profil Registry2. L'activation du publisher
   ne crée aucune preuve ; aucun worker ne la reçoit.
3. Préparer deux répertoires privés distincts hors projet/export : `state_dir`
   pour les heartbeats et `checkpoint_state_dir` pour les checkpoints. Ils
   existent déjà, en 0700 avec ascendance privée ; aucun ne contient l'autre.
   Préserver les deux lors d'une reprise.
4. Copier [presence-checkpoints.json](examples/presence-checkpoints.json) dans
   un fichier régulier privé 0600, avec les IDs et chemins approuvés. `version:2`
   active explicitement l'exigence de source des checkpoints. Les trois champs
   `checkpoint_*` sont obligatoires et refusés en présence V1, même avec null.
5. Utiliser ce fichier dans le `presence_file` existant du profil Registry2,
   puis la commande existante `broker serve --config /chemin/absolu/registry.json`.
   L'adaptateur du service fournit directement ses heads locaux confirmés ;
   aucun fichier de snapshot opérateur, shell ou flux worker ne fait autorité.
   Une config V2 sans cet adaptateur est refusée, sans retour silencieux aux
   seuls heartbeats.

La source lit seulement un writer Full déjà admis par ce profil pour son vrai
journal. Activer V2 n'ouvre aucun writer, n'accepte ni ne lance de job. Sans cette
capacité retenue, l'état checkpoint reste inconnu tandis que le heartbeat peut
continuer. Installation, profil, tenant, namespace et gateway canonique du
journal doivent correspondre exactement au profil configuré.

Les budgets checkpoint sont explicites et indépendants du heartbeat :
`checkpoint_max_operations` vaut 1–32768 ; `checkpoint_max_state_bytes` est
positif, au plus 134217728. L'exemple prévoit 64 Mio par stream retenu, pas une
garantie mesurée disque/RAM. La place du reçu est réservée avant POST.
L'épuisement exige une action opérateur : aucune purge, archive, rotation ou
nouvelle incarnation ne remet implicitement la capacité d'audit à zéro.

HTTPS, confiance AC publique explicite, nom de serveur vérifié, absence de
proxy/redirection/repli, revalidation des fichiers privés et permits partagés
suivent le [contrat de présence](PRESENCE.fr.md). Aucun worker ne reçoit clé
privée d'AC, preuve runner, jeton SSO ou socket moteur.

## Publication et fraîcheur

Le gateway refuse les métadonnées contenant un credential connu du contexte
de la requête, même s'il a la syntaxe d'un identifiant ou hash. Il vérifie la
demande avant persistance et retient une réponse dont les identifiants ou
engagements devraient être masqués. L'erreur reste générique, sans substituer
un autre hash ni réécrire les preuves. Une réponse POST retenue ne prouve pas
un rollback : conserver et réconcilier la même opération. Un GET peut devenir
indisponible après rotation si une donnée conservée correspond désormais à un
secret connu ; préserver les preuves et enquêter sans les purger.

Les nouvelles routes sont POST `/api/v1/brokers/checkpoints` et GET
`/api/v1/brokers/checkpoints/self` via le transport `/control` existant. Le corps
fermé et versionné est limité à 32768 octets : UUID d'opération, incarnation,
séquences précédente/courante exactes, autorité et au plus 100 références
uniques triées `(job_id, revision, checkpoint_hash)`. Le snapshot porte
`has_more` ; ce champ ne rend jamais la couverture complète.
Les deux routes checkpoint exigent la preuve workload Broker authentifiée et
son grant courant correspondant ; ni preuve runner/viewer ni connexion humaine
ne la remplacent. La lecture du registre reste distincte : les lecteurs
techniques runner/viewer utilisent GET `/api/v1/brokers` ; les utilisateurs
nominatifs utilisent GET `/api/human/brokers`, avec contexte tenant autorisé et
permission explicite `read_broker_registry` (schéma 7 ou ultérieur). Ces lectures
n'autorisent ni écriture checkpoint ni accès à un autre broker.

Avant POST, le publisher commit un Pending immuable : mêmes opération,
identité, séquence et contenu. Une réponse perdue rejoue cette opération après
revalidation du binding/grant Broker courant. Un contenu divergent pour cette
identité est un conflit. L'état source plus récent attend l'ACK du Pending ;
les valeurs intermédiaires du watch peuvent être regroupées, tandis que le
journal broker conserve l'historique local complet. Ce stream n'audite pas
chaque transition locale de checkpoint.

Une source `None` signifie indisponible/pas encore capturée : aucun checkpoint
vide ni fraîcheur renouvelée. Une mise à jour source peut déclarer une liste
vide ; cela ne prouve pas zéro activité runtime.
À chaque intervalle configuré (5–150 secondes), le service demande
`list(None, 100)` par la FIFO de ce writer existant. Un vrai ACK réussi est
nécessaire même pour connaître une page vide. Une capacité Full absente ou
étrangère, une lecture refusée ou tainted, ou un contrôle en arrêt, fenced ou
indisponible laisse la source inconnue. L'adaptateur transmet au plus 100 heads
durables dans l'ordre unique du writer ; une entrée sans head omise ou une page
suivante impose `has_more:true`. Il relit même sans changement des heads. Les
lectures attendent derrière le vrai travail du writer ; une lecture source est
attendue par profil, sans nouvelle garantie de latence ou fairness. Les mises
à jour intermédiaires peuvent être regroupées. Ces snapshots acquittés ne sont
pas une observation atomique des mutations ultérieures ou de l'activité runtime.
Les références restent
`declared`, la réception gateway `observed`, avec `coverage:declared_partial`,
y compris pour `has_more:false`.

Le contrôle calcule l'expiration depuis sa réception, le bail et l'expiration
du grant. Un ancien report devient périmé même si le heartbeat reste courant.
La révocation/le remplacement du grant conserve les métadonnées historiques.
L'incertitude d'horloge est explicite. Aucun report ni heartbeat ne produit
une observation worker, un inventaire complet, un quota, une nouvelle échéance
d'exécution ou une libération de réservation.

## Refus, interruption et reprise

Transport/indisponibilité conservent le même Pending : reprises initiales
100/250/500 ms puis cadence au plus 30 s, sans permit réseau retenu pendant
l'attente. L'arrêt interrompt celle-ci ; aucun lancement métier n'est ajouté.
Un refus d'identité, grant/binding, séquence, horloge, protocole ou état privé
arrête la publication, sans identité/grant neuf ni repli runner automatique.

Le service vivant conserve le lecteur source et les deux publishers jusqu'à
leur vraie jointure, y compris le travail filesystem déjà admis et les erreurs
terminales. Annuler un observateur d'arrêt conserve les mêmes tâches pour une
jointure ultérieure. L'arrêt normal attend avant de libérer son rôle local ;
un filesystem bloqué peut maintenir cette attente. Attendre n'établit ni borne
physique de collecte, ni fencing ou reprise après perte du processus. Abandonner
un wrapper demande seulement l'arrêt, sans prouver une jointure. Préserver
l'état après interruption et le réconcilier par les procédures existantes ; la
fin du processus ne prouve pas la fin des travaux runtime ou réservations.

Reprendre avec les configurations et états conservés. Un Pending non résolu
ne traverse pas un changement de binding/grant. Un stream entièrement acquitté
peut reprendre après rotation explicite de la même identité de credential et
grant correspondant ; son checkpoint historique garde son ancien binding.
Un répertoire vide de remplacement ne peut adopter un report déjà committé
pour cette incarnation. Conserver les preuves et réconcilier l'écart.

Utiliser la vue commune autorisée du tenant. Absence, péremption et couverture
`declared_partial` restent visibles auprès des faits jobs/workers ; ces
références ne sont ni des exécutions observées indépendamment ni un inventaire
complet du parc. Déconnexion/reprise de vrais hôtes, cycle TLS/PKI et charge 100
workers demandent leurs propres qualifications. La borne de 100 références de
heads par snapshot ne qualifie ni 100 workers, ni une matrice de transports, ni
une capacité soutenue.
