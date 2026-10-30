# Contrat core proposé : admission, worker et allocation durables

29 octobre 2026. **Conception, non implémentée et non qualifiée.** Sources lues : /tmp/ai-guard-worker-runtime-design.md ; AGENTS.md ; architecture/règles/gates/tests, CLOUD_TENANCY.md, LOCAL_CLOUD_TESTS.md, USAGE_METERING.md ; Store/API/runner/runtime actuels. Aucun moteur lancé et aucun fichier produit modifié pour cette conception. Le correctif indépendant d'ingestion security.decision a été explicitement autorisé ensuite ; son rapport est /tmp/ai-guard-decision-event-authority-report.md.

## 1. Choix déjà arrêtés par root et portée du premier lot

- Une commande métier correspond à une instance worker terminale, exclusivement dans un tenant. Une session stable ne peut pas être rejouée comme un nouveau worker. Les sous-agents restent des agents. Aucun daemon, service, stockage, bus ou dépendance supplémentaire ; gateway/SQLite par tenant et broker Rust hôte existants.
- Première preuve mono-hôte, sur Docker ou Podman séparément. Le broker est dans le contrôle de confiance. Le verrouillage réel de l'autorité et le sort des CLI en vol sont à spécifier/tester avec le propriétaire runtime ; pas de lease SQL présentée comme fencing du moteur.
- Admission versionnée distincte, liant le reçu exact de décision historique à un LaunchSpec complet et nouvellement autorisé côté serveur. Le request_hash historique et la sérialisation DecisionRequest ne changent pas.
- Spécification initiale effective : 2000 millicœurs, 1073741824 octets RAM, PID256 ; quota configurable de0 à100 slots, budgets CPU/RAM et durée maximale. Ces budgets sont des bornes logiques d'admission, pas une capacité physique déclarée ni une preuve100 workers.
- Mesure publique : intervalle du processus StartedAt→FinishedAt et limites inspectées stables, produits entiers exacts, billable=false. Unknown, restart et limites divergentes retiennent le slot. Aucun temps d'attach, coût, prix, consommation CPU ou zéro synthétique.
- Un claim start est irréversible. Seule sa première réponse fraîche peut autoriser une émission ; le replay renvoie AlreadyConsumed. Réponse perdue : inspect/reconcile, jamais retry start.

Approches : reconstruire uniquement entities() échoue à fournir unicité/quotas/claim typés ; déplacer Docker dans gateway enfreint la frontière. **Recommandation : tables typées de projection et d'idempotence dans la même base**, engagées avec les transitions d'audit. Une fondation peut précéder l'intégration, mais ne sera pas appelée registre livré avant un vrai run/reprise/terminal utilisant ces chemins.

## 2. État actuel vérifié et correction d'une source périmée

- API decision_response ne renvoie pas encore phase ni hash de ligne ; Authorization::Allowed(Value) du runner ne garde que decision après vérification du seq/hash. Il faut conserver un reçu complet typé.
- ContainerSpec::new crée toujours une operation UUID ; inspect porte sur .State et pas l'identité complète/configuration du conteneur/daemon. Un nom recalculé avec un nouvel UUID n'est pas une reprise.
- **Correction du document runtime antérieur :** RuntimeState exige maintenant Running et ExitCode ; validate_terminal vérifie statut terminal, running=false, exit et dates. « ExitCode absent devient0 » n'est plus vrai dans le checkout lu. L'identité/restart_count/limites inspectées restent absents de ce type.
- La séparation exécution retenue/cleanup est un chantier parallèle runtime/root : ne pas supposer son intégration tant qu'un vrai accusé terminal précède toute suppression. Le contrat ci-dessous requiert cette séparation, quel que soit son nom final.
- Store v1 valide un schéma exact, la chaîne, les projections d'événement et quota octets ; il ne valide pas de futures tables workers. Ajouter des tables sans versionner/refuser/rejouer les projections casserait le P0.
- Le P1 confirmé sur /events a montré qu'un ancien security.decision pouvait être forgé par un runner. Le refus des namespaces protège les nouvelles ingestions, **pas l'origine des anciennes lignes**. Une admission v2 doit donc produire une nouvelle autorisation centrale complète ; pas convertir un vieux allow en ticket.

## 3. DTO et empreintes

