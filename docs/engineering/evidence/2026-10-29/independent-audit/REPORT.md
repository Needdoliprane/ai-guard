# Revue de sécurité indépendante — ai-guard

## Conclusion exécutive

La revue porte sur la copie figée `/private/tmp/ai-guard-audit-20261029.n8ZxPB`, soit 169 fichiers hors `.git`, `target` et `node_modules`. Son identité reproductible est le manifeste SHA-256 :

`77876e6de40964096f90f458b07b872b7848f1a05257e2f763e5f57679641d34`

Résultat formel :

- **Vulnérabilité démontrée : 0**
- **Risque plausible restant à vérifier : 19**
- **Candidat rejeté après validation : 0**
- **Unités de couverture : 18**
- **Sévérité formelle des 19 candidats : non attribuée**
- **Exécution de code cible : BLOCKED**
- **Tests Rust, front, navigateur et runtime : NOT_RUN dans cette revue**

Ce résultat ne signifie pas que les 19 chemins sont bénins. Plusieurs conduiraient, s'ils étaient reproduits dans les conditions décrites, à une exécution hôte non autorisée, une prise de credentials de contrôle, une redirection de l'audit, une poursuite de worker hors contrôle ou un épuisement de ressources partagé. Leur trace source a survécu à une validation indépendante puis à une seconde relecture finale. Le fait décisif restant appartient toutefois au runtime, au filesystem, au moteur ou au déploiement.

Aucun défaut de sélection libre du tenant, aucune mutation viewer et aucune lecture inter-tenant directe n'ont été établis dans les handlers actuellement implémentés.

## Méthode et limites

La revue a confronté le code à `AGENTS.md`, aux contrats d'architecture et aux documents de qualification. La documentation a été traitée comme intention, jamais comme preuve d'effet.

Le checkout de développement a divergé de l'instantané pendant la revue. Ces changements ultérieurs ne sont pas inclus dans les verdicts ; aucune conclusion du rapport ne doit être transférée au checkout courant sans revalidation différentielle.

Le workflow Cloudflare `security-audit` a été appliqué explicitement. Le skill projet `.agents/skills/ai-guard-security-review/SKILL.md` est absent du checkout ; son chargement automatique n'a donc pas été possible.

Aucun code cible n'a été exécuté, car l'environnement observé ne permet pas de prouver simultanément :

- un environnement vide avec allowlist ;
- l'interdiction du réseau externe et un loopback isolé ;
- la cible et les outils en lecture seule ;
- les écritures limitées au scratch ;
- des limites CPU, mémoire, PID, taille de fichiers, disque et durée appliquées à tous les descendants ;
- une promotion d'artefacts sûre par descripteurs.

Les tests historiques, mocks, fixtures, manifests Compose et tests ignorés ont été lus comme contrôles source, sans être transférés comme preuves de cette revue. `runtime_smoke`, Docker, Podman, Playwright, la recette A1/A2/B1 et les scénarios de panne n'ont pas été exécutés.

## Classification des résultats

| Catégorie | Résultat |
|---|---:|
| Vulnérabilité démontrée | 0 |
| Risque plausible restant à vérifier | 19 |
| Exigence obligatoire encore absente | 8 familles principales |
| Garantie effectivement vérifiée | Source uniquement ; aucune qualification runtime |

Les 19 records sont détaillés dans `NEEDS-VALIDATION.md` et structurés dans `findings.json`.

| Famille | Candidats | Impact potentiel |
|---|---:|---|
| API, SSE et ingress | 4 | Retard des décisions/audits, arrêt prolongé, famine pré-authentification |
| Audit SQLite et fichiers | 2 | Lecture, altération ou redirection de la base d'un tenant |
| Secrets, configuration et redaction | 7 | Prise d'un Bearer de contrôle, envoi d'un credential à Jev, persistance de secrets ou fragments |
| Moteur, arrêt et stockage | 5 | Exécution hôte, substitution du daemon attesté, worker non arrêté, épuisement du stockage |
| Dashboard statique | 1 | Lecture HTTP publique d'un fichier hors racine via symlink |

## Garanties effectivement vérifiées dans la source

Ces garanties résultent d'une lecture contradictoire du code. Elles ne constituent pas une preuve de leur application par Docker, Podman, l'OS ou un déploiement réel.

### Identité, rôles et tenant

- Le Bearer sélectionne le `TenantState`, le rôle et le `Store`; aucun header, paramètre de requête ou champ métier ne remplace cette liaison (`src/api.rs:146-210`).
- Les credentials ambigus entre rôles ou tenants sont refusés.
- Le viewer n'atteint pas les mutations runner.
- SSE, export, projections et recherche restent attachés au Store authentifié (`src/api.rs:372-520`).
- Chaque base porte un `tenant_binding` durable et une réouverture sous une autre identité est refusée (`src/audit.rs:248-333`).

### Audit, transactions et idempotence

