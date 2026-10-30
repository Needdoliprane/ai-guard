# Checkpoint broker V2 — rapport d'implémentation

Base `3e2cffa86c0d0c4784507e78f289419a9e39890f`, worktree `/Users/louis/Documents/ai-guard/.worktrees/broker-service`. Contrat actif : section checkpoint V2 de `docs/engineering/IMPLEMENTATION_PLAN.md` et corrections de revue `/tmp/ai-guard-broker-checkpoint-v2-design-review.md`. RED root/Luna : RC101, module checkpoint/advance_supervised/getter absents, aucun test exécuté lors de ce RED. Aucun commit par cet agent.

## Sources possédées et gel 1

Sous-module privé validation.rs annoncé/approuvé par root pour isoler les liens causaux du format. Aucun autre fichier applicatif/test édité par cet agent ; documentation et tests parallèles root/Luna préservés.

| Fichier | SHA256 gel 1 |
|---|---|
| src/broker/checkpoint.rs | 7017ac8ec9c45e2d760cdbd0086d69be61ad124d1c7901c35f179d02606a2b85 |
| src/broker/checkpoint/validation.rs | 3571b3bd3e3b4130117398ad16c4430eddc485fbdf77fafc332df3489119d8a3 |
| src/broker/mod.rs | fcf105d4a83c416b9df1ffc68c050e1f04265dab53f86b57c69207bec8301aa4 |
| src/broker/journal.rs | 987bb83670e9ae2a0dab0b1471c773deee5f4b3ed2665f30fba73fbdaf623fda |
| src/broker/journal/types.rs | 8d8a117b3beafc6f548d14db7ae8d3a50b48f21cc5c95a7d5c71c2cbb579178b |
| src/runner.rs | f09e82f16296fb53b20656a64cdfa754636dd67a1dc72cc74ab86b6d7aebb3c9 |

## Comportement implémenté au gel

Types privés versionnés et constructeurs/getters exacts du plan ; neuf variantes de requêtes/réponses réelles. Bornes128opérations/32helpers/64réponses inconnues et32MiB pour l'enveloppe. Deserialize public utilise des wires privés avec serde_ignored, diagnostics fixes, EOF dans from_bytes, validation de forme y compris nested Decision et OwnedContainerSnapshot. Debug opaque pour tous nouveaux types publics ; aucun Value nouveau persisté.

Transitions : listes en préfixe, requêtes/parents/snapshots et réponses connues immuables ; nouvelle opération response=None et nouveau helper owned=None ; None→Some seulement sur entrée antérieure. Validation première V2 et de chaque transition relue. Unicité décisions par request_id+phase, autres operation_id, subjects Arm, claims et destinations Export.

Contexte pur : intention/scope/manifeste, demandes historiques normalisées et receipts exacts, admission pending/acceptée possible sans managed et sans Worker inventé. La Decision Allow acquittée précède l'admission ; identité worker/attempt issue de la vraie réponse acceptée. Helpers/Arm/owned exigent binding runtime réel dans managed. Arm.parent égale intégralement un Worker d'une réponse antérieure ; identité physique du worker reste intent.operation_id, helper=helper_id. Permissions import/vérification/export reliées aux receipts antérieurs. ImportVerification réelle et manifeste égal avant CreateIssued/claim worker. Receipt/DTO cohérents ne prouvent aucune authenticité centrale.

La seule couture runner est une vue crate en références sur request/admission/worker/binding/owned et flags de présence legacy ; aucun ManagedCheckpoint public, aucune reconstruction JSON, aucun remaniement execute_plan/PlanState/runtime. Les pending/uncertain/import legacy sont refusés dans V2.

Journal : révisions V1 et V2 dans la même chaîne et publication/CAS existants. Rev0 initial V1, fichiers accepted/current/scope/bindings, domaines/bytes V1 conservés. Domaine hash2 séparé ; parser par version de chaque révision. LoadedCheckpoint interne offre checkpoint() compatible et broker_checkpoint_v2(). Détection interne d'un managed dans tout le préfixe V1, même si la tête l'efface : promotion refusée. Approbation de dernière V1 préservée à l'entrée V2. Chaîne V1+V2* seulement ; advance V1 refuse aussi les anciens replays après tête V2, selon décision root. Replay exact V2 renvoie son reçu historique avant contrôle contre la tête ; mutation nouvelle garde CAS/publication exclusive/fsync/pointer/plancher/caps communs. Aucun second protocole d'écriture ni promotion d'orphelin.

