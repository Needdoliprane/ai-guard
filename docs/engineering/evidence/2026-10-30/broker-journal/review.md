# Revue ciblée FS/durabilité — journal broker

État final : code, preuves et corrections documentaires relus ; constats codec/versions/lock/documentation clos. Aucun défaut bloquant résiduel identifié dans le périmètre ciblé. Aucun test exécuté par cette revue.
Périmètre : `/Users/louis/Documents/ai-guard/.worktrees/broker-supervision`, branche `codex/broker-supervision`, base annoncée `3372150fdb99d8bc71700a33dd1d49c0e75c45b5`. Lecture seule ; aucune exécution de test, cargo, moteur ou modèle. Aucun fichier du dépôt modifié. Cette note ne vaut ni audit global ni qualification d'exécution.

Les deux répertoires de skills projet `.agents/skills/` et `.claude/skills/` sont absents de ce checkout. Politique du mainteneur et contrats d'architecture/code/qualification/tests lus. Le skill générique security-audit a été consulté en mode guidance ciblée, sans activer son audit global ni son workflow Daybreak.

## Faits du code à conserver lors de la construction

1. `control_fs::Directory` retient le FD et la chaîne dev/inode de ses ancêtres, refuse les symlinks, valide FS/owner/mode/ACL à l'ouverture/revalidation. Les fichiers passent type régulier, EUID, 0600 et nlink=1 (`src/control_fs.rs:482-595`). La garantie exclut explicitement les mutations même UID/root ; ne pas transformer les contrôles de métadonnées en prétention de résistance à l'administrateur.

2. Créer un répertoire ne le rend pas durable : `Directory::child` et `open_or_create` ne synchronisent pas le parent après `mkdirat` (`src/control_fs.rs:128-149`, `525-541`). Synchroniser le fichier puis son seul répertoire job ne suffit pas si le job/root/namespace vient d'être créé : chaque nouvelle entrée d'ancêtre doit être synchronisée avant ACK. C'est une exigence à imposer au nouveau caller, pas une vulnérabilité attribuée à tous les usages existants.

3. `Directory::snapshot` est explicitement éphémère : fichier fsync, aucune synchronisation du parent ici, `remove_on_drop=true`, suppression au dernier handle (`src/control_fs.rs:275-345`, `365-374`). Il ne constitue pas le binding manuel durable demandé. `RunPlan.credential_source` est `serde(skip)` (`src/runner.rs:324-325`) et `credentials_from_source` part déjà des bytes du `PrivateFile` initial retenu (`src/runner.rs:948-1011`) ; le nouveau journal doit persister ces bytes validés/canoniques depuis ce handle avant le marqueur, sans rouvrir la source après réservation/rejeu.

4. Une FD secondaire sur SQLite peut affecter ses POSIX locks à sa fermeture. `PrivateFile::open` ouvre d'abord le fichier (`src/control_fs.rs:618-625`), et `PrivateFile::revalidate` en ouvre/ferme un autre (`400-421`). Vérifier le digest ou JSON après cette ouverture est trop tard. Le code Store existant explique la politique : `private_entry` inspecte le nom sans ouvrir l'inode (`src/control_fs.rs:74-81`, `158-203`). Le périmètre retenu par le parent est constructible : journal/locks/bindings dans un namespace privé exclusif, noms générés et types fixes, refus hardlink/type/identité par fstatat avant open, jamais primitive lock SQLite ni binding à chemin libre. Le broker produit communique avec le gateway SQLite via HTTP et n'en possède pas les connexions. Une DB installée par embedding dans le namespace journal, ou une substitution même UID/root, reste un prérequis exclu explicitement ; nlink=1 et les seuls modes/inodes ne permettent pas d'identifier universellement tout inode SQLite inconnu. Tester la coexistence réelle d'une DB et d'un hardlink doit vérifier le refus avant open et la conservation des POSIX locks.

5. `managed::private_state` ne fournit pas encore la primitive journal immutable : `write` fait `renameat` remplaçant une cible après fsync fichier (`src/runner/managed.rs:118-175`), sans CAS révision/identité ; son check fichier ne valide pas l'ACL macOS (`267-276`). Sa lecture n'a qu'un contrôle de taille préalable et `from_reader` n'est pas une lecture bornée en flux (`93-117`). Ne pas copier ces méthodes telles quelles pour les nouveaux enregistrements stricts.

