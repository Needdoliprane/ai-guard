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

## Addendum — relecture du lot correctif SVS01/02/03

2026-10-30, après décision technique du parent : lecture du delta seulement, aucune nouvelle exécution. Le verdict initial et ses preuves restent conservés ci-dessus. Le nouveau Clock n'a pas été lu dans cette revue.

Gel vérifié par SHA-256 :

| Source | SHA-256 |
|---|---|
| `src/audit.rs` | `834cdb1c9c022ed14c2d824248a2abffedbb5c6e90a91237996db61856133208` |
| `src/audit/control.rs` | `2d80c7855dc3e4ab0622c4a41076607ad008b8a107a683dd48efaac60cc895c1` |
| `src/audit/supervision.rs` | `e5fe84ede6ae136e4298c6323aa928dd22b485725cb4b024412ef6aa1053600e` |
| `src/audit/storage.rs` | `ac381df73de99def79aa441853c44ac4ac7b1265d776e2c684e27587490b493c` |
| `src/control.rs` | `afe40dc8477fbb19be6de784a91c135230d2723fef5135499ae9ba7656657d8f` |
| `src/control/supervision.rs` | `1a65c3cd5e9bc10cc8ec98df832a073a0fed1f24ae9fdff45a3631aec2f2484c` |
| `src/api.rs` | `4db5d205e0571a5843e6b92edda61c727c3c14f296d6bbba3631f3d71116e393` |

**Verdict du delta : les trois corrections sont cohérentes par lecture ; aucun nouveau défaut concret identifié dans ce delta. Fermeture comportementale conditionnée aux preuves SQLite finales de Luna.** Il ne s'agit ni d'un PASS de tests déduit du code, ni d'une qualification du fencing D5 ou de la reprise après panne.

### SVS-01 : corrigé dans le code, selon le contrat conservateur approuvé

[SupervisionLease](/Users/louis/Documents/ai-guard/src/audit.rs:69) est privée, hors SQLite et non sérialisée. L'origine fraîche exige `backing.created_here()`, une base effectivement vide et validée lors de sa création. [untouched_fresh_origin](/Users/louis/Documents/ai-guard/src/audit.rs:127) exige aussi tête initiale et watermark exacts, zéro worker et aucune invalidation. La migration ne publie active qu'après son commit vérifié. Une base ancienne vide ne reçoit pas cette capacité ; un événement préalable suffit à empêcher l'activation par ce bootstrap.

[CheckedTransaction](/Users/louis/Documents/ai-guard/src/audit.rs:79) conserve le même garde mutex depuis les vérifications d'identité/préfixe et BEGIN IMMEDIATE jusqu'au commit, contrôle post-commit des noms de fichiers et publication du watermark. Les sœurs ne peuvent donc pas publier leurs têtes dans l'ordre inverse des commits. La vérification du préfixe et du namespace s'ajoute à l'intégrité, au schéma et aux contrôles du backing existants. Une erreur au commit ou dans la vérification post-commit invalide la lease partagée avant retour ; aucun `EmitOnce` n'est alors retourné.

[sibling](/Users/louis/Documents/ai-guard/src/audit.rs:499) ne reçoit ni chemin ni tenant : ouverture depuis l'identité du Store existant, égalité de backing, partage de l'Arc puis vérification sous transaction du namespace/préfixe. Une ouverture indépendante reste inactive, même au même chemin. Aucun paramètre HTTP/config ne crée la capacité.

[persist_supervision_input](/Users/louis/Documents/ai-guard/src/audit/supervision.rs:1031) traite les replays exacts avant la barrière, mais refuse les nouvelles admissions/arms/claims/export inactifs avant append. Les claims historiques rendent toujours `AlreadyConsumed`, jamais une nouvelle permission. Les observations/cleanup helpers restent admis sous leurs validations. [mutate_worker](/Users/louis/Documents/ai-guard/src/audit/control.rs:1215) conserve observe/unknown/stop/cleanup en maintenance, ferme les autres transitions fraîches en v3 et continue d'interdire le StartClaim legacy même lorsque la lease est active. `supervision_state.active` suit désormais la capacité ; l'API traduit le refus typé en 503 `supervision_inactive` sans nouvelle route.

Limites intentionnelles : un redémarrage normal perd aussi la capacité. L'histoire existante reste en maintenance, sans réactivation livrée. Les claims manquants dans une copie ne deviennent pas artificiellement `AlreadyConsumed`. Le bootstrap neuf ne démontre aucune capacité à annuler une ancienne RPC moteur, ni résistance à un administrateur ou à un snapshot matériel du processus vivant.

### SVS-02 : corrigé dans le code

