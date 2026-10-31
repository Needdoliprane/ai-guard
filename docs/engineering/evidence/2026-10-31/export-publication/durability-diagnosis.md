# Export durable — diagnostic et couture minimale

31 octobre 2026, base propre `858f50e0ce21a807d9ae72e7f5af80c4684cd5b8`. Lecture seule : aucune source, commande Cargo/test, runtime, service ou credential utilisé. Gouvernance AGENTS/ARCHITECTURE/CODING/TEST_STRATEGY relue ; skills projet absents. Méthode systematic-debugging : hypothèse liée au chemin de données et aux opérations réellement présentes ; reproduction dynamique encore NOT_RUN, RED proposé ci-dessous. Root conserve workspace.rs et le contrat jusqu'au GO.

## Mécanisme confirmé par les sources

`read_workspace_archive`, workspace.rs:512–516, termine un fichier par `flush()` puis `set_permissions()` ; aucun `sync_all` après les données et métadonnées. `create_directory_beneath`, :579–595, crée/ouvre les répertoires enfants sans barrière de durabilité. `Staging::publish`, :699–737, vérifie parent/staging, effectue renameat2(RENAME_NOREPLACE) ou renameatx_np(RENAME_EXCL), met `published=true` et retourne sans synchroniser le parent. Le chemin autorisé `AuthorizedExportParent::extract`, authorized.rs:263–284, et le legacy `extract_workspace`, workspace.rs:378–411, emploient cette même couture ; le scanner seul appelle le parseur avec staging=None.

Conséquence démontrable par lecture : un retour Ok d'extraction n'est pas conditionné aux barrières fichiers/métadonnées/répertoires/parent. Il n'est pas une preuve de durabilité de la destination. Ce diagnostic ne revendique pas une perte de données après panne électrique reproduite.

**Piège à ne pas introduire :** Drop de Staging (:784–807) appelle `clear_directory(&self.directory)` tant que published est faux. Le FD directory vise toujours l'arbre, même après son renommage. Si une synchronisation ou une faute postrename est ajoutée AVANT `published=true`, le retour d'erreur peut vider la destination publiée. Ce drapeau d'appartenance doit être fixé immédiatement après le succès kernel du rename exclusif, avant toute opération faillible ou hook.

## Contrat de correction

Un ACK de publication ne peut suivre que :

1. Archive entièrement validée avec les mêmes bornes, refus de liens/traversées/doublons, écritures et modes finaux.
2. Données **et métadonnées finales** de chaque fichier régulier synchronisées avec `sync_all` ; jamais une barrière placée avant chmod comme seule preuve du mode final.
3. Tous les répertoires de l'arbre, y compris parents implicites et répertoires vides, synchronisés **des feuilles vers la racine de staging**, après les créations de leurs enfants ; racine de staging incluse.
4. Revalidation des capacités parent et staging existantes, puis publication atomique exclusive inchangée.
5. Marquage mémoire `published=true` immédiat, puis synchronisation du **FD parent retenu** qui reçoit le nom final. La réussite n'est retournée qu'après cette barrière et les dernières validations nécessaires.

Avant rename, une erreur laisse un échec non publié et peut nettoyer seulement le staging privé possédé par cette invocation. Après rename réussi, toute erreur signifie **publication potentielle acquise, durabilité/résultat inconnu** : la destination est conservée absolument, sans clear_directory/unlink/rename inverse. Ni une erreur de parent sync ni une réponse perdue ne prouvent que la destination est absente. Une destination existante n'est jamais supprimée, remplacée ou adoptée au retry.

Le parent est déjà une destination autorisée existante. La preuve doit préciser la portée sur ses ancêtres : les barrières de l'arbre et du parent ne prouvent pas à elles seules la durabilité d'ancêtres externes récemment créés sans synchronisation. Si le contrat promet la persistance du chemin complet, synchroniser aussi la chaîne d'ancêtres retenus/recoupés de bas en haut, ou garder cette précondition/qualification explicitement ouverte ; ne pas annoncer une garantie matérielle universelle depuis un simple succès syscall.

## API interne proposée, sans ports runtime nouveaux

Conserver les signatures publiques `extract_workspace(...) -> anyhow::Result<Manifest>` et `AuthorizedExportParent::extract(...) -> anyhow::Result<Manifest>`, et la forme du scanner staging=None. Le traitement partagé reste dans anchored_export. Proposition minimale :

```rust
impl Staging {
    // Parcours/état uniquement de notre staging privé retenu, sans pathname
    // réouvert hors capacité, liens ou fichier spécial adopté.
    fn sync_tree(&self, limits: Limits) -> Result<()>;

    // Cette méthode impose elle-même sync_tree avant rename : les deux callers
    // legacy/autorisés ne peuvent pas oublier une phase.
    fn publish_durable(
        &mut self, target: &OsStr, parent_path: &Path, limits: Limits,
    ) -> Result<()>;
}

// Erreur typée opaque transportée dans anyhow, pas un nouveau statut persistant.
pub(crate) struct ExportPublicationUnknown {
    phase: ExportPublicationPhase, // PostRename / ParentSync / FinalValidation
}
```

