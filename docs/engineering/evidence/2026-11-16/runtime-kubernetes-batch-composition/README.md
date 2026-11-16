# 16 novembre 2026 — R2 public runtime/Kubernetes : résultats de composition

Le candidat **R2 actuel**, distinct de R1 et des gels standalone, donne **26 PASS / 0 FAIL / 0 BLOCKED / 0 NOT_RUN**, une invocation exacte par cas. Root a revérifié les 26 sorties stdout/stderr/RC, les 2 925 sources et les deux binaires inchangés. Format et Clippy toutes cibles passent ; les **505 diagnostics d'avertissement Clippy restent non stricts**. Cette preuve est logicielle, sans nouvel essai moteur ni mesure de durée physique d'arrêt.

## Source R2 et preuves finales

| Repère | Valeur |
|---|---|
| Base publique | `987162f65b65ce179cd95dbb7698dd67e827e35a` |
| Composition | 18 chemins Rust (10 batch, dont les deux anciens oracles RED, et 8 Kubernetes pré-effet), plus 4 guides EN/FR |
| Archive R2 actuelle | `daa20fcb03ccdcdae57c68ba1bf646f3208d158f06bc113458ff4e44c23b9d66` |
| Volume | 2 925 fichiers, 36 888 064 octets |
| Index R2 | `351893433a8c0a443825e98b1e42715d22aedd9b13852b523bf4ce607f809910` |
| Gel R2 | `7be5df1a41a21af00c9439d756d376ba1feee58ff0d0b8280eded593450e336c` |
| Revue du delta R1 → R2 | `02541918beb6ace023c23262b73e0a36d057bb67b98ef5d3580e6eaced065f3a` |
| Stage A final additif | `55a7409b96119ed44f11508c1ab41ca71405b8f839fb972f6b5a9d5d8976a8af` |
| Phase B R2 | `9764f1d92f5fd7cab780d9499f6cd065a2d5bcfe31599d594aa078e9b4757bc0` |
| Recontrôle final Root | `85c446fd2ad81f2c8052103275b1d5d9431914e864d26c427325659e5d6b4cb5` |

R2 corrige le `never_loop` de R1 par un `match`, place le témoin privé affine201 dans une Box et retire l'import inutilisé. Quatre chemins Rust changent ; les 2 921 autres fichiers et leurs modes restent exacts, dont les deux anciens oracles RED. La source privée Native17 complète n'est pas importée par cette composition. Les présentes mises à jour documentaires restent un lot séparé du gel QA testé.

| Étape R2 | Résultat admis |
|---|---|
| Format ciblé des 18 chemins Rust | **RC0** |
| Clippy toutes cibles | **RC0**, 505 diagnostics d'avertissement, 0 erreur ; **non strict** |
| Build sans exécution, libtest + `runtime_managed` | Première tentative **RC130, blocage de ressources** conservé ; reprise admise **RC0**, deux artefacts neufs |
| Listes des deux cibles | **26 sélections actives uniques**, aucun helper ignoré crédité |
| 18 runtime + 4 Kubernetes + 4 intégrations | **26 PASS**, une invocation par cas, zéro relance |
| Recontrôle Root après exécution | 26 sorties brutes/RC, 2 925 sources et deux exécutables inchangés |

| Binaire R2 après exécution | Taille | SHA256 |
|---|---|---|
| Libtest `agentsandbox` | 308 665 408 octets | `7c13e83dc12e4d463e06f62966724dc785e8eaf35a5374e40830820dd4f73ddf` |
| Intégration `runtime_managed` | 15 494 640 octets | `244700058eb6816e34c127f9eef827e29770437a38f61385950f8f71a4a48f42` |

## Reprise du build R2 et commandes constatées

La première tentative sans exécution n'a produit aucun artefact de test ni `buildFinished` positif ; son **RC130 pour ressources** reste attaché au reçu `b39b70209f5d085a8c74fe0e58a466583354cedc873189a5451a690f2cb4ce14`. Une reprise unique est admise après récupération ciblée de **4 562 fichiers de cache de campagnes QA terminées**, avec **9 196 entrées protégées revérifiées et conservées** (reçu `d0849555b377c65e971144a8b8c2af000f6475747604ca5e25811d7593fc991e`). Aucune source, preuve, donnée métier ou identité n'est supprimée par ce nettoyage borné. La réserve observée avant la reprise est de **8 368 017 408 octets libres** ; cette observation ne constitue pas une garantie de capacité ni un gain causal exclusif.

La reprise consignée utilise `cargo test --lib --test runtime_managed --no-run --locked --offline --message-format=json -j1`, avec les mêmes profils dev/test : opt0, debug0, assertions et overflow checks actifs, 256 unités de codegen, incremental désactivé. Seul le parallélisme d'orchestration passe de `-j2` à `-j1` ; code, epochs, deadlines et profil sont identiques. L'essai interrompu n'est pas réécrit en succès.