- Append et décision utilisent des transactions `IMMEDIATE`.
- Le commit intervient avant l'accusé HTTP.
- Quota d'octets, décision et événement d'audit sont mis à jour dans la même transaction (`src/audit.rs:358-537`).
- Une même identité avec contenu divergent provoque un conflit (`src/audit.rs:416-429`).
- Une approbation ambiguë, un hash erroné ou une revue absente bloque le lancement (`src/audit.rs:539-605`).
- Une erreur d'audit préalable à l'exécution reste fail-closed.

### Liaison autorisation–exécution

- Le plan, les options et le hash approuvés sont vérifiés avant consommation de l'approbation (`src/runner.rs:309-356,478-533`).
- La session durable et `runtime.submitted` précèdent l'effet moteur (`src/runner.rs:535-551,645-654`).
- La commande métier est transmise par programme et arguments séparés, sans shell hôte.
- Aucun fallback d'exécution métier sur l'hôte n'a été trouvé.

### Confinement demandé au moteur

`src/runtime.rs:86-145` construit des arguments demandant : utilisateur 1000, rootfs en lecture seule, capacités supprimées, `no-new-privileges`, limites CPU/RAM/PID, réseau `none` par défaut, tmpfs bornés, aucun socket moteur, aucun mode privilégié et aucun montage du projet original. L'application effective de ces options n'a pas été observée.

### Workspace, imports et exports

`src/workspace.rs:85-185,275-335,417-548` ouvre relativement à des descripteurs, refuse liens symboliques, hardlinks, fichiers spéciaux et traversées, borne fichiers/octets/profondeur, publie sans remplacement dans une destination neuve et n'écrit pas dans le projet original.

### Jev, ingress et frontend

- Jev n'est appelé qu'après un `allow` local et ne peut qu'abaisser la décision ; erreurs, réponses ambiguës et défauts de transport deviennent `deny` (`src/api.rs:330-369`, `src/jev.rs:31-125`).
- L'ingress impose une destination fixe, refuse cibles absolues, `CONNECT`, `TRACE` et upgrades, bufferise le corps avant relais et retire Host/Forwarded/hop-by-hop ; redirects, retries et proxies ambiants sont désactivés (`src/ingress.rs:96-197,241-343`).
- Le front conserve le token en mémoire, omet cookies et redirects, limite les routes de lecture et injecte les données non fiables par `textContent`/`createTextNode` (`front/src/api.ts:17-61,83-134`, `front/src/dom.ts:7-19`). Aucun test navigateur n'a été rejoué.

## Exigences obligatoires encore absentes

Ces écarts ne sont pas des vulnérabilités confirmées ; ils empêchent la qualification du produit demandé.

1. **Identité worker, registre, grants et relais métier.** Aucun principal `worker`, registre, autorisation dirigée `source → destination + canal`, révocation ou relais inter-workers n'est implémenté (`docs/engineering/CLOUD_TENANCY.md:3,31`, `docs/engineering/CURRENT_STATE.md:14,23,52`, routes présentes dans `src/api.rs:272-282`).
2. **Réservations, quotas workers, usage et reprise.** Pas de réservation CPU/RAM durable, libération terminale atomique, mesure idempotente ni état inconnu à réconcilier (`docs/engineering/USAGE_METERING.md:3-9`, `CURRENT_STATE.md:23,25,52`). Le quota de `src/audit.rs:442-469` ne concerne que les octets du journal.
3. **Persistance terminale avant cleanup et réconciliateur.** Le runtime observe le terminal à `src/runtime.rs:479-486`, nettoie à `488-498`, puis le runner ne persiste le résultat qu'à `src/runner.rs:675-704`. Une erreur de cleanup peut donc faire perdre l'observation ; aucun réconciliateur ne rattache conteneur, volume et opération après redémarrage (`CURRENT_STATE.md:46`).
4. **Sauvegarde, restauration, migrations et reprise SQLite/WAL.** La base par tenant existe, mais ces parcours inter-release et après panne ne sont pas qualifiés (`CLOUD_TENANCY.md:39`, `PRODUCTION_READINESS.md:12`).
5. **Cloud et Kubernetes.** L'adaptateur Kubernetes, l'identité workload, TLS/mTLS selon topologie, les politiques réseau effectivement appliquées et le stockage cloud ne sont pas livrés (`src/runtime.rs:10-15`, `CURRENT_STATE.md:15,26`, `PRODUCTION_READINESS.md:17,20`).
6. **Capacité et stockage.** La cible de 100 workers, les quotas de volumes/logs, la réserve du contrôle et la mesure durable restent non qualifiés ; G10 est `NOT_RUN`.
7. **Chaîne de livraison.** Images référencées par tags, bases et paquets APT non épinglés (`images/control.Dockerfile:1,8,14-17`, `images/worker.Dockerfile:1,3,6-8`), sans preuve d'identité, signature, promotion, SBOM/provenance ou candidate de release qualifiée.
8. **Recette multi-client runtime.** A1/A2 chez A et B1 chez B, isolation réelle des volumes/réseaux/logs/exports et matrices Docker/Podman × Linux/macOS n'ont pas été qualifiées (`TEST_STRATEGY.md:29,31,33`).

