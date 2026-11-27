# Création de session et intervention — contrat produit obligatoire

Accord du mainteneur du 15 novembre 2026. **Cible à implémenter et qualifier ; aucune disponibilité annoncée.** [English version](SESSION_CONTROL.md). Ce contrat complète [identité et SSO](../sso.md), [interface](../ui.md) et le [contrat d'architecture](ARCHITECTURE_CONTRACT.md).

## Un parcours utilisateur cohérent

Le dashboard authentifié doit permettre à un utilisateur autorisé de créer une session IA, puis d'envoyer une courte consigne à cette même session. Identité locale nominative et OIDC facultatif suivent les mêmes règles d'autorisation serveur et d'attribution. Les deux capacités forment un seul lot, une branche et un worktree. Codex possède composition, tests avec Luna, intégration, push et fermeture ; Claude peut réaliser le code délégué et les guides EN/FR.

La création choisit une cible projet/source opaque configurée par le contrôle de confiance, un agent/mode d'authentification permis et une tâche initiale. Le serveur résout et présente fournisseur, abonnement/API/interne, modèle, permissions, ressources et couverture réseau effectifs. Le navigateur ne fournit aucun chemin hôte libre, socket moteur, image, argv, environnement ou credential arbitraire. Un abonnement sélectionné ne devient jamais silencieusement un appel API payant.

La préparation reste retenue : elle ne démarre pas l'agent et ne consomme pas de claim de démarrage. Le lancement engage l'intention préparée exacte après autorisation actuelle, quota, politique et audit durable. Un changement de source/configuration périme la préparation. Une réponse perdue se réconcilie avec la même opération, pas un second Submit. Acceptation, ordre de start et running observé par le runtime sont des faits distincts.

## Autorité et attribution

L'identité authentifiée détermine le tenant. Exiger LaunchSession et SendSessionMessage explicites et distincts, ainsi que les ACL de ressource. Lecture/contenu/visibilité administrative, appartenance au tenant et ApproveLaunch ne donnent implicitement aucun de ces droits. Révalider membership, plafond OIDC, révocation et périmètre juste avant l'effet. Le serveur crée réellement le grant nominatif de session via le producteur vérifié existant ; owner/principal dans le JSON du navigateur ne prouvent jamais une identité.

Distinguer initiateur, acteur broker effectif et causalité. Une consigne normale n'approuve aucune action, ne prolonge pas de lease, ne change pas de politique, ne lève pas un refus et ne contourne pas une review obligatoire. Aucun credential runner dans le worker ou le frontend, et aucun jeton SSO humain dans le worker. Le navigateur conserve seulement son bearer humain en mémoire selon le contrat SSO. Les profils de lecture possédés et leurs contrats fermés de routes/formats restent fermés ; profils de mutation, provisionnement et migrations doivent être explicites, sauvegardés et coordonnés avec l'intégrateur.

### Résoudre le plan Legacy confirmé

Sur une source SQLite Legacy avec la préparation de session activée, le broker lié à la confirmation lit le plan sur le transport privé de contrôle, via `GET /control/api/v1/control/session-confirmations/{binding_ref}`. Le gateway revérifie l’identité workload du broker, son grant, la révision de la cible et le droit de lancement courant de l’initiateur. La réponse contient les identifiants d’opération et de session, les références de cible et de profil, le namespace de contrôle, l’empreinte attendue du plan avec l’identifiant de session assigné et l’échéance de l’initiateur.

La réponse prépare la comparaison côté broker. L’admission résout la même confirmation durable et revérifie l’autorisation dans la transaction qui inscrit le grant, la réservation et l’audit. Les anciennes confirmations V1 permettent à l’initiateur autorisé de rejouer le reçu d’origine.

Le même préfixe privé expose `GET …/{binding_ref}/launch-spec` pour le LaunchSpec exact et `POST …/{binding_ref}/initial-decision` avec un corps vide pour la décision Initial. Le gateway dérive la demande du lancement confirmé et garde le reçu sous l’identité broker. Une nouvelle décision renvoie HTTP 201, le rejeu du reçu renvoie HTTP 200. Cette étape consigne aussi les décisions de revue et de refus. L’admission qui réserve le quota relève d’une requête distincte. Le raccord au Writer et au parcours applicatif reste ouvert.

## Un vrai canal de conversation

[Étude des protocoles et oracles de recette](evidence/2026-11-25/provider-conversation.fr.md).

Créer au lancement un binding de conversation retenu, lié à tenant/source, session, broker/incarnation, worker/tentative, instance runtime, epoch d'entrée, fournisseur/protocole et expiration. Une association de log provider-session déclarée ou un UUID libre ne suffit pas. Un redémarrage/changement de tentative/propriétaire ne redirige jamais silencieusement un message.

Utiliser les protocoles bidirectionnels documentés Codex/Claude dans un mode conversation résolu séparément. Les anciennes sessions batch et TTY restent explicitement incompatibles avec l'entrée, sauf preuve d'un vrai canal retenu. Ni injection de terminal ni commande exécutée pour simuler une entrée conversation. Conserver les quatre combinaisons fournisseur/authentification requises et les qualifier séparément.

Conserver le réseau None d'execution17 et la [direction d'egress contrôlé Rust approuvée](EGRESS_CONTROL.md). Un adaptateur conversation n'ouvre pas Bridge, ne relâche pas un ancien preset et ne revient pas silencieusement au legacy. Réutiliser les vraies capacités d'egress/identité/fencing du broker ; les prérequis manquants restent explicites sans réduire la cible à un formulaire définitivement désactivé.

