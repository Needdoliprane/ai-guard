# Journal privé du broker — contrat d'implémentation v1

Base `3372150fdb99d8bc71700a33dd1d49c0e75c45b5`, worktree exclusif broker-supervision. Choix techniques de l'orchestrateur dans la [direction déjà approuvée](BROKER_SUPERVISION.md). Composant implémenté avec [preuves ciblées et limites](evidence/2026-10-30/broker-journal/README.md) ; le raccord au CLI/service et tout droit d'exécution restent hors de ce lot. Le premier jet privé reste conservé ; ce contrat remplace ses choix ouverts. RED, implémentation et revue restent suivis dans [IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md).

## Périmètre réel

Core : `src/broker/journal.rs` et sous-modules hors tests, export minimal `src/broker/mod.rs`, couture pure bornée `src/runner.rs`, parseur fournisseur pur `src/provider.rs`, helpers métier étroits `src/control_fs.rs` / `src/control_fs/persistent.rs`. Seul delta runtime : visibilité crate du getter de version existant dans `src/runtime/managed.rs`. Luna : `tests/broker_journal.rs`, `tests/journal_sqlite.rs` et `src/broker/journal/tests.rs` cfg(test). Aucun changement Store/migration/CLI/IPC/scheduler/activation ni détournement des primitives SQLite.

Checkpoint public opaque défini côté runner : données réelles `{version:1,current_request:DecisionRequest,managed:Option<ManagedCheckpoint>}` ; ManagedCheckpoint reste privé. Aucun Value libre ni copie mutable de RunPlan comme identité. Options/provider_input/provider_credential ne figurent pas dans le payload ; projection compare ces données stables à l'intention avant de retenir demande courante et checkpoint réel. Elle ne rappelle jamais intent.from_plan_bytes après approbation et ne remet pas approval à zéro.

## Signatures exactes pour RED

