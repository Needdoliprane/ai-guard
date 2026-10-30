# Verticale broker permanent — cartographie et contrat à intégrer

Base lue : `537f8d2c072139bb4ced0f3b7bd385f15fb1e9dd`, worktree `broker-supervision`, 30 octobre 2026. Aucun fichier du dépôt édité, aucun test/service/moteur/modèle lancé. AGENTS/RTK, architecture, règles, gates, stratégie, BROKER_SUPERVISION/BROKER_JOURNAL et contrats tenants/usage lus. Les signatures proposées ne sont pas du code livré.

Objectif : une commande batch explicite dont le vrai worker continue après SIGKILL du CLI et conserve son échéance. `run` historique et schéma2 restent les défauts jusqu'à qualification batch, helpers et gardien TTY. Pas d'installation OS, cloud, moteur direct, PATH ambiant ou socket worker.

## 1. Coutures réellement présentes

| Fonction/type | Emplacement exact | Adaptation indispensable |
|---|---|---|
| `OperatorCommand::Run` → `runner::run` | `src/operations.rs:85,506` | Ajouter commandes broker explicites ; aucun fallback/auto-spawn. |
| `RunOptions`, `RunPlan` strict1/2/3 | `src/runner.rs:21,310` | Garder formats/hashes historiques ; aucun champ supervision silencieux. |
| `choose_plan` | `src/runner.rs:837` | Canonicalise/ouvre projet/config avant auth ; fournit UUID nouveaux :1031. Appel uniquement après auth + ticket New ; opération Submit imposée avant projection. |
| Approbation/résolution3 | `src/runner.rs:890,934,958` | L'approbation existante recharge pending ; nouveau daemon approuve l'intention acceptée, sans stdin ni nouvelle résolution. |
| `freeze_provider_credential`, `validate_provider_plan`, `frozen_provider_credentials` | `src/runner.rs:1200,1238,1274` | Conserver binding3/id/digest ; runtime nouveau lit le binding durable du journal, jamais source/providers.json/Auto après ACK. |
| Parseurs purs credentials | `src/runner.rs:1170,1321` | Fournisseur : record exact → env minimal ; manuel : env canonique. `control_secrets` :1381 reste hors worker. |
| `AuditClient` | `src/runner.rs:84,145,186,206` | Origine HTTPS/loopback, client borné sans proxy/redirect, même credential présenté conservé jusqu'aux RPC. |
| `check_identity` | `src/runner/managed.rs:498` | `/api/v1/identity` n'a que tenant/role (`src/api.rs:690`). Nouveau parcours exige `/api/v1/control/protocol` pour namespace et capacité. |
| `run` → `execute_plan` | `src/runner.rs:1407`; `src/runner/managed.rs:1231` | Legacy effectue préparation/credentials avant auth, écrit sessions, appelle reservations legacy. Extraire des opérations étroites ; ne pas simplement lancer cette fonction dans un daemon. |
| Autorité/invocation | `managed.rs:648,832`; `src/runtime.rs:559,570` | Réutiliser programme approuvé, environnement reconstruit, invocation ancrée et autorité vérifiée. |
| `ManagedCheckpoint`, `PlanState::update` | `managed.rs:13,407,431` | Pending est seulement WorkerMutationV1 ; update tient son mutex pendant write/fsync. Nouveau état broker distinct ; aucun stop ne passe par ce mutex. |
| Écriture privée legacy | `managed.rs:118,161,165,174`; `src/runner.rs:776` | Temporaire/fsync/rename du fichier mutable ; aucun dual-write sessions+Journal pour le nouveau parcours. |
| Admission/préparation | `managed.rs:1271–1282,1321,1354` | Décision, archive filtrée, LaunchSpec réutilisables ; POST devient SupervisedAdmissionV1. Binding manuel fixé par manifeste journal, jamais nouveau UUID :1314. |
| `launch_worker`/callback | `managed.rs:1482,1507,1545` | Created existant puis claim legacy ; insérer arm durable + scheduler ready + claim armé avant EmitOnce. |
| Import/scan/export | `managed.rs:1684,1745,1767,1846,2672` | Helpers actuels émettent événements génériques sans arm/claim central. Tous doivent employer politique fixe et vrais reçus. |
| Runtime lifecycle | `src/runtime/managed.rs:830,890,920,977` | `execute_retained/start_owned_retained` inspectent puis callback FnOnce retournant EmitOnce/ObserveOnly : couture disponible pour armement. |
| Timeout local | `src/runtime.rs:889,919,946,954` | `start --attach`, Instant et kill sont dans l'appelant ; callback HTTP peut bloquer ce délai. Scheduler indépendant requis. |
| Stop exact | `src/runtime/managed.rs:749,763,798` | `stop_snapshot_owned` valide CID/config/image/autorité, inspecte/kill/réinspecte. Réutilisable ; ne prouve aucun fencing D5. |
| Terminal/cleanup | `managed.rs:1919,1992,1994,1998` | Terminal+allocation+quota+audit central avant cleanup. Observation absente reste inconnue. |
| Reprise/stop legacy | `managed.rs:2089,2130,2197,2337,2559,2595` | load_session lit FS avant auth ; stop attend FS/HTTP. Ne pas exposer ces fonctions comme IPC ou stop d'échéance. |