## Vérifications et état

PASS constaté ici : `rustfmt --edition 2024 --config skip_children=true src/broker/checkpoint.rs src/broker/checkpoint/validation.rs src/broker/mod.rs src/broker/journal.rs src/broker/journal/types.rs src/runner.rs`, exit0 (parsing/formatage seulement).

NOT_RUN par cet agent : Cargo, compilation, tests, Clippy, build, runtime ou fournisseur. Cache Cargo propriété exclusive Luna ; sources gelées notifiées root/Luna pour son premier GREEN. Ce rapport ne prétend pas que le code compile ou que les tests passent avant leurs résultats. Revue d'implémentation encore attendue au gel1.

## Limites maintenues

Stockage de faits data-only : aucun receipt journal, chargement, disposition EmitOnce stockée ou constructeur ne donne un droit de start/stop/terminal/libération de quota. Le prochain service doit authentifier frais et recouper contrôle/runtime ; scheduler séparé du writer/mutex/HTTP reste à livrer. Aucun fencing irréversible D5 n'est fourni par stop_snapshot_owned. Pas d'anti-restore supplémentaire après restart, pas de preuve power-loss, runtime/TTY/100workers ou gate production fermé.

## Gel2 — corrections de qualification confirmées

Root/Luna ont reproduit sur gel1 constant les quatre écarts logiciels dans `/private/tmp/ai-guard-broker-checkpoint-v2-qualification/` : Unknown reason vide accepté en pending malgré refus Store (`mutation-red.log`) ; admission initiale sous runner approuvé (`admission-phase-red.log`) ; extra:null dans Arm.owned/owned.observation accepté par from_bytes et Deserialize (`serde-red-details.log`) ; ACK Prepare altéré seulement en Terminal/quota=false accepté (`prepare-ack-red.log`). Ce ne sont pas des grants/runtime exploits démontrés. Luna a explicitement arrêté Cargo avant ce correctif commun.

Root a autorisé deux sources supplémentaires : `src/audit/control.rs` expose une fonction **purement cratée** expected_worker_transition(parent,mutation) qui valide enveloppe/IDs/révision, clone le parent et appelle le reducer déterministe apply_worker_transition déjà présent ; `src/audit.rs` ne fait que son réexport interne. Aucun Store/SQL, reducer, hash ou procédure de migration modifié. Le checkpoint vérifie les pending réels par ce reducer et compare intégralement les réponses worker au clone attendu, y compris les WorkerClaim projetés en la même mutation typée que le Store. Cela réutilise aussi les formes/bornes métier existantes au lieu de copier la machine d'états.

Arm.owned reçoit seulement un deserialize_with local, installant serde_ignored **après** le buffering de l'enum interne ; aucun parseur JSON générique, aucun Value/canonicalisation permissive et aucune modification des DTO runtime historiques. L'admission unique est liée exactement à runner.current_request même managed=None. Les révisions ACK (worker_id,attempt_id,revision) sont uniques parmi Admission/WorkerMutation/WorkerClaim ; Arm.parent est exclu de ce compte et les branches pending inconnues ne sont pas effacées.

| Source | SHA256 gel2 |
|---|---|
| src/broker/checkpoint.rs | 7c33365ebf77a9a3ff5d697d9ea26893517ffb8c77d2bdc23ca0d8c5ba0329cf |
| src/broker/checkpoint/validation.rs | 9786c2cad11940f2024cb751023e42ef2d0bfffc27e99e9c7ce3418173ba8e28 |
| src/broker/mod.rs | fcf105d4a83c416b9df1ffc68c050e1f04265dab53f86b57c69207bec8301aa4 |
| src/broker/journal.rs | 987bb83670e9ae2a0dab0b1471c773deee5f4b3ed2665f30fba73fbdaf623fda |
| src/broker/journal/types.rs | 8d8a117b3beafc6f548d14db7ae8d3a50b48f21cc5c95a7d5c71c2cbb579178b |
| src/runner.rs | f09e82f16296fb53b20656a64cdfa754636dd67a1dc72cc74ab86b6d7aebb3c9 |
| src/audit.rs | 9587b78cb74140d7dd0d4daf0af43605c0626c1aeabb6d02373bdf88025b0fb6 |
| src/audit/control.rs | f13a7d06a6a789aef92f524db337f5e3a1c717080891cb14b3dc73637d267447 |

