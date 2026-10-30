# Revue indépendante — premier gel checkpoint V2

**Statut final GEL5 : CLOSE au périmètre logiciel et documentaire déclaré.** Les constats et réserves OPEN conservés plus bas décrivent leurs gels historiques ; la clôture finale à la fin de cette note les remplace. Aucun Grant, runtime ou gate global qualifié.

30 octobre 2026. Worktree `/Users/louis/Documents/ai-guard/.worktrees/broker-service`, base/HEAD `3e2cffa86c0d0c4784507e78f289419a9e39890f`. Revue ciblée du code de checkpoint/journal et de la couture pure runner, sans audit global/Daybreak. AGENTS/RTK, contrats applicables, IMPLEMENTATION_PLAN actif et design-review privé pris en compte. Aucun test, Cargo, build, modèle, moteur ou runtime exécuté par le reviewer ; aucun fichier du dépôt modifié. Note privée 0600.

Gel vérifié directement :

```
7017ac8ec9c45e2d760cdbd0086d69be61ad124d1c7901c35f179d02606a2b85  src/broker/checkpoint.rs
3571b3bd3e3b4130117398ad16c4430eddc485fbdf77fafc332df3489119d8a3  src/broker/checkpoint/validation.rs
fcf105d4a83c416b9df1ffc68c050e1f04265dab53f86b57c69207bec8301aa4  src/broker/mod.rs
987bb83670e9ae2a0dab0b1471c773deee5f4b3ed2665f30fba73fbdaf623fda  src/broker/journal.rs
8d8a117b3beafc6f548d14db7ae8d3a50b48f21cc5c95a7d5c71c2cbb579178b  src/broker/journal/types.rs
f09e82f16296fb53b20656a64cdfa754636dd67a1dc72cc74ab86b6d7aebb3c9  src/runner.rs
```

## Statut du premier passage

**Revue non close : BCP-01 confirmé par RED Luna, BCP-02 confirmé par inspection avec repro attendue, BCP-03 candidat Serde non confirmé par exécution, BCP-04 incohérence causale démontrée par inspection avec RED proposé.** Tous ont été transmis à root et au core. Aucun n'est présenté comme une autorisation d'exécution ou une vulnérabilité moteur : le journal reste un stockage de faits et doit recouper le contrôle réel avant tout effet.

## BCP-01 — WorkerMutation ne valide pas le corps de ses variantes

`checkpoint.rs:500-511` valide version, IDs, expected_revision et l'interdiction StartClaim, puis éventuellement la forme générale du reçu. La transition elle-même n'est jamais examinée. `validation.rs:454-469` ajoute le lien vers une réponse worker antérieure, sans contrôle de forme supplémentaire du payload.

Condition suffisante par inspection : partir d'un préfixe V2 cohérent avec Admission acceptée et Prepare acquittés par un vrai Store. Ajouter une opération nouvelle UUID, même worker/attempt et expected_revision connue, response=None, avec `Unknown { reason: "" }`. La forme accepte la requête et le contexte trouve le worker ; la publication peut l'ACK. Le Store réel refuse cette requête par `bounded(reason,1024)` (`audit/control.rs:1416-1417`). Même divergence pour `CleanupResult { removed:true, error:Some("x") }` (`1436-1438`), ou un Created.inspection/Observe.observation dont les champs ne satisfont pas leurs formes/bornes.

Effet : le format promis comme fermé/validé conserve une requête que le contrôle rejettera pour sa forme, indépendamment d'un conflit de révision ou d'une autorité qui aurait changé. Dans une outbox sans réponse d'erreur libre, elle reste inutilement inconnue. Ce n'est pas une demande de rejouer toute la machine d'états centrale contre le worker courant, ce qui casserait l'historique.

Correction : contrôle pur de forme/bornes des variantes WorkerMutation (inspection, observation et dates, reason/error, cohérence removed/error), conservant le refus StartClaim. Ajouter des négatifs ciblés à préfixe valide ; aucune matrice moteur nécessaire. **OPEN au gel lu, RED CONFIRMED par Luna** : `/private/tmp/ai-guard-broker-checkpoint-v2-qualification/mutation-red.log` relu, 1 test exécuté/1 FAIL, assertion `journal accepted pending unknown-empty-reason` à `tests/broker_checkpoint.rs:616`. L'oracle vérifie d'abord le refus Store et sa tête inchangée, puis attend le refus Journal ; cet échec arrive au deuxième contrôle. Aucun test exécuté par le reviewer.

Nuance d'oracle transmise au parent : la seconde variante CleanupResult du même test est soumise à un Worker Reserved, alors que ce résultat exige Terminal. Son refus Store ne distingue donc pas encore l'erreur de forme de l'erreur d'état. Le premier Unknown vide est décisif ; pour revendiquer le deuxième contrôle isolé, utiliser un parent Terminal autrement valide, ou un oracle pur de forme explicite.

## BCP-02 — admission pré-managed : demande courante exacte non recoupée

`admission_context` (`validation.rs:341-369`) compare la demande à l'intention après normalisation d'approval. Dans la branche Admission (`415-430`), il suffit ensuite de retrouver un Decision antérieur Allow avec la même demande/receipt que l'admission ; `context.request` n'est pas comparée à cette demande. Seule la branche Decision approuvée vérifie que current_request est approuvée.