## Durabilité, bornes et statuts fidèles

Utiliser des clés d'idempotence stables. Même opération et contenu canonique : résultat retenu seulement si l'appelant reste autorisé ; contenu différent : conflit. Queue, attribution nominative et audit obligatoire sont durables avant le start ou le premier octet d'une consigne. Audit indisponible : aucun effet. Préserver isolation tenant, limites session/principal/tenant, rétention et échéances sans consommer les réserves nécessaires à l'arrêt des workers.

Les corps sont des données métier du tenant protégées par les droits de contenu ; l'audit trace métadonnées et reçus sans dupliquer le texte. Appliquer le masquage existant avant persistance/affichage sans changer silencieusement l'instruction. Si la politique de masquage approuvée ne permet pas d'en préserver le sens, refuser avec un motif fixe avant enqueue. Aucun texte brut dans URL, argv, logs techniques ou erreurs.

En attente signifie enregistré durablement ; transmis signifie frame complet écrit sur le canal exact possédé ; acquitté exige une réponse officielle exactement corrélée. Aucun de ces états ne prouve que la consigne a été exécutée. Distinguer incompatible, refusé, expiré, déconnecté et livraison inconnue. Après émission possible sans acquittement réconciliable, conserver l'incertitude et ne pas renvoyer automatiquement. Stop, annulation, révocation et échéance initiale restent prioritaires ; une consigne ne redémarre pas le worker et ne réinitialise pas quota/durée.

## Interface et preuves de fin

Livrer Nouvelle session et Envoyer une consigne dans les vraies vues nominatives, pilotées par les capacités Rust. Conserver direction éditoriale OVPT, deux thèmes, clavier/focus, adaptation écran, destination et provenance explicites. Empêcher doubles activations et réponses tardives repeuplant un contexte révoqué. Annuler fetch ne supprime pas une opération déjà admise côté serveur. Les lecteurs restent en lecture seule.

Livrer implémentation Rust/TypeScript utilisable, API typées, configurations, migrations et guides opérateur EN/FR alignés. Vérifier vrais Store/API/broker et stdio contrôlé, autorisations entre tenants et au sein d'un tenant, idempotence, audit plein, réponses perdues, courses stop/révocation et purge du contexte. Qualifier ensuite runtimes réels et parcours fournisseur/authentification avec des appels utiles, bornés et économes. Fixtures et revues source ne qualifient ni réception CLI, ni workers, OIDC ou production. Suivre prérequis ouverts et preuves dans les registres d'ingénierie existants.
