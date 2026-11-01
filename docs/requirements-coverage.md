# Couverture du cahier des charges

Ce document relie les **13 sections** et les **18 scénarios** du besoin original (`docs/requirements-original.txt`) au produit demandé. Les consignes récentes de `AGENTS.md` et `PROMPT_PRODUCTION.md` priment sur son ancien découpage MVP/V2. Le résultat attendu est un produit open source complet, soigné, local-first et cloud ; une analyse exhaustive ou une base exécutable ne vaut pas conformité.

Les chemins partent de la racine du dépôt afin que cette copie et `requirements-coverage.md` restent identiques. Les critères sont dans `docs/acceptance.md` et `docs/engineering/PRODUCTION_READINESS.md`. Le code présent et les écarts sont établis dans `docs/engineering/CURRENT_STATE.md` ; les résultats exécutés du checkout sont dans `docs/engineering/VALIDATION.md`. `docs/validation.md` et `validation.md` conservent les preuves historiques, sans qualifier le checkout actuel.

## Lecture des statuts et de la couverture

Présence de code, preuve logicielle et qualification réelle sont trois informations distinctes. Chaque lot doit relier exigence, code, tests, revue, exploitation et écarts. Les tableaux ci-dessous sont des points de contrôle, pas un tableau de succès implicites.

| Statut de preuve | Sens |
|---|---|
| `PASS` | Résultat attendu constaté pour une commande, une version et un environnement nommés. |
| `FAIL` | Écart observé ; conserver la reproduction et la régression pertinente. |
| `BLOCKED` | Prérequis externe manquant précisément identifié. |
| `NOT_RUN` | Essai non exécuté, y compris un test ignoré par défaut. |
| `NOT_APPLICABLE` | Non-applicabilité justifiée pour la preuve avant essai ; ne réduit pas le mandat global. |

Une fixture fournisseur ne prouve pas une session authentifiée ; des arguments runtime ne prouvent pas les restrictions effectives ; une capture navigateur ne prouve pas l'exécution d'un worker. Une fonction absente reste un écart obligatoire lorsqu'elle répond à une exigence. Les extensions facultatives sont justifiées séparément, sans étiquette « V2 » qui reporterait la sûreté ou la maintenance.

## Contraintes transversales

| Contrainte | Contrat et point de contrôle |
|---|---|
| Rust hors frontend | `src/`, tests et nouveaux automatismes applicatifs en Rust ; frontend existant en TypeScript. Configurations et commandes d'utilisation ne portent pas de logique métier. |
| Dashboard conteneurisé et séparé | Fichiers statiques de `front/` servis par Rust ; gateway, dashboard et workers distincts ; broker dans le contrôle de confiance. Aucun serveur Node de production, socket moteur ou privilège hôte dans les conteneurs applicatifs. |
| Original préservé | Volume de session alimenté par copie filtrée ; export dans une destination neuve ; refus des liens, traversées et substitutions. Pas de réécriture automatique de l'original. |
| Identité et données multi-client | Identité authentifiée, une base SQLite durablement liée à chaque client, aucun tenant choisi par champ libre, rôle worker limité au relais autorisé. |
| Identité humaine et configuration | `docs/sso.md` : identité locale nominative hors ligne et SSO OIDC public PKCE Web/CLI, permissions/memberships vérifiés côté Rust, configuration par axes indépendants et découverte Keycloak explicitement activée. Ces parcours restent à implémenter et qualifier ; les clés partagées ne les livrent pas. |
| Échanges et état durables | Grants dirigés/révocables, refus inter-client ; quotas, transitions, messages et usage avec transactions/audit ; idempotence et conflit de contenu. |
| Capacité et recette | 0 à 100 workers actifs par client ; recette standard A1/A2 chez A et B1 chez B ; charge 100 distincte. Docker/Podman × Linux x86_64/macOS ARM64 et Kubernetes exigent leurs preuves propres. |
| Vérité des données | `observed`, `declared`, `inferred` et couverture Native/Observed/Generic distincts ; inconnue conservée ; allocation CPU/RAM `billable=false`, sans prix ni facture. |
| Qualité et maintien | Parcours complets, états incomplets/erreurs, accessibilité, lisibilité sous charge, documentation, migrations, sauvegarde/restauration, reprise, contribution et gates de release. |

Références : `docs/engineering/ARCHITECTURE_CONTRACT.md`, `docs/engineering/CODING_RULES.md`, `docs/engineering/CLOUD_TENANCY.md`, `docs/engineering/LOCAL_CLOUD_TESTS.md`, `docs/engineering/USAGE_METERING.md`.

## Correspondance des 13 sections