Toutes les requêtes de contrôle ont version=1, deny_unknown_fields, taille bornée et des enums fermés. UUID et sha256 hex sont validés ; pas de tenant_id en entrée. Sélection Store uniquement par auth runner/viewer existante. IDs physiques, observations et quotas ne proviennent jamais d'une identité worker. Worker est encore interdit de ces routes.

```rust
enum DecisionPhase { Initial, Approved }
struct DecisionReceiptV1 {
    version: u32, request_id: Uuid, request_hash: Sha256Hex,
    phase: DecisionPhase, audit_seq: i64, audit_hash: Sha256Hex,
}
struct AdmissionV1 {
    version: u32, operation_id: Uuid,
    decision_request: DecisionRequest, receipt: DecisionReceiptV1,
    launch: LaunchSpecV1,
}
struct LaunchSpecV1 {
    version: u32, session_id: Uuid, command: CommandSpec, profile: Profile,
    runtime_kind: DockerOrPodman, authority_id: AuthorityId,
    image_ref: String, image_id: ImmutableImageId,
    network: NoneOrBridge, cpu_millis: u32, memory_bytes: u64,
    pids_limit: u32, timeout_seconds: u32,
    security_preset: FixedSandboxV1, restart_policy: No,
    workspace_manifest_digest: Sha256Hex,
    workspace_import_policy_version: u32,
    provider: Provider, capture_output: bool,
    credential_binding_id: Option<Uuid>, credential_names: Vec<EnvName>,
}
struct MutationReceiptV1 {
    operation_id: Uuid, worker_id: Option<Uuid>, revision: Option<u64>,
    input_hash: Sha256Hex, audit_seq: i64, audit_hash: Sha256Hex,
}
enum ClaimDisposition { EmitOnce, AlreadyConsumed }
```

Admission réévalue la commande sous la politique centrale courante, y compris Jev si configuré ; un ancien faux allow strict/rm reste refusé. Le reçu exact sert de lien historique, pas de permission suffisante. Une phase approved exige toujours le hash exact et une revue initiale cohérente ; aucune promotion de deny. Ne pas appeler le cache historique /decide comme unique réévaluation d'admission. L'autorisation d'admission possède son propre hash/domain/version et son événement après genesis.

Comparer launch.command/profile/session et les corrélations attendues au DecisionRequest original fourni et à son hash canonique ; l'événement historique contient parfois une commande masquée et ne suffit pas pour reconstruire l'original. Vérifier reçu.seq/hash/phase/request_id dans la Store authentifiée. Le hash actuel n'autorise pas image/réseau/ressources/secrets : l'admission les vérifie séparément et les lie durablement. Un profil strict/review impose none ; bridge exige observe explicite. Les paramètres UID, caps, readonly, NNP, tmpfs, mounts et restart=no sont un preset fermé, jamais des booléens libres du client. ImageID et autorité seront recoupés à l'inspection avant start ; le runner de confiance atteste la résolution, pas un label du worker.

Seules données publiques/non secrètes dans LaunchSpec/audit/DB : aucune valeur d'env, token, chemin de fichier secret, contenu de projet ou stdout. Credential_binding_id désigne un chargement privé contrôlé par le broker ; names seul ne promet pas l'identité des valeurs. Si le masquage transforme un champ nécessaire à l'identité, au replay ou à l'exécution du spec, refuser400 avant persistance plutôt que conserver un spec inexécutable ou désactiver le masqueur. Secrets dans env explicite, pas dans argv. Les secrets arbitraires inconnus ne sont pas détectables universellement.

Empreintes : sérialisation typée Rust/versionnée, champs Option normalisés, dates UTC/nanosecondes, aucun float ; préfixes distincts `ai-guard:launch:v1`, `ai-guard:admission:v1`, `ai-guard:worker-mutation:v1`, `ai-guard:worker-snapshot:v1`. Le hash input inclut action+worker/attempt+expected_revision+payload, exclut uniquement données de transport. Même operation_id avec divergence→409. Le spec persistant doit être exactement celui empreinté après contrôle du masquage.

## 4. Routes minimales et méthodes Store proposées

