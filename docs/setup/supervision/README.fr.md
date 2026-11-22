# Vue de supervision unifiée

[English](README.md) · [Français](README.fr.md) · [Guides](../README.fr.md)

**Implémentation candidate d'un composant, pas une release qualifiée.** Ce guide décrit deux destinations du dashboard nominatif. **Supervision** est une page qui réunit le registre des brokers, le registre des workers et les ressources autorisées du contexte sélectionné. **Journal** liste tous les événements que ce contexte peut lire, les plus récents d'abord, puis en direct. Elle montre ce que le contrôle a enregistré et vous a restitué. Elle ne prouve ni qu'un worker s'exécute, ni qu'un broker est joignable, ni une capacité. L'interface est en français uniquement : ce guide cite chaque libellé tel qu'il est affiché. Les preuves, l'attribution des sources et les écarts ouverts sont dans le [registre d’intégration](../../engineering/evidence/2026-11-10/unified-control-ui-integration/README.md) ; le contrat visuel et fonctionnel est dans [ui.md](../../ui.md).

La vue exige un compte nominatif : [local](../identity/README.fr.md) ou [SSO](../sso/README.fr.md). Le jeton technique `viewer` ouvre les neuf vues historiques, pas celle-ci.

## Qui voit quoi

La navigation, les sections et les actions suivent uniquement le catalogue d’opérations (`operations`) que le contrôle renvoie avec le contexte sélectionné : une opération s’ouvre lorsqu’elle est `granted` et `available`, et son explication nomme les autres états (`not_granted`, droit détenu par l’autre contexte de la même appartenance, `migration_required`, `profile_unavailable`, `not_in_this_build`). La liste des capacités est affichée, jamais utilisée pour déduire un droit ; un profil de lecture choisit seulement un contrat de réponse. Le navigateur ne déduit jamais un droit d'un nom de compte ; chaque lecture est autorisée de nouveau par le gateway. Le tableau suivant décrit les contrôles qui omettent `read_profiles` (contrats de registre historiques). Le profil owned est détaillé ci-dessous.

| Capacité du contexte sélectionné | Ce que montre Supervision | Lecture émise |
|---|---|---|
| `read_worker_registry` et `read_broker_registry`, mode administration | Brokers avec leurs workers associés, table dense des workers | `GET /api/v1/human/federation/workers?worker_limit=100&broker_limit=100` |
| `read_worker_registry` seule | Table des workers ; brokers annoncés « non accordé » | `GET /api/v1/human/workers?limit=100` |
| `read_broker_registry` seule | Table des brokers ; associations de workers annoncées inconnues | `GET /api/v1/human/brokers?limit=100` |
| `read_metadata`, ou `read_tenant_metadata` en mode administration | Sessions, agents, sandboxes et projets autorisés | `GET /api/v1/human/resources?limit=100` |
| Mêmes capacités de métadonnées | Onglet **Journal** : événements autorisés, les plus récents d'abord, puis en direct | `GET /api/v1/human/events/recent?limit=100&view=metadata`, puis `GET /api/v1/human/stream?…&cursor=<live_cursor>` |
| Aucune de ces capacités | Pas d'entrée Supervision pour ce contexte | aucune |

