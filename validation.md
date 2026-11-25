# ai-guard — rapport de validation

Date : **29 octobre 2026**. Périmètre : modèle multi-client, relais entre workers, relevé d'usage, interface, instructions des agents et renommage en **ai-guard**.

## Conclusion et portée

Les vérifications logicielles de cette révision passent : **132 tests Rust, 24 tests frontend, analyse statique, compilation optimisée et parcours Chromium**. La cible reste un produit open source utilisable en production. Ce rapport ne qualifie pas encore une release de production : les gates applicables de [PRODUCTION_READINESS.md](docs/engineering/PRODUCTION_READINESS.md) doivent être fermés avec leurs preuves.

**Docker et Podman ne sont pas disponibles dans l'environnement de réalisation. Les images et la recette à trois workers n'ont pas été exécutées sur un moteur réel.** Deux tests qui exigent ce moteur sont explicitement ignorés dans la suite ordinaire. Aucun cluster Kubernetes, compte fournisseur IA, service de paiement ou accès à la PKI du NAS n'a été utilisé.

Les preuves distinguent les contrats logiciels, les processus Rust réellement lancés, les fixtures navigateur et les essais runtime restant à mener. Un moteur instrumenté sert aux régressions de protocole et de cycle de vie ; il ne démontre pas l'isolation du système hôte.

## Environnement

| Élément | Version ou état constaté |
|---|---|
| Plateforme | Linux x86_64 |
| Rust | 1.99.1, figé dans `rust-toolchain.toml` |
| Dépendances Rust | `Cargo.lock` livré ; dernière passe hors ligne sur cache préparé |
| Node | 24.22.1 |
| TypeScript / Vite | 5.9.3 / 7.3.1 ; `front/package-lock.json` livré |
| Navigateur | Chromium headless via Playwright |
| Docker / Podman / Kubernetes | Non exécutés ; aucune qualification revendiquée |
| Échanges de test | HTTP loopback ; aucune requête authentifiée vers un fournisseur ou le NAS |

`--offline` signifie que les dépendances étaient déjà disponibles. Une machine vierge doit d'abord les télécharger. Les builds, Clippy et la passe finale des tests Rust utilisent `CARGO_INCREMENTAL=0`.

## Résultats exécutés sur le projet renommé

| Contrôle | Résultat | Portée |
|---|---|---|
| `cargo fmt --all -- --check` | PASS | Format de toutes les cibles Rust. |
| `cargo clippy --locked --offline --all-targets -- -D warnings` | PASS, aucun avertissement | Analyse statique Rust. |
| `cargo test --locked --offline` | **132 PASS, 0 FAIL, 2 ignorés** | Bibliothèque, API, contrôle multi-client, audit, runtime instrumenté et régressions. |
| `cargo build --release --locked --offline` | PASS | Binaire optimisé `ai-guard` pour la plateforme de validation. |
| `npm test`, dans `front/` | **24 PASS, 0 FAIL** | API, modèle, SSE, contexte client, workers, grants et usage. |
| `npm run build`, dans `front/` | PASS | Vérification TypeScript et bundle frontend. |
| `npm run test:browser`, avec Chromium disponible | PASS | Parcours navigateur sur collecteur de fixtures contrôlé. |
| `live_services` | PASS, inclus dans les 132 | Gateway et dashboard Rust réels, SQLite, auth déléguée et redémarrage. |
| Revue indépendante ciblée | Correctifs vérifiés | Droits/tenant, credentials, audit des refus, deadline et conservation des preuves runtime. |
| Kit d'instructions | PASS documentaire | Liens, chemins, noms et copies des quatre skills ; référence historique `DOCUMENTATION_VALIDATION.md`. |

Le bundle comprend environ **49,15 ko de JavaScript, 17,56 ko gzip**, et **31,39 ko de CSS, 6,95 ko gzip**. Ces tailles ne mesurent pas la RAM ou le coût d'exploitation.

### Détail des tests Rust

