Rapport Rust Core — fondation de contrôle v2, 29 octobre 2026

Complément de revue ultérieur : `/tmp/ai-guard-reserved-sqlite-trigger-fix.md` décrit le défaut confirmé des triggers au préfixe SQLite, ses probes métier rouges, sa correction et les vérifications plus récentes. Les résultats ci-dessous décrivent le premier gel, avant ce complément.

Périmètre : stockage et DTO dans `/Users/louis/Documents/ai-guard`. L'intégration API/runner, les effets moteur, les procédures opératoires et la revue indépendante sont détenus par root/autres agents. Cette fondation n'est pas une qualification de production ni une preuve de lancement utilisable avant intégration.

Changements :

- `src/control.rs` : DTO version 1 fermés, bornes et identités, spec intégral, reçu exact, quotas explicites, tentative stable, transitions, résultats, provenance et allocations exactes. `AdmissionV1.validate()` et `input_hash()` publics. `AdmissionResultV1.replayed` vient du résultat transactionnel. Aucun champ tenant libre ni secret d'environnement.
- `src/audit.rs`, `src/audit/control.rs` : nouveaux DB au schéma exact v2 avec genesis audité et namespace serveur ; v1 conservé sans registre tant que migration explicite absente. Admission fraîche distincte des décisions historiques ; reçu historique exact vérifié dans la base authentifiée ; réévaluation déterministe sous transaction et proposition API/Jev exigée Allow. Refus policy/quota durables et idempotents.
- Réservations count/CPU/RAM calculées sous IMMEDIATE ; worker/session/request uniques ; tentative et noms/volumes générés serveur. Toutes les transitions, observations, allocations, mutations et audits partagent la transaction.
- Claim irréversible : une seule réponse fraîche EmitOnce, replay toujours AlreadyConsumed même après progression/redémarrage. CAS et conflits de contenu indépendants du lookup idempotent. Aucun reset du claim.
- Inspection créée durable dans `WorkerAttemptV1.created_inspection`. `WorkerV1.observed_started_at` conserve le premier démarrage crédible. `allocation_inconsistent` retient durablement une divergence identité/configuration/start/restart : un snapshot ultérieur propre ne l'efface pas. Inconnues incomplètes sans contradiction peuvent être complétées par une observation sûre.
- Terminal complet : intervalle runtime exact, produits u128 vérifiés exposés en décimal, `billable=false`, source runtime explicite. Observation + allocation unique + terminal + libération + audit atomiques. Une inconnue conserve le slot et n'invente aucun zéro.
- Reconstruction depuis genesis de l'ensemble exact des colonnes/lignes, y compris opérations, claims, observations, allocations et métadonnées. Revalidation au startup, intégrité et changement externe data_version ; altération = latch fermé, aucune réparation. Recalcul des allocations depuis leur observation.
- `Store::append` refuse `control.*` ; seules les mutations typées créent les projections de contrôle.
- Lectures `workers_page(cursor: Option<&str>,limit: u32)` et `worker_usage_page` bornées 1..100 ; curseurs respectivement worker_id et attempt_id. Outils maintenance `workers()`/`worker_usage()` conservés.
- Migration `Store::migrate_control_v2(&backup_path)` : préflight v1 exact et sain ; sauvegarde SQLite Online Backup API dans une destination create_new/no-follow, jamais un fichier existant, ni ses sidecars ; deadline 10 s, identité/inode, quick_check, chaîne/quota/binding et ancien head vérifiés ; sync fichier et parent avant DDL ; nouveau recheck de head sous IMMEDIATE ; genesis et DDL v2 transactionnels. Audit plein provoque rollback du DDL avec sauvegarde conservée.
- Feature rusqlite `backup` ajoutée à Cargo.toml, sans changement de version ni de Cargo.lock. SQLite décrit l'API comme un snapshot cohérent : https://www.sqlite.org/backup.html ; détails API également lus dans la source officielle de rusqlite 0.41.1 installée (`src/backup.rs`).
- `src/lib.rs` exporte `control`.
- Tests nouveaux : `worker_registry.rs`, `usage_ledger.rs`, `control_migration.rs`. Régressions existantes adaptées au vrai genesis (+1 événement et capacité explicite) dans audit_integrity, audit_load, audit_security, decision_idempotence, tenant_storage. Le test legacy construit un vrai schéma historique, pas une v2 renommée. Le test de DDL malveillant désactive uniquement les FK de sa connexion d'attaque afin de continuer à tester le schéma, sans relâcher la validation produit.

Vérifications constatées (PATH Rust/Homebrew explicite, login:false) :

