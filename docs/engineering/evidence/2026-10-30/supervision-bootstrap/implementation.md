# Bootstrap frais Store3 — rapport d'implémentation

Worktree exclusif : `/Users/louis/Documents/ai-guard/.worktrees/broker-service`, branche `codex/broker-service`, base `f46a23ecb8d17f098d325c017536e7c8a146ed83`. Aucun travail dans broker-supervision/main. Les artefacts précédents sont préservés.

## Changement et contrat

`Store::create_supervised_for_tenant(path: &Path, tenant_id: &str, redactor: Redactor, max_events: u64, backup_path: &Path) -> anyhow::Result<Store>` crée par StoreStorage::create_new/O_EXCL avant tout Connection::open. La création/initialisation réutilise le corps historique par open_prepared_for_tenant et conserve exactement le backing créé et le garde d'initialisation ; ce dernier est relâché après initialisation2, avant migration/backup, comme l'ancien open. La migration3 existante crée, vérifie, ferme et synchronise un vrai backup2 exclusif. Retour seulement après confirmation schema3 et active=true dans cette origine vivante. Réouverture reste maintenance-only. Aucun changement VFS ou audit/control.

Gateway : option explicite `--initialize-supervised-stores BACKUP_ROOT`, modes local et registre. Sans option, branche open_for_tenant historique inchangée et défaut2. Avec option, chaque destination doit être nouvelle ; backup privé `tenant-<SHA256 des bytes UTF8 exacts tenant_id>-v2.sqlite`. Ce hash nomme un fichier et ne constitue aucune identité authentifiée.

Le bootstrap conserve les références runner/viewer déjà résolues par gateway_config, sans champ JSON/API nouveau. Avant toute création SQLite, il normalise les références connues via control_fs, retient des Directory existants privés pour leurs parents, et un Directory privé de backup (aucune réparation de mode). Il compare dev/ino de ces répertoires contre les ancêtres des deux côtés : identité, ancêtre ou descendant sont refusés, y compris alias ordinaires de casse/Darwin. La référence Jev est incluse lorsqu'elle est configurée. Aucune clé n'est rouverte pour cette comparaison et aucun fichier SQLite n'est ouvert pour comparer les répertoires. Le mode ordinaire ne reçoit aucune restriction nouvelle de parent privé.

Une erreur arrête avant serve. Le bootstrap multi-tenant n'est pas globalement atomique : si un tenant ultérieur échoue, les nouvelles bases/backups antérieurs et les nouveaux répertoires éventuels restent conservés ; aucun fichier existant, base, backup, credential ou preuve n'est effacé. Aucun service OS/cloud, moteur ou appel modèle effectué.

## Sources possédées et gels

Seuls trois fichiers applicatifs sont édités par cet agent (autorisation root explicite pour le troisième). Tests appartiennent à Luna et documentation au root ; leurs modifications concomitantes sont préservées.

| Source | SHA256 avant | SHA256 gel proposé GREEN |
|---|---|---|
| src/audit.rs | 112b1044b2be813f47a3ab6adcfda94942fee05702bbb04d57313b99dfe877a3 | c092f53227107f568688d5b0dc0e2717aad39c54cd979fec16ea67d234a3fa4a |
| src/main.rs | 48d73be68241017ef27ce36fd52a4ede72920f5f61ea0c7310fce4cd196893c1 | 6466e3a55232be117d32aa392d14fa0c8f479daf4d46f04412bede671129ffaf |
| src/gateway_config.rs | e75156a82f6e0701acb85dfeda7a1da5b92cd5a94928049a726217d4700f8e9f | 0307bfb462f30f6e24288d67ec39e0523125338dc77ce13252957dc995ace89f |

`src/audit/control.rs` est inchangé : 2d80c7855dc3e4ab0622c4a41076607ad008b8a107a683dd48efaac60cc895c1.

## Vérification au gel