| # | Exigence | Points de contrôle dans le dépôt | Compléments obligatoires à ne pas reporter |
|---|---|---|---|
| **1** | Objectifs de supervision | Vues décrites dans `docs/ui.md`, projections API/audit, `front/src/main.ts` | Mesures effectives CPU/RAM/réseau/disque et statuts cohérents ; limites et données inconnues visibles. Une limite configurée n'est pas une consommation. |
| **2** | Compatibilité multi-agents | `docs/compatibility.md`, `docs/adapters.md`, `docs/sources.md`, `src/adapters.rs` | Versions et comptes réels ; connecteurs détaillés lorsque leurs interfaces le permettent. Pas de niveau Native déduit de la marque ou de sa documentation seule. |
| **3** | Tableau de bord principal | `GET /api/overview`, API/SSE, filtres et exports, `front/src/main.ts` | Santé moteurs, durée et ressources mesurées, fraîcheur/portée explicites, vues workers/relais/usage cohérentes avec les permissions. |
| **4** | Hiérarchie des agents | `src/declared_agents.rs`, `tests/declared_agents.rs`, types frontend et contrat générique | Sous-agents fournisseurs lorsqu'ils sont réellement exposés ; parents et attribution explicites. Cycles, orphelins, contexte de session et provenance par champ. |
| **5** | Timeline des actions | `src/model.rs`, `src/runner.rs`, `schemas/event.schema.json`, API et vues | Demande/décision/soumission/démarrage/effet distincts, rétention bornée, étapes durables. Parsing après fin non assimilé à un contrôle live des outils. |
| **6** | Sécurité / Jev | `src/policy.rs`, `src/jev.rs`, `src/runner.rs`, `docs/security.md` | Refus avec non-effet, panne d'audit, canaux de médiation et contournements. Jev optionnel ne remplace pas la restriction technique ; sa validation réelle exige compte/modèle autorisés. |
| **7** | Filesystem / Git | `src/workspace.rs`, manifestes avant/après, `tests/runtime_security.rs`, vue Fichiers | Diffs Git demandés et attribution justifiée, sans monter l'original ; import/export sûrs, erreurs et reprise. Aucun remplacement automatique du dépôt source. |
| **8** | Gestion des sandboxes | `src/operations.rs`, `src/runtime.rs`, CLI et `docs/operations.md` | État/santé, inventaire et limites effectivement inspectés ; registre, quotas et réconciliation. Les vues utilisateur et administration unifiée de `docs/ui.md` sont obligatoires, selon les permissions Rust du seul périmètre autorisé ; elles restent à implémenter et qualifier. |
| **9** | Architecture technique | `docs/architecture.md`, `docs/design.md`, `Cargo.toml`, `front/package.json`, déploiements | Conserver les frontières validées, base par client et partage local/cloud. API/schéma versionnés et Kubernetes effectif ; aucune métadonnée n'est un lancement de pod. |
| **10** | Interface et audit sûrs | API/authentification, `src/audit.rs`, `src/redaction.rs`, DOM et `docs/security.md` | Durabilité atomique, isolation, saturation, rétention/rotation configurables, sauvegarde/restauration, transport cloud et revue de livraison. Intégrité locale non assimilée à une signature/WORM. |
| **11** | Expérience utilisateur | CLI, `docs/ui.md`, tests frontend/navigateur et `README.md` | Installation et diagnostic utilisables, erreurs/empty/stale/auth/reprise, clavier/contrastes/petits écrans, performances et données réelles. Aucun bouton factice ni compte cloud requis en local. |
| **12** | Tests de validation | `tests/`, `front/tests/`, `docs/acceptance.md`, `docs/engineering/TEST_STRATEGY.md` | Exécuter les scénarios réels, la recette trois workers, pannes/reprises et capacité 100 ; conserver environnements, versions, non-effets et statuts exacts. |
| **13** | Livrables et intégration | Architecture, modèle, API, compatibilité, interface, risques, acceptation et ce document | Le dossier accompagne l'implémentation et ses preuves. Remplacer le découpage historique MVP/V2 par des lots d'un produit complet ; contribution, licence et maintenance font partie de l'acceptation. |

L'indisponibilité réelle d'une capacité fournisseur reste visible, documentée et soumise aux critères de couverture annoncés. Elle ne justifie ni donnée inventée ni promesse d'interception universelle. OTLP/Prometheus et microVM peuvent être des extensions facultatives selon le besoin et les gates ; l'administration unifiée demandée ne l'est pas. Un changement structurel exige une décision motivée préalable.

## Traçabilité des 18 scénarios

Les procédures et résultats attendus complets sont dans `docs/acceptance.md`. Les références ci-dessous indiquent où chercher une preuve ; elles ne déclarent pas les suites exécutées ou réussies.

