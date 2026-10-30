# Revue indépendante — finalisation terminale et rétention

29 octobre 2026. Revue ciblée du lot P1 gelé : `src/runtime.rs`, `src/runner.rs`, `src/operations.rs`, le mapping de `src/audit.rs`, puis les corrections associées de `src/adapters.rs` et `src/redaction.rs`. Tests relus : runtime_retention, runner_terminal_audit, runtime_uncertainty, runtime_terminal_persistence et redaction_boundaries. Comparaisons directes aux copies avant lot ; aucune édition du dépôt.

**PASS pour ce lot gelé. Les défauts reproduits ci-dessous sont traités ; aucun nouveau P0/P1 bloquant identifié dans ce périmètre.** Ce verdict ne clôture pas la qualification du produit, les quotas/consommation de lancement, ni la reprise automatisée après crash.

## Constats reproduits et fermeture

| Constat | Preuve négative examinée | Correction et vérification |
| --- | --- | --- |
| Suppression avant accusé terminal : l'audit refusé perdait l'instance normalement inspectable. | `/tmp/ai-guard-runner-terminal-red.log` : parcours CLI/gateway en échec sur ordre terminal/rm et rétention. | `execute_retained`, token privé sans Drop destructif ; `client.post_event(completed)` précède le cleanup explicite (`src/runner.rs:752`). Régression CLI et réel terminal HTTP503 PASS. |
| Réponse create inutilisable puis nom réattribué : stop/rm visait le successeur. | `/tmp/ai-guard-runtime-retention-invalid-reference-red.log` : six échecs empty/malformed/truncated × retained/legacy ; traces inspect/kill et rm legacy sur le nom substitué. | Token attribué uniquement après référence validée (`src/runtime.rs:595`). Une sortie inutilisable retourne une erreur sans instance adoptée, avec ownership/state unknown. Aucun autre appel CLI. Six cas corrigés inclus dans les 14 sous-cas GREEN. |
| Résultat terminal connu perdu si cleanup puis audit de son erreur échouent, ou si le post-traitement reçoit 503. | `/tmp/ai-guard-cleanup-audit-red.log`, `/tmp/ai-guard-terminal-postprocessing-secret-red.log` : JSON de résultat perdu malgré terminal déjà enregistré. | `RunSummary` conserve exit/timeout, distingue `cleanup.error_audit` et `postprocessing_error`. Audit cleanup non acquitté : changed_files=None, aucun helper ultérieur. Les observations ont leur propre Result ; le CLI imprime le résumé puis termine non zéro. |
| Une erreur rm ne prouvait pas que l'instance existait encore. | Fixture `cleanup-response-lost` : suppression effectuée puis exit125. | `cleanup.state=unconfirmed`, jamais retained sur cette seule erreur ; audit d'erreur acknowledged/unacknowledged explicite. Cas compris dans le GREEN CLI. |
| Secret connu réaffiché dans l'erreur CLI ; fragments persistés après troncature de sortie/projection ; clé JSON non masquée. | `/tmp/ai-guard-terminal-postprocessing-secret-red.log`, `terminal-output-secret-red.log`, `adapter-secret-boundary-red.log`, `redaction-json-keys-red.log` sous `/tmp/ai-guard-` : canaries détectées sans afficher leur valeur. | Chaîne d'erreur complète expurgée avant nouvel anyhow et avant clip (`src/runner.rs:487`) ; stdout/stderr expurgés avant limite8192 ; JSON décodé expurgé avant projection4096 (`src/adapters.rs:43`) ; clés connues expurgées (`src/redaction.rs:91`). Neuf sous-cas CLI et deux régressions clés/valeurs échappées PASS. |

Les cas à canaries utilisent des credentials de fixtures créées pour le test. Les preuves lues ne publient aucune valeur. Ces constats n'établissent pas qu'un worker dispose d'un accès au moteur : la substitution est un scénario de concurrence/ownership du contrôle, reproduit avec un CLI Rust factice.

## Invariants examinés

- `CreatedInstance` et ses champs sont privés, sans Clone/Deserialize/Default. `RetainedExecution` n'offre qu'un résultat emprunté et un nom diagnostique (`src/runtime.rs:299`). Aucun Drop n'arrête ou ne supprime une instance.
- Pour une référence acquittée, start/inspect/kill/rm utilisent la référence retournée par create, pas l'alias demandé. Les tests substituent l'alias et préservent l'autre instance. Un create en erreur ou ambigu ne donne aucun droit d'adoption par nom.
- Le résultat terminal exige les états/timestamps et champs observés requis avant retour. Un échec d'exécution produit `runtime.execution.error`, statut projeté unknown (`src/audit.rs:801`), sans fabriquer un terminal métier, une durée ou un exit zéro.
- Sur erreur après création identifiée, l'arrêt ciblé est tenté puis réinspecté. Une inspection insuffisante reste « stop unknown ». Le token n'est pas supprimé implicitement lors d'un audit refusé ou d'un abandon après résultat terminal.
- L'ancien wrapper `execute` conserve son cleanup pour les helpers. Le worker métier du runner utilise `execute_retained`. La durabilité de l'accusé est appliquée dans ce parcours, pas prétendue par le token seul.
- `parse_redacted_line` vérifie la borne64KiB et parse JSON avant `Redactor.value` puis projection. Il ne fait pas de substitutions regex sur le JSON sérialisé. La valeur canary avec quotes/backslash reste valide et normalisée après masquage ; l'ancien parse_line conserve son comportement.
- Les erreurs de cleanup et de post-traitement rendent le CLI non zéro tout en laissant le résultat observé disponible (`src/operations.rs:231`). Les inconnu/non acquitté ne sont pas présentés comme zéro ou comme succès global.