- PASS constaté par cet agent : `rustfmt --edition 2024 --config skip_children=true src/audit.rs src/main.rs src/gateway_config.rs`, exit0 ; seuls fichiers possédés formatés. Aucun cargo test/build/clippy exécuté par cet agent.
- RED library constaté par Luna avant application : `cargo test --locked --test supervision_bootstrap`, exit101, quatre E0599 pour méthode absente ; artefacts `/tmp/ai-guard-supervision-bootstrap-red-20261030/`. Les sources audit/main correspondaient aux SHA avant ci-dessus.
- PASS library intermédiaire rapporté par Luna : quatre tests du constructeur, dont le défaut déterministe après vraie copie du backup avec max_events=1. Ce n'est pas encore le résultat final du gel CLI.
- RED CLI avant application main/config : option absente ; l'oracle exige un diagnostic de chevauchement et absence de DB, pas seulement un exit non nul. Ce RED constate une fonctionnalité absente, pas une vulnérabilité existante.
- NOT_RUN par cet agent : tests finaux, clippy, build, runtime, installation et E2E ; exécution ciblée déléguée à Luna. Résultats finaux et revue encore attendus à cette version du rapport.

## Limites

Ce lot donne un accès explicite à l'origine Store3 fraîche pour le chantier broker ; il ne livre ni IPC/scheduler/TTY ni fencing D5, reprise d'une autorité restaurée, déploiement ou qualification runtime. Aucun gate production fermé. Aucun commit/push par cet agent.

## Mise à jour finale du gel — preuve Luna relue

Luna a exécuté `cargo test --locked --test supervision_bootstrap` sur les trois SHA applicatifs ci-dessus, inchangés avant/après : exit0, **6 PASS, 0 FAIL, 1 helper enfant ignoré par le runner**. Le helper de locks est invoqué explicitement par son test parent et n'est pas recompté comme fonctionnalité produit. Commande et sortie ont été relues par cet agent dans `/tmp/ai-guard-supervision-bootstrap-green-final6-20261030/{exit-code,cargo.stdout,cargo.stderr}` ; aucun test relancé ici.

SHA256 final test possédé Luna : `5f7d573934675073db4002f7f8e9f4174427f2e186ba99ae41ba0a0f75a32dc6`.

La cible vérifie : création fraîche active3 et backup2 intègre ; invalid tenant avant création ; défaut2 ; reopen maintenance ; cible/backup existants préservés et probes F_GETLK/journal_mode=DELETE ; erreur post-copie max_events=1 ; vrai gateway A/B/protocol authentifié avec namespaces distincts, restart sans option conservant namespaces/inactive, sibling positif ; chevauchements identique/ancêtre/descendant refusés avant DB avec diagnostic. L'alias Darwin est exercé seulement lorsque les préconditions /var et /private/var désignent effectivement le même dev/ino. La preuve est logicielle/SQLite/HTTP local, aucun runtime worker.

Les anciennes mentions « résultats finaux attendus » ci-dessus décrivent l'état intermédiaire avant ce gel. La revue indépendante et les régressions complémentaires orchestrées par root sont encore en cours ; leur verdict ne sera pas inventé dans ce rapport. Aucun changement applicatif depuis le gel GREEN.

## Delta de revue — chaîne de parents durable

La revue indépendante a relevé une barrière manquante pour les parents récursivement nouveaux : le fsync du parent immédiat du fichier ne synchronise pas tous les mkdir ancêtres. Root a demandé la correction exclusivement dans la factory neuve, sans changer StoreStorage ni open legacy.

Nouveau SHA audit.rs : `862d81fc2e74b032825c942aecbfeef73ed26e71f41d110c64ce3a639ec27bfb`. SHA main.rs/gateway_config.rs inchangés. Le gel final6 précédent reste une preuve historique du SHA c092f532 ; il ne qualifie pas ce delta.

Après StoreStorage::create_new, le garde d'initialisation source reste vivant. La factory ouvre un Directory existant de `backing.path().parent()` et appelle sa primitive existante `durable_bootstrap()` **avant** toute ouverture SQLite. Cette primitive retient/valide les descripteurs de la chaîne puis synchronise enfant/ancêtres jusqu'à la racine et revalide. Le helper garde ensuite exactement la durée de garde initiale pendant init2. Après son retour seulement, la factory ouvre/crée le Directory privé du parent absolu de backup et appelle durable_bootstrap avant migrate_control_v3. Les deux Directory sont retenus dans le scope de la factory, les contrôles StoreStorage continuent de vérifier leurs propres identités avant effet. Aucun fichier SQLite/clé n'est ouvert pour ces synchronisations de répertoires.

`rustfmt --edition 2024 --config skip_children=true src/audit.rs` : exit0. Luna reprend la cible sur ce gel et ajoute son probe F_GETLK immédiatement après refus, avant tout autre appel Store ; son premier oracle ne distinguait pas une réacquisition ultérieure du lock. Aucune panne électrique n'a été simulée ou qualifiée. Résultat du nouveau gel attendu.