6. `PlanState::update` garde le mutex du plan pendant `Directory::write`, donc durant fsync (`src/runner/managed.rs:431-439`). Le futur arrêt préarmé ne doit pas dépendre de ce mutex ; c'est déjà une contrainte de conception de `BROKER_SUPERVISION.md:65-69`. Aucune assertion d'arrêt autonome n'est déduite du lot journal.

7. Le plan3 possède un binding durable existant : création exclusive, fsync du fichier via `write_json_new`, fsync du répertoire, digest du vrai fichier et référence UUID (`src/runner.rs:829-864`), validation UUID/résolution/argv (`867-899`), lecture figée par digest et identité (`903-941`). Préserver exactement binding_id/file_name/digest : copier les bytes dans le journal ne doit ni recalculer une nouvelle résolution ni produire un nouveau binding fournisseur.

8. L'intention est un composant pur, borné et opaque : domaine de hash propre, JSON canonique, provenance initiale ; aucun accès FS, identité tenant ou grant (`src/broker/intent.rs:1-16`, `ExecutionIntentV1`). Ses chemins initiaux ne prouvent ni fichiers présents ni credentials ; ne pas les utiliser comme autorité du journal. `validate_execution_intent_provider` reconstruit un RunPlan sans credential_source (`src/runner.rs:367-387`) : cela vérifie cohérence, jamais durabilité du manuel.

## Conditions à revoir sur le contrat puis le delta

- Réservation identité authentifiée/origin/namespace/installation/opération/Submit exact avant toute résolution. Rejeu exact compare avant source ou Auto. Une préparation partielle devient Interrupted et ne réessaye pas la résolution sous cette identité.
- ACK après binding explicite (None/manuel/plan3), intention, checkpoint0, marqueur et fsync de chaque fichier et entrée de parent. Publication immutable sans écrasement ; échec après publication reste ambigu/interrupted jusqu'à revalidation, jamais nouvel Auto.
- Révisions/checkpoint : précédent+hash+intention, chaîne sans trou, CAS sous verrou retenu, current atomique+parent fsync, refus d'ancien writer/clone/restart. Lecture conservatrice après crash ; absence/erreur/corruption ne devient pas état vide ou permission de nouveau start.
- Lock : retenu pour toute transaction ; clones partagent la même autorité et ne déverrouillent pas au Drop d'un clone. Toute acquisition sur une nouvelle ouverture doit être refusée si autre propriétaire ; pas de création de second nom/inode après remplacement.
- Quotas explicites bytes/inodes/jobs/revisions et lecture bornée ; aucun parcours glob/scan ou error-path permettant des temporaires illimités. Disque plein/readonly/sync échoué : aucun ACK faux, aucun effacement d'une preuve existante.
- Stockage ≠ permission d'exécution : aucun claim/start/renewal accordé par un journal accepté, aucun tenant choisi par données de plan, aucune reprise source/Auto/abonnement/API. Intégration auth fraîche avant accès futur, et stop d'un worker parti indépendant d'un credential fournisseur.

## Résultats et limites

PASS (lecture statique seulement) : primitives existantes et obligations de construction recensées avec callsites.
NOT_RUN : tout test, crash/power-loss réel, comportement FS/ACL/locks natif, moteur/modèle, service/IPC/ordonnanceur.
Contrat premier jet lu : `/tmp/ai-guard-broker-journal-contract.md`, SHA annoncé par son auteur `ec3fbe4c3b7006ce39c402e573f4a1c3028006611276e0dde381ad90a3d05ab4`. Son découpage en namespace dédié, réservation, bindings, rev0/current/marqueur, mutex+flock et reprises conservatrices est constructible. Les points ci-dessous sont des précisions à régler avant code, pas des vulnérabilités attribuées à une implémentation absente :

- Remplacer l'exclusion universelle de SQLite inconnu nlink1 par les conditions constructibles du point4 : namespace exclusif, noms fermés et préinspection avant open ; identités SQLite connues en embedding seulement. Parent et auteur du contrat avertis.
- Manuel : décider si `manual-env-v1.bin` conserve source brute exacte (puis reparse/canonicalisation du binding durable au handoff) ou bytes env canoniques. `credentials_from_source` ignore commentaires/lignes vides et trim les lignes ; source_sha256 et stored_sha256 ne sont donc pas nécessairement égaux si le second choix est retenu. Passer la copie brute directement comme handoff ne reproduirait pas nécessairement la sémantique actuelle.
- Checkpoint : rejeter aussi `human_approved=false, approval_hash=Some(_)`, et exiger le hash initial exact lorsque true. Le seul `policy::request_hash` ne suffit pas car il efface ces deux champs (`src/policy.rs:36-39`). Ce stockage ne vaut jamais autorisation humaine centrale.
- `advance` après un orphan rev+1 doit pouvoir confirmer uniquement des bytes identiques ; orphan divergent/partiel doit devenir Conflict/Interrupted, sans overwrite ni choix de la plus grande révision.

