# Checkpoint broker V2 — contrat minimal proposé

Lecture seule sur `broker-service`, gel bootstrap identique à HEAD `3e2cffa86c0d0c4784507e78f289419a9e39890f`. Aucun code/test/runtime exécuté ou modifié. Contrat proposé à root ; les invariants nouveaux ci-dessous ne sont pas encore appliqués. Ce lot prépare le service réel sans refonte du runner.

## 1. Wire et compatibilité

Conserver intégralement `ExecutionIntentV1`, bindings, scope, accepted/current V1, RunPlan1/2/3 et `CheckpointPayloadV1`. `accept` continue d'écrire **rev0 V1**, exactement initial(intent). Nouvelle révision interne V2 : mêmes champs `operation_id,intent_hash,revision,previous`, `version:2`, `payload:BrokerCheckpointV2` ; hash SHA256 domaine `ai-guard:broker-checkpoint:v2\0` + JSON canonique exact. V1 garde son domaine et ses bytes existants.

Lecteur strict discriminé sur version **par révision**, directement depuis bytes ; aucune conversion persistante, aucun fallback V1 après erreur V2. Chaîne confirmée autorisée `V1+ V2*`, rev0 toujours V1 ; V2→V1 est corrompu. Versions/champs/nested inconnus refusés, y compris les champs ignorés par les types historiques : conserver la détection `serde_ignored` déjà utilisée par le payload runner. Aucun `Value` nouveau conservé.

## 2. Champs typés exacts

Déclarations de contrat, pas implémentation existante. Tous records/enums fermés Serde, versions exactes, champs wire privés ; getters étroits et Debug opaque. Les types non redéfinis sont les vrais DTO actuels de `control`, `model`, `runtime` et `runner`.

```rust
struct BrokerCheckpointV2 {
    version: u32, // exactement 2
    runner: CheckpointPayloadV1,
    supervision: SupervisionCheckpointV1,
}
struct SupervisionCheckpointV1 {
    version: u32, // exactement 1
    control_namespace_uuid: Uuid,
    helpers: Vec<HelperPreparationV1>,
    operations: Vec<SupervisionOperationV1>,
}
struct HelperPreparationV1 {
    policy: HelperExecutionPolicyV1,
    permission: HelperPermissionV1,
    owned: Option<OwnedContainerSnapshot>,
}
struct DecisionAnswerV1 {
    decision: Decision,
    receipt: DecisionReceiptV1,
}
enum SupervisionOperationV1 {
    Decision {
        request: DecisionRequest,
        response: Option<DecisionAnswerV1>,
    },
    Admission {
        request: SupervisedAdmissionV1,
        response: Option<AdmissionResultV1>,
    },
    WorkerMutation {
        request: WorkerMutationV1,
        response: Option<MutationResultV1>,
    },
    Arm {
        parent: WorkerV1,
        owned: OwnedContainerSnapshot,
        request: ArmV1,
        response: Option<ArmResultV1>,
    },
    WorkerClaim {
        request: ArmedStartClaimV1,
        response: Option<MutationResultV1>,
    },
    HelperClaim {
        request: HelperStartClaimV1,
        response: Option<HelperClaimResultV1>,
    },
    HelperObservation {
        request: HelperObservationV1,
        response: Option<MutationReceiptV1>,
    },
    HelperCleanup {
        request: HelperCleanupV1,
        response: Option<MutationReceiptV1>,
    },
    Export {
        request: ExportRequestV1,
        response: Option<MutationReceiptV1>,
    },
}
```

L'enum wire utilise `#[serde(tag="kind",rename_all="snake_case",deny_unknown_fields)]`. `response:None` signifie **réponse inconnue**, pas « RPC non envoyé ». La requête exacte doit être confirmée durable avant RPC ; pas d'UUID, clock_check ou deadline régénérés au retry. La première réponse validée est conservée intégralement et devient immuable. Un résultat rejeté d'admission reste réel (`accepted=false`, refusal, worker=None), jamais ACK fabriqué.

Cette liste contient les seules opérations réellement nécessaires : décision/revue, admission, transitions worker ordinaires, armements/claims et helpers/export. Pas d'outbox abstraite, d'erreur HTTP libre, de booléen `start_permitted`, de snapshot de rôle actif ou de lease restaurable. `DecisionAnswerV1` est la projection typée décision+receipt de la réponse existante, sans recopier les détails Jev libres. Les Value déjà historiques du payload runner ne sont ni étendus ni utilisés comme nouveau canal de supervision.