```rust
pub type JournalResult<T> = Result<T, JournalError>;
pub struct Journal { /* Clone partage Arc, FD flock, Mutex writer, limites */ }
pub struct JournalScopeV1 { /* champs wire privés ci-dessous */ }
pub struct FsExclusions { /* identités connues ; non-Serde */ }
pub struct ControlSecrets { /* Clone, non-Serde, Debug fixe, littéraux privés */ }
pub struct JournalLimitsV1 { /* trusted config privée ; non-Serde */ }
pub struct ReservationTicket { /* non-Clone/non-Serde, Arc lease privé */ }
pub enum ReserveResult {
    New(ReservationTicket), InProgress, Accepted(AcceptedJobV1), Interrupted,
}
pub enum BindingInputs<'a> {
    None, Manual { source: &'a PrivateFile }, Provider { source: &'a PrivateFile },
}
pub struct AcceptedJobV1 { /* données chargées et validées, aucun grant */ }
pub struct BindingManifestV1 { /* record fermé, champs privés */ }
pub enum BindingKindV1 { None, Manual, Provider }
pub struct CheckpointReceiptV1 { /* opération et tête publiée */ }
pub struct RevisionRefV1 { /* revision:u64, hash:String ; getters seuls */ }
pub struct CheckpointPayloadV1 { /* wrapper runner réel, serde strict/opaque */ }

impl JournalScopeV1 {
    pub fn new(installation_id: Uuid, tenant_id: &str, gateway_origin: &str,
        control_namespace_uuid: Uuid, profile_id: &str) -> JournalResult<Self>;
}
impl FsExclusions {
    pub fn new() -> Self; // vide valide ; Default équivalent
    pub fn add_known_file(&mut self, parent: &Directory, name: &str) -> JournalResult<()>;
}
impl ControlSecrets {
    pub fn new(values: Vec<String>) -> JournalResult<Self>;
}
impl JournalLimitsV1 {
    pub fn new(job_bytes: u64, job_entries: u32, max_revisions: u32,
        scope_bytes: u64, scope_entries: u32, max_jobs: u32) -> JournalResult<Self>;
    // Default : 256MiB/512/256 ; 1GiB/65536/256, hard max distincts ci-dessous.
}
impl CheckpointPayloadV1 {
    pub fn initial(intent: &ExecutionIntentV1) -> JournalResult<Self>;
    pub fn from_plan_bytes(intent: &ExecutionIntentV1, bytes: &[u8]) -> JournalResult<Self>;
    pub fn from_bytes(bytes: &[u8]) -> JournalResult<Self>;
    pub fn canonical_bytes(&self) -> JournalResult<Vec<u8>>;
}
impl RevisionRefV1 {
    pub fn new(revision: u64, hash: &str) -> JournalResult<Self>;
    pub fn revision(&self) -> u64;
    pub fn hash(&self) -> &str;
}
impl Journal {
    pub fn open(root: &Directory, scope: JournalScopeV1, exclusions: &FsExclusions,
        limits: JournalLimitsV1, control_secrets: ControlSecrets) -> JournalResult<Self>;
    pub fn reserve(&self, operation_id: Uuid, submit_bytes: &[u8]) -> JournalResult<ReserveResult>;
    pub fn accept(&self, ticket: ReservationTicket, intent: &ExecutionIntentV1,
        bindings: BindingInputs<'_>, checkpoint0: &CheckpointPayloadV1) -> JournalResult<AcceptedJobV1>;
    pub fn load(&self, operation_id: Uuid) -> JournalResult<Option<AcceptedJobV1>>;
    pub fn advance(&self, operation_id: Uuid, expected: RevisionRefV1,
        next: &CheckpointPayloadV1) -> JournalResult<CheckpointReceiptV1>;
}
impl AcceptedJobV1 {
    pub fn operation_id(&self) -> Uuid;
    pub fn intent(&self) -> &ExecutionIntentV1;
    pub fn intent_hash(&self) -> &str;
    pub fn submit_hash(&self) -> &str;
    pub fn binding_manifest(&self) -> &BindingManifestV1;
    pub fn binding_source(&self) -> Option<&PrivateFile>;
    pub fn head(&self) -> &RevisionRefV1;
    pub fn checkpoint(&self) -> &CheckpointPayloadV1;
}
impl BindingManifestV1 {
    pub fn kind(&self) -> BindingKindV1;
    pub fn binding_id(&self) -> Option<Uuid>;
    pub fn names(&self) -> &[String];
    pub fn source_sha(&self) -> Option<&str>;
    pub fn stored_sha(&self) -> Option<&str>;
    pub fn handoff_sha(&self) -> Option<&str>;
    pub fn stored_bytes(&self) -> u64;
    pub fn file_name(&self) -> Option<&str>;
}
impl CheckpointReceiptV1 {
    pub fn operation_id(&self) -> Uuid;
    pub fn head(&self) -> &RevisionRefV1;
}
```

JournalError sans données privées : `InvalidInput, UnsupportedVersion, TooLarge, LimitExceeded, Busy, Conflict, Interrupted, Corrupt, UnsafeFilesystem, ControlSecret, Io`. Display/Debug fixes ; aucun path, clé, env, valeur serde ou détail OS propagé publiquement. Records JSON stricts Serde avec erreur statique opaque, Debug fixe ; AcceptedJobV1 et les handles FD ne sont PAS Serialize/Deserialize. BindingKindV1 est Clone+Copy+Debug+PartialEq+Eq ; BindingManifestV1 n'expose aucun secret. None renvoie binding_id/file_name/digests=None, names vide, stored_bytes=0 ; binding_source=None. Les autres kinds renvoient la copie durable validée, pas le PrivateFile source initial. `load(None)` seulement ENOENT du job, jamais corruption/permission refusée.

Scope wire : `{version:1,installation_id:Uuid,tenant_id:String,gateway_origin:String,control_namespace_uuid:Uuid,profile_id:String}`. UUID non nil, tenant selon validateur tenancy existant, profile1..80 ASCII `[A-Za-z0-9_-]`, origine HTTPS ou HTTP loopback sans userinfo/query/fragment/path. Assertion du caller trusted, jamais preuve tenant/role ; futur IPC authentifie frais avant profils/jobs/FS. FsExclusions collecte metadata-only sans open au plus64 identités dev/ino de fichiers privés réellement connus par embedding ; vide valide, aucun scan/détection universelle SQLite inconnu nlink1.

