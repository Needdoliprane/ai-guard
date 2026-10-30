# Revue indépendante — frontière filesystem du contrôle

30 octobre 2026. **Revue source terminée : aucun bloqueur supplémentaire trouvé dans les sources figées ci-dessous.** Les findings FS-1 à FS-4 transmis ont une correction relue et des preuves de développement. FS-4 était une priorité P0 retenue par root. Ce résultat ne qualifie ni release, ni moteurs réels, ni isolation inter-UID.

Périmètre : `src/control_fs.rs`, tests, raccords de runner/managed/operations/ContainerSpec, deux appels filesystem dans invocation, delta root de workspace partagé scan/export et exclusion du contrôle. AGENTS actuel, RTK, contrats architecture/code/gates/tests, design et rapports core/caller lus. Aucun test, cible, probe, moteur, remount ou changement de droits exécuté par ce reviewer ; les logs sont lus, pas des exploitations personnelles. Aucun source ou document du dépôt modifié. Le gel invocation261 historique ne qualifie pas ce lot.

## Findings transmis et traitement

### FS-1 — P1 : volume Darwin ignorant les propriétaires — traité

Préconditions : configuration, credential ou programme sur APFS/HFS local déjà monté avec `MNT_IGNORE_OWNERSHIP`. Le premier validateur ne vérifiait que type local APFS/HFS ; UID/modes ne suffisaient donc pas à soutenir sa garantie autre UID. Aucun remount par l'attaquant n'était requis.

Apple décrit ces volumes comme traitant les fichiers comme appartenant à l'utilisateur courant indépendamment de leur propriétaire réel. Le SDK `sys/mount.h` et XNU définissent `MNT_IGNORE_OWNERSHIP=0x00200000`, alias `MNT_UNKNOWNPERMISSIONS`. Sources primaires : [Apple Support](https://support.apple.com/en-om/guide/mac-help/mchlp1204/mac), [XNU mount.h](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/mount.h).

Correction relue : `control_fs.rs:600/614` refuse ce flag sur les FD contrôlés ; `invocation.rs:294/366` applique la même politique aux FD répertoire/programme. Aucun mount ou ACL modifié. Tests purs APFS/HFS avec flags RED puis GREEN dans le rapport core. Qualification effective d'un mount noowners et accès par deux UID : **NOT_RUN**.

### FS-2 — P1 : source credential dans le projet par alias de casse — traité

Préconditions : APFS/HFS insensible à la casse, source privée dans le projet, orthographe différente dans `--secret-env-file`. `absolute_path` lexical puis `starts_with(project)` pouvait l'accepter. Source primaire : [Apple APFS Guide](https://developer.apple.com/library/archive/documentation/FileManagement/Conceptual/APFS_Guide/FAQ/FAQ.html).

RED lu : `/tmp/ai-guard-control-fs-case-alias-red.log`, acceptation avant décision/réseau. Correction : `PrivateFile::is_within_directory`, `control_fs.rs:282`, compare dev/ino du projet ouvert sans suivre les symlinks aux identités de parents de la source retenue. Erreur de lookup conservée. `runner.rs:458/463` impose ce contrôle initial et garde le même FD process-local (`serde(skip)`) jusqu'aux bytes préparés. GREEN : `/tmp/ai-guard-control-fs-case-alias-green.log` et unités credentials finales.

La reprise revalide source/digest admis sans exiger que l'ancien projet existe encore. Ce n'est pas une nouvelle preuve « source hors projet » à partir d'un chemin absent ; le contrôle physique est obligatoire au choix initial.

### FS-3 — P2 : init annonçait une source inutilisable par le broker — traité

Le raccord initial contrôlait les répertoires puis utilisait l'ancien lecteur de projections ne refusant pas les ACL Darwin des sources. Init pouvait publier une source ensuite refusée par le lecteur privé du broker. Le parent secrets privé protégeait déjà sa traversée : aucune divulgation inter-UID prouvée par ce point.

Correction : `operations.rs:547/576` ouvre et retient les sources via `sources_guard.read_private_file`, compare les bytes préparés puis revalide tous les handles avant retour. Aucune ACL refusée n'est retirée. Projections 0444 et lecteur startup de main inchangés. Test Darwin PASS 1/1 : `/tmp/ai-guard-control-fs-source-acl-green.log`, valeur/ACL source et inode projection préservés.

