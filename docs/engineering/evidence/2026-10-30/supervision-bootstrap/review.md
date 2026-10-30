# Revue ciblée — bootstrap explicite Store3 frais

Worktree `/Users/louis/Documents/ai-guard/.worktrees/broker-service`, base vérifiée `f46a23ecb8d17f098d325c017536e7c8a146ed83`. Revue en lecture seule, hors audit global/Daybreak. Aucun test/cargo/moteur/modèle exécuté par le reviewer et aucun fichier du dépôt modifié. Cette note externe est privée0600.

Politiques lues : AGENTS/RTK, ARCHITECTURE_CONTRACT, CODING_RULES, PRODUCTION_READINESS, TEST_STRATEGY, contrat actif IMPLEMENTATION_PLAN et contrats tenancy/recette/usage. Skills projet .agents/.claude absents dans ce checkout ; aucune invocation automatique projet prétendue.

## Gel initial vérifié

```
c092f53227107f568688d5b0dc0e2717aad39c54cd979fec16ea67d234a3fa4a  src/audit.rs
6466e3a55232be117d32aa392d14fa0c8f479daf4d46f04412bede671129ffaf  src/main.rs
0307bfb462f30f6e24288d67ec39e0523125338dc77ce13252957dc995ace89f  src/gateway_config.rs
```

Handoff lu `/tmp/ai-guard-supervision-bootstrap-implementation.md`. StoreStorage et migration3 existants relus ; leur code n'est pas modifié dans ce premier gel.

## Propriétés correctement construites au gel initial

- Factory : validate tenant/capacité, puis StoreStorage::create_new avant Connection::open. `StoreStorage::prepare(new_only=true)` vérifie noms/type/modes/ACL/nlink et sidecars par metadata-only, refuse les entrées préexistantes avant même le lock d'initialisation, répète sous lock puis O_EXCL et sync du fichier neuf. Le chemin d'un Store existant n'est donc pas ouvert puis fermé par cette nouvelle factory.
- Le helper extrait `open_prepared_for_tenant` conserve l'InitializationGuard nommé pendant toute l'initialisation v2 et sa transaction. Le retour libère ce garde avant migrate_control_v3 ; le backup peut alors prendre son garde même dans le même parent. L'ancien open ordinaire conserve la même durée de garde et le défaut2.
- Migration existante : backup destination exclusive, vraie copie SQLite, validation schema/tenant/head/nombre d'événements/quick_check, fermeture puis sync_closed sous garde ; recoupe de la tête source avant DDL et commit validé. Une erreur conserve les cibles/partiels ; aucune suppression ni réparation de mode ajoutée.
- L'activation reste process-local : fresh_origin exige backing.created_here && empty && initial_valid ; untouched_fresh_origin exige tête/plancher initiaux inchangés et zéro worker. Une réouverture a created_here=false et reste maintenance. La factory exige version3+active au retour. Un checkpoint/backup/restauration ne recrée pas cette autorité.
- Gateway sans flag garde open_for_tenant. Avec flag, le bootstrap est avant création du listener/serve ; toute erreur empêche de servir. Plusieurs tenants ne sont pas atomiques : des nouvelles bases/backups antérieurs peuvent rester après échec ultérieur, comme le contrat l'annonce.
- ConfiguredTenant conserve seulement des références de chemins déjà résolus/lus. Aucun JSON nouveau ni lecture supplémentaire des clés pour comparer les répertoires. La comparaison backup/credentials est bidirectionnelle par dev/ino d'ancêtres validés et couvre identité/ancêtre/descendant et alias physiques ordinaires ; Jev est inclus lorsqu'il est configuré. Aucune identité tenant n'est déduite du nom de backup SHA.

## Constats transmis au parent

### BSB-01 — barrières des nouveaux ancêtres manquantes (source)

Condition concrète : cible DB ou backup sous une chaîne de répertoires initialement absente. `StoreStorage::prepare` et `prepare_supervised_backups` appellent Directory::open_or_create ; cette primitive mkdir ne synchronise pas le parent de chaque nouveau répertoire. create_new/sync_closed synchronisent fichier et parent immédiat seulement. Aucun durable_bootstrap n'est appelé dans les trois sources de ce gel.