Les cas R2 sont lancés en série avec `--exact --test-threads=1 --show-output`, une fois chacun, sous une limite de processus de 180 s par cas. Cette limite du harnais ne modifie aucun délai produit. Les arguments décrivent les invocations enregistrées, sans nouveau tutoriel de commande ni autorisation de relance automatique.

## Sélection R2 — 26 PASS sur ses propres artefacts

Les processus et pipes des fixtures Rust sont réels ; cela reste une preuve logicielle, sans moteur Docker/Podman. Les helpers ignorés ne sont pas comptés comme cas. Les échecs des deux anciens oracles sur le baseline antérieur demeurent historiques.

### Batch runtime — 18 PASS R2

- `runtime::deadline_supervision_tests::deadline_remains_pollable_while_observation_pending`
- `runtime::deadline_supervision_tests::stop_remains_pollable_while_callback_pending`
- `runtime::batch_io::tests::read_error_preserves_partial_capture_and_does_not_invent_eof`
- `runtime::batch_io::tests::bounded_capture_keeps_draining_until_actual_duplex_eof`
- `runtime::batch_io::tests::setup_fault_collects_real_child_and_retains_exact_original_input_fd`
- `runtime::batch_io::tests::successful_io_returns_original_archive_for_the_callers_global_result`
- `runtime::batch_io::tests::ordinary_cli_uses_its_own_two_capture_limits_in_the_same_owner`
- `runtime::deadline_driver_tests::deadline_progresses_stop_during_same_callback_and_preserves_late_causes`
- `runtime::deadline_driver_tests::checked_blocking_keeps_control_pollable_and_allows_second_real_pool_check`
- `runtime::deadline_notification_tests::failed_started_notification_cannot_be_reissued_by_final_reconciliation`
- `runtime::deadline_notification_tests::engaged_notification_keeps_the_same_local_callback_until_its_result`
- `runtime::managed::deadline_semantic_tests::exact_native_terminal_before_pending_callback_disarms_elapsed_deadline`
- `runtime::managed::deadline_semantic_tests::actual_cli_exit_with_pending_inspection_keeps_work_deadline_active`
- `runtime::managed::deadline_semantic_tests::selected_deadline_survives_later_exact_terminal_proof_and_pending_callback`
- `runtime::managed::deadline_semantic_tests::terminal_proof_retains_late_callback_error_and_exempts_only_real_proof_from_fallback`
- `runtime::managed::deadline_semantic_tests::direct_native_created_receipt_binds_its_weaker_proof_and_disarms_deadline`
- `runtime::managed::deadline_semantic_tests::contradicted_terminal_shortcut_requires_the_first_real_native_reconciliation`
- `runtime::managed::deadline_semantic_tests::performed_native_reconciliation_is_not_retried_after_a_later_contradiction`

### Kubernetes pré-effet — 4 PASS R2

Les contrats vérifient le témoin201, la cible d'origine, les octets retenus et les refus avant effet ; pas un serveur d'API ou un cluster réel.

- `runtime::kubernetes::session_effect::tests::create_201_response_witness_is_affine_and_bound_to_the_original_target`
- `runtime::kubernetes::session_effect::tests::non_201_or_non_create_http_reply_never_produces_a_create_response_witness`
- `runtime::kubernetes::session_effect::tests::create_posts_exact_retained_bytes_once_and_only_a_validated_201_retains_an_instance`
- `runtime::kubernetes::session_effect::tests::refused_guard_cancellation_or_changed_anchor_sends_zero_effect_requests`

### Intégration runtime_managed — 4 PASS R2

- `managed_launch_requires_created_ack_and_keeps_owned_evidence`
- `approved_program_changes_and_unapproved_permissions_fail_before_execution`
- `approved_invocation_clears_ambient_environment_and_uses_control_cwd`
- `trusted_anchor_executes_approved_leaf_after_parent_is_replaced`

## Contrat logiciel couvert et limites du batch/Kubernetes

Le batch garde STOP et l'échéance pollables pendant l'inspection et la notification exacte. Les owners Child/IO sont conservés et drainés. Les preuves natives Direct et Managed ont des portées distinctes : seule l'observation terminale native validée ferme l'échéance WORK ; la sortie du CLI ne la remplace pas. Audit, callback et IO restent obligatoires après ce terminal, avec première cause et erreurs retenues. Un résultat inconnu ne devient pas un ACK ni une permission de relance. Aucun arrêt temps réel en une seconde, budget global, variante interactive/préarmée ou annulation externe n'est qualifié par ces cas.

Kubernetes exige une réponse CREATE201 exactement correspondante avant une cible typée non duplicable. Ce témoin est une provenance HTTP, pas un état Running ni une preuve Namespace/cluster/RBAC. Les effets de production S5 et le chemin Native17 C2 restent fermés.

## Historique R1 et standalone — résultats conservés, sans transfert