ControlSecrets : liste vide valide ; <=64 valeurs UTF-8 non vides1..4096bytes, total<=256KiB, exactes sans normalisation ; seuls secrets contrôle fournis par caller trusted, jamais dictionnaire étendu aux clés fournisseur. Comparaison LITTÉRALE, aucun Redactor général/regex/troncature. Vérifier bytes et clés/strings décodées des métadonnées/payload avant write ; binding fournisseur inspecte aussi secrets/env_bytes décodés. Refus sans modification, persistance ou diffusion de la valeur. Pas de getter/serde de valeurs, aucun DLP promis pour encodages arbitraires ; vrais credentials fournisseur autorisés dans binding privé.

## Formats persistants fermés

Racine durable retenue → `broker-journal-v1/<scope_hash>/` ; scope hash SHA256(`ai-guard:broker-journal-scope:v1\0` + JSON canonique scope). `.writer.lock`0600 retenu/flock exclusif non bloquant ; `scope.json` immutable ; `jobs/<operation-UUID-canonique>/`. Noms générés, aucun segment source/caller. Canonique : clés triées récursivement, tableaux/texte exacts ; SHA256 hex minuscules64, domaines séparés terminés par NUL.

- `reservation.json` : `{version:1,scope_hash,operation_id,submit_hash}`, domaine `ai-guard:broker-reservation:v1`.
- `intent.json` : canonical_bytes ExecutionIntent exacts, domaine/hash existants inchangés.
- `bindings.json` : `{version:1,operation_id,intent_hash,binding:BindingManifestV1}`, domaine `ai-guard:broker-bindings:v1`.
- BindingManifest fermé : `{"kind":"none"}` ; ou `{"kind":"manual","binding_id":Uuid,"file_name":"manual-env-v1.bin","names":[String],"source_sha256":H,"stored_sha256":H,"handoff_sha256":H,"stored_bytes":N}` ; ou `{"kind":"provider","binding_id":Uuid,"file_name":"<UUID>.json","names":[BOOTSTRAP_ENV],"source_sha256":H,"stored_sha256":H,"handoff_sha256":H,"stored_bytes":N}`. Aucune valeur Env/secret dans métadonnées.
- `bindings/manual-env-v1.bin` : ENV CANONIQUE préparé depuis bytes du PrivateFile source retenu, même parseur pur extrait de credentials_from_source ; commentaires/lignes ignorées absents, ordre des noms et règles historiques inchangés. UUID manuel minté UNE FOIS ; source_sha=SHA(raw source), stored_sha=SHA(env canonique), handoff_sha=stored_sha ; runtime futur lit seulement copie durable, aucun retour source ni Snapshot unlink-on-drop.
- `bindings/<UUID>.json` fournisseur3 : record original copié EXACTEMENT ; binding_id/file_name/digest égaux intent.provider_credential ; source_sha=stored_sha=digest original, stored_bytes=longueur réelle. Parseur pur partagé valide record/version/id/format/contexte ; handoff_sha=SHA(env_bytes décodés réellement remis au runtime), names=[BOOTSTRAP_ENV]. Aucune résolution/Auto/nouvelUUID/réécriture du record ; env et secrets décodés contrôlés contre ControlSecrets.
- `rev-00000000000000000000.json` puis revision u64 décimale20digits : `{version:1,operation_id,intent_hash,revision,previous:null|RevisionRefV1,payload:CheckpointPayloadV1}`, domaine `ai-guard:broker-checkpoint:v1`. RevisionRef wire `{revision:u64,hash:H}` ; rev0 previous=null, sinon précédente+1/hash exact, sans trou.
- `current.json` : `{version:1,operation_id,intent_hash,head:RevisionRefV1}` ; seule entrée vers révisions confirmées.
- `accepted.json` : `{version:1,scope_hash,operation_id,submit_hash,intent_hash,bindings_hash,initial_checkpoint:RevisionRefV1}`, domaine `ai-guard:broker-accepted:v1`.

None obligatoire et explicite si intention sans credential ; manuel attendu/fournisseur3 manquant ou incompatible refuse. Source ne prouve aucune identité. reserve calcule LUI-MÊME submit_hash=SHA256(`ai-guard:broker-submit:v1\0` + submit_bytes EXACTS), vérifie <=256KiB AVANT I/O et n'accepte aucun hash libre prétendu vérifié. Ces bytes ne sont ni parsés ni exécutés par le journal et peuvent ne pas être persistés ; leur format/frame canonique futur reste hors lot. Toute différence de bytes change l'identité de soumission ; aucune normalisation silencieuse.

