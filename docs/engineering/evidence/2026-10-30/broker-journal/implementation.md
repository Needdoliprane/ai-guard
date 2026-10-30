# Journal broker — implémentation candidate

Worktree exclusif `/Users/louis/Documents/ai-guard/.worktrees/broker-supervision`, base `3372150fdb99d8bc71700a33dd1d49c0e75c45b5`. Contrat autoritatif `docs/engineering/BROKER_JOURNAL.md`. Aucun commit/push ni modification du checkout principal par cet agent. Originaux de runner/control_fs/mod conservés `/tmp/ai-guard-broker-journal-original/`.

## Changements

- Journal FS privé réel : scope fermé, flock retenu partagé par Arc, réservation depuis bytes Submit exacts hashés avec domaine, ticket nonClone et Weak lease (InProgress vivant/Interrupted après perte), contrôle littéral de secrets connus avant I/O Submit et avant écriture de chaque donnée structurée/handoff.
- Acceptation après copie durable explicite None/manuel ENV canonique/provider raw identique : identité de binding, source/stored/handoff SHA distincts, manifeste fermé, intention et checkpoint0, pointeur current puis accepted dernier marqueur ; fsync fichiers et parents avant reçu. Copie durable récupérable par PrivateFile retenu, sans source d'origine ni résolution Auto au replay.
- Révisions immutables et chaîne hashée, CAS current, reçu historique exact même après révisions suivantes, orpheline jamais promue, plancher revision/hash vivant partagé avec clones. Budgets contenus/entrées/jobs/révisions comprenant les partiels et le pic de publication, defaults/hard caps distincts, aucun nettoyage de preuves.
- Wrapper public opaque `runner::CheckpointPayloadV1` sur les vrais types : demande courante et ManagedCheckpoint privé. Projection compare intention/options/input/référence initiaux ; approbation conservée, false+hash interdit, true lié au hash initial exact. Revalidation scope/admission/manifest/runtime/worker/owned/helper/handoff à accept/load/advance. Le payload ne donne aucune autorisation centrale.
- Parseurs credentials purs partagés par chemins historiques : extraction manuelle identique aux règles existantes, record provider mêmes champs et validation ; aucune nouvelle lecture source ni nouveau mode fournisseur.
- Helpers FS sous ownership séparé `/root/journal_filesystem_core` : préinspection metadata/ACL/nlink/exclusions avant ouvrir les fichiers ; retained revalidate sans FD secondaire ; publication étroite current-next→current ; bootstrap et parents synchronisés ; scans metadata bornés sans ouvrir les réguliers.

## Validation constatée

- RED composant absent exécuté par Luna : `/tmp/ai-guard-broker-journal-red.kyXIpo/test.log`, import broker::journal et CheckpointPayloadV1 absents avant code. Ceci est un RED de fonctionnalité, aucune vulnérabilité runtime prétendue.
- PASS : rustfmt des seuls fichiers owned (édition2024, skip_children=true). Commande exécutée par core ; aucune compilation/test/cargo.
- NOT_RUN à ce stade : GREEN Luna, lint/build, suite FS et injections, revue indépendante sur le delta. Root déclenche après gel conjoint source/FS.
- Hooks uniquement cfg(test) vers module Luna : événements après véritables écritures/synchronisations/publications ; aucun envflag prod/moteur/modèle/mock-success. CurrentPublished/CurrentDurable suivent le helper FS qui groupe rename+sync ; ils ne prouvent pas le comportement entre ces deux syscalls ou une panne électrique.

## Limites explicites

Le composant conserve des données privées, pas une permission de start, un claim, une lease Store, une identité tenant authentifiée ou du fencing après restart. Namespace exclusif sans SQLite interne et contrôle account trusted ; aucun anti-admin/mêmeUID/root/VM snapshot. FsExclusions protège seulement les identités réellement connues. ControlSecrets est littéral et fourni par caller, pas un DLP d'encodages arbitraires. Quotas applicatifs, pas quotas physiques bytes/inodes ; capacité100, IPC, arrêt autonome/scheduler/TTY, cloud et power-loss non qualifiés. Les opérations fsync peuvent bloquer ; le futur scheduler ne doit pas attendre le writer.

