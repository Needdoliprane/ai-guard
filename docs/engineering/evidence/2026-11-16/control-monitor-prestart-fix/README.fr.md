# D1 préstart — correctif logiciel borné, 16 novembre 2026

[English](README.md) · [Registre d’audit](../../../AUDIT_RUN_2_20261110.md) · [Validation](../../../VALIDATION.md) · [Gates de production](../../../PRODUCTION_READINESS.md)

## Résultat et frontière de source

**14 tests exacts PASS**, une invocation chacun : quatre vrais CLI ai-guard/Gateway/SQLite, deux unités Managed/PTY, trois voisins PTY et cinq deadline. Le CLI est le binaire Cargo ai-guard `agentsandbox`, **pas Codex CLI ou Claude Code**. Seul le moteur est une fixture Rust existante ; aucun daemon Docker/Podman réel, appel fournisseur ou cluster n’a été exercé. Trois exécutables et les 2 932 fichiers indexés sont revérifiés ; Root a contrôlé les 42 fichiers de sorties/RC.

La source testée contient les huit fichiers Rust D1 finaux exacts et deux fichiers D7 cfg(test). **Ces deux fichiers D7 sont exclus du commit d’intégration D1** ; aucun correctif produit D7 n’est inclus. Ces registrations de tests ne qualifient pas D7. Les huit chemins D1 sont :

- `src/runner/control_monitor.rs`
- `src/runner/control_monitor/prestart.rs`
- `src/runner/managed.rs`
- `src/runtime.rs`
- `src/runtime/managed.rs`
- `src/runtime/managed/legacy_completion_tests.rs`
- `src/runtime/terminal.rs`
- `tests/audit_run2_d1_prestart.rs`

La revue source brute et la préparation de formatage seul ont leurs reçus distincts. Base d’intégration actuelle : `961045788784b277a7a9eaa5e8f411275d7147e0`. **Clippy D1 : RC0 non strict, 507 émissions d’avertissement et 0 erreur**, sur la même source composée `74d60de1…b7b`. SHA-256 du reçu : `dc43ad33095e9b30353ca6ed8424c62f9157dbc8a4da0cf4433f506b31e0e0a2`. Le gate strict reste ouvert. Aucun résultat de suite complète ou Clippy strict n’est revendiqué.

## Changement produit

Le premier GET de contrôle doit réussir et être validé avant readiness. En cas d’échec, le même propriétaire privé révoque l’admission avant d’attendre STOP et conserve runtime, client, plan et FD/flock de tentative originaux. Le veto restreint l’autorité existante du premier start ; il ne crée ni nouveau grant d’exécution, ni Current Native17, ni capacité préarmée. Il est consommé au vrai spawn après les derniers contrôles. La préparation TTY suit le chemin `checked_blocking` joint pour laisser le moniteur pollable ; le veto vient après les préparatifs PTY/stdio/pre-exec, immédiatement avant spawn.

Le terminal natif connu, les captures IO et la cause initiale sont conservés au lieu d’être effacés par une erreur de contrôle ultérieure. Seule une vraie completion fallback de la même opération peut atteindre une fois le finalizer existant ; Created, observation incomplète ou contradiction restent incertains. La sortie d’un CLI n’est pas à elle seule une preuve terminale. La **fenêtre post-ACK de 60 secondes est inchangée**, sans tolérance ajoutée avant ACK ; ce n’est pas une borne mesurée d’arrêt physique.

## Observations baseline et corrigées

La [baseline précédente](../control-monitor-prestart/README.md) reste immuable. Son canari C1 passe. La faute préstart 429 échoue ensuite RC101 : la fixture moteur Rust observe `starts=1` au lieu de zéro ; Root vérifie séparément `audit_valid=true`. Les assertions situées après ce premier échec ne sont pas atteintes et ne sont pas rétrospectivement créditées.

Sur la source corrigée, la même faute préstart passe avec `starts=0` et `audit_valid=true`. La faute au dernier contrôle d’autorité observe aussi zéro start. Le contraste après start/avant second ACK observe `starts=1`, un STOP natif, un callback committé, la sortie du propriétaire et un audit valide. Une erreur de contrôle n’efface pas ce résultat réellement connu.

| Phase | Résultat observé |
|---|---|
| Canari c60 | PASS ; un start légitime, arrêt exact, audit valide |
| Faute préstart c60 | RED RC101 ; un start après contrôle refusé ; audit valide ; assertions suivantes rétention/quota non atteintes |
| Faute préstart corrigée | PASS ; zéro start, audit valide |
| Faute corrigée au dernier contrôle | PASS ; zéro start, audit valide |
| Contraste corrigé après start | PASS ; STOP natif, callback committé, propriétaire sorti, audit valide |

## Sélection exacte exécutée

| # | FQN | Résultat |
|---|---|---|
| 1 | `canary_cli_owner_enforces_durable_stop_without_prestart_fault` | PASS |
| 2 | `prestart_read_failure_refuses_cli_start_and_retains_audited_owner` | PASS |
| 3 | `prestart_failure_during_final_authority_check_refuses_first_start` | PASS |
| 4 | `control_failure_after_start_before_started_ack_preserves_terminal_known` | PASS |
| 5 | `runtime::managed::legacy_completion_tests::late_observe_failure_keeps_real_fallback_completion_and_original_cause` | PASS |
| 6 | `runtime::terminal::tests::final_veto_refuses_after_pty_preparation_before_transport_spawn` | PASS |
| 7 | `runtime::terminal::tests::guard_explicit_restore_and_drop_restore_presentation_and_attributes` | PASS |
| 8 | `runtime::terminal::tests::task_cancellation_restores_real_pty_attributes` | PASS |
| 9 | `runtime::terminal::tests::reaped_transport_cannot_hide_a_filter_failure_or_incomplete_control_in_its_tail` | PASS |
| 10 | `runtime::deadline_supervision_tests::deadline_remains_pollable_while_observation_pending` | PASS |
| 11 | `runtime::deadline_supervision_tests::stop_remains_pollable_while_callback_pending` | PASS |
| 12 | `runtime::deadline_driver_tests::deadline_progresses_stop_during_same_callback_and_preserves_late_causes` | PASS |
| 13 | `runtime::deadline_driver_tests::checked_blocking_keeps_control_pollable_and_allows_second_real_pool_check` | PASS |
| 14 | `runtime::managed::deadline_semantic_tests::terminal_proof_retains_late_callback_error_and_exempts_only_real_proof_from_fallback` | PASS |