## Cinq risques prioritaires

### 1. Résolution du CLI moteur par le PATH

`runtime/path-resolves-project-executable`

Si ai-guard est lancé depuis un projet moins fiable avec un `PATH` relatif, vide ou contenant ce projet avant le vrai moteur, un fichier `docker` ou `podman` du projet peut être exécuté sur l'hôte dès `doctor`.

- Sévérité formelle : non attribuée (`needs_validation`)
- Impact potentiel : exécution de code avec l'autorité OS du broker
- Confiance dans la trace source : élevée
- Condition décisive : PATH/cwd réellement influençables par le projet
- Correction : chemin absolu approuvé, environnement allowlisté et cwd de confiance
- Régression : faux moteur dans le projet, avec vérification qu'il n'est jamais exécuté

### 2. Substitution du répertoire de configuration validé

`operations/config-path-reopen-secret-substitution`

`initialize_config` sécurise un arbre par descripteurs, puis `compose` recanonicalise son pathname après `doctor`. Dans un parent renommable par un autre UID, les projections runner/viewer peuvent être remplacées avant leur montage.

- Sévérité formelle : non attribuée (`needs_validation`)
- Impact potentiel : prise des rôles runner/viewer, lecture d'audit et mutations
- Confiance dans la trace source : élevée
- Condition décisive : parent renommable par un principal hors contrôle
- Correction : conserver et attester dev/inode jusqu'au handoff moteur ; exiger des parents privés
- Régression : course déterministe à deux UID avec tokens factices

### 3. Arrêt brutal laissant un worker hors contrôle

`runtime/abrupt-broker-stop-leaves-worker-unfenced`

Timeout, kill, cleanup et événement terminal dépendent de la future du broker. `Operator::Run` ne traite pas SIGTERM et aucune identité runtime durable ne permet la réconciliation.

- Sévérité formelle : non attribuée (`needs_validation`)
- Impact potentiel : worker continuant après l'arrêt demandé, sans audit terminal
- Confiance dans la trace source : élevée sur l'absence de reprise, moyenne sur la survie moteur
- Condition décisive : Docker/Podman conserve le conteneur après disparition du client/broker
- Correction : identité runtime durable, lease/fencing, SIGTERM commun et réconciliateur
- Régression : interruption aux états submitted/started/terminal/cleanup et reprise idempotente

### 4. Connexion SQLite non liée au descripteur validé

`audit/sqlite-connection-path-reopen-identity-race`

L'application valide un descripteur A puis SQLite rouvre le pathname. Une substitution A → B → A dans un parent renommable peut laisser la connexion attachée à B tandis que les contrôles portent sur A.

- Sévérité formelle : non attribuée (`needs_validation`)
- Impact potentiel : redirection, lecture ou altération de l'audit d'un tenant
- Confiance dans la trace source : moyenne
- Conditions décisives : parent renommable et comportement VFS/WAL compatible
- Correction : lier la connexion SQLite à l'inode validé ou imposer une racine privée non renommable
- Régression : hook déterministe entre les deux ouvertures avec deux bases factices

### 5. Secrets worker insuffisamment protégés avant audit et Jev

Famille : `redaction/json-object-keys-preserved`, `redaction/truncation-before-secret-masking`, `runner/decision-omits-known-worker-secret-redaction`, `operations/jev-control-credential-reuse`.

- Sévérité formelle : non attribuée (`needs_validation`)
- Impact potentiel : persistance ou transmission d'un credential ou d'un fragment à un viewer ou à Jev
- Confiance dans les traces source : élevée
- Corrections : redaction récursive des clés, masquage avant troncature, refus des secrets connus dans la décision, unicité globale Jev/runner/viewer
- Régressions : secrets factices comme clé, autour des bornes 4096/8192, dans argv positionnel et collisions Jev

## Conditions nécessaires à une qualification ultérieure

Une qualification ultérieure exige au minimum :

1. un environnement d'essai respectant le confinement requis par le workflow ;
2. la reproduction non destructive ou la réfutation des 19 candidats avec uniquement des secrets et ressources factices ;
3. des tests séparés Docker et Podman sur Linux x86_64 et macOS ARM64, avec versions consignées ;
4. la recette locale A1/A2/B1 et des preuves d'isolation API, SQLite, volumes, réseaux, logs et exports ;
5. des essais de panne aux frontières décision, soumission, démarrage, observation terminale, audit et cleanup ;
6. un registre durable et un réconciliateur idempotent pour workers, quotas, grants, usage et cleanup ;
7. des quotas effectifs bytes/inodes pour volumes et journaux moteur, plus une réserve de capacité du plan de contrôle ;
8. sauvegarde/restauration, migrations, rétention et reprise SQLite/WAL ;
9. une qualification des arrêts SSE, des lectures coûteuses et de l'admission pré-authentification ;
10. la fermeture ou l'acceptation documentée de tous les gates G01 à G12 applicables.

Aucune conclusion de qualification globale ne peut être tirée du seul fait que le projet compile, que des fixtures existent ou que des manifests demandent des contrôles.
