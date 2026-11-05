# Journal privé du broker — contrats persistants

## Propriétaire authentifié de l'audit terminal — 6 novembre

La boucle du service possède un chemin d'audit avant sa barrière d'admission
métier. Il utilise uniquement une capacité `cold_audit` déjà retenue par le
vrai backend Stops ; ni création de profil, ni conversion Full. L'inventaire
et la préparation précèdent la création du Job et le décompte de ses essais :
un historique sans lot admissible ou une réserve Busy ne remplit pas ce cache.
Les pools existants bornent les opérations et propriétaires concurrents.

Le propriétaire détaché possède la préparation, la lease, les réserves logique
et de queue, le Job et sa capacité avant tout POST. Il échantillonne le budget
HTTP initial de 45 s avant `VerifiedProfile::authenticate(false)`. L'authentification
borne son attente par cette échéance ; le même client vérifié lit le Worker puis
envoie les seuls événements exacts retenus. Le scope complet et les faits purs
terminaux doivent correspondre ; une donnée descriptive ne remplace jamais
l'identité runner confirmée auprès du gateway.

Si un reçu a été obtenu, une erreur de l'envoi suivant, l'expiration du budget
ou le drapeau d'arrêt conduit au submit synchrone du sous-ensemble reçu, sans
nouvelle attente ou acquisition. Zéro reçu n'entraîne aucune publication locale.
Le retour de la boucle observatrice n'annule pas ce propriétaire. Cette garantie
est limitée au processus et au runtime vivants : `serve` ne draine pas encore
explicitement ces nouvelles tâches à son arrêt. Panne d'acteur avant enqueue :
NotQueued ne confirme rien, les IDs prédurables restent pending pour reprise.
Après enqueue, le contrat de rétention/quarantaine de l'acteur ci-dessous reste
applicable. Aucune résistance à une panne physique ou à ENOSPC n'en est déduite.

Ce trajet n'ouvre ni contexte métier, blob fournisseur, runtime ou credential
worker ; il ne décide, lance, exporte, supprime ou réarme rien. Seuls les ACK
sélectionnés de l'outbox changent. Le résultat géré, Arm/Claim, Cancel et le
cycle du gardien restent inchangés. Les [guides EN/FR](../setup/broker/README.md)
expliquent l'utilisation du mode restreint. [Résultats et limites](VALIDATION.md).

## Complétion restreinte sur le même acteur — 6 novembre

Une façade interne distincte est construite seulement pour le vrai backend
Stops. Elle ne dérive pas du `StopWriterClient` et n'accorde aucun accès Full.
La préparation conserve le même acteur, la lease, la réserve logique et un
ticket de complétion unique avant tout HTTP. `submit` consomme ce ticket de
façon synchrone ; l'attente de confirmation ne possède pas l'annulation du
travail déjà accepté.

Le merge exige les reçus HTTP exacts, l'opération, l'intention et les événements
retenus. Une séquence connue ne peut désigner deux IDs, y compris entre le lot
et les événements déjà ACKés. Aucune contiguïté ni ordre global des séquences
n'est supposé. Le checkpoint courant conserve runner, supervision, ordre des
événements et commandes Cancel concurrentes ; seules les séquences sélectionnées
passent de None à Some, ou rejouent la même valeur.

Le scope physique doit conserver exactement ses jobs. Chaque historique est
relu contre ses floors, rejets et usage avant publication ou confirmation ;
un vrai préfixe ancien ne peut abaisser un floor connu. Un reader partagé évite
N inventaires globaux et les payloads des voisins sont libérés successivement.
Le coût reste lié aux historiques bornés du scope, sans SLO déduit.

Le premier lot et son head cible sont scellés avant l'effet de publication.
Sous la même réserve, un lot différent est refusé ; le même lot ne peut que
confirmer le head publié ou ses descendants Cancel. Une erreur met l'input
en quarantaine forte avant réponse. Cet input ne contient aucun sender de
l'acteur : Cancel reste admis et la fermeture de queue libère la lease.
La sortie explicite de quarantaine/recomptage reste à raccorder ; la fermeture
de l'acteur termine actuellement cette rétention en mémoire.

