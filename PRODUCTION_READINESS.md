# Préparation à la production

> **Rectification du 29 octobre 2026 :** l'inventaire du checkout a constaté l'absence des modules `tenancy.rs`, `control.rs`, du registre, des quotas, du relais et du ledger annoncés ci-dessous. Les passages décrivant ces fonctions comme livrées sont des affirmations historiques contredites, et ne doivent pas servir de preuve. Les exigences G01–G12 sont conservées ; l'avancement réel est suivi dans [CURRENT_STATE.md](docs/engineering/CURRENT_STATE.md), les [gates courants](docs/engineering/PRODUCTION_READINESS.md) et [VALIDATION.md](docs/engineering/VALIDATION.md). La cible production reste intégrale.

Date de référence : **29 octobre 2026**. Ce document définit les conditions de publication et d'exploitation d'**ai-guard**, produit open source, local-first, destiné à la production, avec une version cloud partageant majoritairement les mêmes contrats et le même domaine Rust. Il conserve le broker local, les workers isolés, la gateway d'audit, le dashboard séparé et le frontend TypeScript. Le contrôle multi-client Rust est implémenté ; l'orchestration cloud par adaptateur Kubernetes reste à réaliser. Ce document ne déclare pas la version courante qualifiée pour la production.

Documents associés : [stratégie de tests](TEST_STRATEGY.md), [infrastructure et PKI](INFRASTRUCTURE_PKI.md), [checklist de release](RELEASE_CHECKLIST.md), [tenancy cloud](CLOUD_TENANCY.md), [tests local/cloud](LOCAL_CLOUD_TESTS.md), [mesure d'usage](USAGE_METERING.md), [architecture](../architecture.md), [opérations](../operations.md) et [rapport historique de validation](../validation.md).

## 1. Cible et règle de décision

Le profil local est un poste ou serveur administré par un opérateur de confiance, avec plusieurs sessions possibles et des données persistantes. Le navigateur et le CLI utilisent les ports loopback. Kubernetes, la haute disponibilité et une PKI ne sont pas des prérequis de cette production sur un seul hôte. Une maintenance avec interruption maîtrisée est acceptable ; une perte silencieuse de données ou une autorisation sans audit ne l'est pas.

Le profil cloud vise plusieurs clients : **chaque worker/pod/conteneur appartient exclusivement à un tenant**, qui peut disposer de plusieurs workers, jusqu'à une cible de **100 workers par client** à dimensionner et prouver. Des communications entre workers sont nécessaires, mais seulement après un grant explicite ; appartenir au même tenant ne donne pas automatiquement accès. L'identité, les décisions, les événements, les quotas et l'usage suivent les mêmes contrats métier dans les deux profils. Le broker hôte local et le contrôleur cloud sont deux implémentations de l'autorité d'exécution de confiance, avec des adaptateurs de runtime distincts.

**État confirmé du code courant :** runtime Docker/Podman local ; identité tenant dérivée de credentials distincts runner/viewer/worker ; base SQLite liée à chaque tenant ; workers, quotas, grants dirigés, relais JSON et unités d'usage persistants. Le runner réserve avant création et conserve les transitions ambiguës pour réconciliation. Les unités sont `billable:false` : aucune facture financière n'est calculée. Les IDs projet/session seuls ne constituent pas l'autorité client. Les tests logiciels ne qualifient ni le confinement réel ni une plateforme cloud. Le poste ou contrôleur, son administrateur, les images et le runtime restent de confiance ; la résistance à toute faille noyau n'est pas revendiquée. Sources : [tenancy](../../src/tenancy.rs), [contrôle](../../src/control.rs), [runner](../../src/runner.rs), [tenancy cloud](CLOUD_TENANCY.md).

Une offre cloud facturée à l'usage fait partie de la cible. La mesure fiable, l'attribution au bon client, la déduplication et la reprise du ledger sont des exigences d'ingénierie. Les tarifs, devises, taxes, moyens de paiement, conditions commerciales et outils de vente restent des décisions distinctes à prendre ; ils ne sont ni implémentés ni inventés ici. Cette séparation ne réduit aucune exigence de qualité.

**Une release est publiable pour son périmètre annoncé uniquement si les douze gates ci-dessous sont `PASS`, ou `NOT_APPLICABLE` avec une justification admissible.** Un `FAIL`, `BLOCKED` ou `NOT_RUN` sur un contrôle obligatoire bloque la publication comme version de production. Une cible demandée ne devient pas hors périmètre faute de machine de test. Les décisions d'applicabilité sont prises avant les essais, enregistrées et revues ; elles ne servent pas à effacer un échec.

## 2. Nature des informations et statuts

Deux champs distincts accompagnent chaque affirmation :

| Nature | Sens |
|---|---|
| **Confirmé** | Lecture du code/configuration à une révision identifiée, ou résultat directement accompagné de sa preuve. Une lecture confirme l'existence d'un mécanisme, pas son efficacité sur un moteur réel. |
| **Supposé** | Hypothèse de dimensionnement, scénario d'usage ou objectif proposé. Elle doit être confrontée aux mesures. |
| **À vérifier** | Version, capacité, environnement ou comportement à confirmer par une preuve. |

| Statut d'exécution | Utilisation |
|---|---|
| `PASS` | Critère rempli sur le candidat, l'environnement et les options indiqués, avec preuve consultable. |
| `FAIL` | Essai exécuté ou inspection discriminante montrant que le critère n'est pas rempli ; défaut et reproduction joints. |
| `BLOCKED` | Un prérequis manque : implémentation, décision, matériel, autorisation, licence ou accès. Nommer le blocage et son responsable. |
| `NOT_RUN` | Essai prévu mais non exécuté ; aucun résultat implicite. |
| `NOT_APPLICABLE` | Exigence réellement extérieure au périmètre approuvé, avec justification, auteur et date. |

### Baseline historique, distincte d'une qualification

Le [rapport du 29 octobre 2026](../validation.md) consigne **85 tests Rust réussis, 0 échec, 1 test moteur ignoré**, **17 tests frontend réussis**, Clippy, builds et parcours navigateur. Il décrit aussi des échanges HTTP en loopback et des fixtures. Ce sont des résultats historiques sur Linux x86_64 ; ils ne sont pas des résultats nouveaux de ce dossier documentaire, ni des `PASS` transférables à une autre révision ou plateforme.

Le test de charge historique porte sur **1 000 événements et 8 producteurs synthétiques**. Il ne qualifie ni huit agents cloud ni un débit de production. Aucun moteur Docker/Podman, aucune image exécutée, aucun compte Codex/Claude/Jev et aucune plateforme macOS ARM64 n'ont été qualifiés dans cette baseline. Les commandes à rejouer sont dans [TEST_STRATEGY.md](TEST_STRATEGY.md).

L'évolution multi-client ajoute des tests de stockage, API, contrôle persistant, clients et configuration, ainsi qu'un second smoke réel **ignoré**. Les résultats de cette évolution sont consignés séparément dans le rapport courant ; ils n'effacent pas la baseline 85/17 et ne deviennent pas une preuve de trois conteneurs lancés.

## 3. Matrices obligatoires par release

### Plateformes et moteurs

La cible Linux et Mac ARM64 impose au minimum les quatre lignes suivantes. Chaque ligne possède son propre rapport ; un résultat Linux Docker ne couvre ni Podman ni la VM d'un poste Mac. Les versions exactes restent à renseigner, sans valeur inventée.

| Hôte cible | Moteur et mode à qualifier | Informations obligatoires | État de qualification actuel |
|---|---|---|---|
| Linux x86_64 | Docker rootless | Distribution, noyau, version client/daemon/Compose, cgroups, seccomp, LSM, filesystem, digests amd64 | `NOT_RUN` — moteur absent de la baseline |
| Linux x86_64 | Podman rootless | Même inventaire ; fournisseur Compose exact, réseau rootless, mappings UID/GID et SELinux si actif | `NOT_RUN` — moteur absent de la baseline |
| macOS ARM64 | Docker Desktop et sa VM Linux | Version macOS/Desktop/Compose, ressources et noyau VM, architecture arm64, traitement explicite de `--allow-rootful-runtime` si nécessaire | `NOT_RUN` — aucune preuve sur cette cible |
| macOS ARM64 | Podman machine | Versions macOS/Podman/machine/Compose, backend et noyau VM, mode rootless effectif dans la VM, mappings et digests arm64 | `NOT_RUN` — aucune preuve sur cette cible |

Le mode rootless est préféré et requis par défaut dans le CLI. Une exception rootful doit être explicite, enregistrée et visible dans le rapport ; elle ne reçoit pas le label rootless. Les propriétés du moteur doivent être inspectées, pas déduites de son nom. Le code prévoit une publication d'export spécifique Linux/macOS ; seule l'exécution des tests sur l'OS concerné qualifie cette voie. Sources : [runtime](../../src/runtime.rs), [workspace](../../src/workspace.rs), [opérations](../operations.md).

Linux ARM64, macOS Intel, Windows/WSL et d'autres distributions restent **à décider**. Ils ne sont pas implicitement pris en charge. Toute ligne ajoutée reprend les mêmes gates. La matrice d'une release indique aussi les navigateurs et versions retenus, les images par architecture et les versions minimales/maximales réellement testées ; « dernière version » n'est pas un identifiant reproductible.

### Profils local/cloud et preuve de séparation client

| Profil | Parcours obligatoire | État actuel |
|---|---|---|
| Local, workers isolés | Quatre lignes Docker/Podman ci-dessus ; contrats d'exécution/audit communs | Qualification moteur `NOT_RUN` |
| Local, scénario multi-client | Trois workers réels : A1/A2 du tenant A, B1 du tenant B ; grant A1→A2, refus A1→B1, révocation, falsification d'identité, isolation des volumes/audits, usage et quotas | `NOT_RUN` — logique/clients/Compose et smoke ignoré implémentés ; moteur réel absent |
| Kubernetes local de qualification | Même scénario avec l'adaptateur cloud, API/RBAC et CNI capable d'appliquer les NetworkPolicy ; versions exactes du cluster/runtime/CNI enregistrées | `BLOCKED` — adaptateur et environnement non fournis |
| Cloud de production | Même contrat ; TLS/mTLS, stockage, CNI, quotas, ledger et reprise sur la plateforme choisie ; charge réelle selon le plafond annoncé | `BLOCKED` — plateforme et capacité à qualifier, aucun déploiement demandé ici |

Le scénario A1/A2/B1 est commun aux implémentations et chaque moteur apporte sa propre preuve de confinement. La capacité de réservation par tenant est vérifiée séparément dans G10. [Recettes local/cloud](docs/engineering/LOCAL_CLOUD_TESTS.md)

### Fournisseurs

| Intégration | Déclaration exigée pour chaque release | Qualification actuelle |
|---|---|---|
| Codex | Version exacte du CLI et de l'image, mode d'authentification, commande, modèle demandé/retourné, JSONL, capacités de hooks et sous-agents réellement disponibles | `BLOCKED` pour une promesse d'observation native/live complète ; sessions réelles `NOT_RUN` |
| Claude Code | Même inventaire ; options stream-json, chargement effectif des hooks, permissions et modes pouvant les désactiver | Même état ; fixtures seules disponibles |
| Générique | Version du contrat, provenance et IDs explicites, limites de collecte | Tests historiques ; nouvelle qualification `NOT_RUN` |
| Jev | Activation ou désactivation explicite ; si activé, endpoint approuvé, version/contrat, modèle disponible, auth et échec conservatoire | `NOT_APPLICABLE` si désactivé ; sinon essais distants `BLOCKED` jusqu'aux prérequis puis `NOT_RUN` |

Pour Codex et Claude, les quatre couples plateforme/moteur annoncés doivent exécuter au moins une session réelle. Les cas avancés sont également rejoués sur les couples pour lesquels ils sont annoncés. Une dérogation doit préciser le contrat de support réduit avant publication ; le succès d'un parseur synthétique ne remplace jamais une preuve fournisseur. Voir [adaptateurs](../adapters.md) et [compatibilité](../compatibility.md).

## 4. Les douze gates

### G01 — Contrats communs et matrices de support local/cloud

**Bloquant :** figer la révision candidate, les contrats métier Rust partagés, les adaptateurs local/cloud, les quatre couples obligatoires, les modes rootless/rootful, les versions fournisseurs, les navigateurs, les limites annoncées et les options conditionnelles. Toute divergence d'autorisation, d'événement ou d'usage entre adaptateurs est explicite et testée. Chaque case possède un responsable, une procédure et une preuve. Compiler les artefacts de chaque architecture et vérifier leur identité ; un binaire Linux x86_64 n'est pas un binaire Mac ARM64. Une release locale qualifiée n'autorise pas à annoncer le profil cloud qualifié.

**Preuve :** manifeste de release, matrice remplie et rapprochement entre artefacts distribués et artefacts testés. **État : `BLOCKED`**, versions et environnements de qualification non arrêtés.

### G02 — Confinement et isolation des tenants

**Bloquant :** observer sur chaque moteur l'UID 1000, les capacités supprimées, `no-new-privileges`, la racine en lecture seule, le seccomp effectif, les limites PID/RAM/CPU/tmpfs, l'absence de socket/home/projet original et de namespaces hôte. Les tests négatifs doivent prouver l'échec d'accès et de dépassement, pas seulement la présence d'arguments. Qualifier les mappings de volumes et les secrets projetés en mode rootless et dans les VM Mac. Toute restriction ignorée par le runtime est un échec à résoudre.

Pour le profil multi-client, l'appartenance exclusive du worker est fixée par l'autorité de confiance, jamais par une étiquette fournie par le worker. Qualifier la séparation des volumes, identités et ressources de contrôle. Dans Kubernetes, namespaces, RBAC, admission et CNI doivent appliquer la frontière annoncée ; un namespace seul n'est pas une preuve d'isolation réseau. Aucun token d'administration du cluster ne va dans un worker.

**Preuve :** inspections expurgées, effets observés et scénario A1/A2/B1. Les tests d'arguments et de séparation tenant existent. **État : `NOT_RUN`** sur moteurs locaux ; **`BLOCKED`** pour l'adaptateur Kubernetes et sa qualification.

### G03 — Workspaces, copie et export par tenant

**Bloquant :** maintenir l'original inchangé, exclure les secrets et projections, refuser liens/traversal/fichiers spéciaux/dépassements, empêcher les substitutions concurrentes et publier un export neuf sans écrasement. Tester chemins Unicode, permissions, filesystem cible, import interrompu, disque plein et deux exports simultanés. Les limites d'import/export ne sont pas un quota de croissance du volume pendant l'exécution.

Le contrôleur doit dériver l'accès à un volume/export de l'identité authentifiée et de l'appartenance du worker. Deviner l'UUID d'une session de B ne doit permettre à A ni lecture, ni import, ni export, ni réutilisation d'un volume. La réaffectation éventuelle de capacité doit exclure tout résidu d'un ancien client ; les volumes de travail ne sont pas partagés implicitement entre tenants.

**Preuve :** empreintes avant/après de l'original, contenu de l'export, régressions de concurrence et essais des voies Linux/macOS. **État : `NOT_RUN`** pour la qualification complète ; les régressions logicielles historiques existent et ne sont pas des défauts encore ouverts.

### G04 — Autorisations d'action, identité tenant et quotas

**Bloquant :** refus déterministe non contournable par approbation ou Jev, hash lié à la demande exacte, consommation unique de l'approbation, aucun lancement avant audit acquitté, aucun fallback shell hôte. Provoquer panne d'audit avant chaque étape préalable et constater l'absence de worker/effect. Une commande de premier niveau autorisée ne prouve pas le contrôle de ses descendants. Toute promesse de contrôle avant outil impose un chemin d'exécution effectivement obligatoire et des tests de contournement, reliés à G07.

En mode multi-client, l'authentification détermine le tenant autorisé ; changer un champ JSON, un header ou un ID de worker ne change pas cette identité. Contrôler chaque lecture/écriture/décision/export et les droits de création/révocation de grants. Réserver les quotas atomiquement avant création, y compris pour des requêtes concurrentes, puis libérer une seule fois après l'état terminal prouvé. Une erreur d'audit ou de ledger ne donne pas une capacité gratuite ou invisible.

**Preuve :** demandes/décisions corrélées, absence de marqueur et état du moteur après refus, tentatives de spoof tenant, quotas du scénario à trois workers et concurrence logique. Auth/quota tenant sont implémentés et testés au niveau logiciel. **État de qualification complète : `NOT_RUN`** sur moteurs réels.

### G05 — Réseau et secrets

**Bloquant :** `none` sans sortie réseau sur chaque moteur ; `bridge` seulement sur demande explicite en `observe`, annoncé et audité comme sortie non filtrée. Vérifier séparément DNS, IPv4/IPv6 et accès à l'hôte/réseau local. Ne pas annoncer une allowlist egress ou un proxy filtrant absent. Qualifier les originaux `0600`, projections `0444` sous parent `0700`, montages par rôle, rotation, refus des liens et masquage des valeurs connues. Les clés fournisseur injectées restent lisibles par le code du worker et l'administrateur du moteur ; leur confidentialité vis-à-vis de cet agent n'est pas une garantie du mode bridge.

Le profil de communication multi-worker doit partir de **default-deny**, pour ingress et egress selon le contrat retenu. A1→A2 ne fonctionne qu'après grant ciblé, A1→B1 reste refusé, puis la révocation rétablit le refus. Tester ports/protocoles, DNS/IP directes, IPv6 et connexions déjà établies selon la sémantique publiée. Les communications applicatives cloud exigent aussi des ACL tenant et des identités authentifiées, avec TLS/mTLS selon G11 ; une règle réseau ne remplace pas l'autorisation applicative. Le `bridge` actuel n'implémente pas ce modèle.

Le mécanisme implémenté est un **relais de messages JSON authentifié**, avec grants par source/cible/channel et échéance optionnelle. Il n'ouvre pas un mesh TCP ni un filtrage egress générique. La fixture Compose sépare les trois réseaux de workers et les relie seulement à la gateway. **Preuve :** réseau `none`, grants/refus/révocation, montages et valeurs canaris absentes de l'audit. **État : `NOT_RUN`** sur moteurs locaux ; **`BLOCKED`** pour CNI et transport cloud.

### G06 — Audit et ledger d'usage durables

**Bloquant :** préserver chaque événement acquitté après redémarrage/crash dans le domaine de panne annoncé ; refuser les nouvelles actions si stockage plein, indisponible ou incohérent. Qualifier les bornes sur disque réel, WAL inclus. Définir et tester une sauvegarde cohérente, sa restauration sur instance vierge, une politique de rétention et la continuité des preuves entre archives. Un export JSONL ne remplace pas une restauration de base testée. Aucune suppression automatique ni troncature silencieuse n'est autorisée pour libérer la capacité.

Formaliser la version du schéma, les chemins de migration admis, le rejet d'un format incompatible et le rollback. Pour une première release, utiliser le format initial comme fixture et définir le comportement face à une version inconnue ; aucune migration inter-release n'est inventée. Le chaînage local détecte des modifications mais n'atteste pas l'absence de réécriture totale par un administrateur.

Pour le cloud facturé à l'usage, attribuer les événements au tenant vérifié et mesurer depuis une source de confiance. Les tokens déclarés par un CLI ne sont pas automatiquement une base de facture. Le ledger conserve unités, période, source et identité d'idempotence ; doublons, replay, livraison désordonnée et redémarrage ne doivent ni doubler ni réaffecter l'usage. Les corrections restent traçables. Définir le traitement d'un arrêt sans événement terminal et rapprocher la mesure du cycle réel du worker avant tout montant facturable. Voir [USAGE_METERING.md](USAGE_METERING.md).

La liaison persistante d'une base à son tenant, la compatibilité avec l'ancien audit local et l'idempotence des unités d'usage disposent de tests. Le runner rapporte un intervalle issu des timestamps du runtime, vérifié contre sa durée monotone de suivi ; les valeurs incohérentes restent inconnues et ne libèrent pas automatiquement la réservation. Les unités CPU/mémoire sont des allocations multipliées par cette durée, pas une mesure de CPU consommé ou de pic mémoire. **Preuve restante :** crash/disque plein, restauration, migration/rollback et rétention opérationnelle. **État : `BLOCKED`** pour cette qualification complète et la facturation financière, malgré le ledger non facturable implémenté. Sources : [audit](../../src/audit.rs), [tests de stockage tenant](../../tests/tenant_storage.rs), [tests du contrôle](../../tests/control_plane.rs).

### G07 — Codex/Claude et couverture d'observation

**Bloquant :** sessions réelles versionnées, sorties structurées reçues en cours de tâche, erreurs et interruption visibles, couverture explicitement incomplète si des événements manquent. Aujourd'hui `command.started` vient d'un inspect live, mais les lignes fournisseurs sont normalisées **après la fin** ; ce dernier point est un travail à réaliser pour une promesse de sorties fournisseurs live. Aucun hook fournisseur n'est actuellement activé par les parseurs. Sources : [runner](../../src/runner.rs), [adaptateurs](../adapters.md).

Pour chaque capacité de hook disponible et annoncée, vérifier chargement, ordre avant/après outil, refus, timeout, échec et contournements propres à la version. Prouver la hiérarchie par de vrais IDs parent/enfant ; ne pas convertir un ID d'outil en parent d'agent. Les données natives restent déclarées quand elles viennent du fournisseur. Une capacité réellement absente peut être `NOT_APPLICABLE` avec preuve de version et limite visible ; un adaptateur manquant pour une capacité annoncée est `BLOCKED`.

**Preuve :** traces réelles expurgées, chronologie de réception, scénarios d'outils/sous-agents et matrice par version. **État : `BLOCKED`**, collecte live et intégrations annoncées à compléter puis qualifier.

### G08 — API, dashboard et sécurité HTTP/SSE

**Bloquant :** rôles viewer/runner, refus cross-site, absence de token dans URL/stockages/logs, rendu XSS comme texte, CSP effective, routes de proxy limitées aux lectures, limites JSON/URI et export interrompu signalé. Vérifier SSE avec UTF-8 fragmenté, doublons, reconnexion par curseur, lecteurs lents, rotation de clé et saturation. Qualifier les vrais services Rust et le bundle distribué dans les navigateurs annoncés ; un serveur de fixtures ne couvre pas toute cette chaîne.

En multi-client, appliquer l'autorisation tenant côté serveur à chaque ressource, recherche, compteur, export et flux SSE. Un filtre UI n'est pas une frontière de sécurité. Tester IDs et curseurs d'un autre client, pagination et reconnexion après changement/révocation d'identité. Les credentials sont distincts par tenant/rôle/worker dans le registre actuel ; ils restent statiques et sans expiration automatique.

**Preuve :** suite API, frontend, E2E services réels et contrôles d'absence de fuite inter-tenant. L'API multi-client et ses tests logiciels sont présents. **État de qualification complète : `NOT_RUN`** pour la matrice réelle et l'endurance ; la sécurité de transport cloud relève aussi de G11.

### G09 — Cycle de vie, crash, reprise et nettoyage

**Bloquant :** timeout, Ctrl-C, arrêt du dashboard, crash/restart du gateway, perte du daemon et SIGKILL du broker. Réconcilier par IDs les conteneurs réellement créés et conserver les volumes utiles ; ne pas afficher une réussite quand le résultat est inconnu. Une reprise manuelle documentée peut suffire au profil single-host si elle est déterministe et testée. Aucun nettoyage global ou suppression d'un conteneur non créé par l'opération n'est acceptable.

`up` recrée gateway/dashboard et interrompt HTTP/SSE ; les jobs actifs peuvent échouer à la prochaine écriture d'audit. Cette maintenance doit être annoncée et testée. **Preuve :** chronologies, inspections, rapports de reprise et conservation des données. **État : `BLOCKED`** jusqu'à qualification d'un parcours opératoire de reprise, notamment après mort brutale du broker.

En cloud, la reprise du contrôleur doit réconcilier identité exclusive, quotas réservés, grants révoqués et usage persistant. Les retries de création/arrêt ne doivent créer ni worker supplémentaire, ni facture doublée. Aucune reprise ne réactive un grant expiré ou ne rattache un worker de B au tenant A.

### G10 — Dimensionnement, limites et charge

**Bloquant :** budgets mesurés pour les profils publiés : CPU/RAM/disque, ingestion, requêtes de projection, SSE, export et jobs simultanés. Tester les bornes et la dégradation contrôlée, y compris près des plafonds d'audit, avec fichiers volumineux et lecteurs lents. Les volumes de session conservés n'ont pas de quota disque global fourni par les arguments actuels ; définir et éprouver la maîtrise de leur croissance. `healthz` prouve la vie HTTP, pas la capacité d'audit ni la disponibilité du moteur.

**Preuve attendue pour la V1.** Réserver durablement 100 workers pour un tenant, refuser la 101e réservation et vérifier la conservation des quotas après redémarrage ainsi que l’indépendance du quota de B. La recette moteur utilise trois workers, A1 et A2 chez A et B1 chez B, et vérifie les exports et le retour des quotas à zéro.

**PASS au périmètre V1 sur `48f81d20`.** Les jobs SQLite et Docker attestent ces deux volets. Pour les budgets mesurés et les limites de chaque recette, consulter la grille détaillée. [Reçu CI](https://github.com/Needdoliprane/ai-guard/actions/runs/45344259411) · [Grille de qualification](docs/engineering/PRODUCTION_READINESS.md)

### G11 — Infrastructure, transport et PKI conditionnels

**Bloquant si activé :** qualification LAN/cloud en HTTPS, origine autorisée, routage, authentification, chaîne de confiance, SAN, expiration/renouvellement/révocation et mTLS pour les identités de service retenues. La PKI peut être indépendante d'un NAS. Aucun listener TLS/mTLS ni connecteur NAS n'est fourni par les options actuelles ; un reverse proxy seul ne qualifie pas le parcours complet. Codex doit obtenir le mandat précis avant toute connexion NAS, émission ou rotation. Aucun export de clé privée de CA et aucun contournement de vérification TLS ne sont permis.

**Preuve :** fiche approuvée et tests de [INFRASTRUCTURE_PKI.md](INFRASTRUCTURE_PKI.md). **État : `NOT_APPLICABLE` pour la PKI du profil exclusivement loopback** ; `BLOCKED` pour le transport et les identités du profil LAN/cloud tant que leurs paramètres et leur implémentation ne sont pas établis. Le HTTPS sortant fournisseur reste applicable à G05/G07 même sans PKI locale.

### G12 — Publication OSS, supply chain et preuves de release

**Bloquant.** Inclure le texte Apache-2.0 et les notices requises, puis vérifier les droits de redistribution des dépendances et des CLI optionnelles. Définir le support, le signalement de vulnérabilités et la maintenance. Figer les dépendances, versions CLI, digests d'images et entrées de build. Produire un SBOM des artefacts construits, une revue de vulnérabilités et une provenance vérifiable. Vérifier la reconstruction, la CI et la signature sur la candidate.

**Preuve attendue.** Dossier légal et technique, artefacts identifiés, résultats G01–G11, anomalies clôturées, procédure d'installation, mise à jour et rollback vérifiée et décision signée par les responsables nommés. **État `BLOCKED`.** Le mainteneur a retenu Apache-2.0. La chaîne de livraison, les notices, les droits de redistribution et le canal privé restent à vérifier. [Licence](LICENSE) · [Checklist de publication](docs/engineering/RELEASE_CHECKLIST.md)

Le choix de licence doit être cohérent avec le produit OSS et son exploitation cloud payante sans supposer leur incompatibilité ni choisir une licence à la place du responsable. Avant facturation réelle, approuver les unités d'usage, l'application des tarifs versionnés, les corrections, le rapprochement et les obligations commerciales pertinentes. Aucun paiement ni connecteur de paiement n'est autorisé par cette documentation.

## 5. Conditions de clôture

Chaque preuve précise la révision, la date UTC, le responsable, les versions/digests, le mode et les options, la commande ou procédure exacte, l'attendu, le constat, le statut et les limites. Les logs sont expurgés ; les secrets et clés privées ne sont jamais joints. Un défaut corrigé est clôturé par une régression discriminante et un rejeu des gates affectés. Les autres lignes restent valides seulement si leur périmètre n'a pas changé.

Le responsable de release ne peut pas remplacer une preuve manquante par le nombre total de tests. La [checklist](RELEASE_CHECKLIST.md) constitue le point de décision ; la baseline historique reste conservée séparément.
