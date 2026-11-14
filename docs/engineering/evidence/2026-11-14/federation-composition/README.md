# Fédération C3 composée — preuve logicielle du 14 novembre 2026

**44 parents uniques PASS sur `f117ceae016049b7274708197c3702218148e56b`,
avec compilation fraîche et check toutes cibles RC0.** Cette campagne vérifie
la composition logicielle C3 et les neuf scénarios chauds B9 sur un seul
artefact courant. Elle ne qualifie aucun gate production.

## Source, besoin et comportement établi

Source testée `f117ceae016049b7274708197c3702218148e56b`, tree
`ea689490e35f2267739d2a30d802b19ac9af04c1`, archive d'entrée SHA-256
`c1926c8cb71909252bd0849f04b0f83a5458cad25ea8aeea25aaa8a43400e675`.
La fusion ultérieure `b889144a365c012ac4c1d1f485fd0bba27ee7d33` ne modifie que
les six documents de preuve B9 déjà publiés ; elle conserve exactement le
code de `f117`. Aucun nouveau run n'est attribué à cette fusion documentaire.

La sélection vérifie la rotation réellement authentifiée et sa réconciliation :
ACK engagé sous l'acteur historique, preuve de non-commit exigeant rotation et
barrière d'owner, bindings/source/génération exacts et collisions distinctes.
Le publisher conserve les identités engagées, les pending et le journal lors
des réouvertures/supersessions ; les vues heartbeat/checkpoint partagent leur
floor et leur gate jusqu'au vrai save/head update. Ni séquence seule ni ACK
ancien ne devient une autorité.

La route `GET /api/v1/human/owned/brokers` passe réellement par Router, Store14,
projection whitelistée, pagination et audit scoped durable. Les refus vérifient
la permission registre et le contexte administration ; la source sélectionnée
ne lit pas le Store voisin. Le proxy dashboard ne transmet que le GET exact
et son contexte, conserve le refus gateway, et ferme HEAD, mutations, route
legacy et routes voisines. Son peer amont synthétique est un contrat séparé
de la vraie page Store14 ; aucun parcours navigateur n'est établi.

Références de code du gel :
[contrat de réconciliation](../../../../../src/broker_registry/owned_supervision/reconciliation/tests.rs),
[Store/rotation](../../../../../src/audit/broker_registry/owned_reconciliation/tests.rs),
[journal partagé](../../../../../src/broker/presence/owned/journal/tests.rs),
[page Store14 et refus](../../../../../src/api/human_sources/journal_tests/owned_brokers_live.rs),
[proxy](../../../../../src/api/human_sources/journal_tests/dashboard.rs) et
[canaris humains legacy](../../../../../src/audit/human_brokers.rs).
Les neuf B9 utilisent les vrais Journal/SQLite, writer, gateway, IPC et Q
existants, avec observation runtime explicitement `SyntheticReady` ; ils ne
prouvent ni terminal métier, ni libération de worker/quota/usage.
Leur [preuve antérieure](../broker-submitted-pending/README.md) conserve ses
propres gels, RED, abort de pile et limites.

## Commandes et résultats effectivement observés

Dans la QA Linux ARM64 conteneurisée admise, avec toolchain 1.99.1, cache privé
hors ligne, target vide et sources vérifiées avant Cargo :

```console
cargo test --frozen --offline --lib --no-run --message-format=json -j1
cargo check --frozen --offline --all-targets --message-format=json -j1
```

Les deux commandes retournent 0 ; zéro erreur, 34 warnings lib et 168 check.
Clippy strict est NOT_RUN. Le libtest frais est résolu depuis Cargo JSON,
puis lié physiquement par taille/identité/hash avant les listes et groupes :
SHA-256 `a4c642590569abbaded89eadd52d2a573fa165fb09e1075db69fd679905ea83d`,
313796760 octets. Aucun ancien binaire ou résultat n'est importé.

Les 14 listes réelles sont les deux listes globales et les deux listes de
chacun des six groupes (`--list` et `--list --ignored`). Elles établissent
44 FQN uniques actifs, zéro ignoré sélectionné. Chaque groupe est exécuté une
fois avec `--exact` et tous ses noms, puis `--test-threads=1`, capture libtest
normale. Les lignes individuelles, résumés et RC process/phase concordent.
Les listes ne s'ajoutent pas aux tests.