Les [17 parents ciblés](VALIDATION.md#6-novembre--ack-retenu-composé-au-writer-stops)
valident ce périmètre logiciel. Le reçu et le contrôle terminal restent des
données sans authentification. Leur composition avec le même client vérifié,
le budget initial et la boucle ServiceCold est décrite ci-dessus ; les preuves
de chaque lot gardent leur périmètre distinct.

## Préparation bornée de l'audit retenu — 5 novembre

Le module interne `journal::recovery::cold_audit` prépare uniquement les deux
événements V4 retenus, sous la lease et le mutex existants de `StopJournal`.
Il ne convertit pas cette capacité en `Journal` complet et n'ouvre pas le
contenu des blobs. Les tailles physiques comprennent les métadonnées des blobs ;
un inventaire incomplet ou divergent refuse la préparation.

Une seule préparation peut rester vivante par acteur. Son propriétaire conserve
la lease et une réservation logique ; le registre garde une référence faible.
Un reply perdu ou l'abandon de la préparation libère cette réservation sans
verrou dans Drop. Les Cancel existants gardent leur admission et leur publication.
Les rejets fermés sont comptés après validation du reader ; une réservation
inachevée ne devient pas un rejet vide.

Pour chaque job mutable, `c = 64 - nombre total de commandes locales`, `B`
borne l'enveloppe du prochain checkpoint, `P` le pointeur Current maximal et
`δ` l'ajout d'un Cancel avec son séparateur. Les V1/V2 sont comptés dans la forme
V3 de leur prochain Cancel. Pour le job sélectionné, B inclut les deux séquences
d'audit à leur largeur maximale, sans écrire ces séquences fictives.

```text
réserve Cancel du job : c × (B + P) + δ × c × (c + 1) / 2
réserve ACK sélectionné : B + c × δ + P
```

L'usage physique, toutes les réserves Cancel et une réserve ACK doivent tenir
dans les plafonds bytes/entrées du job et du scope, avec les révisions et limites
imbriquées existantes : commandes 64 Kio, outbox 4 Mio, checkpoint 32 Mio. Les calculs
sont contrôlés contre les débordements ; une seule sérialisation comptée par job
évite de fabriquer 64 copies de checkpoints. Cette borne conservatrice peut refuser
une préparation près de la capacité. Elle ne réserve ni blocs ni inodes et
n'immunise pas une écriture contre ENOSPC, EIO ou une panne physique.

Cette préparation ne prouve ni l'identité centrale, ni le terminal, ni un reçu
HTTP. Elle n'accorde aucun ACK, lancement, export ou nettoyage. Son raccord au
writer Stops et au propriétaire authentifié est décrit ci-dessus ; cette réserve
seule ne constitue aucune preuve de boucle ou de reprise. [Preuves et limites](VALIDATION.md).

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

- `reservation.json` : `{version:1,scope_hash,operation_id,submit_hash}`. Le record n'a pas de domaine de hash propre ; `submit_hash` engage les bytes de soumission via `ai-guard:broker-submit:v1`, décrit ci-dessous.
- `intent.json` : canonical_bytes ExecutionIntent exacts, domaine/hash existants inchangés.
- `bindings.json` : `{version:1,operation_id,intent_hash,binding:BindingManifestV1}`, domaine `ai-guard:broker-bindings:v1`.
- BindingManifest fermé : `{"kind":"none"}` ; ou `{"kind":"manual","binding_id":Uuid,"file_name":"manual-env-v1.bin","names":[String],"source_sha256":H,"stored_sha256":H,"handoff_sha256":H,"stored_bytes":N}` ; ou `{"kind":"provider","binding_id":Uuid,"file_name":"<UUID>.json","names":[BOOTSTRAP_ENV],"source_sha256":H,"stored_sha256":H,"handoff_sha256":H,"stored_bytes":N}`. Aucune valeur Env/secret dans métadonnées.
- `bindings/manual-env-v1.bin` : ENV CANONIQUE préparé depuis bytes du PrivateFile source retenu, même parseur pur extrait de credentials_from_source ; commentaires/lignes ignorées absents, ordre des noms et règles historiques inchangés. UUID manuel minté UNE FOIS ; source_sha=SHA(raw source), stored_sha=SHA(env canonique), handoff_sha=stored_sha ; runtime futur lit seulement copie durable, aucun retour source ni Snapshot unlink-on-drop.
- `bindings/<UUID>.json` fournisseur3 : record original copié EXACTEMENT ; binding_id/file_name/digest égaux intent.provider_credential ; source_sha=stored_sha=digest original, stored_bytes=longueur réelle. Parseur pur partagé valide record/version/id/format/contexte ; handoff_sha=SHA(env_bytes décodés réellement remis au runtime), names=[BOOTSTRAP_ENV]. Aucune résolution/Auto/nouvelUUID/réécriture du record ; env et secrets décodés contrôlés contre ControlSecrets.
- `rev-00000000000000000000.json` puis revision u64 décimale20digits : `{version:1,operation_id,intent_hash,revision,previous:null|RevisionRefV1,payload:CheckpointPayloadV1}`, domaine `ai-guard:broker-checkpoint:v1`. RevisionRef wire `{revision:u64,hash:H}` ; rev0 previous=null, sinon précédente+1/hash exact, sans trou.
- `current.json` : `{version:1,operation_id,intent_hash,head:RevisionRefV1}` ; seule entrée vers révisions confirmées.
- `accepted.json` : `{version:1,scope_hash,operation_id,submit_hash,intent_hash,bindings_hash,initial_checkpoint:RevisionRefV1}`. Aucun domaine de hash propre à ce record n'est utilisé actuellement. Le lecteur valide ses champs et leurs liens vers la réservation, l'intention, les bindings et la révision initiale.

None obligatoire et explicite si intention sans credential ; manuel attendu/fournisseur3 manquant ou incompatible refuse. Source ne prouve aucune identité. reserve calcule LUI-MÊME submit_hash=SHA256(`ai-guard:broker-submit:v1\0` + submit_bytes EXACTS), vérifie <=256KiB AVANT I/O et n'accepte aucun hash libre prétendu vérifié. Ces bytes ne sont ni parsés ni exécutés par le journal et peuvent ne pas être persistés ; leur format/frame canonique futur reste hors lot. Toute différence de bytes change l'identité de soumission ; aucune normalisation silencieuse.

### Profil Docker accepté — formats V2

Le registre opérateur wire2 ajoute à chaque profil un choix fermé obligatoire `prearmed_start_profile={"version":1,"mode":"docker_minimal"}` ou le mode explicite `legacy`. Wire1 conserve son schéma et résout Legacy ; un choix manquant/inconnu ou `docker_minimal` avec Podman est refusé. Ce choix concerne uniquement une nouvelle acceptation ; modifier le registre ne transforme pas le binding d'un job accepté.

Pour un nouveau job Docker minimal, `bindings.json` passe en version2 et ajoute `docker_launch_profile: DockerLaunchProfileBindingV1` ; son domaine est `ai-guard:broker-bindings:v2` suivi du NUL de l'helper digest. Le binding fermé contient `version`, `kind`, `journal_scope_hash`, `parent_operation_id`, `intent_hash`, `directory_ref`, `file_name`, `sha256`, `bytes`, `runtime_endpoint` et `invocation_fingerprint`. Les valeurs de contenu sont imposées : version1, kind `docker_minimal`, répertoire `docker-launch-v1`, fichier `config.json`, trois octets exacts `{}\n` et leur SHA256. Scope authentifié, parent accepté, intention, endpoint et invocation policy2 sont recoupés ; un identifiant déclaré n'est pas une preuve d'identité.

Le fichier privé est créé exclusivement, synchronisé avec ses parents et retenu par descripteurs avant Accepted. `accepted.json` porte version2 et les mêmes champs de liens qu'en V1 ; versions Accepted/bindings doivent correspondre. Un V1 avec profil ajouté, un V2 sans profil, un null explicite qui ne respecte pas les octets canoniques ou tout couplage divergent est refusé. Les credentials, intentions, checkpoints V1–V4 et plans fournisseurs1–3 conservent leurs formats. Les anciens lecteurs refusent V2 ; aucun downgrade ou rollback de jobs actifs n'est qualifié.

Le chargement métier et le start exigent le blob exact et ses identités retenues. La projection STOP valide les métadonnées engagées et la chaîne sans ouvrir ce blob ; taille/absence/substitution ne doivent pas effacer une Arm prouvée ou ses siblings. Un budget d'append, audit ou stockage inconnu ne produit toutefois aucun ACK inventé. Le profil est vérifié avant les helpers et le worker, puis la dernière garde compare le binding complet juste avant spawn. Cela ne prouve ni complétion d'un ancien start ni retrait/fencing D5. [Implémentation, régressions et limites](evidence/2026-11-01/docker-launch-profile/README.md).

## Validation checkpoint réelle

initial(intent) produit demande initiale exacte et managed=None. from_plan_bytes lit vrai RunPlan borné32MiB et compare version/op/session/options/provider_input/ref immuables ; conserve approbation appliquée. from_bytes vérifie forme/version/taille seulement, aucune permission/contextualisation. Nouveau wrapper strict même si RunOptions historique ignorait des champs.

À accept/load/advance : validation pure runner obligatoire contre intention+scope+manifeste. Demande identique hors human_approved/approval_hash ; false exige None, true exige hash exact de la demande initiale, sans reset silencieux. managed présent : tenant/origin/config_root et admission/session/spec/worker/owned/pending/import liés aux mêmes identités réelles ; credential_digest égal handoff_sha manifeste lorsqu'il existe. Les faits stockés ne deviennent pas reçus centraux qualifiés ni permission ; toute version inconnue refuse. Mutations pending/uncertain : version1 ; exclusion du contrôle : absence historique ou1 ; invocation : politique1/2 ; fingerprints handle/observation worker et import : versions1/2/3, absence historique égale1. Lire une version historique ne l'adopte pas pour un nouveau start. Consommateur recoupe contrôle avant effet.

Le handle du worker porte `intent.operation_id()` conformément au vrai `worker_spec` et à la reprise existante ; l'UUID central `worker.attempt.attempt_id` reste distinct et lie les mutations du registre. Le handle d'import porte son opération helper propre. Le [complément de validation](evidence/2026-10-30/journal-context/README.md) conserve le défaut de comparaison initial et ses régressions ; aucune réécriture d'anciens handles n'est effectuée.

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

## Extension de cycle de vie — contrat du lot suivant,31octobre2026

Base de conception `4890df13f56f2a61d9ee773f9d1dbd7cca929e72`. **Implémenté et vérifié au périmètre logiciel ciblé**, [preuves du 31 octobre](evidence/2026-10-31/broker-lifecycle/README.md). Ce titre conserve l'ancre du contrat initial. Le service permanent approuvé exige ces faits locaux durables, sans faux Worker/refus policy/receipt ni fichier de statut parallèle. V2 reste fermé et inchangé ; une enveloppe V3 lui ajoute ces faits dans la même chaîne révisionnée. Direction et limites du service restent celles de BROKER_SUPERVISION.md ; aucun nouveau stockage ou déploiement. Le service, l'IPC et l'enforcer concret restent à raccorder.

### Rejet avant intention valide

Dans `broker::journal` : `Journal::reject(&self, ticket: ReservationTicket, phase: RejectionPhaseV1, code: RejectionCodeV1) -> JournalResult<RejectionReceiptV1>`. Phase fermée `SubmissionPreparation`, codes `PreparationFailed` et `CancelledBeforeAcceptance`. Reçu opaque non Serde de valeurs privées, getters `operation_id()->Uuid`, `submit_hash()->&str`, `phase()`, `code()`, `record_hash()->&str`. `ReserveResult::Rejected(RejectionReceiptV1)` rend le même reçu au replay exact ; bytes Submit différents restent Conflict.

Record strict `rejected.json` : `{version:1,scope_hash,operation_id,submit_hash,phase,code}`, hash sous `ai-guard:broker-rejection:v1` + NUL + JSON canonique. Même writer mutex/flock/ticket owner-lease-op et limites existants. Avant reject, liste exacte `reservation.json` seulement ; accepted, intent, binding, révision ou pointeur partiel empêchent le rejet. Écriture O_EXCL puis fsync fichier/job/jobs avant reçu. Au replay, liste exacte reservation+rejected, deux records fermés/canoniques/cohérents, resync avant ACK. accepted+rejected est Corrupt, jamais une priorité silencieuse. Partiels conservés et explicitement Interrupted/Corrupt, aucune nouvelle préparation.

Plancher vivant du rejet partagé par les clones, hash retenu après ACK/observation. Vérifier aussi les planchers accepted existants AVANT tout retour absent/Interrupted/Rejected, pas seulement le chemin accepted. Un état confirmé disparu ne redevient pas une réservation normale ; Conflict. Aucun anti-restore après restart n'est annoncé. Le rejet ne libère aucun quota central et n'est pas un terminal runtime.

### Commandes locales typées dans V3

Dans `broker::lifecycle` :

```rust
BrokerCheckpointV3::new(broker: BrokerCheckpointV2, commands: Vec<LocalCommandV1>) -> JournalResult<Self>
BrokerCheckpointV3::from_bytes(bytes: &[u8]) -> JournalResult<Self>
// getters broker(), commands(), canonical_bytes() ; objet opaque, Clone, serde stricte.
LocalCommandV1::new(operation_id: Uuid, intent_hash: &str, command_id: Uuid,
    request: LocalCommandRequestV1) -> JournalResult<Self>
// getters command_id(), input_hash(), request() ; champs wire privés, Debug opaque.
Journal::advance_lifecycle(&self, operation_id: Uuid, expected: RevisionRefV1,
    next: &BrokerCheckpointV3) -> JournalResult<CheckpointReceiptV1>
Journal::command_receipt(&self, operation_id: Uuid, command_id: Uuid)
    -> JournalResult<Option<LocalCommandReceiptV1>>
AcceptedJobV1::broker_checkpoint_v3(&self) -> Option<&BrokerCheckpointV3>
```

Enveloppe `{version:3,broker:BrokerCheckpointV2,commands:[...]}`. Commande `{version:1,command_id,input_hash,request}` ; son constructeur calcule lui-même SHA256 sous `ai-guard:broker-local-command:v1` + NUL + JSON canonique de `{operation_id,intent_hash,command_id,request}`. Le loader recalcule ce hash dans le vrai contexte du job. UUID non nil et hashes hex64 canoniques. Maximum64commandes et64KiB canoniques cumulés, enveloppe32MiB et caps communs inchangés. Anciennes commandes intégralement immuables, append-only, UUID uniques. Reçu local opaque avec getters `command_id()`, `input_hash()`, `head()` : dérivé de la première révision confirmée contenant cette commande, sans nouvelle écriture/faux ACK HTTP. UUID absent donne None pour un job accepté ; job absent/non accepté ou identité nil sont InvalidInput. Le service compare le hash du nouveau contenu avant tout replay.

`LocalCommandRequestV1` est une enum serde strictement fermée : `ApprovalSubmitted { request_hash: String, review_receipt: DecisionReceiptV1 }`, `CancelRequested`, `FailureObserved { phase: LocalFailurePhaseV1, code: LocalFailureCodeV1 }`. Phases fermées Decision,WorkspacePreparation,Admission,Import,ImportVerification,Worker,FinalScan,Export,Cleanup ; codes InvalidData,Unavailable,IoFailed,ProtocolMismatch,TransportFailed. Aucun message/chemin/exception libre. Une failure locale ne termine pas un RPC pending, n'établit pas la mort d'une cible et ne libère aucun quota. Tous les diagnostics restent opaques, y compris champs inconnus nullable derrière enum/trait Deserialize public.

### Approbation et annulation causales

ApprovalSubmitted exige le vrai Decision Initial/Review ACK intégral d'une révision ANTÉRIEURE : demande normalisée/hash/request_id/phase/receipt exacts et outcome Review. Aucun Admission ou Cancel antérieur/co-né ; au plus une commande d'approbation, et current_request non approuvée à sa naissance. Commit suivant seulement : false→true avec hash exact ET Decision Approved pending ; son ACK arrive encore dans une révision ultérieure. L'entrée humaine n'est pas Allow. Les V1 sans managed et V2 DÉJÀ approuvées gardent leur fait historique à la promotion, sans ApprovalSubmitted fabriqué ; depuis une tête non approuvée, aucune première V3 ne s'auto-approuve. Préserver les anciennes sémantiques/bytes sans les réécrire.

Le premier CancelRequested interdit dès SA révision les nouvelles Decision,Admission,WorkerMutation Prepare/CreateIssued/StartClaim,WorkerClaim,helpers Import/ImportVerification/FinalScan,Arm/HelperClaim de ces cibles. Une réponse tardive n'est admise que pour une requête exacte déjà pending auparavant. Admission inconnue reste à réconcilier avec son UUID/contenu ; GET absent ne fence pas le RPC. ACK Reserved peut alors recevoir CancelReserved pending puis ACK. La transaction CancelReserved du Store libère le quota central avant l'ACK du checkpoint ; une réponse perdue peut laisser le journal local pending alors que ce quota est déjà libéré. Observe/Unknown/Stop/CancelReserved/CleanupResult et observations/cleanup helper restent enregistrables sous les validations existantes. Created est aussi admissible comme fait tardif si CreateIssued était déjà durable avant la frontière cancel, sans autoriser un nouveau create. Cancel est irréversible mais ni terminal ni libération. Preparing sans CID et les cibles jamais démarrées sans terminal admissible gardent leur quota/NeedsReconciliation selon le Store existant ; ces écarts opérateur restent ouverts avant cutover.

L'enregistrement tardif d'un owned n'autorise aucun nouveau create : worker possible seulement si CreateIssued était déjà durable avant cancel ; helper possible pour une préparation antérieure déjà durable. Le service doit en plus vérifier la génération de cancel mémoire monotone avant tout effet/dernier garde. Une nouvelle annulation invalide aussi les exports antérieurs ; un export explicite postérieur possède son propre grant/génération, jamais un reset du job ou un droit worker. Le contrôleur préarmé continue malgré échec de writer/HTTP ; ce lot de données ne le fournit pas.

Un export postmortem explicitement demandé reste possible après cancel : ExportRequest né dans une révision STRICTEMENT POSTÉRIEURE au DERNIER CancelRequested confirmé, puis ACK central requis, helper de purpose Export exclusivement lié à ce grant, puis owned/Arm/claim Export. Un grant pré-cancel inutilisé, un ACK tardif d'une ancienne requête ou une co-naissance avec Cancel ne suffit pas. À chaque nouveau Cancel, le loader retient la longueur de la liste d'opérations de CETTE révision comme frontière historique interne ; après plusieurs annulations, seule la dernière frontière s'applique. Vérifier l'index de naissance de l'ExportRequest du grant pour toute nouvelle préparation/Arm/HelperClaim Export, y compris quand un ancien Arm existe. Cette métadonnée se dérive du préfixe confirmé, sans champ wire libre supplémentaire. Aucun redémarrage worker/import ni nouveau FinalScan implicite n'est permis par cette exception. L'identité fraîche et la demande explicite seront imposées par le service ; le DTO privé n'est pas leur preuve. Toutes validations V2/Store et terminal préalable à l'export restent applicables.

### Chaîne, statut et pagination

Chaîne V1+V2*V3* (saut V1→V3 possible uniquement sous TOUTES règles de promotion V2, notamment ancien managed du préfixe bloquant). Le record externe V3 a son domaine `ai-guard:broker-checkpoint:v3`. Réutiliser publication/CAS/replay/caps/planchers/contrôle des secrets ; aucun octet/domaine V1/V2 ou RunPlan1/2/3 ne change. Après V3, advance V1/V2 refuse même son ancien replay exact. Replay V3 confirmé rend son reçu historique avant validation contre la tête courante. `checkpoint()`/getter V2 gardent la vue typée intérieure. Pas de sidefile d'état ni cache FS ajouté.

`Journal::status(&self, operation_id: Uuid) -> JournalResult<Option<JournalStatusV1>>`, enum opaque `ReservedInProcess`, `Interrupted`, `Rejected(RejectionReceiptV1)`, `Accepted(AcceptedJobV1)`. Absent seul donne None ; mauvais record est une erreur. Sous même mutex sans appel récursif à load/reserve, appliquer toutes exclusivités/planchers. Réservation seule après perte du ticket/restart reste Interrupted. `load` reste accepted-only et rend InvalidInput sur un rejet complet, sans le cacher comme absence.

`Journal::list(&self, after: Option<Uuid>, limit: u32) -> JournalResult<JournalPageV1>` : limit1..100 et curseur non nil, UUID canoniques triés/cursor exclusif, même verrou durant la page. `JournalPageV1::entries()->&[JournalEntryV1]`, `next_after()->Option<Uuid>`. Entrée opaque, sans FD/secret/chemin : `operation_id()`, `kind()->JournalEntryKindV1` (ReservedInProcess/Interrupted/Rejected/Accepted), `head()->Option<&RevisionRefV1>`, `cancel_requested()->bool`, `approval_submitted()->bool`, `failure_count()->usize`, `pending_rpc_count()->Option<usize>`, `worker_state()->Option<&WorkerStateV1>`, `quota_held()->Option<bool>`, `runtime_status()->Option<&RuntimeStatusV1>`, `last_observed_at()->Option<&str>`. Les derniers faits sont ceux STOCKÉS, pas une inspection fraîche ; absence reste None. pending_rpc_count est None en V1 dont la représentation n'est pas exhaustive, et compte les response=None en V2/V3. approval_submitted signifie commande V3 présente, pas permission ni reset d'une approbation V1/V2 historique.

Avant pagination : inventaire complet borné/types/UUID et vérification de TOUS les IDs/planchers déjà observés, même hors page, pour empêcher qu'un ACK disparu devienne invisible. Les IDs déjà lus ReservedInProcess ou Interrupted sont eux aussi suivis/revalidés ; ensemble mémoire borné au cap jobs, disparition/corruption explicite. Charger/valider les statuts retenus sans masquer les erreurs ; cette page ne qualifie pas les corps des jobs non encore observés hors page, et aucun snapshot inter-pages n'est promis. Les lecteurs n'accordent aucune permission : identité/scope demeurent imposés par le service futur.

### Preuves requises avant clôture du lot

RED existant de plancher et API absente avant code ; tests FS réels des rejets/replays/divergences/partiels/exclusivités/planchers, hooks cfg(test) après les vraies barrières si nécessaire, erreurs sans ACK factice. V1/V2 goldens intacts, chaîne V3 stricte/replay ancien/downgrade/limites/diagnostics ; approbation sans Review/co-naissance refusée et reprise aux deux commits, grand-père V1/V2 conservé ; cancel avant Admission et pendant réponse inconnue avec vrai Store Reserved→Cancelled, pas de quota libérée sur failure/Preparing sansCID. Pagination bornée, absence/corruption/plancher hors page et commandes hashées recoupés. Inclure refus des nouvelles actions co-nées avec cancel, owned tardif légitime et export postmortem autorisé sous son vrai grant. Luna exécute les tests ciblés, revue indépendante avant intégration. Aucun moteur/modèle/frontend/E2E général requis par le seul format ; le service et ses vrais scénarios restent obligatoires.

## En cours — livraison d'événements dans V4, 31 octobre 2026

Le service permanent a besoin de conserver un événement avant son POST HTTP, puis de conserver le numéro d'audit réellement reçu. Les neuf variantes de supervision V2 ne comprennent pas cette livraison : elles restent inchangées. Le nouveau module `src/broker/outbox.rs` définit `BrokerCheckpointV4`, enveloppe fermée `{version:4,lifecycle:BrokerCheckpointV3,events:[OutboxEventV1]}`. Son record de révision emploie le domaine `ai-guard:broker-checkpoint:v4`. Les domaines et bytes historiques V1/V2/V3 restent identiques ; la chaîne accepte la promotion et refuse tout retour à une version inférieure. Les getters des versions intérieures restent des vues des mêmes faits.

Chaque entrée comporte `{version:1,event:Event,input_hash,audit_seq:Option<i64>}`. Le hash est calculé par le constructeur sous `ai-guard:broker-outbox-event:v1`, avec l'opération, le hash de l'intention et le contenu exact de l'événement. L'UUID canonique non nil et le contexte projet/session/sandbox/correlation/trace doivent correspondre au job. La liste est append-only, les identités et contenus sont immuables. Une entrée naît avec `audit_seq=null` ; seul un commit ultérieur lui attribue un numéro strictement positif, ensuite immuable. Un événement ou son ACK ne donne aucun droit d'admission, de claim, d'export ou de start. Les namespaces réservés `control`, `worker`, `security.decision` et `usage.allocation` sont refusés ; les événements métier ne remplacent pas les transactions centrales typées.

Bornes : 512 événements, 120 KiB par événement, 4 MiB canoniques cumulés et 32 MiB pour l'enveloppe ; les caps du journal s'appliquent aussi. Les clés et valeurs décodées passent par le contrôle des secrets avant persistance. Le wrapper ferme également les champs de `Decision` historiques, y compris un champ inconnu valant null. Les valeurs métier libres de `Event.data` et `Event.result` restent soumises aux bornes, au contexte et au contrôle des secrets.

`WriterClient::enqueue_events` écrit un lot dans une seule révision ; `acknowledge_events` confirme un lot dans une autre. Cela permet de conserver jusqu'à 500 événements normalisés sans consommer 1 000 révisions. Le replay d'enqueue rend le reçu de naissance pending, même après ACK ; le replay d'ACK rend son premier reçu confirmé. Doublons et contenu divergent sont des conflits. Les reçus sont dérivés de la chaîne confirmée, sans sidefile. Les mutations lifecycle/commandes effectuées par le writer préservent les événements V4 ; une mutation directe vers V3 après V4 est refusée.

État : code raccordé, cinq fonctions V4 en release PASS, dont 500 événements en deux révisions, replays historiques, collisions de type, secrets, contexte et limites. Les fichiers Journal sont réels, les événements et séquences de cette cible sont synthétiques ; la livraison complète via le nouveau service et les pannes HTTP/moteur restent `NOT_RUN`. [Gels et résultats](evidence/2026-10-31/broker-service/README.md). Aucun gate production clôturé.

### Identités des commandes du service

Une identité de commande ne peut pas changer de type entre commande locale, événement de reprise explicite et `ExportRequest`. V4 vérifie leur unicité dans la même révision, au-delà des vérifications préalables du service ; le writer applique aussi ce contrôle aux mutations V3 qu'il reçoit. Cela ferme la course entre deux écritures dans des listes différentes. Un input d'export immutable publié avant son grant réserve déjà son UUID : les autres types de commande le refusent par vérification metadata-only. Un verrou d'identité propre au profil couvre vérification et publication du premier fait local, y compris l'input d'export ; son attente est bornée à 15 s. Il est libéré avant HTTP, runtime ou durée d'exécution. Cancel invalide ses gardes mémoire avant de l'attendre. Le replay du même export conserve son input et sa destination exacts.

## En cours — récupération des arrêts indépendamment du chargement métier

`Journal::open/load` reste strict sur l'ensemble de son namespace, les bindings et les planchers observés. Cette exigence métier empêchait aussi de restaurer un arrêt sain si un autre job était endommagé ou si le blob fournisseur avait disparu. Le lecteur distinct `journal::recovery::StopRecoveryReaderV1` ne construit aucun `AcceptedJobV1`, handoff fournisseur, permission fraîche ou délai nouveau. Il n'ouvre que le scope attendu et sa lease **existante**, sans créer, réparer, remplacer ou supprimer de fichier.

Le rapport conserve la lease pendant les restaurations. Un inventaire metadata-only borné poursuit les siblings après une faute individuelle, compte également les entrées rejetées et conserve un état `incomplete`. Il protège les identités SQLite connues avant toute ouverture de fichier et ne suit pas de lien. Une faute sur l'ancre commune empêche d'utiliser ses projections ; une faute isolée laisse utilisables les autres chaînes intégralement prouvées. Un cap atteint arrête le scan avec état incomplet : aucun inventaire complet n'est alors annoncé.

Les headers, métadonnées de binding, contexte et préfixe complet jusqu'à `current` sont vérifiés. `confirmed.rs` partage les validateurs V1–V4 avec le chargement métier, qui conserve ses contrôles de bytes credentials, usage, fsync, planchers et CAS. Le lecteur STOP ne lit jamais le blob de credentials ; son absence ou un inventaire de bindings différent du manifest rend le rapport incomplet sans supprimer une preuve d'arrêt valide. Un manifest ou une chaîne altérés empêchent de projeter ce job. Un historique V1 managed, même effacé au head, reste incomplet faute d'armement indépendant ; aucun Arm, reçu ou terminal n'est inventé.

Chaque projection fournit seulement les données déjà ACKées de l'Arm, du reçu, du snapshot possédé et de l'autorité figée. Le service valide `RegistrationV1` puis appelle uniquement `EnforcerClient::restore` en ObserveOnly. Les générations mémoire sont monotones, mais ne prouvent pas la causalité d'une annulation : le préfixe confirmé détermine, pour chaque cible, si son Arm était antérieur au dernier Cancel. Un export autorisé après ce Cancel conserve sa deadline d'origine et ne reçoit pas cet ancien ordre d'arrêt. Une nouvelle annulation peut l'arrêter.

Le service ferme les admissions dès qu'une récupération est incomplète, poursuit les restaurations prouvées, puis libère la lease avant d'essayer le writer métier normal. Un échec du writer ne retire pas les arrêts restaurés. La disponibilité effective du gardien est revérifiée avant une nouvelle admission HTTP, y compris le replay d'un POST pending dont l'effet est inconnu ; un ACK local peut être consulté sans réémettre ce POST. Les tests ciblés de projection et les revues de source passent dans leur périmètre. Ces contrôles ne constituent ni un mécanisme de disarm/fencing D5, ni une qualification de durée ou de reprise sur moteur : la restauration de slots non vides et le service/API de bout en bout restent à tester.