`ExportPublicationUnknown` est détectable par downcast/is dans les callers et affiche une explication explicite : la destination peut être publiée, elle est préservée et demande réconciliation. Il ne contient ni credential, contenu exporté, nouveau receipt de succès ni faux booléen de terminal. Préserver la cause IO utile, sans publier de contenu privé. Le service peut continuer à retourner NeedsReconciliation pour cette classe ; un libellé de résultat inconnu doit rester compréhensible pour l'opérateur.

Deux implantations locales sont possibles ; ne pas en faire un framework. Recommandation : écrire/mode comme aujourd'hui, puis un parcours FD ancré du seul staging privé avant publication, qui synchronise les fichiers après leur mode final et les répertoires en postordre. Chaque réouverture interne éventuelle doit rester relative au FD retenu, NOFOLLOW/CLOEXEC, avec vérification type/identité et refus des entrées inattendues. Ne pas garder un FD par fichier (20k possibles). Une pile de parcours et les entrées visitées doivent être bornées ; ne pas prétendre que le nombre d'en-têtes tar borne automatiquement tous les parents implicites. Alternative étroite : synchroniser le FD fichier encore ouvert après chmod et conserver seulement les chemins/identités des répertoires à synchroniser ensuite en postordre ; même exigence de borne explicite. Choisir une seule approche dans l'implémentation, sans doublonner les syncs par réflexe.

La synchronisation reste du travail FS bloquant sur le chemin d'export/pool FS existant ; elle ne doit pas entrer dans le dispatcher des arrêts préarmés. Aucune nouvelle dépendance, logique Python/JS/shell, migration ou service n'est nécessaire.

## Publication, audit/outbox et résultat HTTP

Recoupe indépendante du port existant :

- targets.rs:127–205 persiste l'intention d'export (scope/job/request, identité du parent et nom), **pas son résultat**.
- results.rs:469–473 extrait/publie, puis :475–483 construit ExportFacts et appelle fact_event. C'est seulement `emit_event` :278–294 / Writer enqueue / Journal advance_outbox qui rendent le résultat durable. POST audit puis ACK outbox suivent. `ExportAcknowledged` côté service :1115–1120 n'arrive que sur Ok ; les erreurs :1123–1126 deviennent NeedsReconciliation.
- Après enqueue durable, un POST/ACK perdu est déjà repris par `replay_events` :246–274 avec le même UUID/contenu d'événement, avant le replay du résultat :386–403. Aucune nouvelle extraction n'est nécessaire pour ce préfixe.
- Après rename et AVANT enqueue, un crash/une erreur laisse une destination possible sans résultat durable. Le retry n'a pas le droit de conclure au succès ou à l'absence. Actuellement les bytes helper perdus sont refusés (helpers.rs:545–550, results.rs:456–458) et une destination existante reste exclusive. Conserver ce refus conservateur et son explication ; ne pas ajouter une recopie/suppression/adoption silencieuse pour rendre le retry vert.

Ordre requis après correction : **publication durable → enqueue du succès → POST du même Event → ACK outbox durable → ACK HTTP**. Ne jamais enqueue le résultat de succès avant les barrières de publication : le replay générique de l'outbox pourrait autrement annoncer un export qui n'a jamais été publié. Un échec de sync postrename ne doit pas créer l'événement de succès. Si enqueue/POST/ACK échoue après publication durable, conserver la destination et le résultat inconnu côté appelant ; après enqueue, le replay idempotent retrouve la preuve, avant enqueue une réconciliation explicite reste requise. Aucune atomicité globale FS+Journal+HTTP n'est revendiquée.

Préparer manifest/digest/ExportFacts avant publication peut réduire les erreurs faillibles après rename, mais ne ferme pas la fenêtre crash avant enqueue. Ne pas introduire pour ce lot un sidefile de succès, un marqueur dans l'export ou une nouvelle migration de machine d'états sans contrat séparé. Cette correction ferme l'ACK sans barrières ; elle ne transforme pas le préfixe sans résultat durable en reprise automatique qualifiée.

## RED comportemental proposé avant correction

Le RED doit exercer une vraie archive et le vrai chemin FD/rename sur un TempDir privé ; pas rechercher `sync_all` par grep et pas substituer un faux filesystem.