| Groupe | PASS réels | Portée |
|---|---:|---|
| contracts | 4 | ACK/non-commit, prédecesseur/engagement exacts, pins et profils admis |
| store-auth-rotation | 7 | Auth/rotation réelles, réconciliation scoped sans écriture, collisions et refus |
| publisher-journal | 12 | Réouverture, supersession, floors/gate communs et publication durable |
| human-profile-boundary | 10 | Profils et routes fermés, proxy exact, vraie page Store14 non vide et refus |
| legacy-human-canaries | 2 | Décimaux/leases/incarnation historiques ; SQLite-FULL logique sans page ni perte de head |
| broker-warm-nine | 9 | Même original retenu, Pending/Q/Final, refus de preuves étrangères et cleanup exact |

Total : 44 PASS, 0 FAIL, 0 NOT_RUN/inconnu/non vérifié dans la sélection.
Les neuf B9 passent réellement dans ce run ; crédit de PASS B9 antérieur 0.
L'ordre observé est celui de libtest, sans promesse de canari premier ni cause
de pile précise déduite. Le check toutes cibles ne commence qu'après les 44
résultats vérifiés.

## Sélections exactes

Les noms suivants correspondent aux 44 lignes individuelles conservées en privé ;
chaque nom a un résultat PASS dans cette campagne, sans addition aux historiques.

### contracts

- `broker_registry::owned_supervision::reconciliation::tests::noncommitted_proof_requires_exact_predecessor_and_original_request_commitment`
- `broker_registry::owned_supervision::reconciliation::tests::committed_rotation_ack_keeps_historical_actor_and_never_becomes_noncommitted`
- `broker_registry::owned_supervision::reconciliation::tests::rotation_reconciliation_never_weakens_exact_replay_or_accepts_pin_changes`
- `broker_registry::owned_supervision::reconciliation::tests::final_profile_refuses_old_profile_and_unadmitted_source_format`

### store-auth-rotation

- `audit::broker_registry::owned_reconciliation::tests::committed_lost_response_is_acknowledged_under_original_actor_after_real_rotation_without_write`
- `audit::broker_registry::owned_reconciliation::tests::noncommitted_requires_rotation_and_owner_barrier_and_never_uses_sequence_alone`
- `audit::broker_registry::owned_reconciliation::tests::foreign_source_token_and_torn_binding_history_never_become_noncommitted`
- `audit::broker_registry::owned_reconciliation::tests::stale_generation_and_revoked_bearer_cannot_reconcile_with_an_old_verified_proof`
- `audit::broker_registry::owned_reconciliation::tests::global_operation_collision_with_another_broker_is_not_noncommitted`
- `audit::broker_registry::owned_reconciliation::tests::checkpoint_historical_ack_and_global_different_kind_collision_are_distinct`
- `audit::broker_registry::owned_reconciliation::tests::reserved_reconciliation_authenticates_before_body_and_publishes_real_scoped_absence_without_write`

### publisher-journal

- `broker::presence::owned::journal::tests::synced_supersession_survives_reopen_without_new_uuid_or_rewritten_old_pending`
- `broker::presence::owned::journal::tests::insufficient_recovery_budget_refuses_before_closure_and_torn_record_is_kept`
- `broker::presence::owned::journal::tests::v1_anchor_is_refused_without_conversion_or_rewriting_existing_bytes`
- `broker::presence::owned::journal::tests::checkpoint_replacement_preserves_original_snapshot_and_sequence`
- `broker::presence::owned::journal::tests::reconciled_ack_persists_old_actor_and_current_observation_as_distinct_facts`
- `broker::presence::owned::journal::tests::heartbeat_ack_syncs_a_higher_observation_and_blocks_the_checkpoint_view_without_reopen`
- `broker::presence::owned::journal::tests::checkpoint_ack_syncs_a_higher_observation_and_blocks_the_heartbeat_view_without_reopen`
- `broker::presence::owned::journal::tests::a_stale_ack_cannot_publish_below_the_shared_floor_or_clear_either_pending`
- `broker::presence::owned::journal::tests::rejected_observation_does_not_partially_raise_the_other_floor`
- `broker::presence::owned::journal::tests::late_supersession_cannot_publish_below_a_peer_observation`
- `broker::presence::owned::journal::tests::a_fixed_replacement_keeps_its_engaged_identity_without_lowering_the_current_floor`
- `broker::presence::owned::journal::tests::ack_publication_keeps_the_shared_gate_until_the_real_save_and_head_update`

### human-profile-boundary