Effet : la factory peut retourner un Store actif, puis le gateway servir, sans avoir synchronisé les entrées des nouveaux ancêtres ; ces entrées peuvent être perdues après panne malgré les fsync du fichier et du parent immédiat. Ce constat est une absence de barrière de durabilité dans le nouveau parcours, pas une panne électrique reproduite ni une prétention de résistance à root. Les cas dont tous les parents sont déjà durables ne déclenchent pas cette condition.

Correction à trancher par root/core : appliquer la primitive durable_bootstrap existante aux chaînes cible/backup avant le retour utilisable/serve, ou imposer une précondition explicite de parents privés durables préexistants. Ne pas refondre le VFS ou modifier la durée du garde ordinaire pour cela. Parent et core avertis.

### BSB-02 — oracle de conservation des locks incomplet (test en cours)

`bootstrap_refuses_preexisting_target_and_backup_without_closing_existing_store` appelle son probe child F_GETLK avant le refus (`tests/supervision_bootstrap.rs:266`) mais pas après dans la version lue. Les appels Store supervision_state/head/integrity/append arrivent ensuite (`278-284`) et peuvent réacquérir des locks accidentellement perdus. Le PASS de cette version ne prouve donc pas « refus sans libérer les POSIX locks ».

Correction : probe child immédiatement après le résultat Err de la factory, avant tout nouvel appel SQLite/Store du PID propriétaire ; les contrôles de tête/intégrité/append viennent ensuite. Ne pas ouvrir/fermer soi-même la base dans le parent pour comparer les bytes. Parent averti ; code produit create_new paraît déjà conserver la bonne propriété par inspection.

### BSB-03 — gateway child brut non possédé sur panic (test en cours)

`start_bootstrap_test_gateway` retourne std::process::Child brut. Les assertions et requêtes HTTP suivant une readiness réussie précèdent kill/wait explicites. Si elles échouent/paniquent, le Drop de Child ne tue pas le gateway ; le processus peut survivre à la fixture TempDir. Les branches d'erreur readiness possèdent déjà un kill/wait explicite.

Correction : garde RAII possédant le Child, kill+wait au Drop, y compris pour le restart, comme les autres harnais du dépôt. Aucun effet produit n'est établi par ce constat ; il concerne la fiabilité/propriété des processus de preuve. Parent averti.

## Statut intermédiaire

### Correction BSB-01 relue

Nouveau gel audit vérifié `862d81fc2e74b032825c942aecbfeef73ed26e71f41d110c64ce3a639ec27bfb` ; main/config inchangés aux SHA du premier gel.

La factory retient désormais le Directory du parent cible et appelle durable_bootstrap après create_new, sous le garde d'initialisation vivant, **avant** le premier Connection::open (`audit.rs:419-425`). Cette routine existante retient et vérifie tous les ancêtres puis les synchronise du descendant vers `/`, sans ouvrir de fichier SQLite. Le helper conserve ensuite la durée de garde normale. Après son retour seulement, la factory normalise le chemin backup, ouvre/crée son parent privé, le retient et appelle durable_bootstrap avant migrate_control_v3 (`435-442`). La migration existante garde la responsabilité de fsync du fichier backup et de son parent immédiat.

BSB-01 est donc **CLOSED par inspection du correctif source** ; nouveaux ancêtres cible et backup ont leurs barrières avant retour/serve. Pas de refonte StoreStorage, fermeture de FD SQLite, modification du default2 ou garde maintenu pendant le backup. Les échecs de synchronisation restent refusés avec partiels conservés, sans nouvelle autorité exposée au caller. Aucune panne électrique n'est présentée comme testée.

Revue source : constructeur/exclusivité/backup/fresh-origin/default2, ancêtres durables et séparation de chemins cohérents ; aucun défaut bloquant source résiduel identifié à ce passage.
Revue des oracles au premier passage : test transitoire `f281cd56cb6fb0d423dd5bdfa20aac2038a60878d2ab2aa39d72219110a230d8` relu. BSB-03 est traité dans la source du harnais par OwnedBootstrapChild : kill et attente bornée au Drop, garde conservé pendant les assertions/await, même protection pour restart et commandes négatives. Le timeout d'un OS ne répondant pas à SIGKILL n'est pas transformé en garantie de terminaison absolue. BSB-02 restait ouvert dans cette version : le probe CLI après refus a lieu dans un autre PID et ne remplace pas la sonde immédiatement après le refus factory dans le PID du Store. L'absence de sonde post-factory avant premier appel Store avait été rappelée à Luna avec les lignes exactes.