## Vérifications finales examinées

| Statut | Commande / preuve fournie | Résultat constaté |
| --- | --- | --- |
| PASS | `cargo fmt --all -- --check`, `/tmp/ai-guard-p1-security-fmt.log` | Log vide ; exit0 constaté et communiqué par root. |
| PASS | `cargo clippy --locked --all-targets -- -D warnings`, `p1-security-clippy.log` | Fin réussie en2.39s. |
| PASS logiciel | Suite complète `cargo test --locked`, `p1-security-tests.log` | 147 PASS, 0 FAIL, 3 ignored. Les trois tests moteurs ignorés ne sont pas comptés PASS dans cette commande. |
| PASS | `cargo build --release --locked`, `p1-security-release.log` | Build réussi en17.04s. |
| PASS réel ciblé | `/tmp/ai-guard-p1-security-real-terminal.log` | 1 PASS/0 FAIL/0 ignored en1.25s : vrai Docker, gateway Rust SQLite neuve et middleware terminal503 ; même instance exited0 conservée, dates correspondant à l'observation rejetée, aucun faux terminal stocké, intégrité avant/après réouverture SQLite. |
| PASS source/probes fournis | `/tmp/ai-guard-runtime-retention-invalid-reference-green.log`, `/tmp/ai-guard-terminal-secrets-all-green.log`, suite complète | 14 sous-cas rétention et neuf sous-cas CLI ; deux cas redaction_boundaries inclus dans la suite finale. Source des fixtures et branches visées relus indépendamment. |
| NOT_RUN par le réviseur | Exécution personnelle des probes de code cible et des moteurs | Aucun Docker/Podman, endpoint ou code cible exécuté par moi. Revue des preuves root/auteur, conformément au cadrage final du parent. |

Les références de logs abrégées dans le tableau sont sous `/tmp/ai-guard-`. Le dernier test réel est un test d'adaptateur/runner avec un gateway SQLite réel local et injection de panne HTTP ; il n'est pas une qualification de la nouvelle image des services conteneurisés. Aucun succès de l'ancien candidat n'est transféré à l'image finale.

## Limites conservées

La référence create est opaque et l'autorité CLI est supposée de confiance. Le token ne lie pas encore un endpoint/contexte de daemon immuable et n'est pas une tentative persistée avant create. Un acquittement create perdu reste ambigu, sans récupération automatique. Le cleanup n'est pas une opération durable idempotente.

Annulation/crash en phase active : aucune garantie d'arrêt asynchrone ou de reprise persistée n'est livrée par l'absence de Drop. Registre, quotas/réservations consommées, réconciliation, métriques terminales et reprise demeurent ouverts. Une conservation par ce code ne protège pas d'une suppression externe. Les tests à CLI factice ne qualifient aucun moteur ; le réel ciblé ne qualifie ni Podman, rootless, Kubernetes, Linux x86_64 ni trois workers/tenants.

Le masquage vérifié porte sur secrets connus et règles effectivement implémentées, avec frontières et encodages JSON testés. Ce n'est pas une garantie universelle de détection de toutes les données sensibles ou représentations arbitraires.

Skill chargé : [security-audit](/Users/louis/.agents/skills/security-audit/SKILL.md), guidance de revue ciblée. Sa règle d'exécution exige « only inside an OS-enforced sandbox ». Le profil complet n'a pas été établi pour un probe personnel ; je n'ai pas contourné ce profil. Le parent a ensuite explicitement retenu la revue source et la lecture des reproductions fournies. Les contrats AGENTS/RTK, architecture/coding/readiness/tests ont été relus ; pas d'audit exhaustif ou de couverture globale revendiquée.

## Identité du code gelé

Empreintes recalculées par le réviseur et identiques au manifeste `/tmp/ai-guard-p1-security-source-sha.txt` :

- runtime : `acd84dfb42d316bbb6536983f7d220dec7bf3e312c4b6a8e61e252cf04fe05b4`.
- runner : `f7216790a3acb5d48b40b63f8f1e38dc3864839fba78c00a102102fbea7a95df`.
- operations : `2fc151e6f013cad5d73316339f2c8e68ba8f85a64ecfe8ec557907a14b69d84d`.
- audit : `7ac70d668c0522bdbe5623be448bc7fe4011777bc1cf9efcfd30ae38124280b7`.
- adapters : `1c203189a1430a6facf75bc4c2bbf2b6a9c9fc1ce0b24f16146242c74ca6c5cc`.
- redaction : `88800feca3fe740f732dcee0aeec583c7d9c463f79c6fcf8713d293ec0a1a505`.

Les cinq empreintes de tests relus ont également été comparées au même manifeste. Les autres changements du checkout, notamment API et infrastructure, restent hors de ce verdict sauf les contrats appelés nécessaires à ces preuves.
