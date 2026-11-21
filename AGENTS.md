# ai-guard — consignes de développement

## Mandat

Construire et maintenir un **produit open source destiné à la production**, local first, avec une déclinaison cloud utilisant majoritairement le même code. Une preuve de concept, une démonstration et un MVP ne constituent pas le résultat demandé. Le code ouvert et un service cloud facturé à l'usage sont compatibles ; aucune exigence de sûreté, de tests, d'exploitation ou de maintenance n'est réduite.

**La cible production est une exigence ; la qualification est un résultat à prouver.** Le code existant est une base à compléter et à qualifier. Ne jamais écrire « production-ready » parce que le projet compile, que les fixtures passent ou que la documentation a été renommée.

L'architecture ci-dessous est validée par le mainteneur. Travailler dans cette architecture ; soumettre un changement structurel motivé avant de le réaliser.

Les consignes récentes du mainteneur priment sur les anciens plans. Les documents ont des fonctions distinctes :

| Référence | Autorité |
|---|---|
| Ce fichier et [PROMPT_PRODUCTION.md](PROMPT_PRODUCTION.md) | Mandat, invariants et méthode de travail |
| [Contrat d'architecture](docs/engineering/ARCHITECTURE_CONTRACT.md) et [règles de code](docs/engineering/CODING_RULES.md) | Frontières techniques et règles d'implémentation |
| [Identité, SSO et configuration](docs/sso.md), [interface](docs/ui.md) et contrats métier concernés | Exigences détaillées à conserver lors des implémentations et revues |
| [Plan](docs/engineering/IMPLEMENTATION_PLAN.md) et [décisions](docs/engineering/DECISIONS.md) | Lots, dépendances et décisions datées ; ils ne prouvent pas une livraison |
| [État constaté](docs/engineering/CURRENT_STATE.md), [validation](docs/engineering/VALIDATION.md) et [gates](docs/engineering/PRODUCTION_READINESS.md) | Avancement et preuves, limités à leurs sources, dates et périmètres |
| [Registre des travaux](docs/engineering/WORK_TRACKING.md) | Travaux en cours et à venir, décisions, handoffs et clôture des ressources |

Un contrat cible ne décrit pas une fonction disponible. Une preuve historique n'est pas transférée au checkout courant ou à un autre worktree.

## Qualité du produit attendu

Le produit doit rendre un service complet, compréhensible et agréable à utiliser, sans imposer une infrastructure de supervision externe. Une interface légère peut être aboutie ; une longue liste de fonctionnalités ou un nombre de tests ne remplacent pas la qualité d'un parcours réel.

- Livrer les parcours de bout en bout : préparer l'installation, diagnostiquer ses prérequis, lancer une session autorisée, comprendre un refus, suivre les événements, examiner les changements, exporter dans une destination neuve, arrêter et reprendre après une panne.
- Traiter les états chargement, vide, partiel, périmé, déconnecté, non authentifié, interdit et erreur. Montrer l'action de reprise disponible et la portée des données ; ne jamais présenter une erreur comme une absence d'activité.
- Utiliser les données réelles et leur provenance. Les fixtures restent dans les tests. Ni compteur inventé, ni contrôle visuel sans effet, ni écran de démonstration ne constituent une fonctionnalité livrée.
- Soigner les libellés, les motifs de décision, la navigation clavier, le focus, les contrastes, les tailles d'écran, les thèmes et la lisibilité sous charge. Les restrictions de rôle sont appliquées côté serveur, même si l'interface cache une action.
- Prévoir installation, mise à jour, compatibilité, migrations, sauvegarde/restauration, rétention, diagnostic et reprise. Publier les limites et procédures utiles à un utilisateur ou contributeur qui ne connaît pas le dépôt.
- Conserver une contribution accessible : code et contrats relisibles, documentation cohérente, dépendances maîtrisées, tests utiles et procédure de signalement/support explicite. Ne pas inventer une équipe de support ou un engagement de service.
- Découper l'implémentation en lots sans repousser une exigence obligatoire sous une étiquette « MVP », « V2 », « plus tard » ou « optionnel ». Une extension réellement facultative est justifiée par le besoin et les gates ; toute exigence non satisfaite reste un écart ouvert.

Ces critères font partie de l'acceptation avec les invariants de sûreté. Un lot fusionnable n'est pas une release qualifiée. Les documents historiques conservent leurs résultats datés ; leur ancien périmètre ne redéfinit pas le mandat actuel.

### Direction visuelle de l'IHM de contrôle

Référence demandée par le mainteneur : [Nothin’](https://www.noth.in/?ref=siteinspire), dont l'IHM doit s'inspirer autant que possible. La déclinaison produit est précisée dans [docs/ui.md](docs/ui.md).

Le guide détaillé fourni le 30 octobre 2026 précise désormais cette direction : composition éditoriale, fond `#FAFAF8`, texte `#101010`, navigation fixe sombre `#161714`, accent citron `#E2F471` rare, Arial/Helvetica et repères Courier New, grands titres, traits fins, surfaces neutres et rayons de 4–5 px. Cette référence doit se voir dans le front livré ; une mention documentaire ne remplace pas sa réalisation. Adapter ces codes aux vues réelles d'ai-guard, avec commandes stables, libellés directs et explications de provenance.

Le mainteneur a ensuite fourni deux visuels OVPT et demandé leur adaptation au logo, y compris à l'icône de l'onglet. **OVPT devient la signature visuelle ; ai-guard reste le nom du produit.** Préserver le mot-symbole massif, les lettres nettes, l'espace et le séparateur sobre. La devise fournie est « Omnia Vincit Progressus Technologicus ». À 16–32 px, employer une déclinaison lisible plutôt que comprimer une bannière. Aucun effet orbital, 3D, police distante ou propriété juridique déduite des visuels.

- Rechercher une identité graphique affirmée : typographie expressive et hiérarchisée, base noir/blanc, contrastes francs, composition épurée, espaces de respiration et accents visuels maîtrisés.
- Adapter cette direction aux tâches de supervision : tableaux et logs lisibles, hiérarchie des actions, accès rapide aux détails, densité utile sous charge et états métier explicites.
- Préserver les thèmes clair/sombre, les repères de provenance et de sécurité, la navigation clavier, le focus, les contrastes et l'usage sur petit écran. Les animations respectent `prefers-reduced-motion` et restent compatibles avec le suivi en temps réel.
- Le thème sombre doit avoir une composition propre : noirs neutres, surfaces distinctes, texte ivoire et traits lisibles. Éviter les fonds olive boueux, les grandes surfaces citron et l'inversion automatique du thème clair. Vérifier le rendu réel, les tableaux, les formulaires, les états d'erreur et le favicon sur les deux thèmes.
- Lors d'un chantier visuel, relire les vues principales avec cette référence et vérifier leurs états de chargement, vide, erreur, péremption et activité dense sur desktop/mobile. Les captures accompagnent les tests fonctionnels et les données réelles.

## Lire avant de modifier

Pour les correctifs de sécurité, lire aussi le [rapport final run-2 et son
registre de prise en compte](docs/engineering/AUDIT_RUN_2_20261110.md) : huit
constats démontrés, cinq pistes à valider et trois rejets sur `696efd1`.
Consulter les pièces primaires privées et rapprocher chaque fingerprint du
code courant avant correction ; ne pas assimiler un correctif voisin à sa
clôture. Les audits précédents restent des entrées obligatoires distinctes.

1. `docs/engineering/ARCHITECTURE_CONTRACT.md` : frontières et invariants.
2. `docs/engineering/CODING_RULES.md` : règles de code applicables.
3. `docs/engineering/PRODUCTION_READINESS.md` : gates G01 à G12 et écarts actuels.
4. `docs/engineering/TEST_STRATEGY.md` : tests existants, tests manquants et preuves.
5. Le code, les contrats et les documents directement concernés par la tâche.

Pour tout correctif de sécurité, lire le [suivi du rapport final run-1 sur la branche d'intégration](https://github.com/Needdoliprane/ai-guard/blob/codex/human-brokers-validation-20261104/docs/engineering/AUDIT_FOLLOWUP.md#audit-final-run-1), puis les pièces primaires et le lot concernés indiqués dans ce registre. Revérifier les constats contre la source courante et conserver reproduction, régression, revue et limites ; un ancien correctif partiel ne clôture pas le constat résiduel. Si les pièces privées ne sont pas accessibles, le signaler explicitement et poursuivre les travaux indépendants.

Pour le chantier global, lire aussi `PROMPT_PRODUCTION.md` et `docs/engineering/IMPLEMENTATION_PLAN.md`. Pour l'identité, les permissions, la configuration, l'inférence interne ou la découverte, lire le contrat canonique `docs/sso.md` ; pour les vues et les travaux visuels, lire `docs/ui.md`. Pour une publication, lire `docs/engineering/RELEASE_CHECKLIST.md`. Pour TLS, mTLS, certificats ou le NAS, lire d'abord `docs/engineering/INFRASTRUCTURE_PKI.md`.

Ces consignes s'appliquent aux agents principaux, sous-agents et revues. Les exigences récentes du mainteneur priment sur les anciens plans. Signaler un conflit réel ; ne pas réinterpréter un document historique pour réduire la cible à un MVP.

## Architecture et langues imposées

- Front existant en **TypeScript** ; l'exception de langage est limitée au frontend et à ses tests JS/TS. Le mainteneur autorise TS/JS/Python pour le front ; conserver ici le choix TS.
- Tout code applicatif hors front, y compris gateway, broker, collecteur, adaptateurs, outils de contrôle et nouvelles automatisations propres au projet, est en **Rust**.
- TOML, YAML, JSON, Dockerfiles et commandes d'utilisation restent des configurations. Ne pas introduire de logique métier en scripts Python, JavaScript ou shell hors front.
- Le front est compilé puis servi par un dashboard Rust **dans Docker/Podman** ; aucun serveur Node de production n'est requis.
- Dashboard, gateway/collecteur et workers sont séparés. Le broker Rust s'exécute dans le périmètre de contrôle de confiance : poste local ou service de plateforme cloud.
- Le mainteneur a autorisé le 30 octobre la direction [broker permanent et gardien TTY du même binaire Rust](docs/engineering/BROKER_SUPERVISION.md), avec IPC local privé et échéances durables. L'accord couvre code, configurations et tests, sans installation automatique de service OS ni action cloud/PKI. Conserver les plans fournisseurs1/2/3 et leurs bindings ; ne pas présenter la conception approuvée comme une supervision déjà livrée ou qualifiée.
- Aucun socket de moteur, mode privilégié ou montage du projet original dans les conteneurs applicatifs.
- Chaque session utilise un volume de travail alimenté par copie filtrée ; l'export crée une nouvelle destination. Le projet original n'est jamais réécrit automatiquement.
- Un client (`tenant`) possède de 0 à 100 workers actifs comme cible de dimensionnement. Chaque worker appartient exclusivement à un client ; un client peut posséder plusieurs conteneurs/pods. Les services de contrôle peuvent être partagés et ne sont pas des workers clients.
- Une base SQLite distincte par client est le stockage actuellement livré. La cible approuvée couvre SQLite local, MySQL distant avec identifiant et mot de passe, et MySQL distant avec identifiant, mot de passe et mTLS. Le choix est configurable et les trois profils partagent les contrats, règles, événements et relais. Lors d’un changement de stockage, l’opérateur prépare la cible et réalise lui-même tout transfert d’historique. Le gateway ouvre uniquement la base configurée, vérifie le schéma et la liaison au tenant, puis ajoute les nouveaux événements à son historique. Lire le [contrat de stockage](docs/engineering/ARCHITECTURE_CONTRACT.md#stockage-sql-par-tenant) et le [cahier de tests SQL](docs/engineering/TEST_STRATEGY.md#stockage-sql-configurable--à-qualifier). La configuration `deployment_mode=cloud` ne déploie pas une infrastructure.
- Kubernetes est la cible d'adaptateur cloud, non disponible dans le broker actuel. Ne pas confondre la valeur de métadonnée `runtime=kubernetes` et un lancement de pod réellement implémenté.

## Identité utilisateur, SSO facultatif et traçabilité complète

Le [contrat canonique d'identité, SSO et configuration](docs/sso.md) est obligatoire pour ce chantier. Il décrit une **cible à implémenter et qualifier**, comprenant l'identité locale nominative hors ligne et le SSO facultatif à l'installation, dont le support reste obligatoire dans le produit. ai-guard est un client OIDC configurable (Keycloak, Entra ID ou autre fournisseur qualifié), avec Authorization Code + PKCE S256 et clients publics ; aucun `client_secret` distribué. Vérificateurs privés éphémères, réponses et autorisations validées côté Rust ; le frontend ne fait pas autorité.

Les appartenances et permissions sont établies par le contrôle. Le mapping configurable des claims vérifiés s'applique au jeton d'accès côté Rust, avec périmètre d'organisation/tenants explicite. Le rôle externe de découverte `admin-ai-guard` n'est pas codé en dur pour les autres installations. Ni nom de compte, ni administration du realm, ni rôle applicatif ne donnent de credential `runner` ou d'accès implicite à un autre client. Le mode local suit le même contrat d'autorisation/audit.

L'attribution complète lie utilisateur initiateur → demande → session → agent → délégation → sous-agent → action/commande → décision → acceptation broker → exécution observée → résultat et changements. Distinguer initiateur, acteur effectif et causalité ; une action d'agent ne prouve pas une instruction humaine explicite. Les identités d'exécution sont propres, bornées, révocables et sans élévation lors d'une délégation. Aucun jeton SSO humain ni credential `runner` dans un worker. Les authentifications ai-guard et Codex/Claude restent distinctes ; ne pas réattribuer l'historique des anciennes clés partagées.

Tracer demandes/réponses visibles, délégations, outils, commandes structurées, décisions/approbations, sorties et changements observés dans un audit indépendant, durable, vérifiable et exportable, avec points de contrôle signés selon le contrat. Masquer les secrets avant persistance/diffusion ; contrôler accès, rétention, volumes et contenus sensibles. Toute omission, troncature, perte ou couverture partielle est explicite. Un sous-agent seulement déclaré reste `declared` ; aucune chaîne complète authentifiée sans preuves d'identité et de médiation. Un audit obligatoire indisponible bloque l'action.

Les [axes de configuration indépendants](docs/sso.md#configuration-par-axes-indépendants) couvrent topologie, identité, tenants/permissions, agent, abonnement/API/inférence interne, réseau/TLS, administration et exploitation. Les cas d'usage sont non exhaustifs. Fournir des profils simples, sans coupler SSO et fournisseur IA ; valider choix résolus et prérequis, puis refuser clairement les combinaisons incompatibles ou non qualifiées. Aucun réglage ne désactive un invariant ; aucune combinaison non testée n'est annoncée qualifiée.

## Essai rapide et environnement SSO de découverte

Le [parcours canonique de découverte](docs/sso.md#essayer-sans-configurer-toute-une-infrastructure) vise **environ cinq minutes**, sans garantie : Compose documenté, une seule clé API du fournisseur choisi, diagnostic, accès disponibles et premier agent réel audité. Distinguer préparation des images, démarrage à chaud, étapes manuelles et temps mesurés ; conserver confinement et audit.

Fournir le [profil Keycloak](docs/sso.md#profil-keycloak-de-découverte) local loopback **explicitement activé**, épinglé et préconfiguré, avec ses quatre comptes publics : `admin / admin` (realm), `admin-ai-guard / admin-ai-guard` (rôle applicatif), `user-1 / user-1` et `user-2 / user-2` (sans ce rôle). Ces identifiants ne sont jamais les valeurs par défaut d'une installation normale ; préserver credentials, bases et volumes existants. Aucun nom de fichier Compose n'est imposé.

La recette montre vraie connexion, action autorisée non destructive et trace attribuée, avec refus de rôle/périmètre, sans activité fictive. Identifier le realm comme démonstration. Livrer ensemble code, tests et procédures de découverte/raccordement entreprise, sans prétendre qualifier Keycloak en production ni fermer un gate par documentation.

## Clients et authentification dynamiques

Le projet fournit Dockerfiles et instructions ; le client construit et distribue ses images, y compris hors Internet. Aucune image publique préconstruite n'est obligatoire. Le [profil d'inférence interne](docs/sso.md#inférence-interne-et-déploiement-sans-internet), **à implémenter et qualifier**, lie URL, modèle, format compatible OpenAI précis/adaptation Claude, référence de credential, chaîne publique d'AC privée et destinations approuvées. Préparer le worker sans désactiver TLS, sans clé privée d'AC ni repli cloud ; les presets actuels ne prouvent pas ce parcours.

Le produit doit prendre en charge **Codex CLI et Claude Code, chacun avec abonnement et avec clé API**. Détecter les capacités disponibles et permettre une sélection explicite ou automatique du client et du mode, sans argv fournisseur imposé à l'utilisateur. Le choix résolu, sa provenance et ses prérequis sont visibles avant lancement et liés à l'autorisation lorsque leur changement affecte l'exécution.

La sélection explicite prime. Un abonnement demandé ne bascule jamais silencieusement vers une API facturée. Refuser une configuration ambiguë ; distinguer connexion requise, client absent, version incompatible et credential indisponible. Utiliser les mécanismes officiellement pris en charge, sans extraction implicite des caches OAuth ou du keychain hôte.

Le mainteneur autorise un `.env` local avec les clés fournies : fichier privé, ignoré par Git, exclu du workspace et des artefacts/frontend. Ne transmettre que les credentials nécessaires au client et au mode retenus par le handoff privé du contrôle, jamais le `.env` complet. Aucun secret dans argv, autorisations, logs, événements ou captures. Les exemples n'ont aucune valeur réelle. Les quatre parcours demandent leurs tests réels ; aides et fixtures ne qualifient ni abonnement, ni API, ni conversation interactive.

### Budget des validations fournisseur

Le mainteneur demande des essais économes : Luna pour l'orchestration des tests, tests ciblés selon le risque, aucune relance systématique E2E ou fournisseur. Les prochains essais Claude utilisent explicitement **Haiku 5.5**, identifiant officiel `claude-haiku-5-5` ; pas d'alias susceptible de résoudre un autre modèle ni de basculement silencieux. Les résultats historiques avec l'alias `haiku` ne sont pas rétroactivement qualifiés 5.5. Un appel modèle réel doit avoir un critère utile, une limite et une preuve ; après un défaut du harnais, corriger celui-ci hors ligne avant de recommencer. Une suite complète logicielle reste pertinente pour un changement transverse, sans la répéter par réflexe.

## Vue cloud unifiée par client

Un **dashboard unique par client** regroupe tous ses agents, sous-agents, workers et sandboxes, même répartis sur plusieurs brokers, hôtes ou runtimes. Aucun dashboard par worker à consulter. La cible reste au moins 100 workers par client, à mesurer réellement. L'identité authentifiée sélectionne le tenant : A voit tout A selon son rôle, B tout B, sans exposition inter-client.

Le control plane rassemble registre, événements, décisions, quotas et usage du client ; l'UI utilise ses API autorisées et son flux temps réel, sans moteur ni secrets runner. Afficher autorité d'exécution, provenance, fraîcheur et dernier état connu. Un broker déconnecté ou une collecte partielle n'est jamais présenté comme une vue complète ou zéro activité. Quotas et comptage restent cohérents au niveau du tenant.

Le [contrat des vues par permissions](docs/ui.md#vues-utilisateur-et-administration-unifiée--cible-à-livrer), **à implémenter et qualifier**, couvre utilisateur, administration et synthèse à 360° du périmètre autorisé : utilisateurs, agents/sous-agents, sessions, sandboxes, workers/brokers, décisions, incidents, quotas, usage et configurations. Droits/configuration effectifs viennent de Rust. Distinguer contexte personnel et administration, métadonnées, contenus, export et mutations ; retirer les données devenues interdites après changement de droits/périmètre, y compris détails, caches et flux. Un bouton visible ne constitue jamais une autorisation.

Conserver le cœur commun local/cloud et les frontières existantes. L'agrégation n'autorise ni fusion des bases clients, ni credentials globaux chez les workers, ni nouveaux microservices sans nécessité. Workload identity, transports, connexion des brokers distants et reprise restent à implémenter et qualifier ; cette consigne ne les déclare pas livrés.

## Trois approches Jev et décision locale

Le produit doit distinguer explicitement trois moteurs sélectionnables : **Jev réel** via son contrat authentifié, **moteur Rust déterministe sans modèle**, et **petit modèle local Jev-like** conteneurisé dans Docker/Podman. Ne pas appeler les deux derniers « Jev réel » ni leur attribuer sa couverture sans preuve. Le choix est visible, audité et configurable ; une panne ne permet pas de changer silencieusement de moteur ou de niveau de contrôle.

Pour Jev réel, le mainteneur a confirmé `POST https://api.typesafe.ai/v1/systemone`, Bearer et le modèle demandé `jev-latest`. Conserver le modèle effectivement retourné : cet alias peut évoluer. L'adaptateur de décision utilise une question `choice` (`allow|deny|review`) ; l'exemple `noul` fourni illustre un autre type du même contrat. Le header `TypeSafe-Danger-Zone-Long-Context` n'est pas envoyé par défaut : notre état de décision est réduit et borné, sans contrat long à transmettre. Aucun credential Jev dans un worker. Voir les [contrats officiels](https://docs.typesafe.ai/api/system-one), [Choice](https://docs.typesafe.ai/primitives/choice) et [modèles](https://docs.typesafe.ai/models).

Le moteur Rust a une couverture sémantique incomplète et imparfaite. Des règles et un code robustes ne constituent pas une compréhension exhaustive des intentions, des langages ou des effets transitifs. Nommer les limites et cas inconnus ; ne promettre ni perfection ni équivalence avec un évaluateur modèle. Le petit modèle local est lui aussi faillible : sortie validée et bornée, provenance explicite, incertitude et délai gérés, aucun score présenté comme probabilité calibrée sans validation.

Pour le moteur local, le mainteneur remplace la piste Gemma par une sélection adaptée au besoin, avec **au plus 4B de paramètres et une cible 500 Mo–1 Go de RAM**. Le choix du modèle reste délégué à l'orchestrateur. Modèle/révision, quantification, fenêtre de contexte, CPU, mémoire et délais sont configurables. Appliquer la limite mémoire au conteneur, mesurer le pic réel et les OOM ; une taille de fichier ou une simulation ne prouve pas le respect du budget. Préférer un modèle de génération/classement textuel adapté ; le modèle d'embeddings suggéré n'est pas imposé. Aucune prétention de performance avant essai réel borné. Aucun téléchargement lourd ni matrice d'inférence répétée sans nécessité.

Quel que soit ce moteur, les autorisations centrales, refus d'audit, isolation tenant et confinement broker/runtime restent appliqués indépendamment. Aucun secret Jev ou contrôle dans le worker métier ou dans le modèle local. Aucun modèle ne peut lever un refus impératif. Rust reste le langage de notre logique et de nos adaptateurs ; un runtime d'inférence tiers doit être identifié, épinglé et isolé, sans ajouter de service métier dans un autre langage.

## Contrat multi-client et interconnexion

- Le client est choisi à partir d'une identité authentifiée ; aucun header libre, querystring, corps JSON ou label fourni par un worker ne peut choisir son périmètre.
- Le rôle `runner` est réservé au contrôle de confiance. Le rôle `viewer` consulte son client. L'identité `worker` ne peut utiliser que son relais autorisé, sans accès au registre, aux métriques globales ou aux secrets du contrôle.
- L'autorisation d'un échange est dirigée `source → destination + canal`, bornée dans le temps si configuré et révocable. Même client n'implique pas communication libre. Les échanges entre clients sont refusés.
- Réservation/quota, état du worker, autorisation du relais, message et mesure terminale s'appuient sur des transactions durables avec audit obligatoire. Les reprises sont idempotentes ; une divergence de contenu pour la même identité est un conflit.
- Les messages du relais sont des données métier stockées dans la base du client ; l'audit ne duplique pas leur corps. Ne jamais y envoyer des secrets sans un mécanisme de protection explicite.
- Le relevé actuel est du temps d'allocation CPU/RAM, pas une mesure de CPU réellement consommé, pas un prix et pas une facture. Il porte `billable=false`. Ne pas inventer une durée ou déclarer zéro lorsqu'une observation runtime manque.
- La recette locale standard emploie **3 workers**, A1/A2 chez A et B1 chez B, plus les services de contrôle. Les tests de charge 100 sont distincts ; ne pas imposer 100 conteneurs à un développeur.

Lire `docs/engineering/CLOUD_TENANCY.md`, `docs/engineering/LOCAL_CLOUD_TESTS.md` et `docs/engineering/USAGE_METERING.md` pour tout changement touchant ces contrats.

## Invariants de sûreté

- Une autorisation et les traces obligatoires doivent être durables avant lancement. Une erreur d'audit ou de décision bloque le lancement ; aucun repli d'exécution métier sur l'hôte.
- Exécuter le moteur par programme et arguments séparés ; aucune interprétation de la commande métier par un shell hôte.
- Conserver le réseau coupé par défaut, les limites, le profil non privilégié et le refus des liens/traversées de chemins.
- Distinguer `observed`, `declared` et `inferred`, couverture native et couverture générique. Une information inconnue reste inconnue.
- Une demande, une autorisation, un démarrage et un effet constaté sont des événements distincts. Ne pas transformer un log fournisseur en preuve indépendante.
- Ne pas promettre de contrôle des actions transitives sans preuve d'un canal de médiation avant l'action et de tests de contournement.
- Ne jamais afficher, committer ou transmettre des secrets pour produire une preuve. Masquer avant persistance/diffusion ; conserver les clés et données existantes.
- Ne pas supprimer le dépôt, un worktree principal, des volumes, des preuves ou une configuration existante pour obtenir un test vert. Toute destruction doit être ciblée et autorisée.

## Chantier P1 — contrôle et supervision des sorties réseau

**À développer et qualifier avant la qualification production des parcours avec Internet.** Le mainteneur demande de contrôler les sites et services qu'un agent peut joindre, et de rendre ses tentatives visibles dans le dashboard. Cette exigence ne déclare aucun filtrage déjà livré.

- Conserver le refus par défaut ; autoriser les destinations nécessaires par politique du tenant et de la session. Un domaine inconnu reste bloqué jusqu'à décision autorisée ; les autorisations temporaires sont bornées, expirables et révocables.
- Appliquer le contrôle depuis le périmètre de confiance Rust, hors du worker, à tous ses processus : clients IA, `curl`, Python, Git et descendants. Définir aussi la couverture des outils Web et MCP exécutés à distance ; aucun chemin non couvert n'est présenté comme filtré.
- Refuser par défaut les accès à l'hôte, aux services de contrôle, aux autres tenants et aux métadonnées cloud. Les services internes nécessaires suivent des autorisations explicites et limitées à leur fonction.
- Enregistrer durablement la décision avant la connexion autorisée ; un échec de cet audit bloque la sortie. Corréler tenant, session, agent/worker, destination, motif et résultat réellement observé ; distinguer demande, autorisation et connexion effective.
- Afficher refus, demandes d'autorisation, expiration/révocation et couverture dans le dashboard. Masquer les secrets avant persistance ; ne pas collecter par défaut les corps ni les URL complètes avec paramètres sensibles. Durées et volumes restent inconnus sans observation.
- La réputation d'un site et Jev peuvent conseiller la décision ; ils ne remplacent pas le confinement et ne garantissent ni innocuité du contenu ni absence d'injection de prompt. Une destination autorisée conserve le même contrôle des permissions et des secrets.
- Critères d'acceptation : sur Docker, Podman et Kubernetes réels, vérifier domaine autorisé, domaine inconnu bloqué, autorisation temporaire puis révocation, refus inter-tenant, panne d'audit et tentatives de contournement par DNS, redirection, IP directe, IPv6 ou tunnel. Documenter la portée HTTPS et des outils distants ; relier les preuves à G05/G07/G11, avec revue indépendante et guides EN/FR synchronisés.

## Méthode de travail

**Tenir le [registre des travaux](docs/engineering/WORK_TRACKING.md) à jour pendant toute intervention.** Au démarrage ou à la reprise, vérifier le lot contre Git, ses preuves et ses exécutions. Inscrire les lots à venir dès qu'ils sont identifiés. Mettre le registre à jour après chaque affectation, changement d'état, validation, intégration et fermeture, puis avant tout relais et toute fin de session. Le coordinateur tient le registre canonique dans le checkout principal et y conserve les décisions et résultats datés transmis par les sous-agents.

Pour chaque changement, identifier le besoin, les fichiers touchés, les risques, le test pertinent et le critère d'acceptation. Examiner l'état Git et préserver le travail existant. Découper en changements relisibles ; ne pas refondre des modules sans nécessité liée au besoin.

Pour une correction, reproduire le défaut avant de modifier le comportement lorsque c'est possible, puis conserver une régression pertinente. Pour une documentation, vérifier les liens, les commandes, les contradictions et les statuts. Ne pas créer de tests sans valeur uniquement pour augmenter un compteur.

Préférence de test du mainteneur (30 octobre 2026) : utiliser **Luna pour les sous-agents chargés d'exécuter les tests**, privilégier les comportements réels et les vérifications ciblées. Un vrai Codex dans un worker est autorisé dans le cadre de cette qualification, avec les contrôles de credentials et de confinement applicables ; cette autorisation ne donne pas accès à une infrastructure cloud/PKI externe. Ne pas multiplier les mocks ou relancer systématiquement toute la matrice E2E. Choisir les régressions selon les fichiers et risques touchés ; réserver les campagnes complètes aux changements transverses, aux régressions non résolues et aux jalons de qualification qui les exigent. Cette économie de quota ne transforme pas un test non exécuté en PASS et ne supprime aucun gate obligatoire.

**Parallélisme obligatoire lorsque des tâches indépendantes peuvent avancer utilement.** Rechercher ces occasions à chaque lot et les déléguer sans attendre une nouvelle demande du mainteneur : implémentation, infrastructure, documentation, tests et revue peuvent avancer en parallèle lorsque leurs dépendances le permettent. Une compilation ou un test en cours ne justifie pas de laisser un chantier indépendant à l'arrêt.

Attribuer des propriétaires de fichiers clairs et des worktrees distincts lorsque nécessaire. Transmettre à chaque sous-agent la présente politique, le contrat d'architecture, son périmètre et la preuve attendue. Coordonner les fichiers partagés, les mutations de runtime et les caches de compilation ; ne pas paralléliser une dépendance ni multiplier les tests pour occuper les agents. Conserver la préférence Luna et les validations ciblées ci-dessus. Intégrer puis faire relire les modifications de sécurité, d'audit, d'import/export et de lancement par un second regard.

Mettre à jour les preuves et les écarts après chaque lot. Si une dépendance externe manque, marquer la qualification concernée `BLOCKED` ou `NOT_RUN`, continuer les travaux indépendants et indiquer précisément ce qui manque. Une fonctionnalité factice, un mock permanent ou une exception qui transforme le blocage en succès ne clôture pas le lot.

### Délégations bornées et refus d'outillage

Avant de déléguer, l'agent principal distingue lecture/remédiation ciblée et
reproduction ou intégration nécessitant des effets réels déjà autorisés. Choisir
le travail minimal utile au besoin ; ne pas ouvrir un audit complet par réflexe
ni ajouter des scénarios offensifs inutiles.

Chaque délégation fournit un contexte compact propre au sous-agent, selon ce
mini-gabarit à renseigner avec les faits de la tâche :

- **Autorisation et objectif** : demande réelle du mainteneur, correction attendue,
  comportement à vérifier et critère d'acceptation ;
- **Checkout** : dépôt, branche/base, fichiers possédés et contrats à lire ;
- **Surface** : chemins de la fixture synthétique ou des données autorisées,
  sandbox et adresse loopback si utilisées, réseau absent ou destinations
  effectivement autorisées ; ne pas déclarer une isolation non vérifiée ;
- **Opérations** : commandes ciblées et effets permis, exclusions réelles
  (services externes, credentials, mutations de runtime ou suppressions) ;
- **Validation et limites** : test pertinent, régression minimale, preuve attendue,
  budgets CPU/RAM/disque/délai et arrêt de la reproduction dès le premier oracle
  discriminant, au plafond ou au refus. Ne pas inventer un budget non établi.

Lorsqu'un chantier impose un profil d'exécution isolé, le transmettre explicitement
à tous les descendants avant leur premier outil. Une compilation peut exécuter des
build scripts : elle relève du même profil que les tests. Le sous-agent d'exécution
attend le gel de source, sa cible de compilation propre et le feu vert du coordinateur ;
un exemple Cargo dans un README ne lui donne pas un repli natif. Une commande ou un
harnais manquant se clarifie auprès du coordinateur, sans créer une autre sandbox ni
élargir les permissions. Conserver les travaux indépendants pendant cette clarification.

Une référence dans le code à un service externe n'autorise pas à agir sur ce service.
Nommer explicitement tout besoin d'accès, de donnée réelle ou d'action externe non
déjà autorisé ; poursuivre les travaux qui n'en dépendent pas. Transmettre les
instructions utiles et des pointeurs précis, sans recopier conversation, journaux
complets, secrets ou blocs de mémoire de session. Décrire honnêtement l'intention
et le périmètre défensif ; l'autorisation du mainteneur ne supprime pas les contrôles.

Après un refus, conserver le message exact expurgé des secrets, en signalant toute
expurgation, et les éléments observables : heure, lot, phase, agent, outil, modèle,
surface/chemin d'accès, source du refus et identifiant de requête. Marquer les champs
absents `inconnu` ; ne pas attribuer sans preuve le refus à une cause ou à un contrôle
interne. Marquer l'opération bloquée `BLOCKED` et ses tests non exécutés `NOT_RUN`,
jamais `PASS` ni `RED` produit. Ne pas relancer une requête identique.

Ne pas dissimuler l'intention, retirer un contrôle ou changer de modèle pour forcer
une réponse, ni fragmenter l'opération refusée pour en reconstituer l'exécution.
Le dépannage normal d'un modèle approuvé reste permis. Une ambiguïté légitime peut
être clarifiée par un contexte plus précis conservant intention et autorisation
réelles, sans reconstituer l'opération refusée. Si l'interdiction persiste, garder
le lot concerné `BLOCKED` et poursuivre les tâches distinctes autorisées ; ne pas
bloquer tout le chantier ni inventer un échec du produit. Nommer précisément la
dépendance nécessitant une décision ou information du mainteneur.

Daybreak et une meilleure formulation ne garantissent pas l'absence de refus.
Vérifier droits, réglages, modèle approuvé, connexion et workspace observables sur
chaque surface, sans supposer leur propagation aux sous-agents ni déduire la cause
d'un état UI périmé. Si nécessaire, proposer un signalement au
support expurgé des secrets avec les faits disponibles ; aucun taux de réussite non mesuré.
Consulter la documentation officielle à jour : [contrôles supplémentaires](https://help.openai.com/en/articles/20001326)
et [dépannage Daybreak](https://help.openai.com/en/articles/20001259).

### Cycle de vie des branches, des worktrees et des caches

Appliquer la [procédure de cycle de vie](docs/engineering/WORKSPACE_LIFECYCLE.md) à la création, à la reprise, à l'intégration et à la fermeture d'une branche de travail ou d'un worktree, et avant toute purge de cache. La clôture impose le retrait des branches de travail et des worktrees intégrés dès que les vérifications de conservation et d'usage sont satisfaites.

**La clôture impose la suppression de la branche de travail et du worktree secondaire déjà intégrés.** Vérifier l'intégration, les usages actifs et la conservation du travail avant suppression. Conserver le dépôt principal et les références pérennes. Consigner toute dépendance, preuve ou autorisation manquante et garder la fermeture de la ressource ouverte.

## Documentation et exemples de configuration

Le mainteneur demande un manuel utilisateur du produit complet, en **anglais et français**, organisé en guides par contexte : local, fournisseurs IA, Keycloak, inférence locale et entreprise. `README.md` est la version anglaise ; `README.fr.md` est sa version française. Les deux versions proposent les mêmes contrats, exemples et commandes. L'index du manuel explicite son périmètre de produit cible ; l'avancement réel, les preuves et les écarts d'implémentation restent dans `docs/engineering/`, sans être transformés en réussite.

**Un changement de contrat met à jour dans le même lot les guides anglais/français, les fichiers de configuration et les commandes concernés.** Les exemples `.env.exemple.*` et leurs équivalents typés doivent désigner leur consommateur, leurs priorités et leurs valeurs publiques ; aucun secret réel, option silencieusement ignorée ou endpoint supposé livré. La documentation est un contrat à implémenter, pas une preuve de fonctionnement.

Seuls les tests pertinents du produit sont distribués. Les essais auxiliaires Keycloak, Qwen et du générateur d'AC restent locaux, exclus du suivi par `.gitignore`, avec leurs preuves conservées hors de la distribution. Ne plus les ajouter ou les pousser. Les régressions de sécurité, API, broker, runtime et autres comportements du produit restent suivies. Une instruction d'exclusion Git n'autorise pas l'effacement des fichiers, preuves, volumes ou de l'historique existants.

Le mainteneur demande des commits et push réguliers après les lots cohérents, validés et relus. Utiliser le dépôt distant configuré, préserver son historique et exclure secrets, configurations privées, bases, volumes et artefacts locaux sensibles. Un checkpoint de développement ne constitue pas une release qualifiée.

## Commandes existantes

À la racine, avec la toolchain de `rust-toolchain.toml` :

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
CARGO_INCREMENTAL=0 cargo test --locked
cargo build --release --locked
```

Dans `front/`, avec Node 24 pour reproduire la validation de référence :

```sh
npm ci
npm test
npm run build
```

Le parcours `npm run test:browser` requiert Playwright et Chromium disponibles. Le test `runtime_smoke` est ignoré par défaut et exige un vrai moteur, les images et le gateway démarré. Les procédures exactes sont dans `docs/engineering/TEST_STRATEGY.md` et `docs/operations.md`. Ne jamais compter un test ignoré comme une réussite.

## Skills de projet

| Besoin | Skill |
|---|---|
| Vérifier Rust, front, API ou une correction | `ai-guard-verify` |
| Qualifier Docker/Podman et le scénario local à 3 workers | `ai-guard-qualify-runtime` |
| Examiner une frontière de sécurité | `ai-guard-security-review` |
| Évaluer une candidate à la production | `ai-guard-release-review` |

Les emplacements attendus sont `.agents/skills/` pour Codex et `.claude/skills/` pour Claude ; ces noms ne prouvent pas leur présence dans le checkout. Les copies, lorsqu'elles existent, doivent rester identiques. Claude les appelle avec `/nom-du-skill` ; Codex avec `$nom-du-skill` lorsque la version installée les découvre. Si la découverte échoue, lire explicitement le `SKILL.md` concerné lorsqu'il est présent et signaler que le chargement automatique n'a pas été confirmé. Si le fichier manque aussi, signaler son absence et poursuivre les travaux indépendants avec les contrats ; ne pas prétendre avoir appliqué un skill absent.

Les [skills Matt Pocock installés dans le projet](docs/agents/README.md) complètent cette méthode. Utiliser `diagnosing-bugs` pour un défaut difficile à isoler, `tdd` pour une régression ou une tranche fonctionnelle qui exige un test utile, et `code-review` pour les revues sur les deux axes contrat et règles de code. Les contrats d'ai-guard et le périmètre confié à l'agent s'appliquent aussi aux actions proposées par un skill.

### Suivi des tickets

Les skills utilisent des [fichiers Markdown locaux](docs/agents/issue-tracker.md) dans `.scratch/` et les [cinq états de triage](docs/agents/triage-labels.md). Toute opération sur un suivi distant suit les autorisations de la tâche.

### Documentation du domaine

Les [sources du vocabulaire et des décisions](docs/agents/domain.md) sont les contrats, le plan et le journal de décisions existants.

## Production, infrastructure et comptes

La première qualification proposée est locale, à hôte unique, y compris les contrats multi-client avec 3 workers. Docker et Podman doivent être testés séparément ; une preuve Linux ne qualifie pas macOS/ARM64, ni un cluster Kubernetes. Le cloud exige en plus identité de workload, chiffrement des transports, politiques réseau réellement appliquées, reprise et autorité de comptage. Les versions réellement testées doivent être consignées.

La PKI du NAS est une intégration possible, pas une dépendance supposée. Aucun endpoint, compte, rôle, moteur, certificat ou accès NAS n'est confirmé par ce dépôt. Avant une opération réelle, demander au mainteneur les informations et l'autorisation nécessaires selon le guide PKI. Continuer les travaux locaux sans inventer ces valeurs. Ne jamais désactiver la vérification TLS pour contourner un problème.

La licence open source retenue est Apache-2.0, dans `LICENSE`. Le canal privé de signalement des vulnérabilités reste à décider avant publication. Ne pas inventer un propriétaire, une adresse de contact, un dépôt distant, un certificat ou une autorisation.

## Définition de terminé

Une tâche est terminée lorsque son comportement est implémenté, ses erreurs et états incomplets gérés, son parcours utilisable avec des données réelles, ses tests pertinents exécutés, sa documentation à jour et sa revue traitée. Une release n'est qualifiée que si tous les gates applicables du périmètre déclaré ont une preuve acceptable et que les écarts bloquants sont fermés. Réduire le périmètre d'une preuve ne réduit pas le périmètre du produit demandé.

Inscrire le résultat dans le registre. Un lot intégré dont la branche ou le worktree est encore à retirer reste en clôture avec une action et un propriétaire.

Rendre un bilan : **changements ; commandes et résultats constatés ; critères acceptés ; limites ; blocages précis**. Séparer `PASS`, `FAIL`, `BLOCKED`, `NOT_RUN` et `NOT_APPLICABLE` justifié. Citer les sources officielles pour les faits externes et les fichiers pour les faits du dépôt.

Les instructions et skills guident les agents. Les garanties doivent être appliquées par le code, les permissions, les politiques et les contrôles de validation ; la présence de ce fichier n'est pas une barrière de sécurité.

Les réglages personnels d'outillage, chemins de machine et blocs de mémoire de session appartiennent à la configuration locale des agents. Ne pas les injecter dans ces consignes communes ni les traiter comme des exigences du produit.





<claude-mem-context>
# Memory Context

# [ai-guard] recent context, 2026-11-19 7:07am GMT+1

Legend: 🎯session 🔴bugfix 🟣feature 🔄refactor ✅change 🔵discovery ⚖️decision 🚨security_alert 🔐security_note
Format: ID TIME TYPE TITLE
Fetch details: get_observations([IDs]) | Search: mem-search skill

Stats: 50 obs (27,322t read) | 7,797,796t work | 100% savings

### Nov 3, 2026
S799 Independent adversarial security review of ai-guard Rust AI agent platform — wave 3 launched after critic identifies two new investigation tracks (Nov 3 at 12:17 AM)
S800 Revue de sécurité contradictoire du dépôt ai-guard — vague 3 : hunters B19 et U18, suivi des tests background (Nov 3 at 12:22 AM)
S801 Finish the ai-guard security audit — collect wave-3 hunt/verify results, run reproduction tests, update audit artifacts, and prepare for final report (Nov 3 at 12:27 AM)
S802 Finish the ai-guard security audit — complete wave-3 findings verification, run all reproduction tests, and prepare the final report (Nov 3 at 1:07 AM)
S803 Security audit of ai-guard repository — wave 4 final verification phase, FD exhaustion finding, report generation, B5/B20 verification cleanup (Nov 3 at 1:12 AM)
S804 Adversarial security review of ai-guard (Rust AI agent platform) — independent harness FD-latch verification, B20 fix validation, B5 quota-trap re-confirmation, and precision FD threshold testing (Nov 3 at 8:46 AM)
S805 Adversarial security review of ai-guard — p1/p2 fix discrimination, B20 v3/v6/f2/f1-deadline scenarios, tamper controls, B5 stall probe, stop-fix comparison tests (Nov 3 at 9:29 AM)
S806 Adversarial security review of ai-guard — branch FD attack trigger precision (anon_human vs malformed vs plain), v2/v5 recovery controls, blind pulse history, stall probe comparison, armed-branch import trap, stop-branch precise doses (Nov 3 at 9:31 AM)
S807 Adversarial security review of ai-guard — ingress FD isolation test, nofile=1024 confirmation, kill-main-flood with clean build and fixture engine, rate limiter measurement, ingress architecture constants (Nov 3 at 9:33 AM)
S808 Adversarial security review of ai-guard — f1_base_hold8 unexpected trap, p2 kill-flood fix validation, gated_labels per-phase stall mapping, v4 blind pulses, f2 clean baseline, B5 run 3 (Nov 3 at 9:36 AM)
### Nov 6, 2026
6423 10:16p 🔵 Authentication Audit Ledger: Capacity Checked Before Tenant Profile Read to Prevent Side-Channel Audit
6424 " 🔵 f1 Base Binary Consistently Recovers After FD Hold (429 then Accepted); Fix Binary Has preparation_failed Regression
6425 " 🔵 Armed Import Quota Trap: Consistently Reproduced on Branch — Worker Permanently Stuck in InDoubt
6426 " 🔵 Gateway Write Rate Limiter: 200 Writes/Second Per Tenant Shared Across All Write Paths
6427 " 🔵 Workspace Volume Has No Byte or Inode Quota in Docker/Podman Mode
6428 " 🟣 Three New Audit Comparison Test Files Added to Tip Snapshot 5e52807
6429 " 🔵 Broker Outbox Capacity: 4MiB Byte Cap + 512 Entry Lifetime Cap; Save_worker_result Atomically Freezes Entire Capture Batch
6430 " 🔵 Broker IPC Peer UID Check and Unix Socket Listener Architecture
6432 10:21p 🔵 Outbox Amplification Confirmed on Tip 5e52807: 28.8× Amplification → TooLarge; 11+502 entries → LimitExceeded
6433 " 🔵 Upload Budget: Ingress Comparison Test Passes — Bodyless GET Spared, Runner POST Refused During Hold
6434 " 🔵 FD Exhaustion: Runner Decide Recovers Immediately at +3s Without Any Recovery Read
6435 " 🔵 Gateway Security: Cross-Site Refusal, CSP, Cache-Control Headers Applied to All Responses
6436 " 🔵 Credential Collision Detection: reject_control_collision Checks All Tenants for Human/Worker Token Ambiguity
6437 " 🔵 New Test: audit_cmp_auth_ledger_verified_replay.rs — Residual: One Verified Token Without Account Drains Shared Auth Ledger
6438 " 🔵 Enforcer Server: Slots Insert-Only, Never Evicted, No Retirement in Cancelled Path or Transport Version
6439 " 🔵 audit_w4_verify_kill_release Branch r3 (24 Workers, 20 Readers): PASSED — Worker Reaches Terminal with quota_held=false
6440 " 🔵 Missing Requirements Document: 16 Gaps Including Audit Capacity Exhaustion After ~138 Sessions and No Outbound Network Filtering
6441 " 🔵 AuditClient: HTTPS-Only (Except Loopback HTTP), 15s Timeout, No Retry, No Redirect, No Proxy; 429 Propagated as Error
6442 " 🔵 Cold Audit Reservation Uses Weak Arc Reference; check_selected_pair Requires Exactly 1 Coverage + 1 Result as Pending Events
6443 " 🔵 Offline Identity CLI: provision_local_human Requires --confirm-control-stopped; Online Mutations in human_workers.rs and human_brokers.rs Don't
6444 " 🔵 Gateway Stream (SSE): 500ms Poll Interval, 10s Keep-Alive Heartbeat, 200-Event Batches, Error Breaks Stream
6445 10:27p 🔵 Background Test Run RC=0: auth_ledger_replay (1/1), authentication_start_capacity (7/7), human_read_budget (1/1) All Pass
6446 " 🔵 Tenant Isolation Fix Confirmed at Tip 5e52807: tenant_api 4/4 Pass Including decision_and_live_stream_are_isolated
6447 " 🔵 Synchronous Reads Fix Confirmed: /healthz Returns in 123.5µs During 2 Concurrent 927ms Reads (Lot D, b168784)
6448 " 🔵 Cross-Tenant SQLite Coupling Still Present at Tip: B Waits 10.76s Behind A's Write Lock and 938ms During A's Read
6449 " 🔵 Worker Kill Under FD Flood Confirmed at Tip 5e52807: Kill Observed 0.30s After 336 Sockets Held
6450 " 🔵 Storage Fault Latch Confirmed at Tip 5e52807: EFBIG → Store Latches → healthz 503, decide 503, Reopen Restores
6451 " 🔵 FD Exhaustion Transient Recovery: EMFILE/ENFILE Now Distinguished from Permanent Failures at Tip (lot 6871634)
6452 " 🔵 store_io Gate: Separate Read (32), Control (128), Discovery (16) Lanes with Per-Store Admission; Closes main_sync-reads
6453 " 🔵 58,618 Lines Added Between Branch d6cb136 and Tip 5e52807: Top Modules Are Kubernetes, Broker Service, Cold Audit, Broker Presence, Workload, Relay
6454 " 🔵 Kubernetes Workload Admission Explicitly Closed at Tip: require_workload_admission Always Returns WorkloadAdmissionUnavailable
6455 " 🔵 Relay API Wired but Not Connected to Workers in Network-Isolated Profile: Worker Business Relay Remains Unqualified
6456 " 🔵 human_approved Field Confirmed: 40+ Uses as Boolean/Hash Only; No Approver Identity, Timestamp, or Channel Recorded
6457 " 🔵 f2 Refused-Claim Test (Branch Fix Binary): 3 Workers Trapped at StartIssued quota_held=true; 4th Reservation Refused 429 — Defect Still Present in Fix Binary
6458 " 🔵 AUDIT_FOLLOWUP.md Developer Candidates Table: 7 Open Candidates Including Exchange Pool, Viewer-Synchronous-Reads, Volume-No-Byte-Quota, Abrupt-Broker-Stop
6459 " 🔵 Kubernetes Held-CREATE Quota: Cancel from Reserved Releases; Cancel from EmissionPossible/OwnershipUnknown Does NOT Release Quota
6460 10:35p 🔵 refused_claim Test Confirms Replay Invariant: "a replay must never return a fresh start disposition" Still Enforced
6461 " 🔵 FD Exhaustion Developers' Tests Pass 2/2 at Tip: temporary_emfile_refuses_append + one_free_descriptor_preserves_cause
6462 " 🔵 Broker Service Fencing: AtomicBool Fenced Set at 20+ Sites; Fenced Prevents New Starts but NOT D5 Retirement
6463 " 🔵 Comparison Audit Finalized: 16 Records Validated (13 confirmed, 2 needs_validation, 1 rejected); 55 Coverage Units; 100 Total Agents
6464 " 🔵 Jeu Engine: Local Allow Maps to Review (Advisory Only); RuleCatalog Exact argv Match Only; TypeSafe Backend for Remote
6465 " 🔵 Storage Failure Path Bifurcation: descriptor_shortage → suspended; All Other Errors → permanent invalidate_storage
6466 " 🔵 New Surface Inventory: 58,618 Lines Added Between Branch and Tip; 10 Unaudited High-Risk Modules; 7 New Leads Filed
6467 " 🔵 Comparison Groups Complete: 15 Records Across 6 Groups; Summary of All Statuses
6468 " 🔵 Worker Quota Release: Only 2 Transitions Release quota_held in Control Model; InDoubt, Stop, StartIssued All Keep quota_held=true
6469 " 🔵 Egress Control Is a Design Proposal Only: No Implementation, G05/G07/G11 Open; Bridge Network Unfilterable
6470 " 🔵 Double Collision Scan Per Request: authorize_sync Runs Human+Workload Scans Twice (at authorize and inside every blocking call)
6471 " 🔵 Audit Capacity: Physical Cap 256 MiB Hardcoded; No Archive/Rotation/Purge Path; Capacity Full Refused with retry_action="contact_operator"
6472 " 🔵 Dashboard Proxy Allowlist Includes /api/v1/federation/workers and All /api/human/* Routes; Identity Preflight Doubles Read Cost
6473 " 🔵 CA Bundle: 262,144-byte Limit at Config Load; No Certificate Count Limit in tls::certificates(); Doc States 64 KiB / 8 Certs

Access 7798k tokens of past work via get_observations([IDs]) or mem-search skill.
</claude-mem-context>