`managed.rs` ci-dessus signifie `src/runner/managed.rs`.

Journal : `src/broker/journal.rs:196,279,352,482,491` expose open/reserve/accept/load/advance ; writer partagé :90–106 ; révision fermée V1 :132. Aucun inventory public, verrou global installation, IPC, scheduler ou ArmTarget livré. Scope (`journal/types.rs:80`) est une assertion trusted, jamais preuve tenant.

Horloge : `src/broker/clock.rs:49,64,159,207` fournit HostClock, NativeHostClock et checked_deadline ; consommateur absent. Deadline immuable, même boot/méthode, pas UTC/now+timeout.

API supervision réelle : `src/api/supervision.rs:10–20` : GET `control/protocol` ; POST `workers/supervised-reservations`, `{id}/arms`, `{id}/armed-start-claims`, `{id}/helper-start-claims`, `{id}/helper-observations`, `{id}/helper-cleanup`, `{id}/export-requests` ; GET `{id}/arms/{arm_id}`. DTOs stricts : `src/control/supervision.rs:53,63,121,144,155,175,184,193`. Store raccordé : `src/audit/supervision.rs:1086,1125,1146,1162,1215,1232,1242,1252`. Runner ne les consomme pas actuellement.

## 2. Décisions root confirmées et contrat minimum

### Identité physique

Conserver le producteur : worker_spec met `ContainerSpec.operation=plan.operation_id` (`managed.rs:1118`), repris dans ContainerHandle (`runtime/managed.rs:870`). attempt_id est un UUID central distinct (`audit/control.rs:746`). Root corrige le validateur nouveau `runner.rs:644` vers `intent.operation_id()`, avec régression produite et négatif attempt_id ; aucun ancien handle/start réécrit.

### Gateway frais3 explicite

Store neuf reste2 (`src/audit.rs:454–456`). migrate_control_v3 existe (`audit/control.rs:1059`), activation seulement origine fraîche intacte :1140 ; réouverture maintenance-only (`audit.rs:485–486`, `audit/supervision.rs:947`). CLI migrate appelle seulement v2 (`main.rs:337`).

Ajouter bootstrap gateway opt-in entre open et première lecture opérationnelle (`main.rs:278–284`), contrôlé dans bibliothèque par created_here+empty/fresh_origin, avec vrai backup exclusif synchronisé. Refuser DB préexistante/restaurée avant migration, pas seulement constater active=false ensuite. Défaut2 et maintenance des stores existants inchangés. Un restart gateway n'autorise pas de nouveaux claims ; observation/stop/réconciliation restent possibles.

### Checkpoint broker V2

Payload V1 réel est `{version:1,current_request,managed}` (`runner.rs:350–360,452`) ; il manque armements, receipts, claims/helpers typés. Nouvelle révision broker V2 enveloppe ce payload intact avec `SupervisionCheckpointV1`. Lecteur discriminé strict1/2 ; hashes/domaine1 inchangés, domaine2 explicite. Aucun Value libre.

SupervisionCheckpoint contient namespace+intent_hash, reçu admission, pending enum fermé (SupervisedAdmission/Arm/ArmedStartClaim/HelperStartClaim/HelperObservation/HelperCleanup/ExportRequest), exécutions bornées avec owned exact, ArmV1/receipt, claim/receipt, terminal/cleanup. Mutations worker ordinaires gardent leur pending réel ; aucune duplication contradictoire.

Propositions d'interface :

- `Journal::advance_supervised(operation_id: Uuid, expected: RevisionRefV1, next: &BrokerCheckpointV2) -> JournalResult<CheckpointReceiptV1>` : mêmes CAS/fsync/caps/plancher/replay ; advance V1 refuse de perdre une supervision déjà V2.
- `AcceptedJobV1::supervision_checkpoint() -> Option<&SupervisionCheckpointV1>` ; checkpoint() conserve sa vue runner V1.
- `Journal::inventory() -> JournalResult<Vec<JobInventoryEntryV1>>` borné, UUID fermés, partials explicites, aucun grant.
- `runner::prepare_broker_submission(options: RunOptions, operation_id: Uuid) -> Result<PreparedSubmission>` uniquement après reserve New.
- `runner::restore_broker_plan(intent: &ExecutionIntentV1, payload: &CheckpointPayloadV1) -> Result<BrokerRunState>` pur, current_request approuvé conservé, aucune source/Auto/écriture sessions.
- `runner::broker_credentials(job: &AcceptedJobV1, config: &Directory) -> Result<Option<BrokerCredentials>>` uniquement binding durable ; stop sans clé fournisseur.