### FS-4 — P0 selon root : contrôle/snapshot importé via config personnalisée — traité

Préconditions ordinaires : `--config-dir=project/control`, feuille 0700, source provider 0600 hors projet, observe+Bridge. Le nouveau snapshot était préparé dans control/credential-snapshots avant import. Le filtre nommé n'excluait ni cet artefact ni toute racine personnalisée. Bytes provider et états privés pouvaient entrer dans l'archive/volume et les helpers, qui ne doivent pas recevoir ces clés. Chemin source signalé sans exécution personnelle ; RED lu dans `/tmp/ai-guard-control-fs-custom-config-red.log`.

Correction relue :

- `workspace.rs:136`, API interne `prepare_workspace_excluding(..., &[&File])` : chaque FD doit être un répertoire ; dev/ino comparés aux entrées ouvertes sous le FD projet avant descente. Projet identique ou situé sous un contrôle exclu : refus explicite. Le wrapper public conserve son comportement sans exclusions supplémentaires.
- `runner/managed.rs:1239` transmet le clone du FD de la configuration déjà validée, retenu pendant import. Aucune nouvelle sélection de racine par pathname.
- `workspace.rs:63`, nom réservé credential-snapshots : protège aussi l'artefact résiduel d'un ancien config qui n'est plus courant. Ce filtre complète l'exclusion physique de toute la racine courante ; il ne la remplace pas.
- Checkpoint `workspace_control_exclusion_version=Some(1)`, `runner/managed.rs:35/1304`. Une ancienne archive sans preuve ne déclenche pas un premier start : garde avant bound_runtime dans resume et avant soumission dans launch (`1378/1396/2218`). Archive, worker/attempt, réservation et claims conservés. Allocation au start déjà consommé toujours observable/arrêtable, sans rearm.

Preuve finale : `/tmp/ai-guard-control-fs-fs4-migration-final.log`, **16 PASS, 0 FAIL, 0 ignoré**. Custom config control et alias CONTROL passent sur cet hôte : manifeste/TAR contiennent original.txt seul. Legacy pré-start refuse sans moteur/nouveau claim/altération archive ; legacy consommé poursuit observation/stop sans second start. Les anciens fichiers custom-config-green et legacy-*-green conservent les FAIL intermédiaires du draft : ils ne sont pas comptés PASS.

Unités root finales : `/tmp/ai-guard-control-fs-workspace-final-tests.log`, **8 PASS**, dont exclusion manifeste/archive et voisins métier, refus config==projet/FD non-répertoire, identité retenue après rename, racine externe sans effet, snapshots orphelins malgré une autre racine courante, parité scan/export. Ce test de copie ne qualifie aucun moteur.

## Contrôles positifs et compatibilité relus

- Directory ouvre depuis / par openat NOFOLLOW à chaque composant, UID root/euid et absence d'écriture groupe/autres ; exception ancêtre root/sticky avec validation de l'enfant. Feuille euid 0700. Métadonnées sur FD ; revalidate vérifie identité/politique de chaque parent ; try_clone_file clone le FD retenu.
- Darwin : ACL étendue refusée sur feuille privée ; ancêtres deny/allow de lecture-recherche admis, mutation/bits inconnus refusés. Linux : politique bornée aux filesystems admis et modes POSIX ACL ; aucune qualification effective Linux déduite.
- PrivateFile : euid 0600, régulier, lien unique, NONBLOCK, état avant/après et plafond 32 MiB complété par les limites caller. Handle env direct 64 KiB ; ajout canonique LF également borné avant snapshot/admission.
- Bytes/SHA de snapshot ne sont pas relus par nom pour le digest. Création exclusive sous config privée, sans TMPDIR ; Arc garde le snapshot pendant CLI create. Drop nettoie seulement l'identité créée et préserve le successeur observé.
- ContainerSpec retient et revalide le PrivateFile avant --env-file avec un chemin ordinaire. Garantie autre UID par namespace/modes/ACL, pas par prétendue consommation du FD par Docker/Podman.
- Up conserve la même configuration avant doctor et revalide après les appels moteur avant Compose. Down/reprises valident aussi leur racine. Sources de secrets et projections 0444 séparées ; main inchangé.
- managed_scan utilise le scan readonly borné (`workspace.rs:395`) sans extraction host ni lookup de l'original. Lecteur partagé scan/export : types/liens/traversées/limites/troncatures et conflits file/dir dans les deux ordres refusés. Carte des chemins sans ajout de tous parents implicites. Export public conserve ses gardes de destination neuve et projet original.