Préparer un helper exige son vrai `HelperExecutionPolicyV1` avant `create`, pour figer helper_id/nom/argv/volume même si create perd son ACK. `owned` passe une seule fois None→Some après inspection ; l'Arm doit porter exactement ce snapshot. La politique est comparée au résultat réel de `helper_execution_policy_v1(parent,purpose,helper_id)` ; aucun argv helper libre. Ce record prépare Import, ImportVerification, FinalScan et Export, pas seulement l'import legacy.

Bornes proposées fixes : 128 opérations, 32 helpers, 64 réponses inconnues maximum ; décision au plus deux phases et admission unique. Une opération est ajoutée sans allocation au plafond ; cap atteint = refus de nouvelle transition, jamais suppression de preuve. Envelope complet ≤32MiB, caps bytes/entries/revisions existants inchangés. Ce sont des bornes applicatives, pas qualification100. Aucun poll500ms ne crée une révision.

## 3. Contexte selon les faits disponibles

Le validateur reçoit intention, scope et manifeste du journal ; ce contexte est une assertion de données, **pas une identité authentifiée**.

- Namespace V2 égale scope ; tenant/origin restent ceux du scope. Chaque receipt d'armement présent doit avoir ce namespace exact. Tous request ids/UUID/hash/versions/bornes sont vérifiés par les validateurs DTO existants puis les liens ci-dessous.
- **Avant admission :** runner.managed peut être None et Admission pending présente. Valider SupervisedAdmissionV1, intent_hash, opération d'admission=intent.operation_id, session, demande courante exacte, options LaunchSpec et credential_binding_id/names du manifeste. Aucun Worker/attempt n'est exigé ou inventé. L'archive ou la préparation runtime absente reste une préparation incomplète, pas une permission d'import/start.
- **Après réponse acceptée :** le premier WorkerV1/attempt vient exclusivement du vrai AdmissionResultV1. Son admission/request/spec/receipt sont recoupés ; si managed existe, il porte la même admission et le worker courant de cette identité. Les opérations suivantes tirent worker/attempt de cette liaison, jamais d'un identifiant libre. Refusal n'autorise aucun helper/claim.
- Les opérations WorkerMutation refusent StartClaim legacy. Leurs résultats restent liés à la requête exacte et au reçu/hachage existant ; le worker peut ensuite avancer sans invalider les snapshots historiques.
- L'Arm conserve le **vrai parent au moment de l'armement** : `ArmV1::validate_for_parent` exige sa révision, son état et l'absence de claim. Ne pas lui substituer le worker courant après claim. Valider owned/CID/session/authority/config/image/policy contre ce parent et RuntimeAuthority disponible. Worker handle.operation=intent.operation_id ; helper handle.operation=helper_id, distincts de l'UUID attempt central.
- Claims : receipt exact d'un Arm acquitté du même checkpoint, même sujet/identités, clock_check validé contre sa clock/deadline. Le résultat conserve le vrai disposition ; **EmitOnce persisté n'est jamais un permis rechargeable**. Un ACK perdu peut produire AlreadyConsumed lors du retry ; il n'est jamais converti en EmitOnce.
- HelperObservation/Cleanup : helper préparé, claim et arm correspondants ; inspection exacte. ImportVerification référence l'import terminal acquitté et son manifeste réel attendu/observé. Export référence une ExportRequest acquittée, destination_ref et destination_intent_hash exacts. Receipt lié par operation_id/input_hash/audit_seq/audit_hash ; forme valide ne prouve pas authenticité centrale.

Un pur contexte typé runner donne les champs privés nécessaires sans les rendre publics ni reconstruire un faux ManagedCheckpoint. Par exemple `CheckpointPayloadV1::validate_broker_admission_context(intent, admission, binding_id, names) -> JournalResult<()>` valide le cas managed=None à partir des vrais champs disponibles ; le chemin managed=Some garde `validate_journal_context`. Aucune IO/résolution provider/écriture de plan dans ces méthodes.

Pour le nouveau parcours V2, pending/uncertain/import legacy du payload managed restent vides : les opérations et helpers ci-dessus possèdent ces faits. Aucun double pending contradictoire. L'import complet conserve son reçu réel et sa vérification avant CreateIssued/claim worker ; les vieux flags start_issued/completed ne les remplacent pas.