## Gel final source/oracles et résultats relus

Produit inchangé au gel correctif : audit `862d81fc2e74b032825c942aecbfeef73ed26e71f41d110c64ce3a639ec27bfb`, main `6466e3a55232be117d32aa392d14fa0c8f479daf4d46f04412bede671129ffaf`, config `0307bfb462f30f6e24288d67ec39e0523125338dc77ce13252957dc995ace89f`. Test bootstrap final `2ebddf5510e16954f043764ed071d98e236ad01bcb648777dd0acb6372097959` et rapport Luna `/tmp/ai-guard-supervision-bootstrap-tests.md` `da6fcbd1bb44e78da95e0392372b7b7b99cf4f4a972b8c4193a3230afca78112` vérifiés directement.

**BSB-02 CLOSED :** la sonde indépendante est maintenant la première action après le Err de la factory (`supervision_bootstrap.rs:316`) ; supervision_state/head/integrity/append viennent seulement après (`318+`). Le PID du Store ne peut donc pas masquer une fermeture de FD accidentelle en réacquérant ses locks avant la sonde. Le child ouvre/ferme la DB dans son propre PID, F_GETLK contrôle les locks de l'autre processus puis PRAGMA DELETE doit échouer. Helper exact ignoré invoqué explicitement et non recompté comme fonction produit. Le résultat final actuel, pas les anciens gels, porte cette propriété.

**BSB-03 CLOSED :** OwnedBootstrapChild final relu, readiness bornée et garde conservé pour A/B, restart, mode local et commandes négatives. Arrêt/attente restent bornés ; aucune garantie contre un OS qui ignore/tarde à traiter SIGKILL n'est inventée.

Le nouveau cas de parents imbriqués absents crée séparément la chaîne source et backup, vérifie eUID/0700, Store3 actif puis backup2 intègre. Le positif backup et source dans le même parent reste présent et piège une conservation trop longue du garde pendant la migration. Cette exécution vérifie les chemins et appels réels ; l'absence de panne électrique qualifiée reste explicite.

Logs lus : `/tmp/ai-guard-supervision-bootstrap-lock-probe-postfix-20261030/{cargo.stdout,cargo.stderr,exit-code}` : **7PASS,0FAIL,1helper ignored**, exit0,1,14s de tests ; le rapport annonce4s de commande et ne confond pas les deux durées. Manifestes du run portent bien le gel app final et test final.

### Régression de fixture clone sous umask022

Delta `tests/supervision_store.rs` relu : nouvelle destination clone créée exclusivement0600 avant son ouverture SQLite. Sa FD brute est fermée **avant** toute connexion VFS à cet inode neuf. Aucune permission applicative, assertion de fencing ou code Store n'est relâché ; l'ancien défaut venait de la préparation du fichier de test avec le mode par défaut du processus.

Preuve historique lue : HEAD `f46a23ecb8d17f098d325c017536e7c8a146ed83`, test exact baseline en FAIL sur l'exigence mode0600, puis le même exact corrigé PASS1 et cible complète supervision_store PASS9. Le rapport garde la distinction entre ce FAIL de fixture et un défaut produit. Les17storage/12integrity/1gateway verts annoncés ne sont pas recomptés comme nouveaux tests uniques ni systématiquement relancés après ce seul correctif de fixture.

Release finale : logs exit0 relus,0,18s correspond à la commande cache finale, pas une prétendue nouvelle compilation complète. Manifeste112 avant/après identique, SHA recomputé `10d581fd0e445e9d3647e17b59b098245ea401d1d1b32619eb7f9cbbf4e44a68`. Hashes d'artefacts annoncés `3188c3a6848925a2995d7096fe2f5433d4a609e1eb7daa2c009e70dea5a19b39` / `10d191a1c4e457b57bc358ae9698241ec69b0701036c5da46ee60f87e09376d4` ; les copies conservées finales restent à recouper après fourniture par root.

