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

1. `docs/engineering/ARCHITECTURE_CONTRACT.md` : frontières et invariants.
2. `docs/engineering/CODING_RULES.md` : règles de code applicables.
3. `docs/engineering/PRODUCTION_READINESS.md` : gates G01 à G12 et écarts actuels.
4. `docs/engineering/TEST_STRATEGY.md` : tests existants, tests manquants et preuves.
5. Le code, les contrats et les documents directement concernés par la tâche.

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
- Une base SQLite distincte par client est le stockage actuellement livré. Local et cloud partagent contrats, règles, événements et relais ; changer de stockage ou ajouter la haute disponibilité exige une migration documentée et des preuves. La configuration `deployment_mode=cloud` ne déploie pas une infrastructure.
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

## Méthode de travail

Pour chaque changement, identifier le besoin, les fichiers touchés, les risques, le test pertinent et le critère d'acceptation. Examiner l'état Git et préserver le travail existant. Découper en changements relisibles ; ne pas refondre des modules sans nécessité liée au besoin.

Pour une correction, reproduire le défaut avant de modifier le comportement lorsque c'est possible, puis conserver une régression pertinente. Pour une documentation, vérifier les liens, les commandes, les contradictions et les statuts. Ne pas créer de tests sans valeur uniquement pour augmenter un compteur.

Préférence de test du mainteneur (30 octobre 2026) : utiliser **Luna pour les sous-agents chargés d'exécuter les tests**, privilégier les comportements réels et les vérifications ciblées. Un vrai Codex dans un worker est autorisé dans le cadre de cette qualification, avec les contrôles de credentials et de confinement applicables ; cette autorisation ne donne pas accès à une infrastructure cloud/PKI externe. Ne pas multiplier les mocks ou relancer systématiquement toute la matrice E2E. Choisir les régressions selon les fichiers et risques touchés ; réserver les campagnes complètes aux changements transverses, aux régressions non résolues et aux jalons de qualification qui les exigent. Cette économie de quota ne transforme pas un test non exécuté en PASS et ne supprime aucun gate obligatoire.

Paralléliser les travaux indépendants avec des propriétaires de fichiers clairs. Transmettre à chaque sous-agent la présente politique, le contrat d'architecture, son périmètre et la preuve attendue. Intégrer puis faire relire les modifications de sécurité, d'audit, d'import/export et de lancement par un second regard.

Mettre à jour les preuves et les écarts après chaque lot. Si une dépendance externe manque, marquer la qualification concernée `BLOCKED` ou `NOT_RUN`, continuer les travaux indépendants et indiquer précisément ce qui manque. Une fonctionnalité factice, un mock permanent ou une exception qui transforme le blocage en succès ne clôture pas le lot.

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

## Production, infrastructure et comptes

La première qualification proposée est locale, à hôte unique, y compris les contrats multi-client avec 3 workers. Docker et Podman doivent être testés séparément ; une preuve Linux ne qualifie pas macOS/ARM64, ni un cluster Kubernetes. Le cloud exige en plus identité de workload, chiffrement des transports, politiques réseau réellement appliquées, reprise et autorité de comptage. Les versions réellement testées doivent être consignées.

La PKI du NAS est une intégration possible, pas une dépendance supposée. Aucun endpoint, compte, rôle, moteur, certificat ou accès NAS n'est confirmé par ce dépôt. Avant une opération réelle, demander au mainteneur les informations et l'autorisation nécessaires selon le guide PKI. Continuer les travaux locaux sans inventer ces valeurs. Ne jamais désactiver la vérification TLS pour contourner un problème.

La licence open source retenue est Apache-2.0, dans `LICENSE`. Le canal privé de signalement des vulnérabilités reste à décider avant publication. Ne pas inventer un propriétaire, une adresse de contact, un dépôt distant, un certificat ou une autorisation.

## Définition de terminé

Une tâche est terminée lorsque son comportement est implémenté, ses erreurs et états incomplets gérés, son parcours utilisable avec des données réelles, ses tests pertinents exécutés, sa documentation à jour et sa revue traitée. Une release n'est qualifiée que si tous les gates applicables du périmètre déclaré ont une preuve acceptable et que les écarts bloquants sont fermés. Réduire le périmètre d'une preuve ne réduit pas le périmètre du produit demandé.

Rendre un bilan : **changements ; commandes et résultats constatés ; critères acceptés ; limites ; blocages précis**. Séparer `PASS`, `FAIL`, `BLOCKED`, `NOT_RUN` et `NOT_APPLICABLE` justifié. Citer les sources officielles pour les faits externes et les fichiers pour les faits du dépôt.

Les instructions et skills guident les agents. Les garanties doivent être appliquées par le code, les permissions, les politiques et les contrôles de validation ; la présence de ce fichier n'est pas une barrière de sécurité.

Les réglages personnels d'outillage, chemins de machine et blocs de mémoire de session appartiennent à la configuration locale des agents. Ne pas les injecter dans ces consignes communes ni les traiter comme des exigences du produit.