Rustfmt ciblé des huit sources : exit0. Aucun Cargo/build/test lancé par cet agent ; gel2 notifié root et Luna pour les seules régressions de reproduction d'abord, puis graphe9DTOs/canaris nécessaires. Publication/CAS n'a reçu aucun changement depuis gel1. Le résultat du gel2 et la revue finale ne sont pas encore revendiqués.

## Gel3 — préparation réelle avant admission

BCP-06 a été reproduit sur gel2 par Luna : CPV1 préparé valide avec managed=Some, worker/owned/helpers/Arm absents et admission exactement égale à la requête pending, après vrai Decision ACK Store ; advance refusait Conflict. Preuve `/private/tmp/ai-guard-broker-checkpoint-v2-qualification/pending-managed-red-final.log`, sources.before-pending-managed-red.sha256. Aucun Cargo n'était en cours au correctif.

Le lien est maintenant séparé : requested désigne l'Admission enregistrée même pending/refusée ; managed.admission lui est exactement recoupé. admitted désigne uniquement l'ACK accepté et garde ses seuils worker/helpers/Arm/claims. Aucun binding exigé pour managed seul (préparation Podman possible sans binding), aucun Worker/Grant créé par une requête. Une seule source change : `src/broker/checkpoint/validation.rs` SHA256 `5733a07b6fc34ccdac6512fc366202291a61cb48f428ece3832ec9c486d3cf15`. Les sept autres SHA du gel2 restent exacts. Rustfmt de cette seule source : exit0 ; aucun Cargo ici. Gel3 notifié root/Luna pour test ciblé puis parcours9DTO, résultat encore attendu.

## Gel4 — retrait d'un parse V2 redondant, sans cache

Root/Luna ont terminé le même graphe des neuf variantes sur gel3 : PASS debug230,66s ; PASS release22,39s (compilation LTO53,18s). Ce sont des temps de test logiciel complet, pas une qualification de capacité, de deadline runtime ou de100workers. Le fichier test était gelé e0f59bc0…72039a. Le processus a été terminé avant toute édition.

Root a autorisé un unique delta de coût : dans journal.rs, ne plus reparcourir le payload V2 avec BrokerCheckpointV2::from_bytes après que parse::<SupervisedCheckpoint> l'a déjà désérialisé strictement. Le payload raw n'est plus canonicalisé en branche V2 ; la branche V1 garde exactement sa vérification existante. La frontière restante est toujours serde_json::from_slice sur le record complet (EOF), les Deserialize stricts publics corrigés, l'égalité canonique **du record entier**, puis tous les contrôles de contexte/transitions/chaîne/caps/FS/durabilité. Aucun cache ni canonical/validate_shape n'a changé. Le contrôle retiré était redondant avec ces barrières ; aucun tri permissif de champs inconnus n'est utilisé comme validation.

Gel4, huit SHA complets :

| Source | SHA256 |
|---|---|
| src/broker/checkpoint.rs | 7c33365ebf77a9a3ff5d697d9ea26893517ffb8c77d2bdc23ca0d8c5ba0329cf |
| src/broker/checkpoint/validation.rs | 5733a07b6fc34ccdac6512fc366202291a61cb48f428ece3832ec9c486d3cf15 |
| src/broker/mod.rs | fcf105d4a83c416b9df1ffc68c050e1f04265dab53f86b57c69207bec8301aa4 |
| src/broker/journal.rs | f9b912457d294e1504b976347ea388e57b7a1f54cf7285b6c6f1a56b9649dbaa |
| src/broker/journal/types.rs | 8d8a117b3beafc6f548d14db7ae8d3a50b48f21cc5c95a7d5c71c2cbb579178b |
| src/runner.rs | f09e82f16296fb53b20656a64cdfa754636dd67a1dc72cc74ab86b6d7aebb3c9 |
| src/audit.rs | 9587b78cb74140d7dd0d4daf0af43605c0626c1aeabb6d02373bdf88025b0fb6 |
| src/audit/control.rs | f13a7d06a6a789aef92f524db337f5e3a1c717080891cb14b3dc73637d267447 |