- `api::human_sources::tests::identity_announces_actual_owned_read_profiles_and_omits_them_on_legacy`
- `api::human_sources::tests::workload_discovery_retains_its_nonzero_source_index_without_neighbor_sql`
- `api::human_sources::tests::owned_broker_page_requires_human_auth_while_legacy_and_head_stay_closed`
- `identity::access::read_profile_tests::read_profiles_are_closed_and_do_not_invent_available_worker_protocols`
- `api::human_sources::tests::modern_route_matrix_closes_before_body_extractors_and_store`
- `api::human_sources::journal_tests::dashboard::owned_dashboard_brokers_forwards_exact_get_context_and_gateway_refusal_without_identity_probe`
- `api::human_sources::journal_tests::dashboard::owned_dashboard_brokers_keeps_head_mutations_legacy_and_neighbor_routes_closed`
- `api::human_sources::journal_tests::owned_brokers_live::owned_brokers_returns_nonempty_whitelisted_pages_with_real_scoped_audit_receipts`
- `api::human_sources::journal_tests::owned_brokers_live::owned_brokers_refuses_administration_without_registry_permission`
- `api::human_sources::journal_tests::owned_brokers_live::owned_brokers_refuses_personal_context_even_with_registry_membership`

### legacy-human-canaries

- `audit::human_brokers::tests::human_projection_preserves_exact_large_decimals_all_lease_states_and_old_incarnation`
- `audit::human_brokers::tests::sqlite_full_during_required_read_audit_returns_no_page_and_keeps_last_durable_head`

### broker-warm-nine

- `broker::service::server::retirement_settlement_tests::same_original_submitted_pending_recovers_q_then_final_without_new_retire`
- `broker::service::server::retirement_settlement_tests::same_original_submitted_final_settles_after_lost_q_without_new_final`
- `broker::service::server::retirement_settlement_tests::same_original_not_queued_final_retries_its_retained_input_before_another_q`
- `broker::service::server::retirement_settlement_tests::same_original_inflight_final_keeps_its_owner_then_consumes_completed_without_readmission`
- `broker::service::server::retirement_settlement_tests::same_original_stale_pending_reads_consumed_final_with_closed_registry_without_readmission`
- `broker::service::server::retirement_settlement_tests::same_public_final_from_foreign_core_cannot_query_or_clear_keeper`
- `broker::service::server::retirement_settlement_tests::absent_local_slot_without_opaque_completion_keeps_original_keeper`
- `broker::service::server::retirement_settlement_tests::actual_retained_verified_input_prevents_another_query_wrapper`
- `broker::service::server::retirement_settlement_tests::confirmed_cleanup_after_cancel_preserves_generation_and_closed_admission`

## Intégrité, clôture et format

Les 2876 fichiers source après exécution conservent hashes, tailles et modes.
L'archive post-source distincte porte SHA-256
`149f87b6ffd3c8c4b3508bfa75568872784e737d958b7e7fe85c6d516ac35b03`.
L'inventaire de fixture compte 96 fichiers réguliers, 54 répertoires, 0 socket,
soit 150 nœuds et 2117508 octets réguliers ; les contenus, tailles et modes
capturés sont vérifiés. Les fixtures et leurs corps restent privés.
Archive privée SHA-256
`0e7a25bebde9329c73cac5d34dee441de519d600f7406dd86ddc628cbdd3a753`.

Les six collectes finales et la clôture retournent 0. Un scan complet à deux
énumérations lit quatre processus/quatre records et n'observe aucune cible ;
il n'est pas atomique. Pic mémoire cgroup 8574226432 octets, oom/oom_kill/
oom_group_kill 0. Le CID exact est absent après stop/remove vérifiés ; le flush
Root séparé lie 522 fichiers et 9 répertoires privés, 83911446 octets. Ce sont
des observations de clôture QA et de synchronisation des preuves, pas une
qualification de coupure électrique ou de restauration.

Le profil observé utilise une image épinglée, 4 CPU, 9 GiB sans swap, 128 PIDs,
UID 1000, root read-only, réseau none, toutes capacités supprimées et NNP.
Le pic mémoire concerne build/tests QA, pas la consommation d'un worker ni
une mesure de capacité 100. Le contrôle du réseau de la QA ne prouve pas le
filtrage réseau métier du produit.