Condition : Decision Initial/Allow déjà acquitté, runner.current_request ensuite approuvé, managed=None ; ajouter Admission pending qui conserve la demande initiale non approuvée et son reçu Initial. Les deux demandes normalisées sont égales à l'intention et le Decision Allow historique existe : tous les contrôles du gel passent. Pourtant le contrat actif exige la demande courante exacte pour l'admission. Avec managed=Some, le validateur historique runner impose déjà l'égalité entre managed.admission.decision_request et current_request (`runner.rs`, ancien bloc de contexte conservé).

Correction retenue par root : comparer admission.decision_request intégralement à `context.request` pour l'unique Admission, sans appliquer cette comparaison aux anciennes opérations Decision. Cela interdit un changement de phase après admission, tout en conservant Initial et Approved historiques distincts. Le contenu d'approval et son hash restent liés à l'intention. **OPEN au gel lu, inspection confirmée et repro Luna attendue ; écart contractuel data-only, sans claim d'autorité.**

## BCP-03 — candidat : inconnus perdus dans le buffering de l'enum Serde

`OperationWire` est internally tagged (`checkpoint.rs:94-96`) et enveloppé par serde_ignored (`100`). Le Serde local 1.0.229 génère, dans `serde_derive/.../enum_internally.rs:49-60`, une lecture via TaggedContentVisitor puis un **nouveau ContentDeserializer** pour les champs de la variante. Le wrapper extérieur ne voyage pas avec ce buffer.

`Arm.owned` réutilise OwnedContainerSnapshot, dont la définition historique ne porte pas deny_unknown_fields (`runtime/managed.rs:118-122`). ContainerObservation, ProcessObservation et ObservationDiagnostic sont également des records ouverts (`runtime.rs:1200-1233`). L'Arm ne possède pas de wrapper de champ owned qui réinstalle la détection après buffering. Un champ inconnu, notamment null, dans Arm.owned ou sa observation peut donc être consommé puis perdu sans callback d'ignorance. `BrokerCheckpointV2::from_bytes` ne compare pas l'entrée à sa sortie canonique ; il peut normaliser silencieusement ce champ. Les wrappers DecisionAnswer/HelperPreparation réinstallent au contraire leur détection dans leur propre Deserialize de record.

Confirmation ciblée demandée à Luna : prendre une enveloppe Arm valide, ajouter `unknown:null` dans owned et owned.observation/process, exiger l'erreur opaque de from_bytes et de serde_json::from_slice::<SupervisionOperationV1>. Tester séparément un connu nullable pour éviter le retour de l'ancien défaut nullable. Le loader de journal compare déjà les bytes complets à leur sérialisation canonique : ce garde refuserait les fichiers persistants portant ce champ ; ne pas lui attribuer rétroactivement le refus de l'API from_bytes.

Correction possible sans changer les DTO historiques : wrapper de désérialisation stricte au niveau des champs après buffering, ou wire privé fermé équivalent. **CANDIDATE source forte, pas de résultat de test revendiqué par le reviewer.**

## BCP-04 — résultat WorkerMutation non lié à l'effet de la requête

`validation.rs:470-485` ne compare le Worker de résultat qu'à l'admission/spec, aux IDs de la requête et à expected_revision+1 ; il impose disposition=None et le receipt exact de la requête. Il ne compare pas ce Worker au parent après application de cette transition. Un receipt d'entrée ne contient aucun hash du Worker de sortie.

Construction algébrique depuis des données réelles : conserver un Prepare et son vrai ACK Store (Preparing rev2, receipt intact). Modifier uniquement `response.worker.state=Terminal` et `response.worker.quota_held=false`. `bound_worker` ne consulte ni state ni quota (30-64), et toutes les contraintes 470-485 restent vraies. Le résultat incohérent est donc accepté par le gel lu. Ajouter ensuite Export pending du même worker/attempt : 691-698 trouve ce prétendu Terminal et considère la dépendance satisfaite, bien qu'un Prepare ne puisse l'avoir produit. Aucun droit central ni moteur n'est démontré, mais le lien causal de la chaîne est faux.

La même famille concerne WorkerClaim : la sortie est reliée aux IDs/start_operation_id/emitter, pas au snapshot attendu après claim (550-569). Le format affirme préserver les réponses réelles et leur contexte ; des champs state/quota/CID hérités doivent rester cohérents avec le parent et la requête, indépendamment de leur authenticité.

Correction minimale à étudier : appliquer sur une copie du parent le reducer pur de transition déjà utilisé par Store, puis comparer le Worker résultat complet (y compris les identités/flags monotones). Pour WorkerClaim, son effet worker correspond au StartClaim interne, tout en gardant le hash du DTO ArmedStartClaim dans le receipt. Éviter une seconde machine d'états divergente. RED proposé à Luna depuis un vrai Prepare ACK, sans moteur ; parent/core prévenus. **OPEN par inspection du gel, repro attendue.**

### Pureté du raccord autorisé par root

