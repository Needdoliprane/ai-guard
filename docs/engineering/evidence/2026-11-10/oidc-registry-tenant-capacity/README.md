# Registre OIDC — capacité native par tenant, 10 novembre 2026

**Le vecteur logiciel ciblé est corrigé sur le gel identifié.** Douze parents
FIX uniques passent, suivis du check toutes cibles. Cela ne clôture pas D5 entier,
une qualification SSO/runtime, un SLO ou un gate global. G09 FAIL,0/12 clos.

## Source et contrat

Le fingerprint run-2 est `api/oidc-start-registry-global-slot-exhaustion`.
La preuve historique sur696efd1 insérait256 transactions directement, sans
256 requêtes HTTP observées. Public81 avait déjà séparé224 anonymes/32 natifs,
fenêtres8+8, total256/16 et principal4/2. Le nouveau résidu concerne des
propriétaires natifs vérifiés A qui privent le refresh vérifié de B.

V4 résout les IDs du plan durable trié depuis les memberships positifs vérifiés,
sans index GatewayState ni champ HTTP/JWT non vérifié. L’opération conserve son
propriétaire dans PendingRefresh et NativeIntent, orphelins compris. Les caps
tenant max(1,32/N) / max(1,8/N) restent sous les globaux existants ; au-delà de
N32/8, les minima1 ne sont pas des réserves simultanées pour tous. A+B peut
employer ses deux parts. Snapshot comptable sans droit ajouté, V3 mono explicite,
premier login encore anonyme. Aucun changement de ledger, service ou stockage.