| Route /api/v1 | Rôle | Effet durable / réponse |
|---|---|---|
| POST /workers/reservations | runner | AdmissionV1 ; première acceptation201, répétition200 avec même worker/attempt/spec/receipt ; divergence409, refus de politique403, quota429, audit503 |
| GET /workers/operations/{operation_id} | runner/viewer | Retrouver reçu après réponse perdue ;404 dans son tenant si absent |
| GET /workers et /workers/{worker_id} | runner/viewer | Registre réel, quota_held, dernière observation/source, stale/unknown, action de reprise |
| POST /workers/{id}/prepare | runner | Intentions/helpers/volume serveur, puis preuve d'import réussi ; aucune commande helper libre |
| POST /workers/{id}/create-issued | runner | Création destinée à la tentative stable, avant émission create |
| POST /workers/{id}/created | runner | Identité réelle et config inspectée ; aucune admission de nom étranger |
| POST /workers/{id}/start-claim | runner | CAS+consommation unique ; EmitOnce seulement transition fraîche, sinon AlreadyConsumed sans permission d'émission |
| POST /workers/{id}/observations | runner | Snapshot typé ; si terminal complet, finalisation/ledger/libération dans cette transaction |
| POST /workers/{id}/unknown | runner | Motif borné et preuve disponible, state=in_doubt, slot retenu |
| POST /workers/{id}/stop | runner | Intention durable, pas promesse d'arrêt ; broker doit la relever et agir sur le même CID |
| POST /workers/{id}/cancel-reserved | runner | Uniquement Reserved sans création/claim en vol, fermeture centrale de tout claim futur ; pas de TTL |
| POST /workers/{id}/cleanup-result | runner | Incident ou suppression constatée après reçu terminal ; aucune seconde mesure |
| GET /usage et /quota | runner/viewer | Allocation non financière et ressources retenues de son tenant |

Mutations comportent operation_id, worker_id/attempt_id, expected_revision et payload typé. L'observation_id est l'operation_id de la mutation d'observation, pour éviter deux identités concurrentes. GET ne prend pas un tenant libre. Futur rôle worker ne peut ni lire registre/usage ni muter. /api legacy peut rester limité au contrat d'audit existant ; les nouveaux endpoints sont explicitement v1.

Méthodes, noms proposés : `admit_worker`, `prepare_worker`, `record_create_issued`, `record_created`, `claim_start_once`, `record_runtime_observation`, `mark_worker_in_doubt`, `request_worker_stop`, `cancel_reserved_worker`, `record_cleanup`, `get_worker`, `worker_operation`, `tenant_quota`, `allocation_usage`. Store garde ses primitives transactionnelles privées. Les handlers n'obtiennent jamais Connection ni une transaction à remplir arbitrairement.

Une prévalidation/évaluation externe peut précéder l'ouverture transactionnelle, **jamais un mutex tenu pendant await**. Un second contrôle sous IMMEDIATE revalide input/idempotence/état/politique attachée/quota avant effet. Réponse tirée du résultat engagé. Erreurs typées : InvalidInput400, Forbidden403, NotFound404, ContentConflict/RevisionConflict/AlreadyBound409, QuotaExceeded429, StoreUnavailable503 ; code stable et motif exploitable, aucun secret.

## 5. Schéma v2 proposé

Les objets v1 restent byte-for-byte au schéma connu ; tenant_binding.schema_version=1 versionne sa structure, tandis que PRAGMA user_version passe à2. Pas d'élargissement aux schémas inconnus, triggers ou vues. Les types suivants sont de nouvelles tables/index **dans la même base**, non de nouveaux composants. SQL à figer avec le propriétaire stockage avant exécution, non exécuté ici.