## Validation checkpoint réelle

initial(intent) produit demande initiale exacte et managed=None. from_plan_bytes lit vrai RunPlan borné32MiB et compare version/op/session/options/provider_input/ref immuables ; conserve approbation appliquée. from_bytes vérifie forme/version/taille seulement, aucune permission/contextualisation. Nouveau wrapper strict même si RunOptions historique ignorait des champs.

À accept/load/advance : validation pure runner obligatoire contre intention+scope+manifeste. Demande identique hors human_approved/approval_hash ; false exige None, true exige hash exact de la demande initiale, sans reset silencieux. managed présent : tenant/origin/config_root et admission/session/spec/worker/owned/pending/import liés aux mêmes identités réelles ; credential_digest égal handoff_sha manifeste lorsqu'il existe. Les faits stockés ne deviennent pas reçus centraux qualifiés ni permission ; toute version inconnue refuse. Mutations pending/uncertain : version1 ; exclusion du contrôle : absence historique ou1 ; invocation : politique1/2 ; fingerprints handle/observation worker et import : versions1/2/3, absence historique égale1. Lire une version historique ne l'adopte pas pour un nouveau start. Consommateur recoupe contrôle avant effet.

## Durabilité, concurrence et CAS

1. Racine/ancêtres privés retenus ; chaque nouveau namespace/jobs/job/bindings impose fsync enfant ET parent jusqu'à root durable. Directory::child seul ne suffit pas. Bootstrap root doit synchroniser ses propres parents. Acquérir lock retenu avant mutation scope/jobs ; jamais remplacer l'inode du lock. Clones Journal partagent Arc/FD/mutex, drop clone ne déverrouille pas ; ouverture indépendante Busy.
2. Sous mutex writer+flock, comparer scope/op/submit AVANT source/résolution. Exact accepted complet ⇒ replay journal ; autre submit ⇒ Conflict. Partial avec Weak lease vivante ⇒ InProgress. Ticket privé nonClone porte Arc de lease lié writer ; après drop/restart partial⇒Interrupted, sans nouveau ticket/source/Auto.
3. New : budget sous writer, reservation exclusive0600, fsync fichier+parent puis ticket. Perte ticket n'efface rien, ne donne aucun droit.
4. accept vérifie ticket/intention/op, revalide FD source DÉJÀ retenu sans second open, prépare None/manuel canonique/fournisseur exact par parseurs purs ; contrôle literals ; binding exclusif durable, fsync+parent.
5. Intent+manifest+rev0 exclusifs/fsync chacun+parents ; current0 puis accepted DERNIER marqueur, parent fsync AVANT reçu. Erreur write/sync/rename/publication⇒aucun ACK succès ; partials conservés/comptés.
6. advance : même mutex/flock, validation scope/intent/manifest/current/contexte, expected(revision/hash) CAS ; rev+1 exclusif/fsync+parent ; pointer temporaire0600/fsync ; revalider identité ancien current, rename atomique contrôlé + parent fsync, puis reçu. Un appel exact expected+payload déjà confirmé dans la chaîne de current retourne le reçu original même après révisions suivantes ; même expected avec payload différent⇒Conflict. Aucune orpheline n'est promue/écrasée ; candidate partielle/divergente refuse. Un receipt n'est rendu qu'après revalidation/resync du graphe confirmé.
7. Reload : graphe accepted/current + chaîne exacte/checksums/bindings complets ; jamais choisir plus grand numéro orphelin. Graphe entier après ACK perdu revalidé+resync avant replay ; partial⇒Interrupted, illégal⇒Corrupt, jamais succès vide ni nouvelle résolution.

Dans le writer VIVANT et ses clones Arc, retenir par job le plancher head revision/hash ACKé sous le même mutex : toute lecture/mutation vérifie que current/chaîne contiennent ce préfixe et n'abaissent pas le plancher ; rollback⇒refus conservateur sans nouveau reçu. Mettre à jour ce plancher après durabilité et avant ACK. Aucun anti-restore après restart/fencing n'est promis. Rouvrir/copier journal récupère des données, aucune lease fresh-origin Store/claim/deadline/start nouveau. D5/restart/runtime restent ouverts. Arrêt préarmé futur n'attend/ne détient pas writer mutex et ne dépend pas de load/current/ACK ; ce lot ne livre ni scheduler/IPC ni cette qualification. Pas d'outbox inventée : pending réel conservé dans checkpoint.