[import_verified](/Users/louis/Documents/ai-guard/src/audit/supervision.rs:489) parcourt désormais tous les arms ImportVerification du parent/attempt : il exige au moins un helper, et chacun doit avoir une observation terminale crédible, exit0 et rapport exact. Un helper armé sans claim, pending, inconnu, sans rapport ou divergent bloque. Les arms ne sont pas supprimés ; un rapport terminal `Some` reste immuable au même helper. Un autre succès ne peut donc effacer un mismatch. Cette fonction est commune au live, au gate CreateIssued, au premier claim et au replay.

### SVS-03 : fermé par refus explicite du parcours non pris en charge

Le fresh Bind est refusé avant append, indépendamment d'un éventuel parent Created. Son replay historique reste lisible. Aucun retour Created→Preparing, nouvelle fenêtre ou exemption de vérification n'a été ajouté. La première reprise legacy annoncée dans la proposition initiale demeure **non livrée** ; le correctif remplace l'acceptation trompeuse par un refus de maintenance explicite, conformément au choix du parent.

### Autre delta et preuves restantes

L'admission supervisée exige maintenant un UUID d'autorité avant mutation ; les règles/hashes AdmissionV1 et LaunchSpecV1 legacy restent inchangés. C'est cohérent avec les préconditions d'arm/helper et évite d'accepter une réservation impossible à poursuivre.

Le harnais en cours a été relu pour la séquence clone avant claim (backup SQLite, Store original vivant), le refus typé sans append sur clone/ouverture indépendante, les ordres success→mismatch et mismatch→success et le fresh Bind refusé. Cette lecture n'est pas une preuve d'exécution. Attendre les logs/manifestes finaux pour la clôture comportementale.

La preuve attendue du watermark doit rester distincte du test clone : rollback d'une histoire SQLite vivante sous le préfixe déjà publié, refus/latch partagé à toutes les sœurs ; concurrence des sœurs sans régression du floor ; origine neuve ayant reçu un événement avant migration restant inactive. Ne pas attribuer cette couverture aux seuls tests de copie/réouverture ni aux suites legacy antérieures. Aucune régression générale ou qualification runtime n'a été relancée par le reviewer.

## Résultats intermédiaires recoupés — canari diagnostic en échec

Lecture directe de `/tmp/ai-guard-supervision-store-green-v5.OazIyQ/target.log` : **9 PASS, 0 FAIL, 0 ignoré** pour `supervision_store`, notamment clone pré-claim, rollback live partagé, 503 maintenance et cas import. Ce résultat apporte une preuve comportementale au lot correctif ; il ne qualifie pas le runtime.

Lecture directe de `/tmp/ai-guard-supervision-store-regressions.wTbhtc/target.log` : **audit_integrity 8 PASS / 4 FAIL**. Correction de provenance après recoupe des lignes source : seul `corrupted_audit_opens_for_diagnosis_but_never_accepts_a_new_event` échoue sur `integrity().unwrap()` (ligne 62). Les trois autres, `external_corruption_is_detected_before_append_without_integrity_endpoint`, `external_quota_corruption_blocks_append_instead_of_granting_capacity` et `external_schema_change_is_detected_before_append`, échouent sur `head().unwrap()` (lignes 34, 151, 46 de la source relue). L'ancienne formulation attribuant les quatre à `integrity()` était incorrecte. Les refus d'append ne sont pas à affaiblir pour corriger ce problème.

**Acceptation du lot encore bloquée par cette régression du diagnostic.** Le groupe s'est arrêté après `audit_integrity` ; aucune réussite des cibles suivantes n'est déduite. Correctif annoncé, pas encore relu à ce stade : rendre le diagnostic possible sans réactiver l'autorité, en conservant contrôles de backing/identité, snapshot cohérent, préfixe et latches. Une branche rollback structurellement valide doit rester indigne de confiance relativement au watermark déjà observé ; le diagnostic ne doit jamais abaisser ce watermark.

## Relecture du correctif diagnostic bba9534c — résultats logiciels encore attendus

Source `src/audit.rs` vérifiée : `bba9534c4f37e247d7497cc3961f40c12e9712d5e557e88d6b7970915e2029ca`, autres sources du gel correctif inchangées. Aucun test exécuté par le reviewer.

La séparation `verify_prefix` / `verify` permet de diagnostiquer les faits sans recréer l'autorité d'exécution. `integrity()` garde backing, schéma, métadonnées tenant/quota, chaîne et projections dans un snapshot cohérent ; une ancienne branche structurellement intacte sous le watermark retourne `valid=false`. Le floor n'avance que sur un diagnostic valide et ne peut diminuer. Aucun chemin ne réinitialise healthy/active. Une erreur de stockage, schéma ou métadonnées peut toujours rester une erreur explicite plutôt qu'un `Ok(valid=false)` ; le correctif ne doit pas être présenté comme convertissant toutes les erreurs en résultats structurés.