Une source que le contexte n'accorde pas n'est jamais demandée. Le panneau **Accès de ce contexte** (replié par défaut, ouvert par son chevron) liste chaque opération du catalogue du contrôle avec l’un de quatre états — **Accordé** (accordé et servi), **Non accordé**, **Autre contexte** (détenu par l’autre contexte de la même appartenance) et **Indisponible** (accordé mais non servi, avec sa raison : migration, profil de sources ou build) — et ce qu’elle ouvre. Deux éléments y figurent toujours comme indisponibles : la vue personnelle des workers (aucune chaîne authentifiée d'initiation ne relie encore un worker à une personne) et la hiérarchie agent / sous-agent (le contrat nominatif ne la renvoie pas ; elle reste inconnue et n'est jamais déduite d'un nom).

## Profil de supervision des brokers owned

**Candidat frontend ; exécution et qualification visuelle à consigner sur la source intégrée.** Mettre à jour ensemble gateway Rust et dashboard. Sur un contrôle owned, `GET /api/v1/human/identity` annonce exactement :

```json
{"read_profiles":{"broker_registry":"human_broker_registry_owned_v1","worker_registry":"unavailable","federation_registry":"unavailable"}}
```

Cette annonce reste indépendante des permissions du contexte. La lecture exige un contexte d'administration actuellement proposé par le contrôle portant `read_broker_registry`. Le dashboard choisit `GET /api/v1/human/owned/brokers` avant de demander une page : 25 lignes dans **Brokers**, 100 dans **Supervision**, au plus 100 lignes et 256 Kio par réponse. Il valide profil, version, champs fermés, contexte sélectionné, version des autorisations et reçu. La page est `live_keyset`, l'inventaire runtime est `unknown`, et `count_scope: page` désigne le compte de la page, jamais un total du tenant. Les curseurs owned appartiennent uniquement à ce profil et à la taille de page. Un 403, 404, 503 ou corps incompatible ne déclenche jamais de lecture du registre historique.

Les destinations **Workers** et **Fédération** sont indisponibles pour ce profil annoncé. Supervision continue de lire indépendamment les brokers owned et les ressources autorisés. Si `read_worker_registry` est présent, **Workers · profil indisponible** explique que la route est indisponible ; aucune lecture worker n'est émise et aucune activité nulle n'est affichée. **Relire mes accès** revient à l'offre courante du contrôle. Faire vérifier les profils pris en charge par l'opérateur ; cette action n'active aucune capacité manquante. La supervision multi-hôtes complète des workers reste une exigence ouverte.

La ligne du broker et son détail distinguent fraîcheur du heartbeat et fraîcheur du checkpoint :

| Statut du checkpoint | Sens affiché |
|---|---|
| `received` | **Checkpoint courant**, **Checkpoint périmé**, **Checkpoint historique** ou **Horloge incertaine du checkpoint** ; publication déclarée, réception observée, couverture partielle |
| `absent` | **Aucune publication de checkpoint** : aucune publication dans l'historique complet des opérations du broker ; activité toujours inconnue |
| `unknown` | **Checkpoint inconnu** : la recherche bornée n'a pas établi si une publication existe ; activité toujours inconnue |

Un checkpoint reçu affiche ses propres réception/expiration, relation à l'autorisation courante, compte de références et `has_more`. Même zéro référence ne mesure ni l'activité ni les workers du tenant. Identifiants de jobs, hashes, identifiants d'opération, credentials, commandes et chemins sont exclus. Un identifiant d'autorité peut être masqué ou explicitement libellé **Identifiant blanc** lorsque sa valeur valide ne comporte que des espaces. Un heartbeat périmé peut signaler un broker déconnecté ; l'interface n'en déduit ni déconnexion physique, ni arrêt de worker, ni libération de quota. Les deux états d'horloge/fraîcheur restent indépendants. Une seconde intercalaire RFC3339 valide que le navigateur ne sait pas formater reste visible sous sa chaîne serveur originale avec **date serveur conservée, conversion locale inconnue** ; elle n'est jamais normalisée vers la seconde suivante. La fraîcheur serveur reste celle du serveur, sans nouvelle inférence locale. L'authentification et l'expiration des contextes gardent leurs validateurs séparés.

Chargement, page vide réussie, premier échec de lecture et dernière page réussie datée après un échec d'actualisation restent distincts. Un détail **Brokers** ouvert reste lié à sa page datée pendant les actualisations ordinaires du heartbeat ; il se ferme lorsque son broker quitte la page ou que son autorisation, son incarnation ou ses autorités divulguées changent. Un 403 retire lignes, détails et curseurs dès les en-têtes, sans attendre le corps. Dans Supervision, il invalide aussi les lectures concurrentes avant relecture de l'offre d'accès du contrôle. Une annonce `read_profiles` modifiée ou retirée supprime le contexte sélectionné et ses données ; aucune réponse tardive ne les rétablit. Un 401 déconnecte. Aucun SSE broker, credential runner ou accès moteur n'est ajouté.

Les procédures ci-dessous d'association et de pagination des workers et de fédération concernent les profils historiques ci-dessus. Ces sources restent indisponibles dans le profil owned.

## Vue 360° et Mon espace

Lorsque le catalogue du contexte sélectionné accorde et sert `summary_read`, le dashboard ouvre d’abord **Vue 360°** (contexte d’administration) ou **Mon espace** (contexte personnel). La page lit une fois `GET /api/v1/human/summary` et présente ce que le contrôle a calculé en une transaction, avec l’heure d’évaluation et le reçu de lecture :

- **Situation du tenant / de mon espace** : une ligne par domaine. *Workers* (chaque worker enregistré par état durable et le quota qu’il retient ; l’inventaire runtime reste inconnu), *Brokers* (états des baux à l’évaluation), *Ressources* (sessions, agents, sandboxes et projets autorisés), *Décisions* de la période choisie (autorisées, à revoir, refusées ; lorsque le budget de lecture du journal arrête le décompte, la ligne dit depuis quand elle compte), *Quota du tenant* (réservations, pas une consommation mesurée) et *Usage observé* (allocations terminales de la période, méthode 1, `billable=false`). Une ligne que votre contexte ne peut pas lire indique **Non accordé à ce contexte.** ; une ligne indisponible nomme sa raison. Aucune n’est jamais présentée comme zéro.
- **Flotte par broker** : les workers groupés par autorité de lancement et joints au registre brokers par identifiant exact. Un broker dont le bail n’est plus courant conserve le dernier état connu de ses workers avec une note ; les workers d’une autorité non enregistrée restent comptés sous **Autorité non enregistrée**.
- **Points d’attention** : faits dérivés des seuls états durables du contrôle (worker incertain, allocation incohérente, arrêt pour mémoire, broker sans heartbeat, bail périmé, autorisation expirée ou révoquée, horloge incertaine), avec les comptes complets par type et les 50 éléments les plus récents. **Ouvrir le worker** / **Ouvrir le broker** ouvre la vue exacte lorsque votre contexte l’offre. Ce n’est pas un suivi d’incidents.

**Période** ne change que les lignes des décisions et de l’usage. **Relecture automatique** a lieu toutes les 2 minutes par défaut (30 s ou manuel au choix) tant que l’onglet est visible ; chaque lecture laisse un reçu d’audit durable et compte dans les budgets de lecture humaine. Après une lecture en échec, la dernière synthèse reste affichée comme **Données périmées · dernière synthèse conservée** ; une lecture refusée retire toutes les valeurs et vos accès sont relus. Changer, laisser expirer ou révoquer le contexte, ou se déconnecter, retire la page.

## Ouvrir la vue

1. Connectez-vous avec votre compte nominatif et attendez **Choisir un contexte.**
2. Chaque contexte proposé garde son bouton **Ouvrir …** et gagne **Superviser administration · `<tenant>`** ou **Superviser personnel · `<tenant>`** lorsqu'il accorde au moins une source.
3. Le titre est **Supervision du tenant.** pour un contexte d'administration et **Supervision de mon espace.** pour un contexte personnel. L'onglet **Supervision** reste disponible tant que le contexte est sélectionné.

Quitter la vue pour un autre onglet annule ses lectures et retire ses lignes ; ce que vous avez saisi dans les filtres est conservé tant que le même contexte reste sélectionné. Changer de contexte, se déconnecter ou perdre le contexte retire tout, détails ouverts compris.

## Lire l'écran

- **Fraîcheur.** Chaque source (Workers, Brokers, Ressources) affiche son propre état et l'heure de sa dernière lecture réussie : `Lecture en cours…`, `Lecture réussie`, `Données à actualiser` (plus anciennes que deux fois le rythme d'actualisation, ou deux minutes en mode manuel), `Données périmées · dernière lecture conservée` après une actualisation en échec, `Contrôle injoignable`, `Lecture refusée`, `Données retirées · accès à relire`, `Non accordé`. Une fenêtre de plusieurs pages a la fraîcheur de sa page la plus ancienne : elle affiche alors `Pages lues entre … et …` au lieu de `Lu le …`.
- **Couverture.** Le bandeau indique toujours **Couverture partielle** : la page réunit des instantanés de registre indépendants et jamais un inventaire runtime. Une ligne par source dit ce qui a été chargé, si d'autres pages existent, ou pourquoi rien n'est connu. Une lecture en échec, une source non accordée et une lecture vide réussie sont trois phrases différentes.
- **Comptes.** Tout compte est « n affichés sur N chargés ». Aucun total du tenant n'est affiché ; `d’autres pages existent au contrôle` signifie que le registre contient plus de lignes que la fenêtre chargée.
- **État du contrôle et observation runtime.** `Cycle ouvert` regroupe les états non terminaux du contrôle (`reserved` … `stopping`). Ce n'est pas une exécution observée. `Observation inconnue` signifie qu'aucune inspection runtime n'est enregistrée.
- **Bail du broker.** `Bail courant` ne prouve aucun worker en fonctionnement ; un bail périmé, expiré ou révoqué ne prouve ni arrêt ni libération de quota.
- **Associations d'autorité.** Un worker figure sous un broker seulement si l'une de ses deux associations d'autorité (lancement, inspection) désigne exactement ce broker. L'association est historique et peut être postérieure au fait worker : elle ne prouve jamais quel broker a exécuté ou inspecté la session. Les workers dont l'autorité de lancement n'est pas enregistrée, est inconnue, ou a été lue pendant une indisponibilité du registre brokers sont regroupés sous ce statut au lieu d'être masqués.
- **Points d’attention** : faits renvoyés par le registre (`Allocation incohérente`, `Issue incertaine`, `Arrêt mémoire relevé`, `Runtime différent du broker associé`, `Broker associé sans bail courant`). Ils ne portent ni gravité ni diagnostic.

