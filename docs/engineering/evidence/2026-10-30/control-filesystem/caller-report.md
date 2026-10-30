# Filesystem contrôle — intégration caller du 30 octobre 2026

Ce lot est distinct du gel261 archivé du 29 octobre dans `/tmp/ai-guard-invocation-freeze.iJpHZL`. Aucun résultat261 ne qualifie ces nouveaux changements. Aucun commit, nettoyage global, moteur, cloud ou PKI exécuté par cet agent. Les skills de projet `.agents/.claude` sont absents ; chargement automatique non confirmé. Contrats AGENTS/architecture/code/gates/tests et design filesystem lus.

## Changements

- Le parseur de credentials conserve ses règles de normalisation. La source est un `PrivateFile` validé et retenu ; les bytes canoniques `NAME=value\n` produisent un `Snapshot`600 dans `config/credential-snapshots`, sans `TMPDIR`. Le digest SHA256 hex est celui de ces bytes, jamais une réouverture du snapshot. Borne64KiB appliquée aux bytes canoniques avant snapshot/admission.
- `ContainerSpec.secret_env_file` devient un handle opaque partagé. `with_secret_env_file(&Path)` reste compatible en ouvrant un PrivateFile contrôlé64KiB ; `with_secret_env_handle` a la même borne. Le handle demeure vivant jusqu'au create ; `create_args` et le trajet legacy revalident avant le handoff CLI. Aucun FD dans les DTO ou checkpoints.
- `choose_plan` compare l'identité physique du projet aux ancêtres FD de la source, après validation de celle-ci. Les alias de casse du filesystem actuel ne contournent plus l'exclusion. Un champ uniquement volatil `#[serde(skip)]` retient ce premier handle et le transmet au parseur ; aucun champ persistant ou option CLI ajouté.
- La reprise prépare une seule copie vérifiée des credentials avant tout nouveau helper/premier start. Source privée, collision avec les clés du contrôle et digest admis restent obligatoires. Les claims consommés, les états terminaux et le diagnostic n'exigent pas la présence d'une source provider. Aucun claim réarmé.
- Init retourne en interne une Directory retenue ; Up la garde avant doctor jusqu'à Compose. Down ouvre sa configuration avant le moteur. Compose revalide le même handle avant puis après le contrôle de cible moteur, sans recanonicaliser une nouvelle racine.
- Le broker garde une Directory commune pour checkpoints/archives/locks et clone son FD vérifié. Les lectures conservent la borne32MiB et le format existants ; l'ouverture export d'un checkpoint est désormais non bloquante et vérifie son FD (type, propriétaire, mode et lien unique).
- Les sources runner/viewer/jev sont relues/validées par PrivateFile après la migration de mode déjà documentée, comparées aux valeurs préparées puis retenues avant publication. Les projections0444 sous700 sont préservées. Le lecteur de démarrage conteneur `main::read_token` est distinct et n'a pas été modifié ; seul `runner::read_token` du broker adopte PrivateFile600.
- Les wrappers broker et projections emploient `Directory::try_clone_file`, sans ouverture de pathname ni mkdir/open redondants après la validation commune.
- `managed_scan` consomme l'API streaming `workspace::scan_workspace_archive` fournie par root. Le scan n'extrait plus vers un tempfile et ne consulte pas l'original. L'export public conserve ses gardes ; la parité des archives invalides et le gel du parser partagé appartiennent à root.

## RED observés avant correction

- Digest prenant le successeur du nom substitué : `/tmp/ai-guard-control-fs-digest-red.log`.
- Substitution env-file admise par create_args et init sous parent0777 non sticky : `/tmp/ai-guard-control-fs-handoff-red.log`.
- Snapshot sensible créé dans TMPDIR externe au contrôle : `/tmp/ai-guard-control-fs-tmpdir-red.log` (processus de test isolé).
- Handle direct contournant64KiB : `/tmp/ai-guard-control-fs-handle-bound-red.log`.
- Checkpoint FIFO bloquant avant validation : watchdog2s, `/tmp/ai-guard-control-fs-checkpoint-fifo-red.log`.
- Source physiquement dans projet acceptée par alias `PROJECT`/`project`, avant toute décision/réseau : `/tmp/ai-guard-control-fs-case-alias-red.log`.
- Dernier octet canonique LF dépassant64KiB après source exactement64KiB : `/tmp/ai-guard-control-fs-canonical-bound-red.log`.