## Relecture FS gelée

SHA vérifiés directement :

- `src/control_fs.rs` : `ee6d53fb9b3dd181886dba6b24ac1a97830949441eb968abda95b637cbaf4670`.
- `src/control_fs/persistent.rs` : `95d616e977479efbce7364055feeb114d947587f5cecd2705f1203112b273c0d`.
- Handoff `/tmp/ai-guard-journal-filesystem-implementation.md` : `fded9aa4bdb1fa92cba2ec29398879f8640853a316da4dee7aa46dc5047c3dfc`.
- Contrat final `/tmp/ai-guard-broker-journal-contract.md` : `090cf39e1acfd6dbddd52daeb48a5304c7f40b5612d0a699c6fa270d3f5eec28`.

Constat statique : aucun défaut bloquant isolé identifié dans la primitive FS à ce gel. Ce constat ne couvre pas encore ses callers métier ni leurs barrières/budgets.

- Préopen réel : `persistent_read` appelle `private_entry` avant `open_file`, puis filtre les identités connues avant open et compare fstat au préflight (`persistent.rs:122-138`). Type/eUID/mode/nlink/ACL sont donc contrôlés avant ouverture, ce qui protège le cas concret du hardlink SQLite. Le cas nlink1 inconnu reste un prérequis de namespace, explicitement documenté.
- `persistent_revalidate` n'appelle pas le revalidate legacy : contrôle FD retenu/cache/digest, lecture bornée `read_at` sur cette même FD et nom metadata-only (`persistent.rs:478-518`). Un remplacement du nom n'ouvre aucune FD secondaire au nouveau fichier.
- `durable_bootstrap` retient et vérifie chaque identité d'ancêtre puis synchronise du child vers `/` (`persistent.rs:23-66`). `persistent_child(create=true)` synchronise enfant+parent (`71-108`). L'intégration doit appeler le bootstrap avant les ACK.
- Les créations immutable utilisent O_EXCL et ne réparent/suppriment rien (`169-211`). La seule publication remplaçante est fermée à `current-next.json`→`current.json`, revalide l'ancien inode retenu avant rename puis synchronise le parent et recoupe le nouvel inode (`217-267`). La protection concurrente dépend du mutex/flock du caller ; aucune atomicité contre acteur hostile même UID n'est annoncée.
- Enumeration : `F_DUPFD_CLOEXEC` concerne seulement un répertoire ; compteur borné avant push ; noms/types/ACL vérifiés sans open des réguliers ; errno remis à zéro avant chaque readdir et EOF distinct d'erreur ; `closedir` vérifié sur succès (`272-348`). Le partage de l'offset par dup exige la sérialisation du caller comme documenté.
- flock est appliqué au FD retenu, aucun `LOCK_UN` explicite ; clones PrivateFile partagent Arc/File (`533-544`). L'Inner Journal doit en conserver la durée de vie.

NOT_RUN : exécution réelle coexistence SQLite/POSIX locks, deux processus/clones, ACL et errno/FS, fsync/rename natifs et pannes. Ces preuves appartiennent à Luna ; cette revue n'en invente aucune.

## Relecture globale du candidat gelé

Manifeste reçu `/tmp/ai-guard-broker-journal-implementation.md` (SHA annoncé par le parent `776eae0828c6b1e6852b4ecdd865199abd2a5973578b3eab2fd63bc202130978`). SHA des sources vérifiés directement :

```
514cda1a28941a90a2c205469a6b78f64e287052adbfd5b013299fdfb3edde52  src/broker/journal.rs
b7c77b0d448471c1ef3ffc8b765a62abfd7a525d1f409378d6711b48b6d55c63  src/broker/journal/types.rs
79b4ac6542f52a00cd62dc9e290d39540a71a9d66f16f22affb1a07c5b9afe98  src/runner.rs
1e7f8acf1a37a37ce616519428c3e624a0c64cb14b422bc769f1bd9769854f0f  src/provider.rs
ee6d53fb9b3dd181886dba6b24ac1a97830949441eb968abda95b637cbaf4670  src/control_fs.rs
a8d5506d48d146b731d80a30722063171e6409efe59d268a0b9ad085030e6ec3  src/control_fs/persistent.rs
```

