# Revue indépendante ciblée — Store supervision v3

2026-10-30. Revue de code, sans exécution de tests, moteur, fournisseur, service ou API. Aucun fichier du dépôt modifié. Guidance security-audit uniquement ; ce rapport ne constitue pas l'audit global Daybreak ni la qualification du broker futur.

## Périmètre et gel

- `src/audit.rs` : `d65534460a5028ec5ee0e063f36d5b8e225351ac24d66395283cf3f8bbe80832`.
- `src/audit/control.rs` : `34802e9f29ed6e1f0d81472d4ec569da8f1bc51096e3db26e85f7bff7a3dc852`.
- `src/audit/supervision.rs` initial : `567de58ad51910728a3144872fdae47114dfcf13c1cc037c32a49833521f811c`, conservé dans `/tmp/ai-guard-broker-store-supervision.rs`.
- Même module après correction de compilation : `eedb9253420e582a0f570d8267497d2662af2ea08ceffbd554da7b10e69ceb55`. Diff vérifié : uniquement `!observation.running` devient `observation.running == Some(false)` ligne 280.
- DTO inchangé : `src/control/supervision.rs` `9658a67cc8b030beb9418ec76eb71df4316dd0c8b77f938f6bf8f4737002d10a`.
- Comparaison avec `/tmp/ai-guard-broker-core-baseline.YbL9P1`, contrats v1 et v2 amendé, rapport `/tmp/ai-guard-broker-core-implementation.md`. Les lignes ci-dessous désignent ces sources ; la correction de compilation ne les décale pas.

**Verdict : corrections nécessaires avant exposition/activation du protocole.** Trois défauts établis par lecture et séquences déterministes ci-dessous ; reproduction SQLite à réaliser par Luna. Aucune preuve d'exploitation d'une API actuellement exposée n'est alléguée.

## SVS-01 — P1 conditionnel à l'utilisation du Store : schéma valide assimilé à une autorisation d'exécution

Sources : [require_v3](/Users/louis/Documents/ai-guard/src/audit/supervision.rs:221), [supervision_state](/Users/louis/Documents/ai-guard/src/audit/supervision.rs:916), [mutation commune](/Users/louis/Documents/ai-guard/src/audit/supervision.rs:1034), [EmitOnce](/Users/louis/Documents/ai-guard/src/audit/supervision.rs:1183), [migration](/Users/louis/Documents/ai-guard/src/audit/control.rs:1058).

`supervision_state.active` vaut directement `schema_version == 3`. Le passage par `require_v3` vérifie seulement cette version avant admission/arm/claim. Ni l'ouverture d'une base restaurée ni ces méthodes n'exigent un état de maintenance distinct ou une autorité d'activation extérieure à la copie. Le commentaire « no runtime activation or restore fencing is implied » de la migration contredit donc la métadonnée et les permissions réellement renvoyées par le Store.

Reproduction minimale prévue, sans moteur : préparer une base v3 valide avec import vérifié, worker Created et arm durable ; conserver une sauvegarde privée cohérente avant le claim ; consommer le claim dans l'original (`EmitOnce`) ; fermer et ouvrir la sauvegarde dans un emplacement privé distinct, même tenant ; consulter `supervision_state` puis soumettre les mêmes octets du claim. Le schéma/historique copié satisfait les contrôles, l'opération manque dans cette branche, `replayed=false` et `EmitOnce` sont de nouveau obtenus. Le même phénomène s'applique au claim d'un helper. Un replay dans la base originale renvoie correctement `AlreadyConsumed` ; ce n'est pas le cas d'une restauration antérieure à la consommation.

Impact : une copie intègre peut se déclarer active et renouveler une permission d'émission malgré une autorité ancienne possiblement vivante. Le contrat v2 ligne 123 impose maintenance seule et fencing conservateur avant reprise. L'absence de broker/route actuellement livrés limite l'exposition présente ; elle ne rend pas la réponse Store conforme. Ce finding ne réclame pas de prétendre que D5 est déjà résolu.