| # | Scénario | Références et preuve nécessaire |
|---|---|---|
| **1** | Sandbox Codex | Parcours fournisseur du README et fixtures `tests/adapters_fixtures/codex.jsonl` ; compléter par moteur/version/compte réels. |
| **2** | Sandbox Claude | Parcours fournisseur du README et fixtures `tests/adapters_fixtures/claude.jsonl` ; même preuve réelle distincte. |
| **3** | Tâche simple avec chacun | Session, audit, comparaison avant/après et export neuf ; original intact constaté pour chaque fournisseur. |
| **4** | Création de sous-agents | Contrat générique et hooks effectivement supportés ; déclarations explicites et essai versionné du fournisseur. |
| **5** | Arborescence | `tests/declared_agents.rs` et tests navigateur ; parents, cycles, orphelins et collisions de contexte. |
| **6** | Commande autorisée | `tests/runtime_smoke.rs` explicitement lancé et inspection de démarrage/UID sur moteur. |
| **7** | Commande interdite | `tests/runtime_authorization.rs` et politique ; inspection de l'absence de worker sur moteur. |
| **8** | Décision Jev | `tests/jev_contract.rs` pour le contrat local ; essai distant compte/modèle autorisés pour le fournisseur. |
| **9** | Aucun effet après refus | Régression d'autorisation et marqueur absent après inspection ; inclure panne d'audit avant effet. |
| **10** | Audit par étape | `tests/api_security.rs`, `tests/audit_security.rs`, `verify-audit` et session réelle corrélée. |
| **11** | Fichiers | `tests/runtime_security.rs`, volumes réels, export, refus de liens/traversées/substitution et diffs demandés. |
| **12** | Arrêt brutal | Timeout/Ctrl+C et pannes broker/moteur séparées ; arrêt ciblé, inconnue et reprise réconciliée. |
| **13** | Redémarrage dashboard | Arrêt du seul dashboard pendant session ; contrôle maintenu et reconnexion au bon curseur. |
| **14** | Persistance | Réouverture SQLite, redémarrage gateway et restauration ; identités, événements et intégrité conservés. |
| **15** | Secrets | `src/redaction.rs`, régressions et tests navigateur ; absence des secrets reconnus dans DB/flux/export, portée du masquage explicite. |
| **16** | Charge | `tests/audit_load.rs` pour le synthétique ; vrais workers, lecteurs lents et charge 100 distincts, mesures de capacité et bornes. |
| **17** | Docker / Podman | `runtime_smoke` explicite selon opérations ; quatre rapports moteur × plateforme et scénarios négatifs. |
| **18** | Front/API | `tests/api_security.rs`, tests d'identité/base A/B, `front/tests/` ; permissions, XSS, isolation, états et accessibilité. |

La recette trois workers et le cloud ajoutent les contrats récents aux scénarios d'origine : grants A1→A2, refus A1→B1, révocation, rôle worker, quotas, audit/usage idempotents, redémarrage, arrêt brutal, secrets/privilèges du contrôle et affichage cohérent. Voir `docs/engineering/LOCAL_CLOUD_TESTS.md` et `docs/engineering/TEST_STRATEGY.md`.

## Livrables et clôture

| Livrable | Référence et critère |
|---|---|
| Architecture intégrée et comparaison des stacks | `docs/architecture.md`, `docs/design.md`, contrat engineering ; choix imposés préservés. |
| Agents, sous-agents, événements et API | `docs/data-model.md`, `docs/api.md`, schéma et contrats ; identité/provenance explicites et versions compatibles. |
| Compatibilité des fournisseurs | `docs/compatibility.md`, `docs/adapters.md`, `docs/sources.md` ; capacités réelles, tests versionnés et limites. |
| Pages principales et parcours | `docs/ui.md`, `front/src/main.ts`, `docs/acceptance.md` P01–P03 ; vues utilisables, aucun jeu de données de production fictif. |
| Implémentation par lots | `docs/engineering/IMPLEMENTATION_PLAN.md` ; périmètre, propriétaires, risques, tests et acceptation avant édition. |
| Tests, risques et preuves | `docs/engineering/TEST_STRATEGY.md`, `docs/security.md`, `docs/engineering/VALIDATION.md` ; résultat frais et périmètre précis. |
| Exploitation et maintenance | `docs/operations.md`, gates et checklist release ; migrations, sauvegarde/restauration, reprise, contribution et support documentés. |
| Publication open source | `LICENSE` Apache-2.0 ; canal privé de vulnérabilités à décider avant publication, sans inventer de contact. |

La clôture d'un lot exige comportement réel, erreurs traitées, tests pertinents exécutés, documentation actualisée et revue indépendante traitée selon son risque. La qualification d'une release exige tous les gates applicables et la fermeture des écarts bloquants. Réduire une preuve à un périmètre local ne retire pas cloud/Kubernetes/100 workers du mandat produit.