`src/broker/mod.rs` ne fait qu'exporter le module journal ; aucun autre chemin d'exécution activé. Le seul changement du helper FS depuis sa première relecture est `PersistentLimit` typé et son retour au dépassement d'énumération ; `fscheck` le mappe par type en `LimitExceeded`, sans classifier des chaînes OS.

Résultat de la revue statique globale : **aucun défaut bloquant d'intégrité, de durabilité de séquence ou d'autorisation identifié dans le périmètre examiné**. Cette phrase ne remplace aucun résultat Luna et ne donne aucune qualification runtime/production.

- Réservation : Submit exact borné et hashé avant I/O ; ticket nonClone lié à l'Arc et Weak lease ; replay compare avant toute préparation/source ; perte ticket conserve Interrupted (`journal.rs:265-335`). Aucun code de résolution Auto/source provider n'est appelé au replay.
- Binding : source PrivateFile retenue revalidée sans seconde FD ; manuel ENV canonique issu du parseur historique pur partagé, UUID stable persisté dans manifeste, source/stored/handoff SHA distincts ; provider raw conservé byte-for-byte, ref UUID/fichier/digest exacte à l'intention. Load ne relit que la copie durable, vérifie contexte/client/auth/secret et hashes (`journal.rs:574-762`, `provider.rs:196-249`).
- ACK initial : budget appliqué avant créations, bindings puis intention/manifeste/rev0/current/accepted dernier, chacun sync fichier+parent, puis load/resync graphe avant résultat (`journal.rs:339-465`). La réservation et les répertoires étaient déjà durables ; les erreurs gardent des partials sans nouvelle tentative sous la même identité.
- Révisions : CAS et mutex partagés, chaîne complète précédente+hash/intention/op ; replay historique compare exactement le hash du candidat à son successeur confirmé ; orphan existant refuse, n'est pas promu ; pointeur publié uniquement via la primitive étroite ; load resynchronise avant tout reçu et plancher vivant est mis à jour sous mutex avant ACK (`journal.rs:477-569`, `764-891`).
- Concurrence : Inner retient lock FD et Writer Mutex ; clones partagent tout ; ticket retient aussi l'owner. Aucun nouveau lock inode n'est inventé au Drop/restart et aucun unlock explicite prématuré n'existe. Le plancher vivant refuse un rollback observé ; il ne revendique pas d'anti-restore après restart.
- Ressources : comptage de tous les fichiers/répertoires attendus et partials, bornes enum avant croissance Vec, budgets avant ajout et pic de publication rev+temp. Les hard caps sont trusted et aucun quota physique n'est prétendu. Type inconnu/corruption/erreur n'est pas une absence silencieuse.
- Checkpoint : état réel conservé séparément des options et de la ref fournisseur ; comparaison stable avant projection ; approbation false+hash refusée, true liée au hash initial exact ; validation contextuelle scope/admission/worker/owned/helper/handoff avant stockage/relecture. Aucun reçu journal n'est un claim, une autorisation tenant ou un droit d'exécution.
- ControlSecrets : littéraux seulement, contrôle des bytes et clés/strings JSON décodées ; helper provider vérifie également le secret réel et les tokens Codex imbriqués, pas seulement la liste déclarée du record. Debug et erreurs publiques restent opaques ; aucune nouvelle extraction de source OAuth ou appel fournisseur.

### Observation non bloquante — premier open concurrent

Condition statique reproductible à tester : deux processus ouvrent simultanément un scope vierge et tous deux observent `.writer.lock` absent. Le perdant de la création exclusive retourne `JournalError::Io` (`journal.rs:218-224`) au lieu de relire le lock puis renvoyer `Busy` lorsque l'autre processus le détient. La propriété conservatrice est préservée : aucun second inode, aucune prise de contrôle ni aucun start. C'est une divergence de diagnostic par rapport à « ouverture indépendante Busy », pas une vulnérabilité d'autorité. Parent et auteur avertis ; décision parent attendue si le contrat inclut aussi ce bootstrap concurrent.

### Limites du verdict