Root autorise une couture pure dans audit/control.rs et son réexport craté depuis audit.rs. Le chemin `apply_worker_transition` (1272-1450) et ses dépendances `prepare_worker`, `record_created_worker`, `inspection_matches`, `classify_observation`, `allocation_source` ont été suivis : aucune DB, I/O, lecture d'environnement, horloge courante ou génération d'UUID. Les calculs de dates/UTC et d'allocation dépendent uniquement du snapshot et de la requête fournis ; l'overflow est refusé. L'utiliser sur un clone puis comparer toute la sortie worker est déterministe et approprié, sans SQL nouveau.

Le reducer n'effectue pas lui-même les gardes de version/operation_id/worker_id/attempt_id/expected_revision : le raccord doit les conserver avant l'application. Le reducer incrémente lui-même la révision, il ne faut pas l'incrémenter une seconde fois dans le helper.

WorkerClaim exige cette même comparaison sur le StartClaim interne reconstruit exactement comme `audit/supervision.rs:1174-1184`. Son reçu reste vérifié sous le domaine ArmedStartClaimV1 ; le domaine mutation interne ne le remplace pas. Arm ne modifie pas le Worker : parent historique intégral, requête et receipt exacts déjà contrôlés, sans reducer worker supplémentaire. HelperClaim ne retourne aucun Worker et sa disposition réelle ne peut pas être inférée d'une simple forme de receipt ; la conserver immuable et sans transformer un reload EmitOnce en permission.

## Invariants positifs effectivement relus

- Constructeurs et getters typés présents ; Debug opaque couvre structs et enum public. Versions V2/1 vérifiées, limites 128 opérations/32 helpers/64 inconnues et cap total 32 MiB, EOF contrôlé. Les erreurs publiques ne recopient pas le JSON ou les DTO privés.
- Clé Decision = request_id + human_approved, requêtes historiques comparées à l'intention normalisée. Les réponses et requêtes déjà connues restent immuables ; human_approved ne redescend pas entre V2, ni à la promotion depuis une tête V1 approuvée.
- Naissance response=None et helper.owned=None vérifiée à la première V2 et dans chaque transition suivante. Le loader appelle le même contrôle sur toute la chaîne, pas seulement lors d'advance. Les listes ne peuvent rétrécir ou changer de préfixe.
- `legacy_managed_seen` est calculé pour **tout** le préfixe V1, conservé dans le résultat chargé et utilisé pour refuser la promotion, y compris Some puis None. Aucun changement de validation V1→V1 historique. Rev0 reste V1 initiale exacte.
- Namespace et lien scope contrôlés. Les helpers/Arm/owned exigent managed.binding=Some. Managed pending/uncertain/import legacy sont refusés en V2 ; aucune autorité runtime dupliquée.
- Arm.parent est comparé intégralement à une réponse antérieure acquittée (`known_worker`) et conserve sa révision historique. Les helper permissions sont résolues à nouveau au point de l'Arm contre le préfixe acquitté. ImportVerification relie import terminal réussi et manifeste attendu/observé ; CreateIssued et WorkerClaim exigent une vérification import acquittée.
- Arm, claims, helpers, export et leurs reçus sont recoupés par leurs vrais domaines/IDs. Le hash d'admission couvre SupervisedAdmissionV1 ; les claims n'utilisent pas le domaine mutation legacy. Les dispositions réelles restent conservées, sans reconversion AlreadyConsumed→EmitOnce.
- L'ancien advance V1 refuse dès tête V2 avant tout replay. Le replay V2 exact est vérifié avant la transition contre la tête (`journal.rs:562-575`) ; contexte candidat indépendant de cette tête. La publication exclusive/révision+parent/pointer/fsync/rechargement et le plancher partagé sont communs aux deux versions. Orphelins non promus, pas d'anti-restore après restart revendiqué.

Les preuves finales GREEN, canaris V1/golden, incidents de harnais et documents ne sont pas encore relus dans ce premier passage. D5, ordonnanceur, runtime réel et qualification globale restent hors clôture. La présente note doit être actualisée après corrections et gels finaux ; elle ne vaut pas approbation automatique de push.

## Deuxième passage — GEL2 corrigé

Les constats OPEN ci-dessus décrivent le premier gel et sont conservés comme historique. Au GEL2, les huit hashes ont été recoupés directement. Quatre sources changent :

```
7c33365ebf77a9a3ff5d697d9ea26893517ffb8c77d2bdc23ca0d8c5ba0329cf  src/broker/checkpoint.rs
9786c2cad11940f2024cb751023e42ef2d0bfffc27e99e9c7ce3418173ba8e28  src/broker/checkpoint/validation.rs
9587b78cb74140d7dd0d4daf0af43605c0626c1aeabb6d02373bdf88025b0fb6  src/audit.rs
f13a7d06a6a789aef92f524db337f5e3a1c717080891cb14b3dc73637d267447  src/audit/control.rs
```

mod.rs, journal.rs, journal/types.rs et runner.rs restent aux quatre hashes du gel 1. Le protocole de publication, la promotion, la relecture de chaîne et le CAS n'ont pas de delta supplémentaire à ce gel.

### Fermeture source des constats

**BCP-01 CLOSED au niveau source :** `matching_worker` retourne maintenant le vrai parent antérieur acquitté. Pour toute mutation, réponse connue ou inconnue, `expected_worker_transition` est exécuté avant la branche response (`validation.rs:468-475`). Les payloads Unknown, Created, Observe et CleanupResult passent ainsi par les règles exactes du reducer Store, sans copie divergente de ses bornes ou de sa classification d'inconnu. Une requête mal formée ne peut plus être confirmée comme pending dans le journal. La validation ne demande pas un terminal complet à une observation réellement partielle : elle conserve la sémantique centrale d'InDoubt.

