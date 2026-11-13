# Sources humaines possédées : identité locale et ressources

Ce mode explicite du gateway rouvre une union possédée complète déjà existante. Le défaut reste `legacy`. La validation logicielle de ce changement est **NOT_RUN** jusqu'à admission du gel en QA ; elle ne qualifie ni production, ni moteur de conteneurs, ni OIDC.

L'opérateur doit déjà détenir un vrai manifeste privé de fermeture, son enveloppe technique concordante, le registre des tenants, les Stores14/15 et, si choisi, AuthLedger5 avec configuration OIDCV2 et bundle public de confiance. Ce changement ne fournit pas le producteur hors ligne de ce manifeste/enveloppe. Les migrations publiques monobases existantes s'arrêtent à Store13/Auth4. Ne pas fabriquer de reçus, initialiser des bases de remplacement, relever leur version acceptée ou employer les fixtures comme procédure d'installation. Provisionner personnes et grants avant conversion avec la procédure d'identité hors ligne disponible ; le bootstrap owned complet reste un écart explicite.

Pour une installation dont manifeste/enveloppe retenus proviennent du vrai protocole de migration owned, ajouter ces options au lancement conteneurisé existant du gateway, en conservant sa politique, ses montages, son plafond d'événements et ses autres options :

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

Le credential admis est le bearer local privé déjà provisionné. Le premier GET `human identity` vérifie sa connexion ; aucun endpoint password/login n'est ajouté. Employer les [commandes identité/ressources existantes](README.fr.md#consulter-avec-le-cli), `--credential-file` privé et un contexte réellement reçu. Seuls `GET /api/v1/human/identity` et `GET /api/v1/human/resources` sont adoptés. L'identité décrit la source authentifiante ; en présence d'autres sources tenants, `contexts_complete=false` signifie que leurs memberships restent inconnus. La sélection cross-source est refusée. La page de ressources porte son vrai `read_receipt` durable ; un candidat digest ne fournit ni identité, ni droit, ni reçu.

Les routes connues non adoptées humaines, contrôle, workload, brokers, relais, événements et OIDC renvoient503 explicitement avant extracteurs legacy, corps ou Store. OIDC sélectionné dans le manifeste reste sélectionné et retenu, avec handlers/bearers fermés ; il n'est jamais annoncé désactivé. Les GET status/profile existants « disabled » sont admis seulement si OIDC est absent. La santé consulte la mémoire ; elle ne prouve pas la disponibilité de chaque source. Ces deux lectures n'activent ni approbation, ni lancement, ni permission réseau, ni contrôle egress modernes.

Un mutex ou writer SQLite occupé chez A ne déclenche aucun SQL chez A lors de l'authentification/lecture de B. La propre source occupée renvoie503. Busy n'est ni absence ni mismatch permanent. Un mismatch physique/configuration/marqueur/incarnation réellement comparé conserve l'invalidation globale de l'owner. La revalidation physique couvre toujours l'union entière : syscall bloqué ou epoch Write partagé gardent leurs limites de disponibilité. L'index privé de candidats est borné avant accumulation à65536 entrées/16Mio de représentation ; le dépassement refuse toute publication complète. Ces bornes ne mesurent pas la RAM du processus.

Le serveur réserve son superviseur de cleanup avant builders/binds faillibles. Arrêt et Drop ferment admission humaine/contextes, arrêtent HTTP, observent les vrais joins HTTP, puis attendent usages blocking acceptés/en queue et epochs. Une erreur de drain reste un échec, avec l'erreur primaire de bind/serve éventuelle. Un drain non établi n'est jamais un succès ; les vrais handles d'usage/epoch restants retiennent owner et descripteurs. Terminaison du runtime/processus et panne d'hôte restent hors de cette garantie in-process.

Le `HumanApiClient.close()` local efface le token, annule les requêtes et écarte les réponses tardives. Il ne révoque pas le credential durable. Mutation moderne des credentials, découverte fraîche des memberships des autres sources, sessions/révocation SSO et bootstrap opérateur hors ligne restent des exigences ouvertes. Arrêter les consommateurs et conserver configuration, bases, backups et preuves avant réconciliation ; ne jamais supprimer l'historique pour obtenir un démarrage vert.