| Cible | Réussis | Ignorés |
|---|---:|---:|
| Bibliothèque | 31 | 0 |
| `api_security` | 4 | 0 |
| `audit_load` | 1 | 0 |
| `audit_security` | 3 | 0 |
| `control_plane` | 16 | 0 |
| `declared_agents` | 12 | 0 |
| `jev_contract` | 2 | 0 |
| `live_services` | 1 | 0 |
| `local_cloud_smoke` | 2 | 1 |
| `name_compatibility` | 2 | 0 |
| `review_regressions` | 5 | 0 |
| `runtime_authorization` | 4 | 0 |
| `runtime_lifecycle` | 4 | 0 |
| `runtime_local_cloud` | 2 | 0 |
| `runtime_operations` | 8 | 0 |
| `runtime_races` | 3 | 0 |
| `runtime_security` | 12 | 0 |
| `runtime_smoke` | 0 | 1 |
| `runtime_tenant` | 8 | 0 |
| `tenant_api` | 5 | 0 |
| `tenant_review` | 3 | 0 |
| `tenant_storage` | 4 | 0 |
| **Total** | **132** | **2** |

`main` et les doc-tests n'ajoutent aucun cas. Les deux tests réussis de `local_cloud_smoke` vérifient les préconditions du chemin de configuration ; ils ne lancent pas de conteneur. `audit_load` soumet 1 000 événements depuis huit producteurs synthétiques concurrents : ce n'est ni un benchmark de production, ni un essai de 100 workers.

## Comportements vérifiés

### Portée client et rôles

Les tests vérifient le choix du client par le credential authentifié, la base distincte et liée durablement à ce client, le refus de réattribution d'une base existante et la compatibilité des anciens événements locaux. Ils couvrent le filtrage des événements, projections, flux SSE, exports et usages, ainsi que le refus des identités worker sur les routes de contrôle ou de consultation générale.

Les refus d'un rôle ou d'une portée connus sont audités dans le bon client. Des credentials invalides pour un header HTTP, notamment NUL, DEL et Unicode hors ASCII, sont refusés à la configuration. Ces régressions ont échoué avant correction puis passé.

### Contrôle, relais et usage

Les réservations et quotas sont persistants et atomiques. Les transitions et leurs traces obligatoires partagent une transaction ; un échec d'audit n'accepte pas silencieusement la mutation. Les tests couvrent reprises identiques, conflits de contenu, grants dirigés, expiration et révocation, ainsi que le refus de relire un message devenu interdit.

Le relais vérifie les identités et états des workers, sans accepter un tenant ou une source choisis dans le corps. Un nouveau grant ne réactive pas les messages d'un ancien grant révoqué. La mesure terminale ne s'ajoute qu'une fois, y compris après redémarrage. Le relevé reste **`billable=false`** : il comptabilise de l'allocation CPU/RAM multipliée par une durée rapportée par le contrôle de confiance, pas un prix ni une consommation CPU réellement observée.

### Processus Rust réels

`live_services` lance le gateway et le dashboard par la CLI du binaire renommé, avec la configuration multi-client et des bases temporaires. Il vérifie la délégation d'authentification, le contexte A/B, le proxy de lecture, le refus des mutations et du relais par ce proxy, les envois idempotents, l'intégrité et la persistance de l'usage après redémarrage du gateway.

Les identités de workers y sont synthétiques et aucun moteur n'est lancé. Le fichier HTML servi est une fixture minimale ; le parcours de l'interface compilée est testé séparément dans Chromium. Ce test ne prouve donc pas à lui seul un parcours navigateur de bout en bout avec trois conteneurs.

### Cycle de vie et réconciliation

Les essais instrumentés et la revue indépendante confirment que le timeout reste actif pendant l'inspection et le callback de démarrage. Avec un délai d'une seconde, le harnais indépendant observe la demande d'arrêt à environ 1 002 ms pour le callback lent et 1 003 ms pour l'inspection lente ; ces chiffres décrivent le harnais, pas une latence garantie sur Docker/Podman.

Après une erreur de callback, le conteneur créé reste inspectable après la demande d'arrêt ; les timestamps ne sont pas détruits par un nettoyage prématuré. Le runner conserve aussi un conteneur terminé jusqu'à la persistance de l'observation terminale, de sa transition et de l'acquittement. Une durée ou un état ambigu conserve un besoin de réconciliation, sans usage nul fabriqué ni libération indue de quota.

Les régressions existantes de création concurrente, d'export par descripteur, de non-écrasement, de traversée de chemins, de volumes et de masquage des secrets restent vertes.

### Frontend

