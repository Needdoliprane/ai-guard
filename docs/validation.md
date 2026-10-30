# Rapport de validation — 29 octobre 2026

## Conclusion et périmètre

Le MVP compile et ses suites logicielles passent. Les contrôles ont porté sur le code Rust, l'interface compilée, le navigateur, des échanges HTTP réels en loopback et des fixtures explicitement identifiées.

**Aucun moteur Docker/Podman n'était disponible dans l'environnement de réalisation. Les images n'ont donc pas été construites ni exécutées avec un moteur réel. Aucun compte Codex, Claude ou Jev n'a été utilisé.** La qualification externe définie dans [acceptance.md](acceptance.md) reste nécessaire. Cette distinction s'applique aussi aux restrictions réseau, capacités, seccomp, montages, cgroups et permissions de secrets du déploiement.

## Environnement logiciel

| Élément | Version ou état constaté |
|---|---|
| Plateforme des tests | Linux x86_64 |
| Rust | 1.99.1 ; toolchain officielle, installation vérifiée par SHA-256 |
| Cargo | Toolchain Rust 1.99.1 ; dépendances figées par `Cargo.lock` |
| Node | 24.22.1 |
| TypeScript / Vite | 5.9.3 / 7.3.1, `package-lock.json` livré |
| Navigateur | Chromium headless via Playwright |
| Docker / Podman | Absents ; aucun résultat moteur revendiqué |
| Réseau des tests | Loopback pour les contrats HTTP ; aucune requête authentifiée vers un fournisseur |

## Résultats exécutés

| Contrôle | Résultat | Ce qu'il établit |
|---|---|---|
| `cargo fmt --all` | Réussi | Format Rust appliqué. |
| `cargo clippy --locked --offline --all-targets -- -D warnings` | Réussi, aucun avertissement | Analyse statique de toutes les cibles Rust. |
| `CARGO_INCREMENTAL=0 cargo test --locked --offline` | **85 tests réussis, 0 échec, 1 ignoré** | Contrats, politiques, stockage, API, adaptateurs, chemins, archives, secrets et régressions de concurrence. |
| `cargo build --release --locked --offline` | Réussi | Binaire Rust optimisé compilé pour l'environnement Linux de validation. |
| `npm test` | **17 tests réussis, 0 échec** | Client API, filtres, dates, export, hiérarchie, valeurs inconnues et parseur SSE. |
| `npm run build` | Réussi | Vérification TypeScript et bundle de production. |
| `npm run test:browser` avec Chromium disponible | Réussi | Parcours de l'interface sur collecteur de fixtures dédié. |
| Navigateur et services Rust réels | Réussi | Authentification, routes de lecture, proxy, SSE, XSS, export et intégrité sur une base QA temporaire. |
| Broker Rust et gateway réel, runtime instrumenté | Refus avant tout appel du runtime | Une commande directement interdite n'invoque pas le faux moteur et produit une décision dans l'audit. |
| Lecture statique des deux fichiers Compose | Réussie | Structure, services séparés, ports localhost, réseau interne, droits et montages configurés. |

L'option `--offline` ci-dessus signifie que les dépendances étaient déjà présentes lors de cette passe. La passe finale Rust, y compris Clippy et le build release, a été exécutée avec `CARGO_INCREMENTAL=0`. Un premier `cargo build --locked` ou `npm ci` sur une machine vierge nécessite leur téléchargement. Le bundle contient environ 38,20 ko de JavaScript (14,24 ko gzip) et 28,79 ko de CSS (6,49 ko gzip) ; ces tailles ne mesurent pas la RAM des services ni celle du navigateur.

### Détail de la suite Rust

| Cible | Tests réussis |
|---|---:|
| Tests unitaires de la bibliothèque | 31 |
| `api_security` | 4 |
| `audit_load` | 1 |
| `audit_security` | 3 |
| `declared_agents` | 12 |
| `jev_contract` | 2 |
| `review_regressions` | 5 |
| `runtime_authorization` | 4 |
| `runtime_operations` | 8 |
| `runtime_races` | 3 |
| `runtime_security` | 12 |
| `runtime_smoke` | 0 ; **1 explicitement ignoré** |

Le test `audit_load` soumet **1 000 événements depuis 8 producteurs concurrents synthétiques**. Il contrôle ordre, intégrité et filtres. Il ne simule pas huit fournisseurs IA authentifiés et ne constitue pas une mesure représentative de débit ou de consommation en production.

### Parcours navigateur

Le parcours sur fixtures vérifie le refus puis l'acceptation d'un jeton, la recherche, la période figée, la pagination, l'export appliquant tous les filtres sans la pagination, les détails de commande/JSON, la hiérarchie déclarée, les thèmes clair/sombre et l'affichage mobile à 390 px. Il exerce aussi l'état vide, l'erreur API, le clavier, les textes HTML malveillants, la reconnexion SSE et la conservation du curseur.

Le jeton est vérifié absent des URL, du `localStorage` et du `sessionStorage`. Les tests de flux couvrent les coupures UTF-8, les limites de mémoire, les doublons et les événements incomplets.