## Gel

En attente du marker FS de limite typée puis manifeste final conjoint. Les fichiers de tests Luna et docs root ne sont pas dans l'ownership core.

### Gel source candidat pour GREEN/revue

Scope additionnel provider.rs approuvé par root : helper pure validate_frozen_credential réutilise BootstrapPayload/parse_bootstrap_argv/codex_auth/validate_secret pour client/auth/secret. Ses chaînes/clés décodées (tokens imbriqués inclus) sont contrôlées avant ACK et au load, indépendamment de record.secrets déclaré. Aucun comportement historique modifié. Ownership FS transféré uniquement pour ajouter PersistentLimit typé ; exact downcast vers LimitExceeded, aucun autre delta sur publication/scans FS après le gel agent95d616.

```
514cda1a28941a90a2c205469a6b78f64e287052adbfd5b013299fdfb3edde52  src/broker/journal.rs
b7c77b0d448471c1ef3ffc8b765a62abfd7a525d1f409378d6711b48b6d55c63  src/broker/journal/types.rs
6dcd999d1cdd83a191312c3c4c60356c1a46d995bab3e0efc8e098764f2ee084  src/broker/mod.rs
79b4ac6542f52a00cd62dc9e290d39540a71a9d66f16f22affb1a07c5b9afe98  src/runner.rs
1e7f8acf1a37a37ce616519428c3e624a0c64cb14b422bc769f1bd9769854f0f  src/provider.rs
ee6d53fb9b3dd181886dba6b24ac1a97830949441eb968abda95b637cbaf4670  src/control_fs.rs
a8d5506d48d146b731d80a30722063171e6409efe59d268a0b9ad085030e6ec3  src/control_fs/persistent.rs
```

Source application GELÉE ; ne pas éditer durant les commandes Luna et la revue. Les résultats restent NOT_RUN jusqu'aux journaux fournis par les agents indépendants.

### Correction réelle CP connu-null, avant GREEN final

Luna a reproduit le refus d'un plan valide contenant request.execution_mode:null : le contrôle comparait à la sérialisation qui omet None. RED `/tmp/ai-guard-broker-journal-green.1496/checkpoint-builder.log`. Root a ajouté serde_ignored0.1.14 (Cargo owned root) pour détecter les champs réellement ignorés via Serde, sans liste fragile nullable ni reflet ManagedCheckpoint.

Correctif borné au nouveau codec runner : adaptation RunPlan dans from_plan_bytes avec drapeau booléen ignoré et JsonDeserializer::end explicite ; adaptation Deserialize CheckpointData public avec erreur statique opaque ; options/provider_input typés comparés à l'intention ; aucune remise à zéro d'approbation. checkpoint_known_fields et les secondes lectures Value du codec supprimés. Les lecteurs historiques restent inchangés. Journal/FS ne changent pas dans ce delta. Rustfmt owned PASS ; GREEN ciblé attendu, aucun cargo par core.

Nouveau runner SHA `d0395e1c0a853b36f9c8dd3860c5a58d547c0ae0f47d51a0ef047dbce787f912` remplace79b4ac…fe98. Cargo root SHA eb55ba75b62464b8963ea03261503949f02a31de1934bec35d680e96e8913e75 / lock dc513f61ee7fb60d6a37d936da5818af2d4101586410d1e89b10b53c830ec37f. Source de nouveau gelée pour validation ciblée et revue.

### Correction versions imbriquées après RED managed réel

Luna a construit une admission/worker SQLite valides et reproduit sept variantes de versions incompatibles acceptées par le builder (pending/uncertain2 ; invocation policy999 ; handle/observation owned/import999). Le garde du nouveau codec CP est désormais appelé par canonical_bytes : mutations1 seulement, invocation policies1/2 connues (1 est historique lisible, aucune adoption runtime accordée), fingerprints absents=1 puis1..3, workspace_control_exclusion_version absent/1. Incompatible retourne UnsupportedVersion avant sérialisation ; les erreurs Serde publiques gardent leur masque statique. Aucun lecteur historique modifié et aucune réécriture de snapshot.