## Rechercher, filtrer et charger plus

La recherche et les filtres portent sur les seules lignes chargées, et la ligne de résultat le dit. La recherche est une correspondance littérale, insensible à la casse, de chaque terme (128 caractères au plus, aucune syntaxe de motif) sur les identifiants exacts et les noms d'état. Filtres : groupe d'état du contrôle, runtime déclaré, broker ou statut d'association, observation runtime, points d'attention ; tri par état puis relevé le plus récent, par relevé le plus récent, ou par identifiant.

La table des brokers a sa propre recherche, **Rechercher dans les brokers chargés**, sur les lignes de brokers de la page lue (identifiant, incarnation, état du bail, autorité déclarée) ; elle ne filtre jamais les workers.

Le registre est trié par identifiant côté contrôle, sans filtre d'état serveur. **Charger 100 lignes de plus** ajoute une page, jusqu'à 500 lignes de workers ; au-delà, utilisez la vue exacte **Workers**. Des workers à cycle ouvert peuvent donc exister hors de la fenêtre chargée : un identifiant absent ici n'est pas absent du tenant.

Lorsque plusieurs pages de workers sont chargées, chaque page est son propre instantané et l'instantané des brokers affiché est celui de la dernière page lue.

## Clavier et annonces

- Chaque action est atteignable avec Tab, Entrée, Espace et Échap. Un détail est un dialogue modal ; Échap ramène à la ligne qui l'a ouvert.
- Une commande qui lance une lecture reste dans l'ordre de tabulation tant qu'une lecture est en vol : elle est annoncée indisponible, ignore une nouvelle activation, et garde le focus pendant la lecture et après elle. Cela vaut pour **Actualiser maintenant**, **Relire depuis les plus récents**, **Charger 100 lignes de plus**, **Charger 100 ressources de plus**, **Charger 100 événements plus anciens**, **Lire les événements de cette session**, **Réessayer** et **Reprendre …**, et pour **Actualiser** dans la barre de destinations tant que l'une des deux vues ou la liste des contextes est ouverte. Une page que vous avez chargée reste visible avec son bouton, prêt pour la suivante.
- Une commande que sa propre lecture fait disparaître rend le focus à ce qui la remplace, jamais au haut de la page : la note de fin d'une liste, ou sa ligne de résultat, lorsqu'il n'y a plus rien à lire ; la ligne de résultat du Journal, ou **Actualiser maintenant** dans Supervision, une fois levée l'erreur à laquelle **Réessayer** appartenait ; le titre de la page pour **Reprendre**. Une nouvelle tentative qui échoue encore garde sa ligne d'erreur et le focus sur **Réessayer**. **Limiter** et **Retirer la limite** placent aussitôt le focus sur la ligne de résultat. **Afficher N lignes de plus** rend des lignes déjà chargées, sans lecture, et continue sur la première d'entre elles. Un budget de lecture arrêté désactive ces commandes et retire **Réessayer** ; dans les vues exactes **Brokers**, **Workers** et **Fédération**, **Actualiser** est désactivé pendant leur propre lecture et le focus passe à la destination courante.
- Chaque vue a une seule région d'annonce, dans son bandeau : une phrase courte lorsque l'état d'une source change (par exemple `Workers, brokers : données périmées, dernière lecture conservée. Ressources : lecture réussie.`), jamais lors d'une actualisation automatique qui ne change rien. Une erreur est annoncée une fois, à son apparition. Les lignes de résultat (`24 workers affichés sur 24 chargés · …`) suivent chaque frappe à l'écran et ne sont pas elles-mêmes des régions d'annonce : la région du bandeau dit leur compte.
- Dans Supervision, le compte d'une liste (`18 workers affichés sur 24 chargés.`) est annoncé une fois, après une pause d'une demi-seconde qui suit une recherche ou un filtre, quel que soit le nombre de frappes ; les listes modifiées pendant la même pause sont dites ensemble. Une lecture qui change un compte le dit à sa fin, dans la même phrase que l'état s'il change aussi : la première lecture, une page que vous avez chargée, une actualisation qui change le nombre de lignes. Une actualisation automatique qui change le contenu de lignes (un worker qui a changé d'état, par exemple) sans changer un état ni un compte n'annonce rien, un changement de tri non plus. Une lecture que vous avez demandée (**Actualiser maintenant**, **Actualiser**, **Réessayer**, une page que vous avez chargée) dit toujours comment elle s'est terminée : lorsqu'elle n'a changé ni état ni compte, la région redit l'état des sources (`Workers, brokers, ressources : lecture réussie.`), ou l'erreur encore affichée lorsqu'elle a échoué de la même façon qu'avant. **Voir ces workers** place le focus sur la ligne de résultat, qui est lue depuis là. Un détail ouvert a sa propre région et annonce en une phrase l'issue de **Lire les événements de cette session**, jamais la liste des événements.
- Dans le Journal, la ligne de résultat change à chaque lot en direct : les événements en direct sont annoncés par leur nombre, au plus toutes les 15 secondes, et le résultat d'une recherche ou d'un filtre après une pause d'une demi-seconde. Une lecture que vous avez demandée (**Charger 100 événements plus anciens**, **Limiter**, **Retirer la limite**, **Relire depuis les plus récents**, **Réessayer**, **Actualiser**) annonce son résultat une fois, lorsqu'elle est terminée et que le direct qu'elle rouvre est établi ou refusé, et seulement si elle a réussi ; quand l'état change avec elle, une seule phrase donne l'état, puis le résultat. Une lecture demandée qui échoue est dite par le nouvel état et par sa ligne d'erreur ; lorsqu'elle échoue de nouveau de la même façon, avec la même ligne, la région redit cette erreur. Un lot en direct n'annonce jamais de résultat.
- Lorsque la phrase à dire est celle que la région porte déjà (le même résultat après la même relecture, par exemple), la région est vidée puis la phrase y est réécrite un dixième de seconde plus tard, une fois, pour être une annonce nouvelle et non un texte inchangé. Une actualisation automatique qui ne change rien et un lot en direct ne déclenchent jamais cela.
- Les lignes inchangées ne sont ni reconstruites ni déplacées par une actualisation ou un lot en direct, y compris lorsqu'une autre ligne a changé et pris une nouvelle place dans le tri : une sélection de texte, le focus et un clic en cours y survivent. Une ligne dont le contenu a changé est reconstruite : le focus revient sur la même commande de cette ligne, une sélection de texte qui s'y trouvait est perdue. Lorsque l'ordre de tri lui-même change, les lignes qui doivent changer de place gardent le focus mais perdent une sélection de texte.
- Un détail ouvert est mis à jour section par section : seules les sections dont le contenu a changé sont remplacées. Une actualisation qui ne change rien d'autre remplace la ligne qui porte l'heure de lecture ou le reçu de lecture, si bien qu'une sélection de texte et le focus restent dans le reste.
- Avec beaucoup de destinations, la barre défile latéralement sans barre de défilement ; le bord qui cache d'autres destinations s'estompe, et une destination qui prend le focus est ramenée entièrement dans la zone visible.
- Non vérifié : lecteurs d'écran et audit WCAG (voir le handoff du lot). Les annonces ci-dessus sont constatées comme des écritures dans la page, avec leur texte, leur région et leur ordre ; qu'un lecteur d'écran donné prononce chacune d'elles, région vidée puis réécrite comprise, n'est pas établi.

## Corréler

| De | Vers | Clé | Ce que cela prouve |
|---|---|---|---|
| Broker | Workers | Identifiant exact du broker dans l'association de lancement ou d'inspection d'un worker | Une association historique d'autorité |
| Worker | Ressource session | `session_id` égal, exactement, à l'identifiant d'une ressource `session` accordée | Une référence de session déclarée ; aucun propriétaire n'est attribué |
| Session | Événements | Lecture serveur filtrée sur cette session | Les événements que le contrôle autorise pour ce contexte |
| Agent ↔ session, agent ↔ sous-agent | — | Non renvoyé par le contrat nominatif | Inconnu |

Dans le détail d'un worker ou d'une session, **Lire les événements de cette session** émet une lecture autorisée (`GET /api/v1/human/events/recent?limit=25&view=metadata&resource_kind=session&resource_id=…`) et liste les 25 événements les plus récents de cette session. Rien n'est lu avant votre demande. **Ouvrir cette session dans le Journal** poursuit dans le Journal, limité à cette session. Si le contrôle répond `404 not_found`, le détail affiche un refus et le reste de la page ne change pas : le contrôle ne distingue pas une ressource absente d'une ressource que ce contexte ne peut pas lire. Le contenu et l'export restent dans les vues **Ressources** et **Événements**.

Depuis le Journal, **Limiter** relit côté serveur le journal de la ressource de cet événement, et **Workers** ouvre Supervision avec la recherche positionnée sur l'identifiant de cette session ; le rythme d'actualisation et vos autres choix y sont conservés.

## Journal et direct

L'onglet **Journal** répond à « que vient-il de se passer » sans parcourir l'historique page par page.

1. Il lit les 100 événements autorisés les plus récents (`order: newest_first`). **Charger 100 événements plus anciens** remonte plus loin, jusqu'à 1 000 événements dans la fenêtre ; au-delà, le résumé demande de relire depuis les plus récents ou de limiter le journal à une ressource.
2. Il ouvre ensuite le flux existant à la position renvoyée avec cette page (`live_cursor`). Le flux commence strictement après la page : ni trou, ni rejeu de l'historique. Les nouveaux événements s'insèrent en tête, sans doublon, et **Direct** indique combien sont arrivés depuis la dernière lecture.
3. La recherche et les filtres de décision, de provenance et de type de ressource portent sur les seuls événements chargés. La limite à une ressource est le seul filtre côté serveur.

| État de **Direct** | Sens | Que faire |
|---|---|---|
| `Connexion au direct…` | Le flux est en cours d'ouverture | attendre |
| `Direct connecté` | Les nouveaux événements autorisés arrivent en tête | — |
| `Direct déconnecté · reconnexion` | La connexion est tombée, ou le contrôle a répondu qu'il est indisponible pour l'instant (`retry` : gateway injoignable depuis le dashboard, capacité momentanément atteinte). Le flux est rouvert depuis la dernière position livrée avec un délai croissant (1 à 30 s). La fenêtre est marquée périmée entre-temps | attendre, ou **Relire depuis les plus récents** |
| `Direct suspendu · onglet masqué` | L'onglet est masqué : le flux est fermé et rien n'est lu | revenir sur l'onglet : le journal est relu depuis les plus récents et un nouveau flux démarre après lui. **Relire depuis les plus récents**, onglet visible, fait de même |
| `Direct refusé` | Le flux a été refusé : trop de flux pour ce compte, budget de lecture arrêté, ou refus (HTTP 403) qui ne porte pas le contrat d'erreur du contrôle, comme peut en répondre un intermédiaire placé devant le dashboard. Un refus n'est jamais relancé de lui-même | suivre le message, puis **Relire depuis les plus récents** |
| `Direct arrêté` | Aucun flux : la dernière lecture a échoué, ou le flux n'a pas pu être rouvert après six tentatives | **Réessayer**, ou **Relire depuis les plus récents** |

Le Journal liste les métadonnées des événements rattachés aux ressources que ce contexte peut lire. Les reçus de lecture et les événements de ressources non accordées n'y figurent jamais. Ce n'est ni l'audit complet ni un inventaire runtime. L'onglet **Événements** garde son rôle : une ressource choisie, avec contenu et export selon vos droits, lue depuis l'événement le plus ancien.

## Expiration, nouveau contexte et reprise

Un contexte dure au plus 300 secondes. Le contrôle ne le renouvelle jamais : il peut en proposer un nouveau.

1. À l'expiration du contexte, toutes les lignes, détails, curseurs et flux de la vue sont retirés, et le dashboard relit votre identité.
2. Si le contrôle propose alors **exactement un** contexte de mêmes tenant, organisation, mode, révision d'appartenance et capacités, la liste des contextes affiche **Reprendre la supervision · …** (ou **Reprendre le journal · …**) et lui donne le focus. Aucune donnée n'est lue avant votre confirmation. Après le clic, la même vue s'ouvre avec les filtres que vous aviez saisis, et les données sont relues. Le contexte proposé a sa propre durée, 300 secondes au plus. Si la liste reste ouverte au-delà, l'offre indique que le contexte proposé `a expiré à son tour` ; **Reprendre** relit alors vos accès et rouvre la vue sur le contexte équivalent que le contrôle propose à ce moment, sans autre clic. Tant que le contrôle propose encore le contexte expiré, le dashboard le dit et répète cette lecture comme à l'étape 3, jusqu'à trois fois pour chaque clic de ce type, quelles que soient les lectures qui l'ont précédé ; la vue se rouvre d'elle-même dès qu'un contexte équivalent est proposé. Une fois ces lectures automatiques arrêtées, ou si l'une d'elles échoue, un nouveau clic est nécessaire. Un onglet masqué ne rouvre rien et ne démarre aucune de ces lectures répétées : si l'onglet est masqué avant que la vue se soit rouverte, la confirmation s'arrête là, même si le contrôle propose entre-temps un contexte valide. Le dashboard indique que la reprise est interrompue (`Reprise interrompue`) et relit vos accès à votre retour ; si le contrôle propose alors un contexte de mêmes droits, la reprise est proposée de nouveau, avec le focus, pour un nouveau clic.
3. Si le contrôle propose encore le contexte expiré, le dashboard relit l'identité au plus trois fois de plus (après 2, 4 et 8 secondes), puis s'arrête ; aucune de ces lectures ne démarre tant que l'onglet est masqué. **Actualiser** la relit une fois quand vous le décidez ; la reprise est proposée dès qu'un nouveau contexte équivalent existe. Cela se produit lorsque l'horloge du navigateur avance sur la mesure que le contrôle fait lui-même de la durée du contexte, par exemple après une suspension de l'hôte.

Tant que l'onglet est masqué, les deux vues ne démarrent aucune lecture : le cycle d'actualisation et le direct sont suspendus, et les lectures d'identité répétées des étapes 2 et 3 ne démarrent pas. Deux choses s'y produisent encore. Une lecture déjà en vol se termine. Et lorsque le contexte est perdu (il a expiré, ou une lecture en vol a été refusée), ses lignes sont retirées aussitôt et votre identité est lue une fois, sauf si le budget de lecture est arrêté, pour que la liste des contextes soit à jour à votre retour ; une reprise proposée ainsi attend votre clic comme toute autre.

Aucune reprise n'est proposée après une révocation ou un changement de droits, après un refus, après une déconnexion, lorsque vous êtes revenu vous-même à la liste des contextes, lorsque le budget de lecture est arrêté, pour un autre compte, ou plus de 15 minutes après la perte du contexte. Par le dashboard, une expiration constatée d'abord par le contrôle se présente comme un contexte inconnu, un refus ou un changement d'accès, et mène aussi au choix explicite. L'intention de reprise contient la vue ouverte, les filtres que vous aviez saisis, le rythme d'actualisation et les identifiants que vous aviez sélectionnés (la ressource à laquelle le Journal était limité, le filtre de broker) ; elle reste en mémoire et n'est jamais stockée.

Avec le SSO, chaque renouvellement de jeton remplace la session du dashboard. La même règle s'applique : la liste des contextes suivante propose la reprise, indique que la session a été renouvelée (`Votre session a été renouvelée`) et non qu'un contexte a expiré, et rien n'est lu avant votre confirmation.

## Refus et erreurs

| Affiché | Cause | Action |
|---|---|---|
| `Lecture refusée`, lignes retirées | `403 permission_denied` sur une lecture de registre ou de ressources | **Relire mes accès**, puis sélectionner un contexte actuellement proposé par le contrôle |
| `Lecture refusée` sur Workers et Brokers, `Données retirées · accès à relire` sur Ressources | `403` sur la lecture composée des deux registres. Le client retire ses lignes dès l'arrivée de ce refus, sans en lire le motif : un contexte révoqué ou expiré ne se distingue pas d'un registre refusé. Toutes les sources et tout détail ouvert de la vue sont donc retirés et vos accès sont relus une fois | rien : un contexte révoqué ou expiré vous ramène à **Choisir un contexte.** ; si le contrôle propose toujours le contexte, les ressources sont relues et seuls les registres restent refusés |
| Retour à **Choisir un contexte.** | Contexte expiré, révoqué ou inconnu du contrôle | Sélectionner un contexte proposé. Après une expiration seulement, confirmer la reprise proposée pour retrouver la même vue et les mêmes filtres ; aucune ligne du contexte précédent n'est conservée |
| Formulaire de connexion | `401`, credential refusé ou expiré | Se reconnecter |
| `Contrôle injoignable` | Panne réseau, délai dépassé, ou dashboard sans accès au gateway | **Réessayer** ; la dernière fenêtre réussie reste, datée et marquée périmée |
| `Données indisponibles` avec le message serveur | Stockage ou audit indisponible (`503`) | **Réessayer** ; si le message demande l'opérateur, le contacter |
| `Capacité temporairement atteinte` | `429 capacity_exhausted` temporaire | Réessayer dans un instant |
| Lectures automatiques arrêtées | `429 audit_read_budget_exhausted` avec `contact_operator` | Solliciter l'opérateur ; puis **Relire après intervention de l’opérateur**, qui lit d'abord les registres, puis les ressources une fois que cette lecture a rouvert le budget |
| `Revalidation des accès impossible` au retour sur l'onglet | L'identité n'a pas pu être relue lorsque l'onglet est redevenu visible | rien : la vue quitte sa suspension et sa propre lecture indique l'état réel, avec **Réessayer** si le contrôle reste injoignable |

Une erreur n'est jamais rendue comme un registre vide : sans lecture réussie, les tables indiquent `Workers inconnus`, pas `Aucun worker`.

## Budget de lecture et rythme d'actualisation

Chaque lecture autorisée laisse un reçu durable, et le proxy du dashboard ajoute un reçu d'identité par lecture. Ces reçus sont comptés dans un budget cumulatif par compte, qu'un nouveau contexte ou un redémarrage ne recrédite pas.

| Action | Reçus |
|---|---|
| Un cycle d'actualisation, deux registres, une page de workers | 3 |
| Chaque page de workers chargée en plus dans un cycle | 3 |
| Ressources (actualisation explicite seulement) | 2 par page |
| Événements d'une session | 2 |
| Journal : chaque page lue (récente ou plus ancienne) | 2 |
| Journal limité à une ressource, première lecture répondue `404` avant que la route ait jamais répondu : une lecture sans filtre d'un seul événement lève l'ambiguïté | 2 |
| Journal : ouverture du flux | 1 |
| Journal : chaque lot en direct non vide | 1 |

La flotte est relue toutes les 30 s tant que l'onglet est visible ; choisissez 2 min ou **Manuel** sous **Rythme d’actualisation**. Les ressources ne sont relues que par **Actualiser maintenant**. L'actualisation automatique s'arrête lorsque l'onglet est masqué, lorsque plus de deux pages de workers sont chargées, lorsqu'une lecture a été refusée et lorsque le budget est épuisé. Un contexte qui n'accorde aucun registre n'a rien à actualiser automatiquement : il n'affiche aucun rythme et le dit (`Aucun registre n’est accordé à ce contexte`). Un contexte dure au plus 300 secondes et n'est jamais renouvelé implicitement, ce qui borne une page laissée sans surveillance.

## Limites

- Aucun inventaire runtime. **Vue 360°** résume en une transaction les registres du tenant, les réservations de quota et les allocations observées ; les vues d’administration des appartenances, de l’usage et de la configuration et les changements délégués d’appartenance et de partage sont décrits dans [ui.md](../../ui.md#vues-utilisateur-et-administration-unifiée--cible-à-livrer) avec leur propre état. Le profil de sources possédées ne sert pas encore la synthèse.
- Un tenant par contexte. Aucune vue inter-tenants.
- Ni filtre ni tri serveur du registre des workers ; au plus 500 lignes de workers, 100 brokers et 300 ressources dans cette vue.
- Ni hiérarchie d'agents ni vue personnelle des workers.
- La fenêtre du Journal contient au plus 1 000 événements ; une page coupée par cette borne est signalée comme une fenêtre atteinte, jamais comme le début de la sélection. Les vues **Ressources** et **Événements** ouvrent toujours leur flux depuis le début de l'historique autorisé ; seul le Journal démarre après la page qu'il a lue.
- Une vue laissée ouverte ne reste pas indéfiniment en direct : le contexte s'achève après 300 secondes et la reprise est un clic délibéré.
- Interface en français uniquement.

## Contrat de `GET /api/v1/human/events/recent`

Lecture additive du gateway, admise aussi par le proxy du dashboard à ce chemin exact. En-têtes : `Authorization: Bearer …` et un seul `X-AiGuard-Context`. Paramètres fermés :

| Paramètre | Valeur |
|---|---|
| `limit` | 1 à 100, défaut 25 |
| `view` | `metadata` (défaut) ou `content` |
| `resource_kind` et `resource_id` | les deux ou aucun ; limite à une ressource autorisée |
| `cursor` | `next_cursor` opaque de la page précédente de la même sélection |

```json
{
  "version": 1,
  "context_id": "<context>",
  "authorization_version": "<64 hex>",
  "view": "metadata",
  "order": "newest_first",
  "coverage": "authorized_selection",
  "items": [],
  "has_more": false,
  "next_cursor": null,
  "live_cursor": "<opaque>",
  "read_receipt": { "request_id": "<uuid>", "audit_event_id": "<uuid>", "received_at": "<RFC 3339>" }
}
```

- `items` sont les mêmes projections que `/human/events`, du plus récent au plus ancien. `has_more` vaut `next_cursor != null` et signale des événements autorisés plus anciens.
- `live_cursor` n'existe que sur la première page (requête sans `cursor`). `/human/stream` l'accepte avec exactement les mêmes `limit`, `view` et ressource, et ne livre alors que les événements enregistrés après cette page. Aucune position d'audit n'est exposée. Toute autre limite ou sélection répond `400 invalid_cursor`.
- L'autorisation, le masquage et l'enveloppe de 256 Kio sont ceux de `/human/events`. Le budget de traversée de 1 000 pages et 16 Mio l'est aussi : il vaut **par traversée**, c'est-à-dire par chaîne de curseurs, pendant la vie du contexte. Une requête sans `cursor` ouvre une nouvelle traversée, et `live_cursor` une traversée de flux neuve. Ce n'est pas un plafond par contexte. La borne cumulée est le budget de reçus de lecture humaine du compte (voir [Budget de lecture et rythme d'actualisation](#budget-de-lecture-et-rythme-dactualisation)), avec la fréquence et les places de lecture du tenant. Une traversée qui dépasse son budget répond `429 capacity_exhausted` avec `select_context`. Le curseur d'une route est refusé par l'autre.
- Chaque page enregistre un reçu durable `control.identity.access` avec l'action existante `events`, dans la même transaction du store que son autorisation et avant toute restitution ; il compte dans les budgets de lecture humains et ne distingue pas cette lecture de la lecture avant. Un reçu peut subsister lorsqu'un refus postérieur à cette transaction (révocation, expiration, budget de traversée) empêche la publication de la page : un reçu ne prouve pas qu'une page a été remise.
- Après une mutation d'identité du tenant, la révocation du credential ou une expiration, la route, un `next_cursor` déjà émis et un `live_cursor` déjà émis sont refusés sans aucun élément (`403 context_revoked`, `context_expired` ou `context_invalid` avec `select_context` ; `401` avec `authenticate`), et un flux qui était ouvert se termine par une erreur.
- Erreurs : `400 invalid_request` ou `invalid_cursor` ; `401` ; `403` pour un refus de contexte ou `permission_denied` ; `404 not_found` lorsque la ressource filtrée n'est pas accordée ou n'existe pas ; `429 capacity_exhausted` ou `audit_read_budget_exhausted` ; `503`.

La route est documentée dans [docs/api.md](../../api.md). Son intégration sur `2cc29ee` et les vérifications restant à exécuter sont suivies séparément du lot historique dans les [preuves d’intégration](../../engineering/evidence/2026-11-10/unified-control-ui-integration/README.md).

## Compatibilité de versions

Les indications et le tableau de cette section concernent la route `events/recent`. Le profil brokers owned exige en plus son annonce explicite par le gateway et un dashboard correspondant, sans repli historique.

Aucune migration de schéma, aucun nouveau type d'audit, aucun changement de configuration et aucune nouvelle dépendance. Mettre à jour le gateway et le dashboard ensemble.

| Gateway | Binaire dashboard avec son `front/dist` | Résultat |
|---|---|---|
| Avec la route | Avec la route | Les fonctions events/recent décrites ci-dessus |
| Sans la route | Avec la route | Le Journal affiche **Lecture récente indisponible sur ce contrôle** et n'ouvre aucun flux ; les événements de session dans Supervision se replient sur la lecture avant, du plus ancien au plus récent, et le disent ; toutes les autres vues fonctionnent |
| Avec la route | Plus ancien | La route est simplement inutilisée |

Un gateway sans la route lui répond `404 not_found`, exactement comme pour une ressource non accordée. Sans filtre de ressource, cette réponse ne peut signifier que l'absence de la route. Avec un filtre, elle reste ambiguë tant que la route n'a pas répondu une fois dans la session ; le client ne la transforme donc jamais de lui-même en sélection refusée : les événements de session de Supervision prennent la lecture avant comme autorité, et le Journal lève l'ambiguïté par une lecture sans filtre (réponse : la ressource n'est pas accordée, `Ressource non accordée` ; `404` à nouveau : la route est absente). Un `front/dist` plus récent derrière un binaire dashboard plus ancien n'est pas une combinaison prise en charge : les deux sont livrés ensemble.

## Vérifier en contributeur

Toutes les commandes s'exécutent à la racine du dépôt avec la toolchain épinglée ; aucune ne démarre de runtime de conteneurs ni de modèle. Les scénarios navigateur demandent en plus le paquet `playwright` (1.64.x) et son Chromium, qui ne sont pas des dépendances du paquet frontend : rendez-les résolubles par `NODE_PATH`.

```sh
cargo test --locked --test supervision_stack_api --test human_recent_events
cd front && npm ci && npm test && npm run build
```

Un test de `human_recent_events` attend la durée réelle de 300 secondes d'un contexte et est ignoré par défaut : lancez-le avec `cargo test --locked --test human_recent_events -- --ignored`. Sans cela, il n'est pas exécuté.

`examples/supervision_stack.rs` est un harnais de test, pas un parcours d'essai : il sert les vrais routeurs gateway et dashboard sur le loopback, au-dessus de données synthétiques construites par de vraies mutations du contrôle, pour les scénarios navigateur de `front/tests/browser_supervision.mjs`.

```sh
cargo build --locked --example supervision_stack
cd front && AI_GUARD_SUPERVISION_STACK=../target/debug/examples/supervision_stack \
  AI_GUARD_SUPERVISION_SCENARIO=fleet node tests/browser_supervision.mjs
```

Scénarios : `fleet`, `partial`, `correlation`, `rights`, `revoked`, `isolation`, `expiry`, `resume`, `ttl` (attend une expiration réelle de 300 secondes), `journal`, `keyboard`, `focus`, `regions`, `details`, `states`, `load`. Le script sort avec le code 2 et `BLOCKED` lorsque Playwright, le binaire ou `front/dist` manque, ou lorsque la pile ne démarre pas à temps ; un tel scénario est `NOT_RUN`. Les 120 lignes de registre du jeu de données sont de vraies admissions sans aucun runtime : elles exercent l'interface avec des données denses et ne qualifient aucune capacité de workers.