Le parcours Chromium vérifie les écrans principal et mobile, l'authentification, la recherche, la période, la pagination, les exports, les détails, les thèmes, la navigation au clavier, l'échappement HTML, les états vide/erreur, la reconnexion SSE et son curseur. Le jeton reste absent des URL et des stockages persistants du navigateur.

Les nouveaux scénarios montrent A1/A2 sans B1 dans le contexte A, quotas, grants expirés/révoqués et usage explicitement non facturable. Un backend ancien sans ces routes affiche leur indisponibilité. Une métrique absente ou trop grande pour être représentée exactement en JavaScript reste inconnue ; elle ne devient pas zéro ou un entier arrondi présenté comme exact. Le frontend interrompt une session nommée si un événement porte une portée client incompatible.

### Nom et instructions

Le binaire, les images, la configuration, les variables, le frontend et les skills portent le nom `ai-guard`. Les tests de compatibilité valident le type de télémétrie courant et son alias historique, ainsi que l'exclusion des anciens et nouveaux répertoires privés. Les domaines de hash persistants sont conservés pour ne pas invalider les historiques ; référence historique `RENAMING.md`.

Les règles et les quatre skills ont été relus avec des scénarios de fuite entre clients, moteur absent, qualification prématurée et PKI sans paramètres. Leur découverte automatique n'a pas été testée dans les sessions Claude Code/Codex du mainteneur. Ces instructions sont des consignes de développement, pas une frontière d'isolation.

## Essais et travaux restant à qualifier

| Périmètre | État et preuve attendue |
|---|---|
| Images, Docker et Podman réels | NOT_RUN ; exécuter séparément les recettes sur les versions ciblées. |
| Trois workers A1/A2/B1 | NOT_RUN sur moteur ; recette Rust livrée dans `local_cloud_smoke`. Réservation avant création, volumes de preuve conservés. |
| Isolation système et réseau | Observer UID, capabilities, seccomp, cgroups, montages, trafic et limites sur chaque moteur. Une configuration ne suffit pas à prouver son application. |
| Pannes daemon/hôte et reprise | Qualifier arrêt brutal, observation ambiguë et réconciliation sur moteur ; le journal logiciel ne prouve pas tous ces scénarios. |
| Kubernetes | Adaptateur d'exécution non livré ; `kubernetes_runtime=false`. `runtime=kubernetes` dans le registre reste une métadonnée. |
| Exposition cloud | Identité dynamique des workloads, TLS/mTLS, accès distant, politiques réseau et exploitation à compléter et qualifier. `deployment_mode=cloud` ne les déploie pas. |
| Comptage financier | Ledger facturable, tarifs, arrondis, réconciliation de factures et paiement non livrés ; aucun tarif inventé. |
| 100 workers/client | Objectif non mesuré ; trois workers et des requêtes synthétiques n'en sont pas une preuve. |
| Sauvegarde, restauration, rétention, endurance | Exercices sur données et versions représentatives requis ; pas de RPO/RTO ou de SLO prouvé. |
| Fournisseurs Codex/Claude/Jev | Contrats/fixtures seulement ; aucune session fournisseur authentifiée ou décision Jev distante exécutée. |
| Portabilité, accessibilité, publication | macOS/ARM64/WSL/SELinux, audit WCAG, licence et canal de signalement à décider/qualifier selon le périmètre annoncé. |

Les commandes moteur sont dans [LOCAL_CLOUD_TESTS.md](docs/engineering/LOCAL_CLOUD_TESTS.md), [TEST_STRATEGY.md](docs/engineering/TEST_STRATEGY.md) et [acceptance.md](docs/acceptance.md). Les paramètres à obtenir avant une intégration réelle de la PKI sont dans [INFRASTRUCTURE_PKI.md](docs/engineering/INFRASTRUCTURE_PKI.md). Aucun test ignoré n'est compté comme réussite.

## Livraison

L'archive contient le code Rust/TypeScript, tests et fixtures identifiés, lockfiles, schéma, politiques, Dockerfiles, recettes Compose, bundle frontend et documentation, avec `CLAUDE.md`, `AGENTS.md` et les deux jeux de skills de projet.

Les dépendances téléchargées, `target/`, profils navigateur, captures de QA, secrets, bases temporaires et configurations privées sont exclus. Aucun binaire lié à la seule machine de validation n'est présenté comme une distribution universelle. Le prompt complet et le kit d'instructions sont aussi proposés séparément pour faciliter la reprise du travail.