## Preuves et limites

**PASS source** : FS-1/2/3/4 traités, aucun autre bloqueur concret trouvé dans ce périmètre.

**PASS dans les logs des implémenteurs** : core 15 intégrations APFS et 4 unités FD/noowners/containment ; credentials finales 5 unités ; caller-final2 46 intégrations (7 handoff +14 recovery +1 terminal +6 authorization +10 operations +8 program) ; FS-4 final remplace recovery par 16 PASS. Workspace final 8 unités ; anciennes régressions runtime security/races 15 PASS rapportées par root. Ne pas sommer les réexécutions comme scénarios distincts.

Logs : rapports core/caller, credentials-final2.log, caller-final2-tests.log, workspace-final-tests.log, workspace-regressions.log et fs4-migration-final.log, tous préfixés `/tmp/ai-guard-control-fs-`. Clippy strict ciblé après FS-4 PASS lu dans fs4-final-clippy.log ; root confirme fmt/clippy all-targets globaux PASS et lance la fullsuite séparément. Aucun résultat de fullsuite encore revendiqué ici.

**FAIL historiques traités** : provider/original déplacé avant scan readonly, scan-parity avant garde hiérarchies, custom-config/legacy avant FS-4 final, lint/fmt intermédiaires root. Les traces conservées restent FAIL ; leur nom ne détermine pas leur statut.

**NOT_RUN par ce reviewer** : toute exécution cible ; Linux, HFS/noowners réel, ACL entre deux UID, Docker/Podman réels, startup/projections en conteneurs réels, remount, charge, cloud et release. Aucun gate production fermé par cette revue.

**Hors garantie / limites** : même UID, root, administrateurs stockage/namespace/moteur ; filesystems distants/FUSE non admis ; pas de borne en temps mural pour read bloqué par stockage. Cleanup best effort et snapshots résiduels après crash à diagnostiquer/rétenir, sans balayage destructif ajouté. Exclusion contrôle courant + répertoire réservé des snapshots ne constitue pas une DLP générale de toutes clés possibles du projet. SQLite/VFS/WAL et fencing de durée restent ouverts. Politique noowners corrigée ne signifie pas exploitation inter-UID démontrée.

## Empreintes

SHA256 après annonce gel caller/root workspace, avant QA globale :

| Fichier | SHA256 |
|---|---|
| src/control_fs.rs | ec747ca5352b48d805888942481ad8263b1efe5c322d816caf03d445fef6facb |
| tests/control_fs.rs | 8c845cfa0ff10c2e104455e7af152c53eaed5abedc71eb18e23ffeb342225465 |
| src/runner.rs | 74f962dbca664536530a1c827e2375489a81c8c83e9d668f60578f4f6aa44716 |
| src/runner/managed.rs | b89995d768aa76edcd0b977f086539fd5dda2b2e8f257d2b7e3add2a4c6c4aa3 |
| src/operations.rs | f05e3fa0e229dee8bb2ddf2953da60bcbc14a6c1001f0099167a979ccf991e9b |
| src/runtime.rs | 75368bc71d37ba537f30b37abaf644b3d558d3a0dd28f0ec6a0aa9f75415240f |
| src/runtime/invocation.rs | 1f775a5801edd99990882a8949bf59c4d60e75ed9f9e2be2b12b69fdc0d896a8 |
| src/workspace.rs | b6c3dc715563646dadcf6e964f4db72f9162ceb0529e94c2005f2d1a6f2717e8 |
| tests/runner_recovery.rs | 8b7339456f1e74430de3d5081376ab5cb5799567321bae6e6f0a5cbd9d790fc1 |
| src/main.rs, identique gel261 | dfc772e81dd26a19de7c842f834c38010894068ae10373fc21bc1282212f2822 |

**Conclusion : FS-4 traité dans le gel relu ; aucun bloqueur source supplémentaire identifié.** Qualification globale et réelle sous responsabilité root, distincte de cette revue.