**BCP-02 CLOSED au niveau source :** `validation.rs:420` compare intégralement l'Admission au current_request, y compris avant managed. Le traitement des anciennes décisions continue à utiliser normalized_request ; Initial et Approved ne sont pas confondus. Le RED `admission-phase-red.log` a été relu : 1 FAIL sur l'assertion précise du mélange current approuvé/Admission initiale.

**BCP-03 RED confirmé puis CLOSED au niveau source :** `serde-red.log` puis `serde-red-details.log` ont été relus. Le deuxième log conserve les quatre refus absents au gel 1 : owned/observation × from_bytes/Deserialize. Au GEL2, l'attribut de champ Arm.owned appelle `deserialize_owned` (`checkpoint.rs:89-100,127`), qui remet serde_ignored autour du ContentDeserializer déjà bufferisé. L'ignorance d'un champ owned, observation, process ou diagnostic remonte désormais à ce wrapper et donne une erreur fixe. Aucun lecteur historique runtime n'a été modifié. Le positif de décodage utilise les mêmes faits synthétiques avec les connus nullable présents ; ce test porte sur la forme wire, pas sur un Arm exécuté ou un journal contextuellement armé.

**BCP-04 RED confirmé puis CLOSED au niveau source :** `prepare-ack-red.log` a été relu : 1 FAIL au gel 1 sur l'ACK Prepare modifié. L'oracle courant accepte d'abord la vraie réponse Prepare du Store dans un journal, puis recrée le même préfixe pending dans un autre journal et ne modifie que state/quota de sa réponse ; le refus attendu ne dépend donc pas de l'immutabilité d'un ACK déjà sauvegardé. Au GEL2, le Worker complet est comparé au résultat du reducer (`validation.rs:475-477`). WorkerClaim reconstruit exactement le StartClaim interne, applique le même helper et compare toute la sortie (`551-570`), en conservant le reçu ArmedStartClaim et sa disposition réelle.

### Contrôle du diff audit

Le diff audit.rs ne contient que le réexport craté `expected_worker_transition`. Le diff audit/control.rs ajoute seulement ce wrapper pur (`1272-1288`) : version/UUID/révision positive, taille d'enveloppe <=65536, identités et expected_revision égales au parent, clone, reducer existant, retour du Worker dérivé. Le reducer et ses dépendances, les appels live Store, les domaines/hashes, les requêtes SQL, transactions, redactor et écritures restent inchangés. Il n'y a pas de deuxième incrément de révision, d'horloge courante, génération d'ID, I/O ou Store fabriqué. Le cap d'enveloppe est celui déjà vérifié par mutate_worker ; la validation des secrets reste le garde du journal et l'autorisation reste centrale.

### BCP-05 — branches ACK de même révision

Condition identifiée pendant la revue : deux opérations différentes peuvent être enregistrées pending sur le même Worker Reserved rev1, par exemple Prepare et CancelReserved. Tant que leurs réponses sont inconnues, cette incertitude est représentable. Même un reducer correct pour chaque branche ne suffit pas à permettre deux ACK rev2, ce que le CAS central ne produirait pas.

Le GEL2 ajoute `acknowledged_revisions`, rempli après validation de chaque réponse Worker d'Admission, WorkerMutation ou WorkerClaim (`validation.rs:395,738-758`), avec clé worker_id/attempt_id/revision. Un deuxième ACK de même révision est refusé, y compris s'il recopie le même Worker ; les opérations ont déjà des clés distinctes/uniques. Arm.parent et les pending ne sont pas comptés : le vrai parent historique d'un Arm reste relisible après progression, et plusieurs réponses inconnues ne sont pas artificiellement supprimées.

**BCP-05 CLOSED par inspection source.** Aucun RED ou test dédié de branche contradictoire n'était présent dans le test lu à ce passage ; ne pas attribuer une preuve exécutée spécifique au HashSet. Une régression ciblée est pertinente avant clôture des preuves, sans modèle ni moteur.

### Résultats effectivement relus à ce stade

Les quatre logs RED sont conservés sous `/private/tmp/ai-guard-broker-checkpoint-v2-qualification` et recopiés dans `red-gel1` des preuves en préparation. Le log `gel2-target.log` lu porte **4 PASS, 0 FAIL, 0 ignored**, en 1,66 s de tests. Les premiers RED et leurs numéros de ligne ne sont pas réécrits en numéros du test actuel. L'ancien cas CleanupResult sous parent Reserved a été retiré de l'oracle revendiqué ; le contrôle Unknown vide reste décisif. Aucun résultat de test n'a été généré par le reviewer.

**Verdict code GEL2 : aucun constat bloquant résiduel identifié dans le périmètre relu ; BCP-01/02/03/04/05 traités par le code.** Cette fermeture source reste distincte de la clôture des preuves finales, canaris V1/golden, rapports et documents, encore attendus. Aucun Grant, effet moteur, supervision autonome, D5 ou gate global n'est qualifié par ce verdict.

## Complément producteur réel — BCP-06