Correction attendue : séparer capacité de schéma et activation autorisée ; par défaut, une ouverture/migration/restauration ne doit pas suffire à octroyer une nouvelle admission/émission. La barrière doit être appliquée au chemin des mutateurs, pas seulement à l'affichage. Un booléen actif copié dans la même sauvegarde ne constitue pas une protection anti-rollback. Tant que la preuve externe de fencing n'existe pas, conserver un refus explicite des effets nouveaux, avec les lectures et opérations conservatrices nécessaires à la maintenance.

Test nécessaire : scénario deux branches ci-dessus, copie après arm et avant claim, pour worker et helper ; vérifier refus de nouvelle émission et aucune écriture de claim sur la branche non activée. Ne pas qualifier D5 avec ce seul test SQLite.

## SVS-02 — P2 : un succès ancien neutralise un mismatch d'import plus récent

Sources : [import_verified](/Users/louis/Documents/ai-guard/src/audit/supervision.rs:489), [observations distinctes](/Users/louis/Documents/ai-guard/src/audit/supervision.rs:746), [CreateIssued](/Users/louis/Documents/ai-guard/src/audit/control.rs:1220), [claim](/Users/louis/Documents/ai-guard/src/audit/supervision.rs:636), [subject keys](/Users/louis/Documents/ai-guard/src/control/supervision.rs:483).

`import_verified` accepte dès qu'un des helpers terminaux de l'attempt porte un rapport égal au manifeste admis. Plusieurs ImportVerification sont légalement possibles, car leur sujet est unique par helper UUID. La protection terminale est individuelle au helper ; un résultat contradictoire d'un autre helper n'invalide pas le succès précédent.

Séquence de reproduction : admission supervisée et Prepare ; Import arm/claim/terminal exit0 ; vérification V1 arm/claim/terminal exit0 avec manifeste exact ; vérification V2 sous un autre helper UUID, même parent toujours Preparing et mêmes reçus d'import, terminal exit0 avec un manifeste observé différent mais bien formé ; demander CreateIssued. V1 satisfait toujours `any`, donc CreateIssued reste autorisé, puis le claim worker peut encore réussir. Le replay reconstruit cette même décision. Aucun reçu forgé, changement de quota ou accès moteur n'est nécessaire.

Impact : la continuation est autorisée malgré une divergence déjà connue et auditée. Cela viole le contrat v2 : un rapport mismatch est conservé et bloque la continuation. La faille ne suppose pas que le gateway puisse vérifier lui-même le filesystem ; elle concerne le traitement de deux rapports qu'il a acceptés.

Correction attendue : définir un état de vérification par parent/attempt qui ne sélectionne pas opportunément un ancien succès. Au minimum, une divergence observée doit bloquer de façon durable jusqu'à une réconciliation explicitement prévue ; appliquer exactement la même règle aux chemins live, CreateIssued, claim et replay.

Test nécessaire : success→mismatch avec deux helpers distincts, vérification du refus CreateIssued, absence de claim/quota/usage inventés, fermeture/réouverture puis même refus. Ajouter l'ordre inverse si la règle retient un état d'incohérence persistant ; un nouveau succès ne doit pas effacer implicitement cette incohérence.

## SVS-03 — P2 : bind d'un legacy Created accepté, mais reprise définitivement impossible

Sources : [Bind](/Users/louis/Documents/ai-guard/src/audit/supervision.rs:658), [claim](/Users/louis/Documents/ai-guard/src/audit/supervision.rs:636), [préconditions helper](/Users/louis/Documents/ai-guard/src/control/supervision.rs:523), [Prepare](/Users/louis/Documents/ai-guard/src/audit/control.rs:1586).

Le contrat v1 §7 conserve une première reprise d'un legacy Created non consommé après vérifications/quiescence. Le Store accepte précisément ce cas dans `bind_legacy_intent`. Toutefois, le claim exige une ImportVerification centrale, qui n'existait pas dans la base v2. Après migration, Import et ImportVerification ne peuvent être armés qu'en Preparing, sans create_operation_id. Le worker accepté par Bind est déjà Created avec create_operation_id et CID ; Prepare ne permet pas de le ramener à cet état. Il n'existe donc aucune séquence admise pour fournir la preuve manquante.