## FS et frontières

Namespace privé EXCLUSIF sans DB/VFS SQLite ; noms fermés. Entrée journal préexistante : fstatat SANS open vérifie type/eUID0600/nlink1/ACL/identité et exclusions connues, puis seulement openat NOFOLLOW|NONBLOCK|CLOEXEC, fstat même inode. Hardlink refusé AVANT ouvrir/fermer FD pour préserver locks POSIX SQLite. Aucune détection universelle SQLite inconnu nlink1 ; absence DB dans namespace précondition embedding explicite.

Helper PrivateFile retenu : contrôler FD/cache/digest et identité du nom metadata-only sans revalidate actuel qui rouvre la pathname. Source sélectionnée auparavant par trusted caller ; aucun reopen source par journal. Nouveaux fichiers métier O_EXCL sous Directory retenu, parents/ACL revalidés ; aucune primitive SQLite détournée. Publication contrôlée sur identité, pas rewrite général. Parents privés/ACL/DAC protègent autre UID ; pas résistance promise à course hostile même UID/root/admin ou snapshot VM.

## Limites admission trusted

Fixes par objet : Submit préimage256KiB, intent256KiB, manuel source/env64KiB, record fournisseur512KiB, manifest/markers8KiB, checkpoint et source RunPlan32MiB. DEFAULT job256MiB/512entries/256revisions ; scope1GiB/65536entries/256jobs. JournalLimitsV1::new accepte des valeurs non nulles jusqu'aux HARD MAX configurables trusted : job8GiB/131072entries/65536revisions ; scope1TiB/4194304entries/16384jobs ; job_bytes<=scope_bytes, job_entries<=scope_entries, révisions et jobs compatibles avec leurs entrées disponibles. Aucune allocation upfront à la taille du cap, aucun cap reçu du client ni qualification100 implicite. Objet impossible⇒LimitExceeded avant création. max_revisions compte rev0 et confirmées ; partials comptent bytes/entries, scope.json/lock/jobs-dir aussi.

Tout temporaire/partial/orphelin/répertoire compté sous writer avant création, scan borné refuse dépassement préexistant. Zéro retry à nouveaux noms illimités ; aucune suppression/GC/overwrite pour libérer une preuve. Caps applicatifs, pas quota FS bytes/inodes effectif ni qualification100workers. fsync peut bloquer ; aucune garantie scheduler-stop délivrée par writer.

## Acceptation Luna, FS réel

RED absent puis ciblés : None/manuel/provider3 accept+replay sans source ; canonical manuel omet commentaires/sourceSHA distinct ; provider raw/id/digest intacts et handoffSHA réel ; défaut source/binding refusé ; contrôles literals sans heuristique bloquant provider légitime ; parseurs partagés/serde/Debug opaques. CP initial/approuvé/managed réel ; immutables intent/input/ref/scope/manifest et false+hashSome refusés.

Concurrent reserve=ticket vivant InProgress, drop/restart Interrupted ; Submit bytes exacts/bornes avant I/O ; processus flock/clones/drop ; CAS/exact replay original après révisions suivantes/chaîne/current/troncation/orphelin divergent/plancher vivant partagé ; defaults et hard max bornés sans allocation upfront ; caps bytes/entries/jobs/revisions/partials sans effacement. Vrai Store SQLite + témoin autre processus de locks : hardlink rejeté AVANT open secondaire ; exclusions connues facultatives ; symlink/FIFO/mode/ACL/parents substitués.

Hooks uniquement cfg(test) dans src/broker/journal/tests.rs owned Luna, aux vraies opérations write/fsync/publication ; child Rust SIGKILL aux barrières. Aucun envflag prod, mock permanent, simulation remplaçant FS ni suppression de preuve. Aucun ACK avant barrière réelle ; reprise exacte ou refus explicite. macOS/Linux preuves séparées ; SIGKILL n'est pas panne électrique. Power-loss, IPC/auth frais, scheduler/TTY, fencing après restart, capacité100, quotas physiques restent ouverts.