| Source antérieure | Résultats et portée |
|---|---|
| **Composition R1**, archive `ea8a4339c6e3c4021116731e03cc414540a5028169badd040fc6fc17a637bb53`, 2 925 fichiers | Format/check/build/listes RC0 puis **26 PASS**, avant **Clippy FAIL RC101** sur la cause `never_loop` ; 475 diagnostics d'avertissement. Ce n'est pas le bilan final du R2 actuel. |
| Batch standalone, source `fc9fd83c36bf91c40dcdceee325be4369616fb7b9fba460409b731dee9ce6685`, 2 919 fichiers | Format/build RC0 puis **18 PASS** ; libtest `6aeccb8da15cdc28378074802da1b1f53d2e3557ae51ba2009bfed6f316423e4`. |
| Kubernetes standalone, source `e147f291c3583e48da777712cf376b090f84fd82be0aea793b4fc9a5cafd26e0`, 3 407 fichiers | Format/build RC0 puis **4 PASS** ; libtest `4f6aa1ca0905e4561fed1eafed07bfebad6a154990ba84325a4ad4433c48a79e`. |

Les 26 cas R1 sont consignés dans `24841bdbd0d2bf4e07296e370c9a9a31766e25481061d767b3e0feafcdcf3353`, revérifiés par Root dans `92e87f2e7a28f86c9ba327b4364a35b651af4e3acb10d6bd91dad1ab080f3eb0`. Le Clippy R1 en échec est conservé dans `29090d133d9844495a503f3300b0ccbca8718ef1e0ad189b0956d9544a3c1a0b`. **475 et 505 ne mesurent pas une amélioration ou une régression des avertissements** : R1 s'était arrêté avant toutes les unités de compilation.

Le reçu Root `12251c45d21a2e6a7de9d4216cd2599d407c0589731d662c91be4ad5cd572d1f` vérifie les 22 sorties/RC des deux gels standalone. Aucun de ces PASS n'est transféré à R1 ou R2 : chaque composition possède ses nouvelles invocations et artefacts. Les preuves et gels antérieurs restent intacts ; les pièces brutes et chemins privés ne sont pas publiés ici.

<a id="native17-metadatafd-failure"></a>

## Native17 MetadataFD — échec distinct

Ce diagnostic porte sur une autre source : `6e3aa88f691b0e98e21794f11f84f709519a430b7f3f7bd031cd46a9d3e6b943`, 3 408 fichiers. Il n'est ni une dépendance reprise ni une qualification du batch/Kubernetes public ci-dessus.

- Build libtest **RC0** ; contrôle de format **RC1**, différence de mise en forme conservée.
- Un seul oracle positif exécuté : **FAIL RC101**, `human_authority_epoch_expired` ; aucune relance. Quatre cas dépendants restent **NOT_RUN**.
- **2 331 ms** mesurent `incarnation_append17`. Le processus dure 28,196 s ; libtest rapporte 26,34 s. Ces durées ne désignent pas la même horloge.
- Le probe COMMIT est marqué, mais `full_post` reste incomplet ; la rétention complète n'est pas établie. Aucune correction n'est déclarée fermée et aucun gain de durée n'est déduit d'essais/sources distincts.

Reçu d'exécution : `e48dfdf8f4b602661ce5bbc4c4186d419d322fe3ed569fc0fe8292cb9682a0c5`. Recontrôle Root : `2e932b4757e62b5051bf4c6708dded7b17ac681905b6f1aedf67c43aa1b69df8`. Les échecs R6/R11 et les autres résultats datés restent liés à leurs propres sources dans le [relevé des échecs de démarrage/runtime](../startup-runtime-failures/README.md).

## Audit D1 run-2 — rapprochement repris, sans clôture

Le chantier ciblé du [rapport run-2](../../../AUDIT_RUN_2_20261110.md) a repris pour le fingerprint `control-monitor-prestart-failure-unmonitored-start:src/runner/control_monitor.rs:run`. Le rapprochement avec la source publique `ea8a4339…` est en cours. L'ancien canari **RC134 avant l'oracle** reste conservé ; il ne constitue pas une reproduction admise du constat. Aucune nouvelle reproduction ni correction validée n'est établie ici. Root renseignera séparément le registre d'audit au moment approprié ; ce relevé ne clôt ni ne requalifie le fingerprint.

## Limites maintenues

Aucun gate ni fingerprint d'audit n'est clos par ces 26 résultats logiciels de composition R2 : **0/12 gates clos**. Le vrai caller/Store A06 et la faute OS tardive IO-F01 complète sont des propositions séparées, non incluses. Native17, les effets S5, l'autorisation de production, le cluster/Namespace/RBAC/CNI/stockage, les moteurs/workers/fournisseurs réels, la capacité100 et la qualification de bout en bout restent hors de ces preuves. Le précédent échec Docker demeure un échec historique ; les tests logiciels de supervision ne le transforment pas en mesure d'arrêt réussie.