Reproduction minimale : en v2, admission NoLogs → Prepare → CreateIssued → Created, sans start ; migration v3 ; Bind valide → arm worker valide → claim : Conflict. Une tentative d'arm ImportVerification échoue sur l'état/les préconditions. Aucun appel ne peut compléter la preuve exigée.

Impact : le Store accepte et journalise un bind/arm sans parcours de première reprise possible ; quota conservé et diagnostic réduit à un conflit générique. Ce refus est sûr, mais ne livre pas le contrat annoncé. Les anciens claims déjà consommés ne sont pas concernés et ne doivent jamais être réarmés.

Choix technique nécessaire : soit concevoir un parcours de vérification de migration explicitement borné, avec reçus/provenance et fencing conservateur, soit refuser ce bind dès l'entrée et spécifier la procédure de clôture conservatrice puis nouvelle tentative autorisée. Ne pas enlever simplement `import_verified` pour les Created : cela contournerait le gate d'import demandé.

Test nécessaire : legacy NoLogs Created non consommé migré, preuve explicite de la politique choisie, conservation du refus pour consumed/ambiguous et reprise idempotente ; aucun faux succès fondé uniquement sur Bind/Arm.

## Correction de compilation du gel initial

`terminal_helper` appliquait `!` à `Option<bool>`, impossible en Rust. Signalé immédiatement au parent. Le delta courant utilise `== Some(false)` : `None` n'est pas terminal. **Corrigé par lecture du delta**, sans compilation exécutée par le reviewer ; ne pas transformer le gel initial en PASS rétrospectif.

## Points satisfaisants vérifiés par lecture

- Versions 1/2/3 explicitement traitées ; DDL et colonnes des projections v3 contrôlés, reconstruction complète depuis les événements, refus des événements/version inconnus. Pas de bypass `version != 2 → Ok` pour v3.
- Les permissions Import/ImportVerification/Export résolvent de vrais reçus et exigent une antériorité d'audit ; l'import terminal et l'export destination/hash sont liés au bon parent/attempt. Permissions revalidées au claim helper.
- Audit, index global d'opérations, projection et claims partagent la transaction IMMEDIATE ; contrôles de révision et sujets uniques. Replay identique conserve reçu/deadline et rend AlreadyConsumed dans la même histoire ; divergence refuse.
- Fresh legacy admit/start refusés en v3, y compris au replay post-genesis. Helpers sans allocation worker et sans libération du quota parent ; cleanup seulement après preuve terminale.
- Migration explicite, nouvelle sauvegarde privée sans écrasement, vérification/fsync et égalité de tête avant DDL. Nouveaux Store toujours initialisés v2. Cela protège la migration de données ; ce n'est pas une preuve de fencing runtime.
- Sérialisation/hashes internes legacy conservés ; aucun changement de RunPlan1/2/3 ou binding fournisseur dans ce lot.

## Preuves et limites

Lecture des logs intermédiaires `/tmp/ai-guard-supervision-store-first.vCvkx5/target-first.log` (échec de compilation du harnais) et `/tmp/ai-guard-supervision-store-red.zp6MFY/target-red.log` (3 PASS/2 FAIL, échecs pendant la préparation du helper). Ces échecs ne reproduisent pas SVS-01/02/03 et ne prouvent pas leur correction. Le harnais était encore en évolution lors de la revue ; aucun verdict global sur les tests Store n'est attribué ici.

Les preuves DTO antérieures, ainsi que les lots API-auth, HTTP shutdown et strict-stop clos, ne qualifient pas ce nouveau Store. AuthContext/IPC, ordonnanceur indépendant, horloge native effective, effets helpers, restauration TTY, fencing/crash et parcours Docker/Podman restent hors preuve de ce lot. Leur absence connue n'est pas comptée comme un nouveau défaut du Store.