Rustfmt journal.rs ciblé : exit0 ; aucun Cargo par cet agent. Root/Luna notifiés ; résultats et revue du gel4 attendus, aucun PASS anticipé.

### Reconstructibilité du benchmark gel3

Sur demande root après cinq tests rapides gel4 PASS, le journal.rs gel3 a été reconstruit **uniquement sur une copie privée** par inversion du petit delta de gel4 : `/tmp/ai-guard-broker-checkpoint-v2-gel3-source.CMCIFN/journal.rs`, fichier0600/parent0700. SHA vérifié exactement `987bb83670e9ae2a0dab0b1471c773deee5f4b3ed2665f30fba73fbdaf623fda`. Les sept autres sources gel3 sont inchangées et leurs SHA sont déjà consignés ; le benchmark22,39s peut donc être reconstruit avec ses vrais fichiers, pas seulement un manifeste. Aucun worktree réécrit : journal.rs actif reste f9b912457d294e1504b976347ea388e57b7a1f54cf7285b6c6f1a56b9649dbaa. Aucun Cargo exécuté par cet agent.

## Gel5 — dérogation Clippy locale, contrat public inchangé

Clippy strict sur gel4 a constaté large_enum_variant pour les deux enums générés (Arm≈2840octets, seconde variante≈2384), log `/private/tmp/ai-guard-broker-checkpoint-v2-qualification/gel4/clippy-strict.log`. Cargo a été suspendu avant correction. Boxer leurs champs aurait changé l'API neuf variantes fixée pour ce lot ; deux allow(clippy::large_enum_variant, reason=...) sont donc limités aux déclarations SupervisionOperationV1 et OperationWire, avec justification explicite du contrat DTO possédé et des caps logiques de persistance. Aucune suppression globale, aucun changement de format ou comportement ; cela ne constitue pas un quota de mémoire physique.

SHA checkpoint.rs gel5 : `c300095c30e0a410856f953b2ad4a49addedebf357b33800a696705885e847ff`. Les sept autres sources gardent leurs SHA gel4. Copie exacte de checkpoint.rs gel3/4 conservée avant mutation : `/tmp/ai-guard-broker-checkpoint-v2-gel3-source.CMCIFN/checkpoint.rs`, mode0600, SHA `7c33365ebf77a9a3ff5d697d9ea26893517ffb8c77d2bdc23ca0d8c5ba0329cf`. Rustfmt de cette source seulement : exit0 ; gel5 notifié root/Luna, aucun Cargo ici. Le fichier journal.rs gel3 reste également conservé ; les benchmarks antérieurs restent reconstruisibles.

## Rectification finale — portée des reconstructions intermédiaires

Cette rectification remplace explicitement les affirmations précédentes selon lesquelles le benchmark22,39s, ou l'ensemble des benchmarks intermédiaires, serait entièrement reconstructible. **Seules les deux sources applicatives intermédiaires journal.rs et checkpoint.rs ont été reconstruites et vérifiées aux SHA exacts consignés.** Le fichier de test intermédiaire e0f59bc0…72039a n'a pas été sauvegardé ; la tentative d'inversion n'a pas retrouvé son SHA. Il manque donc un élément nécessaire à la reconstruction complète des sources de ces benchmarks. Les sept autres sources applicatives inchangées ne comblent pas cette absence du test exact.

Les observations de durée rapportées, les binaires et les logs conservés restent des éléments historiques de ces exécutions. Ils ne sont ni retirés ni transformés en résultats d'une nouvelle exécution, et ne suffisent pas à promettre la reproduction exacte de leur état source intermédiaire. Les formulations antérieures « peut donc être reconstruit avec ses vrais fichiers » et « les benchmarks antérieurs restent reconstruisibles » sont retirées dans cette portée complète.

La qualification finale **GEL5 est entièrement archivée** et constitue la référence distincte pour son état source/test, ses commandes, résultats et limites. Elle ne restaure pas rétroactivement le test intermédiaire manquant et ne valide aucune comparaison intermédiaire supplémentaire. Le rapport initial brut du dépôt demeure inchangé ; seule cette copie privée reçoit la présente rectification. Aucun code, test, source du dépôt ou cache Cargo n'a été modifié pour cette correction documentaire.
