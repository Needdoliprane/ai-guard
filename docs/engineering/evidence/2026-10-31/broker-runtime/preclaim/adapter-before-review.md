# Adaptateur lifecycle du Writer — duplication supprimée

Trois fichiers exclusivement : `src/broker/journal.rs`, `src/broker/writer.rs`, `tests/broker_outbox.rs`. Aucun changement de `load_locked`, `persistent_sync`, recovery, Store, schéma, credentials, QA ou API publique existante. Aucun Cargo/test/moteur exécuté par l'auteur. Préimage privée : `/private/tmp/ai-guard-lifecycle-adapter-before.Oed61C/`.

## Delta

Nouveau port interne `Journal::advance_lifecycle_preserving_outbox(op, expected, &BrokerCheckpointV3) -> JournalResult<CheckpointReceiptV1>`. Il acquiert le Mutex Writer, conserve `check_writer` et le préload Full strict, puis adapte V3→V4 à partir de ce loaded vérifié. L'outbox courante est clonée intégralement ; aucune entrée ni ACK ne peut être réécrit par l'appelant lifecycle.

L'ancien corps d'`advance_checkpoint` après son préload est extrait en `advance_loaded_checkpoint`, privé, appelé sous le même verrou par les deux chemins. Son contexte, encode/hash, replay/CAS, transitions, usage, publisher, floors, postload Full strict et `BeforeAck` sont conservés. Seul l'ancien `writer::advance_lifecycle` qui chargeait extérieurement le job est remplacé par l'appel au port. Aucun cache de confiance ni saut de barrière : on supprime une lecture d'adaptation redondante, pas le préload ni le postload de la mutation.

## Compatibilité explicite

- V4 Writer : `expected` doit toujours égaler le head courant, même pour le rejeu identique d'une ancienne révision ; sinon Conflict avant adaptation, comme l'ancien helper.
- V1/V2/V3 : validation `validate_command_ids(next, &[])` conservée et règles historiques de replay/CAS inchangées.
- `Journal::advance_lifecycle` public garde son refus de downgrade après V4 ; le nouveau port ne le rend pas permissif.
- Ordre du Writer conservé : UUID nil refusé avant lecture ; preload et contrôles head/IDs précèdent sa validation du hash attendu. Les ports Journal publics gardent leur validation de hash initiale.
- Même publisher physique et même postload strict. Si le binding disparaît pendant publication, aucune confirmation silencieuse : le floor durable est conservé et l'erreur postload demeure. CompletionFailure/Unknown et ownership de queue ne sont pas touchés.

## Preuves ciblées à exécuter par Luna

La fonction existante `broker_outbox::v4_survives_command_and_lifecycle_updates_and_direct_v3_downgrade_is_refused` est étendue : replay historique V3 accepté ; événement ACKé puis bytes complets préservés après commande et mutation lifecycle ; ancien expected Writer V4 refusé sans nouvelle révision ; downgrade public et collisions continuent leurs assertions existantes. Aucun nouveau test de compteur ou fixture runtime.

Canaris existants pertinents : `broker::journal::tests::full_writer_rechecks_binding_after_real_publication_before_ack` pour le publisher/postload commun ; `broker_lifecycle::lifecycle_wire_context_replay_caps_and_legacy_approval_boundaries` pour les règles historiques/contextes/CAS. Le code de ces tests est inchangé. Clippy/fmt globaux restent à l'orchestrateur.

Validation auteur : rustfmt limité aux trois fichiers et `git diff --check` PASS ; tests et Clippy **NOT_RUN par l'auteur**. La réduction d'une lecture Full est établie par le delta ; aucun gain temporel chiffré, correction d'H4/H5/H7, autonomie5s ou fermeture de G09 n'est revendiqué.

SHA : journal `b13798153545c6aa4de5b9437da214759772f76ec504014522570da7a2b527c3` ; writer `47c72983851dc057c22d6731f55b99b0c208bbe8401d8333e00e582d217538d7` ; test outbox `538d89036298c15c6d1d22ab01afa80920f293529e1875be2912f4e741f3ec7c`.