`head()` devient une lecture forensic après contrôle du backing ; son succès n'atteste ni intégrité, ni healthy, ni activation. Une substitution de chemin continue d'être refusée. Les mutations vérifient le latch atomique partagé au début et au commit.

**Réserve de concurrence transmise au parent avant clôture :** `verify_diagnostic_backing` pose l'invalidation atomique sans mutex de lease. Une sœur peut avoir passé l'unique `healthy()` de `commit_checked`, puis subir cette invalidation concurrente avant son commit/publication. Le code ne revérifie pas ce latch ensuite. Le commentaire de preuve affirmant qu'aucune émission ne peut suivre une invalidation diagnostique concurrente est donc trop fort. Intercalation établie par lecture, non reproduite ; elle nécessite un échec diagnostique de backing concomitant et que les vérifications ultérieures de l'autre connexion passent. Sérialiser aussi cette invalidation sous le mutex commun, sans interdire les diagnostics d'une lease déjà invalidée, donnerait le même ordre de linéarisation que les autres invalidations. Attente de décision/correctif du parent ; aucune attaque runtime ou résistance à un administrateur n'est affirmée.

## Clôture de la revue ciblée — mutex final et preuves recoupées

Gel final `src/audit.rs` : **`112b1044b2be813f47a3ab6adcfda94942fee05702bbb04d57313b99dfe877a3`**, autres sources du tableau correctif inchangées. Le dernier delta place `verify_diagnostic_backing` sous le même garde de lease, avant contrôle et invalidation des champs mémoire/atomique. Son unique appelant est `head()`, après le mutex de connexion ; aucun appel à `head()` depuis une transaction checked ou la migration n'a été trouvé. L'ordre db→lease reste uniforme, sans réentrance introduite. La réserve de concurrence précédente est **fermée par lecture** : commit/publication et invalidation diagnostique ont maintenant un ordre de linéarisation commun. Un permis déjà retourné avant cette invalidation n'est pas physiquement révoqué par ce mécanisme.

Preuves directement lues et hachées, exécutées par Luna :

- Groupe sur audit **bba9534c**, `/tmp/ai-guard-supervision-store-final-group.nGgTF7/target.log`, SHA `df396ab81cd744afab5c8f68ab108484857e40ca360e960cc88fd594448c46fd` : **94 tests uniques PASS**, 0 ignoré. Les trois sorties enfants de `storage_security` ne sont pas additionnées à son total parent de 17. Manifestes avant/après identiques, vérifiés avec `cmp`.
- Après mutex final **112b1044**, `/tmp/ai-guard-supervision-postmutex.sxvTg8/target.log`, SHA `58265163df73c4100477dff3a275c9c803c808e403e773833107640fc216b400` : **Store 9/9, audit_integrity 12/12, audit_security 3/3 PASS**, 0 ignoré. Manifestes avant/après identiques et source audit conforme au gel. Ces 24 tests recouvrent le groupe précédent ; le total n'est pas 118. Les 70 autres n'ont pas été rejoués après ce petit delta.

Les neuf Store couvrent notamment clone pré-claim inactif, rollback SQLite en place à identité de fichier conservée et invalidation des sœurs, reprise de reçus consommés sans nouvelle émission, mismatch dans les deux ordres, helper pending bloquant, Bind refusé et réponse maintenance 503. Les quatre régressions de diagnostic précédemment conservées sont désormais PASS sans affaiblissement des assertions recensées. La suite finale atteste les scénarios testés ; l'ordre mutex et l'absence de réentrance sont établis par lecture, pas par une prétention de stress exhaustif.

**Verdict final de cette revue : SVS-01, SVS-02 et SVS-03 fermés sur le périmètre Store conservateur ; régression diagnostic et réserve de concurrence fermées. Aucun finding bloquant restant dans le delta relu.** SVS-03 est fermé par refus explicite du parcours legacy non réactivable, pas par livraison de sa reprise. Les échecs et gels précédents restent conservés ci-dessus.

Fmt/Clippy/build release finaux ne sont pas encore une preuve de cette revue : le parent les lance ensuite. Aucun test exécuté par le reviewer, aucun fichier applicatif modifié, aucune lecture du prochain Clock. D5, réactivation après redémarrage, broker/IPC/guardian et effets runtime restent non qualifiés ; cette clôture n'en réduit pas les exigences.