Le core a ensuite rapproché le format du vrai ordre de préparation runner, avant le prochain raccord service. Cette lecture ajoute un cas légitime absent des premiers tests managed=None ; elle ne remet pas en cause les corrections BCP-01 à BCP-05.

**BCP-06 — managed préparatoire refusé avant ACK d'admission, OPEN au GEL2.** `runner/managed.rs:1280-1281` copie l'archive puis construit LaunchSpec/admission. Les lignes 1329-1352 créent managed=Some avec admission connue, manifest/archive/digests, worker=None, owned=None, pending/import vides. `PlanState::new` écrit le snapshot (`413-422`) avant le RPC d'admission (`1354-1356`). Le worker et son verrou sont enregistrés seulement après ACK accepté (`1363-1405`) ; Prepare/import/CreateIssued viennent ensuite (`1411-1414`). Le binding peut encore être None dans la préparation Podman : prepare_authority inspecte alors l'image et conserve un seed, avant le bootstrap ultérieur du runtime.

Pourtant `validation.rs:761-763` au GEL2 exige `admitted` pour comparer même `context.admission` seul. Admission pending (response=None), ou refusée (accepted=false/worker=None), laisse admitted=None. Le checkpoint préparatoire cohérent serait donc rejeté, bien que le validateur historique runner accepte précisément managed avec admission/manifest/bindings cohérents et worker=None. Ce défaut est un rejet fonctionnel du parcours préparatoire réel, pas un nouveau manque d'autorité centrale.

**Choix root validé contre le producteur :** distinguer requested (requête Admission unique enregistrée et déjà validée) d'admitted (sa réponse acceptée, avec vrai Worker). Comparer `managed.admission` intégralement à requested.admission. Ne définir admitted qu'après accepted=true/worker=Some et receipt exact. La requête Admission pending doit être dans la même enveloppe que la préparation quand celle-ci est confirmée ; managed seul ne crée pas une demande implicite.

Tous les usages qui conduisent à un worker, helper ou effet restent sous admitted : context.worker ; helpers ; WorkerMutation ; Arm ; WorkerClaim ; puis HelperClaim/Observation/Cleanup via leur Arm antérieur acquitté ; Export. Context.owned sans Worker reste déjà refusé par le validateur historique runner ; un Worker injecté avant ACK doit également échouer au garde admitted. La présence d'un binding préparatoire, d'un manifeste ou d'une archive ne remplace aucun de ces liens. Ne pas exiger binding=Some pour managed seul ; le seuil helpers/Arm/owned reste inchangé.

La correction ne demande ni nouveau DTO d'autorité, ni Worker inventé, ni seconde archive/fichier persistant, ni modification du producteur legacy. Elle permet de conserver les préparations et refus réels. Les comparaisons tenant/origin/options, current_request exact, manifest digest, binding/names/handoff du runner restent appliquées.

Preuve attendue de Luna : positif managed=Some/worker=None avec Admission pending puis ACK accepté, ainsi qu'un ACK refusé conservant la même préparation ; contrôle de load/reopen et snapshots clairement identifiés comme données préparatoires. Négatifs worker/helper/mutation/owned avant ACK accepté avec des données autrement cohérentes ; garder un positif après ACK pour isoler le garde causal. Les reçus viennent du Store réel ; les archives/observations synthétiques ne deviennent pas une preuve de préparation runtime réellement exécutée. **Contrat corrigé approuvé au niveau conception ; correction source et RED/GREEN attendus, aucun test relancé par le reviewer.**

Le verdict code GEL2 précédent reste valable pour ses cinq corrections, mais ne clôt donc pas encore le parcours préparatoire complet. BCP-06 reste ouvert jusqu'au gel suivant.

## Revue de code GEL3 puis GEL4

Nouveaux hashes vérifiés directement, les six autres sources conservant ceux du GEL2 :

```
5733a07b6fc34ccdac6512fc366202291a61cb48f428ece3832ec9c486d3cf15  src/broker/checkpoint/validation.rs  (GEL3)
f9b912457d294e1504b976347ea388e57b7a1f54cf7285b6c6f1a56b9649dbaa  src/broker/journal.rs             (GEL4)
```

### BCP-06 corrigé

Le GEL3 conserve requested séparément d'admitted. La demande est enregistrée après admission_context et égalité current_request (`validation.rs:420-422`) ; un éventuel défaut de décision/reçu fait encore échouer toute la validation, sans publication intermédiaire. À la fin, seule la projection managed.admission est comparée à requested (`763-765`). Worker, mutation, helpers, Arm, claims et Export restent liés au chemin admitted, qui n'est défini qu'après une vraie réponse acceptée avec Worker. Les gardes binding/legacy sont inchangés. Aucun nouveau fichier d'archive, Worker synthétisé par le code ou seconde autorité ajouté.

Le vrai RED `pending-managed-red-final.log` a été relu : le positif préparatoire échoue par Conflict au gel antérieur. `pending-managed-red.log` seul est un échec de compilation du harnais (stored.hash déplacé), pas le RED produit ; cette distinction doit rester dans les preuves. Le `gel3/bcp06-target.log` donne ensuite 1 PASS. La fixture conserve manifest/archive/digests comme données synthétiques et un vrai reçu de décision ; elle ne lance pas le runner ou un moteur pour fabriquer cette préparation.

