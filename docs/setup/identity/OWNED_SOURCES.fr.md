# Sources humaines possédées : identité locale et ressources

Ce mode explicite du gateway rouvre une union possédée complète déjà existante. Le défaut reste `legacy`. La [validation logicielle datée](../../engineering/evidence/2026-11-13/human-sources-operator/README.fr.md) couvre l'adoption locale et les deux lectures adoptées sur une source Linux ARM64 identifiée ; elle ne qualifie ni production, ni runtime de worker, ni OIDC.

Suivre la [procédure d'adoption hors ligne des sources](../human-sources-maintenance/README.fr.md) pour préparer métadonnées/enveloppe privées, migrer l'union et publier le vrai manifeste de fermeture et l'activation du gateway. Ses cinq commandes sont `human-sources prepare`, `resume-prepare`, `apply`, `resume` et `check`. L'adoption exige un registre de tenants et des Stores13 existants, plus Auth4, configuration OIDCV2 et bundle public de confiance explicite si OIDC est choisi. Le coordinateur convertit l'union complète de Store13 à14 et d'Auth4 à5 ; les migrations monobases ordinaires s'arrêtent toujours à Store13/Auth4. Provisionner personnes, credentials et grants avant conversion avec la [procédure d'identité legacy](README.fr.md) ; l'adoption n'en crée aucun. Ne pas fabriquer de reçus, initialiser des bases de remplacement, relever les versions acceptées ou employer les fixtures comme procédure d'installation.

Après publication de l'activation complète par `apply` ou `resume`, l'utiliser pour la réouverture normale dans le lancement conteneurisé existant du gateway :

```text
gateway --human-sources-activation /control/human-preparation/activation.json
```

Conserver bind HTTP, TLS serveur et réglages de décision réels de l'installation. Ne pas combiner activation avec les options directes de sources, registre, credentials, révisions, capacité, initialisation ou OIDC. Le gateway reconstruit et rejoue la vraie union à chaque démarrage ; les fichiers d'activation seuls ne prouvent pas la disponibilité des sources. Arrêter tous les consommateurs avant préparation ou migration : `--confirm-control-stopped` est une attestation manuelle, sans fencing des anciens processus. Après interruption, conserver sur place sources, préparation privée, journal, sauvegardes et configuration concordante, puis employer l'étape de reprise correspondante du guide. `check` valide seulement fichiers/métadonnées, sans vérifier SQLite, authentification ou disponibilité.

Les options directes ci-dessous restent disponibles pour une installation dont manifeste/enveloppe retenus proviennent du vrai protocole de migration owned. Les employer comme alternative à l'activation, en conservant politique, montages, plafond d'événements et autres options réels :

```text
gateway --human-source-mode owned_sources
  --tenants-file /control/tenants.json
  --human-sources-manifest /control/existing-closure.json
  --human-credentials-envelope /control/existing-technical-credentials.json
  --human-control-config-revision <révision-contrôle-positive-réelle>
```

Ce sont des références vers des fichiers privés existants, jamais des tokens en arguments. Le démarrage ne génère ni manifeste ni enveloppe. Le registre donne les quotas et politiques effectifs. Le loader résout indépendamment les défauts et recalcule le digest public version1, sans credentials, digests de credentials ni chemins privés ; les physiques sont comparés séparément. Employer la révision conservée par l'opérateur, sans la deviner depuis une erreur. Avec OIDC, conserver `--oidc-config` et fournir `--human-oidc-mapping-revision` ; cette révision positive de fermeture est distincte de la chaîne `mapping.revision`. Fichier, limite, révision, format ou union absent/divergent refuse le démarrage. Les flags d'initialisation sont incompatibles. Conserver fichiers et commits connus après échec ; aucune migration, remise à zéro ou bascule legacy n'a lieu.

Employer le profil de présentation de confiance concordant :

```text
dashboard --shared-tenants --human-source-mode owned_sources
  --upstream http://gateway:8787 --front-dir /app/front
```

Conserver l'upstream interne et le répertoire statique réels de l'installation. Le repli sur un fichier viewer est incompatible. Le gateway reste l'autorité d'autorisation même si la présentation est mal configurée. HEAD moderne est refusé avant vérification viewer ou conversion en GET.

Le credential admis est le bearer local privé déjà provisionné. Le premier GET `human identity` vérifie sa connexion ; aucun endpoint password/login n'est ajouté. Employer les [commandes identité/ressources existantes](README.fr.md#consulter-avec-le-cli), `--credential-file` privé et un contexte réellement reçu. Seuls `GET /api/v1/human/identity`, `GET /api/v1/human/resources`, `GET /api/v1/human/events` et `GET /api/v1/human/events/recent` sont adoptés ; les deux lectures d’événements sont limitées aux métadonnées. L'identité décrit la source authentifiante ; en présence d'autres sources tenants, `contexts_complete=false` signifie que leurs memberships restent inconnus. La sélection cross-source est refusée. La page de ressources porte son vrai `read_receipt` durable ; un candidat digest ne fournit ni identité, ni droit, ni reçu.