- PASS : `cargo test --locked --test worker_registry --test control_migration --test usage_ledger` — 21 tests (15 + 4 + 2), 0 échec. Un test registry est le helper de processus ; le scénario parent lance réellement deux processus distincts. Les 100 admissions sont logiques et ne qualifient pas 100 conteneurs.
- PASS : `cargo test --locked --test audit_integrity --test audit_load --test audit_security --test tenant_storage --test decision_idempotence --test review_regressions` — 33 tests, 0 échec après adaptation explicite du genesis. Ces résultats ont précédé les derniers champs de provenance ; les 21 nouveaux tests ont été réexécutés après ces champs.
- PASS : `cargo clippy --locked --lib -- -D warnings` — dernière exécution après les derniers changements core, exit 0.
- PASS : rustfmt appliqué uniquement aux fichiers possédés ; aucun cargo fmt global pendant les éditions parallèles.
- FAIL, intégration en cours : dernière tentative `CARGO_INCREMENTAL=0 cargo test --locked --no-fail-fast` dans `/tmp/ai-guard-worker-store-suite.log` a encore quatre cibles root à adapter au genesis : api_security, decision_event_authority, gateway_tenancy, tenant_api. Les cibles core, anciennes régressions stockage, runtime_managed et workers_api étaient PASS dans cette exécution. Root a explicitement pris les quatre fichiers ; ce résultat n'est pas la validation finale intégrée.
- BLOCKED pendant édition parallèle : tentative `cargo clippy --locked --all-targets -- -D warnings` a rencontré le RED runtime_managed appelant ManagedRuntime.doctor avant son implémentation. Aucun assouplissement n'a été fait ; la vérification globale doit être rejouée par root après gel.
- NOT_RUN par cet agent : release build, front, navigateur et tout moteur réel. Aucun moteur/volume/secret/publication manipulé.

Cycle rouge/vert constaté : nouveau schéma attendu 2 contre 1 ; API Store manquante avant implémentation ; migration manquante ; répétition d'un StartedAt divergent pouvait devenir Terminal ; un restart non nul pouvait disparaître via un snapshot ultérieur ; inspection créée absente du snapshot ; source allocation absente. Les régressions correspondantes sont désormais vertes.

Critères couverts : quota 0/1/100, concurrence deux connexions et deux processus, replay après redémarrage et divergence, ancien faux allow refusé par politique fraîche, reçu autre tenant refusé, secret spec refusé, claim concurrent unique/replay non émetteur, annulation Reserved fermant tout futur lancement, terminal exact et idempotent, audit plein sans libération partielle puis reprise, absence de dates/exit/restart/limites/autorité conservatrice, projection/ledger/observation altérée détectée, migration v1 non vide et rollback, version future et schéma partiel refusés, backup et sidecars préservés.

Limites et blocages précis :

- Revue indépendante sécurité/audit/lancement requise par AGENTS.md : root doit la faire avant clôture. Aucun reviewer lancé par cet agent, conformément au partage du travail.
- Les primitives Store ne détiennent pas le verrou runtime OS et ne peuvent fence un ancien broker/CLI. L'autorité moteur et l'unique émission effective sont à intégrer et tester par le propriétaire runtime/root.
- Migration prévue contrôle quiescent/offline : le Store ne peut découvrir les conteneurs legacy encore actifs. Le CLI de maintenance doit imposer la procédure opérateur et le préflight runtime avant son invocation.
- Aucune restauration v2 ni réactivation automatique après restore livrée. Un backup ancien peut précéder un start réellement émis : tant que l'inventaire/réconciliation avec émission désactivée n'est pas conçu et testé, aucune procédure restore v2 ne peut être annoncée sûre.
- Le latch d'allocation contradictoire n'a pas de procédure automatique de libération ; il conserve la capacité et expose une réconciliation nécessaire. Une clôture après Preparing/Created exige la procédure d'autorité/fencing ; seule Reserved est annulable.
- Pannes réelles disque/commit, coupure processus au milieu de chaque étape SQL et restauration système non qualifiées ici. Les tests audit plein et altération logique ne les remplacent pas.
- L'API finale, le runner, les parcours de reprise/stop/export, le scénario réel trois workers, la charge réelle 100, Podman, autres plateformes et cloud restent à qualifier séparément. Aucun prix, consommation CPU réelle, HA ou exactly-once moteur revendiqué.
- Skill `.agents/skills/ai-guard-verify/SKILL.md` absent ; chargement automatique non confirmé. Contrats du dépôt, TDD et vérification avant affirmation ont guidé ce lot.

Le dépôt reste préexistant entièrement untracked sans HEAD ; aucun commit, nettoyage Git, effacement global ou écrasement de travail d'un autre agent effectué.
