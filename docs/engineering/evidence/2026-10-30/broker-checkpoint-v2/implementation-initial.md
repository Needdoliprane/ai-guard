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