Ces caractérisations à un UID ne constituent pas une exploitation autre UID ni une qualification de filesystem/ACL/CLI réel.

## Commandes et résultats

- `PASS` unités credentials5 : canonical bound, digest stable après substitution, correspondance masquage/bytes, TMPDIR hostile et alias de casse. `/tmp/ai-guard-control-fs-credentials-final.log` ; alias isolé `/tmp/ai-guard-control-fs-case-alias-green.log`.
- `PASS` premier lot intégrations : recovery13, authorization6, operations9 et handoff2. `/tmp/ai-guard-control-fs-caller-tests.log`.
- `PASS` handoff7 et operations9 après premiers correctifs de revue : `/tmp/ai-guard-control-fs-caller-review-fixes.log`. Handoff couvre notamment CLI Rust native réellement en attente, bytes exacts, durée de vie du handle, cleanup refusant le successeur, racine remplacée à info#3 et info#4, FIFO, cap64KiB et parent partagé.
- `PASS` runtime_program8 après intégration initiale Directory, `/tmp/ai-guard-control-fs-handoff-green.log`.
- `FAIL` utile nouveau test provider/original déplacé : a révélé une dépendance préexistante de `managed_scan` à `workspace::extract_workspace(original.canonicalize)`. `/tmp/ai-guard-control-fs-provider-resume.log`, puis `/tmp/ai-guard-control-fs-provider-after-digest-fix.log`. `PASS` après scan streaming : `/tmp/ai-guard-control-fs-provider-resume-green.log` ; credentials modifiés refusés sans nouveau helper/appel moteur, source restaurée reprise même worker/attempt et un seul create/start, quota libéré, snapshots nettoyés malgré l'original déplacé.
- `FAIL` intermédiaire clippy : helper root `scan_workspace_archive` encore non consommé (dead_code), `/tmp/ai-guard-control-fs-caller-clippy.log`. À rejouer après gel/insertion de l'API ; aucun ancien PASS ne le remplace.
- `PASS` clippy ciblé après intégration du scan : `cargo clippy --locked --lib --test credential_handoff --test runtime_operations --test runtime_authorization --test runner_recovery -- -D warnings`, `/tmp/ai-guard-control-fs-caller-final-clippy.log`.
- `PASS` intégration ACL source sur fixture locale Darwin : init refuse la source600+ACEread avant publication, garde sa valeur et son ACL, conserve l'inode de la projection ; `/tmp/ai-guard-control-fs-source-acl-green.log`. Ce test de cohérence init/lecteur ne constitue pas une qualification d'accès par un autre UID.
- `PASS` unités credentials5 finales : `/tmp/ai-guard-control-fs-credentials-final2.log`.
- `PASS` série finale des six cibles intégration : **46 tests, zéro échec, zéro ignoré** (credential_handoff7 + runner_recovery14 + runner_terminal_audit1 + runtime_authorization6 + runtime_operations10 + runtime_program8), `/tmp/ai-guard-control-fs-caller-final2-tests.log`. Ne pas compter l'ancien fichier `caller-final-tests.log` comme PASS (il conserve le défaut original déplacé avant correction).
- `PASS` rustfmt --check des sources et tests possédés après toutes corrections. La validation globale après gel du parser workspace appartient à root.

## Revue et limites