[Registre](../../../../../src/api/oidc/registry.rs),
[gateway](../../../../../src/api/oidc/gateway.rs),
[tests HTTP/owners](../../../../../src/api/oidc/admission_registry_tests.rs),
[guide EN](../../../../setup/sso/README.md#login-and-native-refresh-admission),
[guide FR](../../../../setup/sso/README.fr.md#admission-des-connexions-et-refresh-natifs).

## Baselines : observation et résultat du parent restent distincts

R1 `8c7677ba…` compile mais échoue à sa précondition :32 acceptations cumulées,
17 propriétaires avec B au lieu de33. R2 `a3cba155…` utilise quatre vagues de
huit HTTP distincts, passages réels et gardes live ; aucun owner n’est injecté
ou rajeuni. Le setup224 status+start remet sa seule fenêtre privée à zéro :
ce n’est pas une mesure de224 starts à cadence physique.

R2 observe32A vivants/B429 capacity_exhausted, audit inchangé110 records/95020
octets, séquence110. Avant/après B : marges732/581ms, fenêtres1057/1208ms avec
deux départs natifs. La fenêtre est disponible ; les slots32 sont contrôlés
avant next_window. Le total256 est aussi atteint : ces deux caps ne sont pas
causalement séparés par ce cas. Quatre finish sont200, puis401 ; parent
**NONCONCLUSIVE/RC101**, reopen et assertion RED finale non atteints.
La revue indépendante `40f7ac61…` confirme l’observation préalable, pas une
RED finale ou un PASS. Aucune troisième baseline identique n’a été relancée.

## FIX : douze fonctions exactes sur une source

Base publique `2cc29ee42ae3eb3cc8203538bf7a09aee2e26b57`, incluant SetupV2,
plus les six chemins OIDC. Manifeste2378 :
`a5511b0b0dcd9e51080861206b773f1a2b584faaddeb4ded0409f56239d87322`.
Postimages6 : `5f3a438393161a0f5fe26646326c5b0a143ee9cc37bfa5a49fca0e0268ff710b`.
Patch contre public2cc, préimages inchangées depuis public81 :
`cc95f7a7e1c97c21a8a1a4bc6728c563958ae82b34740debfc3e2e069c5f882c`.
AuthCore, Setup, main et le reste du source métier publié sont préservés.
Les ajouts de preuve/guides sont des deltas documentaires séparés de ce gel QA.

Build bibliothèque RC0 19:15:46–19:18:52UTC ; listing RC0. Chaque sélection
suivante a1PASS/RC0,0ignored, une seule exécution comptée :

| Groupe | Sélecteur court exact |
|---|---|
| Nouveau, slots A/B | `disjoint_tenant_native_owners_cannot_take_b_refresh_capacity` |
| Nouveau, fenêtre A/B | `disjoint_tenant_rate_cannot_take_b_verified_window` |
| Nouveau, orphelin | `orphan_native_intent_keeps_the_same_verified_tenant_until_audited_finish` |
| Nouveau, ordre des IDs | `reversed_durable_plan_order_resolves_verified_membership_ids` |
| Nouveau, clés | `tenant_keys_are_explicitly_resolved_by_id_in_sorted_plan_order` |
| Nouveau, caps | `finite_tenant_caps_preserve_global_budgets_and_document_oversubscription` |
| Classe de fenêtre | `one_sided_rate_eight_refuses_ninth_without_borrowing_the_empty_class` |
| Principal/session | `per_session_and_principal_rate_reject_without_a_second_durable_admission` |
| Owner entre phases | `pending_owner_bridges_intent_absence_until_real_verified_rotation_commits` |
| A+B/principal4 | `active_owners_preserve_both_slot_partitions_and_principal_four_without_loans` |
| ACK/logout | `lost_ack_at_each_real_ledger_boundary_replays_the_same_three_facts` |
| ACK FRefresh avant Logout | `durable_refresh_before_first_logout_must_reconcile_without_a_retroactive_logout_begin` |

L’index privé `ROOT_FIX12_INDEX.json` porte les noms complets, UTC et hashes
des raws, SHA `d1eeede0b03fc1c23c32e9c5658dbc6b0880231b98103e096e1ac8317f816b6d`.
Le primaire offre32 requêtes A :16 propriétaires A vivants, B200 et son vrai
owner/reçu,17 finish200 après l’observation puis zéro owner/reopen exact.
Deux finish positifs avant pression sont distincts des17. Audit94→95 à B :
ce sont une intention de refresh admise puis une terminaison contrôlée, pas
une rotation fournisseur réelle. Le total16 seul ne prouve pas4/tenant/fenêtre :
le canari de débit séparé passe sans rollover masqué.

Check Rust toutes cibles RC0 19:30:01–19:31:16UTC,10 avertissements conservés ;
aucun Clippy exécuté dans ce lot. Le PATH réel du parent11 omet `/usr/bin`.
Root vérifie `/bin`→`/usr/bin` dans l’image RO : seul un alias de répertoire
dupliqué est retiré. Exception explicitement acceptée sans rerun ; commande,
index et raws réels sont conservés, sans prétendre une commande littéralement identique.

## Conservation et portée

Post2378 hashes/pathset exacts, cache4 conformes et nonregular0. Archive réelle
post :2378 fichiers réguliers,24514560 octets, SHA
`2252272fdb9bfc4189f21ec0c4f4bd7420b1198e1ddadf9fe5d9da89ccf15f35`.
Index post-source `9751349a4703710caf525a4172bbe750aa81d1b40c7e4bdc996a9f760ec2acbb`.
QA idle arrêtée puis retirée19:33:52.716848793Z ; PID0/exit137/OOMfalse concerne
l’arrêt de son PID1 inactif, pas les tests RC0. Watch terminé normalement.
Clôture privée SHA `2a2dd28b8b6a37be8ae30c0f6f9d0bf61227378f22191f8a945852a0af243c00`.
Les anciennes QA sont arrêtées/retirées ; le post-final R2/STAGES avant watch
est **NOT_CAPTURED**. Archives R1, source R2 gelée et raws sont conservés ;
aucune archive ou réussite finale baseline n’est inventée.

Trois métiers inchangés depuis revue `01c8c8b0…`, harnais commun R2 relu
`864c65e8…`. Rapports, commandes admises, preuves et manifests restent privés,
hors distribution, dans les lots oidc-registry du10novembre. Les liens et
statuts publics sont expurgés. Vérification logiciel Rust/SQLite/HTTP contrôlé :
pas d’IdP réel, SSO déployé, worker/runtime Docker/Podman/Kubernetes,100workers,
SLO ou équité générale qualifiés. D5 entier et les gates restent ouverts.
