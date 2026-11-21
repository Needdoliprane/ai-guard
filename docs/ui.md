# Interface de supervision

Ce document est la référence pour l'identité visuelle et les vues du dashboard. Le [périmètre livré](#périmètre-livré) comprend la consultation technique `viewer` et les lectures nominatives locales/OIDC décrites ci-dessous, dont le registre brokers sur permission explicite. La [supervision unifiée nominative](#supervision-unifiée-nominative--candidat-du-10-novembre-2026) est un composant candidat ajouté le 10 novembre 2026. Les [vues utilisateur et administration unifiée à 360°](#vues-utilisateur-et-administration-unifiée--cible-à-livrer) restent une cible obligatoire à compléter et qualifier. Le [contrat d'identité et de configuration](sso.md) fixe les permissions et profils attendus. Les preuves visuelles datées ne qualifient pas les fonctions futures.

## Identité de version et build — candidat du 14 novembre 2026

La zone permanente de diagnostic version/build présente l'identité propre du bundle
chargé, même avant connexion ou sans données métier. Version produit 0.0.1 et
Build ID commun décrivent l'ensemble distribué ; Interface, Dashboard et
Gateway restent trois provenances distinctes. L'identité frontend est compilée
depuis son package, jamais remplacée par une valeur du serveur. La politique
canonique Cargo et le build source capturé sont décrits dans le [guide FR](setup/release/README.fr.md#versions-and-updates)
et le [guide EN](setup/release/README.md#versions-and-updates).

Les deux lectures publiques ont lieu initialement puis sur actualisation
explicite, sans polling ni recherche de nouvelle release : status dashboard
local et status de son gateway configuré, [contrat HTTP](api.md#build-status).
Elles omettent credentials/cookies, cache et referrer, refusent redirections
et réponse incompatible ; le lecteur frontend borne chaque lecture à 4 s,
4096 octets et 128 chunks. Une requête déjà en cours est partagée ; la fermeture
annule l'observation. Ces bornes frontend restent distinctes du proxy Rust 5 s/
8192 octets. Aucun échec ne déclenche une boucle de relance.

Chargement, route absente d'un ancien serveur, réponse invalide, délai, service
en erreur et connexion impossible ont des libellés distincts. Un ID/révision
absent reste inconnu ; une identité indisponible ne reprend pas l'ancienne
valeur comme courante. Les différences version/build sont visibles, y compris
un bundle ancien en cache ; révision hex normalisée pour la comparaison,
valeur reçue conservée. « Version concordante · build inconnu » et identité
incomplète restent distincts de « Identités concordantes ». Aucun de ces
libellés ne prouve santé, autorisation, readiness, compatibilité des bases ou
mise à jour appliquée. Il n'y a pas de bouton installer/appliquer une mise à
jour ni de contrôleur livré par cette fonction.

Conserver identité OVPT, thèmes, mobile, clavier/focus et annonce accessible
après les lectures terminées, sans répétition des états identiques. Cette
surface est documentée depuis `e0372db` ; tests Node/Rust, build, navigateur,
captures et observations réelles de cette candidate restent NOT_RUN dans ce
lot. Les preuves visuelles et résultats historiques gardent leur source.

The loaded frontend keeps its own compiled version/build before sign-in.
Dashboard and gateway are separate bounded public observations; loading,
unknown/unavailable and mismatches stay explicit. Initial/manual refresh adds
no release discovery, readiness claim, updater or replacement of active
sessions. Candidate validation and visual evidence remain NOT_RUN here.

## Créer et intervenir sur une session — cible autorisée le 15 novembre 2026

Le [contrat FR](engineering/SESSION_CONTROL.fr.md) et le [contrat EN](engineering/SESSION_CONTROL.md) rendent obligatoires deux actions nominatives : Nouvelle session et Envoyer une consigne. Elles restent à implémenter et qualifier ; cette section n’étend pas rétroactivement le périmètre livré.

Nouvelle session choisit une cible projet/source opaque autorisée, tâche initiale et client/mode permis ; résolution effective et prérequis viennent de Rust. Préparer ne lance rien ; Lancer confirme l’intention exacte. Un timeout propose la relecture de la même opération, sans second Submit. Aucun chemin, argv, moteur, credential ou réseau libre dans le formulaire.

Le détail d’une session propose une courte consigne seulement selon droits et canal effectifs. Afficher destination session/worker/tentative, limite et état réel : enregistré, transmis, acquitté au protocole ou livraison inconnue ; jamais « exécuté » sur simple écriture. Les anciens batch/TTY incompatibles sont nommés. Préserver le brouillon tant que l’admission durable reste inconnue, empêcher les doubles envois et ne pas relancer après une émission incertaine.

LaunchSession, SendSessionMessage, lecture/contenu et ApproveLaunch sont distincts côté serveur. Changement d’identité/périmètre ou révocation purge brouillons, corps, reçus et callbacks périmés. Les contrôles restent accessibles au clavier et dans les thèmes clair/sombre et petits écrans ; conserver OVPT et la retenue visuelle de la supervision.

## Direction de conception

Le guide détaillé du mainteneur reçu le 30 octobre 2026 a guidé la refonte livrée en QA. Le front antérieur, avec barre latérale claire et accents turquoise, a été remplacé par le header clair et la navigation sombre fixes. La référence est transposée à **ai-guard**, à ses neuf vues et à ses données de supervision ; elle ne transforme pas le dashboard en page de vente ou en parcours fictif de dix chapitres. Les [preuves et captures réelles](engineering/evidence/2026-10-30/editorial-ui/README.md) identifient le rendu déployé.

### Contrat visuel et plan de réalisation

- Palette claire : fond `#FAFAF8`, texte `#101010`, navigation `#161714`, identité/action sombre `#111111`, citron `#E2F471`, survol citron `#EFFFA1`, texte sur citron `#171B07`, panneaux `#F0F0ED`, traits `#D4D4CF`, texte secondaire `#666662`, seconde lecture des titres `#8A8A84`. Focus général `#5555FF`, focus clair sur navigation sombre. Les verdicts gardent un sens textuel et leurs teintes discrètes ; ils ne deviennent pas des couleurs de marque.
- Arial/Helvetica/sans-serif pour texte et titres ; Courier New/monospace pour numéros, repères, dates et métadonnées. Corps16px/interligne1,55 ; grands titres `clamp(36px,4.2vw,62px)`, graisse500, interligne1 et espacement serré ; sections25px. Les logs et tableaux restent lisibles avec une densité adaptée. Aucune police distante.
- Conteneur1800px maximum, gouttière `clamp(20px,3.2vw,56px)`, traits1px, panneaux4–5px. Utiliser des lignes et de l'espace ; éviter une carte autour de chaque paragraphe. À la demande explicite du mainteneur et d'après ses deux visuels fournis, signature OVPT typographique forte, séparateur sobre et nom du produit ai-guard. Conserver les repères d'identité et de lecture seule ; aucune décoration orbitale.
- Remplacer la barre latérale générique par un header et une navigation sombre fixes en haut. Conserver les neuf destinations réelles, numérotées, le client authentifié et l'état du flux. Afficher la position de vue comme navigation, jamais comme progression de validation. Prévoir précédent/suivant avec destination annoncée et position stable. Actualisation, thème, export disponible et déconnexion gardent des emplacements identifiables ; aucun bouton sans effet.
- Adapter la hauteur réservée sous les commandes, compacter en paysage/écran court et garder les raccourcis numérotés accessibles sur mobile. Tests320px,390px,desktop et paysage : pas de débordement global, aucune donnée cachée sous les commandes. Le citron reste réservé à l'action et à la sélection. Les thèmes clair/sombre, le focus, le clavier, le mouvement réduit et les états réels restent obligatoires.
- Ajouter de la matière utile près des vues : ce qui est observé, déclaré, inconnu, périmé ou non facturable, et l'action de reprise disponible. Conserver les motifs serveur comme données. Les badges, tableaux, filtres, paginations, détails et erreurs restent fonctionnels ; aucun nouveau chiffre ni événement d'exemple dans le produit.

Réalisation initiale : tokens/CSS, marque typographique, login, shell et navigation dans `front/src/{theme.css,style.css,main.ts}`, sans modification API/identité/SSE/stockage du jeton. Les tests navigateur vérifient géométrie stable, thèmes,320/390px/paysage, clavier et états. `control-view.ts` est conservé et ses vues utilisent la nouvelle feuille de style. Au gel éditorial initial, les 27 tests sous Node24, le build, le parcours navigateur et la revue indépendante passent ; le build statique est servi par le dashboard Rust conteneurisé et le parcours réel passe10 contrôles. Le français existant est conservé ; aucun sélecteur de langue inactif n'est ajouté sous prétexte de reprendre l'exemple du guide. Les mesures de contraste et les tests clavier ne constituent pas un audit WCAG complet.

### Complément OVPT et thème sombre — demande du 30 octobre 2026

Les références du mainteneur sont le logo OVPT blanc sur noir et la bannière OVPT noire sur fond clair, avec séparateur et devise « Omnia Vincit Progressus Technologicus ». Leur adaptation est autorisée ; ai-guard conserve son nom de produit et ses contrats existants. Une signature typographique locale conserve la netteté aux tailles du header. Le favicon doit rester identifiable à16 et32px sur fond de navigateur clair et sombre ; une déclinaison compacte peut remplacer les quatre lettres à cette taille. Les images originales ne sont pas altérées. L'identité fournie ne désigne pas à elle seule un propriétaire légal ou un canal de support.

Le thème sombre précédent, dominé par des fonds olive, a été remplacé par des noirs neutres en couches distinctes, du texte ivoire, des métadonnées lisibles et des traits fins visibles. Le citron reste réservé à l'action ou à la sélection ; Suivant désactivé prend un fond gris neutre. Les mêmes surfaces et états que dans le clair ont été vérifiés, y compris authentification, tableaux denses, détails, focus et erreurs. Les commandes restent stables et le bouton de connexion visible à320×568 sans défilement initial. Le [lot OVPT livré](engineering/evidence/2026-10-30/ovpt-ui/README.md) rassemble les captures desktop/mobile/paysage, le contrôle du favicon16/32, les tests fonctionnels, la seconde revue et le parcours sur le dashboard Rust réel. La qualification globale du produit reste distincte.

Le mainteneur retient [Nothin’](https://www.noth.in/?ref=siteinspire) comme référence visuelle, à suivre autant que possible. Sa page d'accueil a été consultée le 29 octobre 2026 : typographie affirmée, hiérarchie de tailles marquée, base noir/blanc, grandes respirations, compositions franches et accents ponctuels. Ces principes guident les prochains chantiers visuels de l'IHM de contrôle.

Pour ai-guard, les titres structurent les vues, les actions essentielles se repèrent immédiatement et les listes, tableaux, logs et détails conservent une densité utile. Les couleurs sémantiques restent explicites pour les décisions, erreurs, inconnues et provenances. L'identité du client authentifié et les états de lecture demeurent faciles à distinguer.

La revue d'un changement visuel compare les vues principales à cette référence et vérifie desktop/mobile, thèmes clair/sombre, navigation clavier, focus, contrastes, mouvement réduit et activité dense. Elle inclut les états chargement, vide, partiel, périmé, déconnecté, interdit et erreur. Les captures complètent les parcours fonctionnels avec données réelles ; les preuves de l'interface livrée restent dans [VALIDATION.md](engineering/VALIDATION.md).

## Navigation entre ressources du tenant

Les détails agent/sandbox chargent réellement le registre workers et permettent d'en parcourir les pages ; un détail worker présente les agents et sandboxes correspondants. Les relations reposent sur les identifiants exacts de session ou de runtime reçus, avec provenance explicite ou inconnue. Elles ne prouvent ni délégation ni exécution et n'attribuent aucune identité de broker. Chaque source indique lecture, fraîcheur, périmètre et erreur éventuelle ; l'absence de correspondance dans une page ne signifie pas absence du tenant. La reprise d'une source retire son erreur résolue sans masquer une erreur indépendante. Le focus reste dans le dialogue lors des changements de liens et des reprises.

Cette navigation utilise les API de lecture existantes du tenant `viewer`, sans nouvel endpoint ni accès moteur. Les lectures nominatives, dont celle du registre brokers, ont leur périmètre distinct décrit ci-dessous ; elles ne livrent pas la fédération ni la vue unifiée à 360°. [Validation ciblée et limites](engineering/VALIDATION.md#1er-novembre--navigation-liée-du-dashboard-et-reprises).

## Lecture nominative du registre brokers

La vue **Brokers** est une lecture des métadonnées d'infrastructure du tenant authentifié, dans le même dashboard Rust/TypeScript. Elle exige un contexte **administration** portant la capacité explicite `read_broker_registry`. Le contrôle Rust établit ses droits ; un rôle IdP, un nom de compte, `read_tenant_metadata` seul ou une sélection locale « admin » ne suffisent pas. L'ancien droit `read_tenant_metadata` conserve son filtre d'organisation sur les ressources. L'attribution/réactivation du nouveau droit appartient au contrôle authentifié/hors ligne, jamais à un administrateur d'organisation ; celui-ci peut conserver le droit existant, le garder dormant ou le retirer dans ses anciens droits. Le [guide EN](setup/identity/README.md#authorize-named-broker-registry-readers) et le [guide FR](setup/identity/README.fr.md#autoriser-la-lecture-nominative-du-registre-brokers) décrivent migration7, membership typé et reprise ; [SSO EN](setup/sso/README.md#opt-in-to-broker-registry-reads)/[FR](setup/sso/README.fr.md#activer-explicitement-la-lecture-du-registre-brokers) ajoutent le plafond explicite sur claims vérifiés.

Après connexion et choix d'un contexte actuellement proposé, un contexte ne possédant que ce droit ouvre directement **Brokers enregistrés du tenant.**. Il ne reçoit pas implicitement `read_metadata` et ne lance aucune lecture automatique ressources/événements/export ou leur SSE. Dans un contexte mixte, entrer dans Brokers arrête les lectures/flux de l'ancienne vue ; en sortir annule et purge page brokers, positions et détails. Nouvelle identité, changement de contexte, expiration, révocation ou logout invalident les générations concernées : une réponse ou callback tardif ne repeuple pas une vue cachée. Un gain de droits exige un contexte courant et une sélection explicite.

Le tableau **Registre des brokers** indique **Broker / incarnation**, **État du bail**, **Autorités déclarées**, **Dernière réception observée**, **Expiration** et **Détails**. **Détails du broker** reprend uniquement les métadonnées de la page datée : incarnation courante, révisions exactes, heures d'autorisation/révocation, heartbeat et reçu durable du contrôle. Les révisions et séquences restent des chaînes décimales, sans conversion flottante. Une autorité masquée affiche **Identifiant masqué** ; son ID vaut `null` et son runtime reste déclaré. Les éléments sensibles, grants bruts et credentials ne sont pas une vue cachée à révéler dans le détail : ils sont exclus du contrat humain.

| État serveur | Libellé réel de la vue | Lecture correcte |
|---|---|---|
| `unseen` | Heartbeat non reçu | Aucun heartbeat pour l'incarnation courante ; une ancienne réception peut rester visible. |
| `current` | Bail courant | Bail enregistré courant, sans preuve de worker en fonctionnement. |
| `stale` | Bail périmé | Dernier bail non courant ; état runtime et quota restent inconnus ici. |
| `expired` | Autorisation expirée | Fin de validité du grant, sans effet d'arrêt observé déduit. |
| `revoked` | Autorisation révoquée | Révocation du registre, sans arrêt moteur ou libération de quota déduits. |
| `clock_uncertain` | Horloge incertaine | L'évaluation ne permet pas une fraîcheur fiable. |

**Heartbeat déclaré · réception observée** distingue l'assertion du broker de sa réception par le contrôle. **Heartbeat d’une ancienne incarnation** garde visible ce fait historique sans le transférer à la nouvelle incarnation. Le pied de vue conserve **Évaluation serveur**, `source tenant_broker_registry`, `cohérence live_keyset` et l'inventaire runtime inconnu. Le détail d'une ligne provient de cette page, sans endpoint de détail ni nouvelle observation moteur. Kubernetes est une autorité déclarée ; aucune création de pod n'est démontrée. Les lignes doivent provenir d'un véritable enregistrement/grant/heartbeat du contrôle : aucune fixture ou activité inventée dans le produit.

Le client demande25 lignes, avec un plafond serveur de100 éléments et256Kio pour l'enveloppe complète. **Page suivante**, **Page précédente** et **Première page** utilisent les curseurs de cette traversée live ; aucun total global ou snapshot n'est inféré. Les lignes peuvent changer entre pages. Les positions restent liées au contexte, à sa version d'autorisation et à la limite ; les curseurs brokers ne servent ni aux ressources ni au flux. La traversée serveur est bornée à1000pages/16Mio dans le contexte d'au plus300secondes. Les détails se ferment par Échap et retrouvent leur déclencheur quand il reste disponible ; le tableau dispose d'une zone de défilement clavier sur petit écran.

Les états **Chargement du registre…**, **Registre indisponible**, **Aucun broker enregistré à cette lecture** et **Fin de cette traversée live** sont distincts. Seule une lecture réussie permet d'affirmer une page vide, sans déduire zéro worker. Une erreur après succès conserve la dernière page datée avec **Données périmées · dernière lecture conservée**, sauf si les droits imposent son retrait. Un403 retire page, détails et curseurs, interdit les lectures automatiques et propose **Relire mes accès**. Le client traite401 comme une fin de session et exige un contexte proposé après expiration/révocation. Aucun repli viewer/runner.

L'actualisation automatique est déclenchée au plus toutes les30secondes lorsque cette vue est visible et autorisée ; **Actualiser** permet une action explicite. Ce rythme ne garantit aucun délai de réponse, de notification ou de retrait des droits : latence, files d'attente, suspension du navigateur et panne de contrôle restent possibles. Il n'existe pas de flux SSE brokers ; le flux des ressources ne couvre pas le registre.

Le [GET humain](api.md#registre-brokers-humain--human-broker-registry) exige un reçu durable avant toute restitution, même vide, dans les budgets humains F1 tenant/principal partagés. Le reçu prouve l'enregistrement de la lecture, ni sa livraison ni une exécution runtime. Le Store projette et audite dans une transaction ; la route réauthentifie avant publication. Une révocation peut donc laisser un reçu durable sans publier la page. Une panne d'audit bloque la lecture. Le navigateur ne reçoit ni clé technique, grant brut, credential workload, ni accès moteur Docker/Podman/Kubernetes.

Un429 `audit_read_budget_exhausted` avec `contact_operator`, y compris durant le préflight identité du proxy dashboard, arrête les lectures automatiques. Tick, visibilité, navigation ou resélection ne réarment pas ce refus cumulatif. La dernière page encore permise reste datée/périmée. Après intervention sur l'occupation/politique d'audit, **Relire après intervention de l’opérateur** autorise une tentative explicite ; seul un succès courant permet de reprendre. Un429 temporaire de capacité ou un503 transport reste distinct. Un503 `migration_required` demande une migration hors ligne6→7 sauvegardée. Ne pas supprimer d'audit, réinitialiser la base ou renouveler les credentials pour contourner un budget.

Cette vue ne remplace pas la cible à360° ci-dessous. Elle ne qualifie ni fédération multi-brokers, connexion runtime distante, agrégation workers/agents/sandboxes/sessions/usage, capacité100 ou release production. L'usage non observé reste inconnu et l'allocation non facturable. Les tests de DTO et fixtures frontend ne prouvent pas un parcours réel : le futur contrôle sur dashboard/gateway conteneurisés avec vrais logins local/OIDC et brokers réellement enregistrés doit corréler rendu, contexte, version d'autorisation et reçu, refus A/B, purge et logout. Sa preuve reste distincte dans l'état/validation d'ingénierie ; aucune capture de fixture ne la remplace.

## Supervision unifiée nominative — candidat du 10 novembre 2026

La destination **Supervision** du dashboard nominatif réunit sur une page les sources que le contexte sélectionné accorde : registre des brokers, registre des workers et ressources autorisées (sessions, agents, sandboxes, projets). Elle s'ajoute aux vues existantes sans changer leurs atterrissages : chaque contexte proposé garde **Ouvrir …** et gagne **Superviser administration · tenant** ou **Superviser personnel · tenant**. Le parcours, les libellés, la table des refus et le budget de lecture sont décrits dans les guides [anglais](setup/supervision/README.md) et [français](setup/supervision/README.fr.md) ; les preuves et écarts de la composition sont dans le [registre d’intégration](engineering/evidence/2026-11-10/unified-control-ui-integration/README.md).

Différences avec les registres exacts **Brokers**, **Workers** et **Fédération**, qui restent la référence pour parcourir tout un registre page par page :

- lecture de 100 lignes par page au lieu de 25, fenêtre bornée (500 workers, 100 brokers, 300 ressources) et jamais de total du tenant : tout compte est « n affichés sur N chargés » ;
- recherche, filtres et tri appliqués aux seules lignes chargées, annoncés comme tels ; le registre n'offre aucun filtre d'état côté serveur ;
- jointures par identifiant exact seulement : broker ↔ worker par les associations d'autorité renvoyées par le contrôle, worker ↔ session par la référence déclarée ; la relation agent / sous-agent n'est pas fournie par le contrat nominatif et reste affichée comme inconnue ;
- bandeau par source (fraîcheur, dernière lecture réussie, couverture) où une lecture en échec, une source non accordée et une lecture vide réussie restent trois états distincts ; la composition est toujours annoncée partielle, l'inventaire runtime inconnu ;
- actualisation de la seule flotte à un rythme réglable (30 s, 2 min, manuel), arrêtée onglet masqué, au-delà de deux pages chargées, après un refus ou à l'arrêt du budget durable ; un contexte sans registre n'affiche aucun rythme et l'annonce comme « non accordé », jamais comme un refus ;
- un `403` de la lecture composée arrive sans motif lisible : toutes les sources et tout détail ouvert sont retirés et l'identité est relue une fois, ce qui distingue un contexte révoqué ou expiré (retour au choix du contexte) d'un registre refusé (les ressources sont relues) ;
- une seule région d'annonce par vue, dans son bandeau (une phrase à chaque changement d'état d'une source, jamais lors d'une actualisation automatique sans changement), et une alerte créée une fois par erreur ; les lignes de résultat suivent chaque frappe et ne sont pas des régions d'annonce ; en Supervision, la région du bandeau dit le compte d'une liste une fois, après une pause d'une demi-seconde qui suit une recherche ou un filtre, et à la fin d'une lecture qui le change (première lecture, page chargée, actualisation qui change le nombre de lignes), jamais pour une actualisation automatique qui ne change aucun compte ; une lecture demandée par la personne dit toujours son issue : lorsqu'elle n'a changé ni état ni compte, la région redit l'état des sources, ou l'erreur encore affichée après un nouvel échec identique ; une phrase identique à celle que la région porte déjà est dite en vidant la région puis en l'y réécrivant une fois, un dixième de seconde plus tard (écritures constatées dans la page, aucune aide technique exécutée) ; un détail ouvert a sa propre région et annonce en une phrase l'issue d'une lecture d'événements de session, jamais leur liste ; dans le Journal, la ligne de résultat change à chaque lot : les événements en direct le sont par leur nombre, au plus toutes les 15 secondes, le résultat d'une recherche ou d'un filtre après une pause de saisie, et le résultat d'une lecture demandée par la personne (page plus ancienne, limite à une ressource ou son retrait, relecture) une fois, quand cette lecture a réussi et que le direct qu'elle rouvre est établi ou refusé, dans la même phrase que l'état s'il change, jamais pour un lot en direct ;
- des lignes conservées d'une publication à l'autre : une ligne inchangée n'est ni reconstruite ni déplacée, même lorsqu'une autre ligne change de place dans le tri, si bien que focus, sélection de texte et clic en cours survivent à une actualisation ou à un lot en direct ; une ligne modifiée est reconstruite et rend le focus à la même commande ; un changement de l'ordre de tri conserve le focus des lignes déplacées, pas leur sélection de texte ; un détail ouvert est mis à jour section par section, une actualisation sans autre changement n'y remplaçant que la ligne datée ou le reçu de lecture ; dans les vues Supervision et Journal, leur détail et l'offre de reprise, une commande qui lance une lecture (actualisation, relecture, page suivante, événements d'une session, « Réessayer », « Reprendre », « Actualiser » de la barre de destinations) reste focalisable tant qu'une lecture est en vol : annoncée indisponible (`aria-disabled`), elle ignore une nouvelle activation et garde le focus pendant la lecture et après elle ; si sa lecture la fait disparaître, le focus passe à ce qui la remplace, jamais au corps de la page ; seul l'arrêt du budget de lecture la désactive ou la retire ;
- à 740 px et moins, les tables denses de cette vue deviennent des blocs étiquetés sans défilement horizontal, et la barre de destinations devient une rangée unique défilable à partir de cinq destinations, à hauteur de dock constante ; à toute largeur, le bord qui cache d'autres destinations s'estompe et une destination qui prend le focus est ramenée entièrement dans la zone visible.

La destination **Journal** liste les événements autorisés du contexte, les plus récents d'abord (`GET /api/v1/human/events/recent`, 100 par page, fenêtre de 1 000), puis ouvre le flux existant à la position renvoyée avec la page lue : le direct ne rejoue pas l'historique, insère les nouveaux événements en tête sans doublon et affiche son état (connexion, connecté, reconnexion, suspendu onglet masqué, refusé, arrêté). Une indisponibilité temporaire annoncée par le contrôle (`retry`) est une déconnexion : le flux est rouvert depuis la dernière position livrée, avec un délai croissant et six tentatives au plus, puis s'arrête en le disant. La limite à une ressource est son seul filtre serveur. Les vues **Ressources** et **Événements** gardent leur lecture depuis le plus ancien et leur flux depuis le début de l'historique autorisé.

Après l'expiration d'un contexte (300 secondes au plus, jamais renouvelé), les données sont retirées et l'identité relue. Si le contrôle propose exactement un contexte de même attribution, la liste des contextes offre **Reprendre la supervision · …** ou **Reprendre le journal · …**, bouton qui reçoit le focus ; aucune donnée n'est lue avant ce clic, qui rétablit la vue et les filtres saisis. L'offre a la durée du contexte proposé : passé ce délai elle le dit, et le clic relit alors les accès avant de rouvrir la vue sur le contexte équivalent proposé à ce moment, en répétant cette lecture sans autre clic tant que le contrôle propose encore le contexte expiré (trois lectures automatiques au plus pour chaque clic) ; un onglet masqué avant la réouverture de la vue met fin à cette confirmation, même si le contrôle propose entre-temps un contexte valide : rien ne se rouvre et aucune de ces relectures ne démarre onglet masqué, la page le dit, et au retour sur l'onglet les accès sont relus : la reprise est proposée de nouveau, avec le focus et pour un nouveau clic, si le contrôle propose alors un contexte de même attribution ; onglet masqué, il ne reste que la fin d'une lecture déjà en vol et, à la perte du contexte, le retrait immédiat de ses données suivi d'une lecture d'identité ; un renouvellement de session OIDC est nommé comme tel, pas comme une expiration. Révocation, changement de droits, refus, déconnexion et navigation volontaire n'offrent aucune reprise.

Une vue détient les données serveur à la fois : quitter Supervision annule ses lectures et retire ses lignes et détails ; les filtres saisis sont conservés tant que le même contexte reste sélectionné, puis effacés avec lui. Cette vue ne livre pas la cible à 360° ci-dessous : ni utilisateurs et appartenances, ni quotas, usage, politiques, configuration effective ou mutations d'administration, ni vue sur plusieurs tenants, ni vue personnelle des workers.

Le Journal nomme les remplacements et demandes de révocation de politique réseau attachés à une session autorisée. Pour ces événements natifs observés par le gateway, il indique la couverture du registre, conserve le type technique et affiche « Aucune décision de connexion ». Il utilise les métadonnées de la sélection autorisée. La couverture des tentatives et connexions effectives relève du raccord egress runtime. [Recette du Journal egress](../tests/e2e/README.md#rejouer-le-journal-des-politiques-egress-dans-chromium)

## Vues utilisateur et administration unifiée — cible à livrer

Complément du mainteneur, 31 octobre 2026 : l'IHM doit refléter les [axes de configuration](sso.md#configuration-par-axes-indépendants), l'identité, les permissions effectives et le périmètre autorisé. **Cible complète à implémenter et qualifier.** Les neuf vues techniques `viewer`, la consultation nominative des ressources et la lecture limitée du registre brokers ci-dessus ne constituent pas cette administration complète. Leurs preuves ne qualifient pas les parcours administrateur ci-dessous.

Conserver une seule application frontend et une identité visuelle commune, avec des vues utilisateur et administrateur adaptées. La navigation, les informations et les commandes découlent d'un contrat typé d'identité, de périmètres et de capacités établi par le contrôle Rust ; le navigateur ne déduit pas des droits d'un nom de compte ou d'un claim non vérifié. Le nom externe de rôle reste configurable. Un même utilisateur peut disposer de permissions différentes selon son périmètre ; distinguer le contexte personnel et l'administration lorsque les deux sont autorisés.

| Vue | Informations et parcours attendus |
|---|---|
| Utilisateur | Ses sandboxes et les ressources explicitement partagées avec lui, agents/sous-agents, sessions, demandes, décisions, événements, résultats et exports autorisés ; limites applicables, état de connexion du client IA et paramètres qu'il peut réellement modifier |
| Administrateur | Vue unifiée de l'organisation et des tenants explicitement administrés : utilisateurs et appartenances, agents/sous-agents, sessions, sandboxes, workers, brokers/hôtes/runtimes, quotas, usage observé, politiques, décisions, audit et incidents ; gestion selon les seules permissions déléguées |
| Synthèse à 360° | Situation globale puis accès aux détails par utilisateur, tenant autorisé, projet, session, agent, worker ou broker ; filtres cohérents entre vues, liens de causalité et retour au contexte sans devoir consulter un dashboard par worker |
| Configuration et diagnostic | Profils IA effectifs, modèle, mode abonnement/API/interne, provenance des réglages, paramètres réseau/TLS et identité locale/OIDC selon les droits ; prérequis manquants, restrictions administratives et actions de reprise explicites, sans afficher de credential |

La vue à 360° couvre tout le périmètre autorisé, y compris plusieurs brokers ou plusieurs tenants rattachés à une organisation lorsque le contrôle l'autorise. Elle ne donne aucun accès implicite aux autres clients. Les droits sur métadonnées, contenus sensibles, export et mutations sont distincts ; voir une ressource ne donne pas automatiquement le droit d'en consulter tous les messages ou de l'administrer. La configuration personnelle simple sans SSO ne doit pas imposer les écrans de gestion d'entreprise.

Le control plane autorise et agrège les sources ; le frontend ne fusionne pas des périmètres en faisant autorité et ne reçoit aucun credential global. Les bases clients restent distinctes. Chaque synthèse précise portée, filtres, période, provenance, fraîcheur et sources absentes ou déconnectées. Un total global ne se déduit pas d'une page chargée ; aucune collecte partielle ne devient zéro activité, aucune réservation ne devient consommation réelle ou montant facturé. Les états et configurations affichés sont fournis par les API, sans compteur ni capacité inventés.

Le changement de rôle, de périmètre, la révocation, l'expiration ou la déconnexion doivent invalider les lectures et abonnements concernés et retirer les données devenues interdites, y compris détails ouverts et caches. Les URL directes, recherches, exports et flux temps réel respectent les mêmes autorisations côté serveur. Les refus, limitations et mises à jour manquantes restent visibles avec un moyen de reprise approprié. Le dashboard conserve les thèmes, la navigation clavier, les commandes stables et la lisibilité à 320/390 px et sous charge.

Les opérations de gestion passent par des contrats API Rust explicites, validés et audités avant effet ; aucun accès moteur depuis le navigateur. Ne pas ajouter de commande sans effet, d'écran de démonstration ni de sélection locale « admin » pour simuler ce comportement. La livraison exige ensemble les contrats serveur, l'agrégation autorisée, les vues frontend, la documentation et les [preuves de rôles et de fraîcheur](engineering/TEST_STRATEGY.md#identité-individuelle-sso-et-découverte--à-qualifier).

## Périmètre livré

Le dossier `front/` contient une interface TypeScript et Vite sans framework ni dépendance JavaScript d’exécution. Les neuf vues sont en français : vue d’ensemble, agents, actions, sécurité, fichiers, sandboxes, workers, quotas et allocations. La production utilise les fichiers statiques de `front/dist/`, servis par le service Rust dashboard. Aucun hébergement public ni service cloud n’est nécessaire.

Ces neuf vues appartiennent au parcours technique `viewer`. Les parcours nominatifs [locaux](setup/identity/README.fr.md) et [OIDC Web/CLI](setup/sso/README.fr.md) disposent aussi de lectures de ressources et d'événements, d'exports et de contextes proposés par le contrôle. Le dashboard nominatif présente ces lectures selon les droits effectifs et, sur permission distincte, la [lecture du registre brokers](#lecture-nominative-du-registre-brokers). Ces fonctions ne constituent pas l'administration unifiée complète ; leurs qualifications et limites restent celles des guides et des preuves datées.

La vue d’ensemble présente les comptes fournis par `/api/overview`, les décisions, le journal récent, la santé des services, les ressources disponibles et l’intégrité du journal. Une valeur absente ou `null` est affichée « Inconnu » ; zéro reste affiché comme zéro. Aucun pourcentage de progression, consommation CPU, coût, débit, agent ou conteneur de démonstration n’est généré dans l’application.

Le statut « Flux connecté » indique une connexion SSE ouverte. Il ne constitue pas une preuve que le moteur de conteneurs ou un agent est opérationnel. Les horodatages sont affichés dans le fuseau du navigateur, avec la date complète dans le titre de l’heure et dans le détail.

## Authentification et frontières de confiance

Fonctionnement du parcours technique `viewer`, distinct des parcours nominatifs locaux/OIDC décrits dans les guides d'identité et SSO :

- Le formulaire demande le jeton **viewer**. Le champ est immédiatement vidé lorsqu’une tentative commence.
- Le jeton vit seulement dans une instance JavaScript en mémoire. Aucun cookie, paramètre d’URL, `localStorage`, `sessionStorage` ou IndexedDB ne le conserve.
- Les requêtes utilisent `Authorization: Bearer …`, `credentials: omit`, `cache: no-store` et `redirect: error`. Un refus 401/403 du client actif ferme la session, efface les données et détails, puis réaffiche la connexion, même si une autre lecture échoue simultanément ou si le détail a déjà été fermé. Une réponse tardive d'un ancien client ne déconnecte pas la nouvelle session.
- Le client n’accepte que les routes de lecture connues, sous `/api/` sur la même origine. Il ne contient aucune fonction de lancement, d’arrêt, de validation de politique ou d’écriture.
- Une actualisation de la page déconnecte la session. Le thème suit initialement le système, puis le bouton clair/sombre agit uniquement sur la page courante.
- Les commandes, noms d’agents, sorties et objets JSON sont traités comme du texte. Le DOM utilise `textContent`, `createTextNode` et des nœuds construits explicitement, sans `innerHTML`, `eval`, gestionnaire inline ou URL fournie par un agent.
- Le document inclut une CSP stricte : scripts et styles de même origine, aucune source externe, `object-src 'none'` et `base-uri 'none'`. Le service Rust ajoute ses en-têtes de sécurité, notamment la protection contre l’intégration en iframe. Aucune police ni icône n’est téléchargée depuis un CDN.

La CSP du serveur de développement est adaptée uniquement aux styles injectés par Vite : un nonce aléatoire par lancement est fourni à Vite et à la directive `style-src`. Les WebSockets sont limités aux adresses locales de développement. Le bundle de production conserve une politique sans scripts ni styles inline.

## Contrat HTTP

Les types partagés côté front sont dans `front/src/types.ts`.

| Route                          | Usage                                                  |
| ------------------------------ | ------------------------------------------------------ |
| `GET /healthz`                 | Disponibilité HTTP du dashboard, sans jeton            |
| `GET /api/overview`            | Comptes, état déclaré par le collecteur et ressources  |
| `GET /api/agents`              | Liste des identités et relations explicites            |
| `GET /api/sandboxes`           | Paramètres de conteneurs reçus                         |
| `GET /api/events`              | Journal paginé et filtré                               |
| `GET /api/stream`              | Événements `event: audit`, avec identifiant séquentiel |
| `GET /api/export?format=jsonl` | Export JSONL selon les filtres de la requête             |
| `GET /api/integrity`           | Résultat de vérification de la chaîne locale           |
| `GET /api/v1/workers?limit=50` | Registre durable des workers du client authentifié |
| `GET /api/v1/workers/{uuid}` | Relecture d’un worker et de sa dernière observation runtime |
| `GET /api/v1/quota` | Politique et réservations durables CPU/RAM/workers |
| `GET /api/v1/usage?limit=50` | Allocations terminales observées, méthode 1, non facturables |

La recherche transmet `q`, `agent_id`, `sandbox_id`, `project_id`, `kind`, `decision`, `from`, `to`, `limit=100` et éventuellement `before` via `URLSearchParams`. La requête d’historique retourne les événements en ordre décroissant. Le bouton « Charger les précédents » utilise `next_cursor`.

Le sélecteur de période propose toutes les périodes, la dernière heure, 24 heures et 7 jours. Il fixe une date `from` RFC3339 au moment du choix et l’affiche sous les filtres. Cette date reste identique lors d’une actualisation, d’un filtrage, d’une navigation, de la pagination et de l’export ; elle est recalculée uniquement au choix d’une autre période. L’absence de borne `to` conserve l’arrivée des nouveaux événements en direct. Le filtre SSE utilise `received_at`, comme l’API, et se replie sur `timestamp` seulement si l’horodatage de réception n’est pas disponible ou exploitable. Les types et le client prennent également en charge une borne `to` facultative.

Les vues Actions, Sécurité et Fichiers sont des projections des événements chargés ; leur pied de tableau indique le nombre affiché et le nombre chargé. La vue Fichiers se fonde sur les types `file.*`, `files.*`, `artifact.*`, `workspace.diff*`, `workspace.snapshot*`, `workspace.import*` et `workspace.export*`. Elle ne parcourt pas le système de fichiers. Un détail expose les données brutes reçues lorsque les chemins ou différences sont fournis dans l’événement.

Dans **Sécurité**, le détail d’une décision observée du gateway présente l’évaluateur attesté et son état. Le champ **Sélection de l’évaluateur** indique la provenance enregistrée par la gateway. On l’omet lorsque l’ancien audit ne fournit aucun choix reconnu. Le moteur de la décision effective conserve son champ propre. Pour Jev TypeSafe et le modèle local, les identifiants demandé et retourné et l’avis sont affichés quand l’audit les fournit. Un refus impératif local peut laisser l’évaluateur configuré non sollicité. Une panne laisse le modèle retourné et l’avis absents. Le pouvoir d’autoriser reste à la politique Rust pour un avis local. Les données brutes restent consultables.

L’export utilise un `fetch` authentifié, puis un `Blob` et une URL objet temporaire. Il reprend tous les filtres appliqués : `q`, `agent_id`, `sandbox_id`, `project_id`, `kind`, `decision` et les bornes de période `from`/`to`. Il inclut tous les résultats correspondants, sans reprendre `before`, `after` ou `limit` de pagination. Sans filtre appliqué, il contient le journal complet retourné par l’API. Les filtres sont figés au clic pour toute la requête d’export. Le jeton n’est jamais ajouté au lien de téléchargement.

## Temps réel et états incomplets

Le flux est lu avec `fetch`, `ReadableStream` et un `TextDecoder` UTF-8. Le parseur supporte les coupures entre fragments, CRLF/LF/CR, commentaires, champs `data` multiples et identifiants. Un message trop volumineux déclenche une reconnexion au lieu de laisser croître le tampon sans borne. Les objets ne possédant pas les champs essentiels d’un événement sont ignorés. Le curseur n’avance qu’après réception d’un événement valide.

Les événements sont dédoublonnés par `seq`. Après interruption, le client se reconnecte avec `Last-Event-ID` dans l’en-tête et un délai exponentiel de 1 à 30 secondes. Aucune authentification n’est passée en paramètre de l’URL SSE. La déconnexion annule les requêtes et la boucle de reconnexion. Les événements live reçus pendant une requête d’historique sont conservés lors de sa résolution.

Une reconnexion, un refus d’authentification et une API indisponible ont des états dédiés. Les erreurs de lecture ne sont pas présentées comme la preuve d’une liste vide. Les résumés sont relus après les événements, regroupés sur une fenêtre d’une seconde, et toutes les 30 secondes. Le tableau live conserve au plus 1 000 événements ; l’historique antérieur reste rechargeable par curseur.

## Workers, quotas et allocations

Ces trois vues lisent les contrats v1 du contrôle, sans déduire un worker des journaux fournisseurs ni des sandboxes historiques. Le périmètre est celui du client authentifié par le jeton ; aucun sélecteur, header ou paramètre ne choisit un autre client. Aucune commande de lancement, arrêt, réconciliation ou modification de quota n’est disponible dans le navigateur.

Le registre sépare l’état durable (`reserved`, `preparing`, `created`, `start_issued`, `running`, `stopping`, `terminal`, `in_doubt`, `cancelled`) de l’observation runtime datée. Un démarrage émis ne devient pas une exécution observée. Sans observation runtime, l’interface indique l’inconnue. Le détail relit l’identifiant réel et expose session, tentative, révision, runtime, ressources, limites, dernière observation, motif d’incertitude et résultat de nettoyage. Le modèle de consultation conserve uniquement ces champs nommés ; les commandes et métadonnées de credentials sont exclues du détail. Cette sélection d’affichage ne remplace pas la protection des réponses côté serveur. Un détail d’agent propose les workers de la page chargée dont la session ou l’identité runtime correspond exactement ; aucune relation n’est déduite des noms ou fournisseurs. L’absence de correspondance sur une page partielle ne prouve pas l’absence de worker.

La table et le détail distinguent **Autorité déclarée du lancement** (`launch.authority_id`) et **Autorité de la dernière inspection enregistrée** (`last_observation.inspection.authority_id`). Cette dernière accompagne le runtime de l’inspection (`runtime_kind`) et sa date `observed_at`, sans remplacer cette date par l’heure de lecture du navigateur. Un champ absent ou `null` reste **Inconnue** ; sans observation, aucune autorité d’inspection n’est déduite du lancement. Ces identifiants ne prouvent ni l’hôte ou le broker associé, ni une disponibilité actuelle ou un inventaire runtime complet.

Les autorités reçues sont conservées exactement comme des chaînes non vides d’au plus 256 octets UTF-8 sans caractères de contrôle. Une valeur invalide produit une erreur de contrat. Le détail affiche aussi les empreintes de l’image et du plan de lancement. L’inspection conserve ses propres valeurs de conteneur, d’image et d’empreinte du plan, avec son autorité, son runtime et sa date. Une valeur historique absente ou nulle s’affiche comme inconnue. La projection de consultation est limitée aux champs nommés. Les [guides locaux anglais](setup/local/README.md#open-the-control-view) et [français](setup/local/README.fr.md#ouvrir-la-vue-de-contrôle) décrivent cette lecture dans le parcours utilisateur.

Les quotas exposent les ressources réservées et les limites de la politique du client. Un quota peut rester retenu pour un worker incertain. Les octets exacts accompagnent les unités de mémoire lisibles. Une valeur inconnue n’est pas un quota nul ; les réservations ne représentent pas une mesure de CPU consommé.

Le relevé d’allocation utilise exclusivement les observations terminales du ledger. Les durées et produits CPU/RAM sont des chaînes décimales conservées sans conversion en nombre flottant : durée en secondes, CPU alloué en milliCPU·s et mémoire allouée en octet·ns. La provenance observée, le runtime et l’autorité fournis par la source accompagnent la période. Il affiche la méthode 1 et `billable=false`, sans prix, coût, facture ou total extrapolé à partir d’une page. L’absence d’allocation terminale laisse la durée inconnue, y compris pour un worker actif ; le signal durable d’allocation incohérente reste visible sur le worker. Un contrat incomplet, une source sans provenance observée, une méthode inconnue ou une unité invalide produit une erreur de lecture explicite.

Workers et allocations utilisent une page de 50 éléments et un curseur UUID opaque fourni par l’API, avec page suivante, page précédente et retour à la première page. Une seule page de données est conservée par vue ; la position de navigation est mémorisée pendant la session. Le résultat n’annonce aucun compte global à partir du nombre d’éléments chargés. Le focus clavier reste dans la vue lors d’une pagination et les tableaux peuvent défiler horizontalement sur petit écran.

Les états chargement, vide, erreur, lecture périmée et flux déconnecté sont distincts. Chaque lecture réussie porte sa date ; une erreur ultérieure conserve les dernières données avec leur avertissement et propose « Réessayer ». Un 401/403 ferme la session et efface les collections. Les vues se chargent à leur ouverture, puis sont relues toutes les 30 secondes lorsqu’elles sont ouvertes. Les événements `control.worker.*`, `worker.*`, `quota.*` et `allocation.*` invalident les données ; leur rafraîchissement est regroupé sur une seconde avec une seule requête en vol par ressource. Aucun appel de registre n’est envoyé pour chaque événement d’une rafale. Une reconnexion invalide également les données.

## Agents, provenance et couverture

Le tableau d’agents présente l’état, le fournisseur, le modèle, la provenance et la compatibilité reçus. La hiérarchie n’utilise que `parent_id` et les identifiants explicites. Elle ne déduit aucun parent à partir du nom, du modèle, du projet ou de la session. Un parent absent ou cyclique est signalé et l’agent reste visible. L’affichage imbriqué est limité à 30 niveaux ; le tableau reste disponible.

Les comptes principaux `agents` et `subagents` représentent les identités observées. Les comptes `declared_agents` et `declared_subagents`, lorsqu’ils sont fournis, sont ajoutés comme mentions séparées « + N déclarés » ; ils ne sont pas additionnés aux observations. Le détail d’un agent affiche `field_provenance` pour chaque champ renseigné, notamment modèle et tâche. La provenance globale de l’identité ne remplace donc pas celle de ses attributs. Les indications `parent_unresolved` et `hierarchy_warning` restent visibles même lorsque le collecteur a supprimé un lien parent invalide.

| Valeur technique        | Libellé français | Sens présenté                                   |
| ----------------------- | ---------------- | ----------------------------------------------- |
| `observed` (provenance) | Observé          | Mesuré ou constaté                              |
| `declared`              | Déclaré          | Fourni par un agent                             |
| `inferred`              | Inféré           | Déduit, à confirmer                             |
| `native` (couverture)   | Native           | Couverture native indiquée par la source        |
| `observed` (couverture) | Observée         | Couverture d’observation indiquée par la source |
| `generic`               | Générique        | Intégration générique indiquée par la source    |

Les badges de provenance et couverture sont distincts. Les valeurs techniques de provenance sont consultables dans le titre du badge. Aucun niveau n’est déduit de la marque d’un agent. La carte d’intégrité rappelle que le chaînage local ne protège pas contre une réécriture complète par un administrateur.

## Accessibilité et adaptation

L’interface propose une navigation sémantique, un lien d’évitement, des labels de formulaires, des états de flux annoncés, des erreurs annoncées, un focus clavier visible et des badges textuels indépendants de la couleur. Les détails utilisent un dialogue natif modal, refermable avec Échap. Le détail worker retrouve son bouton de déclenchement après une relecture du tableau pendant l’ouverture ; si la ligne disparaît, le focus rejoint le résumé de page, ou le titre de la vue lorsque le résumé n’existe plus. Les onglets de détail supportent les flèches, Début et Fin. Le tableau horizontal reste défilable dans son panneau sur petit écran, sans déborder la page. Le menu mobile fermé est retiré du parcours clavier. Les animations respectent `prefers-reduced-motion`.

Ces contrôles constituent une vérification fonctionnelle de base ; ils ne remplacent pas un audit complet WCAG avec lecteur d’écran.

## Développement et vérification

```bash
cd front
npm ci
npm test
npm run build
AGENT_SANDBOX_GATEWAY=http://127.0.0.1:8787 npm run dev
```

Vite écoute sur `http://127.0.0.1:5173` et relaie `/api` et `/healthz` vers la cible. Le port est strict pour conserver une origine de développement prévisible. `npm run preview` sert le build sur `127.0.0.1:4173` ; pour un usage complet, le serveur Rust dashboard fournit le relais de lecture.

`npm test` exécute les tests Node du client API, du modèle et du parseur SSE. Les fichiers `.mjs` importent directement les fichiers TypeScript grâce au support de Node 24 utilisé pour la validation. Node 24 est recommandé pour reproduire ces contrôles.

`npm run test:browser` exerce le bundle de production avec un serveur HTTP de fixtures isolé, limité à `127.0.0.1`. Playwright doit être disponible dans l’environnement de test, soit par résolution Node, soit dans `CODEX_PRIMARY_RUNTIME_NODE_MODULES`. `PLAYWRIGHT_CHROMIUM_EXECUTABLE` peut désigner un binaire Chromium existant. Les fixtures restent dans `front/tests/` et ne sont jamais importées par le bundle. Les captures dans `front/tests/artifacts/` sont des éléments de contrôle local, pas une démonstration de sessions Docker/Podman réelles.

Le parcours couvre le refus puis l’acceptation du jeton, le rendu d’une chaîne HTML malveillante, les détails de commande et JSON, la reconnexion avec curseur, la hiérarchie explicite, la recherche, l’export authentifié, l’absence de stockage de jeton, les thèmes, le format mobile, l’état vide et l’indisponibilité de l’API. Pour le contrôle v1, il vérifie également pagination et focus, détails réels de fixture, inconnue runtime, quotas, unités d’allocation au-delà de la précision flottante, lien explicite agent/worker, conservation des données périmées, refus d’un DTO incomplet, rafale de 200 événements et déconnexion sur 403. Ces données restent des fixtures de tests et ne qualifient pas une exécution réelle de workers.

La validation navigateur utilise un collecteur de test explicite ; elle n’atteste pas de l’exécution d’un agent réel ni d’un moteur Docker/Podman. La validation de la chaîne complète est décrite dans le dossier de validation général du projet.