**BCP-06 CLOSED au niveau source et régression préparatoire positive.** Le cas Admission refusée avec préparation est supporté par requested sans admitted ; aucun test de quota/refus n'était encore présent dans les six fonctions lues à ce passage, donc ce sous-cas n'est pas revendiqué comme PASS exécuté.

### Suppression du décodage V2 redondant

La copie privée GEL3 `/tmp/ai-guard-broker-checkpoint-v2-gel3-source.CMCIFN/journal.rs` a été hashée : `987bb83670e9ae2a0dab0b1471c773deee5f4b3ed2665f30fba73fbdaf623fda`, puis comparée au GEL4. Le diff exact déplace raw_payload canonique dans la branche V1 et retire uniquement le second `BrokerCheckpointV2::from_bytes(&raw_payload)` de la branche V2. Aucun cache ou court-circuit causal ajouté.

Les gardes restantes rendent ce retrait équivalent :

- Le fichier de révision reste lu sous cap 32 MiB et passé au garde de secrets. La lecture raw par `parse<Value>` garde la sélection de version explicite ; version absente/non entière/inconnue refuse.
- `parse<SupervisedCheckpoint>` utilise `serde_json::from_slice` (`journal.rs:1192-1193`), qui exige EOF hors whitespace. Les records externes ont deny_unknown_fields. Le payload utilise son Deserialize fermé : wrappers, contrôle serde_ignored après buffering d'owned, versions/forme et caps de validate_shape. Les champs doublés reconnus sont refusés par les records dérivés ; un champ ignoré quelconque ne pourrait pas survivre à l'égalité canonique complète.
- L'égalité `canonical(&checkpoint) == file.bytes()` (`919`) garde les bytes exacts du record entier : inconnus, doublons ou normalisations silencieuses ne deviennent pas un record confirmé conforme. Version/domaine, opération/intention, numéro/previous, contexte, transition inter-révisions, hash de chaîne et plancher restent vérifiés.
- La branche V1 garde exactement son parse Checkpoint, canonical raw_payload et `CheckpointPayloadV1::from_bytes`, ses bytes et son domaine historique. Rev0, accepted/current/scope/bindings restent inchangés.
- Les fsync fichier/parents, publication CAS contrôlée, mutex/flock, caps d'usage et resynchronisation avant ACK n'ont aucun diff.

**Aucun contrôle de refus nécessaire identifié comme perdu par l'optimisation GEL4.** Cette conclusion vient de la comparaison de code ; les tests publics de décodage ne sont pas présentés comme une campagne exhaustive de corruption des fichiers persistants.

### Oracles supplémentaires relus

**BCP-05 : réserve de régression dédiée retirée.** Le test courant enregistre deux requêtes Prepare pending sur le même parent, garde la vraie réponse Store pour la première, puis synthétise explicitement la deuxième réponse avec ID/hash adaptés (`tests/broker_checkpoint.rs:1285-1342`). Le reducer produirait le même Worker pour ces deux opérations ; le nouveau garde de révision unique est donc nécessaire au refus. La tête ne change pas. Ce deuxième receipt est correctement nommé synthétique, jamais déclaré émis par Store.

**D01 : réserve d'exécution retirée au périmètre testé.** Le graphe crée une vraie branche V1 : rev0 initiale, managed cohérent, puis retour None autorisé par la compatibilité V1. Promotion refusée malgré la tête vide, head et bytes des trois révisions conservés (`2011-2075`). Ce test ne modifie aucun V1 existant hors de sa fixture privée.

**Neuf variantes et quatre helpers : positif réel de données couvert.** `sqlite_store_receipts_cover_all_supervision_operation_variants` utilise de vrais Store SQLite/schema3, décision, admission, mutations, armements et claims. Chaque opération est persistée pending puis acquittée. Import, ImportVerification, FinalScan et Export passent chacun préparation None→owned, Arm/claim, observation terminale et cleanup. Les permissions reprennent le reçu Admission, l'import arm/terminal avec manifeste attendu/observé, ou l'ExportRequest/destination exacts. WorkerClaim et mutations Observe font avancer le Worker sans invalider les anciens parents d'Arm. L'ensemble final des neuf tags est comparé exactement, quatre helpers sont comptés et Store.integrity est valide (`2572-2602`). Autorité, horloge, inspect, terminal et fichier d'archive sont explicitement synthétiques : aucun create/start/stop/export runtime réellement effectué par ce graphe.

**Limite d'oracle conservée :** le négatif helper avant ACK utilise un helper neuf owned=Some et runner initial sans binding (`1893-1949`). Il est refusé, mais aussi par les gardes indépendantes de naissance/binding ; ce test ne distingue donc pas seul le garde admitted. Le positif complet après ACK et la garde source admitted sont présents. Une variante helper owned=None sur managed préparatoire bindingSome isolerait ce contrôle si une preuve dédiée est revendiquée.

**Plancher/replay : portée exacte conservée.** Le premier test V2 ne confirme que rev1, puis vérifie reopen, replay exact attendu depuis rev0 et clone. Il ne restaure pas un ancien pointeur et n'avance pas une seconde révision V2 avant le replay. Le vrai rollback sous plancher writer/clone est couvert par le canari V1 partagé `broker_journal::journal_context_managed_checkpoint_live_floor_refuses_pointer_rollback` ; le chemin de plancher est commun et inchangé. À ce passage, ne pas transformer ce canari en nouveau PASS de rollback V2 ou de replay V2 après révisions V2 ultérieures. Réserve de portée transmise à root, sans défaut source identifié.