Un second parcours utilise le **gateway et le serveur dashboard Rust réellement lancés**, avec le build frontend et une base temporaire. Il a constaté :

- accès anonyme refusé ; jeton viewer accepté ; ingestion par viewer refusée ;
- routes de lecture disponibles et mutations refusées par le proxy du dashboard, sans ajout d'événement ;
- en-têtes CSP, `nosniff`, `DENY` et `no-store` présents ;
- événement QA effectivement reçu par SSE et affiché ;
- chaîne HTML malveillante affichée comme texte, sans exécution ;
- JSONL téléchargé et chaîne d'intégrité valide ;
- aucune exception JavaScript ni violation CSP pendant ce parcours.

Ce parcours ne lance aucun worker. Les captures, clés temporaires, fichiers de base de données et exports de QA sont exclus de l'archive livrée. La dernière évolution des filtres de période/export a été revalidée par les tests unitaires et le parcours navigateur sur fixtures ; elle ne transforme pas ce résultat en qualification Docker/Podman.

### Contrat Jev

Les tests HTTP emploient un serveur local contrôlé et les schémas de l'API officielle. Ils vérifient la requête de choix, les décisions, le veto du moteur et l'échec conservatoire en cas d'indisponibilité ou de réponse inexploitable. Ils ne mesurent ni la qualité de jugement ni la disponibilité du modèle distant. Les sources du contrat sont listées dans [sources.md](sources.md).

## Défauts corrigés et contrôles conservés

La revue a donné lieu à des corrections vérifiées, notamment :

- masquage de secrets présents dans des commandes textuelles, pas seulement dans les champs structurés ;
- impossibilité pour des données déclarées de se promouvoir en observation ou d'augmenter les comptes observés ;
- blocage durable des écritures après détection d'une corruption du journal, y compris avec accès concurrents ;
- normalisation UTC des dates source avant le hash ;
- calcul des demandes en attente à partir de leur dernière décision ;
- originaux de secrets privés en `0600`, projections de montage séparées en `0444` sous un parent `0700`, rafraîchissement atomique et exclusion de ces projections de la copie du projet ;
- refus d'écraser une destination d'export apparue entre la validation et la publication ;
- absence de suppression après un `create` de conteneur non confirmé, et noms d'auxiliaires distincts pour deux opérations d'une même session ;
- import/export refusant liens, fichiers spéciaux, traversées de chemins et dépassements ;
- extraction, publication et nettoyage d'export liés à des descripteurs de répertoire, afin qu'une substitution du parent ne redirige pas les écritures dans l'original.

Pour les deux problèmes de concurrence runtime/export, un harnais indépendant a reproduit l'erreur avant correction puis confirmé : création en échec sans suppression forcée du nom préexistant ; parent d'export substitué sans écriture dans l'original. Une seconde revue ciblée n'a pas identifié de défaut matériel restant dans ce périmètre. Cela ne vaut pas audit exhaustif ni preuve contre un noyau compromis.

## Essais non exécutés

| Essai | Pourquoi il reste à effectuer |
|---|---|
| Construction des images et démarrage Compose Docker/Podman | Moteurs absents. Les références d'images ne sont pas épinglées par digest dans ce MVP. |
| UID effectif, réseau `none`, capacités, seccomp et cgroups | Ces propriétés doivent être observées sur chaque runtime/système cible. |
| Sessions réelles Codex/Claude et authentification fournisseur | Aucun compte ni version de CLI installé dans un worker réel. |
| Décision réelle Jev | Pas d'activation, de compte ou de modèle choisi pour un appel distant. |
| Sous-agents natifs fournisseurs | Intégration absente du MVP ; le contrat générique explicite est testé. |
| Redémarrage du dashboard pendant un worker, timeout et arrêt brutal du daemon | Requièrent un moteur et des processus conteneurisés réels. |
| macOS, ARM64, WSL, SELinux et différents fournisseurs Compose Podman | Non disponibles dans cet environnement ; voies de portabilité non qualifiées. |
| Charge prolongée et métriques CPU/RAM/disque/réseau | Mesures non réalisées ; champs inconnus conservés comme tels. |
| Audit complet WCAG et exposition distante | Hors du périmètre local livré. |

Le test `runtime_smoke` est volontairement marqué `#[ignore]`. La commande pour l'activer, ses prérequis et les 18 scénarios sont dans [acceptance.md](acceptance.md) et le [README](../README.md). Il produit de vraies sessions quand l'opérateur le lance sur sa machine et conserve leurs volumes pour inspection.

## Contenu de la livraison

L'archive contient les sources Rust et TypeScript, tests, fixtures explicites, lockfiles, schéma JSON, politiques, Dockerfiles, Compose, bundle frontend et documentation. Les dépendances téléchargées, `target/`, les profils de navigateur, captures de QA, secrets et données d'audit temporaires sont exclus. Le binaire est reproductible par les commandes documentées ; aucun binaire dépendant de l'environnement de réalisation n'est distribué comme version universelle.