Les quatre premières lignes utilisent la cible d’intégration `audit_run2_d1_prestart` ; les dix autres, le binaire de tests bibliothèque. Les argv exécutés suivent `--exact <FQN> --nocapture --test-threads=1`, pas un client fournisseur. Les cas réutilisent les mêmes artefacts de stage A ; leur invocation ne prouve pas un nouveau build. Le reçu préparatoire vérifie format, check et build des tests sur cette source. Une invocation Clippy distincte a utilisé `--all-targets --locked --offline --message-format=json -j1` avec les mêmes profils explicites ; elle n’a pas relancé les tests. Six émissions concernent quatre positions des fichiers D1. Les deux émissions `collapsible_if` de `runtime/managed.rs:1753` sont nouvelles ; les quatre autres émissions D1 sont déjà présentes dans la préimage. Le `await_holding_lock` ancien appartient à un test de commit bloquant dont la causalité est conservée, pas à une nouvelle attente de production. Cette dette reste ouverte. Aucun résultat Clippy strict ni nouvelle exécution des tests n’est revendiqué.

## Reçus

Les originaux privés restent hors Git. Les noms et empreintes identifient les preuves sans publier rapports privés, chemins, credentials ou logs bruts.

| Pièce | SHA-256 / portée |
|---|---|
| Fixed composed source / Source composée corrigée | `74d60de1c9348615b7c1a34bf8df55fa91bf5448b6c0b1d2197ef3aef0b02b7b` — 2 932 files / fichiers |
| D1 result receipt / Reçu des cas D1 | `7aaee770698c99434e0920e16e8d11d8ebf92da22f50ae44b3ab647a03595526` |
| Root result verification / Vérification Root | `e2dea73b8bd75d05a86f19fc4855518c7e110c8dc9c4e7ac9b9fa794bbe45145` |
| Root preparatory format/check/build verification / Vérification préparatoire | `884abb68dd2f8a6cec1c6881d7873933f8757104b3c35e4ac5368d5c35c3dcd8` |
| Raw D1 R2 source review / Revue source brute D1 R2 | `ed778d68f67fe3949e3eb8a67716448e0806eded418417aecb23c9aa8279aaab` |
| Baseline c60 source / Source baseline c60 | `c60b0cb088491416bcfb4ca09936a4c81d99020b737145004f6794cbae24cc53` |
| Baseline canary C1 / Canari C1 baseline | `c8cf72d5308f5f42d17bcd3c0efcb2a8d5a8cb59a11999fb78dd789ec1207cc0` |
| Baseline fault + Root verification / Faute baseline et vérification Root | `579bca8cf2b24d2ada7ab4cc6d579f8b0ebde6d3a1a7e6c99225c556e59457be` ; `369bb8cb42b6a2512ae84165b0e6d3750f082bfd21314efe8be96442a07721ab` |

## Limites et travaux ouverts

- L’unité Managed de faute tardive vérifie le vrai producteur de completion et la récupération affine avec sa cause originale. Elle **ne démontre pas la faute exacte à travers runner → finalizer Store → quota** ; cette injection de bout en bout reste ouverte.
- Le helper PTY négatif et les trois voisins vérifient les primitives PTY/RAII logicielles locales. Vrai TTY de l’utilisateur/session fournisseur, autres plateformes et durée physique d’arrêt runtime ne sont pas qualifiés.
- Abandon/panic/annulation externe et reprise froide/fencing gardent leurs exigences ouvertes ; Drop du propriétaire ne prouve ni STOP asynchrone ni completion terminale.
- D7 reste ouvert, avec deux phases distinctes. R1 s’arrête au prérequis de setup TMPDIR/chemin UDS avant l’oracle (`6420d89778e218f4efcec5be3550c332934aacbbcf72fe3643ace36d3fe6e84e`) ; cet échec non concluant reste conservé. R2 ne change que cette configuration de harnais : cap2 passe RC0, puis Restore cap1 atteint la vraie assertion de couverture ACK sans STOP/Cancel et échoue RED RC101. Le troisième parent reste NOT_RUN. La vérification Root `5634d05010bfc45376a9420ee62f3697dd22d7fcdbb8d05ac4ee498c0314c487` épingle le résultat `f712a664fcfa4027bf48cf1792ae8b4c523ad2fced32290f992cf92f6a25b971`. Ces observations D7 ne s’additionnent pas aux quatorze PASS D1 ; aucun correctif produit D7, preuve de moteur réel ou nouvelle sévérité n’est revendiqué.
- Aucun parcours fournisseur Codex/Claude, Docker/Podman/Kubernetes réel, cluster, capacité de 100 workers ou release production n’est qualifié. Verdict et sévérité du rapport primaire inchangés ; contribution logicielle au seul fingerprint D1. **0/12 gates de production clos**.