### Résultats lus, sans nouvelle exécution

- GEL3 : positif BCP06 1 PASS ; graphe exact debug 1 PASS en 230,66 s, release 1 PASS en 22,39 s. Les tentatives de harnais antérieures (imports inutilisés, tenant erroné, ouverture indépendante Busy, full target interrompue) restent distinctes du résultat ciblé accepté.
- GEL4 : `quick-target.log` 5 PASS/0 FAIL/1 graphe filtré, 1,75 s de tests ; `release-graph.log` 1 PASS/0 FAIL/5 filtrés, 16,71 s de tests après compilation. Ce sont six fonctions distinctes exécutées en deux profils/commandes, pas un résultat d'une commande full target unique. Les timings ponctuels ne qualifient ni charge/latence de service ni performance 100 workers.
- Le manifeste GEL4 avant ces exécutions porte le test `e0f59bc0ea90f80e3bca46555fc8b7438872d5bfdf56186f3e458d33db72039a` et les huit sources ci-dessus ; hash courant du test recoupé à ce même SHA au passage. Les hashes de tests GEL3 historiques différents ne sont pas assimilés à cette dernière exécution.

**Verdict code GEL4 : CLOSE pour BCP-01 à BCP-06 et l'optimisation relue, aucun constat source bloquant résiduel identifié.** La clôture documentaire/probatoire finale attend encore les rapports, canaris, formatage/Clippy/release et leurs manifestes définitifs, avec les limites d'oracle nommées ci-dessus. D5, service/IPC/ordonnanceur/gardien, OS, runtimes et modèle/fournisseur restent hors clôture ; aucun Grant ou gate global qualifié.

## Clôture finale indépendante — GEL5, code/documents/preuves

Les huit sources et le test final ont été hashés directement. Seul checkpoint.rs change depuis le GEL4 : `c300095c30e0a410856f953b2ad4a49addedebf357b33800a696705885e847ff`. La comparaison avec sa copie GEL3/GEL4 confirme seulement deux attributs Clippy large_enum_variant locaux, justifiés sur les enums public/wire de DTO possédés et bornés ; aucune différence de comportement ou de format. Le test final vaut `050df934ae05f01290b87129fdff3cf95c788c084204679476f204a8d94ae9cb` ; son allowance too_many_arguments vise uniquement le helper de fixture à 14 paramètres et explique le graphe partagé. Aucun lint global désactivé.

### Réserves de preuve maintenant résolues

- Le premier test confirme désormais rev2 V2 avec un vrai ACK Decision, puis rejoue rev1 depuis son expected rev0. Il récupère le reçu original et conserve head rev2, y compris via un clone. Il restaure aussi les bytes exacts d'un ancien pointeur rev0 : writer et clone refusent load et advance_supervised ; le bon pointeur V2 est restauré avant drop/reopen, qui retrouve bien rev2. **Réserve GEL4 replay après V2 ultérieure/plancher V2 dédiée levée.** Ce test ne promet toujours aucun anti-restore après restart.
- Le positif préparatoire conserve managed=Some/worker=None pour Admission pending, puis soumet la même demande au vrai Store sous quota max_active_workers=0. Le vrai résultat Quota, accepted=false/worker=None, est confirmé dans le journal sans perdre la préparation. **Réserve GEL4 refus préparatoire exécuté levée.**
- Le négatif helper avant ACK ajoute maintenant aussi un helper neuf owned=None sur le vrai payload managed préparatoire avec bindingSome. La naissance et le binding sont valides ; l'absence d'Admission acquittée demeure la cause de refus. Le graphe complet fournit le positif après ACK. Le vieux négatif owned=Some reste un contrôle combiné, sans porter seul cette preuve. **Réserve d'oracle causal helper levée.**
- BCP-05 a son négatif de révision ACK doublée, D01 sa vraie chaîne V1 managed→None, et le graphe complet ses neuf variantes/quatre helpers avec les permissions réelles Store. Les six constats BCP-01 à BCP-06 sont clos dans le code et les régressions finales nommées ; aucune authenticité centrale n'est inférée d'une forme ou d'un hash.

### Logs et comptes finaux vérifiés

Logs archivés GEL5 relus : cible release entière **6 PASS/0 FAIL/0 ignored**, compilation 52,72 s, tests 17,59 s ; rapides debug **5 PASS/0 FAIL/1 filtré**, 1,84 s. Le filtré et les rejeux ne sont pas additionnés aux six fonctions. Canaris : **12 broker_journal + 9 supervision_store + 1 journal_sqlite + 4 unités journal = 26 fonctions PASS**, dont un self-test de harnais explicitement indiqué. Les deux helpers ignorés sont invoqués par leurs parents, pas deux fonctions produit supplémentaires ; l'enfant SIGKILL ne se transforme pas en PASS indépendant. Les logs finaux formatage, Clippy strict all-targets et build release confirment le succès ; le dernier build 0,12 s est un passage cache après la compilation de la cible release, pas une nouvelle compilation complète.