```sql
CREATE TABLE control_meta(
  singleton INTEGER PRIMARY KEY CHECK(singleton=1),
  version INTEGER NOT NULL CHECK(version=2),
  namespace_uuid TEXT NOT NULL UNIQUE,
  genesis_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq),
  genesis_hash TEXT NOT NULL CHECK(length(genesis_hash)=64)
);
CREATE TABLE workers(
  worker_id TEXT PRIMARY KEY,
  admission_operation_id TEXT NOT NULL UNIQUE,
  request_id TEXT NOT NULL UNIQUE,
  request_hash TEXT NOT NULL CHECK(length(request_hash)=64),
  decision_phase TEXT NOT NULL CHECK(decision_phase IN ('initial','approved')),
  decision_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq),
  decision_hash TEXT NOT NULL CHECK(length(decision_hash)=64),
  spec_json TEXT NOT NULL CHECK(json_valid(spec_json)),
  spec_hash TEXT NOT NULL CHECK(length(spec_hash)=64),
  admission_policy_hash TEXT NOT NULL CHECK(length(admission_policy_hash)=64),
  state TEXT NOT NULL CHECK(state IN
    ('reserved','preparing','created','start_issued','running','stopping',
     'in_doubt','terminal','never_started')),
  quota_held INTEGER NOT NULL CHECK(quota_held IN (0,1)),
  cpu_millis INTEGER NOT NULL CHECK(cpu_millis=2000),
  memory_bytes INTEGER NOT NULL CHECK(memory_bytes=1073741824),
  revision INTEGER NOT NULL CHECK(revision>=1),
  snapshot_json TEXT NOT NULL CHECK(json_valid(snapshot_json)),
  snapshot_hash TEXT NOT NULL CHECK(length(snapshot_hash)=64),
  last_event_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq),
  CHECK((state IN ('terminal','never_started') AND quota_held=0)
     OR (state NOT IN ('terminal','never_started') AND quota_held=1))
);
CREATE INDEX workers_held ON workers(quota_held,state);
CREATE TABLE worker_attempts(
  attempt_id TEXT PRIMARY KEY,
  worker_id TEXT NOT NULL UNIQUE REFERENCES workers(worker_id),
  authority_id TEXT NOT NULL,
  stable_name TEXT NOT NULL,
  volume_name TEXT NOT NULL,
  container_id TEXT,
  create_issued_seq INTEGER REFERENCES events(seq),
  created_seq INTEGER REFERENCES events(seq),
  start_issued_seq INTEGER UNIQUE REFERENCES events(seq),
  start_operation_id TEXT UNIQUE,
  start_emitter_id TEXT,
  snapshot_json TEXT NOT NULL CHECK(json_valid(snapshot_json)),
  last_event_seq INTEGER NOT NULL REFERENCES events(seq),
  UNIQUE(authority_id,stable_name),
  UNIQUE(authority_id,container_id),
  CHECK((start_issued_seq IS NULL AND start_operation_id IS NULL AND start_emitter_id IS NULL)
     OR (start_issued_seq IS NOT NULL AND start_operation_id IS NOT NULL AND start_emitter_id IS NOT NULL))
);
CREATE TABLE worker_observations(
  observation_id TEXT PRIMARY KEY,
  attempt_id TEXT NOT NULL REFERENCES worker_attempts(attempt_id),
  input_hash TEXT NOT NULL CHECK(length(input_hash)=64),
  payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
  event_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq)
);
CREATE TABLE worker_allocations(
  attempt_id TEXT PRIMARY KEY REFERENCES worker_attempts(attempt_id),
  terminal_observation_id TEXT NOT NULL UNIQUE REFERENCES worker_observations(observation_id),
  started_at TEXT NOT NULL, finished_at TEXT NOT NULL,
  duration_ns TEXT NOT NULL,
  cpu_millicore_nanoseconds TEXT NOT NULL,
  memory_byte_nanoseconds TEXT NOT NULL,
  method_version INTEGER NOT NULL CHECK(method_version=1),
  billable INTEGER NOT NULL CHECK(billable=0),
  event_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq)
);
CREATE TABLE worker_mutations(
  operation_id TEXT PRIMARY KEY,
  worker_id TEXT REFERENCES workers(worker_id),
  action TEXT NOT NULL,
  input_hash TEXT NOT NULL CHECK(length(input_hash)=64),
  event_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq)
);
```

Validation Rust ferme des UUID, autorités, enums, décimaux canoniques et bornes ; les CHECK ne remplacent pas le protocole. Les quantités de ledger sont des chaînes décimales exactes calculées en u128 contrôlé, jamais des nombres JS arrondis. Les ressources/identités/observations dupliquées dans colonnes et snapshot doivent être identiques sous validateur. Le nom/volume sont dérivés d'un namespace serveur+worker UUID serveur ; session_id et noms reçus du broker ne sélectionnent jamais un volume. Ne pas garder `volume_name(session_id libre)` comme autorité multi-client.

worker_mutations conserve les clés des anciennes mutations après progression de l'état. Il permet un replay de reçu historique sans nouvel événement, et rejette même operation_id/action différente. Il référence l'événement qui porte le résultat typé ; pas besoin de stocker un second JSON de résultat susceptible de diverger. Le reçu inclut l'état après cette mutation, pas une permission de reprendre cet état aujourd'hui ; GET donne le courant. Les ID et quantités indispensables ne peuvent être masqués en douce.

