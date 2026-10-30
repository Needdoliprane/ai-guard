# Lot runtime — sorties moteur et compatibilité des reprises

Date : 30 octobre 2026. Périmètre logiciel ciblé, sans moteur ni fournisseur lancé par cet agent.

## Besoin et réalisation

Le nouveau broker crée les workers sous `fixed_sandbox_v2`. Toute création, worker ou helper, passe `--log-driver none` à Docker et Podman. Les arguments de commande restent séparés du programme, sans shell hôte. Les helpers ne reçoivent aucun credential ; le helper d'import garde seul stdin ouvert pour lire le tar. Les workers et exports restent batch, sans TTY ni stdin ouvert.

Avant un premier start, le runtime vérifie `HostConfig.LogConfig.Type=none`, aucun `Config` non vide ni option auxiliaire active. Les champs Docker et Podman sont traités explicitement : Podman peut exposer `Path`/`Tag` vides et `Size=0B`, pas un chemin, tag ou taille non nuls. `Tty` doit être faux ; `OpenStdin` ne peut être vrai que pour l'import. Les workers et exports exigent `StdinOnce=false`. Pour l'import, Docker CLI v29.2.1 configure `StdinOnce=true` avec `--interactive`, tandis que Podman expose false ; le contrôle exige le profil exact du moteur. Champ absent, type invalide, driver différent ou option inattendue bloque avant le callback de claim et avant start. Le chemin public de compatibilité `Runtime::execute_retained` vérifie aussi ce profil avant start.

La lecture de configuration est typée et limitée à 256 KiB. Elle ne désérialise pas `Config.Env`, n'écrit pas l'inspect brut dans un diagnostic et ne lit pas `docker logs`. `start --attach` reste le canal de capture ; les règles existantes de masquage avant audit ne changent pas.

## Compatibilité

La structure sérialisée et le domaine `ai-guard:runtime-config:v1\0` restent inchangés pour les anciens digests. Le golden indépendant de test v1 vaut `694ac612677265f6f82c2f02c30d3ff752bfea82541bdd082e65c210639cdf3f`.

Les nouveaux handles et observations privés portent `configuration_fingerprint_version:2`. L'absence de ce champ signifie v1 et reste omise lors d'une nouvelle sérialisation d'un ancien objet. Le digest v2 utilise le domaine `ai-guard:runtime-config:v2\0`, l'ancienne configuration normalisée et les champs de logging/TTY/stdin. Les champs `Env` en sont exclus. Les DTO `RuntimeInspectionV1` et `RuntimeObservationV1` ne gagnent aucun champ.

`worker_spec` reprend le preset durable du worker ; les observations publiées reprennent ce même preset, au lieu d'affirmer systématiquement v1. Le nouveau broker refuse un premier start v1 avant toute nouvelle commande moteur ou claim. Il garde réservation, archive, identité, digests et anciens claims. `load_session` recharge d'abord le worker serveur : un start déjà consommé ne devient pas un nouveau premier start. Les vieux handles restent lisibles/observables/arrêtables/finalisables avec le digest v1. La reconstruction v1 permet ces usages, mais `create_args`, création managée et démarrage d'une instance possédée refusent v1.

## Fichiers

- `src/runtime.rs` : preset du spec, arguments, garde création, inspection du chemin compatibilité, version du digest privé.
- `src/runtime/managed.rs` : lecture versionnée, validation stream/logging, digest v1/v2, tests privés.
- `src/runner/managed.rs` : admission v2, liaison preset/inspection, refus premier start v1, test de politique.
- `tests/runtime_managed.rs`, `tests/runtime_retention.rs`, `tests/runtime_terminal.rs` et `tests/runtime_fixtures/{managed,runner_terminal}.rs` : régressions ciblées et fixtures natives actualisées.

## Vérifications

- PASS : `cargo check --locked --tests`, Rust 1.99.1, avant remise aux tests ciblés.
- PASS : formatage ciblé avec `rustfmt --edition 2024 --config skip_children=true` sur les fichiers possédés.
- Tests comportementaux exécutés par Luna et logs relus : voir tableau ci-dessous ; aucun PASS inféré du check.
- NOT_RUN : Docker/Podman réels pour ce lot, fournisseur réel, TTY interactif, suite globale et matrice E2E.
- PASS revue statique indépendante : `engine_logs_review` a recoupé la projection et le domaine v1 avec le gel327, les handles/observations versionnés, les refus avant claim et les reprises v1. Le dernier delta import `StdinOnce` a été relu séparément avec les sources Docker CLI v29.2.1 et Podman ; aucun défaut bloquant signalé. Les preuves comportementales restent distinctes.

