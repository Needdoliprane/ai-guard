# Décisions d'industrialisation

## 3 novembre — corrections de l'audit et périmètre des preuves

- L'export publie des fichiers0644 et omet les noms sensibles connus et les
  métadonnées actives ciblées ; le scan conserve les observations brutes.
  Les entrées omises restent validées et bornées. L'avertissement générique
  doit pouvoir être écrit avant publication, sans exposer de noms ; ce stderr
  n'est pas un reçu de livraison au client. Aucun assainissement universel ni
  conservation implicite des bits exécutables.
- L'erreur du store authentifié reste fermée. Les erreurs voisines n'empêchent
  plus l'identité saine ; les collisions lisibles restent refusées, y compris
  après provisioning. Une base illisible ne prouve aucune absence de collision
  et n'authentifie pas ses credentials. `contexts_complete` rend la projection
  partielle explicite ; gateway, dashboard et clients stricts se mettent à jour
  ensemble. Aucun changement de stockage ou migration de données par ce lot.
- Upload et échange ingress ont des budgets distincts. Les limites16/64,
  256Kio,5s/2s sont celles du code actuel, pas de nouvelles options publiques.
  Le runner conserve son arrêt sur erreur de suivi. La proposition60s reste
  soumise séparément ; aucun test de ces budgets ne qualifie le confinement.
- L'initiation SSO initiale reste volatile, bornée et expirable jusqu'à une
  preuve d'access token vérifiée. L'intention est alors durable avant lecture
  du compte/memberships et admission. Cela remplace, pour le login initial,
  l'ACK durable avant échange décrit dans la note historique du2novembre ;
  le refresh authentifié conserve son intention durable et son fencing.
  Une erreur d'enregistrement de cette intention refuse avant lecture tenant ;
  une erreur ultérieure ne signifie pas qu'aucune lecture n'a été auditée.
  Le format et les historiques existants restent conservés ; ce correctif ne
  restaure pas un journal déjà plein et ne qualifie pas le TTL initial complet.

Ces choix restent dans l'architecture autorisée. Les preuves et limitations
sont liées aux gels et plateformes dans [AUDIT_FOLLOWUP.md](AUDIT_FOLLOWUP.md).

### Lot D — périmètre des permis de lecture, décision retenue

Ruling : distinguer les lectures de profils/pages et la résolution des
métadonnées d'identité, avec un plafond tenant pour chacune. La cible est
Read4 et Discovery4 par tenant, sous les bornes globales existantes ; le contrôle
runner conserve sa réserve. Read ne doit pas être présenté comme couvrant
toutes les transactions d'authentification. Un lease doit correspondre au
store réellement consulté ; une source connue distincte du contexte doit
être couverte, sans double acquisition si les deux sont identiques.
Après résolution de la source humaine, `authenticate_local_digest` relève
déjà de Read dans l'extracteur initial. Un runner résolu revalide son propre
store sous son lease Control existant, sans dépendance à Discovery ; cette
réserve doit rester effective même lorsque les métadonnées sont saturées.

La découverte doit pouvoir vérifier les collisions même si le budget des
pages d'un voisin est saturé. Une lecture de métadonnées non admissible reste
une indisponibilité explicite, jamais une absence de binding. Les collisions
lisibles restent refusées dynamiquement ; la politique B d'erreurs voisines
après authentification d'une source saine reste applicable. Si aucune source
ne peut être authentifiée, la requête reste refusée. Aucun index de démarrage
ne devient une autorité et aucun nouveau stockage/service n'est introduit.

Ce choix ajoute une réserve bornée de métadonnées au plafond de pages ; il ne
promet ni équité complète entre tenants ni disponibilité de la découverte
quand son propre budget est saturé. Les profils OIDC restent des lectures
Read, auditables et sujettes aux règles de couverture partielle. Les tests
doivent observer les transactions effectivement exécutées sous saturation,
pas seulement le statut HTTP final. Les trois reproductions ont atteint
leurs assertions de transactions hors permis sans oracle temporel, puis le
correctif a passé ses 59 fonctions ciblées, sa revue et le contrôle composé.
Cette décision est implémentée dans `b168784`, sans qualification globale
de disponibilité, de performance ou des runtimes.

## 2 novembre — navigation publique après le callback OIDC