## 6. Transactions et machine d'état

### Admission et réservation

1. Auth tenant, parse/version/borne/masquage. Vérifier le reçu historique exact et son Request original ; réévaluer fraîchement la politique d'admission complète, sans réutiliser le allow du cache comme résultat suffisant. Jev hors verrou puis recheck sous transaction.
2. `begin_checked` IMMEDIATE : contrôles P0 + projection v2. Lookup operation_id : même hash→reçu déjà engagé ; divergence→409. Une request_id déjà liée à un autre worker ne peut être réservée via une autre phase/seq/operation.
3. Bornes fixes du spec et politique configurée ; calcul transactionnel des ressources retenues :

```sql
SELECT count(*),coalesce(sum(cpu_millis),0),coalesce(sum(memory_bytes),0)
FROM workers WHERE quota_held=1;
```

4. Comparer à max_active_workers≤100, max_cpu_millis, max_memory_bytes et max_timeout_seconds ; pas de compteur libre modifiable. Générer worker_id et attempt_id une seule fois ; snapshot réservable, aucun start consommé ; audit d'admission neuf après genesis.
5. Append du résultat autoritatif interne + INSERT workers/attempts/mutation dans **la même transaction**, vérifier lignes/snapshot/auditrefs puis commit checked. Aucun volume/helper/conteneur client créé avant ce reçu. Une erreur renvoie503, pas un worker sans audit.

Refus de politique/quota : recommandation de journaliser le résultat d'admission sans worker ni slot dans la même mécanique d'opération (worker_id=NULL). Un refus durable est repris identique ; l'action utilisateur « réessayer après libération » utilise une nouvelle operation_id liée au même Request, et non une réécriture du refus. Si root préfère ne pas rendre ces refus idempotents, il faut préciser cette exception avant DTO/tests ; aucun effet partiellement engagé n'est permis dans les deux cas.

### Préparation, création et claim irréversible

Admission→preparing avant helpers. Les helpers Import/Export sont du contrôle : argv fixes, réseau none, aucune identité runner/provider, volume de cette réservation, audit et limites propres ; ils ne sont pas des workers supplémentaires et ne contournent pas le quota avec un purpose libre.

Avant create métier : operation/tentative/nom/autorité déjà durables. Après create : persister le vrai CID et l'inspection complète conforme au spec. Un conflit de nom étranger n'autorise ni adoption ni suppression. Réponse create perdue→inspect même nom/autorité ; pas nouvel UUID.

Claim : état Created conforme, CID/autorité/spec immuables, aucune consommation préalable, expected_revision courant, verrou OS de contrôle détenu. Sous IMMEDIATE, inscrire start_issued_seq/start_operation_id/start_emitter_id, state=start_issued, quota retenu, révision+audit, commit. **EmitOnce uniquement pour cette nouvelle transition et ce broker possédant le verrou.** Replay exact→AlreadyConsumed ; divergence409 ; réponse initiale perdue→inspect/InDoubt. Ne pas renvoyer un ancien EmitOnce depuis worker_mutations comme une nouvelle permission. Aucun reset de start_issued_seq, nouvelle tentative ou lease expirée pour relancer la commande.

L'appel moteur reste hors transaction. La consommation SQL avant appel fournit au plus une émission permise, pas la preuve qu'elle est arrivée au moteur. Un crash entre les deux peut laisser un worker jamais exécuté mais consommé. Privilégier cette incertitude à un double effet. Aucun lease/epoch SQL ne fence un ancien CLI qui peut encore agir ; voir contrat runtime mono-hôte.

### Observations, inconnues et terminal

Observation typée : authority_id, runtime_kind, container_id, attempt_id/spec_hash, status/running, StartedAt/FinishedAt/exit_code optionnels, OOM, restart_count, imageID, limites et mounts inspectés, observed_at et source du broker/adaptateur. Pas de defaults transformant absence en zéro ; pas d'objet brut inspect contenant env/secrets. Une information provider reste declared et ne peut passer ici.

