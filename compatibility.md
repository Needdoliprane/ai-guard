# Compatibilité des agents et intégration Jev

État de la recherche : **29 octobre 2026**. Sources primaires : [registre des sources](docs/sources.md). Les documentations consultées sont évolutives ; elles ne prouvent pas que toutes les versions déjà installées possèdent les fonctionnalités décrites.

## 1. Ce que signifie « compatible »

Deux axes doivent rester séparés :

1. **Capacités du produit tiers** : interface documentée que l'on peut intégrer.
2. **Couverture réellement active dans Agent Sandbox** : composant installé, version reconnue, événement reçu et vérification effectuée.

Un lancement de CLI dans un conteneur ne suffit pas à qualifier son intégration de Native. Un parseur qui sait lire un exemple JSON ne prouve pas non plus que la version installée émettra ce format pour chaque outil.

| Niveau | Condition pour l'afficher | Limite à afficher |
|---|---|---|
| **Native** | Un adaptateur activé exploite une interface explicite du fournisseur, reconnue pour cette version. | Les événements viennent du fournisseur ; ils ne constituent pas automatiquement une observation indépendante du système d'exploitation. |
| **Observed** | Le broker ou le runtime observe le processus, ses sorties, son résultat ou les changements de son workspace. | La visibilité concerne les opérations effectivement observées ; l'activité interne peut manquer. |
| **Generic** | Le conteneur et son cycle de vie sont visibles, sans instrumentation spécifique de l'agent. | Pas de reconstruction des outils, des sous-agents, des tokens ou des décisions internes. |

La provenance de **chaque événement** reste distincte du niveau du connecteur : `observed`, `declared` ou `inferred`. Par exemple, un `PostToolUse` reçu d'un CLI instrumenté est un événement natif mais déclaré par ce CLI. Le code de sortie d'un processus récolté par le broker est observé par celui-ci. Une estimation de coût ne devient jamais une facture réelle.

## 2. Matrice des mécanismes confirmés

| Agent | Interfaces documentées | Niveau atteignable | Sous-agents | Couverture à exiger avant validation |
|---|---|---|---|---|
| **OpenAI Codex CLI** | `codex exec --json` produit du JSONL. Hooks de cycle de vie et d'outils, dont `PreToolUse`, `PostToolUse`, `SubagentStart`, `SubagentStop`. | Native pour un adaptateur de version validé ; Observed pour un lancement supervisé. | Les hooks peuvent fournir des identifiants explicites. Ne pas inventer d'arête à partir du nom d'une tâche. | Échantillons réels, confiance des hooks, outils couverts, gestion des processus persistants. [C1][C2] |
| **Anthropic Claude Code** | CLI non interactif `claude -p`, sorties structurées ; hooks avant/après outils et cycle de vie des sous-agents. | Native avec hooks/SDK correctement intégrés ; Observed avec wrapper. | `SubagentStart`/`SubagentStop`, `agent_id` et type selon le contrat du hook. | Refus avant outil, échec du hook, timeout, arrêt anormal ; ne pas activer `--bare` sans charger explicitement ce qui est nécessaire. [A1][A2] |
| **Google Gemini CLI** | Mode headless, sorties structurées ; hooks `BeforeTool`/`AfterTool` ; télémétrie OpenTelemetry. | Native possible ; Observed en l'absence d'adaptateur actif. | Ne créer une relation parent/enfant que depuis un identifiant explicitement fourni par la version utilisée. | Décisions avant outil, erreurs du hook et format des traces ; éviter de journaliser les prompts par défaut dans la configuration livrée. [G1][G2][G3] |
| **OpenCode** | Plugins avec hooks d'outils et événements ; serveur HTTP et événements ; API V2 distincte de V1. | Native possible avec adaptateur V1 ou V2 explicite. | Sessions/agents selon l'API versionnée ; pas de déduction à partir d'un titre de session. | Épingler la branche d'API et tester les événements réellement reçus. [O1][O2][O3] |
| **Aider** | Exécution `--message`, fichiers d'historique, journal local d'analytics JSONL. | Observed recommandé dans cette livraison ; Native partiel envisageable pour une version validée. | Aucune arborescence de sous-agents universelle n'est établie par les pages consultées. | Processus, modifications Git et sortie finale ; un fichier d'historique n'est pas une preuve exhaustive d'exécution. [D1][D2][D3] |
| **Continue CLI** | `cn -p`, `--format json`, permissions par outil. | Observed recommandé ; résultats structurés intégrables. | Non confirmé par les interfaces consultées. | Ne pas assimiler un résultat JSON à un flux complet de commandes ; tester les modes de permissions choisis. [N1][N2] |
| **Agent personnalisé / framework** | Contrat d'événements du projet et API du broker, sous contrôle du développeur de l'adaptateur. | Native si le contrat est implémenté et testé ; sinon Observed/Generic. | Seulement à partir d'IDs explicites. | Identité, séquence des événements, refus effectif, reprise et volume. |

**État de la livraison :** des parseurs Rust Codex JSONL, Claude stream-json et télémétrie générique v1 sont implémentés et testés sur des fixtures synthétiques fondées sur les schémas officiels. Ils restent `declared`/`observed` : voir [adaptateurs livrés](docs/adapters.md). Gemini, OpenCode, Aider et Continue n'ont pas de parseur natif livré. Les hooks fournisseurs décrits ici sont des possibilités d'intégration documentées, pas des hooks activés par cette livraison.

**État de validation fournisseur :** aucun appel payant aux modèles ni authentification sur un compte fournisseur n'est attesté par cette recherche. Les tests du dépôt distinguent les fixtures de formats, les tests du broker et une véritable exécution Codex/Claude/Gemini. L'absence d'une métrique se représente par une valeur inconnue, jamais par zéro.