## 4. Transitions et durabilité

Promotion V1→V2 autorisée seulement pour un journal encore sans managed legacy (request approuvée possible), afin de ne pas adopter implicitement une session/claim historique. Aucun changement du fichier V1 original ; anciens journaux gérés restent lisibles V1 et exigent une migration dédiée avant exécution nouvelle.

Entre V2 : namespace constant ; helpers/operations en préfixe conservé. Requêtes, parent et snapshots déjà observés immuables ; réponse None→Some une seule fois ; réponse acquittée jamais effacée/remplacée. `human_approved=true` ne redescend pas ; hash exact initial conservé. Les dépendances peuvent référencer seulement des opérations antérieures acquittées ; cycles/duplicates/divergences refusés. Plusieurs réponses inconnues sont représentables pour conserver l'incertitude, sans nouveau grant.

`advance_supervised(operation_id, expected, &BrokerCheckpointV2)` garde le CAS réel revision/hash. Hash candidat couvre **tout** V2. Un replay exact expected+payload confirmé retourne son reçu original même après révisions suivantes ; comparer ce replay avant validation de transition contre la tête courante. Nouvelle mutation : expected=head, validation contre la précédente, création exclusive, fsync révision+parent, pointer temp synchronisé, publication contrôlée, fsync parent, revalidation/resync graphe et plancher vivant avant reçu. Orphelin jamais promu/écrasé ; mêmes caps, flock et mutex.

**Décision forte root :** dès que la tête confirmée est V2, ancien `advance` V1 renvoie Conflict avant tout no-op ou replay historique, même runner identique. Ce refus d'API après promotion est délibéré ; les données/hashes V1 restent lisibles. V2→V1 dans la chaîne refuse également. Le plancher vivant reste partagé entre clones ; aucun anti-restore après restart supplémentaire n'est promis.

Aucun receipt de journal/chargement ne donne start, arrêt, quota libéré ou autorité fraîche. Le futur service authentifie avant profil/job/FS et recoupe contrôle/runtime. Le futur scheduler devra extraire sa cible immuable hors writer/mutex/HTTP avant claim. Ce format ne livre ni ordonnanceur, ni horloge consommée, ni stop hors panne, ni fencing D5, ni TTY ; une ambiguïté garde quota/CID et ne devient jamais terminal zéro.

## 5. Fichiers et cycle court proposé

1. **Nouveau `src/broker/checkpoint.rs`**, export ciblé mod.rs : types fermés, bornes, canonical_bytes/from_bytes, validate_context et validate_transition ; validation pure et Debug opaque. Couture minimale dans runner.rs seulement pour vérifier l'admission avant managed ; aucun remaniement execute_plan/PlanState/helpers dans ce lot.
2. **`src/broker/journal.rs` / `journal/types.rs`** : enum interne de révision V1/V2 et digest par version, loader mixte, advance_supervised partageant la publication actuelle, garde anti-downgrade de advance. AcceptedData conserve un enum checkpoint chargé ; `checkpoint()` renvoie toujours la vue runner V1, ajouter `broker_checkpoint_v2() -> Option<&BrokerCheckpointV2>`. Aucun nouveau fichier persistant annexe ni migration automatique.
3. **Tests Luna ciblés**, puis revue indépendante : cible `tests/broker_checkpoint.rs` pure et compléments journal dans fichiers possédés Luna. Fixtures tirées du vrai Store3/admission/arm/claims/helpers, sans moteur/modèle ; snapshots runtime synthétiques explicitement nommés.

RED utiles : rev0/hash1 inchangés ; pending admission avec managed=None ; quota refusé ; chaîne mixte/load/replay original V2 après révisions suivantes ; même runner mais arm/deadline divergents Conflict ; advance1 après2 refuse même exact ; downgrade en milieu de chaîne/corruption/unknown nested ; approbation préservée ; worker/attempt/namespace/owned/helperpermission mismatches ; vieux parent d'Arm reste valide après worker avancé ; requête/receipt acquis non effaçables ; cap et canaris secrets opaques ; orphelins/panne aux barrières réelles déjà instrumentées ; rollback sous plancher clone. Pas de duplication de toute la matrice journal ni fausse preuve runtime/power-loss.
