# Modèle de données et sémantique des observations

## Objets

| Objet | Identité | Relations | Source de vérité |
|---|---|---|---|
| Projet | Hash du chemin canonique dans cette version | Plusieurs sessions | Configuration du broker ; pas de serveur de projets externe |
| Session | UUID du broker | Un acteur observé, un volume et plusieurs helpers séquentiels | Enregistrement local privé de session + audit |
| Sandbox | ID lié à la session | Image, runtime, réseau, volume et commandes | Paramètres du broker et états inspectés du runtime |
| Agent observé | ID attribué par le broker à son invocation | Projet, session et sandbox | Lifecycle du conteneur de la commande de premier niveau |
| Agent déclaré | Hash d'un espace de noms complet + ID fournisseur explicite | Parent déclaré connu dans le même contexte | Télémétrie de l'agent ; ne prouve pas un processus indépendant |
| Événement | UUID unique + séquence locale | Session, agent, corrélation et trace | Journal SQLite après validation et masquage |
| Décision | Demande identifiée et hash de son contenu | Invocation exacte + événement de décision | Politique de confiance et Jev optionnel |

Un « agent observé » représente l'acteur de la session lancé par le broker, y compris une commande générique de test. Cette fiche ne constitue pas une reconnaissance automatique du framework IA ou d'un modèle : provider et modèle restent explicites ou inconnus.

## Fiche agent

`id`, `parent_id`, `project_id`, `sandbox_id`, `session_id`, `provider`, `name`, `status`, `model`, `task`, `started_at`, `last_activity`, `compatibility`, `provenance` et `field_provenance`. Un champ absent dans la source reste `null` ; les valeurs ne sont pas déduites d'une marque, d'un nom de processus ou du texte d'une réponse.

Les nœuds déclarés comportent également `context_agent_id`, `provider_agent_id`, `provider_parent_agent_id`, `reported_state`, `parent_unresolved` et `hierarchy_warning`. L'identité d'affichage inclut session, sandbox, acteur de contexte et source. Deux agents portant le même nom dans deux sessions ne deviennent donc pas le même agent. Un parent absent ne crée aucune fiche fictive ; un cycle est signalé et son lien est retiré.

`field_provenance` indique la provenance des métadonnées individuellement. Un processus observé peut annoncer un modèle ; le modèle reste alors déclaré. Une télémétrie ne peut pas promouvoir la compatibilité en Native ou remplacer une identité du runner. Les compteurs d'acteurs observés et déclarés restent séparés.

## Fiche sandbox

`id`, `engine`, `image`, `profile`, `status`, `project_id`, `session_id`, `network`, `started_at`, `resources`, `mounts` et provenance des champs. Les limites fixées dans la configuration ne sont pas confondues avec des mesures. Tant qu'aucun capteur ne les produit, CPU/RAM/réseau/disque sont inconnus.

La copie de travail persiste après la suppression des conteneurs éphémères. Une session terminée peut donc encore avoir un volume exportable, sans avoir de processus actif. L'IHM n'assimile pas présence du volume et activité d'un agent.

## Fiche événement

Le type Rust `Event` et `schemas/event.schema.json` décrivent le contrat d'ingestion. `StoredEvent` ajoute `seq`, `received_at`, `prev_hash` et `hash`. Les timestamps sont normalisés en UTC avant calcul de hash ; le temps de réception du collecteur est conservé séparément du temps de la source. Une horloge valide n'est pas pour autant attestée par une autorité de temps externe.

Les commandes passent par des états distincts : demandée, décision obtenue, soumise au runtime, démarrage observé, résultat observé. Un refus s'arrête avant soumission. Une sortie `tool.result` du provider appartient à la catégorie déclarée ; elle ne remplace jamais `command.completed` relevé par le runtime.

`correlation_id` associe les étapes d'une même action et `trace_id` fournit une clé de regroupement. Ces identifiants ne signifient pas qu'un export OpenTelemetry est déjà actif. Les informations réseau ne contiennent pas de capture de paquets ; le mode de réseau est observé/configuré, les destinations détaillées ne sont pas collectées ici.

## Fichiers et Git

Le manifeste d'import/export associe chaque chemin relatif à sa taille et son SHA-256. Les changements créés/modifiés/supprimés sont comparés à l'échelle de la copie de session. Une modification partagée entre plusieurs processus ne reçoit pas une attribution individuelle inventée.

`.git` n'est pas copié dans le profil livré : historiques, hooks et configuration sensibles ne sont pas transmis implicitement. Ce choix signifie que commits et diffs Git ligne par ligne ne sont pas suivis automatiquement. Les fichiers et leurs empreintes sont disponibles ; les diffs Git et la visibilité sur les commits demandés restent des écarts à compléter sans monter l'original ni inventer d'attribution. Une éventuelle application de changements vers l'original exige une décision et une autorisation explicites ; aucun export ne réécrit automatiquement ce dernier.

## Persistance

La table `events` contient le payload canonique masqué, les hashes et des colonnes indexées pour filtrage. Le contrôle d'intégrité vérifie aussi la cohérence de ces projections avec le payload. La table `quota` compte les octets de payloads. L'accès applicatif est append-only : aucune route de modification/suppression de traces n'est exposée.

Les vues agents/sandboxes sont reconstruites à partir du journal. Leurs règles de projection sont donc testables et révisables sans modifier les événements sources. L'archivage automatique, les signatures et l'ancrage externe ne sont pas actifs dans cette version.