Verdict source/oracles : **aucun constat bloquant résiduel identifié ; BSB-01/02/03 traités**. Clôture finale documents/preuves encore attendue. Aucun test, compilation ou scan supplémentaire exécuté par cette revue.
NOT_RUN par ce reviewer : tout test, crash/power-loss, moteur/modèle/service externe. La clôture documents/preuves était réservée au gel final à ce stade ; le verdict ci-dessous remplace cette réserve.

## Clôture finale — documents, Docker et preuves

Relecture finale du 30 octobre 2026 en lecture seule. Les trois sources applicatives et les deux tests sont toujours aux SHA du gel final ci-dessus. Le manifeste `source.sha256` a été vérifié directement contre les 112 fichiers courants : 112 OK, retour 0. Son SHA `10d581fd0e445e9d3647e17b59b098245ea401d1d1b32619eb7f9cbbf4e44a68` est identique aux manifestes avant/après release. Le rapport Luna archivé `tests.md` est byte-for-byte au SHA `da6fcbd1bb44e78da95e0392372b7b7b99cf4f4a972b8c4193a3230afca78112` annoncé. Aucun test ou build relancé par le reviewer.

### Dockerfile et configuration effectivement exercée

Le seul delta de `images/control.Dockerfile` ajoute `/run/secrets` aux répertoires créés, possédés par 1000:1000 et privés 0700 pendant la construction. Le SHA vérifié est `11b5d0ac5c444b702f77782454a58607a2a0724ec79d3645018c84a6626b74b3`. USER, copie du binaire, projections de fichiers et défaut applicatif ne sont pas modifiés par ce delta. Il prépare le parent dans l'image ; il ne répare pas les permissions d'un répertoire hôte monté par-dessus.

Les Dockerfiles comparatifs archivés ont été relus : même ancienne base runtime locale et même nouveau binaire ; la variante préparée ajoute mkdir/chown/chmod du parent secrets. Les deux variantes avec probe copient le même probe. Il s'agit d'overlays hors ligne, pas de la construction multi-stage complète du Dockerfile du dépôt. Cette limitation apparaît dans README, rapport Docker, suivi et validation.

L'inspect JSON archivé confirme, pour les deux contrôleurs : utilisateur 1000:1000, rootfs readonly, réseau none, CapDrop ALL, no-new-privileges, 2 CPU, 512 MiB, 256 PID, aucun port publié. `/data` est l'unique montage writable ; registre et quatre projections de clés sont readonly. Les deux états sont `exited`, Running=false : témoin exit 1, préparé exit 0. Ces limites configurées ne constituent pas une mesure de consommation ou de charge.

Le témoin conserve le refus explicite du parent de credentials non privé ; sa copie DATA finale `witness-data-copy-2` ne contient que les répertoires a, b et backups, sans fichier. Le préparé conserve un healthcheck RC0, le probe RC0 et les observations de métadonnées `/run/secrets` 1000:1000/0700, projections 0444, deux chemins DB distincts 0600 et deux backups privés non vides. Ces observations sont cohérentes avec le besoin de préparation d'image, sans prétendre qualifier un worker.

### Portée de la sonde et correction de preuve

Le code Rust archivé `docker/bootstrap_qualification_probe.rs` a été relu. Il construit le vrai AuditClient depuis les chemins de clés synthétiques, puis contrôle version 1, tenant attendu, rôle runner, schéma 3 et active=true dans chaque réponse authentifiée. Il ne journalise pas les tokens. L'oracle des namespaces exige une chaîne de longueur 36, différente du sentinelle nil et de celle déjà vue ; il ne parse pas la syntaxe UUID complète. Les valeurs effectivement imprimées ont la forme attendue, sans attribuer une assertion plus forte au probe.

Pour les backups, les lignes 41-63 ne contrôlent que compte, préfixe/suffixe du nom et métadonnées de fichier : régulier, UID 1000, mode 0600, nlink 1, longueur non nulle. Elles ne lisent pas SQLite, ne vérifient ni schéma ni quick_check et ne recomputent pas la relation nom-SHA/tenant. La formulation initiale « backups schema2 observés » dépassait cet oracle Docker. Elle est corrigée dans le rapport Docker final SHA `0407e332e6d08d698ee746e3c9ed78d5173e4fb49ed5a8f8d1f1e499ed259874`, l'index SHA `1e67a4de4a898b02559c005a1991de830fcf26fa1d55bc779df100c611ec5018` et le README : deux backups privés sont observés ici ; leur format pré-migration 2 relève de la construction produit et de la vérification native distincte. **Écart documentaire CLOSED**, sans nouveau test requis ni défaut produit déduit.