PASS : lecture statique ciblée et vérification des SHA ci-dessus ; conditions initiales de construction traitées dans ce candidat.
NOT_RUN par ce reviewer : tests/cargo/build/clippy, injections, SIGKILL, POSIX locks multi-processus et coexistence SQLite réels, ACL/autre UID et plateformes natives. Luna possède ces commandes/preuves ; ses résultats ne sont pas présumés ici.
Hors périmètre : service/IPC, identité fraîche des futurs callers, scheduler/TTY, fencing et start, cloud/PKI, capacité100, quotas physiques, power-loss et anti-admin/mêmeUID/root/restore.

Cette revue permet au parent de poursuivre les validations du lot ; elle ne ferme aucun gate et ne constitue pas une approbation automatique.

## Complément codec/contexte/version après correction des champs nullable

SHA effectivement relus : runner `d0395e1c0a853b36f9c8dd3860c5a58d547c0ae0f47d51a0ef047dbce787f912`, journal inchangé `514cda1a28941a90a2c205469a6b78f64e287052adbfd5b013299fdfb3edde52`, Cargo.toml `eb55ba75b62464b8963ea03261503949f02a31de1934bec35d680e96e8913e75`, Cargo.lock `dc513f61ee7fb60d6a37d936da5818af2d4101586410d1e89b10b53c830ec37f`.

### Codec — correction statiquement cohérente

La comparaison Value précédente a été retirée. `from_plan_bytes` désérialise le vrai RunPlan à travers `serde_ignored`, rejette toute valeur ignorée, appelle explicitement `decoder.end()` puis compare la projection stable (`runner.rs:404-440`). Un champ connu présent à null suit la désérialisation réelle du type ; un champ inconnu null n'est pas confondu avec une omission. Le wrapper Deserialize utilise le même garde pour ses sous-types, puis valide sa version et ses bornes ; les chemins from_bytes utilisent `serde_json::from_slice`, qui impose la fin du document. Erreurs et Debug restent statiques, sans chemin de champ ni valeur sensible. Le lecteur historique `read_plan` n'a pas été changé ; le garde strict est propre à la nouvelle projection.

Le code local de `serde_ignored`0.1.14 a été consulté : le callback est déclenché par deserialize_ignored_any ; visit_none transfère le cas Option connu à son visiteur. Le lock n'ajoute qu'un paquet0.1.14. Aucun nouveau défaut de codec trouvé par cette revue. Les PASS codec/intégration annoncés par le parent appartiennent à Luna ; aucun test exécuté ici.

### Hypothèse origine différente — rejetée sur le vrai caller

`validate_journal_context` runner retourne bien tôt pour managed=None (`runner.rs:509-511`), mais ce helper pub(crate) est appelé par `Journal::context`, qui compare **avant** les origins de scope et d'intention (`journal.rs:960-966`). accept/load/advance passent tous par ce caller ; gateway A et B d'origins différents donnent donc Conflict même sans managed. L'hypothèse d'un intent B accepté dans scope A n'est pas un défaut du journal actuel. Aucun invariant d'authentification n'est ajouté à une donnée de stockage.

### Écart confirmé par source — versions inconnues imbriquées conservées

Le contrat `docs/engineering/BROKER_JOURNAL.md:123` affirme « toute version inconnue refuse ». Cette promesse n'est pas actuellement tenue pour tous les types du vrai ManagedCheckpoint :

1. Condition minimale : partir d'un payload managed valide avec worker et pending (ou uncertain). Changer seulement `pending.version` en2, garder worker_id et attempt_id exacts. `WorkerMutationV1.version` est un u32 sans validation Serde (`control.rs:249-255`) ; la boucle contextuelle ne vérifie que les deux identités (`runner.rs:595-600`). Builder/canonical/context/advance/load acceptent donc cette version inconnue. Effet démontré par le chemin source : ACK de données de reprise que le Store refusera ensuite, puisque `audit/control.rs:1174` impose déjà version1. Ce n'est pas un droit d'exécution ni un contournement du Store. Correction étroite : vérifier version1 dans cette boucle et conserver un test négatif pending/uncertain.
2. Même condition pour owned ou import.owned : conserver toutes les identités et changer `observation.configuration_fingerprint_version` en999. Le champ est Option<u32> (`runtime.rs:1227-1230`) ; les branches contextuelles (`runner.rs:602-640`) n'en vérifient pas les versions. Le runtime connaît1..=3 (`runtime.rs:1420`) et refuserait l'effet, mais le journal aura accepté ce snapshot incompatible. Le handle a également un champ de version Option<u32> (`runtime/managed.rs:85-96`). Vérifier les versions historiquement connues sans réécrire les snapshots ; ne pas imposer arbitrairement la version courante aux anciennes données connues.