Les RED comportementaux, le RED d'API absente, les erreurs de compilation/fixtures, l'ouverture indépendante Busy, les corrections et le run debug interrompu gardent leurs statuts et gels propres. Ils ne sont pas remplacés par le résultat GEL5. Aucun test, Cargo, runtime ou modèle n'a été exécuté par le reviewer pour cette clôture.

### Sources et artefacts recoupés

Le manifeste115 `92bb5de2ecaf7275a941328b5b4a8aa4fc9a7ea7850d37092e6e768b973b3ae2` a été vérifié intégralement dans le worktree et dans la copie source privée : **115 OK dans chacun, retour 0**. Les neuf empreintes avant/après build ont le même SHA `e11df891e97879a276a63333276e7c16db53a35b8b1cd56bff789b7efdc8a6a6` et correspondent aux sources/test courants. Le delta déclaré est cohérent avec six fichiers existants modifiés et trois nouveaux ; Root a également recoupé les 106 autres entrées inchangées de la base. La liste du tar ne contient que les entrées source/test/configuration déclarées, sans chemin absolu, base, volume ou binaire.

Hashes recalculés sur les copies privées hors target `/tmp/ai-guard-broker-checkpoint-qualified.VkmKX7` :

| Artefact | SHA-256 |
|---|---|
| source.tar | `2a0b97cc4b047c95bb17ebcf45837ad51a2ba0656063546ce718ba51ddfbaebf` |
| agentsandbox-macos | `b360af55dc6d6b75dd9d9f9ead2d59eecda0428cbe7f68b5cfa833b0a0911f98` |
| libagentsandbox-macos.rlib | `68289f51d52eea893f91a52b7a25f632f71564fb12288d8e7b2ea3ce9042a7c5` |
| broker-checkpoint-test-macos | `e2966caede8524ea7eef8cd8682ea0b0512e82602874a495d2960bfd385d972e` |

Ils concordent avec le rapport et le manifeste des copies Luna. Parent privé 0700 et artefacts 0600 constatés ; la copie supplémentaire tests-final.rs d'abord 0644 a été corrigée en 0600 par Root, puis son mode et son SHA exact final ont été revérifiés. Cette correction de copie n'a changé aucun byte produit. Le golden rev0, sa source de probe, le design initial et la revue de conception correspondent aux empreintes retenues ; le probe est lié à l'ancienne bibliothèque, sans writer V2 pour fabriquer ce golden.

### Cohérence documentaire et limite intermédiaire

README, tests.md, implementation.md, CURRENT_STATE, DECISIONS, IMPLEMENTATION_PLAN, TEST_STRATEGY, VALIDATION et BROKER_SUPERVISION ont été rapprochés du code, des oracles et des logs. Les limites batch/generic/bindingNone et macOS/ARM64 sont explicites. Store, journal, receipts, quotas et audit sont réels ; autorité, observations, horloge et effets runtime demeurent synthétiques. Les étapes service/IPC, cycle de vie local avant admission, ordonnanceur, gardien et D5 sont clairement non implémentées, sans réinterprétation de ces préparations comme une supervision livrée.

**BCP-DOC-01 CLOSED :** la copie courante du rapport d'implémentation affirmait encore que les anciens benchmarks étaient entièrement reconstruisibles. La rectification finale du propriétaire, recopiée au SHA `e2fc87045bc02a59fde26b5d0688f1cefdc862ad66773be44d2c49aee4775c12`, retire explicitement cette portée : seules les deux sources applicatives intermédiaires sont reconstruites aux SHA exacts ; le test e0 exact manque et la tentative d'inversion n'a pas retrouvé son hash. La copie initiale brute reste conservée. README/tests/VALIDATION exposent la même limite. Les temps intermédiaires restent des observations historiques, pas une qualification fondée sur une reconstruction du test perdu ; le GEL5 final, lui, dispose de sa source et de ses artefacts complets. La formulation VALIDATION distingue également correctement les chemins live Store/SQL/corps du reducer inchangés de la couture pure effectivement ajoutée aux fichiers audit.

### Verdict final

**CLOSE au périmètre logiciel/documents/preuves GEL5 : aucun constat bloquant résiduel identifié.** BCP-01 à BCP-06, le garde de branches ACK et l'écart documentaire sont traités. Publication commune, compatibilité V1 et contrôles stricts restent cohérents avec le contrat corrigé. Cette revue ciblée ne constitue ni un audit global ni une approbation automatique d'un déploiement.

**NOT_RUN dans ce lot :** moteurs/workers réels, V2 autres modes/fournisseurs, Linux/Podman/Kubernetes/cloud, authentification IPC/service/ordonnanceur/gardien, mort CLI avec enforcement, D5 irréversible, panne électrique et charge 100. Les canaris des hooks communs ne deviennent pas une nouvelle matrice exhaustive crash V2. Frontend/navigateur non concernés et non rejoués. Aucun Grant, terminal, quota libéré ou autorité restaurée n'est fourni par le seul journal. QA, défaut schema2 et billable=false inchangés ; G09 reste FAIL et 0/12 gates entièrement clôturés.

Le parent doit maintenant recopier cette note finale dans les preuves et générer/vérifier leur SHA256SUMS sur le gel documentaire complet. Aucun fichier du dépôt, archive ou processus n'a été modifié par le reviewer ; seule cette note privée 0600 a été actualisée.