### Artefacts et cohérence du suivi

Hashes recalculés directement sur les copies privées conservées :

| Artefact | SHA-256 |
|---|---|
| macOS `agentsandbox-macos` | `3188c3a6848925a2995d7096fe2f5433d4a609e1eb7daa2c009e70dea5a19b39` |
| macOS `libagentsandbox-macos.rlib` | `10d191a1c4e457b57bc358ae9698241ec69b0701036c5da46ee60f87e09376d4` |
| Archive source | `79407466e2f2417389e7d32faad67207349959024f82080d61c07990a8339275` |
| Linux `linux-artifacts/agentsandbox` | `764b91d0297669c0c07db094dd46fb4874f5145bb8966f330a62138b9cc94e9e` |
| Linux `linux-artifacts/libagentsandbox.rlib` | `dd446b353c38a3d2bd374edc5854bb94ecd2fc8877ee26336c520722505485fc` |
| Linux `probe-output/bootstrap-probe` | `9db646573aa00457b4f1fb7b6c775969d3a3057ae73d1ec1b0587b9daed8c80e` |

Les copies `runtime-binaries/witness-linux` et `corrected-linux` sont au même SHA Linux `764b91d0…94e9e` ; `runtime-binaries/probe-linux` est au SHA du probe `9db64657…8c80e`. Les fichiers natifs préexistants ne sont pas présentés comme des résultats Linux. Le répertoire privé est 0700. Les incidents du builder éphémère/volume non extrait et du linkage ThinLTO sont décrits comme incidents de harnais ; ils ne sont pas convertis en preuves produit.

CURRENT_STATE, DECISIONS, IMPLEMENTATION_PLAN, BROKER_SUPERVISION, TEST_STRATEGY, VALIDATION, operations, quickstart et l'index de preuves ont été recoupés au périmètre modifié. Les comptes sont cohérents : 7 fonctions bootstrap ; 39 canaris distincts = 17 storage + 12 integrity + 1 gateway + 9 supervision_store, sans addition des rejeux exacts/enfants. Le FAIL initial de clone et sa reproduction baseline restent visibles, puis le 9/9 corrigé est archivé ; aucun ancien PASS n'est attribué au probe post-refus ajouté ultérieurement. Les résultats natifs et Docker Desktop Linux/ARM64 sont séparés.

Les documents exposent le défaut 2 inchangé, la maintenance après reopen, le refus de réutiliser le flag, la conservation des partiels et l'absence d'atomicité globale multi-tenant. `active=true` décrit l'autorité fraîche du Store, sans ordonnanceur ou enforcer implicite. La recette utilisable avec bases neuves et clés privées est documentée ; aucune réparation ou suppression automatique de données existantes n'est suggérée.

### Verdict final

**CLOSE au périmètre de ce lot : aucun constat bloquant résiduel identifié après correction de BSB-01, BSB-02, BSB-03 et de la portée documentaire de l'oracle Docker.** Source, oracles, documents et preuves conservées sont cohérents pour la factory neuve et le contrôle gateway ciblé. Cette conclusion est une revue indépendante de ce delta, pas une approbation automatique d'un déploiement ou une qualification globale du produit.

Restent **NOT_RUN** dans ce lot : panne électrique, build multi-stage complet, Linux natif/x86_64, Podman, workers/confinement runtime, modèle/fournisseur, navigateur/E2E général, charge 100, cloud/Kubernetes, service/IPC/ordonnanceur/gardien et reprise/fencing. G09 demeure FAIL, 0/12 gates entièrement clôturés, QA et facturation inchangées. La limite intentionnelle de restart et les nouveaux partiels possibles après erreur multi-tenant restent des contraintes opérationnelles explicites, pas des propriétés qualifiées par la preuve Docker.

Le parent doit encore recopier cette note finale et régénérer le manifeste SHA256SUMS des preuves avant intégration ; le manifeste antérieur ne doit pas être présenté comme couvrant cette nouvelle copie. Aucun fichier du dépôt, archive ou processus runtime n'a été modifié par le reviewer.