Le retour réel Keycloak révèle callback303 puis complete403 : [Fetch Metadata tient compte de toute la chaîne de redirections](https://www.w3.org/TR/fetch-metadata/#redirects), même après retour à l'origine du dashboard. Le dashboard autorise donc la navigation GET exacte vers `/auth/oidc/complete` lorsque SSO est configuré, sans query et avec une seule valeur `navigate`/`document` pour chaque métadonnée correspondante. Son contenu reste le fichier statique public index.html, avec CSP, frame-ancestors/FrameDENY et no-store ; garder cette page sans données d'identité, de session ou de secret.

La remise des tokens reste un POST protégé par l'origine exacte, les métadonnées et le cookie corrélé ; callback, PKCE, nonce, consommation unique, droits et tenant restent sous l'autorité existante du gateway. Les méthodes/actions, API métier, fetch/subresources et configurations SSO désactivées conservent le refus cross-site. Ce correctif garde les frontières et routes approuvées. Régression RED observée,6tests/fmt/Clippy strict PASS et revue indépendante approuvée ; nouvelle image et recette réelle exigées avant qualification du trajet. [Preuves et état](VALIDATION.md#2-novembre--installation-conteneurisée-sso-neuve).

## 2 novembre — sessions OIDC durables et portée de l'audit

Choix d'implémentation de l'orchestrateur dans la frontière SSO approuvée : audit privé d'authentification distinct des bases métiers, empreintes de tokens séparées par domaine, générations positives sans wrap et transitions admission/refresh pending/rotation/fermeture atomiques avec leur audit. Une intention est consommable une seule fois ; son replay ne déclenche aucun nouvel échange fournisseur. Le CLI conserve son PKCE natif mais demande un ACK d'intention durable avant ses échanges : le gateway observe la demande et valide le token remis, sans prétendre avoir observé le nonce ou l'échange CLI. Aucun bearer, refresh token ou ID token brut n'est persisté.

Une lecture métier vérifie la session exacte active/non expirée et l'intégrité de l'audit d'authentification, puis persiste son reçu obligatoire dans le Store du tenant. Elle n'ajoute pas une écriture d'authentification redondante. La saturation du quota d'append d'authentification bloque les nouvelles mutations concernées, sans invalider à elle seule une session existante ; indisponibilité/intégrité invalide ou échec d'audit métier refusent la lecture. Logout efface toujours l'état mémoire local et distingue les résultats d'audit et d'IdP.

Après redémarrage, le registre mémoire vide n'admet ni anciennes sessions ni contextes implicitement ; les empreintes persistantes interdisent la réadmission du même access token déjà vu. Un rollback complet du journal à travers un redémarrage ne peut pas être détecté par ce seul journal : aucune garantie anti-rollback externe n'est annoncée. Restauration/reprise et véritable parcours IdP restent des qualifications ouvertes. Ces décisions décrivent le lot en cours, pas un SSO terminé.

## 1er novembre — propriété SSO approuvée par le mainteneur

Le mainteneur choisit explicitement le gateway Rust comme propriétaire des transactions PKCE et de la validation OIDC. Le dashboard relaie uniquement des routes d'authentification fixes ; seul le gateway rejoint le réseau interne Keycloak lorsque le profil de découverte est activé. Aucun nouveau service, secret client distribué, accès cloud ou changement DNS implicite n'est autorisé par cette décision. Elle remplace la proposition ancienne de propriété PKCE dans le dashboard, sans annoncer de connexion SSO livrée.

Le [contrat OIDC](SSO_OIDC.md#propriété-de-la-transaction) conserve clients publics Web/CLI distincts, stockage éphémère borné, protections du callback et audit obligatoire avant remise. Les durées, le renouvellement, la révocation et le stockage privé de l'audit de connexion doivent être figés avec l'implémentation ; aucune identité non rattachée ne choisit un tenant pour son audit. Le propriétaire et le raccord sont approuvés ; leurs effets restent NOT_RUN.

Le mainteneur demande aussi d'examiner le token exchange. La [RFC8693](https://www.rfc-editor.org/rfc/rfc8693.html) distingue sujet et acteur délégué ; elle n'accorde pas elle-même de permission métier. L'échange standard [Keycloak](https://www.keycloak.org/securing-apps/token-exchange) exige actuellement un client confidentiel demandeur, ne fournit pas une chaîne de révocation des access tokens et présente sa délégation comme preview. Il reste une option à évaluer pour les déploiements compatibles, sans dépendance obligatoire en local. Une session applicative opaque et une capacité d'exécution interne sont des contrats distincts d'un échange RFC8693. Aucun jeton humain ou credential runner ne rejoint un worker ; aucune de ces alternatives n'est déclarée implémentée par cette note.

## 1er novembre — diagnostic Jev compatible et masque effectif

Conserver le code d'erreur public et le downcast Rust existants ; transporter les observations dans un échec interne possédé par la requête, jamais dans un dernier résultat mutable partagé. L'API ajoute seulement un diagnostic fermé optionnel, limité à l'endpoint configuré. Aucune observation de relais ne devient une preuve du modèle ; corps et détails de transport restent exclus. La configuration décrit les paramètres fixes et le prompt réellement admis : un builder commun et un contrôle du masque sur toute la trame fixe avant POST refusent les collisions, sans retirer les masques utilisateur. Pas de nouvelle dépendance, migration, service, retry ou délai augmenté.

## 1er novembre — protocole durable des lectures humaines

Le vocabulaire `control.identity.access` reçoit un schéma5 explicite et le marqueur unique `control.identity.human_access.genesis`, lié au namespace, au genesis identité4 et à la tête précédente. Le SQL ne change pas ; les anciennes versions refusent5. Migration4→5 avec backup/head contrôlés, aucun downgrade ni démarrage automatique. Le défaut CLI de migration reste2 ; les opérations humaines offline conservent4 ou5 pour le vocabulaire de mutation inchangé, tandis que la lecture humaine auditée exige5.

Les credentials humains sont séparés de runner/viewer et des authentifications fournisseurs IA. Le provisioning prépare d'abord les fichiers privés/reçus, puis compte, binding et membership dans une transaction auditée idempotente. L'acteur de maintenance vient du filesystem/UID effectif ; un `actor` JSON est refusé. Permissions et tenant sont relus depuis l'identité authentifiée, sans conversion en clé de lecture tenant entier. OIDC/PKCE et délégation restent des contrats obligatoires distincts, sans prétendre à un SSO livré.

## 1er novembre — pagination humaine et version d'autorisation opaque

Le candidat de lecture humaine doit permettre les précédents et les retries pendant son contexte, sans exposer de compteur global. La revue a constaté que retirer les ancêtres de curseurs cassait la navigation après trois pages. Le contrat de ce lot non publié remplace ce cache de pages par des curseurs AEAD au sein d'un registre borné de contextes :4096 au total,64 par credential/principal, durée absolue300s, saturation explicite sans éviction vivante. Chaque contexte possède une clé CSPRNG et un compteur de nonce partagés uniques, sans wrap ni réinitialisation ; aucune position d'audit n'est publique. La dépendance directe prévue `ring = "=0.17.14"` est déjà transitive et verrouillée ; le wrapper reste à implémenter, tester et relire.

La révision globale d'autorisation reste privée ; les DTO portent une `authorization_version` opaque. Les lectures revalident les droits et enregistrent durablement leur propre reçu avant restitution. Rejouer un curseur ne prouve pas la réception des données. Le SSE reprend après la dernière trame complète validée, avec doublons possibles au retry ; il ne promet pas le même futur lot encore inconnu. Les budgets de traversée ne constituent pas un quota contre l'exfiltration. Cette décision précise un contrat en cours ; elle ne qualifie ni authentification humaine, ni SSO, ni cloud.

## 1er novembre — identité humaine indexée et version de stockage explicite

Choix d'implémentation de l'orchestrateur dans la base SQLite par tenant existante : domaine humain distinct de `runner`/`viewer`, permissions personnelles/administratives explicites et projections indexées, avec audit atomique. Le lot en cours prépare un schéma4 exact et une migration3→4 explicite avec sauvegarde vérifiée ; aucun objet ni nouveau domaine métier n'est dissimulé sous le schéma3. Les lecteurs de supervision ne reconnaissent4 qu'avec les capacités et invariants effectivement conservés ; ni comparaison `>=3` ni réactivation lors d'une ouverture/migration. Les formats des plans fournisseurs restent distincts et inchangés.

Un journal d'identité reconstruit à chaque accès a été écarté : son coût croîtrait avec l'historique et un plafond arbitraire pourrait bloquer des installations valides. Les scopes devront être revalidés à la lecture, y compris après révocation ; aucune ancienne session ou clé technique ne devient implicitement une identité humaine. Le registre broker aura ses propres bindings authentifiés, sa migration coordonnée et son producteur réel ; une identité moteur ne permet aucune attribution rétroactive. Ces décisions organisent les lots, sans livrer OIDC/PKCE, le lancement humain délégué ou la fédération par leur rédaction.

## 1er novembre — propriété native indépendante du listener

Le RED S0 montre qu'une lease locale au futur serveur peut être relâchée avant ses owners IPC/natifs. La même Arc/FD est désormais retenue par State/acteurs et un owner natif borné qui possède les handles. Progression Weak, garde de sortie et vrai join avant toute attente STOP ; fin idle/NotStarted connue, Cancel conservé. Les erreurs ou handles consommés par join échoué restent inconnus : lease et owners restants retenus jusqu'à fin du processus, sans nouvelle autorité, éviction ou preuve de retraite. Le support non mutatif est détaché à l'abandon, distinct des handles acteur/progression qu'il continue à posséder. [Sept fonctions ciblées, checks et limites](VALIDATION.md#1er-novembre--s0-r2-green-ciblé). Cette correction du même binaire ne modifie ni architecture réseau, protocole, ledger, deadline ou installation OS ; D5 demeure ouvert.

## 1er novembre — import TLS explicite vers des volumes de contrôle

Le démarrage réel refuse les projections dont le filesystem n'a pas de sémantique ACL qualifiée ; aucun élargissement de la politique commune n'est retenu. Le [contrat d'import TLS](TLS_MATERIAL_PROVISIONING.md) conserve validation hôte par FD, reader strict et contrôle Rust existant, avec paquet stdin fermé et préparateur ponctuel non privilégié vers deux volumes neufs. CA seule pour gateway, identité pour relais ; aucun secret au modèle/worker. Pas de nouveau service ou stockage métier, d'émission PKI, d'adoption/rotation ou d'activation implicite. Implémentation, tests ciblés et revues source sont depuis vérifiés ; la qualification runtime reste distincte et ouverte. Ce choix ne clôture G11 ni le parcours Jev intégré.

## 1er novembre — forme stricte des données historiques dans STOP3

Le nouveau transport3 conserve les schémas et comportements2. Ses records propres sont fermés, Option explicitement null/valeur. Les V1 embarquées suivent exactement leur sérialiseur historique : chaque membre émis est requis, null compris ; les cinq `skip_serializing_if` restent omis lorsqu'ils valent None. Cette exception résout la contradiction du premier contrat entre uniformité des Option et conservation exacte de RestoreV1. Le premier gel R4 qui injectait des nulls sur des champs omis n'a pas été exécuté ; il est conservé NOT_RUN, remplacé par R4b avant toute reproduction.

Le [contrat STOP3](BROKER_SUPERVISION.md#transport-stop3-complet--contrat-du-candidat-r4b-non-qualifié) exige refus récursif des clés dupliquées avant insertion en map, décodage typé depuis les octets originaux, témoin de forme comparé à Serialize(T), puis validateurs/MAC. Les nombres/lexèmes ne sont pas normalisés par une Value préalable. La feature `serde_json/raw_value` est autorisée seulement si nécessaire, sans nouveau paquet/version et avec lock à recouper. Cette décision ne valide pas le candidat, ses quatre écarts de revue, le ledger ou Full ; les tests restent des preuves à obtenir.

## 1er novembre — liaison TLS explicite du juge local

Le profil HTTP hôte reste disponible. Un profil TLS distinct réutilise le même modèle et le même relais Rust, dans deux réseaux internes : gateway↔relais sur judge_link, relais↔modèle sur inference. Aucun port modèle TLS publié, réseau de worker ajouté ou montage TLS sur le relais d'entrée du contrôle. Seule l'ancre publique atteint le gateway ; chaîne et clé leaf restent sur le relais modèle, clés privées d'AC chez l'opérateur. Le dernier saut HTTP interne n'est pas annoncé chiffré. Aucun DNS ou magasin de confiance OS n'est modifié.

La confiance Jev devient une configuration typée additive, conservant les anciennes API et la vérification chaîne/SAN/validité, sans proxy, redirect, retry ou repli permissif. Le lecteur de projections0444 root/euid est un port étroit distinct de PrivateFile0600, sans relâcher ses contrôles. Ce lot a un worktree dédié depuis58309ed ; implémentation, tests TLS négatifs, bornes/shutdown et revue restent à établir. Une connexion TLS ne qualifie ni le modèle à autoriser ni le cloud ; Allow local reste Review. Aucune nouvelle architecture métier n'est introduite.

## 1er novembre — compatibilité du futur protocole de retrait

R8 exige la forme exacte de Challenge/Auth/Grant/Status/Watch2, avec StopsOnly et Unverified(recovery_transport_only). Ajouter directement Verified, coverage ou Full à ces records fermés casserait le vrai client2. Root retient une négociation V3 authentifiée sur une connexion non encore accordée, après une préface2 inchangée, dans le même IPC et le même binaire. La solution alternative de versions appariées est plus simple mais ne fournit pas cette continuité. Aucun second listener ni migration implicite n'est ajouté.

Le [contrat de supervision](BROKER_SUPERVISION.md) fixe le premier message Negotiate3, le transcript exact et son domaine HMAC distinct. Les échanges2 ne gagnent ni champ3 ni permission Full. Après tout Grant, version et rôle restent immuables ; un refus3 ne devient pas une authentification2 compensatoire sur ce flux. Full3 exigera un ledger Complete et sa propre connexion explicitement authentifiée, sans conversion Stops→Full.

Le premier brouillon de test qui attendait coverage3 dans Status2 n'a jamais été exécuté ; il est corrigé avant Cargo. Le nouveau test vise seulement la négociation3 absente, tout en conservant les positifs Stop2. Auth3/Grant3/Status3, diagnostics Partial et retraite restent à implémenter et qualifier ; une Challenge3 seule ne livrera pas ces capacités. Les défauts de harnais relevés avant exécution sont traités séparément, sans modifier le comportement applicatif R8.

## 1er novembre — reprise STOP distincte de l'admission

La reprise des arrêts autorisés doit fonctionner lorsque l'admission ou le ledger de retrait est dégradé, sous réserve d'une racine, d'une authentification et d'une lease encore vérifiables. La première verticale ajoute un canal IPC explicitement limité aux arrêts, avant le scan lourd et la demande Full. Son authentification lie le rôle demandé ; restauration et reconnexion ne passent plus par un Register métier ou une permission Prepare. Le protocole2 annonce `offered_roles=["stops_only"]` lorsque l'admission est fermée. Un secret IPC absent dans une ancienne autorité n'est pas régénéré comme réparation.

La première implémentation, en validation, conserve le protocole1 et son `EnforcerClient` Full legacy. Le nouveau `EnforcerStopClient` et le mode `broker serve --recovery-only` demandent explicitement le protocole2 ; aucune conversion Stops→Full ou capacité FullV2 fictive n'est ajoutée. L'identité du ledger reste `Unverified`, sans reçu de finalité. La nouvelle dépendance `hmac = "=0.12.1"` réutilise digest0.10 déjà verrouillé : MAC standard, domaines client/serveur distincts, challenge et rôle liés, vérification via `verify_slice`. Le wire et la preuve legacy sont conservés ; aucun credential runner/fournisseur n'entre dans ce handshake privé.

Un retry doit rester possible dans le même epoch et acquitter la version exacte de la cible, sans effacer un Cancel ajouté pendant un ancien essai. La génération numérique d'invalidation ne prouve pas l'ordre des commandes : leur préfixe vient de la chaîne Journal effectivement vérifiée et conserve ses receipts exacts. Un vieux Cancel valide reste acquittable sans remplacer le plus récent ; même identité avec contenu divergent reste un conflit. Ni réservation reçue, ni thread démarré, ni transport vivant ne constitue un effet runtime ou une preuve de retrait. L'échec certain de démarrage d'un thread conserve une réservation réessayable ; un thread vivant avec runtime indisponible ne doit pas être dupliqué.

Après reproduction de l'abandon multi-Arm, R7 possède le lot indépendamment de son waiter : préparation avec les lecteurs retenus hors mutex, puis rétention groupée des cibles valides sous un seul verrou mémoire avant leurs nouveaux RPC. Une erreur de cible ne saute pas les suivantes. Les confirmations attendues restent liées à l'epoch et à la révision désirée courants, avec les tokens et les seize permis partagés existants ; le simple enqueue ou un ACK historique ne devient pas un résultat réussi. Cette possession survit à la disparition du waiter dans le broker vivant, sans prétendre survivre à un crash du broker hors reprise du journal durable.

Les revues imposent un ordre par Arm : classer la projection, rapprocher un Pending/final exact avant toute restauration de cette même cible, puis restaurer les RequiredStop restantes. Les cibles indépendantes continuent pendant le défaut d'un autre job. Après un epoch de reprise partielle ayant pu émettre un STOP, l'ancien Q ou le certificat Closing ne prouve pas le drainage du nouveau RPC : refuser la finalisation récupérée si cette intervention ne peut être exclue. Ce refus conserve la sûreté mais laisse la convergence à résoudre ; une annonce de transport n'est pas une observation d'effet.

Le ledger proposé utilise des deltas engagés par Arm, avec inventaire reconstruit et tombstones attendues, plutôt que des snapshots complets répétés. Le transport de reprise conserve déjà une lease gardien spécialisée O_RDWR, sur la même description/flock, sans parser ni réécrire son contenu. Le cache mutable Parsed/Tainted, les formats, la rétention, la quiescence et leurs preuves restent à implémenter ; PrivateFile général reste immuable. Le [contrat consolidé de supervision](BROKER_SUPERVISION.md) conserve aussi les budgets broker avant Pending, la monotonie V5 et les refus de reprise ambigus. Les premiers RED portent sur les API existantes et les vraies primitives Unix, sans moteur ni types futurs fictifs. Aucun G09/D5 ou gate n'est fermé par cette direction ; les chemins historiques ne sont pas convertis silencieusement.

## 1er novembre — provenance des builds et intégration du profil Docker

Les validations destinées à publication utilisent une cible Cargo dédiée au checkout, réutilisable pour ses changements suivants, sans partage entre worktrees de contenus différents. Une incohérence source/bibliothèque dans l'ancienne cible partagée est conservée sans attribution rétrospective ; une compilation neuve isolée établit seulement les résultats de son propre gel. Aucun clean, effacement de preuve ou relèvement de délai produit n'est utilisé comme correction. Les modes privés des captures ne doivent pas retirer le bit de traversée aux répertoires ni le bit d'exécution aux binaires.

Le profil Docker accepté est intégré avec ses formats V2 couplés, anciens formats inchangés et choix opérateur explicite. La revue a conduit à corriger la projection STOP sous blob surdimensionné puis l'indirection privée des métadonnées ; ses [gels et résultats](evidence/2026-11-01/docker-launch-profile/README.md) restent distincts. L'oracle EOF utilise désormais des barrières causales au lieu d'un sommeil arbitraire ; ses budgets de harnais n'expriment pas une nouvelle latence ou échéance produit. Les retraits de gardien et la réactivation sûre du contrôle restent des exigences ouvertes, sans faux terminal ou libération de quota.

## 1er novembre — manuel bilingue du produit complet et essais auxiliaires locaux

Le mainteneur demande un manuel du produit complet, organisé par contexte avec exemples de configuration : local, fournisseurs IA, Keycloak, Qwen local, combinaison locale et entreprise. La rédaction adopte cette cible complète ; preuves de fonctionnement et écarts du code restent dans les documents d'ingénierie. L'hypothèse éditoriale ne constitue ni une implémentation ni une qualification de production.

`README.md` est l'entrée anglaise et `README.fr.md` son équivalent français ; les guides de `docs/setup/` suivent la même paire. Un changement de contrat modifie dans le même lot les deux langues, exemples et commandes concernés. Les `.env.exemple.*` indiquent leur consommateur : Compose via `--env-file`, ou entrée privée importée explicitement pour les fournisseurs. La CLI ne source pas ces fichiers. Contrôle, IdP et inférence utilisent des projets Compose distincts dont les noms sont conservés sur tout le cycle de vie.

Les essais auxiliaires Keycloak, Qwen et générateur d'AC ne sont plus distribués. Les fichiers déjà suivis sont retirés de l'index et ignorés, avec conservation de chaque copie locale et de l'historique. Les synthèses datées restent dans les dossiers de preuves ; captures/instruments détaillés demeurent locaux. Les tests de sécurité, API, broker/runtime et autres comportements du produit restent suivis. Secrets et données existants ne sont ni remplacés ni supprimés.

## 1er novembre — une commande Rust de PKI de test

Le [générateur local](../demo-pki.md) répond au choix du mainteneur sans nouveau service ni dépendance NAS. OpenSSL/LibreSSL est appelé par chemin absolu sous environnement vidé, avec arguments séparés et délais ; son namespace, propriétaire, ACL et identité sont contrôlés avant chaque appel. Sortie exclusive0700 et fichiers0600, clés d'AC conservées privées, aucune installation de confiance/DNS. La table constante de sorties et sa collecte complète portent l'invariant des deux accès positionnels ; une évolution doit conserver cette correspondance.

Le TLS du test utilise Rustls sur une socket loopback retenue, après constat de l'incompatibilité de bind du s_server LibreSSL hôte. `tokio-rustls` reste seulement une dépendance directe de test, déjà transitive/verrouillée ; le lock n'ajoute qu'une arête, pas un composant ou paquet. [Preuves, quatre FAIL, revue et limites](evidence/2026-11-01/demo-pki/README.md). Aucun endpoint HTTPS produit ou cycle PKI de production n'est livré par ce générateur.

## 1er novembre — Keycloak, PKI de test simple et backend local partagé

Le mainteneur demande davantage de parallélisme, notamment la mise en place et les essais Keycloak pendant le chantier du broker, et confirme que toute occasion utile de déléguer des tâches indépendantes doit être saisie sans nouvelle relance. Cette obligation est inscrite dans [AGENTS.md](../../AGENTS.md), avec périmètres de fichiers, coordination des dépendances et économie des tests. Le profil reste une découverte explicitement activée, isolée des tenants et credentials existants, avec les quatre comptes du [contrat SSO](../sso.md). Le DNS local nécessaire peut être configuré en préservant les entrées existantes ; sur ce Mac, la résolution relève de `/etc/hosts`. Aucun changement de nom de machine n'est nécessaire à ce seul besoin.

Pour les certificats de test, le mainteneur retient une solution simple : un outil/script générant localement AC racine, intermédiaire et certificat serveur, avec clés privées protégées et refus d'écraser une sortie existante. L'automatisation du projet reste en Rust ; elle fournit les éléments utilisables par les clients et services de test, sans dépendance au NAS ni nouvelle infrastructure PKI. Les usages de confiance et les preuves TLS restent explicites.

Le profil du modèle local essayé pour Jev est réutilisé pour tester l'inférence d'entreprise : [Qwen2.5-0.5B-Instruct Q4_K_M](evidence/2026-10-30/local-judge/README.md), révision/quantification/contexte et image déjà consignés. Son protocole et son budget sous1Go ont passé ; son jugement de sécurité a échoué. Le backend peut être partagé, avec routes/profils, contexte, permissions, quotas et traces séparés ; il ne devient pas une autorité de décision. Existence actuelle du service, format compatible OpenAI/Claude, confiance TLS et parcours des clients restent à vérifier, sans repli cloud ni téléchargement d'un autre modèle implicite.

## 1er novembre — factoriser sans annoncer un gain non mesuré

Les checkpoints V2/V3/V4 réutilisent leurs bytes déjà validés, avec mêmes contrôles, caps, ordre de refus, formats et domaines. Les trois goldens sont relevés avant code puis figés ; aucune autorité en cache, barrière ou durée produit n'est supprimée. Le timeout d'un test outbox est reproduit puis séparé de son oracle fonctionnel : seule cette fixture utilise60s déjà admises par Writer, tandis qu'un canari conserve la sémantique timeout/queue/replay. [Neuf fonctions, checks, revue et preuves](evidence/2026-11-01/journal-cost/README.md#résultats-finaux-du-lot).

La comparaison du même graphe ne démontre aucun gain de latence ; le coût principal et H10 restent ouverts. Ni le build release ni ces temps debug ne qualifient un runtime, une capacité100 ou un retrait D5.

## 1er novembre — profil Docker lié au parent accepté, contrat approuvé

Le [contrat concret relu](evidence/2026-11-01/docker-launch-profile/README.md) reste dans le broker/runtime Rust et le journal existants. Nouveau choix opérateur explicite versionné, paire bindingsV2/Accepted2 strictement couplée, profil minimal exact privé et durable avant acceptation, scope authentifié/parent/intention/invocation engagés, seed distinct et contrôle juste avant le start préarmé. Les formats V1, credentials, plans fournisseurs et empreintes historiques restent exacts ; aucun changement implicite de choix au replay. Le profil ne constitue pas une nouvelle preuve d'identité.

Les métadonnées de preuve et le blob sont distincts : la reprise STOP n'ouvre pas ce dernier, mais audit/budget/stockage nécessaires à un append restent obligatoires. Un échec de ces prérequis conserve l'inconnue et les projections/enforcer prouvés, sans faux ACK. Aucun sceau contre mêmeUID/root, aucun témoin de complétion/fencing et aucune permission de retirer des slots n'en découlent. Le premier test de parser est écrit dans un checkout séparé ; code et qualification du profil restent à livrer. Aucun moteur, cloud, PKI ou service OS lancé par cette décision.

## 1er novembre — mesurer sans supprimer les barrières du journal

Après H10, séparer le coût des lectures/validations, des appels exacts `File::sync_all` et de la queue du Writer avant d'optimiser. Le probe est uniquement `cfg(test)`, opt-in et ignoré par défaut ; il exécute les corps applicatifs originaux sur un vrai journal neuf. Champs fermés, racine conservée seulement en mémoire, aucun contenu/credential/identifiant dans les métriques. Les temps inclusifs se chevauchent et le timeout reste coopératif, sans annulation d'une commande Writer déjà acceptée.

Les huit mesures aux heads0/8 confirment les décomptes de source mais montrent aussi une variabilité au même head ; sept fonctions ciblées passent. Elles ne reproduisent pas le graphe H10 de31révisions/866540octets, ne mesurent pas la capacité100 ni la performance CPU release et ne prouvent aucune cause de l'échec. L'observation passive du gardien attribué mesure un CPU propre non nul, pas le coût global ou par Arm. [Preuves, erreurs de harnais et limites](evidence/2026-11-01/journal-cost/README.md).

Aucune suppression de Full pré/post, CAS, floors, contrôles FD, synchronisations ou enforcement n'est autorisée par ces mesures. La prochaine correction doit être étayée sur le graphe plus riche et garder ses refus négatifs. Le coût des slots retenus ne constitue jamais une preuve de retrait sûr ; D5, reprise et G09 restent ouverts.

Journal de décisions datées, distinct des preuves de livraison. Les compléments les plus récents remplacent les choix antérieurs incompatibles ; les entrées historiques restent conservées. Les exigences actuelles d'identité, de configuration et de découverte sont regroupées dans le [contrat canonique](../sso.md), celles des vues dans [ui.md](../ui.md). L'avancement est dans [CURRENT_STATE.md](CURRENT_STATE.md), avec les résultats de [VALIDATION.md](VALIDATION.md).

## 31 octobre — configuration indépendante des catégories de clients

La liste d'usages personnels, administrés, OIDC et sans Internet fournie par le mainteneur est non exhaustive. Couvrir les combinaisons par des axes indépendants et des profils documentés : topologie d'exécution, identité ai-guard, périmètres et permissions, client IA, abonnement/API/inférence interne, réseau/confiance et exploitation. Keycloak, Entra ID et les autres fournisseurs OIDC configurés ne déterminent pas le choix du fournisseur IA. Le rôle externe `admin-ai-guard` et les quatre comptes restent propres aux valeurs de découverte ; les mappings de rôles/claims sont configurables ailleurs. Le mode sans SSO conserve une identité locale et un parcours simple, sans IdP obligatoire.

Le [contrat de configuration et ses parcours](../sso.md#configuration-par-axes-indépendants) fixe les exigences, les diagnostics et les incompatibilités : pas de dépendance à un service public dans un profil strictement sans Internet, pas de repli silencieux ni de désactivation des invariants. La configuration effective et ses priorités sont validées côté contrôle ; les choix utilisateur restent bornés par les délégations. Documentation de cible uniquement, aucune combinaison supplémentaire n'est déclarée livrée ou qualifiée.

Complément IHM : une même application adapte ses vues utilisateur et administration aux permissions et configurations effectives. L'administrateur doit disposer d'une [synthèse unifiée à 360°](../ui.md#vues-utilisateur-et-administration-unifiée--cible-à-livrer) de son périmètre autorisé, avec accès aux détails, provenance et fraîcheur, sans dashboard par worker ni exposition implicite d'autres clients. Contrats et agrégation Rust, droits sur données/actions/exports et mise à jour du frontend font partie d'un même chantier à implémenter et qualifier ; le front `viewer` existant n'est pas présenté comme une livraison de cette cible.

## 31 octobre — images construites par le client et quatre comptes de découverte

Complément du mainteneur : fournir les Dockerfiles et la configuration documentée ; construction, registre et distribution interne des images appartiennent au client. Aucun engagement de publication d'images préconstruites. Le profil d'inférence interne doit porter URL, modèle, format d'API compatible OpenAI précisé, adaptation Claude, credential et chaîne publique d'AC, sans repli Internet ni désactivation TLS. [Contrat fonctionnel](../sso.md#inférence-interne-et-déploiement-sans-internet), à implémenter et qualifier.

La liste de découverte est remplacée par quatre comptes : `admin/admin` administre le realm ; `admin-ai-guard/admin-ai-guard` reçoit le rôle applicatif exact `admin-ai-guard` dans les claims ; `user-1/user-1` et `user-2/user-2` sont des utilisateurs normaux. Ce choix remplace les propositions `user_admin` et `ai-guard-admin`. Le rôle est validé et borné côté Rust, distinct de l'administration Keycloak et du contrôle `runner`. Les valeurs connues restent propres au profil de découverte loopback explicitement activé. Documentation uniquement : aucun realm, compte ou mécanisme SSO n'est créé par cette décision.

## 31 octobre — un échec observé ne doit pas devenir une absence d'activité

H10 conserve une FailureObserved locale mais aucun événement batch consultable, puis atteint600s d'attente. Étendre le rapport de `Execution.failure` aux erreurs sans provenance spécialisée : `broker_step=failure_observed` décrit seulement l'observation dans la phase courante, avec cause et effet inconnus. Les diagnostics natifs/runtime/broker reconnus restent prioritaires. La classification spécialisée peut rester partielle ; le rapport public a un repli fermé, sans sérialiser ni formater l'erreur arbitraire.

Utiliser le masquage/outbox/HTTP/ACK existant, sans journal parallèle ou dépendance. Une erreur de livraison peut encore empêcher le rapport ; elle reste signalée opaque. Le rapport n'annonce ni succès d'une commande dont le résultat est inconnu, ni terminal, arrêt, fencing ou libération de quota. La couverture concerne les erreurs parvenues à ce handler, pas panic/abort, ouverture refusée ou timeout externe. L'IO de publication est intentionnelle après erreur ; aucune IO, attente ou lecture d'horloge supplémentaire dans le callback de succès. Des contextes fermés autour du commit/reload ownership, de l'inspection et de l'échange Created localisent la frontière sans annoncer un POST/ACK ou une cause. API, événements v1 et valeurs historiques restent conservés ; seuls des labels fermés sont ajoutés.

Les prochaines décisions D5 doivent tenir compte du [cycle actuel des slots](evidence/2026-10-31/broker-runtime/d5-source/retirement-analysis.md) et des [sources Moby au commit observé](evidence/2026-10-31/broker-runtime/d5-source/moby-source.md). Cette lecture ne qualifie aucun moteur : retrait absent, Prepared/ObserveOnly distincts, requête tardive et réutilisation d'un CID comme nom sur un moteur administrable restent à prouver. Aucun nettoyage de ressources ni retrait de gardien n'est autorisé par la seule observation d'absence.

## 31 octobre — Draft acquitté et origine du snapshot préclaim

H9 a réellement publié `slot_phase_mismatch` avant claim, sans sample local de l'erreur. L'expiration reste une hypothèse. Ajouter aux seuls retours d'échec concernés la phase, la génération, prepared_once et la clock validée du slot helper déjà lu. L'origine `helper_slot` reste distincte de `native_sample` ; une clock invalide est omise et signalée, une absence reste inconnue. Aucun nouveau sample, RPC ou travail externe sur succès, aucun texte arbitraire, terminal, fencing ou quota déduit de ces données.

Le port privé de mutation du runner retourne un Draft lié à l'opération, au scope et au head du reçu Writer réussi, avec le candidat exact soumis. Une erreur ou un résultat inconnu ne retourne aucune donnée confirmée ; un conflit recharge et réapplique. Le replay ACK, chargé strictement, garde `fresh=false`. Vérifier le contexte avant Register puis réutiliser ces données retire seulement la relecture Full caller après Register. Le head est historique, sans garantie latest ni permission de start ; une révision devenue obsolète peut provoquer un refus conservateur. Les mutations ultérieures conservent Full/CAS, pending/ACK séparés, outbox, Cancel, planchers, bindings et synchronisations. Aucun cache de capacité ni changement persistant/public. Les copies ont un coût ; aucun gain temporel réel annoncé avant essai.

À la demande du mainteneur, porter le budget global du harnais à600s pour préparation/reprise, sans modifier Arm worker5s, deadline d'origine et oracles de faute/arrêt. Le harnais consulte au plus une fois par seconde le premier diagnostic via le filtre API existant et ne conserve que huit métadonnées validées. Sa limite globale n'est jamais une rallonge d'exécution autorisée. [Preuves et limites](evidence/2026-10-31/broker-runtime/README.md).

## 31 octobre — préclaim diagnostiqué et adaptation lifecycle sous le verrou existant

H7 atteint une Arm worker ACKée puis une erreur locale avant claim, hors de la couverture D3. Ajouter les étapes fermées `broker_step` et les diagnostics Register/BeforeClaim : aucune chaîne d'erreur arbitraire sérialisée, aucun sample inventé. La factory Register valide les métadonnées uniquement sur erreur ; `register` reste inchangé. Le Ready public partage le même corps et conserve ordre des gardes, codes, horloge, santé, génération, état et échéance. Ces données restent informatives avec effet inconnu et n'accordent aucune permission de start.

Le writer effectuait un Fullload d'adaptation V3/V4, puis le Journal rechargeait intégralement avant mutation et après publication. Déplacer seulement cette adaptation dans un port interne du Journal sous son mutex existant : une mutation lifecycle passe de trois Fullloads à deux, sans cache ni retrait de préload, postload, fsync, CAS, floors ou recontrôle des bindings. Préserver les événements/ACK V4 exacts et sa règle writer `expected==head`, même au rejeu obsolète ; les ports publics gardent refus de downgrade et contrats historiques. L'élimination de cette duplication est établie en source, sans revendication de gain mesuré ni de correction de la deadline réelle. [Preuves du chantier](evidence/2026-10-31/broker-runtime/README.md).

## 31 octobre — diagnostic typé sans nouvelle autorité de lancement

Les deux premières tentatives Docker du broker permanent échouent après claim durable mais avant Running ; `worker/unavailable` ne permet pas de distinguer Prepare, dernier garde et transport runtime. Ajouter des contextes typés privés aux points de refus existants et un payload version1 `broker.batch.failure`, livré par le chemin V4 de masquage/enqueue/HTTP/ACK déjà présent. Aucun texte arbitraire d'erreur, argv, credential ou configuration n'est sérialisé. Une erreur inconnue ne reçoit pas de cause inventée ; transport Prepare inconnu et refus explicite restent distincts. Le sample d'horloge est celui déjà lu par le contrôle, parfois absent, sans faux horodatage de l'erreur.

Conserver le wire IPC et ses codes, les formats du journal, l'unique permission de start, l'Arm/deadline d'origine et l'adjacence garde→spawn. Aucune vérification, nouvelle lecture d'horloge, attente ou IO n'est ajoutée au chemin de succès. Le diagnostic est informatif, avec `runtime_effect=unknown` ; il ne certifie ni absence d'effet, ni arrêt, ni fencing et ne libère aucun quota. Une livraison incomplète reste signalée sans prétendre que l'événement est absent ou durable. Les tests logiciels et la revue sont distincts de l'émission réelle, encore NOT_RUN. [Résultats et limites](evidence/2026-10-31/broker-runtime/README.md).

## 31 octobre — acteur STOP restreint et diagnostic de reprise

Conserver un seul acteur, une seule file bornée et le même FD de lease par profil. Son admission choisit une capacité métier complète ou une capacité STOP restreinte ; cette dernière ne permet ni chargement métier, ni réservation, ni permission de lancement, ni conversion publique vers le writer complet. La restauration transmet la lease retenue à cet acteur sans fenêtre de déverrouillage. Les planchers et identités déjà observés sont conservés, y compris le hash des rejets : le transfert sous la même lease ne réinitialise ni leur classe ni leur contenu accepté.

La seule nouvelle mutation admise par la capacité STOP est `CancelRequested`, construite depuis la chaîne confirmée et publiée par le mécanisme CAS/fsync commun. Les données du runner, la supervision, les événements et les reçus antérieurs restent inchangés. Les collisions de commandes, événements et inputs d'export sont refusées. Une anomalie de schéma dont les octets et entrées sont tous comptables peut conserver cette mutation ; une taille inconnue, un inventaire tronqué ou un parcours non sûr refuse la nouvelle publication. Un Cancel déjà durable garde sa possibilité de livraison aux seules cibles prouvées. La voie métier garde ses lectures strictes avant **et après** publication, y compris des bindings fournisseur.

Un résultat après enqueue perdu reste inconnu : conserver son UUID et permettre sa résolution/reprise sans nouvelle invalidation de génération ni effacement par un diagnostic partiel. Le refus d'une nouvelle action métier reste appliqué pendant cette ambiguïté. L'authentification fraîche runner, tenant, namespace et protocole reste nécessaire avant tout accès au profil. Une autorité de supervision inactive ferme les actions métier, mais ne retire pas la possibilité d'enregistrer un arrêt authentifié ou de lire ses métadonnées.

Le RPC local additif `RecoveryStatus` et le CLI `broker recovery-status` ont une réponse distincte de `Status`. Ils exposent le head confirmé, les identités des annulations et les armements d'origine, le mode de capacité, les inconnues et la provenance `confirmed_journal`. `runtime_evidence=not_queried` interdit de les interpréter comme une observation d'arrêt ou une libération de quota. Ils ne créent pas un namespace absent. Ils nécessitent cette version du service ; un ancien service refuse la commande inconnue. Aucun endpoint HTTP ni changement de format persistant n'est introduit. La [validation du lot](evidence/2026-10-31/stop-only-actor/README.md) sépare logiciel et qualification moteur.

## 31 octobre — rejeu causal des annulations, checkpoint44324a7

Cette décision décrit le checkpoint précédent. Les cas de premier Cancel et d'acteur dégradé alors ouverts sont traités en logiciel par le lot ci-dessus, avec qualification moteur toujours requise.

La régression du handler reproduit un Cancel durable dont la première livraison échoue puis dont le rejeu ne retente rien. Corriger dans les composants existants : chaque commande conserve une frontière éphémère immuable, égale au nombre d'opérations de supervision à sa première apparition confirmée. Seules les Arms nées avant cette frontière et actuellement acquittées sont ciblées ; un ACK tardif d'une ancienne Arm reste inclus, un export postérieur reste exclu. Aucun nouveau champ JSON, domaine de hash, claim, Arm, permission de lancement, échéance ou quota n'est créé par ce rejeu.

Une projection STOP typée du writer vivant réutilise le vérificateur du reader de récupération, sans ouvrir le blob de credentials ni construire de job métier. Elle partage l'Arc du FD de lease déjà acquis, conserve les planchers anti-retour et les bornes d'observation du writer, synchronise ses métadonnées et leurs parents avant retour. La projection est limitée au job demandé : elle ne certifie pas l'inventaire des siblings, les quotas physiques ou la complétude globale du namespace. Une preuve partielle conserve les cibles connues pour tentative puis signale la réconciliation nécessaire ; jamais un zéro cible complet par défaut. Le port du gardien ne modifie pas la génération partagée lors du rejeu, garde les identités/receipts/deadlines d'origine et borne la livraison. Un ACK de livraison n'est ni un terminal runtime ni une libération de quota.

Les cas de premier Cancel sans credential durable, d'ouverture du writer strict impossible et de convergence automatique après perte du transport restent des critères obligatoires à traiter et qualifier. La lecture blob-free d'un writer déjà ouvert ne ferme pas ces cas à elle seule. Code et tests sont en intégration ; le RED du handler constitue une preuve de défaut, pas une qualification du gardien ou de Docker. Ce chantier ne clôture aucun gate.

## 31 octobre — intégrité et durabilité de la publication d'export

Correction retenue dans les frontières existantes : l'autorisation d'un parent d'export inclut ses ancêtres, leurs propriétaires, modes, ACL et filesystem supporté. Le namespace doit appartenir à root ou au compte de contrôle et interdire les modifications par d'autres UID ; l'exception existante des répertoires sticky appartenant à root reste contrôlée. Une allowlist seule ne garantit pas ces droits. Les capacités FD sont conservées et revalidées pendant l'export. Le contrôle ne répare jamais les permissions d'un chemin existant et ne propage pas cette politique aux sources non fiables.

Avant de retourner un succès, synchroniser les fichiers après leurs données et modes finaux, les répertoires créés des feuilles vers la racine de staging, puis le parent après le renommage exclusif. Les répertoires implicites comptent dans une borne explicite. Le fait d'avoir renommé et la durabilité acquise sont distincts : toute erreur après le renommage conserve la destination et produit un résultat inconnu à réconcilier. Aucun remplacement, adoption ou nettoyage automatique de cette destination au retry. L'événement de succès reste postérieur à ces barrières ; la fenêtre avant sa naissance durable dans l'outbox ne devient pas une transaction globale filesystem/audit.

Les régressions utilisent le vrai filesystem et des fautes de contrôle après de vraies opérations ; elles ne qualifient pas une panne électrique, des erreurs matérielles de disque ou un adversaire root/même UID. La durabilité préalable des ancêtres externes et la matrice de stockage restent à qualifier. Cette décision ne clôture aucun gate ; les preuves et limitations du lot sont consignées après vérification.

## 31 octobre — identité individuelle, SSO client et découverte préconfigurée

Choix explicitement confirmés par le mainteneur : traçabilité complète de l'utilisateur aux délégations/actions/effets, SSO facultatif mais testé, fournisseur configurable, client public OIDC Authorization Code + PKCE S256 sans secret client distribué. ai-guard intègre le SSO du client et ne devient pas fournisseur d'identité ; le mode local nominatif hors ligne conserve le même contrat d'autorisation/audit. Identité humaine, identités d'exécution et authentifications Codex/Claude restent distinctes. Les restrictions de rôle et de tenant sont appliquées côté Rust ; les traces déclarées ne deviennent pas des identités vérifiées.

Fournir un parcours de découverte Compose avec une clé API du fournisseur choisi, ainsi qu'un profil Keycloak explicitement activé, conteneurisé et préconfiguré. La proposition initiale de deux comptes (`admin/admin`, `user/user`) est remplacée par les quatre comptes du complément ci-dessus. Les accès restent locaux loopback, les versions sont épinglées et aucune donnée/clé existante n'est remplacée. Ces identifiants publics sont propres à l'essai ; ils ne constituent pas des valeurs par défaut de déploiement normal. Aucun nom de fichier ou port évoqué comme exemple n'est imposé.

Environ cinq minutes est une cible ergonomique à mesurer, sans promesse ni garantie ; documenter les prérequis, téléchargements et étapes réelles. La démo doit montrer une authentification et une exécution réellement attribuées, avec les contrôles normaux du produit, sans données d'activité fictives. Elle ne vise pas à exploiter ou qualifier un Keycloak de production pour le client. Livrer documentation de découverte et raccordement du SSO d'entreprise, configurations et tests en même temps que le code.

Le [contrat fonctionnel](../sso.md), les fonctionnalités du [plan](IMPLEMENTATION_PLAN.md) et la [matrice de qualification](TEST_STRATEGY.md#identité-individuelle-sso-et-découverte--à-qualifier) détaillent l'acceptation. **Décision et documentation uniquement à ce stade** : aucun connecteur OIDC, realm, profil Compose ou test réel supplémentaire n'est revendiqué livré par cette entrée ; aucun gate clôturé. Le stockage et les frontières de déploiement existants sont conservés ; un changement structurel ultérieur conserve son obligation de proposition motivée.

## 30 octobre — révisions de supervision V2 dans le journal existant

Décision d'implémentation à l'intérieur du broker permanent déjà autorisé : enveloppe typée V2 avec domaine de hash propre, payload runner V1 conservé et vrais DTO de décision/admission/arm/claim/helpers/export. Pas de JSON libre nouveau ni fichier de supervision parallèle. La promotion conserve rev0/bytes/plans historiques et refuse tout préfixe V1 ayant contenu managed ; une tête vide ne suffit pas. Après V2, aucun retour via advance V1, même en replay. Les requêtes naissent pending dans une révision durable avant leur réponse connue ; helper préparé avant owned. L'autorité complète reste dans le vrai contexte managed, obligatoire dès préparation runtime, sans duplication déclarative. Les reçus référencés et parents historiques sont liés causalement, sans constituer une preuve d'authenticité ou un permis rechargeable.

Le [contrat actif](IMPLEMENTATION_PLAN.md) fixe API, bornes, propriété des fichiers et acceptation. Le gel5 a ses [preuves logicielles ciblées](evidence/2026-10-30/broker-checkpoint-v2/README.md) : six fonctions et26canaris PASS, fmt/Clippy/release et corrections de revue traitées. Le raccord au reducer Store reste pur/craté et ne change ni SQL ni état live ; le loader garde la comparaison canonique complète après suppression d'une seconde désérialisation V2 redondante. Deux dérogations Clippy locales justifient les enums de snapshots bornés, sans changement de wire. Ce format ne livre ni IPC, ni échéance autonome, ni fence D5, ni restauration d'autorité. Ces mécanismes et les faits locaux de cycle de vie avant admission exigent leur propre code et leurs essais ; G09 reste FAIL.

## 30 octobre — bootstrap3 exclusif, sans réactivation implicite

Une nouvelle factory conserve le backing réellement créé par O_EXCL et le garde d'initialisation, réutilise l'init2, puis libère ce garde avant le backup/migration3. Parent cible et parent backup reçoivent leurs barrières d'ancêtres. Retour actif seulement dans cette origine neuve ; copier, restaurer ou rouvrir une base ne l'active pas. Le chemin historique reste en schéma2 par défaut.

Le gateway propose une option explicite de création de stores neufs, pas une commande de récupération : backup privé distinct par tenant, aucune réparation/écriture de fichier existant, erreur avant serve avec partiels conservés. Les chemins privés des clés configurées sont retenus sans changer le JSON du registre ; comparaison dev/ino/ancêtres pour séparer backup et répertoires credentials connus. L'image prépare `/run/secrets` en1000/0700 pour ces contrôles, avec projections de fichiers0444 readonly. La qualification Docker comparative vérifie ce besoin réel, sans reconstruire toute la chaîne multi-stage ni remplacer QA. [Preuves, décisions de revue et limites](evidence/2026-10-30/supervision-bootstrap/README.md).

## 30 octobre — opération physique et tentative centrale distinctes

Le handle runtime du worker conserve l'opération du plan/intention, comme le produisent worker_spec et le runtime et comme le vérifie déjà la reprise. L'attempt UUID du Store identifie la tentative centrale et ses mutations ; il ne remplace pas cette opération physique. Le nouveau validateur du journal est corrigé selon le producteur, sans accepter indistinctement les deux UUID ni réécrire les anciennes exécutions. Les helpers gardent leur opération propre. La fixture précédente partageait l'hypothèse incorrecte du consommateur : elle est corrigée et ses premiers PASS ne sont pas attribués au vrai parcours. [Preuves et limites](evidence/2026-10-30/journal-context/README.md).

## 30 octobre — persistance privée du broker et codec strict

Le [journal v1](BROKER_JOURNAL.md) conserve des enregistrements immuables et un pointeur publié sous verrou, dans un namespace privé distinct des bases SQLite. L'acceptation engage le Submit exact, le scope fourni par le contrôle, l'intention, les bindings durables et la révision initiale ; elle n'accorde aucune autorisation runtime. Rejouer l'opération utilise cet engagement et les sources retenues, sans résoudre à nouveau Auto ni rouvrir le credential utilisateur. Une réservation partielle reste explicitement interrompue après perte de son propriétaire.

La validation d'un checkpoint utilise les vrais types privés existants. Une comparaison entre les clés du JSON d'entrée et sa resérialisation rejetait à tort un champ connu nullable omis par `skip_serializing_if` ; ce défaut a été reproduit. L'adaptateur [serde_ignored](https://docs.rs/serde_ignored/0.1.14/serde_ignored/) signale les champs ignorés pendant cette désérialisation, sans dupliquer tous les types ni conserver les chemins dans les erreurs. Version verrouillée0.1.14, sans mise à jour des autres paquets ; les lecteurs historiques restent inchangés. Les cas nullable connu, inconnu nul, approbation et document JSON surnuméraire gardent des régressions distinctes.

Les plafonds bytes/entrées/jobs/révisions protègent l'admission au journal ; ils ne remplacent pas des quotas physiques de stockage. Les erreurs et SIGKILL sur filesystem réel vérifient les frontières effectivement instrumentées, pas une panne électrique ni une résistance à root/même UID. Le contrôle des hardlinks précède toute ouverture pour préserver les locks POSIX SQLite du même processus. Les limites de plateforme, l'absence d'anti-restauration après redémarrage et la qualification du futur ordonnanceur restent explicites ; aucune activation de service ou migration n'est induite par ce lot.

## 30 octobre — identité immuable et acceptation durable distinctes

Cette décision décrit le lot d'intention antérieur. Le composant journal a depuis été implémenté avec les preuves et limites de la décision ci-dessus ; son raccord au service demeure ouvert.

`ExecutionIntentV1` porte l'invocation et les références initiales du vrai RunPlan, sans ses états managed. Format1 distinct des plans1/2/3 ; JSON canonique aux clés ordonnées et domaine SHA256 `ai-guard.execution-intent.v1` suivi de NUL. La lecture historique reste inchangée ; une identité initiale absente ou déjà approuvée n'est pas réparée lors de la projection. Snapshots publics à champs privés et diagnostics opaques, pas de source de credential relue ou d'Auto résolu à nouveau. [Validation et revue](evidence/2026-10-30/broker-intent/README.md).

Le hash ne prouve ni un binding réellement présent, ni les bytes d'un credential manuel, ni une autorisation tenant. Avant ACK, le journal devra lier immuablement l'opération, le Submit exact et son périmètre authentifié aux bindings durables des handoffs manuel/fournisseur, y compris l'absence explicite de credential. En manuel, persister depuis le PrivateFile initial retenu et synchroniser avant marqueur ; le Snapshot éphémère actuel ne suffit pas. Rejeu sous verrou : même engagement/binding, sans source relue ni Auto ; binding absent/altéré refuse admission et premier start. Le binding plan3 existant conserve son digest et son identité. Ce journal reste à implémenter/tester ; aucun droit de start ou de reprise n'est conféré par le composant pur.

## 30 octobre — API v1 et producteur d'horloge de supervision

Les contrats du Store3 disposent de neuf routes HTTP sous `/api/v1` uniquement, sans alias nouveau `/api`, bind legacy ni commande d'activation. Les guards existants authentifient le tenant et vérifient le rôle avant toute extraction de corps ; chaque opération helper retrouve son armement et son parent réels dans cette base. Les prélectures d'admission écartent les demandes invalides et l'autorité inactive avant l'évaluation externe ; un rejeu durable n'appelle pas Jev. Les transactions du Store restent seules habilitées à réserver, consommer et refuser. Une prélecture n'élimine pas une concurrence ou une invalidation postérieure, ni plusieurs évaluations simultanées avant le premier commit.

Le producteur Rust `NativeHostClock` retient l'identité de boot et les paramètres natifs à sa construction, puis produit un compteur continu entier avec erreurs typées, sans UTC de secours ni lecture de fichier par échantillon. Linux utilise `CLOCK_BOOTTIME` ; Darwin utilise `mach_continuous_time` et une conversion entière vérifiée. Références primaires : [Linux timekeeping](https://www.kernel.org/doc/html/latest/core-api/timekeeping.html), [ABI Apple XNU](https://raw.githubusercontent.com/apple-oss-distributions/xnu/main/osfmk/mach/mach_time.h). L'identité de boot n'est ni une identité tenant ni une permission. Les consommateurs doivent encore vérifier méthode, boot et progression, conserver l'échéance et déclencher l'arrêt indépendamment des IO.

Ce lot prépare le service permanent autorisé, sans le remplacer : schéma2 par défaut, pas de migration/activation CLI, pas de réactivation au redémarrage, d'ordonnanceur, d'IPC ou de gardien livré. Qualification native limitée au macOS testé ; Linux, sommeil/reprise, reboot et fencing D5 restent ouverts. Les [preuves propres au lot](evidence/2026-10-30/supervision-api-clock/README.md) distinguent logiciel et effets runtime.

## 30 octobre — arrêt strict et politique helper commune

Les chemins possédant un snapshot runtime complet utilisent un port d'arrêt qui compare identité, image, configuration/version, restart et ressources avant effet. Une divergence ou inspection inconnue conserve l'incertitude sans kill ; l'observation après effet reste nécessaire. Le port legacy sans baseline n'est pas une alternative pour les futurs armements. Aucun terminal, quota ou cleanup supplémentaire n'est déduit d'un retour Created non-running. [Correctif relu et vérifié](evidence/2026-10-30/runtime-stop-snapshot/README.md) ; la perte de durée après SIGKILL du CLI reste un FAIL réel distinct.

La spec helper est dérivée par la même fonction Rust côté contrôle et broker, avec un domaine de hash propre. Elle ne reprend ni le hash de la commande métier ni son preset TTY : tar reste preset2/réseau coupé/sans credential et borné à120s. ImportVerification est une finalité distincte du scan final ; avant premier start v3, sa preuve centrale doit correspondre à l'import et au manifeste admis, y compris dans le flux normal. Les [DTO préparatoires](evidence/2026-10-30/supervision-contract/README.md) sont vérifiés ; le Store et le broker ont leurs validations séparées, sans activation implicite ni annonce de qualification.

## 30 octobre — supervision du broker autorisée

Le mainteneur a répondu **« Implémenter la proposition (recommandé) »** après soumission de la [direction relue](BROKER_SUPERVISION.md), conformément à AGENTS.md : même binaire Rust en broker permanent, IPC local privé, échéances durables et gardien TTY local pour survivre à la mort du CLI. Accord pour implémentation, configurations déclaratives et tests ; aucune installation/charge de service OS, opération cloud/PKI ou destruction de données existantes autorisée par cette réponse. Aucun code de ce service n'est encore livré à ce jalon ; le correctif HTTP est intégré et vérifié avant d'engager la supervision.

Une revue de constructibilité a relevé puis fait corriger trois conditions : intention immuable distincte du checkpoint RunPlan mutable, gardien de terminal avec FD/état retenus avant raw et restauration indépendante de Drop, arrêt préarmé ne dépendant pas d'un fsync ou mutex du journal à l'échéance. Elles sont fermées au niveau conception, pas par des tests. Fencing d'un start ambigu et récupération d'un broker macOS vivant figé restent des capacités/preuves à développer ; une approbation structurelle ne les accepte pas comme risques de release.

## 30 octobre — authentifier avant extraction applicative

Défaut reproduit sur le vrai routeur et HTTP local : le JSON ou la query pouvait être traité avant l'identité, et un client sans token gardait sa requête en attente de corps. Choix : guards Rust typés `FromRequestParts` en premier paramètre de chaque handler protégé, rôles vérifiés avant JSON, sans ajouter un second middleware d'authentification. Index des digests construit depuis la configuration de confiance et partagé ; tenant/Store restent issus exclusivement de cette identité. Aucun changement d'API métier, de base, d'audit ou de hash de décision.

Le partage immuable concerne aussi politique, redactor et client Jev : garder seulement l'index en Arc aurait encore recopié le dictionnaire de secrets au clonage de l'état Axum. Bearer borné à4096octets, refus des headers ambigus et des credentials configurés avec espace blanc ; fichiers historiques et identité invalides échouent sans autorisation partielle. Les contraintes de fichiers existantes ne sont pas élargies. [RED/GREEN, 51 tests ciblés et revue](evidence/2026-10-30/api-auth/README.md). Résistance aux connexions lentes, drainage HTTP et performances sous charge restent à qualifier séparément.

## 30 octobre — contrat Jev réel confirmé

Le mainteneur fournit l'exemple TypeSafe : `POST https://api.typesafe.ai/v1/systemone`, Bearer et modèle demandé `jev-latest`. Le client Rust existant conserve sa question `choice` pour les trois issues `allow|deny|review`, plutôt que remplacer une décision à trois issues par le `noul` de l'exemple. Le contrat accepte les deux types ; [référence HTTP](https://docs.typesafe.ai/api/system-one) et [Choice](https://docs.typesafe.ai/primitives/choice) vérifiées le 30 octobre. L'alias demandé et le modèle retourné restent distincts, car [l'alias évolue](https://docs.typesafe.ai/models).

Le header long contexte reste absent : l'état minimal de décision ne le nécessite pas. Le [fournisseur](https://docs.typesafe.ai/models) signale une dégradation de précision et de délai pour ce mode. La clé reste dans le contrôle via un fichier privé dédié ; l'activation réseau/Compose demeure explicite. Renseigner un `.env` ne démarre ni Jev ni un worker et n'autorise aucun envoi de projet. Un seul appel réel synthétique a ensuite vérifié le contrat, sans retry : réponse `jev-1.14.0`, audit valide après réouverture. [Preuves](evidence/2026-10-30/jev-real/README.md). Sa réussite ne qualifie pas la sémantique générale ni le produit.

Le premier candidat local `Qwen2.5-0.5B-Instruct Q4_K_M` a respecté le plafond de 1 Go au cours de deux inférences Docker (pic cgroup 777 912 320 octets), mais a répondu `allow` à une suppression. **Candidat refusé pour autoriser des actions.** Le protocole et le budget mémoire ne prouvent pas la qualité des décisions ; aucune recherche répétée de prompt vert ni levée des refus Rust.

## 30 octobre — trois moteurs Jev, petit modèle et budget de tests

Choix explicite du mainteneur : conserver Jev réel, fournir une approche Rust sans modèle avec limites sémantiques incomplètes/imparfaites annoncées, et proposer un moteur local Jev-like dans Docker/Podman. La suggestion Gemma est remplacée par une sélection adaptée, déléguée à l'orchestrateur, au plus4B de paramètres et cible500Mo–1Go de RAM. Modèle, révision/quantification/contexte et limites sont modifiables. Respect mémoire à mesurer sur moteur réel, pas à déduire du nom du modèle. Les embeddings Qwen suggérés sont une référence de besoin, pas un choix irrévocable ; vérifier l'adéquation d'un modèle textuel au classement de décisions.

Le moteur sélectionné et ses limites sont visibles/audités. Aucun basculement silencieux, aucune levée d'un refus impératif, aucun remplacement du confinement par Jev ou un modèle. Notre code d'adaptation reste Rust ; runtime tiers éventuel épinglé et isolé. Clé Jev reçue, conservée privée, jamais confiée au worker ; endpoint/modèle ensuite confirmés ci-dessus. La sélection commune des trois moteurs et l'adaptateur local restent à implémenter/qualifier selon le code réellement disponible.

Le mainteneur demande de limiter les tests fournisseur : prochains essais Claude sur `claude-haiku-5-5`, pas d'alias ni de fallback, tests ciblés et pas de campagne E2E automatique. L'identifiant est confirmé par le [guide officiel Anthropic](https://platform.claude.com/docs/en/models/haiku-5-5/migration-guide). Aucun appel modèle n'est effectué pour vérifier cette documentation.

## 30 octobre — fournisseurs dynamiques et dashboard cloud unifié

Choix explicites du mainteneur : Codex CLI et Claude Code, chacun avec abonnement ou clé API, résolution dynamique des capacités et du mode, sélection explicite prioritaire et aucun basculement silencieux vers une API facturée. `.env` local privé autorisé, ignoré par Git et exclu du workspace ; transmission du seul sous-ensemble utile via le handoff du contrôle. Aucun cache OAuth/keychain hôte extrait implicitement. Chaque parcours exige sa qualification réelle.

Le client cloud dispose d'un dashboard unique pour tous ses agents/workers, y compris sur plusieurs brokers/hôtes/runtimes ; aucun accès par dashboard de worker. A/B restent isolés par l'identité authentifiée. Le control plane existant agrège registre, événements, quotas et usage avec provenance/fraîcheur et états partiels visibles. Pas de microservice nouveau obligatoire. Workload identity, transports distants, reprise et capacité100 restent à implémenter et prouver. AGENTS.md et les contrats architecture/tenancy portent ces exigences ; leur inscription ne constitue pas une livraison.

## 29 octobre 2026 — Conserver la base et les frontières existantes

Rust/axum/tokio/SQLite et le frontend TypeScript statique sont conservés. Aucun nouveau moteur de stockage, bus ou service n'est introduit. Le contrôle multi-client est développé dans le domaine commun ; Kubernetes reste un adaptateur à réaliser. Les anciens documents indiquant des fonctions absentes ne diminuent pas la cible.

Git ne contenait aucun commit au départ et tous les fichiers étaient non suivis. Les agents travaillent en place avec propriétaires exclusifs, copies de comparaison temporaires et revue indépendante ; aucun commit global du travail antérieur ni suppression de fichier n'est effectué.

## 29 octobre 2026 — Identité et base d'audit

Un registre versionné de références de fichiers permet plusieurs tenants, chaque credential runner/viewer sélectionnant sa base. Le mode historique conserve l'identité `local`. Une base non vide historique n'est jamais automatiquement transférée à un autre client. Les API `/api/v1` sont ajoutées avec alias `/api` pour conserver les clients locaux. Les fonctions registre/quota/relais/usage ne sont pas simulées par ce lot.

## 29 octobre 2026 — Licence

Le mainteneur a donné latitude de choix pour une licence ouverte (« open bar la license »). Choix de l'orchestrateur : **Apache-2.0**, texte officiel inchangé dans `LICENSE` et identifiant SPDX dans Cargo. Source : [texte officiel Apache](https://www.apache.org/licenses/LICENSE-2.0.txt). Aucun propriétaire de copyright, adresse de contact ou dépôt distant n'est inventé. Le canal privé de signalement reste une décision préalable à la publication.

## 29 octobre 2026 — Audit avant progression fonctionnelle

La revue indépendante a reproduit des acquittements d'audit incompatibles avec des schémas SQLite inattendus et une fenêtre d'altération externe non détectée. Ces P0 sont traités avant les décisions idempotentes et le registre des workers. La conception doit refuser les schémas incompatibles avant mutation, valider l'audit au démarrage et détecter les changements externes avant une nouvelle écriture durable. Elle ne prétend pas résister à une réécriture complète du stockage par son administrateur.

## Proposition au mainteneur — publication locale des services sur Docker Desktop

**Constat réel, P1 déploiement :** image contrôle construite, gateway/dashboard sains sur leur réseau interne, mais aucun port réellement publié sur le poste. Les demandes éphémères puis explicites `127.0.0.1:48787:8787`/`127.0.0.1:48400:4000` sont présentes dans HostConfig, tandis que NetworkSettings.Ports reste vide ; curl loopback échoue. Le smoke n'a pas lancé de worker, faute d'endpoint. Environnement : Desktop 4.97.0/Engine29.9.2 ARM64, Compose5.6.1. Les Compose livrés utilisent la même topologie exclusivement interne.

La [documentation Compose](https://docs.docker.com/compose/how-tos/networking/#internal-networks) précise l'absence de passerelle et le fait qu'un second réseau normal redonne une sortie Internet. La [réponse Moby](https://github.com/moby/moby/discussions/53256) explique pourquoi la publication dépend d'un réseau avec passerelle et propose un proxy séparé pour conserver le service interne isolé. Le constat du dépôt vient des inspections/connexions réelles ; la source externe étaye l'explication.

**Option recommandée, soumise avant réalisation :** ajouter un petit relais d'entrée HTTP Rust de confiance, comme sous-commande du même binaire et même image, dans un conteneur de contrôle distinct connecté à un réseau de publication et au réseau interne. Il expose seulement les deux ports hôte loopback vers les services configurés : gateway et dashboard conservent chacun leurs droits et leur protocole. Le relais ne monte aucun credential, volume de données, projet ou socket moteur ; il ne journalise ni corps ni Authorization. Il transmet des flux bornés/avec timeouts, conserve SSE, refuse toute destination choisie par une requête et tout redirect. Le gateway continue d'authentifier/autoriser chaque requête ; le dashboard reste en lecture seule. Le relais possède une route de sortie par nécessité de publication, les deux services applicatifs restent internes. Aucun réseau worker n'est ouvert.

Coût : un conteneur de contrôle supplémentaire et une surface HTTP à tester/revoir ; aucun nouveau stockage, langage, service externe ni dépendance requis a priori. Ce composant ne remplace pas TLS/mTLS cloud, l'identité workload ou les ACL tenant.

**Alternative plus simple :** attacher directement gateway/dashboard à un bridge normal, avec publication loopback et aucun worker sur ce réseau. Cela supprime un composant mais leur donne une sortie réseau ; ce changement de garantie doit être accepté explicitement, pas présenté comme isolement équivalent.

**Alternative de maintien :** conserver la topologie actuelle et déclarer le parcours local Desktop bloqué ; aucune qualification de l'accès depuis le poste n'est possible. Les travaux Rust et les essais standalone restent indépendants.

Statut : **ACCEPTED** par le mainteneur le 29 octobre 2026 (« Relais Rust séparé : conserver les services internes »), implémenté et relu. Accès loopback, ports, routes IPv4 inspectées, rôles/auth, SSE/reconnexion et smoke worker ont été vérifiés sur la pile QA Docker Desktop 4.97.0/Engine29.9.2 macOS ARM64. Les constats de revue sur délais, progression des flux et santé des deux destinations ont été corrigés et revalidés. Voir `VALIDATION.md` pour le candidat et la portée ; les autres moteurs/plateformes, le cloud et le relais métier inter-workers restent non qualifiés.

## 29 octobre 2026 — Préparer l'autorité durable des workers

Décisions de conception de l'orchestrateur dans les frontières validées ; **registre, quotas et ledger non encore implémentés**. La gateway et la base SQLite de chaque tenant restent l'autorité durable. Aucun nouveau service, stockage ou langage n'est introduit. La première qualification vise un hôte de confiance ; le multi-hôte exige un fencing réellement appliqué au lancement, pas une lease SQL déclarée suffisante.

Une commande métier correspond à une instance worker terminale. Le plan et l'identité de tentative sont stables avant création. Une admission versionnée lie tout le spec d'exécution à une nouvelle décision serveur, avec validation centrale du profil, du réseau, de l'image, des ressources et du timeout. Le hash des demandes historiques n'est pas modifié silencieusement et un ancien `allow` ne devient pas implicitement un droit de lancement. La migration du schéma et la concordance registre/audit doivent être vérifiées sans réécrire les événements anciens.

Réservation et quota sont engagés avant création/préparation ; l'émission `start` est consommée irréversiblement. Une réponse perdue ou un replay n'accorde jamais une seconde émission. L'état incertain conserve sa capacité ; aucune TTL ne libère un worker potentiellement actif. L'annulation avant lancement doit interdire tout claim ultérieur ; un start déjà consommé exige réconciliation avec la même autorité et instance runtime.

Le futur ledger mesure uniquement l'intervalle de processus `StartedAt` → `FinishedAt` et les allocations CPU/RAM réellement inspectées, avec unités entières exactes, source et `billable=false`. Il ne mesure ni préparation, ni CPU consommé, ni prix. Une observation manquante, un restart ou une divergence de ressources n'est pas remplacé par zéro. Transition terminale, mesure, libération et audit constituent une transaction unique. Ces choix orientent l'implémentation ; leurs garanties restent à prouver sur les vrais runtimes.

## 29 octobre 2026 — Présentation partagée et reprise des opérations

Le dashboard conserve le mode viewer fixe pour l'installation locale. Le mode explicite `--shared-tenants` accepte les viewers du gateway, sans credential de tenant monté dans le dashboard. Une lecture d'identité bornée précède chaque requête relayée ; le rôle doit être `viewer`, y compris en mode fixe. Le credential authentifié sélectionne toujours la base. Le proxy conserve sa destination fixe, sa liste de lectures et son refus des mutations ; aucune identité administrateur inter-clients n'est ajoutée. Ce comportement reste dans la frontière de présentation validée.

La reprise ne remet jamais un claim de start consommé à disposition. Deux transitions étroites récupèrent la préparation avant tout create, ou enregistrent une instance exactement inspectée après create dont l'accusé a été perdu. Elles gardent les identités, révisions, réservations et claims antérieurs ; l'audit rejoué vérifie leurs deltas exacts. Le caller possède le verrou de tentative, vérifie tenant/origine, autorité liée, image/configuration/labels et manifeste de copie immuables. Un helper n'est récupérable que terminal réussi et validé ; aucun réimport ni second create n'est déduit d'un accusé manquant. Une incertitude après claim reste à réconcilier. Les tests de fixtures n'établissent pas les effets d'un moteur réel.

Le timeout de l'appelant ne suffit pas à arrêter un worker après mort du broker. Ce besoin reste P1 ouvert et exige un mécanisme Rust de supervision/réconciliation dans le périmètre de contrôle ; ni une lease SQL ni un quota conservé ne clôturent cette obligation. La restauration d'anciens claims et le fencing runtime restent également des exigences de produit ouvertes.

## 29 octobre 2026 — Séparation Jev et frontières de capture

La clé Jev doit différer de toutes les identités runner/viewer, à l'initialisation, au startup et à l'assemblage de GatewayState en bibliothèque. Une collision refuse l'assemblage ou toute autorisation selon le constructeur compatible ; elle ne supprime jamais silencieusement la politique Jev. La clé normalisée comparée est celle utilisée par le client.

Le masquage d'une capture tronquée intervient sur ses octets originaux avant UTF8 lossy et clipping : union des occurrences et suffixes littéraux connus, recherche KMP et marquage linéaires par secret, fusion des dictionnaires privés au dernier moment. Les JSONL complets restent décodés puis masqués ; la ligne finale incomplète est omise et déclarée comme limite. Les erreurs du moteur gardent une capture typée dont Display/Debug sont opaques, avec un rendu explicite protégé dans le broker. Les marqueurs Podman, y compris avant la fin du bootstrap, ne deviennent pas des diagnostics publics bruts. Cette règle ne constitue pas une DLP contre tous les fragments/encodages produits volontairement par un worker.

## 29 octobre 2026 — Référence visuelle demandée par le mainteneur

L'IHM de contrôle prend [Nothin’](https://www.noth.in/?ref=siteinspire) comme inspiration autant que possible. [AGENTS.md](../../AGENTS.md) et [docs/ui.md](../ui.md) précisent l'adaptation à la supervision : hiérarchie typographique, base noir/blanc, respiration, accents maîtrisés et densité lisible. La revue garde les états réels, thèmes, accessibilité et mouvement réduit comme critères d'acceptation. Aucune nouvelle apparence n'est déclarée livrée par l'ajout de cette consigne.

## 29 octobre 2026 — Invocation de confiance du moteur, politique2

Le programme moteur est une dépendance du contrôle, pas une commande choisie par le projet ou le worker. Supprimer la recherche PATH sur tous les parcours, y compris préflight, doctor/build/compose, helpers et reprise. La fabrique utilise des emplacements système connus, validés sans exécution ; une installation personnalisée passe par un chemin absolu explicitement fourni par le contrôle. Les liens d'installation sont résolus vers une cible admise ; un programme dans le projet importé est refusé. Fichier régulier exécutable, propriétaire root/utilisateur de contrôle, absence d'écriture non approuvée et identité/contenu sont vérifiés avant spawn. Les limites d'ACL et de substitution par un administrateur d'installation restent explicites et ne sont pas fermées par la seule canonicalisation.

L'invocation privée porte une version, l'entrée approuvée, le chemin canonique et l'identité/SHA256 du fichier, le cwd et l'environnement reconstruit. L'environnement du processus n'est pas copié : HOME vient du compte OS, les chemins de configuration standards sont explicites, le PATH des helpers est limité aux répertoires système contrôlés, sans segment relatif/projet. Les sélecteurs ou loaders ambiants ne pilotent pas une reprise. `--runtime-program` constitue une entrée explicite du contrôle, pas un mécanisme d'API worker ni une exception réservée aux tests.

La revue du premier patch a montré qu'une vérification de pathname suivie de son ouverture par `exec` ne lie pas ces deux opérations. Le prototype Darwin indépendant démontre la différence : après substitution d'un parent, le pathname absolu exécute le remplaçant, tandis qu'un descripteur du répertoire final suivi de `fchdir` et de l'exécution de `./nom` conserve le programme approuvé. La correction ancre ce répertoire final, exige fichier et répertoire appartenant à root ou au contrôle sans écriture groupe/autres et refuse les ACL étendues non qualifiées. Le cwd de l'enfant devient ce répertoire de CLI, toujours hors projet ; les chemins source/build/Compose restent absolus. Aucune confiance implicite au groupe `admin` macOS n'est ajoutée. La lecture de vérification du programme est bornée. Implémentation et preuves ciblées sont conservées au [gel261](evidence/2026-10-29/trusted-invocation/README.md) ; elles ne clôturent ni les courses globales des fichiers de configuration ni les dépendances/plugins chargés par la CLI depuis le compte de contrôle, ni les déplacements de vrais arbres par un administrateur du namespace.

L'identité du moteur/workload et l'identité du client CLI restent distinctes : le fingerprint existant des handles conserve l'autorité moteur ; un fingerprint d'invocation séparé est imposé avant chaque effet. Les nouvelles liaisons enregistrent l'invocation et refusent sa substitution. Une ancienne liaison sans cette preuve reste consultable sans moteur ; son adoption exige une option opérateur explicite, une vérification en lecture seule de la même autorité puis audit durable et checkpoint avant effet. Aucune adoption ne réarme un claim, ne recrée un worker ni ne change son tenant. Un nouveau run peut utiliser une nouvelle autorité logique liée à l'invocation approuvée en conservant les anciennes données.

Les fixtures Rust passent par une entrée absolue explicite et des métadonnées privées de fixture ; elles ne dépendent plus d'un détournement PATH ou d'un environnement arbitraire transmis au sous-processus. Préserver Docker Desktop/Homebrew, Docker/Podman Linux et le Podman local/VM Unix prévu par l'adaptateur ; aucune prise en charge SSH/TCP n'est ajoutée. La migration des checkpoints, les opérations contrôlées et les tests réels sont des critères de ce chantier, pas une qualification déjà acquise. La supervision de durée et le durcissement global des fichiers SQLite/credentials demeurent des chantiers obligatoires distincts.

## 30 octobre 2026 — Namespace SQLite privé et propriété des descripteurs

Conserver SQLite par tenant et ses VFS standards, sans nouveau service. Le parent dédié700 et sa chaîne de descripteurs protègent le namespace contre un autre UID ordinaire ; main et sidecars présents sont600/UID/lienunique/ACLadmise. Ne pas garder un FD brut du main en parallèle de SQLite : sa fermeture peut annuler des locks POSIX de l'autre connexion du même processus, défaut reproduit. Vérifier noms/métadonnées et `HAS_MOVED` obligatoire ; toute divergence latchée refuse l'action. Un verrou distinct par parent sérialise l'initialisation, attente bornée ; le consumer backup utilise le même garde.

Limiter le basename main à ASCII, parents Unicode admis. Une normalisation Unicode approximative serait insuffisante sur les filesystems qui replient certains caractères vers les noms réservés ; le préfixe de lock et les suffixes SQLite sont réservés sans distinction de casse ASCII. Refuser une ancienne base non conforme avec procédure offline explicite, sans renommer, réparer ou supprimer. L'image initialise/data700 pour volumes neufs ; un volume existant exige une maintenance consciente après arrêt et sauvegarde cohérente. Cette politique ne couvre pas le même UID, root ou administrateur du namespace/stockage et ne fournit pas de handoff VFS atomique contre eux.

Le [gel327](evidence/2026-10-30/sqlite-storage/README.md) conserve source, RED/GREEN, deux revues, tests macOS/Linux limités et maintenance QA avec audit591 inchangé. La restauration des claims et la qualification globale demeurent ouvertes.

## 30 octobre 2026 — Journal moteur et compatibilité du preset

Décision d'implémentation dans l'adaptateur existant : imposer `none` plutôt que dépendre du driver/rétention du daemon pour les sorties brutes d'un worker. Les nouvelles créations sont inspectées avant le start. Preset `fixed_sandbox_v2` et fingerprint privév2 lient cette obligation sans modifier les hashes v1. Les anciens claims ne sont jamais réarmés ; un premier start v1 est refusé, les anciens workers déjà partis restent observables et arrêtables. L'API d'admission v1 explicite reste compatible avec le contrôle de confiance ; la garantie exige le nouveau broker et le preset inspecté, pas seulement une admission déclarée. Mise à jour coordonnée des composants, aucune migration de données. [Preuves et limites](evidence/2026-10-30/engine-logs-v2/README.md).

## 30 octobre 2026 — Terminal local dans le broker, implémentation en cours

Étendre le broker Rust existant par un pont PTY borné, sans service ou socket supplémentaire dans les workers. L'utilisateur demande les vrais CLI Codex/Claude ; `exec`/`-p` ne qualifient pas ce parcours. Mode interactif explicite lié à la demande/approbation et à un preset versionné ; captures/transcripts désactivés, filtre terminal et masquage streaming avant affichage, restauration de l'état terminal. Les JSON/hashes batch historiques gardent exactement leur forme. Un serveur ancien doit refuser le nouveau contrat sans effet, aucune dégradation silencieuse.

Le [design préparatoire](evidence/2026-10-30/interactive-cli/design.md) proposait `StdinOnce=false` pour un réattachement durable. Le [probe Docker réel](evidence/2026-10-30/interactive-cli/docker-flags.md) a **réfuté l'hypothèse de flags** : CLI29.2.1 donne true même avec attaches stdout/stderr explicites. La [source officielle de cette version](https://github.com/docker/cli/blob/v29.2.1/cli/command/container/opts.go) impose AttachStdin avec interactive puis StdinOnce quand les deux sont vrais. Décision pour le premier lot de transport : exiger le profil réellement produit par chaque moteur (Dockertrue ; Podmanfalse reste non qualifié sur moteur réel), traiter toute perte du transport par arrêt ciblé/inspection et ne promettre aucun détachement durable. Aucun accès direct nouveau à l'API socket pour contourner ce résultat.

Le réattachement TTY après claim consommé reste une exigence ouverte, refusée explicitement tant qu'elle n'est pas implémentée et prouvée ; les chemins existants observation/arrêt restent disponibles, sans second start. La supervision autonome après SIGKILL et les limites après panne restent des obligations distinctes. Cette décision autorise l'implémentation du transport dans l'architecture actuelle ; elle ne déclare pas la TUI livrée ni qualifiée.

### Corrections démontrées pendant la qualification du transport

Darwin vide la queue au dernier close du slave et révoque le terminal contrôlant à la sortie de son leader. Le client moteur conserve donc sa session isolée **sans** terminal de contrôle ; ses trois fd restent de vrais TTY. Un keeper explicite garde la queue jusqu'à lecture non bloquante EAGAIN après fin du client, puis ferme et exige EOF/EIO réel sous les bornes du transport. SIGWINCH est transmis explicitement au PID encore possédé avant drain/réap. Les copies Stdio cachées du Command sont fermées. Aucun nouveau service de supervision n'est créé par ce mécanisme.

L'alias `/dev/tty` Darwin n'est pas enregistrable dans kqueue. Le broker l'utilise pour attester session/foreground, puis ouvre séparément un vrai device depuis les streams standard avec O_NOFOLLOW, O_NOCTTY, NONBLOCK et CLOEXEC. Identité dev/inode/rdev, session et foreground sont revérifiés ; les flags du stdin partagé restent inchangés. Des streams tous réduits à l'alias sans device identifiable sont refusés avant admission, sans repli silencieux. Le helper public suppose un runtime Tokio I/O actif, comme documenté ; un Handle courant seul ne garantit pas ce prérequis d'embedding.

Les [preuves](evidence/2026-10-30/interactive-cli/README.md) conservent les FAIL de développement et les régressions avant les deux PASS Docker génériques. Le driver Rust des tests garde le terminal physique pendant vérification et avance par poll borné ; son ancienne attente bloquée n'est pas présentée comme réussite. Les clients Codex/Claude et les autres moteurs/plateformes conservent leurs propres gates.