## 3. Particularités qui influencent l'architecture

### Codex

La documentation indique que les hooks couvrent notamment le shell, `exec_command`, `apply_patch` et MCP. Elle signale aussi des exceptions : les outils hébergés tels que la recherche Web ne suivent pas ce chemin, et `write_stdin` ne redéclenche pas `PreToolUse` pour une session déjà ouverte. Les formats de transcript ne sont pas une interface stable. Les hooks non gérés exigent une confiance explicite dans leur définition. [C1]

Conséquence de conception : ne jamais annoncer « toute exécution Codex passe par Jev » sur la seule base d'un hook. Une session shell interactive autorisée une fois peut recevoir d'autres entrées. La protection du système repose sur les restrictions du conteneur et sur le passage imposé par le broker pour les opérations qu'il contrôle.

### Claude Code

La référence récente documente `onFailure: "block"` pour les hooks `command`/`http` à partir de **v2.1.295**. Sans configuration adaptée, une erreur ou un timeout peut laisser l'action continuer ; le code de sortie `2` a une sémantique bloquante spécifique. `SubagentStart` observe un démarrage et ne constitue pas lui-même une interdiction de créer ce sous-agent. [A1]

Le mode `--bare` saute l'auto-découverte de plusieurs intégrations, dont les hooks. Un profil qui en dépend doit donc vérifier son chargement réel, pas seulement la présence d'un fichier sur disque. [A2]

### Gemini, OpenCode, Aider et Continue

Gemini expose un refus `BeforeTool`; un refus `AfterTool` agit après l'exécution et ne peut annuler ses effets. Sa télémétrie peut contenir des données de prompt et des attributs d'identité : les options de collecte doivent être explicites. [G1][G2]

OpenCode V2 modifie le SDK et les noms de permissions de V1. Un adaptateur ne doit pas mélanger des exemples de ces deux versions. [O3][O4]

L'API Python de scripting d'Aider est déclarée non officiellement prise en charge par sa documentation ; une intégration stable privilégie donc son CLI et un wrapper. [D1]

Continue exclut en mode headless les outils qui demanderaient normalement confirmation. Les modes globaux de permissions peuvent remplacer les règles fines ; enregistrer le mode actif est nécessaire pour expliquer ce qui a été autorisé. [N1][N2]

## 4. Intégration Jev : confirmé, choisi, à confirmer

### Confirmé par les sources officielles

Jev reçoit un état et des questions typées, puis renvoie des réponses structurées. Ce n'est pas un moteur qui exécute des commandes ou remplace le LLM d'un agent de développement. [J1]

L'API officielle documente :

- `GET https://api.typesafe.ai/v1/models` pour les modèles accessibles au compte ;
- `POST https://api.typesafe.ai/v1/systemone` pour les décisions ;
- authentification `Authorization: Bearer …` ;
- `state`, `model`, `questions` dans la requête ;
- `answers`, `model` et `usage` dans la réponse. [J2]

Pour une question `choice`, le résultat comporte notamment `choice`, `confidence` et `probabilities`. La question est retrouvée sous le même identifiant dans `answers`. [J3]

### Décisions d'architecture du projet

Le service Jev est un **conseiller externe du moteur de politiques**. Le broker applique le résultat. Un refus déterministe n'est jamais transformé en autorisation par Jev. Un timeout, une réponse invalide ou une confiance insuffisante ne doit pas autoriser silencieusement une action protégée. Le résultat brut ne doit pas être interprété comme une certification de sûreté.

Le modèle choisi doit être configuré et enregistré dans l'audit. L'alias `jev-latest` apparaît dans les exemples officiels mais peut évoluer ; un déploiement reproductible doit utiliser un modèle disponible pour le compte et documenter la version effectivement renvoyée.

Le proxy éventuel appartient au projet : il termine une requête de décision, borne la taille, masque les secrets et applique un délai. **Ce proxy n'est pas une image officielle de Jev et n'auto-héberge pas ses poids.** La clé TypeSafe reste côté services de confiance, hors du frontend et hors des conteneurs agents.

### À confirmer auprès de TypeSafe

Les pages consultées ne fournissent pas de procédure publique permettant d'installer les poids Jev dans cette stack. La page Enterprise pose la question VPC/on-premises, mais le contenu récupéré ne permet pas de confirmer une offre, ses modalités ni un artefact installable. **Je ne peux pas confirmer un Jev auto-hébergeable.** [J4]

La rétention zéro est annoncée comme une option Enterprise à convenir pour le compte. L'absence d'entraînement sur les entrées ne signifie pas automatiquement absence de traitement ou de rétention. [J4][J5]

En mode hors ligne, utiliser le moteur déterministe local. Si un profil exige explicitement Jev, l'indisponibilité doit être visible et le refus doit rester le comportement de repli.

## 5. Critères pour promouvoir une intégration en Native validée

1. Enregistrer la version du CLI, de l'adaptateur et l'image utilisée.
2. Capturer un événement réel de démarrage, un outil réussi, un outil échoué et une fin de session.
3. Tester un refus avant outil et vérifier l'absence de l'effet demandé dans le workspace.
4. Tester l'indisponibilité du service de décision, le timeout et une réponse JSON incorrecte.
5. Si le fournisseur expose des sous-agents, vérifier les identifiants et relations à partir de ses événements réels.
6. Distinguer dans l'IHM les événements fournisseur et les observations du broker/runtime.
7. Afficher les trous de couverture et les métriques inconnues.
8. Conserver des fixtures expurgées de secrets pour chaque version certifiée par les tests du projet.

Ces critères décrivent une procédure de qualification. Ils ne déclarent pas que les tests fournisseurs ont déjà été exécutés dans l'environnement de livraison.
