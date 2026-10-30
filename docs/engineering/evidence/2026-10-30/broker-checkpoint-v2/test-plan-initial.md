# Sélection de tests ciblés — checkpoint broker V2

Statut : **préparation lecture seule**. Worktree `broker-service`, HEAD `3e2cffa86c0d0c4784507e78f289419a9e39890f`, propre à l’inspection. Aucun code/test/Cargo/Docker exécuté. Ne pas écrire RED avant réception et gel du contrat/API V2. Les noms d’API ci-dessous restent comportementaux; ne pas figer les signatures proposées dans le mapping historique.

## Couverture existante à préserver

Dans `tests/broker_journal.rs`, garder les oracles V1 tels quels et les réexécuter ciblés après intégration :

- `none_binding_acceptance_reopens_with_same_intent_submit_and_checkpoint` : rev0, reopen, même intent/submit/checkpoint, rejeu accepté et conflit de submit divergent.
- `checkpoint_cas_accepts_only_the_confirmed_chain_and_replays_original_receipt` : progression linéaire, replay exact d’un CAS ancien, conflit si le payload diffère et head conservé.
- `reservation_ticket_is_live_once_then_interrupted_after_drop_or_reopen` : garantie de réservation préalable et reprise partielle.

Ajouter un golden V1 seulement si l’API/format V2 touche le sérialiseur commun : bytes canoniques du fichier rev0 V1, ref/hash attendus, `previous=null`, ainsi que le domaine de hash V1. Prendre les attentes depuis les bytes de baseline `3e2cffa`, pas depuis le code V2 en cours d’essai. Les fichiers/domaines V1 restent inchangés.

## Nouvelles cibles minimales proposées

1. **Wire DTO V2 fermé et exact** — un fixture canonique V2 fixe l’enveloppe, les noms/variants de l’enum `pending`, et les sérialisations exactes des DTO arm/receipt/claim/helper réellement inclus par le contrat : au minimum `ArmV1`, `ArmReceiptV1`, `ArmedStartClaimV1`, `HelperStartClaimV1`; ajouter observation/cleanup si ces variantes sont persistées dans la V2 retenue. Vérifier roundtrip exact, refus de champ inconnu, tag inconnu et payload surnuméraire; pas de `serde_json::Value` libre. Garder le payload `CheckpointPayloadV1` comme valeur V1 intacte dans le wrapper V2. Aucun runtime/fournisseur appelé.

2. **V1 rev0 → V2 → reopen sur vrai Journal** — fichier journal réel et portée réelle; créer/réserver/accepter rev0 V1, capturer byte-for-byte sa révision et son hash, puis publier V2 depuis la tête attendue. Après reopen, vérifier chaîne sans trou, domaine/hash V2, payload runner V1 identique et DTO supervision V2 complet via l’accès typé prévu. Pour arm/claims/helpers authentiques, dériver au moins les reçus d’un `Store` SQLite temporaire réel en réutilisant le montage éprouvé de `tests/supervision_store.rs` (`worker_arm`, `helper_arm`, claim helpers, `Store::arm_execution`/claims); aucune fixture de moteur ou de fournisseur. La portée du test est le stockage et le typage, pas l’émission d’un effet runtime.

3. **CAS, replay et downgrade V2** — même expected + mêmes bytes renvoie le reçu original, y compris après avance supplémentaire puis reopen, sans nouveau fichier/head; même expected + payload divergent refuse; tête périmée refuse. Après tête V2, tenter l’API advance V1 doit refuser le downgrade. Recharger et comparer head/révisions avant-après chaque refus pour prouver l’absence de mutation. Ne pas confondre replay V2 avec autorisation/permission de start.

4. **Scope/namespace/tenant/intent** — même operation UUID dans des scopes distincts (tenant différent ou `control_namespace_uuid` différent) ne lit/rejoue aucun job de l’autre scope. Dans un scope donné, un wrapper V2 avec tenant/namespace/intent/admission contradictoires au scope, à l’intent accepté ou au payload V1 refuse avant publication; head et fichiers restent inchangés. Vérifier une fois les frontières tenant et namespace séparément; couvrir les écarts d’intent/admission dans une table de mutations. L’intention et le namespace ne deviennent jamais des champs libres/non vérifiés.

## Oracles et exclusions

- Le JSON scope V1 est `{version,installation_id,tenant_id,gateway_origin,control_namespace_uuid,profile_id}`; son hash distingue déjà ces champs. V2 ajoute des contrôles de cohérence d’intention/supervision, mais ne modifie pas le scope V1 ni son chemin historique.
- `CheckpointPayloadV1` est un DTO strict privé `{version,current_request,managed}` avec `Debug` opaque; `Journal::load` valide contexte et chaîne. `Journal::advance` V1 ne doit jamais effacer un checkpoint supervision V2.
- Utiliser le vrai filesystem du journal pour les créations exclusives, fichiers de révision/current et reopen; SQLite seulement via le vrai `Store` lorsque les receipts sont requis. Les tests existants de `supervision_store.rs` apportent les constructions/transactions arm, claim worker/helper, et leurs contrôles d’idempotence sans moteur.
- Pas de requalification des pannes/crash, IPC, scheduler, TTY, workers ou fournisseurs ici. Aucun test supplémentaire ne remplace les gates runtime ultérieurs.

## Ordre après gel de l’API

1. Fixer DTO/wire, domaines de hash, règles de lecture V1/V2, comportement du downgrade et signature publique.
2. Écrire RED ciblés uniquement pour les quatre comportements manquants ci-dessus, en laissant les tests V1 existants inchangés.
3. Après correction/intégration, exécuter Luna uniquement sur les fonctions ajoutées et les trois canaris V1 touchés; étendre à `journal_sqlite` seulement si la couture Store/receipt change.