### Profils/auth et scheduler

`ControlRouter::authenticate(PresentedRunnerCredential) -> Result<AuthenticatedControl>` async : index mémoire route vers origin approuvé ; même AuditClient appelle protocol, valide runner+tenant+namespace **avant profil/job/FS**. Puis `AuthenticatedControl::open_profile(profile_id) -> Result<AuthenticatedProfile>`. Réconciliation autonome utilise une référence distincte revalidée ; jamais une clé du profil pour remplacer le demandeur.

`BrokerService::submit(control, profile, RawSubmitV1) -> Result<SubmitReplyV1>` : frame stricte bornée, token séparé du payload exact ≤256KiB, reserve avant source ; Accepted rejoue, partial refuse, seul New prépare. `approve/status/request_stop` refont auth frais ; aucune lecture stdin. Socket/parents/inode privés, peer UID noyau, connexions/délais bornés ; UID seul ne prouve pas tenant.

`ArmedStopTarget` privé immuable possède runtime ancré, owned/CID/config, arm/receipt, boot/deadline ; aucun AuditClient/Journal/PlanState/mutex writer/credential fournisseur. `DeadlineSupervisor::register(target) -> Result<StopRegistration>` réserve capacité dédiée ; `recheck_before_start(registration, sample)` refuse expiration/régression/boot différent. `stop_exact()` appelle seulement stop_snapshot_owned.

Callback on_created : owned exact → requête arm privée durable → RPC arm → receipt validé → installation immédiate cible scheduler → receipt privé durable → claim exact durable → RPC claim → réponse durable → recheck → EmitOnce frais seulement. ACK perdu rejoue même opération/clock/deadline ; replay claim reste ObserveOnly. Scheduler sur thread/runtime dédié, slots stop réservés, tick≤250ms proposé ; enqueue try_send borné, sans attendre writer/fsync/HTTP. Réviser seulement transitions sémantiques : refresh500ms actuel (`managed.rs:534,1608`) épuiserait256révisions.

Import obligatoire : Import armé/claimé/terminal engagé, puis ImportVerification armé (permission liée au reçu import), tar réel/scanner Rust/manifeste égal et terminal engagé. Store l'exige avant CreateIssued/claim (`audit/supervision.rs:489,647,1259`). FinalScan/export sont aussi armés. D5 absent : Created/absence/rm/timeout n'autorisent aucune libération ; garder CID, enforcement, in_doubt/quota.

## 3. Trois propriétaires, ordre d'intégration

1. **A broker** : `src/broker/{mod,journal}.rs`, journal/types, nouveaux service/ipc/scheduler/checkpoint ; CP2/inventory/auth/cycle indépendant connexion. Pas d'édition runner.
2. **B runner/runtime** : runner.rs, runner/managed.rs et sous-modules, runtime seulement si nécessaire ; préparation/reconstruction, RPC typés, helpers, arm/claim hooks, terminal/usage. Pas d'édition journal.
3. **C commandes/bootstrap** : main.rs, operations.rs, gateway_config.rs, garde minimale audit/control si nécessaire ; serve/submit/status/approve/stop explicites et fresh3 opt-in. Défaut inchangé.

Intégration séquentielle : fixer identité et figer CP2/bootstrap → RED/logiciel CP2+auth/submit sans exécution → scheduler+helpers+worker raccordés → revue indépendante → un Docker batch réel puis trois workers. Luna exécute les tests ciblés, hors fichiers possédés par implémenteurs ; pas de matrice fournisseur.

## 4. Qualification ultérieure, non exécutée ici

Image/digest et programme Docker absolu déjà disponibles, contrôle conteneurisé, bases A/B privées neuves, service foreground distinct, harnais Rust possédant enfants/preuves ; aucun pull/modèle. A1/A2 chez A, B1 chez B : vrais import/vérification/worker ; SIGKILL CLI après ACK, effet métier après mort, même deadline/CID. Témoin non expiré intact, isolation A/B, une allocation billable=false par tentative.

Mesurer séparément tick≤250ms, déclenchement≤1s après deadline et terminal≤15s en nominal. Tester gateway absent, writer/fsync/mutex bloqués après armement : stop continue, quota retenu avant terminal engagé, replay exact après retour. Perte ACK/source tournée/binding altéré, faux rôle/tenant/namespace, socket substitué, grosses frames : aucun fallback/FS non autorisé. Restart daemon conserve deadline et claims ; restart OS/gestionnaire/gel et fencing D5 restent à qualifier séparément. Export neuf, original inchangé, aucun secret/socket au worker.

Statut : cartographie livrée ; tests/runtime NOT_RUN ; G09 et qualification TTY/Podman/Linux/100workers restent ouverts. Aucun gate fermé.