`WorkerV1` et `WorkerAttemptV1` n'ont pas eux-mêmes de champ version. Leurs receipt/launch sont liés au receipt et launch validés de l'admission (versions1) ; ce chemin n'est pas un cas supplémentaire. `binding.invocation.policy_version` mérite le même inventaire des versions connues (historique/actuelle), pas une prétention d'adoption runtime depuis le journal.

Ces écarts de validation/compatibilité doivent être corrigés ou la promesse du contrat explicitement bornée avant clôture. Ils n'établissent aucune vulnérabilité runtime actuelle. Parent averti ; correction et régression ciblée restent à relire.

## Relecture des corrections de versions et de l'oracle SQLite

Gels vérifiés : runner `a110d4010c310a19582a95a7f6378a890b32bf40edf0515b717e8385a734f61e`, runtime/managed `588bd096a37058e8747f4c3cbc27c29a860015537af1098761c23f4b4c523b14`, test SQLite `55375d225c09c312304e0e9dcd5bf64747a00451fbba83cbd024691dbf275f39`. Journal contient seulement à ce stade le hook cfg(test) WriterLockAbsent, gel `9a2c9fd4683e2033b1a627ed29715a1d4a96558a3df936067b85d850a3dbf3ba` ; la correction de course n'est pas encore incluse dans cette relecture.

### Versions — constats précédents traités dans le code

`canonical_bytes` appelle désormais `validate_versions` (`runner.rs:457`). Ce garde refuse mutations pending/uncertain de version différente de1 ; exclusion workspace différente de1 ; invocation hors versions connues1/2 ; handle et observation owned/import.owned hors versions1..=3 (`runner.rs:476-515`). L'absence de version fingerprint conserve le défaut historique1. Le getter de handle est seulement rendu pub(crate), sans modifier sa sémantique. Les anciens lecteurs RunPlan ne sont pas modifiés et aucune migration/adoption runtime n'est déduite d'une lecture.

Les cas de versions inconnues signalés précédemment sont donc fermés par inspection du chemin réellement appelé par builder/Deserialize/canonical/context. Les régressions Luna relues dans le source couvrent négatifs mutations, invocation, handles/observations et exclusion999 ainsi que positifs historiques1/2/3. Cette revue ne confond pas ce contrôle de versions connues avec la qualification de toutes les observations runtime, leurs effets ou leur provenance.

### SQLite — oracle source correct sur le scénario nommé

Test source et rapport `/tmp/ai-guard-journal-sqlite-tests.md` relus, ainsi que `/tmp/ai-guard-journal-sqlite-diagnostic4.log`. Ce dernier constate 1PASS, exit0, aucun ignored. Ce résultat a été exécuté par Luna, pas par le reviewer.

- Le PID propriétaire garde simultanément Store et une autre connexion rusqlite ouverts (`tests/journal_sqlite.rs:222-227`). Sa FD brute de base est ouverte une fois puis retenue pendant les deux sondes et le refus ; sa lecture utilise read_at sur cette FD (`129-142`, `230`, `257`, `266`). Aucune FD du même inode n'est ouverte puis fermée dans ce PID pendant l'oracle.
- Le premier child vérifie F_GETLK sur le main-file puis exige l'échec du changement WAL→DELETE avant l'effet Journal. Le dispatch enfant retourne depuis le test supérieur après la seule sonde (`146-176`, `198-205`), sans lancer récursivement la fixture.
- L'injection remplace le pointeur current par un hardlink exact du main-file, inode identique et nlink2 contrôlés (`248-254`). Le Journal::open/refus a lieu dans le PID du Store, via son inventory metadata (`258-264`). La seconde sonde F_GETLK vient immédiatement après ce refus, avant tout appel SQLite parent qui pourrait réacquérir ses locks (`265`). Cet ordre permet de détecter une ouverture/fermeture accidentelle par le produit.
- Les ouvertures/fermetures du main-file pour les sondes sont exclusivement dans les children ; elles n'effacent pas les locks POSIX de leur parent. L'assertion bytes compare le même FD retenu. Le cleanup retire seulement le hardlink injecté par cette fixture puis vérifie append/intégrité Store et nlink1 (`268-283`).