La carte de format courante lie 50 postimages : 47 inchangées issues d'une
campagne historique de 48, 2 positives historiques inchangées, et 1 fixture
Store réellement formatée et vérifiée idempotente puis promue. L'ancienne entrée remplacée
n'est pas créditée au gel courant. Le helper test-only inchangé passe un
check stdin courant épinglé rustfmt 1.10.0-stable, édition 2024,
`skip_children=true`, avec sources inchangées. Ce n'est ni un nouveau
formatage des 50 fichiers ni `cargo fmt --all` réussi sur tout le dépôt.

## Historiques conservés

| Étape | Source et résultat réel |
|---|---|
| R1 | `69767277884cb9316bef98cbae737edeaa3f18dd` : RC101, une E0308 du helper test-only (`&Arc<Store>` attendu pour un `&Store`) ; compilation non terminée, listes/tests/check NOT_RUN,44 NOT_RUN. Ce n'est pas un timeout. |
| R2 | `fa4210c8f1b441dcb1cad2605947f9e28a13ba8f` : build 0,14 listes,9 PASS/2 FAIL/33 NOT_RUN ; check NOT_RUN. Les deux vrais échecs restent datés. |
| R3 | `f117ceae016049b7274708197c3702218148e56b` : build 0,14 listes,44 PASS/check 0 et clôture/flush vérifiés. Aucun PASS R1/R2/B9 antérieur transféré. |

R2 échouait dans deux prérequis de fixture : le second broker n'avait pas sa
propre autorité, puis la réouverture du Store ne recevait pas son vrai Redactor
configuré. La correction test-only fournit ces prérequis ; assertions,
oracles et validations produit sont conservés. Le passage R3 ne transforme
pas ces échecs préparatoires en RED produit et ne réécrit pas leurs reçus.

Le frontend `f33bb623` conserve ses 90 PASS/build 0 historiques ; Node et
navigateur ne sont pas sélectionnés ici. Le gel testé reste en version 0.1.0.
L'alignement 0.0.1 et les travaux statut/environnement des agents sont distincts,
non livrés et non validés par ce résultat ; aucune preuve UI 0.0.1 n'est créée.
Les travaux privés Source16/E2 restent hors compilation/activation de ce lot.

## Reçus privés référencés

Seuls les résultats bornés et empreintes sont publiés ; ni reçus bruts, corps
de fixture, credentials, binaires ou chemins personnels ne sont copiés dans Git.

| Pièce primaire | SHA-256 |
|---|---|
| R3 résultat Root | `3032b35d707889a380b3020bb9881f22168b07d034ba978b2a75e1f4d11566df` |
| R3 flush Root | `dff54fd614a5e6328ddbc9df8adb9efa685b11074d5612b46806174bf4bff840` |
| R3 lignes individuelles | `4be7ac7cd8784254b97e9dc50dcf39516948347a85882808106a4d16372f505b` |
| R3 carte de format | `5ee44b739aab96594b6122f1fe485bbfcd287ede1d2a463f71136e9d60e24a77` |
| R1 résultat Root | `f4f4f2d55de135faf03239a96dab09bf553a8c495fe3bccfd6d09b1d463897bc` |
| R1 flush Root | `83eb4159689a5dd4bbfd1d51aa82bc566fb4b567b4a7aafffaaa1d13a9eaa945` |
| R2 résultat Root | `71d4ca82beb3076c58d18424c3d48a1b2026dee3ccaff25984ac48b92a117a7b` |
| R2 flush Root | `5e441908906d1f92712bbc1fb5ea5c9853e9d7ea3b9f890c69d56a5c75381f54` |

## Limites et qualifications ouvertes

Le positif humain ne porte que sur Store14. Les pages humaines positives
Store15/16 sont NOT_RUN ; types, métadonnées, refus et contrat fermé ne les
qualifient pas. D5/fencing global, reprise froide complète et après perte du
propriétaire, multi-hôtes, transports/réseau métier, workers et moteurs réels
Docker/Podman/Kubernetes, Codex/Claude/fournisseurs, navigateur, capacité 100,
Clippy strict et suite complète ne sont pas exécutés/qualifiés dans ce lot.
SQLite-FULL est une limite logique de base de fixture, pas un disque physique
plein. Aucune fin worker, durée consommée ou libération de quota n'est inférée ;
`billable=false` reste inchangé.

Aucun fingerprint run-1/run-2/couverture des risques n'est clôturé par
assimilation. La cause précise des anciens aborts de pile B9 reste ouverte.
G09 demeure FAIL et tous les gates G01–G12 restent ouverts (0/12 clos).
