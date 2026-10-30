# Revue indépendante — premier gel checkpoint V2

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