Les trois FAIL de harnais précédents restent visibles dans le rapport, y compris le faux effet produit dû au fs::read parent corrigé. Aucun oracle n'a été affaibli dans le candidat relu.

Limite de preuve : refus de ce main-file hardlink durant l'inventory Journal::open sur la plateforme d'exécution annoncée. Ce test ne qualifie pas tous les sidecars, un alias nlink1 inconnu, tous les callsites d'ouverture, Linux/autre UID, power-loss ou une barrière de sécurité contre mêmeUID/root. Les garanties structurelles des autres callsites restent issues de la revue FS et des tests distincts.

## Relecture de la correction du premier lock

SHA vérifiés : journal `9de3129179300bee2fbb4558ef5fbf5c4e1269713b91a312790094316c685836`, persistent `b5d664789a250978ab7556ddf3b48e8ad64dfb63f7caeaa3c7991ccf0a747726`, manifeste `/tmp/ai-guard-broker-journal-implementation.md` `a31ad53e9c6564bea7116e19eed94def62ec6292f6c7aa8fda20ed6a081942ff`.

Le diagnostic concurrent est traité par une lecture unique bornée : `persistent_create` renvoie un io::ErrorKind::AlreadyExists lorsqu'un préflight metadata a validé une entrée existante ; le même kind peut provenir d'O_EXCL si l'entrée apparaît entre préflight et create (`persistent.rs:185-200`). Journal.open n'intercepte que ce type, relit une fois par persistent_read avec toutes ses gardes préopen/ACL/exclusions, puis synchronise et tente le flock (`journal.rs:222-242`). Une entrée unsafe ou disparue refuse ; aucun second nom/inode, boucle, repair ou nettoyage n'a été ajouté. Cette lecture n'élargit pas la garantie à une course hostile mêmeUID/root.

Le nouveau test `simultaneous_first_writer_creation_returns_busy` place le child après le vrai constat « lock absent » mais avant O_EXCL (hook cfg(test) seulement). Le parent crée et garde alors un vrai writer/flock ; le child est libéré et doit renvoyer Busy (`journal/tests.rs:333-399`). Entrée child explicitement --ignored, ChildGuard possédant le processus sur échec, attente de barrière bornée. Cet oracle distingue la correction du précédent Io, et ne remplace aucune primitive FS par un succès simulé.

Observation non bloquante de diagnostic fermée par inspection du correctif ; résultat GREEN à joindre depuis Luna. Aucun nouveau défaut bloquant trouvé dans ce delta.

## Revue finale des preuves et documents staged

Dossier relu : `docs/engineering/evidence/2026-10-30/broker-journal/` (README, tests, sqlite-tests, Linux, logs et inspections) ; contrat BROKER_JOURNAL et deltas CURRENT_STATE/VALIDATION/TEST_STRATEGY/IMPLEMENTATION_PLAN/DECISIONS. Rapports bruts historiques gardés tels quels avec leurs gels et mentions NOT_RUN d'alors ; le README et les paragraphes finaux portent la synthèse actuelle.

Vérifications read-only effectivement exécutées :

- `shasum -a 256 -c docs/engineering/evidence/2026-10-30/broker-journal/source.sha256` : exit0, **111/111 OK**, sur le worktree final.
- Manifeste source et ses copies avant/après release : SHA256 identique `cd9775b29d3fa75a67b08ddd395a081e980665c95369fcb2c84f5a326302d520`.
- Artefacts conservés `/tmp/ai-guard-journal-qualified.LtbWas` : binaire `39d88ea07a6ec7de50288d726ce263647cbde20bd7fbfd3cce28483a8efd22a4`, rlib `1419376473a47dcc2d02cdc18e2129e7524144f6c844844c43e493803536ab92`, archive `bc613e718b23bcdaac49233861a4bf13e93352b804ef2ce8150e1096c6204e14` ; trois hashes recomputés conformes.
- Trois if imbriqués convertis en let-chains relus dans runner final : conditions, branches et retours inchangés. Le README dit correctement que les tests macOS précèdent ce delta mécanique et que Linux exécute le gel final.
- Lecture des logs macOS : intégration10, unités4actives+2helpers ignorés, SQLite1, canaris15+7+4=26, Clippy strict PASS et release40,20s. Le self-test de hook est nommé comme harnais et les helpers ne sont pas recomptés comme PASS produit.
- Lecture Linux container.log : mêmes groupes4+10+1, exit0 ; aucune nouvelle fonction unique revendiquée. Inspect final : UID1000:1000, networknone, cap-dropALL, NNPtrue, privilegedfalse,2CPU,2GiB,256PID, mounts[]/bindsnull/devices[]. Les rapports séparent build non borné, limites configurées et consommation non mesurée. Source recopiée111/111 prouvée par le fichier conservé, sans nouveau démarrage exécuté par cette revue.
- `git diff --check --cached` général ne relève que six nouvelles lignes vides finales dans les logs bruts. Elles sont délibérément conservées pour ne pas réécrire les octets des preuves ; le périmètre de contrôle whitespace annoncé par root exclut ces logs. Aucune correction de ces preuves n'est demandée.

