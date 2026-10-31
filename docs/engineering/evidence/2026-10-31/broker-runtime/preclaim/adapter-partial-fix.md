# PR2 — continuité des réservations héritées

RED Luna confirmé : compilation OK, test `broker::journal::tests::lifecycle_adapter_retains_observation_of_an_inherited_partial_job` FAIL après déplacement du job, car le status ne renvoyait pas Conflict. Preuve Root `/private/tmp/ai-guard-journal-partial-red.GgRDO1`.

Le défaut concernait le nouveau port : son appel direct à `load_locked` retournait Interrupted sur une réservation héritée avant d'en retenir l'observation. L'ancien helper Writer passait par `Journal::load` puis `status_locked`, qui inscrit ce préfixe dans observed_jobs. Un déplacement ultérieur pouvait ainsi devenir une absence dans le nouveau chemin.

Seule correction produit : dans `Journal::advance_lifecycle_preserving_outbox`, le préload devient le même match de status que l'ancien `Journal::load` suivi de `ok_or(InvalidInput)` : Accepted fournit le job ; None ou Rejected renvoie InvalidInput ; les autres états renvoient Interrupted. Mutex/check_writer restent au même emplacement. `status_locked` appelle une seule fois `load_locked` pour Accepted : la lecture Full redondante ne revient pas. La réservation héritée passe par la logique d'observation existante.

Aucune modification de status_locked, load_locked, floors, contrôles, sync, publisher, adaptation V4, CAS ou postload. Test RED inchangé. Aucun Cargo, rustfmt ou moteur par l'auteur. Diffcheck ciblé PASS ; GREEN/format/Clippy/release réservés à Luna. Pas de résultat G09 revendiqué.

Gel : journal `bbf829317219756e075cfa18e5d55f88ee54886de5ba99ec82a8cd70613c36ab` ; test journal `3d1ad064979dbee4670ddb32f307f78f0fc11996f8294eb58d030b8d69eb5ad0` ; writer `47c72983851dc057c22d6731f55b99b0c208bbe8401d8333e00e582d217538d7` ; test outbox `538d89036298c15c6d1d22ab01afa80920f293529e1875be2912f4e741f3ec7c`. Diff exact transmis au reviewer indépendant.