| Commande ciblée | Résultat constaté | Preuve privée |
|---|---|---|
| `cargo test --locked --lib log_policy_tests` après correction du profil import | PASS 3/3 | `/tmp/ai-guard-engine-logs-final-lib-log-policy-post-fix-20261030.log` |
| `cargo test --locked --lib first_start_policy_tests` | PASS 1/1 | `/tmp/ai-guard-engine-logs-final-lib-first-start-policy-20261030.log` |
| `cargo test --locked --test runtime_managed` puis filtre `managed_launch_requires_created_ack_and_keeps_owned_evidence` corrigé | PASS 13 autres tests puis 1/1 filtre corrigé | `/tmp/ai-guard-engine-logs-final-runtime-managed-20261030.log`, `/tmp/ai-guard-engine-logs-final-runtime-managed-single-20261030.log` |
| `cargo test --locked --test runtime_retention` | PASS 1/1 | `/tmp/ai-guard-engine-logs-final-runtime-retention-20261030.log` |
| `cargo test --locked --test runtime_terminal` | PASS 1/1 | `/tmp/ai-guard-engine-logs-final-runtime-terminal-20261030.log` |
| `cargo test --locked --test runtime_security` | PASS 12/12 | `/tmp/ai-guard-engine-logs-final-runtime-security-20261030.log` |
| `cargo test --locked --test runner_recovery` après correction du profil import | PASS 16/16 | `/tmp/ai-guard-engine-logs-final-runner-recovery-post-fix-20261030.log` |
| `cargo test --locked --test runner_terminal_audit` | PASS 1/1 | `/tmp/ai-guard-engine-logs-final-runner-terminal-audit-20261030.log` |
| `cargo clippy --locked --all-targets -- -D warnings` | PASS | `/tmp/ai-guard-engine-logs-final-clippy-final-20261030.log` |
| `cargo fmt --all -- --check` | PASS selon Luna, sortie vide | `/tmp/ai-guard-engine-logs-final-fmt-final-20261030.log` |
| `cargo test --locked --test runtime_log_policy` | NOT_RUN : cible inexistante ; la couverture est dans `log_policy_tests` | `/tmp/ai-guard-engine-logs-runtime-log-policy-20261030.log` |

Le premier passage final de `runtime_managed` avait 13 PASS et 1 FAIL dans le nouveau scénario `legacy_observe` : le test traitait à tort comme terminal un conteneur encore Created après refus v2. Le runtime refusait correctement cette fausse preuve terminale. Le test vérifie maintenant d'abord ce refus et le refus du premier start v1 sans callback/commande moteur, puis modélise séparément l'état terminé d'un ancien worker. Le filtre complet concerné a été rejoué PASS ; aucune modification production n'a été nécessaire pour ce défaut de scénario.

La baseline séparée `/tmp/ai-guard-engine-logs-baseline.md` observe `local` sur Docker Engine 29.9.2, avec rotation native par défaut et 5 octets accessibles au canal logs. Elle ne démontre pas de fuite de secret. Le nouveau lot réduit le canal de persistance moteur, indépendamment des règles de masquage du broker.

## Sources moteur et limites

- Docker : https://docs.docker.com/engine/logging/configure/ documente `--log-driver none`, `HostConfig.LogConfig.Type` et l'absence de sortie `docker logs` avec none.
- Podman : https://docs.podman.io/en/latest/markdown/podman-create.1.html documente séparément le driver none ; aucune qualification Podman réelle n'en est déduite.
- Champs Podman : https://github.com/podman-container-tools/podman/blob/main/libpod/define/container_inspect.go documente `InspectLogConfig` (`Type`, `Config`, `Path`, `Tag`, `Size`) et `Tty`/`OpenStdin`/`StdinOnce`.
- Import Docker : https://github.com/docker/cli/blob/v29.2.1/cli/command/container/opts.go configure `StdinOnce=true` lorsque `OpenStdin && AttachStdin` ; `--interactive` active ces deux derniers flags. Cette différence a été détectée et corrigée par revue source avant qualification moteur, sans modifier le profil batch des workers.

L'API d'admission reste compatible avec un runner de confiance explicite v1 ; la garantie des nouvelles créations vient du broker et de son inspection v2. Ce lot ne qualifie pas une release, un autre moteur/OS/architecture ni un canal de médiation des actions transitives.