Seul delta runtime : le getter existant ContainerHandle::fingerprint_version devient pub(crate), sans changer son calcul. Source de nouveau gelée pour GREEN ciblé ; aucun cargo/test par core. Rustfmt owned PASS.

```
a110d4010c310a19582a95a7f6378a890b32bf40edf0515b717e8385a734f61e  src/runner.rs
588bd096a37058e8747f4c3cbc27c29a860015537af1098761c23f4b4c523b14  src/runtime/managed.rs
```

Journal514cda…de52 et persistent a8d550…6ec3 inchangés. La course de diagnostic du lock reste séparée, non corrigée dans ce delta ; attente de sa preuve déterministe avant changement.

### Correction bootstrap concurrent du writer lock

RED réel Luna : deux ouvertures du même scope, enfant arrêté après le vrai read lock absent, parent acquiert et retient le flock ; enfant repris renvoyait Io au lieu de Busy. Commande ciblée `cargo test --locked --lib broker::journal::tests::simultaneous_first_writer_creation_returns_busy -- --nocapture`, exit101 sur assertion Io/Busy ; journal préfix9a2c9f…f3ba et tests2191c6…47a.

Delta minimal : persistent_create retourne io::ErrorKind::AlreadyExists uniquement lorsque private_entry a déjà validé l'entrée existante (type/eUID/mode/nlink/ACL), ou reçoit EEXIST de O_EXCL. Les autres erreurs gardent leur refus. Journal.open, sur ce SEUL type d'erreur, relit une seule fois le lock via persistent_read avec toutes les gardes préopen/exclusions, synchronise et tente le flock. Aucune boucle, nouveau nom/inode, réparation ou retry d'une entrée unsafe. Le hook WriterLockAbsent reste cfg(test) uniquement, après vrai read metadata et sans oracle de succès.

Source GELÉE pour GREEN final/revue et canaris choisis par root. Rustfmt owned PASS ; core n'a exécuté aucun cargo/test/build. Le GREEN des versions est signalé par Luna ; attente du GREEN lock et des validations finales avant clôture.

```
9de3129179300bee2fbb4558ef5fbf5c4e1269713b91a312790094316c685836  src/broker/journal.rs
b5d664789a250978ab7556ddf3b48e8ad64dfb63f7caeaa3c7991ccf0a747726  src/control_fs/persistent.rs
a110d4010c310a19582a95a7f6378a890b32bf40edf0515b717e8385a734f61e  src/runner.rs
588bd096a37058e8747f4c3cbc27c29a860015537af1098761c23f4b4c523b14  src/runtime/managed.rs
b7c77b0d448471c1ef3ffc8b765a62abfd7a525d1f409378d6711b48b6d55c63  src/broker/journal/types.rs
6dcd999d1cdd83a191312c3c4c60356c1a46d995bab3e0efc8e098764f2ee084  src/broker/mod.rs
1e7f8acf1a37a37ce616519428c3e624a0c64cb14b422bc769f1bd9769854f0f  src/provider.rs
ee6d53fb9b3dd181886dba6b24ac1a97830949441eb968abda95b637cbaf4670  src/control_fs.rs
eb55ba75b62464b8963ea03261503949f02a31de1934bec35d680e96e8913e75  Cargo.toml (root)
dc513f61ee7fb60d6a37d936da5818af2d4101586410d1e89b10b53c830ec37f  Cargo.lock (root)
```

### Micro-correction Clippy finale

Retour root/Luna : 10 groupes journal et4 tests unitaires PASS,2 helpers ignorés explicitement non comptés,26 canaris PASS. Clippy strict a ensuite refusé trois collapsible_if du nouveau contexte runner (binding, owned worker, owned import). Aucun défaut fonctionnel nouveau signalé dans ce retour.

Delta mécanique autorisé : let-chains, mêmes prédicats dans le même ordre et mêmes retours Conflict ; aucun autre fichier modifié. Rustfmt owned PASS. Nouveau runner SHA `3b7f949519bc7f8bcd2f1b504e97e09ad1950f48de7c03c80aed0e660deb3805` remplace a110d4…f61e. Application de nouveau GELÉE pour unique relecture Clippy et build release par Luna ; aucun cargo ni tests core. Les journaux finaux root/Luna restent nécessaires avant clôture.