Les compteurs, hashes, rétention des trois FAIL SQLite, huit refus versions dont sept seulement pré-fix, RED versions/lock non conservés en fichiers, logs initiaux fmt/Clippy écrasés et limites de plateforme sont cohérents entre synthèse et preuves. Les scénarios managed complets via Journal et rollback contre plancher mémoire sont explicitement NOT_RUN/requis avant activation. L'intervalle rename/fsync, power-loss, runtime workers, IPC/scheduler/TTY/D5, cloud, capacité100 et release production restent ouverts ; aucun gate n'est fermé par ces PASS ciblés.

### Dernier correctif documentaire demandé

`docs/engineering/BROKER_SUPERVISION.md:125` porte encore **« État courant »** puis « Journal durable [...] restent à livrer ». Cette formulation active contredit l'ajout CURRENT_STATE qui annonce le composant journal et ses preuves. Demande au parent : compter le composant journal parmi les éléments avec preuves distinctes et laisser son raccord au service/IPC/ordonnanceur/gardien/D5 explicitement à livrer. Cela ne nécessite aucune réécriture de rapport brut historique ni changement de source/test. Parent averti.

Verdict à cet instant : **revue source ciblée close sans défaut bloquant résiduel ; cohérence des preuves acceptée pour leur portée déclarée ; correction de l'état courant du design à relire avant clôture documentaire.** Tous les tests restent exécutés par Luna/agents dédiés ; aucun test, modèle, moteur ni audit global lancé par ce reviewer. Aucune approbation de déploiement, activation ou qualification production.

## Verdict final après correction documentaire

Correction du parent relue directement dans le worktree :

- `BROKER_SUPERVISION.md:125` indique désormais le composant journal et ses vérifications ciblées distinctes ; raccord journal/service/IPC, ordonnanceur, gardien et D5 restent à livrer. La contradiction d'état courant est supprimée.
- `CURRENT_STATE.md:17` qualifie l'ancien exécutable de « binaire de ce lot » et distingue ce que le seul format d'intention ne livre pas du nouveau composant journal prouvé.
- `DECISIONS.md:13` identifie explicitement la décision d'intention comme historique, préservant ses résultats datés sans la présenter comme état actuel du journal.
- Le manifeste source reste `cd9775b29d3fa75a67b08ddd395a081e980665c95369fcb2c84f5a326302d520` ; aucune modification applicative/test n'est impliquée par ces retouches.

**Verdict final : revue ciblée CLOSE, sans constat bloquant résiduel.** Les constats concrets codec nullable, versions imbriquées, diagnostic du premier lock et contradiction documentaire sont traités. Les preuves finales étayent les seuls PASS logiciels/filesystem annoncés sur macOS ARM64 et Linux ARM64 virtualisé ; leurs limites et lacunes de rétention sont explicites. Le rapport ne convertit pas les helpers/self-test en PASS produit supplémentaire.

**Écarts maintenus ouverts, correctement déclarés NOT_RUN :** contextualisation managed complète au niveau Journal, rollback contre plancher mémoire, intervalle rename/fsync, panne électrique, plateformes/runtimes non exécutés, activation IPC/service/scheduler/TTY, fencing/reprise, capacité100 et cloud. G09 reste FAIL et aucun gate global n'est clôturé. Ces écarts bloquent leurs qualifications/activations respectives ; ils ne sont pas masqués par la clôture de cette revue du composant de données.

Aucun test/build/modèle/moteur exécuté par le reviewer, aucune modification du dépôt, aucun commit/push. Seule cette note externe a été écrite. La décision d'intégration du lot reste au parent ; aucune approbation automatique de déploiement, de service OS ou de release production n'est donnée.