- Running seulement si running=true et identité/config/date cohérentes. StartedAt seul est une preuve historique de démarrage, pas d'activité courante.
- Première observation déjà exited : enregistrer démarrage historique et terminal comme événements distincts si nécessaires, mais la projection worker passe directement terminal dans une seule transaction ; aucun état Running actuel inventé.
- Échec attach, timeout du client, daemon inaccessible, CID absent, restart_count inconnu/non nul, StartedAt divergent ou limites non conformes : observation/incident/InDoubt, quota_held=1 et allocation inconnue. Pas d'expiration qui libère.

Terminal complet du même CID/autorité/instance : statut reconnu+running=false, dates présentes/ordonnées, exit réellement présent, restart_count=0 et limites inspectées identiques au spec ; calcul contrôlé des entiers. Une durée nulle n'est admise que si elle est réellement observée entre deux dates égales ; jamais par absence d'observation. La durée attach/durée configurée/horloge provider ne sert pas de source.

Transaction unique de finalisation : lookup idempotent observation/mutation → validation de transition → INSERT observation → calcul/INSERT allocation unique par attempt → worker terminal+quota_held=0+révision → événement terminal autoritatif portant observation, allocation et libération → mutation et refs → vérification → commit. Les étapes SQL peuvent être ordonnées pour satisfaire FK ; aucune n'est engagée séparément. Une divergence terminale pour le même attempt est409, pas une deuxième ligne ni une correction silencieuse. L'événement peut inclure les références des autres événements historiques `command.started/completed` créés dans cette même transaction, jamais un corps de message métier.

Cleanup seulement après reçu terminal durable. Erreur d'audit après observation : garder le conteneur arrêté et les preuves, stop de sûreté si nécessaire, slot retenu et reprise du même terminal. Échec cleanup après commit : incident distinct, aucune nouvelle durée/libération. Volume et export sont conservés, jamais suppression globale.

Stop : intention durable puis action broker sur le CID exact ; la gateway sans moteur ne l'exécute pas. Afficher attente du broker au lieu « arrêté ». En panne d'audit, une action de sûreté ciblée reste possible, mais aucun nouveau travail et aucun terminal/libération fictifs.

CancelReserved : uniquement avant création/claim possibles, sous transaction bloquant définitivement les claims futurs. Si claim a gagné la course, annulation refusée et slot retenu. NeverStarted porte un motif prouvé et aucune ligne d'allocation numérique inventée. Après StartIssued, un not-found n'est jamais NeverStarted. L'éventuelle clôture d'un Created non consommé nécessite une procédure de fencing/annulation approuvée avec runtime ; pas d'auto-libération dans ce premier contrat.

## 7. Projection ↔ audit : invariants obligatoires avant toute capacité

Les tables ne deviennent pas sûres parce que leur schéma est exact. Chaque transition interne après genesis inclut : version, operation/action, worker/attempt, input_hash, previous_revision, after_snapshot et snapshot_hash ; données d'observation/allocation lorsque créées. L'empreinte snapshot exclut ses références audit_seq/hash pour éviter un cycle ; les références sont celles du StoredEvent enveloppant.

- Startup, /integrity et changement externe data_version : vérifier v2 exact + chaîne + quotas audit/binding/genesis, puis rejouer les événements de contrôle typés **après genesis** et comparer ensemble exact des lignes/colonnes/compteurs/FK/empreintes aux projections. Lignes ajoutées/supprimées, slot remis libre, claim retiré, ledger modifié ou namespace inconnu→latch503, aucune réparation automatique.
- Chaque méthode interne, sur la même connexion : CAS révision, hash input, état avant, exact nombre de lignes affectées, postcondition snapshot/refs, append+commit checked. Pas de scan complet O(n²) obligatoire sur chaque polling ; les contrôles locaux préservent l'invariant, et le changement externe force la revalidation globale avant de marquer data_version validé.
- Les mutations libres de /events ne produisent jamais ces états. Les namespaces réservés restent refusés, même source=gateway/observed fournie par le client. Une évolution peut centraliser ce refus dans la méthode d'ingestion Store ; les méthodes de contrôle acceptent des DTO, pas un Event arbitraire.
- entities()/sandboxes restent des projections de visibilité historique. Quotas, worker status et ledger viennent des tables vérifiées, pas du texte d'un événement ou d'un compteur front.
- Les snapshots sont bornés pour rester dans la limite128KiB/event après masquage ; aucun polling200ms ne doit générer une transition persistante identique à l'infini. Persister les changements/observations métier pertinents, publier l'âge de la dernière observation durable. Rétention, audit plein et stratégie de reprise restent des obligations opératoires ; ne pas remettre la base à zéro pour retrouver de la capacité.