1. Root ajoute d'abord **seulement une instrumentation cfg(test) non décisionnelle** aux opérations déjà existantes : trace après write/flush, mode final appliqué, mkdir/FD ouvert et succès du rename exclusif. Aucun sync n'est ajouté dans cette préimage RED. Une portée RAII/thread-local garde une trace bornée et isolée ; les tests directs synchrones workspace évitent une dépendance au transport du contexte à travers Tokio.
2. Définir les événements futurs `FileDurable`, `DirectoryDurable`, `ParentDurable`, uniquement émis APRÈS que le vrai `File::sync_all()` correspondant a réussi. Ils ne peuvent pas être émis par flush/chmod/rename. Le code de production n'a aucun log/observer/hook configurable ; l'instrumentation est compilée uniquement pour les tests.
3. Fixture minimale : fichier racine, fichier exécutable nested/a/b et répertoire vide. Faire un export réel dans une destination neuve, constater son contenu/mode et la trace positive rename. Exiger qu'AVANT publication chaque fichier final ait sa barrière, que chaque répertoire enfant/parent implicite/vidé ait été synchronisé dans l'ordre postordre, et qu'avant Ok le parent final ait sa barrière. Avec la préimage instrumentée actuelle, l'export est effectivement publié mais ces barrières manquent : l'assertion échoue pour le comportement absent, sans E0432 ni test basé sur la présence d'une chaîne de caractères source.
4. Contrôle positif du harnais : trace existante des vrais flush/mode/rename non vide et identité inode/FD cohérente ; le RED ne doit pas pouvoir passer parce que le probe n'a jamais été appelé. Même oracle sur l'export legacy et l'export autorisé ; scan_workspace_archive staging=None ne doit créer/synchroniser aucune destination.

Ce RED démontre l'ordre observé du programme et la condition d'ACK absente, pas une coupure électrique. Une instrumentation qui invente les événements sans appel système réel ne constitue pas une preuve ; sa pose immédiatement après le résultat Ok de chaque vraie barrière fait partie de la revue source indépendante.

## Seam de fautes et critères GREEN

Après implémentation, la même couture cfg(test) peut injecter une erreur **après** une vraie opération/barrière, ou garer un enfant propriétaire pour SIGKILL :

```rust
enum ExportTestStage {
    FileSynced,       // vrai sync_all après données+mode réussi
    DirectorySynced,  // vrai sync_all de ce FD répertoire réussi
    RenamedExclusive, // vrai rename réussi, published déjà=true
    ParentSynced,     // vrai sync_all du FD parent réussi
}
// Hook borné, thread-local, cfg(test), jamais corps de fichier ni secret.
// Il reçoit seulement phase et identité/chemin relatif synthétique du test.
```

Cas requis :

- Faute après fichier/répertoire sync avant rename : aucun ACK, original et témoin inchangés, aucune destination publiée ; nettoyage limité au staging de ce test.
- Faute immédiatement après rename (avant parent sync), ou après parent sync avant retour : erreur typée résultat inconnu, destination et tous ses contenus conservés APRÈS Drop ; retry refuse de remplacer, aucun événement d'export réussi ni ACK fictif créé par ce chemin d'erreur.
- Ordre positif : données+mode/fichiers, répertoires postordre incluant vides et implicites, staging root, rename exclusif, parent sync, puis seulement Ok/audit-success/ACK. Le syscall de rename exclusif et les contrôles d'identité ne sont pas affaiblis.
- Destination créée par un tiers entre check et rename : NOREPLACE/EXCL refuse ; aucun remplacement ni suppression de cette destination. Garder régressions liens/traversées/doublons/limites/substitution parent et export autorisé après disparition du projet original.
- Erreur audit/enqueue/POST/ACK après publication durable : destination conservée ; aucun second export destructif ; même Event rejoué si et seulement si sa naissance est déjà durable. Fenêtre avant enqueue reste explicitement inconnue.

Une faute injectée APRÈS un sync réussi valide la propagation/absence d'ACK et la conservation ; elle **ne reproduit pas** un vrai sync_all retournant EIO/ENOSPC ou une panne électrique. Ces cas restent à qualifier sur stockage de test autorisé ; SIGKILL seul ne prouve pas la tenue matérielle après perte d'alimentation. Consigner OS/filesystem/moteur de stockage et limites. Aucun nouveau PASS runtime, audit global ou gate de release n'est créé par ces tests ciblés.

## Empreintes lues et limites

```text
9a692f10f059a0eb02e921d6be2ab73456912e51977e502789fb16a6b295f0a1  src/workspace.rs
5d2beaafa25159cb07d09615344bfad690539d7cd3497294e6bec26fcfe084de  src/workspace/authorized.rs
c9564b9abb342a0eca088b925b568dbc9582292ef1cf6342f276bc57669a6d3f  src/runner/broker/executor/results.rs
890cf28b5b7361ee57055ba5cf0991b6afa0af6f9c3ac87886e1e7aead8e4dba  src/broker/service/targets.rs
```

PASS : diagnostic source et ordre des coutures recoupés. NOT_RUN : reproduction dynamique/RED, compilation/tests, fautes réelles de disque, runtime et fournisseurs. Aucune correction source encore réalisée. Le P0 de durabilité, la fenêtre FS→outbox, les limites de stockage et les gates applicables restent ouverts jusqu'aux preuves de leur périmètre.