La lecture récente du Journal porte seulement sur les métadonnées. Une page réussie avec `stream_mode:"snapshot_only"` et `live_cursor:null` identifie ce profil owned actuel ; le marqueur n’accorde aucun droit et ne décrit pas un autre serveur/profil. Les curseurs des pages plus anciennes conservent leur sélection autorisée et leur borne descendante. L’interface affiche l’heure de lecture et le reçu réels de la dernière page lue. Direct, contenu, détail d’événement et export restent indisponibles pour ce profil, même si les permissions correspondantes figurent dans la projection des droits. Après une page Journal réussie annonçant ce profil, Contenu et Export expliquent cette limite et sont désactivés ; les droits accordés restent inchangés.

Utiliser **Actualiser** pour relire les événements récents. Le Journal n’ouvre ni flux, ni boucle de relance, ni lecture périodique ; le retour dans l’onglet conserve la revalidation ponctuelle normale. Une revalidation en échec retire les données affichées, reçus et positions de pagination du snapshot. Une erreur transitoire dans le même contexte conserve la restriction de profil déjà établie ; changement d’identité/contexte/autorisation ou fermeture de session retire ce constat. Un503 n’établit jamais un autre profil. Une demande explicite de contenu refusée reste une erreur avec retour explicite vers Ressources, sans repli silencieux metadata. Mettre à jour gateway et dashboard ensemble. La preuve logicielle datée en tête de ce guide conserve son périmètre d’origine de deux lectures.

Les méthodes connues non adoptées humaines, contrôle, workload, brokers, relais et OIDC, dont détail d’événement, export et flux, renvoient503 explicitement avant extracteurs legacy, corps ou Store. OIDC sélectionné dans le manifeste reste sélectionné et retenu, avec handlers/bearers fermés ; il n'est jamais annoncé désactivé. Les GET status/profile existants « disabled » sont admis seulement si OIDC est absent. La santé consulte la mémoire ; elle ne prouve pas la disponibilité de chaque source. Ces lectures n’activent ni approbation, ni lancement, ni permission réseau, ni contrôle egress modernes.

Un mutex ou writer SQLite occupé chez A ne déclenche aucun SQL chez A lors de l'authentification/lecture de B. La propre source occupée renvoie503. Busy n'est ni absence ni mismatch permanent. Un mismatch physique/configuration/marqueur/incarnation réellement comparé conserve l'invalidation globale de l'owner. La revalidation physique couvre toujours l'union entière : syscall bloqué ou epoch Write partagé gardent leurs limites de disponibilité. L'index privé de candidats est borné avant accumulation à65536 entrées/16Mio de représentation ; le dépassement refuse toute publication complète. Ces bornes ne mesurent pas la RAM du processus.

Le runtime compare la vraie révision d'autorisation d'identité et le maximum de l'historique des bindings workload de sa propre source avant/après l'opération adoptée. Ces marqueurs couvrent les mutations d'identité et l'émission/rotation/révocation de bindings ; l'audit des lectures ne les déplace pas. Un mismatch comparé exige une reconstruction admission fermée. Ce n'est pas une génération universelle des writers : les autres restent fermés, et une mutation brute du stockage voisin ne peut être détectée instantanément sans l'observer.

Le serveur réserve son superviseur de cleanup avant builders/binds faillibles. Arrêt et Drop ferment admission humaine/contextes, arrêtent HTTP, observent les vrais joins HTTP, puis attendent usages blocking acceptés/en queue et epochs. Une erreur de drain reste un échec, avec l'erreur primaire de bind/serve éventuelle. Un drain non établi n'est jamais un succès ; les vrais handles d'usage/epoch restants retiennent owner et descripteurs. Terminaison du runtime/processus et panne d'hôte restent hors de cette garantie in-process.

Le `HumanApiClient.close()` local efface le token, annule les requêtes et écarte les réponses tardives. Il ne révoque pas le credential durable. Mutation moderne des credentials, découverte fraîche des memberships des autres sources et sessions/révocation SSO restent des exigences ouvertes. Le parcours d'adoption consigne ses résultats logiciels et ses [limites de qualification restantes](../human-sources-maintenance/README.fr.md#périmètre-de-validation). Arrêter les consommateurs et conserver configuration, bases, backups et preuves avant réconciliation ; ne jamais supprimer l'historique pour obtenir un démarrage vert.