- Revue indépendante caller terminée : pas de bloqueur initial ; couverture info#4 et attente native ajoutées ensuite. Revue root a fait corriger cap64KiB des handles et doublons de FD. Revue supplémentaire a fait corriger cohérence ACL des sources init et alias physiques du projet ; cette dernière correction a une régression RED/GREEN réelle.
- Root possède exclusivement `workspace.rs` pour le scan streaming partagé avec l'export, sans écriture hôte ni dépendance au projet original ; intégration caller effectuée, gel et parité du parser sous responsabilité root.
- `NOT_RUN` par cet agent : vrais moteurs Docker/Podman, scénario multi-UID, qualification ACL entre principaux, Linux, conteneurs startup/projections réels, restauration SQLite, charge, cloud et release. Les fixtures réseau Bridge ne créent aucun réseau réel et ne qualifient pas Docker Bridge.
- `BLOCKED` pour une confirmation formelle d'audit : aucun sandbox OS complet correspondant au profil security-audit n'a été établi. Le contrôle du namespace vise autre UID sous modes/ACL/filesystem qualifiés ; aucune isolation promise contre même UID/root/administrateurs. Un nom `--env-file` demeure un handoff de pathname, pas une atomicité FD vers CLI.
- Après crash, des snapshots résiduels peuvent rester pour diagnostic. Aucun balayage récursif ou suppression par nom d'un successeur n'est introduit. Le nettoyage RAII concerne uniquement le fichier créé dont l'identité est vérifiée via FD.
- La reprise repose sur archive/manifeste immuables et digest de credentials déjà admis. L'absence de l'ancien projet n'est pas assimilée à une preuve « source hors projet » ; ce contrôle physique appartient au choix initial, qui reste obligatoire.
- Aucun gate production n'est fermé par ces fixtures. Les sources caller sont gelées ; revue finale des derniers correctifs et validation globale restent coordonnées par root, avec le gel du parser workspace avant qualification globale.

## Complément bloquant FS-4 — contrôle personnalisé dans le projet

Une régression de parcours a confirmé qu'un contrôle placé dans `project/control` pouvait être copié avec ses snapshots déjà préparés : `FAIL` attendu dans `/tmp/ai-guard-control-fs-custom-config-red.log`. La correction conserve cette configuration utilisable : `run` transmet la Directory retenue à `execute_plan`, qui fournit le clone de son FD vérifié à `prepare_workspace_excluding`. L'exclusion porte sur l'identité physique, avant descente/lecture de contenu, et couvre aussi l'alias de casse `CONTROL` observé sur le filesystem courant. Elle ne repose pas sur un nom réservé. Root ajoute en défense complémentaire l'exclusion globale `credential-snapshots` pour les résidus d'un ancien contrôle.

Un champ **privé optionnel** `workspace_control_exclusion_version` (`serde(default, skip None)`) vaut `Some(1)` uniquement après cette préparation et la sauvegarde de l'archive/digest/manifeste immuables. Une ancienne valeur absente ne devient pas une preuve implicite : sans start consommé, reprise et lancement refusent avant credentials/moteur/helper/claim et demandent une nouvelle admission. Archive, réservation et attempt restent conservés. Les démarrages déjà consommés gardent diagnostic, arrêt et réconciliation terminale sans second start. Aucun schéma SQL ou DTO public modifié, aucune archive historique réécrite.

`FAIL` migration avant correction : `/tmp/ai-guard-control-fs-legacy-prestart-red.log` montre qu'une reprise ancienne pouvait repartir sans cette preuve. Les premiers fichiers nommés `custom-config-green`, `legacy-prestart-green` et `legacy-consumed-green` conservent des **FAIL intermédiaires** d'un draft root de traversée parent (`open_beneath("..")`), ensuite corrigé par root avant son gel ; ils ne sont pas des preuves finales PASS.

`PASS` final FS-4 et migration : `cargo test --locked --test runner_recovery -- --nocapture`, **16/16**, zéro échec/ignoré, `/tmp/ai-guard-control-fs-fs4-migration-final.log`. Le test de contrôle personnalisé inspecte le manifeste ET le TAR effectivement envoyé au helper : seul `original.txt` est présent, aucun fichier control/snapshot/seed/token ; variantes normale et alias de casse exécutées. Les tests anciens None avant start et None après start vérifient les comportements de migration ci-dessus. Le bilan précédent recovery14 est historique.

La revue source finale `/tmp/ai-guard-control-fs-final-review.md` a relu ce delta et les preuves nouvelles : aucun bloqueur supplémentaire identifié. Les vérifications globales de root après les gels restent la preuve de l'ensemble, distincte de ces résultats ciblés.

`PASS` clippy final du delta : `cargo clippy --locked --lib --test runner_recovery -- -D warnings`, `/tmp/ai-guard-control-fs-fs4-final-clippy.log`. `PASS` rustfmt --check caller/managed/tests après ce gel. Aucun moteur supplémentaire exécuté.