## 8. Migration explicite v1→v2 et ancien historique

Recommandation : commande de maintenance Rust explicite, gateway/brokers arrêtés et autorité locale quiescente ; pas de CREATE permissif à chaque démarrage. Préflight des conteneurs/sessions legacy sous l'autorité runtime : ne pas initialiser des quotas vides alors qu'un ancien worker non enregistré peut encore tourner. Aucune adoption implicite, arrêt ou suppression sans preuve/autorisation ciblée.

1. Verrou maintenance OS dans le contrôle de confiance ; vérifier chemin/inode/nlink comme P0. Schéma v1 exact, tenant binding, chaîne/quota sains ; version future ou partielle refusée avant mutation du mode/DDL.
2. Sauvegarde cohérente distincte et protégée avant migration : contrôle offline, checkpoint terminé sans busy, fermer SQLite, copie Rust vers destination create_new/no-follow, sync fichier+parent et vérification ; ne pas copier un WAL actif en supposant la cohérence. Qualification restauration à exécuter, pas prétendue acquise par la copie.
3. Reprendre IMMEDIATE, revalider v1 et head attendu. Créer seulement les tables/index v2 connus. Générer namespace serveur ; append `control.schema.migrated` avec ancien head, version cible et namespace ; créer control_meta référant son seq/hash ; tables workers vides. Fixer user_version2, vérifier schéma/projection/audit puis commit checked. Si audit plein/erreur/arrêt avant commit : rollback, v1 inchangé. Ne pas réutiliser une instance Store v1 vivante après migration.
4. Nouveau binaire reconnaît une liste exacte SCHEMA_V1 pour lecture/préflight migration et SCHEMA_V2 pour service de contrôle ; pas « version≤2 avec tables manquantes à réparer ». Le binaire v1 doit refuser une base v2. Binding record v1 reste inchangé, son numéro n'est pas confondu avec user_version global.
5. Les événements/IDs/seq/hashes antérieurs restent intacts. Les anciennes décisions restent visibles au contrat historique, sans origine garantie rétrospectivement. Le genesis exclut les anciennes traces portant un kind control ressemblant de la projection nouvelle. Une nouvelle admission fraîche après genesis vérifie réellement spec/politique ; elle seule peut donner une réservation/claim.

Un backup restauré peut précéder un start réellement émis : **restaurer ne rend jamais une autorisation à nouveau consommable par présomption**. Démarrer après restauration avec autorité d'émission désactivée, inventorier/réconcilier sous l'autorité runtime, puis procédure explicite. Un rollback de release n'est pas un downgrade en place. Aucune rotation de base qui perdrait workers/claims/ledger ou leurs refs audit.

## 9. Plus petit lot vertical réellement utilisable

Séquençage autorisé sans annoncer une fondation comme fonction livrée :

1. Contrats DTO/politiques et migration + stockage/projection/tests de concurrence ; endpoints admission/claim/observations/lecture. Feature de lancement nouvelle non activée sans intégration ; pas de registre de démo.
2. Runner : Authorization garde receipt complet ; RunPlan versionné persiste operation/session/tenant issu identity/autorité/specHash/worker+attempt+reçus, jamais secret. Lookup après réponse perdue ; même opération, aucun fallback ancien exécuteur. Le moteur ne reçoit plus un nom aléatoire régénéré à la reprise.
3. Préflight image/autorité et manifeste filtré avant admission, réservation avant create_volume/import/worker, identités helpers stables ; création/inspection conforme, claim sous verrou puis au plus une émission start sur réponse fraîche vérifiée.
4. Observer/réconcilier le même CID ; passage terminal/ledger/libération central avant cleanup. CLI `status`, `resume --reconcile`, `stop` utilisent ces vraies routes et exposent unknown/slot retenu ; pas de bouton sans effet ni réémission cachée.
5. Même vrai `run` répété après crash retrouve l'opération, pas une seconde instance ; fin crée une seule allocation. Cette preuve ferme le lot vertical de ce périmètre, pas les gates globaux. Le scénario trois workers/grants, cloud, charge100 et UI complète restent des exigences ouvertes.

## 10. Tests précis à écrire et preuves d'acceptation

