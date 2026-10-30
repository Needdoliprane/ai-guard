# Reprise de préparation et création durable — 29 octobre 2026

Périmètre exclusif : `src/audit/control.rs`, `tests/worker_registry.rs`. Aucun autre fichier du dépôt modifié par cet agent. `src/control.rs` inchangé. Dépôt initial sans HEAD et entièrement non suivi conservé, aucun commit/nettoyage.

## Changements et contrat

- `Prepare` accepte `Reserved` ou `InDoubt` seulement avec quota tenu, `create_operation_id`, `container_id`, `created_inspection` et `start_operation_id` absents, et `allocation_inconsistent=false`.
- `Created` accepte `Preparing` ou `InDoubt` seulement avec quota tenu, `create_operation_id` présent, `container_id`, `created_inspection` et `start_operation_id` absents, et `allocation_inconsistent=false`. Validation syntaxique et correspondance exhaustive de l'inspection existantes conservées : autorité, runtime, nom stable, volume, image, spec, CPU/RAM/PIDs, réseau, preset et restart policy.
- Les deux transitions effacent `unknown_reason` et conservent identité, spec, tentative, réservation et tous les claims. Aucune autorisation de réémettre create/start n'est introduite. Le CAS et les opérations idempotentes restent inchangés.
- La relecture du journal applique exactement les mêmes fonctions de transition puis compare le snapshot complet attendu, sans observation ni allocation associées. Un événement simplement renommé prepare/created n'est plus une transition admissible.
- Le caller reste responsable du verrou OS contre un ancien émetteur, de l'identité tenant/origin, de l'archive/manifest immuables, de la propriété/configuration/labels runtime, et du snapshot réellement `Created` sans démarrage antérieur avant de demander `Created`. Le DTO d'inspection seul n'exprime pas les dates runtime.

## Preuves exécutées

Toolchain constatée : `rustc 1.99.1 (58cf8f9c3 2026-10-13)`, `cargo 1.99.1 (5f94df478 2026-08-27)` ; PATH explicite et `login:false`.

- RED constaté : `cargo test --locked --test worker_registry`, 21 PASS et 3 échecs attendus, reprises InDoubt refusées avant changement, replay action prepare forgée accepté. Log `/tmp/ai-guard-worker-prepare-recovery-red.log`.
- Le test replay initial attendait à tort un Err de `Store::open`. L'API ouvre pour diagnostic avec `healthy=false` ; assertion corrigée à cette frontière réelle, puis bloc replay retiré temporairement et rétabli. RED corrigé constaté avec `cargo test --locked --test worker_registry projection_replay_rejects_prepare_and_created_records_without_their_exact_transition`, 1 échec attendu `healthy=true`. Log `/tmp/ai-guard-worker-prepare-recovery-replay-red.log`.
- PASS : `cargo test --locked --test worker_registry --test usage_ledger --test control_migration`, 24 + 2 + 4 = 30 PASS, 0 échec, 0 ignoré. Log `/tmp/ai-guard-worker-prepare-recovery-green.log`.
- PASS : `rustfmt --edition 2024 --check src/audit/control.rs tests/worker_registry.rs`.
- FAIL hors fichiers possédés : `cargo clippy --locked --lib -- -D warnings`, 3 avertissements devenus erreurs dans `src/runner/managed.rs`, needless_borrow ligne1357 (client, state) et collapsible_if ligne1618. Transmis à runner_recovery ; aucune correction hors périmètre. Log `/tmp/ai-guard-worker-prepare-recovery-clippy.log`.
- FAIL global observé : `CARGO_INCREMENTAL=0 cargo test --locked --no-fail-fast`, unique cible échouée `--lib`, test `runner::managed::tests::attempt_guard_rejects_concurrent_owners_substitution_and_linked_files`, ligne374, répertoire temporaire sans mode0700. Toutes les autres cibles ont fini PASS, y compris registry24, runner_recovery3 et runtime_managed6. Trois tests moteurs réels ignorés, donc NOT_RUN. Défaut transmis au propriétaire runner. Log `/tmp/ai-guard-worker-prepare-recovery-suite.log`.

## Critères acceptés et limites

PASS logiciel ciblé : reprise avant create depuis Reserved/Preparing interrompus ; reprise du conteneur trouvé après claim create et ACK perdu ; identité/spec/attempt/quota inchangés ; reason effacé ; ancien expected_revision refusé par ControlFailure::Conflict ; inspection divergente refusée ; claims antérieurs/conteneur déjà enregistré refusent le retour à Preparing ou Created ; claim start récupéré une seule fois, replay après redémarrage AlreadyConsumed ; replay audit prepare/created invalide verrouille le Store ; aucun usage fictif créé.

Le mapping HTTP409 des anciens CAS relève de l'intégration API (demande transmise à root). Les helpers n'effectuent aucune opération moteur ; effets réels Docker/Podman, arrêt brutal, cloud et qualification de release NOT_RUN par cet agent. Aucun secret affiché, volume supprimé ou service distant manipulé.

Revue indépendante demandée à root sur les deux fichiers et requise avant clôture intégrée. Les trois avertissements Clippy et l'échec du nouveau test guard sont des écarts de l'édition parallèle, pas des PASS. Relancer les checks globaux après leur correction.

Les skills projet `.agents/skills/ai-guard-*` et `.claude/skills/ai-guard-*` sont absents ; chargement automatique non confirmé. Contrats du dépôt lus ; skills systematic-debugging, test-driven-development et verification-before-completion appliqués.