Stockage/API sans moteur (preuves logiques seulement) :

- migration_v1_preserves_all_prior_hashes ; future_schema_refused_before_mutation ; interrupted_migration_rolls_back ; v2_missing_table_not_repaired ; backup_restore_does_not_reenable_start_without_reconciliation.
- admission_validates_exact_receipt_seq_hash_phase_and_tenant ; historical_forged_allow_cannot_authorize_v2_admission ; denied_or_review_receipt_not_executable ; launch_command/profile/session/network/resources/timeout_conflict ; known_secret_spec_refused_without_persistence.
- same_operation_same_input_same_receipt_after_restart ; changed_input_same_id409 ; same_request_other_worker_or_phase409 ; same_UUID_other_tenant_independent ; viewer/worker_mutations403.
- concurrent_admissions_two_connections/processes_at_count_cpu_ram_limit ; quota0_no_workers ; quota100_logical_boundary_only ; reducing_policy_below_held_does_not_release_existing ; no admission if audit full.
- start_claim_single_fresh_emit_concurrent ; identical_claim_replay_already_consumed ; lost_reply_never_reissues_emit ; old_revision409 ; cancel_reserved_races_claim_only_one_wins ; no reset after unknown/stop/terminal.
- terminal_observation_inserts_exactly_one_ledger_and_releases_once ; duplicated_terminal_same_receipt ; changed_dates/CID/limits409 ; wrong_authority/missing_exit/restart/changed_started_at/limits_unknown_in_doubt_and_held ; checked_decimal_products_no_JS_rounding_or_overflow.
- trigger/hardlink/schema/hash/quota_tamper_protections_still_pass ; externally_free_slot/reset_claim/delete_observation/modify_ledger_detected_against_audit ; mutation_replay_also_fail_closed ; untrusted_POSTevents_cannot_mint_control_projection.
- commit_failure_at_each_step_leaves_no_partial_state_or_false_ack ; cleanup_failure_after_terminal_does_not_duplicate_usage ; first_observation_already_exited_never_current_running.

Runtime réel distinct, par moteur/plateforme : vrai lancement via runner passe admission+claim ; deux brokers et ancien suspendu ne peuvent émettre un second start ; crash avant/après create/claim/start/terminalCommit/cleanup ; réponse claim perdue sans deuxième effet dans le volume ; job très court ; panne gateway/audit après exécution conserve conteneur arrêté puis réconciliation ; identité moteur changée→unknown ; limites inspectées utilisées dans les produits. Compter les appels/effets réels et conserver volumes/preuves ; un mock ne ferme pas ces essais.

Aucune de ces preuves v2 n'a été exécutée dans cette mission de conception. Leur statut est NOT_RUN, implémentation NOT_STARTED ; le correctif ingestion a ses propres21 tests PASS et n'est pas une preuve de registre.

## 11. Derniers arbitrages root nécessaires avant code v2

Les cinq choix structurants initiaux ont été arrêtés par root, pas à redemander. Restent des paramètres/procédures à figer :

1. Politique tenant/config : valeurs initiales explicites count/CPU/RAM/maxdurée, comportement de refus durable décrit ci-dessus et règle des révisions de politique ; ne pas déduire de capacité matérielle inventée. Recommandation : aucune API worker active sans politique configurée et sérialisée/empreintée.
2. Procédure NeverStarted après préparation/Created non consommé : premier lot limite cancel à Reserved ; une libération plus large exige clôture centrale de claims futurs + preuve de l'autorité exclusive et absence de start en vol. En l'absence de cette preuve : InDoubt, action opérateur explicite, pas TTL.
3. Détails authority_id/imageID/verrou/CLI en vol et références privées de credentials à stabiliser avec le contrat runtime. Le spec public doit pouvoir être repris sans conserver les valeurs de secrets dans la DB ; toute valeur non reconstructible entraîne une reprise refusée plutôt qu'un lancement approximatif.
4. Procédure maintenance/sauvegarde-restauration v2 et capacité de rétention avec états actifs : aucune suppression de l'audit actuel ni restauration autorisant un nouveau start par oubli. Les essais doivent précéder l'activation comme fonction durable.

Ces décisions ne requièrent aucun changement structurel de l'architecture retenue. Aucun engagement HA, Kubernetes, billing financier ou garantie exactly-once moteur n'est inventé.
